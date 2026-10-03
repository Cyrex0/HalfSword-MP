//! Records as JSON for humans and test harnesses only (feature `json`): `ipc-dump`, the
//! sidecar's `--ipc-tap` log, the fake game's `--view` log and the JSON inputs of e2e /
//! `ipc-put`. Never on a data path: the game, the sidecar and the server move records as
//! bytes ([`crate::record`], [`crate::wire`]).
//!
//! Shape (the same as the native module's Lua tables): one JSON object per struct with the
//! Rust field names (fields named `_...` are padding and omitted), arrays as JSON arrays,
//! [`Str`](crate::layout::Str) as strings, [`Bool`](crate::layout::Bool) as booleans, and a
//! variable record's rows as `"rows": [...]` (its count field is derived from them).

use serde_json::{Map, Value as J};

use crate::layout::TypeDesc;
use crate::record::Invalid;
use crate::schema::{record_by_name, record_info, RecordInfo};

fn prim_to_json(name: &str, b: &[u8]) -> J {
    macro_rules! le {
        ($t:ty) => {
            <$t>::from_le_bytes(b[..core::mem::size_of::<$t>()].try_into().unwrap_or_default())
        };
    }
    match name {
        "u8" => J::from(b[0]),
        "i8" => J::from(b[0] as i8),
        "u16" => J::from(le!(u16)),
        "i16" => J::from(le!(i16)),
        "u32" => J::from(le!(u32)),
        "i32" => J::from(le!(i32)),
        "u64" => J::from(le!(u64)),
        "i64" => J::from(le!(i64)),
        "f32" => {
            let v = le!(f32);
            serde_json::Number::from_f64(v as f64).map_or(J::Null, J::Number)
        }
        "f64" => {
            let v = le!(f64);
            serde_json::Number::from_f64(v).map_or(J::Null, J::Number)
        }
        _ => J::Null,
    }
}

/// Bytes laid out as `d` -> JSON.
pub fn to_json(b: &[u8], d: &TypeDesc) -> J {
    match d {
        TypeDesc::Prim { name, .. } => prim_to_json(name, b),
        TypeDesc::Array { elem, len } => {
            let s = elem.size();
            J::Array((0..*len).map(|i| to_json(&b[i * s..(i + 1) * s], elem)).collect())
        }
        TypeDesc::Struct { fields, .. } => {
            let mut m = Map::new();
            for f in *fields {
                if f.name.starts_with('_') {
                    continue;
                }
                m.insert(f.name.to_string(), to_json(&b[f.offset..f.offset + f.ty.size()], f.ty));
            }
            J::Object(m)
        }
        TypeDesc::Str { cap } => {
            let s = &b[..*cap];
            let n = s.iter().position(|&x| x == 0).unwrap_or(*cap);
            J::String(String::from_utf8_lossy(&s[..n]).into_owned())
        }
        TypeDesc::Bool => J::Bool(b[0] != 0),
    }
}

fn put_prim(name: &str, v: &J, out: &mut [u8]) {
    let f = v.as_f64().or_else(|| v.as_bool().map(|b| b as u8 as f64)).unwrap_or(0.0);
    let i = v.as_i64().or_else(|| v.as_u64().map(|u| u as i64)).unwrap_or(f as i64);
    match name {
        "u8" | "i8" => out[0] = i as u8,
        "u16" | "i16" => out[..2].copy_from_slice(&(i as u16).to_le_bytes()),
        "u32" | "i32" => out[..4].copy_from_slice(&(i as u32).to_le_bytes()),
        "u64" => out[..8].copy_from_slice(&v.as_u64().unwrap_or(i as u64).to_le_bytes()),
        "i64" => out[..8].copy_from_slice(&i.to_le_bytes()),
        "f32" => out[..4].copy_from_slice(&(f as f32).to_le_bytes()),
        "f64" => out[..8].copy_from_slice(&f.to_le_bytes()),
        _ => {}
    }
}

/// JSON -> bytes laid out as `d` (missing fields stay zero; strings are truncated on a char
/// boundary). `out` must be `d.size()` bytes, zeroed.
pub fn from_json(v: &J, d: &TypeDesc, out: &mut [u8]) {
    match d {
        TypeDesc::Prim { name, .. } => put_prim(name, v, out),
        TypeDesc::Array { elem, len } => {
            let s = elem.size();
            for (i, x) in v.as_array().into_iter().flatten().take(*len).enumerate() {
                from_json(x, elem, &mut out[i * s..(i + 1) * s]);
            }
        }
        TypeDesc::Struct { fields, .. } => {
            for f in *fields {
                if let Some(x) = v.get(f.name) {
                    from_json(x, f.ty, &mut out[f.offset..f.offset + f.ty.size()]);
                }
            }
        }
        TypeDesc::Str { cap } => {
            let s = v.as_str().unwrap_or("");
            let mut n = s.len().min(*cap);
            while n > 0 && !s.is_char_boundary(n) {
                n -= 1;
            }
            out[..n].copy_from_slice(&s.as_bytes()[..n]);
        }
        TypeDesc::Bool => out[0] = v.as_bool().unwrap_or_else(|| v.as_f64().unwrap_or(0.0) != 0.0) as u8,
    }
}

fn count_offset(r: &RecordInfo) -> Option<(usize, usize)> {
    let f = r.head.field(r.count_field?)?;
    Some((f.offset, f.ty.size()))
}

/// A record payload of `kind` as JSON (`{"rows": [...]}` for a variable record). Invalid
/// payloads are shown as far as they parse, with `"_invalid": "<reason>"`.
pub fn record_to_json(kind: u16, payload: &[u8]) -> J {
    let Some(r) = record_info(kind) else {
        return serde_json::json!({ "_kind": kind, "_len": payload.len() });
    };
    let hs = r.head.size();
    if payload.len() < hs {
        return serde_json::json!({ "_record": r.name, "_invalid": "short", "_len": payload.len() });
    }
    let mut v = to_json(&payload[..hs], r.head);
    if let (Some(row), J::Object(m)) = (r.row, &mut v) {
        let rs = row.size();
        let rows: Vec<J> = payload[hs..].chunks_exact(rs).map(|c| to_json(c, row)).collect();
        m.insert("rows".into(), J::Array(rows));
    }
    if let (Err(e), J::Object(m)) = ((r.check)(payload), &mut v) {
        m.insert("_invalid".into(), J::String(e.to_string()));
    }
    v
}

/// JSON -> a record payload of the record named `name` (rows from `"rows"`; the count field
/// is set from them). The result is validated.
pub fn json_to_record(name: &str, v: &J) -> Result<(u16, Vec<u8>), String> {
    let r = record_by_name(name).ok_or_else(|| format!("unknown record {name:?}"))?;
    let hs = r.head.size();
    let mut out = vec![0u8; hs];
    from_json(v, r.head, &mut out);
    if let Some(row) = r.row {
        let rows = v.get("rows").and_then(|x| x.as_array()).cloned().unwrap_or_default();
        let n = rows.len().min(r.max_rows);
        let rs = row.size();
        let base = out.len();
        out.resize(base + n * rs, 0);
        for (i, x) in rows.iter().take(n).enumerate() {
            from_json(x, row, &mut out[base + i * rs..base + (i + 1) * rs]);
        }
        if let Some((off, size)) = count_offset(r) {
            let b = (n as u64).to_le_bytes();
            out[off..off + size].copy_from_slice(&b[..size]);
        }
    }
    (r.check)(&out).map_err(|e: Invalid| format!("{name}: {e}"))?;
    Ok((r.kind, out))
}
