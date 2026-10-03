# Game modes: framework and integration contract

## Status: what is wired and what is not

**The mode framework in `server/src/modes/` is not used by `hsmp-server`.** It compiles and is tested
only as the standalone `hsmp-modes` crate (`crates/hsmp-modes`, whose `lib.path` points at
`server/src/modes/mod.rs`). The server has no `mod modes;` and no dependency on `hsmp-modes`.

What the server does today:

- **One round rule for every match: last player standing.** The round flow is the server's own state
  machine in `server/src/server/match_core.rs` (`match_step`): lobby → countdown (load barrier) → live
  → roundover (400 ms trade settle, then the result) → next countdown or match over → lobby. A match is
  won at `(best_of + 1) / 2` round wins. `best_of` defaults to 3 and the lobby offers 1/3/5/7 (the
  server accepts up to 31).
- **The mode is a label.** `--mode` (default `duel`; env `HSMP_SERVER_MODE` when the flag is left at
  its default) is advertised to the master list and the server browser, and mapped to a u8 code in the
  session config (`session.rs` `mode_code`: `ffa`/`lms` → FFA, `teams`/`lts`/`team_elim` → TEAM_ELIM,
  `koth`/`king_of_hill` → KING_OF_HILL, anything else → DUEL). The code does not change any rule.
- **The mode cannot be changed at runtime.** `SetConfig` refuses a `mode` change with
  `mode is not configurable yet` (as it does for round time, teams, team rule, fighter / spectator
  caps and join-in-progress). The lobby shows the mode read-only.

The rest of this document describes the framework as built, for whoever wires it into the server.

## 1. What exists

| File | What |
|---|---|
| `server/src/modes/mod.rs` | Module root, `create(id)`, `MODE_IDS = ["duel", "ffa", "lts"]` (`lms` is an alias of `ffa`), `infos()` |
| `server/src/modes/traits.rs` | `GameMode`, `ModeCtx`, `SeatView`, `SeatInfo`, `ModeEvent`, `ModeStatus`, results, policies, `ModeState` (HUD section) |
| `server/src/modes/round.rs` | `RoundCore`: the shared last-standing engine, in ms, grouped by side |
| `server/src/modes/duel.rs` | Duel: the server's round loop as it was when the framework was written (rule table in the file header) |
| `server/src/modes/ffa_lms.rs` | FFA Last Man Standing (first to N wins, round cap, kills tiebreak, assists) |
| `server/src/modes/lts.rs` | Last Team Standing (teams, balance, friendly fire, side swap, uneven-team budget, sudden death) |
| `server/src/modes/teams.rs` | Pure team assignment and balance, join pick, uneven compensation, class ladder, tints, side rotation |
| `server/src/modes/zone.rs` | Pure shrinking zone: phase tables, sampling, seeded centres, server damage backstop |
| `crates/hsmp-modes/Cargo.toml` | The standalone crate |
| `crates/hsmp-modes/tests/props.rs` | proptest properties (section 9) |

Runtime dependency: `serde` (derive) only. No tokio, no sockets, no RNG crate (the zone uses a
published-seed xorshift).

```
cargo test -p hsmp-modes        # 40 unit tests + 5 property tests
```

## 2. Model

**The core owns time and connectivity; the mode owns meaning.** A mode is a pure state machine. The
host (the server's session reducer, once wired):
1. builds a `ModeCtx { now_ms, roster: &[SeatView], out: &mut Vec<ModeEvent> }`,
2. calls a hook,
3. drains `out`,
4. reads `mode.status()` and reconciles its phase (section 3).

| Concept | Rule |
|---|---|
| Time | `Ms = u64`, the host's monotonic server clock. No ticks anywhere: the settle is 400 ms whatever the tick rate. |
| Seat | `SeatId = u32`, allocated by the host at START and **stable across reconnects** (the same player key gives the same seat). Scores, teams and stats are keyed by seat, so a reconnect keeps its wins by construction. Peer ids never reach the mode. |
| Side | `Side::Seat(id)` (Duel, FFA) or `Side::Team(t)` (LTS). Round wins are per side. |
| `SeatView.present` | Connected **and**, during a live round, its game is still reporting (the server's `present_participants`, 20 s game-status timeout). The host computes it. Absent seats are omitted from `roster` or listed with `present: false`. |
| `SeatView.pos` / `ledger_health` | The last accepted root (UE cm) and the damage ledger's health. Only the sudden-death ring and its tiebreak read them. |
| Determinism | Every map is a `BTreeMap` / `BTreeSet`. The same hook sequence gives the same results, events and HUD (a property test checks this). |

## 3. Status to host phase (the reconcile table)

After **every** hook call, the host compares `mode.status()` with the previous value and acts on a
change:

| `ModeStatus` | Phase / reason | Host action on entering it |
|---|---|---|
| `Idle` | `lobby` | none (before START, or after `reset()`) |
| `Waiting { replay: false }` | `countdown` | begin the countdown (3 s), spawn plan (`spawn_policy`, `kit_override`), load barrier |
| `Waiting { replay: true }` | `countdown`, reason cleared | the players came back from a pause: countdown (3 s) |
| `Live { round }` | `live` | (the host caused this itself via `on_round_start`) begin the damage ledger's round and clear the round's deaths |
| `Settling { until_ms }` | `roundover`, reason `"pending"`, `last_winner = 0` | keep **combat open** (`status().combat_open()`): accept trade hits and deaths |
| `RoundOver(result)` | `roundover`, reason `""` (win) or `"draw"`, `last_winner` = the winning seat or 0 | publish the result, show 4 s, then countdown (3 s) |
| `MatchOver(result)` | `match_over`, reason `"forfeit"` if `end == Forfeit`, else `""` | show 5 s, publish the match result, then lobby and `mode.reset()` |
| `Paused { until_ms }` | `paused`, reason `"opponent_left"` | freeze and send the deadline. The mode resolves it itself on a later `on_tick` (replay, forfeit or abandon). |
| `Abandoned` | `lobby` | back to the lobby and `mode.reset()` |

The host keeps the timers that carry no mode meaning: countdown 3 s, round-over display 4 s,
match-over display 5 s, and the load barrier with its 45 s deadline. The settle and pause deadlines
are **mode** timers: the mode fixes them inside `on_tick` when `now_ms` reaches them.

`ModeStatus::phase_str()` returns the phase string. `ModeStatus::combat_open()` replaces the server's
own combat-open check.

## 4. When the host calls each hook

| Hook | Call site | Notes |
|---|---|---|
| `on_config(cfg, roster)` | the START command, after the ready check | Ok gives `Waiting { replay: false }`. Err is refused with the message (unknown or bad option, player count). Options are frozen from here on. |
| `on_round_start(ctx, round, fighters)` | countdown → live | `round` = the new round number (strictly increasing, otherwise it returns false). `fighters` = present seats whose game loaded this round, **including late joiners** that loaded, excluding barrier stragglers. |
| `on_tick(ctx)` | every host tick, where `match_step` runs | Runs supervision (pause / abandon, 3+ player round end), the sudden-death ring, the settle deadline and the pause expiry. |
| `on_death(ctx, victim, killer, cause)` | the server's single death declaration, reached from the owner's death report, the dead flag in `game_status`, vitals and the ledger | `Ignored` means no `S2CDeath` broadcast. `killer` = the ledger's last attacker mapped to a seat (None for environment or self). `cause` uses the wire values (0 reported, 1 damage, 2 vitals, 3 left, 4 zone). |
| `on_damage(ctx, attacker, victim, amount)` | after an accepted, FF-adjusted hit | Optional. FFA uses it for assists. |
| `on_disconnect(ctx, seat)` | a peer leaving | Pass the roster **without** that seat. The seat stops being alive (a reconnect spectates until the next live round), then supervision runs at once. A stale game status is **not** a disconnect: set `present: false`. |
| `on_join(ctx, &SeatInfo)` | a hello while a match runs (new or reconnecting seat) | `NextRound` (spectate now, fight when listed in `fighters`) or `Spectate`. LTS seats the joiner on a team here. |
| `on_barrier_timeout(ctx, loaded)` | the barrier deadline | `loaded` = present seats that loaded. Fewer than 2 loaded sides (in a 2+ side match) gives a forfeit or abandon. Otherwise the stragglers sit out. |
| `friendly_fire(attacker, victim)` | the hit claim path, **before** forwarding to the victim's owner | `Allow`, `Scale(f)` (multiply the claim's damage) or `Deny(reason)` (drop the claim; physics still pushes). |
| `spawn_policy(ctx, round)` / `kit_override(ctx, seat)` | the spawn plan at countdown, and the kit at spawn | See section 7. |
| `team_of(seat)` | building the session roster | `NO_TEAM` (0) for FFA modes. |
| `admin_set_team(seat, team)` | an admin `SET_TEAM` command | Only before the first round. |
| `hud_section(now_ms)` | every session snapshot build | Becomes the snapshot's mode section (section 6). |
| `round_result()` / `round_results()` / `match_result()` | result publishing and history | `round_results()` holds exactly one entry per started round, **including `Void` ones**, in order. |
| `reset()` | match over → lobby, abort, reset match | Status goes back to `Idle`. |

**ModeEvents the host must handle:**
- `Banner { text }` → the HUD banner or a server chat line.
- `KillFeed { victim, killer, cause, assists }` → a kill-feed event.
- `ZoneDamage { seat, amount }` → a **synthetic ledger entry** (attacker 0, `cause = DEATH_ZONE = 4`).
  If the ledger reaches 0, the host declares the death through its normal path, which calls
  `on_death(.., killer, DEATH_ZONE)`. Kill credit is the last attacker within 10 s, else None.

The round-result chat text stays with the host, because the mode does not know nicks.

## 5. Duel compared with the server today

`duel.rs` re-homes the server's round loop as it was when the framework was written. Its header lists
each rule. The server has changed since, so wiring Duel in as-is would change behaviour:

| Rule | Server today (`match_core.rs`) | `Duel` |
|---|---|---|
| `best_of` range | 1..=31 (`MAX_BEST_OF`) | 1..=15 |
| A participant drops mid-round | pauses up to 30 s for its reconnect, within a budget of 1 pause per player and 2 per match; beyond the budget the dropper loses the round (`DEATH_LEFT`) | always pauses 30 s |
| The player comes back from a pause | the interrupted round **resumes** (positions, health, ledger kept) | the round is **replayed** (`Waiting { replay: true }`) |
| A client fails to load | a client that reported a load error gets one 10 s retry window, then sits the round out (a spectator, no loss); the 45 s barrier deadline drops the rest | only the barrier deadline |
| Solo match whose player leaves | back to the lobby | status stays `Live` |
| Everyone leaves the result screen | back to the lobby at once | (not modelled) |

Rules that still match: the win target `(best_of + 1) / 2`; START seats everyone connected and clears
wins; a death counts only while combat is open (live or settling), once per seat per round; the round
ends when nobody stands, or (2+ participants) when at most one stands; a 400 ms trade settle during
which combat stays open; at the settle deadline exactly one standing wins, anything else is a draw; a
draw scores nobody; a forfeit raises the winner to the win target with reason `forfeit`; 3+ player
rounds whose other fighters left end without a pause; a reconnect keeps its wins.

Differences Duel had on purpose:
1. **Void results.** A round cut short by a pause or abandon records `RoundOutcome::Void(..)`, so
   "exactly one result per started round" is checkable. Voids are not displayed or counted.
2. **Exact match winner.** `MatchResult.winner` is the side that reached the target, or the forfeit
   winner.
3. **Time base.** The 400 ms settle and 30 s pause stay the same at any tick rate.
4. **"duel" accepts 1..=8 players** (solo test rounds; 3+ is last man standing). `ffa` is the real FFA
   rule.

The round limit with its sudden-death ring is available as `round_s=N`. It is **off by default**.

## 6. Wire: the mode section of the session snapshot

`hud_section(now_ms) -> ModeState` (serde) is the tagged section meant for the session snapshot:

```
ModeState { mode: "duel"|"ffa"|"lts", round, state: Idle|Waiting|Live|Settling|RoundOver|MatchOver|Paused,
            deadline_ms /* settle, pause, or round-limit/zone end; 0 = none */,
            target_wins, max_rounds,
            sides: [ { side: Seat(id)|Team(t), score, alive } ],
            seats: [ { seat, team, kills, deaths, assists, alive } ],
            last_result: Option<RoundResult>   /* last non-void */,
            objective: None | Zone { phase, center, radius_cm, next_center, next_radius_cm, shrinking, segment_end_ms, dps } }
```

This section is not on the wire yet. The current snapshot carries the mode code, the roster's wins and
alive flags, and the result. All times are server-clock ms; the sidecar converts them with its clock
offset.

## 7. Modes and options

All options go through `ModeConfig.opts`. An unknown key or an unparsable value is an error at
`on_config`. `ModeConfig.map_center` centres the sudden-death ring; without it the ring centres on the
standing players' centroid.

| Mode | Players | Options (default) | Spawn | Kit |
|---|---|---|---|---|
| `duel` | 1..=8 | `best_of` (3, 1..=15), `round_s` (0 = off) | `Fixed` | player choice |
| `ffa` (alias `lms`) | 3..=8 | `wins` (3), `rounds` (5; 0 = no cap), `round_s` (0), `assist_window_s` (10), `assist_min_dmg` (20), `spread_m` (6) | `FarthestFromEnemies { 600 cm }` | player choice |
| `lts` | teams..=32 | `teams` (2, 2..=4), `best_of` (5), `round_s` (300), `ff` (0; 0..=1), `budget_per_missing` (10), `max_rounds` (2·best_of; 0 = none) | `TeamSides`, rotating each round | team tint forced; the short team gets a budget bonus or class upgrades |

**Match end:** a side reaching `wins` (FFA) or `needed_wins(best_of)` (Duel, LTS) wins. With a round
cap, after `max_rounds` decided rounds (wins plus draws), the leader by wins, then by kills, takes it.
A full tie is a match draw (`winner: None`, `MatchEnd::RoundCap`).

**Sudden death** (`round_s > 0`): at `live + round_s`, a `ZoneTable::sudden_death()` ring starts: 3 m
shrinking to 1.5 m over 30 s at 5 Health/s outside, after a 0.5 s grace, emitted as `ZoneDamage`
every 100 ms. If 2+ sides still stand when it ends, the tiebreak is the most standing seats, then the
highest summed ledger health. No unique leader is a draw. The result is marked `tiebreak: true`.

**Teams (LTS):**
- Balanced at START by `teams::balance`: rank by rating, then key, then seat; each seat goes to the
  smallest team, then the lowest rating total, then the lowest id. Sizes differ by at most 1, and input
  order is irrelevant.
- Joiners go to the smallest team (ties: lower score, then lower id), stay pending until their first
  `fighters` listing, and keep the team on reconnect.

**Uneven teams** (computed from present members):
- `KitOverride.budget_bonus = (largest − own) × budget_per_missing`, applied under CUSTOM kit rules.
- `class_upgrade = min(largest − own, 3)` rungs on peasant → brute → man_at_arms → knight, applied
  under CLASSES ONLY. `duelist` upgrades to man_at_arms.
- The host applies whichever of the two matches the frozen kit rules.

**Tints:** team `t` gets cloth tint index `t` (1 CRIMSON, 2 AZURE, 3 FOREST, 4 OCHRE).

**Friendly fire (LTS):** `ff=0` → `Deny("friendly_fire")`; `0<ff<1` → `Scale(ff)`; `ff=1` → `Allow`.
Team kills give no kill credit.

## 8. Zone module (`zone.rs`)

Pure and unit-tested. Positions and radii are in cm, the zone is a 2D cylinder (Z ignored), and times
are in ms.

- `ZoneTable { phases: [ZonePhase { wait_ms, shrink_ms, r_from_cm, r_to_cm, dps }] }`.
  - `validate()` requires radii non-increasing and continuous between phases, no radius jump without
    shrink time, and dps ≥ 0 and non-decreasing.
  - Built-in tables: `ambush16()` (420 s in total) and `sudden_death()`.
- `Zone::fixed` / `Zone::new(centres)` / `Zone::seeded(c0, candidates, seed)`.
  - Each next centre is chosen within `r_cur − r_next`, so the next circle always lies inside the
    current one.
  - `seeded` uses a published-seed xorshift, so a seed reproduces every centre.
- `sample(now)` returns the phase, the interpolated centre and radius, the next circle, whether it is
  shrinking, the end of the current segment, the dps and whether the zone has finished.
- `is_outside`, `dps_at` and `edge_distance_cm` are helpers built on `sample`.
- `ZoneDamage::step(zone, now, seats)` is the server backstop: 100 ms steps, a 0.5 s grace (reset by
  stepping back inside), and catch-up capped at 2 s after a stall.

## 9. Tests

- **Unit (40):**
  - Duel: kill → settle → one winner; a mutual kill in either order is a draw; deaths outside an open
    round are ignored; a 3-player round ends when the others leave; match point; pause, replay, forfeit
    and abandon; a stale status counts as absent but not dead; barrier forfeit; a solo round; the
    sudden-death tiebreak; HUD deadlines.
  - FFA config validation, first-to-N, the round cap with the kills tiebreak, and assists inside and
    outside the window.
  - LTS team wipe, FF verdicts, team-kill credit, uneven budget and tint, joiner placement, a whole
    team dropping (pause then forfeit), admin team moves, and side rotation.
  - Teams and zone (tables, interpolation, monotonic radius, nested centres, determinism, grace and
    rate, bounded catch-up).
- **Property (`crates/hsmp-modes/tests/props.rs`).** Random sequences of advance, kill, suicide,
  damage, drop, rejoin, status toggles, round starts, stale round starts and barrier timeouts, over
  Duel, FFA and LTS (with and without the round limit):
  - **exactly one result per started round** (Void included), in round order;
  - **monotonic rounds:** a stale or duplicate round start is refused and changes nothing; a start is
    accepted exactly when the mode is between rounds;
  - **no last-standing winner while 2+ sides stand:** a `Won(side)` result has exactly `{side}`
    standing, a draw never has exactly one side standing, and `Live` never persists with at most one
    side standing in a multi-side match;
  - match-result consistency: the winner is at or above the target for Score and Forfeit endings;
  - **deterministic replays:** the same ops give the same results, HUD and events;
  - **deterministic team balance:** independent of input order, everyone assigned once, sizes within
    1, and the joiner pick is the smallest team;
  - the zone radius is monotonic, the centre always safe, and `finished` exact;
  - a coverage guard (`harness_reaches_every_outcome`) fails if the generator stops reaching wins,
    team wins, draws, voids, tiebreaks, pauses, replays, score, cap and forfeit endings, or abandons.

## 10. Wiring it in

Two options; both compile.

1. **Crate dependency.** `crates/hsmp-modes` is already a workspace member. Add
   `hsmp-modes = { path = "../crates/hsmp-modes" }` to `server/Cargo.toml` and
   `use hsmp_modes as modes;`.
2. **Module mount.** Put `mod modes;` in the server crate. `server/src/modes/mod.rs` is already in
   place, the files only use `super::` paths, and the server already depends on serde.
   `#![allow(dead_code)]` in `mod.rs` silences the not-yet-called API.

Work that wiring needs besides the reconcile in section 3:
- reconcile Duel with the current server rules (section 5) first, so the default mode does not change
  behaviour;
- the mode section and kill-feed event on the wire;
- the friendly-fire verdict in the hit path, synthetic ledger entries for `ZoneDamage`, and the
  `DEATH_ZONE` cause on the wire;
- seat allocation keyed by player key;
- `spawn_policy` → `spawns.rs` (`TeamSides` and `FarthestFromEnemies` need team tags and positions in
  the spawn tables);
- `KitOverride` → `loadout.rs`;
- allowing `mode` in `SetConfig`, and the HUD widgets.
