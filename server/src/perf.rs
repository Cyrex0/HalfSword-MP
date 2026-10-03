//! Opt-in server performance counters (`HSMP_PERF=1`). Every 5 s logs tick
//! processing time, per-packet handling time, packet/byte rates and frames
//! thinned by relevance or dropped by the per-client bandwidth budget.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::Duration;
use tracing::info;

pub struct Perf {
    pub enabled: AtomicBool,
    tick_us: Mutex<Vec<u32>>,
    handle_us: Mutex<Vec<u32>>,
    pub pkts_in: AtomicU64,
    pub pkts_out: AtomicU64,
    pub bytes_out: AtomicU64,
    pub thinned: AtomicU64,
    pub budget_drops: AtomicU64,
}

pub fn perf() -> &'static Perf {
    static P: OnceLock<Perf> = OnceLock::new();
    P.get_or_init(|| Perf {
        enabled: AtomicBool::new(std::env::var("HSMP_PERF").map(|v| v == "1").unwrap_or(false)),
        tick_us: Mutex::new(Vec::new()),
        handle_us: Mutex::new(Vec::new()),
        pkts_in: AtomicU64::new(0),
        pkts_out: AtomicU64::new(0),
        bytes_out: AtomicU64::new(0),
        thinned: AtomicU64::new(0),
        budget_drops: AtomicU64::new(0),
    })
}

impl Perf {
    #[inline]
    pub fn on(&self) -> bool { self.enabled.load(Ordering::Relaxed) }
    pub fn tick(&self, us: u32) { if self.on() { self.tick_us.lock().unwrap().push(us); } }
    pub fn handle(&self, us: u32) { if self.on() { self.handle_us.lock().unwrap().push(us); } }
    #[inline]
    pub fn sent(&self, bytes: usize) {
        self.pkts_out.fetch_add(1, Ordering::Relaxed);
        self.bytes_out.fetch_add(bytes as u64, Ordering::Relaxed);
    }
}

fn pct(v: &mut [u32], p: f64) -> u32 {
    if v.is_empty() { return 0; }
    v.sort_unstable();
    v[((v.len() as f64 - 1.0) * p).round() as usize]
}

/// Spawns the 5 s reporter if `HSMP_PERF=1`.
pub fn spawn_reporter() {
    let p = perf();
    if !p.on() { return; }
    tokio::spawn(async move {
        let mut iv = tokio::time::interval(Duration::from_secs(5));
        iv.tick().await;
        loop {
            iv.tick().await;
            let mut t = std::mem::take(&mut *p.tick_us.lock().unwrap());
            let mut h = std::mem::take(&mut *p.handle_us.lock().unwrap());
            let ticks = t.len();
            let (t50, t99, tmax) = (pct(&mut t, 0.5), pct(&mut t, 0.99), t.iter().copied().max().unwrap_or(0));
            let (h50, h99) = (pct(&mut h, 0.5), pct(&mut h, 0.99));
            let pin = p.pkts_in.swap(0, Ordering::Relaxed) / 5;
            let pout = p.pkts_out.swap(0, Ordering::Relaxed) / 5;
            let bout = p.bytes_out.swap(0, Ordering::Relaxed) / 5 / 1024;
            let thin = p.thinned.swap(0, Ordering::Relaxed) / 5;
            let bdrop = p.budget_drops.swap(0, Ordering::Relaxed) / 5;
            info!(target: "hsmp_server::perf",
                "perf ticks/5s={} tick_us p50={} p99={} max={} | pkt_handle_us p50={} p99={} | in={}pps out={}pps {}KB/s | thinned={}/s budget_drop={}/s",
                ticks, t50, t99, tmax, h50, h99, pin, pout, bout, thin, bdrop);
        }
    });
}
