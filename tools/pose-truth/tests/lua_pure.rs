//! Runs HSMPAvatars' PURE block (velocity servo, target advance, v2 play-line
//! parser) in Lua 5.4: tests/servo_pure.lua, against the shipping file.
#[test]
fn hsmpavatars_pure_servo_and_parser() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let avatars = root.join("mods/HSMPAvatars/Scripts/avatars_pure.lua");
    let test = std::fs::read(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/servo_pure.lua")).unwrap();
    let lua = mlua::Lua::new();
    let arg = lua.create_table().unwrap();
    arg.set(1, avatars.to_string_lossy().to_string()).unwrap();
    lua.globals().set("arg", arg).unwrap();
    lua.load(&test[..]).set_name("servo_pure.lua").exec().unwrap();
}
