# beta5 review: server (tick, scaling, hostile input, master Worker)

Branch `cloud/server`. Scope: `server/src` (not relay.rs, lagcomp, validate/ or combat.rs),
`crates/hsmp-master-core` and `master-cf/`. Every number below comes from this Linux machine:
4 cores, shared with five other agents, load average 4 to 7 during the runs. Treat CPU
percentages as ±2 points.

## Changes

### 1. The server tick does not allocate in steady play (`3e9dc7d`)

**Profile.** I moved the locked part of the tick into `tick_locked` (sync, so a test can run it)
and counted allocations with a test-only counting allocator (`server/src/alloc_count.rs`,
per-thread counts). With 16 players the tick allocated 25 times in the lobby and 35 times in a
live round. The work behind those allocations, each tick:

- `reconcile_seats` cloned every nick and built three hash sets;
- `observe_links` built a set and two vectors;
- `session_due` rebuilt the whole snapshot (seat vector, a key→peer map, the rows) and cloned
  the arena name for the config signature. It also hashed SHA-256 once per roster row for
  `player_id`;
- `Net::reconcile` built a set of every peer address;
- `loadout::set_round_lock` built a set;
- `loadout::session_view` built a map;
- `present_participants` and `standing` built vectors only to read their lengths;
- the tick loop itself built address and id vectors.

**Fix.** Each of these now checks whether anything changed and does the old work only when it
did. Buffers are kept between ticks (`TickScratch`, `SessionCore::snap_bufs`). The snapshot is
copied out only when it is actually sent (1 to 3 Hz). Player ids are memoised per thread.
Behaviour is unchanged: all 329 existing hsmp-server unit and property tests pass, and the slow
paths are the old code.

**Evidence.** Test `server::tick::alloc_tests::steady_tick_does_not_allocate`. It fails on the
old code with "lobby 25, live 35". It runs 8, 16 and 32 players, lobby and live, and asserts
0 allocations on every tick that does not send the snapshot.

`cargo test --release -p hsmp-server --bin hsmp-server steady_tick -- --nocapture`:

| players | before: allocs / µs per tick | after: allocs / µs per tick |
|---|---|---|
| 16, lobby | 25 / 42.9 | 0 / 3.4 |
| 16, live | 35 / 26.2 | 0 / 5.8 |
| 8, lobby / live | | 0 / 1.5, 0 / 7.4 |
| 32, lobby / live | | 0 / 8.2, 0 / 14.1 |

"Before" is one 2000-tick run of the extracted `tick_locked` on the beta.4 session, match_core,
loadout and net code. "After" is the best of five 2000-tick runs; single runs on the busy
machine ranged from 3.2 to 8 µs.

In `hsmp-loadtest` the tick's p50 (`HSMP_PERF`, which includes the sends after the lock) went
from 45 to 31 µs at 8 bots and from 58 to 39 µs at 16 bots.

**Where the CPU goes.** The tick is a small part of the server's CPU. Packet handling dominates:
AEAD open/seal per datagram, lag compensation and the relay's selection, about 70 to 100 µs p50
per packet in the loadtest. Those live in hsmp-net, lagcomp and relay.rs, which other areas
own. Server CPU per player is roughly linear: about 0.7 % of one core per player at 8 and at 16
bots.

### 2. Relayed records that arrive together leave together (`8b09970`, loadtest `f2697f2`)

The netcode agent's suggestion. The sidecar sends a frame's root, weapon and pose in one
transmit (`ipc_shm.rs` `inbound_hot`). The server relayed each record in its own sealed
datagram. Now, while the receive loop handles one datagram's messages, relays queue per
receiver (`Net::queue_bytes`) and flush once at the end (`Net::flush`,
`dispatch::handle_deliveries`). There is no added delay, no wire change (receivers already take
several chunks per packet) and no change for single-message datagrams.

**Evidence.** Test `dispatch::bundle_tests::records_sent_together_are_relayed_together`: root
and vitals from one datagram reach the receiver as 1 datagram. With the batch disabled it is 3.

Loadtest with `--pump` (new: bots send root and pose together at 50 Hz like the sidecar), 15 s,
the same loadtest binary against the beta.4 server and this branch's:

| bots | server datagrams out /s | server payload out KB/s | client rx KB/s per bot |
|---|---|---|---|
| 8 | 3026 → 2388 (−21 %) | 731 → 705 (−3.6 %) | 77.6 → 74.4 |
| 16 | 8657 → 6654 (−23 %) | 1739 → 1648 (−5.2 %) | 90.9 → 85.7 |

Counting 28 B of IPv4/UDP per datagram, on-wire download drops about 6 % (8 bots) and 7 %
(16 bots). The netcode estimate was 11 %. The difference is because the relay thins root and
pose separately, so not every pair is relayed together. Delivery ratios and relay latency are
unchanged within noise. Without `--pump` the old bots never send two stream records in one
datagram, and the numbers are identical before and after.

### 3. Match timers in seconds at any `--tick-hz`; bounded flags (`8d7ed77`)

The round-flow timers were tick counts that assumed 30 Hz: countdowns, load barrier (45 s),
load-retry window, game-ping and link-stall timeouts, reconnect grace, trade settle, and the
60 s peer timeout. `--tick-hz 60` halved every one of them, while the advertised config
(`countdown_s` and so on) went from 3 to 1 s. They now scale through `hz_ticks`. At 30 Hz the
numbers are exactly the old ones. `--tick-hz 0` divided by zero at startup. The flag now
accepts 10..=120, and `--max-peers` accepts 1..=64 (the `session` record lists at most 64
seats).

Tests: `round_flow_timings_are_seconds_at_any_tick_rate`, which fails on the old code with
config `(1, 2, 22)` instead of `(3, 4, 45)`, and
`limit_args_tests::tick_rate_and_peer_cap_are_bounded`.

The default stays 30 and HSMPMenu still passes `--tick-hz 30`. The player streams are 60 Hz
from the clients whatever the tick rate. A 60 Hz tick would only buy faster snapshots and
death repeats, at about twice the tick CPU (still tiny).

### 4. Master: half the storage writes (`5866b73`)

Every heartbeat wrote the listing row (one SQLite row write in the Durable Object). Now a
heartbeat that changes nothing the browser shows (players, map, mode, NAT) stays in memory, as
long as the stored copy, loaded after a Durable Object restart, would still outlive the next
heartbeat plus 30 s of slack. With the public 120 / 360 s that means every other heartbeat is
written.

Test: `registry::tests::unchanged_heartbeats_skip_writes_without_losing_the_listing_on_restart`.
It runs a day of heartbeats, rebuilds the registry from the stored rows every 7th heartbeat, and
checks after every heartbeat that a restart still lists the server. Result: 720 → 360 writes per
server per day. With the skip disabled it fails (720 writes).

`master-cf` builds for `wasm32-unknown-unknown`:
`cargo check --locked --target wasm32-unknown-unknown` in `master-cf/` passes. `worker-build`
and wrangler are not installed here, so the Worker was not run.

**Free-plan budget (per day).**

| resource | limit | 50 servers, 120 s heartbeat |
|---|---|---|
| Worker requests | 100,000 | 36,000 heartbeats + browser GETs |
| Durable Object requests | 100,000 | 36,000 heartbeats + ≤ 1 list read per isolate per 5 s + ≤ ~360 sweep alarms + punch requests |
| SQLite rows written | 100,000 | 36,000 → about 18,000 |
| SQLite rows read | 5,000,000 | 50 rows per Durable Object wake |
| Durable Object duration | 13,000 GB-s | ≤ 10,800 if it never sleeps; punch sockets hibernate and pings are auto-answered, not billed |

The binding limit is Worker and Durable Object **requests**. Heartbeats alone take 720 per server
per day, so the free plan holds about 100 servers before browser traffic. Halving the request
count would mean a longer heartbeat (for example 240 / 720 s). That is a `wrangler.toml` change
(servers learn the interval at registration), traded against slower expiry of crashed servers.
I left it as is.

CPU per request: the Worker side forwards, and the Durable Object does one Ed25519 verify per
signed request plus JSON. I could not measure wasm CPU here. The 10 ms free-plan limit per
invocation should be far away, but check `wrangler tail` on the next deploy.

### 5. Hostile input

- **Wire.** `server/tests/decode_fuzz.rs`, `record_fuzz.rs` and `pose_fuzz.rs` already cover
  every record kind, the pose codec, the query and the world, session, combat, interact and
  loadout handlers. I audited the non-test index, slice, unwrap and arithmetic sites in my scope
  (rcon.rs, master.rs, query.rs, records and session handlers, world_glue) and found no reachable
  panic. The only `unwrap`s are mutex locks (`world().lock().unwrap()`, which panics only on a
  poisoned lock) and test code.
- **RCON.** The line reader is bounded, numeric arguments are parsed with `parse` and errors are
  answered, and strings are truncated on char boundaries (`Str::new`). Nothing found.
- **Master HTTP (core shared with the Worker).** New test `hostile_requests_never_panic` (`34f2365`)
  sends 5,000 rounds of random and mutated bodies, ids, paths, signatures and addresses to
  register, heartbeat, delete, punch, listen, the report-zip check and the dashboard. No panic
  was found, so it guards against regressions; it fixes nothing.
- **CLI.** `--tick-hz 0` panicked (division by zero); fixed in §3.

### 6. Tooling

`hsmp-loadtest`'s server CPU column printed NaN off Windows (it used PowerShell). It now reads
`/proc/<pid>/stat` on Linux (`proc_stat_cpu_ms`, tested). Also new: `--pump`, and the receive
byte count no longer counts a datagram once per record in it.

## Scaling to 16 players: what caps at 8

- **Server.** Only the default: `--max-peers 8`, and HSMPMenu passes `--max-peers 8` for HOST.
  The flag works up to 64. `SPAWN_SLOTS = 8` is informational. `spawns.rs` plans 16 players
  (derived points beyond the arena's real ones; the Pit wraps at 14, already tested). Seats are
  u8 up to 254, the session roster holds 64, the shared-memory peer table 32. The per-public-IP
  cap is 4. Measured server cost at 16: tick about 6 µs, loadtest 11 to 20 % of one core. I
  changed no defaults.
- **Bandwidth.** At the default `--client-budget-kbps 128`, 16 players thin the far players to
  about 8 to 17 Hz (loadtest fidelity line), while the nearest two stay at 25 to 30 Hz or more.
  The host's upload is about 1 Mbit/s per player.
- **Game side (needs work and in-game tests, not done here).**
  - Stand-ins come from the arena's foe pool, capped by "Free Mode Foes Amount" and untested
    beyond 7.
  - HSMPAvatars probes peer ids 1..8 on full scans (`MAX_PEER_ID`). The roster path covers
    other ids, so this only matters for roster-less peers.
  - Combat engagement logic says "2–8 players in pairs" (combat area).
  - README and the player docs say "up to 8". I added "More than 8 players" in
    docs/hosting/configuration.md and left the player docs alone.

## Test results

- `cargo test -p hsmp-server` (debug): all pass (330 unit tests in hsmp-server plus the
  integration suites). `cargo test -p hsmp-master-core`: 37 pass.
- Workspace: see the final section below.

## Needs Windows or in-game verification

- Relay bundling with the real sidecar and game: the expected effect is fewer, larger datagrams
  per receiver. Watch for any change in pose smoothness. Pacing and loss handling are per
  datagram in hsmp-net and did not change.
- `--tick-hz` other than 30 with a real game (the Director reads phase deadlines in ms, so it
  should not care).
- The master change on the real Worker: a deploy, then confirm the rows-written count in the
  Cloudflare dashboard halves.

## Risks

- **Counting allocator.** `alloc_count.rs` is a `#[global_allocator]` in the hsmp-server test
  build only. If another branch adds a global allocator to the same binary's tests, the merge
  will not compile. Keep one.
- **Relay batch.** The batch flag is on `ServerState`, not per task. A relay from another task
  while the receive loop batches is queued and flushed a few µs later at the batch end. Today
  only the receive path relays. The relay budget still charges a full seal per record, which
  slightly overestimates bundled traffic (conservative).
- **Master replay window.** A skipped heartbeat's `ts` is not persisted. After a Durable Object
  restart, a captured copy of that one heartbeat could be replayed once, which only refreshes the
  listing with the same data.
- **`seats_in_step`, the snapshot row lookup, `is_present`.** These are linear scans per peer
  (O(n²) with n ≤ 64), cheaper than the maps they replace at the sizes measured. At 64 players
  the 32-player figure suggests about 30 µs per tick, still negligible.
- **Shared git stash.** The stash is shared between worktrees. Early on my stash was popped into
  the netcode worktree; the coordinator had it reverted there.
