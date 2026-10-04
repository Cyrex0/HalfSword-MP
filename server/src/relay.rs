//! Hot-path relay for high-rate streams (root, skeletal, weapon).
//!
//! * Encode-once fan-out — the record message is built once and handed to
//!   the v5 transport per recipient (`crate::net`).
//! * Rate plan per (recipient, sender, stream): every stream has a
//!   target rate by relevance (the two nearest players' skeletons at the
//!   sender's full rate, never below 30 Hz; root 30/20 Hz; weapon 5 Hz, it
//!   has no in-game reader) and the plan is fitted into the recipient's
//!   budget farthest-first, never below each stream's floor. The two nearest
//!   are never far-thinned, a slow sender is never thinned below a floor, and
//!   a recipient with no body in play (spectator, dead) ranks everyone
//!   nearest. The bucket keeps room for the nearest players' floors even
//!   under a congestion cut.
//! * Per-recipient bandwidth budget — token bucket over everything sent to a
//!   peer; stream frames are dropped (never reliable events) when it runs dry,
//!   skeletal before root. The bucket never goes below −burst, the
//!   measured non-stream downstream (events, world, loadouts) is taken off
//!   the stream plan, and one sender gets at most 1.5× its fair share of
//!   a recipient's budget.
//! * Each forwarded skeletal frame tells the recipient its current relay
//!   interval (the pose record's `aux`, caps::POSE_RATE) and tells lag
//!   comp which frames the recipient was actually sent (`note_relayed`).
//! * Path congestion per recipient: when the connection to a recipient shows
//!   a standing queue (smoothed RTT well above its minimum for two plans in a
//!   row; a jitter spike is not one), or heavy loss together with some queue,
//!   that recipient's budget shrinks (x0.7, at most once a second, never
//!   below a quarter) and grows back by 5 % per plan once the path is clear.
//!   A listen host on a 2-5 Mbit/s home upload fills its own uplink long
//!   before the fixed budget is reached; this keeps the router queue, and
//!   everyone's latency, short.

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
#[cfg(test)]
fn burst() -> f64 { budget() * BURST_S }
/// Bucket depth in seconds of the rate.
const BURST_S: f64 = 0.375;
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
    /// Congestion state of the path to each recipient.
    paths: HashMap<SocketAddr, PathCtl>,
    /// Recipients with no body in play (spectators, the dead): every player
    /// is relevant to them, wherever their parked body is.
    free: std::collections::HashSet<SocketAddr>,
    /// Bandwidth the two nearest players' streams need at their floors
    /// (bytes/s): a congestion cut never takes a budget below it.
    floor_bps: HashMap<SocketAddr, f64>,
    /// Per sender since the last report: pose frames in, and refused before
    /// the relay (rate gate, lag comp); skeletal frames relayed per pair.
    diag_in: HashMap<SocketAddr, (u32, u32)>,
    diag_out: HashMap<(SocketAddr, SocketAddr), u32>,
}

/// Transport counters of the connection to one recipient (cumulative).
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct PathSample {
    pub pkts_sent: u64,
    pub pkts_lost: u64,
    pub srtt_ms: f64,
    /// Lowest RTT sample seen (0 = none yet).
    pub min_rtt_ms: f64,
}

/// Per-recipient budget scale, driven by queueing delay and loss.
#[derive(Debug, Clone, Copy)]
struct PathCtl {
    scale: f64,
    sent: u64,
    lost: u64,
    /// RTT without queueing: the connection's minimum, drifting up slowly so
    /// a route change to a longer path is not read as a queue forever.
    base_ms: f64,
    last_cut: Option<Instant>,
    /// Consecutive plans that saw congestion.
    over: u8,
}

/// A standing queue this deep (smoothed RTT over the base) is congestion.
const QUEUE_CONGESTED_MS: f64 = 120.0;
/// Below this queue (and loss) the budget grows back.
const QUEUE_CLEAR_MS: f64 = 60.0;
/// Loss counts only together with some queue: random Wi-Fi loss is not
/// helped by sending less.
const LOSS_CONGESTED: f64 = 0.15;
const LOSS_QUEUE_MS: f64 = 30.0;
const LOSS_CLEAR: f64 = 0.03;
/// Fewer packets than this in one plan period say nothing about loss.
const LOSS_MIN_PKTS: u64 = 20;
const SCALE_CUT: f64 = 0.7;
const SCALE_STEP: f64 = 0.05;
const SCALE_MIN: f64 = 0.25;
const CUT_EVERY: Duration = Duration::from_millis(1000);
/// Plans in a row that must see the queue before the first cut.
const CONGESTED_PLANS: u8 = 2;
/// Share of the gap by which the base RTT drifts up per plan (about half
/// way in 35 s).
const BASE_DRIFT: f64 = 0.01;

impl PathCtl {
    fn new(s: &PathSample) -> Self {
        PathCtl { scale: 1.0, sent: s.pkts_sent, lost: s.pkts_lost, base_ms: 0.0, last_cut: None, over: 0 }
    }

    /// One plan period's counters: cut on congestion, else recover.
    fn update(&mut self, s: &PathSample, now: Instant) {
        let sent = s.pkts_sent.saturating_sub(self.sent);
        let lost = s.pkts_lost.saturating_sub(self.lost);
        self.sent = s.pkts_sent;
        self.lost = s.pkts_lost;
        if s.srtt_ms <= 0.0 {
            return;
        }
        let floor = if s.min_rtt_ms > 0.0 { s.min_rtt_ms } else { s.srtt_ms };
        self.base_ms = if self.base_ms <= 0.0 {
            floor
        } else {
            let b = self.base_ms.min(s.srtt_ms);
            (b + (s.srtt_ms - b) * BASE_DRIFT).max(floor)
        };
        let queue = (s.srtt_ms - self.base_ms).max(0.0);
        let loss = if sent >= LOSS_MIN_PKTS { lost as f64 / sent as f64 } else { 0.0 };
        let congested = queue > QUEUE_CONGESTED_MS || (loss > LOSS_CONGESTED && queue > LOSS_QUEUE_MS);
        // A queue must stand for two plans (about a second): a jitter spike
        // (Wi-Fi, a long jittery route) decays within one, a full uplink does not.
        self.over = if congested { self.over.saturating_add(1) } else { 0 };
        if congested {
            if self.over >= CONGESTED_PLANS && self.last_cut.is_none_or(|t| now.duration_since(t) >= CUT_EVERY) {
                self.scale = (self.scale * SCALE_CUT).max(SCALE_MIN);
                self.last_cut = Some(now);
            }
        } else if queue < QUEUE_CLEAR_MS && loss < LOSS_CLEAR {
            self.scale = (self.scale + SCALE_STEP).min(1.0);
        }
    }
}

/// The sustained budget of `dst` (bytes/s): the configured one, scaled down
/// while its path is congested.
fn budget_of(r: &RelayInner, dst: &SocketAddr) -> f64 {
    scaled_budget(r, dst).max(r.floor_bps.get(dst).copied().unwrap_or(0.0))
}

/// The configured budget times the path's congestion scale.
fn scaled_budget(r: &RelayInner, dst: &SocketAddr) -> f64 {
    budget() * r.paths.get(dst).map_or(1.0, |p| p.scale)
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
        Stream::Root => if rank < NEAR { 30.0 } else { 20.0 },
        // No in-game reader (`.weapon_remote`): diagnostics only.
        Stream::Weapon => 5.0,
        // The two nearest are who you fight or watch: never far-thinned
        // (a spectator's parked body can be anywhere).
        Stream::Skel => {
            if rank < NEAR { f64::INFINITY } else if far { 15.0 } else if rank < 4 { 30.0 } else { 15.0 }
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
    (rate_hz / hz - FACTOR_SLACK).ceil().clamp(1.0, 64.0) as u32
}

/// Measurement noise a factor ignores: a sender measured at 61 Hz aimed at
/// 30 Hz is 1 in 2 (not 1 in 3, 20 Hz), one at 59.8 Hz keeps a 30 Hz floor
/// at 1 in 2.
const FACTOR_SLACK: f64 = 0.15;

/// Largest factor that keeps a `rate_hz` stream at or above `floor` Hz.
fn max_factor(rate_hz: f64, floor: f64) -> u32 {
    if !floor.is_finite() { return 1; }
    ((rate_hz / floor + FACTOR_SLACK).floor() as u32).clamp(1, 64)
}

/// The unfitted factor of a stream. A slow sender (a low frame rate) is
/// never thinned below the stream's floor.
fn factor(stream: Stream, rank: u16, dist: f32, rate_hz: f64) -> u32 {
    let rate = if rate_hz > 0.0 { rate_hz } else { DEFAULT_RATE_HZ };
    factor_for(rate, target_hz(stream, rank, dist)).min(max_factor(rate, floor_hz(stream, rank)))
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

    /// The recipients with no body in play this round (spectators, the dead).
    pub fn set_free_viewers(&self, free: impl IntoIterator<Item = SocketAddr>) {
        self.inner.lock().unwrap().free = free.into_iter().collect();
    }

    /// A pose frame from `src` arrived; `relayed` = it passed the rate gate
    /// and lag comp and went to the rate plan.
    pub fn note_pose_in(&self, src: SocketAddr, relayed: bool) {
        let mut r = self.inner.lock().unwrap();
        let e = r.diag_in.entry(src).or_insert((0, 0));
        e.0 += 1;
        if !relayed { e.1 += 1; }
    }

    /// One line per pose sender since the last call: frames in and refused,
    /// the measured rate the plan uses, and per recipient the frames relayed,
    /// the planned factor and the pair's rank.
    pub fn pose_report(&self, secs: f64) -> Vec<String> {
        let mut r = self.inner.lock().unwrap();
        let ids = r.ids.clone();
        let name = |a: &SocketAddr| ids.get(a).map_or_else(|| a.to_string(), |id| format!("peer {id}"));
        let secs = secs.max(1e-3);
        let din = std::mem::take(&mut r.diag_in);
        let dout = std::mem::take(&mut r.diag_out);
        let mut srcs: Vec<SocketAddr> = din.keys().copied().collect();
        srcs.sort();
        srcs.into_iter().map(|src| {
            let (n, refused) = din[&src];
            let rate = r.src.get(&(src, Stream::Skel)).map_or(0.0, |s| s.rate);
            let mut to: Vec<String> = dout.iter().filter(|((_, s), _)| *s == src).map(|((d, _), k)| {
                let f = r.alloc.get(&(*d, src, Stream::Skel)).copied().unwrap_or(0);
                let (rank, dist) = r.rank.get(&(*d, src)).copied().unwrap_or((0, 0.0));
                let free = if r.free.contains(d) { " free" } else { "" };
                format!("{} {:.0} Hz f={f} rank={rank} {dist:.0}uu{free}", name(d), *k as f64 / secs)
            }).collect();
            to.sort();
            format!("pose relay {}: in {:.1} Hz, refused {:.1} Hz (rate gate / lag comp), planned rate {rate:.1} Hz | {}",
                name(&src), n as f64 / secs, refused as f64 / secs, to.join(", "))
        }).collect()
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
        let rate = budget_of(&r, &dst);
        let b = r.buckets.entry(dst).or_insert(Bucket { tokens: rate * BURST_S, last: Instant::now() });
        refill(b, rate);
        b.tokens = (b.tokens - bytes as f64).max(-rate * BURST_S);
    }

    pub fn forget(&self, addr: &SocketAddr) {
        let mut r = self.inner.lock().unwrap();
        r.pos.remove(addr);
        r.buckets.remove(addr);
        r.other_bytes.remove(addr);
        r.other_rate.remove(addr);
        r.ids.remove(addr);
        r.paths.remove(addr);
        r.rank.retain(|(d, s), _| d != addr && s != addr);
        r.shares.retain(|(d, s), _| d != addr && s != addr);
        r.counters.retain(|(d, s, _), _| d != addr && s != addr);
        r.alloc.retain(|(d, s, _), _| d != addr && s != addr);
        r.src.retain(|(s, _), _| s != addr);
        r.free.remove(addr);
        r.floor_bps.remove(addr);
        r.diag_in.remove(addr);
        r.diag_out.retain(|(d, s), _| d != addr && s != addr);
    }

    /// Every RANK_REFRESH: re-rank everyone by distance, measure the source
    /// stream rates and fit each recipient's plan into its budget. Runs on the
    /// server tick, off the receive loop. O(N² log N) per call.
    #[cfg(test)]
    pub fn replan_if_due(&self, dsts: &[SocketAddr]) {
        self.replan_with_paths(dsts, Vec::new);
    }

    /// `replan_if_due` fed with the transport counters of each recipient's
    /// connection (`paths` runs only when a plan is due).
    pub fn replan_with_paths(&self, dsts: &[SocketAddr], paths: impl FnOnce() -> Vec<(SocketAddr, PathSample)>) {
        let now = Instant::now();
        let due = |r: &RelayInner| r.ranked_at.map_or(true, |p| now.duration_since(p) > RANK_REFRESH);
        if !due(&self.inner.lock().unwrap()) { return; }
        // Read outside our lock: the transport has locks of its own.
        let samples = paths();
        let mut r = self.inner.lock().unwrap();
        if !due(&r) { return; }
        for (a, s) in &samples {
            match r.paths.get_mut(a) {
                Some(p) => p.update(s, now),
                None => { r.paths.insert(*a, PathCtl::new(s)); }
            }
        }
        if !samples.is_empty() {
            r.paths.retain(|a, _| samples.iter().any(|(b, _)| b == a));
        }
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
            let rate_dst = budget_of(&r, &dst);
            let share_rate = rate_dst * FAIR_SHARE_X / r.srcs_n.max(1) as f64;
            let sh = r.shares.entry((dst, src)).or_insert(Bucket { tokens: share_rate * 0.375, last: now });
            let dt = now.duration_since(sh.last).as_secs_f64();
            sh.last = now;
            sh.tokens = (sh.tokens + dt * share_rate).min(share_rate * 0.375);
            if sh.tokens < wire_bytes as f64 {
                pf.budget_drops.fetch_add(1, Ordering::Relaxed);
                continue;
            }
            let b = r.buckets.entry(dst).or_insert(Bucket { tokens: rate_dst * BURST_S, last: now });
            refill(b, rate_dst);
            let need = wire_bytes as f64 + if stream == Stream::Skel { SKEL_RESERVE } else { 0.0 };
            if b.tokens < need {
                pf.budget_drops.fetch_add(1, Ordering::Relaxed);
                continue;
            }
            b.tokens -= wire_bytes as f64;
            if let Some(sh) = r.shares.get_mut(&(dst, src)) { sh.tokens -= wire_bytes as f64; }
            let interval_ms = (1000.0 / rate * f as f64).round().clamp(1.0, u16::MAX as f64) as u16;
            if stream == Stream::Skel { *r.diag_out.entry((dst, src)).or_insert(0) += 1; }
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
    let mut floor_bps = HashMap::new();
    for &dst in dsts {
        let other = r.other_rate.get(&dst).copied().unwrap_or(0.0);
        let b = scaled_budget(r, &dst);
        let target = (b * STREAM_SHARE - other).max(b * MIN_STREAM_SHARE);
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
        // The nearest players' root and skeleton at their floors: the bucket
        // keeps room for them even under a congestion cut, or it would drop
        // frames the plan (and the interval each frame carries) promised.
        let near_floor: f64 = items.iter().filter(|x| x.2 < NEAR && x.1 != Stream::Weapon).map(|x| x.3 / x.6 as f64 * x.4).sum();
        if near_floor > 0.0 {
            floor_bps.insert(dst, ((near_floor + SKEL_RESERVE) / STREAM_SHARE + other).min(budget()));
        }
        for (src, stream, _, _, _, f, _) in items {
            alloc.insert((dst, src, stream), f);
        }
    }
    r.alloc = alloc;
    r.floor_bps = floor_bps;
}

fn refill(b: &mut Bucket, rate: f64) {
    let now = Instant::now();
    let dt = now.duration_since(b.last).as_secs_f64();
    b.last = now;
    b.tokens = (b.tokens + dt * rate).min(rate * BURST_S);
}

/// Rank every peer's others by distance (nearest = 0). Peers without a known
/// position rank first (full rate) until their first root arrives.
fn rerank(r: &mut RelayInner, dsts: &[SocketAddr]) {
    let all: Vec<SocketAddr> = dsts.to_vec();
    r.rank.clear();
    for &dst in &all {
        // A recipient with no body in play (a spectator watching anyone, the
        // dead) ranks every player nearest: its parked body says nothing.
        let free = r.free.contains(&dst);
        let dp = if free { None } else { r.pos.get(&dst).copied() };
        let mut others: Vec<(SocketAddr, f32)> = all.iter().filter(|a| **a != dst).map(|&a| {
            let d = match (dp, r.pos.get(&a)) {
                (Some(p), Some(q)) => ((p[0]-q[0]).powi(2) + (p[1]-q[1]).powi(2) + (p[2]-q[2]).powi(2)).sqrt(),
                _ => 0.0,
            };
            (a, d)
        }).collect();
        others.sort_by(|x, y| x.1.partial_cmp(&y.1).unwrap_or(std::cmp::Ordering::Equal));
        for (i, (a, d)) in others.into_iter().enumerate() {
            r.rank.insert((dst, a), (if free { 0 } else { i as u16 }, d));
        }
    }
}

impl Relay {
    /// Diagnostics (the 10 s stats line): the budget the plan gives `dst` now, in bytes/s
    /// (congestion scale applied).
    pub fn budget_bps(&self, dst: &SocketAddr) -> f64 {
        budget_of(&self.inner.lock().unwrap(), dst)
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

    fn sample(sent: u64, lost: u64, srtt: f64, min: f64) -> PathSample {
        PathSample { pkts_sent: sent, pkts_lost: lost, srtt_ms: srtt, min_rtt_ms: min }
    }

    /// A standing queue on the path (the host's upload is full) cuts that
    /// recipient's budget to the floor within a few seconds, and it grows
    /// back once the queue drains.
    #[test]
    fn a_queue_on_the_path_shrinks_the_budget_until_it_drains() {
        let t0 = Instant::now();
        let mut c = PathCtl::new(&sample(0, 0, 0.0, 0.0));
        c.update(&sample(100, 0, 60.0, 50.0), t0);
        assert_eq!(c.scale, 1.0);
        let mut sent = 100;
        for i in 1..=8u64 {
            sent += 100;
            c.update(&sample(sent, 0, 400.0, 50.0), t0 + Duration::from_millis(500 * i));
        }
        assert!(c.scale <= 0.26, "cut to the floor: {}", c.scale);
        assert!(c.scale >= SCALE_MIN);
        for i in 9..=40u64 {
            sent += 100;
            c.update(&sample(sent, 0, 55.0, 50.0), t0 + Duration::from_millis(500 * i));
        }
        assert_eq!(c.scale, 1.0, "recovered");
    }

    /// Random loss with no queue (Wi-Fi) leaves the budget alone; heavy loss
    /// together with a queue is congestion.
    #[test]
    fn random_loss_alone_is_not_congestion() {
        let t0 = Instant::now();
        let mut c = PathCtl::new(&sample(0, 0, 0.0, 0.0));
        for i in 1..=10u64 {
            c.update(&sample(i * 100, i * 25, 52.0, 50.0), t0 + Duration::from_secs(i));
        }
        assert_eq!(c.scale, 1.0);
        let mut c = PathCtl::new(&sample(0, 0, 0.0, 0.0));
        c.update(&sample(100, 0, 50.0, 50.0), t0);
        c.update(&sample(200, 25, 100.0, 50.0), t0 + Duration::from_secs(1));
        assert_eq!(c.scale, 1.0, "one plan is not a standing queue");
        c.update(&sample(300, 50, 100.0, 50.0), t0 + Duration::from_millis(1500));
        assert!(c.scale < 1.0, "loss with a queue: {}", c.scale);
    }

    /// A longer route (a higher RTT that stays) is not a queue forever: the
    /// base follows it and the budget comes back.
    #[test]
    fn a_route_change_is_not_read_as_a_queue_forever() {
        let t0 = Instant::now();
        let mut c = PathCtl::new(&sample(0, 0, 0.0, 0.0));
        c.update(&sample(100, 0, 40.0, 40.0), t0);
        let mut last = 0.0;
        for i in 1..=400u64 {
            c.update(&sample(100 + i * 100, 0, 200.0, 40.0), t0 + Duration::from_millis(500 * i));
            last = c.scale;
        }
        assert_eq!(last, 1.0, "base drifted to the new route");
    }

    /// End to end through the relay: the recipient whose path is congested
    /// gets fewer stream bytes, the one on a clear path is unaffected.
    #[test]
    fn a_congested_recipient_is_sent_less() {
        let relay = Relay::default();
        let a = addrs(4);
        let mut sent = 0u64;
        let mut got: HashMap<SocketAddr, usize> = HashMap::new();
        for step in 0..16 {
            sent += 200;
            {
                let mut r = relay.inner.lock().unwrap();
                r.ranked_at = Instant::now().checked_sub(RANK_REFRESH + Duration::from_millis(1));
                for p in r.paths.values_mut() { p.last_cut = None; }
            }
            let srtt0 = if step == 0 { 50.0 } else { 400.0 };
            let paths: Vec<(SocketAddr, PathSample)> = a.iter().enumerate()
                .map(|(i, x)| (*x, sample(sent, 0, if i == 0 { srtt0 } else { 50.0 }, 50.0))).collect();
            relay.replan_with_paths(&a, || paths);
            {
                let mut r = relay.inner.lock().unwrap();
                for b in r.buckets.values_mut() { b.last -= Duration::from_millis(500); }
                for b in r.shares.values_mut() { b.last -= Duration::from_millis(500); }
            }
            for _ in 0..30 {
                for &src in &a {
                    for p in relay.select(src, &a, Stream::Skel, 392) {
                        if step >= 8 { *got.entry(p.dst).or_insert(0) += 392; }
                    }
                }
            }
        }
        let congested = got.get(&a[0]).copied().unwrap_or(0);
        let clear = got.get(&a[1]).copied().unwrap_or(0);
        assert!(congested * 2 < clear, "congested {congested} B vs clear {clear} B");
        relay.forget(&a[0]);
        assert!(!relay.inner.lock().unwrap().paths.contains_key(&a[0]));
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

    /// Skeletal frames per second `dst` got from `src` over a `run`.
    fn skel_hz(got: &HashMap<(SocketAddr, SocketAddr, Stream), u32>, dst: SocketAddr, src: SocketAddr, secs: f64) -> f64 {
        got.get(&(dst, src, Stream::Skel)).copied().unwrap_or(0) as f64 / secs
    }

    /// The Rhinus join (beta.2): a late joiner spectates with its own body
    /// parked far from the fight. Its two nearest players are never
    /// far-thinned, and as a free viewer it gets every player at the
    /// sender's full rate, with the interval its frames carry saying so.
    #[test]
    fn a_spectator_with_a_far_parked_body_gets_the_fight_at_full_rate() {
        let a = addrs(4);
        let place = |relay: &Relay| {
            relay.update_pos(a[0], [40_000.0, 0.0, 0.0]); // the spectator's parked body
            for (i, x) in a[1..].iter().enumerate() { relay.update_pos(*x, [i as f32 * 300.0, 0.0, 0.0]); }
        };
        // Not marked free: the two nearest at full rate although far.
        let relay = Relay::default();
        place(&relay);
        let got = run(&relay, &a, 60, 6);
        let mut hz: Vec<f64> = a[1..].iter().map(|s| skel_hz(&got, a[0], *s, 6.0)).collect();
        hz.sort_by(|x, y| y.partial_cmp(x).unwrap());
        assert!(hz[0] >= 55.0 && hz[1] >= 55.0, "nearest two of a far viewer: {hz:?}");
        // Marked free (no body in play): everyone at full rate.
        let relay = Relay::default();
        place(&relay);
        relay.set_free_viewers([a[0]]);
        let got = run(&relay, &a, 60, 6);
        for s in &a[1..] {
            let h = skel_hz(&got, a[0], *s, 6.0);
            assert!(h >= 55.0, "free viewer gets {s} at {h:.1} Hz");
        }
        for p in relay.select(a[1], &a, Stream::Skel, 392).into_iter().filter(|p| p.dst == a[0]) {
            assert!(p.interval_ms <= 20, "{p:?}");
        }
        // The fighters still rank each other by distance.
        let r = relay.inner.lock().unwrap();
        assert_eq!(r.rank[&(a[1], a[2])].0, 0);
        assert_eq!(r.rank[&(a[1], a[0])].0, 2, "the parked spectator is far for the fighters");
    }

    /// A slow sender (a listen host at 16 fps) is never thinned below the
    /// floor: near it goes at its own rate, far it keeps 7.5 Hz.
    #[test]
    fn a_slow_sender_is_not_thinned_below_the_floor() {
        for rank in [0u16, 1, 2, 5] {
            for dist in [100.0f32, 9000.0] {
                for rate in [8.0, 16.0, 24.0, 30.0, 45.0, 60.0, 61.0, 120.0] {
                    let f = factor(Stream::Skel, rank, dist, rate);
                    let hz = rate / f as f64;
                    let floor = floor_hz(Stream::Skel, rank).min(rate);
                    assert!(hz >= floor - 0.5, "rank {rank} {dist}uu {rate} Hz: f={f} -> {hz:.1} Hz < {floor}");
                    if rank < NEAR { assert_eq!(f, 1, "nearest at the sender's rate ({rate} Hz, {dist}uu)"); }
                }
            }
        }
        // Rate-measurement noise does not cost a whole step.
        assert_eq!(factor_for(61.0, 30.0), 2);
        assert_eq!(factor_for(60.0, 15.0), 4);
        assert_eq!(max_factor(59.8, 30.0), 2);
    }

    /// What a transport reports over `plans` plan periods of 500 ms: one RTT
    /// sample per 16 ms frame, 1/8 smoothing and a running minimum;
    /// `rtt(t_ms, seed)` is the path's RTT at time t.
    fn path_samples(plans: u64, rtt: impl Fn(u64, &mut u64) -> f64) -> Vec<PathSample> {
        let (mut srtt, mut min, mut seed, mut out) = (0.0f64, f64::INFINITY, 0x9e37_79b9_7f4a_7c15u64, vec![]);
        for p in 0..plans {
            for k in 0..31u64 {
                let s = rtt(p * 500 + k * 16, &mut seed);
                srtt = if srtt == 0.0 { s } else { srtt + (s - srtt) / 8.0 };
                min = min.min(s);
            }
            out.push(sample((p + 1) * 31, 0, srtt, min));
        }
        out
    }

    fn rnd(seed: &mut u64) -> f64 {
        *seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        (*seed >> 11) as f64 / (1u64 << 53) as f64
    }

    /// A healthy but long, jittery internet path (the ~179 ms route to the
    /// Rhinus host: 60 ms of jitter, a 300 ms spike every 7 s, no loss) is not
    /// congestion: the budget stays whole and two 60 Hz players reach the
    /// recipient at full rate.
    #[test]
    fn a_long_jittery_path_without_loss_keeps_the_full_rate() {
        let samples = path_samples(240, |t, s| {
            let spike = if t % 7000 >= 6700 { 250.0 } else { 0.0 };
            150.0 + 60.0 * rnd(s) + spike
        });
        let t0 = Instant::now();
        let mut c = PathCtl::new(&sample(0, 0, 0.0, 0.0));
        let mut lowest: f64 = 1.0;
        for (i, s) in samples.iter().enumerate() {
            c.update(s, t0 + Duration::from_millis(500 * i as u64));
            lowest = lowest.min(c.scale);
        }
        assert_eq!(lowest, 1.0, "jitter read as a queue");
        // Through the relay: 3 peers, the recipient on that path.
        let relay = Relay::default();
        let a = addrs(3);
        for (i, x) in a.iter().enumerate() { relay.update_pos(*x, [i as f32 * 200.0, 0.0, 0.0]); }
        let mut got: HashMap<SocketAddr, u32> = HashMap::new();
        for (i, s) in samples.iter().take(60).enumerate() {
            {
                let mut r = relay.inner.lock().unwrap();
                r.ranked_at = Instant::now().checked_sub(RANK_REFRESH + Duration::from_millis(1));
                for b in r.buckets.values_mut() { b.last -= Duration::from_millis(500); }
                for b in r.shares.values_mut() { b.last -= Duration::from_millis(500); }
            }
            let clear = sample(s.pkts_sent, 0, 40.0, 40.0);
            let paths = vec![(a[0], *s), (a[1], clear), (a[2], clear)];
            relay.replan_with_paths(&a, || paths);
            for _ in 0..30 {
                for &src in &a[1..] {
                    for p in relay.select(src, &a, Stream::Skel, 700) {
                        if p.dst == a[0] && i >= 10 { *got.entry(src).or_insert(0) += 1; }
                    }
                }
            }
        }
        for src in &a[1..] {
            let hz = got.get(src).copied().unwrap_or(0) as f64 / 25.0;
            assert!(hz >= 58.0, "{src}: {hz:.1} Hz on a healthy 179 ms path");
        }
    }

    /// The far path (~300 ms RTT, 50 ms of jitter each way): the RTT minimum sits
    /// ~100 ms under the mean, which is jitter, not a queue.
    #[test]
    fn a_far_jittery_path_is_not_a_queue() {
        // netsim far: AR(1) jitter per direction (tau 50 ms, sd 50/sqrt 3, clipped to 50)
        let st = std::cell::Cell::new((0.0f64, 0.0f64));
        let samples = path_samples(240, |_, s| {
            let rho = (-16.0f64 / 50.0).exp();
            let g = |s: &mut u64| (-2.0 * rnd(s).max(1e-12).ln()).sqrt() * (std::f64::consts::TAU * rnd(s)).cos();
            let (a, b) = st.get();
            let (a, b) = (rho * a + (1.0 - rho * rho).sqrt() * g(s), rho * b + (1.0 - rho * rho).sqrt() * g(s));
            st.set((a, b));
            let j = |x: f64| (x * 50.0 / 3f64.sqrt()).clamp(-50.0, 50.0);
            300.0 + j(a) + j(b)
        });
        let t0 = Instant::now();
        let mut c = PathCtl::new(&sample(0, 0, 0.0, 0.0));
        let mut lowest: f64 = 1.0;
        for (i, s) in samples.iter().enumerate() {
            c.update(s, t0 + Duration::from_millis(500 * i as u64));
            lowest = lowest.min(c.scale);
        }
        assert_eq!(lowest, 1.0, "jitter read as a queue");
    }

    /// With the path really congested (budget cut to the minimum, a low
    /// configured budget) the two nearest players keep their 30 Hz floor:
    /// the bucket leaves room for the floors, so it never drops frames the
    /// plan's interval promised.
    #[test]
    fn a_congestion_cut_keeps_the_nearest_floors() {
        let relay = Relay::default();
        let a = addrs(3);
        for (i, x) in a.iter().enumerate() { relay.update_pos(*x, [i as f32 * 200.0, 0.0, 0.0]); }
        relay.inner.lock().unwrap().ranked_at = None;
        let mut got: HashMap<SocketAddr, u32> = HashMap::new();
        for step in 0..30u64 {
            {
                let mut r = relay.inner.lock().unwrap();
                r.ranked_at = Instant::now().checked_sub(RANK_REFRESH + Duration::from_millis(1));
                for p in r.paths.values_mut() { p.last_cut = None; }
            }
            let srtt = if step == 0 { 50.0 } else { 600.0 };
            let paths: Vec<(SocketAddr, PathSample)> = a.iter().map(|x| (*x, sample(step * 100, 0, srtt, 50.0))).collect();
            relay.replan_with_paths(&a, || paths);
            // Half a second of 60 Hz frames, spread as wall time spreads them.
            for k in 0..30 {
                if k % 6 == 0 {
                    let mut r = relay.inner.lock().unwrap();
                    for b in r.buckets.values_mut() { b.last -= Duration::from_millis(100); }
                    for b in r.shares.values_mut() { b.last -= Duration::from_millis(100); }
                }
                for &src in &a[1..] {
                    // 1.5 KB frames: two 60 Hz players need 180 KB/s, far over
                    // the cut budget (32 KB/s); their 30 Hz floors need 90.
                    for p in relay.select(src, &a, Stream::Skel, 1500) {
                        if p.dst == a[0] && step >= 15 { *got.entry(src).or_insert(0) += 1; }
                    }
                }
            }
        }
        let scale = relay.inner.lock().unwrap().paths[&a[0]].scale;
        assert!(scale <= 0.26, "the path was cut: {scale}");
        for src in &a[1..] {
            let hz = got.get(src).copied().unwrap_or(0) as f64 / 7.5;
            assert!(hz >= 29.0, "{src}: {hz:.1} Hz under a congestion cut");
        }
    }
}
