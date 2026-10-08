//! hsmp-query — server-browser helper spawned by the in-game HSMPMenu.
//!
//! UE4SS Lua cannot open sockets and must never block the game thread, so
//! the browser launches this process through the native module
//! (`IPC.spawn_capture`, an anonymous pipe; see
//! docs/development/ipc-shared-memory.md) and polls for its exit
//! (`IPC.capture_poll`). Nothing is written to disk: the result is printed to
//! stdout once, when the work is done.
//!
//!   1. GET <master>/v1/servers (4 s timeout). `--master` may be repeated
//!      (or comma-separated): primary first, then fallbacks, tried in order
//!      until one answers. Each entry is validated on its own — malformed
//!      entries are skipped and counted, never fatal.
//!      * With `--lan` (in parallel with 1): the browser query is broadcast to
//!        255.255.255.255 and sent to 127.0.0.1 on every `--lan-ports` port;
//!        every server that answers is listed with source `lan`. No master
//!        needed (servers already answer queries on their game port from any
//!        source, so the server side needs no change).
//!   2. Sends a UDP browser query (query.rs) to every server (+ any
//!      `--direct host:port`), 3 attempts, and records the best RTT and the
//!      server's live info (players / map / name / ...).
//!   3. Prints the result to stdout with `done 1` and exits 0.
//!
//! Output is tab-separated lines (trivial to parse from Lua):
//!
//!   HSMPQ  1
//!   gen    <echo of --gen>
//!   status <ok|master_unreachable|master_error|master_bad_data|no_master>  <message>
//!   counts <listed> <skipped>
//!   proto  <client PROTOCOL_VERSION>
//!   master <url> <index 1..n> <n>      (only when a master answered)
//!   lan    <0|1> <servers found>        (only with --lan)
//!   done   <0|1>
//!   S host port name map mode players max pwd proto version region ping_ms source live content proto_min proto_max nat punch mods mods_kb
//!
//! ping_ms: -1 = pending, -2 = no answer. live: 1 if the server answered
//! (fields then come from the server itself, fresher than the master).
//! source: master | direct | lan.
//! content: the first 16 hex chars of the content hash the server enforces ("" = any or
//! unknown); proto_min / proto_max: its protocol range (0 = unknown, use proto).
//! nat: how the listing says it is reachable (open, upnp, pcp, natpmp, double, cone, symmetric,
//! unknown; "" = not reported); punch: 1 if the list can relay a hole punch to it now.
//! mods / mods_kb: the server mods it serves (count, size in KiB; 0 = none or not reported;
//! docs/hosting/server-mods.md).

// Only needed for PROTOCOL_VERSION; kept out of this binary's test build so
// proto.rs's own tests (run by hsmp-server / hsmp-sidecar) aren't duplicated.
#[cfg(not(test))]
#[allow(dead_code)]
mod proto;
mod query;

#[cfg(not(test))]
fn client_proto() -> u32 { proto::PROTOCOL_VERSION }
#[cfg(test)]
fn client_proto() -> u32 { 0 }

use clap::Parser;
use std::collections::HashMap;
use std::net::{IpAddr, SocketAddr};
use std::time::{Duration, Instant};
use tokio::net::UdpSocket;

const MAX_SERVERS: usize = 500;
const ATTEMPTS: u32 = 3;
const ATTEMPT_GAP_MS: u64 = 400;

#[derive(Debug, Parser)]
#[command(author, version, about = "HSMP server-browser query helper")]
struct Args {
    /// Master registry base URL (http://host:port). Repeatable (or
    /// comma-separated): tried in order, first answer wins. Omit for
    /// direct/LAN-only.
    #[arg(long)]
    master: Vec<String>,
    /// Also discover servers on the LAN (UDP broadcast + 127.0.0.1).
    #[arg(long)]
    lan: bool,
    /// Game ports to probe for LAN discovery: "7777" or "7777-7786" (max 64).
    #[arg(long, default_value = "7777-7786")]
    lan_ports: String,
    /// Extra server to probe (host:port). Repeatable.
    #[arg(long)]
    direct: Vec<String>,
    /// Opaque token echoed back so the caller can drop stale results.
    #[arg(long, default_value = "")]
    gen: String,
    /// Total UDP ping budget, ms.
    #[arg(long, default_value_t = 2000)]
    timeout_ms: u64,
}

#[derive(Debug, Clone, Default)]
struct Row {
    host: String,
    port: u16,
    name: String,
    map: String,
    mode: String,
    players: u32,
    max: u32,
    pwd: bool,
    proto: u32,
    version: String,
    region: String,
    ping: i64,
    source: &'static str,
    live: bool,
    /// `query::content_tag` of the hash the server enforces; "" = any / unknown.
    content: String,
    /// Accepted protocol range; 0 = unknown (only `proto`).
    proto_min: u32,
    proto_max: u32,
    /// The listing's `nat` (open, upnp, cone, symmetric, ...; "" = not reported) and whether
    /// the list can relay a punch to it right now.
    nat: String,
    punch: bool,
    /// Server mods: count and size in KiB (0 = none / not reported).
    mods: u32,
    mods_kb: u64,
}

fn tsv(s: &str) -> String {
    let c: String = s.chars().map(|c| if c.is_control() { ' ' } else { c }).collect();
    query::clip(c.trim(), 96)
}

fn valid_host(h: &str) -> bool {
    !h.is_empty()
        && h.len() <= 253
        && h.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | ':' | '[' | ']'))
}

/// Split "host:port" / "[v6]:port".
fn split_host_port(s: &str) -> Option<(String, u16)> {
    let s = s.trim();
    let (h, p) = if let Some(rest) = s.strip_prefix('[') {
        let (h, p) = rest.split_once("]:")?;
        (h.to_string(), p)
    } else {
        let (h, p) = s.rsplit_once(':')?;
        (h.to_string(), p)
    };
    let port: u16 = p.parse().ok().filter(|p| *p != 0)?;
    valid_host(&h).then_some((h, port))
}

/// Validate one master entry. None = malformed (skipped, counted).
fn row_from_json(v: &serde_json::Value) -> Option<Row> {
    let o = v.as_object()?;
    let host = o.get("host")?.as_str()?.trim().to_string();
    if !valid_host(&host) {
        return None;
    }
    let port = o.get("port")?.as_u64().filter(|p| *p > 0 && *p <= 65535)? as u16;
    let s = |k: &str| o.get(k).and_then(|x| x.as_str()).map(tsv).unwrap_or_default();
    let n = |k: &str| o.get(k).and_then(|x| x.as_u64()).unwrap_or(0).min(100_000) as u32;
    let mut name = s("name");
    if name.is_empty() {
        name = format!("{host}:{port}");
    }
    let max = n("max_players");
    Some(Row {
        host,
        port,
        name,
        map: s("map"),
        mode: s("mode"),
        players: n("players").min(if max > 0 { max } else { u32::MAX }),
        max,
        pwd: o.get("pwd_protected").and_then(|x| x.as_bool()).unwrap_or(false),
        proto: n("proto_ver"),
        version: s("version"),
        region: s("region"),
        ping: -1,
        source: "master",
        live: false,
        content: query::content_tag(o.get("content_hash").and_then(|x| x.as_str()).unwrap_or("")),
        proto_min: n("proto_min").min(65_535),
        proto_max: n("proto_max").min(65_535),
        nat: s("nat"),
        punch: o.get("punch").and_then(|x| x.as_bool()).unwrap_or(false),
        mods: n("mods").min(hsmp_ipc::schema::mods::MAX_MODS as u32),
        mods_kb: o.get("mods_bytes").and_then(|x| x.as_u64()).unwrap_or(0).min(1 << 30).div_ceil(1024),
    })
}

/// Parse a master `/v1/servers` body. Err = not a JSON array at all.
fn parse_master_list(body: &str) -> Result<(Vec<Row>, usize), String> {
    let v: serde_json::Value = serde_json::from_str(body).map_err(|e| format!("bad JSON: {e}"))?;
    let arr = v.as_array().ok_or_else(|| "listing is not an array".to_string())?;
    let mut rows = Vec::new();
    let mut skipped = 0;
    for e in arr {
        match row_from_json(e) {
            Some(r) => rows.push(r),
            None => skipped += 1,
        }
    }
    Ok((rows, skipped))
}

struct Status {
    code: &'static str,
    msg: String,
    listed: usize,
    skipped: usize,
}

/// Which master answered (url, 1-based index, how many were configured) and
/// whether LAN discovery ran.
#[derive(Debug, Clone, Default)]
struct Meta {
    master: Option<(String, usize, usize)>,
    lan: bool,
}

fn render(gen: &str, st: &Status, meta: &Meta, rows: &[Row], done: bool) -> String {
    let mut out = String::with_capacity(256 + rows.len() * 160);
    out.push_str("HSMPQ\t1\n");
    out.push_str(&format!("gen\t{}\n", tsv(gen)));
    out.push_str(&format!("status\t{}\t{}\n", st.code, tsv(&st.msg)));
    out.push_str(&format!("counts\t{}\t{}\n", st.listed, st.skipped));
    out.push_str(&format!("proto\t{}\n", client_proto()));
    if let Some((url, i, n)) = &meta.master {
        out.push_str(&format!("master\t{}\t{}\t{}\n", tsv(url), i, n));
    }
    if meta.lan {
        let n = rows.iter().filter(|r| r.source == "lan").count();
        out.push_str(&format!("lan\t1\t{n}\n"));
    }
    out.push_str(&format!("done\t{}\n", if done { 1 } else { 0 }));
    for r in rows {
        out.push_str(&format!(
            "S\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\n",
            tsv(&r.host), r.port, tsv(&r.name), tsv(&r.map), tsv(&r.mode),
            r.players, r.max, r.pwd as u8, r.proto, tsv(&r.version), tsv(&r.region),
            r.ping, r.source, r.live as u8, r.content, r.proto_min, r.proto_max, tsv(&r.nat), r.punch as u8,
            r.mods, r.mods_kb,
        ));
    }
    out
}

/// `--master a --master b,c` -> [a, b, c] (trimmed, empty and duplicate entries dropped).
fn master_list(args: &[String]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for a in args {
        for u in a.split(',') {
            let u = u.trim().trim_end_matches('/').to_string();
            if !u.is_empty() && !out.contains(&u) && out.len() < 8 {
                out.push(u);
            }
        }
    }
    out
}

/// Try every master in order; the first that answers with a list wins. When
/// all fail, the status is the last failure and the message names every one.
async fn fetch_masters(
    urls: &[String],
    per_try: Duration,
) -> (Status, Vec<Row>, Option<(String, usize, usize)>) {
    let n = urls.len();
    let mut why: Vec<String> = Vec::new();
    let mut last_code = "master_unreachable";
    for (i, u) in urls.iter().enumerate() {
        let (st, rows) = fetch_master(u, per_try).await;
        if st.code == "ok" {
            return (st, rows, Some((u.clone(), i + 1, n)));
        }
        last_code = st.code;
        why.push(if n > 1 { st.msg.clone() } else { st.msg });
    }
    let msg = if n > 1 { format!("{} lists tried: {}", n, why.join("; ")) } else { why.join("; ") };
    (Status { code: last_code, msg, listed: 0, skipped: 0 }, vec![], None)
}

/// True for http(s)://localhost, 127.x.x.x or [::1] URLs.
fn is_loopback_url(url: &str) -> bool {
    let rest = url.split_once("://").map(|(_, r)| r).unwrap_or(url);
    let auth = rest.split(['/', '?', '#']).next().unwrap_or("");
    let auth = auth.rsplit_once('@').map(|(_, h)| h).unwrap_or(auth);
    let host = if let Some(v6) = auth.strip_prefix('[') { v6.split(']').next().unwrap_or("") } else { auth.rsplit_once(':').map(|(h, _)| h).unwrap_or(auth) };
    host.eq_ignore_ascii_case("localhost") || host.parse::<IpAddr>().map(|ip| ip.is_loopback()).unwrap_or(false)
}

async fn fetch_master(url: &str, total: Duration) -> (Status, Vec<Row>) {
    let base = url.trim().trim_end_matches('/');
    let mut builder = reqwest::Client::builder()
        .connect_timeout(total.min(Duration::from_secs(3)))
        .timeout(total);
    // A loopback master (tests, a local master) must never go through HTTP(S)_PROXY: on a
    // proxied box the proxy answers 502 and the browser reads it as master_error.
    if is_loopback_url(base) {
        builder = builder.no_proxy();
    }
    let client = match builder.build() {
        Ok(c) => c,
        Err(e) => return (Status { code: "master_unreachable", msg: e.to_string(), listed: 0, skipped: 0 }, vec![]),
    };
    let resp = match client.get(format!("{base}/v1/servers")).send().await {
        Ok(r) => r,
        Err(e) => {
            let why = if e.is_timeout() { "timed out".to_string() } else if e.is_connect() { "connection refused / no route".to_string() } else { e.to_string() };
            return (Status { code: "master_unreachable", msg: format!("{base}: {why}"), listed: 0, skipped: 0 }, vec![]);
        }
    };
    if !resp.status().is_success() {
        return (Status { code: "master_error", msg: format!("{base} answered HTTP {}", resp.status().as_u16()), listed: 0, skipped: 0 }, vec![]);
    }
    // Cap the body: a hostile/broken master must not exhaust memory.
    let body = match resp.bytes().await {
        Ok(b) if b.len() <= 4 << 20 => String::from_utf8_lossy(&b).into_owned(),
        Ok(_) => return (Status { code: "master_bad_data", msg: "listing too large".into(), listed: 0, skipped: 0 }, vec![]),
        Err(e) => return (Status { code: "master_unreachable", msg: format!("read: {e}"), listed: 0, skipped: 0 }, vec![]),
    };
    match parse_master_list(&body) {
        Ok((rows, skipped)) => {
            let n = rows.len();
            (Status { code: "ok", msg: String::new(), listed: n, skipped }, rows)
        }
        Err(e) => (Status { code: "master_bad_data", msg: e, listed: 0, skipped: 0 }, vec![]),
    }
}

async fn resolve(host: &str, port: u16) -> Option<SocketAddr> {
    if let Ok(ip) = host.trim_matches(|c| c == '[' || c == ']').parse::<IpAddr>() {
        return Some(SocketAddr::new(ip, port));
    }
    let fut = tokio::net::lookup_host((host.to_string(), port));
    match tokio::time::timeout(Duration::from_millis(1500), fut).await {
        Ok(Ok(it)) => {
            let all: Vec<SocketAddr> = it.collect();
            all.iter().find(|a| a.is_ipv4()).or(all.first()).copied()
        }
        _ => None,
    }
}

/// Query every row over UDP; fill ping + live info in place.
async fn ping_all(rows: &mut [Row], budget: Duration) {
    let t0 = Instant::now();
    let mut addrs: Vec<Option<SocketAddr>> = Vec::with_capacity(rows.len());
    for r in rows.iter() {
        addrs.push(resolve(&r.host, r.port).await);
    }
    let v4 = UdpSocket::bind("0.0.0.0:0").await.ok();
    let v6 = UdpSocket::bind("[::]:0").await.ok();
    let base: u64 = rand::random::<u64>() & !0xFFFF_FFFF;
    let mut sent: HashMap<u64, (usize, Instant)> = HashMap::new();
    let mut best: Vec<Option<(u64, query::QueryInfo)>> = vec![None; rows.len()];
    let mut buf = [0u8; 2048];
    let deadline = t0 + budget;

    for attempt in 0..ATTEMPTS {
        for (i, a) in addrs.iter().enumerate() {
            let Some(a) = a else { continue };
            if best[i].is_some() {
                continue;
            }
            let nonce = base | ((attempt as u64) << 24) | i as u64;
            let sock = if a.is_ipv4() { v4.as_ref() } else { v6.as_ref() };
            if let Some(s) = sock {
                if s.send_to(&query::build_request(nonce), a).await.is_ok() {
                    sent.insert(nonce, (i, Instant::now()));
                }
            }
        }
        let until = if attempt + 1 < ATTEMPTS {
            (Instant::now() + Duration::from_millis(ATTEMPT_GAP_MS)).min(deadline)
        } else {
            deadline
        };
        loop {
            if best.iter().zip(&addrs).all(|(b, a)| b.is_some() || a.is_none()) {
                break;
            }
            let left = until.saturating_duration_since(Instant::now());
            if left.is_zero() {
                break;
            }
            // v6 replies are rare; drain that socket opportunistically.
            if let Some(s) = &v6 {
                while let Ok((n, _)) = s.try_recv_from(&mut buf) {
                    handle_reply(&buf[..n], &sent, &mut best);
                }
            }
            let wait = left.min(Duration::from_millis(50));
            let got = match &v4 {
                Some(s) => tokio::time::timeout(wait, s.recv_from(&mut buf)).await,
                None => {
                    tokio::time::sleep(wait).await;
                    continue;
                }
            };
            // Err inside = e.g. ICMP port-unreachable reported on Windows.
            if let Ok(Ok((n, _))) = got {
                handle_reply(&buf[..n], &sent, &mut best);
            }
        }
        if Instant::now() >= deadline {
            break;
        }
    }

    for (i, r) in rows.iter_mut().enumerate() {
        match best[i].take() {
            Some((ms, info)) => apply_info(r, ms, &info),
            None => r.ping = -2,
        }
    }
}

/// Fill a row from a server's own query reply.
fn apply_info(r: &mut Row, ms: u64, info: &query::QueryInfo) {
    r.ping = ms as i64;
    r.live = true;
    if !info.name.trim().is_empty() { r.name = tsv(&info.name); }
    if !info.map.trim().is_empty() { r.map = tsv(&info.map); }
    if !info.mode.trim().is_empty() { r.mode = tsv(&info.mode); }
    if !info.build.trim().is_empty() { r.version = tsv(&info.build); }
    if !info.region.trim().is_empty() { r.region = tsv(&info.region); }
    r.max = info.max_players;
    r.players = info.players.min(info.max_players.max(info.players));
    r.pwd = info.password;
    r.proto = info.proto_ver;
    r.proto_min = info.proto_min;
    r.proto_max = info.proto_max;
    r.content = query::content_tag_of_tag(&info.content_tag);
    r.mods = info.mods.min(hsmp_ipc::schema::mods::MAX_MODS as u32);
    r.mods_kb = info.mods_kb.min(1 << 20);
}

/// "7777" / "7777-7786" -> ports (at most 64). None = malformed.
fn parse_port_range(s: &str) -> Option<Vec<u16>> {
    let s = s.trim();
    let (a, b) = match s.split_once('-') {
        Some((a, b)) => (a.trim().parse::<u16>().ok()?, b.trim().parse::<u16>().ok()?),
        None => {
            let p = s.parse::<u16>().ok()?;
            (p, p)
        }
    };
    if a == 0 || b < a || b - a > 63 {
        return None;
    }
    Some((a..=b).collect())
}

/// LAN discovery: send the browser query to 255.255.255.255:p (LAN
/// broadcast) and 127.0.0.1:p (this machine; broadcast loopback is not
/// guaranteed) for every port, twice, and collect every answer until
/// `budget`. A server seen on several addresses (loopback + its LAN IP) is
/// listed once, preferring 127.0.0.1; identity = its server key, else
/// name + port.
async fn lan_discover(ports: &[u16], budget: Duration) -> Vec<Row> {
    lan_discover_to(ports, budget, &[IpAddr::from([255, 255, 255, 255]), IpAddr::from([127, 0, 0, 1])]).await
}

async fn lan_discover_to(ports: &[u16], budget: Duration, targets: &[IpAddr]) -> Vec<Row> {
    let Ok(sock) = UdpSocket::bind("0.0.0.0:0").await else { return vec![] };
    let _ = sock.set_broadcast(true);
    let t0 = Instant::now();
    let deadline = t0 + budget;
    let base: u64 = rand::random::<u64>() & !0xFFFF_FFFF;
    let mut sent: HashMap<u64, Instant> = HashMap::new();
    // (ip, port) -> (rtt ms, info)
    let mut found: HashMap<SocketAddr, (u64, query::QueryInfo)> = HashMap::new();
    let mut buf = [0u8; 2048];
    for attempt in 0..2u64 {
        let nonce = base | (attempt << 24) | 0x4C41; // "LA"
        sent.insert(nonce, Instant::now());
        let req = query::build_request(nonce);
        for ip in targets {
            for p in ports {
                let _ = sock.send_to(&req, SocketAddr::new(*ip, *p)).await;
            }
        }
        let until = if attempt == 0 { (Instant::now() + Duration::from_millis(300)).min(deadline) } else { deadline };
        loop {
            let left = until.saturating_duration_since(Instant::now());
            if left.is_zero() {
                break;
            }
            match tokio::time::timeout(left, sock.recv_from(&mut buf)).await {
                Ok(Ok((n, from))) => {
                    let Some((nonce, info)) = query::parse_reply(&buf[..n]) else { continue };
                    let Some(t) = sent.get(&nonce) else { continue };
                    let ms = t.elapsed().as_millis() as u64;
                    match found.get(&from) {
                        Some((prev, _)) if *prev <= ms => {}
                        _ => {
                            found.insert(from, (ms, info));
                        }
                    }
                }
                Ok(Err(_)) => continue, // e.g. ICMP port-unreachable on Windows
                Err(_) => break,
            }
        }
        if Instant::now() >= deadline {
            break;
        }
    }
    // One row per server identity, loopback preferred, then the fastest.
    let mut by_id: HashMap<String, (SocketAddr, u64, query::QueryInfo)> = HashMap::new();
    for (addr, (ms, info)) in found {
        let id = if info.server_key.trim().is_empty() {
            format!("{}|{}", info.name, addr.port())
        } else {
            info.server_key.clone()
        };
        let better = match by_id.get(&id) {
            None => true,
            Some((a, pms, _)) => {
                (addr.ip().is_loopback() && !a.ip().is_loopback())
                    || (addr.ip().is_loopback() == a.ip().is_loopback() && ms < *pms)
            }
        };
        if better {
            by_id.insert(id, (addr, ms, info));
        }
    }
    let mut rows: Vec<Row> = by_id
        .into_values()
        .map(|(addr, ms, info)| {
            let mut r = Row {
                host: addr.ip().to_string(),
                port: addr.port(),
                name: format!("{}:{}", addr.ip(), addr.port()),
                source: "lan",
                ..Default::default()
            };
            apply_info(&mut r, ms, &info);
            r
        })
        .collect();
    rows.sort_by(|a, b| (a.host.as_str(), a.port).cmp(&(b.host.as_str(), b.port)));
    rows
}

fn handle_reply(
    pkt: &[u8],
    sent: &HashMap<u64, (usize, Instant)>,
    best: &mut [Option<(u64, query::QueryInfo)>],
) {
    let Some((nonce, info)) = query::parse_reply(pkt) else { return };
    let Some((i, t)) = sent.get(&nonce) else { return };
    let ms = t.elapsed().as_millis() as u64;
    match &best[*i] {
        Some((prev, _)) if *prev <= ms => {}
        _ => best[*i] = Some((ms, info)),
    }
}

#[tokio::main(flavor = "current_thread")]
async fn main() {
    let args = Args::parse();
    let masters = master_list(&args.master);
    // One master: a 4 s budget. Several: 3 s each so the whole chain
    // stays inside the browser's deadline (12 s + 4 s per fallback).
    let per_try = if masters.len() > 1 { Duration::from_secs(3) } else { Duration::from_secs(4) };
    let lan_ports = if args.lan { parse_port_range(&args.lan_ports) } else { None };
    let mut meta = Meta { master: None, lan: lan_ports.is_some() };

    let fetch = async {
        if masters.is_empty() {
            return (Status { code: "no_master", msg: String::new(), listed: 0, skipped: 0 }, vec![], None);
        }
        fetch_masters(&masters, per_try).await
    };
    let lan = async {
        match &lan_ports {
            Some(p) => lan_discover(p, Duration::from_millis(900)).await,
            None => vec![],
        }
    };
    let ((mut st, mut rows, used), lan_rows) = tokio::join!(fetch, lan);
    meta.master = used;
    for d in &args.direct {
        match split_host_port(d) {
            Some((h, p)) => rows.push(Row {
                name: format!("{h}:{p}"),
                host: h,
                port: p,
                ping: -1,
                source: "direct",
                ..Default::default()
            }),
            None => st.skipped += 1,
        }
    }
    // Dedupe host:port (keep first), cap the work. LAN answers go first: they
    // are already live, and a master / direct entry for the same address is
    // the same server.
    let n_lan = lan_rows.len();
    let mut all: Vec<Row> = lan_rows;
    all.append(&mut rows);
    let mut seen = std::collections::HashSet::new();
    all.retain(|r| seen.insert((r.host.to_ascii_lowercase(), r.port)));
    all.truncate(MAX_SERVERS);
    let mut rows = all;
    let n_lan = n_lan.min(rows.len());

    if rows.len() > n_lan {
        ping_all(&mut rows[n_lan..], Duration::from_millis(args.timeout_ms.clamp(300, 10_000))).await;
    }
    use std::io::Write;
    let mut so = std::io::stdout().lock();
    let _ = so.write_all(render(&args.gen, &st, &meta, &rows, true).as_bytes());
    let _ = so.flush();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn loopback_master_urls_bypass_the_proxy() {
        for u in ["http://127.0.0.1:7778", "http://localhost/", "https://LOCALHOST:443/x", "http://[::1]:7778/v1", "http://u:p@127.0.0.2:1"] {
            assert!(is_loopback_url(u), "{u}");
        }
        for u in ["http://master.example.com", "https://10.0.0.1:7778", "http://127.0.0.1.evil.com/", "http://[2001:db8::1]:80"] {
            assert!(!is_loopback_url(u), "{u}");
        }
    }

    #[test]
    fn master_list_skips_malformed_entries() {
        let body = r#"[
            {"name":"Good","host":"1.2.3.4","port":7777,"players":2,"max_players":8,"map":"Map_Arena_Pit","mode":"Free Fight","pwd_protected":true,"proto_ver":3,"version":"0.1.0","region":"EU"},
            {"name":"NoPort","host":"1.2.3.4"},
            {"name":"BadPort","host":"1.2.3.4","port":70000},
            {"name":"BadHost","host":"evil host;rm","port":7777},
            "not an object",
            {"host":"5.6.7.8","port":7778}
        ]"#;
        let (rows, skipped) = parse_master_list(body).unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(skipped, 4);
        assert_eq!(rows[0].name, "Good");
        assert!(rows[0].pwd);
        assert_eq!(rows[0].region, "EU");
        assert_eq!(rows[1].name, "5.6.7.8:7778"); // nameless → address
        assert_eq!(rows[1].max, 0);
    }

    #[test]
    fn master_rows_carry_the_build_identity() {
        let h = "AB".repeat(32);
        let body = format!(r#"[{{"host":"1.2.3.4","port":7777,"proto_ver":6,"proto_min":6,"proto_max":7,"content_hash":"{h}"}},{{"host":"1.2.3.4","port":7778,"content_hash":"junk"}}]"#);
        let (rows, _) = parse_master_list(&body).unwrap();
        assert_eq!((rows[0].content.as_str(), rows[0].proto_min, rows[0].proto_max), ("abababababababab", 6, 7));
        assert_eq!(rows[1].content, "", "a malformed hash is dropped");
        let st = Status { code: "ok", msg: String::new(), listed: 2, skipped: 0 };
        let out = render("g", &st, &Meta::default(), &rows, true);
        let s: Vec<&str> = out.lines().find(|l| l.starts_with("S\t")).unwrap().split('\t').collect();
        assert_eq!(&s[15..20], &["abababababababab", "6", "7", "", "0"]);
        let mut r = rows[1].clone();
        apply_info(&mut r, 5, &query::QueryInfo { content_tag: "CDCDCDCDCDCDCDCD".into(), proto_min: 6, proto_max: 6, ..Default::default() });
        assert_eq!((r.content.as_str(), r.proto_min), ("cdcdcdcdcdcdcdcd", 6));
    }

    #[test]
    fn nat_and_punch_columns() {
        let body = r#"[{"host":"1.2.3.4","port":7777,"nat":"cone","punch":true},{"host":"1.2.3.4","port":7778}]"#;
        let (rows, _) = parse_master_list(body).unwrap();
        assert_eq!((rows[0].nat.as_str(), rows[0].punch, rows[1].nat.as_str(), rows[1].punch), ("cone", true, "", false));
        let st = Status { code: "ok", msg: String::new(), listed: 2, skipped: 0 };
        let out = render("g", &st, &Meta::default(), &rows, true);
        let s: Vec<&str> = out.lines().find(|l| l.starts_with("S\t")).unwrap().split('\t').collect();
        assert_eq!(&s[18..20], &["cone", "1"]);
    }

    /// The server mods columns: from the listing (`mods`, `mods_bytes`) and from the server's
    /// own query reply; absent = 0.
    #[test]
    fn mods_columns() {
        let body = r#"[{"host":"1.2.3.4","port":7777,"mods":3,"mods_bytes":2049},{"host":"1.2.3.4","port":7778,"mods":999}]"#;
        let (mut rows, _) = parse_master_list(body).unwrap();
        assert_eq!((rows[0].mods, rows[0].mods_kb), (3, 3));
        assert_eq!(rows[1].mods, hsmp_ipc::schema::mods::MAX_MODS as u32, "clamped");
        let st = Status { code: "ok", msg: String::new(), listed: 2, skipped: 0 };
        let out = render("g", &st, &Meta::default(), &rows, true);
        let s: Vec<&str> = out.lines().find(|l| l.starts_with("S\t")).unwrap().split('\t').collect();
        assert_eq!(&s[20..], &["3", "3"]);
        apply_info(&mut rows[1], 5, &query::QueryInfo { mods: 2, mods_kb: 40, ..Default::default() });
        assert_eq!((rows[1].mods, rows[1].mods_kb), (2, 40));
        let none: Vec<serde_json::Value> = serde_json::from_str(r#"[{"host":"1.2.3.4","port":1}]"#).unwrap();
        assert_eq!(row_from_json(&none[0]).map(|r| (r.mods, r.mods_kb)), Some((0, 0)));
    }

    #[test]
    fn master_list_non_array_is_error() {
        assert!(parse_master_list("{\"a\":1}").is_err());
        assert!(parse_master_list("<html>").is_err());
        assert_eq!(parse_master_list("[]").unwrap().0.len(), 0);
    }

    #[test]
    fn tabs_and_newlines_cannot_break_lines() {
        let body = r#"[{"name":"A\tB\nS\tfake","host":"1.2.3.4","port":1}]"#;
        let (rows, _) = parse_master_list(body).unwrap();
        let st = Status { code: "ok", msg: "x\ny".into(), listed: 1, skipped: 0 };
        let out = render("g", &st, &Meta::default(), &rows, true);
        let lines: Vec<&str> = out.lines().collect();
        assert_eq!(lines.iter().filter(|l| l.starts_with("S\t")).count(), 1);
        assert_eq!(lines.last().unwrap().split('\t').count(), 22);
    }

    #[test]
    fn host_port_parsing() {
        assert_eq!(split_host_port("1.2.3.4:7777"), Some(("1.2.3.4".into(), 7777)));
        assert_eq!(split_host_port(" my.host-name.net:1 "), Some(("my.host-name.net".into(), 1)));
        assert_eq!(split_host_port("[::1]:7777"), Some(("::1".into(), 7777)));
        assert_eq!(split_host_port("1.2.3.4"), None);
        assert_eq!(split_host_port("1.2.3.4:0"), None);
        assert_eq!(split_host_port("a b:1"), None);
        assert_eq!(split_host_port("x:99999"), None);
    }

    #[tokio::test]
    async fn pings_a_live_server_and_times_out_a_dead_one() {
        let sock = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let port = sock.local_addr().unwrap().port();
        tokio::spawn(async move {
            let mut buf = [0u8; 2048];
            loop {
                let Ok((n, from)) = sock.recv_from(&mut buf).await else { return };
                if let Some(nonce) = query::parse_request(&buf[..n]) {
                    let info = query::QueryInfo {
                        name: "Live One".into(), map: "Map_Arena_Yard".into(),
                        players: 2, max_players: 4, proto_ver: 3, ..Default::default()
                    };
                    let _ = sock.send_to(&query::build_reply(nonce, &info), from).await;
                }
            }
        });
        // A bound-but-silent socket = server that never answers.
        let dead = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
        let dead_port = dead.local_addr().unwrap().port();
        let mut rows = vec![
            Row { host: "127.0.0.1".into(), port, ping: -1, source: "master", ..Default::default() },
            Row { host: "127.0.0.1".into(), port: dead_port, ping: -1, source: "master", ..Default::default() },
        ];
        ping_all(&mut rows, Duration::from_millis(900)).await;
        assert!(rows[0].live);
        assert!(rows[0].ping >= 0);
        assert_eq!(rows[0].name, "Live One");
        assert_eq!(rows[0].players, 2);
        assert_eq!(rows[1].ping, -2);
        assert!(!rows[1].live);
        drop(dead);
    }

    #[test]
    fn master_list_splits_dedupes_and_keeps_order() {
        let a = vec!["http://a:1/".to_string(), " http://b:2 , http://a:1,,http://c:3".to_string()];
        assert_eq!(master_list(&a), vec!["http://a:1", "http://b:2", "http://c:3"]);
        assert!(master_list(&[]).is_empty());
    }

    #[test]
    fn port_ranges() {
        assert_eq!(parse_port_range("7777"), Some(vec![7777]));
        assert_eq!(parse_port_range(" 7777-7779 "), Some(vec![7777, 7778, 7779]));
        assert_eq!(parse_port_range("7779-7777"), None);
        assert_eq!(parse_port_range("0"), None);
        assert_eq!(parse_port_range("1-100"), None); // > 64 ports
        assert_eq!(parse_port_range("x"), None);
    }

    #[test]
    fn render_names_the_master_and_lan_count() {
        let st = Status { code: "ok", msg: String::new(), listed: 1, skipped: 0 };
        let meta = Meta { master: Some(("http://b:2".into(), 2, 3)), lan: true };
        let rows = vec![
            Row { host: "192.168.1.5".into(), port: 7777, source: "lan", ..Default::default() },
            Row { host: "1.2.3.4".into(), port: 7777, source: "master", ..Default::default() },
        ];
        let out = render("g", &st, &meta, &rows, true);
        assert!(out.contains("\nmaster\thttp://b:2\t2\t3\n"), "{out}");
        assert!(out.contains("\nlan\t1\t1\n"), "{out}");
        let plain = render("g", &st, &Meta::default(), &rows, true);
        assert!(!plain.contains("\nmaster\t") && !plain.contains("\nlan\t"));
    }

    /// Minimal HTTP/1.1 responder: answers every request with `body`.
    async fn fake_master(body: &'static str) -> String {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = l.local_addr().unwrap();
        tokio::spawn(async move {
            loop {
                let Ok((mut s, _)) = l.accept().await else { return };
                tokio::spawn(async move {
                    let mut buf = [0u8; 4096];
                    let _ = s.read(&mut buf).await;
                    let resp = format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                        body.len(), body);
                    let _ = s.write_all(resp.as_bytes()).await;
                    let _ = s.shutdown().await;
                });
            }
        });
        format!("http://{addr}")
    }

    /// A master that never answers: it accepts and closes every connection at once.
    /// The port stays bound for the whole test. (Binding a port and releasing it was
    /// flaky: the OS hands the freed port to the next bind, which may be another
    /// test's fake master or listener running in parallel, and then the "dead"
    /// master answered, or answered HTTP 404 -> master_error.)
    async fn dead_url() -> String {
        let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = l.local_addr().unwrap();
        tokio::spawn(async move {
            loop {
                let Ok((s, _)) = l.accept().await else { return };
                drop(s);
            }
        });
        format!("http://{addr}")
    }

    #[tokio::test]
    async fn masters_are_tried_in_order_and_the_answering_one_is_named() {
        let dead = dead_url().await;
        let live = fake_master(r#"[{"name":"Fallback Srv","host":"5.6.7.8","port":7777}]"#).await;
        let urls = vec![dead.clone(), live.clone()];
        let (st, rows, used) = fetch_masters(&urls, Duration::from_secs(2)).await;
        assert_eq!(st.code, "ok");
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].name, "Fallback Srv");
        assert_eq!(used, Some((live, 2, 2)));
        // the dead one was tried first (and is not the one named)
        assert_ne!(used.as_ref().map(|u| u.0.clone()), Some(dead));
    }

    #[tokio::test]
    async fn all_masters_down_reports_every_one() {
        let urls = vec![dead_url().await, dead_url().await];
        assert_ne!(urls[0], urls[1]);
        let (st, rows, used) = fetch_masters(&urls, Duration::from_secs(2)).await;
        assert_eq!(st.code, "master_unreachable");
        assert!(rows.is_empty() && used.is_none());
        assert!(st.msg.starts_with("2 lists tried:"), "{}", st.msg);
        assert!(st.msg.contains(&urls[0]) && st.msg.contains(&urls[1]), "{}", st.msg);
    }

    #[tokio::test]
    async fn lan_discovery_finds_a_local_server_once() {
        let sock = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let port = sock.local_addr().unwrap().port();
        tokio::spawn(async move {
            let mut buf = [0u8; 2048];
            loop {
                let Ok((n, from)) = sock.recv_from(&mut buf).await else { return };
                if let Some(nonce) = query::parse_request(&buf[..n]) {
                    let info = query::QueryInfo {
                        name: "LAN Pit".into(), map: "Map_Arena_Pit".into(), players: 1, max_players: 8,
                        server_key: "ab".repeat(32), ..Default::default()
                    };
                    let _ = sock.send_to(&query::build_reply(nonce, &info), from).await;
                }
            }
        });
        // loopback only (no broadcast in a unit test); both attempts answer -> one row
        let rows = lan_discover_to(&[port], Duration::from_millis(700), &[IpAddr::from([127, 0, 0, 1])]).await;
        assert_eq!(rows.len(), 1, "{rows:?}");
        assert_eq!(rows[0].host, "127.0.0.1");
        assert_eq!(rows[0].port, port);
        assert_eq!(rows[0].name, "LAN Pit");
        assert_eq!(rows[0].source, "lan");
        assert!(rows[0].live && rows[0].ping >= 0);
        // nothing listening -> nothing found, within the budget
        let t = Instant::now();
        let dead = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
        let none = lan_discover_to(&[dead.local_addr().unwrap().port()], Duration::from_millis(400),
            &[IpAddr::from([127, 0, 0, 1])]).await;
        assert!(none.is_empty());
        assert!(t.elapsed() < Duration::from_millis(1500));
    }
}
