# Empty native skeletal holder evidence

Installed Shipping EXE SHA256:
367DFCCF1AACA3BBF6824C9BB616F2F31BC30E7AC70C5FA8657E212DBB2C2E03.
Preferred image base140000000. Addresses below are from matched read-only native
disassembly, with no original IDA modification or runtime source writes.

Exact SkeletalMeshComponent constructor143BFCE10 installs vtable147659CB0;
class sizeF70. Deprecated SkeletalMesh is558 and SkinnedAsset560. Native
GetSkinnedAsset143C10CE0 reads558 first, falling back to560. GetNumBones143C1B610
uses the same priority and returns0 at143C1B65C when both are null. Missing Lua
wrappers cannot establish either native null.

Socket vtable slot4D0 targets143C1CBC0. With both assets null, socket lookup has
no asset and bone-index helper143C15CB0 returns-1 at143C15D16; the getter falls
back to the actual component transform. Leader-pose resolution follows a valid
bone index and cannot provide a socket for this native empty profile.

MeshObject at7A0 must also be null. Render helper143C1C110 checks MeshObject
before the assets and can return its render data at40 even with both assets
null. CreateSceneProxy slot7D8 targets143C136B0: the null render-data branch
143C13708→143C13790 returns null. Primitive callback143BD13F0 stores that
result at2F0 and4F8. Both links must remain null. No opaque render-resource
dereference is required or permitted for this absence proof.

LODInfo at760 has stride28 and per-row override pointer10, bounded to16 rows.
Matched RVP bytes144A698D6..144A698E9 pin that interpretation. Every original
row must have a null override; never infer absence from a CPU vertex getter.

OverrideMaterials starts518 (count520). Native GetNumMaterials returns0 when
both assets are null even if the raw override array has slots. GetMaterial
checks that array first. Capture all original raw slots, including explicit nulls,
bounded to32, and recheck the original header, identities and null mask after
all callbacks. Null and nonnull slots must remain in order on owned inert mirrors.

Matched SetMaterial exec143451440 dispatches slot668 to143BBA450. The equality
early return143BBA485..143BBA4A3 applies only to an existing slot. An out-of-range
null reaches143BBA4A9, grows count to index+1 at143BBA4C5, zero-fills new slots
143BBA4E0..143BBA4F0, then writes the requested material. Explicit trailing null
slots can therefore be reproduced without writing raw source arrays.
CreateDynamicMaterialInstance exec14344B170 dispatches slot688 to143BD11C0.
Nonnull SourceMaterial first calls SetMaterial143BD11E4, then GetMaterial
143BD11F2, creates the MID143BD1236 and assigns it143BD1249. It does not call
GetNumMaterials, so native getter count0 does not prevent nonnull slot setup.

MeshDeformerInstanceSet size20 contains DeformerInstances Array at0 with
ObjectProperty inner8 (SDK MeshDeformerInstanceSet.hpp34; ObjectDump82134–36).
GetMeshDeformerInstance exec143461C60 calls143C1AC60: it reads count5A0,
then array598/element0 if positive, otherwise returns0 at143C1AC9F. The absence
check validates that exact reflected member and inner layout and the complete
array header. The opaque remaining16 bytes are retained only for coherence,
without assigning meaning or defaults. bSetMeshDeformer uses its reflected,
admitted bool mask; unrelated bits in byte580 do not become that flag.

Cooked BaseModularWeapon has four assetless skeletal templates: GripSk, GuardSk,
HeadSk and PommelSk. Its SetUpModulePartSk checks component validity but does
not check asset validity before SetMesh/GetMaterial/SetMaterial, so a null asset
does not establish an empty material array. Generic armor holders use the same
native class; observed populated assets retain their existing exact capture.
No new nullable Groom profile is justified by the audited current roster.

Schema6/private ABI11/private kind9/capability26 cover only the qualified exact
skeletal profile. Preserve observed collision, parent/socket, flags and nullable
PhysicsAssetOverride. Unsupported nonnull physics overrides remain explicit
refusals until owned mirror binding/readback exists. Animation, cloth, deformer,
leader-pose and active simulation gates remain in force. Source objects remain
untouched; only owned presentation mirrors become inert.

Current runtime evidence remains c87dda89: Grip39 passes empty-static proof,
then GripSk40 refuses native skeletal asset unavailable. No actual empty skeletal
proof, complete recipe, mirror frame or owned active input has passed yet.
Schema/source checkpoint181cef2f passes Lua243 assertions/syntax3, source scopes32,
raw parser8, presentation binding8 and descriptor filter12. Independent source/
schema review is closed. Provider and direct production-path C++ verification
pass925 assertions (all prior567 retained) and strict /W4 /WX compilation after
the final reflected deformer-layout check. Fixture checkpoint0d62a43d exercises
actual capture/create/material branches, source remaining untouched, owned inert
state, complete original material provenance, late callbacks and scalar-only
snapshot cleanup. Independent native/provider/FFI review is closed. The exact
clean G0/deployment/native scenario remains pending.

Read-only byte audit of that same actual UUID/pass1 finds39 completed component
rows,600 bones,15 morphs and37 material slots. Three observed splines contribute
776/1201/1201 bytes, plus one56-byte SpringArm payload. Known entity1 V3 lower
bound is54,959 bytes before embedded core and any material parameter/texture
payload; counting all49 closure row headers raises it to55,869. Entity2 and the
last10 rows were not captured, and no canonical recipe JSON exists. Do not invent
their exact size. A second entity with the observed composition would exceed the
unchanged65,536-byte fragmented-message cap (HDR8). The next actual must preserve
exact source/encoding refusal evidence; any fix must use bounded lossless chunks
with atomic complete-frame verification, without dropping bones or raising caps.
