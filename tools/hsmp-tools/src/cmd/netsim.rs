//! `hsmp-tools netsim`: UDP network-impairment proxy for testing HSMP under
//! internet conditions.
//!
//! Sits between a client (sidecar) and hsmp-server and applies, per direction
//! (up = client -> server, down = server -> client):
//!
//! * a bottleneck link: optional rate cap (kbit/s) with a bounded FIFO queue
//!   (tail drop when the queueing delay would exceed `queue_ms`: bufferbloat);
//! * one-way delay + AR(1)-correlated jitter (Gaussian, stationary stdev
//!   jitter/sqrt(3), as a uniform +/-jitter has, clipped to +/-jitter, correlation
//!   time `jitter_tau_ms`) + periodic latency spikes;
//! * FIFO delivery: a packet is never delivered before the one sent ahead of it
//!   (jitter alone does not reorder, as on a real path);
//! * explicit reordering: `reorder_pct` of packets are held back `reorder_ms`
//!   and let later packets overtake them;
//! * Gilbert-Elliott burst loss: the long-run mean is `loss` %, losses come in
//!   bursts of mean length `loss_burst` packets (1 = independent);
//! * duplication: the copy follows the original by 0.05-0.5 ms.
//!
//! Each client gets its own upstream socket, so the server sees one peer per
//! client; clients idle for 60 s are pruned.
//!
//!     hsmp-tools netsim --listen 127.0.0.1:7790 --upstream 127.0.0.1:7777 \
//!         --delay 60 --jitter 20 --loss 2 --dup 0.5
//!     hsmp-tools netsim --profile wifi --upstream 127.0.0.1:7777
//!     hsmp-tools netsim --self-test            # measure every profile
//!
//! Point the joiner at the proxy by launching its game with
//! HSMP_NETSIM_ADDR=127.0.0.1:7790 (HSMPMenu's JOIN honours it).
//! RTT ~= 2 * delay. Prints stats every 5 s.
//!
//! Timing: receive is tokio (IOCP, wakes on arrival); delivery is a dedicated
//! time-critical scheduler thread in a HIGH_PRIORITY_CLASS process that sleeps on
//! a condvar until ~2 ms before a packet's due time, then on a high-resolution
//! waitable timer (Windows 10 1803+; else a 1 ms-period sleep), then spins. The
//! proxy's own scheduling error (actual send minus due time, "lateness") is
//! measured for every packet and reported in each `netsim_stats` line
//! (late_n / late_p50_ms / late_p99_ms / late_max_ms over the window since the
//! previous line), so a starved proxy is visible to the gate.

use anyhow::{bail, Context, Result};
use rand::rngs::StdRng;
use rand::{Rng, SeedableRng};
use std::cmp::Reverse;
use std::collections::{BinaryHeap, HashMap};
use std::net::{SocketAddr, UdpSocket};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering::Relaxed};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

/// Scenario presets (docs/development/subsystems/replication.md). One-way per direction;
/// RTT ~= 2 * delay.
/// (name, delay ms, jitter ms, loss %, dup %, spike every s, spike ms)
/// crates/hsmp-combat-sim/src/sim/net.rs mirrors these seven columns.
pub const PROFILES: &[(&str, f64, f64, f64, f64, f64, f64)] = &[
    ("lan", 1.0, 0.5, 0.0, 0.0, 0.0, 0.0),
    ("good", 20.0, 4.0, 0.2, 0.1, 0.0, 0.0),
    ("typical", 50.0, 12.0, 1.0, 0.3, 0.0, 0.0),
    ("wifi", 35.0, 25.0, 2.0, 0.5, 7.0, 120.0),
    // A long, healthy internet route (~176 ms RTT, the first public-server join).
    ("intl", 88.0, 30.0, 0.3, 0.1, 0.0, 0.0),
    ("bad", 110.0, 40.0, 5.0, 1.0, 10.0, 200.0),
    // A far server (~300 ms RTT, 50 ms jitter, 2 % loss): the high-ping case of SMOOTH-1.
    ("far", 150.0, 50.0, 2.0, 0.3, 0.0, 0.0),
    ("awful", 180.0, 70.0, 10.0, 2.0, 5.0, 300.0),
];

/// The path model of each profile: (name, reorder %, reorder depth ms,
/// jitter correlation time ms, mean loss burst (packets), rate cap kbit/s per direction
/// (0 = none), max queueing delay ms (0 = unbounded)). Real paths reorder well under 1 %;
/// the rate caps are far above HSMP's traffic (~60 Hz, a few hundred kbit/s) except on
/// bad/awful, where a burst queues.
pub const PROFILE_PATH: &[(&str, f64, f64, f64, f64, f64, f64)] = &[
    ("lan", 0.0, 0.0, 20.0, 1.0, 0.0, 0.0),
    ("good", 0.0, 10.0, 30.0, 1.5, 50_000.0, 100.0),
    ("typical", 0.1, 25.0, 40.0, 2.0, 20_000.0, 150.0),
    ("wifi", 1.0, 30.0, 30.0, 3.0, 10_000.0, 200.0),
    ("intl", 0.1, 25.0, 40.0, 2.0, 20_000.0, 200.0),
    ("bad", 2.0, 40.0, 60.0, 4.0, 2_000.0, 300.0),
    ("far", 0.5, 30.0, 50.0, 2.0, 20_000.0, 300.0),
    ("awful", 3.0, 60.0, 80.0, 5.0, 1_000.0, 400.0),
];

/// A client that sent nothing (and got nothing) for this long is dropped.
const CLIENT_IDLE_MS: u64 = 60_000;
/// Direction index: client -> server.
pub const UP: usize = 0;
/// Direction index: server -> client.
pub const DOWN: usize = 1;
/// IPv4 + UDP header bytes, counted against the rate cap.
const UDP_OVERHEAD: usize = 28;

fn profile_names() -> Vec<&'static str> {
    let mut v: Vec<_> = PROFILES.iter().map(|p| p.0).collect();
    v.sort();
    v
}

#[derive(clap::Args, Clone)]
pub struct Args {
    #[arg(long, default_value = "127.0.0.1:7790")]
    listen: String,
    #[arg(long, default_value = "127.0.0.1:7777")]
    upstream: String,
    /// one-way base delay, ms
    #[arg(long, default_value_t = 60.0)]
    delay: f64,
    /// jitter bound, ms: AR(1) Gaussian with stdev jitter/sqrt(3), clipped to +/- jitter
    #[arg(long, default_value_t = 20.0)]
    jitter: f64,
    /// long-run drop probability, percent (both directions unless --loss-up/--loss-down)
    #[arg(long, default_value_t = 2.0)]
    loss: f64,
    /// duplicate probability, percent
    #[arg(long, default_value_t = 0.5)]
    dup: f64,
    #[arg(long)]
    seed: Option<u64>,
    /// preset delay/jitter/loss/dup/spikes and path model (overrides those flags): lan|good|typical|wifi|bad|awful
    #[arg(long, value_parser = clap::builder::PossibleValuesParser::new(profile_names()))]
    profile: Option<String>,
    /// every N s, add a latency spike (Wi-Fi scan / bufferbloat); 0 = off
    #[arg(long = "spike-every", default_value_t = 0.0)]
    spike_every: f64,
    /// extra delay during a spike, ms
    #[arg(long = "spike-ms", default_value_t = 0.0)]
    spike_ms: f64,
    /// spike duration, ms
    #[arg(long = "spike-len", default_value_t = 300.0)]
    spike_len: f64,
    /// explicit reordering: percent of packets held back --reorder-ms
    #[arg(long, default_value_t = 0.0)]
    reorder: f64,
    /// how long a reordered packet is held back, ms
    #[arg(long = "reorder-ms", default_value_t = 20.0)]
    reorder_ms: f64,
    /// jitter correlation time, ms (0 = independent per packet)
    #[arg(long = "jitter-tau-ms", default_value_t = 30.0)]
    jitter_tau_ms: f64,
    /// mean loss burst length, packets (Gilbert-Elliott; 1 = independent losses)
    #[arg(long = "loss-burst", default_value_t = 1.0)]
    loss_burst: f64,
    /// client -> server loss %, overrides --loss / the profile for that direction
    #[arg(long = "loss-up")]
    loss_up: Option<f64>,
    /// server -> client loss %, overrides --loss / the profile for that direction
    #[arg(long = "loss-down")]
    loss_down: Option<f64>,
    /// rate cap per direction, kbit/s (0 = none)
    #[arg(long = "rate-kbps", default_value_t = 0.0)]
    rate_kbps: f64,
    /// client -> server rate cap, kbit/s (overrides --rate-kbps / the profile)
    #[arg(long = "rate-up-kbps")]
    rate_up_kbps: Option<f64>,
    /// server -> client rate cap, kbit/s (overrides --rate-kbps / the profile)
    #[arg(long = "rate-down-kbps")]
    rate_down_kbps: Option<f64>,
    /// max queueing delay behind the rate cap before tail drop, ms (0 = unbounded)
    #[arg(long = "queue-ms", default_value_t = 0.0)]
    queue_ms: f64,
    /// Measure the proxy in-process: delay/jitter model, loss/burst/dup/reorder rates and
    /// scheduler lateness, both directions. No --profile = all profiles.
    #[arg(long = "self-test")]
    self_test: bool,
    /// self-test: packets per profile and direction leg (sent 1 per ms; the down leg sends half)
    #[arg(long = "packets", default_value_t = 3000)]
    packets: usize,
    /// self-test against an EXTERNAL proxy listening here (its upstream must be --sink)
    #[arg(long)]
    external: Option<String>,
    /// self-test sink address for --external
    #[arg(long, default_value = "127.0.0.1:7799")]
    sink: String,
    /// Poll this file every 100 ms: "blackout" drops every datagram both ways,
    /// "normal" / empty restores the starting impairment, a profile name switches live.
    #[arg(long = "control-file")]
    control_file: Option<std::path::PathBuf>,
    /// Exit when this process is gone (no orphan proxies).
    #[arg(long = "parent-pid")]
    parent_pid: Option<u32>,
    /// Append JSONL events: netsim_start / netsim_mode / netsim_stats / netsim_exit.
    #[arg(long)]
    events: Option<std::path::PathBuf>,
}

/// Impairment parameters.
#[derive(Clone, Copy, Debug)]
pub struct Impair {
    pub delay: f64,
    pub jitter: f64,
    /// long-run mean loss %, the mean of the two directions
    pub loss: f64,
    pub dup: f64,
    pub spike_every: f64,
    pub spike_ms: f64,
    pub spike_len: f64,
    pub reorder: f64,
    pub reorder_ms: f64,
    pub jitter_tau_ms: f64,
    pub loss_burst: f64,
    pub loss_up: f64,
    pub loss_down: f64,
    pub rate_up_kbps: f64,
    pub rate_down_kbps: f64,
    pub queue_ms: f64,
}

impl Impair {
    fn from_args(a: &Args) -> Impair {
        let mut i = Impair {
            delay: a.delay,
            jitter: a.jitter,
            loss: a.loss,
            dup: a.dup,
            spike_every: a.spike_every,
            spike_ms: a.spike_ms,
            spike_len: a.spike_len,
            reorder: a.reorder,
            reorder_ms: a.reorder_ms,
            jitter_tau_ms: a.jitter_tau_ms,
            loss_burst: a.loss_burst,
            loss_up: a.loss,
            loss_down: a.loss,
            rate_up_kbps: a.rate_kbps,
            rate_down_kbps: a.rate_kbps,
            queue_ms: a.queue_ms,
        };
        if let Some(p) = &a.profile {
            i.apply_profile(p);
        }
        if let Some(v) = a.loss_up {
            i.loss_up = v;
        }
        if let Some(v) = a.loss_down {
            i.loss_down = v;
        }
        i.loss = (i.loss_up + i.loss_down) / 2.0;
        if let Some(v) = a.rate_up_kbps {
            i.rate_up_kbps = v;
        }
        if let Some(v) = a.rate_down_kbps {
            i.rate_down_kbps = v;
        }
        i
    }

    /// A profile's full parameter set (spike_len 300 ms).
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn profile(name: &str) -> Option<Impair> {
        let mut i = Impair {
            delay: 0.0, jitter: 0.0, loss: 0.0, dup: 0.0, spike_every: 0.0, spike_ms: 0.0, spike_len: 300.0,
            reorder: 0.0, reorder_ms: 0.0, jitter_tau_ms: 0.0, loss_burst: 1.0, loss_up: 0.0, loss_down: 0.0,
            rate_up_kbps: 0.0, rate_down_kbps: 0.0, queue_ms: 0.0,
        };
        PROFILES.iter().any(|p| p.0 == name).then(|| {
            i.apply_profile(name);
            i
        })
    }

    fn apply_profile(&mut self, name: &str) {
        let p = PROFILES.iter().find(|p| p.0 == name).expect("profile");
        self.delay = p.1;
        self.jitter = p.2;
        self.loss = p.3;
        self.loss_up = p.3;
        self.loss_down = p.3;
        self.dup = p.4;
        self.spike_every = p.5;
        self.spike_ms = p.6;
        let q = PROFILE_PATH.iter().find(|q| q.0 == name).expect("profile path model");
        self.reorder = q.1;
        self.reorder_ms = q.2;
        self.jitter_tau_ms = q.3;
        self.loss_burst = q.4;
        self.rate_up_kbps = q.5;
        self.rate_down_kbps = q.5;
        self.queue_ms = q.6;
    }

    /// Spike extra delay (ms) at `t` seconds since proxy start.
    fn spike_at(&self, t: f64) -> f64 {
        if self.spike_every > 0.0 && (t % self.spike_every) * 1000.0 < self.spike_len {
            self.spike_ms
        } else {
            0.0
        }
    }

    fn loss_dir(&self, dir: usize) -> f64 {
        if dir == UP { self.loss_up } else { self.loss_down }
    }

    fn rate_dir(&self, dir: usize) -> f64 {
        if dir == UP { self.rate_up_kbps } else { self.rate_down_kbps }
    }
}

// ---- the path model (pure functions, unit-tested) -------------------------------------------

/// One AR(1) step of a stationary standard-normal process: correlation exp(-dt/tau) to the
/// previous value. tau <= 0: independent samples.
pub fn ar1_next(prev: f64, dt_ms: f64, tau_ms: f64, z: f64) -> f64 {
    if tau_ms <= 0.0 {
        return z;
    }
    let rho = (-dt_ms.max(0.0) / tau_ms).exp();
    rho * prev + (1.0 - rho * rho).sqrt() * z
}

/// Jitter in ms from the AR(1) state: stdev jitter/sqrt(3) (that of a uniform +/-jitter),
/// clipped to +/- jitter.
pub fn jitter_ms(state: f64, jitter: f64) -> f64 {
    if jitter <= 0.0 {
        return 0.0;
    }
    (state * jitter / 3f64.sqrt()).clamp(-jitter, jitter)
}

/// A standard-normal sample (Box-Muller).
fn gauss(rng: &mut StdRng) -> f64 {
    let u1: f64 = rng.gen::<f64>().max(1e-300);
    let u2: f64 = rng.gen();
    (-2.0 * u1.ln()).sqrt() * (2.0 * std::f64::consts::PI * u2).cos()
}

/// One Gilbert-Elliott step: returns the new state (true = Bad = this packet is lost).
/// `p` is the long-run loss probability (0..1), `burst` the mean Bad run length in packets:
/// P(Bad->Good) = 1/burst, P(Good->Bad) = p/(burst (1-p)), so the stationary Bad share is p.
/// burst <= 1: independent (Bernoulli) losses.
pub fn ge_step(bad: bool, p: f64, burst: f64, u: f64) -> bool {
    if p <= 0.0 {
        return false;
    }
    if p >= 1.0 {
        return true;
    }
    if burst <= 1.0 {
        return u < p;
    }
    let r = 1.0 / burst;
    let g2b = (p * r / (1.0 - p)).min(1.0);
    if bad { u >= r } else { u < g2b }
}

/// The bottleneck link: a packet of `bytes` arriving at `now` starts transmitting when the
/// link is free (`free_at`) and occupies it for its serialization time. Returns the time its
/// transmission ends (the new `free_at`), or None when its queueing delay would exceed
/// `queue_ms` (tail drop; 0 = unbounded). kbps <= 0: no cap (returns `now`).
pub fn rate_slot(now: Instant, free_at: Option<Instant>, bytes: usize, kbps: f64, queue_ms: f64) -> Option<Instant> {
    if kbps <= 0.0 {
        return Some(now);
    }
    let start = free_at.filter(|f| *f > now).unwrap_or(now);
    let wait_ms = (start - now).as_secs_f64() * 1000.0;
    if queue_ms > 0.0 && wait_ms > queue_ms {
        return None;
    }
    let ser = Duration::from_secs_f64(bytes as f64 * 8.0 / (kbps * 1000.0));
    Some(start + ser)
}

/// Percentile (nearest rank) of unsorted samples; NaN when empty.
pub fn pct(v: &[f64], p: f64) -> f64 {
    if v.is_empty() {
        return f64::NAN;
    }
    let mut s = v.to_vec();
    s.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let k = ((p / 100.0 * s.len() as f64).ceil() as usize).clamp(1, s.len()) - 1;
    s[k]
}

/// {n, p50, p99, max} of lateness samples (ms); the percentiles are null without samples.
pub fn late_summary(v: &[f64]) -> (usize, Option<f64>, Option<f64>, Option<f64>) {
    if v.is_empty() {
        return (0, None, None, None);
    }
    let r = |x: f64| (x * 1000.0).round() / 1000.0;
    (v.len(), Some(r(pct(v, 50.0))), Some(r(pct(v, 99.0))), Some(r(pct(v, 100.0))))
}

// ---- Windows timer resolution / priority -----------------------------------------------------

fn hires_timers() {
    #[cfg(windows)]
    unsafe {
        // Without this, Windows quantises sleeps/waits to the 15.6 ms tick.
        windows_sys::Win32::Media::timeBeginPeriod(1);
    }
}

/// HIGH_PRIORITY_CLASS for the whole proxy (a starved delivery thread added up to ~48 ms
/// of unmodelled delay while two games and a build ran).
fn boost_process() {
    #[cfg(windows)]
    unsafe {
        use windows_sys::Win32::System::Threading::{GetCurrentProcess, SetPriorityClass, HIGH_PRIORITY_CLASS};
        SetPriorityClass(GetCurrentProcess(), HIGH_PRIORITY_CLASS);
    }
}

fn boost_thread() {
    #[cfg(windows)]
    unsafe {
        use windows_sys::Win32::System::Threading::{GetCurrentThread, SetThreadPriority, THREAD_PRIORITY_TIME_CRITICAL};
        SetThreadPriority(GetCurrentThread(), THREAD_PRIORITY_TIME_CRITICAL);
    }
}

#[cfg(windows)]
thread_local! {
    /// This thread's high-resolution waitable timer (0 = not available: Windows < 10 1803).
    static HIRES_TIMER: isize = unsafe {
        use windows_sys::Win32::System::Threading::{CreateWaitableTimerExW, CREATE_WAITABLE_TIMER_HIGH_RESOLUTION, TIMER_ALL_ACCESS};
        CreateWaitableTimerExW(std::ptr::null(), std::ptr::null(), CREATE_WAITABLE_TIMER_HIGH_RESOLUTION, TIMER_ALL_ACCESS)
    };
}

/// Sleep `d` with sub-millisecond precision where the OS allows it: a high-resolution
/// waitable timer on Windows, else the OS sleep.
pub fn precise_sleep(d: Duration) {
    #[cfg(windows)]
    {
        let h = HIRES_TIMER.with(|h| *h);
        if h != 0 {
            unsafe {
                use windows_sys::Win32::System::Threading::{SetWaitableTimer, WaitForSingleObject};
                // relative due time in 100 ns units (negative = relative)
                let due: i64 = -((d.as_nanos() / 100).max(1) as i64);
                if SetWaitableTimer(h, &due, 0, None, std::ptr::null(), 0) != 0 {
                    WaitForSingleObject(h, 1000);
                    return;
                }
            }
        }
    }
    std::thread::sleep(d);
}

/// Sleep until `t` precisely: OS sleep until ~1.5 ms before, a precise wait, then spin.
pub fn sleep_until(t: Instant) {
    loop {
        let now = Instant::now();
        if now >= t {
            return;
        }
        let left = t - now;
        if left > Duration::from_micros(2000) {
            std::thread::sleep(left - Duration::from_micros(1500));
        } else if left > Duration::from_micros(300) {
            precise_sleep(left - Duration::from_micros(200));
        } else {
            std::hint::spin_loop();
        }
    }
}

// ---- delivery scheduler --------------------------------------------------------------------

struct Pending {
    sock: Arc<UdpSocket>,
    dest: SocketAddr,
    data: Vec<u8>,
    dir: usize,
    due: Instant,
}

#[derive(Default)]
struct Queue {
    heap: BinaryHeap<Reverse<(Instant, u64)>>,
    items: HashMap<u64, Pending>,
    seq: u64,
}

/// Lateness samples (actual send - due, ms) for the self-test: (dir, ms).
type LateAll = Arc<Mutex<Vec<(usize, f64)>>>;

/// Packets waiting for their delivery time; drained by one scheduler thread. Ties (equal due
/// times, e.g. FIFO-held packets) go out in push order.
pub struct Scheduler {
    q: Mutex<Queue>,
    cv: Condvar,
    stop: AtomicBool,
    out: AtomicU64,
    /// lateness per direction since the last `take_late`
    late: Mutex<[Vec<f64>; 2]>,
    late_all: Option<LateAll>,
}

impl Scheduler {
    fn new(late_all: Option<LateAll>) -> Arc<Scheduler> {
        let s = Arc::new(Scheduler {
            q: Mutex::new(Queue::default()),
            cv: Condvar::new(),
            stop: AtomicBool::new(false),
            out: AtomicU64::new(0),
            late: Mutex::new([vec![], vec![]]),
            late_all,
        });
        let s2 = s.clone();
        std::thread::Builder::new().name("netsim-deliver".into()).spawn(move || s2.run()).unwrap();
        s
    }

    fn push(&self, p: Pending) {
        let mut q = self.q.lock().unwrap();
        q.seq += 1;
        let id = q.seq;
        q.heap.push(Reverse((p.due, id)));
        q.items.insert(id, p);
        drop(q);
        self.cv.notify_one();
    }

    fn queued(&self) -> usize {
        self.q.lock().unwrap().items.len()
    }

    /// The lateness samples since the previous call, per direction.
    fn take_late(&self) -> [Vec<f64>; 2] {
        std::mem::take(&mut *self.late.lock().unwrap())
    }

    fn run(&self) {
        boost_thread();
        let mut due: Vec<Pending> = vec![];
        let mut lat: Vec<(usize, f64)> = vec![];
        while !self.stop.load(Relaxed) {
            let next = {
                let mut q = self.q.lock().unwrap();
                let now = Instant::now();
                while let Some(Reverse((t, id))) = q.heap.peek().copied() {
                    if t > now {
                        break;
                    }
                    q.heap.pop();
                    if let Some(p) = q.items.remove(&id) {
                        due.push(p);
                    }
                }
                match q.heap.peek() {
                    Some(Reverse((t, _))) => Some(*t),
                    None if due.is_empty() => {
                        let _ = self.cv.wait_timeout(q, Duration::from_millis(50)).unwrap();
                        continue;
                    }
                    None => None,
                }
            };
            for p in due.drain(..) {
                let late = Instant::now().saturating_duration_since(p.due).as_secs_f64() * 1000.0;
                if p.sock.send_to(&p.data, p.dest).is_ok() {
                    self.out.fetch_add(1, Relaxed);
                }
                lat.push((p.dir, late));
            }
            if !lat.is_empty() {
                let mut l = self.late.lock().unwrap();
                for (d, ms) in &lat {
                    l[*d].push(*ms);
                }
                drop(l);
                if let Some(all) = &self.late_all {
                    all.lock().unwrap().extend(lat.iter().copied());
                }
                lat.clear();
            }
            if let Some(t) = next {
                let left = t.saturating_duration_since(Instant::now());
                if left > Duration::from_micros(2500) {
                    // Wake early (an earlier packet may arrive meanwhile: the condvar wakes us).
                    let q = self.q.lock().unwrap();
                    // A packet pushed since we peeked (its notify may predate this wait).
                    if q.heap.peek().map(|r| r.0 .0 < t).unwrap_or(false) {
                        continue;
                    }
                    let _ = self.cv.wait_timeout(q, left - Duration::from_micros(2000)).unwrap();
                } else if left > Duration::from_micros(300) {
                    // ~2 ms out: a high-resolution timer wait (a packet pushed meanwhile is due
                    // >= delay later, so it cannot be missed by sleeping this little)
                    precise_sleep(left - Duration::from_micros(200));
                } else {
                    std::hint::spin_loop();
                }
            }
        }
    }
}

// ---- the proxy -------------------------------------------------------------------------------

/// Per-copy trace for the self-test.
#[derive(Clone, Copy, Debug)]
struct TraceRec {
    id: u64,
    dir: usize,
    /// intended delivery
    at: Instant,
    spike: f64,
    reordered: bool,
    /// the model's jitter for this packet (ms, before FIFO)
    jit: f64,
    /// the duplicate copy
    copy: bool,
    /// delivery moved later by the FIFO line
    held: bool,
}
type Trace = Arc<Mutex<Vec<TraceRec>>>;

#[derive(Default)]
struct Stats {
    inp: [AtomicU64; 2],
    lost: [AtomicU64; 2],
    qdrop: [AtomicU64; 2],
    reordered: [AtomicU64; 2],
    dup: AtomicU64,
    blackout: AtomicU64,
}

/// Per-direction path state.
#[derive(Default, Clone, Copy)]
struct DirState {
    ge_bad: bool,
    /// AR(1) jitter state (standard normal units)
    j: f64,
    last_rx: Option<Instant>,
    /// FIFO: the latest scheduled in-order delivery
    last_at: Option<Instant>,
    /// bottleneck link busy until
    free_at: Option<Instant>,
}

struct Core {
    imp: Mutex<Impair>,
    blackout: AtomicBool,
    rng: Mutex<StdRng>,
    start: Instant,
    sched: Arc<Scheduler>,
    stats: Stats,
    dirs: Mutex<[DirState; 2]>,
    trace: Option<Trace>,
}

impl Core {
    /// Apply blackout / loss / rate / delay / reorder / dup to one datagram travelling `dir`.
    fn schedule(&self, dir: usize, sock: &Arc<UdpSocket>, data: &[u8], dest: SocketAddr) {
        let now = Instant::now();
        self.stats.inp[dir].fetch_add(1, Relaxed);
        if self.blackout.load(Relaxed) {
            self.stats.blackout.fetch_add(1, Relaxed);
            return;
        }
        let imp = *self.imp.lock().unwrap();
        let mut rng = self.rng.lock().unwrap();
        let mut dirs = self.dirs.lock().unwrap();
        let st = &mut dirs[dir];
        // burst loss
        st.ge_bad = ge_step(st.ge_bad, imp.loss_dir(dir) / 100.0, imp.loss_burst, rng.gen());
        if st.ge_bad {
            self.stats.lost[dir].fetch_add(1, Relaxed);
            return;
        }
        // bottleneck link + bounded queue
        let ready = match rate_slot(now, st.free_at, data.len() + UDP_OVERHEAD, imp.rate_dir(dir), imp.queue_ms) {
            Some(t) => t,
            None => {
                self.stats.qdrop[dir].fetch_add(1, Relaxed);
                return;
            }
        };
        if imp.rate_dir(dir) > 0.0 {
            st.free_at = Some(ready);
        }
        // correlated jitter
        let dt = st.last_rx.map(|t| (now - t).as_secs_f64() * 1000.0).unwrap_or(1e9);
        st.last_rx = Some(now);
        st.j = ar1_next(st.j, dt, imp.jitter_tau_ms, gauss(&mut rng));
        let jit = jitter_ms(st.j, imp.jitter);
        let spike = imp.spike_at((now - self.start).as_secs_f64());
        let d = (imp.delay + spike + jit).max(0.0);
        let mut at = ready + Duration::from_secs_f64(d / 1000.0);
        let reordered = imp.reorder > 0.0 && rng.gen::<f64>() * 100.0 < imp.reorder;
        // FIFO behind the previous packet, which a rate-capped link releases one serialization
        // time apart (a spike or a jitter drop then drains as a queue, not as one burst)
        let gap = if imp.rate_dir(dir) > 0.0 {
            Duration::from_secs_f64((data.len() + UDP_OVERHEAD) as f64 * 8.0 / (imp.rate_dir(dir) * 1000.0))
        } else {
            Duration::ZERO
        };
        let mut held = false;
        if let Some(l) = st.last_at {
            if at < l + gap {
                at = l + gap;
                held = true;
            }
        }
        if reordered {
            // held back behind the FIFO line: later packets overtake it (the line does not move)
            at += Duration::from_secs_f64(imp.reorder_ms.max(0.0) / 1000.0);
            self.stats.reordered[dir].fetch_add(1, Relaxed);
        } else {
            st.last_at = Some(at);
        }
        let mut copies = vec![(at, false)];
        if rng.gen::<f64>() * 100.0 < imp.dup {
            self.stats.dup.fetch_add(1, Relaxed);
            let c = at + Duration::from_secs_f64(rng.gen_range(0.05..0.5) / 1000.0);
            if !reordered {
                st.last_at = Some(c);
            }
            copies.push((c, true));
        }
        drop(dirs);
        drop(rng);
        for (at, copy) in copies {
            if let Some(tr) = &self.trace {
                if data.len() >= 8 {
                    let id = u64::from_le_bytes(data[..8].try_into().unwrap());
                    tr.lock().unwrap().push(TraceRec { id, dir, at, spike, reordered, jit, copy, held });
                }
            }
            self.sched.push(Pending { sock: sock.clone(), dest, data: data.to_vec(), dir, due: at });
        }
    }

    /// netsim_stats / netsim_exit body. Counters are cumulative; the late_* fields cover the
    /// window since the previous call.
    fn stats_json(&self, clients: usize) -> serde_json::Value {
        let late = self.sched.take_late();
        let mut all = late[UP].clone();
        all.extend_from_slice(&late[DOWN]);
        let (n, p50, p99, max) = late_summary(&all);
        let ld = |d: usize| {
            let (n, _, p99, max) = late_summary(&late[d]);
            serde_json::json!({
                "late_n": n, "late_p99_ms": p99, "late_max_ms": max,
                "lost": self.stats.lost[d].load(Relaxed), "in": self.stats.inp[d].load(Relaxed),
                "queue_dropped": self.stats.qdrop[d].load(Relaxed), "reordered": self.stats.reordered[d].load(Relaxed),
            })
        };
        let s = |a: &[AtomicU64; 2]| a[UP].load(Relaxed) + a[DOWN].load(Relaxed);
        serde_json::json!({
            "in": s(&self.stats.inp),
            "out": self.sched.out.load(Relaxed),
            "lost": s(&self.stats.lost),
            "dup": self.stats.dup.load(Relaxed),
            "blackout_dropped": self.stats.blackout.load(Relaxed),
            "queued": self.sched.queued(),
            "clients": clients,
            "late_n": n, "late_p50_ms": p50, "late_p99_ms": p99, "late_max_ms": max,
            "lost_up": self.stats.lost[UP].load(Relaxed), "lost_down": self.stats.lost[DOWN].load(Relaxed),
            "queue_dropped": s(&self.stats.qdrop), "reordered": s(&self.stats.reordered),
            "up": ld(UP), "down": ld(DOWN),
        })
    }
}

fn parse_addr(s: &str) -> Result<SocketAddr> {
    use std::net::ToSocketAddrs;
    s.to_socket_addrs()?.next().with_context(|| format!("bad address {s}"))
}

fn udp_pair(bind: SocketAddr) -> Result<(Arc<UdpSocket>, tokio::net::UdpSocket)> {
    let s = UdpSocket::bind(bind).with_context(|| format!("bind {bind}"))?;
    s.set_nonblocking(true)?;
    let send = Arc::new(s.try_clone()?);
    let recv = tokio::net::UdpSocket::from_std(s)?;
    Ok((send, recv))
}

/// JSONL event sink (`--events`); every line has `ev`, `wall_ms`, `listen`.
struct Events {
    path: Option<std::path::PathBuf>,
    listen: String,
}

impl Events {
    fn emit(&self, ev: &str, extra: serde_json::Value) {
        let Some(p) = &self.path else { return };
        let wall_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);
        let mut o = serde_json::json!({ "ev": ev, "wall_ms": wall_ms, "listen": self.listen });
        if let (Some(m), serde_json::Value::Object(x)) = (o.as_object_mut(), extra) {
            m.extend(x);
        }
        use std::io::Write;
        if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(p) {
            let _ = writeln!(f, "{o}");
        }
    }
}

fn impair_json(i: &Impair) -> serde_json::Value {
    serde_json::json!({
        "delay": i.delay, "jitter": i.jitter, "loss": i.loss, "dup": i.dup,
        "spike_every": i.spike_every, "spike_ms": i.spike_ms, "spike_len": i.spike_len,
        "loss_up": i.loss_up, "loss_down": i.loss_down, "loss_burst": i.loss_burst,
        "reorder_pct": i.reorder, "reorder_ms": i.reorder_ms,
        "jitter_tau_ms": i.jitter_tau_ms, "jitter_model": "ar1_gauss_clipped",
        "rate_up_kbps": i.rate_up_kbps, "rate_down_kbps": i.rate_down_kbps, "queue_ms": i.queue_ms,
        "fifo": true,
    })
}

/// Is process `pid` still running?
fn pid_alive(pid: u32) -> bool {
    #[cfg(windows)]
    unsafe {
        use windows_sys::Win32::Foundation::{CloseHandle, STILL_ACTIVE};
        use windows_sys::Win32::System::Threading::{GetExitCodeProcess, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION};
        let h = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if h == 0 {
            return false;
        }
        let mut code: u32 = 0;
        let ok = GetExitCodeProcess(h, &mut code) != 0;
        CloseHandle(h);
        ok && code == STILL_ACTIVE as u32
    }
    #[cfg(not(windows))]
    {
        std::path::Path::new(&format!("/proc/{pid}")).exists()
    }
}

/// One proxied client: its own upstream socket, last activity (ms since proxy start, either
/// direction) and the stop flag of its server -> client task.
struct Client {
    up: Arc<UdpSocket>,
    last_ms: Arc<AtomicU64>,
    stop: Arc<AtomicBool>,
}

/// Drop clients idle for more than `idle_ms` (stopping their receive tasks). Returns how many.
fn prune_idle(clients: &mut HashMap<SocketAddr, Client>, now_ms: u64, idle_ms: u64) -> usize {
    let before = clients.len();
    clients.retain(|_, c| {
        let keep = now_ms.saturating_sub(c.last_ms.load(Relaxed)) <= idle_ms;
        if !keep {
            c.stop.store(true, Relaxed);
        }
        keep
    });
    before - clients.len()
}

/// Everything one proxy instance needs.
struct ProxyOpts {
    listen: SocketAddr,
    upstream: SocketAddr,
    imp: Impair,
    profile: Option<String>,
    seed: Option<u64>,
    trace: Option<Trace>,
    late_all: Option<LateAll>,
    stop: Arc<AtomicBool>,
    verbose: bool,
    control_file: Option<std::path::PathBuf>,
    parent_pid: Option<u32>,
    events: Option<std::path::PathBuf>,
}

/// Run the proxy until `stop` is set (or the parent dies / Ctrl-C). `ready` receives the bound address.
fn proxy(o: ProxyOpts, ready: std::sync::mpsc::Sender<SocketAddr>) -> Result<()> {
    let rt = tokio::runtime::Builder::new_current_thread().enable_all().build()?;
    rt.block_on(async move {
        let stop = o.stop.clone();
        let core = Arc::new(Core {
            imp: Mutex::new(o.imp),
            blackout: AtomicBool::new(false),
            rng: Mutex::new(match o.seed {
                Some(s) => StdRng::seed_from_u64(s),
                None => StdRng::from_entropy(),
            }),
            start: Instant::now(),
            sched: Scheduler::new(o.late_all.clone()),
            stats: Stats::default(),
            dirs: Mutex::new([DirState::default(); 2]),
            trace: o.trace,
        });
        let (front_tx, front_rx) = udp_pair(o.listen)?;
        let bound = front_rx.local_addr()?;
        let _ = ready.send(bound);
        let ev = Events { path: o.events.clone(), listen: bound.to_string() };
        let mut start = serde_json::json!({
            "upstream": o.upstream.to_string(), "profile": o.profile, "pid": std::process::id(),
        });
        if let (Some(m), serde_json::Value::Object(x)) = (start.as_object_mut(), impair_json(&o.imp)) {
            m.extend(x);
        }
        ev.emit("netsim_start", start);
        let up_bind: SocketAddr = if o.upstream.ip().is_loopback() {
            "127.0.0.1:0".parse().unwrap()
        } else if o.upstream.is_ipv6() {
            "[::]:0".parse().unwrap()
        } else {
            "0.0.0.0:0".parse().unwrap()
        };
        let elapsed_ms = |c: &Core| c.start.elapsed().as_millis() as u64;
        let mut clients: HashMap<SocketAddr, Client> = HashMap::new();
        let mut buf = vec![0u8; 65535];
        let mut next_stats = Instant::now() + Duration::from_secs(5);
        let mut next_ctl = Instant::now();
        let mut next_parent = Instant::now() + Duration::from_millis(500);
        let mut next_prune = Instant::now() + Duration::from_secs(1);
        let mut ctl_last: Option<String> = None;
        let mut exit_reason = "stopped";
        let ctrl_c = tokio::signal::ctrl_c();
        tokio::pin!(ctrl_c);
        while !stop.load(Relaxed) {
            let r = tokio::select! {
                r = tokio::time::timeout(Duration::from_millis(50), front_rx.recv_from(&mut buf)) => r,
                _ = &mut ctrl_c => { exit_reason = "ctrl_c"; break; }
            };
            if let Ok(Ok((n, src))) = r {
                let up = match clients.get(&src) {
                    Some(c) => {
                        c.last_ms.store(elapsed_ms(&core), Relaxed);
                        c.up.clone()
                    }
                    None => {
                        let (up_tx, up_rx) = udp_pair(up_bind)?;
                        if o.verbose {
                            println!("new client {src} via {}", up_rx.local_addr()?);
                        }
                        let last_ms = Arc::new(AtomicU64::new(elapsed_ms(&core)));
                        let cstop = Arc::new(AtomicBool::new(false));
                        clients.insert(src, Client { up: up_tx.clone(), last_ms: last_ms.clone(), stop: cstop.clone() });
                        // Server -> this client.
                        let core2 = core.clone();
                        let front2 = front_tx.clone();
                        let stop2 = stop.clone();
                        tokio::spawn(async move {
                            let mut b = vec![0u8; 65535];
                            while !stop2.load(Relaxed) && !cstop.load(Relaxed) {
                                match tokio::time::timeout(Duration::from_millis(200), up_rx.recv_from(&mut b)).await {
                                    Ok(Ok((n, _))) => {
                                        last_ms.store(core2.start.elapsed().as_millis() as u64, Relaxed);
                                        core2.schedule(DOWN, &front2, &b[..n], src)
                                    }
                                    Ok(Err(_)) => {} // e.g. WSAECONNRESET from an ICMP unreachable
                                    Err(_) => {}
                                }
                            }
                        });
                        up_tx
                    }
                };
                core.schedule(UP, &up, &buf[..n], o.upstream);
            }
            let now = Instant::now();
            if now >= next_prune {
                next_prune = now + Duration::from_secs(1);
                let k = prune_idle(&mut clients, elapsed_ms(&core), CLIENT_IDLE_MS);
                if k > 0 && o.verbose {
                    println!("[netsim] pruned {k} idle client(s)");
                }
            }
            // --control-file: blackout / normal / <profile>, polled every 100 ms.
            if let Some(cf) = &o.control_file {
                if now >= next_ctl {
                    next_ctl = now + Duration::from_millis(100);
                    let cur = std::fs::read_to_string(cf).map(|s| s.trim().to_ascii_lowercase()).unwrap_or_default();
                    if ctl_last.as_deref() != Some(cur.as_str()) {
                        let mode = match cur.as_str() {
                            "blackout" => {
                                core.blackout.store(true, Relaxed);
                                Some("blackout".to_string())
                            }
                            "" | "normal" => {
                                *core.imp.lock().unwrap() = o.imp;
                                core.blackout.store(false, Relaxed);
                                Some("normal".to_string())
                            }
                            p if PROFILES.iter().any(|x| x.0 == p) => {
                                core.imp.lock().unwrap().apply_profile(p);
                                core.blackout.store(false, Relaxed);
                                Some(p.to_string())
                            }
                            other => {
                                eprintln!("netsim: unknown control word {other:?} (blackout|normal|<profile>)");
                                None
                            }
                        };
                        // The first read of an absent/empty file is the starting state: no event.
                        if let Some(m) = mode {
                            if ctl_last.is_some() || m != "normal" {
                                let imp = *core.imp.lock().unwrap();
                                if o.verbose {
                                    println!("[netsim] mode {m}");
                                }
                                let mut e = serde_json::json!({ "mode": m });
                                if let (Some(x), serde_json::Value::Object(y)) = (e.as_object_mut(), impair_json(&imp)) {
                                    x.extend(y);
                                }
                                ev.emit("netsim_mode", e);
                            }
                        }
                        ctl_last = Some(cur);
                    }
                }
            }
            // --parent-pid: no orphan proxies.
            if let Some(pp) = o.parent_pid {
                if now >= next_parent {
                    next_parent = now + Duration::from_millis(500);
                    if !pid_alive(pp) {
                        exit_reason = "parent_exited";
                        if o.verbose {
                            println!("[netsim] parent {pp} gone, exiting");
                        }
                        break;
                    }
                }
            }
            if now >= next_stats {
                next_stats = now + Duration::from_secs(5);
                let st = core.stats_json(clients.len());
                if o.verbose {
                    println!(
                        "[netsim] in={} out={} lost={} dup={} reordered={} qdrop={} queued={} clients={} late p50/p99/max={}/{}/{} ms{}",
                        st["in"], st["out"], st["lost"], st["dup"], st["reordered"], st["queue_dropped"], st["queued"], st["clients"],
                        st["late_p50_ms"], st["late_p99_ms"], st["late_max_ms"],
                        if core.blackout.load(Relaxed) { " BLACKOUT" } else { "" }
                    );
                }
                ev.emit("netsim_stats", st);
            }
        }
        stop.store(true, Relaxed);
        core.sched.stop.store(true, Relaxed);
        let mut e = core.stats_json(clients.len());
        e["reason"] = serde_json::Value::from(exit_reason);
        ev.emit("netsim_exit", e);
        Ok::<(), anyhow::Error>(())
    })
}

pub fn run(a: Args) -> Result<i32> {
    hires_timers();
    boost_process();
    if a.self_test {
        return self_test(&a);
    }
    let imp = Impair::from_args(&a);
    let listen = parse_addr(&a.listen)?;
    let upstream = parse_addr(&a.upstream)?;
    println!(
        "netsim {} -> {}  profile={} delay={}ms jitter=±{}ms (tau {}ms) loss={}% (up {} down {}, burst {}) dup={}% reorder={}%/{}ms rate={}/{}kbps queue={}ms spikes={}ms/{}s (RTT ~{:.0}ms)",
        a.listen,
        a.upstream,
        a.profile.as_deref().unwrap_or("-"),
        imp.delay,
        imp.jitter,
        imp.jitter_tau_ms,
        imp.loss,
        imp.loss_up,
        imp.loss_down,
        imp.loss_burst,
        imp.dup,
        imp.reorder,
        imp.reorder_ms,
        imp.rate_up_kbps,
        imp.rate_down_kbps,
        imp.queue_ms,
        imp.spike_ms,
        imp.spike_every,
        2.0 * imp.delay
    );
    let (tx, _rx) = std::sync::mpsc::channel();
    proxy(
        ProxyOpts {
            listen,
            upstream,
            imp,
            profile: a.profile.clone(),
            seed: a.seed,
            trace: None,
            late_all: None,
            stop: Arc::new(AtomicBool::new(false)),
            verbose: true,
            control_file: a.control_file.clone(),
            parent_pid: a.parent_pid,
            events: a.events.clone(),
        },
        tx,
    )?;
    Ok(0)
}

// ---- self-test ---------------------------------------------------------------------------------

fn mean(v: &[f64]) -> f64 {
    v.iter().sum::<f64>() / v.len().max(1) as f64
}

fn stdev(v: &[f64]) -> f64 {
    let m = mean(v);
    (v.iter().map(|x| (x - m) * (x - m)).sum::<f64>() / (v.len().max(2) - 1) as f64).sqrt()
}

/// Lag-1 autocorrelation.
fn autocorr1(v: &[f64]) -> f64 {
    if v.len() < 3 {
        return f64::NAN;
    }
    let m = mean(v);
    let den: f64 = v.iter().map(|x| (x - m) * (x - m)).sum();
    let num: f64 = v.windows(2).map(|w| (w[0] - m) * (w[1] - m)).sum();
    if den > 0.0 { num / den } else { f64::NAN }
}

/// Lengths of the runs of consecutive ids missing from `got` in 0..n.
fn loss_runs(n: u64, got: &std::collections::HashSet<u64>) -> Vec<usize> {
    let mut runs = vec![];
    let mut cur = 0;
    for id in 0..n {
        if got.contains(&id) {
            if cur > 0 {
                runs.push(cur);
            }
            cur = 0;
        } else {
            cur += 1;
        }
    }
    if cur > 0 {
        runs.push(cur);
    }
    runs
}

/// Arrivals (in arrival order) whose id is below an id that arrived before them: one per
/// overtaken packet. Only first copies count.
fn reorder_count(arrivals_in_order: &[u64]) -> usize {
    let mut seen = std::collections::HashSet::new();
    let mut max: Option<u64> = None;
    let mut k = 0;
    for id in arrivals_in_order {
        if !seen.insert(*id) {
            continue;
        }
        if max.map(|m| *id < m).unwrap_or(false) {
            k += 1;
        }
        max = Some(max.map_or(*id, |m| m.max(*id)));
    }
    k
}

struct Verdict {
    lines: Vec<String>,
    ok: bool,
}

impl Verdict {
    fn check(&mut self, ok: bool, what: String) {
        self.lines.push(format!("    {} {what}", if ok { "ok  " } else { "FAIL" }));
        self.ok &= ok;
    }
    fn note(&mut self, what: String) {
        self.lines.push(format!("    note {what}"));
    }
}

/// Stdev of a standard normal clipped at +/- sqrt(3), relative to the unclipped one.
const CLIPPED_SD: f64 = 0.926;

/// Judge one direction leg: `n` packets with local ids 0..n sent 1/ms at `sent`.
#[allow(clippy::too_many_arguments)]
fn analyze_leg(v: &mut Verdict, leg: &str, dir: usize, imp: &Impair, n: usize, sent: &[Instant], arr: &[(u64, Instant)], tr: &[TraceRec], late: &[f64], internal: bool) {
    let nf = n as f64;
    let mut per_id: HashMap<u64, usize> = HashMap::new();
    for (id, _) in arr {
        *per_id.entry(*id).or_default() += 1;
    }
    let uniq = per_id.len();
    let copies = arr.len();
    let lost = 1.0 - uniq as f64 / nf;
    let p = imp.loss_dir(dir) / 100.0;
    let b = imp.loss_burst.max(1.0);
    // Gilbert-Elliott: the loss count's variance is inflated by about (2b - 1)
    let tol = 4.0 * (p * (1.0 - p) / nf * (2.0 * b - 1.0)).sqrt() + 0.003;
    v.check((lost - p).abs() <= tol, format!("{leg} loss {:.2}% (want {:.2}% ± {:.2})", lost * 100.0, p * 100.0, tol * 100.0));
    let got: std::collections::HashSet<u64> = per_id.keys().copied().collect();
    let runs = loss_runs(n as u64, &got);
    let mburst = runs.iter().sum::<usize>() as f64 / runs.len().max(1) as f64;
    if runs.len() >= 8 {
        let btol = 4.0 * (b * (b - 1.0) / runs.len() as f64).sqrt() + 0.35 * b.max(1.0) * 0.5 + 0.2;
        v.check((mburst - b).abs() <= btol, format!("{leg} loss bursts: {} runs, mean {mburst:.2} packets (want {b:.2} ± {btol:.2})", runs.len()));
    } else {
        v.note(format!("{leg} loss bursts: {} runs (mean {mburst:.2}): too few to judge (want {b:.2})", runs.len()));
    }
    let q = imp.dup / 100.0;
    let dup = if uniq > 0 { (copies - uniq) as f64 / uniq as f64 } else { 0.0 };
    let tol = 4.0 * (q * (1.0 - q) / nf).sqrt() + 0.002;
    v.check((dup - q).abs() <= tol, format!("{leg} dup {:.2}% (want {:.2}% ± {:.2})", dup * 100.0, imp.dup, tol * 100.0));
    // reorder: explicit only (FIFO otherwise)
    let order: Vec<u64> = arr.iter().map(|a| a.0).collect();
    let ro = reorder_count(&order);
    let rr = ro as f64 / uniq.max(1) as f64;
    let qr = imp.reorder / 100.0;
    if qr <= 0.0 {
        v.check(ro == 0, format!("{leg} reorder {ro} packet(s) (want 0: FIFO path)"));
    } else {
        let tol = 4.0 * (qr * (1.0 - qr) / uniq.max(1) as f64).sqrt() + 0.002;
        v.check((rr - qr).abs() <= tol, format!("{leg} reorder {:.2}% ({ro}) (want {:.2}% ± {:.2})", rr * 100.0, imp.reorder, tol * 100.0));
    }
    // delay of first copies the model did not spike, reorder or FIFO-hold: delay + jitter only
    // (spike, excluded, intended delivery) per id
    let mut spike_of: HashMap<u64, (f64, bool, Instant)> = HashMap::new();
    for t in tr.iter().filter(|t| !t.copy) {
        spike_of.insert(t.id, (t.spike, t.reordered || t.held, t.at));
    }
    let mut first: std::collections::HashSet<u64> = std::collections::HashSet::new();
    let mut base = vec![];
    let mut added = vec![];
    for (id, t) in arr {
        if !first.insert(*id) {
            continue;
        }
        let d = (*t - sent[*id as usize]).as_secs_f64() * 1000.0;
        added.push(d);
        match spike_of.get(id).copied() {
            // the model's own delay (send -> intended delivery): the proxy's timing is judged
            // separately (lateness), so a preempted send does not fail the model
            Some((sp, ex, at)) if sp == 0.0 && !ex => base.push((at - sent[*id as usize]).as_secs_f64() * 1000.0),
            Some(_) => {}
            None => base.push(d), // --external: no trace
        }
    }
    let lo = (imp.delay - imp.jitter).max(0.0);
    let hi = imp.delay + imp.jitter;
    let slack = 1.0; // ms: client send -> proxy receive (+ the proxy's timing, external)
    let bmin = base.iter().cloned().fold(f64::INFINITY, f64::min);
    let bmax = base.iter().cloned().fold(f64::NEG_INFINITY, f64::max);
    v.check(bmin >= lo - 0.05, format!("{leg} min delay {bmin:.2} ms (want >= {lo:.2})"));
    // (packets the FIFO line held back, e.g. draining after a spike, are not in `base`)
    let cap = hi + slack;
    v.check(bmax <= cap, format!("{leg} max delay {bmax:.2} ms (want <= {cap:.2})"));
    let bm = mean(&base);
    v.check(bm >= imp.delay - imp.jitter * 0.25 - 0.5 && bm <= imp.delay + imp.jitter * 0.75 + slack,
        format!("{leg} mean delay {bm:.2} ms (want {:.2}..{:.2}: delay, FIFO only adds)", imp.delay - imp.jitter * 0.25 - 0.5, imp.delay + imp.jitter * 0.75 + slack));
    if internal && imp.jitter >= 2.0 {
        // the jitter model itself (before FIFO)
        let js: Vec<f64> = tr.iter().filter(|t| !t.copy).map(|t| t.jit).collect();
        let jmin = js.iter().cloned().fold(f64::INFINITY, f64::min);
        let jmax = js.iter().cloned().fold(f64::NEG_INFINITY, f64::max);
        v.check(jmin >= -imp.jitter - 1e-9 && jmax <= imp.jitter + 1e-9, format!("{leg} jitter within ±{}: [{jmin:.2}, {jmax:.2}]", imp.jitter));
        let want_sd = imp.jitter / 3f64.sqrt() * CLIPPED_SD;
        let s = stdev(&js);
        // correlated samples: about n / (2 tau) independent ones at 1 ms spacing
        let n_eff = (js.len() as f64 / (2.0 * imp.jitter_tau_ms.max(0.5))).max(1.0);
        let rel = (3.0 / (2.0 * n_eff).sqrt()).clamp(0.15, 0.6);
        v.check((s - want_sd).abs() <= rel * want_sd + 0.3, format!("{leg} jitter stdev {s:.2} ms (want {want_sd:.2} ± {:.0} %)", rel * 100.0));
        let ac = autocorr1(&js);
        let want_ac = if imp.jitter_tau_ms > 0.0 { (-1.0 / imp.jitter_tau_ms).exp() } else { 0.0 };
        v.check((ac - want_ac).abs() <= 0.12, format!("{leg} jitter lag-1 autocorrelation {ac:.3} (want {want_ac:.3} at 1 ms spacing, tau {} ms)", imp.jitter_tau_ms));
    }
    if internal {
        let (ln, l50, l99, lmax) = late_summary(late);
        let f = |x: Option<f64>| x.map(|x| format!("{x:.3}")).unwrap_or("-".into());
        v.check(l99.map(|x| x <= 1.0).unwrap_or(false),
            format!("{leg} scheduler lateness n={ln} p50 {}  p99 {}  max {} ms (want p99 <= 1.0)", f(l50), f(l99), f(lmax)));
        // the whole path: arrival vs the intended delivery (includes the loopback hop)
        let mut intended: HashMap<u64, Vec<Instant>> = HashMap::new();
        for t in tr {
            intended.entry(t.id).or_default().push(t.at);
        }
        let mut terr = vec![];
        for (id, t) in arr {
            if let Some(v) = intended.get_mut(id) {
                if !v.is_empty() {
                    let k = (0..v.len()).min_by_key(|&k| v[k]).unwrap();
                    let at = v.remove(k);
                    terr.push(if *t >= at { (*t - at).as_secs_f64() * 1000.0 } else { -(at - *t).as_secs_f64() * 1000.0 });
                }
            }
        }
        v.check(pct(&terr, 99.0) <= 1.0 && pct(&terr, 0.0) >= -0.05,
            format!("{leg} proxy timing error (arrival - due) p50 {:.3}  p99 {:.3}  max {:.3} ms (want p99 <= 1.0)", pct(&terr, 50.0), pct(&terr, 99.0), pct(&terr, 100.0)));
    }
    println!(
        "  {leg:4} n={n} recv={copies} | added delay p1 {:.2}  p50 {:.2}  p99 {:.2}  mean {:.2} ms | lost {:.2}% bursts {} mean {mburst:.2} | reorder {:.3}%",
        pct(&added, 1.0), pct(&added, 50.0), pct(&added, 99.0), mean(&added), lost * 100.0, runs.len(), rr * 100.0
    );
}

/// A UDP receiver thread recording (id, arrival, source).
fn receiver(sock: Arc<UdpSocket>, stop: Arc<AtomicBool>) -> (std::thread::JoinHandle<()>, Arc<Mutex<Vec<(u64, Instant, SocketAddr)>>>) {
    let arr: Arc<Mutex<Vec<(u64, Instant, SocketAddr)>>> = Arc::new(Mutex::new(vec![]));
    let a2 = arr.clone();
    let h = std::thread::spawn(move || {
        boost_thread();
        let mut b = [0u8; 2048];
        while !stop.load(Relaxed) {
            if let Ok((k, src)) = sock.recv_from(&mut b) {
                let t = Instant::now();
                if k >= 8 {
                    a2.lock().unwrap().push((u64::from_le_bytes(b[..8].try_into().unwrap()), t, src));
                }
            }
        }
    });
    (h, arr)
}

/// Send `n` packets 1/ms from `from` to `to`, ids base..base+n. Returns the send times.
fn send_leg(from: &UdpSocket, to: SocketAddr, n: usize, base: u64) -> Result<Vec<Instant>> {
    let mut sent = Vec::with_capacity(n);
    let t0 = Instant::now() + Duration::from_millis(20);
    for i in 0..n {
        sleep_until(t0 + Duration::from_millis(i as u64));
        let mut pkt = [0u8; 16];
        pkt[..8].copy_from_slice(&(base + i as u64).to_le_bytes());
        sent.push(Instant::now());
        from.send_to(&pkt, to)?;
    }
    Ok(sent)
}

/// One measured run: client -> proxy -> sink (n packets), then sink -> proxy -> client (n/2).
fn measure(name: &str, imp: Impair, n: usize, external: Option<(SocketAddr, SocketAddr)>) -> Result<bool> {
    const DOWN_BASE: u64 = 1 << 32;
    let sink_addr = match external {
        Some((_, s)) => s,
        None => "127.0.0.1:0".parse().unwrap(),
    };
    let sink = Arc::new(UdpSocket::bind(sink_addr).with_context(|| format!("bind sink {sink_addr}"))?);
    sink.set_read_timeout(Some(Duration::from_millis(50)))?;
    let sink_addr = sink.local_addr()?;
    let client = Arc::new(UdpSocket::bind("127.0.0.1:0")?);
    client.set_read_timeout(Some(Duration::from_millis(50)))?;
    let stop = Arc::new(AtomicBool::new(false));
    let (sink_thread, sink_arr) = receiver(sink.clone(), stop.clone());
    let (client_thread, client_arr) = receiver(client.clone(), stop.clone());
    let trace: Trace = Arc::new(Mutex::new(vec![]));
    let late_all: LateAll = Arc::new(Mutex::new(vec![]));
    let pstop = Arc::new(AtomicBool::new(false));
    let (proxy_addr, proxy_thread) = match external {
        Some((p, _)) => (p, None),
        None => {
            let (tx, rx) = std::sync::mpsc::channel();
            let (st, tr, la) = (pstop.clone(), trace.clone(), late_all.clone());
            let h = std::thread::spawn(move || {
                let _ = proxy(
                    ProxyOpts {
                        listen: "127.0.0.1:0".parse().unwrap(),
                        upstream: sink_addr,
                        imp,
                        profile: None,
                        seed: Some(7),
                        trace: Some(tr),
                        late_all: Some(la),
                        stop: st,
                        verbose: false,
                        control_file: None,
                        parent_pid: None,
                        events: None,
                    },
                    tx,
                );
            });
            (rx.recv_timeout(Duration::from_secs(5)).context("proxy did not start")?, Some(h))
        }
    };
    let tail = Duration::from_secs_f64((imp.delay + imp.jitter + imp.spike_ms + imp.reorder_ms + imp.queue_ms + 300.0) / 1000.0);
    // up leg
    let sent_up = send_leg(&client, proxy_addr, n, 0)?;
    std::thread::sleep(tail);
    // down leg: the sink answers the proxy's per-client upstream socket
    let up_src = sink_arr.lock().unwrap().first().map(|a| a.2);
    let nd = (n / 2).max(100);
    let sent_down = match up_src {
        Some(src) => {
            let s = send_leg(&sink, src, nd, DOWN_BASE)?;
            std::thread::sleep(tail);
            s
        }
        None => vec![],
    };
    stop.store(true, Relaxed);
    pstop.store(true, Relaxed);
    let _ = sink_thread.join();
    let _ = client_thread.join();
    if let Some(h) = proxy_thread {
        let _ = h.join();
    }

    let internal = external.is_none();
    let tr = trace.lock().unwrap().clone();
    let la = late_all.lock().unwrap().clone();
    let mut v = Verdict { lines: vec![], ok: true };
    println!(
        "{name:8} profile delay {} ± {} (tau {} ms) spikes {}ms/{}s loss {}%/{}% burst {} dup {}% reorder {}%/{}ms rate {}/{} kbps queue {} ms",
        imp.delay, imp.jitter, imp.jitter_tau_ms, imp.spike_ms, imp.spike_every, imp.loss_up, imp.loss_down, imp.loss_burst, imp.dup,
        imp.reorder, imp.reorder_ms, imp.rate_up_kbps, imp.rate_down_kbps, imp.queue_ms
    );
    let up_arr: Vec<(u64, Instant)> = sink_arr.lock().unwrap().iter().filter(|a| a.0 < n as u64).map(|a| (a.0, a.1)).collect();
    let up_tr: Vec<TraceRec> = tr.iter().filter(|t| t.dir == UP).copied().collect();
    let up_late: Vec<f64> = la.iter().filter(|l| l.0 == UP).map(|l| l.1).collect();
    analyze_leg(&mut v, "up", UP, &imp, n, &sent_up, &up_arr, &up_tr, &up_late, internal);
    if sent_down.is_empty() {
        v.check(false, "down leg not run: no packet reached the sink".into());
    } else {
        let dn_arr: Vec<(u64, Instant)> = client_arr.lock().unwrap().iter().filter(|a| a.0 >= DOWN_BASE).map(|a| (a.0 - DOWN_BASE, a.1)).collect();
        let dn_tr: Vec<TraceRec> = tr.iter().filter(|t| t.dir == DOWN).map(|t| TraceRec { id: t.id.wrapping_sub(DOWN_BASE), ..*t }).collect();
        let dn_late: Vec<f64> = la.iter().filter(|l| l.0 == DOWN).map(|l| l.1).collect();
        analyze_leg(&mut v, "down", DOWN, &imp, nd, &sent_down, &dn_arr, &dn_tr, &dn_late, internal);
    }
    for l in &v.lines {
        println!("{l}");
    }
    Ok(v.ok)
}

fn self_test(a: &Args) -> Result<i32> {
    let ext = match &a.external {
        Some(e) => Some((parse_addr(e)?, parse_addr(&a.sink)?)),
        None => None,
    };
    let mut runs: Vec<(String, Impair)> = vec![];
    let base = Impair::from_args(a);
    match &a.profile {
        Some(p) => runs.push((p.clone(), base)),
        None if ext.is_some() => runs.push(("custom".into(), base)),
        None => {
            for p in PROFILES {
                let mut i = base;
                i.apply_profile(p.0);
                runs.push((p.0.to_string(), i));
            }
        }
    }
    if a.packets < 100 {
        bail!("--packets must be >= 100");
    }
    let mut all_ok = true;
    for (name, imp) in runs {
        all_ok &= measure(&name, imp, a.packets, ext)?;
    }
    println!("{}", if all_ok { "SELF-TEST OK" } else { "SELF-TEST FAILED" });
    Ok(if all_ok { 0 } else { 1 })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rng() -> StdRng {
        StdRng::seed_from_u64(42)
    }

    #[test]
    fn profiles_have_a_path_model_and_keep_their_delays() {
        for p in PROFILES {
            assert!(PROFILE_PATH.iter().any(|q| q.0 == p.0), "{} has no path model", p.0);
        }
        // the gate's pose one-way latency limits depend on these
        let d = |n: &str| Impair::profile(n).unwrap().delay;
        assert_eq!((d("lan"), d("good"), d("typical"), d("wifi"), d("bad"), d("awful")), (1.0, 20.0, 50.0, 35.0, 110.0, 180.0));
        let t = Impair::profile("typical").unwrap();
        assert!(t.reorder > 0.0 && t.reorder < 0.5 && t.loss_burst > 1.0 && t.loss_up == t.loss && t.loss_down == t.loss);
        let w = Impair::profile("wifi").unwrap();
        assert!((w.reorder - 1.0).abs() < 1e-9);
        assert_eq!(Impair::profile("good").unwrap().reorder, 0.0);
        assert_eq!(Impair::profile("lan").unwrap().reorder, 0.0);
    }

    #[test]
    fn ar1_jitter_is_bounded_with_the_old_stdev_and_correlated() {
        let mut r = rng();
        let (j, tau) = (25.0, 30.0);
        let mut s = 0.0;
        let mut v = vec![];
        for _ in 0..200_000 {
            s = ar1_next(s, 1.0, tau, gauss(&mut r));
            v.push(jitter_ms(s, j));
        }
        assert!(v.iter().all(|x| x.abs() <= j));
        assert!(mean(&v).abs() < 1.0, "mean {}", mean(&v));
        let want = j / 3f64.sqrt() * CLIPPED_SD;
        assert!((stdev(&v) - want).abs() < 0.08 * want, "sd {} want {want}", stdev(&v));
        let ac = autocorr1(&v);
        assert!((ac - (-1.0f64 / tau).exp()).abs() < 0.02, "ac {ac}");
        // tau 0 = independent
        let mut w = vec![];
        for _ in 0..50_000 {
            w.push(ar1_next(0.7, 1.0, 0.0, gauss(&mut r)));
        }
        assert!(autocorr1(&w).abs() < 0.03);
        // long gaps decorrelate
        assert!((ar1_next(1.0, 1e9, 30.0, 0.25) - 0.25).abs() < 1e-9);
    }

    #[test]
    fn gilbert_elliott_keeps_the_mean_and_makes_bursts() {
        let mut r = rng();
        for (p, b) in [(0.01, 2.0), (0.02, 3.0), (0.10, 5.0), (0.05, 1.0)] {
            let n = 400_000;
            let mut bad = false;
            let mut lost = 0usize;
            let mut runs = vec![];
            let mut cur = 0;
            for _ in 0..n {
                bad = ge_step(bad, p, b, r.gen());
                if bad {
                    lost += 1;
                    cur += 1;
                } else if cur > 0 {
                    runs.push(cur);
                    cur = 0;
                }
            }
            let m = lost as f64 / n as f64;
            assert!((m - p).abs() < 0.12 * p, "p {p} b {b}: mean {m}");
            let mb = runs.iter().sum::<usize>() as f64 / runs.len() as f64;
            let want = if b <= 1.0 { 1.0 / (1.0 - p) } else { b };
            assert!((mb - want).abs() < 0.1 * want, "p {p} b {b}: burst {mb}");
        }
        assert!(!ge_step(true, 0.0, 3.0, 0.0));
        assert!(ge_step(false, 1.0, 3.0, 0.99));
    }

    #[test]
    fn rate_cap_queues_and_tail_drops() {
        let t0 = Instant::now();
        // no cap: immediate
        assert_eq!(rate_slot(t0, None, 1000, 0.0, 10.0), Some(t0));
        // 1000 bytes at 8000 kbit/s = 1 ms each
        let mut free = None;
        let mut ends = vec![];
        for _ in 0..5 {
            match rate_slot(t0, free, 1000, 8000.0, 2.5) {
                Some(e) => {
                    free = Some(e);
                    ends.push(e);
                }
                None => ends.push(t0),
            }
        }
        let ms = |t: Instant| (t - t0).as_secs_f64() * 1000.0;
        assert!((ms(ends[0]) - 1.0).abs() < 1e-6 && (ms(ends[1]) - 2.0).abs() < 1e-6 && (ms(ends[2]) - 3.0).abs() < 1e-6);
        // the 4th packet would wait 3 ms > 2.5: dropped
        assert_eq!(rate_slot(t0, free, 1000, 8000.0, 2.5), None);
        // later, the queue has drained
        assert!(rate_slot(t0 + Duration::from_millis(10), free, 1000, 8000.0, 2.5).is_some());
        // unbounded queue never drops
        assert!(rate_slot(t0, Some(t0 + Duration::from_secs(5)), 1000, 8000.0, 0.0).is_some());
    }

    #[test]
    fn lateness_summary_percentiles() {
        assert_eq!(late_summary(&[]), (0, None, None, None));
        let v: Vec<f64> = (1..=100).map(|x| x as f64 / 10.0).collect();
        let (n, p50, p99, max) = late_summary(&v);
        assert_eq!(n, 100);
        assert_eq!(p50, Some(5.0));
        assert_eq!(p99, Some(9.9));
        assert_eq!(max, Some(10.0));
    }

    #[test]
    fn reorder_and_loss_run_measures() {
        assert_eq!(reorder_count(&[0, 1, 2, 3]), 0);
        assert_eq!(reorder_count(&[0, 2, 1, 3]), 1);
        assert_eq!(reorder_count(&[1, 2, 3, 0]), 1);
        assert_eq!(reorder_count(&[0, 1, 1, 2]), 0); // a duplicate is not a reorder
        let got: std::collections::HashSet<u64> = [0, 3, 4, 7].into_iter().collect();
        assert_eq!(loss_runs(9, &got), vec![2, 2, 1]);
    }

    fn core(imp: Impair) -> Core {
        Core {
            imp: Mutex::new(imp),
            blackout: AtomicBool::new(false),
            rng: Mutex::new(StdRng::seed_from_u64(1)),
            start: Instant::now(),
            sched: Scheduler::new(None),
            stats: Stats::default(),
            dirs: Mutex::new([DirState::default(); 2]),
            trace: Some(Arc::new(Mutex::new(vec![]))),
        }
    }

    #[test]
    fn fifo_keeps_order_and_reorder_is_explicit() {
        let sock = Arc::new(UdpSocket::bind("127.0.0.1:0").unwrap());
        let dest = sock.local_addr().unwrap();
        // heavy, uncorrelated jitter, 1 packet "at once": FIFO must keep every packet in order
        let mut imp = Impair::profile("awful").unwrap();
        imp.loss_up = 0.0;
        imp.loss_down = 0.0;
        imp.dup = 5.0;
        imp.reorder = 0.0;
        imp.jitter_tau_ms = 0.0;
        imp.spike_every = 0.0;
        imp.rate_up_kbps = 0.0;
        let c = core(imp);
        for i in 0..2000u64 {
            c.schedule(UP, &sock, &i.to_le_bytes(), dest);
        }
        c.sched.stop.store(true, Relaxed);
        let tr = c.trace.as_ref().unwrap().lock().unwrap().clone();
        let mut ats: Vec<(Instant, u64)> = tr.iter().filter(|t| !t.copy).map(|t| (t.at, t.id)).collect();
        assert!(ats.windows(2).all(|w| w[0].0 <= w[1].0), "FIFO violated");
        // duplicates follow their original closely and never pass a later packet
        for t in tr.iter().filter(|t| t.copy) {
            let orig = tr.iter().find(|o| o.id == t.id && !o.copy).unwrap();
            let d = (t.at - orig.at).as_secs_f64() * 1000.0;
            assert!((0.05..=0.5).contains(&d), "dup gap {d}");
            if let Some(next) = tr.iter().find(|o| o.id == t.id + 1 && !o.copy) {
                assert!(next.at >= t.at);
            }
        }
        ats.sort();
        assert_eq!(ats.iter().map(|a| a.1).collect::<Vec<_>>(), (0..2000).collect::<Vec<_>>());

        // explicit reorder: about reorder_pct of packets are held back
        let mut imp2 = imp;
        imp2.reorder = 2.0;
        imp2.dup = 0.0;
        let c = core(imp2);
        for i in 0..20_000u64 {
            c.schedule(UP, &sock, &i.to_le_bytes(), dest);
        }
        c.sched.stop.store(true, Relaxed);
        let tr = c.trace.as_ref().unwrap().lock().unwrap().clone();
        let k = tr.iter().filter(|t| t.reordered).count() as f64 / 20_000.0;
        assert!((k - 0.02).abs() < 0.005, "reorder share {k}");
        assert_eq!(c.stats.reordered[UP].load(Relaxed) as usize, tr.iter().filter(|t| t.reordered).count());
    }

    #[test]
    fn per_direction_loss_and_stats_fields() {
        let sock = Arc::new(UdpSocket::bind("127.0.0.1:0").unwrap());
        let dest = sock.local_addr().unwrap();
        let mut imp = Impair::profile("lan").unwrap();
        imp.loss_up = 50.0;
        imp.loss_down = 0.0;
        let c = core(imp);
        for i in 0..2000u64 {
            c.schedule(UP, &sock, &i.to_le_bytes(), dest);
            c.schedule(DOWN, &sock, &i.to_le_bytes(), dest);
        }
        std::thread::sleep(Duration::from_millis(60));
        let st = c.stats_json(1);
        c.sched.stop.store(true, Relaxed);
        let up_lost = st["lost_up"].as_u64().unwrap();
        assert!((800..1200).contains(&up_lost), "{up_lost}");
        assert_eq!(st["lost_down"], 0);
        assert_eq!(st["lost"].as_u64().unwrap(), up_lost);
        assert_eq!(st["in"], 4000);
        assert_eq!(st["up"]["lost"], up_lost);
        assert_eq!(st["down"]["in"], 2000);
        assert!(st["late_n"].as_u64().unwrap() > 0);
        for k in ["late_p50_ms", "late_p99_ms", "late_max_ms"] {
            assert!(st[k].as_f64().is_some(), "{k}");
        }
        for k in ["late_n", "late_p99_ms", "late_max_ms", "lost", "in"] {
            assert!(st["up"].get(k).is_some() && st["down"].get(k).is_some(), "{k}");
        }
        for k in ["out", "dup", "blackout_dropped", "queued", "clients", "queue_dropped", "reordered"] {
            assert!(st.get(k).is_some(), "{k}");
        }
        // the window resets: nothing delivered since
        let st2 = c.stats_json(1);
        assert_eq!(st2["late_n"], 0);
        assert!(st2["late_p99_ms"].is_null());
    }

    #[test]
    fn idle_clients_are_pruned() {
        let mk = |last: u64| Client {
            up: Arc::new(UdpSocket::bind("127.0.0.1:0").unwrap()),
            last_ms: Arc::new(AtomicU64::new(last)),
            stop: Arc::new(AtomicBool::new(false)),
        };
        let mut m: HashMap<SocketAddr, Client> = HashMap::new();
        m.insert("127.0.0.1:1".parse().unwrap(), mk(0));
        m.insert("127.0.0.1:2".parse().unwrap(), mk(100_000));
        let old_stop = m[&"127.0.0.1:1".parse().unwrap()].stop.clone();
        assert_eq!(prune_idle(&mut m, 120_000, CLIENT_IDLE_MS), 1);
        assert_eq!(m.len(), 1);
        assert!(old_stop.load(Relaxed));
        assert!(m.contains_key(&"127.0.0.1:2".parse().unwrap()));
        assert_eq!(prune_idle(&mut m, 120_000, CLIENT_IDLE_MS), 0);
    }

    #[test]
    fn impair_json_keeps_the_old_keys() {
        let j = impair_json(&Impair::profile("wifi").unwrap());
        for k in ["delay", "jitter", "loss", "dup", "spike_every", "spike_ms", "spike_len", "reorder_pct", "reorder_ms", "jitter_tau_ms",
                  "loss_burst", "loss_up", "loss_down", "rate_up_kbps", "rate_down_kbps", "queue_ms", "fifo"] {
            assert!(j.get(k).is_some(), "{k}");
        }
        assert_eq!(j["delay"], 35.0);
        assert_eq!(j["loss"], 2.0);
    }
}
