# Native source runtime-path proof (2026-10-09)

The measured `3b2` source profile took 24.406 s; its provider trace counted 638
guards, 696 path searches and 84,678 admissions. The isolated original runtime
`StaticFindObject` search took 45.889 ms. Those measurements motivate replacing
repeated whole-object-array searches with verification of the complete original
native outer chain. They do not prove the replacement's runtime cost or source
rendering readiness.

## Matched inputs

- Local RE-UE4SS HEAD: `e3ba1016562d6c0868c410d0a71e88bfcdbf691b`.
- `game/HalfswordUE5/Binaries/Win64/ue4ss/UE4SS.dll` SHA256:
  `680A026890ABB4D0DF2211251F8DEFC1681A584275F1521DCC0FE30AF480006F`.
- PE machine: AMD64; preferred image base `0x180000000`. All offsets below are
  RVAs; the displayed static disassembly VA is image base plus RVA.
- Primary binary reads used the installed `C:/Program Files/LLVM/bin/llvm-readobj.exe`
  (`--coff-exports`) and `llvm-objdump.exe` (`-d --start-address=... --stop-address=...`).
  No game launch, live memory probe, IDA operation or source build was used.
- `mod/RE-UE4SS/deps/first/Unreal` is empty locally. Its implementation bodies
  were therefore established from this exact DLL, not attributed to an unread
  UEPseudo source checkout. Pinned caller/configuration source is available below.

## Exact metadata entry points

| Member | Mangled DLL export | RVA |
|---|---|---|
| `UObjectBase::GetOuterPrivate() const` | `?GetOuterPrivate@UObjectBase@Unreal@RC@@QEBAAEAPEBVUObject@23@XZ` | `0x3E3640` |
| `UObjectBase::GetNamePrivate() const` | `?GetNamePrivate@UObjectBase@Unreal@RC@@QEBAAEBVFName@23@XZ` | `0x3E1C50` |
| `UObjectBase::GetClassPrivate() const` | `?GetClassPrivate@UObjectBase@Unreal@RC@@QEBAAEAPEBVUClass@23@XZ` | `0x3E0830` |

The outer member returns a **reference to the native pointer field**,
`const RC::Unreal::UObject*&`, rather than the outer pointer itself. The flattened
MSVC x64 signature for read-only use is `const void* const* (*)(const void*)`:
`RCX=this`, `RAX=field address`; dereference once. The class member has the same
pointer-field-reference ABI. The name member returns `const FName&`, likewise a
field address; copy the matched native eight-byte FName only after admission.
No C++ string ABI or hidden return buffer is involved in these three getters.

`GetOuterPrivate`'s complete body is RVA `0x3E3640..0x3E36BC`. Its initialized
path loads a cached signed member offset at `0x3E366C`, adds the object address at
`0x3E3673` and returns at `0x3E367B`. The first-call helper
`0x3D7BC0..0x3D7D47` hashes/looks up the exported
`UObjectBase::MemberOffsets` map and returns its stored integer at `0x3D7D2D`;
other calls are string/CRT initialization, comparison, allocation and cleanup.
Neither path invokes ProcessEvent, an engine virtual function, FName conversion,
StaticFindObject or GUObjectArray iteration. Missing metadata is not a usable
offset and must refuse.

The name getter uses the same cached-offset-plus-object pattern at
`0x3E1C7C/0x3E1C83`; the class getter does so at `0x3E085C/0x3E0863`.
These are metadata reads, not lifetime checks. The existing original weak slot,
serial/address, class and `RF_MirroredGarbage=0x40000000` checks must precede them.

## Why the string getter is not an unconditional no-callback route

The DLL also exports:

```text
GetPathName append overload, RVA 0x3E37F0:
?GetPathName@UObject@Unreal@RC@@QEBAXPEAV123@AEAV?$basic_string@_WU?$char_traits@_W@std@@V?$allocator@_W@2@@std@@@Z
GetPathName returning std::wstring, RVA 0x3E37A0:
?GetPathName@UObject@Unreal@RC@@QEBA?AV?$basic_string@_WU?$char_traits@_W@std@@V?$allocator@_W@2@@std@@PEAV123@@Z
GetFullName returning std::wstring, RVA 0x3E0D80:
?GetFullName@UObject@Unreal@RC@@QEBA?AV?$basic_string@_WU?$char_traits@_W@std@@V?$allocator@_W@2@@std@@PEAV123@@Z
```

The append overload's flattened x64 call is
`void (*)(const void* self, const void* stop_outer, std::wstring& result)`:
`RCX=self`, `RDX=stop_outer`, `R8=&result`. The two returning members instead use
`RDX` for the hidden `std::wstring` result and `R8` for `stop_outer`. Such strings
should not be marshaled as a guessed Rust struct.

`GetPathName`'s body (`0x3E37F0..0x3E3A21`) walks OuterPrivate recursively and
converts each FName. It contains no direct ProcessEvent call, but its
`FName::ToString` callee (`0x3F52F0`, internal body `0x3F5310`) has a native
`ToStringInternal` route and a reflected conversion fallback. The fallback loads
`Conv_NameToStringInternal`/`KismetStringLibraryCDO` and invokes exported
`UObject::ProcessEvent` at **`0x3F57EF -> 0x3E7EA0`**. Therefore GetPathName and
GetFullName cannot be described as unconditionally free of callbacks.

Pinned [SettingsManager.hpp:45](../../../mod/RE-UE4SS/UE4SS/include/SettingsManager.hpp#L45)
defaults to `Scan`; [SettingsManager.cpp:113](../../../mod/RE-UE4SS/UE4SS/src/SettingsManager.cpp#L113)
also accepts `Conv_NameToString`; [UE4SSProgram.cpp:859](../../../mod/RE-UE4SS/UE4SS/src/UE4SSProgram.cpp#L859)
passes that selection to the initializer. The installed settings file requests
`Scan` at line 80. A configured preference does not establish that the native
function is available or that every call avoids the fallback.

## Exact path format and preserved source identity

Pinned [LuaUObject.hpp:532–541](../../../mod/RE-UE4SS/UE4SS/include/LuaType/LuaUObject.hpp#L532)
calls native `GetFullName()` for the Lua method. The matched full-name body
obtains the class name, adds one space, then calls the append path getter at
`0x3E0F90`. [Source runtime_path:81](../../../mods/HSMPMatch/Scripts/native_source_render.lua#L81)
matches `^%S+%s+(.+)$`, preserving the remaining full runtime path, including
package, world, level, actor, subobject separators and component-name spaces.
It rejects an absent path, NUL or more than 512 UTF-8 bytes. It performs no
case folding, suffix stripping or `:`/`.` splitting.

The native path routine appends `:` when the immediate outer's class FName is
not `GPackageName` and that outer's outer class FName equals `GPackageName`;
otherwise it appends `.`. The relevant comparisons are `0x3E386C..0x3E38A8`,
the colon write is `0x3E38AA`, and the dot write is `0x3E38DE`/`0x3E38F0`.
`GPackageName` is an exported eight-byte FName at RVA `0x13472F0`.

The safe replacement is an original scalar witness, not just a component name:

1. Admit source role, game thread, original world and exact directory/entity
   generation. Keep the exact initial path search and require its result to equal
   the admitted original component address. Bracket witness capture with the same
   identity/world/path admission so the initial path and captured chain agree.
2. Capture the **entire** chain from component through all actual outers to native
   NULL, including world/package; record each node's original weak slot/serial,
   address, native FName, exact class weak/address and class FName. Qualify the
   node and its class, including garbage flags, before each metadata read. Use the
   same serial-zero address/FName/class discipline as current source identities;
   do not allocate a weak serial. Record/verify the original `GPackageName` bits.
3. On every later guard, resolve and verify these original scalar identities;
   require each fresh OuterPrivate pointer to equal the original next node, and
   require the last pointer to remain NULL. A class FName change matters even if
   its pointer is unchanged because it can alter the colon boundary. Preserve
   the existing owner/root/AttachParent/current-world checks independently.
4. Detect cycles and refuse a chain that does not reach NULL within the approved
   64-node witness bound. Never stop at the actor/world or synthesize a root.
   Existing component/wire/path limits remain unchanged. Drop witnesses as
   scalars only on world loss; retain no borrowed UObject/field pointers.

Given an initially matched path, identical native node names, class names,
package-name discriminator and every outer link through NULL preserve exactly
the names, order and delimiters used by the matched path formatter. This verifies
the original path without per-guard string conversion or whole-array lookup;
it additionally refuses an outer replacement with coincidentally identical text.
Native cost and full source/mirror success still require the next measured run.
