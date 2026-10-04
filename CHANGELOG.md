# Changelog

All notable changes to HalfSword-MP are documented here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and versions follow
[Semantic Versioning](https://semver.org/). The product version is the `version` in
`tools/release/release.json`; releases are tagged `v<version>`. The network protocol and the
game-to-sidecar IPC have their own versions (currently protocol 6, IPC ABI 2).

## [Unreleased]

### Changed

- Less work per frame in the game mods: the stand-in driver, the pose sender, the world
  replication tick and the IPC facade allocate far less Lua garbage per frame (the stand-in
  driver about 25 times less), and the pose sender makes one native call per sample.
- Damage: two blows that the game would gate in single player are gated in MP too when their
  replays arrive more than 0.2 s apart; a stand-in that took a blow natively no longer turns
  the next frames of the same contact into extra claims.
- Tools: `hsmp-combat-sim --hvf` / `--hvf-logs` measure the server's Hit Velocity ceiling
  (`hit_vel_factor`) from the sim and from in-game logs; the impact rescale log line carries the
  weapon class.
- Stand-ins get their owner's body: bone masses, Mass Scale and Muscle Rate (new `body`
  record, capability `BODY`; beta.4 peers neither send nor receive it). Height is carried but
  not applied yet.
- The lobby menu waits briefly for the server's answer to a command before it shows a result
  read from the session state, so a lost packet at high ping no longer shows a guessed result.
- Developer: `hsmp-gate g0 --skip <checks>`; CI runs G0, the end-to-end suite and clippy as
  parallel jobs.

## [0.1.0-beta.4] - 2026-10-04

### What's new

- Spectator camera: when you are out, you get a follow camera on the other fighters and an
  arena view. No more black screen.
- Fixed a crash when switching between spectate targets.
- Damage now matches single player across all armour types. Hits are replayed at the right
  spot on the body, so armour protects the way it does offline.
- Weapon and body hit types are carried over the network: a pommel strike no longer cuts.
- Smoother play at high ping: no more rubber banding, and hits are accepted at 150-300 ms ping.
- No more stretched or broken bodies when fighters spawn.
- Props and items stay in sync: who owns them, smooth movement, and two players grabbing the
  same item no longer both get it.
- Hosting is easier: the router opens the hosting port by itself (UPnP, PCP or NAT-PMP), and
  players behind a router that blocks incoming connections can still join through hole
  punching.
- Every game session writes its own logs, and the launcher has a "Create bug report" button
  that collects them into a zip you can save or attach to a GitHub issue.
- Servers write logs and regular stats lines.
- Fixed the hair-streaming crash a few seconds into the first arena: hair is now drawn as hair
  cards.

### Launcher

- "Upload to HSMP" in the bug report window only appears when the release says the server list
  takes reports (`report_upload` in `release.json`, off for this release). Use "Save report zip"
  or "Open GitHub issue". If an upload cannot reach the service, the launcher says so and points
  at those two buttons.

### Hosting

- **The router port opens automatically.** A hosted server asks the router for its UDP port
  (UPnP-IGD, then PCP, then NAT-PMP), renews the lease and removes it when it stops. The host
  lobby says "Router port opened automatically (UPnP)" or, when it could not, which UDP port to
  forward by hand. **SETTINGS > HOSTING > ROUTER PORT: OFF**, or `hsmp-server --port-map off`,
  turns it off.
- **NAT traversal.** A host whose port is still closed can be joined through the server list: the
  joiner's game asks the list for a punch, the host sends a few small probes towards the joiner,
  and the normal handshake follows. The browser shows such servers with **NAT** in PING; the
  joiner's lobby reads "CONNECTING THROUGH YOUR ROUTER..." meanwhile, and "The host's network
  blocks incoming connections; ask them to forward UDP <port>" when nothing gets through.
- The server learns its public address with STUN from its game port and lists the port that
  works (the router's or the NAT's mapped port). Listings carry `nat` and `punch`.
- The HOST PORT is always 1024-65535 when the server starts: a hand-edited port 0 used to make
  the OS pick a random port, which was then listed.

### Server list

- Punch relay: `GET /v1/punch/listen/{id}` (a signed WebSocket per listed host, held with the
  Durable Object hibernation API on the Worker) and `POST /v1/punch` (the requester's own endpoint
  only, rate-limited). Deploy the Worker for the relay; older Workers simply have none.

### World objects

- Props, dropped and thrown weapons stay together on every screen at higher ping. A body another
  player simulates plays in that player's timeline while they hold or push it, and is shown
  where it is now once it flies free; corrections blend instead of snapping. In the simulator
  at ~180 ms RTT the median gap between two screens drops from 67 to 37 cm and snaps from 14 per
  run to none.
- Pushing a prop another player pushed before hands it over once you are clearly nearer.
- Two players grabbing the same weapon: the one the server picks keeps it; the other lets go
  (it no longer stays in both hands).
- A prop you pushed no longer jumps back to where it started after it comes to rest.

### Fixes

- **No more crash a few seconds into the first arena of a session** (an engine crash on the
  IoDispatcher thread, `PAK_ASYNC_READ_OOB`). Hair is now drawn with the game's hair cards
  instead of hair strands (`r.HairStrands.UseCardsInsteadOfStrands=1`, set by the launcher, the
  Steam launch options and the mod), so close-up hair is less detailed. The crash was more likely
  on slow connections.
- In the first arena of a session, a newly spawned fighter is hidden for dressing 0.5 s after it
  appears instead of at once, so for that half second it is seen in its base clothes.

## [0.1.0-beta.3] - 2026-10-03

### Launcher

- **Launch through Steam** is back. It starts Half Sword through Steam with the hair-streaming
  crash workaround (`r.HairStrands.Streaming=0`) on the command line, after the same career-save
  and `Engine.ini` checks the launcher runs at start. beta.2 removed it by mistake along with Play.
  Starting from Steam directly still works. On the command line: `hsmp-launcher launch`.

### Hosting

- The launcher adds a Windows Firewall rule at install and update ("Half Sword MP server": inbound
  UDP for the installed `hsmp-server.exe`, all network types) through one Windows prompt, and
  removes it on uninstall. A host whose server was listed but unreachable (the first-run firewall
  prompt dismissed, or a "Public" network) can now be joined. Declining the prompt shows a warning
  and a **Fix firewall** button; `hsmp-launcher firewall-status` prints the rule's state.
- The host lobby says which UDP port players outside your network need forwarded.
- Without an `hsmp.cfg` (a hand-unzipped install) the mods use the public server list instead of
  `http://127.0.0.1:7778`. Dev and test deploys still write the local master into `hsmp.cfg`.
- A listen host adapts what it sends to each player to its own upload. When the connection backs
  up, the host sends less to that player and grows it back once the path is clear, so a home
  upload no longer builds up lag for everyone in an 8-player game.
- A server whose PC clock is wrong stays on the public server list. It used to be refused by the
  list forever; it now takes the time from the list's answer and registers again.
- Less log spam when a player quits: the stream of "connection reset" errors Windows reports
  until the player times out is logged at debug level now.

### Joining

- IPv6 server addresses work. The sidecar connects on the server's address family (IPv4 first
  when a name has both), and the in-game address box and browser accept `[addr]:port`.

### Fixes for developers

- CI: the clippy job builds again (a deny-level lint in an IPC test), and the Linux sidecar
  process checks read real process start times from `/proc`.

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

[Unreleased]: https://github.com/Cyrex0/HalfSword-MP/compare/v0.1.0-beta.3...HEAD
[0.1.0-beta.3]: https://github.com/Cyrex0/HalfSword-MP/compare/v0.1.0-beta.2...v0.1.0-beta.3
[0.1.0-beta.2]: https://github.com/Cyrex0/HalfSword-MP/compare/v0.1.0-beta.1...v0.1.0-beta.2
[0.1.0-beta.1]: https://github.com/Cyrex0/HalfSword-MP/releases/tag/v0.1.0-beta.1
