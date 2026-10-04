//! `hsmp-tools net-bench [--clients N] [--secs S] [--warmup W] [--bins DIR] [--json OUT]`:
//! a reproducible, offline benchmark of the network / IPC stack (no game).
//!
//! Starts the real `hsmp-server` (HSMP_PERF=1, private identity / state dir, free UDP port)
//! and N fake games (`hsmp-tools ipc-game --synth`), each with its own real `hsmp-sidecar`
//! attached over HSMP-SHM. Every sidecar talks to the server through an in-process counting
//! UDP proxy (one upstream socket per client), so per-client bytes and packets in both
//! directions are exact. After the warmup it measures, over --secs:
//!
//! * process CPU time (kernel + user, GetProcessTimes) of the server, every sidecar and
//!   every fake game, as % of one core;
//! * UDP payload bytes / packets per second per client, up (client -> server) and down,
//!   also with the 28 B IPv4 + UDP header per packet;
//! * each fake game's slot-write cost (`synth_stats`: avg / p99 µs per slot kind);
//! * the server's own HSMP_PERF report (tick / packet-handling µs, thinned, budget drops);
//! * the machine's idle % before the run and during the window (other load skews CPU).
//!
//! Every child gets `--parent-pid` (server, games; each sidecar gets its game), so even a
//! hard kill of this process leaves no orphans; on a normal exit, an error or Ctrl-C the
//! children are stopped by PID (games first via --stop-file, so they print their stats).

use anyhow::{bail, Context, Result};
use serde_json::{json, Value as J};
use std::collections::HashMap;
use std::net::{SocketAddr, UdpSocket};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

#[derive(clap::Args)]
pub struct Args {
    /// Number of players (fake game + sidecar pairs).
    #[arg(long, default_value_t = 4)]
    pub clients: usize,
    /// Measurement window, s.
    #[arg(long, default_value_t = 30.0)]
    pub secs: f64,
    /// Seconds of steady traffic before the window starts.
    #[arg(long, default_value_t = 5.0)]
    pub warmup: f64,
    /// Dir with hsmp-server(.exe) and hsmp-sidecar(.exe) (default: this exe's dir).
    #[arg(long)]
    pub bins: Option<PathBuf>,
    /// Write the report here as JSON.
    #[arg(long)]
    pub json: Option<PathBuf>,
    /// Pose / root / weapon sample rate of each fake game (HSMPSync send_hz).
    #[arg(long, default_value_t = 60.0)]
    pub hz: f64,
    /// Vitals slot rate of each fake game.
    #[arg(long, default_value_t = 20.0)]
    pub vitals_hz: f64,
    /// Server --tick-hz.
    #[arg(long, default_value_t = 60)]
    pub tick_hz: u32,
    /// Give up if the sidecars are not all connected after this many seconds.
    #[arg(long, default_value_t = 30.0)]
    pub connect_timeout: f64,
    /// Keep the work dir (logs, state dirs) instead of deleting it.
    #[arg(long)]
    pub keep: bool,
}

/// UDP/IPv4 header bytes per datagram (20 IP + 8 UDP).
const HDR: u64 = 28;

// ---- CPU accounting -------------------------------------------------------------------

/// A process handle for CPU-time reads (kept open so an exited process still reads).
struct Cpu {
    #[cfg(windows)]
    h: windows_sys::Win32::Foundation::HANDLE,
}

#[cfg(windows)]
fn ft(f: &windows_sys::Win32::Foundation::FILETIME) -> u64 {
    ((f.dwHighDateTime as u64) << 32) | f.dwLowDateTime as u64
}

impl Cpu {
    #[cfg(windows)]
    fn open(pid: u32) -> Option<Cpu> {
        use windows_sys::Win32::System::Threading::{OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION};
        // SAFETY: plain Win32 call; the handle is closed in Drop.
        let h = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
        (h != 0).then_some(Cpu { h })
    }
    #[cfg(not(windows))]
    fn open(_pid: u32) -> Option<Cpu> {
        None
    }
    /// Kernel + user time, 100 ns units.
    #[cfg(windows)]
    fn time(&self) -> Option<u64> {
        use windows_sys::Win32::Foundation::FILETIME;
        use windows_sys::Win32::System::Threading::GetProcessTimes;
        let z = FILETIME { dwLowDateTime: 0, dwHighDateTime: 0 };
        let (mut c, mut e, mut k, mut u) = (z, z, z, z);
        // SAFETY: valid handle owned by self; out-params are locals.
        let ok = unsafe { GetProcessTimes(self.h, &mut c, &mut e, &mut k, &mut u) };
        (ok != 0).then(|| ft(&k) + ft(&u))
    }
    #[cfg(not(windows))]
    fn time(&self) -> Option<u64> {
        None
    }
}

#[cfg(windows)]
impl Drop for Cpu {
    fn drop(&mut self) {
        // SAFETY: handle opened by Cpu::open, closed once.
        unsafe { windows_sys::Win32::Foundation::CloseHandle(self.h) };
    }
}

/// (idle, kernel incl. idle, user) system times, 100 ns units summed over all cores.
#[cfg(windows)]
fn system_times() -> Option<(u64, u64, u64)> {
    use windows_sys::Win32::Foundation::FILETIME;
    use windows_sys::Win32::System::Threading::GetSystemTimes;
    let z = FILETIME { dwLowDateTime: 0, dwHighDateTime: 0 };
    let (mut i, mut k, mut u) = (z, z, z);
    // SAFETY: out-params are locals.
    let ok = unsafe { GetSystemTimes(&mut i, &mut k, &mut u) };
    (ok != 0).then(|| (ft(&i), ft(&k), ft(&u)))
}
#[cfg(not(windows))]
fn system_times() -> Option<(u64, u64, u64)> {
    None
}

fn idle_pct(a: Option<(u64, u64, u64)>, b: Option<(u64, u64, u64)>) -> Option<f64> {
    let (a, b) = (a?, b?);
    let idle = b.0.saturating_sub(a.0) as f64;
    let total = (b.1.saturating_sub(a.1) + b.2.saturating_sub(a.2)) as f64;
    (total > 0.0).then(|| 100.0 * idle / total)
}

// ---- the counting proxy ---------------------------------------------------------------

/// Per-client counters (one client = one source address seen by the proxy).
#[derive(Default)]
pub struct Counters {
    pub up_bytes: AtomicU64,
    pub up_pkts: AtomicU64,
    pub down_bytes: AtomicU64,
    pub down_pkts: AtomicU64,
}

#[derive(Clone, Copy, Default, Debug, PartialEq)]
pub struct Snap {
    pub up_bytes: u64,
    pub up_pkts: u64,
    pub down_bytes: u64,
    pub down_pkts: u64,
}

impl Counters {
    fn snap(&self) -> Snap {
        Snap {
            up_bytes: self.up_bytes.load(Ordering::Relaxed),
            up_pkts: self.up_pkts.load(Ordering::Relaxed),
            down_bytes: self.down_bytes.load(Ordering::Relaxed),
            down_pkts: self.down_pkts.load(Ordering::Relaxed),
        }
    }
}

/// A UDP proxy `listen` -> `upstream` that gives every client its own upstream socket and
/// counts payload bytes / datagrams per client and direction.
pub struct Proxy {
    pub addr: SocketAddr,
    stop: Arc<AtomicBool>,
    clients: Arc<Mutex<Vec<(SocketAddr, Arc<Counters>)>>>,
    threads: Vec<std::thread::JoinHandle<()>>,
    pub upstream_threads: Arc<Mutex<Vec<std::thread::JoinHandle<()>>>>,
}

fn big_udp(addr: SocketAddr) -> Result<UdpSocket> {
    let s = socket2::Socket::new(socket2::Domain::IPV4, socket2::Type::DGRAM, Some(socket2::Protocol::UDP))?;
    // a fan-out burst of N peers' frames must not overflow a 64 KB default buffer
    let _ = s.set_recv_buffer_size(8 << 20);
    let _ = s.set_send_buffer_size(8 << 20);
    s.bind(&addr.into())?;
    let s: UdpSocket = s.into();
    s.set_read_timeout(Some(Duration::from_millis(50)))?;
    Ok(s)
}

impl Proxy {
    pub fn start(upstream: SocketAddr) -> Result<Proxy> {
        let listen = Arc::new(big_udp("127.0.0.1:0".parse()?)?);
        let addr = listen.local_addr()?;
        let stop = Arc::new(AtomicBool::new(false));
        let clients: Arc<Mutex<Vec<(SocketAddr, Arc<Counters>)>>> = Arc::default();
        let up_threads: Arc<Mutex<Vec<std::thread::JoinHandle<()>>>> = Arc::default();
        let (l, st, cl, ut) = (listen.clone(), stop.clone(), clients.clone(), up_threads.clone());
        let main = std::thread::Builder::new().name("bench-proxy".into()).spawn(move || {
            let mut map: HashMap<SocketAddr, (Arc<UdpSocket>, Arc<Counters>)> = HashMap::new();
            let mut buf = vec![0u8; 65536];
            while !st.load(Ordering::Relaxed) {
                let (n, from) = match l.recv_from(&mut buf) {
                    Ok(x) => x,
                    Err(_) => continue, // timeout (or a WSAECONNRESET from an ICMP): keep going
                };
                let entry = match map.get(&from) {
                    Some(e) => e.clone(),
                    None => {
                        let Ok(up) = big_udp("127.0.0.1:0".parse().unwrap()) else { continue };
                        if up.connect(upstream).is_err() {
                            continue;
                        }
                        let up = Arc::new(up);
                        let c = Arc::new(Counters::default());
                        cl.lock().unwrap().push((from, c.clone()));
                        let (up2, c2, l2, st2) = (up.clone(), c.clone(), l.clone(), st.clone());
                        let h = std::thread::Builder::new().name("bench-proxy-up".into()).spawn(move || {
                            let mut b = vec![0u8; 65536];
                            while !st2.load(Ordering::Relaxed) {
                                if let Ok(n) = up2.recv(&mut b) {
                                    if l2.send_to(&b[..n], from).is_ok() {
                                        c2.down_bytes.fetch_add(n as u64, Ordering::Relaxed);
                                        c2.down_pkts.fetch_add(1, Ordering::Relaxed);
                                    }
                                }
                            }
                        });
                        if let Ok(h) = h {
                            ut.lock().unwrap().push(h);
                        }
                        map.insert(from, (up.clone(), c.clone()));
                        (up, c)
                    }
                };
                if entry.0.send(&buf[..n]).is_ok() {
                    entry.1.up_bytes.fetch_add(n as u64, Ordering::Relaxed);
                    entry.1.up_pkts.fetch_add(1, Ordering::Relaxed);
                }
            }
        })?;
        Ok(Proxy { addr, stop, clients, threads: vec![main], upstream_threads: up_threads })
    }

    /// Counters of every client seen so far, in order of first packet.
    pub fn snap(&self) -> Vec<(SocketAddr, Snap)> {
        self.clients.lock().unwrap().iter().map(|(a, c)| (*a, c.snap())).collect()
    }
}

impl Drop for Proxy {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        for t in self.threads.drain(..) {
            let _ = t.join();
        }
        for t in self.upstream_threads.lock().unwrap().drain(..) {
            let _ = t.join();
        }
    }
}

// ---- children ---------------------------------------------------------------------------

struct Kids {
    kids: Vec<(String, Child)>,
    stop_file: PathBuf,
}

impl Kids {
    fn spawn(&mut self, name: &str, cmd: &mut Command, log: &Path) -> Result<u32> {
        let lf = std::fs::File::create(log).with_context(|| format!("create {}", log.display()))?;
        let c = cmd
            .stdin(Stdio::null())
            .stdout(Stdio::from(lf.try_clone()?))
            .stderr(Stdio::from(lf))
            .spawn()
            .with_context(|| format!("spawn {name}"))?;
        let pid = c.id();
        self.kids.push((name.to_string(), c));
        Ok(pid)
    }

    fn dead(&mut self) -> Option<String> {
        for (n, c) in &mut self.kids {
            if let Ok(Some(st)) = c.try_wait() {
                return Some(format!("{n} exited early ({st})"));
            }
        }
        None
    }

    /// Games first (stop file: they print synth_stats and the sidecars see them go), then
    /// whatever is left, by PID.
    fn stop(&mut self) {
        let _ = std::fs::write(&self.stop_file, b"stop");
        let deadline = Instant::now() + Duration::from_secs(4);
        while Instant::now() < deadline {
            let all = self.kids.iter_mut().filter(|(n, _)| n.starts_with("game")).all(|(_, c)| matches!(c.try_wait(), Ok(Some(_))));
            if all {
                break;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        for (_, c) in &mut self.kids {
            if !matches!(c.try_wait(), Ok(Some(_))) {
                let _ = c.kill(); // TerminateProcess on this child's own handle (by PID, never by name)
            }
        }
        for (_, c) in &mut self.kids {
            let _ = c.wait();
        }
        self.kids.clear();
    }
}

impl Drop for Kids {
    fn drop(&mut self) {
        self.stop();
    }
}

/// Sidecars are grandchildren (spawned by their game): stop any that outlive their game
/// (they exit by themselves through --parent-pid within ~1 s; this is the backstop).
fn kill_pid(pid: u32) {
    #[cfg(windows)]
    {
        use windows_sys::Win32::System::Threading::{OpenProcess, TerminateProcess, PROCESS_TERMINATE};
        // SAFETY: plain Win32 calls on a handle opened and closed here.
        unsafe {
            let h = OpenProcess(PROCESS_TERMINATE, 0, pid);
            if h != 0 {
                TerminateProcess(h, 1);
                windows_sys::Win32::Foundation::CloseHandle(h);
            }
        }
    }
    #[cfg(not(windows))]
    let _ = pid;
}

fn pid_alive(pid: u32) -> bool {
    #[cfg(windows)]
    {
        hsmp_ipc::shm::ProcessHandle::open(pid).map(|h| h.is_alive()).unwrap_or(false)
    }
    #[cfg(not(windows))]
    {
        let _ = pid;
        false
    }
}

fn free_udp_port() -> Result<u16> {
    let s = UdpSocket::bind("127.0.0.1:0")?;
    Ok(s.local_addr()?.port())
}

fn exe(bins: &Path, n: &str) -> PathBuf {
    bins.join(if cfg!(windows) { format!("{n}.exe") } else { n.to_string() })
}

/// Ctrl-C sets this (a tokio signal listener on its own thread).
fn ctrl_c_flag() -> Arc<AtomicBool> {
    let f = Arc::new(AtomicBool::new(false));
    let f2 = f.clone();
    let _ = std::thread::Builder::new().name("bench-ctrlc".into()).spawn(move || {
        if let Ok(rt) = tokio::runtime::Builder::new_current_thread().enable_all().build() {
            rt.block_on(async {
                if tokio::signal::ctrl_c().await.is_ok() {
                    f2.store(true, Ordering::SeqCst);
                }
            });
        }
    });
    f
}

/// Sleep `secs`, failing early on Ctrl-C or a dead child.
fn wait(secs: f64, ctrlc: &AtomicBool, kids: &mut Kids) -> Result<()> {
    let end = Instant::now() + Duration::from_secs_f64(secs);
    while Instant::now() < end {
        if ctrlc.load(Ordering::SeqCst) {
            bail!("interrupted");
        }
        if let Some(d) = kids.dead() {
            bail!("{d}");
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    Ok(())
}

fn read_jsonl(p: &Path) -> Vec<J> {
    std::fs::read_to_string(p)
        .unwrap_or_default()
        .lines()
        .filter_map(|l| serde_json::from_str::<J>(l.trim()).ok())
        .collect()
}

fn strip_ansi(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut it = s.chars().peekable();
    while let Some(c) = it.next() {
        if c == '\u{1b}' {
            for d in it.by_ref() {
                if d.is_ascii_alphabetic() {
                    break;
                }
            }
        } else {
            out.push(c);
        }
    }
    out
}

/// The server's HSMP_PERF lines: `perf ticks/5s=.. tick_us p50=.. p99=.. max=.. |
/// pkt_handle_us p50=.. p99=.. | in=..pps out=..pps ..KB/s | thinned=../s budget_drop=../s`.
fn perf_lines(log: &Path) -> Vec<J> {
    let text = String::from_utf8_lossy(&std::fs::read(log).unwrap_or_default()).to_string();
    let re = regex::Regex::new(
        r"perf ticks/5s=(\d+) tick_us p50=(\d+) p99=(\d+) max=(\d+) \| pkt_handle_us p50=(\d+) p99=(\d+) \| in=(\d+)pps out=(\d+)pps (\d+)KB/s \| thinned=(\d+)/s budget_drop=(\d+)/s",
    )
    .unwrap();
    text.lines()
        .filter_map(|l| {
            let l = strip_ansi(l);
            let c = re.captures(&l)?;
            let n = |i: usize| c[i].parse::<u64>().unwrap_or(0);
            Some(json!({"ticks_5s": n(1), "tick_us_p50": n(2), "tick_us_p99": n(3), "tick_us_max": n(4), "handle_us_p50": n(5),
                        "handle_us_p99": n(6), "in_pps": n(7), "out_pps": n(8), "out_kbps": n(9), "thinned_per_s": n(10), "budget_drop_per_s": n(11)}))
        })
        .collect()
}

fn r2(x: f64) -> f64 {
    (x * 100.0).round() / 100.0
}

// ---- the run ------------------------------------------------------------------------------

pub fn run(a: Args) -> Result<i32> {
    if a.clients == 0 {
        bail!("--clients must be >= 1");
    }
    let me = std::env::current_exe()?;
    let bins = match &a.bins {
        Some(b) => b.clone(),
        None => me.parent().context("exe dir")?.to_path_buf(),
    };
    for n in ["hsmp-server", "hsmp-sidecar"] {
        if !exe(&bins, n).exists() {
            bail!("{} not found (build it: cargo build --release -p hsmp-server; or pass --bins)", exe(&bins, n).display());
        }
    }
    let ctrlc = ctrl_c_flag();
    let work = crate_temp_dir()?;
    let res = run_in(&a, &me, &bins, &work, &ctrlc);
    if res.is_ok() && !a.keep {
        let _ = std::fs::remove_dir_all(&work);
    } else {
        eprintln!("net-bench: work dir kept: {}", work.display());
    }
    res
}

fn crate_temp_dir() -> Result<PathBuf> {
    hsmp_tools::paths::make_temp_dir("hsmp-net-bench-")
}

fn run_in(a: &Args, me: &Path, bins: &Path, work: &Path, ctrlc: &AtomicBool) -> Result<i32> {
    let n = a.clients;
    let my_pid = std::process::id().to_string();
    // Machine load before anything starts (2 s).
    let s0 = system_times();
    std::thread::sleep(Duration::from_secs(2));
    let idle_before = idle_pct(s0, system_times());

    let mut kids = Kids { kids: vec![], stop_file: work.join("stop") };
    let sv_dir = work.join("server");
    std::fs::create_dir_all(&sv_dir)?;
    let sport = free_udp_port()?;
    let server_pid = kids.spawn(
        "server",
        Command::new(exe(bins, "hsmp-server"))
            .current_dir(&sv_dir)
            .args(["--bind", &format!("127.0.0.1:{sport}"), "--tick-hz", &a.tick_hz.to_string()])
            .args(["--max-peers", &n.max(8).to_string(), "--parent-pid", &my_pid])
            .arg("--bans-file")
            .arg(sv_dir.join("bans.txt"))
            .env("HSMP_PERF", "1")
            .env("HSMP_STATE_DIR", &sv_dir)
            .env("HSMP_CAREER_GUARD", "0")
            .env_remove("HSMP_MASTER_URL"),
        &work.join("server.log"),
    )?;
    wait(0.5, ctrlc, &mut kids)?;
    let proxy = Proxy::start(format!("127.0.0.1:{sport}").parse()?)?;
    let target = proxy.addr.to_string();

    let sidecar = exe(bins, "hsmp-sidecar");
    let mut game_pids = Vec::new();
    for i in 0..n {
        let d = work.join(format!("c{i}"));
        std::fs::create_dir_all(&d)?;
        let pid = kids.spawn(
            &format!("game{i}"),
            Command::new(me)
                .args(["ipc-game", "--synth", "--synth-hz", &a.hz.to_string(), "--synth-vitals-hz", &a.vitals_hz.to_string()])
                .args(["--synth-seed", &(i + 1).to_string(), "--parent-pid", &my_pid])
                .arg("--synth-out")
                .arg(d.join("synth.jsonl"))
                .arg("--stop-file")
                .arg(work.join("stop"))
                .arg("--")
                .arg(&sidecar)
                .args(["--server", &target, "--nick", &format!("Bench{}", i + 1), "--state-dir"])
                .arg(&d)
                .env("HSMP_STATE_DIR", &d)
                .env("HSMP_CAREER_GUARD", "0")
                .env_remove("HSMP_MASTER_URL"),
            &d.join("game.log"),
        )?;
        game_pids.push(pid);
        // join in order (peer ids 1..N); the handshake takes a few ms on loopback
        wait(0.15, ctrlc, &mut kids)?;
    }

    // Ready: every game saw its session connected, and the proxy sees both directions for
    // every client.
    let t_connect = Instant::now();
    let mut sidecar_pids: Vec<Option<u32>> = vec![None; n];
    loop {
        for i in 0..n {
            if sidecar_pids[i].is_none() {
                let ev = read_jsonl(&work.join(format!("c{i}")).join("synth.jsonl"));
                if let Some(e) = ev.iter().find(|e| e["ev"] == "synth_connected") {
                    sidecar_pids[i] = e["sidecar_pid"].as_u64().map(|p| p as u32);
                }
            }
        }
        let flows = proxy.snap().iter().filter(|(_, s)| s.up_pkts > 0 && s.down_pkts > 0).count();
        if sidecar_pids.iter().all(|p| p.is_some()) && flows >= n {
            break;
        }
        if t_connect.elapsed().as_secs_f64() > a.connect_timeout {
            bail!(
                "not all sidecars connected after {}s ({} of {n} sessions, {flows} two-way flows; logs in {})",
                a.connect_timeout,
                sidecar_pids.iter().filter(|p| p.is_some()).count(),
                work.display()
            );
        }
        wait(0.1, ctrlc, &mut kids)?;
    }
    let connect_s = t_connect.elapsed().as_secs_f64();
    eprintln!("net-bench: {n} clients connected in {connect_s:.1}s; warmup {}s, measuring {}s", a.warmup, a.secs);
    let sidecar_pids: Vec<u32> = sidecar_pids.into_iter().flatten().collect();
    wait(a.warmup, ctrlc, &mut kids)?;

    // ---- the window --------------------------------------------------------------------
    let open = |p: u32| Cpu::open(p);
    let cpu_server = open(server_pid);
    let cpu_games: Vec<Option<Cpu>> = game_pids.iter().map(|p| open(*p)).collect();
    let cpu_sidecars: Vec<Option<Cpu>> = sidecar_pids.iter().map(|p| open(*p)).collect();
    let cpu_self = open(std::process::id());
    let read = |c: &Option<Cpu>| c.as_ref().and_then(|c| c.time());
    let t_srv0 = read(&cpu_server);
    let t_g0: Vec<Option<u64>> = cpu_games.iter().map(read).collect();
    let t_s0: Vec<Option<u64>> = cpu_sidecars.iter().map(read).collect();
    let t_me0 = read(&cpu_self);
    let net0 = proxy.snap();
    let sys0 = system_times();
    let w0 = Instant::now();

    wait(a.secs, ctrlc, &mut kids)?;

    let wall = w0.elapsed().as_secs_f64();
    let sys1 = system_times();
    let net1 = proxy.snap();
    let t_srv1 = read(&cpu_server);
    let t_g1: Vec<Option<u64>> = cpu_games.iter().map(read).collect();
    let t_s1: Vec<Option<u64>> = cpu_sidecars.iter().map(read).collect();
    let t_me1 = read(&cpu_self);
    let idle_during = idle_pct(sys0, sys1);

    // ---- stop everything, then collect -----------------------------------------------
    kids.stop();
    for p in &sidecar_pids {
        let end = Instant::now() + Duration::from_secs(3);
        while pid_alive(*p) && Instant::now() < end {
            std::thread::sleep(Duration::from_millis(50));
        }
        if pid_alive(*p) {
            kill_pid(*p);
        }
    }
    drop(proxy);

    let pct = |a: Option<u64>, b: Option<u64>| -> Option<f64> { Some((b?.saturating_sub(a?)) as f64 / 1e7 / wall * 100.0) };
    let server_cpu = pct(t_srv0, t_srv1);
    let game_cpu: Vec<Option<f64>> = t_g0.iter().zip(&t_g1).map(|(a, b)| pct(*a, *b)).collect();
    let sidecar_cpu: Vec<Option<f64>> = t_s0.iter().zip(&t_s1).map(|(a, b)| pct(*a, *b)).collect();
    let bench_cpu = pct(t_me0, t_me1);
    let avg = |v: &[Option<f64>]| {
        let x: Vec<f64> = v.iter().flatten().copied().collect();
        if x.is_empty() { None } else { Some(x.iter().sum::<f64>() / x.len() as f64) }
    };
    let max = |v: &[Option<f64>]| v.iter().flatten().copied().fold(None, |m: Option<f64>, x| Some(m.map_or(x, |m| m.max(x))));

    // per-client network: delta over the window per source address
    let before: HashMap<SocketAddr, Snap> = net0.into_iter().collect();
    let mut flows = Vec::new();
    for (addr, s1) in &net1 {
        let s0 = before.get(addr).copied().unwrap_or_default();
        let d = Snap {
            up_bytes: s1.up_bytes - s0.up_bytes,
            up_pkts: s1.up_pkts - s0.up_pkts,
            down_bytes: s1.down_bytes - s0.down_bytes,
            down_pkts: s1.down_pkts - s0.down_pkts,
        };
        if d.up_pkts + d.down_pkts == 0 {
            continue;
        }
        flows.push(json!({
            "addr": addr.to_string(),
            "up_bytes_per_s": r2(d.up_bytes as f64 / wall), "down_bytes_per_s": r2(d.down_bytes as f64 / wall),
            "up_wire_bytes_per_s": r2((d.up_bytes + HDR * d.up_pkts) as f64 / wall),
            "down_wire_bytes_per_s": r2((d.down_bytes + HDR * d.down_pkts) as f64 / wall),
            "up_pps": r2(d.up_pkts as f64 / wall), "down_pps": r2(d.down_pkts as f64 / wall),
            "up_avg_payload": r2(d.up_bytes as f64 / d.up_pkts.max(1) as f64),
            "down_avg_payload": r2(d.down_bytes as f64 / d.down_pkts.max(1) as f64),
        }));
    }
    let favg = |k: &str| r2(flows.iter().filter_map(|f| f[k].as_f64()).sum::<f64>() / n as f64);
    let fmax = |k: &str| r2(flows.iter().filter_map(|f| f[k].as_f64()).fold(0.0, f64::max));

    let synth: Vec<J> = (0..n)
        .map(|i| read_jsonl(&work.join(format!("c{i}")).join("synth.jsonl")).into_iter().rev().find(|e| e["ev"] == "synth_stats").unwrap_or(J::Null))
        .collect();
    let wavg = |k: &str, f: &str| {
        let x: Vec<f64> = synth.iter().filter_map(|s| s["write"][k][f].as_f64()).collect();
        if x.is_empty() { J::Null } else { json!(r2(x.iter().sum::<f64>() / x.len() as f64 * 1000.0) / 1000.0) }
    };
    let wmax = |k: &str, f: &str| {
        let x: Vec<f64> = synth.iter().filter_map(|s| s["write"][k][f].as_f64()).collect();
        x.iter().copied().fold(None, |m: Option<f64>, v| Some(m.map_or(v, |m| m.max(v)))).map_or(J::Null, |v| json!(v))
    };
    let mut writes = serde_json::Map::new();
    for k in ["local_root", "local_weapon", "local_pose", "vitals_encode", "vitals", "doorbell", "sample_total"] {
        writes.insert(k.into(), json!({"avg_us": wavg(k, "avg_us"), "p99_us": wavg(k, "p99_us"), "max_p99_us": wmax(k, "p99_us")}));
    }
    // the perf reports that fall inside the window (5 s cadence; skip the first after start)
    let perf = perf_lines(&work.join("server.log"));
    let in_win = ((a.secs / 5.0).floor() as usize).max(1);
    let perf_win: Vec<J> = perf.iter().rev().take(in_win).cloned().collect();
    let pavg = |k: &str| {
        let x: Vec<f64> = perf_win.iter().filter_map(|p| p[k].as_f64()).collect();
        if x.is_empty() { J::Null } else { json!(r2(x.iter().sum::<f64>() / x.len() as f64)) }
    };
    let opt = |x: Option<f64>| x.map_or(J::Null, |v| json!(r2(v)));
    let report = json!({
        "ev": "net_bench",
        "clients": n, "secs": r2(wall), "warmup": a.warmup, "hz": a.hz, "vitals_hz": a.vitals_hz, "tick_hz": a.tick_hz,
        "connect_s": r2(connect_s),
        "git": git_rev(),
        "cpus": std::thread::available_parallelism().map(|p| p.get()).unwrap_or(0),
        "system_idle_pct_before": opt(idle_before), "system_idle_pct_during": opt(idle_during),
        "server_cpu_pct": opt(server_cpu), "server_cpu_pct_per_player": opt(server_cpu.map(|c| c / n as f64)),
        "sidecar_cpu_pct": {"avg": opt(avg(&sidecar_cpu)), "max": opt(max(&sidecar_cpu)), "each": sidecar_cpu.iter().map(|c| opt(*c)).collect::<Vec<_>>()},
        "game_cpu_pct": {"avg": opt(avg(&game_cpu)), "max": opt(max(&game_cpu)), "each": game_cpu.iter().map(|c| opt(*c)).collect::<Vec<_>>()},
        "bench_cpu_pct": opt(bench_cpu),
        "up_bytes_per_s_per_client": favg("up_bytes_per_s"), "down_bytes_per_s_per_client": favg("down_bytes_per_s"),
        "up_wire_bytes_per_s_per_client": favg("up_wire_bytes_per_s"), "down_wire_bytes_per_s_per_client": favg("down_wire_bytes_per_s"),
        "up_pps_per_client": favg("up_pps"), "down_pps_per_client": favg("down_pps"),
        "down_bytes_per_s_max_client": fmax("down_bytes_per_s"),
        "flows": flows,
        "server_perf": {"avg": {"tick_us_p50": pavg("tick_us_p50"), "tick_us_p99": pavg("tick_us_p99"), "handle_us_p50": pavg("handle_us_p50"),
                                "handle_us_p99": pavg("handle_us_p99"), "in_pps": pavg("in_pps"), "out_pps": pavg("out_pps"), "out_kbps": pavg("out_kbps"),
                                "thinned_per_s": pavg("thinned_per_s"), "budget_drop_per_s": pavg("budget_drop_per_s")},
                        "reports": perf_win},
        "game_writes": writes,
        "synth_stats": synth,
    });
    print_summary(&report);
    println!("{report}");
    if let Some(p) = &a.json {
        if let Some(d) = p.parent().filter(|d| !d.as_os_str().is_empty()) {
            std::fs::create_dir_all(d)?;
        }
        std::fs::write(p, serde_json::to_string_pretty(&report)? + "\n").with_context(|| format!("write {}", p.display()))?;
    }
    Ok(0)
}

fn git_rev() -> J {
    let out = Command::new("git").args(["rev-parse", "--short", "HEAD"]).stderr(Stdio::null()).output();
    match out {
        Ok(o) if o.status.success() => json!(String::from_utf8_lossy(&o.stdout).trim().to_string()),
        _ => J::Null,
    }
}

fn print_summary(r: &J) {
    let f = |v: &J| v.as_f64().map_or("-".to_string(), |x| format!("{x:.2}"));
    println!("net-bench: {} clients, {} s, {} Hz pose, server tick {} Hz", r["clients"], f(&r["secs"]), r["hz"], r["tick_hz"]);
    println!("  system idle %   before {}  during {}  ({} logical CPUs)", f(&r["system_idle_pct_before"]), f(&r["system_idle_pct_during"]), r["cpus"]);
    println!("  server CPU %    {}  ({} per player)", f(&r["server_cpu_pct"]), f(&r["server_cpu_pct_per_player"]));
    println!("  sidecar CPU %   avg {}  max {}", f(&r["sidecar_cpu_pct"]["avg"]), f(&r["sidecar_cpu_pct"]["max"]));
    println!("  fake game CPU % avg {}   bench (proxy) CPU % {}", f(&r["game_cpu_pct"]["avg"]), f(&r["bench_cpu_pct"]));
    println!(
        "  per client up   {} B/s payload, {} B/s wire, {} pps",
        f(&r["up_bytes_per_s_per_client"]),
        f(&r["up_wire_bytes_per_s_per_client"]),
        f(&r["up_pps_per_client"])
    );
    println!(
        "  per client down {} B/s payload, {} B/s wire, {} pps",
        f(&r["down_bytes_per_s_per_client"]),
        f(&r["down_wire_bytes_per_s_per_client"]),
        f(&r["down_pps_per_client"])
    );
    let p = &r["server_perf"]["avg"];
    println!(
        "  server perf     tick p50/p99 {}/{} us, pkt {}/{} us, thinned {}/s, budget drops {}/s",
        f(&p["tick_us_p50"]),
        f(&p["tick_us_p99"]),
        f(&p["handle_us_p50"]),
        f(&p["handle_us_p99"]),
        f(&p["thinned_per_s"]),
        f(&p["budget_drop_per_s"])
    );
    let w = &r["game_writes"];
    println!(
        "  game slot write avg/p99 us: root {}/{}  weapon {}/{}  pose {}/{}  vitals {}/{} (+encode {}/{})  doorbell {}/{}",
        f(&w["local_root"]["avg_us"]),
        f(&w["local_root"]["p99_us"]),
        f(&w["local_weapon"]["avg_us"]),
        f(&w["local_weapon"]["p99_us"]),
        f(&w["local_pose"]["avg_us"]),
        f(&w["local_pose"]["p99_us"]),
        f(&w["vitals"]["avg_us"]),
        f(&w["vitals"]["p99_us"]),
        f(&w["vitals_encode"]["avg_us"]),
        f(&w["vitals_encode"]["p99_us"]),
        f(&w["doorbell"]["avg_us"]),
        f(&w["doorbell"]["p99_us"])
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn proxy_counts_bytes_both_ways() {
        // a loopback echo server behind the proxy
        let echo = UdpSocket::bind("127.0.0.1:0").unwrap();
        echo.set_read_timeout(Some(Duration::from_millis(100))).unwrap();
        let eaddr = echo.local_addr().unwrap();
        let stop = Arc::new(AtomicBool::new(false));
        let st = stop.clone();
        let eh = std::thread::spawn(move || {
            let mut b = [0u8; 2048];
            while !st.load(Ordering::Relaxed) {
                if let Ok((n, from)) = echo.recv_from(&mut b) {
                    // reply twice as long (distinct down count)
                    let mut r = b[..n].to_vec();
                    r.extend_from_slice(&b[..n]);
                    let _ = echo.send_to(&r, from);
                }
            }
        });
        let proxy = Proxy::start(eaddr).unwrap();
        let (c1, c2) = (UdpSocket::bind("127.0.0.1:0").unwrap(), UdpSocket::bind("127.0.0.1:0").unwrap());
        for c in [&c1, &c2] {
            c.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
        }
        let mut b = [0u8; 4096];
        for i in 0..10 {
            c1.send_to(&vec![7u8; 100 + i], proxy.addr).unwrap();
            let n = c1.recv(&mut b).unwrap();
            assert_eq!(n, 2 * (100 + i));
        }
        for _ in 0..3 {
            c2.send_to(&[1u8; 50], proxy.addr).unwrap();
            assert_eq!(c2.recv(&mut b).unwrap(), 100);
        }
        // counters are bumped after the forward: give the threads a moment
        std::thread::sleep(Duration::from_millis(100));
        let s = proxy.snap();
        assert_eq!(s.len(), 2);
        let up1: u64 = (0..10).map(|i| 100 + i as u64).sum();
        assert_eq!(s[0].1, Snap { up_bytes: up1, up_pkts: 10, down_bytes: 2 * up1, down_pkts: 10 });
        assert_eq!(s[1].1, Snap { up_bytes: 150, up_pkts: 3, down_bytes: 300, down_pkts: 3 });
        drop(proxy);
        stop.store(true, Ordering::Relaxed);
        eh.join().unwrap();
    }

    #[test]
    fn perf_line_parses() {
        let d = hsmp_tools::paths::make_temp_dir("hsmp-nb-test-").unwrap();
        let p = d.join("s.log");
        std::fs::write(&p, "2026 \u{1b}[32m INFO\u{1b}[0m hsmp_server::perf: perf ticks/5s=150 tick_us p50=12 p99=80 max=95 | pkt_handle_us p50=3 p99=9 | in=960pps out=2880pps 700KB/s | thinned=0/s budget_drop=4/s\n").unwrap();
        let v = perf_lines(&p);
        assert_eq!(v.len(), 1);
        assert_eq!(v[0]["out_pps"], 2880);
        assert_eq!(v[0]["budget_drop_per_s"], 4);
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn idle_math() {
        assert_eq!(idle_pct(Some((0, 0, 0)), Some((50, 80, 20))), Some(50.0));
        assert_eq!(idle_pct(None, Some((1, 1, 1))), None);
    }
}
