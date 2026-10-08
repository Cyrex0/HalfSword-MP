//! Bounded raw Lua table -> exact source recipe. No engine access, metamethod,
//! implicit field default, or network publication occurs in this parser.
use crate::lua::*;
use hsmp_server::native_descriptor::{MAX_RECIPE_BYTES, SourceRecipe};
use std::ffi::c_int;

fn quoted(out: &mut String, s: &str) -> Result<(), String> {
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if c < ' ' => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    if out.len() > MAX_RECIPE_BYTES {
        return Err("source recipe byte bound".into());
    }
    Ok(())
}
unsafe fn encode(
    L: *mut lua_State,
    index: c_int,
    out: &mut String,
    depth: usize,
    nodes: &mut usize,
) -> Result<(), String> {
    unsafe {
        *nodes += 1;
        if depth > 24 || *nodes > 16000 || out.len() > MAX_RECIPE_BYTES {
            return Err("source recipe table bound".into());
        }
        let index = lua_absindex(L, index);
        match lua_type(L, index) {
            LUA_TBOOLEAN => out.push_str(if lua_toboolean(L, index) != 0 {
                "true"
            } else {
                "false"
            }),
            LUA_TNUMBER => {
                if lua_isinteger(L, index) != 0 {
                    out.push_str(&arg_int(L, index).ok_or("source integer")?.to_string());
                } else {
                    let value = arg_num(L, index)
                        .filter(|x| x.is_finite())
                        .ok_or("source finite number")?;
                    let mut text = value.to_string();
                    if !text.contains('.') && !text.contains('e') && !text.contains('E') {
                        text.push_str(".0");
                    }
                    out.push_str(&text);
                }
            }
            LUA_TSTRING => quoted(out, arg_str(L, index).ok_or("source utf8")?)?,
            LUA_TTABLE => {
                if lua_checkstack(L, 6) == 0 {
                    return Err("source stack bound".into());
                }
                let n = lua_rawlen(L, index) as usize;
                if n > 16000 {
                    return Err("source array bound".into());
                }
                // Empty tables encode as arrays. Every recipe object has required
                // fields, so an empty object still fails the typed schema.
                lua_pushnil(L);
                let nonempty = lua_next(L, index) != 0;
                let array = n > 0 || !nonempty;
                if nonempty {
                    pop(L, 2);
                }
                if array {
                    let mut count = 0;
                    lua_pushnil(L);
                    while lua_next(L, index) != 0 {
                        let key = arg_int(L, -2)
                            .filter(|x| *x >= 1 && *x as usize <= n)
                            .ok_or("source dense array keys")?;
                        let _ = key;
                        count += 1;
                        pop(L, 1);
                    }
                    if count != n {
                        return Err("source dense array count".into());
                    }
                    out.push('[');
                    for i in 1..=n {
                        if i != 1 {
                            out.push(',');
                        }
                        lua_rawgeti(L, index, i as i64);
                        encode(L, -1, out, depth + 1, nodes)?;
                        pop(L, 1);
                    }
                    out.push(']');
                } else {
                    let mut keys = Vec::new();
                    lua_pushnil(L);
                    while lua_next(L, index) != 0 {
                        let key = arg_str(L, -2).ok_or("source object key")?;
                        if key.len() > 128 || keys.len() >= 256 {
                            return Err("source object bound".into());
                        }
                        keys.push(key.to_owned());
                        pop(L, 1);
                    }
                    keys.sort();
                    out.push('{');
                    for (i, key) in keys.iter().enumerate() {
                        if i != 0 {
                            out.push(',');
                        }
                        quoted(out, key)?;
                        out.push(':');
                        rawget_str(L, index, key);
                        encode(L, -1, out, depth + 1, nodes)?;
                        pop(L, 1);
                    }
                    out.push('}');
                }
            }
            _ => return Err("source copied scalar/table required".into()),
        }
        if out.len() > MAX_RECIPE_BYTES {
            return Err("source recipe byte bound".into());
        }
        Ok(())
    }
}
/// Reads without raising into Lua; restores the original stack on every refusal.
pub unsafe fn read_recipe(L: *mut lua_State, index: c_int) -> Result<SourceRecipe, String> {
    unsafe {
        let top = lua_gettop(L);
        let mut bytes = String::new();
        let result = encode(L, index, &mut bytes, 0, &mut 0)
            .and_then(|()| SourceRecipe::decode_recipe(bytes.as_bytes()).map_err(str::to_owned));
        lua_settop(L, top);
        result
    }
}
