# Combat: validation, lag compensation, damage and the simulator

Code: `server/src/{lagcomp.rs,lagcomp/,combat.rs,combat_client.rs,validate/,vitals.rs}`,
`server/src/server/combat_glue.rs`, `mods/HSMPCombat`, `crates/hsmp-combat-sim`.
Records: `crates/hsmp-ipc/src/schema/combat.rs`. The in-game checklist is in
[combat-parity.md](combat-parity.md); the vitals stream is in [vitals.md](vitals.md).

## 1. Overview

Native injury channels remain independent: a blunt hit can lower consciousness
and head crush without crossing the main Health damage threshold. Temporary
unconsciousness remains recoverable. The owner's native `Event Lose Match`,
after its own prolonged knockout/submission checks, reports a scoped final
defeat in Brawl only (`death_report.reason=1`, `death.cause=5`). Other modes
continue through automatic knockouts until native death or deliberate surrender
(`death_report.reason=2`, `death.cause=6`). Elimination leaves a surrendered or
knocked-out body alive; it never writes Health zero or calls native Death/Dying.
The ordinary biological death report retains reason zero.

Damage is owner-authoritative and server-validated:

1. My weapon (or body) touches a peer's **stand-in** on my screen. The stand-in takes no damage.
   HSMPCombat reads the inputs of the game's own "Deal Complex Damage" call and sends them as a
   `damage` claim.
2. The server judges the claim against its rewound history of both players (lag compensation),
   bounds the inputs by what the contact could physically do, and either rejects it, holds it
   briefly for a possible parry, or accepts it.
3. An accepted claim goes to the victim's owner as `damage_in`. The victim's HSMPCombat **replays
   it natively** through its own pawn's "Deal Complex Damage": its own armour, height, wounds,
   bleeding, dismemberment and death apply exactly as in solo play.
4. After the victim reports an authenticated changed native injury outcome, every other
   player gets the approved record as `hitfx_in`. It replays cosmetics on its stand-in of
   the victim, then restores the stand-in's damage state. Transport ACKs and bookkeeping
   alone do not authorize wounds.
5. The victim's vitals stream (CON, part health, bleeding, ...) is what every screen shows.

Record flow (G2S = game to sidecar over shared memory, C2S / S2C = network):

| record | flow | what |
|---|---|---|
| `damage` | G2S, C2S | the claim; the sidecar fills `hit_id`, `round`, `age_ms` and resends every 120 ms until a final verdict (gives up after 2 s) |
| `damage_in` | S2C, S2G | the approved claim, to the victim's owner (`WireHdr.peer` = attacker) |
| `hitfx_in` | S2C, S2G | the approved claim after observed native owner injury, to other players with `caps::HIT_FX` |
| `damage_verdict` | S2C, S2G | `CONFIRM` (first acceptance), `FINAL` (ok or a reason code) or `CLASH` |
| `damage_ack` | C2S | the victim's sidecar got a `damage_in` |
| `clash` / `touch` | G2S, C2S | parry evidence: my weapon met theirs / their stand-in reached my body |
| `death_report` / `death_ack` | G2S, C2S / S2C | the owner's own death, resent until acked |
| `death` | S2C, S2G | a server-declared death |
| `vitals` / `peer_vitals` | slot, C2S, S2C | see [vitals.md](vitals.md) |
| `body` | G2S, C2S, S2C, S2G | the owner's passport body for its stand-ins (§5 "Stand-in body"); only with `caps::BODY` |

## 2. Simulator (`crates/hsmp-combat-sim`)

The crate compiles the **real** server sources in (`src/lib.rs` `#[path]` includes of `proto`,
`posecodec`, `lagcomp`, `combat`, `validate/`; `build.rs` lifts `loadout::catalog`) and drives them
with synthetic fights.

- **Fights:** 2–8 players in pairs, with a flanker for odd counts. Engagement kinds are clean
  (with retreats), parry, missed parry, feint, trade, clinch and fall. Weapons are sword, longsword,
  messer, rondel, axe, hammer and poleaxe; armour sets come from the catalogue.
- **Clocks:** each client has its own `os.clock` with up to ±300 ppm drift (a speedhack runs at 1.3×).
- **Links:** `hsmp-tools netsim` profiles per direction (delay, uniform jitter, loss, duplicates,
  spikes): `loopback`, `good`, `typical`, `wifi`, `bad`. The reliable channel retransmits after an RTO.
- **Display:** each peer is shown through the receiver jitter buffer, `clamp(p90 + 6, 16, 250)`, on
  top of one frame of servo lag. Servo noise is 2.5 uu for skeleton-driven stand-ins and 20 uu (σ)
  for the older PhysicsHandle stand-ins (`--v1`). Extrapolation stops after 100 ms or 30 uu.
- **Contacts:** on each screen, the blade is swept over the frame and bisected to the first touch.
  Hands also touch bodies. A physics blade stops after a two-frame touch. Blade-on-blade contacts
  raise a clash report and deflect the blade.
- **Damage:** an independent copy of the decompiled Get Damage formula (§4).
- **Client:** the HSMPCombat claim path, either `--legacy` (one claim per call) or dedupe (one claim
  per contact). The sidecar resends every 120 ms with `age_ms` and stops on the final verdict.
- **Server:** the glue for confirm, forward, owner ack and final verdict. Held hits are flushed and
  clashes judged on a 60 Hz tick. RTT samples feed `lagcomp::note_rtt`.
- **Cheats** (`ALL_CHEATS`): `Backtrack`, `ReachFake`, `ReachBlade`, `ReachArm`, `DamageInflate`,
  `ParrySpam`, `TsForge`, `Speedhack`, and:
  - `JitterInflate`: the victim's sidecar holds each pose packet 0–250 ms (a lag switch on its own
    stream). Success = an honest hit on it rejected.
  - `AckHold`: the same lag switch on all outgoing traffic, transport acks included, so the
    server-measured `rttvar` inflates too.
  - `FakeParry`: when an attacker's shown blade comes near the victim's body, the victim streams
    that blade segment as its own and reports a clash. Success = an honest hit cancelled as
    `parried` with no clash on any screen.
  - `GodMode`: the victim acks forwarded hits, never applies them, and keeps reporting full Health.
    It counts as caught when the server flags it within 2 s (§8).

  A cheat claim counts only when it is *effective*, meaning it lies by more than every server
  tolerance.

Run it:

```
cargo test -p hsmp-combat-sim -- --nocapture          # the asserted thresholds + report tables
cargo run --release -p hsmp-combat-sim -- --seeds 3 [--profiles a,b] [--legacy] [--v1] [--parity] [--no-cheats]
cargo run --release -p hsmp-combat-sim -- --debug PROFILE [--players N] [--seed S] [--cheat NAME]
cargo run --release -p hsmp-combat-sim -- --parrystats PROFILE
cargo run --release -p hsmp-combat-sim -- --health PROFILE
cargo run --release -p hsmp-combat-sim -- --hvf PROFILE         # hit_vel_factor calibration (§7)
cargo run --release -p hsmp-combat-sim -- --hvf-logs FILE...    # ... from UE4SS / server logs
```

The assertions in `tests/combat_sim.rs`:

- honest acceptance ≥ 99 % on loopback, ≥ 97 % on typical and ≥ 93 % on wifi;
- ≥ 97 % on typical with PhysicsHandle stand-ins (20 uu servo noise);
- no `bad_field` rejections;
- false parry-cancels ≤ 0.5 %;
- ≥ 90 % of hits through a parry that some screen showed are cancelled;
- ≥ 90 % of real trades land on both sides;
- ≤ 1.5 claims per contact;
- no accepted hit judged against a victim pose older than 360 ms (cap + tolerance);
- ≤ 3 % of honest hits damage-clamped;
- every cheat ≤ 0.5 %, or at most one success when there are fewer than 200 attempts;
- honest players next to cheaters ≥ 97 %;
- solo parity and health replication (§7, §8);
- the replayed hit spot falls under the same armour layer as the touched one for ≥ 99.5 % of
  replayed blade blows (`hit_location_survives_the_replay_delay`, typical and wifi; §5 "Where the
  blow lands").

Notes on the profiles:

- `bad` (RTT 220 ms, ±40 jitter, 200 ms spikes) and `far` (RTT 300 ms, ±50 jitter) put the honest
  view lag at 400–430 ms. The rewind cap follows the attacker's path (§3), so these hits land:
  accepted 98 % on `bad` and 99.9 % on `far` (they were 5 % and 0 % under the fixed 300 ms cap).
  Beyond 600 ms of view lag (`REWIND_CEILING_MS`) the defender wins.
- Without a blade stream (PhysicsHandle stand-ins), parries cannot be validated: blades estimated
  from hand plus weapon actor fall outside the clash tolerance.

## 3. Lag compensation (`lagcomp.rs`)

The server keeps about 1.2 s of every player's root, skeleton, blade and body capsules, keyed by the
sender's own clock, with a sorted insert so reordered UDP fills holes. A per-connection clock map
(`offset = min(server_rx − sender_ts)` over 2 s, plus `jitter_p90` of the excess) and a per-connection
RTT let the server **predict** what the attacker was displaying:

```
expected_view(victim) = attacker_ts + off_a − off_v − rtt_a
                        − interp(interval_v + jitter_path_p90 + 6, 20..250) − 1 frame
```

The claim's `victim_view_ts` is only a hint, clamped into a window around that prediction. A cheater
cannot pick the most favourable victim pose from the history buffer.

Policy, in order:

1. Timestamps are required once the victim streams. The attacker's own timestamp must match the
   claim's arrival through its clock map.
2. Rewind: at most the cap behind the victim's newest sample, net of a capped delivery credit.
   The cap is the attacker's honest view lag on its measured path (RTT + its modelled buffer of
   the victim + one frame, `ViewPrediction::honest_lag`) plus the view tolerance and 40 ms,
   never below the configured cap (300 ms; 400 on high-latency servers) and never above 600 ms.
   Beyond the cap the defender wins.
3. Geometry: the contact must lie on the victim's rewound capsules, and on the attacker's blade swept
   over the last frame (or, without a blade stream, within reach of its weapon actor and hands).
4. Blocks: only server-validated clashes cancel a hit. A geometrically valid hit is held for the
   defender grace only when a parry is plausible.
5. Trades: an attacker that died at most 150 ms (its clock) before its own hit still lands it.

Every constant and why:

| constant | value | why |
|---|---|---|
| `HISTORY_MS` | 1200 | Covers the 300/400 ms rewind cap, delivery and resends. |
| `MAX_REWIND_MS` / `HIGH_LATENCY_REWIND_MS` | 300 / 400 | The cap applies to the **view lag**: how old the victim pose was at the attacker's hit, measured on the server's clock maps and never from the client's `age_ms`. |
| `REWIND_SLACK_MS` / `REWIND_CEILING_MS` | 40 / 600 | The cap grows to `honest_lag + tol + 40`, up to 600 ms: a 300 ms RTT attacker still lands hits, a view far behind its path does not. |
| `DELIVERY_CREDIT_MS` | 130 | The total age when judged (view lag + claim delivery) may exceed the cap by one resend or RTO plus a Lua tick. Spikes beyond that favour the defender. |
| `FUTURE_MS`, `ATTACKER_LEAD_MS` | 150, 300 | A clamped view time may lead the newest victim sample by 150 ms. The attacker's ts may lead its stream by 300 ms; beyond that the claim is a `ts_future` reject. |
| `ARRIVAL_LEAD_MS` / `ARRIVAL_LATE_MS` | 50 / 150 (+2·jitter) | `attacker_ts` mapped through the clock map must agree with the claim's arrival, net of `age_ms` (the game's `lage_ms` plus the sidecar's resend age). |
| `FRAME_MS` | 17 | One displayed frame (servo lag) in the view prediction. |
| `TOL_FLOOR_MS` / `TOL_MAX_MS` | 30 / 120 | Floor and ceiling of the ±(2·jitter_path + frame) window around the predicted view. The window uses the transport-bounded jitter, so a client inflating its own jitter cannot widen it. |
| `NET_JITTER_K`, `NET_JITTER_FLOOR_MS`, `NET_JITTER_WINDOW_MS` | 3, 15, 10 s | A stream's `jitter_p90` is built from the client's own timestamps, so a client that holds its pose packets controls it. The transport's RTT variation (`rttvar`, from its own acks, fed once a second by `note_net_jitter`) bounds what the network explains: K·rttvar + floor, the max over 10 s. ±J uniform jitter gives p90 lateness ≈ 1.8 J and rttvar ≈ 0.67 J. |
| `VICTIM_EXCESS_MAX_MS`, `LAG_SWITCH_FLAG_MS` | 250, 60 | `ViewPrediction::victim_excess`: the display delay the victim's jitter beyond the bound adds for every receiver. The rewind cap does not charge it (a victim that delays its own stream is hit where it was shown), up to 250 ms. Above 60 ms it bumps the victim's `LagSwitch` counter. |
| `VICTIM_JITTER_CHARGED_MS` | 120 | The most of a victim's own stream jitter (p90) the rewind cap charges, whatever its transport shows. A lag switch that also holds its acks inflates `rttvar`; this keeps it hittable. Honest p90 on typical and Wi-Fi links is 20–60 ms. |
| `INTERP_MIN_MS` / `INTERP_MIN_V2_MS` / `INTERP_MARGIN_MS` / `INTERP_MAX_MS` | 20 / 16 / 6 / 250 | The receiver jitter buffer. Skeleton stream: `clamp(p90 + 6, 16, 250)`. Older stream: `interval + p90 + 6`. |
| `DEFAULT_RTT_MS`, `UNKNOWN_RTT_SLACK_MS` | 120, 100 | Used before any RTT sample. The server tick feeds every peer's transport srtt to `note_rtt` once a second, and the damage_in → ack time adds samples. |
| `BODY_TOL` | 25 | Contact to victim capsule surface: 15 + 10 for the retarget and servo error of skeleton-driven stand-ins. |
| `BODY_TOL_V1` | 60 | An attacker without a blade stream sees PhysicsHandle stand-ins 6–78 uu off. 60 is the smallest value that reaches 97 %+ in the sim with 20 uu σ. |
| `UNARMED_TOL` | 10 | Fist, elbow or knee contacts must touch the attacker's **own** streamed capsules (exact geometry). The speed is that body part's. |
| `SWEEP_TOL` + `BLADE_RADIUS` | 15 + 3 | Contact to the swept blade. Sweep steps are ≤ 4 uu and arc-interpolated (Hermite on the streamed tip velocity, bounded at 25 % of the chord gap). |
| `DEGENERATE_BLADE_UU` | 10 | The weapon's tip scene sits about 1 uu from the grip on polearms and fists. The blade is rebuilt up the haft (far hand → near hand) or along the forearm at the class's nominal length; fists get no blade. |
| `ARM_REACH_MAX` | 70 | Shoulder → hand. The stock arm is about 58 (30 + 28); +20 % allows for ragdoll stretch. Beyond that is a reach cheat. |
| `validate::pose::geom` | per class | Longest catalogue member +15 % for the blade length, plus grip distance (sword 125/45, polearm 260/220, ...). |
| `MAX_STRIKE_SPEED` | 4500 cm/s | Measured speeds are capped here. A faster stream is lying to raise its damage cap. |
| `SPEED_FRAMES` / `PEAK_FRAMES` | 2 / 3 | Contact speed: with a blade stream, the contacting point's velocity at the first sweep step that touches; without one, the max over 2 frames. The peak speed over the 3 frames before the contact bounds the impact inputs (§7). |
| `CLASH_TOL` (+2·blade radius) / `CLASH_TOL_FALLBACK` | 10 (16) / 40 | Two swept blades within this over ±1 frame validate a clash. |
| `CLASH_WINDOW_MS` / `CLASH_AFTER_MS` / `CLASH_AFTER_OWN_MS` | 150 / 17 / 5 | A validated clash cancels a hit if its time on the attacker clock is between 150 ms before the cut and 1 frame after it. A blade that met the parry *after* it cut is not cancelled. Only the **victim's** report cancels (the attacker's own report still counts as a clash), and not when the victim also reported a `touch` from that attacker from 17 ms before its clash up to 150 ms after the hit (`TOUCH_BEFORE_CLASH_MS` / `TOUCH_AFTER_MS`): a bind that slid into the body landed. |
| `CLASH_BUCKET_CAP` / `CLASH_REFILL_PER_S` / `CLASH_BUCKETS_MAX` | 5 / 10 / 64 | The client reports every 100 ms during a contact. The per-reporter map is bounded, and only peers with history count. |
| `DISTRUST_INVALID` / `DISTRUST_WINDOW_MS` / `DISTRUST_MS` | 4 / 3000 / 5000 | Parry spam: four geometrically impossible reports in 3 s and the reporter is ignored for 5 s. |
| `CLASH_BLADE_SPEED_MAX` + reporter plausibility | 6000 uu/s | A clash is valid only if the reporter's own stream is physically held at its clash time: no reach violation (arm, blade length, grip), no blade teleport within 2 frames (tip faster than 6000 uu/s between samples), the blade's hilt within the class's grip distance of a hand (polearms along the haft), and without a blade stream the weapon actor within blade + grip of a hand. Limit: a blade snapped into place once and then *held* is, from the next frame on, geometrically indistinguishable from a real block. |
| `POSE_ROOT_MAX`, `PELVIS_SPEED_MAX`, `BODY_EXTENT` | 250 uu, 2000 uu/s (+30), 150 uu | Pose samples whose pelvis is more than 250 uu off the sender's validated root at that ts, or moved faster than 20 m/s since the previous sample, are not recorded; nor are capsules or a blade base beyond root + 400 uu. A skeleton streamed next to the victim while the root stays put is never judged. Counter `Teleport`. The root itself is capped at `validate::input::ROOT_SPEED_MAX` 15 m/s · dt + `ROOT_BURST` 3 m. |
| `ROOT_COVER_LEAD_MS` | 500 | Samples are recorded only while a timestamped root sample covers them; the root stream may lag the pose stream by this much. |
| `CLASH_JUDGE_AFTER_MS` | 120 | Eager clash judging, once both blade streams can have arrived. |
| `GRACE_MIN/DEFAULT/MAX_MS`, `IPC_MS` | 120 / 200 / 350, 40 | Defender hold: rtt_v + display delay + 2·jitter + IPC. That is when the victim's clash report can arrive. |
| `PARRY_RELEASE_UU` | 26 | A held hit is released early once the victim's own stream shows its blade was never near the attack while the attack was on its screen. |
| `PARRY_PLAUSIBLE_UE` / `PARRY_NEAR_CONTACT_UE` | 70 / 160 | When to hold at all. The victim blade's distance at view time does not separate real parries from other hits (`--parrystats`), so holds are common, and the attacker gets its `CONFIRM` at acceptance (≤ RTT) instead of after the hold. |
| `RATE_*` | 500 ms buckets ×16, ≥ 2 s span, > 5 %, fit residual ≤ 12 ms, sticky 10 s | Speedhack: the Theil–Sen slope of the arrival lower envelope. Honest PC clocks drift < 0.05 %. A route change is a step, not a slope, and the residual guard rejects it. A victim flagged this way forfeits the rewind cap; otherwise everybody's buffers display it late and it becomes unhittable. |
| `TRADE_WINDOW_MS` / `TRADE_SERVER_TTL` | 150 / 1 s | A dead attacker's hit lands if it was swung no later than 150 ms (its clock) after its death. |

### Engine (`combat.rs`)

The verdict for `(attacker, hit_id)` is made once, on first arrival, and cached. Resends never
re-validate; they only re-forward to the owner (while it has not acked) or repeat the verdict. The
owner's sidecar dedups by `(attacker, hit_id)` and always acks, so a hit is applied at most once.

| constant | value | why |
|---|---|---|
| `WAIT_MAX_MS` | 80 | A claim can overtake the attacker's own pose stream (reliable vs unreliable channel). It waits for the stream sample of its hit frame instead of being judged against an older blade. |
| `BUCKET_CAP` / `BUCKET_REFILL_PER_S` | 20 / 20 | Accepted claims per attacker. With one claim per contact, honest players stay below 7/s per target. |
| `CLAIM_BUCKET_CAP` / `CLAIM_REFILL_PER_S`, `MAX_DECISIONS_PER_ATTACKER` | 40 / 40, 256 | Every first arrival of a claim pays a token before any validation. Over the budget or the decision cap it is dropped silently (`Verdict::Ignore`, counter `ClaimFlood`). |
| `LAG_WINDOW_MS` | 350 | Maximum `age_ms` of a claim. |
| `FORWARD_TTL`, `DECISION_TTL` | 1.5 s, 10 s | Re-forwarding and sticky decisions. |
| `BOOKKEEPING_FIELDS` [15, 17], `HEALTH_DELTA_CAP` 150, `DELTA_ABS_CAP` 500 | — | Delta rows (legacy claims only) are sanitised, never a reject reason: bookkeeping fields, unknown indices and NaN are dropped, magnitudes clamped. Fields 15 and 17 hold the hit's DRS (thousands); a per-field check on them once rejected every hit. |

Reject reasons begin with a stable code (`code: details`), mapped to the `damage_reason` enum of the
`damage_verdict` record: `no_ts`, `ts_future`, `ts_old`, `ts_inconsistent`, `no_clock`, `no_history`,
`future`, `rewind_cap`, `body_miss`, `blade_miss`, `reach`, `no_cover`, `clock_rate`, `parried`,
`rate_limited`, `bad_field`, `attacker_down`, `target_down`, `self_hit`, `no_target`, `not_live`,
`stale_round`, `too_late`, `range`, `round_over` and `expired`. The sidecar adds `timeout` (no final
verdict within 2 s) and `queue_full` (256 claims pending). HSMPCombat counts rejects by these codes.

## 4. Damage formula (`validate/damage.rs`), derived from the game

Decompiled from Willie_BP "Get Damage" and "Deal Complex Damage" and ModularWeaponBP "Collision
Hit" with kismet-analyzer (see [../halfsword/README.md](../halfsword/README.md)).

```
Raw    = R_eff · |HitVel|            R_eff = rig_tag · quality(≤1.1) · (1 + align·(M−1)), M ≤ 1.666 → ≤ 2.333×
HitVel = max(|weapon COM velocity| cm/s, |normal impulse| kg·cm/s)
armour (layers summed): E = CP·V·R, resist = (DefCut·(1−s) + DefStab·s)·1000, frac = max(0, E−resist)/E,
         Raw = max(0, R·V·(1−frac) − 100·DefBlunt) + R·V·frac
DRS    = 2 · hm(0.875..1.125) · Raw · GI damage rate(1)
Health −= DRS · kH[part] · 0.00025   only if DRS > 1000 (or Inside / stab)
         kH: head 15, neck 7.5, upper torso 5, lower torso 2.5, arms/legs 0.1 (limbs lose <Part> Health instead)
Snap Neck (Health = 0) once Neck Health ≤ 25 and the hit is strong.
```

Get Damage never takes Health below the hit part's floor (head 1, neck 2.5, upper torso 5, lower
torso 10, limbs 15; a hit at or below the floor leaves Health unchanged), and contacts on one bone
within 0.1–0.2 s gate each other. Fights are decided by Snap Neck, dismemberment, consciousness and
blood loss, not by summed Health loss. That is why the HUD shows CON / BODY % / BLEED
([hud.md](hud.md)) and why the server never kills from its own damage estimate (§8).

The server's Health-loss cap for a claim is that formula with every unknown at its worst case:

- **Rigidity** is the highest `rig` tag of any module of the class, pommel and guard included (a
  pommel strike or a mordhau is a sword blow): sword 1.05, dagger 1.05, axe 1.25, blunt 2.0,
  polearm 1.45, shield 1.0. Unarmed is 1.0 and unknown is 2.5 (traps).
- **Quality and alignment** are taken at ×1.1 (best quality) and ×2.333 (blunt alignment).
- **Velocity** is the server-measured striking speed, × `SPEED_TOL` 1.0.
- **Impulse factor** (`hit_vel_factor`): dagger 1.2, sword 1.8, axe / blunt / polearm / shield 2.5,
  unarmed or unknown 3.0. Not calibrated against the game yet; the procedure is in §7
  ("Calibrating `hit_vel_factor`").
- **Armour** is the weakest piece of each layer: padded 5/20/2, mail 1/150/15, plate (gauntlets)
  10/200/75 (blunt / cut / stab).
- **Height** factor hm is taken at its maximum, 1.125.
- **Part:** the claim's bone selects the part. The result is multiplied by `MERGE_HEADROOM` 1.5.
- **Floor:** 0.5 HP (`LOSS_FLOOR_MEASURE`). Keep it small: accepted claims refill at 20/s, so a floor
  F lets a spammer book 20·F per second.
- **Snap Neck:** a lethal head or neck claim is allowed once the victim's vitals report Neck Health
  ≤ 35 (`SNAP_NECK_HEALTH`: the game's 25 + 10 for the hit itself and vitals lag).

## 5. HSMPCombat client (`mods/HSMPCombat`)

UE4SS runs a hook on a Blueprint function **after** the function body; the "post" callback is never
called. Nothing can change a Blueprint call's inputs before it runs. Every design choice below
follows from that.

- **Claims** come from the "Deal Complex Damage" callback on a stand-in whose Collided Component is
  mine (weapon or body): the armour-stage inputs as the game passed them (claim flag bit 5,
  `FLAG_COMPLEX`), no delta rows. Nothing is measured on the stand-in. Every call that passed the
  stand-in's own Deal Complex Damage gate (|Hit Impulse|·(Cutting Power+1) at least the last one on
  that bone within 0.1 s; read back from `Last Complex Damage Impulse` and `Last Complex Damage Bone`
  after the call) is a claim, in order, at most 4 per bone and tick (`BF.MAX_PER_BONE_TICK`). That
  gate runs on my screen exactly as it would on the victim in solo, so these are the calls that reach
  Get Damage in solo: a graze followed by a harder frame lands twice there and here, a weaker frame
  inside the window never. Without readable gate state, a tick's calls on one bone give the strongest
  (Rigidity·|Hit Velocity| first). Each claim carries the game's claim id `cid`
  and `lage_ms` (time held in the game). `attacker_ts = floor(os.clock·1000)` in the callback (the
  pose clock); `victim_view_ts` / `victim_arm_ts` come from HSMPAvatars' `playback` bus key (what the
  stand-in was displaying). These are physical sample times on the owner's clock,
  including the sender physics step. The servo quantizes the time to whole milliseconds
  before constructing its targets, so the existing u32 fields carry that exact time;
  publication never rounds a fractional target or subtracts the selected frame's step.
  Lag compensation samples that time directly using only the frames relayed to the
  attacker, and refuses missing delivered endpoints or relay history. Sender frame-start
  stamps remain the relay/cache keys and clock-estimator inputs. Deploy the Avatar and
  server changes together: an older Avatar publishes a different timestamp meaning.
  A Get Damage callback is never a claim.
- **Verdicts** arrive as `damage_verdict` events: `CONFIRM` the first time the server accepts the
  hit (forwarded or held), `FINAL` with ok or a reason code, and `CLASH` for a validated clash. They
  feed the log and the counters below.
- **`combat_quality`** (hsmp_log event) every 5 s while a round is live: `{claims, accepted,
  rejected_by_reason: {code: n}, confirmed, clashes, pending, round, window_s}`. A quiet window is
  reported too (claims 0).
- **Stand-ins take no native damage.** `Invulnerable` (Get Damage's first gate) and their weapons'
  `Temp Disable Damage` (Collision Hit's gate) are re-asserted every tick; anything that still lands
  is put back from the tick's baseline in the callback. Only real damage (a part health down, a
  bleed up) counts, so the game's own regeneration is never "put back". A weapon a stand-in dropped
  and I pick up gets its gate lifted. The put-back keeps the stand-in's Deal Complex Damage contact
  gate (`Last Complex Damage Impulse` / `Bone`) as the call left it: that gate decides which of my
  calls are claims.
- **Team.** A stand-in spawns with the local player's `Team Int`, and Collision Hit treats a
  non-zero matching team as friendly fire: Cutting Rate 0 and Hit Velocity / Impulse ×0.1, so every
  blow would be a weak blunt touch. Each stand-in therefore gets its own team (100 + peer), and mine
  only when the session roster says we are teammates.
- **Hit FX.** Accepted hits on other players arrive as `hitfx_in` and are replayed natively on my
  stand-in of the victim for blood, wounds, bruises and sounds; its damage, reaction, life flags and
  contact gates are put back at once and its Health re-pinned. Dismemberment comes from the vitals
  mirror, not from the replay (stand-ins have Force Disable Dismemberment).
- **Victim.** A stand-in's blow that lands on my pawn locally (its body, or a weapon whose gate
  leaked) is undone in the callback from my baseline (refreshed every tick and after every
  legitimate hit) and reported as a `touch`. The server-validated replay through my own Deal
  Complex Damage is the only damage another player does to me, reaction included. Replays arrive
  bunched (parry holds, jitter, resends), so the replay sets the game's gates as solo would have them:
  `Last Complex Damage Impulse` is cleared (the claim already passed that gate on the attacker's
  screen), and Get Damage's per-bone `Last Damage Taken` is kept only after a blow of the same
  attacker on the same bone less than 0.2 s earlier on the attacker's clock (`BF.GD_GATE_MS`, the
  game's RetriggerableDelay); otherwise it is cleared. When it is kept but the replays arrive more
  than 0.2 s apart on my clock, the game's own reset has already cleared it: the replay writes the
  earlier blow's value and bone back, so the gate holds as in solo.
- **Where the blow lands.** Deal Complex Damage maps the hit point into the hit bone's space and
  traces the armour layers covering that spot (layers stack). The claim carries `offset`, `normal`,
  `velocity` and `impulse` in the hit bone's frame (flag bit 6, `BF.LOCAL`; from
  `GetSocketTransform`, rotation removed, offset divided by the bone scale), and every replay turns
  them back with its own bone. A world offset re-added to a victim that turned or leaned since the
  attacker saw it put the blow somewhere else on the body: under the helmet instead of the open face,
  on the back plate instead of the gap, or inside the body where the trace finds nothing.
- **Stand-in body.** A stand-in wears the owner's armour exactly: the same slots and proxy
  collisions with the same Def tags (measured in game, combat-parity.md §3). As a pooled Willie it
  kept the foe's passport body: Height Rate, Muscle Rate, `Mass Scale (Set in BP)`, and bone masses
  up to 2.6× the owner's. The replay's damage uses the victim's own body, but the normal impulse
  the attacker's blade gets from the stand-in does not, and Hit Velocity takes the larger of that
  impulse and the weapon's speed. HSMPCombat (`standin_body.lua`) now writes my own body into the
  `body` record every 2 s (a new version only when it changed: rates, BP mass and character
  scales, one row per simulated body with its mass in kg and mass scale). The record travels only
  on connections that negotiated `caps::BODY` (beta.4 peers neither send nor receive it; kind
  0x0515, stream 0x89, newest version per owner, replayed to capable joiners). Once a second each
  stand-in gets its owner's rates and every bone whose mass differs by more than 1 % gets the mass
  scale that gives it the owner's mass (`SetMassScale` on its `Mesh`, computed from the mass it has
  now, so armour weight and bone size are included). A mass the game puts back is set again and
  counted in the log line. The geometry (Character Scale, the stand-in's height) is carried but not
  applied: HSMPAvatars measures the stand-in's bone offsets once per drive, and a mesh rescaled
  under it would stretch every joint.
- **What struck.** Flag bit 7 (`BF.WEAPON`): the striking component was a weapon. The replay passes
  the attacker's weapon (its first collision component; my own for the hit fx of my blow) or its body
  mesh as Collided Component: Get Damage reads the 'Weapon' tag (consciousness of light blows,
  `Last Hit By Weapon`).
- **Clash reports.** Weapon-on-weapon contact between my weapon and a stand-in's (Collision Hit) is
  sent as `clash`, at most every 100 ms per peer.
- **Death.** My own native Death / Dying sends a `death_report` at once. A server `death` about me
  in the current round forces my pawn dead. Stand-ins that play a death are listed on the
  `standin_dead` bus key for HSMPAvatars.
- **Spawn heal.** Career-save wounds are applied on spawn (the Tavern normally heals them). On every
  new local pawn and at each round's live start, the damage fields are restored from the
  Willie_BP CDO. The save is never written.

## 6. Server glue (`server/src/server/combat_glue.rs`)

Vitals take a per-sender budget (`rate::Kind::Vitals`, 40 burst, 25/s; an honest owner sends
≤ 20/s), are relayed only when their seq is newer than the last one relayed for that peer
(`combat::vitals_relay_fresh`), and fan out through the relay's per-recipient bandwidth budget
(`relay::Stream::Vitals`).

The early `CONFIRM` is not final; the sidecar keeps resending. The `FINAL` verdict follows the
victim's ack. A validated clash produces a `CLASH` verdict (`hit_id` 0) to both players; clashes are
judged on the server tick (`lagcomp::judge_due`). On the first forward of a hit the glue books it in
the ledger. Cosmetics wait for a fresh authenticated `REPLAY_CHANGED` owner outcome with
changed injury fields; `damage_ack` alone is insufficient. Then `hitfx_in` goes to other
players with `caps::HIT_FX`, the attacker included. Duplicate receipts, refused native
attempts, suppressed origins and bookkeeping-only changes cannot paint a wound. A hit forwarded after the
round ended or the target died is answered `round over / target down`.

Stand-ins keep native `Force Disable Vertex Paint=true`, which gates DCD armour paint
before Get Damage's invulnerability gate. Accepted cosmetic replay temporarily opens
this guard, restores it even on Lua/native errors, and discards wrappers if the world
guard dropped during replay. Probe damage sampling remains enabled separately. This
uses the owner's injury outcome to authorize approved-input cosmetic replay; it does
not yet copy the owner's vertex colors or paint-only outcomes with no sampled injury.

API notes:

- `lagcomp::record_blade(id, ts, Blade{base, tip, vel})` and `record_capsules(id, ts, &[Capsule])`:
  record the pose for `ts` first (`record_skeletal`). Degenerate blades (< 10 uu) are handled inside
  `record_blade`. Blade semantics: the striking segment from the grip side to the far tip, world
  space, sender ts = the frame start; capsules in stable order.
- `lagcomp::note_rtt(peer, rtt_ms)` and `note_net_jitter(peer, rttvar_ms)`: fed once a second from
  the transport by the server tick (`server/src/server/tick.rs`).
- `lagcomp::Store::clash_log` and `Info::parry_d` are there for a hit inspector.

## 7. Solo parity: the victim's replay is the damage

The claim is the armour-stage call my weapon made on the stand-in. The victim replays it natively:
HSMPCombat calls its own pawn's `Deal Complex Damage` with the server-approved inputs, so its own
armour, Willie height, part Health and Snap Neck apply exactly as in solo play. No top-up and no
trim.

| `damage` field | carries (FLAG_COMPLEX) |
|---|---|
| `impulse` / `velocity` | Hit Impulse / Hit Velocity (pre-armour; bone frame with bit 6, lengths unchanged) |
| `offset` / `normal` | hit point minus the bone, and the impact normal (bone frame with bit 6) |
| `location` | world hit point on the attacker's screen (lag comp) |
| `cutting_power` / `draw_cut` | Cutting Power / Draw Cut (pre-armour) |
| `pain_rate` | Stab Rate |
| `damage_out` | Rigidity |
| `raw_damage` | the stand-in's measured relative speed (uu/s) |
| `dism_blunt` | Blunt Destruction Int \| Kick·10 << 8 \| Lower Threshold << 16 \| Extra High Velocity << 17 \| fist source << 18 \| left-hand source << 19 \| right-hand source << 20 |

Source bits preserve the native striking component independently of geometry. Fists retain the weapon flag for native replay, but the server validates them against the attacker's body capsules and body velocity. A named left or right weapon source uses that hand's history and replay component; it cannot borrow the other hand's geometry. Both hand bits set, source bits without the weapon flag, and undefined packed bits are rejected. Legacy claims without source bits retain their previous selection behavior.

**Server bounds (`damage::clamp_complex`).** The inputs are the attacker's own native call, so they
pass unchanged unless physically impossible:

- **Servo over-read (down only).** When the attacker has a blade stream (`Info::rel_exact`) and the
  stand-in's relative speed exceeds both the attacker's peak striking speed and lag comp's relative
  speed (the attacker's striking point against the victim's *replicated* body), the stand-in's servo
  rushed into the blade. Hit Impulse (and Hit Velocity when the impulse set it) is scaled down to
  match.
- **Hard ceilings.** Hit Velocity ≤ max(peak striking speed, `hit_vel_factor`·relative speed); Hit
  Impulse ≤ `IMP_CEIL_K` (12) × that base. Rigidity is capped at the class maximum (with the blunt
  alignment bonus only for low cutting power), Stab Rate to 0..1, Kick to ≤ 10, the blunt destruction
  level to 0..6, Extra High Velocity is cleared (no projectiles), Cutting Power to 0..200 and Draw
  Cut to 0..400.
- A two-way rescale toward lag comp's speed was tried and removed: it moved real blows by up to 12×
  either way in game (a rubber-banded frame has Hit Velocity = 0.1 · impulse; the instantaneous blade
  speed at the touch reads a blade already stopping in its target).
- Without a blade stream the striking speed is the weapon or hand speed × the shoulder-pivot lever
  `|contact − shoulder| / |grip − shoulder|` (1..`LEVER_MAX` 6, `LEVER_FALLBACK` 2).
- **Ledger booking** (`combat::hit_loss`) for an armour-stage claim is the server's own replay
  estimate, `damage::replay_loss` at `HM_MIN` 0.875 through the victim's armour. It must never exceed
  the real replay (§8).

**Calibrating `hit_vel_factor`.** The factor bounds how far an honest Hit Velocity (the larger
of the weapon's COM speed and the contact's normal impulse, which scales with both bodies'
masses) can exceed the relative speed. It is a measured quantity with two parts: the game's
physics ratio |Hit Velocity| / relative speed, and the server's error on the relative speed. The
tool is in `crates/hsmp-combat-sim` (`src/calib.rs`, tests in `tests/calib.rs`, which also prove
that its ceiling is the server's `clamp_impact_ex`):

1. **Offline, network part.** `cargo run --release -p hsmp-combat-sim -- --hvf typical --seeds 3`
   (and `wifi`): per class, the factor every honest armour-stage claim needs against the server's
   relative speed, the share the shipped factor cuts, the share it would cut against the true
   relative speed, and `net p99` (true / server relative speed at p99). The sim's physics is the
   shipped factor itself (`game::draw_contact`), so the sim measures the server's error, not the
   game.
2. **In game, physics part.** Dev deploy (HSMPParity on), both instances in an MP session, real
   fights per weapon class (or `parity near` / `swing`): HSMPParity logs every Deal Complex Damage
   the game makes (`DCD on ... by <class> wp=<weapon actor>: |vel| |imp| rel`, up to 3000 per
   session), and the server logs `combat: impact rescale` with `class`, the claimed Hit Velocity,
   the peak and the relative speed (5 lines/s budget). Also fight the same weapons in solo with
   HSMPParity on (its DCD lines need no MP).
3. `cargo run --release -p hsmp-combat-sim -- --hvf-logs UE4SS.log server.log ...` prints the same
   table for `Game` (solo / attacker-screen physics) and `Server` (what the ceiling really saw)
   samples. A class gets a recommendation (p99.9 of the needed factor × 1.1) only from 200 binding
   samples.
4. A candidate factor must also keep `cargo test -p hsmp-combat-sim --test combat_sim
   cheats_rejected` green: a looser ceiling is exactly what `DamageInflate` exploits.

First numbers (sim, 3 seeds; no in-game DCD capture yet, so the factor stays as shipped):

| profile | class | claims | shipped cuts | ... against the true rel | ... with `hvf · max(peak, rel)` | net p99 |
|---|---|---|---|---|---|---|
| typical | sword | 476 | 5.0 % | 1.3 % | 0.8 % | 2.06 |
| typical | axe | 166 | 3.0 % | 1.2 % | 0.6 % | 2.04 |
| typical | blunt | 139 | 8.6 % | 2.2 % | 2.9 % | 2.05 |
| typical | polearm | 299 | 6.0 % | 1.3 % | 2.3 % | 2.41 |
| wifi | sword | 472 | 3.6 % | 1.5 % | 0.9 % | 2.02 |
| wifi | polearm | 283 | 7.1 % | 1.1 % | 1.1 % | 3.33 |

Most of the honest blows the ceiling cuts are cut by the server's relative speed (lag comp's view
of the victim's body), not by the physics. The looser form `hvf · max(peak, rel)` was tried: it
spares most of them, but `cheats_rejected` then lets 14 of 187 `DamageInflate` claims through
(0.5 relative-speed floor of the peak: 3, 0.7: 5; the bar is 1). The ceiling therefore stays
`max(peak, hvf · rel)`; a better relative speed from lag comp is the way to recover those blows.

**Parity (sim, `damage_parity_with_solo`).** A paired Monte Carlo median of hits-to-kill per weapon
class × victim armour × zone. A cell needs ≥ 20 contacts and a solo TTK within the cap; at least 45
cells must be checked. MP/solo must be within ±10 % on typical and wifi with a blade stream, ±35 %
with PhysicsHandle stand-ins (20 uu servo noise). Scripted-blade teleports (> 4500 uu/s) are not
counted as contacts.

**Offline proof in Lua.** `hsmp-tools lua-test damage_parity` runs the real HSMPCombat `main.lua`
against a model of the decompiled Deal Complex Damage → Get Damage (bone frames, armour coverage by
spot, both contact gates, the weapon tag) with after-call hooks. See
[combat-parity.md](combat-parity.md).

## 8. Health replication end to end (`sim::health`, `health_replication_end_to_end`)

`Config::health_model` adds the victim's own game and the vitals path to the fight:

- the victim applies each forwarded hit once (its replay);
- the stand-in echo on its pawn is restored;
- HSMPCombat samples vitals every 2nd tick and in the replay tick (deadband, 1 s heartbeat);
- the sidecar `VitalsGate` sends them;
- the server runs the real `combat::Ledger` and relays the ledger-clamped record;
- viewers keep the newest (life, seq);
- deaths come from the reliable `death_report`, from dead vitals, or from the stall rule.

The checks:

1. HP lost after 5 hits matches solo (median within ±5 %, p10 ≥ 0.8, p90 ≤ 1.25).
2. Viewers show the victim's HUD value: never a value it did not have, never cut by the ledger, and
   promptly (p99 ≤ 150 ms, max ≤ 500 ms behind).
3. Nothing is applied twice, and the echo is restored. A fault-injection run (`--health`, "echo
   leak") proves the checker catches a leak.
4. A server death needs the owner's native death or the stall rule, and none are missed.

The run also covers a stalled owner (its Lua hung for 10 s while its sidecar keeps acking).

**The ledger** (`combat::Ledger`) per player and round:

- **Ceiling.** The owner's report is what peers are shown. Only healing is bounded: once a hit was
  forwarded in this life, reported Health may rise at most `REGEN_PER_S` (1 hp/s) above the last
  accepted value; a faster rise is clamped and logged (`vitals: Health rose faster than any heal;
  clamped`). A report that stays high after estimated damage is not cut: the per-part floors and
  contact gates make that honest.
- **Stall rule.** Booked losses can declare a death only for an owner whose vitals stopped for
  `OWNER_STALL` (2.5 s; one lost 1 s heartbeat on Wi-Fi is not a stall).
- **God-mode bound.** Each life carries a bound: 100 (`HP_DEFAULT`, never the owner's own first
  report) + `GODMODE_REGEN_PER_S` (0.5 hp/s) − the *trusted* loss of every hit the owner surely
  applied (acked and `REFLECT_AFTER` 250 ms past, or forwarded `FORWARD_TTL` ago and never acked).
  Trusted loss is `replay_loss` at `HM_MIN` for armour-stage claims and `HM_MIN / HM_MAX` of the
  booking otherwise. When the bound stays ≤ 0 for `GODMODE_GRACE` (300 ms) while the owner reports
  alive, the server **flags** it (counter `GodMode`, one warning per life). It kills only when
  `HSMP_GODMODE_ENFORCE=1` is set on the server: because of the part floors and gates, an honest
  owner's replays can take less than the estimate, and an enforced rule kills honest players.
- **Repeats.** `VitalsGate` repeats every significant frame at +120 and +360 ms (`VITALS_REPEAT`), so
  one lost datagram does not leave peers stale until the 1 s keyframe.

Unit tests worth knowing: `ledger_ceiling_never_cuts_an_owner_that_reports_in_the_replay_tick`,
`honest_owner_at_full_health_after_estimated_hits_is_shown_and_kept` (combat.rs) and
`victim_touch_after_its_clash_keeps_the_hit` (lagcomp/tests.rs).

## Protocol 8 historical cutting shape

The 280-byte Damage header carries an exact ASCII native source class (48-byte field; capture rejects names of 48 bytes or more) and thirteen original HitBox values: center in physical centimetres in the victim bone's rotation frame, local quaternion, original world scale, and unscaled native extent. Capture subtracts the original bone position and applies inverse rotation; replay and server validation apply rotation and add the corresponding bone position. Neither operation divides by native socket scale or multiplies by the median skeleton reference scale. Those scales have different provenance, so normalizing with one and reconstructing with the other moves a legitimate Box. Other damage point and velocity coordinates retain their existing semantics. All 24 injury deltas fit the 472-byte ring payload exactly. Weapon claims require an original component ordinal, one source hand, and exact class history. Ordinal-zero weapon fallback is rejected.

The server reconstructs the historical cutting Box against authenticated original pose history, including victim bone frame, exact source class fingerprint, component ordinal, native Box scale, and scaled extent. Keeping scale separate prevents a forged reciprocal extent/scale pair from changing the native X-before-scale clamp. The original live weapon is never transformed or resized. Replay uses a private collision-disabled, nonsimulating Box pool bounded to 128 native actors per world, including partial construction failures. Session reconnect retains the pool; world travel retires its numeric identities. Native Shipping snapshots and deep-copies shape transforms/extents synchronously before asynchronous paint work, allowing bounded reuse while retaining the UObject for its world lifetime.

The tracked extra melee registry records thirteen original PAK classes independently verified as direct ModularWeaponBP descendants. The diagnostic command `autotest parity inventory extra` checks those separately from the configured 128 weapon classes. Catalogue presence and inheritance evidence do not establish successful native injury for every weapon. Thrown or ranged source objects still need separate authenticated object identity, and body-striker HitBox replay remains rejected where original native Box scale history is unavailable. The private factory command `autotest parity cutproxy` must pass native construction, readback, collision flags, and reuse checks before cutting PvP is considered verified.
