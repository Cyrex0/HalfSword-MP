# beta5 review: quality (lints, dead code, tests, flakes, CI)

Branch `cloud/quality`, based on `acbc700` (0.1.0-beta.4). All numbers are from the Linux review
machine (4 cores, shared with five other agents) unless stated otherwise.

## 1. DoD-10 under netsim `far`: the menu raced its own inference against the server's answer

**Root cause.** `HSMPMenu/Scripts/commands.lua` resolves a command from two sources: the server's
`cmd_result` record (S2G event, the `ordered` channel) and an inference from the session snapshot
(`rel_latest` channel) and SERVER chat replies. `C.tick` (every 500 ms) took whichever it saw
first. The server sends the result and the snapshot back to back, but on different channels: when
the result packet is lost (2 % loss, mean burst 2 packets on `far`; 5 % on `bad`) the snapshot
arrives a retransmit earlier (initial RTO 300 ms, ~500 ms at 300 ms RTT, doubled on a second
loss), the menu shows `source="inferred"`, and DoD-10 fails with "the server answered, but the
menu resolved it by inferred first", although the server did answer. With a few commands per
instance per run that is a per-run failure probability of several percent on `far`, rising with
loss: a flake that only shows at high loss. The sidecar side is not the cause: it resends every
250 ms for 5 s (`session_client.rs`), far inside every menu timeout.

**Fix.** When the record transport is present (`HSMP_IPC.events`), an inferred outcome is held for
`C.INFER_GRACE_S = 2.0` s (two retransmits at 300 ms RTT) before it is shown; the server's record
wins if it arrives meanwhile. If the inference goes away during the grace, the grace restarts.
Without the record transport the inference is immediate as before. The per-kind timeouts and the
resend logic are unchanged (the grace ends before the shortest timeout, 6 s). Cost: when a result
packet really is lost, the player sees the result up to 2 s later instead of a guess.

**Test (fails without the fix).** New suite `tools/hsmp-tools/lua-tests/commands.lua`
(`hsmp-tools lua-test commands`): 12 checks. Without the change 4 fail, among them
"resolved by the server's answer" (`source="inferred"` after 1 s) and "still pending inside the
grace"; with it 12/12 pass. Covered: snapshot before the answer, answer never arrives (inferred
after the grace, before the timeout, one result only, a late answer adds none), inference that
disappears during the grace, a refusal read from the snapshot loses to the server's reason, no
record transport (immediate), no answer and no inference (one resend, then timeout).

`menu_ui.lua`'s emulated server never sends a `cmd_result` record (it answers through state
files), so its boot sets `INFER_GRACE_S = 0`; its assertions about inference stay as they were.

**Needs in-game verification:** `mp_test.ps1` scenarios with commands under `-Netsim far` and
`-Netsim bad` (map_change, the lobby flows): DoD-10 should stop failing with "resolved by inferred
first". Not reproducible here (no game).

## 2. Two Lua suites that failed on Linux (and hid 11 900 checks)

Both failed at baseline on Linux and therefore in G0's `lua_test` there.

- `menu_ui`: `isolated_scripts()` created its temp dir with `os.execute('mkdir "..\\.." >nul')`,
  which only works on Windows; on Linux the suite aborted in the cfg cases, so everything after
  them never ran (610 checks before, **12 511** after). The harness gets `T.mkdir(p)`
  (`create_dir_all`) and the suite uses it. On Linux the old call also left a file named `nul` in
  the working directory.
- `hsmpworld`: golden vector 2 (yaw 90) has quaternion z == w up to the C library's last bit;
  glibc's `sin`/`cos` make w larger, the Rust reference (f32) drops z. Both encodings decode to
  the same rotation, so the vector accepts either dropped index (`alt_flags`).

`hsmp-tools lua-test` on Linux: 9 462 passed / 18 failed (with menu_ui aborted) at baseline,
21 380 passed / 0 failed now.

## 3. Clippy

`cargo clippy --workspace --all-targets --locked -j 2` on Linux:

| | warning lines | unique warnings (file:line) |
|---|---|---|
| baseline (acbc700) | 179 | 142 |
| now | 22 | 19 |

The 19 left are in files other agents own and are actively changing, so they are left to them to
avoid conflicts (each is a one-line mechanical change):
`server/src/combat.rs` (5 clone_on_copy, single_match, doc indentation), `server/src/lagcomp.rs`
(5 doc indentation, question_mark, manual_div_ceil), `server/src/server/match_core.rs`
(needless_late_init, manual_div_ceil), `server/src/server/dispatch.rs` (redundant_locals),
`crates/hsmp-combat-sim/tests/combat_sim.rs` (2 cloned_ref_to_slice_refs).

What changed beyond the mechanical fixes:

- regexes compiled inside loops in the contract lint, `check_wg` and crash-triage are built once;
- `poseplay::sample_lead`'s doc comment had drifted above `blend_step`; moved back;
- NaN guards in hsmp-pose spelled out (`x.is_nan() || x <= eps`), same behaviour;
- Windows-only items get `cfg`/`cfg_attr` instead of renames that would break the Windows build
  (`memwatch::FILETIME_TO_DOTNET_TICKS`, the vcvars binding in `g0.rs`, native `ProcEnt`, the
  `shm.rs` unmap `return`, the `log_session.rs` helpers);
- the sidecar handler tests hold their serial guard across awaits on purpose: an
  `#[allow(clippy::await_holding_lock)]` with the reason (the production code has no such hit).

**Allow list.** `collapsible_if`, `collapsible_else_if`, `new_without_default` and
`manual_is_multiple_of` leave `[workspace.lints.clippy]` (fixed: 2 + 0 + 1 + 25 sites; `Native`
gets a `Default`; `is_multiple_of` is stable since 1.87, rust-version is 1.88). Still allowed, with
their hit counts: `unnecessary_map_or` 94, `needless_range_loop` 33, `field_reassign_with_default`
23, `type_complexity` 17, `too_many_arguments` 11. These are mostly in netcode, combat and IPC
files; worth doing crate by crate after the beta5 merges.

**Caveat:** CI runs clippy on Windows, where `cfg(windows)` code (launcher, hsmp-native, the gate's
memwatch) is linted too; I could only lint the Linux configuration. Warnings there do not fail CI.

## 4. Dead code and `#[path]`

- `tools/hsmp-tools/src/bin/dirlint/luablock.rs` was mounted by `check_travel` and `check_wg` with
  `#[path]` + `#[allow(dead_code)]`; it is now `hsmp_tools::luablock` (library module).
- `hsmp-pose`: the module-wide `#![allow(dead_code)]` in `posecodec`, `posecodec_v2` and
  `poseplay` (left from when they were `#[path]`-mounted into the sidecar) are gone. They hid
  `posecodec_v2::put_vel` (dead: removed) and `poseplay::sample_frames` (test-only: `#[cfg(test)]`).
- `hsmp-modes`: the crate-wide `#![allow(dead_code)]` (for a `mod modes;` mount that does not
  exist) is gone; nothing in it is dead.

**Plan for the server's shared files (not done: it conflicts with the server, netcode and combat
agents' work in the same files).** Do it right after the beta5 merges, one crate per PR:

1. `crates/hsmp-proto`: `server/src/proto.rs` + `vitals.rs` (+ `query.rs`, `world.rs` wire parts if
   they are std/serde only). Users: hsmp-server, the sidecar, master, query, loadtest, combat-sim,
   `server/tests/{record_fuzz,decode_fuzz}.rs`. Removes the `#[allow(dead_code)]`s in proto.rs
   (5), vitals.rs and query.rs and the nested `#[path = "vitals.rs"]`.
2. A `[lib]` target in the server package (`server/src/lib.rs`) for `events`, `proc_util`,
   `build_id`, `log_init`, `ipkey`: the four bins then `use hsmp_server::...` instead of
   re-compiling the files; removes six module-wide allows and the duplicate compiles.
3. `crates/hsmp-sim-core` (or a `lib` in hsmp-combat-sim proper): `lagcomp`, `combat`, `validate`.
   This is the one with real risk (the simulator must keep running the shipped code; the
   `build.rs` catalog lift stays), and it lets combat-sim have a test target.
4. Sidecar-only client files (`*_client.rs`, `world_rx.rs`) move under `server/src/sidecar/`.

Each step is mechanical but touches every importer; doing it now would conflict with five
concurrent branches.

## 5. Flaky timing tests

Ran every test binary of hsmp-server, hsmp-net, hsmp-pose, hsmpworld-sim, hsmp-nat,
hsmp-master-core and hsmp-tools (26 binaries) repeatedly, directly (no rebuild), while the other
agents were building (a loaded 4-core machine): 5 full rounds (130 binary runs), no failure. The
timing asserts I looked at (`sidecar/net.rs` drain < 500 ms, `query_tool.rs` LAN discovery < 1.5 s,
`master.rs` header timeout, `parent.rs`) have generous margins and did not trip. I found no
reproducible Rust flake, so none was changed. The gate fixtures and rules
(`scripts/fixtures`, `rules.rs`) are deterministic (recorded events, no clocks), so the only
timing-sensitive DoD-10 behaviour is the one in §1.

## 6. CI (`.github/workflows/ci.yml`)

Measured from the last three green `main` runs (GitHub API, step timestamps):

| job | before (wall) | critical steps |
|---|---|---|
| Windows (G0 + e2e) | 15.6 / 16.3 / 17.3 min | build tools+server 4.5-4.8, G0 6.3-6.7, e2e 3.8-4.6 |
| Clippy (Windows) | 1.6 / 1.9 / 3.1 min | clippy 0.9-2.0 (warm cache) |
| Linux (server) | 2.6 / 3.5 / 3.7 min | |
| Docker | 1.9 / 2.0 / 2.0 min | |

The Windows job was the whole wall time, and its G0 ran clippy again (G0's `clippy` check is the
clippy job's command), so clippy ran twice per push. Now:

- **Windows (G0)**: release `hsmp-tools` only, then `hsmp-gate g0 --no-stamp --skip clippy`.
- **Windows (e2e)**: release `hsmp-tools` + `hsmp-server`, then `scripts/e2e-test.sh`.
- **Clippy**: unchanged; now the only clippy run.
- Linux and Docker: unchanged.

Every check runs exactly once. Expected wall time: max(G0 job ≈ 1 setup + ~3.5 tools build + ~5
G0 without clippy ≈ 9.5 min, e2e job ≈ 1 + 4.7 + 4.2 ≈ 10 min, clippy ≈ 2-3 min) ≈ **10 min
instead of 16-17** (about 6-7 min, ~40 %, saved per push), at the cost of one more Windows runner
(the clippy runner existed already). Smaller wins: `save-if: main` on every rust-cache (pull
requests restore main's cache and stop evicting it; saves the ~30 s post-job upload on PRs) and
`CARGO_PROFILE_DEV_DEBUG=line-tables-only` (smaller PDBs: faster MSVC links in the debug test
build and a smaller cache; backtraces keep file:line). Not done: cargo-nextest (G0's `cargo`
check parses `cargo test` output, and the test time is dominated by a few long binaries, not by
running binaries in sequence), sccache (rust-cache already covers dependencies; the workspace
crates rebuild either way), Docker layer caching (the cargo build layer depends on the server
sources, so a layer cache would not hit without restructuring the Dockerfile into a deps-only
stage; the job is 2 min and off the critical path).

`hsmp-gate g0 --skip <checks>` is new for this: the named checks report SKIP (so no G0 stamp is
written), an unknown name is an error (exit 2). Tests: `g0::tests::skip_reports_the_named_checks_only`,
`g0::tests::quick_skips_cargo_and_clippy`.

**Needs a real CI run** to confirm the times (the new jobs start cold until main has saved their
caches once).

## 7. Test results

On the tree merged with `dev` (merge commit on this branch), Linux, `CARGO_INCREMENTAL=0`:

- `cargo test --workspace --locked --no-fail-fast -j 2`: 1167 passed, 12 failed. The 12 are exactly
  the known Linux-only failures from the baseline (5 launcher `firewall::tests`, 2 launcher
  `install::tests` case-only renames, hsmp-native `lua_api_end_to_end`,
  `records_dev_proc_conformance_and_bench`, `s2g_only_from_the_live_sidecar`,
  `native_sampling_g1`, `claims_and_held_items_are_typed_records`: Windows shared memory).
- `cargo clippy --workspace --all-targets --locked -j 2`: exit 0, 22 warning lines / 19 unique
  (baseline 179 / 142), all in the files listed in §3.
- `hsmp-gate g0 --quick --no-stamp`: PASS (bp_names and unsafe U1/U2 skip: no game dump);
  `lua_test` 21 447 checks passed, 0 failed.
- `hsmp-tools lua-test commands`: 12/12 (4 fail on the old commands.lua).
- `cargo test -p hsmp-tools --bin hsmp-gate g0::`: the two new tests pass.

## 8. Risks

- §1 changes what the player sees: a command whose result packet is lost shows "waiting" up to
  2 s longer. Behaviour with the record transport absent (the menu harness, legacy paths) is
  unchanged.
- `--skip clippy` in CI relies on the clippy job for the same check; if someone removes that job,
  clippy silently stops running in CI.
- Lint-only edits touch files other agents own (server, hsmp-ipc, hsmp-native, hsmp-net tests):
  one or two lines each, no behaviour change; conflicts, if any, are trivial.
