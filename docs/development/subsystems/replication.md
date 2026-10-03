# Character replication (pose, weapon, root)

Each player simulates only their own Willie. On every other screen that player is a
**stand-in**: one of the arena's natively spawned Willies, taken over by HSMPAvatars and driven
from the owner's replicated physical state. This document covers how that state is sampled,
sent, buffered and applied.

| Part | Where |
|---|---|
| Sampler and sender | `mods/HSMPSync` (policy), `crates/hsmp-native` `sample.rs` (native sampling) |
| Record builder and codec | `crates/hsmp-pose` (`sample`, `posecodec_v2`, `posecodec`) |
| Jitter buffer | `crates/hsmp-pose/src/poseplay.rs`, run by the sidecar (`server/src/sidecar/pose.rs`) |
| Server validation and relay | `server/src/server/pose_glue.rs`, `server/src/relay.rs` |
| Stand-in driver | `mods/HSMPAvatars` (targets, gains, metrics), `crates/hsmp-native` `servo.rs` / `neutralise.rs` (native servo) |
| Ground-truth analyser | `tools/pose-truth` (package `hsmp-pose-truth`) |

Game ↔ sidecar traffic goes through shared memory (see
[../ipc-shared-memory.md](../ipc-shared-memory.md)); game ↔ server traffic is the protocol v6
record transport (see [../protocol.md](../protocol.md)). There are no state files on this path.

## Pipeline

```
 sender game, every frame (HSMPSync fast_send, at send_hz, default 60)
   one sample, one timestamp: root + 23 bones + velocities + up to 2 weapons + control
   HSMPNative sample_local (native) or put_root / put_weapon / put_pose (Lua fallback)
     -> shm slots local_root / local_weapon / local_pose   (records, quantised once)
        │  sidecar ipc pump: the changed slots are framed as they are and sent together,
        │  so root + weapon + pose of one sample share one datagram
        ▼
 server (pose_glue.rs): record::view, input gate, stream rate, speed cap,
   one decode of the pose frame for lag compensation, then relay:
   the same bytes with WireHdr.peer = owner and aux = this pair's relay interval
        ▼
 receiving sidecar: root -> per-peer slot peer_root; pose -> poseplay::Playback (per peer)
   dedup / reorder / late-drop / cut / restart, clock offset + p90 jitter -> adaptive delay
   the hsmp-poseplay thread evaluates every peer every 2 ms -> per-peer slot peer_play
        ▼
 HSMPAvatars, every game frame: read peer_play, build joint-consistent targets,
   per-body velocity servo (HSMPNative servo_bodies / servo_weapon, or the Lua path)
```

`weapon` records are kept by the server for lag compensation only and are not relayed:
receivers get the weapons inside the pose frame.

## What is sent

One codec v2 frame per sample (`crates/hsmp-pose/src/posecodec_v2.rs`). The frame starts with the
magic `FF FF FF 02`. Only v2 frames are accepted on the wire: the server and the receiving
sidecar both decode with `posecodec::v2::decode` and drop anything else.

| Part | Encoding |
|---|---|
| time | `ts` u32 ms plus a 1/256 ms fraction: the sender's world real time, anchored to `os.clock` (sub-ms; `os.clock` alone is 1 ms quantised) |
| pelvis | position 3×f32, rotation smallest-three 15 bit |
| 22 other bones (`BONES`: spine_01..05, neck_01/02, head, clavicles, upper/lower arms, hands, thighs, calves, feet) | parent-relative rotation, smallest-three 13 bit, encoded closed-loop (each bone against the decoded parent). Positions come from FK over the SK_Body_Man reference offsets × a per-character scale `k` (u16), with an explicit offset only where the measured one differs by more than 0.1 uu (dislocations, stretch) |
| velocities | per body: linear (±10000 uu/s) and angular (±5000 deg/s), 8-bit µ-law per axis |
| weapons (≤ 2) | holding hand(s), class tag (djb2 % 255 + 1), position relative to the pelvis, rotation, linear and angular velocity, blade base (`Root Scene`) and tip (`TippyTipScene`) in weapon space. `Weapon_Fists_C` (a non-simulating pseudo-weapon far from the hand) and anything not simulating within 150 uu of the hand are not sent |
| control (every 2nd frame) | 32 BP flags, 2 grip types, 16 scalars, aim, control rotation, 4 IK vectors (`R/L Out End/Joint Pos`) |

Size: about 308 B without the control block and 355 B with it, plus the record and transport
envelope: roughly 23–25 KB/s per peer at 60 Hz.

`crates/hsmp-pose/src/sample.rs` is the one place the records are built from the game's numbers
(rotator → quaternion, whole-ms `ts`, the codec frame). The native sampler and the Lua fallback
both call it, so their records are byte-identical (`sample::tests`).

The 16-bone `posecodec` (v1) module is still in the crate: its `HERO_BONES` index table is used
by lag compensation and by the interaction channel (`bone` < 16). No sender produces v1 frames.

### Native sampling

`HSMPNative.sample_local` reads the 23 socket transforms, the weapon transforms and the control
block through `ProcessEvent` and writes the three slots directly. HSMPSync keeps the policy:
when to sample, death and world checks, and the fallback to the Lua sampler. It is on by default.
Switches: settings key `"native_sample": false`, env `HSMP_NATIVE_SAMPLE=0`, or
`hsmp-tools ipc-ctl --pid <game> tune native_sample 0`. The 5 s `pose sender` log line names
the sampler in use. The rules every native UObject access follows (game thread only, weak handles,
world epoch, no Soft* access, exact BP names) are in
[../ipc-shared-memory.md](../ipc-shared-memory.md) §8.4.

## Server side

`pose_glue.rs` handles `root`, `weapon` and `pose`:

- `root`: validated (`validate::input::check_root`: finite, inside the world, speed cap), rate
  limited per sender, recorded for lag compensation, relayed.
- `pose`: must decode. The one decode feeds lag compensation: hero bones, the exact blade
  (`lagcomp::record_blade`: grip, tip, tip velocity) and capsules for all 22 bodies
  (`lagcomp::record_capsules`). A frame lag compensation refuses (pose not tied to the root) is
  not relayed.
- Relay (`relay.rs`): encode-once fan-out with a per-recipient rate plan. The two nearest
  players' skeletons go at the sender's full rate and never below 30 Hz; farther ones are thinned
  (floor 7.5 Hz). Root goes at 30/20 Hz (floor 10 Hz). The plan fits into the per-client
  downstream budget (`--client-budget-kbps`, default 128 KB/s), farthest first. Each relayed
  pose carries the pair's current relay interval in `aux`, so the receiver sizes its buffer for
  it.
- Congested paths: every plan (500 ms) reads the transport counters of each recipient's
  connection. A standing queue (smoothed RTT more than 120 ms over the path's minimum), or more
  than 15 % loss together with 30 ms of queue, cuts that recipient's budget ×0.7 (at most once a
  second, floor ¼ of the budget). Below 60 ms of queue and 3 % loss it grows back 5 % per plan.
  Random loss alone changes nothing. This is what keeps a listen host on a home upload playable:
  8 players at the full budget need about 7 Mbit/s of upload, and without it the router queue
  grows until everyone's latency does. The base RTT drifts up slowly, so a lasting route change
  is not taken for a queue for more than about a minute.

## Jitter buffer (`poseplay.rs`)

25 slots per peer (23 bones, `weapon_r`, `weapon_l`).

- **Ordering.** Sorted insert by sender `ts`. Duplicates are dropped. A frame whose `ts` is
  already played is dropped as late. A `ts` jump back of 2 s or more (`RESTART_BACK_MS`) means
  the sender restarted: the stream is reset. Lag compensation uses the same threshold.
- **Clock.** Offset = min(rx − ts) over the last 2 s (tracks drift). Jitter = p90 of
  (rx − ts − offset). p95 latched the buffer at its cap for seconds after every Wi-Fi spike; p90
  rides those out with brief extrapolation or hold instead.
- **Delay.** `clamp(p90 + 6 ms + max(0, interval − 17 ms), 16, 250)` (`DELAY_MIN_V2_MS`,
  `DELAY_MARGIN_V2_MS`, `V2_EXTRAP_OK_MS`; the interval part is capped at 66 ms). That is about
  one frame on a LAN at 60 Hz. A stream the relay decimated to 30 / 15 / 7.5 Hz buffers the part
  of its frame interval that extrapolation cannot cover, so it interpolates between received
  frames instead of extrapolating. The interval is the relay's `aux` when present, else the
  median spacing of the newest frames. The delay grows fast and shrinks slowly.
- **Playback clock.** Advances with local time, slewed at most ±10 % toward
  `now − offset − delay`. Hard re-sync only when more than 250 ms off. It never runs backwards.
- **Sampling.** Hermite per bone with the sender's velocities as tangents (no finite
  differences), slerp for rotations, up to a 270 ms gap (`HERMITE_MAX_GAP_V2_MS`; a 7.5 Hz relay
  is 133 ms). Past the newest frame: extrapolation with the sender's velocities, including
  rotation, at full velocity for 34 ms (`EXTRAP_FULL_MS`) and then easing out, at most 100 ms and
  30 uu per bone, then hold. Newest frame older than 1 s: `stale`.
- **Look-ahead.** HSMPAvatars writes the `pose_lead` slot (≤ 120 ms, `LEAD_MAX_MS`): the poses
  are evaluated where the bodies should stand after the coming physics step, interpolated from the
  buffer where possible, with sender acceleration applied for up to 60 ms.
- **Cuts.** A pelvis jump of more than 300 uu at more than 3000 uu/s (respawn, round reset,
  teleport) drops all older frames, shows the new place at once and increments `cut`.
  HSMPAvatars snaps the mesh when `cut` changes.

The evaluated result is the `peer_play` record (`crates/hsmp-ipc/src/schema/pose.rs`
`PeerPlay`): mode (`interp` / `extrap` / `hold` / `stale`), `cut`, the sender time `pt` the pose
was evaluated at, age, delay, jitter, lead, the frame interval `iv`, root, 25 slots × 13 floats
(position, quaternion, linear and angular velocity), weapon geometry and the control block.

## Driver (HSMPAvatars): per-body velocity servo

Kinematic driving was ruled out: PoseableMesh bone writes did not move the body,
`SetLeaderPoseComponent` crashed, and kinematic bodies cannot be hit natively. The stand-in stays
a physics body and each body is given a velocity every frame.

- **Servo.** Every game frame (game thread), for each of the 22 simulated bodies of the
  stand-in's `Mesh`: the target is the played-back sample. The COM velocity that lands the body
  on the target in one step is displacement / dt; the angular velocity comes from the quaternion
  error. The sender's velocity is fed forward (COM: v + ω×r) and only `gain` of the remaining
  error is corrected (`SERVO_GAIN` 0.3; legs 0.5), with the correction beyond the feed-forward
  capped at 900 uu/s and 900 deg/s, so nothing is launched. dt is the last frame's dt, at most
  1/30 s.
- **Native servo.** Three stages, each on by default with its own switch (settings key
  `false`, env `HSMP_<KEY>=0`, or dev tune `<key> 0`):

  | Stage | Key | What runs natively |
  |---|---|---|
  | body servo | `native_servo` | the body loop (22 × GetSocketTransform + the two velocity sets) as one call, `HSMPNative.servo_bodies`, the same math as the Lua `PURE.servo` bit for bit (`crates/hsmp-native/tests/servo_parity.rs`) |
  | neutralise | `native_neutralise` | the ~30 BP variable writes and the joint-motor call (`HSMPNative.neutralise`) |
  | weapon servo | `native_wservo` | a held weapon's root servo (`HSMPNative.servo_weapon`) |

  Lua keeps the targets, gains, the yield policy and every metric. Any refusal falls back to the
  Lua path for that frame. In two-instance gate runs the native stages cut stand-in frame cost by
  about 31 % and pose-sender cost by about 85 %, with pose quality and latency unchanged.
- **Weapons.** Held weapons (matched by class tag) get the same servo on their root body.
- **Gravity** is off on driven bodies and servoed weapons, re-asserted every tick (anything that
  rebuilds the physics state turns it back on).
- **The stand-in's own forces are neutralised** every frame and in a post-hook on
  `Willie_BP_C:ReceiveTick` (the BP re-sets them in its tick, before physics): `Head Tonus` 0,
  `Root Linear/Angular Constraint Power` 0, `Block * Muscles` true, pain and flinch values 0
  (`Pain` itself is left alone), joint motors (`SetAllMotorsAngularDriveParams` 0),
  PhysicalAnimation strength 0. Measured per body before each fix: head 40° off (tonus), 97° off
  (motors), a "broken back" (PhysicalAnimation pulling to the idle pose).
- **Muscle tone** is zeroed while driven (`All Body Tonus`, `Upper Body Tonus`, `Arm R/L Tonus`,
  `Leg R/L Tonus`, `Muscle Power`) and restored on release. With the BP defaults the stand-in's
  own arm control held a guard pose 30–50° off at the forearms whatever else was done; zeroed,
  the same pose tracks to 0.04 uu / 0.2°.
- **Grip constraints** (weapon BaseMesh ↔ hand_r/hand_l): SLERP drive off while driven, limits
  freed while that hand's weapon is servoed, restored on release.
- **Joint-consistent targets** (`PURE.fk_retarget`, knob `retarget`): each bone is rebuilt from
  its parent's target with the stand-in's own parent offset, so the servo never asks for a
  stretched joint. Joint limits stay the asset's. Half Sword's joints are named
  `UserConstraint_N`; `SetAngularLimits(bone name)` is a no-op on them.
- **Collision.** World geometry (WorldStatic/WorldDynamic) is ignored by the driven mesh: the
  owner's pose already contains his contact with his world. Contact with the local pawn and
  weapons stays physical, so local hits fire the native damage path (see [combat.md](combat.md)).
  `BoneCore` collision is off.
- **Release** (free native ragdoll) only on a server-declared death (the `standin_dead` bus key
  from HSMPCombat, or the owner's `peer_vitals` record), stale data for 500 ms, or a cut. A
  vitals "dead" flag that outlives a respawn is ignored, so a stand-in is never frozen for a
  round.
- **Interaction yield.** The typed bus key `pose_yield`
  `{ rows = {{peer, until_ms, gain, cap_lin, cap_ang}} }` (process clock `os.clock()*1000`)
  makes that peer's stand-in softer until `until_ms`. HSMPInteract writes it while a player holds
  or shoves a stand-in (see [interact.md](interact.md)).
- **Peers** are discovered from the sidecar's peer directory (`peer_dir` slot), with no fixed id
  range.

HSMPAvatars still contains the older PhysicsHandle driver (one handle per hero bone) as the
fallback for codec-v1 data. No current sender produces v1 data, so it does not run in normal
play.

### Background and minimized windows

Sender and stand-in meshes get `VisibilityBasedAnimTickOption = 0` (always tick bones), and
HSMPSync sets `t.IdleWhenNotForeground 0` while an MP session is live and restores the previous
value afterwards. A minimized sender keeps streaming at full rate; a minimized receiver keeps
driving and resyncs on restore. HSMPSync streams nothing outside a live session
(`shared/hsmp_session.lua`), so single-player is untouched.

## Lag compensation contract

Every stream's `ts` is the sender's sample time in ms, the same clock for root, pose and
weapon. The bus key `playback` gives, per peer, the sender time of what is on screen now and the
local time it was read. HSMPCombat stamps its hit claims with it, and the server rewinds the
victim to that label. Details: [combat.md](combat.md).

## Measuring fidelity

**In game.** HSMPAvatars emits `pose_quality {peer, arm_p95_uu, tip_p95_uu, latency_ms,
jitter_ratio, foot_slide_p95, idle_rms}` (hsmp_log) every 5 s per peer, skipped while the window
is not rendering. `latency_ms` is sample age + buffer + one servo frame − lead; it excludes
network transit. Contact impulses near the local pawn are logged every 5 s as
`x_pose_contact {peer, near_p50, near_p95, near_max, body, far_p95, n, yielding}`. The test gate's
rule POSE-1 checks `pose_quality` during Live: under `typical` netsim and better, arm p95 ≤ 5 uu
and tip p95 ≤ 8 uu; under `wifi`, arm p95 ≤ 10 uu; latency ≤ one-way delay + 40 ms
([../testing.md](../testing.md)).

**Ground truth.** With `HSMP_POSE_PROBE=1` the sender appends every sample to
`<state>/.probe_send.txt` and the receiver writes what its stand-in actually shows each frame,
labelled with the sender time it was driven to (`.probe_recv<id>.txt`). `tools/pose-truth`
matches them per bone and reports per motion class (idle, swinging, walking, walk+swing, falling,
recover): all bones, arm chain, hands and blade tip p50/p95/max, display latency, smoothness
ratio, planted-foot speed and idle RMS. The probe marks a run invalid when more than 5 % of frames
were not rendered.

Measured with the probe (two instances on one machine, Map_Arena_Pit, scripted owner motion, both
directions; p95 over each motion class, worst class shown, "A→B / B→A"):

| | LAN | netsim typical (50 ±12 ms, 1 % loss) | netsim wifi (35 ±25 ms, 2 %, spikes) |
|---|---|---|---|
| all bones p50 | 0.03–0.05 / 0.03–0.26 uu | 0.03–0.07 / 0.03–0.10 uu | 0.06–0.18 / 0.03–0.07 uu |
| arm chain p95 | ≤ 0.98 / ≤ 3.7 uu | ≤ 1.03 / ≤ 2.3 uu | ≤ 3.0 / ≤ 3.5 uu |
| hands p95 | ≤ 1.8 / ≤ 8.7 uu | ≤ 1.8 / ≤ 3.9 uu | ≤ 3.8 / ≤ 6.4 uu |
| blade tip p95 | ≤ 2.7 / ≤ 15 uu | ≤ 2.7 / ≤ 7.1 uu | ≤ 3.3 / ≤ 18.9 uu |
| display latency p50 (mean of both directions) | ≈ 31 ms | ≈ 87 ms (50 of it netsim) | ≈ 95 ms |

Offline, `hsmp-tools pose-probe --bins <dir> [--profile typical|wifi|bad|awful]` runs a real
server and two sidecars with synthetic 60 Hz senders and reports display latency, buffer delay
and hand error per profile.

## Known issue: fidelity on heavy maps

Remote fighters can look less precise when the receiving game's frame time is high. At long
frames the servo aims a whole physics step ahead and leans on look-ahead and extrapolation, so
arms and blade can trail or overshoot the owner's real position and feet can slide. In gate runs some
instance-rounds exceed the POSE-1 thresholds, mostly on the heavier arenas. Hits are not affected in the same way: the server judges them on
the replicated data and lag-compensation history, not on what the stand-in shows. Fast swings
also show hand error up to about one frame of motion (≈ 9 uu at 800 uu/s) even on a LAN.

## Tunables

| Where | Name | Default | Meaning |
|---|---|---|---|
| `.settings.json` | `send_hz` | 60 | sample rate of root + pose + weapon (10..120) |
| `.settings.json` | `native_sample`, `native_servo`, `native_neutralise`, `native_wservo` | on | the native stages (above) |
| HSMPAvatars | `SERVO_GAIN` / `PX.LEG_GAIN` | 0.3 / 0.5 | share of the remaining error corrected per step |
| HSMPAvatars | `SERVO_CAP_LIN` / `SERVO_CAP_ANG` | 900 uu/s / 900 deg/s | correction cap beyond the feed-forward |
| HSMPAvatars | `PLAY_STALE_MS` | 500 | release the stand-in when playback stops |
| HSMPAvatars | `SNAP_BODY_ERR` / `SNAP_BODY_MS` | 150 uu / 150 ms | rigid snap on a sustained pelvis error |
| HSMPAvatars | `POSE_RANGE` | 4000 uu | pose-drive only stand-ins this close |
| poseplay.rs | `DELAY_MIN_V2_MS` / `DELAY_MAX_MS` / `DELAY_MARGIN_V2_MS` | 16 / 250 / 6 | buffer bounds |
| poseplay.rs | `JITTER_QUANTILE`, `CLOCK_WINDOW_MS` | 0.90 / 2000 | jitter estimate |
| poseplay.rs | `EXTRAP_MS`, `EXTRAP_MAX_UU` | 100 / 30 | loss concealment |
| poseplay.rs | `CUT_DIST_UU`, `CUT_SPEED_UUPS` | 300 / 3000 | teleport detection |
| poseplay.rs | `SLEW_MAX`, `RESYNC_MS` | 0.10 / 250 | playback clock |
| sidecar/pose.rs | `PLAY_WRITE` | 2 ms | `peer_play` evaluation cadence |

**Dev knobs** (dev builds, `DEVCTL` capability): `hsmp-tools ipc-ctl --pid <game> tune <key>
<value>` pushes a `dev_cmd` TUNE record into the DevCtl ring; values last for the game session.
Flags (nonzero = on): `servo`, `wpn`, `world`, `ghost`, `clock`, `motors`, `grips`, `retarget`,
`tonus`, `stamp`, `noacc`, `plant`, `v1aim`, `tdiag`, `native_servo`, `native_neutralise`,
`native_wservo`. Numbers (clamped): `cap_lin` / `cap_ang` 0..20000, `lead` −500..500 ms,
`gain` / `leg_gain` 0..1, `lat` 0..500 ms, `limits` 0..180, `bench` 1..1000 (repeat the driver
N times to measure frame cost). Unknown keys are logged once and ignored. HSMPSync's timing
diagnostics: `ipc-ctl --pid <game> tdiag on|off`.

## Tests

- `cargo test -p hsmp-pose`: codec round trip of all fields, dislocation overrides, size bounds,
  malformed input; jitter buffer (dup / reorder / late, Hermite exactness, smooth clock under
  jitter, loss → extrapolation → hold → stale, teleport cut, sender restart, clock drift,
  decimated v2 streams). `POSE_V2_LINES=<file>` and `POSE_REPLAY=<file>` replay recorded motion.
- `cargo test -p hsmp-native`: native sampling against a fake engine (`tests/sample.rs`), servo
  parity with the Lua source (`tests/servo_parity.rs`).
- `hsmp-tools lua-test pose`, `lua-test avatars`, `lua-test sync`: the Lua sides with the typed
  HSMPNative mock.
- `server/tests/pose_fuzz.rs`: pose records from hostile input.
- `hsmp-tools pose-probe` and `hsmp-tools netsim --profile <name>` (see [../tools.md](../tools.md))
  for latency under impairment.

## In-game log lines

Sender (`[HSMPSync]`):
- `pose sender: sampling Mesh (CharacterMesh0) codec v2, 23 bones ...` once per pawn;
- every 5 s: `pose sender: 60.0 frames/s (target 60 Hz) codec v2, ... pose sample+write avg
  ... ms, sampler native (native N, fallbacks M)`.

Receiver (`[HSMPAvatars]`):
- `pose: stand-in Willie_BP_C_... drives <Field> ...` when a stand-in is claimed;
- every few seconds: `pose peer N: play mode=interp new=.../s ... driving=true weapon=... |
  err body ... hands max ... uu | ... frame cost avg ... ms`;
- every 5 s: `pose peer N: pose_quality arm_p95=... tip_p95=... latency~... ms`;
- the native stages log `native servo ON` / `refused (<reason>): Lua path`.

Must not appear: `had NO simulated bodies`, `weapon=nograb`.
