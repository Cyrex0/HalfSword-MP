//! Procedural fighters. A fighter is a pure function of TRUE time (ms):
//! root path, facing, posture (standing / falling) and one right-arm action
//! at a time (guard, swing, thrust, feint, parry). The 16 hero bones follow
//! posecodec::HERO_BONES; the held weapon is a blade segment (grip → base →
//! tip) per weapon class; the TRUE collision bodies are the server capsule
//! table with per-fighter radius variation plus the parts the bone chain
//! misses (skull above the head bone, fingers past the hand bone, toes), so
//! honest contacts land where the real physics asset would put them, not
//! where the server's model assumes.

use super::*;
use crate::posecodec::*;
use crate::validate::damage::WeaponClass;

/// Shoulder → hand at full extension (stock Willie ≈ 30 + 28 uu).
pub const ARM: f32 = 58.0;
/// Pelvis height standing.
pub const PELVIS_Z: f32 = 95.0;
/// Blade half-thickness (collision).
pub const BLADE_R: f32 = 2.5;

/// Lower-case bone names HSMPCombat sends (`Get Damage` bone param).
pub const BONE_NAMES: [&str; 16] = [
    "pelvis", "spine_02", "spine_04", "head", "upperarm_l", "lowerarm_l", "hand_l",
    "upperarm_r", "lowerarm_r", "hand_r", "thigh_l", "calf_l", "foot_l", "thigh_r", "calf_r", "foot_r",
];

/// Server capsule table (lagcomp SEGMENTS), with the name of the bone a
/// contact on that segment reports.
pub const SEGS: [(usize, usize, f32, &str); 15] = [
    (PELVIS, SPINE_02, 18.0, "pelvis"), (SPINE_02, SPINE_04, 18.0, "spine_03"), (SPINE_04, HEAD, 12.0, "head"),
    (SPINE_04, UPPERARM_L, 9.0, "clavicle_l"), (UPPERARM_L, LOWERARM_L, 9.0, "upperarm_l"), (LOWERARM_L, HAND_L, 7.0, "lowerarm_l"),
    (SPINE_04, UPPERARM_R, 9.0, "clavicle_r"), (UPPERARM_R, LOWERARM_R, 9.0, "upperarm_r"), (LOWERARM_R, HAND_R, 7.0, "lowerarm_r"),
    (PELVIS, THIGH_L, 14.0, "thigh_l"), (THIGH_L, CALF_L, 9.0, "thigh_l"), (CALF_L, FOOT_L, 7.0, "calf_l"),
    (PELVIS, THIGH_R, 14.0, "thigh_r"), (THIGH_R, CALF_R, 9.0, "thigh_r"), (CALF_R, FOOT_R, 7.0, "calf_r"),
];
/// Extension of the true body past the segment's distal bone (uu): skull
/// above the head bone, fingers past the hand bone, toes past the foot.
fn seg_ext(i: usize) -> f32 {
    match i { 2 => 12.0, 5 | 8 => 9.0, 11 | 14 => 10.0, _ => 0.0 }
}

#[derive(Clone, Copy, Debug)]
pub struct Weapon {
    pub id: &'static str,
    pub class: WeaponClass,
    /// hand → blade base (grip / haft below the striking part), uu
    pub grip: f32,
    /// blade base → tip, uu
    pub blade: f32,
    pub two_hand: bool,
}

pub const WEAPONS: &[Weapon] = &[
    Weapon { id: "w_arming1", class: WeaponClass::Sword, grip: 10.0, blade: 78.0, two_hand: false },
    Weapon { id: "w_longsword1", class: WeaponClass::Sword, grip: 14.0, blade: 98.0, two_hand: true },
    Weapon { id: "w_messer_a", class: WeaponClass::Sword, grip: 10.0, blade: 70.0, two_hand: false },
    Weapon { id: "w_rondel", class: WeaponClass::Dagger, grip: 6.0, blade: 30.0, two_hand: false },
    // Hafted weapons: the streamed "blade" (POSE codec v2) is the striking
    // segment from just above the top hand to the far end of the head.
    Weapon { id: "w_waraxe3", class: WeaponClass::Axe, grip: 8.0, blade: 80.0, two_hand: true },
    Weapon { id: "w_hammer_ll", class: WeaponClass::Blunt, grip: 8.0, blade: 82.0, two_hand: true },
    Weapon { id: "w_poleaxe_m", class: WeaponClass::Polearm, grip: 10.0, blade: 175.0, two_hand: true },
];

#[derive(Clone, Copy, Debug)]
pub struct Blade {
    pub base: V3,
    pub tip: V3,
}

#[derive(Clone, Copy, Debug)]
pub struct Pose {
    pub bones: [V3; 16],
    pub blade: Blade,
    /// Weapon actor origin (the v1 stream's weapon pseudo-bone): the grip.
    pub grip: V3,
}

#[derive(Clone, Copy, Debug)]
pub struct Capsule {
    pub a: V3,
    pub b: V3,
    pub r: f32,
    pub seg: usize,
}

#[derive(Clone, Debug)]
pub enum Act {
    /// d(θ) = cos θ·u + sin θ·v, θ = theta_end·smoothstep((t − t0)/dur)
    Swing { t0: f64, dur: f64, u: V3, v: V3, theta_end: f32 },
    /// blade along d; hand from S + ext0·d to S + ext1·d (eased)
    Thrust { t0: f64, dur: f64, d: V3, ext0: f32, ext1: f32 },
    /// a swing that turns back at theta_max (sin profile): no contact
    Feint { t0: f64, dur: f64, u: V3, v: V3, theta_max: f32 },
    /// hold the blade through world point p
    Parry { t0: f64, t1: f64, p: V3 },
}

impl Act {
    fn start(&self) -> f64 {
        match *self {
            Act::Swing { t0, .. } | Act::Thrust { t0, .. } | Act::Feint { t0, .. } | Act::Parry { t0, .. } => t0,
        }
    }
    fn end(&self) -> f64 {
        match *self {
            Act::Swing { t0, dur, .. } | Act::Thrust { t0, dur, .. } | Act::Feint { t0, dur, .. } => t0 + dur,
            Act::Parry { t1, .. } => t1,
        }
    }
}

#[derive(Clone, Debug)]
pub struct Fall {
    pub t0: f64,
    /// time the fighter is back up
    pub t1: f64,
}

#[derive(Clone)]
pub struct Fighter {
    pub weapon: Weapon,
    pub armour: Vec<&'static str>,
    /// (t, x, y) root waypoints, linear in between
    pub path: Vec<(f64, f32, f32)>,
    /// (t, yaw rad) facing samples, linear (short way) in between
    pub face: Vec<(f64, f32)>,
    pub acts: Vec<Act>,
    pub falls: Vec<Fall>,
    /// True-body radius factor per segment (physics asset vs server table).
    pub rk: [f32; 15],
}

const WIND_MS: f64 = 90.0;
const RECOVER_MS: f64 = 180.0;

fn smooth(x: f64) -> f64 {
    let x = x.clamp(0.0, 1.0);
    x * x * (3.0 - 2.0 * x)
}

fn lerp_path(p: &[(f64, f32, f32)], t: f64) -> (f32, f32) {
    if p.is_empty() { return (0.0, 0.0); }
    if t <= p[0].0 { return (p[0].1, p[0].2); }
    for w in p.windows(2) {
        if t <= w[1].0 {
            let f = ((t - w[0].0) / (w[1].0 - w[0].0).max(1e-6)) as f32;
            return (w[0].1 + (w[1].1 - w[0].1) * f, w[0].2 + (w[1].2 - w[0].2) * f);
        }
    }
    let l = p[p.len() - 1];
    (l.1, l.2)
}

fn lerp_yaw(p: &[(f64, f32)], t: f64) -> f32 {
    if p.is_empty() { return 0.0; }
    if t <= p[0].0 { return p[0].1; }
    for w in p.windows(2) {
        if t <= w[1].0 {
            let f = ((t - w[0].0) / (w[1].0 - w[0].0).max(1e-6)) as f32;
            let mut d = w[1].1 - w[0].1;
            while d > std::f32::consts::PI { d -= 2.0 * std::f32::consts::PI; }
            while d < -std::f32::consts::PI { d += 2.0 * std::f32::consts::PI; }
            return w[0].1 + d * f;
        }
    }
    p[p.len() - 1].1
}

impl Fighter {
    pub fn root_xy(&self, t: f64) -> (f32, f32) { lerp_path(&self.path, t) }
    pub fn yaw(&self, t: f64) -> f32 { lerp_yaw(&self.face, t) }

    /// Fall progress 0 (standing) .. 1 (on the ground).
    fn fallen(&self, t: f64) -> f32 {
        for f in &self.falls {
            if t >= f.t0 && t <= f.t1 {
                let down = smooth((t - f.t0) / 450.0);
                let up = smooth((t - (f.t1 - 700.0)) / 700.0);
                return (down * (1.0 - up)) as f32;
            }
        }
        0.0
    }

    /// Frame (pelvis, forward, left, up) of the torso at t.
    pub fn frame(&self, t: f64) -> (V3, V3, V3, V3) {
        let (x, y) = self.root_xy(t);
        let yaw = self.yaw(t);
        let f0 = [yaw.cos(), yaw.sin(), 0.0];
        let l = [-yaw.sin(), yaw.cos(), 0.0];
        let a = self.fallen(t) * 1.35; // falls backwards to ~77°
        let up = [0.0, 0.0, 1.0];
        let u = norm(sub(mul(up, a.cos()), mul(f0, a.sin())));
        let f = norm(add(mul(up, a.sin()), mul(f0, a.cos())));
        let pz = PELVIS_Z - 65.0 * self.fallen(t);
        ([x, y, pz], f, l, u)
    }

    pub fn shoulder_r(&self, t: f64) -> V3 {
        let (p, _f, l, u) = self.frame(t);
        add(p, add(mul(u, 46.0), mul(l, -18.0)))
    }

    fn guard_dir(&self, t: f64) -> (V3, V3) {
        let (_p, f, l, u) = self.frame(t);
        // Blade raised on the right side (vom Tag), slightly forward.
        let d = norm(add(add(mul(f, 0.30), mul(u, 0.93)), mul(l, -0.20)));
        // Hand by the right shoulder, a little in front.
        let arm = norm(add(add(mul(f, 0.75), mul(u, -0.45)), mul(l, -0.35)));
        (d, mul(arm, 34.0))
    }

    /// Right hand position and blade direction at t.
    fn arm(&self, t: f64) -> (V3, V3) {
        let s = self.shoulder_r(t);
        let (gd, ga) = self.guard_dir(t);
        let guard = (add(s, ga), gd);
        // The action whose window (with wind-up / recovery) contains t.
        let act = self.acts.iter().rev().find(|a| t >= a.start() - WIND_MS && t <= a.end() + RECOVER_MS);
        let Some(act) = act else { return guard };
        let (hand, d) = match *act {
            Act::Swing { t0, dur, u, v, theta_end } => {
                let th = theta_end * smooth((t - t0) / dur) as f32;
                let d = norm(add(mul(u, th.cos()), mul(v, th.sin())));
                (add(s, mul(d, ARM)), d)
            }
            Act::Feint { t0, dur, u, v, theta_max } => {
                let x = ((t - t0) / dur).clamp(0.0, 1.0);
                let th = theta_max * (std::f64::consts::PI * x).sin() as f32;
                let d = norm(add(mul(u, th.cos()), mul(v, th.sin())));
                (add(s, mul(d, ARM)), d)
            }
            Act::Thrust { t0, dur, d, ext0, ext1 } => {
                let e = ext0 + (ext1 - ext0) * smooth((t - t0) / dur) as f32;
                (add(s, mul(d, e)), d)
            }
            Act::Parry { p, .. } => {
                let w = &self.weapon;
                let to = sub(p, s);
                let d = norm(to);
                let reach = (len(to) - w.grip - 0.5 * w.blade).clamp(18.0, ARM);
                (add(s, mul(d, reach)), d)
            }
        };
        // Blend from guard during the wind-up and back during the recovery.
        let k = if t < act.start() {
            smooth((t - (act.start() - WIND_MS)) / WIND_MS)
        } else if t > act.end() {
            1.0 - smooth((t - act.end()) / RECOVER_MS)
        } else {
            1.0
        } as f32;
        if k >= 0.999 { return (hand, d); }
        (lerp3(guard.0, hand, k), norm(lerp3(guard.1, d, k)))
    }

    pub fn pose(&self, t: f64) -> Pose {
        let (p, f, l, u) = self.frame(t);
        let o = |x: f32, y: f32, z: f32| add(p, add(add(mul(f, x), mul(l, y)), mul(u, z)));
        let sh_l = o(0.0, 18.0, 46.0);
        let sh_r = o(0.0, -18.0, 46.0);
        let (hand_r, d) = self.arm(t);
        let w = &self.weapon;
        let spread = if w.class == WeaponClass::Polearm { 40.0 } else { 14.0 };
        let hand_l = if w.two_hand { sub(hand_r, mul(d, spread)) } else { o(26.0, 22.0, 30.0) };
        let elbow = |sh: V3, h: V3| sub(lerp3(sh, h, 0.5), mul(u, 7.0));
        let mut b = [[0.0f32; 3]; 16];
        b[PELVIS] = p;
        b[SPINE_02] = o(0.0, 0.0, 25.0);
        b[SPINE_04] = o(0.0, 0.0, 50.0);
        b[HEAD] = o(0.0, 0.0, 70.0);
        b[UPPERARM_L] = sh_l;
        b[LOWERARM_L] = elbow(sh_l, hand_l);
        b[HAND_L] = hand_l;
        b[UPPERARM_R] = sh_r;
        b[LOWERARM_R] = elbow(sh_r, hand_r);
        b[HAND_R] = hand_r;
        b[THIGH_L] = o(0.0, 10.0, -5.0);
        b[CALF_L] = o(4.0, 11.0, -48.0);
        b[FOOT_L] = o(0.0, 11.0, -88.0);
        b[THIGH_R] = o(0.0, -10.0, -5.0);
        b[CALF_R] = o(4.0, -11.0, -48.0);
        b[FOOT_R] = o(0.0, -11.0, -88.0);
        let base = add(hand_r, mul(d, w.grip));
        let tip = add(base, mul(d, w.blade));
        Pose { bones: b, blade: Blade { base, tip }, grip: hand_r }
    }

    /// The TRUE collision bodies at pose `p` (what the physics asset touches).
    pub fn true_caps(&self, p: &Pose) -> Vec<Capsule> {
        SEGS.iter().enumerate().map(|(i, &(a, b, r, _))| {
            let (pa, pb) = (p.bones[a], p.bones[b]);
            let ext = seg_ext(i);
            let pb = if ext > 0.0 { add(pb, mul(norm(sub(pb, pa)), ext)) } else { pb };
            Capsule { a: pa, b: pb, r: r * self.rk[i], seg: i }
        }).collect()
    }

    /// Kit for `validate::damage::set_kit`.
    pub fn kit(&self) -> crate::loadout::KitSel {
        crate::loadout::KitSel::new("custom", self.weapon.id, "", &self.armour, [0; 4])
    }
}
