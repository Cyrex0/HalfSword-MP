# World physics and interactable replication (HSMPWorld)

Loose weapons, physics props, destructibles, levers and traps must sit in the same place, in the
same state, on every screen. HSMPWorld (`mods/HSMPWorld/Scripts/main.lua`) discovers these
bodies, gives them network ids that match across clients, and replicates them with a lease model:
whoever touches or holds a body simulates it, everyone else follows.

| Part | Where |
|---|---|
| Discovery, ids, binding, leases, follow, dynamic items, actor states, consistency report | `mods/HSMPWorld/Scripts/main.lua` |
| Records (one binary form game → server → game) | `crates/hsmp-ipc/src/schema/world.rs` |
| Sidecar (slots ↔ wire, scope and epoch gating, resends) | `server/src/world_client.rs` |
| Server rules (pure logic) | `server/src/world.rs`, static arena table `server/src/world_static_table.rs` |
| Server glue and the 30 Hz world task | `server/src/server/world_glue.rs` |

## What is replicated

The classes come from the arena `.umap` imports (Alley, Pit, Yard, Slums, Cellar, LordsHall,
EastTower) and their sublevels:

| Kind | Classes | Bodies |
|---|---|---|
| weapon | `ModularWeaponBP_C` and every subclass: rack and floor weapons, tools (pitchfork, shovel, rake, scythe, flail), Reforged polearms, `Shield_Pavise_Tower`, treasure (chalice, goblet, paten), `BP_Weapon_Trap*` | 1: the simulating one of `BaseMesh` / `Grip` / `Head`, otherwise the root |
| prop | `StaticMeshActor` whose `StaticMeshComponent` simulates: the `*_PhysicsProps` sublevels (buckets, cups, ladder, firewood bundles, benches) | 1 |
| multi-body | `BP_Barrel_Destructable_Constraits`, `BP_Structure_Plank/Board_Destructible_*`, `BP_Fence_Flimsy_*`, `BP_Fence_Bags`, `Chain_BP`, `Trap_BP`, `Trap_Kettle_BP`, `BP_Structure_Trap`, chandeliers, candle stands, `BP_Candle`, `BP_Container_*` chests | each simulating `StaticMeshComponent`, plus each Movable one on destructibles; at most 12 per actor |
| dynamic item | a weapon that leaves **my** hand: a drop or a disarm of my loadout weapon | 1 |

**Excluded:**

- Willies and anything attached to an actor, except pieces of compound structures: actors
  attached to `BP_Structure_*` (Alley's barricades), `Trap_BP` or `Trap_Kettle_BP` are bodies like
  any other.
- Weapons that are held, have a `Parent Actor` or a `Last Parent`. These are loadout or NPC
  weapons; the loadout and pose code owns them.
- CDO and archetype templates (`RF_ClassDefaultObject | RF_ArchetypeObject`, or an outer that is
  not a `Level`), and `BP_GameManager`'s temporary pool weapons at the origin (`pool-temp`).
- Skeletal ragdolls such as the training dummy and banners.

The arena "doors" are Alley's `BP_Structure_Trap_1..5` barricades (planks, boards and barrels as
attached child actors) and EastTower's lever-dropped blocks. The real door Blueprints
(`BP_Env_Arena_Church_Door_A_002`, `BP_Env_Arena_LordHall_Door_A_001/002`) are static meshes with
no physics.

Discovery runs four passes, at 0.1, 1, 3 and 8 s after load, sliced at 300 actors per tick. It
writes a dev report, `<state>/.world_scan.txt`, with every body, its class, its id, whether it is
placed or runtime-spawned, and why each skipped actor was skipped.

Property names matter here: the Blueprint properties have spaces (`"Is Held"`, `"Parent Actor"`,
`"Weapon R"`, `"Weapon L"`). CamelCase reads silently return nil. The SDK `.hpp` headers drop the
spaces, so `UE4SS_ObjectDump.txt` is the reference (see [../halfsword/](../halfsword/README.md)).

**What the static map data (`docs/arena_static/`) means here:**

- Placed weapons and chests are destroyed at round start by the game's `Clean Up Map` in the
  modes the MP game-instance profile uses. World weapons then come from drops and disarms (the
  dynamic-item path).
- Arena progression mode culls placed weapons at random per client and re-spawns survivors with
  the same class at the same spot. Bodies culled on only one client log as "unmatched".
- Chests roll random contents per client. Expect a class-mismatch log for chest loot.

## Identity: deterministic ids and a canonical manifest

- **Id formula:** `FNV1a(class | component | where) & 0x7FFFFFFF`.
  - `where` is `levelpath:FName` for level-placed actors, which have the same name on every
    client.
  - Otherwise `where` is the spawn cell `@x,y,z` at 50 cm resolution, from the position at
    discovery.
  - Candidates are sorted by key before ordinals (`#2`, `#3`, …) are given to collisions, so every
    client assigns the same ids whatever order `FindAllOf` returns.
- **Manifest (known arenas).** The server builds the manifest itself from the statically extracted
  level data (`docs/arena_static` → `world_static_table.rs`), with the same id formula, so a level
  bucket starts out seeded. Client proposals (`world_manifest_out` → `world_manifest` record) are
  then accepted only as aliases: folded into the nearest server entry with the same class and
  component hash within 120 cm (`ALIAS_R`), or, for bodies whose position is not known statically
  (multi-body props, chest loot), accepted only within 500 cm of a static object of the arena
  (`VOUCH_R`). Everything else is rejected (and still acked).
- **Manifest (other levels).** Clients submit their ids; the first submission wins, and a later
  entry with the same hash within 60 cm (`MATCH_R`) is folded into the existing id.
- **Binding.** Each client binds its bodies to the canonical list: the exact id; then the same
  class and component within 120 cm of the spawn point ("alias"); then, for weapons only, any
  weapon within 40 cm. That last case logs a class-mismatch warning.
- **Dynamic ids:** `0x80000000 | (peer & 0x7FFF) << 16 | counter`, proposed through
  `world_dyn_out` → `world_dyn`. The server rejects entries whose namespace, owner or class (it
  must start with `/Game/`) does not match the sender, at most 32 per peer per epoch. Receivers
  spawn a local copy of the class with `BeginDeferredActorSpawnFromClass` and copy the
  `Weapon Passport` from the dropping peer's stand-in weapon, so the copy looks like the real one.

## Authority: leases

```
FREE ──claim touch / first state──► TOUCH(peer) ──claim hold──► HOLD R/L(peer)
  ▲                                    │   ▲                         │
  └──── release(rest pose) ◄── at rest 1.5 s   └── let go (drop/disarm)┘
  └──── lease lapse: 3 s without a state, or owner disconnects
```

**Server rules** (`server/src/world.rs`, unit-tested):

- The first claim processed wins.
- A hold claim beats a touch lease, so a pickup steals a pushed object.
- Nobody can rob a holder.
- An owner re-claiming renews its lease without bumping the version.
- A non-owner's release does nothing.
- A state for a free body is an implicit touch claim, which saves a round trip when someone knocks
  something over. States are accepted only from the lease holder and are sanity-checked (speed,
  distance of a held body from its holder) before the implicit claim.
- Claims are limited to 20/s per peer, burst 40.

**Owner.** The owner's game simulates the body and streams it. The body counts as awake while
held, while moving faster than 8 cm/s, while it moved more than 1.5 cm or turned more than 1°
since the last read, or until it has been still for 400 ms. Then it sends the final "asleep"
frame 3 times, and after 1.5 s at rest it releases the body with the rest pose. The server keeps
that pose as the body's **anchor**.

**Everyone else:**

- **Owned by another peer:** physics is off and the body follows the owner's stream kinematically
  (teleport sweep off), so two simulations never fight.
- **Free:** physics runs locally and the body sits at its anchor.
  - If it moves and my pawn is the nearest Willie within 3 m, or one of my moving bodies is within
    1.5 m, I claim it.
  - If it moves and nobody claims it within 1.2 s (for example, a stand-in's body nudged it), it
    snaps back to the anchor.
- **Lost frames:** the server re-sends the anchors of free bodies in rotation every 250 ms and the
  owner table every 2 s, so lost final frames and lost owner updates heal.

## Pickups, drops and disarms

| Event | Owner's client | Server | Other clients |
|---|---|---|---|
| A picks up a scene weapon | its own hand weapon (`"Weapon R"` / `"Weapon L"`) is the body → claim HOLD R/L, stream with `held` (+ `left`) at 20 Hz | grants HOLD (steals a touch lease) | their copy turns kinematic and follows the stream into A's stand-in hand; the `world_held` bus key lists it |
| A drops it or is disarmed | hand empty → claim TOUCH, stream the fall → asleep → release | frees the body, stores the anchor | the copy follows the fall; physics comes back at the final rest frame |
| A drops a **loadout** weapon | it becomes a dynamic item (manifest entry + optimistic stream) | validates the namespace | spawns a local copy of the class, which follows the fall |
| B picks up A's dropped item | normal hold claim | grants | normal |
| A grabs a prop with a hand (`"Grab Component R/L"` + `"Grabbed R/L"`) | claims HOLD | grants | the prop follows |

**`world_held` (bus key, HSMPWorld → HSMPLoadout).** Rows `{peer, nid, hand (0 R, 1 L), actor}`
for every world item another peer holds, with `actor` the local actor's FName. If a row exists for
a stand-in's hand, HSMPLoadout must not give that hand a weapon, strips a loadout weapon already
there (the world item is shown instead), and never destroys an actor named in it. When a new
dynamic item from a peer appears, HSMPLoadout strips the stand-in's copy of that class at once
and keeps the hand empty until that peer's next loadout version.

## Rounds and late joiners

- **Epochs.** Every world record is scoped by `(level = FNV(UWorld path), epoch)`.
  - The server's world task (30 Hz) bumps the epoch when the match enters countdown (including
    replaying a round after a pause) and when it returns to the lobby. A bump drops every level
    bucket and tells each in-level peer the new epoch.
  - The sidecar clears its tables and publishes `world_owners` with `sync = false` and the new
    epoch. HSMPWorld stops replicating at once and, if the arena is not reloaded within 1.2 s,
    puts every body back at its spawn pose, retires the copies it spawned, and re-syncs.
  - Stale-epoch traffic gets a rate-limited hint and is otherwise dropped.
- **Sync.** The G2S `world_sync` record returns the owner table (`sync = true`,
  `manifest_len`), the full manifest and the cached transforms (owned bodies' latest states plus
  anchors). HSMPWorld retries every 1.5 s until answered. The sidecar re-syncs on its own if the
  server lists more manifest entries than it holds for more than 3 s.
- **Reloads** are detected by the world key, because the level name stays the same.

## Records and shared memory

Every message is a typed record (protocol v6, kinds `0x04xx`, capability `WORLD`). HSMPWorld
quantises once (smallest-three rotations, i16 velocities); the sidecar frames the bytes; the server
validates them in place and builds its fan-out from the same rows.

| Record | Flow | Channel | Game side |
|---|---|---|---|
| `world_state` (head + ≤ 32 `WorldObj`) | C2S, S2C | latest-wins (unreliable) | slot `world_out` (my owned bodies) |
| `world_claim` | C2S | reliable | G2S ring |
| `world_sync` | C2S | reliable | G2S ring |
| `world_owners` | S2C | reliable | slot `world_owners` |
| `world_snapshot` | S2C | reliable | merged into `world_remote` |
| `world_manifest` / `world_dyn` | C2S, S2C | ordered | slots `world_manifest_out` / `world_dyn_out` (proposals), `world_manifest` / `world_dyn` (canonical) |
| `world_hash` | C2S | ordered | slot `world_hash` |
| `world_verdict` | S2C | ordered | slot `world_consistency` |
| `world_remote` | sidecar → game | — | slot `world_remote`: newest sample per body plus anchors, 10 s TTL |
| `world_held` | game-local | — | bus key `world_held` |

`WorldObj` is 32 bytes: id u32, position 3×f32, smallest-three quaternion 3×i16 (< 0.01° error;
the dropped index sits in flag bits 6–7), velocity 3×i16 cm/s, flags (asleep, held, left, sim),
padding.

Claim modes: 0 free, 1 touch, 2 hold R, 3 hold L, 4 actor state (`CLAIM_STATE`, `rest.vel[0]` =
the bits), 5 initial rest pose (`CLAIM_INIT`). An owner record with `mode ≥ 0x80` is an actor-state
record, `mode & 0x7F` = the bits.

## Budget and rates

**Server relevance.** Asleep frames always go out. Held bodies are thinned to ¼ rate only beyond
60 m. Moving bodies go at ½ rate beyond 30 m and ¼ rate beyond 60 m. States are coalesced per
recipient every 33 ms (≤ 1000 B per message). Each recipient has its own 16 KB/s world budget
(8 KB burst) on top of the relay's accounting. Bodies are prioritised by distance, held bodies
weighted ×0.5; bodies that do not fit are dropped, which is safe because states are latest-wins
and anchors heal.

**Owner rate.** Sends go out every 3rd tick (~20 Hz), at most 24 bodies per send. Each body
accumulates priority each send (held 8, faster than 3 m/s 4, awake 2, asleep repeat 6, halved
beyond 10 m) and the top 24 go out, so busy scenes round-robin fairly.

**Rough cost.** A held world item costs its holder about 20 Hz × 32 B ≈ 0.7 KB/s up, plus headers.
A recipient that sees 15 players each holding one item gets about 10 KB/s. The worst case
(everyone holding two items plus a falling destructible) hits the 16 KB/s cap; far bodies degrade
first.

**Interpolation.** Each sender has its own render clock 110 ms behind its newest `ts`, with nlerp
on quaternions and at most 150 ms of extrapolation. Samples are ordered by `ts`, so reordered or
duplicate packets are harmless.

## Identical worlds

Both screens must show the same arena. Otherwise a hand that is free on one screen hits a door on
the other.

### State-bearing actors of the MP arenas

Sources: `docs/arena_static/*.json`, the arena `.umap` exports and the prop Blueprints decompiled
with `tools/mapdump --probe` and `hsmp-tools kismet-pp`.

| Class (count per arena) | What can differ | Randomised at start? | Changes in play | Handling |
|---|---|---|---|---|
| `BP_Structure_Trap_1..5_C` (Alley 5): the barricades. Their pieces are child actors: 12 `BP_Structure_Plank/Board_Destructible_*` and 9 `BP_Barrel_Destructable_Constraits` | piece poses, broken joints | no, but tagged `Respawn`: `BP_LevelManager` destroys and re-spawns them 2.5 s after load, then physics settles them per client | broken by hits | pieces are bodies, initial anchors after the 3 s scan, per-piece actor states |
| `BP_Structure_Plank_Destructible_2M/3M/4M`, `..._Board_Destructible_1X3M/1X4M/1X6M` (Alley 13, Pit 1, Slums 5) | the two `Plank Part`s and their breakable `PhysicsConstraint`; BeginPlay traces add wall joints; Tick breaks the joint when the parts dislocate > 10 cm | no | break | bodies + state bits |
| `BP_Barrel_Destructable_Constraits` (Alley 9) | staves, lids and rings on 12 constraints | no | hits break constraints | bodies + 2 state groups |
| `BP_Fence_Flimsy_Small/Curved/Big` (Slums 18) | `Constraint Health` drains with angular force, then breaks | no | break | bodies + break bit (the health is not replicated; the break is) |
| `ST_Lever_C` (EastTower 6) | handle on a driven hinge; Tick flips `Loaded` at ±30° roll and fires its gate and trap | no | pulled | handle = hinged body under the lease model; `Loaded` = state flag |
| `ST_LeverActivated_Child_C` (EastTower 6): lever-dropped block | `Activate Event` breaks and destroys it and drops the block | no | lever | state bit; remote apply calls the BP's own `Activate Event` |
| `Trap_BP_C` + `BP_Weapon_Trap_C` (EastTower 6) | BeginPlay spawns a `Chain_BP` and hangs the blade; `Event Activate Trap` turns blade physics on | no | lever | blade and chain links = bodies; armed = state flag, remote apply calls `Event Activate Trap` |
| `Chain_BP_C` (Cellar 4 placed, EastTower 6 runtime) | link bodies on joints | sound pitch only | pushed | bodies + state groups |
| `Trap_Kettle_BP_C` + `BP_Weapon_Trap_Kettle_C` (Cellar 1) | kettle on a joint, physics after 2 s | no | pushed | bodies + state |
| `BP_Container_Chest_002..005_C` (Alley 3, Cellar 2, LordsHall 2, Slums 3) | hinged `Lid`; tier and loot | **yes** | opened | destroyed on every client by `Clean Up Map` under the MP profile; if a mode keeps them: lid = hinged body, open = state flag |
| Placed weapons and quivers (Alley 4, Cellar 5, EastTower 5, LordsHall 9, Slums 6) | existence, passport | **yes** (`SpawnChance`, `BP_Generator_Weapons_Random`) | picked up | destroyed by `Clean Up Map` under the MP profile (except `Trap`-tagged ones); bodies + leases otherwise |
| Trap weapons `BP_Weapon_Trap_Spike*` (Pit 35, Slums 2) | spikes | no | — | bodies |
| `BP_Prop_Cooking_Utencil_Pot_001` (LordsHall 2) | pot + lid joint | no | pushed | bodies + state |
| Simulating / movable `StaticMeshActor`s (Alley 27, Cellar 108, EastTower 1, LordsHall 30, Pit 16, Slums 16, Yard 14) | pose after the load settle | no, but the settle is not deterministic | pushed | initial anchors + leases |
| Candles and light props (`BP_Candle_C`, `BP_CandleLight_C`, `BP_CandleMoveable_C`, stands, chandeliers) | **existence**: the Pit/Yard/Alley/Slums level BPs stream a lighting sublevel chosen from the game instance's `Day Time` | **yes, per client** (`Day Time` comes from each player's career save) | — | not pinned: presence mismatches show up in the consistency check |

### Initial state: one client's settled world, forced everywhere

1. After the 3 s discovery pass (after the 2.5 s `Respawn` re-spawn), each client proposes the rest
   pose of every unanchored free body once it has been still for 600 ms (at the latest at 5 s):
   `world_claim{mode = CLAIM_INIT, rest}`.
2. The server makes the **first proposer the init authority** of that (level, epoch). Only its
   poses become anchors, so the result is one client's world, never a mix. They go out as
   `world_snapshot` (sender 0) to everyone in the level, the proposer included, and are kept for
   late joiners and keyframes. The role passes on if that peer leaves.
3. Every client forces initial anchors exactly (> 0.5 cm or > 0.5°), asleep.
4. Actor states: the owner table at sync carries every recorded state; a client applies what its
   actors lack.

HSMPWorld logs `world ready: ... bound, ... anchored, ... forced, ... unanchored ...` (or
`(INCOMPLETE)` after a 25 s timeout). The Director does not wait on world readiness; it emits its
own `world_ready` event (see [director.md](director.md)).

### Runtime: hinges, breaks, levers

- **Hinged and constrained bodies** (lever handles, chest lids, gate blocks, plank parts, chain
  links) use the lease model unchanged: the toucher owns and streams, the others follow
  kinematically, so the hinge angle is the streamed orientation. When a lease ends mid-swing, the
  follower turns physics back on with the owner's angular velocity from the last two samples.
- **Actor states** (`CLAIM_STATE`): per state actor and group of 6 constraints (FName order,
  identical on every client), bit k = constraint k broken (sticky for the epoch), bit 6 = the
  kind's flag (lever `Loaded`, trap armed, lid open). A client reports new local breaks and flag
  edges; the server merges them (breaks never un-break, the flag is last-writer-wins), versions
  them and broadcasts them as owner records. Clients apply what they lack: `BreakConstraint()`,
  the gate's `Activate Event`, the trap's `Event Activate Trap`. A break anywhere is a break
  everywhere.
- **Settled props stay identical.** A free, anchored body that nobody touches is pinned: whenever
  it is more than 0.5 cm or 0.5° off its anchor while slow (< 2 cm/s), it is teleported onto the
  anchor and put to sleep (at most once a second per body). Each client's solver otherwise settles
  the same prop slightly differently after every heal. A real disturbance (my pawn or one of my
  bodies pushing it) still hands it to a lease. A prop something else knocked off its anchor and
  that is at rest again is pinned back at once. A body freed after following an owner takes the
  owner's final streamed pose as its anchor, not the local (interpolated, lagging) pose.

### Verification: the world hash

- Every 5 s after ready, each client puts a `world_hash` record: per static body
  `[id, record ver, status, x, y, z, packed quat]` and per actor state `[id, ver, status|4, bits]`,
  plus `hash` = FNV over the settled and missing rows (bit-identical in Lua and Rust; golden vectors
  in both test suites). Status: 1 alive, 2 settled (free, not followed, unmoved since the previous
  report), 4 actor state.
- The server diffs it against the most recent report (≤ 12 s old) of another peer in the level,
  over the bodies both can judge (same record version; settled on both, or settled on one and
  missing on the other):
  - pose: more than 5 cm or 3° apart;
  - presence: present and at rest on one screen, missing on the other;
  - state: different bits.
- The verdict (`world_verdict`, at most 24 mismatches) goes back to the reporter. HSMPWorld emits
  `world_consistency{hash_match, mismatched, mismatched_n, compared, hash_equal, peer, seq,
  level, epoch, world}` (hsmp_log) and heals: mismatched free bodies are forced back onto their
  anchors and actor states are re-applied. The server re-sends those anchors to both peers, logs
  `world consistency MISMATCH` and emits the same event in its events log (with `peer`, `other`,
  `kinds`).
- `hash_match` = no mismatch within tolerance. `hash_equal` = the exact quantised hashes agree
  (informational; cm rounding can flip it).

The test gate's rule WORLD-1 ([../testing.md](../testing.md)) checks, per round and instance during
Live: at least one verdict with `compared ≥ 1`; no id in `mismatched` of two consecutive verdicts
of the same level (a single mismatch is healed and allowed); the last verdict of the round has
`hash_match = true`. Verdicts come every 5 s, so only rounds whose Live lasted at least 10 s are
judged.

## Known limitations

- **Simultaneous pickup of one item:** the server grants one player. The loser's game still shows
  it in their hand; HSMPWorld logs `conflict:` and stops driving it until they drop it.
- **Disarming a stand-in on my screen** (a native hit knocks its loadout weapon loose) is decided
  locally.
- **Push attribution is by proximity**, so a thrown weapon of mine hitting a far prop is claimed
  only if I own that weapon and it is within 1.5 m.
- Barrel and plank joint bits follow the constraint FName order. A wall joint whose BeginPlay
  trace found nothing on one client shifts that client's order. Wall joints do not break, so in
  practice only the main joint matters.
- A fence's `Constraint Health` (accumulated damage) is not replicated; only the break is.
- Chest loot is per client (chests are destroyed under the MP profile).
- `Day Time` is not pinned, so candle props on Pit, Yard, Alley and Slums can exist on one screen
  only. They show up as `presence` mismatches.

## Tests

- `cargo test -p hsmp-server world`: every server rule, the static manifest, aliases, leases,
  epochs, relevance thinning, budget, actor states, the hash diff.
- `cargo test -p hsmp-ipc world`: record layouts, checks, quantisation.
- `hsmp-tools lua-test hsmpworld` and the `tests/hsmpworld-tests` crate: HSMPWorld logic with
  mocked UE4SS.
- `hsmp-tools lua-test world_state`: codec golden vectors, initial-state forcing and ready, hinge
  lease hand-off, actor states, world-guard safety, consistency report and verdict, barricade
  discovery.
- `hsmp-tools check-bp-names`.

## In-game checks

Use two clients on the same arena, ideally through `hsmp-tools netsim` with 100 ms delay and 2 %
loss.

1. **On load**, each client logs `level World /Game/Maps/Arenas/Map_Arena_X... -> level hash N`,
   `requesting world sync (try 1)`, `world sync complete: epoch=E ...`, four `scan k: +n bodies ->
   T total (weapons=a props=b multi-body=c); skipped: ...` lines, and `manifest X entries: bound
   ... exact ... alias ... class-mismatch ...`. Both clients should bind nearly every body, with
   class-mismatch 0.
2. **Server:** `world sync served`, and every 10 s `world 10s ... states_in=.. objs_out=..
   claims=.. granted=..`.
3. **Pickup:** A logs `claim mode 2 <class>/W#id (picked up; owner was 0)`; B sees the weapon leave
   the rack and move in A's stand-in's hand, with no second weapon in that hand.
4. **Drop:** A logs `claim mode 1 ... (let go (drop/disarm))` and, about 2 s after it lands,
   `release ... (at rest)`. B sees the same fall and the weapon rests at the same spot.
5. **Push a bucket or bench:** the pusher logs `claim mode 1 ... (pushed N cm/s)`; it comes to rest
   at the same place on both screens.
6. **Loadout weapon drop:** A logs `my R-hand item /Game/... left my hand -> world item ...`; B logs
   `peer A's dropped item ... spawned local copy`.
7. **Next round:** the server logs `world epoch bumped (arenas reload; objects reset)`; the clients
   log `world epoch E -> E+1 (round reset)`, then `world sync complete: epoch=E+1`.
8. **Late join mid-round:** the late joiner sees moved and dropped items where the others see them.
9. **Every 10 s**, each client logs `stats 10s: ... snap-backs`. A steadily climbing snap-back count
   means stand-ins are nudging props.
10. **Never expected:** `tick error`, `update error on ...`, repeated `conflict:` lines, or a
    `WORLD MISMATCH vs peer` that repeats for the same body.
