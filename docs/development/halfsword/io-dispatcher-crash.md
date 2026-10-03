# The IoDispatcher pak-read crash

A native crash in the game executable, on the `IoDispatcher` thread, during the first load from
the main menu into an arena. It is an engine bug (UE 5.4 hair-strands streaming plus the shipping
pak reader), not a bug in HSMP or UE4SS. HSMP works around it by turning hair-strands streaming
off. `hsmp-tools crash-triage` classifies it as `PAK_ASYNC_READ_OOB`.

## Symptom

- The game closes with the Unreal crash reporter about a minute after launch, roughly 0.5 s after
  the `OpenLevel` into an arena.
- The faulting thread is `IoDispatcher`. No Lua runs on any thread: the UE4SS mod threads are
  asleep and the game thread is in its normal frame.
- The fault is an access violation on `inc dword ptr [r15+2Ch]` with a garbage `r15`, at RVA
  `0x21e5ca4` of `HalfswordUE5-Win64-Shipping.exe` (UE 5.4.4, changelist 2705). Both known dumps
  have `PCallStackHash` `44106A9DC8756FBE63C328366971E9612B82C734` and the same seven game-module
  frames: `21e5ca4 1444293 1444e8e 11fdf3d 11fe57a 12e1067 12e0f89`.
- It is not memory pressure: `MemoryStats.bIsOOM=0` and the pointer is garbage at one specific
  index, not a failed allocation.

It was seen only in two-instance multiplayer sessions. No vanilla crash dump on the same machine
had this signature.

## Root cause, as far as it is established

The minidumps hold stack memory only, and there are no public symbols for the game, so the frames
were identified from function bounds, struct layouts and constants:

| Frame | Function start (RVA) | Function |
|---|---|---|
| 04 | `11fe530` | `FIoDispatcherImpl::Run` (the thread loop) |
| 03 | `11fd820` | `FIoDispatcherImpl::ProcessIncomingRequests` |
| 02 | `1444c60` | the package-resource I/O backend's resolve. It tests chunk-id byte 11 for `EIoChunkType` 13 (`PackageResource`): it serves `FBulkData` reads from the `.pak` |
| 01 | `14441e0` | issues `IAsyncReadFileHandle::ReadRequest` (vtable `+0x10`) with the request's offset, size and buffer |
| 00 | `21e5960` | `FPakAsyncReadFileHandle::ReadRequest` (`IPlatformFilePak.cpp`) |

Frame 00 follows the UE 5.4 source. For a compressed entry it computes
`FirstBlock = Offset / CompressionBlockSize` and `LastBlock = (Offset + BytesToRead - 1) /
CompressionBlockSize`, then for each block takes `Blocks[i]` (allocating a `FCachedAsyncBlock` if
the slot is null) and increments its `RefCount` at `+0x2C`. The bounds `check()` on `LastBlock` is
compiled out in Shipping. When the request runs past the end of the entry, `Blocks[i]` is read past
the end of the array: a non-null garbage value is used as a block pointer, and `RefCount++` faults.

**Which file.** The block index at the fault was 339 in one dump and 340 in the other. With the
default 64 KiB compression block size (the game's `DefaultGame.ini` sets Oodle/Kraken and no block
size override), exactly one entry of the 19 699 `.ubulk` and `.uexp` files in
`pakchunk0-Windows.pak` has 339 blocks:
`HalfswordUE5/Content/MetaHumans/Taro/MaleHair/Hair/Hair_M_SideSweptFringe.ubulk`
(22 161 796 bytes, blocks 0..338). Index 339 is one past its end. Index 340 fits the same file if
the request started past the end, or if slot 339 happened to be null and `GetBlock` wrote a fresh
pointer past the array.

That file is the strands bulk data of the only groom in the game, the `Hair` component of
`Willie_BP`. Every Willie carries it: the player, the arena's fighters and HSMP's remote-player
puppets. The game's `DefaultEngine.ini` sets `r.HairStrands.LODMode=True`, the continuous hair LOD
that streams strands curves in pages (`r.HairStrands.Streaming`,
`r.HairStrands.Streaming.CurvePage`). The two dumps overshoot the end of the file by different
amounts (at least 120 445 and 185 981 bytes), which fits a paged request whose last page is not
clamped to the bulk size, and does not fit a fixed off-by-one.

Confidence:

| Claim | Confidence |
|---|---|
| The code site (`FPakAsyncReadFileHandle::ReadRequest`, block index past the table) | certain |
| The file (`Hair_M_SideSweptFringe.ubulk`) | high: the only 339-block entry, the only groom, loaded by every pawn |
| Hair-strands streaming paging builds the over-long request | medium: inferred from the cvars and the varying overshoot, not proven without symbols or a live repro |

**What was ruled out.** Mod paks (only `pakchunk0-Windows.pak` and `pakchunk0optional-Windows.pak`
are mounted), the Oodle decompressor (the fault is in block bookkeeping before any decompression
starts), memory pressure, and UE4SS soft-object reads (no UE4SS frame on any thread).

**HSMP's part.** Multiplayer probably makes the crash more likely without causing it: more Willies
become visible in the first second of an arena (one per remote player), and Lua `LoadAsset` calls
are synchronous loads that flush async loading while streaming requests are in flight. This is
not proven.

## The workaround: `r.HairStrands.Streaming=0`

With streaming off, the strands bulk data loads in one request the size of the bulk data instead
of in pages. The cost is one 22 MB groom kept in memory. It is applied in three places, because
the game sometimes rewrites its `Engine.ini` and a cvar set from Lua after boot comes too late for
the menu Willie's first request:

| Where | What |
|---|---|
| Launcher, before every launch | writes `r.HairStrands.Streaming=0` under `[SystemSettings]` in `%LOCALAPPDATA%\HalfSwordUE5\Saved\Config\Windows\Engine.ini` (`launcher/src/ini.rs`). The uninstaller removes the line, or puts back the player's own value |
| Launcher, launch arguments | `-ini:Engine:[SystemSettings]:r.HairStrands.Streaming=0` (`launcher/src/launch.rs`) |
| HSMPMatch, at runtime | the Director sets the cvar at boot and on every new world through `KismetSystemLibrary.ExecuteConsoleCommand` (`D.RUNTIME_CVARS` in `mods/HSMPMatch/Scripts/director.lua`). The log line is `[HSMPMatch] hair-strand streaming workaround: r.HairStrands.Streaming=0 (runtime)` |

To apply it by hand without the launcher, add the line to `Engine.ini`:

```ini
; %LOCALAPPDATA%\HalfSwordUE5\Saved\Config\Windows\Engine.ini
[SystemSettings]
r.HairStrands.Streaming=0
```

If the crash comes back with streaming off, the next candidates are `r.HairStrands.LODMode=0`
(fixed LOD, no screen-size LOD changes) and, as a last resort, `r.HairStrands.Strands=0` (hair
cards only: no strands bulk reads at all, but the hair looks different).

## Lua-side mitigations

None of these can fix the engine bug. They reduce the load the mods put on the streamer while a
level loads, and they also remove hitches.

In place:

- **Per-world class caches.** HSMPLoadout's class cache and HSMPWorld's `W.classes` are dropped
  on every world change, without touching the cached objects. A UClass cached across a level
  change could have been collected, and calling `IsValid()` on it is a freed-pointer access.

Recommended for new code, not yet done everywhere:

- **No `LoadAsset` while a level loads.** `resolve_class` and `harvest_templates` in
  `mods/HSMPLoadout/Scripts/main.lua` and `resolve_weapon_class` in
  `mods/HSMPWorld/Scripts/main.lua` fall back to a synchronous `LoadAsset` when `StaticFindObject`
  misses. Never call it from an `OpenLevel` pre-hook or in the first frames of a new world: use
  `StaticFindObject` only and retry on a later tick.
- **Preload in the menu.** Load the catalogue's weapon and armour classes and the
  `DA_Equipment_Loadout_Tier_*` data assets once while the menu is idle, so an arena load finds
  them without `LoadAsset`.
- **One load per tick.** Queue loads so a kit with eight pieces cannot stack eight flushes into
  one frame.
- **Stagger Willie changes.** Show or dress puppets after the world is ready, one per tick, not in
  the arena's load frame.

## Triage and symbolisation recipe

`hsmp-tools crash-triage` reads `%LOCALAPPDATA%\HalfSwordUE5\Saved\Crashes` and has two
`PAK_ASYNC_READ_OOB` signatures:

- **high:** thread `IoDispatcher`, no Lua frame, and the CrashContext RVA frame
  `HalfswordUE5-Win64-Shipping+0x21e5ca4`. This RVA is only valid for the 5.4.4-2705 executable;
  a game patch moves it.
- **medium:** any access violation in the game module on the `IoDispatcher` thread with no Lua
  frame.

The records carry the CrashContext's RVA frames (`rva_frames`) and `callstack_hash`
(`PCallStackHash`), so a signature can match exact code addresses even when cdb symbolises a frame
to the wrong export. To re-check dumps:

```
hsmp-tools crash-triage --all --no-state-update
hsmp-tools crash-triage --since 2030-01-31T22:00:00Z --until 2030-01-31T22:30:00Z --out crash_triage.json
```

To look at a dump by hand with cdb (Windows SDK debugging tools):

```
cdb -z <dump.dmp> -y "<game dir>\HalfswordUE5\Binaries\Win64\ue4ss;<game dir>\HalfswordUE5\Binaries\Win64" -c ".ecxr; r; kn; .fnent @rip; ~*kc 40; q"
```

1. `.ecxr; r` gives the faulting instruction and registers. For this crash, `r12` holds the block
   index and `[rsp+70h]` the last block.
2. Export names in the game module are misleading: cdb picks the nearest export (this site shows
   as `src_strerror+0x126ec4`). Use the RVA (address minus the module base) and compare it with
   the CrashContext xml's frames.
3. `.fnent <address>` gives each frame's function bounds from the unwind data. Compare the function
   start RVAs with the table above.
4. With a new game build, identify the function by its constants and layouts instead: the
   `FCachedAsyncBlock` is 0x38 bytes, with `RefCount` at `+0x2C`, `BlockIndex = -1` at `+0x30` and
   the in-flight flags at `+0x34`; the backend frame compares chunk-id byte 11 with 13. Once a new
   RVA is confirmed, add it to the high-confidence signature in
   `tools/hsmp-tools/src/cmd/crash_triage.rs`.
5. `~*kc 40` lists every thread. Check that no thread has a Lua or UE4SS frame above engine code.

UE4SS overwrites `UE4SS.log` on every launch, and two instances on one machine share it. Keep a
copy of the log and the `hsmp_events*.jsonl` files from a crashing session before relaunching.
