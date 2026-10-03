//! hsmp-master — central server registry for the Half Sword multiplayer mod.
//!
//! A tiny HTTP service that running `hsmp-server` instances heartbeat into so
//! the in-game lobby's browser can list "what servers are up right now."
//!
//! Endpoints (all JSON):
//!   POST   /v1/register      — new server registration (returns secret + id)
//!   POST   /v1/heartbeat/{id} — keepalive + player-count/map update
//!   GET    /v1/servers       — list all live servers (JSON array)
//!   DELETE /v1/servers/{id}  — explicit deregistration
//!
//! Listing schema (additive; old readers keep working): every entry carries
//! `name, host, port, mode, map, players, max_players, proto_ver, proto_min,
//! proto_max, pwd_protected, version, content_hash, region, reachable, ping_ms,
//! last_seen_utc_ms, age_s`. `content_hash` is the 64-hex hash the server enforces
//! ("" = any). `ping_ms` is master→server, NOT client→server — the in-game
//! browser measures its own RTT with the UDP query (query.rs).
//!
//! - In-memory `DashMap`, no DB. Capacity enforced; LRU-ish evict on insert.
//! - Heartbeat / delete are only accepted from the IP that registered the
//!   entry (server ids are public in the listing).
//! - One entry per host:port — a restarted server replaces its old entry.
//! - Reachability: on register the master sends a real browser query
//!   (query.rs) to host:port; `reachable` = it answered.
//! - Per-IP register limit (1/60 s); per-server heartbeat limit (1/2 s).
//! - Stale entries (no heartbeat for TTL) are hidden from the listing
//!   immediately and swept from memory every 10 s.
//! - `remote_addr` is authoritative for `host` — client-sent host is ignored.
//! - All strings are trimmed, stripped of control chars and length-capped.

mod ipkey; // per-IP limits key IPv6 by /64
mod proc_util; // --pid-file / --parent-pid (docs/development/testing.md)
mod query;

use ipkey::ip_key;

use anyhow::{Context, Result};
use axum::{
    body::Bytes,
    extract::{ConnectInfo, Path, State},
    http::StatusCode,
    response::{Html, IntoResponse},
    routing::{delete, get, post},
    Json, Router,
};
use clap::Parser;
use dashmap::DashMap;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    net::{IpAddr, SocketAddr},
    sync::Arc,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
use tokio::{net::UdpSocket, time};
use tower_http::cors::{Any, CorsLayer};
use tracing::{debug, info};
use uuid::Uuid;

const DEFAULT_BIND: &str = "0.0.0.0:7778";
const TTL_SECS: u64 = 30;
const SWEEP_INTERVAL_SECS: u64 = 10;
const REGISTER_RL_SECS: u64 = 60;
/// Min gap between two heartbeats of the same server.
const HEARTBEAT_MIN_GAP_MS: u64 = 2_000;
const MAX_SERVERS: usize = 10_000;
/// Registry entries per source IP (a LAN / host may run several servers).
/// With MAX_SERVERS this keeps one address from filling the list.
const MAX_SERVERS_PER_IP: usize = 16;
/// NAT rendezvous bounds: rooms, members per room, entries per
/// IP per room, POSTs per IP per window.
const RV_MAX_ROOMS: usize = 4096;
const RV_MAX_PER_ROOM: usize = 8;
const RV_PER_IP: usize = 2;
const RV_BURST: u32 = 4;
const RV_WINDOW_MS: u64 = 2_000;
const RV_TTL_SECS: u64 = 60;
const UDP_PING_TIMEOUT_MS: u64 = 500;

const MAX_NAME: usize = 48;
const MAX_MODE: usize = 32;
const MAX_MAP: usize = 64;
const MAX_REGION: usize = 16;
const MAX_VERSION: usize = 24;
const MAX_PLAYERS_CAP: u32 = 256;
/// Request-size and connection bounds. Every JSON body the
/// master accepts is a few hundred bytes; nonces are 16-32 hex chars and the
/// HMAC is exactly 64 hex chars. Without these a 2 MB nonce per heartbeat
/// (axum's default body limit) would be retained 32 times per entry.
const MAX_BODY_BYTES: usize = 4096;
const MAX_NONCE: usize = 64;
const HMAC_LEN: usize = 64;
const RECENT_NONCES: usize = 32;
/// A client must finish sending its request headers within this long.
const HEADER_READ_TIMEOUT: Duration = Duration::from_secs(if cfg!(test) { 1 } else { 5 });
/// Hard cap on one connection's lifetime (covers slow bodies and idle
/// keep-alive connections; requests are tiny and a reconnect is cheap).
const CONN_MAX_LIFETIME: Duration = Duration::from_secs(30);
/// Concurrent HTTP connections; further accepts wait.
const MAX_CONNS: usize = 1024;
/// Concurrent connections one source (`ip_key`) may hold (otherwise one
/// host can hold every slot with headers-but-no-body connections). Beyond it the
/// new connection is closed at once.
const MAX_CONNS_PER_IP: usize = 16;
/// A request body must arrive within this long.
const BODY_READ_TIMEOUT: Duration = Duration::from_secs(if cfg!(test) { 1 } else { 5 });

/// A nonce is 1..=MAX_NONCE printable ASCII bytes.
fn nonce_ok(n: &str) -> bool {
    !n.is_empty() && n.len() <= MAX_NONCE && n.bytes().all(|b| b.is_ascii_graphic())
}
fn hmac_ok(h: &str) -> bool {
    h.len() == HMAC_LEN && h.bytes().all(|b| b.is_ascii_hexdigit())
}

#[derive(Debug, Parser)]
#[command(author, version, about)]
struct Args {
    /// HTTP bind address.
    #[arg(long, default_value = DEFAULT_BIND)]
    bind: String,

    /// Write {"v":1,"role":"master","pid":..} here once the port is bound
    /// (atomic) and remove it on clean exit (docs/development/testing.md). The
    /// in-game HOST uses it to stop only the master it started.
    #[arg(long)]
    pid_file: Option<std::path::PathBuf>,

    /// Exit when this process (e.g. the hosting game) exits.
    #[arg(long)]
    parent_pid: Option<u32>,
}

// --- types -----------------------------------------------------------------

type ServerId = String; // uuid v4 as hex (no hyphens)

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ServerEntry {
    server_id: ServerId,
    name: String,
    host: String,
    port: u16,
    mode: String,
    /// Arena map name (e.g. "Map_Arena_Alley"). Joiners load this so both
    /// ends land in the same world. Empty string = host didn't advertise.
    #[serde(default)]
    map: String,
    players: u32,
    max_players: u32,
    proto_ver: u32,
    pwd_protected: bool,
    /// Lowest / highest wire protocol the server accepts; 0 = an older server (only
    /// `proto_ver` known).
    #[serde(default)]
    proto_min: u32,
    #[serde(default)]
    proto_max: u32,
    /// Server build version ("0.1.0"); empty for pre-browser servers.
    #[serde(default)]
    version: String,
    /// The content hash (64 lowercase hex) the server enforces in the handshake; "" =
    /// it accepts any mod files, or an older server.
    #[serde(default)]
    content_hash: String,
    /// Free-form region tag ("EU", "NA-East"); empty = unknown.
    #[serde(default)]
    region: String,
    ping_ms: u32,
    reachable: bool,
    last_seen_utc_ms: u64,
    /// Seconds since the last heartbeat, computed at list time.
    #[serde(default)]
    age_s: u64,
}

#[derive(Debug, Clone)]
struct ServerStored {
    entry: ServerEntry,
    /// IP that registered this entry; only it may heartbeat / delete.
    owner_ip: IpAddr,
    /// SHA-256 of the raw secret handed to the registrant.
    secret_hash: [u8; 32],
    /// Replay nonces we've already accepted for this server (short memory).
    recent_nonces: std::collections::VecDeque<String>,
    last_heartbeat: SystemTime,
    /// Monotonic time of the last accepted heartbeat request (rate limit).
    last_hb_req: Option<Instant>,
}

#[derive(Clone)]
struct AppState {
    servers: Arc<DashMap<ServerId, ServerStored>>,
    rate_register: Arc<DashMap<IpAddr, SystemTime>>,
    /// NAT rendezvous: room_id → list of (public_addr, posted_at).
    /// Two clients post with the same room_id and get each other's addrs.
    rendezvous: Arc<DashMap<String, Vec<(SocketAddr, SystemTime)>>>,
    rate_rendezvous: Arc<DashMap<IpAddr, (u32, SystemTime)>>,
}

impl AppState {
    fn new() -> Self {
        Self {
            servers: Arc::new(DashMap::new()),
            rate_register: Arc::new(DashMap::new()),
            rendezvous: Arc::new(DashMap::new()),
            rate_rendezvous: Arc::new(DashMap::new()),
        }
    }
}

/// One rendezvous POST (bounded): returns the other members'
/// addresses, or the HTTP status and text of a refusal. Every HTTP
/// connection has a new source port, so keying members by address alone would
/// let one client grow a room without limit: an IP holds at most RV_PER_IP
/// entries per room (its oldest is replaced; two players behind one NAT or
/// on one test box still fit), a room at most RV_MAX_PER_ROOM, the map at
/// most RV_MAX_ROOMS rooms, and an IP may POST RV_BURST times per RV_WINDOW.
fn rendezvous_join(st: &AppState, room_id: &str, remote: SocketAddr, now: SystemTime)
    -> Result<Vec<SocketAddr>, (StatusCode, &'static str)> {
    if room_id.len() > 64 || room_id.is_empty() {
        return Err((StatusCode::BAD_REQUEST, "bad room id"));
    }
    {
        let mut r = st.rate_rendezvous.entry(ip_key(remote.ip())).or_insert((0, now));
        if now.duration_since(r.1).unwrap_or(Duration::ZERO) >= Duration::from_millis(RV_WINDOW_MS) {
            *r = (0, now);
        }
        if r.0 >= RV_BURST {
            return Err((StatusCode::TOO_MANY_REQUESTS, "rendezvous throttled"));
        }
        r.0 += 1;
    }
    if !st.rendezvous.contains_key(room_id) && st.rendezvous.len() >= RV_MAX_ROOMS {
        return Err((StatusCode::SERVICE_UNAVAILABLE, "too many rendezvous rooms"));
    }
    let mut entry = st.rendezvous.entry(room_id.to_string()).or_default();
    // Prune stale entries and this exact address's previous post.
    let cutoff = now - Duration::from_secs(RV_TTL_SECS);
    entry.retain(|(a, t)| *t > cutoff && *a != remote);
    // At most RV_PER_IP entries per IP: the oldest of this IP makes room.
    let me = ip_key(remote.ip());
    while entry.iter().filter(|(a, _)| ip_key(a.ip()) == me).count() >= RV_PER_IP {
        let Some(i) = entry.iter().position(|(a, _)| ip_key(a.ip()) == me) else { break };
        entry.remove(i);
    }
    if entry.len() >= RV_MAX_PER_ROOM {
        return Err((StatusCode::CONFLICT, "rendezvous room full"));
    }
    let peers = entry.iter().map(|(a, _)| *a).collect();
    entry.push((remote, now));
    Ok(peers)
}

// --- request / response bodies --------------------------------------------

#[derive(Debug, Deserialize)]
struct RegisterReq {
    name: String,
    #[allow(dead_code)]
    host: Option<String>, // ignored — remote_addr is authoritative
    port: u16,
    #[serde(default)]
    mode: String,
    #[serde(default)]
    map: String,
    #[serde(default)]
    players: u32,
    #[serde(default = "default_max_players")]
    max_players: u32,
    #[serde(default)]
    proto_ver: u32,
    #[serde(default)]
    proto_min: u32,
    #[serde(default)]
    proto_max: u32,
    #[serde(default, alias = "password_required")]
    pwd_protected: bool,
    #[serde(default)]
    version: String,
    #[serde(default)]
    content_hash: String,
    #[serde(default)]
    region: String,
    /// 16 hex bytes.
    nonce: String,
    /// hmac for first-registration — we accept any value here; a fresh
    /// secret is minted and returned.
    #[serde(default)]
    #[allow(dead_code)]
    hmac: Option<String>,
}
fn default_max_players() -> u32 { 8 }

#[derive(Debug, Serialize)]
struct RegisterResp {
    server_id: ServerId,
    ttl_s: u64,
    /// 32 hex bytes. Client stores this in memory and HMACs every subsequent
    /// call. Master only stores the SHA-256 of this.
    secret: String,
}

#[derive(Debug, Deserialize)]
struct HeartbeatReq {
    #[serde(default)]
    players: u32,
    #[serde(default)]
    map: Option<String>,
    /// The server's live mode label (optional; older servers omit it).
    #[serde(default)]
    mode: Option<String>,
    nonce: String,
    hmac: String,
}

#[derive(Debug, Deserialize)]
struct DeleteReq {
    nonce: String,
    hmac: String,
}

// --- helpers ---------------------------------------------------------------

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

fn sha256_bytes(bytes: &[u8]) -> [u8; 32] {
    let d = Sha256::digest(bytes);
    let mut out = [0u8; 32];
    out.copy_from_slice(&d);
    out
}

fn gen_secret() -> String {
    use rand::RngCore;
    let mut buf = [0u8; 32];
    rand::thread_rng().fill_bytes(&mut buf);
    hex::encode(buf)
}

/// Trim, drop control characters (tabs/newlines would break line-based
/// clients), collapse to `max` bytes. Returns None if over `max` before
/// clipping (so the caller can 400 instead of silently truncating).
fn clean(s: &str, max: usize) -> Option<String> {
    let c: String = s.chars().filter(|c| !c.is_control()).collect();
    let c = c.trim().to_string();
    if c.len() > max { None } else { Some(c) }
}

/// "" or 64 hex chars (stored lower case); anything else is refused.
fn clean_content_hash(s: &str) -> Option<String> {
    let s = s.trim();
    if s.is_empty() {
        return Some(String::new());
    }
    (s.len() == 64 && s.bytes().all(|c| c.is_ascii_hexdigit())).then(|| s.to_ascii_lowercase())
}

/// The accepted protocol range: missing ends default to `proto_ver`; versions are u16.
fn proto_range(ver: u32, min: u32, max: u32) -> Option<(u32, u32)> {
    let min = if min == 0 { ver } else { min };
    let max = if max == 0 { ver } else { max };
    (min <= max && max <= u16::MAX as u32).then_some((min, max))
}

/// Real browser query against host:port. `Some(rtt_ms)` iff it answered.
async fn udp_reachability_ping(host: &str, port: u16) -> Option<u32> {
    let addr: SocketAddr = format!("{}:{}", host, port).parse().ok()?;
    let bind = if addr.is_ipv4() { "0.0.0.0:0" } else { "[::]:0" };
    let sock = UdpSocket::bind(bind).await.ok()?;
    sock.connect(addr).await.ok()?;
    let nonce: u64 = rand::random();
    let start = Instant::now();
    sock.send(&query::build_request(nonce)).await.ok()?;
    let mut buf = [0u8; 2048];
    let deadline = start + Duration::from_millis(UDP_PING_TIMEOUT_MS);
    loop {
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() {
            return None;
        }
        match time::timeout(left, sock.recv(&mut buf)).await {
            Ok(Ok(n)) => {
                if let Some((got, _)) = query::parse_reply(&buf[..n]) {
                    if got == nonce {
                        return Some(start.elapsed().as_millis() as u32);
                    }
                }
            }
            // ICMP port-unreachable surfaces as a recv error on Windows.
            Ok(Err(_)) | Err(_) => return None,
        }
    }
}

fn rate_limit_hit(map: &DashMap<IpAddr, SystemTime>, ip: IpAddr, window: Duration) -> bool {
    let now = SystemTime::now();
    if let Some(prev) = map.get(&ip) {
        if now.duration_since(*prev).unwrap_or(Duration::ZERO) < window {
            return true;
        }
    }
    map.insert(ip, now);
    false
}

/// Room for one more registration from `host`. A full registry
/// refuses new servers instead of evicting the oldest-heartbeat entry
/// (which let a fast-heartbeating attacker push real servers out), and one
/// address holds at most MAX_SERVERS_PER_IP entries.
fn registry_room(st: &AppState, ip: IpAddr) -> Result<(), (StatusCode, &'static str)> {
    let k = ip_key(ip);
    if st.servers.iter().filter(|r| ip_key(r.value().owner_ip) == k).count() >= MAX_SERVERS_PER_IP {
        return Err((StatusCode::TOO_MANY_REQUESTS, "too many servers from this address"));
    }
    if st.servers.len() >= MAX_SERVERS {
        let now = SystemTime::now();
        st.servers.retain(|_, s| !is_stale(s, now));
        if st.servers.len() >= MAX_SERVERS {
            return Err((StatusCode::SERVICE_UNAVAILABLE, "server list full"));
        }
    }
    Ok(())
}

fn is_stale(s: &ServerStored, now: SystemTime) -> bool {
    now.duration_since(s.last_heartbeat).unwrap_or(Duration::ZERO) > Duration::from_secs(TTL_SECS)
}

/// Live (non-stale) entries, newest-first, with `age_s` filled in.
fn live_entries(st: &AppState) -> Vec<ServerEntry> {
    let now = SystemTime::now();
    let mut out: Vec<ServerEntry> = st
        .servers
        .iter()
        .filter(|r| !is_stale(r.value(), now))
        .map(|r| {
            let mut e = r.value().entry.clone();
            e.age_s = now.duration_since(r.value().last_heartbeat).unwrap_or(Duration::ZERO).as_secs();
            e
        })
        .collect();
    out.sort_by(|a, b| b.last_seen_utc_ms.cmp(&a.last_seen_utc_ms).then(a.server_id.cmp(&b.server_id)));
    out
}

// --- handlers --------------------------------------------------------------

async fn health() -> &'static str {
    "ok"
}

/// STUN-lite: tells the caller what the server sees as their public address.
async fn myaddr(ConnectInfo(remote): ConnectInfo<SocketAddr>) -> Json<serde_json::Value> {
    Json(serde_json::json!({
        "ip": remote.ip().to_string(),
        "port": remote.port(),
        "addr": remote.to_string(),
    }))
}

/// NAT rendezvous. Two clients wanting to connect P2P both POST to
/// `/v1/rendezvous/<room_id>` from their actual game-port socket (so the
/// master sees the exact public (ip, port) they want to receive on). The
/// POST returns the OTHER party's addrs. Once two members are in a room,
/// subsequent requests get the full peer list.
async fn rendezvous_post(
    State(st): State<AppState>,
    Path(room_id): Path<String>,
    ConnectInfo(remote): ConnectInfo<SocketAddr>,
) -> impl IntoResponse {
    let peers = match rendezvous_join(&st, &room_id, remote, SystemTime::now()) {
        Ok(p) => p,
        Err((code, why)) => return (code, why).into_response(),
    };
    let peers_s: Vec<String> = peers.iter().map(|a| a.to_string()).collect();
    Json(serde_json::json!({
        "you": remote.to_string(),
        "peers": peers_s,
    })).into_response()
}

async fn dashboard(State(st): State<AppState>) -> Html<String> {
    let servers = live_entries(&st);

    let rows: String = if servers.is_empty() {
        r#"<tr><td colspan="9" style="text-align:center;color:#888;padding:24px">no servers registered</td></tr>"#.into()
    } else {
        servers.iter().map(|s| {
            let reach = if s.reachable { "✓" } else { "·" };
            let lock = if s.pwd_protected { "🔒" } else { "" };
            format!(
                r#"<tr><td>{reach}{lock}</td><td><b>{}</b></td><td>{}</td><td>{}</td><td><code>{}:{}</code></td><td>{}/{}</td><td>{}</td><td>{}</td><td>{}s</td></tr>"#,
                html_escape(&s.name),
                html_escape(&s.mode),
                html_escape(&s.map),
                html_escape(&s.host),
                s.port,
                s.players,
                s.max_players,
                html_escape(&s.region),
                html_escape(&s.version),
                s.age_s,
            )
        }).collect::<Vec<_>>().join("\n")
    };

    let count = servers.len();
    let body = format!(
        r##"<!doctype html>
<html lang="en"><head><meta charset="utf-8"><title>HSMP master — dashboard</title>
<meta http-equiv="refresh" content="5">
<style>
  :root {{ color-scheme: dark; }}
  body {{ font-family: ui-sans-serif, system-ui, -apple-system, sans-serif;
         background:#111;color:#ddd;margin:0;padding:24px;}}
  h1 {{ margin:0 0 4px; font-size:20px; }}
  .sub {{ color:#888; font-size:12px; margin-bottom:16px; }}
  table {{ border-collapse:collapse; width:100%; max-width:1100px; }}
  th, td {{ text-align:left; padding:8px 12px; border-bottom:1px solid #222; font-size:14px; }}
  th {{ color:#888; font-weight:500; font-size:11px; text-transform:uppercase; letter-spacing:.05em;}}
  code {{ color:#8fd; }}
  tr:hover td {{ background:#181818; }}
</style></head>
<body>
<h1>Half Sword Multiplayer — master</h1>
<div class="sub">{count} server(s) registered · auto-refresh 5 s</div>
<table>
  <thead><tr><th>✓</th><th>name</th><th>mode</th><th>map</th><th>host</th><th>players</th><th>region</th><th>version</th><th>age</th></tr></thead>
  <tbody>
{rows}
  </tbody>
</table>
<p style="margin-top:24px;color:#555;font-size:11px">
  <a href="/v1/servers" style="color:#8af">JSON</a> ·
  <a href="/v1/health" style="color:#8af">health</a>
</p>
</body></html>
"##
    );
    Html(body)
}

fn html_escape(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

async fn list_servers(State(st): State<AppState>) -> Json<Vec<ServerEntry>> {
    Json(live_entries(&st))
}

async fn register(
    State(st): State<AppState>,
    ConnectInfo(remote): ConnectInfo<SocketAddr>,
    Json(req): Json<RegisterReq>,
) -> impl IntoResponse {
    let ip = remote.ip();

    if rate_limit_hit(&st.rate_register, ip_key(ip), Duration::from_secs(REGISTER_RL_SECS)) {
        return (StatusCode::TOO_MANY_REQUESTS, "register throttled").into_response();
    }
    if !nonce_ok(&req.nonce) || req.port == 0 {
        return (StatusCode::BAD_REQUEST, "name/port/nonce required").into_response();
    }
    let Some(name) = clean(&req.name, MAX_NAME).filter(|n| !n.is_empty()) else {
        return (StatusCode::BAD_REQUEST, "name missing or too long").into_response();
    };
    let Some(mode) = clean(&req.mode, MAX_MODE) else {
        return (StatusCode::BAD_REQUEST, "mode too long").into_response();
    };
    let Some(map) = clean(&req.map, MAX_MAP) else {
        return (StatusCode::BAD_REQUEST, "map too long").into_response();
    };
    let Some(region) = clean(&req.region, MAX_REGION) else {
        return (StatusCode::BAD_REQUEST, "region too long").into_response();
    };
    let Some(version) = clean(&req.version, MAX_VERSION) else {
        return (StatusCode::BAD_REQUEST, "version too long").into_response();
    };
    let Some(content_hash) = clean_content_hash(&req.content_hash) else {
        return (StatusCode::BAD_REQUEST, "content_hash must be 64 hex chars or empty").into_response();
    };
    let Some((proto_min, proto_max)) = proto_range(req.proto_ver, req.proto_min, req.proto_max) else {
        return (StatusCode::BAD_REQUEST, "bad protocol range").into_response();
    };

    // Authoritative host = remote IP. Never trust client-sent host.
    let host = ip.to_string();

    // One entry per host:port: a restarted server replaces its old entry
    // instead of showing up twice until the old one expires.
    let dupes: Vec<String> = st
        .servers
        .iter()
        .filter(|r| r.value().entry.host == host && r.value().entry.port == req.port)
        .map(|r| r.key().clone())
        .collect();
    for d in &dupes {
        st.servers.remove(d);
    }
    if let Err((code, why)) = registry_room(&st, ip) {
        return (code, why).into_response();
    }

    let server_id = Uuid::new_v4().simple().to_string();
    let secret = gen_secret();
    let secret_hash = sha256_bytes(secret.as_bytes());

    // Real browser query (bounded to 500 ms). Pre-query servers simply show
    // reachable=false; they are still listed.
    let (ping_ms, reachable) = match udp_reachability_ping(&host, req.port).await {
        Some(ms) => (ms, true),
        None => (0, false),
    };

    let max_players = req.max_players.clamp(1, MAX_PLAYERS_CAP);
    let entry = ServerEntry {
        server_id: server_id.clone(),
        name,
        host,
        port: req.port,
        mode,
        map,
        players: req.players.min(max_players),
        max_players,
        proto_ver: req.proto_ver,
        proto_min,
        proto_max,
        pwd_protected: req.pwd_protected,
        version,
        content_hash,
        region,
        ping_ms,
        reachable,
        last_seen_utc_ms: now_ms(),
        age_s: 0,
    };

    let mut nonces = std::collections::VecDeque::with_capacity(8);
    nonces.push_back(req.nonce);
    st.servers.insert(
        server_id.clone(),
        ServerStored {
            entry,
            owner_ip: ip,
            secret_hash,
            recent_nonces: nonces,
            last_heartbeat: SystemTime::now(),
            last_hb_req: None,
        },
    );

    info!(server_id = %server_id, ip = %ip, port = req.port, reachable, replaced = dupes.len(), "server registered");

    (
        StatusCode::CREATED,
        Json(RegisterResp {
            server_id,
            ttl_s: TTL_SECS,
            secret,
        }),
    )
        .into_response()
}

async fn heartbeat(
    State(st): State<AppState>,
    ConnectInfo(remote): ConnectInfo<SocketAddr>,
    Path(server_id): Path<String>,
    Json(req): Json<HeartbeatReq>,
) -> impl IntoResponse {
    let mut row = match st.servers.get_mut(&server_id) {
        Some(r) => r,
        None => return (StatusCode::GONE, "unknown or expired").into_response(),
    };
    if is_stale(&row, SystemTime::now()) {
        // Hidden from the listing already; make the server re-register so
        // its reachability is re-checked.
        drop(row);
        st.servers.remove(&server_id);
        return (StatusCode::GONE, "expired").into_response();
    }
    // Server ids are public in /v1/servers: only the registering IP may
    // keep an entry alive or change it.
    if row.owner_ip != remote.ip() {
        return (StatusCode::FORBIDDEN, "not the owner").into_response();
    }
    if let Some(prev) = row.last_hb_req {
        if prev.elapsed() < Duration::from_millis(HEARTBEAT_MIN_GAP_MS) {
            return (StatusCode::TOO_MANY_REQUESTS, "heartbeat throttled").into_response();
        }
    }

    // NOTE: the master does not keep the plaintext secret, so the HMAC is
    // shape-checked only; owner-IP binding + nonce replay guard + rate
    // limits are what stop third parties from spoofing heartbeats.
    // A nonce is ≤ MAX_NONCE bytes and the HMAC exactly 64 hex chars,
    // so the retained replay memory is ≤ RECENT_NONCES × MAX_NONCE per entry.
    if !nonce_ok(&req.nonce) {
        return (StatusCode::BAD_REQUEST, "nonce missing or too long").into_response();
    }
    if !hmac_ok(&req.hmac) {
        return (StatusCode::BAD_REQUEST, "bad hmac shape").into_response();
    }
    if row.recent_nonces.iter().any(|n| n == &req.nonce) {
        return (StatusCode::CONFLICT, "nonce replay").into_response();
    }
    let _ = &row.secret_hash;
    row.recent_nonces.push_back(req.nonce);
    while row.recent_nonces.len() > RECENT_NONCES {
        row.recent_nonces.pop_front();
    }

    row.entry.players = req.players.min(row.entry.max_players);
    if let Some(m) = req.map.as_deref().and_then(|m| clean(m, MAX_MAP)) {
        row.entry.map = m;
    }
    if let Some(m) = req.mode.as_deref().and_then(|m| clean(m, MAX_MODE)) {
        row.entry.mode = m;
    }
    row.entry.last_seen_utc_ms = now_ms();
    row.last_heartbeat = SystemTime::now();
    row.last_hb_req = Some(Instant::now());

    (StatusCode::NO_CONTENT, "").into_response()
}

async fn delete_server(
    State(st): State<AppState>,
    ConnectInfo(remote): ConnectInfo<SocketAddr>,
    Path(server_id): Path<String>,
    body: Bytes,
) -> impl IntoResponse {
    let req: DeleteReq = match serde_json::from_slice(&body) {
        Ok(r) => r,
        Err(_) => return (StatusCode::BAD_REQUEST, "bad body").into_response(),
    };
    if !hmac_ok(&req.hmac) || !nonce_ok(&req.nonce) {
        return (StatusCode::BAD_REQUEST, "nonce+hmac required").into_response();
    }
    let owner = st.servers.get(&server_id).map(|r| r.owner_ip);
    match owner {
        None => (StatusCode::NOT_FOUND, "unknown").into_response(),
        Some(ip) if ip != remote.ip() => (StatusCode::FORBIDDEN, "not the owner").into_response(),
        Some(_) => {
            st.servers.remove(&server_id);
            info!(server_id = %server_id, "server deregistered");
            (StatusCode::NO_CONTENT, "").into_response()
        }
    }
}

// --- sweeper ---------------------------------------------------------------

fn sweep_once(st: &AppState) -> usize {
    let now = SystemTime::now();
    let to_drop: Vec<String> = st
        .servers
        .iter()
        .filter(|r| is_stale(r.value(), now))
        .map(|r| r.key().clone())
        .collect();
    for id in &to_drop {
        st.servers.remove(id);
    }
    // Trim rate maps that are far stale.
    let cutoff = now - Duration::from_secs(600);
    st.rate_register.retain(|_, t| *t > cutoff);
    let rl_cutoff = now - Duration::from_secs(10);
    st.rate_rendezvous.retain(|_, (_, t)| *t > rl_cutoff);
    let rv_cutoff = now - Duration::from_secs(RV_TTL_SECS);
    st.rendezvous.retain(|_, v| {
        v.retain(|(_, t)| *t > rv_cutoff);
        !v.is_empty()
    });
    to_drop.len()
}

async fn sweeper(st: AppState) {
    let mut ticker = time::interval(Duration::from_secs(SWEEP_INTERVAL_SECS));
    loop {
        ticker.tick().await;
        let n = sweep_once(&st);
        if n > 0 {
            debug!(count = n, "swept expired servers");
        }
    }
}

// --- entrypoint ------------------------------------------------------------

fn app(state: AppState) -> Router {
    let cors = CorsLayer::new().allow_origin(Any).allow_methods(Any).allow_headers(Any);
    Router::new()
        .route("/", get(dashboard))
        .route("/v1/health", get(health))
        .route("/v1/register", post(register))
        .route("/v1/heartbeat/:id", post(heartbeat))
        .route("/v1/servers", get(list_servers))
        .route("/v1/servers/:id", delete(delete_server))
        .route("/dashboard", get(dashboard))
        // NAT traversal helpers.
        .route("/v1/myaddr", get(myaddr))
        .route("/v1/rendezvous/:room_id", post(rendezvous_post))
        // No request needs more than a few hundred bytes (413 beyond).
        .layer(axum::extract::DefaultBodyLimit::max(MAX_BODY_BYTES))
        // A body that trickles in is cut off (408), freeing the slot.
        .layer(tower_http::timeout::RequestBodyTimeoutLayer::new(BODY_READ_TIMEOUT))
        .layer(cors)
        .with_state(state)
}

/// One connection's share of its source's MAX_CONNS_PER_IP (released on drop).
struct IpSlot {
    map: Arc<std::sync::Mutex<std::collections::HashMap<std::net::IpAddr, usize>>>,
    ip: std::net::IpAddr,
}

impl IpSlot {
    fn take(map: &Arc<std::sync::Mutex<std::collections::HashMap<std::net::IpAddr, usize>>>, ip: std::net::IpAddr) -> Option<IpSlot> {
        let mut m = map.lock().unwrap_or_else(|e| e.into_inner());
        let n = m.entry(ip).or_insert(0);
        if *n >= MAX_CONNS_PER_IP { return None; }
        *n += 1;
        Some(IpSlot { map: map.clone(), ip })
    }
}

impl Drop for IpSlot {
    fn drop(&mut self) {
        let mut m = self.map.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(n) = m.get_mut(&self.ip) {
            *n -= 1;
            if *n == 0 { m.remove(&self.ip); }
        }
    }
}

/// Serve `router` on `listener` with connection bounds:
/// headers must arrive within HEADER_READ_TIMEOUT (slowloris), a connection
/// lives at most CONN_MAX_LIFETIME (slow bodies, idle keep-alives), and at
/// most MAX_CONNS connections are served at once. `axum::serve` sets none
/// of these.
async fn serve_bounded(listener: tokio::net::TcpListener, router: Router) -> std::io::Result<()> {
    use hyper_util::rt::{TokioIo, TokioTimer};
    let slots = Arc::new(tokio::sync::Semaphore::new(MAX_CONNS));
    let per_ip: Arc<std::sync::Mutex<std::collections::HashMap<std::net::IpAddr, usize>>> = Default::default();
    loop {
        let permit = slots.clone().acquire_owned().await.expect("semaphore never closed");
        let (stream, remote) = match listener.accept().await {
            Ok(x) => x,
            Err(e) => {
                // EMFILE / ECONNABORTED: back off briefly, keep serving.
                debug!(error = %e, "accept failed");
                time::sleep(Duration::from_millis(50)).await;
                continue;
            }
        };
        // Per-source connection cap (the permit is released by the drop).
        let Some(ip_slot) = IpSlot::take(&per_ip, ip_key(remote.ip())) else {
            debug!(%remote, "connection refused: too many from this source");
            drop(stream);
            continue;
        };
        let svc = router.clone().layer(axum::Extension(ConnectInfo(remote)));
        let hyper_svc = hyper_util::service::TowerToHyperService::new(svc);
        tokio::spawn(async move {
            let _permit = permit;
            let _ip_slot = ip_slot;
            let mut b = hyper::server::conn::http1::Builder::new();
            b.timer(TokioTimer::new()).header_read_timeout(HEADER_READ_TIMEOUT);
            let conn = b.serve_connection(TokioIo::new(stream), hyper_svc);
            let _ = time::timeout(CONN_MAX_LIFETIME, conn).await;
        });
    }
}

#[tokio::main(flavor = "multi_thread", worker_threads = 2)]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "hsmp_master=info".into()),
        )
        .init();

    let args = Args::parse();
    info!(bind = %args.bind, "hsmp-master starting");

    let state = AppState::new();
    tokio::spawn(sweeper(state.clone()));

    let addr: SocketAddr = args.bind.parse().context("parse --bind")?;
    let listener = tokio::net::TcpListener::bind(addr).await?;
    info!(local = %addr, "listening");
    // Only after a successful bind: a second master that loses the port race
    // must not overwrite (and then delete) the winner's pid file.
    let _pid_file = match &args.pid_file {
        Some(p) => match proc_util::write_pid_file(p, "master", args.parent_pid) {
            Ok(g) => Some(g),
            Err(e) => {
                tracing::warn!(error = %e, path = ?p, "cannot write --pid-file");
                None
            }
        },
        None => None,
    };
    let serve = serve_bounded(listener, app(state));
    tokio::select! {
        r = serve => r?,
        _ = proc_util::wait_parent_exit(args.parent_pid) => {
            info!(parent_pid = ?args.parent_pid, "parent process exited; shutting down");
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    async fn spawn_master() -> (String, AppState) {
        let st = AppState::new();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let router = app(st.clone());
        tokio::spawn(async move {
            serve_bounded(listener, router).await.unwrap();
        });
        (format!("http://{}", addr), st)
    }

    fn reg_body(name: &str, port: u16) -> serde_json::Value {
        json!({"name": name, "mode": "Free Fight", "host": "", "port": port,
               "map": "Map_Arena_Pit", "players": 1, "max_players": 4,
               "proto_ver": 3, "pwd_protected": true, "version": "0.1.0",
               "region": "EU", "nonce": "n1", "hmac": ""})
    }

    async fn list(c: &reqwest::Client, base: &str) -> Vec<serde_json::Value> {
        c.get(format!("{base}/v1/servers")).send().await.unwrap().json().await.unwrap()
    }

    #[tokio::test]
    async fn listing_carries_browser_fields() {
        let (base, _st) = spawn_master().await;
        let c = reqwest::Client::new();
        assert!(list(&c, &base).await.is_empty());
        let r = c.post(format!("{base}/v1/register")).json(&reg_body("Pit Fights", 17777)).send().await.unwrap();
        assert_eq!(r.status(), 201);
        let l = list(&c, &base).await;
        assert_eq!(l.len(), 1);
        let e = &l[0];
        assert_eq!(e["name"], "Pit Fights");
        assert_eq!(e["map"], "Map_Arena_Pit");
        assert_eq!(e["mode"], "Free Fight");
        assert_eq!(e["players"], 1);
        assert_eq!(e["max_players"], 4);
        assert_eq!(e["pwd_protected"], true);
        assert_eq!(e["version"], "0.1.0");
        assert_eq!(e["region"], "EU");
        assert_eq!(e["proto_ver"], 3);
        assert_eq!(e["host"], "127.0.0.1");
        // Nothing listens on 17777 → not reachable, but still listed.
        assert_eq!(e["reachable"], false);
    }

    #[tokio::test]
    async fn listing_carries_build_identity() {
        let (base, _st) = spawn_master().await;
        let c = reqwest::Client::new();
        let mut b = reg_body("Versioned", 17778);
        b["proto_ver"] = json!(6);
        b["proto_min"] = json!(6);
        b["proto_max"] = json!(7);
        b["content_hash"] = json!("AB".repeat(32));
        b["future_field"] = json!("ignored");
        assert_eq!(c.post(format!("{base}/v1/register")).json(&b).send().await.unwrap().status(), 201);
        let l = list(&c, &base).await;
        let e = l.iter().find(|e| e["name"] == "Versioned").unwrap();
        assert_eq!((e["proto_ver"].as_u64(), e["proto_min"].as_u64(), e["proto_max"].as_u64()), (Some(6), Some(6), Some(7)));
        assert_eq!(e["content_hash"], "ab".repeat(32));
        // An older server: the range defaults to proto_ver, no content hash.
        let mut old = reg_body("Old", 17779);
        old["nonce"] = json!("n2");
        let r = c.post(format!("{base}/v1/register")).json(&old).send().await.unwrap();
        assert!(r.status() == 201 || r.status() == 429, "{}", r.status());
        assert_eq!(clean_content_hash(""), Some(String::new()));
        assert_eq!(clean_content_hash("abc"), None);
        assert_eq!(clean_content_hash(&"zz".repeat(32)), None);
        assert_eq!(proto_range(5, 0, 0), Some((5, 5)));
        assert_eq!(proto_range(6, 7, 6), None);
        assert_eq!(proto_range(6, 6, 70_000), None);
        // A malformed hash is refused, not stored.
        let mut bad = reg_body("Bad", 17780);
        bad["content_hash"] = json!("not-a-hash");
        let st2 = spawn_master().await.0;
        assert_eq!(c.post(format!("{st2}/v1/register")).json(&bad).send().await.unwrap().status(), 400);
    }

    #[tokio::test]
    async fn legacy_register_body_still_accepted() {
        // Exactly what pre-browser servers / e2e-test.sh send.
        let (base, _st) = spawn_master().await;
        let c = reqwest::Client::new();
        let r = c.post(format!("{base}/v1/register"))
            .json(&json!({"name":"E2E Server","mode":"duel","host":"","port":7777,"players":1,
                          "max_players":4,"password_required":false,"nonce":"e2e","hmac":""}))
            .send().await.unwrap();
        assert_eq!(r.status(), 201);
        let l = list(&c, &base).await;
        assert_eq!(l[0]["version"], "");
        assert_eq!(l[0]["content_hash"], "");
        assert_eq!(l[0]["region"], "");
    }

    #[tokio::test]
    async fn heartbeat_updates_players_and_map_and_rejects_replay() {
        let (base, _st) = spawn_master().await;
        let c = reqwest::Client::new();
        let reg: serde_json::Value = c.post(format!("{base}/v1/register"))
            .json(&reg_body("HB", 17778)).send().await.unwrap().json().await.unwrap();
        let sid = reg["server_id"].as_str().unwrap().to_string();
        let hmac = "a".repeat(64);
        let r = c.post(format!("{base}/v1/heartbeat/{sid}"))
            .json(&json!({"players": 3, "map": "Map_Arena_Yard", "mode": "Best of 5", "nonce": "h1", "hmac": hmac}))
            .send().await.unwrap();
        assert_eq!(r.status(), 204);
        let l = list(&c, &base).await;
        assert_eq!(l[0]["players"], 3);
        assert_eq!(l[0]["map"], "Map_Arena_Yard");
        // The live mode label replaces the registered one
        assert_eq!(l[0]["mode"], "Best of 5");
        // Too soon (per-server limit) → 429.
        let r = c.post(format!("{base}/v1/heartbeat/{sid}"))
            .json(&json!({"players": 9, "nonce": "h2", "hmac": hmac})).send().await.unwrap();
        assert_eq!(r.status(), 429);
        // Unknown id → 410 so the server re-registers.
        let r = c.post(format!("{base}/v1/heartbeat/nope"))
            .json(&json!({"players": 1, "nonce": "h3", "hmac": hmac})).send().await.unwrap();
        assert_eq!(r.status(), 410);
    }

    #[tokio::test]
    async fn players_clamped_to_max() {
        let (base, _st) = spawn_master().await;
        let c = reqwest::Client::new();
        let mut b = reg_body("Clamp", 17779);
        b["players"] = json!(99);
        c.post(format!("{base}/v1/register")).json(&b).send().await.unwrap();
        assert_eq!(list(&c, &base).await[0]["players"], 4);
    }

    #[tokio::test]
    async fn malformed_and_oversized_registrations_rejected() {
        let (base, st) = spawn_master().await;
        let c = reqwest::Client::new();
        // Not JSON.
        let r = c.post(format!("{base}/v1/register")).header("content-type", "application/json")
            .body("{nope").send().await.unwrap();
        assert!(r.status().is_client_error());
        st.rate_register.clear();
        // Name too long.
        let mut b = reg_body(&"x".repeat(200), 17780);
        b["nonce"] = json!("n2");
        let r = c.post(format!("{base}/v1/register")).json(&b).send().await.unwrap();
        assert_eq!(r.status(), 400);
        st.rate_register.clear();
        // Control chars stripped; blank name rejected.
        let r = c.post(format!("{base}/v1/register")).json(&reg_body("\t\n", 17781)).send().await.unwrap();
        assert_eq!(r.status(), 400);
        st.rate_register.clear();
        let r = c.post(format!("{base}/v1/register")).json(&reg_body("Tab\tName\n", 17782)).send().await.unwrap();
        assert_eq!(r.status(), 201);
        assert_eq!(list(&c, &base).await[0]["name"], "TabName");
    }

    #[tokio::test]
    async fn same_host_port_replaces_old_entry() {
        let (base, st) = spawn_master().await;
        let c = reqwest::Client::new();
        c.post(format!("{base}/v1/register")).json(&reg_body("Old", 17783)).send().await.unwrap();
        st.rate_register.clear(); // simulate restart after the throttle window
        c.post(format!("{base}/v1/register")).json(&reg_body("New", 17783)).send().await.unwrap();
        let l = list(&c, &base).await;
        assert_eq!(l.len(), 1);
        assert_eq!(l[0]["name"], "New");
    }

    #[tokio::test]
    async fn stale_entries_hidden_and_swept() {
        let (base, st) = spawn_master().await;
        let c = reqwest::Client::new();
        let reg: serde_json::Value = c.post(format!("{base}/v1/register"))
            .json(&reg_body("Stale", 17784)).send().await.unwrap().json().await.unwrap();
        let sid = reg["server_id"].as_str().unwrap().to_string();
        // Age it past the TTL.
        st.servers.get_mut(&sid).unwrap().last_heartbeat =
            SystemTime::now() - Duration::from_secs(TTL_SECS + 1);
        assert!(list(&c, &base).await.is_empty(), "stale entry must be hidden immediately");
        assert_eq!(sweep_once(&st), 1);
        assert!(st.servers.is_empty());
    }

    #[tokio::test]
    async fn stale_heartbeat_forces_reregister() {
        let (base, st) = spawn_master().await;
        let c = reqwest::Client::new();
        let reg: serde_json::Value = c.post(format!("{base}/v1/register"))
            .json(&reg_body("Late", 17785)).send().await.unwrap().json().await.unwrap();
        let sid = reg["server_id"].as_str().unwrap().to_string();
        st.servers.get_mut(&sid).unwrap().last_heartbeat =
            SystemTime::now() - Duration::from_secs(TTL_SECS + 5);
        let r = c.post(format!("{base}/v1/heartbeat/{sid}"))
            .json(&json!({"players": 1, "nonce": "h1", "hmac": "b".repeat(64)})).send().await.unwrap();
        assert_eq!(r.status(), 410);
    }

    #[tokio::test]
    async fn heartbeat_and_delete_bound_to_owner_ip() {
        let (base, st) = spawn_master().await;
        let c = reqwest::Client::new();
        let reg: serde_json::Value = c.post(format!("{base}/v1/register"))
            .json(&reg_body("Owned", 17786)).send().await.unwrap().json().await.unwrap();
        let sid = reg["server_id"].as_str().unwrap().to_string();
        // Pretend someone else registered it.
        st.servers.get_mut(&sid).unwrap().owner_ip = "203.0.113.9".parse().unwrap();
        let r = c.post(format!("{base}/v1/heartbeat/{sid}"))
            .json(&json!({"players": 1, "nonce": "h1", "hmac": "c".repeat(64)})).send().await.unwrap();
        assert_eq!(r.status(), 403);
        let r = c.delete(format!("{base}/v1/servers/{sid}"))
            .json(&json!({"nonce": "d1", "hmac": "c".repeat(64)})).send().await.unwrap();
        assert_eq!(r.status(), 403);
        assert_eq!(list(&c, &base).await.len(), 1);
        // Owner can delete.
        st.servers.get_mut(&sid).unwrap().owner_ip = "127.0.0.1".parse().unwrap();
        let r = c.delete(format!("{base}/v1/servers/{sid}"))
            .json(&json!({"nonce": "d2", "hmac": "c".repeat(64)})).send().await.unwrap();
        assert_eq!(r.status(), 204);
        assert!(list(&c, &base).await.is_empty());
    }

    /// One client cannot grow a room (new source port per
    /// HTTP connection), rooms and members are capped, POSTs are throttled.
    #[test]
    fn rendezvous_is_bounded() {
        let st = AppState::new();
        let t0 = SystemTime::now();
        let a = |ip: u8, port: u16| SocketAddr::from(([198, 51, 100, ip], port));
        // The same IP from 100 source ports: at most RV_PER_IP entries.
        for port in 0..100u16 {
            st.rate_rendezvous.clear();
            let _ = rendezvous_join(&st, "room", a(1, 40_000 + port), t0);
        }
        assert_eq!(st.rendezvous.get("room").unwrap().len(), RV_PER_IP);
        let peers = rendezvous_join(&st, "room", a(2, 1), t0).unwrap();
        assert_eq!(peers, vec![a(1, 40_098), a(1, 40_099)], "the IP's latest addresses");
        // Two players behind one address (or one test box) see each other.
        st.rate_rendezvous.clear();
        let p = rendezvous_join(&st, "pair", a(9, 1), t0).unwrap();
        assert!(p.is_empty());
        assert_eq!(rendezvous_join(&st, "pair", a(9, 2), t0).unwrap(), vec![a(9, 1)]);
        // Throttled per IP.
        for k in 0..RV_BURST as u16 { let _ = rendezvous_join(&st, "x", a(2, 10 + k), t0); }
        assert_eq!(rendezvous_join(&st, "room", a(2, 2), t0).unwrap_err().0, StatusCode::TOO_MANY_REQUESTS);
        // At most RV_MAX_PER_ROOM members.
        for ip in 3..40u8 { let _ = rendezvous_join(&st, "room", a(ip, 1), t0); }
        assert_eq!(st.rendezvous.get("room").unwrap().len(), RV_MAX_PER_ROOM);
        // At most RV_MAX_ROOMS rooms.
        for k in 0..RV_MAX_ROOMS + 50 {
            st.rate_rendezvous.clear();
            let _ = rendezvous_join(&st, &format!("r{k}"), a(200, 1), t0);
        }
        assert!(st.rendezvous.len() <= RV_MAX_ROOMS);
        st.rate_rendezvous.clear();
        assert_eq!(rendezvous_join(&st, "brand-new", a(201, 1), t0).unwrap_err().0, StatusCode::SERVICE_UNAVAILABLE);
    }

    /// A full list refuses newcomers instead of evicting live
    /// servers, and one address cannot hold more than MAX_SERVERS_PER_IP.
    #[tokio::test]
    async fn registry_refuses_instead_of_evicting() {
        let (base, st) = spawn_master().await;
        let c = reqwest::Client::new();
        for k in 0..MAX_SERVERS_PER_IP as u16 + 2 {
            st.rate_register.clear();
            let r = c.post(format!("{base}/v1/register")).json(&reg_body("Many", 18_000 + k)).send().await.unwrap();
            if (k as usize) < MAX_SERVERS_PER_IP { assert_eq!(r.status(), 201); } else { assert_eq!(r.status(), 429); }
        }
        assert_eq!(list(&c, &base).await.len(), MAX_SERVERS_PER_IP);
        // Re-registering an existing host:port still works (replaces itself).
        st.rate_register.clear();
        let r = c.post(format!("{base}/v1/register")).json(&reg_body("Again", 18_000)).send().await.unwrap();
        assert_eq!(r.status(), 201);
        assert!(registry_room(&st, "203.0.113.1".parse().unwrap()).is_ok());
    }

    /// Nonces/hmacs are length-checked, bodies are capped at
    /// MAX_BODY_BYTES, and an entry's replay memory stays bounded.
    #[tokio::test]
    async fn heartbeat_inputs_and_bodies_are_bounded() {
        let (base, st) = spawn_master().await;
        let c = reqwest::Client::new();
        let reg: serde_json::Value = c.post(format!("{base}/v1/register"))
            .json(&reg_body("Bounded", 17790)).send().await.unwrap().json().await.unwrap();
        let sid = reg["server_id"].as_str().unwrap().to_string();
        let hmac = "d".repeat(64);
        // Nonce over MAX_NONCE (but body under the limit) → 400.
        let r = c.post(format!("{base}/v1/heartbeat/{sid}"))
            .json(&json!({"players": 1, "nonce": "n".repeat(MAX_NONCE + 1), "hmac": hmac}))
            .send().await.unwrap();
        assert_eq!(r.status(), 400);
        // Non-hex HMAC → 400.
        let r = c.post(format!("{base}/v1/heartbeat/{sid}"))
            .json(&json!({"players": 1, "nonce": "ok", "hmac": "z".repeat(64)}))
            .send().await.unwrap();
        assert_eq!(r.status(), 400);
        // 2 MB nonce: refused by the body limit before it is parsed (413, or
        // the connection is closed while the client is still uploading).
        let r = c.post(format!("{base}/v1/heartbeat/{sid}"))
            .json(&json!({"players": 1, "nonce": "n".repeat(2 << 20), "hmac": hmac}))
            .send().await;
        if let Ok(r) = r { assert_eq!(r.status(), 413); }
        assert!(st.servers.get(&sid).unwrap().recent_nonces.iter().all(|n| n.len() <= MAX_NONCE));
        let r = c.post(format!("{base}/v1/register"))
            .json(&json!({"name": "x".repeat(MAX_BODY_BYTES), "port": 1, "nonce": "a"}))
            .send().await.unwrap();
        assert_eq!(r.status(), 413);
        // Many accepted heartbeats: memory per entry ≤ RECENT_NONCES × MAX_NONCE.
        for k in 0..100 {
            st.servers.get_mut(&sid).unwrap().last_hb_req = None;
            let nonce = format!("{:0>64}", k);
            let r = c.post(format!("{base}/v1/heartbeat/{sid}"))
                .json(&json!({"players": 1, "nonce": nonce, "hmac": hmac})).send().await.unwrap();
            assert_eq!(r.status(), 204);
        }
        let row = st.servers.get(&sid).unwrap();
        assert_eq!(row.recent_nonces.len(), RECENT_NONCES);
        assert!(row.recent_nonces.iter().map(|n| n.len()).sum::<usize>() <= RECENT_NONCES * MAX_NONCE);
    }

    /// A client that never finishes its request headers is
    /// disconnected after HEADER_READ_TIMEOUT (slowloris).
    #[tokio::test]
    async fn slow_headers_are_cut_off() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let (base, _st) = spawn_master().await;
        let addr = base.trim_start_matches("http://").to_string();
        let mut s = tokio::net::TcpStream::connect(&addr).await.unwrap();
        s.write_all(b"GET /v1/health HTTP/1.1\r\nHost: x\r\n").await.unwrap();
        let t0 = Instant::now();
        let mut buf = [0u8; 256];
        let r = time::timeout(HEADER_READ_TIMEOUT * 4, s.read(&mut buf)).await;
        let n = r.expect("connection must be closed by the header timeout").unwrap_or(0);
        // hyper may answer 408 before closing; either way it ends.
        assert!(n == 0 || std::str::from_utf8(&buf[..n]).unwrap_or("").contains("408"), "{:?}", &buf[..n]);
        assert!(t0.elapsed() >= HEADER_READ_TIMEOUT / 2);
        // A normal request still works.
        let r = reqwest::get(format!("{base}/v1/health")).await.unwrap();
        assert_eq!(r.status(), 200);
    }

    /// Exploit regression: one host holding many connections that
    /// never finish their request cannot lock others out; a trickled body is
    /// cut off after BODY_READ_TIMEOUT.
    #[tokio::test]
    async fn one_host_cannot_hold_every_connection() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let (base, _st) = spawn_master().await;
        let addr = base.trim_start_matches("http://").to_string();
        // 40 connections from 127.0.0.1 that send headers and a partial body.
        let mut held = Vec::new();
        for _ in 0..40 {
            let mut s = tokio::net::TcpStream::connect(&addr).await.unwrap();
            let _ = s.write_all(b"POST /v1/register HTTP/1.1\r\nHost: x\r\nContent-Length: 100\r\nContent-Type: application/json\r\n\r\n{").await;
            held.push(s);
        }
        time::sleep(Duration::from_millis(100)).await;
        // Beyond the per-source cap the extra connections were closed at once.
        let mut closed = 0;
        for s in held.iter_mut() {
            let mut b = [0u8; 64];
            if matches!(time::timeout(Duration::from_millis(20), s.read(&mut b)).await, Ok(Ok(0)) | Ok(Err(_))) { closed += 1; }
        }
        assert!(closed >= 40 - MAX_CONNS_PER_IP, "{closed} closed");
        // The held ones are cut off by the body timeout, then a request works.
        time::sleep(BODY_READ_TIMEOUT * 2).await;
        let r = reqwest::get(format!("{base}/v1/health")).await.unwrap();
        assert_eq!(r.status(), 200);
    }

    #[test]
    fn ip_slots_are_capped_and_released() {
        let m: Arc<std::sync::Mutex<std::collections::HashMap<std::net::IpAddr, usize>>> = Default::default();
        let ip: std::net::IpAddr = "203.0.113.4".parse().unwrap();
        let held: Vec<IpSlot> = (0..MAX_CONNS_PER_IP).map(|_| IpSlot::take(&m, ip).unwrap()).collect();
        assert!(IpSlot::take(&m, ip).is_none());
        assert!(IpSlot::take(&m, "203.0.113.5".parse().unwrap()).is_some(), "per source");
        drop(held);
        assert!(IpSlot::take(&m, ip).is_some());
    }

    /// IPv6 sources are rate-limited per /64.
    #[test]
    fn rendezvous_ipv6_limited_per_slash_64() {
        let st = AppState::new();
        let t0 = SystemTime::now();
        let a = |h: u16| SocketAddr::new(IpAddr::V6(std::net::Ipv6Addr::new(0x2001, 0xdb8, 7, 7, h, h, h, h)), 4000);
        for h in 0..50u16 {
            st.rate_rendezvous.clear();
            let _ = rendezvous_join(&st, "v6", a(h + 1), t0);
        }
        assert_eq!(st.rendezvous.get("v6").unwrap().len(), RV_PER_IP, "one /64 = one host");
        for h in 0..RV_BURST as u16 { let _ = rendezvous_join(&st, &format!("t{h}"), a(100 + h), t0); }
        assert_eq!(rendezvous_join(&st, "t", a(999), t0).unwrap_err().0, StatusCode::TOO_MANY_REQUESTS);
    }

    #[tokio::test]
    async fn reachability_uses_browser_query() {
        // A fake hsmp-server that answers browser queries.
        let sock = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let port = sock.local_addr().unwrap().port();
        tokio::spawn(async move {
            let mut buf = [0u8; 2048];
            loop {
                let Ok((n, from)) = sock.recv_from(&mut buf).await else { return };
                if let Some(nonce) = query::parse_request(&buf[..n]) {
                    let info = query::QueryInfo { name: "Q".into(), ..Default::default() };
                    let _ = sock.send_to(&query::build_reply(nonce, &info), from).await;
                }
            }
        });
        assert!(udp_reachability_ping("127.0.0.1", port).await.is_some());
    }
}
