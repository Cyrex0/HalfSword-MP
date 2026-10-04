# beta5 review: security (internet-facing surfaces)

Branch `cloud/security`. Scope: server pre-auth / handshake / cookie / amplification
(`crates/hsmp-net`, `server/src/net`, `server/src/query.rs`), record validation, RCON and admin
commands, the NAT punch relay (`crates/hsmp-nat`, `server/src/nat`), the masters
(`server/src/master.rs`, `crates/hsmp-master-core`, `master-cf/`) and launcher update
verification (`launcher/`).

Overall: the transport and the launcher's signed-update path were already in good shape
(stateless cookie, reply <= request everywhere, fail-closed replay memory, strict Ed25519,
size-capped reads, signature before parse, journaled install). The fixes below are what the
review and the new mutation tests turned up.

## Findings

| # | Severity | Location | Description | Status |
|---|---|---|---|---|
| 1 | Medium | `server/src/server/session.rs` `apply_command` KICK/BAN | Any in-game admin, including one the listen host granted with PROMOTE, could kick or **IP-ban the listen host (OWNER)** from its own server. The ban is persisted, so the host's own address was locked out. | Fixed |
| 2 | Medium | `crates/hsmp-nat/src/upnp.rs` `discover` / `soap` | UPnP discovery followed the LOCATION of **any** SSDP answer (no check that it came from the device it names), the description could send SOAP calls to any host (absolute `controlURL` / `URLBase`), and description and SOAP bodies were read without a size limit into a quadratic parser. Anything that can answer the discovery socket (LAN device, or a spoofed datagram where the socket is reachable) could make `hsmp-server` fetch / POST arbitrary internal URLs (SSRF, e.g. cloud metadata) and exhaust memory and CPU. | Fixed |
| 3 | Low/Medium | `server/src/query.rs`, `server/src/server_info.rs` | The browser-query reply budget was server-wide only (200/s): one host could use it up and blank every other player's server-browser ping/info for that server. | Fixed (per-source budget) |
| 4 | Low | `server/src/master.rs` `sweep_once` | `listen_ts` (punch listen replay guard) was never pruned: every listing that ever opened a listen socket left an entry forever (slow unbounded growth of the self-hosted master). Punch rate buckets were never swept either. | Fixed |
| 5 | Low | `master-cf/src/lib.rs` / `hsmp-master-core` `Registry::listen` | The listen-request replay guard (`listen_ts`) lives in Durable Object memory; after a hibernation wake a captured signed listen request could be replayed inside its 5-minute skew window. Needs a TLS-captured request; the effect is only re-opening the host's own listen socket. | Documented |
| 6 | Low | `server/src/nat/mod.rs` `on_stun` | STUN answers are matched by the 96-bit random transaction id only, not by the server they were sent to. Unguessable in practice. | Documented |
| 7 | Low | `server/src/rcon.rs` `constant_time_eq`, `hsmp-master-core` `reports::same_secret` | Hand-rolled constant-time compares (fold of XOR); fine with today's codegen but not guaranteed against the optimiser. Length is not hidden (acceptable for a password/token). | Documented |
| 8 | Low | `server/src/master.rs` `register` | The self-hosted master's registration is unsigned: another client behind the **same public IP** can replace a listing at the same port (the Worker's signed registry does not have this). Known limitation ("identifies registrants by source IP"). | Documented |
| 9 | Low | `launcher/src/install.rs` journal | Journal paths (`state.json` in `%LOCALAPPDATA%\HSMP`) are not re-validated with `is_safe_rel` on uninstall. Only a local user who can already write that folder can influence them. | Documented |
| 10 | Info | `hsmp-master-core::reports::check` (Worker uploads) | Checked under mutation incl. extreme size/offset fields: no panic, also on 32-bit (wasm) offsets because of the `checked_add` guard. | No issue |

No Critical or High findings. Nothing found in: Hello/Auth/cookie/replay/AuthReject paths
(mutation-tested below), stateless reset and path probes (bounded, size-checked), record views
(`hsmp_ipc::record::view` exact-size checks; existing `server/tests/decode_fuzz.rs`,
`record_fuzz.rs`), RCON pre-auth (bounded lines, auth deadline, per-IP lockout, session caps),
punch request checks (endpoint IP must be the requester, port >= 1024, three rate limits, host-side
probe budget), master signatures (strict verify, ts replay guard), Worker body limits, report
admin token, launcher signature/size/SHA-256 ordering, downgrade refusal and asset-name safety.

## Changes and evidence

Every fix has a test that fails without it (the run "without" is the fix reverted on the same
tree).

1. **Admin cannot kick/ban the owner** (`a1c10eb`). A KICK or BAN from a player is refused
   (`NOT_ADMIN`) when the target's role is higher than the sender's. Admins can still kick
   players and each other, the owner can kick admins, RCON is unchanged.
   Test: `server::session::tests::an_admin_cannot_kick_or_ban_the_owner` (fails at the first
   assertion without the fix: the kick of the host is accepted and queued).
2. **UPnP hardening** (`f9114a4`). `upnp::device_location`: an SSDP answer is followed only when
   its LOCATION host is the address that sent it. `find_wan_service` ignores control URLs on
   another host than the description's. `read_body` caps description and SOAP answers at
   `MAX_BODY` = 64 KiB (Content-Length checked first, then streamed). Tests:
   `upnp::tests::discovery_ignores_a_location_on_another_host` and
   `upnp::tests::oversized_description_is_refused` both FAIL with the old `discover`
   (verified by reverting the two lines), `upnp::tests::device_location_must_be_the_answering_device`,
   and the fuzz test `upnp_description_parse_is_bounded_on_hostile_input` (64 KiB hostile
   description parses in well under a second). Existing mock-router tests (`portmap::tests::*`)
   still pass.
3. **Per-source query budget** (`7f4426c`). `query::QueryLimiter`: 20/s, burst 40 per source
   (IPv4, IPv6 /64), then the existing 200/s / 400 global bucket; at most 4096 sources tracked
   (pruned at most once a second; a table full of active sources falls back to the global
   budget). Test: `query::tests::one_source_cannot_starve_the_others` (one source gets <= 40
   replies in an instant and another source is still answered; with the old global-only
   limiter the flooder gets all 400 and the second source none).
4. **Master sweep** (`78e91d4`).
   Test: `tests::sweep_forgets_listen_timestamps_of_gone_listings` in `hsmp-master` (fails
   without: 100 stale entries kept).

### Seeded mutation ("fuzz") tests, committed at CI size

All use a small xorshift mutator over real seeds (bit flips, boundary values in length fields,
truncation, insertion, splicing, structure tokens); `HSMP_FUZZ_ITERS` raises the count.

| Test file | Decoders | Extra invariants | CI iters / time (debug) | Local soak |
|---|---|---|---|---|
| `crates/hsmp-net/tests/fuzz_wire.rs` | server Hello/Auth/data, established-connection datagrams (incl. from a new address), client Challenge/PreReject/AuthReject/data/reset, raw handshake parsers | reply <= request; no connection or pre-auth state; **only the genuine Auth reaches admission**; no mutated packet delivers messages; no mutated reply connects a client | 3k/2k/3k/10k, ~24 s | 100 000 per test, release, 49 s, clean |
| `crates/hsmp-nat/tests/fuzz.rs` | STUN, probe, NAT-PMP, PCP, SSDP, IGD description, SOAP fault | STUN only parses well-framed messages; a followed LOCATION / control URL stays on the device | 20k/20k/8k, ~5 s | 1 000 000, release, 9 s, clean |
| `crates/hsmp-master-core/tests/fuzz.rs` | signed register/heartbeat/delete bodies, signatures, listen path, punch request, report zip | **no mutated signed write is accepted**; punches only target the requester; dashboard escapes listing text | 4k/10k/20k, ~4 s | 300 000, release, clean |
| `launcher/tests/fuzz_manifest.rs` | manifest JSON, `manifest.sig`, `.sha256`, GitHub release JSON, a signed release zip | validating manifests only install safe paths under Win64; asset names cannot leave the downloads dir; **a mutated zip never opens with another manifest or loads changed bytes** | 5k/8k/600, ~2 s | 30 000, debug, 68 s, clean |
| `server/src/query.rs` `query_codec_survives_mutation` | browser query request/reply | padded requests only; any reply fits in the request | 20k, <1 s | — |

The existing `hsmp_net::fuzz` smoke test, `fuzz_soak` (ignored) and the server record fuzzers
were left as they are.

The fuzz runs found no panics in product code (the only failures during development were in
the new tests' own mutators and over-strict assertions).

## Test results

- `cargo test --workspace --locked -j 2 --no-fail-fast` (Linux, debuginfo off for disk space): every
  suite passes except exactly the known Linux-only baseline failures (7 in `hsmp-launcher` lib:
  5 `firewall::tests::*`, 2 case-rename install tests; 4 `hsmp-native` Windows shared-memory tests;
  `hsmpworld-tests` `claims_and_held_items_are_typed_records`). 69 suites ok.
- `cargo clippy --workspace --all-targets --locked -j 2`: exit 0, 179 warning lines (same as the
  baseline); none in the new or changed code.
- Package runs: `hsmp-nat` 29 + 4 fuzz, `hsmp-master-core` fuzz 3, `hsmp-net` fuzz_wire 4,
  `hsmp-server` session 50 / query 12, `hsmp-master` 29, launcher fuzz_manifest 3: all pass.

## Needs Windows / in-game verification

- UPnP against real routers (Fritz!Box, miniupnpd, ISP boxes): the new rules require the SSDP
  answer's LOCATION host to equal the answering IP and the control URL to stay on that host. Every
  router seen in the tests and fixtures does this, but a router that announces a hostname, or
  serves the description from another address, would now fall back to PCP / NAT-PMP / manual
  forwarding. Worth a check on the hosts used for beta testing (`hsmp-server` log line
  "router port opened automatically").
- The query budget: a LAN party of more than ~10 players refreshing the browser at once behind
  one address could see a few servers miss a ping for one refresh.

## Risks

- Finding 1 changes admin behaviour: an admin can no longer remove the host. That is the intent;
  nothing else about admin roles changed.
- No wire format or IPC layout change; protocol v6 interoperability is unaffected.
- `master-cf/` was reviewed but not changed (no test harness here for the Worker crate).
