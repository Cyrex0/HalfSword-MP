//! `hsmp-sidecar --log-session start|watch` and `--log-dir`, end to end with the real binary.
//! A short-lived child process stands in for the game.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

fn sidecar() -> &'static str {
    env!("CARGO_BIN_EXE_hsmp-sidecar")
}

fn scratch(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("hsmp_logsession_{tag}_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn start(logs: &Path, pid: u32) -> Vec<(String, String)> {
    let out = Command::new(sidecar())
        .args(["--log-session", "start", "--parent-pid", &pid.to_string()])
        .env("HSMP_LOGS_DIR", logs)
        .output()
        .unwrap();
    assert!(out.status.success(), "{out:?}");
    String::from_utf8_lossy(&out.stdout).lines().filter_map(|l| l.split_once('=')).map(|(k, v)| (k.to_string(), v.to_string())).collect()
}

fn get<'a>(kv: &'a [(String, String)], k: &str) -> Option<&'a str> {
    kv.iter().find(|(a, _)| a == k).map(|(_, v)| v.as_str())
}

/// A "game" that exits with a non-zero code: the watcher records the exit and the outcome,
/// and the next start reports it as the previous session.
#[cfg(windows)]
#[test]
fn start_watch_and_the_next_start() {
    let d = scratch("sw");
    let logs = d.join("logs");
    let mut game = Command::new("cmd").args(["/c", "ping -n 3 127.0.0.1 >nul & exit 7"]).stdout(Stdio::null()).spawn().unwrap();
    let kv = start(&logs, game.id());
    let dir = PathBuf::from(get(&kv, "dir").expect("dir="));
    assert!(dir.join("session.json").is_file());
    assert_eq!(get(&kv, "prev_id"), None, "first session: {kv:?}");
    let mut w = Command::new(sidecar())
        .args(["--log-session", "watch", "--parent-pid", &game.id().to_string(), "--state-dir"])
        .arg(d.join("state"))
        .arg("--session-dir")
        .arg(&dir)
        .env("LOCALAPPDATA", d.join("la"))
        .spawn()
        .unwrap();
    let _ = game.wait();
    let t0 = Instant::now();
    let st = loop {
        if let Some(s) = w.try_wait().unwrap() {
            break s;
        }
        assert!(t0.elapsed() < Duration::from_secs(60), "watcher did not finish");
        std::thread::sleep(Duration::from_millis(100));
    };
    assert!(st.success());
    let info: serde_json::Value = serde_json::from_slice(&std::fs::read(dir.join("session.json")).unwrap()).unwrap();
    assert_eq!(info["exit_code"], 7, "{info}");
    assert_eq!(info["outcome"], "abnormal", "{info}");
    assert!(info["game_exe"].as_str().unwrap().to_lowercase().ends_with("cmd.exe"), "{info}");
    let kv2 = start(&logs, std::process::id());
    assert_eq!(get(&kv2, "prev_id"), get(&kv, "id"), "{kv2:?}");
    assert_eq!(get(&kv2, "prev_outcome"), Some("abnormal"));
    let _ = std::fs::remove_dir_all(&d);
}

/// `--log-dir`: the sidecar's tracing output also lands in <dir>/sidecar.log.
#[test]
fn log_dir_gets_the_sidecar_log() {
    let d = scratch("ld");
    // no --ipc: the sidecar logs the refusal and exits 64
    let out = Command::new(sidecar()).arg("--log-dir").arg(&d).arg("--state-dir").arg(d.join("state")).env("HSMP_CAREER_GUARD", "0").output().unwrap();
    assert_eq!(out.status.code(), Some(64), "{out:?}");
    let log = std::fs::read_to_string(d.join("sidecar.log")).unwrap();
    assert!(log.contains("==== sidecar") && log.contains("needs --ipc shm"), "{log}");
    assert!(!log.contains("\u{1b}["), "no colour codes in the file");
    let _ = std::fs::remove_dir_all(&d);
}
