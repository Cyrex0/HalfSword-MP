# Security policy

HSMP runs an internet-facing UDP server, a master server, a sidecar inside every player's game
session and a launcher that installs signed packages into the game folder. Please report security
problems privately so they can be fixed before they are public.

## Reporting a vulnerability

- Use GitHub's **private vulnerability reporting** for this repository
  (Security tab, "Report a vulnerability").
- Do not open a public issue for a security problem.
- Include what you found, how to reproduce it, the HSMP version (or commit) and the platform.

You should get an answer within a week. Once a fix is released, the report is credited in the
release notes unless you ask otherwise.

Areas where reports are especially welcome:

- the transport and handshake (`crates/hsmp-net`): authentication, replay, amplification, crashes
  on malformed packets;
- the dedicated server and RCON (`server/src`): remote crashes, privilege problems, resource exhaustion;
- the master server (`server/src/master.rs`): listing spoofing or flooding;
- the sidecar and the Lua mods: anything that lets a remote peer, server or master write files or run
  code on a player's PC;
- the launcher and release signing (`launcher/`, `tools/release/`): anything that installs unsigned
  or modified files.

## Server mods

A server can serve UE4SS Lua mods to its players ([docs/hosting/server-mods.md](docs/hosting/server-mods.md)).
**By design, an accepted server mod runs with full access to the player's PC**, like any mod the
player installs: it is not sandboxed, and its environment in HSMPModHost (own globals,
`HSMPNative` hidden, a `require` confined to its folder) keeps mods apart but is **not a security
boundary**. What a server mod does after the player accepted it is therefore not an HSMP
vulnerability; report malicious servers to the server list's operators.

These **are** vulnerabilities, please report them:

- anything that downloads, writes or runs server mod code **without the player's explicit
  ACCEPT** for exactly that set from that server (a consent bypass, a remembered consent that
  covers another set or another server, NEVER not honoured);
- a manifest path that writes **outside the mod cache** (`hsmp_mods/<hash>/`): traversal, absolute,
  drive, UNC or device paths, links, case tricks;
- **unverified bytes** being written into the cache or loaded (a file whose SHA-256 differs from
  the announced manifest, a cached file changed after download);
- a server mod that **impersonates or replaces an HSMP mod** (an `HSMP*` name, a write into
  `ue4ss/Mods`), or obtains `HSMPNative` / the shared-memory IPC through the documented
  environment;
- any file type outside `.lua .json .txt .csv .ini .md`, or precompiled Lua, reaching the cache or
  being loaded;
- a joining player hurting the players in a match beyond the configured rates, or the server
  serving more than the per-player budget.

## Supported versions

Only the latest release receives security fixes during the beta.

## Known limitations

These are known and tracked; reports that add new information are still welcome.

- RCON is plain text. It listens on loopback only unless `--rcon-allow-remote` is given; reach it
  through an SSH tunnel.
- The master server is plain HTTP and identifies registrants by source IP.
- Clients do not yet pin the server key on first contact (trust on first use). Remembered server-mod
  consent is keyed by that server key.
- Server mods cannot be fully unloaded while the game runs (key binds, console commands, some UE4SS
  hooks stay registered but inert; changes to the game world stay). Restart the game after leaving a
  server with mods for a clean state.

## Secrets

Never commit secrets. The release signing key, server identity keys (`*.key`), RCON passwords and
`.env` files are ignored by `.gitignore`; keep the signing key outside the repository.
