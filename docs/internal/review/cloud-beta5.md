# beta5 review-and-optimise pass

Six areas, each worked on its own branch and merged into `dev`. Each area has its own note with
the evidence, the exact commands and the numbers; this page is the summary. `dev` also carries
the post-beta.4 work from the development repo (60 Hz default tick, real-time match timers,
SMOOTH-1, `docs/development/tick-rate.md`).

| Area | Note |
|---|---|
| Netcode | [cloud-beta5-netcode.md](cloud-beta5-netcode.md) |
| Server and master | [cloud-beta5-server.md](cloud-beta5-server.md) |
| Game-side frame cost | [cloud-beta5-framecost.md](cloud-beta5-framecost.md) |
| Combat | [cloud-beta5-combat.md](cloud-beta5-combat.md) |
| Security | [cloud-beta5-security.md](cloud-beta5-security.md) |
| Code quality and CI | [cloud-beta5-quality.md](cloud-beta5-quality.md) |

Every change has a test that fails without it. Wire compatibility: protocol stays v6 and beta.4
peers interoperate; the one new record (`body`) is behind `caps::BODY`. The shared-memory layout
hash changed (new `body` slot), so the game-side files and the sidecar must ship together, as
they always do.

## Changes and numbers

### Netcode
- In-place seal/open, reused buffers, cheaper ack bookkeeping (`hsmp-net/tests/alloc_hotpath.rs`,
  release): channel-0 stream 9.75 → 3.75 allocations per round, reliable 10.5 → 5.3, idle poll
  1 → 0; callgrind 1152.5M → 1014.7M instructions (−12 %).
- Lag comp: the attacker RTT is the minimum over the last 4 s instead of the last 16 samples.
  After a 40 → 140 ms step the old code predicted 100 ms off for 16 s (tolerance ±30 ms).
  Combat sim, 3 seeds: wifi 99.91 → 100 %, bad 97.02 → 97.41 % honest hits; cheat false-accepts
  unchanged.
- Relay: FxHash for the per-frame maps, 3980 → 980 ns per frame at 16 players, 18677 → 5044 ns at
  64; the per-datagram budget charge includes the ACK_DELAY chunk (~1 % overspend fixed).
- `next_timeout` wakes when the pacer frees held reliable data (API fix; the server and sidecar
  don't call it today).

### Server and master
- Steady-state tick allocates nothing (`steady_tick_does_not_allocate`), 16 players, release:
  allocations per tick 25 → 0 (lobby) and 35 → 0 (live); tick time 42.9 → 3.4 µs (lobby),
  26.2 → 5.8 µs (live).
- Relay bundling: a sender's root and pose that arrived in one datagram leave in one datagram per
  receiver, no added delay, no wire change. `hsmp-loadtest --pump`: datagrams out 3026 → 2388/s
  (8 bots), 8657 → 6654/s (16 bots); about −6 to −7 % bytes on the wire.
- Master Worker: unchanged heartbeats skip the storage write, 720 → 360 writes per server per
  day; free-tier budget math in the server note (the binding limit is requests, ~100 servers).
- `--max-peers` accepts 1–64. The server handles 16 (5.6 µs tick, ~16 % of a core); the game side
  does not yet (stand-ins untested past 7, the Avatars id probe stops at 8, combat pairing
  assumes ≤ 8), see `docs/hosting/configuration.md`, "More than 8 players".
- Hostile-input audit plus a 5000-round master-core test: no panic found.

### Game-side frame cost
- Per-frame Lua garbage (new `hsmp-tools lua-test framecost`, each budget fails on beta.4):
  stand-in frame (native servo, 22 bodies) 141 KB → 5.5 KB; pose sender (native sampling)
  3178 → 121 B; one IPC facade call 112 B → ~0; HSMPWorld `can_drive` 368 → 0 B.
  Stand-in driver Lua CPU 343 → ~135 µs per frame on the shared Linux machine.
- The pose sender makes one native `sample_local` call per sample instead of three.
- Splits (code moved unchanged, proven by `pure_equal` against verbatim beta.4 copies):
  `avatars_pure.lua`, `world_pure.lua`, `menu_umg.lua`. HSMPAvatars 4356 → 3961 lines.

### Combat
- Two MP-vs-solo divergences fixed in HSMPCombat (damage_parity 26/26, 4 new checks): blows the
  game gates in solo landed twice when their replays arrived > 0.2 s apart; a stand-in
  Invulnerable put-back reset the contact gate and turned weaker frames into extra claims.
- Owner's body on stand-ins: new `body` record (caps::BODY), relayed and replayed to capable
  peers; stand-ins get the owner's Muscle Rate, Mass Scale and per-bone masses. Height is
  carried but not applied (HSMPAvatars would need to re-measure bone offsets).
- `hit_vel_factor`: calibration tooling only (`hsmp-combat-sim --hvf`, `--hvf-logs`), procedure
  in combat.md §7. The shipped ceiling cuts 3–10 % of honest blade blows in the sim; a looser
  ceiling let 3–14 of 187 DamageInflate cheats through, so the factor is unchanged until an
  in-game capture exists.

### Security
- No Critical or High findings.
- Fixed (Medium): an in-game admin could kick or ban the listen host (and the ban persisted).
- Fixed (Medium): UPnP followed any SSDP LOCATION and control URL (SSRF) and read unbounded
  bodies; now same-device only, 64 KiB cap.
- Fixed (Low/Medium): per-source budget for browser query replies (20/s, burst 40). Fixed (Low):
  the self-hosted master pruned `listen_ts` never.
- Seeded mutation tests for every pre-auth decoder, NAT parser, master signed write / punch /
  report, and the launcher manifest, signature and signed zip (`HSMP_FUZZ_ITERS` raises counts;
  local soaks up to 1M iterations found no panics). Five Low items are documented, not fixed.

### Quality and CI
- Clippy 179 → 0 warnings on the merged tree; four lints left the workspace allow list.
- DoD-10: the lobby menu waits up to 2 s for the server's cmd_result before showing a result
  inferred from the session state.
- All Lua suites pass on Linux (21 447 checks; 9462 with 18 failures at the start).
- CI runs G0, e2e and clippy as parallel Windows jobs (estimated ~10 min per push, was 16–17).
- The gate's contract scanner now stops at the test module, not at the first `#[cfg(test)]`
  (a test-only helper in `session.rs` hid three real emitters).

## Test results (Linux, merged `dev`)

- `cargo test --workspace --locked --no-fail-fast`: 1172 passed. The only failures are the 12
  Windows-only tests that also fail on beta.4 under Linux: 7 in `hsmp-launcher` (firewall,
  case-insensitive install), 5 in `hsmp-native` (Windows shared memory).
- `cargo clippy --workspace --all-targets --locked`: exit 0, 0 warnings.
- Not run here: `scripts/e2e-test.sh` (Windows/MSYS), the live gate (`mp_test.ps1`), G0 with the
  game dump.

## Needs Windows or in-game verification

- G0 with the game dump, e2e, and a live two-instance gate (POSE-1, SMOOTH-1, SPAWN-1, DoD-10
  under `-Netsim far` and `bad`).
- Frame cost: the stand-in "frame cost" log line drops; native sampling still reports
  "sampler native" with the combined call; the three new Lua files deploy and package.
- Combat: the two damage fixes in real fights; stand-in body values on a live stand-in
  (combat-parity.md §4); a DCD capture through HSMPParity for `hit_vel_factor`.
- Netcode: a soak where a client's ping changes mid-fight (hit acceptance holds).
- Server: relay bundling in a real match; the Worker write saving on a real deploy.
- Security: UPnP on real routers (a router serving its description from another address now
  falls back to PCP / NAT-PMP / manual forwarding).
- CI: the first run of the new workflow confirms the time saving.

## Risks

- The `body` record changes the shared-memory layout hash: a mismatched native module and
  sidecar refuse each other (by design, but deploy both).
- Reused target tables in HSMPAvatars are safe only while nothing holds a target table past two
  drives or an aim table past one (true today, documented in the framecost note).
- The per-source query budget may drop the odd ping when more than ~10 players browse from one
  public address.
- Relay bundling changes datagram composition; any receiver-side assumption of one record per
  datagram would show up in the live gate.
- Loadtest CPU numbers were taken on a shared 4-core machine and are noisy.

## Not done

- HSMPHud and the combat claim builder were not moved native; the stand-in target math (about
  half the remaining stand-in frame cost) is still Lua.
- Knocked-off armour replication, held items on the hand bone, POSE-1 on heavy maps, a traffic
  relay for symmetric NAT: untouched in this pass.
- Height on stand-ins, and the three combat issues listed in the combat note (reverse-order
  replays, ledger armour, picked-up weapon caps).
