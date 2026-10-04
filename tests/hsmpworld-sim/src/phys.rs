//! A small rigid-body world per client: spheres with a rest height on a flat floor, gravity,
//! bounce, friction, tumbling that settles flat, sphere-sphere contacts and kinematic pawn
//! pushers. Not Chaos, but it has the property that matters for sync: two clients stepping
//! the same body with their own frame times (and slightly different contact noise) end up in
//! different places unless replication keeps them together.

use crate::Rng;

pub type V3 = [f64; 3];
/// Quaternion [x, y, z, w] (UE convention).
pub type Q = [f64; 4];

pub const G: f64 = 980.0;
pub const FLOOR: f64 = 0.0;

pub fn add(a: V3, b: V3) -> V3 { [a[0] + b[0], a[1] + b[1], a[2] + b[2]] }
pub fn sub(a: V3, b: V3) -> V3 { [a[0] - b[0], a[1] - b[1], a[2] - b[2]] }
pub fn mul(a: V3, k: f64) -> V3 { [a[0] * k, a[1] * k, a[2] * k] }
pub fn dot(a: V3, b: V3) -> f64 { a[0] * b[0] + a[1] * b[1] + a[2] * b[2] }
pub fn len(a: V3) -> f64 { dot(a, a).sqrt() }
pub fn dist(a: V3, b: V3) -> f64 { len(sub(a, b)) }

pub fn qmul(a: Q, b: Q) -> Q {
    [
        a[3] * b[0] + a[0] * b[3] + a[1] * b[2] - a[2] * b[1],
        a[3] * b[1] - a[0] * b[2] + a[1] * b[3] + a[2] * b[0],
        a[3] * b[2] + a[0] * b[1] - a[1] * b[0] + a[2] * b[3],
        a[3] * b[3] - a[0] * b[0] - a[1] * b[1] - a[2] * b[2],
    ]
}
pub fn qnorm(q: Q) -> Q {
    let n = (q[0] * q[0] + q[1] * q[1] + q[2] * q[2] + q[3] * q[3]).sqrt();
    if n < 1e-12 { [0.0, 0.0, 0.0, 1.0] } else { [q[0] / n, q[1] / n, q[2] / n, q[3] / n] }
}
/// Angle between two orientations, degrees.
pub fn qangle(a: Q, b: Q) -> f64 {
    let d = (a[0] * b[0] + a[1] * b[1] + a[2] * b[2] + a[3] * b[3]).abs().min(1.0);
    2.0 * d.acos().to_degrees()
}
pub fn qslerp_lin(a: Q, b: Q, t: f64) -> Q {
    let d = a[0] * b[0] + a[1] * b[1] + a[2] * b[2] + a[3] * b[3];
    let s = if d < 0.0 { -1.0 } else { 1.0 };
    qnorm([
        a[0] + (s * b[0] - a[0]) * t,
        a[1] + (s * b[1] - a[1]) * t,
        a[2] + (s * b[2] - a[2]) * t,
        a[3] + (s * b[3] - a[3]) * t,
    ])
}

/// UE FRotator::Quaternion (degrees).
pub fn rot_to_quat(pitch: f64, yaw: f64, roll: f64) -> Q {
    let h = std::f64::consts::PI / 360.0;
    let (sp, cp) = (pitch * h).sin_cos();
    let (sy, cy) = (yaw * h).sin_cos();
    let (sr, cr) = (roll * h).sin_cos();
    [cr * sp * sy - sr * cp * cy, -cr * sp * cy - sr * cp * sy, cr * cp * sy - sr * sp * cy, cr * cp * cy + sr * sp * sy]
}

fn wrap180(d: f64) -> f64 { (d + 180.0).rem_euclid(360.0) - 180.0 }

/// UE FQuat::Rotator (pitch, yaw, roll degrees), as HSMPWorld's quat_to_rot.
pub fn quat_to_rot(q: Q) -> V3 {
    let [x, y, z, w] = q;
    let sing = z * x - w * y;
    let yaw = (2.0 * (w * z + x * y)).atan2(1.0 - 2.0 * (y * y + z * z)).to_degrees();
    if sing < -0.4999995 {
        [-90.0, yaw, wrap180(-yaw - 2.0 * x.atan2(w).to_degrees())]
    } else if sing > 0.4999995 {
        [90.0, yaw, wrap180(yaw - 2.0 * x.atan2(w).to_degrees())]
    } else {
        let pitch = (2.0 * sing).clamp(-1.0, 1.0).asin().to_degrees();
        let roll = (-2.0 * (w * x + y * z)).atan2(1.0 - 2.0 * (x * x + y * y)).to_degrees();
        [pitch, yaw, roll]
    }
}

/// Yaw-only part of an orientation (how a body lies once it has settled flat).
fn flat(q: Q) -> Q {
    let r = quat_to_rot(q);
    rot_to_quat(0.0, r[1], 0.0)
}

#[derive(Clone, Debug)]
pub struct Body {
    pub pos: V3,
    pub vel: V3,
    pub q: Q,
    /// Angular velocity, rad/s (world).
    pub w: V3,
    /// Contact sphere radius and rest height of the centre above the floor.
    pub r: f64,
    pub h: f64,
    pub mass: f64,
    /// Simulating (dynamic) or kinematic.
    pub sim: bool,
    pub sleeping: bool,
    still: f64,
    pub alive: bool,
    /// Attached to a pawn hand (kinematic, placed by the owner of the Phys).
    pub attached: bool,
}

impl Body {
    pub fn new(pos: V3, yaw: f64, r: f64, h: f64, mass: f64, sim: bool) -> Body {
        Body { pos, vel: [0.0; 3], q: rot_to_quat(0.0, yaw, 0.0), w: [0.0; 3], r, h, mass, sim, sleeping: true,
               still: 1.0, alive: true, attached: false }
    }
    pub fn wake(&mut self) { self.sleeping = false; self.still = 0.0; }
    pub fn on_ground(&self) -> bool { self.pos[2] <= FLOOR + self.h + 0.5 }
}

/// A kinematic pusher (a pawn or stand-in): a vertical capsule approximated by a sphere at
/// body height.
#[derive(Clone, Copy, Debug)]
pub struct Pusher {
    pub pos: V3,
    pub vel: V3,
    pub r: f64,
}

pub struct Phys {
    pub bodies: Vec<Body>,
    /// Per-client contact noise (Chaos is not deterministic across machines and frame rates).
    pub fric_scale: f64,
    rng: Rng,
}

impl Phys {
    pub fn new(rng: &mut Rng) -> Phys {
        let mut r = rng.fork(91);
        let fric_scale = 1.0 + r.range(-0.04, 0.04);
        Phys { bodies: Vec::new(), fric_scale, rng: r }
    }

    pub fn add(&mut self, b: Body) -> usize {
        self.bodies.push(b);
        self.bodies.len() - 1
    }

    pub fn step(&mut self, dt: f64, pushers: &[Pusher]) {
        let n = self.bodies.len();
        for i in 0..n {
            let b = &mut self.bodies[i];
            if !b.alive || !b.sim || b.attached || b.sleeping { continue; }
            b.vel[2] -= G * dt;
            b.pos = add(b.pos, mul(b.vel, dt));
            // orientation: q' = q + 0.5 * (w, 0) * q * dt
            let wq = [b.w[0] * 0.5 * dt, b.w[1] * 0.5 * dt, b.w[2] * 0.5 * dt, 0.0];
            let dq = qmul(wq, b.q);
            b.q = qnorm([b.q[0] + dq[0], b.q[1] + dq[1], b.q[2] + dq[2], b.q[3] + dq[3]]);
            let zmin = FLOOR + b.h;
            if b.pos[2] < zmin {
                b.pos[2] = zmin;
                if b.vel[2] < -60.0 {
                    // bounce: restitution 0.3, a little contact noise into the tangent and the spin
                    let vz = -b.vel[2];
                    b.vel[2] = vz * 0.3;
                    let k = vz * 0.04;
                    b.vel[0] += self.rng.range(-k, k);
                    b.vel[1] += self.rng.range(-k, k);
                    b.w[0] += self.rng.range(-1.0, 1.0) * vz * 0.004;
                    b.w[1] += self.rng.range(-1.0, 1.0) * vz * 0.004;
                } else {
                    b.vel[2] = 0.0;
                }
            }
            if b.on_ground() {
                // Coulomb friction on the floor
                let hv = [b.vel[0], b.vel[1], 0.0];
                let sp = len(hv);
                let dv = 0.45 * G * self.fric_scale * dt;
                if sp <= dv {
                    b.vel[0] = 0.0;
                    b.vel[1] = 0.0;
                } else {
                    let k = (sp - dv) / sp;
                    b.vel[0] *= k;
                    b.vel[1] *= k;
                }
                let damp = (-7.0 * self.fric_scale * dt).exp();
                b.w = mul(b.w, damp);
                // settle flat (pitch / roll relax, yaw stays)
                let t = 1.0 - (-5.0 * dt).exp();
                b.q = qslerp_lin(b.q, flat(b.q), t);
            } else {
                b.w = mul(b.w, (-0.3 * dt).exp());
            }
        }
        // sphere-sphere contacts between simulating bodies
        for i in 0..n {
            for j in (i + 1)..n {
                let (a, b) = { let (l, r) = self.bodies.split_at_mut(j); (&mut l[i], &mut r[0]) };
                if !a.alive || !b.alive || a.attached || b.attached { continue; }
                if !(a.sim || b.sim) { continue; }
                let d = sub(b.pos, a.pos);
                let dl = len(d);
                let rr = a.r + b.r;
                if dl >= rr || dl < 1e-6 { continue; }
                let nrm = mul(d, 1.0 / dl);
                let pen = rr - dl;
                let rel = dot(sub(b.vel, a.vel), nrm);
                let (ia, ib) = (if a.sim { 1.0 / a.mass } else { 0.0 }, if b.sim { 1.0 / b.mass } else { 0.0 });
                if ia + ib == 0.0 { continue; }
                let corr = pen / (ia + ib);
                a.pos = sub(a.pos, mul(nrm, corr * ia));
                b.pos = add(b.pos, mul(nrm, corr * ib));
                if rel < 0.0 {
                    let jimp = -(1.0 + 0.2) * rel / (ia + ib);
                    a.vel = sub(a.vel, mul(nrm, jimp * ia));
                    b.vel = add(b.vel, mul(nrm, jimp * ib));
                    if rel < -20.0 {
                        if a.sim { a.wake(); }
                        if b.sim { b.wake(); }
                    }
                }
            }
        }
        // kinematic pushers (pawns and stand-ins)
        for p in pushers {
            for b in self.bodies.iter_mut() {
                if !b.alive || !b.sim || b.attached { continue; }
                if (b.pos[2] - p.pos[2]).abs() > 110.0 { continue; }
                let d = [b.pos[0] - p.pos[0], b.pos[1] - p.pos[1], 0.0];
                let dl = len(d);
                let rr = b.r + p.r;
                if dl >= rr || dl < 1e-6 { continue; }
                let nrm = mul(d, 1.0 / dl);
                // depenetration is speed-capped (Chaos MaxDepenetrationVelocity), never a teleport
                b.pos = add(b.pos, mul(nrm, (rr - dl).min(4.0)));
                let vn = dot(b.vel, nrm);
                let pn = dot(p.vel, nrm).max(0.0) * 1.1 + 15.0;
                if vn < pn {
                    b.vel = add(b.vel, mul(nrm, pn - vn));
                    b.w[2] += self.rng.range(-0.5, 0.5);
                }
                b.wake();
            }
        }
        // sleep
        for b in self.bodies.iter_mut() {
            if !b.alive || !b.sim || b.attached || b.sleeping { continue; }
            if b.on_ground() && len(b.vel) < 3.0 && len(b.w) < 0.05 {
                b.still += dt;
                if b.still > 0.4 {
                    b.sleeping = true;
                    b.vel = [0.0; 3];
                    b.w = [0.0; 3];
                }
            } else {
                b.still = 0.0;
            }
        }
    }
}
