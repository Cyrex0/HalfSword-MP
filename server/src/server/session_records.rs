//! Session domain records on the server (kinds `0x02xx`): the C2S records a
//! client's sidecar framed from its game (command, game_status, spawned, leave, chat, ping)
//! are validated in place (`hsmp_ipc::record::view`) and acted on; the S2C records the server
//! originates (welcome, session, pings, cmd_result, notice, kicked, server_closing, chat_in,
//! admin_state, pong) are built as structs and framed once with `hsmp_ipc::wire::encode`.

use super::*;
use hsmp_ipc::layout::Str;
use hsmp_ipc::record::{view, Invalid};
use hsmp_ipc::schema::session as rec;
use hsmp_ipc::wire;

// ---- builders (S2C) ------------------------------------------------------------------

/// The `welcome` record (first message after the handshake).
pub(crate) fn welcome_msg(peer_id: PeerId, seat: u8, server_epoch: u64, server_time_ms: u64, caps: u64, nick: &str) -> Vec<u8> {
    let w = rec::Welcome {
        server_epoch, server_time_ms, caps, peer_id, seat, role: rec_role::FIGHTER, _r: 0, nick: Str::new(nick),
    };
    wire::encode(0, 0, &w, &[])
}

/// Role codes (the schema's `role` table).
pub(crate) mod rec_role {
    pub const FIGHTER: u8 = 0;
}

/// A `notice` record (args beyond four are dropped, each clipped to 48 bytes).
pub(crate) fn notice_msg(event_id: u32, match_id: u64, code: u16, args: &[&str]) -> Vec<u8> {
    let mut n = rec::Notice { match_id, event_id, code, ..Default::default() };
    for (i, a) in args.iter().take(rec::NOTICE_ARGS).enumerate() {
        n.args[i] = Str::new(a);
    }
    wire::encode(0, 0, &n, &[])
}

/// Queue a notice for everyone (sent by `flush_out`).
pub(crate) fn push_notice(inner: &mut Inner, code: u16, args: &[&str]) {
    let event_id = inner.sess.next_event_id();
    let match_id = inner.sess.match_id;
    inner.out_msgs.push((None, notice_msg(event_id, match_id, code, args)));
}

/// A `chat_in` record (from_peer 0 = the server).
pub(crate) fn chat_in_msg(from_peer: PeerId, nick: &str, text: &str) -> Vec<u8> {
    let c = rec::ChatIn { from_peer, _r: 0, nick: Str::new(nick), text: Str::new(text) };
    wire::encode(0, from_peer, &c, &[])
}

/// A server chat line ("SERVER": from_peer 0).
pub(crate) fn server_chat_msg(text: &str) -> Vec<u8> {
    chat_in_msg(0, "SERVER", text)
}

/// Queue a server chat line for everyone (sent by `flush_out`).
pub(crate) fn push_server_chat(inner: &mut Inner, text: &str) {
    inner.out_msgs.push((None, server_chat_msg(text)));
}

/// The `kicked` record.
pub(crate) fn kicked_msg(event_id: u32, code: u8, reason: &str, retry_after_s: u32) -> Vec<u8> {
    let k = rec::Kicked { match_id: 0, event_id, retry_after_s, code, _r: [0; 7], reason: Str::new(reason) };
    wire::encode(0, 0, &k, &[])
}

/// The `server_closing` record.
pub(crate) fn closing_msg(reason: u8, text: &str, reconnect_after_ms: u32) -> Vec<u8> {
    let c = rec::ServerClosing { reconnect_after_ms, reason, _r: [0; 3], text: Str::new(text) };
    wire::encode(0, 0, &c, &[])
}

/// The `cmd_result` record.
pub(crate) fn cmd_result_msg(r: &rec::CmdResult) -> Vec<u8> {
    wire::encode(0, 0, r, &[])
}

/// The `admin_state` record one peer gets (admin.rs `admin_state_for`).
pub(crate) fn admin_state_msg(admin_peer: PeerId, bans: &[String]) -> Vec<u8> {
    let rows: Vec<rec::BanRow> = bans.iter().take(rec::MAX_BANS).map(|b| rec::BanRow { entry: Str::new(b) }).collect();
    wire::encode(0, 0, &rec::AdminStateHead { admin_peer, n: 0, _r: 0 }, &rows)
}

/// The `pings` record.
pub(crate) fn pings_msg(epoch: u64, server_time_ms: u64, rows: &[rec::PingRow]) -> Vec<u8> {
    let n = rows.len().min(rec::MAX_PINGS);
    wire::encode(0, 0, &rec::PingsHead { epoch, server_time_ms, n: 0, _r: [0; 3] }, &rows[..n])
}

/// The `session` record (head + roster rows), framed once per broadcast.
pub(crate) fn session_msg(s: &SessionSnap) -> Vec<u8> {
    wire::encode(0, 0, &s.head, &s.rows)
}

/// The session snapshot as built from server state (`session::build_session`).
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct SessionSnap {
    pub head: rec::SessionHead,
    pub rows: Vec<rec::RosterRow>,
}

// ---- small send helpers ---------------------------------------------------------------

/// Send one record message to `addr` (its kind's channel), outside every lock.
pub(crate) async fn send_msg_to(socket: &UdpSocket, state: &ServerState, addr: SocketAddr, msg: Vec<u8>) {
    let out = state.net.send_msg(addr, msg);
    send_out(socket, state, out).await;
}

/// Send the same record message to every address (one encode; a copy per destination).
pub(crate) async fn broadcast_msg(socket: &UdpSocket, state: &ServerState, addrs: &[SocketAddr], msg: Vec<u8>) {
    let n = addrs.len();
    let mut msg = Some(msg);
    for (i, a) in addrs.iter().enumerate() {
        let m = if i + 1 == n { msg.take().unwrap_or_default() } else { msg.clone().unwrap_or_default() };
        send_msg_to(socket, state, *a, m).await;
    }
}

/// A server chat line to one peer.
pub(crate) async fn chat_to(socket: &UdpSocket, state: &ServerState, addr: SocketAddr, text: &str) {
    send_msg_to(socket, state, addr, server_chat_msg(text)).await;
}

// ---- inbound (C2S) ----------------------------------------------------------------------

/// One session-domain record from `from` (records.rs). Every kind is validated first; a
/// kind a client may not send is refused.
pub(super) async fn handle(socket: &Arc<UdpSocket>, state: &Arc<ServerState>, from: SocketAddr,
                           h: hsmp_ipc::wire::WireHdr, payload: &[u8]) -> anyhow::Result<()> {
    match h.kind {
        rec::K_COMMAND => {
            let v = view::<rec::Command>(payload).map_err(super::records::refused)?;
            let cmd = v.head();
            let _ = run_command(socket, state, Actor::Peer(from), &cmd).await;
        }
        rec::K_GAME_STATUS => {
            let v = view::<rec::GameStatus>(payload).map_err(super::records::refused)?;
            on_game_status(socket, state, from, &v.head).await;
        }
        rec::K_SPAWNED => {
            let v = view::<rec::Spawned>(payload).map_err(super::records::refused)?;
            on_spawned(state, from, &v.head).await;
        }
        rec::K_LEAVE => {
            let v = view::<rec::Leave>(payload).map_err(super::records::refused)?;
            on_leave(socket, state, from, v.head.reason).await?;
        }
        rec::K_CHAT => {
            let v = view::<rec::Chat>(payload).map_err(super::records::refused)?;
            on_chat(socket, state, from, v.head.text.as_str().unwrap_or("")).await;
        }
        rec::K_PING => {
            let v = view::<rec::Ping>(payload).map_err(super::records::refused)?;
            let server_time_ms = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0);
            let pong = rec::Pong { client_time_ms: v.head.client_time_ms, server_time_ms };
            send_msg_to(socket, state, from, wire::encode(0, 0, &pong, &[])).await;
        }
        k => {
            // Server-originated or game-local kinds never come from a client.
            let r = hsmp_ipc::schema::check_payload(k, payload);
            return Err(super::records::refused(match r {
                Err(e) => e,
                Ok(()) => Invalid::Kind(k),
            }));
        }
    }
    Ok(())
}

/// Validate a client-sent session record without acting (decode fuzz, tools): the same
/// checks [`handle`] applies before acting.
#[allow(dead_code)]
pub(crate) fn validate_c2s(kind: u16, payload: &[u8]) -> Result<(), Invalid> {
    match kind {
        rec::K_COMMAND => view::<rec::Command>(payload).map(|_| ()),
        rec::K_GAME_STATUS => view::<rec::GameStatus>(payload).map(|_| ()),
        rec::K_SPAWNED => view::<rec::Spawned>(payload).map(|_| ()),
        rec::K_LEAVE => view::<rec::Leave>(payload).map(|_| ()),
        rec::K_CHAT => view::<rec::Chat>(payload).map(|_| ()),
        rec::K_PING => view::<rec::Ping>(payload).map(|_| ()),
        k => Err(Invalid::Kind(k)),
    }
}

/// `chat` from a peer: trimmed, at most 256 characters, budgeted, broadcast as `chat_in`.
async fn on_chat(socket: &Arc<UdpSocket>, state: &Arc<ServerState>, from: SocketAddr, text: &str) {
    let trimmed = text.trim();
    if trimmed.is_empty() || trimmed.chars().count() > 256 { return; }
    let (from_id, from_nick, addrs) = {
        let mut inner = state.inner.lock().await;
        let st = inner.server_tick;
        let (fid, fnick) = match inner.peers.get_mut(&from) {
            Some(p) => { p.last_seen_tick = st; (p.id, p.nick.clone()) }
            None => return,
        };
        if !super::dispatch::within_budget(state, fid, crate::validate::rate::Kind::Chat, 1.0) {
            debug!(peer_id = fid, "chat rate-limited");
            return;
        }
        (fid, fnick, inner.peers.keys().copied().collect::<Vec<_>>())
    };
    info!(from_id, nick = %from_nick, len = trimmed.len(), "chat");
    broadcast_msg(socket, state, &addrs, chat_in_msg(from_id, &from_nick, trimmed)).await;
}

/// `leave` from a peer (on purpose: BACK TO MENU / LEAVE MATCH / quit).
async fn on_leave(socket: &Arc<UdpSocket>, state: &Arc<ServerState>, from: SocketAddr, reason: u8) -> anyhow::Result<()> {
    use hsmp_net::net::close_code;
    info!(%from, reason, "client leaving");
    // On purpose = forfeit now (no reconnect pause) + "left" notice; a listen host
    // leaving closes the server for everyone ("Host closed the server").
    let host_closes = {
        let mut inner = state.inner.lock().await;
        let closes = session::host_closes_on_leave(&inner, from);
        session::on_deliberate_leave(&mut inner, from, reason);
        closes
    };
    if host_closes {
        let out = state.net.close(from, close_code::NORMAL, "leave");
        send_out(socket, state, out).await;
        peer_leave(socket, state, from).await?;
        info!(%from, "listen host left: closing the server");
        shutdown(socket, state, v5::ClosingReason::HOST_LEFT, "Host closed the server").await;
        return Ok(());
    }
    let code = if reason == v5::LeaveReason::GAME_EXITED { close_code::GAME_EXITED } else { close_code::NORMAL };
    let out = state.net.close(from, code, "leave");
    send_out(socket, state, out).await;
    peer_leave(socket, state, from).await
}

/// `spawned` from a peer: it placed its pawn on its order (logged once per round) and its
/// spawn protection starts (`note_placed`).
async fn on_spawned(state: &Arc<ServerState>, from: SocketAddr, s: &rec::Spawned) {
    let mut inner = state.inner.lock().await;
    let Some(pid) = inner.peers.get(&from).map(|p| p.id) else { return };
    let planned = inner.spawn_plan.iter().find(|a| a.peer_id == pid).map(|a| a.pos);
    let off = planned
        .map(|p| ((p[0] - s.pos[0]).powi(2) + (p[1] - s.pos[1]).powi(2)).sqrt())
        .unwrap_or(f32::NAN);
    let seen = inner.match_peers.entry(pid).or_default();
    if seen.spawned_round != s.round {
        seen.spawned_round = s.round;
        info!(peer_id = pid, round = s.round, slot = s.slot, x = s.pos[0], y = s.pos[1], z = s.pos[2],
              clear = s.clear.get(), offset_cm = off, "spawn report: pawn placed");
    }
    note_placed(&mut inner, pid, s.round);
}
