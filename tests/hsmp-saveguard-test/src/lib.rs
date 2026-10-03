//! Test-only crate for `mods/shared/hsmp_saveguard.lua`.
//!
//! The tests live in `tests/saveguard.rs` (Rust runner, mlua/Lua 5.4) and
//! `tests/saveguard_spec.lua` (scenarios run against a mocked UE4SS
//! `RegisterHook` / `RemoteUnrealParam`). Nothing here ships.
