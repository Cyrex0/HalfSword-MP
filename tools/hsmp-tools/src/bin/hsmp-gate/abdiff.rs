//! `hsmp-gate ab-diff --a <run> --b <run>`: compare two gate runs of the same scenario (a
//! baseline and a candidate build). Metrics per run:
//! * `frame_cost_ms`: HSMPAvatars "frame cost: X ms per stand-in frame" (UE4SS.log), mean / p95;
//! * `pose_peer_frame_ms`: HSMPAvatars "pose peer ...| frame cost avg X ms", mean;
//! * `pose_sender_ms`: HSMPSync "pose sample+write avg X ms", mean / p95;
//! * `pose_quality`: arm_p95_uu, tip_p95_uu, latency_ms (event means);
//! * `hitches`: count of `hitch` events and their max ms;
//! * `ipc_stats`: the last `ipc_stats` event per instance and the tap's last `stats` line.
//!
//! Prints a table with the delta (b - a) and writes `--json` if asked. Exit 0 always, 2 when
//! a run dir is unreadable: the verdict is a human / gate decision.

use crate::events;
use crate::util;
use regex::Regex;
use serde_json::{json, Map, Value};
use std::path::Path;

fn stats(v: &[f64]) -> Value {
    if v.is_empty() {
        return Value::Null;
    }
    let mut s = v.to_vec();
    s.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let mean = s.iter().sum::<f64>() / s.len() as f64;
    let p95 = s[((s.len() as f64 * 0.95).ceil() as usize).saturating_sub(1).min(s.len() - 1)];
    json!({"n": s.len(), "mean": (mean * 1e4).round() / 1e4, "p95": (p95 * 1e4).round() / 1e4, "max": s[s.len() - 1]})
}

fn log_values(text: &str, re: &Regex) -> Vec<f64> {
    text.lines().filter_map(|l| re.captures(l)).filter_map(|c| c[1].parse::<f64>().ok()).filter(|v| v.is_finite()).collect()
}

/// The metrics of one run dir.
pub fn metrics(run: &Path) -> anyhow::Result<Value> {
    if !run.is_dir() {
        anyhow::bail!("{} is not a run dir", run.display());
    }
    let mut logs = String::new();
    for p in [run.join("UE4SS.log")].into_iter().chain(
        std::fs::read_dir(run).into_iter().flatten().flatten().map(|e| e.path().join("UE4SS.log")),
    ) {
        if let Ok(t) = std::fs::read(&p) {
            logs.push_str(&String::from_utf8_lossy(&t));
        }
    }
    let frame = Regex::new(r"frame cost: ([0-9.]+) ms per stand-in frame").unwrap();
    let peer = Regex::new(r"pose peer \d+: play .*\| frame cost avg ([0-9.]+) ms").unwrap();
    let sender = Regex::new(r"pose sender: .*pose sample\+write avg ([0-9.]+) ms").unwrap();
    let cfg = events::load_run(run);
    let evs = events::gather(run, &cfg);
    let pick = |name: &str, k: &str| -> Vec<f64> {
        evs.iter().filter(|e| util::ev_name(e) == name).filter_map(|e| util::f64v(e, k)).collect()
    };
    let hitches = pick("hitch", "ms");
    let mut ipc = Map::new();
    for e in evs.iter().filter(|e| util::ev_name(e) == "ipc_stats") {
        ipc.insert(format!("inst{}", util::inst(e)), Value::Object(e.clone()));
    }
    for d in std::fs::read_dir(run).into_iter().flatten().flatten().map(|e| e.path()).filter(|p| p.is_dir()) {
        let tap = d.join("ipc_tap.jsonl");
        if let Ok(t) = std::fs::read_to_string(&tap) {
            if let Some(last) = t.lines().rev().filter_map(|l| serde_json::from_str::<Value>(l).ok()).find(|v| v["ev"] == "stats") {
                ipc.insert(format!("{}_tap", d.file_name().and_then(|n| n.to_str()).unwrap_or("?")), last);
            }
        }
    }
    Ok(json!({
        "run": run.display().to_string(),
        "ipc": cfg["ipc"].clone(),
        "frame_cost_ms": stats(&log_values(&logs, &frame)),
        "pose_peer_frame_ms": stats(&log_values(&logs, &peer)),
        "pose_sender_ms": stats(&log_values(&logs, &sender)),
        "pose_quality": {"arm_p95_uu": stats(&pick("pose_quality", "arm_p95_uu")), "tip_p95_uu": stats(&pick("pose_quality", "tip_p95_uu")),
                         "latency_ms": stats(&pick("pose_quality", "latency_ms"))},
        "hitches": {"count": hitches.len(), "max_ms": hitches.iter().cloned().fold(0.0, f64::max)},
        "ipc_stats": ipc,
    }))
}

fn mean(v: &Value) -> Option<f64> {
    v["mean"].as_f64()
}

pub fn run(a: &Path, b: &Path, json_out: Option<&Path>) -> i32 {
    let (ma, mb) = match (metrics(a), metrics(b)) {
        (Ok(x), Ok(y)) => (x, y),
        (Err(e), _) | (_, Err(e)) => {
            eprintln!("ab-diff: {e:#}");
            return 2;
        }
    };
    let rows: Vec<(&str, Option<f64>, Option<f64>, bool)> = vec![
        ("frame cost ms/stand-in frame (mean)", mean(&ma["frame_cost_ms"]), mean(&mb["frame_cost_ms"]), true),
        ("frame cost ms/stand-in frame (p95)", ma["frame_cost_ms"]["p95"].as_f64(), mb["frame_cost_ms"]["p95"].as_f64(), true),
        ("pose peer frame cost avg ms", mean(&ma["pose_peer_frame_ms"]), mean(&mb["pose_peer_frame_ms"]), true),
        ("pose sender sample+write ms (mean)", mean(&ma["pose_sender_ms"]), mean(&mb["pose_sender_ms"]), true),
        ("pose sender sample+write ms (p95)", ma["pose_sender_ms"]["p95"].as_f64(), mb["pose_sender_ms"]["p95"].as_f64(), true),
        ("pose_quality arm_p95_uu", mean(&ma["pose_quality"]["arm_p95_uu"]), mean(&mb["pose_quality"]["arm_p95_uu"]), true),
        ("pose_quality tip_p95_uu", mean(&ma["pose_quality"]["tip_p95_uu"]), mean(&mb["pose_quality"]["tip_p95_uu"]), true),
        ("pose_quality latency_ms", mean(&ma["pose_quality"]["latency_ms"]), mean(&mb["pose_quality"]["latency_ms"]), true),
        ("hitch events", ma["hitches"]["count"].as_f64(), mb["hitches"]["count"].as_f64(), true),
        ("hitch max ms", ma["hitches"]["max_ms"].as_f64(), mb["hitches"]["max_ms"].as_f64(), true),
    ];
    println!("A = {}  (ipc {})", a.display(), ma["ipc"]);
    println!("B = {}  (ipc {})", b.display(), mb["ipc"]);
    println!("{:42} {:>12} {:>12} {:>12}", "metric (lower is better)", "A", "B", "B - A");
    let f = |x: Option<f64>| x.map_or("-".to_string(), |v| format!("{v:.3}"));
    let mut table = Vec::new();
    for (name, x, y, _) in &rows {
        let d = match (x, y) {
            (Some(x), Some(y)) => Some(y - x),
            _ => None,
        };
        println!("{:42} {:>12} {:>12} {:>12}", name, f(*x), f(*y), d.map_or("-".into(), |v| format!("{v:+.3}")));
        table.push(json!({"metric": name, "a": x, "b": y, "delta": d}));
    }
    for (k, v) in mb["ipc_stats"].as_object().into_iter().flatten() {
        println!("B ipc_stats {k}: {v}");
    }
    if let Some(p) = json_out {
        let _ = util::write_json(p, &json!({"a": ma, "b": mb, "table": table}));
    }
    0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_log_metrics() {
        let d = std::env::temp_dir().join(format!("hsmp-ab-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        std::fs::write(d.join("UE4SS.log"), concat!(
            "[HSMPAvatars] frame cost: 1.250 ms per stand-in frame (3 drives, 0 deferred by the 4 ms budget) | ReceiveTick hook calls 3, driven 3\n",
            "[HSMPAvatars] frame cost: 0.750 ms per stand-in frame (3 drives, 0 deferred by the 4 ms budget) | ReceiveTick hook calls 3, driven 3\n",
            "[HSMPSync] pose sender: 60.0 frames/s (target 60 Hz) codec v2, 23 bones + weapon in 100%, control in 50%, pose sample+write avg 0.85 ms\n",
        )).unwrap();
        let m = metrics(&d).unwrap();
        assert_eq!(m["frame_cost_ms"]["n"], 2);
        assert_eq!(m["frame_cost_ms"]["mean"], 1.0);
        assert_eq!(m["pose_sender_ms"]["mean"], 0.85);
        assert_eq!(run(&d, &d, None), 0);
        let _ = std::fs::remove_dir_all(&d);
    }
}
