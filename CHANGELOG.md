# Changelog

All notable changes to HalfSword-MP are documented here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and versions follow
[Semantic Versioning](https://semver.org/). The product version is the `version` in
`tools/release/release.json`; releases are tagged `v<version>`. The network protocol and the
game-to-sidecar IPC have their own versions (currently protocol 12, IPC ABI 2).

## [Unreleased]

### In development

- Reuse verified exact native object lookups within a single scene operation,
  retaining complete original object and callback checks. Actual timing pending.
- Preserve confirmed removal of native map managers after their original positive
  references expire, without reading or deleting replacement objects. Add bounded
  scene capture timings to locate the current slow native update stage.
- Publish replayed camera-arm endpoints to their owned attached components using
  the game's native child update, retaining full attachment and scene checks.
  Both actual clients now pass complete model readback; LIVE publication cadence
  and an owned player view remain under development.
- Preserve the original gear mesh when the engine naturally assigns its first
  reference serial, retaining complete asset identity and later scene checks.
  Actual complete model and gameplay verification remains pending.
- Validate native cloth suspension separately from the game's disable-cloth flag,
  and report exact pose preparation failures.
- Replace the unsupported skeletal-to-poseable leader link with complete native
  pose-buffer publication; actual model and gameplay verification remains pending.
- Skip discarded post-getter object wrappers while preserving the complete native
  identity and final closure checks used during scene preparation.
- Make native loading visibly active with real elapsed time and immediate asset
  counts; distinguish asset preparation from player-model creation.
- Add bounded native model-creation checkpoints to identify engine-call failures
  without per-frame logging or changes to validation and network data. Identify
  the exact render or pose-calculator mesh assignment when asset identity differs,
  and the component/value differences when transform readback refuses.
- Include UE4SS minidumps in native crash detection and reject clients that
  stop before readiness, retaining their exact failure reason and owned cleanup.
- Keep experimental native clients behind an opaque loading screen with actual
  preparation counts and indeterminate progress for unknown server work.
  Prepare exact assets across bounded ticks and retain visible errors until
  the test client is stopped. Owned-view readiness remains under verification.
- Batch up to four existing scene records per peer per transport flush, keeping
  original frame cursors, backpressure and complete-frame acknowledgments.
  This increases delivery capacity without changing precision or record size;
  real multi-player gameplay throughput remains under verification.
- Remove repeated source admissions inside callback-free native identity and
  pointer reads. Keep fresh object checks and full admission around engine
  callbacks and before returning captured data.
- Construct fresh native component wrappers from already qualified source
  identities, avoiding repeated global path searches during full model capture.
  Preserve both complete harvests, all fields and identity checks; actual
  startup performance and visible client models remain under verification.
- Separate experimental source sampling retries from capture timestamps and
  represent an unqualified physics timestep as unknown. Preserve original
  frame freshness while allowing sampling to resume after slow captures.
- Preserve original native-client stdout/stderr and backtraces during tests,
  draining both streams asynchronously so failures can be diagnosed without
  blocking the client or changing its readiness limits.
- Record bounded client startup phases around asset preparation and native
  presentation, keeping diagnostic events separate from readiness and input.
- Reuse already admitted attachment-parent identities during experimental
  model capture, avoiding duplicate setup while retaining fresh native
  identity checks and both complete harvests. Actual capture speed and model
  presentation remain under verification.
- Publish native team facts for the complete original roster before capturing
  player models. Wait for the exact roster acknowledgment and verify teams
  during descriptor registration, so admitting one model cannot invalidate
  another player's capture. Actual two-client presentation remains unverified.
- Capture native dynamic body/armour material instances independently of their
  transient flag, preserving parent overrides and guarding original parameter
  arrays across callbacks. Actual body meshes remain under native verification.
- Identify the exact copied component/material field when experimental native
  scene registration fails, retaining native values and existing validation.
  Actual player meshes and owned client views remain under verification.
- Preserve independent native construction/live armour-map keys and passport slots,
  retaining empty rows and all gear fields. Extend the larger node budget to
  Lua capture signatures and make long native captures cooperatively stop.
  Actual registration and playable verification remain pending.
- Adapt experimental native scene delivery to compact lossless recipes and
  bounded complete-frame parts. Keep generation-valid startup scenes during
  native preparation and require fresh applied scenes for live input. This
  addresses actual full-gear recipe overflow; runtime verification is pending.
- Preserve native empty skeletal weapon holders and their complete observed
  material slots in the experimental headless scene. Require fresh asset,
  render-cache and override absence checks before mirror readiness. Actual
  scene and gameplay verification remain pending.
- Add an experimental native authority host: embed the network service inside
  HSMPNative, isolate its worker role from client policies, and supervise the
  licensed headless game through `hsmp-server native`. Native gameplay and
  complete co-op presentation remain under live verification.
- Test that host with two normal AI-input game clients and one headless
  authority; qualify native scene attachment anchors and client actor
  retirement against the installed game's world and garbage-state evidence.
  Scene replication and live input remain unverified.
- Carry complete native spline curves, interpolation settings and visibility
  through a negotiated experimental scene format; require source profiles and
  native readbacks before accepting a mirrored scene. Live verification remains
  pending.
- Verify admitted native source paths through their complete original object
  hierarchy, retaining exact initial lookups and lifetime/world checks while
  avoiding repeated full object searches. The actual headless test reduced one
  spline profile from24.406s to0.059s; complete scene capture and playable
  mirroring remain unverified.
- Recheck the original static mesh asset around native LOD reads and report the
  exact getter result, asset class and path when scene capture refuses it.
  Real paired tests prove LOD0 under no-render mode and LOD1 offscreen for one
  engine mesh; complete vertex capture and a client travel exit remain open.
- Record the original owned client's process exit code and verification phase
  when it leaves a native test; preserve unknown codes and avoid PID reuse.
- Require a separate native vertex-state capability before admitting experimental
  presentation peers. Native override-absence implementation and actual scene
  readiness remain under verification.
- Preserve cooked static asset colors only after a complete guarded native
  override-array census. Recheck original source and mirror state before whole
  scene publication/readiness; present paint overrides retain exact capture,
  and unknown state remains an explicit refusal. The actual Sphere.Sphere
  absence proof passes; complete scene verification remains pending.
- Extend the experimental source scene with exact native Camera and SpringArm
  ancestors. Negotiate a new scene revision for cached spring-arm socket output;
  require complete current source and inert mirror readbacks. Complete scene,
  local camera/HUD ownership and gameplay remain under native verification.
- Retain empty inherited native StaticMesh holders as exact inert components
  with guarded original null and vertex-override checks. Preserve their observed
  collision/material/attachment state and invalidate recipes when modules
  change. Complete native scene and gameplay verification remains pending.

## [0.1.0-beta.6] - 2026-10-08

### Known limitations

- This is a release of the existing PvP mod. Native headless hosting and co-op are
  still development work and are not included in this release.
- Round-start placement and settling can still fail or delay input release. Remote
  arms and wrists can twist, drift or overshoot, especially under high frame cost.
- Native body construction, dismemberment and clothing/armour replication do not
  yet have complete single-player parity. Partial cuts and detached components
  remain unfinished; diagnostic observations are not proof of equivalent damage.
- Prior live captures reported pose and smoothness gate failures. Final beta.6
  release validation must be recorded on the release commit; no G2 pass is claimed
  by these notes.

### Launcher fixes

- Launcher windows use OpenGL and automatically retry Direct3D 12 on Windows if OpenGL
  initialization fails. `HSMP_LAUNCHER_RENDERER=glow` or `dx12` explicitly selects a renderer. Startup records the
  renderer and adapter in `launcher.log`, and graphics initialization errors show a message
  with recovery instructions instead of silently closing.

### What's new

- Finish pending readiness commands from server replies during the existing bounded
  quit teardown, without sending new commands or retrying while the session closes.
- Preserve complete session and mode state during developer wrist captures while
  allowing sequence and server-clock heartbeats, retaining strict gameplay guards;
  bound loading-phase retries and record the transported grip without changing it.
- Keep initial spawn position anchored through Loading and Countdown while native
  limb motion stays active; require fresh settling after release before player input.
- Make developer AI takeover wait for the controller's actual input release.
- Carry native absolute foot targets and step splines with residual spawn placement
  corrections, refusing stale body or component bindings.
- Preserve the displayed pose's actual discontinuity context so developer wrist
  fault captures can admit a real persistent stall.
- Avoid repeated grip motor writes when the same freshly verified binding already
  has all six linear drive flags disabled, preserving restoration checks.
- Capture bounded developer wrist settings and current grip flags during an actual
  settling fault, and distinguish native spawn rotation targets from physical body data.
- Disable native linear grip motors on a driven proxy using six verified enable
  flags, preserving native strengths and restoring exact flags to the same binding.
- Add focused developer observations for the left shoulder and three protected
  spawn corrections, retaining native body and capsule positions separately.
- Preserve a stuck blade's original pelvis contact separately from its initial
  spine anchor, allowing the same validated constraint to return to that contact.
- Retain stuck-blade continuations while their known parent awaits pose coverage,
  preserving source delivery order without requiring a child resend.
- Reuse developer joint-capture session readers while preserving fresh native
  header, link, peer and sample checks at every guard.
- Add a bounded developer journal for native stuck-blade setup, marker wear and
  sever callbacks, with explicit missing history and no sever authority.
- Retain the existing owner damage call's native material-response outputs with
  explicit incomplete results after copy-out failures and world changes.
- Correct scalar output containers for the pinned game bridge, including current
  joint settings, without adding native calls or changing guard limits.
- Stage seven internal authored weapon recipes with exact native passports across
  equip, verification and recovery; public selection awaits recipe-specific validation.
- Restore source order within a single drained batch of approved hits when the
  full life context and source timestamps agree, preserving duplicate protection.
- Identify the first failed developer joint-capture guard from existing sampled
  values, without adding native reads or changing capture limits.
- Stop creating new hit claims after a fighter's confirmed elimination while
  preserving claims already queued for trade resolution.
- Preserve precise bounded developer capture refusals, and finish capture cleanup
  before optional stop logging, including when logging fails or reenters.
- Add a bounded developer capture of current right-arm joint limits, drive strengths,
  softness and projection on each local owner and remote proxy, with explicit identities.
- Record native caller admission refusals from one nonblocking attempt, preserving
  the existing guard and saving bounded callback process totals in developer logs.
- Distinguish confirmed foreign-thread damage callbacks from unavailable admission
  on the proven game thread, retaining fail-closed handling and explicit coverage gaps.
- Prepare native damage observations with sampling disabled, then activate after
  fresh playback and current character, weapon and hit-box bindings are verified.
- Measure bounded native damage-observer enrollment stages to diagnose setup stalls.
- Read native limb target outputs in their actual struct format, and measure
  damage-observer setup stages without changing playback freshness checks.
- Add bounded developer captures for limb constraints and current animation-drive
  components, and precise playback refusal details for native damage observations.
- Preserve a truly empty remote left hand instead of creating a fist weapon,
  retaining native polearm offhand setup. Recovery separates refused calls from
  attempted native operations to avoid repeating uncertain mass changes.
- Read the complete bounded native function property chain when validating damage
  observer inputs, including Blueprint locals. Pending hand diagnostics record
  actual Mode mismatches without treating them as authoritative gameplay state.
- Add bounded developer captures for current hand grip motor settings and native
  damage-sampling hit-box changes, with explicit identity and availability checks.
  Observer refusals identify their failed stage; unavailable grip axes remain
  explicitly unavailable, and fixtures isolate probe settings from the environment.
  Read-only hand diagnostics compare actual decoded, final, prior and current
  driver poses, plus native grip flags and current hand constraint identities.
- Run stand-in arm neutralisation in the actual Blueprint ReceiveTick callback slot,
  validating the current pawn, life, world and mesh before applying the existing policy.
- Resolve stuck-blade memberships once with bounded reads and explicit parent ambiguity,
  retaining the proven same-constraint left-arm rebind and validation metadata on refusal.
- Publish verified deathmatch respawn preparation frames before the loaded acknowledgement,
  while keeping death reporting restricted to the active life.
- Refuse developer deployments with incompatible compiled content before changing game files,
  and resolve custom binary paths consistently with the game configuration.
- Start playback clocks from the current match, round, life and pose discontinuity
  after placement returns; preserve the existing timing detector within that generation.
- Keep the fixed Rondel dagger's native weapon passport instead of replacing it
  with a sword recipe from merchant stock; incomplete native defaults still refuse equip.

- Harvest the local game's complete indexed gear dependency set offline, with
  inherited class defaults, native enum mappings and explicit coverage gaps.
- Preserve native armor passports for items outside tier templates and verify
  complete equipped armor/weapon passports, including modules and materials.
- Add short read-only armor trace bursts with bounded native reads and explicit
  availability, independent of the stand-in damage probe.
- Build the lab controller before use and keep each running session on its own
  executable copy, avoiding stale IPC layouts and Windows rebuild locks.
- Record native parent-bone-space arm/driver displacement and dislocation inputs
  with strict identity and availability checks; sampling grants no damage authority.
- Recover initial dropped kit weapons by re-equipping the same native actor, retaining the
  outfit and assigned fighter through the stand-in fallback's brief possession swap.
- Reassert stand-in grip limits after native hand updates and check physical pelvis
  placement on the first teleport, retaining the existing spawn readiness limits.
- Refresh a validated weapon root's mutable physics state so native setup enabling
  simulation resumes pose driving instead of retaining an obsolete kinematic cache.
- Preserve native joint-dislocation protection during non-Live placement and restore
  its original value with fresh ownership and readback checks.
- Keep confirmed missing-limb servo and physics exclusions through temporary playback
  release; add separate read-only native body and sever-component diagnostics.
- Correct native sever-hook lookup through UE4SS's function-prefix parser and add
  explicit topology availability plus optional native damage-caller diagnostics.
- Read completed native distal cuts from the typed ledger with fresh body-life
  and physical hiding checks, instead of the legacy unwritten array. Partial
  cut geometry and detached components remain separate replication work.
- Check diagnostic sample budgets before native function lookup to bound work
  in hot weapon callbacks while retaining exact native identity validation.
- Add `hsmp-lab` recipe sessions, incremental evidence collectors, bootstrap comparisons,
  explicit baseline promotion, a regression journal and in-session A/B runs.
- Add isolated modes/server-mods backend smoke and repeatable native deathmatch placement
  checks; correct compatibility expectations and the test fixture's per-file limits.
- Fix repeated unchanged kit saves leaving the game acknowledgement stale. The lab waits
  for the exact accepted server kit before starting an experiment.
- Publish the stand-in's physical sample time at target construction and reconstruct body,
  weapon and cutting geometry from delivered physical frames, removing sender-step inversion.
  Protocol 12 rejects earlier clients because timestamp and continuation meanings changed.
- Bind stuck-blade continuations to their exact native origin/module, including suppressed
  origins; reject deferred children if their parent is later parried. Dev AI Duel yield
  counts as scoped surrender; harness timeouts are recorded separately.
- Enable lab AI only after the arena is live, with native takeover evidence recorded
  separately from command submission. Pause verified AI attack intent after a round ends.
- Require fresh current-life streams, native collision and settled arm/hand physics before
  the initial input unlock; keep ordinary wounded fighters controllable after release.
- Place two-game tests on the smallest secondary display, with no window activation.
- Record freshly read unchanged Health as a measured zero and correct native POST armour
  trace argument ordering; cached or unread values remain unavailable.
- Keep actual pose receipt time separate from physical frame time so late callbacks do not
  introduce artificial playback clock jumps.

- Game modes, picked by the host on the lobby's new **GAME MODE** screen (or with `--mode` and
  RCON `MODE`):
  - **Team elimination** (2 to 4 teams): teams balanced automatically or picked by the players in
    the lobby; teammates spawn together and cannot hurt each other unless friendly fire is on.
  - **King of the hill**: hold the hill alone to score; the first to the target wins the round.
    The HUD shows who holds it and how far and where it is.
  - **Weapon roulette**: everybody gets the same random weapon and armour each round.
  - **Brawl**: fists only, no armour.
  - **Timed deathmatch**: respawn after a death, most kills when the clock runs out wins; a tie
    goes to sudden death.
  - King of the hill, roulette, brawl and deathmatch can be played in teams too, and every mode
    can have a round clock.
- The scoreboard (TAB) shows kills and deaths, and team tags in team modes; the top banner shows
  team scores, points or kills, and the round clock.
- The server browser lists a server's game mode.
- Server admins: new flags `--teams`, `--team-rule`, `--round-time`, `--koth-target`,
  `--friendly-fire`, `--respawn-delay` (and `HSMP_KOTH_ZONES`), and RCON `MODE`, `TEAMS`, `TEAM`,
  `ROUNDTIME`, `OPTION`; `STATUS` reports teams, kills and deaths.
- **Server mods.** A server can now serve its own Lua mods (`hsmp-server --mods-dir`). When you
  join one, a warning lists every mod (name, version, author, size) and says plainly that they run
  with full access to your PC; nothing downloads unless you click ACCEPT & JOIN. DECLINE takes you
  back to the server browser. The mods download over the game connection, every file is checked
  against the server's hash, and they start without restarting the game.
- The server browser marks servers with mods (**[MODS n]**).
- SETTINGS > SERVER MODS: ASK ME or NEVER, and FORGET REMEMBERED SERVERS. "Remember for this
  server" asks again whenever the server's mods change.
- Hosting: [server mods](docs/hosting/server-mods.md): folder layout, `mod.json`, the rules, the
  download rate limits that keep players in a match unaffected.

### Compatibility

- Protocol 12 requires matching beta.6 clients and servers. Earlier releases,
  including beta.5, cannot join; update the host and every player together.

### Changed

- New release mod HSMPModHost (runs accepted server mods). Downloaded mods are kept in
  `Win64\hsmp_mods`, outside the UE4SS mods folder; the launcher's uninstall deletes it.
- Protocol: capability bit 18 `SERVER_MODS`, records `0x0901`-`0x090A`, reject code 11
  `MODS_REQUIRED` for clients without server-mods support. The server list and the browser ping
  carry the mod count and size (older readers ignore them).

## [0.1.0-beta.5] - 2026-10-04

### What's new

- Damage matches single player in two more cases: two blows the game counts as one no longer
  both land when their replays arrive late, and a stand-in hit natively no longer turns the rest
  of the same contact into extra hits.
- Stand-ins carry their owner's body weight and build, so weapons meet the same resistance as
  on the owner's screen.
- Fairer hits when a player's ping changes mid-match: lag compensation follows the new ping
  within seconds instead of up to 16 seconds later.
- The server runs at 60 Hz by default (`--tick-hz` 20 to 240, `HSMP_TICK_HZ`), and every match
  timer runs in real time.
- Lower frame cost in game with remote players on screen, and less network traffic from the
  server.
- Security: an admin can no longer kick or ban the listen host; UPnP only talks to the router
  that answered; one host can no longer use up a server's browser-ping replies.

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

### Server

- The server tick no longer allocates in steady play and costs about a fifth of what it did at
  16 players.
- Relayed records a player sent together (root and pose of one frame) reach the other players
  in one datagram: about a fifth fewer datagrams and 5 to 7 % less download.
- `--max-peers` accepts 1 to 64 (the session roster's size).
- The public server list writes a listing at most every other heartbeat when nothing visible
  changed (half the storage writes on the free Cloudflare plan).

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
