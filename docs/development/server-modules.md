# Server and sidecar module map

The `hsmp-server` and `hsmp-sidecar` binaries are module trees under `server/src/server/` and
`server/src/sidecar/`. The big picture (all binaries, the shared-memory IPC, the crates) is in
[architecture.md](architecture.md); the shared subsystem files at `server/src/` are listed there
(§5).

## How the tree is wired

- Each child module starts with `use super::*;`, so it sees everything its root sees: imports,
  the state types, and private fields such as `ServerState::inner`. Descendant modules may use
  private fields, so the structs need no visibility changes.
- The shared structs stay in the roots: `PeerState`, `ServerState`, `Inner` and `MatchPeer` in
  `server/mod.rs`; `Args` and `SharedState` in `sidecar/main.rs`.
- A helper that a sibling module needs is `pub(super)`. The root pulls it in with
  `use <child>::*;`, so siblings reach it through `use super::*`. If you add a `pub(super)`
  helper to a module that is not glob-imported yet, add the `use <child>::*;` line to the root.
- The sidecar bin root is `server/src/sidecar/main.rs` (`server/Cargo.toml`
  `[[bin]] hsmp-sidecar`). The modules it shares with the server (`proto`, `events`, the
  `*_client` files) are included with `#[path = "../x.rs"]`; the pose codec and jitter buffer
  come from the `hsmp-pose` crate (see "Shared source files" in [architecture.md](architecture.md)).
- Log targets are `hsmp_server::server::<module>` and `hsmp_sidecar::<module>`. `RUST_LOG`
  prefix filters such as `hsmp_server=info` or `hsmp_sidecar=info` match them.

## Server: `server/src/server/`

Every message the server receives is a protocol v6 record ([protocol.md](protocol.md) §6).
`records.rs` dispatches by kind; each domain validates the record in place and acts on it.

| File | Contents |
|---|---|
| `mod.rs` | Module docs (match flow, admin model), imports, `PeerState`, `ServerState`, `Inner`, `MatchPeer`, hit-validation constants, module declarations and re-exports |
| `dispatch.rs` | `recv_loop` (UDP receive, browser-query answers, transport), admission (`admit`: ban, capacity, per-public-IP cap `MAX_PER_PUBLIC_IP = 4`, duplicate key / session resume, welcome, roster), `migrate_peer` (path migration: re-keys per-address state, applies bans and the per-IP cap), per-peer rate budgets, transport stats |
| `records.rs` | `handle`: one arm per record kind or domain range, calling the domain's handler. A kind nobody handles is dropped and counted |
| `session.rs` | Player identity and seats by key, nick deduplication, peer leave, the speed-capped root bookkeeping, `start_match`, `auto_start_step` (no-admin lobby), the `session` record (`session_due`: 1 Hz lobby, 3 Hz match, at once on change), `command` handling with one cached `cmd_result` per `(player, cmd_id)` (`CMD_CACHE = 64`); RCON uses the same `apply_command` |
| `session_records.rs` | The session-domain records (`0x02xx`): validates `command`, `game_status`, `spawned`, `leave`, `chat`, `ping`; builds `welcome`, `session`, `pings`, `cmd_result`, `notice`, `kicked`, `server_closing`, `chat_in`, `admin_state`, `pong` |
| `session_tests.rs` | Unit and property tests of `session.rs` (snapshots, command dedup, phase transitions, seats by key, nick dedup), mounted with `#[path]` from it |
| `match_core.rs` | Arena registry, match verbs, authoritative deaths and settle, the round-flow state machine (`match_step`: lobby, countdown, live, roundover, paused, match_over), the load barrier (`BARRIER_TIMEOUT_MS`, 45 s); every phase timer is real time on the tick's server clock, spawn plan use (`SPAWN_SLOTS = 8`), round-flow tests |
| `admin.rs` | `AdminPolicy` (owner, configured and granted keys), admins-file reload, per-recipient `admin_state` with masked bans, ban-list load and save, RCON admin commands, admin verbs |
| `broadcast.rs` | `flush_out`, `send_out`, `broadcast_admin_state`, `kick`, `shutdown`, `relay_pose` (per-receiver `aux` = relay interval), `relay_record` |
| `tick.rs` | `tick_loop` and `advance`: held-hit flush, timeouts, `match_step`, session and result broadcasts, `build_pings`, admin state after an admin timed out |
| `pose_glue.rs` | `root` (input gate, stream rate, speed cap, lag compensation, relay), `weapon` (kept for lag compensation, not relayed), `pose` (one decode for lag compensation, relayed as the incoming bytes) |
| `combat_glue.rs` | `damage`, `damage_ack`, `clash`, `touch`, `death_report`, `vitals` in; `damage_in` to the victim, `hitfx_in` to every other `HIT_FX` player, `damage_verdict` to the attacker, `death_ack`, `death`, relayed `vitals` (Health clamped to the ledger's ceiling) out; held-hit flush |
| `world_glue.rs` | The world records (`0x04xx`): validates them in place and hands the rows to `world.rs`; the world sender task |
| `interact_glue.rs` | A C2S `interact` record → `crate::interact` validation → the record to the owner of the affected body; per-tick lease expiry and departed-peer cleanup ([interact.md](subsystems/interact.md)) |

## Sidecar: `server/src/sidecar/`

| File | Contents |
|---|---|
| `main.rs` | Bin root: `#[path]` shared modules, module declarations, the 1 ms Windows timer, `Args`, `SharedState`, IPC attach (`--ipc shm:<name>` is required: exit 64 without it, 70–77 on a refused segment), `--print-player-key`, `--career-recover`, `main()` (startup, first handshake, the tasks, the career guard enter and leave, the final detach) |
| `net.rs` | The protocol client: one `hsmp_net::net::Client` behind a mutex, `send_msg` / `send_msgs` (record messages on their channel; one pump step's records in one transmit), the receive task, the 5 ms transport task, the reconnect watchdog, the identity (`identity`, `.player_key`), server-key pinning and `known_servers.json`, the 2 s `ping` task |
| `handlers.rs` | `handle_server_record`: every S2C record kind handed to its domain (session, pose, combat, world, loadout, interact) |
| `session_client.rs` | The client half of session, match and connection: `session` ordered by `(epoch, seq)` and copied into the `session` slot (the peer directory follows its roster); `cmd_result`, `notice`, `kill_feed`, `chat_in` deduplicated into the S2G ring; `admin_state` into the `admin` slot; `pings` into the peer directory; the one `link` record. G2S `command` framed as it is and resent every 250 ms until answered (5 s give-up) |
| `pose.rs` | Remote peer playback buffers and the `peer_play` slot writer (thread `hsmp-poseplay`); relayed `root` / `pose` into poseplay and the `peer_root` slot; sent-frame and latency reports |
| `records_in.rs` | `on_g2s`: every validated G2S record from the game to its domain (frame it for the server, or feed the domain client's logic first). Runs on the `hsmp-ipc` thread and never blocks |
| `ipc_shm.rs` | `ShmLink`: attach, refuse, detach; `post_record` / `push_record` / `send_record`; the `hsmp-ipc` thread (slots, blobs, rings, heartbeat, tap); the root / weapon / pose forwarder ([ipc-shared-memory.md](ipc-shared-memory.md)) |
| `ipc_shm_tests.rs` | An in-process fake game (`Mapping::anonymous` + `handshake::init_game`) against the real `ShmLink` |
| `parent.rs` | Which game process the sidecar belongs to (`--parent-pid`, else the parent chain) and waiting for it to exit, checked every 500 ms through a process handle |
| `career_guard.rs` | Layer 2 of the career-save guard: backup on entering MP, restore and verification on leaving, crash recovery ([career-saves.md](../players/career-saves.md)) |
| `panic_guard.rs` | Any panic in any task or thread is fatal: log to stderr and `<state>/.sidecar_panic.log`, restore the career save, exit |

The domain clients the sidecar mounts from `server/src/` are `combat_client.rs`,
`loadout_client.rs`, `world_client.rs` and `interact_client.rs`. `events.rs` (`--events` JSONL) is
shared by both binaries.

## Tests

- `cargo test -p hsmp-server` runs the unit tests of every binary in the package and the
  integration tests in `server/tests/` (`sidecar_shm.rs`, `sidecar_panic.rs`, `interact_e2e.rs`,
  `decode_fuzz.rs`, `record_fuzz.rs`, `pose_fuzz.rs`).
- `cargo test -p hsmp-server --bin hsmp-sidecar career_guard` runs only the career-guard tests.
- `scripts/e2e-test.sh` runs the real binaries end to end ([testing.md](testing.md)).
