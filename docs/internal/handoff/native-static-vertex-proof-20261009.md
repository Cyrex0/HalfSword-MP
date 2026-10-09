# Native static vertex override proof (2026-10-09)

This note records the matched native layout and the minimum complete override
census. A CPU-gated getter returning zero does not establish absence of paint.
The candidate proof still needs an actual source capture and mirror readback.

## Matched primary inputs

- Shipping image: `game/HalfswordUE5/Binaries/Win64/HalfswordUE5-Win64-Shipping.exe`.
- SHA256, independently re-read for this note:
  `367DFCCF1AACA3BBF6824C9BB616F2F31BC30E7AC70C5FA8657E212DBB2C2E03`.
- AMD64 preferred image base `0x140000000`; recorded ObjectDump ASLR base
  `0x7FF7D34F0000`. Below, RVA means offset from the image base.
- Primary binary reads used installed `C:/Program Files/LLVM/bin/llvm-objdump.exe`
  with exact `-d --start-address=... --stop-address=...` windows. No game launch,
  live caller probe, Python, IDA operation or build was used for this note.
- Pinned dump: `game/HalfswordUE5/Binaries/Win64/ue4ss/UE4SS_ObjectDump.txt`.
- Pinned generated SDK root:
  `game/HalfswordUE5/Binaries/Win64/ue4ss/UE4SS_SDK/src/UE4SS_SDK/Script/`.

## Reflected ABI and independently matched private slots

| Fact | Primary evidence |
| --- | --- |
| Hard `StaticMeshComponent.StaticMesh` object at `0x560` | ObjectDump:2875; SDK `Engine/StaticMeshComponent.hpp`:30 |
| `LODData` array at `0x588`, size `0x10` | ObjectDump:2901–2902; SDK `Engine/StaticMeshComponent.hpp`:63; `CXXHeaderDump/Engine.hpp`:23660 |
| Inner is exactly `Engine.StaticMeshComponentLODInfo`, stride `0x90` | ObjectDump:82292; SDK `Engine/StaticMeshComponentLODInfo.hpp`:11–18; CXX dump:7715–7717 |
| Array pointer/count/capacity at array offsets `0/8/0xC` | Native count branch reads pointer at component `0x588` and count at `0x590`; typed native header is 16 bytes |
| `bAllowCPUAccess` at asset `0x185`, mask `0x2` | ObjectDump:25602; SDK `Engine/StaticMesh.hpp`:56 |

The SDK's LOD-info struct is opaque: it supplies size, not named private fields.
The following two independently inspected native branches identify the slot as
the pointer used for per-component override colors. They do not establish a
general-purpose private C++ buffer layout.

- Count thunk: ObjectDump:40526–40529, RVA `0x4A17AB0`, calls count helper
  RVA `0x4A696E0` at `0x4A17B5F`. Signature is
  `int32 GetMeshComponentAmountOfVerticesOnLOD(UPrimitiveComponent*, int32)`;
  reflected inputs are object `0`, LOD `8`, integer return `0xC`, no out params.
- Count helper checks LOD against component count `0x590` at `0x4A697AB`.
  The exact 19-byte window at RVA `0x4A697B7` is:
  `48 8B 87 88 05 00 00 48 8D 0C F6 48 03 C9 48 8B 44 C8 30`.
  It loads `LODData.data`, computes `LOD * 9 * 2 * 8 = LOD * 0x90`, and
  loads the pointer at element `+0x30`. If nonnull, `0x4A697CF` reads its
  32-bit count at `+0x34`; a null pointer follows the asset-buffer branch.
- Color thunk: ObjectDump:40512–40516, RVA `0x4A17C10`; its return is the
  typed `TArray<FColor>`. Its native color helper uses the exact 26-byte window
  at RVA `0x4A6D26D`:
  `49 8B 87 88 05 00 00 4A 8D 0C E5 00 00 00 00 49 03 CC 48 03 C9 48 8B 4C C8 30`.
  This independently computes the same stride and loads the same `+0x30`
  override slot. A nonnull pointer branches to `0x4A6D2BF`, calling
  RVA `0x3EC4460`; that routine reads the buffer count at `+0x34` at
  `0x3EC4477` and uses it to size the copied color array.
- The candidate native provider pins both exact code windows and checks the
  reflected array/inner types, offsets and sizes before interpreting a slot.
  PE-machine/image-bound, metadata or code-window mismatch refuses the proof.

## Why the public getter refuses a valid native mesh

The static count helper tests asset byte `0x185`, bit `0x2`, at RVA
`0x4A69796`; false jumps to `0x4A69A8A`, which returns integer zero. This
occurs before the component override slot is examined. The color helper has
the same early CPU-access gate at `0x4A6D13D`, before its override branch.
Consequently zero/empty RVP results cannot prove all override slots are null.
The flag is an actual getter prerequisite; it must never be rewritten to make
the diagnostic or capture pass.

## Complete bounded proof and acceptance boundary

The implementation is in `crates/hsmp-native/cpp/src/native_vertex_state_impl.h`.
Its admission requires the original owner/component/world generation, native
slot/serial/address/FName/exact-class witnesses and flags, plus the original
hard static asset of the native static class. Garbage is refused before engine
operations; transient assets cannot acquire the cooked-asset profile.

After exact reflection/code admission, copy the entire 16-byte array header and
every actual `+0x30` slot for count `0..16`. Require nonnegative count/capacity,
capacity at least count, and a backing pointer whenever capacity is nonzero.
Copy the original hard asset link too. Requalify, repeat the complete copy, and
compare asset address, data pointer, count, capacity and every slot. After the
last callback-capable qualification, repeat with pure original native identity
and flag reads. Any mutation or unknown layout refuses; no truncation is used.

All-null slots yield `{state='native_asset', lod_info_count=actual,
no_override=true}`. Any verified nonnull slot yields `captured_required` with
`no_override=false`; that proves presence, not buffer readability. It retains
the exact existing color capture path or explicitly refuses when unreadable.
The census never dereferences an opaque color-buffer pointer.

Native source capture and inert mirror application/readback must repeat this
proof. A final complete target-set closure prevents a later target's native
callback from changing an earlier target: recheck every original target after
all callback-capable work, with no callback/PE/string conversion afterward.
Unknown build, world/owner/link replacement, flags or slot changes refuse.
The existing NativeAsset DTO keeps exact native cooked asset colors and empty
instance overrides; it does not synthesize white/color arrays or skip geometry.

## Actual evidence remains narrower than the candidate

Run `test-results/20261009-075353-7198ee-native-host` (candidate `689`) records
source-owned `host/8a59ed14-2b6a-4ac1-9d89-0f14d8062e68/worker/hsmp_events.jsonl`
seq94: `/Engine/BasicShapes/Sphere.Sphere` has native LOD count1.
Seq96 records LOD0 count `number0` and actual `bAllowCPUAccess=boolean false`.
The source still had frame0 and no active input. That run did not execute the
new complete slot census, so native override absence and mirror readiness are
not yet actual-runtime facts. Focused implementation tests are separate proof.
