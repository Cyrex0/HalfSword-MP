//! Server-browser UDP query ("A2S_INFO"-style), shared by `hsmp-server`
//! (answers) and `hsmp-query` (asks).
//!
//! Lets the in-game browser measure a real RTT to every listed server and
//! read live info (players, map, ...) without joining. It lives on the game
//! UDP port, so the ping goes through exactly the NAT / firewall path a join
//! would.
//!
//! Wire (no bincode, so it can never be confused with a `Datagram`: bincode
//! starts a `Datagram` with a u32 variant index 0 or 1, these start 0xFF):
//!
//!   request  = MAGIC_REQ (8) | nonce u64 LE (8) | zero padding to REQ_LEN
//!   reply    = MAGIC_RSP (8) | nonce u64 LE (8) | compact JSON `QueryInfo`
//!
//! Anti-amplification: the request must be at least REQ_LEN bytes and the
//! reply is capped at REQ_LEN bytes, so a spoofed source gets <= 1x back.
//! The server also token-bucket limits replies (`RateLimiter`).

// Shared by three binaries; each uses a different subset.
#![allow(dead_code)]

use serde::{Deserialize, Serialize};
use std::time::Instant;

pub const MAGIC_REQ: &[u8; 8] = b"\xFFHSMPQ1\x00";
pub const MAGIC_RSP: &[u8; 8] = b"\xFFHSMPR1\x00";
/// Request size == max reply size (anti-amplification).
pub const REQ_LEN: usize = 320;
/// Version of the query wire itself (not the game protocol).
pub const QUERY_VERSION: u32 = 1;

/// Live info a server reports about itself. Short keys keep it small.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct QueryInfo {
    /// Query wire version.
    #[serde(rename = "q", default)]
    pub qver: u32,
    #[serde(rename = "n", default)]
    pub name: String,
    #[serde(rename = "m", default)]
    pub map: String,
    #[serde(rename = "g", default)]
    pub mode: String,
    #[serde(rename = "p", default)]
    pub players: u32,
    #[serde(rename = "x", default)]
    pub max_players: u32,
    #[serde(rename = "w", default)]
    pub password: bool,
    /// Game wire protocol version (proto::PROTOCOL_VERSION).
    #[serde(rename = "v", default)]
    pub proto_ver: u32,
    /// Server build version (CARGO_PKG_VERSION).
    #[serde(rename = "b", default)]
    pub build: String,
    #[serde(rename = "r", default)]
    pub region: String,
    /// Lowest / highest game protocol version the server accepts (v5
    /// version ranges; a browser marks a server incompatible when the
    /// ranges do not overlap). 0 = an older server that only sent `v`.
    #[serde(rename = "a", default)]
    pub proto_min: u32,
    #[serde(rename = "z", default)]
    pub proto_max: u32,
    /// Server identity: hex X25519 public key, pinned by the client
    /// (`hsmp-sidecar --server-key`).
    #[serde(rename = "k", default)]
    pub server_key: String,
}

pub fn build_request(nonce: u64) -> Vec<u8> {
    let mut v = vec![0u8; REQ_LEN];
    v[..8].copy_from_slice(MAGIC_REQ);
    v[8..16].copy_from_slice(&nonce.to_le_bytes());
    v
}

/// Cheap prefix test for the hot recv path (no allocation).
#[inline]
pub fn is_request(data: &[u8]) -> bool {
    data.len() >= 8 && &data[..8] == MAGIC_REQ
}

/// `Some(nonce)` for a well-formed, properly padded request.
pub fn parse_request(data: &[u8]) -> Option<u64> {
    if data.len() < REQ_LEN || !is_request(data) {
        return None;
    }
    Some(u64::from_le_bytes(data[8..16].try_into().ok()?))
}

/// Truncate to at most `max` bytes on a char boundary.
pub fn clip(s: &str, max: usize) -> String {
    if s.len() <= max {
        return s.to_string();
    }
    let mut end = max;
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    s[..end].to_string()
}

/// Encode a reply, never longer than REQ_LEN: long strings are clipped
/// until it fits.
pub fn build_reply(nonce: u64, info: &QueryInfo) -> Vec<u8> {
    let mut info = info.clone();
    info.qver = QUERY_VERSION;
    let mut caps = [48usize, 64, 24, 16];
    for _ in 0..8 {
        info.name = clip(&info.name, caps[0]);
        info.map = clip(&info.map, caps[1]);
        info.mode = clip(&info.mode, caps[2]);
        info.region = clip(&info.region, caps[3]);
        info.build = clip(&info.build, 16);
        let json = serde_json::to_vec(&info).unwrap_or_default();
        if 16 + json.len() <= REQ_LEN {
            let mut v = Vec::with_capacity(16 + json.len());
            v.extend_from_slice(MAGIC_RSP);
            v.extend_from_slice(&nonce.to_le_bytes());
            v.extend_from_slice(&json);
            return v;
        }
        for c in caps.iter_mut() {
            *c /= 2;
        }
    }
    // Unreachable in practice; minimal valid reply.
    let mut v = Vec::new();
    v.extend_from_slice(MAGIC_RSP);
    v.extend_from_slice(&nonce.to_le_bytes());
    v.extend_from_slice(b"{}");
    v
}

pub fn parse_reply(data: &[u8]) -> Option<(u64, QueryInfo)> {
    if data.len() < 16 || &data[..8] != MAGIC_RSP {
        return None;
    }
    let nonce = u64::from_le_bytes(data[8..16].try_into().ok()?);
    let info: QueryInfo = serde_json::from_slice(&data[16..]).ok()?;
    Some((nonce, info))
}

/// Simple token bucket (not thread-safe; wrap in a Mutex).
pub struct RateLimiter {
    rate_per_s: f64,
    burst: f64,
    tokens: f64,
    last: Instant,
}

impl RateLimiter {
    pub fn new(rate_per_s: f64, burst: f64) -> Self {
        Self { rate_per_s, burst, tokens: burst, last: Instant::now() }
    }
    pub fn allow_at(&mut self, now: Instant) -> bool {
        let dt = now.saturating_duration_since(self.last).as_secs_f64();
        self.last = now;
        self.tokens = (self.tokens + dt * self.rate_per_s).min(self.burst);
        if self.tokens >= 1.0 {
            self.tokens -= 1.0;
            true
        } else {
            false
        }
    }
    #[allow(dead_code)]
    pub fn allow(&mut self) -> bool {
        self.allow_at(Instant::now())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn request_roundtrip_and_padding() {
        let r = build_request(0xDEADBEEF_12345678);
        assert_eq!(r.len(), REQ_LEN);
        assert!(is_request(&r));
        assert_eq!(parse_request(&r), Some(0xDEADBEEF_12345678));
        // Unpadded request is refused (anti-amplification).
        assert_eq!(parse_request(&r[..64]), None);
        // Not a request.
        assert_eq!(parse_request(&vec![0u8; REQ_LEN]), None);
    }

    #[test]
    fn query_packets_never_look_like_datagrams() {
        // bincode Datagram starts with a u32 LE variant index (0 or 1).
        let r = build_request(1);
        let idx = u32::from_le_bytes(r[..4].try_into().unwrap());
        assert!(idx > 1);
        let p = build_reply(1, &QueryInfo::default());
        let idx = u32::from_le_bytes(p[..4].try_into().unwrap());
        assert!(idx > 1);
    }

    #[test]
    fn reply_roundtrip() {
        let info = QueryInfo {
            qver: 0,
            name: "Willie's Pit".into(),
            map: "Map_Arena_Pit".into(),
            mode: "Best of 3".into(),
            players: 3,
            max_players: 8,
            password: true,
            proto_ver: 3,
            build: "0.1.0".into(),
            region: "EU".into(),
            proto_min: 5,
            proto_max: 5,
            server_key: "ab".repeat(32),
        };
        let p = build_reply(77, &info);
        assert!(p.len() <= REQ_LEN);
        let (n, got) = parse_reply(&p).unwrap();
        assert_eq!(n, 77);
        assert_eq!(got.qver, QUERY_VERSION);
        assert_eq!(got.name, info.name);
        assert_eq!(got.players, 3);
        assert!(got.password);
    }

    #[test]
    fn reply_is_capped_even_with_huge_strings() {
        let big = "\u{00e9}".repeat(4000); // multibyte, tests char-boundary clip
        let info = QueryInfo {
            name: big.clone(), map: big.clone(), mode: big.clone(),
            region: big.clone(), build: big, ..Default::default()
        };
        let p = build_reply(5, &info);
        assert!(p.len() <= REQ_LEN, "reply {} > {}", p.len(), REQ_LEN);
        assert!(parse_reply(&p).is_some());
    }

    #[test]
    fn malformed_replies_rejected() {
        assert!(parse_reply(b"").is_none());
        assert!(parse_reply(b"\xFFHSMPR1\x00short").is_none());
        let mut p = build_reply(1, &QueryInfo::default());
        p.truncate(20); // cut JSON
        assert!(parse_reply(&p).is_none());
    }

    #[test]
    fn rate_limiter_bursts_then_refills() {
        let t0 = Instant::now();
        let mut rl = RateLimiter::new(10.0, 3.0);
        rl.last = t0;
        assert!(rl.allow_at(t0));
        assert!(rl.allow_at(t0));
        assert!(rl.allow_at(t0));
        assert!(!rl.allow_at(t0));
        assert!(rl.allow_at(t0 + Duration::from_millis(150)));
    }
}
