# RCON (remote administration)

RCON is a small line-based text protocol over TCP for administering a running `hsmp-server`: list
and kick players, ban and unban, broadcast messages, change the arena, start and stop matches, shut
the server down. An RCON session is always admin, independent of who holds the in-game admin role.

> **Never expose RCON to the internet.** RCON is plain text: the password and every command can be
> read by anyone on the path. Bind it to `127.0.0.1` and reach it through an SSH tunnel. Do not open
> the RCON port in any firewall, router or cloud security group.

## Enabling RCON

RCON is off unless you pass `--rcon-bind`. It needs a password, from `--rcon-password` or (better,
because command lines are visible to other local users) the `HSMP_RCON_PASSWORD` environment
variable.

The server refuses to start when:

- the password is empty, only whitespace, or starts or ends with whitespace;
- `--rcon-bind` is not an `ip:port` (for example `localhost:2345`; use `127.0.0.1:2345`);
- `--rcon-bind` is not a loopback address and `--rcon-allow-remote` is not given;
- `--rcon-allow-remote` is given and the password is shorter than 16 characters.

Use a long random password even on loopback. Generate one:

```bash
openssl rand -base64 24
```

```powershell
$b = New-Object byte[] 24
[System.Security.Cryptography.RandomNumberGenerator]::Create().GetBytes($b)
[Convert]::ToBase64String($b)
```

In this documentation the password is written as `<a long random password>`. Never reuse an example
password from anywhere.

### Linux (systemd)

Keep the password in a root-only environment file:

```bash
sudo install -d -m 0750 /etc/hsmp
printf 'HSMP_RCON_PASSWORD=%s\n' "$(openssl rand -base64 24)" | sudo tee /etc/hsmp/rcon.env >/dev/null
sudo chmod 600 /etc/hsmp/rcon.env
```

Then in the unit from [Linux](linux.md#systemd-service) add `EnvironmentFile=/etc/hsmp/rcon.env`
and append `--rcon-bind 127.0.0.1:2345` to `ExecStart`. Restart the service.

### Windows

Run by hand:

```powershell
$env:HSMP_RCON_PASSWORD = "<a long random password>"
.\hsmp-server.exe --bind 0.0.0.0:7777 --rcon-bind 127.0.0.1:2345
```

For the Windows service, see [Configuration](configuration.md#windows-service-install-serviceps1)
(`nssm set ... AppParameters` and `AppEnvironmentExtra`).

## Connecting through an SSH tunnel

This is the recommended way. The server listens on `127.0.0.1:2345` on the VPS; SSH carries your
connection there, encrypted.

1. On your own PC, open the tunnel (Linux, macOS, and Windows 10 or later have `ssh` built in):

   ```bash
   ssh -N -L 2345:127.0.0.1:2345 user@vps.example.com
   ```

   Leave it running. `-N` means "no remote shell, just the tunnel".
2. In a second terminal, connect to your local end of the tunnel, `127.0.0.1:2345`, with a raw TCP
   client (see below) and log in with `AUTH`.

On Windows, PuTTY can do the same: Connection > SSH > Tunnels, source port `2345`, destination
`127.0.0.1:2345`, Add; then open a second PuTTY session of type **Raw** to `127.0.0.1` port `2345`.

Simpler still: SSH into the server and run the client there, against `127.0.0.1:2345`.

## Clients

Any raw TCP client works. Commands are one line each, ended by a newline. You have **10 seconds**
after connecting to send `AUTH`.

Interactive, with `nc` (netcat) or `ncat`:

```bash
nc 127.0.0.1 2345
AUTH <a long random password>
OK authenticated
STATUS
```

To avoid typing the password inside the 10-second window (and to keep it out of your shell history),
keep it in a file only you can read and pipe it in:

```bash
{ printf 'AUTH %s\n' "$(cat ~/.hsmp-rcon-password)"; cat; } | nc 127.0.0.1 2345
```

For scripts, the developer tool `hsmp-tools` (built from this repository with
`cargo build --release -p hsmp-tools`) has a one-shot `rcon` command. It opens one connection, sends
each line, waits (300 ms by default, or `@<ms>`), and prints the **last** reply:

```bash
hsmp-tools rcon 127.0.0.1:2345 "AUTH <a long random password>" "SAY Restart in 5 minutes"
```

It reads at most 256 bytes per reply, so long `LIST`, `BANS` or `STATUS` answers are cut off; use
`nc` for those. Its arguments, including the password, are visible to other local users while it runs.

## Docker

Inside a container RCON must listen on the container's own interface (`0.0.0.0`), so it is not
loopback from the server's point of view. You therefore need `HSMP_RCON_ALLOW_REMOTE=true` and a
password of at least 16 characters, and you **publish the port on the host's loopback only**:

```bash
docker run -d --name hsmp --restart unless-stopped --stop-signal SIGINT \
  -p 7777:7777/udp \
  -p 127.0.0.1:2345:2345 \
  -e HSMP_RCON_BIND=0.0.0.0:2345 \
  -e HSMP_RCON_ALLOW_REMOTE=true \
  --env-file ./rcon.env \
  -v hsmp-data:/hsmp/data hsmp-server
```

`rcon.env` contains one line, `HSMP_RCON_PASSWORD=<a long random password>`, and is readable only by
you (`chmod 600 rcon.env`). Then connect from the host (or through an SSH tunnel to the host) to
`127.0.0.1:2345`.

`HSMP_RCON_ALLOW_REMOTE` accepts `1`/`true`/`yes`/`on`.

Never use `-p 2345:2345`: that publishes RCON on every interface of the host, and Docker's port
publishing can bypass host firewalls such as ufw.

## Commands

Verbs are **upper case**. Every command answers with one line, except `LIST`, `BANS`, `ADMIN LIST` and `REPORT`, which
end with a line `END`. Before `AUTH`, every other command answers `ERR auth required`.

| Command | Reply | Notes |
|---|---|---|
| `AUTH <password>` | `OK authenticated` or `ERR bad password` | A wrong password closes the connection. |
| `HELP` | one line listing the verbs | |
| `STATUS` | `OK {json}` | Phase, round, match id, arena, best-of, connected peers, the game mode (`mode`, `teams`, `team_rule`, `round_time_s`, `koth_target_s`, `friendly_fire`, `respawn_s`, `team_wins`, `round_kit`), and the roster by seat (nick, peer id, ready, alive, wins, team, kills, deaths, score, respawning, admin, role). |
| `REPORT` | the newest stats report, then `END` | The 10-second `stats:` line and one `stats peer` line per player (see [Logs and diagnostics](logs-and-diagnostics.md#the-stats-lines)). |
| `LIST` | `<id> <nick> <ip:port>` per player, then `END` | The id is what `KICK` and `BAN` take. |
| `KICK <id>` | `OK kicked`, `ERR no such peer` or `ERR <reason>` | The player is disconnected and may rejoin. |
| `BAN <id>` | `OK banned <ip>`, `ERR no such peer` or `ERR <reason>` | Bans the player's IP address permanently, saves the ban list, kicks the player. |
| `BANS` | one IP per line (at most 1000, then `... N more`), then `END` | |
| `UNBAN <ip>` | `OK unbanned`, `ERR not banned` or `ERR bad ip` | Saves the ban list. |
| `SAY <text>` | `OK broadcast` or `ERR empty` | Shown in chat as `SERVER`. |
| `MAP <arena>` | `OK <arena>` or `ERR <reason>` | Lobby only. Accepts a short name (`Pit`), a map name (`Map_Arena_Pit`) or a path. Arenas: Alley, Cellar, EastTower, LordsHall, Pit, Slums, Yard. |
| `START` / `START FORCE` | `OK starting on <arena>` or `ERR <reason>` | Starts the match. Without `FORCE` it waits until enough players are ready (for example `ERR start blocked: 1 of 2 peers ready`). |
| `ABORT` | `OK aborted` or `OK already in the lobby` | Ends the match and returns to the lobby. |
| `BESTOF <n>` | `OK config updated` or `ERR <reason>` | Lobby only. 1 to 31. |
| `KIT <mode> [budget]` | `OK config updated` or `ERR <reason>` | Lobby only. Mode `free`, `classes` or `custom` (or `0`, `1`, `2`). The budget applies to `custom` and is 100 if left out. |
| `MODE <mode>` | `OK config updated` or `ERR <reason>` | Lobby only. `duel`, `ffa`, `teams`, `koth`, `roulette`, `brawl`, `deathmatch` ([Game modes](configuration.md#game-modes)). Refused while a connected player's HSMP is too old for it. |
| `TEAMS <off\|auto\|fixed> [n]` | `OK config updated` or `ERR <reason>` | Lobby only. The team rule and, optionally, the team count (2 to 4). |
| `TEAM <seat> <team>` | `OK team Red` or `ERR <reason>` | Lobby only, `fixed` teams. Puts the player at that seat (see `STATUS`) on team 1 to 4; 0 removes the pick. |
| `ROUNDTIME <s>` | `OK config updated` or `ERR <reason>` | Lobby only. Round clock in seconds, 0 to 1800 (0 = the mode's default). |
| `OPTION <name> <value>` | `OK <what changed>` or `ERR <reason>` | Lobby only. `koth_target <10..600>`, `ff <on\|off>`, `respawn <1..30>`. |
| `ADMIN ADD <peer id \| player id \| key>` | `OK admin <player_id>` | Makes that player an admin by player key; appended to `--admins-file` when one is set. |
| `ADMIN REMOVE <peer id \| player id \| key>` | `OK not admin <player_id>` | |
| `ADMIN LIST` | `OK <key> <owner\|config\|file\|runtime> <online peer N nick\|offline>` lines, then `END` | |
| `SHUTDOWN` | `OK stopping` | Tells every player the server is shutting down, then exits cleanly (exit code 0). |
| `DEBUG KILL <seat>` | `OK ...` or `ERR ...` | Test verb; only works with `--debug-verbs`. Never on a public server. |

Exact `ERR` texts other than those shown come from the match logic and may change between versions.

After `SHUTDOWN`, whether the server comes back depends on your supervisor: systemd with
`Restart=on-failure` leaves it stopped (clean exit), Docker with `--restart unless-stopped` or
`always` starts it again, NSSM restarts it by default.

A typical admin session:

```text
LIST
2 Alice 198.51.100.7:50123
END
ADMIN ADD 2
OK admin 9f2c4e1a0b3d5c7e
ADMIN LIST
OK <64-hex key> file online peer 2 Alice
END
```

`ADMIN ADD` and `ADMIN REMOVE` take a peer id from `LIST`, the 16-hex player id of a connected
player, or a full 64-hex player key (the player need not be online for a key). With
`--admins-file`, `ADMIN ADD` appends the key to the file and `ADMIN REMOVE` deletes it from there;
without one, the change lasts until the server stops. The listen host's own key cannot be removed.

In-game admins come from player keys only (see
[Admins](dedicated-server.md#admins)); joining first gives
nothing. `ADMIN ADD`/`ADMIN REMOVE` change them at run time, and `KICK`/`BAN` remove anyone else.

## Limits and brute-force protection

| Limit | Value |
|---|---|
| Time to authenticate after connecting | 10 seconds |
| Idle time before an authenticated session is closed | 30 minutes |
| Longest command line | 4096 bytes (the session ends) |
| Sessions at once | 16 in total, 4 per source address |
| Failed logins | 5 per source address within 60 seconds, then that address is refused until the minute is over |

IPv6 sources are counted per /64. There is no global limit on failed logins across many addresses,
and no lockout beyond one minute: another reason never to expose RCON. Through an SSH tunnel every
connection comes from `127.0.0.1`, so five wrong passwords lock out everyone using the tunnel for a
minute, and four open sessions fill the per-address limit.

Every RCON connection is logged with its source address, and the match and admin commands (`MAP`, `START`,
`ABORT`, `BESTOF`, `KIT`, `ADMIN`, `DEBUG`) are logged with their replies.
