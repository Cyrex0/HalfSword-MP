//! Fight generator: 2–8 fighters in pairs (plus flankers for odd counts),
//! each pair running back-to-back ENGAGEMENTS of a random kind. Strikes aim
//! at the target's TRUE position at the planned contact time; whether they
//! connect on each screen is decided later by the world, from what each
//! client actually displays.

use super::body::*;
use super::*;
use crate::validate::damage::WeaponClass;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Kind {
    /// Attack; the defender stands or steps back during it.
    Clean,
    /// The defender parries on its own screen (blade through the attacker's
    /// displayed blade path).
    Parry,
    /// The defender's parry misses by 45–80 uu.
    MissParry,
    /// Feint (turns back), then a real attack.
    Feint,
    /// Both strike within ±80 ms, both lethal.
    Trade,
    /// Close range (≈55 uu), short thrusts, hands touching bodies.
    Clinch,
    /// The defender falls as the attack lands.
    Fall,
}

pub const ALL_KINDS: [Kind; 7] = [Kind::Clean, Kind::Parry, Kind::MissParry, Kind::Feint, Kind::Trade, Kind::Clinch, Kind::Fall];

#[derive(Clone, Debug)]
pub struct Engagement {
    pub kind: Kind,
    pub attacker: usize,
    pub defender: usize,
    /// Planned contact time(s) of the attacker's strike(s), true ms.
    pub t_c: f64,
    pub t_end: f64,
}

#[derive(Clone, Copy, Debug)]
pub struct Mix {
    pub w: [f64; 7],
}

impl Mix {
    pub fn standard() -> Mix { Mix { w: [0.34, 0.16, 0.10, 0.08, 0.08, 0.12, 0.12] } }
    pub fn only(k: Kind) -> Mix {
        let mut w = [0.0; 7];
        w[ALL_KINDS.iter().position(|x| *x == k).unwrap()] = 1.0;
        Mix { w }
    }
    fn pick(&self, rng: &mut Rng) -> Kind {
        let tot: f64 = self.w.iter().sum();
        let mut x = rng.f() * tot;
        for (i, w) in self.w.iter().enumerate() {
            if x < *w { return ALL_KINDS[i]; }
            x -= w;
        }
        Kind::Clean
    }
}

pub struct Plan {
    pub fighters: Vec<Fighter>,
    pub engagements: Vec<Engagement>,
}

const ARMOUR_SETS: &[&[&str]] = &[
    &[],
    &["b_gambeson"],
    &["m_hauberk", "h_kettle1"],
    &["c_cuirass1", "h_sallet1", "bv_1"],
    &["c_cuirass1", "m_hauberk", "h_armet"],
];

/// Target point on the defender for a strike at t (true pose).
fn target_point(f: &Fighter, t: f64, which: u32) -> V3 {
    let p = f.pose(t);
    let (_, fw, l, u) = f.frame(t);
    match which % 6 {
        0 | 1 => add(p.bones[SPINE_04_I], mul(fw, 6.0)),
        2 => add(p.bones[SPINE_02_I], mul(fw, 8.0)),
        3 => add(p.bones[HEAD_I], mul(u, 4.0)),
        4 => lerp3(p.bones[UPPERARM_R_I], p.bones[LOWERARM_R_I], 0.5),
        _ => lerp3(p.bones[THIGH_L_I], p.bones[CALF_L_I], 0.4),
    }
}
const SPINE_02_I: usize = crate::posecodec::SPINE_02;
const SPINE_04_I: usize = crate::posecodec::SPINE_04;
const HEAD_I: usize = crate::posecodec::HEAD;
const UPPERARM_R_I: usize = crate::posecodec::UPPERARM_R;
const LOWERARM_R_I: usize = crate::posecodec::LOWERARM_R;
const THIGH_L_I: usize = crate::posecodec::THIGH_L;
const CALF_L_I: usize = crate::posecodec::CALF_L;

/// Distance (root to root) that puts the strike's target point `want` uu
/// from the attacker's right shoulder (shoulder sits 18 uu right, 46 up).
fn root_distance_for(want: f32, tp_height_above_pelvis: f32) -> f32 {
    let dz = tp_height_above_pelvis - 46.0;
    let horiz = (want * want - dz * dz).max(400.0).sqrt();
    horiz.max(30.0)
}

pub struct Builder<'a> {
    rng: &'a mut Rng,
    pub f: Vec<Fighter>,
    /// (t, focus) per fighter
    focus: Vec<Vec<(f64, usize)>>,
    pub eng: Vec<Engagement>,
    /// One-way display delay estimate (attacker → defender screen), ms.
    pub display_ms: f64,
    /// Extra reach a cheater fights from (it believes its lie), uu.
    pub extra_reach: Vec<f32>,
}

impl<'a> Builder<'a> {
    pub fn new(n: usize, rng: &'a mut Rng, display_ms: f64) -> Builder<'a> {
        let mut f = Vec::new();
        for i in 0..n {
            let w = *rng.pick(WEAPONS);
            let armour = rng.pick(ARMOUR_SETS).to_vec();
            let mut rk = [1.0f32; 15];
            for k in rk.iter_mut() { *k = rng.rf(0.85, 1.2); }
            f.push(Fighter { weapon: w, armour, path: Vec::new(), face: Vec::new(), acts: Vec::new(), falls: Vec::new(), rk });
        }
        Builder { rng, f, focus: vec![Vec::new(); n], eng: Vec::new(), display_ms, extra_reach: vec![0.0; n] }
    }

    fn put(&mut self, i: usize, t: f64, x: f32, y: f32) {
        let p = &mut self.f[i].path;
        if let Some(l) = p.last() { if t <= l.0 { return; } }
        p.push((t, x, y));
    }

    fn reach(&self, i: usize) -> f32 {
        let w = &self.f[i].weapon;
        ARM + w.grip + 0.62 * w.blade + self.extra_reach[i]
    }

    /// Plan the whole fight: `dur_ms` of back-to-back engagements.
    pub fn fight(&mut self, dur_ms: f64, mix: Mix) {
        let n = self.f.len();
        // Pair centres on a line, 700 uu apart (no cross-pair contact).
        let pairs = n / 2;
        for i in 0..n {
            let pair = (i / 2).min(pairs.saturating_sub(1));
            let cx = pair as f32 * 700.0;
            let side = if i % 2 == 0 { -1.0 } else { 1.0 };
            let (x, y) = if i >= 2 * pairs { (cx, 160.0) } else { (cx + side * 130.0, 0.0) };
            self.put(i, 0.0, x, y);
            let partner = if i >= 2 * pairs { 2 * pair } else { i ^ 1 };
            self.focus[i].push((0.0, partner.min(n - 1)));
        }
        // Streams flow through the round countdown (3 s) before anyone strikes.
        let mut t = 3500.0;
        while t < dur_ms - 1800.0 {
            let mut t_next = t;
            for pair in 0..pairs {
                let (mut a, mut d) = (2 * pair, 2 * pair + 1);
                if self.rng.chance(0.5) { std::mem::swap(&mut a, &mut d); }
                let k = mix.pick(self.rng);
                let t0 = t + self.rng.range(0.0, 150.0);
                let te = self.engage(k, a, d, t0);
                // Flanker (odd counts): strikes the pair's defender from the side.
                if n % 2 == 1 && pair == pairs - 1 && self.rng.chance(0.6) {
                    let fl = n - 1;
                    let t0 = t + self.rng.range(200.0, 500.0);
                    let tf = self.flank(fl, d, t0);
                    t_next = t_next.max(tf);
                }
                t_next = t_next.max(te);
            }
            t = t_next + self.rng.range(100.0, 400.0);
        }
    }

    fn line(&self, a: usize, d: usize, t: f64) -> (V3, V3) {
        let (ax, ay) = self.f[a].root_xy(t);
        let (dx, dy) = self.f[d].root_xy(t);
        let dir = norm([dx - ax, dy - ay, 0.0]);
        ([ax, ay, 0.0], dir)
    }

    /// One engagement starting at t; returns its end time.
    fn engage(&mut self, kind: Kind, a: usize, d: usize, t: f64) -> f64 {
        let rng_t = self.rng.range(700.0, 1000.0);
        let t_c = t + rng_t;
        let which = (self.rng.u64() % 6) as u32;
        let (pa, dir) = self.line(a, d, t);
        let mid = add(pa, mul(dir, 120.0));
        // Defender holds its ground at `dpos` (or retreats / falls).
        let dpos = add(mid, mul(dir, 60.0));
        let dist = match kind {
            Kind::Clinch => 55.0,
            _ => {
                let h = match which % 6 { 3 => 74.0, 2 => 33.0, 5 => -25.0, _ => 56.0 };
                let want = if kind == Kind::Fall { self.reach(a) - 10.0 } else { self.reach(a) };
                root_distance_for(want, h)
            }
        };
        let apos = sub(dpos, mul(dir, dist));
        self.focus[a].push((t, d));
        self.focus[d].push((t, a));
        self.put(a, t + 450.0, apos[0], apos[1]);
        self.put(d, t + 450.0, dpos[0], dpos[1]);
        // Defender motion around the strike.
        let retreat = kind == Kind::Clean && self.rng.chance(0.5);
        if retreat {
            let v = self.rng.rf(100.0, 400.0); // uu/s
            let r0 = t_c - 250.0;
            self.put(d, r0, dpos[0], dpos[1]);
            let p1 = add(dpos, mul(dir, v * 0.5));
            self.put(d, t_c + 250.0, p1[0], p1[1]);
        }
        if kind == Kind::Fall {
            self.f[d].falls.push(Fall { t0: t_c - 300.0, t1: t_c + 1500.0 });
        }
        self.put(a, t_c + 120.0, apos[0], apos[1]);
        let t_end = t_c + if kind == Kind::Fall { 1700.0 } else { 650.0 };
        let back_a = sub(apos, mul(dir, 110.0));
        let back_d = add(self.f[d].path.last().map(|l| [l.1, l.2, 0.0]).unwrap_or(dpos), mul(dir, 80.0));
        self.put(a, t_end, back_a[0], back_a[1]);
        self.put(d, t_end, back_d[0], back_d[1]);
        self.eng.push(Engagement { kind, attacker: a, defender: d, t_c, t_end });
        if kind == Kind::Trade {
            self.eng.push(Engagement { kind, attacker: d, defender: a, t_c: t_c + self.rng.range(-80.0, 80.0), t_end });
        }
        if kind == Kind::Clinch {
            self.eng.push(Engagement { kind, attacker: a, defender: d, t_c: t_c + 380.0, t_end });
        }
        if kind == Kind::Feint {
            // The feint comes first; the real attack is the engagement's t_c.
            let e = self.eng.last_mut().unwrap();
            e.t_c = t_c;
        }
        t_end
    }

    fn flank(&mut self, fl: usize, d: usize, t: f64) -> f64 {
        let t_c = t + self.rng.range(600.0, 900.0);
        let (dx, dy) = self.f[d].root_xy(t_c);
        let side = [0.0, if self.rng.chance(0.5) { 1.0 } else { -1.0 }, 0.0];
        let dist = root_distance_for(self.reach(fl), 56.0);
        let p = add([dx, dy, 0.0], mul(side, dist));
        self.focus[fl].push((t, d));
        self.put(fl, t + 450.0, p[0], p[1]);
        self.put(fl, t_c + 150.0, p[0], p[1]);
        let back = add(p, mul(side, 150.0));
        self.put(fl, t_c + 650.0, back[0], back[1]);
        self.eng.push(Engagement { kind: Kind::Clean, attacker: fl, defender: d, t_c, t_end: t_c + 650.0 });
        t_c + 650.0
    }

    /// After all paths: facings toward the focus, then the arm actions.
    pub fn finish(mut self, dur_ms: f64) -> Plan {
        let n = self.f.len();
        for i in 0..n {
            let mut face = Vec::new();
            let mut t = 0.0;
            while t <= dur_ms {
                let fo = self.focus[i].iter().rev().find(|e| e.0 <= t).map(|e| e.1).unwrap_or((i + 1) % n);
                let (x, y) = self.f[i].root_xy(t);
                let (ox, oy) = self.f[fo].root_xy(t);
                face.push((t, (oy - y).atan2(ox - x)));
                t += 50.0;
            }
            self.f[i].face = face;
        }
        let engs = self.eng.clone();
        for (k, e) in engs.iter().enumerate() {
            let which = (k as u32).wrapping_mul(2654435761) >> 29;
            match e.kind {
                Kind::Clinch => self.thrust(e.attacker, e.defender, e.t_c, which, true),
                Kind::Feint => {
                    self.feint(e.attacker, e.defender, e.t_c - 450.0);
                    self.strike(e.attacker, e.defender, e.t_c, which);
                }
                _ => self.strike(e.attacker, e.defender, e.t_c, which),
            }
        }
        for e in &engs {
            if e.kind == Kind::Parry || e.kind == Kind::MissParry {
                self.parry(e.defender, e.attacker, e.t_c, e.kind == Kind::MissParry);
            }
        }
        for f in self.f.iter_mut() {
            f.acts.sort_by(|a, b| a_start(a).total_cmp(&a_start(b)));
        }
        Plan { fighters: self.f, engagements: self.eng }
    }

    fn strike(&mut self, a: usize, d: usize, t_c: f64, which: u32) {
        let thrusty = matches!(self.f[a].weapon.class, WeaponClass::Dagger | WeaponClass::Polearm);
        if thrusty || self.rng.chance(0.25) {
            self.thrust(a, d, t_c, which, false);
        } else {
            self.swing(a, d, t_c, which);
        }
    }

    fn swing(&mut self, a: usize, d: usize, t_c: f64, which: u32) {
        let tp = target_point(&self.f[d], t_c, which);
        let s = self.f[a].shoulder_r(t_c);
        let dc = norm(sub(tp, s));
        let up = [0.0, 0.0, 1.0];
        let n0 = norm(cross(dc, up));
        let phi = self.rng.rf(0.0, 1.45) * if self.rng.chance(0.5) { 1.0 } else { -1.0 };
        let n = norm(rot(n0, dc, phi));
        let th_c = self.rng.rf(1.3, 1.8);
        let th_end = th_c + self.rng.rf(0.9, 1.4);
        let u = rot(dc, n, -th_c);
        let v = cross(n, u);
        let dur = self.rng.range(280.0, 420.0);
        // smoothstep(s_c) = th_c / th_end: invert numerically.
        let target = (th_c / th_end) as f64;
        let (mut lo, mut hi) = (0.0f64, 1.0f64);
        for _ in 0..40 {
            let m = 0.5 * (lo + hi);
            if smooth01(m) < target { lo = m } else { hi = m }
        }
        let t0 = t_c - lo * dur;
        self.f[a].acts.push(Act::Swing { t0, dur, u, v, theta_end: th_end });
    }

    fn thrust(&mut self, a: usize, d: usize, t_c: f64, which: u32, clinch: bool) {
        let tp = target_point(&self.f[d], t_c, if clinch { which % 3 } else { which });
        let s = self.f[a].shoulder_r(t_c);
        let to = sub(tp, s);
        let dd = norm(to);
        let w = self.f[a].weapon;
        // Extension at contact: the target point sits at 75 % of the blade.
        let ext_c = (len(to) - w.grip - 0.75 * w.blade).clamp(8.0, ARM);
        let ext1 = (ext_c + self.rng.rf(8.0, 20.0)).min(ARM);
        let ext0 = (ext_c - self.rng.rf(25.0, 40.0)).max(4.0);
        let dur = self.rng.range(150.0, 230.0);
        // Eased extension reaches ext_c at fraction s_c.
        let target = ((ext_c - ext0) / (ext1 - ext0).max(1.0)) as f64;
        let (mut lo, mut hi) = (0.0f64, 1.0f64);
        for _ in 0..40 {
            let m = 0.5 * (lo + hi);
            if smooth01(m) < target { lo = m } else { hi = m }
        }
        let t0 = t_c - lo * dur;
        self.f[a].acts.push(Act::Thrust { t0, dur, d: dd, ext0, ext1 });
    }

    fn feint(&mut self, a: usize, d: usize, t: f64) {
        let tp = target_point(&self.f[d], t, 0);
        let s = self.f[a].shoulder_r(t);
        let dc = norm(sub(tp, s));
        let n = norm(cross(dc, [0.0, 0.0, 1.0]));
        let u = rot(dc, n, -1.6);
        let v = cross(n, u);
        self.f[a].acts.push(Act::Feint { t0: t - 150.0, dur: 330.0, u, v, theta_max: 0.75 });
    }

    /// Defender parry: blade through the attacker's blade point (65 % along)
    /// at contact, held over the window in which the DEFENDER's screen shows
    /// the attacker's blade passing there (display delay later).
    fn parry(&mut self, d: usize, a: usize, t_c: f64, miss: bool) {
        let ap = self.f[a].pose(t_c);
        let mut p = lerp3(ap.blade.base, ap.blade.tip, 0.65);
        if miss {
            let ab = norm(sub(ap.blade.tip, ap.blade.base));
            let side = norm(cross(ab, [0.0, 0.0, 1.0]));
            p = add(p, mul(side, self.rng.rf(45.0, 80.0) * if self.rng.chance(0.5) { 1.0 } else { -1.0 }));
        }
        let c = t_c + self.display_ms;
        self.f[d].acts.push(Act::Parry { t0: c - 140.0, t1: c + 160.0, p });
    }
}

fn smooth01(x: f64) -> f64 {
    let x = x.clamp(0.0, 1.0);
    x * x * (3.0 - 2.0 * x)
}

fn a_start(a: &Act) -> f64 {
    match *a {
        Act::Swing { t0, .. } | Act::Thrust { t0, .. } | Act::Feint { t0, .. } | Act::Parry { t0, .. } => t0,
    }
}

/// A full fight plan.
pub fn plan(n: usize, dur_ms: f64, mix: Mix, display_ms: f64, extra_reach: &[f32], rng: &mut Rng) -> Plan {
    let mut r2 = rng.fork(11);
    let mut b = Builder::new(n, &mut r2, display_ms);
    for (i, e) in extra_reach.iter().enumerate().take(n) { b.extra_reach[i] = *e; }
    b.fight(dur_ms, mix);
    b.finish(dur_ms)
}
