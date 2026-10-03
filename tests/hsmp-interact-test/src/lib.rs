//! Test-only crate for `mods/HSMPInteract` (the interaction
//! channel: grabs and shoves on stand-ins reach the real body's owner).
//!
//! The tests live in `tests/interact.rs` (Rust runner, mlua/Lua 5.4) and
//! `tests/mock_ue.lua` (a mocked UE4SS: objects, Willies with hero bones,
//! PhysicsHandles, AddImpulseAtLocation, RegisterHook, the game-thread loop
//! API and a fake clock). Nothing here ships.
//!
//! ```text
//! cargo test -p hsmp-interact-test
//! ```
