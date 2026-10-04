//! `--log-session start|watch`: the per-run log folder (crates/hsmp-diag).
//!
//! * `start --parent-pid <game>`: close earlier sessions whose watcher died, create this run's
//!   folder, prune old ones, and print `key=value` lines for HSMPMenu (`dir`, `id`,
//!   `prev_id`, `prev_outcome`, `prev_crash`). Exits at once.
//! * `watch --session-dir <dir> --parent-pid <game> --state-dir <dir>`: wait for the game to
//!   exit, give the crash reporter time to write its folder, then collect UE4SS.log, the
//!   engine log, the run's crash folders and the event-log slices into the folder, and prune.
//!   It holds a process handle and sleeps in the wait: no CPU while the game runs.

use hsmp_diag::collect::{self, Sources};
use hsmp_diag::proc::{self, Proc};
use hsmp_diag::sessions::{self, Session};
use std::path::{Path, PathBuf};
use std::time::Duration;

fn alive(pid: u32) -> bool {
    proc::alive(pid)
}

/// `start`: stdout lines, exit status.
pub fn start(game_pid: Option<u32>, version: &str) -> (Vec<String>, i32) {
    let Some(root) = sessions::logs_root() else { return (vec!["error=LOCALAPPDATA is not set".into()], 1) };
    let pid = game_pid.unwrap_or(0);
    match sessions::start(&root, pid, hsmp_diag::time::now_ms(), version, &alive) {
        Ok((cur, prev)) => {
            let mut out = vec![format!("dir={}", cur.dir.display()), format!("id={}", cur.info.id)];
            if let Some(p) = prev {
                out.push(format!("prev_id={}", p.info.id));
                out.push(format!("prev_outcome={}", p.info.outcome.as_str()));
                if let Some(c) = p.info.crash_dirs.first() {
                    out.push(format!("prev_crash={c}"));
                }
            }
            (out, 0)
        }
        Err(e) => (vec![format!("error={e}")], 1),
    }
}

/// Wait (at most ~20 s) until the crash folders of the run have a dump whose size stopped
/// changing: the engine writes them while the process is going down.
fn settle_crash_dirs(src: &Sources, start_ms: u64) {
    let Some(saved) = &src.ue_saved else { return };
    let crashes = saved.join("Crashes");
    std::thread::sleep(Duration::from_secs(3));
    let mut last: Vec<(PathBuf, u64)> = vec![];
    for _ in 0..17 {
        let now: Vec<(PathBuf, u64)> = collect::crash_dirs_in(&crashes, start_ms, hsmp_diag::time::now_ms())
            .into_iter()
            .map(|d| {
                let size = std::fs::read_dir(&d).map(|r| r.flatten().filter_map(|e| e.metadata().ok()).map(|m| m.len()).sum()).unwrap_or(0);
                (d, size)
            })
            .collect();
        if now == last {
            return;
        }
        last = now;
        std::thread::sleep(Duration::from_secs(1));
    }
}

/// `watch`: returns the exit status.
pub fn watch(session_dir: &Path, game_pid: Option<u32>, state_dir: &Path) -> i32 {
    let Some(mut s) = Session::load(session_dir) else {
        eprintln!("--log-session watch: {} is not a session folder", session_dir.display());
        return 1;
    };
    let game = game_pid.and_then(Proc::open);
    s.info.watcher_pid = std::process::id();
    s.info.game_exe = game.as_ref().and_then(Proc::image).map(|p| p.display().to_string());
    let _ = s.save();
    let src = Sources::for_game(s.info.game_exe.as_deref().map(Path::new), Some(state_dir.to_path_buf()));
    let exit_code = match &game {
        Some(g) => {
            g.wait(None);
            g.exit_code()
        }
        None => {
            s.info.notes.push("the game process was not found when the watcher started".into());
            None
        }
    };
    let ended = hsmp_diag::time::now_ms();
    settle_crash_dirs(&src, s.info.started_ms);
    collect::finish(&mut s, &src, exit_code, ended);
    if let Some(root) = session_dir.parent() {
        sessions::prune(root, sessions::KEEP_SESSIONS, sessions::MAX_TOTAL_BYTES, &[session_dir], &alive);
    }
    0
}
