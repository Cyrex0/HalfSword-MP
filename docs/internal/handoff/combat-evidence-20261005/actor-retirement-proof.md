# Native inventory retirement proof — 2026-10-05

The diagnostic now requires a positive live-world observation before destruction, followed by successful same-world/class enumeration absence AND either global UObject absence or the exact fresh object's native mirrored-garbage flag. A successful K2 call or a valid global wrapper alone proves neither success nor failure. Native smoke remains required; the full catalogue has not passed.

## Matched executable evidence

The isolated IDA copy analysed `workspace/ida/HalfSwordUE5-Shipping.exe`, SHA-256 `367dfccf1aaca3bbf6824c9bb616f2f31bc30e7ac70c5fa8657e212dbb2c2e03`. See `index.json`, `strings.json`, and `actor-1435ebd50.c` in this directory.

* `K2_DestroyActor` string at `0x147445f70` is registered at `0x14744f830`, with native exec pointer `0x1434a7a30`. Its short thunk dispatches actor virtual slot `0x368`; the underlying virtual implementation has not been identified. Nearby registration pointers must not be described as the DestroyActor body.
* `IsActorBeingDestroyed` native exec `0x1434a5be0` reads actor byte `0x5d` bit zero. This does not inspect UObject garbage flags.
* `GetAllActorsOfClass` native exec `0x1435ebd50` creates an actor iterator with flags 5. It rejects objects with native object flag `0x40000000`, filters inactive levels, and checks the expected native world. Thus world enumeration absence alone could also mean level filtering, not destruction.
* Pinned UE4SS `mod/RE-UE4SS/UE4SS/include/LuaType/LuaUObject.hpp:679` exposes `HasAnyFlags` directly through `object->HasAnyFlags(object_flags)`. It does not call ProcessEvent. Its `IsValid` implementation checks object-map membership and unreachable state, not this mirrored-garbage flag.

Epic names `0x40000000` `RF_MirroredGarbage`; the mask is independently present in this executable's actor iterator. [EObjectFlags](https://dev.epicgames.com/documentation/en-us/unreal-engine/API/Runtime/CoreUObject/EObjectFlags). The documented DestroyActor contract separates removal from the world's actor list from later garbage collection. [UWorld::DestroyActor](https://dev.epicgames.com/documentation/unreal-engine/API/Runtime/Engine/UWorld/DestroyActor?lang=en-US).

## Runtime observation and bounded fix

In completed run `20261005-125818-c9ef9c-combat_manual`, the first two clients' temporary actors disappeared at the same second despite being created five seconds apart. Their successors remained globally valid for the diagnostic's 45-second timeout. This is consistent with deferred garbage collection, but does not itself prove successful native retirement.

`collider_inventory.lua` now stores only plain original identities between callbacks. It freshly enumerates the exact class in the exact current world, checks original actor/class/name/address/world binding, and calls the direct native flag API only on that freshly found valid object. Unknown enumeration, scope mismatch, unavailable native flag API, or non-garbage global presence prevents progression. An unresolved cleanup obligation also blocks an explicit inventory restart. No forced GC or broad deletion is used.

Focused fixture result: **172 checks passed**. Fixtures include live actor before destruction, retained valid UObject after retirement, active-level-like omission without garbage, unavailable flags, changed native world, missing pre-observation, and unresolved restart refusal. These fixtures validate the state machine and bridge contract, not native catalogue coverage. Run one isolated class first; only a successful native retirement result permits a full sweep.
