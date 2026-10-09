# Native Camera/SpringArm attachment checkpoint

## Actual trigger and scope

Actual clean build02909df8 reaches the guarded static color-absence proof for
Sphere.Sphere, then refuses component6 `CameraBoom(Shoulder)`, exact class
`/Script/Engine.SpringArmComponent`. Source recipes/scene frames remain zero.
Run: `test-results/20261009-084130-2f5bc9-native-host`; source UUID
abc9910b-2172-4d47-998a-bd7bd4ae6bcc; authority29952, supervisor44664,
clients40560/21020. The timed authority ends normally and client2's original
held process handle reports exit0. This does not explain the separate earlier
client travel exit and does not establish playable replication.

## Cooked attachment evidence

Read-only cooked Willie SCS census has315 actual nodes (highest node label319).
The mesh and owned-spline ancestor projection contains exactly native SkeletalMesh,
StaticMesh, Groom, Spline, Scene, Camera and SpringArm classes. No Arrow node
appears in this full census. The critical chain is
SCS_Node_99 (Aim Spline Scene Sphere) →5 (AimSplineScene) →78 (AimSpline) →2
(FollowCamera) →41 (CameraBoom(Shoulder)) →34 (CameraScene) →external native
CharacterMesh0. The other arms are FP node10 and Center node1; the other cameras
are FP node11 and FollowCamera1 node104. Live original-root closure remains
authoritative. These facts support both new classes together without speculative
shape/Arrow allowlisting.

The shoulder template enables rotation/camera lag and control rotation. Its
debug-lag flag is absent from the cooked template; this is not evidence of its
actual runtime value. The source must freshly observe false and retain exact
parent/socket identities.

## Matched installed native code

Shipping EXE SHA256:
367DFCCF1AACA3BBF6824C9BB616F2F31BC30E7AC70C5FA8657E212DBB2C2E03.
Preferred image base140000000. Read-only disassembly and pinned ObjectDump/SDK
evidence were used; the original IDA database was not edited.

SpringArm constructor143CBA350 calls Scene constructor143BEA040 and installs
vtable1476952B8; exact class size330. Reflected `bDrawDebugLagMarkers` is offset271,
mask1. Vtable slot490 is ApplyWorldOffset143CBA4C0; slot4D0 is
GetSocketTransform143CBA580. The latter reads cached translation at2F0/2F8/300
and quaternion310/318/320/328. Component transform-space2 returns those values
with native scale[1,1,1]; world0 composes the component transform, actor1 uses the
owner-relative path. The socket-name argument is unused by this matched body,
but the source recipe still copies its actual complete singleton socket census.
A plain Scene substitute loses this native endpoint behavior.

Matched QuerySupportedSockets143CBB6A0 appends exactly one socket entry and reads
the native FName64 from preferred VA148D70CE0 (image-relative8D70CE0), independent
of the component. The final native census pins that query body and compares the
original global FName bits with the guarded prepared recipe name. No name
conversion/lookup, socket getter or ProcessEvent follows the final census.

Matched ActorComponent IsComponentTickEnabled143B51630 is the eight-byte body
`80 79 3B 00 0F 95 C0 C3`: it returns whether byte3B is nonzero. The Camera,
SpringArm and base Scene vtables inherit it at slot3E8. The final owned-mirror
check pins these targets and reads disabled byte3B==0, alongside reflected
PrimaryComponentTick offset30/size30 and bIsActive8A/mask8. Review caught that
comparing a later tick snapshot alone could accept activation between getter
and snapshot; the direct matched disabled-byte check closes that gap.

Camera constructor143AFB160 calls Scene143BEA040, installs vtable1476085C0 and
has exact size9E0. Comparison of base Scene virtual slots below5F0 differs only
at destruction,330 and528. Camera330/143AFD800 tail-jumps to Scene OnRegister
143BF3A50; Camera528/1413973D0 tail-jumps to14112E0B0. Camera's matched reflected
OnCameraMeshHiddenChanged143255FA0 only advances FFrame.Code and returns,
without receiver access or calls. These establish the narrow native ancestor
profile; local owned-camera/view-target/HUD behavior remains a separate gate.

## Implementation contract under verification

Source schema4 adds exact Camera and SpringArm Scene evidence, collision=null,
SpringArm actual debugfalse and actual singleton socket name. Render revision3
record0A13 appends explicit optional raw endpoint translation3/rotation4 doubles
to each component. Required capability24 joins existing20–23; older presenting
peers/records are refused. Protocol12, IPC ABI2, entity32/component64 and the64KiB
message bound remain unchanged.

Private provider ABI9 uses exact native classes. Source reads qualify original
class/identity/layout/code and compare the cached endpoint with the native socket
getter. Only owned inert mirrors receive private cache writes; tick/activation
remain disabled and native socket output must read back. Parent-first world
transforms preserve attached children. The final callback-free complete-scene
boundary rechecks original identities, fresh debug flags and endpoint values
after all earlier getters/callbacks. No missing class, debug value, socket census,
layout signature or endpoint acquires a default.

Focused checks and review establish bounded implementation behavior only.
Actual complete recipes, advancing source/mirror frames, both legal controller
inputs, combat/gear/body/cut parity and camera/HUD ownership are still required.
There is no headless release acceptance at this checkpoint.

## Focused checkpoint before the next native run

Transport committed047eeadc/3f0a64ce; source six-file checkpoint9546c74b.
Server native filter46 passed before the final source test addition; the
strengthened authenticated UDP endpoint test passed separately and preserves
encoded endpoint bits. Source Lua192 assertions, syntax3 files, descriptor
filter10 and raw parser6 passed. Native Rust presentation filter6 passed,
including exact private kinds6/7, actual socket text, arm-only allocation,
Box-backed pointer lifetime and raw values. The provider object compiles with
/W4 /WX. Source stores pointer-free vertex bindings and builds borrowed FFI
finish targets only for the guarded call; no unsafe Send was added.

Native C++ fixture compiles /W4 /WX and passes487 checks, preserving all prior340.
The new cases cover exact Camera/Arm profiles and frame presence, original
raw output/getter mismatch, earlier-arm mutation by later target callbacks,
and mirror tick/activation changes after getters, metadata and the final guard.
Independent native review and root integration/fixture review are closed.
Root independently rechecked the installed native tick getter's eight bytes.

Combined full G0, clean deployment and the actual two-client/one-authority run
remain pending. These checks do not establish scene publication, active input,
native parity or release readiness.
