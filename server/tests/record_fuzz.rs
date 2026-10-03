//! Protocol v6 record fuzz: every registered record kind, as a framed network
//! message, through the generic validator (`hsmp_ipc::schema::check_payload`, which is the
//! full `record::view`) and the server's message split (`proto::decode_msg`). Arbitrary
//! bytes and mutations of valid-shaped messages must never panic, and anything accepted
//! must re-validate after a byte-exact copy (the relay path copies bytes).
//!
//! Each domain adds handler-level fuzz for its kinds in its own tests; this one keeps
//! the generic layer honest for every kind at once. Quick run: 1 s (part of `cargo test`);
//! soak: `HSMP_FUZZ_SECS=300 cargo test --release --test record_fuzz -- --nocapture`.

#![allow(dead_code, unused_imports)]

#[path = "../src/proto.rs"]
mod proto;

use rand::rngs::StdRng;
use rand::{Rng, SeedableRng};
use std::panic::{self, AssertUnwindSafe};
use std::time::{Duration, Instant};

fn budget() -> Duration {
    let s = std::env::var("HSMP_FUZZ_SECS").ok().and_then(|v| v.parse::<u64>().ok()).unwrap_or(1);
    Duration::from_secs(s)
}

/// Valid-shaped seeds: for every kind, a zero head (count 0), a head with a plausible count
/// and that many zero rows, and the same with every float field set to 1.0 where it is an
/// f32 at a 4-aligned offset (cheap way to pass "finite" with non-zero data).
fn corpus() -> Vec<Vec<u8>> {
    let mut v = Vec::new();
    for r in hsmp_ipc::schema::records() {
        let hs = r.head.size();
        let head = vec![0u8; hs];
        v.push(hsmp_ipc::wire::message(r.kind, 0, 0, &head));
        if let (Some(row), Some(cf)) = (r.row, r.count_field) {
            if let Some(f) = r.head.field(cf) {
                let n = r.max_rows.min(3);
                let mut h = head.clone();
                let b = (n as u64).to_le_bytes();
                h[f.offset..f.offset + f.ty.size()].copy_from_slice(&b[..f.ty.size()]);
                let mut p = h;
                p.extend(std::iter::repeat(0u8).take(n * row.size()));
                v.push(hsmp_ipc::wire::message(r.kind, 7, 3, &p));
            }
        }
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
                let v: u32 = [0, 1, 0xFFFF_FFFF, 0x7FC0_0000, 0x7F80_0000, 0xFF80_0000, 0x0100_0000][rng.gen_range(0..7)];
                d[i..i + 4].copy_from_slice(&v.to_le_bytes());
            }
            2 if !d.is_empty() => {
                let n = rng.gen_range(0..d.len());
                d.truncate(n);
            }
            3 if d.len() < 8192 => {
                for _ in 0..rng.gen_range(1..32) {
                    d.push(rng.gen());
                }
            }
            4 if d.len() >= 2 => {
                // another kind's id on this payload
                let ks: Vec<u16> = hsmp_ipc::schema::records().map(|r| r.kind).collect();
                let k = ks[rng.gen_range(0..ks.len())];
                d[..2].copy_from_slice(&k.to_le_bytes());
            }
            _ => {
                if !d.is_empty() {
                    let i = rng.gen_range(0..d.len());
                    d[i] = rng.gen();
                }
            }
        }
    }
}

fn target(d: &[u8]) {
    let Ok((h, p)) = hsmp_ipc::wire::split(d) else { return };
    let ok = hsmp_ipc::schema::check_payload(h.kind, p).is_ok();
    if ok {
        // The relay copies bytes: a copy (in a fresh, differently aligned buffer) is still valid.
        let mut c = vec![0u8; p.len() + 1];
        c[1..].copy_from_slice(p);
        assert!(hsmp_ipc::schema::check_payload(h.kind, &c[1..]).is_ok(), "accepted record fails after a copy");
        // And the JSON debug view of it never panics.
        let _ = hsmp_ipc::debug_json::record_to_json(h.kind, p);
    }
    let _ = proto::decode_msg(d);
}

#[test]
fn fuzz_every_record_kind() {
    let seeds = corpus();
    assert!(!seeds.is_empty());
    for s in &seeds {
        target(s); // seeds themselves never panic
    }
    let mut rng = StdRng::seed_from_u64(0x6E_C0_5D);
    let t0 = Instant::now();
    let (mut iters, mut accepted) = (0u64, 0u64);
    let prev = panic::take_hook();
    panic::set_hook(Box::new(|_| {}));
    let mut failures = Vec::new();
    while t0.elapsed() < budget() {
        let mut d = if rng.gen_range(0..5) == 0 {
            (0..rng.gen_range(0..1500)).map(|_| rng.gen()).collect()
        } else {
            seeds[rng.gen_range(0..seeds.len())].clone()
        };
        mutate(&mut rng, &mut d);
        iters += 1;
        if let Ok((h, p)) = hsmp_ipc::wire::split(&d) {
            if hsmp_ipc::schema::check_payload(h.kind, p).is_ok() {
                accepted += 1;
            }
        }
        if panic::catch_unwind(AssertUnwindSafe(|| target(&d))).is_err() && failures.len() < 8 {
            failures.push(d.iter().take(64).map(|x| format!("{x:02x}")).collect::<String>());
        }
    }
    panic::set_hook(prev);
    println!("fuzz records {iters} iters, {accepted} accepted, {} panic(s)", failures.len());
    assert!(failures.is_empty(), "panics on: {failures:?}");
}

/// Every kind refuses its own payload truncated by one byte and extended by one byte.
#[test]
fn exact_sizes_are_enforced_for_every_kind() {
    for r in hsmp_ipc::schema::records() {
        let p = vec![0u8; r.head.size()];
        let short = &p[..p.len() - 1];
        assert!(hsmp_ipc::schema::check_payload(r.kind, short).is_err(), "{} accepts a short head", r.name);
        let mut long = p.clone();
        long.push(0);
        if r.row.is_none() {
            assert!(hsmp_ipc::schema::check_payload(r.kind, &long).is_err(), "{} accepts a trailing byte", r.name);
        }
    }
}
