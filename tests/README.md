# Offline Lua test harnesses

Rust test crates that load the shipped Lua mods into Lua 5.4 (`mlua`, vendored) against mocked
UE4SS and Unreal objects. They need no game.

| Crate | Tests |
|---|---|
| `hsmp-hud-test` | `mods/HSMPHud` |
| `hsmp-interact-test` | `mods/HSMPInteract` |
| `hsmp-saveguard-test` | `mods/shared/hsmp_saveguard.lua` |
| `hsmpworld-tests` | `mods/HSMPWorld` (dynamic items, round-reset restore, physics guards) |

Run them with `cargo test -p <crate>` or as part of `cargo test --workspace`. Most Lua suites live
in `tools/hsmp-tools/lua-tests/` and run with `hsmp-tools lua-test`; both are part of the G0 gate.
