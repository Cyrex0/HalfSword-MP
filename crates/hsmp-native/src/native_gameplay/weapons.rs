//! Caller-owned complete weapon readback for the same pending application.
use super::*;
use serde_json::{json as value, Value as Json};
use std::collections::HashSet;

#[repr(C)]
pub struct Path {
    pub length: u32,
    pub pad: u32,
    pub data: [u16; 513],
}
impl Default for Path {
    fn default() -> Self {
        Self {
            length: 0,
            pad: 0,
            data: [0; 513],
        }
    }
}
#[repr(C)]
pub struct Name {
    pub length: u32,
    pub pad: u32,
    pub data: [u16; 129],
}
impl Default for Name {
    fn default() -> Self {
        Self {
            length: 0,
            pad: 0,
            data: [0; 129],
        }
    }
}
#[repr(C)]
#[derive(Default)]
pub struct Passport {
    pub pawn_index: u32,
    pub pad: u32,
    pub actor_class: Path,
    pub classes: [Path; 7],
    pub name: Name,
    pub id: i32,
    pub materials: [u8; 4],
    pub tier: u8,
    pub padding: [u8; 3],
    pub sizes: [[f64; 3]; 4],
    pub mass: [f64; 4],
    pub price: f64,
    pub colors: [[f32; 4]; 2],
}
fn text(units: &[u16], length: u32, maximum: usize, empty: bool) -> Result<String, String> {
    let length = length as usize;
    if length > maximum || length >= units.len() || units[length] != 0 {
        return Err("native weapon text extent changed".into());
    }
    let text = String::from_utf16(&units[..length]).map_err(|_| "native weapon text encoding")?;
    if (!empty && text.is_empty()) || text.len() > maximum || text.contains('\0') {
        return Err("native weapon text bound changed".into());
    }
    Ok(text)
}
fn path(v: &Path, empty: bool) -> Result<String, String> {
    if v.pad != 0 {
        return Err("native weapon path reserved field".into());
    }
    text(&v.data, v.length, 512, empty)
}
fn number(v: f64) -> Result<Json, String> {
    serde_json::Number::from_f64(v)
        .map(Json::Number)
        .ok_or_else(|| "native weapon nonfinite field".into())
}
fn numbers(v: impl IntoIterator<Item = f64>) -> Result<Json, String> {
    v.into_iter()
        .map(number)
        .collect::<Result<Vec<_>, _>>()
        .map(Json::Array)
}
fn passport(v: &Passport) -> Result<Json, String> {
    if v.pad != 0 || v.padding != [0; 3] || v.name.pad != 0 {
        return Err("native weapon reserved field".into());
    }
    let mut fields = serde_json::Map::new();
    for (key, class) in [
        "class",
        "head_sub1",
        "head_sub2",
        "head",
        "guard",
        "pommel",
        "grip",
    ]
    .into_iter()
    .zip(&v.classes)
    {
        fields.insert(key.into(), Json::String(path(class, true)?));
    }
    fields.insert("id".into(), value!(v.id));
    fields.insert(
        "name".into(),
        Json::String(text(&v.name.data, v.name.length, 128, true)?),
    );
    for (key, size) in ["head_size", "guard_size", "grip_size", "pommel_size"]
        .into_iter()
        .zip(v.sizes)
    {
        fields.insert(key.into(), numbers(size)?);
    }
    for (key, mass) in ["mass_head", "mass_guard", "mass_grip", "mass_pommel"]
        .into_iter()
        .zip(v.mass)
    {
        fields.insert(key.into(), number(mass)?);
    }
    for (key, material) in ["mat_steel", "mat_colored", "mat_wood", "mat_leather"]
        .into_iter()
        .zip(v.materials)
    {
        fields.insert(key.into(), value!(material));
    }
    for (key, color) in ["color_wood", "color_leather"].into_iter().zip(v.colors) {
        fields.insert(key.into(), numbers(color.map(f64::from))?);
    }
    fields.insert("price".into(), number(v.price)?);
    fields.insert("tier".into(), value!(v.tier));
    Ok(Json::Object(fields))
}
fn batch(original: &GameplayScene, aliases: &[u32], rows: &[Passport]) -> Result<Json, String> {
    if aliases.len() != original.result.entities.len() * 7 {
        return Err("native weapon alias extent changed".into());
    }
    let mut used = HashSet::new();
    let mut entities = Vec::with_capacity(original.result.entities.len());
    for (pawn_index, entity) in original.result.entities.iter().enumerate() {
        let aliases = &aliases[pawn_index * 7..(pawn_index + 1) * 7];
        for &alias in aliases {
            if alias != 0 {
                let row = rows
                    .get(alias as usize - 1)
                    .ok_or("native weapon alias row unavailable")?;
                if row.pawn_index as usize != pawn_index {
                    return Err("native weapon alias pawn changed".into());
                }
                used.insert(alias);
            }
        }
        let mut weapons = Vec::new();
        for (index, row) in rows
            .iter()
            .enumerate()
            .filter(|(_, row)| row.pawn_index as usize == pawn_index)
        {
            weapons.push(value!({"index": index + 1, "actor_class": path(&row.actor_class, false)?, "passport": passport(row)?}));
        }
        if weapons.len() > 7 {
            return Err("native weapon row bound changed".into());
        }
        entities.push(value!({"id":entity.reference.id,"incarnation":entity.reference.incarnation,"aliases":aliases,"weapons":weapons}));
    }
    if used.len() != rows.len() {
        return Err("native weapon row unbound".into());
    }
    // Epoch is published separately as the exact signed Lua integer, just as
    // gameplay_scene does. A u64 JSON number could otherwise pass through f64.
    Ok(
        value!({"dir_seq":original.directory.seq,"authority_tick":original.result.authority_tick,"entities":entities}),
    )
}

impl Native {
    pub unsafe fn native_gameplay_weapons(&mut self, L: *mut lua_State) -> c_int {
        unsafe {
            let top = lua_gettop(L);
            let result = (|| -> Result<(), String> {
                let original = self
                    .native_host
                    .gameplay
                    .applied
                    .clone()
                    .ok_or("no pending native gameplay apply")?;
                if !is_table(L, 1)
                    || int(L, 1, "epoch").map(|v| v as u64) != Some(original.directory.epoch)
                    || int(L, 1, "dir_seq") != Some(original.directory.seq as i64)
                    || int(L, 1, "authority_tick") != Some(original.result.authority_tick as i64)
                {
                    return Err("native weapon application result changed".into());
                }
                if !original.fresh() {
                    return Err("native gameplay result is stale".into());
                }
                let vt = reflect::vt().ok_or("gameplay reflection unavailable")?;
                let p = provider()?;
                let mut handles = Vec::with_capacity(original.result.entities.len());
                let mut capacity = 0usize;
                let mut origin = None;
                for row in &original.result.entities {
                    let pawn = self
                        .native_host
                        .gameplay
                        .pawns
                        .get(&row.reference.id)
                        .ok_or("native weapon original pawn missing")?;
                    if !pawn.finished
                        || pawn.descriptor.reference != row.reference
                        || !same_generation(&pawn.scene, &original)
                    {
                        return Err("native weapon original generation changed".into());
                    }
                    handles.push(pawn.handle);
                    capacity += pawn.descriptor.recipe.equipment.weapons.len();
                    origin.get_or_insert((pawn.world, pawn.controller));
                }
                if capacity > handles.len() * 7 {
                    return Err("native weapon source capacity changed".into());
                }
                let (world, controller) = origin.ok_or("native weapon empty roster")?;
                let mut context = Context::new(self, vt, original.clone(), world, controller)?;
                context.application = true;
                if !context.valid() {
                    return Err(context
                        .failure
                        .guard_error("native weapon admission changed".into()));
                }
                let mut aliases = vec![0; handles.len() * 7];
                let mut rows = (0..capacity)
                    .map(|_| Passport::default())
                    .collect::<Vec<_>>();
                let mut written = 0u32;
                let mut info = ResultInfo::default();
                if (p.weapons)(
                    handles.as_ptr(),
                    handles.len() as u32,
                    &context.ffi(),
                    aliases.as_mut_ptr(),
                    rows.as_mut_ptr(),
                    capacity as u32,
                    &mut written,
                    &mut info,
                ) != 1
                    || info.complete != 1
                {
                    let reason = info.reason();
                    return Err(context.failure.provider_error(
                        &reason,
                        format!("native weapon complete readback: {reason}"),
                    ));
                }
                if written as usize > capacity {
                    return Err("native weapon output capacity changed".into());
                }
                rows.truncate(written as usize);
                let copied = batch(&original, &aliases, &rows)?;
                if !context.valid() {
                    return Err(context
                        .failure
                        .guard_error("native weapon final admission changed".into()));
                }
                json(L, &copied)?;
                set_int(L, lua_gettop(L), "epoch", original.directory.epoch as i64);
                if !context.valid() {
                    return Err(context
                        .failure
                        .guard_error("native weapon Lua publication changed".into()));
                }
                Ok(())
            })();
            match result {
                Ok(()) => 1,
                Err(reason) => {
                    lua_settop(L, top);
                    nil_err(L, &reason)
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn weapon_abi_and_text_bounds_are_exact() {
        assert_eq!(std::mem::size_of::<Path>(), 1036);
        assert_eq!(std::mem::size_of::<Name>(), 268);
        assert_eq!(std::mem::size_of::<Passport>(), 8744);
        assert_eq!(std::mem::offset_of!(Passport, classes), 1044);
        assert_eq!(std::mem::offset_of!(Passport, name), 8296);
        assert_eq!(std::mem::offset_of!(Passport, id), 8564);
        assert_eq!(std::mem::offset_of!(Passport, sizes), 8576);
        assert_eq!(std::mem::offset_of!(Passport, mass), 8672);
        assert_eq!(std::mem::offset_of!(Passport, price), 8704);
        assert_eq!(std::mem::offset_of!(Passport, colors), 8712);
        assert!(text(&[0xd800, 0], 1, 128, true).is_err());
        assert!(text(&[0, 0], 1, 128, true).is_err());
        assert!(text(&[65, 66], 1, 128, true).is_err());
        assert!(text(&[65, 0], 2, 128, true).is_err());
        assert!(text(&[0], 0, 512, false).is_err());
        assert_eq!(text(&[0], 0, 512, true).unwrap(), "");
        let mut unicode = vec![0x00e9; 128];
        unicode.push(0);
        assert!(text(&unicode, 128, 128, true).is_err());
    }
    #[test]
    fn complete_passport_retains_all_fields_and_native_numeric_bits() {
        let mut row = Passport::default();
        row.id = -123;
        row.materials = [1, 2, 3, 255];
        row.tier = 127;
        row.mass[0] = f64::from_bits(0x3ff0000000000001);
        row.price = -0.;
        row.sizes[0][0] = -0.;
        row.colors[0][0] = -0.;
        let copied = passport(&row).unwrap();
        assert_eq!(copied.as_object().unwrap().len(), 25);
        assert_eq!(
            copied["mass_head"].as_f64().unwrap().to_bits(),
            row.mass[0].to_bits()
        );
        assert_eq!(
            copied["price"].as_f64().unwrap().to_bits(),
            (-0f64).to_bits()
        );
        assert_eq!(
            copied["head_size"][0].as_f64().unwrap().to_bits(),
            (-0f64).to_bits()
        );
        assert_eq!(
            copied["color_wood"][0].as_f64().unwrap().to_bits(),
            (-0f64).to_bits()
        );
        assert_eq!(copied["id"].as_i64(), Some(-123));
        assert_eq!(copied["mat_leather"].as_u64(), Some(255));
        row.colors[1][3] = f32::NAN;
        assert!(passport(&row).is_err());
        row.colors[1][3] = 0.;
        row.sizes[3][2] = f64::INFINITY;
        assert!(passport(&row).is_err());
    }
    fn row(pawn_index: u32) -> Passport {
        let mut row = Passport::default();
        row.pawn_index = pawn_index;
        let units = "/Game/Weapons/Original.Original_C"
            .encode_utf16()
            .collect::<Vec<_>>();
        row.actor_class.length = units.len() as u32;
        row.actor_class.data[..units.len()].copy_from_slice(&units);
        row
    }
    #[test]
    fn alias_binding_rejects_missing_cross_pawn_and_unbound_rows() {
        let scene = super::super::tests::scene_fixture();
        let rows = vec![row(0), row(1)];
        let aliases = [1, 0, 1, 0, 0, 0, 0, 2, 0, 0, 0, 0, 2, 0];
        let copied = batch(&scene, &aliases, &rows).unwrap();
        assert_eq!(copied["entities"][0]["aliases"][2], value!(1));
        assert_eq!(copied["entities"][1]["weapons"][0]["index"], value!(2));
        assert!(batch(&scene, &aliases[..13], &rows).is_err());
        let mut bad = aliases;
        bad[0] = 3;
        assert!(batch(&scene, &bad, &rows).is_err());
        bad = aliases;
        bad[0] = 2;
        assert!(batch(&scene, &bad, &rows).is_err());
        bad = aliases;
        bad[7] = 0;
        bad[12] = 0;
        assert!(batch(&scene, &bad, &rows).is_err());
        let mut invalid = rows;
        invalid.push(row(2));
        assert!(batch(&scene, &aliases, &invalid).is_err());
    }
    #[test]
    fn lua_publication_preserves_large_epoch_and_signed_zero() {
        let scene = super::super::tests::scene_fixture();
        let mut row = row(0);
        row.price = -0.;
        let copied = batch(&scene, &[1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0], &[row]).unwrap();
        unsafe {
            let L: *mut lua_State = mlua::ffi::luaL_newstate().cast();
            assert!(!L.is_null());
            json(L, &copied).unwrap();
            set_int(L, lua_gettop(L), "epoch", scene.directory.epoch as i64);
            assert_eq!(int(L, 1, "epoch").unwrap() as u64, scene.directory.epoch);
            rawget_str(L, 1, "entities");
            lua_rawgeti(L, -1, 1);
            rawget_str(L, lua_gettop(L), "weapons");
            lua_rawgeti(L, -1, 1);
            rawget_str(L, lua_gettop(L), "passport");
            rawget_str(L, lua_gettop(L), "price");
            let mut actual = 0;
            assert_eq!(
                lua_tonumberx(L, -1, &mut actual).to_bits(),
                (-0f64).to_bits()
            );
            assert_eq!(actual, 1);
            mlua::ffi::lua_close(L.cast());
        }
    }
}
