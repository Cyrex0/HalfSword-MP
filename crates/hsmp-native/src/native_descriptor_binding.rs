//! Bounded raw Lua table -> exact source recipe. No engine access, metamethod,
//! implicit field default, or network publication occurs in this parser.
use crate::lua::*;
use hsmp_server::native_descriptor::{
    MAX_RECIPE_DEPTH, MAX_RECIPE_NODES, MAX_RECIPE_STRING_BYTES, SourceRecipe,
};
use serde_json::{Map, Number, Value};
use std::ffi::c_int;
unsafe fn copied_value(
    L: *mut lua_State,
    index: c_int,
    depth: usize,
    nodes: &mut usize,
    nullable_field: bool,
) -> Result<Value, String> {
    unsafe {
        *nodes += 1;
        if depth > MAX_RECIPE_DEPTH || *nodes > MAX_RECIPE_NODES {
            return Err(format!(
                "source recipe table bound nodes={} depth={depth}",
                *nodes
            ));
        }
        let index = lua_absindex(L, index);
        Ok(match lua_type(L, index) {
            LUA_TBOOLEAN => {
                let b = lua_toboolean(L, index) != 0;
                if nullable_field && !b {
                    Value::Null
                } else {
                    Value::Bool(b)
                }
            }
            LUA_TNUMBER => {
                if lua_isinteger(L, index) != 0 {
                    Value::Number(Number::from(arg_int(L, index).ok_or("source integer")?))
                } else {
                    Value::Number(
                        Number::from_f64(arg_num(L, index).ok_or("source number")?)
                            .ok_or("source finite number")?,
                    )
                }
            }
            LUA_TSTRING => {
                let s = arg_str(L, index).ok_or("source utf8")?;
                if s.len() > MAX_RECIPE_STRING_BYTES {
                    return Err("source recipe string bound".into());
                }
                Value::String(s.to_owned())
            }
            LUA_TTABLE => {
                if lua_checkstack(L, 6) == 0 {
                    return Err("source stack bound".into());
                }
                let n = lua_rawlen(L, index) as usize;
                if n > MAX_RECIPE_NODES - *nodes {
                    return Err("source array bound".into());
                }
                // Empty arrays stay explicit; empty objects fail required fields.
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
                        arg_int(L, -2)
                            .filter(|x| *x >= 1 && *x as usize <= n)
                            .ok_or("source dense array keys")?;
                        count += 1;
                        pop(L, 1);
                    }
                    if count != n {
                        return Err("source dense array count".into());
                    }
                    if n != 0 && depth == MAX_RECIPE_DEPTH {
                        return Err("source recipe table bound".into());
                    }
                    let mut a = Vec::with_capacity(n);
                    for i in 1..=n {
                        lua_rawgeti(L, index, i as i64);
                        a.push(copied_value(L, -1, depth + 1, nodes, false)?);
                        pop(L, 1);
                    }
                    Value::Array(a)
                } else {
                    let mut keys = Vec::new();
                    lua_pushnil(L);
                    while lua_next(L, index) != 0 {
                        let k = arg_str(L, -2).ok_or("source object key")?;
                        if k.len() > 128
                            || keys.len() >= 256
                            || keys.len() >= MAX_RECIPE_NODES - *nodes
                        {
                            return Err("source object bound".into());
                        }
                        keys.push(k.to_owned());
                        pop(L, 1);
                    }
                    keys.sort();
                    let mut o = Map::new();
                    for k in keys {
                        rawget_str(L, index, &k);
                        let v = copied_value(
                            L,
                            -1,
                            depth + 1,
                            nodes,
                            k == "collision" || k == "spline_profile",
                        )?;
                        pop(L, 1);
                        o.insert(k, v);
                    }
                    Value::Object(o)
                }
            }
            _ => return Err("source copied scalar/table required".into()),
        })
    }
}
/// Restores the original stack on every refusal, without raising into Lua.
pub unsafe fn read_recipe(L: *mut lua_State, index: c_int) -> Result<SourceRecipe, String> {
    unsafe {
        let top = lua_gettop(L);
        let result = (|| {
            let value = copied_value(L, index, 0, &mut 0, false)?;
            let recipe: SourceRecipe =
                serde_json::from_value(value).map_err(|_| "source recipe schema")?;
            let stats = recipe.encoding_stats_for_diagnostics();
            if let Err(reason) = recipe.validate() {
                let counts = match &stats {
                    Ok(stats) => stats.diagnostic(),
                    Err(reason) => format!("encoding_stats_unavailable={reason}"),
                };
                return Err(format!(
                    "{}: {counts}",
                    recipe.validation_diagnostic(reason)
                ));
            }
            let stats = stats.map_err(str::to_owned)?;
            recipe
                .canonical_bytes()
                .map_err(|reason| format!("{reason}: {}", stats.diagnostic()))?;
            Ok(recipe)
        })();
        lua_settop(L, top);
        result
    }
}
