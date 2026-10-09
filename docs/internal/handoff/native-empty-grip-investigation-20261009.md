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

Implementation and focused checks are in progress. No empty-native source/mirror
or complete scene parity has been proved in a running game yet.
