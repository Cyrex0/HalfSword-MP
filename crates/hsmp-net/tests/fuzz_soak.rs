//! Time-boxed fuzz soak of the `hsmp_net::fuzz` entry points.
//!
//! cargo-fuzz/libFuzzer does not run on the Windows dev box, so this is a
//! seeded random + mutation driver over the same entry points the future
//! `fuzz/` crate will call. Ignored by default (it runs for minutes):
//!
//!     $env:HSMP_FUZZ_SECS=300; cargo test --release -p hsmp-net \
//!         --test fuzz_soak -- --ignored --nocapture --test-threads 1
//!
//! HSMP_FUZZ_SECS (default 10) is the budget PER TARGET, HSMP_FUZZ_SEED the
//! RNG seed (default 0x5EED). A panic records the input (hex) and the panic
//! message + location, and the test fails listing every crashing target.

use std::panic::{self, AssertUnwindSafe};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use hsmp_net::fuzz;
use hsmp_net::net::{Client, ClientConfig, ConnConfig, Incoming, ServerConfig, ServerEndpoint};
use rand::rngs::StdRng;
use rand::{Rng, SeedableRng};

fn budget() -> Duration {
    let s = std::env::var("HSMP_FUZZ_SECS").ok().and_then(|v| v.parse::<u64>().ok()).unwrap_or(10);
    Duration::from_secs(s)
}

fn seed() -> u64 {
    std::env::var("HSMP_FUZZ_SEED").ok().and_then(|v| u64::from_str_radix(v.trim_start_matches("0x"), 16).ok()).unwrap_or(0x5EED)
}

/// Mutate `d` in place: bit flips, byte sets, interesting values, truncation,
/// duplication, splicing with another corpus entry, insertion.
fn mutate(rng: &mut StdRng, d: &mut Vec<u8>, corpus: &[Vec<u8>]) {
    const INTERESTING: [u8; 9] = [0, 1, 0x7F, 0x80, 0xFF, 0xFE, 0x10, 0x40, 0x20];
    for _ in 0..rng.gen_range(1..6) {
        match rng.gen_range(0..9) {
            0 if !d.is_empty() => {
                let i = rng.gen_range(0..d.len());
                d[i] ^= 1 << rng.gen_range(0..8);
            }
            1 if !d.is_empty() => {
                let i = rng.gen_range(0..d.len());
                d[i] = rng.gen();
            }
            2 if !d.is_empty() => {
                let i = rng.gen_range(0..d.len());
                d[i] = INTERESTING[rng.gen_range(0..INTERESTING.len())];
            }
            3 if !d.is_empty() => {
                let n = rng.gen_range(0..d.len());
                d.truncate(n);
            }
            4 if !d.is_empty() && d.len() < 4096 => {
                let a = rng.gen_range(0..d.len());
                let b = rng.gen_range(a..d.len().min(a + 64) + 1).min(d.len());
                let chunk = d[a..b].to_vec();
                let at = rng.gen_range(0..=d.len());
                d.splice(at..at, chunk);
            }
            5 if !corpus.is_empty() => {
                let o = &corpus[rng.gen_range(0..corpus.len())];
                let cut = rng.gen_range(0..=d.len());
                let from = rng.gen_range(0..=o.len());
                d.truncate(cut);
                d.extend_from_slice(&o[from..]);
            }
            6 if d.len() >= 4 => {
                // a little-endian length/count field set to an extreme
                let i = rng.gen_range(0..d.len() - 3);
                let v: u32 = [0, 1, 0xFFFF, 0x7FFF_FFFF, 0xFFFF_FFFF, 65_536][rng.gen_range(0..6)];
                d[i..i + 4].copy_from_slice(&v.to_le_bytes());
            }
            7 if d.len() < 4096 => {
                let at = rng.gen_range(0..=d.len());
                let n = rng.gen_range(1..16);
                for k in 0..n {
                    d.insert(at + k, rng.gen());
                }
            }
            _ => {
                if d.len() < 4096 {
                    d.push(rng.gen());
                }
            }
        }
    }
}

pub struct Outcome {
    pub name: &'static str,
    pub iters: u64,
    pub panics: Vec<String>,
}

/// Run `f` over random + mutated inputs for `budget`, catching panics.
fn soak(name: &'static str, corpus: &[Vec<u8>], budget: Duration, seed: u64, f: &dyn Fn(&[u8])) -> Outcome {
    let last_panic: Arc<Mutex<String>> = Arc::new(Mutex::new(String::new()));
    let lp = last_panic.clone();
    let prev = panic::take_hook();
    panic::set_hook(Box::new(move |info| {
        let msg = info
            .payload()
            .downcast_ref::<&str>()
            .map(|s| s.to_string())
            .or_else(|| info.payload().downcast_ref::<String>().cloned())
            .unwrap_or_else(|| "<non-string payload>".into());
        let loc = info.location().map(|l| format!("{}:{}", l.file(), l.line())).unwrap_or_default();
        *lp.lock().unwrap_or_else(|e| e.into_inner()) = format!("{msg} @ {loc}");
    }));
    let mut rng = StdRng::seed_from_u64(seed ^ name.len() as u64);
    let t0 = Instant::now();
    let mut iters = 0u64;
    let mut panics = vec![];
    let mut seen = std::collections::BTreeSet::new();
    while t0.elapsed() < budget {
        for _ in 0..64 {
            let mut d: Vec<u8> = if corpus.is_empty() || rng.gen_range(0..4) == 0 {
                let n = rng.gen_range(0..1500);
                (0..n).map(|_| rng.gen()).collect()
            } else {
                corpus[rng.gen_range(0..corpus.len())].clone()
            };
            mutate(&mut rng, &mut d, corpus);
            iters += 1;
            if panic::catch_unwind(AssertUnwindSafe(|| f(&d))).is_err() {
                let msg = last_panic.lock().unwrap_or_else(|e| e.into_inner()).clone();
                if seen.insert(msg.clone()) && panics.len() < 16 {
                    let hex: String = d.iter().take(256).map(|b| format!("{b:02x}")).collect();
                    panics.push(format!("{msg}\n      input ({} B): {hex}", d.len()));
                }
            }
        }
    }
    panic::set_hook(prev);
    Outcome { name, iters, panics }
}

fn corpus() -> Vec<Vec<u8>> {
    let mut rng = StdRng::seed_from_u64(0xC0);
    let mut c = Client::new(ClientConfig::new([3; 32], "seed"), ConnConfig::default(), 0, &mut rng);
    let hello = c.poll_transmit(0).expect("hello");
    let mut ep = ServerEndpoint::new(ServerConfig::new([7; 32]), ConnConfig::default(), 1_000, Some(1));
    let challenge = match ep.handle(1_000, "192.0.2.1:7777".parse().unwrap(), &hello) {
        Incoming::Reply(r) => r,
        _ => vec![],
    };
    c.handle(1_001, &challenge);
    let auth = c.poll_transmit(1_002).unwrap_or_default();
    let mut v = vec![
        hello,
        challenge,
        auth,
        vec![0x07, 0x02, 0, 0, 0, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0, 0], // a v6 record message
    ];
    // fragment-record shaped input and replay sequences
    v.push((0u8..64).collect());
    v.push((0u32..64).flat_map(|s| (s * 3).to_le_bytes()).collect());
    // u32-tag shaped inputs with a zero / random tail
    for k in 0u32..48 {
        let mut z = k.to_le_bytes().to_vec();
        z.extend(std::iter::repeat(0u8).take(64));
        v.push(z);
        let mut r = k.to_le_bytes().to_vec();
        r.extend((0..96).map(|_| rng.gen::<u8>()));
        v.push(r);
    }
    v
}

#[test]
#[ignore = "long-running fuzz soak; run with --ignored (HSMP_FUZZ_SECS per target)"]
fn fuzz_soak_all_entry_points() {
    let b = budget();
    let s = seed();
    let corpus = corpus();
    let targets: Vec<(&'static str, fn(&[u8]))> = vec![
        ("server_datagram", fuzz::server_datagram),
        ("client_handshake", fuzz::client_handshake),
        ("conn_payload", fuzz::conn_payload),
        ("conn_datagram", fuzz::conn_datagram),
        ("reassembly", fuzz::reassembly),
        ("replay", fuzz::replay),
    ];
    let mut bad = vec![];
    for (name, f) in targets {
        let o = soak(name, &corpus, b, s, &f);
        println!("fuzz {:<17} {:>6.0}s {:>10} iters  {} panic(s)", o.name, b.as_secs_f64(), o.iters, o.panics.len());
        for p in &o.panics {
            println!("   PANIC {p}");
        }
        if !o.panics.is_empty() {
            bad.push(o.name);
        }
    }
    assert!(bad.is_empty(), "panicking fuzz targets: {bad:?}");
}
