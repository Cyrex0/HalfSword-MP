//! `hsmp-tools pose-probe [--bins DIR] [--profile P] [--secs S] [--hz H]`:
//! measure the character-replication pipeline end to end, without the game.
//!
//! Starts hsmp-server + two sidecars (optionally behind `hsmp-tools netsim`). This process
//! is BOTH games: it creates one shared-memory segment per sidecar (role=game,
//! hsmp_tools::ipcgame::GameHost) and starts each sidecar with `--parent-pid <own pid>
//! --ipc shm:<name>`. It publishes sender A's root / pose records at 60 Hz exactly as
//! HSMPSync does (swinging right hand + weapon) and reads receiver B's PeerPlay slot for
//! A at ~240 Hz as HSMPAvatars does. Because both ends share this process's clock, it
//! reports true numbers:
//!
//!   display latency = now - pt     (sample taken -> pose shown, excl. game physics)
//!   error           = |hand_r/weapon_r(pt) - truth(pt)|  (interpolation accuracy)
//!   mode mix, torn (seqlock busy) / missing reads, playback-clock step smoothness.

use anyhow::{bail, Context, Result};
use hsmp_ipc::schema::pose::{PeerPlay, NB};
use hsmp_ipc::schema::CAP_POSE;
use hsmp_ipc::seqlock::ReadError;
use hsmp_ipc::shm;
use hsmp_tools::ipcgame::{play_mode, GameHost, POSE_BONES};
use hsmp_tools::paths;
use hsmp_tools::pyfmt::fixed;
use regex::Regex;
use serde_json::json;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

#[derive(clap::Args)]
pub struct Args {
    /// Dir with hsmp-server / hsmp-sidecar (default: $CARGO_TARGET_DIR/release, else <repo>/target/release)
    #[arg(long)]
    bins: Option<PathBuf>,
    /// netsim profile (lan/good/typical/wifi/bad/awful); default: direct
    #[arg(long)]
    profile: Option<String>,
    #[arg(long, default_value_t = 20.0)]
    secs: f64,
    #[arg(long, default_value_t = 60.0)]
    hz: f64,
}

type V3 = (f64, f64, f64);

/// World pose at sender ms ts: walk +X at 300 uu/s, hand on a 60 cm circle at
/// 6 rad/s, weapon 50 uu beyond the hand.
fn truth(ts: f64) -> (V3, V3, V3, (f64, f64, f64, f64)) {
    let t = ts / 1000.0;
    let px = 300.0 * t;
    let (c, s) = ((6.0 * t).cos(), (6.0 * t).sin());
    let hand = (px + 60.0 * c, 60.0 * s, 140.0);
    let wpn = (px + 110.0 * c, 110.0 * s, 140.0);
    let half = 3.0 * t;
    ((px, 0.0, 100.0), hand, wpn, (0.0, 0.0, half.sin(), half.cos()))
}


fn free_port() -> Result<u16> {
    let s = std::net::UdpSocket::bind("127.0.0.1:0")?;
    Ok(s.local_addr()?.port())
}

fn pct(v: &[f64], p: f64) -> f64 {
    if v.is_empty() {
        return f64::NAN;
    }
    let mut v = v.to_vec();
    v.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let i = ((p / 100.0 * v.len() as f64).ceil() as i64 - 1).clamp(0, v.len() as i64 - 1);
    v[i as usize]
}

/// Python `f"{x:.{n}f}"` incl. nan.
fn f(x: f64, n: usize) -> String {
    if x.is_nan() {
        "nan".into()
    } else {
        fixed(x, n)
    }
}

fn maxv(v: &[f64]) -> f64 {
    // Python: max(v or [0])
    if v.is_empty() {
        0.0
    } else {
        v.iter().cloned().fold(f64::NEG_INFINITY, f64::max)
    }
}

fn exe(bins: &Path, n: &str) -> PathBuf {
    bins.join(if cfg!(windows) { format!("{n}.exe") } else { n.to_string() })
}

struct Procs {
    kids: Vec<Child>,
    logs: Vec<(String, PathBuf)>,
}

impl Procs {
    fn spawn(&mut self, work: &Path, cmd: &mut Command, name: &str) -> Result<()> {
        let lp = work.join(format!("{name}.log"));
        let lf = std::fs::File::create(&lp)?;
        let child = cmd
            .stdout(Stdio::from(lf.try_clone()?))
            .stderr(Stdio::from(lf))
            .spawn()
            .with_context(|| format!("spawn {name}"))?;
        self.logs.push((name.to_string(), lp));
        self.kids.push(child);
        Ok(())
    }
}

impl Drop for Procs {
    fn drop(&mut self) {
        // our own children only
        for c in &mut self.kids {
            let _ = c.kill();
        }
        for c in &mut self.kids {
            let _ = c.wait();
        }
    }
}


/// The flat 23 x 13 bone array of one sample: every body at the pelvis with identity
/// rotation except hand_r (the swinging hand), as a real skeleton would never be but the
/// codec carries every translation it is given.
fn bones(pel: V3, hand: V3, q: (f64, f64, f64, f64)) -> Vec<f64> {
    let hand_i = POSE_BONES.iter().position(|n| *n == "hand_r").unwrap_or(16);
    let mut b = Vec::with_capacity(NB * 13);
    for i in 0..NB {
        let (p, r) = if i == hand_i { (hand, q) } else { (pel, (0.0, 0.0, 0.0, 1.0)) };
        b.extend_from_slice(&[p.0, p.1, p.2, r.0, r.1, r.2, r.3, 300.0, 0.0, 0.0, 0.0, 0.0, 0.0]);
    }
    b
}

pub fn run(a: Args) -> Result<i32> {
    let root = paths::repo_root()?;
    let bins = a.bins.clone().unwrap_or_else(|| std::env::var_os("CARGO_TARGET_DIR").map(PathBuf::from).unwrap_or_else(|| root.join("target")).join("release"));
    let t0 = Instant::now();
    let now_ms = || t0.elapsed().as_secs_f64() * 1000.0;

    let work = paths::make_temp_dir("hsmp-pose-probe-")?;
    let (da, db) = (work.join("A"), work.join("B"));
    std::fs::create_dir_all(&da)?;
    std::fs::create_dir_all(&db)?;
    // One segment per "game" (both in this process: distinct names, same parent pid).
    let pid = shm::current_pid();
    let base = shm::mapping_name(pid, shm::process_create_time(pid).unwrap_or(1));
    let nonce = shm::random_u64();
    let mut host_a = GameHost::create(Some(&format!("{base}.probeA{nonce:x}")), u64::MAX >> 1)?;
    let mut host_b = GameHost::create(Some(&format!("{base}.probeB{nonce:x}")), u64::MAX >> 1)?;
    let sport = free_port()?;
    let mut procs = Procs { kids: vec![], logs: vec![] };
    procs.spawn(
        &work,
        Command::new(exe(&bins, "hsmp-server")).args(["--bind", &format!("127.0.0.1:{sport}"), "--tick-hz", "30", "--max-peers", "4"]),
        "server",
    )?;
    let mut target = format!("127.0.0.1:{sport}");
    if let Some(p) = &a.profile {
        let nport = free_port()?;
        procs.spawn(
            &work,
            Command::new(std::env::current_exe()?).args([
                "netsim", "--listen", &format!("127.0.0.1:{nport}"), "--upstream", &target, "--profile", p, "--seed", "7",
            ]),
            "netsim",
        )?;
        target = format!("127.0.0.1:{nport}");
    }
    std::thread::sleep(Duration::from_millis(500));
    let side = |dir: &Path, nick: &str, host: &GameHost| {
        let mut c = Command::new(exe(&bins, "hsmp-sidecar"));
        c.args(["--server", &target, "--state-dir"])
            .arg(dir)
            .args(["--nick", nick])
            .args(host.sidecar_args())
            // its own identity key (player_key lives in HSMP_STATE_DIR): two sidecars with one
            // key would replace each other on the server
            .env("HSMP_STATE_DIR", dir)
            .env("RUST_LOG", "hsmp_sidecar=info");
        c
    };
    procs.spawn(&work, &mut side(&da, "A", &host_a), "sidecarA")?;
    std::thread::sleep(Duration::from_millis(300)); // A joins first => peer 1
    procs.spawn(&work, &mut side(&db, "B", &host_b), "sidecarB")?;
    // Both sidecars attached with POSE, and B sees A in its peer directory.
    let deadline = Instant::now() + Duration::from_secs(10);
    let slot = loop {
        host_a.pump();
        host_b.pump();
        if host_a.has(CAP_POSE) && host_b.has(CAP_POSE) {
            if let Ok((dir, _)) = host_b.seg().peers.dir.read() {
                if let Some(s) = dir.slot_of(1) {
                    break s;
                }
            }
        }
        if Instant::now() > deadline {
            bail!("the sidecars did not attach with POSE, or B never saw peer 1 (logs in {})", work.display());
        }
        std::thread::sleep(Duration::from_millis(20));
    };
    let weapon_i = NB; // PeerPlay slot weapon_r
    let hand_i = POSE_BONES.iter().position(|n| *n == "hand_r").context("hand_r")?;

    let period = 1000.0 / a.hz;
    let mut next_send = now_ms();
    let mut seq: u64 = 0;
    let (mut lat, mut err_h, mut err_w, mut steps) = (vec![], vec![], vec![], vec![]);
    let mut modes: BTreeMap<String, u64> = BTreeMap::new();
    let (mut reads, mut torn, mut missing, mut fresh) = (0u64, 0u64, 0u64, 0u64);
    let (mut last_seq, mut last_pt): (u64, Option<f64>) = (0, None);
    let end = now_ms() + a.secs * 1000.0;
    let warm = now_ms() + 3000.0;
    let dist = |p: (f64, f64, f64), q: V3| ((p.0 - q.0).powi(2) + (p.1 - q.1).powi(2) + (p.2 - q.2).powi(2)).sqrt();
    let mut pp: Box<PeerPlay> = hsmp_ipc::layout::boxed_zeroed();
    while now_ms() < end {
        let n = now_ms();
        if n >= next_send {
            next_send = (next_send + period).max(n - period);
            seq += 1;
            let ts = n;
            let (pel, hand, wpn, q) = truth(ts);
            host_a.put("local_root", &json!({"tick": seq, "ts": ts, "pos": [pel.0, pel.1, pel.2], "rot": [0, 0, 0], "vel": [300, 0, 0]}))?;
            let w = [1.0, 1.0, wpn.0, wpn.1, wpn.2, q.0, q.1, q.2, q.3, 300.0, 0.0, 0.0, 0.0, 0.0, 0.0, wpn.0, wpn.1, wpn.2, wpn.0, wpn.1, wpn.2];
            host_a.put("local_pose", &json!({"tick": seq, "ts": ts, "dt": period, "b": bones(pel, hand, q), "w": [w]}))?;
            host_a.pump();
        }
        // Game-frame read (~240 fps), as HSMPAvatars peer_play.
        reads += 1;
        let r = host_b.seg().peers.slots[slot].play.read_if_changed(last_seq, &mut pp);
        match r {
            Ok(None) => {}
            Err(ReadError::Busy) => torn += 1,
            Err(ReadError::Empty) => missing += 1,
            Ok(Some(s)) => {
                last_seq = s;
                fresh += 1;
                let pt = pp.pt;
                if n > warm {
                    *modes.entry(play_mode(pp.mode).to_string()).or_insert(0) += 1;
                    lat.push(n - pt);
                    let (_, th, tw, _) = truth(pt);
                    let at = |i: usize| (pp.b[i][0] as f64, pp.b[i][1] as f64, pp.b[i][2] as f64);
                    if pp.mask & (1 << hand_i) != 0 {
                        err_h.push(dist(at(hand_i), th));
                    }
                    if pp.mask & (1 << weapon_i) != 0 {
                        err_w.push(dist(at(weapon_i), tw));
                    }
                    if let Some(lp) = last_pt {
                        steps.push(pt - lp);
                    }
                }
                last_pt = Some(pt);
            }
        }
        host_b.pump();
        std::thread::sleep(Duration::from_secs_f64(1.0 / 240.0));
    }
    let logs = procs.logs.clone();
    drop(procs); // stop server / sidecars / netsim
    drop(host_a);
    drop(host_b);

    let total: u64 = modes.values().sum::<u64>().max(1);
    println!(
        "profile={}  rate={} Hz  secs={}  workdir={}",
        a.profile.as_deref().unwrap_or("direct (localhost)"),
        fixed(a.hz, 0),
        fixed(a.secs, 0),
        work.display()
    );
    println!(
        "display latency (sample -> shown)  p50 {}  p95 {}  max {} ms",
        f(pct(&lat, 50.0), 1),
        f(pct(&lat, 95.0), 1),
        f(maxv(&lat), 1)
    );
    println!("hand error  p50 {}  p95 {}  max {} uu", f(pct(&err_h, 50.0), 2), f(pct(&err_h, 95.0), 2), f(maxv(&err_h), 2));
    println!("weapon err  p50 {}  p95 {}  max {} uu", f(pct(&err_w, 50.0), 2), f(pct(&err_w, 95.0), 2), f(maxv(&err_w), 2));
    println!(
        "modes {}",
        modes.iter().map(|(k, v)| format!("{k} {}%", f(100.0 * *v as f64 / total as f64, 1))).collect::<Vec<_>>().join("  ")
    );
    println!("reads {reads}  new samples {fresh}  torn (seqlock busy) {torn}  missing {missing}");
    if !steps.is_empty() {
        let mn = steps.iter().cloned().fold(f64::INFINITY, f64::min);
        println!(
            "pt step  p50 {}  p99 {}  min {} ms (negative = playback went backwards)",
            f(pct(&steps, 50.0), 2),
            f(pct(&steps, 99.0), 2),
            f(mn, 2)
        );
    }
    let strip = Regex::new(r"^.*?INFO\s+").unwrap();
    for (name, path) in &logs {
        if name == "sidecarB" {
            let text = std::fs::read(path).map(|b| String::from_utf8_lossy(&b).to_string()).unwrap_or_default();
            let last = text.lines().filter(|l| l.contains("pose peer") || l.contains("latency ms")).last();
            if let Some(l) = last {
                println!("sidecar B last report: {}", strip.replace(l.trim(), ""));
            }
        }
    }
    Ok(0)
}
