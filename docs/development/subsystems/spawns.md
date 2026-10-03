# Spawns, slow motion and the native lose flow (MP)

This document covers:

1. the server-authoritative spawn system (map data, assignment, wire, client placement,
   protection);
2. slow motion disabled in MP sessions;
3. the native lose / give-up travel blocked in MP sessions;
4. tests, log lines to check in game, and known gaps.

---

## 1. Server-authoritative spawns

### 1.1 Why the server decides

Two failure modes drove this design: players overlapping or spawning inside barriers, and players
starting on the same native spawner. Placing the local pawn on a native spawner chosen from the
peer id does not work: peer ids grow on every reconnect, and the spawners the LevelManager leaves
alive differ per client. So the server picks every spawn point from static map data, sends the
plan to everyone, and clients move their pawn only on that order, after a clearance check.

### 1.2 Map data (generated)

`hsmp-tools gen-map-data` reads `docs/arena_static/Map_Arena_<X>.json` and writes:

- `server/data/maps/Map_Arena_<X>.json` (`schema: hsmp-mapdata/1`), compiled into the server with
  `include_str!`;
- `mods/shared/hsmp_arenas.lua`, the Lua copy (HSMPMenu reads its spawn counts for the arena
  tiles). The cargo test `lua_arena_catalogue_matches_server_map_data` keeps it equal to the JSON.

`--check` exits 1 when a committed output is stale. `--report` prints the geometry table.

Each map file contains every `BP_SpawnerPoint_Willies` with its absolute position, yaw,
`Spawn Team Int`, game and play modes, and a `valid` verdict with a reason. It also has derived
`overflow` points, the `centre`, rough `bounds`, a `kill_z` and `max_players`.

**Validity rules:**

| Rule | Effect |
|---|---|
| `Works in these Play Modes` is false for every play mode | excluded: the Pit/Yard corner points outside the ring, the Cellar side-room points, the Narrow-Passage-only points, Slums 8–12 |
| Location at the world origin | excluded (unplaced template) |
| Within 120 cm of a barrier actor (fence / gate / door / portcullis) on the same floor (±2 m) | pushed straight away from it to 120 cm; a point *on* a barrier is excluded. Only Slums sp0 is affected: 82 cm from `BP_Fence_Flimsy_Small_C`, pushed 38 cm |
| Overflow points | the midpoint of two valid points on the same floor (\|dz\| < 40 cm, 2.5–9 m apart), at least 1.5 m from every other point. Excluded within 1.5 m of a trap or fence, or over a pit (a trap more than 3 m below within 2 m, such as the Pit's spike pit). Hazards from dynamic sublevels are ignored because those sublevels are not loaded in the MP combat mode |

The static data cannot tell which side of a fence a point is on, or whether an area is a spectator
gallery. The disabled corner and side-room points are the only ones that look like that, and they
are excluded. Everything else is caught at runtime by the client's clearance check (§1.5).

**Geometry rules.** `docs/arena_static` has no collision geometry, so the local floor is estimated
from what designers put on floors: every spawner and the floor-resting world objects (weapons,
quivers, traps, fences, destructibles, containers, dummies; dynamic sublevels excluded) within
5 m (XY) and ±4 m. A valid point more than 2 m above the lowest of them (roof, ledge, gallery), or
farther than max(15 m, 2.5 × median) from the centroid of the valid points (outside the arena), is
excluded at generation, and `gen-map-data --check` fails if a committed valid spawn or overflow
point breaks either rule. LordsHall has no floor evidence within 5 m (flat hall, z = 5
everywhere).

**MP arena-floor points.** Some spawners are disabled for every native play mode but stand on the
arena floor proper. `gen-map-data`'s `MP_ROOM_POINTS` makes them valid for MP, with
`why = "mp: arena-floor spawner (native play-mode flags off)"`, and only when they lie inside the
box of the natively valid points. They are then checked against the geometry rules like any other
point.

- Pit sp10 is the missing south-east point of the 5 m ring; sp1/sp4/sp5 are its mirror images.
- Cellar sp4/sp5/sp7/sp8 are the corner spawners of the main room.

**Spacing rule.** Any two spawns of a round are at least 4 m apart for 2–4 players and at least
3 m apart for 5–8 (`spawns::min_sep_cm`). Idle pawns closer than that were knocked down by each
other before the round began.

- Each map file carries `max_players`: the most players for which some valid + overflow subset
  meets the rule.
- `gen-map-data --check` fails when an arena is below 4, below its declared `CAPACITY_FLOOR`, or
  when `max_players` is stale.
- Only the Cellar stops below 8. It is one 5.8 × 5.6 m room, and its best 5 points are 298 cm
  apart. `spawns::capacity(arena)` exposes the limit.
- Above capacity the server still gives the best effort: distinct points, as far apart as
  possible.

| Arena | Valid | Overflow | Excluded | max_players |
|---|---|---|---|---|
| Alley | 12 | 9 | none | 8 |
| Pit | 8 | 8 | sp8, sp9, sp11–sp14 (disabled); sp10 used for MP | 8 |
| Yard | 8 | 13 | sp8–sp11 (disabled) | 8 |
| Slums | 8 | 6 | sp8–sp12 (disabled); sp0 nudged off a fence | 8 |
| Cellar | 8 | 2 | sp6, sp9, sp10 (east side room); sp4/5/7/8 used for MP | 4 |
| LordsHall | 6 | 8 | none | 8 |
| EastTower | 10 | 19 | sp8, sp9 (disabled) | 8 |

Every arena has at least 8 candidates (valid + overflow).

### 1.3 Assignment (`server/src/spawns.rs`)

`spawns::assign(arena, round, seats) -> Vec<SpawnAssign>`:

- **Deterministic.** The result depends only on (arena, round, seats). Seats are sorted first, so
  input order does not matter. The server passes each player's seat (stable per player key across
  reconnects), not the peer id.
- **Distinct.** Each seat gets its own candidate point. With more players than candidates, extra
  players get an offset ring (8 directions, 1.2 m steps) around the chosen points, maximising the
  distance to everyone already placed.
- **Spacing first, then maximum separation.** Subsets are ranked by:
  1. meets `min_sep_cm(n)`;
  2. widest pair at most `MAX_SPREAD_CM` (1600), so a duel on the 45 m Alley does not start with a
     minute of walking;
  3. the largest smallest pairwise distance.

  Valid points come before overflow points, unless only overflow points meet the spacing. The
  search is exhaustive up to 20 000 combinations and greedy farthest-point beyond that. If that
  misses the spacing, a depth-first search finds a subset that meets it: inside the spread cap
  first, then without it.
- **Rotation.** The chosen points are put in ring order around their centroid. Each round,
  everyone moves one place, so players swap sides.
- **Team-aware.** With teams, the points are ordered along the team axis and each team gets one
  side; the sides swap every round. The server passes `team: 0` today.
- **Facing.** Each player faces the centroid of the other players' spawns; a lone player faces the
  arena centre.
- `spawns::add_seat` adds a peer who joins or reconnects during the countdown. It picks the free
  point furthest from everyone and never moves existing seats.

**Server wiring (`server/src/server/match_core.rs`):**

- `begin_countdown` calls `plan_spawns`: every connected peer gets a point for round
  `match_round + 1`.
- `sync_spawn_plan` runs every tick during the countdown: it seats newcomers and frees the points
  of peers who left.
- `reset_to_lobby` clears the plan.
- Log lines: `spawn plan round=N peer_id=P slot=K src=spX derived=false x=.. y=.. z=.. yaw=..`, and
  `spawn report: pawn placed ... clear=.. offset_cm=..` for the client's report.

### 1.4 Wire (protocol v6)

The plan rides in the `session` record (`crates/hsmp-ipc/src/schema/session.rs`), the server's
full, idempotent session snapshot. Each roster row carries its spawn order: `spawn_id`
(`round << 8 | index`; 0 = none), `spawn_pos` (cm, floor level; the client ground-snaps),
`spawn_yaw`, `spawn_protect_ms` and `spawn_slot` (index into the arena's candidate list). The
snapshot is sent on every change and otherwise at 1 Hz in the lobby and 3 Hz in a match, so a lost
packet is replaced quickly and every client knows every spawn. The sidecar copies it into the
game's `session` slot as it arrived.

The client reports back with the G2S `spawned` record (`round`, `slot`, `pos`, `clear`), framed
C2S by the sidecar. The Director's `game_status` record also carries the last applied
`spawn_id`.

### 1.5 Client placement (HSMPSync `spawn_place.lua`)

The pawn moves **only** on the server's order:

1. The order must be for this round (countdown or paused: `round + 1`; live or roundover:
   `round`) and for the arena actually loaded (the world's short name equals the session arena).
2. **The new world must be fully up.** The world guard passed this tick (no level change pending),
   the world key has been unchanged for 1.5 s, the same pawn has been possessed for 1.5 s, and
   `HasActorBegunPlay()` is true. Until then nothing is traced or teleported, so nothing in the
   spawn path can touch the old or a half-built world.
3. **Ground snap:** a line trace from 150 cm above to 50 m below, against WorldStatic first, then
   Visibility.
4. **Clearance:** a `CapsuleTraceSingleForObjects` sweep, 5 cm long, sized from the pawn's
   `CapsuleComponent` (defaults r = 34, half-height = 88), 10 cm above the floor, against
   WorldStatic, WorldDynamic, Pawn and PhysicsBody. Ignored: our own held weapons and every
   `BP_SpawnerPoint_Willies_C` (its spawn-area `Sphere` answers the trace and would make every
   native spawn point read "blocked").
5. **Distance:** at least 1.5 m from every other `Willie_BP_C` (stand-ins, foes) and from every
   other player's planned spawn.
6. **Spiral** if the point is blocked: rings of 75, 150 and 225 cm × 8 directions. An offset may not
   change floor height by more than 150 cm, which rules out roofs and holes.
7. **Teleport the whole body.** The actor is moved with a physics teleport (plus rotation and
   control rotation). Every mesh component (`Mesh`, `SK_Skeleton`, `BoneCore`, `DriverSkeleton`)
   that did not follow (a simulating mesh is detached from the capsule) is moved rigidly by the same
   offset, and all body velocities are zeroed. From the second try on, a mesh whose component
   followed but whose pelvis did not is snapped by the pelvis residual. **Physics simulation is
   never switched on or off.** Switching it off does not re-attach a body: an earlier version left
   the visible ragdoll on the native player spawner while the capsule moved, and forcing simulation
   on for kinematic copies let one drop through the floor.
8. **Hold, then verify.** For 0.9 s after each teleport the body is held on the destination: every
   tick all body velocities are zeroed and an actor that drifted more than 10 cm (XY) is moved back
   rigidly (actor, meshes and Willie_BP's PhysicsHandle targets). After a teleport, Willie_BP's
   balance and step controller keeps its pre-jump state and walks the body about 1.3 m further in
   the direction of the teleport; the hold absorbs that. Then, 0.4 s and 0.8 s later, the actor
   must be within **1.0 m** (XY; `verify_tol_cm`, carried as `tol_cm`) and 2.5 m (Z) of the
   destination, and the visible body (`Mesh` pelvis) within 2.5 m. Otherwise it is teleported again
   (3 tries), else the placement is reported failed.
9. **Handshake with the Director.** The `spawn_status` bus record is written after every attempt
   (`verified = false`) and on success (`verified = true`). The Director waits on it and asks once
   for a re-place through the `spawn_request` bus record (see [director.md](director.md)).
10. **Report.** Once verified (or failed), the `spawned` record is sent 3 times, about 1 s apart.
    The server logs it once per round, with the distance from the plan point. Nothing is sent before
    the session is up; a full ring retries a second later.

**Only for a live MP session.** Orders apply only while `shared/hsmp_session.lua` says the session
is live (the `link` record says connected and the sidecar's heartbeat in the shared segment is
fresh). Leaving mid-Live and playing Free Mode in the same arena never teleports or protects the
single-player pawn.

If no order arrives within 20 s, the client keeps the native placement and logs this once. That is
the solo, lobby and unknown-map case; it never falls back to a local guess.

### 1.6 Spawn protection and the fall watchdog

- **Window:** from the first tick a pawn exists with an MP order **until the round goes Live**.
  Once the fight starts nobody is protected. The server uses the same window
  (`match_core::spawn_protected`: placed for the round being counted down, phase countdown), so
  from Live on there is no protected attacker and no protected victim on either side.
  - `protect_ms` (the order's, at least `SPAWN_PROTECT_MS` = 3000, at most 15 s on the client)
    only bounds a window before Live, for a failed placement that never sees Live.
  - `spawn_status` carries no `protect_until` until Live, then is rewritten once with the Live
    transition time (process clock) as the end.
  - A pawn possessed during Live (a native respawn, a world load that sits the round out) is not
    protected. The next countdown protects it again.
- **No collision with other Willies while protected.** Without this, idle pawns about 1.5 m apart
  knocked each other down before the round started (one hit at 951 uu/s dropped Consciousness to
  74, the pawn went down and the game dropped its weapons).
  - Every 0.5 s, every other `Willie_BP_C` (stand-ins, foes, hidden newborns) gets its collision
    with the local pawn disabled: the local pawn's `Mesh` and `SK_Skeleton` and held weapon
    `BaseMesh` / `Scabbard` against the other pawn's, in both directions.
  - It uses the game's own **CollisionDisabler** plugin (`DisableCollision_SkeletalVsSkeletal` /
    `_SkeletalVsSingleBody` / `_SingleBodyVsSingleBody` with each pawn's `Rigid Bones` and TTL −1),
    the per-pair filter Willie_BP itself uses between its skeletons and weapons. The capsules also
    `IgnoreActorWhenMoving` each other.
  - Channel responses were rejected because they would also drop collision with every other pawn
    and weapon. `SetActorEnableCollision(false)` was rejected because the ragdoll falls through the
    floor.
  - A pair is re-issued when its bodies change (a re-armed weapon). Collision is re-enabled
    (`EnableCollision_*`) when protection ends; while another Willie still overlaps us (< 150 cm)
    this waits, for at most 5 s. A possession swap also re-enables it.
  - Log: `spawn protection: no collision with Willie_BP_C_N (ok: ...)` and `... collision with N
    other Willie(s) restored`.
- **While protected:** the pawn's `Invulnerable` is kept true. Willie_BP's `Get Damage` returns at
  once when it is set, and so do the pain and consciousness ticks. The native BeginPlay clears it
  2 s after a spawn, so it is re-asserted every tick. Vitals are topped up from the CDO every 0.5 s
  if Health dropped. At the end `Invulnerable` is cleared once (not on a dead pawn).
- **Fall watchdog:** while protected, a pawn 4 m below its placed floor (or pushed 3 m off its spot
  before Live) is re-placed; afterwards, a pawn 15 m below the lowest known floor (void) is.
  Nobody dies from falling out of the world.
  - A pawn with `DED` or `Health <= 0` is left where it is (logged once).
  - Before Live, the rescue heals the pawn from the CDO, re-arms the protection and re-places it
    on its order (native spawner 0 when there is none).
  - During Live, the living pawn is re-placed on its order (or the native spawner) without a heal
    and without protection: a fall is not a free reset.
- **A new pawn** (native re-possession, or HSMPAvatars' `Spawn Combatants` stealing possession)
  gets its own placement and window.
- **The stand-in body.** MP arenas never spawn a foe, so each client gets its stand-in bodies from
  `BP_LevelManager."Spawn Combatants"`, which spawns at a random entry of the chosen spawner's
  `Spawn Locations`, re-traced from a grid inside the spawner's `Sphere`. That often put the body
  on our own spawn point. HSMPAvatars therefore moves every live spawner to the peer's pose,
  shrinks its `Sphere` to 1 cm and clears both lists, so the body appears where the peer stands.
  That is local only; a world reload restores the spawners.

### 1.7 Weapons at spawn and the `pawn_state` events

**Kit weapons (HSMPLoadout `kit.lua`).**

- `Weapon_Fists_C` and `Weapon_Feet_C` never count as a held kit weapon. Every `kit_status` write
  re-checks both hands, so `ok` is false whenever a kit hand holds fists, nothing or another class.
  `r_class` / `l_class` are always the real actors' classes.
- After the 3 s stability window the hands are watched every 0.5 s for the rest of the world:
  - **Countdown / paused, or inside spawn protection:** a dropped kit weapon is re-equipped at
    once (weapons only, at most 8 times). The dropped actor itself goes back in hand
    (`"Set Up Right/Left Hand Weapon"(Class, Actor, ...)`), and kit.lua checks that this very
    actor is in the hand afterwards.
  - **HSMPLoadout never destroys a weapon that left the own hand.** HSMPWorld registers a loose own
    weapon as a dynamic world item 100 ms after it left the hand, and peers spawn copies of it.
    Destroying it would leave a free ghost weapon on every other screen. Re-equipping the same
    actor is, for HSMPWorld, its own item being picked up again: one weapon on every screen. If
    the re-equip does not take, or somebody else holds the dropped weapon, a new kit weapon is
    given and the dropped one stays a replicated world item (`(dropped actor left as a world
    item)`).
  - **Live, outside protection:** a drop is gameplay, so there is no re-arm. `kit_status` gets
    `ok = false, error = "dropped during Live: R (Weapon_Fists)"` and `pawn_state{at=weapon_drop}`
    is emitted. A kit weapon picked back up reads ok again.

**`pawn_state` events** (`shared/hsmp_log.lua`): required fields `consciousness, downed,
weapon_r, weapon_l`; also `at, round, pawn, fallen, health, protected, live, dist_cm`, plus
`reason` for kit drops.

- HSMPSync writes them at `placed`, `ready` (the Director's state Ready/Live), every 5 s while
  protected (`protect`), at `live` and at `protect_end`.
- HSMPLoadout writes them at `rearm` and `weapon_drop`.
- The UE4SS.log mirror line is `pawn_state[protect] round 1: consciousness=100 downed=false
  fallen=false health=100 R=ModularWeaponBP_... L=... protected=true 3 cm from the spawn`.

**Gate rule PAWN-1** (`hsmp-gate`, in `p0_gate` and `p0_wifi`; [../testing.md](../testing.md)).
For every `pawn_state` with `protected = true` and `at` ∈ placed / ready / protect / live:
`downed == false`, `consciousness >= 95`, `dist_cm <= 100` (measured from the placement
destination), and `weapon_r` / `weapon_l` not fists / None when the player's kit has a weapon in
that hand (read from the instance's own `kit_verified{who="self", ok=true}`). There must also be
no `pawn_state{at=rearm}` after `at=ready` in a round: a drop that needed a re-arm before Live
means something still hits the idle pawns. The rule names the first offending event.

**Known issue.** In rare rounds a fighter still spawns slightly off its point, is knocked down, or
needs a re-arm right after the round begins. PAWN-1 is the check that catches these in gate runs,
and the `pawn_state` event it names is the starting point for a diagnosis.

---

## 2. Slow motion: fully disabled in MP

### 2.1 Every path (evidence)

From `UE4SS_ObjectDump.txt` plus Kismet bytecode decompiled with `tools/mapdump --probe` and
`hsmp-tools kismet-pp`:

| Path | Evidence |
|---|---|
| SloMo input action | `Willie_BP_C:InpActEvt_SloMo_K2Node_InputActionEvent_8`. If `Consciousness > 50` and `Stamina > 0.5`, it toggles: `PlaySound2D(SlowMoEffect)`, `Slomo Active = true`, `Slomo Timeline.Play()`, or else `.Reverse()` |
| Slomo timeline | `Slomo Timeline__UpdateFunc` sets GI `Time Dilation` = FClamp(`Slomo_Timeline_Time_Dilation`, lerp(0.75→0.25 by `Body Skill (Temp)`), 1), then `Arrow Time = GI.Time Dilation`, then `SetGlobalTimeDilation(Arrow Time)` plus vignette / fringe post-process |
| Arrow time | `Willie_BP_C:End Arrow Time Event` re-applies `Arrow Time` to the global dilation |
| Death slow-mo | `UI_Lose_C` and `UI_DED_C` / `UI_DeathDoor_C` (Construct): GI `Time Dilation = 0.25` plus `SetGlobalTimeDilation`. `UI_GiveUp_C`, `UI_WinScreen_C`, `UI_WeaponSelection_C` and `BPC_PhotoMode_C` also call `SetGlobalTimeDilation` |
| GI setting | `GI_Settings_C:Time Dilation` (double) |
| Console | `/Script/Engine.CheatManager:Slomo` |
| Unlock flag | `Willie_BP_C:Skill Unlock Body Slomo` (bool). Not read anywhere in Willie_BP bytecode; it is still set false |
| Per actor | `/Script/Engine.Actor:CustomTimeDilation` |

### 2.2 What HSMPMatch does (MP sessions only)

"MP session" is the shared predicate of `shared/hsmp_session.lua`: the sidecar's `link` record
says connected and its heartbeat in the shared segment is fresh. A crashed or killed sidecar stops
beating, so single-player is never affected. The log line is `MP session ACTIVE: slow-mo off,
native lose flow blocked` / `MP session inactive ...`.

- **At the source:** a pre-hook on `GameplayStatics:SetGlobalTimeDilation` rewrites any value
  other than 1 to 1.0 (this covers every BP caller above); a pre-hook on `CheatManager:Slomo` does
  the same.
- **On the triggers** (Blueprint hooks run after the function body in the pinned UE4SS build, see
  [../ue4ss.md](../ue4ss.md), so they run right after, in the same frame): `InpActEvt_SloMo…`,
  `Slomo Timeline__UpdateFunc` and `End Arrow Time Event`. Each one stops the timeline and sets
  its time to 0, clears `Slomo Active`, sets `Arrow Time` and CustomTimeDilation to 1, turns the
  unlock off, and restores global and GI dilation. These hooks are registered lazily once
  Willie_BP is loaded.
- **Guard, every 50 ms:** resets global time dilation and GI `Time Dilation` to 1.0; every
  500 ms it also resets every Willie's `CustomTimeDilation`, `Arrow Time`, `Slomo Active`, the
  timeline and the unlock. Fresh lookups only, skipped while a level change is pending.

Each correction is logged, rate-limited to once per 10 s per kind, with a running count.

---

## 3. The native lose flow (death → Tavern) blocked at the source

**Evidence (bytecode):**

- Willie_BP (player give-up / downed path): if the GameMode's `Match Won` is false and no
  `UI_NextFIght` exists, it calls `Create(UI_Lose_C)`.
- `UI_Lose_C`, in order: `Time Dilation 0.25`, `Delay(0.25)`, `Delay(1.0)`, `IsValid(self)`,
  `Health = 100`, `SetGamePaused(true)`, **`GI.Save Game()`** (writes the career slot), then
  `OpenLevelBySoftObjectPtr(Map_Hub_Tavern_Frank | Map_Menu_Startup)`.
- `UI_DED_C`, `UI_DeathDoor_C`, `UI_GiveUp_C` and `UI_NextFIght_C` all reach their
  `OpenLevelBySoftObjectPtr` behind a `KismetSystemLibrary:Delay` latent action owned by the
  widget.

Removing the widget from the viewport does not stop a pending Delay, so suppressing the widget
alone is followed by a hub load.

**Fix (HSMPMatch, MP sessions only):**

1. A pre-hook on the native `/Script/Engine.KismetSystemLibrary:Delay` rewrites `Duration` to
   10⁷ s when the latent owner is one of `UI_Lose_C`, `UI_DED_C`, `UI_DeathDoor_C`, `UI_GiveUp_C`,
   `UI_NextFIght_C`, `UI_WIN_C` or `UI_WinScreen_C`. The continuation (pause, career save, travel)
   never runs before the server's round reset reloads the world. The dead player stays in the arena
   and spectates.
2. Second layer: the Director is the only code that changes levels in an MP session, and it
   rewrites every native `OpenLevel` while a session exists, so a native travel that still fires is
   rerouted to the server's arena (mid-match) or the menu (after the match).

Log: `blocked native lose-flow travel (UI_Lose_C:Delay) x1`, on the 1st and every 20th occurrence.

---

## 4. Tests and checks

- `cargo test -p hsmp-server spawns`: `every_mp_arena_has_a_table`,
  `distinct_for_2_to_8_players_every_arena_every_round`, `deterministic_and_order_independent`,
  `duel_spawns_are_far_apart_and_face_each_other`, `max_separation_beats_every_other_subset_on_pit`
  (brute force), `players_rotate_between_rounds`, `overflow_and_wrap_stay_distinct`,
  `teams_take_opposite_sides_and_swap`, `add_seat_keeps_existing_and_picks_a_free_point`,
  `lua_arena_catalogue_matches_server_map_data`, `disabled_and_template_spawners_are_never_valid`,
  `unknown_arena_gives_empty_plan`; plus
  `match_core::round_tests::countdown_publishes_distinct_spawn_plan_and_seats_late_peers`.
- `hsmp-tools lua-test spawn_place`: placement → verify → status, snap-back, body left behind,
  Director retry; protection (held until Live, ends at Live), dead pawns, falls (dead pawns left
  alone, Live falls without heal or protection), drift, new pawn, spiral, no ground, no order, no
  live session, world change; the `spawned` report (held until connected); no-collision pairs
  (re-issue, late stand-in, restore, overlap deferral, possession swap); the `pawn_state` events.
- `hsmp-tools lua-test kit_status`: the kit evidence and the re-arm rules.
- `scripts/e2e-test.sh`: **T10s** (both clients see the same 2 distinct server spawns for round 1
  in the session roster) and **T29a** (the live round still carries the plan).
- `hsmp-tools gen-map-data --check` and `hsmp-tools check-bp-names` both exit 0.

### In-game log lines (UE4SS.log)

| Mod | Line | Means |
|---|---|---|
| HSMPSync | `spawn: round 1 slot 3 -> (-524,-7,101) clear=yes yaw=0 id=256 [round start]` | placed exactly on the server point |
| HSMPSync | `spawn: round 2 slot 5 -> (..) offset=(75,0) ...` | point blocked; spiral offset used |
| HSMPSync | `spawn: ... clear=NO (blocked everywhere; placed on the server point)` | check the arena data for that slot |
| HSMPSync | `spawn: no server spawn order for this world (...)` | no plan (solo, lobby), or a round or arena mismatch (the reason is printed) |
| server | `spawn plan round=N peer_id=P slot=K src=spX ...`, then `spawn report: pawn placed ... offset_cm=..` | plan made; the client placed the pawn |
| HSMPMatch | `MP session ACTIVE: slow-mo off, native lose flow blocked` | the guard is armed (never in single-player) |
| HSMPMatch | `slow-mo hook registered: /Game/Character/Blueprints/Willie_BP.Willie_BP_C:InpActEvt_SloMo_K2Node_InputActionEvent_8` (and two more) | BP hooks are live |
| HSMPMatch | `slow-mo guard: corrected SetGlobalTimeDilation call: 0.25 -> 1.0 (x1 this session)` | a death or slow-mo dilation was neutralised |
| HSMPMatch | `blocked native lose-flow travel (UI_Lose_C:Delay) x1` | the death flow cannot travel to the Tavern |

---

## 5. Known gaps

- Team ids are not populated yet (`team: 0`): the game modes in `server/src/modes` are not wired
  into the server (see [modes.md](modes.md)).
- If `CapsuleTraceSingleForObjects` errors (Lua marshalling of the object-type array and hit
  struct), the clearance check reports clear and only the distance check applies.
