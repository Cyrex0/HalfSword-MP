# Empty inherited Grip investigation

Actual clean/gated/deployed db69d461 reaches38 complete static component rows,
then refuses weapon owner1 component39 `Grip` at27.218s with
`native static asset unavailable`. Run/source/cleanup details are recorded in
`native-camera-spring-arm-proof-20261009.md`. No complete recipe, bulk native
scene operation, canonical/mirror frame or active player input is proved.

The production collector's original qualified StaticMesh read returns no wrapper
or an address-zero wrapper. Its `object_id` helper emits this refusal for those
two cases; invalid nonzero objects, transient/runtime state and changed hard
assets have distinct paths. The wrapper form was not logged. This is not a
native hard-null proof and not a vertex-color or paint refusal.

Read-only cooked evidence distinguishes inherited ModularWeaponBP
`Grip_GEN_VARIABLE` from Axe `Grip_0_GEN_VARIABLE`. The base holder is a native
StaticMeshComponent with collision/tag configuration and no serialized
StaticMesh assignment. Base SCS_Node_5 attaches it to BaseMesh root SCS_Node_16.
Axe SCS_Node_4 is the distinct Grip_0, attached externally to BaseMesh; it has
the AxeShaft mesh, PM_Wood and its own relative transform. SDK base Grip field
is offset318, Axe Grip_0 offsetC60. No Axe inherited-component override targets
base Grip.

Axe construction calls its parent and adds Blade/Grip_0 to the collision array.
Base construction has no SetStaticMesh reference and includes inherited Grip
in its collision array. Runtime module setup does call SetStaticMesh, so the
original hard link can change later. Omitted cooked data alone is not live
absence evidence.

Any complete empty-static support must retain the exact native component,
original parent/socket, collision/material observations and transforms. It must
prove the fresh source hard mesh link is null and establish matched native null
socket/proxy semantics, then recheck the whole original component set after all
callbacks at every capture/publication and owned inert mirror readback. Do not
substitute AxeShaft, omit the holder, replace it with Scene or invent defaults.

The existing sixth native source API adopts a nonnull StaticMesh before its
override census and cannot prove this null case. Its matched hard-link offset560
and complete LODData offset588/stride90 provide existing layout evidence for a
bounded versioned extension.

## Matched native semantics and agreed implementation

Exact StaticMeshComponent constructor143C544B0 installs table14766FC60 at
143C544D6 and initializes hard StaticMesh560 to0 at143C544FE. Socket table
slot4D0 is143C58B70; helper143C58B00 reads560 and returns0 at143C58B60 on null.
The getter then falls back to SceneComponent GetSocketTransform143BF1620 at
143C58CCF. This preserves the exact native empty component's socket behavior.

Native render-admission slot348/143C5B2B0 returnsfalse on hard-null, and
ActorComponent registration143B52582/143B50B51 skips render-state creation350.
The exact table's proxy factory slot7D8/143956670 reads560 at143956692, testsnull
at143956699, jumps1439568F0 (xor eax,eax) and returns143956912 before allocation.
Primitive callback143BD13F0 invokes that slot at143BD1403 and stores the returned
proxy at2F0/4F8. These are matched installed-code facts, not a runtime null witness
for the previously exited Grip.

Agreed schema5 introduces `Geometry::NativeEmpty` only for exact native static
components: logical asset empty, vertex state NotApplicable, actual collision
and material dictionaries retained. The private provider ABI10 proof is12 bytes
and appends required asset_present0/1; the sixth source API exposes four required
fields, including that boolean. `native_empty` requires false plus a complete
all-null override census. Private kind8 carries the actual component_class for
class proof and constructs an exact inert StaticMeshComponent with no mesh.
Final scene checks retain original hard-null/census evidence after all callbacks.
Capability25 is required by presenting peers; current render revision3 remains.

Implementation and focused review are closed. Source Lua213 assertions/syntax3,
Rust source_scope30/raw parser7/presentation7/descriptor11 and C++567 checks pass.
The changed native provider object and fixture compile /W4 /WX. Cap25/server
native filter47 passes, including older presenting peer refusal and authenticated
V3 delivery. The private native implementation is18d6fb26, fixtureca24223f,
source/schema73bc8bf5. No unsafe Send, bound or timeout relaxation.

Integration review also caught an earlier Camera/SpringArm creation defect:
collision_off's nonprimitive branch still required exact Scene. It now admits
only the already-proved exact Camera/Arm profiles. Direct production-helper
regressions catch that old refusal and verify the empty static's Primitive
collision/physics path. The final native/compatibility/source reviews are closed.

Combined full G0, clean deployment and actual native scenario remain pending.
No empty-native source/mirror or complete scene parity has been proved in a
running game yet.

## Actual c87dda89 result

Full G0 passes74 Lua suites/1331 Rust checks in79 binaries, clippy0 errors and12
warnings. Dev pushdb69d461→c87dda89 and exact clean RequireG0 deployment pass.
Actual run `test-results/20261009-102258-f482bd-native-host`, source UUID
8af2dd23-3423-493e-8f52-75f3a2f861de, authority42828/supervisor2128,
normal clients32796/44164. NativeReady15.029s.

Original Grip component39/address3090283243328/owner1 passes native proof27.087s:
state=native_empty, no_override=true, asset_present=false, count0. The complete
ordinary collector preserves material count0 and completes that row27.091s.
It then begins GripSk component40/address3090641211568/owner1 and refuses
`native skeletal asset unavailable`27.137s. Exact class/current hard-null for
GripSk remain unproved; it cannot reuse static-component evidence.

Complete recipes/canonical/mirror frames0 and active inputs[0,0]. The real
empty-static source proof passed; native bulk capture/creation/readback remain
unexercised. Source stops normally48.127s, clients stop after session end,
client1 held exit0/0x0 at09:23:47.481UTC. All4 owned processes absent,20/20
original save hashes unchanged, crash[]/unobservedchildren[]. Full native trace,
authority-only filter and native_operator_summary.json are retained.
