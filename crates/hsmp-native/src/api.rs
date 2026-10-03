//! The C entry points: one non-raising Lua C function per API call, the registration
//! functions for option F (`hsmp_native_open`, called by the C++ mod) and option E
//! (`luaopen_hsmp_lua`), and the guards every call goes through:
//! - `catch_unwind`: a Rust panic never crosses into Lua; it poisons IPC for the session
//!   (`nil, "disabled"` from then on);
//! - the in-process try-lock (contention = a shim bypass = `nil, "busy"`);
//! - the game-thread check (captured at the first `frame()`; other threads get
//!   `nil, "wrong thread"`, counted).

use std::ffi::{c_char, c_int, CStr};
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::{Mutex, OnceLock, TryLockError};

use crate::lua::*;
use crate::native::Native;

fn global() -> &'static Mutex<Native> {
    static G: OnceLock<Mutex<Native>> = OnceLock::new();
    G.get_or_init(|| Mutex::new(Native::new()))
}

/// Which registration path built the table ("F" or "E").
static IMPL: OnceLock<&'static str> = OnceLock::new();

#[derive(Clone, Copy, PartialEq, Eq)]
enum Guard {
    /// Needs the game thread once it is known.
    GameThread,
    /// Any thread (pure functions, `ipc_open` before the first frame).
    Any,
}

fn with_native(L: *mut lua_State, guard: Guard, f: impl FnOnce(&mut Native) -> c_int) -> c_int {
    let g = global();
    let mut n = match g.try_lock() {
        Ok(n) => n,
        Err(TryLockError::Poisoned(p)) => p.into_inner(),
        // SAFETY: plain pushes on the caller's stack.
        Err(TryLockError::WouldBlock) => return unsafe { nil_err(L, "busy") },
    };
    if n.poisoned {
        // SAFETY: as above.
        return unsafe { nil_err(L, "disabled") };
    }
    if guard == Guard::GameThread {
        if let Some(t) = n.game_thread {
            if t != std::thread::current().id() {
                n.count_wrong_thread();
                // SAFETY: as above.
                return unsafe { nil_err(L, "wrong thread") };
            }
        }
    }
    f(&mut n)
}

fn on_panic() {
    let mut n = match global().try_lock() {
        Ok(n) => n,
        Err(TryLockError::Poisoned(p)) => p.into_inner(),
        Err(TryLockError::WouldBlock) => return,
    };
    n.poison("hsmp-native: rust panic");
}

macro_rules! entry {
    ($($name:ident => $guard:expr, |$n:ident, $L:ident| $body:expr;)*) => {$(
        unsafe extern "C-unwind" fn $name($L: *mut lua_State) -> c_int {
            let r = catch_unwind(AssertUnwindSafe(|| with_native($L, $guard, |$n| {
                #[allow(unused_unsafe)]
                unsafe { $body }
            })));
            match r {
                Ok(v) => v,
                Err(_) => {
                    on_panic();
                    // SAFETY: plain pushes.
                    unsafe { nil_err($L, "disabled") }
                }
            }
        }
    )*};
}

entry! {
    l_ipc_open => Guard::GameThread, |n, L| n.ipc_open(L);
    l_ipc_info => Guard::GameThread, |n, L| n.ipc_info(L);
    l_frame => Guard::GameThread, |n, L| n.frame(L);
    l_flags => Guard::GameThread, |n, L| { lua_pushinteger(L, n.last_flags() as i64); 1 };
    l_world_leaving => Guard::GameThread, |n, L| n.world_leaving(L);
    l_world_ready => Guard::GameThread, |n, L| n.world_ready(L);
    l_put_root => Guard::GameThread, |n, L| n.put_root(L);
    l_put_weapon => Guard::GameThread, |n, L| n.put_weapon(L);
    l_put_pose => Guard::GameThread, |n, L| n.put_pose(L);
    l_put_lead => Guard::GameThread, |n, L| n.put_lead(L);
    l_put => Guard::GameThread, |n, L| n.r_put(L);
    l_get => Guard::GameThread, |n, L| n.r_get(L);
    l_peers => Guard::GameThread, |n, L| n.peers(L);
    l_peer_play => Guard::GameThread, |n, L| n.peer_play(L);
    l_peer => Guard::GameThread, |n, L| n.r_peer(L);
    l_send => Guard::GameThread, |n, L| n.r_send(L);
    l_subscribe => Guard::GameThread, |n, L| n.subscribe(L);
    l_poll => Guard::GameThread, |n, L| n.poll(L);
    l_bus_put => Guard::GameThread, |n, L| n.r_bus_put(L);
    l_bus_get => Guard::GameThread, |n, L| n.r_bus_get(L);
    l_dev_poll => Guard::GameThread, |n, L| n.dev_poll(L);
    l_spawn => Guard::GameThread, |n, L| n.spawn(L);
    l_spawn_capture => Guard::GameThread, |n, L| n.spawn_capture(L);
    l_capture_poll => Guard::GameThread, |n, L| n.capture_poll(L);
    l_proc_alive => Guard::GameThread, |n, L| n.proc_alive(L);
    l_proc_exit_code => Guard::GameThread, |n, L| n.proc_exit_code(L);
    l_proc_kill => Guard::GameThread, |n, L| n.proc_kill(L);
    l_current_pid => Guard::Any, |n, L| n.current_pid(L);
    l_sample_config => Guard::GameThread, |n, L| n.sample_config(L);
    l_sample_local => Guard::GameThread, |n, L| n.sample_local(L);
    l_sample_status => Guard::GameThread, |n, L| n.sample_status(L);
    l_servo_config => Guard::GameThread, |n, L| n.servo_config(L);
    l_servo_bodies => Guard::GameThread, |n, L| n.servo_bodies(L);
    l_servo_weapon => Guard::GameThread, |n, L| n.servo_weapon(L);
    l_servo_status => Guard::GameThread, |n, L| n.servo_status(L);
    l_neutralise_config => Guard::GameThread, |n, L| n.neutralise_config(L);
    l_neutralise => Guard::GameThread, |n, L| n.neutralise(L);
    l_neutralise_status => Guard::GameThread, |n, L| n.neutralise_status(L);
    l_now_us => Guard::Any, |n, L| { lua_pushinteger(L, n.now_us()); 1 };
    l_thread_ok => Guard::Any, |n, L| {
        let ok = n.game_thread.is_none_or(|t| t == std::thread::current().id());
        lua_pushboolean(L, ok as c_int);
        1
    };
}

/// `abi()` is pure: no lock, no thread rule.
unsafe extern "C-unwind" fn l_abi(L: *mut lua_State) -> c_int {
    let r = catch_unwind(AssertUnwindSafe(|| unsafe {
        lua_pushinteger(L, hsmp_ipc::ABI_MAJOR as i64);
        lua_pushinteger(L, hsmp_ipc::ABI_MINOR as i64);
        push_str(L, &format!("{:016x}", hsmp_ipc::LAYOUT_HASH));
        3
    }));
    r.unwrap_or_else(|_| unsafe { nil_err(L, "disabled") })
}

const FUNCS: &[(&str, lua_CFunction)] = &[
    ("ipc_open", l_ipc_open),
    ("ipc_info", l_ipc_info),
    ("frame", l_frame),
    ("flags", l_flags),
    ("world_leaving", l_world_leaving),
    ("world_ready", l_world_ready),
    ("put_root", l_put_root),
    ("put_weapon", l_put_weapon),
    ("put_pose", l_put_pose),
    ("put_lead", l_put_lead),
    ("put", l_put),
    ("get", l_get),
    ("peers", l_peers),
    ("peer_play", l_peer_play),
    ("peer", l_peer),
    ("send", l_send),
    ("subscribe", l_subscribe),
    ("poll", l_poll),
    ("bus_put", l_bus_put),
    ("bus_get", l_bus_get),
    ("dev_poll", l_dev_poll),
    ("spawn", l_spawn),
    ("spawn_capture", l_spawn_capture),
    ("capture_poll", l_capture_poll),
    ("proc_alive", l_proc_alive),
    ("proc_exit_code", l_proc_exit_code),
    ("proc_kill", l_proc_kill),
    ("current_pid", l_current_pid),
    ("sample_config", l_sample_config),
    ("sample_local", l_sample_local),
    ("sample_status", l_sample_status),
    ("servo_config", l_servo_config),
    ("servo_bodies", l_servo_bodies),
    ("servo_weapon", l_servo_weapon),
    ("servo_status", l_servo_status),
    ("neutralise_config", l_neutralise_config),
    ("neutralise", l_neutralise),
    ("neutralise_status", l_neutralise_status),
    ("abi", l_abi),
    ("now_us", l_now_us),
    ("thread_ok", l_thread_ok),
];

/// Push a fresh API table and register `L`'s state for events.
unsafe fn push_api(L: *mut lua_State, impl_tag: &str) {
    unsafe {
        lua_createtable(L, 0, FUNCS.len() as c_int + 4);
        let t = lua_gettop(L);
        for (name, f) in FUNCS {
            lua_pushcclosure(L, *f, 0);
            rawset_str(L, t, name);
        }
        set_str(L, t, "_impl", impl_tag);
        set_str(L, t, "version", env!("CARGO_PKG_VERSION"));
        set_int(L, t, "ABI_MAJOR", hsmp_ipc::ABI_MAJOR as i64);
        set_str(L, t, "LAYOUT_HASH", &format!("{:016x}", hsmp_ipc::LAYOUT_HASH));
        let main = main_thread(L);
        if let Ok(mut n) = global().lock() {
            n.register_state(main);
        }
    }
}

/// Option F: register the global `HSMPNative` into a Lua state (called by the C++ mod from
/// `on_lua_start`, before the mod's main.lua runs). `mod_name` may be null. Returns 1 on
/// success, 0 on failure. Leaves the stack balanced.
///
/// # Safety
/// `L` must be a valid Lua state of the same Lua 5.4.7 ABI; `mod_name` null or a C string.
#[no_mangle]
pub unsafe extern "C-unwind" fn hsmp_native_open(L: *mut lua_State, mod_name: *const c_char) -> c_int {
    let r = catch_unwind(AssertUnwindSafe(|| unsafe {
        let _ = IMPL.set("F");
        let top = lua_gettop(L);
        if lua_checkstack(L, 8) == 0 {
            return 0;
        }
        push_api(L, "F");
        lua_setglobal(L, c"HSMPNative".as_ptr());
        lua_settop(L, top);
        if !mod_name.is_null() {
            let name = CStr::from_ptr(mod_name).to_string_lossy().into_owned();
            if let Ok(mut n) = global().lock() {
                n.note_mod(&name);
            }
        }
        1
    }));
    r.unwrap_or(0)
}

/// Option E: `require("hsmp_lua")` / `package.loadlib(...)`. If option F already gave this
/// state an `HSMPNative` table, the same table is returned, so one process never has two
/// IPC clients.
///
/// # Safety
/// `L` must be a valid Lua state of the same Lua 5.4.7 ABI.
#[no_mangle]
pub unsafe extern "C-unwind" fn luaopen_hsmp_lua(L: *mut lua_State) -> c_int {
    let r = catch_unwind(AssertUnwindSafe(|| unsafe {
        if lua_checkstack(L, 8) == 0 {
            return 0;
        }
        if lua_getglobal(L, c"HSMPNative".as_ptr()) == LUA_TTABLE {
            return 1;
        }
        pop(L, 1);
        pin_self();
        let _ = IMPL.set("E");
        push_api(L, "E");
        1
    }));
    match r {
        Ok(n) => n,
        Err(_) => unsafe {
            lua_pushnil(L);
            1
        },
    }
}

/// Keep this module loaded for the life of the process (a Lua state teardown would
/// otherwise FreeLibrary the E module while the process-global state still lives in it).
fn pin_self() {
    #[cfg(windows)]
    unsafe {
        extern "system" {
            fn GetModuleHandleExW(flags: u32, name: *const u16, module: *mut *mut core::ffi::c_void) -> i32;
        }
        const FROM_ADDRESS: u32 = 0x4;
        const PIN: u32 = 0x1;
        let mut h: *mut core::ffi::c_void = std::ptr::null_mut();
        let addr = pin_self as *const () as *const u16;
        GetModuleHandleExW(FROM_ADDRESS | PIN, addr, &mut h);
    }
}

/// Test hook: forget the game thread (tests run several "game threads" in sequence).
#[doc(hidden)]
pub fn reset_game_thread_for_tests() {
    if let Ok(mut n) = global().lock() {
        n.game_thread = None;
    }
}

/// Test hook: add records and slots that are not in the schema (the slots live outside the
/// segment, e.g. leaked `SeqSlot<Stamped<T>>`s). Used by `tests/records.rs` for shapes the
/// real schema does not have yet (Str / Bool / struct rows / every slot form).
#[doc(hidden)]
pub fn register_test_schema(records: &'static [hsmp_ipc::schema::RecordInfo], slots: &'static [crate::records::TestSlot]) {
    if let Ok(mut n) = global().lock() {
        n.rec.set_test_schema(records, slots);
    }
}
