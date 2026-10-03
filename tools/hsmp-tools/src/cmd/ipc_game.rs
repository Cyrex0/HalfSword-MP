//! Game-role tools for tests (e2e, gate) without Half Sword:
//!
//! `hsmp-tools ipc-game [opts] -- <sidecar.exe> <args...>`: a game process stand-in. Creates
//! the segment for its own PID (role = game), spawns the sidecar with
//! `--parent-pid <own pid> --ipc shm:<name>` (+ `--ipc-tap`), pumps the game side every ms
//! (heartbeat, S2G drain, state blobs, peer slots) and appends what the game sees to
//! `--view` (JSON lines: attach, refuse, slot (session / link / admin records), state, peer_dir, peer_root, peer_play
//! (at most 5 Hz per peer), peer_vitals, peer_kit, s2g). Exits with the sidecar's exit code,
//! on --stop-file, or when --parent-pid dies (the sidecar then sees its parent die too).
//!
//! `--synth` (net-bench): once the session is connected, play a live match into the game
//! slots (hsmp_tools::synth: 60 Hz root / weapon / pose records, 20 Hz vitals) and
//! report the slot-write cost as `{"ev":"synth_stats"}` at exit.
//!
//! `hsmp-tools ipc-put --name <mapping> <name> <json|@file>`: write one game-written record
//! slot (schema SLOTS), G2S record or `dev_cmd` from the record's Lua-table shape
//! (`hsmp_ipc::debug_json::json_to_record`: Rust field names, `rows` for variable records),
//! or a legacy slot / blob / G2S message in the legacy file's JSON shape (names:
//! mods/shared/hsmp_ipc_schema.lua). Prints the req_id for ring records.
//!
//! `--view` also logs every sidecar-written record slot on change (`{"ev":"slot",...}`) and
//! renders S2G / DevCtl records generically (`hsmp_tools::ipcgame`).

use anyhow::{bail, Context, Result};
use hsmp_ipc::shm::Access;
use hsmp_tools::ipcgame as ig;
use serde_json::{json, Value as J};
use std::io::Write;
use std::path::PathBuf;
use std::time::{Duration, Instant};

pub fn json_arg(s: &str) -> Result<J> {
    let text = match s.strip_prefix('@') {
        Some(p) => std::fs::read_to_string(p).with_context(|| format!("read {p}"))?,
        None => s.to_string(),
    };
    serde_json::from_str(text.trim_start_matches('\u{feff}')).context("bad JSON")
}

#[derive(clap::Args)]
pub struct PutArgs {
    #[arg(long)]
    pub name: Option<String>,
    #[arg(long)]
    pub pid: Option<u32>,
    pub kind: String,
    /// The value as JSON (or @file).
    pub value: String,
}

pub fn run_put(a: PutArgs) -> Result<i32> {
    let m = super::ipc_dump::open(a.pid, a.name.as_deref(), Access::ReadWrite)?;
    let s = ig::segment(&m)?;
    let v = json_arg(&a.value)?;
    let id = ig::put_json(s, &a.kind, &v)?;
    let (g, _) = hsmp_ipc::shm::doorbell_names(m.name());
    if let Ok(b) = hsmp_ipc::shm::Doorbell::open(&g) {
        b.ring();
    }
    if let Some(id) = id {
        println!("{id}");
    }
    Ok(0)
}

#[derive(clap::Args)]
pub struct GameArgs {
    /// Capability bits the fake game offers (hex or "all").
    #[arg(long, default_value = "all")]
    pub caps: String,
    /// Write the mapping name here once the segment exists.
    #[arg(long)]
    pub name_file: Option<PathBuf>,
    /// Append what the game side observes here (JSON lines).
    #[arg(long)]
    pub view: Option<PathBuf>,
    /// Pass `--ipc-tap <path>` to the sidecar.
    #[arg(long)]
    pub tap: Option<PathBuf>,
    /// Exit when this process dies.
    #[arg(long)]
    pub parent_pid: Option<u32>,
    /// Exit when this file appears.
    #[arg(long)]
    pub stop_file: Option<PathBuf>,
    /// Do not spawn the sidecar; only host the segment.
    #[arg(long)]
    pub no_sidecar: bool,
    /// Mirror the game side into the legacy state files in this dir and forward the files a
    /// test writes (hsmp_tools::ipcbridge; the e2e suite's file-shaped assertions).
    #[arg(long)]
    pub bridge: Option<PathBuf>,
    /// Play a live match (hsmp_tools::synth): once the session is connected, write
    /// root / weapon / pose records at --synth-hz and the vitals slot at
    /// --synth-vitals-hz; print {"ev":"synth_connected"} and, at exit, {"ev":"synth_stats"}.
    #[arg(long)]
    pub synth: bool,
    #[arg(long, default_value_t = 60.0)]
    pub synth_hz: f64,
    #[arg(long, default_value_t = 20.0)]
    pub synth_vitals_hz: f64,
    #[arg(long, default_value_t = 1)]
    pub synth_seed: u64,
    /// Write the synth JSON lines here (default: stdout).
    #[arg(long)]
    pub synth_out: Option<PathBuf>,
    /// The sidecar executable and its arguments (after `--`).
    #[arg(last = true)]
    pub sidecar: Vec<String>,
}

pub fn caps_arg(s: &str) -> Result<u64> {
    if s == "all" {
        return Ok(u64::MAX >> 1);
    }
    u64::from_str_radix(s.trim_start_matches("0x"), 16).context("--caps: hex or all")
}

pub fn run_game(a: GameArgs) -> Result<i32> {
    let caps = caps_arg(&a.caps)?;
    let mut host = ig::GameHost::create(None, caps)?;
    if let Some(p) = &a.name_file {
        std::fs::write(p, host.name())?;
    }
    let mut view = match &a.view {
        Some(p) => Some(std::fs::OpenOptions::new().create(true).append(true).open(p).with_context(|| format!("open {}", p.display()))?),
        None => None,
    };
    let mut child = if a.no_sidecar {
        None
    } else {
        let Some((exe, rest)) = a.sidecar.split_first() else { bail!("ipc-game: pass `-- <sidecar.exe> <args>` or --no-sidecar") };
        let mut c = std::process::Command::new(exe);
        c.args(rest).args(host.sidecar_args());
        if let Some(t) = &a.tap {
            c.arg("--ipc-tap").arg(t);
        }
        Some(c.spawn().with_context(|| format!("spawn {exe}"))?)
    };
    eprintln!("ipc-game: pid {} segment {}", host.pid, host.name());
    let mut last_check = Instant::now();
    let mut bridge = a.bridge.as_deref().map(hsmp_tools::ipcbridge::Bridge::new);
    let mut synth = a.synth.then(|| SynthRun::new(&a, child.as_ref().map(|c| c.id())));
    loop {
        let evs = host.pump();
        if let Some(b) = bridge.as_mut() {
            b.on_events(&evs);
            b.poll_files(&host);
        }
        if let Some(s) = synth.as_mut() {
            s.on_events(&evs, &host);
            s.synth.step(&host);
        }
        for mut e in evs {
            if let Some(f) = view.as_mut() {
                e["t"] = json!(std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis() as u64));
                let _ = writeln!(f, "{e}");
            }
        }
        if let Some(c) = child.as_mut() {
            if let Ok(Some(st)) = c.try_wait() {
                // What the sidecar published last (its final `link` record, status ENDED, is
                // written just before it exits): one more frame so the view has it.
                for mut e in host.pump() {
                    if let Some(f) = view.as_mut() {
                        e["t"] = json!(std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis() as u64));
                        let _ = writeln!(f, "{e}");
                    }
                }
                eprintln!("ipc-game: sidecar exited: {st}");
                if let Some(s) = synth.as_mut() {
                    s.finish();
                }
                return Ok(st.code().unwrap_or(1));
            }
        }
        if last_check.elapsed() >= Duration::from_millis(250) {
            last_check = Instant::now();
            if a.stop_file.as_ref().is_some_and(|p| p.exists()) || a.parent_pid.is_some_and(|p| !pid_alive(p)) {
                if let Some(s) = synth.as_mut() {
                    s.finish();
                }
                // Like a game that quits: the sidecar notices through --parent-pid.
                return Ok(0);
            }
        }
        std::thread::sleep(Duration::from_millis(1));
    }
}

/// `--synth`: the synthetic match player plus its JSON-lines output.
struct SynthRun {
    synth: hsmp_tools::synth::Synth,
    out: Box<dyn Write>,
    sidecar_pid: Option<u32>,
    connected: bool,
}

impl SynthRun {
    fn new(a: &GameArgs, sidecar_pid: Option<u32>) -> SynthRun {
        // A game runs a high-resolution frame loop; without this a 1 ms sleep is ~15.6 ms
        // on Windows and the 60 Hz schedule would stutter.
        // SAFETY: plain winmm call, no pointers.
        #[cfg(windows)]
        unsafe {
            windows_sys::Win32::Media::timeBeginPeriod(1);
        }
        let out: Box<dyn Write> = match a.synth_out.as_ref().and_then(|p| std::fs::OpenOptions::new().create(true).append(true).open(p).ok()) {
            Some(f) => Box::new(f),
            None => Box::new(std::io::stdout()),
        };
        SynthRun { synth: hsmp_tools::synth::Synth::new(a.synth_hz, a.synth_vitals_hz, a.synth_seed), out, sidecar_pid, connected: false }
    }

    fn emit(&mut self, j: &J) {
        let _ = writeln!(self.out, "{j}");
        let _ = self.out.flush();
    }

    fn on_events(&mut self, evs: &[J], host: &ig::GameHost) {
        if self.connected {
            return;
        }
        for e in evs {
            // The sidecar's `link` record says CONNECTED.
            if e["ev"] == "slot" && e["slot"] == "link" && ig::link_connected(&e["v"]) {
                self.connected = true;
                self.synth.start();
                let j = json!({"ev": "synth_connected", "game_pid": host.pid, "sidecar_pid": self.sidecar_pid,
                               "peer_id": e["v"]["my_peer_id"], "seed": self.synth.seed});
                self.emit(&j);
                return;
            }
        }
    }

    fn finish(&mut self) {
        let mut j = self.synth.stats_json();
        j["game_pid"] = json!(std::process::id());
        j["sidecar_pid"] = json!(self.sidecar_pid);
        self.emit(&j);
    }
}

#[cfg(windows)]
fn pid_alive(pid: u32) -> bool {
    hsmp_ipc::shm::ProcessHandle::open(pid).map(|h| h.is_alive()).unwrap_or(false)
}
#[cfg(not(windows))]
fn pid_alive(_pid: u32) -> bool {
    true
}
