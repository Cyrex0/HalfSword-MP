//! Pose domain record fuzz (protocol v6): `root`, `weapon` and `pose` messages exactly as the
//! game's native module produces them (`hsmp_pose::sample`), mutated, through the generic
//! validator and then the domain's own consumers: the server's ONE pose decode (codec v2
//! structural check + lag compensation extras + the v1 hero-bone view), the receiving
//! sidecar's jitter-buffer frame and its `peer_root` composition. Nothing may panic; an
//! accepted pose must decode to finite bones; an accepted root re-validates as a peer_root.
//!
//! Quick run: 1 s (part of `cargo test`); soak:
//! `HSMP_FUZZ_SECS=300 cargo test --release --test pose_fuzz -- --nocapture`.

use hsmp_ipc::schema::pose::{PeerRoot, PoseBuf, PoseHead, Root, Weapon, K_PEER_ROOT, K_POSE, K_ROOT, K_WEAPON};
use hsmp_pose::posecodec::v2;
use hsmp_pose::{poseplay, sample};
use rand::rngs::StdRng;
use rand::{Rng, SeedableRng};
use std::time::{Duration, Instant};

fn budget() -> Duration {
    let s = std::env::var("HSMP_FUZZ_SECS").ok().and_then(|v| v.parse::<u64>().ok()).unwrap_or(1);
    Duration::from_secs(s)
}

fn corpus() -> Vec<Vec<u8>> {
    let mut v = Vec::new();
    for k in 0..4u32 {
        let t = k as f64;
        let r = sample::root(&[t, 1000.0 + t, 100.0 * t, -50.0, 95.0, 5.0, 30.0 * t, 0.0, 300.0, 0.0, 0.0], 1_700_000_000_000);
        v.push(hsmp_ipc::wire::encode(0, 0, &r, &[]));
        let w = sample::weapon(&[t, 1000.0, 7.0, 1.0 + (k % 2) as f64, 10.0, 20.0, 95.0, 0.0, 90.0, 0.0, 1.0, 2.0, 3.0]);
        v.push(hsmp_ipc::wire::encode(0, 0, &w, &[]));
        // A FK body with a held weapon; control on odd samples.
        let mut b = [0f64; sample::BONE_NUMS];
        let mut pos = vec![[0f64; 3]; v2::NB];
        pos[0] = [100.0 * t, -50.0, 95.0];
        for i in 0..v2::NB {
            if i > 0 {
                let p = pos[v2::PARENT[i]];
                pos[i] = [p[0] + v2::REF_T[i][0] as f64, p[1] + v2::REF_T[i][1] as f64, p[2] + v2::REF_T[i][2] as f64];
            }
            b[i * 13..i * 13 + 3].copy_from_slice(&pos[i]);
            b[i * 13 + 6] = 1.0;
            b[i * 13 + 7] = 250.0 * t;
        }
        let wv = [1.0, 3.0, 40.0, -50.0, 120.0, 0.0, 0.0, 0.0, 1.0, 500.0, 0.0, 0.0, 0.0, 900.0, 0.0, 40.0, -50.0, 130.0, 40.0, -50.0, 220.0];
        let c: [f64; sample::CONTROL_NUMS] = std::array::from_fn(|i| i as f64 * 0.5);
        let a = sample::PoseArgs { tick: t, ts: 1000.0 + 16.7 * t, dt: 8.3, b: &b, w: &[wv], c: (k % 2 == 1).then_some(&c) };
        let mut out = PoseBuf::new_boxed();
        assert!(sample::encode_pose(&a, &mut sample::Scratch::default(), &mut out));
        v.push(hsmp_ipc::wire::message(K_POSE, 0, 0, out.payload()));
    }
    v
}

fn mutate(rng: &mut StdRng, d: &mut Vec<u8>) {
    for _ in 0..rng.gen_range(1..5) {
        match rng.gen_range(0..6) {
            0 if !d.is_empty() => {
                let i = rng.gen_range(0..d.len());
                d[i] ^= 1 << rng.gen_range(0..8);
            }
            1 if d.len() >= 4 => {
                let i = rng.gen_range(0..d.len() - 3);
                let v: u32 = [0, 1, 0xFFFF_FFFF, 0x7FC0_0000, 0x7F80_0000, 0xFF80_0000, 0x4B18_9680][rng.gen_range(0..7)];
                d[i..i + 4].copy_from_slice(&v.to_le_bytes());
            }
            2 if !d.is_empty() => {
                let n = rng.gen_range(0..d.len());
                d.truncate(n);
            }
            3 if d.len() < 2048 => {
                for _ in 0..rng.gen_range(1..32) {
                    d.push(rng.gen());
                }
            }
            4 if d.len() >= 16 => {
                // keep the record valid-shaped: rewrite the pose head's byte count to the length
                let n = (d.len() - 16) as u16;
                d[12..14].copy_from_slice(&n.to_le_bytes());
            }
            _ => {
                if d.len() > 8 {
                    let i = rng.gen_range(8..d.len());
                    d[i] = rng.gen();
                }
            }
        }
    }
}

/// Every consumer of one message; returns whether it was accepted as a pose domain record.
fn consume(msg: &[u8]) -> bool {
    let Ok((h, p)) = hsmp_ipc::wire::split(msg) else { return false };
    if hsmp_ipc::schema::check_payload(h.kind, p).is_err() {
        return false;
    }
    match h.kind {
        K_ROOT => {
            let r = hsmp_ipc::record::view::<Root>(p).unwrap().head();
            assert!(r.pos.iter().chain(r.rot.iter()).chain(r.vel.iter()).all(|x| x.is_finite()));
            // the receiving sidecar's peer_root: the bytes behind the owner id, re-validated
            let mut rec = vec![0u8; std::mem::size_of::<PeerRoot>()];
            rec[..4].copy_from_slice(&7u32.to_le_bytes());
            rec[8..].copy_from_slice(p);
            hsmp_ipc::schema::check_payload(K_PEER_ROOT, &rec).expect("an accepted root is a valid peer_root");
            let _ = poseplay::quat_yaw(r.rot);
            true
        }
        K_WEAPON => {
            let w = hsmp_ipc::record::view::<Weapon>(p).unwrap().head();
            assert!(w.held <= 2);
            true
        }
        K_POSE => {
            let v = hsmp_ipc::record::view::<PoseHead>(p).unwrap();
            if let Some(f) = v2::decode(&v.rows) {
                assert!(f.bones.iter().all(|b| b.p.iter().chain(b.q.iter()).all(|x| x.is_finite())), "decoded bones are finite");
                let x = v2::extras_of(&f);
                let _ = v2::to_v1(&f);
                assert!(x.caps.iter().all(|c| c.0.iter().chain(c.1.iter()).all(|v| v.is_finite())));
                let fr = poseplay::Frame::from_v2(v.head.tick, &f);
                let mut pb = poseplay::Playback::new();
                pb.push_pose(1000.0, fr);
                let _ = pb.sample(1100.0);
            }
            true
        }
        _ => false,
    }
}

#[test]
fn pose_records_survive_hostile_bytes() {
    let corpus = corpus();
    for m in &corpus {
        assert!(consume(m), "every seed is a valid record");
    }
    let mut rng = StdRng::seed_from_u64(0x9053);
    let t0 = Instant::now();
    let (mut n, mut ok) = (0u64, 0u64);
    while t0.elapsed() < budget() {
        let mut d = if n % 8 == 0 {
            let k = [K_ROOT, K_WEAPON, K_POSE][rng.gen_range(0..3)];
            let mut d = k.to_le_bytes().to_vec();
            d.extend((0..rng.gen_range(0..700)).map(|_| rng.gen::<u8>()));
            d
        } else {
            corpus[rng.gen_range(0..corpus.len())].clone()
        };
        mutate(&mut rng, &mut d);
        if consume(&d) {
            ok += 1;
        }
        n += 1;
    }
    println!("pose_fuzz: {n} messages, {ok} accepted");
    assert!(n > 1000);
}
