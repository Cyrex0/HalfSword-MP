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

## Actual native-return corrective run

Exact clean4842e8bd passed full G0 (74 Lua suites, 1311 Rust tests) and deployed
matching binaries/native modules. `test-results/20261009-005629-ebdcae-native-host`
then ran two normal clients and one NullRHI authority. Native-ready occurred
at9.575s with two opposed humans and no AI, reproducing the bootstrap proof.
The earlier nil iterator errors did not recur. Both clients instead refused
`suppression_destroy_readback` at14.020/13.723s; the event does not yet identify
the first actor. The source's own event file retained the concrete failure
`native render attachment parent not captured`, with zero canonical frames/
samples, three refusals, six neutral dispatches and active input [0,0]. Neither
actor destruction nor complete attachment mirroring is therefore demonstrated.
All four recorded processes were independently absent after cleanup, original
saves remained unchanged, and no crash/unobserved-child record appeared.

Bounded review also found that C++ mirror destruction lacks the borrowed world
guard even though its GetLevel qualification invokes ProcessEvent. A reentrant
map change can make that qualification stale; guarded ABI3 destruction and
discard-only behavior for stale worlds require explicit lifetime verification.
The source attachment representation currently lacks nonmesh scene anchors.
Collect exact native child/parent/owner identities before extending it; never
pretend an uncaptured parent is the actor root or skip closure verification.

Corrective/diagnostic commits: 940d506c adds ABI3 borrowed guards to mirror
destruction and preserves discard-only stale-world handling. Focused native
checks verify zero K2_DestroyActor calls when GetLevel changes the active world,
exactly-once destruction in the original current world, and no actor reads
after destruction (31 C++ checks plus the native Rust API check passed).
Client suppression retains its strict refusal and now records the target's
native name/class, world/protection facts and pre/post lifecycle flags.
4f212ae1 retains strict attachment closure and reports freshly qualified native
child/parent/owner/root identities and exact parent transforms (37 source Lua
checks passed). Actual success still requires another exact three-game run.

Native cancellation facts: the only reflected CancelLatentActions is the
parameterless UUserWidget method. KismetSystemLibrary has no generic actor
cancellation method in the pinned dump. LevelManager and Spawner use native
delays with CallbackTarget=self; disabling tick alone cannot cancel these
continuations or prevent direct spawn function calls. Lua UObject:IsValid
does not supply an actor pending-destruction proof. Never admit a client based
on an invented cancellation call or weakened same-world fighter census.

## Actual qualified-object diagnostic run

Exact clean c53a7f1e passed full G0 and was pushed to dev, then deployed with
matching binaries/native modules. Real three-game evidence is in
`test-results/20261009-011615-85bf8f-native-host`. The authority again reached
native_ready at9.818s with two opposed humans, no AI, but zero samples/frames
and active input [0,0]. The original scene failure is now identified precisely:
Willie's StaticMeshComponent `Aim Spline Scene Sphere` attaches to the actual
nonmesh `Aim Spline Scene`, whose parent is `Aim Spline`; their original pawn
owner, CollisionCylinder root, class identities, socket and exact native
transforms were qualified in the authority's own event file. The existing
mesh-only dictionary could not honestly express this chain.

Both clients refused the exact nonPersistent `BP_LevelManager_C_1` after
K2_DestroyActor returned: native current-world/hidden/tick/collision facts
remained true/true/false/false; Lua IsValid was true, and IsActorBeingDestroyed,
RF_BeginDestroyed and RF_FinishDestroyed were false. These facts neither prove
engine rejection nor exclude pending kill. The pinned Lua method dispatch uses
the correct colon receiver and reaches ProcessEvent; no LevelManager override
or tracked mod hook suppressing destruction was found. Native PendingKill is
an internal object flag distinct from begin/finish destruction. The initial
candidate captured an original weak identity and resolved it afterward; the
matched native analysis below shows why that weak validity alone does not
prove actor-world liveness. Do not dereference an invalidated actor, waive the
test, or claim that changing call language proves removal. All four owned
processes were absent after cleanup, saves unchanged,
and crash/orphan lists empty.

The next candidate adds explicitly typed nonrendering scene anchors and a
guarded native driver-retirement proof. Exact native SceneComponent anchors,
SplineComponent anchors with actual bDrawDebug=false, and currently hidden
CapsuleComponent roots require their own qualified eligibility checks. Unknown
or rendered geometry must still refuse. Copy every real parent/root and apply
parents before children; later parent movement must not corrupt already-applied
world transforms. Native scene/input/playability proof remains outstanding.

Candidate commits 153db525/81a32749 now implement that bounded change. Recipe
schema2 requires exact component classes and scene evidence, explicit
not-applicable geometry/vertex state for anchors, and explicit null collision
only for a proven nonprimitive SceneComponent. The complete actual ancestor
graph is cycle/bound/owner/world checked. C++ source capture rechecks eligible
anchor classes and visibility/debug facts, creates inert plain scene anchors,
and rejects missing parents before applying transforms in parent-first order.

Presentation ABI4 adds a guarded driver-retirement candidate using the
originally live weak identity and GUObjectItem validity. Foreign, protected,
changed-world and reused identities refuse. Its weak-invalidity-only success
predicate is insufficient for actor-world retirement, as established below.
Scalar proof records are probed
before any later engine call on a pending-garbage Lua wrapper; same-world
reconnect retains these proofs and actual world drop clears them. Native
invalidity does not mean UObject memory has been deallocated. Gear/AI removal
keeps its prior strict path until corresponding native evidence justifies a
qualified extension. Focused checks passed: source Lua46, DTO8/parser4;
native C++82, two native Rust filters, client/suppression Lua82. None substitutes
for the next exact three-game scene and control proof.

## Actual scene-anchor and ABI4 retirement run

Exact clean d45b74c2 passed full G0 (74 Lua suites, 1314 Rust tests and 79
binaries) and deployed matching native modules. The requested topology ran in
`test-results/20261009-014908-007ad9-native-host`: two normal AI-driven clients,
one NullRHI game authority, plus its non-game supervisor. The authority again
proved two native human pawns with opposed Team Int1/2 and no native AI. It
produced zero canonical samples/frames, 115 source capture refusals and active
input [0,0]; the 230 dispatches were neutral. The concrete source refusal was
`native returned array key`. Diagnostic commit 8a39317f records the exact
getter, owner/component, key and value types without relaxing array acceptance.
That diagnostic has not yet run in the game.

Both clients refused retirement of the same qualified original
`BP_LevelManager_C_1`. Native dispatch was recorded, but its original weak
handle still resolved (`alive_after=1`). Authority/role observations were
HasAuthority=true, LocalRole3 and RemoteRole0. This observation does not prove
that DestroyActor failed. Neither client reached isolation, canonical scene
acceptance or active input. All four recorded process identities (38460,
19380, 27756 and 23528) were independently absent after cleanup; original
saves remained unchanged, with no new crash or unobserved-child records.

## Matched native retirement correction

The existing matched evidence in
`combat-evidence-20261005/actor-retirement-proof.md` requires positive original
actor membership in the exact current world before removal, followed by
successful same-world/class enumeration absence AND either original-object
global absence or its fresh native RF_MirroredGarbage flag. World absence
alone can reflect inactive-level filtering; a valid Lua wrapper or resolving
weak handle proves neither retirement success nor failure.

Read-only LLVM inspection confirmed the installed game executable still
matches SHA256
`367dfccf1aaca3bbf6824c9bb616f2f31bc30e7ac70c5fa8657e212dbb2c2e03`.
Its KismetSystemLibrary::IsValid at RVA 0x3708ce0 checks nonnull and native
object flag bit30 clear. The matched GetAllActorsOfClass iterator rejects
RF_MirroredGarbage (0x40000000), inactive levels and foreign worlds. The
deployed pinned UE4SS.dll, SHA256
`680a026890abb4d0df2211251f8defc1681a584275f1521dcc0fe30af480006f`,
has FUObjectItem::IsValid(false) at RVA 0x439fe0 checking internal bits28/29.
Its UE5.4 GetFlagsInternal path reads raw internal flags without mapping the
game's mirrored-garbage flag. General weak lifetime guards therefore also
must not be described as proof of native actor-world activity.

The next correction preserves original identity, current-world, role and
protection guards and uses the matched compound retirement predicate. It
must not call actor ProcessEvent or GetWorld after destruction, force garbage
collection, waive missing evidence, or assume deallocation. Native scene
replication and controls still require a fresh exact three-game proof.

The ABI5 candidate replaces the weak-invalidity-only predicate with that
compound proof. Its 400-byte result records separate original weak presence
and engine retirement, plus before/after lifecycle facts with explicit known
masks. Native same-world/class enumeration owns and frees its returned array;
post-enumeration identity and garbage state are freshly checked. The global
absence path checks the actual native object-array slot; a weak-invalid but
still globally present object refuses, rather than masquerading as absence.
Retained
garbage actors receive no post-dispatch actor ProcessEvent. Missing initial
world membership, ordinary world omission, unavailable APIs, identity reuse
and reentrant travel all refuse. Scalar proofs remain bound to their original
world and are forgotten on world drop.

Focused candidate checks passed: strict native C++ compile plus 165
lifetime/retirement/scene checks, two native Rust binding checks and one API
check, Lua83 checks and two-file syntax. These
results establish the guarded bridge behavior only; actual game acceptance
remains outstanding and must use two normal clients and one headless game.

## Actual ABI5 three-game run

Exact clean cfe9afe4 passed full G0 (74 Lua suites, 1314 Rust tests in 79
binaries; clippy zero errors/six warnings) and build/deploy exited zero. The
deployment's commit, binary commit and nested G0 commit matched; source was
clean and native modules deployed. The default actual topology ran in
`test-results/20261009-022009-ea57ee-native-host`, with two normal AI-input
clients, one NullRHI engine authority and its supervisor. Source again
reached native_ready with two opposed human pawns and no AI.

Both clients advanced beyond the old LevelManager retirement refusal. Each
suppression summary recorded two retired native drivers, then refused
`suppression_component_census_empty` at 14.456/14.494s before any fighter,
gear or AI retirement completed. The failing component owner is not yet
identified in that summary. No successful client isolation or active input
has been demonstrated. Driver retirement advanced in the real engine; this
does not prove complete local combat suppression.

Successful retirement's raw ABI5 before/after fields were cleared before
logging, so this run persists only the strict provider-gated `drivers=2`
summary. Periodic retirement probes were not reached. The next diagnostic
must retain bounded original-driver identity and before/after proof fields,
as well as the exact owner of the empty component census.

The source's new diagnostic identifies its refusal exactly:
`Actor.K2_GetComponentsByClass(SceneComponent) collect owner=Willie_BP_C_2147482026`,
numeric key257, userdata value, max256. This is a real native component-count
bound, not an undocumented array annotation. Do not silently raise limits or
skip native clothing/gear; capture the exact census and required ancestor
closure before changing the representation or resource budgets.

Authority-owned evidence remained zero samples/frames, 116 refusals and
active input [0,0]; the 232 dispatches were neutral. All four owned process
identities (supervisor24916, source44720, clients4324/42052) were independently
absent after cleanup. Original save hashes remained unchanged, with no new
crash or unobserved-child records. The tracked tree stayed at exact clean
cfe9afe4 throughout the run. Native playability remains unverified.
