# Master server (server list)

The master server is the list behind the in-game server browser. Game servers register with it
and send heartbeats; the browser downloads the list, then queries each server directly over UDP
for its live player count and ping.

There are two implementations with the same HTTP API:

| | Public list (Cloudflare Worker) | Self-hosted `hsmp-master` |
|---|---|---|
| URL | `https://master.halfswordmp.workers.dev` | `http://<your host>:7778` |
| Transport | HTTPS only | plain HTTP |
| Listing ownership | signed with the server's key | signed when the server signs, else the registering IP |
| Heartbeat / expiry | 120 s / 360 s | 10 s / 30 s |
| Reachability check | none (a Worker cannot send UDP) | one UDP query at registration |
| Source | `master-cf/` + `crates/hsmp-master-core` | `server/src/master.rs` |

The release points the game at the public list first and at `http://127.0.0.1:7778` (a master
on the player's own PC) as the fallback. Players can always join any server by typing its
`ip:port` in the browser's address box, and LAN servers are found by LAN discovery without any
master.

## Registering a game server

Set `HSMP_MASTER_URL` in the game server's environment:

```bash
HSMP_MASTER_URL=https://master.halfswordmp.workers.dev hsmp-server --bind 0.0.0.0:7777 --name "EU Duels" --region EU
```

The in-game HOST does this by itself with the first `master_url` in `hsmp.cfg`.

The server then registers, heartbeats at the interval the master asks for (`heartbeat_s` in the
registration answer; 10 s if the master does not say) with its live player count, arena and
mode, and retries forever with backoff (5, 10, 20, 40, then 60 seconds) if the master is down. On
a clean shutdown (Ctrl+C, RCON `SHUTDOWN`) it removes its listing. Look for `registered with
master` in its log.

What the master lists:

- **host**: the source address of the registration request, never a value the server sends.
  On Cloudflare this is `CF-Connecting-IP`. A server bound to IPv4 (the default `0.0.0.0`)
  registers over IPv4, so it is never listed under an IPv6 address it does not listen on.
- **port**: the port from the server's `--bind`. It is the only part of the address the server
  chooses; the public list refuses ports below 1024.
- name, mode, map, region, players, max players, protocol version and range, server version,
  and `server_key` (the server's identity, which clients pin).

### Listing ownership

The server identity (`server_identity.key`, an X25519 key) cannot sign, so the server derives an
Ed25519 **listing key** from it (HKDF-SHA256). Every register, heartbeat and delete carries
`listing_key` and `ts` (unix ms) in its JSON body and an Ed25519 signature over
`"HSMP master v1\n" METHOD " " PATH "\n" BODY` in the `x-hsmp-sig` header. The master keeps the
listing key and the last `ts` per listing:

- a heartbeat or delete must verify against the listing's key and carry a newer `ts`;
- a registration for a host:port that another key holds is refused (409) until that listing
  expires; the same key re-registering (a restart) keeps its listing and id;
- a heartbeat from a different source address drops the listing (410) and the server registers
  again from its new address.

So nobody can refresh, change or delete another server's listing, even from the same IP
address, and a captured request cannot be replayed. A server keeps its listing key for as long
as it keeps its identity file.

## Public list: Cloudflare Worker

The public list runs on the Cloudflare Workers **free plan** in `master-cf/`: a Rust Worker
(`worker` crate, built to wasm with `worker-build`) and one SQLite-backed Durable Object that holds
the list. The registry logic, validation and signature checks are in `crates/hsmp-master-core`,
with its unit tests in the normal `cargo test --workspace`.

### Why a Durable Object

| Free plan, per day | Limit | 50 servers, 120 s heartbeat |
|---|---|---|
| Worker requests | 100,000 | 36,000 heartbeats + browser refreshes |
| Durable Object requests | 100,000 | the same writes, plus list reads not served from cache |
| SQLite rows written | 100,000 | ~36,000 (one row per heartbeat) |
| SQLite rows read | 5,000,000 | the list is read once per object wake, then kept in memory |
| Durable Object duration | 13,000 GB-s | at most ~10,800 if it never sleeps (128 MB × 24 h) |
| Storage | 5 GB | a few hundred KB |

Workers KV allows only 1,000 writes a day, far too few for heartbeats. D1 has the same row
limits as a SQLite Durable Object, but a Durable Object also serialises every write in one
place (the per-host caps, rate limits and replay checks need that) and keeps the list in memory
between requests. Since SQLite is the only Durable Object storage on the free plan, that is what
`wrangler.toml` declares.

The heartbeat interval and expiry are `HEARTBEAT_S` and `TTL_S` in `wrangler.toml`
(`120` and `360`). The registration answer tells each server the interval, so changing it needs
no server update. At 120 s, 50 servers use 36,000 of the 100,000 daily requests and leave ~64,000
for browser refreshes (about 44 a minute). A crashed server disappears after at most 6 minutes;
the browser shows it as not answering before that. If a day's quota runs out, Cloudflare refuses
requests until midnight UTC; games already running are not affected, and the browser falls back
to `127.0.0.1` and LAN discovery.

### What the Worker enforces

- **HTTPS only.** Plain-HTTP reads are redirected to HTTPS, plain-HTTP writes are refused.
  Every response carries `Strict-Transport-Security`.
- **Signed ownership** as above. Unsigned registrations (servers before signing) get 400.
- **Field checks:** name 1-48, mode 32, map 64 (identifier characters only), region 16 and version
  24 bytes (version: letters, digits, `.+-`); control and invisible Unicode characters are removed;
  max players 1-256 (players clamped to it); protocol numbers range-checked; `server_key` empty or
  64 hex; body at most 4 KiB with a `Content-Length`; `ts` within 10 minutes of the Worker's
  clock at registration.
- **Rate limits:** registrations 6 per host (IPv4 address or IPv6 /64) then 1 a minute, 60 at once
  globally then 1 a second; heartbeats at most one per 30 s per listing; list reads 30 per host then
  one per 2 s (per Worker instance). The list is cached for 5 s.
- **Caps:** 500 servers in total, 8 per host, 32 per network (IPv4 /24, IPv6 /48). A full list
  refuses newcomers; it never evicts live servers.
- **Expiry:** a listing is hidden as soon as its TTL passes and removed by the next request or by a
  Durable Object alarm.
- **No CORS.** No `Access-Control-Allow-*` headers are sent, and any POST or DELETE that carries
  an `Origin` header (a web page) is refused.

`ping_ms` is always 0 and `reachable` always false in its listing (a Worker cannot send UDP);
the browser measures both itself. `/v1/rendezvous` is not implemented (no HSMP client uses it; the
punch relay replaces it).

### Punch relay (NAT traversal)

A server behind a home router that could not open its port (see
[Ports and firewall](ports-and-firewall.md#no-forwarded-port-nat-traversal)) keeps one
WebSocket to the list: `GET /v1/punch/listen/{id}?ts=<unix ms>`, signed with its listing key over
an empty body (same scheme as the writes; `ts` within 5 minutes and newer than the last listen of
that listing). A joiner that gets no answer posts `POST /v1/punch` with the listing's `host` and
`port` and its own public UDP endpoint as STUN saw it from its game socket. The list sends one
message down the host's socket, `{"t":"punch","to":"<ip:port>","nonce":".."}`, and the host's
server sends 4 probes of 16 bytes to that endpoint from its game port. That opens the host's NAT
for the joiner, whose next handshake gets through.

What keeps this from being a traffic cannon:

- the endpoint's IP must be the address the request comes from (`CF-Connecting-IP`), and its port
  at least 1024: a requester can only make a host send probes to itself;
- per requester (IPv4 address or IPv6 /64) 6 requests, then one per 10 s; per listing 12, then one
  per 2 s; 60 at once globally, then 10 a second;
- the host sends at most one burst per endpoint per 2 s and 30 bursts a minute, whatever the
  list asks, and a burst is 64 bytes in all, less than the HTTPS request that caused it.

Listings say how they are reachable: `nat` is `open`, `upnp`, `pcp`, `natpmp`, `double` (mapped,
but behind another NAT), `cone` (punchable), `symmetric` or `unknown`, and `punch` is true while
the host's relay socket is open. Older servers send neither.

On the Worker the socket is accepted by the Durable Object with the hibernation API and tagged
with the listing id; the host's `ping` every 45 s is answered by `setWebSocketAutoResponse`, which
does not wake the object and is not billed. The cost per day for 50 listed servers, on top of the
table above:

| | Requests (Worker and Durable Object each) | Why |
|---|---|---|
| Listen sockets | ~1,200 | one per server start, plus reconnects (a deploy or network change); assume one an hour per server |
| Pings | 0 | auto-response: no wake, not billed (even billed as messages they would be 96,000 / 20 = 4,800) |
| Punch requests | ~1,000 | one per request, 1-2 per join that needs one; assume 1,000 a day. The message down the socket is free (outgoing WebSocket messages are not billed) |
| Total with the heartbeats | ~38,000-43,000 of 100,000 | |

Duration does not grow: a hibernatable socket does not keep the object in memory, and the
heartbeats (one every 2.4 s at 50 servers) already keep it warm, which is at most 128 MB × 24 h
= ~11,000 of the 13,000 GB-s a day. A long-poll instead of the socket would cost a request every
time it times out (50 servers × 90 s = 48,000 a day), and faster heartbeats while a punch is
pending would still leave the host up to one interval late.

### Discord announcements (optional)

With the `DISCORD_WEBHOOK_URL` secret set, the Worker posts "Server up" (name, mode, map, players,
region, version and, unless `DISCORD_SHOW_ADDRESS = "0"`, the `ip:port`) and "Server down"
messages to that webhook. Without the secret nothing is posted and everything else works the
same.

- A restart of the same server is not announced. After an "up", the next "up" for that server
  waits at least 30 minutes, and a "down" is sent only after an "up", so a flapping server posts
  at most two messages per half hour.
- At most 10 messages at once across all servers, then one a minute.
- User text is shown inside code spans and `allowed_mentions` is empty, so names cannot ping,
  link or format. The webhook URL is a secret: it never appears in a response or the code.

### Build and deploy

One-time setup on the deploying machine:

```bash
rustup target add wasm32-unknown-unknown
cargo install worker-build --version 0.8.7 --locked
# Node.js 22 or newer (wrangler 4 needs it)
```

From `master-cf/`:

```bash
npx --yes wrangler@4.147.0 deploy          # runs `worker-build --release`, then uploads
```

The first deploy creates the `Registry` Durable Object class from the `[[migrations]]` entry in
`wrangler.toml` (tag `v1`, `new_sqlite_classes`); nothing has to be created by hand, and the
tables are created by the object on first use. `wrangler` needs `npx wrangler login` or a
`CLOUDFLARE_API_TOKEN` with Workers edit rights for the account in `wrangler.toml`.

Optional Discord webhook:

```bash
npx --yes wrangler@4.147.0 secret put DISCORD_WEBHOOK_URL     # paste the webhook URL
npx --yes wrangler@4.147.0 secret delete DISCORD_WEBHOOK_URL  # turn announcements off
```

Check it:

```bash
curl -s https://master.halfswordmp.workers.dev/v1/health        # ok
curl -s https://master.halfswordmp.workers.dev/v1/servers       # []
npx --yes wrangler@4.147.0 tail                                 # live logs
```

### Running it locally

```bash
cd master-cf
npx --yes wrangler@4.147.0 dev --port 8787 --var HEARTBEAT_S:5 --var TTL_S:15
HSMP_MASTER_URL=http://127.0.0.1:8787 hsmp-server --bind 127.0.0.1:7777
```

Plain HTTP is accepted only on `localhost` / `127.0.0.1`. `scripts/e2e-master-cf.sh` does this
with the real `hsmp-server` and `hsmp-query` (part of `scripts/e2e-test.sh`; skipped without
Node.js 22+, `worker-build` or the wasm target).

## Self-hosted: hsmp-master

You can run your own master for a community: run `hsmp-master` on a host with a public IP, point
your game servers at it with `HSMP_MASTER_URL`, and have your players add its URL in the game.

Read [Limitations of hsmp-master](#limitations-of-hsmp-master) first: it is plain HTTP and trusts
source IP addresses.

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

`hsmp-master` lists **reachable**: whether the server answered one UDP browser query from the
master within 500 ms at registration. Unreachable servers are still listed, marked as such. An
entry disappears 30 seconds after its last heartbeat. A server that restarts replaces its own
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

### Limitations of hsmp-master

- **Plain HTTP.** The list and the registrations travel unencrypted. Someone on the network path can
  read or change the list a player downloads, and send players to a different server. A release
  build refuses a non-local `http://` master in `release.json`, and the launcher refuses one in a
  manifest.
- **Ownership by source IP.** A heartbeat or delete is accepted when it comes from the IP address
  that registered the entry; the signature fields are not checked. Anyone sharing that IP can update
  or delete the listing.
- **Any website can talk to it.** CORS allows every origin.
- **Floodable.** Limits are per IPv4 address or IPv6 /64, and unreachable entries are listed too.

For a public list, use the Worker; it has none of these.

### Putting it behind TLS

A reverse proxy (nginx, Caddy) with a certificate gives players an `https://` list, but `hsmp-master`
takes every server's address from the TCP source address and does not understand
`X-Forwarded-For` or the PROXY protocol. Proxy **only the read side** over HTTPS, and let game
servers register directly:

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

## Players

Players add a master in the game's multiplayer **SETTINGS** screen, in the **SERVER LISTS** field
(comma-separated URLs, primary first), or in `master_url` in `hsmp.cfg` next to the game's `Win64`
binaries. The launcher writes the release's list
(`https://master.halfswordmp.workers.dev, http://127.0.0.1:7778`) and keeps a `master_url` that was
changed by hand. Without any `hsmp.cfg` (a hand-unzipped install) the mods use the public list
`https://master.halfswordmp.workers.dev`.

## HTTP API

All responses are JSON except `/`, `/dashboard` and `/v1/health`. Request bodies are JSON, at most
4 KiB.

| Method and path | Purpose | Responses |
|---|---|---|
| `GET /` and `GET /dashboard` | HTML page of live servers | 200 |
| `GET /v1/health` | Liveness check, body `ok` | 200 |
| `GET /v1/servers` | Live servers, newest heartbeat first | 200, JSON array |
| `POST /v1/register` | A game server registers | 201 `{server_id, ttl_s, secret, heartbeat_s}`; 400 bad input, unsigned (Worker) or clock skew; 403 bad signature; 409 address held by another key, or stale `ts`; 429 throttled or too many servers from this address or network; 503 list full |
| `POST /v1/heartbeat/{id}` | Keep-alive with players, map and mode | 204; 400; 403 bad or missing signature (hsmp-master: not from the registering IP); 409 stale `ts` or nonce replay; 410 unknown, expired or new source address (register again); 429 too soon |
| `DELETE /v1/servers/{id}` | Remove an entry | 204; 400; 403; 404 unknown; 409 stale `ts` |
| `GET /v1/myaddr` | The caller's address as the master sees it: `{ip, port, addr}` | 200 |
| `GET /v1/punch/listen/{id}?ts=..` | A listed host's punch relay WebSocket (signed; hsmp-master: also from the listing's address) | 101; 400 bad path or clock skew; 403 bad signature or not the owner; 404 unknown or expired; 409 stale `ts`; 426 not a WebSocket upgrade (Worker) |
| `POST /v1/punch` | A joiner asks a host to punch: `{host, port, endpoint, nonce}` | 202 `{sent: true}`; 400 bad body, endpoint port below 1024; 403 the endpoint is not the requester's address; 404 not listed; 409 the host has no relay socket; 429 throttled |
| `POST /v1/rendezvous/{room}` | NAT rendezvous helper (hsmp-master only) | not used by any HSMP client |
| `POST /v1/reports` | A bug-report zip from the launcher or `hsmp-server --report` (Worker only; up to 25 MB) | 201 `{id, retention_days}`; 400 not a report; 411; 413; 429 burst or daily cap; 503 not enabled. Admin routes and setup: [Bug reports](../development/bug-reports.md) |

Register body: `name`, `port` (required), `mode`, `map`, `players`, `max_players`, `proto_ver`,
`proto_min`, `proto_max`, `server_key`, `pwd_protected`, `version`, `region`, `listing_key`, `ts`,
`nat`, `punch`, and the legacy `host` (ignored), `nonce` and `hmac`. Heartbeat body: `players`, `map`,
`mode`, `nat`, `ts`, `nonce`, `hmac`. Delete body: `ts`, `nonce`, `hmac`. Signed requests carry `x-hsmp-sig`.

Each entry of `GET /v1/servers` has these fields: `server_id`, `name`, `host`, `port`, `mode`, `map`,
`players`, `max_players`, `proto_ver`, `pwd_protected`, `version`, `region`, `ping_ms` (master to
server at registration, not the player's ping), `reachable`, `last_seen_utc_ms`, `age_s` (seconds
since the last heartbeat), `nat` and `punch` (see [Punch relay](#punch-relay-nat-traversal)); the Worker
adds `proto_min`, `proto_max` and `server_key`.

```bash
curl -s https://master.halfswordmp.workers.dev/v1/servers
curl -s http://203.0.113.5:7778/v1/health
```

## hsmp-master limits

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
