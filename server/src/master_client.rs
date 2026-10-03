//! Master-registry client for `hsmp-server`.
//!
//! When `HSMP_MASTER_URL` is set in the environment, the server registers itself and
//! heartbeats so it shows up in the in-game server browser's `GET /v1/servers` list. The
//! interval is the one the master asks for at registration (`heartbeat_s`; 10 s for an
//! hsmp-master that predates it, 120 s on the public Cloudflare list).
//!
//! Every request is signed with the listing key derived from the server identity
//! (hsmp_master_core::auth), so only this server can refresh or remove its listing. The legacy
//! `nonce` / `hmac` fields are still sent for older hsmp-master builds.
//!
//! A server bound to IPv4 registers over IPv4: the master lists the address the registration
//! came from, and an IPv6 listing would be unreachable for a server that only listens on
//! IPv4.
//!
//! Survives outages: a master that is down at startup, a transient HTTP failure, a 429 or
//! the master forgetting us (410 after an outage / master restart) never permanently delists
//! the server — we retry registration with capped exponential backoff, forever. On a clean
//! shutdown the listing is removed.

use crate::server::ServerState;
use crate::server_info;
use anyhow::Context;
use ed25519_dalek::SigningKey;
use hsmp_master_core::auth;
use hsmp_master_core::{DeleteReq, HeartbeatReq, RegisterReq, RegisterResp, SIG_HEADER};
use sha2::{Digest, Sha256};
use std::net::{IpAddr, SocketAddr};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::time;
use tracing::{info, warn};

/// What a master that does not send `heartbeat_s` expects (hsmp-master's TTL is 30 s).
const LEGACY_HEARTBEAT_SECS: u64 = 10;
const HTTP_TIMEOUT_SECS: u64 = 5;
/// Master's per-IP register throttle is 60 s; back off past it on 429.
const THROTTLED_RETRY_SECS: u64 = 61;
const MAX_BACKOFF_SECS: u64 = 60;

fn gen_nonce() -> String {
    use rand::RngCore;
    let mut b = [0u8; 16];
    rand::thread_rng().fill_bytes(&mut b);
    hex::encode(b)
}

/// The legacy HMAC field (hsmp-master checks only its shape).
fn hmac_sig(secret: &str, port: u16, name: &str, nonce: &str) -> String {
    let mut v = Vec::with_capacity(secret.len() + name.len() + nonce.len() + 8);
    v.extend_from_slice(secret.as_bytes());
    v.extend_from_slice(port.to_string().as_bytes());
    v.extend_from_slice(name.as_bytes());
    v.extend_from_slice(nonce.as_bytes());
    hex::encode(Sha256::digest(&v))
}

fn local_unix_ms() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
}

/// Offset (ms) from the local clock to an HTTP `Date` header (second precision).
fn clock_offset(date: &str, local_ms: u64) -> Option<i64> {
    let t = httpdate::parse_http_date(date.trim()).ok()?;
    let master = t.duration_since(UNIX_EPOCH).ok()?.as_millis() as i64;
    Some(master - local_ms as i64)
}

/// Next backoff delay: 5, 10, 20, 40, 60, 60, ...
fn next_backoff(prev: u64) -> u64 {
    if prev == 0 { 5 } else { (prev * 2).min(MAX_BACKOFF_SECS) }
}

/// The heartbeat interval a master asked for, bounded.
fn heartbeat_secs(asked: Option<u64>) -> u64 {
    asked.unwrap_or(LEGACY_HEARTBEAT_SECS).clamp(5, 600)
}

fn is_loopback_url(url: &str) -> bool {
    let rest = url.split_once("://").map(|(_, r)| r).unwrap_or(url);
    let auth = rest.split(['/', '?', '#']).next().unwrap_or("");
    let host = match auth.strip_prefix('[') {
        Some(v6) => v6.split(']').next().unwrap_or(""),
        None => auth.rsplit_once(':').map(|(h, _)| h).unwrap_or(auth),
    };
    host.eq_ignore_ascii_case("localhost") || host.parse::<IpAddr>().map(|ip| ip.is_loopback()).unwrap_or(false)
}

/// Resolves host names to their IPv4 addresses only.
struct Ipv4Only;

impl reqwest::dns::Resolve for Ipv4Only {
    fn resolve(&self, name: reqwest::dns::Name) -> reqwest::dns::Resolving {
        let host = name.as_str().to_string();
        Box::pin(async move {
            let addrs: Vec<SocketAddr> = tokio::net::lookup_host((host.as_str(), 0)).await?.filter(|a| a.is_ipv4()).collect();
            if addrs.is_empty() {
                return Err(format!("{host} has no IPv4 address").into());
            }
            Ok(Box::new(addrs.into_iter()) as reqwest::dns::Addrs)
        })
    }
}

enum RegisterOutcome {
    Ok { server_id: String, secret: String, heartbeat_s: u64 },
    Throttled,
    Failed(String),
}

enum Beat {
    Ok,
    /// The master no longer has this listing (or refused its timestamp): register again.
    Forgotten,
    Failed(String),
}

/// One registration with one master.
pub struct MasterClient {
    url: String,
    port: u16,
    key: SigningKey,
    http: reqwest::Client,
    /// Newest `ts` sent; every request uses a larger one even if the clock steps back.
    last_ts: Mutex<u64>,
    /// Added to the local clock for `ts`: learned from the master's `Date` header when it
    /// refused a registration for clock skew (a host clock minutes off would otherwise never
    /// be listed).
    clock_offset_ms: std::sync::atomic::AtomicI64,
    /// The listing this server holds right now (for the shutdown delete).
    current: Mutex<Option<String>>,
}

impl MasterClient {
    /// `bind` is the server's UDP bind address; `static_secret` its identity key.
    pub fn new(url: String, bind: SocketAddr, static_secret: &[u8; 32]) -> anyhow::Result<MasterClient> {
        let url = url.trim().trim_end_matches('/').to_string();
        if url.starts_with("http://") && !is_loopback_url(&url) {
            warn!(master = %url, "HSMP_MASTER_URL is plain http: registrations and the list travel unencrypted (use https)");
        }
        let mut b = reqwest::Client::builder().timeout(Duration::from_secs(HTTP_TIMEOUT_SECS));
        if is_loopback_url(&url) {
            b = b.no_proxy();
        }
        if bind.is_ipv4() {
            b = b.dns_resolver(Arc::new(Ipv4Only));
        }
        Ok(MasterClient {
            url,
            port: bind.port(),
            key: auth::listing_key(static_secret),
            http: b.build().context("reqwest client")?,
            last_ts: Mutex::new(0),
            clock_offset_ms: std::sync::atomic::AtomicI64::new(0),
            current: Mutex::new(None),
        })
    }

    fn next_ts(&self) -> u64 {
        let off = self.clock_offset_ms.load(std::sync::atomic::Ordering::Relaxed);
        let now = local_unix_ms().saturating_add_signed(off);
        let mut last = self.last_ts.lock().unwrap_or_else(|e| e.into_inner());
        *last = now.max(*last + 1);
        *last
    }

    async fn send(&self, method: reqwest::Method, path: &str, body: Vec<u8>) -> reqwest::Result<reqwest::Response> {
        let sig = auth::sign(&self.key, method.as_str(), path, &body);
        self.http
            .request(method, format!("{}{path}", self.url))
            .header("content-type", "application/json")
            .header(SIG_HEADER, sig)
            .body(body)
            .send()
            .await
    }

    async fn register_once(&self, state: &ServerState) -> RegisterOutcome {
        let a = server_info::advertised();
        let (players, map) = server_info::live(state);
        let nonce = gen_nonce();
        let req = RegisterReq {
            name: a.name.clone(),
            host: Some(String::new()),
            port: self.port,
            mode: server_info::live_mode(state),
            map,
            players,
            max_players: a.max_players,
            proto_ver: crate::proto::PROTOCOL_VERSION,
            proto_min: hsmp_net::net::VERSION_MIN as u32,
            proto_max: hsmp_net::net::VERSION_MAX as u32,
            server_key: a.server_key.clone(),
            pwd_protected: a.password,
            version: env!("CARGO_PKG_VERSION").to_string(),
            region: a.region.clone(),
            content_hash: a.content_hash.clone(),
            hmac: Some(hmac_sig("", self.port, &a.name, &nonce)),
            nonce,
            listing_key: Some(auth::public_hex(&self.key)),
            ts: Some(self.next_ts()),
        };
        let body = match serde_json::to_vec(&req) {
            Ok(b) => b,
            Err(e) => return RegisterOutcome::Failed(format!("encode: {e}")),
        };
        let resp = match self.send(reqwest::Method::POST, "/v1/register", body).await {
            Ok(r) => r,
            Err(e) => return RegisterOutcome::Failed(format!("send: {e}")),
        };
        if resp.status() == reqwest::StatusCode::TOO_MANY_REQUESTS {
            return RegisterOutcome::Throttled;
        }
        if !resp.status().is_success() {
            let st = resp.status();
            let date = resp.headers().get(reqwest::header::DATE).and_then(|v| v.to_str().ok()).map(str::to_string);
            let body = resp.text().await.unwrap_or_default();
            if st == reqwest::StatusCode::BAD_REQUEST && body.contains("clock skew") {
                if let Some(off) = date.as_deref().and_then(|d| clock_offset(d, local_unix_ms())) {
                    self.clock_offset_ms.store(off, std::sync::atomic::Ordering::Relaxed);
                    warn!(offset_s = off / 1000, "this computer's clock is off; registering with the master's time (sync the clock)");
                    return RegisterOutcome::Failed("clock skew; retrying with the master's time".into());
                }
            }
            return RegisterOutcome::Failed(format!("status {st}: {}", body.chars().take(120).collect::<String>()));
        }
        match resp.json::<RegisterResp>().await {
            Ok(b) => RegisterOutcome::Ok { server_id: b.server_id, secret: b.secret, heartbeat_s: heartbeat_secs(b.heartbeat_s) },
            Err(e) => RegisterOutcome::Failed(format!("parse: {e}")),
        }
    }

    async fn beat(&self, state: &ServerState, server_id: &str, secret: &str) -> Beat {
        let (players, map) = server_info::live(state);
        let mode = server_info::live_mode(state);
        let nonce = gen_nonce();
        let hb = HeartbeatReq {
            players,
            map: (!map.is_empty()).then_some(map),
            mode: (!mode.is_empty()).then_some(mode),
            hmac: hmac_sig(secret, self.port, &server_info::advertised().name, &nonce),
            nonce,
            ts: Some(self.next_ts()),
        };
        let body = serde_json::to_vec(&hb).unwrap_or_default();
        match self.send(reqwest::Method::POST, &format!("/v1/heartbeat/{server_id}"), body).await {
            Ok(r) if r.status().is_success() => Beat::Ok,
            Ok(r) if matches!(r.status().as_u16(), 404 | 409 | 410) => Beat::Forgotten,
            Ok(r) => Beat::Failed(format!("status {}", r.status())),
            Err(e) => Beat::Failed(e.to_string()),
        }
    }

    /// Heartbeat until the master forgets us (returns) — transient errors are logged and
    /// retried on the next tick.
    async fn heartbeat_loop(&self, state: &ServerState, server_id: &str, secret: &str, every_s: u64) {
        let mut ticker = time::interval(Duration::from_secs(every_s));
        ticker.set_missed_tick_behavior(time::MissedTickBehavior::Delay);
        ticker.tick().await; // skip the immediate first tick
        let mut fails = 0u32;
        loop {
            ticker.tick().await;
            match self.beat(state, server_id, secret).await {
                Beat::Ok => {
                    if fails > 0 {
                        info!(after = fails, "master heartbeat recovered");
                    }
                    fails = 0;
                }
                Beat::Forgotten => {
                    warn!("master forgot us (expired / restarted); re-registering");
                    return;
                }
                Beat::Failed(why) => {
                    fails += 1;
                    warn!(error = %why, fails, "heartbeat failed (master unreachable?)");
                }
            }
        }
    }

    /// Register + heartbeat forever.
    pub async fn run(self: Arc<Self>, state: Arc<ServerState>) {
        let mut backoff = 0u64;
        loop {
            match self.register_once(&state).await {
                RegisterOutcome::Ok { server_id, secret, heartbeat_s } => {
                    info!(server_id = %server_id, master = %self.url, heartbeat_s, "registered with master");
                    backoff = 0;
                    *self.current.lock().unwrap_or_else(|e| e.into_inner()) = Some(server_id.clone());
                    self.heartbeat_loop(&state, &server_id, &secret, heartbeat_s).await;
                    *self.current.lock().unwrap_or_else(|e| e.into_inner()) = None;
                }
                RegisterOutcome::Throttled => {
                    warn!(retry_s = THROTTLED_RETRY_SECS, "master register throttled (429)");
                    time::sleep(Duration::from_secs(THROTTLED_RETRY_SECS)).await;
                }
                RegisterOutcome::Failed(why) => {
                    backoff = next_backoff(backoff);
                    warn!(error = %why, retry_s = backoff, url = %self.url, "master register failed");
                    time::sleep(Duration::from_secs(backoff)).await;
                }
            }
        }
    }

    /// Remove our listing (clean shutdown). Best effort, at most `HTTP_TIMEOUT_SECS`.
    pub async fn deregister(&self) {
        let Some(id) = self.current.lock().unwrap_or_else(|e| e.into_inner()).take() else { return };
        let req = DeleteReq { nonce: gen_nonce(), hmac: "0".repeat(64), ts: Some(self.next_ts()) };
        let body = serde_json::to_vec(&req).unwrap_or_default();
        match self.send(reqwest::Method::DELETE, &format!("/v1/servers/{id}"), body).await {
            Ok(r) if r.status().is_success() => info!(server_id = %id, "removed from master"),
            Ok(r) => warn!(status = %r.status(), "master delete refused"),
            Err(e) => warn!(error = %e, "master delete failed"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backoff_is_capped() {
        let mut b = 0;
        let seq: Vec<u64> = (0..7).map(|_| { b = next_backoff(b); b }).collect();
        assert_eq!(seq, vec![5, 10, 20, 40, 60, 60, 60]);
    }

    #[test]
    fn heartbeat_interval_follows_the_master() {
        assert_eq!(heartbeat_secs(None), 10);
        assert_eq!(heartbeat_secs(Some(120)), 120);
        assert_eq!(heartbeat_secs(Some(0)), 5);
        assert_eq!(heartbeat_secs(Some(100_000)), 600);
    }

    #[test]
    fn loopback_urls() {
        assert!(is_loopback_url("http://127.0.0.1:7778"));
        assert!(is_loopback_url("http://localhost:8787/"));
        assert!(is_loopback_url("http://[::1]:7778"));
        assert!(!is_loopback_url("https://master.halfswordmp.workers.dev"));
    }

    /// A clock far off is corrected from the master's Date header.
    #[test]
    fn clock_offset_comes_from_the_date_header() {
        let local = 784_111_777_000u64; // Sun, 06 Nov 1994 08:49:37 GMT
        assert_eq!(clock_offset("Sun, 06 Nov 1994 08:49:37 GMT", local), Some(0));
        assert_eq!(clock_offset("Sun, 06 Nov 1994 09:49:37 GMT", local), Some(3_600_000));
        assert_eq!(clock_offset("Sun, 06 Nov 1994 08:29:37 GMT", local), Some(-1_200_000));
        assert_eq!(clock_offset("garbage", local), None);
        let c = MasterClient::new("https://m.example".into(), "0.0.0.0:7777".parse().unwrap(), &[3; 32]).unwrap();
        c.clock_offset_ms.store(-3_600_000, std::sync::atomic::Ordering::Relaxed);
        let ts = c.next_ts();
        assert!(ts.abs_diff(local_unix_ms() - 3_600_000) < 5_000, "{ts}");
    }

    #[test]
    fn timestamps_always_grow() {
        let c = MasterClient::new("https://m.example".into(), "0.0.0.0:7777".parse().unwrap(), &[3; 32]).unwrap();
        let a = c.next_ts();
        *c.last_ts.lock().unwrap() = a + 1_000_000; // clock stepped back
        assert!(c.next_ts() > a + 1_000_000);
    }

    /// The request a server sends is what the shared registry accepts (signature over the
    /// exact body, the key derived from the identity, the old fields still present).
    #[test]
    fn signed_register_is_accepted_by_the_registry() {
        let secret = [5u8; 32];
        let c = MasterClient::new("https://m.example".into(), "0.0.0.0:7777".parse().unwrap(), &secret).unwrap();
        let now = c.next_ts();
        let req = RegisterReq {
            name: "n".into(), port: 7777, mode: "Free Fight".into(), map: "Map_Arena_Pit".into(),
            players: 2, max_players: 8, proto_ver: 5, proto_min: 5, proto_max: 5, server_key: "ab".repeat(32),
            version: "0.1.0".into(), region: "EU".into(), content_hash: "ab".repeat(32), nonce: "x".into(), hmac: Some("y".into()),
            listing_key: Some(auth::public_hex(&c.key)), ts: Some(now), ..Default::default()
        };
        let body = serde_json::to_vec(&req).unwrap();
        let v: serde_json::Value = serde_json::from_slice(&body).unwrap();
        for k in ["name", "map", "mode", "players", "max_players", "pwd_protected", "region", "version", "proto_ver",
                  "proto_min", "proto_max", "server_key", "nonce", "hmac", "listing_key", "ts", "content_hash"] {
            assert!(v.get(k).is_some(), "missing {k}");
        }
        let sig = auth::sign(&c.key, "POST", "/v1/register", &body);
        let mut reg = hsmp_master_core::Registry::new(hsmp_master_core::Config::public(120, 360, false), vec![], vec![], now);
        let (r, _) = reg.register(now, "203.0.113.5".parse().unwrap(), &body, Some(&sig));
        assert_eq!(r.status, 201, "{}", r.body);
        assert_eq!(auth::public_hex(&c.key), auth::public_hex(&auth::listing_key(&secret)));
    }
}
