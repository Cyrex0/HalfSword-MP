# Running a dedicated server

`hsmp-server` is the HalfSword-MP game server. It is a small Rust program that speaks UDP. It does
**not** need Half Sword, a GPU or a Windows desktop: players run the game, and the HSMP helper on
their PC (`hsmp-sidecar`) connects to your server. It runs on Windows and Linux, on a home PC, a
VPS or in Docker.

This page is the quick start. The details live on these pages:

| Page | What it covers |
|---|---|
| [Ports and firewall](ports-and-firewall.md) | Every port, firewall commands, router forwarding, CGNAT |
| [Configuration](configuration.md) | Every flag and environment variable, the identity key, the ban list, the admin list |
| [RCON](rcon.md) | Remote administration through an SSH tunnel |
| [Master server](master-server.md) | Running your own server list (`hsmp-master`) |
| [Docker](docker.md) | The container image, volumes, backups, compose |
| [Linux](linux.md) | Building from source, systemd, firewall, logs |

## Quick start

A server needs three things: the program, an open UDP port (default **7777**), and at least one
admin, so that someone can pick the arena and start matches. Without an admin the server still
works; see [No admin connected](#no-admin-connected).

### Step 1: get your player key

Admins are named by their **player key**: 64 hex characters that identify one HSMP install. Each
player who should be admin runs this once on their own gaming PC, in the game's `hsmp` folder
(`<game dir>\HalfswordUE5\Binaries\Win64\hsmp\`):

```powershell
.\hsmp-sidecar.exe --print-player-key
```

It prints one line of 64 hex characters. The key is created on first use and stored in
`%LOCALAPPDATA%\HSMP\identity`, so it stays the same across HSMP updates and reinstalls. It is the
public half of the identity, not a secret: every server the player joins sees it. After a player has
hosted or joined once, the same key is also in `<game dir>\HalfswordUE5\Binaries\Win64\hsmp_state\.player_key`.

### Step 2a: Windows

1. Get `hsmp-server.exe`. It ships in the `hsmp\` folder of the HSMP release: in a game install that
   is `<game dir>\HalfswordUE5\Binaries\Win64\hsmp\`, in the release zip `payload\Win64\hsmp\`. Copy
   the folder somewhere of its own, for example `C:\HSMP\server`. Or build it from source:
   `cargo build --release --locked -p hsmp-server` (output in `target\release\`).
2. Open the firewall for UDP 7777 (administrator PowerShell):

   ```powershell
   New-NetFirewallRule -DisplayName "HSMP server (UDP 7777)" -Direction Inbound -Protocol UDP -LocalPort 7777 -Action Allow
   ```

3. Forward UDP 7777 on your router to this PC (see [Ports and firewall](ports-and-firewall.md)).
4. Create `C:\HSMP\server\admins.txt` with one player key per line (`#` starts a comment):

   ```text
   # Alice
   <Alice's 64-hex player key>
   ```

5. Start the server:

   ```powershell
   cd C:\HSMP\server
   $env:HSMP_STATE_DIR = "C:\HSMP\server"
   $env:HSMP_MASTER_URL = "https://master.halfswordmp.workers.dev"   # leave out for a LAN-only server
   .\hsmp-server.exe --bind 0.0.0.0:7777 --name "My HSMP server" --region EU --bans-file .\bans.txt --admins-file .\admins.txt
   ```

6. Players find it on the public server list in **SERVER BROWSER**, or type `<your public IP>:7777`
   into the address box and press **CONNECT**. Stop the server with Ctrl+C.

The same with the helper script, which writes a `dedicated-server.conf` and registers with the
public list unless you pass `-NoMaster`:

```powershell
.\scripts\run-dedicated-server.ps1 -BinDir C:\HSMP\server -AdminKey <Alice's key> -Region EU
```

To start it at boot as a Windows service, use `scripts\install-service.ps1`, which takes the same
`-AdminKey`, `-AdminsFile`, `-Region`, `-Map`, `-NoMaster` and RCON parameters (see
[Configuration](configuration.md#windows-service-install-serviceps1)).

### Step 2b: Linux

No Linux binary is published yet; build it once (details in [Linux](linux.md)):

```bash
sudo apt install -y build-essential git curl        # Debian/Ubuntu: C toolchain for linking
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --profile minimal
. "$HOME/.cargo/env"
git clone https://github.com/Cyrex0/HalfSword-MP.git && cd HalfSword-MP
cargo build --release --locked -p hsmp-server --bin hsmp-server --bin hsmp-master
sudo install -m 0755 target/release/hsmp-server target/release/hsmp-master /usr/local/bin/
sudo ufw allow 7777/udp
mkdir -p "$HOME/hsmp-state"
echo "<64-hex player key>" > "$HOME/hsmp-state/admins.txt"
HSMP_STATE_DIR="$HOME/hsmp-state" hsmp-server --bind 0.0.0.0:7777 --name "My HSMP server" \
  --bans-file "$HOME/hsmp-state/bans.txt" --admins-file "$HOME/hsmp-state/admins.txt"
```

For a permanent install, use the systemd unit in [Linux](linux.md#systemd-service).

### Step 2c: Docker

No prebuilt image is published yet; build it from the repository root (details in [Docker](docker.md)):

```bash
git clone https://github.com/Cyrex0/HalfSword-MP.git && cd HalfSword-MP
docker build -t hsmp-server .
docker run -d --name hsmp --restart unless-stopped \
  -p 7777:7777/udp -v hsmp-data:/hsmp/data \
  -e HSMP_NAME="My HSMP server" hsmp-server
docker exec hsmp sh -c 'echo "<64-hex player key>" >> /hsmp/data/admins.txt'
docker logs -f hsmp
```

The image always passes `--admins-file /hsmp/data/admins.txt`, and the server re-reads that file
when it changes, so no restart is needed. The `hsmp-data` volume also holds the server identity key
and `bans.txt`. Keep it and back it up.

### Step 3: check it

The startup log shows the server identity (`server identity ... fingerprint=...`) and the admin
policy, for example `admin policy: configured admins only` with the number of admins. From a machine outside your
network, `hsmp-query --direct <your public IP>:7777` sends the same query the in-game browser sends
(see [Testing reachability](ports-and-firewall.md#testing-reachability)).

## Admins

An admin can pick the arena, the number of rounds and the kit rules, start and abort matches, and
kick, ban and unban players. A dedicated server has **no admin by default**: joining first gives
nothing. There are four ways to make a player admin, all by player key:

| How | Lasts | Example |
|---|---|---|
| `--admin-key <key>` | while the server runs with that flag | `--admin-key <64-hex key>` (repeat the flag, or separate keys with commas) |
| `--admins-file <path>` | as long as the key is in the file; the file is re-read when it changes | one key per line, `#` comments |
| RCON `ADMIN ADD <peer id \| player id \| key>` | written to `--admins-file` when one is set, otherwise until the server stops | `ADMIN ADD 2` (peer id from `LIST`) |
| **MAKE HOST** in the lobby, by an admin | until the server stops | the granting admin keeps admin too |

RCON also has `ADMIN REMOVE <...>` and `ADMIN LIST`; see [RCON](rcon.md). An RCON session is always
admin itself.

In-game admins see banned IPs masked (for example `203.0.x.x#1a2b3c4d`) and can unban by that
entry. Full IP addresses are shown only over RCON (`BANS`).

A **listen server** started from the in-game **HOST GAME** button needs none of this: the menu
starts the server with `--owner-key-file <state>\.player_key`, so the hosting player is admin by
their own key, keeps it after a reconnect, and nobody else inherits it.

### No admin connected

When no admin is connected, the lobby runs itself:

- At least 2 players must be connected, and all of them must press **READY**.
- The server then starts the match after 5 seconds. Anyone un-readying cancels the start.
- The lobby shows the match settings read-only, plus **AUTO START (n/m)** (n of m players ready).

The arena, rounds and kit rules are then whatever the server was started with (`--map`,
`HSMP_KIT_MODE`, `HSMP_KIT_BUDGET`, best of 3), or what RCON set.

## What the server does

- **Runs the match.** The server decides the arena, the mode and the match flow: lobby, ready-up,
  rounds (best of 1 to 31, default 3), scores and the result screen. The game clients follow it.
- **Checks the game.** It relays player movement, and validates hit claims (with lag
  compensation), loadouts and interactions before they reach the other players.
- Holds up to `--max-peers` players (default **8**). At most 4 players may join from one public IP
  address (IPv6: one /64). LAN and loopback addresses are not limited.
- Keeps a player's seat and wins for 30 seconds after a connection drop, so they can reconnect.
- Answers server-browser queries on its game port (name, map, mode, players, ping), so players can
  check it by address and find it by LAN discovery, even without a server list.
- Registers with a master server (server list) if you set `HSMP_MASTER_URL`, for example the
  public list `https://master.halfswordmp.workers.dev` (see [Master server](master-server.md)).
  `run-dedicated-server.ps1`, `install-service.ps1` and the Docker image use the public list by
  default; `-NoMaster` (Docker: `HSMP_MASTER_URL=off`) keeps a server LAN-only.
- Keeps a persistent IP ban list (`--bans-file`) and a persistent identity key
  (see [Configuration](configuration.md#server-identity-key)).
- Offers optional remote administration over [RCON](rcon.md).

All traffic between the server and players is encrypted (X25519 key exchange, ChaCha20-Poly1305).
Players identify themselves with an Ed25519 key, which is the player key above.

## What the server does not do

- **No NAT traversal.** The server must be reachable on its UDP port: forward the port on your
  router, or run it on a host with a public IP. If your ISP uses carrier-grade NAT, port
  forwarding cannot work; use a VPS (see [Ports and firewall](ports-and-firewall.md#cgnat)).
- **One rule set.** `--mode` (and `HSMP_SERVER_MODE`) changes the label in the browser and the lobby.
  Every match is played as rounds with the last fighter standing winning the round, best of N.
- **No join password.** Anyone who can reach the port can join. To restrict a server to known
  players, allow only their IP addresses in your firewall
  (see [Ports and firewall](ports-and-firewall.md#allow-only-known-players)).
- **Same build only.** The server refuses clients whose mod files or server data differ from its
  own build (the content hash `hsmp-server --build-info` prints). Update the server with every
  release; `--allow-mismatched-content` turns the check off for development.
- No voice relay, no asset downloads (players install the mod themselves), no statistics or
  accounts.

## Security

### RCON

RCON is plain text: the password and every command travel unencrypted. It is off by default. The
server refuses a non-loopback `--rcon-bind` unless you add `--rcon-allow-remote`, and then requires
a password of at least 16 characters. **Bind RCON to `127.0.0.1` and reach it through an SSH
tunnel.** Never open the RCON port in a firewall. See [RCON](rcon.md).

### Never on a public server

- `--debug-verbs`: enables a test-only RCON verb that kills a player.
- `--rcon-allow-remote` with the RCON port reachable from the internet.
- A `--tick-hz` other than 30: many game timers are counted in ticks and assume 30 Hz.

## Updating

The server and the players' mod must speak the same network protocol. This release speaks
**protocol v6 only**; a player with another protocol version is turned away at connect with a
readable message. Update the server whenever a new HSMP release comes out, and tell your players
to update too.

1. Stop the server (Ctrl+C, `systemctl stop hsmp-server`, `nssm stop HsmpDedicatedServer` or
   `docker stop hsmp`).
2. Replace `hsmp-server` (and `hsmp-master` if you run one) with the new build, or rebuild the image.
3. Start it again. The identity key, `bans.txt` and `admins.txt` are kept, so players see the same
   server.

`hsmp-server --version` prints the version. The `server identity` line in the startup log shows the
protocol (`proto=v6..=v6`) and the key fingerprint, which must not change across an update.

## Logs and privacy

The server logs to standard output. `RUST_LOG` sets the detail (default `hsmp_server=info`;
see [Configuration](configuration.md#environment-variables)). Where the log ends up depends on how
you run it: the console, `journalctl` (systemd), NSSM's log files (Windows service) or
`docker logs`.

What the logs contain:

- For every join: the player's **IP address and port, nickname and player fingerprint**.
- RCON connections with their source address, and the RCON match and admin commands with their
  replies.
- Kicks, bans and admin changes; match phases and round results.
- The master server logs the IP address of every server that registers.
- Chat text is **not** logged (only its length).

`bans.txt` keeps banned IP addresses until you remove them. The server does not delete or rotate
logs. Retention is up to you: decide how long you keep them, rotate them (journald, NSSM rotation,
Docker `--log-opt max-size`), and tell your players. Depending on where you and they live, IP
addresses and nicknames can be personal data.

## Troubleshooting

| Symptom | Likely cause | Fix |
|---|---|---|
| Players on your LAN can join, players outside cannot | Port not forwarded, firewall, or CGNAT | [Ports and firewall](ports-and-firewall.md); test with `hsmp-query` from outside |
| Nobody gets the admin tools | No admin configured, or a wrong key | Check `ADMIN LIST` over RCON, or the `admin policy` line in the startup log; compare with `hsmp-sidecar --print-player-key` on the player's PC |
| Server exits at start: `the RCON password must not be blank ...` | Empty or space-padded RCON password | Use a long random password, see [RCON](rcon.md) |
| Server exits at start: `--rcon-bind ... is not a loopback address` | RCON bound to `0.0.0.0` | Bind `127.0.0.1:2345` and use an SSH tunnel |
| Server exits at start: `--rcon-bind set but --rcon-password / HSMP_RCON_PASSWORD not provided` | No RCON password | Set `HSMP_RCON_PASSWORD` |
| Server exits at start: address in use | Another server on the same port | Use another `--bind` port or stop the other process |
| Players see "banned" | Their IP is in `bans.txt` | RCON `UNBAN <ip>`, or edit the file while the server is stopped |
| Players' HSMP logs report a changed server identity | The identity key was lost or replaced | Restore `server_identity.key` from your backup |
| Server is missing from your master's list | `HSMP_MASTER_URL` unset or wrong, or registration throttled | Check the log for `registered with master`; see [Master server](master-server.md) |
| Your master lists the server as `127.0.0.1` | Server registered through a loopback URL | See [Master server](master-server.md#a-master-on-the-same-machine) |
