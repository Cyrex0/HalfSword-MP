//! The sidecar's panic policy (src/sidecar/panic_guard.rs): a panic in a spawned task must
//! end the process (exit 101) and leave a panic log in the state dir, never a half-dead
//! sidecar. Debug builds only (the forced panic hook `HSMP_SIDECAR_TEST_PANIC` is compiled
//! out of release builds). Also: a sidecar started without `--ipc shm:<name>` exits at once
//! with code 64 (there is no file backend).
#![cfg(windows)]

mod common;
use common::*;

use std::process::{Command, Stdio};

#[test]
fn task_panic_exits_the_sidecar_with_a_log() {
    if !cfg!(debug_assertions) {
        eprintln!("skipped: release build (forced panic compiled out)");
        return;
    }
    let g = Game::new("panic");
    let mut child = g.spawn_env(hsmp_ipc::shm::current_pid(), &[("HSMP_SIDECAR_TEST_PANIC", "1")]);
    let (code, err) = wait_exit(&mut child, 20);
    assert_eq!(code, Some(101), "stderr:\n{err}");
    assert!(err.contains("FATAL panic") && err.contains("forced test panic"), "stderr:\n{err}");
    let log = std::fs::read_to_string(g.dir.join(".sidecar_panic.log")).expect("panic log written");
    assert!(log.contains("forced test panic") && log.contains("backtrace"), "{log}");
    // One panic log: no `.sidecar_panic.txt`, no IPC files.
    assert!(!g.dir.join(".sidecar_panic.txt").exists());
    assert_eq!(ipc_files(&g.dir), Vec::<String>::new());
    // The panic path detaches (the game sees the sidecar leave).
    assert_eq!(g.seg().header.sidecar.state(), Some(hsmp_ipc::SideState::Closing));
    let _ = std::fs::remove_dir_all(&g.dir);
}

#[test]
fn no_ipc_link_exits_64() {
    let dir = std::env::temp_dir().join(format!("hsmp_sidecar_noipc_{}", std::process::id()));
    let _ = std::fs::create_dir_all(&dir);
    for extra in [&[][..], &["--ipc", "file"][..], &["--status-file", "x.json", "--poll-ms", "33"][..]] {
        let mut child = Command::new(env!("CARGO_BIN_EXE_hsmp-sidecar"))
            .args(["--server", "127.0.0.1:9", "--state-dir"])
            .arg(&dir)
            .args(extra)
            .env("HSMP_CAREER_GUARD", "0")
            .env("HSMP_IDENTITY_DIR", &dir)
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let (code, err) = wait_exit(&mut child, 20);
        assert_eq!(code, Some(64), "{extra:?}: stderr:\n{err}");
        assert!(err.contains("--ipc shm:<name>"), "{extra:?}: stderr:\n{err}");
    }
    assert_eq!(ipc_files(&dir), Vec::<String>::new());
    let _ = std::fs::remove_dir_all(&dir);
}
