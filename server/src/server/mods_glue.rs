//! Server mods on the server (docs/hosting/server-mods.md; the set and the rate limits are in
//! `crate::server_mods`). A player who joins a server with mods is **pending** until its
//! game reports the set loaded (`mod_ready`):
//!
//! * admission refuses a client without `caps::SERVER_MODS` (`MODS_REQUIRED`), and sends
//!   `mod_manifest` + `mod_files` right after the welcome (again on a session resume);
//! * a pending player is not in the roster others see, cannot READY or start anything, is
//!   not a participant of a match that starts, and only its mod records, `leave`, `ping`,
//!   `command` (answered with a refusal) and its loadout records are taken; everything else
//!   it sends is dropped (`gate`);
//! * its chunk requests are served by [`serve_loop`] within the per-player and server-wide
//!   rates; a player still pending after `--mods-timeout-s`, or over its byte budget, is
//!   disconnected with the reason.

use super::*;
use crate::server_mods::{Host, Req};
use hsmp_ipc::record::view;
use hsmp_ipc::schema::mods as rec;
use hsmp_ipc::schema::session as srec;
use std::sync::atomic::Ordering;

/// Serve loop period.
const SERVE_EVERY: Duration = Duration::from_millis(10);
/// Chunks one serve pass sends at most (all players together).
const SERVE_MAX: usize = 64;

pub(crate) fn host(state: &ServerState) -> Option<&Arc<Host>> {
    state.mods.get()
}

/// Is `id` waiting for its mods?
pub(crate) fn is_pending(inner: &Inner, id: PeerId) -> bool {
    inner.mods_pending.contains_key(&id)
}

/// A peer was removed: forget its pending state.
pub(crate) fn forget(inner: &mut Inner, id: PeerId) {
    inner.mods_pending.remove(&id);
}

/// Admission: the reject for a client that cannot take this server's mods, if any.
pub(super) fn admit_check(state: &ServerState, caps: u64) -> Option<(u8, String)> {
    let h = host(state)?;
    if caps & hsmp_net::net::caps::SERVER_MODS != 0 {
        return None;
    }
    Some((hsmp_net::net::handshake::reject_code::MODS_REQUIRED, format!(
        "This server uses {} server mod{}. Update HalfSword-MP via the launcher to join.",
        h.built.manifest.mods.len(), if h.built.manifest.mods.len() == 1 { "" } else { "s" })))
}

/// A new peer joined (under the game lock): it is pending.
pub(super) fn on_joined_locked(state: &ServerState, inner: &mut Inner, id: PeerId) {
    if let Some(h) = host(state) {
        inner.mods_pending.insert(id, inner.now_ms.max(state.net.now_ms()));
        h.mark_pending();
    }
}

/// After the welcome of a pending peer (join or resume): the manifest. A resumed peer asks
/// for its chunks again on the new connection.
pub(super) async fn after_welcome(socket: &Arc<UdpSocket>, state: &Arc<ServerState>, addr: SocketAddr, id: PeerId, resumed: bool) {
    let Some(h) = host(state) else { return };
    if !is_pending(&*state.inner.lock().await, id) {
        return;
    }
    if resumed {
        h.clear_queue(id);
    }
    let mut out = state.net.send_msg(addr, h.manifest_msg.clone());
    out.extend(state.net.send_msg(addr, h.files_msg.clone()));
    send_out(socket, state, out).await;
    info!(peer_id = id, mods = h.built.manifest.mods.len(), bytes = h.built.manifest.total_bytes(), resumed,
          set = %h.built.manifest.set_hash_hex(), "server mods offered");
}

/// Kinds a pending peer may send.
fn allowed_while_pending(kind: u16) -> bool {
    rec::is_mods_kind(kind)
        || matches!(kind, srec::K_LEAVE | srec::K_PING | srec::K_COMMAND)
        || matches!(kind, hsmp_ipc::schema::loadout::K_KIT | hsmp_ipc::schema::loadout::K_LOADOUT | hsmp_ipc::schema::loadout::K_BODY)
}

/// The records gate: true = drop this record (its sender is pending). Costs one atomic load
/// while nobody is pending.
pub(super) async fn gate(state: &Arc<ServerState>, from: SocketAddr, kind: u16) -> bool {
    let Some(h) = host(state) else { return false };
    if !h.any_pending.load(Ordering::Acquire) || allowed_while_pending(kind) {
        return false;
    }
    let inner = state.inner.lock().await;
    let pending = inner.peers.get(&from).is_some_and(|p| is_pending(&inner, p.id));
    if pending {
        GATED.fetch_add(1, Ordering::Relaxed);
        if crate::validate::rate::log_ok("mods_gate") {
            debug!(%from, kind, "record from a player still loading the server's mods; dropped");
        }
    }
    pending
}

/// Records dropped by the gate (stats).
pub(crate) static GATED: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// `mod_chunk_req` and `mod_ready` from a client.
pub(super) async fn handle_record(socket: &Arc<UdpSocket>, state: &Arc<ServerState>, from: SocketAddr, h: hsmp_ipc::wire::WireHdr,
                                  payload: &[u8]) -> anyhow::Result<()> {
    // Validate first, whoever sent it (a fuzzer reaches only the validator).
    hsmp_ipc::schema::check_payload(h.kind, payload).map_err(refused)?;
    let Some(host) = host(state).cloned() else { return Ok(()) };
    let (id, pending) = {
        let inner = state.inner.lock().await;
        match inner.peers.get(&from) {
            Some(p) => (p.id, is_pending(&inner, p.id)),
            None => return Ok(()),
        }
    };
    match h.kind {
        rec::K_MOD_CHUNK_REQ => {
            if !pending {
                return Ok(()); // loaded already (or never asked): nothing to serve
            }
            let r = view::<rec::ModChunkReq>(payload).map_err(refused)?.head();
            match host.request(id, &r, state.net.now_ms()) {
                Req::Queued => {}
                Req::Refused(why) => debug!(peer_id = id, why, file = r.file, offset = r.offset, "mod chunk request refused"),
                Req::Abuse(why) => {
                    warn!(peer_id = id, why, "server mods: disconnecting the player");
                    kick(socket, state, from, why, 0).await;
                    let _ = peer_leave(socket, state, from).await;
                }
            }
        }
        rec::K_MOD_READY => {
            let r = view::<rec::ModReady>(payload).map_err(refused)?.head();
            let ours = r.set_hash == host.set_hash();
            let mut inner = state.inner.lock().await;
            let since = inner.mods_pending.get(&id).copied();
            let ms = since.map(|s| inner.now_ms.saturating_sub(s));
            match (r.result, ours, since) {
                (rec::mod_result::LOADED, true, Some(_)) => {
                    inner.mods_pending.remove(&id);
                    inner.match_state_dirty = true; // the roster now lists the player
                    drop(inner);
                    host.retain(|p| p != id);
                    info!(peer_id = id, ms, failed = r.failed, "server mods loaded: the player joins");
                    crate::events::emit("server_mods", serde_json::json!({"peer_id": id, "result": "loaded", "ms": ms, "failed": r.failed}));
                }
                (rec::mod_result::LOADED, false, _) => {
                    drop(inner);
                    warn!(peer_id = id, "mod_ready for another mod set; ignored");
                }
                (rec::mod_result::LOADED, true, None) => {}
                (res, _, _) => {
                    drop(inner);
                    let what = if res == rec::mod_result::DECLINED { "declined" } else { "failed" };
                    info!(peer_id = id, ms, result = what, "server mods {what} by the player's game");
                    crate::events::emit("server_mods", serde_json::json!({"peer_id": id, "result": what, "ms": ms, "failed": r.failed}));
                }
            }
        }
        _ => {}
    }
    Ok(())
}

/// Every 10 ms while the server has mods: serve due chunks; disconnect players still pending
/// after the timeout; forget the transfer state of players that loaded or left.
pub async fn serve_loop(socket: Arc<UdpSocket>, state: Arc<ServerState>) {
    let Some(host) = host(&state).cloned() else { return };
    let mut iv = time::interval(SERVE_EVERY);
    iv.set_missed_tick_behavior(time::MissedTickBehavior::Delay);
    let timeout_ms = host.cfg.timeout_s as u64 * 1000;
    loop {
        iv.tick().await;
        if !host.any_pending.load(Ordering::Acquire) {
            continue;
        }
        let now = state.net.now_ms();
        let (addrs, late, none) = {
            let inner = state.inner.lock().await;
            let addrs: HashMap<PeerId, SocketAddr> = inner.peers.iter()
                .filter(|(_, p)| is_pending(&inner, p.id)).map(|(a, p)| (p.id, *a)).collect();
            let late: Vec<SocketAddr> = inner.mods_pending.iter()
                .filter(|(_, since)| now.saturating_sub(**since) > timeout_ms)
                .filter_map(|(id, _)| addrs.get(id).copied()).collect();
            (addrs, late, inner.mods_pending.is_empty())
        };
        host.retain(|id| addrs.contains_key(&id));
        if none {
            // Cleared under no lock race: a new pending peer sets it again after its insert.
            let inner = state.inner.lock().await;
            if inner.mods_pending.is_empty() {
                host.any_pending.store(false, Ordering::Release);
            }
            continue;
        }
        for addr in late {
            warn!(%addr, timeout_s = host.cfg.timeout_s, "server mods not loaded in time: disconnecting the player");
            kick(&socket, &state, addr, &format!("The server's mods were not loaded within {} s", host.cfg.timeout_s), 0).await;
            let _ = peer_leave(&socket, &state, addr).await;
        }
        let due = host.take_due(now, SERVE_MAX);
        if due.is_empty() {
            continue;
        }
        let mut flush: Vec<SocketAddr> = Vec::new();
        for (id, r, msg) in due {
            let Some(addr) = addrs.get(&id).copied() else { continue };
            let mode = hsmp_net::net::SendMode::Reliable;
            if state.net.queue_bytes(addr, mode, msg) {
                if !flush.contains(&addr) { flush.push(addr); }
            } else {
                host.retry(id, r); // backpressure: first in line again, tokens refunded
            }
        }
        for addr in flush {
            let out = state.net.flush(addr);
            send_out(&socket, &state, out).await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::dispatch::resume_tests::{seed, TClient};
    use super::*;
    use crate::server_mods::{manifest, Config};
    use hsmp_net::net::{Client, ClientConfig, ConnConfig};

    fn with_mods(cfg: Config) -> (Arc<ServerState>, Arc<Host>) {
        let d = manifest::tests::mods_dir(&[("arena", "Scripts/main.lua", &[7u8; 70_000]), ("arena", "mod.json", br#"{"author":"Ann"}"#)]);
        let (b, _) = manifest::build_from_dir(&d, &manifest::Limits::PROTOCOL).unwrap();
        let _ = std::fs::remove_dir_all(&d);
        let net = crate::net::Net::with_caps(rand::random(), None, hsmp_net::net::caps::SERVER_MODS);
        let state = Arc::new(ServerState::with_net(8, net));
        let host = Arc::new(Host::new(cfg, b));
        let _ = state.mods.set(host.clone());
        (state, host)
    }

    /// A client that offers SERVER_MODS (as the sidecar does).
    async fn join_modded(socket: &Arc<UdpSocket>, state: &Arc<ServerState>, s: u8) -> TClient {
        let sock = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let addr = sock.local_addr().unwrap();
        let mut cfg = ClientConfig::new([s; 32], "m");
        cfg.caps |= hsmp_net::net::caps::SERVER_MODS;
        let c = Client::new(cfg, ConnConfig::default(), state.net.now_ms(), &mut rand::rngs::OsRng);
        let mut t = TClient { c, sock, addr, records: Vec::new() };
        for _ in 0..100 {
            t.pump(socket, state).await;
            if t.c.is_connected() && t.welcome_id().is_some() && !t.recs(rec::K_MOD_FILES).is_empty() { break; }
        }
        assert!(t.welcome_id().is_some(), "no welcome");
        t
    }

    async fn deliver(socket: &Arc<UdpSocket>, state: &Arc<ServerState>, from: SocketAddr, msg: &[u8]) -> anyhow::Result<()> {
        let (h, p) = hsmp_ipc::wire::split(msg).unwrap();
        super::super::records::handle(socket, state, from, h, p).await
    }

    /// A client without the capability is refused with a readable reason; one with it gets
    /// the manifest right after the welcome and stays out of the roster, READY and START
    /// until it reports the set loaded.
    #[tokio::test]
    async fn join_is_gated_until_the_mods_are_loaded() {
        let (state, host) = with_mods(Config::default());
        let socket = Arc::new(UdpSocket::bind("127.0.0.1:0").await.unwrap());
        state.inner.lock().await.next_peer_id = 81_001;
        // no SERVER_MODS: an authenticated MODS_REQUIRED reject
        let sock = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let addr = sock.local_addr().unwrap();
        let c = Client::new(ClientConfig::new([seed(81_009); 32], "old"), ConnConfig::default(), state.net.now_ms(), &mut rand::rngs::OsRng);
        let mut old = TClient { c, sock, addr, records: Vec::new() };
        let mut rejected = None;
        let mut buf = [0u8; 2048];
        for _ in 0..60 {
            tokio::time::sleep(Duration::from_millis(5)).await;
            let now = state.net.now_ms();
            while let Ok((n, _)) = old.sock.try_recv_from(&mut buf) { old.c.handle(now, &buf[..n]); }
            while let Some(ev) = old.c.poll_event() {
                if let hsmp_net::net::ClientEvent::Rejected { code, text, authenticated } = ev { rejected = Some((code, text, authenticated)); }
            }
            if rejected.is_some() { break; }
            while let Some(dg) = old.c.poll_transmit(now) {
                let (act, out) = state.net.handle(old.addr, &dg);
                for (_, r) in out { old.c.handle(now, &r); }
                if let crate::net::Action::Auth(p) = act { super::super::dispatch::admit(&socket, &state, p).await; }
            }
        }
        let (code, text, auth) = rejected.expect("the old client was not refused");
        assert_eq!(code, hsmp_net::net::handshake::reject_code::MODS_REQUIRED);
        assert!(auth && text.contains("server mod") && text.contains("Update"), "{text}");
        assert!(state.inner.lock().await.peers.is_empty());

        // with the capability: the manifest, then pending
        let mut a = join_modded(&socket, &state, seed(81_001)).await;
        let aid = a.welcome_id().unwrap();
        let m = a.recs(rec::K_MOD_MANIFEST);
        let f = a.recs(rec::K_MOD_FILES);
        assert_eq!((m.len(), f.len()), (1, 1));
        let ann = manifest::from_payloads(&m[0], &f[0], &manifest::Limits::PROTOCOL).unwrap();
        assert_eq!(ann.manifest.set_hash, host.set_hash());
        assert_eq!(ann.manifest.mods[0].author, "Ann");
        // a second, ordinary player is in the lobby
        let mut b = join_modded(&socket, &state, seed(81_002)).await;
        let bid = b.welcome_id().unwrap();
        {
            let mut i = state.inner.lock().await;
            assert!(is_pending(&i, aid) && is_pending(&i, bid));
            i.mods_pending.remove(&bid); // B loaded its mods earlier
        }
        // A cannot READY (refused with a result), and its game records are dropped
        let ready = hsmp_ipc::wire::encode(0, 0, &srec::Command { flag: true.into(), ..srec::Command::new(5, srec::cmd_op::READY) }, &[]);
        deliver(&socket, &state, a.addr, &ready).await.unwrap();
        a.pump(&socket, &state).await;
        let res = a.recs(srec::K_CMD_RESULT);
        let r = view::<srec::CmdResult>(&res[0]).unwrap().head();
        assert!(!r.ok.get() && r.reason_text.lossy().contains("mods"), "{:?}", r.reason_text.lossy());
        let gs = hsmp_ipc::wire::encode(0, 0, &srec::GameStatus { flags: srec::status_flag::IN_MENU, ..Default::default() }, &[]);
        deliver(&socket, &state, a.addr, &gs).await.unwrap();
        assert!(!state.inner.lock().await.match_peers.contains_key(&aid), "game_status of a pending player is dropped");
        assert!(GATED.load(Ordering::Relaxed) >= 1);
        // the roster lists B only; START by B (alone) does not take A into the match
        {
            let mut i = state.inner.lock().await;
            let snap = session::build_session(&i, 1000);
            assert!(snap.rows.iter().all(|r| r.peer_id != aid) && snap.rows.iter().any(|r| r.peer_id == bid));
            let o = session::start_match(&mut i, false, "test");
            assert!(o.ok, "{}", o.text);
            let akey = session::peer_key(i.peers.values().find(|p| p.id == aid).unwrap());
            assert!(!i.participants.contains(&akey), "a pending player is not a participant");
            assert!(!i.peers.values().find(|p| p.id == aid).unwrap().alive);
        }
        // mod_ready for another set is ignored; for ours: A joins
        let other = hsmp_ipc::wire::encode(0, 0, &rec::ModReady { set_hash: [9; 32], result: rec::mod_result::LOADED, failed: 0, _r: [0; 6] }, &[]);
        deliver(&socket, &state, a.addr, &other).await.unwrap();
        assert!(is_pending(&*state.inner.lock().await, aid));
        let ok = hsmp_ipc::wire::encode(0, 0, &rec::ModReady { set_hash: host.set_hash(), result: rec::mod_result::LOADED, failed: 0, _r: [0; 6] }, &[]);
        deliver(&socket, &state, a.addr, &ok).await.unwrap();
        let i = state.inner.lock().await;
        assert!(!is_pending(&i, aid));
        assert!(session::build_session(&i, 2000).rows.iter().any(|r| r.peer_id == aid), "listed once loaded");
        drop(i);
        b.pump(&socket, &state).await;
    }

    /// Chunks are served by the serve loop (not the receive path), from the announced bytes;
    /// a pending player is disconnected after the timeout with the reason. (The rates and the
    /// byte budget: `server_mods::tests`.)
    #[tokio::test]
    async fn chunks_are_served_within_budget_and_timeouts_kick() {
        let (state, host) = with_mods(Config { peer_rate_bps: 256 << 10, timeout_s: 1, ..Config::default() });
        let socket = Arc::new(UdpSocket::bind("127.0.0.1:0").await.unwrap());
        state.inner.lock().await.next_peer_id = 82_001;
        tokio::spawn(serve_loop(socket.clone(), state.clone()));
        let mut a = join_modded(&socket, &state, seed(82_001)).await;
        // ask for the whole 70 000-byte main.lua in 3 chunks (the files are in name order:
        // Scripts/main.lua is row 0)
        let ann = manifest::from_payloads(&a.recs(rec::K_MOD_MANIFEST)[0], &a.recs(rec::K_MOD_FILES)[0], &manifest::Limits::PROTOCOL).unwrap();
        let main = ann.manifest.flat().iter().position(|(_, f)| f.path == manifest::MAIN).unwrap() as u16;
        for (i, off) in [0u64, 32768, 65536].iter().enumerate() {
            let r = hsmp_ipc::wire::encode(0, 0, &rec::ModChunkReq { offset: *off, req_id: i as u32 + 1, len: 32768, file: main, _r: [0; 6] }, &[]);
            deliver(&socket, &state, a.addr, &r).await.unwrap();
        }
        let mut got = vec![0u8; 70_000];
        let mut n = 0;
        for _ in 0..200 {
            // the transport timer (paced fragments, retransmits) runs in recv_loop in the server
            let (out, _) = state.net.tick();
            send_out(&socket, &state, out).await;
            a.pump(&socket, &state).await;
            for c in a.recs(rec::K_MOD_CHUNK).iter().skip(n) {
                let v = view::<rec::ModChunkHead>(c).unwrap();
                got[v.head.offset as usize..v.head.offset as usize + v.rows.len()].copy_from_slice(&v.rows);
                n += 1;
            }
            if n == 3 { break; }
        }
        assert_eq!(n, 3, "all chunks served");
        assert_eq!(manifest::sha256(&got), ann.manifest.flat()[main as usize].1.sha256);
        // the player never loads: disconnected after the 1 s timeout with the reason
        for _ in 0..100 {
            tokio::time::sleep(Duration::from_millis(20)).await;
            a.pump(&socket, &state).await;
            if !state.inner.lock().await.peers.contains_key(&a.addr) { break; }
        }
        assert!(!state.inner.lock().await.peers.contains_key(&a.addr), "a pending player is disconnected after the timeout");
        let k = a.recs(srec::K_KICKED);
        assert!(view::<srec::Kicked>(&k[0]).unwrap().head().reason.lossy().contains("not loaded"));
        let _ = host;
    }
}
