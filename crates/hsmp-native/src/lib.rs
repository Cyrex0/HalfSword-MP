//! HSMPNative: the game-side half of HSMP-SHM (docs/development/ipc-shared-memory.md).
//! A Rust staticlib linked into
//! - `Mods/HSMPNative/dlls/main.dll` (option F, the UE4SS C++ mod in `cpp/`, which calls
//!   [`api::hsmp_native_open`] from `on_lua_start` for every `HSMP*` Lua mod), and
//! - `hsmp_lua.dll` (option E, [`api::luaopen_hsmp_lua`]),
//!
//! from the same source. It owns the game's shared-memory segment (one per process), the
//! Lua-table <-> record marshalling and the S2G event fan-out.
//! The embedded native endpoint (`native_host.rs`) owns a network thread that receives only
//! immutable DTOs and bounded queues. All
//! UObject access is native sampling / servo (`sample.rs`, `servo.rs`, `neutralise.rs`; rules in
//! the `sample.rs` header) inside a Lua call;
//! every entry point is non-raising, panic-safe and game-thread checked.

#![allow(clippy::missing_safety_doc)]
#![allow(non_snake_case)]

pub mod api;
pub mod dev;
pub mod lua;
pub mod marshal;
pub mod native;
pub mod neutralise;
pub mod pose_hot;
pub mod proc;
pub mod records;
pub mod reflect;
pub mod sample;
pub mod servo;
pub mod worker_input;
pub mod native_host;
pub mod native_descriptor_binding;
pub mod native_presentation;
pub mod native_input_capture;
pub mod native_source_scope;

pub use api::{hsmp_native_open, luaopen_hsmp_lua};
