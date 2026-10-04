//! UDP recv loop and per-message dispatch (every message is a record: `records::handle`), the v5
//! admission (`admit`: ban / capacity / duplicate-key checks, welcome,
//! roster) and the transport timer.
//!
//! Transport: the wire protocol (docs/development/protocol.md).

use super::*;
use hsmp_net::net::handshake::reject_code;
use hsmp_net::net::{close_code, ConnState, PendingAuth};
use proto::v5;

/// Transport timer period: acks, retransmits, keepalives, CLOSE repeats.
const TRANSPORT_TICK: Duration = Duration::from_millis(5);
/// Liveness refresh of `PeerState::last_seen_ms` from authenticated
/// packets, at most once per this long per peer (replaces C2SKeepalive).
const TOUCH_EVERY: Duration = Duration::from_millis(500);

/// Per-peer budgets of the fan-out messages.
fn limits() -> &'static std::sync::Mutex<crate::validate::rate::PeerLimits> {
    static L: std::sync::OnceLock<std::sync::Mutex<crate::validate::rate::PeerLimits>> = std::sync::OnceLock::new();
    L.get_or_init(Default::default)
}

/// Take `cost` of `kind` from `peer`'s budget (server clock).
pub(super) fn within_budget(state: &ServerState, peer: PeerId, kind: crate::validate::rate::Kind, cost: f32) -> bool {
    let now = state.net.now_ms() as i64;
    limits().lock().unwrap_or_else(|e| e.into_inner()).take(peer, kind, cost, now)
}

/// Empty `peer`'s `kind` budget.
pub(super) fn drain_budget(state: &ServerState, peer: PeerId, kind: crate::validate::rate::Kind) {
    let now = state.net.now_ms() as i64;
    limits().lock().unwrap_or_else(|e| e.into_inner()).drain(peer, kind, now);
}

/// Drop a departed peer's budgets (peer ids are never reused).
pub(crate) fn forget_limits(peer: PeerId) {
    limits().lock().unwrap_or_else(|e| e.into_inner()).forget(peer);
}

/// Per-source packet budgets of the high-rate streams.
fn stream_limits() -> &'static std::sync::Mutex<crate::validate::rate::StreamLimits> {
    static L: std::sync::OnceLock<std::sync::Mutex<crate::validate::rate::StreamLimits>> = std::sync::OnceLock::new();
    L.get_or_init(Default::default)
}

/// One packet of stream `k` from `from` within its budget? Checked first in
/// each stream arm, before any lock of the game state (otherwise a 10k/s
/// root flood takes the state and lag-comp locks per packet and starves every
/// receiver's relay budget).
pub(super) fn stream_ok(state: &ServerState, from: SocketAddr, k: crate::validate::rate::StreamKind) -> bool {
    let now = state.net.now_ms() as i64;
    let ok = stream_limits().lock().unwrap_or_else(|e| e.into_inner()).take(from, k, now);
    if !ok && crate::validate::rate::log_ok("stream_rate") {
        warn!(%from, ?k, "stream packet over the per-sender rate; dropped");
    }
    ok
}

/// Drop an address's stream budgets (peer left / moved).
pub(crate) fn forget_stream_limits(addr: &SocketAddr) {
    stream_limits().lock().unwrap_or_else(|e| e.into_inner()).forget(addr);
}

pub async fn recv_loop(socket: Arc<UdpSocket>, state: Arc<ServerState>) -> anyhow::Result<()> {
    tokio::spawn(transport_timer(socket.clone(), state.clone()));
    let mut buf = vec![0u8; 65536];
    let mut touched: HashMap<SocketAddr, std::time::Instant> = HashMap::new();
    loop {
        let (n, from) = match socket.recv_from(&mut buf).await {
            Ok(x) => x,
            Err(e) if recv_error_is_routine(&e) => { debug!(error = %e, "recv_from: ICMP unreachable from a gone client"); continue; }
            Err(e) => { warn!(error = %e, "recv_from error"); continue; }
        };
        let data = &buf[..n];
        // NAT traversal (nat/): the test NAT emulation, STUN answers to this socket's own
        // requests, and punch probes (never answered).
        if !crate::nat::emu::inbound_ok(from) { continue; }
        if hsmp_nat::stun::is_stun(data) { crate::nat::on_stun(from, data); continue; }
        if hsmp_nat::probe::is_probe(data) { continue; }
        // Server-browser ping/info query (query.rs) — answered before decode.
        if crate::query::is_request(data) {
            crate::server_info::answer(&socket, &state, from, data).await;
            continue;
        }
        let (action, out) = state.net.handle(from, data);
        send_out(&socket, &state, out).await;
        match action {
            crate::net::Action::None => {}
            crate::net::Action::Auth(p) => admit(&socket, &state, p).await,
            crate::net::Action::Data { deliveries, peer, migrated_to } => {
                // The peer's key address: its packets may arrive from a new
                // path that is still being validated (NAT rebind); once
                // validated, the peer is re-keyed to the new address.
                let from = match migrated_to {
                    Some(new) => { migrate_peer(&socket, &state, peer, new).await; new }
                    None => peer,
                };
                // Any authenticated packet (PINGs included) keeps the peer alive.
                let now = std::time::Instant::now();
                if touched.get(&from).map_or(true, |t| now.duration_since(*t) >= TOUCH_EVERY) {
                    touched.insert(from, now);
                    if touched.len() > 4 * state.max_peers + 64 {
                        touched.retain(|_, t| now.duration_since(*t) < Duration::from_secs(5));
                    }
                    let mut inner = state.inner.lock().await;
                    let now = inner.now_ms;
                    if let Some(p) = inner.peers.get_mut(&from) { p.last_seen_ms = now; }
                }
                handle_deliveries(&socket, &state, from, deliveries).await;
            }
        }
    }
}

/// The messages of one datagram from the peer known as `from`, each to its domain handler.
/// Records relayed on are queued per receiver and sent after the last one, so what a sender
/// packed into one datagram (root and pose of one frame) reaches each receiver in one too.
pub(super) async fn handle_deliveries(socket: &Arc<UdpSocket>, state: &Arc<ServerState>, from: SocketAddr,
                                      deliveries: Vec<hsmp_net::net::Delivery>) {
    let batch = deliveries.len() > 1;
    if batch { state.begin_relay_batch(); }
    for d in deliveries {
        let (h, payload) = match proto::decode_msg(&d.data) {
            Ok(m) => m,
            Err(e) => { debug!(%from, error = %e, len = d.data.len(), "undecodable message dropped"); continue; }
        };
        // v6 typed record: validated in place by its domain handler (an unknown
        // kind, 0 included, is validated and dropped there).
        let pf = crate::perf::perf();
        pf.pkts_in.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let t_handle = std::time::Instant::now();
        if let Err(e) = super::records::handle(socket, state, from, h, payload).await {
            debug!(%from, kind = h.kind, error = %e, len = payload.len(), "v6 record refused");
        }
        pf.handle(t_handle.elapsed().as_micros() as u32);
    }
    if batch { flush_relay_batch(socket, state).await; }
}

/// Windows reports an ICMP port-unreachable for an earlier send as a
/// ConnectionReset on the next receive. A client that closed its game draws
/// one per packet we still send it until its connection times out, so these
/// are routine, not warnings.
fn recv_error_is_routine(e: &std::io::Error) -> bool {
    e.kind() == std::io::ErrorKind::ConnectionReset
}

/// A peer's connection moved from `old` to `new` (validated NAT rebind):
/// re-key everything kept per address, in one critical section with the
/// transport's maps (`commit_migration`), so the peer keeps its id, seat,
/// round and alive state instead of re-joining as a spectator.
pub(crate) async fn migrate_peer(socket: &Arc<UdpSocket>, state: &Arc<ServerState>, old: SocketAddr, new: SocketAddr) {
    if old == new { return; }
    // A migration never displaces another live player, and the
    // per-IP seat cap applies to the address the player moves to (a NAT
    // rebind onto the same IP — the common case — is not counted against
    // itself). A refused migration closes the moving connection: its client
    // re-handshakes and goes through admission like any join.
    let (stale, same_ip) = {
        let inner = state.inner.lock().await;
        (inner.peers.contains_key(&new),
         same_host_count(inner.peers.keys().filter(|a| **a != old && **a != new), new))
    };
    let refuse = if stale && state.net.live_at(new) {
        Some("address in use by another player")
    } else if !per_ip_ok(new.ip(), same_ip, state.max_peers) {
        Some("too many players from this address")
    } else { None };
    if let Some(why) = refuse {
        warn!(%old, %new, why, "migration refused; closing the moving connection");
        kick(socket, state, old, why, 0).await;
        let _ = peer_leave(socket, state, old).await;
        return;
    }
    if stale {
        // A peer whose connection is already gone (not reaped yet).
        info!(%new, "migration target was a departed peer's address; removing that peer");
        let _ = peer_leave(socket, state, new).await;
    }
    let mut inner = state.inner.lock().await;
    let Some(p) = inner.peers.remove(&old) else {
        // Not (or no longer) a peer: only the transport maps move.
        state.net.commit_migration(old, new);
        return;
    };
    let id = p.id;
    if !state.net.commit_migration(old, new) {
        // A live connection appeared at `new` meanwhile: refuse (above).
        inner.peers.insert(old, p);
        drop(inner);
        warn!(peer_id = id, %old, %new, "migration refused: address taken by a live connection");
        kick(socket, state, old, "address in use by another player", 0).await;
        let _ = peer_leave(socket, state, old).await;
        return;
    }
    inner.peers.insert(new, p);
    // Messages queued under the lock for the old address follow the peer.
    for (to, _) in inner.out_msgs.iter_mut() {
        if *to == Some(old) { *to = Some(new); }
    }
    // A ban applies to the address the player now uses, as at admission.
    // (The per-IP seat cap governs new joins only: a player whose NAT rebinds
    // onto a shared address keeps its seat.)
    let banned = is_banned_ip(&inner.banned_ips, new.ip());
    drop(inner);
    state.relay.forget(&old); // relevance / budget state rebuilds from the next frames
    state.relay.set_peer(new, id);
    forget_stream_limits(&old);
    info!(peer_id = id, %old, %new, "peer migrated to a new address (same connection, no re-join)");
    if banned {
        info!(peer_id = id, %new, "migrated onto a banned address: kicked");
        kick(socket, state, new, "banned", 0).await;
        let _ = peer_leave(socket, state, new).await;
    }
}

/// Every 5 ms: flush the transport (acks, retransmits, keepalives, CLOSE
/// repeats) and remove peers whose connection ended (idle timeout 20 s,
/// close by the client, protocol violation).
async fn transport_timer(socket: Arc<UdpSocket>, state: Arc<ServerState>) {
    let mut iv = time::interval(TRANSPORT_TICK);
    iv.set_missed_tick_behavior(time::MissedTickBehavior::Delay);
    let mut last_report = std::time::Instant::now();
    loop {
        iv.tick().await;
        if last_report.elapsed() >= STATS_EVERY {
            let secs = last_report.elapsed().as_secs_f64();
            last_report = std::time::Instant::now();
            log_transport_stats(&state);
            // Pose relay health per sender: frames in vs refused vs relayed
            // to each recipient (a host stuck at 4 Hz shows here).
            for line in state.relay.pose_report(secs) {
                info!("{line}");
            }
            log_peer_links(&state).await;
        }
        let (out, dead) = state.net.tick();
        send_out(&socket, &state, out).await;
        for (addr, st) in dead {
            let present = state.inner.lock().await.peers.contains_key(&addr);
            if present {
                let why = match st {
                    ConnState::Closed { code, by_peer } => format!("code {} by_peer {}", code, by_peer),
                    other => format!("{:?}", other),
                };
                info!(%addr, %why, "connection ended; removing peer");
                let _ = peer_leave(&socket, &state, addr).await;
            }
        }
    }
}

const STATS_EVERY: Duration = Duration::from_secs(10);

/// One line of transport health (handshakes, drops, AEAD failures).
pub(crate) fn log_transport_stats(state: &ServerState) {
    let (conns, preauth, aead_failed, ep) = state.net.stats();
    let c = state.net.counters();
    info!(conns, preauth, aead_failed, accepted = ep.accepted, rejected = ep.rejected,
          preauth_in = ep.preauth_in, preauth_bytes_in = ep.preauth_bytes_in,
          preauth_out = ep.preauth_out, preauth_bytes_out = ep.preauth_bytes_out,
          replay_dropped = c.dropped_replay, addr_mismatch = c.dropped_addr_mismatch,
          unknown_conn = c.dropped_unknown_conn, handshake_dropped = c.dropped_handshake,
          garbage = c.dropped_garbage, legacy_v4 = c.dropped_legacy, v4_rejects = c.v4_rejects,
          backpressure = c.backpressure, "v5 transport stats");
}

/// One line per connected peer every STATS_EVERY: RTT and loss (bug-report logs read these).
/// By peer id, never by address.
pub(crate) async fn log_peer_links(state: &ServerState) {
    let samples = state.net.path_samples();
    if samples.is_empty() {
        return;
    }
    let ids: std::collections::HashMap<std::net::SocketAddr, PeerId> = state.inner.lock().await.peers.iter().map(|(a, p)| (*a, p.id)).collect();
    for (addr, s) in samples {
        let loss = if s.pkts_sent > 0 { 100.0 * s.pkts_lost as f64 / s.pkts_sent as f64 } else { 0.0 };
        info!(peer_id = ?ids.get(&addr), srtt_ms = s.srtt_ms.round() as u64, min_rtt_ms = s.min_rtt_ms.round() as u64,
              loss_pct = %format!("{loss:.2}"), pkts_sent = s.pkts_sent, pkts_lost = s.pkts_lost, "peer link");
    }
}

/// Display nick: control characters stripped, at most 32 bytes, made unique
/// among the present peers ("Willie (2)").
fn dedup_nick(raw: &str, taken: &[String]) -> String {
    // One rule for every path (case-insensitive, 32 bytes).
    session::dedup_nick_among(taken.iter().map(String::as_str), raw)
}

/// Players per public source IP (so one host cannot fill every seat). Loopback and private / link-local addresses (LAN parties, local
/// tests, the listen host) are not limited.
pub(crate) const MAX_PER_PUBLIC_IP: usize = 4;

/// May another player join from `ip`, which already has `same_ip` peers?
/// (`same_ip` counts peers this join does not replace.)
pub(crate) fn per_ip_ok(ip: std::net::IpAddr, same_ip: usize, max_peers: usize) -> bool {
    use std::net::IpAddr;
    let local = match ip {
        IpAddr::V4(v) => v.is_loopback() || v.is_private() || v.is_link_local() || v.is_unspecified(),
        IpAddr::V6(v) => v.is_loopback() || v.is_unspecified() || (v.segments()[0] & 0xfe00) == 0xfc00
            || (v.segments()[0] & 0xffc0) == 0xfe80
            || v.to_ipv4_mapped().map_or(false, |m| m.is_loopback() || m.is_private()),
    };
    local || same_ip < MAX_PER_PUBLIC_IP.min(max_peers.max(1))
}

/// Present peers on the same host as `from` (exact IPv4; IPv6 by /64, since
/// one IPv6 host owns a whole /64).
pub(crate) fn same_host_count<'a>(addrs: impl Iterator<Item = &'a SocketAddr>, from: SocketAddr) -> usize {
    addrs.filter(|a| crate::ipkey::same_key(a.ip(), from.ip())).count()
}

/// Admission for a verified v5 Auth: ban, full, duplicate
/// key (the reconnect replaces the old connection), then accept + Welcome.
async fn admit(socket: &Arc<UdpSocket>, state: &Arc<ServerState>, p: Box<PendingAuth>) {
    let from = p.addr;
    let key = p.player_key();
    let fp = hsmp_net::net::handshake::player_fingerprint(&key);
    // The listen host's key file appears when its sidecar starts; an edited
    // admins file applies to the next join (admin.rs, outside the lock).
    let admins_reloaded = reload_admin_files(state).await;

    // A peer of ANOTHER player at this exact address (its socket was reused)
    // is replaced: it leaves first. The same player key is never "replaced":
    // that is a session resume (below).
    let (banned, replaces) = {
        let inner = state.inner.lock().await;
        let banned = is_banned_ip(&inner.banned_ips, from.ip());
        let replaces: Vec<SocketAddr> = inner.peers.iter()
            .filter(|(a, peer)| **a == from && !same_player(peer, &key))
            .map(|(a, _)| *a)
            .collect();
        (banned, replaces)
    };
    if banned {
        info!(%from, player = %fp, "banned IP tried to join");
        crate::stats::refused("banned");
        let out = state.net.reject(p, reject_code::BANNED, "banned");
        send_out(socket, state, out).await;
        return;
    }
    for old in &replaces {
        info!(%from, old = %old, player = %fp, "join: another player's connection at this address is replaced");
        let out = state.net.close(*old, close_code::REPLACED, "replaced by a new connection");
        send_out(socket, state, out).await;
        let _ = peer_leave(socket, state, *old).await;
    }

    let mut inner = state.inner.lock().await;
    // Session resume: the same player key re-handshaking
    // (sidecar re-join after a stall, a new socket, a forced re-handshake)
    // continues the SAME peer, exactly as a NAT rebind does (`migrate_peer`):
    // same peer id, so the combat ledger, cheat counters, lag-comp history,
    // rate budgets, kit facts, root anchor and alive state all carry over,
    // and the other clients keep the same stand-in (no PeerLeft/PeerJoined).
    // Decided and applied in this one critical section, so the tick can never
    // run between "is it alive" and "insert".
    if let Some(old) = inner.peers.iter().find(|(_, q)| same_player(q, &key)).map(|(a, _)| *a) {
        resume(socket, state, inner, p, old).await;
        return;
    }
    // Same host: exact IPv4, or the same IPv6 /64.
    let same_ip = same_host_count(inner.peers.keys(), from);
    if !per_ip_ok(from.ip(), same_ip, state.max_peers) {
        drop(inner);
        info!(%from, player = %fp, same_ip, "join rejected: too many players from this address");
        crate::stats::refused("rate_limited");
        let out = state.net.reject(p, reject_code::RATE_LIMITED, "too many players from this address");
        send_out(socket, state, out).await;
        return;
    }
    if inner.peers.len() >= state.max_peers {
        let max = state.max_peers;
        drop(inner);
        info!(%from, player = %fp, "join rejected: server full");
        crate::stats::refused("full");
        let out = state.net.reject(p, reject_code::FULL, &format!("server full ({} peers)", max));
        send_out(socket, state, out).await;
        return;
    }
    let caps = p.caps;
    let taken: Vec<String> = inner.peers.values().map(|q| q.nick.clone()).collect();
    let nick = dedup_nick(p.nick(), &taken);
    // Accept under the peer lock, so the tick's reconcile never sees a
    // connection without its peer.
    let cid = state.net.accept(p);
    let id = inner.next_peer_id;
    inner.next_peer_id += 1;
    let now = inner.now_ms;
    // Admin comes from the policy (the listen host's key,
    // configured or granted keys), never from joining first.
    let role = inner.admins.role(&key);
    let is_admin = role >= v5::AdminRole::ADMIN;
    // A brand-new peer (no session of this key is open) fights only from the
    // lobby; mid-match it spectates until the next round it has loaded.
    let joins_alive = inner.match_state == "lobby";
    inner.peers.insert(
        from,
        PeerState {
            id, nick: nick.clone(), cid, player_key: key,
            last_seen_ms: now,
            last_root: None,
            ready: false,
            last_valid_pos: None, last_valid_ms: now,
            // Joining mid-match: spectate (not alive, can't hit or be
            // hit) until the next round's live transition decides.
            wins: 0, alive: joins_alive,
            is_admin,
        },
    );
    state.relay.forget(&from);
    state.relay.set_peer(from, id);
    crate::interact::note_caps(id, caps); // interaction channel: negotiated caps per peer
    // An admin (the listen host back after a drop, a configured admin)
    // joining changes who is admin: everyone's S2CAdminState follows.
    let admins_changed = refresh_admins(&mut inner);
    on_joined(&mut inner, from); // seat by player key, wins restored on reconnect
    let seat = inner.sess.seats.get(&key).copied().unwrap_or(0xFF);
    inner.match_state_dirty = true;
    let admin_state = admin_state_for(&inner, from);
    let all_addrs: Vec<SocketAddr> = if admins_changed || admins_reloaded {
        inner.peers.keys().copied().filter(|a| *a != from).collect()
    } else { Vec::new() };
    drop(inner);

    // First message on channel 2 (encrypted; no key material): the `welcome` record.
    send_msg_to(socket, state, from, welcome_msg(id, seat, state.net.epoch(), state.net.now_ms(), caps, &nick)).await;
    // Who is admin (the `admin_state` record; the welcome has no admin flag).
    send_msg_to(socket, state, from, admin_state).await;
    // The roster (who else is here) rides in the next `session` snapshot.
    if !all_addrs.is_empty() {
        broadcast_admin_state(socket, state, &all_addrs).await;
    }
    // Late joiner / reconnect: replay everyone's newest complete loadout.
    crate::loadout::replay_to(socket, state, from).await;
    info!(%from, peer_id = id, nick = %nick, player = %fp, is_admin, role, "peer joined");
    crate::stats::joined(state.net.addrs().len());
}

/// The peer belongs to the player holding `key` (a real v5 identity; the
/// all-zero key of fixtures never matches).
pub(super) fn same_player(p: &PeerState, key: &[u8; 32]) -> bool {
    *key != [0u8; 32] && p.player_key == *key
}

/// Bans match the address's per-source key (IPv4 exactly, IPv6 by /64), so an
/// IPv6 player rotating inside its /64 is still banned.
pub(crate) fn is_banned_ip(banned: &HashSet<IpAddr>, ip: IpAddr) -> bool {
    banned.contains(&ip) || banned.iter().any(|b| crate::ipkey::same_key(*b, ip))
}

/// Session resume: `p` is the same player as the peer known at
/// `old`. The peer continues on the new connection at `p.addr` with its id
/// and every piece of per-id state; the old connection is closed REPLACED.
/// Runs under the game lock taken by `admit` (one critical section).
async fn resume(
    socket: &Arc<UdpSocket>,
    state: &Arc<ServerState>,
    mut inner: tokio::sync::MutexGuard<'_, Inner>,
    p: Box<PendingAuth>,
    old: SocketAddr,
) {
    let from = p.addr;
    let caps = p.caps;
    let fp = hsmp_net::net::handshake::player_fingerprint(&p.player_key());
    // The per-host seat cap applies when the player moves to another host
    // (the old seat itself is not counted against it).
    if !crate::ipkey::same_key(old.ip(), from.ip()) {
        let same_ip = same_host_count(inner.peers.keys().filter(|a| **a != old), from);
        if !per_ip_ok(from.ip(), same_ip, state.max_peers) {
            drop(inner);
            info!(%from, %old, player = %fp, same_ip, "resume rejected: too many players from the new address");
            let out = state.net.reject(p, reject_code::RATE_LIMITED, "too many players from this address");
            send_out(socket, state, out).await;
            return;
        }
    }
    let mut closes = Vec::new();
    if old != from {
        // Unmap the old address first: reaping its connection must never
        // report the resumed peer as gone.
        closes = state.net.retire(old, close_code::REPLACED, "resumed on a new connection");
    }
    // Same address: `accept` replaces (and closes) the old connection itself.
    let cid = state.net.accept(p);
    let Some(mut peer) = inner.peers.remove(&old) else { return };
    let now = inner.now_ms;
    let old_cid = peer.cid;
    peer.cid = cid;
    peer.last_seen_ms = now;
    let (id, nick, alive) = (peer.id, peer.nick.clone(), peer.alive);
    inner.peers.insert(from, peer);
    // Messages queued under the lock for the old address follow the peer.
    for (to, _) in inner.out_msgs.iter_mut() {
        if *to == Some(old) { *to = Some(from); }
    }
    state.relay.forget(&old);
    state.relay.forget(&from);
    state.relay.set_peer(from, id);
    forget_stream_limits(&old);
    crate::interact::note_caps(id, caps);
    session::on_resumed(&mut inner, from, old);
    let seat = inner.sess.seats.get(&peer_key_of(&inner, from)).copied().unwrap_or(0xFF);
    let admin_state = admin_state_for(&inner, from);
    drop(inner);
    send_out(socket, state, closes).await;
    info!(%from, %old, peer_id = id, old_cid, cid, alive, player = %fp,
          "session resumed: same peer id, state carried over (no re-join)");
    // The resumed client starts its session view over: welcome (same id), admin, the
    // session snapshot (on_resumed forces one), loadouts. The other clients see no change.
    send_msg_to(socket, state, from, welcome_msg(id, seat, state.net.epoch(), state.net.now_ms(), caps, &nick)).await;
    send_msg_to(socket, state, from, admin_state).await;
    crate::loadout::replay_to(socket, state, from).await;
    flush_out(socket, state).await;
}

fn peer_key_of(inner: &Inner, addr: SocketAddr) -> session::PlayerKey {
    inner.peers.get(&addr).map(session::peer_key).unwrap_or([0; 32])
}

/// Act on a ledger verdict for `pid`'s vitals (under the game lock): the
/// owner's own death, or the server's god-mode verdict (lethal
/// server-validated damage, owner still reporting alive) as DEATH_DAMAGE.
pub(super) fn on_ledger_verdict(inner: &mut Inner, pid: PeerId, round: u32, v: &crate::combat::LedgerVerdict, dead: bool) {
    if !v.lethal || !combat_open(inner) { return; }
    let killer = crate::combat::ledger_last_attacker(pid, round);
    if v.forced {
        crate::validate::cheat::bump(pid, crate::validate::cheat::Kind::GodMode);
        if declare_death(inner, pid, killer, DEATH_DAMAGE) {
            warn!(peer_id = pid, round, killer, est = v.est,
                  "death (server ledger): validated damage is lethal but the owner keeps reporting alive");
        }
    } else if declare_death(inner, pid, killer, DEATH_VITALS) {
        info!(peer_id = pid, round, hp = v.hp, est = v.est, dead, "death (owner vitals)");
    }
}

#[cfg(test)]
pub(crate) mod resume_tests {
    use super::*;
    use crate::net::Action;
    use hsmp_net::net::{Client, ClientConfig, ConnConfig};

    /// A v5 client on its own loopback socket: the server's real sends reach
    /// it; its datagrams are fed to `Net::handle` directly (no recv loop).
    pub(crate) struct TClient { pub c: Client, pub sock: UdpSocket, pub addr: SocketAddr, pub records: Vec<Vec<u8>> }

    impl TClient {
        /// Deliver what the server sent us, then hand our datagrams to the
        /// server (an Auth goes through the real admission, `admit`).
        pub(crate) async fn pump(&mut self, socket: &Arc<UdpSocket>, state: &Arc<ServerState>) {
            tokio::time::sleep(Duration::from_millis(5)).await;
            let mut buf = [0u8; 2048];
            let now = state.net.now_ms();
            while let Ok((n, _)) = self.sock.try_recv_from(&mut buf) { self.c.handle(now, &buf[..n]); }
            while let Some(ev) = self.c.poll_event() {
                if let hsmp_net::net::ClientEvent::Message(d) = ev {
                    if proto::decode_msg(&d.data).is_ok() { self.records.push(d.data.clone()); }
                }
            }
            while let Some(dg) = self.c.poll_transmit(now) {
                let (act, out) = state.net.handle(self.addr, &dg);
                for (_, r) in out { self.c.handle(now, &r); }
                if let Action::Auth(p) = act { admit(socket, state, p).await; }
            }
        }
        pub(crate) fn welcome_id(&self) -> Option<PeerId> {
            self.records.iter().rev().find_map(|m| hsmp_ipc::wire::decode::<hsmp_ipc::schema::session::Welcome>(m).ok().map(|(_, v)| v.head.peer_id))
        }
        /// The record messages of `kind` received so far (payloads).
        pub(crate) fn recs(&self, kind: u16) -> Vec<Vec<u8>> {
            self.records.iter().filter(|m| hsmp_ipc::wire::kind_of(m) == kind).map(|m| m[hsmp_ipc::wire::HDR..].to_vec()).collect()
        }
    }

    /// Handshake a v5 client with player seed `seed` (a fresh loopback
    /// socket) through the real admission. Returns it with its Welcome.
    pub(crate) async fn join(socket: &Arc<UdpSocket>, state: &Arc<ServerState>, seed: u8) -> TClient {
        let sock = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let addr = sock.local_addr().unwrap();
        let c = Client::new(ClientConfig::new([seed; 32], "w"), ConnConfig::default(), state.net.now_ms(), &mut rand::rngs::OsRng);
        let mut t = TClient { c, sock, addr, records: Vec::new() };
        for _ in 0..100 {
            t.pump(socket, state).await;
            if t.c.is_connected() && t.welcome_id().is_some() { break; }
        }
        assert!(t.c.is_connected() && t.welcome_id().is_some(), "client {seed} did not get a Welcome");
        t
    }

    /// Two players in a live duel; peer ids far from other tests' (the combat
    /// ledger and cheat tables are process-global).
    pub(crate) async fn live_duel(base: PeerId) -> (Arc<UdpSocket>, Arc<ServerState>, TClient, TClient) {
        let state = Arc::new(ServerState::new(8));
        let socket = Arc::new(UdpSocket::bind("127.0.0.1:0").await.unwrap());
        state.inner.lock().await.next_peer_id = base;
        let ca = join(&socket, &state, seed(base)).await;
        let cb = join(&socket, &state, seed(base + 1)).await;
        {
            let mut i = state.inner.lock().await;
            let keys: Vec<session::PlayerKey> = i.peers.values().map(session::peer_key).collect();
            i.participants = keys;
            i.match_state = "live".into();
            i.match_round = 1;
            for p in i.peers.values_mut() { p.alive = true; }
        }
        (socket, state, ca, cb)
    }

    /// Player seed for a test peer id (distinct per id within a test).
    pub(crate) fn seed(id: PeerId) -> u8 { (id % 251) as u8 + 1 }

    /// The public player key of seed `s` (what the handshake verifies).
    fn key_of(s: u8) -> [u8; 32] {
        Client::new(ClientConfig::new([s; 32], ""), ConnConfig::default(), 0, &mut rand::rngs::OsRng).player_key()
    }

    fn admin_state(t: &TClient) -> Option<(PeerId, Vec<String>)> {
        use hsmp_ipc::schema::session as rec;
        t.recs(rec::K_ADMIN_STATE).last().and_then(|p| hsmp_ipc::record::view::<rec::AdminStateHead>(p).ok())
            .map(|v| (v.head.admin_peer, v.rows.iter().map(|r| r.entry.lossy().into_owned()).collect()))
    }

    /// A send to a port nobody listens on comes back, on Windows, as a
    /// ConnectionReset on the next receive; that error is routine, and the
    /// socket keeps receiving after it.
    #[tokio::test]
    async fn icmp_unreachable_on_receive_is_routine() {
        assert!(recv_error_is_routine(&std::io::Error::from(std::io::ErrorKind::ConnectionReset)));
        assert!(!recv_error_is_routine(&std::io::Error::from(std::io::ErrorKind::PermissionDenied)));
        let s = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let dead = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
        let dead_addr = dead.local_addr().unwrap();
        drop(dead);
        s.send_to(b"x", dead_addr).await.unwrap();
        let peer = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        peer.send_to(b"live", s.local_addr().unwrap()).await.unwrap();
        let mut buf = [0u8; 16];
        let mut got = None;
        for _ in 0..4 {
            match tokio::time::timeout(Duration::from_secs(2), s.recv_from(&mut buf)).await.unwrap() {
                Ok((n, _)) => { got = Some(buf[..n].to_vec()); break; }
                Err(e) => assert!(recv_error_is_routine(&e), "unexpected receive error {e:?}"),
            }
        }
        assert_eq!(got.as_deref(), Some(&b"live"[..]));
    }

    /// Admin assignment through the real admission: on a dedicated server the
    /// first stranger is not admin; a configured key is, and only it gets
    /// the (masked) ban list.
    #[tokio::test]
    async fn admission_dedicated_first_joiner_is_not_admin_configured_key_is() {
        let state = Arc::new(ServerState::new(8));
        let socket = Arc::new(UdpSocket::bind("127.0.0.1:0").await.unwrap());
        state.inner.lock().await.banned_ips.insert("198.51.100.9".parse().unwrap());
        configure_admins(&state, AdminOpts { admin_keys: vec![hex::encode(key_of(212))], ..Default::default() }).await.unwrap();
        let mut stranger = join(&socket, &state, 211).await;
        let sid = stranger.welcome_id().unwrap();
        {
            let i = state.inner.lock().await;
            assert!(i.peers.values().all(|p| !p.is_admin), "the first joiner is not admin");
            assert_eq!(i.admin_peer_id, 0);
        }
        assert_eq!(admin_state(&stranger), Some((0, vec![])));
        let mut adm = join(&socket, &state, 212).await;
        let aid = adm.welcome_id().unwrap();
        stranger.pump(&socket, &state).await;
        adm.pump(&socket, &state).await;
        let (me, bans) = admin_state(&adm).unwrap();
        assert_eq!(me, aid, "the configured admin's client sees itself as admin");
        assert_eq!(bans.len(), 1);
        assert!(bans[0].starts_with("198.51.x.x#") && !bans[0].contains("100.9"), "{bans:?}");
        // The stranger is told who the admin is, never the bans.
        assert_eq!(admin_state(&stranger), Some((aid, vec![])));
        let i = state.inner.lock().await;
        assert!(i.peers.values().find(|p| p.id == aid).unwrap().is_admin);
        assert!(!i.peers.values().find(|p| p.id == sid).unwrap().is_admin);
    }

    /// RCON ADMIN ADD / REMOVE by peer id: the grant and the revocation reach the
    /// player's client (S2CAdminState), LIST names it.
    #[tokio::test]
    async fn rcon_admin_add_and_remove() {
        let state = Arc::new(ServerState::new(8));
        let socket = Arc::new(UdpSocket::bind("127.0.0.1:0").await.unwrap());
        let mut a = join(&socket, &state, 231).await;
        let aid = a.welcome_id().unwrap();
        assert!(rcon_admin(&socket, &state, "ADD 999").await.is_err());
        assert!(rcon_admin(&socket, &state, "FROB 1").await.is_err());
        assert!(rcon_admin(&socket, &state, &format!("ADD {aid}")).await.is_ok());
        a.pump(&socket, &state).await;
        assert_eq!(admin_state(&a).map(|x| x.0), Some(aid));
        let list = rcon_admin(&socket, &state, "LIST").await.unwrap();
        assert!(list.contains(&hex::encode(key_of(231))) && list.contains("runtime online"), "{list}");
        assert!(rcon_admin(&socket, &state, &format!("REMOVE {aid}")).await.is_ok());
        a.pump(&socket, &state).await;
        assert_eq!(admin_state(&a).map(|x| x.0), Some(0));
        assert_eq!(state.inner.lock().await.admin_peer_id, 0);
    }

    /// Listen server: the host's key (from its key file, written after the
    /// server started) is admin even when a stranger joined first.
    #[tokio::test]
    async fn admission_listen_host_is_admin_by_key_even_after_a_stranger() {
        let dir = std::env::temp_dir().join(format!("hsmp-owner-{}-{}", std::process::id(), rand::random::<u32>()));
        std::fs::create_dir_all(&dir).unwrap();
        let kf = dir.join(".player_key");
        let state = Arc::new(ServerState::new(8));
        let socket = Arc::new(UdpSocket::bind("127.0.0.1:0").await.unwrap());
        configure_admins(&state, AdminOpts { owner_key_file: Some(kf.clone()), ..Default::default() }).await.unwrap();
        let _stranger = join(&socket, &state, 221).await;
        std::fs::write(&kf, format!("{}\n", hex::encode(key_of(222)))).unwrap();
        let host = join(&socket, &state, 222).await;
        let hid = host.welcome_id().unwrap();
        {
            let i = state.inner.lock().await;
            assert_eq!(i.admin_peer_id, hid);
            assert_eq!(i.peers.values().filter(|p| p.is_admin).count(), 1);
        }
        // Pinned once the host joined: rewriting the file changes nothing.
        std::fs::write(&kf, format!("{}\n", hex::encode(key_of(223)))).unwrap();
        let other = join(&socket, &state, 223).await;
        let oid = other.welcome_id().unwrap();
        let i = state.inner.lock().await;
        assert!(!i.peers.values().find(|p| p.id == oid).unwrap().is_admin);
        drop(i);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A same-key re-join during Live is a session resume: same
    /// peer id, same alive state, root anchor and cheat record, no
    /// PeerLeft/PeerJoined for the others, the old connection closed.
    #[tokio::test]
    async fn same_key_rejoin_during_live_resumes_the_same_peer() {
        let (socket, state, mut ca, cb) = live_duel(71_001).await;
        let (b, bid) = (cb.addr, cb.welcome_id().unwrap());
        crate::validate::cheat::bump(bid, crate::validate::cheat::Kind::Teleport);
        {
            let mut i = state.inner.lock().await;
            i.peers.get_mut(&b).unwrap().last_valid_pos = Some([10.0, 20.0, 30.0]);
        }
        flush_out(&socket, &state).await; // the join notices of the setup
        ca.pump(&socket, &state).await;
        ca.records.clear();
        // The cheater (or a stalled sidecar) re-handshakes from a new socket.
        let cb2 = join(&socket, &state, seed(71_002)).await;
        assert_eq!(cb2.welcome_id(), Some(bid), "the resumed Welcome carries the same peer id");
        {
            let i = state.inner.lock().await;
            assert!(!i.peers.contains_key(&b), "old address gone");
            let p = i.peers.get(&cb2.addr).expect("resumed at the new address");
            assert_eq!((p.id, p.alive, p.last_valid_pos), (bid, true, Some([10.0, 20.0, 30.0])),
                       "id, alive state and root anchor carried over (no free teleport)");
            assert_eq!(i.peers.len(), 2);
        }
        for _ in 0..5 { ca.pump(&socket, &state).await; }
        // The other client keeps the same stand-in: its roster never loses the peer id and no
        // PLAYER_LEFT / PLAYER_JOINED notice is sent.
        let notices = ca.recs(hsmp_ipc::schema::session::K_NOTICE);
        assert!(notices.is_empty(), "no join / leave notice on a resume: {} notice(s)", notices.len());
        assert_eq!(crate::validate::cheat::get(bid).teleport, 1, "cheat record kept");
        let addrs = state.net.addrs();
        assert_eq!(addrs.len(), 2, "one live connection per player");
        assert!(addrs.contains(&cb2.addr) && !addrs.contains(&b));
        // Reaping the old connection never removes the resumed peer.
        for _ in 0..3 {
            let (_, dead) = state.net.tick();
            assert!(dead.is_empty(), "{dead:?}");
            tokio::time::sleep(Duration::from_millis(40)).await;
        }
    }

    /// Exploit regression: lethal, validated, unreflected damage
    /// booked against a god-mode client, then a forced re-join — the server
    /// still declares the death (the ledger survives the resume).
    #[tokio::test]
    async fn rejoin_does_not_reset_the_god_mode_ledger() {
        // Kills are opt-in (HSMP_GODMODE_ENFORCE=1); this exercises
        // that mode. (Never turned off again here: concurrent tests that rely
        // on the default only use their own Ledger instances.)
        crate::combat::ledger_set_enforce_godmode(true);
        let (socket, state, ca, cb) = live_duel(72_001).await;
        let (aid, bid) = (ca.welcome_id().unwrap(), cb.welcome_id().unwrap());
        // The ledger is process-global and other tests' go_live resets every
        // life to their round: use a round of our own and retry the scenario
        // if a concurrent test wiped it mid-way.
        let round = 72_001;
        for attempt in 0..5 {
            if god_mode_scenario(&socket, &state, aid, bid, round).await { return; }
            eprintln!("ledger reset by a concurrent test (attempt {attempt}); retrying");
        }
        panic!("the god-mode bound must still fire after a re-join");
    }

    /// Book lethal unreflected damage, resume the victim, keep reporting full
    /// health: true once the ledger declares the death (false: the life was
    /// reset by another test's round start, inconclusive).
    async fn god_mode_scenario(socket: &Arc<UdpSocket>, state: &Arc<ServerState>, aid: PeerId, bid: PeerId, round: u32) -> bool {
        crate::combat::ledger_forget(bid);
        let hit = |id: u32| proto::DamageEvent::new(crate::proto::Damage {
            hit_id: id, target_peer_id: bid, round, bone: hsmp_ipc::layout::Str::new("spine_02"), offset: [0.0; 3],
            location: [0.0; 3], impulse: [0.0; 3], velocity: [0.0; 3], normal: [1.0, 0.0, 0.0],
            raw_damage: 40.0, cutting_power: 1.0, pain_rate: 1.0, draw_cut: 0.0, damage_out: 70.0,
            dism_blunt: 0, flags: 0, age_ms: 0,
            attacker_ts: 0, victim_view_ts: 0, victim_arm_ts: 0, ..Default::default()
        }, &crate::proto::deltas_of(&[(crate::combat::FIELD_HEALTH, -70.0)]));
        assert!(crate::combat::ledger_vitals(bid, round, 1, 100.0, false).is_some());
        for id in 1..=3 {
            crate::combat::ledger_forward(aid, round, &hit(id));
            crate::combat::ledger_ack(bid, aid, id);
        }
        // Shortly before the ledger would fire, the client forces a re-handshake.
        let cb2 = join(socket, state, seed(72_002)).await;
        assert_eq!(cb2.welcome_id(), Some(bid));
        // ... and keeps reporting full health.
        for seq in 2..80u32 {
            tokio::time::sleep(Duration::from_millis(50)).await;
            if crate::combat::ledger_last_attacker(bid, round) != aid { return false; }
            if let Some(v) = crate::combat::ledger_vitals(bid, round, seq, 100.0, false) {
                if v.lethal { assert!(v.forced); return true; }
            }
        }
        false
    }

    /// A re-join never resurrects a player the round already
    /// declared dead (the alive flag is the old peer's, read and written in
    /// one critical section).
    #[tokio::test]
    async fn rejoin_never_resurrects_a_dead_player() {
        let (socket, state, _ca, cb) = live_duel(73_001).await;
        let bid = cb.welcome_id().unwrap();
        {
            let mut i = state.inner.lock().await;
            let addrs: Vec<SocketAddr> = i.peers.keys().copied().collect();
            // A 3-player round, so one death does not end it.
            let mut c = match_core::round_tests::peer(73_900, "c");
            c.player_key = [77; 32];
            i.peers.insert("127.0.0.1:9".parse().unwrap(), c);
            i.participants.push([77; 32]);
            assert_eq!(addrs.len(), 2);
            assert!(declare_death(&mut i, bid, 0, DEATH_REPORTED));
            assert_eq!(i.match_state, "live");
        }
        let cb2 = join(&socket, &state, seed(73_002)).await;
        let i = state.inner.lock().await;
        let p = i.peers.get(&cb2.addr).unwrap();
        assert_eq!((p.id, p.alive), (bid, false));
    }

    /// A same-address re-handshake (sidecar restart, same socket) resumes too.
    #[tokio::test]
    async fn same_address_rehandshake_resumes() {
        let (socket, state, _ca, cb) = live_duel(74_001).await;
        let bid = cb.welcome_id().unwrap();
        let TClient { sock, addr, .. } = cb;
        let c = Client::new(ClientConfig::new([seed(74_002); 32], "w"), ConnConfig::default(), state.net.now_ms(), &mut rand::rngs::OsRng);
        let mut cb2 = TClient { c, sock, addr, records: Vec::new() };
        for _ in 0..100 {
            cb2.pump(&socket, &state).await;
            if cb2.welcome_id().is_some() { break; }
        }
        assert_eq!(cb2.welcome_id(), Some(bid));
        let i = state.inner.lock().await;
        assert_eq!(i.peers.get(&addr).map(|p| p.id), Some(bid));
        assert_eq!(i.peers.len(), 2);
        drop(i);
        for _ in 0..3 {
            let (_, dead) = state.net.tick();
            assert!(dead.is_empty(), "{dead:?}");
            tokio::time::sleep(Duration::from_millis(40)).await;
        }
        assert_eq!(state.inner.lock().await.peers.len(), 2);
    }


    /// Exploit regression: ten root packets bunched inside one
    /// server tick cannot each claim a fresh burst (≈35 m teleport).
    #[tokio::test]
    async fn bunched_roots_cannot_teleport() {
        let state = Arc::new(ServerState::new(8));
        let a: SocketAddr = "203.0.113.30:4000".parse().unwrap();
        {
            let mut i = state.inner.lock().await;
            let mut p = match_core::round_tests::peer(75_001, "t");
            p.last_valid_pos = Some([0.0, 0.0, 100.0]);
            i.peers.insert(a, p);
        }
        let mut x = 0.0;
        for _ in 0..10 {
            let pos = [x + 350.0, 0.0, 100.0];
            if session::accept_root(&state, a, pos, Default::default()).await.is_some() { x = pos[0]; }
        }
        assert!(x <= 1050.0 + 1.0, "moved {x} uu inside one tick");
        // An honest sprint (700 uu/s, 60 Hz roots on the arrival clock) keeps passing.
        let mut x = x;
        for k in 0..120 {
            tokio::time::sleep(Duration::from_millis(16)).await;
            let pos = [x + 700.0 / 60.0, 0.0, 100.0];
            assert!(session::accept_root(&state, a, pos, Default::default()).await.is_some(), "sprint step {k} refused");
            x = pos[0];
        }
    }

    impl TClient {
        /// Queue one v6 record message (its kind's channel) for the next pump.
        pub(crate) fn send_rec(&mut self, msg: Vec<u8>) {
            let (h, _) = hsmp_ipc::wire::split(&msg).unwrap();
            let mode = proto::record_mode(h.kind, h.peer).unwrap();
            self.c.send(mode, msg).unwrap();
        }
    }

    /// The session records end to end through the real recv path (records.rs ->
    /// session_records.rs): welcome, command -> cmd_result (+ the cached duplicate), chat ->
    /// chat_in for everyone, ping -> pong, an invalid command refused without a result,
    /// spawned / game_status accepted, the session snapshot carrying both seats, leave.
    #[tokio::test]
    async fn session_records_end_to_end() {
        use hsmp_ipc::layout::Str;
        use hsmp_ipc::schema::session as rec;
        let state = Arc::new(ServerState::new(8));
        let socket = Arc::new(UdpSocket::bind("127.0.0.1:0").await.unwrap());
        state.inner.lock().await.next_peer_id = 76_001;
        let mut a = join(&socket, &state, seed(76_001)).await;
        let mut b = join(&socket, &state, seed(76_002)).await;
        let (aid, bid) = (a.welcome_id().unwrap(), b.welcome_id().unwrap());
        // Each record goes through the same validation the recv loop uses.
        async fn deliver(socket: &Arc<UdpSocket>, state: &Arc<ServerState>, from: SocketAddr, msg: &[u8]) -> bool {
            let (h, p) = hsmp_ipc::wire::split(msg).unwrap();
            super::super::records::handle(socket, state, from, h, p).await.is_ok()
        }
        // READY with a cmd_id: one result; a duplicate gets the cached one.
        let ready = hsmp_ipc::wire::encode(0, 0, &rec::Command { flag: true.into(), ..rec::Command::new(41, rec::cmd_op::READY) }, &[]);
        assert!(deliver(&socket, &state, a.addr, &ready).await);
        assert!(deliver(&socket, &state, a.addr, &ready).await);
        a.pump(&socket, &state).await;
        let results: Vec<rec::CmdResult> = a.recs(rec::K_CMD_RESULT).iter()
            .map(|p| hsmp_ipc::record::view::<rec::CmdResult>(p).unwrap().head()).collect();
        assert_eq!(results.len(), 2);
        assert!(results.iter().all(|r| r.cmd_id == 41 && r.ok.get() && r.op == rec::cmd_op::READY));
        assert!(state.inner.lock().await.peers[&a.addr].ready, "applied once");
        // A hostile command (cmd_id 0) is refused by the validator: no result.
        let bad = hsmp_ipc::wire::encode(0, 0, &rec::Command::new(0, rec::cmd_op::START), &[]);
        assert!(!deliver(&socket, &state, a.addr, &bad).await);
        // Chat: everyone gets the chat_in from A.
        let chat = hsmp_ipc::wire::encode(0, 0, &rec::Chat { text: Str::new("  gg wp  ") }, &[]);
        assert!(deliver(&socket, &state, a.addr, &chat).await);
        b.pump(&socket, &state).await;
        let c = b.recs(rec::K_CHAT_IN);
        let ci = hsmp_ipc::record::view::<rec::ChatIn>(&c[0]).unwrap().head();
        assert_eq!((ci.from_peer, ci.nick.lossy().into_owned(), ci.text.lossy().into_owned()), (aid, "w".to_string(), "gg wp".to_string()));
        // Ping -> pong to the sender only.
        let ping = hsmp_ipc::wire::encode(0, 0, &rec::Ping { client_time_ms: 1234 }, &[]);
        assert!(deliver(&socket, &state, b.addr, &ping).await);
        b.pump(&socket, &state).await;
        let pongs = b.recs(rec::K_PONG);
        assert_eq!(hsmp_ipc::record::view::<rec::Pong>(&pongs[0]).unwrap().head.client_time_ms, 1234);
        // A server-originated kind from a client is refused.
        let fake = hsmp_ipc::wire::encode(0, 0, &rec::Notice { code: 1, ..Default::default() }, &[]);
        assert!(!deliver(&socket, &state, b.addr, &fake).await);
        // game_status (lobby: liveness) and spawned are accepted.
        let gs = hsmp_ipc::wire::encode(0, 0, &rec::GameStatus { flags: rec::status_flag::IN_MENU, ..Default::default() }, &[]);
        assert!(deliver(&socket, &state, b.addr, &gs).await);
        assert!(state.inner.lock().await.match_peers.get(&bid).is_some_and(|m| m.aware), "a game client now");
        let sp = hsmp_ipc::wire::encode(0, 0, &rec::Spawned { round: 1, slot: 0, pos: [1.0, 2.0, 3.0], ..Default::default() }, &[]);
        assert!(deliver(&socket, &state, b.addr, &sp).await);
        // The snapshot (built and framed as the tick does) has both seats; A is ready.
        let snap = { let mut i = state.inner.lock().await; session::session_due(&mut i, 1000).unwrap() };
        let msg = session_msg(&snap);
        let (_, v) = hsmp_ipc::wire::decode::<rec::SessionHead>(&msg).unwrap();
        assert_eq!(v.rows.len(), 2);
        assert!(v.rows.iter().any(|r| r.peer_id == aid && r.ready.get()));
        // Leave on purpose: B is gone, A gets the "left" notice.
        let leave = hsmp_ipc::wire::encode(0, 0, &rec::Leave { reason: 0, _r: [0; 7] }, &[]);
        assert!(deliver(&socket, &state, b.addr, &leave).await);
        assert!(!state.inner.lock().await.peers.contains_key(&b.addr));
        a.pump(&socket, &state).await;
        let left: Vec<rec::Notice> = a.recs(rec::K_NOTICE).iter()
            .map(|p| hsmp_ipc::record::view::<rec::Notice>(p).unwrap().head()).filter(|n| n.code == v5::Notice::PLAYER_LEFT).collect();
        assert_eq!(left.len(), 1);
        assert_eq!(left[0].args[1], "left");
        // The transport path of a record (a TClient send) reaches the same handler.
        a.send_rec(hsmp_ipc::wire::encode(0, 0, &rec::Command { flag: false.into(), ..rec::Command::new(42, rec::cmd_op::READY) }, &[]));
        a.pump(&socket, &state).await;
    }

    /// A stream flood from one address is cut before any lock.
    #[test]
    fn stream_flood_is_cut_per_source() {
        let state = ServerState::new(8);
        let a: SocketAddr = "203.0.113.31:4000".parse().unwrap();
        let ok = (0..5_000).filter(|_| stream_ok(&state, a, crate::validate::rate::StreamKind::Root)).count();
        assert!(ok <= crate::validate::rate::STREAM_BURST as usize + 40, "{ok}");
        forget_stream_limits(&a);
    }

    /// A ban matches the whole IPv6 /64.
    #[test]
    fn bans_match_the_ipv6_slash_64() {
        let mut bans = HashSet::new();
        bans.insert("2001:db8:1:2::5".parse::<IpAddr>().unwrap());
        assert!(is_banned_ip(&bans, "2001:db8:1:2:aaaa::9".parse().unwrap()));
        assert!(!is_banned_ip(&bans, "2001:db8:1:3::5".parse().unwrap()));
        bans.insert("203.0.113.7".parse().unwrap());
        assert!(is_banned_ip(&bans, "203.0.113.7".parse().unwrap()));
        assert!(!is_banned_ip(&bans, "203.0.113.8".parse().unwrap()));
    }
}

#[cfg(test)]
mod migrate_tests {
    use super::*;

    /// A validated NAT rebind re-keys the peer (same id,
    /// seat, alive state, admin) instead of a re-join as a spectator.
    #[tokio::test]
    async fn migrated_peer_keeps_its_identity_and_state() {
        let state = Arc::new(ServerState::new(8));
        let socket = Arc::new(UdpSocket::bind("127.0.0.1:0").await.unwrap());
        let old: SocketAddr = "203.0.113.5:40000".parse().unwrap();
        let new: SocketAddr = "203.0.113.5:41234".parse().unwrap();
        let (_c, cid) = crate::net::testkit::connect(&state.net, old, 9);
        {
            let mut inner = state.inner.lock().await;
            inner.peers.insert(old, PeerState {
                id: 7, nick: "Willie".into(), cid, player_key: [9; 32],
                last_seen_ms: 0, last_root: None,
                ready: true, last_valid_pos: Some([1.0, 2.0, 3.0]), last_valid_ms: 0,
                wins: 2, alive: true, is_admin: true,
            });
            inner.match_state = "live".into();
            inner.out_msgs.push((Some(old), crate::net::testkit::test_msg()));
        }
        migrate_peer(&socket, &state, old, new).await;
        let inner = state.inner.lock().await;
        assert!(!inner.peers.contains_key(&old));
        let p = inner.peers.get(&new).expect("re-keyed");
        assert_eq!((p.id, p.wins, p.alive, p.is_admin, p.cid), (7, 2, true, true, cid));
        assert_eq!(inner.out_msgs[0].0, Some(new), "queued messages follow the peer");
        assert_eq!(state.net.addrs(), vec![new]);
    }

    fn fixture(id: PeerId, cid: u64, key: u8) -> PeerState {
        PeerState {
            id, nick: format!("p{id}"), cid, player_key: [key; 32],
            last_seen_ms: 0, last_root: None,
            ready: true, last_valid_pos: None, last_valid_ms: 0,
            wins: 0, alive: true, is_admin: false,
        }
    }

    /// A migration onto another live player's address is refused
    /// (the moving connection is closed); the other player is never removed.
    #[tokio::test]
    async fn migration_never_displaces_another_live_player() {
        let state = Arc::new(ServerState::new(8));
        let socket = Arc::new(UdpSocket::bind("127.0.0.1:0").await.unwrap());
        let a: SocketAddr = "203.0.113.20:40000".parse().unwrap();
        let b: SocketAddr = "198.51.100.30:41000".parse().unwrap();
        let (_ca, cid_a) = crate::net::testkit::connect(&state.net, a, 21);
        let (_cb, cid_b) = crate::net::testkit::connect(&state.net, b, 22);
        {
            let mut inner = state.inner.lock().await;
            inner.peers.insert(a, fixture(1, cid_a, 1));
            inner.peers.insert(b, fixture(2, cid_b, 2));
        }
        migrate_peer(&socket, &state, b, a).await;
        let inner = state.inner.lock().await;
        assert_eq!(inner.peers.get(&a).map(|p| p.id), Some(1), "the live player keeps its address");
        assert!(!inner.peers.values().any(|p| p.id == 2), "the moving connection is closed");
    }

    /// The per-IP seat cap applies to a migration (a NAT rebind
    /// within the same IP is not counted against itself).
    #[tokio::test]
    async fn migration_respects_the_per_ip_seat_cap() {
        let state = Arc::new(ServerState::new(8));
        let socket = Arc::new(UdpSocket::bind("127.0.0.1:0").await.unwrap());
        let mover: SocketAddr = "198.51.100.40:41000".parse().unwrap();
        let (_c, cid) = crate::net::testkit::connect(&state.net, mover, 23);
        {
            let mut inner = state.inner.lock().await;
            for i in 0..MAX_PER_PUBLIC_IP as u32 {
                let a: SocketAddr = format!("203.0.113.9:{}", 5000 + i).parse().unwrap();
                inner.peers.insert(a, fixture(10 + i, 0, 10 + i as u8));
            }
            inner.peers.insert(mover, fixture(2, cid, 2));
        }
        migrate_peer(&socket, &state, mover, "203.0.113.9:6000".parse().unwrap()).await;
        let inner = state.inner.lock().await;
        assert!(!inner.peers.values().any(|p| p.id == 2), "fifth player onto a full public IP refused");
        assert_eq!(inner.peers.len(), MAX_PER_PUBLIC_IP);
        drop(inner);
        // A rebind that only changes the port (same IP) is fine even at the cap.
        let state = Arc::new(ServerState::new(8));
        let old: SocketAddr = "203.0.113.9:5000".parse().unwrap();
        let (_c, cid) = crate::net::testkit::connect(&state.net, old, 24);
        {
            let mut inner = state.inner.lock().await;
            for i in 1..MAX_PER_PUBLIC_IP as u32 {
                let a: SocketAddr = format!("203.0.113.9:{}", 5000 + i).parse().unwrap();
                inner.peers.insert(a, fixture(10 + i, 0, 10 + i as u8));
            }
            inner.peers.insert(old, fixture(3, cid, 3));
        }
        let new: SocketAddr = "203.0.113.9:7000".parse().unwrap();
        migrate_peer(&socket, &state, old, new).await;
        assert_eq!(state.inner.lock().await.peers.get(&new).map(|p| p.id), Some(3));
    }

    /// A migration onto a banned address is a kick, as a join from it would be.
    #[tokio::test]
    async fn migration_onto_a_banned_address_is_kicked() {
        let state = Arc::new(ServerState::new(8));
        let socket = Arc::new(UdpSocket::bind("127.0.0.1:0").await.unwrap());
        let old: SocketAddr = "203.0.113.6:40000".parse().unwrap();
        let new: SocketAddr = "198.51.100.9:41234".parse().unwrap();
        let (_c, cid) = crate::net::testkit::connect(&state.net, old, 9);
        {
            let mut inner = state.inner.lock().await;
            inner.peers.insert(old, PeerState {
                id: 8, nick: "Banned".into(), cid, player_key: [8; 32],
                last_seen_ms: 0, last_root: None,
                ready: true, last_valid_pos: None, last_valid_ms: 0,
                wins: 0, alive: true, is_admin: false,
            });
            inner.banned_ips.insert(new.ip());
        }
        migrate_peer(&socket, &state, old, new).await;
        let inner = state.inner.lock().await;
        assert!(inner.peers.is_empty(), "the banned address does not keep the seat");
    }
}

#[cfg(test)]
mod bundle_tests {
    use super::*;
    use super::resume_tests::live_duel;
    use crate::net::Action;

    /// A sender's root and vitals that arrive in one datagram reach the other player in one
    /// datagram too (both relayed), not one datagram each.
    #[tokio::test]
    async fn records_sent_together_are_relayed_together() {
        use hsmp_ipc::schema::{combat::Vitals, pose::Root};
        let (socket, state, mut a, mut b) = live_duel(46_100).await;
        // Let the relay plan see both players first.
        for _ in 0..3 { a.pump(&socket, &state).await; b.pump(&socket, &state).await; }
        let mut buf = [0u8; 2048];
        while b.sock.try_recv_from(&mut buf).is_ok() {}
        let root = Root { tick: 1, ts: 1000, send_wall_ms: 0, pos: [100.0, 200.0, 50.0], rot: [0.0, 0.0, 0.0, 1.0], vel: [0.0; 3] };
        let vit = Vitals { seq: 7, ..Default::default() };
        let msgs = [hsmp_ipc::wire::encode(0, 0, &root, &[]), hsmp_ipc::wire::encode(0, 0, &vit, &[])];
        for m in &msgs {
            let (h, _) = hsmp_ipc::wire::split(m).unwrap();
            a.c.send(proto::record_mode(h.kind, 0).unwrap(), m.clone()).unwrap();
        }
        let now = state.net.now_ms();
        let dg = a.c.poll_transmit(now).expect("one datagram");
        assert!(a.c.poll_transmit(now).is_none(), "the client packed both records into one datagram");
        let (act, _) = state.net.handle(a.addr, &dg);
        let Action::Data { deliveries, peer, .. } = act else { panic!("data expected") };
        assert_eq!(deliveries.len(), 2);
        handle_deliveries(&socket, &state, peer, deliveries).await;
        tokio::time::sleep(Duration::from_millis(20)).await;
        let mut dgs = 0;
        let mut kinds = Vec::new();
        let now = state.net.now_ms();
        while let Ok((n, _)) = b.sock.try_recv_from(&mut buf) {
            dgs += 1;
            b.c.handle(now, &buf[..n]);
        }
        while let Some(ev) = b.c.poll_event() {
            if let hsmp_net::net::ClientEvent::Message(d) = ev { kinds.push(hsmp_ipc::wire::kind_of(&d.data)); }
        }
        assert!(kinds.contains(&hsmp_ipc::schema::pose::K_ROOT) && kinds.contains(&hsmp_ipc::schema::combat::K_VITALS), "{kinds:x?}");
        assert_eq!(dgs, 1, "root and vitals in one datagram");
    }
}
