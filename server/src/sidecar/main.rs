//! hsmp-sidecar — bridges one game instance with the UDP relay.
//!
//! The game (HSMPNative) creates a shared-memory segment and starts the sidecar with
//! `--parent-pid <game pid> --ipc shm:<mapping name>` (HSMP-SHM,
//! docs/development/ipc-shared-memory.md). The sidecar attaches, reads the local root /
//! weapon / pose slots and the G2S queues, and publishes the session state, the remote
//! peers' playback and the S2G events into the segment. There is no file IPC.
//!
//! Usage:
//!   hsmp-sidecar.exe --server <host:port> --state-dir <path> --nick <name> \
//!                    --parent-pid <pid> --ipc shm:<name>
//!
//! Without `--ipc shm:<name>` the sidecar exits with [`EXIT_NO_IPC`]; a refused segment
//! exits with the refuse code's status (70..78). `--print-player-key` and
//! `--career-recover` need neither. The state dir only holds logs (`.career_guard.jsonl`,
//! `.sidecar_panic.log`), the identity and `.player_key`.

use anyhow::{Context, Result};
use clap::Parser;

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::net::UdpSocket;
use tokio::sync::Mutex;
use tokio::time;
use tracing::{debug, info, warn};

#[path = "../proto.rs"]
mod proto; // reuse the server's proto.rs
#[path = "../loadout_client.rs"]
mod loadout_client;
#[path = "../combat_client.rs"]
mod combat_client;
use hsmp_pose::{posecodec, poseplay};
#[path = "../world_client.rs"]
mod world_client;
#[path = "../interact_client.rs"]
mod interact_client; // interaction channel (docs/development/subsystems/interact.md)
mod career_guard; // layer 2: career save backup/restore
#[path = "../events.rs"]
mod events; // --events JSONL (docs/development/testing.md)
mod session_client; // session records, commands + results

mod net;
mod handlers;
mod pose;
mod panic_guard; // any panic is fatal (log + career restore + exit)
mod ipc_shm; // HSMP-SHM: the shared-memory backend
mod records_in; // ABI 2: typed G2S records -> their domain
mod parent; // which game process we belong to
#[path = "../build_id.rs"]
mod build_id;

// Siblings share each other's items through `use super::*`.
use net::*;
use handlers::*;
use pose::*;

// Windows quantises sleeps/timers (tokio intervals included) to the 15.6 ms
// system tick unless the process asks for 1 ms resolution — without this a
// "3 ms" poll silently becomes ~16 ms.
#[cfg(windows)]
#[link(name = "winmm")]
extern "system" {
    fn timeBeginPeriod(u_period: u32) -> u32;
}

struct PeerPlay {
    pb: poseplay::Playback,
    seq: u64,
    stale_written: bool,
    rx_pose: u32,
    w: lat::Acc,
}

impl PeerPlay {
    fn new() -> Self {
        PeerPlay { pb: poseplay::Playback::new(), seq: 0, stale_written: false, rx_pose: 0,
                   w: lat::Acc::default() }
    }
}

/// Per-hop latency instrumentation, summarised to the log every 5 s.
mod lat {
    use std::collections::HashMap;
    use std::sync::{Mutex, OnceLock};

    #[derive(Default)]
    pub struct Acc { n: u64, sum: f64, max: f64 }

    impl Acc {
        pub fn add(&mut self, v: f64) {
            self.n += 1;
            self.sum += v;
            if v > self.max { self.max = v; }
        }
        pub fn take(&mut self) -> String {
            if self.n == 0 { return "-".into(); }
            let s = format!("avg {:.1} max {:.1} (n={})", self.sum / self.n as f64, self.max, self.n);
            *self = Acc::default();
            s
        }
    }

    #[derive(Default)]
    pub struct Stats {
        /// Game -> sidecar, root / skeletal (`lua->sidecar` in the report).
        pub lua_root: Acc,
        pub lua_skel: Acc,
        /// Sender sidecar send -> this sidecar receive (wall clock; exact on
        /// one machine, skewed by clock offset across machines).
        pub oneway: Acc,
        /// Transit jitter: |Δrecv - Δsender_ts| between consecutive roots.
        pub jitter: Acc,
        /// Receive -> `peer_root` record posted (`recv->slot` in the report).
        pub write: Acc,
        /// peer -> (last sender ts, last receive wall ms)
        pub last: HashMap<u32, (u32, f64)>,
    }

    pub fn with<R>(f: impl FnOnce(&mut Stats) -> R) -> R {
        static S: OnceLock<Mutex<Stats>> = OnceLock::new();
        let m = S.get_or_init(|| Mutex::new(Stats::default()));
        let mut g = m.lock().unwrap_or_else(|e| e.into_inner());
        f(&mut g)
    }
}

#[derive(Debug, Parser)]
#[command(author, version, about)]
struct Args {
    /// Central relay server endpoint.
    #[arg(long, default_value = "127.0.0.1:7777")]
    server: String,

    /// The game instance's state dir. It only holds the sidecar's logs
    /// (`.career_guard.jsonl`, `.sidecar_panic.log`), the identity and `.player_key`;
    /// no IPC file is read or written there.
    #[arg(long, default_value = "./hsmp_state")]
    state_dir: PathBuf,

    /// Nick to advertise to the relay on join.
    #[arg(long, default_value = "Willie")]
    nick: String,

    /// Ignored (the game link is shared memory); accepted so older launch
    /// command lines keep working.
    #[arg(long, hide = true)]
    poll_ms: Option<u64>,

    /// Ignored (the game link is shared memory: the status is the session
    /// blob's `status` sub-map); accepted so older launch command lines keep working.
    #[arg(long, hide = true)]
    status_file: Option<PathBuf>,

    /// Game instance id (career guard backup naming; matches the Lua
    /// saveguard's `HSMP_<inst>_*` slots).
    #[arg(long, env = "HSMP_INST", default_value = "0")]
    inst: String,

    /// Expected server identity (64 hex chars, from the browser / master
    /// listing `server_key`). A mismatch fails closed. Without it the key is
    /// trusted on first use and pinned for this process's reconnects.
    #[arg(long)]
    server_key: Option<String>,

    /// Content hash to present (64 hex chars) instead of the one this build embeds;
    /// servers that enforce one refuse other content before the cookie.
    #[arg(long)]
    content_hash: Option<String>,

    /// Test only: lowest protocol version offered (default: this build's).
    #[arg(long, hide = true)]
    proto_min: Option<u16>,

    /// Test only: highest protocol version offered (default: this build's).
    #[arg(long, hide = true)]
    proto_max: Option<u16>,

    /// Append structured JSONL events (session phase, command sent/result)
    /// to this file (docs/development/testing.md).
    #[arg(long)]
    events: Option<PathBuf>,

    /// The game's pid: when it exits, send Leave and exit cleanly (no orphan
    /// sidecar, the seat frees at once). Without it the game is found through
    /// the parent process chain (parent.rs).
    #[arg(long)]
    parent_pid: Option<u32>,

    /// Recover-only mode (used by hsmp-launcher before Play and Uninstall): run the
    /// career guard's crash recovery (the same `startup_check` a session start runs) over every
    /// open backup whose sidecar is gone, print one JSON line per action to stdout, and exit
    /// (0 = nothing to do or recovered, 1 = a guard error). Never connects, never backs up.
    #[arg(long)]
    career_recover: bool,

    /// Print this install's player key (64 hex; the identity is created on
    /// first use) and exit. Server operators put it in `--admin-key` /
    /// `--admins-file` to make that player an admin of a dedicated server.
    #[arg(long)]
    print_player_key: bool,

    /// Print this build's identity (version, protocol, IPC ABI, content hash) as one
    /// JSON line and exit. HSMPMenu compares it with the mods' own at startup.
    #[arg(long)]
    build_info: bool,

    /// The game link: `shm:<name>`, the game's shared segment (HSMP-SHM). Required for a
    /// session (with --parent-pid); without it the sidecar exits with code 64.
    #[arg(long, default_value = "")]
    ipc: String,

    /// HSMP-SHM debug tap: JSON lines of everything crossing the segment (gate / dev only).
    #[arg(long, env = "HSMP_IPC_TAP")]
    ipc_tap: Option<PathBuf>,

    /// `--career-recover`: the game's SaveGames dir (default %LOCALAPPDATA%\HalfSwordUE5\Saved\SaveGames).
    #[arg(long, requires = "career_recover")]
    career_save_dir: Option<PathBuf>,

    /// `--career-recover`: the guard's backup root (default %LOCALAPPDATA%\HSMP\save_backups).
    #[arg(long, requires = "career_recover")]
    career_backup_root: Option<PathBuf>,
}

/// `--career-recover`: crash recovery only. Returns the stdout JSON lines
/// (`{"ev":"career_guard","action","file","why","kind","backup"}`) and whether every
/// recovery succeeded. A session left open by a crash is recovered BEFORE the player's next
/// career play (otherwise that play makes the MP-damaged file look `skipped_newer`).
fn career_recover(cfg: Option<career_guard::GuardConfig>, owner: Option<career_guard::OwnerAlive>) -> (Vec<serde_json::Value>, bool) {
    let line = |action: &str, file: Option<&str>, why: &str, kind: &str, backup: &str| {
        serde_json::json!({"ev": "career_guard", "action": action, "file": file, "why": why, "kind": kind, "backup": backup})
    };
    let Some(cfg) = cfg else {
        return (vec![line("error", None, "LOCALAPPDATA is not set and no --career-save-dir/--career-backup-root given", "recover", "")], false);
    };
    let mut g = career_guard::CareerGuard::new(cfg);
    if let Some(f) = owner {
        g = g.with_owner_check(f);
    }
    match g.startup_check() {
        Ok(reports) => {
            let mut out = vec![];
            let mut ok = true;
            for r in &reports {
                for (action, file, why) in career_guard::report_actions(r) {
                    ok &= action != "error";
                    out.push(line(action, file.as_deref(), &why, &r.kind, &r.backup));
                }
            }
            if reports.is_empty() {
                out.push(line("none", None, "no open career-guard session to recover", "recover", ""));
            }
            (out, ok)
        }
        Err(e) => (vec![line("error", None, &format!("startup_check: {e}"), "recover", "")], false),
    }
}

fn career_recover_config(args: &Args) -> Option<career_guard::GuardConfig> {
    let mut cfg = career_guard::GuardConfig::from_env(&args.inst);
    if let (Some(s), Some(b)) = (&args.career_save_dir, &args.career_backup_root) {
        cfg = Some(career_guard::GuardConfig::with_dirs(s.clone(), b.clone(), &args.inst));
    } else if let Some(c) = cfg.as_mut() {
        if let Some(s) = &args.career_save_dir {
            c.save_dir = s.clone();
        }
        if let Some(b) = &args.career_backup_root {
            c.backup_root = b.clone();
        }
    }
    cfg
}

/// Exit status when the sidecar is started for a session without `--ipc shm:<name>`
/// (sysexits EX_USAGE).
const EXIT_NO_IPC: i32 = 64;

/// `--ipc shm:<name>`: attach the shared-memory link (HSMP-SHM), the only game link. A
/// missing `--ipc` exits with [`EXIT_NO_IPC`]. A refusal (wrong ABI / layout, wrong parent,
/// foreign owner, no segment) is written to the segment header when possible, logged,
/// tapped, and ends the process with the refuse code's exit status (70..78).
fn attach_ipc(args: &Args) -> &'static ipc_shm::ShmLink {
    let spec = args.ipc.trim();
    if spec.is_empty() || spec == "file" {
        let msg = "hsmp-sidecar needs --ipc shm:<name> (the game's shared segment) and --parent-pid; \
                   the file IPC backend was removed";
        eprintln!("{msg}");
        warn!(ipc = spec, "{msg}");
        events::emit("ipc_refused", serde_json::json!({"code": "no_ipc", "detail": msg}));
        std::process::exit(EXIT_NO_IPC);
    }
    let refuse = |r: hsmp_ipc::handshake::Refusal| -> ! {
        warn!(code = r.code.as_str(), detail = %r.detail, "ipc: refused the game's segment");
        events::emit("ipc_refused", serde_json::json!({"code": r.code.as_str(), "detail": r.detail}));
        if let Some(t) = &args.ipc_tap {
            ipc_shm::tap_refusal(t, &r);
        }
        std::process::exit(r.code.exit_code());
    };
    let Some(name) = hsmp_ipc::shm::parse_ipc_arg(spec) else {
        refuse(hsmp_ipc::handshake::Refusal { code: hsmp_ipc::RefuseCode::NotReady, detail: format!("bad --ipc value {spec:?} (want shm:<name>)") })
    };
    match ipc_shm::ShmLink::attach(name, args.parent_pid) {
        Ok(l) => {
            events::emit("ipc_attached", serde_json::json!({
                "name": name, "abi": format!("{}.{}", hsmp_ipc::ABI_MAJOR, hsmp_ipc::ABI_MINOR),
                "caps": l.segment().header.caps_effective.load(std::sync::atomic::Ordering::Acquire),
            }));
            l
        }
        Err(r) => refuse(r),
    }
}

// Multi-threaded, so a slow handler never starves the UDP receive and the
// transport timer; the segment is served by its own threads (hsmp-ipc, hsmp-poseplay).
#[tokio::main(flavor = "multi_thread", worker_threads = 3)]
async fn main() -> Result<()> {
    let args = Args::parse();
    if args.build_info {
        println!("{}", build_id::identity().to_json());
        std::process::exit(0)
    }
    // Before the log subscriber: stdout carries only the key.
    if args.print_player_key {
        match net::player_key_hex() {
            Ok(h) => { println!("{h}"); std::process::exit(0) }
            Err(e) => { eprintln!("player key: {e:#}"); std::process::exit(1) }
        }
    }
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "hsmp_sidecar=info".into()),
        )
        .init();

    #[cfg(windows)]
    unsafe {
        timeBeginPeriod(1);
    }

    if args.career_recover {
        let (lines, ok) = career_recover(career_recover_config(&args), None);
        for l in lines {
            println!("{l}");
        }
        std::process::exit(if ok { 0 } else { 1 });
    }
    panic_guard::install(Some(args.state_dir.clone()));
    info!(?args, "sidecar starting");
    if let Err(e) = events::init(args.events.as_deref(), "sidecar") {
        warn!(error = %e, path = ?args.events, "cannot open --events file");
    }
    // HSMP-SHM: attach to the game's segment before anything else (a refusal is an
    // install error: exit before the career guard enters MP).
    let shm_link = attach_ipc(&args);

    spawn_latency_report();

    let play_task = spawn_play_writer();

    tokio::fs::create_dir_all(&args.state_dir)
        .await
        .with_context(|| format!("mkdir {:?}", args.state_dir))?;

    // Career save guard: sidecar start = entering MP. Recover a
    // crashed earlier session, then back up + hash the career saves, all
    // before the first Join.
    // HSMP_CAREER_GUARD=0 (tests only) skips enter/heartbeat/leave entirely.
    let guard_enabled = std::env::var("HSMP_CAREER_GUARD").map_or(true, |v| v.trim() != "0");
    if !guard_enabled {
        warn!("career guard DISABLED (HSMP_CAREER_GUARD=0): career saves are not backed up or restored");
    }
    let guard = Arc::new(std::sync::Mutex::new(if guard_enabled {
        career_guard::GuardConfig::from_env(&args.inst).map(career_guard::CareerGuard::new)
    } else {
        None
    }));
    {
        let g = guard.clone();
        let sd = args.state_dir.clone();
        tokio::task::spawn_blocking(move || {
            if let Some(g) = g.lock().unwrap().as_mut() {
                match g.startup_check() {
                    Ok(rs) => for r in rs {
                        if !r.is_clean() { warn!(?r, "career save restored at startup ({} files)", r.changed()) }
                        career_guard_report(&sd, &r);
                    },
                    Err(e) => {
                        warn!("career guard startup_check: {e}");
                        career_guard_event(&sd, "error", None, &format!("startup_check: {e}"), "recover", "");
                    }
                }
                match g.enter() {
                    Ok(er) => career_guard_event(&sd, "backup", None,
                        &format!("{} career file(s), {} bytes backed up", er.files, er.bytes), "enter", &er.backup.display().to_string()),
                    Err(e) => {
                        warn!("career guard enter: {e}");
                        career_guard_event(&sd, "error", None, &format!("enter: {e}"), "enter", "");
                    }
                }
            }
        }).await.ok();
    }
    {
        // Panic = exit: restore the career saves first (best effort, never blocks).
        let g = guard.clone();
        let sd = args.state_dir.clone();
        panic_guard::on_fatal_panic(move || {
            let Ok(mut gg) = g.try_lock() else { return };
            let restored = gg.as_mut().map_or(true, |cg| match cg.leave() {
                Ok(Some(r)) => { career_guard_report(&sd, &r); true }
                Ok(None) => true,
                Err(_) => false,
            });
            // Always detach (and close the tap); "ended" only once the saves are back.
            if let Some(l) = ipc_shm::link() {
                let fin = session_client::final_link();
                l.shutdown(restored.then_some(fin.as_slice()), Duration::from_millis(100));
            }
        });
    }
    // Heartbeat well inside the guard's stale_ms (90 s), and often enough
    // that a hard kill's crash-recovery cutoff (alive_ms + grace) is tight
    // (career play right after a kill must not be "restored").
    let _guard_heartbeat = {
        let g = guard.clone();
        tokio::spawn(async move {
            let mut t = time::interval(career_guard::HEARTBEAT);
            t.tick().await;
            loop {
                t.tick().await;
                let g = g.clone();
                let r = tokio::task::spawn_blocking(move || {
                    g.lock().unwrap().as_ref().map(|g| g.heartbeat())
                }).await;
                if let Ok(Some(Err(e))) = r { warn!("career guard heartbeat: {e}") }
            }
        })
    };

    // Bound on the server's address family, so an IPv6 server is reachable too.
    let sock = Arc::new(net::connect_udp(&args.server).await?);
    info!(server = %args.server, local = %sock.local_addr()?, "udp connected");

    let shared = Arc::new(Mutex::new(SharedState {
        my_peer_id: 0,
        status: "connecting",
        is_admin: false,
    }));
    // The first link record (status CONNECTING) before any network event.
    session_client::link_update(|_| {});

    // The game process we belong to: its exit tears us down.
    let game = parent::resolve_now(args.parent_pid);
    match &game {
        parent::Target::Unwatched(why) => warn!(%why, "no game process to watch; the sidecar will not exit with the game"),
        t => info!(target = ?t, "watching the game process"),
    }
    events::emit("parent_watch", serde_json::json!({"target": format!("{game:?}")}));

    // Protocol v5: identity + client; the transport task sends the Hello.
    net::init(&args)?;
    panic_guard::maybe_test_panic();

    // Long-lived tasks, spawned in the original order (see each fn's notes).
    let recv_task = spawn_recv_task(&sock, &shared);
    let ping_task = spawn_ping_task(&sock, &shared);
    let transport_task = spawn_transport_task(&sock, &shared);
    // Root / skeletal / weapon from the game: the hsmp-ipc thread reads the typed slots and
    // hands the bodies to the forwarder, which sends them.
    let send_task = {
        let (tx, fwd) = ipc_shm::spawn_forwarder(&sock, &shared);
        shm_link.start(tx, args.ipc_tap.clone());
        fwd
    };

    // Loadout outbox: sends the game's loadout on change + every 5 s (detached).
    let _loadout_task = loadout_client::spawn_outbox_task(sock.clone(), shared.clone());

    // Combat replication: reliable damage/death + vitals (detached).
    let _combat_task = combat_client::spawn_task(sock.clone(), shared.clone());

    // World-object replication plumbing (detached; see world_client.rs).
    let _world_task = world_client::spawn_tasks(sock.clone(), shared.clone());

    // Session: the command resend loop (detached; session_client.rs).
    session_client::spawn_tasks();

    let mut leave_reason: u8 = proto::v5::LeaveReason::USER;
    tokio::select! {
        _ = tokio::signal::ctrl_c() => info!("ctrl-c; sidecar exit"),
        _ = parent::wait_game_exit(game.clone()) => {
            info!(parent_pid = ?args.parent_pid, target = ?game, "game process exited; leaving the server");
            events::emit("parent_exit", serde_json::json!({"parent_pid": args.parent_pid, "target": format!("{game:?}")}));
            leave_reason = proto::v5::LeaveReason::GAME_EXITED;
        }
        r = recv_task => warn!(?r, "recv task exited"),
        r = send_task => warn!(?r, "send task exited"),
        r = transport_task => warn!(?r, "transport task exited"),
        r = session_client::leave_requested() => {
            info!(reason = r, "leave requested by the game");
            leave_reason = r;
        }
        r = ping_task => warn!(?r, "ping task exited"),
        r = play_task => warn!(?r, "pose play task exited"),
    }

    // Send Leave so the relay removes us promptly.
    net::leave(&sock, &shared, leave_reason).await;
    events::emit("sidecar_exit", serde_json::json!({"leave_reason": leave_reason}));

    // Career save guard: sidecar exit = leaving MP. Order
    // matters: restore first, then the terminal status (the Lua guard turns
    // off and reloads the restored career into the GI).
    let g = guard.clone();
    let rep = tokio::task::spawn_blocking(move || g.lock().unwrap().as_mut().map(|g| g.leave())).await;
    match rep {
        Ok(Some(Ok(Some(r)))) => {
            if r.is_clean() { info!("career saves unchanged") }
            else { warn!(?r, "career save restored ({} files)", r.changed()) }
            career_guard_report(&args.state_dir, &r);
        }
        Ok(Some(Ok(None))) | Ok(None) => {}
        Ok(Some(Err(e))) => {
            warn!("career guard leave: {e}");
            career_guard_event(&args.state_dir, "error", None, &format!("leave: {e}"), "leave", "");
        }
        Err(e) => warn!("career guard leave task: {e}"),
    }
    // The terminal link record (status ENDED), a last publish, then detach.
    let fin = session_client::final_link();
    shm_link.shutdown(Some(&fin), Duration::from_millis(500));
    Ok(())
}

/// Career guard evidence for the gate: every backup / restore / recreate /
/// quarantine / error is one `career_guard{action,file,why,kind,backup}` event through
/// `--events` AND one line in `<state>/.career_guard.jsonl` (the game launches the sidecar
/// without `--events`; mp_test.ps1 collects the file into `inst<i>/`). Append-only, never
/// swept, so a restart's recovery and the final leave are both kept. Never fails the caller.
fn career_guard_event(state_dir: &Path, action: &str, file: Option<&str>, why: &str, kind: &str, backup: &str) {
    let f = serde_json::json!({"action": action, "file": file, "why": why, "kind": kind, "backup": backup});
    events::emit("career_guard", f.clone());
    let mut line = f;
    line["ev"] = serde_json::json!("career_guard");
    line["wall_ms"] = serde_json::json!(events::wall_ms());
    line["pid"] = serde_json::json!(std::process::id());
    use std::io::Write as _;
    if let Ok(mut fh) = std::fs::OpenOptions::new().create(true).append(true).open(state_dir.join(CAREER_GUARD_LOG)) {
        let _ = fh.write_all(format!("{line}\n").as_bytes());
    }
}

const CAREER_GUARD_LOG: &str = ".career_guard.jsonl";

fn career_guard_report(state_dir: &Path, r: &career_guard::Report) {
    for (action, file, why) in career_guard::report_actions(r) {
        career_guard_event(state_dir, action, file.as_deref(), &why, &r.kind, &r.backup);
    }
}


struct SharedState {
    my_peer_id: u32,
    /// The sidecar status name ("connecting", "connected", ...; the link record carries the code).
    status: &'static str,
    is_admin: bool,
}

#[cfg(test)]
mod startup_tests {
    use super::*;

    /// `--career-recover` restores a crashed session's career file and reports it,
    /// leaves a live session alone, and says "none" when there is nothing to recover.
    #[test]
    fn career_recover_mode() {
        let d = std::env::temp_dir().join(format!("hsmp_cgrecover_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        let (saves, root) = (d.join("SaveGames"), d.join("save_backups"));
        std::fs::create_dir_all(&saves).unwrap();
        let cfg = || career_guard::GuardConfig::with_dirs(saves.clone(), root.clone(), "1");
        let (lines, ok) = career_recover(Some(cfg()), Some(|_, _| false));
        assert!(ok);
        assert_eq!(lines[0]["action"], "none", "{lines:?}");

        std::fs::write(saves.join("GameProgress.sav"), b"career").unwrap();
        // a session that "crashed": entered, never left (the guard is dropped without leave)
        career_guard::CareerGuard::new(cfg()).enter().unwrap();
        std::fs::write(saves.join("GameProgress.sav"), b"MP damaged").unwrap();
        // its sidecar still runs: not touched
        let (lines, ok) = career_recover(Some(cfg()), Some(|_, _| true));
        assert!(ok);
        assert_eq!(lines[0]["action"], "none", "{lines:?}");
        assert_eq!(std::fs::read(saves.join("GameProgress.sav")).unwrap(), b"MP damaged");
        // its sidecar is gone: restored
        let (lines, ok) = career_recover(Some(cfg()), Some(|_, _| false));
        assert!(ok, "{lines:?}");
        assert!(lines.iter().any(|l| l["action"] == "restored" && l["file"] == "GameProgress.sav" && l["kind"] == "recover"), "{lines:?}");
        assert_eq!(std::fs::read(saves.join("GameProgress.sav")).unwrap(), b"career");
        // recovered once: the manifest is closed
        let (lines, _) = career_recover(Some(cfg()), Some(|_, _| false));
        assert_eq!(lines[0]["action"], "none", "{lines:?}");
        // no LOCALAPPDATA and no dirs: an error, exit 1
        let (lines, ok) = career_recover(None, None);
        assert!(!ok);
        assert_eq!(lines[0]["action"], "error");
        let _ = std::fs::remove_dir_all(&d);
    }

    /// The career guard's actions reach `<state>/.career_guard.jsonl` (appended, one JSON
    /// object per line), and the session-start sweep keeps the file.
    #[test]
    fn career_guard_log_is_appended_and_kept() {
        let d = std::env::temp_dir().join(format!("hsmp_cglog_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        let r = career_guard::Report { kind: "leave".into(), backup: "B".into(), restored: vec!["GameProgress.sav".into()], ..Default::default() };
        career_guard_event(&d, "backup", None, "3 career file(s)", "enter", "B");
        // The state dir is never swept: the log is kept across sessions.
        career_guard_report(&d, &r);
        let text = std::fs::read_to_string(d.join(CAREER_GUARD_LOG)).unwrap();
        let rows: Vec<serde_json::Value> = text.lines().map(|l| serde_json::from_str(l).unwrap()).collect();
        assert_eq!(rows.len(), 2, "{text}");
        assert_eq!(rows[0]["action"], "backup");
        assert_eq!(rows[1]["ev"], "career_guard");
        assert_eq!(rows[1]["action"], "restored");
        assert_eq!(rows[1]["file"], "GameProgress.sav");
        assert_eq!(rows[1]["kind"], "leave");
        assert!(rows[1]["wall_ms"].as_u64().unwrap() > 1_600_000_000_000);
        let _ = std::fs::remove_dir_all(&d);
    }
}
