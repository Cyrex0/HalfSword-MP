//! Synthetic game-side streams for `hsmp-tools ipc-game --synth` (used by `net-bench`).
//!
//! Writes what HSMPSync / HSMPCombat write in a live match, at the game's rates, into the
//! game-owned slots of an HSMP-SHM segment:
//!
//! * the `root` + `weapon` + `pose` records (built as HSMPNative builds them) once per sample (`--synth-hz`, default 60 =
//!   HSMPSync `send_hz`), all stamped with the same sender ts; the control block on every
//!   2nd sample (HSMPSync samples it every other frame);
//! * the `vitals` codec slot (`--synth-vitals-hz`, default 20) with HSMPCombat's table
//!   `{seq, hp, dead, f, v = {19 numbers}, dism = {}}`.
//!
//! The pose is a standing / walking / swinging humanoid built by forward kinematics from
//! the SK_Body_Man reference offsets (`posecodec_v2::REF_T` / `PARENT`, copied here) with
//! sinusoidally varying local rotations; linear and angular velocities are the analytic
//! finite difference of the same motion (1 ms), one weapon is held in the right hand.
//! Everything is a pure function of (seed, time), so two runs with one seed match.
//!
//! The cost of each slot write (the seqlock copy, plus the codec encode for vitals and the
//! doorbell ring) is timed and reported by [`Synth::stats_json`].
//!
//! Self-contained on purpose: a later ABI port replaces only [`Synth::write_sample`] /
//! [`Synth::write_vitals`].

use crate::ipcgame::{game_meta, GameHost};
use hsmp_ipc::header::ctr;
use hsmp_ipc::schema::pose::{PoseBuf, Root, K_POSE, K_ROOT, K_WEAPON, NB};
use hsmp_ipc::schema::combat::{vitals_q, Vitals, K_VITALS};
use hsmp_ipc::schema::RawSlot;
use hsmp_pose::sample::{PoseArgs, BONE_NUMS, CONTROL_NUMS, WEAPON_NUMS};
use serde_json::{json, Value as J};
use std::time::Instant;

pub type V3 = [f64; 3];
/// x, y, z, w (UE FQuat order, as in the pose slots).
pub type Quat = [f64; 4];

/// `posecodec_v2::PARENT`.
pub const PARENT: [usize; NB] = [0, 0, 1, 2, 3, 4, 5, 6, 7, 5, 9, 10, 11, 5, 13, 14, 15, 0, 17, 18, 0, 20, 21];
/// `posecodec_v2::REF_T` (SK_Body_Man reference local translations, uu).
pub const REF_T: [[f64; 3]; NB] = [
    [0.0, 0.0, 0.0],
    [0.0, -0.23, 3.67], [0.0, 1.60, 6.60], [0.0, 1.42, 7.10], [0.0, 0.21, 8.52], [0.0, -3.00, 19.41],
    [0.0, -0.61, 11.87], [0.0, 0.61, 5.02], [0.0, 0.0, 4.91],
    [1.43, -1.60, 5.44], [17.81, 0.0, 0.0], [27.77, -0.01, 0.01], [27.25, 0.0, 0.0],
    [-1.43, -1.60, 5.44], [-17.81, 0.0, 0.0], [-27.77, -0.01, 0.01], [-27.25, 0.0, 0.0],
    [9.97, 0.26, -2.35], [2.36, 2.60, -43.20], [1.76, -2.60, -42.10],
    [-9.97, 0.26, -2.35], [-2.36, 2.60, -43.20], [-1.76, -2.60, -42.10],
];
/// `posecodec_v2::CONTROL_SCALARS` ranges.
const SCALAR_RANGE: [(f64, f64); 16] = [
    (0.0, 100.0), (0.0, 2.0), (0.0, 2.0), (0.0, 2.0), (0.0, 2.0), (0.0, 2.0), (0.0, 2.0), (0.0, 100.0),
    (0.0, 2.0), (0.0, 1.0), (0.0, 1.0), (0.0, 1.0), (0.0, 5.0), (0.0, 5.0), (0.0, 100.0), (0.0, 100.0),
];
/// HSMPCombat `VITALS` count.
pub const N_VITALS: usize = 19;
/// Pelvis height of a standing Willie (uu).
const PELVIS_Z: f64 = 95.0;
const DEG: f64 = std::f64::consts::PI / 180.0;

// ---- small quaternion kit -----------------------------------------------------------

pub fn qmul(a: Quat, b: Quat) -> Quat {
    [
        a[3] * b[0] + a[0] * b[3] + a[1] * b[2] - a[2] * b[1],
        a[3] * b[1] - a[0] * b[2] + a[1] * b[3] + a[2] * b[0],
        a[3] * b[2] + a[0] * b[1] - a[1] * b[0] + a[2] * b[3],
        a[3] * b[3] - a[0] * b[0] - a[1] * b[1] - a[2] * b[2],
    ]
}
pub fn qconj(q: Quat) -> Quat {
    [-q[0], -q[1], -q[2], q[3]]
}
pub fn qrot(q: Quat, v: V3) -> V3 {
    let (x, y, z, w) = (q[0], q[1], q[2], q[3]);
    let t = [2.0 * (y * v[2] - z * v[1]), 2.0 * (z * v[0] - x * v[2]), 2.0 * (x * v[1] - y * v[0])];
    [v[0] + w * t[0] + (y * t[2] - z * t[1]), v[1] + w * t[1] + (z * t[0] - x * t[2]), v[2] + w * t[2] + (x * t[1] - y * t[0])]
}
/// Rotation of `deg` degrees about a unit axis.
pub fn qaxis(axis: V3, deg: f64) -> Quat {
    let h = deg * DEG * 0.5;
    let s = h.sin();
    [axis[0] * s, axis[1] * s, axis[2] * s, h.cos()]
}
fn add(a: V3, b: V3) -> V3 {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}
fn sub(a: V3, b: V3) -> V3 {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}
fn scale(a: V3, k: f64) -> V3 {
    [a[0] * k, a[1] * k, a[2] * k]
}
pub fn dist(a: V3, b: V3) -> f64 {
    let d = sub(a, b);
    (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt()
}
const X: V3 = [1.0, 0.0, 0.0];
const Y: V3 = [0.0, 1.0, 0.0];
const Z: V3 = [0.0, 0.0, 1.0];

/// Angular velocity (deg/s, world) taking `q0` to `q1` in `dt` seconds.
fn ang_vel(q0: Quat, q1: Quat, dt: f64) -> V3 {
    let mut d = qmul(q1, qconj(q0));
    if d[3] < 0.0 {
        d = [-d[0], -d[1], -d[2], -d[3]];
    }
    let s = (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt();
    if s < 1e-12 {
        return [0.0; 3];
    }
    let angle = 2.0 * s.atan2(d[3]);
    scale([d[0] / s, d[1] / s, d[2] / s], angle / DEG / dt)
}

/// UE `FQuat::Rotator()`: (pitch, yaw, roll) in degrees.
pub fn quat_to_rotator(q: Quat) -> V3 {
    let (x, y, z, w) = (q[0], q[1], q[2], q[3]);
    let sing = z * x - w * y;
    let yaw = (2.0 * (w * z + x * y)).atan2(1.0 - 2.0 * (y * y + z * z)) / DEG;
    let pitch = if sing < -0.4999995 {
        -90.0
    } else if sing > 0.4999995 {
        90.0
    } else {
        (2.0 * sing).asin() / DEG
    };
    let roll = (-2.0 * (w * x + y * z)).atan2(1.0 - 2.0 * (x * x + y * y)) / DEG;
    [pitch, yaw, roll]
}

// ---- the motion model ---------------------------------------------------------------

/// Per-player motion parameters (from the seed).
#[derive(Clone, Debug)]
pub struct Motion {
    /// Centre of this player's walk circle (uu).
    pub centre: V3,
    pub radius: f64,
    /// rad/s around the circle (sign = direction).
    pub omega: f64,
    pub phase: f64,
    /// Sword swing rate (Hz).
    pub swing_hz: f64,
}

impl Motion {
    pub fn from_seed(seed: u64) -> Motion {
        // splitmix64: deterministic, no dependency on rand's stream stability
        let mut s = seed.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut next = move || {
            s = s.wrapping_add(0x9E37_79B9_7F4A_7C15);
            let mut z = s;
            z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
            z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
            ((z ^ (z >> 31)) >> 11) as f64 / (1u64 << 53) as f64
        };
        // Players spread over an arena-sized area (~12 m across), walking 100-200 uu/s.
        let a = next() * std::f64::consts::TAU;
        let r = 150.0 + next() * 450.0;
        let radius = 120.0 + next() * 180.0;
        let speed = 100.0 + next() * 100.0;
        let dir = if next() < 0.5 { -1.0 } else { 1.0 };
        Motion {
            centre: [r * a.cos(), r * a.sin(), 0.0],
            radius,
            omega: dir * speed / radius,
            phase: next() * std::f64::consts::TAU,
            swing_hz: 0.6 + next() * 0.8,
        }
    }
}

/// One evaluated body: world transforms of the 23 bones, the held weapon and the root.
#[derive(Clone, Debug)]
pub struct Body {
    pub p: [V3; NB],
    pub q: [Quat; NB],
    pub weapon_p: V3,
    pub weapon_q: Quat,
    /// Actor location (capsule centre ~ pelvis) and facing yaw (deg).
    pub root: V3,
    pub yaw: f64,
}

/// Weapon geometry in weapon space: grip at the origin, blade along +Z.
pub const BLADE_BASE: V3 = [0.0, 0.0, 18.0];
pub const BLADE_TIP: V3 = [0.0, 0.0, 105.0];

impl Motion {
    /// The body at `t` seconds.
    pub fn body(&self, t: f64) -> Body {
        // Walk speed varies (stand still for a moment every ~10 s: factor 0..1).
        let gait = 0.5 + 0.5 * (0.6 * t + self.phase).sin();
        // angular speed omega * (1 + 0.5 sin(..)): the walk speeds up and slows down
        let ang = self.phase + self.omega * (t - 0.5 * (0.6 * t + self.phase).cos() / 0.6);
        let root = [self.centre[0] + self.radius * ang.cos(), self.centre[1] + self.radius * ang.sin(), PELVIS_Z];
        // heading = tangent of the circle
        let yaw = (ang + self.omega.signum() * std::f64::consts::FRAC_PI_2) / DEG;
        let stride = 2.0 * std::f64::consts::PI * 1.6 * t; // ~1.6 steps/s
        let swing = (2.0 * std::f64::consts::PI * self.swing_hz * t + self.phase).sin();
        let s = |f: f64, ph: f64| (f * t + ph + self.phase).sin();

        // Local rotation of each bone relative to its parent (rest = identity: REF_T).
        let mut lq = [[0.0, 0.0, 0.0, 1.0]; NB];
        // The skeleton faces -Y in its rest pose; turn it to face +X, then to `yaw`.
        lq[0] = qmul(qaxis(Z, yaw + 90.0), qmul(qaxis(X, 3.0 * s(1.3, 0.0)), qaxis(Y, 2.0 * (stride * 2.0).sin() * gait)));
        lq[2] = qaxis(Z, 6.0 * s(0.9, 1.0));
        lq[3] = qaxis(Y, 3.0 * s(1.1, 2.0));
        lq[4] = qaxis(Z, 8.0 * swing);
        lq[5] = qaxis(X, 4.0 * s(0.7, 0.5));
        lq[6] = qaxis(X, 5.0 * s(0.5, 0.3));
        lq[8] = qmul(qaxis(Z, 20.0 * s(0.35, 2.2)), qaxis(X, 8.0 * s(0.6, 1.4)));
        // Arms: hang (rest is a T-pose along X), left arm guarding, right arm swinging.
        lq[9] = qaxis(Y, 8.0 * s(1.0, 0.1));
        lq[10] = qmul(qaxis(Y, 65.0), qaxis(Z, 25.0 + 10.0 * s(0.8, 0.7)));
        lq[11] = qaxis(Z, 45.0 + 15.0 * s(1.2, 1.1));
        lq[12] = qaxis(X, 15.0 * s(1.5, 0.4));
        lq[13] = qaxis(Y, -8.0 * s(1.0, 0.9));
        lq[14] = qmul(qaxis(Y, -55.0 + 40.0 * swing), qaxis(Z, -30.0 - 35.0 * swing));
        lq[15] = qaxis(Z, -(30.0 + 30.0 * (swing * 0.5 + 0.5)));
        lq[16] = qmul(qaxis(X, 25.0 * swing), qaxis(Y, 10.0 * s(2.0, 0.2)));
        // Legs: stride (scaled by gait), knees bend on the back swing.
        let st = stride.sin() * gait;
        lq[17] = qaxis(X, 25.0 * st);
        lq[18] = qaxis(X, -(5.0 + 30.0 * (0.5 - 0.5 * stride.cos()) * gait));
        lq[19] = qaxis(X, 10.0 * st);
        lq[20] = qaxis(X, -25.0 * st);
        lq[21] = qaxis(X, -(5.0 + 30.0 * (0.5 + 0.5 * stride.cos()) * gait));
        lq[22] = qaxis(X, -10.0 * st);

        let mut p = [[0.0; 3]; NB];
        let mut q = [[0.0, 0.0, 0.0, 1.0]; NB];
        p[0] = [root[0], root[1], PELVIS_Z + 2.0 * (stride * 2.0).cos() * gait];
        q[0] = lq[0];
        for i in 1..NB {
            let par = PARENT[i];
            q[i] = qmul(q[par], lq[i]);
            p[i] = add(p[par], qrot(q[par], REF_T[i]));
        }
        // Weapon: gripped in hand_r, blade out along the hand's -X (the finger direction of
        // the right arm chain), tilted up.
        let hq = q[16];
        let weapon_q = qmul(hq, qaxis(Y, -90.0));
        let weapon_p = add(p[16], qrot(hq, [-8.0, 0.0, 0.0]));
        Body { p, q, weapon_p, weapon_q, root, yaw }
    }
}

/// One sample's world-space state with velocities (13 floats per bone like the slot).
#[derive(Clone, Debug)]
pub struct Sample {
    pub b: [[f32; 13]; NB],
    pub weapon: [f32; 13],
    pub base: [f32; 3],
    pub tip: [f32; 3],
    pub root: V3,
    pub root_vel: V3,
    pub yaw: f64,
}

fn xf13(p: V3, q: Quat, v: V3, w: V3) -> [f32; 13] {
    [
        p[0] as f32, p[1] as f32, p[2] as f32, q[0] as f32, q[1] as f32, q[2] as f32, q[3] as f32,
        v[0] as f32, v[1] as f32, v[2] as f32, w[0] as f32, w[1] as f32, w[2] as f32,
    ]
}

impl Motion {
    /// The sample at `t` seconds: positions + velocities (finite difference over 1 ms).
    pub fn sample(&self, t: f64) -> Sample {
        const H: f64 = 0.001;
        let a = self.body(t - H);
        let b = self.body(t);
        let mut out = [[0f32; 13]; NB];
        for i in 0..NB {
            out[i] = xf13(b.p[i], b.q[i], scale(sub(b.p[i], a.p[i]), 1.0 / H), ang_vel(a.q[i], b.q[i], H));
        }
        let weapon = xf13(b.weapon_p, b.weapon_q, scale(sub(b.weapon_p, a.weapon_p), 1.0 / H), ang_vel(a.weapon_q, b.weapon_q, H));
        let wb = add(b.weapon_p, qrot(b.weapon_q, BLADE_BASE));
        let wt = add(b.weapon_p, qrot(b.weapon_q, BLADE_TIP));
        Sample {
            b: out,
            weapon,
            base: [wb[0] as f32, wb[1] as f32, wb[2] as f32],
            tip: [wt[0] as f32, wt[1] as f32, wt[2] as f32],
            root: b.root,
            root_vel: scale(sub(b.root, a.root), 1.0 / H),
            yaw: b.yaw,
        }
    }
}

// ---- timing stats -------------------------------------------------------------------

/// Durations of one kind of write, ns.
#[derive(Default)]
pub struct Timing(Vec<u32>);

impl Timing {
    pub fn add(&mut self, t: Instant) {
        self.0.push(t.elapsed().as_nanos().min(u32::MAX as u128) as u32);
    }
    pub fn json(&self) -> J {
        if self.0.is_empty() {
            return json!({"n": 0});
        }
        let mut v = self.0.clone();
        v.sort_unstable();
        let n = v.len();
        let sum: u64 = v.iter().map(|x| *x as u64).sum();
        let p = |q: f64| v[((n as f64 - 1.0) * q).round() as usize] as f64 / 1000.0;
        let r = |x: f64| (x * 1000.0).round() / 1000.0;
        json!({"n": n, "avg_us": r(sum as f64 / n as f64 / 1000.0), "p50_us": r(p(0.5)), "p99_us": r(p(0.99)), "max_us": r(v[n - 1] as f64 / 1000.0)})
    }
}

// ---- the writer ---------------------------------------------------------------------

pub struct Synth {
    pub hz: f64,
    pub vitals_hz: f64,
    pub seed: u64,
    pub motion: Motion,
    t0: Instant,
    /// The sender clock origin (ms): HSMPSync's ts is game time, never 0 in a match.
    ts0: f64,
    started: Option<Instant>,
    next_ms: f64,
    next_vitals_ms: f64,
    tick: u32,
    vitals_seq: i64,
    pub root_w: Timing,
    pub weapon_w: Timing,
    pub pose_w: Timing,
    pub vitals_enc: Timing,
    pub vitals_w: Timing,
    pub ring: Timing,
    /// root + weapon + pose + ring of one sample (the per-frame cost).
    pub sample_total: Timing,
    late: u64,
    scratch: Vec<u64>,
    enc: hsmp_pose::sample::Scratch,
    pose_buf: Box<PoseBuf>,
    pose_bytes: u64,
}

/// One sample's `put_pose` arguments.
pub struct PoseIn {
    pub b: [f64; BONE_NUMS],
    pub w: [f64; WEAPON_NUMS],
    pub c: Option<[f64; CONTROL_NUMS]>,
}

impl Synth {
    pub fn new(hz: f64, vitals_hz: f64, seed: u64) -> Synth {
        Synth {
            hz: hz.clamp(1.0, 1000.0),
            vitals_hz: vitals_hz.clamp(0.0, 1000.0),
            seed,
            motion: Motion::from_seed(seed),
            t0: Instant::now(),
            ts0: 50_000.0 + (seed % 1000) as f64 * 17.0,
            started: None,
            next_ms: 0.0,
            next_vitals_ms: 0.0,
            tick: 0,
            vitals_seq: 0,
            root_w: Timing::default(),
            weapon_w: Timing::default(),
            pose_w: Timing::default(),
            vitals_enc: Timing::default(),
            vitals_w: Timing::default(),
            ring: Timing::default(),
            sample_total: Timing::default(),
            late: 0,
            scratch: Vec::new(),
            enc: hsmp_pose::sample::Scratch::default(),
            pose_buf: PoseBuf::new_boxed(),
            pose_bytes: 0,
        }
    }

    fn now_ms(&self) -> f64 {
        self.t0.elapsed().as_secs_f64() * 1000.0
    }

    pub fn started(&self) -> bool {
        self.started.is_some()
    }

    /// Start writing (the session is live).
    pub fn start(&mut self) {
        if self.started.is_none() {
            self.started = Some(Instant::now());
            let n = self.now_ms();
            self.next_ms = n;
            self.next_vitals_ms = n;
        }
    }

    /// Write whatever is due (drift-free schedule like HSMPSync's fast_send).
    pub fn step(&mut self, host: &GameHost) {
        if self.started.is_none() {
            return;
        }
        let n = self.now_ms();
        let iv = 1000.0 / self.hz;
        if n >= self.next_ms {
            if n - self.next_ms > iv {
                self.late += 1;
            }
            self.next_ms = (self.next_ms + iv).max(n - iv);
            self.write_sample(host, n);
        }
        if self.vitals_hz > 0.0 && n >= self.next_vitals_ms {
            let viv = 1000.0 / self.vitals_hz;
            self.next_vitals_ms = (self.next_vitals_ms + viv).max(n - viv);
            self.write_vitals(host, n);
        }
    }

    /// The `put_pose` arguments of sample `tick` (bones, the held weapon, the control layer on
    /// even ticks), as HSMPSync passes them (Lua numbers).
    pub fn pose_args(&self, s: &Sample, tick: u32, ts: f64) -> PoseIn {
        let mut p = PoseIn { b: [0.0; BONE_NUMS], w: [0.0; WEAPON_NUMS], c: None };
        for (i, b) in s.b.iter().enumerate() {
            for (k, x) in b.iter().enumerate() {
                p.b[i * 13 + k] = *x as f64;
            }
        }
        p.w[0] = 1.0;
        p.w[1] = 3.0;
        for k in 0..13 {
            p.w[2 + k] = s.weapon[k] as f64;
        }
        for k in 0..3 {
            p.w[15 + k] = s.base[k] as f64;
            p.w[18 + k] = s.tip[k] as f64;
        }
        if tick % 2 == 0 {
            let t = ts / 1000.0;
            let mut c = [0f64; CONTROL_NUMS];
            let guard = (t * 0.5).sin() > 0.3;
            // R_Guarding + Any_Guarding while guarding, now and then Threatening Stance
            let threat = (t * 0.21).sin() > 0.9;
            c[0] = ((if guard { 0b101 } else { 0 }) | (if threat { 1 << 26 } else { 0 })) as f64;
            c[1] = 1.0;
            c[2] = 0.0;
            for (i, (lo, hi)) in SCALAR_RANGE.iter().enumerate() {
                c[3 + i] = lo + (hi - lo) * (0.5 + 0.4 * (t * (0.1 + 0.03 * i as f64) + i as f64).sin());
            }
            let pitch = 10.0 * (t * 0.4).sin();
            let (cy, sy, cp, sp) = ((s.yaw * DEG).cos(), (s.yaw * DEG).sin(), (pitch * DEG).cos(), (pitch * DEG).sin());
            c[19..22].copy_from_slice(&[cp * cy, cp * sy, sp]);
            c[22] = pitch;
            c[23] = s.yaw;
            for (j, i) in [16usize, 12, 15, 11].iter().enumerate() {
                for k in 0..3 {
                    c[24 + j * 3 + k] = s.b[*i][k] as f64;
                }
            }
            c[36] = 15.0;
            p.c = Some(c);
        }
        p
    }

    /// Root + weapon + pose of one sample, stamped with one ts, then the doorbell. Each record
    /// is built exactly as HSMPNative's `put_root` / `put_weapon` / `put_pose` build it
    /// (`hsmp_pose::sample`: rotator -> quaternion, the codec v2 encode) and published into its
    /// slot; the timings are that whole game-side cost (minus reading the Lua arguments).
    pub fn write_sample(&mut self, host: &GameHost, now_ms: f64) {
        self.tick = self.tick.wrapping_add(1);
        let tick = self.tick;
        let tsf = self.ts0 + now_ms;
        let ts_int = tsf.floor(); // HSMPSync passes integer ms to put_root / put_weapon
        let s = self.motion.sample(now_ms / 1000.0);
        let seg = host.seg();
        let go = &seg.game_out;
        let rot = quat_to_rotator([s.weapon[3] as f64, s.weapon[4] as f64, s.weapon[5] as f64, s.weapon[6] as f64]);
        let ra = [tick as f64, ts_int, s.root[0], s.root[1], s.root[2], 0.0, s.yaw as f32 as f64, 0.0,
            s.root_vel[0] as f32 as f64, s.root_vel[1] as f32 as f64, s.root_vel[2] as f32 as f64];
        let wa = [tick as f64, ts_int, (1000 + self.seed % 50) as f64, 1.0, s.weapon[0] as f64, s.weapon[1] as f64, s.weapon[2] as f64,
            rot[0], rot[1], rot[2], s.weapon[7] as f64, s.weapon[8] as f64, s.weapon[9] as f64];
        let pin = self.pose_args(&s, tick, tsf);
        let wall = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis() as u64);

        let t_all = Instant::now();
        let t = Instant::now();
        let r = hsmp_pose::sample::root(&ra, wall);
        go.local_root.put(game_meta(seg, tick), K_ROOT, bytemuck::bytes_of(&r), &mut self.scratch);
        self.root_w.add(t);
        let t = Instant::now();
        let w = hsmp_pose::sample::weapon(&wa);
        go.local_weapon.put(game_meta(seg, tick), K_WEAPON, bytemuck::bytes_of(&w), &mut self.scratch);
        self.weapon_w.add(t);
        let t = Instant::now();
        let a = PoseArgs { tick: tick as f64, ts: tsf, dt: 1000.0 / self.hz, b: &pin.b, w: std::slice::from_ref(&pin.w), c: pin.c.as_ref() };
        if hsmp_pose::sample::encode_pose(&a, &mut self.enc, &mut self.pose_buf) {
            go.local_pose.put(game_meta(seg, tick), K_POSE, self.pose_buf.payload(), &mut self.scratch);
            self.pose_bytes += self.pose_buf.payload_len() as u64;
        }
        self.pose_w.add(t);
        seg.header.game.count(ctr::SLOT_WRITES, 3);
        let t = Instant::now();
        host.ring();
        self.ring.add(t);
        self.sample_total.add(t_all);
    }

    /// HSMPCombat's vitals record (`vitals`, quantised at the source).
    pub fn vitals_value(&self, seq: i64, t: f64) -> Vitals {
        let ph = (self.seed % 97) as f64;
        let mut v = [0f64; N_VITALS];
        // Health / limb health: slow damage with regen; consciousness, stamina, exhaustion,
        // bleeding, blood rate, pain, fallen rate move continuously (as in a fight).
        v[0] = 100.0 - 30.0 * (0.5 + 0.5 * (t * 0.05 + ph).sin());
        for (i, x) in v.iter_mut().enumerate().take(11).skip(1) {
            *x = 100.0 - 40.0 * (0.5 + 0.5 * (t * (0.03 + 0.004 * i as f64) + ph + i as f64).sin());
        }
        v[11] = 100.0 - 20.0 * (0.5 + 0.5 * (t * 0.2 + ph).sin());
        v[12] = 100.0 - 15.0 * (0.5 + 0.5 * (t * 0.17 + ph).sin());
        v[13] = 50.0 + 45.0 * (t * 0.9 + ph).sin();
        v[14] = 20.0 + 15.0 * (t * 0.3 + ph).sin();
        v[15] = (2.0 * (t * 0.4 + ph).sin()).max(0.0);
        v[16] = (1.0 * (t * 0.35 + ph).sin()).max(0.0);
        v[17] = 30.0 + 25.0 * (t * 0.7 + ph).sin();
        v[18] = (0.5 * (t * 0.25 + ph).sin()).max(0.0);
        let mut r = Vitals { seq: seq as u32, ..Default::default() };
        for (i, x) in v.iter().enumerate() {
            r.v[i] = vitals_q(i, *x as f32);
        }
        r
    }

    pub fn write_vitals(&mut self, host: &GameHost, now_ms: f64) {
        self.vitals_seq += 1;
        let seg = host.seg();
        let t = Instant::now();
        // The game quantises once (the native module's marshal of HSMPCombat's table).
        let val = self.vitals_value(self.vitals_seq, now_ms / 1000.0);
        self.vitals_enc.add(t);
        if let Some(slot) = seg.slot_ref("vitals", 0) {
            let t = Instant::now();
            slot.put(game_meta(seg, 0), K_VITALS, hsmp_ipc::bytemuck::bytes_of(&val), &mut self.scratch);
            self.vitals_w.add(t);
            seg.header.game.count(ctr::SLOT_WRITES, 1);
            host.ring();
        }
    }

    pub fn stats_json(&self) -> J {
        let secs = self.started.map_or(0.0, |s| s.elapsed().as_secs_f64());
        json!({"ev": "synth_stats", "seed": self.seed, "hz": self.hz, "vitals_hz": self.vitals_hz, "secs": (secs * 1000.0).round() / 1000.0,
               "samples": self.tick, "vitals": self.vitals_seq, "late": self.late,
               "achieved_hz": if secs > 0.0 { (self.tick as f64 / secs * 100.0).round() / 100.0 } else { 0.0 },
               "local_pose_bytes_avg": if self.tick > 0 { (self.pose_bytes as f64 / self.tick as f64).round() } else { 0.0 },
               "local_root_bytes": std::mem::size_of::<Root>(), "local_weapon_bytes": std::mem::size_of::<hsmp_ipc::schema::pose::Weapon>(),
               "write_includes": "record build (rotator->quat, codec v2 encode) + slot put, as HSMPNative",
               "write": {"local_root": self.root_w.json(), "local_weapon": self.weapon_w.json(), "local_pose": self.pose_w.json(),
                         "vitals_encode": self.vitals_enc.json(), "vitals": self.vitals_w.json(), "doorbell": self.ring.json(),
                         "sample_total": self.sample_total.json()}})
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ipcgame::GameHost;

    #[test]
    fn synth_pose_is_finite_and_humanoid() {
        let m = Motion::from_seed(7);
        let mut last_root: Option<V3> = None;
        for i in 0..600 {
            let t = i as f64 / 60.0;
            let s = m.sample(t);
            for (bi, b) in s.b.iter().enumerate() {
                assert!(b.iter().all(|x| x.is_finite()), "bone {bi} at {t}: {b:?}");
                let qn = (b[3] * b[3] + b[4] * b[4] + b[5] * b[5] + b[6] * b[6]).sqrt();
                assert!((qn - 1.0).abs() < 1e-3, "quat norm {qn}");
                // velocities in the codec range (10000 uu/s, 5000 deg/s)
                assert!(b[7..10].iter().all(|v| v.abs() < 3000.0), "lin vel {b:?}");
                assert!(b[10..13].iter().all(|v| v.abs() < 2000.0), "ang vel {b:?}");
            }
            // Bone lengths equal the reference offsets (FK, k = 1): within float precision.
            for bi in 1..NB {
                let par = PARENT[bi];
                let d = dist([s.b[bi][0] as f64, s.b[bi][1] as f64, s.b[bi][2] as f64], [s.b[par][0] as f64, s.b[par][1] as f64, s.b[par][2] as f64]);
                let r = (REF_T[bi][0].powi(2) + REF_T[bi][1].powi(2) + REF_T[bi][2].powi(2)).sqrt();
                assert!((d - r).abs() < 0.01, "bone {bi}: {d} vs {r}");
            }
            // Standing: head above pelvis above feet; head 150-180 uu, feet near the floor.
            let z = |i: usize| s.b[i][2] as f64;
            assert!(z(8) > 140.0 && z(8) < 185.0, "head z {}", z(8));
            assert!(z(19) > -5.0 && z(19) < 40.0 && z(22) > -5.0 && z(22) < 40.0, "feet {} {}", z(19), z(22));
            assert!(z(0) > 85.0 && z(0) < 105.0);
            // the weapon is in the right hand and the blade is ~87 uu long
            let wp = [s.weapon[0] as f64, s.weapon[1] as f64, s.weapon[2] as f64];
            assert!(dist(wp, [s.b[16][0] as f64, s.b[16][1] as f64, s.b[16][2] as f64]) < 10.0);
            let bl = dist([s.base[0] as f64, s.base[1] as f64, s.base[2] as f64], [s.tip[0] as f64, s.tip[1] as f64, s.tip[2] as f64]);
            assert!((bl - 87.0).abs() < 0.01, "blade {bl}");
            // root moves continuously at walking speed
            if let Some(lr) = last_root {
                assert!(dist(lr, s.root) < 300.0 / 60.0 + 1.0, "root jump {}", dist(lr, s.root));
            }
            let sp = (s.root_vel[0].powi(2) + s.root_vel[1].powi(2)).sqrt();
            assert!(sp < 350.0, "speed {sp}");
            last_root = Some(s.root);
        }
        // and it moves (not a frozen pose)
        let (a, b) = (m.sample(0.0), m.sample(0.5));
        assert!((a.b[16][0] - b.b[16][0]).abs() + (a.b[16][1] - b.b[16][1]).abs() + (a.b[16][2] - b.b[16][2]).abs() > 1.0);
    }

    #[test]
    fn velocities_match_motion() {
        let m = Motion::from_seed(3);
        let dt = 1.0 / 60.0;
        let (a, b) = (m.sample(2.0), m.sample(2.0 + dt));
        for i in [0usize, 8, 16, 22] {
            let d = [(b.b[i][0] - a.b[i][0]) as f64 / dt, (b.b[i][1] - a.b[i][1]) as f64 / dt, (b.b[i][2] - a.b[i][2]) as f64 / dt];
            let v = [a.b[i][7] as f64, a.b[i][8] as f64, a.b[i][9] as f64];
            assert!(dist(d, v) < 60.0, "bone {i}: fd {d:?} vs v {v:?}");
        }
    }

    #[test]
    fn writes_land_in_the_slots_and_are_timed() {
        let g = GameHost::anonymous(u64::MAX >> 1).unwrap();
        let mut s = Synth::new(60.0, 20.0, 5);
        s.write_sample(&g, 1000.0);
        s.write_sample(&g, 1016.7);
        s.write_vitals(&g, 1000.0);
        let go = &g.seg().game_out;
        let (_, _, b) = crate::ipcgame::read_slot(&go.local_root).unwrap();
        let r = hsmp_ipc::record::view::<Root>(&b).unwrap().head();
        assert_eq!(r.tick, 2);
        assert!(r.pos[2] > 80.0);
        let (_, _, b) = crate::ipcgame::read_slot(&go.local_pose).unwrap();
        let v = hsmp_ipc::record::view::<hsmp_ipc::schema::pose::PoseHead>(&b).unwrap();
        assert_eq!(v.head.tick, 2);
        let d = hsmp_pose::posecodec::v2::decode(&v.rows).expect("v2 frame");
        assert!(d.control.is_some(), "control on even ticks");
        assert_eq!(d.weapons.len(), 1);
        let (v, _) = go.local_vitals.read().unwrap();
        let r = hsmp_ipc::record::view::<Vitals>(v.payload()).expect("a valid vitals record");
        assert_eq!(r.head.seq, 1);
        assert!(r.head.health().is_some());
        let st = s.stats_json();
        assert_eq!(st["write"]["local_pose"]["n"], 2);
        assert_eq!(st["write"]["vitals"]["n"], 1);
    }

    #[test]
    fn rotator_of_yaw_quat() {
        let r = quat_to_rotator(qaxis(Z, 30.0));
        assert!((r[1] - 30.0).abs() < 1e-9 && r[0].abs() < 1e-9 && r[2].abs() < 1e-9, "{r:?}");
    }
}
