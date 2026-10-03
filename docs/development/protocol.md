# Wire protocol (v6)

The UDP protocol between `hsmp-sidecar` (client) and `hsmp-server`.

- **Implementation:** `crates/hsmp-net` (sans-IO Rust: it never touches a socket;
  `server/src/net/mod.rs` and `server/src/sidecar/net.rs` own the sockets). `cargo test -p
  hsmp-net` runs its unit and end-to-end tests. Section numbers in code comments refer to this
  page.
- **Messages:** every application message is a typed record from `crates/hsmp-ipc` (§6), the
  same bytes the game writes into shared memory ([ipc-shared-memory.md](ipc-shared-memory.md)).
- **Overview:** [architecture.md](architecture.md).

## Design summary

- **Encrypted from the first data packet.** Keys come from X25519 on both sides; no key
  material is ever sent. Traffic is sealed with ChaCha20-Poly1305, one key per direction, with a
  key update.
- **Counter nonces and a replay window.** The nonce is the packet number; a 1024-packet window
  rejects replays.
- **Stateless cookie.** The server keeps no state before the client has proven its address, and
  no reply to unauthenticated input is larger than the input.
- **Player identity.** Each install has an Ed25519 key; the server keys seats, wins, bans and
  admin rights by it.
- **Routing by `conn_id`**, derived from the handshake, so a NAT rebind can migrate a connection.
- **Three channels** over one connection: unreliable-latest, reliable-unordered and
  reliable-ordered, with fragmentation up to 64 KiB.
- **Versioning.** `PROTOCOL_VERSION` changes only when the packet header (§4) or handshake (§3)
  changes, or when the message representation changes. Everything else is added behind a
  capability bit (§7).

### What changed in v6

The transport (§1–§5) is the same as in v5. v6 changed what a channel message contains: in v5
it was a bincode-encoded `Body` enum; in v6 it is one typed record behind an 8-byte header (§6).
There is no dual stack: v5 and v6 do not interoperate, and a v5 client gets a `VERSION`
PreReject. The key-derivation labels still read `HSMP/5` / `hsmp5`; they are constants of the
key schedule and did not change.

## 1. Datagram namespace

All integers are little-endian. Byte 0 of every datagram is its type. The values never collide
with legacy v4 bincode datagrams (bytes 0..4 are `00 00 00 00` or `01 00 00 00`) or with the
server-browser query (`\xFF HSMPQ1 \0`, `server/src/query.rs`), so one UDP port serves all three.

| Byte 0 | Name | Direction | Encrypted | Size |
|---|---|---|---|---|
| `0xA1` | `C2SHello` | C→S | no | ≥ 1200 (padded) |
| `0xA2` | `S2CChallenge` | S→C | no (carries a MAC'd cookie) | 120 |
| `0xA3` | `C2SAuth` | C→S | payload sealed with `auth` | about 260 |
| `0xA4` | `S2CPreReject` | S→C | no | ≤ the Hello |
| `0xA5` | `S2CAuthReject` | S→C | sealed with `s2c` | ≤ the Auth |
| `0xA6` | `S2CReset` (stateless reset, §4.7) | S→C | no (carries a token) | 25 |
| `0xB0` | data packet | both | sealed with the direction key | ≤ 1200 |
| `0xFF` | browser query | both | no | — |
| `0x00`/`0x01` + `00 00 00` | v4 datagram | — | — | dropped (§7.3) |

`MAX_DATAGRAM = 1200` bytes for every packet type in both directions. It stays below every
common path MTU (the IPv6 minimum of 1280 minus headers).

## 2. Cryptography

| Primitive | Use |
|---|---|
| X25519 | Key agreement: client ephemeral × server ephemeral (`ee`), client ephemeral × server static (`es`) |
| HKDF-SHA256 | Key derivation, key update |
| ChaCha20-Poly1305 | AEAD for the Auth payload, the AuthReject and data packets (16-byte tag) |
| HMAC-SHA256 | Stateless cookie (truncated to 16 bytes), password proof, reset token |
| SHA-256 | Transcript hash, player id |
| Ed25519 | Player identity (`player_key`, one per install) |

### 2.1 Long-term keys

- **Server static key** `s_static`: an X25519 key pair the server persists
  (`$HSMP_STATE_DIR/server_identity.key`, else `%LOCALAPPDATA%\HSMP\server_identity.key`).
  - Its public half is published in the master listing and the query reply (`server_key`, hex).
  - The browser pins it (`ClientConfig.pinned_server_key`; sidecar `--server-key`). A pinned-key
    mismatch fails closed (`Failed("server key does not match the pinned key")`).
  - A direct-IP join without a known key is trust on first use: the client reports the key in
    `Connected{server_key}`; the sidecar pins it for this process's reconnects and records it
    in `<state>/known_servers.json`.
- **Player key** `player_key`: an Ed25519 key pair generated once per install and stored in
  `%LOCALAPPDATA%\HSMP\identity` (or under `HSMP_STATE_DIR` for test instances).
  - Seats, wins, bans and admin rights are keyed by its public half.
  - `player_id = SHA-256(player_key)[0..8]`, shown as 16 hex characters.
  - `hsmp-sidecar --print-player-key` prints the public key.

### 2.2 Rotating server secrets

The server ephemeral `s_eph` and the cookie key `cookie_key` are server-global and rotate every
`rotate_ms = 120 000` ms. The server keeps exactly one previous generation.

- An Auth that references an `s_eph` older than the previous generation fails with
  `UnknownEph`, and the client restarts the handshake.
- Forward secrecy: traffic keys depend on `ee`, so they cannot be recomputed once both `s_eph`
  generations have rotated out (at most 4 minutes).

### 2.3 Key schedule

```
hello_core      = §3.1 bytes (exactly as sent)
challenge_core  = §3.2 bytes
th   = SHA-256("HSMP/5 transcript\0" || u16 len(hello_core) || hello_core || challenge_core)
ee   = X25519(c_eph, s_eph)        both must be contributory (not all-zero), else LowOrder
es   = X25519(c_eph, s_static)
prk  = HKDF-Extract(salt = th, ikm = ee || es)
auth    = HKDF-Expand(prk, "hsmp5 auth",   32)   C2SAuth payload key
c2s     = HKDF-Expand(prk, "hsmp5 c2s",    32)   client -> server traffic secret, phase 0
s2c     = HKDF-Expand(prk, "hsmp5 s2c",    32)   server -> client traffic secret, phase 0
conn_id = LE u64 of HKDF-Expand(prk, "hsmp5 cid", 8); 0 is replaced by 1
resume  = HKDF-Expand(prk, "hsmp5 resume", 32)   reserved for resume tickets
next(secret) = HKDF-Expand(PRK = secret, "hsmp5 key update", 32)
```

- The AEAD key is the traffic secret itself.
- **Nonce:** `LE u64(full packet number) || 00 00 00 00`.
- **Reserved packet numbers:** `u64::MAX` (the Auth payload, key `auth`) and `u64::MAX − 1`
  (the AuthReject, key `s2c`). Data packets never reach them.
- Both sides derive `conn_id`, so the client knows it before the server answers. A collision
  with an existing connection is treated as a duplicate Auth (probability 2⁻⁶⁴).

## 3. Handshake

```
client                                          server
  C2SHello  (>= 1200 B, c_eph, versions, caps,
             content_hash)              ------>  no state; one HMAC
                                        <------  S2CChallenge (120 B: s_static, s_eph, cookie)
                                                 or S2CPreReject (version/content, <= Hello)
  C2SAuth   (hello_core again, s_eph, cookie,
             sealed{player_key, sig(th), nick,
                    role, tokens})     ------>  cookie check -> DH -> AEAD -> Ed25519
                                                 -> admission (ban, full, password)
                                        <------  data packet: welcome record on channel 2
                                                 or S2CAuthReject (sealed, <= Auth)
```

### 3.1 `C2SHello` (type `0xA1`)

| Offset | Size | Field |
|---|---|---|
| 0 | 1 | type `0xA1` |
| 1 | 2 | `version_min` |
| 3 | 2 | `version_max` |
| 5 | 8 | `caps`: the client's capability bits (§7.2) |
| 13 | 32 | `c_eph`: the client ephemeral X25519 public key, fresh per handshake |
| 45 | 32 | `content_hash` (the build's, embedded by `server/build.rs`; sidecar `--content-hash` overrides) |
| 77 | 1 | `build_len`, at most 32 |
| 78 | n | `build`: UTF-8, informational |
| 78+n | … | zero padding up to **at least 1200 bytes in total** |

`hello_core` = bytes `1 .. 78+n`. The server **drops** a Hello shorter than 1200 bytes and does
not reply: that is the anti-amplification floor.

### 3.2 `S2CChallenge` (type `0xA2`, 120 bytes)

| Offset | Size | Field |
|---|---|---|
| 0 | 1 | type `0xA2` |
| 1 | 8 | `echo` = `c_eph[0..8]` (the client ignores a Challenge without it; this defeats off-path injection) |
| 9 | 2 | `version`: the highest common version (§7.1) |
| 11 | 8 | `server_caps` |
| 19 | 32 | `s_static` |
| 51 | 32 | `s_eph` (current generation) |
| 83 | 1 | `flags`: `0x01` NEEDS_PASSWORD |
| 84 | 16 | `pwd_salt` (zero when no password) |
| 100 | 20 | `cookie` = `ts u32` ‖ `HMAC-SHA256(cookie_key, M)[0..16]` |

`challenge_core` = bytes `9..120`.

```
M  = "HSMP/5 cookie\0" || (4 || ipv4[4] | 6 || ipv6[16]) || u16 port || SHA-256(hello_core) || s_eph || ts
ts = server monotonic seconds
```

The cookie is valid when the MAC verifies under the current or the previous `cookie_key`,
`now_s − ts ≤ 30` and `ts ≤ now_s + 1`.

### 3.3 `S2CPreReject` (type `0xA4`)

Sent instead of a Challenge when there is no common version, or when `content_hash` differs and
the server enforces one (on by default: its own build's hash; `--allow-mismatched-content` turns it off). The text names both releases: "Server runs HalfSword-MP X, you have Y" plus what to do.

| Offset | Size | Field |
|---|---|---|
| 0 | 1 | type `0xA4` |
| 1 | 8 | `echo` = `c_eph[0..8]` |
| 9 | 1 | `code` (§3.7) |
| 10 | 2 | `server_min` |
| 12 | 2 | `server_max` |
| 14 | 2 | `text_len`, at most 512, truncated so the datagram is ≤ the Hello |
| 16 | n | `text`: UTF-8, actionable ("Your mod is OUTDATED: update it.") |

The PreReject is unauthenticated. An on-path attacker could forge one, but an on-path attacker
can already drop the traffic, so this adds no new denial of service. The `echo` field stops
off-path forgery. The client treats it as terminal and shows the text.

### 3.4 `C2SAuth` (type `0xA3`)

| Offset | Size | Field |
|---|---|---|
| 0 | 1 | type `0xA3` |
| 1 | 1 | `hello_len` |
| 2 | hello_len | `hello_core`: byte-identical to the Hello |
| … | 32 | `s_eph`: echoed from the Challenge |
| … | 20 | `cookie`: echoed from the Challenge |
| … | rest | `ChaCha20-Poly1305(key = auth, nonce = u64::MAX, aad = all preceding bytes, AuthPayload)` |

The server is stateless, so the Auth carries everything it needs: the hello core to recompute
the cookie and the transcript, and `s_eph` to select the ephemeral generation. The server
rebuilds `challenge_core` from its config, `s_eph` and the cookie.

**`AuthPayload` (plaintext):**

| Size | Field |
|---|---|
| 32 | `player_key` (Ed25519 public key) |
| 64 | `sig` = Ed25519(player_sk, `"HSMP/5 auth\0" ‖ th`), checked with `verify_strict` |
| 1 | `want_role`: 0 fighter, 1 spectator |
| 1 | `opt_flags`: bit 0 resume, bit 1 pwd, bit 2 party; other bits must be 0 |
| 16 | `resume_token` (if bit 0; reserved) |
| 32 | `pwd_proof` = HMAC-SHA256(password key, `"HSMP/5 pwd\0" ‖ th`) (if bit 1) |
| 16 | `party_token` (if bit 2; reserved) |
| 1 | `nick_len`, at most 32 |
| n | `nick`: UTF-8, cosmetic; the server deduplicates it |

No trailing bytes are allowed. The identity is hidden from passive observers. The signature
binds the player key to this exact handshake (both ephemerals, the versions, the caps, the
content hash and the cookie). The password proof is bound to `th`, so it cannot be replayed.

### 3.5 Server processing order (cheap checks first)

1. **Hello:** length ≥ 1200, else drop; parse, else drop; version negotiation, else PreReject
   `VERSION`; content check, else PreReject `CONTENT`; otherwise Challenge. The only cost is one
   HMAC. No per-client state is kept.
2. **Auth:**
   1. Parse.
   2. Look up `s_eph` among the current and previous generations (`UnknownEph`).
   3. **Cookie check** (address and port bound, ≤ 30 s; `BadCookie` / `StaleCookie`). Only now
      is the source address proven.
   4. **Replay memory.** An Auth whose `c_eph` the server already answered (a resend or a
      replay) is answered from the replay memory here, before the budget is charged and before
      any public-key work, so a client's resends never drain its budget.
   5. **Per-source Auth budget.** A token bucket per source (`ip_key`: the IPv4 address, or the
      IPv6 /64), 5/s with a burst of 10, charged after the cookie proved the address (a spoofer
      cannot drain someone else's budget) and before any public-key work. Over budget: dropped
      (`AuthRateLimited`). The table holds at most 65 536 sources and fails closed beyond that.
      Hellos are not limited per source: they are stateless and cost one HMAC, and a per-source
      Hello limit would let a spoofer lock a victim's address out.
   6. Version negotiation again.
   7. Two X25519 operations (`LowOrder`).
   8. Derive the keys, open the payload (`Decrypt`), decode it.
   9. Verify the signature (`BadSignature`).
3. **Replay memory.** The server remembers each `c_eph` it accepted or rejected for 35 s (longer
   than the cookie TTL), at most 65 536 entries and at most 64 per source. A full memory refuses
   new Auths (fail closed, never evicts). Entries expire in insertion order. A second Auth with
   the same `c_eph`, or for an existing `conn_id`, is dropped, so a captured Auth replayed from
   the victim's own address allocates nothing. If the first outcome was a reject, the cached
   sealed reject is resent at most 3 times.
4. **Admission** is the application's decision on `PendingAuth`: ban by key and IP, full,
   password. The same player key while its session is still open is a **session resume**: the
   old connection is closed `REPLACED` and the same peer continues on the new one (same peer id
   and server state, no roster change for the others). Then `accept()` allocates the
   connection, or `reject()` returns an `S2CAuthReject`.

### 3.6 `S2CAuthReject` (type `0xA5`)

| Offset | Size | Field |
|---|---|---|
| 0 | 1 | type `0xA5` |
| 1 | 8 | `conn_id` |
| 9 | rest | `ChaCha20-Poly1305(key = s2c, nonce = u64::MAX−1, aad = bytes 0..9, code u8 ‖ text_len u16 ‖ text)` |

It is authenticated, so nobody can forge a "banned" or "full" message, and it is truncated so
that it is never larger than the Auth it answers.

### 3.7 Reject codes

| Code | Name | When |
|---|---|---|
| 1 | VERSION | no common version (pre-cookie) |
| 2 | CONTENT | `content_hash` differs (pre-cookie) |
| 3 | BANNED | key or IP ban |
| 4 | FULL | fighter and spectator caps reached |
| 5 | BAD_PASSWORD | wrong or missing `pwd_proof` |
| 6 | BAD_IDENTITY | key not acceptable |
| 7 | KICK_COOLDOWN | kicked recently (`retry_after_s` in the text) |
| 8 | SERVER_CLOSING | shutdown in progress |
| 9 | DUPLICATE_PLAYER | the same key is connected and replacement is refused |
| 10 | RATE_LIMITED | too many attempts |
| 255 | INTERNAL | — |

### 3.8 Client behaviour

- **Retransmission:** the Hello is resent at 250 ms, doubling to at most 2 s, until a Challenge
  or PreReject arrives. The Auth is then resent on the same schedule until the first data
  packet decrypts.
- **Timeout:** the handshake gives up after 10 s (`Failed("no answer from the server")`).
- A Challenge whose `echo` does not match, or that fails a check, is ignored (it may be forged).
  A pinned-key mismatch is fatal.
- **Connected** = the first data packet authenticates under `s2c`. It carries the `welcome`
  record on channel 2. Reliable messages queued before this point go out after it.

### 3.9 Amplification bounds

| Unauthenticated input | Reply | Bound |
|---|---|---|
| Hello < 1200 B, garbage, unknown type, legacy v4 | none | 0 |
| Data (≥ 26 B) for an unknown `conn_id` | `S2CReset` 25 B, global budget 50/s, 5/s (burst 5) per source | reply < request |
| Authenticated data from an unvalidated new address | path probe (sealed `PATH_CHALLENGE` 47 B, or PING 39 B for peers without `caps::PATH_CHALLENGE`), ≤ 1 per 50 ms; size checked before sealing | reply ≤ request |
| Hello ≥ 1200 B | Challenge 120 B or PreReject | reply ≤ request |
| Auth with a bad cookie, eph, AEAD or signature | none | 0 |
| Auth for an already-rejected `c_eph` | cached AuthReject, at most 3 times | reply ≤ request |

Tests: `amplification_is_bounded_and_spoofed_floods_leave_no_state` (2000 spoofed sources, total
bytes out ≤ total bytes in, no connections, no pre-auth state),
`spoofed_and_replayed_handshake_packets_allocate_nothing`, and the fuzz target
`server_datagram`, which asserts reply ≤ request for every input.

## 4. Connection layer

### 4.1 Data packet (type `0xB0`)

| Offset | Size | Field |
|---|---|---|
| 0 | 1 | type `0xB0` |
| 1 | 8 | `conn_id` |
| 9 | 4 | `pkt_seq`: low 32 bits of the sender's packet number |
| 13 | 4 | `ack`: low 32 bits of the highest packet number received |
| 17 | 4 | `ack_bits`: bit *i* = packet `ack − 1 − i` received |
| 21 | 1 | `flags`: `0x01` KEY_PHASE, `0x02` ACK_VALID; other bits must be 0 (else drop) |
| 22 | … | ciphertext of the chunk payload (§5.1) + 16-byte tag |

- The header is cleartext and authenticated as AEAD associated data, so tampering with any
  header byte fails the tag.
- Overhead: 22 + 16 = 38 bytes per datagram. Largest payload: `MAX_PLAINTEXT = 1162` bytes.

### 4.2 Packet numbers and replay window

- Each direction numbers its packets 0, 1, 2, … as a u64 and never reuses a number.
- The wire carries the low 32 bits. The receiver reconstructs the u64 closest to
  `highest_received + 1` (RFC 9000 §A.3 with a 32-bit window), so `pkt_seq` wraps freely.
- **Replay window: 1024 packets**, a bitmap ring. Newer than the highest: fresh.
  `highest − seq ≥ 1024`: too old. Otherwise fresh unless its bit is set (duplicate).
- The window is updated only after the AEAD tag verified, so a forged packet never moves it.
- The same bitmap produces `ack` / `ack_bits` for outgoing packets.

### 4.3 Acks, RTT, loss and pacing

- **Acks.** Every packet carries `ack` / `ack_bits` once anything has been received. A packet
  with a non-empty payload is ack-eliciting. After an ack-eliciting packet, an ack-only packet
  (empty payload) goes out when nothing else was sent for `ack_delay = 20 ms`, or at once after
  16 ack-eliciting packets. Ack-only packets are not acked themselves.
- **RTT** (RFC 6298): each newly acked largest packet gives a sample `now − sent_at`;
  `srtt ← 7/8·srtt + 1/8·s`, `rttvar ← 3/4·rttvar + 1/4·|srtt − s|`.
  `RTO = clamp(srtt + max(4·rttvar, 5) + 25, 50, 2000) ms`, initial 300 ms. The 25 ms term
  (`MAX_ACK_DELAY_MS`: the peer's 20 ms ack timer plus a 5 ms transport tick, RFC 9002 §6.2.1)
  adds back the time a peer may hold an ack.
- **`ACK_DELAY`** (`caps::ACK_DELAY`): a packet that carries an ack may also say how long the
  acked packet waited at the receiver. The sender subtracts it from the sample, clamped to 25 ms
  and never below `min_rtt − 25`, so a lying peer moves its RTT by at most 25 ms.
- **Loss.** An unacked packet is lost when a packet `reorder_thresh` numbers newer was acked
  (starts at 3), or a newer packet was acked and this one is older than
  `(8 + e)/8 · max(srtt, latest_rtt) + 5 ms` (`e` starts at 1, RFC 9002's 9/8), or it is older
  than `RTO << backoff`. Each time-based loss round doubles the backoff (cap ×16); any ack
  resets it.
- **Spurious loss.** The sender remembers the last 256 packets it declared lost. If one is acked
  after all, the loss was reordering: `reorder_thresh` doubles (max 32), `e` grows by one (max
  16), the time threshold is never below `srtt + 4·rttvar` from then on, a packet-threshold loss
  must also be at least one srtt old, and the congestion decrease is undone.
- **Pacing.** Every connection paces its data packets (channel 0, reliable new data and
  retransmissions) with a token bucket at `cc_rate`, burst `max(10 ms · rate, 4 × 1200 B)`.
  Ack-only packets, PINGs, PATH_RESPONSEs, path probes and CLOSE packets are never paced.
- **Congestion control** (AIMD on `cc_rate`): starts at 1 MiB/s; a loss event multiplies it by
  0.7, at most once per `max(srtt, 50 ms)`; floor 160 KiB/s, which stays above the relay's game
  stream budget so random loss only throttles bursts; while the pacer is the limit, every ack adds
  512 KiB/s per second, capped at 16 MiB/s. An application-limited connection never grows. A
  500 KB reliable burst (a world re-sync) leaves in about 0.4 s while channel-0 frames keep their
  latency.
- **Retransmission.** The reliable fragments of a lost packet go back to unsent and are re-sent
  in the next packet. An ack of any copy completes a fragment. Channel-0 data is never
  retransmitted.
- **Metrics** (`ConnStats`): srtt, rttvar, min RTT (after ack delay; the relay reads `srtt − min` as the standing queue), RTO, packets and bytes sent and received, lost,
  retransmits, duplicate and too-old drops, AEAD failures, key updates, `spurious_lost`,
  `cc_rate_bps`, `paced`, `local_stalls`.

### 4.4 Keepalive, idle timeout, path-dead, close

- **Keepalive:** nothing sent for 1000 ms → a `PING` chunk (ack-eliciting), so both sides see
  traffic at least once a second.
- **Idle timeout:** no authenticated packet for `idle_timeout` → `Closed{TIMEOUT}`. The default
  is 10 s; the server uses 20 s, so a pause or a short outage keeps the connection. The client
  uses 10 s, then re-handshakes, and the server treats that as a session resume.
- **Path-dead** (clients, 2 s): the client sent ack-eliciting packets but heard nothing for 2 s
  → `Closed{TIMEOUT}` and an immediate re-handshake. After half of that, keepalives go out every
  250 ms, so honest loss does not trip it.
  - **A local stall is not a dead path.** If the application has not called `poll_transmit` for
    more than half the path-dead time (the process itself stalled: a level load, antivirus, a
    laptop resume), the clock restarts at that poll, so replies already queued in the socket
    are processed first.
  - **With a reset token the bar is 5 s** (`DEAD_AFTER_WITH_RESET_MS`). A restarted server is
    then detected at once by the stateless reset (§4.7), so path-dead only has to catch a
    vanished server. Wi-Fi stalls of up to about 5 s keep the connection.
- **Close:** `close(code, reason)` sends a `CLOSE` chunk in three packets 30 ms apart, then the
  connection is `Closed`. Receiving `CLOSE` gives `Closed{code, by_peer: true}`.

| Close code | Name |
|---|---|
| 0 | NORMAL |
| 1 | TIMEOUT |
| 2 | PROTOCOL_VIOLATION |
| 3 | KICKED |
| 4 | SERVER_CLOSING |
| 5 | GAME_EXITED |
| 6 | REPLACED |
| 7 | RESET (local only: a verified `S2CReset` arrived) |

An authenticated payload with a malformed or unknown chunk, a bad channel, a reliable id beyond
the receive window or a reassembly violation closes the connection with `PROTOCOL_VIOLATION`.
Only the authenticated peer can cause this. Unauthenticated garbage is dropped and counted.

### 4.5 Key update

Each sender switches to `next(secret)` and flips KEY_PHASE after 2³⁰ packets or 1 hour in a
phase. The receiver, on a flipped phase bit, tries `next(current)`; on success it promotes it and
keeps the old key as `prev` for reordered stragglers; otherwise it tries `prev`. The two
directions update independently.

### 4.6 Addressing and path migration

Connections are routed by `conn_id`, not by address. A connection starts on the address that
completed the handshake.

1. A data packet for a known `conn_id` arrives from a new address. If it authenticates and is
   fresh, its messages are delivered at once: only the keys' holder can produce it.
2. The server does not send to the new address yet. It answers with a path probe: at most one
   per 50 ms, up to 8 outstanding, never larger than the packet that caused it, not tracked for
   loss. With `caps::PATH_CHALLENGE` it is a sealed `PATH_CHALLENGE` carrying 8 random bytes
   (47 B); otherwise a sealed PING (39 B).
3. **Validation.** With the capability, only a `PATH_RESPONSE` echoing one of those tokens,
   arriving from the new address, validates it. The client answers every `PATH_CHALLENGE` at
   once. Without the capability, a packet from the new address that acks one of the probes
   validates it.
4. **The move** happens only when, in addition, the current path has been silent for
   `max(2·srtt, 100 ms)` (no authenticated packet and no authentic duplicate from the current
   address), the connection has not migrated in the last 1 s and fewer than 5 times in the last
   minute, and no other live connection uses the new address. Then the address moves and the
   application gets `Incoming::Data{migrated_from}`. The server's `migrate_peer` keeps the peer's
   id, seat and state, and also applies the ban list and the per-public-IP seat cap to the new
   address; a refused migration closes the moving connection.
5. An authenticated packet on the current address cancels a pending candidate.

A captured packet replayed from a spoofed source is a duplicate and dropped. An off-path
observer that races copies from its own address fails the silence condition while the real
client keeps talking. A key holder spoofing a victim's address never sees the challenge sent
there. A NAT rebind recovers within about 500 ms at 80 ms RTT. The server keeps the old address
routable for 5 s so messages already addressed to it still go out.

### 4.7 Stateless reset

With `caps::RESET` a restarted server tells clients at once that it forgot them.

- **Token:** `HMAC-SHA256(reset_key, "HSMP/5 reset\0" ‖ conn_id)[..16]`, with
  `reset_key = HKDF(salt = "HSMP/5 stateless reset", ikm = server static secret,
  "hsmp5 reset key")`. It depends only on the server's long-term key, so a restart with the same
  identity file computes the same token.
- **Delivery:** the server puts a `RESET_TOKEN` chunk in its packets until one of them is acked.
  The token only ever travels encrypted.
- **Reset:** a data packet (≥ 26 B) for an unknown `conn_id` draws
  `S2CReset = 0xA6 · conn_id u64 · token[16]` (25 B), from a global budget of 50/s and 5/s
  (burst 5) per source, in a table of at most 4096 sources.
- **Client:** a reset whose `conn_id` and token match (constant-time compare) closes the
  connection with `RESET` and the sidecar re-handshakes at once. Anything else is ignored.
- The session layer sees the restart through a new `welcome.server_epoch`.

## 5. Channels and chunks

### 5.1 Chunk payload

The decrypted payload is a sequence of chunks:

| Kind | Layout | Notes |
|---|---|---|
| `0x00` PADDING | `00` … | The rest of the payload is ignored |
| `0x01` UNREL | `01 · key u32 · len u16 · data` | Channel 0; `len ≥ 1` |
| `0x02` REL | `02 · ch u8 · msg_id u32 · len u16 · data` | A complete reliable message, `ch` ∈ {1, 2} |
| `0x03` FRAG | `03 · ch u8 · msg_id u32 · idx u8 · count u8 · len u16 · data` | One fragment, `2 ≤ count ≤ 64`, `idx < count` |
| `0x04` PING | `04` | Ack-eliciting, no data |
| `0x05` CLOSE | `05 · code u8 · len u8 · reason` | `reason` ≤ 100 B UTF-8; parsing stops here |
| `0x06` ACK_DELAY | `06 · delay_ms u16` | Not ack-eliciting; only with `caps::ACK_DELAY` |
| `0x07` RESET_TOKEN | `07 · token[16]` | S→C, ack-eliciting; only with `caps::RESET` |
| `0x08` PATH_CHALLENGE | `08 · token[8]` | Ack-eliciting; in a path probe; only with `caps::PATH_CHALLENGE` |
| `0x09` PATH_RESPONSE | `09 · token[8]` | Ack-eliciting; the echo of a PATH_CHALLENGE, sent at once |

**Keyed chunks** (`caps::REL_KEY`): a `ReliableLatest` message sets bit 7 of the `ch` byte
(`ch | 0x80`, channel 1 only) and carries its u32 supersede key right after `msg_id`. The
receiver remembers the highest `msg_id` delivered per key (at most 4096 keys) and consumes,
without delivering, an older copy that arrives after a newer one. Without this, a stale
`game_status` that was already in flight could overwrite a newer one.

Any other kind is a protocol violation. A packet bundles as many chunks as fit in 1162 bytes.
The sender fills channel 0 first (only the newest message per key is queued, so pose frames never
wait behind a reliable backlog), then channel 2, then channel 1. PATH_RESPONSE, RESET_TOKEN and
ACK_DELAY go where they fit.

### 5.2 Channel semantics

| Ch | Name | Delivery | Used for (record `chan`) |
|---|---|---|---|
| 0 | unreliable-latest | At most once, never older than what was delivered for the same key. The sender keeps only the newest unsent message per key. | `latest`: root, weapon, pose, vitals, world state, ping / pong |
| 1 | reliable-unordered | Exactly once, in arrival order. `ReliableLatest{key}` cancels the retransmission of older unacked messages with the same key. | `reliable`: damage, deaths, clashes, world claims, notices, interactions. `rel_latest`: session, game status, pings, kit, loadout, grab updates |
| 2 | reliable-ordered | Exactly once, in send order | `ordered`: welcome, commands and results, chat, admin state, manifests, server closing, leave |

Channel-0 and `ReliableLatest` keys are `key = stream << 24 | entity`
(`hsmp_net::proto_v5::keys`). Channel-0 streams: ROOT 1, SKEL 2, WEAPON 3, VITALS 4, WORLD 5,
PING 6, VOICE 7. The entity is the source peer id (< 2²⁴); `proto::record_mode(kind, peer)` maps
a record's `chan` to the send mode. The receiver remembers the newest packet number per key (at
most 4096 keys) and drops older messages.

### 5.3 Reliable windows and limits

| Limit | Value | Enforced by |
|---|---|---|
| Messages in flight per reliable channel | `msg_id < oldest_unacked + 256` | Sender (later messages wait) |
| Bytes in flight (started messages) | 256 KiB | Sender |
| Bytes queued (all reliable) | 1 MiB | Sender: `send()` returns `Backpressure` |
| Channel 2 receive window | `msg_id < next_expected + 1024`, else violation | Receiver |
| Channel 2 out-of-order buffer | 1 MiB, else violation | Receiver |
| Channel 1 dedup | 1024-id window (older ids are duplicates) | Receiver |

Message ids are u32 per channel; wrapping would need 4·10⁹ messages on one connection. With a
256-message send window, any channel-1 id older than `highest − 256` must already have been
delivered, so treating ids beyond 1024 as duplicates is safe.

### 5.4 Fragmentation

- A reliable message larger than `MAX_SINGLE = 1154` bytes is split into `FRAG_DATA = 1152`-byte
  fragments, at most 64. `MAX_MESSAGE = 64 KiB`. Each fragment is retransmitted on its own.
- **Sender:** at most 32 fragmented messages started and incomplete at once per connection; a
  further one waits (single-chunk messages behind it still go). The receiver holds 64 partials,
  so an honest sender never overflows it.
- **Receiver, per connection:** 64 concurrent partials, 512 KiB held in partials, a partial idle
  for 30 s is discarded. A packet whose fragments would exceed these limits is deferred, not a
  violation: it is not marked received, so it is never acked and the sender retransmits.
- A `ReliableLatest` supersede drops only messages not yet on the wire. A fragmented message
  whose first fragments were sent is completed.
- Inconsistent `count`, `idx ≥ count`, empty fragments and over-limit sizes are violations.
  Duplicate fragments are ignored.
- Channel-0 messages are never fragmented (`MAX_UNRELIABLE = 1155` bytes, else `TooLarge`).

## 6. Messages: typed records

### 6.1 Framing

Every channel message is one record:

```
[WireHdr { kind: u16, aux: u16, peer: u32 }][record payload]
```

- The header is 8 bytes (`crates/hsmp-ipc/src/wire.rs`). The payload is the record's
  `#[repr(C)]` bytes exactly as they sit in shared memory.
- **Client to server:** `peer = 0`; the connection identifies the sender.
- **Server to client:** `peer` is the player the record is about or from (0 = the server). A
  relay copies the incoming message and patches these 4 bytes. `aux` is kind-specific: a
  relayed `pose` carries the relay interval for this sender and receiver in ms, which the
  receiver uses to size its jitter buffer.
- The sidecar sends what one shared-memory pump step produced together, so root, weapon and
  pose of one sample go out in one datagram.

### 6.2 Kinds and validation

A kind id is `domain << 8 | n` with `n` in `0x10..=0xFF`. Every kind, with its layout,
capability bit, allowed flow (`c2s`, `s2c`, `g2s`, `s2g`, `local`) and channel, is declared once
in `crates/hsmp-ipc/src/schema/<domain>.rs` (`RECORDS`). `hsmp-tools gen-ipc` generates the C
header and the Lua schema from it.

`hsmp_ipc::record::view` is the only way untrusted bytes become a record: exact size (head +
count × row), count ≤ max, finite floats, canonical `Str`, `Bool` 0 or 1, then the record's own
check (enum ranges, world bounds). The server dispatches by kind (`server/src/server/records.rs`),
validates in place and acts on the borrowed record. A kind not allowed in that direction, an
unknown kind and kind 0 are dropped. Server-originated records are built as structs and framed
with `wire::encode`.

The numeric codes the records carry (`phase`, `cmd_op`, `cmd_reason`, `notice`, ...) are in each
domain's `ENUMS`, available in Lua as `S.ENUMS.<table>.<NAME>` and in C as `HSMP_<TABLE>_<NAME>`.
Some also exist as constants in `hsmp_net::proto_v5::codes`.

### 6.3 Network records

Records with flow `local` or only `g2s` / `s2g` never go on the wire; they are listed in
[ipc-shared-memory.md](ipc-shared-memory.md).

**Pose (`0x01`)**

| Kind | Name | Dir | Channel | What |
|---|---|---|---|---|
| `0x0110` | `root` | C→S, S→C | latest | Root position, rotation (quaternion), velocity, tick, timestamps |
| `0x0111` | `weapon` | C→S | latest | Held weapon transform; kept by the server for lag compensation, not relayed |
| `0x0112` | `pose` | C→S, S→C | latest | A pose codec v2 frame as rows (≤ 640 bytes). The server decodes it once (structural check, lag compensation) and relays the incoming bytes |

**Session, match and connection (`0x02`)**

| Kind | Name | Dir | Channel | What |
|---|---|---|---|---|
| `0x0210` | `welcome` | S→C | ordered | First message: `server_epoch` (random at boot; a change means the server restarted), `server_time_ms`, negotiated `caps`, `peer_id`, `seat`, `role`, deduplicated `nick` |
| `0x0211` | `session` | S→C | rel_latest | The full, idempotent session snapshot (§6.4) |
| `0x0212` | `pings` | S→C | rel_latest | Every connected player's transport srtt, about 1 Hz (`caps::PING`) |
| `0x0213` | `command` | C→S | ordered | A player command (§6.5) |
| `0x0214` | `cmd_result` | S→C | ordered | Exactly one result per `cmd_id` |
| `0x0215` | `game_status` | C→S | rel_latest | The game's state report, about 1 Hz and on change: `match_id`, `round`, `world_key`, `flags` (LOADED, READY, DEAD, IN_MENU, SPECTATING, BACKGROUND), `spawn_id`, `load_error`, `arena`. The server counts a player as loaded only if `match_id`, `round` and `arena` match the frozen config |
| `0x0216` | `spawned` | C→S | ordered | The pawn was placed on its spawn order (`round`, `slot`, `pos`, `clear`) |
| `0x0217` | `notice` | S→C | reliable | `notice` code with up to 4 string args; idempotent by `event_id` |
| `0x0218` | `kill_feed` | S→C | reliable | Killer and victim seats, cause, weapon |
| `0x0219` | `kicked` | S→C | reliable | Terminal: no automatic rejoin before `retry_after_s`; followed by close `KICKED` |
| `0x021A` | `server_closing` | S→C | ordered | `closing_reason`, text, `reconnect_after_ms` (0 = do not); followed by close `SERVER_CLOSING` |
| `0x021B` | `leave` | C→S | ordered | `leave_reason`; the sidecar sends it when the game exits, then closes |
| `0x021C` | `chat` | C→S | ordered | A chat line |
| `0x021D` | `chat_in` | S→C | ordered | A chat line from a player, or the server (`from_peer` 0) |
| `0x021E` | `admin_state` | S→C | ordered | Who is admin, and the masked ban list (§6.6) |
| `0x021F` | `ping` | C→S | latest | Clock probe, every 2 s while connected |
| `0x0220` | `pong` | S→C | latest | Answer to `ping` with the server wall clock |

**Combat (`0x03`)**

| Kind | Name | Dir | Channel | What |
|---|---|---|---|---|
| `0x0310` | `damage` | C→S | reliable | A hit claim (≤ 24 rows), validated with lag compensation |
| `0x0311` | `damage_in` | S→C | reliable | An accepted hit, to its victim, which replays it natively |
| `0x0312` | `hitfx_in` | S→C | reliable | An accepted hit, to every other player that negotiated `caps::HIT_FX`, replayed on their stand-in of the victim for blood and wounds |
| `0x0313` | `damage_verdict` | S→C | reliable | Confirm / final / clash verdict with a `damage_reason` code |
| `0x0314` | `damage_ack` | C→S | reliable | Receipt of a delivered hit |
| `0x0318` | `clash` | C→S | reliable | The sender's screen showed a blade-on-blade contact |
| `0x0319` | `touch` | C→S | reliable | The sender's screen showed a peer's stand-in reach its body (evidence against a parry; `caps::HIT_FX`) |
| `0x0320` | `death_report` | C→S | reliable | The owning game reports its death |
| `0x0321` | `death_ack` | S→C | reliable | Receipt of a death report |
| `0x0322` | `death` | S→C | reliable | A declared death, with `death_cause` |
| `0x0328` | `vitals` | C→S, S→C | latest | Health and wound state as u16 values and `vitals_flag` bits |

**World (`0x04`)**

| Kind | Name | Dir | Channel | What |
|---|---|---|---|---|
| `0x0410` | `world_state` | C→S, S→C | latest | Leased physics bodies (≤ 32 rows, smallest-three quaternions) |
| `0x0411` | `world_claim` | C→S | reliable | Claim or release a body lease |
| `0x0412` | `world_sync` | C→S | reliable | Request a world snapshot |
| `0x0413` | `world_owners` | S→C | reliable | Lease owners |
| `0x0414` | `world_snapshot` | S→C | reliable | World snapshot |
| `0x0415` | `world_manifest` | C→S, S→C | ordered | The level's body manifest |
| `0x0416` | `world_dyn` | C→S, S→C | ordered | Dropped items |
| `0x0417` | `world_hash` | C→S | ordered | Per-body hashes for the consistency check |
| `0x0418` | `world_verdict` | S→C | ordered | Consistency verdict |

**Loadout (`0x05`)**

| Kind | Name | Dir | Channel | What |
|---|---|---|---|---|
| `0x0510` | `kit` | C→S | rel_latest | The player's kit |
| `0x0511` | `kit_verdict` | S→C | rel_latest | The server's verdict (accepted, replaced, default) per player |
| `0x0512` | `kit_rules_req` | C→S | ordered | Set the kit rules (admin) |
| `0x0513` | `kit_rules` | S→C | rel_latest | The current kit rules |
| `0x0514` | `loadout` | C→S, S→C | rel_latest | The full armour and weapon loadout, fragmented as needed |

**Interact (`0x06`, `caps::INTERACT`)**

| Kind | Name | Dir | Channel | What |
|---|---|---|---|---|
| `0x0610` | `interact` | C→S, S→C | reliable | Grab start / end, impulse, grab denied |
| `0x0611` | `interact_grab_r` | C→S, S→C | rel_latest | Right-hand grab update, superseded per (initiator, hand) |
| `0x0612` | `interact_grab_l` | C→S, S→C | rel_latest | Left-hand grab update |

Field-level definitions: the structs in `crates/hsmp-ipc/src/schema/<domain>.rs`, or the
generated `crates/hsmp-native/cpp/gen/hsmp_ipc.h`.

### 6.4 The `session` record

The one full snapshot of the server's state, sent at 1 Hz in the lobby, 3 Hz in a match, and at
once on any change. It is a variable record: `SessionHead` plus one `RosterRow` per seat (up to
64).

- **Head:** `epoch` (the server instance), `match_id` (new on every START, 0 in the lobby),
  `phase_deadline_ms` (server clock when the phase ends; in the lobby, when the no-admin auto
  start fires), `server_time_ms`, `seq`, `round`, `world_epoch`, `phase`, `winner_seat`,
  `result_reason`, `has_frozen`, the live `config` and the `frozen` config.
- **Phases:** LOBBY 0, LOADING 1, COUNTDOWN 2, LIVE 3, ROUND_OVER 4, MATCH_OVER 5, POST_MATCH 6,
  PAUSED 7.
- **Config:** `rev`, `arena`, `mode` (DUEL, FFA, TEAM_ELIM, KING_OF_HILL), `best_of`,
  `round_time_limit_s`, `team_rule`, `teams`, `kit_mode` / `kit_budget` / `kit_fairness`,
  `max_fighters`, `max_spectators`, `countdown_s`, `roundover_s`, `matchover_s`,
  `barrier_timeout_s`, `join_in_progress` (SPECTATE, NEXT_ROUND, NEVER).
- **Roster row:** `peer_id`, `loaded_round`, `wins`, `spawn_id`, `kit_rev`, `player_id`,
  `spawn_pos`, `spawn_yaw`, `spawn_protect_ms`, `seat`, `role` (FIGHTER, SPECTATOR, QUEUED),
  `team`, `admin_role` (NONE, MODERATOR, ADMIN, OWNER), `ready`, `alive`, `connected`, `nick`.

Receiver rules: drop the snapshot if `(epoch, seq) ≤ last`; a new epoch resets all tracking (in a
match it sends the Director back to the lobby with "Server restarted"). The map is
`frozen.arena` in a match, else `config.arena`. The own spawn is the own roster row's spawn.

### 6.5 Commands

`command { cmd_id, expected_rev (0 = don't care), op, flag, role, choice, peer_id, duration_s,
ballot, text, patch }`. `op` is one of READY 1, START 2, ABORT 3, PICK_ARENA 4, SET_CONFIG 5,
KICK 6, BAN 7, PROMOTE 8, VOTE 9, SWITCH_ROLE 10, SET_TEAM 11, UNBAN 12, RESET_MATCH 13.
`SET_CONFIG` carries a `ConfigPatch` whose `mask` says which fields are set.

`cmd_result { cmd_id, config_rev, reason_code, ok, op, reason_text }`. Reason codes: OK 0,
NOT_ADMIN 1, WRONG_PHASE 2, NOT_ALL_READY 3, REV_MISMATCH 4, UNKNOWN_ARENA 5, INVALID_VALUE 6,
UNKNOWN_PLAYER 7, NOT_ENOUGH_PLAYERS 8, RATE_LIMITED 9, UNSUPPORTED 10.

- Exactly one result per `cmd_id`. The server caches the last 64 results per player key and
  answers a duplicate with the cached result.
- Channel 2 already delivers exactly once within one connection. After a reconnect the sidecar
  resends a pending command every 250 ms until its result arrives, giving up after 5 s; the
  cache makes that idempotent.

### 6.6 Admin assignment and ban privacy

Admin rights are decided by the verified handshake `player_key`. The join order, the address
and the nick never decide them.

- **Listen server** (started by the HOST menu): the hosting player is the owner (`admin_role` 3).
  The server learns the host's key from `--owner-key <hex>` or `--owner-key-file <path>`.
  HSMPMenu passes the host's `<state>/.player_key`, which `hsmp-sidecar` writes before its first
  Hello; the server re-reads it at each join until the host has joined. The host is admin
  whenever it is connected. When it drops, no one inherits admin.
- **Dedicated server:** no admin by default. Admins (`admin_role` 2) come from `--admin-key <hex>`
  (repeatable), `--admins-file` (one key per line, `#` comments, re-read on change), RCON
  `ADMIN ADD <peer_id|player_id|key>` (written to the admins file when one is configured), or a
  connected admin's PROMOTE (a grant, not persisted).
- **`admin_state.admin_peer` is per recipient.** An admin receives its own peer id, so every
  admin's client shows the admin tools. Everyone else receives the lowest-id connected admin, or
  0 when none is connected.
- **Ban list:** non-admins receive no rows. Admins receive masked entries `<prefix>#<token>`
  (`203.0.x.x#1a2b3c4d`, `2001:db8:x#1a2b3c4d`); the token is 8 hex characters of
  SHA-256(per-run salt ‖ ip). UNBAN accepts the masked entry or its token. Full IPs are only
  available over RCON (`BANS`).
- **No admin connected:** when at least 2 players are connected, all are ready, and no admin is
  connected, the server arms a 5 s start (the lobby snapshot's `phase_deadline_ms` and a SERVER
  chat line). Anyone un-readying, or an admin arriving, cancels it. A dedicated server without
  an admin therefore never deadlocks in the lobby.
- **Owner leaving:** a listen host leaving on purpose closes the server (`HSMP_LISTEN_HOST=1`).
  Only the owner key does this, never an admin it granted. A host that drops is announced with
  the `HOST_LEFT` notice.

## 7. Versions and capabilities

### 7.1 Version ranges

- The Hello carries `version_min..=version_max`. The server picks the highest common version, or
  answers PreReject `VERSION` with its own range and an actionable text.
- This build: `VERSION_MIN = VERSION_MAX = PROTOCOL_VERSION = 6`
  (`crates/hsmp-net/src/net/mod.rs`).
- Adding a record kind does not bump the version: it goes behind a capability bit. Changing an
  existing record's layout changes the shared-memory layout hash; add a new kind instead.

### 7.2 Capability bits (u64)

The client sends its bits in the Hello, the server in the Challenge, and the negotiated set is
`client & server`, echoed in `welcome.caps`. A sender must not send anything a bit gates unless
it was negotiated. Receivers ignore unknown bits. New bits are append-only.

| Bit | Name | Status |
|---|---|---|
| 0 | MODES | Reserved: game-mode sections, kill feed gating |
| 1 | ZONE | Reserved |
| 2 | INTERACT | Offered by server and sidecar: the interact records |
| 3 | BRACKET | Reserved |
| 4 | MAP_HASH | Reserved |
| 5 | POSE2 | Reserved name; pose codec v2 is the only pose format |
| 6 | BUNDLE | Reserved |
| 7 | DELTA | Reserved |
| 8 | RESUME | Reserved: resume tickets (path migration works without a bit) |
| 9 | PASSWORD | Reserved: password servers |
| 10 | PING | Offered by server and sidecar: the `pings` record |
| 11 | ACK_DELAY | Transport: `ACK_DELAY` chunk (§4.3) |
| 12 | RESET | Transport: `RESET_TOKEN` chunk and `S2CReset` (§4.7) |
| 13 | POSE_RATE | Offered by the sidecar only. In v6 every relayed `pose` carries its relay interval in `aux`, so this bit gates nothing |
| 14 | PATH_CHALLENGE | Transport: path validation by an echoed token (§4.6) |
| 15 | REL_KEY | Transport: keyed `ReliableLatest` chunks (§5.1) |
| 16 | HIT_FX | Offered by server and sidecar: `touch` up, `hitfx_in` down |

`caps::SUPPORTED = ACK_DELAY | RESET | PATH_CHALLENGE | REL_KEY` are the transport bits
`hsmp-net` implements itself; every client and server built from it offers them through
`ClientConfig::new` / `ServerConfig::new`.

### 7.3 v4 clients

There is no dual stack. The server drops v4 datagrams (`DatagramKind::LegacyV4`). It answers a v4
join with one v4-format reject that starts with `OUTDATED`, truncated to the size of the request
and at most once per IP per 10 s (`net::V4_REJECT_TEXT` in `server/src/net/mod.rs`). Old
installs get a clear "outdated" banner and the server is not an amplifier.

## 8. Limits

| Limit | Value |
|---|---|
| Datagram | ≤ 1200 B; Hello ≥ 1200 B |
| Data overhead | 38 B per datagram + 7–10 B per chunk; plus the 8-byte record header per message |
| Channel-0 message | ≤ 1155 B |
| Reliable message | ≤ 64 KiB (≤ 64 fragments of 1152 B) |
| Replay window | 1024 packets |
| Reliable send window | 256 messages per channel, 256 KiB in flight, 1 MiB queued, ≤ 32 fragmented messages in progress |
| Receive buffers | ch-2 window 1024 ids and 1 MiB; reassembly 64 partials and 512 KiB, 30 s TTL (overflow defers the packet); 4096 supersede keys |
| Channel-0 keys tracked | 4096 |
| Cookie | 20 B, 30 s TTL, address and port bound |
| Ephemeral and cookie-key rotation | 120 s, one previous generation |
| Auth replay memory | 35 s, ≤ 65 536 entries, ≤ 64 per source (IPv4 / IPv6 /64) |
| Auth budget per source | 5/s, burst 10 (after the cookie) |
| Stateless resets | 25 B, 50/s global, 5/s per source |
| Path probes | ≤ 1 per 50 ms, ≤ 8 outstanding; migration after `max(2·srtt, 100 ms)` of silence on the old path, ≥ 1 s apart, ≤ 5 per minute |
| Path-dead (client) | 2 s of silence while sending (5 s once the reset token is held); a local stall restarts the clock |
| Handshake | 250 ms resend, doubling to 2 s, 10 s give-up |
| Keepalive / ack delay | 1000 ms / 20 ms; an ack at once after 16 ack-eliciting packets |
| Idle timeout | 10 s default; the server uses 20 s |
| RTO | `srtt + max(4·rttvar, 5) + 25` ms; initial 300 ms, [50, 2000] ms, backoff ×16 max |
| Loss thresholds | 3 packets / 9/8·RTT, widening to 32 packets / 3·RTT on spurious loss |
| Pacing / congestion | token bucket at `cc_rate`: 1 MiB/s initial, ×0.7 per loss event, +512 KiB/s per s while limited, floor 160 KiB/s, cap 16 MiB/s |
| Key update | 2³⁰ packets or 1 h |
| Nick / build / reject text | 32 / 32 / 512 B |

## 9. Testing and fuzzing

`cargo test -p hsmp-net` runs the transport tests (`net/tests.rs`, `net/tests_transport.rs` and
the per-module tests).

| Requirement | Tests |
|---|---|
| Handshake round trip | `handshake_round_trip_derives_matching_keys` (also asserts no secret bytes in any cleartext datagram), `handshake_round_trip_and_bidirectional_messages` |
| Tampered packets | `tampered_header_or_body_is_rejected_without_moving_the_window` (every byte flipped), `cookie_binds_address_hello_and_time` |
| Replayed and spoofed packets | `replayed_datagram_is_rejected`, `spoofed_and_replayed_handshake_packets_allocate_nothing` |
| Signature and low-order points | `signature_and_low_order_points_are_checked` |
| Replay-window edges and wraparound | `window_edges`, `advancing_clears_stale_bits`, `expand_handles_u32_wrap`, `window_works_across_wire_wraparound` |
| Reliable delivery over a bad link | `reliable_delivery_over_a_lossy_reordering_duplicating_link` (5 schedules at 20–30 % loss, 10 % duplication, 30–70 ms jitter, ordered, unordered and fragmented messages plus a 60 Hz channel-0 stream) |
| Fragmentation limits | `reassembles_in_any_order_with_duplicates`, `rejects_malformed_fragments_without_side_effects`, `enforces_message_and_buffer_limits`, `caps_concurrent_partials`, `fragmented_burst_with_alternate_loss_completes`, `reassembly_overflow_defers_instead_of_closing` |
| Transport regressions (`net/tests_transport.rs`) | RTO and loss: `clean_link_has_no_spurious_losses_or_retransmits`, `jittery_link_adapts_reorder_threshold`; migration: `off_path_racer_cannot_steal_the_path`, `owner_forged_path_response_cannot_redirect_the_stream`, `migrations_are_rate_limited`, `migration_never_displaces_another_live_connection`, `nat_rebind_migrates_the_connection_within_500_ms`; pacing: `reliable_burst_is_paced_and_latest_keeps_flowing`, `congestion_rate_decreases_on_loss_and_recovers`; stalls: `a_local_stall_is_not_a_dead_path`, `a_four_second_wifi_stall_keeps_the_connection`; others: `reliable_latest_never_delivers_an_older_copy_last`, `auth_resends_do_not_drain_the_source_budget`, `accept_never_overwrites_a_live_connection`, `stateless_resets_are_shared_per_source`, `oversize_probe_is_never_sealed` |
| Amplification | `amplification_is_bounded_and_spoofed_floods_leave_no_state`, `version_and_content_rejects_fit_in_the_hello`, `auth_reject_is_sealed_and_bounded`, `rejected_client_gets_an_authenticated_reason` |
| Version ranges | `version_negotiation_table`, `version_mismatch_is_reported_before_the_cookie` |
| Lifecycle | `idle_timeout_and_keepalive`, `close_is_delivered_and_repeated`, `server_close_reaches_the_client_and_is_reaped`, `silent_peer_times_out`, `key_update_survives_reordering`, `pinned_key_mismatch_fails_closed` |

**Fuzz entry points** (`hsmp_net::fuzz`): each is panic-free for any input, and a unit test
smoke-runs all of them over random and mutated inputs.

| Function | Target |
|---|---|
| `server_datagram` | Any datagram into a server endpoint; asserts reply ≤ request |
| `client_handshake` | Any datagram into a client awaiting the Challenge, plus all handshake parsers |
| `conn_payload` | Authenticated payloads (AEAD bypassed): chunks, channels, reassembly |
| `conn_datagram` | Raw datagrams into an established connection |
| `reassembly` | Fragment sequences |
| `replay` | Sequence-number streams through the window |

**Record decoders:** `server/tests/decode_fuzz.rs` sends every registered kind and kind 0 behind
a header, random and mutated, through `schema::check_payload` and the domain handlers;
`record_fuzz.rs` and `pose_fuzz.rs` cover the record and pose decoders.

## 10. Design notes

- **No Noise `IK`.** The handshake is plain X25519 + HKDF + ChaCha20-Poly1305. A stateless retry
  cookie does not fit Noise's message pattern without an extra round trip.
- **Server ephemerals are shared between clients for up to 2 minutes.** That bounds forward
  secrecy at about 4 minutes and keeps the Hello path to one HMAC.
- **`conn_id` is visible to observers.** It is derived from secret material, so it cannot be
  linked to the keys. Path migration keeps it, so an observer of both paths can link a NAT
  rebind; rotating it is left for `caps::RESUME`.
- **Reconnect** is a new handshake plus seat reclaim by `player_key`. `resume_ticket` and the
  resume flag are reserved.
- **Password proof:** the wire field and the transcript binding are defined; password servers
  (`caps::PASSWORD`) are not enabled.
