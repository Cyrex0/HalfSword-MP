//! Token bucket on server milliseconds.

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Bucket {
    pub tokens: f32,
    pub at_ms: i64,
}

impl Bucket {
    pub fn full(cap: f32, now_ms: i64) -> Bucket { Bucket { tokens: cap, at_ms: now_ms } }

    /// Refill at `per_s` up to `cap`, then take one token if available.
    pub fn take(&mut self, cap: f32, per_s: f32, now_ms: i64) -> bool {
        let dt = (now_ms - self.at_ms).max(0) as f32 / 1000.0;
        self.tokens = (self.tokens + dt * per_s).min(cap);
        self.at_ms = self.at_ms.max(now_ms);
        if self.tokens >= 1.0 {
            self.tokens -= 1.0;
            true
        } else {
            false
        }
    }
}

/// Per-peer message budgets for the fan-out paths without one of their own:
/// chat, vitals, root movement. `forget`
/// drops a peer's buckets when it leaves (peer ids are never reused).
#[derive(Debug, Default)]
pub struct PeerLimits {
    buckets: std::collections::HashMap<(u32, Kind), Bucket>,
}

/// What is budgeted, with (burst, refill per second) in units of `cost`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Kind {
    /// Chat lines.
    Chat,
    /// C2SVitals + C2SVitalsFrame packets (each one is relayed to every peer).
    /// Honest owners send ≤ 20/s (VitalsGate ≥ 50 ms apart, plus
    /// two repeats of a significant frame and the 1 s keyframe).
    Vitals,
    /// Root movement, in uu: a distance bucket on the server
    /// clock. Refills at the body speed cap; holds at most half a second of
    /// it plus the jitter burst, so bunched packets can never each claim a
    /// fresh burst (otherwise ten roots in one tick could move ~35 m).
    RootDist,
}

/// Highest honest stream send rate (HSMPSync `send_hz` maximum, 120 Hz).
pub const STREAM_MAX_HZ: f32 = 120.0;
/// Per-sender packet budget of each high-rate stream:
/// 1.5 × the maximum honest rate, plus a burst for jitter-bunched packets.
pub const STREAM_RATE_PER_S: f32 = STREAM_MAX_HZ * 1.5;
pub const STREAM_BURST: f32 = 60.0;

/// The high-rate streams a client sends (root, skeletal, weapon).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum StreamKind { Root, Skel, Weapon }

/// Per-source-address packet buckets of the high-rate streams.
/// Keyed by address so the check runs before any lock of the game state;
/// bounded (stale entries pruned past MAX_STREAM_SOURCES).
#[derive(Debug, Default)]
pub struct StreamLimits {
    b: std::collections::HashMap<(std::net::SocketAddr, StreamKind), Bucket>,
}

pub const MAX_STREAM_SOURCES: usize = 4096;

impl StreamLimits {
    pub fn take(&mut self, from: std::net::SocketAddr, k: StreamKind, now_ms: i64) -> bool {
        if self.b.len() >= MAX_STREAM_SOURCES && !self.b.contains_key(&(from, k)) {
            // Entries idle for 10 s are full again anyway: forget them.
            self.b.retain(|_, b| now_ms - b.at_ms < 10_000);
            if self.b.len() >= MAX_STREAM_SOURCES { return false; }
        }
        self.b.entry((from, k)).or_insert(Bucket::full(STREAM_BURST, now_ms))
            .take(STREAM_BURST, STREAM_RATE_PER_S, now_ms)
    }
    pub fn forget(&mut self, from: &std::net::SocketAddr) {
        self.b.retain(|(a, _), _| a != from);
    }
    pub fn len(&self) -> usize { self.b.len() }
}

impl Kind {
    pub fn budget(self) -> (f32, f32) {
        match self {
            Kind::RootDist => (
                super::input::ROOT_BURST + super::input::ROOT_SPEED_MAX * 0.5,
                super::input::ROOT_SPEED_MAX,
            ),
            Kind::Chat => (5.0, 1.0),
            // Opus at 64 kbit/s is 8 KB/s; 4x headroom.
            Kind::Vitals => (40.0, 25.0),
        }
    }
}

impl PeerLimits {
    /// Take `cost` units of `kind` for `peer`; false = over budget (drop).
    pub fn take(&mut self, peer: u32, kind: Kind, cost: f32, now_ms: i64) -> bool {
        let (cap, per_s) = kind.budget();
        let b = self.buckets.entry((peer, kind)).or_insert(Bucket::full(cap, now_ms));
        let dt = (now_ms - b.at_ms).max(0) as f32 / 1000.0;
        b.tokens = (b.tokens + dt * per_s).min(cap);
        b.at_ms = b.at_ms.max(now_ms);
        if b.tokens >= cost {
            b.tokens -= cost;
            true
        } else {
            false
        }
    }

    /// Empty `peer`'s `kind` bucket (a step accepted on the plain speed rule
    /// used up any burst).
    pub fn drain(&mut self, peer: u32, kind: Kind, now_ms: i64) {
        self.buckets.insert((peer, kind), Bucket { tokens: 0.0, at_ms: now_ms });
    }

    pub fn forget(&mut self, peer: u32) {
        self.buckets.retain(|(p, _), _| *p != peer);
    }

    pub fn len(&self) -> usize { self.buckets.len() }
}

/// Lines per second each client-drivable log site may emit at its normal
/// level; beyond that the caller logs at `debug!`.
pub const LOG_PER_S: u32 = 5;

/// Log budget for a per-packet / per-claim log site that a client can drive
/// (over-cap roots, rejected claims, clamped damage ...): true at most
/// LOG_PER_S times per wall-clock second per `key`, so a flood cannot fill
/// the log (and the disk of a dedicated host). Usage:
/// `if log_ok("root_cap") { warn!(..) } else { debug!(..) }`.
pub fn log_ok(key: &'static str) -> bool {
    log_ok_at(key, log_clock_s())
}

fn log_clock_s() -> u64 {
    static T0: std::sync::OnceLock<std::time::Instant> = std::sync::OnceLock::new();
    T0.get_or_init(std::time::Instant::now).elapsed().as_secs()
}

fn log_ok_at(key: &'static str, sec: u64) -> bool {
    use std::collections::HashMap;
    use std::sync::{Mutex, OnceLock};
    static M: OnceLock<Mutex<HashMap<&'static str, (u64, u32)>>> = OnceLock::new();
    let mut m = M.get_or_init(Default::default).lock().unwrap_or_else(|e| e.into_inner());
    let e = m.entry(key).or_insert((sec, 0));
    if e.0 != sec { *e = (sec, 0); }
    e.1 += 1;
    e.1 <= LOG_PER_S
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn log_budget_caps_each_site_per_second() {
        let n = (0..1000).filter(|_| log_ok_at("test_site_a", 7)).count();
        assert_eq!(n, LOG_PER_S as usize);
        assert!(log_ok_at("test_site_b", 7), "per site");
        assert!(log_ok_at("test_site_a", 8), "next second refills");
    }

    #[test]
    fn peer_limits_cap_each_kind_per_peer() {
        let mut l = PeerLimits::default();
        assert_eq!((0..100).filter(|_| l.take(1, Kind::Chat, 1.0, 0)).count(), 5);
        assert!(l.take(2, Kind::Chat, 1.0, 0), "per peer");
        assert!(l.take(1, Kind::Vitals, 1.0, 0), "per kind");
        assert!(l.take(1, Kind::Chat, 1.0, 1_000), "refills");
        l.forget(1);
        assert_eq!(l.len(), 1, "only peer 2's bucket left");
    }

    /// A vitals flood is capped per sender (5 000/s → ≤ burst +
    /// 25/s), while an honest 20 Hz stream with repeats always passes.
    #[test]
    fn vitals_budget_caps_a_flood_not_an_honest_stream() {
        let mut l = PeerLimits::default();
        let flood = (0..5_000).filter(|k| l.take(7, Kind::Vitals, 1.0, *k as i64 / 5)).count();
        assert!(flood <= 40 + 25 + 1, "{flood}");
        let mut l = PeerLimits::default();
        // 20 Hz for 10 s plus a repeat pair every 500 ms.
        let mut sent = 0;
        let mut ok = 0;
        for ms in 0..10_000i64 {
            let n = (ms % 50 == 0) as usize + if ms % 500 == 120 || ms % 500 == 360 { 1 } else { 0 };
            for _ in 0..n { sent += 1; if l.take(8, Kind::Vitals, 1.0, ms) { ok += 1; } }
        }
        assert_eq!(ok, sent);
    }

    /// A 10 000/s root flood from one sender is cut to the burst
    /// plus 180/s; an honest 120 Hz stream (with jitter bunching) always passes.
    #[test]
    fn stream_limits_cap_a_flood_not_an_honest_120hz_stream() {
        let a: std::net::SocketAddr = "203.0.113.1:5000".parse().unwrap();
        let mut l = StreamLimits::default();
        let flood = (0..10_000).filter(|k| l.take(a, StreamKind::Root, *k as i64 / 10)).count();
        assert!(flood as f32 <= STREAM_BURST + STREAM_RATE_PER_S + 1.0, "{flood}");
        assert!(l.take(a, StreamKind::Skel, 0), "per stream");
        let mut l = StreamLimits::default();
        // 120 Hz for 10 s, every 10th packet delayed and delivered with the next 3.
        let mut ok = 0;
        let mut n = 0;
        for k in 0..1200i64 {
            let t = k * 1000 / 120;
            let t = if k % 10 < 3 { t - (k % 10) * 8 + 24 } else { t };
            n += 1;
            if l.take(a, StreamKind::Skel, t) { ok += 1; }
        }
        assert_eq!(ok, n);
        l.forget(&a);
        assert_eq!(l.len(), 0);
    }

    /// Bunched roots share one distance bucket.
    #[test]
    fn root_distance_bucket_stops_per_packet_bursts() {
        let mut l = PeerLimits::default();
        // Ten 350 uu steps in the same millisecond: only the first fits.
        let n = (0..10).filter(|_| l.take(1, Kind::RootDist, 350.0, 1_000)).count();
        assert!(n <= 3, "{n} packets moved {} uu at once", n * 350);
        // A sprint at 700 uu/s, 60 Hz, for 10 s: always accepted.
        let mut l = PeerLimits::default();
        let step = 700.0 / 60.0;
        assert!((0..600).all(|k| l.take(2, Kind::RootDist, step, k * 1000 / 60)));
        // 10 m after half a second of standing still (knock-back): accepted.
        let mut l = PeerLimits::default();
        assert!(l.take(3, Kind::RootDist, 1000.0, 500));
    }

    #[test]
    fn burst_then_refill() {
        let mut b = Bucket::full(3.0, 0);
        assert!((0..3).all(|_| b.take(3.0, 10.0, 0)));
        assert!(!b.take(3.0, 10.0, 0));
        assert!(!b.take(3.0, 10.0, 50)); // 0.5 token
        assert!(b.take(3.0, 10.0, 100));
        // A clock going backwards never mints tokens.
        assert!(!b.take(3.0, 10.0, -10_000));
    }
}
