//! HSMP v5 transport (sans-IO). See `docs/development/protocol.md` for the wire format.
//!
//! Module map:
//! * [`wire`]: bounds-checked byte reader/writer.
//! * [`replay`]: 64-bit sequence reconstruction + 1024-packet replay window.
//! * [`crypto`]: HKDF key schedule, AEAD with `pkt_seq` nonces, key update.
//! * [`handshake`]: `C2SHello` / `S2CChallenge` / `C2SAuth` codecs, the
//!   stateless cookie and the server/client handshake logic.
//! * [`frag`]: fragmentation and bounded reassembly.
//! * [`channel`]: per-channel send queues, reliability bookkeeping and
//!   receive-side dedup/ordering; chunk codec.
//! * [`conn`]: one connection: header, acks, RTT/RTO, loss, packet assembly,
//!   keepalive, idle timeout, close, key update.
//! * [`endpoint`]: `ServerEndpoint` (routing by `conn_id`, handshake
//!   admission) and `Client` (handshake driver + connection).

pub mod channel;
pub mod conn;
pub mod crypto;
pub mod endpoint;
pub mod frag;
pub mod handshake;
pub mod replay;
pub mod wire;

#[cfg(any(test, feature = "testlink"))]
pub mod testlink;
#[cfg(test)]
mod tests;

pub use channel::{Delivery, SendError, SendMode, CH_ORDERED, CH_RELIABLE, CH_UNRELIABLE};
pub use conn::{close_code, Conn, ConnConfig, ConnState, ConnStats, Side};
pub use endpoint::{Client, ClientConfig, ClientEvent, Incoming, PendingAuth, ServerConfig, ServerEndpoint};
/// The header/handshake format version. v6 (HSMP-SHM ABI 2): every channel message is a
/// typed record behind an 8-byte header (`hsmp_ipc::wire`); the transport is unchanged.
pub const PROTOCOL_VERSION: u16 = 6;
/// Lowest protocol version this build accepts (v5 bodies are not v6 messages).
pub const VERSION_MIN: u16 = 6;
/// Highest protocol version this build speaks.
pub const VERSION_MAX: u16 = 6;

/// Largest datagram either side ever sends (safe below every common MTU).
pub const MAX_DATAGRAM: usize = 1200;
/// `C2SHello` is padded to at least this size (anti-amplification).
pub const HELLO_LEN: usize = 1200;

/// Packet type, byte 0 of every v5 datagram. Chosen to never collide with the
/// v4 bincode tags (0x00/0x01 LE u32) or the query magic (0xFF).
pub const PT_HELLO: u8 = 0xA1;
pub const PT_CHALLENGE: u8 = 0xA2;
pub const PT_AUTH: u8 = 0xA3;
pub const PT_PRE_REJECT: u8 = 0xA4;
pub const PT_AUTH_REJECT: u8 = 0xA5;
pub const PT_DATA: u8 = 0xB0;
/// Stateless reset: `0xA6 | conn_id u64 | token [16]` (25 bytes), sent by a
/// server for a data packet of a connection it does not know.
pub const PT_RESET: u8 = 0xA6;
pub const RESET_LEN: usize = 1 + 8 + 16;

/// `type u8 | conn_id u64 | pkt_seq u32 | ack u32 | ack_bits u32 | flags u8`
pub const DATA_HEADER_LEN: usize = 22;
pub const TAG_LEN: usize = 16;
/// Largest plaintext payload of one data packet (1162 B).
pub const MAX_PLAINTEXT: usize = MAX_DATAGRAM - DATA_HEADER_LEN - TAG_LEN;

pub type ConnId = u64;

/// Capability bits (negotiated as `client_caps & server_caps`). Every wire
/// addition after v5 hides behind one of these; none is required.
pub mod caps {
    /// `S2CSession.mode` sections other than `None`, `S2CEvent::KillFeed`.
    pub const MODES: u64 = 1 << 0;
    /// Zone (battle royale ring) state in `S2CSession.mode`.
    pub const ZONE: u64 = 1 << 1;
    /// Interaction channel messages (`Impulse`, `Grab`, `Clash`).
    pub const INTERACT: u64 = 1 << 2;
    /// Tournament bracket messages.
    pub const BRACKET: u64 = 1 << 3;
    /// `map_hash` in the session config and map download.
    pub const MAP_HASH: u64 = 1 << 4;
    /// Pose codec v2 (local rotations, smallest-three).
    pub const POSE2: u64 = 1 << 5;
    /// One bundled downstream datagram per tick (interest management).
    pub const BUNDLE: u64 = 1 << 6;
    /// Acked baselines + delta compression.
    pub const DELTA: u64 = 1 << 7;
    /// Resume tickets + path migration (reserved).
    pub const RESUME: u64 = 1 << 8;
    /// Password-protected servers (reserved).
    pub const PASSWORD: u64 = 1 << 9;
    /// `Msg::S2CPings`: every player's smoothed RTT, about once a second.
    pub const PING: u64 = 1 << 10;
    /// Transport: data packets may carry an `ACK_DELAY` chunk (how long the
    /// acked packet waited before this ack), subtracted from RTT samples.
    pub const ACK_DELAY: u64 = 1 << 11;
    /// Transport: the server hands out a stateless-reset token (chunk
    /// `RESET_TOKEN`) and answers data for an unknown connection with a
    /// `PT_RESET` the client can verify (fast re-handshake after a server
    /// restart).
    pub const RESET: u64 = 1 << 12;
    /// Application: relayed skeletal frames arrive as
    /// `Body::S2CSkeletalBroadcastRated` (the frame + the relay interval for
    /// this sender → receiver pair). Opted in by server and sidecar.
    pub const POSE_RATE: u64 = 1 << 13;
    /// Transport: ReliableLatest chunks carry their supersede key (channel
    /// byte | 0x80, u32 key after msg_id), so a receiver never delivers an
    /// older superseded copy after a newer one.
    pub const REL_KEY: u64 = 1 << 15;
    /// Transport: path validation by PATH_CHALLENGE / PATH_RESPONSE chunks
    /// (8 random bytes echoed from the new address).
    pub const PATH_CHALLENGE: u64 = 1 << 14;
    /// Application: combat hit effects. The client reports
    /// `Body::C2STouch` (a peer's stand-in reached its body on its own
    /// screen: evidence against a parry) and receives `Body::S2CHitFx` (every
    /// accepted hit on ANOTHER player, replayed on that player's stand-in for
    /// blood and wounds). Opted in by server and sidecar.
    pub const HIT_FX: u64 = 1 << 16;
    /// Application: the `body` record (the owner's passport body: Height / Muscle
    /// Rate, mass scales, bone masses), sent by the client and relayed to the other
    /// players that negotiated it, for their stand-ins of it. Opted in by server and
    /// sidecar; peers without it never send or receive the record.
    pub const BODY: u64 = 1 << 17;
    /// Application: server-served mods (`mod_manifest` / `mod_files` / `mod_chunk` down,
    /// `mod_chunk_req` / `mod_ready` up; docs/hosting/server-mods.md). The server offers it
    /// only when it has a `--mods-dir`, the sidecar always; a server with mods refuses a
    /// client without it (`reject_code::MODS_REQUIRED`).
    pub const SERVER_MODS: u64 = 1 << 18;
    /// Everything this build implements at the transport level (the
    /// application-level bits are opted in by the server / sidecar).
    pub const SUPPORTED: u64 = ACK_DELAY | RESET | PATH_CHALLENGE | REL_KEY;
}

/// The key per-source limits use for an IP address: IPv4 as is (an
/// IPv4-mapped IPv6 address counts as its IPv4), IPv6 by its /64 prefix
/// (one host or customer usually gets a whole /64, so a per-/128 limit is
/// no limit at all).
pub fn ip_key(ip: std::net::IpAddr) -> std::net::IpAddr {
    use std::net::{IpAddr, Ipv6Addr};
    match ip {
        IpAddr::V4(_) => ip,
        IpAddr::V6(v6) => match v6.to_ipv4_mapped() {
            Some(v4) => IpAddr::V4(v4),
            None => {
                let s = v6.segments();
                IpAddr::V6(Ipv6Addr::new(s[0], s[1], s[2], s[3], 0, 0, 0, 0))
            }
        },
    }
}

/// Highest common version of two inclusive ranges, if any.
pub fn negotiate_version(cmin: u16, cmax: u16, smin: u16, smax: u16) -> Option<u16> {
    if cmin > cmax || smin > smax {
        return None;
    }
    let lo = cmin.max(smin);
    let hi = cmax.min(smax);
    (lo <= hi).then_some(hi)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DatagramKind {
    /// Server-browser query (`query.rs`, magic `\xFFHSMPQ1\0`).
    Query,
    /// A v4 bincode datagram (`Datagram::Clear`/`Encrypted`).
    LegacyV4,
    Hello,
    Challenge,
    Auth,
    PreReject,
    AuthReject,
    Data,
    Reset,
    Unknown,
}

/// Classify a datagram by its first bytes (cheap, before any parsing).
pub fn classify(b: &[u8]) -> DatagramKind {
    match b.first() {
        None => DatagramKind::Unknown,
        Some(0xFF) => DatagramKind::Query,
        Some(0x00) | Some(0x01) if b.len() >= 4 && b[1..4] == [0, 0, 0] => DatagramKind::LegacyV4,
        Some(&PT_HELLO) => DatagramKind::Hello,
        Some(&PT_CHALLENGE) => DatagramKind::Challenge,
        Some(&PT_AUTH) => DatagramKind::Auth,
        Some(&PT_PRE_REJECT) => DatagramKind::PreReject,
        Some(&PT_AUTH_REJECT) => DatagramKind::AuthReject,
        Some(&PT_DATA) => DatagramKind::Data,
        Some(&PT_RESET) => DatagramKind::Reset,
        Some(_) => DatagramKind::Unknown,
    }
}

#[cfg(test)]
mod mod_tests {
    use super::*;

    #[test]
    fn version_negotiation_table() {
        // (client range, server range, expected)
        let cases = [
            ((5, 5), (5, 5), Some(5)),
            ((5, 7), (5, 5), Some(5)),
            ((3, 6), (5, 9), Some(6)),
            ((5, 9), (3, 6), Some(6)),
            ((6, 9), (3, 5), None),
            ((3, 4), (5, 5), None),
            ((7, 5), (5, 9), None), // inverted range is malformed
            ((0, u16::MAX), (5, 5), Some(5)),
        ];
        for ((cmin, cmax), (smin, smax), want) in cases {
            assert_eq!(
                negotiate_version(cmin, cmax, smin, smax),
                want,
                "{cmin}..={cmax} vs {smin}..={smax}"
            );
        }
    }

    #[test]
    fn ip_key_groups_ipv6_by_64_and_unmaps_ipv4() {
        use std::net::IpAddr;
        let k = |s: &str| ip_key(s.parse::<IpAddr>().unwrap());
        assert_eq!(k("2001:db8:1:2:aaaa::1"), k("2001:db8:1:2:ffff:1:2:3"));
        assert_ne!(k("2001:db8:1:2::1"), k("2001:db8:1:3::1"));
        assert_eq!(k("2001:db8:1:2:aaaa::1"), "2001:db8:1:2::".parse::<IpAddr>().unwrap());
        assert_eq!(k("::ffff:203.0.113.7"), "203.0.113.7".parse::<IpAddr>().unwrap());
        assert_eq!(k("203.0.113.7"), "203.0.113.7".parse::<IpAddr>().unwrap());
        assert_ne!(k("203.0.113.7"), k("203.0.113.8"));
    }

    #[test]
    fn classify_keeps_namespaces_apart() {
        assert_eq!(classify(b"\xFFHSMPQ1\x00rest"), DatagramKind::Query);
        assert_eq!(classify(&[0, 0, 0, 0, 9]), DatagramKind::LegacyV4);
        assert_eq!(classify(&[1, 0, 0, 0, 9]), DatagramKind::LegacyV4);
        assert_eq!(classify(&[PT_HELLO]), DatagramKind::Hello);
        assert_eq!(classify(&[PT_DATA, 1]), DatagramKind::Data);
        assert_eq!(classify(&[]), DatagramKind::Unknown);
        assert_eq!(classify(&[0x42]), DatagramKind::Unknown);
    }
}
