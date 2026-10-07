# HSMP developer tools (`tools/hsmp-tools`)

**There is no Python in this repo.** Tooling is Rust (or C++); Lua runs only
in the game and in the offline Lua tests. Every developer tool lives in one Rust package,
`tools/hsmp-tools`: a CLI binary `hsmp-tools <subcommand>`, the gate
`hsmp-gate`, the lints, and a library (`hsmp_tools::*`) that new tools build
on. Lua tests run under **mlua** (Lua 5.4.7, vendored, all standard
libraries incl. `debug`, like UE4SS).

## Build

`hsmp-lab` keeps a two-game session alive for recipe experiments, incremental
combat/pose analysis, bootstrap comparisons and reviews. See [Native combat lab](lab.md).

```powershell
cargo build --release -p hsmp-tools
# binaries: target/release/hsmp-tools.exe, hsmp-gate.exe, check_travel.exe, check_wg.exe,
#           check_unsafe.exe   (or $env:CARGO_TARGET_DIR\release)
# or one-shot:
cargo run --release -p hsmp-tools -- <subcommand> ...
cargo run --release -p hsmp-tools --bin check_wg -- --strict
```

Every subcommand finds the repo root itself (`HSMP_ROOT`, else the first
ancestor of the current dir / the exe that holds `mods/` and
`server/Cargo.toml`), so it can be run from anywhere inside a checkout or
worktree. The game install is `HSMP_GAME_DIR`, else `<repo>/game`, else the
main worktree's `game/` (`src/paths.rs`); there is no machine-specific
fallback.

## Subcommands

In the order `hsmp-tools --help` lists them (`src/main.rs`; implementations in `src/cmd/`):

| command | what |
|---|---|
| `hsmp-tools netsim ...` | UDP impairment proxy (latency, jitter, loss, duplication, spikes, profiles); `--self-test` measures it |
| `hsmp-tools natlab stun --bind <addr>` / `natlab catch --bind <addr>` | NAT traversal test stand-ins: a local STUN server (answers Binding requests with the source address) and a punch-probe catcher (prints `probes <n> from <addr>`). Used by `scripts/e2e-nat.sh` and `e2e-master-cf.sh` |
| `hsmp-tools check-bp-names [--dump F]` | CamelCase use of a space-named Blueprint function or property; reads `UE4SS_ObjectDump.txt` (+ SDK headers); exits 2 if no dump is found |
| `hsmp-tools lua-check [paths]` | compile every `*.lua` under `mods/` (or the given paths) under Lua 5.4 |
| `hsmp-tools lua-test [suite...] [--verbose] [--list] [-- args]` | the Lua suites in `lua-tests/` (below) |
| `hsmp-tools gen-ipc [--check]` | the generated IPC files (`crates/hsmp-native/cpp/gen/hsmp_ipc.h`, `mods/shared/hsmp_ipc_schema.lua`) from `crates/hsmp-ipc`; `--check` diffs them (G0 `ipc_schema`) |
| `hsmp-tools ipc-dump --pid P \| --name N [--json] [--full] [--records N] [--follow] [--get SLOT [--peer ID \| --slot N]]` | read-only live view of a game's shared-memory segment ([ipc-shared-memory.md](ipc-shared-memory.md) §11): header, epochs, caps, heartbeats, counters, peer directory and plays, session, bus keys, every record slot (rendered from the schema), the last ring records; `--follow` tails the G2S / S2G / DevCtl rings as JSON lines; `--get records` or `--get <slot name>` prints one |
| `hsmp-tools ipc-ctl --pid P \| --name N [--id N] autotest CMD [ARG] \| tune KEY VALUE \| tdiag on\|off` | one `dev_cmd` record into the game's DevCtl ring; exit 0 queued, 3 ring full (the game is not polling), 2 error |
| `hsmp-tools ipc-game [--caps HEX\|all] [--view F] [--tap F] [--name-file F] [--parent-pid P] [--stop-file F] [--no-sidecar] [--bridge DIR] [--synth ...] -- SIDECAR ARGS...` | a fake game for tests: creates the segment for its own PID, starts the sidecar with `--parent-pid <it> --ipc shm:<name>`, pumps the game side every millisecond and logs what the game sees (JSON lines). `--bridge DIR` mirrors what the game receives into files in DIR and forwards the files a test writes there, for the file-shaped assertions of `e2e-test.sh`; the sidecar never touches them. `--synth` plays a live match into the game slots (used by `net-bench`) |
| `hsmp-tools ipc-put --name N NAME JSON\|@FILE` | write one game-written record slot, G2S record or `dev_cmd` from the record's Lua-table shape (Rust field names, `rows` for variable records; validated); prints the `req_id` for ring records |
| `hsmp-tools ipc-stress [--duration S] [--only 1,2,...]` | cross-process stress of the shared-memory primitives: six scenarios (torn reads, writer and reader kill / restart, stalled consumer, isolation, refusals); exit 0 only if all pass ([ipc-shared-memory.md](ipc-shared-memory.md) §12) |
| `hsmp-tools gen-map-data [--check]` | `server/data/maps/*.json` + `mods/shared/hsmp_arenas.lua` from `docs/arena_static` |
| `hsmp-tools gvas-diff A B [--grep X]` | heuristic diff of scalar properties between two GVAS `.sav` files |
| `hsmp-tools dump-diff A B` | diff two HSMP property dumps (`== Section:` / `  key = value`) |
| `hsmp-tools net-bench [--clients N] [--secs S] [--warmup W] [--hz HZ] [--vitals-hz HZ] [--tick-hz HZ] [--bins DIR] [--json F] [--keep]` | offline benchmark of the network / IPC stack: real server + N fake games (`ipc-game --synth`: 60 Hz root / weapon / pose, 20 Hz vitals), each with a real sidecar, behind a counting UDP proxy; reports server / sidecar / game CPU %, per-client up / down bytes and pps, slot-write µs. Results in `bench/` |
| `hsmp-tools pose-probe ...` | end-to-end pose pipeline probe over shared memory: server + 2 sidecars (optional netsim `--profile`); the probe plays both games (one segment each), publishes the local pose and reads the peer's `peer_play` slot |
| `hsmp-tools rcon ADDR "LINE[@ms]"...` | send lines to the server's RCON port, print the last reply |
| `hsmp-tools kismet-pp` | Kismet bytecode pretty-printer for CUE4Parse JSON dumps |
| `hsmp-tools mapdump-summary` | build `docs/arena_static/_summary.md` from MapDump output |
| `hsmp-tools crash-triage [--dir D] [--since T] [--until T] [--all] [--out F] [--cdb EXE] [--sympath P] [--symsrv] [--no-symbolise] ...` | symbolise and classify new minidumps in `%LOCALAPPDATA%\HalfSwordUE5\Saved\Crashes`; JSON report; exit 1 on new crashes (gate rule DoD-2). Known signatures (crates/hsmp-diag/src/triage.rs, shared with the launcher's bug reports) include the RVP round-reset crash ([crash-rr.md](crash-rr.md)) |
| `hsmp-tools report-fixture --out F [--magic M] [--pad-mb N]` | a minimal bug-report zip in the launcher's upload format (or a broken one) for the Worker's `/v1/reports` tests (CF10-CF13); see [bug-reports.md](bug-reports.md) |

Standalone binaries in `src/bin/` (same package):

| tool | run | what |
|---|---|---|
| `hsmp-gate` | `hsmp-gate g0 [--quick] [--strict] [--skip clippy,...]`, `hsmp-gate scenarios`, `assert`, `selftest`, ... | the gate harness ([testing.md](testing.md)) |
| `check_travel` | `cargo run --release -p hsmp-tools --bin check_travel` | only the Director changes level |
| `check_wg` | `... --bin check_wg [-- --strict] [--sites] [--release-set]` | world-guard lint W1-W3 |
| `check_unsafe` | `... --bin check_unsafe [-- --strict] [--dump F \| --no-dump] [--json F] [--list-soft] [--write-baseline]` | UE4SS API lint U1-U6 (soft hook params/props, SetLeaderPoseComponent, K2_DestroyActor, off-thread delays, unguarded hook captures, console outside the Director); known findings in `tools/hsmp-tools/check_unsafe.baseline` |

`src/luablock.rs` (in the hsmp-tools library) is the Lua block parser
that `check_travel` and `check_wg` share. What each lint checks, with
examples, is in [lua-mods.md](lua-mods.md).

Lua suites (`hsmp-tools lua-test --list`): `avatars`, `combat`,
`damage_parity`, `director`, `flow`, `framecost`, `hsmp_log`, `hsmp_session`, `hsmpworld`,
`hud_net`, `ipc`, `kit_status`, `loadout`, `menu_ui`, `pose`, `records`, `rvp`,
`sidecar_peers`, `spawn_place`, `sync`, `ui_scale`, `vitals`, `world_guard`,
`world_state`. `menu_ui -- dump` renders every menu screen as ASCII; `framecost` prints the
per-frame Lua garbage (and the stand-in driver's Lua CPU time) of the hot loops.

`hsmp-tools lua-test` with no suite runs them all; `--verbose` prints every
passing check; `--list` lists suites. Exit codes: 0 ok, 1 check/lint
failure, 2 tool error (bad args, missing input).

Four more Lua harnesses are separate workspace packages under `tests/`
(run by `cargo test --workspace`): `hsmp-hud-test` (HSMPHud),
`hsmp-interact-test` (HSMPInteract), `hsmp-saveguard-test`
(`shared/hsmp_saveguard.lua`) and `hsmpworld-tests` (HSMPWorld). Each has its
own mocked UE4SS in `tests/<crate>/tests/*.lua` or `ue4ss_mock.lua`.

## Building blocks for new tools

### Lua test suites: `hsmp_tools::luatest` + `lua-tests/*.lua`

A new Lua test is **one file**: `tools/hsmp-tools/lua-tests/<suite>.lua`
(no Rust needed). It is picked up by `hsmp-tools lua-test` automatically and
runs in a fresh Lua 5.4 state with a global `T`:

```lua
-- tools/hsmp-tools/lua-tests/mymodule.lua
local M = dofile(T.path("mods/shared/hsmp_rvp.lua"))   -- the module under test, from the tree
local clock, logs = 10.0, {}
local R = M.new({
    log = function(fmt, ...) logs[#logs + 1] = string.format(fmt, ...) end,
    clock = function() return clock end,
    ue = { default_limit = 10,                          -- a fake UE layer instead of UE4SS
           counts = function() return { paint = 0, detect = 0 } end,
           set_limit = function(v) return true, 10 end,
           purge = function() return 0 end },
})
T.check(R.hold("test") == true, "the first call holds the travel")
clock = clock + 0.3
T.check(R.hold("test") == false, "an empty queue lets it go after QUIET_S", T.repr(logs))
```

Suites that drive whole mods use the shared mocks below (`umg_mock.lua` for UE4SS and UMG, the
native-module mocks for the shared-memory API).

`T` (Rust side, `src/luatest.rs`): `check(cond, msg [, detail])` (counted;
truthiness: nil/false/0/"" fail), `root`, `path(rel)`,
`read/write/append/remove/exists`, `tmpdir(prefix)`, `glob(base, pat)`,
`json_decode(s)`, `re_match/re_search/re_findall` (Rust regex),
`isolated(file, ...)` (run another file in a NEW Lua state, e.g. one per
resolution; checks add to the same totals), `counts()`, `log(s)`.
Lua helpers (`src/luatest_prelude.lua`): `sorted, eq, repr, contains,
startswith, count, lines, keys, map, filter, any, all, set, len, list, truthy`.

Shared mocks in `lua-tests/lib/` (`require`-able): `umg_mock.lua` (UE4SS
objects + UMG widgets/slots, game-thread loop/delay scheduler on a fake
clock, an event log `M.logs`, click/hover driving, freed-object touch
tracking), `hsmp_native_mock.lua` and `hsmp_native_records.lua` (the
`HSMPNative` API and its record table rules, checked against the real module
by `records_conformance.lua`), `hsmp_native_pose.lua`, `dir_world.lua` (the
Director's world) and `ui_res.lua` (resolutions). Add more mocks there rather
than copying them.

From Rust: `luatest::Shared::new(root, verbose)`, `luatest::run_file(...)`,
`luatest::new_lua()`.

### Lints: `hsmp_tools::lint` + `hsmp_tools::lualex`

```rust
use hsmp_tools::{lint, paths};
let root = paths::repo_root()?;
let mut rep = lint::Report::new("check-wg");
for f in lint::LuaFile::load_all(&root, &paths::mod_script_files(&root))? {
    for (ln, code) in f.code_lines() {          // strings blanked, comments removed
        if code.contains("LoopAsync(") { rep.push(&f, ln, "use LoopInGameThreadWithDelay"); }
    }
}
std::process::exit(rep.finish("no off-thread loops"));
```

`lualex::strip/code_lines` handle `"..."`, `'...'`, long strings
`[==[ ]==]`, `--` and `--[[ ]]` comments while keeping line numbers.
`paths::{repo_root, game_dir, ue4ss_dir, glob, walk, mod_script_files, rel}`.

### Adding a tool

Either add a subcommand (`src/cmd/<tool>.rs` with `Args` + `run`, one line
each in `src/cmd/mod.rs` and `src/main.rs`), or a standalone binary `src/bin/<tool>.rs` that uses `hsmp_tools::*`
(built as `<target>/release/<tool>.exe`, where `<target>` is `$CARGO_TARGET_DIR` or
`<repo>/target`). Outside this package, depend on it
by path (`hsmp-tools = { path = "../tools/hsmp-tools" }`, adjusted to
your crate's depth) and add the crate to the root `Cargo.toml` `members`.

Thin `.ps1`/`.sh` wrappers only where something calls a tool by path.
