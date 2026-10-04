//! Process API: the game spawns the sidecar, the local server and the master
//! with `CreateProcessW` (no shell, no window by default) and keeps the process handles in the
//! process-global state, so liveness and kill go by our handle (never by name, never a PID we
//! did not spawn).
//!
//! | Call | Returns |
//! |---|---|
//! | `spawn(exe, args, opts)` | `pid` \| `nil, err` |
//! | `proc_alive(pid)` | `bool` (false for a PID we did not spawn) |
//! | `proc_exit_code(pid)` | `code` \| `nil, "running"` \| `nil, "not_ours"` |
//! | `proc_kill(pid)` | `true` \| `nil, "not_ours"` \| `nil, "gone"` |
//! | `spawn_capture(exe, args, opts?)` | `h` (= the child's pid) \| `nil, err` |
//! | `capture_poll(h, wait_ms?)` | `false` (running) \| `true, exit_code, output` (then released) \| `nil, "bad"` |
//! | `current_pid()` | the game's pid |
//!
//! `args` is a Lua array of strings (numbers are converted), quoted per the MSVCRT /
//! `CommandLineToArgvW` rules; `exe` is always quoted. `opts`: `cwd` (string), `env` (table:
//! `KEY = "value"` overrides, `KEY = false` removes; merged over the game's environment for
//! that child only), `hide` (default true: `CREATE_NO_WINDOW`). An `exe` ending in `.cmd` /
//! `.bat` runs as `%ComSpec% /d /s /c ""exe" "arg" ..."` (every argument quoted; `"`, `%`,
//! `!`, CR / LF in arguments are refused, cmd.exe would interpret them).
//!
//! Every spawned process is assigned to its own job object (created suspended, assigned,
//! resumed, so grandchildren are in it too) WITHOUT kill-on-close: children survive a game
//! crash (they follow `--parent-pid`). `proc_kill` terminates the whole job (the tree; e.g.
//! the server behind a `.cmd` wrapper), falling back to the process if the job could not be
//! created.
//!
//! `spawn_capture` gives the child one anonymous pipe for stdout + stderr (stdin = NUL; only
//! those handles are inherited, via `PROC_THREAD_ATTRIBUTE_HANDLE_LIST`). `capture_poll`
//! reads it non-blocking (`PeekNamedPipe` + `ReadFile` of what is available) on the game
//! thread; there is no reader thread. A child that writes more than the pipe buffer between
//! two polls simply waits. Output is capped at [`CAPTURE_MAX`] bytes (the rest is drained
//! and dropped). Handles are closed when a capture is reported done, and otherwise live in
//! the process-global state until the game exits (a few processes per session).
//!
//! `capture_poll(h, wait_ms)` first waits up to `wait_ms` (at most [`WAIT_MAX_MS`]) for the
//! child to exit, draining the pipe meanwhile. The boot-time career recovery uses it, since
//! it has to finish before the game can write a save.

use std::collections::HashMap;
use std::ffi::c_int;

use crate::lua::*;
use crate::native::Native;

/// Largest captured output kept.
pub const CAPTURE_MAX: usize = 1 << 20;
/// Longest blocking `capture_poll` wait.
pub const WAIT_MAX_MS: u64 = 5000;

/// One process we spawned.
#[cfg_attr(not(windows), allow(dead_code))] // job and eof are read by the Windows backend only
pub struct ProcEnt {
    /// Process handle (as an integer, so the state stays `Send`).
    handle: isize,
    /// Job handle (0 = none).
    job: isize,
    /// Pipe read end for `spawn_capture` (0 = not captured).
    pipe: isize,
    out: Vec<u8>,
    eof: bool,
}

#[derive(Default)]
pub struct ProcState {
    procs: HashMap<u32, ProcEnt>,
    tmp: Vec<u8>,
}

/// Parsed `spawn` arguments.
struct SpawnReq {
    exe: String,
    args: Vec<String>,
    cwd: Option<String>,
    env: Vec<(String, Option<String>)>,
    hide: bool,
}

/// Append `a` quoted per the MSVCRT / `CommandLineToArgvW` rules.
pub fn quote_arg(a: &str, out: &mut String) {
    if !a.is_empty() && !a.contains([' ', '\t', '\n', '\x0b', '"']) {
        out.push_str(a);
        return;
    }
    out.push('"');
    let mut bs = 0usize;
    for c in a.chars() {
        match c {
            '\\' => bs += 1,
            '"' => {
                out.extend(std::iter::repeat_n('\\', bs * 2 + 1));
                out.push('"');
                bs = 0;
            }
            _ => {
                out.extend(std::iter::repeat_n('\\', bs));
                bs = 0;
                out.push(c);
            }
        }
    }
    out.extend(std::iter::repeat_n('\\', bs * 2));
    out.push('"');
}

fn is_batch(exe: &str) -> bool {
    let l = exe.to_ascii_lowercase();
    l.ends_with(".cmd") || l.ends_with(".bat")
}

/// The full command line (`None` if an argument cannot be passed safely).
pub fn command_line(exe: &str, args: &[String]) -> Option<String> {
    if exe.is_empty() || exe.contains(['"', '\0', '\r', '\n']) || args.iter().any(|a| a.contains('\0')) {
        return None;
    }
    let mut s = String::new();
    if is_batch(exe) {
        if args.iter().any(|a| a.contains(['"', '%', '!', '\r', '\n'])) {
            return None;
        }
        let comspec = std::env::var("ComSpec").unwrap_or_else(|_| "cmd.exe".to_string());
        s.push('"');
        s.push_str(&comspec);
        s.push_str("\" /d /s /c \"\"");
        s.push_str(exe);
        s.push('"');
        for a in args {
            s.push_str(" \"");
            s.push_str(a);
            s.push('"');
        }
        s.push('"');
        return Some(s);
    }
    s.push('"');
    s.push_str(exe);
    s.push('"');
    for a in args {
        s.push(' ');
        quote_arg(a, &mut s);
    }
    Some(s)
}

/// String or number at stack `idx` (numbers via `tostring`).
unsafe fn str_or_num(L: *mut lua_State, idx: c_int) -> Option<String> {
    unsafe {
        match lua_type(L, idx) {
            LUA_TSTRING => arg_str(L, idx).map(str::to_string),
            LUA_TNUMBER => {
                lua_pushvalue(L, idx);
                let s = arg_str(L, -1).map(str::to_string);
                pop(L, 1);
                s
            }
            _ => None,
        }
    }
}

unsafe fn parse_spawn(L: *mut lua_State) -> Option<SpawnReq> {
    unsafe {
        let exe = arg_str(L, 1)?.to_string();
        let mut args = Vec::new();
        match lua_type(L, 2) {
            LUA_TNIL | LUA_TNONE => {}
            LUA_TTABLE => {
                let t = lua_absindex(L, 2);
                let n = lua_rawlen(L, t) as i64;
                for i in 1..=n {
                    lua_rawgeti(L, t, i);
                    let a = str_or_num(L, -1);
                    pop(L, 1);
                    args.push(a?);
                }
            }
            _ => return None,
        }
        let mut req = SpawnReq { exe, args, cwd: None, env: Vec::new(), hide: true };
        match lua_type(L, 3) {
            LUA_TNIL | LUA_TNONE => {}
            LUA_TTABLE => {
                let o = lua_absindex(L, 3);
                match rawget_str(L, o, "cwd") {
                    LUA_TNIL => {}
                    LUA_TSTRING => req.cwd = arg_str(L, -1).map(str::to_string),
                    _ => {
                        pop(L, 1);
                        return None;
                    }
                }
                pop(L, 1);
                match rawget_str(L, o, "hide") {
                    LUA_TNIL => {}
                    LUA_TBOOLEAN => req.hide = lua_toboolean(L, -1) != 0,
                    _ => {
                        pop(L, 1);
                        return None;
                    }
                }
                pop(L, 1);
                match rawget_str(L, o, "env") {
                    LUA_TNIL => pop(L, 1),
                    LUA_TTABLE => {
                        let e = lua_absindex(L, -1);
                        lua_pushnil(L);
                        while lua_next(L, e) != 0 {
                            let k = if lua_type(L, -2) == LUA_TSTRING { arg_str(L, -2).map(str::to_string) } else { None };
                            let v = match lua_type(L, -1) {
                                LUA_TBOOLEAN if lua_toboolean(L, -1) == 0 => Some(None),
                                LUA_TSTRING | LUA_TNUMBER => str_or_num(L, -1).map(Some),
                                _ => None,
                            };
                            pop(L, 1);
                            match (k, v) {
                                (Some(k), Some(v)) if !k.is_empty() && !k.contains(['=', '\0']) && !v.as_ref().is_some_and(|v| v.contains('\0')) => {
                                    req.env.push((k, v))
                                }
                                _ => {
                                    lua_settop(L, e - 1);
                                    return None;
                                }
                            }
                        }
                        pop(L, 1);
                    }
                    _ => {
                        pop(L, 1);
                        return None;
                    }
                }
            }
            _ => return None,
        }
        Some(req)
    }
}

impl Native {
    pub unsafe fn spawn(&mut self, L: *mut lua_State) -> c_int {
        unsafe { self.spawn_any(L, false) }
    }

    pub unsafe fn spawn_capture(&mut self, L: *mut lua_State) -> c_int {
        unsafe { self.spawn_any(L, true) }
    }

    unsafe fn spawn_any(&mut self, L: *mut lua_State, capture: bool) -> c_int {
        unsafe {
            let Some(mut req) = parse_spawn(L) else { return nil_err(L, "bad") };
            if capture {
                req.hide = true;
            }
            let Some(cmd) = command_line(&req.exe, &req.args) else { return nil_err(L, "bad") };
            self.procs.reap();
            match sys::spawn(&req, &cmd, capture) {
                Ok((pid, ent)) => {
                    if let Some(old) = self.procs.procs.insert(pid, ent) {
                        sys::close(&old);
                    }
                    lua_pushinteger(L, pid as i64);
                    1
                }
                Err(e) => nil_err(L, &format!("spawn: {}", e)),
            }
        }
    }

    pub unsafe fn proc_alive(&mut self, L: *mut lua_State) -> c_int {
        unsafe {
            let alive = self.ours(L).is_some_and(|p| sys::exit_code(p.handle).is_none());
            lua_pushboolean(L, alive as c_int);
            1
        }
    }

    pub unsafe fn proc_exit_code(&mut self, L: *mut lua_State) -> c_int {
        unsafe {
            let Some(p) = self.ours(L) else { return nil_err(L, "not_ours") };
            match sys::exit_code(p.handle) {
                Some(c) => {
                    lua_pushinteger(L, c as i64);
                    1
                }
                None => nil_err(L, "running"),
            }
        }
    }

    pub unsafe fn proc_kill(&mut self, L: *mut lua_State) -> c_int {
        unsafe {
            let Some(p) = self.ours(L) else { return nil_err(L, "not_ours") };
            if sys::exit_code(p.handle).is_some() {
                // The process is gone; still take down what is left of its job.
                sys::kill_job(p);
                return nil_err(L, "gone");
            }
            if sys::kill(p) {
                lua_pushboolean(L, 1);
                1
            } else {
                nil_err(L, "gone")
            }
        }
    }

    pub unsafe fn capture_poll(&mut self, L: *mut lua_State) -> c_int {
        unsafe {
            let Some(pid) = arg_int(L, 1).and_then(|v| u32::try_from(v).ok()) else { return nil_err(L, "bad") };
            let ps = &mut self.procs;
            let Some(p) = ps.procs.get_mut(&pid).filter(|p| p.pipe != 0) else { return nil_err(L, "bad") };
            let wait_ms = arg_int(L, 2).unwrap_or(0).clamp(0, WAIT_MAX_MS as i64) as u64;
            if wait_ms > 0 {
                let end = std::time::Instant::now() + std::time::Duration::from_millis(wait_ms);
                while std::time::Instant::now() < end {
                    sys::drain(p, &mut ps.tmp);
                    if sys::wait_exit(p.handle, 10) {
                        break;
                    }
                }
            }
            let code = sys::exit_code(p.handle);
            sys::drain(p, &mut ps.tmp);
            let Some(code) = code else {
                lua_pushboolean(L, 0);
                return 1;
            };
            sys::drain(p, &mut ps.tmp);
            let Some(p) = ps.procs.remove(&pid) else { return nil_err(L, "bad") };
            lua_pushboolean(L, 1);
            lua_pushinteger(L, code as i64);
            push_bytes(L, &p.out);
            sys::close(&p);
            3
        }
    }

    pub unsafe fn current_pid(&mut self, L: *mut lua_State) -> c_int {
        unsafe {
            lua_pushinteger(L, hsmp_ipc::shm::current_pid() as i64);
            1
        }
    }

    unsafe fn ours(&self, L: *mut lua_State) -> Option<&ProcEnt> {
        let pid = unsafe { arg_int(L, 1) }.and_then(|v| u32::try_from(v).ok())?;
        self.procs.procs.get(&pid)
    }
}

impl ProcState {
    /// Bound the table: close entries of exited, non-captured processes once it is large.
    fn reap(&mut self) {
        if self.procs.len() < 64 {
            return;
        }
        let dead: Vec<u32> = self.procs.iter().filter(|(_, p)| p.pipe == 0 && sys::exit_code(p.handle).is_some()).map(|(k, _)| *k).collect();
        for k in dead {
            if let Some(p) = self.procs.remove(&k) {
                sys::close(&p);
            }
        }
    }
}

#[cfg(windows)]
#[allow(clippy::upper_case_acronyms, non_camel_case_types)]
mod sys {
    use super::{ProcEnt, SpawnReq, CAPTURE_MAX};
    use std::ffi::c_void;
    use std::io;

    type HANDLE = *mut c_void;

    #[repr(C)]
    struct STARTUPINFOW {
        cb: u32,
        lp_reserved: *mut u16,
        lp_desktop: *mut u16,
        lp_title: *mut u16,
        dw_x: u32,
        dw_y: u32,
        dw_x_size: u32,
        dw_y_size: u32,
        dw_x_count_chars: u32,
        dw_y_count_chars: u32,
        dw_fill_attribute: u32,
        dw_flags: u32,
        w_show_window: u16,
        cb_reserved2: u16,
        lp_reserved2: *mut u8,
        h_std_input: HANDLE,
        h_std_output: HANDLE,
        h_std_error: HANDLE,
    }

    #[repr(C)]
    struct STARTUPINFOEXW {
        si: STARTUPINFOW,
        attrs: *mut c_void,
    }

    #[repr(C)]
    struct PROCESS_INFORMATION {
        h_process: HANDLE,
        h_thread: HANDLE,
        pid: u32,
        tid: u32,
    }

    #[repr(C)]
    struct SECURITY_ATTRIBUTES {
        n_length: u32,
        sd: *mut c_void,
        inherit: i32,
    }

    #[allow(clippy::too_many_arguments)]
    extern "system" {
        fn CreateProcessW(
            app: *const u16,
            cmd: *mut u16,
            pa: *const SECURITY_ATTRIBUTES,
            ta: *const SECURITY_ATTRIBUTES,
            inherit: i32,
            flags: u32,
            env: *const c_void,
            cwd: *const u16,
            si: *const STARTUPINFOW,
            pi: *mut PROCESS_INFORMATION,
        ) -> i32;
        fn CloseHandle(h: HANDLE) -> i32;
        fn TerminateProcess(h: HANDLE, code: u32) -> i32;
        fn GetExitCodeProcess(h: HANDLE, code: *mut u32) -> i32;
        fn WaitForSingleObject(h: HANDLE, ms: u32) -> u32;
        fn CreatePipe(r: *mut HANDLE, w: *mut HANDLE, sa: *const SECURITY_ATTRIBUTES, size: u32) -> i32;
        fn SetHandleInformation(h: HANDLE, mask: u32, flags: u32) -> i32;
        fn PeekNamedPipe(h: HANDLE, buf: *mut c_void, size: u32, read: *mut u32, avail: *mut u32, left: *mut u32) -> i32;
        fn ReadFile(h: HANDLE, buf: *mut c_void, n: u32, read: *mut u32, ov: *mut c_void) -> i32;
        fn InitializeProcThreadAttributeList(list: *mut c_void, count: u32, flags: u32, size: *mut usize) -> i32;
        fn UpdateProcThreadAttribute(list: *mut c_void, flags: u32, attr: usize, value: *const c_void, size: usize, prev: *mut c_void, ret: *mut usize) -> i32;
        fn DeleteProcThreadAttributeList(list: *mut c_void);
        fn CreateFileW(name: *const u16, access: u32, share: u32, sa: *const SECURITY_ATTRIBUTES, disp: u32, flags: u32, tmpl: HANDLE) -> HANDLE;
        fn CreateJobObjectW(sa: *const SECURITY_ATTRIBUTES, name: *const u16) -> HANDLE;
        fn AssignProcessToJobObject(job: HANDLE, process: HANDLE) -> i32;
        fn TerminateJobObject(job: HANDLE, code: u32) -> i32;
        fn ResumeThread(h: HANDLE) -> u32;
        fn GetLastError() -> u32;
    }

    const CREATE_SUSPENDED: u32 = 0x4;
    const CREATE_UNICODE_ENVIRONMENT: u32 = 0x400;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    const EXTENDED_STARTUPINFO_PRESENT: u32 = 0x0008_0000;
    const STARTF_USESTDHANDLES: u32 = 0x100;
    const HANDLE_FLAG_INHERIT: u32 = 1;
    const PROC_THREAD_ATTRIBUTE_HANDLE_LIST: usize = 0x0002_0002;
    const WAIT_OBJECT_0: u32 = 0;
    const GENERIC_READ: u32 = 0x8000_0000;
    const OPEN_EXISTING: u32 = 3;
    const ERROR_BROKEN_PIPE: u32 = 109;

    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(std::iter::once(0)).collect()
    }

    /// The child's environment block: ours with `req.env` merged over it (case-insensitive).
    fn env_block(req: &SpawnReq) -> Option<Vec<u16>> {
        if req.env.is_empty() {
            return None;
        }
        let mut vars: std::collections::BTreeMap<String, (String, String)> = std::env::vars_os()
            .filter_map(|(k, v)| Some((k.into_string().ok()?, v.into_string().ok()?)))
            .map(|(k, v)| (k.to_uppercase(), (k, v)))
            .collect();
        for (k, v) in &req.env {
            match v {
                Some(v) => {
                    vars.insert(k.to_uppercase(), (k.clone(), v.clone()));
                }
                None => {
                    vars.remove(&k.to_uppercase());
                }
            }
        }
        let mut b = Vec::new();
        for (k, v) in vars.values() {
            b.extend(k.encode_utf16());
            b.push('=' as u16);
            b.extend(v.encode_utf16());
            b.push(0);
        }
        if b.is_empty() {
            b.push(0);
        }
        b.push(0);
        Some(b)
    }

    pub fn spawn(req: &SpawnReq, cmd: &str, capture: bool) -> io::Result<(u32, ProcEnt)> {
        let mut cmdw = wide(cmd);
        let cwd = req.cwd.as_deref().map(wide);
        let env = env_block(req);
        // SAFETY: plain Win32 calls with owned, NUL-terminated buffers; every handle opened
        // here is closed on every path.
        unsafe {
            let mut six: STARTUPINFOEXW = std::mem::zeroed();
            six.si.cb = std::mem::size_of::<STARTUPINFOW>() as u32;
            let mut flags = CREATE_UNICODE_ENVIRONMENT | CREATE_SUSPENDED;
            if req.hide {
                flags |= CREATE_NO_WINDOW;
            }
            let mut pipe_r: HANDLE = std::ptr::null_mut();
            let mut pipe_w: HANDLE = std::ptr::null_mut();
            let mut nul: HANDLE = std::ptr::null_mut();
            let mut attr_buf: Vec<u64> = Vec::new();
            let mut list: [HANDLE; 2] = [std::ptr::null_mut(); 2];
            let cleanup = |r: HANDLE, w: HANDLE, n: HANDLE| {
                for h in [r, w, n] {
                    if !h.is_null() && h as isize != -1 {
                        CloseHandle(h);
                    }
                }
            };
            if capture {
                let sa = SECURITY_ATTRIBUTES { n_length: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32, sd: std::ptr::null_mut(), inherit: 1 };
                if CreatePipe(&mut pipe_r, &mut pipe_w, &sa, 64 * 1024) == 0 {
                    return Err(io::Error::last_os_error());
                }
                SetHandleInformation(pipe_r, HANDLE_FLAG_INHERIT, 0);
                nul = CreateFileW(wide("NUL").as_ptr(), GENERIC_READ, 3, &sa, OPEN_EXISTING, 0, std::ptr::null_mut());
                if nul as isize == -1 {
                    let e = io::Error::last_os_error();
                    cleanup(pipe_r, pipe_w, std::ptr::null_mut());
                    return Err(e);
                }
                six.si.dw_flags = STARTF_USESTDHANDLES;
                six.si.h_std_input = nul;
                six.si.h_std_output = pipe_w;
                six.si.h_std_error = pipe_w;
                list = [pipe_w, nul];
                let mut size = 0usize;
                InitializeProcThreadAttributeList(std::ptr::null_mut(), 1, 0, &mut size);
                attr_buf = vec![0u64; size.div_ceil(8)];
                let al = attr_buf.as_mut_ptr() as *mut c_void;
                if InitializeProcThreadAttributeList(al, 1, 0, &mut size) == 0
                    || UpdateProcThreadAttribute(al, 0, PROC_THREAD_ATTRIBUTE_HANDLE_LIST, list.as_ptr() as *const c_void, std::mem::size_of_val(&list), std::ptr::null_mut(), std::ptr::null_mut()) == 0
                {
                    let e = io::Error::last_os_error();
                    cleanup(pipe_r, pipe_w, nul);
                    return Err(e);
                }
                six.attrs = al;
                six.si.cb = std::mem::size_of::<STARTUPINFOEXW>() as u32;
                flags |= EXTENDED_STARTUPINFO_PRESENT;
            }
            let mut pi: PROCESS_INFORMATION = std::mem::zeroed();
            let ok = CreateProcessW(
                std::ptr::null(),
                cmdw.as_mut_ptr(),
                std::ptr::null(),
                std::ptr::null(),
                capture as i32,
                flags,
                env.as_ref().map_or(std::ptr::null(), |e| e.as_ptr() as *const c_void),
                cwd.as_ref().map_or(std::ptr::null(), |c| c.as_ptr()),
                &six.si,
                &mut pi,
            );
            let err = io::Error::last_os_error();
            if capture {
                DeleteProcThreadAttributeList(six.attrs);
                drop(attr_buf);
                cleanup(std::ptr::null_mut(), pipe_w, nul);
            }
            if ok == 0 {
                cleanup(pipe_r, std::ptr::null_mut(), std::ptr::null_mut());
                return Err(err);
            }
            // Own job (no kill-on-close): proc_kill takes the tree.
            let mut job = CreateJobObjectW(std::ptr::null(), std::ptr::null());
            if !job.is_null() && AssignProcessToJobObject(job, pi.h_process) == 0 {
                CloseHandle(job);
                job = std::ptr::null_mut();
            }
            ResumeThread(pi.h_thread);
            CloseHandle(pi.h_thread);
            let _ = &list;
            Ok((pi.pid, ProcEnt { handle: pi.h_process as isize, job: job as isize, pipe: pipe_r as isize, out: Vec::new(), eof: false }))
        }
    }

    /// `Some(code)` once the process has exited.
    pub fn exit_code(h: isize) -> Option<u32> {
        // SAFETY: our own process handle.
        unsafe {
            if WaitForSingleObject(h as HANDLE, 0) != WAIT_OBJECT_0 {
                return None;
            }
            let mut c = 0u32;
            GetExitCodeProcess(h as HANDLE, &mut c);
            Some(c)
        }
    }

    /// Wait up to `ms` for the process to exit.
    pub fn wait_exit(h: isize, ms: u32) -> bool {
        // SAFETY: our own process handle.
        unsafe { WaitForSingleObject(h as HANDLE, ms) == WAIT_OBJECT_0 }
    }

    pub fn kill(p: &ProcEnt) -> bool {
        // SAFETY: our own handles.
        unsafe {
            if p.job != 0 && TerminateJobObject(p.job as HANDLE, 1) != 0 {
                return true;
            }
            TerminateProcess(p.handle as HANDLE, 1) != 0
        }
    }

    pub fn kill_job(p: &ProcEnt) {
        if p.job != 0 {
            // SAFETY: our own job handle.
            unsafe { TerminateJobObject(p.job as HANDLE, 1) };
        }
    }

    /// Read whatever the pipe holds right now (never blocks).
    pub fn drain(p: &mut ProcEnt, tmp: &mut Vec<u8>) {
        if p.pipe == 0 || p.eof {
            return;
        }
        if tmp.len() < 64 * 1024 {
            tmp.resize(64 * 1024, 0);
        }
        // SAFETY: our own pipe handle; `tmp` is large enough for the read size.
        unsafe {
            loop {
                let mut avail = 0u32;
                if PeekNamedPipe(p.pipe as HANDLE, std::ptr::null_mut(), 0, std::ptr::null_mut(), &mut avail, std::ptr::null_mut()) == 0 {
                    if GetLastError() == ERROR_BROKEN_PIPE {
                        p.eof = true;
                    }
                    break;
                }
                if avail == 0 {
                    break;
                }
                let want = (avail as usize).min(tmp.len()) as u32;
                let mut got = 0u32;
                if ReadFile(p.pipe as HANDLE, tmp.as_mut_ptr() as *mut c_void, want, &mut got, std::ptr::null_mut()) == 0 || got == 0 {
                    break;
                }
                let room = CAPTURE_MAX.saturating_sub(p.out.len());
                p.out.extend_from_slice(&tmp[..(got as usize).min(room)]);
            }
        }
    }

    pub fn close(p: &ProcEnt) {
        // SAFETY: our own handles, closed once (the entry is dropped after).
        unsafe {
            for h in [p.handle, p.job, p.pipe] {
                if h != 0 {
                    CloseHandle(h as HANDLE);
                }
            }
        }
    }
}

#[cfg(not(windows))]
mod sys {
    use super::{ProcEnt, SpawnReq};
    use std::io;
    pub fn spawn(_req: &SpawnReq, _cmd: &str, _capture: bool) -> io::Result<(u32, ProcEnt)> {
        Err(io::Error::new(io::ErrorKind::Unsupported, "unsupported"))
    }
    pub fn exit_code(_h: isize) -> Option<u32> {
        Some(0)
    }
    pub fn wait_exit(_h: isize, _ms: u32) -> bool {
        true
    }
    pub fn kill(_p: &ProcEnt) -> bool {
        false
    }
    pub fn kill_job(_p: &ProcEnt) {}
    pub fn drain(_p: &mut ProcEnt, _tmp: &mut Vec<u8>) {}
    pub fn close(_p: &ProcEnt) {}
}
