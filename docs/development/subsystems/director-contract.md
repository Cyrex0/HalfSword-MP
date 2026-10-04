# Director contract: travel requests from HSMPMenu

The menu side is `mods/HSMPMenu/Scripts/travel.lua`; the Director side is
`mods/HSMPMatch/Scripts/director.lua` ([director.md](director.md)). Both are tested offline
(`hsmp-tools lua-test menu_ui director`).

**Rule:** the Director in HSMPMatch is the only code that changes the level. HSMPMenu never calls
`OpenLevel` or the console `open` command. It publishes a travel *want*; the Director decides and acts
on the server's state. The only exception is the debug shim `legacy_travel.lua` (section 5), which is
off unless the game runs with `HSMP_LEGACY_TRAVEL=1`.

All channels are typed bus keys in the game-local shared memory (schema
`crates/hsmp-ipc/src/schema/session.rs`; [ipc-shared-memory.md](../ipc-shared-memory.md)). UE4SS
gives each mod its own Lua state; the bus is how they talk.

## 1. When the menu asks

| Situation | `want` | `arena` | `reason` |
|---|---|---|---|
| The server's session phase becomes `countdown` or `live` while the lobby screen is active | `arena` | the server's arena as a short name, such as `Map_Arena_Pit` | `match_countdown` or `match_live` |
| `HSMP_AUTOTEST` splash skip (on `Map_Menu_SplashScreens`) | `menu` | none | `autotest_splash_skip` |

The menu sends at most one request per match start. A refused request is retried no sooner than 3 s
later.

The menu **never** requests:
- an arena the server did not report (with no arena in the session snapshot, nothing is sent and the
  lobby shows `THE SERVER REPORTED NO ARENA - WAITING`);
- the host's local pick;
- the browser's advertised map.

The Director does not wait for a menu request: it follows the session on its own. The request exists
so the menu can hand over cleanly and show the result.

## 2. The request: bus key `travel_request`

| Field | Type | Meaning |
|---|---|---|
| `want` | `"arena"` \| `"menu"` (and the UI wants of [director.md](director.md) 3.2) | Where the menu wants the player to be |
| `arena` | string | The short map name; empty when not needed |
| `reason` | string | Free text for logs and the `travel` event |
| `seq` | u32 | Strictly increasing. On load, the menu continues from the `seq` it finds in the key |
| `t` | f64 | Unix seconds when the request was written |
| `from` | string | `"HSMPMenu"` |

- **Poll rate:** the Director reads the key every tick (250 ms).
- **Ordering:** it acts only on `seq > last_seen_seq`. A cleared key has `seq` 0 and is ignored.
- **Startup:** when the Director initialises, it takes the `seq` present at that moment as already
  seen, so a request left over from an earlier load of the mods (bus keys outlive a Lua reload within
  one game process) is never replayed.
- **Authority:** the request is a *want*, never a command.
  - `want=arena` is accepted only while a session exists, the link is up and settled, no lost
    connection is latched, the phase is `loading`, `countdown`, `live`, `roundover` or `paused`, and the
    server has an arena.
  - If `arena` differs from the server's arena, the Director travels to **the server's arena** and says
    so in the ack.
  - `want=menu` is refused while a match is running and the link is fresh.

## 3. Optional direct channel: global `HSMP_DIRECTOR`

If a table `HSMP_DIRECTOR` with a function `request_travel(req)` exists **in HSMPMenu's Lua state**,
the menu calls it with the same request. A truthy return value is an immediate ack: `true` means
accepted, a table `{status=, reason=, arena=}` gives the full result. The bus key is written as well.
Because each mod has its own Lua state, this global is normally absent, and the bus key is the channel
that matters.

## 4. Acknowledgement and liveness (the Director writes these)

**Ack:** bus key `travel_ack {seq, status, arena, reason}`, written in the same tick the request is
read.

- `seq` echoes the request. The menu ignores an ack whose `seq` differs from its own.
- `status` is `accepted` (the Director owns the travel from now on) or `refused`.
  - On `accepted`, the menu leaves its lobby screen.
  - On `refused`, the lobby shows `TRAVEL REFUSED: <reason>` and the menu retries after at least 3 s.
- `arena` is the arena the Director will actually load.
- `reason` is a human-readable reason for a refusal, or a note when the arena differs.

**Heartbeat:** bus key `director`, refreshed every second. The menu needs only `hb` (unix seconds);
the other fields are informative ([director.md](director.md) 3.1). The menu treats the Director as
alive when `|now - hb| <= 5 s` and `state` is set.

**Return to the lobby:** when a match ends and the session is still connected, the Director writes a
new `seq` into the bus key `return_to_lobby` before or while loading `Map_Menu_Startup`. The menu
reopens its lobby and clears its per-match travel latch once per new `seq`. The `seq` present when the
menu loads is treated as a leftover.

**Timing on the menu side** (`travel.lua`): it checks for an ack after 1 s. A live Director that has
not acked yet gets up to 5 s. With no live Director, or after those 5 s, the request ends as
`refused` (`no director`) and the log reads
`DIRECTOR MISSING and no legacy shim - NOT travelling`. If the world changes before then, the request
ends as `world_changed` (someone already travelled). A newer request supersedes a pending one.

## 5. Debug shim: `legacy_travel.lua` (off by default)

`main.lua` loads `HSMPMenu/Scripts/legacy_travel.lua` only when the game runs with
`HSMP_LEGACY_TRAVEL=1`. It logs `!!! LEGACY TRAVEL SHIM ENABLED ...` at load and
`!!! LEGACY TRAVEL USED ...` on every use. With the shim armed, an unacked request (same deadlines as
section 4) makes the menu travel by itself:

- GI profile writes with read-back, and a one-time backup and restore (`.gi_backup.txt`);
- the native foe count;
- `OpenLevel` with a console `open` fallback;
- dismissing the menu and forcing game-only input.

The log reads `DIRECTOR MISSING - legacy travel (#seq want=… arena=… reason=…)`. GI restore on
CANCEL, or on the main menu with no session, also goes through the shim, and only while no live
Director heartbeat exists.

The shim is the only allow-listed `OpenLevel` caller outside the Director in `check_travel`
(`tools/hsmp-tools/src/bin/check_travel/main.rs`). Deleting `legacy_travel.lua` and its allow-list
entry removes it; nothing else changes.

## 6. What the Director owns (not the menu)

| Concern | Where |
|---|---|
| `OpenLevel` to the arena | Director: Travel |
| The MP GI profile (`Player Just Died`, `Current Game Mode Enum`, `Current Combat Mode`, `Current Play Mode`, `Free Mode Activated`, `Rounds To WIn`, `Rounds Won`, `Free Mode Carnage` / `Brawling` / `Blossfechten`) with read-back | Director: Prepare (`MP_GI_PROFILE`) |
| `Free Mode Foes Amount` | Director: from the roster (remote fighters) |
| `.gi_backup.txt` backup and restore | Director: Prepare and TravelMenu |
| Dismissing the menu and game-only input after a load | Director: Spawn (input frozen until Live) |
| "Load the local pick anyway" when START fails | Not done by anyone: the server starts the match or refuses |
| Autotest splash skip | Director, via `want=menu` |

## 7. Events

The menu emits these through `shared/hsmp_log.lua`, or a no-op stub when the library is not deployed:
- `lobby_ready{role, peer_id, arena}`, once per session when the sidecar is connected;
- `cmd_sent{cmd, cmd_id, arg, backend, via}`, once per command;
- `cmd_result{cmd, cmd_id, ok, reason, state, source, ms, tries}`, exactly once per `cmd_id`;
- `travel{from, to, by="menu_legacy"}`, from the shim only.

The Director emits `travel{by="director"}` and `world_ready`.

## 8. Commands and their results

Every host or player action is a command with a visible result (`commands.lua`):
`Cmd.send(kind, args) -> id`, and `Cmd.status(id)` returns `pending`, `accepted`, `refused` or
`superseded`, plus a reason.

Kinds: `pick_arena{arena}`, `best_of{n}`, `kit_rules{mode,budget}`, `start`, `abort`,
`ready{value}`, `kick{peer}`, `promote{peer}`, `ban`, `unban`, `reset_match`.

- **Transport:** a typed G2S `command` record (`cmd_id, op, flag, peer_id, text, patch`). The sidecar
  resends it until the server's `cmd_result` record (`cmd_id, ok, reason_code, reason_text`) arrives,
  and that maps 1:1 onto the command's state. The server caches results per (player key, `cmd_id`),
  so a resend is answered with the same result.
- **Fallback inference:** while no `cmd_result` has arrived, the result is also inferred from the
  session snapshot and from SERVER chat replies (`start blocked…`, `map can only be changed…`,
  `unknown arena…`, ...), with a timeout per kind. An inferred outcome is held for up to 2 s
  (`INFER_GRACE_S`) so the server's own `cmd_result` can still win: the snapshot and the result travel
  on different channels, and a lost result packet would otherwise turn a server answer into an
  inferred one (DoD-10).
- **A pick is a one-shot request, never a desired state.** It is resent only while it is unanswered.
  It is dropped (`superseded`) when a newer pick is made, when the server's arena changes to another
  value for any reason (RCON, another admin), or when the server answers or refuses it. The menu never
  re-sends a pick over a newer server arena. `best_of`, `ready` and `kit_rules` supersede the same way.
