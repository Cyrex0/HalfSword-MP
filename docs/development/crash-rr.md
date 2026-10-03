# The round-reset crash and the RVP travel guard

| | |
|---|---|
| **Crash** | Access violation in the game exe about 1 s after a level change, with no HSMP or Lua frame on the stack. |
| **Triage IDs** | `RVP_TASK_AFTER_TRAVEL` (game thread, exe `+0x4a23c64`), `RVP_WORKER_AFTER_TRAVEL` (an RVP plugin thread, exe `+0x1e28f4f`). `hsmp-tools crash-triage` prints these. |
| **Guard** | `mods/shared/hsmp_rvp.lua`, wired into the Director (`mods/HSMPMatch/Scripts/director.lua`) and `mods/HSMPMatch/Scripts/main.lua`. |
| **Tests** | `hsmp-tools lua-test rvp`, `hsmp-tools lua-test director` (the "RVP guard" blocks), `cargo test -p hsmp-tools crash_triage`. |

## 1. What crashed
In multiplayer, a round reset reloads the arena a few seconds after a Willie dies. Sometimes the game died about 1 s into that reload. The stack held only engine and plugin frames.

The game paints blood and wounds with the Runtime Vertex Paint and Detection plugin (`VertexPaintDetectionPlugin`, "RVP" here).

1. **The queue outlives the level.** RVP's task queue is `UVertexPaintDetectionGISubSystem.TaskQueue`, a GameInstance subsystem. It holds `CalculateColorsPaintQueue`, `CalculateColorsDetectionQueue` and the per-component ID maps `ComponentPaintTaskIDs` / `ComponentDetectTaskIDs`. Every queued task copies `FRVPDPTaskFundamentalSettings`, which starts with the raw pointers `UWorld* TaskWorld` and `UPrimitiveComponent* MeshComponent`.
2. **A bleeding Willie floods the queue.** While a Willie bleeds, the game issues roughly 90–180 `PaintOnMeshAtLocation` calls per second. The paint queue then holds 1–7 tasks at any moment.
3. **Tasks finish after their world is gone.** A task still queued or running at the teardown finishes on an RVP worker. Its "task finished" lambda then runs on the game thread through the task graph, inside `UGameEngine::Tick`. It computes

   ```
   Results.TaskDuration = TaskWorld->TimeSeconds - TaskStartTime
   ```

   (`movsd xmm1,[rax+6D0h]` with `rax` = the copied `TaskWorld`, exe `+0x4a23c64`). LoadMap's GC has already freed the old `UWorld`, so the read hits a decommitted page.
4. **The worker-side variant.** The worker itself can read the vertex data of a mesh freed by the same teardown: `movzx eax,byte ptr [rdx+rax]` at a page boundary, exe `+0x1e28f4f`.

This is a bug in the unmodded game. A single-player reload while a corpse bleeds can hit it too. The single-player reproduction in section 4 crashes at the same address and stack with no session, no IPC and no network. Multiplayer hits it more often because every round reset reloads the level right after a death.

### How the dump was decoded (cdb, no PDB)
- **Faulting frame.** A 0x40-byte function. `rax = [rcx+0x628]`; it reads a double at `rax+0x6D0`, subtracts a float at `rcx+0x5B0`, clamps at 0, and stores to `rcx+0x1C`, which is `FRVPDPTaskResults.TaskDuration` (+4) of a results struct at `rcx+0x18`. It then broadcasts through a delegate at `rcx+8`. The capture is larger than `FRVPDPCalculateColorsInfo` (0x2870 bytes): that struct plus the results and a callback array, i.e. a lambda capture.
- **Frame 01** invokes the `TFunction` (callable at `+0x60`, heap storage at `+0x70`, inline storage at `+0x80`).
- **Frames 02–05** are task-graph named-thread processing.
- **Frames 06–08** are the engine tick behind UE4SS's `UEngine::Tick` detour.

## 2. How the guard works
Rule: never change the level while RVP has work. `hsmp_rvp.lua` exposes `new(opts)`, which returns an instance with these functions:

| Function | What it does |
|---|---|
| `hold(why)` | Returns true while a travel must wait. First call: **close** the queue, **purge** it, log. Later calls: re-purge every `PURGE_EVERY_S` (0.1 s) while busy; return false once both queues have been empty for `QUIET_S` (0.2 s), or after `MAX_HOLD_S` (2.5 s) with an `rvp_hold_timeout` event. A travel is never blocked for longer than that. |
| `travel_issued()` | Called right after OpenLevel. Ends the hold; the queue stays closed. |
| `on_loadmap()` | LoadMap pre-hook. Purges again and reopens the queue, unless tasks are still queued, in which case it stays closed until the new world's first tick or `REOPEN_S`. |
| `reopen(why)` | Restores the saved limit (or the default, 10). Refused while a hold is in progress. |
| `purge_now(why)` | Close and purge synchronously, for a travel that cannot wait. |
| `tick()` | Safety nets: a hold not polled for `ABANDON_S` (1 s) is dropped and the queue reopened; a closed queue never outlives `REOPEN_S` (15 s). |
| `counts()` | `{ paint, detect, paint_comps, detect_comps }`, or nil when the plugin is not there. |

**Close.** Set `MaxAmountOfAllowedTasksPerMesh = 0` on the `UVertexPaintDetectionSettings` CDO. With the limit at 0 the plugin refuses every new task, so a corpse that is still bleeding stops feeding the queue. The write is read back; a failed write is logged.

**Purge.** Call `RemoveComponentFromPaintTaskQueue` / `RemoveComponentFromDetectTaskQueue` on the `VertexPaintFunctionLibrary` CDO for every component in the plugin's own `ComponentPaintTaskIDs` / `ComponentDetectTaskIDs` maps. GC nulls destroyed components in those maps, so only live components are passed.

**Wait.** The task that was already running finishes, and its game-thread callback runs while the old world is still alive.

**Reopen at LoadMap.** The queue must be open before the new world's BeginPlay: arenas build their weapons through RVP tasks, and with the limit at 0 a level loads with no weapon actors. So `on_loadmap()` reopens it. If something is still queued at that point (a hold that timed out, a native travel), opening the queue would start that task during the teardown, which is the crash itself. In that case the queue stays closed until the new world's first tick (`Dir:on_world` calls `reopen`) or `REOPEN_S`. The cost of that rare case is a missing weapon build, not a crash.

### Wiring
- **Director travels.** `Dir:open` calls `env.travel_hold(why)` every Director tick (250 ms) and returns `"held"` while it is true. `step_travel` stays in `Travel`, and held ticks do not count as OpenLevel attempts. A held menu travel is stored in `menu_held` and retried every tick. The soft-object reroute (`opts.now`) never waits, because the native travel is already pending.
- **New world.** `Dir:on_world` calls `env.rvp_reopen`. Every Director tick calls `env.rvp_tick`.
- **Native travels in an MP session.** An `OpenLevel` / `OpenLevelBySoftObjectPtr` that the Director did not issue cannot wait. Its pre-hook in `HSMPMatch/Scripts/main.lua` calls `purge_now`. This runs only while a session exists; single-player is never touched.
- **Kill switch.** `HSMP_RVP_GUARD=0` disables the guard. It exists for A/B runs and field diagnosis. The log line `RVP travel guard: ...` at mod start says which state is active.

### UE4SS rules the module follows
- Game thread only. Callers are game-thread loops and hooks.
- Only process-lifetime objects are cached: the subsystem, its `TaskQueue`, and the two CDOs. Each is looked up again when it reads as invalid.
- Components are taken only from the plugin's own maps, and only before the OpenLevel, while their world is alive.
- No `Soft*` `:get()`.
- **Never call the RVP task wrappers from Lua with a hand-built settings table.** UE4SS marshals the unset `FString` / `TArray` members of the 0x458-byte settings struct as garbage, which crashes in `memcpy`.

## 3. Testing
### Offline
```
hsmp-tools lua-test rvp
hsmp-tools lua-test director
cargo test -p hsmp-tools crash_triage
```
- `rvp` drives the module through its injectable `ue` layer and clock, then runs `real_ue()` against a mock UE4SS object model.
- `director` checks that a travel is held, logged once, retried, and that held ticks do not count as attempts, for both arena and menu travels.
- `crash_triage` checks that the two signatures classify correctly.

### In game: the probe
`experimental/crash-rr/HSMPRvpProbe` is a dev-only UE4SS mod. `build-and-deploy.ps1` does not deploy it. To use it, copy the folder to `ue4ss/Mods/`, copy `mods/shared/hsmp_wg.lua` and `mods/shared/hsmp_rvp.lua` into its `Scripts/`, add `HSMPRvpProbe : 1` to `ue4ss/Mods/mods.txt`, and run with `HSMP_DEV=1`.

- It logs the RVP queue sizes every 100 ms when they change, the per-second call counts of the RVP wrappers, and the queue at every OpenLevel and LoadMap.
- `HSMP_RVP_STRESS=<n>` turns it into a single-player crash amplifier. It opens `HSMP_RVP_ARENA` (default `Map_Arena_Alley`), makes every Willie bleed through the game's own `Get Damage` Blueprint function, keeps the hits coming up to the travel frame, and reloads the arena 0 / 0.3 / 0.6 / 0.9 s after the burst, n times. The iteration count is appended to `<state>/.rvp_stress.txt` before each reload, so it survives a crash.
- `HSMP_RVP_GATE=1` routes the reload through `hsmp_rvp.lua`. An A/B differs only in this flag.

`experimental/crash-rr/stress.ps1` runs one boot (`-N`, `-Gate`, `-Tag`, `-Arena`, `-TimeoutS`). `soak.ps1` reboots the game until `-Target` reloads are done (`-MaxBoots`, default 12) and reads each new crash dump's RVA and thread with cdb. Both find the game through `-Win64`, else `HSMP_GAME_DIR`, else `<repo>\game`.

> **Back up your career save first.** The stress runs in single-player, where the career save guard is inactive, and it wounds your own Willie. The game saves that state into `GameProgress.sav`, and later MP sessions seed from it. Copy `%LOCALAPPDATA%\HalfswordUE5\Saved\SaveGames` aside before a run and put it back afterwards.

### Measured A/B (single-player amplifier, Alley)
| Guard | Reloads | Crashes | Notes |
|---|---|---|---|
| off | 60 | 2 | Both at `+0x4a23c64` on the game thread. 50 of 60 travels had paint tasks in flight at LoadMap. |
| on | 120 | 0 | Every travel quiesced. 22 had tasks queued at hold start, which were purged. No timeouts. The queue was empty at every LoadMap. A hold took about 0.22 s. |

At the unguarded rate (2 in 60), the chance of 0 crashes in 120 reloads is about 1.8%.

Multiplayer gate soaks (two instances, `typical` netsim, world sync on, about 70 Director travels across Alley and Yard) showed no crashes. One Yard reload logged two `rvp_hold_timeout` events. The MP crash rate without the guard is too low (about 2 in 150 instance-reloads) for an MP-only A/B, which is why the single-player amplifier exists.

## 4. Remaining risk
- **A hold that times out** travels with work in flight. It needs a task that runs for more than 2.5 s with the queue closed.
- **Travels outside the Director** get a purge only (MP-session native OpenLevel, HSMPMenu `legacy_travel.lua`), or nothing (single-player). A task already running at that moment can still crash.
- **A plugin update** could change the struct layout or how `MaxAmountOfAllowedTasksPerMesh` is honoured. If `counts()` returns nil, `hold` returns false at once, so the game falls back to the unguarded behaviour instead of breaking.
