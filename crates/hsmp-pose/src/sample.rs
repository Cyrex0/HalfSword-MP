//! The game-side sample -> the ONE canonical record (quantisation only at the source; see
//! docs/development/ipc-shared-memory.md). HSMPNative's hot paths (`put_root`, `put_weapon`,
//! `put_pose`) call these with the numbers Lua passed; the tools that stand in for the game (`hsmp-tools` synth / net-bench /
//! pose-probe / ipc-game) call the same functions, so they measure and produce exactly what the
//! game does.
//!
//! The conversions reproduce the pre-ABI-2 path bit for bit (native `LocalPose` slot ->
//! sidecar `pose_frame` -> `v2::from_parts` -> `v2::encode`): every Lua number goes through
//! `f32` first (the old slot's field type), the step `dt` too, the rotator becomes a
//! quaternion in f64 from those f32 degrees, `ts` stays f64 for the frame and becomes whole ms
//! for root / weapon. `tests::frames_match_the_pre_abi2_path` proves it.

use hsmp_ipc::schema::pose::{PoseBuf, PoseHead, Root, Weapon, POSE_FRAME_MAX};

use crate::posecodec::v2;

/// Bone numbers of one sample: NB bones x (p xyz, q xyzw, v xyz, w xyz).
pub const BONE_NUMS: usize = v2::NB * 13;
/// Numbers per weapon: hands, class, p xyz, q xyzw, v xyz, w xyz, base xyz, tip xyz.
pub const WEAPON_NUMS: usize = 21;
/// Numbers of the control layer: flags, grip_r, grip_l, 16 scalars, aim xyz, ctrl pitch,
/// yaw, 4 x ik xyz, ik_world (ignored: derived by the encoder).
pub const CONTROL_NUMS: usize = 37;

/// UE FRotator (degrees) -> quaternion (x, y, z, w).
pub fn rotator_to_quat(pitch_deg: f64, yaw_deg: f64, roll_deg: f64) -> [f32; 4] {
    let deg2rad = std::f64::consts::PI / 180.0;
    let (hp, hy, hr) = (pitch_deg * 0.5 * deg2rad, yaw_deg * 0.5 * deg2rad, roll_deg * 0.5 * deg2rad);
    let (sp, cp) = hp.sin_cos();
    let (sy, cy) = hy.sin_cos();
    let (sr, cr) = hr.sin_cos();
    [
        (cr * sp * sy - sr * cp * cy) as f32,
        (-cr * sp * cy - sr * cp * sy) as f32,
        (cr * cp * sy - sr * sp * cy) as f32,
        (cr * cp * cy + sr * sp * sy) as f32,
    ]
}

/// Yaw (degrees) of a quaternion (x, y, z, w), as FQuat::Rotator.
pub fn quat_yaw(q: [f32; 4]) -> f32 {
    crate::poseplay::quat_yaw(q)
}

fn rot_of(p: f64, y: f64, r: f64) -> [f32; 4] {
    rotator_to_quat(p as f32 as f64, y as f32 as f64, r as f32 as f64)
}

fn v3(a: f64, b: f64, c: f64) -> [f32; 3] {
    [a as f32, b as f32, c as f32]
}

/// `put_root(tick, ts, px, py, pz, pitch, yaw, roll, vx, vy, vz)` as the `root` record.
/// `wall_ms` = the wall clock now (ms since the Unix epoch).
pub fn root(a: &[f64; 11], wall_ms: u64) -> Root {
    Root {
        tick: a[0] as u32,
        ts: a[1] as u64 as u32,
        send_wall_ms: wall_ms,
        pos: v3(a[2], a[3], a[4]),
        rot: rot_of(a[5], a[6], a[7]),
        vel: v3(a[8], a[9], a[10]),
    }
}

/// `put_weapon(tick, ts, weapon_id, held, px, py, pz, pitch, yaw, roll, vx, vy, vz)` as the
/// `weapon` record.
pub fn weapon(a: &[f64; 13]) -> Weapon {
    Weapon {
        tick: a[0] as u32,
        ts: a[1] as u64 as u32,
        weapon_id: a[2] as u32 as u16,
        held: (a[3] as u32).min(255) as u8,
        _r: 0,
        _r2: 0,
        pos: v3(a[4], a[5], a[6]),
        rot: rot_of(a[7], a[8], a[9]),
        vel: v3(a[10], a[11], a[12]),
    }
}

/// One pose sample as HSMPNative `put_pose(tick, ts, dt, k, b, w, c)` receives it.
pub struct PoseArgs<'a> {
    pub tick: f64,
    /// Sender game clock, ms (sub-ms).
    pub ts: f64,
    /// The physics step that produced the state, ms (0 = unknown).
    pub dt: f64,
    pub b: &'a [f64; BONE_NUMS],
    /// Up to 2 weapons (extra ones are ignored).
    pub w: &'a [[f64; WEAPON_NUMS]],
    pub c: Option<&'a [f64; CONTROL_NUMS]>,
}

/// Reusable scratch of [`encode_pose`] (no allocation per sample once warm).
#[derive(Default)]
pub struct Scratch {
    b: Vec<f32>,
    frame: Vec<u8>,
}

/// The sample as a codec v2 frame in `out` (head + bytes). False (out unchanged) when a bone
/// number is not finite (the frame would not decode the same) — the old sidecar dropped
/// such a sample too.
pub fn encode_pose(a: &PoseArgs<'_>, s: &mut Scratch, out: &mut PoseBuf) -> bool {
    s.b.clear();
    s.b.extend(a.b.iter().map(|x| *x as f32));
    let mut ws = [[0f32; WEAPON_NUMS]; 2];
    let nw = a.w.len().min(2);
    for (wi, w) in a.w.iter().take(2).enumerate() {
        let v = w.map(|x| x as f32);
        let o = &mut ws[wi];
        // The old slot held hands / class as u32.
        o[0] = v[0] as u32 as f32;
        o[1] = v[1] as u32 as f32;
        o[2..].copy_from_slice(&v[2..]);
    }
    let ctl = a.c.map(|c| v2::Control {
        flags: c[0] as i64 as u32,
        grip_r: (c[1] as u32).min(255) as u8,
        grip_l: (c[2] as u32).min(255) as u8,
        scalars: std::array::from_fn(|i| c[3 + i] as f32),
        aim: [c[19] as f32, c[20] as f32, c[21] as f32],
        ctrl_pitch: c[22] as f32,
        ctrl_yaw: c[23] as f32,
        ik: std::array::from_fn(|i| [c[24 + i * 3] as f32, c[25 + i * 3] as f32, c[26 + i * 3] as f32]),
        ik_world: [false; 4],
    });
    let tick = a.tick as u32;
    let Some((_, f)) = v2::from_parts(tick, a.ts, a.dt as f32 as f64, &s.b, &ws[..nw], ctl) else { return false };
    v2::encode_into(&f, &mut s.frame);
    if s.frame.len() > POSE_FRAME_MAX {
        return false;
    }
    out.set(&PoseHead { tick, n: 0, _r: 0 }, &s.frame);
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use hsmp_ipc::record::{view, VarBuf};

    /// A frozen copy of the pre-ABI-2 path: HSMPNative wrote the Lua numbers into a
    /// `LocalPose` slot (f32 fields, u32 hands / class / grips) and the sidecar's
    /// `ipc_shm::pose_frame` rebuilt the encoder input from it.
    fn old_path(a: &PoseArgs<'_>) -> Option<Vec<u8>> {
        // native put_pose -> LocalPose
        let tick = a.tick as u32;
        let ts_ms = a.ts;
        let dt_ms = a.dt as f32;
        let mut pb = [[0f32; 13]; v2::NB];
        for bone in 0..v2::NB {
            for j in 0..13 {
                pb[bone][j] = a.b[bone * 13 + j] as f32;
            }
        }
        struct Ws { hands: u32, class_id: u32, xf: [f32; 13], base: [f32; 3], tip: [f32; 3] }
        let mut wsamp = Vec::new();
        for w in a.w.iter().take(2) {
            let v = w.map(|x| x as f32);
            let mut xf = [0f32; 13];
            xf.copy_from_slice(&v[2..15]);
            wsamp.push(Ws { hands: v[0] as u32, class_id: v[1] as u32, xf, base: [v[15], v[16], v[17]], tip: [v[18], v[19], v[20]] });
        }
        let ctl = a.c.map(|v| {
            let flags = v[0] as i64 as u32;
            let (gr, gl) = (v[1] as u32, v[2] as u32);
            let mut scalars = [0f32; 16];
            for i in 0..16 {
                scalars[i] = v[3 + i] as f32;
            }
            let mut ik = [[0f32; 3]; 4];
            for i in 0..4 {
                ik[i] = [v[24 + i * 3] as f32, v[25 + i * 3] as f32, v[26 + i * 3] as f32];
            }
            (flags, gr, gl, scalars, [v[19] as f32, v[20] as f32, v[21] as f32], v[22] as f32, v[23] as f32, ik)
        });
        // sidecar pose_frame
        let mut b = Vec::with_capacity(v2::NB * 13);
        for bone in &pb {
            b.extend_from_slice(bone);
        }
        let mut ws: Vec<[f32; 21]> = Vec::new();
        for w in &wsamp {
            let mut x = [0f32; 21];
            x[0] = w.hands as f32;
            x[1] = w.class_id as f32;
            x[2..15].copy_from_slice(&w.xf);
            x[15..18].copy_from_slice(&w.base);
            x[18..21].copy_from_slice(&w.tip);
            ws.push(x);
        }
        let c = ctl.map(|(flags, gr, gl, scalars, aim, p, y, ik)| v2::Control {
            flags,
            grip_r: gr.min(255) as u8,
            grip_l: gl.min(255) as u8,
            scalars,
            aim,
            ctrl_pitch: p,
            ctrl_yaw: y,
            ik,
            ik_world: [false; 4],
        });
        let (_, f) = v2::from_parts(tick, ts_ms, dt_ms as f64, &b, &ws, c)?;
        Some(v2::encode(&f))
    }

    struct Rng(u64);
    impl Rng {
        fn f(&mut self) -> f64 {
            self.0 ^= self.0 << 13;
            self.0 ^= self.0 >> 7;
            self.0 ^= self.0 << 17;
            (self.0 >> 11) as f64 / (1u64 << 53) as f64 * 2.0 - 1.0
        }
    }

    /// A plausible sample in Lua's f64 numbers (FK from random local rotations, with noise
    /// so overrides, weapons, control and odd values all occur).
    fn sample(seed: u64) -> ([f64; BONE_NUMS], Vec<[f64; WEAPON_NUMS]>, [f64; CONTROL_NUMS], f64, f64) {
        let mut r = Rng(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1);
        let f = crate::posecodec::v2::tests::sample_state(seed as u32, 0.95 + 0.1 * r.f().abs() as f32);
        let mut b = [0f64; BONE_NUMS];
        for (i, x) in f.bones.iter().enumerate() {
            let jit = if seed % 3 == 0 { 0.37 * r.f() } else { 1e-4 * r.f() };
            let o = i * 13;
            b[o] = x.p[0] as f64 + jit;
            b[o + 1] = x.p[1] as f64 + 1e-3 * r.f();
            b[o + 2] = x.p[2] as f64;
            for k in 0..4 {
                b[o + 3 + k] = x.q[k] as f64 * (1.0 + 1e-3 * r.f());
            }
            for k in 0..3 {
                b[o + 7 + k] = x.v[k] as f64 + 0.123456789;
                b[o + 10 + k] = x.w[k] as f64 - 0.987654321;
            }
        }
        let wq = f.weapons[0].q;
        let w = [1.0, 7.9, 10.1 + r.f(), -20.2, 95.3, wq[0] as f64, wq[1] as f64, wq[2] as f64, wq[3] as f64, 2500.5, -900.25, 100.0,
            1800.0, 0.0, -600.0, 11.0, -19.5, 112.0, 12.0, -18.0, 200.0];
        let mut w2 = w;
        w2[0] = 2.0;
        w2[1] = 300.0; // class over 255: saturates on both paths
        let ws = match seed % 4 { 0 => vec![], 1 => vec![w], _ => vec![w, w2] };
        let mut c = [0f64; CONTROL_NUMS];
        for (i, x) in c.iter_mut().enumerate() {
            *x = r.f() * 50.0 + i as f64;
        }
        c[0] = 0xA5A5_0001u32 as f64;
        c[1] = 3.7;
        c[2] = 260.0;
        let pel = [b[0], b[1], b[2]];
        if seed % 2 == 0 {
            for i in 0..2 {
                for k in 0..3 {
                    c[24 + i * 3 + k] = pel[k] + 30.0 * r.f();
                }
            }
        }
        let ts = 123_456.0 + seed as f64 * 16.6667 + 0.123;
        let dt = [0.0, 8.333333333, 16.7, 300.0, -1.0][(seed % 5) as usize];
        (b, ws, c, ts, dt)
    }

    #[test]
    fn frames_match_the_pre_abi2_path() {
        let mut s = Scratch::default();
        let mut out = VarBuf::<PoseHead, POSE_FRAME_MAX>::new_boxed();
        for seed in 0..400u64 {
            let (b, ws, c, ts, dt) = sample(seed);
            let a = PoseArgs { tick: seed as f64 + 0.7, ts, dt, b: &b, w: &ws, c: (seed % 2 == 1).then_some(&c) };
            let want = old_path(&a).expect("old path encodes");
            assert!(encode_pose(&a, &mut s, &mut out), "seed {seed}");
            assert_eq!(out.used(), &want[..], "seed {seed}: frame bytes differ");
            assert_eq!(out.head.tick, seed as u32);
            let v = view::<PoseHead>(out.payload()).expect("the record validates");
            assert_eq!(&*v.rows, &want[..]);
        }
    }

    #[test]
    fn non_finite_bones_are_refused() {
        let (mut b, ws, _, ts, dt) = sample(1);
        b[40] = f64::NAN;
        let a = PoseArgs { tick: 1.0, ts, dt, b: &b, w: &ws, c: None };
        let mut out = VarBuf::<PoseHead, POSE_FRAME_MAX>::new_boxed();
        assert!(!encode_pose(&a, &mut Scratch::default(), &mut out));
        assert!(old_path(&a).is_none());
    }

    #[test]
    fn root_and_weapon_records() {
        let r = root(&[42.9, 1000.75, 500.0, 0.0, 100.0, 0.0, 90.0, 0.0, 1.0, 2.0, 3.0], 1_700_000_000_123);
        assert_eq!((r.tick, r.ts, r.send_wall_ms), (42, 1000, 1_700_000_000_123));
        assert!((quat_yaw(r.rot) - 90.0).abs() < 1e-3);
        hsmp_ipc::record::check(&r, &[]).unwrap();
        let w = weapon(&[3.0, 5.5, 77.0, 1.0, 1.0, 2.0, 3.0, 10.0, 20.0, 30.0, 0.0, 0.0, 0.0]);
        assert_eq!((w.tick, w.ts, w.weapon_id, w.held), (3, 5, 77, 1));
        hsmp_ipc::record::check(&w, &[]).unwrap();
        let bad = weapon(&[3.0, 5.5, 77.0, 7.0, 1.0, 2.0, 3.0, 10.0, 20.0, 30.0, 0.0, 0.0, 0.0]);
        assert!(hsmp_ipc::record::check(&bad, &[]).is_err(), "held 7 is out of range");
    }
}

#[cfg(test)]
mod bench {
    use super::*;

    /// `cargo test -p hsmp-pose --release -- --ignored --nocapture bench_encode`: the game-side
    /// cost of one pose record (from_parts + encode + VarBuf copy), and its parts.
    #[test]
    #[ignore]
    fn bench_encode() {
        let f = crate::posecodec::v2::tests::sample_state(3, 1.0);
        let mut b = [0f64; BONE_NUMS];
        for (i, x) in f.bones.iter().enumerate() {
            let v = [x.p[0], x.p[1], x.p[2], x.q[0], x.q[1], x.q[2], x.q[3], x.v[0], x.v[1], x.v[2], x.w[0], x.w[1], x.w[2]];
            for k in 0..13 {
                b[i * 13 + k] = v[k] as f64;
            }
        }
        let w = [1.0, 3.0, 10.0, 20.0, 95.0, 0.0, 0.0, 0.0, 1.0, 100.0, 0.0, 0.0, 0.0, 0.0, 0.0, 11.0, 20.0, 95.0, 90.0, 20.0, 95.0];
        let c: [f64; CONTROL_NUMS] = std::array::from_fn(|i| i as f64);
        let mut s = Scratch::default();
        let mut out = PoseBuf::new_boxed();
        for (label, ctl) in [("no control", None), ("with control", Some(&c))] {
            let a = PoseArgs { tick: 1.0, ts: 1000.0, dt: 8.3, b: &b, w: std::slice::from_ref(&w), c: ctl };
            for _ in 0..1000 {
                encode_pose(&a, &mut s, &mut out);
            }
            let n = 100_000;
            let t = std::time::Instant::now();
            for _ in 0..n {
                std::hint::black_box(encode_pose(std::hint::black_box(&a), &mut s, &mut out));
            }
            println!("TIMING encode_pose ({label}, {} B): {:.3} us", out.n(), t.elapsed().as_secs_f64() * 1e6 / n as f64);
        }
        let n = 100_000;
        let t = std::time::Instant::now();
        for _ in 0..n {
            std::hint::black_box(v2::encode(std::hint::black_box(&f)));
        }
        println!("TIMING v2::encode alone: {:.3} us", t.elapsed().as_secs_f64() * 1e6 / n as f64);
    }
}
