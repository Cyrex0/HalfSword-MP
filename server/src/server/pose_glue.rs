//! Pose domain of the protocol v6 record dispatch (`records.rs`): `root`, `weapon` and
//! `pose` from a client, validated in place and acted on as they arrived.
//!
//! - `root`: `record::view` + the input gate (`validate::input::check_root`), the per-sender
//!   stream rate, the speed cap (`accept_root`), lag compensation, then the relay: the same
//!   bytes with `peer` = the owner (relay.rs rate plan, budgets and relevance).
//! - `weapon`: validated and kept for lag compensation only (no receiver reads it: the
//!   peers get the weapon inside the pose frame), so it is not relayed.
//! - `pose`: the codec v2 frame must decode (structural check); the ONE decode feeds lag
//!   compensation (hero bones, blade, capsules). Relayed with `peer` = owner and `aux` = the
//!   pair's relay interval, patched per receiver.

use super::*;
use crate::validate::rate::StreamKind;
use hsmp_ipc::record::view;
use hsmp_ipc::schema::pose::{PoseHead, Root, Weapon, K_POSE, K_ROOT, K_WEAPON};

/// Is `kind` one of this domain's client records?
pub(super) fn owns(kind: u16) -> bool {
    matches!(kind, K_ROOT | K_WEAPON | K_POSE)
}

/// One `root` / `weapon` / `pose` record from `from`.
pub(super) async fn handle(socket: &Arc<UdpSocket>, state: &Arc<ServerState>, from: SocketAddr, kind: u16, payload: &[u8]) -> anyhow::Result<()> {
    match kind {
        K_ROOT => on_root(socket, state, from, payload).await,
        K_WEAPON => on_weapon(state, from, payload).await,
        _ => on_pose(socket, state, from, payload).await,
    }
}

/// The input gate's refusal (non-finite / out-of-world floats), logged at most once a second.
fn gate_refused(from: SocketAddr, field: &'static str) -> anyhow::Error {
    if crate::validate::rate::log_ok("v6_pose_gate") {
        warn!(%from, field, "speed cap exceeded / unusable float (non-finite or outside the world); record dropped");
    }
    super::records::refused(hsmp_ipc::record::Invalid::Range(field))
}

async fn on_root(socket: &Arc<UdpSocket>, state: &Arc<ServerState>, from: SocketAddr, payload: &[u8]) -> anyhow::Result<()> {
    let r = view::<Root>(payload).map_err(super::records::refused)?.head();
    crate::validate::input::check_root(&r).map_err(|f| gate_refused(from, f))?;
    if !super::dispatch::stream_ok(state, from, StreamKind::Root) {
        return Ok(());
    }
    if let Some(sid) = accept_root(state, from, r.pos, r).await {
        crate::lagcomp::record_root(sid, r.ts, r.pos);
        relay_record(socket, state, from, crate::relay::Stream::Root, hsmp_ipc::wire::message(K_ROOT, 0, sid, payload)).await;
    }
    Ok(())
}

async fn on_weapon(state: &Arc<ServerState>, from: SocketAddr, payload: &[u8]) -> anyhow::Result<()> {
    let w: Weapon = view::<Weapon>(payload).map_err(super::records::refused)?.head();
    crate::validate::input::check_weapon(&w).map_err(|f| gate_refused(from, f))?;
    if !super::dispatch::stream_ok(state, from, StreamKind::Weapon) {
        return Ok(());
    }
    let Some(sid) = touch_peer(state, from).await else { return Ok(()) };
    crate::lagcomp::record_weapon(sid, w.ts, w.pos);
    Ok(())
}

async fn on_pose(socket: &Arc<UdpSocket>, state: &Arc<ServerState>, from: SocketAddr, payload: &[u8]) -> anyhow::Result<()> {
    let v = view::<PoseHead>(payload).map_err(super::records::refused)?;
    if !super::dispatch::stream_ok(state, from, StreamKind::Skel) {
        state.relay.note_pose_in(from, false);
        return Ok(());
    }
    // The structural check is the decode lag compensation needs anyway: done once.
    let Some(full) = crate::posecodec::v2::decode(&v.rows) else {
        return Err(super::records::refused(hsmp_ipc::record::Invalid::Range("frame")));
    };
    let Some(sid) = touch_peer(state, from).await else { return Ok(()) };
    crate::stats::pose_in(sid);
    // A frame lag comp rejects (pose not tied to the root) is not relayed either.
    if !crate::lagcomp::record_skeletal_v2(sid, &full) {
        state.relay.note_pose_in(from, false);
        return Ok(());
    }
    state.relay.note_pose_in(from, true);
    // Codec v2 frames carry the exact blade (grip, tip, tip velocity) and capsules for all 22
    // bodies.
    let x = crate::posecodec::v2::extras_of(&full);
    if let Some((base, tip, vel)) = x.blade {
        crate::lagcomp::record_blade(sid, x.ts, crate::lagcomp::Blade { base, tip, vel: Some(vel) });
    }
    let caps: Vec<crate::lagcomp::Capsule> = x.caps.iter().map(|c| crate::lagcomp::Capsule { a: c.0, b: c.1, r: c.2 }).collect();
    crate::lagcomp::record_capsules(sid, x.ts, &caps);
    relay_pose(socket, state, from, sid, hsmp_ipc::wire::message(K_POSE, 0, sid, payload), x.ts).await;
    Ok(())
}
