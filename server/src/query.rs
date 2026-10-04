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
    /// The first 16 hex chars of the content hash the server enforces (`content_tag`);
    /// "" = it accepts any content, or an older server.
    #[serde(rename = "h", default)]
    pub content_tag: String,
}

/// The content tag of a full content hash: its first 16 hex chars, lower case
/// ("" unless `hex` is 64 hex chars). Enough to tell builds apart in the browser; the
/// handshake checks the full hash.
pub fn content_tag(hex: &str) -> String {
    let h = hex.trim();
    if h.len() == 64 && h.bytes().all(|c| c.is_ascii_hexdigit()) { h[..16].to_ascii_lowercase() } else { String::new() }
}

/// A content tag as received (16 hex chars, lower-cased); anything else = "".
pub fn content_tag_of_tag(tag: &str) -> String {
    let t = tag.trim();
    if t.len() == 16 && t.bytes().all(|c| c.is_ascii_hexdigit()) { t.to_ascii_lowercase() } else { String::new() }
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
        info.content_tag = clip(&info.content_tag, 16);
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

/// Query replies, all sources together.
pub const GLOBAL_RATE: f64 = 200.0;
pub const GLOBAL_BURST: f64 = 400.0;
/// Query replies per source (IPv4 address, IPv6 /64). A browser refresh sends a few; a LAN
/// party behind one address browsing together stays well inside.
pub const PER_SOURCE_RATE: f64 = 20.0;
pub const PER_SOURCE_BURST: f64 = 40.0;
/// Sources tracked at once. A full table (all of them active) falls back to the global
/// budget alone for new sources.
pub const MAX_SOURCES: usize = 4096;

/// The server's reply budget: a per-source bucket, then the global one, so one host cannot
/// use up the global budget and blank every other browser's ping.
pub struct QueryLimiter {
    global: RateLimiter,
    by_src: std::collections::HashMap<std::net::IpAddr, (f64, Instant)>,
    pruned_at: Option<Instant>,
}

impl Default for QueryLimiter {
    fn default() -> Self {
        Self::new()
    }
}

impl QueryLimiter {
    pub fn new() -> Self {
        QueryLimiter { global: RateLimiter::new(GLOBAL_RATE, GLOBAL_BURST), by_src: Default::default(), pruned_at: None }
    }

    fn key(ip: std::net::IpAddr) -> std::net::IpAddr {
        match ip {
            std::net::IpAddr::V6(v) => match v.to_ipv4_mapped() {
                Some(v4) => v4.into(),
                None => {
                    let s = v.segments();
                    std::net::Ipv6Addr::new(s[0], s[1], s[2], s[3], 0, 0, 0, 0).into()
                }
            },
            v4 => v4,
        }
    }

    pub fn sources(&self) -> usize {
        self.by_src.len()
    }

    pub fn allow_at(&mut self, ip: std::net::IpAddr, now: Instant) -> bool {
        let k = Self::key(ip);
        let refill = |t: f64, at: Instant| (t + now.saturating_duration_since(at).as_secs_f64() * PER_SOURCE_RATE).min(PER_SOURCE_BURST);
        if !self.by_src.contains_key(&k) && self.by_src.len() >= MAX_SOURCES {
            if self.pruned_at.is_none_or(|p| now.saturating_duration_since(p).as_secs() >= 1) {
                self.pruned_at = Some(now);
                self.by_src.retain(|_, (t, at)| refill(*t, *at) < PER_SOURCE_BURST);
            }
            if self.by_src.len() >= MAX_SOURCES {
                return self.global.allow_at(now);
            }
        }
        let e = self.by_src.entry(k).or_insert((PER_SOURCE_BURST, now));
        e.0 = refill(e.0, e.1);
        e.1 = now;
        if e.0 < 1.0 || !self.global.allow_at(now) {
            return false;
        }
        e.0 -= 1.0;
        true
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
            content_tag: content_tag(&"Cd".repeat(32)),
        };
        assert_eq!(info.content_tag, "cdcdcdcdcdcdcdcd");
        assert_eq!(content_tag("abc"), "");
        let p = build_reply(77, &info);
        assert!(p.len() <= REQ_LEN);
        let (n, got) = parse_reply(&p).unwrap();
        assert_eq!(n, 77);
        assert_eq!(got.qver, QUERY_VERSION);
        assert_eq!(got.name, info.name);
        assert_eq!(got.players, 3);
        assert!(got.password);
        assert_eq!(got.content_tag, info.content_tag, "the tag survives a full-size reply");
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

    /// Seeded mutation of requests and replies: no panic, a request parses only when padded,
    /// and a reply built from any strings fits in the request.
    #[test]
    fn query_codec_survives_mutation() {
        let mut x: u64 = 0x9E37_79B9_7F4A_7C15;
        let mut rnd = move || { x ^= x << 13; x ^= x >> 7; x ^= x << 17; x };
        let info = QueryInfo { name: "n".into(), map: "Map_Arena_Pit".into(), server_key: "ab".repeat(32), ..Default::default() };
        let seeds = [build_request(7), build_reply(7, &info)];
        for i in 0..20_000 {
            let mut d = seeds[i % 2].clone();
            for _ in 0..1 + rnd() % 4 {
                let n = d.len().max(1);
                match rnd() % 4 {
                    0 if !d.is_empty() => { let j = (rnd() as usize) % n; d[j] ^= 1 << (rnd() % 8); }
                    1 => d.truncate((rnd() as usize) % n),
                    2 if !d.is_empty() => { let j = (rnd() as usize) % n; d[j] = b"{}\":,\\x\xff0"[(rnd() % 9) as usize]; }
                    _ => d.push(rnd() as u8),
                }
            }
            if parse_request(&d).is_some() {
                assert!(d.len() >= REQ_LEN);
            }
            if let Some((_, got)) = parse_reply(&d) {
                let mut s = |n: usize| (0..n).map(|_| char::from_u32(0x20 + (rnd() % 0x3000) as u32).unwrap_or('?')).collect::<String>();
                let q = QueryInfo { name: got.name + &s(200), map: s(300), mode: s(100), region: s(50), build: s(40), ..got };
                assert!(build_reply(1, &q).len() <= REQ_LEN, "reply larger than the request");
            }
        }
    }

    /// One source cannot use up the global reply budget: past its own burst it is refused
    /// while other sources still get answers.
    #[test]
    fn one_source_cannot_starve_the_others() {
        let t0 = Instant::now();
        let mut q = QueryLimiter::new();
        let flood: std::net::IpAddr = "198.51.100.7".parse().unwrap();
        let served = (0..1000).filter(|_| q.allow_at(flood, t0)).count();
        assert!(served <= PER_SOURCE_BURST as usize, "{served} replies to one source in one instant");
        let other: std::net::IpAddr = "203.0.113.9".parse().unwrap();
        assert!(q.allow_at(other, t0), "another source is still answered");
        assert!(q.allow_at(flood, t0 + Duration::from_secs(1)), "the flooder refills");
        // a flood of distinct sources is bounded by the global budget and the table size
        let n = (0..100_000u32).filter(|i| q.allow_at(std::net::IpAddr::from(i.to_be_bytes()), t0)).count();
        assert!(n <= GLOBAL_BURST as usize);
        assert!(q.sources() <= MAX_SOURCES);
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
