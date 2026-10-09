//! Raw-table parser fixtures only. This establishes no native scene evidence.
use hsmp_native::native_descriptor_binding::read_recipe;
use hsmp_server::native_descriptor::SourceRecipe;
use mlua::ffi;
use serde_json::Value;
use std::ffi::{CStr, CString};

struct State(*mut ffi::lua_State);
impl State {
    fn new() -> Self {
        unsafe {
            let l = ffi::luaL_newstate();
            assert!(!l.is_null());
            ffi::luaL_openlibs(l);
            let fixture: Value = serde_json::from_str(include_str!(
                "../../../tools/hsmp-tools/lua-tests/fixtures/native_source_recipe.json"
            ))
            .unwrap();
            push_json(l, &fixture);
            ffi::lua_setglobal(l, c"recipe".as_ptr());
            Self(l)
        }
    }
    fn run(&self, code: &str) {
        unsafe {
            let text = CString::new(code).unwrap();
            assert_eq!(ffi::luaL_loadstring(self.0, text.as_ptr()), 0);
            let rc = ffi::lua_pcall(self.0, 0, 0, 0);
            if rc != 0 {
                let message = CStr::from_ptr(ffi::lua_tostring(self.0, -1));
                panic!("fixture Lua: {}", message.to_string_lossy());
            }
        }
    }
    fn read(&self) -> Result<SourceRecipe, String> {
        unsafe {
            ffi::lua_getglobal(self.0, c"recipe".as_ptr());
            let top = ffi::lua_gettop(self.0);
            let result = read_recipe(self.0.cast(), -1);
            assert_eq!(
                ffi::lua_gettop(self.0),
                top,
                "parser restores original stack"
            );
            ffi::lua_pop(self.0, 1);
            result
        }
    }
}
impl Drop for State {
    fn drop(&mut self) {
        unsafe { ffi::lua_close(self.0) }
    }
}
unsafe fn push_json(l: *mut ffi::lua_State, value: &Value) {
    unsafe {
        match value {
            Value::Null => ffi::lua_pushnil(l),
            Value::Bool(v) => ffi::lua_pushboolean(l, i32::from(*v)),
            Value::Number(v) => {
                if let Some(i) = v.as_i64() {
                    ffi::lua_pushinteger(l, i);
                } else {
                    ffi::lua_pushnumber(l, v.as_f64().unwrap());
                }
            }
            Value::String(v) => {
                ffi::lua_pushlstring(l, v.as_ptr().cast(), v.len());
            }
            Value::Array(v) => {
                ffi::lua_createtable(l, v.len() as i32, 0);
                let table = ffi::lua_absindex(l, -1);
                for (i, item) in v.iter().enumerate() {
                    push_json(l, item);
                    ffi::lua_rawseti(l, table, i as i64 + 1);
                }
            }
            Value::Object(v) => {
                ffi::lua_createtable(l, 0, v.len() as i32);
                let table = ffi::lua_absindex(l, -1);
                for (key, item) in v {
                    ffi::lua_pushlstring(l, key.as_ptr().cast(), key.len());
                    if item.is_null() && matches!(key.as_str(), "collision" | "spline_profile") {
                        // Native source Lua explicitly sends false for these
                        // required not-applicable fields; nil means missing.
                        ffi::lua_pushboolean(l, 0);
                    } else {
                        push_json(l, item);
                    }
                    ffi::lua_rawset(l, table);
                }
            }
        }
    }
}

#[test]
fn copied_native_widths_empty_arrays_and_explicit_false_survive() {
    let lua = State::new();
    let recipe = lua.read().unwrap();
    recipe.validate_mirror_profile().unwrap();
    assert_eq!(
        recipe.passport.height.to_bits(),
        0.500000000000123f64.to_bits()
    );
    assert!(!recipe.construction.is_zombie);
    assert!(recipe.equipment.weapons.is_empty());
    lua.run("recipe.passport.actor_class=''; recipe.passport.name='Unicode: Ж 雨'; recipe.passport.height=-0.0");
    let recipe = lua.read().unwrap();
    assert_eq!(recipe.passport.height.to_bits(), (-0.0f64).to_bits());
    assert!(recipe.passport.actor_class.is_empty());
    assert_eq!(recipe.passport.name, "Unicode: Ж 雨");
    for value in [
        f64::MIN_POSITIVE,
        f64::from_bits(1),
        f64::MAX,
        1.0000000000000002,
        -3.312785073519018e-130,
    ] {
        unsafe {
            ffi::lua_getglobal(lua.0, c"recipe".as_ptr());
            ffi::lua_getfield(lua.0, -1, c"passport".as_ptr());
            ffi::lua_pushnumber(lua.0, value);
            ffi::lua_setfield(lua.0, -2, c"height".as_ptr());
            ffi::lua_pop(lua.0, 2);
        }
        assert_eq!(
            lua.read().unwrap().passport.height.to_bits(),
            value.to_bits()
        );
    }
    lua.run("recipe.construction.is_zombie=nil");
    assert!(
        lua.read().is_err(),
        "absent native false does not become false"
    );
}

#[test]
fn raw_access_refuses_missing_sparse_and_unsupported_values() {
    for mutation in [
        "recipe.passport.hair_length=nil",
        "recipe.components[1].component_class=nil",
        "recipe.components[1].scene=nil",
        "recipe.components[1].collision=nil",
        "recipe.components[1].collision=false",
        "recipe.components[1].spline_profile=nil",
        "recipe.components[1].spline_profile=true",
        "recipe.invented_default=true",
        "recipe.components[1].relative.scale[2]=nil",
        "recipe.components[1].relative.scale.extra=1",
        "recipe.passport.height=0/0",
        "recipe.passport.height=math.huge",
        "recipe.passport.height=function()end",
        "recipe.passport.height=coroutine.create(function()end)",
        "recipe.passport.name=string.rep('x',70000)",
        "recipe.cycle=recipe",
    ] {
        let lua = State::new();
        lua.run(mutation);
        assert!(lua.read().is_err(), "must refuse: {mutation}");
    }
    let lua = State::new();
    lua.run("setmetatable(recipe.components,{__len=function()error('metamethod invoked')end}); setmetatable(recipe.passport,{__index=function()error('metamethod invoked')end})");
    lua.read().unwrap();
    lua.run("recipe.passport.hair_length=nil");
    assert!(
        lua.read().is_err(),
        "raw required field does not invoke __index"
    );
}

#[test]
fn spline_profile_has_exact_counts_and_required_null_sentinel() {
    let lua = State::new();
    assert!(lua.read().unwrap().components[0].spline_profile.is_none());
    lua.run("local c=recipe.components[1]; c.name='Offline live spline'; c.role='attachment'; c.component_class='/Script/Engine.SplineComponent'; c.kind='spline'; c.geometry='native_spline'; c.scene={type='not_applicable'}; c.vertex_state='not_applicable'; c.asset=''; c.skeleton=''; c.physics_asset=''; c.bones={}; c.spline_profile={position_count=3,rotation_count=2,scale_count=1,reparam_count=21,metadata_null=true}");
    let recipe = lua.read().unwrap();
    recipe.validate_mirror_profile().unwrap();
    let profile = recipe.components[0].spline_profile.as_ref().unwrap();
    assert_eq!(
        (
            profile.position_count,
            profile.rotation_count,
            profile.scale_count,
            profile.reparam_count,
            profile.metadata_null
        ),
        (3, 2, 1, 21, true)
    );
    for mutation in [
        "recipe.components[1].spline_profile=false",
        "recipe.components[1].spline_profile=nil",
        "recipe.components[1].spline_profile.metadata_null=false",
        "recipe.components[1].spline_profile.metadata_null=nil",
        "recipe.components[1].spline_profile.position_count=nil",
        "recipe.components[1].spline_profile.rotation_count=nil",
        "recipe.components[1].spline_profile.scale_count=nil",
        "recipe.components[1].spline_profile.reparam_count=nil",
        "recipe.components[1].spline_profile.position_count=65",
        "recipe.components[1].spline_profile.rotation_count=65",
        "recipe.components[1].spline_profile.scale_count=65",
        "recipe.components[1].spline_profile.reparam_count=1025",
        "recipe.components[1].spline_profile.position_count=2.5",
        "recipe.components[1].spline_profile.extra=0",
        "recipe.components[1].collision=false",
        "recipe.components[1].component_class='/Script/Engine.CustomSplineSubclass'",
    ] {
        let lua = State::new();
        lua.run("local c=recipe.components[1]; c.component_class='/Script/Engine.SplineComponent'; c.kind='spline'; c.geometry='native_spline'; c.scene={type='not_applicable'}; c.vertex_state='not_applicable'; c.asset=''; c.skeleton=''; c.physics_asset=''; c.bones={}; c.spline_profile={position_count=3,rotation_count=2,scale_count=1,reparam_count=21,metadata_null=true}");
        lua.read().unwrap();
        lua.run(mutation);
        assert!(lua.read().is_err(), "must refuse: {mutation}");
    }
}

#[test]
fn scene_collision_sentinel_is_explicit_required_and_class_qualified() {
    let lua = State::new();
    lua.run("local c=recipe.components[1]; c.id=2; c.name='Offline scene'; c.role='anchor'; c.component_class='/Script/Engine.SceneComponent'; c.kind='scene'; c.geometry='not_applicable'; c.scene={type='scene'}; c.vertex_state='not_applicable'; c.asset=''; c.skeleton=''; c.physics_asset=''; c.bones={}; c.collision=false");
    let recipe = lua.read().unwrap();
    assert!(recipe.components[0].collision.is_none());
    recipe.validate_mirror_profile().unwrap();
    lua.run("recipe.components[1].collision=nil");
    assert!(
        lua.read().is_err(),
        "missing is not observed not-applicable"
    );
    lua.run("recipe.components[1].collision=false; recipe.components[1].component_class='/Script/Engine.CapsuleComponent'; recipe.components[1].scene={type='hidden_capsule'}; recipe.components[1].hidden=true");
    assert!(
        lua.read().is_err(),
        "primitive capsule cannot claim absent collision"
    );
}

#[test]
fn exact_camera_and_spring_arm_raw_fields_do_not_default() {
    for (class, evidence) in [
        ("CameraComponent", "{type='camera'}"),
        (
            "SpringArmComponent",
            "{type='spring_arm',draw_debug_lag_markers=false,socket_name='OfflineExactSocket'}",
        ),
    ] {
        let lua = State::new();
        lua.run(&format!("local c=recipe.components[1]; c.role='anchor'; c.component_class='/Script/Engine.{class}'; c.kind='scene'; c.geometry='not_applicable'; c.scene={evidence}; c.vertex_state='not_applicable'; c.asset=''; c.skeleton=''; c.physics_asset=''; c.bones={{}}; c.collision=false"));
        let recipe = lua.read().unwrap();
        recipe.validate_mirror_profile().unwrap();
        assert!(recipe.components[0].collision.is_none());
        if class == "SpringArmComponent" {
            for mutation in [
                "recipe.components[1].scene.draw_debug_lag_markers=nil",
                "recipe.components[1].scene.draw_debug_lag_markers=true",
                "recipe.components[1].scene.socket_name=nil",
                "recipe.components[1].scene.socket_name=''",
                "recipe.components[1].scene.socket_name='None'",
                "recipe.components[1].scene.extra=0",
            ] {
                lua.run("recipe.components[1].scene={type='spring_arm',draw_debug_lag_markers=false,socket_name='OfflineExactSocket'}");
                lua.read().unwrap();
                lua.run(mutation);
                assert!(lua.read().is_err(), "must refuse: {mutation}");
            }
        }
    }
}

#[test]
fn native_empty_static_raw_fields_are_explicit_and_complete() {
    let lua = State::new();
    let reset = "local c=recipe.components[1]; c.component_class='/Script/Engine.StaticMeshComponent'; c.kind='static'; c.geometry='native_empty'; c.asset=''; c.skeleton=''; c.physics_asset=''; c.bones={}; c.vertex_state='not_applicable'; c.collision={enabled=3,object_type=3,profile='ObservedHolder',responses={0,1,2,0,1,2,0,1,2,0,1,2,0,1,2,0,1,2,0,1,2,0,1,2,0,1,2,0,1,2,0,1},simulating=true}";
    lua.run(reset);
    let recipe = lua.read().unwrap();
    recipe.validate_mirror_profile().unwrap();
    assert_eq!(
        recipe.components[0].geometry,
        hsmp_server::native_descriptor::Geometry::NativeEmpty
    );
    assert_eq!(
        recipe.components[0].collision.as_ref().unwrap().profile,
        "ObservedHolder"
    );
    for mutation in [
        "recipe.components[1].asset=nil",
        "recipe.components[1].asset='/Game/Test/Other.Other'",
        "recipe.components[1].component_class=nil",
        "recipe.components[1].component_class='/Script/Engine.CustomStaticSubclass'",
        "recipe.components[1].vertex_state=nil",
        "recipe.components[1].vertex_state='native_asset'",
        "recipe.components[1].collision=false",
        "recipe.components[1].collision=nil",
    ] {
        lua.run(reset);
        lua.run(mutation);
        assert!(lua.read().is_err(), "counterfeit native empty: {mutation}");
    }
}

#[test]
fn sparse_native_blocked_slots_preserve_observed_false() {
    let lua = State::new();
    unsafe {
        let armor: Value = serde_json::from_str(include_str!(
            "../../../tools/hsmp-tools/lua-tests/fixtures/native_armor_passport.json"
        ))
        .unwrap();
        push_json(lua.0, &armor);
        ffi::lua_setglobal(lua.0, c"armor".as_ptr());
    }
    lua.run("recipe.equipment.armor={{slot=armor.pslot,passport=armor}}; armor.slots_blocked={{slot=2,value=false},{slot=5,value=true}}");
    let recipe = lua.read().unwrap();
    let blocked = &recipe.equipment.armor[0].passport.slots_blocked;
    assert_eq!(blocked.len(), 2);
    assert_eq!(blocked[0].slot, 2);
    assert!(!blocked[0].value);
    assert_eq!(blocked[1].slot, 5);
    lua.run("armor.slots_blocked[1].value=nil");
    assert!(lua.read().is_err());
}
