# Interaction channel (grabs and shoves between players)

Capability `INTERACT` (`hsmp_net::net::caps`).

**Problem:** each player simulates only their own Willie. The opponent on my screen is a stand-in, servoed hard to the owner's replicated pose ([replication.md](replication.md)). A grab or shove acts on the stand-in, and without this channel nothing feeds it back to the owner's real body, so the opponent behaves like an immovable, infinite mass.

**Model:** the initiator detects the interaction, the server validates it, and the owner of the affected body applies it.

```
 grabber / shover (A)                     server                         owner of the body (B)
 HSMPInteract detects on B's stand-in     interact/ validates            HSMPInteract applies on B's REAL pawn
   G2S interact ──► sidecar ──► C2S interact ──► S2C interact ──► sidecar ──► S2G interact
   bus pose_yield ──► HSMPAvatars: B's stand-in yields while A holds it
```

## Wire (protocol v6 record `interact`, `crates/hsmp-ipc/src/schema/interact.rs`)

- Capability bit 2 `INTERACT` (`hsmp_net::net::caps`). The server and the sidecar both offer it on top of the base set. Nothing is sent unless the connection negotiated it: the server ignores interaction records from a peer without it and never sends one to such a peer, and the sidecar drops the game's records.
- ONE record, `Interact` (48 B, `#[repr(C)]`), in every direction: `{ kind u8, hand u8, bone u8, _r, target_peer u32, id u32, ts u32, point [f32;3], vector [f32;3], gid u32, _r2 }`. A message is the 8-byte `WireHdr` + 48 B = 56 B. The record's check refuses an unknown kind, `hand` > 1, `bone` ≥ 16, `target_peer` ≥ 2^24, `|point|` > 10^4, `|vector|` > 10^8 and non-finite floats before anything reads it.
- Kinds: `interact` (0x0610, reliable) and two wire aliases of the same layout for `GRAB_UPDATE`: `interact_grab_r` (0x0611) / `interact_grab_l` (0x0612), `RelLatest` on streams 0x86 / 0x88, so an unacked update is superseded per hand (C2S: peer 0 = per hand; S2C: per (initiator, hand)).
- S2C: `WireHdr::peer` = the initiator; 0 = the server (a denial or a server-side end of OUR grab).

| kind | from → to | meaning |
|---|---|---|
| 1 `GRAB_START` | grabber → owner | grab of `bone` (HERO_BONES index) by `hand` (0 R, 1 L) at `point` (bone frame); `vector` is the grabber's hand world position at `ts` |
| 2 `GRAB_UPDATE` | grabber → owner | lease refresh (~10 Hz) with the current hand position (wire alias per hand) |
| 3 `GRAB_END` | grabber → owner, or server → both | the grab is over |
| 4 `IMPULSE` | shover → owner | world impulse `vector` (kg·cm/s) at `point` (bone frame) |
| 5 `GRAB_DENIED` | server → grabber (peer 0) | the start was refused (or a refresh failed the context) |

- `id` is the grab id. The grabber's sidecar assigns it (patched into the record in place), monotonically for its whole lifetime, so a Lua reload can never reuse an id the server already closed. For an impulse, `id` is a sequence number.
- `gid` is HSMPInteract's own grab id: set by the game on G2S, patched back by the sidecar into server answers (peer 0), zeroed on records from other peers.
- `ts` is the initiator's sender clock (`os.clock()` ms, the clock the pose stream uses).
## Server validation (`server/src/interact/`, glue `server/src/server/interact_glue.rs`)

The cheap checks run first.

1. **Shape:** client kinds only, `bone` < 16, `hand` ≤ 1, finite numbers, `|point|` ≤ `MAX_POINT` (60 uu), grab id ≠ 0, not self.
2. **Context:**
   - both sides negotiated INTERACT;
   - the initiator is alive;
   - the target is connected;
   - the match is not frozen (`countdown`/`paused`).

   Lobby, live, roundover and match_over are allowed: no damage flows, but you can still push people. `GRAB_END` is always honoured for a known grab, even when the sender is dead or the match is frozen.
3. **Rate limits per initiator** (token buckets, burst/per second):
   - impulses: 10/20;
   - grab starts: 4/4;
   - grab updates: 20/40. A flood of updates is thinned but still keeps the lease.
4. **Reach against the lag-comp history** (`lagcomp::interact_distance`, file `lagcomp/interact_query.rs`):
   - The initiator is rewound to its `ts` and the target to the server-predicted instant the initiator was displaying (`predict_view`), searched over ± the prediction tolerance.
   - Grab: the grabbing hand must be within `GRAB_REACH` (90 uu) of the grabbed bone.
   - Impulse: the contacted bone must be within `IMPULSE_REACH` (90 uu) of the initiator's body-capsule surface.
   - Without pose history, the last validated roots must be within 400 uu.
5. **Impulse magnitude:**
   - Each event is clamped to `MAX_IMPULSE` (15 000).
   - A per-target budget applies: `RECV_BUDGET` 40 000/s, burst 20 000, summed over every initiator. No combination of claims can launch a body.

**Grabs are leases:**
- A grab with no update for `GRAB_LEASE_MS` (1 s) ends: GRAB_END goes to the target, and GRAB_END (from 0) goes to the grabber.
- When either peer leaves, times out or is replaced, the server tick ends the grab at once.
- A new grab with the same hand ends the old one.

**Channel 1 is unordered.** Per hand, grab ids only grow:
- a START or UPDATE whose id is at or below the last ended id is stale;
- an UPDATE that overtakes its START opens the grab, and is forwarded as a START;
- an END that overtakes its START makes the late START stale.

A refused START, or a refresh that fails the context, is answered with GRAB_DENIED, so the grabber's stand-in stops yielding. If the grab was active on the target, a GRAB_END also goes to the target.

**Tests:**
- `interact::tests`: 17 tests covering shape, context, both orderings, reach and no-history fallback, impulse clamp, budget, rate limits, leases and leave, and hostile records through `record::view` and the judge.
- `schema::interact::tests` (hsmp-ipc): layout, round trip, every range check, the per-hand wire kinds, the `hero_bone` table, `pose_yield` rows, decoder fuzz; `server/tests/decode_fuzz.rs` `fuzz_interact_records`: whole wire messages.
- `lagcomp::interact_query::tests`: 3 tests.
- `server/tests/interact_e2e.rs`: a real server and two real sidecars. It covers caps negotiation, impulse relay plus clamp, grab start/update/end, lease expiry on both sides, denial for a disconnected target, and that self-targeted or off-bone events go nowhere. It drives the two games' segments with typed records (G2S push, S2G pop).

## Sidecar (`server/src/interact_client.rs`)

- **G2S** (`records_in.rs` -> `interact_client::on_g2s`, on the `hsmp-ipc` thread): the game's `interact` record, validated by the ring reader. The sidecar mints / maps the wire id (one live grab per hand; an end takes its target from the mapping; a late update of a closed grab is dropped), patches `id` in place and frames the same 48 bytes (`wire::message`, the per-hand alias for a grab update).
- **S2C** (`handlers::handle_server_record` -> `interact_client::on_s2c`): validated, `gid` patched (server answers: our Lua gid, and the grab is closed; peer records: 0), pushed into the S2G ring as `interact` with the header's peer.
- No JSON, no bone names, no polling task.
## HSMPInteract (`mods/HSMPInteract/`)

**Files.** `Scripts/main.lua` is the UE glue. `Scripts/interact_core.lua` is pure logic: bone map (name <-> wire index, `C.HERO` = the schema's `hero_bone` table), bone-frame math, the `interact` record tables both ways (`C.grab_rec`, `C.grab_end_rec`, `C.impulse_rec`, `C.from_rec`), accumulator, clamps and the `pose_yield` record.

**Runtime.**
- The mod runs only while an MP session is live (the shared predicate in `shared/hsmp_session.lua`: the `link` record says connected and the sidecar's heartbeat in the shared segment is fresh) and in a gameplay world.
- It uses the game-thread shim and the shared world guard (`hsmp_wg`): every cache is dropped untouched on a world change, events queued meanwhile are skipped, and a mod reload starts at the end of the event cursor. Records go out with `IPC.send("interact", t)` and come in with `IPC.events("interact")` (`r.peer` = the initiator).
- Mod order: `mods/mods.release.txt` lists `HSMPInteract : 1` after HSMPHud.

### Detection: the grab (per frame)

1. The mod reads my pawn's grab state (Willie_BP, UE4SS_ObjectDump.txt): `Grabbed R`, `Grab Component R` (the grabbed PrimitiveComponent) and `Grab Bone R` (NameProperty). The left hand mirrors these properties.
2. A grabbed component whose owner is a stand-in (bus key `puppets`) starts a grab.
3. The skeleton bone is mapped to a hero bone (`C.map_bone`: twist, clavicle, finger and neck bones), or to the hero bone nearest my hand.
4. The grip point is my hand (`Hand_R`/`Hand_L` socket) expressed in that bone's frame, clamped to 55 uu.
5. While I hold the stand-in, the mod sends `grab_update` every 100 ms and asks HSMPAvatars to yield.
6. When `Grabbed` goes false, or the component changes, it sends `grab_end`.
7. A denial stops the updates and the yield.

### Detection: impulses (body-hit hook)

- **Hook:** `Willie_BP_C:BndEvt__BP_ThirdPersonCharacter_Mesh_K2Node_ComponentBoundEvent_0_ComponentHitSignature__DelegateSignature` (HitComponent, OtherActor, OtherComp, NormalImpulse, Hit).
- **Counts only when every filter passes.** Without them, stand-in feet touching the floor were sent as shoves while the pawns were apart, about 200 per minute.
  1. **The stand-in side:** self is a stand-in, and HitComponent's owner is that stand-in.
  2. **The other side, by identity (addresses):** OtherComp must be one of my pawn's skeletal meshes (`SK_Skeleton`, `BoneCore`, `DriverSkeleton`, `Mesh`) owned by my pawn, or a component of a weapon my pawn holds (`Weapon R`/`Weapon L`). The OtherActor parameter alone is never trusted. Floor, world and prop components never qualify.
  3. **Spawn protection:** my pawn must not be spawn-protected. The source is HSMPSync's `spawn_status` bus record, `protect_until` in os.clock seconds; without `has_protect_until` the pawn is protected until Live. Inside protection, nothing is sent and nothing is applied.
  4. **Distance:** the two pelvises must be within `contact_pelvis_max` (200 uu).
  5. **Closing velocity:** the bodies must close on each other along the contact normal at more than `closing_min` (150 uu/s). The test is `GetPhysicsLinearVelocity` of my contacting body (or weapon) minus the stand-in bone's velocity. A resting or sliding touch is not a shove.
  6. **Per-pair rate cap:** at most one shove per peer every `shove_gap_ms` (150 ms).
- Weapon hits that deal damage are dropped by the damage dedupe below, because HSMPCombat replicates them. Contacts on my own pawn are ignored, because the owner sends those.
- The `stats:` line (every 10 s) counts contacts from my body per stage: apart, slow, protected and foreign self. The first 5 accepted contacts are logged with the bones and the closing speed.
- **Bone and point:** taken from `Hit.MyBoneName` and `Hit.ImpactPoint`, falling back to the BP's cached `Body Hit Bone Name Self` and `Body Hit Impact Point`.
- **Direction:** the impulse is oriented away from my pelvis.
- **Accumulation** (`C.acc_flush`):
  - contacts are summed per peer;
  - the first contact at or over `impulse_min` goes at the next frame;
  - further contacts are summed over 50 ms windows;
  - weak touches add up for one window, then expire.
- **Damage dedupe:** a contact in the same ±1 frame as a `Get Damage` on that stand-in from my body with Raw Damage ≥ 5 is dropped while a round is live, because HSMPCombat replicates that hit. In the lobby the shove still goes.
- **Yield:** a sent shove asks for a brief yield (150 ms).

### Application on my body (per frame)

**Impulse:**
- applied with `AddImpulseAtLocation` on my simulated mesh at the bone's point;
- `impulse_gain` (1.0) applies, then a cap of 15 000, then a cap on the bone's velocity change: `GetBoneMass × apply_max_dv` (800 cm/s). This rules out launches.

**Grab:**
- A pooled `PhysicsHandleComponent` on my pawn grabs my bone with `GrabComponentAtLocation`, position only.
- The handle is soft: `grab_k` 1500 and `grab_d` 150.
- The handle target is the grabber's **stand-in hand on my screen** (`Hand_R`/`Hand_L` of its pose mesh), which keeps the loop physically consistent. If that hand is not available, the streamed hand position is used.
- The target leads the grip point by at most `grab_lead` (40 uu), so the force is at most stiffness × lead. I can struggle.
- Release happens on any of:
  - `grab_end`;
  - lease expiry (no update for 1.5 s);
  - breaking free (more than 130 uu for more than 400 ms);
  - a pawn change;
  - a world change.
- A grab I broke free from stays broken: its later updates are ignored.

**`.settings.json` overrides:**

| Key | Effect |
|---|---|
| `"interact": false` | turns the mod off |
| `interact_impulse_min` | the detection threshold |
| `interact_impulse_gain` | the gain on applied impulses |
| `interact_grab_stiffness` | the grab handle stiffness |

**Tests:** `tests/hsmp-interact-test` (mlua, Lua 5.4, mocked UE4SS, the typed HSMPNative mock), 17 tests:
- core: bone map, bone-frame round trip, the record tables both ways and the schema's code tables, accumulator, clamps, the yield record;
- detection to message: grab start/update/end with a rotated bone frame and yield; props; unknown bone; denial; impulses (orientation, point, window); weapon, self and prop contacts are ignored; damage dedupe while live but not in the lobby;
- message to application: impulse placement and caps; grab handle soft, position-only, anchored on the stand-in hand, lead-clamped; lease; pooling; grab_end; breaking free; pawn change;
- world guard: no freed object touched, stale events skipped;
- no MP session: nothing happens.

## Stand-in compliance (HSMPAvatars)

**Bus key:** `pose_yield`, a typed record (`PoseYield` head + up to 32 `YieldRow { until_ms u64, peer u32, gain f32, cap_lin f32, cap_ang f32 }`; Lua `{ rows = { ... } }`). `until_ms` is on the process clock `os.clock()*1000`, which every UE4SS mod shares.

**Effect:** until `until_ms`, that peer's stand-in servo uses the given gain and caps instead of its defaults (0.3 / 900 uu/s / 900 deg/s). HSMPAvatars reads the key at 30 Hz. To end a yield, write no row for the peer or let `until_ms` pass.

**What HSMPInteract writes:**
- while I hold a stand-in: `{peer, now+300, 0.15, 150, 150}`, refreshed every 100 ms;
- after a shove: `{peer, now+150, 0.35, 300, 300}`.

Requests are merged per peer (latest deadline, softest gain and caps). The record is rewritten when a row changes (at most every 100 ms otherwise) and written with zero rows when nothing is pending.
## Tunables

| Where | Name | Default | Meaning |
|---|---|---|---|
| interact/ | `MAX_IMPULSE` | 15 000 | per-event clamp (kg·cm/s) |
| interact/ | `RECV_BUDGET` / `RECV_BURST` | 40 000/s / 20 000 | impulse one body may receive |
| interact/ | `GRAB_REACH` / `IMPULSE_REACH` / `ROOT_REACH` | 90 / 90 / 400 uu | reach checks |
| interact/ | `GRAB_LEASE_MS` | 1000 | server lease |
| interact/ | `IMPULSE_RATE` / `GRAB_START_RATE` / `GRAB_UPDATE_RATE` | 10,20 / 4,4 / 20,40 | burst, per second |
| HSMPInteract | `impulse_min` | 1500 | detection threshold. Size it above `x_pose_contact` `far_p95` (HSMPAvatars logs it every 5 s in contact) |
| HSMPInteract | `impulse_window` | 50 ms | contact summing |
| HSMPInteract | `apply_max_dv` | 800 cm/s | per-event bone velocity-change cap |
| HSMPInteract | `grab_k` / `grab_d` / `grab_lead` | 1500 / 150 / 40 uu | owner-side handle |
| HSMPInteract | `grab_break` / `grab_break_ms` | 130 uu / 400 ms | break free |
| HSMPInteract | `grab_lease_ms` | 1500 | owner-side lease |

## In-game checks

1. **Grab drags the real arm.** Grab the opponent's arm. On both screens, the opponent's real arm is dragged toward the grabber's hand. Grabber log: `grab N (hand R) on peer P: bone Lowerarm_L`. Owner log: `grabbed by peer P (hand R) on my Lowerarm_L`. HSMPAvatars logs `pose peer P: yielding to an interaction`.
2. **A shove moves the owner's pawn on both screens.** Logs: `shove on peer P` on the shover, `shoved by peer P` on the owner. The latency should be at most RTT + 1 frame: detection is per frame, the sidecar's ipc thread forwards the record at once, and the owner applies on the first frame after it lands.
3. **No launches.** Check the `capped` counts in `stats:`. The server logs clamps at debug level (`interaction altered`).

## Open questions

- **Does the victim's native hit replay push the body?** HSMPCombat replays an accepted hit on the victim's own pawn through its native damage path (see [combat.md](combat.md)). If that replay already pushes the body, a punch is not double-counted: the shove detector drops contacts that coincide with a damage hit in a live round. If it does not push, set `damage_dedupe = false` in HSMPInteract so punches also push.
- **The sign and units of `NormalImpulse` on a servoed stand-in.** The mod orients the impulse away from the shover. Tune `impulse_min` and `impulse_gain` from the `x_pose_contact` numbers.
- **The grabber's own constraint** stays the game's (`R Grab Force Limit` 250 000). If the yielding stand-in still feels too stiff, lower that force limit on my pawn while the grab target is a stand-in.
- **Clashes** (blade on blade) are not in this channel. They travel as the `clash` combat record and are validated by lag compensation.
- A collision matrix and contact-aware servo stiffness are not implemented; the stand-in yields only through `pose_yield`.
