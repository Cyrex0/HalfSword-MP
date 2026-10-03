//! G2S typed records (IPC ABI 2): the `hsmp-ipc` thread hands every validated
//! record from the game's G2S ring here, as it is. Each domain adds one arm per kind it owns
//! and acts on the bytes: frame them for the server (`ShmLink::send_record` with
//! `hsmp_ipc::wire::message`), or feed its client's logic (resend tables, gates), which may
//! patch ids / sequence numbers / rounds in place before framing.
//!
//! Runs on the `hsmp-ipc` thread: never block here (no locks held across awaits, no IO).

use super::ipc_shm::ShmLink;
use super::session_client;
use hsmp_ipc::ring::RecordHeader;
use hsmp_ipc::schema::session as rs;
use tracing::debug;

pub(super) fn on_g2s(l: &'static ShmLink, h: &RecordHeader, payload: &[u8]) {
    match h.kind {
        // ---- session, match, connection ----
        // A command: resent until its result arrives (session_client.rs).
        rs::K_COMMAND => {
            if let Some(m) = session_client::submit(payload) {
                l.send_record(m);
            }
        }
        // Framed as they are.
        rs::K_GAME_STATUS | rs::K_SPAWNED | rs::K_CHAT => l.send_record(session_client::frame(h.kind, payload)),
        // Leave the server and exit (main's select!).
        rs::K_LEAVE => session_client::request_leave(payload),
        // Grabs and shoves: wire id minted / mapped in place, framed for the server.
        hsmp_ipc::schema::interact::K_INTERACT => super::interact_client::on_g2s(l, payload),
        // Combat (damage claims, clash / touch, death reports): combat_client.rs.
        k if k >> 8 == 0x03 => crate::combat_client::on_g2s(k, payload),
        // World replication: claims and sync requests go to the server as they are.
        hsmp_ipc::schema::world::K_WORLD_CLAIM | hsmp_ipc::schema::world::K_WORLD_SYNC => {
            crate::world_client::on_g2s(l, h.kind, payload)
        }
        k => debug!(kind = k, len = payload.len(), "ipc: G2S record of a kind nobody handles; dropped"),
    }
}
