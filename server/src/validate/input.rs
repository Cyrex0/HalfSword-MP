//! Input gate: every float a client sends is checked here before any game
//! logic sees it. `NaN > x` is false, so one NaN position would otherwise
//! switch off the speed cap (`accept_root`) and every range check built on
//! `last_valid_pos`, and be relayed to every client. A record that fails the
//! gate is dropped whole by its domain handler (`pose_glue`).
//!
//! What is checked where:
//! - here: the `root` / `weapon` records (`check_root`, `check_weapon`: position, rotation,
//!   velocity);
//! - downstream, because those paths answer the sender (a reject the
//!   sidecar needs to stop resending) or filter per object: v6 records are
//!   validated in place by `hsmp_ipc::record::view` (combat: `combat_glue`, then
//!   `combat::validate` / `sanitize_deltas`; world: finite, `WORLD_LIMIT`, and the
//!   poses again by `world::Level::pos_ok`: finite + bound + arena box).
//!
//! Integer fields (timestamps, sequence numbers, ids) cannot be non-finite;
//! their semantic checks live with their consumers.
//!
//! `fuzz_root_record` is the panic-free fuzz entry point (cargo-fuzz:
//! `fuzz_target!(|d: &[u8]| hsmp_server::validate::input::fuzz_root_record(d))`
//! once the server exposes a lib target; the unit test runs it over random
//! and mutated input meanwhile).

use hsmp_ipc::schema::pose::{Root, Weapon};

/// World positions (UE cm). Same bound as `world::WORLD_BOUND` (20 km).
pub const POS_BOUND: f32 = 2.0e6;
/// Rotations: quaternion components (a sender may not normalise exactly).
pub const ROT_BOUND: f32 = 16.0;
/// Velocities (cm/s): 10 km/s is far beyond anything a ragdoll does.
pub const VEL_BOUND: f32 = 1.0e6;

fn within<const N: usize>(v: &[f32; N], bound: f32) -> bool {
    v.iter().all(|x| x.is_finite() && x.abs() <= bound)
}

/// A world position a client may report: finite and inside the world bound.
pub fn pos_ok(p: &[f32; 3]) -> bool { within(p, POS_BOUND) }
pub fn rot_ok(q: &[f32; 4]) -> bool { within(q, ROT_BOUND) }
pub fn vel_ok(v: &[f32; 3]) -> bool { within(v, VEL_BOUND) }

fn stream_ok(position: &[f32; 3], rotation: &[f32; 4], vel: &[f32; 3]) -> Result<(), &'static str> {
    if !pos_ok(position) { return Err("position"); }
    if !rot_ok(rotation) { return Err("rotation"); }
    if !vel_ok(vel) { return Err("velocity"); }
    Ok(())
}

/// The `root` record (protocol v6) after `record::view`: position inside the world bound,
/// rotation and velocity bounded (the record check only bounds the position at 1e7 and the
/// quaternion norm).
pub fn check_root(r: &Root) -> Result<(), &'static str> {
    stream_ok(&r.pos, &r.rot, &r.vel)
}

/// The `weapon` record (protocol v6), as [`check_root`].
pub fn check_weapon(w: &Weapon) -> Result<(), &'static str> {
    stream_ok(&w.pos, &w.rot, &w.vel)
}

/// Root speed a body can sustain (a 300 m/s cap would let a
/// root move 10 m per tick): a sprint is ~7 m/s, a ragdoll launched by a
/// heavy blow or a fall reaches ~15 m/s.
pub const ROOT_SPEED_MAX: f32 = 1500.0;
/// Burst allowance on top of `ROOT_SPEED_MAX · dt` (uu): packets bunched by
/// jitter / a delay spike, a ragdoll impulse. A step beyond this in one go is
/// a teleport; a player that really moved further is accepted again once
/// its time since the last accepted root covers the distance.
pub const ROOT_BURST: f32 = 300.0;

/// Speed-capped root step (anti-teleport), NaN-safe: `prev` = last accepted
/// position and how many seconds ago. Rejects non-finite or out-of-bound
/// positions on the first packet too, and any step not provably within
/// `max_speed · dt + burst` (a NaN distance is a reject, never a pass).
pub fn root_step_ok(prev: Option<([f32; 3], f32)>, pos: [f32; 3], max_speed: f32) -> bool {
    root_step_ok_burst(prev, pos, max_speed, 0.0)
}

/// `root_step_ok` with a burst allowance (uu).
pub fn root_step_ok_burst(prev: Option<([f32; 3], f32)>, pos: [f32; 3], max_speed: f32, burst: f32) -> bool {
    if !pos_ok(&pos) { return false; }
    match prev {
        None => true,
        Some((p, dt_s)) => {
            if !pos_ok(&p) { return true; } // cannot happen; a bad baseline must not lock the player out
            let (dx, dy, dz) = (pos[0] - p[0], pos[1] - p[1], pos[2] - p[2]);
            let d = (dx * dx + dy * dy + dz * dz).sqrt();
            d <= max_speed * dt_s.max(1e-3) + burst
        }
    }
}

/// Fuzz entry point for a `root` record payload: never panics; an accepted root has a
/// usable position and passes the first-packet speed cap.
pub fn fuzz_root_record(data: &[u8]) {
    let Ok(v) = hsmp_ipc::record::view::<Root>(data) else { return };
    let r = v.head();
    if check_root(&r).is_ok() {
        assert!(pos_ok(&r.pos));
        assert!(root_step_ok(None, r.pos, 1.0));
    }
    // Whatever came before, a bad position never passes the speed cap.
    if !pos_ok(&r.pos) { assert!(!root_step_ok(Some(([0.0; 3], 0.0333)), r.pos, f32::MAX)); }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    fn root(pos: [f32; 3]) -> Root {
        Root { tick: 1, ts: 2, send_wall_ms: 3, pos, rot: [0.0, 0.0, 0.0, 1.0], vel: [0.0; 3] }
    }

    #[test]
    fn non_finite_floats_are_refused_everywhere() {
        for bad in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY, 3.0e6] {
            assert!(check_root(&root([bad, 0.0, 0.0])).is_err(), "{bad}");
            assert!(check_root(&Root { rot: [bad * 10.0, 0.0, 0.0, 1.0], ..root([0.0; 3]) }).is_err());
            assert!(check_weapon(&Weapon { tick: 1, ts: 1, weapon_id: 1, held: 0, _r: 0, _r2: 0, pos: [0.0; 3],
                rot: [0.0, 0.0, 0.0, 1.0], vel: [0.0, bad * 1e3, 0.0] }).is_err());
        }
        assert!(check_root(&root([100.0, -200.0, 50.0])).is_ok());
    }

    #[test]
    fn nan_never_passes_the_speed_cap() {
        let prev = Some(([0.0; 3], 1.0 / 30.0));
        assert!(!root_step_ok(prev, [f32::NAN, 0.0, 0.0], 1500.0));
        assert!(!root_step_ok(None, [f32::NAN, 0.0, 0.0], 1500.0), "first packet too");
        assert!(!root_step_ok(None, [f32::INFINITY, 0.0, 0.0], 1500.0));
        // A teleport after a (refused) NaN is still measured from the last good position.
        assert!(!root_step_ok(prev, [50_000.0, 0.0, 0.0], 1500.0));
        assert!(root_step_ok(prev, [10.0, 0.0, 0.0], 1500.0));
    }

    #[test]
    fn fuzz_entry_point_survives_random_and_mutated_input() {
        use rand::{Rng, SeedableRng};
        let mut rng = rand::rngs::StdRng::seed_from_u64(0x1A9);
        let seeds: Vec<Vec<u8>> = vec![
            bytemuck::bytes_of(&root([1.0, 2.0, 3.0])).to_vec(),
            vec![0u8; 8],
        ];
        for i in 0..5_000 {
            let mut d: Vec<u8> = if i % 4 == 0 {
                (0..rng.gen_range(0..200)).map(|_| rng.gen()).collect()
            } else {
                seeds[i % seeds.len()].clone()
            };
            for _ in 0..rng.gen_range(0..6) {
                if !d.is_empty() {
                    let j = rng.gen_range(0..d.len());
                    d[j] = rng.gen();
                }
            }
            let mut r = bytemuck::bytes_of(&root([1.0, 2.0, 3.0])).to_vec();
            for _ in 0..rng.gen_range(0..6) {
                let j = rng.gen_range(0..r.len());
                r[j] = rng.gen();
            }
            fuzz_root_record(&r);
            fuzz_root_record(&d);
        }
    }

    /// The root cap is a body's speed plus a short burst, not
    /// 300 m/s: a 10 m step in one tick is refused, a sprint and bunched
    /// packets pass, and a player that really travelled far is accepted
    /// again once the elapsed time covers it.
    #[test]
    fn root_cap_is_a_plausible_body_speed() {
        let tick = 1.0 / 30.0;
        let ok = |d: f32, dt: f32| root_step_ok_burst(Some(([0.0; 3], dt)), [d, 0.0, 0.0], ROOT_SPEED_MAX, ROOT_BURST);
        assert!(!ok(1000.0, tick), "10 m in one tick: teleport");
        assert!(!ok(500.0, tick), "5 m in one tick");
        assert!(ok(700.0 * tick, tick), "sprint");
        assert!(ok(250.0, 0.001), "packets bunched by jitter");
        assert!(ok(1000.0, 0.5), "10 m after half a second (fall / knock-back)");
        assert!(!ok(f32::NAN, 1.0));
    }

    proptest! {
        /// Any accepted root position is finite and bounded, for any bit
        /// pattern of the three floats.
        #[test]
        fn accepted_roots_are_finite(a in any::<u32>(), b in any::<u32>(), c in any::<u32>()) {
            let p = [f32::from_bits(a), f32::from_bits(b), f32::from_bits(c)];
            if check_root(&root(p)).is_ok() {
                prop_assert!(p.iter().all(|x| x.is_finite() && x.abs() <= POS_BOUND));
            }
            let prev = Some(([0.0; 3], 0.0333));
            if root_step_ok(prev, p, 1500.0) {
                prop_assert!(p.iter().all(|x| x.is_finite()));
            }
        }
    }
}
