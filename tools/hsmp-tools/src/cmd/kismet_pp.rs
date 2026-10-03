//! `hsmp-tools kismet-pp dump.json [function-name-regex]`: Kismet bytecode
//! pretty-printer for CUE4Parse JSON dumps (port of tools/mapdump/kismet_pp.py,
//! same output).
//!
//!     dotnet MapDump.dll --probe <package> "*" > dump.json
//!     hsmp-tools kismet-pp dump.json [function-name-regex] > dump.txt
//!
//! Prints every UFunction of the dump as compact pseudo-code (one statement per
//! line, prefixed with its bytecode offset) so Blueprint logic can be read and
//! grepped statically. Python object semantics (truthiness, `str()`, `repr()`,
//! `json.dumps`) and the original's error strings are reproduced so the text
//! output is identical.

use anyhow::{Context, Result};
use hsmp_tools::paths;
use hsmp_tools::pyfmt::{json_str, py_repr};
use regex::Regex;
use serde_json::Value;
use std::io::Write;
use std::path::PathBuf;

#[derive(clap::Args)]
pub struct Args {
    /// CUE4Parse JSON dump (MapDump --probe output)
    dump: PathBuf,
    /// Only functions whose name matches this regex
    filter: Option<String>,
}

static NULL: Value = Value::Null;

type R<T> = std::result::Result<T, String>;

fn get<'a>(e: &'a Value, k: &str) -> &'a Value {
    e.get(k).unwrap_or(&NULL)
}

fn truthy(v: &Value) -> bool {
    match v {
        Value::Null | Value::Bool(false) => false,
        Value::Number(n) => n.as_f64().map(|x| x != 0.0).unwrap_or(true),
        Value::String(s) => !s.is_empty(),
        Value::Array(a) => !a.is_empty(),
        Value::Object(o) => !o.is_empty(),
        _ => true,
    }
}

fn or<'a>(a: &'a Value, b: &'a Value) -> &'a Value {
    if truthy(a) {
        a
    } else {
        b
    }
}

fn type_name(v: &Value) -> &'static str {
    match v {
        Value::Null => "NoneType",
        Value::Bool(_) => "bool",
        Value::Number(n) if n.is_f64() => "float",
        Value::Number(_) => "int",
        Value::String(_) => "str",
        Value::Array(_) => "list",
        Value::Object(_) => "dict",
    }
}

fn num_str(n: &serde_json::Number) -> String {
    if let Some(i) = n.as_i64() {
        i.to_string()
    } else if let Some(u) = n.as_u64() {
        u.to_string()
    } else {
        py_repr(n.as_f64().unwrap_or(f64::NAN))
    }
}

/// Python `repr(str)`.
fn str_repr(s: &str) -> String {
    super::gvas_diff::py_str_repr(s)
}

/// Python `repr()` of a json-loaded object.
fn repr(v: &Value) -> String {
    match v {
        Value::Null => "None".into(),
        Value::Bool(b) => if *b { "True" } else { "False" }.into(),
        Value::Number(n) => num_str(n),
        Value::String(s) => str_repr(s),
        Value::Array(a) => format!("[{}]", a.iter().map(repr).collect::<Vec<_>>().join(", ")),
        Value::Object(o) => {
            format!("{{{}}}", o.iter().map(|(k, v)| format!("{}: {}", str_repr(k), repr(v))).collect::<Vec<_>>().join(", "))
        }
    }
}

/// Python `str()`.
fn pstr(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        v => repr(v),
    }
}

/// Python `json.dumps()` (default separators, ensure_ascii).
fn dumps(v: &Value) -> String {
    match v {
        Value::Null => "null".into(),
        Value::Bool(b) => if *b { "true" } else { "false" }.into(),
        Value::Number(n) => num_str(n),
        Value::String(s) => json_str(s),
        Value::Array(a) => format!("[{}]", a.iter().map(dumps).collect::<Vec<_>>().join(", ")),
        Value::Object(o) => {
            format!("{{{}}}", o.iter().map(|(k, v)| format!("{}: {}", json_str(k), dumps(v))).collect::<Vec<_>>().join(", "))
        }
    }
}

/// Python iteration over a value.
fn iter(v: &Value) -> R<Vec<Value>> {
    match v {
        Value::Array(a) => Ok(a.clone()),
        Value::Object(o) => Ok(o.keys().map(|k| Value::String(k.clone())).collect()),
        Value::String(s) => Ok(s.chars().map(|c| Value::String(c.to_string())).collect()),
        v => Err(format!("'{}' object is not iterable", type_name(v))),
    }
}

/// `x or []` then iterate.
fn iter_or_empty(v: &Value) -> R<Vec<Value>> {
    if truthy(v) {
        iter(v)
    } else {
        Ok(vec![])
    }
}

fn py_get<'a>(v: &'a Value, k: &str) -> R<&'a Value> {
    match v {
        Value::Object(_) => Ok(get(v, k)),
        v => Err(format!("'{}' object has no attribute 'get'", type_name(v))),
    }
}

/// `', '.join(E(p) for p in items)`.
fn join_e(items: &[Value], sep: &str) -> R<String> {
    let mut parts = vec![];
    for (i, p) in items.iter().enumerate() {
        match ex(p)? {
            Some(s) => parts.push(s),
            None => return Err(format!("sequence item {i}: expected str instance, NoneType found")),
        }
    }
    Ok(parts.join(sep))
}

/// `sep.join(strings)` over a Python iterable.
fn join_strs(v: &Value, sep: &str) -> R<String> {
    let mut parts = vec![];
    for (i, p) in iter(v)?.iter().enumerate() {
        match p {
            Value::String(s) => parts.push(s.clone()),
            o => return Err(format!("sequence item {i}: expected str instance, {} found", type_name(o))),
        }
    }
    Ok(parts.join(sep))
}

fn objname(o: &Value) -> String {
    match o {
        Value::Null => "null".into(),
        Value::Object(_) => {
            let n = o.get("ObjectName").map(pstr).unwrap_or_default();
            let re = Regex::new(r"^(\w+)'(.*)'\n?\z").unwrap();
            match re.captures(&n) {
                Some(c) => c[2].to_string(),
                None => n,
            }
        }
        o => pstr(o),
    }
}

fn prop_name(v: &Value) -> R<String> {
    if let Value::Object(m) = v {
        if let Some(p) = m.get("Path") {
            return join_strs(p, ".");
        }
        let p = or(get(v, "Property"), &NULL);
        if let Value::Object(pm) = p {
            if let Some(n) = pm.get("Name") {
                return Ok(pstr(n));
            }
        }
        return Ok(pstr(v));
    }
    Ok(pstr(v))
}

fn fn_name(f: &Value) -> String {
    let s = objname(f);
    if s.contains(':') {
        s.rsplit(':').next().unwrap_or("").to_string()
    } else {
        s
    }
}

/// `f"{E(x)}"` (None prints as "None").
fn es(v: &Value) -> R<String> {
    Ok(ex(v)?.unwrap_or_else(|| "None".into()))
}

fn has_token(v: &Value) -> bool {
    matches!(v, Value::Object(m) if m.contains_key("Token"))
}

fn tail3(t: &str) -> String {
    t.chars().skip(3).collect()
}

/// The pretty-printer `E(e)`; Ok(None) = statement elided; Err = Python exception text.
fn ex(e: &Value) -> R<Option<String>> {
    if e.is_null() {
        return Ok(Some("_".into()));
    }
    if !e.is_object() {
        return Ok(Some(dumps(e)));
    }
    let tv = e.get("Token").cloned().unwrap_or(Value::String("?".into()));
    let t = match &tv {
        Value::String(s) => s.clone(),
        v => return Err(format!("'{}' object is not subscriptable", type_name(v))),
    };
    let g = |k: &str| get(e, k);
    let s = |x: String| Ok(Some(x));
    match t.as_str() {
        "EX_LocalVariable" | "EX_LocalOutVariable" | "EX_DefaultVariable" | "EX_ClassSparseDataVariable" => {
            s(prop_name(g("Variable"))?)
        }
        "EX_InstanceVariable" => s(format!("self.{}", prop_name(g("Variable"))?)),
        "EX_Let" | "EX_LetObj" | "EX_LetBool" | "EX_LetWeakObjPtr" | "EX_LetDelegate" | "EX_LetMulticastDelegate"
        | "EX_LetValueOnPersistentFrame" => {
            let lhs = if has_token(g("Variable")) {
                es(g("Variable"))?
            } else {
                prop_name(or(g("Variable"), g("DestinationProperty")))?
            };
            s(format!("{lhs} = {}", es(or(g("Expression"), g("Assignment")))?))
        }
        "EX_CallMath" | "EX_FinalFunction" | "EX_LocalFinalFunction" | "EX_CallMulticastDelegate" => {
            let f = fn_name(or(g("Function"), g("StackNode")));
            s(format!("{f}({})", join_e(&iter_or_empty(g("Parameters"))?, ", ")?))
        }
        "EX_VirtualFunction" | "EX_LocalVirtualFunction" => {
            let f = pstr(or(g("VirtualFunctionName"), g("Function")));
            s(format!("{f}({})", join_e(&iter_or_empty(g("Parameters"))?, ", ")?))
        }
        "EX_Context" | "EX_Context_FailSilent" | "EX_ClassContext" => {
            let a = es(g("ObjectExpression"))?;
            s(format!("{a}.{}", es(g("ContextExpression"))?))
        }
        "EX_InterfaceContext" => ex(g("InterfaceValue")),
        "EX_StructMemberContext" => {
            let pr = g("Property");
            if let Value::Object(m) = pr {
                if let Some(p) = m.get("Path") {
                    let a = es(g("StructExpression"))?;
                    return s(format!("{a}.{}", join_strs(p, ".")?));
                }
            }
            let a = es(g("StructExpression"))?;
            let arg = match pr {
                Value::Object(m) if m.contains_key("Name") => {
                    let mut o = serde_json::Map::new();
                    o.insert("Property".into(), pr.clone());
                    Value::Object(o)
                }
                _ => pr.clone(),
            };
            s(format!("{a}.{}", prop_name(&arg)?))
        }
        "EX_ObjectConst" | "EX_SoftObjectConst" => {
            let v = g("Value");
            match v {
                Value::Object(m) if m.contains_key("ObjectName") => s(objname(v)),
                _ => ex(v),
            }
        }
        "EX_IntConst" | "EX_Int64Const" | "EX_UInt64Const" | "EX_FloatConst" | "EX_DoubleConst" | "EX_ByteConst"
        | "EX_IntConstByte" | "EX_NameConst" | "EX_StringConst" | "EX_UnicodeStringConst" => s(dumps(g("Value"))),
        "EX_TextConst" => {
            let empty = Value::Object(Default::default());
            let v = or(g("Value"), &empty);
            let a = py_get(v, "SourceString")?;
            let b = py_get(v, "Text")?;
            let x = if truthy(a) {
                a.clone()
            } else if truthy(b) {
                b.clone()
            } else {
                Value::String(pstr(v).chars().take(60).collect())
            };
            s(dumps(&x))
        }
        "EX_True" => s("true".into()),
        "EX_False" => s("false".into()),
        "EX_Self" => s("self".into()),
        "EX_NoObject" | "EX_NoInterface" => s("None".into()),
        "EX_IntZero" => s("0".into()),
        "EX_IntOne" => s("1".into()),
        "EX_VectorConst" | "EX_RotationConst" | "EX_TransformConst" => s(format!("{}({})", tail3(&t), dumps(g("Value")))),
        "EX_JumpIfNot" => {
            let c = es(g("BooleanExpression"))?;
            s(format!("if not ({c}) goto {}", pstr(g("CodeOffset"))))
        }
        "EX_Jump" => s(format!("goto {}", pstr(g("CodeOffset")))),
        "EX_PushExecutionFlow" => s(format!("push_flow {}", pstr(g("PushingAddress")))),
        "EX_PopExecutionFlow" => s("pop_flow".into()),
        "EX_PopExecutionFlowIfNot" => s(format!("pop_flow_if_not ({})", es(g("BooleanExpression"))?)),
        "EX_ComputedJump" => s(format!("goto computed {}", es(g("CodeOffsetExpression"))?)),
        "EX_Return" => s(format!("return {}", es(g("ReturnExpression"))?)),
        "EX_DynamicCast" | "EX_MetaCast" | "EX_ObjToInterfaceCast" | "EX_CrossInterfaceCast" | "EX_InterfaceToObjCast" => {
            let c = objname(or(g("ClassPtr"), g("InterfaceClass")));
            s(format!("Cast<{c}>({})", es(or(g("Target"), g("CastExpression")))?))
        }
        "EX_PrimitiveCast" => ex(g("Target")),
        "EX_ArrayConst" | "EX_SetArray" | "EX_SetSet" | "EX_SetMap" | "EX_MapConst" | "EX_SetConst" => {
            let els = iter_or_empty(g("Elements"))?;
            let head = if truthy(g("AssigningProperty")) {
                match ex(g("AssigningProperty"))? {
                    Some(h) => h + " = ",
                    None => return Err("unsupported operand type(s) for +: 'NoneType' and 'str'".into()),
                }
            } else {
                String::new()
            };
            s(format!("{head}[{}]", join_e(&els, ", ")?))
        }
        "EX_StructConst" => {
            let n = objname(g("Struct"));
            let items = iter_or_empty(or(g("Properties"), g("Value")))?;
            s(format!("{n}{{{}}}", join_e(&items, ", ")?))
        }
        "EX_ArrayGetByRef" => {
            let a = es(g("ArrayVariable"))?;
            s(format!("{a}[{}]", es(g("ArrayIndex"))?))
        }
        "EX_SwitchValue" => {
            let it = es(g("IndexTerm"))?;
            let mut parts = vec![];
            for c in iter_or_empty(g("Cases"))? {
                let k = ex(py_get(&c, "CaseIndexValueTerm")?)?;
                let Some(k) = k else {
                    return Err("unsupported operand type(s) for +: 'NoneType' and 'str'".into());
                };
                let v = ex(py_get(&c, "CaseTerm")?)?;
                let Some(v) = v else {
                    return Err("can only concatenate str (not \"NoneType\") to str".into());
                };
                parts.push(format!("{k}: {v}"));
            }
            s(format!("switch({it}) {{{}; default: {}}}", parts.join("; "), es(g("DefaultTerm"))?))
        }
        "EX_EndOfScript" | "EX_Nothing" | "EX_Tracepoint" | "EX_WireTracepoint" | "EX_EndFunctionParms" => Ok(None),
        "EX_BindDelegate" => {
            let f = pstr(g("FunctionName"));
            let d = es(g("Delegate"))?;
            s(format!("BindDelegate({f}, {d}, {})", es(g("ObjectTerm"))?))
        }
        "EX_AddMulticastDelegate" | "EX_RemoveMulticastDelegate" => {
            let d = es(g("Delegate"))?;
            s(format!("{}({d}, {})", tail3(&t), es(g("DelegateToAdd"))?))
        }
        "EX_InstanceDelegate" => s(format!("delegate:{}", pstr(g("FunctionName")))),
        _ => {
            let mut parts = vec![];
            for (k, v) in e.as_object().unwrap() {
                if k == "Token" || k == "StatementIndex" || k == "ObjectPath" {
                    continue;
                }
                if has_token(v) {
                    parts.push(format!("{k}={}", es(v)?));
                } else if let Value::Array(a) = v {
                    if !a.is_empty() && has_token(&a[0]) {
                        let mut xs = vec![];
                        for x in a {
                            xs.push(ex(x)?.unwrap_or_default());
                        }
                        parts.push(format!("{k}=[{}]", xs.join(", ")));
                    }
                } else if matches!(v, Value::Number(_) | Value::String(_) | Value::Bool(_)) {
                    parts.push(format!("{k}={}", pstr(v)));
                }
            }
            s(format!("{}({})", tail3(&t), parts.join(", ")))
        }
    }
}

/// `f"{x:>6}"` for an int / str.
fn rjust6(v: &Value) -> String {
    format!("{:>6}", pstr(v))
}

pub fn run(a: Args) -> Result<i32> {
    let text = paths::read_text(&a.dump)?;
    let frx = match &a.filter {
        Some(f) => Some(Regex::new(f).context("bad function-name regex")?),
        None => None,
    };
    let out = std::io::stdout();
    let mut out = std::io::BufWriter::new(out.lock());
    let b = text.as_bytes();
    let mut i = 0;
    while i < b.len() {
        while i < b.len() && b[i] != b'{' && b[i] != b'[' {
            i += 1;
        }
        if i >= b.len() {
            break;
        }
        let mut it = serde_json::Deserializer::from_str(&text[i..]).into_iter::<Value>();
        let o = match it.next() {
            Some(Ok(v)) => v,
            Some(Err(e)) => anyhow::bail!("JSON decode error at byte {i}: {e}"),
            None => break,
        };
        i += it.byte_offset();
        if !(o.is_object() && get(&o, "Type").as_str() == Some("Function")) {
            continue;
        }
        let name = o.get("Name").cloned().unwrap_or(Value::String("?".into()));
        let name = pstr(&name);
        if let Some(r) = &frx {
            if !r.is_match(&name) {
                continue;
            }
        }
        writeln!(out, "\n=== FUNCTION {name}  ({})", objname(get(&o, "Outer")))?;
        let empty = Value::Array(vec![]);
        let code = or(get(&o, "ScriptBytecode"), &empty);
        let stmts = iter(code).unwrap_or_default();
        for st in &stmts {
            let s = match ex(st) {
                Ok(s) => s,
                Err(err) => Some(format!("<{} pp-error {err}>", pstr(get(st, "Token")))),
            };
            if let Some(s) = s {
                if !s.is_empty() {
                    let idx = st.get("StatementIndex").cloned().unwrap_or(Value::String("?".into()));
                    writeln!(out, "  {}: {s}", rjust6(&idx))?;
                }
            }
        }
    }
    out.flush()?;
    Ok(0)
}
