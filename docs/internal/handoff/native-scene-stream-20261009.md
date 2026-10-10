# Native full-scene encoding checkpoint

## Current priority: compact native gameplay and lossless compression

Actualec9c60c4 fullG0 Lua77/Rust1377/79, pusheddev/RequireG0/all396:
standardGameplay045316-c0c96b client1 offered-result acquisition succeeds and
provider apply frame198/dir7 executes, then refuses `native gameplay exact
root/velocity readback failed` after7ms; receipt40.0785ms. Client2's original-stage
guard refusal follows the primary apply error. Readiness/input0/0; no tolerance
change or gameplay parity. Early original windows secondaryPASS; later Sky capture
misses exited windows, no pixels/model proof. CleanupPASS/four absent/save20/nodumps.
Evidence native_operator_summary. Next exact requested/readback field diagnostics
and native root/rotation/velocity semantics; do not relax memcmp to hide mismatch.

Bounded native audit: GetVelocity exec RVA34A5A00 dispatches vtable+358 rather
than directly reading Movement.Velocity; derived Pawn/physics branch remains
unpinned. GetActorRotation exec34A7C00 reads root world quaternion and derives
cached Euler through1227A60; setter34A8290→349F490→3BF2EA0 takes Rotator.
Raw requested/getter Euler identity is not a proved native contract. Next copied
readback diagnostic identifies first field/axis/bits and all-nine mismatch mask
without changing setters, comparisons or guards. No inferred physics fix.
Copied failure diagnostic now implemented: first field/axis, requested/actual
17-digit doubles and raw64 bits, all-nine mask, bounded192-character reason;
success performs no formatting/logging/engine callback. Six focused diagnostic
cases cover exact match/ULP/signed-zero/velocity/extreme formatting. Strict C++
box_provider_compile + native_presentation_check PASS1770; independent two-file
review CLOSED. FullG0/deploy and native diagnostic retry pending.

Actuala66662ed fullG0 Lua77/Rust1377/79, pusheddev/RequireG0/all396:
standardGameplay044025-3f1f8d passes native begin/construct/finish on both clients.
Armor and held weapon restoration, full pre-possession gear checks, possession
and post-possession gear checks advance; first client apply frame367/dir7 refuses
`native gameplay result generation changed` with receipt54.452ms. No ready or
active dispatch0/0; no combat/all-gear parity. Source reports native_ready until
requested stop. Early qualified windows both secondary; actual Sky screenshots
show loading overlays only. CleanupPASS/four absent/save20 unchanged/nodumps.
Eight encoder samples2568 raw→1232 compressed envelope bytes/24us aggregate,
excluding UDP/auth overhead; no client decode timing. Evidence native_operator_summary.
Next narrow original-result lease/apply boundary investigation; don't loosen
freshness/generation/roster guards to accept an unrelated scene.

Exact failure path is scene() comparing Lua authority_tick with the latest
ResultArc. Source remains dir7/inc5 through sampled366/frame367 wall3659521;
client apply refuses wall3659560, while dir8/inc6 arrives only3659587 afterward.
Candidate retains the exact successfully offered Scene Arc and original receipt,
checks the requested tuple against that offer and current full same-generation
roster/descriptors, and keeps250ms application expiry. Newer same-generation tick
cannot replace or renew the offered data. Discard drops the offer. Core exports a
scene before normal pawn cleanup; take_pawns clears applied proof and takes old
handles while retaining that immutable offer. Focused native gameplay tests6 PASS,
including original lease and actual scene→cleanup→begin boundary cases. Independent
narrow acquisition and cleanup-order reviews CLOSED. FullG0/deploy and actual
retry pending.

Held-weapon candidate follows native startup5707/5874: exact captured class and
all25 passport fields, actual typed-null hand, Dropped=false/DestroyPrevious=true.
Existing logical actor aliases reuse one original actor rather than spawning
duplicates. Unknown/wrong actors and missing sheath-only creation refuse. Caller
restores armor, then weapons, before unchanged full gear verification/possession.
Focused helper62/caller132 and syntax2 each PASS (offline). Final original actor
and missing-target checks follow function metadata and latest-pawn callbacks;
three replacement regressions refuse before dispatch. Independent narrow helper
and caller reviews CLOSED; actual native retry pending. No gameplay parity claim.

Actualf84c3828 fullG0/dev/RequireG0/all396 StandardGameplay041713-6940fb:
client1 native construct and restore_live_armor succeed, then full gear verification
refuses `native missing weapon: Weapon R` (frame23/receipt42.755ms). No possession,
ready/view/input established. Client2 original guard refusal follows primary
weapon error; stage/freshness semantics remain unchanged. Early identity-based
window coordinator qualifies both physically on secondary. Sky selects returned
windows but processes exit before pixels; no screenshot/model proof. CleanupPASS
four absent/save20 unchanged/nodumps. Evidence native_operator_summary in thatrun.
Next exact native source weapon setup/alias verification, no guessed Fists,
passport defaults, tolerance changes or duplicate detached weapon accepted.

Actuala6adc862 fullG0 passes Lua77/Rust1377/79, pusheddev/RequireG0/all396;
standardGameplay040054-7fc376. First two source captures16204–16666 and16671–17109
total905ms; later recapture700ms. Prior full render bootstrap28.382s, separate-run
comparison. Two complete native recipes5603/5594 bytes. Eight actual gameplay
encoder samples2568 raw application bytes→1258 compressed envelope bytes (51.01%
smaller),22us aggregate encode; excludes UDP/auth overhead, no client decode timing
emitted. Compression does not establish player capacity, combat or playability.
Client1 native_beginPASS then passport stage refuses `native source/world operation
guard changed`; client2 native_begin/constructPASS then live armor differs. No
ready/full owned view/input/active dispatch0/0. Test stopped before independent
secondary placement could observe visible originals; do not claim placementpass.
CleanupPASS/four absent/save20 unchanged/nodumps. Evidence native_operator_summary
at test-results/20261010-040054-7fc376-native-host. Fix codec diagnostic missing
required arena/reason fields. Host stage guard and exact native live gear setup
under investigation, no relaxed receipts/defaults or empty gear accepted.
Primary stage code already separates bootstrap from250ms application receipt;
no stage/freshness/guard semantics changed. Client2 armor error precedes client1
guard refusal by53ms; first fix live gear before treating that second refusal as
independent. Cooked native update703 calls Set Up Armor(false,true); initialization
41256 calls(false,false), and no Clear Previous read exists in retained routine.
Candidate stages exact captured live armor values in typed construction map,
checks complete temporary passport, calls proved update(false,true), checks full
live armor then restores/readbacks the full original construction passport even
on qualified failure. Known matching gear skips update. Helper47/client121 Lua
assertions PASS/syntax2 each; native retry pending. Early read-only identity-based
window placement coordinator observes original test windows before fast failure.

Actuale81c4868 fullG0/dev/RequireG0/all396 StandardGameplay033032-52d9ad
passes client2 native_gameplay_begin (frame2/dir7), closing the Actor wrapper
failure. Next native refusal is Character Passport's nested Equipment.ArmorinSlots
StructProperty table conversion. Pinned LuaUObject.cpp1287's table-to-map setter
uses default stored_at_index1 for key AND value despite nested parent passing-1;
actual stack index1 is Weight0.5, rather than the armor passport table. Correct
typed gear assignment is under investigation; no defaults/guessed native map
layout or empty-gear success. Dispatch0/0; no eight-sample codec metric yet.
Independent cleanupPASS/four originals absent/save20 unchanged/no dumps/secondary.
Evidence: test-results/20261010-033032-52d9ad-native-host/native_operator_summary.json.
The actual28s cold recipe capture motivates a recipe-only gameplay bootstrap;
full mirror/vertex metadata is unnecessary when local native assets construct it.
Audit the exact source gear recipe and original generation checks before changing.

Candidate GP-only schema1 omits only render/component/topology records; all native
Character/Construction/Armor/Weapon fields and hand/sheath aliases remain strict.
Source does two complete equal harvests and retains native original scope from
first pass through describe's final owner/world/all-seven-weapon-null hardlinks.
Captured scope cleanup covers pre-native dispatcher refusal and every postcapture
phase/team/describe fault. Source353/worker397 Lua assertions PASS; source syntax6
PASS; independent narrow review CLOSED. Typed passport Empty/Add/Find staging
avoids broken nested table setter, complete readback retained; helper38 and client
111 assertions PASS/syntax2 each; reviews CLOSED. Native Rust/server integration,
fullG0/deploy and actual timing/model/movement/codec validation still pending.
Rust focused server9 pass and corrected complete synthetic weapon roundtrip1
pass; original fixture had no live weapon, so indexing it failed before encoding.
No runtime/tolerance change in that correction. Native gameplay4 pass including
all-seven original weapon/null dictionary closure. Separate GP multipart0x0A93
retains authenticated compression hint/bounds and full exact original recipe.
Host closed native lease address-before-dereference and final sequence/dictionary
review. Source construction.actor_scale replaces the prior begin scale1 default.
All candidate slices frozen; next fullG0/push/deploy then actual standard run.

Actual816dd5bf fullG0 passes (Lua77 suites/Rust1375 tests79 binaries), pushed
dev and RequireG0 deployed all396 hashes. StandardGameplay032237-23a5a1
reaches authority native_ready16.927s; original two recipes capture17.111–45.493s.
Client2 receives compact frame2 fresh25.437ms, then native_gameplay_begin refuses
`gameplay wrapper: source wrapper factory userdata/stack contract`. Both clients
remain opaque loading views; native movement dispatch is0/0. No codec eight-sample
metrics emitted yet, so no live byte/time gain or gameplay readiness is claimed.
Independent cleanupPASS: four original processes absent, save20 hashes unchanged,
no new dumps/unobserved children; both original windows physically on secondary.
Evidence: test-results/20261010-032237-23a5a1-native-host/native_operator_summary.json.

Pinned UE4SS LuaUObject.cpp377–379 constructs AActor userdata for actual Willie,
while the shared protected factory checks only registry metatable UObject. Fix
the exact remote-object metatable set to UObject/AActor, preserving current Lua
coroutine, protected call, exact-one-result and all original object/generation
checks. Add narrow bridge regression, independent review, fullG0/deploy and retry;
do not infer readiness from transport or widen existing native bounds.
Candidate bridge/current-coroutine/postcallback fixture163 assertions PASS;
strict C++ bridge/provider object compile PASS. No native retry yet.

First full G0 of3fd71545 blocked events (six undeclared client diagnostic fields)
and the existing input fixture's exact `possession` spelling. Declare the explicit
client/codec fields and retain the established reason with all fresh guards and
incomplete latches unchanged. Focused native_worker_input_g1 then passes; no
fixture, tolerance, native acceptance or deployment bypass was changed.

The owner's API/RPC and compression steering supersedes further mirror metadata
micro-optimizations. Implement capability-gated actions to the native authority,
exact compact authoritative root/Health/Stamina results, and real client Willie
pawns constructed from the complete source passports and local native assets.
The initial slice must prove movement/Run, original possession, native gear and owned
view/HUD; it does not establish limb injury, dismemberment or co-op parity.

Independent LZ4 blocks retain original payload bits, bounded decode and a raw
fallback for small/incompressible records. Codec's three focused Rust tests pass;
ordered-input Lua suite passes382 assertions. Neither result establishes native
gameplay or network capacity. Before a native run: complete independent review,
commit, pre-push full G0 and exact RequireG0 deployment. Record actual packet bytes,
encode/decode time, command execution ACKs, native pawn/view/HUD proof and cleanup.

Focused compact server8 tests pass, including two authenticated UDP clients,
compressed exact results, complete recipe bootstrap without RenderWorld,
native-ready acknowledgement, ordered Run press/release and stale-receipt refusal.
Provider2 Rust layout/type checks compile and pass; C++ provider object compile
passes `/W4 /WX`. These are transport/implementation evidence only. Generic
AHUD visibility does not yet establish Half Sword's actual stat widget bindings.

## Actual830a1e0f: contiguous layout not enough; no recursive guard shortcut

FullG0/dev/all394 normal021736-c902b3, sourceb48c86fe-5ec5-4471-9ff1-
f48d2abe9120/auth25816/c47092/20732. All16 LIVE-state warm attempts exitstale;
c2421–462ms/c1533–629ms. No adequate layout gain, standardfalse/activeboth0.
CleanupPASS4absent/save20same/nodumps/unobservedchildren/bothsecondary. FullG0
passes after deterministic resize fixture fix; no UI tolerance/runtime change.
Bounded code audit finds NO recursive check_guard path in mesh validation or
Rust callback. Counts are actual repeated OUTER get/property/Function reads;
do not implement recursion suppression. Next primary proof must identify a
specific callback-free metadata/argument-preparation block before any fresh
entry/exit batching. Retain all original identity reads and EVERY callback/PE
guard; do not infer purity from getter names or native duration alone.

## f1556483 G0 blocked by unrelated resize fixture substring

CPP contiguous-plan compile/review1,764 pass, but fullG0Lua74/75 failed
menu_ui.resize1's global negative "2883" search; Rust1364/79 and clippy pass.
No deployment/native run of this commit. Paths use random16hex state IDs and
the menu logs them; matching bare digits does not identify a canvas regression.
Unchanged target repeat12526 passes. Root fixture correction injects actualmock
bin path C:/HSMP/2883/bin, verifies it is logged, and forbids exact canvas text
"-> canvas 2883x1622" while retaining correct viewport/physical widget/lobby
checks. Fixed focused suite12527 passes; no runtime UI or tolerance change.
Need independent tiny review then new clean combined commit/fullG0/native.

## Actual1cc1b96f: accurate timing confirms fresh mesh proof dominates

Normal020203-b03f9c/fullG0/dev/all394, source08be7971-abed-49be-9e50-
bca90fdf292d/auth18848/c32640/2260. Warm private nanoseconds accumulate and
convert once. c1 profiled apply181ms/mesh133ms/guard149ms; obj_prop2.35ms,
getidentity7.16ms/cachedFunction6.63ms. Finish96ms/mesh78/guard84ms. Extra
metadata/schema/copy costs do not explain most delay; no speculative schema cache.
c1laterLIVE warm424–431ms, c2LIVE542–621ms (direct totals); allpostapplystale.
Originalguardcounts185143/74215 stay. Standardfalse/activeboth0/ownedviewunproved.
CleanupPASS4absent/save20same/nodumps/unobservedchildren/bothsecondary. Next
concrete fresh MeshBoundary.validate cost reduction supported by original proofs;
no successfulvalidation reuse, pinrefresh or looser receipt.

Candidate compiles map-based strict agreeing merges, then copies full original
class/node entries into contiguous arrays in exactly map order. Both immutable
arrays and mutable map fallback use the SAME native validation body/order,
two fresh passes and all original per-binding link/RF/pin checks. No native
field pointer or extra assumption; two CPP files/no ABI/receipt/lifetime change.
Fixture compares complete array ordering and exact native resolve sequences/
counts with the old map path. Actual timing pending; no assumed vector gain.
Strict provider/fixture1,764 checks pass; independent reviewclosed. Complete
ordered rows and native resolve sequence/count equality are explicitly asserted.

## Actual65946a48: hot-path microsecond truncation invalidates internal attribution

Normal015139-f6bbe0/fullG0/dev/all394, source18d63492-ec12-4fff-8e52-
85cc39ae64b3/auth44736/c27192/18672. All warm exitstale; activeboth0. c1 first
profiled operations are READY, c2LIVE; do not conflate states. Wholewarm totals
are direct elapsed observations, but each inner timer truncates EACH call to
microseconds before summing. c1 getidentity21us/144,764calls andmesh445us/
185,143guards cannot establish negligible cost: submicrosecond work is lost.
No property schema/identity cache optimization follows from those numbers.
Next precision-only change accumulates private warm nanoseconds/ticks and
converts once at copied emission, preserving existing source/public-row units,
two-attempt/68linebound and every native check. CleanupPASS4absent/save20same/
nodumps/unobservedchildren/bothsecondary. No gain or gameplay claim fromprobe.

Precision candidate changes exact CPP implementation+fixture only. Warm buckets
sum private nanoseconds and convert once to existing microsecond log labels;
only the exact warm row uses private units, all other source/public rows retain
their original convention. Deterministic400ns aggregate, source-unit isolation,
TLS/unwind and68linebound assertions cover it. Actual accurate attribution pending.
Strict provider/fixture1,762checks pass (six precision/source-unit/isolation/
saturation assertions). Independent review closed; warm-only addition saturates
without changing source arithmetic. Deterministic1000×400ns emits400us once.

## Actual6733e739: whole warm presentation roughly halves, still stale

Exact fullG0/dev/all394 normal014140-46a600, source4ab96e5b-c850-4347-b94f-
902988b3b3db/auth5040/c45832/46372. All16 warm totals515,588–591,281us vs prior
983–1529ms, actual separate-run gain, not normalized benchmark. Instrumented
permirror applies223–242ms/meshproof70–99ms/guard73–102ms; alloriginal counts
remain185,143guards/3,462PEs and finish74,215guards/354PEs. Finish roughly97–130ms.
Every warm attempt still entersfresh/exitsstale postapply; original250ms retained.
Standardfalse/activeboth0/ownedview unproved. CleanupPASS4absent/save20same/no
dumps/unobservedchildren/bothsecondary. Retain measured improvement; attribute
remaining apply/finish costs before another change. No playable or release claim.

Remaining permirror apply~140–150ms lies outside measured guard time. Extend
existing firsttwo warm profile with actual obj_prop metadata call, get identity
body AFTER unchanged guard, and cached Function construction/signature copy.
Function bucket is inclusive and excludes cold cache misses. Three CPP files,
private counters/timers only, no query/API/ABI/guard change; max68 copied lines.
Fixture executes actual helper paths and checks counts/reset/logging TLS. Actual
attribution pending; no unsupported schema cache or admission batching yet.
Strict provider/fixture and1,756 checks pass; independent instrumentation review
closed. Existing bound/reset tests now assert68lines and actual helper counters.

## Actualb21c6c25: per-call mesh proof/table construction dominates

Normal013151-58dedd/fullG0/dev/all394, sourcebbfe61b2-76e8-45fe-94fd-
d02574dcc64e/auth46628/c12672/35632. Each instrumented mirror apply makes185,143
guards for3,462 PEs; aggregate finish74,215 guards for354 PEs. Apply totals493–
561ms permirror; meshproof293–432ms, guardinclusive334–440ms. Finish289–297ms,
meshproof224–257ms. Find31–75us and PE628–827us inapply. Inclusive buckets
overlap, never add them. Sixteen whole-frame warm totals983–1529ms, all entryfresh
and exitstale postapply. Standardfalse/activeboth0; no playable/view claim.
IndependentcleanupPASS4absent/save20same/nodumps/unobservedchildren/bothsecondary.
Next candidate compiles copied immutable expectation plans once per warm apply/
finish MeshWatch operation, then runs SAME fresh two-pass metadata and every
original per-binding link/pin/RF check at EVERY existing guard. Mutable create/
current-Part watches keep original rebuilding path. Nested plans combine strictly;
no successful validation ticket, refresh or cross-operation lifetime. Measure
actual whole-frame result before claiming gain.

Candidate owns COPIES of binding expectations in warm apply/finish MeshWatch
scopes, compiles agreeing merges once and reuses only that immutable plan. Every
guard runs unchanged fresh metadata and raw links/RF/pin checks. Mutable creation
chains rebuild; nested immutable watches compile combined original plans with
conflict refusal. Exact two CPP files only, no headers/API/ABI. Fixture includes
ownership, next-boundary mutation and nested/conflicting originals. Actual pending.
Strict provider/fixture and1,753 checks pass (nine ownership/freshness/mutation/
nesting/conflict assertions). Independent review closed after explicitly retaining
mutable create/current-Part callsites and immutable apply/aggregate-finish only.

## Actual027f48e9: mesh witness dedup does not resolve warm delay

Exact fullG0/dev/all394 normal012036-e89ef9, source8fc797cb-0968-4767-9b79-
779226430dd5/auth34216/c20476/31100. All16 warm attempts enterfresh and exitstale
atpostapply. Total1,052,083–1,342,364us/apply813,006–1,062,520us/finish223,282–
300,974us. No adequate complete-presentation improvement; do not claim gain
from smaller isolated final times. Normalfalse/activeboth0. Independent cleanup
PASS4absent/save20same/nodumps/unobservedchildren/bothsecondary. Next bounded
actual attribution of apply/finish guard, lookup and engine-call time before
another optimization. Keep originalreceipt/250ms/sameSceneArc/all proofs.

Bounded attribution candidate changes native_presentation.cpp,
native_scene_impl.h, dllmain.cpp and existing fixture. Firsttwo warm applies plus
matching aggregate finishes use existing stack capture counters/timers. Fixed
copied scalar diagnostics emit after operation scopes, TLS and mutex unwind;
at most44 lines. Guard/proof buckets overlap and are explicitly nonadditive.
No engine query, API/ABI, original check or freshness change. Fixture covers
bound, nesting, success/failure reset and unlocked logging. Actual data pending.
Strict provider/fixture compile and1,744 presentation checks pass (15 additional
diagnostic bound/reset/nesting/unwind assertions). Independent review closed;
logger arguments follow the existing stage/edge/value/marker signature.

## Actual ce46189f: input bindings recovered; warm apply is the live blocker

Exact clean fullG0/dev/all394 normal run20261010-010930-fe6500-native-host,
source88ef8f66-7062-42bc-ab65-38a870dabcad, authority44716 clients30896/47056.
The recorded axis binding refusal is gone: input_refused0, neutral dispatched170,
active_pc0/1 both0. Active gameplay is not proved. First98-row capture273,289us,
provider255,232us/CPP220,663us, rowfind561us/160 and finish17,512us.
Of170 neutral dispatches,150 precede stop. Source83 complete frames overall,
74 confirmed beforestop with zero capture refusals; frame1-to74 mean238ms.
Both MirrorReady50.667/50.908s, more than14s beforestop. No client LIVE.
All16 bounded warm attempts enter fresh and finish stale at postapply. Total
997,758–1,128,286us, apply718,184–815,736us, finish275,298–333,229us;
prepare129–262us. Original receipt is not renewed. This proves local apply/
verification consumes the freshness budget; it does not establish wire latency.
Normal pass=false. Cleanup independently PASS4absent/save20unchanged/no dumps/
unobservedchildren; both windows physically contained on secondary display.
Next minimal performance change deduplicates agreeing original shared path and
class witnesses within EACH mesh_call_guard boundary, preserving all mesh pins,
flags, owner/Level/world links, every guard call and fresh final verification.
No validation may survive a callback or boundary; conflicts refuse the whole.

Candidate changes native_presentation.cpp and its existing CPP fixture only.
Each guard creates copied agreeing expectations, validates fresh unique nodes/
classes before and after every original per-binding owner/Level/world/pin/RF
check. Standalone mesh_binding_final and all guard calls remain. Fixture covers
shared reads and fresh repeat, mutation between passes and original name,
positive pin, class and package conflicts. Actual performance pending.
Strict provider/fixture compilation and1,729 existing presentation checks pass
(11 new boundary/conflict/garbage-access checks). Independent review closed
after preserving RF rejection before metadata access. No performance claim yet.

## Exact input-path regression fixed; warm-present delay measurement next

The a54aa9e9 repeat logs one detailed controller1/entity2 refusal for
Willie_BP_C:InpAxisEvt_Move Forward / Backward_K2Node_InputAxisEvent_14.
Its literal slash is inside the member FName, outside the qualified hash-path
grammar. Route qualification now retains original Slow lookup before any hash
attempt for such names. Canonical paths and qualified-null behavior remain.
Strict provider compilation and 35 focused route checks pass; independent
review closed. Actual input recovery remains pending.

Completed repeat audit: 55 source frames, frame1-to53 mean260.288ms (~3.84fps),
reported capture durations200–282ms. Sparse endpoints cannot establish per-frame
p50/p95. Both clients reach MirrorReady before stop, neither logs LIVE, then
both refuse stale applied scenes. Input dispatched0; aggregate refusal108
does not identify separate controller paths. Original receipt is correctly
retained through apply and input; no renewal or tolerance change is justified.
First presentation takes1363/1630ms. Warm duration/entry and exit receipt age are
unmeasured; bounded diagnostics preserve the same SceneArc and every check.

Warm diagnostic uses existing copied-scalar capture logger tag2 without an ABI
change. At most eight warm native_present attempts report fixed rejection stage,
entry/exit original receipt age, prepare/apply/finish/ready time and success/
freshness booleans. It adds no engine call and cannot diagnose Lua prepeek/input
boundaries. Cold exclusion, bounded attempts and retained stale original receipt
are covered by the focused fixture. Actual measurements pending.

## Actual a54aa9e9: qualified hash active; frame227ms, both models before deadline

Exact fullG0/dev/all394 first run005121-9fc69d spends~58s before worker/client
loops; NativeReady/hash qualification and partial recipe begin follow stop by
~7s. No pre-stop lookup/provider failure. It cannot establish frame speed.
Repeat of the SAME verified build005320-2543e6 (no rebuild/G0 repetition),
source9d68aee9-5e25-415c-8616-cb525afbe97f, authority32616, clients38972/28952,
records exports1/available1/canonical1/qualified_hash on allthree original games.
First complete98-row frame226,809us/provider208,242us/CPP rows181,081us;
rowfind284us/160calls, finish17,836us. This is an actual complete-frame gain
from2.517s, not a normalized hardware benchmark or full gameplay claim.
Both models verify BEFOREstop: client1MirrorReady55.307s/frame9; client2
54.530s/frame7 and55.785s/frame14. Later appliedscene freshness fails; normal
pass=false, complete source cadence/input/receipt audit underway. Preserve
original250ms receipt, same applied SceneArc and all complete readback proofs.
No ownedview or playable/headless/co-op release claim. Both runs independently
PASS4originals absent/save20same/no dumps/unobservedchildren/bothsecondary.
Deployment is warm: directCargo0.29s, CMake0.21s twice. Next exact applied-frame
freshness cause; source-owned camera metadata remains unobserved.

## Native lookup route checkpoint

Primary pinned DLL/source proof: reflection r_find always calls
StaticFindObject_InternalSlow, whose RVA3FB310 enters ForEachUObject at3FB35C.
The DLL also exports StaticFindObjectByPath at4ACEA0 with signature
UObject*(UClass*,const wchar_t*,bool exact,bool useHash). It recursively resolves
canonical slash Package.Object and colon function/subobject paths and calls the
hash wrapper4ACD70. IsAvailable4ABDE0 requires configured+self-test state1;
otherwise the wrapper falls back to an object scan. Three successful hash
self-tests exist in preserved UE4SS logs, but per-call runtime availability
must still be checked. New route resolves exact exports, never guessed RVAs;
canonical slash paths use exact=false/useHash=true only when qualified. Missing
optional APIs/unavailable hash or unproved unqualified names retain old lookup.
Qualified null remains null. Original identity/path/flags/callback proofs stay.
No new wire/provider ABI or cache lifetime. Strict provider and route fixture
compile pass,32 focused checks pass. Static export mapping: pinned real DLL
mandatory23/23 optional2/2; old mock mandatory23/23 optional0/2 still accepted.
PE timestamp/image-size pin is mandatory for this optional route and never uses
the offline unpinned override. First-use logging copies four scalars after the
lookup; no object/string/state access or per-frame log. Independent final review
closed with no blocker; actual route/cadence/readiness remain pending.

## Actual 6f8dd46c: dedup lowers complete frame to2.517s

Exact fullG0/dev/all394 normal run20261010-003619-0f4d03-native-host,
source4b81b085-0414-4daa-a795-0fed1defae60, authority11844, clients43904/32164.
First complete98-row frame2,517,188us/provider2,476,586us/CPP rows2,453,469us;
row find2,262,503us/160calls, externalfinish40,113us. This actual full-frame
improvement from7.186s is still far above original250ms LIVE receipt and2000ms
source watchdog. Separate runs are not a normalized hardware benchmark. Native
find dominates again; verify the actual reflection lookup implementation and
already available native hashed exact-path API, retaining all original proofs.
No speculative native address or new persistent lookup-cache bypass.
Both49/600/38 recipes bind36.147/52.442s; first scene publishes55.087s. Client2
receives completeScene atwall1791589034892, client1 at1791589037000. Client1
nativePresent60.444→65.991s and MirrorReady65.992s are792ms AFTERstopfiles;
client2generation refusal66.025s is883ms afterstop, no client2MirrorReady. Neither
LIVE nor activeinput beforestop. Sourcefive completeframes55.115/57.696/60.286/
62.735/65.063s, gaps2581/2590/2449/2329ms. Firstsource guardchanged873ms after
stop; no pre-stop provider/transport/error/LIVEwatchdog. Normal pass=false. Independent
cleanupPASS4absent/save20same/no dumps/unobserved children/bothsecondary.
Cargo child env normalization is verified: direct release108s, CMake warm
verification0.23s and0.21s, versus107s duplicated verification previously.

## Reviewed dedup of original native witness checks

Each OperationScope interns agreeing original object/class expectations, full
FName/RF/Outer/package/private zero-slot facts and retained positive pins. A
conflicting copied witness refuses; only the existing original Natural0-to-first
positive rule can strengthen a pin. Each boundary freshly reads every unique
class/object, then all original raw row hard links, then every unique class/object
again. No successful validation survives a callback or nested operation. Lookup
key pinning retains its original exact-find/full-path qualification. Existing
world/GI closure, ownership/root/parent/socket/Level-world, per-row tails and
final batch proof remain; no field/check is omitted. Strict CPP provider/fixture
compile and1718 checks pass, including shared Package read count, conflicting
metadata/positive pin and changed next-boundary/later-row refusals. Independent
review closed with no blocker; actual speed pending. CMake normalizes LIB/INCLUDE absence
only for its Cargo child, preserving explicit configure-time SDK environment.

## Actual 0c304fd2: fewer lookups, slower complete frame

Exact fullG0/dev/all394 normal run20261010-002248-8ae887-native-host,
source358042b2-3b1e-45d0-93d7-deb0737ee943, authority1676, clients27312/8676.
First98-row complete frame7,186,196us/provider7,140,975us/CPP rows6,813,380us;
row find time1,891,835us/160 calls, finish43,613us. Recorded row lookup count drops
but complete-frame latency REGRESSES versus prior5.360s. Prebatch admission
finds are outside row counters. Repeated full lookup-map/receiver path proofs
are the next measured CPU target; dedup must retain every original admitted
node, RF/class/weak/FName/Outer/world/owner/parent/socket relation and all
callback/final boundaries. No dropped data/check, global cache or wider TTL.
Both49/600/38 recipes bind35.220/51.733s; first frame publishes59.053s.
Both clients receive completeScene61.088/60.768s, then nativePresent begins
64.650/64.414s. Client1 MirrorReady69.244s occurs4120ms AFTERstopfiles; client2
mirror-generation refusal follows stop by4505ms. Neither is a predeadline gate
success/failure. Frame2 sampled66.168s follows stop by1691ms; publication gap
7110ms. Later source guard/dir/incarnation refusals are teardown. No provider or
transport failure beforestop. Normal pass=false, no active input/owned view proof.
Independent cleanupPASS4absent/save20same/no dumps/unobserved children/both
secondary. CMake marker is absent as intended, but LIB/INCLUDE remain injected
and tracked by ring despite being absent in direct build:108s then107s plus
0.23s repeat. Normalize only the Cargo child environment next, preserving
MSBuild C++ environment and intentional configure-time compiler settings.

## Reviewed: one synchronous complete-frame native capture

Private provider ABI12 appends capture_frame; the wire/schema remain unchanged.
Caller owns all prepared recipes, output storage and expected texture paths
through one call. It pins original world/player/controller and descriptor Arc,
directory/ref/revision/slot for every callback guard. CPP uses one temporary
lookup map and full original receiver/owner/parent/Level paths with RF/class,
weak/FName and hard attachment fields. All rows are checked after callbacks and
at the final pure boundary; nested operations isolate and restore prior scopes.
Original indexed/recipe preparation, scalar frame values and both existing
encode/finish and separate prepublication proofs remain. Failure discards the
whole frame. Texture validation stays inside the synchronous operation, before
the final whole-set proof. No cross-frame cache or freshness relaxation.

OwnerPrivate90 qualification reuses the pinned32-byte shipping GetOwner leaf
at RVA3B54190 from the SourceScope evidence, before and after callbacks. Original
SceneComponent AttachParentB0/AttachSocketNameB8 and Level OwningWorldC0 offsets
are qualified before reads; original receiver paths/dispatch metadata remain
pinned. CPP strict provider/fixture compilation and1667 checks pass; Rust
presentation16/16 pass. Independent Rust review closed. The receiver witness
ends at CPP return; the same Rust full generation/possession/world guard is
checked again after external native finish, before pending assignment. Existing
separate prepublication aggregate proof remains. CPP independent review closed
with no blocker; fullG0/exact deploy and actual cadence pending.
Provider total includes receiver admission; per-row timing/find counts exclude
that preparation. Compare complete-frame totals, not only row find_us.

## Actual ea02f223: repeated asset lookups dominate complete frames

Exact fullG0/dev/all394 normal run20261009-235522-a0d66a-native-host,
source9c640a76-97fe-4238-b690-1c9f9b5aa87f, authority28888, clients41900/7296.
Both49-component/600-bone/38-slot recipes bind by46.561s. Complete first frame
98rows takes5,359,885us; native finds5,231,196us/2564 calls. First diagnostic
stores64 exact path keys with540 untracked events; totals include overflow.
Repeated asset examples: Fabric24 raw finds/462321us, Flesh24/301558us,
Invisible12/233226us, Sphere12/208980us; VertexPaint CDO36/315043us and
StaticMeshComponentLODInfo104/275095us. Classes are generally much cheaper.
Missing path costs must not be guessed. One synchronous frame scope is next,
with ALL original receiver/owner/attachment witnesses and final pure closure.
Independent sums: recorded64 paths4,311,495us, unrecorded919,701us. Recorded
asset labels3,223,318us, classes410,746us, LODInfo schema275,095us, CDO315,043us,
functions87,293us. Short labels remain shortened; no full paths reconstructed.
These costs partition find time only, not inclusive provider/frame totals.
Overflow540 combines98 raw-find/49 cold/393 hit events, not missing paths or
540 finds. Source LIVE watchdog native_glue525–535 rejects publication gaps
above2000ms; retain this and original250ms client receipt freshness unchanged.

Source publications52.035/57.498/62.903s are5463/5405ms apart. Both clients
receive coherent scenes and full MirrorReady59.918/61.898s. Stale frames then
stoppedpublishing fail before stopfiles; later source descriptor-generation
refusal is447ms after stopfiles and remains expected teardown. No active input
or owned view proof. Normal pass=false. All original4 absent/save20 unchanged,
no dumps/unobserved children; both physical windows secondary. Build156s then
CMake115s; exact package selection alone does not close repeated compilation.
Pinned cc/find-msvc-tools reads MSBuild's VSTEL_MSBuildProjectFullPath marker,
recorded in ring's fingerprint but absent in direct build. Next CMake change
unsets only this marker; actual warm verification remains pending.

## Reviewed first-frame lookup diagnostic

First attempted capture reports up to64 distinct class/asset paths, original
cold/hit/bootstrap/capacity counts and raw find time. Overflow remains explicit
and totals retain all events; failure and smaller-roster boundaries are labelled.
Copied reports emit only after the native operation and capture TLS restore.
No extra engine lookup, acceptance change, provider ABI or wire change.
Independent review closed; strict provider/fixture compilation and1586 checks
pass. CMake now selects the same three packages as normal deployment. Actual
lookup distribution and warm verification time await the exact next run.

## Actual a99dbc05: both recipes/models succeed; LIVE frames are too slow

Exact clean fullG0/dev/all394 normal65s run20261009-233229-884aad-native-host,
source1b0951fb-1ce2-4e0c-868d-436581081bd6, authority46188, clients45780/32416.
Both full adapters complete by45.279s. First originalScope15.659969s:49 cold
keeps3.322425s,49 retained keeps3.226ms;102 native finds3.320631s. Inclusive
costs overlap; remaining firstScope costs are retained in authority PID log.
First CPP capture COMPLETE98rows: total4,449,330us, provider4,414,712us,
find4,334,326us/2564calls, aggregate finish33,986us; no truncation. Old4cf total
6,096,600us/find5,300,958us/5634calls. These are actual separate runs, not a
normalized hardware/gear benchmark or a latency/parity claim.

Both clients receive coherent complete scenes at51.280/51.985s; native full
creation/readback succeeds, MirrorReady58.743/61.550s (epoch2025201219372449035,
dir7, refs1/2 inc5 rev1). Client1 also MirrorReadyframe3 at61.702s. Both receive
frame4/dir7 then refuse LIVE stale64.593/64.045s. Publication gaps4.569/4.489/
4.413s cannot meet original receipt250ms. No active input or owned view proof.
Native client zero fields while waiting are report placeholders, not directory
loss. No actual transport/reassembly failure; receipt starts at complete publish,
and exact packet timestamps are absent. Source sampled1–4; later frame5 no-source
and generation changes belong to deadline teardown. Standard pass=false.

All original4 absent/save20 unchanged/no fresh dumps/unobserved children; both
physical windows secondary, separate Steam session untouched. Lookup maps work,
but restart per component:2 bootstrap class finds then cold double-finds; observed
max36/component proves512 capacity is not responsible. Skeletal/static find
time4.041s of4.334s. Next bounded first-frame path/category/time evidence and one
whole-frame batch lifetime with original identity/world/dispatch/final guards.
No global/cross-frame cache, no freshness relaxation. Build first combined112s,
two-package CMake verification87s, identical repeat0.22s; CMake now matches the
exact three-package selection, timing gain pending. No release/main authorization
from these results. User explicitly demands less analysis and faster actual work.

## Actual 314b40d6: slot-zero fixed; standard readiness misses metadata harvest

Exact clean full G0/dev/all394 deployment, normal100s/65s readiness run
20261009-225731-b9c50f-native-host, sourcefd61803b-508e-428c-ae40-556ae0ced6ec,
authority39632, clients44832/15232. Original slot-zero failure is gone. Entity1
complete capture21.381→46.565s=25.184s; bind46.567→47.199s=632ms PASS, with
49components/600bones/38slots, compact35551/raw74701. Entity2 starts47.200s;
first harvest47.442→59.235s=11.793s, second begins59.455s and remains incomplete
at the65s deadline. No whole recipe/frame or NATIVE_CAPTURE_FRAME timing yet.
Completed harvests retain6243 component reads/12844 qualifications/4mesh calls/
49parent hops. Do not confuse per-entity completion with whole-scene readiness.

First source generation refusal atwall1791583116555 occurs15ms AFTER stop files
wall1791583116540. NativeGlue left/unbind increments own incarnation/directory;
Scope correctly refuses olddir7. This is teardown after readiness timeout, not
a predeadline identity bug. Keep every generation check. Next target is measured
guarded metadata harvest cost, then bounded first controller/PCM observation and
full owned view. Native exact-lookup frame speed remains unmeasured.

Independent original4 absence/save20 unchanged/no new dump/unobserved child;
user's separate Steam game7736 and its qualified children remained untouched.
Both development windows physically contained on secondary. Read-only pixel
inspection shows opaque waiting view with animated bar/elapsed time; actual
asset-completion/error text pixels remain unproved. Standard pass=false.

Actual entity1 nested timing: render24.717s; static component metadata12.513s
over98 rows, mesh census8.604s/8 pairs, parent closure1.578s/2. Within static:
vertex2.830s/60, bones2.529s/26, materials2.307s/78; nested totals are not additive.
Initial mesh row admission8.418s versus final complete recensuses186ms. Both
descriptor passes currently begin a fresh native Scope and repeat cold row
admission. Existing Rust keep checks retained address only AFTER double exact
find/path capture. Native schema/property/GetOwner caches already exist.
Per-field ignored Lua current() guards also repeat indexed PC/world resolution;
derived37458 GetPC/24972 GetWorld per harvest are structural estimates, not
measured native counters. Native Scope.base alone lacks indexed remapping proof;
do not replace these guards with token-only checks.

In progress: one original Scope across both complete equal harvests with every
Lua indexed/current guard and payload read retained; qualified original-witness
fastkeep removes only repeated cold path capture/find, retaining strict fresh
Identity/Parent/path/RF/owner/world/final closure. Same-path replacement refuses.
Final component qualification must precede scalar-only scope_end after last
callback. Add bounded first-Scope timings after borrow release. No speed claim.
Build optimization shares explicit locked Cargo release output and requests the
two shipped DLL targets; full G0/RequireG0/content/hashes remain. Independent
review/tests and new actual timing are pending.

Coherent optimization review CLOSED: Rust SourceScope48/48, Lua adapter327 and
syntax2 pass; compiler/Cargo released, files frozen. Successful Adapter closes
with explicit end(id,true) after final indexed/native callbacks; this takes the
Scope outside RefCell and verifies ALL original paths, hard links, owners and
original Actor→Level→OwningWorld without callbacks. Default end(id) remains
scalar cleanup on error/world drop. Original failure is never masked.
Retained keep preserves strict original fresh Identity/Parent/path/owner/world
checks, refuses same-path different-address rebinding and skips only fresh cold
find/path capture. Resolve CAPI final proof uses input handle, not returned address.
GetOwner exact32-byte shipping RVA3B54190 leaf proves raw90, with original type/
function/class/code identity; Level.OwningWorld reflectedObjectProperty8/C0.
Schema82 includes the added world field. One first Scope emits15 inclusive cost
rows after borrow release; begin/op/end failures clear/log once. No ABI/schema/
wire change, no indexed guard removal, no tolerated freshness changes. Build
patch uses combined locked server/native/fake output +normal CMake Cargo verify,
targeting HSMPNative and hsmp_lua. Actual new-build speed/readiness/view pending.

## Actual d9823407: first original refusal proves the slot-zero case

Exact clean dev/full G0/all394 hashes, non-CDB run20261009-223840-80958f-native-host,
source31b4341b-3b5c-42a0-9e77-b2041cc75a0a, authority33344, client14296/27956.
First source failure21.811s preserves `native path node weak unavailable` at
outer_node depth1, weak0; idx0=1/1/1/1 proves admitted slot metadata, present item,
copied failed Outer address equal to slot0 object, and slot serial0. No complete
recipe/frame/model, owned view or active input. This is a legitimate original
slot witness missing from the private lookup cache, not evidence for global
null-weak relaxation. Net owns the narrow correction; Host independent review.

Root requested supported graceful stop after capturing the predicate, retaining
pass=false and operator_stop.json; harness stop-request result is not the cause.
Independent cleanup verifies all four original processes absent, original save20
unchanged, no fresh dumps or unobserved children. Both qualified client physical
windows were contained on the smallest secondary display. No debugger attached.
Operator summary retains original failure and deliberate diagnostic stop.

Loading UI correction labels only player assets and changes n/n to an animated
pending model/view stage rather than a completed match bar. First bounded UTF-8
fatal cause survives later stop/progress/world drop. Focused63 checks/syntax2
pass, root review closed; no positive owned-view/readiness change. Actual pixel
verification remains pending. User is playing separate Steam PvP game7736 and
server46100 and explicitly requires leaving them running. Root ignored operator
preserves their exact PID/start/exe records, refuses any other existing session;
original save audit stays strict and any concurrent change is retained as failure.

Private operation lookup now admits only a terminal Package ancestor with the
exact original FUObjectItem0 pointer/address/serial0, full Package discriminator,
original class serial/FName/RF and original Outer/path closure. Slot evidence is
checked before newly admitted UObject metadata reads and at capture/hit/final
tails. No weak0 root/returned Obj/source path/global identity entry, serial write,
promotion, cross-operation cache or lowered gameplay bound. Strict provider/
fixture /W4 /WX pass1233 checks (1215 retained plus18); compiler released. Actual
cache cadence and model/gameplay verification remain pending on the new build.

## Actual 0bb472af: cold witness refuses; debugger shutdown needs correction

Exact clean dev/full G0/all394 hashes run20261009-221837-e888fa-native-host,
authority46656, fails first metadata harvest around19.697s before any complete
recipe/frame. First Vector cold lookup stops in core_layout at21guards/3finds;
the exact native predicate is hidden by Lua post-failure component resolution
after the Rust profile scope was dropped. Do not claim speed from this run.
The reviewed narrow diagnostic preserves that original reason before post-scope
access and reports fixed cold-witness stage/depth/copied node/address plus
explicit slot0 metadata evidence on weak0 only. No admission relaxation.
Strict1215 native checks and321 Lua checks/syntax2 pass, independent reviews close.

All four original processes and debugger helpers are gone; save20 unchanged.
Two new client dumps are retained: .4765908 PID43448 SHA B4A381D4E6A263764BFB3272F317B2B8FEFDCF6202C6991C27F0065D83F843E3,
.4951808 PID15364 SHA D6BD8F2F3FBF2A40008EF12B695DE6D32C9E984E9443ACFB3B03E9675FC88CA7.
Both are exception80000003 at ucrtbase!exit RVA91700, runtime byteCC where the
disk byte is45. CDB helpers ended22:20:16.76; dumps22:20:39.48. The old timed
CDB Kill(false)/-pd cleanup left software breakpoints after detachment. This is
a diagnostic-harness fault, distinct from the cold-witness refusal and older
native cleanup AV. Do not reuse that software-breakpoint helper. Next exact
native run uses the gated non-CDB extended operator with early V2 window
placement; future debugging must remove breakpoints cleanly or use exception-only
capture and positively verify unmodified target exit bytes before admitting it.

## Actual 4cf0c143: retirement holds; exact lookups dominate capture

Exact clean dev/full G0/all394 hashes run20261009-215308-6ea88e-native-host,
source09a3ea6c-2e91-43ab-9ab9-27cf231059d5, authority23796, reports one complete
98-component capture with no truncation. Total6,096,600us, provider5,370,536us,
native exact-path find5,300,958us across5634 calls; aggregate finish725,204us.
Nested guard time3019us/229033 calls and PE2013us/4000 calls do not explain the
delay. Rust preparation158us/copy167us/encoding346us. Nested measurements are
not added to contiguous stage totals. This is actual data, not a speed claim.

Both clients again complete both49-component mirrors, and the corrected native
retirement probe prevents the post-MirrorReady retravel/reload observed before.
Client1 also applies frame3 while retaining its mirrors. The LIVE publication
watchdog still faults; no owned view, active input, visible pixels or gameplay
claim. Keep the2s publication and250ms freshness bounds unchanged.

Next: bounded operation-scoped reuse of the original exact lookup, with full
original path/Outer/FName/class/flags/weak witnesses and final pure closure.
No blind persistent path cache or original-object replacement. All original
games/debuggers/helpers are gone, all20 save hashes unchanged, no fresh dump;
both initial game windows physically on secondary. Capture rows, primary weak
proofs, native logs and cleanup are retained in the ignored run.

The reviewed provider-only correction reuses exact lookups within OperationScope
and discards them on exit. Cold binding brackets the full original witness with
two native exact lookups and pins the first observed positive before callbacks.
Hits qualify original full Outer/FName/class/RF and pinned serial; no path refresh
on mismatch. Pure complete-cache closure precedes every reflected/direct native
dispatch and all successful operation returns. Existing native guards/getters/
readbacks stay intact. The512-entry reuse bound falls back to ordinary exact
lookup, imposing no new roster/scene limit. Missing metadata refuses before reads.
Four native files pass strict compile and1205 focused checks, including42 new
reuse/lifetime/negative cases; independent review closes. No Rust/ABI/wire change
or claimed speed gain. Exact full G0 deployment and native measurement are next.

## Actual e7321504: both full mirrors pass; publication cadence fails LIVE

Exact clean dev/full G0/all394 hashes run20261009-213011-14ba4f-native-host,
source715eee6b-ab72-4e4b-b15d-498d662be1a1, verifies both49-component entity
mirrors create and complete model/whole-scene readback on BOTH clients. The
original FollowCamera11 bounds now pass. MirrorReady is observed around80s;
this does not prove an owned player view, visible pixels, input or gameplay.

Authority accepted frame2/3/4 publication times71.932/78.088/84.891s leave
6.763/6.156/6.803s gaps. The existing LIVE2s publication watchdog faults around
86.7s with `native simulation stopped publishing`; clients also reject stale
applied frames. First native_render is6.588s, first publish0.693s. Measure the
recurring capture stages before optimizing; retain the2s watchdog and250ms LIVE
freshness. Client retirement-probe refusal also causes post-MirrorReady travel
and asset reload; investigate independently rather than assuming one cause.

Full G075 Lua/1364 Rust79 binaries, clippy0 errors/16 warnings, strict1029
focused checks and independent review pass. All four original game processes,
two original CDBs and helpers are gone, all20 save hashes unchanged, no fresh
dump. Both initial game windows are physically contained on the secondary
display. Exact actual events, native logs, capture and cleanup are preserved in
the ignored run and native_operator_summary.json. No release acceptance claim.

The next reviewed checkpoint adds one bounded first-attempt capture diagnostic:
at most98 component rows plus a frame summary, with original admission/static/
dynamic/final stages and nested guard/find/PE timings. Rust preparation/copy,
scene encoding and aggregate proof have separate timings. No native caller
probe, production roster bound, getter, proof or timeout changes. Counters only
observe the existing calls; nested measurements are not additive to stages.

The separate retirement-only fix retains prior-success/same-world/original-class
census absence and admits expiration of an ORIGINAL positive weak serial only
after native slot serial mismatch plus NULL original resolution. Repeated pure
original world/class/slot closure follows every callback. Never read the expired
or replacement actor; serial-zero reuse, unchanged-serial pointer change, listed
address, unknown APIs and changed world/class remain refused. Immediate retire
and live actor_scope remain strict. The matched weak getters prove mismatch
returns NULL before object access; no serial allocation is used.
Strict provider/fixture1163 focused checks and Rust presentation10 checks pass;
both independent production reviews close. Actual expiry and stage timings are
pending the exact gated next run; neither is a gameplay or speed claim.

## Actual 5ee665e6: FollowCamera fails immediately after world setter

Exact clean dev/full G0/all394 hashes run20261009-210901-4a9dbb-native-host,
source051d9608-9d90-4db8-98e8-e6127306a150, localizes BOTH first-entity failures
to stage=world_set, component11 kind6 FollowCamera, parent6 Shoulder SpringArm,
socket=None. Maximum position delta211.51344970236539, sign-aware quaternion
component delta0.37686964143071849, scale delta0; existing sign-aware quaternion
L1 error0.53150414919121891. Times70.935s/72.183s. No tolerance is changed.

Current arm replay sets parent world transform before writing its cached endpoint
at2F0/310 and does not publish that cache change to children. Matched native
UpdateDesiredArmLocation143CBB780 writes those fields143CBC38E-3B6 and then
calls UpdateChildTransforms143BF7370(this,0,0) at143CBC3BD. This omitted native
step is restored with complete owned-child/world/slot guards. The proposed
unchanged-relative setter early-out is not proved: preserve that uncertainty.
Exact world-readback closure remains pending actual verification of the complete
native publication sequence. Primary ASM is retained in this ignored run.
Keep source parent/socket attachment and original transform bounds.

The three-file correction retains complete owned parent/socket/child-array
witnesses and OwnerOuter->Level->World proof around native publication. Check
original array headers before reading retained slots; reject foreign, replaced,
renamed, garbage or changed attachments. Retain publication evidence for later
callbacks and whole-mirror finish. Strict provider/fixture1029 focused checks
and independent production review pass. No ABI/wire/setter/tolerance changes.
Actual corrected camera/complete-scene readback remains pending.

Full G075 Lua/1364 Rust79 binaries, clippy0 errors/16 warnings pass;994 focused
diagnostic checks and independent review close. Both first mirrors create49
components, but complete scene/owned view/input remain unverified. All four
original games/two CDBs independently absent, save20 hashes unchanged, no fresh
dump; both initial game windows physically contained on secondary. Exact deltas,
native logs and cleanup are retained in the ignored operator summary.

## Actual 9676db66: first full mirror creates; transform readback refuses

Exact clean dev/full G0/all394 hashes run20261009-205648-6725b5-native-host,
source87d24c9f-2157-447a-ae7a-b33038661fdb, verifies the qualified asset-serial
fix advances past Hosen and all49 components of the first entity1 mirror on BOTH
clients. NATIVE_CREATE operation1 terminal marker128 reports create edge=exit,
component49. Rust creates/applies one entity at a time, so the second entity
mirror is not certified by that first completion.

Both then refuse first-entity apply with mirror complete readback: mirror world
transform readback failed at74.208s/74.184s. Component ID and actual deltas are
not yet recorded. Apply already uses parent_order and publishes parent skeletal
poses before children; no naive ordering correction is justified. Matched native
setter/getter Transform96 layout is confirmed. Preserve original position0.001,
quaternion0.00001 and scale0.00001 comparison bounds while localizing the fault.
No all-mirror readiness, visible owned view, input or gameplay parity proof.

The reviewed next diagnostic preserves close()/parent_order and adds an immediate
world_set checkpoint plus the original complete checkpoint. It reports copied
component ID/kind/parent/socket and raw f64 position, sign-aware quaternion and
scale deltas; no new query follows fault detection. Strict provider/fixture994
checks and independent review pass. Cause remains pending the next actual run.

Full G075 Lua/1364 Rust79 binaries and clippy0 errors/16 warnings pass. The
asset fix992 focused checks and independent review close. Both original CDB
captures admitted/completed; all four games/two CDBs independently absent,
save20 hashes unchanged, no fresh dump. Both initial windows prove physical
secondary containment. Operator summary/logs are retained in the ignored run.

## Actual 584792a7: native serial assignment mistaken for asset replacement

Exact clean dev/full G0/all394 hashes run20261009-203529-352d72-native-host,
sourcec81c581a-2696-4f43-913b-396407d29198, proves BOTH Hosen003 refusals happen
immediately after SetSkeletalMeshAsset: address_equal=1, index_equal=1,
zero_to_nonzero=1. Times71.026s/70.974s. The expected zero-serial handle and
returned positive-serial handle refer to the same object and original slot;
this is not evidence of a different mesh asset. The earlier actual=different
diagnostic represented strict weak-handle inequality as well as pointer inequality.

Pinned native assignment1414DE830 uses original UObject index+0C and allocator
1414B6B90. The allocator assigns the previously zero serial in the unchanged
24-byte original slot; an existing positive serial is returned unchanged.
Primary proof/ASM is retained in run202218-e92cdb. Never invoke the unverified
UE4SS weak constructor/allocator or write engine serial fields.

The private qualified asset correction is implemented and independently reviewed.
Retain original slot/address/full name/class/outer/path/RF and world/owner-level
witnesses. Pin the first naturally observed positive serial after original pure
closure, propagate it into PoseBinding, and retain the original witness through
all later callbacks and owned create/apply/complete-scene finishes. Global same()
stays strict. No engine serial allocation or writes are introduced. Strict provider
and fixture compilation passes992 focused checks, including real replacement,
positive mismatch, rename, class, garbage, Outer, world, slot reuse, dual-alias and
later-callback refusals. The exact deployed correction still needs an actual run;
this diagnostic does not establish a completed mirror or gameplay parity.
All four original games/two CDBs independently absent, save20 hashes unchanged,
no new dump; both initial windows prove secondary containment. Full G075 Lua/
1364 Rust79 binaries and clippy0 errors/16 warnings pass. The diagnostic969
focused checks and independent review pass; extended verdict remainsfalse.

## Actual 3c670480: render mesh mismatch isolated to clothing

Exact clean dev/full G0/all394 hashes run20261009-202218-e92cdb-native-host,
source207d594e-852e-4ae1-8325-d5bc882ea297, reproduces the same safe refusal
on both clients. Components4,7,8,10 pass native pose binding. Component15,
NODE_AddSkeletalMeshComponent-13, has67 source bones and requests the full
SkeletalMesh path /Game/Assets/Clothing/Hosen/SkeletalMeshes/
SK_Clothing_Hosen_Standard_003.SK_Clothing_Hosen_Standard_003.
The render component's GetSkinnedAsset returns a different admitted identity;
the calculator check is not reached. Refusals occur69.762s/69.879s.
Both fail while creating the first entity1 mirror. Entity2's corresponding
component uses Standard_002. Qualified source captures and native bulk checks
agree with each exact recipe; cooked Hosen blueprint sets SkeletalMesh and
SkinnedAsset to the same package export. No source path collision is found.

The preceding e0b80 run200724-463c15 proved the corrected suspension check
passes those first four populated components, then reached this same component
with an undifferentiated mesh refusal. The narrow 3c670480 diagnostics preserve
the original getter and exact identity requirement. Strict provider compilation,
967 focused assertions and independent review pass. Full G0 passes75 Lua and
1364 Rust tests in79 binaries, clippy0 errors/16 warnings. Runtime cause of the
different clothing mesh is still unproved; do not accept substitutes or report
completed mirrors, owned views, input, gameplay parity or release readiness.

Both original CDB captures admitted and completed; original four games and two
debuggers independently absent, save20 hashes unchanged, no fresh dump. First
owned-window observations prove both clients physically contained on the
smallest secondary display. Evidence and native_operator_summary.json are
retained in the ignored run directories. Extended diagnostic verdict staysfalse.

The initial e0b80 pre-push G0 encountered parallel-test interference in
mode_records_go_to_capable_peers_on_change_and_periodically: process-wide
capability retention can remove another independent test's peer entries.
The unchanged exact test and complete serial G0 pass. RUST_TEST_THREADS=1 is
used for these full gate executions; no assertions or product bounds changed.

## Actual e132: safe pose refusal; suspension proof corrected

Exact clean dev/G0/all394 hashes run195159-f0d052/source0e63df57-0388-4b55-
b15b-a40a76789f42 publishes66.617s. Captures21.502s/21.193s remain in prior
19-22s range; no startup speed improvement is established. Full49-component/
600-bone/38-material recipes, all four harvests6243 reads/12844 qualifications,
49 parent hops/4 mesh censuses are retained. Runtime factory counters are absent.

Both original-client CDB captures admitted. Client2PID46104 reaches component4
pose_bind69.789s, then safely returns the combined render/animation/cloth refusal
instead of the old SetLeader AV. Whole creation remains incomplete; no rendering,
view, input or combat parity proof. All4 games/2 original CDBs gone, helpers0,
save20same, no new dump. First window observation proves secondary containment;
later observation occurs after owned teardown and does not erase the first.

Native SuspendClothingSimulation143C0EFA5 ORsA52/08; its getter143C0C325 reads
that same bit. Pose and prior empty-skeletal checks had instead required the
separate reflected bDisableClothSimulationA42/04. Correct the private suspension
snapshot/guards/fixtures while retaining original A42 source observations and
all existing inert/whole-set checks. Split combined errors into exact bounded
reasons. Strict provider object/958 focused assertions and independent review
pass. The next actual run must distinguish any remaining guard failure.

## Replacement and startup checkpoint under verification

The source scalar tail is implemented in four files and independently reviewed:
explicit optional boolean mode retains complete Scope.resolve and the same final
pure path/hard-link/admission closure, releases STATE before callbacks and restores
only its original sequence. Default fresh pre-getter userdata remains unchanged.
Only discarded post-getter wrappers are omitted. Rust44/Lua317/syntax3 pass;
actual startup improvement is not yet measured.

Native pose replacement keeps the hidden Poseable calculator unlinked from the
visible Skeletal renderer. Preserve ComponentSpace enums (socket2/setter1).
Pinned Poseable AB0 refresh publishes its complete engine-calculated CS buffer;
copy all elements into the renderer's existing editable buffer, set the native
publication bit, call Skeletal AF0/finalizer and native bounds/render/transform
notifications. No engine array allocation/resize, leader spoof, fallback pose,
wire change or removed bone/morph/material/hidden readback. Component ticks are
explicitly disabled. Original class/asset/world/owner, buffer allocation and
publication/inert state are checked around calls and after the whole batch.
Strict actual provider object compilation and952 focused native assertions pass;
independent final review is closed. Primary native proof/ASM is retained under
191021-69d625/creation-pose-transfer-proof.md with the original image SHA256.
An exact deployment/game run remains pending; no native rendering parity claim.

## Reactive loading follow-up

User reports asset counts briefly visible and the bar completing before exit.
Loading now paints changed actual counts immediately; cosmetic waiting dots/
QPC elapsed seconds are capped2Hz. Final nonempty asset preparation yields once
with the existing known pending reason after setting Creating player models,
giving the outer UI tick a paint boundary before native creation. Next apply
rechecks original world/current scope; generation changes restart preparation.
Only that exact pending reason preserves the model stage. Unknown work remains
marquee; no receipt refresh, camera shortcut or fake percentage. Focused
loading58/client98=156 assertions and syntax5 pass. Actual new UI appearance
remains pending deployment; this does not fix the native creation defect below.

## Actual creation fault identified: skeletal follower with poseable leader

Exact9b2721d9/all394 deployed hashes/full G0 are used for bounded extended
diagnostics. Run20261009-185331-da3331-native-host publishes60.445s; client1
PID39360 enters presentation63.384s and exits3. Marker70 enters component4
kind0 after the explicit first32-PE limit. Run185828-2e5ae8 reproduces the
same marker/exit on client2PID40256. A separate190121 run exits3 during travel.
No specific call is inferred from these capped markers alone.

Ignored early CDB capture attaches only to original held client identities,
uses local symbols, preserves first/second-chance exception disposition, caps
stacks and automatically detaches. Initial ambiguous exit symbols and echoed
guard false positives are corrected; those captures were refused, not certified.
Run190839 produces a separate shutdown dump19_09_47.2674283, SHA256
37780DE7303E5BC1E813A02427D6D51DEC1B9ABAF8611A3DA4EEC51B43DDB3B0;
the corrected harness detects it. All original game PIDs gone/save20same.

Run20261009-191021-69d625-native-host admits both CDB captures on original
clients35560/7308. Both first-chance AVs read0x8D0 at shippingRVA3C09521.
CPP returnRVA4622E6 is exactly SetLeaderPoseComponent(renderSkeletal,
leaderPoseable,force=true,followerTick=false), proved by the fixed setter
argument string/call disassembly. Native update resolves weak LeaderPoseComponent
at568, casts to SkeletalMeshComponent, sets the result NULL for Poseable, then
unconditionally reads AnimScriptInstance at8D0. SDK/object dump/native class
registration agree. This identifies the creation defect; it is distinct from
shutdown cleanup. A setter-flag change does not guard this path. Replacement
pose-driver implementation remains under native investigation; no parity claim.

All4 game processes and both original CDB processes are independently gone,
helper exits0/complete output/detach verified, original20 saves match, no new
dump in191021. Diagnostic pass remainsfalse and active input remains0.

## Creation fault localization checkpoint

Create-only native diagnostics now emit at most128 markers per operation,
including explicit exhaustion and a reserved terminal marker. Fixed code labels
identify component/kind and the first32 ProcessEvent call boundaries; exit is
logged immediately after the engine call and before post-call guards. No
pointers, recipe data, ABI changes or frame logging. The terminal label owns
its64-byte storage, avoiding a reviewed stack-label lifetime defect. Strict
provider object compilation and936 focused native assertions pass; independent
review closes both this change and the52-check crash-detection harness.
Actual failing client1 call remains unknown until the next exact deployment/run.

## Actual7f2e02dd: startup still slow; separate presentation and shutdown failures

Full G0 passes75 Lua/1364 Rust79 binaries, clippy0 errors/16 warnings. Dev push/
RequireG0 and all394 deployed hashes match7f2e02ddfcccff4713bddadcf7091d37cc38bacf.
Normal20261009-182210-f8587b-native-host/sourcec421b0cd-d8df-4c28-85c9-35475167a5cf
captures21.937s/20.592s and accepts both49-component/600-bone/38-material recipes
39.087s/60.094s. Render starts60.164s and refuses65.642s after original65s
teardown. No complete scene. Loading.tick accepted its native widget operations
before Core travel; no widget error, but pixels were not captured. Both actual
ViewTargets identify hidden PlayerControllers, not owned mirror actors. Both
UnrealWindow observations are physically secondary. All4PIDs gone/save20same.

Root then uses independently reviewed IGNORED extended diagnostic scripts,
keeping the original65s target separately and unconditionally forcing pass=false.
Readiness observation may continue120s/authority180s; live observation stays10s.
No product, freshness, physics or source guard changes. Extended
20261009-182736-4d7e7f-native-host/source0d830e19-f87b-4191-b031-e1e18d100224
publishes a complete scene68.584s/sample1/frame1. Client1 originalPID41220
prepares assets across ticks, enters native_present70.838s/wall17:28:47.744UTC
with ownEntity2/incarnation5/dir7, then exits known3 observed17:28:48.439UTC.
There is no provider-exit marker or attributed error/stack yet; exact failing
engine call remains unproved. Client2 PID6596 never enters native_present and
stops by request17:28:48.632UTC. Source cleanstop82.017s; active inputs0.

The user supplies a Fatal Error screenshot naming
crash_2026_10_09_18_28_48.9013862.dmp. The34,565,637B dump is preserved with SHA256
DCE5CBF731D182FC1B7AF415E235F67B5B42369B7F1B242EDFC3660170A3D207. Positive
MINIDUMP_MISC_INFO flags0x3f7 identifyPID6596/CLIENT2; header17:28:49UTC agrees.
C0000005 NULL write at shippingRVA119C415 occurs on thread45652 through CRT
exit/onexit callbacks. It does NOT explain client1's native_present exit3.
The bottom shipping return12CBF84 follows exit(777003) through the positively
resolved ucrtbase IAT, so this is an engine error-exit cleanup stack, not proof
of an ordinary graceful-quit failure. Code identifies getter1177FC0/global
8ADCCC0 and an onexit clear-writer6B03CD4, but that global is absent from the
partial dump: the clear-writer execution and original error remain unproved.
Matching local/deployedDLL/PDB and bounded CDB evidence are retained under
run/dump-attribution-analysis.md. Do not attribute this dump to client1.

Independent all4 originalPIDs absent/Shipping0/save20same. Legacy new_crashes[]
checked only Saved/Crashes and MISSED the UE4SS dump, so it cannot establish
crash-free teardown. Root now checks both directories and UE4SS crash_*.dmp;
focused harness52 checks pass, including new-dump detection without an engine
crash directory. Normal harness also fails immediately on stopped/error clients.
Next bounded creation/PE checkpoints localize client1; native teardown analysis
independently traces client2's cleared global. No mirror/view/input parity proof.

## Current follow-up: visible loading, capture admission and transport

The user now explicitly requests an opaque loading screen with honest progress
instead of a broken world, and an optimal transport review for8+players.
Loading implementation runs alongside the narrow admission fix. Unknown server
preparation uses an indeterminate bar; asset progress uses actual completed/
total work. Travel/world changes reset scoped widgets/data. Complete models
and a separate positive owned-view proof are required before exposing gameplay;
current LIVE/mirror readiness does not prove a camera. Errors remain visible
until the original operator stop request, with no post-stop inputs.

Root transport flush now permits4 records/peer/INVOCATION in fair rounds,
blocking each peer immediately on queue refusal and preserving accepted-only
cursor advancement, original pinned token/ACK and all existing record bounds.
This is per flush, not per60Hz tick: other dispatch paths also flush. Independent
code review finds no stream correctness blocker. Focused pinned-stream and
authenticated UDP delivery checks pass; the latter transfers125362B in265ms
including cold metadata, offline only. One targeted8-entity full-data fixture
preserves8192bones in661499B/11parts, exact encoded bits after reorder. This
proves codec capacity, not8native players, latency bounds or gameplay scaling.
Full transport architecture review remains separate from this bounded change.

Loading implementation and independent review are closed: loading41/client97
assertions and syntax6 pass. One exact cooked asset is prepared per tick under
the original scene generation/world; only its specific pending reason waits.
The fullscreen opaque viewport widget uses actual completed/total assets and
marquee for unknown preparation. It remains opaque without positive owned-view
proof. Widget errors stop Core/input after UI unwind, retain the error loop,
back off1s/max3 failures per world and log only changed errors. Owned stop quits
once. Removal failures retain their original widget and use the same fatal
contract. World drop forgets old UObjects and resets ready/count/retry scalars.
Actual widget appearance and player meshes remain pending the combined run.

Transport audit is saved in ignored test-results/native-transport-design-20261009.md.
Keep authenticated UDP/native authority, but separate reliable control/recipes,
identified input edges/deltas and deadline-driven LIVE snapshots. Current LIVE
Sender waits per-batch admission/completion ACKs; started ReliableLatest fragment
work shares a256KiB in-flight window and can hold newer pending frames. These
are code-proven risks, not this run's measured failure. Eight stress-fixture
frames at60Hz imply39.7MB/s/client before headers versus16MiB/s congestion max;
this is a capacity calculation, not an actual workload. Raw-bit delta/full
recovery and bounded retirement need actual consecutive complete-frame replay
before implementation choices are certified. Mouse axes2/3 are consumed
deltas and seven actions are press/release pairs: do not blindly change all
inputs to Latest or discard/double-apply edges. Native ownership, original
sample times, full reconstruction and MirrorReady remain required.

The next client isolation observation includes diagnostic-only actual ViewTarget
name/class/full name/hidden flag, from pinned Engine.hpp GetViewTarget methods.
Unknown results stay unknown; these copied facts do not establish a camera,
identify pixels, change isolation acceptance or release the loading screen.

Native batching implementation is frozen and independently reviewed. Source
scope39 focused tests pass, including last pure owner-field directory mutation
before Begin publication. Pure original identity/pointer/Outer-path reads keep
all object checks without nested admission; callback GetWorld/GetOwner/find/
name/schema/factory paths and operation return boundaries freshly admit.
No admission ticket survives callbacks or operations. Actual performance is
pending the combined loading-screen checkpoint and next exact-G0 deployment.

## Latest actual: 98e25c02, fresh wrapper works; startup still too slow

Full G0 passes74 Lua/1363 Rust79 binaries, clippy0 errors/16 warnings and142
event emitters without violations. Dev push/RequireG0 and all393 deployed
hashes match98e25c02234e6f60f3182cb3600fe0e401acee75. Actual
20261009-174612-e7926a-native-host/source2e9de2ac-c828-4f03-a769-18ddcd9fb9f2
uses supervisor24948/authority32788/clients43940,46776. NativeReady15.325s.

Fresh native wrappers work through both complete recipes without a factory
refusal. Entity1 capture16.174→37.519s/21.345s, bind37.797s/278ms; entity2
capture37.798→58.433s/20.635s, bind58.738s/305ms. Both retain49components,
600bones/38material slots; compact35551/35545 and rawJSON74625/74698 bytes.
No actual capture speed improvement is established from this run. Native
render58.822→65.126s/6.304s succeeds; publication refuses final generation
65.805s after unchanged65s client teardown. Source cleanstop66.723s. No
published scene, client phases, mirrors, owned view, input or pixel proof.
Both clients stop cleanly; original pipe drains complete0B without errors.

V2 operator observations16:46:32.346UTC and the later snapshot independently
qualify both actual visible UnrealWindow handles by original process/class/
thread, exact880x527 outer placement and positive DWM physical containment on
the smaller secondary display. Console/debug handles are recorded separately.
History and immutable snapshots are retained; no OS input or activation.
Independent all4 originalPIDs absent/Shipping0,20/20 saves unchanged, no
crashes/unobserved children. Exact692-line authority trace/fullNative and
operator summary retained. Next optimization must follow measured remaining
native guard/capture costs, preserving all identity proofs and full data.

Measured nested stages: entity1 mesh6.738s/parent1.283s/static10.983s, including
bones2.434s/material2.116s/vertexproof2.019s; entity2 mesh6.357s/parent1.314s/
static10.814s including bones2.288s/material2.126s/vertexproof2.032s. Both
harvests retain6243 field reads/12844 qualifications/49 hops/4 mesh censuses.
Each fresh qualifier calls full Scope.resolve before and after allocation;
native Runtime.verify separately admits before and after each pure identity,
with repeated directory locks/clones and three base-identity/native-world
checks. Next bounded batching shares admission only within proven callback-free
scalar regions, retaining original per-object checks, full callback brackets
and final generation closure. Net implements; Host independently reviews.

## Fresh-wrapper capture correction, implementation under review

The Lua capture now consumes a fresh native UObject wrapper returned beside the
already qualified original address. It removes per-field StaticFindObject and
name conversions without caching wrappers across callbacks or omitting data.
The native bridge registers through the pinned UE4SS dispatcher, which supplies
the calling coroutine's actual Lua context; allocation/postcallbacks stay inside
a protected call. Rust releases its scoped-state borrow across that call and
rechecks the original object, path, links, roster and generation afterward.
Native source-scope36 tests and protected bridge137 checks pass; standalone
bridge and actual provider/dllmain compile with /W4 /WX. Independent native
review is closed, including nested reentry fault propagation and protected
postcallbacks. Source files are frozen; build ownership is released.

Lua source312/worker370 assertions and syntax2 pass; independent Lua review is
closed. Fixtures require distinct fresh wrappers, trap old-wrapper reads and
hot path searches, preserve the complete data/binding signature and refuse
identity/name/class/path/owner/link/flags/world/generation/constructor changes.
No ABI/schema/wire/capacity/timing changes. Actual speed and visible models
remain unproved until the next exact-G0 deployment and two-client run.

The ignored window helper V2 is parsed/compiled and root-reviewed: it selects
only the unique original visible UnrealWindow, records console candidates
separately, forces880x527 without activation, checks DWM physical bounds and
preserves every observation. Earlier MainWindowHandle readbacks prove only
placement of that selected handle; historical claims of both actual game
windows being fully secondary are not established by those older records.

## Latest actual: 0c085a97, deadline before source publication

Full G0 passes74 Lua/1363 Rust79 binaries, clippy0 errors/15 warnings,142 event
emitters without violations, dev push/RequireG0 and all393 deployed hashes match
0c085a970a6bca4b2a5f7ae3745e9c555a70a215. Actual20261009-171718-59e0be-native-host
uses source70890f4b-3d7b-4664-9a03-ad9123332c82, supervisor5128/authority33208/
clients17292,24916. NativeReady14.917s; roster15.851→15.889s acknowledges dir7.

Both full49-component/600-bone/38-material recipes pass again. Entity1 captures
15.916→37.979s/22.063s, NativeBind38.300s/320ms. Entity2 captures38.301→59.217s/
20.916s, NativeBind59.501s/283ms. Core71ms succeeds, render59.573→64.998s/5.425s
succeeds, then publication65.683s refuses source vertex final generation after
unchanged65s client teardown. No complete source scene; sourcecleanstop66.752s.
Source capture remains too slow/variable for robust readiness. The previous
17s capture result must not be treated as a guaranteed startup bound.

Neither client unexpectedly exits or enters presentation; both own streams
remain wait_scene then clean stopped. All original stdout/stderr drains report
complete true,0B, no copy errors. Thus the prior exit3 is not reproduced and its
cause remains unproved. Startup traces and clock recovery have not run past a
successful publication yet; no model/owned camera/input proof.

Window evidence also exposes an operator-helper defect: first client1
MainWindowHandle was160x39 at minimized coordinates; moving that handle does
not prove the actual game window was moved. Client2 initially880x527 then
changed to1133x597 on main and was moved again. The helper's later post-exit
observation overwrote prior JSON. Next helper must qualify exact original
game HWND by class/owner, distinguish console windows, force880x527 and preserve
every observation. Do not claim both visible game windows were fully secondary
for this run. No OS input is introduced.

Independent all4 original PIDs absent/Shipping0,20/20 save hashes unchanged,
crash[]/unobservedchildren[]. Exact692-line authority trace/fullNative/pipe
records/operator summary retained. Next work targets the repeated per-field
global object searches with a positively qualified fresh native wrapper,
retaining both complete harvests and every identity/timing/data guard.

## Latest actual: 8b775c5d, first complete source scene

Full G0 passes74 Lua/1363 Rust79 binaries, clippy0 errors/15 warnings, dev push
and clean RequireG0 deployment; all393 hashes match8b775c5d780f43ac87689351c9ae9e8fc446f79d.
Run20261009-164645-8fb437-native-host uses supervisor6844/authority2656/clients
23416,11364, source6ff89559-bdd2-4e8f-b12b-a160f9f2c227. First original
PID/start/exe/HWND helper fits both880x527 windows
fully secondary. NativeReady16.399s; roster2 facts17.016→17.051s, dir6→7.

Entity1 captures17.079→34.240s (17.161s), NativeBind34.498s/258ms succeeds;
entity2 captures34.499→51.302s (16.803s), NativeBind51.536s/234ms succeeds.
Both keep49components/600bones/38materials in BOTH harvests, dir7/incarnation5.
Compact35551/35545 bytes, rawJSON74730/74725. First parent closure1.064s versus
5.593s previously, measured reduction4.529s. Total first capture improves8.436s;
other stage changes include run variation and are not all attributed to reuse.
Exact fewer-keep count is fixture evidence only; no runtime keep counter exists.

Canonical core51.537→51.610s/73ms succeeds; native render51.610→56.954s/5.344s
succeeds; complete canonical publish56.954→57.545s/591ms succeeds. Source
evidence57.548s reports sampled1/refused0/frame1, active inputs[0,0]. This is the
FIRST complete published source scene, not verified client delivery or meshes.

Client1 original PID23416 exits with known code3, observed15:47:44.557UTC before
live verification. Its own last event55.381s is wait_scene; client2 last55.525s
also wait_scene. Native logs for both clients contain only startup registrations;
shared UE4SS has no attributable fatal/assert/panic/stack. Engine logs are absent
or empty. Exact exit cause remains unknown; next harness captures original client
stdout/stderr with async drains and client-only backtraces. No fabricated crash
site or native-create proof. No mirror/readiness/input/visible-model evidence.

Original-client diagnostics implementation is independently reviewed and frozen.
Both output pipes drain through .NET CopyToAsync immediately, retain original
held-process identity, persist processes.json before setup, and complete only
after true EOF/original exit; bounded incomplete cancellation is reported.
Files are outside state directories, with client-only RUST_BACKTRACE=1.
Focused harness50 checks/syntax pass, including saturated524288B pipes each,
final error marker/known exit3, live lifetime and cancellation behavior.

NativeClient startup diagnostics add six edges once per stable original scene
generation: outer presentation_apply and exact native_scene_assets/native_present
dot calls. The distinct x_native_client_phase contract carries only copied
epoch/epoch_text/directory/frame/own reference, api/edge/optional result and
reason bounded256. It does not log pointers/generation bytes, poll a new scene,
change status/readiness/input or refresh receipts. Variadic native results,
nil/false holes and exceptions retain original semantics; an exception leaves
the matching entry as evidence. NativeClient87 assertions/syntax2 pass; independent
review closes21 same-generation frames/retries, new generation and world reset.

A separate confirmed sampling latch follows: worker records pre-Boundary wall
time only on success; after5.935s render/publication its next dt_ms exceeds native
sample.rs0..1000ms limit. Failed attempts never advance that clock, yielding39
sample-step refusals by58.556s. Pose v2 documents this field as PHYSICS step,
with0 explicitly unknown; capture spacing is not physics time. Next correction
separates33ms attempt scheduling from real source timestamps and explicitly
represents unqualified physics step as unknown, without age renewal or clamping.
Native physics-step qualification remains a distinct open fidelity item.

Clock separation is independently reviewed and frozen; worker370/source295
assertions and syntax2 pass. The production-loop regression represents the
actual5.935s render/publish followed by another successful sample, intermittent
failure/retry and delayed roster ACK. No source receipt or scene age is renewed.

Source cleanly stops58.844s; independent all4 original PIDs absent/Shipping0,
20/20 saves unchanged, crash[]/unobservedchildren[]. Exact692-line authority
trace,18-line client traces each, fullNative, original exit record/operator/
window/cleanup evidence preserved. Both original identity guards and unchanged
100/65/10/250ms bounds remain; expensive caller probe stays off.

## Latest actual: 67175710 roster preflight

Full G0 passes74 Lua suites/1363 Rust checks in79 binaries; clippy0 errors/
15 warnings. Dev push and clean RequireG0 deployment succeed; root verifies
all393 deployed hashes against671757100c4a0c96dc589644178e655ed85de858.
Run20261009-162404-9c6e68-native-host uses source0716f305-1fc3-446d-bb75-
427262ec0197, supervisor11288/authority13128/clients13764,6436. First original
PID/start/exe/HWND-qualified helper fits both880x527 windows fully secondary.

NativeReady15.082s. Roster2 facts15.818→15.859s (41ms) acknowledge directory6→7
before all captures. Entity1/incarnation5 captures15.887→41.484s (25.597s),
then NativeBind41.797s succeeds after312ms: compact35551/rawJSON74706/nodes6604/
dictionary8713/tokens26838/components49/bones600/material_slots38. Entity2 starts
41.798s and continues both harvests in the SAME incarnation5/directory7.
No descriptor team publication invalidates it; the roster ordering blocker is
closed in this actual run.

The unchanged65s readiness deadline expires before entity2 finishes. Client
stop.request files are written wall1791559510605; generation refusal occurs
at65.144s/wall1791559510618,13ms later during pass2 SK_Skeleton bone_dictionary
component34. Core.stop closes native host before its later stopped event;
NativeCore::unbind advances incarnation and directory, explaining subsequent
incarnation6/directory8,9. This is teardown after slow startup capture, not a
spontaneous pre-timeout roster change. Native complete frames/mirrors/input
remain0, and visible player meshes/owned camera remain unverified.

Source cooperatively stops66.597s without fallback. Independent original4PIDs
absent/Shipping0,20/20 career-save hashes unchanged, crash[]/children[]. Root
retains exact580-line authority13128 trace/fullNative/operator/window/cleanup
evidence. Next task is evidence-driven capture speed, preserving all model
data, double-harvest stability, original identity and unchanged timing bounds.

Measured nested capture costs (do not add parent totals to child totals):
entity1 render harvests12.377+13.174s; initial mesh census6.995s/final census0.168s,
parent closure5.593s, component static10.641s including bone dictionary2.221s,
materials2.052s and native vertex proofs2.120s. Source signature45ms; native bind
312ms. Entity2 first harvest12.907s/second interrupted10.439s. The first small
speed correction avoids repeated Scope.keep initializations for an already
admitted attachment parent, with fresh incoming identity/path match and fresh
original scope resolution. No dictionary/field loss, fewer harvests or relaxed
guards. Actual performance must be measured before claiming a speedup.

Parent/root reuse implementation is frozen in native_source_render and its
source descriptor fixture. Focused source295/worker356 assertions and syntax2
pass; repeated parent and mesh-root path, FName, class, owner, attachment,
world and garbage mutations refuse. Counterfeit returned path also refuses.
Both adapter harvests retain each component exactly once, with all original
begin/end mesh/spline censuses and bounded phase trace checks unchanged.

The attempted focused development run359e1e0c/20261009-164130-7045d0 verifies
all393 deployed hashes but the native supervisor refuses before starting a
game: native hosting requires a clean deployment with full G0 for its exact
build. No native process/client is launched. This prerequisite remains intact;
root returns to full G0 and RequireG0 deployment before all actual native runs.
Do not bypass the supervisor or falsify its deployment stamp.

## Roster preflight correction under verification

The user reports seeing a sphere with the default grey/dark-grey texture during
these tests, without player models. The latest actual run has no complete scene
or client mirror creation, so this observation is not a verified player spawn.
The sphere's exact native actor/component has not been identified. Model,
material and owned-camera presentation must be observed in the actual clients;
recipe acceptance or network readiness alone cannot close this report.

The source-only native_source_roster_facts API qualifies all original roster
bindings and their native Team Int values before either static recipe capture.
One complete batch is queued against its original epoch/directory sequence;
the network tick applies all facts atomically only while that exact roster
still matches. Native returns the expected acknowledgment sequence under the
queue lock: the base sequence if unchanged, otherwise exactly base+1.

Lua waits for that exact sequence, complete original references, slots,
controllers, ownership and known native teams, then rechecks original scalar
world/pawn/controller bindings. Unrelated newer generations restart preflight;
pending acknowledgment publishes no partial scene. Team zero is valid only
when positively observed. Registration verifies native/recipe/directory teams
before and after copying and has no roster metadata side effect. Existing
capture cancellation, readiness and receipt-age limits remain unchanged.

Independent native review found that checking pawn/controller possession alone
cannot prove the original indexed player assignment after another actor's
callback. Pinned executable evidence closes the lookup semantics:
GameplayStatics exec1435EDFB0 calls1435E1B00; World.OwningGameInstance at1D8
must have native GetWorld1435CF760 equal to that world, then LocalPlayers at38
(getter1411A1310) is scanned for NONNULL LocalPlayer.PlayerController at30.
Indices count nonnull controllers, not raw array positions. GI GetWorld reads
its native context at30 and context World at2C0 (SDK WorldContext size2C8).
The final callback-free census must retain that original native branch, whole
array header/all slots and original local-player/controller links before any
fact enqueue. Exact proof is preserved in native-roster-*.asm under test-results.

Focused Lua worker356/source277 assertions and syntax3 pass. Independent review
of the three Lua files closes the exact ACK/binding/downstream generation
contract. Focused server roster2/native roster5/existing presentation binding8
Rust tests pass; independent native production review closes the original
player-map witness. All seven implementation files are frozen.
Actual accepted recipes, complete frames, mirror meshes and owned views require
the next normal run.

Independent native saved-code review closes the final roster boundary: the
original whole local-player header/slots, dense nonnull controller prefix,
local-player/controller links and original world/game-instance native context
are checked without callbacks after all callback-capable qualifications. The
native lookup/GetWorld pins match the primary disassembly. Sparse/fallback
lookup profiles explicitly refuse; current two-local-player support still needs
the actual run. Later-row regressions replace an earlier LocalPlayer controller
while retaining the old PC/pawn possession links; the final pure census refuses.

## Latest actual: 9d465f79

Native material-type correction passes full G0 (74 Lua suites/1361 Rust checks
in79 binaries; clippy0 errors/13 warnings), dev push and clean RequireG0
deployment. All393 deployed hashes match
9d465f79acbd847eec71e7ff1c44c1a8092c4924. Content identity
007d6d900851cec8cd7d011089816b70f1540755884358e101d0103e41b7f502.

Actual `test-results/20261009-154130-0b06d0-native-host` uses source UUID
72b63a2c-3662-43de-83b3-eba9cb8f8308, supervisor31040/authority33668 and
clients9460/42776. First original-qualified window helper fits/readbacks both
880x527 windows fully secondary. Entity1 epoch5503944532568704591/incarnation5/
directory6/revision1/frame1 capture16.215s to41.911s (25.696s). NativeBind
42.353s/seq913 succeeds after0.441s with compact35551/rawJSON74731/nodes6604/
dictionary8713/tokens26838/components49/bones600/material_slots38. This is the
first accepted complete native recipe, not a published complete scene.

Actual material layers close the prior type/flag blocker: Body component7 slot0
22.944s–23.035s and Head component8 slot2 23.281s–23.328s both report exact
MaterialInstanceDynamic/dynamic=true/RF_Transient=false. Armour components15,
16,17 have dynamic=true/RF_Transient=true. All five depth1 layers complete in
both harvests. Eight new copied scalar observations account for nodes6556→6604.

Entity2 starts42.354s and refuses42.510s with `source scope entity generation
changed`. Exact producer is Rust Runtime::admit, before native original-object
checks. host_describe queues recipe.team, which is the separately read live
pawn Team Int. native_glue tick changes unknown team to known and publishes
directory6→7; Bridge clears descriptors tied to6. Lua continues its original
directory6 snapshot for entity2, correctly refusing. Next loop recaptures
entity1/revision2. Do not relax the scope guard or retag old native bindings.

The next coherent correction qualifies and publishes ALL original-roster team
facts atomically before expensive recipe capture, waits for exact known-team/
reference acknowledgment, then captures/binds/samples one fixed generation.
Descriptor admission becomes team verification, without roster side effects.
Sol implementation/review is in progress. No complete canonical frame, native
mirror or active input; both clients still fail unchanged65s readiness.

Source cooperatively stops66.532s/seq1676 without fallback; supervisor confirms
clean stop. Independent all4 original PIDs absent/Shipping0, all20 career-save
hashes unchanged, crash[]/unobservedchildren[]. Full Native and exact580-line
authority33668 trace plus original-window/cleanup evidence are preserved.
Owned client view integration remains open; visible bodies/gear still unproved.

## Previous actual: ddccc175

Precise material diagnostics checkpoint passes full G0 (74 Lua suites,
1361 Rust checks in79 binaries; clippy0 errors/13 warnings), dev push and
clean RequireG0 deployment. All393 deployed hashes match
ddccc17506d0ea971d92b5118ffed413a8637e95 before launch.

Actual `test-results/20261009-150349-520cf5-native-host` uses source UUID
bf4912c5-2109-41d3-9cd3-7f3c2e6278eb, supervisor14908/authority15264 and
clients32796/15500. Original process identities and both880x527 windows are
qualified and fit/read back fully secondary. NativeReady15.407s; entity1
epoch text-4884128863321254425/incarnation5/directory6/revision1/frame1
captures16.079s to40.190s. NativeBind40.194s refuses body CharacterMesh0,
component7/material0/slot0, field=base. Exact copied base:
`/Game/Maps/Arenas/Map_Arena_Yard.Map_Arena_Yard:PersistentLevel.Willie_BP_C_2147482004.CharacterMesh0.MID_M_Body_Inst_Frank_21`.
It has134 bytes and a colon; copied scalar/vector/texture counts are0.
Plan compact35316/rawJSON74312/nodes6556/dictionary8771/tokens26545,
49components/600bones/38material slots. Full registration reason is retained
in native evidence/operator summary beyond the512-byte phase trace.

The material collector traverses parents only while RF_Transient is true;
a runtime-owned material remaining at the end of that traversal is not a
cooked asset. Next correction qualifies the native dynamic-material type
independently of that flag, captures every override layer and resolves the
original immutable base. Native class/create/dataflow proof is required;
do not admit a map-owned path or substitute a material. The diagnostic
asset_transient=false reports substring absence, not the native RF flag.

The type-driven correction is implemented from pinned native/cooked evidence
in native-material-instance-proof. Every supported dynamic layer retains its
exact parameters and child-first precedence; original slot/parent/full FName
and complete parameter-array headers are rechecked after callbacks before
borrowed reads. Native empty skeletal null slots remain unchanged. Focused Lua
source277/syntax2 and independent Sol review pass, including same-material
reallocation with zero stale reads. Source timing, class/RF/layer observations,
accepted descriptors and native mesh creation still require the next actual run.

Both clients fail unchanged65s readiness/wait_scene. Accepted recipes,
canonical/mirror frames and active inputs remain0; no player meshes were
created. Owned camera/view integration remains separately open. Source
cooperatively stops67.551s/seq929 without fallback; supervisor confirms clean
stop. Independent original4PIDs absent/Shipping0, all20 career-save hashes
unchanged, crash[]/unobservedchildren[]. Full Native and692-line authority15264
trace, complete copied refusal, window evidence and cleanup are preserved.

## Previous actual: f13e9233

Independent live armour semantics checkpoint passes full G0 (74 Lua suites,
1358 Rust checks in79 binaries; clippy0 errors/13 warnings), dev push and
clean RequireG0 deployment. All393 deployed hashes match
f13e9233b5620be381702413391b071248eeca36 before launch.

Actual `test-results/20261009-144122-23a921-native-host` uses source UUID
75f67dbe-95e3-4109-adea-de54f4a201a6, supervisor39032/authority27816 and
normal clients43408/42256. The first qualified original PID/start/exe/HWND
window operation fits and reads back both880x527 windows fully secondary:
(-1760,0,-880,527) and(-880,0,0,527). NativeReady15.226s.

Entity1 epoch text-8881493164514497873/incarnation5/directory6/revision1/frame1
completes both49-component captures40.812s. NativeBind40.815s now passes the
independent armour-slot observations and refuses `render material`. Copied
prevalidation Plan compact35316/rawJSON74312/nodes6556/dictionary8771/
tokens26545/components49/bones600/material_slots38. The rejected material's
component, slot and exact predicate are not emitted yet. No material semantic
change is justified without those values. Next diagnostics locate the first
failed copied component/material field while retaining existing validation.

Precise diagnostics are implemented and independently reviewed: component id,
material index/slot, the exact failed predicate, parameter identity/native index
and bounded escaped asset labels precede descriptive labels. Asset predicate
flags expose faults beyond the quoted path prefix. The existing native-empty
skeletal null-material exemption and validator bodies remain unchanged.
Focused descriptor25/raw parser12 checks pass. This establishes observability;
the actual material values and semantic correction await the next exact run.

Both clients remain wait_scene and fail unchanged65s readiness. Accepted
recipes/canonical frames/native mirrors/active inputs remain0; entity2 and
visible bodies/gear are unproved. The player view path separately lacks owned
client view-target and effective camera-state integration; mirror readiness
alone cannot establish usable presentation. Native camera work must preserve
actual selected final view, projection and post-processing rather than guessed
defaults. Cooked fringe/vignette writes are an ArrowTime effect; injury-specific
camera behaviour has not been established by that branch.

Source cooperatively stops67.582s/seq929 during the replacement capture; the
supervisor reports clean authority stop without fallback. Independent all4
original PIDs absent/Shipping[],20/20 original save SHA256 unchanged,
crash[]/unobservedchildren[]. Full Native log and exact692-line authority27816
trace are preserved with independent cleanup and live window evidence. This
closes the actual armour-slot refusal, not scene or gameplay acceptance.

## Previous actual: 0b9301bd

Combined construction armour/Lua sizing/cancellation and test-global isolation
checkpoint passes full G0: 74 Lua suites, 1358 Rust checks in79 binaries,
clippy0 errors/13 warnings. It is pushed to dev and cleanly RequireG0 deployed;
all393 deployed hashes match0b9301bd62447195157d569e590cac09091ed212.

Actual `test-results/20261009-140709-303c3a-native-host` uses source UUID
a9899fb6-d2db-434b-8acd-ce76714274fd, supervisor32796/authority22152 and normal
clients35688/42556. Both original PID/start/exe/HWND windows are qualified
early and fit/read back fully secondary: (-1760,0,-880,527) and(-880,0,0,527),
both880x527. The earlier window containment blocker is closed for this run.

Entity1 epoch4662431686152274015/incarnation5/directory6/revision1/frame1
completes both49-component captures at40.238s. NativeBind refuses at40.244s:
`armor slot binding table=equipment.armor row=0 slot=12 pslot=2`, exact class
`/Game/Assets/Armor/Blueprints/Modular_Armor/BP_Armor_Modular_Core_Body_Doublet_Arming.BP_Armor_Modular_Core_Body_Doublet_Arming_C`.
Copied typed planned sizes are compact35316/rawJSON74310/nodes6556,
dictionary8771/tokens26545/components49/bones600/material_slots38. These actual
measurements close unknown first-entity sizing; semantic registration still
fails before canonical/native binding/publication, so no scene delivery,
MirrorReady, active inputs[0,0], entity2 size or gameplay parity is established.
The next correction requires exact Doublet actor-slot/passport-slot dataflow and
raw source field proof; do not rewrite either observed value to force equality.

The unchanged65s readiness gate fails. New cooperative cancellation is proven
in actual source collection: seq929 stopped at67.708s, reason stop requested,
during the replacement generation's mesh census. The supervisor reports
`Native authority stopped cleanly`; no owned stop fallback. Independent all4
original PIDs absent/Shipping[],20/20 original save hashes unchanged,
crash[]/unobservedchildren[]. Full Native log, exact692-line authority22152
trace and live window evidence are preserved. Capture/delivery/pose/body/gear
playability gates remain open; this is an actual cancellation/window milestone.

The exact live Doublet refusal is now corrected from complete native/cooked/raw
field evidence. Source Slot_30 maps the SDK enum byte at0x78 and the pinned
UE4SS getter reads the raw u8. Exact Doublet CDO actor slot NewEnumerator1 is12;
its child construction script changes team colours and calls the core super,
without slot normalization. Base passport default NewEnumerator0 is2; the core
preserves that passport Slot. Live Map_Add independently uses actor Armor Slot
and the current passport. Both actual12/2 observations therefore remain exact.
Remove only their unsupported equality; live class, enum/order/full-field gates
remain. Focused descriptor23/parser11/Lua261/syntax2 and independent Sol review
pass. No actual canonical/native mirror success is inferred before the next run.

The user reports a grey checkerboard sphere and no player models during tests.
Current actual clients stayed wait_scene, so the native Presentation factory
never created player/gear mirrors or a placeholder sphere. Per-client isolation
proves GameModeBase/PlayerController and original Willie hidden/collision0/simfalse
after UnPossess. The sphere's exact creator is unproved by these records. Source
Aim Spline Scene sphere was authority-only and never published. The next actual
must show real skeletal bodies and gear on both clients; sphere-only presentation
cannot satisfy playability. This concern is preserved alongside native readback.

## Previous actual: 5339682b

The complete lossless codec/stream/lifecycle checkpoint passes full G0: 74 Lua
suites, 1355 Rust checks in 79 binaries and clippy with 0 errors/13 warnings.
It is pushed to dev and cleanly RequireG0 deployed; all 393 deployed hashes
match HEAD/binaries/G0 `5339682ba8c33a0385666c575be3a613628ce04b`.
The first gate's sole test-only Instant arithmetic lint was corrected with
checked_sub and its existing stale-scene assertion; the required full retry passes.

Actual `test-results/20261009-133011-adbf6a-native-host` uses source UUID
96ce45e0-0c7b-464f-bdc9-991de1e44e01, supervisor 39560, authority 36552 and
normal rendered clients 31508/44272. NativeReady is 15.668s. Entity1 original
epoch 6164706162233355395/incarnation5/directory6/revision1/frame1 capture
runs 16.273s to 39.031s (22.758s). Both complete 49-component harvests pass,
including all four empty skeletal holders. NativeBind refuses `armor slot
binding` at 39.035s. Encoding statistics validate first, so actual compact/raw
bytes and the 512KiB encoded capacity remain unproved; the old Lua-to-JSON
staging refusal is no longer first. Entity2 capture, canonical/mirror frames
and active input [0,0] remain unverified. No scene reached delivery, so this
run cannot establish network latency or justify changing the part scheduler.

The unchanged 65s readiness gate fails. Both clients stop; the authority begins
another capture after directory replacement and misses graceful shutdown. Its
exact owned supervisor fallback stops it. Last own phase is seq1338 at75.108s,
mesh_census/pass2/entity1/incarnation6/directory8. All four original PIDs are
independently absent; Shipping games[], 20/20 original save hashes unchanged,
crash[]/unobservedchildren[]. Full native log, exact 860-line authority36552
trace and native_operator_summary.json are preserved.

Both original windows were qualified and moved early, with initial evidence
preserved. Their outer bounds were 896x566; client2's right edge remained16px
on the primary. A later 880x527 fit occurred after exit and was unqualified,
with no window action. Full secondary containment is still unproved. The
ignored operator helper now performs the requested fit immediately next run.

Next fixes: match construction armor map semantics from actual cooked
Map_Values/independent passport Slot evidence, preserving every row and field;
emit precise copied slot context and bounded prevalidation size diagnostics;
cancel long captures at throttled admission boundaries and perform native
teardown only after unwinding. Then focused tests/independent review, combined
G0/dev checkpoint and another exact deployed actual scenario. No source
performance or scene delivery gain is inferred from this run.

The construction fix is implemented from the native evidence in
`native-armor-slot-proof-20261009.md`: all construction map keys and passport
slots remain independent copied values; strict live bindings remain unchanged.
Refusals now report copied table/row/key/passport slot/class plus bounded planned
encoding statistics before semantic admission. Invalid recipes cannot publish.
The production Lua plain/signature path also now uses the 512KiB token-derived
value/table budget, replacing its remaining old16000 ceiling. Focused descriptor
23/raw parser11/Lua source257/syntax2 checks and independent Sol review pass.
The wide Lua capture retains all32768 bone occurrences and detects tail mutation.
The fixture changes retain native f32 colour widths. No new actual success is
inferred from these offline results.

Worker cancellation is also implemented and independently reviewed: stop-file
and parent checks are throttled at250ms admission boundaries and only latch a
reason/fault. Source capture unwinds and closes its original native scope before
the outer loop performs the existing controller/host/Director shutdown once.
Focused worker247 assertions/syntax2 pass, including stop and unavailable parent
mid-capture, refusal of the next getter/registration and scope-end-before-teardown.
Original world/lifetime/controller checks remain. Actual graceful stop remains
pending; these checks do not establish playable registration or scene delivery.

Combined checkpoint5ba23d3c first full G0 passes74 Lua suites and all lints,
but cargo stops at1145 passed/1 failed in47 binaries after the sidecar
`deaths_are_scoped_by_match_and_epoch` test. Clippy has0 errors/13 warnings.
That test and two other death-dedup tests omitted the shared test-global lock,
allowing reset_session to erase another test's dedup/vitals state. The next
correction adds the existing shared lock to those three tests only; no gameplay,
transport or timing tolerance changes. Focused normal-parallel sidecar checks
pass106/0 failures/1 ignored in1.61s; the required complete gate retry is next.
No5ba23d3c deploy or actual run.

The live clothing audit found no supported mismatch: ArmorSlots enum labels are
reordered, NewEnumerator2=numeric16 and NewEnumerator0=numeric2. The exact native
Modular Panties branch contributes live key16/passport16. Live strict validation
therefore remains. See native-armor-slot-proof and its primary references; label
suffixes are never used as native values. Actual source census from5339682b has
49 components: Camera1/Capsule1/Scene4/Skeletal13/Spline3/SpringArm1/Static26,
with no Groom component. This is the observed entity1 only, not complete gear parity.

## Previous actual: 0039aacd

Latest actual checkpoint0039aacde42c5b0a4c69c2de32aaf3f0df6a160f passed full
G0 (74 Lua suites/1333 Rust checks in79 binaries, clippy0 errors/12 warnings),
dev push and exact clean RequireG0 deployment. Native source/helper/FFI review
was closed with focused C++925 /W4 /WX and source Lua243/syntax3, scopes32,
parser8/presentation8/descriptor12. These offline checks do not prove gameplay.

Actual run `test-results/20261009-122114-c48890-native-host` used source UUID
552c6bcd-c83d-42f9-a825-176aaefb92f0, authority39116/supervisor23932 and normal
rendered clients42056/43700. NativeReady15.747s. Original entity1 has epoch
1413459191929052231/incarnation5/directory6. GripSk40 at28.171s, GuardSk43 at
28.477s, HeadSk45 at28.723s and PommelSk47 at28.965s all prove native_empty_skeletal,
both assets absent, no override, LOD count0, material count/mask0 and bone count0.
All49 component rows complete in both harvests29.906s/42.189s. Adapter/source
capture finishes42.222s; native registration refuses `source recipe byte bound`
at42.223s/own seq893. Exact raw JSON length was not emitted. This closes the
actual empty skeletal source-collector blocker, while native bulk capture,
client mirror creation/readback and playable inputs remain unverified.

Canonical/mirror frames0, sampled0 and active inputs[0,0]. The authority misses
its graceful stop deadline after the expensive capture and the supervisor stops
only the original owned process. No clean source-stopped event is present.
Both clients qualify suppression/isolation, remain wait_scene and then stop.
All4 original PIDs are independently absent, no shipping game remains,20/20
original save hashes match, crash[]/unobservedchildren[]. Full HSMPNative.log,
exact authority39116 trace524 lines and native_operator_summary.json are saved.
Live window rectangles were missed before exit; secondary placement remains
unproved and no window movement was performed in this run.

Root cause: native_descriptor_binding's Lua→JSON staging applies60KiB before a
typed SourceRecipe exists; canonical_bytes/decode_recipe independently impose
the same old JSON limit. Actual first-entity fields also prove600 bone transforms
and a V3 lower bound55,869 bytes before core/material payloads. Exact second-
entity and recipe lengths remain unobserved. The full source cannot be reduced
by omitting body/gear components, materials, collision fields or native bone data.

Implementation in progress requires capability27 NATIVE_SCENE_STREAM, compact
dictionary recipes with unchanged logical schema6 and a versioned binary codec,
direct bounded LuaValue staging, and trusted scalar size diagnostics. The user
explicitly authorizes increased data budgets on2026-10-09: give complete scenes
more capacity, preserve high resolution and speed, and use only lossless encoding.
The encoded recipe budget therefore increases to512KiB with bounded descriptor
parts0AC2;64KiB remains an individual transport-record limit. Preserve
depth24 and all typed field bounds. The value/table node budget derives from
the512KiB encoded budget (each token consumes at least one byte); object keys
are dictionary references rather than extra nodes. This replaces the old16000
node ceiling without dropping fields. Dictionary and decoded allocation bounds
derive from the accepted encoded budget.
No inferred defaults or lost duplicate occurrences. Decode rejects malformed,
duplicate, unknown or out-of-bound data before aggregate allocation.

Lossless render manifests/byte parts retain internal RenderWorldV3 and every
native f64 bit. Old single-message V3 helpers keep64KiB refusal. Source preflight,
Bridge publication and relay must use bounded logical-frame validation together.
Complete logical scene capacity derives from accepted recipe counts and can
exceed100KiB; never shrink resolution to fit a packet. Recipe dictionaries run
only on metadata capture, while pose frame values remain raw. Measure actual
processing/transmission costs; no zero-latency claim follows from losslessness.
Per-peer one pinned active batch plus one replaceable pending latest frame closes
backpressure and supersession starvation. Receiver ADMITTED precedes parts and
requires current recipes/derived allocation bounds; COMPLETE follows complete
validated atomic publication. Neither acknowledgment can grant native MirrorReady.
Native complete-scene readback/final original guards remain mandatory.

Startup READY complete scenes remain generation-pinned through native preparation.
LIVE input keeps the existing250ms freshness requirement after apply and before
send. Keep inert mirrors through transient gaps; clear only actual identity/world
generation changes. Temporary-expiry recreation and post-apply input freshness
omissions are fixed with73 focused Lua assertions; actual validation is pending.
Safe performance work must
follow actual stage measurements and retain all final native proofs.

Source changes are authorized; no new actual run/full G0 until the coherent
codec/scheduler/lifecycle checkpoint passes focused checks and independent review.
Focused verification and independent reviews are closed. Core lifecycle73 Lua
assertions and syntax2 pass; network/server native64, presentation8, raw parser10,
source scopes32 and expanded descriptor21 checks pass. Prior native capability
bits20–26 without27 are refused for both presenting PvP and diagnostic peers.
World-only diagnostic peers retain their existing admission behavior.

Offline authenticated UDP delivers a complete125,362-byte synthetic scene
atomically in311ms in the debug/local endpoint test. This is neither native nor
release latency evidence. The expanded64-component/32,768-bone recipe fixture
retains all fields: raw JSON2,142,406 bytes, compact393,603, nodes103,422,
dictionary23,772 and tokens369,831. The smaller synthetic49-component/600-bone
fixture is74,449 raw JSON bytes/24,511 compact; debug metadata encode mean is
5,345 microseconds over8 iterations and has no material slots. These fixtures
prove capacity/bit retention, not actual roster size or gameplay parity.

Next: one coherent full G0/dev push, exact clean RequireG0 deployment, then the
normal100-second native host scenario with unchanged65-second readiness,
10-second observation and250ms LIVE freshness bounds. Capture both owned client
window rectangles early and verify secondary placement. Measure real recipe,
frame, mirror and input processing/delivery evidence before further optimization.
No main PR, merge, tag or headless/co-op release is authorized by these results.
