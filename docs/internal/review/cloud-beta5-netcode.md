# beta5 review: netcode (hsmp-net, hsmp-nat, relay, lag comp)

Branch `cloud/netcode`, on top of `acbc700` (0.1.0-beta.4). There are no wire format changes:
beta.4 peers interoperate unchanged. All numbers are from the Linux review box (4 cores, shared
with other builds), so read the wall-clock timings as relative, not absolute.

## Changes

### 1. Transport: fewer allocations per packet, cheaper ack processing (`0b8eb55`)

- Data packets are sealed and opened in place (`crypto::seal_in_place`, `crypto::open_into`).
  Before, each packet built a ciphertext or plaintext `Vec` and then copied it.
- The payload is assembled in a buffer kept on the `Conn`, so a poll with nothing to send
  allocates nothing. The server polls every connection on every tick.
- `Outbox::fill` keeps unsent channel-0 frames in place (`VecDeque::retain`) instead of
  rebuilding the queue on every call.
- A single-fragment reliable message is built without an intermediate `Vec`.
- `on_ack` skips ack bits below the oldest outstanding packet. Before, every ack did 32
  `BTreeMap::remove` lookups, most of them for packets that were already acked.

Evidence: `crates/hsmp-net/tests/alloc_hotpath.rs` uses a per-thread counting global allocator
and fails on the old code.

| (release) | before | after |
|---|---|---|
| channel-0 stream, allocations per round (server frame + every 4th client frame, both directions) | 9.75 | 3.75 (the returned datagram and the deliveries, which the caller owns) |
| idle `poll_transmit` | 1 | 0 |
| reliable stream, allocations per round | 10.5 | 5.3 |
| callgrind instructions, whole test | 1,152.5 M | 1,014.7 M (−12 %) |
| `BTreeMap::remove` inside it | 47.6 M | 5.4 M |

Commands:

- `cargo test -p hsmp-net --release --test alloc_hotpath -- --nocapture`
- `valgrind --tool=callgrind <alloc_hotpath test binary> --test-threads 1`

The crypto test `in_place_variants_match_the_allocating_ones` shows the in-place variants produce
the same bytes as the allocating ones. All 84 hsmp-net lib tests pass, including the lossy and
reordering netsim schedules, key update across reordering, and the tamper test.

### 2. `Conn::next_timeout` ignored a paced reliable backlog (`5cbc3bf`)

`next_timeout` only woke the caller for the pacer when channel-0 data was pending. If the pacer
was holding back reliable data or retransmissions, the connection reported itself due at the
next RTO or keepalive instead: 300 ms instead of 1 ms. A caller that sleeps until `next_timeout`
would have stalled a world sync on every pacer burst.

The fix counts reliable data only while the pacer is what holds it back, so a backlog waiting
for the send window does not cause busy wake-ups.

Today the server and the sidecar tick on a fixed period and do not call `next_timeout`, so this
fixes the sans-IO API rather than live behaviour.

Test: `net::conn::tests::next_timeout_wakes_for_paced_reliable_data`. Before the fix it failed
with "next_timeout 300 ms, pacer ready at 1 ms". It also covers the window-blocked case.

### 3. Lag comp: attacker RTT over the last 4 s (`8b052f9`)

`predict_view` subtracts the attacker's RTT from a clock offset that is measured over 2 s. The
RTT itself was the minimum of the last 16 samples, which means 16 s of the once-a-second
transport srtt (samples up to 60 s old counted).

After a path gets slower (a route change, or a download filling the link), the prediction
stayed on the old RTT for 16 s. A 40 → 140 ms step put it 100 ms off, far outside the ±30 ms
view tolerance, so honest hints were clamped onto the wrong victim pose. A second problem: a
burst of damage→ack samples during a fight (which include client processing time) could push
every transport sample out of the 16-entry deque and inflate the minimum.

Now the RTT is the minimum over the 4 s before the newest sample. That window still holds
several transport samples, so the processing time in damage→ack samples is still filtered out.
The deque now holds 64 samples. The same RTT feeds the defender grace and the parry-window
release.

Test: `lagcomp::tests::prediction_follows_an_rtt_step_within_seconds`. It checks exact values
for the RTT after the step and after a 30-sample fight burst, plus an end-to-end prediction
within one frame of the honest display. It fails on the old code.

Combat sim, `cargo run --release -p hsmp-combat-sim -- --seeds 3`, all profiles, before → after.
The links in the sim are static, so this is a no-regression check:

- loopback, good, typical, intl, far: 100 / 100 / 100 / 100 / 99.71 %, unchanged.
- wifi: 99.91 → 100 %.
- bad: 97.02 → 97.41 %.
- Every cheat's false-accept rate is unchanged (DamageInflate 0.53 %, AckHold 1.89 %, as at
  baseline).

### 4. Relay: FxHash maps, and budget charge includes the ACK_DELAY chunk (`6bfcff7`)

`Relay::select` runs for every recipient of every stream frame and does several map lookups
keyed by (recipient, sender) addresses each time. SipHash over socket addresses was most of its
cost. The relay's maps now use a local FxHash. The keys are the addresses of admitted peers (at
most 64), so hash flooding is not a concern.

Test: `relay::tests::select_cost`. It runs in release, uses 1-byte frames so there are no budget
drops, and makes the same picks before and after. Command:
`cargo test -p hsmp-server --release --bin hsmp-server select_cost -- --nocapture`.

| players | before | after |
|---|---|---|
| 16 | 3,980 ns per frame | 980 ns per frame |
| 64 | 18,677 ns per frame | 5,044 ns per frame |

At 64 players streaming at 60 Hz that is roughly 110 → 30 ms of CPU per second.

`SEAL_OVERHEAD` was 45 bytes, but on a live connection every datagram also carries the 3-byte
ACK_DELAY chunk (`caps::ACK_DELAY` is always negotiated). The stream plan therefore overspent
each recipient's budget by about 1 %. It is now 48. `seal_overhead_matches_a_real_v5_datagram`
now measures a negotiated connection; it failed before the fix (112 vs 109).

## Looked at, no change

- **RTT/RTO, loss detection, spurious-loss undo, AIMD.** These follow RFC 6298 and 9002 as
  documented. The additive increase is time-based, so it does not depend on RTT and is fair
  between 0 and 300 ms of ping. I found no bug I could prove.
- **Small pacer accounting inaccuracy.** An ack-only packet sent while data is window-blocked is
  charged to the pacer (about 41 B). Negligible, left as is.
- **Relay `PathCtl` on long, jittery paths.** The existing tests
  (`a_long_jittery_path_without_loss_keeps_the_full_rate`, `a_far_jittery_path_is_not_a_queue`)
  cover it. The base RTT tracks the lower envelope of srtt, not the minimum sample.
- **Lag comp rings.** Interpolation, sorted insert, lead clamping and restart are covered by
  existing exact-value tests. Stale samples older than 2 s that could reset a ring cannot
  arrive, because channel 0 drops older packets per key.
- **hsmp-nat.** Off the hot path; reviewed, nothing worth changing.

## Bandwidth: the biggest remaining item, not done

The relay sends each stream frame through `send_bytes` → `poll_transmit_one`, which means one
datagram per relayed message, each with 38 B of header and tag. With an average stream message
of about 290 B, bundling one recipient's frames per tick would save about 11 % of stream
bandwidth, for at most one tick of added latency.

The receive side already handles several chunks per packet, so this needs no wire change
(`caps::BUNDLE` is reserved but not required). It is a change to the server loop
(`server/src/server/broadcast.rs`, `server/src/net/mod.rs`), which is outside this area, so I
did not make it.

## Test results

- `cargo test --workspace --locked -j 2 --no-fail-fast`: 1128 passed, 12 failed. The 12 are
  exactly the known Linux-only baseline failures (7 in hsmp-launcher, 5 in hsmp-native shared
  memory). The combat-sim G0 thresholds and `alloc_hotpath` pass.
- `cargo clippy --workspace --all-targets --locked -j 2`: exit 0, 179 warning lines, same as
  baseline. None of them is new.

## Not run, and what still needs checking

- **hsmp-loadtest and `bench/`.** Not run. Both need release builds of the server, and disk was
  full during the review.
- **Windows and in-game.** Nothing here is Windows-specific. Worth a soak with two clients to
  confirm:
  - transport stats stay normal (srtt, loss, `paced`);
  - hit acceptance holds while a client's RTT changes mid-fight, for example by starting a
    download.

## Risks

- **Lag comp RTT window.** On a link whose srtt is noisy, the 4 s minimum sits a few ms above the
  old 16-sample minimum. That is well inside the view tolerance, and the combat sim shows no
  loss.
- **In-place open.** `open_into` re-copies the ciphertext before every key it tries, so a
  failed attempt cannot affect the next one.
