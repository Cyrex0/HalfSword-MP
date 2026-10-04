//! Background sampler -> observed.jsonl (only on change). Fallback evidence while the Lua
//! side is not instrumented. Stops when <run>/.stop_observer exists or the parent PID is gone.
//!
//! Source per instance: the sidecar's tap (`ipc_tap.jsonl`, run.json `ipc_taps[inst]`, else
//!   `<run>/inst<N>/ipc_tap.jsonl`), tailed live: `slot` lines carry every record-slot publish;
//!   the `link` record (the sidecar's status) and the `session` snapshot (phase, round, arena)
//!   carry the state (docs/development/ipc-shared-memory.md).

use crate::events;
use crate::util;
use hsmp_tools::ipcgame as ig;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::time::Duration;
/// The tap path of instance `i`.
pub fn tap_path(run_dir: &Path, cfg: &Value, i: &str) -> PathBuf {
    cfg["ipc_taps"][i].as_str().map(PathBuf::from).unwrap_or_else(|| run_dir.join(format!("inst{i}")).join("ipc_tap.jsonl"))
}

/// Incremental reader of a JSONL file that is still being written (complete lines only).
#[derive(Default)]
pub struct Tail {
    off: u64,
}

impl Tail {
    pub fn read(&mut self, p: &Path) -> Vec<Value> {
        let Ok(mut f) = std::fs::File::open(p) else { return Vec::new() };
        let len = f.metadata().map(|m| m.len()).unwrap_or(0);
        if len < self.off {
            self.off = 0;
        }
        let mut buf = String::new();
        if len <= self.off || f.seek(SeekFrom::Start(self.off)).is_err() || f.read_to_string(&mut buf).is_err() {
            return Vec::new();
        }
        let complete = buf.rfind('\n').map(|p| p + 1).unwrap_or(0);
        self.off += complete as u64;
        buf[..complete].lines().filter_map(|l| serde_json::from_str(l).ok()).collect()
    }
}

struct Obs {
    out: PathBuf,
    last: HashMap<String, String>,
}

impl Obs {
    /// A `link` record: the sidecar status (name) and my peer id.
    fn sidecar(&mut self, i: &str, lk: &Value, now: i64) {
        if !lk.is_object() {
            return;
        }
        let status = ig::link_status_name(lk);
        let val = format!("{}|{}", status, lk["my_peer_id"]);
        if self.last.insert(format!("sc{i}"), val.clone()).as_ref() != Some(&val) {
            let rec = json!({"ev":"obs_sidecar","inst":i,"status":status,"peer_id":lk["my_peer_id"],"wall_ms":now});
            let _ = util::append_line(&self.out, &rec.to_string());
        }
    }
    /// A `session` record: the legacy match state, the arena in force and the round.
    fn matchst(&mut self, i: &str, ss: &Value, now: i64) {
        if !ss.is_object() || !ss["phase"].is_u64() {
            return;
        }
        let state = ig::legacy_state(ss["phase"].as_u64().unwrap_or(0));
        let arena = ig::session_arena(ss);
        let val = format!("{}|{:?}|{}", state, arena, ss["round"]);
        if self.last.insert(format!("mt{i}"), val.clone()).as_ref() != Some(&val) {
            let rec = json!({"ev":"obs_match","inst":i,"state":state,"arena":arena,"round":ss["round"],"wall_ms":now});
            let _ = util::append_line(&self.out, &rec.to_string());
        }
    }
    /// One tap line.
    fn line(&mut self, i: &str, l: &Value, now: i64) {
        if l["ev"] != "slot" {
            return;
        }
        let t = l["t"].as_i64().unwrap_or(now);
        match l["slot"].as_str() {
            Some("link") => self.sidecar(i, &l["v"], t),
            Some("session") => self.matchst(i, &l["v"], t),
            _ => {}
        }
    }
}

pub fn run(run_dir: &Path, parent_pid: u32) {
    let cfg = events::load_run(run_dir);
    let dirs: Vec<(String, String)> = cfg["state_dirs"]
        .as_object()
        .into_iter()
        .flatten()
        .filter_map(|(k, v)| v.as_str().map(|d| (k.clone(), d.to_string())))
        .collect();
    let mut obs = Obs { out: run_dir.join("observed.jsonl"), last: HashMap::new() };
    let stop = run_dir.join(".stop_observer");
    let mut taps: HashMap<String, Tail> = HashMap::new();
    let mut tick: u64 = 0;
    loop {
        if stop.exists() || (tick.is_multiple_of(8) && !util::pid_alive(parent_pid)) {
            break;
        }
        tick += 1;
        let now = util::now_ms();
        for (i, _) in &dirs {
            let tp = tap_path(run_dir, &cfg, i);
            for l in taps.entry(i.clone()).or_default().read(&tp) {
                obs.line(i, &l, now);
            }
        }
        std::thread::sleep(Duration::from_millis(250));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tap_slot_lines_become_obs_records() {
        let d = std::env::temp_dir().join(format!("hsmp-obs-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(d.join("inst1")).unwrap();
        let tap = d.join("inst1").join("ipc_tap.jsonl");
        std::fs::write(&tap, concat!(
            "{\"t\":1,\"ev\":\"attach\",\"abi\":\"2.0\"}\n",
            "{\"t\":2,\"ev\":\"slot\",\"slot\":\"link\",\"v\":{\"status\":1,\"my_peer_id\":1}}\n",
            "{\"t\":2,\"ev\":\"slot\",\"slot\":\"session\",\"v\":{\"phase\":0,\"round\":0,\"has_frozen\":false,\"config\":{\"arena\":\"Map_Arena_Alley\"}}}\n",
            "{\"t\":3,\"ev\":\"slot\",\"slot\":\"link\",\"v\":{\"status\":1,\"my_peer_id\":1,\"rtt_ms\":40.0}}\n",
            "{\"t\":3,\"ev\":\"slot\",\"slot\":\"session\",\"v\":{\"phase\":3,\"round\":1,\"has_frozen\":true,\"frozen\":{\"arena\":\"Map_Arena_Pit\"},\"config\":{\"arena\":\"Map_Arena_Alley\"}}}\n",
            "{\"t\":4,\"ev\":\"slot\",\"slot\":\"link\",\"v\":{\"stat",
        )).unwrap();
        let cfg = json!({"state_dirs": {"1": d.join("nostate").display().to_string()}});
        let mut obs = Obs { out: d.join("observed.jsonl"), last: HashMap::new() };
        let mut t = Tail::default();
        for l in t.read(&tap_path(&d, &cfg, "1")) {
            obs.line("1", &l, 0);
        }
        let out = std::fs::read_to_string(d.join("observed.jsonl")).unwrap();
        let lines: Vec<Value> = out.lines().map(|l| serde_json::from_str(l).unwrap()).collect();
        assert_eq!(lines.len(), 3, "{out}"); // 1 sidecar (a metrics-only change is no change) + 2 match states; the torn last line waits
        assert_eq!(lines[0]["status"], "connected");
        assert_eq!(lines[2]["state"], "live");
        assert_eq!(lines[2]["arena"], "Map_Arena_Pit");
        assert!(t.read(&tap).is_empty());
        let _ = std::fs::remove_dir_all(&d);
    }
}
