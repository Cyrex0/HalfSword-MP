//! Hit location across the replay delay.
//!
//! Deal Complex Damage maps the hit point into the hit bone's space and traces
//! the armour layers that cover THAT spot of the bone (a breastplate covers the
//! front of the torso, an open helmet not the face). The victim replays a blow
//! a round trip (plus jitter buffer and parry hold) after the attacker's
//! stand-in showed it, and fighters turn and lean in between. This measures,
//! over real simulated fights, how often the replayed spot falls under a
//! different armour layer than the spot the blade touched:
//!
//! * `world`: the claim's world offset from the bone re-added to the victim's
//!   bone at replay time (HSMPCombat before hits were replayed in bone space);
//! * `local`: the offset in the bone's own frame (HSMPCombat `BF.LOCAL`).
//!
//! The stand-in's servo error moves where the blade meets the body; that is the
//! same for both and stays (it is what the attacker saw). The sim's capsule
//! frames take their roll from the body's facing, which is not rigid for
//! twisting arms: the `local` floor here (~0.2 %) is that, not the method.

use super::body::SEGS;
use super::world::{ClaimKind, World};
use super::*;

#[derive(Clone, Debug, Default)]
pub struct LocParity {
    pub n: usize,
    /// Replayed spot under a different armour layer than the touched spot (or
    /// inside the body where the touch was on its surface, or the reverse).
    pub world_off: usize,
    pub local_off: usize,
    /// Replayed point deep inside the body where the touch was not (the trace
    /// then finds nothing).
    pub world_inside: usize,
    pub local_inside: usize,
    /// |yaw change| of the victim between the pose shown and the replay (deg).
    pub turn: Vec<f32>,
}

impl LocParity {
    pub fn add(&mut self, o: &LocParity) {
        self.n += o.n;
        self.world_off += o.world_off;
        self.local_off += o.local_off;
        self.world_inside += o.world_inside;
        self.local_inside += o.local_inside;
        self.turn.extend_from_slice(&o.turn);
    }
    pub fn pct(&self, k: usize) -> f64 { if self.n == 0 { 0.0 } else { 100.0 * k as f64 / self.n as f64 } }
}

/// Orthonormal frame of a capsule: z along the bone, x the body's forward
/// direction made orthogonal to it, y = z × x.
fn seg_frame(a: V3, b: V3, fwd: V3) -> [V3; 3] {
    let z = norm(sub(b, a));
    let mut x = sub(fwd, mul(z, dot(fwd, z)));
    if len(x) < 1e-3 { x = cross(z, [0.0, 0.0, 1.0]); }
    let x = norm(x);
    [x, cross(z, x), z]
}
fn to_local(f: &[V3; 3], v: V3) -> V3 { [dot(v, f[0]), dot(v, f[1]), dot(v, f[2])] }
fn to_world(f: &[V3; 3], v: V3) -> V3 { add(add(mul(f[0], v[0]), mul(f[1], v[1])), mul(f[2], v[2])) }

/// Armour layer id covering local direction `d` (unit) of capsule `seg`:
/// breastplate on the front of the torso, an open-faced helmet, vambraces and
/// cuisses on the outer / front side. 0 = flesh (or cloth) only.
pub fn layer(seg: usize, d: V3) -> u8 {
    match SEGS[seg].3 {
        "pelvis" | "spine_03" => if d[0] > -0.2 { 1 } else { 0 },
        "head" => if d[0] > 0.55 && d[2].abs() < 0.6 { 0 } else { 2 },
        "upperarm_l" | "lowerarm_l" | "upperarm_r" | "lowerarm_r" => if d[0] > -0.5 { 3 } else { 0 },
        "thigh_l" | "thigh_r" | "calf_l" | "calf_r" => if d[0] > -0.2 { 4 } else { 0 },
        _ => 5,
    }
}

/// Spot (layer, inside) of world point `p` on capsule (a, b, r) with frame `f`.
fn spot(seg: usize, a: V3, b: V3, r: f32, f: &[V3; 3], p: V3) -> (u8, bool) {
    let (dist, u) = point_seg(p, a, b);
    let axis = lerp3(a, b, u);
    let d = to_local(f, norm(sub(p, axis)));
    (layer(seg, d), dist < 0.5 * r)
}

/// Every honest blade claim the victim replayed in `w`.
pub fn location_parity(w: &World) -> LocParity {
    let mut o = LocParity::default();
    for r in &w.claims {
        if r.kind != ClaimKind::Honest { continue; }
        let Some((seg, shown_t, loc)) = r.geo else { continue };
        let Some(t_rep) = r.applied_t.or(r.forwarded_t) else { continue };
        if r.outcome.as_ref().map_or(true, |x| !x.0) { continue; }
        let f = &w.plan.fighters[r.target];
        let cap_at = |t: f64| {
            let caps = f.true_caps(&f.pose(t));
            caps.into_iter().find(|c| c.seg == seg).map(|c| (c.a, c.b, c.r, f.frame(t).1))
        };
        let (Some((a0, b0, r0, f0)), Some((a1, b1, r1, f1))) = (cap_at(shown_t), cap_at(t_rep)) else { continue };
        let fr0 = seg_frame(a0, b0, f0);
        let fr1 = seg_frame(a1, b1, f1);
        let solo = spot(seg, a0, b0, r0, &fr0, loc);
        // world offset re-added at the replay pose
        let pw = add(a1, sub(loc, a0));
        let sw = spot(seg, a1, b1, r1, &fr1, pw);
        // bone-local offset (the stand-in's bone frame and the bodies the blade
        // touched are the same physics bodies: the conversion is exact)
        let pl = add(a1, to_world(&fr1, to_local(&fr0, sub(loc, a0))));
        let sl = spot(seg, a1, b1, r1, &fr1, pl);
        o.n += 1;
        // (a blade already embedded touches inside the body in solo too)
        if sw != solo { o.world_off += 1; }
        if sl != solo { o.local_off += 1; }
        if sw.1 && !solo.1 { o.world_inside += 1; }
        if sl.1 && !solo.1 { o.local_inside += 1; }
        let dyaw = (f.yaw(t_rep) - f.yaw(shown_t)).to_degrees();
        o.turn.push(((dyaw + 540.0) % 360.0 - 180.0).abs());
    }
    o
}
