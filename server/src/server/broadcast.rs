//! Broadcast / send helpers: per-peer send, stream relay, `Inner::out_msgs`
//! flush, admin-state broadcasts (the `admin_state` record), the kick and
//! server-closing paths.
//!
//! Sends go through the v5 transport (`crate::net`). See
//! docs/development/server-modules.md.

use super::*;
use hsmp_net::net::close_code;


/// Send everything queued in `Inner::out_msgs` (lock released first).
pub(super) async fn flush_out(socket: &UdpSocket, state: &Arc<ServerState>) {
    let (recs, addrs) = {
        let mut inner = state.inner.lock().await;
        if inner.out_msgs.is_empty() { return; }
        (std::mem::take(&mut inner.out_msgs), inner.peers.keys().copied().collect::<Vec<_>>())
    };
    for (to, m) in recs {
        match to {
            Some(a) => send_msg_to(socket, state, a, m).await,
            None => broadcast_msg(socket, state, &addrs, m).await,
        }
    }
}

pub(crate) async fn broadcast_admin_state(socket: &UdpSocket, state: &Arc<ServerState>, addrs: &[SocketAddr]) {
    // Per recipient (admin.rs `admin_state_for`): banned IPs are private, an
    // admin gets them masked, everyone else gets none.
    let msgs: Vec<(SocketAddr, Vec<u8>)> = {
        let inner = state.inner.lock().await;
        addrs.iter().map(|a| (*a, admin_state_for(&inner, *a))).collect()
    };
    for (addr, m) in msgs {
        send_msg_to(socket, state, addr, m).await;
    }
}

/// Send already-sealed datagrams (outside every lock).
pub(crate) async fn send_out(socket: &UdpSocket, state: &ServerState, out: crate::net::Out) {
    let pf = crate::perf::perf();
    for (addr, dg) in out {
        if socket.send_to(&dg, addr).await.is_ok() {
            pf.sent(dg.len());
            // Reliable/control traffic is never dropped, but it counts against
            // the client's budget so the stream relays leave room for it.
            state.relay.charge(addr, dg.len());
        }
    }
}

/// Tell `addr` it is kicked (reliable event) and close its connection with
/// `KICKED`. The caller still removes the peer (`peer_leave`).
pub(crate) async fn kick(socket: &UdpSocket, state: &Arc<ServerState>, addr: SocketAddr, reason: &str, retry_after_s: u32) {
    let code = if reason == "banned" { hsmp_net::net::handshake::reject_code::BANNED } else { 0 };
    let ev = kicked_msg(rand::random::<u32>() | 1, code, reason, retry_after_s);
    // The event is flushed in its own packet before the close starts: a
    // closing connection only sends CLOSE chunks. If the event is lost the
    // KICKED close code alone is terminal for the client.
    let mut out = state.net.send_msg(addr, ev);
    out.extend(state.net.close(addr, close_code::KICKED, reason));
    send_out(socket, state, out).await;
}

/// Graceful shutdown (ctrl-c / RCON SHUTDOWN): every client gets
/// `S2CServerClosing` and a `SERVER_CLOSING` close, so it shows "server
/// closed" instead of timing out.
pub async fn shutdown(socket: &UdpSocket, state: &Arc<ServerState>, reason: u8, text: &str) {
    let msg = closing_msg(reason, text, 0);
    let mut out = Vec::new();
    for addr in state.net.addrs() {
        out.extend(state.net.send_msg(addr, msg.clone()));
        out.extend(state.net.close(addr, close_code::SERVER_CLOSING, text));
    }
    send_out(socket, state, out).await;
    // The CLOSE chunk is repeated three times, 30 ms apart.
    for _ in 0..4 {
        tokio::time::sleep(Duration::from_millis(31)).await;
        let (out, _) = state.net.tick();
        send_out(socket, state, out).await;
    }
    super::dispatch::log_transport_stats(state);
}

/// A `pose` record from `src` (peer `sid`), relayed by the rate plan. `msg` is the
/// incoming message with `peer` already patched to `sid`; each receiver gets it with `aux` =
/// the pair's relay interval (ms; patched per receiver, no re-encode). Lag comp learns which
/// frames (`ts`) each receiver was sent.
pub(super) async fn relay_pose(socket: &UdpSocket, state: &Arc<ServerState>, src: SocketAddr, sid: PeerId, msg: Vec<u8>, ts: u32) {
    let Some(mode) = proto::record_mode(hsmp_ipc::schema::pose::K_POSE, sid) else { return };
    let dsts = state.net.addrs();
    let wire = msg.len() + crate::relay::SEAL_OVERHEAD;
    let chosen = state.relay.select(src, &dsts, crate::relay::Stream::Skel, wire);
    let pf = crate::perf::perf();
    for crate::relay::Pick { dst, interval_ms } in chosen {
        if let Some(v) = state.relay.peer_id(&dst) {
            crate::lagcomp::note_relayed(v, sid, ts, interval_ms);
        }
        let mut b = msg.clone();
        hsmp_ipc::wire::set_aux(&mut b, interval_ms);
        for (a, dg) in state.net.send_bytes(dst, mode, b) {
            if socket.send_to(&dg, a).await.is_ok() { pf.sent(dg.len()); }
        }
    }
}

/// A record message (`peer` already patched to its owner) to every relevant peer within its
/// budget: one copy per destination, the last one takes the buffer.
pub(super) async fn relay_record(socket: &UdpSocket, state: &Arc<ServerState>, src: SocketAddr,
                      stream: crate::relay::Stream, mut msg: Vec<u8>) {
    let Ok((h, _)) = hsmp_ipc::wire::split(&msg) else { return };
    let Some(mode) = proto::record_mode(h.kind, h.peer) else { return };
    let dsts = state.net.addrs();
    let wire = msg.len() + crate::relay::SEAL_OVERHEAD;
    let chosen = state.relay.select(src, &dsts, stream, wire);
    let pf = crate::perf::perf();
    let n = chosen.len();
    for (i, crate::relay::Pick { dst, .. }) in chosen.into_iter().enumerate() {
        let bytes = if i + 1 == n { std::mem::take(&mut msg) } else { msg.clone() };
        for (a, dg) in state.net.send_bytes(dst, mode, bytes) {
            if socket.send_to(&dg, a).await.is_ok() { pf.sent(dg.len()); }
        }
    }
}

#[cfg(test)]
mod tests {
    /// The per-receiver patching of a relayed pose record: peer and aux are the only bytes
    /// that change; the record still validates.
    #[test]
    fn pose_relay_patching_keeps_the_record() {
        let frame = hsmp_pose::posecodec::v2::encode(&hsmp_pose::posecodec::v2::Full::default());
        let head = hsmp_ipc::schema::pose::PoseHead { tick: 9, n: 0, _r: 0 };
        let mut m = hsmp_ipc::wire::encode(0, 0, &head, &frame);
        let orig = m.clone();
        hsmp_ipc::wire::set_peer(&mut m, 5);
        hsmp_ipc::wire::set_aux(&mut m, 33);
        assert_eq!(&m[8..], &orig[8..]);
        let (h, v) = hsmp_ipc::wire::decode::<hsmp_ipc::schema::pose::PoseHead>(&m).unwrap();
        assert_eq!((h.peer, h.aux, v.head.tick, &v.rows[..]), (5, 33, 9, &frame[..]));
    }
}
