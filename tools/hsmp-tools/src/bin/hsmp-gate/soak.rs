//! Soak rules (scenarios with a `soak` block: soak_60m, soak_10m) and the crash-triage
//! evidence DoD-2 shares. Limits come from the scenario's `soak` object (scenarios.json).
//!
//! * SOAK-CRASH  `<run>/crash_triage.json` (`hsmp-tools crash-triage --since <run start> --until <run end>`,
//!               written by mp_test.ps1 or, if missing, by the gate itself): new dumps <= crashes_max,
//!               each reported with its signature; every game alive at quit_all.
//! * SOAK-MEM    `<run>/mem.jsonl` (`hsmp-gate memwatch`): per game, the samples after `mem_warmup_min`
//!               (from that process's first sample) -> least-squares private-bytes slope (MB/h) <
//!               `mem_slope_max_mb_per_h`; growth = peak - median(first 3 post-warm-up samples) <=
//!               `mem_growth_max_mb`; handle slope <= `handle_slope_max_per_h`. server/master/sidecar:
//!               slope <= `aux_mem_slope_max_mb_per_h`. Fewer than `mem_min_samples` = incomplete.
//! * SOAK-LUAERR Lua errors in the run's UE4SS.log (LUA_ERR*, "attempt to <op>", "stack traceback",
//!               "<file>.lua:<n>:", "bad argument #") + hsmp_log `lua_error` events <= `lua_errors_max`;
//!               `lua_error_allow` = regexes of lines to ignore.
//! * SOAK-HITCH  `stall_check` with `hitch_max_ms` (shared with DoD-2): per instance the `frame_hb`
//!               heartbeat (release profile, every 10 s) must be present, else incomplete; a `hitch`
//!               with ms > the limit and travel=false fails (no flag: the [travel, world_ready]
//!               window decides), and so do heartbeat gaps / frame_hb max_ms over the limit
//!               outside travel.

use crate::rules::{FAIL, INCOMPLETE, PASS};
use crate::util::{self, bv, ev_name, f64v, inst, s, sv, wall, Ev};
use regex::Regex;
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// (status, message, instance)
pub type Out = Vec<(&'static str, String, Option<String>)>;

pub struct SoakCtx<'a> {
    pub run: &'a Path,
    pub evs: &'a [Ev],
    pub sc: &'a Value,
    pub fin: Option<&'a Value>,
    pub triage: Option<&'a Value>,
    pub insts: &'a [String],
}

fn lim(sc: &Value, k: &str, default: f64) -> f64 {
    sc["soak"][k].as_f64().unwrap_or(default)
}

const MB: f64 = 1024.0 * 1024.0;

// ------------------------------------------------------------------------------------------
// crash-triage evidence (DoD-2 + SOAK-CRASH)
// ------------------------------------------------------------------------------------------

fn tools_exe() -> Option<PathBuf> {
    let dir = std::env::current_exe().ok()?.parent()?.to_path_buf();
    let p = dir.join(if cfg!(windows) { "hsmp-tools.exe" } else { "hsmp-tools" });
    p.is_file().then_some(p)
}

/// `<run>/crash_triage.json`; if it is missing and the run is a finished real run (final.json,
/// started_wall_ms, not a fixture), run `hsmp-tools crash-triage` over the run window first.
pub fn triage_report(run: &Path, cfg: &Value, fin: Option<&Value>) -> Option<Value> {
    let p = run.join("crash_triage.json");
    if let Some(v) = util::read_json(&p) {
        return Some(v);
    }
    let fixture = cfg["fixture"].as_bool().unwrap_or(false);
    let start = cfg["started_wall_ms"].as_i64();
    if fixture || fin.is_none() || start.is_none() {
        return None;
    }
    let exe = tools_exe()?;
    let until = fin.and_then(|f| f["wall_ms"].as_i64()).unwrap_or_else(util::now_ms);
    let mut cmd = std::process::Command::new(exe);
    cmd.args(["crash-triage", "--since", &start.unwrap().to_string(), "--until", &until.to_string(), "--no-state-update", "--quiet", "--out"])
        .arg(&p);
    if let Some(d) = cfg["crashes_dir"].as_str() {
        cmd.arg("--dir").arg(d);
    }
    let _ = cmd.stdin(std::process::Stdio::null()).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null()).status();
    util::read_json(&p)
}

/// The triage report's new crashes as "SIG thread time dir".
pub fn triage_new(t: &Value) -> Vec<String> {
    t["crashes"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|c| c["new"].as_bool() == Some(true))
        .map(|c| {
            format!(
                "{} [{}] {} {}",
                c["signature"]["id"].as_str().unwrap_or("?"),
                c["thread"].as_str().filter(|x| !x.is_empty()).unwrap_or("?"),
                c["time"].as_str().unwrap_or("?"),
                c["dir"].as_str().unwrap_or("?")
            )
        })
        .collect()
}

pub fn triage_window(t: &Value) -> String {
    format!("{}..{}", t["since"].as_str().unwrap_or("?"), t["until"].as_str().unwrap_or("end"))
}

pub fn crash(x: &SoakCtx) -> Out {
    let mut o: Out = vec![];
    let max = lim(x.sc, "crashes_max", 0.0) as usize;
    match x.triage {
        None => o.push((INCOMPLETE, "crash_triage.json missing (hsmp-tools crash-triage not run for the run window)".into(), None)),
        Some(t) => {
            let new = triage_new(t);
            let st = if new.len() <= max { PASS } else { FAIL };
            let list = if new.is_empty() { "none".to_string() } else { new.join("; ") };
            o.push((st, format!("crash-triage {}: {} new dump(s) (max {max}): {list}", triage_window(t), new.len()), None));
        }
    }
    match x.fin {
        None => o.push((INCOMPLETE, "final.json missing (liveness at quit_all not captured)".into(), None)),
        Some(f) => {
            let alive = f["alive_at_end"].as_object();
            for i in x.insts {
                let role = format!("game{i}");
                match alive.and_then(|a| a.get(&role)) {
                    Some(Value::Bool(true)) => o.push((PASS, format!("{role} alive at quit_all"), Some(i.clone()))),
                    Some(_) => o.push((FAIL, format!("{role} was dead before quit_all"), Some(i.clone()))),
                    None => o.push((INCOMPLETE, format!("{role} liveness not recorded"), Some(i.clone()))),
                }
            }
        }
    }
    o
}

// ------------------------------------------------------------------------------------------
// SOAK-MEM
// ------------------------------------------------------------------------------------------

/// Least-squares slope (y per x unit) and R^2.
pub fn linfit(pts: &[(f64, f64)]) -> (f64, f64) {
    let n = pts.len() as f64;
    if pts.len() < 2 {
        return (0.0, 0.0);
    }
    let mx = pts.iter().map(|p| p.0).sum::<f64>() / n;
    let my = pts.iter().map(|p| p.1).sum::<f64>() / n;
    let sxx: f64 = pts.iter().map(|p| (p.0 - mx).powi(2)).sum();
    let sxy: f64 = pts.iter().map(|p| (p.0 - mx) * (p.1 - my)).sum();
    let syy: f64 = pts.iter().map(|p| (p.1 - my).powi(2)).sum();
    if sxx == 0.0 {
        return (0.0, 0.0);
    }
    let slope = sxy / sxx;
    let r2 = if syy == 0.0 { 1.0 } else { (sxy * sxy) / (sxx * syy) };
    (slope, r2)
}

fn median(mut v: Vec<f64>) -> f64 {
    v.sort_by(|a, b| a.partial_cmp(b).unwrap());
    if v.is_empty() {
        0.0
    } else {
        v[v.len() / 2]
    }
}

#[derive(Clone, Debug)]
pub struct MemRow {
    pub t: i64,
    pub private: f64,
    pub ws: f64,
    pub handles: f64,
}

pub fn read_mem(run: &Path) -> Option<BTreeMap<String, Vec<MemRow>>> {
    let text = util::read_text(&run.join("mem.jsonl"))?;
    let mut by: BTreeMap<String, Vec<MemRow>> = BTreeMap::new();
    for l in text.lines() {
        let Ok(Value::Object(e)) = serde_json::from_str::<Value>(l.trim()) else { continue };
        if ev_name(&e) != "mem" {
            continue;
        }
        let (Some(role), Some(p)) = (s(&e, "role"), f64v(&e, "private_bytes")) else { continue };
        by.entry(role.to_string()).or_default().push(MemRow {
            t: wall(&e),
            private: p,
            ws: f64v(&e, "working_set").unwrap_or(0.0),
            handles: f64v(&e, "handles").unwrap_or(0.0),
        });
    }
    for v in by.values_mut() {
        v.sort_by_key(|r| r.t);
    }
    Some(by)
}

pub fn mem(x: &SoakCtx) -> Out {
    let mut o: Out = vec![];
    let Some(by) = read_mem(x.run) else {
        o.push((INCOMPLETE, "mem.jsonl missing (hsmp-gate memwatch did not run)".into(), None));
        return o;
    };
    let warm = (lim(x.sc, "mem_warmup_min", 10.0) * 60_000.0) as i64;
    let min_n = lim(x.sc, "mem_min_samples", 6.0) as usize;
    let slope_max = lim(x.sc, "mem_slope_max_mb_per_h", 150.0);
    let growth_max = lim(x.sc, "mem_growth_max_mb", 400.0);
    let hslope_max = lim(x.sc, "handle_slope_max_per_h", 500.0);
    let aux_max = lim(x.sc, "aux_mem_slope_max_mb_per_h", 20.0);
    let hours = |r: &MemRow, t0: i64| (r.t - t0) as f64 / 3_600_000.0;
    for i in x.insts {
        let role = format!("game{i}");
        let Some(rows) = by.get(&role).filter(|r| !r.is_empty()) else {
            o.push((INCOMPLETE, format!("{role}: no memory samples"), Some(i.clone())));
            continue;
        };
        let t0 = rows[0].t;
        let post: Vec<&MemRow> = rows.iter().filter(|r| r.t >= t0 + warm).collect();
        if post.len() < min_n {
            o.push((INCOMPLETE, format!("{role}: {} samples after the {} min warm-up (need {min_n}); run too short?", post.len(), warm / 60_000), Some(i.clone())));
            continue;
        }
        let pts: Vec<(f64, f64)> = post.iter().map(|r| (hours(r, t0), r.private / MB)).collect();
        let (slope, r2) = linfit(&pts);
        let base = median(post.iter().take(3).map(|r| r.private / MB).collect());
        let peak = post.iter().map(|r| r.private / MB).fold(f64::MIN, f64::max);
        let growth = peak - base;
        let span_min = (post.last().unwrap().t - post[0].t) as f64 / 60_000.0;
        let ws_end = post.last().unwrap().ws / MB;
        // slope strictly below the limit, growth at most the limit
        let ok = slope < slope_max && growth <= growth_max;
        o.push((
            if ok { PASS } else { FAIL },
            format!(
                "{role}: private bytes slope {slope:+.1} MB/h (R^2 {r2:.2}, limit {slope_max}) over {span_min:.1} min after warm-up; start {:.0} MB, end {:.0} MB, peak {peak:.0} MB, growth {growth:+.0} MB (limit {growth_max}); working set end {ws_end:.0} MB",
                pts[0].1,
                pts.last().unwrap().1
            ),
            Some(i.clone()),
        ));
        if post.iter().any(|r| r.handles > 0.0) {
            let hp: Vec<(f64, f64)> = post.iter().map(|r| (hours(r, t0), r.handles)).collect();
            let (hs, _) = linfit(&hp);
            o.push((
                if hs <= hslope_max { PASS } else { FAIL },
                format!("{role}: handle slope {hs:+.0}/h (limit {hslope_max}), {} -> {} handles", hp[0].1, hp.last().unwrap().1),
                Some(i.clone()),
            ));
        }
    }
    for (role, rows) in &by {
        if role.starts_with("game") || rows.is_empty() {
            continue;
        }
        let t0 = rows[0].t;
        let post: Vec<&MemRow> = rows.iter().filter(|r| r.t >= t0 + warm).collect();
        if post.len() < min_n {
            continue; // aux processes found late (sidecars) may not have enough samples: not judged
        }
        let pts: Vec<(f64, f64)> = post.iter().map(|r| (hours(r, t0), r.private / MB)).collect();
        let (slope, _) = linfit(&pts);
        o.push((
            if slope <= aux_max { PASS } else { FAIL },
            format!("{role}: private bytes slope {slope:+.1} MB/h (limit {aux_max}), {:.1} -> {:.1} MB", pts[0].1, pts.last().unwrap().1),
            None,
        ));
    }
    o
}

// ------------------------------------------------------------------------------------------
// SOAK-LUAERR
// ------------------------------------------------------------------------------------------

pub const LUA_ERROR_RX: &str = r"(?i)\bLUA_ERR(RUN|SYNTAX|MEM|ERR|FILE|GCMM)?\b|attempt to (index|call|compare|perform|concatenate|get length|divide|modulo|yield)|stack traceback|\.lua:\d+:|bad argument #\d+";

pub fn lua_error_lines(text: &str, allow: &[Regex]) -> Vec<String> {
    let rx = Regex::new(LUA_ERROR_RX).unwrap();
    text.lines()
        .filter(|l| !l.contains("[hsmp_ev]") && rx.is_match(l) && !allow.iter().any(|a| a.is_match(l)))
        .map(|l| l.trim().chars().take(240).collect())
        .collect()
}

pub fn luaerr(x: &SoakCtx) -> Out {
    let mut o: Out = vec![];
    let max = lim(x.sc, "lua_errors_max", 0.0) as usize;
    let allow: Vec<Regex> = x.sc["soak"]["lua_error_allow"].as_array().into_iter().flatten().filter_map(|v| v.as_str()).filter_map(|r| Regex::new(r).ok()).collect();
    let evs: Vec<String> = x.evs.iter().filter(|e| ev_name(e) == "lua_error")
        .map(|e| format!("inst {} {}: {}", inst(e), sv(e, "mod").unwrap_or_default(), sv(e, "error").or_else(|| sv(e, "msg")).unwrap_or_default()))
        .collect();
    match util::read_text(&x.run.join("UE4SS.log")) {
        None => o.push((INCOMPLETE, "UE4SS.log not collected".into(), None)),
        Some(text) => {
            let lines = lua_error_lines(&text, &allow);
            let st = if lines.len() <= max { PASS } else { FAIL };
            let shown = if lines.is_empty() { "none".to_string() } else { lines.iter().take(5).cloned().collect::<Vec<_>>().join(" | ") };
            o.push((st, format!("UE4SS.log Lua errors: {} (max {max}): {shown}", lines.len()), None));
        }
    }
    o.push((if evs.len() <= max { PASS } else { FAIL }, format!("hsmp_log lua_error events: {} {}", evs.len(), evs.iter().take(5).cloned().collect::<Vec<_>>().join(" | ")), None));
    o
}

// ------------------------------------------------------------------------------------------
// SOAK-HITCH
// ------------------------------------------------------------------------------------------

fn is_client(e: &Ev) -> bool {
    matches!(util::src(e), "inst" | "ue4ss") && !matches!(inst(e), "server" | "harness")
}

/// A travel window never extends past this (a menu travel has no world_ready).
pub const TRAVEL_WINDOW_MAX_MS: i64 = 30_000;

/// Travel windows of one instance: [travel, the next world_ready], at most TRAVEL_WINDOW_MAX_MS.
fn travel_windows(evs: &[Ev], i: &str) -> Vec<(i64, i64)> {
    let mut w = vec![];
    let mine: Vec<&Ev> = evs.iter().filter(|e| is_client(e) && inst(e) == i && matches!(ev_name(e), "travel" | "world_ready")).collect();
    for (k, e) in mine.iter().enumerate() {
        if ev_name(e) == "travel" {
            let cap = wall(e) + TRAVEL_WINDOW_MAX_MS;
            let end = mine[k + 1..].iter().find(|x| ev_name(x) == "world_ready").map(|x| wall(x)).unwrap_or(cap).min(cap);
            w.push((wall(e), end));
        }
    }
    w
}

/// One instance's hitch events: (all ms values, stalls > max_ms outside travel, stalls > max_ms
/// during travel). The emitter's `travel` flag decides (contract: travel=false > 2 s fails);
/// only a hitch WITHOUT the flag falls back to the [travel, world_ready] windows.
pub fn hitch_scan(evs: &[Ev], i: &str, max_ms: f64) -> (Vec<f64>, Vec<String>, usize) {
    let wins = travel_windows(evs, i);
    let mut all = vec![];
    let mut bad = vec![];
    let mut travel_n = 0;
    for h in evs.iter().filter(|e| is_client(e) && inst(e) == i && ev_name(e) == "hitch") {
        let ms = f64v(h, "ms").unwrap_or(f64::INFINITY);
        all.push(ms);
        if ms <= max_ms {
            continue;
        }
        let (lo, hi) = (wall(h) - ms.min(1e9) as i64, wall(h));
        let travel = match bv(h, "travel") {
            Some(t) => t,
            None => wins.iter().any(|(a, b)| lo <= *b && hi >= *a),
        };
        if travel {
            travel_n += 1;
        } else {
            let wk = sv(h, "world_key").map(|k| format!(" in {k}")).unwrap_or_default();
            bad.push(format!("{ms:.0} ms at {}{wk}", util::ms_to_iso(wall(h))));
        }
    }
    (all, bad, travel_n)
}

/// Heartbeat period of the stall probe (`frame_hb` every 10 s) and the largest gap that is
/// still not a stall (period + slack for the 1 s wall-clock granularity of hsmp_log).
pub const HB_PERIOD_MS: i64 = 10_000;
pub const HB_GAP_MAX_MS: i64 = 3 * HB_PERIOD_MS + 5_000;

/// The stall verdict for one instance (DoD-2 and SOAK-HITCH share it).
///
/// * no `frame_hb` at all: incomplete (the probe is not running; a missing hitch is no evidence);
/// * a `hitch` with ms > max_ms and travel=false (or no flag and outside every travel window): fail;
/// * a `frame_hb` whose `max_ms` > max_ms while its 10 s window overlaps no travel window: fail
///   (the probe saw the stall; the hitch event is missing or was lost);
/// * a gap between heartbeats, or between the instance's first/last event and its first/last
///   heartbeat, longer than HB_GAP_MAX_MS outside travel: fail (the game thread did not run the
///   probe loop: a hang the hitch event can never report).
pub fn stall_check(evs: &[Ev], i: &str, max_ms: f64) -> (&'static str, String) {
    let hb: Vec<&Ev> = evs.iter().filter(|e| is_client(e) && inst(e) == i && ev_name(e) == "frame_hb").collect();
    let (mut all, mut bad, travel_n) = hitch_scan(evs, i, max_ms);
    if hb.is_empty() {
        let extra = if bad.is_empty() { String::new() } else { format!("; but {} stall(s) > {max_ms:.0} ms outside travel: {}", bad.len(), bad.join(", ")) };
        let st = if bad.is_empty() { INCOMPLETE } else { FAIL };
        return (st, format!("inst {i}: no frame_hb heartbeat (stall probe not running; contract: docs/development/testing.md \"Stall probe\"){extra}"));
    }
    let wins = travel_windows(evs, i);
    let overlaps = |lo: i64, hi: i64| wins.iter().any(|(a, b)| lo <= *b && hi >= *a);
    let mut worst_frame = 0.0f64;
    for h in &hb {
        let m = f64v(h, "max_ms").unwrap_or(0.0);
        worst_frame = worst_frame.max(m);
        if m > max_ms && !overlaps(wall(h) - HB_PERIOD_MS, wall(h)) {
            let covered = evs.iter().any(|e| is_client(e) && inst(e) == i && ev_name(e) == "hitch"
                && (wall(h) - HB_PERIOD_MS..=wall(h) + 1000).contains(&wall(e)));
            if !covered {
                bad.push(format!("frame_hb max_ms {m:.0} at {} with no hitch event", util::ms_to_iso(wall(h))));
            }
        }
    }
    // heartbeat coverage: from the instance's first to its last event
    let mine: Vec<i64> = evs.iter().filter(|e| is_client(e) && inst(e) == i && wall(e) > 0).map(wall).collect();
    let (first, last) = (mine.iter().copied().min().unwrap_or(0), mine.iter().copied().max().unwrap_or(0));
    let mut ts: Vec<i64> = hb.iter().map(|h| wall(h)).collect();
    ts.sort();
    let mut pts = vec![first];
    pts.extend(ts.iter().copied());
    pts.push(last);
    let mut gaps = 0;
    // time of [a, b] not covered by any travel window (a level load may silence the probe)
    let uncovered = |a: i64, b: i64| -> i64 {
        let mut cut: Vec<(i64, i64)> = wins.iter().map(|(x, y)| ((*x).max(a), (*y).min(b))).filter(|(x, y)| x < y).collect();
        cut.sort();
        let (mut covered, mut end) = (0i64, a);
        for (x, y) in cut {
            let x = x.max(end);
            if y > x {
                covered += y - x;
                end = y;
            }
        }
        (b - a) - covered
    };
    for w in pts.windows(2) {
        let (a, b) = (w[0], w[1]);
        if uncovered(a, b) > HB_GAP_MAX_MS {
            gaps += 1;
            bad.push(format!("no frame_hb for {:.0} s from {}", (b - a) as f64 / 1000.0, util::ms_to_iso(a)));
        }
    }
    all.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let st = if bad.is_empty() { PASS } else { FAIL };
    (st, format!(
        "inst {i}: {} heartbeats (worst frame {worst_frame:.0} ms), {} hitch event(s); stalls > {max_ms:.0} ms outside travel: {}{}{}; during travel (not failing): {travel_n}",
        hb.len(),
        all.len(),
        bad.len(),
        if gaps > 0 { format!(" ({gaps} heartbeat gap(s))") } else { String::new() },
        if bad.is_empty() { String::new() } else { format!(": {}", bad.iter().take(5).cloned().collect::<Vec<_>>().join(", ")) },
    ))
}

pub fn hitch(x: &SoakCtx) -> Out {
    let max_ms = lim(x.sc, "hitch_max_ms", 2000.0);
    x.insts
        .iter()
        .map(|i| {
            let (st, msg) = stall_check(x.evs, i, max_ms);
            (st, msg, Some(i.clone()))
        })
        .collect()
}

/// A clean or failing crash_triage.json for fixtures (same shape as hsmp-tools crash-triage).
pub fn fixture_triage(since: i64, until: i64, new: &[(&str, &str, &str, i64)]) -> Value {
    let crashes: Vec<Value> = new
        .iter()
        .map(|(dir, sig, thread, t)| json!({"dir": dir, "time": util::ms_to_iso(*t), "time_ms": t, "new": true, "thread": thread,
                                            "signature": {"id": sig, "title": "fixture", "confidence": "high"}}))
        .collect();
    let n = crashes.len();
    json!({"tool": "hsmp-tools crash-triage", "version": 1, "since": util::ms_to_iso(since), "until": util::ms_to_iso(until),
           "crashes": crashes, "summary": {"total": n, "triaged": n, "new": n}})
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fit() {
        let pts: Vec<(f64, f64)> = (0..10).map(|k| (k as f64, 3.0 * k as f64 + 1.0)).collect();
        let (s, r2) = linfit(&pts);
        assert!((s - 3.0).abs() < 1e-9 && (r2 - 1.0).abs() < 1e-9);
        assert_eq!(linfit(&[(1.0, 2.0)]), (0.0, 0.0));
        assert_eq!(median(vec![3.0, 1.0, 2.0]), 2.0);
    }

    #[test]
    fn lua_error_patterns() {
        let log = "\
[2026-10-02 00:25:33.1] [Lua] [HSMPMatch] director ERROR spawn_timeout: not placed\n\
[2026-10-02 00:25:34.1] [Lua] Error: ...Mods/HSMPLoadout/Scripts/main.lua:285: attempt to index a nil value (local 'c')\n\
[2026-10-02 00:25:34.2] stack traceback:\n\
[2026-10-02 00:25:35.0] [Lua] [hsmp_ev] {\"ev\":\"x\",\"msg\":\"attempt to index\"}\n\
[2026-10-02 00:25:36.0] FArchiveState::ArIsError = 0x29\n\
[2026-10-02 00:25:37.0] LUA_ERRRUN in callback\n\
[2026-10-02 00:25:38.0] [Lua] bad argument #1 to 'ipairs' (table expected, got nil)\n";
        let l = lua_error_lines(log, &[]);
        assert_eq!(l.len(), 4, "{l:?}");
        let allow = vec![Regex::new("LUA_ERRRUN").unwrap()];
        assert_eq!(lua_error_lines(log, &allow).len(), 3);
    }
}
