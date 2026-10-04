//! Deterministic mocked gate runs under scripts/fixtures/<case>/ and the self-test
//! that evaluates them against expected.json. Regenerate with
//! `hsmp-gate make-fixtures` after changing the vocabulary or the evaluator.
//!
//! Every event here has the shape its REAL emitter writes (invented fields such as
//! spawn_verified.dist_m/z_ok/min_peer_m would keep the self-test green while the gate
//! could not judge real runs):
//!
//! * director.lua: travel{from,to,by,reason}, world_ready{arena,world_key,round,server_arena},
//!   spawn_verified{round,ok,arena,world_key,vitals_ok,gi_ok,x,y,z,spawn_id,dist_cm,steps,
//!   load_error,ms}, willie_census{visible,expected,extras,missing,at="ready",round},
//!   ready_report{round,arena,load_error}
//! * kit.lua kit_verified{who="self",armour_n,r_class,l_class,ok,exp_armour_n,round,kit,tries}
//! * hsmp_saveguard.lua x_save_guard{active,why,session}, save_redirected{fn,slot,to_slot,op,ok,how}
//! * commands.lua cmd_sent{cmd,cmd_id,arg,backend,via}, cmd_result{cmd,cmd_id,ok,reason,state,
//!   source,ms,tries}; the sidecar tap's S2G cmd_result records (session_client::result_line)
//! * hsmp-server --events phase{from,to,match_id,round,arena,frozen_arena,epoch},
//!   cmd_result{player,seat,peer_id,source,cmd,cmd_id,ok,reason,code,config_rev}, ...
//! * netsim --events netsim_start{listen,upstream,profile,pid,delay,...}
//!
//! `Shape::Contract` adds the fields/events that are AGREED but not emitted yet (contract
//! PENDING: hitch/frame_hb stall probe, spawn_verified.snap_z, puppet kit_verified, Live
//! willie_census, pose_quality) so the pass path of every rule is exercised; `Shape::Today`
//! is exactly what the tree emits now (`p0_gate_today`: incomplete, never pass).
//! `contract::check` validates every fixture event against the contract.

use crate::g0;
use crate::rules;
use crate::soak;
use crate::util;
use serde_json::{json, Map, Value};
use std::path::{Path, PathBuf};

const T0: i64 = 1790880000000;
const COMMIT: &str = "0123456789abcdef0123456789abcdef01234567";
/// SHA-256 of the deployed files in the fixtures' deploy stamp (server, sidecar, master, a Lua file)
const BIN_SHA: [&str; 4] = [
    "A1A1A1A1A1A1A1A1A1A1A1A1A1A1A1A1A1A1A1A1A1A1A1A1A1A1A1A1A1A1A1A1",
    "B2B2B2B2B2B2B2B2B2B2B2B2B2B2B2B2B2B2B2B2B2B2B2B2B2B2B2B2B2B2B2B2",
    "C3C3C3C3C3C3C3C3C3C3C3C3C3C3C3C3C3C3C3C3C3C3C3C3C3C3C3C3C3C3C3C3",
    "D4D4D4D4D4D4D4D4D4D4D4D4D4D4D4D4D4D4D4D4D4D4D4D4D4D4D4D4D4D4D4D4",
];

pub fn fixtures_dir(repo: &Path) -> PathBuf {
    repo.join("scripts").join("fixtures")
}

#[derive(Clone, Copy, PartialEq)]
enum Shape {
    /// only what a real emitter writes today
    Today,
    /// + the agreed, pending contract fields (contract::PENDING)
    Contract,
}

struct Run {
    dir: PathBuf,
    n: usize,
    cfg: Value,
    shape: Shape,
    inst: Vec<Vec<Value>>,
    results: Vec<Vec<Value>>,
    seq: Vec<i64>,
    server: Vec<Value>,
    harness: Vec<Value>,
    observed: Vec<Value>,
    netsim: Vec<Vec<Value>>,
    t: i64,
    phase: String,
    match_id: i64,
    epoch: i64,
    arena: String,
    frozen: Option<String>,
    cmd_n: i64,
    baseline: Value,
    fin: Value,
    g0: Option<Value>,
    /// default pose_quality sample [arm_p95_uu, tip_p95_uu, latency_ms, jitter_ratio, foot_slide_p95, idle_rms]
    pose: [f64; 6],
    /// pose_quality without the stand-in quality fields (an older pose emitter)
    pose_legacy: bool,
    /// frame_hb heartbeat per instance (Shape::Contract); next due time per instance
    hb: bool,
    hb_next: Vec<i64>,
    /// new dumps in crash_triage.json: (dir, signature, thread, wall_ms); written unless `no_triage`
    crashes: Vec<(String, String, String, i64)>,
    no_triage: bool,
    /// soak memory sampler rows (mem.jsonl)
    mem: Vec<Value>,
    /// extra UE4SS.log lines (Lua errors etc.)
    ue4ss: Vec<String>,
    /// the sidecar's <state>/.career_guard.jsonl per instance (backup at start, clean leave at
    /// the end unless `cg_leave` is false); `cg_extra` adds lines
    career_guard: bool,
    /// inst<N>/state_dir_listing.txt per instance (STATE-1); None = not written
    listing: Vec<Option<Vec<&'static str>>>,
    cg_extra: Vec<(usize, Value)>,
    /// netsim_stats every 5 s per impaired instance (cumulative); false = the client bypassed
    /// the proxy (counters stay 0, clients 0)
    netsim_traffic: Vec<bool>,
    /// the proxy's scheduling-lateness p99 per window (ms) per instance; None = an old
    /// netsim without late_* fields
    netsim_late: Vec<Option<f64>>,
    /// measured loss as a fraction of the profile's (1.0 = exactly the profile)
    netsim_loss_scale: Vec<f64>,
    /// how long each round's Live lasts (ms; Live -> RoundOver). The real p0_gate kills at once
    /// (~1 s); WORLD-1 / COMBAT-1 only judge rounds with Live >= 10 s.
    live_len: i64,
    /// HSMPWorld world_track / world_sync_quality during Live (harness runs; WORLD-2)
    track: bool,
}

fn obj(v: Value) -> Map<String, Value> {
    v.as_object().cloned().unwrap_or_default()
}

fn profile_impair(p: &str) -> Value {
    let (delay, jitter, loss) = match p {
        "wifi" => (35, 15, 2.0),
        "bad" => (110, 40, 5.0),
        _ => (50, 12, 1.0),
    };
    json!({"delay": delay, "jitter": jitter, "loss": loss, "dup": 0.3, "spike_every": 0, "spike_ms": 0, "spike_len": 0})
}

impl Run {
    fn new(root: &Path, name: &str, scenario: &str, arenas: &[&str], rounds: u32, mode: &str, shape: Shape) -> Run {
        let dir = root.join(name);
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let n = 2;
        let baseline = json!({"save_hashes": {"GameProgress.sav": "aa11", "Settings.sav": "bb22"},
                              "save_files": {"GameProgress.sav": {"size": 18432, "mtime_ms": T0 - 86_400_000, "sha256": "aa11"},
                                             "Settings.sav": {"size": 2048, "mtime_ms": T0 - 86_000_000, "sha256": "bb22"}},
                              "crash_dirs": ["UECC-old"], "wall_ms": T0 - 20_000});
        let fin = json!({"save_hashes": baseline["save_hashes"].clone(), "save_files": baseline["save_files"].clone(), "new_crashes": [],
                         "alive_at_end": {"game1": true, "game2": true, "server": true}, "orphans": [], "restarted": {},
                         "quit_path": {"game1": "menu_quit", "game2": "menu_quit"}, "game_force_killed": []});
        let listen = scenario == "host_leave";
        let mut r = Run {
            dir, n,
            cfg: json!({"scenario": scenario, "mode": mode, "instances": n, "arenas": arenas, "rounds": rounds, "fixture": true,
                       "topology": if listen { "listen" } else { "dedicated" },
                       "netsim": {"1": if listen { "none" } else { "typical" }, "2": "typical"},
                       "deploy": {"commit": COMMIT, "dirty": false, "profile": "release", "g0_ok": true, "g0": {"commit": COMMIT, "ok": true},
                                  "skip_build": false, "bins_commit": COMMIT, "bins_match_commit": true, "bin_dir": "hsmp",
                                  "hashes": {"hsmp/hsmp-server.exe": BIN_SHA[0], "hsmp/hsmp-sidecar.exe": BIN_SHA[1],
                                             "hsmp/hsmp-master.exe": BIN_SHA[2], "ue4ss/Mods/HSMPMatch/Scripts/main.lua": BIN_SHA[3]}},
                       // mp_test.ps1 re-hashes the stamped files at run start and the binaries it runs
                       "deploy_check": {"checked": 4, "mismatched": [], "missing": [], "bin_dir": "C:/game/HalfswordUE5/Binaries/Win64/hsmp",
                                        "bins_used": {"hsmp-server.exe": BIN_SHA[0], "hsmp-sidecar.exe": BIN_SHA[1], "hsmp-master.exe": BIN_SHA[2]}},
                       // the judging gate's identity in a synthetic run (rules::judge_identity)
                       "fixture_gate": {"commit": COMMIT, "dirty": "0"}}),
            shape,
            inst: vec![vec![]; n + 1], results: vec![vec![]; n + 1], seq: vec![0; n + 1], server: vec![], harness: vec![], observed: vec![],
            netsim: vec![vec![]; n + 1],
            t: T0, phase: "Lobby".into(), match_id: 0, epoch: 1, arena: "default".into(), frozen: None, cmd_n: 0, baseline, fin,
            g0: Some(g0_doc(&[])),
            pose: [3.1, 5.2, 72.0, 1.05, 2.0, 0.2], pose_legacy: false,
            hb: shape == Shape::Contract, hb_next: vec![T0; n + 1],
            crashes: vec![], no_triage: false, mem: vec![], ue4ss: vec![],
            career_guard: true, cg_extra: vec![], netsim_traffic: vec![true; n + 1], live_len: 11_000, track: false,
            listing: vec![None; n + 1],
            netsim_late: vec![Some(0.4); n + 1], netsim_loss_scale: vec![1.0; n + 1],
        };
        r.set_netsim();
        r
    }
    /// netsim<i>.jsonl netsim_start per impaired instance, from run.json's profiles
    fn set_netsim(&mut self) {
        for i in 1..=self.n {
            self.netsim[i].clear();
            let p = self.cfg["netsim"][i.to_string()].as_str().unwrap_or("none").to_string();
            if p == "none" {
                continue;
            }
            let mut st = obj(json!({"ev": "netsim_start", "wall_ms": T0 - 3000, "listen": format!("127.0.0.1:{}", 7790 + i),
                                    "upstream": "127.0.0.1:7777", "profile": p, "pid": 5000 + i}));
            st.extend(obj(profile_impair(&p)));
            self.netsim[i].push(Value::Object(st));
        }
    }
    fn ev(&mut self, i: usize, ev: &str, dt: i64, f: Value) {
        self.seq[i] += 1;
        let modname = match ev {
            "lobby_ready" | "cmd_sent" | "cmd_result" | "x_autotest_cmd" => "HSMPMenu",
            "kit_verified" | "pawn_state" => "HSMPLoadout",
            "pose_quality" | "netfeel" | "spawn_stretch" => "HSMPAvatars",
            _ => "HSMPMatch",
        };
        let mut rec = obj(json!({"v": 1, "ev": ev, "inst": i.to_string(), "mod": modname, "seq": self.seq[i],
                                 "t_ms": self.t + dt - T0 + 5000, "wall_ms": self.t + dt}));
        for (k, v) in obj(f) {
            rec.insert(k, v);
        }
        self.inst[i].push(Value::Object(rec));
    }
    /// hsmp-server --events phase (session.rs observe_phase)
    fn srv(&mut self, to: &str, dt: i64, round: i64) {
        let label = |p: &str| p.to_string();
        let rec = json!({"v": 1, "ev": "phase", "inst": "server", "seq": self.server.len() + 1, "t_ms": self.t + dt - T0,
                         "wall_ms": self.t + dt, "from": label(&self.phase), "to": label(to), "match_id": self.match_id, "round": round,
                         "arena": self.frozen.clone().unwrap_or_else(|| self.arena.clone()), "frozen_arena": self.frozen, "epoch": self.epoch});
        self.phase = to.into();
        self.server.push(rec);
    }
    fn srv_ev(&mut self, ev: &str, dt: i64, f: Value) {
        let mut rec = obj(json!({"v": 1, "ev": ev, "inst": "server", "seq": self.server.len() + 1, "t_ms": self.t + dt - T0, "wall_ms": self.t + dt}));
        rec.extend(obj(f));
        self.server.push(Value::Object(rec));
    }
    fn h(&mut self, ev: &str, dt: i64, f: Value) {
        let mut rec = obj(json!({"ev": ev, "wall_ms": self.t + dt}));
        for (k, v) in obj(f) {
            rec.insert(k, v);
        }
        self.harness.push(Value::Object(rec));
    }
    /// frame_hb every 10 s up to `until` for every instance (Shape::Contract)
    fn heartbeat(&mut self, until: i64) {
        if !self.hb {
            return;
        }
        for i in 1..=self.n {
            while self.hb_next[i] <= until {
                let at = self.hb_next[i];
                self.seq[i] += 1;
                let n = (at - T0) / 10_000 + 1;
                self.inst[i].push(json!({"v": 1, "ev": "frame_hb", "inst": i.to_string(), "mod": "HSMPMatch", "seq": self.seq[i],
                                         "t_ms": at - T0 + 5000, "wall_ms": at, "n": n, "max_ms": 40 + (n % 7) * 3}));
                self.hb_next[i] += 10_000;
            }
        }
    }
    fn adv(&mut self, ms: i64) {
        self.t += ms;
        self.heartbeat(self.t);
    }
    fn lobby(&mut self, epoch: i64) {
        for i in 1..=self.n {
            let role = if i == 1 { "host" } else { "join" };
            self.ev(i, "lobby_ready", 200 * i as i64, json!({"epoch": epoch, "peer_id": i, "notice": null, "role": role, "arena": ""}));
            if self.seq[i] <= 2 {
                // hsmp_saveguard set_state: ACTIVE when the session appears
                self.ev(i, "x_save_guard", 250, json!({"active": true, "why": "session", "session": 1}));
            }
        }
        self.adv(2000);
    }
    /// A menu command answered by the server: cmd_sent, the server's cmd_result, the sidecar's
    /// tap S2G cmd_result record, the menu's cmd_result{source="server"}.
    fn command(&mut self, i: usize, cmd: &str, arg: Option<&str>, ok: bool, dt: i64) -> i64 {
        self.cmd_n += 1;
        let id = 1_790_880_000 + self.cmd_n;
        self.ev(i, "cmd_sent", dt, json!({"cmd": cmd, "cmd_id": id, "arg": arg, "backend": "legacy", "via": "autotest"}));
        let reason = if ok { Value::Null } else { json!("start blocked: 1 of 2 peers ready") };
        let code = if ok { "OK" } else { "NOT_ALL_READY" };
        self.srv_ev("cmd_result", dt + 120, json!({"player": format!("HSMP{i}"), "seat": i, "peer_id": i, "source": "command", "cmd": cmd,
                                                   "cmd_id": id, "ok": ok, "reason": reason, "code": code, "config_rev": 3}));
        let w = self.t + dt + 160;
        self.results[i].push(json!({"cmd_id": id, "ok": ok, "reason_code": code, "code": if ok { 0 } else { 3 },
                                    "reason_text": if ok { "" } else { "start blocked: 1 of 2 peers ready" }, "config_rev": 3, "cmd": cmd, "wall_ms": w}));
        self.ev(i, "cmd_result", dt + 300, json!({"cmd": cmd, "cmd_id": id, "ok": ok, "reason": reason, "state": if ok { "accepted" } else { "refused" },
                                                  "source": "server", "ms": 300, "tries": 1}));
        id
    }
    fn round(&mut self, arena: &str, rnd: i64, seat: usize, first: bool, bad: &Bad) {
        let path = format!("Map_Arena_{arena}");
        if first {
            self.arena = path.clone();
            self.frozen = Some(path.clone());
            self.srv_ev("arena_picked", 100, json!({"arena": path, "by": "rcon"}));
        }
        self.srv("Loading", if first { 300 } else { 0 }, rnd);
        let n = self.n;
        let full = self.shape == Shape::Contract;
        for i in 1..=n {
            let frm = if first { "Map_Menu_Startup".to_string() } else { path.clone() };
            self.ev(i, "travel", 600, json!({"from": frm, "to": path, "by": "director", "reason": "server phase loading"}));
            let world = if bad.world.0 == i { bad.world.1.to_string() } else { path.clone() };
            let wk = format!("{world}#{rnd}");
            self.ev(i, "world_ready", 4000 + 100 * i as i64, json!({"arena": world, "world_key": wk, "round": rnd, "server_arena": path}));
            let d = if bad.dist.0 == i { bad.dist.1 } else { 30.0 };
            let x = if bad.close == i { 400.0 + 80.0 } else { 400.0 * i as f64 };
            let z = if bad.sunk == i { 20.0 } else { 100.0 };
            let vit = bad.vitals != i;
            let mut sv = json!({"round": rnd, "ok": vit, "arena": world, "world_key": wk, "vitals_ok": vit, "gi_ok": true,
                                "x": x, "y": 0.0, "z": z, "spawn_id": 256 + i, "dist_cm": d, "steps": "gi=ok vitals=ok place=hsmpsync(id=257 tries=1 30cm) kit=ok census=ok",
                                "load_error": null, "ms": 4200});
            if full {
                sv["snap_z"] = json!(100.0);
            }
            self.ev(i, "spawn_verified", 4600, sv);
            let r_cls = if bad.rhand == i { "None" } else { "BP_Sword_C" };
            self.ev(i, "kit_verified", 4700, json!({"who": "self", "armour_n": 7, "r_class": r_cls, "l_class": "None", "ok": r_cls != "None",
                                                     "exp_armour_n": 7, "round": rnd, "kit": "Knight", "tries": 1}));
            if full {
                for j in (1..=n).filter(|j| *j != i) {
                    self.ev(i, "kit_verified", 4800, json!({"who": format!("peer:{j}"), "armour_n": 7, "r_class": "BP_Sword_C", "l_class": "None", "ok": true,
                                                             "exp_armour_n": 7, "round": rnd, "kit": "Knight", "tries": 1}));
                }
            }
            self.ev(i, "willie_census", 4900, json!({"visible": n, "expected": n, "extras": 0, "missing": 0, "at": "ready", "round": rnd}));
            self.ev(i, "ready_report", 5000, json!({"round": rnd, "arena": path, "load_error": null}));
            // HSMPSync spawn_place emit_state: placed, ready, protect (every 5 s), live
            for (at, dt) in [("placed", 4100), ("ready", 4950), ("protect", 7000), ("live", 9100)] {
                let (mut wr, mut cons, mut downed) = ("ModularWeaponBP_Sword_C".to_string(), 100.0, false);
                if bad.pawn.0 == i && at == "protect" {
                    match bad.pawn.1 {
                        "fists" => wr = "Weapon_Fists_C".into(),
                        "downed" => { cons = 74.0; downed = true; }
                        _ => {}
                    }
                }
                let pd = if bad.pawn.0 == i && bad.pawn.1 == "far" { 140 } else { 30 };
                self.ev(i, "pawn_state", dt, json!({"at": at, "round": rnd, "pawn": format!("Willie_BP_C_{}", 2147481200 + i), "consciousness": cons,
                                                     "downed": downed, "fallen": false, "health": 100, "weapon_r": wr, "weapon_l": "None",
                                                     "protected": true, "live": at == "live", "dist_cm": pd}));
                if bad.pawn.0 == i && bad.pawn.1 == "fists" && at == "protect" {
                    // HSMPLoadout kit.lua state_event: the dropped kit weapon re-equipped before Live
                    self.ev(i, "pawn_state", dt + 400, json!({"at": "rearm", "who": "self", "pawn": format!("Willie_BP_C_{}", 2147481200 + i), "round": rnd,
                                                               "consciousness": 100, "downed": false, "fallen": false, "health": 100,
                                                               "weapon_r": "ModularWeaponBP_Sword_C", "weapon_l": "None", "reason": "R (Weapon_Fists); window: countdown"}));
                }
            }
        }
        self.srv("Countdown", 6000, rnd);
        self.srv("Live", 9000, rnd);
        let live_end = 9000 + self.live_len;
        // HSMPWorld world_consistency (hsmp_log writes the caller's seq as f_seq) every 5 s
        for (k, dt) in [(0i64, 7_500), (1, 11_000), (2, 16_000)] {
            if dt >= live_end {
                continue;
            }
            for i in 1..=n {
                let div = bad.world_div == i && k > 0;
                let peer = if i == 1 { 2 } else { 1 };
                self.ev(i, "world_consistency", dt, json!({"hash_match": !div, "mismatched": if div { json!([1318867211]) } else { json!({}) }, "mismatched_n": if div { 1 } else { 0 },
                                                         "compared": 40, "hash_equal": !div, "peer": peer, "f_seq": 2 * rnd + k, "level": 3458262474u64,
                                                         "epoch": 1, "world": path}));
            }
        }
        // HSMPWorld world_track (server clock = wall + 1500 ms): one body thrown at 300 cm/s,
        // instance 2 sampling 37 ms later and seeing it 100 ms behind; then the rest poses
        if self.track {
            for i in 1..=n {
                let off = if bad.world2 == i { 300.0 } else { 0.0 };
                let (phase, lag) = if i == 1 { (0i64, 0.0) } else { (37, 0.1) };
                for k in 0..30i64 {
                    let dt = 10_000 + 100 * k + phase;
                    let tt = (dt as f64 / 1000.0 - 10.0 - lag).max(0.0);
                    self.ev(i, "world_track", dt, json!({"nid": 1318867211u64, "t": self.t + dt + 1500, "x": 300.0 * tt + off, "y": 50.0, "z": 30.0,
                                                       "qx": 0.0, "qy": 0.0, "qz": 0.0, "qw": 1.0, "mode": if i == 1 { "own" } else { "follow" },
                                                       "owner": 1, "rest": false, "level": 3458262474u64, "epoch": 1}));
                }
                for (dt, x) in [(13_200i64, 900.0), (16_200, 900.0)] {
                    let rx = if bad.world2 == i { x + 10.0 } else { x };
                    self.ev(i, "world_track", dt + phase, json!({"nid": 1318867211u64, "t": self.t + dt + phase + 1500, "x": rx, "y": 50.0, "z": 30.0,
                                                               "qx": 0.0, "qy": 0.0, "qz": 0.0, "qw": 1.0, "mode": "free", "owner": 0, "rest": true,
                                                               "level": 3458262474u64, "epoch": 1}));
                }
                self.ev(i, "world_sync_quality", 15_000, json!({"hard_snaps": if bad.world2 == i { 2 } else { 0 }, "max_off_cm": 12, "lost_races": 0,
                                                              "takeovers": 0, "poked": if i == 1 { 1 } else { 0 }, "follow_ticks": 180, "window_s": 5.0,
                                                              "level": 3458262474u64, "epoch": 1}));
            }
        }
        // HSMPCombat combat_quality every 5 s of combat (Shape::Contract: the players fight)
        if full {
            for (k, dt) in [(1i64, 14_000), (2, 19_000)] {
                if dt >= live_end {
                    continue;
                }
                for i in 1..=n {
                    let leak = bad.combat == i;
                    let (acc, rej) = if leak { (6, json!({"blade_miss": 6})) } else { (12, json!({})) };
                    self.ev(i, "combat_quality", dt, json!({"claims": 12, "accepted": acc, "rejected_by_reason": rej, "confirmed": acc, "clashes": 0,
                                                          "pending": if leak { 3 + 9 * (k - 1) } else { 1 }, "round": rnd, "window_s": 5}));
                }
            }
        }
        if full {
            for k in [1i64, 2].into_iter().filter(|k| 9500 + 5000 * k < live_end) {
                for i in 1..=n {
                    let vis = if k == 2 && bad.census.0 == i { bad.census.1 } else { n };
                    self.ev(i, "willie_census", 9000 + 5000 * k, json!({"visible": vis, "expected": n, "extras": vis.saturating_sub(n), "missing": n.saturating_sub(vis), "at": "live", "round": rnd}));
                }
            }
            for k in [1i64, 2].into_iter().filter(|k| 9500 + 5000 * k < live_end) {
                for i in 1..=n {
                    let [arm, tip, lat, jit, foot, idle] = if bad.pose.0 == i { bad.pose.1 } else { self.pose };
                    for j in (1..=n).filter(|j| *j != i) {
                        let mut f = json!({"peer": j, "arm_p95_uu": arm, "tip_p95_uu": tip, "latency_ms": lat, "round": rnd});
                        if !self.pose_legacy {
                            f["jitter_ratio"] = json!(jit);
                            f["foot_slide_p95"] = json!(foot);
                            f["idle_rms"] = json!(idle);
                        }
                        self.ev(i, "pose_quality", 9500 + 5000 * k, f);
                        self.ev(i, "netfeel", 9500 + 5000 * k, json!({"peer": j, "frames": 290, "snaps_per_min": 2.0, "jump_max_uu": 4.0,
                            "rigid_snaps": 0, "clock_resets": 0, "window_s": 5.0, "round": rnd}));
                        if k == 1 {
                            self.ev(i, "spawn_stretch", 9400, json!({"who": "standin", "peer": j, "max_uu": 2.5, "bone": "hand_r", "why": "drive start", "frames": 180, "round": rnd}));
                        }
                    }
                }
            }
        }
        self.h("rcon", live_end - 500, json!({"cmd": format!("DEBUG KILL {seat}"), "ok": true, "reply": "OK killed"}));
        self.srv_ev("debug_kill", live_end - 490, json!({"seat": seat, "ok": true, "detail": "killed"}));
        self.srv("RoundOver", live_end, rnd);
        self.h("mark", live_end + 100, json!({"name": "round_end", "round": rnd}));
        self.adv(23000);
    }
    fn arena_block(&mut self, arena: &str, rounds: i64, bad_round: i64, bad: &Bad, skip: i64) {
        self.match_id += 1;
        self.h("rcon", 0, json!({"cmd": format!("MAP {arena}"), "ok": true, "reply": "OK", "pick": arena}));
        self.h("mark", 50, json!({"name": "start", "arena": arena}));
        self.h("rcon", 60, json!({"cmd": "START", "ok": true, "reply": "OK"}));
        for r in 1..=(rounds - skip) {
            let seat = ((r - 1) as usize % self.n) + 1;
            let b = if r == bad_round { bad.clone() } else { Bad::none() };
            self.round(arena, r, seat, r == 1, &b);
        }
        self.h("rcon", 0, json!({"cmd": "ABORT", "ok": true, "reply": "OK"}));
        let last = rounds - skip;
        self.srv("MatchOver", 400, last); // the session protocol's label; the gate maps it to PostMatch
        self.frozen = None;
        self.srv("Lobby", 500, last);
        for i in 1..=self.n {
            self.ev(i, "travel", 900, json!({"from": format!("Map_Arena_{arena}"), "to": "Map_Menu_Startup", "by": "director", "reason": "match over"}));
        }
        self.adv(3000);
        self.lobby(self.epoch);
    }

    fn server_log_lines(&self) -> Vec<String> {
        let msg = |to: &str| match to {
            "Loading" => "match: countdown begins (waiting for clients to load)",
            "Countdown" => "match: all clients loaded; countdown running",
            "Live" => "match: live",
            "RoundOver" => "match: round over, settling 1500 ms for trades",
            "Lobby" => "match: all participants gone; back to lobby",
            "Paused" => "match: participant dropped; pausing for reconnect",
            _ => "?",
        };
        let ts = |ms: i64| util::ms_to_iso(ms).replace('Z', "000Z");
        let mut out = vec![format!("\x1b[2m{}\x1b[0m \x1b[32m INFO\x1b[0m \x1b[2mhsmp_server\x1b[0m\x1b[2m:\x1b[0m hsmp-server starting \x1b[3mbind\x1b[0m\x1b[2m=\x1b[0m127.0.0.1:7777", ts(T0 - 5000))];
        for e in self.server.iter().filter(|e| e["ev"] == "phase") {
            let w = e["wall_ms"].as_i64().unwrap();
            let to = e["to"].as_str().unwrap_or("");
            if to == "Loading" && e["from"] == "Lobby" {
                if let Some(a) = e["frozen_arena"].as_str() {
                    out.push(format!("{}  INFO hsmp_server::server: match start: arena locked arena={a}", ts(w)));
                }
            }
            let fields = e["round"].as_i64().map(|r| format!(" round={r}")).unwrap_or_default();
            out.push(format!("\x1b[2m{}\x1b[0m \x1b[32m INFO\x1b[0m \x1b[2mhsmp_server::server\x1b[0m\x1b[2m:\x1b[0m {}{fields}", ts(w), msg(to)));
        }
        out
    }

    fn write(&mut self, expected: Value, server_as_log: bool, inst_as_ue4ss: bool) {
        let last = self.inst.iter().flatten().filter_map(|e| e["wall_ms"].as_i64()).max().unwrap_or(self.t);
        self.heartbeat(last + 1);
        let d = &self.dir;
        let dump = |p: PathBuf, rows: &[Value]| {
            let s: String = rows.iter().map(|r| r.to_string() + "\n").collect();
            std::fs::write(p, s).unwrap();
        };
        util::write_json(&d.join("run.json"), &self.cfg).unwrap();
        if server_as_log {
            std::fs::write(d.join("server.log"), self.server_log_lines().join("\n") + "\n").unwrap();
        } else if !self.server.is_empty() {
            let mut rows = self.server.clone();
            rows.sort_by_key(|r| r["wall_ms"].as_i64().unwrap_or(0));
            dump(d.join("server.jsonl"), &rows);
        }
        if inst_as_ue4ss {
            let mut rows: Vec<&Value> = self.inst.iter().flatten().collect();
            rows.sort_by_key(|r| r["wall_ms"].as_i64().unwrap_or(0));
            let mut s = String::from("[12:00:00] UE4SS started\n");
            for r in rows {
                s += &format!("[12:00:01] [Lua] [hsmp_ev] {r}\n[12:00:01] [HSMPMatch] something unrelated\n");
            }
            for l in &self.ue4ss {
                s += &format!("{l}\n");
            }
            std::fs::write(d.join("UE4SS.log"), s).unwrap();
        } else {
            if !self.ue4ss.is_empty() {
                std::fs::write(d.join("UE4SS.log"), self.ue4ss.join("\n") + "\n").unwrap();
            }
            for (i, rows) in self.inst.iter().enumerate().skip(1) {
                if !rows.is_empty() {
                    let id = d.join(format!("inst{i}"));
                    std::fs::create_dir_all(&id).unwrap();
                    let mut rows = rows.clone();
                    rows.sort_by_key(|r| r["wall_ms"].as_i64().unwrap_or(0));
                    dump(id.join("hsmp_events.jsonl"), &rows);
                }
            }
        }
        for (i, rows) in self.results.iter().enumerate().skip(1) {
            if !rows.is_empty() {
                let id = d.join(format!("inst{i}"));
                std::fs::create_dir_all(&id).unwrap();
                // the sidecar tap's S2G cmd_result records (docs/development/ipc-shared-memory.md)
                let s: String = rows.iter().map(|r| format!("{{\"t\":0,\"ev\":\"s2g\",\"kind\":\"cmd_result\",\"v\":{r}}}\n")).collect();
                std::fs::write(id.join(crate::events::TAP_FILE), s).unwrap();
            }
        }
        // mp_test.ps1 Collect-State: the final state-dir listing (STATE-1)
        for (i, names) in self.listing.iter().enumerate().skip(1) {
            if let Some(names) = names {
                let id = d.join(format!("inst{i}"));
                std::fs::create_dir_all(&id).unwrap();
                std::fs::write(id.join("state_dir_listing.txt"), names.iter().map(|n| format!("{n}\n")).collect::<String>()).unwrap();
            }
        }
        // server/src/sidecar/main.rs career_guard_event: {action,file,why,kind,backup,ev,wall_ms,pid}
        if self.career_guard {
            for i in 1..=self.n {
                let bk = format!("C:/Users/x/AppData/Local/HSMP/save_backups/20261002-000000-00{i}");
                let mut rows = vec![json!({"action": "backup", "file": null, "why": "2 career file(s), 20480 bytes backed up", "kind": "enter",
                                           "backup": bk, "ev": "career_guard", "wall_ms": T0 + 150 * i as i64, "pid": 6000 + i})];
                rows.extend(self.cg_extra.iter().filter(|(j, _)| *j == i).map(|(_, v)| v.clone()));
                if rows.iter().any(|r| r["kind"] == "leave") {
                    let id = d.join(format!("inst{i}"));
                    std::fs::create_dir_all(&id).unwrap();
                    dump(id.join(".career_guard.jsonl"), &rows);
                    continue;
                }
                rows.push(json!({"action": "clean", "file": null, "why": "2 career file(s) unchanged", "kind": "leave",
                                 "backup": bk, "ev": "career_guard", "wall_ms": self.t + 1000, "pid": 6000 + i}));
                let id = d.join(format!("inst{i}"));
                std::fs::create_dir_all(&id).unwrap();
                dump(id.join(".career_guard.jsonl"), &rows);
            }
        }
        for (i, rows) in self.netsim.iter().enumerate().skip(1) {
            if !rows.is_empty() {
                // hsmp-tools netsim stats_json every 5 s: cumulative counters, ~60 packets/s each way
                let mut rows = rows.clone();
                let traffic = self.netsim_traffic[i];
                // the profile's loss (netsim_start.loss), scaled: the proxy's measured loss
                let loss = rows[0]["loss"].as_f64().unwrap_or(1.0) * self.netsim_loss_scale[i];
                let mut t = T0 - 2000;
                while t <= self.t + 6000 {
                    let k = if traffic { ((t - T0 + 2000).max(0) / 1000) * 60 } else { 0 };
                    let lost = (k as f64 * loss / 100.0) as i64;
                    let mut row = json!({"ev": "netsim_stats", "wall_ms": t, "listen": format!("127.0.0.1:{}", 7790 + i), "in": k, "out": k - lost,
                                         "lost": lost, "dup": k / 300, "blackout_dropped": 0, "queued": 0, "clients": if traffic && t > T0 { 1 } else { 0 }});
                    // per-window scheduling lateness (null percentiles when nothing was delivered)
                    if let Some(p99) = self.netsim_late[i] {
                        let n = if traffic && t > T0 { 300 } else { 0 };
                        row["late_n"] = json!(n);
                        for (key, v) in [("late_p50_ms", 0.05), ("late_p99_ms", p99), ("late_max_ms", p99 * 2.0 + 0.3)] {
                            row[key] = if n > 0 { json!(v) } else { Value::Null };
                        }
                    }
                    rows.push(row);
                    t += 5000;
                }
                dump(d.join(format!("netsim{i}.jsonl")), &rows);
            }
        }
        if !self.harness.is_empty() {
            dump(d.join("harness.jsonl"), &self.harness);
        }
        if !self.observed.is_empty() {
            dump(d.join("observed.jsonl"), &self.observed);
        }
        if !self.mem.is_empty() {
            dump(d.join("mem.jsonl"), &self.mem);
        }
        if !self.no_triage {
            let new: Vec<(&str, &str, &str, i64)> = self.crashes.iter().map(|(a, b, c, t)| (a.as_str(), b.as_str(), c.as_str(), *t)).collect();
            util::write_json(&d.join("crash_triage.json"), &soak::fixture_triage(T0 - 10_000, self.t + 60_000, &new)).unwrap();
        }
        if let Some(g) = &self.g0 {
            util::write_json(&d.join("g0.json"), g).unwrap();
        }
        util::write_json(&d.join("baseline.json"), &self.baseline).unwrap();
        util::write_json(&d.join("final.json"), &self.fin).unwrap();
        util::write_json(&d.join("expected.json"), &expected).unwrap();
    }
}

/// A g0.json as `hsmp-gate g0 --quick --json` writes it (every required check; `fail` names
/// the checks that failed).
fn g0_doc(fail: &[&str]) -> Value {
    let mut checks = Map::new();
    for k in g0::REQUIRED_CHECKS {
        let st = if *k == "cargo" { "skip" } else if fail.contains(k) { "fail" } else { "pass" };
        checks.insert(k.to_string(), json!({"status": st, "summary": if *k == "cargo" { "--quick" } else { "fixture" }}));
    }
    json!({"commit": COMMIT, "tree_dirty": false, "time": util::ms_to_iso(T0 - 60_000), "quick": true, "ok": fail.is_empty(), "checks": checks})
}

#[derive(Clone)]
struct Bad {
    world: (usize, &'static str),
    census: (usize, usize),
    rhand: usize,
    dist: (usize, f64),
    pose: (usize, [f64; 6]),
    /// this instance's pawn lands 80 cm from instance 1's
    close: usize,
    /// this instance's pawn is 80 cm below its snap Z
    sunk: usize,
    /// vitals_ok=false (career wounds)
    vitals: usize,
    /// PAWN-1: this instance's protected pawn is "fists" (dropped weapon + re-arm), "downed" or "far"
    pawn: (usize, &'static str),
    /// WORLD-1: this instance's world diverges (the same id mismatched in consecutive verdicts)
    world_div: usize,
    /// COMBAT-1: this instance's claims leak (pending grows) and half are rejected
    combat: usize,
    /// WORLD-2: this instance's world_track is 3 m off the other's, its rest pose 10 cm off,
    /// and it reports hard snaps
    world2: usize,
}
impl Bad {
    fn none() -> Bad {
        Bad { world: (0, ""), census: (0, 0), rhand: 0, dist: (0, 0.0), pose: (0, [0.0; 6]), close: 0, sunk: 0, vitals: 0,
              pawn: (0, ""), world_div: 0, combat: 0, world2: 0 }
    }
}

/// mem.jsonl rows for one process: every `every_s` over `mins` minutes from `t0`;
/// `f(minute, k)` -> (private MB, handles).
fn mem_rows(role: &str, t0: i64, mins: f64, every_s: i64, f: impl Fn(f64, i64) -> (f64, f64)) -> Vec<Value> {
    let inst: String = if role.starts_with("game") || role.starts_with("sidecar") { role.chars().filter(|c| c.is_ascii_digit()).collect() } else { role.into() };
    let n = (mins * 60.0) as i64 / every_s;
    (0..=n)
        .map(|k| {
            let m = (k * every_s) as f64 / 60.0;
            let (mb, h) = f(m, k);
            let pb = (mb * 1024.0 * 1024.0) as u64;
            json!({"ev": "mem", "wall_ms": t0 + k * every_s * 1000, "role": role, "inst": inst, "pid": 1000 + inst.len(),
                   "private_bytes": pb, "working_set": pb / 10 * 9, "handles": h as u64})
        })
        .collect()
}

/// soak fixtures: soak_60m pass / fail, soak_10m without the sampler or the stall probe (incomplete).
fn make_soak(root: &Path) {
    let none = Bad::none();
    let benign = vec![
        "[2026-10-02 00:24:50.24] [Lua] [HSMPLobby] world guard: world changed -> object caches dropped (#1)".to_string(),
        "[2026-10-02 00:24:53.12] FArchiveState::ArIsError = 0x29".to_string(),
        "[2026-10-02 00:25:33.14] [Lua] [HSMPMatch] director ERROR spawn_timeout: not placed on order 257 after 20 s".to_string(),
    ];
    // warm-up ramp 2000 -> 3200 MB in 10 min, then +60 MB/h with +-32 MB noise
    let healthy = |m: f64, k: i64| -> (f64, f64) {
        let mb = if m < 10.0 { 2000.0 + 120.0 * m } else { 3200.0 + 60.0 * (m - 10.0) / 60.0 + (((k * 37) % 9) - 4) as f64 * 8.0 };
        (mb, 1800.0 + (k % 5) as f64)
    };
    let t0 = T0 - 60_000;

    let mut r = Run::new(root, "soak_60m_pass", "soak_60m", &["Alley", "Pit"], 2, "rcon", Shape::Contract);
    r.cfg["netsim"] = json!({"1": "wifi", "2": "wifi"});
    r.set_netsim();
    r.lobby(1);
    r.arena_block("Alley", 2, 0, &none, 0);
    r.arena_block("Pit", 2, 0, &none, 0);
    r.ev(1, "hitch", 500, json!({"ms": 4200, "travel": true, "world_key": "Map_Menu_Startup#0"}));
    r.ev(2, "hitch", 800, json!({"ms": 900, "travel": false, "world_key": "Map_Menu_Startup#0"}));
    // a 2.6 s stall between a travel and its world_ready from an emitter without the flag:
    // the travel window decides (not a failure)
    r.ev(1, "travel", 2000, json!({"from": "Map_Menu_Startup", "to": "Map_Menu_Startup", "by": "director", "reason": "reload"}));
    r.ev(1, "hitch", 3500, json!({"ms": 2600}));
    for role in ["game1", "game2"] {
        r.mem.extend(mem_rows(role, t0, 60.0, 30, healthy));
    }
    r.mem.extend(mem_rows("server", t0, 60.0, 30, |_, k| (30.0 + (k % 3) as f64 * 0.1, 120.0)));
    r.mem.extend(mem_rows("sidecar1", t0 + 20_000, 59.0, 30, |_, k| (12.0 + (k % 2) as f64 * 0.05, 90.0)));
    r.ue4ss = benign.clone();
    let mut exp = all_pass(&[1, 2, 3, 6, 12]);
    for k in ["SOAK-CRASH", "SOAK-MEM", "SOAK-LUAERR", "SOAK-HITCH", "NETSIM"] {
        exp[k] = json!("pass");
    }
    r.write(json!({"verdict": "pass", "rules": exp,
                   "messages": ["game1: private bytes slope", "growth", "during travel (not failing): 2", "0 new dump(s)", "UE4SS.log Lua errors: 0", "heartbeats"]}), false, false);

    let mut r = Run::new(root, "soak_60m_fail", "soak_60m", &["Alley", "Pit"], 2, "rcon", Shape::Contract);
    r.cfg["netsim"] = json!({"1": "wifi", "2": "wifi"});
    r.set_netsim();
    r.lobby(1);
    r.arena_block("Alley", 2, 0, &none, 0);
    r.arena_block("Pit", 2, 0, &none, 0);
    r.ev(2, "hitch", 40_000, json!({"ms": 3500, "travel": false, "world_key": "Map_Menu_Startup#0"}));
    // leak: +200 MB/h and +1000 handles/h after warm-up
    r.mem.extend(mem_rows("game1", t0, 60.0, 30, |m, _| if m < 10.0 { (2000.0 + 120.0 * m, 1800.0) } else { (3200.0 + 200.0 * (m - 10.0) / 60.0, 1800.0 + 1000.0 * (m - 10.0) / 60.0) }));
    // step: +500 MB at minute 50
    r.mem.extend(mem_rows("game2", t0, 60.0, 30, |m, _| (if m < 10.0 { 2000.0 + 130.0 * m } else if m < 50.0 { 3300.0 } else { 3800.0 }, 1800.0)));
    r.mem.extend(mem_rows("sidecar1", t0, 60.0, 30, |m, _| (12.0 + 40.0 * m / 60.0, 90.0)));
    r.ue4ss = benign.clone();
    r.ue4ss.push("[2026-10-02 00:31:02.51] [Lua] Error: ...Mods/HSMPLoadout/Scripts/main.lua:285: attempt to index a nil value (local 'c')".into());
    r.ue4ss.push("[2026-10-02 00:31:02.51] stack traceback:".into());
    r.fin["new_crashes"] = json!(["UECC-Windows-SOAK_0000"]);
    r.fin["alive_at_end"]["game2"] = json!(false);
    let t = r.t;
    r.crashes.push(("UECC-Windows-SOAK_0000".into(), "PAK_ASYNC_READ_OOB".into(), "IoDispatcher".into(), t - 5_000));
    let mut exp = json!({"DoD-2": "fail"});
    for k in ["SOAK-CRASH", "SOAK-MEM", "SOAK-LUAERR", "SOAK-HITCH"] {
        exp[k] = json!("fail");
    }
    r.write(json!({"verdict": "fail", "rules": exp,
                   "messages": ["1 new dump(s) (max 0): PAK_ASYNC_READ_OOB [IoDispatcher]", "game2 was dead before quit_all",
                                "game1: private bytes slope +200.0 MB/h", "growth +500 MB", "game1: handle slope +1000/h",
                                "sidecar1: private bytes slope +40.0 MB/h", "attempt to index a nil value", "UE4SS.log Lua errors: 2",
                                "> 2000 ms outside travel: 1"]}), false, false);

    // soak_10m with no memory sampler and no stall probe: incomplete, never green
    let mut r = Run::new(root, "soak_10m_no_sampler", "soak_10m", &["Alley"], 2, "rcon", Shape::Today);
    r.cfg["netsim"] = json!({"1": "wifi", "2": "wifi"});
    r.set_netsim();
    r.lobby(1);
    r.arena_block("Alley", 2, 0, &none, 0);
    r.ue4ss = benign;
    r.write(json!({"verdict": "incomplete", "rules": {"SOAK-MEM": "incomplete", "SOAK-HITCH": "incomplete", "SOAK-CRASH": "pass", "SOAK-LUAERR": "pass", "DoD-2": "incomplete"},
                   "messages": ["mem.jsonl missing", "no frame_hb heartbeat"]}), false, false);
}

/// The selftest's stand-in repo (`_clean_repo`: no mods or scripts, so the lints and greps find
/// nothing) carries a release template, so DoD-13 can compare a dev deploy's mod table with it.
fn write_clean_repo(root: &Path) {
    let m = root.join("_clean_repo").join("mods");
    std::fs::create_dir_all(&m).unwrap();
    std::fs::write(m.join("mods.release.txt"),
                   "# fixture release template (hsmp-gate make-fixtures)\nHSMPMatch : 1\nHSMPWorld : 1\nHSMPDiag : dev\nHSMPLobby : 0\nKeybinds : dev\n").unwrap();
}

/// A passing p0_gate run on one arena (the base every DoD-13 / DoD-8 / DoD-3 hole fixture edits).
fn p0_base(root: &Path, name: &str) -> Run {
    let mut r = Run::new(root, name, "p0_gate", &["Alley"], 2, "rcon", Shape::Contract);
    r.lobby(1);
    r.arena_block("Alley", 2, 0, &Bad::none(), 0);
    for i in 1..=2 {
        r.command(i, "ready", None, true, 0);
        r.ev(i, "save_redirected", 400, json!({"fn": "SaveGameToSlot", "slot": "GameProgress", "to_slot": format!("HSMP_{i}_GameProgress"), "op": "write", "ok": true, "how": "redirected"}));
    }
    r
}

/// DoD-13, DoD-8 and DoD-3: holes found by mutating a passing run, each with a fixture
/// that catches it.
fn make_g13(root: &Path) {
    write_clean_repo(root);
    let dev_mods = json!({"HSMPMatch": "1", "HSMPWorld": "1", "HSMPDiag": "1", "HSMPLobby": "0", "Keybinds": "1"});

    // the gate's dev profile (release mods + the template's dev-only diagnostics) certifies
    let mut r = p0_base(root, "p0_gate_deploy_dev_pass");
    r.cfg["deploy"]["profile"] = json!("dev");
    r.cfg["deploy"]["mods"] = dev_mods.clone();
    r.write(json!({"verdict": "pass", "rules": {"DoD-13": "pass"},
                   "messages": ["deploy profile: dev (release mods + the template's dev-only diagnostics)", "4 deployed files re-hashed at run start",
                                "the run executed the stamped binaries", "judged by hsmp-gate built from the deployed commit"]}), false, false);

    // -SkipBuild with binaries from another commit, and a "dev" deploy that turned a
    // non-shipped mod on and a shipped one off: the stamp's commit does not describe the build
    let mut r = p0_base(root, "p0_gate_deploy_unproven");
    r.cfg["deploy"]["profile"] = json!("dev");
    r.cfg["deploy"]["mods"] = json!({"HSMPMatch": "1", "HSMPWorld": "0", "HSMPDiag": "1", "HSMPLobby": "1", "Keybinds": "1"});
    r.cfg["deploy"]["skip_build"] = json!(true);
    r.cfg["deploy"]["bins_commit"] = json!("fedcba9876543210fedcba9876543210fedcba98");
    r.cfg["deploy"]["bins_match_commit"] = json!(false);
    r.write(json!({"verdict": "incomplete", "rules": {"DoD-13": "incomplete"},
                   "messages": ["deployed with -SkipBuild: the binaries (built from fedcba9876) are not proven to match the deployed commit",
                                "HSMPWorld (shipped) was off", "HSMPLobby (not shipped) was on"]}), false, false);

    // the same mutation (profile dev + skip_build) on an OLD stamp without hashes
    let mut r = p0_base(root, "p0_gate_deploy_old_stamp");
    for k in ["hashes", "bins_commit", "bins_match_commit", "bin_dir"] {
        r.cfg["deploy"].as_object_mut().unwrap().remove(k);
    }
    r.cfg["deploy"]["profile"] = json!("dev");
    r.cfg["deploy"]["skip_build"] = json!(true);
    r.cfg.as_object_mut().unwrap().remove("deploy_check");
    r.write(json!({"verdict": "incomplete", "rules": {"DoD-13": "incomplete"},
                   "messages": ["the deploy stamp has no file hashes (old build-and-deploy.ps1)", "deployed with -SkipBuild",
                                "deploy profile dev differs from the release mod set: cannot compare (the stamp has no mods table)"]}), false, false);

    // after the deploy, another checkout rebuilt server\target\release (or a file changed):
    // the run executed a server binary that is not the stamped one -> fail
    let mut r = p0_base(root, "p0_gate_deploy_mismatch");
    r.cfg["deploy_check"]["mismatched"] = json!(["hsmp/hsmp-server.exe"]);
    r.cfg["deploy_check"]["bins_used"]["hsmp-server.exe"] = json!("EEEE000000000000000000000000000000000000000000000000000000000000");
    r.write(json!({"verdict": "fail", "rules": {"DoD-13": "fail"},
                   "messages": ["deployed files changed since the deploy stamp: 1 changed [\"hsmp/hsmp-server.exe\"]",
                                "the run executed binaries that are not the deployed ones: [\"hsmp-server.exe\"]"]}), false, false);

    // the run was judged by an hsmp-gate built from another commit (stale tools), and one
    // built from the right commit with uncommitted rule changes
    let mut r = p0_base(root, "p0_gate_stale_gate");
    r.cfg["fixture_gate"] = json!({"commit": "1111111111111111111111111111111111111111", "dirty": "0"});
    r.write(json!({"verdict": "incomplete", "rules": {"DoD-13": "incomplete"},
                   "messages": ["hsmp-gate was built from 1111111111 but the deployed build is 0123456789"]}), false, false);
    let mut r = p0_base(root, "p0_gate_dirty_gate");
    r.cfg["fixture_gate"] = json!({"commit": COMMIT, "dirty": "1"});
    r.write(json!({"verdict": "incomplete", "rules": {"DoD-13": "incomplete"},
                   "messages": ["with uncommitted tools changes (dirty=1)"]}), false, false);

    // Settings.sav written AFTER the guard armed and went off again (the session ended / the
    // sidecar dropped): not a pre-session write -> fail, and the off interval is reported
    let mut r = Run::new(root, "p0_gate_career_guard_gap", "p0_gate", &["Alley"], 2, "rcon", Shape::Contract);
    r.ev(2, "x_save_guard", -400, json!({"active": true, "why": "sidecar live (connected)", "session": 1}));
    r.lobby(1);
    r.arena_block("Alley", 2, 0, &Bad::none(), 0);
    for i in 1..=2 {
        r.command(i, "ready", None, true, 0);
    }
    r.ev(2, "x_save_guard", 100, json!({"active": false, "why": "no .sidecar.json", "session": 1}));
    r.ev(2, "x_save_call", 200, json!({"fn": "SaveGameToSlot", "slot": "Settings", "active": false}));
    let t = r.t;
    r.fin["save_files"]["Settings.sav"]["mtime_ms"] = json!(t + 200);
    r.write(json!({"verdict": "fail", "rules": {"DoD-8": "fail"},
                   "messages": ["Settings.sav: written during the run (size 2048 -> 2048", "guard off after arming: inst 2 off from"]}), false, false);

    // instance 2's proxy delivered packets up to 3.5 ms late at p99 during Live (a starved
    // delivery thread): the delay the run added was not the profile's -> NETSIM incomplete
    let mut r = p0_base(root, "p0_gate_netsim_late");
    r.netsim_late[2] = Some(3.5);
    r.write(json!({"verdict": "incomplete", "rules": {"NETSIM": "incomplete"},
                   "messages": ["the proxy's own scheduling error exceeded the profile: late_p99_ms > 2 in", "worst p99 3.5 ms",
                                "proxy scheduling error p99 <= 0.40 ms"]}), false, false);
    // an old netsim without the lateness measurement cannot certify the profile
    let mut r = p0_base(root, "p0_gate_netsim_unmeasured");
    r.netsim_late = vec![None; 3];
    r.write(json!({"verdict": "incomplete", "rules": {"NETSIM": "incomplete"},
                   "messages": ["no scheduling-lateness fields (late_*): an old netsim"]}), false, false);
    // the proxy dropped nothing although its profile says 1 % -> impaired less
    let mut r = p0_base(root, "p0_gate_netsim_lossless");
    r.adv(60_000);
    r.netsim_loss_scale[2] = 0.0;
    r.write(json!({"verdict": "incomplete", "rules": {"NETSIM": "incomplete"},
                   "messages": ["measured loss 0.00 % over", "below half the profile's 1 %", "measured loss 1.00 % over"]}), false, false);

    // a native OpenLevel rewritten in place to Pit, then a travel to ANOTHER arena: the
    // instance never reached the rewritten destination (any travel within 10 s must not count)
    let mut r = p0_base(root, "p0_gate_rewrite_wrong_travel");
    r.ev(1, "native_travel_rewritten", 0, json!({"from": "Map_Menu_Startup", "to": "Map_Arena_Pit", "soft": false, "phase": "Lobby", "n": 1}));
    r.ev(1, "travel", 500, json!({"from": "Map_Menu_Startup", "to": "Map_Arena_Yard", "by": "director", "reason": "stray"}));
    r.write(json!({"verdict": "fail", "rules": {"DoD-3": "fail"},
                   "messages": ["native_travel_rewritten Map_Menu_Startup->Map_Arena_Pit (in place): has NO world_ready in (or a travel to) the rewritten arena"]}), false, false);
}

fn all_pass(rules: &[u32]) -> Value {
    Value::Object(rules.iter().map(|d| (format!("DoD-{d}"), json!("pass"))).collect())
}

pub fn make(root: &Path) {
    std::fs::create_dir_all(root).unwrap();
    // fixtures are regenerated from scratch: drop cases that no longer exist
    for e in std::fs::read_dir(root).into_iter().flatten().flatten() {
        if e.path().join("expected.json").exists() {
            let _ = std::fs::remove_dir_all(e.path());
        }
    }
    let none = Bad::none();
    let p0_cmds = |r: &mut Run| {
        for i in 1..=2 {
            r.command(i, "ready", None, true, 0);
            r.ev(i, "save_redirected", 400, json!({"fn": "SaveGameToSlot", "slot": "GameProgress", "to_slot": format!("HSMP_{i}_GameProgress"), "op": "write", "ok": true, "how": "redirected"}));
        }
    };
    // 1. full pass, server --events + per-instance JSONL, every contract field present
    let mut r = Run::new(root, "p0_gate_pass", "p0_gate", &["Alley", "Pit"], 3, "rcon", Shape::Contract);
    r.lobby(1);
    r.arena_block("Alley", 3, 0, &none, 0);
    r.arena_block("Pit", 3, 0, &none, 0);
    p0_cmds(&mut r);
    for i in 1..=2 {
        r.ev(i, "hitch", 500, json!({"ms": 4200, "travel": true, "world_key": "Map_Arena_Pit#1"}));
        // the guard's own safe block (no rewrite possible, the save object was nulled): fine
        r.ev(i, "save_redirected", 450, json!({"fn": "SaveGameToSlot", "slot": "Settings", "to_slot": "<blocked>", "op": "write", "ok": true, "how": "blocked"}));
    }
    r.fin["save_hashes"]["HSMP_1_GameProgress.sav"] = json!("cc33"); // save-guard slot: ignored by DoD-8
    let mut rules_pass = all_pass(&[1, 2, 3, 4, 5, 6, 7, 8, 10, 12, 13]);
    for k in ["POSE-1", "NETSIM", "PAWN-1", "WORLD-1", "COMBAT-1"] {
        rules_pass[k] = json!("pass");
    }
    r.write(json!({"verdict": "pass", "rules": rules_pass, "messages": ["full G0 passed for the deployed commit", "proxy ran typical", "game traffic through the proxy", "server answered", "clean spawn", "career guard: 1 backup(s), 1 clean check(s)", "games quit through the menu path"]}), false, false);

    // 1a. STATE-1: a shared-memory run's state dirs hold only config, identity
    // and logs; a process-plumbing file (a pid file) fails the run; a missing listing leaves
    // it incomplete.
    let clean_dir: Vec<&'static str> = vec![".career_guard.jsonl", ".player_key", ".settings.json",
                                            "hsmp_events.jsonl", "hsmp_events.1.jsonl", "identity", "known_servers.json"];
    for (case, stray, verdict, st) in [
        ("p0_gate_state_dir_pass", None, "pass", "pass"),
        ("p0_gate_state_dir_fail", Some(".pid.sidecar.json"), "fail", "fail"),
        ("p0_gate_state_dir_missing", None, "incomplete", "incomplete"),
    ] {
        let mut r = Run::new(root, case, "p0_gate", &["Alley", "Pit"], 3, "rcon", Shape::Contract);
        r.cfg["ipc"] = json!("shm");
        r.lobby(1);
        r.arena_block("Alley", 3, 0, &none, 0);
        r.arena_block("Pit", 3, 0, &none, 0);
        p0_cmds(&mut r);
        if case != "p0_gate_state_dir_missing" {
            r.listing[1] = Some(clean_dir.clone());
            let mut two = clean_dir.clone();
            two.extend(stray);
            r.listing[2] = Some(two);
        }
        let mut rules = all_pass(&[1, 2, 3, 4, 5, 6, 7, 8, 10, 12, 13]);
        for k in ["POSE-1", "NETSIM", "PAWN-1", "WORLD-1", "COMBAT-1"] {
            rules[k] = json!("pass");
        }
        rules["STATE-1"] = json!(st);
        let msg = match st {
            "pass" => "file(s) in the state dir, all allow-listed",
            "fail" => "files outside the state-dir allow-list (IPC belongs in shared memory): .pid.sidecar.json",
            _ => "no inst1/state_dir_listing.txt",
        };
        r.write(json!({"verdict": verdict, "rules": rules, "messages": [msg]}), false, false);
    }

    // 1b. exactly what the tree emits today: the pending emitters keep the verdict incomplete
    let mut r = Run::new(root, "p0_gate_today", "p0_gate", &["Alley", "Pit"], 3, "rcon", Shape::Today);
    r.lobby(1);
    r.arena_block("Alley", 3, 0, &none, 0);
    r.arena_block("Pit", 3, 0, &none, 0);
    p0_cmds(&mut r);
    r.write(json!({"verdict": "incomplete",
                   "rules": {"DoD-1": "pass", "DoD-2": "incomplete", "DoD-3": "pass", "DoD-4": "pass", "DoD-5": "incomplete", "DoD-6": "incomplete",
                             "DoD-7": "incomplete", "DoD-8": "pass", "DoD-10": "pass", "DoD-12": "pass", "DoD-13": "pass", "POSE-1": "incomplete", "NETSIM": "pass",
                             "PAWN-1": "pass", "WORLD-1": "pass", "COMBAT-1": "incomplete"},
                   "messages": ["no frame_hb heartbeat", "spawn_verified lacks snap_z", "no kit_verified{who=\"peer:<id>\"}", "only at=\"ready\" willie_census",
                                "no pose_quality events", "no combat_quality events"]}), false, false);

    // 2. the same through fallbacks: server.log (tracing text) + shared UE4SS.log
    let mut r = Run::new(root, "p0_gate_fallback_logs", "p0_gate", &["Alley", "Pit"], 3, "rcon", Shape::Contract);
    r.lobby(1);
    r.arena_block("Alley", 3, 0, &none, 0);
    r.arena_block("Pit", 3, 0, &none, 0);
    let mut rules_fb = all_pass(&[1, 2, 3, 5, 6, 7]);
    rules_fb["POSE-1"] = json!("pass");
    r.write(json!({"verdict": "incomplete", "rules": rules_fb}), true, true);

    // 3. everything that can go wrong
    let mut r = Run::new(root, "p0_gate_fail", "p0_gate", &["Alley", "Pit"], 3, "rcon", Shape::Contract);
    r.lobby(1);
    let bad = Bad { world: (2, "Hub_Tavern"), census: (1, 3), rhand: 2, dist: (1, 250.0), pose: (1, [7.5, 12.0, 140.0, 1.4, 6.5, 0.9]),
                    close: 2, sunk: 1, vitals: 2, pawn: (2, "fists"), world_div: 1, combat: 2, world2: 0 };
    r.arena_block("Alley", 3, 2, &bad, 0);
    r.arena_block("Pit", 3, 0, &none, 1);
    // a menu command whose server answer never came: the menu's own timeout is no result
    r.ev(1, "cmd_sent", 0, json!({"cmd": "start", "cmd_id": 1790889999, "arg": null, "backend": "legacy", "via": "autotest"}));
    r.ev(1, "cmd_result", 9000, json!({"cmd": "start", "cmd_id": 1790889999, "ok": false, "reason": "the server did not start the match - check that everyone is ready",
                                        "state": "refused", "source": "timeout", "ms": 9000, "tries": 2}));
    // 40 s after the last travel, flagged travel=false by the probe
    r.ev(2, "hitch", 40_000, json!({"ms": 3100, "travel": false, "world_key": "Map_Arena_Pit#2"}));
    // the save guard could not rewrite the slot: the write hit the career file
    r.ev(2, "save_redirected", 0, json!({"fn": "SaveGameToSlot", "slot": "GameProgress", "to_slot": "HSMP_2_GameProgress", "op": "write", "ok": false, "how": "redirected"}));
    r.fin["new_crashes"] = json!(["UECC-Windows-NEW_0000"]);
    let t = r.t;
    r.crashes.push(("UECC-Windows-NEW_0000".into(), "PAK_ASYNC_READ_OOB".into(), "IoDispatcher".into(), t + 30_000));
    r.fin["save_hashes"]["GameProgress.sav"] = json!("ff99");
    r.fin["orphans"] = json!([{"pid": 4242, "name": "hsmp-sidecar.exe"}]);
    r.srv_ev("load_failed", -1000, json!({"peer_id": 2, "seat": 2, "nick": "HSMP2", "error": "spawn_timeout", "round": 2, "match_id": 2}));
    r.srv_ev("round_void", -900, json!({"round": 2, "fighters": 1, "streak": 1, "match_id": 2}));
    // the proxy of instance 2 ran a different profile than run.json claims
    r.netsim[2][0]["profile"] = json!("good");
    r.g0 = Some(g0_doc(&["unsafe"]));
    let mut fail_rules: Value = Value::Object([1, 2, 3, 5, 6, 7, 8, 10, 12, 13].iter().map(|d| (format!("DoD-{d}"), json!("fail"))).collect());
    fail_rules["POSE-1"] = json!("fail");
    fail_rules["NETSIM"] = json!("fail");
    for k in ["PAWN-1", "WORLD-1", "COMBAT-1"] {
        fail_rules[k] = json!("fail");
    }
    r.write(json!({"verdict": "fail", "rules": fail_rules,
                   "messages": ["Pit: 2/3", "Hub_Tavern", "dist_cm=250>100", "cm from inst 1's pawn", "< snap_z 100 - 50", "vitals_ok=false",
                                "UECC-Windows-NEW_0000", "GameProgress.sav", "ok=false (NOT diverted)", "hsmp-sidecar.exe", "R=None",
                                "arm_p95_uu=7.5>5", "tip_p95_uu=12>8", "latency_ms=140>90",
                                "jitter_ratio=1.4>1.2", "foot_slide_p95=6.5>5", "idle_rms=0.9>0.5",
                                "load_failed event(s)", "round_void event(s)", "no server answer; the menu resolved it by timeout",
                                "1 new dump(s): PAK_ASYNC_READ_OOB [IoDispatcher]", "3100 ms at", "proxy ran \"good\"", "unsafe: fail",
                                "weapon_r=Weapon_Fists_C (the kit has a weapon there)", "pawn_state at=rearm",
                                "id(s) 1318867211 mismatched in two consecutive verdicts", "last verdict has hash_match=false",
                                "pending grew 3 -> 12", "accepted 12/24 = 50.0 % (floor 97 % under typical"]}), false, false);

    // 3. WORLD-2: both screens track the thrown body 100 ms apart and agree on its rest pose
    // (pass); one screen 3 m off with its rest pose 10 cm off and two hard snaps (fail)
    for (case, w2, verdict) in [("world_sync_pass", 0usize, "pass"), ("world_sync_fail", 2, "fail")] {
        let mut r = Run::new(root, case, "world_sync", &["Cellar"], 2, "rcon", Shape::Contract);
        r.track = true;
        r.lobby(1);
        let bad = Bad { world2: w2, ..Bad::none() };
        r.arena_block("Cellar", 2, if w2 > 0 { 1 } else { 0 }, &bad, 0);
        let exp = if w2 == 0 {
            json!({"verdict": verdict, "rules": {"WORLD-1": "pass", "WORLD-2": "pass"}, "messages": ["paired samples p50 30 / p95 30 cm (limit 130, typical)"]})
        } else {
            json!({"verdict": verdict, "rules": {"WORLD-2": "fail"},
                   "messages": ["moving bodies p95 270 cm apart > 130 (typical)", "rests 10.0 cm apart", "2 hard snap(s)"]})
        };
        r.write(exp, false, false);
    }

    // 3a. DoD-2 with zero hitch events and no heartbeat: incomplete, never pass
    let mut r = Run::new(root, "p0_gate_no_heartbeat", "p0_gate", &["Alley"], 2, "rcon", Shape::Contract);
    r.hb = false;
    r.lobby(1);
    r.arena_block("Alley", 2, 0, &none, 0);
    p0_cmds(&mut r);
    r.write(json!({"verdict": "incomplete", "rules": {"DoD-2": "incomplete", "DoD-7": "pass", "DoD-10": "pass"},
                   "messages": ["inst 1: no frame_hb heartbeat", "inst 2: no frame_hb heartbeat"]}), false, false);

    // 3b. a heartbeat gap (the game thread hung without a hitch event) fails DoD-2
    let mut r = Run::new(root, "p0_gate_hang_fail", "p0_gate", &["Alley"], 2, "rcon", Shape::Contract);
    r.lobby(1);
    r.arena_block("Alley", 2, 0, &none, 0);
    let gap_lo = r.t + 1_000;
    r.inst[2].retain(|e| !(e["ev"] == "frame_hb" && e["wall_ms"].as_i64().unwrap_or(0) > gap_lo - 30_000));
    r.hb_next[2] = r.t + 60_000;
    r.adv(70_000);
    r.ev(2, "lobby_ready", 0, json!({"epoch": 1, "peer_id": 2, "notice": null, "role": "join", "arena": ""}));
    r.write(json!({"verdict": "fail", "rules": {"DoD-2": "fail"}, "messages": ["no frame_hb for"]}), false, false);

    // 3c. DoD-8: a failed divert fails even when the career file guard restored the hash
    let mut r = Run::new(root, "p0_gate_save_divert_fail", "p0_gate", &["Alley"], 2, "rcon", Shape::Contract);
    r.lobby(1);
    r.arena_block("Alley", 2, 0, &none, 0);
    r.ev(1, "save_redirected", 0, json!({"fn": "SaveGameToSlot", "slot": "GameProgress", "to_slot": "HSMP_1_GameProgress", "op": "write", "ok": false, "how": "redirected"}));
    r.write(json!({"verdict": "fail", "rules": {"DoD-8": "fail"}, "messages": ["1 failed", "ok=false (NOT diverted)", "changed/added/removed: none"]}), false, false);

    // 3c2. DoD-8: the sidecar's career guard restored a career file at leave: the hashes
    // match afterwards, but layer 1 missed a write -> fail
    let mut r = Run::new(root, "p0_gate_career_restored", "p0_gate", &["Alley"], 2, "rcon", Shape::Contract);
    r.lobby(1);
    r.arena_block("Alley", 2, 0, &none, 0);
    let t = r.t;
    r.cg_extra.push((2, json!({"action": "restored", "file": "GameProgress.sav", "kind": "leave", "ev": "career_guard", "wall_ms": t + 900, "pid": 6002,
                                "why": "changed during the MP session; restored from the backup (MP copy kept in mp_modified)",
                                "backup": "C:/Users/x/AppData/Local/HSMP/save_backups/20261002-000000-002"})));
    r.write(json!({"verdict": "fail", "rules": {"DoD-8": "fail"},
                   "messages": ["career guard touched career saves (layer 1 missed a write): restored GameProgress.sav (leave)", "changed/added/removed: none"]}), false, false);

    // 3c3. DoD-8: Settings.sav rewritten with the same bytes during the session: the hash
    // cannot see it, the mtime can -> fail
    let mut r = Run::new(root, "p0_gate_career_same_bytes", "p0_gate", &["Alley"], 2, "rcon", Shape::Contract);
    r.lobby(1);
    r.arena_block("Alley", 2, 0, &none, 0);
    r.fin["save_files"]["Settings.sav"]["mtime_ms"] = json!(T0 + 30_000);
    r.write(json!({"verdict": "fail", "rules": {"DoD-8": "fail"},
                   "messages": ["Settings.sav: written during the run (size 2048 -> 2048", "changed/added/removed: none"]}), false, false);

    // 3c4. DoD-8: the same mtime change, attributed: inst 2's native menu saved Settings before its
    // own guard went active (vanilla pre-session behaviour) -> a note, not a failure
    let mut r = Run::new(root, "p0_gate_career_presession", "p0_gate", &["Alley"], 2, "rcon", Shape::Contract);
    r.ev(2, "x_save_call", -1000, json!({"fn": "DoesSaveGameExist", "slot": "Settings", "active": false}));
    r.ev(2, "x_save_call", -990, json!({"fn": "SaveGameToSlot", "slot": "Settings", "active": false}));
    r.ev(2, "x_save_guard", -400, json!({"active": true, "why": "sidecar live (connected)", "session": 1}));
    r.lobby(1);
    r.arena_block("Alley", 2, 0, &none, 0);
    p0_cmds(&mut r);
    r.fin["save_files"]["Settings.sav"]["mtime_ms"] = json!(T0 - 700);
    r.write(json!({"verdict": "pass", "rules": {"DoD-8": "pass"},
                   "messages": ["pre-session native write by inst 2 (SaveGameToSlot with its guard off"]}), false, false);

    // 3c5. DoD-8: no career save in the baseline proves nothing -> incomplete
    let mut r = Run::new(root, "p0_gate_no_career_saves", "p0_gate", &["Alley"], 2, "rcon", Shape::Contract);
    r.baseline["save_hashes"] = json!({});
    r.baseline["save_files"] = json!({});
    r.fin["save_hashes"] = json!({"HSMP_1_GameProgress.sav": "cc33"});
    r.fin["save_files"] = json!({"HSMP_1_GameProgress.sav": {"size": 10, "mtime_ms": T0 + 5000, "sha256": "cc33"}});
    r.lobby(1);
    r.arena_block("Alley", 2, 0, &none, 0);
    p0_cmds(&mut r);
    r.write(json!({"verdict": "incomplete", "rules": {"DoD-8": "incomplete"},
                   "messages": ["the baseline has no career files: nothing was protected, nothing proven"]}), false, false);

    // 3d. DoD-10: the menu timed out, the server answered twice for one command
    let mut r = Run::new(root, "map_change_cmd_fail", "map_change", &["Alley"], 1, "rcon", Shape::Contract);
    r.lobby(1);
    r.ev(1, "cmd_sent", 0, json!({"cmd": "pick_arena", "cmd_id": 1790881111, "arg": "Map_Arena_Yard", "backend": "legacy", "via": "autotest"}));
    r.ev(1, "cmd_result", 18_000, json!({"cmd": "pick_arena", "cmd_id": 1790881111, "ok": false, "reason": "no answer from the server",
                                          "state": "refused", "source": "timeout", "ms": 18000, "tries": 6}));
    let id = r.command(1, "pick_arena", Some("Map_Arena_Slums"), true, 20_000);
    r.srv_ev("cmd_result", 20_500, json!({"player": "HSMP1", "seat": 1, "peer_id": 1, "source": "command", "cmd": "pick_arena", "cmd_id": id,
                                          "ok": true, "reason": null, "code": "OK", "config_rev": 4}));
    r.write(json!({"verdict": "fail", "rules": {"DoD-10": "fail"}, "messages": ["no server answer; the menu resolved it by timeout", "times (dedup cache missed)"]}), false, false);

    // 3e. -Netsim weaker than the scenario's: cannot certify it
    let mut r = Run::new(root, "p0_gate_netsim_weaker", "p0_gate", &["Alley"], 2, "rcon", Shape::Contract);
    r.cfg["netsim"] = json!({"1": "none", "2": "none"});
    r.set_netsim();
    r.pose = [3.1, 5.2, 30.0, 1.05, 2.0, 0.2];
    r.lobby(1);
    r.arena_block("Alley", 2, 0, &none, 0);
    r.write(json!({"verdict": "incomplete", "rules": {"NETSIM": "incomplete"}, "messages": ["ran under none, weaker than the scenario's typical"]}), false, false);

    // 3e2. NETSIM: instance 2's proxy started with the right profile but carried no game
    // traffic (the client connected to the server directly): not impaired -> fail
    let mut r = Run::new(root, "p0_gate_netsim_bypass", "p0_gate", &["Alley"], 2, "rcon", Shape::Contract);
    r.netsim_traffic[2] = false;
    r.lobby(1);
    r.arena_block("Alley", 2, 0, &none, 0);
    r.write(json!({"verdict": "fail", "rules": {"NETSIM": "fail", "DoD-1": "pass"},
                   "messages": ["the proxy carried no game traffic (in +0, out +0, clients max 0", "proxy ran typical", "growing across 2 round(s)"]}), false, false);

    // 3e3. PAWN-1: instance 1's protected pawn was knocked down before Live; Live lasts 1 s as in
    // the real p0_gate (DEBUG KILL at once), so WORLD-1 and COMBAT-1 cannot judge any round
    let mut r = Run::new(root, "p0_gate_pawn_downed_short_live", "p0_gate", &["Alley"], 2, "rcon", Shape::Contract);
    r.live_len = 1000;
    let bad = Bad { pawn: (1, "downed"), ..Bad::none() };
    r.lobby(1);
    r.arena_block("Alley", 2, 1, &bad, 0);
    r.write(json!({"verdict": "fail", "rules": {"PAWN-1": "fail", "WORLD-1": "incomplete", "COMBAT-1": "incomplete"},
                   "messages": ["downed=true, consciousness=74 (< 95)", "no round had Live >= 10 s (2 shorter"]}), false, false);

    // 3e4. PAWN-1: a protected pawn 1.4 m from its placement (the DoD-7 limit is 1 m)
    let mut r = Run::new(root, "p0_gate_pawn_far", "p0_gate", &["Alley"], 2, "rcon", Shape::Contract);
    let bad = Bad { pawn: (2, "far"), ..Bad::none() };
    r.lobby(1);
    r.arena_block("Alley", 2, 2, &bad, 0);
    r.write(json!({"verdict": "fail", "rules": {"PAWN-1": "fail", "WORLD-1": "pass", "COMBAT-1": "pass"},
                   "messages": ["dist_cm=140 (> 100 from the placement)"]}), false, false);

    // 3f. an older pose emitter: pose_quality without jitter/foot/idle -> POSE-1 incomplete
    let mut r = Run::new(root, "p0_gate_pose_legacy", "p0_gate", &["Alley"], 2, "rcon", Shape::Contract);
    r.pose_legacy = true;
    r.lobby(1);
    r.arena_block("Alley", 2, 0, &none, 0);
    r.write(json!({"verdict": "incomplete", "rules": {"DoD-1": "pass", "DoD-3": "pass", "POSE-1": "incomplete"},
                   "messages": ["pose_quality lacks foot_slide_p95, idle_rms, jitter_ratio"]}), false, false);

    // 3g. census samples without visible/expected are not judged (incomplete, not pass)
    let mut r = Run::new(root, "p0_gate_census_unjudged", "p0_gate", &["Alley"], 2, "rcon", Shape::Contract);
    r.lobby(1);
    r.arena_block("Alley", 2, 0, &none, 0);
    for rows in r.inst.iter_mut() {
        for e in rows.iter_mut().filter(|e| e["ev"] == "willie_census") {
            let o = e.as_object_mut().unwrap();
            o.remove("visible");
            o.remove("expected");
        }
    }
    r.write(json!({"verdict": "incomplete", "rules": {"DoD-6": "incomplete", "DoD-5": "pass"},
                   "messages": ["without visible/expected (not judged)"]}), false, false);

    // 4. instrumentation not there yet -> incomplete (exit 2), never a false green
    let mut r = Run::new(root, "p0_gate_uninstrumented", "p0_gate", &["Alley"], 2, "rcon", Shape::Today);
    r.lobby(1);
    r.arena_block("Alley", 2, 0, &none, 0);
    for i in 1..=2 {
        r.inst[i].retain(|e| matches!(e["ev"].as_str(), Some("lobby_ready" | "travel")));
    }
    r.write(json!({"verdict": "incomplete", "rules": {"DoD-1": "pass", "DoD-2": "incomplete", "POSE-1": "incomplete", "DoD-3": "incomplete", "DoD-5": "incomplete",
                                                      "DoD-6": "incomplete", "DoD-7": "incomplete", "DoD-8": "incomplete", "DoD-10": "incomplete"}}), false, false);

    // 4b. p0_wifi: POSE-1 under the wifi profile (arm <= 10, no tip bound, latency <= 35 + 40)
    for (name, pose, verdict) in [("p0_wifi_pose_pass", [8.0, 14.0, 70.0, 1.4, 7.0, 0.9], "pass"), ("p0_wifi_pose_fail", [11.0, 6.0, 70.0, 1.6, 9.0, 0.3], "fail")] {
        let mut r = Run::new(root, name, "p0_wifi", &["Alley", "Pit"], 2, "rcon", Shape::Contract);
        r.cfg["netsim"] = json!({"1": "wifi", "2": "wifi"});
        r.set_netsim();
        r.pose = pose;
        r.lobby(1);
        r.arena_block("Alley", 2, 0, &none, 0);
        r.arena_block("Pit", 2, 0, &none, 0);
        let mut exp = json!({"verdict": verdict, "rules": {"DoD-14": "pass", "DoD-12": "pass", "NETSIM": "pass", "POSE-1": verdict}});
        if verdict == "fail" {
            exp["messages"] = json!(["arm_p95_uu=11>10", "jitter_ratio=1.6>1.5", "foot_slide_p95=9>8"]);
        }
        r.write(exp, false, false);
    }

    // 5. the legacy HSMP_AUTOTEST stopgap: server.log + observed state files only
    let mut r = Run::new(root, "p0_smoke_autotest_stopgap", "p0_gate", &["Alley"], 1, "autotest", Shape::Today);
    r.cfg["topology"] = json!("listen");
    r.cfg["netsim"] = json!({"1": "none", "2": "typical"});
    r.set_netsim();
    r.observed = vec![
        json!({"ev": "obs_sidecar", "inst": "1", "status": "connected", "wall_ms": T0 + 1000}),
        json!({"ev": "obs_sidecar", "inst": "2", "status": "connected", "wall_ms": T0 + 3000}),
        json!({"ev": "obs_match", "inst": "1", "state": "live", "arena": "Map_Arena_Alley", "round": 1, "wall_ms": T0 + 60000}),
    ];
    r.arena = "Map_Arena_Alley".into();
    r.frozen = Some("Map_Arena_Alley".into());
    r.srv("Loading", 50000, 1);
    r.srv("Live", 59000, 1);
    r.h("mark", 60000, json!({"name": "live"}));
    r.write(json!({"verdict": "incomplete", "rules": {"DoD-1": "incomplete", "DoD-2": "incomplete", "DoD-8": "incomplete", "DoD-12": "pass"}}), true, false);

    // 6. map_change: three picks, both instances load the last one; every pick answered by the server
    let mut r = Run::new(root, "map_change_pass", "map_change", &["Alley"], 1, "rcon", Shape::Contract);
    r.lobby(1);
    for (k, a) in ["Yard", "Slums", "Cellar"].iter().enumerate() {
        r.h("mark", 0, json!({"name": format!("pick{}", k + 1)}));
        r.h("client_cmd", 10, json!({"inst": "1", "cmd": "pick_arena", "arg": a, "pick": a}));
        r.command(1, "pick_arena", Some(&format!("Map_Arena_{a}")), true, 20);
        r.adv(1000);
    }
    r.h("mark", 0, json!({"name": "start", "arena": "Cellar"}));
    r.command(1, "start", None, true, 20);
    r.match_id += 1;
    r.round("Cellar", 1, 1, true, &none);
    r.write(json!({"verdict": "pass", "rules": {"DoD-4": "pass", "DoD-10": "pass", "DoD-12": "pass", "NETSIM": "pass", "CRASH": "pass"}}), false, false);

    // 6b. the same run, but the last pick's server answer reached the sidecar 2 s AFTER the
    // menu had given up ("no answer from the server", source=timeout): the player saw a refusal
    let mut r = Run::new(root, "map_change_late_answer_fail", "map_change", &["Alley"], 1, "rcon", Shape::Contract);
    r.lobby(1);
    for (k, a) in ["Yard", "Slums"].iter().enumerate() {
        r.h("mark", 0, json!({"name": format!("pick{}", k + 1)}));
        r.h("client_cmd", 10, json!({"inst": "1", "cmd": "pick_arena", "arg": a, "pick": a}));
        r.command(1, "pick_arena", Some(&format!("Map_Arena_{a}")), true, 20);
        r.adv(1000);
    }
    r.h("mark", 0, json!({"name": "pick3"}));
    r.h("client_cmd", 10, json!({"inst": "1", "cmd": "pick_arena", "arg": "Cellar", "pick": "Cellar"}));
    r.ev(1, "cmd_sent", 20, json!({"cmd": "pick_arena", "cmd_id": 1790881790, "arg": "Map_Arena_Cellar", "backend": "legacy", "via": "autotest"}));
    r.ev(1, "cmd_result", 6020, json!({"cmd": "pick_arena", "cmd_id": 1790881790, "ok": false, "reason": "no answer from the server",
                                        "state": "refused", "source": "timeout", "ms": 6000, "tries": 3}));
    r.srv_ev("cmd_result", 7900, json!({"player": "HSMP1", "seat": 1, "peer_id": 1, "source": "command", "cmd": "pick_arena", "cmd_id": 1790881790,
                                         "ok": true, "reason": null, "code": "OK", "config_rev": 4}));
    let w = r.t + 8020;
    r.results[1].push(json!({"cmd_id": 1790881790, "ok": true, "reason_code": "OK", "code": 0, "reason_text": "", "config_rev": 4, "cmd": "pick_arena", "wall_ms": w}));
    r.adv(9000);
    r.h("mark", 0, json!({"name": "start", "arena": "Cellar"}));
    r.command(1, "start", None, true, 20);
    r.match_id += 1;
    r.round("Cellar", 1, 1, true, &none);
    r.write(json!({"verdict": "fail", "rules": {"DoD-4": "pass", "DoD-10": "fail"},
                   "messages": ["pick_arena #1790881790: the server answered, but the menu resolved it by timeout first (answer arrived 2000 ms after the menu gave up)"]}), false, false);

    // 7. start_refused, but instance 2 travelled anyway
    let mut r = Run::new(root, "start_refused_fail", "start_refused", &["Alley"], 1, "rcon", Shape::Contract);
    r.lobby(1);
    r.h("rcon", 0, json!({"cmd": "MAP Pit", "ok": true, "reply": "OK", "pick": "Pit"}));
    r.h("mark", 100, json!({"name": "start_refused"}));
    r.h("rcon", 110, json!({"cmd": "START", "ok": true, "reply": "ERR start blocked: 1 of 2 peers ready"}));
    r.ev(2, "travel", 2000, json!({"from": "Map_Menu_Startup", "to": "Map_Arena_Pit", "by": "menu_legacy"}));
    r.write(json!({"verdict": "fail", "rules": {"DoD-4": "fail"}, "messages": ["1 client travels"]}), false, false);

    // 8. reconnect: paused during the blackout, back to Live, seat restored
    let reconnect = |r: &mut Run| {
        r.lobby(1);
        r.match_id += 1;
        r.arena = "Map_Arena_Alley".into();
        r.frozen = Some("Map_Arena_Alley".into());
        r.srv("Loading", 0, 1);
        r.srv("Live", 9000, 1);
        r.h("mark", 12000, json!({"name": "blackout", "inst": "2"}));
        r.h("netsim", 12010, json!({"inst": "2", "mode": "blackout"}));
        r.srv("Paused", 15000, 1);
        r.h("mark", 20000, json!({"name": "blackout_end"}));
        r.srv_ev("seat_restored", 23000, json!({"player_key": "k2", "seat": 2, "old_seat": 2, "same_seat": true, "wins": 0, "same_wins": true, "match_id": 1}));
        r.srv("Live", 26000, 1);
        r.adv(30_000); // the run goes on (the proxy's stats cover the Live phases)
    };
    let mut r = Run::new(root, "reconnect_pass", "reconnect", &["Alley"], 1, "rcon", Shape::Contract);
    reconnect(&mut r);
    r.write(json!({"verdict": "pass", "rules": {"DoD-11": "pass", "DoD-12": "pass", "NETSIM": "pass", "CRASH": "pass"}}), false, false);

    // 8b. the joiner crashed during the resync just after the blackout. DoD-11 already saw
    // Live again; the crash dir, the dump, game2 dead before quit_all and its dead_before_quit
    // quit path must fail the run (DoD-12 must not count them as "quit through the menu path")
    let mut r = Run::new(root, "reconnect_crash_fail", "reconnect", &["Alley"], 1, "rcon", Shape::Contract);
    reconnect(&mut r);
    r.fin["new_crashes"] = json!(["UECC-Windows-DEAD_0000"]);
    r.fin["alive_at_end"]["game2"] = json!(false);
    r.fin["quit_path"] = json!({"game1": "menu_quit", "game2": "dead_before_quit"});
    let t = r.t;
    r.crashes.push(("UECC-Windows-DEAD_0000".into(), "EXCEPTION_ACCESS_VIOLATION".into(), "GameThread".into(), t + 27_000));
    r.write(json!({"verdict": "fail", "rules": {"DoD-11": "pass", "DoD-12": "fail", "CRASH": "fail"},
                   "messages": ["game(s) dead before quit_all (crashed or exited on their own; no scenario step ended them): game2",
                                "new crash dirs: [\"UECC-Windows-DEAD_0000\"]", "1 new dump(s): EXCEPTION_ACCESS_VIOLATION [GameThread]",
                                "processes dead before teardown: [\"game2\"]"]}), false, false);

    // 8c. a dead_before_quit with no crash dir (the game exited on its own) still fails DoD-12
    let mut r = Run::new(root, "reconnect_exit_fail", "reconnect", &["Alley"], 1, "rcon", Shape::Contract);
    reconnect(&mut r);
    r.fin["quit_path"] = json!({"game1": "menu_quit", "game2": "dead_before_quit"});
    r.write(json!({"verdict": "fail", "rules": {"DoD-11": "pass", "DoD-12": "fail", "CRASH": "pass"},
                   "messages": ["no scenario step ended them): game2"]}), false, false);

    // 9. host_leave (listen): joiner took 8 s and showed no reason
    let mut r = Run::new(root, "host_leave_fail", "host_leave", &["Alley"], 1, "rcon", Shape::Contract);
    r.lobby(1);
    r.h("mark", 0, json!({"name": "host_leave"}));
    r.ev(2, "travel", 8000, json!({"from": "Map_Arena_Alley", "to": "Map_Menu_Startup", "by": "director"}));
    r.write(json!({"verdict": "fail", "rules": {"DoD-11": "fail", "NETSIM": "pass"}, "messages": ["after 8000 ms"]}), false, false);

    // 9b. DoD-12: the listen host quit through the menu, but the server it launched
    // outlived it; the joiner's game had to be killed by PID
    let mut r = Run::new(root, "host_leave_quit_fail", "host_leave", &["Alley"], 1, "rcon", Shape::Contract);
    r.lobby(1);
    r.h("mark", 0, json!({"name": "host_leave"}));
    r.ev(2, "travel", 2000, json!({"from": "Map_Arena_Alley", "to": "Map_Menu_Startup", "by": "director", "reason": "Host closed the server"}));
    r.h("game_force_killed", 30_000, json!({"role": "game2", "pid": 7002, "why": "no exit within 30 s of the menu quit nor 15 s of WM_CLOSE"}));
    r.fin["quit_path"] = json!({"game1": "menu_quit", "game2": "force_killed"});
    r.fin["game_force_killed"] = json!(["game2"]);
    r.fin["orphans"] = json!([{"pid": 4343, "name": "hsmp-server.exe", "parent": 7001, "cmd": "hsmp-server.exe --bind 0.0.0.0:7777", "role": "server", "launched_by": "game"}]);
    r.write(json!({"verdict": "fail", "rules": {"DoD-11": "pass", "DoD-12": "fail"},
                   "messages": ["force-killed by PID at quit_all (neither the menu quit nor WM_CLOSE ended them): game2", "\"launched_by\":\"game\""]}), false, false);

    // 9c. host_leave pass: the listen host leaves, its server exits ON PURPOSE (the step's
    // expect_exit -> harness expected_exit{role=server}): CRASH must not count that as a crash
    for (name, expected_exit) in [("host_leave_pass", true), ("host_leave_server_died_fail", false)] {
        let mut r = Run::new(root, name, "host_leave", &["Alley"], 1, "rcon", Shape::Contract);
        r.lobby(1);
        r.h("mark", 0, json!({"name": "host_leave"}));
        r.h("client_cmd", 10, json!({"inst": "1", "cmd": "leave"}));
        if expected_exit {
            r.h("expected_exit", 11, json!({"role": "server", "why": "step 7 client_cmd leave"}));
        }
        r.ev(2, "travel", 2000, json!({"from": "Map_Arena_Alley", "to": "Map_Menu_Startup", "by": "director", "reason": "Host closed the server"}));
        r.fin["alive_at_end"]["server"] = json!(false);
        if expected_exit {
            r.write(json!({"verdict": "pass", "rules": {"DoD-11": "pass", "DoD-12": "pass", "CRASH": "pass", "NETSIM": "pass"},
                           "messages": ["ended on purpose by a scenario step: [\"server\"]"]}), false, false);
        } else {
            r.write(json!({"verdict": "fail", "rules": {"DoD-11": "pass", "DoD-12": "pass", "CRASH": "fail"},
                           "messages": ["processes dead before teardown: [\"server\"]"]}), false, false);
        }
    }

    // 10. server_restart: new epoch seen by both
    let mut r = Run::new(root, "server_restart_pass", "server_restart", &["Alley"], 1, "rcon", Shape::Contract);
    r.lobby(1);
    r.h("mark", 0, json!({"name": "server_restart"}));
    r.adv(5000);
    r.epoch = 2;
    r.srv_ev("server_start", 100, json!({"epoch": 2, "debug_verbs": true}));
    for i in 1..=2 {
        r.ev(i, "lobby_ready", 1000, json!({"epoch": 2, "peer_id": i + 2, "notice": "Server restarted", "role": if i == 1 { "host" } else { "join" }, "arena": ""}));
    }
    r.write(json!({"verdict": "pass", "rules": {"DoD-11": "pass", "DoD-12": "pass", "NETSIM": "pass", "CRASH": "pass"}}), false, false);

    // 10b. no lobby_ready before the restart -> the "new" epoch is not proven
    let mut r = Run::new(root, "server_restart_no_prior_epoch", "server_restart", &["Alley"], 1, "rcon", Shape::Contract);
    r.adv(2000);
    r.h("mark", 0, json!({"name": "server_restart"}));
    r.adv(5000);
    r.epoch = 2;
    r.srv_ev("server_start", 100, json!({"epoch": 2, "debug_verbs": true}));
    for i in 1..=2 {
        r.ev(i, "lobby_ready", 1000, json!({"epoch": 2, "peer_id": i + 2, "notice": "Server restarted", "role": if i == 1 { "host" } else { "join" }, "arena": ""}));
    }
    r.write(json!({"verdict": "incomplete", "rules": {"DoD-11": "incomplete"},
                   "messages": ["no lobby_ready with an epoch before the restart"]}), false, false);

    make_g13(root);
    make_soak(root);
}

/// Evaluate every fixture against its expected.json. Returns (cases, failing case messages).
pub fn selftest(repo: &Path, verbose: bool) -> (usize, Vec<String>) {
    let root = fixtures_dir(repo);
    let clean = root.join("_clean_repo"); // no mods/scripts: lints are skipped, grep finds nothing
    let mut cases: Vec<PathBuf> = std::fs::read_dir(&root).into_iter().flatten().flatten().map(|e| e.path()).filter(|p| p.join("expected.json").exists()).collect();
    cases.sort();
    let mut bad = vec![];
    for d in &cases {
        let name = d.file_name().unwrap().to_string_lossy().into_owned();
        let exp = util::read_json(&d.join("expected.json")).unwrap_or(json!({}));
        let rep = match rules::evaluate(d, &clean, None) {
            Ok(r) => r,
            Err(e) => {
                bad.push(format!("{name}: {e}"));
                continue;
            }
        };
        let mut probs = vec![];
        if rep["verdict"] != exp["verdict"] {
            probs.push(format!("verdict {} != {}", rep["verdict"], exp["verdict"]));
        }
        for (k, v) in exp["rules"].as_object().into_iter().flatten() {
            if rep["rules"].get(k) != Some(v) {
                probs.push(format!("{k} {} != {v}", rep["rules"].get(k).cloned().unwrap_or(Value::Null)));
            }
        }
        for needle in exp["messages"].as_array().into_iter().flatten().filter_map(|v| v.as_str()) {
            if !rep["checks"].as_array().unwrap().iter().any(|c| c["msg"].as_str().unwrap_or("").contains(needle)) {
                probs.push(format!("no check message containing {needle:?}"));
            }
        }
        if probs.is_empty() {
            if verbose {
                println!("ok   {name}: verdict {}", rep["verdict"].as_str().unwrap_or(""));
            }
        } else {
            if verbose {
                println!("FAIL {name}: {}", probs.join("; "));
                for c in rep["checks"].as_array().into_iter().flatten().filter(|c| c["status"] != "pass") {
                    println!("     {} {}: {}", c["rule"].as_str().unwrap_or(""), c["status"].as_str().unwrap_or(""), c["msg"].as_str().unwrap_or(""));
                }
            }
            bad.push(format!("{name}: {}", probs.join("; ")));
        }
    }
    if cases.is_empty() {
        bad.push(format!("no fixtures under {}", root.display()));
    }
    (cases.len(), bad)
}
