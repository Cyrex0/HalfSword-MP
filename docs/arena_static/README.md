# Static arena data (spawn points, world objects, runtime spawners)

Extracted **statically** from Half Sword's cooked `.umap` files: no game launch and no UE4SS. The tool reads
`game/HalfswordUE5/Content/Paks/pakchunk0-Windows.pak` read-only, using the AES key and the UE4SS `.usmap`
(unversioned-property mappings). It covers all **88** `.umap` packages in the game. `pakchunk0optional` has none;
it holds only one sky texture.

| Path | What |
|---|---|
| `Map_Arena_<Name>.json` (7) | One file per arena (Alley, Pit, Yard, Slums, Cellar, LordsHall, EastTower). Each includes everything it streams and every level instance it pulls in. |
| `all_maps/<Path__Name>.json` (81) | Every other umap, each processed standalone: hub, menus, test maps, Abyss, sublevels, lighting levels, and level instances. |
| `_index.json` | All 88 maps: kind, parse status, spawn/world-object counts, `pulled_in_by` (which maps stream it) and `referenced_by_assets` (which BPs/UI name it, for example OpenLevel targets). |
| `_summary.md` | Generated markdown tables: per-arena spawn lists with mode filters, and the all-maps index with unused/dev flags. |
| `_combat_events.json` | Every `BP_CombatEvent_*` definition (map, base `Combatants Amount`, combat modes, tiers, lose conditions), with CDO inheritance resolved. |
| `bytecode/*.txt` | Decompiled Kismet pseudo-code (offset: statement) for the BPs that drive spawning: `BP_LevelManager`, `BP_SpawnerPoint_Willies`, `BP_GameManager`, `BP_Generator_Weapons_Random`, `BP_Container_Master`, and the Alley/Pit/Cellar level BPs. |

## Regenerate

```powershell
powershell -ExecutionPolicy Bypass -File tools\mapdump\run.ps1             # all 88 maps (~1 min)
powershell -ExecutionPolicy Bypass -File tools\mapdump\run.ps1 -ArenasOnly # just the 7 arenas
powershell -ExecutionPolicy Bypass -File tools\mapdump\run.ps1 -Maps Map_Arena_Alley
```

- `run.ps1` installs a **user-local .NET 10 SDK** into `%LOCALAPPDATA%\dotnet10` if it is missing. The current
  CUE4Parse (`1.2.2.202610`, the first version that reads usmap v4) only targets net10. The script then builds
  `tools/mapdump/MapDump.csproj`, runs it, and runs `hsmp-tools mapdump-summary`.
- Inputs come from the environment (`run.ps1` sets them): `HSMP_GAME_DIR` (the game, default `<repo>/game`),
  `HSMP_OODLE_DLL` (your own `oo2core_9_win64.dll`, default `workspace/tools/repak/`, never committed) and
  `HSMP_PAK_AES` (the pak key; `Program.cs` carries a default). The `.usmap` is read from the game's `ue4ss` folder.
- Extra modes:
  - `MapDump.dll --probe <pkg> [export|*]` dumps a package's exports as JSON, including Kismet bytecode.
  - `hsmp-tools kismet-pp dump.json [funcRegex]` turns that dump into readable pseudo-code.
  - `--scan <pathRegex> <needle1,needle2>` lists packages whose name table contains all the needles.
  - `--selftest` checks the transform math.

## JSON schema (per map)

`{map, package, kind, units, sources[], spawn_points[], world_objects[], spawners[], other_actors[], static_mesh_actors_static_count, errors[], warnings[]}`

- `sources[]`: every package that contributed. Each entry has `file`, `via` (`persistent`,
  `streaming:LevelStreamingAlwaysLoaded_N`, `streaming:LevelStreamingDynamic_N`, or `level_instance:<actor> (<label>)`),
  `parent_chain`, and `level_transform` (the composed LevelTransform or level-instance transform).
- Every actor entry has:
  - `name` (FName in its level), `label` (editor ActorLabel), `class`, `class_path` (`/Game/...`), and `class_chain`
    (BP super chain, then native chain via usmap).
  - `source` (umap) and `via` (only when not persistent).
  - `location` [x,y,z], `rotation` {pitch,yaw,roll}, and `scale`, all in **world space**: cm and degrees.
  - `attached_to`, when the root component is attached to another actor's component.
  - `tags`: effective Actor Tags. These drive runtime logic; see below.
- `spawn_points[]` adds `instance_properties` (what the level overrides) and `effective_properties` (instance values,
  then the BP CDO chain). Real BP names are used, spaces included: `Works in these Game Modes`,
  `Works in these Combat Modes`, `Works in these Play Modes`, `Spawn Team Int`, `Spawn Player`, `Spawn Sphere Radius`,
  `Required Combatants Amount`, and so on. User-defined enum values print as `Enum_X::NewEnumeratorN (DisplayName)`.
- `world_objects[]` adds:
  - `category`: one of weapon, quiver, trap, destructible, fence, chain, lever, light_prop, container,
    training_dummy, character, static_mesh_actor_movable, or physics_other.
  - `simulate_physics` and `simulating_components[]`, which come from `BodyInstance.bSimulatePhysics` on the
    instance, then the SCS template, then the CDO.
  - `root_mobility`.
  - `static_mesh`, for StaticMeshActors.
  - `instance_properties`.
  - `weapon_overrides` and `bp_simulates_physics` (the weapon BP's own `Simulates Physics` variable), for weapons.
  - `child_actor_components[]` (`ChildActorClass` plus the spawned child's level name).

  Child actors such as barrels inside `BP_Structure_Trap_*` are serialized as separate level actors and are listed
  in their own right, with `attached_to`.
- `spawners[]`: runtime spawners and managers, meaning BP_LevelManager, BP_GameManager, the level script actor,
  containers, and anything whose properties reference BP classes. Each entry carries `effective_properties` (class
  arrays and similar). Large defaults that hold no class references are listed in `__omitted_large_defaults`.
- `other_actors[]`: a brief list of every remaining placed actor (lights, decals, volumes, LevelInstance actors, and
  so on). Static, non-simulating StaticMeshActors (architecture) are only counted, in
  `static_mesh_actors_static_count`.

## Validation

Alley in-game showed 3 `BP_SpawnerPoint_Willies` at about (-1246,226,99) yaw -60, (248,-247,184) yaw 100 and
(1310,176,406) yaw 43. The static output has `_C_8` (-1246.3, 226.3, 98.7) yaw -60.17, `_C_5` (247.8, -247.3, 184.0)
yaw 100.05, and `_C_0` (1310.2, 176.4, 406.2) yaw 42.67, an exact match.

Those three are exactly the Alley spawners whose `Works in these Game Modes` includes **Tavern**, and Alley's combat
event has `Combatants Amount = 3`. This matches the selection logic below: the other 9 were filtered out and
destroyed at round start. `--selftest` covers the transform math, including parent rotation and scale, pitch/roll
round-trips, and the gimbal case.

## Arena summary

| Map | Combat event (base combatants) | Spawn pts (enabled) | Weapons | Destructibles | Traps | Fences | Chests | Light props | Levers/Chains | Movable/sim SMAs | Runtime spawners |
|---|---|---|---|---|---|---|---|---|---|---|---|
| Alley | Alley (3) | 12 (12) | 3 tools + 1 quiver | 22 (9 barrels, 13 planks/boards) | 5 `BP_Structure_Trap_1..5` | 0 | 3 | 0 | 0 | 35 | LevelManager, GameManager, 3 chests, level BP |
| Pit | Pit (1) | 14 (7) | 0 | 1 | 35 spike traps (12 of them in NarrowPassage_Spikes sublevel) | 0 | 0 | 56 candles (night/evening lighting sublevels only) | 0 | 60 | LevelManager, GameManager, level BP |
| Yard | Yard (1), Tourney (1) | 12 (8) | 0 | 0 | 0 | 0 | 0 | 56 candles (lighting sublevels) | 0 | 58 | LevelManager, GameManager, level BP |
| Slums | Slums (1) | 13 (8) | 6 tools | 5 boards | 2 spike rows (NarrowPassage sublevel) | 18 `BP_Fence_Flimsy_*` | 3 | 0 | 0 | 18 | LevelManager, GameManager, 3 chests, level BP |
| Cellar | Cellar (1) | 11 (4) | 5 (3 tools, rondel, flail) | 0 | 2 (`Trap_Kettle_BP` + `BP_Weapon_Trap_Kettle`) | 0 | 2 | 29 (13 movable candles) | 4 `Chain_BP` | 108 | LevelManager, GameManager, 2 chests, candle stand, level BP |
| LordsHall | Tourney_Baron (1) | 6 (6) | 9 (4 polearms/dagger, pavise, 4 treasure cups) | 0 | 0 | 0 | 2 | 72 (36 candles + 36 candle lights) | 0 | 30 | LevelManager, GameManager, 2 chests, level BP |
| EastTower | Hall "Slaughter Hall" (4) | 12 (10) | 5 polearms | 0 | 12 (6 `Trap_BP` + 6 `BP_Weapon_Trap`) | 0 | 0 | 56 (incl. 2 chandeliers) | 12 (6 `ST_Lever` + 6 `ST_LeverActivated_Child`) | 4 | LevelManager, GameManager, 7 candle stands, level BP |

The "enabled" count excludes spawners whose `Works in these Play Modes` is false for every play mode; they can
never pass the filter. Not present in any arena: `BP_Fence_Bags`, `BP_Prop_Training_Dummy` (one appears in Cellar
and in Arena_Cutting_Map), and `BP_Container_*` other than `Chest_002..005`.

### Spawn points

All spawn points are `BP_SpawnerPoint_Willies_C` (`/Game/Blueprints/Spawner/BP_SpawnerPoint_Willies`). Each is
listed as index:(x,y,z) yaw, followed by the game modes that are true. "DISABLED" means no play mode is true.
"teamN" is the `Spawn Team Int` override. Full mode maps are in `_summary.md` and the JSON. Every spawner has
pitch = roll = 0, and all sit in the persistent level.

- **Alley**: 0:(1310,176,406) y43 [Errand/Tavern]; 1:(815,280,319) y-128 [Arena] team1; 2:(-541,160,173) y-109 [Arena]; 3:(-2460,-219,35) y70 [Arena] team1; 4:(1388,0,411) y-177 [Arena] team1; 5:(248,-247,184) y100 [Tavern] team2; 6:(1486,570,416) y-120 [Arena] team2; 7:(-472,-632,167) y80 [Arena] team2; 8:(-1246,226,99) y-60 [Tavern]; 10:(-2092,236,43) y-84 [Arena] team1; 11:(-2728,99,34) y-5 [Arena] team1; 12:(-93,139,184) y-95 [Arena] team2
- **Pit**: 0:(2,509,1) y-90 [Sparring/Tavern]; 1:(-391,-394,1) y45 [Arena]; 2:(2,-584,1) y90 [Sparring]; 4:(346,334,1) y-135 [Arena]; 5:(-338,334,-1) y-45 [Arena]; 6:(527,9,1) y180 [Arena]; 7:(-524,-7,1) y0 [Arena]; 8:(1,218,1) y-90 Narrow-Passage-only, DISABLED; 9:(2,-275,1) y90 Narrow-Passage-only, DISABLED; 10:(356,-359,2), 11:(646,-625,2), 12:(651,651,2), 13:(-634,629,2), 14:(-628,-670,2) DISABLED
- **Yard**: 0:(1,438,1) y-90 [Sparring/Tavern]; 1:(-324,-315,1) y45 [Arena]; 2:(1,-472,1) y90 [Sparring]; 3:(329,-324,2) y135 [Arena]; 4:(315,292,1) y-135 [Arena]; 5:(-299,327,-1) y-45 [Arena]; 6:(468,1,1) y180 [Arena]; 7:(-435,3,1) y0 [Arena]; 8:(-637,625,-1), 9:(644,614,-1), 10:(660,-596,2), 11:(-624,-646,1) DISABLED
- **Slums**: 0:(42,568,829) y-91; 1:(-408,163,811) y-43; 2:(-490,-86,833) y-4; 3:(-83,-434,867) y92; 4:(517,250,855) y-153; 5:(257,415,853) y-125 [Tavern]; 6:(623,17,886) y-169; 7:(-561,-516,886) y41 (0-4 and 6-7 are [Arena]); 8:(558,465,895), 9:(865,832,907) Narrow-Passage-only, DISABLED; 10:(1591,648,902), 11:(1046,1063,902), 12:(1419,946,902) DISABLED
- **Cellar**: 0:(1010,708,13) y180 [Errand/Training/Tavern] team1; 1:(433,708,13) y0 [Arena] team2; 2:(722,1001,13) y-90 [Arena] team1; 3:(722,442,13) y90 [Arena] team2; 4-10 DISABLED: (482,958), (460,477), (1677,396), (923,927), (945,490), (1740,1078), (1854,713), all at z=13
- **LordsHall**: 0:(-796,233,5) y90; 1:(-796,2048,5) y-90; 2:(-401,1563,5) y180; 3:(-1204,1572,5) y0; 4:(-401,742,5) y180; 5:(-1204,742,5) y0. All [Tavern]; 1-5 exclude the Fisting combat mode.
- **EastTower**: 0:(4,2118,5) y-90; 1:(409,1802,5) y180; 2:(4,603,5) y90 [Arena]; 3:(-434,1798,5) y0; 4:(409,1394,5) y180; 5:(-434,978,5) y0; 6:(409,970,5) y180; 7:(-434,1399,5) y0; 8:(360,2244,5), 9:(-347,2204,5) DISABLED; 10:(-353,540,5) y55 [Arena]; 11:(385,603,5) y130 [Arena]. Indices without a tag are [Tavern].

Also note `Map_Arena_Ambush_Test` (the "Forest Ambush" combat event, `BP_CombatEvent_Forest`): it has **29** spawn
points, 4 chests, a crossbow, a quiver and 3 `Wasp_Nest_BP`. See `all_maps/Maps__Arenas__Map_Arena_Ambush_Test.json`.

## How the game picks spawn points at runtime

The logic lives in `BP_LevelManager.Spawn Combatants` (see `bytecode/BP_LevelManager.txt`):

1. `GetAllActorsOfClass(BP_SpawnerPoint_Willies_C)`, then **`Array_Shuffle`**. The order is random.
2. Filter each spawner on four conditions, all of which must hold. `Map_Find` on a missing key counts as false.
   - `Works in these Game Modes[GI.Current Game Mode enum]`
   - `Works in these Play Modes[GI.Current Play Mode]`
   - `Works in these Combat Modes[GI.Current Combat Mode]`
   - `Required Combatants Amount <= count`

   In Free Mode, Current Combat Mode is forced to Carnage (1), or to 3 when `Free Mode Carnage` is set.
3. The first filtered spawner becomes the **player spawner**. Remaining slots are chosen by distance from it: more
   than 500 cm, or more than 3x the current best distance. Each spawner then gets `Spawn Willies`, with
   `Spawn Player`, `Spawn Mercenary`, `Start in Pants` and `Blossfechten Gear` set at runtime. Spawners that are not
   used are **K2_DestroyActor'd**. That is why only 3 remained in Alley.
4. `BP_SpawnerPoint_Willies` spawns a `BP_Generator_Characters_Random_C`, which builds a Character Passport. The
   spawner then spawns `Willie_BP_C` with `Team Int`, `Character Passport`, `Spawn in Pants`,
   `Start Body Condition`, `Blossfechten Gear` and `Bolts in Quiver`.

The candidate set is fixed for a given (game mode, play mode, combat mode). **Which** candidate is the player spawn,
and the shuffle order, are random every round.

## Runtime weapon / prop spawning

Everything here comes from bytecode, so it is static evidence; the in-game counts are not reproduced. There are
several sources of runtime-named (`..._2147xxxxxx`) ModularWeaponBP actors:

1. **Every Willie** (`Willie_BP`) spawns its own body weapons:
   - 2x `Weapon_Fists_C` and 2x `Weapon_Feet_C`. Both are `ModularWeaponBP_C` subclasses, attached to sockets.
   - Its Character Passport equipment: hand weapons through `Set Up Left/Right Hand Weapon`, and sheathed slot
     weapons spawned from `WeaponClass` in the passport maps.
   - A `BP_Quiver_Bolt_1_C` when it has bolts.

   Classes come from the randomly generated passport (`BP_Generator_Characters_Random` / `BP_Generator_Weapons_Random`,
   tier from the combat event's `Available Tiers` and equipment budget). They are **random per round**, and the
   positions follow the Willie.
2. **`Clean Up Map` runs synchronously in `BP_LevelManager` BeginPlay**, before anything else. It has two branches,
   chosen by `X = Free Mode Activated OR Current Game Mode != Arena(1) OR Combat Mode == Duel(0) OR Combat Mode == Riot(10)`.
   (`Enum_GameMode`: 0 Tavern, 1 Arena, 2 Errand, 3 Training, 4 Hell, 5 Sparring.)
   - **X true**, which covers every Free Mode session and the Tavern/Errand game modes: **every** placed
     `ModularWeaponBP_C` not tagged `Trap` is destroyed, along with every `BP_Container_Master_C` (chests), every
     `BP_Quiver_Master_C` and every `BP_Armor_Master_C`. None of the placed weapons, chests or quivers in this JSON
     survive. Spike and kettle traps (tagged `Trap`) do. This is consistent with the Alley test: its 3 spawners
     were the Tavern-mode ones, and none of the ~57 weapons seen in-game were level-placed.
   - **X false** (Arena game mode in Progression, with a combat mode other than Duel or Riot): **SpawnChance
     culling.** Each weapon, quiver and chest tagged `SpawnChance` is destroyed with `RandomBoolWithWeight(w)`. The
     weight depends on `GI.Day Time`: 0.666, 0.75, 0.75 or 0.444. The tag is removed from survivors. The chest
     `Tier` is set to `clamp(Player Tier + rand(0..4), 0, 7)`. The surviving set is **random**.

   `Clean Up Map` also destroys every `Willie_BP_C` and `BP_NavigationBlocker_C` not tagged `Persistent`. The placed
   `Willie_Drunk` in Alley is tagged `Persistent`, so it survives.
3. **Surviving placed weapons are replaced.** Right after `Clean Up Map`, `BP_LevelManager` runs
   `GetAllActorsOfClass(ModularWeaponBP_C)`. For each weapon tagged `Persistent`, and not tagged `NoSpawn` or
   `Trap`, it:
   - spawns a `BP_Generator_Weapons_Random_C` and calls `Generate Weapon(type 0, tier 0, Spawn Specific Weapon = true,
     Specific Class = <original class>, Specific Passport = <original Weapon Passport>)`;
   - spawns `Passport.WeaponClass` **at the original actor's transform**;
   - copies `Simulates Physics`, then destroys both the generator and the original.

   All level-placed arena weapons carry `Persistent` + `SpawnChance`. When they survive step 2, their **class and
   transform are deterministic** (the values in this JSON), but the live actor has a runtime name.
4. **Chest contents** (`BP_Container_Master`, at BeginPlay after a delay of 0; only for chests that survive step 2):
   - It rolls a random weapon from the tiered `Weapon S/L Array_<tier>` class arrays via
     `BP_Generator_Weapons_Random`, and random armor via `BP_Generator_Armor_Random` (`Armor S/L Array_*`).
   - It may also spawn a `Treasure Passport` item, gated by `RandomBool` and a weight based on Day Time.
   - Items appear at the chest's `Weapon Spawn Point`, `Armor Spawn Point` and `Treasure Spawn Point` components.

   Those class arrays are in each chest's `spawners[].effective_properties`. The result is **random**.
5. **Equipment-pool generation** (`BP_GameManager.Generate Random Equipment`, run at BeginPlay): this tops up
   `GI.Available Weapons 1H`, `Available Weapons 2H` and `Available Shields` to 25 each, plus extra 1H rolls, and
   Custom Armor to 100. Each weapon roll spawns a `BP_Generator_Weapons_Random` at **(0,0,0)**. That generator spawns
   a temporary base-class **`ModularWeaponBP_C`** to compute the passport, then destroys both.

   This is the most likely source of large counts of exact-class `ModularWeaponBP_C` with runtime names (for
   example "~57 in Alley"). If so, they should sit at or near the world origin and be pending-kill, or be gone
   after GC. **Verify in-game** by checking `IsValid` and the location of those actors.
6. **Respawn / Random Delete.**
   - `BP_LevelManager` destroys actors tagged `Respawn` and spawns a fresh actor of the same class at the same
     transform, copying Tags. This covers all `BP_Structure_Trap_*` in Alley, all Pit spike traps, and some boards.
     Their transform is deterministic, but the name is new.
   - When `Purge Map` is set, actors tagged `Random Delete` are destroyed with 50% chance. No arena object has this
     tag in the cooked data.

For world replication this means:
- Static `world_objects` transforms and classes are reliable for traps, destructibles, fences, levers, chains,
  candles and physics props.
- Placed weapons, chests and quivers either all disappear (Free Mode, Tavern/Errand modes, Duel, Riot) or are
  culled at random and re-spawned under runtime names (Arena progression).
- Live identities of surviving weapons and respawned traps differ from the level FNames. Match them by class plus
  transform, not by name.
- Chest loot, NPC loadouts, SpawnChance culling and spawn-point choice are random and must be replicated from the
  host.

## Other maps (scope expanded to every umap)

Flags come from `_index.json` (`pulled_in_by`, `referenced_by_assets`) and `_combat_events.json`:

- **Map_Hub_Tavern_Frank**: the hub, referenced by GI_Settings and the death/lose UIs. It streams
  `Workshop_Smithery_Map`. It has no spawn points, 23 world objects (`Willie_BP_DressUp`,
  `ModularWeaponBP_Customizable`, candles, stools) and spawners `Weapon_Forge_BP`, `BP_MerchantMenu`, and the
  smithing tool rack.
- **Map_Menu_Startup**: the main menu. **Map_Menu_SplashScreens** is unreferenced by any asset; it is likely set
  only in project settings.
- **Map_Arena_Ambush_Test**: named "Test", but it is used by the `BP_CombatEvent_Forest` ("Forest Ambush") combat
  event and listed by `UI_NextFIght` and `UI_Jester_FreeMode`. It is a **hidden/WIP arena** with 29 spawn points.
  Its foliage sublevel has an empty `PLA_Arena_Ambush_Trees_01` level instance.
- **Arena_Cutting_Map**: a cutting/test range referenced only by `UI_Pause`. It has a NoBrain Willie, a training
  dummy, a spike pit and a board, and streams `Arena_Cutting_Ground_A..G`. Flagged as dev/test.
- **Map_Test_Empty**: the `Level` default of `BP_CombatEvent_Master`. Dev map with armor-head props and two Willies.
- **Abyss_Map_Open_EA**: referenced by the startup menu and the Seer dialogs, but **near-empty**: 2 StaticMeshActors
  plus a level BP. The Abyss content is not in this pak.
- **Entry**: the engine default map; unreferenced.
- The remaining 69 maps are sublevels or level instances; `pulled_in_by` lists their parents. All 88 parse with
  0 errors. Warnings are limited to empty LevelInstances: four in Alley's persistent level labelled
  `Map_Arena_Alley*`, which have no CookedWorldAsset and so load nothing, and one in the Ambush foliage sublevel.

## Limitations

- **Sockets are not resolved.** For an `AttachSocketName` attachment, the transform is relative to the parent
  component's origin. No arena spawn point or world object uses one, but attached child actors do use plain
  component attachment, and that is handled.
- **Construction-script and runtime changes are not evaluated.** Values are as cooked: instance, then archetype,
  then CDO. BP logic that moves, hides or destroys actors at BeginPlay is not reflected in the positions; the
  sections above describe what it does.
- **`simulate_physics` reflects `BodyInstance.bSimulatePhysics`** on cooked components through the template chain.
  Weapons also expose the BP variable `Simulates Physics` as `bp_simulates_physics`, which is what the game
  applies at runtime.
- **Dynamic lighting sublevels** (`LevelStreamingDynamic_*`: Morning, Evening, Night, Dawn and so on) are included,
  with `via` showing the source. The game loads one of them according to the time of day, so objects from them, such
  as the Pit and Yard candles, exist only in that variant. `NarrowPassage_Spikes` sublevels likewise exist only in
  the Narrow Passage combat mode.
- **Classification is by class-name rules plus class chain.** Anything that simulates physics but matches no rule is
  `physics_other`. Light props include every `Candle*` and `Chandelier` class.
- **The bytecode decompiler is a readability aid, not a full decompiler.** Latent and flow-stack control flow is
  shown as `push_flow`/`pop_flow` with offsets.
