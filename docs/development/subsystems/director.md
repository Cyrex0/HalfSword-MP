# Director (HSMPMatch) and the shared world guard

The Director is the client-side code that makes the local game follow the server's match. The server
is authoritative over the map, the mode and the match flow (lobby, ready, countdown, round, result).
The Director reads that state, loads the server's arena, runs a verified spawn pipeline, reports what
the game has loaded, and is the **only** code in the mod tree that changes the level.

**Files:**

| File | What |
|---|---|
| `mods/HSMPMatch/Scripts/director.lua` | The Director: pure logic over an `env` table, plus `make_ue_env` (the real UE4SS environment) |
| `mods/HSMPMatch/Scripts/main.lua` | Wiring, the OpenLevel hooks, native end-of-match flow suppression, the slow-motion guard, spectating after death, runtime cvars |
| `mods/shared/hsmp_session.lua` | The session view the Director reads (typed `link` / `session` records and the sidecar heartbeat) |
| `mods/shared/hsmp_wg.lua` | The world guard (section 6) |
| `mods/shared/hsmp_saveguard.lua` | The career save guard (section 3.5) |
| `mods/shared/hsmp_rvp.lua` | The Runtime Vertex Paint travel guard ([crash-rr.md](../crash-rr.md)) |
| `tools/hsmp-tools/src/bin/check_travel/`, `check_wg/` | The two lints |
| `tools/hsmp-tools/lua-tests/director.lua`, `world_guard.lua`, `flow.lua` | Offline tests (`hsmp-tools lua-test director world_guard flow`) |
| `scripts/e2e-conn.sh` | Headless `reconnect` / `host_leave` / `server_restart` cases (sourced by `e2e-test.sh`) |

## 1. Rules

- The Director is the **only** code that changes the level. `check_travel` fails on any
  `OpenLevel`, `OpenLevelBySoftObjectPtr`, `ServerTravel`, `ClientTravel` or console `open`/`travel`
  outside `HSMPMatch/Scripts/director.lua`. The one allow-listed exception is
  `HSMPMenu/Scripts/legacy_travel.lua`, a debug shim that is only loaded with `HSMP_LEGACY_TRAVEL=1`
  ([director-contract.md](director-contract.md) section 5).
- It decides only from the server's state. The sidecar copies the server's session snapshot and its
  own connection view into shared memory ([ipc-shared-memory.md](../ipc-shared-memory.md)):
  - slot `link`: sidecar status, my peer id, admin flag, link state (`up` / `stalled` /
    `reconnecting` / terminal), rx age, reconnect attempt, kick / reject reason;
  - slot `session`: the server's snapshot (phase, arena, round, best-of, match id, epoch, roster with
    wins / alive / spawn orders, result);
  - the sidecar heartbeat in the segment header.

  `D.session_reader` turns these into one normalised table per tick.
- **A session process exists** when a `link` record is present, the sidecar heartbeat is younger than
  `session_stale_s` (45 s) and the status is not `ended`. A sidecar that panicked or was killed stops
  beating, so its last `connected` status stops counting. 45 s is longer than `sidecar_stale_s` (6 s)
  plus `resume_window_s` (35 s), so a sidecar that dies mid-match first ends in the `sidecar_stopped`
  modal. That modal stays until the player answers it or a session process exists again.
- **Absence is debounced.** A missing `link` record or heartbeat counts only after at least 2 reads
  spanning at least 1 s. One missed read does not end a session.
- The session snapshot present when the reader starts belongs to an earlier session of this game
  process. It counts as stale until the server sends a new one (a new record version).
- `lose("sidecar_stopped")` and `lose("link_lost")` mark the last snapshot stale. A dead or
  unreachable session's last `live` phase never sends a later single-player travel into its arena.
  The mark is cleared once the link is fresh again.
- After a leave request (LEAVE, BACK TO MENU) the Director does not follow the old session's state
  until that process is gone, its status is no longer `connected` / `reconnecting`, or 15 s passed.
- **Match identity.** Round numbers restart at 1 in every match, so round bookkeeping is keyed by
  `<match context>:<round>`. The context moves when the server's `match_id` changes, when the server
  epoch changes, or when the phase falls back from a match to the lobby. A `match_id` change that
  arrives after the lobby already moved the context (and before a round of the new context ended) is
  adopted without a second move, so a late id never reloads the arena during the countdown.
- **Native travel is rewritten.** The `OpenLevel` pre-hook rewrites every native travel to where the
  server wants the player (the arena during a match, the menu otherwise) whenever a session process
  exists. `OpenLevelBySoftObjectPtr` cannot be rewritten in place, so its post-hook re-issues the
  Director's own `OpenLevel` (the last request wins). While the Director stays in the arena on purpose
  (the result screen in `match_over`, a REMATCH hold in the lobby), a native travel is rewritten to that
  arena. Outside a match with the link down, native travel is left alone.
- **Travel waits for the vertex paint queue.** No level change is issued while the Runtime Vertex
  Paint plugin (blood and wound painting) still has tasks queued or running; `open()` returns `held`
  and the caller retries on a later tick. See [crash-rr.md](../crash-rr.md).

## 2. State machine

```
Menu ─(phase loading/countdown/live/roundover/paused + valid server arena)─► Prepare
Prepare    force the save guard ON (held until the menu world arrives) → GI profile write + read-back
           (3 tries; on failure load_error=gi_verify, retry after 5 s) → seed the session save slot
Travel     OpenLevel(arena)   (3 tries, then open_level_failed; "held" while the RVP queue drains)
WaitWorld  new world key whose short name is the arena (30 s → world_timeout, retry;
           landed elsewhere → retry)
Spawn      world → gi → pawn → vitals → place → kit → census → ready   (each step is world-key guarded)
Ready      ready_round = the round this world load serves; game_status reports it
Live       phase == live; input released unless the server has us dead
Ready/Live ─(countdown for a round this world does not serve)─► Prepare (same arena, fresh world)
Prepare/Travel/WaitWorld ─(server arena changed)─► Prepare(new) ; ─(lobby)─► TravelMenu
any ─(lobby after a match, session gone 3 s, connection lost (2.1))─► TravelMenu
TravelMenu return_to_lobby (if a session exists) → GI restore
           → OpenLevel(Map_Menu_Startup) → Menu (menu world arrived: save guard back to automatic)
```

A world change at any time restarts the pipeline for the new world key. If the possessed pawn
changes, the pipeline restarts at the pawn step. A world load that happens during `live` sits out the
round and serves the next one.

**Pipeline steps:**

| Step | Done when | Timeout → result |
|---|---|---|
| world | short name == server arena, key == the pipeline's key | `wrong_world` |
| gi | the GI profile is re-applied **after** the arena's own `Load Game` and read back | 3 s → `gi_verify` |
| pawn | a possessed pawn; game-only input applied | 15 s → `no_pawn` (keeps waiting) |
| vitals | the Willie_BP CDO values (`Health`, every `* Health`, `Consciousness`, `Bleeding`, `Pain`, `Stamina`, `Exhaustion`, ...) written and read back, after the BP reset functions (`Reset Sustained Damage`, `Reset Last Damage Taken`, `Reset Blood Bleed`, `Reset Latest Complex Damage`) | 4 s → `vitals` |
| place | HSMPSync's `spawn_status` for this round, arena and pawn with `verified=true`, **and** the pawn still within 100 cm (XY) of that spot | no verified placement after 8 s (or HSMPSync reported a failure) → one `spawn_request` retry; no order for 20 s → `no_order`; an order but no verified placement after 20 s → `spawn_timeout` |
| kit | HSMPLoadout's `kit_status` with `ok=true` for this pawn, written for this world load (`t` ≥ pipeline start − 0.5 s; pooled Willie FNames come back after a reload), that has held for 1.5 s or is `stable` | no status for 4 s → `unavailable`; 15 s → `kit_error` |
| census | visible Willies other than mine == remote fighters, and current-life combat proof below | keeps waiting with the explicit missing proof; elapsed time cannot release Ready |
| ready | waits for any HSMPSync re-placement in progress, then final vitals and GI read-back (re-applied once if they drifted) | emits the events |

Input is frozen from the moment the pawn exists until `Live`, and stays frozen in a `live` phase while
the pipeline has not finished. The **first** load error of a world load is kept and reported to the
server. Later errors are only logged.

The native combat proof requires a successful, advancing pose write for the placed owner's
exact match/round/life (`sample_status().pose`), a fresh matching local root, and advancing
known-health vitals for that life. Every living remote fighter must have matching applied
playback on its named stand-in, a current physical source frame (`PeerPlay` interp/extrap and
source age within 250 ms), advancing owner vitals, and native alive/physical collision readback.
Before Ready and the first Live release, all six upper/lower arms and hands must also have fresh
actual native transforms within 5 uu and 10 degrees of their previously integrated aim for
150 continuous ms. The typed local playback row binds this evidence to the native world,
displayed pawn, full match/round/life and source discontinuity; missing samples name their cause
and cannot be replaced by a timer. This local record does not change network protocol 12.
Pose clocks and stand-in visibility alone cannot prove this: playback publications continue
while a stopped sender is extrapolated. The final Ready step re-checks the proof, and input
remains frozen without Ready evidence for the exact current world and pawn. Older native
modules without successful-write pose evidence report an unavailable proof and remain blocked.
The first Live input release re-checks this proof, closing the interval between Ready and the
end of the countdown. Release is then latched for that exact placed life, so ordinary wounds
or knockdowns during Live do not re-run spawn proof and freeze controls. Pause/reconnect clear
the input-release latch and require fresh streams again; physical spawn qualification remains
for the exact already released life, so injured limbs do not prevent that life resuming.
A different world, owner life/pawn, remote life/pawn or source discontinuity discards the
corresponding physical qualification and requires new measured settlement.

**Foe count** (GI `Free Mode Foes Amount`) is `max(1, remote fighters in the roster)`. The roster is
the session snapshot's connected fighter rows. The extra foe in a solo match is a hidden stand-in.

**GI profile** (`D.MP_GI_PROFILE`): `Player Just Died` false, `Current Game Mode Enum` 0,
`Current Combat Mode` 0, `Current Play Mode` 0, `Free Mode Activated` false, `Rounds To WIn` 0,
`Rounds Won` 0, `Free Mode Foes Amount` (the foe count), `Free Mode Carnage` / `Brawling` /
`Blossfechten` false. The single-player values are backed up once to `<state>/.gi_backup.txt` and
restored on the way back to the menu. GI `Player Body Condition` (the wounds the spawner hands to the
next pawn) is healed to the CDO values together with the profile, so a death in one round does not
start the next round wounded.

## 2.1 Connection state machine

The session state is trusted only while the link is **up and settled**: sidecar status `connected`,
the heartbeat younger than `sidecar_stale_s` (6 s), the link state not `stalled`, and up for
`resume_settle_s` (1 s) after any down period. A stale phase never starts a travel. Only the
native-travel rewrite uses the last known state.

There is **one** player-facing state, published in the bus key `conn_state`:

```
ok ──(in a match, link down)──► reconnecting ──(link back + settled)──► ok
                                     │               └─ new epoch ─► notice "server restarted" (+ lobby)
                                     │               └─ lobby     ─► notice "match ended" (+ lobby)
                                     ├─(35 s, resume_window_s)──────► lost(link_lost | sidecar_stopped)
any ──(kicked / replaced / server_closed / rejected)────────────────► lost(<status>)
lost ──(RECONNECT, link up)──► ok        lost ──(RECONNECT, link down)──► reconnecting ("rejoining")
lost ──(BACK TO MENU)──► ok + leave request
lost ──(session ends / a new session starts)──► ok
```

- **reconnecting:** the world stays loaded and frozen (input off), no travel. HSMPHud shows
  "RECONNECTING... N s left".
- **Resume:** drop the overlay and follow the server again. If the round is still live, there is no
  reload. If the server paused and replays the round, the same arena is reloaded once (a new round),
  never via the menu.
- **lost** is latched: **one** travel to the menu (no lobby hand-back), the reason in the modal, and
  no automatic re-entry. A background reconnect of the sidecar does not travel. Only RECONNECT does,
  and only while the session process still exists and is not terminal.
- **notice** (`server_restarted`, `match_ended_away`): informational, with an OK button. The session
  is fine and the regular flow goes to the lobby screen.

`conn_state` (typed record, written on every change and at 1 Hz while reconnecting):
`wall, seq, state, reason, title, text, elapsed_s, remaining_s, window_s, in_match, latched,
actions[4], sidecar, attempt`.

| reason | title | actions |
|---|---|---|
| `link_lost` | CONNECTION LOST, "No answer from the server for 35 s." | reconnect, menu |
| `sidecar_stopped` | CONNECTION LOST, "The network helper (hsmp-sidecar) stopped responding." | menu |
| `kicked` | YOU WERE KICKED, the kick reason | menu |
| `server_closed` | SERVER CLOSED, the close reason ("Host closed the server") | menu |
| `rejected` | CONNECTION REJECTED, the reject reason | menu |
| `replaced` | DISCONNECTED, "You joined this server from another game." | menu |
| `server_restarted` (notice) | SERVER RESTARTED | ok |
| `match_ended_away` (notice) | MATCH ENDED | ok |

**REMATCH** (`want=rematch` in `match_over`): in the next lobby phase the Director stays in the arena
for up to `rematch_hold_s` (15 s). Every 2 s it sends a `READY` command, plus `START` when this player
is admin. A countdown reloads the same arena for round 1 (the next match numbers its rounds from 1
again, so this world serves none of them). With no START the Director goes to the lobby screen.

**Events:** `conn_state{state,reason,seq}`, `resume{ok,away_s,phase,round}`, `travel_reason{reason,text}`.

### The sidecar's link view

The sidecar updates the `link` record every 250 ms or on change. Link state: `up`, `stalled`
(connected but no datagram for 2.5 s), `reconnecting` (a new handshake with the same identity after
1, 2, 4, then every 8 s) or terminal. The Director's overlay starts at a 3 s rx age (`stall_s`),
long before the transport's 10 s idle timeout.

## 3. Contracts

All game-local channels below are typed bus keys in shared memory (schema
`crates/hsmp-ipc/src/schema/session.rs` and `loadout.rs`). Messages to the server are G2S records
sent through the IPC facade; the sidecar forwards them.

### 3.1 HSMPMenu ↔ Director

See [director-contract.md](director-contract.md). Summary:

- `travel_request` is polled every tick (250 ms). On startup the Director takes the `seq` present at
  that moment as already seen. Only a larger `seq` is handled.
- `travel_ack {seq, status, arena, reason}` is written in the same tick:
  - `want=arena`: `accepted` with **the server's arena** (the reason says so when it differs), or
    `refused` (`no session`, `connection lost (...)`, `not connected to the server`,
    `no match running (server phase lobby)`, `the server has no arena yet`);
  - `want=menu`: `accepted` unless a match is running and the link is fresh (`match in progress`).
- `director` heartbeat every 1 s: `hb, state, phase, round, ready_round, world_key, world, target,
  load_error, travel_n, native_rewritten, in_match, rematch, conn, conn_reason`.
- `return_to_lobby {seq}`: a new `seq` is written before loading the menu after a match. HSMPMenu
  reopens its lobby once per new `seq`.

### 3.2 HSMPHud → Director: `ui_request`

`ui_request {seq, want, reason, t, from}` has its own `seq`, independent of `travel_request`. It is
not acked; the Director logs its verdict. Wants: `reconnect`, `dismiss` (BACK TO MENU in the
connection modal), `ok`, `leave` (LEAVE MATCH / QUIT in the MP pause menu), `rematch`, `menu` (BACK TO
LOBBY on the result screen). `travel_request` accepts the same wants.

`leave` and an answered `lost` modal make the Director send a G2S `leave` record. The sidecar sends
`C2SLeave` to the server, restores the career saves and exits with status `ended`.

### 3.3 HSMPMatch → HSMPHud: `spectate`

`spectate {target, nick, round, alive}`; `target` 0 means not spectating, `0xFFFFFFFF` the arena view.
After the player's death HSMPMatch's own camera (`spectate_cam.lua`, one CameraActor per world)
follows a living opponent's stand-in from behind (Q / E cycle), or orbits high over the arena when
nobody is left to watch or the stand-in is gone. It never views through the stand-in itself: the
stand-in's active camera may be the first-person one inside its head, which showed black. Entering
clears any camera fade. HSMPHud shows "SPECTATING <NAME>" or "ARENA VIEW".

### 3.4 HSMPSync ↔ Director: `spawn_status` and `spawn_request`

HSMPSync (`HSMPSync/Scripts/spawn_place.lua`, [spawns.md](spawns.md)) writes `spawn_status` after
**every** placement attempt (round start, new pawn, fall rescue, Director retry) and again when it
has verified that the pawn stayed there. Fields: `seq, round, arena, pawn, spawn_id, slot, pos
(has_dest), floor (has_floor), clear, why, verified, tries, tol_cm, protect_ms, protect_until
(has_protect_until), t, error`. The key is world-scoped.

- `round` / `arena` are the order's; `pawn` is the possessed pawn's FName; `pos` the ground-snapped
  destination.
- `verified=false` with no `error`: placed, being checked. `verified=false` with an `error`: HSMPSync
  gave up.
- `protect_until` is on the **process clock** (`os.clock`, shared by every mod of the game). Unset
  means protected until verified **and until the round is Live**; HSMPSync writes the bounded end when
  Live starts.

The Director asks for one re-place per world load with `spawn_request {seq, round, arena, pawn,
spawn_id, why}`. HSMPSync acts on a `seq` it has not seen when round, arena and pawn match its own
order.

**Spawn protection in the status report.** While `spawn_status` says the pawn is protected, a pawn
with `Health <= 0` is reported alive, and the Director logs `pawn Health <= 0 inside the spawn
protection window; not reported dead` once.

### 3.5 HSMPLoadout → Director: `kit_status`

HSMPLoadout (`kit.lua`) writes `kit_status` after every verify of the own pawn's kit, when dressing
starts, when a native re-arm undoes it, when it gives up, and when its stability window held. Fields
include `pawn, ok, armour_n, exp_armour_n, r_class, l_class, r_visible, l_visible, tries, error, kit,
stable, round, t`.

`ok=true` needs every kit armour class worn and both hands holding the kit classes **visibly**. The
Director trusts `ok=true` once it has held 1.5 s (`t`, process clock) or `stable=true`, so a native
re-arm right after dressing cannot slip through to Ready. See [classes-loadout.md](classes-loadout.md).

### 3.6 Director → server: `game_status`

About once a second while connected, the Director sends a G2S `game_status` record (C2SGameStatus):
`match_id`, `round` (the round whose world is ready, 0 = none), `world_key` (FNV-1a hash of the world
key), `flags` (`LOADED`, `DEAD`, `IN_MENU`), `spawn_id` (the applied spawn order), `load_error` (a
`load_error` code) and `arena` (the loaded arena). The server uses it for the load barrier, game
liveness and as a redundant death report (`server/src/server/match_core.rs`).

| Director load error | wire code |
|---|---|
| `open_level_failed` | `TRAVEL_FAILED` |
| `wrong_world` | `WRONG_WORLD` |
| `no_pawn` | `NO_PAWN` |
| `vitals` | `VITALS` |
| `spawn_timeout` | `SPAWN_PLACE` |
| `kit_error` | `DRESS` |
| `world_timeout` | `TIMEOUT` |
| anything else (`gi_verify`, `save_seed`) | `OTHER` |

### 3.7 Save guard (`shared/hsmp_saveguard.lua`)

- It is installed only in HSMPMatch.
- `SG.tick()` runs at the top of every 250 ms tick; `SG.selftest()` runs once in `Map_Menu_Startup`.
- `SG.seed_session_slot()` runs in Prepare, between the verified GI apply and `OpenLevel`, never
  inside a hook. The arena's game manager runs GI `Load Game` on every arena load; seeding the session
  slot with the verified profile makes that load read the MP values back.
- `SG.set_active(true)` runs when the Director prepares a match or starts a spawn pipeline in an
  arena, whatever the guard's own liveness says. `set_active(nil)` hands it back only once the menu
  world has arrived. The guard's own check turns off (and runs GI `Load Game` on the career slot) in
  the same tick that sees `kicked` / `server_closed`, while the MP arena is still loaded and a native
  save could still reach the career slot; holding it on until the menu closes that window.

## 4. Other HSMPMatch duties (`main.lua`)

- **Native end-of-match flow.** The GameMode's enemy count is pinned so stand-in deaths never trigger
  the native win flow; the native death / win / lose widgets are collapsed; native lose-flow travel is
  blocked while an MP match runs.
- **Slow motion off.** Every slow-motion path of the game (global time dilation, GI `Time Dilation`,
  the per-Willie `CustomTimeDilation` / slomo flags, `CheatManager:Slomo`) is blocked or reset during an
  MP session, and a paused world is unpaused.
- **Runtime cvars.** `r.HairStrands.Streaming 0` is applied at boot and on every world change. It
  works around an engine crash in pak reads; see
  [io-dispatcher-crash.md](../halfsword/io-dispatcher-crash.md). Test rigs may add `r.VSync` and
  `t.MaxFPS` through `HSMP_TEST_CVARS`; no other cvar is accepted from it.
- **Thread safety.** `LoopAsync`, `ExecuteWithDelay` and async key binds are routed onto the game
  thread.

## 5. Events (`shared/hsmp_log.lua`; a no-op stub when it is not deployed)

`travel{from,to,by,reason}` (`by` = `director`, `native` or `native_rewrite`),
`world_ready{arena,world_key,round,server_arena}`,
`spawn_verified{round,ok,arena,world_key,vitals_ok,gi_ok,x,y,z,spawn_id,dist_cm,dest_cm,offset_cm,tol_cm,snap_z,steps,load_error,ms}`,
`willie_census{visible,expected,extras,missing,at,round}` (`at` = `ready`, and every 5 s while Live),
`ready_report{round,arena,load_error}`, `native_travel_rewritten{from,to,phase,n[,soft]}`.

## 6. In-game log lines (`UE4SS.log`, `[HSMPMatch]`)

| Line | Means |
|---|---|
| `Director v1 up (state=Menu, last request seq=N)` | loaded |
| `save guard: shared/hsmp_saveguard.lua installed` | career saves are diverted during a session |
| `session source: shared-memory records (link, session)` | the session view is the shared-memory one |
| `director: Menu -> Prepare (server phase countdown, arena Map_Arena_X)` | the match starts |
| `director: GI profile verified for Map_Arena_X (foes=1 from 1 remote fighter(s))` | Prepare OK |
| `director: session save slot seeded (...)` | the arena load will read the MP GI |
| `director: travel to Map_Arena_X held (...): waiting for the Runtime Vertex Paint queue` | the travel waits for the blood painting queue |
| `director: travel Map_Menu_Startup -> Map_Arena_X (...)` | the only travel line |
| `director: travel request #7 accepted arena=Map_Arena_X` | the menu handed over |
| `director: world ready Map_Arena_X (phase=countdown round=0) -> serving round 1` | WaitWorld done |
| `director: input frozen (...)` / `input released (...)` | the freeze |
| `director: READY round 1 on Map_Arena_X in 3.2 s [gi=ok vitals=ok place=hsmpsync(id=256 tries=1 12cm) kit=ok(...) census=ok]` | the verified spawn |
| `director: placement on order 256 not confirmed after 8 s (...); asking HSMPSync to re-place (retry 1)` | the one retry before `spawn_timeout` |
| `director: native OpenLevel('Map_Hub_Tavern_Frank') rewritten -> 'Map_Arena_X' ... [#n]` | a native travel was caught. Each one is a defect worth investigating |
| `director ERROR <code>: ...` | a load error (`gi_verify`, `no_pawn`, `vitals`, `spawn_timeout`, `kit_error`, `world_timeout`, `wrong_world`, `open_level_failed`, `save_seed`) |
| `director: stand-in census DEFECT visible=N expected=M` | a missing or extra Willie |
| `director: connection reconnecting (stalled)` | link down in a match: overlay, frozen, no travel |
| `director: link restored after 8.4 s (phase=countdown round=2)` | resumed; the regular flow continues |
| `director: connection LOST (link_lost): No answer from the server for 35 s.` | the one transition to the menu (modal) |
| `director: ui request #3 accepted` / `RECONNECT pressed; ...` | the player's modal / pause / result buttons |
| `hair-strand streaming workaround: r.HairStrands.Streaming=0 (runtime)` | the pak-read crash workaround is applied |
| `spectating Mate (peer 2)` | the camera follows a living opponent after the player's death |
| **absent:** `watchdog: in ...` | no stray world |

## 7. Shared world guard (`shared/hsmp_wg.lua`)

Every mod that caches UObjects must drop them when the world changes; touching an object of a freed
world crashes the game. The world guard is the one implementation:

```lua
local WG = require("hsmp_wg").new({ log = Log, UEHelpers = UEHelpers })   -- installs the OpenLevel pre-hooks
local wg_check, wg_on_drop, wg_token, wg_same = WG.check, WG.on_drop, WG.token, WG.same
WG.cache("banner", function() banner = {} end)        -- the named form of on_drop
```

- The world key is `name@address#PC`.
- A travel holds for 10 s; a new world drops the registered caches and skips that tick.
- `WG.key`, `WG.name`, `WG.travel_from` and `WG.drops` are fields; `WG.short()` and `WG.pending()`
  are helpers.

`build-and-deploy.ps1` copies `shared/*.lua` into every mod's `Scripts/`. Mods also find the library
in the source tree (`Scripts/../../shared/`).

`check_wg` lints that each mod uses the shared guard and calls it where it must; `check_wg --sites`
lists the exact lines.
