//! Read actual engine key state without dispatching local Willie actions.
//! Verified getter layouts are cached until the guarded native world is dropped.
use crate::{
    lua::*,
    native::Native,
    reflect::{self, wide, HsmpProp, HsmpReflect},
    sample::{fname, props_of, Params},
};
use std::{
    collections::HashMap,
    ffi::{c_int, c_void},
};

#[derive(Clone, Copy)]
struct Getter {
    weak: u64,
    key: usize,
    out: HsmpProp,
    down: bool,
}
pub(crate) struct Reader {
    class: u64,
    analog: Getter,
    down: Getter,
    keys: HashMap<String, u64>,
}
impl Reader {
    unsafe fn new(vt: &HsmpReflect) -> Result<Self, String> {
        unsafe {
            let class = (vt.find)(wide("/Script/Engine.PlayerController").as_ptr());
            if class.is_null() {
                return Err("input controller class unavailable".into());
            }
            let class = reflect::keep(vt, class).ok_or("input controller class weak")?;
            let key_struct = (vt.find)(wide("/Script/InputCore.Key").as_ptr());
            if key_struct.is_null() {
                return Err("input FKey unavailable".into());
            }
            let (props, size) = props_of(vt, key_struct);
            if size != 24
                || props.len() != 1
                || props[0].name != fname(vt, "KeyName", true)
                || props[0].offset != 0
                || props[0].size != 8
                || props[0].cls != fname(vt, "NameProperty", true)
            {
                return Err("input FKey ABI".into());
            }
            Ok(Self {
                class,
                analog: Self::getter(vt, false)?,
                down: Self::getter(vt, true)?,
                keys: HashMap::new(),
            })
        }
    }
    unsafe fn getter(vt: &HsmpReflect, down: bool) -> Result<Getter, String> {
        unsafe {
            let path = if down {
                "/Script/Engine.PlayerController:IsInputKeyDown"
            } else {
                "/Script/Engine.PlayerController:GetInputAnalogKeyState"
            };
            let f = (vt.find)(wide(path).as_ptr());
            if f.is_null() {
                return Err("input engine getter unavailable".into());
            }
            let (fields, size) = props_of(vt, f);
            let key = fields
                .iter()
                .find(|p| p.name == fname(vt, "Key", true))
                .ok_or("input key parameter")?;
            let out = *fields
                .iter()
                .find(|p| p.name == fname(vt, "ReturnValue", true))
                .ok_or("input key return")?;
            if fields.len() != 2
                || size <= 0
                || size > 64
                || key.cls != fname(vt, "StructProperty", true)
                || key.sub != fname(vt, "Key", true)
                || key.size != 24
                || key.offset < 0
                || key.offset + 24 > size
                || out.offset < 0
                || out.size <= 0
                || out.offset + out.size > size
                || if down {
                    out.cls != fname(vt, "BoolProperty", true)
                        || out.size != 1
                        || out.bool_mask == 0
                        || out.bool_offset as i32 >= out.size
                } else {
                    out.cls != fname(vt, "FloatProperty", true) || out.size != 4
                }
            {
                return Err("input getter ABI".into());
            }
            Ok(Getter {
                weak: reflect::keep(vt, f).ok_or("input getter weak")?,
                key: key.offset as usize,
                out,
                down,
            })
        }
    }
    unsafe fn read(
        &mut self,
        vt: &HsmpReflect,
        pc: *mut c_void,
        key: &str,
        guard: impl Fn() -> bool,
    ) -> Result<(f64, f64), String> {
        unsafe {
            let class = reflect::get(vt, self.class);
            if !guard() || class.is_null() || (vt.is_a)(pc, class) != 1 {
                return Err("input controller class/world changed".into());
            }
            let weak = reflect::keep(vt, pc).ok_or("input controller weak")?;
            let name = if let Some(name) = self.keys.get(key) {
                *name
            } else {
                if self.keys.len() >= 96 {
                    self.keys.clear();
                }
                let name = fname(vt, key, true);
                self.keys.insert(key.to_owned(), name);
                name
            };
            let call = |getter: Getter| -> Result<f64, String> {
                let f = reflect::get(vt, getter.weak);
                if !guard() || f.is_null() || reflect::get(vt, weak) != pc {
                    return Err("input controller/getter changed".into());
                }
                let mut params = Params([0; crate::sample::PARAMS_BYTES]);
                params.0[getter.key..getter.key + 8].copy_from_slice(&name.to_le_bytes());
                (vt.call)(pc, f, params.0.as_mut_ptr().cast());
                if !guard() || reflect::get(vt, weak) != pc {
                    return Err("input controller/world changed".into());
                }
                let at = getter.out.offset as usize;
                Ok(if getter.down {
                    ((params.0[at + getter.out.bool_offset as usize] & getter.out.bool_mask) != 0)
                        as u8 as f64
                } else {
                    f32::from_le_bytes(params.0[at..at + 4].try_into().map_err(|_| "input float")?)
                        as f64
                })
            };
            let analog = call(self.analog)?;
            let down = call(self.down)?;
            if !analog.is_finite() {
                return Err("input key finite".into());
            }
            Ok((analog, down))
        }
    }
}
impl Native {
    pub unsafe fn native_key_state(&mut self, L: *mut lua_State) -> c_int {
        unsafe {
            if !self.native_host.is_client() || !self.sample.world_ok {
                return nil_err(L, "client input role/world");
            }
            let result = (|| -> Result<(), String> {
                let vt = reflect::vt().ok_or("input reflection unavailable")?;
                let address = arg_int(L, 1)
                    .filter(|v| *v != 0)
                    .ok_or("input controller")?;
                if !is_table(L, 2) || lua_rawlen(L, 2) > 96 {
                    return Err("input key bound".into());
                }
                let key = self
                    .world_key
                    .clone()
                    .ok_or("input world token unavailable")?;
                let mut reader = match self.presentation.input.take() {
                    Some(r) => r,
                    None => Reader::new(vt)?,
                };
                let n = lua_rawlen(L, 2);
                let mut values = Vec::with_capacity(n as usize);
                let sampled = (|| -> Result<(), String> {
                    for i in 1..=n {
                        lua_rawgeti(L, 2, i as i64);
                        let name = arg_str(L, -1)
                            .filter(|s| !s.is_empty() && s.len() <= 128)
                            .map(str::to_owned);
                        pop(L, 1);
                        let name = name.ok_or("input key name")?;
                        values.push(reader.read(
                            vt,
                            address as usize as *mut c_void,
                            &name,
                            || {
                                self.native_guard_ok(vt)
                                    && self.world_key.as_deref() == Some(key.as_slice())
                            },
                        )?);
                    }
                    Ok(())
                })();
                if self.sample.world_ok && self.world_key.as_deref() == Some(key.as_slice()) {
                    self.presentation.input = Some(reader);
                }
                sampled?;
                lua_createtable(L, n as c_int, 0);
                let table = lua_gettop(L);
                for (i, (analog, down)) in values.iter().enumerate() {
                    lua_createtable(L, 2, 0);
                    let row = lua_gettop(L);
                    fill_array(L, row, [*analog, *down].into_iter());
                    lua_rawseti(L, table, i as i64 + 1);
                }
                Ok(())
            })();
            match result {
                Ok(()) => 1,
                Err(e) => nil_err(L, &e),
            }
        }
    }
}
