//! Compact skeletal pose wire codec, shared by the sidecar (encode / decode
//! for stand-ins) and the server (decode into lag-compensation history).
//!
//! Frame layout:
//!   ts:u32 | root(first bone) world pos 3×f32 | n:u8 |
//!   n × [ idx:u8 | pos rel. to root 3×i16 (1 uu) | quat: largest-idx:u8 + 3×i16 ]
//! 14 B per bone → ~255 B for the full set incl. weapon (well under the
//! 1200 B budget).
//!
//! Slot 16 ("Weapon") is a pseudo-bone: the held weapon actor's world
//! transform, sampled in the same frame as the body so a swing's blade and
//! hands stay on one timeline. Lag compensation only reads the 16 real bones
//! (`BONE_COUNT`); decoders accept every `WIRE_BONES` slot.

#![allow(dead_code)]

/// Codec v2 (full physical state); see posecodec_v2.rs.
#[path = "posecodec_v2.rs"]
pub mod v2;

/// Bones the Lua writer samples (Willie skeleton names). Both ends must agree
/// on this order — a bone is sent as its index into this table. Must match
/// HERO_BONES in HSMPSync and HSMPAvatars.
pub const HERO_BONES: &[&str] = &[
    "Pelvis", "Spine_02", "Spine_04", "Head",
    "Upperarm_L", "Lowerarm_L", "Hand_L",
    "Upperarm_R", "Lowerarm_R", "Hand_R",
    "Thigh_L", "Calf_L", "Foot_L",
    "Thigh_R", "Calf_R", "Foot_R",
];
pub const BONE_COUNT: usize = 16;

/// Every slot that can appear on the wire: the hero bones + the weapon.
pub const WIRE_BONES: &[&str] = &[
    "Pelvis", "Spine_02", "Spine_04", "Head",
    "Upperarm_L", "Lowerarm_L", "Hand_L",
    "Upperarm_R", "Lowerarm_R", "Hand_R",
    "Thigh_L", "Calf_L", "Foot_L",
    "Thigh_R", "Calf_R", "Foot_R",
    "Weapon",
];
pub const WIRE_COUNT: usize = 17;
pub const WEAPON: usize = 16;

pub const PELVIS: usize = 0;
pub const SPINE_02: usize = 1;
pub const SPINE_04: usize = 2;
pub const HEAD: usize = 3;
pub const UPPERARM_L: usize = 4;
pub const LOWERARM_L: usize = 5;
pub const HAND_L: usize = 6;
pub const UPPERARM_R: usize = 7;
pub const LOWERARM_R: usize = 8;
pub const HAND_R: usize = 9;
pub const THIGH_L: usize = 10;
pub const CALF_L: usize = 11;
pub const FOOT_L: usize = 12;
pub const THIGH_R: usize = 13;
pub const CALF_R: usize = 14;
pub const FOOT_R: usize = 15;

const QUAT_SCALE: f32 = 32767.0 / std::f32::consts::FRAC_1_SQRT_2;

/// One decoded frame: sender ts (ms, sender clock) and the bones present,
/// as (HERO_BONES index, absolute world pos, quat xyzw).
#[derive(Debug, Clone)]
pub struct PoseFrame {
    pub ts: u32,
    pub bones: Vec<(u8, [f32; 3], [f32; 4])>,
}

/// Encode bones (HERO_BONES index, [px,py,pz,qx,qy,qz,qw]); the first entry is
/// the root the others are made relative to. Empty input → empty frame.
pub fn encode(ts: u32, bones: &[(u8, [f32; 7])]) -> Vec<u8> {
    if bones.is_empty() { return Vec::new(); }
    let root = [bones[0].1[0], bones[0].1[1], bones[0].1[2]];
    let mut out = Vec::with_capacity(17 + bones.len() * 14);
    out.extend_from_slice(&ts.to_le_bytes());
    for v in root { out.extend_from_slice(&v.to_le_bytes()); }
    out.push(bones.len().min(255) as u8);
    for (idx, b) in bones.iter().take(255) {
        out.push(*idx);
        for k in 0..3 {
            let rel = (b[k] - root[k]).round().clamp(-32767.0, 32767.0) as i16;
            out.extend_from_slice(&rel.to_le_bytes());
        }
        // Smallest-three: drop the largest |component|, force it positive.
        let mut q = [b[3], b[4], b[5], b[6]];
        let len = (q.iter().map(|c| c * c).sum::<f32>()).sqrt();
        if len > 1e-6 { for c in q.iter_mut() { *c /= len; } } else { q = [0.0, 0.0, 0.0, 1.0]; }
        let mut li = 0usize;
        for i in 1..4 { if q[i].abs() > q[li].abs() { li = i; } }
        if q[li] < 0.0 { for c in q.iter_mut() { *c = -*c; } }
        out.push(li as u8);
        for i in 0..4 {
            if i == li { continue; }
            let v = (q[i] * QUAT_SCALE).round().clamp(-32767.0, 32767.0) as i16;
            out.extend_from_slice(&v.to_le_bytes());
        }
    }
    out
}

/// Inverse of `encode`, or None if the frame is malformed. Unknown bone
/// indices are skipped.
pub fn decode(buf: &[u8]) -> Option<PoseFrame> {
    if v2::is_v2(buf) { return v2::decode(buf).map(|f| v2::to_v1(&f)); }
    if buf.len() < 17 { return None; }
    let rd_f = |o: usize| f32::from_le_bytes([buf[o], buf[o + 1], buf[o + 2], buf[o + 3]]);
    let rd_i = |o: usize| i16::from_le_bytes([buf[o], buf[o + 1]]) as f32;
    let ts = u32::from_le_bytes([buf[0], buf[1], buf[2], buf[3]]);
    let root = [rd_f(4), rd_f(8), rd_f(12)];
    if !root.iter().all(|v| v.is_finite()) { return None; }
    let n = buf[16] as usize;
    if buf.len() < 17 + n * 14 { return None; }
    let mut bones = Vec::with_capacity(n);
    for i in 0..n {
        let o = 17 + i * 14;
        let idx = buf[o] as usize;
        if idx >= WIRE_COUNT { continue; }
        let p = [root[0] + rd_i(o + 1), root[1] + rd_i(o + 3), root[2] + rd_i(o + 5)];
        let li = (buf[o + 7] & 3) as usize;
        let mut q = [0f32; 4];
        let mut j = 0usize;
        let mut sum = 0f32;
        for k in 0..4 {
            if k == li { continue; }
            q[k] = rd_i(o + 8 + j * 2) / QUAT_SCALE;
            sum += q[k] * q[k];
            j += 1;
        }
        q[li] = (1.0 - sum).max(0.0).sqrt();
        bones.push((idx as u8, p, q));
    }
    Some(PoseFrame { ts, bones })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encode_decode_roundtrip() {
        let bones = [
            (PELVIS as u8, [100.0, 200.0, 90.0, 0.0, 0.0, 0.0, 1.0]),
            (HAND_R as u8, [140.0, 250.0, 140.0, 0.5, 0.5, -0.5, 0.5]),
        ];
        let f = decode(&encode(42, &bones)).unwrap();
        assert_eq!(f.ts, 42);
        assert_eq!(f.bones.len(), 2);
        assert_eq!(f.bones[1].0, HAND_R as u8);
        assert!((f.bones[1].1[0] - 140.0).abs() < 0.6);
        assert!(decode(&[1, 2, 3]).is_none());
    }

    fn quat_angle(a: [f32; 4], b: [f32; 4]) -> f32 {
        let d = (a[0] * b[0] + a[1] * b[1] + a[2] * b[2] + a[3] * b[3]).abs().min(1.0);
        2.0 * d.acos()
    }

    fn norm(q: [f32; 4]) -> [f32; 4] {
        let l = (q.iter().map(|c| c * c).sum::<f32>()).sqrt();
        [q[0] / l, q[1] / l, q[2] / l, q[3] / l]
    }

    #[test]
    fn full_frame_with_weapon_roundtrips_precisely() {
        let mut bones = Vec::new();
        for i in 0..WIRE_COUNT {
            let f = i as f32;
            let q = norm([0.1 * f - 0.8, 0.37, -0.2 + 0.05 * f, 0.9 - 0.03 * f]);
            bones.push((i as u8, [1234.5 + f * 13.7, -987.25 - f * 9.1, 95.0 + f * 7.3, q[0], q[1], q[2], q[3]]));
        }
        let buf = encode(0xDEAD_BEEF, &bones);
        assert_eq!(buf.len(), 17 + WIRE_COUNT * 14);
        let f = decode(&buf).unwrap();
        assert_eq!(f.ts, 0xDEAD_BEEF);
        assert_eq!(f.bones.len(), WIRE_COUNT);
        for (i, (idx, p, q)) in f.bones.iter().enumerate() {
            assert_eq!(*idx as usize, i);
            let src = bones[i].1;
            for k in 0..3 { assert!((p[k] - src[k]).abs() <= 0.51, "bone {} axis {} {} vs {}", i, k, p[k], src[k]); }
            let ang = quat_angle(*q, [src[3], src[4], src[5], src[6]]);
            assert!(ang < 0.001, "bone {} rotation error {} rad", i, ang);
        }
        assert_eq!(WIRE_BONES[WEAPON], "Weapon");
        assert_eq!(&WIRE_BONES[..BONE_COUNT], HERO_BONES);
    }

    #[test]
    fn quaternion_sign_and_largest_component_cases() {
        // Each component as the largest, both signs, plus near-degenerate input.
        let cases: [[f32; 4]; 6] = [
            [0.9, 0.1, 0.1, 0.4], [0.1, -0.95, 0.2, 0.2], [0.0, 0.0, 1.0, 0.0],
            [-0.3, 0.2, 0.1, -0.92], [0.5, 0.5, 0.5, 0.5], [0.0, 0.0, 0.0, 0.0],
        ];
        for c in cases {
            let buf = encode(1, &[(0, [0.0, 0.0, 0.0, c[0], c[1], c[2], c[3]])]);
            let q = decode(&buf).unwrap().bones[0].2;
            let want = if c.iter().all(|v| *v == 0.0) { [0.0, 0.0, 0.0, 1.0] } else { norm(c) };
            assert!(quat_angle(q, want) < 0.001, "{:?} -> {:?}", c, q);
        }
    }

    #[test]
    fn unknown_slots_skipped_and_offsets_clamped() {
        let mut buf = encode(9, &[(0, [0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 1.0]),
                                 (3, [50000.0, 0.0, 10.0, 0.0, 0.0, 0.0, 1.0])]);
        // Far bone clamps to the i16 range instead of wrapping.
        let f = decode(&buf).unwrap();
        assert!((f.bones[1].1[0] - 32767.0).abs() < 1.0);
        // Corrupt the second bone's index to an unknown slot: it is skipped.
        buf[17 + 14] = 200;
        let f = decode(&buf).unwrap();
        assert_eq!(f.bones.len(), 1);
        // Non-finite root is rejected.
        let mut bad = encode(1, &[(0, [0.0; 7])]);
        bad[4..8].copy_from_slice(&f32::NAN.to_le_bytes());
        assert!(decode(&bad).is_none());
    }
}
