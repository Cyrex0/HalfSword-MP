# The IoDispatcher pak-read crash

A native crash in the game executable, on the `IoDispatcher` thread, a few seconds into the first
arena of a game session. The faulting code is the engine's: a groom-data read through the
shipping pak reader. HSMP triggered it by hiding every arena Willie at BeginPlay and showing it
later. The fix draws the hair with the game's hair cards instead of strands, so that data is
never read (`r.HairStrands.UseCardsInsteadOfStrands=1`); HSMPLoadout also lets the hair render
for a moment before the first hide. `hsmp-tools crash-triage` classifies the crash as `PAK_ASYNC_READ_OOB`.

## Symptom

- The faulting thread is `IoDispatcher`: an access violation on `inc dword ptr [r15+2Ch]` with a
  garbage `r15`, at RVA `0x21e5ca4` of `HalfswordUE5-Win64-Shipping.exe` (UE 5.4.4, changelist
  2705). Every known dump has `PCallStackHash` `44106A9DC8756FBE63C328366971E9612B82C734` and the
  same seven game-module frames: `21e5ca4 1444293 1444e8e 11fdf3d 11fe57a 12e1067 12e0f89`.
- It happens in the first arena a process loads, 3-5 s after the travel, within about 100 ms of
  a Willie that had been hidden since BeginPlay becoming visible. It has never happened on a later
  arena load of the same process.
- **The game keeps running after the fault.** The crash reporter parks only the faulting thread.
  The game thread keeps ticking, and Lua keeps logging, for up to about 40 s, until the process is
  killed. The next level load then hangs, because the IoDispatcher is gone. So `hsmp_events` and
  `UE4SS.log` show events after the crash, and a scenario fails on a later step (typically "round
  2 reload never finished") although the crash happened in the first arena. Take the crash time
  from the crash folder's file times (`CrashContext.runtime-xml`), not from the last log line.
- It is not memory pressure: `MemoryStats.bIsOOM=0`, and the pointer is garbage at one specific
  index, not a failed allocation.

## What is read, and why it faults

The minidumps hold stack memory only, and there are no public symbols for the game, so the frames
were identified from function bounds, struct layouts and constants:

| Frame | Function start (RVA) | Function |
|---|---|---|
| 04 | `11fe530` | `FIoDispatcherImpl::Run` (the thread loop) |
| 03 | `11fd820` | `FIoDispatcherImpl::ProcessIncomingRequests` |
| 02 | `1444c60` | the package-resource I/O backend's resolve. It tests chunk-id byte 11 for `EIoChunkType` 13 (`PackageResource`), opens a fresh handle with `IPackageResourceManager::OpenAsyncReadPackage(path, segment)` for every request, and keeps it in a map keyed by the request |
| 01 | `14441e0` | issues `IAsyncReadFileHandle::ReadRequest` (vtable `+0x10`) with the request's offset, size and buffer |
| 00 | `21e5960` | `FPakAsyncReadFileHandle::ReadRequest` (`IPlatformFilePak.cpp`) |

Frame 00 follows the UE 5.4 source. For a compressed entry it computes
`FirstBlock = Offset / CompressionBlockSize` (`r13`) and `LastBlock = (Offset + BytesToRead - 1) /
CompressionBlockSize` (`[rsp+70h]`), then for each block takes `Blocks[i]` (allocating a
`FCachedAsyncBlock` if the slot is null) and increments its `RefCount` at `+0x2C`. The bounds
`check()` is compiled out in Shipping.

In every dump the fault is on the **first** block of the request: `r12 == r13 == FirstBlock` (339
in all dumps but one, which has 340), with `LastBlock = FirstBlock + 1`. So the whole request
lies past the end of the entry, and `Blocks[339]` is heap garbage.

**Which file.** `Hair_M_SideSweptFringe.ubulk`
(`HalfswordUE5/Content/MetaHumans/Taro/MaleHair/Hair/`, 22 161 796 bytes, 339 blocks of 64 KiB),
the strands bulk data of the only groom in the game, the `Hair` component of `Willie_BP`. Two
pieces of evidence:

- Stale stack values in every dump: `0x20e0` (8416), the 16-byte-aligned raw size of that entry's
  last compressed block (block 338, 8403 bytes), and `0x1_b492de89`, a pak offset 1 MiB before that
  block. The IoDispatcher thread had just read the end of this file.
- It is a 339-block entry, and the request starts at block 339: one block past its end.

The file's 45 bulk payloads end exactly at its size (the package's data resource table), so no
payload legitimately starts past it.

**What was ruled out.** `r.HairStrands.Streaming` (the crash happens with it 0 and with it
unset), mod paks (only `pakchunk0` and `pakchunk0optional` are mounted; the optional one holds a
sky texture), the Oodle decompressor (the fault is before any decompression), memory pressure, a
freed handle (the handle is opened fresh for the request), and UE4SS or Lua on the faulting
thread.

## The trigger

HSMPLoadout's invisible dressing hides every new arena Willie at BeginPlay and shows it once its
kit is on, or after 3 s ("safety reveal"). If the groom has never rendered in this process, showing
it makes the engine read past the end of the strands data. A helmet kit turns the hair off before
the reveal, so the normal path only showed a groom when the kit came late (a slow link: the high-ping
far and bad profiles, the world-sync intl / bad / far runs).

Reproduced in single-player with no HSMP mod loaded, only the dev probe
`experimental/io-crash/HSMPIoProbe` (`soak.ps1 -PerBoot 1`: each trial is one boot and its first
arena load, `Map_Arena_Pit`):

| Probe mode | Boots | IO-1 crashes |
|---|---|---|
| `plain`: vanilla load, nothing touched | 15 | 0 |
| `reveal`: hidden at BeginPlay, shown 3 s later | 15 | 9 |
| `reveal`, hidden 300 ms after BeginPlay instead | 29 | 0 |
| `reveal` with later loads in the same process (arena to arena, or through the menu) | 38 loads | 0 |

The dressing itself is not needed: the probe never dresses, and in the MP dumps the fault came
before the late kit was applied.

## The fix: hair cards instead of strands

`r.HairStrands.UseCardsInsteadOfStrands=1` makes the engine draw the groom's hair cards (the
`Hair_M_SideSweptFringe_CardsMesh_Group0_LOD*` meshes the game already ships) instead of
strands, so the strands bulk data, the file that is read past its end, is never read. Close-up
hair is less detailed; it is not missing. The cvar must be set before the first groom loads, so
it is set in the same places as the old `r.HairStrands.Streaming=0` line:

| Where | What |
|---|---|
| Launcher, at install and at every start (re-applied if the game dropped it) | `ini_settings` in `tools/release/release.json`: `r.HairStrands.UseCardsInsteadOfStrands=1` under `[SystemSettings]` in `%LOCALAPPDATA%\HalfSwordUE5\Saved\Config\Windows\Engine.ini`. The uninstaller removes it or puts back the player's own value |
| Launch through Steam | `-ini:Engine:[SystemSettings]:r.HairStrands.UseCardsInsteadOfStrands=1` (`launch_args` in the same file) |
| `scripts/build-and-deploy.ps1` | adds both lines to Engine.ini when they are missing |
| HSMPMatch, at boot and on every new world (backstop) | `D.RUNTIME_CVARS` in `mods/HSMPMatch/Scripts/director.lua`; the log line `[HSMPMatch] IO-1 hair cvars applied (runtime) [read back r.HairStrands.Streaming=0 r.HairStrands.UseCardsInsteadOfStrands=1, ...]` |

By hand, without the launcher:

```ini
; %LOCALAPPDATA%\HalfSwordUE5\Saved\Config\Windows\Engine.ini
[SystemSettings]
r.HairStrands.Streaming=0
r.HairStrands.UseCardsInsteadOfStrands=1
```

### Defence in depth: the hair warm-up

`mods/HSMPLoadout/Scripts/main.lua` (`HAIR_WARM_S`, the BeginPlay hook):

- In the first arena world of a process, a new Willie is hidden `HAIR_WARM_S` (0.5 s) after
  BeginPlay instead of in the hook, so its groom renders first. It shows in its base clothes for
  that half second. Later worlds hide at BeginPlay as before.
- The delayed hide runs on the game thread and never touches an actor of a world that is gone
  (a counter bumped by the LoadMap pre-hook). A Willie revealed in the meantime is not hidden.
- `HSMP_HAIR_WARM_S=0` restores the old hide, for A/B runs.

Also in place: a visible Willie is never re-dressed ("Set Up Armor") within `HAIR_SETTLE_S`
(2.5 s) of being first seen visible, while a hidden one is dressed at once (`dress_wait`, with a
backstop in `call_setup`). That was the first theory. The measurements do not show it is needed,
and it delays a late kit by up to 2.5 s. `HSMP_HAIR_SETTLE_S=0` turns it off.

`HSMP_DEV_KIT_DELAY_S` (only with `HSMP_DEV=1`) holds the own kit back so the safety reveal comes
first on every load: the slow-link path, for soaks.

### Measured in multiplayer (warm-up only, before the cards cvar)

`mp_test.ps1 -Scenario p0_gate -Arenas Pit -Rounds 4`, `HSMP_DEV_KIT_DELAY_S=3.3` (late kit and
safety reveal on every load), CPU load: 10 runs. Two runs never reached an arena (boots starved by
the load generator's pak readers, since removed); 8 runs had 2 first arena loads each plus round
reloads. **1 IO-1 crash** (run 8), about 0.5 s after world ready: the delayed hide itself landed
inside the groom's first load. Before the warm-up, the same path crashed about half of first loads
(the high-ping and world-sync runs: 6 crashes, all on a safety reveal of the first arena; single-player 9
of 15). A timer only lowers the odds, which is why the cards cvar is the fix.

With the cards cvar: `r.HairStrands.UseCardsInsteadOfStrands` read back 1 on both instances of
a normal MP run (`p0_gate`, Pit, 2 rounds: verdict PASS, no crash dump), at boot and in the
single-player menu. A screenshot of a bare-headed Willie was not obtained (MP kits wear helmets);
check the card hair visually in the next manual session.

## `r.HairStrands.Streaming=0`

The launcher, the deploy script and HSMPMatch still set `r.HairStrands.Streaming=0` (Engine.ini,
the Steam launch argument and a runtime cvar). It was the earlier workaround. It does not prevent
this crash: the six multiplayer dumps of 2026-10-04 all had it read back as 0. It is harmless and
stays in place next to the cards line.
## Lua rules that remain good practice

- **Per-world class caches.** HSMPLoadout's class cache and HSMPWorld's `W.classes` are dropped
  on every world change, without touching the cached objects.
- **No `LoadAsset` while a level loads.** `resolve_class` and `harvest_templates` (HSMPLoadout)
  and `resolve_weapon_class` (HSMPWorld) fall back to a synchronous `LoadAsset`. Keep them out of
  `OpenLevel` pre-hooks and the first frames of a world.
- **Never toggle a groom's visibility around its first load** (spawn hide / reveal) if strands are
  ever turned back on. New hide-at-spawn code goes through HSMPLoadout's warm-up.

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
   `crates/hsmp-diag/src/triage.rs` (the table `hsmp-tools crash-triage` uses).
5. `~*kc 40` lists every thread. Check that no thread has a Lua or UE4SS frame above engine code.

UE4SS overwrites `UE4SS.log` on every launch, and two instances on one machine share it. Keep a
copy of the log and the `hsmp_events*.jsonl` files from a crashing session before relaunching.
