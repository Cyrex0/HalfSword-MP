# HalfSword-MP handoff — 10 October 2026

## Redesign: display-only puppets (supersedes the per-frame proof path)

The owner approved replacing the per-frame verification doctrine below. Every
earlier run failed the same way: each client re-proved weapons, gear and the
whole roster on every authority frame, which cost 200–300 ms of game-thread time.
Every result was older than the 250 ms freshness limit by the time it applied,
so no run reached LIVE. Position-only sync would also have kept the bridge's
limb and weapon drift.

The new path:

- **Authority** (`sample.rs` compact path): each fighter's update now carries
  its full physical pose. That is an `hsmp_pose` codec-v2 blob with 23 bodies,
  held weapons and the control block. The wire is `RESULT_VERSION` 4, with
  `State.pose` capped at 768 bytes. In gameplay mode the worker passes the
  held-weapon pointers.
- **Client bootstrap** is unchanged: begin, passport, construct, settle,
  restore gear, settle, possess.
  - Pending native latent work no longer blocks forever. Lua and native
    finish both advance after 3 s, because Blueprint polling delays can
    re-arm indefinitely.
  - Gear is checked once, after setup and after possession. A mismatch is
    reported (`x_native_gameplay_timing` with `kind=gear`), not fatal.
- **Client per frame** is one call, `native_gameplay_puppet` (`puppet.rs`).
  - At bind, each fighter's mesh gets physics, collision and tick turned off.
    Its CharacterMovement stops ticking, and a hidden PoseableMeshComponent
    calculator is added. Held weapons get physics, collision and tick off.
  - Each frame: sample the `poseplay::Playback` jitter buffer, teleport the
    root, and place the mesh at the pelvis. Set the 23 bones in component
    space on the calculator. `puppet_publish` (`native_puppet_impl.h`) then
    copies them into the visible mesh, using the pinned stages from
    `pose_transfer`. Teleport the held weapons, and set the own
    PlayerController's control rotation from the authority.
  - Per frame, each object is only re-resolved through its weak handle. A lost
    object clears and rebuilds the roster, up to 3 times, then the client
    stops.
- **Readiness**: a fresh result applied sends `K_GAMEPLAY_READY`, which keeps
  the server LIVE and admits input. Stale frames still play back from the
  buffer.
- **Input**: the client sends all 8 axes and 7 buttons it already captures.
  The authority already executes all of them, and the Move + Run limit is gone.
  The lab AI uses `Input.ai`.

Still open: dropped weapons freeze where they left the hand; sheathed weapons
are not moved explicitly; finger poses are reference pose. The client's own
body lags by the round-trip time, and there is no prediction yet. The old
per-frame APIs (`native_gameplay_apply`, `_weapons`, `_confirm`, and C++
`gameplay_apply`, `_complete`, `_weapons`) are now unused. Remove them after
the first live run.

## Read this first (pre-redesign state)

The public PvP beta is released. **The current native authority/two-client
candidate with strict complete weapon readback has no verified LIVE run.**
Earlier Move + Run build `c03d8896` passed the original 10-second LIVE gate in
`20261010-150912-cd7161-native-host`; visible body/HUD and native combat parity
were still unverified. Combat parity, limb damage,
dismemberment and co-op Abyss remain unfinished. There is no defensible overall
completion percentage. Offline catalogue coverage and passing code checks do
not establish native gameplay parity.

The user requested this handoff and storage cleanup during ongoing development.
The latest implementation is committed and pushed; its actual game verification
is the next required step. Do not restart the project or repeat completed
analysis merely because a new chat takes over.

## Repository and exact state

| Item | State |
|---|---|
| Workspace | `D:\HalfswordMultiplayert` |
| Branch / remote | `dev`, `https://github.com/Cyrex0/HalfSword-MP.git` |
| Latest implementation | `a80322019ef04493cdaa3803c5d689166aa2c9b7` |
| Implementation push | Pushed to `origin/dev` |
| Full G0 for that implementation | PASS: 78 Lua suites, 1380 Rust tests in 79 binaries; 26 clippy warnings, no errors |
| Focused candidate checks | Strict native production/fixture compile PASS; native presentation 2269, weapons 181, Rust gameplay 21 checks/tests PASS; independent review closed |
| Last deployed and game-tested build | `6de97731f8fcdbd26bb02c5f2b3b095e63018e17` |
| Latest implementation deployed/game-tested? | **No.** ABI6 roster/performance correction still requires deployment and an actual run |
| Active game/build processes at cleanup | None observed after the full gate completed |
| Career saves | Latest actual run independently verified all 20 hashes unchanged |

Check `git status` at takeover. This handoff and the pointer in the older journal
are documentation changes above the tested implementation. A deployment must
use a clean committed HEAD with its own full G0 stamp and exact deployed hashes.
Never treat an earlier stamp as admission for a later HEAD.

Commits are authored as **Cyrex0 <moashie@gmail.com>**, with no AI trailers.
Push only with `git push origin refs/heads/dev:refs/heads/dev`; a second remote
named `dev` makes a bare destination ambiguous. Root owns builds, git index,
deployment, game processes and window operations. Agents edit disjoint files.
Use retained Sol 6.1 high/extra-high agents; the user prohibited further Astra.

## Public release

- [PvP release v0.1.0-beta.6](https://github.com/Cyrex0/HalfSword-MP/releases/tag/v0.1.0-beta.6)
- [Merged PR #8](https://github.com/Cyrex0/HalfSword-MP/pull/8)
- Release/main commit: `4f3018826bd2f12c105f2146986bb9052febb129`.
- Published ZIP: 28,111,890 bytes; SHA256
  `79bc2f4a2dff76536b05a14187ffd8e3105e21a9ac03db4c3e7f425ba49c1149`.
- Signing, reproducibility, public download and main CI were previously verified.
  Accepted legacy PvP drift, twisted arms, performance and body/gear limitations
  are documented. Do not replace a published asset in place.

The later native-authority pivot remains on dev. Do not PR, merge, tag or release
that work until its required native scenarios and documented bounds pass.

## What changed and what it actually proves

| Commit | Change | Actual evidence |
|---|---|---|
| `d03e5ed3` | Stop original Willie Actor tick after construction; wait for freshly observed native latent work before gear setup | Both clients constructed and entered initialization queries; subsequent finish still found pending work |
| `f4f38fb8` | Wait after equipment restoration; freshly recheck gear; allow only exact pre-mutation pending finish | Both clients correctly retained loading on pending; a later tick-enabled refusal stopped the run |
| `6de97731` | Stop tick after possession/HUD, then freshly read back false; label existing failures by API | Client 1 passed the former tick failure and reached native state application and weapon batches; no verified LIVE |
| `a8032201` | Native ABI6 takes the complete original source roster count; defer metadata until all expected fighters finish; remove a measured duplicate alias proof | Code checks and independent review pass; no actual run yet |

The duplicate alias proof cost 25.117–37.089 ms in eight failing actual samples.
Only that callback-free repeated pass was removed. Every callback guard, capture
pre/post proof, full alias/owner/world check and final publication check remains.
Its actual saving and complete verification cost are unmeasured.

The roster change prevents an own-first one-fighter configuration being
prepared, then discarded by the next spawn. The count comes from the retained
original server directory, never a Lua count. Each pawn stores count 1–32;
disagreement, overfill and multiple owners refuse. Metadata waits for the exact
count, every pawn at finished stage, and exactly one owner. Sequential Lua
bootstrap stages are unchanged. No successful readiness or gear values are cached.

## Latest actual game evidence

Canonical run: `test-results/20261010-202103-a842f2-native-host` on `6de97731`.
Topology: one native game authority and two normal clients. Caller probe and
heap/debug capture were off. Both visible clients were on the smallest secondary
display. The original startup limit was 65 seconds and receipt limit 250 ms.

- C1 PID30640, own-first: first finish succeeded at 26.377–30.122 s; first native
  apply succeeded at 48.249–48.342 s. Later weapon batches repeatedly expired.
- The eight instrumented weapon prefixes took 139.213–229.791 ms; the final
  phase never completed. The first full Lua apply span was 93.452 ms. Later apply
  spans were 63.706–81.924 ms. These spans nest where documented; do not add
  inclusive subspans as independent costs.
- C2 PID48720, remote-first: first finish reported typed pending at 25.823 s;
  still preparing at 63.225 s; finish reported generation refusal at 65.477 s
  during the startup deadline shutdown. Once-per-generation phase traces do not
  expose all later attempts. Do not invent a cause or a cold metadata subspan.
- No LIVE, accepted input dispatch, complete gear result, model/HUD screenshot
  acceptance or combat acceptance was observed.
- Cleanup PASS: supervisor14208, authority47436 and both clients absent; 20 save
  hashes unchanged; no new dumps or unobserved children.

The earlier `20261010-190634-ebf737-native-host` run positively observed an
original **remote weapon root** becoming Garbage (`00000008`→`40000008`). It
did not prove which native route retired it. Later failures before complete
weapon verification do not establish that this issue is fixed.

## Acceptance rules and next actions

1. Commit any handoff-only changes with the next coherent milestone, run full
   G0 on the exact clean HEAD, then deploy with `-RequireG0`. Preserve the source
   commit distinction above; `a8032201` is tested code, not an actual game pass.
2. Run the unchanged authority-plus-two-client standard gameplay test. Inspect
   **both** client logs, not only the harness headline. Require complete gear
   verification and continuous LIVE, then qualify actual model/HUD pixels.
3. Use existing bounded native/Lua timings to choose the next cost correction.
   Do not add another diagnostic-only milestone before using these observations.
   Do not widen receipt/startup limits or remove original-identity checks.
4. Once live startup is stable, implement and verify native body/limb state and
   combat action replay; then faithful cuts/dismemberment, gear/material/collision
   scenarios, fair spawn/pose, co-op Abyss waves and remaining modes/server mods.

Bounds remain **250 ms receipt**, **65 s startup**, and **10 s continuous LIVE**
for the standard gate. The operator supports a total 30 s continuous observation;
this does not relax startup or freshness. No stale readiness, receipt renewal,
replacement-actor adoption, RF masks, guessed defaults or failed-profile
rebaselining. Native errors stay explicit.

The user decided that native AI yield counts as surrender in AI duel tests.
Keep that decision when validating native round-end behavior.

The compact gameplay path currently supports **Move + Run only**. The proposed
full combat action/axis queue and ACK contract is not implemented. Compact state
does not yet carry continuous physical limb transforms or cut state. Inventory
coverage of 141 melee recipes is offline coverage, not all-gear combat parity.
Back sheath slot4 is unsupported; non-Back slots0–3 have primary support.

## Native primary evidence to reuse

- `test-results/native-initialization-proof-20261010/possession-tick-lifecycle.md`:
  PlayerController possession explicitly reenables tick. Route
  `355B140 → 3559F90 → 37D5E60`; `37D5FFC` tests pawn byte32 bit4
  (`bStartWithTickEnabled`), then `37D6007` invokes tick setter slot480 with true.
  Actor setter `34A99B5` uses that same slot. This is a proved route, not proof
  the branch executed in a particular failed sample.
- The same directory's `post-restoration-cooked-latents.md`: cooked hand/armor
  work can legitimately enqueue delays after restoration. Actual pending UUIDs
  were not captured. Never replace fresh counting with a fixed sleep.
- Native ALL-action count `3717FD0`, 91 bytes, FNV64 `9ceeb57e10d0c85d`;
  World/GI manager selector `39E6370`, 28 bytes, `b05c21d8bd2ad52e`.
  Polls retain original manager/callback identity, validate bounded storage and
  code before/after, and require original positive pending then fresh zero.
- `test-results/native-body-pose-proposal-20261010/`: named physics transform
  `3E84C10 → 1D3FB90`; direct linear/angular velocities differ from socket/point
  velocity. Setter `3E22770` and EndPhysics publication `3E652A0` have retained
  primary boundaries, but are **not approved by complete lifetime proof**.
- `one-limb-capture-lock-gate.md` there records the remaining problem: resolution
  occurs before scene read lock; null/unsupported branches return defaults.
  `1F86B20/1D3FAD0` selects a scene owner without allocation pin;
  `3E97BC0` may leave a lock inactive. Body/actor/PhysicsObject retirement and
  write-lock ownership remain unproved. Finite output cannot establish success.

IDA MCP callable tools were unavailable in this session. Local pinned-binary
disassembly and cooked evidence were used; do not claim IDA MCP calls happened.
Use an isolated database, never modify the original. No Python is permitted.

## Commands and ownership

Read `CONTRIBUTING.md` and `docs/development/testing.md`. Use PowerShell 7:

```powershell
$taskPwsh='C:\Users\johns\.cache\codex-runtimes\codex-primary-runtime\dependencies\native\powershell\pwsh.exe'
cmake --build target/hsmp-native-cmake --config Release --target box_provider_compile native_presentation_check native_gameplay_weapons_check
target/hsmp-native-cmake/Release/native_presentation_check.exe
target/hsmp-native-cmake/Release/native_gameplay_weapons_check.exe
cargo test --locked -p hsmp-native --lib native_gameplay::tests -- --test-threads=1
$env:RUST_TEST_THREADS='1'
git push origin refs/heads/dev:refs/heads/dev
& $taskPwsh -NoProfile -File scripts/build-and-deploy.ps1 -Dev -RequireG0
& $taskPwsh -NoProfile -File test-results/native_operator_gameplay_windowed.ps1 -LogName native-next-gameplay -ObservationSeconds 30
```

Use the saved operator scripts for original PID/start-time/executable/window
qualification and independent cleanup. `native_operator_live_read.ps1` only
observes; it does not refresh admission. After a run, execute
`native_operator_cleanup.ps1 -RunPath <absolute-run-path>` and
`native_operator_summary.ps1 -RunPath <absolute-run-path> -Finding <measured-finding>`.
All 397 deployed files must match the same clean committed HEAD.
Do not edit tracked files during G0, deployment or a native run.

No OS input/activation, kill by image name, career-save deletion or secret
output. Leave any user Steam game untouched. The window operator uses qualified
owned windows and no activation; secondary bounds are `[-1760,0,0,527]`.
Do not use the expensive native caller probe during gameplay.

## Storage cleanup and retained evidence

The requested cleanup completed at 19:55:54 UTC. All **91 selected targets**
were removed. Measured free-space increase was **51,012,902,912 bytes**
(51.013 GB / 47.509 GiB): D gained 46.043 GiB and C gained 1.466 GiB. Afterward,
D had 106.417 GiB free and C had 32.236 GiB free. Logical removed file sizes
totaled 57.201 GiB; that is not the same quantity as measured disk reclamation.
Only regenerable caches and verified duplicates were selected.
Detailed before/after free space, removed paths and byte totals are recorded in:

- `test-results/storage-cleanup-report-20261010.json`
- `test-results/storage-cleanup-removed-20261010.json`
- `test-results/storage-duplicate-candidates-20261010.json`

The completed plan was 91 targets / 57.201 GiB logical size: old Cargo targets
for four worktrees; main debug incremental cache; unused nested CMake Cargo
cache; release-worktree repro cache; and 84 exact duplicate artifacts. Actual
reclaimed space is recorded in the completed report, not inferred from sizes.
The release checkout is protected/pinned and was retained. Main compiled
executables, libraries and native fixtures remain; builds can regenerate caches.

The latest four actual runs, all unique pose/IPC/crash evidence, native primary
proofs, SDK/ObjectDump/PDB data, inventory/catalogue, raw game packages, IDA
databases, source, Git history and career saves remain protected. Older duplicate
full logs can be reconstructed from their preserved exact-prefix reference and
byte count in the manifest. Canonical per-PID logs and run events remain.

The recurring follow-up `halfsword-multiplayer-native-iteration` was paused during
active work; inspect its current state before scheduling anything. Do not create
a duplicate. The last account snapshot was 8% weekly remaining with zero reset
credits; no reset or purchase occurred. No active goal was created.

For older evidence only, consult `native-scene-stream-20261009.md`,
`claude-combat-20261005.md`, and `modes-and-server-mods.md`. Their chronological
"pending" passages may be superseded by this handoff; use exact commit/run
evidence above to decide what remains open.
