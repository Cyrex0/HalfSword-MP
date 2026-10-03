//! Fake game instance for exercising scripts/mp_test.ps1 end to end WITHOUT Half Sword
//! (`mp_test.ps1 -FakeGame`). It drives the REAL hsmp-sidecar / hsmp-server through the
//! same shared-memory IPC the Lua mods use (docs/development/ipc-shared-memory.md)
//! and writes the same hsmp_events.jsonl vocabulary, so the harness's process handling,
//! netsim, RCON, waits, collection and assertions run for real.
//!
//! Environment = the game contract (docs/development/testing.md): HSMP_INST, HSMP_STATE_DIR (relative
//! to cwd = Binaries/Win64), HSMP_AUTOTEST (1 | host | join), HSMP_AUTOTEST_ADDR,
//! HSMP_AUTOTEST_EXTERNAL, HSMP_AUTOTEST_READY, HSMP_AUTOTEST_WAIT_MS, HSMP_NETSIM_ADDR,
//! HSMP_SERVER_EXE, HSMP_SIDECAR_EXE, HSMP_IPC_TAP. Like HSMPMenu (`IPC.spawn`) it launches
//! binaries directly with no window and does not kill them at exit, so a sidecar outlives the
//! fake unless it honours --parent-pid (DoD-12).
//!
//! The fake is the shared-memory GAME (hsmp_tools::ipcgame::GameHost): it creates the segment,
//! starts the sidecar with `--parent-pid <own pid> --ipc shm:<name>` (+ `--ipc-tap
//! $HSMP_IPC_TAP`), reads the session domain record slots (STATE: `link` = the sidecar's status,
//! `session` = the server's snapshot), sends typed G2S records (`command`, `game_status`) and
//! takes the S2G `cmd_result` records (QUEUES). There is no file IPC fallback.
//! Harness commands arrive as `dev_cmd` AUTOTEST records in the DevCtl ring
//! (`hsmp-tools ipc-ctl --pid <fake> autotest <cmd> [arg]`), as for the real HSMPMenu.

use crate::util;
use hsmp_ipc::schema::{CAP_QUEUES, CAP_STATE};
use hsmp_ipc::schema::session::{cmd_op, status_flag};
use hsmp_tools::ipcgame::{self as ig, GameHost};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::path::PathBuf;
use std::time::{Duration, Instant};

struct Fake {
    inst: String,
    state: PathBuf,
    seq: i64,
    t0: Instant,
    host: GameHost,
    /// S2G cmd_result payloads not yet consumed.
    results: Vec<Value>,
    /// DevCtl commands not yet consumed.
    ctl: Vec<Value>,
    /// The negotiated caps were missing STATE/QUEUES once (logged once).
    caps_warned: bool,
}

impl Fake {
    fn ev(&mut self, name: &str, f: Value) {
        self.seq += 1;
        let mut rec = json!({"v": 1, "ev": name, "inst": self.inst, "mod": "FakeGame", "seq": self.seq,
                             "t_ms": self.t0.elapsed().as_millis() as i64, "wall_ms": util::now_ms()});
        for (k, v) in f.as_object().into_iter().flatten() {
            rec[k] = v.clone();
        }
        let _ = util::append_line(&self.state.join("hsmp_events.jsonl"), &rec.to_string());
    }
    /// One typed G2S record (`command`, `game_status`, ...).
    fn send_rec(&self, name: &str, v: Value) {
        if let Err(e) = self.host.send_record(name, &v) {
            eprintln!("fake-game: g2s {name} failed ({e:#})");
        }
    }
    /// One game frame of the shared-memory link: collect cmd results and DevCtl commands.
    fn pump(&mut self) {
        for e in self.host.pump() {
            match e["ev"].as_str() {
                Some("s2g") if e["kind"] == "cmd_result" => self.results.push(e["v"].clone()),
                // dev_cmd records (hsmp-tools ipc-ctl): AUTOTEST {key = cmd, arg}
                Some("devctl") if e["kind"] == "dev_cmd" && e["v"]["op"] == json!(hsmp_ipc::schema::dev::DEV_OP_AUTOTEST) => {
                    let arg = e["v"]["arg"].as_str().filter(|a| !a.is_empty()).map(|a| json!(a)).unwrap_or(Value::Null);
                    self.ctl.push(json!({"cmd": e["v"]["key"], "arg": arg, "id": e["v"]["id"]}));
                }
                Some("attach") | Some("refuse") => eprintln!("fake-game: ipc {e}"),
                _ => {}
            }
        }
        if self.host.sidecar_ready() && !self.caps_warned && !(self.host.has(CAP_STATE) && self.host.has(CAP_QUEUES)) {
            self.caps_warned = true;
            eprintln!("fake-game: the sidecar did not negotiate STATE+QUEUES (caps {:#x}); this fake has no file fallback", self.host.caps_effective());
        }
    }
    /// A typed `command` record exactly as HSMPMenu commands.lua sends it: the sidecar resends
    /// it until the server's S2G `cmd_result` record arrives.
    fn send_cmd(&self, cmd: &str, arg: Option<&str>, id: u64) {
        let mut c = json!({"cmd_id": id});
        match (cmd, arg) {
            ("pick_arena", Some(a)) => {
                c["op"] = json!(cmd_op::PICK_ARENA);
                c["text"] = json!(a);
            }
            ("ready", _) => {
                c["op"] = json!(cmd_op::READY);
                c["flag"] = json!(true);
            }
            ("unready", _) => {
                c["op"] = json!(cmd_op::READY);
                c["flag"] = json!(false);
            }
            ("start", _) => c["op"] = json!(cmd_op::START),
            _ => c["op"] = json!(cmd_op::ABORT),
        }
        self.send_rec("command", c);
    }
    /// The sidecar's `link` record (its status), `{}` until it published one.
    fn link(&self) -> Value {
        self.host.link().cloned().unwrap_or(json!({}))
    }
    /// The server's `session` snapshot record, `{}` until one arrived.
    fn sess(&self) -> Value {
        self.host.session().cloned().unwrap_or(json!({}))
    }
}

fn env(k: &str) -> Option<String> {
    std::env::var(k).ok().filter(|v| !v.trim().is_empty())
}

/// Like HSMPMenu through the native module (`IPC.spawn`): CreateProcess with
/// no window and no shell; the child is not killed when the fake exits (the sidecar follows
/// its --parent-pid; the harness kills the server by PID).
fn spawn(exe: &str, args: &[String]) -> Option<u32> {
    let mut c = std::process::Command::new(exe);
    c.args(args).stdin(std::process::Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        c.creation_flags(CREATE_NO_WINDOW);
    }
    match c.spawn() {
        Ok(ch) => Some(ch.id()),
        Err(e) => {
            eprintln!("fake-game: spawn {exe} failed ({e})");
            None
        }
    }
}

pub fn run() -> i32 {
    let inst = env("HSMP_INST").unwrap_or("0".into());
    let state = PathBuf::from(env("HSMP_STATE_DIR").unwrap_or("hsmp_state".into()));
    let role = env("HSMP_AUTOTEST").unwrap_or_default();
    let external = env("HSMP_AUTOTEST_EXTERNAL").as_deref() == Some("1");
    let ready = env("HSMP_AUTOTEST_READY").as_deref() != Some("0");
    let wait_ms: u128 = env("HSMP_AUTOTEST_WAIT_MS").and_then(|v| v.parse().ok()).unwrap_or(4000);
    let _ = std::fs::create_dir_all(&state);
    let host = match GameHost::create(None, u64::MAX >> 1) {
        Ok(h) => h,
        Err(e) => {
            eprintln!("fake-game: the shared-memory segment could not be created ({e:#})");
            return 2;
        }
    };
    let mut g = Fake { inst: inst.clone(), state: state.clone(), seq: 0, t0: Instant::now(), host, results: Vec::new(), ctl: Vec::new(), caps_warned: false };
    g.ev("_open", json!({"lib_v": 1, "state_dir": state.display().to_string(), "fake": true}));

    let sidecar = env("HSMP_SIDECAR_EXE").unwrap_or("hsmp/hsmp-sidecar.exe".into()).replace('/', "\\");
    // persistent config: stays a file
    let settings = util::read_json(&state.join(".settings.json")).unwrap_or(json!({}));
    let srv_addr = settings["server"].as_str().unwrap_or("127.0.0.1:7777").to_string();
    let host = matches!(role.as_str(), "1" | "host") && !external;
    let target = if host {
        let port = srv_addr.rsplit(':').next().unwrap_or("7777").to_string();
        let server = env("HSMP_SERVER_EXE").unwrap_or("hsmp/hsmp-server.exe".into()).replace('/', "\\");
        // As HSMPMenu HOST: this instance is the listen server's owner (admin) by its
        // player key, which its sidecar writes to <state>\.player_key.
        let owner = state.join(".player_key").display().to_string();
        let a: Vec<String> = ["--bind", &format!("0.0.0.0:{port}"), "--tick-hz", "30", "--max-peers", "8", "--owner-key-file", &owner]
            .iter().map(|s| s.to_string()).collect();
        spawn(&server, &a);
        std::thread::sleep(Duration::from_millis(500));
        srv_addr.clone()
    } else {
        env("HSMP_NETSIM_ADDR").or_else(|| env("HSMP_AUTOTEST_ADDR")).unwrap_or(srv_addr.clone())
    };
    let st = state.display().to_string();
    let mut a: Vec<String> = vec!["--server".into(), target, "--state-dir".into(), st, "--nick".into(), format!("HSMP{inst}")];
    a.extend(g.host.sidecar_args());
    if let Some(tap) = env("HSMP_IPC_TAP") {
        a.extend(["--ipc-tap".to_string(), tap]);
    }
    spawn(&sidecar, &a);

    let mut connected = false;
    let mut started = false;
    let mut ready_sent: Option<Instant> = None;
    let mut ready_ok = false;
    let mut in_match = false;
    let mut last_key: Option<(String, i64)> = None;
    let mut cur_world = "Map_Menu_Startup".to_string();
    // u32 command ids unique per process, as commands.lua (C.base = os.time() % 4e6 * 1000)
    let cmd_base: u64 = (util::now_ms() as u64 / 1000 % 4_000_000) * 1000;
    let mut cmd_n: u64 = 0;
    let mut pending: HashMap<u64, (String, Instant)> = HashMap::new();
    let mut last_ping: Option<Instant> = None;
    // stall probe (docs/development/testing.md "Stall probe"): frame_hb{n,max_ms} every 10 s
    let (mut hb_n, mut hb_at, mut hb_max, mut last_loop) = (0u64, Instant::now(), 0u128, Instant::now());
    let my_role = if host { "host" } else { "join" };
    loop {
        let gap = last_loop.elapsed().as_millis();
        last_loop = Instant::now();
        hb_max = hb_max.max(gap);
        if hb_at.elapsed() >= Duration::from_secs(10) {
            hb_n += 1;
            g.ev("frame_hb", json!({"n": hb_n, "max_ms": hb_max}));
            hb_at = Instant::now();
            hb_max = 0;
        }
        g.pump();
        let lk = g.link();
        let ss = g.sess();
        // The server instance (S2CSession epoch; the link's Welcome epoch until a snapshot came).
        let epoch = if ss["epoch"].is_u64() { ss["epoch"].clone() } else { lk["server_epoch"].clone() };
        let me = lk["my_peer_id"].clone();
        if ig::link_connected(&lk) && !connected {
            connected = true;
            g.ev("lobby_ready", json!({"epoch": epoch, "peer_id": me, "role": my_role, "arena": ""}));
        }
        let mut submit = |g: &mut Fake, cmd: &str, arg: Option<&str>| {
            cmd_n += 1;
            let id = cmd_base + cmd_n;
            g.ev("cmd_sent", json!({"cmd": cmd, "cmd_id": id, "arg": arg, "backend": "legacy", "via": "autotest"}));
            g.send_cmd(cmd, arg, id);
            pending.insert(id, (cmd.to_string(), Instant::now()));
        };
        // auto-ready as a typed command once per lobby arrival (HSMPMenu autotest_ready_tick:
        // cmd_sent / cmd_result{cmd=ready}; the harness waits for that answer before START)
        if connected && ready && !ready_ok && ready_sent.map(|t| t.elapsed() > Duration::from_secs(3)).unwrap_or(true) {
            ready_sent = Some(Instant::now());
            submit(&mut g, "ready", Some("true"));
        }
        if connected && role == "1" && !started && g.t0.elapsed().as_millis() >= wait_ms {
            started = true;
            submit(&mut g, "start", None);
        }
        // harness client commands: dev_cmd AUTOTEST records (hsmp-tools ipc-ctl, docs/development/testing.md)
        let cmds: Vec<Value> = std::mem::take(&mut g.ctl);
        for c in cmds {
            match c["cmd"].as_str().unwrap_or("") {
                "pick_arena" => {
                    let a = c["arg"].as_str().unwrap_or("Alley");
                    let arena = if a.starts_with("Map_") { a.to_string() } else { format!("Map_Arena_{a}") };
                    submit(&mut g, "pick_arena", Some(&arena));
                }
                v @ ("start" | "ready" | "unready" | "abort") => submit(&mut g, v, None),
                "leave" | "quit" => {
                    g.ev("travel", json!({"from": cur_world, "to": "Map_Menu_Startup", "by": "director", "reason": "left the match"}));
                    return 0;
                }
                _ => {}
            }
        }
        // results: ONLY the server's answer (an S2G cmd_result from the real sidecar) resolves a
        // command, as commands.lua does; else a timeout.
        let answers: Vec<Value> = std::mem::take(&mut g.results);
        {
            for r in answers {
                let Some(id) = r["cmd_id"].as_u64() else { continue };
                if let Some((cmd, t)) = pending.remove(&id) {
                    let ok = r["ok"].as_bool() == Some(true);
                    if cmd == "ready" && ok {
                        ready_ok = true;
                    }
                    let reason = if ok { Value::Null } else { r["reason_text"].clone() };
                    g.ev("cmd_result", json!({"cmd": cmd, "cmd_id": id, "ok": ok, "reason": reason, "state": if ok { "accepted" } else { "refused" },
                                               "source": "server", "ms": t.elapsed().as_millis() as u64, "tries": 1}));
                }
            }
        }
        let timed_out: Vec<u64> = pending.iter().filter(|(_, (_, t))| t.elapsed() > Duration::from_secs(15)).map(|(id, _)| *id).collect();
        for id in timed_out {
            let (cmd, t) = pending.remove(&id).unwrap();
            g.ev("cmd_result", json!({"cmd": cmd, "cmd_id": id, "ok": false, "reason": "no answer from the server", "state": "refused",
                                       "source": "timeout", "ms": t.elapsed().as_millis() as u64, "tries": 1}));
        }
        // The legacy match state, arena in force and round, from the session record.
        let st_s = if ss.is_object() && ss["phase"].is_u64() { ig::legacy_state(ss["phase"].as_u64().unwrap_or(0)).to_string() } else { String::new() };
        let (arena, rnd) = (ig::session_arena(&ss), ss["round"].as_i64().unwrap_or(0));
        if matches!(st_s.as_str(), "countdown" | "live" | "roundover" | "paused" | "match_over") {
            in_match = true;
        } else if st_s == "lobby" && in_match {
            // back in the lobby after a match (ABORT / match over): the Director travels to
            // the menu and the menu re-emits lobby_ready and re-readies (docs/development/testing.md)
            in_match = false;
            last_key = None;
            g.ev("travel", json!({"from": cur_world, "to": "Map_Menu_Startup", "by": "director", "reason": "match over"}));
            cur_world = "Map_Menu_Startup".to_string();
            g.ev("lobby_ready", json!({"epoch": epoch, "peer_id": me, "role": my_role, "arena": arena.clone().unwrap_or_default()}));
            ready_ok = false;
            ready_sent = None;
        }
        if matches!(st_s.as_str(), "countdown" | "live") {
            if let Some(arena) = arena.clone() {
                let want_round = rnd + if st_s == "countdown" { 1 } else { 0 };
                let key = (arena.clone(), want_round);
                if last_key.as_ref() != Some(&key) {
                    last_key = Some(key);
                    // The real emitters' shapes (director.lua / kit.lua). Only what a real
                    // emitter writes: no puppet kit_verified, no pose_quality, no snap_z.
                    g.ev("travel", json!({"from": cur_world, "to": arena, "by": "director", "reason": "server phase"}));
                    cur_world = arena.clone();
                    let wk = format!("{arena}#{want_round}");
                    g.ev("world_ready", json!({"arena": arena, "world_key": wk, "round": want_round, "server_arena": arena}));
                    // a fake pawn per instance, 4 m apart, 20 cm from its order
                    let k: f64 = inst.parse().unwrap_or(1.0);
                    g.ev("spawn_verified", json!({"round": want_round, "ok": true, "arena": arena, "world_key": wk, "vitals_ok": true, "gi_ok": true,
                                                  "x": 400.0 * k, "y": 0.0, "z": 100.0, "spawn_id": inst, "dist_cm": 20, "steps": "fake", "load_error": null, "ms": 1000}));
                    let me = me.as_i64();
                    // remote fighters: the connected roster rows of the session record
                    let mut peers: Vec<i64> = ss["rows"].as_array().into_iter().flatten()
                        .filter(|r| r["connected"] == true)
                        .filter_map(|r| r["peer_id"].as_i64())
                        .filter(|p| Some(*p) != me && *p != 0).collect();
                    peers.sort();
                    peers.dedup();
                    g.ev("kit_verified", json!({"who": "self", "armour_n": 5, "r_class": "BP_Sword_C", "l_class": "None", "ok": true,
                                                "exp_armour_n": 5, "round": want_round, "kit": "fake", "tries": 1}));
                    let n = peers.len();
                    g.ev("willie_census", json!({"visible": 1 + n, "expected": 1 + n, "extras": 0, "missing": 0, "at": "ready", "round": want_round}));
                    g.ev("ready_report", json!({"round": want_round, "arena": arena, "load_error": null}));
                }
                if last_ping.map(|t| t.elapsed() >= Duration::from_secs(1)).unwrap_or(true) {
                    last_ping = Some(Instant::now());
                    // The Director's game status (load barrier + liveness), as a typed record.
                    let flags = if want_round > 0 { status_flag::LOADED | status_flag::READY } else { 0 };
                    g.send_rec("game_status", json!({"match_id": ss["match_id"], "round": want_round, "flags": flags, "arena": arena}));
                }
            }
        }
        std::thread::sleep(Duration::from_millis(250));
    }
}
