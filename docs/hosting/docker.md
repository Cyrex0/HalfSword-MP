# Docker

The repository's `Dockerfile` builds a small Linux image with `hsmp-server` and `hsmp-master`. No game
files are needed. **No prebuilt image is published yet**: build it yourself from the repository root.

## Build

```bash
git clone https://github.com/Cyrex0/HalfSword-MP.git && cd HalfSword-MP
docker build -t hsmp-server .
```

The build compiles the server with the Rust toolchain pinned in `rust-toolchain.toml` (1.98.1) and
copies the binaries into a `debian:bookworm-slim` runtime image. The first build takes a few minutes.
The image runs as the unprivileged user `hsmp` (uid 1000) under `tini`.

## Run

Game server only:

```bash
docker run -d --name hsmp --restart unless-stopped --stop-signal SIGINT \
  -p 7777:7777/udp \
  -v hsmp-data:/hsmp/data \
  -e HSMP_NAME="My HSMP server" \
  hsmp-server
```

- `-p 7777:7777/udp`: the game port. Remember `/udp`.
- `-v hsmp-data:/hsmp/data`: a named volume for the identity key and the ban list. **Always mount
  it.** Without it, a new container gets a new identity and an empty ban list.
- `--stop-signal SIGINT`: the server shuts down cleanly (and tells players) on SIGINT. The image already sets `STOPSIGNAL SIGINT`; the flag keeps it explicit. With Docker's
  default SIGTERM it exits at once and players see a timeout instead.

Open UDP 7777 in the host firewall and any cloud firewall (see [Ports and firewall](ports-and-firewall.md)).

### Environment variables

The entry script (`scripts/docker-entry.sh`) turns these into flags:

| Variable | Default in the image | Becomes |
|---|---|---|
| `HSMP_BIND` | `0.0.0.0:7777` | `--bind` |
| `HSMP_MAX_PEERS` | `8` | `--max-peers` |
| `HSMP_NAME` | `HSMP Dedicated` | `--name` |
| `HSMP_MODE` | `duel` | `--mode` (a label only) |
| `HSMP_BANS_FILE` | `/hsmp/data/bans.txt` | `--bans-file` |
| `HSMP_ADMINS_FILE` | `/hsmp/data/admins.txt` | `--admins-file`: admin player keys, one per line (see [Admins](dedicated-server.md#admins)); RCON `ADMIN ADD` appends to it |
| `HSMP_STATE_DIR` | `/hsmp/data` | read by the server: folder of `server_identity.key` |
| `HSMP_RCON_BIND` | unset | `--rcon-bind` (RCON off when unset) |
| `HSMP_RCON_PASSWORD` | unset | read by the server |
| `HSMP_RCON_ALLOW_REMOTE` | unset | read by the server; must be `1` (or `true`) for RCON in a container (see [RCON](rcon.md#docker)) |
| `HSMP_WITH_MASTER` | `0` | `1` also starts `hsmp-master` in the container |
| `HSMP_MASTER_BIND` | `0.0.0.0:7778` | `hsmp-master --bind` (with `HSMP_WITH_MASTER=1`) |
| `RUST_LOG` | `hsmp_server=info,hsmp_master=info` | log filter |

Every other server variable from [Configuration](configuration.md#environment-variables) is passed
through as is, for example `HSMP_REGION`, `HSMP_LOBBY_MAP`, `HSMP_KIT_MODE`, `HSMP_KIT_BUDGET`,
`HSMP_MASTER_URL`, `HSMP_PERF`. Use `HSMP_NAME` for the name: `HSMP_SERVER_NAME` has no effect in the
image, because the entry script always passes `--name`. `--tick-hz` is not exposed; it stays at 30.

To pass flags the entry script does not know, override the entry point:

```bash
docker run -d --name hsmp ... --entrypoint /usr/bin/tini hsmp-server \
  -g -- /usr/local/bin/hsmp-server --bind 0.0.0.0:7777 --bans-file /hsmp/data/bans.txt \
  --client-budget-kbps 192
```

### Admins

The image always runs the server with `--admins-file /hsmp/data/admins.txt`. Put one player key
per line in that file (see [Admins](dedicated-server.md#admins)). The server re-reads it when it
changes, so this works while the container runs:

```bash
docker exec hsmp sh -c 'echo "<64-hex player key>  # Alice" >> /hsmp/data/admins.txt'
```

RCON `ADMIN ADD` writes to the same file.

### With RCON

RCON is published on the host's loopback only; see [RCON](rcon.md#docker) for the full recipe and
the reasons:

```bash
docker run -d --name hsmp --restart unless-stopped --stop-signal SIGINT \
  -p 7777:7777/udp -p 127.0.0.1:2345:2345 \
  -e HSMP_RCON_BIND=0.0.0.0:2345 -e HSMP_RCON_ALLOW_REMOTE=true --env-file ./rcon.env \
  -v hsmp-data:/hsmp/data hsmp-server
```

`rcon.env` holds `HSMP_RCON_PASSWORD=<a long random password>` (generate it with
`openssl rand -base64 24`; at least 16 characters). Never publish 2345 without `127.0.0.1:`.

### With a master server

```bash
docker run -d --name hsmp ... -p 7777:7777/udp -p 7778:7778 -e HSMP_WITH_MASTER=1 \
  -v hsmp-data:/hsmp/data hsmp-server
```

The server then registers with the master in the same container through `127.0.0.1`, so the master
lists it as `127.0.0.1:7777`. That is only useful on the same machine, for testing. For a real server
list, run the master on another host, or natively (see
[Master server](master-server.md#a-master-on-the-same-machine)).

To register a containerised server with a master elsewhere, set
`-e HSMP_MASTER_URL=http://<master address>:7778` and leave `HSMP_WITH_MASTER` at 0.

## Compose

Save as `docker-compose.yml` in the repository root, then `docker compose up -d --build`:

```yaml
services:
  hsmp:
    build: .
    image: hsmp-server
    container_name: hsmp
    restart: unless-stopped
    stop_signal: SIGINT
    ports:
      - "7777:7777/udp"
      # RCON, host loopback only (see docs/hosting/rcon.md):
      # - "127.0.0.1:2345:2345"
    environment:
      HSMP_NAME: "My HSMP server"
      HSMP_MAX_PEERS: "8"
      HSMP_REGION: "EU"
      # HSMP_RCON_BIND: "0.0.0.0:2345"
      # HSMP_RCON_ALLOW_REMOTE: "true"
    # env_file: ./rcon.env          # HSMP_RCON_PASSWORD=<a long random password>
    volumes:
      - hsmp-data:/hsmp/data
    logging:
      driver: json-file
      options:
        max-size: "10m"
        max-file: "5"

volumes:
  hsmp-data:
```

## The identity key and the data volume

`/hsmp/data` holds:

| File | What it is |
|---|---|
| `server_identity.key` | The server identity. Players' clients remember it; losing it makes your server look like a different server. See [Configuration](configuration.md#server-identity-key). |
| `bans.txt` | The ban list (IP addresses). |
| `admins.txt` | The admin player keys (created when you add the first one). |

The entry script sets `umask 077`, so new files are readable by the container user only.

Find the identity in the log:

```bash
docker logs hsmp 2>&1 | grep "server identity"
```

### Backup and restore

Back up the whole volume (stop the container first so the ban list is not being written):

```bash
docker stop hsmp
docker run --rm -v hsmp-data:/data -v "$PWD":/backup debian:bookworm-slim \
  tar czf /backup/hsmp-data.tgz -C /data .
docker start hsmp
```

Or copy just the key out of the container (works while it runs):

```bash
docker cp hsmp:/hsmp/data/server_identity.key ./server_identity.key
```

Restore into a new or empty volume, before the first start (otherwise the server creates a new key):

```bash
docker volume create hsmp-data
docker run --rm -v hsmp-data:/data -v "$PWD":/backup debian:bookworm-slim \
  sh -c 'tar xzf /backup/hsmp-data.tgz -C /data && chown -R 1000:1000 /data && chmod 600 /data/server_identity.key'
```

Keep backups private: the key lets anyone impersonate your server.

### Using a host folder instead of a named volume

`-v /srv/hsmp:/hsmp/data` works too, but the folder must be writable by uid 1000:

```bash
sudo mkdir -p /srv/hsmp && sudo chown 1000:1000 /srv/hsmp && sudo chmod 700 /srv/hsmp
```

## Logs

```bash
docker logs -f hsmp
docker logs --since 1h hsmp
```

The log contains player IP addresses and nicknames (see
[Logs and privacy](dedicated-server.md#logs-and-privacy)). Docker's default `json-file` driver keeps
logs forever; limit them with `--log-opt max-size=10m --log-opt max-file=5` (or the `logging`
section in the compose file above). Add `-e NO_COLOR=1` for logs without colour codes.

## Upgrading

```bash
cd HalfSword-MP && git pull
docker build -t hsmp-server .
docker stop hsmp && docker rm hsmp
docker run -d --name hsmp ...   # the same command as before, with the same -v hsmp-data:/hsmp/data
```

With compose: `git pull && docker compose up -d --build`.

The volume, and with it the identity key and the ban list, survives. Players must run the same HSMP
release (protocol v6 in this version); see [Updating](dedicated-server.md#updating).
