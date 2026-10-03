//! Lua table <-> record bytes, generic over [`TypeDesc`]. The executable spec
//! is `tools/hsmp-tools/lua-tests/lib/hsmp_native_records.lua`; this file follows it rule by
//! rule and `lua-tests/lib/records_conformance.lua` checks both against each other:
//!
//! - a struct is a table keyed by its Rust field names; `_...` padding fields never appear;
//! - integers: a Lua integer wraps to the field width, a float truncates toward zero and
//!   saturates (NaN -> 0; Rust `as`), `true` = 1 / `false` = 0; a u64 above 2^63-1 reads
//!   back negative (the same 64 bits);
//! - floats must be finite, also after rounding to f32 (else `bad:<field>`); `true` = 1.0;
//! - arrays are 1-based Lua arrays (extra elements ignored, missing ones default);
//! - `Str<N>`: a string (a number is converted with `tostring`), truncated on a UTF-8
//!   boundary to N bytes; it must then be valid UTF-8 without NUL bytes (else `bad:<field>`);
//! - `Bool`: a boolean (nil = false, a number = `~= 0`, anything else = truthiness);
//! - a missing field is 0 / "" / false / a zeroed struct;
//! - a variable record carries `t.rows`; the count field is set from `#t.rows` (raw length);
//!   more than `max_rows` -> `too_big`; `rows` not a table -> `bad:rows`; a row that is not a
//!   table -> `bad:<RowType>`.
//!
//! Field names in errors are the innermost field's name (an array element reports the array
//! field, a row reports the row type's name). Nothing here raises a Lua error, allocates on
//! the Lua side when writing, or calls a metamethod (raw access only).

use std::ffi::c_int;

use hsmp_ipc::layout::TypeDesc;
use hsmp_ipc::record::Invalid;
use hsmp_ipc::schema::RecordInfo;

use crate::lua::*;

/// Why a table could not become a record.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MErr {
    /// Not a table (`"bad"`).
    Bad,
    /// A field of the wrong type / a non-finite float / a bad string (`"bad:<field>"`).
    Field(&'static str),
    /// More rows than the record's capacity, or larger than the slot / ring record.
    TooBig,
    /// The record's own validation refused it (`"bad:<field>"` or `"bad:<reason>"`).
    Check(Invalid),
}

impl MErr {
    /// The Lua error string.
    pub fn message(&self) -> String {
        match self {
            MErr::Bad => "bad".to_string(),
            MErr::Field(f) => format!("bad:{}", f),
            MErr::TooBig => "too_big".to_string(),
            MErr::Check(i) => format!("bad:{}", i.field().unwrap_or(i.as_str())),
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum P {
    U8,
    U16,
    U32,
    U64,
    I8,
    I16,
    I32,
    I64,
    F32,
    F64,
}

#[inline]
fn prim(name: &str) -> P {
    match name.as_bytes() {
        b"u8" => P::U8,
        b"u16" => P::U16,
        b"u32" => P::U32,
        b"u64" => P::U64,
        b"i8" => P::I8,
        b"i16" => P::I16,
        b"i32" => P::I32,
        b"i64" => P::I64,
        b"f32" => P::F32,
        _ => P::F64,
    }
}

/// `s` cut to at most `cap` bytes, backing off continuation bytes (the spec's `trunc_utf8`).
#[inline]
pub fn trunc_utf8(s: &[u8], cap: usize) -> &[u8] {
    if s.len() <= cap {
        return s;
    }
    let mut n = cap;
    while n > 0 {
        let b = s[n];
        if !(0x80..0xC0).contains(&b) {
            break;
        }
        n -= 1;
    }
    &s[..n]
}

#[inline]
fn put_int(b: &mut [u8], p: P, v: i64) {
    let le = v.to_le_bytes();
    let n = match p {
        P::U8 | P::I8 => 1,
        P::U16 | P::I16 => 2,
        P::U32 | P::I32 => 4,
        _ => 8,
    };
    b[..n].copy_from_slice(&le[..n]);
}

/// A float converted like Rust `as` (truncate toward zero, saturate, NaN -> 0), as the bits
/// of the field (wrapping is then a byte truncation).
#[inline]
fn float_to_int(p: P, f: f64) -> i64 {
    match p {
        P::U8 => f as u8 as i64,
        P::U16 => f as u16 as i64,
        P::U32 => f as u32 as i64,
        P::U64 => f as u64 as i64,
        P::I8 => f as i8 as i64,
        P::I16 => f as i16 as i64,
        P::I32 => f as i32 as i64,
        _ => f as i64,
    }
}

/// Write the Lua value on top of the stack as `d` into `b` (`b` is zeroed, `d.size()` long).
/// Leaves the stack as it found it on success; on error the caller restores the top.
unsafe fn write_val(L: *mut lua_State, d: &'static TypeDesc, b: &mut [u8], field: &'static str) -> Result<(), MErr> {
    unsafe {
        let ty = lua_type(L, -1);
        match d {
            TypeDesc::Prim { name, .. } => {
                let p = prim(name);
                if p == P::F32 || p == P::F64 {
                    let v = match ty {
                        LUA_TNIL | LUA_TNONE => return Ok(()),
                        LUA_TBOOLEAN => lua_toboolean(L, -1) as f64,
                        LUA_TNUMBER => lua_tonumberx(L, -1, std::ptr::null_mut()),
                        _ => return Err(MErr::Field(field)),
                    };
                    if !v.is_finite() {
                        return Err(MErr::Field(field));
                    }
                    if p == P::F32 {
                        let x = v as f32;
                        if !x.is_finite() {
                            return Err(MErr::Field(field));
                        }
                        b[..4].copy_from_slice(&x.to_le_bytes());
                    } else {
                        b[..8].copy_from_slice(&v.to_le_bytes());
                    }
                    return Ok(());
                }
                let v = match ty {
                    LUA_TNIL | LUA_TNONE => return Ok(()),
                    LUA_TBOOLEAN => lua_toboolean(L, -1) as i64,
                    LUA_TNUMBER => {
                        if lua_isinteger(L, -1) != 0 {
                            lua_tointegerx(L, -1, std::ptr::null_mut())
                        } else {
                            float_to_int(p, lua_tonumberx(L, -1, std::ptr::null_mut()))
                        }
                    }
                    _ => return Err(MErr::Field(field)),
                };
                put_int(b, p, v);
                Ok(())
            }
            TypeDesc::Bool => {
                b[0] = match ty {
                    LUA_TNIL | LUA_TNONE => 0,
                    LUA_TNUMBER => (lua_tonumberx(L, -1, std::ptr::null_mut()) != 0.0) as u8,
                    _ => (lua_toboolean(L, -1) != 0) as u8,
                };
                Ok(())
            }
            TypeDesc::Str { cap } => {
                let pushed = match ty {
                    LUA_TNIL | LUA_TNONE => return Ok(()),
                    LUA_TSTRING => false,
                    LUA_TNUMBER => {
                        // tostring(v): convert a copy (lua_tolstring converts in place).
                        lua_pushvalue(L, -1);
                        true
                    }
                    _ => return Err(MErr::Field(field)),
                };
                let mut len = 0usize;
                let p = lua_tolstring(L, -1, &mut len);
                let s: &[u8] = if p.is_null() { &[] } else { std::slice::from_raw_parts(p as *const u8, len) };
                let s = trunc_utf8(s, *cap);
                let ok = !s.contains(&0) && std::str::from_utf8(s).is_ok();
                if ok {
                    b[..s.len()].copy_from_slice(s);
                }
                if pushed {
                    pop(L, 1);
                }
                if ok {
                    Ok(())
                } else {
                    Err(MErr::Field(field))
                }
            }
            TypeDesc::Array { elem, len } => {
                match ty {
                    LUA_TNIL | LUA_TNONE => return Ok(()),
                    LUA_TTABLE => {}
                    _ => return Err(MErr::Field(field)),
                }
                let t = lua_absindex(L, -1);
                let es = elem.size();
                for i in 0..*len {
                    lua_rawgeti(L, t, i as i64 + 1);
                    write_val(L, elem, &mut b[i * es..(i + 1) * es], field)?;
                    pop(L, 1);
                }
                Ok(())
            }
            TypeDesc::Struct { fields, .. } => {
                match ty {
                    LUA_TNIL | LUA_TNONE => return Ok(()),
                    LUA_TTABLE => {}
                    _ => return Err(MErr::Field(field)),
                }
                let t = lua_absindex(L, -1);
                for f in *fields {
                    if f.name.as_bytes().first() == Some(&b'_') {
                        continue;
                    }
                    rawget_str(L, t, f.name);
                    let sz = f.ty.size();
                    write_val(L, f.ty, &mut b[f.offset..f.offset + sz], f.name)?;
                    pop(L, 1);
                }
                Ok(())
            }
        }
    }
}

/// Marshal the table at `idx` as record `r` into `out` (head + used rows; cleared first).
/// Generic checks only; [`validate`] runs the record's own check.
///
/// # Safety
/// `L` a valid Lua state on the calling thread.
pub unsafe fn table_to_record(L: *mut lua_State, idx: c_int, r: &RecordInfo, out: &mut Vec<u8>) -> Result<(), MErr> {
    unsafe {
        if lua_type(L, idx) != LUA_TTABLE {
            return Err(MErr::Bad);
        }
        let idx = lua_absindex(L, idx);
        let top = lua_gettop(L);
        if lua_checkstack(L, 16) == 0 {
            return Err(MErr::Bad);
        }
        let res = (|| {
            let hs = r.head.size();
            out.clear();
            out.resize(hs, 0);
            lua_pushvalue(L, idx);
            write_val(L, r.head, &mut out[..hs], r.layout)?;
            pop(L, 1);
            let Some(row) = r.row else { return Ok(()) };
            let n = match rawget_str(L, idx, "rows") {
                LUA_TNIL => 0,
                LUA_TBOOLEAN if lua_toboolean(L, -1) == 0 => 0,
                LUA_TTABLE => lua_rawlen(L, -1) as usize,
                _ => return Err(MErr::Field("rows")),
            };
            if n > r.max_rows {
                return Err(MErr::TooBig);
            }
            let rows = lua_absindex(L, -1);
            let rs = row.size();
            out.resize(hs + n * rs, 0);
            let rname = row.name();
            for i in 0..n {
                lua_rawgeti(L, rows, i as i64 + 1);
                // A row is a table (or a scalar for a primitive row type); nil = zeroed.
                write_val(L, row, &mut out[hs + i * rs..hs + (i + 1) * rs], rname)?;
                pop(L, 1);
            }
            if let Some(cf) = r.count_field.and_then(|c| r.head.field(c)) {
                if let TypeDesc::Prim { name, .. } = cf.ty {
                    put_int(&mut out[cf.offset..], prim(name), n as i64);
                }
            }
            Ok(())
        })();
        lua_settop(L, top);
        res
    }
}

/// The record's full validation (`schema::check_payload` semantics for this record).
pub fn validate(r: &RecordInfo, payload: &[u8]) -> Result<(), MErr> {
    (r.check)(payload).map_err(MErr::Check)
}

// ---- reading ---------------------------------------------------------------------------------

#[inline]
fn get_int(b: &[u8], p: P) -> i64 {
    match p {
        P::U8 => b[0] as i64,
        P::I8 => b[0] as i8 as i64,
        P::U16 => u16::from_le_bytes([b[0], b[1]]) as i64,
        P::I16 => i16::from_le_bytes([b[0], b[1]]) as i64,
        P::U32 => u32::from_le_bytes(b[..4].try_into().unwrap_or([0; 4])) as i64,
        P::I32 => i32::from_le_bytes(b[..4].try_into().unwrap_or([0; 4])) as i64,
        _ => i64::from_le_bytes(b[..8].try_into().unwrap_or([0; 8])),
    }
}

/// Push a scalar (prim / Bool / Str) value of `d` from `b`. False for composites.
#[inline]
unsafe fn push_scalar(L: *mut lua_State, d: &TypeDesc, b: &[u8]) -> bool {
    unsafe {
        match d {
            TypeDesc::Prim { name, .. } => {
                match prim(name) {
                    P::F32 => lua_pushnumber(L, f32::from_le_bytes(b[..4].try_into().unwrap_or([0; 4])) as f64),
                    P::F64 => lua_pushnumber(L, f64::from_le_bytes(b[..8].try_into().unwrap_or([0; 8]))),
                    p => lua_pushinteger(L, get_int(b, p)),
                }
                true
            }
            TypeDesc::Bool => {
                lua_pushboolean(L, (b[0] != 0) as c_int);
                true
            }
            TypeDesc::Str { cap } => {
                let s = &b[..*cap];
                let n = s.iter().position(|&x| x == 0).unwrap_or(*cap);
                push_bytes(L, &s[..n]);
                true
            }
            _ => false,
        }
    }
}

/// Fill the table at absolute index `t` (a struct or array of `d`) from `b`, reusing nested
/// tables that are already there.
unsafe fn fill_composite(L: *mut lua_State, d: &'static TypeDesc, b: &[u8], t: c_int) {
    unsafe {
        match d {
            TypeDesc::Struct { fields, .. } => {
                for f in *fields {
                    if f.name.as_bytes().first() == Some(&b'_') {
                        continue;
                    }
                    let fb = &b[f.offset..f.offset + f.ty.size()];
                    push_str(L, f.name);
                    if push_scalar(L, f.ty, fb) {
                        lua_rawset(L, t);
                    } else {
                        // Reuse t[name] if it is a table.
                        lua_pushvalue(L, -1);
                        if lua_rawget(L, t) != LUA_TTABLE {
                            pop(L, 1);
                            let (na, nr) = shape(f.ty);
                            lua_createtable(L, na, nr);
                            lua_pushvalue(L, -2);
                            lua_pushvalue(L, -2);
                            lua_rawset(L, t);
                        }
                        let sub = lua_gettop(L);
                        fill_composite(L, f.ty, fb, sub);
                        pop(L, 2);
                    }
                }
            }
            TypeDesc::Array { elem, len } => {
                let es = elem.size();
                for i in 0..*len {
                    let eb = &b[i * es..(i + 1) * es];
                    if !push_scalar(L, elem, eb) {
                        if lua_rawgeti(L, t, i as i64 + 1) != LUA_TTABLE {
                            pop(L, 1);
                            let (na, nr) = shape(elem);
                            lua_createtable(L, na, nr);
                            lua_pushvalue(L, -1);
                            lua_rawseti(L, t, i as i64 + 1);
                        }
                        let sub = lua_gettop(L);
                        fill_composite(L, elem, eb, sub);
                        pop(L, 1);
                        continue;
                    }
                    lua_rawseti(L, t, i as i64 + 1);
                }
                trim_array(L, t, *len as i64);
            }
            _ => {}
        }
    }
}

/// Table preallocation (array part, hash part) for a composite.
fn shape(d: &TypeDesc) -> (c_int, c_int) {
    match d {
        TypeDesc::Array { len, .. } => (*len as c_int, 0),
        TypeDesc::Struct { fields, .. } => (0, fields.len() as c_int),
        _ => (0, 0),
    }
}

/// Is `key` (raw bytes) a Lua-visible field of record `r` (or `rows` of a variable record)?
fn is_record_key(r: &RecordInfo, key: &[u8]) -> bool {
    if r.row.is_some() && key == b"rows" {
        return true;
    }
    r.head.fields().iter().any(|f| f.name.as_bytes() == key && key.first() != Some(&b'_'))
}

/// Remove every key of the table at `t` that is not a field of `r`.
unsafe fn clear_foreign(L: *mut lua_State, r: &RecordInfo, t: c_int) {
    unsafe {
        lua_pushnil(L);
        while lua_next(L, t) != 0 {
            pop(L, 1);
            let keep = lua_type(L, -1) == LUA_TSTRING && arg_bytes(L, -1).is_some_and(|k| is_record_key(r, k));
            if !keep {
                lua_pushvalue(L, -1);
                lua_pushnil(L);
                lua_rawset(L, t);
            }
        }
    }
}

/// Push record `r`'s table for `payload` (already validated). With `out` (an absolute index
/// of a table) that table is filled in place and pushed; nested tables in it are reused,
/// trailing array / row entries are set to nil and keys that are not fields are removed.
///
/// # Safety
/// `L` a valid Lua state on the calling thread; `payload` at least the head's size and
/// `head + count * row` bytes (as validation guarantees).
pub unsafe fn record_to_table(L: *mut lua_State, r: &RecordInfo, payload: &[u8], out: Option<c_int>) -> bool {
    unsafe {
        let hs = r.head.size();
        if payload.len() < hs || lua_checkstack(L, 16) == 0 {
            return false;
        }
        let t = match out {
            Some(o) => {
                lua_pushvalue(L, o);
                let t = lua_gettop(L);
                clear_foreign(L, r, t);
                t
            }
            None => {
                lua_createtable(L, 0, r.head.fields().len() as c_int + r.row.is_some() as c_int);
                lua_gettop(L)
            }
        };
        fill_composite(L, r.head, &payload[..hs], t);
        if let Some(row) = r.row {
            let rs = row.size().max(1);
            let n = ((payload.len() - hs) / rs).min(r.max_rows);
            subtable(L, t, "rows", n as c_int, 0);
            let rt = lua_gettop(L);
            for i in 0..n {
                let rb = &payload[hs + i * rs..hs + (i + 1) * rs];
                if push_scalar(L, row, rb) {
                    lua_rawseti(L, rt, i as i64 + 1);
                } else {
                    if lua_rawgeti(L, rt, i as i64 + 1) != LUA_TTABLE {
                        pop(L, 1);
                        let (na, nr) = shape(row);
                        lua_createtable(L, na, nr);
                        lua_pushvalue(L, -1);
                        lua_rawseti(L, rt, i as i64 + 1);
                    }
                    let sub = lua_gettop(L);
                    fill_composite(L, row, rb, sub);
                    pop(L, 1);
                }
            }
            trim_array(L, rt, n as i64);
            pop(L, 1);
        }
        true
    }
}
