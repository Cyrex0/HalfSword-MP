//! Dictionary encoding preserves every field/value occurrence; only strings share storage.
use serde_json::{Map, Number, Value};
use std::collections::{BTreeMap, BTreeSet};
pub const VERSION: u8 = 1;
pub const MAX_DEPTH: usize = 24;
// Every value/table occurrence requires at least one encoded token byte.
pub const MAX_NODES: usize = super::MAX_RECIPE_BYTES;
pub const MAX_STRING_BYTES: usize = 512;
pub const MAX_OBJECT_FIELDS: usize = 256;
pub const MAX_KEY_BYTES: usize = 128;
// One value string and one object key per existing node, at most.
const MAX_STRINGS: usize = MAX_NODES * 2;
const MAGIC: &[u8; 4] = b"HSDR";
pub struct Plan<'a> {
    value: &'a Value,
    strings: Vec<&'a str>,
    ids: BTreeMap<&'a str, u64>,
    pub dictionary_bytes: usize,
    pub token_bytes: usize,
    pub nodes: usize,
}
struct Output<'a> {
    bytes: Option<&'a mut Vec<u8>>,
    len: usize,
}
impl Output<'_> {
    fn put(&mut self, b: &[u8]) -> Result<(), &'static str> {
        self.len = self.len.checked_add(b.len()).ok_or("source codec size")?;
        if let Some(out) = &mut self.bytes {
            if self.len > super::MAX_RECIPE_BYTES {
                return Err("source recipe byte bound");
            }
            out.extend_from_slice(b);
        }
        Ok(())
    }
    fn byte(&mut self, b: u8) -> Result<(), &'static str> {
        self.put(&[b])
    }
    fn uint(&mut self, mut n: u64) -> Result<(), &'static str> {
        while n >= 128 {
            self.byte((n as u8 & 127) | 128)?;
            n >>= 7;
        }
        self.byte(n as u8)
    }
}
fn collect<'a>(
    v: &'a Value,
    depth: usize,
    nodes: &mut usize,
    strings: &mut BTreeSet<&'a str>,
) -> Result<(), &'static str> {
    *nodes += 1;
    if depth > MAX_DEPTH || *nodes > MAX_NODES {
        return Err("source recipe table bound");
    }
    match v {
        Value::String(s) => {
            if s.len() > MAX_STRING_BYTES {
                return Err("source recipe string bound");
            }
            strings.insert(s);
        }
        Value::Array(a) => {
            if a.len() > MAX_NODES - *nodes {
                return Err("source array bound");
            }
            for v in a {
                collect(v, depth + 1, nodes, strings)?;
            }
        }
        Value::Object(o) => {
            if o.len() > MAX_OBJECT_FIELDS {
                return Err("source object bound");
            }
            for (k, v) in o {
                if k.len() > MAX_KEY_BYTES {
                    return Err("source object key bound");
                }
                strings.insert(k);
                collect(v, depth + 1, nodes, strings)?;
            }
        }
        _ => {}
    }
    if strings.len() > MAX_STRINGS {
        return Err("source dictionary bound");
    }
    Ok(())
}
fn tokens(v: &Value, ids: &BTreeMap<&str, u64>, out: &mut Output<'_>) -> Result<(), &'static str> {
    match v {
        Value::Null => out.byte(0)?,
        Value::Bool(false) => out.byte(1)?,
        Value::Bool(true) => out.byte(2)?,
        Value::Number(n) => {
            if let Some(n) = n.as_u64() {
                out.byte(4)?;
                out.uint(n)?;
            } else if let Some(n) = n.as_i64() {
                out.byte(3)?;
                out.uint(((n as u64) << 1) ^ ((n >> 63) as u64))?;
            } else {
                out.byte(5)?;
                out.put(
                    &n.as_f64()
                        .filter(|v| v.is_finite())
                        .ok_or("source finite number")?
                        .to_bits()
                        .to_le_bytes(),
                )?;
            }
        }
        Value::String(s) => {
            out.byte(6)?;
            out.uint(ids[s.as_str()])?;
        }
        Value::Array(a) => {
            out.byte(7)?;
            out.uint(a.len() as u64)?;
            for v in a {
                tokens(v, ids, out)?;
            }
        }
        Value::Object(o) => {
            out.byte(8)?;
            out.uint(o.len() as u64)?;
            let sorted: BTreeMap<_, _> = o.iter().collect();
            for (k, v) in sorted {
                out.uint(ids[k.as_str()])?;
                tokens(v, ids, out)?;
            }
        }
    }
    Ok(())
}
impl<'a> Plan<'a> {
    pub fn new(value: &'a Value) -> Result<Self, &'static str> {
        let mut strings = BTreeSet::new();
        let mut nodes = 0;
        collect(value, 0, &mut nodes, &mut strings)?;
        let strings: Vec<_> = strings.into_iter().collect();
        let ids = strings
            .iter()
            .enumerate()
            .map(|(i, s)| (*s, i as u64))
            .collect();
        let mut plan = Self {
            value,
            strings,
            ids,
            dictionary_bytes: 0,
            token_bytes: 0,
            nodes,
        };
        let mut out = Output {
            bytes: None,
            len: 0,
        };
        plan.dictionary(&mut out)?;
        plan.dictionary_bytes = out.len;
        tokens(value, &plan.ids, &mut out)?;
        plan.token_bytes = out.len - plan.dictionary_bytes;
        Ok(plan)
    }
    fn dictionary(&self, out: &mut Output<'_>) -> Result<(), &'static str> {
        out.put(MAGIC)?;
        out.byte(VERSION)?;
        out.uint(self.strings.len() as u64)?;
        for s in &self.strings {
            out.uint(s.len() as u64)?;
            out.put(s.as_bytes())?;
        }
        Ok(())
    }
    pub fn len(&self) -> usize {
        self.dictionary_bytes + self.token_bytes
    }
    pub fn encode(&self) -> Result<Vec<u8>, &'static str> {
        if self.len() > super::MAX_RECIPE_BYTES {
            return Err("source recipe byte bound");
        }
        let mut bytes = Vec::with_capacity(self.len());
        let mut out = Output {
            bytes: Some(&mut bytes),
            len: 0,
        };
        self.dictionary(&mut out)?;
        tokens(self.value, &self.ids, &mut out)?;
        Ok(bytes)
    }
}
struct Input<'a> {
    bytes: &'a [u8],
    pos: usize,
    nodes: usize,
}
impl<'a> Input<'a> {
    fn remaining(&self) -> usize {
        self.bytes.len() - self.pos
    }
    fn take(&mut self, n: usize) -> Result<&'a [u8], &'static str> {
        if n > self.remaining() {
            return Err("short source recipe codec");
        }
        let s = &self.bytes[self.pos..self.pos + n];
        self.pos += n;
        Ok(s)
    }
    fn byte(&mut self) -> Result<u8, &'static str> {
        Ok(self.take(1)?[0])
    }
    fn uint(&mut self) -> Result<u64, &'static str> {
        let mut n = 0;
        for i in 0..10 {
            let b = self.byte()?;
            if i == 9 && b > 1 {
                return Err("source integer overflow");
            }
            n |= u64::from(b & 127) << (i * 7);
            if b & 128 == 0 {
                if i > 0 && b == 0 {
                    return Err("noncanonical source integer");
                }
                return Ok(n);
            }
        }
        Err("source integer overflow")
    }
    fn count(&mut self, max: usize, min_bytes: usize) -> Result<usize, &'static str> {
        let n = usize::try_from(self.uint()?).map_err(|_| "source count")?;
        if n > max || n > self.remaining() / min_bytes {
            return Err("source codec count bound");
        }
        Ok(n)
    }
    fn string<'b>(&mut self, strings: &'b [String]) -> Result<&'b str, &'static str> {
        let id = usize::try_from(self.uint()?).map_err(|_| "source string id")?;
        strings
            .get(id)
            .map(String::as_str)
            .ok_or("source string id")
    }
    fn value(&mut self, strings: &[String], depth: usize) -> Result<Value, &'static str> {
        self.nodes += 1;
        if depth > MAX_DEPTH || self.nodes > MAX_NODES {
            return Err("source recipe table bound");
        }
        Ok(match self.byte()? {
            0 => Value::Null,
            1 => Value::Bool(false),
            2 => Value::Bool(true),
            3 => {
                let n = self.uint()?;
                Value::Number(Number::from(((n >> 1) as i64) ^ -((n & 1) as i64)))
            }
            4 => Value::Number(Number::from(self.uint()?)),
            5 => {
                let bits = u64::from_le_bytes(self.take(8)?.try_into().unwrap());
                Value::Number(Number::from_f64(f64::from_bits(bits)).ok_or("source finite number")?)
            }
            6 => Value::String(self.string(strings)?.to_owned()),
            7 => {
                let n = self.count(MAX_NODES - self.nodes, 1)?;
                if n != 0 && depth == MAX_DEPTH {
                    return Err("source recipe table bound");
                }
                let mut a = Vec::with_capacity(n);
                for _ in 0..n {
                    a.push(self.value(strings, depth + 1)?);
                }
                Value::Array(a)
            }
            8 => {
                let n = self.count(MAX_OBJECT_FIELDS.min(MAX_NODES - self.nodes), 2)?;
                if n != 0 && depth == MAX_DEPTH {
                    return Err("source recipe table bound");
                }
                let mut o = Map::new();
                let mut previous: Option<&str> = None;
                for _ in 0..n {
                    let k = self.string(strings)?;
                    if k.len() > MAX_KEY_BYTES || previous.is_some_and(|p| p >= k) {
                        return Err("noncanonical source object keys");
                    }
                    previous = Some(k);
                    o.insert(k.to_owned(), self.value(strings, depth + 1)?);
                }
                Value::Object(o)
            }
            _ => return Err("source codec tag"),
        })
    }
}
pub fn decode(bytes: &[u8]) -> Result<Value, &'static str> {
    if bytes.is_empty() || bytes.len() > super::MAX_RECIPE_BYTES {
        return Err("source recipe byte bound");
    }
    let mut input = Input {
        bytes,
        pos: 0,
        nodes: 0,
    };
    if input.take(4)? != MAGIC || input.byte()? != VERSION {
        return Err("source recipe codec version");
    }
    // Sorted unique strings need a length byte and at least one content byte,
    // except a single empty string. The mandatory root token covers that byte.
    let n = input.count(MAX_STRINGS, 2)?;
    let mut strings = Vec::<String>::with_capacity(n);
    for _ in 0..n {
        let len = usize::try_from(input.uint()?).map_err(|_| "source string length")?;
        if len > MAX_STRING_BYTES {
            return Err("source recipe string bound");
        }
        let s = std::str::from_utf8(input.take(len)?).map_err(|_| "source utf8")?;
        if strings.last().is_some_and(|p| p.as_str() >= s) {
            return Err("noncanonical source dictionary");
        }
        strings.push(s.to_owned());
    }
    let value = input.value(&strings, 0)?;
    if input.remaining() != 0 {
        return Err("source codec trailing bytes");
    }
    if Plan::new(&value)?.encode()? != bytes {
        return Err("noncanonical source recipe codec");
    }
    Ok(value)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn codec_scalar_bits_unicode_and_explicit_absence_roundtrip() {
        let value = serde_json::json!({"empty":"", "false":false,"null":null,"array":[],
            "integer_min":i64::MIN,"integer_max":u64::MAX,"unicode":"Ж 雨","duplicate":"Ж 雨"});
        let mut value = value;
        for n in [-0.0, f64::from_bits(1), f64::MAX, 1.0000000000000002] {
            value["float"] = Value::Number(Number::from_f64(n).unwrap());
            let plan = Plan::new(&value).unwrap();
            let bytes = plan.encode().unwrap();
            let decoded = decode(&bytes).unwrap();
            assert_eq!(decoded["float"].as_f64().unwrap().to_bits(), n.to_bits());
            assert_eq!(decoded["integer_min"].as_i64(), Some(i64::MIN));
            assert_eq!(decoded["integer_max"].as_u64(), Some(u64::MAX));
            assert_eq!(decoded, value);
            assert_eq!(Plan::new(&decoded).unwrap().encode().unwrap(), bytes);
        }
    }
    #[test]
    fn codec_rejects_noncanonical_unknown_truncated_and_oversized_input() {
        for suffix in [
            vec![0, 255],
            vec![128, 0, 0],
            vec![2, 1, b'a', 1, b'a', 0],
            vec![2, 1, b'b', 1, b'a', 0],
            vec![1, 1, 255, 0],
            vec![1, 1, b'a', 0],
            vec![1, 1, b'a', 8, 2, 0, 0, 0, 1],
            vec![0, 7, 127, 0],
            vec![0, 6, 0],
            vec![0, 0, 0],
            vec![0, 4, 255, 255, 255, 255, 255, 255, 255, 255, 255, 2],
        ] {
            let mut bytes = b"HSDR\x01".to_vec();
            bytes.extend(suffix);
            assert!(decode(&bytes).is_err(), "invalid bytes {bytes:?}");
        }
        let bytes = Plan::new(&Value::Null).unwrap().encode().unwrap();
        for n in 0..bytes.len() {
            assert!(decode(&bytes[..n]).is_err());
        }
        let mut version = bytes.clone();
        version[4] = 2;
        assert!(decode(&version).is_err());
        assert!(decode(&vec![0; super::super::MAX_RECIPE_BYTES + 1]).is_err());
        let mut nonfinite = b"HSDR\x01\x00\x05".to_vec();
        nonfinite.extend(f64::INFINITY.to_bits().to_le_bytes());
        assert!(decode(&nonfinite).is_err());
    }
    #[test]
    fn codec_bounds_nodes_depth_strings_and_encoded_bytes_before_copy() {
        let array = Value::Array(vec![Value::Null; MAX_NODES - 1]);
        assert_eq!(Plan::new(&array).unwrap().nodes, MAX_NODES);
        assert!(Plan::new(&Value::Array(vec![Value::Null; MAX_NODES])).is_err());
        assert!(Plan::new(&Value::String("x".repeat(MAX_STRING_BYTES + 1))).is_err());
        let mut nested = Value::Null;
        for _ in 0..MAX_DEPTH {
            nested = Value::Array(vec![nested]);
        }
        Plan::new(&nested).unwrap();
        nested = Value::Array(vec![nested]);
        assert!(Plan::new(&nested).is_err());
        let values = Value::Array(
            (0..1100)
                .map(|i| Value::String(format!("{i:04}{}", "x".repeat(508))))
                .collect(),
        );
        let plan = Plan::new(&values).unwrap();
        assert!(plan.len() > super::super::MAX_RECIPE_BYTES);
        assert!(plan.encode().is_err());
    }
}
