//! `--events <path>`: structured JSONL events for the gate (docs/development/testing.md:
//! `phase`, `cmd_result`, `seat_restored`, ...).
//!
//! Shared by `hsmp-server` and `hsmp-sidecar` (the sidecar includes this file
//! with `#[path]`). One JSON object per line, appended and flushed per event,
//! so a crash loses nothing and the harness can tail the file:
//!
//! ```json
//! {"v":1,"ev":"phase","inst":"server","seq":7,"t_ms":5321,"wall_ms":1790000000000,"from":"Lobby","to":"Loading",...}
//! ```
//!
//! `emit` is a no-op until `init` was called with a path. Events are rare
//! (phase changes, command results), so the synchronous write is fine even
//! under the server's state lock.

#![allow(dead_code)]

use serde_json::{json, Map, Value};
use std::io::Write;
use std::path::Path;
use std::sync::{Mutex, OnceLock};
use std::time::Instant;

struct Sink {
    file: std::fs::File,
    inst: String,
    seq: u64,
    t0: Instant,
}

fn sink() -> &'static Mutex<Option<Sink>> {
    static S: OnceLock<Mutex<Option<Sink>>> = OnceLock::new();
    S.get_or_init(|| Mutex::new(None))
}

/// Wall clock, ms since the Unix epoch.
pub fn wall_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// Open (append) the event file. `inst` tags every line ("server", "sidecar").
pub fn init(path: Option<&Path>, inst: &str) -> std::io::Result<()> {
    let Some(path) = path else { return Ok(()) };
    if let Some(dir) = path.parent() {
        if !dir.as_os_str().is_empty() {
            std::fs::create_dir_all(dir)?;
        }
    }
    let file = std::fs::OpenOptions::new().create(true).append(true).open(path)?;
    *sink().lock().unwrap_or_else(|e| e.into_inner()) =
        Some(Sink { file, inst: inst.to_string(), seq: 0, t0: Instant::now() });
    Ok(())
}

pub fn enabled() -> bool {
    sink().lock().map(|s| s.is_some()).unwrap_or(false)
}

/// The line `emit` writes (envelope + `fields`, which must be a JSON object).
fn line(inst: &str, seq: u64, t_ms: u64, ev: &str, fields: Value) -> String {
    let mut m = Map::new();
    m.insert("v".into(), json!(1));
    m.insert("ev".into(), json!(ev));
    m.insert("inst".into(), json!(inst));
    m.insert("seq".into(), json!(seq));
    m.insert("t_ms".into(), json!(t_ms));
    m.insert("wall_ms".into(), json!(wall_ms()));
    if let Value::Object(f) = fields {
        for (k, v) in f {
            m.insert(k, v);
        }
    }
    let mut s = Value::Object(m).to_string();
    s.push('\n');
    s
}

/// Append one event. Never fails the caller.
pub fn emit(ev: &str, fields: Value) {
    let mut g = sink().lock().unwrap_or_else(|e| e.into_inner());
    let Some(s) = g.as_mut() else { return };
    s.seq += 1;
    let l = line(&s.inst, s.seq, s.t0.elapsed().as_millis() as u64, ev, fields);
    let _ = s.file.write_all(l.as_bytes());
    let _ = s.file.flush();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn line_has_envelope_and_fields() {
        let l = line("server", 3, 10, "phase", json!({"from": "Lobby", "to": "Loading", "round": 1}));
        assert!(l.ends_with('\n'));
        let v: Value = serde_json::from_str(l.trim()).unwrap();
        assert_eq!(v["v"], 1);
        assert_eq!(v["ev"], "phase");
        assert_eq!(v["inst"], "server");
        assert_eq!(v["seq"], 3);
        assert_eq!(v["to"], "Loading");
        assert!(v["wall_ms"].as_u64().unwrap() > 1_600_000_000_000);
    }
}
