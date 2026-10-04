//! The run loop (1 ms of true time per step), the scenario and the metrics.
//!
//! * Clients run a game frame every ~16.7 ms (±2 ms, their own phase): HSMPWorld's tick,
//!   then their physics. Their sidecar pass runs every 25 ms.
//! * The server runs the world tick every 33 ms (lease expiry, coalesced fan-out with
//!   relevance and budget, heals, keyframes), and dispatches records like world_glue.rs.
//! * Links: each client has an up and a down link with the profile; `world_state` rides the
//!   unreliable channel (Latest), everything else the reliable one.
//! * Stand-ins: client c shows peer p's pawn where it was one pose-path latency ago
//!   (both links + the pose jitter buffer, `pose_delay`), and that stand-in pushes c's bodies.
//!
//! The scenario is driven with mod-side actions only: pawns walking (into props), the
//! game's own pickup (a Willie's "Weapon R"), throws (the body's velocity at release).

use crate::game::{Client, SceneBody};
use crate::net::{Link, Profile};
use crate::phys::{self, Pusher, Q, V3};
use crate::world::{self, ClaimResult, Msg, World, WorldEntry};
use crate::Rng;
use hsmp_ipc::record::view;
use hsmp_ipc::schema::world as rec;
use std::collections::{BTreeMap, BinaryHeap, HashMap, HashSet};
use std::time::{Duration, Instant};

pub const LEVEL_WORLD: &str = "World /Game/Maps/SimArena.SimArena";
const FRAME_MS: f64 = 16.7;
const SIDECAR_MS: f64 = 25.0;
const SERVER_MS: f64 = 33.0;
const SAMPLE_MS: f64 = 20.0;
const HAND_OFF: V3 = [35.0, 20.0, 35.0];

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Act {
    /// The pawn picks up scene body `idx` (a weapon) in its right hand.
    Hold(usize),
    /// The pawn lets go of its right-hand weapon with this velocity (cm/s).
    Throw(V3),
    /// A new client joins (late join).
    Join,
    /// Round reset: the server bumps the world epoch; every arena reloads 800 ms later.
    Reset,
    /// Every arena reloads (the second half of a reset).
    Reload,
    /// The harness's world_poke: throw the n free bodies nearest the pawn (cm/s, n).
    Poke(f64, i64),
}

/// One phase of the scenario: a labelled time window for the metrics.
#[derive(Clone, Debug)]
pub struct Phase { pub name: &'static str, pub from: f64, pub to: f64 }

pub struct Scenario {
    pub scene: Vec<SceneBody>,
    /// Per client: pawn waypoints (true ms, position).
    pub paths: Vec<Vec<(f64, V3)>>,
    /// (true ms, client index, action)
    pub acts: Vec<(f64, usize, Act)>,
    /// Clients present from the start.
    pub start_clients: usize,
    pub end_ms: f64,
    pub phases: Vec<Phase>,
    /// (true ms, label): pairwise "same world" checks of every body at rest.
    pub checks: Vec<(f64, &'static str)>,
}

fn lerp3(a: V3, b: V3, k: f64) -> V3 { [a[0] + (b[0] - a[0]) * k, a[1] + (b[1] - a[1]) * k, a[2] + (b[2] - a[2]) * k] }

fn path_at(path: &[(f64, V3)], t: f64) -> V3 {
    if path.is_empty() { return [0.0, 0.0, 90.0]; }
    if t <= path[0].0 { return path[0].1; }
    for w in path.windows(2) {
        if t <= w[1].0 {
            let k = (t - w[0].0) / (w[1].0 - w[0].0).max(1e-9);
            return lerp3(w[0].1, w[1].1, k);
        }
    }
    path[path.len() - 1].1
}

/// The mixed scenario: push, chain push, carry + throw, kick, contested pickup, a hand-off
/// while moving, a late joiner, a round reset.
pub fn mixed() -> Scenario {
    let s = 1000.0;
    let prop = |name: &str, x: f64, y: f64, r: f64, h: f64, m: f64| SceneBody {
        name: name.into(), weapon: None, pos: [x, y, h], yaw: (x * 0.37 + y * 0.11) % 360.0, r, h, mass: m };
    let weapon = |name: &str, cls: &str, x: f64, y: f64| SceneBody {
        name: name.into(), weapon: Some(cls.into()), pos: [x, y, 3.0], yaw: (x + y) % 360.0, r: 10.0, h: 3.0, mass: 2.0 };
    let mut scene = vec![
        prop("SM_Crate_A", 300.0, 0.0, 30.0, 30.0, 20.0),
        prop("SM_Crate_B", 365.0, 10.0, 30.0, 30.0, 20.0),
        prop("SM_Bucket", -200.0, 200.0, 15.0, 15.0, 3.0),
        prop("SM_Bench", 0.0, -300.0, 40.0, 25.0, 30.0),
        weapon("ModularWeaponBP_LongSword_T3_C_7", "ModularWeaponBP_LongSword_T3_C", 100.0, 300.0),
        weapon("ModularWeaponBP_Axe_T2_C_3", "ModularWeaponBP_Axe_T2_C", -150.0, -150.0),
    ];
    for k in 0..6 {
        scene.push(prop(&format!("SM_Barrel_{k}"), 1800.0, -500.0 + 200.0 * k as f64, 25.0, 40.0, 15.0));
    }
    let z = 90.0;
    // A: pushes the crates, kicks the bucket, goes for the axe, pushes the bench.
    let a = vec![
        (0.0, [-400.0, 0.0, z]), (14.0 * s, [-400.0, 0.0, z]), (17.0 * s, [420.0, 0.0, z]),
        (18.0 * s, [420.0, 0.0, z]), (20.0 * s, [-450.0, 200.0, z]), (27.0 * s, [-450.0, 200.0, z]),
        (28.0 * s, [-80.0, 200.0, z]), (29.0 * s, [-80.0, 100.0, z]), (31.0 * s, [-160.0, -95.0, z]),
        (36.0 * s, [-160.0, -95.0, z]), (37.0 * s, [-150.0, -300.0, z]), (38.0 * s, [-100.0, -300.0, z]),
        (40.0 * s, [60.0, -300.0, z]), (41.0 * s, [-200.0, -500.0, z]),
    ];
    // B: carries the sword and throws it, contests the axe, pushes the bench back.
    let b = vec![
        (0.0, [400.0, -400.0, z]), (19.0 * s, [400.0, -400.0, z]), (21.5 * s, [110.0, 280.0, z]),
        (22.5 * s, [110.0, 280.0, z]), (24.5 * s, [0.0, 320.0, z]), (26.0 * s, [100.0, -100.0, z]),
        (31.0 * s, [-140.0, -205.0, z]), (36.0 * s, [-140.0, -205.0, z]), (38.5 * s, [250.0, -300.0, z]),
        (39.6 * s, [250.0, -300.0, z]), (41.0 * s, [-60.0, -300.0, z]), (42.0 * s, [300.0, -600.0, z]),
    ];
    // C: joins late, stands aside.
    let c = vec![(0.0, [-600.0, -600.0, z])];
    let acts = vec![
        (22.5 * s, 1, Act::Hold(4)),
        (25.2 * s, 1, Act::Throw([-650.0, 120.0, 380.0])),
        (33.0 * s, 0, Act::Hold(5)),
        (33.04 * s, 1, Act::Hold(5)),
        (35.0 * s, 0, Act::Throw([0.0, -400.0, 300.0])),
        (35.0 * s, 1, Act::Throw([0.0, -400.0, 300.0])),
        (44.0 * s, 2, Act::Join),
        (52.0 * s, 1, Act::Poke(450.0, 3)),
        (62.0 * s, 0, Act::Reset),
        (62.8 * s, 0, Act::Reload),
    ];
    let phases = vec![
        Phase { name: "push", from: 14.0 * s, to: 20.0 * s },
        Phase { name: "throw", from: 22.5 * s, to: 27.0 * s },
        Phase { name: "kick", from: 27.0 * s, to: 31.0 * s },
        Phase { name: "contest", from: 33.0 * s, to: 38.0 * s },
        Phase { name: "handoff", from: 38.0 * s, to: 44.0 * s },
        Phase { name: "poke", from: 52.0 * s, to: 58.0 * s },
        // every phase up to the late join (the same content in a before / after comparison: the
        // poke needs HSMPWorld's autotest hook)
        Phase { name: "all", from: 12.0 * s, to: 50.0 * s },
    ];
    let checks = vec![(43.5 * s, "settled"), (61.5 * s, "late join"), (78.0 * s, "after reset")];
    Scenario { scene, paths: vec![a, b, c], acts, start_clients: 2, end_ms: 78.5 * s, phases, checks }
}

#[derive(PartialEq, Eq, PartialOrd, Ord)]
struct Ev { t_us: std::cmp::Reverse<u64>, seq: std::cmp::Reverse<u64>, to: usize, up: bool, idx: usize }

/// What one run measured (raw samples; `report` aggregates).
#[derive(Default, Debug)]
pub struct Metrics {
    /// phase -> moving pairwise (time-aligned) position error cm, angle deg.
    pub moving_d: BTreeMap<&'static str, Vec<f64>>,
    pub moving_a: BTreeMap<&'static str, Vec<f64>>,
    /// phase -> follower path error (the closest owner pose within the last 700 ms), cm.
    pub path_d: BTreeMap<&'static str, Vec<f64>>,
    /// phase -> pairwise error while every copy rests, cm / deg.
    pub rest_d: BTreeMap<&'static str, Vec<f64>>,
    pub rest_a: BTreeMap<&'static str, Vec<f64>>,
    /// phase -> snaps (follower jumps not explained by the owner's motion).
    pub snaps: BTreeMap<&'static str, u32>,
    /// check label -> (max cm, max deg, bodies compared, bodies off by > 2 cm / 2 deg)
    pub checks: Vec<(&'static str, f64, f64, usize, usize)>,
    /// Lease owner changes between two peers / changes back within 1 s.
    pub flips: u32,
    pub fights: u32,
    /// ms during which two pawns held the same body.
    pub conflict_ms: f64,
    /// Held items on observers: distance to the holder's stand-in hand, cm.
    pub hand_d: Vec<f64>,
    /// World bytes per client and second, down / up.
    pub down_bps: f64,
    pub up_bps: f64,
    pub violations: Vec<String>,
    pub events: Vec<(usize, f64, String, String)>,
    pub logs: Vec<Vec<String>>,
}

pub struct Sim {
    pub prof: Profile,
    pub sc: Scenario,
    pub clients: Vec<Option<Client>>,
    up: Vec<Link>,
    down: Vec<Link>,
    world: World,
    level: u32,
    base: Instant,
    q: BinaryHeap<Ev>,
    payloads: Vec<Vec<u8>>,
    qseq: u64,
    next_frame: Vec<f64>,
    next_side: Vec<f64>,
    rng: Rng,
    /// Per client: true pawn path history (for the stand-ins).
    pub m: Metrics,
    keep_logs: bool,
    /// nid -> idx per client (refreshed every 100 ms).
    map: Vec<HashMap<u32, (usize, u32, bool)>>,
    prev: Vec<HashMap<u32, (V3, Q)>>,
    owner_hist: HashMap<(usize, u32), Vec<(f64, V3, Q)>>,
    /// id -> (owner now, when it changed, last non-zero owner, the one before it, when that flip was)
    lease: HashMap<u32, (u32, f64, u32, u32, f64)>,
    held_since: HashMap<u32, f64>,
    prev_steps: HashMap<(usize, u32), f64>,
    hold_start: HashMap<u32, f64>,
    ready_at: Vec<Option<f64>>,
}

fn fnv1a(s: &str) -> u32 {
    let mut h: u32 = 2166136261;
    for b in s.bytes() { h ^= b as u32; h = h.wrapping_mul(16777619); }
    h
}

impl Sim {
    pub fn new(prof: Profile, sc: Scenario, seed: u64, keep_logs: bool) -> Sim {
        let mut rng = Rng::new(seed);
        let n = sc.paths.len();
        let up = (0..n).map(|_| Link::new(prof, &mut rng)).collect();
        let down = (0..n).map(|_| Link::new(prof, &mut rng)).collect();
        let mut level = fnv1a(LEVEL_WORLD);
        if level == 0 { level = 1; }
        let mut s = Sim {
            prof, sc, clients: (0..n).map(|_| None).collect(), up, down, world: World::new(), level,
            base: Instant::now(), q: BinaryHeap::new(), payloads: Vec::new(), qseq: 0,
            next_frame: vec![0.0; n], next_side: vec![0.0; n], rng, m: Metrics::default(), keep_logs,
            map: vec![HashMap::new(); n], prev: vec![HashMap::new(); n], owner_hist: HashMap::new(),
            lease: HashMap::new(), held_since: HashMap::new(), prev_steps: HashMap::new(), hold_start: HashMap::new(), ready_at: vec![None; n],
        };
        for c in 0..s.sc.start_clients { s.join(c, 0.0); }
        s
    }

    fn join(&mut self, c: usize, t: f64) {
        let off = self.rng.range(1000.0, 90_000.0);
        let cl = Client::new(c as u32 + 1, &mut self.rng, &self.sc.scene, off);
        cl.host.borrow_mut().keep_logs = self.keep_logs;
        {
            let g = cl.lua.globals();
            g.set("SIM_RTT", self.prof.rtt() + 2.0).unwrap();
            g.set("SIM_TRACK_OFF", -off).unwrap();
            let t = cl.lua.create_table().unwrap();
            for p in 1..=self.sc.paths.len() as i64 { t.set(p, self.prof.rtt() + 2.0).unwrap(); }
            g.set("SIM_PEER_RTT", t).unwrap();
        }
        cl.host.borrow_mut().pawn = path_at(&self.sc.paths[c], t);
        self.clients[c] = Some(cl);
        self.next_frame[c] = t + self.rng.range(0.0, FRAME_MS);
        self.next_side[c] = t + self.rng.range(0.0, SIDECAR_MS);
        // the welcome asks for a world sync
        self.clients[c].as_mut().unwrap().rx.want_resync = false;
    }

    fn now(&self, t: f64) -> Instant { self.base + Duration::from_micros((t * 1000.0) as u64) }

    fn push(&mut self, at: f64, to: usize, up: bool, bytes: Vec<u8>) {
        self.payloads.push(bytes);
        self.qseq += 1;
        self.q.push(Ev { t_us: std::cmp::Reverse((at * 1000.0) as u64), seq: std::cmp::Reverse(self.qseq), to, up,
                         idx: self.payloads.len() - 1 });
    }

    fn reliable_kind(kind: u16) -> bool {
        !matches!(hsmp_ipc::schema::record_info(kind).map(|r| r.chan), Some(hsmp_ipc::schema::Chan::Latest(_)))
    }

    fn send_up(&mut self, t: f64, c: usize, msg: Vec<u8>) {
        let kind = hsmp_ipc::wire::kind_of(&msg);
        if Self::reliable_kind(kind) {
            let at = self.up[c].send_reliable(t, msg.len());
            self.push(at, c, true, msg);
        } else {
            for at in self.up[c].send(t, msg.len()) { self.push(at, c, true, msg.clone()); }
        }
    }

    fn send_out(&mut self, t: f64, out: Vec<world::Out>) {
        for (pid, msg) in out {
            let c = pid as usize - 1;
            if c >= self.clients.len() || self.clients[c].is_none() { continue; }
            let unreliable = matches!(msg, Msg::States { .. });
            let parts = msg.encode();
            if unreliable {
                // one datagram per packet (a sender's chunks share it)
                let n: usize = parts.iter().map(|p| p.len()).sum();
                let arr = self.down[c].send(t, n);
                for at in arr { for p in &parts { self.push(at, c, false, p.clone()); } }
            } else {
                for p in parts {
                    let at = self.down[c].send_reliable(t, p.len());
                    self.push(at, c, false, p);
                }
            }
        }
    }

    fn pawn_true(&self, c: usize, t: f64) -> V3 { path_at(&self.sc.paths[c], t) }

    fn present(&self) -> Vec<usize> { (0..self.clients.len()).filter(|c| self.clients[*c].is_some()).collect() }

    /// A record from client c at the server (world_glue.rs handle_record).
    fn server_rx(&mut self, t: f64, c: usize, msg: &[u8]) {
        let now = self.now(t);
        let peer = c as u32 + 1;
        let (h, payload) = hsmp_ipc::wire::split(msg).expect("framed");
        let pos = { let p = self.pawn_true(c, t - self.prof.delay); [p[0] as f32, p[1] as f32, p[2] as f32] };
        let w = &mut self.world;
        let out: Vec<world::Out> = match h.kind {
            rec::K_WORLD_STATE => {
                let v = view::<rec::WorldStateHead>(payload).expect("state");
                let hd = v.head();
                if hd.epoch != w.epoch {
                    w.stale_hint(peer, hd.level, now).into_iter().collect()
                } else {
                    let (_, changed) = w.state(peer, hd.level, hd.epoch, hd.seq, hd.ts, &v.rows, Some(pos), now);
                    if changed.is_empty() { Vec::new() } else { w.owners_to_level(hd.level, &changed) }
                }
            }
            rec::K_WORLD_CLAIM => {
                let v = view::<rec::WorldClaim>(payload).expect("claim");
                let cl = v.head();
                let rest = cl.has_rest.get().then_some(cl.rest);
                match w.claim(peer, cl.level, cl.epoch, cl.id, cl.mode, rest, now) {
                    ClaimResult::Done { rec, changed } => {
                        if changed { w.owners_to_level(cl.level, &[rec]) } else {
                            let e = w.epoch;
                            let mlen = w.levels.get(&cl.level).map_or(0, |l| l.manifest.len() as u32);
                            vec![(peer, Msg::Owners { level: cl.level, epoch: e, sync: false, manifest_len: mlen, owners: vec![rec] })]
                        }
                    }
                    ClaimResult::Stale => w.stale_hint(peer, cl.level, now).into_iter().collect(),
                    _ => Vec::new(),
                }
            }
            rec::K_WORLD_SYNC => {
                let v = view::<rec::WorldSync>(payload).expect("sync");
                w.sync(peer, v.head.level, now)
            }
            rec::K_WORLD_MANIFEST => {
                let v = view::<rec::ManifestHead>(payload).expect("manifest");
                let e: Vec<WorldEntry> = v.rows.iter().map(WorldEntry::from_static).collect();
                w.manifest(peer, v.head.level, v.head.epoch, v.head.req, e, now)
            }
            rec::K_WORLD_DYN => {
                let v = view::<rec::DynHead>(payload).expect("dyn");
                let e: Vec<WorldEntry> = v.rows.iter().map(WorldEntry::from_dyn).collect();
                w.manifest(peer, v.head.level, v.head.epoch, v.head.req, e, now)
            }
            rec::K_WORLD_HASH => {
                let v = view::<rec::HashHead>(payload).expect("hash");
                let hd = v.head();
                w.hash_report(peer, hd.level, hd.epoch, hd.seq, hd.hash, &v.rows, now)
            }
            k => panic!("server got kind {k:#x}"),
        };
        self.send_out(t, out);
    }

    fn server_tick(&mut self, t: f64) {
        let now = self.now(t);
        let present: HashSet<u32> = self.present().iter().map(|c| *c as u32 + 1).collect();
        let positions: HashMap<u32, [f32; 3]> = self.present().iter().map(|c| {
            let p = self.pawn_true(*c, t - self.prof.delay);
            (*c as u32 + 1, [p[0] as f32, p[1] as f32, p[2] as f32])
        }).collect();
        let out = self.world.tick(now, &present, &positions);
        self.send_out(t, out);
        // lease history (flips / fights)
        if let Some(lv) = self.world.levels.get(&self.level) {
            let ids: Vec<u32> = lv.manifest.iter().map(|e| e.id).collect();
            for id in ids {
                let owner = lv.leases.get(&id).map_or(0, |l| l.owner);
                let e = self.lease.entry(id).or_insert((0, t, 0, 0, -1e9));
                if owner != e.0 {
                    if owner != 0 {
                        if e.2 != 0 && e.2 != owner {
                            self.m.flips += 1;
                            // a fight: back to the owner before within 1 s
                            if owner == e.3 && t - e.4 < 1000.0 {
                                self.m.fights += 1;
                                if std::env::var("SIMSNAP").is_ok() { eprintln!("FIGHT t={t:.0} id={id} {} -> {owner} after {:.0} ms", e.2, t - e.4); }
                            }
                            e.3 = e.2;
                            e.4 = t;
                        }
                        e.2 = owner;
                    }
                    e.0 = owner;
                    e.1 = t;
                }
            }
        }
    }

    fn act(&mut self, t: f64, c: usize, a: Act) {
        match a {
            Act::Join => self.join(c, t),
            Act::Reset => {
                let out = self.world.bump_epoch();
                self.send_out(t, out);
                self.lease.clear();
            }
            Act::Reload => {
                let scene = self.sc.scene.clone();
                for cl in self.clients.iter_mut().flatten() { cl.reload(&scene); }
            }
            Act::Poke(speed, n) => {
                if let Some(cl) = self.clients[c].as_ref() {
                    // (an older main.lua has no poke hook: nothing happens)
                    if let Ok(f) = cl.t.get::<mlua::Function>("poke") {
                        let _: i64 = f.call((speed, n, t)).unwrap();
                    }
                }
            }
            Act::Hold(idx) => {
                if let Some(cl) = self.clients[c].as_mut() {
                    let _: bool = cl.call("hold", (idx as i64, "R"));
                }
            }
            Act::Throw(v) => {
                if let Some(cl) = self.clients[c].as_mut() {
                    let idx: Option<i64> = cl.call("release", "R");
                    if let Some(i) = idx {
                        let mut p = cl.phys.borrow_mut();
                        let b = &mut p.bodies[i as usize];
                        b.attached = false;
                        b.sim = true;
                        b.vel = v;
                        b.w = [3.0, 1.5, 6.0];
                        b.wake();
                    }
                }
            }
        }
    }

    fn hand(&self, pawn: V3, t: f64) -> (V3, Q) {
        let s = (t / 1000.0 * 2.0 * std::f64::consts::PI * 1.2).sin();
        let pos = phys::add(pawn, [HAND_OFF[0] + 25.0 * s, HAND_OFF[1], HAND_OFF[2] + 30.0 * s]);
        (pos, phys::rot_to_quat(40.0 * s, 30.0 * s, 0.0))
    }

    /// How old a stand-in's pose is on screen: both links, the pose IPC, and poseplay's buffer
    /// (one 16 ms interval + p90 lateness + 6) measured from the fastest path. With two links
    /// of uniform +-J jitter that is 2 delay + 22 + 1.1 J (+ 5 ms IPC).
    fn pose_delay(&self) -> f64 { 2.0 * self.prof.delay + 27.0 + 1.1 * self.prof.jitter }

    pub fn run(mut self) -> Metrics {
        let mut t = 0.0;
        let mut next_server = 0.0;
        let mut next_sample = 0.0;
        let mut acts = self.sc.acts.clone();
        acts.sort_by(|a, b| a.0.total_cmp(&b.0));
        let mut ai = 0;
        let mut ci = 0;
        let checks = self.sc.checks.clone();
        while t <= self.sc.end_ms {
            while ai < acts.len() && acts[ai].0 <= t {
                let (_, c, a) = acts[ai];
                self.act(t, c, a);
                ai += 1;
            }
            // deliveries
            while let Some(top) = self.q.peek() {
                if top.t_us.0 as f64 > t * 1000.0 { break; }
                let ev = self.q.pop().unwrap();
                let bytes = std::mem::take(&mut self.payloads[ev.idx]);
                if ev.up {
                    self.server_rx(t, ev.to, &bytes);
                } else if let Some(cl) = self.clients[ev.to].as_mut() {
                    cl.deliver(t, &bytes);
                }
            }
            if t >= next_server { next_server += SERVER_MS; self.server_tick(t); }
            for c in self.present() {
                if t >= self.next_side[c] {
                    self.next_side[c] += SIDECAR_MS;
                    let up = self.clients[c].as_mut().unwrap().sidecar(t);
                    for (_, m) in up { self.send_up(t, c, m); }
                }
                if t >= self.next_frame[c] {
                    let dt = FRAME_MS + self.rng.range(-2.0, 2.0);
                    self.next_frame[c] = t + dt;
                    self.frame(c, t, dt);
                    let g = self.clients[c].as_mut().unwrap().take_g2s();
                    for m in g { self.send_up(t, c, m); }
                }
            }
            if t >= next_sample {
                next_sample += SAMPLE_MS;
                self.sample(t);
            }
            while ci < checks.len() && checks[ci].0 <= t {
                self.check(checks[ci].1);
                ci += 1;
            }
            t += 1.0;
        }
        let secs = self.sc.end_ms / 1000.0;
        let nc = self.clients.len() as f64;
        self.m.down_bps = self.down.iter().map(|l| l.bytes as f64).sum::<f64>() / nc / secs;
        self.m.up_bps = self.up.iter().map(|l| l.bytes as f64).sum::<f64>() / nc / secs;
        for (i, cl) in self.clients.iter().enumerate() {
            if let Some(cl) = cl {
                let h = cl.host.borrow();
                for v in &h.violations { self.m.violations.push(format!("client {}: {v}", i + 1)); }
                for (et, n, f) in &h.events { self.m.events.push((i, *et, n.clone(), f.clone())); }
                self.m.logs.push(h.logs.clone());
            }
        }
        self.m
    }

    fn frame(&mut self, c: usize, t: f64, dt: f64) {
        let pawn = self.pawn_true(c, t);
        let pawn_prev = self.pawn_true(c, t - dt);
        let pd = self.pose_delay();
        let mut pushers = vec![Pusher { pos: pawn, vel: phys::mul(phys::sub(pawn, pawn_prev), 1000.0 / dt), r: 35.0 }];
        let mut puppets = HashMap::new();
        let peers: Vec<usize> = self.present().into_iter().filter(|p| *p != c).collect();
        for &p in &peers {
            let pp = self.pawn_true(p, t - pd);
            let pv = phys::mul(phys::sub(pp, self.pawn_true(p, t - pd - dt)), 1000.0 / dt);
            pushers.push(Pusher { pos: pp, vel: pv, r: 35.0 });
            puppets.insert(p as u32 + 1, pp);
        }
        let hand = self.hand(pawn, t);
        let cl = self.clients[c].as_mut().unwrap();
        {
            let mut h = cl.host.borrow_mut();
            h.pawn = pawn;
            h.puppets = puppets;
        }
        let ids: Vec<i64> = peers.iter().map(|p| *p as i64 + 1).collect();
        let _: () = cl.call("set_puppets", ids);
        cl.frame(t, dt / 1000.0, &pushers, Some(hand));
    }

    fn phases_at(&self, t: f64) -> Vec<&'static str> {
        self.sc.phases.iter().filter(|p| t >= p.from && t < p.to).map(|p| p.name).collect()
    }

    fn sample(&mut self, t: f64) {
        let present = self.present();
        // refresh the nid -> body maps
        if (t as u64) % 100 < SAMPLE_MS as u64 {
            for &c in &present {
                let cl = self.clients[c].as_ref().unwrap();
                let rows: Vec<Vec<i64>> = cl.call("objs", ());
                self.map[c] = rows.into_iter().map(|r| (r[0] as u32, (r[1] as usize, r[2] as u32, r[3] != 0))).collect();
                let ready: bool = cl.call("ready", ());
                if ready && self.ready_at[c].is_none() { self.ready_at[c] = Some(t); }
                if !ready { self.ready_at[c] = None; }
            }
        }
        let phases = self.phases_at(t);
        // poses per client
        // a simulating body's own speed on that screen (its physics, not a teleport)
        let mut speed: Vec<HashMap<u32, f64>> = vec![HashMap::new(); self.clients.len()];
        let mut poses: Vec<HashMap<u32, (V3, Q)>> = vec![HashMap::new(); self.clients.len()];
        for &c in &present {
            let cl = self.clients[c].as_ref().unwrap();
            let p = cl.phys.borrow();
            for (nid, (idx, _, _)) in &self.map[c] {
                if let Some(b) = p.bodies.get(*idx) {
                    if b.alive { poses[c].insert(*nid, (b.pos, b.q)); speed[c].insert(*nid, if b.sim && !b.attached { phys::len(b.vel) } else { 0.0 }); }
                }
            }
        }
        // the true holder of each body (a pawn's own hand) and the conflicts
        let mut holders: HashMap<u32, Vec<usize>> = HashMap::new();
        for &c in &present {
            let cl = self.clients[c].as_ref().unwrap();
            if let Some(i) = cl.held_idx("R") {
                if let Some((nid, _)) = self.map[c].iter().find(|(_, v)| v.0 == i) { holders.entry(*nid).or_default().push(c); }
            }
        }
        self.hold_start.retain(|k, _| holders.contains_key(k));
        for k in holders.keys() { self.hold_start.entry(*k).or_insert(t); }
        for (nid, hs) in &holders {
            if hs.len() > 1 {
                let since = *self.held_since.entry(*nid).or_insert(t);
                if t - since > 300.0 { self.m.conflict_ms += SAMPLE_MS; }
            } else {
                self.held_since.remove(nid);
            }
        }
        let nids: Vec<u32> = {
            let mut s: HashSet<u32> = HashSet::new();
            for m in &self.map { s.extend(m.keys()); }
            let mut v: Vec<u32> = s.into_iter().collect();
            v.sort();
            v
        };
        let owners: HashMap<u32, u32> = self.world.levels.get(&self.level)
            .map(|lv| lv.leases.iter().map(|(k, l)| (*k, l.owner)).collect()).unwrap_or_default();
        let pd = self.pose_delay();
        for nid in nids {
            let have: Vec<usize> = present.iter().copied().filter(|c| poses[*c].contains_key(&nid)).collect();
            if have.len() < 2 { continue; }
            let mut moving = false;
            let mut step = HashMap::new();
            for &c in &have {
                let (p, q) = poses[c][&nid];
                if let Some((pp, pq)) = self.prev[c].get(&nid) {
                    let d = phys::dist(p, *pp);
                    let a = phys::qangle(q, *pq);
                    step.insert(c, (d, a));
                    if d > 0.4 || a > 0.5 { moving = true; }
                }
            }
            let owner = owners.get(&nid).copied().unwrap_or(0);
            let oc = if owner != 0 { Some(owner as usize - 1) } else { None };
            for &c in &have {
                let h = self.owner_hist.entry((c, nid)).or_default();
                h.push((t, poses[c][&nid].0, poses[c][&nid].1));
                h.retain(|(ht, _, _)| t - ht < 1500.0);
            }
            if let Ok(tr) = std::env::var("SIMTRACE") { if moving && phases.contains(&tr.as_str()) && have.len() >= 2 { let o0 = oc.unwrap_or(9); eprintln!("{t:.0} nid={nid} own={} {}", o0, have.iter().map(|c| format!("c{c}=({:.0},{:.0},{:.0})", poses[*c][&nid].0[0], poses[*c][&nid].0[1], poses[*c][&nid].0[2])).collect::<Vec<_>>().join(" ")); if std::env::var("SIMFW").is_ok() { for c in &have { let s: String = self.clients[*c].as_ref().unwrap().call("fw", nid as i64); eprintln!("    c{c} {s}"); } } } }
            // the "held" / "free" (pushed, thrown, knocked) split of the moving samples
            let mut phases = phases.clone();
            if moving && phases.contains(&"all") && oc.is_some() { phases.push(if holders.contains_key(&nid) { "held" } else { "free" }); }
            // pairwise divergence
            for i in 0..have.len() {
                for j in (i + 1)..have.len() {
                    let (a, b) = (have[i], have[j]);
                    let d = phys::dist(poses[a][&nid].0, poses[b][&nid].0);
                    let an = phys::qangle(poses[a][&nid].1, poses[b][&nid].1);
                    for ph in &phases {
                        if moving {
                            self.m.moving_d.entry(ph).or_default().push(d);
                            self.m.moving_a.entry(ph).or_default().push(an);
                        } else {
                            self.m.rest_d.entry(ph).or_default().push(d);
                            self.m.rest_a.entry(ph).or_default().push(an);
                        }
                    }
                }
            }
            // follower path error and snaps
            if let Some(o) = oc {
                let hist = self.owner_hist.get(&(o, nid)).cloned().unwrap_or_default();
                let o_step = step.get(&o).copied().unwrap_or((0.0, 0.0));
                for &c in &have {
                    if c == o { continue; }
                    if moving && !hist.is_empty() {
                        let p = poses[c][&nid].0;
                        let e = hist.iter().filter(|(ht, _, _)| t - ht <= 700.0).map(|(_, hp, _)| phys::dist(p, *hp)).fold(f64::MAX, f64::min);
                        if e < f64::MAX { if e > 20.0 && std::env::var("SIMPATH").is_ok() { eprintln!("PATH t={t:.0} nid={nid} c={c} e={e:.0} p={p:?} hist_n={} last={:?}", hist.len(), hist.last()); } for ph in &phases { self.m.path_d.entry(ph).or_default().push(e); } }
                    }
                    if !self.counts(c, t) { continue; }
                    if let Some((d, a)) = step.get(&c) {
                        // the largest owner step over the last 700 ms bounds a smooth follower's step
                        let win = hist.windows(2).filter(|w| t - w[1].0 <= 700.0).map(|w| phys::dist(w[0].1, w[1].1)).fold(o_step.0, f64::max);
                        if (*d - win > 15.0 && *d > 2.0 * win) || *a > 25.0 + 2.0 * self.ang_win(o, nid, t) {
                            if std::env::var("SIMSNAP").is_ok() { eprintln!("SNAP owned t={t:.0} nid={nid} c={c} own={o} step={d:.1}cm/{a:.1}deg win={win:.1}"); } for ph in &phases { *self.m.snaps.entry(ph).or_default() += 1; }
                        }
                    }
                }
                // a held item on observers: distance to the holder's stand-in hand
                if holders.get(&nid).map_or(false, |h| h.len() == 1 && h[0] == o) && t - self.hold_start.get(&nid).copied().unwrap_or(t) > 400.0 {
                    let hand = self.hand(self.pawn_true(o, t - pd), t - pd).0;
                    for &c in &have {
                        if c != o { let d = phys::dist(poses[c][&nid].0, hand); if d > 100.0 && std::env::var("SIMDBG").is_ok() { eprintln!("t={t:.0} nid={nid} obs={c} own={o} d={d:.0} obs_pos={:?} hand={hand:?} own_pos={:?}", poses[c][&nid].0, poses[o][&nid].0); } self.m.hand_d.push(d); }
                    }
                }
            } else {
                // free: a copy that jumps while nobody owns it was teleported (snap-back / pin)
                for &c in &have {
                    let native = holders.get(&nid).map_or(false, |h| h.contains(&c)) || self.hold_start.contains_key(&nid);
                    // on a screen where that player's own pawn touches the body, a jolt is its own physics
                    let touching = phys::dist(self.pawn_true(c, t), poses[c][&nid].0) < 150.0;
                    let native = native || touching;
                    let phys_step = speed[c].get(&nid).copied().unwrap_or(0.0) * SAMPLE_MS / 1000.0 * 1.5 + 1.0;
                    if native || !self.counts(c, t) { continue; }
                    if let Some((d, _)) = step.get(&c) {
                        let prev_step = self.prev_step(c, nid);
                        if *d > 6.0 && *d > 3.0 * prev_step + 4.0 && *d > phys_step {
                            if std::env::var("SIMSNAP").is_ok() { eprintln!("SNAP free t={t:.0} nid={nid} c={c} step={d:.1}cm prev={prev_step:.1}"); } for ph in &phases { *self.m.snaps.entry(ph).or_default() += 1; }
                        }
                    }
                }
            }
            for &c in &have {
                let ps = step.get(&c).map_or(0.0, |s| s.0);
                self.prev_steps.insert((c, nid), ps);
            }
        }
        for &c in &present { self.prev[c] = poses[c].clone(); }
    }

    /// Snaps count once a client has been ready for 1 s (its initial forcing is not a snap).
    fn counts(&self, c: usize, t: f64) -> bool { self.ready_at[c].map_or(false, |r| t - r > 1000.0) }

    /// The owner's largest orientation change between two samples over the last 700 ms, deg.
    fn ang_win(&self, o: usize, nid: u32, t: f64) -> f64 {
        self.owner_hist.get(&(o, nid)).map_or(0.0, |h| h.windows(2).filter(|w| t - w[1].0 <= 700.0)
            .map(|w| phys::qangle(w[0].2, w[1].2)).fold(0.0, f64::max))
    }

    fn prev_step(&self, c: usize, nid: u32) -> f64 { self.prev_steps.get(&(c, nid)).copied().unwrap_or(0.0) }

    /// Every body on every pair of clients: how far apart (a "same world" check).
    fn check(&mut self, label: &'static str) {
        let present = self.present();
        let mut maxd: f64 = 0.0;
        let mut maxa: f64 = 0.0;
        let mut n = 0;
        let mut off = 0;
        let mut poses: Vec<HashMap<u32, (V3, Q)>> = vec![HashMap::new(); self.clients.len()];
        for &c in &present {
            let cl = self.clients[c].as_ref().unwrap();
            let rows: Vec<Vec<i64>> = cl.call("objs", ());
            let p = cl.phys.borrow();
            for r in rows {
                if let Some(b) = p.bodies.get(r[1] as usize) { if b.alive { poses[c].insert(r[0] as u32, (b.pos, b.q)); } }
            }
        }
        let mut all: Vec<u32> = poses.iter().flat_map(|m| m.keys().copied()).collect();
        all.sort();
        all.dedup();
        for nid in all {
            let have: Vec<usize> = present.iter().copied().filter(|c| poses[*c].contains_key(&nid)).collect();
            if have.len() < 2 { continue; }
            n += 1;
            let mut worst: (f64, f64) = (0.0, 0.0);
            for i in 0..have.len() {
                for j in (i + 1)..have.len() {
                    let d = phys::dist(poses[have[i]][&nid].0, poses[have[j]][&nid].0);
                    let a = phys::qangle(poses[have[i]][&nid].1, poses[have[j]][&nid].1);
                    worst = (worst.0.max(d), worst.1.max(a));
                }
            }
            maxd = maxd.max(worst.0);
            maxa = maxa.max(worst.1);
            if worst.0 > 2.0 || worst.1 > 2.0 { off += 1; }
        }
        self.m.checks.push((label, maxd, maxa, n, off));
    }
}
