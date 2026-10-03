//! Shared harness for the sidecar integration tests: this test process plays the game. It
//! creates and initialises a named HSMP-SHM segment with `hsmp-ipc` and spawns the real
//! `hsmp-sidecar --parent-pid <us> --ipc shm:<name>`. Windows only (named mappings).
#![allow(dead_code)]

use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use hsmp_ipc::handshake::{init_game, SideParams};
use hsmp_ipc::shm::{self, Mapping};
use hsmp_ipc::Segment;

/// Every capability the game can offer.
pub const ALL_CAPS: u64 = (1 << 13) - 1;

pub struct Game {
    pub map: Mapping,
    pub name: String,
    pub epoch: u64,
    pub dir: PathBuf,
}

impl Game {
    pub fn new(tag: &str) -> Game {
        let pid = shm::current_pid();
        let ct = shm::process_create_time(pid).unwrap();
        let name = format!("{}.test{}{:x}", shm::mapping_name(pid, ct), tag, shm::random_u64());
        let (map, existed) = Mapping::create(&name).unwrap();
        assert!(!existed);
        let epoch = shm::random_u64();
        let p = SideParams { pid, create_time: ct, epoch, caps: ALL_CAPS, build_id: "sidecar test game" };
        unsafe { init_game(map.segment_ptr(), map.len(), &p, shm::qpc_freq()) }.unwrap();
        let dir = std::env::temp_dir().join(format!("hsmp_sidecar_test_{}_{}", tag, pid));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        Game { map, name, epoch, dir }
    }

    pub fn seg(&self) -> &Segment {
        self.map.segment().unwrap()
    }

    /// The sidecar's command line for this game (state dir, parent, link, tap).
    pub fn sidecar_args(&self, server: &str, nick: &str, parent_pid: u32) -> Vec<String> {
        vec![
            "--server".into(), server.into(), "--state-dir".into(), self.dir.display().to_string(),
            "--nick".into(), nick.into(), "--parent-pid".into(), parent_pid.to_string(),
            "--ipc".into(), format!("shm:{}", self.name),
            "--ipc-tap".into(), self.dir.join("ipc_tap.jsonl").display().to_string(),
        ]
    }

    /// Spawn the sidecar with piped stderr (for exit-code tests).
    pub fn spawn(&self, parent_pid: u32) -> Child {
        self.spawn_env(parent_pid, &[])
    }

    pub fn spawn_env(&self, parent_pid: u32, env: &[(&str, &str)]) -> Child {
        let mut c = Command::new(env!("CARGO_BIN_EXE_hsmp-sidecar"));
        c.args(self.sidecar_args("127.0.0.1:9", "Willie", parent_pid))
            .env("HSMP_CAREER_GUARD", "0")
            .env("HSMP_STATE_DIR", &self.dir)
            .env("HSMP_IDENTITY_DIR", &self.dir)
            .stdout(Stdio::null())
            .stderr(Stdio::piped());
        for (k, v) in env {
            c.env(k, v);
        }
        c.spawn().expect("spawn hsmp-sidecar")
    }

    /// A sidecar-written record slot (`session`, `link`, `admin`): (meta, kind, payload).
    #[allow(dead_code)]
    pub fn slot(&self, name: &str) -> Option<(hsmp_ipc::schema::SlotMeta, u16, Vec<u8>)> {
        let (mut scratch, mut out) = (Vec::new(), Vec::new());
        let (m, k, _) = self.seg().slot_ref(name, 0)?.get(&mut scratch, &mut out)?;
        Some((m, k, out))
    }

    /// The sidecar's current `link` record.
    #[allow(dead_code)]
    pub fn link(&self) -> Option<hsmp_ipc::schema::session::Link> {
        let (_, _, p) = self.slot("link")?;
        hsmp_ipc::record::view::<hsmp_ipc::schema::session::Link>(&p).ok().map(|v| v.head())
    }

    /// Wait until the link record says connected as `peer_id`.
    #[allow(dead_code)]
    pub fn wait_connected(&self, peer_id: u32, secs: u64) -> bool {
        use hsmp_ipc::schema::session::sidecar_status;
        until(secs, || self.link().is_some_and(|l| l.status == sidecar_status::CONNECTED && l.my_peer_id == peer_id))
    }
}

pub fn wait_exit(child: &mut Child, secs: u64) -> (Option<i32>, String) {
    let t0 = Instant::now();
    let st = loop {
        if let Some(s) = child.try_wait().unwrap() {
            break s;
        }
        if t0.elapsed() > Duration::from_secs(secs) {
            let _ = child.kill(); // our own child, by its handle (PID)
            let _ = child.wait();
            panic!("sidecar still running after {secs} s");
        }
        std::thread::sleep(Duration::from_millis(20));
    };
    let mut err = String::new();
    use std::io::Read;
    let _ = child.stderr.take().unwrap().read_to_string(&mut err);
    (st.code(), err)
}

pub fn until(secs: u64, mut f: impl FnMut() -> bool) -> bool {
    let t0 = Instant::now();
    while t0.elapsed() < Duration::from_secs(secs) {
        if f() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    false
}

pub fn tap_events(dir: &Path) -> Vec<serde_json::Value> {
    std::fs::read_to_string(dir.join("ipc_tap.jsonl"))
        .unwrap_or_default()
        .lines()
        .filter_map(|l| serde_json::from_str(l).ok())
        .collect()
}

/// A child process killed (by its own handle, i.e. its PID) when dropped.
pub struct Proc(pub Child);
impl Drop for Proc {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

pub fn spawn_logged(bin: &str, args: &[String], dir: &Path, log: &Path) -> Proc {
    let log = std::fs::File::create(log).unwrap();
    Proc(
        Command::new(bin)
            .args(args)
            .env("HSMP_STATE_DIR", dir)
            .env("HSMP_IDENTITY_DIR", dir)
            .env("HSMP_CAREER_GUARD", "0")
            .env("RUST_LOG", "info")
            .current_dir(dir)
            .stdout(log.try_clone().unwrap())
            .stderr(log)
            .spawn()
            .unwrap_or_else(|e| panic!("spawn {bin}: {e}")),
    )
}

pub fn free_port() -> u16 {
    std::net::UdpSocket::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port()
}

/// A real hsmp-server on a free localhost port; returns (process, "127.0.0.1:port").
pub fn server(dir: &Path) -> (Proc, String) {
    let addr = format!("127.0.0.1:{}", free_port());
    let p = spawn_logged(
        env!("CARGO_BIN_EXE_hsmp-server"),
        &["--bind".into(), addr.clone(), "--tick-hz".into(), "30".into(), "--max-peers".into(), "4".into()],
        dir,
        &dir.join("server.log"),
    );
    std::thread::sleep(Duration::from_millis(300));
    (p, addr)
}

/// Every file in `dir` whose name looks like a legacy IPC contract (none may exist).
pub fn ipc_files(dir: &Path) -> Vec<String> {
    const IPC: &[&str] = &[
        ".sidecar.json", ".match.json", ".session.json", ".link.json", ".metrics.json", ".spawns.json", ".admin.json",
        ".kicked.json", ".reject.json", ".kit_rules.json", ".pose_play", "me_", ".deaths.log", ".notices.jsonl",
        ".cmd_results.jsonl", ".chat.log", ".damage_in.jsonl", ".hitfx_in.jsonl", ".combat_feedback.jsonl",
        ".interact_in.jsonl", ".vitals_remote", ".kit_remote", ".loadout_remote", ".world_", ".sidecar_panic.txt",
    ];
    std::fs::read_dir(dir)
        .map(|rd| {
            rd.flatten()
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .filter(|n| IPC.iter().any(|p| n.starts_with(p)))
                .collect()
        })
        .unwrap_or_default()
}
