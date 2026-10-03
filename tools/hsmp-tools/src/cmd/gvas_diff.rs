//! `hsmp-tools gvas-diff A.sav B.sav [--grep NAME]`: heuristic diff of scalar
//! properties between two Unreal GVAS save files (port of scripts/gvas_diff.py,
//! same output).
//!
//! Scans for serialized property headers (name, type, size) and decodes the
//! simple scalar types, then prints every property path whose value differs or
//! exists in only one file. Good enough to spot a bad value written into a save;
//! not a full GVAS parser.

use anyhow::{Context, Result};
use hsmp_tools::pyfmt::{py_repr, py_round};
use std::collections::BTreeMap;
use std::path::PathBuf;

#[derive(clap::Args)]
pub struct Args {
    a: PathBuf,
    b: PathBuf,
    /// Only names containing this (case-insensitive)
    #[arg(long)]
    grep: Option<String>,
}

const SCALARS: [&str; 10] = [
    "IntProperty",
    "Int64Property",
    "UInt32Property",
    "FloatProperty",
    "DoubleProperty",
    "BoolProperty",
    "ByteProperty",
    "EnumProperty",
    "NameProperty",
    "StrProperty",
];

/// A decoded scalar, with Python value semantics for equality / repr.
#[derive(Debug, Clone)]
enum PV {
    None,
    Bool(bool),
    Int(i64),
    Float(f64),
    Str(String),
}

impl PV {
    fn num(&self) -> Option<f64> {
        match self {
            PV::Bool(b) => Some(if *b { 1.0 } else { 0.0 }),
            PV::Int(i) => Some(*i as f64),
            PV::Float(f) => Some(*f),
            _ => None,
        }
    }
    fn eq(&self, o: &PV) -> bool {
        match (self, o) {
            (PV::None, PV::None) => true,
            (PV::Str(a), PV::Str(b)) => a == b,
            (PV::Int(a), PV::Int(b)) => a == b,
            _ => match (self.num(), o.num()) {
                (Some(a), Some(b)) => a == b,
                _ => false,
            },
        }
    }
    fn repr(&self) -> String {
        match self {
            PV::None => "None".into(),
            PV::Bool(b) => if *b { "True" } else { "False" }.into(),
            PV::Int(i) => i.to_string(),
            PV::Float(f) => py_repr(*f),
            PV::Str(s) => py_str_repr(s),
        }
    }
}

/// Python `repr(str)`.
pub fn py_str_repr(s: &str) -> String {
    let q = if s.contains('\'') && !s.contains('"') { '"' } else { '\'' };
    let mut o = String::new();
    o.push(q);
    for c in s.chars() {
        match c {
            '\\' => o.push_str("\\\\"),
            '\n' => o.push_str("\\n"),
            '\r' => o.push_str("\\r"),
            '\t' => o.push_str("\\t"),
            c if c == q => {
                o.push('\\');
                o.push(c);
            }
            c if (c as u32) < 0x20 || c as u32 == 0x7f => o.push_str(&format!("\\x{:02x}", c as u32)),
            c if (0x80..0xa0).contains(&(c as u32)) => o.push_str(&format!("\\x{:02x}", c as u32)),
            c if c.is_control() => o.push_str(&format!("\\u{:04x}", c as u32)),
            c => o.push(c),
        }
    }
    o.push(q);
    o
}

fn printable(s: &str) -> bool {
    s.chars().all(|c| !c.is_control() && c != '\u{ad}' && !(c as u32 >= 0xd800 && (c as u32) < 0xe000))
}

fn i32_at(b: &[u8], o: usize) -> Option<i32> {
    b.get(o..o + 4).map(|s| i32::from_le_bytes(s.try_into().unwrap()))
}

/// Python read_fstr: (string or None, new offset).
fn read_fstr(b: &[u8], o: usize) -> (Option<String>, usize) {
    if o + 4 > b.len() {
        return (None, o);
    }
    let n = i32_at(b, o).unwrap();
    let o = o + 4;
    if n == 0 {
        return (Some(String::new()), o);
    }
    if 0 < n && n < 512 && o + n as usize <= b.len() {
        let raw = &b[o..o + n as usize - 1];
        if raw.iter().all(|&c| c < 0x80) {
            return (Some(raw.iter().map(|&c| c as char).collect()), o + n as usize);
        }
        return (None, o);
    }
    if -512 < n && n < 0 && o + (-n) as usize * 2 <= b.len() {
        let len = (-n) as usize * 2 - 2;
        let units: Vec<u16> = b[o..o + len].chunks(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect();
        return match String::from_utf16(&units) {
            Ok(s) => (Some(s), o + (-n) as usize * 2),
            Err(_) => (None, o),
        };
    }
    (None, o)
}

type Entry = (usize, &'static str, PV, i64);

fn find_all(h: &[u8], needle: &[u8]) -> Vec<usize> {
    let mut out = vec![];
    if needle.is_empty() || h.len() < needle.len() {
        return out;
    }
    for i in 0..=h.len() - needle.len() {
        if h[i] == needle[0] && &h[i..i + needle.len()] == needle {
            out.push(i);
        }
    }
    out
}

fn decode(b: &[u8], t: &str, o: usize) -> Option<PV> {
    let g = |o: usize, n: usize| b.get(o..o + n);
    Some(match t {
        "BoolProperty" => PV::Bool(*b.get(o)? != 0),
        "EnumProperty" | "ByteProperty" => {
            let (en, o2) = read_fstr(b, o);
            let o2 = o2 + 1; // guid flag
            if t == "EnumProperty" || en.as_deref().map(|e| !e.is_empty() && e != "None").unwrap_or(false) {
                match read_fstr(b, o2).0 {
                    Some(s) => PV::Str(s),
                    None => PV::None,
                }
            } else {
                PV::Int(*b.get(o2)? as i64)
            }
        }
        _ => {
            let o = o + 1; // guid flag
            match t {
                "IntProperty" => PV::Int(i32::from_le_bytes(g(o, 4)?.try_into().unwrap()) as i64),
                "Int64Property" => PV::Int(i64::from_le_bytes(g(o, 8)?.try_into().unwrap())),
                "UInt32Property" => PV::Int(u32::from_le_bytes(g(o, 4)?.try_into().unwrap()) as i64),
                "FloatProperty" => PV::Float(py_round(f32::from_le_bytes(g(o, 4)?.try_into().unwrap()) as f64, 4)),
                "DoubleProperty" => PV::Float(py_round(f64::from_le_bytes(g(o, 8)?.try_into().unwrap()), 4)),
                _ => match read_fstr(b, o).0 {
                    Some(s) => PV::Str(s),
                    None => PV::None,
                },
            }
        }
    })
}

fn scan(path: &PathBuf) -> Result<BTreeMap<String, Vec<Entry>>> {
    let b = std::fs::read(path).with_context(|| format!("read {}", path.display()))?;
    let mut out: BTreeMap<String, Vec<Entry>> = BTreeMap::new();
    for t in SCALARS {
        let mut needle = ((t.len() + 1) as i32).to_le_bytes().to_vec();
        needle.extend_from_slice(t.as_bytes());
        needle.push(0);
        for i in find_all(&b, &needle) {
            // walk back to the property name: [len][name\0] directly before
            let mut name = None;
            for back in 6..200 {
                if back > i {
                    break;
                }
                let j = i - back;
                let (s, e) = read_fstr(&b, j);
                if let Some(s) = s {
                    if !s.is_empty() && e == i && printable(&s) {
                        name = Some(s);
                        break;
                    }
                }
            }
            let Some(name) = name else { continue };
            let o = i + needle.len();
            if o + 8 > b.len() {
                continue;
            }
            let size = i64::from_le_bytes(b[o..o + 8].try_into().unwrap());
            let Some(val) = decode(&b, t, o + 8) else { continue };
            out.entry(name).or_default().push((i, t, val, size));
        }
    }
    for v in out.values_mut() {
        v.sort_by_key(|e| e.0);
    }
    Ok(out)
}

fn list_repr(v: &[PV]) -> String {
    let mut parts: Vec<String> = v.iter().take(6).map(|x| x.repr()).collect();
    if v.len() > 6 {
        parts.push(py_str_repr(&format!("... ({} total)", v.len())));
    }
    format!("[{}]", parts.join(", "))
}

pub fn run(a: Args) -> Result<i32> {
    let grep = a.grep.map(|g| g.to_lowercase());
    let (ma, mb) = (scan(&a.a)?, scan(&a.b)?);
    let mut names: Vec<&String> = ma.keys().chain(mb.keys()).collect();
    names.sort();
    names.dedup();
    println!("{} names in A, {} names in B", ma.len(), mb.len());
    for n in names {
        if let Some(g) = &grep {
            if !n.to_lowercase().contains(g.as_str()) {
                continue;
            }
        }
        let va: Vec<PV> = ma.get(n).map(|v| v.iter().map(|e| e.2.clone()).collect()).unwrap_or_default();
        let vb: Vec<PV> = mb.get(n).map(|v| v.iter().map(|e| e.2.clone()).collect()).unwrap_or_default();
        let same = va.len() == vb.len() && va.iter().zip(&vb).all(|(x, y)| x.eq(y));
        if !same {
            let ta = ma.get(n).filter(|v| !v.is_empty()).or_else(|| mb.get(n)).map(|v| v[0].1).unwrap_or("?");
            println!("{n} [{ta}]\n    A: {}\n    B: {}", list_repr(&va), list_repr(&vb));
        }
    }
    Ok(0)
}
