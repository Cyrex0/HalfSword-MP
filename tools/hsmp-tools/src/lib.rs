//! HSMP developer tooling.
//!
//! The binary is `hsmp-tools <subcommand>`; this library is the shared part
//! other tools build on (see docs/development/tools.md):
//!
//! * [`paths`]   - repo root / game dir / UE4SS dump discovery, file walking.
//! * [`lualex`]  - a Lua lexer that blanks strings and drops comments, line-preserving.
//! * [`luablock`] - line-based Lua helpers for the lints: comments, strings, indentation, blocks.
//! * [`lint`]    - lint framework: load mod Lua files, collect findings, print, exit code.
//! * [`luatest`] - mlua (Lua 5.4) test harness: the `T` API (checks + counting,
//!   fs, JSON, regex, temp dirs, isolated states) used by `lua-tests/*.lua`.
//! * [`ipcgame`] - HSMP-SHM JSON views, game-role writes and the game emulator (`GameHost`).
//! * [`pyfmt`]   - Python-compatible float/JSON formatting for byte-identical generators.
//! * [`synth`]   - synthetic HSMPSync / HSMPCombat slot streams (`ipc-game --synth`, `net-bench`).

pub mod ipcbridge;
pub mod ipcgame;
pub mod lint;
pub mod luablock;
pub mod lualex;
pub mod luatest;
pub mod paths;
pub mod pyfmt;
pub mod synth;
pub mod lab;
