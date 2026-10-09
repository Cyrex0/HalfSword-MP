# Native authority bootstrap checkpoint

## Current state: 2026-10-09, latest actual build55b2ed06

The actual topology launches: one NullRHI game authority and two rendered native
clients. The authority qualifies two human fighters on opposing native teams;
both clients now complete local combat suppression. Original attachment closure
also completes (47 nodes:26 pawn meshes,13 axe meshes plus native parents).
The production helper wiring and the client256-component resource assumption are
fixed. No canonical scene frame or active client input has been proved yet.

Last live publication blocker: native Aim Spline has actual bDrawDebug=true,
visible=true, hidden=false and owner_hidden=false. Existing scene-anchor support
refuses it. Shipping non-rendering has not been proved, and faithful native spline
curve/settings/dynamic-state replication is being implemented as ABI7, source
schema3 and capability-negotiated render V2. This coupled change has not run in
the game yet. Do not bypass this refusal or infer pixels from visibility alone.

Current performance blocker: first capture still spends roughly16 seconds before
that refusal. Scope-local schema caching passed focused checks but the real run
showed only small timing changes. Repeated Lua indexed-controller/current-world
resolution now avoids duplicate qualification (six indexed controller queries
become three per guarded source read); focused worker checks pass, but actual
runtime improvement remains unmeasured. All source lifetime/world/player checks
remain required.

Next acceptance steps are (1) evidenced spline support, (2) complete recipes and
advancing source/mirror frames for both clients, (3) positive legal native inputs
for both owned controllers. Native combat/body/gear parity, camera/HUD ownership,
mode/end-state handling and co-op Abyss remain subsequent open gates. The public
existing PvP beta.6 release is separate and already published; the native headless
branch has not passed playable or release acceptance.

The remainder preserves the historical evidence in order; its older “next run”
instructions are superseded by this current state and the final recorded run.

## Pending spline integration: focused evidence

Source schema3 changes are committed as2a5905bb; the worker requires and forwards
all five native scope APIs as80f23cac. Source Lua133 assertions and syntax3 pass;
worker216 assertions and syntax checks pass. Native Rust presentation5,
source scope16, table parser5 and API1 checks pass. C++226 checks cover bounded
raw curve copying, allocation cleanup and callback lifetime admission, without
claiming rendered parity. Independent Sol6.1 review found and closed invalid
pointer arithmetic across separate float members and late garbage checks before
spline callbacks; no further concrete review blocker was found.

The combined server native filter passes45 tests, including complete profile
readiness, raw nonunit quaternion/zero tangent and signed-zero transmission,
all frame truncations, invalid flags/counts/values, unchanged64KiB message bounds
and authenticated UDP scene delivery. The UDP test initially caught missing
0x0A12 routing; adding the route made the same test pass. No full G0, deployed
ABI7 build or native game run is claimed by these focused checks.

The observed menu-to-exit behaviour is the bounded harness ending initialization
after no canonical scene becomes available. Last live builds recorded clean
supervisor shutdown and no orphan/crash process. The spline integration still
needs actual source capture, two mirror readbacks, advancing frames and positive
owned input before the native branch can be considered playable.

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

## Mesh-seeded closure and client-census diagnostic candidate

Pinned Willie CXX metadata declares 233 SphereComponent references, and the
cooked Willie template has 384 component exports (including 242 spheres,
50 constraints, 42 scenes, nine skeletal and eight static meshes). These are
template counts, not observed live counts. Together with the actual source
return at key257, they establish that a full SceneComponent census capped at
256 is inappropriate for this actor; raising that cap would also worsen its
repeated per-getter allocation cost.

The replacement enumerates every native MeshComponent for each original pawn
and owned weapon, then includes every actual hard owner root and follows each
actual GetAttachParent link. Scalar witnesses replay the fresh mesh/root seed
and complete native parent path before/after reads. Complete mesh-set,
address/FName, original owner/world, root, parent, cycle and supported-anchor
checks remain strict. No native mesh is filtered by visibility or gear type,
and the final64-component and wire bounds remain unchanged. Full physics-helper
censuses are unnecessary for this render graph; their native combat behavior
continues to belong to the engine authority. This is a representation/capture
correction, not evidence of armour or body parity.

Focused source checks: Lua66 and two syntax files passed, including 300 nonmesh
helpers with complete mesh/root/parent closure and refusal of any full Scene
enumeration. Native mesh addition/removal, identities, roots, links, owners,
world changes, unsupported render kinds and graph bounds remain covered.

The client candidate retains bounded successful driver retirement/probe
identities and raw before/after proofs. Inert entry records its exact target,
native mirrored-garbage flag, original world/protection, hard RootComponent
and optional Blueprint DefaultSceneRoot facts, exact component getter/count
and actor inert readbacks. Empty census still refuses. Unknown/garbage flags
stop new actor calls/root reads; earlier cohort GetWorld checks remain an
explicit limitation. Focused client Lua97 checks and two syntax files passed.
Neither candidate has yet passed a fresh actual three-game run.

## Actual mesh-closure revision run and bootstrap regression

Exact clean ec06613e passed full G0 (74 Lua suites, 1314 Rust tests/79
binaries, clippy zero errors/six warnings), pushed to dev and deployed with
matching nested G0/native/binary stamps. The default actual three-game run is
`test-results/20261009-023819-95f629-native-host`. The authority never reached
native_ready: both controllers existed but neither had a pawn; total Willie
was zero and compatible spawners were zero of12. Its current native Game Mode
had changed to Hell5 instead of selected Arena1. Mesh-seeded descriptor
capture was never exercised, so this run proves no capture regression/fix.

The retained client proofs now directly establish the first two driver
retirements. Each original LevelManager_C_1 and Spawner_C_0 changed from exact
world membership=true and native flags2621448 to membership=false and
flags1076363272 (including RF_MirroredGarbage), while its weak handle remained
present. Native actor-destroying=true and root-live=false were also observed.
This is actual matched engine retirement evidence, not inferred weak death.

The exact next refusal is driver `BP_SpawnerPoint_Willies_C_1`. Its hard
RootComponent and Blueprint DefaultSceneRoot reference the same nonnull
native SceneComponent with garbage=false. Actor garbage was false before/
after and its typed ActorComponent getter completed with an empty table.
Current-world and nonPersistent facts were true; hidden readback was false,
collision/tick false. Native world-class membership remains unobserved for
this actor. Do not infer garbage, pooling, inactive level or successful
suppression from those facts; the native actor iterator filters inactive
levels whereas global FindAllOf does not establish active membership.

Active input remained [0,0], with no accepted frames. All four owned process
identities (supervisor11348, authority21596, clients28628/42792) were
independently absent after cleanup, original saves unchanged, and crash/
unobserved-child lists empty. The tracked tree stayed exact clean ec06613e.

Own authority events explain the mode regression: selected profile was
verified/travel dispatched at6.899/7.104s, followed by unredirected career
GameProgress loads at7.458/7.596s. Native GameManager BeginPlay invokes GI
Load Game (bytecode2211); GI Load Game copies saved CurrentGameMode into GI
(statement2528). The worker omitted Director's established
SG.seed_session_slot step, allowing the later native load to overwrite the
profile. The next bootstrap correction must seed the diverted native profile
after verification and before travel, then requalify original world/profile
after the native call. Never delete saves, fabricate spawns or patch loaded
mode fields repeatedly to conceal that initialization order.

Bootstrap candidate 2452d700 adds the required protected session seed after
verified profile and before travel. Original world/profile are requalified
around native seeding and held travel; seed/reflection/world/profile failures
refuse without fallback spawning or field reapplication. Focused worker125
and Director335 checks plus two syntax files passed. The in-memory saveguard
fixture reproduces original career Hell5 and both native BeginPlay loads:
they read the diverted Arena profile while the original remains unchanged.
That is fixture evidence; another actual three-game run must prove the fix.
Standalone travel lint reported the pre-existing legacy_travel.lua allowance;
full G0 must validate its configured legacy exception before deployment.

The next client correction adds a guarded read-only native actor scope and
uses scope then strict native retirement for the exact allowed spawn drivers.
Native world/class membership is freshly enumerated and original raw-slot,
address/name/class and garbage state requalified afterward. Unknown, absent
or garbage scope refuses before new actor ProcessEvent. The scope's original
weak identity must still match immediately before removal. Full native
retirement remains the required final proof; a generic component inert census
is unnecessary for a driver that is demonstrably removed from the engine
world. This does not accept an empty fighter/gear/AI physics census or broaden
removal classes. Client scope and final driver proofs are retained for the
actual next run. Native acceptance remains pending.

The coherent ABI6 candidate's focused native checks now passed: strict C++
provider compile and 196 lifetime/scope/retirement checks, two Rust binding
checks and one native API check; client Lua101 and three syntax files passed.
ActorScope is 304 bytes and explicitly distinguishes verified native world
membership from retirement. Scope absence/garbage refuses without new actor
ProcessEvent, and retirement accepts only the original scope weak/address;
wrong-generation inputs dispatch zero K2 calls. Full G0/deployment and actual
source readiness, capture, isolation and active input remain pending.

Read-only NullRHI preparation audit found the worker already sets verified
VisibilityBasedAnimTickOption0 (AlwaysTickPoseAndRefreshBones), rate skipping
false and ticking enabled for four named source mesh fields (Mesh,
SK_Skeleton, BoneCore, DriverSkeleton). It does not yet read back the tick
getter or measure complete live skeletal/armour component progression. This
establishes configuration code, not actual NullRHI animation/physics parity.
After canonical frames work, measure the complete current skeletal census,
actual policy/tick state, LastPoseTickFrame and coherent bone/physics changes
under legal input. A NullRHI client host does not establish UE dedicated-server
netmode from its label.

## Actual protected-seed and ABI6 run

Exact clean c0b37e11 passed full G0 (74 Lua suites, 1314 Rust tests/79
binaries; clippy zero errors/six warnings), pushed to dev and deployed with
matching clean G0/native/binary stamps. Actual two normal AI-input clients
plus one NullRHI authority ran in
`test-results/20261009-030024-b1ff11-native-host`.

Authority-owned evidence records the diverted GameProgress seed write at
6.806s before travel at6.807s. All subsequent arena GameProgress loads were
redirected to that protected session slot. Qualified two-human native_ready
occurred at9.600s. No direct post-load GI Game Mode getter was logged; these
are actual initialization/readiness facts, not an invented mode readback.

Both clients' fresh ABI6 scopes now identify Spawner_C_1 as world-listed=false,
RF_MirroredGarbage=true (flags0x40280008), native actor-destroying=true and
root-live=false. Original memory/class/name qualification succeeds. Strict
`suppression_driver_native_garbage` refusals occur at14.515/14.903s. This is
an already-engine-retired global wrapper; it is not a new driver requiring a
component census or own-mod destruction. The next live driver census must
classify only fresh qualified world-absent AND garbage wrappers separately,
without calling actor functions or manufacturing own pre-removal/probe proof.
Unknown, contradictory or nongarbage absent evidence remains refused.

The authority's own JSONL ends immediately after native_ready (seq47), with
no capture/refusal/frame/input or stopped event. Supervisor stderr records
that its graceful shutdown deadline was missed and its verified owned
process was stopped. Cleanup therefore succeeded by owned-process fallback,
not graceful source shutdown. An unidentified synchronous post-ready
operation may have blocked; no own phase/stack evidence identifies it yet.
Do not assert a particular deadlock, renderer dependency or capture failure.
Narrow rare capture-stage entry/exit diagnostics are required before another
game run, with the expensive native caller probe still off.

No accepted canonical frame or active input was demonstrated: active [0,0].
All four owned process identities (supervisor26376, authority6832,
clients44812/18556) were independently absent after cleanup. Original saves
remained unchanged, with no new crash or unobserved-child records; tracked
source stayed exact clean c0b37e11.

The follow-up client correction 94ce250c scopes each exact driver before
wrapper GetWorld, protection or actor functions. Fresh verified world absence
AND native RF_MirroredGarbage is logged separately in external_drivers and
skipped; it is not stored as own removal/probe proof. Original external scalar
identity is bounded and rechecked for reuse. Listed nongarbage drivers still
require scope-bound strict native retirement. Unknown scope/class, absent
nongarbage, contradictory listed garbage, reuse and external-to-live changes
refuse. Fighter/gear/AI census policy is unchanged. Focused client Lua110 and
two syntax files passed.

Rare capture diagnostics use existing x_native_worker events with exact
epoch/entity/incarnation/directory/revision/frame references, stage and
enter/exit edges. First status/directory/control/config calls are covered
before Adapter.capture; static harvest/passport/equipment, class/mesh census,
parent closure, component/bone/morph/material dictionaries, vertex LOD/count/
native getter/RGBA copy and signature passes precede bind/core/render/publish.
Scalar details are copied without querying/stringifying engine objects.
Records are capped at4096 per world with an explicit limit marker; repeated
hot frames stay quiet. Existing native acceptance checks remain unchanged.
Worker146 and source74 checks plus four/five syntax files respectively passed.

Read-only audit found no evidenced ordinary capture deadlock: API reentry
uses try_lock, borrowed guards do not reacquire it, source capture does not
hold mirror_mutex, bridge reads release locks before engine calls and pinned
UE4SS/Lua locks are recursive on these paths. No lock was rewritten. Concrete
static capture costs are high: bone metadata uses12B complete mesh censuses
across two harvests, each qualifying every mesh, and typed RGBA copying may
read four reflected channels per vertex with repeated guards. These are
algorithmic cost facts, not measured attribution of the actual stall. Use
the next native stage evidence before claiming a specific blocking operation
or weakening consistency checks.

## Actual rare-stage run: parent closure and complete-body budget

Exact clean 19b7e9b1 passed full G0 (74 Lua suites, 1314 Rust tests/79
binaries; events141 emitters/zero violations, clippy zero errors/six warnings),
pushed to dev and deployed with matching stamps. The shorter45s default real
three-game test is `test-results/20261009-032525-978353-native-host`.
Authority native_ready occurred at9.566s. First status/directory/config calls
returned; first native_control took0.698s (9.567 to10.265).

Capture entered at10.268s, passport returned10.413 and equipment10.603.
Willie's complete native mesh census returned26 components in0.315s
(10.608 to10.923); its axe returned13 in0.190s (10.925 to11.115). The last
unmatched entry is parent_closure at11.115s, getter
`Actor.RootComponent/SceneComponent.GetAttachParent`, entity1/incarnation4/
directory4/revision1/frame1/pass1. No component-static/bone/morph/material/
LOD/vertex/bind/core/render/publish entry followed before shutdown. This
localizes the observed stall to attachment closure; it does not prove an
infinite loop. Repeating the measured complete mesh census for each parent
read is already a material computational cost and must be eliminated with
equivalent original-handle, owner/world/root/path and complete-census guards.

Both clients progressed past95 fresh qualified already-garbage external
driver records, then refused the live Willie's ActorComponent census at
count257 against per-actor256, at15.865/16.082s. This is another incorrect
resource assumption for the native body, not evidence that a collider can be
skipped. The next complete inert census must fit the established aggregate
1024-component budget and retain every native owner/tick/physics/collision
readback; no pose/fairness tolerance is changed.

There were no canonical frames or active inputs ([0,0]). The authority again
missed graceful shutdown and was stopped by verified owned fallback. All
four original process identities (supervisor29164, source35100,
clients15820/40688) were independently absent, original saves unchanged and
crash/unobserved-child lists empty; source stayed exact clean19b7e9b1.
The trace's large signed epoch was rendered as scientific numeric notation
by shared logging, so its exact64 bits must not be invented. Ref/component
identities are exact. The next narrow phase trace adds original integer
epoch_text without changing native metadata or wire encoding.

Client budget candidate 40717ae2 uses the existing1024 aggregate budget for
per-actor iteration as well; the aggregate is not raised. Every component is
still owner/native-garbage/tick/primitive-physics/collision/visibility checked,
duplicates refuse and the actual root must remain included and unchanged.
Focused client Lua121 registered checks and two syntax files passed, including
all384 primitives and the final collider, exact1024 coverage, over-budget
totals, missing/changed root and garbage components. Actual coverage awaits
the next run.

Phase-only epoch_text correction df897190 preserves original integer signed
decimal text and explicitly reports unknown for missing/floating epochs.
Native metadata/wire remains unchanged. The worker fixture also corrected41
older standalone T.eq predicates into registered T.check assertions. Earlier
historical suite counts did not prove those equalities; the current worker191
registered checks and two syntax files passed, including the seeded original
save and world/reentry touch-count assertions.

The next source performance correction proposes complete native Mesh censuses
at each harvest's start/end, retaining two equal harvests, and original native
handle resolution for individual reads. Handles must pin original index/
serial/address/FName/class and verify native garbage, original owner/world,
root, exact runtime path and actual parent links. Serial-zero components use
the existing checked original-name/class/address strategy without allocating
serials. Each read must obtain a fresh Lua wrapper from the exact runtime path
after resolving the original scalar identity, then requalify afterward; no
unchecked UObject wrapper survives a native call/callback. Native source
bind/capture guards, complete geometry, cycle/final64/wire bounds and all
fairness tolerances remain required. This removes repeated whole-mesh work
without treating a stale wrapper as lifetime proof. Implementation and actual
verification remain pending.

## Source identity scope candidate ready for native verification

Commits bb4e87a7 and 92daec38 implement the source scope described above.
Four game-thread-only native APIs admit, keep, resolve and close original
scalar identities. Admission also checks the source role, current world and
directory/entity generation. Component reads qualify slot/serial/address,
native FName/class, garbage flags, owner/world/root/path and attachment links
before and after obtaining fresh exact-path Lua wrappers. Serial-zero slots
retain the checked original-name/class/address strategy; no serial is allocated.
Unknown ancestor owners are discovered natively and admitted only within the
original pawn/weapon owner set. No unchecked wrapper is retained across calls.

Both equal render harvests retain complete mesh sets at their start and end.
The two-owner fixture now performs four complete censuses instead of more
than240, without changing final64-component, geometry, wire or fairness bounds.
Focused native scope tests10, native API test1, source Lua107 genuine registered
assertions and three Lua syntax checks passed. Thirty older standalone T.eq
predicates were corrected into registered assertions. These results establish
the focused contracts only; actual attachment completion, frame publication,
client mirror acceptance and positive native player inputs remain unverified.
The next deployment/run must use one exact clean head and two rendered native
AI-intent clients plus one actual NullRHI game authority.

## Actual source-scope run: client census fixed, worker injection missing

Exact clean 767110e3 passed full G0 (74 Lua suites, 1314 Rust tests/79 binaries,
events141/zero violations, clippy zero errors/six warnings), pushed to dev and
deployed with matching clean commit/binary/nested G0/native stamps.
The actual45s three-game run is `test-results/20261009-041101-6fe6f8-native-host`;
source UUID is06ef078c-74b8-4c6d-9e54-f85e511a49d1.

Source reached native_ready at9.540s. Both clients completed native suppression
(two own retired drivers and95 qualified already-garbage external records) and
reached wait_scene; the earlier live fighter component257/max256 blocker is
cleared. Source capture now refused promptly at10.226 to10.617s with
`native source identity scope unavailable`, exact epoch_text
`-8112226904552778149`, entity1/incarnation5/directory6/revision1/frame1.
The production worker Adapter.new passed only resolve/WG/phase and omitted the
new source_scope dependency. Thus this run did not exercise native scope entry,
attachment closure or native render publication. This is a concrete integration
omission; the isolated scope/Lua checks did not establish production wiring.

Canonical frames/samples and active inputs remained zero/[0,0]. The2044 neutral
dispatches at44.082s do not establish player input. Source stopped cleanly at
44.978s, supervisor exited cleanly and all original owned identities
(supervisor11376, source24624, clients34680/14048) were independently absent.
Original saves remained unchanged, crash/unobserved-child lists were empty and
source remained exact clean767110e3 throughout. The immediate correction is
explicit injection/type checks for all four native scope APIs, with a worker
fixture exercising actual production adapter options instead of bypassing this
dependency via a capture_render substitute. Real native acceptance remains open.

The correction now passes all four exact native functions directly into the
production Adapter.new source_scope table, preserving dot-call arguments and
both keep return scalars. Any absent/nonfunction endpoint refuses before adapter
construction. Worker198 genuine registered assertions, production source107
assertions, two syntax checks and whitespace validation passed. The regression
constructs the actual production Adapter with no capture_render override and
checks each missing API independently. A read-only second-agent audit confirmed
all endpoint argument/return shapes against lifecycle integer metadata and native
source-role/game-thread admission. This candidate still requires an exact clean
deployment and the actual three-game run; no frame or input progress is claimed.

## Actual wired scope run: complete attachment closure, Aim Spline refusal

Exact clean281c3784 passed full G0 (74 Lua suites, 1314 Rust tests/79 binaries,
events141/zero violations, clippy six warnings/zero errors), pushed and deployed
with matching clean binary/G0/native stamps. Run
`test-results/20261009-042355-626e08-native-host` configured the standard100s
actual three-game topology. Source UUID6cb8539b-2f26-4899-833b-89bba21324e1;
exact epoch_text6598923911927418815, first entity1/incarnation5/directory6/
revision1/frame1/pass1. Native_ready occurred9.375s.

The source helper now executes. First capture9.996 to26.692s: pawn mesh census26
completed10.359 to14.687s, axe mesh13 completed14.691 to16.675s, and complete
attachment closure47 nodes returned16.675 to25.633s. Component_static entered
25.634s for Aim Spline, original address1422673405696, and refused at26.692s:
`native spline anchor rendering not proved absent: Aim Spline`. Later attempts
report the same refusal at41.882/56.953s. This establishes complete attachment
progress, not usable scene publication. bDrawDebug type/value and shipping spline
rendering remain to be established; do not assume absent or strip native content.

Both clients passed suppression (two own retired drivers,95 qualified external
drivers and one fighter), then remained wait_scene with frame/entity zero. The
unchanged65s client readiness deadline requested stop at65.348/65.784s despite
the configured100s run. Directory7/8 and incarnation6 changes followed that
teardown, so they do not establish an ordinary gameplay lifecycle fault. The
final retry completed closure47 at76.881s and entered Aim Spline static at
76.882s before missing graceful shutdown. Canonical samples/frames and active
inputs remained zero/[0,0]; no vertex/bind/core/render/publish stage was reached.
All original owned PIDs42196/30608/5952/38208 were independently absent after
verified fallback; saves were unchanged and crash/unobserved-child lists empty.

Separate read-only cost audits identified repeated native GetOwner function/class
lookup and ABI enumeration, repeated hard-property layout lookup, plus redundant
Lua indexed-controller/world resolution. The immediate safe native candidate is
a scope-local scalar function/class identity and copied HsmpProp schema cache,
with original identity/flags admission before and after callbacks and all actual
link values freshly read. A pure-token-only Lua guard is not yet equivalent:
actual GI current-world and indexed-player mapping must remain checked. Keep the
present Lua current() checks until equivalent native admission is demonstrated.

Candidate fd5a9690 implements only the scope-local scalar schema cache.
GetOwner function/class identities and its verified8-byte return layout are
captured once per original scope; hard-pointer layouts are keyed by original
class identity/literal field and bounded81. Every pointer value is still read
fresh, original identities/native flags/world/generation are still admitted and
the function/class/component are now requalified after every owner callback.
No borrowed object/property pointer or cross-scope cache is retained. Focused
native15 tests passed (ten retained/five new), covering schema inspection counts
while actual dispatch/read counts continue, class-specific offsets/serial-zero
class FName reuse, garbage/reuse before dispatch and callback identity/world/
root/parent changes, invalid ABI and the exact bound. Native timing is pending.

Candidate f1517b53 keeps spline eligibility strict and adds rare scalar
scene_eligibility evidence to the existing phase contract. It records actual
bDrawDebug type/value, copied component visibility/hidden state, actor hidden
state and existing direct HasAnyFlags transient/garbage reads before copied
class/address/owner/root labels. Critical facts survive the512-byte phase reason
limit even with long names. Source Lua112 registered assertions and two syntax
checks passed; no API/DTO/CPP/profile change. Matched SDK/header/ObjectDump prove
USplineComponent is a UPrimitiveComponent with a hard bDrawDebug bool at0x5F8;
pinned LuaUObject BoolProperty Get pushes an actual Lua boolean. Cooked/live
debug/visibility values and shipping rendering absence remain unproved. The next
ordinary actual capture must supply these facts before changing support.

## Actual spline facts: visible/debug enabled, publication still refused

Exact clean55b2ed06 passed full G0 (74 Lua suites, 1314 Rust tests/79 binaries,
events141/zero violations, clippy six warnings/zero errors), pushed and deployed
with matching clean binary/G0/native stamps. Actual45s run
`test-results/20261009-044807-1a4b12-native-host`, source UUID
29c8f909-86be-4c70-9df3-282e49382f51, records native Aim Spline facts:
class /Script/Engine.SplineComponent, address1189686658720, bDrawDebug is an
actual boolean true, component visible=true/hidden=false, owner_hidden=false.
Component/owner transient and native garbage reads are all false. Thus this is
not a missing bool conversion or an observed hidden-spline case. Native component
flags alone do not establish that shipping spline scene proxies render pixels;
rendering absence has not been proved, so strict eligibility stays refused.

First capture refuses at26.309s. Pawn mesh census26 took3.823s (10.972 to14.795),
axe mesh13 took2.089s (14.800 to16.889), closure47 took8.217s (16.889 to25.106).
These are only small changes from the previous4.328/1.984/8.958s and do not
establish a useful speed gain from schema caching. Both clients again completed
suppression and reached wait_scene, with canonical samples/frame zero and active
inputs[0,0]. Repeated refusal occurs40.221/53.794s; the45s stop request was
serviced after capture, with own stopped at53.815s and clean supervisor exit.
Original owned PIDs8640/35252/32640/2488 were independently absent, original
saves unchanged and crash/unobserved-child lists empty; source remained exact
clean55b2ed06 throughout.

The next investigation must establish shipping spline rendering or implement
faithful native spline geometry/settings and dynamic state on both ends. Do not
accept debug=true by name, discard rendering or invent a static curve. In
parallel, audit repeated Lua indexed-controller/world resolution while retaining
actual GameInstance current-world and original player/possession checks around
callbacks. No native playable state or parity is established by this run.
