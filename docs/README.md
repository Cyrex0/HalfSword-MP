# HalfSword-MP documentation

HalfSword-MP (HSMP) is an unofficial multiplayer mod for Half Sword. The documentation is split by
audience. Start with the [project README](../README.md) for an overview.

## Players

| Document | What it covers |
|---|---|
| [Install](players/install.md) | Requirements, installing with the launcher, first launch |
| [Playing](players/playing.md) | The HSMP menus, hosting from the menu, joining, the lobby, controls, the HUD |
| [Troubleshooting](players/troubleshooting.md) | Common problems, logs and crash reports |
| [FAQ](players/faq.md) | Short answers to common questions |
| [Uninstall](players/uninstall.md) | Removing HSMP with the launcher, and by hand |
| [Career saves](players/career-saves.md) | How HSMP protects your single-player career save, and how to restore it |

## Server hosts

| Document | What it covers |
|---|---|
| [Dedicated server](hosting/dedicated-server.md) | Quick start on Windows and Linux, admins, what the server does and does not do |
| [Ports and firewall](hosting/ports-and-firewall.md) | Every port HSMP uses, with firewall commands |
| [Configuration](hosting/configuration.md) | Every `hsmp-server` flag and environment variable |
| [RCON](hosting/rcon.md) | Remote administration, safely (loopback + SSH tunnel) |
| [Master server](hosting/master-server.md) | Running `hsmp-master`, the server list |
| [Docker](hosting/docker.md) | The container image, volumes, the identity key |
| [Linux](hosting/linux.md) | Native Linux build and a systemd unit |

## Contributors

| Document | What it covers |
|---|---|
| [Architecture](development/architecture.md) | How the pieces fit together (diagrams) |
| [Wire protocol](development/protocol.md) | The network protocol (v6) specification |
| [Shared-memory IPC](development/ipc-shared-memory.md) | The game-to-sidecar link: segment layout, primitives, schema, native module |
| [Server modules](development/server-modules.md) | Module map of the server and the sidecar |
| [Lua mod guide](development/lua-mods.md) | Writing and changing the UE4SS Lua mods, and the rules that keep the game alive |
| [Round-reset crash guard](development/crash-rr.md) | The Runtime Vertex Paint crash on level change and how HSMP avoids it |
| [Testing and the gate](development/testing.md) | Event log, G0, e2e, the in-game gate |
| [Developer tools](development/tools.md) | `hsmp-tools`, `hsmp-gate` and the lints |
| [Releasing](development/releasing.md) | Building, signing and publishing a release |
| [UE4SS](development/ue4ss.md) | Which UE4SS build HSMP uses and how to get it |
| [Half Sword modding notes](development/halfsword/README.md) | What is known about the game's internals |

### Subsystems

| Document | What it covers |
|---|---|
| [Replication](development/subsystems/replication.md) | Pose, root and weapon streaming and playback on stand-ins |
| [World replication](development/subsystems/world-replication.md) | Shared weapons and props |
| [Combat](development/subsystems/combat.md) | Hit claims, lag compensation, native hit replay |
| [Combat parity](development/subsystems/combat-parity.md) | Matching single-player damage, effects and gore |
| [Vitals](development/subsystems/vitals.md) | Health, limbs, stamina, bleeding |
| [Classes and loadouts](development/subsystems/classes-loadout.md) | Kits, classes and the host's rules |
| [Spawns](development/subsystems/spawns.md) | Spawn placement and round-start protection |
| [Director](development/subsystems/director.md) and [contract](development/subsystems/director-contract.md) | Server-driven level changes and match flow in the game |
| [Modes](development/subsystems/modes.md) | The game-mode framework |
| [Menu UI](development/subsystems/menu-ui.md) | The main-menu buttons, browser, lobby and UI scaling |
| [HUD](development/subsystems/hud.md) | The in-match HUD |
| [Interaction](development/subsystems/interact.md) | Grabs and shoves between players |

## Data

`docs/arena_static/` holds data extracted from the game's maps (`tools/mapdump`). It is the input of
`hsmp-tools gen-map-data`, which generates `server/data/maps/*.json` and `mods/shared/hsmp_arenas.lua`.
