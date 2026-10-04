# Logs and diagnostics

What an HSMP server records about itself, where it goes, how to read it, and how to send it to
us when something is wrong.

On this page:

* [Where the logs are](#where-the-logs-are)
* [Log level](#log-level)
* [The stats lines](#the-stats-lines)
* [Lifecycle lines](#lifecycle-lines)
* [The events file](#the-events-file)
* [Sending us a report](#sending-us-a-report)
* [Privacy](#privacy)

---

## Where the logs are

Every server logs to standard output, as before. It also writes files:

| Server | Folder | Files | Kept |
|---|---|---|---|
| Dedicated (Windows, Linux) | `<state dir>\logs\server\` | `server-<YYYYMMDD>.log`, `server-events-<YYYYMMDD>.jsonl` | 14 days, 500 MB in all |
| Dedicated (Windows service, `install-service.ps1`) | `C:\ProgramData\HSMP\server\logs\server\` | the same | the same |
| Docker | the data volume, `/hsmp/data/logs/server/` | the same | the same |
| Listen host (HOST GAME in the menu) | the player's session folder, `%LOCALAPPDATA%\HSMP\logs\<run>\` | `server.log`, `server-events.jsonl` | the newest 10 game runs, 200 MB |

The state dir is `$HSMP_STATE_DIR`, else `%LOCALAPPDATA%\HSMP` (the folder that holds
`server_identity.key`). `--log-dir <folder>` (or `HSMP_LOG_DIR`) puts the files somewhere else;
`--no-log-file` turns them off. Days are UTC. A file that passes 64 MB in one day continues in
`server-<day>.1.log`. The server deletes its own files older than 14 days, and the oldest ones
when all of them pass 500 MB.

## Log level

`--log-level` takes `error`, `warn`, `info` (the default), `debug` or `trace`, or a full filter in
`RUST_LOG` syntax (`hsmp_server=debug,hsmp_net=info`). Without it, `RUST_LOG` applies. Docker:
`-e HSMP_LOG_LEVEL=debug`; the Windows service: `install-service.ps1 -LogLevel debug`. The same
filter applies to standard output and to both files.

## The stats lines

Every 10 seconds the server writes one `stats:` line and one `stats peer` line per connected
player. RCON `REPORT` prints the newest set.

```text
stats: 2 player(s) | tick avg 0.31 ms p99 1.20 ms max 4.0 ms (300 ticks) | cpu 3% | up 182.4 KB/s down 61.0 KB/s | Live round 3 Map_Arena_Pit | combat accepted 14 rejected 2 (body_miss 1, range 1) | lag comp rejects 1 (body_miss 1) | refused joins 0 | master listed as 203.0.113.7:7777 (heartbeat 40 s ago) | nat port map upnp 7777, type cone, port 7777, relay connected | up 3600 s
stats peer 1 nick="Alice" 198.51.100.7:50123: rtt 42 ms (min 35, jitter 4) | loss 0.40% | retransmits 3 | in 30.1 KB/s out 91.2 KB/s | pose in 60.0 Hz relayed 59.8 Hz | budget 128 KB/s
```

| Field | Meaning | Look for |
|---|---|---|
| `tick avg / p99 / max` | Time the server spent in one tick (60 Hz by default), over the last 10 s | p99 above about 10 ms: the machine is too slow or overloaded |
| `cpu` | CPU time of the server process, as a share of one core | |
| `up / down` | Bandwidth to / from all players | `up` near your upload speed: lower `--client-budget-kbps` |
| phase, round, arena | The match state | |
| `combat accepted / rejected` | Final damage verdicts in the last 10 s, rejections by reason code | Many rejections of one code point at that check |
| `lag comp rejects` | The rejections that lag compensation made (`body_miss`, `blade_miss`, `rewind_cap`, `reach`, ...) | |
| `refused joins` | Joins turned away (`version`, `content`, `full`, `banned`, `rate_limited`) | `version` / `content`: players run another HSMP release |
| `master` | `off` (no `HSMP_MASTER_URL`), `listed as <ip:port>` with the last heartbeat, or `NOT listed` with the failures and the last error | |
| `nat` | Router port mapping (`upnp` / `pcp` / `natpmp` and the port, `failed`, `off`), the NAT type STUN saw (`open`, `cone`, `symmetric`), the port players use, and whether the master's punch relay is connected | `failed` and `symmetric`: players outside your network may not reach you |
| `rtt (min, jitter)` | Smoothed round-trip time, its lowest sample and its variation | `rtt` far above `min`: a queue on the path (often the host's upload) |
| `loss`, `retransmits` | Packets lost and resent in the last 10 s | Loss above 2 %: a bad connection |
| `in / out` | Bandwidth from / to this player | |
| `pose in / relayed` | Pose frames per second received from this player, and copies of them sent to the others | `pose in` well under 60: the player's game runs slowly |
| `budget` | This player's current downstream budget (cut while their path is congested) | below the configured budget: congestion |

Every 10 s there is also a line `v5 connection stats` (transport totals) and `v5 transport
stats` (handshakes, drops). A per-sender `pose relay` line from the relay comes with the same
period on builds that have it.

## Lifecycle lines

These are always written at the info level:

| Line | When |
|---|---|
| `hsmp-server starting`, `server identity`, `content check on`, `server config` | Start-up: version, protocol, bind address, name / mode / region, master, RCON on or off, the client budget |
| `registered with master`, `listed address (as the master sees this server)` | The listing and the public address players use |
| `peer joined` / `peer left` / `player left on purpose` / `peer kicked` | Every join and leave, with the peer id, nick, player fingerprint and reason |
| `join refused: version mismatch` / `join refused: content mismatch`, `join rejected: server full`, `banned IP tried to join` | Refusals |
| `seat kept: session resumed`, `match: opponent left on purpose; forfeit` | Reconnects and forfeits |
| `rcon` (command and reply), `promoted to admin`, `ip banned by ...`, `rcon admin change` | Admin actions. RCON passwords are never logged |
| `panic: ...` with a backtrace | A bug in the server: please send us a report |
| `shutdown summary` | Clean exit: uptime, joins, leaves, kicks, refusals, peak players, combat totals, MB sent and received |

## The events file

`server-events-<day>.jsonl` holds every log event as one JSON object per line: `ts` (UTC), `level`,
`target`, `pid`, `message`, and the event's own fields, typed. The 10-second report is in it as
`{"message":"stats event","report":{...}}` with every number of the stats lines. It is meant for
scripts:

```bash
grep '"message":"stats event"' server-events-20261004.jsonl | jq '.report.tick_ms.p99'
```

## Sending us a report

```powershell
hsmp-server --report                    # the last 2 days of logs, redacted, as a zip next to them
hsmp-server --report --report-days 7
hsmp-server --report --report-upload    # also send it to us; prints a report id
hsmp-server --report --log-dir D:\hsmp\logs   # logs somewhere else
```

Docker: `docker exec hsmp hsmp-server --report --log-dir /hsmp/data/logs/server` (the zip lands in
the volume). The command lists every file it put in the zip. Attach the zip to a
[GitHub issue](https://github.com/Cyrex0/HalfSword-MP/issues/new?template=server_host.yml), or
quote the report id. Uploaded reports are readable only by the HSMP developers and are deleted
after about 60 days.

A listen host's `server.log` is part of the player's own bug report from the launcher
(**Create bug report**), because it is in the game run's session folder.

## Privacy

Your own log files keep player IP addresses, ports and nicknames: they are your logs, and you
need them to deal with abuse. Treat them as personal data and keep them no longer than you need
(the server deletes them after 14 days).

A report made with `--report` removes, from every file, before anything is written:

- player IP addresses, each replaced by a number (`<ip-1>`, `<ip-2>`: the same address keeps its
  number, so we can still follow a player), unless you pass `--report-keep-ips`; your server's
  own public address always (`<my-ip>`);
- nicknames, the Windows or Linux user name and home folder, e-mail addresses;
- keys, passwords and tokens (`player_key`, `server_key`, `rcon_password`, `--rcon-password`,
  `token`, `secret`, webhook URLs, `Bearer ...`).

LAN and loopback addresses are kept; they say nothing about who you are.
