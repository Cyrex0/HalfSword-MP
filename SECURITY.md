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

## Supported versions

Only the latest release receives security fixes during the beta.

## Known limitations

These are known and tracked; reports that add new information are still welcome.

- RCON is plain text. It listens on loopback only unless `--rcon-allow-remote` is given; reach it
  through an SSH tunnel.
- The master server is plain HTTP and identifies registrants by source IP.
- Clients do not yet pin the server key on first contact (trust on first use).

## Secrets

Never commit secrets. The release signing key, server identity keys (`*.key`), RCON passwords and
`.env` files are ignored by `.gitignore`; keep the signing key outside the repository.
