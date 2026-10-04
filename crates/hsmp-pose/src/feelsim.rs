//! Netfeel: an offline, deterministic model of how a remote fighter looks on
//! another player's screen over a bad path.
//!
//! sender game (60 fps, frame-time noise) -> uplink -> server relay (dedup,
//! decimation, `aux` interval) -> downlink -> the REAL jitter buffer
//! (`poseplay::Playback`, sampled every 2 ms like the sidecar's writer) ->
//! a model of HSMPAvatars' v2 driver (smooth Lua clock, aim, velocity servo
//! on free point bodies, the rigid-snap rule).
//!
//! The links copy `hsmp-tools netsim`'s path model: AR(1) correlated jitter,
//! FIFO delivery, Gilbert-Elliott burst loss, explicit reordering,
//! duplication and periodic spikes. Sender and receiver clocks have their
//! own offsets and drift.
//!
//! What it reports per run (`Report`): display latency vs RTT, buffer delay,
//! tracking error against the owner's true motion, and the rubber-band
//! measures: per-frame jumps of the shown body beyond the owner's own motion,
//! backward steps, playback-clock rate excursions and rigid snaps.
//!
//! Pure and fast (a 60 s run takes well under a second in release).

use crate::poseplay::{Frame, Mode, Playback, Sample, SLOTS};

/// Bodies the model drives: the pelvis and the right hand.
pub const PEL: usize = 0;
pub const HAND: usize = 16;
const BODIES: [usize; 2] = [PEL, HAND];

// ---- deterministic RNG ------------------------------------------------------------------

#[derive(Clone)]
pub struct Rng(u64);

impl Rng {
    pub fn new(seed: u64) -> Rng { Rng(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) ^ 0xD1B5_4A32_D192_ED03) }
    pub fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }
    pub fn f(&mut self) -> f64 { (self.next_u64() >> 11) as f64 / (1u64 << 53) as f64 }
    pub fn range(&mut self, a: f64, b: f64) -> f64 { a + (b - a) * self.f() }
    pub fn gauss(&mut self) -> f64 {
        let u1 = self.f().max(1e-300);
        let u2 = self.f();
        (-2.0 * u1.ln()).sqrt() * (2.0 * std::f64::consts::PI * u2).cos()
    }
}

// ---- link model (netsim's) --------------------------------------------------------------

/// One direction of a path. Mirrors `hsmp-tools netsim` PROFILES + PROFILE_PATH
/// (a test in hsmp-tools checks they agree).
#[derive(Clone, Copy, Debug)]
pub struct PathModel {
    pub name: &'static str,
    pub delay: f64,
    pub jitter: f64,
    pub loss: f64,
    pub dup: f64,
    pub spike_every_s: f64,
    pub spike_ms: f64,
    pub reorder_pct: f64,
    pub reorder_ms: f64,
    pub tau_ms: f64,
    pub burst: f64,
}

/// netsim's default spike length.
pub const SPIKE_LEN_MS: f64 = 300.0;

const fn pm(name: &'static str, delay: f64, jitter: f64, loss: f64, dup: f64, spike_every_s: f64, spike_ms: f64,
            reorder_pct: f64, reorder_ms: f64, tau_ms: f64, burst: f64) -> PathModel {
    PathModel { name, delay, jitter, loss, dup, spike_every_s, spike_ms, reorder_pct, reorder_ms, tau_ms, burst }
}

pub const PATHS: &[PathModel] = &[
    pm("lan", 1.0, 0.5, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 20.0, 1.0),
    pm("good", 20.0, 4.0, 0.2, 0.1, 0.0, 0.0, 0.0, 10.0, 30.0, 1.5),
    pm("typical", 50.0, 12.0, 1.0, 0.3, 0.0, 0.0, 0.1, 25.0, 40.0, 2.0),
    pm("wifi", 35.0, 25.0, 2.0, 0.5, 7.0, 120.0, 1.0, 30.0, 30.0, 3.0),
    pm("intl", 88.0, 30.0, 0.3, 0.1, 0.0, 0.0, 0.1, 25.0, 40.0, 2.0),
    pm("far", 150.0, 50.0, 2.0, 0.3, 0.0, 0.0, 0.5, 30.0, 50.0, 2.0),
    pm("bad", 110.0, 40.0, 5.0, 1.0, 10.0, 200.0, 2.0, 40.0, 60.0, 4.0),
    pm("awful", 180.0, 70.0, 10.0, 2.0, 5.0, 300.0, 3.0, 60.0, 80.0, 5.0),
];

pub fn path(name: &str) -> Option<PathModel> { PATHS.iter().copied().find(|p| p.name == name) }

/// One direction of one client's path: send times in, arrival times out.
pub struct Link {
    p: PathModel,
    j: f64,
    last_send: Option<f64>,
    bad: bool,
    line: Option<f64>,
    phase_ms: f64,
    rng: Rng,
}

impl Link {
    pub fn new(p: PathModel, seed: u64) -> Link {
        let mut rng = Rng::new(seed);
        let phase_ms = rng.range(0.0, p.spike_every_s.max(1.0) * 1000.0);
        Link { p, j: 0.0, last_send: None, bad: false, line: None, phase_ms, rng }
    }

    fn spike(&self, t: f64) -> f64 {
        if self.p.spike_every_s > 0.0 && ((t + self.phase_ms) % (self.p.spike_every_s * 1000.0)) < SPIKE_LEN_MS { self.p.spike_ms } else { 0.0 }
    }

    /// Arrival times of a datagram sent at `t` (ms): empty = lost, two = duplicated.
    pub fn send(&mut self, t: f64) -> Vec<f64> {
        let p = self.p;
        let loss = p.loss / 100.0;
        let u = self.rng.f();
        self.bad = if loss <= 0.0 {
            false
        } else if p.burst <= 1.0 {
            u < loss
        } else {
            let r = 1.0 / p.burst;
            if self.bad { u >= r } else { u < (loss * r / (1.0 - loss)).min(1.0) }
        };
        if self.bad { return Vec::new(); }
        let dt = self.last_send.map_or(1e9, |l| t - l);
        self.last_send = Some(t);
        let z = self.rng.gauss();
        self.j = if p.tau_ms <= 0.0 { z } else {
            let rho = (-dt.max(0.0) / p.tau_ms).exp();
            rho * self.j + (1.0 - rho * rho).sqrt() * z
        };
        let jit = if p.jitter > 0.0 { (self.j * p.jitter / 3f64.sqrt()).clamp(-p.jitter, p.jitter) } else { 0.0 };
        let mut at = t + (p.delay + self.spike(t) + jit).max(0.0);
        let reordered = p.reorder_pct > 0.0 && self.rng.f() * 100.0 < p.reorder_pct;
        if let Some(l) = self.line { if at < l { at = l; } }
        if reordered { at += p.reorder_ms; } else { self.line = Some(at); }
        let mut out = vec![at];
        if self.rng.f() * 100.0 < p.dup {
            let c = at + self.rng.range(0.05, 0.5);
            if !reordered { self.line = Some(c); }
            out.push(c);
        }
        out
    }
}

// ---- the owner's true motion ------------------------------------------------------------

pub type V3 = [f64; 3];

#[derive(Clone, Copy, Debug, Default)]
pub struct State { pub pos: [V3; 2], pub vel: [V3; 2] }

/// Fighter motion sampled every 1 ms: the pelvis walks, runs, stops, turns and is
/// knocked back; the right hand swings fast arcs around it.
pub struct Motion { pub st: Vec<State> }

fn add(a: V3, b: V3) -> V3 { [a[0] + b[0], a[1] + b[1], a[2] + b[2]] }
fn sub(a: V3, b: V3) -> V3 { [a[0] - b[0], a[1] - b[1], a[2] - b[2]] }
fn mul(a: V3, k: f64) -> V3 { [a[0] * k, a[1] * k, a[2] * k] }
fn len(a: V3) -> f64 { (a[0] * a[0] + a[1] * a[1] + a[2] * a[2]).sqrt() }
fn dot(a: V3, b: V3) -> f64 { a[0] * b[0] + a[1] * b[1] + a[2] * b[2] }

impl Motion {
    pub fn generate(secs: f64, seed: u64) -> Motion {
        let mut rng = Rng::new(seed ^ 0x5EED);
        let n = (secs * 1000.0) as usize + 2;
        let dt = 0.001;
        let mut st = Vec::with_capacity(n);
        let (mut p, mut v, mut kv, mut kpend, mut kleft) = ([0.0, 0.0, 100.0], [0.0; 3], [0.0; 3], [0.0; 3], 0.0f64);
        let mut tv = [0.0; 3];
        let mut heading: f64 = 0.0;
        let mut next_seg = 0.0;
        let mut next_kb = rng.range(3.0, 6.0);
        let mut th = 0.0f64;
        let mut swing: Option<(f64, f64, f64, f64)> = None; // (t0, dur, th0, delta)
        let mut next_swing = rng.range(0.5, 1.2);
        for k in 0..n {
            let t = k as f64 * dt;
            if t >= next_seg {
                next_seg = t + rng.range(0.5, 2.0);
                heading += rng.range(-2.0, 2.0);
                let r = rng.f();
                let spd = if r < 0.25 { 0.0 } else if r < 0.7 { 250.0 } else { 480.0 };
                tv = [spd * heading.cos(), spd * heading.sin(), 0.0];
            }
            if t >= next_kb {
                next_kb = t + rng.range(4.0, 8.0);
                let a = rng.range(0.0, std::f64::consts::TAU);
                kpend = [1000.0 * a.cos(), 1000.0 * a.sin(), 0.0];
                kleft = 0.04;
            }
            // accelerate toward the target velocity (3000 uu/s^2)
            let dv = sub(tv, v);
            let l = len(dv);
            let step = 3000.0 * dt;
            v = if l > step { add(v, mul(dv, step / l)) } else { tv };
            // a blow accelerates the body over ~40 ms, then it slows down
            kv = mul(kv, (-dt / 0.15).exp());
            if kleft > 0.0 { kv = add(kv, mul(kpend, dt / 0.04)); kleft -= dt; }
            let speed = len(v);
            let bob_v = if speed > 50.0 { 3.0 * 2.0 * std::f64::consts::TAU * (2.0 * std::f64::consts::TAU * t).cos() } else { 0.0 };
            let pv = [v[0] + kv[0], v[1] + kv[1], bob_v];
            // hand: swings of ~2.4 rad in 0.28 s (peak ~1000 uu/s), slow drift between
            if swing.is_none() && t >= next_swing {
                let d = if rng.f() < 0.5 { 2.4 } else { -2.4 };
                swing = Some((t, rng.range(0.24, 0.34), th, d));
            }
            let thv = if let Some((t0, dur, th0, d)) = swing {
                let s = ((t - t0) / dur).min(1.0);
                let tau = std::f64::consts::TAU;
                th = th0 + d * (s - (tau * s).sin() / tau);
                if s >= 1.0 {
                    swing = None;
                    next_swing = t + rng.range(0.6, 1.8);
                    0.0
                } else {
                    d * (1.0 - (tau * s).cos()) / dur
                }
            } else {
                let w = 0.4 * (1.3 * t).cos();
                th += w * dt;
                w
            };
            let r = 60.0;
            let hand = add(p, [r * th.cos(), r * th.sin(), 40.0]);
            let hv = add(pv, [-r * th.sin() * thv, r * th.cos() * thv, 0.0]);
            st.push(State { pos: [p, hand], vel: [pv, hv] });
            p = add(p, mul(pv, dt));
        }
        Motion { st }
    }

    /// State at true time `t` ms (linear between the 1 ms samples).
    pub fn at(&self, t: f64) -> State {
        let n = self.st.len();
        let x = t.clamp(0.0, (n - 2) as f64);
        let k = x.floor() as usize;
        let s = x - k as f64;
        let (a, b) = (&self.st[k], &self.st[k + 1]);
        let mut o = State::default();
        for i in 0..2 {
            for ax in 0..3 {
                o.pos[i][ax] = a.pos[i][ax] + (b.pos[i][ax] - a.pos[i][ax]) * s;
                o.vel[i][ax] = a.vel[i][ax] + (b.vel[i][ax] - a.vel[i][ax]) * s;
            }
        }
        o
    }
}

// ---- configuration ----------------------------------------------------------------------

/// A local clock: `local = true_ms * rate + off`.
#[derive(Clone, Copy, Debug)]
pub struct ClockMap { pub off: f64, pub rate: f64 }
impl ClockMap {
    pub fn local(&self, t: f64) -> f64 { t * self.rate + self.off }
    pub fn true_of(&self, local: f64) -> f64 { (local - self.off) / self.rate }
}

#[derive(Clone, Debug)]
pub struct Config {
    pub secs: f64,
    pub seed: u64,
    /// Client A (sender) -> server and server -> client B (receiver).
    pub up: PathModel,
    pub down: PathModel,
    pub send_fps: f64,
    pub recv_fps: f64,
    /// Relay decimation of the skeleton (1 = full rate) and of the root.
    pub skel_factor: u32,
    pub root_factor: u32,
    /// Game-side model knobs (HSMPAvatars).
    pub gain: f64,
    pub cap_lin: f64,
    pub lat_target_ms: f64,
    /// Lua playback-clock hard reset threshold (ms).
    pub clk_reset_ms: f64,
    /// The game clock follows the sidecar's playback rate and the step offset uses
    /// the smoothed frame time (false = the driver before the smoothing change).
    pub lua_v2: bool,
}

impl Config {
    pub fn profile(name: &str, secs: f64, seed: u64) -> Config {
        let p = path(name).unwrap_or_else(|| panic!("no path model {name}"));
        Config { secs, seed, up: p, down: p, send_fps: 60.0, recv_fps: 60.0, skel_factor: 1, root_factor: 2,
                 gain: 0.3, cap_lin: 900.0, lat_target_ms: 75.0, clk_reset_ms: 50.0, lua_v2: true }
    }
}

// ---- the report -------------------------------------------------------------------------

#[derive(Clone, Debug, Default)]
pub struct Report {
    pub profile: String,
    pub rtt_ms: f64,
    pub frames: usize,
    /// Owner's true time -> what the screen shows (ms): p50 / p95.
    pub latency_p50: f64,
    pub latency_p95: f64,
    /// Buffer delay (mean) and jitter estimate (mean), ms.
    pub delay_mean: f64,
    pub jitter_mean: f64,
    /// Shown body vs the owner's true pose at the time it stands for (uu).
    pub pel_err_p95: f64,
    pub hand_err_p95: f64,
    pub hand_err_max: f64,
    /// Per-frame jump of the shown body beyond the owner's own motion over the
    /// same span (uu): p99 and max; and the count per minute of frames that
    /// jump more than SNAP_UU (pelvis) / SNAP_HAND_UU (hand).
    pub pel_jump_p99: f64,
    pub pel_jump_max: f64,
    pub hand_jump_p99: f64,
    pub hand_jump_max: f64,
    pub pel_snaps_per_min: f64,
    pub hand_snaps_per_min: f64,
    /// Frames the shown pelvis moves against a moving owner (> 100 uu/s).
    pub backsteps_per_min: f64,
    /// Rigid mesh snaps (SNAP_BODY_ERR rule) and the playback-clock resets
    /// (Lua clock > clk_reset_ms off), per minute.
    pub rigid_snaps: u32,
    pub clock_resets_per_min: f64,
    /// Shown-time rate excursions: frames where the label advanced more than
    /// 25 % faster or slower than real time.
    pub warp_frames_pct: f64,
    /// Sampler modes (%).
    pub interp_pct: f64,
    pub extrap_pct: f64,
    pub hold_pct: f64,
    /// Frames dropped as late by the buffer, and cuts.
    pub late: u32,
    pub cuts: u32,
    /// Capsule (root stream) vs pelvis at the same playback time, p95 (uu).
    pub root_pel_p95: f64,
}

/// A frame of the shown pelvis moving this much more (or less) than the owner did
/// over the same span counts as a visible snap (3 uu = 180 uu/s of velocity error).
pub const SNAP_UU: f64 = 3.0;
/// Hands move fast; a hand snap is a larger jump.
pub const SNAP_HAND_UU: f64 = 8.0;

pub fn pct(v: &[f64], p: f64) -> f64 {
    if v.is_empty() { return f64::NAN; }
    let mut s = v.to_vec();
    s.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let k = ((p / 100.0 * s.len() as f64).ceil() as usize).clamp(1, s.len()) - 1;
    s[k]
}

fn mean(v: &[f64]) -> f64 { if v.is_empty() { f64::NAN } else { v.iter().sum::<f64>() / v.len() as f64 } }

impl Report {
    pub fn header() -> String {
        format!("{:<8} {:>5} {:>6} {:>6} {:>6} {:>5} {:>6} {:>6} {:>6} {:>7} {:>7} {:>6} {:>6} {:>5} {:>4} {:>5} {:>5} {:>5} {:>5} {:>4}",
            "profile", "rtt", "lat50", "lat95", "delay", "jit", "pel95", "hnd95", "hndmx", "pjmp99", "pjmpmx", "psnp/m",
            "hsnp/m", "bk/m", "rig", "clk/m", "warp%", "ext%", "hld%", "late")
    }
    pub fn row(&self) -> String {
        format!("{:<8} {:>5.0} {:>6.1} {:>6.1} {:>6.1} {:>5.1} {:>6.2} {:>6.2} {:>6.1} {:>7.2} {:>7.1} {:>6.1} {:>6.1} {:>5.1} {:>4} {:>5.1} {:>5.1} {:>5.1} {:>5.1} {:>4}",
            self.profile, self.rtt_ms, self.latency_p50, self.latency_p95, self.delay_mean, self.jitter_mean,
            self.pel_err_p95, self.hand_err_p95, self.hand_err_max, self.pel_jump_p99, self.pel_jump_max,
            self.pel_snaps_per_min, self.hand_snaps_per_min, self.backsteps_per_min, self.rigid_snaps,
            self.clock_resets_per_min, self.warp_frames_pct, self.extrap_pct, self.hold_pct, self.late)
    }
}

// ---- the run ----------------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq)]
enum Ev { Pose(u32), Root(u32) }

fn frame_of(tick: u32, ts: f64, s: &State) -> Frame {
    let mut b = [[0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 1.0]; SLOTS];
    let mut vel = [[0.0f32; 6]; SLOTS];
    for (k, &i) in BODIES.iter().enumerate() {
        for ax in 0..3 {
            b[i][ax] = s.pos[k][ax] as f32;
            vel[i][ax] = s.vel[k][ax] as f32;
        }
    }
    let mask = (1 << PEL) | (1 << HAND);
    Frame { ts, tick, mask, b, vmask: mask, vel, v2: true, extra: None }
}

/// Game-side state of one stand-in (HSMPAvatars drive_v2, reduced to two bodies).
struct Driver {
    clk: Option<(f64, f64, f64)>,
    dsm: f64,
    body: [V3; 2],
    vset: [V3; 2],
    vprev: Option<(f64, [V3; 2])>,
    acc: [Option<V3>; 2],
    xoff_last: Option<f64>,
    err_since: Option<f64>,
    shown_label: Option<f64>,
    started: bool,
}

/// Advance a target (pos, vel) by `ms` with its velocity and acceleration.
fn advance(p: V3, v: V3, a: Option<V3>, ms: f64) -> (V3, V3) {
    let s = ms / 1000.0;
    match a {
        Some(a) => (add(add(p, mul(v, s)), mul(a, 0.5 * s * s)), add(v, mul(a, s))),
        None => (add(p, mul(v, s)), v),
    }
}

pub fn run(cfg: &Config) -> Report {
    let mot = Motion::generate(cfg.secs + 2.0, cfg.seed);
    let mut rng = Rng::new(cfg.seed ^ 0xF00D);
    let sclk = ClockMap { off: 7_000_000.0 + rng.range(0.0, 1e6), rate: 1.0 + 40e-6 };
    let rclk = ClockMap { off: 3_000_000.0 + rng.range(0.0, 1e6), rate: 1.0 - 25e-6 };

    // Sender frames.
    let mut sends: Vec<(f64, u32)> = Vec::new();
    let (mut t, mut tick) = (200.0, 1u32);
    let sp = 1000.0 / cfg.send_fps;
    let end = cfg.secs * 1000.0;
    while t < end {
        sends.push((t, tick));
        tick += 1;
        let hitch = rng.f() < 0.002;
        t += if hitch { 3.0 * sp } else { sp + rng.range(-1.0, 1.0) };
    }
    // Uplink (one datagram per sample: root + pose).
    let mut up = Link::new(cfg.up, cfg.seed ^ 0xA);
    let mut at_server: Vec<(f64, u32)> = Vec::new();
    for &(t, k) in &sends {
        for a in up.send(t) { at_server.push((a, k)); }
    }
    at_server.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap());
    // Relay: transport dedup, counter decimation, separate datagrams down.
    let mut down = Link::new(cfg.down, cfg.seed ^ 0xB);
    let mut seen = std::collections::HashSet::new();
    let (mut n_skel, mut n_root) = (0u32, 0u32);
    let mut at_recv: Vec<(f64, Ev)> = Vec::new();
    for &(t, k) in &at_server {
        if !seen.insert(k) { continue; }
        if n_skel % cfg.skel_factor.max(1) == 0 {
            for a in down.send(t) { at_recv.push((a, Ev::Pose(k))); }
        }
        n_skel += 1;
        if n_root % cfg.root_factor.max(1) == 0 {
            for a in down.send(t + 0.01) { at_recv.push((a, Ev::Root(k))); }
        }
        n_root += 1;
    }
    at_recv.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap());
    let send_t: std::collections::HashMap<u32, f64> = sends.iter().map(|&(t, k)| (k, t)).collect();
    let interval_hint = (1000.0 / cfg.send_fps * cfg.skel_factor.max(1) as f64).round() as f32;

    // Receiver.
    let mut pb = Playback::new();
    let mut ev_i = 0;
    let mut next_write = 0.0f64;
    let mut latest: Option<(u64, Sample)> = None;
    let mut wseq = 0u64;
    let mut lead_req = 0.0f64;
    let rp = 1000.0 / cfg.recv_fps;
    let mut next_frame = 1000.0;
    let mut last_frame: Option<f64> = None;
    let mut read_seq = 0u64;
    let mut cur: Option<(Sample, f64)> = None; // sample + local read time
    let mut d = Driver { clk: None, dsm: rp, body: [[0.0; 3]; 2], vset: [[0.0; 3]; 2], vprev: None, acc: [None, None],
                         xoff_last: None, err_since: None, shown_label: None, started: false };
    let warm = 3000.0;
    let dbg: f64 = std::env::var("FEELSIM_DEBUG").ok().and_then(|v| v.parse().ok()).unwrap_or(0.0);
    let (mut lat, mut pel_err, mut hand_err) = (Vec::new(), Vec::new(), Vec::new());
    let (mut pel_jump, mut hand_jump) = (Vec::new(), Vec::new());
    let (mut delays, mut jits, mut root_pel) = (Vec::new(), Vec::new(), Vec::new());
    let (mut backsteps, mut clock_resets, mut warp, mut rigid) = (0u32, 0u32, 0u32, 0u32);
    let (mut n_frames, mut mi, mut me, mut mh) = (0usize, 0u32, 0u32, 0u32);
    let mut prev_shown: Option<(f64, [V3; 2])> = None;
    let stats0 = pb.stats;
    let mut late0 = None;
    let tend = end + 500.0;
    let mut tt = 0.0f64;
    while tt < tend {
        let t_next = next_write.min(next_frame);
        // Arrivals up to the next writer / game tick.
        while ev_i < at_recv.len() && at_recv[ev_i].0 <= t_next {
            let (ta, e) = at_recv[ev_i];
            ev_i += 1;
            let rx = rclk.local(ta);
            match e {
                Ev::Pose(k) => {
                    let st = mot.at(send_t[&k]);
                    pb.set_interval_hint(interval_hint);
                    pb.push_pose(rx, frame_of(k, sclk.local(send_t[&k]), &st));
                }
                Ev::Root(k) => {
                    let st = mot.at(send_t[&k]);
                    let p = [st.pos[0][0] as f32, st.pos[0][1] as f32, st.pos[0][2] as f32];
                    let v = [st.vel[0][0] as f32, st.vel[0][1] as f32, st.vel[0][2] as f32];
                    pb.push_root(rx, sclk.local(send_t[&k]), p, v, 0.0);
                }
            }
        }
        tt = t_next;
        if next_write <= next_frame {
            next_write += 2.0;
            if let Some(s) = pb.sample_lead(rclk.local(tt), lead_req) {
                wseq += 1;
                latest = Some((wseq, s));
            }
            continue;
        }
        // ---- one receiver game frame ----
        let now = rclk.local(tt);
        let dn = last_frame.map_or(rp, |l| now - l).clamp(0.0, 80.0);
        last_frame = Some(now);
        let hitch = rng.f() < 0.002;
        next_frame += if hitch { 3.0 * rp } else { rp + rng.range(-1.0, 1.0) };
        let h_pred = dn.clamp(4.0, 80.0);
        // physics: the step that just ran moved the bodies with last frame's velocities
        for i in 0..2 { d.body[i] = add(d.body[i], mul(d.vset[i], dn / 1000.0)); }
        let fresh = match &latest {
            Some((seq, s)) if *seq != read_seq => { read_seq = *seq; cur = Some((s.clone(), now)); true }
            _ => false,
        };
        let Some((c, read_at)) = cur.clone() else { continue };
        if !d.started {
            for (k, &i) in BODIES.iter().enumerate() { for ax in 0..3 { d.body[k][ax] = c.bones[i][ax] as f64; } }
            d.started = true;
        }
        // smooth Lua clock
        let srate = if cfg.lua_v2 && c.rate > 0.0 { c.rate } else { 1.0 };
        let expect = c.pt - c.lead + (now - read_at) * srate;
        let reset = match d.clk {
            None => true,
            Some((pt, at, r)) => (expect - (pt + (now - at) * r)).abs() > cfg.clk_reset_ms,
        };
        let clk = if reset { (expect, now, srate) } else {
            let (pt0, at, r) = d.clk.unwrap();
            let p = pt0 + (now - at) * r;
            // v2: the clock runs at the sidecar's rate, so a stretch is followed without lag
            let r2 = if cfg.lua_v2 { r + 0.3 * (srate - r) } else { 1.0 };
            (p + 0.1 * (expect - p), now, r2)
        };
        if reset && d.clk.is_some() && tt > warm { clock_resets += 1; }
        d.clk = Some(clk);
        d.dsm = d.dsm + 0.15 * (dn - d.dsm);
        let dx = if cfg.lua_v2 { d.dsm } else { dn };
        let xoff = (8.0 + c.delay + dx - cfg.lat_target_ms).clamp(0.0, dx);
        d.xoff_last = Some(xoff);
        // v2: a Hold sample already stands still (zero velocity); only Stale freezes the
        // label, so leaving a hold does not step the shown time back.
        let frozen = if cfg.lua_v2 { c.mode == Mode::Stale } else { matches!(c.mode, Mode::Hold | Mode::Stale) };
        let tget = |i: usize| -> (V3, V3) {
            ([c.bones[i][0] as f64, c.bones[i][1] as f64, c.bones[i][2] as f64],
             [c.vel[i][0] as f64, c.vel[i][1] as f64, c.vel[i][2] as f64])
        };
        if fresh {
            let vv = [tget(PEL).1, tget(HAND).1];
            match d.vprev {
                Some((ppt, pv)) if c.pt - ppt >= 2.0 && c.pt - ppt <= 80.0 => {
                    let k = 1000.0 / (c.pt - ppt);
                    for b in 0..2 {
                        let mut a = mul(sub(vv[b], pv[b]), k);
                        let l = len(a);
                        if l > 40000.0 { a = mul(a, 40000.0 / l); }
                        d.acc[b] = Some(a);
                    }
                }
                Some((ppt, _)) if c.pt - ppt > 80.0 => d.acc = [None, None],
                None => d.acc = [None, None],
                _ => {}
            }
            d.vprev = Some((c.pt, vv));
        }
        let label = clk.0 + xoff;
        let mut ms = (label - c.pt).clamp(-120.0, 120.0);
        let mut ma = (ms + h_pred).clamp(-120.0, 120.0);
        if frozen { ms = 0.0; ma = 0.0; }
        let label = c.pt + ms;
        let mut targets = [[0.0; 3]; 2];
        let mut aims = [([0.0; 3], [0.0; 3]); 2];
        for (b, &i) in BODIES.iter().enumerate() {
            let (p, v) = tget(i);
            let a = if frozen { None } else { d.acc[b] };
            targets[b] = advance(p, v, a, ms).0;
            let e = advance(p, v, a, ma).0;
            let h = (ma - ms) / 1000.0;
            let ff = if h > 1e-4 { mul(sub(e, targets[b]), 1.0 / h) } else { advance(p, v, a, ma).1 };
            aims[b] = (e, ff);
        }
        // metrics on what is on screen now (the bodies after the step that just ran)
        if tt > warm && tt < end {
            if let Some(lab) = d.shown_label.map(|l| l + dn) {
                n_frames += 1;
                let tl = sclk.true_of(lab);
                let truth = mot.at(tl);
                lat.push(tt - tl);
                pel_err.push(len(sub(d.body[0], truth.pos[0])));
                hand_err.push(len(sub(d.body[1], truth.pos[1])));
                if let Some((pl, pp)) = prev_shown {
                    let tp = mot.at(sclk.true_of(pl));
                    let dl = lab - pl;
                    let rate = dl / dn.max(1e-3);
                    if !(0.75..=1.25).contains(&rate) { warp += 1; }
                    for b in 0..2 {
                        let ds = sub(d.body[b], pp[b]);
                        let dt_ = sub(truth.pos[b], tp.pos[b]);
                        let j = len(sub(ds, dt_));
                        if b == 0 { pel_jump.push(j) } else { hand_jump.push(j) }
                        if dbg > 0.0 && j > dbg * if b == 0 { 1.0 } else { 2.5 } {
                            eprintln!("t={:.0} body={} jump={:.1} dn={:.1} mode={:?} fresh={} delay={:.0} age={:.0} lead={:.0} ms={:.1} label-pt={:.1} dl={:.1} clkreset={} truth_v={:.0}",
                                tt, b, j, dn, c.mode, fresh, c.delay, c.age, c.lead, ms, lab - c.pt, dl, reset, len(truth.vel[b]));
                        }
                    }
                    let v = truth.vel[0];
                    if len(v) > 100.0 && dot(sub(d.body[0], pp[0]), v) < 0.0 { backsteps += 1; }
                }
                prev_shown = Some((lab, d.body));
                match c.mode { Mode::Interp => mi += 1, Mode::Extrap => me += 1, _ => mh += 1 }
                delays.push(c.delay);
                jits.push(c.jitter);
                if let Some((r, _)) = c.root {
                    let pel = c.bones[PEL];
                    let dx = (r[0] - pel[0]) as f64;
                    let dy = (r[1] - pel[1]) as f64;
                    root_pel.push((dx * dx + dy * dy).sqrt());
                }
            } else {
                prev_shown = None;
            }
            if late0.is_none() { late0 = Some(pb.stats); }
        }
        // rigid snap rule (sustained pelvis error vs the same-label target)
        let pe = len(sub(d.body[0], targets[0]));
        if pe > 150.0 {
            let since = *d.err_since.get_or_insert(now);
            if now - since >= 150.0 {
                let o = sub(targets[0], d.body[0]);
                for b in 0..2 { d.body[b] = add(d.body[b], o); d.vset[b] = [0.0; 3]; }
                d.err_since = None;
                if tt > warm { rigid += 1; }
                prev_shown = None;
            }
        } else {
            d.err_since = None;
        }
        // servo: feed-forward the chord velocity, correct `gain` of the rest, capped
        let dt = h_pred / 1000.0;
        for b in 0..2 {
            let (aim, ff) = aims[b];
            let need = mul(sub(aim, d.body[b]), 1.0 / dt);
            let mut corr = mul(sub(need, ff), cfg.gain);
            let l = len(corr);
            if l > cfg.cap_lin { corr = mul(corr, cfg.cap_lin / l); }
            d.vset[b] = add(ff, corr);
        }
        // the bodies will stand for `label` + the step that actually runs next
        d.shown_label = Some(label);
        lead_req = (xoff + h_pred + 0.5).floor();
    }
    let st = pb.stats;
    let s0 = late0.unwrap_or(stats0);
    let mins = (end - warm) / 60_000.0;
    let pel_snaps = pel_jump.iter().filter(|j| **j > SNAP_UU).count() as f64;
    let hand_snaps = hand_jump.iter().filter(|j| **j > SNAP_HAND_UU).count() as f64;
    let nm = (mi + me + mh).max(1) as f64;
    Report {
        profile: cfg.down.name.to_string(),
        rtt_ms: cfg.up.delay + cfg.down.delay,
        frames: n_frames,
        latency_p50: pct(&lat, 50.0),
        latency_p95: pct(&lat, 95.0),
        delay_mean: mean(&delays),
        jitter_mean: mean(&jits),
        pel_err_p95: pct(&pel_err, 95.0),
        hand_err_p95: pct(&hand_err, 95.0),
        hand_err_max: pct(&hand_err, 100.0),
        pel_jump_p99: pct(&pel_jump, 99.0),
        pel_jump_max: pct(&pel_jump, 100.0),
        hand_jump_p99: pct(&hand_jump, 99.0),
        hand_jump_max: pct(&hand_jump, 100.0),
        pel_snaps_per_min: pel_snaps / mins,
        hand_snaps_per_min: hand_snaps / mins,
        backsteps_per_min: backsteps as f64 / mins,
        rigid_snaps: rigid,
        clock_resets_per_min: clock_resets as f64 / mins,
        warp_frames_pct: 100.0 * warp as f64 / n_frames.max(1) as f64,
        interp_pct: 100.0 * mi as f64 / nm,
        extrap_pct: 100.0 * me as f64 / nm,
        hold_pct: 100.0 * mh as f64 / nm,
        late: st.late - s0.late,
        cuts: st.cuts - s0.cuts,
        root_pel_p95: pct(&root_pel, 95.0),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn links_lose_and_delay_as_configured() {
        let p = path("far").unwrap();
        let mut l = Link::new(p, 1);
        let (mut n, mut lost, mut sum) = (0, 0, 0.0);
        for k in 0..20_000 {
            let t = k as f64 * 16.7;
            let a = l.send(t);
            n += 1;
            if a.is_empty() { lost += 1; } else { sum += a[0] - t; }
        }
        let loss = lost as f64 / n as f64 * 100.0;
        assert!((1.0..3.0).contains(&loss), "loss {loss}");
        let m = sum / (n - lost) as f64;
        assert!((140.0..175.0).contains(&m), "mean delay {m}");
    }

    #[test]
    fn motion_velocities_match_positions() {
        let m = Motion::generate(10.0, 3);
        for k in (100..9000).step_by(97) {
            let a = m.at(k as f64);
            let b = m.at(k as f64 + 1.0);
            for i in 0..2 {
                let fd = mul(sub(b.pos[i], a.pos[i]), 1000.0);
                assert!(len(sub(fd, a.vel[i])) < 0.05 * len(a.vel[i]) + 30.0, "{k} {i}");
            }
        }
    }

    #[test]
    fn lan_run_tracks_closely() {
        let r = run(&Config::profile("lan", 20.0, 1));
        assert!(r.frames > 900, "{r:?}");
        assert!(r.pel_err_p95 < 5.0, "{}\n{}", Report::header(), r.row());
    }
}
