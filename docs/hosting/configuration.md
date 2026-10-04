# Configuration

`hsmp-server` is configured with command-line flags and a few environment variables. There is no
config file for the server itself; the Windows helper script has one (see
[`dedicated-server.conf`](#dedicated-serverconf-run-dedicated-serverps1)). `hsmp-server --help`
prints the same list as the table below.

## hsmp-server flags

| Flag | Default | Environment fallback | What it does |
|---|---|---|---|
| `--bind <ip:port>` | `0.0.0.0:7777` | | UDP address for players and browser queries. Use `[::]:7777` for IPv6. The master lists this port, unless the router or the NAT maps it to another one (see `--port-map`, `--stun`). |
| `--max-peers <n>` | `8` | | Most players at once, 1 to 64. Further joins are refused. The game side is tested with up to 8 players; see [More than 8 players](#more-than-8-players) before raising it. |
| `--name <text>` | `Half Sword MP` | `HSMP_SERVER_NAME` | Server name in the browser and on the master. Control characters are removed; cut to 48 characters. |
| `--mode <text>` | `duel` | `HSMP_SERVER_MODE` | The game mode the lobby starts with: `duel`, `ffa`, `teams` (team elimination), `koth` (King of the hill), `roulette` (weapon roulette), `brawl` or `deathmatch`. Also the label in the browser (cut to 32 characters); any other text ("Best of 5") is only a label and plays duel. A label starting with "Best of" is rewritten to the live best-of value; a mode other than duel is listed by its name. See [Game modes](#game-modes). |
| `--teams <n>` | `2` | `HSMP_TEAMS` | Team count of the team modes, 2 to 4. |
| `--team-rule <rule>` | `none` | `HSMP_TEAM_RULE` | `auto` (the server balances the teams at START), `fixed` (players pick their team in the lobby, the rest fill the smallest team) or `none` (no teams; team elimination then uses `auto`). Team elimination always has teams; duel and FFA never do. |
| `--round-time <s>` | `0` | `HSMP_ROUND_TIME` | Round clock in seconds, 0 to 1800. 0 = none, except deathmatch (300) and King of the hill (240). |
| `--koth-target <s>` | `60` | `HSMP_KOTH_TARGET` | King of the hill: seconds a player or team must hold the hill alone to win a round, 10 to 600. |
| `--friendly-fire` | off | `HSMP_FRIENDLY_FIRE` | Team modes: teammates can hurt each other. Off by default: the server drops a hit between teammates. |
| `--respawn-delay <s>` | `3` | `HSMP_RESPAWN_S` | Deathmatch: seconds from a death to the respawn order, 1 to 30. |
| `--map <arena>` | empty | `HSMP_LOBBY_MAP` | Starting arena, also advertised so joiners load the same arena, for example `Map_Arena_Pit`. Arenas: `Map_Arena_Alley`, `Map_Arena_Pit`, `Map_Arena_Yard`, `Map_Arena_Slums`, `Map_Arena_Cellar`, `Map_Arena_LordsHall`, `Map_Arena_EastTower`. Empty or unknown means Alley. |
| `--region <tag>` | empty | `HSMP_REGION` | Region tag in the browser, for example `EU` or `NA-East` (cut to 16 characters). |
| `--bans-file <path>` | `bans.txt` | | Persistent ban list. A relative path is relative to the **working directory**. See [Ban list](#ban-list). |
| `--key-file <path>` | see [identity key](#server-identity-key) | `HSMP_STATE_DIR` (folder) | Server identity key. Created on first run. |
| `--admin-key <64 hex>` | none | | Dedicated server: a player key (`hsmp-sidecar --print-player-key`) that is admin. Repeatable or comma-separated. See [Admins](dedicated-server.md#admins). |
| `--admins-file <path>` | off | | Dedicated server: admin player keys, one per line (`#` comments). Re-read when it changes; RCON `ADMIN ADD` appends to it. |
| `--owner-key <64 hex>` | off | | Listen server: the host's player key; that player is the owner (admin) whenever connected. |
| `--owner-key-file <path>` | off | | Listen server: a file holding the host's player key (the in-game HOST menu passes `<state>/.player_key`). |
| `--rcon-bind <ip:port>` | off | | Turns RCON on. Must be an IP address and port, for example `127.0.0.1:2345` (not a host name). See [RCON](rcon.md). |
| `--rcon-password <text>` | none | `HSMP_RCON_PASSWORD` | RCON password; required with `--rcon-bind`. Must not be blank or start or end with whitespace. Prefer the environment variable: command lines are visible to other local users. |
| `--rcon-allow-remote` | off | `HSMP_RCON_ALLOW_REMOTE` | Accept a non-loopback `--rcon-bind`. Requires a password of at least 16 characters. Needed in Docker (see [RCON](rcon.md#docker)); otherwise use an SSH tunnel. |
| `--client-budget-kbps <n>` | `128` | | Downstream budget per player for replicated streams, in KB/s. Streams are thinned by distance to fit. 128 KB/s fits 8 players; raise it only on a well-connected host with more players. The server lowers it on its own for a player whose path shows a queue (a full home upload), down to a quarter. Upload needed at the full budget: about 1 Mbit/s per remote player. |
| `--tick-hz <n>` | `60` | `HSMP_TICK_HZ` | Server tick rate, 20 to 240. How often the server runs the match flow, releases held hits and judges parries. Every timer (countdowns, timeouts, grace periods) is in real time, so the value never changes game timings; it only trades decision latency against CPU. Poses are relayed when they arrive, not on the tick. 60 is the measured sweet spot ([tick rate](../development/tick-rate.md)). |
| `--content-hash <64 hex>` | built in | | The content hash to enforce instead of the one this build embeds (`hsmp-server --build-info` prints it). Clients with other mod files or server data are refused with "Server runs HalfSword-MP X, you have Y". |
| `--allow-mismatched-content` | off | `HSMP_ALLOW_MISMATCHED_CONTENT` | Development only: no content check. The protocol version is still checked. |
| `--debug-verbs` | off | | Enables the RCON test verb `DEBUG KILL <seat>`. **Never on a public server.** |
| `--events <path>` | off | | Appends structured JSONL match events to a file (used by the test tools). |
| `--pid-file <path>` | off | | Writes a small JSON file with the process id at start and removes it on a clean exit. |
| `--parent-pid <pid>` | off | | Exit when that process exits (used when hosting from the game). |
| `--log-dir <folder>` | `<state dir>/logs/server` | `HSMP_LOG_DIR` | Daily log files and the JSON-lines events file, kept 14 days / 500 MB ([Logs and diagnostics](logs-and-diagnostics.md)). |
| `--no-log-file` | off | | Standard output only. |
| `--log-level <level>` | info | `RUST_LOG` | `error`, `warn`, `info`, `debug`, `trace`, or a `RUST_LOG`-style filter. |
| `--report` | | | Write a redacted bug-report zip of the recent logs and exit (`--report-out`, `--report-upload`, `--report-keep-ips`, `--report-days`). |
| `--port-map <auto, off>` | `auto` | `HSMP_PORT_MAP` | Open the UDP port on the router (UPnP-IGD, PCP, NAT-PMP), renew the lease, remove it on a clean shutdown. Never for a `127.0.0.1` or IPv6 bind. See [Ports and firewall](ports-and-firewall.md#automatic-upnp-pcp-nat-pmp). |
| `--stun <host:port,...>` | Cloudflare, Google, Nextcloud | `HSMP_STUN_SERVERS` | STUN servers asked from the game port for the public address and NAT type. `off` = no NAT detection and no hole punching. Off by default on a `127.0.0.1` bind. |
| `--punch <auto, off>` | `auto` | `HSMP_PUNCH` | Take hole-punch requests the server list relays when the router port is not open (needs STUN and `HSMP_MASTER_URL`). |
| `--mods-dir <dir>` | off | `HSMP_MODS_DIR` | Serve the UE4SS Lua mods in this folder (one sub-folder per mod) to every player, after a warning they accept. An invalid mod stops the server. See [Server mods](server-mods.md). |
| `--mods-max-mb <n>` | `64` | `HSMP_MODS_MAX_MB` | Server mods: largest total size, 1 to 64 MiB. |
| `--mods-timeout-s <s>` | `300` | `HSMP_MODS_TIMEOUT_S` | Server mods: time a joining player has to accept, download and load them (30 to 3600). |
| `--mods-rate-kbps <n>` | `2048` | `HSMP_MODS_RATE_KBPS` | Server mods: download rate per joining player, KiB/s. |
| `--mods-total-rate-kbps <n>` | `8192` | `HSMP_MODS_TOTAL_RATE_KBPS` | Server mods: download rate of all joining players together, KiB/s. |
| `-V`, `--version` | | | Print the version. |

How the environment fallbacks work: `HSMP_SERVER_NAME`, `HSMP_SERVER_MODE`, `HSMP_LOBBY_MAP` and
`HSMP_REGION` are used only when the matching flag is absent (or given with its default value). A
flag with any other value wins. `HSMP_RCON_PASSWORD` is used only when `--rcon-password` is absent.

### Game modes

The mode is a lobby setting: the host changes it on the lobby's **MODE** screen, RCON with `MODE`,
`TEAMS`, `TEAM`, `ROUNDTIME` and `OPTION` ([rcon.md](rcon.md#commands)); the flags above only set
what the lobby starts with. It is frozen for a match like the arena and the kit rules.

| Mode | Rules |
|---|---|
| `duel`, `ffa` | Last player standing wins the round. |
| `teams` | Team elimination: last team standing. 2 to 4 teams, `auto` or `fixed`. |
| `koth` | King of the hill: a player (or team) alone on the hill scores; the first to the target wins the round; deaths still eliminate; at the round clock the most points win. |
| `roulette` | Everyone fights with the same random weapon and armour set, a new one each round. |
| `brawl` | Fists only, no armour. |
| `deathmatch` | Respawns for the round clock; most kills wins the round; a tie goes to sudden death (the next kill, 60 s). |

`koth`, `roulette`, `brawl` and `deathmatch` take teams too (`--team-rule auto|fixed`). The hill is
the centroid of the arena's spawn points; override it per arena with
`HSMP_KOTH_ZONES="Map_Arena_Pit:x,y,z,radius;..."` (cm).

**Older clients.** Every mode but `duel` and `ffa` needs an HSMP newer than 0.1.0-beta.5 on every
player: while such a mode is set, older clients are refused at the handshake with "this server is
playing <mode>, which needs a newer HSMP", and the lobby cannot switch to one while an older client
is connected.

### More than 8 players

`--max-peers` accepts up to 64, and the server side handles 16 players without trouble: the
tick costs about 6 µs at 16 players, and relaying 16 players takes about 12 to 17 % of one core
in `hsmp-loadtest`. What limits a bigger match today is the game side and the upload:

- The game is tested with up to 8 players. Each remote player needs a stand-in body, taken from
  the arena's own fighters (capped by the game's "Free Mode Foes Amount"), and the mods have
  not been tested with more than 7 of them.
- Upload: every player receives every other player's stream, thinned by distance to fit
  `--client-budget-kbps` (128 KB/s by default). At 16 players the far players arrive at a lower
  rate (about 8 to 17 Hz instead of 30 to 60 Hz); the two nearest stay at 30 Hz or more. The
  host needs about 1 Mbit/s of upload per player at the full budget.
- At most 4 players may join from one public IP address (LAN and loopback addresses are not
  limited).

### Environment variables

| Variable | Default | What it does |
|---|---|---|
| `HSMP_STATE_DIR` | unset | Folder for the identity key (`server_identity.key`). Set it on every server you run as a service. The Docker image sets `/hsmp/data`. |
| `HSMP_MASTER_URL` | unset | Register with this master server, for example the public list `https://master.halfswordmp.workers.dev` or `http://203.0.113.5:7778`. Unset means the server is not listed anywhere. The helper scripts and the Docker image set the public list by default (opt out with `-NoMaster` or `HSMP_MASTER_URL=off`). See [Master server](master-server.md). |
| `HSMP_SERVER_NAME`, `HSMP_SERVER_MODE`, `HSMP_LOBBY_MAP`, `HSMP_REGION` | unset | Fallbacks for `--name`, `--mode`, `--map`, `--region` (see above). |
| `HSMP_KOTH_ZONES` | unset | King of the hill zones per arena: `Map:x,y,z,radius` entries (cm), `;`-separated. Arenas not listed use the centroid of their spawn points. |
| `HSMP_RCON_PASSWORD` | unset | RCON password (fallback for `--rcon-password`). |
| `HSMP_RCON_ALLOW_REMOTE` | unset | `1`, `true`, `yes` or `on` act like `--rcon-allow-remote`; `0`, `false`, `no` or `off` leave it off. Leave it unset unless you need it. |
| `HSMP_KIT_MODE` | `free` | Initial kit rules: `free` (`0`), `classes` (`1`) or `custom` (`2`). Anything else means `free`. Can be changed in the lobby (RCON `KIT`). |
| `HSMP_KIT_BUDGET` | `30` | Initial point budget for `custom` kits, 1 to 200. |
| `HSMP_TICK_HZ` | unset | Fallback for `--tick-hz`. |
| `HSMP_PERF` | unset | `1` logs performance counters (tick time, packet rates, bandwidth drops) every 5 seconds. |
| `HSMP_LISTEN_HOST` | unset | Set to `1` by HOST GAME in the menu: the server closes when the hosting player leaves. Do not set it on a dedicated server. |
| `RUST_LOG` | `hsmp_server=info` | Log filter, for example `hsmp_server=debug` or `hsmp_server=warn`. For the master the default is `hsmp_master=info`. |
| `NO_COLOR` | unset | Any non-empty value turns off colour codes in the log. Useful for log files and journald. |

### Examples

A public duel server in Europe, listed on your own master:

```bash
HSMP_STATE_DIR=/var/lib/hsmp \
HSMP_MASTER_URL=http://203.0.113.5:7778 \
hsmp-server --bind 0.0.0.0:7777 --name "EU Duels" --region EU --map Map_Arena_Pit \
  --bans-file /var/lib/hsmp/bans.txt
```

Two servers on one host need different ports, ban files and identity keys:

```bash
HSMP_STATE_DIR=/var/lib/hsmp/a hsmp-server --bind 0.0.0.0:7777 --bans-file /var/lib/hsmp/a/bans.txt --name "Server A"
HSMP_STATE_DIR=/var/lib/hsmp/b hsmp-server --bind 0.0.0.0:7779 --bans-file /var/lib/hsmp/b/bans.txt --name "Server B"
```

LAN discovery only finds servers on ports 7777 to 7786, so keep LAN servers in that range (and away
from 7778 if a master runs on the same machine, which is why the example skips it).

## Server identity key

Each server has a long-term identity: an X25519 key pair. The private key is a 32-byte file,
`server_identity.key`. Players' clients remember the public half the first time they connect to an
address ("trust on first use", stored in their `%LOCALAPPDATA%\HSMP\known_servers.json`). Within
one session the key is pinned: a reconnect to a server with another key fails. If the key changes
between visits, the server looks like a **different server** at the same address, and the
player's log shows `SERVER IDENTITY CHANGED since the last visit`. So:

- **Keep the key** across restarts, updates and reinstalls.
- **Back it up**, and copy it along when you move the server to a new machine.
- **Keep it private.** Whoever has it can impersonate your server. On Linux, `chmod 600` it and make
  it owned by the user the server runs as.

Where the key lives:

1. `--key-file <path>` if given.
2. Else `$HSMP_STATE_DIR/server_identity.key` if `HSMP_STATE_DIR` is set.
3. Else `%LOCALAPPDATA%\HSMP\server_identity.key` (Windows; for a service running as LocalSystem that
   is the system profile, which is why `install-service.ps1` sets `HSMP_STATE_DIR`).
4. Else `server_identity.key` in the working directory (Linux without `HSMP_STATE_DIR`).

The server creates the key (and its folder) on the first start. If the file exists but is not exactly
32 bytes, the server logs a warning and **replaces it with a new key**, so never edit it.

At every start the server logs its identity, for example (timestamp left out):

```
INFO hsmp_server: server identity key_file=/var/lib/hsmp/server_identity.key server_key=<64 hex> fingerprint=<short id> proto=v6..=v6
```

Note the fingerprint: after a restore it must be the same.

Backup and restore:

```bash
# Linux
sudo cp -p /var/lib/hsmp/server_identity.key /root/hsmp-identity-backup.key
sudo systemctl stop hsmp-server
sudo install -o hsmp -g hsmp -m 600 /root/hsmp-identity-backup.key /var/lib/hsmp/server_identity.key
sudo systemctl start hsmp-server
```

```powershell
# Windows service (StateDir default)
Copy-Item "$env:ProgramData\HSMP\server\server_identity.key" "D:\Backup\hsmp-identity.key"
```

For Docker, see [Docker](docker.md#backup-and-restore).

## Ban list

`--bans-file` (default `bans.txt` in the working directory) holds banned IP addresses, one per line.
`#` starts a comment and blank lines are ignored. IPv4 and IPv6 addresses are accepted; invalid lines
are skipped with a warning.

- The file is read once, at startup. A missing file means an empty list.
- It is rewritten (atomically) on every ban or unban, whether from RCON or the in-game admin. A
  rewrite keeps only a header line and the addresses, so your own comments are lost.
- A ban covers one exact IP address. It is permanent until you remove it.
- To unban while the server runs, use RCON `UNBAN <ip>`. To edit the file by hand, stop the server
  first, edit, then start it (otherwise the next ban overwrites your edit).

```
# hsmp-server banlist — one IP per line
198.51.100.9
2001:db8::5
```

The ban list is personal data (IP addresses); see [Logs and privacy](dedicated-server.md#logs-and-privacy).

## Admin list

`--admins-file <path>` holds the player keys of a dedicated server's admins, one 64-hex key per
line. `#` starts a comment; blank lines are ignored. A player prints their key with
`hsmp-sidecar --print-player-key` (see [Admins](dedicated-server.md#admins)).

- A missing file means no admins from the file. The server starts anyway.
- The file is checked again before every join and re-read when it changed, so you can edit it
  while the server runs.
- RCON `ADMIN ADD` appends a key to it. `ADMIN REMOVE` takes a key out of it.
- `--admin-key` adds keys on the command line; both sources count.

```
# HSMP admins
# Alice
<64-hex player key>
```

## hsmp-master flags

| Flag | Default | What it does |
|---|---|---|
| `--bind <ip:port>` | `0.0.0.0:7778` | HTTP address of the master. |
| `--pid-file <path>` | off | Writes a small JSON file with the process id once the port is bound; removed on a clean exit. |
| `--parent-pid <pid>` | off | Exit when that process exits (used when hosting from the game). |

The master keeps everything in memory and has no other settings. `RUST_LOG` defaults to
`hsmp_master=info`. See [Master server](master-server.md).

## Windows helper scripts

### dedicated-server.conf (`run-dedicated-server.ps1`)

`scripts\run-dedicated-server.ps1` starts `hsmp-server.exe` (and optionally `hsmp-master.exe`) from
a small `key = value` file. Parameters given on the command line override the file.

| Parameter | Default | Meaning |
|---|---|---|
| `-Config <path>` | `dedicated-server.conf` in the repository root, or in `-BinDir` when given | Config file. Written with defaults on first run. |
| `-BinDir <folder>` | `$env:CARGO_TARGET_DIR\release`, else `<repo>\target\release` | Folder with `hsmp-server.exe` (and `hsmp-master.exe`), for example the `hsmp\` folder of a release. |
| `-AdminKey <key>` | none | Admin player key (`--admin-key`). Repeatable: `-AdminKey k1,k2`. Added to `admin_keys`. |
| `-AdminsFile <path>` | config `admins_file` | `--admins-file` |
| `-MasterUrl <url>` | config `master_url` | Server list to register with; `off` = not listed. |
| `-NoMaster` | off | LAN-only: register with no server list (same as `master_url = off`). |
| `-Region <tag>` | config `region` | `--region` |
| `-Map <arena>` | config `map` | `--map` |
| `-RconBind <ip:port>` | config `rcon_bind` | `--rcon-bind` |
| `-RconPassword <text>` | config `rcon_password` | Turns RCON on. Passed to the server as `HSMP_RCON_PASSWORD`, not on its command line. |
| `-WithMaster` | off | Also start `hsmp-master.exe` on `master_bind` and register the server with it instead. |
| `-DebugLog` | off | `RUST_LOG=debug` for the server and master. |

Config keys and their defaults:

| Key | Default | Passed as |
|---|---|---|
| `bind` | `0.0.0.0:7777` | `--bind` |
| `tick_hz` | `60` | `--tick-hz` (20 to 240) |
| `max_peers` | `8` | `--max-peers` |
| `name` | `HSMP Dedicated` | `--name` (write it without quotes: the value is everything after `=`, trimmed) |
| `mode` | `duel` | `--mode` |
| `map` | empty | `--map` (when set) |
| `region` | empty | `--region` (when set) |
| `master_url` | `https://master.halfswordmp.workers.dev` | `HSMP_MASTER_URL`. `off` (or empty) = LAN-only. Ignored with `-WithMaster`, which uses the local master. |
| `master_bind` | `0.0.0.0:7778` | `hsmp-master --bind` (only with `-WithMaster`) |
| `bans_file` | `bans.txt` | `--bans-file` (a relative path is next to the config file) |
| `admins_file` | `admins.txt` | `--admins-file` (a relative path is next to the config file; a missing file is an empty list) |
| `admin_keys` | empty | `--admin-key` per key, comma-separated |
| `rcon_bind` | `127.0.0.1:2345` | `--rcon-bind`, only when a password is set |
| `rcon_password` | empty | `HSMP_RCON_PASSWORD`; RCON stays off without it (or without `$env:HSMP_RCON_PASSWORD`) |

Lines starting with `#` are comments. A config written by an older version has an empty
`master_url`, which still means LAN-only: set it to the public URL to be listed.

```powershell
# Public server in Europe, you as admin, RCON on localhost
.\scripts\run-dedicated-server.ps1 -BinDir C:\HSMP\server -AdminKey <your player key> -Region EU -RconPassword "<16+ characters>"
# LAN-only
.\scripts\run-dedicated-server.ps1 -BinDir C:\HSMP\server -NoMaster
```

The script does not set `HSMP_STATE_DIR`, so the identity key goes to `%LOCALAPPDATA%\HSMP\`
unless you set it first. A master started with `-WithMaster` lists the server as `127.0.0.1` (see
[Master server](master-server.md#a-master-on-the-same-machine)), so it is only useful on that machine
or for testing.


### Windows service (`install-service.ps1`)

`scripts\install-service.ps1` installs `hsmp-server.exe` as a Windows service with
[NSSM](https://nssm.cc). Run it in an **administrator** PowerShell. NSSM must be on `PATH`; if it is
not and Chocolatey is installed, the script installs NSSM with `choco install nssm`. Running it again
replaces the service with the new settings.

| Parameter | Default | Meaning |
|---|---|---|
| `-BinDir <folder>` | `$env:CARGO_TARGET_DIR\release`, else `<repo>\target\release` | Folder with `hsmp-server.exe`. |
| `-StateDir <folder>` | `%ProgramData%\HSMP\server` | Identity key, `bans.txt`, `admins.txt` and `logs\`. Set as `HSMP_STATE_DIR`. |
| `-Bind <ip:port>` | `0.0.0.0:7777` | `--bind` |
| `-Name <text>` | `HSMP Dedicated` | `--name` |
| `-Mode <mode>` | `duel` | `--mode` |
| `-Map <arena>` | none | `--map` |
| `-Region <tag>` | none | `--region` |
| `-AdminKey <key>` | none | `--admin-key`. Repeatable: `-AdminKey k1,k2`. |
| `-AdminsFile <path>` | `<StateDir>\admins.txt` | `--admins-file` |
| `-MasterUrl <url>` | `https://master.halfswordmp.workers.dev` | `HSMP_MASTER_URL`; `off` = not listed. |
| `-NoMaster` | off | LAN-only. |
| `-RconBind <ip:port>` | `127.0.0.1:2345` | `--rcon-bind`, only with `-RconPassword` |
| `-RconPassword <text>` | none | Turns RCON on; stored in the service environment as `HSMP_RCON_PASSWORD`. |
| `-ServiceName <name>` | `HsmpDedicatedServer` | Windows service name. |
| `-DryRun` | | Print the command line and environment; install nothing (no administrator needed). |
| `-Uninstall` | | Stop and remove the service. The state folder is kept. |

```powershell
.\scripts\install-service.ps1 -BinDir C:\HSMP\server -AdminKey <your player key> -Region EU
nssm status HsmpDedicatedServer
nssm restart HsmpDedicatedServer
.\scripts\install-service.ps1 -Uninstall
```

The service runs as LocalSystem, starts at boot, uses `--max-peers 8` and
`--bans-file <StateDir>\bans.txt`, and writes `logs\hsmp-server.stdout.log` and
`hsmp-server.stderr.log` under the state folder, rotated at 10 MB. Rotated files are kept until you
delete them. The script does **not** add a firewall rule (see [Ports and firewall](ports-and-firewall.md#windows-firewall))
and turns on RCON only with `-RconPassword`. The admins file is always passed, so admins can also be
added later by editing `<StateDir>\admins.txt` (re-read when it changes) or with RCON `ADMIN ADD`.

To change settings, run the script again with the new parameters (it replaces the service). You can
also edit them with NSSM; `AppEnvironmentExtra` replaces the whole list, so always repeat
`HSMP_STATE_DIR` and `HSMP_MASTER_URL`:

```powershell
nssm set HsmpDedicatedServer AppParameters --bind 0.0.0.0:7777 --name MyServer --max-peers 8 --bans-file C:\ProgramData\HSMP\server\bans.txt --admins-file C:\ProgramData\HSMP\server\admins.txt --rcon-bind 127.0.0.1:2345
nssm set HsmpDedicatedServer AppEnvironmentExtra HSMP_STATE_DIR=C:\ProgramData\HSMP\server HSMP_MASTER_URL=https://master.halfswordmp.workers.dev HSMP_RCON_PASSWORD=<a long random password> NO_COLOR=1
nssm restart HsmpDedicatedServer
```

NSSM stores these settings in the registry, readable by administrators. `nssm edit HsmpDedicatedServer`
opens the same settings in a window.
