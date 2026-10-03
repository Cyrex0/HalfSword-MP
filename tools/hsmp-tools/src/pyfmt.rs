//! Python-compatible number formatting and `json.dumps(..., indent=N)` emission,
//! so generators ported from Python stay byte-identical with committed output.

/// Exact decimal expansion of |x| (finite), as (integer digits, fraction digits).
fn exact_decimal(x: f64) -> (String, String) {
    // Every finite f64 has a terminating decimal expansion of <= 1074 fraction digits;
    // Rust's fixed-precision formatting is exact at that precision.
    let s = format!("{:.1074}", x.abs());
    let (i, f) = s.split_once('.').unwrap_or((&s, ""));
    (i.to_string(), f.trim_end_matches('0').to_string())
}

/// Python `round(x, nd)` for floats (correctly rounded, ties to even on the exact value).
pub fn py_round(x: f64, nd: usize) -> f64 {
    if !x.is_finite() || x == 0.0 {
        return x;
    }
    let (ip, fp) = exact_decimal(x);
    if fp.len() <= nd {
        return x;
    }
    let mut digits: Vec<u8> = ip.bytes().chain(fp.bytes().take(nd)).map(|b| b - b'0').collect();
    let rest = &fp.as_bytes()[nd..];
    let first = rest[0] - b'0';
    let tail_nonzero = rest[1..].iter().any(|&b| b != b'0');
    let last_odd = digits.last().map(|d| d % 2 == 1).unwrap_or(false);
    let up = first > 5 || (first == 5 && (tail_nonzero || last_odd));
    if up {
        let mut k = digits.len();
        loop {
            if k == 0 {
                digits.insert(0, 1);
                break;
            }
            k -= 1;
            if digits[k] == 9 {
                digits[k] = 0;
            } else {
                digits[k] += 1;
                break;
            }
        }
    }
    let ilen = digits.len() - nd;
    let mut s: String = digits[..ilen].iter().map(|d| (b'0' + d) as char).collect();
    if nd > 0 {
        s.push('.');
        s.extend(digits[ilen..].iter().map(|d| (b'0' + d) as char));
    }
    let v: f64 = s.parse().unwrap();
    if x < 0.0 {
        -v
    } else {
        v
    }
}

/// Python `repr(float)` (shortest round-trip, `.0` suffix, exponent outside 1e-4..1e16).
pub fn py_repr(x: f64) -> String {
    if x.is_nan() {
        return "nan".into();
    }
    if x.is_infinite() {
        return if x > 0.0 { "inf".into() } else { "-inf".into() };
    }
    if x == 0.0 {
        return if x.is_sign_negative() { "-0.0".into() } else { "0.0".into() };
    }
    // Shortest round-trip digits from Rust's `{:e}`: "-1.2345e3".
    let e = format!("{:e}", x);
    let (mant, exp) = e.split_once('e').unwrap();
    let exp: i32 = exp.parse().unwrap();
    let neg = mant.starts_with('-');
    let digits: String = mant.chars().filter(|c| c.is_ascii_digit()).collect();
    let decpt = exp + 1;
    let mut out = String::new();
    if neg {
        out.push('-');
    }
    if decpt > -4 && decpt <= 16 {
        if decpt <= 0 {
            out.push_str("0.");
            out.extend(std::iter::repeat('0').take((-decpt) as usize));
            out.push_str(&digits);
        } else if (decpt as usize) >= digits.len() {
            out.push_str(&digits);
            out.extend(std::iter::repeat('0').take(decpt as usize - digits.len()));
            out.push_str(".0");
        } else {
            out.push_str(&digits[..decpt as usize]);
            out.push('.');
            out.push_str(&digits[decpt as usize..]);
        }
    } else {
        out.push_str(&digits[..1]);
        if digits.len() > 1 {
            out.push('.');
            out.push_str(&digits[1..]);
        }
        let e = decpt - 1;
        out.push_str(&format!("e{}{:02}", if e < 0 { '-' } else { '+' }, e.abs()));
    }
    out
}

/// A Python-shaped JSON value with insertion-ordered dicts.
#[derive(Debug, Clone, PartialEq)]
pub enum PyVal {
    Null,
    Bool(bool),
    Int(i64),
    Float(f64),
    Str(String),
    List(Vec<PyVal>),
    Dict(Vec<(String, PyVal)>),
}

impl From<f64> for PyVal {
    fn from(v: f64) -> Self {
        PyVal::Float(v)
    }
}
impl From<i64> for PyVal {
    fn from(v: i64) -> Self {
        PyVal::Int(v)
    }
}
impl From<bool> for PyVal {
    fn from(v: bool) -> Self {
        PyVal::Bool(v)
    }
}
impl From<&str> for PyVal {
    fn from(v: &str) -> Self {
        PyVal::Str(v.to_string())
    }
}
impl From<String> for PyVal {
    fn from(v: String) -> Self {
        PyVal::Str(v)
    }
}

/// Python `json.dumps(s)` string literal (ensure_ascii=True).
pub fn json_str(s: &str) -> String {
    let mut o = String::with_capacity(s.len() + 2);
    o.push('"');
    for ch in s.chars() {
        match ch {
            '"' => o.push_str("\\\""),
            '\\' => o.push_str("\\\\"),
            '\n' => o.push_str("\\n"),
            '\r' => o.push_str("\\r"),
            '\t' => o.push_str("\\t"),
            '\u{8}' => o.push_str("\\b"),
            '\u{c}' => o.push_str("\\f"),
            c if (c as u32) < 0x20 || (c as u32) > 0x7e => {
                let mut buf = [0u16; 2];
                for u in c.encode_utf16(&mut buf) {
                    o.push_str(&format!("\\u{:04x}", u));
                }
            }
            c => o.push(c),
        }
    }
    o.push('"');
    o
}

fn scalar(v: &PyVal) -> Option<String> {
    Some(match v {
        PyVal::Null => "null".into(),
        PyVal::Bool(b) => if *b { "true" } else { "false" }.into(),
        PyVal::Int(i) => i.to_string(),
        PyVal::Float(f) => {
            if f.is_nan() {
                "NaN".into()
            } else if f.is_infinite() {
                if *f > 0.0 { "Infinity" } else { "-Infinity" }.into()
            } else {
                py_repr(*f)
            }
        }
        PyVal::Str(s) => json_str(s),
        _ => return None,
    })
}

/// Python `json.dumps(v, indent=indent)` (default separators `,` / `: ` with indent).
pub fn dumps_indent(v: &PyVal, indent: usize) -> String {
    let mut o = String::new();
    emit(v, indent, 0, &mut o);
    o
}

fn emit(v: &PyVal, ind: usize, level: usize, o: &mut String) {
    if let Some(s) = scalar(v) {
        o.push_str(&s);
        return;
    }
    let pad = |n: usize, o: &mut String| {
        o.push('\n');
        o.extend(std::iter::repeat(' ').take(n * ind));
    };
    match v {
        PyVal::List(items) => {
            if items.is_empty() {
                o.push_str("[]");
                return;
            }
            o.push('[');
            for (i, x) in items.iter().enumerate() {
                if i > 0 {
                    o.push(',');
                }
                pad(level + 1, o);
                emit(x, ind, level + 1, o);
            }
            pad(level, o);
            o.push(']');
        }
        PyVal::Dict(items) => {
            if items.is_empty() {
                o.push_str("{}");
                return;
            }
            o.push('{');
            for (i, (k, x)) in items.iter().enumerate() {
                if i > 0 {
                    o.push(',');
                }
                pad(level + 1, o);
                o.push_str(&json_str(k));
                o.push_str(": ");
                emit(x, ind, level + 1, o);
            }
            pad(level, o);
            o.push('}');
        }
        _ => unreachable!(),
    }
}

/// Python `f"{x:.{n}f}"` (exact value, ties to even; same as Rust `{:.n}` except
/// Rust may differ on exact ties, so go through [`py_round`]).
pub fn fixed(x: f64, n: usize) -> String {
    format!("{:.*}", n, py_round(x, n))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn repr() {
        assert_eq!(py_repr(1.0), "1.0");
        assert_eq!(py_repr(-50.5), "-50.5");
        assert_eq!(py_repr(0.1), "0.1");
        assert_eq!(py_repr(1e16), "1e+16");
        assert_eq!(py_repr(1234567890123456.0), "1234567890123456.0");
        assert_eq!(py_repr(0.0001), "0.0001");
        assert_eq!(py_repr(0.00001), "1e-05");
        assert_eq!(py_repr(1.5e-7), "1.5e-07");
        assert_eq!(py_repr(-0.0), "-0.0");
        assert_eq!(py_repr(123.456), "123.456");
    }
    #[test]
    fn round() {
        assert_eq!(py_round(0.25, 1), 0.2);
        assert_eq!(py_round(0.75, 1), 0.8);
        assert_eq!(py_round(1.25, 1), 1.2);
        assert_eq!(py_round(2.675, 2), 2.67); // binary value is below the tie
        assert_eq!(py_round(-1.05, 1), -1.1); // -1.05000000000000004...
        assert_eq!(py_round(9.96, 1), 10.0);
        assert_eq!(py_round(-0.04, 1), -0.0);
        assert_eq!(fixed(0.25, 1), "0.2");
        assert_eq!(fixed(-0.04, 1), "-0.0");
    }
    #[test]
    fn dumps() {
        let v = PyVal::Dict(vec![
            ("a".into(), PyVal::List(vec![1.0.into(), PyVal::Int(2)])),
            ("b".into(), PyVal::List(vec![])),
            ("c".into(), "Lord's \u{e9}".into()),
        ]);
        assert_eq!(dumps_indent(&v, 1), "{\n \"a\": [\n  1.0,\n  2\n ],\n \"b\": [],\n \"c\": \"Lord's \\u00e9\"\n}");
    }
}
