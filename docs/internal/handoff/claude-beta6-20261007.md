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
