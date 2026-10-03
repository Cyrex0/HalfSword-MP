//! Hot-path relay for high-rate streams (root, skeletal, weapon).
//!
//! * Encode-once fan-out — the record message is built once and handed to
//!   the v5 transport per recipient (`crate::net`).
//! * Rate plan per (recipient, sender, stream): every stream has a
//!   target rate by relevance (the two nearest players' skeletons at the
//!   sender's full rate, never below 30 Hz; root 30/20 Hz; weapon 5 Hz, it
//!   has no in-game reader) and the plan is fitted into the recipient's
//!   budget farthest-first, never below each stream's floor.
//! * Per-recipient bandwidth budget — token bucket over everything sent to a
//!   peer; stream frames are dropped (never reliable events) when it runs dry,
//!   skeletal before root. The bucket never goes below −burst, the
//!   measured non-stream downstream (events, world, loadouts) is taken off
//!   the stream plan, and one sender gets at most 1.5× its fair share of
//!   a recipient's budget.
//! * Each forwarded skeletal frame tells the recipient its current relay
//!   interval (the pose record's `aux`, caps::POSE_RATE) and tells lag
//!   comp which frames the recipient was actually sent (`note_relayed`).

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::atomic::Ordering;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use crate::proto::PeerId;

/// Sustained downstream budget per client (bytes/s), set once at startup
/// from `--client-budget-kbps` (default 128 KB/s ≈ 1 Mbit/s).
static BUDGET_BPS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(128 * 1024);
pub fn set_client_budget_kbps(kb: u64) { BUDGET_BPS.store(kb.max(16) * 1024, Ordering::Relaxed); }
fn budget() -> f64 { BUDGET_BPS.load(Ordering::Relaxed) as f64 }
fn burst() -> f64 { budget() * 0.375 }
/// Skeletal frames leave this much headroom so root updates keep flowing.
const SKEL_RESERVE: f64 = 3.0 * 1024.0;
/// Bytes a v5 datagram adds around one channel-0 body: header 22 + AEAD tag
/// 16 + UNREL chunk header 7 (docs/development/protocol.md).
pub const SEAL_OVERHEAD: usize = 22 + 16 + 7;
const RANK_REFRESH: Duration = Duration::from_millis(500);
const FAR_UU: f32 = 6000.0;

/// `Vitals` are never thinned by relevance (every peer needs
/// them) but share the per-recipient budget.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Stream { Root, Skel, Weapon, Vitals }

struct Bucket { tokens: f64, last: Instant }

/// Measured send rate and frame size of one source stream.
#[derive(Default, Clone, Copy)]
struct SrcStat { frames: u32, rate: f64, bytes: f64 }

#[derive(Default)]
struct RelayInner {
    pos: HashMap<SocketAddr, [f32; 3]>,
    /// (dst, src) -> (rank among dst's others by distance, distance)
    rank: HashMap<(SocketAddr, SocketAddr), (u16, f32)>,
    counters: HashMap<(SocketAddr, SocketAddr, Stream), u32>,
    buckets: HashMap<SocketAddr, Bucket>,
    /// Per (dst, src) share of dst's budget (fair share).
    shares: HashMap<(SocketAddr, SocketAddr), Bucket>,
    ranked_at: Option<Instant>,
    src: HashMap<(SocketAddr, Stream), SrcStat>,
    /// Budget-fitted decimation factor per (dst, src, stream).
    alloc: HashMap<(SocketAddr, SocketAddr, Stream), u32>,
    /// Non-stream bytes charged per dst since the last plan, and their
    /// measured rate.
    other_bytes: HashMap<SocketAddr, f64>,
    other_rate: HashMap<SocketAddr, f64>,
    /// Peer id at each address (lag comp, per-peer caps).
    ids: HashMap<SocketAddr, PeerId>,
    srcs_n: usize,
}

/// Share of the per-client budget planned for streams; the rest is headroom
/// for reliable/control traffic and bursts.
const STREAM_SHARE: f64 = 0.9;
/// The stream plan never shrinks below this share of the budget, however
/// much non-stream traffic was measured.
const MIN_STREAM_SHARE: f64 = 0.4;
/// A sender may use at most this many times its fair share of a recipient's
/// budget: one flooding sender cannot starve everyone else.
const FAIR_SHARE_X: f64 = 1.5;
/// The two nearest players: who you are fighting.
const NEAR: u16 = 2;
/// Rate floors (Hz) the fit never goes below.
const SKEL_NEAR_FLOOR_HZ: f64 = 30.0;
const SKEL_FLOOR_HZ: f64 = 7.5;
const ROOT_FLOOR_HZ: f64 = 10.0;
const WEAPON_FLOOR_HZ: f64 = 1.0;
/// Assumed sender rate before the first measurement.
const DEFAULT_RATE_HZ: f64 = 60.0;

#[derive(Default)]
pub struct Relay { inner: Mutex<RelayInner> }

/// Target rate (Hz) of a stream by relevance, before the budget fit.
fn target_hz(stream: Stream, rank: u16, dist: f32) -> f64 {
    let far = dist > FAR_UU;
    match stream {
        // The skeleton carries the pelvis: root only steers the stand-in
        // between frames and feeds relevance.
        Stream::Root => if rank < NEAR && !far { 30.0 } else { 20.0 },
        // No in-game reader (`.weapon_remote`): diagnostics only.
        Stream::Weapon => 5.0,
        Stream::Skel => {
            if far { 15.0 } else if rank < NEAR { f64::INFINITY } else if rank < 4 { 30.0 } else { 15.0 }
        }
        Stream::Vitals => f64::INFINITY,
    }
}

/// Lowest rate (Hz) the budget fit may cut a stream to.
fn floor_hz(stream: Stream, rank: u16) -> f64 {
    match stream {
        Stream::Skel => if rank < NEAR { SKEL_NEAR_FLOOR_HZ } else { SKEL_FLOOR_HZ },
        Stream::Root => ROOT_FLOOR_HZ,
        Stream::Weapon => WEAPON_FLOOR_HZ,
        Stream::Vitals => f64::INFINITY,
    }
}

/// Send every `factor`-th frame of a `rate_hz` stream to get ≤ `hz`.
fn factor_for(rate_hz: f64, hz: f64) -> u32 {
    if !hz.is_finite() || hz <= 0.0 { return 1; }
    (rate_hz / hz).ceil().clamp(1.0, 64.0) as u32
}

/// Largest factor that keeps a `rate_hz` stream at or above `floor` Hz.
fn max_factor(rate_hz: f64, floor: f64) -> u32 {
    if !floor.is_finite() { return 1; }
    ((rate_hz / floor).floor() as u32).clamp(1, 64)
}

/// The unfitted factor of a stream.
fn factor(stream: Stream, rank: u16, dist: f32, rate_hz: f64) -> u32 {
    let rate = if rate_hz > 0.0 { rate_hz } else { DEFAULT_RATE_HZ };
    factor_for(rate, target_hz(stream, rank, dist))
}

/// One recipient chosen for a frame, and the pair's relay interval (ms).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Pick { pub dst: SocketAddr, pub interval_ms: u16 }

impl Relay {
    pub fn update_pos(&self, addr: SocketAddr, pos: [f32; 3]) {
        self.inner.lock().unwrap().pos.insert(addr, pos);
    }

    /// The peer id at `addr` (admission, resume, migration).
    pub fn set_peer(&self, addr: SocketAddr, id: PeerId) {
        self.inner.lock().unwrap().ids.insert(addr, id);
    }

    pub fn peer_id(&self, addr: &SocketAddr) -> Option<PeerId> {
        self.inner.lock().unwrap().ids.get(addr).copied()
    }

    /// Account bytes sent outside the stream path (events, match state) so
    /// the budget reflects the client's whole downstream. The bucket never
    /// goes below −burst: a whole-level world sync must not black out
    /// every stream to that client for seconds.
    pub fn charge(&self, dst: SocketAddr, bytes: usize) {
        let mut r = self.inner.lock().unwrap();
        *r.other_bytes.entry(dst).or_insert(0.0) += bytes as f64;
        let b = r.buckets.entry(dst).or_insert(Bucket { tokens: burst(), last: Instant::now() });
        refill(b);
        b.tokens = (b.tokens - bytes as f64).max(-burst());
    }

    pub fn forget(&self, addr: &SocketAddr) {
        let mut r = self.inner.lock().unwrap();
        r.pos.remove(addr);
        r.buckets.remove(addr);
        r.other_bytes.remove(addr);
        r.other_rate.remove(addr);
        r.ids.remove(addr);
        r.rank.retain(|(d, s), _| d != addr && s != addr);
        r.shares.retain(|(d, s), _| d != addr && s != addr);
        r.counters.retain(|(d, s, _), _| d != addr && s != addr);
        r.alloc.retain(|(d, s, _), _| d != addr && s != addr);
        r.src.retain(|(s, _), _| s != addr);
    }

    /// Every RANK_REFRESH: re-rank everyone by distance, measure the source
    /// stream rates and fit each recipient's plan into its budget. Runs on the
    /// server tick, off the receive loop. O(N² log N) per call.
    pub fn replan_if_due(&self, dsts: &[SocketAddr]) {
        let now = Instant::now();
        let mut r = self.inner.lock().unwrap();
        let due = r.ranked_at.map_or(true, |p| now.duration_since(p) > RANK_REFRESH);
        if !due { return; }
        let dt = r.ranked_at.map(|p| now.duration_since(p).as_secs_f64());
        rerank(&mut r, dsts);
        r.srcs_n = dsts.len().saturating_sub(1).max(1);
        if let Some(dt) = dt {
            measure_rates(&mut r, dt);
            allocate(&mut r, dsts);
        }
        r.ranked_at = Some(now);
    }

    /// Recipients of one stream frame from `src`, after the rate plan, the
    /// fair share and the bandwidth budget, each with the pair's interval.
    /// Charges the budget for the chosen ones.
    pub fn select(&self, src: SocketAddr, dsts: &[SocketAddr], stream: Stream, wire_bytes: usize) -> Vec<Pick> {
        let pf = crate::perf::perf();
        let mut r = self.inner.lock().unwrap();
        let now = Instant::now();
        let rate = {
            let s = r.src.entry((src, stream)).or_default();
            s.frames += 1;
            s.bytes = if s.bytes == 0.0 { wire_bytes as f64 } else { s.bytes * 0.9 + wire_bytes as f64 * 0.1 };
            if s.rate > 0.0 { s.rate } else { DEFAULT_RATE_HZ }
        };
        let share_rate = budget() * FAIR_SHARE_X / r.srcs_n.max(1) as f64;
        let mut out = Vec::with_capacity(dsts.len());
        for &dst in dsts {
            if dst == src { continue; }
            let (rank, dist) = r.rank.get(&(dst, src)).copied().unwrap_or((0, 0.0));
            let f = r.alloc.get(&(dst, src, stream)).copied().unwrap_or_else(|| factor(stream, rank, dist, rate));
            let c = r.counters.entry((dst, src, stream)).or_insert(0);
            *c = c.wrapping_add(1);
            // The first frame of a stream always goes (a rarely-updated stream
            // must not lose its only frame); then every f-th.
            if f > 1 && c.wrapping_sub(1) % f != 0 {
                pf.thinned.fetch_add(1, Ordering::Relaxed);
                continue;
            }
            // Fair share of dst's budget per sender (streams only).
            let sh = r.shares.entry((dst, src)).or_insert(Bucket { tokens: share_rate * 0.375, last: now });
            let dt = now.duration_since(sh.last).as_secs_f64();
            sh.last = now;
            sh.tokens = (sh.tokens + dt * share_rate).min(share_rate * 0.375);
            if sh.tokens < wire_bytes as f64 {
                pf.budget_drops.fetch_add(1, Ordering::Relaxed);
                continue;
            }
            let b = r.buckets.entry(dst).or_insert(Bucket { tokens: burst(), last: now });
            refill(b);
            let need = wire_bytes as f64 + if stream == Stream::Skel { SKEL_RESERVE } else { 0.0 };
            if b.tokens < need {
                pf.budget_drops.fetch_add(1, Ordering::Relaxed);
                continue;
            }
            b.tokens -= wire_bytes as f64;
            if let Some(sh) = r.shares.get_mut(&(dst, src)) { sh.tokens -= wire_bytes as f64; }
            let interval_ms = (1000.0 / rate * f as f64).round().clamp(1.0, u16::MAX as f64) as u16;
            out.push(Pick { dst, interval_ms });
        }
        out
    }

    /// Test/diagnostic view: the fitted factor of a pair's stream.
    #[cfg(test)]
    fn alloc_of(&self, dst: SocketAddr, src: SocketAddr, stream: Stream) -> Option<u32> {
        self.inner.lock().unwrap().alloc.get(&(dst, src, stream)).copied()
    }
}

fn measure_rates(r: &mut RelayInner, dt_s: f64) {
    for s in r.src.values_mut() {
        let inst = s.frames as f64 / dt_s.max(1e-3);
        s.rate = if s.rate == 0.0 { inst } else { s.rate * 0.6 + inst * 0.4 };
        s.frames = 0;
    }
    let dsts: Vec<SocketAddr> = r.other_bytes.keys().copied().collect();
    for d in dsts {
        let inst = r.other_bytes.insert(d, 0.0).unwrap_or(0.0) / dt_s.max(1e-3);
        let e = r.other_rate.entry(d).or_insert(inst);
        *e = *e * 0.6 + inst * 0.4;
    }
}

/// Fit each recipient's planned stream bandwidth into its budget minus the
/// measured non-stream traffic. Starts from the relevance targets, then
/// halves rates farthest-first, never below each stream's floor: skeletal
/// of the far players, then their root/weapon, then the two nearest
/// players' skeletons (never below 30 Hz), then their root.
fn allocate(r: &mut RelayInner, dsts: &[SocketAddr]) {
    let mut alloc = HashMap::new();
    for &dst in dsts {
        let other = r.other_rate.get(&dst).copied().unwrap_or(0.0);
        let target = (budget() * STREAM_SHARE - other).max(budget() * MIN_STREAM_SHARE);
        // (src, stream, rank, rate, bytes, factor, max_factor)
        let mut items: Vec<(SocketAddr, Stream, u16, f64, f64, u32, u32)> = Vec::new();
        for &src in dsts {
            if src == dst { continue; }
            let (rank, dist) = r.rank.get(&(dst, src)).copied().unwrap_or((0, 0.0));
            for stream in [Stream::Root, Stream::Weapon, Stream::Skel] {
                if let Some(s) = r.src.get(&(src, stream)) {
                    if s.rate > 0.0 {
                        let f = factor(stream, rank, dist, s.rate);
                        let cap = max_factor(s.rate, floor_hz(stream, rank)).max(f);
                        items.push((src, stream, rank, s.rate, s.bytes, f, cap));
                    }
                }
            }
        }
        let item_cost = |x: &(SocketAddr, Stream, u16, f64, f64, u32, u32)| x.3 / x.5 as f64 * x.4;
        let mut total: f64 = items.iter().map(item_cost).sum();
        let mut order: Vec<usize> = (0..items.len()).collect();
        order.sort_by(|a, b| items[*b].2.cmp(&items[*a].2));
        let passes: [(bool, bool); 4] = [(true, false), (false, false), (true, true), (false, true)];
        for (skel_pass, near_pass) in passes {
            loop {
                if total <= target { break; }
                let mut changed = false;
                for &i in &order {
                    if total <= target { break; }
                    let it = &mut items[i];
                    let stream_ok = (it.1 == Stream::Skel) == skel_pass;
                    let group_ok = (it.2 < NEAR) == near_pass;
                    if stream_ok && group_ok && it.5 < it.6 {
                        total -= item_cost(it);
                        it.5 = (it.5 * 2).min(it.6);
                        total += item_cost(it);
                        changed = true;
                    }
                }
                if !changed { break; }
            }
        }
        for (src, stream, _, _, _, f, _) in items {
            alloc.insert((dst, src, stream), f);
        }
    }
    r.alloc = alloc;
}

fn refill(b: &mut Bucket) {
    let now = Instant::now();
    let dt = now.duration_since(b.last).as_secs_f64();
    b.last = now;
    b.tokens = (b.tokens + dt * budget()).min(burst());
}

/// Rank every peer's others by distance (nearest = 0). Peers without a known
/// position rank first (full rate) until their first root arrives.
fn rerank(r: &mut RelayInner, dsts: &[SocketAddr]) {
    let all: Vec<SocketAddr> = dsts.to_vec();
    r.rank.clear();
    for &dst in &all {
        let dp = r.pos.get(&dst).copied();
        let mut others: Vec<(SocketAddr, f32)> = all.iter().filter(|a| **a != dst).map(|&a| {
            let d = match (dp, r.pos.get(&a)) {
                (Some(p), Some(q)) => ((p[0]-q[0]).powi(2) + (p[1]-q[1]).powi(2) + (p[2]-q[2]).powi(2)).sqrt(),
                _ => 0.0,
            };
            (a, d)
        }).collect();
        others.sort_by(|x, y| x.1.partial_cmp(&y.1).unwrap_or(std::cmp::Ordering::Equal));
        for (i, (a, d)) in others.into_iter().enumerate() {
            r.rank.insert((dst, a), (i as u16, d));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::proto;

    fn addrs(n: usize) -> Vec<SocketAddr> {
        (0..n).map(|i| format!("10.0.{}.{}:7000", i / 200, i % 200 + 1).parse().unwrap()).collect()
    }

    /// Feed `secs` of traffic: every peer sends root, weapon and skeletal at
    /// `hz` with realistic sizes; returns frames received per (dst, src, stream).
    fn run(relay: &Relay, addrs: &[SocketAddr], hz: u32, secs: u32) -> HashMap<(SocketAddr, SocketAddr, Stream), u32> {
        let mut got = HashMap::new();
        for _ in 0..secs * 2 {
            {
                let mut r = relay.inner.lock().unwrap();
                r.ranked_at = Instant::now().checked_sub(RANK_REFRESH + Duration::from_millis(1));
            }
            relay.replan_if_due(addrs);
            for _ in 0..hz / 2 {
                for &a in addrs {
                    for (st, b) in [(Stream::Root, 109usize), (Stream::Weapon, 104), (Stream::Skel, 392)] {
                        for p in relay.select(a, addrs, st, b) {
                            *got.entry((p.dst, a, st)).or_insert(0) += 1;
                        }
                    }
                }
            }
            // Refill the buckets as half a second of wall time would.
            let mut r = relay.inner.lock().unwrap();
            for b in r.buckets.values_mut() { b.tokens = burst(); }
            for b in r.shares.values_mut() { b.last -= Duration::from_millis(500); }
        }
        got
    }

    #[test]
    fn seal_overhead_matches_a_real_v5_datagram() {
        use hsmp_net::net::{Conn, ConnConfig, SendMode, Side};
        let r = hsmp_ipc::schema::pose::Root { tick: 99, ts: 1, send_wall_ms: 2, pos: [1.0, 2.0, 3.0], rot: [0.0, 0.0, 0.0, 1.0], vel: [0.5, 0.0, 0.0] };
        let bytes = hsmp_ipc::wire::encode(0, 4, &r, &[]);
        let mode = proto::record_mode(hsmp_ipc::schema::pose::K_ROOT, 4).unwrap();
        let mut c = Conn::new(Side::Server, 9, [1; 32], [2; 32], 0, ConnConfig::default());
        c.send(mode, bytes.clone()).unwrap();
        assert!(matches!(mode, SendMode::Latest { .. }));
        let dg = c.poll_transmit(0).unwrap();
        assert_eq!(dg.len(), bytes.len() + SEAL_OVERHEAD);
    }

    #[test]
    fn far_peers_are_thinned_and_budget_caps() {
        let relay = Relay::default();
        let addrs: Vec<SocketAddr> = (0..10).map(|i| format!("127.0.0.1:{}", 1000 + i).parse().unwrap()).collect();
        for (i, a) in addrs.iter().enumerate() { relay.update_pos(*a, [i as f32 * 1000.0, 0.0, 0.0]); }
        relay.replan_if_due(&addrs);
        // Peer 0 sends 40 skeletal frames (60 Hz assumed); peer 9 (rank 8,
        // 9000 uu away) gets the far rate, 15 Hz: 1 in 4.
        let mut got_far = 0;
        for _ in 0..40 {
            if relay.select(addrs[0], &addrs, Stream::Skel, 100).iter().any(|p| p.dst == addrs[9]) { got_far += 1; }
        }
        assert_eq!(got_far, 10);
        // A 1 KB frame stream to one peer is capped by the bucket.
        let relay = Relay::default();
        let two = &addrs[..2];
        let sent = (0..200).filter(|_| relay.select(two[0], two, Stream::Root, 1024).len() == 1).count();
        assert!(sent < 60, "burst cap should stop an instant flood, sent {}", sent);
    }

    /// With 60 Hz senders the two nearest players' skeletons stay
    /// ≥ 30 Hz at 8 players, weapon drops to its diagnostic rate, and the
    /// interval each pick carries matches the plan.
    #[test]
    fn nearest_two_skeletons_never_below_30hz_at_8_players() {
        let relay = Relay::default();
        let a = addrs(8);
        for (i, x) in a.iter().enumerate() { relay.update_pos(*x, [i as f32 * 150.0, 0.0, 0.0]); }
        let got = run(&relay, &a, 60, 6);
        let secs = 6.0;
        for &dst in &a {
            let mut ranked: Vec<(u16, SocketAddr)> = a.iter().filter(|s| **s != dst)
                .map(|s| (relay.inner.lock().unwrap().rank[&(dst, *s)].0, *s)).collect();
            ranked.sort();
            for (rank, src) in ranked {
                let skel = got.get(&(dst, src, Stream::Skel)).copied().unwrap_or(0) as f64 / secs;
                let weapon = got.get(&(dst, src, Stream::Weapon)).copied().unwrap_or(0) as f64 / secs;
                if rank < NEAR { assert!(skel >= 29.0, "rank {rank}: {skel:.1} Hz"); }
                assert!(skel >= SKEL_FLOOR_HZ - 0.5, "rank {rank}: {skel:.1} Hz");
                assert!(weapon <= 6.0, "weapon {weapon:.1} Hz");
            }
        }
        // Interval: 60 Hz sender, factor f -> f * 16.7 ms.
        let p = relay.select(a[0], &a, Stream::Skel, 392);
        for pick in p {
            let f = relay.alloc_of(pick.dst, a[0], Stream::Skel).unwrap();
            assert!((pick.interval_ms as f64 - 1000.0 / 60.0 * f as f64).abs() <= 2.0, "{pick:?} f={f}");
        }
    }

    /// The first frame of a stream is never thinned (a rarely updated stream
    /// such as a diagnostic weapon file must not lose its only frame).
    #[test]
    fn first_frame_of_a_stream_always_goes() {
        let relay = Relay::default();
        let a = addrs(2);
        relay.replan_if_due(&a);
        for st in [Stream::Root, Stream::Weapon, Stream::Skel] {
            assert_eq!(relay.select(a[0], &a, st, 100).len(), 1, "{st:?}");
        }
    }

    /// A huge non-stream burst (world sync) cannot drive the
    /// bucket below −burst, so streams resume within a short time.
    #[test]
    fn charge_is_floored() {
        let relay = Relay::default();
        let a = addrs(2);
        relay.charge(a[1], 10 * 1024 * 1024);
        let t = relay.inner.lock().unwrap().buckets[&a[1]].tokens;
        assert!(t >= -burst() - 1.0, "{t}");
    }

    /// Measured non-stream traffic comes off the stream plan.
    #[test]
    fn non_stream_traffic_is_planned_for() {
        let relay = Relay::default();
        let a = addrs(8);
        for (i, x) in a.iter().enumerate() { relay.update_pos(*x, [i as f32 * 150.0, 0.0, 0.0]); }
        let _ = run(&relay, &a, 60, 2);
        let before: u32 = a[1..].iter().map(|s| relay.alloc_of(a[0], *s, Stream::Skel).unwrap()).sum();
        // 40 KB/s of world traffic to a[0].
        for _ in 0..4 {
            relay.charge(a[0], 20 * 1024);
            let _ = run(&relay, &a, 60, 1);
            relay.charge(a[0], 20 * 1024);
        }
        let after: u32 = a[1..].iter().map(|s| relay.alloc_of(a[0], *s, Stream::Skel).unwrap()).sum();
        assert!(after > before, "plan thinned for the measured world traffic ({before} -> {after})");
    }

    /// One sender flooding cannot take more than ~1.5x its fair
    /// share of a recipient's budget.
    #[test]
    fn a_flooding_sender_gets_only_its_fair_share() {
        let relay = Relay::default();
        let a = addrs(4);
        relay.replan_if_due(&a);
        // 1000 frames of 1 KB from a[1] to a[0] in an instant.
        let n = (0..1000).filter(|_| relay.select(a[1], &a, Stream::Root, 1024).iter().any(|p| p.dst == a[0])).count();
        let share_burst = budget() * FAIR_SHARE_X / 3.0 * 0.375;
        assert!((n * 1024) as f64 <= share_burst + 1024.0, "{n} KB");
    }

    /// The bandwidth plan at 64 players (every recipient over
    /// budget, so every pass runs) is cheap and runs off the receive path.
    #[test]
    fn replan_at_64_players_is_fast_and_fits_the_budget() {
        let relay = Relay::default();
        let addrs = addrs(64);
        for (i, a) in addrs.iter().enumerate() {
            relay.update_pos(*a, [(i % 8) as f32 * 300.0, (i / 8) as f32 * 300.0, 0.0]);
        }
        relay.replan_if_due(&addrs);
        {
            let mut r = relay.inner.lock().unwrap();
            for &a in &addrs {
                for (st, n, b) in [(Stream::Root, 30u32, 109.0), (Stream::Weapon, 30, 104.0), (Stream::Skel, 30, 392.0)] {
                    let s = r.src.entry((a, st)).or_default();
                    s.frames = n;
                    s.bytes = b;
                }
            }
            r.ranked_at = Instant::now().checked_sub(RANK_REFRESH + Duration::from_millis(1));
        }
        let t0 = Instant::now();
        relay.replan_if_due(&addrs);
        let took = t0.elapsed();
        let r = relay.inner.lock().unwrap();
        assert_eq!(r.alloc.len(), 64 * 63 * 3);
        let target = budget() * STREAM_SHARE;
        for &dst in &addrs {
            let cost: f64 = addrs.iter().filter(|s| **s != dst).flat_map(|&src| {
                [Stream::Root, Stream::Weapon, Stream::Skel].map(|st| {
                    let s = r.src[&(src, st)];
                    s.rate / r.alloc[&(dst, src, st)] as f64 * s.bytes
                })
            }).sum();
            let capped = addrs.iter().filter(|s| **s != dst).all(|&src| {
                let (rank, _) = r.rank[&(dst, src)];
                let rate = r.src[&(src, Stream::Skel)].rate;
                r.alloc[&(dst, src, Stream::Skel)] >= max_factor(rate, floor_hz(Stream::Skel, rank))
            });
            assert!(cost <= target + 1.0 || capped, "{dst}: {cost:.0} B/s > {target:.0}");
        }
        println!("replan at 64 players: {:?}", took);
        assert!(took < Duration::from_millis(250), "replan took {took:?}");
    }
}
