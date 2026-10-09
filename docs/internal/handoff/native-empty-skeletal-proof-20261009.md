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
Implementation/focused verification and the next exact native scenario are pending.
