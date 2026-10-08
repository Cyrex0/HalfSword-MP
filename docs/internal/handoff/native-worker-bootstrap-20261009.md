# Native authority bootstrap checkpoint

The exact gated build `6b49c29670c77d49e4b4f7102f35d8e10b8b9f3d` ran one NullRHI
authority and two normal native clients. Raw evidence is retained in ignored
`test-results/20261008-235227-91548c-native-host`.

Both clients travelled, then refused local combat isolation at about 13–14 seconds.
Their own JSONL records the refusal but lacks the actual GameMode class/condition,
so that cause remains unproved. The harness stopped the source at about 15 seconds;
the 45-second native spawn timeout was not reached. The source census observed
native Local Multiplayer=true, valid PC0 and PC1, PC0's Willie/team 1, no PC1 pawn,
LevelManager Amount of Characters to Spawn=2, and exactly one Willie/no AI. No
canonical frame, mirror readiness, native input dispatch, gear, cut, or combat
parity was proved. All four recorded processes stopped, player save hashes were
unchanged, and no new crash report appeared.

The old diagnostic `condition and value or "unavailable"` lost actual false
values. The corrected converter preserves false, true and zero, and copies the
bounded scalar census into the authority's own event file. Old `unavailable`
values must not be retrospectively interpreted as false.

## Primary native source

- Pinned `game/HalfswordUE5/Binaries/Win64/ue4ss/CXXHeaderDump/Enum_GameMode_enums.hpp`:
  Arena (`NewEnumerator1`) is 1; Tavern (`NewEnumerator0`) is 0.
- Pinned sibling `Enum_PlayMode_enums.hpp`: Free Mode (`NewEnumerator2`) is 1;
  Progression (`NewEnumerator0`) is 0. Enumerator suffixes are not numeric values.
- `docs/arena_static/Map_Arena_Yard.json` effective spawn points: C_0 accepts Tavern;
  C_1/C_3–C_11 accept Arena, and C_2 accepts Sparring. Some additional spawn points
  require six combatants. Missing cooked properties do not establish native defaults.
- `docs/arena_static/bytecode/BP_LevelManager.txt`, Clean Up and Spawn Actors
  offsets 35–148: Free Mode Activated changes Current Combat Mode to 1 when
  Free Mode Carnage=false, or 3 when true. The observed boot 0→native 1 change is
  expected native behavior.
- Spawn Combatants offsets 434–498 compute Free Mode Foes Amount+1 for native
  spawner compatibility. Offsets 656–1169 check actual game/combat/play bool maps
  and Required Combatants Amount. Offsets 1414–1456 add the player to Amount of
  Characters to Spawn. The loop uses Amount>Spawned at 1783, increments Spawned
  at 1831/1873, and calls Spawn Willies(1) at 2191/8174/8459. There is no loop
  LastIndex subtraction proving that PvP needs two foes. PvP retains one foe;
  explicit diagnostic retains two foes.
- Clean Up Map offsets 801–922 select FreeMode Multiplayer and set native Local
  Multiplayer. Offsets 1217–1253 check HasMultipleLocalPlayers and call
  CreatePlayer(self,-1,true) when needed. The false branch at 1281–1436 can remove
  PC1. The real census proves PC1 creation succeeded in this run.
- `docs/arena_static/bytecode/BP_SpawnerPoint_Willies.txt` offsets 3498–4341 select
  the real Free Mode Player Character 2 passport for a nonplayer spawn while P2
  is not spawned. Offsets 11118–11405 require the selected multiplayer flag,
  !SpawnPlayer, !P2Spawned, valid PC1, then native Possess and P2Spawned=true.
  Team Int is a native int property assigned at 7477, never a guessed team value.

The corrected profile selects Arena=1 and Free Mode=1 and retains the native
FreeMode Multiplayer creation/possession path. The next run must prove whether
that correction restores the second pawn. A bounded, world-qualified native
spawner census records actual map compatibility and current Spawn Player/
Spawn Mercenary flags; it does not construct a fallback pawn or change enemy AI.
Exactly two normal clients and one headless source remain the acceptance topology.
# Actual corrected-profile run and next candidate

`test-results/20261009-001359-af698c-native-host` ran exact clean deployed
5a31d1bf with two normal native clients and one NullRHI game authority. The
authority reached native_ready at9.681s with two native human controllers/pawns,
zero AI and actual opposing Team Int1/2. Arena1/Free1 therefore restored the
missing second fighter; Foes1 remains the proven native two-fighter quantity.

Both clients independently confirmed `/Script/Engine.GameModeBase`, but each
still had one visible, collision-enabled, simulating nonPersistent Willie
possessed by PC0. Isolation correctly refused and source frame_seq stayed0.
Source descriptor capture also refused the unavailable `GetComponentsByClass`
alias; pinned reflection exposes `K2_GetComponentsByClass`. No live input or
complete render frame was demonstrated. All four owned process records (three
games plus supervisor) stopped, original saves were unchanged, and crash and
unobserved-child lists were empty.

Narrow source getter correction dadd38ae uses the actual reflected getter and
tests refusal of the plain alias. Client candidate e78d13cf suppresses the
current-world native arena callback drivers and retires local nonPersistent
fighters/owned gear/AI with fresh inert readback. Protected Persistent actors
remain untouched and must pass their own inert proof. Pooled inactive fighters
require the original scalar retirement identity and complete fresh checks;
ordinary hidden actors cannot qualify. The new behavior still needs a fresh
exact G0 deployment and three-game run. Do not claim playable mirroring yet.

## Actual suppression-candidate run

Exact clean gated/deployed f826b657 ran the requested two normal game clients
and one NullRHI authority in `test-results/20261009-004058-646bf3-native-host`.
The authority reached native_ready at9.885s with two opposed native human
fighters, Team Int1/2 and no native AI. Both clients stopped at14.727/14.556s
because native_client.lua:38 invoked `ForEach` on a native component getter
result that was an ordinary Lua table. The source independently refused capture at
native_source_render.lua:33: the actual K2_GetComponentsByClass return also
had no `GetArrayNum` method. These are binding representation errors, not
evidence that suppression or complete rendering passed.

Authority-owned JSONL records zero canonical samples/frames, 156 refusals and
active native input dispatch [0,0]. The 312 total dispatches were neutral
control frames and do not demonstrate AI input. All four owned process records
were independently absent after cleanup, original player save hashes remained
unchanged, and crash/unobserved-child lists were empty. Shared UE4SS.log must
not contribute to active-input pass criteria; use the authority's own event
file for attribution. The next candidate corrects function-returned Lua tables
separately from reflected property TArray wrappers, using the pinned UE4SS
binding implementation and realistic fixtures.

Corrective candidates: 0e73b15d uses the actual function-return representation
for client component retirement and input mappings; ee51b5ba does so for source
components, morphs and vertex colors and reads the actual component physics
override or qualified skeletal asset physics getter. Focused fixtures now use
the pinned return representation and trap the unavailable component getter.
3844ee71 removes shared-log input attribution and retains the latest concrete
source refusal in authority-owned native_evidence. These fixes require a new
exact deployment and actual three-game proof; their offline checks do not
establish live presentation or controls.
