//! Protocol v6: typed record messages (docs/development/ipc-shared-memory.md).
//!
//! Every message is `[WireHdr kind,aux,peer][record payload]` (`hsmp_ipc::wire`). The payload
//! is the record exactly as the game wrote it into shared memory; the client's sidecar only
//! framed it. Here each kind is validated in place (`hsmp_ipc::record::view`: exact size,
//! row count within capacity, finite floats, canonical strings, 0/1 bools, then the record's
//! own range checks) and handed to its domain. Nothing is converted.
//!
//! Domains add their kinds to [`handle`] (one arm per kind, or one arm per domain range that
//! calls the domain's `handle_record`). A kind nobody handles is dropped and counted.

use super::*;

/// Records refused (bad payload) or unknown, by reason, for the transport stats line.
pub(crate) static REFUSED: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
pub(crate) static UNKNOWN: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// One v6 record message from `from`. `h.peer` from a client is ignored (the connection
/// says who sent it).
pub(super) async fn handle(
    socket: &Arc<UdpSocket>,
    state: &Arc<ServerState>,
    from: SocketAddr,
    h: hsmp_ipc::wire::WireHdr,
    payload: &[u8],
) -> anyhow::Result<()> {
    let _ = (socket, state, from, payload);
    // A player still loading the server's mods sends nothing else (mods_glue.rs).
    if super::mods_glue::gate(state, from, h.kind).await {
        return Ok(());
    }
    match h.kind {
        // Server mods (0x09xx): chunk requests and the loaded report (mods_glue.rs).
        k if hsmp_ipc::schema::mods::is_mods_kind(k) => super::mods_glue::handle_record(socket, state, from, h, payload).await,
        // Session, match and connection (0x02xx): session_records.rs.
        k if k >> 8 == 0x02 => super::session_records::handle(socket, state, from, h, payload).await,
        // Grabs and shoves (interact_glue.rs): validated, judged, forwarded to the body's owner.
        k if hsmp_ipc::schema::interact::is_interact_kind(k) => {
            super::interact_glue::on_interact_record(socket, state, from, payload).await
        }
        // Domains plug in here (pose 0x01xx, session 0x02xx, combat 0x03xx, world 0x04xx,
        // loadout 0x05xx, interact 0x06xx).
        k if k >> 8 == 0x03 => super::combat_glue::handle_record(socket, state, from, h, payload).await,
        0x0400..=0x04FF => return super::world_glue::handle_record(socket, state, from, h.kind, payload).await,
        k if k >> 8 == 0x05 => crate::loadout::handle_record(socket, state, from, h, payload).await,
        k if super::pose_glue::owns(k) => super::pose_glue::handle(socket, state, from, k, payload).await,
        k => {
            // Validate anyway: an unknown-but-registered kind is still checked, so a fuzzer
            // or a hostile client never reaches anything but the validator.
            let r = hsmp_ipc::schema::check_payload(k, payload);
            UNKNOWN.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            if crate::validate::rate::log_ok("v6_unknown_kind") {
                debug!(%from, kind = k, valid = r.is_ok(), "v6 record of a kind the server does not handle; dropped");
            }
            Ok(())
        }
    }
}

/// Count and describe a refused record (callers `return Err(refused(e))`).
#[allow(dead_code)]
pub(crate) fn refused(e: hsmp_ipc::record::Invalid) -> anyhow::Error {
    REFUSED.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    anyhow::anyhow!("invalid record: {e}")
}
