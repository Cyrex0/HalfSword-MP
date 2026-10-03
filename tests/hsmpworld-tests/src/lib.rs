//! Test-only crate for HSMPWorld (see tests/mock_world.rs).
//!
//! ```text
//! cargo test -p hsmpworld-tests
//! (loads mods/HSMPWorld/Scripts/main.lua and mods/shared/)
//! ```
//!
//! `ue4ss_mock.lua` emulates the slice of UE4SS 3.0.1 / UE 5.4 that
//! HSMPWorld touches. UFunction calls are arity-checked exactly like UE4SS
//! ("UFunction expected N parameters, received M"), objects are invalidated
//! like pending-kill actors, and every call on an invalid object is recorded
//! as a violation.
