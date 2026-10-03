# Contributing to HalfSword-MP

Thanks for helping. HalfSword-MP (HSMP) is an unofficial multiplayer mod for Half Sword: a Rust
server, sidecar, launcher and tools, plus a native UE4SS module and UE4SS Lua mods that run inside
the game. This page covers how to build it, how to test a change, and the rules a change has to
follow.

By contributing you agree that your contribution is licensed under **MIT OR Apache-2.0**, the
project's licence, without additional terms.

## Prerequisites

| Tool | Why |
|---|---|
| [rustup](https://rustup.rs/) | The toolchain is pinned in `rust-toolchain.toml` (Rust 1.98.1, with clippy and rustfmt) |
| A C compiler | The Lua test harnesses build a vendored Lua 5.4 (`mlua`). Windows: Visual Studio Build Tools, "Desktop development with C++". Linux: `build-essential` |
| [Git for Windows](https://git-scm.com/) | Git Bash runs `scripts/e2e-test.sh` (it is Windows/MSYS-only today) |
| PowerShell 5.1+ | The deploy and in-game test scripts |
| Half Sword + UE4SS (optional) | Only for in-game testing and for the two G0 checks that read the game's UE4SS object dump |

There is **no Python** anywhere in this project, and none may be added: tooling and tests are Rust
(or C++), mod code is Lua, host scripts are PowerShell or POSIX shell.

## Build and test

```sh
cargo build --workspace                        # debug build of everything
cargo test --workspace --locked                # all Rust tests and the Lua harnesses
cargo clippy --workspace --all-targets --locked
cargo build --release --locked -p hsmp-tools
target/release/hsmp-gate g0                    # the G0 gate (see below)
```

`cargo` puts every binary in `target/` (or `$CARGO_TARGET_DIR`). If you work in several git worktrees,
give each its own `CARGO_TARGET_DIR`.

### The G0 gate

`hsmp-gate g0` is the gate every push has to pass. It runs:

- `bp_names`: Blueprint function names with spaces used correctly from Lua (needs the game dump)
- `lua_check` and `lua_test`: every mod file compiles under Lua 5.4, and every offline Lua suite passes
- `travel`: only the Director (`mods/HSMPMatch/Scripts/director.lua`) changes levels
- `wg`: mods register their cached UObjects with the world guard
- `unsafe`: UE4SS API uses that are known to crash the game (rules U1/U2 need the game dump)
- `image_kill`, `instant_sub`: no kill-by-image-name, no `Instant::now() - Duration`
- `events`: the event contract between emitters and the gate
- `gate_selftest`: the in-game gate's rules against recorded runs in `scripts/fixtures`
- `ipc_schema`: the generated IPC header and Lua schema match `crates/hsmp-ipc` (regenerate with
  `hsmp-tools gen-ipc`), and the header compiles as C11 and C++17 (when MSVC is installed)
- `state_files`: no new state-dir files outside the allow-list (the game and the sidecar talk over
  shared memory)
- `cargo`: `cargo test --workspace --locked`
- `clippy`: `cargo clippy --workspace --all-targets --locked`, the same command as the CI clippy job;
  a compile error or a deny-level lint fails, warnings do not (`--quick` skips it and `cargo`)

Without Half Sword (no `HSMP_GAME_DIR`, no `<repo>/game`), the checks that need the game's
`UE4SS_ObjectDump.txt` report **SKIP** and G0 still passes; `hsmp-gate g0 --strict` turns those skips into
failures. Maintainers run the full gate with the game before merging.

Enable the pre-push hook once per clone:

```sh
git config core.hooksPath .githooks
```

### End-to-end tests

`scripts/e2e-test.sh` runs the real server, sidecars and master over UDP and HTTP (no game needed):

```sh
cargo build --release --locked -p hsmp-server -p hsmp-tools
HSMP_TOOLS=$PWD/target/release/hsmp-tools.exe HSMP_BINS=$PWD/target/release bash scripts/e2e-test.sh
```

### In-game testing

`scripts/build-and-deploy.ps1` builds the server binaries and copies the mods into the game
(`-Dev` adds the developer mods, `-DryRun` shows what it would do). `scripts/mp_test.ps1` drives two
game instances through the in-game gate scenarios. See [docs/development/testing.md](docs/development/testing.md).
Never inject OS input (mouse or keyboard) from a test: drive the game through mod hooks, files or RCON.

## Rules for Lua mods

The mods run inside a shipping UE5 game through UE4SS, where many mistakes crash the game with an
access violation that `pcall` cannot catch. Read [docs/development/lua-mods.md](docs/development/lua-mods.md)
before changing a mod. The short version:

- Run mod logic on the game thread only (`LoopInGameThreadWithDelay`, `ExecuteInGameThreadWithDelay`);
  every mod carries a shim for this.
- Never read a SoftObject/SoftClass property or parameter (`:get()` crashes the game).
- Drop cached UObjects when the world changes, without touching them (the shared world guard).
- Half Sword's Blueprint functions have spaces in their names: `obj["Set Up Armor"](obj, ...)`.
- Only the Director changes levels.
- New shared code goes in `mods/shared/`; a new mod must be added to `mods/mods.release.txt`.

## Pull requests

1. Fork, create a branch from `main`, keep the change focused.
2. Run `cargo test --workspace --locked` and `hsmp-gate g0` (or let the pre-push hook do it).
3. Add or update tests: Rust unit tests, a Lua suite in `tools/hsmp-tools/lua-tests/`, or an e2e case.
4. Update the docs when behaviour, flags, ports or file formats change, and add a line to
   `CHANGELOG.md` under "Unreleased".
5. Open the pull request and fill in the template. CI runs the Windows gate (without the game), the
   Linux server build and the Docker image build.

Commit messages: a short summary line, then a body that says what changed and why.

## Where things are

See the project layout in [README.md](README.md#project-layout) and the documentation index in
[docs/README.md](docs/README.md).
