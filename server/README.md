# hsmp-server package

The `hsmp-server` Cargo package builds five binaries from one source tree:

| Binary | Source | What it is |
|---|---|---|
| `hsmp-server` | `src/main.rs` | The authoritative match server (UDP, protocol v6): sessions, seats, rounds, validation, relay, RCON |
| `hsmp-sidecar` | `src/sidecar/main.rs` | Runs next to each game; bridges the game's shared-memory link to the server |
| `hsmp-master` | `src/master.rs` | Optional HTTP server list (register, heartbeat, `GET /v1/servers`) |
| `hsmp-query` | `src/query_tool.rs` | Server-info queries, used by the in-game browser |
| `hsmp-loadtest` | `src/loadtest.rs` | Bot clients for load and attack tests |

The transport and protocol live in `crates/hsmp-net`. The sidecar, master and query tool include some
of the server's source files with `#[path]` (see `docs/development/architecture.md`, "Shared source
files"). Arena spawn data is compiled in from `data/maps/*.json`, which `hsmp-tools gen-map-data`
generates.

## Build and test

From the repository root:

```sh
cargo build --release --locked -p hsmp-server     # all five binaries, in target/release
cargo test --locked -p hsmp-server
```

## Run

```sh
target/release/hsmp-server --bind 0.0.0.0:7777 --bans-file bans.txt
target/release/hsmp-server --help
```

Set `RUST_LOG=hsmp_server=debug` for verbose logs. Hosting guides, every flag and the RCON
reference are in [docs/hosting](../docs/hosting/dedicated-server.md); the module map is in
[docs/development/server-modules.md](../docs/development/server-modules.md).
