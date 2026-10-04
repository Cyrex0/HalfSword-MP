//! NAT traversal through the server list ("punch relay").
//!
//! A listed server that sits behind a NAT holds one WebSocket to the master
//! (`GET /v1/punch/listen/{id}?ts=<unix ms>`, signed with its listing key like every other
//! write). A joiner that cannot reach it learns its own public UDP endpoint with STUN (from
//! its game socket) and posts `POST /v1/punch` with that endpoint. The master checks the
//! request and forwards one [`PunchMsg`] down the host's WebSocket; the host's server then
//! sends a few 16-byte probes to the endpoint (hsmp_nat::probe), which opens the host's NAT
//! for the joiner's handshake.
//!
//! The master never lets this aim traffic at third parties: the endpoint's IP must be the
//! address the request came from (CF-Connecting-IP on the Worker), its port must be >= 1024,
//! and requests are rate-limited per requester, per listed server and globally. The host
//! only ever sends a handful of small probes per request.

use crate::bucket::Bucket;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::net::{IpAddr, SocketAddr};

/// `POST /v1/punch` body.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PunchReq {
    /// The listing's host and port, as `GET /v1/servers` shows them.
    pub host: String,
    pub port: u16,
    /// The joiner's public UDP endpoint as STUN saw it (`ip:port` / `[v6]:port`).
    pub endpoint: String,
    /// Echoed to the host (log correlation); hex, at most 32 chars.
    #[serde(default)]
    pub nonce: String,
}

/// `POST /v1/punch` 202 answer.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PunchResp {
    pub sent: bool,
}

/// One message from the master to a listening host (WebSocket text frame, JSON).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PunchMsg {
    /// "punch".
    pub t: String,
    /// The joiner's public endpoint.
    pub to: String,
    #[serde(default)]
    pub nonce: String,
}

/// The app-level keep-alive on the listen socket. The Worker answers it without waking the
/// Durable Object (`setWebSocketAutoResponse`).
pub const PING: &str = "ping";
pub const PONG: &str = "pong";
/// How often a host pings its listen socket (well inside Cloudflare's idle limit).
pub const PING_EVERY_S: u64 = 45;

/// Smallest joiner port a punch may target (no probes at well-known services).
pub const MIN_TARGET_PORT: u16 = 1024;
/// Listen timestamps must be this close to the master's clock.
pub const LISTEN_SKEW_MS: u64 = 5 * 60_000;

/// Rate limits (token buckets: burst, then one per `*_every_ms`).
#[derive(Debug, Clone)]
pub struct Limits {
    pub per_ip_burst: f64,
    pub per_ip_every_ms: u64,
    pub per_server_burst: f64,
    pub per_server_every_ms: u64,
    pub global_burst: f64,
    pub global_every_ms: u64,
}

impl Default for Limits {
    fn default() -> Limits {
        // A joiner sends at most a few requests per join attempt; a busy server sees a few
        // joiners a minute.
        Limits {
            per_ip_burst: 6.0,
            per_ip_every_ms: 10_000,
            per_server_burst: 12.0,
            per_server_every_ms: 2_000,
            global_burst: 60.0,
            global_every_ms: 100,
        }
    }
}

/// The punch rate limits (in memory only; losing them on a restart is harmless).
#[derive(Debug, Clone)]
pub struct Limiter {
    lim: Limits,
    by_ip: HashMap<IpAddr, Bucket>,
    by_server: HashMap<String, Bucket>,
    global: Bucket,
}

/// Bound for the maps (a flood of addresses resets them, never grows them).
const MAX_KEYS: usize = 20_000;

impl Limiter {
    pub fn new(lim: Limits, now: u64) -> Limiter {
        Limiter { global: Bucket::full(lim.global_burst, now), lim, by_ip: HashMap::new(), by_server: HashMap::new() }
    }

    /// One request from `ip` (IPv4 host or IPv6 /64).
    pub fn requester_ok(&mut self, now: u64, ip: IpAddr) -> bool {
        if self.by_ip.len() >= MAX_KEYS {
            self.by_ip.clear();
        }
        let l = &self.lim;
        self.by_ip.entry(crate::ip::host_key(ip)).or_insert_with(|| Bucket::full(l.per_ip_burst, now)).take(now, l.per_ip_burst, l.per_ip_every_ms)
    }

    /// One punch sent to `server` (and to anyone: the global cap).
    pub fn server_ok(&mut self, now: u64, server: &str) -> bool {
        if self.by_server.len() >= MAX_KEYS {
            self.by_server.clear();
        }
        let l = &self.lim;
        let ok = self.by_server.entry(server.to_string()).or_insert_with(|| Bucket::full(l.per_server_burst, now)).take(now, l.per_server_burst, l.per_server_every_ms);
        ok && self.global.take(now, l.global_burst, l.global_every_ms)
    }

    /// Forget full buckets.
    pub fn sweep(&mut self, now: u64) {
        let l = &self.lim;
        self.by_ip.retain(|_, b| !b.is_full(now, l.per_ip_burst, l.per_ip_every_ms));
        self.by_server.retain(|_, b| !b.is_full(now, l.per_server_burst, l.per_server_every_ms));
    }
}

/// `ip:port` or `[v6]:port`, IPv4-mapped IPv6 folded to IPv4.
pub fn parse_endpoint(s: &str) -> Option<SocketAddr> {
    let a: SocketAddr = s.trim().parse().ok()?;
    Some(SocketAddr::new(crate::ip::canonical(a.ip()), a.port()))
}

/// Why a punch request is refused before any lookup.
pub fn check_request(req: &PunchReq, requester: IpAddr) -> Result<SocketAddr, (u16, &'static str)> {
    let ep = parse_endpoint(&req.endpoint).ok_or((400, "endpoint must be ip:port"))?;
    if ep.port() < MIN_TARGET_PORT {
        return Err((400, "endpoint port below 1024"));
    }
    if ep.ip() != crate::ip::canonical(requester) {
        return Err((403, "endpoint must be the address this request comes from"));
    }
    if req.port == 0 || req.host.trim().is_empty() || req.host.len() > 64 {
        return Err((400, "host and port of the listed server required"));
    }
    if req.nonce.len() > 32 || !req.nonce.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err((400, "nonce must be at most 32 hex chars"));
    }
    Ok(ep)
}

/// The path a host signs to open its listen socket.
pub fn listen_path(server_id: &str, ts: u64) -> String {
    format!("/v1/punch/listen/{server_id}?ts={ts}")
}

/// `(server_id, ts)` of a listen path (`/v1/punch/listen/<id>?ts=<ms>`).
pub fn parse_listen_path(path_and_query: &str) -> Option<(String, u64)> {
    let rest = path_and_query.strip_prefix("/v1/punch/listen/")?;
    let (id, q) = rest.split_once('?')?;
    if id.is_empty() || id.len() > 64 || !id.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    let ts = q.split('&').find_map(|kv| kv.strip_prefix("ts="))?.parse().ok()?;
    Some((id.to_string(), ts))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn req(ep: &str) -> PunchReq {
        PunchReq { host: "203.0.113.5".into(), port: 7777, endpoint: ep.into(), nonce: "ab12".into() }
    }

    #[test]
    fn the_endpoint_must_be_the_requester() {
        let me: IpAddr = "198.51.100.20".parse().unwrap();
        assert_eq!(check_request(&req("198.51.100.20:40000"), me), Ok("198.51.100.20:40000".parse().unwrap()));
        // another address: refused (no aiming probes at third parties)
        assert_eq!(check_request(&req("192.0.2.1:40000"), me).unwrap_err().0, 403);
        // IPv4-mapped forms on either side are the same host
        assert!(check_request(&req("[::ffff:198.51.100.20]:40000"), me).is_ok());
        assert!(check_request(&req("198.51.100.20:40000"), "::ffff:198.51.100.20".parse().unwrap()).is_ok());
        // well-known ports, garbage, missing fields
        assert_eq!(check_request(&req("198.51.100.20:53"), me).unwrap_err().0, 400);
        assert_eq!(check_request(&req("nope"), me).unwrap_err().0, 400);
        let mut r = req("198.51.100.20:40000");
        r.port = 0;
        assert_eq!(check_request(&r, me).unwrap_err().0, 400);
        r = req("198.51.100.20:40000");
        r.nonce = "zz".into();
        assert_eq!(check_request(&r, me).unwrap_err().0, 400);
    }

    #[test]
    fn listen_paths() {
        let p = listen_path("00ff", 1_800_000_000_000);
        assert_eq!(p, "/v1/punch/listen/00ff?ts=1800000000000");
        assert_eq!(parse_listen_path(&p), Some(("00ff".into(), 1_800_000_000_000)));
        assert_eq!(parse_listen_path("/v1/punch/listen/00ff"), None);
        assert_eq!(parse_listen_path("/v1/punch/listen/../x?ts=1"), None);
        assert_eq!(parse_listen_path("/v1/punch/listen/ab?ts=x"), None);
    }

    #[test]
    fn message_shape_is_fixed() {
        let m = PunchMsg { t: "punch".into(), to: "198.51.100.20:40000".into(), nonce: "ab".into() };
        assert_eq!(serde_json::to_string(&m).unwrap(), r#"{"t":"punch","to":"198.51.100.20:40000","nonce":"ab"}"#);
    }
}
