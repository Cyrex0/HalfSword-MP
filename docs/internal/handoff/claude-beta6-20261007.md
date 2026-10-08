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

**Owner steering, 2026-10-08:** keep iterating while the owner sleeps, using native
tests and reverse engineering as needed. The owner conditionally authorizes a PR,
merge to main and release/update only once all intended behavior is verified and
the known gameplay gaps and documented acceptance gates are satisfied. This is
not authorization to ship the current failing checkpoint. Until that evidence
exists, verified milestones still go to dev only. A thread heartbeat named
HalfSword multiplayer native iteration continues the work every30 minutes and
stays quiet except for meaningful results, material failures or required input.

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

### Native22: first actual equipped-passport and read-only armor check

Commit03c71b7a passed full strict G0 (59 Lua suites;1,279 Rust tests in75 binaries;
clippy0errors/2warnings;93 production Lua parses) and matching RequireG0 deploy.
Raw run: `test-results/20261008-024922-2a708d-combat_manual`. Both windows were
confirmed on DISPLAY1 at x=-1760/-880,880x527. The old release lab controller
initially refused IPC with stale layout9824d6249fd2dc68 versus deployed262ac8453e7af06c,
before starting a recipe. Renaming its in-use executable to an ignored checkpoint
allowed a matching controller rebuild without restarting the games. Build the
lab controller explicitly before future sessions; the deploy's tool list does
not guarantee that auxiliary binary is current.

Two90-second fixed-axe recipes completed with the caller journal off and local
damage probe off. Cloth completed4 native-yield rounds with8 actual AI takeover
records; plate completed2 native-yield rounds with4 takeovers. No harness round
timeout occurred. Repeated loser notifications are not extra rounds, and yield
at Health96.72 is not biological death. Own cloth3-piece and knight12-piece kits
reported exact equipped passports and successful hands; fresh displayed peers
also reported exact=true. No unavailable-default/passport mismatch was observed
in these selected recipes. This does not validate every class/module recipe.

The read-only bursts recorded55 traces:3 cloth(2owner/1source) and52 plate
(27owner/25source). All55 have complete native trace reads;26 plate owner traces
had matching active replay spans.45 plate rows include density10000 armor tags;
the remaining rows must be inspected individually rather than called proven
soft-spot contacts. Observed cloth tags included defB1.45/defC0.5/defS0.25/dens150;
plate shoulder/cuirass tags included defC187.5/defS112.5/dens10000, with flesh and
clothing also in ordered hit lists. Caller/source-parent authority remains
unavailable. Some traces are native damage evaluation at isolated coordinates,
not world-space contact positions. These are actual native layers, not a paired
solo/network injury ratio or faithful sever/collision acceptance.

Fairness still fails: cloth honest acceptance0.77397(n146), proxy rotation
median22.04/max42.93 degrees(n6), clock resets mean2.105/min(n17). Plate honest
acceptance0.953824(n693), proxy rotation median25.14/max103.19 degrees(n74),
clock resets0(n26). Neither pose sample count reaches200; do not relax bounds.
The manual harness verdict passed its limited crash/teardown/network/state
checks, with zero new crashes and normal menu quits. It is not p0_gate or a
release verdict. No game/server/sidecar remained after teardown.

Read-only native follow-up: Native21 Arm_R-to-zero cid99 was Inside=true;
Get Damage excludes Inside from Break Arm R, so its false broken flag was
expected. Fracture also needs limbHP<=25 and the native snapping/proximity
inequality. Dislocation instead compares Mesh vs DriverSkeleton upperarm
parent-bone-space positions (>15 units) and Bone Snapping, not HP0. Cooked socket
getters use space3; the pinned SDK identifies3 as RTS_ParentBoneSpace, while
RTS_Component is2. Do not substitute component or world positions. Current audit
does not measure those inputs. Forwarding stuck damage does not reproduce the
source constraint's physical force history; measure the two bone positions and
source/owner native constraint forces before changing replay or joint limits.

The next reviewed checkpoint adds `scripts/lab.ps1`: it builds the controller,
uses Cargo's actual reported executable (including configured targets), preserves
CLI arguments/exit codes, and copies each session owner to a unique ignored path.
Independent Windows PowerShell5.1 verification passed17 focused checks, including
a rebuild while the old owner executable remains running. Use this wrapper for
future sessions/experiments instead of an unverified release lab binary.

The read-only body audit now samples exact space3 upperarm Mesh/DriverSkeleton
translations, Bone Snapping, strict native flags and map3/6 values. Both skeletal
components need distinct fresh identities and the current pawn owner/world.
Missing or changing inputs stay unavailable; sampled predicates never authorize
an event. Astra approved source; focused body+combat checks passed483 assertions
(130+353). A positive matching native diagnostic capture remains pending.

### Native23: positive native arm-input capture and current overnight scope

Commit22ea6bef passed full strict G0 and matching RequireG0 deploy. Its dev push
completed with the pre-push G0 passing (59 Lua suites,1,279 Rust tests in75
binaries, clippy0errors/2warnings); origin/dev now matches22ea6bef. The source
tree was clean at that checkpoint. No main push, PR, tag or release occurred.

The new lab wrapper drove a real session, experiment and normal stop without
an executable-lock or stale-controller-layout failure. Raw run:
`test-results/20261008-032557-453bc1-combat_manual`; experiment:
`test-results/lab/native-20261008-23/axe-cloth-arm.json`. Both windows stayed on
the smaller DISPLAY1; the caller journal and local damage probe stayed off.
The60-second cloth/axe recipe recorded6 actual native AI takeover notifications.
This is a takeover count, not evidence of6 completed rounds.

The read-only body audit recorded368 samples with qualified distinct current
Mesh/DriverSkeleton component identities and parent-bone-space3 positions.
Right-arm distance max0.18755255, left max0.19035925; zero samples exceeded the
native15-unit threshold. All368 complete dislocation predicates remained
unavailable because native completed-map inputs were unavailable. Neither an
actual dislocation nor a fracture/cut is proved. The probe was enabled for
approximately2minutes including idle and combat, then disabled; this run is
not an uninstrumented performance comparison. Detailed summary:
`test-results/lab/native-20261008-23/body-input-summary.json`.

Fairness remains below acceptance: honest acceptance0.91044776(n134), proxy
rotation mean31.82884/max137.75492degrees(n32), clock resets mean6.48309/min(n11).
The sample does not meet the200-pose minimum. Exact claim pairs remain0;65
calibration samples and3 delivered inside continuations do not establish solo
parity. The manual report passed only its limited crash, teardown, network and
state checks, with0 new crashes and normal menu quits. All game/server/sidecar
processes stopped. This is not a p0 or release verdict.

Astra's read-only native-angle investigation found local cooked frame2 axes
reproduce all four Native16 joint-angle triples when Euler degrees are converted
as radians. UE5.3 source getters use stored frame2 Euler values and the engine
wrapper applies RadiansToDegrees. Shipping5.4 attribution remains an inference,
not a machine-code inspection. Dividing by57.2958 would still yield stored
frame angles, not verified live joint angles. Keep physics_verified=false;
this does not explain away the independent pose failures or justify relaxed
joint limits. Local evidence: body-primary-joint-dictionary.json and Native16.

The owner subsequently authorized continued overnight iteration and conditionally
PR/main/release/update only once intended behavior is verified. That condition
is not met. A30-minute heartbeat continues this same chat, notifying only on
meaningful changes. Next bounded work: fixed Rondel weapon-passport selection,
explicit native modular recipes, pose/display-time fairness, accepted-parent
physical damage/constraint lineage, and the outstanding modes/native gates.

The next bounded clock correction was reproduced in probe-OFF Native22: all5
cloth reset events align with source cuts (inst1 event lines337/350/509;
inst2 lines236/248). Native23 inst2 event71 also shows cut2 reset149.071ms at
t27311 before body_probe ON event121 at t64812. drive_frame consumed cut_seen
on placement/repose returns before its clock integrated, leaving the clock from
the previous generation. The correction independently binds each clock record
to validated match/round/life/cut on its first actual drive. The50ms same-generation
detector is unchanged. The actual-drive regression covers cut, repose return,
sub50ms stale phase replacement, and a same-generation80ms error still reported.
Focused Avatar262 checks and Loadout143 checks passed independently at root.

The exact harvested fixed Rondel class now bypasses merchant-family lookup and
uses its complete runtime native passport. Partial/unavailable defaults still
fail before any mutation, and another package with the same leaf name has no
exemption. Astra approved this bounded correction; regression covers replacing
a polluted same-class passport and exact reuse. General modular preset/tier
recipe selection remains open. Actual Rondel equip and uninstrumented clock
behavior still need the matching Native24 capture before live improvement claims.

### Native24 deployment refusal and Native25 actual checkpoint evidence

Commitf7687d38 (clock lifecycle and exact fixed Rondel selection) passed strict
G0:59 Lua suites,1,279 Rust tests in75 binaries,clippy0errors/2warnings,93Lua
parses. Native24 used a SkipBuild deploy and failed before connection: changed
mods content3b388deed5c38697 versus old shipped sidecar9a0c8e04c279cb95. Raw:
`test-results/20261008-040101-2fd290-combat_manual`. Menu correctly refused;
150s lobby timeout,normal menu quits,0crashes,noorphans. No recipe ran and no
combat evidence was produced. A full RequireG0 rebuild/deploy then independently
confirmed identical compiled sidecar and Lua content hashes. SkipBuild is not
safe for changed Lua content: server/build.rs embeds it. The deploy preflight
and bins-match bookkeeping need to include this dependency, before any game writes.

Fresh Native25 raw:`test-results/20261008-040558-dc4405-combat_manual`, session:
`test-results/lab/native-20261008-25`. Both windows confirmed DISPLAY1;caller,
body and armor probes remained off. Rondel90s: both owners and displayed peers
passed exact loadout/equip checks with ModularWeaponBP_DaggerRondel_C,Rok,
Lnone on owners;6 actual AI takeovers and3 authoritative round results. This
proves the complete runtime fixed passport can equip in these cases, not all
recipes or solo injury parity. Clock resets0 over16 windows;honest acceptance
0.91005291(n189);proxy rotation mean35.45606/max80.01591degrees(n22).

Plate axe180s:clock resets0 over56 windows;honest acceptance0.93764706(n1700);
proxy rotation mean26.75144/max76.87013degrees(n177). Pose still fails and lacks
200samples; neither acceptance meets0.97. Clock observations support the
reviewed lifecycle correction, not a global pose-quality or repeatable A/B claim.

The same session actually attempted deathmatch respawns (2 planned per player).
InitialLive/native placement and all5 lobby-only configuration refusals passed.
Only the first explicit DEBUG KILL death ran: server accepted it,issued life2
same-round respawn and native client reloaded Yard,dressed its12-piece knight
axe kit,then remained waiting for `combat spawn proof: own root life` (04:14:26).
ServerSTATUS retainedalive=false,loaded_round0,respawning=true.35s placement
timeout ended this attempt; no completed respawn sample. Evidence:
`modes-two-deaths/report.json`,RCONjournal and rawUE4SS.log. This is a real
respawn blocker, not a reason to widen the timeout. A12-A14 remain unaccepted;
HUD/control/collision and remote-life correctness are not verified.

Normal session stop produced0newcrashes,noorphans and menu_quit on bothclients.
The manualreport passes onlylimitedcrash/teardown/network/state checks while
the modes subcheck failed; neitherreport is p0 or release acceptance. Nextwork:
fix deploymentpreflight; resolve stuck-constraint memberships without fabricated
first/lastparent authority; investigate ownrootlife respawnproof circularity.

The respawn publication correction is now source-reviewed by Astra: strict
pose_context.of remains unchanged for all3 death lookup sites. for_publication
admits only exact verified live deathmatch respawn assignment/pawn/match/round,
respawn marker+wrapped order bits and authoritative full life; life130 cannot
alias oldlife2. Four root/pose writers use this context, with independent active
and publication memo buckets. Focused context25 and Sync49 checks passed,
including actual Lua root+pose beforeLOADED and absent deathreport after that
cache was populated. Existing physical readiness5uu/10deg/150ms bounds unchanged.
Native26 repeated-death proof remains pending. Server and receiver preparation
streams already accept the original current life beforeLOADED; the first deadlock
is local publication, not a reason to remove Ready's physical proof.

The deploy preflight was independently verified by root:16 WindowsPowerShell5.1
actual full-script scratch fixtures passed. Stale/malformed/unavailable compiled
server/sidecar identities fail before game writes, unchanged reuse and normal
builds succeed, relativeBinDir uses Win64-relative paths, DryRun stays read-only.
Astra approved. Real matching deploy and stale-check refusal should be recorded
at the next checkpoint; Native24's failure is retained as the before evidence.

The stuck-membership correction is now frozen and Astra-reviewed: one bounded
complete scan is shared by journal and forwarding; missing/unreadable/stale/
overflowed membership or conflicting full parent tuples refuses. Multiple
memberships with one exact parent may forward one GD with emitting-constraint
identity unavailable. Native actor/component world/owner/address/name and full
current match/round/lives are revalidated; target component must belong to victim.
Constraint actor GetOwner is not assumed. Diagnostic admission precedes optional
reads; queued send uses a wire copy retaining validation metadata on refusal.
This does not add automatic retries after IPC facade refusal (the existing queue
flush clears entries; ring-full facade queuing returns success).

Astra caught and the correction preserves the already-proved SAME-constraint
lowerarm_l->hand_l native rebind, retaining the immutable original binding/header
for parent validation. Reused-name replacement, reverse, unproved right-arm alias
and mid-read changes refuse. Candidate50ms origin binding remains nonauthoritative
for caller/marker/sever lineage. Root independently passed407 focused assertions
(Combat364+resolver43),2production parses; agent also parsedbothtestfiles. Native
callback cost and forwarding require Native26; no physical factory/event added.

Read-only Native25 geometry follow-up found original source cutting/paint Box
extent growth after proxy hits. Source1 native snapshots grow from(6,1,10) up to
(20,1,47.44),source2 up toZ21.04. Cooked DCD andGD resize caller Box then proxy
ForceDisableVertexPaint branches exit before native restore. Inst1 cid711 cut74.48
changesZ14.96971->26.30335; server8682-8685 rejects711-713 with extent difference
11.303345 matching that delta. This proves a native damage-sampling Box lifecycle
gap, not enlargement of the main static-mesh striking collider and not the
separate75degree quaternion correction. A safe restore needs a true before-native
snapshot+exact post scope; Blueprint Lua callbacks are POST. No stale/CDO restore,
paint reenablement or physical change is justified. TargetedC++hook feasibility
is under read-only investigation; global caller probe remainsOFF.

### Native26: publication recovery, second-respawn failure and ReceiveTick cause

Clean commit3b9b2770 passed strict G0:60 Lua suites,1,279 Rust tests in75 binaries,
94 production Lua parses,clippy0errors/2warnings. Normal RequireG0 deployment
rebuilt matching compiled content. Actual stale-deploy refusal independently
left the installed stamp,mod script and sidecar hashes unchanged:
`test-results/lab/integration/native26-stale-deploy-refusal.{log,json}`.
The dev push passed its independent pre-push G0. No main/tag/release action.

Native26 session:`test-results/lab/native-20261008-26`,raw:
`test-results/20261008-045042-64a926-combat_manual`. Both windows DISPLAY1;
caller/body/armor/local-damage probes OFF. Repeated-death plan5 per player
completed the first HSMP1 respawn in12,409ms in the same Live round. This
verifies publication-beforeLOADED recovery for that life. The second HSMP2
respawn timed out at35s on the physical peer hand_r position/integrated aim.
Fresh60Hz sourcecut4 remained stable;hand_r12.5-12.8cm/51degrees and
lowerarm_r27degrees while other limbs were0.1-0.3cm/<1.3degrees. Local reposes
invalidated aim; this is not evidence to widen5uu/10degree/150ms proof.
Only1 completed respawn sample; full A12-A14 remain unaccepted.

Short90s clothed axe continuation run recorded21 accepted Inside records,
18 delivery records and19 native records,not matched causal pairs.35
Inside rejection records comprise8 missing accepted parents and27 target-down
records. General damage decisions count42 refusals across both attackers:
target-down28,round-over/target-down2,attacker-down3,parried4,no-parent5.
These layers have different units and must not be added or called parity.
Clock resets0/16windows;honest acceptance0.79234973(n183),proxy rotation
mean29.76622/max94.27895degrees(n29).66 calibration samples,0exactclaimpairs.
Normal teardown0newcrashes/noorphans;manual PASS covers only limited
crash/teardown/network/state checks and does not override the modes failure.
See `checkpoint-summary.json` and `modes-five-deaths/report.json`.

Native26 frame logs show ReceiveTick hook calls/driven0 despite registration.
Astra verified the pinned UE4SS Blueprint hook stores argument2 and ignores
the native-only argument3: the empty callback ran while neutralisation never
did. Correcting the callback slot preserves the existing physical policy and
adds fresh world,pawn/life and mesh identity checks,including mesh replacement
without touching a freed retained wrapper. Source review/focused checks and
Native27 repeated respawns are the next checkpoint; physical improvement is
not yet claimed. The true before/after cutting Box observer remains a separate
default-OFF proof-only investigation; no restore or paint mutation is justified.

Final two-file Avatar correction is Astra-approved and root-independently
passes278 focused assertions and2 Lua parses. The actual callback is exercised
after simulated Blueprint writes,including dead retained mesh and reused-address
replacement refusals. Source approval is not native readiness or parity proof.

### Native27: real Blueprint hook execution, remaining grip motor hypothesis

Commit094dc6de passed strict G0 (60 Lua suites,1,279 Rust tests in75 binaries,
94Lua parses,clippy0errors/2warnings). The initial sandbox pass could not write
the .git stamp; normal RequireG0 deployment repeated the full gate and recorded
its clean pass before rebuilding matching content. Future root gates must use
the authorized Git write path to avoid this duplicate. Deployment log:
`test-results/lab/integration/native27-deploy.log`.

Session:`test-results/lab/native-20261008-27`;raw:
`test-results/20261008-052544-36d4fc-combat_manual`. Both DISPLAY1 and all costly
caller/body/armor/local-damage probes OFF. Actual callback execution is now
proved:135 logged windows,69,672calls,34,416qualified hits,36,848driver frames,
0deferred. Weighted driver mean0.824397ms,max window mean1.513ms. These counters
do NOT include callback elapsed time or renderer FPS. Before Native26:89windows,
23,494drives,0hook calls/hits,driver weighted mean0.784714ms. Different workloads
prevent a general performance comparison. Five-deaths-per-player plan again
completed only HSMP1 first respawn (12,852ms); secondHSMP2 failed physical hand
readiness at35s. Actual hooks run despite hand_r12.7cm/51.8deg and laterhand_l
171.1deg errors. No physical readiness acceptance or tolerance change.

Cloth90s:clock resets0/16windows,honest acceptance154/173,proxyrotation mean
20.03/max27.13degrees(n20),27Insideaccepted/21delivery/14native records and
55Inside rejections. Plate180s:clock resets0/56windows,honest acceptance0.94045911(n1394),
proxyrotation mean28.54/max105.84degrees(n168),468calibration samples and0exact
claimpairs. These are log-layer records rather than paired native/replay damage
results; low cloth rotation sample count and persistent plate/spawn failure
preclude a general pose/parity improvement claim. Normal teardown0newcrashes,
noorphans,limited manual PASS; modes runner FAIL/full modes acceptance NOT RUN.
Complete scalar summary:`checkpoint-summary.json`.

Astra's next read-only review identified a concrete competing writer: grips_off
zeros angular params and frees six limits but never disables linear drives.
Cooked native Willie writes both-hand SetLinearDriveParams and enables left
SetLinearPositionDrive(true,true,true). Free limits do not disable a position
motor. Fresh fault-state LinearDrive.X/Y/Z enable flags,stiffness/damping/maxforce
and exact constrained-component/bone pairing are required before a corrective
lease. A possible minimal correction disables only position/velocity enables,
preserving params/targets/mode and restoring six proven original flags on the
same exact pair; no physical change has been made. Do not treat cached
grips_desc `free` as a fresh motor readback. True Box observer implementation
remains default-OFF/proof-only and cannot authorize restore or damage.

The next bounded observers are frozen and Astra-approved. Root independently
passed Avatar317 assertions/2parses and Box106 C++ API/pair checks with mock
provider,161 Lua assertions (new observer87+frames26+AI43+input5),3Lua parses.
Actual provider/reflection/registration translation units compile /W4 /WX;
8 pinned optional export signatures exist in offline DLL mapping. Full native
DLL linking/deployment and actual pre/post argument/order/serial availability
remain pending Native28. No IPC/protocol layout or physical policy changes.

Grip capture requires HSMP_DEV=1 and HSMP_GRIP_PROBE=1 on each game. It admits
5s per-role/peer/pawn/full-life snapshots,120groups/240jointrows maximum,
180s expiry from first admitted group. Two fresh named hand fields only; driven
post-BP/post-policy pairs use exact current endpoints/owners/bones and same-pair
readback. Linear XYZ booleans,finite strengths/targets and reference/limit/angular
state are scalar observations. GRIPCALL elapsed includes capture overhead,
not renderer FPS. Every row carries HSMP_INST for unambiguous process grouping.
Current native header reads bypass the facade's retained 1s cache on failed
refresh; positive session/effective round/Mode full life and live link are
required. Missing modes never inherit PURE's optional-session/life1 defaults.

Box capture uses dev parity `boxobserve <peer> <r|l> <1..15>` and at most32
native entries with depth8. Enrollment resolves exact paths before addresses;
formal fields require reflected ObjectProperty/CPF_Parm/bounds. Scope reads
are copied only from enrolled objects. Same-role nesting aborts even beyond
budget; marks are pending-scope only and tied to the owner main Lua VM,
allowing pinned same-VM hook coroutines. Ambiguous hook submission never retries.
Object/class/function lifetime serials must be signed-positive for qualification;
zero/negative serials remain unqualified diagnostics,authority alwaysfalse.
Fresh native ready state/numeric heartbeat and module current-version Mode
views guard admission/end. Both probes default OFF; no restore,paint,damage or
factory calls were added. Native28 must establish actual availability and hand
fault state before any six-bit suppression correction.

### Native28/29: separate hand faults and unqualified damage observer

Commit20dcd0f1 passed strict G0:61 Lua suites,1,279 Rust tests/75 binaries,
95 production Lua parses,clippy0errors/2warnings. Normal RequireG0 deployment
linked the real native DLL and deployed matching compiled content; log:
`test-results/lab/integration/native28-deploy.log`. Native28 launch quickG0
FAILED one fixture because opt-in grip environment leaked through mocked getenv.
The failed evidence is retained. A fixture isolation correction passes both
ambient probe enabled/disabled runs; production capture remains default OFF.

Native28:`test-results/lab/native-20261008-28`,raw:
`test-results/20261008-061636-000e38-combat_manual`. Both windows DISPLAY1;
caller/body/armor/localdamage probes OFF, bounded grip probe ON. Two physical
respawns completed in12,860ms and12,664ms; third attempt failed35s placement.
Full modes acceptance remains NOT RUN. Normal teardown,0newcrashes/noorphans;
limited manual PASS does not override physical or modes failure.

Two distinct fault states are now observed; do not conflate their causes:
- Inst2 peer1 round2life2 hand_r13.7uu/51.3deg at06:19:32 (UE4SS4688).
  Fresh post-BP/post-policy groups49/51 show current same constraint77, held
  polearm BaseMesh->Mesh hand_r, XYZposition+velocity enabled,k7500,d0.
  This supports a competing grip motor hypothesis,not measured solver force.
- Inst1 peer2 round2life2 hand_l0.2uu/174.1deg at06:20:11 (UE4SS6209).
  Group62 is current/complete/same-pair: Fists BaseMesh->CharacterMesh0 hand_l,
  all XYZ position/velocity disabled,k50,d1,all limits free,angular0. Native
  owner peer2 instead has left constraint on held polearm with position enabled
  andk7500. Right motor hask0 by group62. A transient right lock during repose
  cannot explain sustained left174deg; suppressing linear enables alone cannot
  resolve this second state. No physical policy or tolerances changed.

Cloth90 probe-on metrics:0clock resets/17windows,honest167/183=.91256831,
rotation30.32mean/52.83max(n11),33Insideaccepted/24delivery/20native records.
Second cloth90:0resets/14windows,honest219/250=.876,rotation32.57/72.38(n35),
43Insideaccepted/31delivery/14native records. Both0exactclaimpairs. These
unpaired log layers and different workloads do not prove damage or pose parity.
GRIPCALL includes capture/log overhead,not baseline callback cost or rendererFPS.

Native29:`test-results/lab/native-20261008-29`,raw:
`test-results/20261008-064040-1913dd-combat_manual`. Probe OFF,controlled AI OFF.
Box enrollment06:44:40.752 was positively Live (06:44:14->06:46:14),but refused
without stage detail. Owner weaponstate06:45:45 proves held native axe/grip1;
remaining nativeBox/module/header/context predicate is unknown. Native28's two
refusals occurred in Lobby/LoadingReady. Across both sessions0BOXOBSstarted,
0PAIR: actual native pre/post scope,serial availability and Box lifecycle remain
UNVERIFIED. Native29 normal QuitGame06:48:40,0newcrashes/noorphans. Ignored scalar
`checkpoint-summary.json` files retain metrics and limitations for both sessions.

Cooked native solo initialization uses RthenL. Remote omitted-L appearance
instead synthesizes Fists and calls valid left setup,which clears offhand
constraints and R Two Handed Grip; own empty-L kit does not call that branch.
This is a proved topology mutation difference,not proof of174deg causation.
Universal L-first reorder would alter legitimate dual wielding and is not
justified. Next work is stage-coded Box refusal diagnostics and bounded copies
of actual hand decoded/final/prior/current quaternions and body simulation state,
followed by Astra review and real paired evidence. Main/release remain blocked.

### Native30: contact-selected Box enrollment timing

Clean bd4629e9 strictG0 passed61 Lua suites,1,279 Rust tests/75 binaries,
95parses,clippy0errors/2warnings. Normal matching native/Rust/mod deployment
reused that stamp. Independently317->318 Avatar assertions pass with both
ambient probe settings,177 Box/control assertions and5changedfileparses pass.
Box stage diagnostics and unavailable-axis formatting corrections are Astra
approved,with no physical policy change.

Session:`test-results/lab/native-20261008-30`,raw:
`test-results/20261008-070155-557538-combat_manual`. ProbeOFF,bothDISPLAY1,
clothaxe120s with actualAI control.0clock resets/21windows,honest256/304=.84210526,
proxyrotation35.63mean/96.58max(n27),114calibration samples,82Insideaccepted/
41delivery/23native and85rejected records,0exactpairs. Actual native hits and
server damage were observed; these log layers do not certify solo parity.
Normal shutdown0newcrashes/noorphans,limitedmanualPASS6rounds.

Six scoped Box refusals are now specifically held_box;0started/0PAIR. Exact
review shows commands were before the same round's first qualifying blade
contact: round2 refusals07:03:54.407/.447 precede Blade/BoxDCD55.161; round3
refusals07:04:24.766/.784 precede AI takeover25.314/.326. Native weapon field
is contact-selected state,not guaranteed by equip or another round's hits.
Pinned SDK ModularWeaponBP.hpp91 declares UBoxComponent* Hit Box Collision;
Axe inherits it;Willie has no separate such field. Cooked only assignment12926
selects a non-Tip childBox during Collision Hit,not BeginPlay/construction.
Native30 first Grip_0 DCD31.901 has nilBox,Blade31.921 passes its ownBox,later
Grip_0 uses rememberedBox,and32.265 can pass a stuck-constraintBox. Existing
owner/membership refusal must remain; no arbitraryBox fallback is justified.
Next native enrollment must follow same-life/currentweapon actualBox contact.

The small read-only weaponstate extension is frozen/Astra-approved and root
independently passes56 changed-suite assertions/4parses. It copies exactly4
strictly typed current grip/offhand scalars and2current hand-field constraint
identities,with fresh native header+before/after fullworld/actor/currentMesh
scope. Missing/new-proof failure leaves ordinary weaponstate available;
zero/false remain actual values. No loadoutorder or physics setters were added.
It still needs matching deployment and actual owner/proxy captures.

The HAND pipeline observer is now frozen for final Astra review. Root independent
checks pass343 Avatar assertions with ambientDEV/GRIP/HAND all1 and all0,plus
2parses. Explicit HSMP_DEV=1 AND HSMP_HAND_PIPELINE_PROBE=1 admits at most60
scalar capturegroups in180s,5s per exactpeer/pawn/world/life/cut. It deep-copies
actual decoded/finalaim/prioraim and existing returned current c7 values for
both lowerarms/hands,usingupperarms only as relative-quaternion anchors. Source
sequence/cut/frame/fullscope and current mesh stamp qualify prior availability.
Only4bones get bounded post-driver simulation/angular-velocity readbacks;
current quaternions retain their pre-driver-returned phase label. No extra
socket-quaternion getter,joint-angle API,physics setter or driver policy change.
Unknown/missing/nonfinite/stale data stays unavailable. Native execution and
causal interpretation remain pending Native31.
Final Astra HAND source review approved the frozen two-file diagnostic for matching Native31; native verification remains pending.

### Native31: runtime grip mismatch and diagnostic coverage gaps

Clean fbcb5dd3 strictG0 passed61Lua suites,1,279Rust tests/75binaries,
95parses,clippy0errors/2warnings. Matching fullnative/Rust/mod deployment
completed. Session:`test-results/lab/native-20261008-31`,raw:
`test-results/20261008-072148-a909f6-combat_manual`;bothDISPLAY1,HAND+GRIP
boundedopt-insON,costlycaller/body/armor/localdamageOFF.

Round1 failed despite both native placements verifying: HSMP2 proxy peer1
upperarm_l~87.7deg,lowerarm_l~178.4,hand_l~24.6 withfresh60/s source persisted
through reposes. Detailed capture correctly refused because Mode stillheld
round0/life0 until result/load_failed07:23:16.734,while pending source/display
wasround1/life1. This is physical readiness failure,notfailed native placement.
Round2 later reachedLive07:23:33.502 and produced qualifiedhealthy HAND:
23samples/bone/instance through07:25:33,maxhandworlderrors(inst1)L.229/R.574deg,
(inst2)L.206/R.944deg,decoded->aim max<=.357deg,allfourbones simtrue,norm1.
These healthy samples do NOT explain the failed initial preparation.

Two exact WPNHAND scopes in both processes (07:24:38,match8750572547180498/r2/
life1 and07:26:41,match4601685102968841/r1/life1) independently show owners
currentR/L14/0,twohand/offhand true;proxies3/0,bothfalse. Current hand-field IDs
join actual native endpoints: ownerL->samepolearm asR;proxyL->Fists. Allfour
actualpolearmactors have defaultR14/L3. CurrentR3 is runtime state,notdifferent
weapon defaults. Firstscope persists24qualified owner/proxy endpoint snapshots
perinstance (proxiesbothpost-BP/post-policy,samepairtrue). Cooked native offhand
loss branch can set CurrentR toWeaponR's defaultL thenbreakoffhand; whichtrigger
occurred remains unobserved. No flags or loadoutorder were forced.

Five-deaths-per-player subcheck completed3 respawns:12,862/18,731/12,701ms,
then35sphysicaltimeout. One completedplacement exceeds15s target. Laterhand_l
171.5–174.5deg fault recurred07:27:35–54,afterglobal180s HAND/GRIP captureexpiry.
Full modesacceptanceNOTRUN. Thus neither fault interval has qualified HAND
causeevidence; onlyhealthyLive capture is verified. Next diagnostic work must
explicitly cover pending-source versus actualMode mismatch without claiming
authority,and choose a bounded window that overlaps the actual fault.

Ashortbounded12-attempt Box sequence07:30:26–35 firstrefusedheld_box,then both
sourcesbecame valid07:30:28.957/.032 and native enrollment refused formal
parametersunavailable.0started/0PAIR;exactparameterreflection now needs provider
investigation,withallnativegatespreserved. Cloth120recipe didnotcomplete before
owner deadline; no completedrecipe summary exists and no fullrecipe metrics
are claimed. NormalQuitGame07:31:08,0newcrashes/noorphans;limitedmanualPASSdoes
not overrideinitial/readiness/modes/observerfailures. Ignoredscalar summary:
`checkpoint-summary.json`. No main/PR/tag/release action; shipping remainsNO.
Native31 provider follow-up: cooked Get Damage has1,070 childproperties including Blueprint locals,while formal() rejects>512; inputs remain valid8-byteObjectProperty/Parm bounds. Preserved22Boxrows comprise6held_box and16formal-enrollment refusals,0starts/PAIR. A complete bounded enumeration with unchanged input checks needs a production-helper regression and native retry.
Owner sleep instruction conditionally authorizes PR/main merge and coordinated release/update only after intended gameplay and all required acceptance gates genuinely pass. That condition remains unmet; continue dev checkpoints and native debugging without further permission requests.

### Native32 preparation: complete reflection and pending hand observations

Committed `c6c75483` enumerates the entire bounded native function property chain
instead of refusing Get Damage's 1,070 descriptors at the old 512 cutoff. Exact
input-name uniqueness, ObjectProperty/Parm flags, by-value pointer and parameter
buffer bounds remain mandatory. The production helper and Lua API observer
fixtures pass 140 assertions across two native tests; changed provider sources
compile with warnings treated as errors. Real enrollment and native callback
ordering remain unverified until matching deployment.

Committed `9a25ce65` records pending hand drive observations only under fresh,
exact positive source/display and Session spawn assignments. It records the
actual Mode tuple, including zero/mismatching values, as unqualified and
non-authoritative rather than inventing Mode readiness. Strict Live and GRIP
qualification is unchanged. Focused Avatar fixtures pass 362 assertions with
ambient diagnostic flags both enabled and disabled; Astra approved the source.

The remote empty-left correction is being finalized separately. Native31 proved
that a generated left Fists actor displaced the polearm offhand arrangement on
proxies. Review requires native-null proof, complete world/pawn/mesh/life/spawn
scope, separate armour/hand success markers, and no retry of uncertain native
mass accounting. Fresh native acceptance remains pending; these offline checks
do not establish fair spawning, native damage parity or release readiness.

The six-file loadout/fixture correction is frozen: root independently passes
388 assertions (helper 47, production loadout 180, Kit 161) and six Lua parses.
Positive native nullptr leaves L genuinely empty without entering Left setup.
An actual unwanted L uses the exported native None cleanup literal; only its
completed cleanup can authorize guarded reuse of the exact retained R actor.
Scope includes Mesh address and FName plus actual positive Playback life and
Session spawn assignment. A failed hand operation clears previous hand success
while retaining successful armour, so recovery retries hands only. Kit's
compatible first return now has a second preflight/attempted/complete stage;
the production adapter forwards both. Attempted failures and uncertain
post-call readbacks latch against further accounting on the same full scope.
Fixtures model weight subtraction before a throw, preflight refusal followed
by success, and an armour-only change with an unchanged hand key. Explicit
left fists, dual weapons, shields and world-held items retain their paths;
absent-right behaviour is unchanged and remains a separate parity limitation.
Final Astra source and production-fixture review approved the clean checkpoint.
Native grip binding, mass stability and physical readiness remain pending the
matching native32 run.

### Native32: empty-left verified, physical and replay gaps remain

Clean `ea9ecf738f1e39d3b0447be796abe1afdbcd7797` passes strict G0
(62 Lua suites, 96 parses, 1,279 Rust tests / 75 binaries, clippy no errors /
two warnings), with a matching full native/Rust/mod deployment. Session:
`test-results/lab/native-20261008-32`; raw:
`test-results/20261008-084604-53d186-combat_manual`. Both windows were on
DISPLAY1 at x=-1760/-880, 880x527. HAND/GRIP were bounded opt-ins; expensive
caller/body/armour/localdamage probes stayed off.

Remote L is now genuinely None: initial dressing reports R=ok L=none and
weapon-state capture no longer finds a generated Fists L. However proxies
still resolve R14 to R3, with twohand/offhand false and no valid current L
constraint. Initial owner1 also loses offhand while retaining R14; owner2
retains it. Round2 owners have R14 and both flags true, proxies remain R3.
Cooked native Tick requires Arm L Tonus>0.25 OR an existing offhand attachment
to retain the two-handed default; Avatar neutralisation zeros that tone.
Source controls carry tone/grip data but Avatar does not consume them. This is
a semantic mismatch mechanism, not proof that restoring one scalar fixes the
limb fault. Both proxies share the mismatch, yet only one initially stalls.

Initial round1 failed physical readiness after successful placement; round2
reached Live. Pending HAND now honestly captures the failed initial interval:
source/display round1/life1 and current Session spawn256, actual Mode round0 /
life0, qualification=false and authority=false. Groups7–9 show lowerarm_l
returned-world versus aim error175.26–175.39 degrees, while decoded-to-aim
differs only0.044–0.141 degrees. Astra recomputed group7's existing servo:
with dt approximately1/60 and correction cap900, post-driver angular velocity
matches the intended output within about0.00008 degrees/s. That proves this
sample's write/readback, not next-step integration or physical body/frame
equivalence. Next diagnostic must observe exact limb constraints and fresh
PhysicalAnimation state, preserving all scope checks and existing policy.

Repeated-death subcheck completed four respawns (14,840 / 11,888 / 12,299 /
12,903ms), then failed its35-second physical readiness wait. Full modes,
HUD/control acceptance and release gates remain NOT RUN. Two completed cloth
120s recipes report honest acceptance250/291=0.8591 and264/309=0.8544;
rotation means25.01 /29.49deg, maxima46.72 /105.35deg (27 /45 samples), with
zero clock resets across22 windows each. The plate90s recipe reports
484/504=0.9603 acceptance; rotation mean32.71deg /max121.84deg (123 samples),
clock-reset rate mean0.597/max11.950 across20 windows. These fail agreed
combat/pose bounds; no recipe certifies native damage parity.

Actual wire evidence confirms an ongoing Inside geometry loss: parent56
contains its complete13-field cutting Box frame, flags224, ordinal12 and
collider10. Children58–60 retain collider10 but transmit all13 Box values as
zero, despite observed current constraint Box frames with extents6/1/10.
Those child positions/rotations change, so the parent frame cannot substitute.
Native initial Inside passes None; ongoing Inside passes its constraint Box.
Owner replay still changes injury on some boxed children, so this is a proven
lost native input/area branch, not a blanket claim of no damage. Current
schema/server contracts reject continuation Box representation; coordinated
changes require exact constraint-owned Box and callback proof first.

The formal reader correction reaches actual native enrollment twice:
08:58:33.890 and09:01:06.676 (inst1 peer2/r). Both captures stop on playback
qualification after roughly46/58ms, with zero qualified pairs. There is no
new formal-parameter refusal, but serial/callback ordering/extent qualification
remain unproved. Split the generic playback refusal into exact observed
failure fields without loosening its250ms/current-life guard.

Normal menu QuitGame09:03:36, zero new crashes/orphans/forced game kills.
Limited combat_manual DoD12 PASS does not override the failures above. No
main/PR/tag/release action. Native33 preparation is read-only bounded limb/PA
and playback-refusal diagnostics; no guessed remap, forced grip flags,
unverified PhysicsObject/GetCurrentJointAngles, or wider damage change.

### Native33 diagnostic checkpoint preparation

Committed `b14ce975` preserves Box enrollment predicates while replacing generic
playback refusals with exact missing/time/pawn/match/round/life reasons. At most
32 bounded text rows report independently available actual/expected fields and
signed age. Astra approved both source files; root independently passes212
assertions and two parses. Neither46ms nor58ms Native32 stop can be attributed
to a specific predicate from the old logs, so no reason is guessed.

The four-file limb diagnostic is frozen with430 root-verified assertions
(Avatar386/helper44) and four parses. It requires HSMP_DEV=1 and
HSMP_LIMB_BURST_PROBE=1, attempts at most24 admissions, and records one
three-drive burst of at most12 phase rows over500ms. It uses current verified
constraint accessors14/15/16 and their exact endpoints, assets, limits and
drive settings, plus fresh PhysicalAnimation/Phys Anim Array bindings and
strength compared with scalar IDs captured at fresh body construction.
It logs actual computed driver parameters and separately available native
ReceiveTick DeltaSeconds, without claiming a physics boundary or render FPS.
Unavailable APIs/fields stay unavailable; optional read/log failures cannot
interrupt the normal driver. Fresh PC world precedes old object access, and
mesh/world changes during a read prevent writing through earlier wrappers.
The default path adds no diagnostic parameter allocations, setters or policy
change. A failed or throwing current-scope check latches the burst closed;
getter/accessor and PA guards prevent later reads even if a synthetic scope
appears healthy again. False/throw/recovery regressions prove zero subsequent
old reads and no mixed row. Final Astra review approved the four-file source;
native output usefulness and rigid-body authority remain unproved until the
matching Native33 capture.

### Native33 and Native34 measured results

Native33 uses clean deployed `2b84eaef` (raw
`test-results/20261008-094741-428e53-combat_manual`), with HAND and LIMB
diagnostics enabled and GRIP disabled. The initial eleven limb rows identify
current left constraints/accessors and the current PhysicalAnimation binding;
they do not identify a stale component. Lowerarm rotation remains approximately
178 degrees across three pre-drive samples. Angular velocity writes read back
successfully and remain unchanged through Blueprint POST and the first policy
POST callbacks. Their attenuation before the following frame remains unexplained.
The measured native tick steps include 78.83 and 94.07 milliseconds during the
burst; this is not an uninstrumented FPS result. Target getters were incorrectly
reported unavailable because the diagnostic adapter expected nested outputs,
whereas pinned UE4SS returns direct struct fields. Correct that reader only.

Native33 cloth: honest acceptance118/131 (.900763), rotation mean32.074/max98.948
degrees over30 samples, clock-reset rate mean1.3262/max12 over18 windows.
Both native Box enrollments stop on playback age285/399 milliseconds with exact
pawn/match/round/life tuples; zero qualified pairs. The later LIMBBURST diagnostic
timestamp is not `shown.at` and cannot prove a newer applied pose was published
late. The earlier publication inference is withdrawn.

Native34 runs the same clean deployed build with HAND/GRIP/LIMB all disabled
(raw `test-results/20261008-100213-f4a58d-combat_manual`, session
`test-results/lab/native-20261008-34`). Initial and axe rounds1/2/3 obtain both
six-limb settling proofs before releasing input; no load_failed observed. The
sampled avatar update cost is at most1.102 milliseconds with zero budget
deferrals; this does not establish render FPS. Cloth acceptance242/245=.987755,
rotation mean24.084/max49.679 degrees over20 samples, clock-reset rate
mean.425931/max11.9261 over28 windows. These rotation values measure claim
reconstruction, not limb tracking; bounds/sample counts still fail. Unequal
sample windows prevent a causal comparison with Native33.
The last active window reports297 avatar drives and approximately59 pose
samples per second, with no budget deferrals. Its22-body servo tracking summary
is mean1.41/max38.26 degrees (worst foot_r), distinct from claim reconstruction.
These counters measure update activity, not rendered FPS.

Native34 Box starts10:06:47.954/.981 and stops10:06:48.001/.018 local time,
ages276/279 milliseconds, exact tuples, zero qualified pairs. Optional limb
diagnostics are therefore not the sole cause. Inst2's clock-reset event at
10:06:47.996 has actual drive local_ms251772 and prior clk_at251516.102;
the refused bus timestamp251516 matches that prior baseline. This supports a
frame gap/publication window, but does not measure completed shown assignment,
snapshot cost, native enrollment cost or Lua hook installation cost. Measure
those separately before changing publication. Keep the250ms guard and original
applied timestamps; never refresh a stale pose's timestamp.

Both sessions stop through normal menu quit with no new crashes or orphaned
games. Mandatory prepush G0 and matching normal deployment passed for `2b84eaef`;
limited harness PASS does not establish combat parity. No PR/main/tag/release.

Next diagnostic checkpoint: the limb reader accepts finite direct struct
outputs (pinned UE4SS's actual format), retains nested adapter compatibility,
and rejects ambiguous/nonfinite values. Root independently passes65 helper
assertions and two parses; owner also passes386 unchanged Avatar assertions.
Astra approves the two-file correction. Native values remain unverified until
a matching capture. Parity setup timing brackets use the same process clock,
report only existing playback timestamp/generation reads, and preserve every
qualification predicate. Command-enter measures entry into the command function,
not command queue arrival. Measurements cannot by themselves establish
Avatar shown assignment/publication ordering.
Root independently passes217 Box observer plus65 limb helper assertions and
five parses. Timing is bounded to32 rows per observer lifetime; optional clock,
formatting and log failures cannot alter enrollment, marking or refresh control.
Astra approves both frozen code changes; next measure Native35 on the matching
clean deployed checkpoint. No gameplay publication or physics policy changed.

### Native35: repeated native enrollment stall isolated

Clean deployed `f487492a`, full prepush G0 PASS and normal build/deploy; raw
`test-results/20261008-102359-f81c87-combat_manual`, session
`test-results/lab/native-20261008-35`. GRIP/HAND off, one bounded LIMB burst per
process enabled. All69 native joint snapshots across23 rows read both targets
available as[0,0,0]; this verifies direct-struct adapter behavior, not a default.
Inst2 has11 pending-Mode rows; inst1 has12 exact-Mode rows. All authority=false.
Fresh PA binding is complete23/23. Native post-driver angular velocity persists
through BP/policy callbacks; PA strength rises in BP and returns0 at policy POST.
Long burst tick steps are instrumented data, not normal render performance.

At10:25:34.629/.676 local time, inst1/2 enrollment costs are snapshot1/2ms,
native begin226/228ms, Lua hook installation1/1ms. First refresh takes1ms each,
then refuses playback ages287/298ms with exact identity tuples. Bus generation
advances1946→1948 and sample advances~50ms, but remains from before native begin.
A warmed inst2 enrollment at10:26:47.893 repeats225ms native begin, snapshot1ms,
install0ms, refresh0ms, refusalage290ms. Thus expensive native begin repeats
after hooks exist and dominates the stale interval; this is not solely initial
hook registration, the Lua snapshot, or optional limb diagnostic cost. Zero
qualified pairs. Native substage costs still need proof before choosing metadata
reuse or staged setup. Do not freshen old applied timestamps or widen250ms.

Cloth metrics:258/280=.921429 honest acceptance; claim reconstruction rotation
mean35.159/max78.753 degrees over31 samples; clock-reset rate mean.566332/max
11.893 over21 windows. Combat acceptance is not achieved. Normal menu quit both
clients; zero new crashes/orphans/forced kills. No PR/main/tag/release.

Offline cooked cut-path audit is ignored local evidence at
`test-results/dev-feature-checks/native35-cut-eligibility-audit.json`:
speculative proxy severing/painting is intentionally suppressed, while gated
local pawns are ungated for approved owner replay. DCD computes actual component
protection/density tags; soft-spot equivalence still needs source/owner trace
provenance. Native marker wear in Constraint_Weapon_Stuck_BP:Dismemberment Check
precedes sever eligibility and is not established through replay. Next observe
that exact8-byte Damage input and current parent/marker wear before/after; no
guessed topology, factory or sever call. Existing owner-confirmed distal injury
mirroring does not provide detached-limb/armour transport or complete force
history. Body/limb/dismemberment100% remains unmet.

Spawn/HUD follow-up: ignored `native35-spawn-discriminator.json` proposes bounded
source/proxy reference transforms, exact asset-template joint frames and current
limits/projection under fresh full-life identities. Template DefaultInstance is
not a live runtime bind frame; matching templates cannot prove physics versus
animation-blend causality. No raw per-bone physics pose API/offset is invented.
Opponent HUD bars follow roster rows, not actor-attached widgets. Current
readiness proves life-scoped vitals and named actor collision configuration;
it does not prove rendered bars, per-bone collision response or necessarily
the exact driven-Mesh identity. A future small evidence capture should compare
fresh roster/vitals/displayed-pawn tuples with current HUD host/row/bar
validity, visibility and fill. Keep widget state distinct from rendered pixels.

Native36 preparation: frozen five-file C++ diagnostic adds at most16 scalar
GetTickCount64 stage intervals during explicit developer enrollment, with32
bounded print emissions per process. No engine reads or writes are added.
Copied stage costs and failure details precede Lua callbacks; all native state
and return construction completes before raw-global protected print. Reentrant
print->stop remains stopped, and throwing print cannot change activation.
No stage tables are allocated in the existing hot status path. Native provider
and probe compile /W4 /WX;121 focused checks pass, with root independent CTest
PASS. Astra approves the source. No caching, qualification, publication or
physics change; matching Native36 stages still needed for cost attribution.

### Native36: identity lookup dominates enrollment

Clean deployed `b20d339d`, full G0/dev push/normal deployment passed; raw
`test-results/20261008-104829-76521c-combat_manual`, session
`test-results/lab/native-20261008-36`, all GRIP/HAND/LIMB flags off. Initial
polearm spawn waits approximately31 seconds for the left arm before settling
and releasing control; not a quick/reliable spawn acceptance.

Inst1 exact native enrollment at10:51:06.474:14 stages, no overflow,
scope_identity203ms, DCD function15ms, GD function32ms; initialization,
properties, all formal scans, owner function/return and current-scope checks
measure0ms at GetTickCount64 resolution. Total250ms; Lua bracket239ms, then
first playback refusal age300ms with exact tuple. Native wall and process-clock
brackets are distinct clocks; do not claim submillisecond zero work. Inst2
refuses held_box in the same control attempt; only one native stage capture.
The bridge uses StaticFindObject_InternalSlow. Identity lookup dominates this
capture, not formal enumeration or hook registration. Zero qualified pairs.

Cloth:218/245=.889796 acceptance; claim rotation mean25.796/max50.882 degrees
over20 samples; zero clock resets over24 windows. Combat bounds remain unmet.
Normal menu quit both, zero new crashes/orphans/forced kills.

Next candidate is separated preparation and activation: costly lookup/metadata
setup with observation disabled, then a bounded wait for an actually newer
current playback sample and fast full native identity/ownership revalidation
before activation. Preserve250ms freshness, tuple and serial-zero qualification
rules. A positive-serial-only lookup cache may not help the actual newly spawned
objects; do not assume cache hits or relax identity proof. No implementation or
release claim yet. IDA plugin is installed, no GUI process currently open and
no IDA tools are exposed to this session; no shipping disassembly was performed.

### Native37 preparation: disabled setup, fresh activation

The seven-file setup fix is frozen and Astra approved. Native begin remains
available; prepare performs costly path/layout proof and hook submission with
observation disabled, then issues one VM-owned single-use token with250ms
GetTickCount64 expiry. Lua installs hooks while inactive and waits at most250ms
for an original applied timestamp strictly newer than its post-install cutoff.
Pending age rows are explicitly ineligible and still audit the full current
tuple, grip, held Box, module membership, world and session. Identity/life/Box
changes stop setup. Activation re-resolves current native identities and freshly
rebuilds property/formal scalar descriptors without slow path lookup before
enabling observation. Existing250ms freshness and serial-zero qualification
remain unchanged; no old timestamps are refreshed.

World drop during prepare/install/activation cancels setup. Native in-flight
guards survive stop and prevent nested prepare from replacing metadata being
read by an outer provider. DEV/game-thread refusal precedes the non-atomic
guard; no engine work follows optional logging. Owner and root independently
compile current provider/probe /W4 /WX and pass141 native assertions; root
passes239 observer Lua assertions and three parses. Actual activation duration,
callback ordering, extent samples and qualified pairs remain unproved until
the matching Native37 capture. No cache, physical policy, sever or release change.

IDA feasibility attempt used installed idat9.1 and a real independent2.2GB copy
under ignored `workspace/ida/native-evidence-20261008/name-query-986bdca44322`.
No-save directives and name-only bounded IDC were verified against installed
references. Headless startup did not reach the query/log within120 seconds;
only owned PID23568 was stopped, no IDA process remains. Original and clone
SHA256 are unchanged. Runtime/license/name results are unavailable, not zero
matches; no binary address, body-pose API or allocator proof was obtained.

### Native37: fresh activation works, native admission fails next

Clean deployed `4c9dce85`, full G0/dev push/normal deployment passed; raw
`test-results/20261008-113030-41978f-combat_manual`, session
`test-results/lab/native-20261008-37`, all GRIP/HAND/LIMB off. Inst1 id6 prepares
at11:33:30.339 and activates11:33:30.427: native preparation218ms, post-install
cutoff160522, actual original playback160559 (37ms newer), age50ms. Activation
160610→160610 is0ms at the available process-clock resolution, not zero work.
First active refresh remains fresh at49ms. Thus the setup fix reaches real
fresh activation without changing250ms or refreshing an old timestamp.

At11:33:30.475, capture finishes `game thread unavailable`, entries0/unpaired0/
discarded0; no qualified pairs. The generic observer aborts before target
filtering when hsmp_native_caller_thread_ok returns0. That Rust API also returns0
for native mutex contention, poisoning or unknown thread. Therefore this event
does not prove a foreign animation callback or identify a target function.
Next distinguish a proven Win32 thread identity from generic admission failure:
confirmed different-thread callbacks may be ignored/count-only with no
context/frame/state/object reads; same-thread unavailable retains fail-closed
handling. Coverage gaps remain explicit; no skipped-target completeness claim.

Cloth234/280=.835714 acceptance; claim rotation mean27.471/max61.313 degrees
over40 samples; zero clock resets over20 windows. Combat acceptance still fails.
Both normal menu quit; zero new crashes/orphans/forced kills. No PR/main/release.

Native38 preparation: observer records a positive Win32 thread ID only after
successful existing native admission. Different proven callback threads increment
a saturating atomic process total and return before provider, frame or state
reads; skipped targets remain unknown. Same-thread unavailable and unset-thread
identity retain fail-closed aborts with distinct reasons. Status counters are
process totals; current Lua completion logging prints reasons, not these totals.
Astra approved the frozen three-file change. Root rebuilt current targets and
verified149 focused C++ checks; provider compilation also passed. This is a
diagnostic classification change, not native callback coverage or combat proof.

### Native38: repeated initial spawn refusal, no observer capture

Clean deployed e4cd0443 passed full G0 and normal deployment; all GRIP/HAND/LIMB
off. Raw test-results/20261008-115244-bae837-combat_manual, session
test-results/lab/native-20261008-38. Initial polearm round1 and round2 each ended
load_failed (11:54:14 and11:55:06). Round1 only one proxy settled; round2 only
the opposite proxy settled. Round2 peer2 tracking windows report right-hand
max10.65–11.07uu and153.38–154.36deg, with367–397 capped operations; peer1
windows report max0.27–0.32uu and1.72–1.88deg. These are window maxima,
not per-frame samples or render FPS. Ready remained unavailable during the
bounded experiment-start wait; no cloth recipe or Box capture ran.

Round3 both proxies settled11:55:19; normal stop was requested10:55:24UTC,
with input release logged11:55:23.406/.433 just before teardown. Thus this is
repeated initial spawn failure followed by late recovery, not permanent failure
to reach Live. Zero new crashes/orphans/forced kills, both menu_quit. Harness
combat_manual PASS covers its limited teardown checks, not playable acceptance.
Prioritize exact source/proxy runtime constraint comparison without loosening
spawn readiness. Thread-classification native result remains unavailable.

Native39 admission preparation: Rust exports a single-attempt result distinguishing
allowed, would_block, mutex_poisoned, native_poisoned, frame_thread_unset,
wrong_thread and panic. Existing boolean remains true only for allowed. Production
Box consumes that same attempt and retains all refusals before frame/key/snapshot
access. Completion logging validates uint32 process counters and whitelists the
explicitly available copied result, with skipped-target coverage unknown. Permanent
poison can prevent later status retrieval under the unchanged control guard; do
not infer its reason from unavailable status. Root rebuilt173 C++ checks/provider,
four targeted Rust tests and241 Lua checks/parse passed; Astra approved the frozen
eight-file admission/logging delta. Runtime reason remains to be measured.

Native39 joint configuration preparation: default off, requires HSMP_DEV=1 and
HSMP_JOINT_PROFILE_PROBE=1. Three bounded admissions, one descriptive pair per
process for actual right constraints10/11/12, with returned accessor owner/index/
endpoints, current limits/strength/softness/projection and four native hand fields.
Local source and proxy are different peers; only counterpart peer rows across
clients support source/proxy comparison. Shared world/gen/match/round required;
legitimate separate lives retained, native successful fractional pose timestamps
remain distinct from admission and actual row observation timestamps. Right-only
coverage, no temporal pair, solver or rigid-body authority claim. Scope loss/throw
latches before subsequent optional reads; independent PC-world-first writer check
after library lookup prevents touching or writing old proxy Mesh. Root verified
31 helper +403 Avatar focused checks and four parses. Verified spawn assignment
must equal the current Session order; an old same-life/pawn placement cannot
be relabeled with a different assignment. No setters, readiness,
physics tolerances or damage authority changes. Astra approved the final freeze.

Native37 cloth rejection audit corrects earlier attribution: all46 general
rejections were target_down40/attacker_down6 after authoritative elimination in
RoundOver; all95 Inside rejects were also after elimination. General and Inside
streams overlap (61 accepted and27 rejected keys), so do not add totals. Six
eliminations across five cloth rounds were cause6/reason2 AI-yield surrender;
the native bodies remained alive. Server down denotes gameplay participation,
not native consciousness or recoverable Downed. The .835714 aggregate still fails
the unchanged bound but does not establish armour damage loss. Claim rotation
remains separate from limb tracking. Complete native damage/topology still unproved.

### Native39: native pairs available, configuration refusal unresolved

Clean fd8c2d40 passed full G0/dev push/normal deployment. Raw
test-results/20261008-122100-44c909-combat_manual, session native-20261008-39.
Only JOINT_PROFILE enabled; GRIP/HAND/LIMB off. Initial spawn reached readiness.
Both profile captures refused attempt1 with generic scope_or_capture_unavailable;
no configuration rows or counterpart pairs. The adapter discarded the helper's
second-return refusal reason, so exact cause is unavailable. No parity claim.

Native damage capture produced12 rows on inst2 (7DCD/5GD). Ten had available
PRE/POST, LuaInside and one POST mark, all extents6/1/10→6/1/10. Two lacked
formal parameters. Four readable nested GD/DCD sequences plus one unavailable
nested sequence. All lifetime_available/qualified false: zero class serials on
world/Mesh and readable function classes. No serial allocation or restoration
authority. Last DCD11 spans16ms, others0 at clock resolution. Native thread totals
were not saved because owner_mode stop preceded budget completion. A narrow tail
watcher missed activation and re-enrolled one active capture; its ignored replacement
uses incremental log bytes and stops issuing controls on the first new activation.

Cloth aggregate170/230=.739130; claim Box rotation mean42.346/max96.211degrees,
7samples, zero clock resets over20 windows. Full typed262 verdicts differ from
that denominator:170accepted,75target_down,10attacker_down,2round-over,4parried,
1body_strike_miss. Owner outcomes114changed/52no-observed-change/4expired;
do not claim all170calls completed. Both menu_quit, zero new crashes/orphans/
forced kills. Limited combat_manual PASS is not complete acceptance. No release.

Native40 checkpoint preparation: new-source-only participation veto requires
fresh direct native header/link/roster and exact raw Session/Mode/full-life rows,
explicit boolean alive=false, and final unchanged version/header checks. Unknown
authority preserves existing path. life_for, queued origin/Inside, trades, sends,
replay and server gates unchanged. Root verified17helper+368Combat checks and
four parses; Astra approved. Profile refusal now saves sanitized120-character
actual reason, failed stage and total capture-entry→refusal elapsed time, no added
UE reads or loosened scope; root35helper+410Avatar/four parses, Astra approved.
Parity STOP copies native scalars, finishes stop/protected drain, clears state,
then protected optional logs; reentry guard prevents repeated cleanup. Root252
checks/two parses, Astra approved. All source frozen for native verification.

084fd52e dev push blocked solely by G0 state_files: opaque dot-prefixed profile
stage labels were classified as new filenames. No deployment/push occurred.
Other G0 checks passed. Renamed stage separators to colons, matching fixtures
only; direct state-files passes, root35helper+410Avatar/three parses pass, Astra
approved. No lint exception, I/O, scope or gameplay changes.

Native39 replay status7 correction: this status is a below-watermark/dedup expiry,
not a TTL. Four accepted positive IDs arrive out of order before surrender in
same live match/round/life: attacker2→victim1 id2 after3; attacker1→victim2 ids19
after21,20after22,33after35. S2G ingress already inverted; server accept/forward
logs show same order. Source emissions ordered, ages126/8/6/123ms. Session Live,
both Mode alive, no world drop. replay_attempts refuses before native callback.
Thus these four are measured pre-terminal execution loss, not expected transition
expiry. Next bounded at-most-once reorder design must preserve replay protection.

### Native40: terminal claim admission verified, foreign callback counts measured

Clean b2287d6a full G0 retry/dev push/normal deploy passed; raw
test-results/20261008-125147-5a5b8f-combat_manual, session native-20261008-40.
Only JOINT_PROFILE on, GRIP/HAND/LIMB off. New source claim target_down veto
actually logs round2 (#1–3). Cloth239/243=.983539, compared to previous170/230;
aggregate improvement supports the fix, not complete native body/gear parity.
Claim Box rotation mean34.483/max73.156degrees,21samples, remains over target;
zero clock resets22windows. Physical limb rotation is a separate measurement.

Profile emits no pairs: inst1 proxy:UserConstraint_11:accessor_before, total148ms;
inst2 proxy:asset_after, total191ms. Both actual reason scope changed. These
are total capture durations, not getter costs; failed current predicate remains
unknown. Do not infer getter unavailability or source/proxy equality. Next narrow
diagnostic should report which current proof failed, not expand native reads.

Native observer inst1 starts12:53:54.585 after227ms preparation and fresh51ms
actual playback; capture budget completes12:53:55.165, entries32, unpaired1,
discarded0. Proven OS TID28820, foreign_callbacks_process_total12580,
same_thread_unavailable_process_total0, unknown_thread_callbacks_process_total0.
Foreign targets remain unknown; no worker callback coverage claim. Actual32rows
are saved, scalar extent/lifetime summary pending. This run proves different-thread
callbacks are common and excluded without same-thread failure; it does not identify
Native37's exact rejected event. Byte-cursor watcher stops first activation and
does not re-enroll an active capture. Completion logging saves counters here;
no STOP status row was expected after budget had already cleared active state.

Both normal menu_quit, zero new crashes/orphans/forced kills; owner exit0. No
release/main/PR. User asked percentage/release: no defensible overall completion
percentage; zero of two required full release-acceptance runs completed. Latest
98.35% is claim acceptance only. Quiet existing heartbeat continues native-first
iteration; current next work is exact profile predicate and bounded replay ordering.

Native40 final corpus supplements:32emitted paired rows (20DCD/12GD),18 readable
PRE/POST all6/1/10 unchanged and LuaInside,14formal-parameter-unavailable; zero
lifetime/qualified rows. Completion's unpaired1 is an event counter, not an
unpaired emitted row. Class serial0 remains explicit; no allocator attempts.
Artifact test-results/dev-feature-checks/native40-box-pair-native-summary.json.

Read-only participation audit: all246positive-cid source claims checked, zero
new emissions after exact full-life Mode eliminated either participant. Typed
verdicts246=239accepted+3parried+2round-over/target-down+1future(view167ms)+
1body_strike_miss(12uu). Owner239=150changed+87no-observed-change+1stale_context+
1expired. Terminal ids65/66 were emitted125/75ms before surrender, so queued
pre-down records remain intact. Expired75 is another lower-ID ordering refusal;
stale145 cause remains pending, do not invent transition/TTL attribution.

Next profile proposal is ignored native40-profile-failed-predicate-proposal.json:
preserve optional first-failure reason/scalars from existing source/proxy checks,
not repeated diagnosis or additional native getters. Session/Mode sequence
comparison, sample freshness and pending qualification are candidates, not proven.
The active proxy stage still checks source first, so it cannot attribute failure.
Replay exact batch membership is also unproved: ingress before a replay RESULT
is before callback completion, not necessarily before batch snapshot/entry. Do
not claim sorting necessarily repairs all four Native39 cases. Bounded retired
window initialization0 preserves prior wrap behavior but across-batch older
timestamps can change native damage-gate ordering; design not yet implemented.

Saved final audit: test-results/lab/native-20261008-40/participation-native-audit.json,
plus replay-order-design.json. Independently237native(ok) logs match150changed+
87no-observed-change; no suppressed-origin acknowledgements. Expired75 arrives
after77. Stale145 ingress12:55:47.628 precedes server abort47.630923; refusal
47.695 follows director quiesce47.680. Exact ctx-nil versus tuple-mismatch remains
unavailable; abort-transition timing is measured, not an inferred TTL failure.

### Native41 preparation: conservative batch order and first failed predicate

User accepts 98.35% claim acceptance, conditional on all armor/clothing/weapons
and faithful slashing, piercing, body/limb damage and dismemberment. Do not add
a 100% claim gate or call this overall completion. Native gear/body requirements
and documented pose/spawn/scenario gates remain open; no release authorization
conditions have yet been satisfied.

Root adds replay_batch: stable source-ID order only inside one already-drained
dense batch of at most1024 events, grouped by match/round/attacker/full lives.
Unambiguous modular ID span and positive nondecreasing raw source timestamps
are required. Different stream slots and immutable records are preserved; unknown
or regressing groups retain arrival order. Original ReplayAttempts watermark,
eviction, duplicate cache, native GD gate and later-batch refusals are unchanged.
No waiting or extra native calls are introduced. Exact Native39/40 batch membership
remains unproved, so this does not claim to fix across-batch ordering loss.

Spawn diagnostic reports actual first source/proxy predicate failure with bounded
already-read scalars, including sample age, Mode/Session versions and qualification.
Guards, getter order, freshness and capture budgets remain unchanged. Astra approves
both production deltas. Focused4suites861assertions(28batch+371Combat+43profile+
419Avatar),8parses and direct state-files pass; actual main typed-event fixture
proves batch ordering, once-only native execution even when optional logging throws,
and next-batch older-ID refusal. Artifact native41-focused.json. Native41 is pending,
and no native material/sever/physical parity is inferred from these fixtures.

### Native41 completed; full cuts and gear remain the next priority

d8072b4d full G0 passed66Lua suites,1279Rust tests/75binaries, clippy0errors/
2warnings, no skips; exact dev push and normal RequireG0 deploy passed. Raw
20261008-132915-402757-combat_manual, owned session native-20261008-41. Only
bounded joint profile enabled; expensive Combat/Box/Grip/Hand/Limb probes off.
Both games normal menu_quit, no new crashes/orphans/forced kills; owner/exp exit0.
Cloth recipe256/263=.973384, claim Box rotation mean39.155/max81.865degrees
(21samples), clock0/20windows. This is a separate sample from accepted Native40
98.35%, not an overall completion percentage or a loosened acceptance bound.

Actual batch_order triggers once(records2,moved2). One lowerID66 still arrives
after64/65/67/68/69/70 in round3 with identical source timestamp101784 and parent64.
It remains below a prior batch watermark. All-raw-run unique owner receipts256=
177changed+78no-observed-change+1expired. No repeated native application. This
proves the correction runs, not complete ordering repair. Typed census/ingress and
actual diagnostic lines saved in native-20261008-41/native-audit.json.

Actual first predicate inst1: proxy audit_changed, fieldmode_seq8→9, even though
active getter stage source:UserConstraint_12:accessor_before(total86ms). This
identifies a version-change refusal, not a causal physics defect. Inst2 emits one
pending/unqualified source/proxy configuration row; every rightjoint current output
unavailable at ChildBody, assets match but hand flags differ. No counterpart pair
or native joint/force parity. Comparison saved native41-joint-profile-native-comparison.json.

User confirms missing natural decapitation, whole limbs and torso cuts. Priority
is complete native sharp-cut transaction and owner-produced detached topology,
not extra distal masks or HP-based forced sever. Exact cooked factory has25
initializer inputs; current Damage lacks selected-tip and complete native setup/
force/marker history. Source scalar DCD/GD cannot reproduce the native constraint's
marker-wear→CallDismember→Initiate/Delayed transaction. Thrust is excluded from
the sharp-sever branch; piercing injury must still follow native rules.

Gear audit proves tier children inherit an empty passport and native BeginPlay
overwrites module fields from the supplied passport. First-family merchant
fallback cannot prove selected tier. Direct authored presets need their own exact
asset/property/entry identities; runtime generation mutates the world and must
choose/freeze one validated complete recipe for peers. Do not silently relabel
generic base presets as tier children. Mid/High polearm list3haft cases but shown
RNG0..1 reaches only2. Exact ignored audit native-tier-passport-provenance-20261008.json.

Owner's next feature, ordered after dismemberment and clothing/armor are fixed:
co-op Abyss, shared multiplayer waves against AI enemies. Preserve that request
in the backlog and subsequent heartbeat handoffs; do not replace current combat
and gear blockers with co-op work. Conditional PR/merge/release gates remain open.

### Next checkpoint preparation: native scalar responses and explicit recipes

Native bridge source LuaUObject.cpp203–225 registers scalar out tables and continues
without consuming the original argument. Registry::make_ref pops only its duplicate.
Consequently consecutive scalar outputs can all target the first supplied table;
Native41's missing ChildBody is consistent with this source-proven incompatibility.
Owner DCD now supplies one fresh shared container in slots18–23, preserving all
exact output names; joint profile does the same for both FName outputs and each
scalar getter group. Native inputs/getter counts/guards are unchanged. Actual
effectiveness still requires the next run, not an inferred successful native capture.

native_damage_response retains byte Hit Surface, doubles Damage/Cutting Rate/
Rigidity/Material Density Out and native bool Lower Threshold Out from the existing
single owner DCD call. No extra native call/getter, retry, default value or sever
authority. Snapshot plain invocation context before native call, preserve only
same world/drop/peer afterward, and clear on WG drop. Partial copy-out stays
incomplete even when native injury receipt is successful; protected copying/logging
cannot disrupt original cleanup. Tests model first-table fan-in, exact zero/false,
missing bool slot, copy-out-after-injury, optional observer failure and actual
world drop during invocation. Root4suites859assertions/6parses, Astra approved.

Armor's frozen private native_weapon_presets module has7exact authored recipes:
Pollaxe, Longsword, Bastard Sword, Arming Sword, Mace, Baron Sword and Baron Mace.
Native asset/property/array entry or exact enum-map key is required, with all25
saved fields and material enum numbers from actual Names. All175 literal fields
match cooked evidence. Recipe identity reaches owner equip/verify/rearm and remote
fallback/empty-left debt; explicit recipe refusal cannot borrow merchant templates.
207assertions/5parses, Astra approved. No public catalogue/server admission change:
all use ModularWeaponBP_C, whose mixed native families need exact historical
recipe/source authentication before combat admission. Existing tier ambiguity and
all-gear runtime geometry/material acceptance remain open.

Cut journal implementation/review is in progress: independent developer control,
POST-only bounded initializer/marker/cut-phase evidence, copied owner responses,
explicit unknown baseline/history and zero relay permission. No executor, forced
sever or unguarded proxy damage is introduced. Full native cutting/upper-body
topology, fair native gear, spawn pose and native scenario gates are still blockers.

All next-checkpoint source is now frozen and Astra approved. Root final7suites
1120assertions,13Lua parses, direct state-files and diff-check pass. Journal43+
Combat392 subset435 passes; explicit dev cut_probe on/off lazily installs5POST
hooks only in fresh Live. One15s window,16source events/3failed admissions plus
2separate owner scalar responses; world/drop checked before borrowed objects,
stop/deadline token reaches nested topology and every existing read. No heavy
caller probe or automatic start. Natural cut and native performance still pending.

Actual enum Names must be used for labels: Head0,Neck1,Torso2,UpperArmR3,
LowerArmR4,HandR5,UpperArmL6,LowerArmL7,HandL8,ThighR9,CalfR10,FootR11,
ThighL12,CalfL13,FootL14. The existing distal projection is nevertheless correct:
raw Delayed2298 ByteConst selects3→lowerarm_r,4→hand_r,etc, then8554 hides that
subtree and24563 writes PartsMap using the original enum byte. Named cut segment
and wholly hidden distal subtree differ for partial cuts. Never mechanically
renumber projection from labels; hand/foot-at-seam and head/torso need geometry.

Native42 proposed public w_axe2h cloth oracle: authored tips5/6 are candidates,
ordinary w_axe tips max2; native selected Closest Tip overwrites current Sharp
Level, so journal actual CutLevel/GoreRate rather than assuming authored maxima.
At actual Gore1 torso needslevel5/6markers, headlevel2/4markers. Journal Live-only
coverage ends on surrender/RoundOver; corpse continuation remains unavailable,
though native Initiate/Delayed has no DED/alive prerequisite. This is a separate
future topology observation scope, not permission to resume eliminated hit claims.

### Native42 completed: material copy-out verified and natural wear observed

Clean cf53c916 full G0/dev push/normal deploy passed (69Lua suites;1279Rust/
75binaries; clippy0errors2warnings; no skips). Raw20261008-142911-96d320-combat_manual,
session native-20261008-42, two-handed axe cloth recipe. Joint profile only startup
flag; all expensive caller/Combat/Grip/Hand/Limb/Box probes off. Cut journal armed
once via exact owned processes after both kit verdicts w_axe2h and RCON Live.
Both5POST-hook sets returned complete registration, both normal menu_quit; no
new crashes/orphans/forced kills, owner/exp/watcher exit0.

Recipe157/158=.993671 claims, but below required200samples; do not call acceptance
complete or compare directly with prior ordinary-axe recipe. Claim Box rotation
mean24.251/max43.658degrees,n17; clock0,n14. Round1 and3 load_failed; round2 and
later4/5 settled. Fair spawning remains blocked even with costly probes off.

Native rows11=4owner scalar responses+4constraint_begin+3wear, all source rows
inst2. All4owner responses have all6fields available, exact context and complete
native call. Actual densities1000/1150, rigidity.5/.45, HitSurface1/2 and boolfalse
retained, including a legitimate zero DamageOut. This verifies pinned scalar
fan-in correction on actual DCD. It does not prove paired material/protection parity.

All4native25-field initializers complete. BeginPOST Gore0/markers0 precedes
latent setup; later actual wear Gore2, CutLevel6/5, Thrustfalse. Wear observations:
ThighL(part12)3markers/2current,Damage657.157,HP099 first; Torso(part2)1/1,
Damage476.261,HP100; same torso1/0,Damage53,916,185.127,HP000. Large input is an
observed native value, not a proved servo cause or solo force parity. Torso marker
count1 cannot meet native6-marker gate even atHP0. Zero cut_attempt/Initiate/Delayed
rows, so no completed sever or full cut topology. Parent candidate original snapshots
retain cidnil; first2wear rows have no assignedcid yet, third exposes currentcid39.
Accepted whole wear history remains unknown; no relay authority. Source reads mean31.857/
max53ms across7rows; whole capture/performance acceptance is a separate measure.

Artifacts native42-cut-native-rows.json and native42-cut-native-summary.json.
Both profile captures now fail actual scope before a complete row: inst1 proxy
Mode_seq8→9 at source:UserConstraint_11:accessor_after,total165ms; inst2 original
source sample_age250.334ms at source:asset_after,total221ms. No current joint pair,
no threshold relaxation. Comparison native42-joint-profile-native-comparison.json.

Next implementation while games stopped: preserve waiting-parent continuation
in server lower-ID barrier. Existing inside_continuation can Hold before storing
Decision; child66 then relies on120ms resend, allowing67–71 past. Native41 server
order supports that branch but original hold was not logged. This is narrower than
a new execution-sequence protocol or admitting old native timestamps. Sol owns
Rust fix/regression, Astra reviews. Cuts/gear remain priority; co-op Abyss queued.

### Native43 preparation: retained parent wait and cheaper scope allocations

Sol server fix retains the original continuation and copied first-arrival context
in the existing bounded Decision.waiting/active barrier when its known parent is
waiting for stream coverage. Tick and retransmit use inside_continuation only;
child pose evaluation, edited resend replacement and a second rate debit are
excluded. Full match/round/attacker-life/victim-life ordering scope is preserved.
Production Engine/Store v2 body-history regression proves65WAIT,66retained,
65accepted,67held,flush66/67 without120ms child resend. Parent rejection/orphan,
life130/131, immutable retry and existing160token admission are covered.
50Combat+61lagcomp=111focusedRust tests pass; Astra approves frozen two-file delta.
Artifact native42-parent-wait-regression.json. This repairs the demonstrated
missing-retention mechanism, not all possible dispatch inversions or native parity.

Avatar scope guards now reuse two lazy source/proxy Lua session readers. Each
guard still freshly reads and validates native info, updates its own plain facade,
force-polls and checks liveness at the original location. Current Mode, peer,
assignment, world/pawn/mesh,250ms age, before/after guards and loss latch remain
unchanged. No native result cache, getter removal, freshness reset or budget change.
Actual-main success then fresh-header/heartbeat/link/peer refusal and restoration
fixture passes. Root3Lua suites506assertions(Avatar429,joint44,session33),2parses,
direct state-files and diff-check pass. Astra approves both-file delta. Native43
performance, full joint pair and reliable spawning are pending actual game evidence.

Native42 Astra audit confirms first two wear inputs exactly follow cooked initial
impact*(1-DrawCut),657.1573457/476.260963959. ActualGore2 thigh count3 fails required
5; torso count1 fails required6, independently of marker HP. The53,916,185 later
input is consistent with clampedHP0 but cannot authenticate prior wear. No forced
cut or threshold adjustment is warranted. Full owner-authorized cutting transaction,
detached head/limb/torso geometry and all-gear runtime validation remain open.
Co-op Abyss multiplayer AI waves follows those blockers. No PR/main/tag/release.

### Native43 completed; user reprioritizes fair spawning before release and co-op

be58307d full G0/dev push/normal deploy passed69Lua suites,1283Rust tests in75
binaries, clippy0errors2warnings; no skips. Raw20261008-150436-e8260e-combat_manual,
session native-20261008-43. Root used the wrong joint-capture environment variable,
so this run is probes-OFF gameplay evidence, not cached-reader performance proof.
Correct next run flag is HSMP_JOINT_PROFILE_PROBE=1, with HSMP_GRIP_PROBE,
HSMP_HAND_PIPELINE_PROBE and HSMP_LIMB_BURST_PROBE=0. Expensive native caller stays off.
Both windows were on smallest secondary DISPLAY1,880x527. Owner/exp exit0,
normal menu_quit both, no new crashes/orphans/forced kills. Manual harness PASS
certifies cleanup/basic state only, not fair spawning or release gates.

Recipe194/196=.989796, below200minimum; Box rotation mean30.606/max106.586degrees
(37samples), clock resets0(24windows). All-run unique owner receipts194=
123changed+70no-observed-change+1dropped, expired0. This is observed absence of
the prior inversion refusal, not proof that every server ordering branch occurred.
Load failures r1peer2/r6peer1 timeout;30twisted-arm reposes. Native42 source audit
also finds fresh upperarm_l123.3degrees/hand_l52uu divergence and protected owner
drift corrections61–78cm. Repose resets continuous150ms settling qualification;
source placement resets publication. Existing refusal is appropriate; native physical
cause remains unproved. Artifacts native43/native-audit.json and native42-spawn-readiness-audit.json.

Cooked constructor maps pelvis contact to spine_02 constraint anchor; actual same
constraint can return to the original pelvis contact. Source membership confused
that immutable contact with initial anchor and refused the Native42 continuation.
New narrow branch requires original anchor spine_02, current pelvis, original
captured header AND current parent contact pelvis, plus every existing exact
constraint/module/world/full-life/header check. No server bone-policy change;
no marker wear/cut authority. Wrong original bones, changed parent/header/life/module,
replacement constraint and unrelated bones refuse. Sol68assertions/2parses and
Astra approval. Artifact native43-original-contact-return.json.

LATEST HUMAN INSTRUCTION supersedes the earlier perfection-before-release ordering:
fix body spawning and twisted arms, then merge main and produce the release build,
then implement co-op Abyss multiplayer waves against AI. Prioritize that concrete
spawn/pose repair now. Release authorization remains conditional on demonstrating
fair stable spawning and required G0/G1/native G2 checks; do not merge/release while
those fail. Explicitly disclose remaining cuts/all-gear parity rather than claiming
them complete. Optional cut-descriptor implementation suspended with no partial edits.
The prior broader combat/gear/dismemberment work remains backlog, not a reason to
override this newer requested order. Subsequent handoffs/heartbeat must preserve it.

### Native44 preparation: grip motors and fault-side source evidence

cbe95b23 full G0/dev push passed69Lua suites,1283Rust tests/75binaries,
clippy0errors2warnings; no skips. This is the contact-return checkpoint; native
deployment/test now uses the combined subsequent spawn milestone.

Proxy grip motor-off policy previously zeroed angular motors while leaving native
linear enable flags active. Native28 right-hand fault retained XYZ position/velocity
true with stiffness7500. New flags-only lease disables those six enables while the
current proxy hand servo runs. All native per-axis strengths, targets, modes and
existing limits are preserved. Originals are captured before either setter, and
independent plain restoration debt survives p.grips disposal/partial failure and
retries once/s. Fresh exact constraint/endpoints/bones/owners/body/held root/world/
full life bind every mutation and restoration; changed bindings never inherit old
flags. Source local peer refuses. Readback is required for off_confirmed/restored.
Bounded64 process-total once-per-lease scalar logs distinguish actual000000 from
refusal. Fresh PC-world/plain scope is checked immediately after endpoint getter,
before returned object access, and at final validation; actual-main regression
changes PC-world only while cached WG remains old and returns dead endpoints.

This is a supported right-hand conflict repair, not yet a native convergence or
FPS result. Full binding resolution repeats four times per steady hand application
in current frame/BP policy paths; Native44 must measure cost rather than claim it
cheap. Native43 round6 was separate left upperarm12.9–14.4degrees while hands were
close; linear flags alone cannot explain that failure. Native Set Up Armor resets
Bone Constraints Current from Ded, applies minima for equipped armor and writes
current limb limits. Asset defaults/name agreement alone cannot prove runtime parity.

Developer double-opt-in joint focus upperarm_l selects only nativeUserConstraint_14
with actual parentclavicle_l/childupperarm_l in both independent roles. Cooked
asset and prior native capture agree; runtime accessor owner/index/endpoints and
before/after asset/freshness/loss checks remain mandatory. Fixed selection copied
once; arbitrary names refuse. Default three right joints unchanged. Source guards,
250ms limits and capture budgets unchanged; no cached native values.

Separate HSMP_DEV=1+HSMP_SPAWN_DRIFT_PROBE=1 captures at most3 already-triggered
protected drift corrections per process. It compares fresh physical pelvis COM
with already-read capsule and destination before existing correction, with exact
current assignment/pawn/Mesh/world guards before/after reads and logging. Historical
verified coordinates retain separate time and explicit unknown Mesh continuity.
Ordinary unavailable COM does not widen proof; scope loss prevents old-world moves.
DefaultOFF adds no reads, and no new physics setters or placement tolerances exist.

Root final4Lua suites800assertions(Avatar451,profile51,session33,placement265),
7parses, state-files and diff-check pass. Astra independently approves all3scopes.
Native44 must prove flags really execute, frame/callback cost, actual6-limb settling
and the remaining left-shoulder/capsule-body mechanism. Until then no spawn closure,
main merge or release. Main/release checks stay distinct from manual-lab PASS.

### Native44 completed: six flags execute but spawning remains failed

b1175bb7 dev fullG0 and normal RequireG0 deploy passed69Lua suites,1283Rust/
75binaries, clippy0errors2warnings/no skips. Raw20261008-155217-bdacbd-combat_manual,
owned session native-20261008-44; both windows smallest secondary DISPLAY1.
Correct joint flag/focus upperarm_l and bounded3source-drift observations enabled;
other probes and expensive native caller off. Owner/exp exit0, menu_quit both,
no new crashes/orphans/forced kills. Manual basic-state/cleanup PASS is not spawning
acceptance. Custom recipe145/165=.878788, Box rotation mean35.39/max137.74degrees
(37samples), clock1reset/10windows. Acceptance is lower than Native43, not a proved
single-cause experiment or permission to loosen any bound.

19right-hand linear off_confirmed rows retain original111111/actual000000. Zero
left confirmations and zero restorations logged; unavailable left objects are explicit.
Initial right hands were close, but left lowerarm173degrees persisted; customr2
right hand144.7–168degrees and21right-hand reposes remain. One-time flag confirmation
does not prove contemporaneous flags at the later fault or all bound endpoint/root
identities as separate log scalars. The policy executes; it does not close arm faults.

Native source/proxy current left-shoulder rows complete in both instances: actual
index15,clavicle_l→upperarm_l,limits75/75/45,soft50/5/contact1,projection1/0 equal;
source drive500/1/0 versus proxy0/0/0. Both warm qualificationsfalse, bootstrap match,
before/overlapping final proxy Set Up Armor. These prove early configuration only,
not final dressed or later-fault parity. Capture costs184/188ms. Saved dynamic
counterpart comparison native44-joint-profile-native-comparison.json.

All6SPAWNDRIFT rows current/available, three perprocess: inst1customr3/peer1slot2,
inst2customr6/peer2slot2. CapsuleXY error62.92–77.58cm, actual physical pelvis COM
error78.63–92.44cm; body-minus-capsule only15.98–17.16cm. Both physically displaced
in the same direction; capsule-only false correction is unsupported. Captures6–9ms,
historical Mesh continuity unknown. This same slot affects both peers. Re-placement
then creates stale/held source cascades; other arm failures occur without drift.
Native lower-body balance targets and translated-only rotational placement need
separate proof. No changing drift threshold or using capsule-only/body-only shortcuts.

Bootstrapr1 failed HSMP2; customr2 failed HSMP1/r3 failed HSMP2; customr1/r4/r5 had
both6limbsettles and nativeAI combat. Root stopped after bounded recipe. Logged
active driver cost weighted.777522ms Native43→3.462802ms Native44,4.45x; mean drives
perwindow296.36→281.31. Callback elapsed/renderFPS excluded. Performance regression
requires action, not a claim of fixed2FPS. Exact audit flags-performance-audit.json
and six-source-row analysis native44-source-drift-analysis.json are ignored artifacts.

NEXT: retain flags correctness but avoid setters when an existing exact lease already
reads all6off, with full fresh binding before/after and original mutation proof on
any changed flag. Capture actual wrist fault after dressing rather than early warm
left profile. Sol independently examines incomplete source yaw/physical/native
balance-target placement; Astra reviews. Main/release remain unfulfilled because
body spawning and twists still fail. User's order remains spawn fix, main/release,
then co-op Abyss; do not replace it with unrelated gear/cut expansion.

### Native45 preparation: avoid redundant writes and observe actual wrist faults

Sol fast grip lease path now performs two fresh complete binding resolutions and
two strict flag reads. Stable six-off flags avoid all setters; any changed flag
falls through the original guarded mutation path. Native endpoint getter reentry
must preserve fresh Mesh, constraint and held-root identity before dereferencing
returned endpoints. Native45 must measure runtime cost; no improvement claimed yet.

DefaultOFF bounded joint capture supports exact hand_r UserConstraint_12 focus and
fault-only admission from completed six-limb SETTLE state. Measurement Mesh identity,
generation, full life/cut, aim timestamp, freshness and actual hand stall are checked
before consuming a capture attempt. Six-limb maxima are labeled as such. Read-only
grip observations preserve explicit unavailable results, independently unavailable
lease originals and stable source/binding identity; they never write physical state.

Existing three source-drift captures add actor/control/ground yaw, evaluated socket
rotations, input and feet positions, native lower-body handle targets and separate
physical pelvis COM. Physical orientation remains explicitly unavailable; evaluated
sockets are not independent rigid-body rotations. No new placement setters or bounds.
Cooked handle first target uses pelvis socket rotation plus90; later ground-yaw
logic may recompute it, so translated-only rotation is a hypothesis pending game data.

All three scopes frozen and Astra approved. Final offline checks: four suites,
841 assertions (Avatar466/profile67/placement275/session33), six Lua parses,
state-file allowlist and whitespace clean. These are mocked/source checks, not native
spawn acceptance. Native45 uses hand_r focus, fault trigger and bounded drift capture;
native caller and other expensive probes remain off. Keep spawn→main/release gates
→co-op Abyss ordering; spawning remains failed on latest completed Native44 evidence.

### Native45 completed: less driver cost, wrist capture admission gap found

46446274 fullG0 passed (69Lua suites,1283Rust/75binaries,clippy0errors2warnings),
exact dev push and normal RequireG0 deployment passed. Native45 raw run
20261008-163437-f5b8d2-combat_manual, session native-20261008-45. Both windows
on smallest secondary DISPLAY1. Native caller and other expensive probes off.
Owner and180s recipe exited0; normal harness shutdown, no new crash dumps.
Manual basic-state/cleanup verdict PASS does not certify spawn or release readiness.

Harness quickG0 failed12Avatar assertions because the new trigger/focus inherited
into fixtures: boot pinned PROBE but omitted TRIGGER/FOCUS. Full prepushG0 had
passed under ordinary environment. Native45 remains diagnostic-only; do not count
its quickG0 as green. Minimal fixture env isolation is prepared for Native46.

Recipe128/137 accepted (.9343065693);47stall events, wrist170.2degrees with118frame
repose observed. Box rotation mean35.97/max75.69degrees,31samples; zero clock resets
in21windows. Bounds and minimum counts unchanged; these do not meet spawn acceptance.
Final same-filter88active windows driver cost2.410778ms versus Native44 3.462802ms,
about30.38percent lower, median2.3695ms,286.9659drives/window,0deferred; callback
elapsed and renderedFPS excluded. Final flags-performance-audit.json preserves
19R/0L confirmations and31RH reposes. No claim that grip policy cured the fault.

All6current source drift rows available, three eachprocess. Capsule60.3–78.3cm and
physicalCOM76.3–95.5cm displaced in inst1 bootstrap. Actor/control approximately89/90,
evaluated Mesh/Driver approximately0, native lowerhandle86–90degrees. Native Mesh
relative yaw270 and pelvis+90 lowerhandle convention explain those offsets; this
rejects the simple stale evaluated Mesh/Driver/lowerhandle yaw hypothesis. GroundYaw
is native derived/interpolated, not independent physical orientation. Inputvector0.
Rotation captures65–97ms, significantly dearer than prior6–9ms COM-only captures.
Physical orientation unavailable remains explicit. No yaw-reset fix authorized by this.

No JOINTPROFILE capture occurred despite actual persistent RH faults. Production
PURE.displayed_pose copied match/round/life but dropped cut; actual fault admission
required shown.cut and therefore always refused. Prior positive helper tests supplied
a fabricated cut; actual-main test covered only refusal. Native46 copies exact pose.cut,
pins fixture env and includes a positive27frame actual drive_v2 regression. Astra
approved. Raw fault-only native joint angles use shared scalar out table, remain
physics_verified=false/native lookup behavior unproved and grant no target authority.

Another concrete placement gap: native absolute Foot IK scene components are carried
by initial teleport but omitted from residual protected hold. Sol preparing the narrow
same-translation hold repair and world/replacement regression; native sole causality
still requires the next run. Keep Native46 diagnostic admission and this placement
fix separate from any declaration that all arms or source spawning are repaired.

### Native46 frozen preparation: carry absolute balance inputs during protected pins

Native Willie R/L Foot IK and StepSplineR/L templates have absolute locations;
idle foot writers sample each spline GetLocationAtTime(0,World) and set Foot IK.
Residual env.hold previously moved actor, meshes, weapons and scalar targets but
omitted those four scene components. The fix snapshots these exact fields only
inside residual>existing10cm branch, then carries any that did not already follow
the actor, using the existing hold mesh/weapon5cm follow tolerance. Initial
teleport30cm follow, drift60cm and actual six-limb5uu/10degree/150ms gates unchanged.
No yaw reset, profile reset, new physics strengths or generic component enumeration.

Fresh placement/PC-world/body and exact component field/owner/identity guards stop
old writes after replacement. Retained scene components are reacquired after actor
translation. Non-ProcessEvent identity wrappers are grouped under before/after
world checks, while reflected read/write stages retain individual guards. Fixture
read ceiling1200 not raised: modeled four-target residual pin852wrapper/845world
checks, down from1282/2813 before grouping. Actual game correction cost remains
unmeasured; this count is not runtime/FPS acceptance. New real-env12/20/29cm pins,
already-following targets and PC-world/same-world Mesh/target replacements tested.

Alongside exact displayed cut propagation, ambient fixture isolation and positive
production fault admission, final four suites881assertions pass (Avatar475,
profile72,placement301,session33), seven Lua parses and state-files pass. Astra
approved all correction scopes and final tiny timing delta.
Native46 must establish
source drift, final-dressed fault-time wrist/flags and actual six-limb spawn behavior.
Raw joint angles remain unverified native API outputs, with explicit no authority.

Existing developer drift opt-in also brackets env.hold with exactly two os.clock
reads, finite monotonic timing stored only in current assignment's plain counters.
Calls/moved/total/max append only to the existing once hold-release log. DefaultOFF
adds no clock/stat reads; no new UE gets/setters/files or per-frame logging. Actual
source pose sender timing starts after placement and would not measure this cost.

### Native46 completed: source drift remains; user demands direct lifecycle correction

763aba06 fullG0, exactdevpush and normalRequireG0deploy passed; harness quickG0
also passed with diagnostic environment, confirming fixture isolation correction.
Raw20261008-171510-724b19-combat_manual, session native-20261008-46, both windows
on smallest secondary DISPLAY1. Owner/180s recipe exited0, both menu_quit/eventloop
ended, no new crash dumps. Manual basic-state/cleanup PASS is not spawn acceptance.
337/346 claims accepted; honest subset337/342=.98538011696 (user's earlier98.35
sample threshold exceeded, NOT overall progress or spawn acceptance). Box rotation
mean38.263/max178.276degrees,77samples. One foot-r stall/repose logged; severe
upperarm/wrist rotation remained in diagnostic/physical-ready checks. User personally
reported one actor moving while protected and wrong arm again; root acknowledged
foot carry had not solved whole-body drift and stopped after the bounded run.

Early recipe rounds1/2 verified placement try1 and both6limbsettles; later recipe
r6 inst2 peer2 slot2/spawn1537 repeatedly drifts. Three exact current observations:
capsule64.28/61.05/79.80cm, physical pelvis COM75.61/75.94/94.62cm, native input0,
body-minus-capsule15.5–16cm. Re-verification13–44cm then repeated>60cm watchdog
re-placement resets source freshness and creates source held/stale + upperarm errors.
All values overlapNative45; the four-target repair closes a consistency defect but
does not prove convergence. Final source audit native46-hold-drift-live.json.

27timed hold-release rows, worst call60ms; three max>=50ms rows (60/59/58), row
mean1.0–9.474ms. Later failed re-placements often short no-move holds, so cost is
not proved sole drift cause. Rotation observations84–95ms separate from hold cost.
Shared release logs have no exact process/assignment IDs; attribution unavailable,
no time-only join or summing cumulative retry aggregates as independent totals.

Both fault captures refused, zero completed profiles. inst2 123ms fails proxy hand
stage Session53→54 with loaded_round/waiting change; attempt precedes actual proxy
kit_verified by268ms. inst1 156ms fails proxy wrist accessor Session190→191 after
proxy kit_verified10.102s; tap comparison changes onlyseq/server_time_ms, sameLive
full semantic fields. Keep guard/refusal evidence; no fault flags/angles/current
paired grip proof. Next physical test disables JOINTPROFILE to remove those stalls.

Concrete lifecycle gap: full hold ends0.9s after teleport and skips c.done, while
spawn protection continues Loading/Countdown. Native balance then moves the body
without movement input. Native47 work GO: keep original initial0.9s fullhold,
then exact protected Loading/Countdown residual position carry only (no blanket
body/weapon velocity cancellation), including verified c.done; release beforeLive,
never anchor warm respawn/no_protect/wounded/old assignments. Preserve all world,
body/field bindings, physical simulation and readiness/position/arm bounds.

Sync/Match agreed existing local SpawnStatus why=anchor_released + original durable
t in seconds, full match/round/life/spawn/pawn/arena/verified/positive seq. No new
network schema. Release only after fresh exact initialLive, never inferred from
protect_until or unknown world/session. Match first input must not reuse old
qualification/hold-era stable proof. New post-release suffix requires actual
min(settle_stable_ms,settle_sample_ms-original_local_release_ms)>=150, strictly
newer/fresh sample and unchanged fullscope/6limbs/5uu/10degrees. This avoids a
deadlock from demanding continuous producer interval start after release, while
proving the whole last150ms was actually stable and after release. Do not rewrite
producer timestamps or compare clocks across clients. Astra approved design;
implementation owned Sync by Sol, Match/helper/fixtures by Armor. Root will review,
G0/deploy and run native evidence; main/release still conditional on spawn correction.

### Native47 frozen implementation: close protected anchoring lifetime gap

Sync now keeps exact initial Loading/Countdown residual XY anchoring after the
original0.9s transient hold, including c.done, until initialLive. Continuation
does not cancel any mesh or weapon linear/angular velocity, including nested
carry_weapons; no-residual continuation performs no physical setters. All original
finite0.9s Livefall/DM/no_protect/warm holds retained, only new continuation/ACK
excluded there. Phase changes inside reflected getters stop later old setters.
FirstLive source disables anchor independently of input, native-qualifies current
world/pawn/Mesh, then writes durable existing SpawnStatus marker and original t.
No guessed protect_until release or new network schema; rewritten status preserves t.

Match normal firstLoading pipeline (typednativephase1 maps countdown) requires
exact ACK, an actual newer own pose and150ms contained measured physical suffix
after release. Old qualification cannot bypass first proof. Exact release-qualified
context survives paused/reconnected injured same-life control; warm firstLive
never anchored does not require ACK. All6limbs/5uu/10degrees/150ms and existing
freshness/health/collision/source/context guards unchanged. Producer samples untouched.

Root found test AI can bypass human input barrier: Parity previously gates server
Live and AI_PROOF only. It now additionally requires strict false from actual
Controller:IsMoveInputIgnored immediately after proof, then fresh native PCworld,
same controller address and current pawn address+FName before takeover. Unknown/
throw/travel/possession/reused pointer refuse. Ongoing already-controlled AI intent
unchanged; actual Native47 scripted mover disabled. This controller flag is actual
input state, not an exclusive Director token (scripted mover can also reset it).

Astra approved all3frozen scopes. Final5suites1216assertions(Avatar475/Director335/
Sync323/ParityAI50/session33), seven Lua parses, state-files and whitespace pass.
AI fixture's replacement world has a real matching name but different address,
and verifies no post-getter old pawn read on world loss. Native47 must show protected
body staysplaced, arm settling, postrelease input/AI and convergence. JOINTPROFILE
disabled to avoid123/156ms failed capture stalls; native caller/hand/limb probes off.
Existing bounded drift observations and plain hold timing may remain enabled.
No main/PR/release yet: Native46 visibly failed the user's spawning requirement.

### Native47 actual outcome: protected position maintained, wrist still blocks release

Commit d5ab91ce52 passed normal full G0 (69 Lua suites, 1283 Rust tests), exact
dev push and RequireG0 deployment. Actual raw run is
test-results/20261008-175608-61ecd8-combat_manual; expensive probes off, mover off.
Bootstrap polearm round had bilateral six-limb settling and input release after
explicit post-release-stabilizing waits. The cloth/axe recipe failed Ready in
all three observed rounds because one proxy's right hand stayed twisted; two
completed load_failed and the third was stopped. No AI takeover observed. The
experiment exited 2 waiting for Live; combat_manual cleanup PASS is not spawn PASS.

Initial protected placements all succeeded on try 1 with 0-5cm residuals and no
Loading/Countdown watchdog re-placement. Native balance still moves: 94/98
residual corrections in long protected holds show active anchoring, not natural
convergence. Two large COM/capsule drift samples occur after round 1 load_failed,
when anchoring has ended. Worst timed hold call was 72ms; combined logs cannot
attribute this cost to an exact process/assignment or prove render FPS.
61 right-hand stall/repose rows persisted. Typical failed wrist rotation was
165-170 degrees while pelvis rotation stayed near zero. Do not claim fair spawn.

Source hold release and post-release wait/input log chronology is available.
The late read-only local-bus capture saw Menu with invalid empty SpawnStatus;
it does not observe the original durable release t or prove its numeric suffix.
Games stopped gracefully, saves preserved, no new crash dumps. Bounded audits:
test-results/dev-feature-checks/native47-protected-anchor-analysis.json,
native47-hold-drift-final.json, native47-anchor-chronology-final.json, and
test-results/lab/native-20261008-47/settle-readonly-audit.json.

Native48 bootstrapped the same deployed commit but was stopped before the proposed
limits=150 recipe. TUNE actually defaults to zero, contrary to an old comment;
the optional SetAngularLimits path uses bone names while native joints are named
UserConstraint_N (replication.md), so this command is not a verified intervention.
No joint-limit physics change was made. Existing close_limits selects the None
profile whereas cooked native initialization selects Motor; it is not an exact
prior-profile restoration. Neither 165-degree tracking error nor saved older
bone-relative angles proves current constraint-frame clamping.

Next bounded diagnostic preserves full actual raw Session/Mode semantics while
admitting only sequence/server-clock heartbeat changes; production readiness,
writer, hand and limb guards stay unchanged. This is needed because Native46
refused an otherwise unchanged wrist capture on a Session heartbeat. Loaded,
waiting, roster, spawn, map, phase and all other semantic transitions must still
refuse. Capture actual current named wrist limits/reference/grip flags before
choosing a physical repair. Main/release remains blocked by the twisted wrist,
then requires G1 e2e and documented G2 scenarios twice green. Co-op Abyss follows.

Static native gear candidate (not yet proved by current runtime data): Axe2H has
native default right grip 10, left grip 1 and alternate 15. SetUpRH copies the
actual held weapon default, but the native tick can reselect current right grip
10 to 1 when left-arm tonus/offhand eligibility fails. Proxy neutralization zeros
left-arm tonus. Current grips already travel in pose control C[2]/C[3], but Avatar
does not apply them. Repose copies the proxy's own animation, not the source's
DriverSkeleton. The bounded profile will include actual native current grips and
copied transported values, explicitly without native-read availability or a
separate control timestamp. Packet zero can be a sampler fallback; do not treat
it as proved native state. Primary audit:
test-results/dev-feature-checks/native49-axe-current-grip-audit.json.

Diagnostic retries, if finalized, remain fault-only: at most three process-total
attempts, five seconds apart, success terminal. Repeat full reads require fresh
source/proxy Loading/Countdown admissions. Only explicit semantic transition or
expired source sample may retry; snapshot availability, native world/binding/ABI
or structural failures remain terminal. Each attempt keeps its own wrappers and
first-loss latch; no old accessor/object reuse across attempts. Warm mode remains
once-only. This is bounded evidence collection, not a physical repair.

Native49 diagnostic source is frozen: six files, three focused suites with 616
assertions passed (Avatar524, semantic helper11, joint probe81), six Lua parses
and whitespace checks pass. A now-Live retry is stopped after fresh admission
before optional library lookup; the actual-main control-boundary fixture retains
the original measured fault via an explicit fixture-local oracle and proves no
second lookup/capture. It does not refresh sample timestamps or change production
fault predicates. Semantic snapshot and retry behavior are developer-only;
ordinary gameplay/readiness/writer guards and all physics remain unchanged.
