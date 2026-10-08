# Vitals replication (health, limbs, stamina, bleeding, posture flags, dismemberment)

Each player's game is the authority for its own pawn's vitals. HSMPCombat samples them, the sidecar
sends them, the server checks and relays them, and every other screen mirrors them onto its
stand-in of that player and shows them in the HUD.

Code:

- `mods/HSMPCombat/Scripts/main.lua`: sampling (`publish_own_vitals`), the stand-in mirror
  (`mirror_vitals`) and the damage paths ([combat.md](combat.md)).
- `crates/hsmp-ipc/src/schema/combat.rs`: the `Vitals` record, its names, caps and flags.
- `server/src/vitals.rs` (included as `proto::vitals`): the send policy (deadbands, urgency) and
  helpers.
- `server/src/combat_client.rs`: the sidecar's `VitalsGate` and the receive side.
- `server/src/combat.rs` + `server/src/server/combat_glue.rs`: the ledger and the relay.
- `mods/HSMPHud/Scripts/hud_state.lua`: the HUD reader ([hud.md](hud.md)).

## Pipeline

```
owner game (HSMPCombat, 33 ms tick)
  every 2nd tick (~15 Hz) + in the same tick as any replayed hit:
  sample own pawn -> per-field deadband or 1 s heartbeat -> `vitals` slot (quantised once)
        |  sidecar polls every 5 ms (combat_client.rs)
        v  VitalsGate: deadband + 50 ms rate cap (20 Hz), flags / parts at once, 1 s keyframe,
           every significant frame repeated at +120 and +360 ms; seq patched with the sidecar's
 `vitals` record, C2S                               (56 B with the wire header; channel 0, latest-wins)
        |  server (combat_glue.rs)
        |    record validated in place -> per-sender budget -> ledger (Health ceiling, deaths)
        |    Health clamped in place to the ceiling
        v
 `vitals` record, S2C to every other peer (WireHdr.peer = owner), through the relay's bandwidth budget
        |  receiving sidecar: highest seq wins (wrap-safe; a large jump back = the peer's sidecar restarted)
        v
 `peer_vitals` slot of that peer
        |  HSMPCombat update_standins (every tick)        HSMPHud (about 5 Hz)
        v                                                  v
 stand-in mirror (never lethal)                    CON / BODY % / BLEED for that player
```

The owner's HUD reads its own `vitals` slot through the same reader and quantisation as an
opponent's `peer_vitals`, so both screens compute the same numbers.

## The record (`schema/combat.rs` `Vitals`)

```
seq: u32 | dism: u32 (bitmask over DISM_PARTS) | flags: u16 (VF_*) | v: [u16; 19]  (round(x · 64), 0xFFFF = unknown)
```

- **Size:** 48 B payload, 56 B with the 8-byte wire header.
- **Rate:** 20 Hz at most and about 1 Hz idle.
- **Resolution:** 1/64. Health-type values (index 0..12) are at most 150, stamina and exhaustion at
  most 200; the game clamps before quantising, and a record outside those ranges, with unknown flag
  bits or unknown part bits, is invalid and dropped.
- **Order:** `VITALS_NAMES`, append only. HSMPCombat's `VITALS` table must match it.

| # | Property (exact name on `Willie_BP_C`) | Mirror on stand-in |
|---|---|---|
| 0 | `Health` | no: pinned ≥ `PUPPET_HP_PIN` (100) |
| 1 | `Head Health` | floored at 1 |
| 2 | `Neck Health` | floored (also feeds the server's Snap Neck rule) |
| 3 | `Body Upper Health` | floored |
| 4 | `Body Lower Health` | floored |
| 5 | `Back Health` | floored |
| 6 | `Arm_R Health` | floored |
| 7 | `Arm_L Health` | floored |
| 8 | `Leg_R Health` | floored |
| 9 | `Leg_L Health` | floored |
| 10 | `Head Health (Crush)` (the only "Crush" variant on the class) | floored |
| 11 | `Consciousness` | floored |
| 12 | `Consciousness 2 (Legs)` | floored |
| 13 | `Stamina` | written |
| 14 | `Exhaustion` | written |
| 15 | `Bleeding` | written |
| 16 | `Blood Rate` | written |
| 17 | `Pain` | written |
| 18 | `Fallen Rate` | no (posture comes from the pose stream) |

All are double properties. The names are checked by `hsmp-tools lua-test vitals` against
[../halfsword/willie_props.txt](../halfsword/willie_props.txt), a property dump of a live Willie.

**Flags** (`VF_*`, all `BoolProperty` on the class):

| Bit | Property |
|---|---|
| 1 | `DED` (dead) |
| 2 | `Fallen` |
| 4 | `Downed` |
| 8 | `Headless` |
| 16 | `Pain Shock` |

The owner also sets bit 1 when Health ≤ 0. None of the flags are written on a stand-in.

**Dismemberment:** the owner reads the native typed `Dismembered Parts Map` at about 5 Hz.
Only completed true entries with an actually hidden distal root on the same fresh owner Mesh
can add a whole-region bit. `Dismemberment In Process` must be false around the read; world,
actor, Mesh, peer, match, round and full life must match. The conservative native mappings are
3→lowerarm_r, 4→hand_r, 6→lowerarm_l, 7→hand_l, 9→calf_r, 10→foot_r,
12→calf_l and 13→foot_l. Native partial cuts use geometry and have no whole-root projection.
Confirmed positives survive unavailable reads within that body and cannot cross into a new
publication life. Protocol 12 has no independent topology-unknown field: initial zero is not
proof of an intact body. Its 23 representable parts are the Willie bone names in `DISM_PARTS`
(`pelvis`, `spine_01` .. `spine_05`, `neck_01`, `neck_02`, `head`, the clavicles, arms, hands,
thighs, calves and feet).

**Not replicated:** the per-action stamina drain rates (`Stamina Burn`, `Stamina Burn Swing R/L`,
`Stamina Burn Thrust R/L`, `Stamina Burn Dodge`, `R/L Hand Stamina Burn Rate`, `2H Default/Alt
Stamina Burn Rate`). They are computed from the owner's own input; the resulting `Stamina` is what
is sent. There is no "Fatigue", "Breath" or "Tired" property on the class.

Observed in live dumps: Consciousness drops to about 53 while Health stays 100, so Consciousness
must travel separately; deaths read `Health 0, Head Health 0, Consciousness 0, Downed, DED`.

## Server: ledger and relay

The server reads Health and the dead flag from each record into its ledger (`combat::vitals_in`,
`Ledger::on_vitals`):

- A record with `DED` kills even when Health is still above 0 (head trauma): the server declares
  the death (`death (owner vitals)`, cause 2).
- The relayed Health is the owner's report. Only healing is bounded: after the life's first hit,
  Health may rise at most 1 hp/s above the last accepted value. A faster rise is clamped in place in
  the relayed record and logged (`vitals: Health rose faster than any heal; clamped`).
- The server never lowers the relayed Health by its own damage estimate. Half Sword floors Health
  per body part and gates repeated contacts, so an honest owner often stays at 100 after hits the
  estimate books at 5–10 HP each.
- Neck Health feeds `damage::note_neck_health` (Snap Neck plausibility).
- Per-limb values are only range-checked, not ledgered. A limb-driven death reaches the server
  through `DED`, Health 0 or the owner's `death_report`.

The god-mode and stall rules are in [combat.md](combat.md) §8.

## Stand-in mirror (HSMPCombat `mirror_vitals`)

**When it runs:** on every new `seq`, plus a re-assert every 10 ticks (`MIRROR_REASSERT`; native
regeneration drifts the values), and only while the owner is alive. Once the owner is dead (server
declared, or a trusted dead flag), the stand-in plays its death once instead.

**Lethality guards:**

- Health is not written; it stays pinned at ≥ 100. The game's own regeneration clamps Health to 100
  every tick, so a higher pin would be rewritten each frame.
- Limbs and consciousness are floored at 1 (`STANDIN_FLOOR`).
- Unknown fields are not written.
- A dead flag counts only once the owner's seq has advanced in this world, or when the server
  declared the death for this round; a flag left over from the last round never kills the new
  round's stand-in.

**Posture is not written.** `Fallen`, `Downed` and `Fallen Rate` are left alone: the pose stream
already shows the owner's real posture, and the native get-up logic would fight the stand-in drive.
Limb values still feed the Blueprint's tonus and blood logic.

**Severed parts:** Combat hides the confirmed root on the stand-in's `Mesh` / `SK_Skeleton`, only
if `GetBoneIndex` finds the bone. Avatar playback excludes confirmed missing descendants from
servo targets and attempts native physical-body exclusion, including temporary release and
same-actor reuse. Actual contact exclusion/restoration requires native acceptance. The bone is
un-hidden when a verified new body's owner mask clears it. A real native dismember of the stand-in is not triggered: HSMPAvatars sets
`Force Disable Dismemberment`, and `Dismember Function Initiate` needs markers and collision boxes.

## Stamina

The owner's stamina is native and local:

- **Swinging and thrusting:** drained by the owner's own input.
- **Blocking:** weapon-on-weapon contact with a stand-in's blade is local physics on the owner's
  screen; the native block cost stays.
- **Being hit:** whatever the native replay of an accepted hit does to stamina, it does on the
  victim's own pawn. Echo contacts are undone.
- **Round start:** `Stamina` and `Exhaustion` are restored from the Willie_BP CDO with the wounds.

Remote stamina is written on the stand-in, which drives the native exhaustion behaviour, if any.

## Tunables

| Where | Name | Default |
|---|---|---|
| HSMPCombat | `VITALS_TICKS` / `VITALS_BEAT_S` | 2 ticks (~15 Hz) / 1 s |
| HSMPCombat | `VITALS[i][2]` deadbands | health-type 0.25, consciousness 0.5, stamina / exhaustion 0.5, bleeding / blood rate 0.05, pain 0.25, fallen rate 0.05 |
| HSMPCombat | `DISM_TICKS`, `STANDIN_FLOOR`, `MIRROR_REASSERT`, `MIRROR_DISMEMBER`, `PUPPET_HP_PIN` | 6 ticks, 1.0, 10 ticks, true, 100 |
| combat_client.rs | `VITALS_MIN_INTERVAL` / `VITALS_KEYFRAME` / `VITALS_REPEAT` | 50 ms / 1000 ms / +120, +360 ms |
| vitals.rs | `DEADBAND` | the same bands as HSMPCombat |
| schema/combat.rs | `VITALS_HP_CAP`, `VITALS_STAMINA_CAP` | 150, 200 |
| validate/rate.rs | `Kind::Vitals` budget | 40 burst, 25/s |

## Tests

- `cargo test -p hsmp-server`:
  - ledger (`combat::tests`): reflection, the healing ceiling, reorder, `DED` kills, Health-less
    records;
  - sidecar (`combat_client` tests): the gate's deadband, rate cap, keyframe, urgency and
    latest-wins, peer records landing in the `peer_vitals` slot, the 56-byte message size.
- `hsmp-tools lua-test vitals`: HSMPCombat under mlua (Lua 5.4) with mocked pawns:
  - names are real properties and the Lua / Rust order matches;
  - sampling into the quantised record (deadband, heartbeat, dead flag, part bitmask, round trip);
  - the mirror (pin, floors, written fields, untouched flags and unknowns, hide / unhide);
  - owner death leads to the stand-in's `Death()` once;
  - the attacker claim and the owner's native replay (no top-up or trim);
  - a stand-in's weapon echo on my pawn undone, plus the touch report.
- `bash scripts/e2e-test.sh` (with `HSMP_BINS=<target>/release`): **T9c** checks that A's `vitals`
  record reaches B's `peer_vitals` slot with every field.

## In-game checks

**Owner** (`[HSMPCombat]`, every 5 s):
`vitals: out seq=N writes=W/S samples (5 s) hp=... st=... flags=F | mirror peer P seq=... hp=... st=... fields=17 hidden=0 | ...`

- W is about 75 or fewer while stamina moves, and about 5 when idle.
- `fields=17` means every mirrored property resolved.

**Receiver** (once per stand-in):
`vitals mirror: peer P stand-in Willie_BP_C_... <- seq ...: hp=... head=... torso=.../... ... (17 fields written)`

**Severed limb:** `peer P severed '<bone>' -> stand-in bone hidden`. `NOT found on stand-in mesh`
means the mask holds a name that is not a bone on that mesh; report it.

**Sidecar** (every 10 s): `vitals: sent frames in the last 10 s frames=... per_s=...`, at most 20.

**Server:** `death (owner vitals)` on a `DED` record. `vitals: Health rose faster than any heal;
clamped` is a cheat or desync signal.

**Must not appear:** `WARNING: stand-in of peer N ran its native death locally`.

## Known limits

- Whether writing `Stamina`, `Exhaustion` or `Bleeding` on an Invulnerable stand-in produces
  visible native behaviour has not been confirmed in game.
- Per-limb values are only range-checked, not ledgered (see above).
