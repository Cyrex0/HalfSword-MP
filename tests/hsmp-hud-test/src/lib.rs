//! Test-only crate for `mods/HSMPHud` (the in-match HUD).
//!
//! The tests live in `tests/hud.rs` (Rust runner, mlua/Lua 5.4) and
//! `tests/mock_ue.lua` (a mocked UE4SS + UMG: StaticConstructObject, canvas
//! slots with anchors, a native `UI_HUD_C`, the viewport, `FindAllOf`, the
//! game-thread loop API, `RegisterKeyBind`, a fake clock). Nothing here ships.
//!
//! ```text
//! cargo test -p hsmp-hud-test
//! ```
