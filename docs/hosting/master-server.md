# Master server (server list)

`hsmp-master` is the server list behind the in-game server browser. Game servers register with it
and send a heartbeat every 10 seconds; the browser downloads the list, then queries each server
directly over UDP for its live player count and ping.

**There is no public HSMP master server yet.** The release points every player's browser at
`http://127.0.0.1:7778`, a master on their own PC, which only lists games hosted on that PC (the
in-game HOST starts it when needed). Players can always join any server by typing its `ip:port` in
the browser's address box, and LAN servers are found by LAN discovery without any master.

You can run your own master for a community: run `hsmp-master` on a host with a public IP, point
your game servers at it with `HSMP_MASTER_URL`, and have your players add its URL in the game.

Read [Current limitations](#current-limitations) first: the master is plain HTTP and trusts source
IP addresses.

## Running it

`hsmp-master` ships next to `hsmp-server` (release `hsmp\` folder, the Docker image, or
`cargo build --release --locked -p hsmp-server --bin hsmp-master`).

```bash
hsmp-master --bind 0.0.0.0:7778
```

| Flag | Default | Meaning |
|---|---|---|
| `--bind <ip:port>` | `0.0.0.0:7778` | HTTP listen address. |
| `--pid-file <path>` | off | Write a pid file once bound. |
| `--parent-pid <pid>` | off | Exit when that process exits. |

Log filter: `RUST_LOG`, default `hsmp_master=info`. The master logs the IP address of every server
that registers.

Open **TCP 7778** in the firewall (see [Ports and firewall](ports-and-firewall.md)). The master needs
no inbound UDP: its reachability check uses a temporary outbound UDP socket.

Everything is kept in memory. After a restart the list is empty, and every running server
re-registers by itself within about 10 seconds (its next heartbeat is refused, so it registers again).

A systemd unit for the master is the same as the server's in [Linux](linux.md#systemd-service), with
`ExecStart=/usr/local/bin/hsmp-master --bind 0.0.0.0:7778` and no state directory. In Docker, see
[Docker](docker.md#with-a-master-server).

## Registering a game server

Set `HSMP_MASTER_URL` in the game server's environment:

```bash
HSMP_MASTER_URL=http://203.0.113.5:7778 hsmp-server --bind 0.0.0.0:7777 --name "EU Duels" --region EU
```

The server then registers, heartbeats every 10 seconds with its live player count, arena and mode,
and retries forever with backoff (5, 10, 20, 40, then 60 seconds) if the master is down. Look for
`registered with master` in its log.

What the master lists:

- **host**: the source IP address of the registration request, never a value the server sends. This
  matters; see the next section.
- **port**: the port from the server's `--bind`.
- name, mode, map, region, players, max players, protocol version, server version.
- **reachable**: whether the server answered one UDP browser query from the master within 500 ms
  at registration. Unreachable servers are still listed, marked as such.

An entry disappears 30 seconds after its last heartbeat. A server that restarts replaces its own
entry (same host and port).

### A master on the same machine

Because the listed host is the address the registration came from, a server that registers through
`http://127.0.0.1:7778` is listed as **`127.0.0.1`**, which only works for players on that same
machine. This is the case for:

- `HSMP_WITH_MASTER=1` in the Docker image,
- `run-dedicated-server.ps1 -WithMaster`,
- any `HSMP_MASTER_URL` that points at `127.0.0.1` or `localhost`.

These setups are fine for testing and for the in-game HOST. For players elsewhere:

- run the master on a **different machine** from your game servers, or
- on a VPS whose public IP is assigned to its network interface, set
  `HSMP_MASTER_URL=http://<the VPS public IP>:7778` so the registration leaves from the public
  address. (Inside Docker this does not help: the master sees a Docker network address.)

Check the result with `curl http://<master>:7778/v1/servers`: `host` must be the address players use.

### Players

Players add your master in the game's multiplayer **SETTINGS** screen, in the **SERVER LISTS** field
(comma-separated URLs, primary first, for example `http://203.0.113.5:7778`), or in `master_url` in
`hsmp.cfg` next to the game's `Win64` binaries. The launcher keeps a `master_url` that was changed by
hand. Both `http://` and `https://` URLs are accepted.

## HTTP API

All responses are JSON except `/`, `/dashboard` and `/v1/health`. Request bodies are JSON, at most
4 KiB.

| Method and path | Purpose | Responses |
|---|---|---|
| `GET /` and `GET /dashboard` | HTML dashboard of live servers, refreshes every 5 seconds | 200 |
| `GET /v1/health` | Liveness check, body `ok` | 200 |
| `GET /v1/servers` | Live servers, newest heartbeat first | 200, JSON array |
| `POST /v1/register` | A game server registers | 201 `{server_id, ttl_s, secret}`; 400 bad input; 429 throttled or too many servers from this address; 503 list full |
| `POST /v1/heartbeat/{id}` | Keep-alive with players, map and mode | 204; 400; 403 not from the registering IP; 409 nonce replay; 410 unknown or expired (re-register); 429 sooner than 2 s after the last one |
| `DELETE /v1/servers/{id}` | Remove an entry | 204; 400; 403 not from the registering IP; 404 unknown |
| `GET /v1/myaddr` | Returns the caller's address as the master sees it: `{ip, port, addr}` | 200 |
| `POST /v1/rendezvous/{room}` | NAT rendezvous helper | Not used by any HSMP client; ignore it |

Each entry of `GET /v1/servers` has these fields: `server_id`, `name`, `host`, `port`, `mode`, `map`,
`players`, `max_players`, `proto_ver`, `pwd_protected`, `version`, `region`, `ping_ms` (master to
server at registration, not the player's ping), `reachable`, `last_seen_utc_ms`, `age_s` (seconds
since the last heartbeat).

```bash
curl -s http://203.0.113.5:7778/v1/servers
curl -s http://203.0.113.5:7778/v1/health
```

## Limits

| Limit | Value |
|---|---|
| Entry lifetime without heartbeat | 30 s (expired entries are hidden at once and removed every 10 s) |
| Registrations | 1 per 60 s per source address (IPv6 per /64) |
| Heartbeats | 1 per 2 s per server |
| Servers | 10,000 in total, 16 per source address. A full list refuses new servers; it never evicts live ones. |
| Text fields | name 48, mode 32, map 64, region 16, version 24 characters; control characters removed |
| Max players | clamped to 1..256 |
| HTTP connections | 1024 at once, 16 per source address; headers and body must arrive within 5 s each; a connection lives at most 30 s |
| Request body | 4 KiB |

## Current limitations

The master is an early, simple service. Know its weaknesses before you run it for strangers:

- **Plain HTTP.** The list and the registrations travel unencrypted. Someone on the network path can
  read or change the list a player downloads, and send players to a different server.
- **Ownership by source IP.** A heartbeat or delete is accepted when it comes from the IP address
  that registered the entry. The heartbeat signature is only checked for its shape, not verified.
  Anyone sharing that IP (the same LAN, carrier-grade NAT, another program on the server) can update
  or delete the listing.
- **Any website can talk to it.** CORS allows every origin, so a web page visited by someone on the
  server's IP can heartbeat or delete that server's listing.
- **Floodable.** Limits are per IPv4 address or IPv6 /64, so someone with many addresses can fill the
  list. Unreachable entries are listed too.
- **No server key.** The listing does not carry the server's identity key, so players cannot check
  that the server they reach is the one that was listed. Their client trusts the key it sees on the
  first connection.
- **No paging or caching** of `GET /v1/servers`; a very full list is a few megabytes per request.
- Names may contain invisible Unicode characters that make look-alike names possible on the HTML
  dashboard (the in-game browser shows ASCII only).

### Putting it behind TLS

A reverse proxy (nginx, Caddy) with a certificate gives players an `https://` list they can trust,
but **the master cannot be fully proxied yet**: it takes every server's address and ownership from
the TCP source address and does not understand `X-Forwarded-For` or the PROXY protocol. Behind a
proxy, every registration would come from the proxy and every server would be listed with the
proxy's address.

What works today: proxy **only the read side** over HTTPS, and let game servers register directly.

- Players use `https://master.example.com` (the proxy), which forwards only `GET /`, `GET /dashboard`,
  `GET /v1/servers` and `GET /v1/health` to the master.
- Game servers use `HSMP_MASTER_URL=http://master.example.com:7778` (direct). If all your game servers
  are yours, allow TCP 7778 only from their addresses in the firewall.

A Caddy example (Caddy obtains the certificate by itself):

```
master.example.com {
    @read {
        method GET
        path / /dashboard /v1/servers /v1/health
    }
    handle @read {
        reverse_proxy 127.0.0.1:7778
    }
    handle {
        respond 404
    }
}
```

Registrations still travel in plain HTTP, and the ownership limitations above remain. Native TLS and
real request signing are planned.
