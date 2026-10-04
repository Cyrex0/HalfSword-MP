# beta5 review: framecost (per-frame cost of the Lua mods)

Branch `cloud/framecost`. Scope: `mods/HSMP*` (not HSMPCombat), `mods/shared`, `crates/hsmp-native`,
`crates/hsmp-ipc`. Everything below is behaviour-preserving unless it says otherwise.

## How it is measured

New lua-test suite `tools/hsmp-tools/lua-tests/framecost.lua` (`hsmp-tools lua-test framecost`).
Every UE4SS call and the native module are replaced by allocation-free stand-ins (constant
return values, no-op writes), the GC is stopped around each simulated frame and
`collectgarbage("count")` is read before and after, so the number is the mods' own per-frame
garbage (temporary tables, closures, formatted strings). The harness prelude's `pcall` wrapper
(a `table.pack` per call) is bypassed so the numbers are what UE4SS runs. Each case checks a
budget that the beta.4 code fails. The "before" numbers come from the same suite run against an
export of `acbc700` (`HSMP_ROOT=<export> hsmp-tools lua-test framecost`).

| Case (per call / per frame) | beta.4 | now | budget |
|---|---|---|---|
| `ipc.flags` / `ipc.peer_play` / `ipc.bus_table` (one facade call) | 112 B | 0.1 B | 8 |
| `sync_native` (HSMPSync per-frame sender, native sampling, one 60 Hz sample) | 3178 B | 121 B | 400 |
| `sync_lua` (same, Lua sampler) | 11234 B | 1877 B | 2500 |
| `avatars_native` (HSMPAvatars `on_frame`, one 22-body v2 stand-in, native servo) | 141007 B | 5466 B | 8000 |
| `avatars_lua` (same, Lua servo) | 166310 B | 6193 B | 9000 |
| `world.can_drive` (HSMPWorld per-body check of the 16 ms tick) | 368 B | 0 B | 40 |
| `world.read_hands` (HSMPWorld hand read, once per tick) | 1720 B | 152 B | 200 |

About 2 KB of the remaining `avatars_*` bytes are the native mock's `peer_play` (the real module
refills its output tables in place).

Lua CPU of the stand-in driver (`on_frame`, same harness, real `os.clock`, this shared 4-core
box, informational only): 343 us -> 130-139 us per stand-in frame with the native servo, 414 ->
171 us with the Lua servo. Per-frame GC pressure went down by the same factor as the bytes.

## Changes

1. **IPC facade `call()` without `table.pack`** (`mods/shared/hsmp_ipc.lua`). Every native call
   of every mod packed its results into a table; they now pass straight through a vararg
   helper. Test: `framecost` case `ipc` (112 -> 0 B per call).

2. **HSMPAvatars v2 driver** (`mods/HSMPAvatars/Scripts/main.lua`, `avatars_pure.lua`).
   - `PURE.advance` / `PURE.aim` are scalar (no table per quaternion, position or nested
     advance) and fill a per-stand-in ring of 6 table sets (targets are read until two drives
     later by the quality history, aims until the next drive).
   - `PURE.fk_retarget_in` rebuilds this frame's targets and aims in place (the original
     `fk_retarget` stays for the dev `v1aim` path).
   - `PURE.play_from_out(o, into)` refills one of two alternating tables per peer (slot /
     weapon / root / control tables reused) instead of ~28 new tables per sample.
   - acceleration and velocity history tables are reused; the metrics loop uses scalar
     `qangle8` and the twist formula written out, reuses `sv.cmd[i]` and a scratch transform
     table for bodies nothing keeps past the frame.
   - `PURE.servo` (Lua servo) writes its quaternion products out as scalars; the Lua path's
     velocity arguments are two reused tables; contact impulse and weapon lookups no longer
     make a closure per body.
   - Proof of identical numbers: `framecost` case `pure_equal` runs the new functions against
     the beta.4 implementations kept verbatim in `lua-tests/lib/avatars_pure_ref.lua` on 3000
     random samples (advance, advance with acceleration, aim, aim into a table, qangle,
     clamp_acc, fk_retarget, fk_retarget_in, play_from_out fresh and refilled across changing
     masks): every number identical. `cargo test -p hsmp-native --test servo_parity` still
     matches the scalar `PURE.servo` against the native servo bit for bit (4000 vectors).
   - Test that fails without it: `framecost` `avatars_native` / `avatars_lua` budgets.

3. **HSMPSync per-frame sender** (`mods/HSMPSync/Scripts/main.lua`) and **`hsmp_wg`**
   (`mods/shared/hsmp_wg.lua`). Reflected reads go through module-level functions
   (`pcall(f, obj, ...)`) instead of a new closure per read (weapon fields, class names,
   addresses, real time, world delta); the Lua sampler fills one reused bone table and reads
   weapon components without a per-sample table. `WG.pc`, `WG.world`, `WG.world_key` and the
   frame id use functions made once per guard; the world key is built with one concatenation.
   Test: `framecost` `sync_native` / `sync_lua`; `lua-test sync`, `world_guard` unchanged.

4. **One native call per pose sample** (HSMPSync, `NSAMPLE.all`). `sample_local` already takes
   root, held weapon and pose in one argument table (mask 1 / 2 / 4); HSMPSync made three calls
   (three tables marshalled, three `sample_verify`s). Now one call per sample; the Lua path
   writes only the parts the mask lacks. Tests: `lua-test sync` case `native_sample` now
   checks one call per sample and that a refused pose leaves root native; `framecost`
   `sync_native` checks 600 calls for 600 samples. Diagnostics difference: a refusal is counted
   once per call (the 5 s `fallbacks` number), not once per part; the `pose sample+write avg`
   in that line now includes root and weapon (one call).

5. **HSMPWorld per-body reads** (`mods/HSMPWorld/Scripts/main.lua`). `valid`, `addr`,
   `field`, `is_sim`, `get_loc`, `body_pose`, `natively_held`, the velocity read and the
   tick's PlayerController / hand reads pcall shared `RD.*` functions instead of closures.
   Test: `framecost` case `world`.

6. **Module splits** (mechanical, code moved unchanged):
   - `HSMPAvatars/Scripts/avatars_pure.lua`: the whole PURE block (play parsing, quaternions,
     servo, targets, reference skeleton, tune knobs). main.lua 4356 -> 3961 lines, main-chunk
     locals 196 -> 192.
   - `HSMPWorld/Scripts/world_pure.lua`: the pure helper block and W2's record / codec
     functions (`install(T, K, W2)`), 3373 -> 3004 lines, locals 172 -> 167.
   - `HSMPMenu/Scripts/menu_umg.lua`: the stateless UMG helpers, called as `U.*`; locals 183 ->
     176.
   Loaded with each mod's `load_module` (require, then next to main.lua); a missing sibling
   raises at load with a clear message. Tests that read the PURE source now read
   `avatars_pure.lua`: `crates/hsmp-native/tests/servo_parity.rs`, `tests/sample.rs`,
   `crates/hsmp-pose/src/posecodec_v2.rs` (REF_T check), `tools/pose-truth/tests/lua_pure.rs`,
   `lua-tests/pose.lua`, `lua-tests/ipc.lua` (path changes only; hsmp-pose and pose-truth are
   outside this area).
   HSMPCombat was not split.

## Not done / tried

- **Native target build** (advance / aim / retarget for 25 slots in Rust). After the changes
  above the Lua target math is about half of the remaining ~130 us per stand-in frame here; a
  native port would save perhaps 30-60 us per stand-in frame but needs its own bit-for-bit
  parity test against `avatars_pure.lua` and the Lua path as fallback. Left as the next step.
- **World key memo per frame in `hsmp_wg`**: each mod has its own guard, so a memo would only
  help a mod that checks twice in one frame; not done.
- **HSMPHud** runs at 10 Hz and builds its model per tick; not touched (low per-frame cost).
- No IPC record layout change; the layout hash is unchanged (`ipc_schema` PASS).

## Test results

- `hsmp-tools lua-check`: 50 files, 0 errors.
- `hsmp-tools lua-test`: all suites pass except two that fail identically on the beta.4 tree on
  Linux: `hsmpworld` "qobj golden vector 2" (sin/cos tie at yaw 90 in glibc; Windows CRT
  matches the Rust vector) and `menu_ui` aborting at its Windows-only `mkdir "...\..." >nul`
  (that command also leaves a stray `nul` file in the repo root on Linux). With that mkdir
  patched locally to `mkdir -p`, `menu_ui` passes 12511 checks on this branch.
- `hsmp-gate --repo $PWD g0 --quick --no-stamp`: every check PASS or SKIP (no game dump)
  except `lua_test`, which fails only on the two baseline Linux failures above.
- `cargo test -p hsmp-native --test servo_parity`, `-p hsmp-pose-truth --test lua_pure`,
  `-p hsmp-pose --lib lua_ref`, `-p hsmpworld-tests`, `-p hsmpworld-sim`: pass (except the
  baseline Windows-only `claims_and_held_items_are_typed_records`).
- `cargo test --workspace --locked --no-fail-fast -j 2` (CARGO_INCREMENTAL=0): only the known
  Linux-only baseline failures (hsmp-launcher firewall x5 and two install case tests; hsmp-native
  api, records, s2g_epoch, sample; hsmpworld-tests claims_and_held_items_are_typed_records).
- `cargo clippy --workspace --all-targets --locked -j 2`: exit 0, 179 warning lines (baseline).


## Needs Windows / in-game verification

- A normal two-instance gate run (POSE-1, SMOOTH-1, SPAWN-1) to confirm stand-in behaviour is
  unchanged; the math is proven identical offline but the ring buffers' lifetimes are only
  covered by the offline driver.
- The `pose sender` and `pose peer N ... frame cost` log lines: frame cost should drop; native
  sampling should still read "sampler native" with few fallbacks.
- `native_sample` with the combined call on the pinned UE4SS (the native side already accepted
  combined tables; the offline mock checks the Lua side).
- That `avatars_pure.lua`, `world_pure.lua` and `menu_umg.lua` are deployed and packaged
  (they are top-level `Scripts/*.lua`, which deploy and the release tool copy).

## Risks

- Ring-buffered target / aim tables: correct as long as nothing keeps a target table longer
  than two drives or an aim table longer than one. The current readers (quality history a1 / a2,
  `p.aim`, `p.targets.Pelvis`) are within that; new code that keeps a target table longer must
  copy it.
- `play_from_out` refills the play table two reads later; code that keeps `p.last` from an
  older read would see it change (none does).
