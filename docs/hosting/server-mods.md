# Server mods

A server can serve its own UE4SS Lua mods to its players. When a player joins, the game shows
the list of mods and a clear warning; if the player accepts, the mods download over the game
connection, every file is checked against the server's hash, and the game loads them without a
restart. A player who declines goes back to the server browser.

**Server mods run with full access to the player's PC**, like any UE4SS mod the player installs
by hand. There is no sandbox. Players are told exactly that before anything downloads, and most
will only accept on servers they know. Keep your mods small, readable and honest.

Related: [configuration.md](configuration.md) (every flag), [../players/playing.md](../players/playing.md#server-mods)
(what players see), [../development/lua-mods.md](../development/lua-mods.md#7-server-mods)
(writing a server mod), [../development/protocol.md](../development/protocol.md#12-server-mods)
(the wire).

## Hosting mods

1. Make a folder with one sub-folder per mod:

   ```
   my-mods/
     ArenaRules/
       mod.json                 optional: version, author, description
       Scripts/main.lua         required: the entry point
       Scripts/rules.lua        loaded with require("rules")
       data/weights.csv
     ChatCommands/
       Scripts/main.lua
   ```

2. Start the server with it:

   ```
   hsmp-server --mods-dir my-mods
   ```

   or set `HSMP_MODS_DIR=my-mods` (Docker: mount the folder and set the variable).

3. The server reads and checks every mod at startup and logs one line per mod (name, version,
   author, files, size, hash) and the set's hash. **If any mod breaks a rule, the server does
   not start** and says which mod and which file.

`mod.json` (optional, all keys optional, other keys ignored):

```json
{ "version": "1.2", "author": "Ann", "description": "Faster rounds and a sudden-death timer" }
```

`version` is at most 16 bytes, `author` 48, `description` 160. They are shown to players exactly
as written (control and invisible characters are refused).

The files are read once, at startup, and served from memory: what players download is exactly
what the server announced, whatever happens to the folder afterwards. Restart the server to
publish a change; every player is asked again because the set hash changed.

## Rules

| Rule | Limit |
|---|---|
| Mods per server | 16 |
| Files (all mods) | 256 |
| Total size | 64 MiB (`--mods-max-mb` sets a lower limit) |
| One file | 16 MiB (or the total limit, if lower) |
| Mod (folder) name | 1-32 of `A-Z a-z 0-9 _ -`; **must not start with `HSMP`** (any case); not a Windows device name (`CON`, `NUL`, `COM1`, ...); unique without regard to case |
| File path | relative, `/`-separated, 1-8 levels of `A-Z a-z 0-9 . _ -`, no part starting or ending with `.` (no `..`, no hidden files), no device names, at most 128 bytes, unique without regard to case |
| File types | `.lua`, `.json`, `.txt`, `.csv`, `.ini`, `.md` only. No `.dll`, `.exe`, `.pak`, `.bat`, `.ps1`, precompiled Lua or anything else in this version |
| Entry point | every mod has `Scripts/main.lua` |
| Links | symbolic links are refused; hidden entries (`.git`) and loose files next to the mod folders are skipped with a note |

`HSMP*` names belong to HalfSword-MP's own mods, which get the native API; a server mod can never
use or overwrite one. Players check every rule again on their side before they download.

## Flags

| Flag | Env | Default | |
|---|---|---|---|
| `--mods-dir <dir>` | `HSMP_MODS_DIR` | none | The mods to serve |
| `--mods-max-mb <n>` | `HSMP_MODS_MAX_MB` | 64 | Largest total size, 1-64 MiB |
| `--mods-timeout-s <s>` | `HSMP_MODS_TIMEOUT_S` | 300 | Time a joining player has to accept, download and load (30-3600) |
| `--mods-rate-kbps <n>` | `HSMP_MODS_RATE_KBPS` | 2048 | Download rate per joining player, KiB/s |
| `--mods-total-rate-kbps <n>` | `HSMP_MODS_TOTAL_RATE_KBPS` | 8192 | Download rate of all joining players together, KiB/s |

## What players see

- The server browser shows **[MODS n]** after the server's name, and selecting the server says
  how many mods it installs and how big they are (from the server list and the server's own
  ping answer; older browsers ignore it).
- On JOIN, before anything downloads: *"This server wants to install N mods that run with FULL
  ACCESS to your PC ... Only accept for servers you trust."*, then every mod with its version,
  author, description and size, and the server's key and the set's hash.
- **ACCEPT & JOIN** downloads, checks and loads them, then the player enters the lobby.
  **DECLINE** leaves the server. **REMEMBER FOR THIS SERVER** (on by default) skips the question
  next time for this server key and exactly this set; any change asks again.
- A player can set SETTINGS > SERVER MODS to **NEVER** (then servers with mods cannot be joined)
  and **FORGET REMEMBERED SERVERS**.
- Mods already downloaded (from this or any other server) are not downloaded again.

## While players join

- A joining player is **pending** until its game reports the mods loaded: it is not in the roster
  the others see, cannot READY, is not counted for START or the auto start, never becomes a
  participant of a match, and the server drops every game record it sends (only its mod requests,
  `leave`, `ping`, commands, answered with "still loading the server's mods", and its loadout are
  taken).
- Downloads are pulled by the player in 32 KiB chunks, at most 4 in flight, and served from a
  separate loop within `--mods-rate-kbps` per player and `--mods-total-rate-kbps` for everyone,
  on the reliable channel the transport fills after the game streams. Players in a match keep the
  rest of your upstream.
- A player still pending after `--mods-timeout-s` is disconnected with "The server's mods were not
  loaded within N s". A player that downloads the set more than about three times in one session
  is disconnected.
- A client without server-mods support (an old build) is refused with "This server uses N server
  mods. Update HalfSword-MP via the launcher to join." (reject code `MODS_REQUIRED`).
- The server log has one `server mods offered` line per join, `server mods loaded: the player
  joins` with the time it took, and `server_mods` events in `--events`.

## Limits of this version

- Lua and data files only: no C++ mods, no `.pak` content, no Blueprint mods.
- Unloading is best effort: when a player leaves the server, hooks, custom events and timers are
  removed; key binds, console commands and a few UE4SS hooks cannot be removed and stay inert
  until the game restarts; changes a mod made to the game world stay. Players are told to restart
  the game for a fully clean state.
- HOST GAME in the menu has no option for mods: serve them from a dedicated server.
