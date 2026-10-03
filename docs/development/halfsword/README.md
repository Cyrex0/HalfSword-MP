# Half Sword modding notes

What the HSMP project learned about Half Sword's internals while building the mod, in our own
words. The game is a UE 5.4.4 shipping build (changelist 2705, the developers' own engine branch);
everything here was found with UE4SS dumps, decompiled Blueprint bytecode and in-game tests. It
can change with any game patch: the launcher pins the supported game builds by exe hash
(`tools/release/release.json` `game.builds`) for that reason.

How to apply these findings in mod code, with examples: [../lua-mods.md](../lua-mods.md) §5.

## Files in this folder

| File | What |
|---|---|
| [sdk-dump-notes.md](sdk-dump-notes.md) | Engine identification, the UE4SS build that works, and how the SDK, header, object and USMAP dumps were produced |
| [pak-unlocked.md](pak-unlocked.md) | How the pak's AES key was found and how to extract and repack `pakchunk0-Windows.pak`. The Half Sword developers share this key with modders, and the project decided to publish these notes |
| [pak-bp-patch-guide.md](pak-bp-patch-guide.md) | An optional, unused path: patching `UI_Startup_Menu.uasset` in the pak instead of hooking it at runtime |
| [io-dispatcher-crash.md](io-dispatcher-crash.md) | The engine crash on the `IoDispatcher` thread during an arena load: symptom, root cause, the `r.HairStrands.Streaming=0` workaround and how to symbolise a new dump |
| [willie_props.txt](willie_props.txt) | A property dump of a live `Willie_BP_C` (the character class): every reflected property name, including the ones with spaces and GUID suffixes |

## Blueprint names have spaces

Half Sword's Blueprint functions and many Blueprint properties are named with spaces:
`"Set Up Armor"`, `"Wake Up"`, `"Get Damage"`, `"Spawn Combatants"`,
`"Setup Character Event"`, `"Character Passport"`, `"Spawn in Pants"`. From UE4SS Lua a squashed
name such as `pawn:SetUpArmor()` resolves to nothing and **fails silently**; the call must be
`pawn["Set Up Armor"](pawn, ...)`. The real name is the text after the last `:` in the object
dump's `Function /Game/...:<Name>` line. Engine (`/Script/...`) functions have no spaces.

Struct fields of Blueprint structs carry GUID suffixes, for example
`HeadHealth_2_61859BB444171EF8952E0FA5DD8628EE` in the body-condition struct or
`WeaponinHands_23_B3FE...` in the equipment struct. They are stable within one game build and may
change in another, which is one more reason the game build is pinned.

`hsmp-tools check-bp-names` checks every mod against the space-named members listed in the dump.

## Characters: Willies

The player and every fighter are `Willie_BP_C` actors.

### Armour and weapons come from the Character Passport

Decompiling Willie_BP's `"Set Up Armor"(Clear Previous, No Check Block)` showed:

- It reads **only** the pawn's own `"Character Passport"` → `Equipment` → `ArmorinSlots` map
  (slot byte → armour passport). It writes what it accepted to `"Currently Equipped Armor"`,
  which is an output that is cleared on every call. `ArmorSlots`, `ExternalArmorSlots` and
  `NewArmorSlots` are never read by it.
- A piece spawns only if `(slot == 16 or not "Spawn in Pants")` and
  `(slot in 12/15/16 or not "Blossfechten Gear")`. The arena spawns pawns with
  `"Spawn in Pants" = true`, which is why a naively dressed pawn ends up in trousers only. The
  flags are per pawn and never touch the save; HSMPLoadout clears them right before it calls
  `"Set Up Armor"`.
- `"Spawn in Pants"` also gates re-arming the hands from the passport and a helmet-knock rule in
  the ubergraph and in `"Get Damage"`.
- About 0.2 s after a setup, the ubergraph **re-arms the hands** from
  `Equipment.WeaponinHands` (key 0 = right hand, 1 = left). Keep that map in sync with the kit,
  or the right hand turns back into fists.
- The tier loadout data assets (`DA_Equipment_Loadout_Tier_*`) only hold base clothing passports.

### Willies are pooled

The game pools its Willies. `K2_DestroyActor` on a Willie is a silent no-op that snaps it to the
world origin, and `Willie_BP_C_0` (tag `Persistent`, invisible mesh) sits at the origin in every
arena. To take a Willie out of play, hide it and disable its collision, tick and physics, and
destroy its AI controller and weapons. HSMP's census ignores hidden Willies, Willies with an
invisible mesh and the `Persistent` one.

HSMP shows each remote player by **puppeting** one of the arena's natively spawned foe Willies
(detaching its AI controller) rather than spawning a new `Willie_BP_C`: raw spawns skip the native
init chain and come out naked and T-posed. The Director sets the game's "Free Mode Foes Amount"
to the number of remote fighters (at least one), so the arena spawns enough foes to puppet. When
there are still too few, HSMPAvatars asks `BP_LevelManager`'s "Spawn Combatants" for more.

`SK_Skeleton` is an exact copy of the visible, simulated `Mesh`. Calling
`SetLeaderPoseComponent` on it from Lua crashed the game.

### Career wounds and the CDO heal

The career save keeps per-limb wounds in `Str_Character_Body_Condition` (head, neck, arms, upper
and lower body, legs), and the game applies them to the pawn on spawn; in the career, the Tavern
innkeeper heals them. HSMP never visits the Tavern, so without a fix players spawned in MP with
limbs at about 25 after earlier deaths. Within a match, the same struct on the game instance
(`"Player Body Condition"`, handed to the next pawn as `"Start Body Condition"`) carried one
round's wounds into the next.

The fix, in MP sessions only: copy the `Health`, every `* Health` and the `Consciousness` defaults
from the Willie_BP class default object
(`/Game/Character/Blueprints/Willie_BP.Default__Willie_BP_C`) onto the pawn at possession and at
each round start, and heal the game instance's body condition to the same values before every
travel (the Director's vitals step, HSMPCombat's `restore_vitals`).

**Never call the game instance's `"Reset Player Character"`** for this: it very likely wipes the
career equipment, and HSMP must never change the career.

## The main menu

The real main menu is `UI_Startup_Menu_C` (package `/Game/UI/UI_Startup_Menu`); the
`UI_StartUpScreen_*` widgets are only the splash screens shown before it. Its root is a
`CanvasPanel` (`CanvasPanel_5`) with the background images, a version footer (`TextBlock_7`) and
the ribbon buttons as direct children:

| Widget | Ribbon |
|---|---|
| `Button_0` | Play Progression |
| `Button_4` | Play Free Mode |
| `Button` | Play Abyss |
| `Button_1` | Settings |
| `Button_2` | Credits |
| `Button_3` | Quit |

Each button's canvas slot uses centred anchors (negative X positions) and is about 198 × 80. The
ribbon textures are under `HalfswordUE5/Content/UI/Images/StartUp_Buttons/`.

**The Button_3 hijack (history).** The first menu integration renamed the Quit ribbon to
"Multiplayer" and tried to take over its click. Its `OnClicked` handler is the Blueprint
function
`/Game/UI/UI_Startup_Menu.UI_Startup_Menu_C:BndEvt__UI_Startup_Menu_Button_3_K2Node_ComponentBoundEvent_9_OnButtonClickedEvent__DelegateSignature`
(the `_9_` index is generated by the editor and may change between game builds). It did not work:

- The Blueprint handler calls `KismetSystemLibrary::QuitGame`. A `RegisterHook` callback, also one
  on `QuitGame`, runs after the function body, so by the time the mod opened its lobby the engine
  was already shutting down. A hook cannot cancel a Blueprint function.
- `Button.OnClicked:Bind(...)` returned `false`: the UE4SS build tested does not bind a Lua closure to a
  UMG multicast delegate.
- `UClass:ForEachFunction` was not exposed to Lua either, so finding the handler by name
  means walking the class's `Children` list by hand.

Renaming the ribbon's `TextBlock` and resizing its `CanvasPanelSlot` from Lua did work. The
pak-patch alternative is described in [pak-bp-patch-guide.md](pak-bp-patch-guide.md).

**What HSMPMenu does today** (`mods/HSMPMenu/Scripts/main.lua`): when a `UI_Startup_Menu_C` is in
the viewport, it injects its own ribbons as a second column on the same canvas, cloning a native
button's style, and edge-polls `IsPressed()` for clicks. Opening an MP screen hides the native
buttons and its own ribbons and builds the screen on the same canvas; BACK removes it and shows
them again. Nothing native is cancelled or rewired.

## Other things worth knowing

- **Viewport size:** `GameViewportClient:GetViewportSize()` silently fails from Lua; use
  `WidgetLayoutLibrary` size and scale (`mods/shared/hsmp_ui_scale.lua`).
- **Damage:** `"Get Damage"` is the entry point for every hit, and damage lands in many fields
  (per-region health, consciousness, bleeding, pain), not only `Health`: a head hit can kill
  through `HeadHealth` or `Consciousness` while `Health` barely moves
  ([../subsystems/combat.md](../subsystems/combat.md), [../subsystems/vitals.md](../subsystems/vitals.md)).
- **Idle cutscenes:** the game plays cutscenes and videos when it thinks the player is idle, and
  an unfocused second window always looks idle (HSMPNoCutscene).
- **Hair streaming crash:** an engine crash on the `IoDispatcher` thread while an arena loads is
  worked around with `r.HairStrands.Streaming=0`, applied by the launcher and by HSMPMatch
  ([io-dispatcher-crash.md](io-dispatcher-crash.md)).
- **Bytecode:** Blueprint logic can be read without running the game: extract the asset from the
  pak (see [pak-unlocked.md](pak-unlocked.md)), convert it with
  [kismet-analyzer](https://github.com/trumank/kismet-analyzer)
  (`kismet-analyzer to-json <asset> --ue-version VER_UE5_4 -m <usmap>`), or use `tools/mapdump`
  (`MapDump --probe`) and `hsmp-tools kismet-pp` for readable pseudo-code.

## Map data: `docs/arena_static`

[`docs/arena_static/`](../../arena_static/) holds data extracted **statically** from the game's
cooked maps: no game launch and no UE4SS. Its own [README](../../arena_static/README.md) has the
JSON schema. In short:

| Path | What |
|---|---|
| `Map_Arena_<Name>.json` | one file per MP arena (Alley, Pit, Yard, Slums, Cellar, LordsHall, EastTower), including everything it streams: spawn points, world objects, runtime spawners |
| `all_maps/*.json` | every other map, processed standalone (hub, menus, test maps, Abyss, sublevels, level instances) |
| `_index.json`, `_summary.md` | all maps with their kind, counts and who references them; the generated summary tables |
| `_combat_events.json` | every `BP_CombatEvent_*` definition |
| `bytecode/*.txt` | decompiled pseudo-code of the Blueprints that drive spawning |

**Regenerating it.** `tools/mapdump` is a small C# program on
[CUE4Parse](https://github.com/FabianFG/CUE4Parse) that reads `pakchunk0-Windows.pak` read-only.
`tools/mapdump/run.ps1` installs a user-local .NET 10 SDK if needed, builds and runs it, and then
runs `hsmp-tools mapdump-summary`. It reads its inputs from the environment: `HSMP_GAME_DIR` (the
game install, default `<repo>\game`), `HSMP_OODLE_DLL` (your own copy of `oo2core_9_win64.dll`,
never committed) and optionally `HSMP_PAK_AES`.

```powershell
powershell -ExecutionPolicy Bypass -File tools\mapdump\run.ps1 -ArenasOnly
```

**What uses it.** `hsmp-tools gen-map-data` turns `docs/arena_static` into the two generated
copies of the MP map data: `server/data/maps/<Map>.json` (compiled into the server with
`include_str!`, used for spawn plans) and `mods/shared/hsmp_arenas.lua` (the Lua copy). A cargo
test keeps the two equal, and `hsmp-tools gen-map-data --check` fails when a committed output
differs from a fresh run. Never edit the outputs by hand
([../subsystems/spawns.md](../subsystems/spawns.md)).

## Prior art and tools

- **massclown's Half Sword mods**, the best existing reference for class names and how to hook
  the game:
  [HalfSwordTrainerMod](https://github.com/massclown/HalfSwordTrainerMod) (UE4SS Lua; reads and
  writes player state),
  [HalfSwordSplitScreenMod](https://github.com/massclown/HalfSwordSplitScreenMod) (a second local
  player through UE4SS; the closest earlier proof of multi-player state in Half Sword) and
  [HalfSwordModInstaller](https://github.com/massclown/HalfSwordModInstaller) (the install layout
  mods are expected to use). The Trainer's note that the game is UE 5.1 is out of date.
- [Nexus Mods: Half Sword](https://www.nexusmods.com/halfsword), the community mod site.
- [RE-UE4SS](https://github.com/UE4SS-RE/RE-UE4SS), the scripting system HSMP runs on (which build:
  [../ue4ss.md](../ue4ss.md)).
- [ZenTools](https://github.com/Archengius/ZenTools), a Zen-container pak extractor, the fallback
  when standard pak tools fail.
- [ue4ss-ModMenu](https://github.com/mattdavida/ue4ss-ModMenu), a reference for runtime UMG from
  UE4SS Lua (construction, edge-polled input).
