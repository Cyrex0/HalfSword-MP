# HSMP testing: event log, gate, G0, deploy

The reference for the event vocabulary, the harness-to-game contract and the gates. The rule
IDs below (`DoD-n`, `POSE-1`, `STATE-1`, ...) are the names `hsmp-gate` prints in its reports.
There is no Python in this repo. Every tool is Rust in `tools/hsmp-tools` (see
[tools.md](tools.md)), and the gate is the `hsmp-gate` binary there.

```powershell
cargo build --release -p hsmp-tools
# -> target/release/hsmp-gate.exe, hsmp-tools.exe and the lints (or $env:CARGO_TARGET_DIR\release)
```

| Gate | When | What | Command |
|---|---|---|---|
| G0 | every push (pre-push hook) | bp names, Lua compile, Lua suites, travel lint, world guard, unsafe API, kill-by-name grep, `Instant` underflow grep, event contract, gate self-test, generated IPC files, state-dir allow-list, `cargo test --workspace --locked`, the CI clippy command (§5) | `hsmp-gate g0` (`--quick` skips cargo and clippy, `--strict` fails instead of skipping the game-dump checks) |
| G1 | before a merge to main | `scripts/e2e-test.sh` + G0 | |
| G2 | before a release | `scripts/mp_test.ps1` gate scenarios, green twice | see §4 |

### During development: check the domains you changed

```powershell
.\scripts\dev-test.ps1 -List
.\scripts\dev-test.ps1 -Domain combat
.\scripts\dev-test.ps1 -Domain modes,mods -Plan    # inspect the selections, start nothing
.\scripts\dev-test.ps1 -Domain kit
```

The runner builds the Lua test tool, selects the related Lua suites and Rust test filters,
and writes command selections, raw output, timings and counts to
`test-results/dev-check-<id>/report.json`. It fails if a Rust selection runs zero passing
tests or a selected Lua suite runs no successful assertions. Available domains are
`combat`, `pose`, `modes`, `mods`, `kit`, `ui`, `ipc`, `launcher` and `world`; multiple
domains share one deduplicated selection. IPC includes the real sidecar round trip;
`-Domain ipc -Stress` also runs the cross-process stress scenarios.

This is an **offline development subset**. It does not audit every dependency, write a G0
stamp, prove native game behavior or pass a release gate. Run full G0 before pushing and
the appropriate in-game gates before release. For schema/shared-runtime changes or an
unclear change boundary, broaden the domains or run the full gate. A lab session runs quick
G0 once at startup; its individual experiments reuse the session without rerunning G0.

Counts use different units: G0 reports **Lua suites** and **Rust tests** separately.
Assertions inside a Lua scenario are recorded in its detailed report, not presented as
thousands of independent tests. `hsmp-tools lua-test combat --json <report.json>` writes
the suite names, assertion totals, failures and timings; G0 retains these under its
`lua_test.details` field. The full menu/HUD resolution matrix still runs for UI changes
and in full G0. Workspace Lua syntax checking stays in `lua-check`, so the world suite
no longer repeats the same syntax scan.

Native binding correctness and allocation checks remain in normal `cargo test`. Their
timing-only microbenchmarks are opt-in:

```powershell
$previousNativeBench = $env:HSMP_NATIVE_BENCH
try {
    $env:HSMP_NATIVE_BENCH = '1'
    cargo test --locked -p hsmp-native --test api --test records -- --nocapture
} finally {
    if ($null -eq $previousNativeBench) { Remove-Item Env:HSMP_NATIVE_BENCH -ErrorAction SilentlyContinue }
    else { $env:HSMP_NATIVE_BENCH = $previousNativeBench }
}
```

### Without the game you can run

Everything below works on a machine with no Half Sword install (no `HSMP_GAME_DIR`, no `game/`):

```powershell
cargo test --workspace                       # every Rust package and the mlua Lua harnesses under tests/
cargo build --release -p hsmp-tools -p hsmp-server
target\release\hsmp-gate.exe g0              # full G0; bp_names and the U1/U2 rules of `unsafe` report SKIP
```

```bash
# Git Bash: the headless end-to-end suite (real binaries over real UDP and HTTP).
#   HSMP_TOOLS = the hsmp-tools.exe file (never built by the script)
#   HSMP_BINS  = the release dir with hsmp-server.exe, hsmp-sidecar.exe, hsmp-master.exe, ...
HSMP_TOOLS=<target>/release/hsmp-tools.exe HSMP_BINS=<target>/release bash scripts/e2e-test.sh
```

`cargo test` skips the slow simulations and mutation fuzzers of systems that rarely change
(`#[ignore = "slow: ..."]`: the combat sim, the world-sync and netfeel sims, the IPC primitive
property tests, and the wire, IPC, NAT, master and launcher-manifest fuzzers). Run them with
`cargo test --workspace -- --ignored` after changing one of those systems or before a release.

Both default to `$CARGO_TARGET_DIR/release`, else `<repo>/target/release`. Every sidecar in the
suite runs behind a fake game (`hsmp-tools ipc-game`); S1-S12 (`scripts/e2e-shm.sh`) check the
shared-memory contracts directly. The suite allocates its ports per run and uses a scratch dir
per process (`HSMP_SCRATCH` overrides), so parallel runs do not collide. Its sidecars skip the
career save guard, so the suite never touches `%LOCALAPPDATA%\HalfSwordUE5\Saved\SaveGames`.

Shared memory is the only game↔sidecar IPC ([ipc-shared-memory.md](ipc-shared-memory.md)).
`mp_test.ps1` gives every instance `HSMP_IPC=shm`, the sidecar taps to
`<run>\inst<N>\ipc_tap.jsonl`, `ipc-dump --json` is saved before quit, and the final state-dir
listing goes to `<run>\inst<N>\state_dir_listing.txt` (rule STATE-1). `hsmp-gate ab-diff --a
<run> --b <run> [--json F]` compares frame cost, pose sender cost, pose_quality, hitches and
ipc_stats of two runs. `build-and-deploy.ps1` builds and ships HSMPNative and fails without it
(`-SkipNative` deploys without multiplayer). `mp_test.ps1 -FakeGame` runs the harness with
`hsmp-gate fake-game` as the shared-memory game.

`<target>` is `$CARGO_TARGET_DIR` or `<repo>/target`. Without the game, G0 prints
`skipped: no game dump (set HSMP_GAME_DIR)` for `bp_names` and for the U1/U2 rules of `unsafe`
(the other `unsafe` rules still run), still exits 0, and writes **no** G0 stamp, so a deploy or a
gate run cannot be certified from such a machine (§5). `hsmp-gate g0 --strict` turns those skips
into failures. The game dir is `HSMP_GAME_DIR`, else `<repo>/game`, else the main worktree's
`game/` (`tools/hsmp-tools/src/paths.rs`).

---

## 1. `shared/hsmp_log.lua`: structured events

`build-and-deploy.ps1` copies `mods/shared/*.lua` into **every** deployed mod's
`Scripts/`. Do not keep private copies (deploy warns and overwrites them).

### Loading it in a mod

```lua
local function load_shared(name)              -- require first, then next to main.lua
    local ok, m = pcall(require, name)
    if ok and type(m) == "table" then return m end
    local dir = ((debug.getinfo(1, "S").source or ""):gsub("^@", "")):match("^(.*)[/\\]") or "."
    local ok2, m2 = pcall(dofile, dir .. "/" .. name .. ".lua")
    return ok2 and m2 or nil
end
local HL = load_shared("hsmp_log")
HL.init("HSMPMatch")                  -- or HL.init{mod="HSMPMatch", state_dir=STATE_DIR}
HL.world_ready(arena, world_key, { round = r, match_id = id })
HL.event("travel", { from = a, to = b, by = "director" })   -- generic form
```

### Format and guarantees

* One JSON object per line in `<state_dir>/hsmp_events.jsonl`. `state_dir` is `HSMP_STATE_DIR`
  (default `hsmp_state`). Envelope: `{"v":1,"ev":..,"inst":..,"mod":..,"seq":..,"t_ms":..,"wall_ms":..}`.
  `inst` = `HSMP_INST` (default `"0"`), `seq` is per mod, `t_ms` = process ms (`os.clock`),
  `wall_ms` = epoch ms (anchored on `os.time`, converges to within the event spacing).
* The JSONL file is the only default sink. With `HSMP_LOG_ECHO=1` (also `true`/`yes`/`on`) or
  `init{echo=true}` each line is also echoed to UE4SS.log as `[hsmp_ev] {json}` (the harness sets it).
  Two instances share one UE4SS.log, so the `inst` tag is what attributes a line. The gate uses
  UE4SS.log lines for an instance only if that instance's JSONL file is missing. The echo used to be on
  by default and filled a normal player's UE4SS.log with hundreds of `[hsmp_ev]` lines.
* Each event opens the file in append mode, writes one complete line and closes it. Several mods
  can share the file, and a crash loses nothing. Cost is about 50 µs per event. **Never emit per frame.**
* Rotation: above 8 MB the file is renamed to `hsmp_events.1.jsonl` (keep 4) and the new file
  starts with `_rotated`. The gate reads all of them, in order.
* **Game thread only** (hooks, `LoopInGameThreadWithDelay` bodies, keybinds marshalled with
  `ExecuteInGameThread`). Never call it from `NotifyOnNewObject` without hopping to the game thread.
* Unknown names become `_bad_event`; a missing required field is written with
  `"_missing":[...]` (the gate reports it). Experiments use an `x_` prefix. New names are
  additive.

### Vocabulary

Wrappers take the required fields positionally and an optional table of extra fields.

The field lists below are what the **real emitters write today** (read from their code), and the
binding list is `tools/hsmp-tools/src/bin/hsmp-gate/contract.rs` (`CONTRACT`): G0's `events` check
fails when an emitter writes an event or a judged field the contract does not know, when the gate reads
a field no real emitter writes (unless `contract.rs` lists it in `PENDING`), or when a fixture uses a
shape outside the contract. A rule that needs a field no emitter writes reports **incomplete**.

| Event | Fields the real emitter writes (gate-read in **bold**) | Emitter | DoD |
|---|---|---|---|
| `lobby_ready` | **`epoch`**, **`notice`** (message shown, e.g. after a server restart), `peer_id`, `role`, `arena` | HSMPMenu `main.lua` when the lobby screen is up and connected | 11, harness sync |
| `cmd_sent` | **`cmd`**, **`cmd_id`**, `arg`, `backend`, `via` | HSMPMenu `commands.lua`: once per command (first send) | 10 |
| `cmd_result` | **`cmd`**, **`cmd_id`**, **`ok`**, **`reason`**, **`source`** (`server` \| `timeout` \| `local` \| `inferred`), `state`, `ms`, `tries` | HSMPMenu `commands.lua`: exactly one per `cmd_id` | 10 |
| `travel` | **`to`**, **`reason`**, `from`, `by` (`director`, `menu_legacy`) | Director `director.lua` (the only travel owner) | 3, 4, 11 |
| `world_ready` | **`arena`** (world short name), `world_key`, `round`, `server_arena` | Director: the arena world is up | 3, 4 |
| `spawn_verified` | **`ok`**, **`vitals_ok`**, **`gi_ok`**, **`dist_cm`** (horizontal, pawn → server spawn order), **`dest_cm`** (horizontal, pawn → HSMPSync's placement destination: the order, or its clearance-spiral point), **`offset_cm`** (destination → order), **`tol_cm`** (the placer's acceptance radius), **`x`**/**`y`**/**`z`** (pawn, cm), **`snap_z`** (the destination Z HSMPSync placed on), **`load_error`**, `round`, `arena`, `world_key`, `spawn_id`, `steps`, `ms` | Director spawn pipeline at Ready | 7 |
| `kit_verified` | **`who`** (`"self"`, or `"peer:<id>"` per puppet from HSMPLoadout `main.lua`), **`armour_n`**, **`r_class`**, **`l_class`**, **`ok`**, `exp_armour_n`, `round`, `kit`, `tries`, `error`, `r_visible`, `l_visible` | HSMPLoadout `kit.lua` | 5 |
| `willie_census` | **`visible`**, **`expected`**, **`at`** (`"ready"` at the pipeline's census step; `"live"` on the first Live tick and then every 5 s), `extras`, `missing`, `round`, `detail` (names of the visible Willies other than the own pawn). Visible = not `bHidden`, `Mesh:IsVisible()`, and no `Persistent` tag (the pooled template `Willie_BP_C_0` at the origin has an invisible mesh) | Director census step / Live sampler | 6 |
| `x_save_guard` | **`active`**, `why`, `session`, `ok` | `shared/hsmp_saveguard.lua` on every state change | 8 |
| `save_redirected` | **`fn`**, **`slot`**, **`to_slot`** (`HSMP_<inst>_<slot>` or `"<blocked>"`), **`op`** (`read`/`write`/`delete`), **`ok`** (false = the rewrite failed and the call hit the career slot), `how` | `shared/hsmp_saveguard.lua` | 8 |
| `native_travel_rewritten` | **`from`**, **`to`**, **`soft`** (true: the Director re-issues the travel), `phase`, `n` | Director OpenLevel pre-hook / soft-travel post-hook | 3 |
| `hitch` | **`ms`**, **`travel`**, **`world_key`** | the stall probe (§1.1) | 2 |
| `frame_hb` | `n`, **`max_ms`** | the stall probe (§1.1) | 2 |
| `rvp_hold_timeout` | `why`, `waited_s`, `paint`, `detect` | `shared/hsmp_rvp.lua` ([crash-rr.md](crash-rr.md)) | — |
| `ready_report` | `round`, `arena`, `load_error` | Director at Ready | harness sync |
| `phase` | **`from`**, **`to`**, **`round`**, **`match_id`**, **`arena`**, **`frozen_arena`**, **`epoch`** | **server** (`hsmp-server --events`, `session.rs observe_phase`) | 1, 3, 4, 11 |
| `cmd_result` (server) | **`source`** (`command`/`rcon`/`legacy`), **`player`**, **`cmd_id`**, `seat`, `peer_id`, `cmd`, `ok`, `reason`, `code`, `config_rev` | **server** `command_locked`: once per applied command (a cached resend is `cmd_dup`) | 10 |
| S2G `cmd_result` (tap) | **`cmd_id`**, **`ok`**, `reason_code`, `code`, `reason_text`, `config_rev`, `cmd`, `wall_ms` | **sidecar**: one per fresh `cmd_result` record from the server (an `s2g` record in `inst<i>/ipc_tap.jsonl`; the gate reads its payload as `cmd_result`, src `sidecar`) | 10 |
| `pose_quality` | **`peer`**, **`arm_p95_uu`**, **`tip_p95_uu`**, **`latency_ms`**, **`jitter_ratio`**, **`foot_slide_p95`**, **`idle_rms`** (each `-1` = not applicable in that Live window; see POSE-1 below) | HSMPAvatars, every 5 s over Live frames only | POSE-1 |
| `netfeel` | **`peer`**, **`snaps_per_min`**, **`rigid_snaps`**, **`clock_resets`**, **`jump_max_uu`**, **`window_s`**, `frames`, `buffer_ms`, `jitter_ms` | HSMPAvatars, with each `pose_quality` sample | SMOOTH-1 |
| `x_pose_clock_reset` | `peer`, `threshold_ms` (50), `expect`, `projected_clk`, `delta_ms`, `clk_pt`, `clk_at`, `clk_rate`, `local_ms`, `source_pt`, `read_at`, `rate`, `lead`, `mode`, `age`, `delay`, `jitter`, `iv`, `quiet`, `cut`, `step`, `frame`, `source_seq`, `fresh`, `match_id`, `round`, `life`, `has_context` | HSMPAvatars, once at each clock discontinuity before correction; excludes initialisation and explicit pose cuts. Times are milliseconds, rates are clock multipliers; `quiet` is time since the latest fresh playback record. | diagnostic |
| `pawn_correction` | **`why`** (`round start`, `new round`, `fell`, `drift`, `director retry`, `launch_clamp`), **`live`**, **`dist_cm`** | HSMPSync spawn_place, HSMPAvatars (launch clamp): every move of the local pawn | SMOOTH-1 |
| `spawn_stretch` | **`who`** (`standin`/`pawn`), **`peer`**, **`max_uu`**, **`bone`**, **`why`**, `frames` | HSMPAvatars: 3 s after a stand-in starts / is re-posed, 8 s after our pawn appears | SPAWN-1 |
| `load_failed` | **`round`**, **`nick`**, **`error`**, `peer_id`, `seat`, `match_id` | **server** | 1 |
| `round_void` | **`round`**, `fighters`, `streak`, `match_id` | **server** | 1 |
| `seat_restored` | **`same_seat`**, **`same_wins`**, `player_key`, `seat`, `old_seat`, `wins`, `match_id`, `how` | **server** | 11 |
| `cmd_timeout` | **`cmd`**, **`cmd_id`**, **`tries`** | **sidecar** `--events` | 10 |
| `netsim_start` | **`profile`**, `upstream`, `pid`, impairment parameters | `hsmp-tools netsim --events` (`netsim<i>.jsonl`) | NETSIM |
| `netsim_stats` | **`in`**, **`out`**, **`clients`** (cumulative, every 5 s), `listen`, `lost`, `dup`, `blackout_dropped`, `queued` | `hsmp-tools netsim --events` | NETSIM |
| `x_save_call` | **`fn`**, **`slot`**, **`active`**, `err` | `shared/hsmp_saveguard.lua`: every write/delete call (reads only with `spy`) | 8 (attribution) |
| `career_guard` | **`action`** (`backup`/`clean`/`restored`/`recreated`/`quarantined`/`skipped_newer`/`error`), **`file`**, **`why`**, **`kind`** (`enter`/`leave`/`recover`), `backup`, `pid` | **sidecar** (`main.rs career_guard_event`): `--events` and `<state>/.career_guard.jsonl` (append-only, never swept; collected into `inst<i>/`, read as src `sidecar`) | 8 |
| `pawn_state` | **`at`**, **`protected`**, **`downed`**, **`consciousness`**, **`dist_cm`** (from the placement destination), **`weapon_r`**, **`weapon_l`**, **`reason`** (kit drops), `round`, `pawn`, `fallen`, `health`, `live`, `who` | HSMPSync `spawn_place.lua emit_state` (placed/ready/protect/live/protect_end), HSMPLoadout `kit.lua` (rearm/weapon_drop) | PAWN-1 |
| `world_consistency` | **`compared`**, **`mismatched`** (ids), **`hash_match`**, **`level`**, `mismatched_n`, `hash_equal`, `peer`, `epoch`, `world`, `f_seq` (the caller's `seq`); server: `other`, `kinds` | HSMPWorld `main.lua` (verdict every 5 s after Ready); **server** `world_glue.rs` (pairing) | WORLD-1 |
| `world_track` | **`nid`**, **`t`** (host clock ms: the performance counter, the same in every game process on the machine), **`x`**/**`y`**/**`z`**, **`rest`**, **`level`**, **`epoch`**, `qx`/`qy`/`qz`/`qw`, `mode` (`own`/`follow`/`free`), `owner` | HSMPWorld `main.lua` under the harness (`HSMP_AUTOTEST` or `HSMP_WORLD_TRACK`): every 100 ms per owned or recently moving body, a `rest` sample when it stops and 3 s later | WORLD-2 |
| `world_sync_quality` | **`hard_snaps`**, `max_off_cm`, `lost_races`, `takeovers`, `poked`, `follow_ticks`, `window_s`, `level`, `epoch` | HSMPWorld `main.lua`, every 5 s while it followed, lost a race or poked | WORLD-2 |
| `combat_quality` (`x_combat_quality` before the vocabulary listed it) | **`claims`**, **`accepted`**, **`pending`** (carried), **`rejected_by_reason`** (`{code: n}`), `confirmed`, `clashes`, `round`, `window_s` (per-window counts, every 5 s of combat; silent without combat) | HSMPCombat `main.lua emit_quality` | COMBAT-1 |

Known but not judged:
`x_autotest_cmd`, server `server_start`/`server_stop`/`link_stall`/`forfeit`/`cmd_dup`/`debug_kill`/`death_ignored`/`arena_picked`,
sidecar `phase`/`cmd_sent`/`session_epoch`/`notice`/`leave_request`/`link_resumed`/`parent_exit`/`sidecar_exit`,
the Director's `conn_state`, `travel_reason` and `resume`.

The server's phase label `MatchOver` is read as `PostMatch` everywhere in the gate. A `load_failed` or
`round_void` event anywhere in the run fails DoD-1, and any `cmd_timeout` fails DoD-10.

### 1.1 Stall probe contract (DoD-2, SOAK-HITCH), release profile

Emitter: `shared/hsmp_log.lua` `M.frame()`, driven from one game-thread loop per process (HSMPMatch's
250 ms loop is its only caller). It runs in the **release** profile (no `-Dev` deploy needed):

* `hitch{ms, world_key, travel}` for every game-thread frame longer than **2000 ms**. `ms` = the frame's
  duration, `world_key` = the world it happened in, `travel` = true only while a level load is in progress
  (between a travel and the next world being up; the first world load after boot counts as travel).
* `frame_hb{n, max_ms}` every **10 s**: `n` = heartbeat counter, `max_ms` = the longest frame of the last 10 s.
* Both names are in `M.EVENTS`.

Gate rule (`soak::stall_check`, per instance):

* **no `frame_hb` at all → incomplete** (the probe did not run; zero `hitch` events prove nothing);
* a `hitch` with `ms > 2000` and `travel=false` → **fail** (an emitter without the flag falls back to the
  [travel, world_ready] window);
* a `frame_hb` whose `max_ms > 2000` while its 10 s window overlaps no travel window, with no `hitch` for it → **fail**;
* no `frame_hb` for more than 35 s (outside travel windows) between the instance's first and last event → **fail**
  (a hang never emits its own `hitch`).

**POSE-1** (part of `p0_gate`, `p0_wifi` and the stopgap). Every `pose_quality`
sample during Live is judged against the most impaired netsim profile in the run (a peer's pose crosses both links):

| Profile | arm_p95_uu | tip_p95_uu | latency_ms | jitter_ratio | foot_slide_p95 | idle_rms |
|---|---|---|---|---|---|---|
| `typical` (also `none`/`lan`/`good`) | ≤ 5 | ≤ 8 | ≤ 50 + 40 = 90 | ≤ 1.2 | ≤ 5 | ≤ 0.5 |
| `wifi` | ≤ 10 | not bounded | ≤ 35 + 40 = 75 | ≤ 1.5 | ≤ 8 | not bounded |
| `bad` / `awful` | rule incomplete (no limits defined) | | | | | |

No tip or idle limit was given for wifi, so those two are unbounded there. A sample that lacks
`jitter_ratio`/`foot_slide_p95`/`idle_rms` makes the rule **incomplete** (an old emitter), never pass. Each instance needs samples for every remote peer in every round longer than 6 s. With no
samples at all the rule is incomplete.

POSE-1 does not pass on every run today: remote-pose fidelity on heavy arenas (many physics bodies)
can exceed these limits in some rounds.

**SMOOTH-1** (`netfeel`, `p0_gate`, `p0_wifi`). Per instance: no `pawn_correction` with
`live = true` other than `fell` (an honest pawn is never moved during Live); per remote peer, over
all Live `netfeel` samples: snaps per minute and rigid snaps per minute within the worst
profile's limits, and at most one playback-clock reset per minute.

| Profile | snaps/min | rigid snaps/min |
|---|---|---|
| `none`/`lan`/`good`/`typical` | ≤ 10 | ≤ 0.5 |
| `intl` | ≤ 20 | ≤ 0.5 |
| `far` | ≤ 30 | ≤ 0.5 |
| `wifi` | ≤ 40 | ≤ 1 |
| `bad` | ≤ 80 | ≤ 1 |
| `awful` | rule incomplete | |

A snap is a frame whose stand-in pelvis motion changes more than 3 uu beyond the change in its
targets' (second differences). The limits come from the offline model ([subsystems/replication.md](subsystems/replication.md)
"Rubber banding") with headroom; they are to be checked against in-game runs.

**SPAWN-1** (`netfeel`, `p0_gate`, `p0_wifi`). Every `spawn_stretch` sample (the largest joint
stretch against the reference skeleton after a spawn) is at most 10 uu. No samples: incomplete.

**What the emitter measures (HSMPAvatars).** A sample covers only Live frames with the owner alive
(the Director's `director` bus key in state `Live`), each Live window starts clean, and a window with less than 0.5 s of such frames is
not reported. `jitter_ratio` is judged on moving bodies only (target >= 20 uu/s): on a still stand-in it compared
sub-millimetre solver noise with nothing and read 2.2 at an arm error of 0.08 uu. A window with no moving body
reports `-1`; it does not repeat the previous window's ratio (that counted one rough window twice). `foot_slide_p95` is the planted
foot's displacement over >= 0.1 s of a planted run divided by its duration (a 60 Hz finite difference read
0.05-0.08 uu of noise per frame as 3-5 uu/s against a limit of 5). `idle_rms` is the RMS about the mean of the
stand-in's tracking error while the owner is idle (pelvis and hands < 10 uu/s) over 30-120 frames (the old test
needed < 2 uu/s on all three for 1 s, which a standing ragdoll never is, so it was never measured). The three are
always present: `-1` means not applicable in that window (nothing moved, no planted foot, the owner never idle);
the gate does not judge a `-1`, but a metric that is `-1` in every Live sample of the run is incomplete.
`p0_gate` holds 12 s of Live per round so these windows exist (it used to kill about 1 s after Live).

Library meta events: `_open` (on init; the harness waits for it when starting instances),
`_rotated`, `_bad_event`.

Offline test: `hsmp-tools lua-test hsmp_log` (`tools/hsmp-tools/lua-tests/hsmp_log.lua`).

---

## 2. `shared/hsmp_cfg.lua` + `hsmp.cfg` (install-relative configuration)

`hsmp.cfg` sits next to `HalfswordUE5-Win64-Shipping.exe` (`Binaries\Win64`). It is plain `key = value`,
with `#`/`;` comments and optional quotes. `build-and-deploy.ps1` writes it if it is missing
(`-WriteCfg` overwrites it, and a kept file without `master_url` gets the local one added). The
launcher writes it on install with the release's server lists. A deploy writes:

```
bin_dir    = hsmp                     # relative to Win64 (the copies deploy/launcher put in Win64\hsmp)
master_url = http://127.0.0.1:7778    # local master: dev and test runs never register publicly
```

The file is looked up in this order: `$HSMP_CFG`, `hsmp.cfg`, `ue4ss/hsmp.cfg`. Env overrides:
`HSMP_BIN_DIR`, `HSMP_MASTER_URL`, plus the legacy `HSMP_SERVER_EXE` / `HSMP_SIDECAR_EXE` / `HSMP_QUERY_EXE`.
Defaults (no file, e.g. a hand-unzipped install) are `bin_dir = hsmp` and
`master_url = https://master.halfswordmp.workers.dev` (the public list). `mp_test.ps1`, `spawn_test.ps1`
and `e2e-test.sh` set `HSMP_MASTER_URL` to their own local master. Any key works, so later phases can add keys.

```lua
local cfg = load_shared("hsmp_cfg")
cfg.bin_dir, cfg.master_url          -- plain fields (also cfg.get(key), cfg.load()[key])
cfg.server_exe(), cfg.sidecar_exe(), cfg.query_exe()   -- bin_dir/<name>.exe, forward slashes
cfg.state_dir(), cfg.inst(), cfg.dev()                 -- HSMP_STATE_DIR / HSMP_INST / HSMP_DEV=1
cfg.safe_url(u)                                        -- master_url is always validated (cmd-safe)
```

Offline test: same suite as §1.

---

## 3. Process tracking: process handles, never kill by image name

DoD-12 says no orphan `hsmp-*` process and no kill by image name anywhere in the tree. G0 `image_kill` flags any
kill by name in the repo root, `mods/`, `scripts/`, `server/src`, `tools/` (including `src/bin`), `crates/`,
`launcher/`, `tests/` and `.githooks`: `taskkill /IM`, `taskkill` with `IMAGENAME eq` but no `/PID`, `Stop-Process`/`spps`/`kill -Name`,
`Get-Process <name> | Stop-Process`, a `ProcessName -match/-like/-eq` filter that feeds a stop, and
`pkill`/`killall`/`kill $(pgrep …)`. Scripts stop only PIDs they recorded themselves and check the image name and
start time first, as `mp_test.ps1`, `launch_net_demo.ps1` and `spawn_test.ps1` do.

**Process handles, no pid files** ([ipc-shared-memory.md](ipc-shared-memory.md) §5.5). The game spawns the
sidecar, the listen server and its local master through the native module (`IPC.spawn`:
CreateProcessW, no window, no shell; a job object per child). The module keeps the process
handles, so `IPC.proc_alive(pid)` / `IPC.proc_kill(pid)` work only on processes THIS game
spawned, take the child's tree, and cannot hit a reused PID. CLOSE LOBBY / CANCEL / QUIT kill
exactly those (HSMPMenu `kill_role`); nothing is ever killed by name or through a file.
`IPC.current_pid()` gives the game's own PID (no PowerShell walk), and the shipped binaries
match the module (the shm ABI check is the capability check), so there are no `--help` probes.

* `--parent-pid <game pid>` on the sidecar, the listen server and the local master: exit on
  parent death. This is what makes DoD-12 pass when a game crashes or is killed.
* `hsmp-server --pid-file` / `hsmp-master --pid-file` remain for hosting (service managers);
  the sidecar has no `--pid-file`. The harness discovers game-spawned processes by command line
  (state dir / RCON password) and records them in its own pidfile.

  Ask the process to leave gracefully first (the autotest `quit` command, or RCON for the server). Use the
  kill only as the fallback.
* The harness (`mp_test.ps1`) keeps its own `<run>\mp_test.pids.json` with `{role,pid,name,start_ticks,launched_by}`
  (`harness` for what it started, `game` for sidecars / listen servers it found by command line) and registers the run
  in `%LOCALAPPDATA%\HSMP\runs\<runid>.json` (`scripts\lib\hsmp_runs.ps1`). It holds the machine-wide run
  lock (`runs\game.lock`) for its lifetime: a second `mp_test.ps1` / `spawn_test.ps1` / `launch_net_demo.ps1` refuses
  to start (exit 2) and touches nothing. Only a run whose harness is **dead** is reaped: its recorded PIDs (PID, image
  name **and** start time must match) are stopped and its own state dirs removed. Every run has its own state dirs
  (`hsmp_state_<runid>_<i>`) and free ports (server, RCON, master, one netsim port per instance), and a `try/finally`
  tears down its own processes whatever ends it. Offline proof: `scripts\tests\hsmp_runs_selftest.ps1`.

---

## 4. The gate: `scripts/mp_test.ps1` + `hsmp-gate`

The gate launches the real game on a machine with Half Sword installed. Run it on a machine you are not using:
it starts several game instances and drives them without OS input. The verdict is in `report.json`.

```powershell
.\scripts\build-and-deploy.ps1 -RequireG0      # release profile; runs the FULL G0 first when none is recorded for HEAD (clean tree), so the stamp ties the run to a G0-checked commit (DoD-13)
.\scripts\mp_test.ps1 -Instances 2 -Scenario p0_gate -Netsim typical -Arenas all -Rounds 10
.\scripts\mp_test.ps1 -Scenario map_change -Netsim bad       # DoD-10 under bad
.\scripts\mp_test.ps1 -Scenario p0_wifi                      # DoD-14
.\scripts\mp_test.ps1 -Scenario reconnect | host_leave | server_restart | start_refused
.\scripts\mp_test.ps1 -AssertOnly test-results\<run>         # re-evaluate a run
.\scripts\mp_test.ps1 -Scenario p0_gate -DryRun              # plan only (env, commands, all steps)
.\scripts\mp_test.ps1 -Scenario p0_gate -FakeGame -GamePath <scratch> -HoldScale 0.1   # harness self-test
.\scripts\mp_test.ps1 -KillPrevious                          # reap DEAD runs only (a live run is never touched)
```

Exit code = verdict: **0 pass, 1 fail, 2 incomplete** (evidence missing, a stopgap run, or a
scenario that cannot run yet). DoD exit requires `p0_gate`, `map_change` (also under `-Netsim bad`), `start_refused`,
`reconnect`, `host_leave`, `server_restart` and `p0_wifi` to be green twice in a row.

### What a run does

1. Takes the run lock or refuses (§3); reaps only dead runs. Always builds hsmp-gate / hsmp-tools (`cargo build --release
   --locked -p hsmp-tools`, a no-op when current; `CARGO_TARGET_DIR` honoured) so the run is judged by this tree's rules (G2), and
   re-hashes the deployed files against `hsmp_deploy.json` (`run.json` `deploy_check`, DoD-13).
2. Creates its own `Binaries\Win64\hsmp_state_<runid>_<i>` and seeds `.settings.json` (`nick=HSMP<i>`, the server address).
3. Baseline: sha256 of `%LOCALAPPDATA%\HalfSwordUE5\Saved\SaveGames\*.sav` (`save_hashes`) and size + mtime + sha256
   per file (`save_files`), plus a list of `Saved\Crashes`.
4. Starts `hsmp-master`, `hsmp-server` (dedicated topology) and one `hsmp-tools netsim` per client. The netsim
   gets a `--control-file` for blackouts, `--parent-pid` = the harness and `--events`. It also starts `hsmp-gate observe`.
5. Starts the N games one by one. Each start waits for that instance's `_open` event; in listen topology it also waits
   for the host's `lobby_ready`, so the host is admin before a joiner connects.
6. Runs the scenario steps. Waits are event predicates (`hsmp-gate wait`), never sleeps. The only
   sleeps are `hold` steps (an impairment or observation duration).
7. `quit_all`: records liveness, then quits each game the user's way: the autotest command `quit` (HSMPMenu
   `quit_desktop`: session teardown, then console quit; 30 s), then `CloseMainWindow` (15 s), then a kill by recorded PID
   (`game_force_killed`, a DoD-12 failure). `final.json` records `quit_path` per game (`menu_quit` / `wm_close` /
   `force_killed` / `dead_before_quit`) and `game_force_killed`. It then waits up to 15 s for every **game-launched**
   process (sidecars, a listen host's server: `launched_by=game` in the pidfile) to exit, stops the observer and memwatch
   (stop files, then waits for their PIDs) and only then scans for orphans: only harness-launched processes
   (`launched_by=harness`: server, master, netsim, observer, memwatch) are exempt. Finally it stops its own processes,
   collects the files and re-hashes the saves.
8. `hsmp-gate assert` writes `report.json` and `junit.xml`.

### Scenarios (data: `tools/hsmp-tools/src/bin/hsmp-gate/scenarios.json`)

`hsmp-gate scenarios` lists them. `hsmp-gate scenario p0_gate --arenas all --rounds 10` prints the expanded steps.
Step kinds: `mark`, `wait`, `expect_none`, `rcon`, `client_cmd`, `netsim`, `hold`, `kill`, `restart`, `quit_all`.
The `foreach_arena`/`foreach_round` blocks unroll. Placeholders are `{arena} {seat} {round} {best_of}`.

| Scenario | DoD | Topology | Drives |
|---|---|---|---|
| `p0_gate` | 1-8, 10, 12, 13, POSE-1, SMOOTH-1, SPAWN-1, PAWN-1, WORLD-1, COMBAT-1 | dedicated | per arena: `BESTOF 2R-1`, `MAP`, `START`, then R × (ready_report each, Live, `DEBUG KILL <alternating seat>`, RoundOver), `ABORT` |
| `p0_wifi` | 14 (= 1-7), 12, POSE-1, SMOOTH-1, SPAWN-1, PAWN-1, WORLD-1, COMBAT-1 | dedicated | p0_gate on Alley+Pit under `wifi` |
| `netfeel` | 12, SMOOTH-1, SPAWN-1, POSE-1 | dedicated | two fighters on Pit over `far` (`-Netsim intl`/`bad` for the others), moving, R rounds |
| `world_sync` | 1, 2, 3, 12, 13, WORLD-1, WORLD-2 | dedicated | per arena (Cellar, Alley) and round: Live, both players `world_poke` props in turn (3 throws), 8 s to settle, `DEBUG KILL`; run it with `-Netsim typical`, `intl` or `far` |
| `map_change` | 4, 10, 12 | dedicated | the host's autotest client picks Yard, Slums, Cellar, then starts; everyone loads Cellar |
| `start_refused` | 4, 12 | dedicated | last instance `HSMP_AUTOTEST_READY=0`; `START` must be refused; 20 s with no travel |
| `reconnect` | 11, 12 | dedicated | 8 s netsim blackout on the joiner mid-Live; Paused, then Live, same seat/wins |
| `host_leave` | 11, 12 | listen | the host's client `leave`; the joiner is in the menu within 5 s with reason "host…" |
| `server_restart` | 11, 12 | dedicated | kill + restart the server; every client shows `lobby_ready` with a new `epoch` |
| `p0_smoke_autotest` | 1-3, 5-8, 12 | listen | stopgap (below) |

### Stopgap mode (a server without the RCON debug verbs)

`-Mode auto` (the default) checks `hsmp-server --help`; `-Mode rcon` / `-Mode autotest` force one. Without `--debug-verbs`, `p0_gate`/`p0_wifi`
fall back to `p0_smoke_autotest`. That mode uses the legacy `HSMP_AUTOTEST=1` (host + START after
`HSMP_AUTOTEST_WAIT_MS`) and `HSMP_AUTOTEST=join` hooks in HSMPMenu. It observes one match for 60 s and quits.
It is **never green**: DoD-1 stays `incomplete`. The listen host's `HSMP_SERVER_EXE` points at a generated
`hsmp-server-wrap.cmd`, which adds `--rcon-bind/--rcon-password` (and `--events` when supported) and logs to `server.log`.
Scenarios that need RCON or the command channel exit 2 in this mode.

### Evidence and fallbacks

| Source | Preferred | Fallback while not implemented |
|---|---|---|
| server phases | `server.jsonl` (`hsmp-server --events <path>`) | `server.log` parsed (tracing text: "match: live", "round over", "countdown begins", "pausing for reconnect", "back to lobby", ...); one epoch per `server[.N].log` |
| client events | `<state_i>/hsmp_events*.jsonl` (copied to `<run>/inst<i>/`) | `[hsmp_ev]` lines in the shared UE4SS.log, attributed by `inst` |
| lobby_ready | the event | the observer sees the tap's session `status` = `connected`, only for an instance that has not emitted a real `lobby_ready` anywhere in the run (a sidecar reconnect after `server_restart` is not the lobby) |
| phase (live waits) | the server events | the observer's session `match` state (from the tap) (only when no server phase exists at all) |
| server command answers | S2G `cmd_result` records in the sidecar tap (`<run>/inst<i>/ipc_tap.jsonl`) | the menu's `cmd_result.source == "server"` |
| impairment | `netsim<i>.jsonl` `netsim_start{profile}` + `netsim_stats` traffic | none: NETSIM is incomplete |

A rule with no evidence at all is **incomplete**, never pass. That covers a stream that has not instrumented its
events yet, and every sub-check whose field is missing (a missing field is never a silent pass). If the evidence
exists but a round lacks it, the rule **fails**.

### What each rule judges (real fields; see §1 for the emitters)

| Rule | Pass needs | Incomplete when | Fails when |
|---|---|---|---|
| DoD-2 | no new crash dump (crash-triage), every process alive at the end, the stall probe (§1.1) clean per instance | final.json / crash_triage.json / `frame_hb` missing | new dump, dead process, hitch `travel=false` > 2 s, heartbeat gap or `max_ms` > 2 s outside travel |
| DoD-3 | `world_ready.arena` == the server's frozen arena per round; a `native_travel_rewritten` reaches its destination (soft: a `travel` within 10 s; in place: a `world_ready` in that arena within 30 s; menu: a travel / `lobby_ready` within 30 s) | no rounds / no world_ready anywhere | wrong world, menu world during the match, rewrite with no arrival |
| DoD-4 | `check_travel` (the G0 rule with its allow-list) clean; the first world after START == the pick; START refused → no travel | no START marks | |
| DoD-5 | own `kit_verified{who="self"}` per round with `ok=true`; puppets `who="peer:<id>"` | no kit_verified; no puppet evidence anywhere in the run (pending emitter) | `ok=false`, a round without own/puppet evidence |
| DoD-6 | census `visible == expected` at Ready, and on entering Live and every 5 s during Live | no census; no non-Ready sample anywhere | mismatch, missing round sample, too few Live samples |
| DoD-7 | `spawn_verified`: `ok`, `vitals_ok`, `gi_ok` true; `dest_cm` ≤ 100 (older emitters: `dist_cm` ≤ 100); `offset_cm` ≤ the outer spiral ring (225); `tol_cm` == 100 (HSMPSync `verify_tol_cm` = the Director's `place_tol_cm` = the gate's limit, so placer and gate cannot disagree); `z` ≥ `snap_z` − 50; ≥ 150 cm from every other instance's pawn (`x`/`y`) in the same round | any of those fields missing | a limit is violated |
| DoD-8 | career `.sav` hashes unchanged (HSMP_* ignored); no career `.sav` size/mtime change in `save_files` (baseline → final) unless attributed (below); every `save_redirected` `ok=true` with `to_slot` `HSMP_*` or `<blocked>`; `x_save_guard{active=true}` per instance; per instance a `career_guard` report with no `restored`/`recreated`/`quarantined`/`error` | hashes missing, **the baseline has no career `.sav`** (nothing proven), no `save_files` record (old harness), a save event without `ok`, the guard never reported active, no `career_guard` line for an instance | a career hash changed; a career file written (size/mtime changed, even with the same bytes) during the run; `ok=false` (the divert failed); the sidecar's career guard restored, recreated or quarantined a career file, or reported an error (layer 2 hid a layer-1 miss) |
| DoD-10 | every menu `cmd_sent` has exactly one menu `cmd_result` with `source="server"` AND exactly one server answer (its S2G `cmd_result` record in the sidecar tap): both are required | no cmd_sent; no source and no sidecar tap; `source="server"` but that instance has no sidecar tap | a menu timeout/local/inferred result, even when the server answer arrived later ("answer arrived N ms after the menu gave up"); `source="server"` with no answer record in the tap; 0 or 2+ menu results; 2+ server answers; the server applied one (player, cmd_id) twice; a sidecar `cmd_timeout` |
| DoD-12 | no orphan `hsmp-*` process of this run after quit_all (a game-launched server/sidecar alive 15 s after its game quit is one); no kill by image name in the tree; every game quit through the menu path (`quit_path` `menu_quit`) | no orphan scan; no `quit_path` (old harness); a game that only WM_CLOSE ended (the menu quit is not proven); no game quit through the menu at all | an orphan; a kill-by-name hit; a game the harness had to kill by PID (`game_force_killed` in final.json or harness.jsonl); a game already dead at quit_all (`dead_before_quit`) that no scenario step ended on purpose |
| CRASH | (every scenario that does not list DoD-2: map_change, start_refused, reconnect, host_leave, server_restart) no new crash dir, no new dump in crash-triage over the run window, every tracked game/server/master/netsim alive at quit_all unless a step ended it on purpose (harness `expected_exit{role}` from a step's `expect_exit`, or `proc_killed`) | final.json / crash_triage.json / `alive_at_end` missing | a new crash dir or dump; a process dead before teardown that no step ended |
| DoD-13 | `g0.json` has every G0 check and all pass; clean tree; its commit == the deploy stamp's commit; the quick run's skipped `cargo` is covered by the stamp's full G0 (`g0_ok`, same commit, clean); the stamp's profile is `release`, or `dev` = the release mod set plus only the template's `dev` entries; the stamp hashes every deployed file (`hashes`) and `deploy_check` re-hashed all of them identical at run start; the binaries the run executed (`bins_used`) are the stamped ones; `-SkipBuild` only with binaries proven valid for the commit (`bins_match_commit`); `report.json` `gate.commit` (the judging hsmp-gate's build) == the deployed commit, built from a clean tools tree | g0.json / deploy stamp missing, dirty tree, commit mismatch, no full G0 for the deployed commit; a stamp without `hashes` (an old deploy: redeploy); no `deploy_check`; another profile, or a dev deploy that turned a shipped mod off or a non-shipped mod on; `-SkipBuild` with unproven binaries; the gate built from another commit or from a dirty tools tree | a G0 check failed; a deployed file changed or went missing after the deploy; the run executed binaries other than the stamped ones |
| PAWN-1 | per round and instance, every `pawn_state{protected=true, at ∈ placed/ready/protect/live}`: `downed=false`, `consciousness ≥ 95`, `dist_cm ≤ 100` (DoD-7's limit), `weapon_r`/`weapon_l` not fists/None where the instance's `kit_verified{who=self, ok=true}` has a weapon; no `pawn_state{at=rearm}` after `at=ready` ([spawns.md](subsystems/spawns.md) §1.7) | no `pawn_state` in the run; a field missing; the kit's hands unknown | a limit is violated (the first offending event is named); a round without a protected `pawn_state` |
| WORLD-1 | per round whose Live lasted ≥ 10 s, per instance, during Live: a `world_consistency` with `compared ≥ 1`; no id in `mismatched` of two consecutive verdicts (same `level`); the round's last verdict `hash_match=true` ([world-replication.md](subsystems/world-replication.md)) | no `world_consistency`; no verdict with `compared ≥ 1` in a judged round; **no round with Live ≥ 10 s** (the real p0_gate kills at once: Live ≈ 1 s) | a repeated mismatch; the last verdict mismatched |
| WORLD-2 | per round whose Live lasted ≥ 10 s: every two instances' `world_track` of the same body (same `nid`, `level`, `epoch`) paired at the same host-clock time `t` (the other track linear between samples ≤ 250 ms apart): the moving samples' p95 distance ≤ the profile's limit (none/lan/good 80 cm, typical/wifi 130, intl 170, bad and far 260: the simulator's results with margin, [world-replication.md](subsystems/world-replication.md) "Measuring sync"); the last `rest` samples of each body both screens tracked within 5 cm (WORLD-1's pose tolerance); no `world_sync_quality` hard snap during Live | no `world_track` at all; a judged round with no paired sample (nothing moved on two screens); a profile without a limit (awful); no round with Live ≥ 10 s | p95 above the limit; a body resting > 5 cm apart; a hard snap |
| COMBAT-1 | per round whose Live lasted ≥ 10 s, per instance: ≥ 1 `combat_quality` during Live; `pending` not growing (last ≤ first + 5); `accepted / (claims − parried)` ≥ the documented honest-acceptance floor ([combat.md](subsystems/combat.md): 99 % none/lan, 97 % good/typical, 93 % wifi; most impaired profile in the run) when ≥ 20 claims were decided; the same ratio over every Live sample of the run per instance | no `combat_quality` (the emitter is silent without combat: the autotest players do not fight); no sample in a judged round; **no round with Live ≥ 10 s**; bad/awful (no floor) | `pending` grew; the acceptance ratio is below the floor |
| STATE-1 | (every shared-memory run: run.json `ipc = "shm"`) every name in `inst<i>/state_dir_listing.txt` is on the state-dir allow-list the gate was built with (`state_files.allow`: names, prefixes, `{}`/`<id>` patterns and `[runtime]` globs) | an instance has no `state_dir_listing.txt` | a file outside the list is left in a state dir (an IPC file came back) |
| NETSIM | each impaired instance ran at least the scenario's profile, its proxy logged `netsim_start` with that profile (the listen host has no proxy by design), and its `netsim_stats{in,out,clients}` show game traffic: `in` and `out` growing over the run with `clients ≥ 1`, and growing across every round (last sample at/before Live → first sample at/after the round's end); every `netsim_stats` window overlapping Live has the proxy's own scheduling error `late_p99_ms` ≤ 2 ms, and the measured loss (`lost`/`in`, ≥ 5000 packets) is at least half the profile's `loss` | a weaker `-Netsim` override, no `netsim_start`, no `netsim_stats`; no `late_*` fields (an old netsim); a Live window with `late_p99_ms` > 2 ms (the box starved the proxy: the path was not the profile's); measured loss below half the profile's | the proxy ran a different profile than run.json says; a proxy with zero traffic (the client bypassed the impairment); a round with no traffic through the proxy |

DoD-8 attribution: a career file's mtime change is a note, not a failure, when an instance logged the native write
itself while its own guard was off: `x_save_call{fn ∈ SaveGameToSlot/AsyncSaveGameToSlot/DeleteGameInSlot,
slot=<file without .sav>, active=false}` within ±3 s of the new mtime and the call came before that instance's
**first** `x_save_guard{active=true}` (vanilla pre-session behaviour: the native main menu saves `Settings` on load).
A write while the guard was off again later (session end, a mid-session sidecar drop) is not exempt, and
every guard-off interval after the first arm is listed in the DoD-8 message. The report says "pre-session native
write by inst N". A hash change still fails.

### Run dir (`test-results/<yyyyMMdd-HHmmss>-<scenario>/`)

`run.json`, `plan.txt`, `harness.jsonl` (steps, marks, RCON replies, process starts, `step_failed`),
`observed.jsonl`, `server.log`/`server.jsonl`, `netsim<i>.jsonl/.log`, `inst<i>/` (events + state
files: logs and config only, `ipc_tap.jsonl`, `ipc_dump.json`, `state_dir_listing.txt`, `.career_guard.jsonl`), `UE4SS.log`, `baseline.json`, `final.json` (save hashes,
`save_files`, new crash dirs, `alive_at_end`, orphans, `quit_path`, `game_force_killed`),
`g0.json`, `pids.json`, `report.json`, `junit.xml`.

`report.json` contains `verdict`, `rules` (`DoD-n` → pass/fail/incomplete), `rounds` (from the server:
arena, round, live/end ms), `checks` (one line per rule × round × instance) and `event_counts`. DoD-8 ignores
`HSMP_*.sav` (the save guard creates those on purpose).

### What the harness relies on

| Interface | Contract |
|---|---|
| `hsmp-server --events <path>` | JSONL `{ev, wall_ms, ...}`: `phase{from,to,match_id,round,frozen_arena}` on every transition, `cmd_result{player,cmd,cmd_id,ok,reason}`, `seat_restored{player_key,seat,same_seat,same_wins}`. Without it the gate falls back to parsing `server.log` |
| `hsmp-server --debug-verbs` | RCON `MAP <arena>`, `START`, `ABORT`, `BESTOF <n>`, `DEBUG KILL <seat>` (reply `OK ...` / `ERR <reason>`); `START` with an unready player → `ERR`. Without it the harness runs the stopgap mode |
| `--parent-pid` | On the server, the sidecar and the master: exit when the parent dies (§3) |
| `HSMP_AUTOTEST` (HSMPMenu) | `host` = be seat 1 (join `HSMP_AUTOTEST_ADDR` when `HSMP_AUTOTEST_EXTERNAL=1`, else host a listen server and do not auto-start); `join` = join `HSMP_AUTOTEST_ADDR`; `HSMP_AUTOTEST_READY=1\|0` = auto-ready in the lobby; legacy `1` for the stopgap |
| Autotest commands | Under `HSMP_AUTOTEST`, HSMPMenu consumes `dev_cmd` AUTOTEST records from the DevCtl ring (`IPC.dev_poll`). The harness sends `hsmp-tools ipc-ctl --pid <game> --id <n> autotest pick_arena\|start\|ready\|unready\|leave\|quit\|move\|world_poke\|team|kit|mods_accept|mods_decline [arg]\|mods_accept\|mods_decline [arg]`; they run through the same code path as the buttons, with `cmd_sent` / `cmd_result` events. `move` (arg `"<seconds>[,noswing][,nowalk]"`, `autotest_mover.lua`) walks the local pawn (its own input disabled meanwhile) and swings its right arm by impulses, then stands still for the last 3 s; `quit` uses `KismetSystemLibrary:QuitGame`; `world_poke` (arg `"<cm/s>[,<n>]"`) is HSMPWorld's (its own DevCtl cursor; HSMPMenu ignores it): it throws the n free bodies nearest the pawn by a mod-side impulse after a touch claim (`world_sync`, WORLD-2); `team <n>` is the GAME MODE screen's YOUR TEAM pick, `kit <class>` the LOADOUT screen's class card + SAVE, `mods_accept` / `mods_decline` the SERVER MODS screen's buttons |
| Events | The emitters in §1 |

### Environment the harness gives each game

`HSMP_INST=<i>`, `HSMP_STATE_DIR=hsmp_state_<runid>_<i>`, `HSMP_DEV=1`, `HSMP_LOG_ECHO=1`, `HSMP_IPC=shm`, `HSMP_TEST_CVARS=r.VSync=0;t.MaxFPS=60` (two instances share one GPU: with VSync on a missed 16.7 ms frame drops to 30 fps on the heavier arenas, starving the pose stream; HSMPMatch applies only allow-listed cvars), `HSMP_AUTOTEST`,
`HSMP_AUTOTEST_ADDR`, `HSMP_AUTOTEST_EXTERNAL`, `HSMP_AUTOTEST_READY`, `HSMP_NETSIM_ADDR` (impaired
clients), `HSMP_SIDECAR_EXE`, `HSMP_QUERY_EXE`, `HSMP_MASTER_URL`, and `HSMP_SERVER_EXE` (listen host only).
The save guard's redirect uses `HSMP_INST` in the slot name (`HSMP_<inst>_<slot>`).

---

## 5. G0 pre-push hook

```powershell
git config core.hooksPath .githooks          # once per clone AND per worktree
```

`.githooks/pre-push` builds `hsmp-tools` (`cargo build --release --locked -p hsmp-tools`) and runs `hsmp-gate g0`. G0 tests the working tree,
so the hook refuses a dirty tree and a push of any commit other than HEAD. The checks, in the order G0 runs them
(`REQUIRED_CHECKS` in `tools/hsmp-tools/src/bin/hsmp-gate/g0.rs`; a `g0.json` that lacks any of them fails DoD-13):

| Check | Tool | Notes |
|---|---|---|
| `bp_names` | `hsmp-tools check-bp-names` | Needs the game's `UE4SS_ObjectDump.txt` (game dir: `HSMP_GAME_DIR`, `<repo>/game`, then the main worktree's `game/`). Without one G0 reports `skipped: no game dump (set HSMP_GAME_DIR)`; `--strict` makes that a failure. Run by hand without a dump, the subcommand itself exits 2 rather than pass on an empty scan |
| `lua_check` | `hsmp-tools lua-check` | every mod `*.lua` compiles under Lua 5.4 |
| `lua_test` | `hsmp-tools lua-test` | every `lua-tests/*.lua` suite (`hsmp_log` included) |
| `travel` | `check_travel` (built next to `hsmp-gate`) | OpenLevel / OpenLevelBySoftObjectPtr / Server/ClientTravel calls or console `open`/`travel` strings outside `HSMPMatch/Scripts/director.lua`. Allow-list: the opt-in `HSMPMenu/Scripts/legacy_travel.lua` shim (until it is deleted). Retired mods are notes. Fails if `check_travel` is not built |
| `wg` | `check_wg` (built next to `hsmp-gate`) | world-guard rules W1-W3 over every mod, then `check_wg --strict --release-set` (every mod `mods/mods.release.txt` enables must be clean, TODOs included) |
| `unsafe` | `check_unsafe` (built next to `hsmp-gate`) | UE4SS API rules U1-U6 (see [lua-mods.md](lua-mods.md)). U1/U2 need the object dump: without it they are skipped (`--no-dump`) and the check reports SKIP, or FAIL under `--strict`; the other rules always run. Known findings: `tools/hsmp-tools/check_unsafe.baseline` |
| `image_kill` | `hsmp-gate` | DoD-12 kill-by-name grep (all patterns in §3) over the repo root, mods/, scripts/, server/src, tools/ (src/bin included), crates/, launcher/, tests/, .githooks |
| `instant_sub` | `hsmp-gate` | C1: `Instant::now() - <Duration>` panics on a PC booted less than that long ago. Fails on any new hit in server/src, crates/, tools/, launcher/, tests/. Known hits are listed in `tools/hsmp-tools/instant_sub.baseline` (`path: line text`); these are reported but do not fail, and a stale entry is only a note |
| `events` | `hsmp-gate check-events` | the event contract (`contract.rs`, §1): every event/judged field a real emitter writes (Lua release mods + shared, `hsmp-server`/`hsmp-sidecar --events`, the sidecar tap's S2G `cmd_result` payloads, netsim, the fake game) is known; every field the gate reads has a real emitter or a `PENDING` request; every field literal the rules read is declared; every fixture event is a contract shape; Lua names outside `M.EVENTS` fail unless recorded. `--verbose` lists every extracted call site |
| `gate_selftest` | `hsmp-gate selftest` | the mocked runs in `scripts/fixtures/` (one per gate hole found so far, each of which an older gate passed), every event in a real emitter's shape (regenerate: `hsmp-gate make-fixtures`). `p0_gate_today` is exactly today's emitters and must stay **incomplete**; `p0_gate_pass` adds the pending contract fields |
| `ipc_schema` | `hsmp-gate` | the committed generated IPC files equal `hsmp-tools gen-ipc` output, and `hsmp_ipc.h` compiles under `cl /W4 /WX` as C11 and C++17 (every size/offset `static_assert`); without MSVC the compile is noted, not failed |
| `state_files` | `hsmp-gate state-files [--verbose] [--update]` | the lint `check_no_state_files`: every state-dir name a Lua mod names (string literals in `mods/**/*.lua`, the generated schema skipped) must be in `tools/hsmp-tools/src/bin/hsmp-gate/state_files.allow`, and in `server/src` (outside test modules) `std::fs` / `tokio::fs` writes may only appear in its `fs:` modules (logs, panic guard, career guard, identity, the tap, hsmp-server / master persistence). The list is categorised, and the only categories allowed are config, identity and log (plus `runtime:` globs, `fs:` writers and `debt: <owner>` for a contract not yet moved to shared memory; the list has none): any other category fails, so plumbing, tool-output and knob files cannot come back. Unused entries are notes; `--update` drops them (the list only shrinks) |
| `cargo` | `cargo test --workspace --locked` | one run over the root workspace: `server`, `launcher`, `crates/*`, `tools/*` and the mlua Lua harnesses in `tests/` (HUD, interact, save guard, HSMPWorld). Skipped with `--quick` |
| `clippy` | `cargo clippy --workspace --all-targets --locked` | the CI clippy job's command, verbatim: a compile error or a deny-level lint (`approx_constant` and other `clippy::correctness` lints) fails, warnings are counted only. Skipped with `--quick`. Not in `REQUIRED_CHECKS`, so older stamps and the recorded fixtures stay valid |

A full pass with nothing skipped, on a clean tree, writes `<git-common-dir>/hsmp-g0/<commit>.json` (and the
legacy `<git-common-dir>/hsmp-g0.json`). A run that skipped a check (no game dump, `--quick`, `--only`, `--skip`) writes no
stamp. `build-and-deploy.ps1` records it in the deploy stamp
(`g0_ok`), and `-RequireG0` refuses to deploy without it. `mp_test.ps1` records a `--quick` G0 in each run (`g0.json`).

**DoD-13 order (the only one that passes):** commit everything (a dirty tree never counts) -> `build-and-deploy.ps1`
(it runs the full G0 itself when none is recorded for HEAD on a clean tree, then builds and stamps `g0_ok=true`;
`-SkipG0` skips that for dev loops) -> `mp_test.ps1` on the same, still clean, commit (its quick G0 must name the
same commit as the stamp). Any commit between deploy and run makes DoD-13 incomplete.
Do not skip the hook with `--no-verify`.

---


### CI (`.github/workflows/ci.yml`) and what G0 does not cover

CI runs five parallel jobs: Windows G0 (`hsmp-gate g0 --no-stamp --skip clippy`), Windows e2e
(`scripts/e2e-test.sh`), Clippy (the same command as the G0 `clippy` check, which is why G0 skips it
there), Linux (`cargo test --locked -p hsmp-net -p hsmp-server`, then a release build of `hsmp-server`, `hsmp-master` and `hsmp-query`) and a
Docker image smoke test. A full local G0 plus `scripts/e2e-test.sh` covers the three Windows jobs. Build caches are saved from
`main` only; pull requests restore them. The Linux job cannot be reproduced on Windows:
the `cfg(not(windows))` code (the sidecar's `/proc` process checks, for one) only compiles and runs
there. With WSL and a distro, run the job's test command in the distro on a copy of the tree
(rustup picks up `rust-toolchain.toml`):

```sh
rsync -a --delete --exclude target --exclude .git /mnt/d/<worktree>/ ~/hsmp-linux/
cd ~/hsmp-linux && cargo test --locked -p hsmp-net -p hsmp-server
```

Without WSL, the Linux job is first seen on the push.
## 6. Deploy and release profile

`scripts/build-and-deploy.ps1 [-SkipBuild] [-Dev] [-GamePath ..] [-BinDir ..] [-WriteCfg] [-Template ..] [-RequireG0] [-SkipG0] [-SkipNative] [-UE4SSSrc ..] [-DryRun]`

* Builds `cargo build --release -p hsmp-server` (unless `-SkipBuild`) into `$CARGO_TARGET_DIR` or `<repo>/target`,
  copies the binaries into `Binaries\Win64\hsmp\` (the layout the launcher installs) and points `hsmp.cfg`
  `bin_dir` at `hsmp` (relative), so a later `cargo build` elsewhere can never change the binaries under test.
  `-BinDir` writes an explicit folder instead (not stamped). `mp_test.ps1` reads `bin_dir` from there too.
* Copies every `Scripts/*.lua` of each `mods/HSMP*` and `mods/dev/HSMP*` mod and then `mods/shared/*.lua` into the
  mod's deployed `Scripts/`. Retired mods (`HSMPLobby`, `HSMPAdmin`, `HSMPCharacter`, `HSMPSettings`, `HSMPChat`)
  are not deployed.
* Builds HSMPNative with CMake (or, with `-SkipBuild`, reuses the previous native build) and deploys
  `ue4ss/Mods/HSMPNative/dlls/main.dll` and `hsmp_lua.dll`. A missing native build fails the deploy unless
  `-SkipNative` asks for a deploy without multiplayer (`HSMPNative : 0`).
* Writes `ue4ss/Mods/mods.txt` from `mods/mods.release.txt` (with a `.hsmp_bak` backup). It **never** turns a mod on
  by itself: a mod missing from the template is written as `: 0` with a warning. `dev` entries (HSMPDiag,
  UE4SS Keybinds) are on only with `-Dev`. The UE4SS developer mods (console enabler/commands, cheat manager,
  kismet debugger, event viewer, line trace, dumpers) are off in every profile. Third-party lines the template
  does not list are kept unchanged. Existing `mods.json` entries are kept consistent.
* Writes `hsmp.cfg` when it is missing (§2).
* Writes `Binaries\Win64\hsmp_deploy.json`: commit, branch, dirty, time, profile, G0 stamp, mod table, the SHA-256
  of every deployed file (`hashes`) and the commit the shipped binaries were built from (`bins_commit`,
  `bins_match_commit`).
* The gate does **not** need `-Dev`: HSMPDiag emits no gate events (the census comes from the Director, the stall
  probe runs in the release profile, §1.1). `mp_test.ps1` warns when there is no deploy stamp (DoD-13) and when a
  `-Netsim` override is weaker than the scenario's profile (NETSIM).
* The deploy does not touch `UE4SS-settings.ini`: a developer install keeps its consoles. Player installs get the
  release overrides from `tools/release/release.json` (`settings_overrides`: consoles and hot reload off), applied
  by `hsmp-release` when it packages UE4SS ([releasing.md](releasing.md), [ue4ss.md](ue4ss.md)). Developer-only
  behaviour in the mods sits behind `HSMP_DEV=1` (`cfg.dev()`).
