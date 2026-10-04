# Server tick rate

`hsmp-server --tick-hz <n>` (default **60**, accepted 20 to 240, env `HSMP_TICK_HZ`) sets how
often the server's tick loop runs. This page says what the tick does and does not do, what a
higher rate buys, what it costs, and why the default is 60.

## Three clocks that are not the same

| Clock | Rate | Where | Changes with `--tick-hz`? |
|---|---|---|---|
| The game's frame | the player's fps | Unreal Engine | no |
| The mods' Lua loops | ~30 Hz (`TICK_MS = 33` in HSMPCombat, HSMPAvatars; HSMPSync's base loop) | game thread | no |
| The pose stream | 60 Hz per sender (HSMPSync `send_hz`) | game → sidecar → server | no |
| The server world task | fixed 33 ms (`world_glue.rs` `WORLD_TICK`; HSMPWorld `FAST_SEND_MS` mirrors it) | server | no |
| **The server tick** | `--tick-hz` | `server/src/server/tick.rs` | yes |

Nothing on the client knows the server tick rate, and nothing on the wire carries it. Clients
see only real-time values: `phase_deadline_ms` on the server clock (counted down with the local
clock in `shared/hsmp_session.lua`), the config's `*_s` seconds and the per-pair relay interval
in a `pose` record's `aux`. Do not change a mod's own loop to "match" the server tick.

## What the tick does

Per tick: the relay plan check (it replans every 500 ms), the held-hit flush
(`combat::flush_pending`), clash judging, the ledger sweep, interaction lease expiry, the match
flow (`advance`), the session snapshot when due, death repeats, peer timeouts and the 1 s pings.

What it does **not** do:

- **Relay poses.** Pose, root and weapon records are forwarded when they arrive
  (`broadcast::relay_pose`), so the tick adds no relay latency.
- **Sample history.** Lag compensation records every client sample on arrival, stamped with the
  sender's clock (`lagcomp::Ring`, 1.2 s by time). Hit claims are evaluated on arrival against
  that history. The tick is not a snapshot.
- **Count time.** Every timer is real time on the tick's server clock (`Inner::now_ms`):

| Timer | Value | Was |
|---|---|---|
| First / next-round countdown | 3 s | 90 ticks |
| Round over / match over screen | 4 s / 5 s | 120 / 150 ticks |
| Trade settle | 400 ms | 12 ticks |
| Load barrier | 45 s | `45 * 30` ticks |
| Load-error retry window | 10 s | `10 * 30` ticks |
| Game-ping timeout (live) | 20 s | `20 * 30` ticks |
| Link stall (live, paused) | 3 s | `3 * 30` ticks |
| Reconnect grace (pause) | 30 s | `30 * 30` ticks |
| Peer timeout | 60 s | `60 * 30` ticks |
| Late placement report | 1 s | 30 ticks |
| No-admin auto start | 5 s | `5 * tick_hz` ticks |
| Death record repeat | 1 s lobby / 333 ms match | `tick_hz` / `tick_hz / 3` ticks |
| Pings to lag comp + `pings` record | 1 s | `tick_hz` ticks |
| Root speed rule | time between root arrivals | ticks between roots / `tick_hz` |
| Session config `countdown_s` & co. | ms / 1000 | ticks / `tick_hz` (integer: 1 at 60 Hz, 0 at 100 Hz) |

The tick only decides how soon after its deadline a timer is noticed. The session tests run
every timing test at 30, 60, 100 and 128 Hz (`session_tests.rs` `at_rates`), and
`scripted_match_is_identical_at_every_tick_rate` plays one three-player match (load error, kill,
a player leaving, a stalled link, a pause running out, the result screen) at all four rates and
checks that the phases, results and phase lengths match.

## What a higher tick buys (measured)

### Lag-comp history resolution

`lagcomp::tests::history_keeps_every_sample_whatever_the_tick_rate`: a 2500 uu/s swing streamed
at 60 Hz (±1.5 ms frame jitter, 40–55 ms network delay), sampled back at 4500 random hit times.
The server keeps every sample; the other rows are a design it does not use (newest sample per
tick), for comparison.

| History | Blade tip (Hermite) p50 / p95 / max uu | Linear (bones, capsules) |
|---|---|---|
| Every sample (the server, any tick rate) | 0.00 / 0.01 / 0.12 | 0.91 / 1.83 / 2.51 |
| Newest sample per 30 Hz tick | 0.07 / 0.94 / 7.99 | 3.89 / 10.67 / 17.63 |
| Newest sample per 60 Hz tick | 0.01 / 0.13 / 3.48 | 1.12 / 5.72 / 8.64 |
| Newest sample per 100 Hz tick | 0.00 / 0.06 / 2.44 | 0.98 / 3.55 / 8.34 |
| Newest sample per 128 Hz tick | 0.00 / 0.02 / 2.61 | 0.96 / 2.17 / 8.33 |

### Hit validation against ground truth

`hsmp-combat-sim --tick-hz 30,60,100,128 --profiles typical,intl,far --seeds 10 --ablate`
(160 fights per row; the real lagcomp + combat code against the simulator's true bodies).
"Victim feels it" is from the blow to the victim's game applying it; it includes the held-hit
flush, the only step on the tick.

| Profile | Tick | Accepted % | Parry cancelled % | False cancel % | Trades mutual % | Victim feels it p95 ms | Confirm p50 / p95 ms | Honest clamped |
|---|---|---|---|---|---|---|---|---|
| typical | 30 | 99.97 | 97.3 | 0.00 | 100 | 554 | 147 / 246 | 28 |
| typical | 60 | 99.97 | 97.4 | 0.00 | 100 | 545 | 146 / 248 | 21 |
| typical | 100 | 99.97 | 97.9 | 0.00 | 100 | 541 | 145 / 246 | 23 |
| typical | 128 | 99.97 | 97.6 | 0.00 | 100 | 540 | 146 / 248 | 25 |
| intl | 30 | 99.95 | 96.0 | 0.00 | 100 | 863 | 227 / 349 | 23 |
| intl | 60 | 99.97 | 97.5 | 0.00 | 100 | 853 | 227 / 350 | 25 |
| intl | 100 | 99.92 | 98.4 | 0.00 | 100 | 848 | 226 / 348 | 22 |
| intl | 128 | 99.97 | 97.9 | 0.00 | 100 | 849 | 227 / 352 | 25 |
| far | 30 | 99.83 | 82.3 | 0.00 | 100 | 1164 | 365 / 508 | 31 |
| far | 60 | 99.83 | 81.0 | 0.00 | 100 | 1156 | 362 / 501 | 29 |
| far | 100 | 99.66 | 79.6 | 0.00 | 100 | 1156 | 364 / 509 | 33 |
| far | 128 | 99.80 | 83.2 | 0.00 | 100 | 1148 | 364 / 510 | 31 |

Cheat suite (typical, every cheat kind): false accepts 0.27 % at 30 Hz, 0.29 % at 60, 0.27 % at
100, 0.39 % at 128 (11–16 of ~4100 effective cheat claims; run-to-run noise).

The two designs the server does not use, at the same rates (`--ablate`):

| Profile | Design | 30 Hz | 60 Hz | 100 Hz | 128 Hz |
|---|---|---|---|---|---|
| typical | history per tick: confirm p50 ms / honest clamped | 234 / 80 | 156 / 33 | 151 / 26 | 150 / 27 |
| far | history per tick: accepted % | 99.27 | 99.58 | 99.27 | 99.72 |
| typical | relay on tick: view lag p50 ms | 162 | 150 | 146 | 144 |
| far | relay on tick: accepted % | 99.66 | 99.69 | 99.32 | 99.63 |

(Server as built: typical confirm p50 147 ms, 21–28 clamped; view lag p50 140 ms.)

### Reading it

- Validation accuracy, parry and trade fairness do not depend on the tick: the server already
  decides on every sample at the sender's timestamps, and claims are judged on arrival.
- What the tick does change is **when** held hits, clash glints and grab expiry are released:
  up to one period late, half a period on average (16.7 ms at 30 Hz, 8.3 ms at 60, 5 ms at 100).
  That is the 9–14 ms "victim feels it" gain from 30 to 60–128 Hz; past 60 it is 1–8 ms more.
- A tick-snapshot server would need 100+ Hz to approach what keeping every sample gives at any
  rate, and relaying on the tick would add half a period of view lag to every pose. Neither is
  worth doing; keep samples timestamped and use the tick only for scheduling.

## What it costs (measured)

`hsmp-tools net-bench --clients N --secs 20 --tick-hz H`: the real server, N fake games at 60 Hz
with real sidecars, behind a counting proxy (lobby traffic: no match runs, so no held hits).

| players | tick Hz | server CPU % (per player) | tick p50 / p99 us | pkt p50 / p99 us | up per client KB/s, pps | down per client KB/s, pps | thinned/s | budget drops/s | idle % during |
|---|---|---|---|---|---|---|---|---|---|
| 2 | 30 | 1.40 (0.70) | 15.00 / 54.75 | 11.75 / 44.25 | 33.5, 79.41 | 29.1, 111.58 | 60.00 | 0.00 | 89.31 |
| 2 | 60 | 1.56 (0.78) | 12.25 / 43.50 | 15.25 / 45.75 | 33.5, 79.54 | 29.1, 111.71 | 60.00 | 0.00 | 86.47 |
| 2 | 100 | 1.80 (0.90) | 12.75 / 59.00 | 17.00 / 46.25 | 33.5, 79.41 | 29.1, 111.69 | 59.75 | 0.00 | 91.08 |
| 2 | 128 | 1.79 (0.90) | 10.50 / 54.25 | 13.75 / 45.00 | 33.5, 79.53 | 29.1, 111.71 | 59.75 | 0.00 | 92.04 |
| 4 | 30 | 2.81 (0.70) | 18.00 / 88.25 | 22.00 / 66.00 | 33.5, 79.23 | 72.8, 288.89 | 519.50 | 0.00 | 88.74 |
| 4 | 60 | 3.20 (0.80) | 15.25 / 64.50 | 23.25 / 58.75 | 33.5, 79.36 | 72.9, 289.36 | 519.75 | 0.00 | 91.23 |
| 4 | 100 | 2.50 (0.62) | 12.00 / 66.50 | 24.00 / 56.25 | 33.5, 79.27 | 72.9, 289.18 | 519.75 | 0.00 | 92.14 |
| 4 | 128 | 2.97 (0.74) | 10.75 / 69.25 | 20.25 / 59.50 | 33.5, 79.28 | 72.8, 289.19 | 519.75 | 0.00 | 92.82 |
| 8 | 30 | 6.94 (0.87) | 20.25 / 117.50 | 26.75 / 79.25 | 33.5, 79.66 | 117.9, 519.52 | 3639.50 | 0.00 | 91.75 |
| 8 | 60 | 8.19 (1.02) | 23.25 / 140.25 | 27.50 / 90.25 | 33.5, 79.91 | 118.2, 519.22 | 3639.75 | 0.00 | 87.20 |
| 8 | 100 | 6.55 (0.82) | 16.00 / 106.25 | 28.00 / 88.75 | 33.5, 79.98 | 118.6, 519.73 | 3639.50 | 0.00 | 87.66 |
| 8 | 128 | 6.71 (0.84) | 14.00 / 101.25 | 27.00 / 81.50 | 33.5, 79.62 | 118.2, 519.55 | 3639.50 | 0.00 | 90.44 |


`hsmp-loadtest` bots behind `hsmp-tools netsim`:

| netsim | tick Hz | players | server CPU % | tick p50 / p99 us | relay latency p50 / p99 ms (root, end to end) | root / skel delivered % | down KB/s per client | server in / out pps | thinned /s | budget drops /s | client pkts lost / retransmits |
|---|---|---|---|---|---|---|---|---|---|---|---|
| none | 30 | 2 | 0.6 | 15 / 60 | 0.07 / 0.14 | 100.9 / 100.0 | 10.0 | 135 / 143 | 0 | 0 | 0 / 0 |
| none | 30 | 4 | 2.0 | 16 / 89 | 0.08 / 0.18 | 84.1 / 100.1 | 28.3 | 248 / 683 | 60 | 0 | 0 / 0 |
| none | 30 | 8 | 3.2 | 18 / 115 | 0.08 / 0.18 | 64.9 / 78.9 | 51.8 | 476 / 2436 | 844 | 0 | 0 / 0 |
| none | 60 | 2 | 0.7 | 12 / 46 | 0.07 / 0.13 | 101.0 / 100.0 | 10.0 | 135 / 143 | 0 | 0 | 0 / 0 |
| none | 60 | 4 | 2.0 | 13 / 60 | 0.07 / 0.14 | 84.1 / 100.0 | 28.3 | 248 / 684 | 60 | 0 | 0 / 0 |
| none | 60 | 8 | 4.6 | 16 / 100 | 0.09 / 0.18 | 64.9 / 78.8 | 51.8 | 475 / 2434 | 843 | 0 | 0 / 0 |
| none | 100 | 2 | 1.3 | 12 / 67 | 0.08 / 0.14 | 101.0 / 100.0 | 10.0 | 135 / 143 | 0 | 0 | 0 / 0 |
| none | 100 | 4 | 1.9 | 10 / 52 | 0.07 / 0.13 | 84.1 / 100.0 | 28.3 | 249 / 683 | 60 | 0 | 0 / 0 |
| none | 100 | 8 | 4.4 | 12 / 84 | 0.08 / 0.17 | 64.9 / 78.8 | 51.7 | 476 / 2437 | 845 | 0 | 0 / 0 |
| typical | 30 | 2 | 0.7 | 15 / 51 | 100.89 / 120.91 | 98.9 / 97.7 | 9.8 | 132 / 143 | 0 | 0 | 22 / 0 |
| typical | 30 | 4 | 2.0 | 18 / 95 | 100.45 / 120.65 | 82.9 / 98.2 | 27.9 | 247 / 682 | 60 | 0 | 32 / 0 |
| typical | 30 | 8 | 3.0 | 17 / 99 | 100.97 / 121.03 | 63.7 / 77.1 | 50.7 | 469 / 2395 | 835 | 0 | 97 / 2 |
| typical | 60 | 2 | 1.1 | 13 / 47 | 99.78 / 121.59 | 99.3 / 98.0 | 9.8 | 134 / 144 | 0 | 0 | 18 / 0 |
| typical | 60 | 4 | 1.9 | 13 / 56 | 100.84 / 122.01 | 82.6 / 98.1 | 27.8 | 246 / 681 | 60 | 0 | 32 / 2 |
| typical | 60 | 8 | 3.3 | 16 / 102 | 100.97 / 121.60 | 63.8 / 77.3 | 50.8 | 472 / 2415 | 839 | 0 | 80 / 4 |
| typical | 100 | 2 | 0.8 | 10 / 56 | 100.66 / 120.92 | 99.0 / 97.8 | 9.8 | 132 / 143 | 0 | 0 | 39 / 5 |
| typical | 100 | 4 | 1.2 | 10 / 58 | 100.03 / 123.21 | 82.8 / 98.0 | 27.8 | 247 / 681 | 60 | 0 | 38 / 2 |
| typical | 100 | 8 | 4.1 | 13 / 104 | 102.02 / 121.50 | 63.7 / 77.2 | 50.7 | 471 / 2408 | 839 | 0 | 94 / 3 |
| intl | 30 | 2 | 0.6 | 14 / 60 | 183.41 / 230.09 | 99.7 / 98.5 | 9.8 | 134 / 151 | 0 | 0 | 14 / 1 |
| intl | 30 | 4 | 0.8 | 17 / 86 | 176.99 / 227.16 | 84.4 / 100.0 | 28.4 | 249 / 689 | 61 | 0 | 17 / 2 |
| intl | 30 | 8 | 2.7 | 18 / 104 | 186.52 / 228.89 | 64.9 / 78.9 | 51.8 | 474 / 2422 | 841 | 0 | 32 / 1 |
| intl | 60 | 2 | 1.1 | 12 / 48 | 182.84 / 228.78 | 100.1 / 99.2 | 9.9 | 133 / 152 | 0 | 0 | 15 / 2 |
| intl | 60 | 4 | 1.9 | 17 / 75 | 181.63 / 230.16 | 84.2 / 99.9 | 28.3 | 248 / 687 | 60 | 0 | 15 / 0 |
| intl | 60 | 8 | 3.3 | 16 / 107 | 185.13 / 227.90 | 65.2 / 79.4 | 52.1 | 475 / 2430 | 843 | 0 | 31 / 2 |
| intl | 100 | 2 | 0.9 | 10 / 52 | 182.06 / 229.89 | 99.9 / 98.7 | 9.9 | 135 / 153 | 0 | 0 | 10 / 2 |
| intl | 100 | 4 | 1.9 | 10 / 66 | 181.18 / 226.62 | 84.4 / 99.9 | 28.3 | 248 / 690 | 60 | 0 | 11 / 0 |
| intl | 100 | 8 | 3.6 | 13 / 97 | 182.15 / 224.82 | 65.3 / 79.3 | 52.1 | 474 / 2425 | 843 | 0 | 13 / 0 |
| far | 30 | 2 | 0.7 | 17 / 68 | 304.25 / 392.96 | 96.3 / 96.2 | 9.6 | 131 / 149 | 0 | 0 | 38 / 0 |
| far | 30 | 4 | 1.5 | 17 / 70 | 310.85 / 384.13 | 80.9 / 96.0 | 27.2 | 240 / 663 | 58 | 0 | 85 / 11 |
| far | 30 | 8 | 3.3 | 20 / 97 | 321.18 / 391.89 | 62.9 / 77.5 | 50.7 | 461 / 2355 | 823 | 0 | 144 / 7 |
| far | 60 | 2 | 0.4 | 13 / 61 | 306.75 / 381.34 | 95.2 / 95.8 | 9.5 | 131 / 151 | 0 | 0 | 47 / 0 |
| far | 60 | 4 | 1.0 | 13 / 61 | 312.23 / 384.78 | 81.1 / 95.0 | 27.0 | 246 / 681 | 60 | 0 | 70 / 4 |
| far | 60 | 8 | 4.6 | 31 / 179 | 312.18 / 382.02 | 64.0 / 77.5 | 50.9 | 478 / 2441 | 857 | 0 | 150 / 2 |
| far | 100 | 2 | 2.4 | 12 / 59 | 307.37 / 387.24 | 96.4 / 95.8 | 9.6 | 131 / 148 | 0 | 0 | 41 / 2 |
| far | 100 | 4 | 2.0 | 27 / 130 | 307.65 / 390.89 | 80.9 / 95.2 | 27.1 | 245 / 684 | 60 | 0 | 68 / 3 |
| far | 100 | 8 | 4.4 | 31 / 184 | 314.38 / 386.49 | 63.9 / 76.5 | 50.9 | 467 / 2373 | 830 | 0 | 158 / 3 |


The bots send roots at 30 Hz and skeletons at 20 Hz (older rates than the game's 60 Hz), so the
rows compare tick rates, not absolute load. Relay latency is the netsim path plus ~0.1 ms of
server: the same at 30, 60 and 100 Hz (a relay on the tick would add 5–17 ms on average).
"Delivered" below 100 % at 4 and 8 players is the relevance thinning of far players (by design,
the same at every tick); the budget never dropped a packet. CPU and tick p99 are inside the
run-to-run noise of this shared machine at every rate; even at 8 players and 100 Hz the tick
uses well under 1 % of a core.

Bandwidth and packet rates do not change with the tick: streams are relayed on arrival and every
periodic record (session, deaths, pings) is on a real-time interval. The relay's budgets refill
by elapsed time and replan every 500 ms, so they behave the same at any tick. CPU grows only
with the per-tick bookkeeping, which is tens of microseconds.

## The default: 60 Hz

- It halves the decision delay of 30 Hz (held-hit release ≤ 16.7 ms, average 8.3 ms), which is
  most of what any rate can buy.
- It matches the clients' 60 Hz pose stream and the combat simulator's model.
- 100 or 128 Hz buys 1–8 ms more at 1.7–2× the tick work, with no measurable accuracy gain.
- Windows timers have 1 ms resolution (`timeBeginPeriod(1)`): 60 Hz runs 16/17 ms periods with a
  correct average; above ~200 Hz the jitter is a large share of the period.

Raise it on a strong host if you want the last few milliseconds; never below 30.
