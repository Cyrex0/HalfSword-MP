//! Session folders: `<logs root>\<UTC stamp>_p<game pid>\` with a `session.json`.
//!
//! The newest [`KEEP_SESSIONS`] are kept, and older ones go while the total is above
//! [`MAX_TOTAL_BYTES`]. The current session and any session whose watcher still runs are
//! never pruned.

use crate::time;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

pub const SESSION_FILE: &str = "session.json";
pub const KEEP_SESSIONS: usize = 10;
pub const MAX_TOTAL_BYTES: u64 = 200 * 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum Outcome {
    /// The game is (or was, when last seen) running.
    #[default]
    Running,
    /// Exit code 0.
    Clean,
    /// A crash folder or dump from this run, or an NTSTATUS exit code.
    Crashed,
    /// Another exit code (Task Manager, a forced close).
    Abnormal,
    /// The watcher never finished (PC shut down, watcher killed): nothing was collected.
    Unfinished,
}

impl Outcome {
    pub fn as_str(self) -> &'static str {
        match self {
            Outcome::Running => "running",
            Outcome::Clean => "clean",
            Outcome::Crashed => "crashed",
            Outcome::Abnormal => "abnormal",
            Outcome::Unfinished => "unfinished",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct SessionInfo {
    pub v: u32,
    pub id: String,
    pub started_ms: u64,
    pub ended_ms: Option<u64>,
    pub game_pid: u32,
    pub watcher_pid: u32,
    pub game_exe: Option<String>,
    pub exit_code: Option<u32>,
    pub outcome: Outcome,
    /// UECC-* folders of this run (copied under `crashes/`).
    pub crash_dirs: Vec<String>,
    /// UE4SS crash dumps of this run.
    pub ue4ss_dumps: Vec<String>,
    /// Files the watcher (or the launcher) collected, relative to the folder.
    pub collected: Vec<String>,
    pub notes: Vec<String>,
    pub hsmp_version: String,
}

#[derive(Debug, Clone)]
pub struct Session {
    pub dir: PathBuf,
    pub info: SessionInfo,
}

impl Session {
    pub fn save(&self) -> std::io::Result<()> {
        write_atomic(&self.dir.join(SESSION_FILE), &serde_json::to_vec_pretty(&self.info).unwrap_or_default())
    }

    pub fn load(dir: &Path) -> Option<Session> {
        let name = dir.file_name()?.to_str()?;
        if !is_session_name(name) {
            return None;
        }
        let info = std::fs::read(dir.join(SESSION_FILE))
            .ok()
            .and_then(|b| serde_json::from_slice::<SessionInfo>(&b).ok())
            .unwrap_or_else(|| SessionInfo { id: name.to_string(), outcome: Outcome::Unfinished, ..Default::default() });
        Some(Session { dir: dir.to_path_buf(), info })
    }

    /// Bytes on disk.
    pub fn size(&self) -> u64 {
        dir_size(&self.dir)
    }
}

/// `%LOCALAPPDATA%\HSMP\logs`, or `HSMP_LOGS_DIR`.
pub fn logs_root() -> Option<PathBuf> {
    if let Some(d) = std::env::var_os("HSMP_LOGS_DIR").filter(|v| !v.is_empty()) {
        return Some(PathBuf::from(d));
    }
    std::env::var_os("LOCALAPPDATA").map(|l| PathBuf::from(l).join("HSMP").join("logs"))
}

/// `20261004_130405_p1234`
pub fn session_id(started_ms: u64, game_pid: u32) -> String {
    format!("{}_p{game_pid}", time::stamp(started_ms))
}

pub fn is_session_name(n: &str) -> bool {
    let b = n.as_bytes();
    b.len() >= 17 && b[..8].iter().all(u8::is_ascii_digit) && b[8] == b'_' && b[9..15].iter().all(u8::is_ascii_digit) && n[15..].starts_with("_p")
}

/// Every session under `root`, newest first.
pub fn list(root: &Path) -> Vec<Session> {
    let mut v: Vec<Session> = std::fs::read_dir(root)
        .map(|rd| rd.flatten().filter(|e| e.path().is_dir()).filter_map(|e| Session::load(&e.path())).collect())
        .unwrap_or_default();
    v.sort_by(|a, b| b.info.id.cmp(&a.info.id).then_with(|| b.dir.cmp(&a.dir)));
    v
}

/// Create a new session folder (a second start in the same second gets a suffix).
pub fn create(root: &Path, started_ms: u64, game_pid: u32, version: &str) -> std::io::Result<Session> {
    std::fs::create_dir_all(root)?;
    let base = session_id(started_ms, game_pid);
    let mut id = base.clone();
    let mut n = 1;
    while root.join(&id).exists() {
        n += 1;
        id = format!("{base}_{n}");
    }
    let dir = root.join(&id);
    std::fs::create_dir_all(&dir)?;
    let s = Session { dir, info: SessionInfo { v: 1, id, started_ms, game_pid, hsmp_version: version.to_string(), ..Default::default() } };
    s.save()?;
    Ok(s)
}

/// Sessions still marked running whose watcher and game are both gone: the watcher never
/// finished. They become `Unfinished` (the launcher may still rescue their UE4SS.log).
pub fn close_stale(root: &Path, alive: &dyn Fn(u32) -> bool) -> Vec<String> {
    let mut out = vec![];
    for mut s in list(root) {
        if s.info.outcome != Outcome::Running || alive(s.info.watcher_pid) || alive(s.info.game_pid) {
            continue;
        }
        s.info.outcome = Outcome::Unfinished;
        s.info.notes.push("the session watcher did not finish (PC shut down, or the watcher was stopped)".into());
        if s.save().is_ok() {
            out.push(s.info.id.clone());
        }
    }
    out
}

#[derive(Debug, Default, PartialEq)]
pub struct Pruned {
    pub removed: Vec<String>,
    pub total_bytes: u64,
}

/// Keep the newest `keep` sessions and at most `max_bytes` in total. `protect` (the
/// current session) and sessions whose watcher runs are never removed.
pub fn prune(root: &Path, keep: usize, max_bytes: u64, protect: &[&Path], alive: &dyn Fn(u32) -> bool) -> Pruned {
    let all = list(root);
    let mut sized: Vec<(Session, u64, bool)> = all
        .into_iter()
        .map(|s| {
            let size = s.size();
            let keep_it = protect.iter().any(|p| same_path(p, &s.dir)) || (s.info.outcome == Outcome::Running && alive(s.info.watcher_pid));
            (s, size, keep_it)
        })
        .collect();
    let mut out = Pruned::default();
    let remove = |s: &Session, out: &mut Pruned| {
        if std::fs::remove_dir_all(&s.dir).is_ok() {
            out.removed.push(s.info.id.clone());
            true
        } else {
            false
        }
    };
    // by count (newest first, so index >= keep is old)
    let mut i = 0;
    sized.retain(|(s, _, protected)| {
        let idx = i;
        i += 1;
        if idx >= keep && !protected {
            !remove(s, &mut out)
        } else {
            true
        }
    });
    // by size, oldest first
    let mut total: u64 = sized.iter().map(|x| x.1).sum();
    for (s, size, protected) in sized.iter().rev() {
        if total <= max_bytes {
            break;
        }
        if !protected && remove(s, &mut out) {
            total -= size;
        }
    }
    out.total_bytes = total;
    out
}

fn same_path(a: &Path, b: &Path) -> bool {
    let n = |p: &Path| p.to_string_lossy().replace('/', "\\").trim_end_matches('\\').to_lowercase();
    n(a) == n(b)
}

pub fn dir_size(p: &Path) -> u64 {
    let Ok(rd) = std::fs::read_dir(p) else { return 0 };
    rd.flatten()
        .map(|e| match e.file_type() {
            Ok(t) if t.is_dir() => dir_size(&e.path()),
            Ok(_) => e.metadata().map(|m| m.len()).unwrap_or(0),
            Err(_) => 0,
        })
        .sum()
}

pub fn write_atomic(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, bytes)?;
    std::fs::rename(&tmp, path).inspect_err(|_| {
        let _ = std::fs::remove_file(&tmp);
    })
}

/// What `hsmp-sidecar --log-session start` does: close stale sessions, create this run's,
/// prune, and return it with the previous session (the newest older one).
pub fn start(root: &Path, game_pid: u32, now_ms: u64, version: &str, alive: &dyn Fn(u32) -> bool) -> std::io::Result<(Session, Option<Session>)> {
    close_stale(root, alive);
    let prev = list(root).into_iter().find(|s| s.info.game_pid != game_pid || s.info.started_ms < now_ms);
    let cur = create(root, now_ms, game_pid, version)?;
    prune(root, KEEP_SESSIONS, MAX_TOTAL_BYTES, &[&cur.dir], alive);
    Ok((cur, prev))
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    pub(crate) fn tmp(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("hsmp_diag_{tag}_{}_{}", std::process::id(), time::now_ms()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    const T0: u64 = 1_790_899_200_000;

    #[test]
    fn names() {
        assert_eq!(session_id(T0, 42), "20261002_000000_p42");
        assert!(is_session_name("20261002_000000_p42"));
        assert!(is_session_name("20261002_000000_p42_2"));
        assert!(!is_session_name("crash_reports"));
        assert!(!is_session_name("2026100_000000_p42"));
    }

    #[test]
    fn create_list_and_same_second() {
        let r = tmp("create");
        let a = create(&r, T0, 7, "0.1").unwrap();
        let b = create(&r, T0, 7, "0.1").unwrap();
        create(&r, T0 + 1000, 8, "0.1").unwrap();
        std::fs::create_dir_all(r.join("not-a-session")).unwrap();
        assert_ne!(a.dir, b.dir);
        let l = list(&r);
        assert_eq!(l.len(), 3);
        assert_eq!(l[0].info.game_pid, 8, "newest first");
        assert_eq!(Session::load(&a.dir).unwrap().info.hsmp_version, "0.1");
        let _ = std::fs::remove_dir_all(&r);
    }

    #[test]
    fn prune_by_count_and_size_keeps_the_current_and_live_ones() {
        let r = tmp("prune");
        let mut dirs = vec![];
        for i in 0..14u64 {
            let mut s = create(&r, T0 + i * 1000, 100 + i as u32, "v").unwrap();
            s.info.outcome = Outcome::Clean;
            if i == 1 {
                // an old session whose watcher still runs
                s.info.outcome = Outcome::Running;
                s.info.watcher_pid = 4242;
            }
            s.save().unwrap();
            std::fs::write(s.dir.join("blob"), vec![0u8; 1000]).unwrap();
            dirs.push(s.dir);
        }
        let alive = |pid: u32| pid == 4242;
        let p = prune(&r, 10, u64::MAX, &[&dirs[0]], &alive);
        // 14 sessions, keep 10 newest + the protected oldest + the live one: 2 removed (#2, #3)
        assert_eq!(p.removed.len(), 2, "{p:?}");
        assert!(dirs[0].exists() && dirs[1].exists() && !dirs[2].exists() && !dirs[3].exists());
        // a size cap of ~5 sessions removes the oldest unprotected ones
        let per = Session::load(&dirs[13]).unwrap().size();
        let p = prune(&r, 10, per * 5 + per / 2, &[&dirs[0]], &alive);
        assert!(p.total_bytes <= per * 5 + per / 2, "{p:?}");
        assert!(dirs[0].exists() && dirs[1].exists() && dirs[13].exists());
        assert!(!dirs[4].exists());
        let _ = std::fs::remove_dir_all(&r);
    }

    #[test]
    fn start_closes_stale_sessions_and_reports_the_previous_one() {
        let r = tmp("start");
        let mut old = create(&r, T0, 5, "v").unwrap();
        old.info.watcher_pid = 6;
        old.save().unwrap();
        let (cur, prev) = start(&r, 9, T0 + 60_000, "v", &|_| false).unwrap();
        let prev = prev.unwrap();
        assert_eq!(prev.info.id, old.info.id);
        assert_eq!(prev.info.outcome, Outcome::Unfinished);
        assert_eq!(cur.info.outcome, Outcome::Running);
        assert_eq!(cur.info.game_pid, 9);
        let _ = std::fs::remove_dir_all(&r);
    }
}
