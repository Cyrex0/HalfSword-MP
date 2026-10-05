# Articulated source collision prototype

Dev-only files (not imported or registered by production):

- `mods/dev/HSMPParity/Scripts/skeletal_collision_stage.lua`
- `mods/dev/HSMPParity/Scripts/skeletal_hit_context_stage.lua`
- `tools/hsmp-tools/lua-tests/skeletal_collision_stage.lua`
- `native-strap-fixture.lua` in this audit directory, generated from the exact twelve cooked AggGeom capsule definitions.

Focused result: **62 checks passed**. No native game execution or production/schema edits occurred.

## What is implemented in the prototype

The reader resolves the component PhysicsAsset override, otherwise its SkeletalMesh's PhysicsAsset, then visits each exact SkeletalBodySetup and BoneName. It supports native SphereElems, BoxElems and SphylElems without replacing them with render bounds. FKBox X/Y/Z are full dimensions; FKSphyl Length is the cylinder length, with hemispherical radius handled separately. It records component ordinal, body bone, primitive type and primitive index as distinct identity fields.

Geometry uses actual primitive rotation and center. Point-to-sphere, point-to-oriented-box and point-to-capsule distances are exact. The sweep interpolates the stated rigid-body history using quaternion slerp and center translation. Exact shape distances establish acceptance; Lipschitz interval bounds establish exclusion. If its bounded subdivision budget cannot decide, it returns `indeterminate`, never acceptance based on a broad box. Velocity at a contact uses linear plus angular cross displacement.

Source identity includes actor, original match/round/life, module, bone, element and PhysicsAsset. Changed source identity or dimensions cannot be interpolated. Unsupported convex/tapered/level-set shapes, nonuniform scale, invalid body names and capacity overflow fail explicitly. Prototype bounds are32 bodies /128 primitives per actor, with no truncation. These are resource limits, not a claim every future actor fits.

The tests include the actual B/C/I strap definitions, four bodies each; a rounded capsule corner which a bounding box would incorrectly accept; a moving capsule; angular contact speed with stationary center; changed actor/bone rejection; numeric native collision enums; and overflow/unsupported-shape rejection.

## Native frame API and required proof

The pinned SDK exposes:

`/Script/Engine.PhysicsObjectBlueprintLibrary:GetPhysicsObjectWorldTransform(Component, BoneName)`

Its static receiver is `/Script/Engine.Default__PhysicsObjectBlueprintLibrary`; it takes the original primitive component and FName and returns FTransform. See `PhysicsObjectBlueprintLibrary.hpp:27`. The reader injects this API through `physics_transform`; it does not substitute rendered `GetBoneTransform` or a nearest-bone search. Point and angular velocities use the original BoneName through the reflected PrimitiveComponent APIs.

**Still unverified in a running native game:** whether this physics-object transform carries all body scale needed to transform the unscaled PhysicsAsset AggGeom, including native module scaling. Every staged row remains `native_frame_verified=false`. A required `physics_body_exists` adapter must also prove that the named native physics object currently exists; an asset body entry or an identity transform returned for a missing bone is insufficient. No verified implementation of that adapter is claimed here. The isolated probe must compare four named body transforms, native COM, point velocity, asset centers/radii/lengths, component/bone scale, and an actual collision contact. A successful mocked frame is not authorization to publish production shapes. Nonuniform scaling requires exact Chaos shape scaling semantics; the prototype refuses it.

## Original source bone across DCD

Native `ModularWeaponBP:Collision Hit` receives the original HitComponent and FHitResult, copies them into its ubergraph arguments (function offsets0–54), then calls its graph at74603. The graph writes `self.Hit Result` at3899 before the nested DCD. Native setup lists collision modules in order `[Head, Sub1, Sub2, HeadSk, Guard, GuardSk, Grip, Pommel, PommelSk]`; the original animated HeadSk source therefore uses ordinal4, and the original hit's MyBoneName identifies Joint1..4.

The staged context module requires a **true native entry/exit scope** around this original function. It copies only plain original identities, point and MyBoneName, assigns a token, and binds nested DCD only if weapon/component/victim/bone/match/round/life agree. Nested identical contacts preserve distinct inner/outer source bones. No context survives return.

**A Lua Blueprint post hook alone does not provide this entry guarantee.** Existing observed post-hook timing occurs after nested DCD. Production must either prove a native pre/post ProcessEvent bridge for this exact function, or establish an equally strict original-input mechanism. A post hoc nearest pending record or mutable `Hit Result` field alone is not accepted as proof under reentrancy. This prototype installs no hooks and deliberately does not claim this bridge exists.

## Bounded transport proposal

Do not expand the already near-MTU pose into arbitrary module/body rows. Use two linked messages:

1. An ordered, reliable **shape manifest**, scoped by session/match/round/life plus server-owned weapon-instance identity and manifest revision. It names every active module, exact body bone, primitive index/type, native local dimensions and rotation, PhysicsAsset identity, and original construction/passport hash. Bound bytes, bodies and primitives; reject unsupported or incomplete manifests explicitly. Sparse component IDs remain distinct from shape count.
2. A separate unreliable **body-frame stream**, carrying the same immutable identity/revision, timestamp/sequence and every active body's physical transform. Each complete snapshot has an explicit body count and completeness marker. If a snapshot needs multiple packets, every fragment repeats the scope/revision/snapshot identity; only a fully reassembled, bounded snapshot enters history. Missing fragments never turn into partial geometry. Reassembly has a fixed count/byte/time budget.

For the known strap case, only four dynamic body frames are required, with local capsule geometry sent once. Even all three native strap assets have twelve bodies total; their geometry need not consume the ordinary static/cutting-child pose rows. Keep existing position/rotation precision or improve it; do not choose coarser packing merely to force every case into one packet. Exact maximum encoded sizes and fragments must be tested before an ABI proposal is approved.

Server validation must select the original hand/weapon-instance/module/**body bone** from the authenticated source token, find only that body's primitive set at the original timestamp, and use its own local motion for contact velocity. A missing named body is a missing-history result, never a fallback to another capsule, the actor center, or the full component render bounds. Static held weapons and source-role/parent checks continue on their current route until this independent stream has native proof.
