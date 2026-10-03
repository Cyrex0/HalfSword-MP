# Changelog

All notable changes to HalfSword-MP are documented here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and versions follow
[Semantic Versioning](https://semver.org/). The product version is the `version` in
`tools/release/release.json`; releases are tagged `v<version>`. The network protocol and the
game-to-sidecar IPC have their own versions (currently protocol 6, IPC ABI 2).

## [Unreleased]

## [0.1.0-beta.2] - 2026-10-03

### Server list

- There is now a public server list, run on Cloudflare at `https://master.halfswordmp.workers.dev`.
  Servers show up in the in-game browser without anyone running their own list.
- Listings are signed. Each server signs its register, heartbeat and delete requests with a key
  derived from its identity, so nobody else can take over or remove its entry. A server removes its
  listing when it shuts down cleanly.

### Launcher

- The launcher updates itself. It checks the GitHub releases at start and on "Check for updates",
  shows what is new, downloads the zip, checks its SHA-256 and signature, and installs it. Betas are
  offered by default; "Stable releases only" turns them off. On the command line: `check-update`
  and `update`.
- No more Play button. Install with the launcher, then start Half Sword from Steam as usual. The
  checks Play used to do now run when the launcher opens and after every install or update.

### Compatibility checks

- Players and servers compare versions and mod files before a match. Joining a server that runs a
  different HalfSword-MP version, or different mod files, is refused with a message that says which
  version the server runs and what to do.
- The server browser greys out servers you cannot join ("needs vX") and has a COMPATIBLE filter.

### Career saves

- Career-save recovery now runs when the game boots, before the game can save. If a crash left an
  MP session open, your single-player career save is put back first.

### Dedicated servers

- `run-dedicated-server.ps1` and `install-service.ps1` take admin keys, an admins file, region, map
  and RCON settings. Servers join the public server list by default; `-NoMaster` (or
  `master_url = off`) keeps a server LAN-only. `install-service.ps1` replaces an existing service
  when run again and has `-DryRun`.
- The Docker image gets `HSMP_ADMIN_KEYS`, `HSMP_MAP`, `HSMP_REGION` and the same server-list
  default (`HSMP_MASTER_URL=off` for LAN-only).

### Licences

- The release zip now includes `THIRD-PARTY-NOTICES.html` with the licences of the Rust crates,
  Lua, the launcher fonts and the components bundled in UE4SS.

## [0.1.0-beta.1] - 2026-10-03

First public beta.

### Multiplayer

- Player-versus-player matches for 2 to 8 players: lobby, ready-up, arena pick, best-of-N rounds,
  round reset and match result. The server is authoritative over the map, the mode and the match
  flow; the Director in `HSMPMatch` is the only code that changes levels.
- Full-body replication: each player's root, 16-bone pose and held weapon are sampled in the game,
  streamed, and played back on physics stand-ins with a jitter buffer sized for internet links.
- Combat parity with single-player: hits are replayed through the game's own damage code on the
  victim's game, with the same hit effects, blood and dismemberment. The server validates every
  damage claim with lag compensation (rewind, swept blades, clash and block adjudication) and
  per-player limits.
- Vitals (health, limbs, stamina, bleeding, falls) and world objects are kept in sync.
- Loadouts and classes under the host's rules.
- In-game menus on Half Sword's main menu: HOST GAME, a server browser (LAN list, the master
  server list and DIRECT CONNECT), settings and character screens, a lobby, and an in-match HUD
  showing each fighter's CON, BODY % and BLEED. All HSMP widgets scale with the window size and
  DPI, including ultrawide and 4:3 screens.

### Engine integration

- The game and the sidecar talk over shared memory: a native UE4SS module (`HSMPNative`) maps one
  segment per game process, laid out by a versioned `#[repr(C)]` schema with an ABI and layout-hash
  check on both sides. Local pose sampling and the stand-in servo run natively.
- A guard against the round-reset crash in the game's Runtime Vertex Paint plugin: the paint queue
  is closed and drained before every level change.
- The launcher applies `r.HairStrands.Streaming=0` to avoid a known engine crash while loading
  arenas.

### Servers and security

- Encrypted transport (protocol 6): X25519 key exchange, ChaCha20-Poly1305, stateless cookies and
  a per-install Ed25519 player identity.
- Dedicated server for Windows and Linux (no game needed), a Docker image, RCON (loopback only
  unless `--rcon-allow-remote`), a ban list, and an optional self-hosted server list
  (`hsmp-master`).
- Admins are named by player key (`--admin-key`, `--admins-file`, RCON `ADMIN ADD`); a listen
  host is the owner through `--owner-key-file`. Nobody becomes admin by joining first.

### Players

- A launcher that checks the game build, installs and removes the signed release byte-exactly,
  and backs up saves.
- Career-save protection: the single-player career save is backed up before a session and
  restored after it.

[Unreleased]: https://github.com/Cyrex0/HalfSword-MP/compare/v0.1.0-beta.2...HEAD
[0.1.0-beta.2]: https://github.com/Cyrex0/HalfSword-MP/compare/v0.1.0-beta.1...v0.1.0-beta.2
[0.1.0-beta.1]: https://github.com/Cyrex0/HalfSword-MP/releases/tag/v0.1.0-beta.1
