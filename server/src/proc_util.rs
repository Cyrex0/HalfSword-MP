//! Process tracking (docs/development/testing.md): `--pid-file` and
//! `--parent-pid`. Shared by `hsmp-server` and `hsmp-sidecar` (`#[path]`).
//!
//! * The pid file is written atomically (tmp + rename) at startup and removed
//!   when the returned guard drops (clean exit):
//!   `{"v":1,"role":"server","pid":1234,"exe":"hsmp-server.exe","started_ms":..,"parent_pid":5678|null}`
//! * `wait_parent_exit(pid)` resolves once that process is gone, so the binary
//!   can leave cleanly (the sidecar sends its Leave) instead of being orphaned.
//!   It never kills anything.

#![allow(dead_code)]

use std::path::{Path, PathBuf};
use std::time::Duration;

/// Removes the pid file on drop (clean exit only; a crash leaves it, and the
/// harness checks pid + image + start time before trusting it).
pub struct PidFile(PathBuf);

impl Drop for PidFile {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

fn exe_name() -> String {
    std::env::current_exe()
        .ok()
        .and_then(|p| p.file_name().map(|n| n.to_string_lossy().to_string()))
        .unwrap_or_default()
}

pub fn pid_file_json(role: &str, pid: u32, exe: &str, started_ms: u64, parent_pid: Option<u32>) -> String {
    serde_json::json!({
        "v": 1, "role": role, "pid": pid, "exe": exe,
        "started_ms": started_ms, "parent_pid": parent_pid,
    })
    .to_string()
}

/// Write `path` atomically for this process.
pub fn write_pid_file(path: &Path, role: &str, parent_pid: Option<u32>) -> std::io::Result<PidFile> {
    if let Some(dir) = path.parent() {
        if !dir.as_os_str().is_empty() {
            std::fs::create_dir_all(dir)?;
        }
    }
    let started = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);
    let body = pid_file_json(role, std::process::id(), &exe_name(), started, parent_pid);
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, body + "\n")?;
    if std::fs::rename(&tmp, path).is_err() {
        let _ = std::fs::remove_file(path);
        std::fs::rename(&tmp, path)?;
    }
    Ok(PidFile(path.to_path_buf()))
}

#[cfg(windows)]
mod sys {
    type Handle = *mut core::ffi::c_void;
    const SYNCHRONIZE: u32 = 0x0010_0000;
    const PROCESS_QUERY_LIMITED_INFORMATION: u32 = 0x1000;
    const WAIT_TIMEOUT: u32 = 0x102;
    const ERROR_ACCESS_DENIED: u32 = 5;

    #[link(name = "kernel32")]
    extern "system" {
        fn OpenProcess(access: u32, inherit: i32, pid: u32) -> Handle;
        fn WaitForSingleObject(h: Handle, ms: u32) -> u32;
        fn CloseHandle(h: Handle) -> i32;
        fn GetLastError() -> u32;
    }

    pub fn pid_alive(pid: u32) -> bool {
        unsafe {
            let h = OpenProcess(SYNCHRONIZE | PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
            if h.is_null() {
                // A process we may not open still exists; anything else
                // (ERROR_INVALID_PARAMETER) means it is gone.
                return GetLastError() == ERROR_ACCESS_DENIED;
            }
            let r = WaitForSingleObject(h, 0);
            CloseHandle(h);
            r == WAIT_TIMEOUT
        }
    }
}

#[cfg(not(windows))]
mod sys {
    pub fn pid_alive(pid: u32) -> bool {
        std::path::Path::new(&format!("/proc/{pid}")).exists()
    }
}

/// True while process `pid` exists.
pub fn pid_alive(pid: u32) -> bool {
    pid != 0 && sys::pid_alive(pid)
}

/// Resolves when `pid` has exited (polled every 500 ms). `None`: never.
pub async fn wait_parent_exit(pid: Option<u32>) {
    let Some(pid) = pid else {
        std::future::pending::<()>().await;
        return;
    };
    let mut t = tokio::time::interval(Duration::from_millis(500));
    loop {
        t.tick().await;
        if !pid_alive(pid) {
            return;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn own_pid_is_alive_and_a_bogus_one_is_not() {
        assert!(pid_alive(std::process::id()));
        assert!(!pid_alive(0));
        assert!(!pid_alive(0x7FFF_FFF0));
    }

    #[test]
    fn pid_file_round_trip_and_removed_on_drop() {
        let dir = std::env::temp_dir().join(format!("hsmp_pidtest_{}", std::process::id()));
        let p = dir.join(".pid.server.json");
        {
            let _g = write_pid_file(&p, "server", Some(42)).unwrap();
            let v: serde_json::Value = serde_json::from_str(std::fs::read_to_string(&p).unwrap().trim()).unwrap();
            assert_eq!(v["role"], "server");
            assert_eq!(v["pid"], std::process::id());
            assert_eq!(v["parent_pid"], 42);
            assert!(!v["exe"].as_str().unwrap().is_empty());
        }
        assert!(!p.exists());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
