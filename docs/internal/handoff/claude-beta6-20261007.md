# Handoff: road to 0.1.0-beta.6 and to solo-equal online combat (2026-10-07)

For the next agent (GPT Sol 6.1). Read this whole page, then `CONTRIBUTING.md`,
`docs/development/testing.md`, `docs/internal/handoff/claude-combat-20261005.md` (native
evidence and earlier measurements) and `docs/internal/handoff/modes-and-server-mods.md`
(the modes / mods test plan, still NOT run).

Repository: public `https://github.com/Cyrex0/HalfSword-MP`, branch `dev`. Locally that
remote is named `origin`; a second remote named `dev` points at HalfSword-MP-dev and makes the
bare refname `dev` ambiguous, so push with `git push origin refs/heads/dev:refs/heads/dev`.
Never push to `main`, never tag or release until the owner says so. User-authored commits
only (no AI trailers). No Python anywhere (tools and tests are Rust/C++, mods are Lua, host
scripts PowerShell / POSIX shell). Never inject OS input; drive the game through mod hooks,
the DevCtl ring (`hsmp-tools ipc-ctl`) and RCON.

Last state of this session: everything is pushed to `origin` `dev` (this handoff included). GitHub
once rejected three pushes in a row with "Internal Server Error" while G0 passed; a later retry
with the explicit refspec went through.

**Verdict: `dev` is NOT ready to ship as 0.1.0-beta.6.** Blockers are listed in section 6.
"Passes tests", "verified live" and "matches solo" are kept apart everywhere below.

---

## 1. What this session changed (all on `dev`, commit messages carry the numbers)

| Commit | What | Evidence level |
|---|---|---|
| `62741888` | `sidecar_shm` test drives a real placed match (protocol 11 drops lobby roots by design) | tests (Windows, 5/5 x5) |
| `9aa6bca7` | **Stand-ins were never created** under protocol 11: `pose_context_ok` needed `has_context`, which Root records do not carry. `PURE.root_context` | live: both stand-ins, Live census missing=0 |
| `a972aa9d` | Roots authorized at placement, not at Ready (census deadlock: 18 s wait, `census=DEFECT(0/1)` every round). Director pings the verified order without LOADED; server separates root authorization from the load barrier | live: census ok, Ready 8.1 s (was 22.6 s) |
| `1cfee0e9` | HSMPParity `drive` (walk/guard/swing via Willie_BP input events), frames-probe out-param fix | live |
| `f796f1c2` | `-ServerArgs` on mp_test/combat_test; autotest `team`, `mods_accept`, `mods_decline` | dry run |
| `c87ea519` | Server accepted a contact only on the retained cutting HitBox; native keeps the last module's Box (haft contacts 48-178 cm off it). Now telemetry | live: 226 wrong rejects -> 0 |
| `0ea7a593` | Stand-in impact yield fired on steady servo offsets (438 false "struck" in a no-combat gate). Now a step detector | live: 36/39 struck within 1 s of a real hit |
| `35313125` | Calibration tool classifies live DCD by exact native class | offline + live data |
| `e08088f5` | Launcher test server read once then closed -> Windows RST (4/5 failures) | tests 0/12 |
| `0f28df7e` | Weapon shape envelope excluded the hilt: modular T3 arming sword grips 31.5 cm behind the guard were refused, every claim of that player "no_cover: original source class unavailable" (42 %) | live: sword accept 46-55 % -> 80.8 % |
| `a1a6fc6d` | **Native AI drives MP players** (`parity ai on/off/auto`), `WG.ai_pawn` fallbacks; **per-blow native damage probe** (`combat_probe`) | live |
| `84b5b1cb`, `140db47f` | combat_test `-Arena -Kit -KitRules -Ai -Probe`; autotest `kit`; game windows found by `UnrealWindow` (MainWindowHandle was the UE4SS console); start-up event timing | live |
| `11ba20ad` | Lag comp names why a victim frame is absent; equal candidate poses and the floored label window | tests + live (12 % -> 9.7 %) |
| `0f4ff9df` | AI-driven fighter keeps the camera | live |
| `f7f3f98e`, `306618bb` | **Stuck-blade damage forwarding** (Inside Get Damage continuation under the accepted parent cid) | tests; live partial (section 4.2) |

Rejected after measurement (kept as evidence, not shipped): stand-in grip locked to the owner's
weapon-in-hand frame (`test-results/beta6/rejected-gripframe.diff`): grip rotation already
matched within 1 deg, the transplant made arm tracking worse (median 3.8 uu vs 0.6-0.7).

## 2. Measured state (live, this machine, two instances on one GPU)

### 2.1 Gates
- G0 (game present): PASS (cargo 1255 / 0, clippy 0 errors, all Lua suites, events, unsafe, wg).
- `p0_gate -Netsim typical` (70 rounds, 7 arenas): **FAIL** -
  POSE-1 (128 round x instance failures), SMOOTH-1 (13.7-15.9 clock resets/min, limit 1),
  SPAWN-1 (stand-in stretched 10.5-14.9 uu at drive start, limit 10, once 50.6), PAWN-1
  (one `weapon_r=None` at placed), DoD-2 (one 2.06-2.08 s frame stall per instance).
  Everything else passes (DoD-1,3-8,10,12,13, COMBAT-1, NETSIM, STATE-1, WORLD-1, DoD-6 census).
  Per arena, pose quality follows frame rate: Pit arm mean 1.4 uu, LordsHall 20 (sender
  20 fps), EastTower 24 / tip max 453 uu. Part is GPU contention (two games, one GPU), part is
  code (section 5.1). The run before this session's fixes is not available for a regression
  diff; short A/B runs (Alley, 4 rounds) are too noisy for pass/fail, use distributions.

### 2.2 Hit registration (AI vs AI, `combat_test -Ai`)
| Run | Accepted | Main rejections |
|---|---|---|
| protocol 10 baseline (2026-10-05, manual) | 0 / 89 | 73 hit_box |
| scripted driver, polearm | 2159 / 2418 (89 %) | 226 hit_box (fixed since) |
| AI polearm | 1584 / 1886 (84 %) | 236 no_cover bone frame |
| AI arming sword, clothing, before hilt fix | 88-94 / 171-191 (46-55 %) | source class unavailable |
| AI arming sword after hilt + sampling fixes | 712 / 815 (87.4 %) | 97 "delivered frames do not sample" |
Stand-in frame error at accepted hits (`proxy box frame differs`): center ~10 cm, rotation
mean 20-37 deg, max up to 166 deg (pose sync during fast swings, section 5.2).

### 2.3 Damage parity (paired per blow, `combat_probe`)
The attacker's own native Deal Complex Damage on the stand-in (same kit, owner-mirrored
vitals) logged as `PROBE native` beside the owner's replay `HIT from peer`:
- polearm, 791 pairs: Leg/Body Lower identical; Health tiny on both sides (blunt on mail).
- arming sword vs clothing, 136 pairs: Body Upper/Lower, Leg_L, Head Crush identical; matched
  single blows identical to the decimal. Aggregate Health native -52.6 vs replay -10.0:
  **~60 % of native Health loss came from slow (<300 uu/s) embedded-blade contacts** (the
  stuck-constraint route) -> forwarded since `f7f3f98e`, partially working (4.2).
- plate (knight) hides everything: 206 pairs, almost no vital change natively either.
- `hit_vel_factor`: shipped factors clamp only ~1 % of real blows (polearm 0.84 %, unarmed
  0.17 %; server rescale mean factor 0.975 / 0.984). They are not why damage looked low.

## 3. Tests and tools (how to get data fast)

| What | Command | Notes |
|---|---|---|
| G0 | `target\release\hsmp-gate.exe g0` (pre-push hook) | needs no running session: the observer locks hsmp-gate.exe |
| Build + deploy | `.\scripts\build-and-deploy.ps1 -Dev -SkipG0` | Rust + HSMPNative together; never -SkipBuild across protocol/layout changes |
| Regression gate | `.\scripts\mp_test.ps1 -Instances 2 -Scenario p0_gate -Netsim typical` | ~45 min, 70 rounds |
| AI combat session | `.\scripts\combat_test.ps1 -Minutes 15 -Arena Yard -KitRules custom -Kit "duelist r=w_arming3 l= armor=b_tunic,l_hosen3,f_shoes1" -Ai [-Probe]` | first round is the real one; AI every round; Yard (Alley walls block straight walking) |
| Kits | `-Kit "<class> [r=<weapon id>] [l=<id>] [armor=<id,..>]"`, classes knight / man_at_arms / duelist / brute / peasant, items in `mods/HSMPLoadout/Scripts/hsmp_catalog.lua` | `KitRules custom` for edited kits |
| Dev commands | `hsmp-tools ipc-ctl --pid <game> autotest parity "<exp>"`: `ai on|off|auto`, `drive <s> [walk] [guard] [swing]`, `frames <peer> [bone]`, `kit`, `fists`, `inventory ..`; `autotest combat_probe on|off`; `tune <key> <v>` (impact_dv, impact_ms, grips, downed_world, ...) | results in UE4SS.log and `<state>/.parity_results.txt` |
| RCON | `hsmp-gate rcon --addr 127.0.0.1:<port> --password <from plan.txt> "<CMD>"` | never print the password |
| Calibration | `cargo run --release -p hsmp-combat-sim -- --hvf-logs <parity.txt..> <server.log>` | needs >= 200 binding samples per class |
| Pose evidence | `cargo run -p hsmp-tools --example combat_pose_decode -- <tap.jsonl> <out>` | taps exist with `combat_test` |
Ad-hoc analyses used this session (bash/awk in the agent's scratchpad, NOT committed; port
them to Rust per the no-Python/Rust-tools rule, see 7): server.log tally (accepted / rejected by
reason / proxy frame error / per attacker), PROBE<->HIT pairing (key: attacker, bone, |vel|,
rig, cut; per-field sums and ratios), pose_quality distributions (median/p90 per metric,
per arena), stuck-call value ranges from `INSIDE_JOURNAL`.

## 4. Open items found live (start here)

### 4.1 Asymmetric damage between the two players
User report: one player visibly cut, health/stats reflect it; the other "looks hit but 100 %".
Measured in one round: player 1 had 72 accepted hits, player 2 only 9 (rejections few): one
fighter got downed early and the AI stops attacking downed / yielded targets (fight dynamics).
Separately a real visual desync: the attacker's native DCD paints wounds on the stand-in even
though its damage is gated, so a rejected or weaker claim leaves a stand-in looking cut while
the owner is untouched.
- [ ] Count claims per attacker per round in every run (tool, 7.3) and alarm on asymmetry not
      explained by downed time.
- [ ] Wound/gore paint: make the stand-in's wounds the owner's (replicate the owner's applied
      cuts / dismember state) and suppress local DCD paint on stand-ins, or re-apply only from
      accepted replays' outcomes.

### 4.2 Stuck-blade continuation (`f7f3f98e`), partially working
Live: continuations forwarded and accepted, but 7 refused "not the parent hit's victim, life or
bone" (reason now prints both bones) and 5 attacker-side calls had no claimed parent.
- [ ] Read the bone pairs from the new reason; extend `same_bone` only with native evidence
      (`stuck-weapon-native.txt` maps pelvis -> spine_02; check others).
- [ ] Parent binding: the constraint is bound to this frame's `CX.pending` claim for the same
      body/bone; when the native contact gate or the per-bone selection dropped that claim
      there is no parent. Bind to the native call (DCD hook order) instead, or send the parent
      even when it is a gate-suppressed contact.
- [ ] Victim replay passes `Hit By Component = nil`; native passes the weapon module. Resolve
      the attacker stand-in's module (needs the source ordinal on continuations; today the
      server rejects component bits without FLAG_COMPLEX).
- [ ] Head / bone / snap events (`@41353`, `@41728`, `@43210`): verify they arrive as Inside
      calls with their constants; `Dismemberment Check` from the constraint is not forwarded.
- [ ] A parent later parried while a continuation is deferred still delivers it: drop deferred
      continuations of rejected parents.
- [ ] Paired probe run: native vs replay Health with continuations on (target ratio ~1.0).

### 4.3 Remaining hit rejections (~10 %): "delivered frames do not sample at the view time"
Around a sender hitch two frames 2 ms apart (steps 10 and 22) both explain the frame-start
label and their poses differ (12 ms apart). Fix the contract, not the tolerance:
- [ ] The stand-in publishes its exact display time (the sample time the servo targets
      represent) instead of `floor(label + hstep)`; the server samples there directly.
- [ ] Replace `sample_delivered_bones`' candidate search; keep "never a hidden future frame".

### 4.4 Rounds that never end in AI tests
A downed AI-driven player never "holds G"; the AI stops attacking a yielded foe; duel ends only
on death or deliberate surrender (by design). Decide with the owner whether the native AI yield
(`Event Lose Match`, hooked as verified defeat) is a surrender in duel. Harness: a round
timeout or `DEBUG KILL` after N s without claims, recorded as such.

### 4.5 Regeneration after bleeding
Owner saw health climb back after a bleed. The previous pass implemented the native regen
formula; verify against solo with the same wounds (side by side, same kit).

## 5. Work breakdown to perfection (each item: measure first, then fix; acceptance in brackets)

### 5.1 Pose sync quality (POSE-1, SMOOTH-1, SPAWN-1)
1. [ ] Re-run p0_gate on a machine with one game per GPU (or one instance per PC) to separate
   GPU contention from code [per-arena arm/tip p95 table].
2. [ ] SMOOTH-1 clock resets 13-16/min: `HSMPAvatars` resets when the sidecar playback pt
   jumps > 50 ms (`main.lua` "clkr"). Log each reset with sidecar buffer state; find whether
   it is starvation (rate change) or real discontinuity [<= 1/min typical].
3. [ ] Idle stand-in stalls "twisted against a joint" (hand_r/upperarm_r, 41-125 deg) for
   idle two-handed holds; grip rotation matches, so measure joint limits per body with the
   frames probe (constraint library now reads limits; GetCurrentJointAngles returns constants)
   [no repeated stall re-pose at rest].
4. [ ] Spawn stretch 10-15 uu (once 50 uu) at drive start [<= 10 uu].
5. [ ] PAWN-1 `weapon_r=None` at placed (Yard r2) [no offending pawn_state].
6. [ ] DoD-2 2.06 s stalls without hitch events [find the frame].
7. [ ] Swing-time rotation error at hits (mean 20-37 deg): measure owner vs stand-in hand and
   weapon during fast swings (frames probe at 60 Hz, not 1 Hz) [p95 < 10 deg].

### 5.2 Combat parity
1. [ ] 4.1, 4.2, 4.3 above.
2. [ ] Damage per weapon class: sword, axe/mace, polearm, fists, each vs clothing / mail /
   plate, paired probe runs [per-field ratio 0.95-1.05 for ordinary blows].
3. [ ] `hit_vel_factor` per class from >= 200 binding samples (AI duels give them fast).
4. [ ] Hit momentum to the victim (measure whether the echo contact already pushes first).
5. [ ] Downed / fallen stand-ins: lie on the floor, get up cleanly (`tune downed_world`),
   stand-ins never get Fallen/Downed/broken flags yet.
6. [ ] Severed limbs keep physics bodies; dismemberment state replication.
7. [ ] Projectiles, thrown weapons, traps, quivers (11 classes fail closed); articulated maces.
8. [ ] Impact yield tuning (impact_dv/ms, gain, cap) on measured servo error; defaults are
   still first guesses (only the false-positive detector was fixed).

### 5.3 Modes and server mods (plan in `modes-and-server-mods.md`, NOT run)
Harness support exists now (`-ServerArgs --mods-dir C:\hsmp-test-mods`, autotest `team`,
`mods_accept/decline`, `kit`); test mods are in `C:\hsmp-test-mods` (TestBanner, BigData 20 MB).
A1-A18, B1-B19, C1-C2 all NOT RUN. Priority: deathmatch respawn (A12-14), teams + friendly fire
(A4-7), roulette/brawl (A10-11), hill per arena (A8), mods consent/download/load/unload
(B2-8, B12), beta.5 compatibility rejects (A18, B17).

### 5.4 Release
1. [ ] p0_gate green twice; map_change, reconnect, host_leave, server_restart, start_refused,
   p0_wifi green twice (DoD exit).
2. [ ] Version bump to 0.1.0-beta.6 (Cargo.toml workspace + `tools/release/release.json`),
   CHANGELOG, release only on the owner's word.

## 6. Blockers for beta.6 (ranked)
1. p0_gate FAIL (POSE-1, SMOOTH-1, SPAWN-1, PAWN-1, DoD-2).
2. Modes and server mods never run in the real game.
3. Stuck-blade continuation incomplete (bone mapping, parent binding, Hit By Component).
4. ~10 % of hits rejected around sender hitches (display-time contract).
5. Stand-in wound paint not the owner's (visual desync).
6. Release gates (twice-green scenario set) not run.

## 7. New test platform ("lab"): fast, accurate, self-reviewing

Goal: one command answers "did this change make online combat closer to solo, and what broke",
in minutes, with numbers, and gets better each run. Build it in Rust (`tools/hsmp-tools`,
new `hsmp-lab` binary) on top of the existing harness; no Python.

### 7.1 Principles
- Never wait on a fixed sleep; never re-run what did not change; never judge pass/fail from
  4 rounds: use distributions with confidence bounds and a stored baseline.
- One session, many experiments: keep the games running and switch arena / kit / mode / tune
  knobs over RCON + DevCtl between experiments (no restart: 2 min saved per experiment).
- Every number traceable to raw evidence (run dir + log line), every verdict compared to the
  previous accepted baseline.

### 7.2 Pieces
1. **Session controller** (`hsmp-lab session`): starts the harness once (2 games, windows side
   by side), holds RCON and the game PIDs, exposes `lab exp <name>` that applies a recipe:
   arena, mode, kit rules, each player's kit, AI on/off, probe on/off, tune knobs, duration,
   round-end policy (timeout / DEBUG KILL when no claims for N s).
2. **Recipes** (`tools/hsmp-tools/lab/*.json`): e.g. `sword-cloth`, `sword-mail`, `sword-plate`,
   `axe-cloth`, `mace-mail`, `polearm-mail`, `fists`, `idle-standins` (pose at rest),
   `swing-only` (pose under motion, no damage), `hitch` (netsim burst), `respawn-dm`,
   `teams-ff`, `mods-download`. Each declares its metrics and acceptance bounds.
3. **Collectors** (Rust ports of this session's awk): server.log decisions (accepted /
   rejected by reason / per attacker / proxy frame error), PROBE<->HIT pairing per field,
   pose_quality distributions per arena and per peer, struck/stall/clock-reset counts,
   stuck-blade continuation accounting, claim asymmetry, calibration samples (feeds
   `hsmp-combat-sim --hvf-logs`), crash/hitch evidence. Read logs incrementally (byte offset
   per file) so a 5-minute experiment is analysed in < 1 s.
4. **Baseline store** (`test-results/lab/baseline/<recipe>.json`, committed summaries only):
   metric distributions of the last accepted build; `lab compare` prints deltas with a
   significance test (bootstrap CI on medians / p90 / ratios) instead of single-run pass/fail.
5. **Reviewer** (`lab review`): after each experiment writes a short report: regressions,
   improvements, new rejection reasons, unexplained asymmetry, top-3 suspected causes with the
   log lines; appends to `test-results/lab/journal.jsonl` so trends across commits are visible.
6. **Auto-bisect hooks**: `lab ab <knob>` runs the same recipe with a tune knob on/off in the
   same session (A/B within one session removes machine noise).
7. **Fast inner loop**: Lua-only changes hot-deploy (copy Scripts, restart only the games, keep
   server); Rust server changes restart only the server (clients reconnect); native/layout
   changes need the full build (assert the deploy stamp).

### 7.3 Acceptance metrics to track per recipe
- accept rate and rejections by reason (per attacker); asymmetry ratio;
- paired native/replay per field (Health, part healths, consciousness, bleeding, pain) with
  ratio and CI; stuck continuation forwarded/accepted/orphaned;
- proxy frame error at hits (center cm, rotation deg p50/p95);
- pose_quality per arena (arm/tip p95, jitter, foot slide, idle rms), clock resets/min,
  struck false positives at rest, stall re-poses/min;
- sender fps per instance (separates GPU contention from code), hitch counts;
- downed/fallen agreement between screens (both screens' vitals + stand-in state).

## 8. How to make online combat feel like solo (the sync model)

The design that works, and that this codebase already follows in the parts that pass:
1. **The owner is the authority over its own body and vitals.** Every damage to a player is
   applied by that player's own game through the game's own native function (Deal Complex
   Damage / Get Damage) with the attacker's original inputs, so armour, height, part health,
   bleeding and death are exactly solo. Verified per blow (2.3).
2. **The attacker is the authority over its contact.** The attacker's screen detects the hit
   natively on the stand-in; the server authenticates it against both players' real history
   (lag compensation on what the attacker actually displayed: relay log + native frames),
   never against a guessed pose. Rejections must be explicit and measured, never widened.
3. **Stand-ins are puppets, not simulations.** They follow the owner's transmitted pose with a
   velocity servo; they take no native damage (their wounds must be the owner's, 4.1); they
   yield to real impacts only as a step (`0ea7a593`) and return to the owner's pose.
4. **Continuations stay with their origin.** Ongoing effects that exist only on one screen
   (stuck blades, grabs, embedded projectiles) are forwarded as continuations of an accepted
   event, bound to it on the server, applied natively on the owner (4.2).
5. **Time is explicit.** Every sample carries its sender clock; displays publish the exact time
   they show; the server reconstructs only from delivered frames (4.3). No hidden future frames.
6. **World physics (props) follow one owner per body** with touch claims (`world_sync`), the
   same rules.
7. **Test the native way:** the game's own AI fights over the real network path; the same
   blows are logged natively and as replays; numbers decide (section 7).

## 9. Local resources (not in git)
Game install `D:\HalfswordMultiplayert\game`; PAK/AES/IDA material under `workspace\` and
`test-results\dev-feature-checks\` (use the isolated IDA copy, never the original .i64; the
ida MCP server failed to connect this session, headless idat works). Evidence of this session:
`test-results/beta6/*` (logs per run: duel-evidence, ai-online-1, ai-probe-*, sword-fixed-*,
inside-*, rotation-probe, p0_gate logs). Career saves were verified byte-identical before and
after all runs (GameProgress, Save_GiveUp, Settings).

## 10. Codex continuation: source milestones (2026-10-07)

User clarified that there is no AMD GPU on this PC and requested portable launcher
rendering. OpenGL remains the default, with automatic Windows DX12 retry before
the app is created. Native screenshots on the local RTX 3060: OpenGL14 frames,
DX12 15 frames, injected OpenGL startup failure -> actual DX12 15 frames, each
exit0. Launcher128 tests and clippy passed. Evidence:
`test-results/launcher-amd-20261007/` (historical folder name, no AMD fault established).

`hsmp-lab` now supplies session/exp/stop, analyse/compare/review/baseline and AB/BA
knob experiments. Recipes cover sword/axe/mace/polearm/fists against three armour
levels and idle/swing poses. Native sessions use the existing save/PID/window harness.
Kit SAVE waits for an exact current-epoch server receipt; repeated same-kit SAVE
acknowledgements had failed to publish and are fixed. Four lab checks pass. See
`docs/development/lab.md`. Configured combat sweep75min plus transitions, idle2min;
full sweep runtime is not yet measured. Historical cloth3 reference:197 accepted,
35 rejected (25 no_cover,10 parried),133 unambiguous Health pairs; two signatures
excluded as ambiguous. This failed/insufficient reference is committed under
`test-results/lab/baseline/sword-cloth-reference.json`, explicitly unaccepted.

Stuck-blade source fixes and exact native offsets are in
`stuck-blade-continuation-20261007.md`:44 combat Rust,9 schema,267 combat Lua,
28 damage-parity checks pass offline. Structural dismemberment and arbitrary
constraint rebinds remain gaps. Exact physical timestamp sampling also covers
native broad capsules and displayed main/offhand weapons;61 lagcomp tests,
7 sampler tests and153 Avatar Lua checks pass offline. Per-reset `x_pose_clock_reset`
telemetry records buffer/clock/native context before correction; the50ms threshold is unchanged.
Protocol12 now explicitly rejects11 even with content checking disabled (zero peer admission),
because display timestamps and origin-only continuations changed semantics. These numbers are not live
parity evidence and do not close release blockers.

The owner selected native AI yield as surrender in AI-driven Duel tests. The
dev-only own `AI_BP_C` native lose branch uses `Give Up` (@256919), without the
player-only `Give Up2`; scoped surrender reason2 preserves native living HP.
Human Duel KO rules remain unchanged. Harness timeout stimuli have separate journal entries.

Modes/mods offline checks:38 real server RCON/browser/manifest subchecks pass;
26 modes,18 shared server-mod executions,7 kit-client and38 modes/mods Lua checks pass.
Evidence `test-results/modes-smoke-67968ebf/report.json`. All39 A/B/C full live
acceptance items remain NOT RUN pending the native runs. The old20MiB single-file
fixture exceeded the16MiB file limit; use two10MiB files. Beta.5 is refused on Duel
as well as other modes by the current universal protocol version check.

Fresh G0, deployment, native lab sessions, typical p0_gate and twice-green release
scenarios are still required. Version remains beta.5 until beta.6 is justified;
no main push, tag or release is authorized.

### 11. Native reproduction and focused development checks (2026-10-07)

Test workflow milestone `db4860ee`: focused combat checks ran six Lua suites and
115 Rust tests in 8.2 seconds including rebuild. Launcher ran 128 tests in 7.6 seconds.
Full G0 remains the push checkpoint. Lua assertion counts are distinct from suite counts;
native timing benchmarks are opt-in while allocation and delivery checks remain enabled.
The reconnect ledger regression no longer retries after interference from other tests.

Native session `test-results/lab/native-20261007-04` reached both real game instances
and ran a five-minute sword/clothing recipe against deployed `9ccf61d15d`. Raw run:
`test-results/20261007-200119-fc7120-combat_manual`. The owner observed repeated missing
opponent health/HUD/collision and twisted spawn arms. The log reproduces one owner's
vitals sampling stopping and the remote pose becoming stale, then releasing the stand-in.
Round 9 recorded two AI handovers to different pawns less than one second apart; the
first targeted a same-team pawn before the remote stand-in became ready. Ownership and
fresh current-life readiness are under repair; this run does not certify fairness.

The summary contains 435 exact native/replay claim pairs, but only 53 observed Health
pairs. Their replay/native sum ratio is 0.8551 (exploratory bootstrap interval
0.7498–0.9431), below the requested parity band and below the 200-pair minimum.
Clock reset rate p90 is 23.93/min over 34 windows, above the 1/min bound. Rotation
error at hits is unavailable. Acceptance is unavailable: inherited `RUST_LOG=warn`
suppressed accepted decision records. Observed refusals remain recorded; lab startup
now explicitly requests server INFO logging, and missing accepted logs cannot become
a fabricated zero-percent acceptance result.

The session closed normally: no new crash dumps; CRASH, DoD-12, NETSIM and STATE-1
passed. That manual-harness verdict is not a p0, spawn, combat-parity or release verdict.
User priority is real native evidence and playable/fair spawns before expanding gear
sweeps. Three Sol agents at extra-high reasoning own pose/AI ownership, readiness/gear
proof, and native combat diagnostics respectively. No main/tag/release is authorized.

The next source checkpoint fixes verified AI ownership across temporary native PC
possession, queues AI takeover until fresh current-life pose/vitals/native collision,
and binds takeover once to the verified fighter life. Director readiness uses actual
successful pose writes and advancing vitals on both sides rather than visible census
or elapsed time. Pending placed vitals and poses relay during Loading; verified DM
respawn initialization relays without reviving the seat or mutating the combat ledger.
Old-round/life death state cannot mark a newly displayed pending stand-in dead.

Focused evidence: pose domain four Lua suites/115 Rust tests in 10.5 seconds;
Director/kit/loadout 462 checks; Combat 319 and protection/trace 19 checks; four real
server record-flow tests including initial/DM initialization; native API/sampling
tests retain zero allocation growth over 10,000 frames. These are offline checks.
`spawn-fairness` is a 240-second AI recipe with probe disabled for the next native
spawn/collision run. Native diagnostics record DCD outputs, nested GD values and
ordered trace hits/tags under probe, using exact claim/life IDs and no extra native
damage or trace calls. Actual native hook output and fixed-spawn behavior still
require a new deployed run. Exact armour-passport verification remains unfinished;
`gear_verify.lua` is a draft helper, not integrated proof of all loadouts.

Session `native-20261007-05` on deployed `24b97e4e50` recorded 14 successful
matched sword/cloth spawns through round seven, continuing pose/vitals on both
owners, and 198 accepted contacts with 197 native replay records. No health-stream
loss, duplicate AI takeover or old-life death applied to a fresh stand-in was
observed. AI commands sent before START were refused while the world was unsettled;
manual reissue enabled both AIs only for rounds three through seven. This is five
AI rounds, not a full 240 seconds of AI combat. Early rounds ended by explicit
harness timeout. HUD rendering was not independently inspected.

Review evidence is `test-results/lab/native-20261007-05/spawn-fairness.review.json`:
clock-reset p90 was zero; honest acceptance was 0.76448 versus the 0.97 bound;
rotation evidence had only 38 samples and cannot satisfy its 200-sample bound.
Probe was disabled, so this run cannot establish exact native damage parity.
The polearm left-arm twist remains a separate physical defect. New read-only
`parity weaponstate` diagnostics compare actual native weapon physics with cached
Avatar state before any physics behavior changes. A final Ready-to-Live freshness
latch now holds initial controls until proof is fresh, retaining control through
ordinary injuries after release. These new edits still need a matched native run.

Checkpoint `fb5e3196` fixes lab AI timing, the first-Live input latch, and persistent
smallest-secondary-display placement. A matched experimental deployment was made
before that commit, so session `native-20261007-10` truthfully carries the prior
commit plus a dirty deploy stamp. Its completed four-minute spawn recipe recorded
20 observed native AI handovers across ten rounds; both AIs initialized combat
automatically after Live. There were 494 accepted contacts of 658 decisions.
The unchanged honest-acceptance metric is 0.77918 and fails its bound. Qualified
Live-only evidence is 492 accepts of 502 non-parry decisions (98.008%); 130 rejects
occurred after RoundOver. This separate breakdown does not replace the gate.
Clock reset p90 is 23.9068/min and remains outside its bound. Rotation p90 is
50.7455 degrees with only 69 samples, below the required 200.

Repeated native weapon captures showed actual/cached sim=true and free stand-in
grips, so no speculative cache invalidation was applied. A stronger physical
readiness gap was observed: round seven Ready at 21:19:33.751/.908, lower-arm stall
at 21:19:35.270, AI at 21:19:37.555/.564, upper-arm stall at 21:19:38.502. Per-frame
physical alignment must qualify initial spawn readiness, separately from current
source/vitals/collision. Do not infer exact fault duration from five-second maxima.

The subsequent native probe records real DCD surface/density and nested wound
outputs. Ordered trace evidence remains unavailable in that deployment: the native
POST bridge used Blueprint argument order. Pinned UE4SS `LuaMod.cpp` passes native
POST context, ReturnValue, then reflected arguments. The bridge signature is now
corrected and 23 protection checks pass, including real-bridge argument ordering
and receiver isolation. A new deployment must prove actual ordered trace output.

The next coordinated source checkpoint adds six-limb initial physical qualification
(5 uu/10 degrees for 150 ms), exact-life qualification retained through wounded
pause/reconnect resumes, and a LOCAL-only typed playback extension. The maximum
32-peer record is 12,808 bytes; the bus is 16 KiB, adding 768 KiB to the segment.
Generated IPC layout hash is `262ac8453e7af06c`; network protocol stays 12. Focused
native transport tests retain zero allocation growth. Verified AI intent stops
outside Live and resumes only the same generation; native wounds/physics and
already captured trades are preserved.

Clock evidence found 146 rapid positive/negative reset pairs with negligible delay
changes: newly read data was backdated to the frame's earlier physical time. The
producer now retains actual slot receipt time and projects to physical time using
that receipt; the 50 ms detector and raw events remain unchanged. A production
read regression covers the 67 ms oscillation; a real 80 ms shift still resets.

Health audit found 308 exact replay samples with fresh native HIT `+0.00` readings
wrongly excluded by the changed-field mask; one early origin acknowledgment had
no native snapshot and remains unavailable. Fresh diagnostic Health now travels
only as a callback-local fifth return, never in cached ReplayAttempts or the
production changed mask. The old 59-pair ratio is biased and incomplete. Combat
327/protection 23 assertions passed; native effects of this entire checkpoint
still require deployment and real-game validation.

### First-round polearm recovery checkpoint

Clean checkpoint `32367829` passed G0. Native session 11 proved the ordered
Doublet/flesh protection trace and fresh zero-Health diagnostics, but the default
polearm spawn still failed physical readiness. Sword/cloth produced five AI
rounds; that does not certify the polearm, all gear, or the release gates.

Session 12 (`20261007-221011-303f48-combat_manual`) reproduced the owner's
first-round duplicate polearm. Astra's read-only review traced an initial 912 cm
placement, then a native hand collision with Yard fence actor StaticMeshActor_1061
nine milliseconds later, followed by weapon drop and whole-kit re-dress. The
original weapon and replacement had different native actor names. This is evidence
of duplicate creation through recovery, not evidence of a world-replication clone.
Captured actual/cached COM differed by less than 0.00002 uu; no cache refresh was
justified. A separate native grip capture showed actual right linear Z limited
while our cached intent still said free; native Timeline8 writes that limit.

The focused candidate retains the recoverable actor through hand-only failures,
preserves kit ownership only through the exact assigned fighter's temporary
possession context, reasserts existing grip limits on fresh matching components,
and checks fresh physical pelvis COM on the first placement attempt. Unknown
physical reads fail placement verification. Readiness bounds stay unchanged.
Offline tests and review are prerequisites, not native acceptance. Require a
matched deployment and first-round polearm actor identity, actual grip limits,
physical readiness, and repeated AI rounds before calling this defect resolved.

Checkpoint `9953a4c5` passed the clean G0 (54 Lua suites, 1,279 Rust tests), then
received a matching developer deployment. Native session 13/raw run
`20261007-224749-4a88dd-combat_manual` reached Live on the first default polearm
spawn. Its original own weapon actors remained held through the fresh capture;
one outfit application per client and no re-dress were observed. Actual proxy
right-hand constraints were free, not merely labelled free. This initial round
was explicitly aborted after AI combat, not completed through biological defeat.

The separate four-minute polearm recipe had five AI rounds, four natural round
completions, no load failures, and no observed initial re-dress/recovery loop.
Normal teardown passed save/process/crash checks. Repeated fairness still failed:
round two's input release differed by 28.684 seconds. The decisive round-three
capture at 22:52:16 showed the same valid weapon/root with cached_sim=false but
actual_root_sim=true, no servo timestamp, and the right grip locked; COM differed
by only 0.000003 uu. Refreshing this mutable physics state is justified by this
capture. The earlier session-12 sim=true capture did not prove that transition.
One later initial pelvis/fence contact was observed despite zero immediate
placement residual; do not claim the placement correction eliminates all native
post-teleport contacts. The recovery branch was not exercised by this run.

Native session 14/raw `20261007-230249-83f1b6-combat_manual` matched clean
`6df78e27` (focused checks; full G0 still pending for this commit). Mutable
simulation refresh executed 54 false-to-true transitions and captured states
agreed with native reads. The repeat recipe completed six natural combat rounds,
voided one loading timeout, and aborted its last loading round at its explicit
240-second boundary. Successful control-release gaps were 37-318 ms. The first
default loading round also failed: a left lowerarm remained near 178 degrees;
the final recipe loading round had a right hand near 16 cm/45 degrees. Every
recipe start had a native pelvis/fence contact. Save/process/crash teardown
passed, but this is not a passing spawn/fairness acceptance run.

The owner's next explicit priority is complete body/limb damage and native
dismemberment. Static native proof explains two independent gaps. Joint
dislocation checks DriverSkeleton/Mesh separation above 15 cm and the native
`Block Spine Breaking` bool, independently of `Invulnerable` or Health. The
candidate spawn-only guard preserves that bool through exact fresh ownership
and restores it with readback before calling Live injury behavior normal.
Astra found and required fixes for pre-placement reassignment and restoration
retry leaks; final source approval is only for a matching native trial.

Sharp severing uses separate stuck-weapon marker/geometry checks and delayed
native component construction; authenticated Inside/GetDamage replay alone
does not reproduce that path. Current mask sampling reads a legacy array, while
decoded native completion writes a typed part map. Neither unreadable zero nor
hidden bones alone proves intact/severed topology (camera hiding is possible).
New read-only `body_probe`/`body_snapshot` telemetry must establish actual part
HP, individually readable injury flags, arrays/map, native bones, cut boxes,
constraints and constructed limb/armor/UpperBodyMesh components. It adds no
native damage, trace or physics execution. Blueprint arbitrary/nested GD PRE
remains unavailable; only our existing owner invocation can be bracketed with
fresh PRE/POST. A proposed confirmed-mask consumer removes missing-limb servo
targets and preserves physical exclusion through temporary source release;
actual collision exclusion/restoration, corrected topology producer, detached
components and authoritative cutting continuation are still required.

Native15 (clean `5084bb2542`, raw
`test-results/20261007-235519-c869cf-combat_manual`) confirms protected native
`Block Spine Breaking=true` becomes actual `false` before Live. It does not
solve every spawn: default round 2 timed out, and default round 3 retained
roughly 15–16 cm / 45–48 degree right-hand errors despite fresh source poses,
healthy owner limbs, complete empty native sever collections and simulated
weapons. The repeated polearm experiment failed with `RCON START: ERR start
blocked: 0 of 2 peers ready`; its first combat round used the recorded harness
timeout/DEBUG KILL, not a natural win. Normal teardown reports no new crashes;
the manual harness PASS is not spawn, fairness or dismemberment acceptance.

Read-only body diagnostics prove an owner replay hit reduced left-arm health
100 to 92.9927 with increased bleeding/pain. Native regeneration explains later
gradual health increases; combat_probe was explicitly off. Detailed body rows
expand armor/bone readbacks and add overhead, so body_probe was disabled after
the injury slice (native confirmations 00:02:34–36). Probe-active timings must
not be promoted as clean performance evidence. Both native sever hooks failed
registration throughout this run; missing callbacks do not prove absent cuts.
The next diagnostic repair retains exact reflected names and reports the
actual registration exception and returned IDs, suppressing identical error
spam while retrying unavailable hooks. It still needs matching native readback.
The audit contains 594 body rows and no observed structural break, dislocation
or completed sever; all sever collections are readable and empty in that slice.
One initial own-kit recovery explicitly reused the same polearm actor/outfit.
Twelve native original-false guard restores were verified; final abort refusal
against a replaced pawn/world is stale-target protection, not a verified write.

Native16 (`9b491cb0`, raw `test-results/20261008-001108-944b10-combat_manual`)
isolated the sever-hook lookup failure: the engine searched only `Initiate` /
`Delayed`. Pinned UE4SS `LuaMod.cpp:88` removes the first `Function ` substring
anywhere in its argument, including inside these Blueprint names. Prefixing
the full reflected path with `Function ` preserves the actual path through
that parser. The source fix has focused regression coverage and Astra review;
matching native registration and completed sever callbacks are still required.
Read-only owner/proxy arm captures show equal native elbow/wrist limits and
zero proxy motors versus owner strength 250. Current-angle values are not
qualified physics evidence, and the moving/drop-repickup slice does not
reproduce Native15's persistent simulated/free-grip fault. No limits changed.
Native16 closed normally with no new crash; it was a diagnostic run, not a
passing all-gear/body/spawn acceptance run.

Native17 (`b09a57a5`, raw `test-results/20261008-001612-261fa2-combat_manual`)
closes the hook-registration defect in both clients: actual script IDs 68/68
for Initiate and 69/69 for Delayed, registered=true, ambiguous=false, error=none.
No completed sever was captured. Round 1 reproduced the pending spawn failure:
healthy owner at 100 on every limb, all structural flags false, complete empty
sever collections; failing proxy hand_r 12.7 cm / 53.5 degrees and lowerarm_r
27.5 degrees, fresh source at 60 fps. Actual weapon simulation and COM matched,
and the proxy right grip's six limits/drive were zero. All four frame/joint
requests refused the pending Loading context; their Native16 results must not
be substituted. Round 1 timed out; normal teardown reported no new crash.
The next diagnostic change admits only the exact assigned pending tuple and
revalidates fresh native world/pawn/Mesh, without granting readiness or changing
physics. Native cutting continuation, structural wound replication and actual
sever topology/collision remain open gameplay requirements.

The next combined checkpoint adds a reviewed opt-in native caller journal
(`HSMP_NATIVE_CALLER_PROBE=1`, default off). It uses the actual pinned DLL's
legacy script callbacks and pointer-reference frame getters, only after a real
Native frame established the game thread. Current frame chains are bounded to
16, native object/class observations are copied scalars, serial-zero function
roles re-resolve their full paths, and PID+sequence identifies shared-log rows.
The journal grants no damage/factory authority; sequence is an observation,
not a causal invocation identifier. Native callback/ancestry proof is pending.
The harness now preserves HSMPNative.log with raw evidence. The body probe uses
a separate bounded topology reader preserving 15-part presence versus true
values and independent availability, while ordinary health/death Vitals stay
unchanged. Its positive-cut fixtures are mocks. Pending frame/weapon snapshots
now follow exact assigned Loading/Countdown tuples and fresh world/pawn/Mesh;
placement verification is explicitly separate from diagnostic admission.

Native18 (`637e04b5`, raw `test-results/20261008-005617-d355a6-combat_manual`)
ran with the caller journal enabled. After the saved byte boundary, 1,242
observations and 3,171 frames demonstrate actual Get Damage -> Deal Complex
Damage ancestry and 14 cross-object weapon/Willie chains. All recorded chains
completed; absent sever/constraint roles do not establish absence because the
journal samples. All three Loading rounds timed out, with frame gaps up to
43.941 seconds. Joint requests refused world-not-settled; they do not validate
or invalidate pending-context admission. No new crash was recorded.

Native19 (same commit, caller journal disabled, raw
`test-results/20261008-010529-4ab8c8-combat_manual`) reached Live. The first
180-second sword/cloth recipe completed five rounds through actual native AI
yield (server cause 6), with ten actual takeover records and no harness kill.
Round 2 failed readiness and was voided; round 7 ended at the recipe boundary.
There were 254 owner replay outcomes, including 28 negative Health deltas.
Surrenders at Health 95.83 and 84.54 are not biological-death evidence. The
body probe was off throughout that recipe, so structural damage and completed
severing remain unproved. Honest acceptance was approximately 88.8%; proxy
rotation median 26.52 degrees/max 96.19, based on only 47 samples. A left-arm
readiness failure persisted despite simulated sword/free grips. Maximum frame
gap was 1.128 seconds; warmer loading confounds comparison with Native18.
The later 60-second capture attempt was interrupted by normal session expiry
and returned ERR no players connected, not a completed experiment. Four valid
pending BODYFRAME_CONTEXT captures at 01:13:09.241-.299 match
4007177418340735/round2/life1: exact owners have placement_verified=true and
fresh displayed proxies have separate unknown placement. This is native proof
of pending diagnostic admission. Arm limits match (shoulder75/75/45,
elbow75/30/30, wrist45/75/85); owner strength500/250/250,damping1 versus proxy0.
Socket quaternions closely match, while current-angle queries remain implausible
and physics_verified=false. These are not qualified actual joint angles. Root
sent the body diagnostic under the wrong DevCtl key; no body snapshots resulted.
Normal teardown reported no new crash. Do not count this session as body,
dismemberment, fair-spawn, or all-gear acceptance.

The next producer correction is deliberately limited to native whole distal
regions. Cooked Willie Delayed's Hide Bone Local at offset2298 maps parts
3/4/6/7/9/10/12/13 to lowerarm_r/hand_r/lowerarm_l/hand_l/calf_r/foot_r/calf_l/
foot_l. Spawn Bone is a different, proximal attachment/cut root and must not
be used here. Parts0/1/2/5/8/11/14 use None; arbitrary head/torso/partial cuts
need independent topology geometry. Delayed hides the distal root before
Map_Add(part,true) at24563. Publication requires complete typed entries,
process=false before/after, same fresh owner/world/Mesh/full life and the
mapped root actually hidden on that Mesh. Key presence/false and camera hiding
alone never qualify. Confirmed positives survive unavailable reads within the
same body; a new publication life cannot borrow them. Scalar Vitals continue.
Protocol12 cannot represent unknown topology independently of its zero bone
mask: initial unavailable/unsupported regions are not intact-body proof.
Matched natural distal-sever/collision acceptance remains pending.

The same checkpoint captures all seven reflected sever inputs (Master Mesh,
native part, Attach Marker, ordered Overlapped Markers, Box1, Box2, Weapon) as
bounded typed identity/geometry observations. Owner and exact displayed source
contexts are separate and freshly validate actor/Mesh/world and full life.
Blueprint snapshots are POST-only; no pre-marker wear or accepted eligibility
is inferred. No native sever/factory call is added. Stand-in local sever guards
still prevent completed source cuts, so absent callbacks do not prove coverage.
Focused checkpoint: four Lua suites, 556 assertions, zero failures; four changed
Lua files parse; C++ caller walker/admission has 26 focused checks and CTest pass.
These are assertion counts, not 556 independent real-game experiments.

### Native20–21: caller cost and real body observations (2026-10-08)

Native20 (`2b3dcf0c`, raw `test-results/20261008-013833-ddbfc6-combat_manual`)
reproduced the owner's severe slowdown with the optional caller journal enabled.
After the saved log boundary it produced 5,329 observations/15,884 frames, but
both clients timed out Loading. Maximum heartbeat gaps were 7.105/6.638 seconds;
pose transmission averaged only 0.756/2.051 Hz over the measured intervals.
Those are network pose rates, not renderer FPS. Normal verified-PID DevCtl quit
stopped both clients. The earlier lookup budget change did not solve the cost;
keep HSMP_NATIVE_CALLER_PROBE disabled for gameplay validation.

Native21 (same deployed commit, journal disabled, raw
`test-results/20261008-014254-a01555-combat_manual`) completed the 600-second
session normally with no new crash. The initial default round still had a peer
Loading timeout. The subsequent 150-second axe/cloth recipe completed six rounds
through verified native AI yield, with twelve actual AI takeovers. Yield is not
biological death. Heartbeat maxima were 1.054/1.594 seconds, with no measured
gap over two seconds; pose transmission averaged 44.86/37.51 Hz including travel.
This demonstrates usable gameplay with diagnostics disabled, not renderer FPS
or a controlled single-variable performance acceptance.

The brief cloth body capture contains 420 actual LAB_BODY observations, including
129 matched native Deal Complex Damage PRE/POST pairs, two Get Damage PRE/POST
pairs and one displayed-source Dismember Function Initiate POST. Fifteen matched
complex-damage pairs reduced Health and none increased it. Exact owner peer2,
match5606929952832964/round2/life1, Get Damage cid99 reduced Arm_R from82.58257
to0. All observed structural flags remained false and completed-cut maps were
readable and empty: limb HP zero alone does not prove fracture or severing.

The actual source Initiate at01:46:14.4413429 supplied part4 and eight readable
lowerarm markers, including DM_Sphere_Lowerarm_R_13 with tags Lowerarm R/HP000,
plus real same-Mesh lowerarm cutting boxes and axe mesh. Its source guards still
had ForceDisableDismemberment=true and Current Part remained0. This verifies the
POST argument binding; it does not establish accepted eligibility, a causal
damage parent or an owner completed cut. Unknown Master/parent fields stay
explicitly unavailable. No native factory or sever invocation is introduced.

Fairness remains failing: cloth honest acceptance0.73574 (333 observations),
proxy rotation median30.49 degrees/p95 74.66/max131.16 (46 samples), and mean
clock resets6.60 (27 windows). The plate recipe also finished, but sampled
owner/peer Vitals were from unmatched Loading/world/round transitions and cannot
be used as a matched damage comparison. Neither recipe establishes solo parity.
Plate round1 and round2 specifically ended by recorded 60-second harness
timeouts (lab-actions.jsonl rows27/30, policy DEBUG KILL1), not native AI yield
or natural death. Plate honest acceptance was0.718137 (816 observations), proxy
rotation median32.18/p95 89.62/max166.69 degrees (188 samples, below200), and
mean clock resets20.04/min (39 windows). These fail the recipe's fairness bounds.

### Offline inventory checkpoint (2026-10-08)

`tools/mapdump/inventory.ps1` extends the existing read-only cooked parser.
Final fresh output: `test-results/dev-feature-checks/inventory-harvest-20261008/`.
13,029 effective indexed packages balance as10,536 exported,0 failed and2,493
explicitly skipped. The selected closure contains10,549 packages, including13
map bodies explicitly outside inventory scope. All indexed gear roots were
selected; there are zero missing strong object dependencies. There remain2,027
weak name-table candidate observations (307 distinct paths), not proven live
dependencies. LogicMods and shadowed archive entries are outside this effective
top-level game-package scope. Raw copyrighted exports remain ignored locally.

The canonical catalogue retains883 full package/class rows:154 built weapons,
397 weapon module variants plus4 module bases, and324 armor gear classes, including
134 built armor/clothing items (28 clothing),126 cores and62 modules. One weapon
animation row and one armor animation row are separate from playable gear. Short names collide;
package-qualified class identities must be used. All883 game-class ancestor
chains now reach positively resolved native endpoints; engine defaults remain
unavailable. Unique observed cut/stab/blunt defaults are available on314 rows
each. Missing/ambiguous fields remain unavailable, with candidates/provenance.
The quaternion magnitude incorrectly labeled size was removed; no canonical
gear-size value is currently proved. Native enum Names establishes Steel value3
as Bare, independently of its enumerator suffix.

This proves offline package/class/default coverage, not runtime constructed
protection/density, every module combination, decoded complex collision
triangles, or combat parity. Raw component/SCS/BodySetup references are retained.
Focused extractor build and24 ledger/provenance regressions passed. Loadout and
armor observer checkpoint:6 Lua suites,714 assertions passed;5 production Lua
files parse. Astra reviewed the loadout and read-only observer; a matching
native clothing/plate trace and actual equipped-passport check remain pending.

Read-only follow-up identified a remaining fairness defect in class-only modular
weapon setup: GI Available Weapons1H/2H are merchant/save stock, not class-default
recipes. Selecting the first family passport is not a canonical tier choice.
Cooked free-mode inventory data assets contain complete native presets, while
tier class CDOs can expose a grip without a head. These require explicit recipe
mapping; neither a tier suffix nor an empty head may be guessed. The current
passport identity checks do not close this recipe-selection gap.
