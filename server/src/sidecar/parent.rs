//! Which game process this sidecar belongs to, and waiting for it to exit:
//! a dead game must always tear the sidecar down, so the seat frees,
//! `career_guard.leave()` runs and the link record says "ended".
//!
//! Resolution, first match wins:
//! 1. `--parent-pid N`: that process. Dead at startup = the game is gone.
//! 2. The parent chain (Toolhelp snapshot): skip shells (cmd.exe, conhost,
//!    PowerShell); the first ancestor whose image matches the game
//!    (`HSMP_GAME_IMAGE`, default contains "halfsword") is the game. Every hop
//!    must be created no later than its child (a reused PID breaks the chain).
//!    A live non-shell, non-game ancestor (a test harness, a terminal) means
//!    someone else owns our lifetime: no watch.
//! 3. A broken chain (HSMPMenu spawns us with `cmd /c start "" /B ...`, and
//!    that cmd.exe exits at once): every live game process created before
//!    us. One candidate is exact; with several (two local instances) the
//!    sidecar leaves when the last one is gone (never too early).
//!
//! The watch holds a process HANDLE (`SYNCHRONIZE`), so a PID reused after
//! the game exits can never keep the sidecar alive.
//! `HSMP_PARENT_WATCH=0` disables 2 and 3 (an explicit `--parent-pid` still applies).

#![allow(dead_code)]

use std::time::Duration;

#[derive(Debug, Clone, PartialEq)]
pub struct ProcInfo {
    pub pid: u32,
    pub ppid: u32,
    pub exe: String,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Target {
    /// `--parent-pid`.
    Explicit(u32),
    /// The game found up the parent chain.
    Ancestor(u32),
    /// Chain broken: these game processes (leave when all are gone).
    Candidates(Vec<u32>),
    /// Nothing to watch (reason for the log).
    Unwatched(String),
}

pub fn is_shell(exe: &str) -> bool {
    let e = exe.to_ascii_lowercase();
    matches!(e.as_str(), "cmd.exe" | "conhost.exe" | "powershell.exe" | "pwsh.exe" | "openconsole.exe")
}

pub fn game_pattern() -> String {
    std::env::var("HSMP_GAME_IMAGE").ok().filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| "halfsword".into()).to_ascii_lowercase()
}

pub fn is_game(exe: &str, pattern: &str) -> bool {
    exe.to_ascii_lowercase().contains(pattern)
}

/// Pure resolution over a process snapshot (`created(pid)` = creation time
/// in ms, None = gone).
pub fn resolve(me: u32, procs: &[ProcInfo], created: &dyn Fn(u32) -> Option<u64>, pattern: &str) -> Target {
    let find = |pid: u32| procs.iter().find(|p| p.pid == pid);
    let my_created = created(me).unwrap_or(u64::MAX);
    let mut cur = me;
    let mut cur_created = my_created;
    for _ in 0..32 {
        let Some(me_entry) = find(cur) else { break };
        let ppid = me_entry.ppid;
        let Some(parent) = find(ppid).filter(|_| ppid != 0 && ppid != cur) else { break };
        let Some(pc) = created(ppid) else { break };
        if pc > cur_created {
            break; // the parent PID was reused by a younger process
        }
        if is_game(&parent.exe, pattern) {
            return Target::Ancestor(ppid);
        }
        if !is_shell(&parent.exe) {
            return Target::Unwatched(format!("launched by {} (pid {}), not the game", parent.exe, ppid));
        }
        cur = ppid;
        cur_created = pc;
    }
    let mut c: Vec<u32> = procs.iter()
        .filter(|p| p.pid != me && is_game(&p.exe, pattern))
        .filter(|p| created(p.pid).is_some_and(|t| t <= my_created))
        .map(|p| p.pid)
        .collect();
    c.sort_unstable();
    c.dedup();
    if c.is_empty() { Target::Unwatched("no game process found".into()) } else { Target::Candidates(c) }
}

#[cfg(windows)]
mod sys {
    use super::ProcInfo;
    type Handle = *mut core::ffi::c_void;
    const TH32CS_SNAPPROCESS: u32 = 0x2;
    const INVALID_HANDLE_VALUE: isize = -1;
    const SYNCHRONIZE: u32 = 0x0010_0000;
    const PROCESS_QUERY_LIMITED_INFORMATION: u32 = 0x1000;
    const WAIT_TIMEOUT: u32 = 0x102;

    #[repr(C)]
    struct ProcessEntry32W {
        dw_size: u32,
        cnt_usage: u32,
        th32_process_id: u32,
        th32_default_heap_id: usize,
        th32_module_id: u32,
        cnt_threads: u32,
        th32_parent_process_id: u32,
        pc_pri_class_base: i32,
        dw_flags: u32,
        sz_exe_file: [u16; 260],
    }

    #[repr(C)]
    #[derive(Default, Clone, Copy)]
    struct FileTime { low: u32, high: u32 }

    #[link(name = "kernel32")]
    extern "system" {
        fn CreateToolhelp32Snapshot(flags: u32, pid: u32) -> Handle;
        fn Process32FirstW(snap: Handle, pe: *mut ProcessEntry32W) -> i32;
        fn Process32NextW(snap: Handle, pe: *mut ProcessEntry32W) -> i32;
        fn OpenProcess(access: u32, inherit: i32, pid: u32) -> Handle;
        fn GetProcessTimes(h: Handle, c: *mut FileTime, e: *mut FileTime, k: *mut FileTime, u: *mut FileTime) -> i32;
        fn WaitForSingleObject(h: Handle, ms: u32) -> u32;
        fn CloseHandle(h: Handle) -> i32;
    }

    pub fn snapshot() -> Vec<ProcInfo> {
        let mut out = Vec::new();
        unsafe {
            let snap = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0);
            if snap.is_null() || snap as isize == INVALID_HANDLE_VALUE { return out; }
            let mut pe: ProcessEntry32W = std::mem::zeroed();
            pe.dw_size = std::mem::size_of::<ProcessEntry32W>() as u32;
            let mut ok = Process32FirstW(snap, &mut pe);
            while ok != 0 {
                let n = pe.sz_exe_file.iter().position(|c| *c == 0).unwrap_or(260);
                out.push(ProcInfo {
                    pid: pe.th32_process_id,
                    ppid: pe.th32_parent_process_id,
                    exe: String::from_utf16_lossy(&pe.sz_exe_file[..n]),
                });
                ok = Process32NextW(snap, &mut pe);
            }
            CloseHandle(snap);
        }
        out
    }

    /// An open process handle (keeps the process object, so its PID cannot
    /// be confused with a later process).
    pub struct Proc(Handle);
    unsafe impl Send for Proc {}
    unsafe impl Sync for Proc {}

    impl Proc {
        pub fn open(pid: u32) -> Option<Proc> {
            if pid == 0 { return None; }
            let h = unsafe { OpenProcess(SYNCHRONIZE | PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
            (!h.is_null()).then_some(Proc(h))
        }
        pub fn alive(&self) -> bool {
            unsafe { WaitForSingleObject(self.0, 0) == WAIT_TIMEOUT }
        }
        /// Creation time, unix ms.
        pub fn created_ms(&self) -> Option<u64> {
            let (mut c, mut e, mut k, mut u) = (FileTime::default(), FileTime::default(), FileTime::default(), FileTime::default());
            if unsafe { GetProcessTimes(self.0, &mut c, &mut e, &mut k, &mut u) } == 0 { return None; }
            let t100 = ((c.high as u64) << 32) | c.low as u64;
            // 100 ns ticks since 1601-01-01 -> ms since 1970-01-01
            (t100 / 10_000).checked_sub(11_644_473_600_000)
        }
    }

    impl Drop for Proc {
        fn drop(&mut self) { unsafe { CloseHandle(self.0); } }
    }
}

#[cfg(not(windows))]
mod sys {
    use super::ProcInfo;
    pub fn snapshot() -> Vec<ProcInfo> { Vec::new() }
    pub struct Proc(u32);
    impl Proc {
        pub fn open(pid: u32) -> Option<Proc> {
            (pid != 0 && std::path::Path::new(&format!("/proc/{pid}")).exists()).then_some(Proc(pid))
        }
        pub fn alive(&self) -> bool { std::path::Path::new(&format!("/proc/{}", self.0)).exists() }
        pub fn created_ms(&self) -> Option<u64> { Some(0) }
    }
}

pub use sys::Proc;

/// Creation time (unix ms) of a live process.
pub fn created_ms(pid: u32) -> Option<u64> {
    let p = Proc::open(pid)?;
    if !p.alive() { return None; }
    p.created_ms()
}

/// Process `pid` is alive and was created at or before `not_after_ms`
/// (career guard: a manifest's sidecar PID that was reused is dead).
pub fn alive_since(pid: u32, not_after_ms: u64) -> bool {
    created_ms(pid).is_some_and(|c| c <= not_after_ms)
}

/// Decide what to watch for this process.
pub fn resolve_now(explicit: Option<u32>) -> Target {
    if let Some(p) = explicit {
        return Target::Explicit(p);
    }
    if std::env::var("HSMP_PARENT_WATCH").is_ok_and(|v| v.trim() == "0") {
        return Target::Unwatched("HSMP_PARENT_WATCH=0".into());
    }
    let procs = sys::snapshot();
    resolve(std::process::id(), &procs, &created_ms, &game_pattern())
}

/// Resolves when the watched game is gone (`Unwatched`: never). Polled every
/// 500 ms on handles opened once.
pub async fn wait_game_exit(target: Target) {
    let pids: Vec<u32> = match &target {
        Target::Explicit(p) | Target::Ancestor(p) => vec![*p],
        Target::Candidates(v) => v.clone(),
        Target::Unwatched(_) => {
            std::future::pending::<()>().await;
            return;
        }
    };
    // A pid we cannot open is already gone.
    let handles: Vec<Proc> = pids.iter().filter_map(|p| Proc::open(*p)).collect();
    let mut t = tokio::time::interval(Duration::from_millis(500));
    loop {
        t.tick().await;
        if !handles.iter().any(|h| h.alive()) {
            return;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(pid: u32, ppid: u32, exe: &str) -> ProcInfo { ProcInfo { pid, ppid, exe: exe.into() } }

    fn times(t: &[(u32, u64)]) -> impl Fn(u32) -> Option<u64> + '_ {
        move |pid| t.iter().find(|(p, _)| *p == pid).map(|(_, c)| *c)
    }

    #[test]
    fn finds_the_game_through_shells() {
        let procs = [p(1, 0, "explorer.exe"), p(10, 1, "HalfSwordUE5-Win64-Shipping.exe"),
                     p(20, 10, "cmd.exe"), p(30, 20, "hsmp-sidecar.exe")];
        let t = [(1, 1), (10, 100), (20, 200), (30, 300)];
        assert_eq!(resolve(30, &procs, &times(&t), "halfsword"), Target::Ancestor(10));
    }

    #[test]
    fn broken_chain_falls_back_to_game_candidates() {
        // cmd.exe (pid 20) already exited: the chain stops at us.
        let procs = [p(10, 1, "HalfSwordUE5-Win64-Shipping.exe"), p(11, 1, "HalfSwordUE5-Win64-Shipping.exe"),
                     p(12, 1, "HalfSwordUE5-Win64-Shipping.exe"), p(30, 20, "hsmp-sidecar.exe")];
        let t = [(10, 100), (11, 150), (12, 900), (30, 300)];
        assert_eq!(resolve(30, &procs, &times(&t), "halfsword"), Target::Candidates(vec![10, 11]),
                   "a game started after us is not ours");
        let one = [p(10, 1, "HalfSwordUE5-Win64-Shipping.exe"), p(30, 20, "hsmp-sidecar.exe")];
        assert_eq!(resolve(30, &one, &times(&t), "halfsword"), Target::Candidates(vec![10]));
        assert!(matches!(resolve(30, &[p(30, 20, "x.exe")], &times(&t), "halfsword"), Target::Unwatched(_)));
    }

    #[test]
    fn reused_parent_pid_breaks_the_chain() {
        // Our parent pid 20 now belongs to a younger, unrelated process.
        let procs = [p(20, 1, "HalfSwordUE5-Win64-Shipping.exe"), p(30, 20, "hsmp-sidecar.exe")];
        let t = [(20, 500), (30, 300)];
        assert!(matches!(resolve(30, &procs, &times(&t), "halfsword"), Target::Unwatched(_)),
                "a younger process with our parent's pid is not our game");
    }

    #[test]
    fn a_harness_parent_is_not_watched() {
        let procs = [p(5, 1, "hsmp-gate.exe"), p(30, 5, "hsmp-sidecar.exe"), p(10, 1, "HalfSwordUE5-Win64-Shipping.exe")];
        let t = [(5, 10), (30, 300), (10, 100)];
        assert!(matches!(resolve(30, &procs, &times(&t), "halfsword"), Target::Unwatched(_)));
    }

    #[test]
    fn own_process_is_alive_and_created_before_now() {
        let me = std::process::id();
        let c = created_ms(me).expect("own creation time");
        assert!(c <= crate::career_guard::now_ms());
        assert!(alive_since(me, crate::career_guard::now_ms()));
        assert!(!alive_since(me, c.saturating_sub(60_000)), "created after the cutoff = a reused pid");
        assert!(!alive_since(0x7FFF_FFF0, u64::MAX));
    }

    /// A real child: wait_game_exit returns once it exits, through a handle.
    #[cfg(windows)]
    #[tokio::test]
    async fn wait_returns_when_the_watched_process_exits() {
        let mut child = std::process::Command::new("cmd").args(["/c", "ping -n 2 127.0.0.1 >nul"]).spawn().unwrap();
        let pid = child.id();
        let start = std::time::Instant::now();
        tokio::time::timeout(Duration::from_secs(15), wait_game_exit(Target::Explicit(pid))).await
            .expect("watch ended");
        assert!(start.elapsed() >= Duration::from_millis(500));
        let _ = child.wait();
        // A pid that is already gone resolves at the first tick.
        tokio::time::timeout(Duration::from_secs(2), wait_game_exit(Target::Candidates(vec![0x7FFF_FFF0]))).await.unwrap();
    }

    /// The snapshot sees this test process with its real parent.
    #[cfg(windows)]
    #[test]
    fn snapshot_contains_self() {
        let s = sys::snapshot();
        let me = s.iter().find(|p| p.pid == std::process::id()).expect("self in snapshot");
        assert!(me.exe.to_ascii_lowercase().ends_with(".exe"));
    }
}
