//! What the session watcher copies into the session folder once the game has exited
//! (or, for a run whose watcher never finished, what the launcher can still rescue).
//!
//! | file in the session folder | from |
//! |---|---|
//! | `UE4SS.log` | `<game Win64>\ue4ss\UE4SS.log` (the next start overwrites it) |
//! | `game.log` | `%LOCALAPPDATA%\HalfSwordUE5\Saved\Logs\HalfswordUE5.log` |
//! | `crashes\<UECC-...>\` | this run's engine crash folders |
//! | `crashes\ue4ss\*.dmp` | this run's UE4SS crash dumps |
//! | `hsmp_events.jsonl` | this run's lines of `<state>\hsmp_events*.jsonl` |
//! | `career_guard.jsonl` | this run's lines of `<state>\.career_guard.jsonl` |
//! | `sidecar_panic.log` | `<state>\.sidecar_panic.log`, when written during the run |
//!
//! "This run" is the window [started - 5 s, ended + 2 min] by file time or `wall_ms`.

use crate::sessions::{Outcome, Session};
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

/// A single log copy keeps at most this much (the tail).
pub const MAX_LOG_BYTES: u64 = 40 * 1024 * 1024;
/// Crash files larger than this are left out (full memory dumps).
pub const MAX_CRASH_FILE_BYTES: u64 = 64 * 1024 * 1024;
const BEFORE_MS: u64 = 5_000;
const AFTER_MS: u64 = 120_000;

#[derive(Debug, Clone, Default)]
pub struct Sources {
    /// UE4SS.log of the game install.
    pub ue4ss_log: Option<PathBuf>,
    /// The `ue4ss` folder (its crash dumps).
    pub ue4ss_dir: Option<PathBuf>,
    /// `%LOCALAPPDATA%\HalfSwordUE5\Saved`.
    pub ue_saved: Option<PathBuf>,
    /// The game's HSMP state dir.
    pub state_dir: Option<PathBuf>,
}

impl Sources {
    /// Paths for a game exe (`...\Binaries\Win64\HalfswordUE5-Win64-Shipping.exe`).
    pub fn for_game(exe: Option<&Path>, state_dir: Option<PathBuf>) -> Sources {
        let win64 = exe.and_then(|e| e.parent()).map(Path::to_path_buf);
        let ue4ss_dir = win64.as_ref().map(|w| w.join("ue4ss"));
        Sources {
            ue4ss_log: ue4ss_dir.as_ref().map(|d| d.join("UE4SS.log")),
            ue4ss_dir,
            ue_saved: std::env::var_os("LOCALAPPDATA").map(|l| PathBuf::from(l).join("HalfSwordUE5").join("Saved")),
            state_dir: state_dir.map(|s| if s.is_relative() { win64.as_ref().map(|w| w.join(&s)).unwrap_or(s) } else { s }),
        }
    }
}

/// The run's outcome from its exit code and crash evidence.
pub fn outcome(exit_code: Option<u32>, crash_evidence: bool) -> Outcome {
    if crash_evidence {
        return Outcome::Crashed;
    }
    match exit_code {
        None => Outcome::Unfinished,
        Some(0) => Outcome::Clean,
        Some(c) if c >= 0xC000_0000 => Outcome::Crashed,
        Some(_) => Outcome::Abnormal,
    }
}

fn in_window(t: u64, start: u64, end: u64) -> bool {
    t + BEFORE_MS >= start && t <= end + AFTER_MS
}

/// Copy at most the last `max` bytes of `src` to `dst`. Returns the bytes written.
pub fn copy_tail(src: &Path, dst: &Path, max: u64) -> std::io::Result<u64> {
    let mut f = std::fs::File::open(src)?;
    let len = f.metadata()?.len();
    let mut out = std::fs::File::create(dst)?;
    if len > max {
        f.seek(SeekFrom::Start(len - max))?;
        std::io::Write::write_all(&mut out, format!("[hsmp: the first {} bytes were left out]\n", len - max).as_bytes())?;
    }
    std::io::copy(&mut f.take(max), &mut out)
}

/// `"wall_ms":<n>` of a JSONL line, without parsing the whole object.
pub fn wall_ms_of(line: &str) -> Option<u64> {
    let i = line.find("\"wall_ms\":")? + 10;
    let digits: String = line[i..].trim_start().chars().take_while(|c| c.is_ascii_digit()).collect();
    digits.parse().ok()
}

/// The lines of `srcs` (oldest file first) whose `wall_ms` is in the window.
pub fn slice_jsonl(srcs: &[PathBuf], start: u64, end: u64) -> String {
    let mut out = String::new();
    for p in srcs {
        let Ok(b) = std::fs::read(p) else { continue };
        for l in String::from_utf8_lossy(&b).lines() {
            if wall_ms_of(l).is_some_and(|t| in_window(t, start, end)) {
                out.push_str(l);
                out.push('\n');
            }
        }
    }
    out
}

fn copy_dir_capped(src: &Path, dst: &Path) -> Vec<String> {
    let mut skipped = vec![];
    let _ = std::fs::create_dir_all(dst);
    let Ok(rd) = std::fs::read_dir(src) else { return skipped };
    for e in rd.flatten() {
        let p = e.path();
        if p.is_dir() {
            skipped.extend(copy_dir_capped(&p, &dst.join(e.file_name())));
        } else if e.metadata().map(|m| m.len()).unwrap_or(0) > MAX_CRASH_FILE_BYTES {
            skipped.push(p.display().to_string());
        } else {
            let _ = std::fs::copy(&p, dst.join(e.file_name()));
        }
    }
    skipped
}

/// Engine crash folders (`UECC-*`) whose dump or folder time is in the window.
pub fn crash_dirs_in(crashes: &Path, start: u64, end: u64) -> Vec<PathBuf> {
    let mut v: Vec<PathBuf> = std::fs::read_dir(crashes)
        .map(|rd| {
            rd.flatten()
                .map(|e| e.path())
                .filter(|p| p.is_dir() && p.file_name().is_some_and(|n| n.to_string_lossy().starts_with("UECC-")))
                .filter(|p| {
                    let dump = std::fs::read_dir(p).ok().and_then(|r| r.flatten().map(|e| e.path()).find(|f| f.extension().is_some_and(|x| x.eq_ignore_ascii_case("dmp"))));
                    dump.as_deref().and_then(crate::time::mtime_ms).or_else(|| crate::time::mtime_ms(p)).is_some_and(|t| in_window(t, start, end))
                })
                .collect()
        })
        .unwrap_or_default();
    v.sort();
    v
}

fn files_in(dir: &Path, ext: &str, start: u64, end: u64) -> Vec<PathBuf> {
    std::fs::read_dir(dir)
        .map(|rd| {
            rd.flatten()
                .map(|e| e.path())
                .filter(|p| p.is_file() && p.extension().is_some_and(|x| x.eq_ignore_ascii_case(ext)))
                .filter(|p| crate::time::mtime_ms(p).is_some_and(|t| in_window(t, start, end)))
                .collect()
        })
        .unwrap_or_default()
}

/// Collect everything for a finished run into its folder, set the outcome and save
/// `session.json`. `ended_ms` is when the game exited (now, for a rescue).
pub fn finish(s: &mut Session, src: &Sources, exit_code: Option<u32>, ended_ms: u64) {
    let (start, end) = (s.info.started_ms, ended_ms);
    let mut collected = vec![];
    let mut notes = vec![];
    if let Some(log) = src.ue4ss_log.as_ref().filter(|p| p.is_file()) {
        // only this run's log: the next start truncates it, so an mtime before our start is an older run
        if crate::time::mtime_ms(log).is_some_and(|t| t + BEFORE_MS >= start) {
            match copy_tail(log, &s.dir.join("UE4SS.log"), MAX_LOG_BYTES) {
                Ok(_) => collected.push("UE4SS.log".to_string()),
                Err(e) => notes.push(format!("UE4SS.log not copied: {e}")),
            }
        } else {
            notes.push("UE4SS.log is older than this run".into());
        }
    }
    if let Some(saved) = &src.ue_saved {
        let gl = saved.join("Logs").join("HalfswordUE5.log");
        if crate::time::mtime_ms(&gl).is_some_and(|t| in_window(t, start, end)) && copy_tail(&gl, &s.dir.join("game.log"), MAX_LOG_BYTES).is_ok() {
            collected.push("game.log".into());
        }
        for d in crash_dirs_in(&saved.join("Crashes"), start, end) {
            let name = d.file_name().unwrap_or_default().to_string_lossy().to_string();
            for sk in copy_dir_capped(&d, &s.dir.join("crashes").join(&name)) {
                notes.push(format!("left out (too large): {sk}"));
            }
            collected.push(format!("crashes/{name}/"));
            if !s.info.crash_dirs.contains(&name) {
                s.info.crash_dirs.push(name);
            }
        }
    }
    if let Some(ud) = &src.ue4ss_dir {
        for d in files_in(ud, "dmp", start, end) {
            let name = d.file_name().unwrap_or_default().to_string_lossy().to_string();
            let dst = s.dir.join("crashes").join("ue4ss");
            let _ = std::fs::create_dir_all(&dst);
            if std::fs::metadata(&d).map(|m| m.len()).unwrap_or(0) <= MAX_CRASH_FILE_BYTES && std::fs::copy(&d, dst.join(&name)).is_ok() {
                collected.push(format!("crashes/ue4ss/{name}"));
            }
            if !s.info.ue4ss_dumps.contains(&name) {
                s.info.ue4ss_dumps.push(name);
            }
        }
    }
    if let Some(sd) = &src.state_dir {
        let mut ev: Vec<PathBuf> = (1..=4).rev().map(|i| sd.join(format!("hsmp_events.{i}.jsonl"))).collect();
        ev.push(sd.join("hsmp_events.jsonl"));
        for (srcs, name) in [(ev, "hsmp_events.jsonl"), (vec![sd.join(".career_guard.jsonl")], "career_guard.jsonl")] {
            let text = slice_jsonl(&srcs, start, end);
            if !text.is_empty() && std::fs::write(s.dir.join(name), text).is_ok() {
                collected.push(name.to_string());
            }
        }
        let panic = sd.join(".sidecar_panic.log");
        if crate::time::mtime_ms(&panic).is_some_and(|t| in_window(t, start, end)) && copy_tail(&panic, &s.dir.join("sidecar_panic.log"), MAX_LOG_BYTES).is_ok() {
            collected.push("sidecar_panic.log".into());
        }
    }
    let evidence = !s.info.crash_dirs.is_empty() || !s.info.ue4ss_dumps.is_empty();
    s.info.outcome = outcome(exit_code, evidence);
    if exit_code.is_some() {
        s.info.exit_code = exit_code;
    }
    s.info.ended_ms = Some(ended_ms);
    for c in collected {
        if !s.info.collected.contains(&c) {
            s.info.collected.push(c);
        }
    }
    s.info.notes.extend(notes);
    let _ = s.save();
}

/// A run whose watcher never finished (`Unfinished`, newest first): when its UE4SS.log is
/// still the file in the game folder (no game started since), copy it in. The launcher calls
/// this before it starts the game and at start-up while the game is not running.
pub fn rescue(root: &Path, src: &Sources, now_ms: u64) -> Option<String> {
    let log = src.ue4ss_log.as_ref()?;
    let t = crate::time::mtime_ms(log)?;
    let mut sessions = crate::sessions::list(root);
    // the newest session that started before the log was last written owns it
    let idx = sessions.iter().position(|s| s.info.started_ms <= t + BEFORE_MS)?;
    let s = &mut sessions[idx];
    if s.info.outcome != Outcome::Unfinished || s.info.collected.iter().any(|c| c == "UE4SS.log") {
        return None;
    }
    let ended = s.info.ended_ms.unwrap_or(t.min(now_ms));
    finish(s, src, None, ended);
    // finish() maps "no exit code" to Unfinished unless crash evidence turned up
    s.info.notes.push("UE4SS.log rescued by the launcher (the watcher had not finished)".into());
    let _ = s.save();
    Some(s.info.id.clone())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sessions::{create, tests::tmp};

    #[test]
    fn outcomes() {
        assert_eq!(outcome(Some(0), false), Outcome::Clean);
        assert_eq!(outcome(Some(0), true), Outcome::Crashed);
        assert_eq!(outcome(Some(0xC000_0005), false), Outcome::Crashed);
        assert_eq!(outcome(Some(1), false), Outcome::Abnormal);
        assert_eq!(outcome(None, false), Outcome::Unfinished);
    }

    #[test]
    fn jsonl_window_and_tail_copy() {
        assert_eq!(wall_ms_of(r#"{"ev":"x","wall_ms":1790000000123,"y":1}"#), Some(1_790_000_000_123));
        assert_eq!(wall_ms_of(r#"{"ev":"x"}"#), None);
        let d = tmp("jsonl");
        std::fs::write(d.join("a.jsonl"), "{\"wall_ms\":1000}\n{\"wall_ms\":50000}\n{\"wall_ms\":999999}\nnot json\n").unwrap();
        let s = slice_jsonl(&[d.join("a.jsonl"), d.join("missing")], 10_000, 60_000);
        assert_eq!(s, "{\"wall_ms\":50000}\n");
        std::fs::write(d.join("big.log"), "0123456789".repeat(10)).unwrap();
        copy_tail(&d.join("big.log"), &d.join("t.log"), 15).unwrap();
        let t = std::fs::read_to_string(d.join("t.log")).unwrap();
        assert!(t.starts_with("[hsmp: the first 85 bytes") && t.ends_with("567890123456789"), "{t}");
        let _ = std::fs::remove_dir_all(&d);
    }

    /// A crashed run: UE4SS.log, the crash folder of the window (not an older one), the
    /// event slice and the outcome all land in the session.
    #[test]
    fn finish_collects_a_crashed_run() {
        let d = tmp("finish");
        let now = crate::time::now_ms();
        let (game, saved, state) = (d.join("Win64"), d.join("Saved"), d.join("Win64/hsmp_state"));
        std::fs::create_dir_all(game.join("ue4ss")).unwrap();
        std::fs::create_dir_all(saved.join("Logs")).unwrap();
        std::fs::create_dir_all(saved.join("Crashes/UECC-Windows-NEW")).unwrap();
        std::fs::create_dir_all(&state).unwrap();
        std::fs::write(game.join("ue4ss/UE4SS.log"), "[Lua] hello\n").unwrap();
        std::fs::write(saved.join("Logs/HalfswordUE5.log"), "engine\n").unwrap();
        std::fs::write(saved.join("Crashes/UECC-Windows-NEW/UEMinidump.dmp"), b"MDMP").unwrap();
        std::fs::write(state.join("hsmp_events.jsonl"), format!("{{\"ev\":\"old\",\"wall_ms\":{}}}\n{{\"ev\":\"new\",\"wall_ms\":{now}}}\n", now - 3_600_000)).unwrap();
        let root = d.join("logs");
        let mut s = create(&root, now - 60_000, 1, "v").unwrap();
        let src = Sources::for_game(Some(&game.join("HalfswordUE5-Win64-Shipping.exe")), Some(PathBuf::from("hsmp_state")));
        let src = Sources { ue_saved: Some(saved.clone()), ..src };
        assert_eq!(src.state_dir.as_deref(), Some(state.as_path()));
        finish(&mut s, &src, Some(3), now);
        let s = crate::sessions::Session::load(&s.dir).unwrap();
        assert_eq!(s.info.outcome, Outcome::Crashed);
        assert_eq!(s.info.crash_dirs, vec!["UECC-Windows-NEW".to_string()]);
        assert!(s.dir.join("UE4SS.log").is_file() && s.dir.join("game.log").is_file());
        assert!(s.dir.join("crashes/UECC-Windows-NEW/UEMinidump.dmp").is_file());
        let ev = std::fs::read_to_string(s.dir.join("hsmp_events.jsonl")).unwrap();
        assert!(ev.contains("\"new\"") && !ev.contains("\"old\""), "{ev}");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn rescue_copies_the_log_of_an_unfinished_run_once() {
        let d = tmp("rescue");
        let now = crate::time::now_ms();
        let root = d.join("logs");
        let mut s = create(&root, now - 120_000, 1, "v").unwrap();
        s.info.outcome = Outcome::Unfinished;
        s.save().unwrap();
        std::fs::create_dir_all(d.join("ue4ss")).unwrap();
        std::fs::write(d.join("ue4ss/UE4SS.log"), "last words\n").unwrap();
        let src = Sources { ue4ss_log: Some(d.join("ue4ss/UE4SS.log")), ..Default::default() };
        assert_eq!(rescue(&root, &src, now).as_deref(), Some(s.info.id.as_str()));
        assert_eq!(std::fs::read_to_string(s.dir.join("UE4SS.log")).unwrap(), "last words\n");
        assert_eq!(rescue(&root, &src, now), None, "once");
        let _ = std::fs::remove_dir_all(&d);
    }
}
