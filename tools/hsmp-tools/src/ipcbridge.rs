//! The legacy-file bridge of the test game (`hsmp-tools ipc-game --bridge <dir>`).
//!
//! The sidecar talks to the game over shared memory only, but the e2e suite's
//! assertions are written against the legacy state files, so the fake game
//! plays the part that the Lua facade's transition bridge played inside the real game: it
//! turns what the GAME side of the segment observes into those files, and the files a test
//! writes into game-side slots and G2S messages. The sidecar itself never touches them; the
//! round trip under test is sidecar <-> shared memory <-> game.
//!
//! Mapping (mirrors the sidecar's `ipc_shm::FIXED` routes):
//! - the session, combat and world domains are typed records: NOT mirrored (tests put them
//!   with `ipc-put` and read the game's `slot` / `s2g` events from the view);
//! - queue and request files: none left (every G2S message is a typed record);
//! - game-state files (`me_<pid>.json`, `.weapon.json`, `.skeletal.json`, `.pose_lead`): written to their
//!   slot / blob whenever their content changes.
//!
//! Peer streams (root, play, vitals, kit) and the typed records (the session domain: `session`,
//! `link`, `admin` slots, `cmd_result` / `notice` / `chat_in` events) are NOT mirrored: tests
//! read them from the `--view` log and write typed records with `hsmp-tools ipc-put`.

use crate::ipcgame::{put_json, GameEvent, GameHost};
use serde_json::{json, Value as J};
use std::collections::HashMap;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

pub const STATE_FILES: &[(&str, &str)] = &[];
pub const EVENT_FILES: &[(&str, &str)] = &[
];
pub const QUEUE_FILES: &[(&str, &str)] = &[
];
pub const REQUEST_FILES: &[(&str, &str)] = &[];
pub const GAME_STATE_FILES: &[(&str, &str)] = &[
    (".weapon.json", "local_weapon"), (".skeletal.json", "local_pose"), (".pose_lead", "pose_lead"),
];

/// How often the bridge looks at the game-side files.
const POLL: Duration = Duration::from_millis(5);

pub struct Bridge {
    dir: PathBuf,
    queue_off: HashMap<&'static str, u64>,
    /// Last content put per game-state file (by path).
    state_seen: HashMap<PathBuf, Vec<u8>>,
    /// None = never polled.
    last_poll: Option<Instant>,
    /// Lines and puts that failed (logged once each by the caller's stderr).
    pub errors: u64,
}

fn write_atomic(p: &Path, text: &str) {
    let tmp = p.with_extension(format!("{}.bridge.tmp", p.extension().and_then(|e| e.to_str()).unwrap_or("")));
    if std::fs::write(&tmp, text).is_ok() {
        for _ in 0..20 {
            if std::fs::rename(&tmp, p).is_ok() {
                return;
            }
            std::thread::sleep(Duration::from_millis(1));
        }
        let _ = std::fs::remove_file(&tmp);
    }
}

fn append(p: &Path, line: &str) {
    use std::io::Write;
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(p) {
        let _ = f.write_all(line.as_bytes());
        let _ = f.write_all(b"\n");
    }
}

/// The line a legacy file held for a value: JSON, or the raw text a non-JSON line carried.
fn line_of(v: &J) -> String {
    match v {
        J::String(s) => s.clone(),
        other => other.to_string(),
    }
}

/// A file's text as the JSON value the sidecar would have parsed from it.
fn value_of(text: &str) -> J {
    let t = text.trim_start_matches('\u{feff}').trim();
    serde_json::from_str(t).unwrap_or_else(|_| J::String(t.to_string()))
}

impl Bridge {
    pub fn new(dir: &Path) -> Bridge {
        let _ = std::fs::create_dir_all(dir);
        // Queue files that exist at start were addressed to a previous session (the legacy
        // file IPC skipped them too): start at their end.
        let queue_off = QUEUE_FILES
            .iter()
            .map(|(f, _)| (*f, std::fs::metadata(dir.join(f)).map(|m| m.len()).unwrap_or(0)))
            .collect();
        Bridge { dir: dir.to_path_buf(), queue_off, state_seen: HashMap::new(), last_poll: None, errors: 0 }
    }

    /// Mirror what the game side observed (`GameHost::pump` events) into the legacy files.
    pub fn on_events(&mut self, ev: &[GameEvent]) {
        for e in ev {
            match e["ev"].as_str() {
                Some("state") => {
                    if let Some((_, f)) = STATE_FILES.iter().find(|(k, _)| e["kind"] == *k) {
                        write_atomic(&self.dir.join(f), &(line_of(&e["v"]) + "\n"));
                    }
                }
                Some("s2g") => {
                    if let Some((_, f)) = EVENT_FILES.iter().find(|(k, _)| e["kind"] == *k) {
                        append(&self.dir.join(f), &line_of(&e["v"]));
                    }
                }
                _ => {}
            }
        }
    }

    /// Forward what a test wrote into the game-side files (rate-limited to every 5 ms).
    pub fn poll_files(&mut self, host: &GameHost) {
        if self.last_poll.is_some_and(|t| t.elapsed() < POLL) {
            return;
        }
        self.last_poll = Some(Instant::now());
        let s = host.seg();
        // queues: every new complete line is one message
        for (f, kind) in QUEUE_FILES {
            let p = self.dir.join(f);
            let Ok(mut fh) = std::fs::File::open(&p) else { continue };
            let len = fh.metadata().map(|m| m.len()).unwrap_or(0);
            let off = self.queue_off.entry(f).or_insert(0);
            if len < *off {
                *off = 0; // truncated / recreated
            }
            if len == *off || fh.seek(SeekFrom::Start(*off)).is_err() {
                continue;
            }
            let mut buf = String::new();
            if fh.read_to_string(&mut buf).is_err() {
                continue;
            }
            let complete = buf.rfind('\n').map(|i| i + 1).unwrap_or(0);
            *off += complete as u64;
            for line in buf[..complete].lines().filter(|l| !l.trim().is_empty()) {
                match serde_json::from_str::<J>(line.trim_start_matches('\u{feff}').trim()) {
                    Ok(v) => {
                        if put_json(s, kind, &v).is_err() {
                            self.errors += 1;
                        } else {
                            host.ring();
                        }
                    }
                    Err(_) => self.errors += 1, // the sidecar ignored non-JSON lines too
                }
            }
        }
        // requests: consumed once
        for (f, kind) in REQUEST_FILES {
            let p = self.dir.join(f);
            let Ok(text) = std::fs::read_to_string(&p) else { continue };
            let consumed = self.dir.join(format!("{f}.consumed"));
            let _ = std::fs::remove_file(&consumed);
            if std::fs::rename(&p, &consumed).is_err() {
                continue;
            }
            let v = match (value_of(&text), *kind) {
                (J::Object(o), _) => J::Object(o),
                (x, _) => json!({"why": x}),
            };
            if put_json(s, kind, &v).is_ok() {
                host.ring();
            } else {
                self.errors += 1;
            }
        }
        // game-written state: on content change
        let mut files: Vec<(PathBuf, &str)> = GAME_STATE_FILES.iter().map(|(f, k)| (self.dir.join(f), *k)).collect();
        if let Some(root) = newest_root_file(&self.dir) {
            files.push((root, "local_root"));
        }
        for (p, kind) in files {
            let Ok(bytes) = std::fs::read(&p) else { continue };
            if bytes.is_empty() || self.state_seen.get(&p) == Some(&bytes) {
                continue;
            }
            let text = String::from_utf8_lossy(&bytes).to_string();
            let v = value_of(&text);
            let v = if kind == "pose_lead" { json!(v.as_f64().or_else(|| v.as_str().and_then(|s| s.parse().ok())).unwrap_or(0.0)) } else { v };
            if put_json(s, kind, &v).is_ok() {
                host.ring();
            } else {
                self.errors += 1;
            }
            self.state_seen.insert(p, bytes);
        }
    }
}

/// The newest `me_<n>.json` (the game's root file; never `me_remote*`).
fn newest_root_file(dir: &Path) -> Option<PathBuf> {
    let rd = std::fs::read_dir(dir).ok()?;
    rd.flatten()
        .filter(|e| {
            let n = e.file_name().to_string_lossy().to_string();
            n.starts_with("me_") && n.ends_with(".json") && !n.starts_with("me_remote") && n[3..n.len() - 5].chars().all(|c| c.is_ascii_digit())
        })
        .max_by_key(|e| e.metadata().and_then(|m| m.modified()).ok())
        .map(|e| e.path())
}

#[cfg(test)]
mod tests {
    use super::*;
    use hsmp_ipc::handshake::{attach_sidecar, SideParams};
    use hsmp_ipc::ring::{Pop, Record};
    use hsmp_ipc::schema::{CAP_COMBAT, CAP_POSE, CAP_QUEUES, CAP_STATE};

    #[test]
    fn session_events_and_queues_round_trip() {
        let d = std::env::temp_dir().join(format!("hsmp-bridge-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        let all = CAP_POSE | CAP_STATE | CAP_QUEUES | CAP_COMBAT;
        let mut g = GameHost::anonymous(all).unwrap();
        let sp = SideParams { pid: 7, create_time: 7, epoch: 0x55, caps: all, build_id: "t" };
        attach_sidecar(g.seg(), g.mapping().len(), g.pid, None, &sp).unwrap();
        let mut b = Bridge::new(&d);
        // sidecar -> game: a session record (not mirrored; no legacy S2G event route remains)
        let s = g.seg();
        let (kind, payload) = hsmp_ipc::debug_json::json_to_record("link", &json!({"status": 1, "my_peer_id": 3})).unwrap();
        assert!(s.slot_ref("link", 0).unwrap().put(hsmp_ipc::schema::SlotMeta { valid: 1, ..Default::default() }, kind, &payload, &mut Vec::new()));
        let ev = g.pump();
        assert!(ev.iter().any(|e| e["ev"] == "slot" && e["slot"] == "link"), "{ev:?}");
        b.on_events(&ev);
        for f in [".sidecar.json", ".match.json", ".admin.json", ".link.json", ".cmd_results.jsonl"] {
            assert!(!d.join(f).exists(), "{f} is not mirrored any more");
        }
        assert!(!d.join(".damage_in.jsonl").exists(), "combat records are not mirrored");
        // game -> sidecar: a game-state file (no outbox file is bridged: G2S messages are records)
        std::fs::write(d.join(".world_claim.jsonl"), "{\"kind\":\"match\",\"verb\":\"ready\"}\nnot json\n").unwrap();
        std::fs::write(d.join("me_4242.json"), r#"{"pid":4242,"tick":5,"pos":[1,2,3],"rot":[0,0,0],"vel":[0,0,0]}"#).unwrap();
        std::thread::sleep(POLL);
        b.poll_files(&g);
        let s = g.seg();
        let mut r = Record::default();
        assert_eq!(s.g2s().pop(g.epoch, &mut r), Pop::Empty, "an old outbox file is ignored");
        assert_eq!(b.errors, 0);
        let (_, _, rb) = crate::ipcgame::read_slot(&s.game_out.local_root).unwrap();
        assert_eq!(hsmp_ipc::record::view::<hsmp_ipc::schema::pose::Root>(&rb).unwrap().head.pos, [1.0, 2.0, 3.0]);
        // unchanged content is not re-put
        let seq = hsmp_ipc::schema::RawSlot::version(&s.game_out.local_root);
        std::thread::sleep(POLL);
        b.poll_files(&g);
        assert_eq!(hsmp_ipc::schema::RawSlot::version(&g.seg().game_out.local_root), seq);
        let _ = std::fs::remove_dir_all(&d);
    }
}
