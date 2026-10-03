//! `hsmp-gate memwatch`: soak memory sampler. Every `--interval-s` it samples each
//! process the harness tracks (mp_test.ps1's pidfile: `[{role, pid, name, start_ticks}]`,
//! re-read every tick so sidecars found later are picked up) and appends one row per process
//! to `<run>/mem.jsonl`:
//!
//!     {"ev":"mem","wall_ms":..,"role":"game1","inst":"1","pid":..,"private_bytes":..,"working_set":..,"handles":..}
//!
//! Roles sampled: game<i>, server, master, sidecar<i>. A PID whose start time no longer matches
//! the pidfile (reused PID) or that has exited gets one `mem_gone` row and is dropped.
//! Stops when `<run>/.stop_observer` or `<run>/.stop_memwatch` exists or `--parent-pid` is gone.
//! The SOAK-MEM rule (rules.rs / soak.rs) evaluates the file.

use crate::util;
use serde_json::{json, Value};
use std::collections::HashSet;
use std::path::Path;
use std::time::{Duration, Instant};

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Sample {
    pub private_bytes: u64,
    pub working_set: u64,
    pub handles: u32,
    /// process creation time as .NET UTC ticks (100 ns since 0001-01-01), 0 if unknown
    pub start_ticks: i64,
}

/// FILETIME (100 ns since 1601) -> .NET DateTime ticks (100 ns since 0001), as PowerShell's
/// `$p.StartTime.ToUniversalTime().Ticks` records them in the pidfile.
pub const FILETIME_TO_DOTNET_TICKS: i64 = 504_911_232_000_000_000;

#[cfg(windows)]
pub fn sample(pid: u32) -> Option<Sample> {
    use windows_sys::Win32::Foundation::{CloseHandle, FILETIME};
    use windows_sys::Win32::System::ProcessStatus::{K32GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS, PROCESS_MEMORY_COUNTERS_EX};
    use windows_sys::Win32::System::Threading::{
        GetExitCodeProcess, GetProcessHandleCount, GetProcessTimes, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_VM_READ,
    };
    const STILL_ACTIVE: u32 = 259;
    // SAFETY: plain Win32 calls on a handle we open and close here; out-params are local.
    unsafe {
        let mut h = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_VM_READ, 0, pid);
        if h == 0 {
            h = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        }
        if h == 0 {
            return None;
        }
        let mut code: u32 = 0;
        if GetExitCodeProcess(h, &mut code) != 0 && code != STILL_ACTIVE {
            CloseHandle(h);
            return None;
        }
        let mut pmc: PROCESS_MEMORY_COUNTERS_EX = std::mem::zeroed();
        pmc.cb = std::mem::size_of::<PROCESS_MEMORY_COUNTERS_EX>() as u32;
        let ok = K32GetProcessMemoryInfo(h, &mut pmc as *mut PROCESS_MEMORY_COUNTERS_EX as *mut PROCESS_MEMORY_COUNTERS, pmc.cb);
        let mut handles: u32 = 0;
        GetProcessHandleCount(h, &mut handles);
        let z = FILETIME { dwLowDateTime: 0, dwHighDateTime: 0 };
        let (mut c, mut e, mut k, mut u) = (z, z, z, z);
        let start_ticks = if GetProcessTimes(h, &mut c, &mut e, &mut k, &mut u) != 0 {
            (((c.dwHighDateTime as u64) << 32) | c.dwLowDateTime as u64) as i64 + FILETIME_TO_DOTNET_TICKS
        } else {
            0
        };
        CloseHandle(h);
        if ok == 0 {
            return None;
        }
        Some(Sample { private_bytes: pmc.PrivateUsage as u64, working_set: pmc.WorkingSetSize as u64, handles, start_ticks })
    }
}

#[cfg(not(windows))]
pub fn sample(_pid: u32) -> Option<Sample> {
    None
}

/// Roles worth sampling and the instance they belong to.
pub fn role_inst(role: &str) -> Option<String> {
    let digits: String = role.chars().filter(|c| c.is_ascii_digit()).collect();
    if (role.starts_with("game") || role.starts_with("sidecar")) && !digits.is_empty() {
        Some(digits)
    } else if matches!(role, "server" | "master") {
        Some(role.to_string())
    } else {
        None
    }
}

/// The pidfile's entries to sample: (role, pid, start_ticks).
pub fn tracked(pidfile: &Path) -> Vec<(String, u32, i64)> {
    let v = util::read_json(pidfile).unwrap_or(Value::Null);
    let list = match v {
        Value::Array(a) => a,
        Value::Object(_) => vec![v], // ConvertTo-Json of a one-element list without -InputObject
        _ => vec![],
    };
    list.iter()
        .filter_map(|e| {
            let role = e["role"].as_str()?.to_string();
            role_inst(&role)?;
            let pid = e["pid"].as_u64()? as u32;
            let st = e["start_ticks"].as_i64().unwrap_or(0);
            Some((role, pid, st))
        })
        .collect()
}

/// Same process as recorded? (1 s tolerance; 0 = not recorded / unknown)
pub fn same_start(recorded: i64, now: i64) -> bool {
    recorded == 0 || now == 0 || (recorded - now).abs() < 10_000_000
}

pub fn run(run_dir: &Path, pidfile: &Path, interval_s: f64, parent_pid: u32) {
    let out = run_dir.join("mem.jsonl");
    let stops = [run_dir.join(".stop_observer"), run_dir.join(".stop_memwatch")];
    let interval = Duration::from_secs_f64(interval_s.max(1.0));
    let mut gone: HashSet<(String, u32)> = HashSet::new();
    let mut next = Instant::now();
    loop {
        if stops.iter().any(|p| p.exists()) {
            break;
        }
        if Instant::now() >= next {
            next += interval;
            // tasklist per check: only once per sample, not per 250 ms poll
            if !util::pid_alive(parent_pid) {
                break;
            }
            let now = util::now_ms();
            for (role, pid, st) in tracked(pidfile) {
                let key = (role.clone(), pid);
                if gone.contains(&key) {
                    continue;
                }
                let inst = role_inst(&role).unwrap_or_default();
                match sample(pid) {
                    Some(s) if same_start(st, s.start_ticks) => {
                        let row = json!({"ev": "mem", "wall_ms": now, "role": role, "inst": inst, "pid": pid,
                                         "private_bytes": s.private_bytes, "working_set": s.working_set, "handles": s.handles});
                        let _ = util::append_line(&out, &row.to_string());
                    }
                    _ => {
                        gone.insert(key);
                        let row = json!({"ev": "mem_gone", "wall_ms": now, "role": role, "inst": inst, "pid": pid});
                        let _ = util::append_line(&out, &row.to_string());
                    }
                }
            }
        }
        std::thread::sleep(Duration::from_millis(250));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roles() {
        assert_eq!(role_inst("game2").as_deref(), Some("2"));
        assert_eq!(role_inst("sidecar1").as_deref(), Some("1"));
        assert_eq!(role_inst("server").as_deref(), Some("server"));
        assert_eq!(role_inst("netsim1"), None);
        assert_eq!(role_inst("observer"), None);
        assert_eq!(role_inst("game"), None);
    }

    #[test]
    fn pidfile_shapes_and_start_match() {
        let tmp = hsmp_tools::paths::make_temp_dir("hsmp_memwatch_").unwrap();
        let p = tmp.join("pids.json");
        std::fs::write(&p, r#"[{"role":"game1","pid":10,"name":"x","start_ticks":5},{"role":"netsim1","pid":11},{"role":"server","pid":12,"start_ticks":0}]"#).unwrap();
        assert_eq!(tracked(&p), vec![("game1".to_string(), 10, 5), ("server".to_string(), 12, 0)]);
        std::fs::write(&p, r#"{"role":"sidecar2","pid":13,"start_ticks":7}"#).unwrap();
        assert_eq!(tracked(&p), vec![("sidecar2".to_string(), 13, 7)]);
        assert!(tracked(&tmp.join("missing.json")).is_empty());
        assert!(same_start(0, 99));
        assert!(same_start(100_000_000, 100_000_001));
        assert!(!same_start(100_000_000, 300_000_000));
        let _ = std::fs::remove_dir_all(&tmp);
    }

    /// Our own process: memory > 0, and the start time agrees with "now" within a day.
    #[cfg(windows)]
    #[test]
    fn samples_self() {
        let s = sample(std::process::id()).expect("sample self");
        assert!(s.private_bytes > 0 && s.working_set > 0 && s.handles > 0);
        let now_ticks = util::now_ms() * 10_000 + 621_355_968_000_000_000; // unix ms -> .NET ticks
        assert!((now_ticks - s.start_ticks).abs() < 864_000_000_000, "start {} now {}", s.start_ticks, now_ticks);
        assert!(sample(0xFFFF_FFF0).is_none());
    }
}
