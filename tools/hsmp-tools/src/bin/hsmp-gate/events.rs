//! Evidence collection: every source in a run dir normalised into one sorted event list.
//!
//! Each event gets `src` (inst | ue4ss | server | serverlog | sidecar | harness | observed),
//! `inst` ("1".., "server", "harness") and `wall_ms`.

use crate::util::{self, Ev};
use regex::Regex;
use serde_json::{json, Value};
use std::collections::HashSet;
use std::path::{Path, PathBuf};

/// The per-instance sidecar tap (`--ipc-tap`): the only source of the sidecar's view of the
/// game link (command answers, session snapshots).
pub const TAP_FILE: &str = "ipc_tap.jsonl";

pub fn normalise(mut e: Ev, src: &str, inst: Option<&str>) -> Ev {
    e.insert("src".into(), json!(src));
    let cur = util::sv(&e, "inst");
    let i = match (cur, inst) {
        (Some(c), _) => c,
        (None, Some(x)) => x.to_string(),
        (None, None) => "?".into(),
    };
    e.insert("inst".into(), json!(i));
    if !e.contains_key("wall_ms") {
        let w = if let Some(ts) = util::s(&e, "ts") {
            util::iso_to_ms(ts).unwrap_or(0)
        } else if let Some(t) = util::i64v(&e, "time_ms") {
            t
        } else if src != "inst" {
            util::i64v(&e, "t_ms").unwrap_or(0)
        } else {
            0
        };
        e.insert("wall_ms".into(), json!(w));
    }
    if !e.contains_key("ev") {
        if let Some(x) = e.get("event").cloned() {
            e.insert("ev".into(), x);
        }
    }
    // The session protocol names the end-of-match phase MatchOver; the gate's vocabulary says PostMatch
    if util::ev_name(&e) == "phase" {
        for k in ["from", "to"] {
            if util::s(&e, k) == Some("MatchOver") {
                e.insert(k.into(), json!("PostMatch"));
            }
        }
    }
    e
}

pub fn read_jsonl(p: &Path, src: &str, inst: Option<&str>) -> Vec<Ev> {
    let Some(text) = util::read_text(p) else { return vec![] };
    let mut out = vec![];
    for (n, line) in text.lines().enumerate() {
        let l = line.trim();
        if l.is_empty() {
            continue;
        }
        match serde_json::from_str::<Value>(l) {
            Ok(Value::Object(o)) => out.push(normalise(o, src, inst)),
            Ok(_) => {}
            Err(_) => {
                let mut e = Ev::new();
                e.insert("ev".into(), json!("_torn_line"));
                e.insert("line".into(), json!(n + 1));
                e.insert("file".into(), json!(p.display().to_string()));
                e.insert("wall_ms".into(), json!(0));
                out.push(normalise(e, src, inst));
            }
        }
    }
    out
}

struct LogRule {
    rx: Regex,
    ev: &'static str,
    to: Option<&'static str>,
}

fn serverlog_rules() -> Vec<LogRule> {
    let r = |p: &str, ev: &'static str, to: Option<&'static str>| LogRule { rx: Regex::new(p).unwrap(), ev, to };
    vec![
        r(r"match start: arena locked", "phase", Some("Loading")),
        r(r"match: countdown begins", "phase", Some("Loading")),
        r(r"match: participant back; replaying", "phase", Some("Loading")),
        r(r"match: all clients loaded; countdown running", "phase", Some("Countdown")),
        r(r"match: live\b", "phase", Some("Live")),
        r(r"match: round over", "phase", Some("RoundOver")),
        r(r"match: round ends", "phase", Some("RoundOver")),
        r(r"match: participant dropped; pausing", "phase", Some("Paused")),
        r(r"match over by forfeit", "phase", Some("PostMatch")),
        r(r"back to lobby", "phase", Some("Lobby")),
        r(r"match: reconnected participant restored", "seat_restored", None),
        r(r"peer joined", "peer_joined", None),
        r(r"peer left", "peer_left", None),
        r(r"arena picked by host", "arena_picked", None),
        r(r"lobby arena \(startup\)", "arena_picked", None),
        r(r"load barrier timed out", "load_timeout", None),
        r(r"hsmp-server starting", "server_start", None),
    ]
}

/// hsmp-server stdout (tracing fmt, ANSI or not) -> server events. One epoch per server start.
pub fn parse_server_log(p: &Path, epoch_base: i64) -> Vec<Ev> {
    let Some(text) = util::read_text(p) else { return vec![] };
    let ansi = Regex::new(r"\x1b\[[0-9;]*m").unwrap();
    let head = Regex::new(r"^(\d{4}-\d\d-\d\dT\d\d:\d\d:\d\d(?:\.\d+)?)Z?\s+(TRACE|DEBUG|INFO|WARN|ERROR)\s+([\w:]+):\s*(.*)$").unwrap();
    let kv = Regex::new(r#"(\w+)=("[^"]*"|\S+)"#).unwrap();
    let rules = serverlog_rules();
    let mut out = vec![];
    let mut last_phase: Option<String> = None;
    let mut epoch = epoch_base;
    for raw in text.lines() {
        let line = ansi.replace_all(raw, "");
        let Some(c) = head.captures(&line) else { continue };
        let wall = util::iso_to_ms(&c[1]).unwrap_or(0);
        let msg = c[4].to_string();
        let mut fields = serde_json::Map::new();
        for f in kv.captures_iter(&msg) {
            let v = f[2].trim_matches('"').to_string();
            let val = v.parse::<i64>().map(|n| json!(n)).unwrap_or(json!(v));
            fields.insert(f[1].to_string(), val);
        }
        for r in &rules {
            if !r.rx.is_match(&msg) {
                continue;
            }
            let mut e = Ev::new();
            e.insert("ev".into(), json!(r.ev));
            e.insert("src".into(), json!("serverlog"));
            e.insert("inst".into(), json!("server"));
            e.insert("wall_ms".into(), json!(wall));
            for k in ["round", "arena", "peer_id", "nick", "epoch"] {
                if let Some(v) = fields.get(k) {
                    e.insert(k.into(), v.clone());
                }
            }
            if r.ev == "server_start" {
                epoch += 1;
                last_phase = None;
            }
            e.insert("epoch".into(), json!(epoch));
            if let Some(to) = r.to {
                e.insert("from".into(), json!(last_phase));
                e.insert("to".into(), json!(to));
                last_phase = Some(to.to_string());
            }
            out.push(e);
            break;
        }
        if fields.get("match_over").and_then(|v| v.as_str()) == Some("true") {
            let mut e = Ev::new();
            e.insert("ev".into(), json!("phase"));
            e.insert("src".into(), json!("serverlog"));
            e.insert("inst".into(), json!("server"));
            e.insert("wall_ms".into(), json!(wall));
            e.insert("from".into(), json!(last_phase));
            e.insert("to".into(), json!("PostMatch"));
            e.insert("epoch".into(), json!(epoch));
            last_phase = Some("PostMatch".into());
            out.push(e);
        }
    }
    out
}

/// "[hsmp_ev] {json}" lines from the shared UE4SS.log.
pub fn parse_ue4ss_log(p: &Path) -> Vec<Ev> {
    let Some(text) = util::read_text(p) else { return vec![] };
    let mut out = vec![];
    for line in text.lines() {
        let Some(pos) = line.find("[hsmp_ev]") else { continue };
        let rest = line[pos + 9..].trim();
        if let Ok(Value::Object(o)) = serde_json::from_str::<Value>(rest) {
            out.push(normalise(o, "ue4ss", None));
        }
    }
    out
}

pub fn load_run(run: &Path) -> Value {
    util::read_json(&run.join("run.json")).unwrap_or(json!({}))
}

fn rotated_order(p: &Path) -> i64 {
    let n = p.file_name().and_then(|x| x.to_str()).unwrap_or("");
    let re = Regex::new(r"hsmp_events\.(\d+)\.jsonl$").unwrap();
    re.captures(n).map(|c| -c[1].parse::<i64>().unwrap_or(0)).unwrap_or(0)
}

fn glob_files(dir: &Path, prefix: &str, suffix: &str) -> Vec<PathBuf> {
    let mut v: Vec<PathBuf> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .filter(|p| {
            p.is_file()
                && p.file_name().and_then(|n| n.to_str()).map(|n| n.starts_with(prefix) && n.ends_with(suffix)).unwrap_or(false)
        })
        .collect();
    v.sort();
    v
}

/// hsmp_events*.jsonl for instance i: collected copy (run/inst<i>/) first, else the live state dir.
pub fn inst_event_files(run: &Path, cfg: &Value, i: usize) -> Vec<PathBuf> {
    let mut dirs = vec![run.join(format!("inst{i}"))];
    if let Some(d) = cfg["state_dirs"].get(i.to_string()).and_then(|v| v.as_str()) {
        dirs.push(PathBuf::from(d));
    }
    for d in dirs {
        if !d.is_dir() {
            continue;
        }
        let mut files = glob_files(&d, "hsmp_events", ".jsonl");
        if !files.is_empty() {
            files.sort_by_key(|p| rotated_order(p));
            return files;
        }
    }
    vec![]
}

pub fn gather(run: &Path, cfg: &Value) -> Vec<Ev> {
    let n = cfg["instances"].as_u64().unwrap_or(0) as usize;
    let mut evs: Vec<Ev> = vec![];
    let mut have_inst: HashSet<String> = HashSet::new();
    if n > 0 {
        for i in 1..=n {
            let is = i.to_string();
            for fp in inst_event_files(run, cfg, i) {
                let mut got = read_jsonl(&fp, "inst", Some(&is));
                for e in got.iter_mut() {
                    if matches!(util::inst(e), "0" | "?") {
                        e.insert("inst".into(), json!(is));
                    }
                }
                if !got.is_empty() {
                    have_inst.insert(is.clone());
                }
                evs.extend(got);
            }
        }
    } else {
        for entry in std::fs::read_dir(run).into_iter().flatten().flatten() {
            let p = entry.path();
            let name = p.file_name().and_then(|x| x.to_str()).unwrap_or("").to_string();
            if p.is_dir() && name.starts_with("inst") {
                let i: String = name.chars().filter(|c| c.is_ascii_digit()).collect();
                for fp in glob_files(&p, "hsmp_events", ".jsonl") {
                    let got = read_jsonl(&fp, "inst", Some(&i));
                    if !got.is_empty() {
                        have_inst.insert(i.clone());
                    }
                    evs.extend(got);
                }
            }
        }
    }
    for e in parse_ue4ss_log(&run.join("UE4SS.log")) {
        if !have_inst.contains(util::inst(&e)) {
            evs.push(e);
        }
    }
    let mut server_evs = read_jsonl(&run.join("server.jsonl"), "server", Some("server"));
    for e in server_evs.iter_mut() {
        e.insert("inst".into(), json!("server"));
    }
    if !server_evs.iter().any(|e| util::ev_name(e) == "phase") {
        // server.log, then server.2.log ... after harness restarts (one epoch per file)
        let mut logs: Vec<(i64, PathBuf)> = glob_files(run, "server", ".log")
            .into_iter()
            .map(|p| {
                let n = p.file_name().and_then(|x| x.to_str()).unwrap_or("").to_string();
                let k = Regex::new(r"^server\.(\d+)\.log$").unwrap().captures(&n).map(|c| c[1].parse().unwrap_or(1)).unwrap_or(1);
                (k, p)
            })
            .collect();
        logs.sort();
        for (k, (_, lp)) in logs.iter().enumerate() {
            server_evs.extend(parse_server_log(lp, k as i64));
        }
    }
    evs.extend(server_evs);
    for fp in glob_files(run, "sidecar", ".jsonl") {
        let n = fp.file_name().and_then(|x| x.to_str()).unwrap_or("");
        let i: String = n.chars().filter(|c| c.is_ascii_digit()).collect();
        evs.extend(read_jsonl(&fp, "sidecar", Some(&i)));
    }
    // The server's command answers as each sidecar received them: S2G cmd_result records in
    // the sidecar's tap (<run>/inst<N>/ipc_tap.jsonl, docs/development/ipc-shared-memory.md;
    // one record per fresh S2CCommandResult) -> src=sidecar ev=cmd_result per instance
    // (DoD-10). A `_collected` marker records that the tap existed.
    let insts: Vec<usize> = if n > 0 {
        (1..=n).collect()
    } else {
        std::fs::read_dir(run).into_iter().flatten().flatten()
            .filter_map(|e| e.file_name().to_str().and_then(|x| x.strip_prefix("inst")).and_then(|d| d.parse().ok()))
            .collect()
    };
    for i in insts {
        let is = i.to_string();
        // the sidecar's career file guard actions (<state>/.career_guard.jsonl, DoD-8; a log)
        let mut cg = vec![run.join(format!("inst{i}")).join(".career_guard.jsonl")];
        if let Some(d) = cfg["state_dirs"].get(&is).and_then(|v| v.as_str()) {
            cg.push(PathBuf::from(d).join(".career_guard.jsonl"));
        }
        if let Some(p) = cg.into_iter().find(|p| p.is_file()) {
            for mut e in read_jsonl(&p, "sidecar", Some(&is)) {
                e.insert("inst".into(), json!(is));
                evs.push(e);
            }
        }
        let tap = crate::observe::tap_path(run, &cfg, &is);
        if tap.is_file() {
            let mut mark = Ev::new();
            mark.insert("ev".into(), json!("_collected"));
            mark.insert("file".into(), json!(TAP_FILE));
            mark.insert("wall_ms".into(), json!(0));
            evs.push(normalise(mark, "sidecar", Some(&is)));
            for line in std::fs::read_to_string(&tap).unwrap_or_default().lines() {
                let Ok(serde_json::Value::Object(t)) = serde_json::from_str::<serde_json::Value>(line) else { continue };
                if t.get("ev").and_then(|v| v.as_str()) != Some("s2g") || t.get("kind").and_then(|v| v.as_str()) != Some("cmd_result") {
                    continue;
                }
                let mut e: Ev = t.get("v").and_then(|v| v.as_object()).cloned().unwrap_or_default().into_iter().collect();
                e.insert("ev".into(), json!("cmd_result"));
                e.insert("file".into(), json!(TAP_FILE));
                e.entry("wall_ms".to_string()).or_insert_with(|| t.get("t").cloned().unwrap_or(json!(0)));
                let mut e = normalise(e, "sidecar", Some(&is));
                e.insert("inst".into(), json!(is));
                evs.push(e);
            }
        }
    }
    // hsmp-tools netsim --events (netsim<i>.jsonl): netsim_start{profile,...}, netsim_mode, ...
    for fp in glob_files(run, "netsim", ".jsonl") {
        let n = fp.file_name().and_then(|x| x.to_str()).unwrap_or("");
        let i: String = n.chars().filter(|c| c.is_ascii_digit()).collect();
        for mut e in read_jsonl(&fp, "netsim", Some(&i)) {
            e.insert("inst".into(), json!(i));
            evs.push(e);
        }
    }
    evs.extend(read_jsonl(&run.join("harness.jsonl"), "harness", Some("harness")));
    evs.extend(read_jsonl(&run.join("observed.jsonl"), "observed", None));
    evs.sort_by_key(|e| (util::wall(e), util::i64v(e, "seq").unwrap_or(0)));
    evs
}
