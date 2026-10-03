//! Pose codec v2: the full physical state of a Willie in one frame.
//!
//! Carried in the same pose body as v1 (`C2SSkeletalState.bones` /
//! `S2CSkeletalBroadcast.bones`); a v2 frame starts with the magic
//! `FF FF FF 02` (a v1 frame starts with its u32 sender ms, which never
//! reaches 0xFFFFFF00). `posecodec::decode` maps v2 frames onto the v1
//! `PoseFrame` (16 hero bones + weapon slot) so lag compensation keeps
//! working unchanged; `decode` here returns everything.
//!
//! What a frame holds (see docs/development/subsystems/replication.md for the byte budget):
//!   * the 22 simulated bodies of the visible mesh (`CharacterMesh0`) plus
//!     `spine_01` (body-less, links pelvis and spine_02): pelvis world pose,
//!     every other bone as a rotation relative to its parent (smallest-three,
//!     13 bit), positions by forward kinematics from the SK_Body_Man
//!     reference offsets × the sender's skeleton scale `k`, with an exact
//!     per-bone override wherever the live offset differs by > 0.1 uu
//!     (dislocation, joint slop, another skeleton);
//!   * per-body linear velocity (at the bone origin, uu/s) and angular
//!     velocity (deg/s), 8-bit µ-law;
//!   * up to two held weapons: world pose, linear/angular velocity, blade
//!     base/tip in weapon space, grip hand(s), class id;
//!   * optionally (every other frame) the control layer: guard/attack/grip/
//!     fallen flags, grip types, muscle tonus / motor strengths, aim, control
//!     rotation and the two-bone arm IK targets.
//!
//! The encoder works closed-loop: each child's local rotation and translation
//! are measured against the parent pose the DECODER will rebuild, so
//! quantisation never accumulates down a chain (hand error = its own
//! quantisation only).

#![allow(dead_code)]

pub const MAGIC: [u8; 4] = [0xFF, 0xFF, 0xFF, 0x02];

/// Bones of a v2 frame, parents first. Real (lower-case) Willie names.
pub const BONES: [&str; 23] = [
    "pelvis", "spine_01", "spine_02", "spine_03", "spine_04", "spine_05",
    "neck_01", "neck_02", "head",
    "clavicle_l", "upperarm_l", "lowerarm_l", "hand_l",
    "clavicle_r", "upperarm_r", "lowerarm_r", "hand_r",
    "thigh_l", "calf_l", "foot_l",
    "thigh_r", "calf_r", "foot_r",
];
pub const NB: usize = 23;
pub const PARENT: [usize; NB] = [0, 0, 1, 2, 3, 4, 5, 6, 7, 5, 9, 10, 11, 5, 13, 14, 15, 0, 17, 18, 0, 20, 21];
/// Every bone but spine_01 is a simulated body (in-game: IsSimulatingPhysics).
pub const HAS_BODY: [bool; NB] = [true, false, true, true, true, true, true, true, true, true, true, true, true,
    true, true, true, true, true, true, true, true, true, true];
/// SK_Body_Man reference local translations (GetRefPosePosition), uu.
pub const REF_T: [[f32; 3]; NB] = [
    [0.0, 0.0, 0.0],
    [0.0, -0.23, 3.67], [0.0, 1.60, 6.60], [0.0, 1.42, 7.10], [0.0, 0.21, 8.52], [0.0, -3.00, 19.41],
    [0.0, -0.61, 11.87], [0.0, 0.61, 5.02], [0.0, 0.0, 4.91],
    [1.43, -1.60, 5.44], [17.81, 0.0, 0.0], [27.77, -0.01, 0.01], [27.25, 0.0, 0.0],
    [-1.43, -1.60, 5.44], [-17.81, 0.0, 0.0], [-27.77, -0.01, 0.01], [-27.25, 0.0, 0.0],
    [9.97, 0.26, -2.35], [2.36, 2.60, -43.20], [1.76, -2.60, -42.10],
    [-9.97, 0.26, -2.35], [-2.36, 2.60, -43.20], [-1.76, -2.60, -42.10],
];
/// v2 bone index of each v1 hero bone (posecodec::HERO_BONES order).
pub const HERO_TO_V2: [usize; 16] = [0, 2, 4, 8, 10, 11, 12, 14, 15, 16, 17, 18, 19, 20, 21, 22];

/// Control-layer flag bits (Willie_BP_C property names, docs/development/halfsword/willie_props.txt).
pub const CONTROL_FLAGS: [&str; 32] = [
    "R_Guarding", "L_Guarding", "Any_Guarding", "Parrying R", "Parrying L",
    "R_Thrusting", "L_Thrusting", "Alt Thrusting", "Kicking  R", "Kicking  L",
    "Fallen", "Downed", "R Kneel", "L Kneel", "Is Crouched", "Dodging",
    "R Two Handed Grip", "R Alt Grip", "Mordhau Grip", "Master Stroke Grip",
    "R Hand Reverse Grip", "L Hand Reverse Grip", "Pain Shock", "Being Grabbed",
    "Grabbed R", "Grabbed L", "Threatening Stance", "R Down", "L Down",
    "Classic Half Swording Toggle", "L Hand In Offhand Attached", "R Kneel Falling",
];
/// Control-layer scalars: (property, min, max), 8 bit each.
pub const CONTROL_SCALARS: [(&str, f32, f32); 16] = [
    ("All Body Tonus", 0.0, 100.0), ("Upper Body Tonus", 0.0, 2.0), ("Arm R Tonus", 0.0, 2.0),
    ("Arm L Tonus", 0.0, 2.0), ("Leg R Tonus", 0.0, 2.0), ("Leg L Tonus", 0.0, 2.0),
    ("Head Tonus", 0.0, 2.0), ("Muscle Power", 0.0, 100.0), ("Constraint Rate", 0.0, 2.0),
    ("R Thrust Alpha", 0.0, 1.0), ("Fallen Rate", 0.0, 1.0), ("Get Up Rate", 0.0, 1.0),
    ("Root Linear Constraint Power", 0.0, 5.0), ("Root Angular Constraint Power", 0.0, 5.0),
    ("Consciousness", 0.0, 100.0), ("Stamina", 0.0, 100.0),
];
/// IK vectors carried in the control layer, in this order.
pub const CONTROL_IK: [&str; 4] = ["R Out End Pos", "L Out End Pos", "R Out Joint Pos", "L Out Joint Pos"];

const VLIN_MAX: f32 = 10000.0; // uu/s
const VANG_MAX: f32 = 5000.0; // deg/s
const MU: f32 = 255.0;
const OVERRIDE_UU: f32 = 0.1;

pub type Quat = [f32; 4];
pub type V3 = [f32; 3];

#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Bone { pub p: V3, pub q: Quat, pub v: V3, pub w: V3 }

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Weapon {
    /// 1 = right hand, 2 = left hand, 3 = both (two-handed).
    pub hands: u8,
    pub id: u8,
    pub p: V3, pub q: Quat, pub v: V3, pub w: V3,
    /// Blade base and tip in weapon space (uu).
    pub base: V3, pub tip: V3,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Control {
    pub flags: u32,
    pub grip_r: u8, pub grip_l: u8,
    pub scalars: [f32; 16],
    pub aim: V3,
    pub ctrl_pitch: f32, pub ctrl_yaw: f32,
    pub ik: [V3; 4],
    /// true: ik[i] is a world position; false: raw (component) value.
    pub ik_world: [bool; 4],
}

#[derive(Debug, Clone, PartialEq)]
pub struct Full {
    /// Sender clock, ms (sub-ms precise).
    pub ts: f64,
    /// Sender skeleton scale (live offset / reference offset).
    pub k: f32,
    pub bones: [Bone; NB],
    pub weapons: Vec<Weapon>,
    pub control: Option<Control>,
    /// Bones whose translation was sent explicitly (diagnostics).
    pub overrides: u32,
    /// The sender's physics step (ms) that produced this state: the bodies
    /// stand for ts + step (ts = the frame start). Optional trailing byte,
    /// flags bit 3; older decoders never read it.
    pub step: f32,
}

impl Default for Full {
    fn default() -> Self {
        Full { ts: 0.0, k: 1.0, bones: [Bone { q: [0.0, 0.0, 0.0, 1.0], ..Default::default() }; NB],
               weapons: Vec::new(), control: None, overrides: 0, step: 0.0 }
    }
}

// ---- math -------------------------------------------------------------------

pub fn qmul(a: Quat, b: Quat) -> Quat {
    [a[3] * b[0] + a[0] * b[3] + a[1] * b[2] - a[2] * b[1],
     a[3] * b[1] - a[0] * b[2] + a[1] * b[3] + a[2] * b[0],
     a[3] * b[2] + a[0] * b[1] - a[1] * b[0] + a[2] * b[3],
     a[3] * b[3] - a[0] * b[0] - a[1] * b[1] - a[2] * b[2]]
}
pub fn qconj(q: Quat) -> Quat { [-q[0], -q[1], -q[2], q[3]] }
pub fn qrot(q: Quat, v: V3) -> V3 {
    let (x, y, z, w) = (q[0], q[1], q[2], q[3]);
    let tx = 2.0 * (y * v[2] - z * v[1]);
    let ty = 2.0 * (z * v[0] - x * v[2]);
    let tz = 2.0 * (x * v[1] - y * v[0]);
    [v[0] + w * tx + (y * tz - z * ty), v[1] + w * ty + (z * tx - x * tz), v[2] + w * tz + (x * ty - y * tx)]
}
pub fn qnorm(q: Quat) -> Quat {
    let l = (q.iter().map(|c| c * c).sum::<f32>()).sqrt();
    if !(l > 1e-6) || !l.is_finite() { return [0.0, 0.0, 0.0, 1.0]; }
    [q[0] / l, q[1] / l, q[2] / l, q[3] / l]
}
/// Angle between two rotations, degrees.
pub fn qangle_deg(a: Quat, b: Quat) -> f32 {
    let d = (a[0] * b[0] + a[1] * b[1] + a[2] * b[2] + a[3] * b[3]).abs().min(1.0);
    2.0 * d.acos().to_degrees()
}
fn sub(a: V3, b: V3) -> V3 { [a[0] - b[0], a[1] - b[1], a[2] - b[2]] }
fn add(a: V3, b: V3) -> V3 { [a[0] + b[0], a[1] + b[1], a[2] + b[2]] }
fn scale(a: V3, k: f32) -> V3 { [a[0] * k, a[1] * k, a[2] * k] }
pub fn dist(a: V3, b: V3) -> f32 { let d = sub(a, b); (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt() }

// ---- bit stream -------------------------------------------------------------

struct BitW { out: Vec<u8>, acc: u64, n: u32 }
impl BitW {
    fn new(out: Vec<u8>) -> Self { BitW { out, acc: 0, n: 0 } }
    fn put(&mut self, v: u32, bits: u32) {
        debug_assert!(bits <= 32);
        let m = if bits == 32 { u32::MAX } else { (1u32 << bits) - 1 };
        self.acc |= ((v & m) as u64) << self.n;
        self.n += bits;
        while self.n >= 8 { self.out.push(self.acc as u8); self.acc >>= 8; self.n -= 8; }
    }
    fn put_i(&mut self, v: i32, bits: u32) { self.put(v as u32, bits) }
    fn finish(mut self) -> Vec<u8> { if self.n > 0 { self.out.push(self.acc as u8); } self.out }
}
struct BitR<'a> { b: &'a [u8], pos: usize, acc: u64, n: u32 }
impl<'a> BitR<'a> {
    fn new(b: &'a [u8]) -> Self { BitR { b, pos: 0, acc: 0, n: 0 } }
    fn get(&mut self, bits: u32) -> Option<u32> {
        while self.n < bits {
            let byte = *self.b.get(self.pos)?;
            self.pos += 1;
            self.acc |= (byte as u64) << self.n;
            self.n += 8;
        }
        let m = if bits == 32 { u32::MAX as u64 } else { (1u64 << bits) - 1 };
        let v = (self.acc & m) as u32;
        self.acc >>= bits;
        self.n -= bits;
        Some(v)
    }
    fn get_i(&mut self, bits: u32) -> Option<i32> {
        let v = self.get(bits)?;
        let sh = 32 - bits;
        Some(((v << sh) as i32) >> sh)
    }
}

fn qi(x: f32, step: f32, bits: u32) -> i32 {
    let lim = (1i32 << (bits - 1)) - 1;
    let v = (x / step).round();
    if !v.is_finite() { return 0; }
    (v as i64).clamp(-(lim as i64), lim as i64) as i32
}

/// Smallest-three quaternion: 2-bit index of the dropped component + 3 × bits.
fn put_quat(w: &mut BitW, q: Quat, bits: u32) -> Quat {
    let mut q = qnorm(q);
    let mut li = 0usize;
    for i in 1..4 { if q[i].abs() > q[li].abs() { li = i; } }
    if q[li] < 0.0 { for c in q.iter_mut() { *c = -*c; } }
    w.put(li as u32, 2);
    let step = std::f32::consts::FRAC_1_SQRT_2 / ((1u32 << (bits - 1)) - 1) as f32;
    let mut rec = [0f32; 4];
    let mut sum = 0.0;
    for i in 0..4 {
        if i == li { continue; }
        let v = qi(q[i], step, bits);
        w.put_i(v, bits);
        rec[i] = v as f32 * step;
        sum += rec[i] * rec[i];
    }
    rec[li] = (1.0 - sum).max(0.0).sqrt();
    qnorm(rec)
}
fn get_quat(r: &mut BitR, bits: u32) -> Option<Quat> {
    let li = r.get(2)? as usize;
    let step = std::f32::consts::FRAC_1_SQRT_2 / ((1u32 << (bits - 1)) - 1) as f32;
    let mut q = [0f32; 4];
    let mut sum = 0.0;
    for i in 0..4 {
        if i == li { continue; }
        q[i] = r.get_i(bits)? as f32 * step;
        sum += q[i] * q[i];
    }
    q[li] = (1.0 - sum).max(0.0).sqrt();
    Some(qnorm(q))
}

/// 8-bit µ-law (sign + 7-bit magnitude), range ±max.
fn mu_enc(x: f32, max: f32) -> u32 {
    if !x.is_finite() { return 0; }
    let a = (x.abs() / max).min(1.0);
    let m = ((1.0 + MU * a).ln() / (1.0 + MU).ln() * 127.0).round() as u32;
    (if x < 0.0 { 0x80 } else { 0 }) | m.min(127)
}
fn mu_dec(c: u32, max: f32) -> f32 {
    let m = (c & 0x7F) as f32 / 127.0;
    let a = ((1.0 + MU).powf(m) - 1.0) / MU * max;
    if c & 0x80 != 0 { -a } else { a }
}
/// [`put_vel`] without rebuilding the decoded value (the encoder does not need it).
fn put_vel_enc(w: &mut BitW, v: V3, max: f32) {
    for c in v { w.put(mu_enc(c, max), 8); }
}
fn put_vel(w: &mut BitW, v: V3, max: f32) -> V3 {
    let mut out = [0f32; 3];
    for i in 0..3 { let c = mu_enc(v[i], max); w.put(c, 8); out[i] = mu_dec(c, max); }
    out
}
fn get_vel(r: &mut BitR, max: f32) -> Option<V3> {
    Some([mu_dec(r.get(8)?, max), mu_dec(r.get(8)?, max), mu_dec(r.get(8)?, max)])
}
fn put_v3(w: &mut BitW, v: V3, step: f32, bits: u32) -> V3 {
    let mut out = [0f32; 3];
    for i in 0..3 { let q = qi(v[i], step, bits); w.put_i(q, bits); out[i] = q as f32 * step; }
    out
}
fn get_v3(r: &mut BitR, step: f32, bits: u32) -> Option<V3> {
    Some([r.get_i(bits)? as f32 * step, r.get_i(bits)? as f32 * step, r.get_i(bits)? as f32 * step])
}

// ---- encode / decode ----------------------------------------------------------

const ROOT_QBITS: u32 = 15;
const QBITS: u32 = 13;
const OVR_STEP: f32 = 1.0 / 64.0;
const WPOS_STEP: f32 = 1.0 / 8.0;
const BLADE_STEP: f32 = 1.0 / 16.0;
const IK_STEP: f32 = 0.5;

pub fn is_v2(buf: &[u8]) -> bool { buf.len() >= 4 && buf[..4] == MAGIC }

/// Encode a full state (world-space bones). Returns the frame and the
/// state the decoder will rebuild (for tests / the probe).
pub fn encode(f: &Full) -> Vec<u8> {
    let mut out = Vec::with_capacity(400);
    encode_into(f, &mut out);
    out
}

/// [`encode`] into `out` (cleared first; its capacity is reused).
pub fn encode_into(f: &Full, out: &mut Vec<u8>) {
    out.clear();
    out.extend_from_slice(&MAGIC);
    let ts = if f.ts.is_finite() && f.ts >= 0.0 { f.ts } else { 0.0 };
    let ms = ts.floor();
    out.extend_from_slice(&((ms as u64 & 0xFFFF_FFFF) as u32).to_le_bytes());
    out.push(((ts - ms) * 256.0).floor().clamp(0.0, 255.0) as u8);
    let nw = f.weapons.len().min(2);
    let step = if f.step.is_finite() { f.step.round().clamp(0.0, 255.0) as u8 } else { 0 };
    let flags = (f.control.is_some() as u8) | ((nw as u8) << 1) | (((step > 0) as u8) << 3);
    out.push(flags);
    let pel = f.bones[0].p;
    for v in pel { out.extend_from_slice(&v.to_le_bytes()); }
    let k = if f.k.is_finite() { f.k.clamp(0.5, 1.5) } else { 1.0 };
    let kq = (((k - 0.5) * 65535.0).round() as u32).min(65535) as u16;
    out.extend_from_slice(&kq.to_le_bytes());
    let k = 0.5 + kq as f32 / 65535.0;

    let mut w = BitW::new(std::mem::take(out));
    // Rotations, closed loop: rec[i] is what the decoder will have.
    let mut rec = [Bone::default(); NB];
    rec[0].p = pel;
    rec[0].q = put_quat(&mut w, f.bones[0].q, ROOT_QBITS);
    // Translation overrides (bone index, quantised offset); at most NB - 1, on the stack.
    let mut ovr = [(0usize, [0f32; 3]); NB];
    let mut n_ovr = 0usize;
    for i in 1..NB {
        let par = rec[PARENT[i]];
        let lq = qmul(qconj(par.q), qnorm(f.bones[i].q));
        let lq = put_quat(&mut w, lq, QBITS);
        rec[i].q = qnorm(qmul(par.q, lq));
        let need = qrot(qconj(par.q), sub(f.bones[i].p, par.p));
        let reft = scale(REF_T[i], k);
        let t = if dist(need, reft) > OVERRIDE_UU {
            let qv = [qi(need[0], OVR_STEP, 16), qi(need[1], OVR_STEP, 16), qi(need[2], OVR_STEP, 16)];
            let tq = [qv[0] as f32 * OVR_STEP, qv[1] as f32 * OVR_STEP, qv[2] as f32 * OVR_STEP];
            ovr[n_ovr] = (i, tq);
            n_ovr += 1;
            tq
        } else { reft };
        rec[i].p = add(par.p, qrot(par.q, t));
    }
    for i in 0..NB {
        if !HAS_BODY[i] { continue; }
        put_vel_enc(&mut w, f.bones[i].v, VLIN_MAX);
        put_vel_enc(&mut w, f.bones[i].w, VANG_MAX);
    }
    let mut mask = 0u32;
    for (i, _) in &ovr[..n_ovr] { mask |= 1 << (i - 1); }
    w.put(mask, (NB - 1) as u32);
    for (_, t) in &ovr[..n_ovr] { for c in t { w.put_i((c / OVR_STEP).round() as i32, 16); } }
    for wp in f.weapons.iter().take(2) {
        w.put(wp.hands as u32 & 3, 2);
        w.put(wp.id as u32, 8);
        put_v3(&mut w, sub(wp.p, pel), WPOS_STEP, 16);
        put_quat(&mut w, wp.q, QBITS);
        put_vel_enc(&mut w, wp.v, VLIN_MAX);
        put_vel_enc(&mut w, wp.w, VANG_MAX);
        put_v3(&mut w, wp.base, BLADE_STEP, 16);
        put_v3(&mut w, wp.tip, BLADE_STEP, 16);
    }
    if let Some(c) = &f.control {
        w.put(c.flags, 32);
        w.put(c.grip_r as u32 & 15, 4);
        w.put(c.grip_l as u32 & 15, 4);
        for (i, (_, lo, hi)) in CONTROL_SCALARS.iter().enumerate() {
            let x = c.scalars[i];
            let x = if x.is_finite() { ((x - lo) / (hi - lo)).clamp(0.0, 1.0) } else { 0.0 };
            w.put((x * 255.0).round() as u32, 8);
        }
        let l = (c.aim[0] * c.aim[0] + c.aim[1] * c.aim[1] + c.aim[2] * c.aim[2]).sqrt();
        let a = if l > 1e-6 && l.is_finite() { scale(c.aim, 1.0 / l) } else { [0.0; 3] };
        for v in a { w.put_i((v * 127.0).round() as i32, 8); }
        w.put_i(qi(c.ctrl_pitch, 360.0 / 65536.0, 16), 16);
        w.put_i(qi(((c.ctrl_yaw + 180.0).rem_euclid(360.0)) - 180.0, 360.0 / 65536.0, 16), 16);
        for i in 0..4 {
            let world = c.ik_world[i];
            w.put(world as u32, 1);
            let v = if world { sub(c.ik[i], pel) } else { c.ik[i] };
            put_v3(&mut w, v, IK_STEP, 12);
        }
    }
    *out = w.finish();
    if step > 0 { out.push(step); }
}

/// Decode a v2 frame (None if malformed or not v2). Positions/rotations are
/// world space, rebuilt by forward kinematics.
pub fn decode(buf: &[u8]) -> Option<Full> {
    if !is_v2(buf) || buf.len() < 4 + 4 + 1 + 1 + 12 + 2 { return None; }
    let ms = u32::from_le_bytes([buf[4], buf[5], buf[6], buf[7]]) as f64;
    let ts = ms + buf[8] as f64 / 256.0;
    let flags = buf[9];
    let rf = |o: usize| f32::from_le_bytes([buf[o], buf[o + 1], buf[o + 2], buf[o + 3]]);
    let pel = [rf(10), rf(14), rf(18)];
    if !pel.iter().all(|v| v.is_finite()) { return None; }
    let k = 0.5 + u16::from_le_bytes([buf[22], buf[23]]) as f32 / 65535.0;
    let mut r = BitR::new(&buf[24..]);
    let mut f = Full { ts, k, ..Default::default() };
    f.bones[0].p = pel;
    f.bones[0].q = get_quat(&mut r, ROOT_QBITS)?;
    let mut lq = [[0f32; 4]; NB];
    for i in 1..NB { lq[i] = get_quat(&mut r, QBITS)?; }
    for i in 0..NB {
        if !HAS_BODY[i] { continue; }
        f.bones[i].v = get_vel(&mut r, VLIN_MAX)?;
        f.bones[i].w = get_vel(&mut r, VANG_MAX)?;
    }
    let mask = r.get((NB - 1) as u32)?;
    let mut t = [[0f32; 3]; NB];
    for i in 1..NB { t[i] = scale(REF_T[i], k); }
    for i in 1..NB {
        if mask & (1 << (i - 1)) != 0 { t[i] = get_v3(&mut r, OVR_STEP, 16)?; }
    }
    f.overrides = mask;
    for i in 1..NB {
        let par = f.bones[PARENT[i]];
        f.bones[i].q = qnorm(qmul(par.q, lq[i]));
        f.bones[i].p = add(par.p, qrot(par.q, t[i]));
    }
    let nw = ((flags >> 1) & 3) as usize;
    for _ in 0..nw.min(2) {
        let hands = r.get(2)? as u8;
        let id = r.get(8)? as u8;
        let p = add(pel, get_v3(&mut r, WPOS_STEP, 16)?);
        let q = get_quat(&mut r, QBITS)?;
        let v = get_vel(&mut r, VLIN_MAX)?;
        let wv = get_vel(&mut r, VANG_MAX)?;
        let base = get_v3(&mut r, BLADE_STEP, 16)?;
        let tip = get_v3(&mut r, BLADE_STEP, 16)?;
        f.weapons.push(Weapon { hands, id, p, q, v, w: wv, base, tip });
    }
    if flags & 1 != 0 {
        let mut c = Control { flags: r.get(32)?, grip_r: r.get(4)? as u8, grip_l: r.get(4)? as u8, ..Default::default() };
        for (i, (_, lo, hi)) in CONTROL_SCALARS.iter().enumerate() {
            c.scalars[i] = lo + (hi - lo) * r.get(8)? as f32 / 255.0;
        }
        for i in 0..3 { c.aim[i] = r.get_i(8)? as f32 / 127.0; }
        c.ctrl_pitch = r.get_i(16)? as f32 * 360.0 / 65536.0;
        c.ctrl_yaw = r.get_i(16)? as f32 * 360.0 / 65536.0;
        for i in 0..4 {
            let world = r.get(1)? != 0;
            let v = get_v3(&mut r, IK_STEP, 12)?;
            c.ik_world[i] = world;
            c.ik[i] = if world { add(pel, v) } else { v };
        }
        f.control = Some(c);
    }
    if flags & 8 != 0 { f.step = *buf.last()? as f32; }
    Some(f)
}

/// Blade base/tip world positions of a decoded weapon.
pub fn blade_world(w: &Weapon) -> (V3, V3) {
    (add(w.p, qrot(w.q, w.base)), add(w.p, qrot(w.q, w.tip)))
}

// ---- game sample -> Full ----------------------------------------------------------

/// A game-side sample as a [`Full`] (the input of the ONE encode, `crate::sample`): `b` =
/// NB x 13 floats (p, q, v, w per bone, world space), `ws` = up to 2 weapons as 21 numbers
/// (hands, id, p q v w, blade base xyz, tip xyz; world space; base / tip are moved into
/// weapon space here), `ctl` = the control layer with raw `ik` values (`ik_world` is
/// derived here). `None` when a bone value is not finite.
pub fn from_parts(tick: u32, ts: f64, dt: f64, b: &[f32], ws: &[[f32; 21]], ctl: Option<Control>) -> Option<(u32, Full)> {
    if b.len() != NB * 13 || !b.iter().all(|x| x.is_finite()) || !ts.is_finite() { return None; }
    let mut f = Full { ts, ..Default::default() };
    f.step = Some(dt).filter(|x| x.is_finite() && *x > 0.0 && *x <= 250.0).unwrap_or(0.0) as f32;
    for i in 0..NB {
        let o = i * 13;
        f.bones[i] = Bone {
            p: [b[o], b[o + 1], b[o + 2]], q: qnorm([b[o + 3], b[o + 4], b[o + 5], b[o + 6]]),
            v: [b[o + 7], b[o + 8], b[o + 9]], w: [b[o + 10], b[o + 11], b[o + 12]],
        };
    }
    // Skeleton scale from the limb offsets (robust to one dislocated joint:
    // median of the per-bone ratios).
    // (On the stack: this runs on the game thread for every sample.)
    let mut rbuf = [0f32; NB];
    let mut nr = 0usize;
    for i in 1..NB {
        let par = f.bones[PARENT[i]];
        let lr = (REF_T[i][0].powi(2) + REF_T[i][1].powi(2) + REF_T[i][2].powi(2)).sqrt();
        if lr < 5.0 { continue; }
        let r = dist(f.bones[i].p, par.p) / lr;
        if r.is_finite() && r > 0.5 && r < 1.5 { rbuf[nr] = r; nr += 1; }
    }
    let ratios = &mut rbuf[..nr];
    ratios.sort_unstable_by(|a, b| a.partial_cmp(b).unwrap());
    f.k = if nr == 0 { 1.0 } else { ratios[nr / 2] };
    for a in ws.iter().take(2) {
        if !a.iter().all(|x| x.is_finite()) { continue; }
        let p = [a[2], a[3], a[4]];
        let q = qnorm([a[5], a[6], a[7], a[8]]);
        let inv = qconj(q);
        f.weapons.push(Weapon {
            hands: a[0] as u8, id: a[1].clamp(0.0, 255.0) as u8, p, q,
            v: [a[9], a[10], a[11]], w: [a[12], a[13], a[14]],
            base: qrot(inv, sub([a[15], a[16], a[17]], p)),
            tip: qrot(inv, sub([a[18], a[19], a[20]], p)),
        });
    }
    if let Some(mut ctl) = ctl {
        // (A frame without IK values keeps every ik_world false, as before.)
        let has_ik = ctl.ik.iter().any(|v| v.iter().any(|x| *x != 0.0));
        for i in 0..4 {
            if !has_ik { break; }
            // A world point lies near the body; anything else is a
            // component/local value and is sent raw.
            ctl.ik_world[i] = dist(ctl.ik[i], f.bones[0].p) < 1000.0;
        }
        f.control = Some(ctl);
    }
    Some((tick, f))
}

// ---- lag-compensation extras (server) -------------------------------------------

/// What the server's lag compensation can take from a v2 frame beyond the
/// v1 hero bones: the held blade (grip, tip, tip velocity) and per-segment
/// body capsules (a, b, radius) covering all 22 bodies.
#[derive(Debug, Clone, PartialEq)]
pub struct LagExtras {
    pub ts: u32,
    pub blade: Option<(V3, V3, V3)>,
    pub caps: Vec<(V3, V3, f32)>,
}

/// Body capsules (bone a, bone b, radius uu) between v2 bones.
const CAPS: [(usize, usize, f32); 16] = [
    (0, 2, 18.0), (2, 4, 18.0), (4, 5, 18.0), (5, 7, 8.0),
    (9, 10, 8.0), (10, 11, 9.0), (11, 12, 7.0),
    (13, 14, 8.0), (14, 15, 9.0), (15, 16, 7.0),
    (17, 18, 9.0), (18, 19, 7.0),
    (20, 21, 9.0), (21, 22, 7.0),
    (0, 17, 14.0), (0, 20, 14.0),
];

pub fn lagcomp_extras(buf: &[u8]) -> Option<LagExtras> {
    if !is_v2(buf) { return None; }
    let f = decode(buf)?;
    Some(extras_of(&f))
}

pub fn extras_of(f: &Full) -> LagExtras {
    let b = &f.bones;
    let mut caps: Vec<(V3, V3, f32)> = CAPS.iter().map(|&(a, c, r)| (b[a].p, b[c].p, r)).collect();
    // Head: neck_02 -> head and on to the crown.
    let up = sub(b[8].p, b[7].p);
    caps.push((b[8].p, add(b[8].p, scale(up, 1.6)), 12.0));
    // Hands: from the wrist along the forearm direction.
    for (lo, h) in [(11usize, 12usize), (15, 16)] {
        let d = sub(b[h].p, b[lo].p);
        let l = (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt().max(1e-3);
        caps.push((b[h].p, add(b[h].p, scale(d, 10.0 / l)), 5.0));
    }
    // Feet: foot bone to the ball (SK_Body_Man ball offset in the foot frame).
    for (ft, ball) in [(19usize, [1.69f32, 14.92, -7.49]), (22, [-1.69, 14.92, -7.49])] {
        caps.push((b[ft].p, add(b[ft].p, qrot(b[ft].q, scale(ball, f.k))), 5.0));
    }
    // Held blade: prefer the right hand / two-handed weapon.
    let w = f.weapons.iter().find(|w| w.hands & 1 != 0).or(f.weapons.first());
    let blade = w.map(|w| {
        let (base, tip) = blade_world(w);
        let r = sub(tip, w.p);
        let om = [w.w[0].to_radians(), w.w[1].to_radians(), w.w[2].to_radians()];
        let cross = [om[1] * r[2] - om[2] * r[1], om[2] * r[0] - om[0] * r[2], om[0] * r[1] - om[1] * r[0]];
        (base, tip, add(w.v, cross))
    });
    LagExtras { ts: f.ts.floor() as u32, blade, caps }
}

/// The v1 view of a v2 frame (16 hero bones world + weapon slot 16), so
/// lag compensation and older readers keep working.
pub fn to_v1(f: &Full) -> super::PoseFrame {
    let mut bones = Vec::with_capacity(17);
    for (h, &i) in HERO_TO_V2.iter().enumerate() {
        bones.push((h as u8, f.bones[i].p, f.bones[i].q));
    }
    if let Some(w) = f.weapons.first() { bones.push((super::WEAPON as u8, w.p, w.q)); }
    super::PoseFrame { ts: f.ts.floor() as u32, bones }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    fn axis_q(ax: V3, deg: f32) -> Quat {
        let l = (ax[0] * ax[0] + ax[1] * ax[1] + ax[2] * ax[2]).sqrt();
        let h = deg.to_radians() / 2.0;
        [ax[0] / l * h.sin(), ax[1] / l * h.sin(), ax[2] / l * h.sin(), h.cos()]
    }

    /// A plausible full state built by FK from random local rotations.
    pub(crate) fn sample_state(seed: u32, k: f32) -> Full {
        let mut s = seed.wrapping_mul(2654435761).wrapping_add(12345);
        let mut rnd = || { s ^= s << 13; s ^= s >> 17; s ^= s << 5; (s as f32 / u32::MAX as f32) * 2.0 - 1.0 };
        let mut f = Full { ts: 123456.789, k, ..Default::default() };
        f.bones[0].p = [-434.6, 990.0, 85.0];
        f.bones[0].q = axis_q([rnd(), rnd(), 1.0], 170.0 * rnd());
        for i in 1..NB {
            let par = f.bones[PARENT[i]];
            let lq = axis_q([rnd(), rnd(), rnd() + 0.01], 90.0 * rnd());
            f.bones[i].q = qnorm(qmul(par.q, lq));
            f.bones[i].p = add(par.p, qrot(par.q, scale(REF_T[i], k)));
        }
        for i in 0..NB {
            f.bones[i].v = [1500.0 * rnd(), 800.0 * rnd(), 300.0 * rnd()];
            f.bones[i].w = [900.0 * rnd(), 2000.0 * rnd(), 40.0 * rnd()];
            if !HAS_BODY[i] { f.bones[i].v = [0.0; 3]; f.bones[i].w = [0.0; 3]; }
        }
        f.weapons.push(Weapon {
            hands: 1, id: 7, p: add(f.bones[16].p, [3.0, -2.0, 1.0]), q: axis_q([0.3, 1.0, 0.2], 77.0),
            v: [2500.0, -900.0, 100.0], w: [1800.0, 0.0, -600.0], base: [0.0, 0.0, 18.0], tip: [0.0, 0.0, 105.5],
        });
        f
    }

    #[test]
    fn roundtrip_is_exact_within_spec() {
        for seed in 0..50u32 {
            let k = 0.97 + 0.0007 * seed as f32;
            let f = sample_state(seed, k);
            let buf = encode(&f);
            let d = decode(&buf).expect("decode");
            assert!((d.ts - f.ts).abs() < 0.005, "ts {} {}", d.ts, f.ts);
            for i in 0..NB {
                let e = dist(d.bones[i].p, f.bones[i].p);
                let a = qangle_deg(d.bones[i].q, f.bones[i].q);
                assert!(e < 0.3, "seed {} bone {} pos err {}", seed, BONES[i], e);
                assert!(a < 0.1, "seed {} bone {} rot err {}", seed, BONES[i], a);
                for ax in 0..3 {
                    let tv = f.bones[i].v[ax];
                    assert!((d.bones[i].v[ax] - tv).abs() <= 0.03 * tv.abs() + 2.0, "lin vel {} vs {}", d.bones[i].v[ax], tv);
                    let tw = f.bones[i].w[ax];
                    assert!((d.bones[i].w[ax] - tw).abs() <= 0.03 * tw.abs() + 1.0, "ang vel {} vs {}", d.bones[i].w[ax], tw);
                }
            }
            let (wd, ws) = (&d.weapons[0], &f.weapons[0]);
            let ((b0, t0), (b1, t1)) = (blade_world(wd), blade_world(ws));
            assert!(dist(t0, t1) < 0.3, "tip err {}", dist(t0, t1));
            assert!(dist(b0, b1) < 0.3);
            assert_eq!((wd.hands, wd.id), (1, 7));
            assert_eq!(d.overrides, 0, "pure FK state needs no overrides");
        }
    }

    #[test]
    fn dislocated_joint_is_sent_exactly() {
        let mut f = sample_state(3, 1.0);
        // Pull the left hand 6 uu out of its socket (and everything is still FK above it).
        f.bones[12].p = add(f.bones[12].p, [6.0, -2.0, 1.0]);
        let d = decode(&encode(&f)).unwrap();
        assert!(d.overrides & (1 << 11) != 0);
        assert!(dist(d.bones[12].p, f.bones[12].p) < 0.05);
        assert!(dist(d.bones[11].p, f.bones[11].p) < 0.3);
    }

    #[test]
    fn size_is_within_budget() {
        let mut f = sample_state(1, 1.0);
        let no_ctl = encode(&f).len();
        f.control = Some(Control { flags: 0xA5A5_0001, grip_r: 3, grip_l: 1, aim: [0.0, 1.0, 0.0],
            ik: [[10.0, 20.0, 30.0]; 4], ..Default::default() });
        let with_ctl = encode(&f).len();
        println!("v2 frame: {} B without control, {} B with control", no_ctl, with_ctl);
        assert!(no_ctl <= 315, "{} B", no_ctl);
        assert!(with_ctl <= 360, "{} B", with_ctl);
    }

    #[test]
    fn control_roundtrips() {
        let mut f = sample_state(2, 1.0);
        let mut s = [0f32; 16];
        for (i, (_, lo, hi)) in CONTROL_SCALARS.iter().enumerate() { s[i] = lo + (hi - lo) * (i as f32 / 16.0); }
        let pel = f.bones[0].p;
        f.control = Some(Control { flags: 0x8000_0401, grip_r: 9, grip_l: 2, scalars: s, aim: [0.6, 0.8, 0.0],
            ctrl_pitch: -12.5, ctrl_yaw: 171.0,
            ik: [add(pel, [40.0, 10.0, 30.0]), add(pel, [-40.0, 10.0, 30.0]), [1.0, 2.0, 3.0], [-5.0, 0.0, 9.5]],
            ik_world: [true, true, false, false] });
        let d = decode(&encode(&f)).unwrap().control.unwrap();
        let c = f.control.unwrap();
        assert_eq!((d.flags, d.grip_r, d.grip_l), (c.flags, c.grip_r, c.grip_l));
        for i in 0..16 { let (_, lo, hi) = CONTROL_SCALARS[i]; assert!((d.scalars[i] - c.scalars[i]).abs() <= (hi - lo) / 255.0); }
        assert!((d.ctrl_yaw - 171.0).abs() < 0.01 && (d.ctrl_pitch + 12.5).abs() < 0.01);
        for i in 0..4 { assert!(dist(d.ik[i], c.ik[i]) < 0.5, "ik {}", i); }
    }

    #[test]
    fn malformed_is_rejected_and_v1_is_not_v2() {
        let f = sample_state(4, 1.0);
        let buf = encode(&f);
        for n in [0usize, 3, 10, 24, 40, buf.len() / 2] { assert!(decode(&buf[..n]).is_none(), "truncated at {}", n); }
        let v1 = super::super::encode(1234, &[(0, [0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 1.0])]);
        assert!(!is_v2(&v1));
        assert!(decode(&v1).is_none());
        let mut bad = buf.clone();
        bad[10..14].copy_from_slice(&f32::NAN.to_le_bytes());
        assert!(decode(&bad).is_none());
    }

    #[test]
    fn from_parts_moves_blade_into_weapon_space() {
        let f = sample_state(5, 0.9966);
        let mut b = Vec::new();
        for x in &f.bones { b.extend([x.p[0], x.p[1], x.p[2], x.q[0], x.q[1], x.q[2], x.q[3], x.v[0], x.v[1], x.v[2], x.w[0], x.w[1], x.w[2]]); }
        let w = &f.weapons[0];
        let (bw, tw) = blade_world(w);
        let wv = [1.0, 7.0, w.p[0], w.p[1], w.p[2], w.q[0], w.q[1], w.q[2], w.q[3], w.v[0], w.v[1], w.v[2], w.w[0], w.w[1], w.w[2], bw[0], bw[1], bw[2], tw[0], tw[1], tw[2]];
        let ctl = Control { flags: 5, grip_r: 2, aim: [1.0, 0.0, 0.0], ctrl_yaw: 90.0, ..Default::default() };
        let (tick, p) = from_parts(9, 5000.25, 0.0, &b, &[wv], Some(ctl)).expect("parts");
        assert_eq!(tick, 9);
        assert!((p.ts - 5000.25).abs() < 1e-6);
        assert!((p.k - 0.9966).abs() < 0.001, "k {}", p.k);
        assert!(dist(p.weapons[0].tip, w.tip) < 0.01);
        assert_eq!(p.control.as_ref().unwrap().flags, 5);
        let d = decode(&encode(&p)).unwrap();
        assert!(dist(d.bones[16].p, f.bones[16].p) < 0.3);
        assert!(from_parts(1, 2.0, 0.0, &[1.0, 2.0], &[], None).is_none());
    }

    #[test]
    fn v1_view_maps_hero_bones() {
        let f = sample_state(6, 1.0);
        let v1 = to_v1(&f);
        assert_eq!(v1.bones.len(), 17);
        assert_eq!(v1.bones[9].1, f.bones[16].p); // Hand_R
        assert_eq!(v1.bones[16].0 as usize, super::super::WEAPON);
    }
}
