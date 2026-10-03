//! Raw Lua 5.4.7 C API (the subset HSMPNative uses) and non-raising helpers.
//!
//! In the shipped DLLs these symbols resolve to HSMPNative's private copy of UE4SS's own
//! Lua 5.4.7 sources (built by CMake from RE-UE4SS `deps/first/LuaRaw`). In `cargo test`
//! they resolve to mlua's vendored Lua 5.4 (dev-dependency).
//!
//! Rules (they keep the dual-copy setup safe):
//! - never raise a Lua error from Rust (no `luaL_check*`, no `lua_error`): a longjmp across
//!   Rust frames is UB;
//! - never trigger metamethods on caller tables: only `lua_raw*` on tables;
//! - every table access first checks the type (`lua_rawgeti` on a non-table is UB).

#![allow(non_camel_case_types, non_snake_case, dead_code)]

use std::ffi::{c_char, c_int, c_void};

pub type lua_State = c_void;
pub type lua_Integer = i64;
pub type lua_Number = f64;
pub type lua_CFunction = unsafe extern "C-unwind" fn(*mut lua_State) -> c_int;

pub const LUA_TNONE: c_int = -1;
pub const LUA_TNIL: c_int = 0;
pub const LUA_TBOOLEAN: c_int = 1;
pub const LUA_TLIGHTUSERDATA: c_int = 2;
pub const LUA_TNUMBER: c_int = 3;
pub const LUA_TSTRING: c_int = 4;
pub const LUA_TTABLE: c_int = 5;
pub const LUA_TFUNCTION: c_int = 6;
pub const LUA_TTHREAD: c_int = 8;

/// `-LUAI_MAXSTACK - 1000` with LUAI_MAXSTACK = 1000000 (32-bit int, both copies).
pub const LUA_REGISTRYINDEX: c_int = -1_001_000;
pub const LUA_RIDX_MAINTHREAD: lua_Integer = 1;

extern "C-unwind" {
    pub fn lua_gettop(L: *mut lua_State) -> c_int;
    pub fn lua_settop(L: *mut lua_State, idx: c_int);
    pub fn lua_absindex(L: *mut lua_State, idx: c_int) -> c_int;
    pub fn lua_checkstack(L: *mut lua_State, n: c_int) -> c_int;
    pub fn lua_pushvalue(L: *mut lua_State, idx: c_int);
    pub fn lua_type(L: *mut lua_State, idx: c_int) -> c_int;
    pub fn lua_isinteger(L: *mut lua_State, idx: c_int) -> c_int;
    pub fn lua_tonumberx(L: *mut lua_State, idx: c_int, isnum: *mut c_int) -> lua_Number;
    pub fn lua_tointegerx(L: *mut lua_State, idx: c_int, isnum: *mut c_int) -> lua_Integer;
    pub fn lua_toboolean(L: *mut lua_State, idx: c_int) -> c_int;
    pub fn lua_tolstring(L: *mut lua_State, idx: c_int, len: *mut usize) -> *const c_char;
    pub fn lua_tothread(L: *mut lua_State, idx: c_int) -> *mut lua_State;
    pub fn lua_rawlen(L: *mut lua_State, idx: c_int) -> u64;
    pub fn lua_pushnil(L: *mut lua_State);
    pub fn lua_pushnumber(L: *mut lua_State, n: lua_Number);
    pub fn lua_pushinteger(L: *mut lua_State, n: lua_Integer);
    pub fn lua_pushlstring(L: *mut lua_State, s: *const c_char, len: usize) -> *const c_char;
    pub fn lua_pushboolean(L: *mut lua_State, b: c_int);
    pub fn lua_pushcclosure(L: *mut lua_State, f: lua_CFunction, n: c_int);
    pub fn lua_getglobal(L: *mut lua_State, name: *const c_char) -> c_int;
    pub fn lua_setglobal(L: *mut lua_State, name: *const c_char);
    pub fn lua_rawget(L: *mut lua_State, idx: c_int) -> c_int;
    pub fn lua_rawgeti(L: *mut lua_State, idx: c_int, n: lua_Integer) -> c_int;
    pub fn lua_createtable(L: *mut lua_State, narr: c_int, nrec: c_int);
    pub fn lua_rawset(L: *mut lua_State, idx: c_int);
    pub fn lua_rawseti(L: *mut lua_State, idx: c_int, n: lua_Integer);
    pub fn lua_next(L: *mut lua_State, idx: c_int) -> c_int;
}

#[inline]
pub unsafe fn pop(L: *mut lua_State, n: c_int) {
    unsafe { lua_settop(L, -n - 1) };
}

#[inline]
pub unsafe fn push_str(L: *mut lua_State, s: &str) {
    unsafe { lua_pushlstring(L, s.as_ptr() as *const c_char, s.len()) };
}

#[inline]
pub unsafe fn push_bytes(L: *mut lua_State, b: &[u8]) {
    unsafe { lua_pushlstring(L, b.as_ptr() as *const c_char, b.len()) };
}

/// `nil, msg` (2 results).
pub unsafe fn nil_err(L: *mut lua_State, msg: &str) -> c_int {
    unsafe {
        lua_pushnil(L);
        push_str(L, msg);
    }
    2
}

pub unsafe fn is_table(L: *mut lua_State, idx: c_int) -> bool {
    unsafe { lua_type(L, idx) == LUA_TTABLE }
}

/// Number argument (integers convert), `None` if absent or not a number.
pub unsafe fn arg_num(L: *mut lua_State, idx: c_int) -> Option<f64> {
    unsafe {
        if lua_type(L, idx) != LUA_TNUMBER {
            return None;
        }
        let mut ok = 0;
        let v = lua_tonumberx(L, idx, &mut ok);
        (ok != 0).then_some(v)
    }
}

/// Number argument with a default for nil / none; `None` if present but not a number.
pub unsafe fn opt_num(L: *mut lua_State, idx: c_int, def: f64) -> Option<f64> {
    unsafe {
        match lua_type(L, idx) {
            LUA_TNONE | LUA_TNIL => Some(def),
            _ => arg_num(L, idx),
        }
    }
}

/// Integer argument (a float with an exact integer value converts).
pub unsafe fn arg_int(L: *mut lua_State, idx: c_int) -> Option<i64> {
    unsafe {
        if lua_type(L, idx) != LUA_TNUMBER {
            return None;
        }
        let mut ok = 0;
        let v = lua_tointegerx(L, idx, &mut ok);
        (ok != 0).then_some(v)
    }
}

pub unsafe fn opt_int(L: *mut lua_State, idx: c_int, def: i64) -> Option<i64> {
    unsafe {
        match lua_type(L, idx) {
            LUA_TNONE | LUA_TNIL => Some(def),
            _ => arg_int(L, idx),
        }
    }
}

/// String argument bytes (only real strings, never number coercion).
pub unsafe fn arg_bytes<'a>(L: *mut lua_State, idx: c_int) -> Option<&'a [u8]> {
    unsafe {
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
}

pub unsafe fn arg_str<'a>(L: *mut lua_State, idx: c_int) -> Option<&'a str> {
    unsafe { arg_bytes(L, idx).and_then(|b| std::str::from_utf8(b).ok()) }
}

/// `t[key] = <value on top>` without metamethods; pops the value. `t` absolute index.
#[inline]
pub unsafe fn rawset_str(L: *mut lua_State, t: c_int, key: &str) {
    unsafe {
        push_str(L, key);
        lua_pushvalue(L, -2);
        lua_rawset(L, t);
        pop(L, 1);
    }
}

pub unsafe fn set_num(L: *mut lua_State, t: c_int, key: &str, v: f64) {
    unsafe {
        push_str(L, key);
        lua_pushnumber(L, v);
        lua_rawset(L, t);
    }
}

pub unsafe fn set_int(L: *mut lua_State, t: c_int, key: &str, v: i64) {
    unsafe {
        push_str(L, key);
        lua_pushinteger(L, v);
        lua_rawset(L, t);
    }
}

pub unsafe fn set_bool(L: *mut lua_State, t: c_int, key: &str, v: bool) {
    unsafe {
        push_str(L, key);
        lua_pushboolean(L, v as c_int);
        lua_rawset(L, t);
    }
}

pub unsafe fn set_str(L: *mut lua_State, t: c_int, key: &str, v: &str) {
    unsafe {
        push_str(L, key);
        push_str(L, v);
        lua_rawset(L, t);
    }
}

pub unsafe fn set_nil(L: *mut lua_State, t: c_int, key: &str) {
    unsafe {
        push_str(L, key);
        lua_pushnil(L);
        lua_rawset(L, t);
    }
}

/// Push `t[key]` (raw). Returns its type.
pub unsafe fn rawget_str(L: *mut lua_State, t: c_int, key: &str) -> c_int {
    unsafe {
        push_str(L, key);
        lua_rawget(L, t)
    }
}

/// Push the table `t[key]`, creating (and storing) a fresh one if it is not a table.
/// Leaves the sub-table on the stack.
pub unsafe fn subtable(L: *mut lua_State, t: c_int, key: &str, narr: c_int, nrec: c_int) {
    unsafe {
        if rawget_str(L, t, key) != LUA_TTABLE {
            pop(L, 1);
            lua_createtable(L, narr, nrec);
            lua_pushvalue(L, -1);
            rawset_str(L, t, key);
        }
    }
}

/// Overwrite `t[1..=vals.len()]` with numbers and clear `t[n+1..=old_len]`. `t` absolute.
pub unsafe fn fill_array(L: *mut lua_State, t: c_int, vals: impl Iterator<Item = f64>) {
    unsafe {
        let mut i: i64 = 0;
        for v in vals {
            i += 1;
            lua_pushnumber(L, v);
            lua_rawseti(L, t, i);
        }
        trim_array(L, t, i);
    }
}

/// Set `t[n+1..]` to nil (up to the current raw length).
pub unsafe fn trim_array(L: *mut lua_State, t: c_int, n: i64) {
    unsafe {
        let old = lua_rawlen(L, t) as i64;
        let mut j = old;
        while j > n {
            lua_pushnil(L);
            lua_rawseti(L, t, j);
            j -= 1;
        }
    }
}

/// Read `t[i]` as a number (raw); `None` if not a number. `t` absolute.
#[inline]
pub unsafe fn geti_num(L: *mut lua_State, t: c_int, i: i64) -> Option<f64> {
    unsafe {
        let ty = lua_rawgeti(L, t, i);
        let r = if ty == LUA_TNUMBER {
            let mut ok = 0;
            let v = lua_tonumberx(L, -1, &mut ok);
            (ok != 0).then_some(v)
        } else {
            None
        };
        pop(L, 1);
        r
    }
}

/// The main thread of `L`'s state (stable identity of a Lua state / mod).
pub unsafe fn main_thread(L: *mut lua_State) -> usize {
    unsafe {
        let ty = lua_rawgeti(L, LUA_REGISTRYINDEX, LUA_RIDX_MAINTHREAD);
        let p = if ty == LUA_TTHREAD { lua_tothread(L, -1) } else { L };
        pop(L, 1);
        p as usize
    }
}

/// Read `t[first..first+out.len()]` as numbers with one push per value, one conversion per
/// value and one pop for the batch (every Lua API call takes UE4SS's lua_lock). `false` if
/// any value is not a number. `t` absolute.
pub unsafe fn geti_nums(L: *mut lua_State, t: c_int, first: i64, out: &mut [f64]) -> bool {
    unsafe {
        if lua_checkstack(L, out.len() as c_int + 2) == 0 {
            return false;
        }
        let top = lua_gettop(L);
        for i in 0..out.len() {
            lua_rawgeti(L, t, first + i as i64);
        }
        let mut ok = true;
        for (i, o) in out.iter_mut().enumerate() {
            let mut isnum = 0;
            *o = lua_tonumberx(L, top + 1 + i as c_int, &mut isnum);
            if isnum == 0 {
                ok = false;
                break;
            }
        }
        lua_settop(L, top);
        ok
    }
}
