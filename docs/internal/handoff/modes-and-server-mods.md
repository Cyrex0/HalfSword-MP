# Handoff: game modes and server mods (in-game test pass)

Branch `dev` (`3227d16` or later). Nothing here has run in the real game yet. The server rules,
the sidecar, the wire records and the Lua screens build, pass clippy and pass their unit and Lua
tests on Linux; everything that touches the live game is unproven. This page is the test plan.

## 1. What is new

**Game modes** (picked on the lobby's GAME MODE screen, `--mode`, or RCON `MODE`):

| Mode | UI name | Rule |
|---|---|---|
| `duel` / `ffa` | DUEL / FFA | Unchanged (last player standing), plus kills, deaths and a kill feed |
| `teams` | TEAMS | 2 to 4 teams, AUTO-balanced or PICK (players choose). Teams spawn on their own side, sides swap each round. Teammates cannot hurt each other unless friendly fire is on. Last team standing wins the round |
| `koth` | HILL | Hill centred on the arena's spawns (radius 250 to 600 cm, `HSMP_KOTH_ZONES` overrides). Alone on the hill scores time, contested scores nothing; first to the target wins. Deaths still eliminate; at the clock, most points wins |
| `roulette` | ROULETTE | Everyone gets the same random weapon (never a shield) and armour set each round; own kits come back in the lobby |
| `brawl` | BRAWL | Fists only, plain clothes |
| `deathmatch` | DEATHMATCH | Respawn after a death at the spawn farthest from living players, 2 s protection. Most kills at the clock (default 5 min) wins; a tie plays 60 s of sudden death, then a draw |

Respawn is a reload: the dead player's game reloads the arena through the normal round-start
sequence (placement, vitals, kit, stand-ins). Expect 5 to 15 s from death to fighting again.

**Server mods** (`hsmp-server --mods-dir <dir>`): when a player joins, a warning lists every mod
and says they run with full access to the PC. ACCEPT & JOIN downloads them over the game
connection (32 KiB chunks, SHA-256 checked per file and per mod), stores them in
`Win64\hsmp_mods\<hash>\`, and the new HSMPModHost mod starts them without a game restart.
DECLINE returns to the browser. Consent is remembered per server key and mod set. Leaving the
server unloads them as far as UE4SS allows.

**Compatibility:** every mode requires the protocol range declared by
`crates/hsmp-net/src/net/mod.rs` (`VERSION_MIN` / `VERSION_MAX`; protocol 12 at this update).
Placement and cutting claims include their original native evidence. Older protocol clients must
update before joining, including duel. Mode and server-mod capability checks still apply after
the protocol check. Use the deployed server's `--build-info` and browser `proto_min` / `proto_max`
as evidence when the development protocol changes again.

## 2. Setup

1. Prerequisites: Rust 1.98.1 (rustup), Visual Studio Build Tools with C++, `cmake` on PATH,
   an RE-UE4SS checkout at commit `e3ba1016` for the native build (`docs/development/ue4ss.md`).
2. `git fetch; git checkout dev; git pull`.
3. Deploy to each test PC's game: `.\scripts\build-and-deploy.ps1 -SkipG0` (add `-GamePath <game
   folder>` or set `HSMP_GAME_DIR`). Check that `Win64\ue4ss\Mods\mods.txt` lists
   `HSMPModHost : 1` and `HSMPNative : 1`.
4. Build the tools once: `cargo build --release -p hsmp-tools -p hsmp-server`.
5. Two players are needed for most tests; three or four for team tests. Two PCs (or VMs) on one
   LAN is simplest. One PC can run two instances the way `scripts/mp_test.ps1` does.
6. Dedicated test server (on one of the PCs):

   ```powershell
   $env:HSMP_RCON_PASSWORD = "test-password-123456"
   .\target\release\hsmp-server.exe --bind 0.0.0.0:7777 --rcon-bind 127.0.0.1:2345 --debug-verbs --events server-events.jsonl
   ```

   RCON: `nc 127.0.0.1 2345`, then `AUTH test-password-123456`, then commands (one reply per
   line; `STATUS` returns JSON with mode, teams, and per-seat team, kills, deaths, score,
   respawning). Players join with DIRECT CONNECT `<server ip>:7777`.
7. The gate still covers the old behaviour: `.\scripts\mp_test.ps1 -Instances 2 -Scenario p0_gate
   -Netsim typical` must stay green (no gate scenario covers the new features yet).

## 3. Test plan

Each test: do the steps, compare with "expect", save evidence (section 4). "P1", "P2" are players.

### A. Game modes

| # | Steps | Expect |
|---|---|---|
| A1 | Lobby, host clicks MODE | GAME MODE screen opens; rows MODE, TEAMS, YOUR TEAM, ROUND CLOCK, HILL TARGET, FRIENDLY FIRE, RESPAWN DELAY; BACK returns. Rows that don't apply are greyed with a reason. A non-host sees "Only the host changes the mode". Check 1080p and 1440p (and 720p if possible): nothing clipped or overlapping; the MODE button fits the lobby's button row |
| A2 | Server browser | Each server shows its mode by name |
| A3 | DUEL, 2 players, best of 3 | Same as beta.5, plus kill feed (top right) and kills/deaths on TAB |
| A4 | TEAMS, AUTO, 2 teams, 4 players (or 2 players, 1 per team) | Teams balanced at START; each team spawns on one side; sides swap next round; TAB shows team tags; top banner shows one cell per team; last team standing wins the round, every member gets the win |
| A5 | TEAMS, PICK: players choose RED/BLUE under YOUR TEAM; then try START with an empty team | Picks shown; START refused with a reason while a team is empty |
| A6 | TEAMS, friendly fire OFF: P1 hits teammate P2 hard several times | P2 takes no damage (server drops the hit); no kill credit |
| A7 | TEAMS, friendly fire ON (`OPTION ff on`) | Teammate hits do damage (the game's own same-team reduction applies); a team kill gives no kill credit |
| A8 | HILL, 2 players, every arena (`MAP <arena>` in RCON between matches) | HUD shows "HILL: n m <direction>" pointing at the hill and "ON THE HILL" when there; P1 alone on it scores (banner); both on it = no score; first to target (default 60 s) wins the round. Note any arena whose hill lands on an obstacle, in a wall, on another floor or off the playable area |
| A9 | HILL, ROUNDTIME 120, nobody reaches the target | At the clock, most points wins |
| A10 | ROULETTE, 2 players, 3+ rounds | Each round both players (and the stand-ins on the other screen) hold the same weapon, never a shield, and the same armour; centre text names the kit; the weapon changes between rounds; own kits come back after the match |
| A11 | BRAWL | Both players have empty hands and plain clothes all round; the game never re-arms anyone; fists do damage |
| A12 | DEATHMATCH, 2 players, ROUND CLOCK 3 MIN | After a death: "YOU DIED", "RESPAWN IN n", "RESPAWNING..."; the dead player's game reloads the arena and they spawn at the spawn point farthest from the living player, in their kit, with controls back; 2 s protection; measure death to control (target under 15 s). Kill counts go up on TAB and the banner |
| A13 | Same match, watch from the other player's screen | The corpse is replaced by a living, moving stand-in at the new spot with the right kit; that player's second death in the round plays normally (ragdoll, kill feed) |
| A14 | DEATHMATCH, die repeatedly (5+ times each) | No stuck "RESPAWNING...", no frozen input, no duplicated or missing stand-ins, spectate camera never sticks |
| A15 | DEATHMATCH, tied score at the clock | "SUDDEN DEATH", 60 s; next kill wins, else draw |
| A16 | RCON: `MODE koth`, `TEAMS auto 3`, `TEAM 1 2`, `ROUNDTIME 180`, `OPTION koth_target 90`, `OPTION respawn 5`, `STATUS`; then the same during a match | Lobby: OK replies and the GAME MODE screen updates; during a match: ERR (lobby only) |
| A17 | Leave mid-round in TEAMS (one of two on a team) | Match pauses only when a whole team is gone; a team forfeits only when none of it is left |
| A18 | Compatibility: a client built from `main` (`67588b0`, beta.5) joins a `teams` server, then a duel server | Both are refused by the protocol-version check with a readable update reason. A beta.5 client cannot remain connected while switching modes. Separately, exercise the mode capability refusal using a client on the current protocol with MODES capability omitted; it is refused for teams, and changing from duel to teams names that connected client |

### B. Server mods

Make two mods in `C:\hsmp-test-mods\`:

`TestBanner\mod.json`:
```json
{ "version": "1.0", "author": "QA", "description": "Prints, ticks and binds F8" }
```
`TestBanner\Scripts\main.lua`:
```lua
local helper = require("helper")
print("[TestBanner] loaded " .. helper.tag)
LoopAsync(5000, function() print("[TestBanner] tick") return false end)
RegisterKeyBind(Key.F8, function() print("[TestBanner] F8") end)
function OnUnload() print("[TestBanner] unloaded") end
```
`TestBanner\Scripts\helper.lua`: `return { tag = "helper ok" }`

`BigData\Scripts\main.lua`: `print("[BigData] loaded")`, plus
`BigData\Scripts\blob-a.txt` and `blob-b.txt`, each 10 MiB (any text), to test a real
20 MiB download. One 20 MiB file is invalid: the protocol allows at most 16 MiB per file.
`.\scripts\lab-server-mods-smoke.ps1 -FixturesOnly` creates these fixtures in a fresh
`test-results\modes-smoke-<id>\fixtures\valid` folder, plus separate invalid and load-error sets.

Start the server with `--mods-dir C:\hsmp-test-mods` (plus the setup flags).

| # | Steps | Expect |
|---|---|---|
| B1 | Browser | The server shows `[MODS 2]`; selecting it shows the full-access warning line with count and size |
| B2 | Join, first time | SERVER MODS screen: server label, key, mod set hash, both mods with version, author, size; DECLINE focused; nothing downloads before a click (check `Win64\hsmp_mods` stays empty) |
| B3 | DECLINE | Back to the browser with a readable reason; server log shows the player left; no files in `hsmp_mods` |
| B4 | Join again, ACCEPT & JOIN (REMEMBER on) | Progress bar fills; lobby opens; `UE4SS.log` shows `[TestBanner] loaded helper ok`, a tick every 5 s, `[TestBanner] F8` on F8, `[BigData] loaded`; `hsmp_events.jsonl` has `x_server_mods_offer` (decision accept) and `x_server_mods_loaded` with ok=2 failed=0; server log "server mods loaded: the player joins" |
| B5 | Download while another player is mid-match on that server | The fighting players see no lag spike or pose stutter during the 20 MB download (note ping and feel before and during) |
| B6 | While downloading, P2 in lobby | P2 is not in the roster until loaded, can't READY, START doesn't count them |
| B7 | Leave the server | `[TestBanner] unloaded`; ticks stop; `x_server_mods_unloaded` lists the inert count; F8 does nothing (key bind stays registered but inert) |
| B8 | Rejoin same server | No warning (remembered), no download (cached), mods load again |
| B9 | Change a mod on the server (edit main.lua), restart server, rejoin | Warning appears again (new mod set) |
| B10 | SETTINGS > SERVER MODS > FORGET REMEMBERED SERVERS, rejoin | Warning again, cached files reused (no download) |
| B11 | SETTINGS > SERVER MODS > NEVER, join | Declined immediately, back to browser, readable message |
| B12 | Pull the network (Wi-Fi off) halfway through the 20 MB download, restore within ~10 s | Download resumes where it stopped |
| B13 | Corrupt the cache: edit a file under `Win64\hsmp_mods\<hash>\`, rejoin | The bad copy is detected and re-downloaded or refused; never loaded as-is |
| B14 | Start with `--mods-timeout-s 30` and never click the warning | Kicked after 30 s with "The server's mods were not loaded within 30 s" |
| B15 | Server-side refusals: add a mod named `HSMPEvil`, then separately a `.dll` file in a mod, then a mod without `Scripts\main.lua` | The server refuses to start each time, naming the mod and file |
| B16 | A mod that errors (`error("boom")` in main.lua) | The player still joins; the error is logged with the mod name (`x_server_mod_error`); HSMP keeps working |
| B17 | Compatibility: a beta.5 client joins the mods server | Refused by the protocol-version check with a readable update reason. Separately, a client on the current protocol with SERVER_MODS capability omitted is refused with the server-mod-specific update reason |
| B18 | Launcher uninstall on a test PC | `Win64\hsmp_mods` is removed |
| B19 | Mods + modes together: `--mods-dir` and `--mode deathmatch` | Both work; the late joiner loading mods does not take part until loaded |

### C. Regression

| # | Steps | Expect |
|---|---|---|
| C1 | `.\scripts\mp_test.ps1 -Instances 2 -Scenario p0_gate -Netsim typical` | Exit 0, `report.json` verdict pass |
| C2 | One normal duel match through HOST GAME (no dedicated server) | Same as beta.5 |

## 4. Evidence to collect per failed (or doubtful) test

- `%LOCALAPPDATA%\HSMP\logs\<newest run>\` on each PC (`UE4SS.log`, `game.log`, `sidecar.log`,
  `hsmp_events.jsonl`), or the launcher's "Create bug report" zip.
- Server: the console output and `server-events.jsonl` (or `%LOCALAPPDATA%\HSMP\logs\server\`).
- `Win64\hsmp_state\.server_mods.json` and a listing of `Win64\hsmp_mods\` for mod tests.
- RCON `STATUS` output at the moment of the problem.
- A screenshot or short clip for anything visual (layout, HUD, stand-ins, respawn).

Useful events: server `mode_set`, `respawn_order`, `respawned`, `server_mods`, `round_void`;
sidecar `server_mods`; game `respawn`, `x_server_mods_offer`, `x_server_mods_loaded`,
`x_server_mods_unloaded`, `x_server_mod_error`.

## 5. Known gaps (not bugs)

- No marker in the world for the hill, only the HUD hint.
- No team colour tints.
- The spectate camera can flash on briefly during a respawn reload.
- Some mod registrations stay inert until a game restart (key binds, `NotifyOnNewObject`,
  console handlers, UE4SS pre/post hooks); changes a mod made to the world stay.
- HOST GAME cannot serve mods (dedicated server only).
- A download resumes after a reconnect but not after a game or sidecar restart.
- The co-op abyss mode is not started.

## 6. Report format

One line per test: `ID | PASS / FAIL / BLOCKED / NOT RUN | what happened | evidence file`. Then
for each FAIL: steps to reproduce, expected vs actual, and the log lines around the failure (with
the file name). Finish with anything odd seen outside the plan.

## 7. Repeatable backend checks (2026-10-07)

These provide evidence for parts of the plan. They do not turn a UI, native combat or
release-gate acceptance item green.

The lab also waits for the server's acknowledgement of the newly saved kit sequence
and checks the accepted class, hands, cosmetics and armour before START. Saving the same
kit twice now updates the game's acknowledgement even when its server revision stays
unchanged; previously the sidecar kept that newer acknowledgement internally but did not
publish it to `peer_kit`, leaving the menu or an exact lab waiter stuck.

```powershell
# No game needed: fresh fixtures, actual dedicated-server startup refusals,
# lobby TCP RCON and live UDP browser mode/mod metadata.
.\scripts\lab-server-mods-smoke.ps1

# Existing real two-game hsmp-lab session: same-round native reload and verified
# placement after five explicit DEBUG KILL deaths per player; lobby-only mode
# command refusals while Live. Starts no additional game or server processes.
.\scripts\lab-modes-test.ps1 -Session test-results\lab\session -DeathsPerPlayer 5
```

The native script takes the lab experiment lock, verifies recorded process identities,
disables AI, and returns to the lobby. Its timer is **death to server-verified placement**,
not death to player control. A12 still needs control/HUD, kit, farthest-spawn and actual
protection-hit evidence; A13/A14 still need remote stand-in and repeated-death visuals.
`DEBUG KILL` is labelled as the stimulus in every sample and proves no damage or kill-credit
parity. The report keeps backend subchecks separate from full acceptance verdicts.

Offline run `test-results/modes-smoke-67968ebf/report.json`: 38 backend subchecks PASS,
including all seven browser/RCON mode labels, the two-mod 20 MiB set, configuration
options and the three B15 manifest refusals. Binary SHA-256 values and raw RCON/query/server
logs are in that run. A1-A18, B1-B19 and C1-C2 remain NOT RUN as live acceptance at this
update. The native script has been parsed but has not run against the game yet.
