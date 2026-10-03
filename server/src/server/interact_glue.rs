//! Interaction-channel glue (docs/development/subsystems/interact.md): a C2S `interact`
//! record (protocol v6) -> validation (`crate::interact`) -> the record to the owner of the
//! affected body (`WireHdr::peer` = the initiator, 0 = the server); the per-tick lease
//! expiry / departed-peer cleanup.

use super::*;
use crate::interact::{self, Ctx, Interact, Route, K};
use hsmp_ipc::schema::interact::wire_kind;

/// One C2S interaction record (`interact`, `interact_grab_r` / `_l`) from `from`, validated
/// in place (`record::view`: size, finite floats, kind / hand / bone / bounds).
pub(super) async fn on_interact_record(socket: &UdpSocket, state: &Arc<ServerState>, from: SocketAddr, payload: &[u8])
    -> anyhow::Result<()> {
    let ev = hsmp_ipc::record::view::<Interact>(payload).map_err(super::records::refused)?.head();
    on_interact(socket, state, from, ev).await;
    Ok(())
}

/// One validated interaction from `from`.
pub(super) async fn on_interact(socket: &UdpSocket, state: &Arc<ServerState>, from: SocketAddr, ev: Interact) {
    let (initiator, ctx) = {
        let inner = state.inner.lock().await;
        let Some(a) = inner.peers.get(&from) else { return };
        let t = inner.peers.values().find(|p| p.id == ev.target_peer);
        let roots = match (a.last_valid_pos, t.and_then(|p| p.last_valid_pos)) {
            (Some(x), Some(y)) => Some((x, y)),
            _ => None,
        };
        (a.id, Ctx {
            initiator_caps: interact::has_interact(a.id),
            initiator_alive: a.alive,
            target_present: t.is_some(),
            target_caps: t.map_or(false, |p| interact::has_interact(p.id)),
            frozen: matches!(inner.match_state.as_str(), "countdown" | "paused"),
            roots,
        })
    };
    let now = crate::lagcomp::now_ms();
    let v = interact::with(|ix| ix.on_c2s(initiator, ev, &ctx, |e| {
        let hand = (e.kind != K::IMPULSE).then(|| interact::hand_bone(e.hand));
        crate::lagcomp::interact_distance(initiator, e.target_peer, e.bone as usize, hand, e.ts)
    }, now));
    if let Some(note) = &v.note {
        if v.routes.is_empty() || v.routes.iter().any(|r| r.ev.kind == K::GRAB_DENIED) {
            debug!(initiator, target = ev.target_peer, kind = ev.kind, id = ev.id, %note, "interaction refused");
        } else {
            debug!(initiator, target = ev.target_peer, kind = ev.kind, %note, "interaction altered");
        }
    }
    if ev.kind == K::GRAB_START && v.routes.iter().any(|r| r.ev.kind == K::GRAB_START) {
        info!(initiator, target = ev.target_peer, id = ev.id, hand = ev.hand, bone = ev.bone, "grab started");
    }
    send_routes(socket, state, v.routes).await;
}

/// Server tick: lease expiry + cleanup of peers that are gone.
pub(super) async fn interact_tick(socket: &UdpSocket, state: &Arc<ServerState>) {
    let present: Vec<PeerId> = {
        let inner = state.inner.lock().await;
        inner.peers.values().map(|p| p.id).collect()
    };
    let routes = interact::tick(&present, crate::lagcomp::now_ms());
    if routes.is_empty() { return; }
    for r in &routes {
        info!(to = r.to, from = r.from, id = r.ev.id, "grab ended by the server (lease expired / peer gone)");
    }
    send_routes(socket, state, routes).await;
}

/// The S2C message of one route: the record behind `[kind, aux 0, peer = from]`. A grab
/// update keeps its hand's supersede stream (keyed by the initiator).
pub(crate) fn route_msg(r: &Route) -> Vec<u8> {
    hsmp_ipc::wire::message(wire_kind(&r.ev), 0, r.from, hsmp_ipc::bytemuck::bytes_of(&r.ev))
}

async fn send_routes(socket: &UdpSocket, state: &Arc<ServerState>, routes: Vec<Route>) {
    if routes.is_empty() { return; }
    let addrs: Vec<(PeerId, SocketAddr)> = {
        let inner = state.inner.lock().await;
        inner.peers.iter().map(|(a, p)| (p.id, *a)).collect()
    };
    for r in routes {
        // Capability rule: never send an interaction to a peer that did not
        // negotiate INTERACT.
        if !interact::has_interact(r.to) { continue; }
        if let Some((_, addr)) = addrs.iter().find(|(id, _)| *id == r.to) {
            let out = state.net.send_msg(*addr, route_msg(&r));
            send_out(socket, state, out).await;
        }
    }
}
