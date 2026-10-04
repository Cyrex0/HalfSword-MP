//! Simulator model. Everything is a pure function of the seed.
//!
//! * `body`   — procedural fighters: root paths, postures (falls), right-arm
//!              actions (swing, thrust, feint, parry, guard), blade geometry
//!              per weapon, the TRUE collision bodies (what the game's physics
//!              assets touch) and the server capsule table.
//! * `script` — fight generator: engagements between pairs / 2v1 in 2–8
//!              player melees (clean attacks with retreats, parries, missed
//!              parries, feints, trades, clinches, falls).
//! * `net`    — netsim profiles (same numbers as `hsmp-tools netsim`) and the
//!              link model (delay, uniform jitter, loss, dup, spikes; reliable
//!              channel retransmission after an RTO).
//! * `world`  — the run loop: clients (clock with drift, jitter-buffered
//!              display of every peer, contact / clash detection on their own
//!              screen, the HSMPCombat claim path and its dedupe policy, the
//!              sidecar resend/ack path), the server (real `lagcomp::Store` +
//!              `combat::Engine` + glue), cheats, and metrics.
//! * `report` — aggregation and the report table.
//! * `frame`  — hit location across the replay delay (armour layer by body-relative spot).

pub mod body;
pub mod core;
pub mod frame;
pub mod game;
pub mod net;
pub mod report;
pub mod script;
pub mod suite;
pub mod world;

pub type V3 = [f32; 3];

pub fn add(a: V3, b: V3) -> V3 { [a[0] + b[0], a[1] + b[1], a[2] + b[2]] }
pub fn sub(a: V3, b: V3) -> V3 { [a[0] - b[0], a[1] - b[1], a[2] - b[2]] }
pub fn mul(a: V3, s: f32) -> V3 { [a[0] * s, a[1] * s, a[2] * s] }
pub fn dot(a: V3, b: V3) -> f32 { a[0] * b[0] + a[1] * b[1] + a[2] * b[2] }
pub fn cross(a: V3, b: V3) -> V3 {
    [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]]
}
pub fn len(a: V3) -> f32 { dot(a, a).sqrt() }
pub fn norm(a: V3) -> V3 {
    let l = len(a);
    if l < 1e-6 { [1.0, 0.0, 0.0] } else { mul(a, 1.0 / l) }
}
pub fn lerp3(a: V3, b: V3, f: f32) -> V3 { add(a, mul(sub(b, a), f)) }
/// Rotate v about unit axis k by angle a (Rodrigues).
pub fn rot(v: V3, k: V3, a: f32) -> V3 {
    let (s, c) = a.sin_cos();
    add(add(mul(v, c), mul(cross(k, v), s)), mul(k, dot(k, v) * (1.0 - c)))
}

/// Closest points between segments p1q1 and p2q2: (distance, s on first, t on second).
pub fn seg_seg(p1: V3, q1: V3, p2: V3, q2: V3) -> (f32, f32, f32) {
    let (d1, d2, r) = (sub(q1, p1), sub(q2, p2), sub(p1, p2));
    let (a, e, f) = (dot(d1, d1), dot(d2, d2), dot(d2, r));
    let (s, t);
    if a < 1e-6 && e < 1e-6 {
        return (len(r), 0.0, 0.0);
    }
    if a < 1e-6 {
        s = 0.0;
        t = (f / e).clamp(0.0, 1.0);
    } else {
        let c = dot(d1, r);
        if e < 1e-6 {
            t = 0.0;
            s = (-c / a).clamp(0.0, 1.0);
        } else {
            let b = dot(d1, d2);
            let denom = a * e - b * b;
            let mut s0 = if denom > 1e-6 { ((b * f - c * e) / denom).clamp(0.0, 1.0) } else { 0.0 };
            let mut t0 = (b * s0 + f) / e;
            if t0 < 0.0 { t0 = 0.0; s0 = (-c / a).clamp(0.0, 1.0); }
            else if t0 > 1.0 { t0 = 1.0; s0 = ((b - c) / a).clamp(0.0, 1.0); }
            s = s0;
            t = t0;
        }
    }
    let c1 = add(p1, mul(d1, s));
    let c2 = add(p2, mul(d2, t));
    (len(sub(c1, c2)), s, t)
}

pub fn point_seg(p: V3, a: V3, b: V3) -> (f32, f32) {
    let ab = sub(b, a);
    let l2 = dot(ab, ab);
    let t = if l2 < 1e-6 { 0.0 } else { (dot(sub(p, a), ab) / l2).clamp(0.0, 1.0) };
    (len(sub(p, add(a, mul(ab, t)))), t)
}

/// SplitMix64: small, fast, deterministic.
#[derive(Clone)]
pub struct Rng(pub u64);

impl Rng {
    pub fn new(seed: u64) -> Rng { Rng(seed ^ 0x9E37_79B9_7F4A_7C15) }
    pub fn u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }
    /// [0, 1)
    pub fn f(&mut self) -> f64 { (self.u64() >> 11) as f64 / (1u64 << 53) as f64 }
    pub fn range(&mut self, a: f64, b: f64) -> f64 { a + (b - a) * self.f() }
    pub fn rf(&mut self, a: f32, b: f32) -> f32 { self.range(a as f64, b as f64) as f32 }
    pub fn chance(&mut self, p: f64) -> bool { self.f() < p }
    pub fn pick<'a, T>(&mut self, v: &'a [T]) -> &'a T { &v[(self.u64() % v.len() as u64) as usize] }
    pub fn gauss(&mut self) -> f64 {
        let u1 = self.f().max(1e-12);
        let u2 = self.f();
        (-2.0 * u1.ln()).sqrt() * (2.0 * std::f64::consts::PI * u2).cos()
    }
    pub fn fork(&mut self, salt: u64) -> Rng { Rng::new(self.u64() ^ salt.wrapping_mul(0xA24B_AED4_963E_E407)) }
}
