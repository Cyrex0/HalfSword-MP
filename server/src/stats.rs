//! The server's health report: every 10 s one `stats:` line, one `stats peer` line per
//! player and the same numbers as a structured event (target `hsmp_server::stats`, so the
//! JSON-lines events file holds them with typed fields), plus the counters behind the
//! shutdown summary and the RCON `REPORT` verb.
//!
//! The hooks are cheap (a short mutex hold): the tick time, pose frames in / relayed per
//! peer, combat verdicts and lag-comp rejects by reason code, joins / leaves / kicks /
//! refusals, the master registration state, and the NAT status (nat/mod.rs: port mapping,
//! STUN type, punch relay). Per-peer transport numbers (RTT, loss, retransmits, bytes)
//! are read from the connections at report time.

use crate::server::ServerState;
use serde_json::{json, Value};
use std::collections::{BTreeMap, HashMap};
use std::net::SocketAddr;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};
use tracing::info;

pub const EVERY: Duration = Duration::from_secs(10);

#[derive(Default, Debug, Clone)]
pub struct Master {
    pub url: Option<String>,
    pub listed: bool,
    pub last_ok_ms: Option<u64>,
    pub fails: u32,
    pub last_error: Option<String>,
    /// ip:port as the master sees this server.
    pub public_addr: Option<String>,
}

#[derive(Default)]
struct Inner {
    ticks_us: Vec<u32>,
    pose_in: HashMap<u32, u32>,
    pose_out: HashMap<u32, u32>,
    combat_ok: u64,
    combat_rej: BTreeMap<String, u64>,
    lagcomp_rej: BTreeMap<String, u64>,
    refused: BTreeMap<String, u64>,
    master: Master,
    prev_bytes: HashMap<SocketAddr, (u64, u64, u64, u64, u64)>,
    prev_cpu: Option<(u64, Instant)>,
    last: String,
    totals: Totals,
}

#[derive(Default, Clone, Debug)]
pub struct Totals {
    pub joins: u64,
    pub leaves: u64,
    pub kicks: u64,
    pub refused: u64,
    pub peak_players: usize,
    pub combat_ok: u64,
    pub combat_rejected: u64,
    pub lagcomp_rejects: u64,
    pub bytes_out: u64,
    pub bytes_in: u64,
}

fn st() -> std::sync::MutexGuard<'static, Inner> {
    static S: OnceLock<Mutex<Inner>> = OnceLock::new();
    S.get_or_init(Default::default).lock().unwrap_or_else(|e| e.into_inner())
}

fn started() -> Instant {
    static T: OnceLock<Instant> = OnceLock::new();
    *T.get_or_init(Instant::now)
}

/// Stable reason code ("code: details" -> "code"), at most 32 chars.
fn code(reason: &str) -> String {
    let c = reason.split(':').next().unwrap_or(reason).trim();
    let c: String = c.chars().take(32).collect();
    if c.is_empty() { "unknown".into() } else { c }
}

pub fn tick(us: u32) {
    let mut s = st();
    if s.ticks_us.len() < 4096 {
        s.ticks_us.push(us);
    }
}
pub fn pose_in(peer: u32) {
    *st().pose_in.entry(peer).or_insert(0) += 1;
}
pub fn pose_relayed(peer: u32, copies: usize) {
    *st().pose_out.entry(peer).or_insert(0) += copies as u32;
}
/// Reject codes that come from lag compensation (lagcomp.rs `Eval::Reject`).
const LAGCOMP_CODES: &[&str] = &["no_ts", "no_history", "clock_rate", "ts_future", "ts_old", "ts_inconsistent", "no_clock", "future", "rewind_cap", "body_miss", "no_cover", "blade_miss", "reach"];

/// A final damage verdict (combat_glue). Lag-comp rejects are also counted on their own.
pub fn combat(accepted: bool, reason: &str) {
    let mut s = st();
    if accepted {
        s.combat_ok += 1;
        s.totals.combat_ok += 1;
        return;
    }
    let c = code(reason);
    if LAGCOMP_CODES.contains(&c.as_str()) {
        *s.lagcomp_rej.entry(c.clone()).or_insert(0) += 1;
        s.totals.lagcomp_rejects += 1;
    }
    *s.combat_rej.entry(c).or_insert(0) += 1;
    s.totals.combat_rejected += 1;
}
pub fn joined(players_now: usize) {
    let mut s = st();
    s.totals.joins += 1;
    s.totals.peak_players = s.totals.peak_players.max(players_now);
}
pub fn left() {
    st().totals.leaves += 1;
}
pub fn kicked() {
    st().totals.kicks += 1;
}
/// A join refused before it was accepted (version, content, full, banned, ...).
pub fn refused(why: &str) {
    let mut s = st();
    *s.refused.entry(code(why)).or_insert(0) += 1;
    s.totals.refused += 1;
}
pub fn master_url(url: &str) {
    st().master.url = Some(url.to_string());
}
pub fn master_ok() {
    let mut s = st();
    s.master.listed = true;
    s.master.fails = 0;
    s.master.last_ok_ms = Some(crate::events::wall_ms());
    s.master.last_error = None;
}
pub fn master_failed(err: &str) {
    let mut s = st();
    s.master.fails += 1;
    s.master.last_error = Some(err.chars().take(120).collect());
}
pub fn master_unlisted() {
    st().master.listed = false;
}
pub fn master_public(addr: &str) {
    st().master.public_addr = Some(addr.to_string());
}
/// The NAT status (nat/mod.rs) as the report carries it: the router port mapping, the STUN
/// NAT type, the public endpoint, the port players use and whether the master's punch relay
/// is connected.
pub fn nat_json(s: &crate::nat::Status) -> Value {
    let pm = match &s.port_map {
        crate::nat::PortMap::Off => "off".to_string(),
        crate::nat::PortMap::Trying => "trying".to_string(),
        crate::nat::PortMap::Mapped { method, external, double, .. } => format!("{method} {external}{}", if *double { " (double NAT)" } else { "" }),
        crate::nat::PortMap::Failed(why) => format!("failed: {why}"),
    };
    json!({"port_map": pm, "type": s.mapping.map(|m| m.as_str()), "kind": s.nat_kind(), "public": s.public.map(|p| p.to_string()),
           "advertised_port": s.advertised_port(), "relay": s.listening, "punch": s.punch_on, "stun": s.stun_on})
}

fn nat_text(v: &Value) -> String {
    if v.is_null() {
        return "-".into();
    }
    format!("port map {}, type {}, port {}, relay {}", v["port_map"].as_str().unwrap_or("?"), v["type"].as_str().unwrap_or("unknown"), v["advertised_port"], if v["relay"].as_bool() == Some(true) { "connected" } else { "off" })
}
/// Start the uptime clock (call once at start-up).
pub fn start() {
    let _ = started();
}

/// The newest report (the RCON `REPORT` verb).
pub fn last_report() -> String {
    let s = st();
    if s.last.is_empty() { "no stats yet (the first report comes 10 s after start)".into() } else { s.last.clone() }
}
pub fn totals() -> Totals {
    st().totals.clone()
}

/// This process's CPU time (user + kernel), microseconds.
#[cfg(windows)]
fn cpu_us() -> Option<u64> {
    #[repr(C)]
    #[derive(Default)]
    struct FileTime {
        lo: u32,
        hi: u32,
    }
    extern "system" {
        fn GetCurrentProcess() -> isize;
        fn GetProcessTimes(h: isize, c: *mut FileTime, e: *mut FileTime, k: *mut FileTime, u: *mut FileTime) -> i32;
    }
    let (mut c, mut e, mut k, mut u) = (FileTime::default(), FileTime::default(), FileTime::default(), FileTime::default());
    // SAFETY: the pseudo handle of this process and four out pointers.
    let ok = unsafe { GetProcessTimes(GetCurrentProcess(), &mut c, &mut e, &mut k, &mut u) } != 0;
    let t = |f: &FileTime| ((f.hi as u64) << 32 | f.lo as u64) / 10;
    ok.then(|| t(&k) + t(&u))
}

#[cfg(not(windows))]
fn cpu_us() -> Option<u64> {
    let s = std::fs::read_to_string("/proc/self/stat").ok()?;
    let rest = s.rsplit_once(')')?.1;
    let f: Vec<&str> = rest.split_whitespace().collect();
    // fields 14 and 15 (utime, stime) are at 11, 12 after the ")"
    let ticks: u64 = f.get(11)?.parse::<u64>().ok()? + f.get(12)?.parse::<u64>().ok()?;
    Some(ticks * 10_000) // USER_HZ = 100
}

/// One player's numbers for one report.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct PeerLine {
    pub id: u32,
    pub nick: String,
    pub addr: String,
    pub rtt_ms: f64,
    pub min_rtt_ms: f64,
    pub jitter_ms: f64,
    pub loss_pct: f64,
    pub retransmits: u64,
    pub in_kbs: f64,
    pub out_kbs: f64,
    pub pose_in_hz: f64,
    pub pose_relayed_hz: f64,
    pub budget_kbs: f64,
}

/// Everything one report says.
#[derive(Debug, Clone, Default)]
pub struct Report {
    pub uptime_s: u64,
    pub players: usize,
    pub tick_avg_ms: f64,
    pub tick_p99_ms: f64,
    pub tick_max_ms: f64,
    pub ticks: usize,
    pub cpu_pct: Option<f64>,
    pub up_kbs: f64,
    pub down_kbs: f64,
    pub phase: String,
    pub round: u64,
    pub arena: String,
    pub combat_ok: u64,
    pub combat_rej: BTreeMap<String, u64>,
    pub lagcomp_rej: BTreeMap<String, u64>,
    pub refused: BTreeMap<String, u64>,
    pub master: Master,
    pub nat: Option<Value>,
    pub peers: Vec<PeerLine>,
}

fn counts(m: &BTreeMap<String, u64>) -> String {
    if m.is_empty() {
        return "0".into();
    }
    let n: u64 = m.values().sum();
    format!("{n} ({})", m.iter().map(|(k, v)| format!("{k} {v}")).collect::<Vec<_>>().join(", "))
}

impl Report {
    /// The `stats:` line and one `stats peer` line per player.
    pub fn lines(&self, now_ms: u64) -> Vec<String> {
        let master = match &self.master.url {
            None => "off".to_string(),
            Some(_) if self.master.listed => format!("listed{} (heartbeat {} s ago)", self.master.public_addr.as_ref().map(|a| format!(" as {a}")).unwrap_or_default(), self.master.last_ok_ms.map_or(0, |t| now_ms.saturating_sub(t) / 1000)),
            Some(_) => format!("NOT listed ({} failure(s){})", self.master.fails, self.master.last_error.as_ref().map(|e| format!(": {e}")).unwrap_or_default()),
        };
        let nat = self.nat.as_ref().map_or("-".to_string(), nat_text);
        let cpu = self.cpu_pct.map_or("?".to_string(), |c| format!("{c:.0}%"));
        let mut out = vec![format!(
            "stats: {} player(s) | tick avg {:.2} ms p99 {:.2} ms max {:.1} ms ({} ticks) | cpu {cpu} | up {:.1} KB/s down {:.1} KB/s | {} round {} {} | combat accepted {} rejected {} | lag comp rejects {} | refused joins {} | master {master} | nat {nat} | up {} s",
            self.players, self.tick_avg_ms, self.tick_p99_ms, self.tick_max_ms, self.ticks, self.up_kbs, self.down_kbs,
            self.phase, self.round, if self.arena.is_empty() { "-" } else { &self.arena },
            self.combat_ok, counts(&self.combat_rej), counts(&self.lagcomp_rej), counts(&self.refused), self.uptime_s
        )];
        for p in &self.peers {
            out.push(format!(
                "stats peer {} nick={:?} {}: rtt {:.0} ms (min {:.0}, jitter {:.0}) | loss {:.2}% | retransmits {} | in {:.1} KB/s out {:.1} KB/s | pose in {:.1} Hz relayed {:.1} Hz | budget {:.0} KB/s",
                p.id, p.nick, p.addr, p.rtt_ms, p.min_rtt_ms, p.jitter_ms, p.loss_pct, p.retransmits, p.in_kbs, p.out_kbs, p.pose_in_hz, p.pose_relayed_hz, p.budget_kbs
            ));
        }
        out
    }

    pub fn json(&self) -> Value {
        json!({
            "uptime_s": self.uptime_s, "players": self.players,
            "tick_ms": {"avg": self.tick_avg_ms, "p99": self.tick_p99_ms, "max": self.tick_max_ms, "n": self.ticks},
            "cpu_pct": self.cpu_pct, "up_kbs": self.up_kbs, "down_kbs": self.down_kbs,
            "phase": self.phase, "round": self.round, "arena": self.arena,
            "combat": {"accepted": self.combat_ok, "rejected": self.combat_rej}, "lagcomp_rejects": self.lagcomp_rej,
            "refused": self.refused,
            "master": {"url": self.master.url, "listed": self.master.listed, "public_addr": self.master.public_addr, "last_ok_ms": self.master.last_ok_ms, "fails": self.master.fails, "last_error": self.master.last_error},
            "nat": self.nat,
            "peers": self.peers.iter().map(|p| json!({
                "peer_id": p.id, "nick": p.nick, "addr": p.addr, "rtt_ms": p.rtt_ms, "min_rtt_ms": p.min_rtt_ms, "jitter_ms": p.jitter_ms,
                "loss_pct": p.loss_pct, "retransmits": p.retransmits, "in_kbs": p.in_kbs, "out_kbs": p.out_kbs,
                "pose_in_hz": p.pose_in_hz, "pose_relayed_hz": p.pose_relayed_hz, "budget_kbs": p.budget_kbs,
            })).collect::<Vec<_>>(),
        })
    }
}

fn pct(v: &mut [u32], p: f64) -> u32 {
    if v.is_empty() {
        return 0;
    }
    v.sort_unstable();
    v[((v.len() as f64 - 1.0) * p).round() as usize]
}

/// Gather one report (resets the per-interval counters).
pub async fn collect(state: &Arc<ServerState>, secs: f64) -> Report {
    let status: Value = serde_json::from_str(&crate::server::rcon_status(state).await).unwrap_or(Value::Null);
    let peers: Vec<(SocketAddr, u32, String)> = state.lock().lock().await.peers.iter().map(|(a, p)| (*a, p.id, p.nick.clone())).collect();
    let conns: HashMap<SocketAddr, hsmp_net::net::ConnStats> = state.net.conn_stats_by_addr().into_iter().collect();
    let budgets: Vec<(SocketAddr, f64)> = peers.iter().map(|(a, _, _)| (*a, state.relay.budget_bps(a))).collect();
    let mut s = st();
    let secs = secs.max(1e-3);
    let mut t = std::mem::take(&mut s.ticks_us);
    let n = t.len();
    let avg = if n == 0 { 0.0 } else { t.iter().map(|&x| x as f64).sum::<f64>() / n as f64 / 1000.0 };
    let max = t.iter().copied().max().unwrap_or(0) as f64 / 1000.0;
    let p99 = pct(&mut t, 0.99) as f64 / 1000.0;
    let now = Instant::now();
    let cpu = cpu_us();
    let cpu_pct = match (s.prev_cpu, cpu) {
        (Some((c0, t0)), Some(c1)) => Some(100.0 * c1.saturating_sub(c0) as f64 / now.duration_since(t0).as_micros().max(1) as f64),
        _ => None,
    };
    if let Some(c) = cpu {
        s.prev_cpu = Some((c, now));
    }
    let pose_in = std::mem::take(&mut s.pose_in);
    let pose_out = std::mem::take(&mut s.pose_out);
    let (mut up, mut down) = (0u64, 0u64);
    let mut lines = vec![];
    let mut prev = std::mem::take(&mut s.prev_bytes);
    for (addr, id, nick) in &peers {
        let Some(c) = conns.get(addr) else { continue };
        let (bs0, br0, ps0, pl0, rt0) = prev.remove(addr).unwrap_or((0, 0, 0, 0, 0));
        let fresh = c.bytes_sent < bs0;
        let d = |now: u64, then: u64| if fresh { now } else { now.saturating_sub(then) };
        let (dbs, dbr, dps, dpl, drt) = (d(c.bytes_sent, bs0), d(c.bytes_recv, br0), d(c.pkts_sent, ps0), d(c.pkts_lost, pl0), d(c.retransmits, rt0));
        s.prev_bytes.insert(*addr, (c.bytes_sent, c.bytes_recv, c.pkts_sent, c.pkts_lost, c.retransmits));
        up += dbs;
        down += dbr;
        lines.push(PeerLine {
            id: *id,
            nick: nick.clone(),
            addr: addr.to_string(),
            rtt_ms: c.srtt_ms,
            min_rtt_ms: c.min_rtt_ms,
            jitter_ms: c.rttvar_ms,
            loss_pct: if dps > 0 { 100.0 * dpl as f64 / dps as f64 } else { 0.0 },
            retransmits: drt,
            in_kbs: dbr as f64 / 1024.0 / secs,
            out_kbs: dbs as f64 / 1024.0 / secs,
            pose_in_hz: pose_in.get(id).copied().unwrap_or(0) as f64 / secs,
            pose_relayed_hz: pose_out.get(id).copied().unwrap_or(0) as f64 / secs,
            budget_kbs: budgets.iter().find(|(a, _)| a == addr).map_or(0.0, |(_, b)| b / 1024.0),
        });
    }
    lines.sort_by_key(|p| p.id);
    s.totals.bytes_out += up;
    s.totals.bytes_in += down;
    Report {
        uptime_s: started().elapsed().as_secs(),
        players: peers.len(),
        tick_avg_ms: avg,
        tick_p99_ms: p99,
        tick_max_ms: max,
        ticks: n,
        cpu_pct,
        up_kbs: up as f64 / 1024.0 / secs,
        down_kbs: down as f64 / 1024.0 / secs,
        phase: status["phase"].as_str().unwrap_or("?").to_string(),
        round: status["round"].as_u64().unwrap_or(0),
        arena: status["arena"].as_str().unwrap_or("").to_string(),
        combat_ok: std::mem::take(&mut s.combat_ok),
        combat_rej: std::mem::take(&mut s.combat_rej),
        lagcomp_rej: std::mem::take(&mut s.lagcomp_rej),
        refused: std::mem::take(&mut s.refused),
        master: s.master.clone(),
        nat: Some(nat_json(&crate::nat::status())),
        peers: lines,
    }
}

/// Every EVERY: the report as log lines and one structured event.
pub fn spawn(state: Arc<ServerState>) {
    let _ = started();
    tokio::spawn(async move {
        let mut iv = tokio::time::interval(EVERY);
        iv.tick().await;
        let mut at = Instant::now();
        loop {
            iv.tick().await;
            let secs = at.elapsed().as_secs_f64();
            at = Instant::now();
            let r = collect(&state, secs).await;
            let lines = r.lines(crate::events::wall_ms());
            for l in &lines {
                info!(target: "hsmp_server::stats", "{l}");
            }
            // the structured copy goes to the JSON-lines file only (log_init: EVENT_ONLY)
            info!(target: "hsmp_server::stats::event", report = %r.json(), "stats event");
            st().last = lines.join("\n");
        }
    });
}

/// The clean-shutdown summary line.
pub fn shutdown_summary() {
    let t = totals();
    info!(uptime_s = started().elapsed().as_secs(), joins = t.joins, leaves = t.leaves, kicks = t.kicks, refused = t.refused,
          peak_players = t.peak_players, combat_accepted = t.combat_ok, combat_rejected = t.combat_rejected,
          lagcomp_rejects = t.lagcomp_rejects, mb_out = t.bytes_out / (1 << 20), mb_in = t.bytes_in / (1 << 20), "shutdown summary");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn report_lines_and_json() {
        let mut r = Report {
            uptime_s: 61,
            players: 1,
            tick_avg_ms: 0.4,
            tick_p99_ms: 1.25,
            tick_max_ms: 3.0,
            ticks: 300,
            cpu_pct: Some(4.0),
            up_kbs: 120.0,
            down_kbs: 40.5,
            phase: "Live".into(),
            round: 2,
            arena: "Map_Arena_Pit".into(),
            combat_ok: 5,
            master: Master { url: Some("https://m".into()), listed: true, last_ok_ms: Some(1_000), ..Default::default() },
            ..Default::default()
        };
        r.combat_rej.insert("range".into(), 2);
        r.lagcomp_rej.insert("body_miss".into(), 1);
        r.peers.push(PeerLine { id: 3, nick: "Bob".into(), addr: "203.0.113.7:5000".into(), rtt_ms: 41.6, min_rtt_ms: 35.0, jitter_ms: 4.0, loss_pct: 0.5, retransmits: 3, in_kbs: 20.0, out_kbs: 60.0, pose_in_hz: 60.0, pose_relayed_hz: 58.0, budget_kbs: 128.0 });
        let l = r.lines(31_000);
        assert_eq!(l.len(), 2);
        assert_eq!(l[0], "stats: 1 player(s) | tick avg 0.40 ms p99 1.25 ms max 3.0 ms (300 ticks) | cpu 4% | up 120.0 KB/s down 40.5 KB/s | Live round 2 Map_Arena_Pit | combat accepted 5 rejected 2 (range 2) | lag comp rejects 1 (body_miss 1) | refused joins 0 | master listed (heartbeat 30 s ago) | nat - | up 61 s");
        assert_eq!(l[1], "stats peer 3 nick=\"Bob\" 203.0.113.7:5000: rtt 42 ms (min 35, jitter 4) | loss 0.50% | retransmits 3 | in 20.0 KB/s out 60.0 KB/s | pose in 60.0 Hz relayed 58.0 Hz | budget 128 KB/s");
        r.master = Master { url: Some("https://m".into()), fails: 2, last_error: Some("timeout".into()), ..Default::default() };
        assert!(r.lines(0)[0].contains("master NOT listed (2 failure(s): timeout)"));
        r.master.url = None;
        r.nat = Some(json!({"port_map": "upnp 7777", "type": "cone", "advertised_port": 7777, "relay": true}));
        assert!(r.lines(0)[0].contains("master off | nat port map upnp 7777, type cone, port 7777, relay connected"), "{}", r.lines(0)[0]);
        let s = crate::nat::Status::default();
        assert_eq!(nat_json(&s)["port_map"], "off");
        let j = r.json();
        assert_eq!(j["peers"][0]["pose_relayed_hz"], 58.0);
        assert_eq!(j["combat"]["rejected"]["range"], 2);
        assert_eq!(j["tick_ms"]["p99"], 1.25);
    }

    #[test]
    fn counters_and_reason_codes() {
        assert_eq!(code("body_miss: contact 30 uu"), "body_miss");
        assert_eq!(code(""), "unknown");
        combat(false, "rewind_cap: 300 ms");
        combat(false, "range: too far");
        combat(true, "");
        let t = totals();
        assert!(t.lagcomp_rejects >= 1 && t.combat_rejected >= 1 && t.combat_ok >= 1);
        assert!(cpu_us().is_some() || cfg!(not(any(windows, target_os = "linux"))));
    }
}
