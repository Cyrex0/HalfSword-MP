# Game modes

## Status

Seven modes are wired into the server and the game. They are lobby settings (the GAME MODE screen,
RCON, the `--mode` family of flags), frozen at START like the arena and the kit rules.

| Mode (`game_mode`) | Round rule | Teams | Kit |
|---|---|---|---|
| DUEL 0, FFA 1 | last player standing | never | the players' own |
| TEAM_ELIM 2 | last team standing | always (AUTO when the rule is NONE) | own |
| KING_OF_HILL 3 | first to the target on the hill alone; deaths eliminate; at the clock the most points | optional | own |
| ROULETTE 4 | last side standing | optional | one random weapon + armour set for everyone, per round |
| BRAWL 5 | last side standing | optional | fists, plain clothes |
| DEATHMATCH 6 | respawns; most kills at the clock; a tie goes to sudden death | optional | own |

Status of the parts that need the game:

- **Server rules, records, commands, RCON:** done and unit-tested (`modes_tests.rs`,
  `session_tests.rs`, `rcon.rs`, `loadout.rs`, `combat.rs`).
- **Lobby GAME MODE screen, HUD, Director respawn, HSMPCombat respawn:** done; covered by the
  offline Lua suites (`modes_ui`, `commands`, `hud_net`, `hsmp_session`, `director`, `combat`).
  **Not yet verified in game** (see [In-game verification](#in-game-verification)).
- **Not done:** an in-world marker for the hill (the HUD gives distance and direction), team tints
  on the kit, per-team class upgrades for uneven teams, a shrinking-zone mode. The earlier
  stand-alone framework (`server/src/modes/`, crate `hsmp-modes`) was removed: its round loop
  duplicated `match_core.rs` with different pause rules; the useful parts (balanced teams, side
  scoring, sudden death) are in `modes.rs`.

## 1. Where the code is

| File | What |
|---|---|
| `server/src/server/modes.rs` | `ModeCore` (in `Inner::modes`): config, teams, stats, round clock, King of the hill, round kits, respawns, the `mode` / `zone` / `kill_feed` records |
| `server/src/server/match_core.rs` | the round loop; calls the hooks (below) and decides rounds through `modes::decide` |
| `server/src/server/session.rs` | SET_CONFIG mode fields, SET_TEAM, SET_OPTION, START (`modes::on_start`), roster `team`, `STATUS` |
| `server/src/loadout.rs` | `impose`: the round kit replaces every player's kit verdict |
| `server/src/spawns.rs` | team sides (`Seat.team`), `respawn_point` |
| `server/src/server/combat_glue.rs` | `modes::hit_refusal` on the hit path (friendly fire, respawn protection) |
| `server/src/server/dispatch.rs` | `modes::join_refusal` at the handshake |
| `server/src/sidecar/session_client.rs` | `mode` / `zone` slots; re-opens the death dedup of a respawned peer |
| `mods/shared/hsmp_session.lua` | `HS.mode()`: the normalised mode view |
| `mods/HSMPMenu/Scripts/modes_ui.lua` | the GAME MODE screen |
| `mods/HSMPHud/Scripts/hud_model.lua` | team cells, hill status and direction, respawn countdown, K / D |
| `mods/HSMPMatch/Scripts/director.lua` | deathmatch respawn reload |
| `mods/HSMPCombat/Scripts/main.lua` | `C3.respawns`: a respawned peer's stand-in is alive again |

## 2. Hooks into the round loop

| Hook | Called from | Does |
|---|---|---|
| `on_start` | START | freezes the config (`ModeCore.run`), seats teams (FIXED picks, then the smallest team in seat order), finds the hill; refuses a START with an empty team |
| `seat_late` | `go_live`, a late joiner | its FIXED pick, else the smallest connected team |
| `on_countdown` | `begin_countdown` | the round kit (roulette: `roulette_kit(match_id, round)`, brawl) |
| `on_live` | `go_live` | per-round stats, the round clock, respawns and the hill start over |
| `step` | every tick while Live (`tick::advance`) | hill scoring, respawn orders, the round clock, sudden death; true = settle now |
| `on_death` | `declare_death_unprotected` | deaths / kills (no credit for team kills), `kill_feed`, respawn scheduling, sudden-death end |
| `standing_sides` | `check_round_end`, `match_step` | sides with a standing / present participant (allocation-free) |
| `decide` | `finalize_round` | winner side, `match_reason`, `mode_result` |
| `team_won`, `set_team_wins` | `finalize_round`, `forfeit_to` | a team's round win is every member's win (roster `wins`) |
| `on_lobby` | `reset_to_lobby` | the match's mode state goes; the round kit is lifted |

Pauses and forfeits work per **side**: a match pauses when fewer than 2 sides are present (a
teammate dropping does not pause), a pause that runs out with one side present is that side's
forfeit, and a deliberate leave forfeits when the remaining participants are one side.

## 3. Rules in detail

- **Teams.** AUTO: seat order, each player to the smallest team. FIXED: the players' lobby picks
  (`SET_TEAM`), then the rest. Teams spawn on their own side (`spawns::assign` side ordering, sides
  swap every round). The roster's `team` drives HSMPCombat's native same-team behaviour
  ("Team Int"); the server drops hits between teammates (`friendly fire`) unless the option allows
  them. A team kill gives no kill credit.
- **Round clock** (`round_time_limit_s`; 0 = none, deathmatch 300 s, King of the hill 240 s): it
  only runs while Live. At 0: elimination modes give the round to the side with the most standing
  players (else a draw), King of the hill to the most points, deathmatch to the most kills.
  `match_reason` is `time_limit` (session `result_reason` TIME_LIMIT).
- **King of the hill.** The hill is a cylinder: the centroid of the arena's valid spawn points,
  radius a third of their mean distance from it (250..600 cm), ±300 cm in height;
  `HSMP_KOTH_ZONES` overrides it per arena. Every tick, the alive participants whose last
  validated root (`accept_root`, speed-capped) is inside are counted by side: one side alone
  scores the tick's ms (a team's points are the team's), two or more is contested. The target
  (`mode_opt` KOTH_TARGET, default 60 s) ends the round (`mode_result` OBJECTIVE).
- **Weapon roulette.** `splitmix(match_id, round)` picks one catalogue weapon (never a shield) for
  the right hand and one armour set (a class's, or plain clothes). `loadout::impose` replaces every
  kit verdict (verdict REPLACED, reason "round kit"), keeping cosmetics; the kit is imposed at the
  countdown, before Live freezes the damage model's kit facts. The lobby lifts it.
- **Brawl.** The same, with empty hands (`WeaponClass::Unarmed`) and plain clothes.
- **Deathmatch.** A death schedules a respawn after `respawn_s` (default 3). The server picks the
  candidate spawn farthest from the living players (`spawns::respawn_point`), replaces the
  player's spawn order with `spawn_id = round << 8 | 0x80 | life`, and the `mode` row says
  `respawning`. The player stays dead until its client reports the order placed in `game_status`;
  then it is alive with a fresh damage-ledger life (`ledger_respawn`), its old death is no longer
  repeated, and nothing can hurt it, nor it anyone, for 2 s. A respawning player's silent game (a
  level load) does not count as gone. An order never confirmed is issued again after 60 s;
  headless clients (no game status) revive at once. A clock that ends tied starts sudden death
  (60 s; the next kill that makes one side lead wins, else a draw).

## 4. Wire and compatibility

[protocol.md](../protocol.md) §6.4.1: records `mode` (0x0240, `caps::MODES`) and `zone` (0x0241,
`caps::ZONE`), `kill_feed` (now sent, `caps::MODES`), command op `SET_OPTION` (14), `SET_TEAM`
(11), `game_mode` codes 4..6. Existing record layouts are unchanged. A server playing any mode but
duel / FFA refuses joiners without `caps::MODES` at the handshake (reject VERSION with a clear
text), and the lobby cannot switch to such a mode while an older client is connected; duel / FFA
servers keep working with beta.5 clients.

## 5. The client side of a respawn

The respawn reuses the round-start path instead of reviving a dead Willie in place (a dead
Willie_BP's ragdoll, `DED` flag, dropped weapons and native death flow are not reliably undoable
from Lua):

1. The Director sees `HS.mode()` say `respawning` for itself and its roster spawn order carry the
   0x80 respawn bit for the current round, and reloads the arena (Prepare → Travel → WaitWorld →
   Spawn), serving the **current** round.
2. HSMPSync places the new pawn on the order (a fresh world in Live is placed as a round start);
   the pipeline heals vitals, dresses the kit, waits for the census, reaches Ready and reports the
   order in `game_status`.
3. The server revives the player; the Director releases input when the roster says alive.
4. On the other clients the stand-in played a death; HSMPCombat drops that death when the peer's
   `life` goes up, and HSMPAvatars re-claims a body once the owner's fresh vitals say alive. The
   sidecar lets the peer's next death in the round through.

## 6. In-game verification

Needed on Windows with two or more game instances:

1. **Respawn (deathmatch):** after a death, the dead player's arena reloads 3 s later, the pawn is
   placed on the respawn point, dressed, and can fight; input stays frozen until then. Check the
   reload time on a slow PC (the server waits for the report, re-issuing after 60 s).
2. **Respawn on the other screens:** the corpse stand-in is replaced by a living, driven stand-in
   at the respawn point (HSMPAvatars re-claim), shows the owner's kit, and a second death in the
   same round plays (kill feed, ragdoll).
3. **Spectating during the reload:** HSMPMatch's spectate camera after a death and its hand-back
   when the respawned pawn is alive.
4. **Round kits:** roulette weapons in hand at FIGHT for every player, the same on every screen
   (stand-ins too), plain clothes; brawl strips both hands (fists) with no native re-arm; kit
   status `ok` with empty hands; the player's own kit back after the match.
5. **Teams:** the native same-team behaviour between teammates (blunt, weak blows) together with
   the server's friendly-fire refusal, team sides at spawn, the HUD team cells.
6. **King of the hill:** the hill's position per arena (centroid of the spawns; tune
   `HSMP_KOTH_ZONES` where it lands on an obstacle or another level), the HUD direction hint, and
   points only while alone.
7. **Lobby:** the GAME MODE screen's layout at common resolutions, the MODE button in the lobby's
   action row, team picks, refusals shown on the message line.

## 7. Tests

- `server/src/server/modes_tests.rs`: config / names / compatibility gate, AUTO and FIXED teams,
  team elimination and team wins, pause / forfeit per side, friendly fire, kills and the kill
  feed, the hill (scoring, contest, target, clock, team points), clock draws, roulette / brawl
  kits, deathmatch respawn on the placement report, protection, sudden death, the records.
- `session_tests.rs` (`mode_teams_and_options_are_lobby_commands`, the compatibility refusal),
  `rcon.rs` (`mode_verbs_*`), `loadout.rs` (`an_imposed_round_kit_replaces_every_kit_until_lifted`),
  `combat.rs` (`ledger_respawn_starts_a_new_life_in_the_same_round`), `server_info.rs` (listing
  label), the sidecar (`a_respawn_reopens_the_peers_death_dedup`, the mode / zone slots), hsmp-ipc
  (record round trips and checks).
- Lua: `hsmp_session` (mode view), `director` (respawn reload), `combat` (respawned stand-in),
  `commands` (the new command records), `modes_ui`, `hud_net` (hill direction).
