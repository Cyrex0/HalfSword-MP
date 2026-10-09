//! Wire protocol v10: life-bound client <-> server records (docs/development/protocol.md;
//! docs/development/ipc-shared-memory.md).
//!
//! The transport (handshake, encryption, channels) lives in `hsmp-net` (`crate::net` on the
//! server, `sidecar/net.rs` on the client). Every channel message is one typed record:
//! `[WireHdr kind,aux,peer: 8 B][record payload]` (`hsmp_ipc::wire`), validated in place by
//! `hsmp_ipc::record::view` for its kind. [`record_mode`] picks the channel. There is no
//! envelope and no session token: the connection authenticates the sender.

/// The negotiated representation version (`hsmp_net::net::PROTOCOL_VERSION`).
#[allow(dead_code)] // not every binary that includes proto.rs uses it
pub const PROTOCOL_VERSION: u32 = hsmp_net::net::PROTOCOL_VERSION as u32;

pub type PeerId = u32;

/// The protocol codes (`v5::Phase`, `v5::CmdReason`, `v5::AdminRole`, ...) and the channel
/// keys (`v5::keys`) the records carry.
#[allow(unused_imports)]
pub use hsmp_net::proto_v5 as v5;

/// Split one received channel message into its wire header and record payload. Every kind
/// (including 0, the retired bincode envelope) is a record kind to the caller: an unknown
/// one is validated and dropped like any other. Errors are dropped by the caller (counted /
/// logged), never acted on.
pub fn decode_msg(data: &[u8]) -> Result<(hsmp_ipc::wire::WireHdr, &[u8]), String> {
    hsmp_ipc::wire::split(data).map_err(|e| e.to_string())
}

/// The hsmp-net channel of a record kind (`hsmp_ipc::schema::Chan`), keyed by the source
/// peer for the per-peer streams. `None` for a kind that is not a network record.
pub fn record_mode(kind: u16, peer: PeerId) -> Option<hsmp_net::net::SendMode> {
    use hsmp_ipc::schema::Chan;
    use hsmp_net::net::SendMode;
    use hsmp_net::proto_v5::keys::key;
    // Additive native DTOs are network-only; the shared-memory schema stays unchanged.
    match kind {
        0x0A80 => return Some(SendMode::Ordered),
        0x0A10 => return Some(SendMode::ReliableLatest { key: key(0x92, 0) }),
        0x0AC0 => return Some(SendMode::ReliableLatest { key: key(0x93, 0) }),
        0x0AC1 => return Some(SendMode::Reliable),
        0x0A11 | 0x0A12 => return Some(SendMode::Latest { key: key(0x94, 0) }),
        0x0A81 => return Some(SendMode::Ordered),
        _ => {}
    }
    Some(match hsmp_ipc::schema::record_info(kind)?.chan {
        Chan::None => return None,
        Chan::Latest(s) => SendMode::Latest { key: key(s, peer) },
        Chan::RelLatest(s) => SendMode::ReliableLatest { key: key(s, peer) },
        Chan::Reliable => SendMode::Reliable,
        Chan::Ordered => SendMode::Ordered,
    })
}

/// Vitals frame codec (src/vitals.rs).
#[path = "vitals.rs"]
pub mod vitals;

#[cfg(test)]
mod route_tests {
    use super::*;
    use hsmp_net::net::SendMode;
    use hsmp_net::proto_v5::keys::{self, key};

    #[test]
    fn native_render_revisions_route_to_the_same_scene_stream() {
        assert_eq!(record_mode(0x0A11, 0), Some(SendMode::Latest { key: key(0x94, 0) }));
        assert_eq!(record_mode(0x0A12, 0), record_mode(0x0A11, 0));
    }

    #[test]
    fn interact_records_route_per_hand() {
        use hsmp_ipc::schema::interact::*;
        // Grab updates supersede per grabbing hand (C2S, peer 0) and per (initiator, hand)
        // (S2C, peer = initiator); every other interaction is reliable.
        assert_eq!(record_mode(K_INTERACT_GRAB_R, 0), Some(SendMode::ReliableLatest { key: key(STREAM_GRAB_R, 0) }));
        assert_eq!(record_mode(K_INTERACT_GRAB_L, 0), Some(SendMode::ReliableLatest { key: key(STREAM_GRAB_L, 0) }));
        assert_eq!(record_mode(K_INTERACT_GRAB_L, 3), Some(SendMode::ReliableLatest { key: key(STREAM_GRAB_L, 3) }));
        assert_ne!(record_mode(K_INTERACT_GRAB_R, 3), record_mode(K_INTERACT_GRAB_L, 3));
        assert_eq!(record_mode(K_INTERACT, 3), Some(SendMode::Reliable));
        assert_eq!(record_mode(K_POSE_YIELD, 0), None, "a bus record never goes on the wire");
        // No v5 / v4 stream uses the grab streams.
        for s in [keys::ROOT, keys::SKEL, keys::WEAPON, keys::VITALS, keys::WORLD, keys::PING, keys::VOICE,
                  keys::KIT, keys::LOADOUT, hsmp_ipc::schema::combat::STREAM_VITALS] {
            assert!(s != STREAM_GRAB_R && s != STREAM_GRAB_L, "stream {s:#x}");
        }
        for k in [keys::SESSION, keys::GAME_STATUS, keys::PINGS, keys::MATCH_STATE, keys::KIT_RULES] {
            assert!(keys::stream_of(k) != STREAM_GRAB_R && keys::stream_of(k) != STREAM_GRAB_L);
        }
    }
}

/// A damage claim as the server keeps it: the `damage` record head
/// (`hsmp_ipc::schema::combat::Damage`) and its delta rows, owned (a decision holds the
/// claim across the lag-compensation wait / defender grace). Built from the validated
/// record view; sent back out as the same record (`msg`). Derefs to the head, so the
/// record's fields are used directly (`hit.target_peer_id`, `hit.bone`, ...).
#[allow(dead_code)] // not every binary that includes proto.rs keeps claims
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct DamageEvent {
    pub h: Damage,
    deltas_buf: [DamageDelta; hsmp_ipc::schema::combat::MAX_DELTAS],
    n: usize,
}

pub use hsmp_ipc::schema::combat::{Damage, DamageDelta};

impl core::ops::Deref for DamageEvent {
    type Target = Damage;
    fn deref(&self) -> &Damage { &self.h }
}
impl core::ops::DerefMut for DamageEvent {
    fn deref_mut(&mut self) -> &mut Damage { &mut self.h }
}

#[allow(dead_code)]
impl DamageEvent {
    /// A claim from a record head and its delta rows (rows beyond the capacity dropped).
    pub fn new(h: Damage, deltas: &[DamageDelta]) -> Self {
        let mut e = DamageEvent { h, ..Default::default() };
        e.set_deltas(deltas);
        e
    }
    /// From a validated record view.
    pub fn from_view(v: &hsmp_ipc::record::View<'_, Damage>) -> Self {
        Self::new(v.head(), &v.rows)
    }
    /// The delta rows.
    pub fn deltas(&self) -> &[DamageDelta] { &self.deltas_buf[..self.n] }
    pub fn deltas_mut(&mut self) -> &mut [DamageDelta] { &mut self.deltas_buf[..self.n] }
    /// Replace the delta rows (truncated to the capacity).
    pub fn set_deltas(&mut self, d: &[DamageDelta]) {
        let n = d.len().min(self.deltas_buf.len());
        self.deltas_buf[..n].copy_from_slice(&d[..n]);
        self.n = n;
        self.h.n = n as u8;
    }
    /// Keep the deltas `f` accepts.
    pub fn retain_deltas(&mut self, mut f: impl FnMut(&DamageDelta) -> bool) {
        let kept: Vec<DamageDelta> = self.deltas().iter().copied().filter(|d| f(d)).collect();
        self.set_deltas(&kept);
    }
    /// The bone name.
    pub fn bone_str(&self) -> &str { self.h.bone_str() }
    /// This claim as a v6 message of `kind` (`damage_in` / `hitfx_in` to receivers,
    /// `damage` from a client) with `WireHdr.peer` = `peer`.
    pub fn msg(&self, kind: u16, peer: PeerId) -> Vec<u8> {
        let mut m = hsmp_ipc::wire::encode(0, peer, &self.h, self.deltas());
        m[..2].copy_from_slice(&kind.to_le_bytes());
        m
    }
}

#[allow(dead_code)]
impl DamageEvent {
    /// The deltas as (field, change) pairs (logs, tests).
    pub fn delta_pairs(&self) -> Vec<(u8, f32)> {
        self.deltas().iter().map(|d| (d.i, d.v)).collect()
    }
    /// Replace the deltas from (field, change) pairs.
    pub fn set_delta_pairs(&mut self, p: &[(u8, f32)]) {
        let d: Vec<DamageDelta> = p.iter().map(|&(i, v)| DamageDelta::new(i, v)).collect();
        self.set_deltas(&d);
    }
    /// Set the bone name (truncated to its capacity).
    pub fn set_bone(&mut self, b: &str) {
        self.h.bone = hsmp_ipc::layout::Str::new(b);
    }
}

#[allow(dead_code)]
/// Delta rows from (field, change) pairs.
pub fn deltas_of(p: &[(u8, f32)]) -> Vec<DamageDelta> {
    p.iter().map(|&(i, v)| DamageDelta::new(i, v)).collect()
}
