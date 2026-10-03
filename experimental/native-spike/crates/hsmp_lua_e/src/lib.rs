//! Option E: a Lua 5.4 C module written in Rust, loaded by UE4SS's Lua with
//! `require("hsmp_lua")` (UE4SS adds `<mod>/Scripts/?.dll` to package.cpath).
//!
//! UE4SS links Lua statically and exports no `lua_*` symbols, so this module
//! is linked (by CMake) against a private copy of the *same* Lua 5.4.7 sources
//! and luaconf.h that UE4SS was built from (RE-UE4SS deps/first/LuaRaw, compiled
//! as C). Lua 5.4 tolerates two copies of its code operating on one state as
//! long as the struct layouts match, which they do by construction. The
//! UE4SS-specific lua_lock is forwarded to UE4SS's own critical section by
//! `hsmp_luauser.c` when the exact UE4SS build is detected.
//!
//! Rules this file follows (they are what make the dual-copy setup safe):
//! - never raise a Lua error from Rust (no luaL_check*/luaL_error): a longjmp
//!   across Rust frames is UB. Bad input returns `nil, "message"` instead;
//! - never create or close states; only push/read values on the caller's stack;
//! - every entry point is wrapped in catch_unwind.

#![allow(non_camel_case_types, non_snake_case)]

use std::ffi::{c_char, c_int, c_void, CStr};
use std::panic::{catch_unwind, AssertUnwindSafe};

use hsmp_core_stub as core;

type lua_State = c_void;
type lua_Integer = i64;
type lua_Number = f64;
type lua_CFunction = unsafe extern "C" fn(*mut lua_State) -> c_int;

const LUA_TNONE: c_int = -1;
const LUA_TNIL: c_int = 0;
const LUA_TNUMBER: c_int = 3;
const LUA_TSTRING: c_int = 4;

extern "C" {
    fn lua_gettop(L: *mut lua_State) -> c_int;
    fn lua_type(L: *mut lua_State, idx: c_int) -> c_int;
    fn lua_createtable(L: *mut lua_State, narr: c_int, nrec: c_int);
    fn lua_pushcclosure(L: *mut lua_State, f: lua_CFunction, n: c_int);
    fn lua_setfield(L: *mut lua_State, idx: c_int, k: *const c_char);
    fn lua_rawseti(L: *mut lua_State, idx: c_int, n: lua_Integer);
    fn lua_pushlstring(L: *mut lua_State, s: *const c_char, len: usize) -> *const c_char;
    fn lua_pushinteger(L: *mut lua_State, n: lua_Integer);
    fn lua_pushnumber(L: *mut lua_State, n: lua_Number);
    fn lua_pushboolean(L: *mut lua_State, b: c_int);
    fn lua_pushnil(L: *mut lua_State);
    fn lua_tolstring(L: *mut lua_State, idx: c_int, len: *mut usize) -> *const c_char;
    fn lua_tointegerx(L: *mut lua_State, idx: c_int, isnum: *mut c_int) -> lua_Integer;

    // hsmp_luauser.c
    fn hsmp_lua_lock_mode() -> *const c_char;
}

#[cfg(windows)]
extern "system" {
    fn GetModuleHandleExW(flags: u32, name: *const u16, module: *mut *mut c_void) -> i32;
}

unsafe fn push_str(L: *mut lua_State, s: &str) {
    lua_pushlstring(L, s.as_ptr() as *const c_char, s.len());
}

unsafe fn push_bytes(L: *mut lua_State, b: &[u8]) {
    lua_pushlstring(L, b.as_ptr() as *const c_char, b.len());
}

unsafe fn nil_err(L: *mut lua_State, msg: &str) -> c_int {
    lua_pushnil(L);
    push_str(L, msg);
    2
}

unsafe fn arg_bytes<'a>(L: *mut lua_State, idx: c_int) -> Option<&'a [u8]> {
    if lua_type(L, idx) != LUA_TSTRING {
        return None;
    }
    let mut len = 0usize;
    let p = lua_tolstring(L, idx, &mut len);
    if p.is_null() {
        None
    } else {
        Some(std::slice::from_raw_parts(p as *const u8, len))
    }
}

unsafe fn arg_str<'a>(L: *mut lua_State, idx: c_int, default: &'a str) -> Option<&'a str> {
    match lua_type(L, idx) {
        LUA_TNONE | LUA_TNIL => Some(default),
        LUA_TSTRING => arg_bytes(L, idx).and_then(|b| std::str::from_utf8(b).ok()),
        _ => None,
    }
}

unsafe fn arg_int(L: *mut lua_State, idx: c_int, default: i64) -> Option<i64> {
    match lua_type(L, idx) {
        LUA_TNONE | LUA_TNIL => Some(default),
        LUA_TNUMBER => {
            let mut ok = 0;
            let v = lua_tointegerx(L, idx, &mut ok);
            (ok != 0).then_some(v)
        }
        _ => None,
    }
}

fn err_name(code: i32) -> &'static str {
    match code {
        core::E_NOT_RUNNING => "not running",
        core::E_ALREADY_RUNNING => "already running",
        core::E_BAD_ARG => "bad argument",
        core::E_IO => "socket error",
        core::E_FULL => "queue full",
        core::E_TOO_BIG => "message too big",
        core::E_BUSY => "busy (concurrent caller)",
        core::E_EMPTY => "empty",
        core::E_TOO_SMALL => "buffer too small",
        _ => "internal error",
    }
}

/// Wraps a binding so a Rust panic becomes `nil, "panic"` instead of UB.
macro_rules! lua_fn {
    ($name:ident, |$L:ident| $body:block) => {
        unsafe extern "C" fn $name($L: *mut lua_State) -> c_int {
            match catch_unwind(AssertUnwindSafe(|| unsafe { $body })) {
                Ok(n) => n,
                Err(_) => unsafe { nil_err($L, "hsmp_lua: rust panic") },
            }
        }
    };
}

lua_fn!(l_ping, |L| {
    let lock = CStr::from_ptr(hsmp_lua_lock_mode()).to_string_lossy();
    let s = format!(
        "pong from hsmp_lua (option E) | {} | lua 5.4 dual-copy | lock={} | top={}",
        core::VERSION,
        lock,
        lua_gettop(L)
    );
    push_str(L, &s);
    1
});

lua_fn!(l_version, |L| {
    push_str(L, core::VERSION);
    1
});

lua_fn!(l_now_us, |L| {
    lua_pushinteger(L, core::now_us() as i64);
    1
});

lua_fn!(l_lock_mode, |L| {
    let lock = CStr::from_ptr(hsmp_lua_lock_mode()).to_string_lossy();
    push_str(L, &lock);
    1
});

// net_start(bind = "127.0.0.1:0", peer = "self") -> port | nil, err
lua_fn!(l_net_start, |L| {
    let (Some(bind), Some(peer)) = (arg_str(L, 1, "127.0.0.1:0"), arg_str(L, 2, "self")) else {
        return nil_err(L, "net_start(bind:string?, peer:string?)");
    };
    match core::start(bind, peer) {
        Ok(port) => {
            lua_pushinteger(L, port as i64);
            1
        }
        Err(e) => nil_err(L, err_name(e)),
    }
});

lua_fn!(l_net_stop, |L| {
    core::stop();
    lua_pushboolean(L, 1);
    1
});

// net_send(bytes) -> true | nil, err
lua_fn!(l_net_send, |L| {
    let Some(b) = arg_bytes(L, 1) else { return nil_err(L, "net_send(data:string)") };
    match core::send(b) {
        core::OK => {
            lua_pushboolean(L, 1);
            1
        }
        e => nil_err(L, err_name(e)),
    }
});

// net_poll(max = 64) -> { msg1, msg2, ... }  (never blocks)
lua_fn!(l_net_poll, |L| {
    let Some(max) = arg_int(L, 1, 64) else { return nil_err(L, "net_poll(max:int?)") };
    lua_createtable(L, 0, 0);
    let mut buf = [0u8; core::spsc::SLOT_BYTES];
    let mut i = 0i64;
    while i < max.clamp(0, 4096) {
        let n = core::poll(&mut buf);
        if n < 0 {
            break;
        }
        push_bytes(L, &buf[..n as usize]);
        i += 1;
        lua_rawseti(L, -2, i);
    }
    1
});

lua_fn!(l_net_stats, |L| {
    let s = core::stats();
    lua_createtable(L, 0, 11);
    let fields: [(&[u8], i64); 11] = [
        (b"sent\0", s.sent as i64),
        (b"recv\0", s.recv as i64),
        (b"send_err\0", s.send_err as i64),
        (b"recv_err\0", s.recv_err as i64),
        (b"drop_in_full\0", s.drop_in_full as i64),
        (b"drop_out_full\0", s.drop_out_full as i64),
        (b"loops\0", s.loops as i64),
        (b"inq_len\0", s.inq_len as i64),
        (b"outq_len\0", s.outq_len as i64),
        (b"running\0", s.running as i64),
        (b"local_port\0", s.local_port as i64),
    ];
    for (k, v) in fields {
        lua_pushinteger(L, v);
        lua_setfield(L, -2, k.as_ptr() as *const c_char);
    }
    1
});

/// Keeps this DLL loaded for the life of the process. Without it, a Lua state
/// teardown (UE4SS "restart mods") FreeLibrary()s the module while the net
/// thread is still executing its code.
fn pin_self() -> bool {
    #[cfg(windows)]
    unsafe {
        const FROM_ADDRESS: u32 = 0x4;
        const PIN: u32 = 0x1;
        let mut h: *mut c_void = std::ptr::null_mut();
        let addr = pin_self as *const () as *const u16;
        return GetModuleHandleExW(FROM_ADDRESS | PIN, addr, &mut h) != 0;
    }
    #[allow(unreachable_code)]
    false
}

/// `require("hsmp_lua")` entry point.
#[no_mangle]
pub unsafe extern "C" fn luaopen_hsmp_lua(L: *mut lua_State) -> c_int {
    let r = catch_unwind(AssertUnwindSafe(|| unsafe {
        let pinned = pin_self();
        let funcs: [(&[u8], lua_CFunction); 9] = [
            (b"ping\0", l_ping),
            (b"version\0", l_version),
            (b"now_us\0", l_now_us),
            (b"lock_mode\0", l_lock_mode),
            (b"net_start\0", l_net_start),
            (b"net_stop\0", l_net_stop),
            (b"net_send\0", l_net_send),
            (b"net_poll\0", l_net_poll),
            (b"net_stats\0", l_net_stats),
        ];
        lua_createtable(L, 0, funcs.len() as c_int + 2);
        for (name, f) in funcs.iter() {
            lua_pushcclosure(L, *f, 0);
            lua_setfield(L, -2, name.as_ptr() as *const c_char);
        }
        lua_pushboolean(L, pinned as c_int);
        lua_setfield(L, -2, b"pinned\0".as_ptr() as *const c_char);
        lua_pushnumber(L, core::spsc::SLOT_BYTES as f64);
        lua_setfield(L, -2, b"max_payload\0".as_ptr() as *const c_char);
        1
    }));
    match r {
        Ok(n) => n,
        Err(_) => {
            lua_pushnil(L);
            1
        }
    }
}
