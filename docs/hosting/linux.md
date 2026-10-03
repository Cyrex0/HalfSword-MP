# Linux

`hsmp-server` and `hsmp-master` build and run natively on Linux. **No Linux binary is published
yet**, so you build them from source once; it takes a few minutes. If you prefer containers, see
[Docker](docker.md) instead.

CI builds and tests the server on Ubuntu (`.github/workflows/ci.yml`), and the Docker image compiles the same code on
Debian. Windows-only code (a timer-resolution call
and the game-side sidecar) is either compiled out or not built on Linux.

## Build from source

You need git, curl, a C toolchain (for the linker and for the `ring` crypto library used by the HTTPS
client) and Rust via rustup. OpenSSL is not needed: HTTPS uses rustls.

```bash
# Debian / Ubuntu
sudo apt update && sudo apt install -y build-essential git curl ca-certificates
# Fedora / RHEL / Rocky / Alma
sudo dnf install -y gcc git curl
```

```bash
# Rust (as your normal user, not root)
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --profile minimal
. "$HOME/.cargo/env"

git clone https://github.com/Cyrex0/HalfSword-MP.git && cd HalfSword-MP
# The repository pins its toolchain in rust-toolchain.toml (1.98.1):
rustup toolchain install 1.98.1 --profile minimal
cargo build --release --locked -p hsmp-server --bin hsmp-server --bin hsmp-master --bin hsmp-query
```

The binaries are in `target/release/`. Build only these three: the package also contains the
game-side sidecar and test tools, which you do not need on a server.

## Install

```bash
sudo install -m 0755 target/release/hsmp-server target/release/hsmp-master target/release/hsmp-query /usr/local/bin/
hsmp-server --version

# A system user and its state folder (identity key, ban list)
sudo useradd --system --home-dir /var/lib/hsmp --shell /usr/sbin/nologin hsmp
sudo install -d -o hsmp -g hsmp -m 0700 /var/lib/hsmp
```

### Optional: larger UDP buffers

The server asks the kernel for 8 MB UDP send and receive buffers, so bursts to many players are not
dropped. Linux silently caps the request at `net.core.rmem_max` / `wmem_max` (often about 200 KB).
To allow it:

```bash
printf 'net.core.rmem_max = 8388608\nnet.core.wmem_max = 8388608\n' | sudo tee /etc/sysctl.d/90-hsmp.conf
sudo sysctl --system
```

## systemd service

Save as `/etc/systemd/system/hsmp-server.service`:

```ini
[Unit]
Description=HSMP dedicated server (Half Sword multiplayer)
Wants=network-online.target
After=network-online.target

[Service]
Type=simple
User=hsmp
Group=hsmp
WorkingDirectory=/var/lib/hsmp
Environment=HSMP_STATE_DIR=/var/lib/hsmp
Environment=RUST_LOG=hsmp_server=info
Environment=NO_COLOR=1
# Optional: register with a master server (see master-server.md)
#Environment=HSMP_MASTER_URL=http://203.0.113.5:7778
# Optional: RCON password file, root-only (see rcon.md); then add --rcon-bind 127.0.0.1:2345 below
#EnvironmentFile=/etc/hsmp/rcon.env
ExecStart=/usr/local/bin/hsmp-server --bind 0.0.0.0:7777 --name "My HSMP server" --bans-file /var/lib/hsmp/bans.txt --admins-file /var/lib/hsmp/admins.txt
# The server shuts down cleanly (and tells players) on SIGINT; SIGTERM ends it at once.
KillSignal=SIGINT
TimeoutStopSec=15
Restart=on-failure
RestartSec=5

# Hardening
UMask=0077
NoNewPrivileges=yes
ProtectSystem=strict
ProtectHome=yes
ReadWritePaths=/var/lib/hsmp
PrivateTmp=yes
PrivateDevices=yes
ProtectKernelTunables=yes
ProtectKernelModules=yes
ProtectControlGroups=yes
RestrictAddressFamilies=AF_INET AF_INET6 AF_UNIX
RestrictNamespaces=yes
LockPersonality=yes
MemoryDenyWriteExecute=yes
SystemCallArchitectures=native
CapabilityBoundingSet=

[Install]
WantedBy=multi-user.target
```

```bash
sudo systemctl daemon-reload
sudo systemctl enable --now hsmp-server
systemctl status hsmp-server
```

Notes:

- Port 7777 is above 1024, so the service needs no capabilities.
- `Restart=on-failure` restarts after a crash, not after a clean exit (an RCON `SHUTDOWN` or
  `systemctl stop`).
- The identity key is created at the first start as `/var/lib/hsmp/server_identity.key`. Back it up
  (see [Configuration](configuration.md#server-identity-key)).
- Admins: put one player key per line in `/var/lib/hsmp/admins.txt`
  (`sudo -u hsmp tee -a /var/lib/hsmp/admins.txt`). The server re-reads the file when it changes;
  see [Admins](dedicated-server.md#admins).
- Change flags by editing `ExecStart` (see [Configuration](configuration.md)), then
  `sudo systemctl daemon-reload && sudo systemctl restart hsmp-server`.

### A master server

If you run a master (see [Master server](master-server.md)), give it its own unit,
`/etc/systemd/system/hsmp-master.service`: copy the unit above, change `Description`, remove the
`HSMP_STATE_DIR`, `HSMP_MASTER_URL`, `EnvironmentFile` and `ReadWritePaths` lines, set
`Environment=RUST_LOG=hsmp_master=info`, and use:

```ini
ExecStart=/usr/local/bin/hsmp-master --bind 0.0.0.0:7778
```

The master writes no files, so `ProtectSystem=strict` with no writable path is fine.

## Firewall

```bash
# ufw
sudo ufw allow OpenSSH
sudo ufw allow 7777/udp comment 'HSMP game server'
sudo ufw allow 7778/tcp comment 'HSMP master server'   # only if you run a master for others
sudo ufw enable

# firewalld
sudo firewall-cmd --permanent --add-port=7777/udp
sudo firewall-cmd --permanent --add-port=7778/tcp      # only if you run a master for others
sudo firewall-cmd --reload
```

Never open the RCON port; use an SSH tunnel ([RCON](rcon.md)). Also allow UDP 7777 in your cloud
provider's firewall if it has one. More in [Ports and firewall](ports-and-firewall.md).

## Logs

```bash
journalctl -u hsmp-server -f              # follow
journalctl -u hsmp-server --since today
journalctl -u hsmp-server -g "peer joined"
```

The log contains player IP addresses and nicknames ([Logs and privacy](dedicated-server.md#logs-and-privacy)).
How long journald keeps it is set in `/etc/systemd/journald.conf` (`SystemMaxUse=`,
`MaxRetentionSec=`). For example, to keep at most two weeks:

```bash
sudo mkdir -p /etc/systemd/journald.conf.d
printf '[Journal]\nMaxRetentionSec=2week\n' | sudo tee /etc/systemd/journald.conf.d/retention.conf
sudo systemctl restart systemd-journald
```

For more detail temporarily, set `Environment=RUST_LOG=hsmp_server=debug` and restart.

## Upgrading

```bash
cd ~/HalfSword-MP && git pull
cargo build --release --locked -p hsmp-server --bin hsmp-server --bin hsmp-master --bin hsmp-query
sudo install -m 0755 target/release/hsmp-server target/release/hsmp-master target/release/hsmp-query /usr/local/bin/
sudo systemctl restart hsmp-server        # and hsmp-master if you run it
hsmp-server --version
journalctl -u hsmp-server -n 20           # the "server identity" line: same fingerprint as before
```

If `rust-toolchain.toml` names a new Rust version, install it first with
`rustup toolchain install <version> --profile minimal`. The state folder is untouched, so the identity
key and the ban list survive. Players need the matching HSMP release (protocol v6 in this version);
see [Updating](dedicated-server.md#updating).
