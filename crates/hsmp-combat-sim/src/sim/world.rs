//! The run loop (1 ms steps of TRUE time; the server clock is true time).
//!
//! Client c (one game + sidecar), per 60 Hz frame:
//!   * samples its own body → pose stream (IPC 1–6 ms, then its uplink;
//!     unreliable, so loss leaves holes and jitter reorders);
//!   * shows every peer p through a poseplay-style jitter buffer:
//!     offset = min(rx − ts) over 2 s, delay = interval + p90 lateness + 6
//!     (≥ 16, grows at once, shrinks 1 ms per 50 ms), playback label
//!     pt = clock − offset − delay; the stand-in shows LAST frame's label
//!     (servo lag) with 2.5 uu servo noise, extrapolated ≤ 100 ms / 30 uu
//!     past the newest sample, then held;
//!   * detects contacts of its own (true) blade, swept over the frame, and
//!     its hands with each displayed peer's TRUE collision bodies → one
//!     `Get Damage` event per body touched (ats = its clock, vts = vats =
//!     the label shown), and blade-on-blade contacts → clash report
//!     (≤ 1 per 100 ms per peer, CLASH_GAP_MS) + a 200 ms deflection (the
//!     blade is stopped on that screen: no body contact through a parry);
//! and per 33 ms Lua tick turns the events into claims (legacy: one claim
//! per event; dedupe: one per contact episode + ≤ 1 continuation per
//! 150 ms), which the sidecar picks up (≤ 5 ms), sends reliably and resends
//! every 120 ms with age_ms until the final ack (give up after 2 s).
//!
//! Server: records every stream into the real `lagcomp::Store`, relays poses
//! (unreliable) to the other clients, runs `combat::Engine` on claims and the
//! glue (`server/combat_glue.rs`): forward S2CDamage + early "confirm" ack,
//! final ack after the owner's ack, held hits flushed and clashes judged on a
//! 60 Hz tick, RTT samples from the connection layer every 500 ms.

use super::body::*;
use super::core::Core;
use super::game;
use super::net::*;
use super::script::{self, Kind, Mix, Plan};
use super::*;
use crate::combat::{self, Ctx, Verdict};
use crate::lagcomp;
use crate::proto::{DamageEvent, PeerId};
use crate::validate::damage;
use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};
use std::sync::atomic::{AtomicU32, Ordering};

#[path = "health.rs"]
pub mod health;

pub const FRAME_MS: f64 = 1000.0 / 60.0;
pub const LUA_TICK_MS: f64 = 1000.0 / 30.0;
pub const SIDECAR_TICK_MS: f64 = 5.0;
pub const RESEND_EVERY_MS: f64 = 120.0;
pub const HIT_GIVE_UP_MS: f64 = 2000.0;
pub const CLASH_GAP_MS: f64 = 100.0;
pub const SERVER_TICK_MS: f64 = 1000.0 / 60.0;
/// HSMPCombat dedupe policy (mirrors mod/ue4ss_mods/HSMPCombat/Scripts/main.lua).
pub const EPISODE_GAP_MS: f64 = 120.0;
pub const CONT_MS: f64 = 150.0;
/// Servo tracking noise of a stand-in bone (uu, 1σ) — POSE v2 replay p95 ≈ 1–4 uu.
pub const SERVO_NOISE: f32 = 2.5;
/// Stand-in relative-speed noise per uu of servo noise (uu/s, 1σ per axis).
/// Calibrated on recorded live swings (54 contacts above 300 uu/s): the
/// stand-in over-read the real relative speed by >1.3× once (2 %) and under-read
/// it 41 times; 100 (σ 250) over-read most contacts and made the server rescale
/// real blows ×1/3..×3.
pub const SERVO_VEL_K: f32 = 24.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Cheat {
    /// Claims a hit on the victim pose from 150–500 ms before what it displayed.
    Backtrack,
    /// Claims contact on the displayed body when its blade missed by 30–120 uu.
    ReachFake,
    /// Streams a blade 60 uu longer than its weapon and claims what that reaches.
    ReachBlade,
    /// Streams its hand + blade 45 uu further out (stretched arm).
    ReachArm,
    /// Honest contacts, Health loss claimed at 95.
    DamageInflate,
    /// As a victim: clash reports every frame-ish naming the attacker's shown ts.
    ParrySpam,
    /// Honest contact re-stamped at ats ± Δ (and age_ms forged to match).
    TsForge,
    /// Clock (and game) runs 1.3× fast.
    Speedhack,
    /// As a victim: its sidecar holds each pose packet a random
    /// 0–250 ms (a lag switch on its own stream), so every receiver buffers
    /// it ~250 ms late. Success = an honest hit on it rejected.
    JitterInflate,
    /// As a victim: when an attacker's displayed blade comes near
    /// its body and no real parry is on its screen, it streams that blade
    /// segment as its own for those frames and reports a clash.
    FakeParry,
    /// As a victim: never applies forwarded hits (still acks
    /// them) and keeps reporting full health. Success = still fighting 1.5 s
    /// after its replays would have killed it (margin: see report).
    GodMode,
    /// As a victim: a lag switch on ALL its outgoing traffic — its
    /// pose stream AND the connection-layer acks are held a random 0–250 ms,
    /// so the server-measured rttvar inflates too and no longer bounds the
    /// stream jitter it believes. Success = an honest hit on it rejected.
    AckHold,
}

pub const ALL_CHEATS: [Cheat; 12] = [Cheat::Backtrack, Cheat::ReachFake, Cheat::ReachBlade, Cheat::ReachArm,
    Cheat::DamageInflate, Cheat::ParrySpam, Cheat::TsForge, Cheat::Speedhack,
    Cheat::JitterInflate, Cheat::FakeParry, Cheat::GodMode, Cheat::AckHold];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Policy {
    /// Pre-change HSMPCombat: one claim per Get Damage call.
    Legacy,
    /// One claim per contact episode (+ continuations).
    Dedupe,
}

#[derive(Clone, Debug)]
pub struct Config {
    pub players: usize,
    pub dur_ms: f64,
    pub profile: Profile,
    pub mix: Mix,
    pub seed: u64,
    /// POSE codec v2: blade (base, tip, tip velocity) + true capsules streamed.
    pub stream_v2: bool,
    pub policy: Policy,
    /// (fighter index, cheat)
    pub cheats: Vec<(usize, Cheat)>,
    pub drift_ppm: f64,
    /// Stream polearm blades as POSE codec v2 does: tip scene ~1 uu from the
    /// grip (the server rebuilds the haft from the hands).
    pub degenerate_polearm: bool,
    /// Stand-in tracking error (uu, 1σ per bone). v2 servo stand-ins ≈ 2.5;
    /// the v1 PhysicsHandle stand-ins missed by 6–78 uu (measured in game).
    /// Env SIM_SERVO_NOISE overrides (report runs).
    pub servo_noise: f32,
    /// End-to-end health replication (sim::world::health): victim replays, the
    /// vitals stream, the real ledger, deaths from vitals / the stall rule.
    pub health_model: bool,
    /// (fighter, from, to ms): its Lua stops (no vitals, no replays, no claims);
    /// its sidecar still acks.
    pub stall: Option<(usize, f64, f64)>,
    /// Fault injection for the checker: the stand-in's echo blow on the
    /// victim's pawn is NOT restored (double application).
    pub echo_leak: bool,
    /// Server tick period (ms): held-hit flush, clash judging, ledger sweep.
    pub server_tick_ms: f64,
    /// Ablation: the server keeps only the newest pose sample per tick
    /// (a tick-snapshot history) instead of every sample on arrival.
    pub record_on_tick: bool,
    /// Ablation: poses are relayed on the next tick, not on arrival.
    pub relay_on_tick: bool,
}

impl Config {
    pub fn new(profile: Profile, players: usize, seed: u64) -> Config {
        Config {
            players, dur_ms: 30_000.0, profile, mix: Mix::standard(), seed, stream_v2: true,
            policy: Policy::Dedupe, cheats: Vec::new(), drift_ppm: 300.0, degenerate_polearm: true,
            servo_noise: std::env::var("SIM_SERVO_NOISE").ok().and_then(|s| s.parse().ok()).unwrap_or(SERVO_NOISE),
            health_model: false, stall: None, echo_leak: false,
            server_tick_ms: SERVER_TICK_MS,
            record_on_tick: false, relay_on_tick: false,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum ClaimKind {
    Honest,
    Cheat(Cheat),
}

#[derive(Clone, Debug)]
pub struct ClaimRec {
    pub attacker: usize,
    pub target: usize,
    pub kind: ClaimKind,
    /// True time of the (earliest) hit event in the claim.
    pub t_hit: f64,
    /// True time of the event whose ats the claim carries (the newest frame).
    pub t_main: f64,
    pub contact: u64,
    /// True age of the victim pose the attacker displayed at the hit (ms).
    pub view_lag: f64,
    pub eng: Option<(usize, Kind)>,
    pub lethal: bool,
    pub claimed_loss: f32,
    /// The loss the game itself did (honest truth; cheats: what an honest
    /// claim of the same contact would have said).
    pub native_loss: f32,
    pub booked_loss: Option<f32>,
    /// Final outcome: (accepted, reason).
    pub outcome: Option<(bool, String)>,
    pub confirm_t: Option<f64>,
    pub applied_t: Option<f64>,
    pub arrived_t: Option<f64>,
    pub forwarded_t: Option<f64>,
    /// Whether the cheat actually needed the lie (geometry / time differs
    /// from the honest truth by more than the tolerances).
    pub effective: bool,
    pub hand: bool,
    pub n_events: usize,
    /// Physical maximum Health loss of the merged contacts.
    pub bound_sum: f32,
    pub lie: f32,
    pub speed: f32,
    pub bone: &'static str,
    /// Server-measured contact speed (debug).
    pub server_speed: Option<f32>,
    /// Server Info::parry_d at first arrival (debug).
    pub parry_d: Option<[f32; 3]>,
    pub s_blade: f32,
    /// Solo Health loss of the main contact, and what the victim's replay of
    /// the server-approved armour-stage inputs did (solo-parity metric).
    pub solo: Option<f32>,
    pub mp: Option<f32>,
    /// (weapon class, victim armour, part rank, Willie height draw, stab)
    pub pcell: Option<(damage::WeaponClass, damage::Armour, u8, f32, bool)>,
    /// Debug: true relative speed, stand-in one, server estimate.
    pub rel_dbg: Option<(f32, f32, Option<f32>, f32)>,
    /// Server peak striking speed at first arrival (Info::peak_speed, or the contact speed).
    pub server_peak: Option<f32>,
    /// Honest blade contacts: (capsule segment, true time of the victim pose shown, contact point).
    pub geo: Option<(usize, f64, V3)>,
}

#[derive(Clone, Copy)]
enum Msg {
    SPose { from: usize, ts: u32, t: f64 },
    SClaim { from: usize, rec: usize },
    SClash { from: usize, other: usize, my_ts: u32, other_ts: u32 },
    SOwnerAck { from: usize, attacker: usize, hit_id: u32 },
    CPose { to: usize, from: usize, ts: u32, t: f64 },
    CDamage { to: usize, attacker: usize, hit_id: u32, rec: usize },
    CAck { to: usize, hit_id: u32, accepted: bool, final_: bool, rec: usize },
    CClash { to: usize, other: usize },
    /// Lua outbox line picked up by the sidecar.
    Pickup { client: usize, rec: usize },
    PickupClash { client: usize, other: usize, my_ts: u32, other_ts: u32 },
    SVitals { from: usize, seq: u32, hp: f32, dead: bool, life: u32 },
    CVitals { to: usize, from: usize, seq: u32, hp: f32, life: u32 },
    SDeath { from: usize, life: u32 },
}

struct PeerView {
    /// (rx local ms, ts) arrivals, 2 s window
    arr: VecDeque<(f64, u32)>,
    /// received sample ts (sorted, deduped), newest last, ≤ 64 kept
    ts: VecDeque<u32>,
    delay: f64,
    last_shrink: f64,
    /// label shown this frame / last frame
    label: Option<f64>,
    prev_label: Option<f64>,
}

impl PeerView {
    fn new() -> PeerView {
        PeerView { arr: VecDeque::new(), ts: VecDeque::new(), delay: 60.0, last_shrink: 0.0, label: None, prev_label: None }
    }
}

#[derive(Clone)]
struct GdEvent {
    t: f64,
    ats: u32,
    vts: u32,
    bone: &'static str,
    loc: V3,
    raw: f32,
    loss: f32,
    stab: bool,
    contact: u64,
    view_lag: f64,
    kind: ClaimKind,
    effective: bool,
    native: f32,
    age_forge: u32,
    hand: bool,
    /// Physical maximum of this Get Damage call (game::bound).
    bound: f32,
    /// The game's per-bone gate swallowed it: no field changed.
    gated: bool,
    /// Size of the lie (uu) for geometric cheats.
    lie: f32,
    speed: f32,
    s_blade: f32,
    /// Armour-stage inputs the stand-in measured (blade contacts).
    dcd: Option<Dcd>,
    /// Solo Health loss of the same contact (true relative motion).
    solo: f32,
    solo_vrel: f32,
    /// Capsule segment touched and the true time of the victim pose shown.
    seg: usize,
    shown_t: f64,
}

#[derive(Clone, Copy, Debug)]
pub struct Dcd {
    pub dir: V3,
    pub hv: f32,
    pub ni: f32,
    pub vrel_s: f32,
    pub k: game::Contact,
    pub armour: damage::Armour,
}

#[derive(Default)]
struct Episode {
    open: bool,
    last_event: f64,
    last_sent: f64,
    acc: Vec<GdEvent>,
}

struct Pending {
    rec: usize,
    created: f64,
    last_sent: f64,
    hit: DamageEvent,
}

struct Client {
    pid: PeerId,
    clk0: f64,
    rate: f64,
    up: Link,
    down: Link,
    next_frame: f64,
    next_lua: f64,
    next_sidecar: f64,
    views: Vec<PeerView>,
    events: Vec<Vec<GdEvent>>,
    episodes: HashMap<(usize, &'static str), Episode>,
    clash_last: Vec<f64>,
    deflect_until: Vec<f64>,
    /// End of the current two-frame touch per target (then 200 ms stopped).
    stop_until: Vec<f64>,
    /// contact tracking per target: (last frame t, id)
    contact: Vec<(f64, u64)>,
    /// (target, bone) → last time that bone took damage ("Last Damage Taken").
    bone_gate: HashMap<(usize, &'static str), f64>,
    cheat_last: Vec<f64>,
    pending: BTreeMap<u32, Pending>,
    next_hit_id: u32,
    seen: HashSet<(usize, u32)>,
    /// Lua-visible feedback queued for the next Lua tick
    lua_inbox: Vec<(usize, bool, bool)>, // (rec, is_apply, is_confirm)
    rng: Rng,
    cheat: Option<Cheat>,
    /// Connection-layer RTT estimator (hsmp-net conn.rs, RFC 6298): srtt, rttvar.
    srtt: Option<f64>,
    rttvar: f64,
}

impl Client {
    fn clock(&self, t: f64) -> u32 { (self.clk0 + t * self.rate).floor().max(1.0) as u32 }
    /// True time at which this client's clock read `ts`.
    fn true_of(&self, ts: f64) -> f64 { (ts - self.clk0) / self.rate }
}

static NEXT_PID: AtomicU32 = AtomicU32::new(100_000);

pub struct World {
    pub cfg: Config,
    pub plan: Plan,
    clients: Vec<Client>,
    core: Core,
    q: BTreeMap<(u64, u64), Msg>,
    qseq: u64,
    pub claims: Vec<ClaimRec>,
    /// Claim payloads (same index as `claims`; Msg is Copy).
    pub pending_hits: Vec<DamageEvent>,
    /// age_ms each claim left Lua with (same index).
    base_age: Vec<u32>,
    hitmap: HashMap<(usize, u32), usize>,
    alive: Vec<bool>,
    dead_at: Vec<f64>,
    next_tick: f64,
    /// Pose samples held for the next tick (ablations): (from, ts, true t).
    tick_records: Vec<(usize, u32, f64)>,
    tick_relays: Vec<(usize, u32, f64)>,
    next_rtt: f64,
    next_contact: u64,
    /// screen clashes: (client, peer, true t, true time of the peer pose shown)
    pub screen_clashes: Vec<(usize, usize, f64, f64)>,
    /// glints delivered: (to, other, true t)
    pub glints: Vec<(usize, usize, f64)>,
    pub contacts_honest: HashSet<(usize, u64)>,
    pub cheat_attempts: HashMap<Cheat, u32>,
    /// FakeParry cheat: (cheater, attacker shown, true t, effective, true time of
    /// the attacker pose shown) per faked frame.
    pub fake_parries: Vec<(usize, usize, f64, bool, f64)>,
    /// FakeParry: the blade streamed instead of the true one, by frame time (ms).
    fake_blade: HashMap<usize, BTreeMap<i64, Blade>>,
    rng: Rng,
    pub last_s: f32,
    pub(crate) health: health::HealthState,
}

fn bone_index(name: &str) -> usize { BONE_NAMES.iter().position(|b| *b == name).unwrap_or(0) }

impl World {
    pub fn new(cfg: Config) -> World {
        let mut rng = Rng::new(cfg.seed);
        let display_ms = 2.0 * cfg.profile.delay + 16.0 + 1.6 * cfg.profile.jitter + 6.0;
        let extra: Vec<f32> = (0..cfg.players).map(|i| match cfg.cheats.iter().find(|c| c.0 == i).map(|c| c.1) {
            Some(Cheat::ReachBlade) => 50.0,
            Some(Cheat::ReachArm) => 40.0,
            Some(Cheat::ReachFake) => 45.0,
            _ => 0.0,
        }).collect();
        let plan = script::plan(cfg.players, cfg.dur_ms, cfg.mix, display_ms, &extra, &mut rng);
        let n = cfg.players;
        let base_pid = NEXT_PID.fetch_add(n as u32 + 1, Ordering::Relaxed);
        let mut clients = Vec::new();
        for i in 0..n {
            let mut r = rng.fork(i as u64 + 1);
            let cheat = cfg.cheats.iter().find(|c| c.0 == i).map(|c| c.1);
            let drift = r.range(-cfg.drift_ppm, cfg.drift_ppm) * 1e-6;
            let rate = if cheat == Some(Cheat::Speedhack) { 1.3 } else { 1.0 + drift };
            let pid = base_pid + i as u32;
            damage::set_kit(pid, &plan.fighters[i].kit());
            clients.push(Client {
                pid,
                clk0: r.range(20_000.0, 2_000_000.0),
                rate,
                up: Link::new(cfg.profile, &mut r),
                down: Link::new(cfg.profile, &mut r),
                next_frame: r.range(0.0, FRAME_MS),
                next_lua: r.range(0.0, LUA_TICK_MS),
                next_sidecar: r.range(0.0, SIDECAR_TICK_MS),
                views: (0..n).map(|_| PeerView::new()).collect(),
                events: vec![Vec::new(); n],
                episodes: HashMap::new(),
                clash_last: vec![-1e9; n],
                deflect_until: vec![-1e9; n],
                stop_until: vec![-1e9; n],
                contact: vec![(-1e9, 0); n],
                bone_gate: HashMap::new(),
                cheat_last: vec![-1e9; n],
                pending: BTreeMap::new(),
                next_hit_id: 1,
                seen: HashSet::new(),
                lua_inbox: Vec::new(),
                rng: r.fork(99),
                srtt: None,
                rttvar: 0.0,
                cheat,
            });
        }
        World {
            core: Core::new(),
            q: BTreeMap::new(),
            qseq: 0,
            claims: Vec::new(),
            pending_hits: Vec::new(),
            base_age: Vec::new(),
            hitmap: HashMap::new(),
            alive: vec![true; n],
            dead_at: vec![-1e9; n],
            next_tick: 0.0,
            tick_records: Vec::new(),
            tick_relays: Vec::new(),
            next_rtt: 0.0,
            next_contact: 1,
            screen_clashes: Vec::new(),
            glints: Vec::new(),
            contacts_honest: HashSet::new(),
            cheat_attempts: HashMap::new(),
            fake_parries: Vec::new(),
            fake_blade: HashMap::new(),
            rng: rng.fork(5),
            last_s: 0.0,
            health: health::HealthState::new(n),
            cfg,
            plan,
            clients,
        }
    }

    pub fn pid(&self, i: usize) -> PeerId { self.clients[i].pid }

    fn push(&mut self, at: f64, m: Msg) {
        self.qseq += 1;
        self.q.insert(((at * 1000.0).max(0.0) as u64, self.qseq), m);
    }

    fn rto(&self, c: usize) -> f64 { self.clients[c].up.rto(&self.clients[c].down) }

    // ---- streams --------------------------------------------------------------

    /// What client `i` streams about itself at true time t (cheats applied).
    fn stream_pose(&self, i: usize, t: f64) -> Pose {
        let f = &self.plan.fighters[i];
        let mut p = f.pose(t);
        match self.clients[i].cheat {
            Some(Cheat::ReachBlade) => {
                let d = norm(sub(p.blade.tip, p.blade.base));
                p.blade.tip = add(p.blade.tip, mul(d, 60.0));
            }
            Some(Cheat::ReachArm) => {
                // Stretch the arm 45 uu along itself (hand, blade, grip follow).
                let sh = p.bones[crate::posecodec::UPPERARM_R];
                let s = mul(norm(sub(p.bones[crate::posecodec::HAND_R], sh)), 45.0);
                p.bones[crate::posecodec::HAND_R] = add(p.bones[crate::posecodec::HAND_R], s);
                p.bones[crate::posecodec::LOWERARM_R] = add(p.bones[crate::posecodec::LOWERARM_R], mul(s, 0.5));
                p.blade.base = add(p.blade.base, s);
                p.blade.tip = add(p.blade.tip, s);
                p.grip = add(p.grip, s);
            }
            Some(Cheat::FakeParry) => {
                // The frame's faked blade (snapped onto an incoming attack).
                if let Some(m) = self.fake_blade.get(&i) {
                    let k = t.round() as i64;
                    if let Some((_, b)) = m.range(k - 2..=k + 2).next() { p.blade = *b; }
                }
            }
            _ => {}
        }
        p
    }

    fn server_record(&mut self, from: usize, ts: u32, t: f64, now: i64) {
        let p = self.stream_pose(from, t);
        let pid = self.clients[from].pid;
        let bones: Vec<(u8, [f32; 3], [f32; 4])> =
            p.bones.iter().enumerate().map(|(i, b)| (i as u8, *b, [0.0, 0.0, 0.0, 1.0])).collect();
        let fr = crate::posecodec::PoseFrame { ts, bones };
        let lc = &mut self.core.lc;
        lc.note_pose_codec(pid, self.cfg.stream_v2);
        lc.record_root(pid, ts, p.bones[0], now);
        lc.record_pose(pid, &fr, now);
        lc.record_weapon(pid, ts, p.grip, now);
        if self.cfg.stream_v2 {
            // Tip velocity (uu/s, per sender-clock second) by central difference.
            let c = &self.clients[from];
            let (a, b) = (self.stream_pose(from, t - 1.0), self.stream_pose(from, t + 1.0));
            let vel = mul(sub(b.blade.tip, a.blade.tip), 1000.0 / (2.0 * c.rate as f32));
            let lc = &mut self.core.lc;
            let degenerate = self.cfg.degenerate_polearm
                && self.plan.fighters[from].weapon.class == damage::WeaponClass::Polearm;
            let tip = if degenerate { add(p.grip, mul(norm(sub(p.blade.tip, p.blade.base)), 1.0)) } else { p.blade.tip };
            let base = if degenerate { p.grip } else { p.blade.base };
            lc.record_blade(pid, ts, lagcomp::Blade { base, tip, vel: Some(vel) }, now);
            let caps: Vec<lagcomp::Capsule> = self.plan.fighters[from].true_caps(&p).iter()
                .map(|c| lagcomp::Capsule { a: c.a, b: c.b, r: c.r }).collect();
            lc.record_capsules(pid, ts, &caps, now);
        }
    }

    // ---- client display ---------------------------------------------------------

    fn view_rx(&mut self, c: usize, from: usize, ts: u32, t: f64) {
        let rx = self.clients[c].clock(t) as f64;
        let v = &mut self.clients[c].views[from];
        v.arr.push_back((rx, ts));
        while v.arr.front().map_or(false, |e| rx - e.0 > 2000.0) { v.arr.pop_front(); }
        let i = v.ts.partition_point(|x| *x < ts);
        if i >= v.ts.len() || v.ts[i] != ts { v.ts.insert(i, ts); }
        while v.ts.len() > 64 { v.ts.pop_front(); }
        let off = v.arr.iter().map(|e| e.0 - e.1 as f64).fold(f64::INFINITY, f64::min);
        let mut late: Vec<f64> = v.arr.iter().map(|e| e.0 - e.1 as f64 - off).collect();
        late.sort_by(|a, b| a.total_cmp(b));
        let p90 = late[((late.len() as f64 * 0.9).ceil() as usize).clamp(1, late.len()) - 1];
        // POSE contract: v2 buffers clamp(p90 + 6, 16, 250); v1 adds the interval.
        let want = if self.cfg.stream_v2 { (p90 + 6.0).clamp(16.0, 250.0) } else { (FRAME_MS + p90 + 6.0).clamp(20.0, 250.0) };
        if want > v.delay {
            v.delay = want;
        } else if rx - v.last_shrink >= 50.0 {
            v.delay = (v.delay - 1.0).max(want);
            v.last_shrink = rx;
        }
    }

    /// Playback label (sender clock of `p`) client c shows at true time t.
    fn playback(&self, c: usize, p: usize, t: f64) -> Option<f64> {
        let v = &self.clients[c].views[p];
        if v.arr.len() < 3 { return None; }
        let off = v.arr.iter().map(|e| e.0 - e.1 as f64).fold(f64::INFINITY, f64::min);
        let pt = self.clients[c].clock(t) as f64 - off - v.delay;
        let newest = *v.ts.back()? as f64;
        if pt - newest > 1000.0 { return None; } // stale
        Some(pt)
    }

    /// The stand-in of p on c's screen at `label`: (pose, true time it shows).
    fn displayed(&mut self, c: usize, p: usize, label: f64) -> Option<(Pose, f64)> {
        let v = &self.clients[c].views[p];
        let newest = *v.ts.back()? as f64;
        let cp = &self.clients[p];
        let fp = &self.plan.fighters[p];
        let mut pose;
        let shown_t;
        if label <= newest {
            shown_t = cp.true_of(label);
            pose = self.stream_pose(p, shown_t);
        } else {
            // Extrapolate from the two newest samples (≤ 100 ms, ≤ 30 uu), then hold.
            let n = v.ts.len();
            let t2 = cp.true_of(newest);
            let t1 = if n >= 2 { cp.true_of(v.ts[n - 2] as f64) } else { t2 - 16.0 };
            let p2 = self.stream_pose(p, t2);
            let p1 = self.stream_pose(p, t1);
            let ahead = (cp.true_of(label) - t2).min(100.0);
            let k = (ahead / (t2 - t1).max(1.0)) as f32;
            let ex = |a: V3, b: V3| {
                let d = mul(sub(b, a), k);
                let l = len(d);
                add(b, if l > 30.0 { mul(d, 30.0 / l) } else { d })
            };
            pose = p2;
            for i in 0..16 { pose.bones[i] = ex(p1.bones[i], p2.bones[i]); }
            pose.blade.base = ex(p1.blade.base, p2.blade.base);
            pose.blade.tip = ex(p1.blade.tip, p2.blade.tip);
            pose.grip = ex(p1.grip, p2.grip);
            shown_t = cp.true_of(label); // what the label claims
        }
        // Servo noise.
        let sn = self.cfg.servo_noise;
        let r = &mut self.clients[c].rng;
        let mut nz = || [r.gauss() as f32 * sn, r.gauss() as f32 * sn, r.gauss() as f32 * sn];
        for b in pose.bones.iter_mut() { *b = add(*b, nz()); }
        let shift = nz();
        pose.blade.base = add(pose.blade.base, shift);
        pose.blade.tip = add(pose.blade.tip, add(shift, nz()));
        Some((pose, shown_t))
    }

    // ---- client frame --------------------------------------------------------

    fn client_frame(&mut self, c: usize, t: f64) {
        let n = self.cfg.players;
        // Own pose → stream.
        let ts = self.clients[c].clock(t);
        let mut ipc = self.clients[c].rng.range(1.0, 6.0);
        if matches!(self.clients[c].cheat, Some(Cheat::JitterInflate) | Some(Cheat::AckHold)) {
            ipc += self.clients[c].rng.range(0.0, 250.0);
        }
        for at in self.clients[c].up.send(t + ipc) {
            self.push(at, Msg::SPose { from: c, ts, t });
        }
        let me = self.plan.fighters[c].clone();
        let cheat = self.clients[c].cheat;
        // The blade this client's own screen collides with (cheats see their lie).
        let own = |w: &World, tt: f64| -> Pose {
            match cheat { Some(Cheat::ReachBlade) | Some(Cheat::ReachArm) => w.stream_pose(c, tt), _ => me.pose(tt) }
        };
        const SUB: usize = 4;
        let sweep: Vec<Pose> = (0..=SUB).map(|k| own(self, t - FRAME_MS + FRAME_MS * k as f64 / SUB as f64)).collect();
        let truth: Vec<Pose> = (0..=SUB).map(|k| me.pose(t - FRAME_MS + FRAME_MS * k as f64 / SUB as f64)).collect();
        for p in 0..n {
            if p == c { continue; }
            // Advance the display labels (servo shows last frame's target).
            let pt = self.playback(c, p, t);
            {
                let v = &mut self.clients[c].views[p];
                v.prev_label = v.label;
                v.label = pt;
            }
            let Some(label) = self.clients[c].views[p].prev_label else { continue };
            let Some((dp, shown_t)) = self.displayed(c, p, label) else { continue };
            let view_lag = t - shown_t;
            let caps = self.plan.fighters[p].true_caps(&dp);
            let vts = label.max(1.0) as u32;

            // Blade on the displayed blade: clash (both screens report).
            let mut clash = false;
            for b in &sweep {
                let (d, _, _) = seg_seg(b.blade.base, b.blade.tip, dp.blade.base, dp.blade.tip);
                if d <= 2.0 * BLADE_R + 1.0 { clash = true; break; }
            }
            if clash && self.alive[c] {
                self.screen_clashes.push((c, p, t, shown_t));
                let cl = &mut self.clients[c];
                cl.deflect_until[p] = t + 200.0;
                let now_ms = cl.clock(t) as f64;
                if now_ms - cl.clash_last[p] >= CLASH_GAP_MS {
                    cl.clash_last[p] = now_ms;
                    let pick = t + cl.rng.range(0.5, SIDECAR_TICK_MS + 0.5);
                    self.push(pick, Msg::PickupClash { client: c, other: p, my_ts: now_ms as u32, other_ts: vts });
                }
            }
            if cheat == Some(Cheat::FakeParry) && !clash && self.alive[c] && self.alive[p] {
                let tp = me.pose(t);
                let mine = self.plan.fighters[c].true_caps(&tp);
                let near = mine.iter().map(|k| seg_seg(dp.blade.base, dp.blade.tip, k.a, k.b).0 - k.r)
                    .fold(f32::INFINITY, f32::min);
                // The FakeParry attack: stream the attacker's displayed blade
                // segment as my own for every frame the swing is near, and report
                // the clash (rate-limited like an honest client).
                if near <= 60.0 && !self.fake_blade.get(&c).map_or(false, |m| m.contains_key(&(t.round() as i64))) {
                    let fake = dp.blade;
                    // Effective: no hand of mine could hold that segment by its hilt
                    // (lagcomp::blade_unholdable: base within my class's grip distance
                    // of a hand; polearms anywhere along the haft) or it is longer
                    // than my weapon class allows.
                    let g = crate::validate::pose::geom(self.plan.fighters[c].weapon.class);
                    let pole = self.plan.fighters[c].weapon.class == damage::WeaponClass::Polearm;
                    let grip = [crate::posecodec::HAND_R, crate::posecodec::HAND_L].iter()
                        .map(|&h| if pole { point_seg(tp.bones[h], fake.base, fake.tip).0 } else { len(sub(tp.bones[h], fake.base)) })
                        .fold(f32::INFINITY, f32::min);
                    let effective = grip > g.grip_max + 5.0 || len(sub(fake.tip, fake.base)) > g.blade_max + 5.0;
                    self.fake_blade.entry(c).or_default().insert(t.round() as i64, fake);
                    self.fake_parries.push((c, p, t, effective, shown_t));
                    let cl = &mut self.clients[c];
                    let now_ms = cl.clock(t) as f64;
                    if now_ms - cl.clash_last[p] >= CLASH_GAP_MS {
                        cl.clash_last[p] = now_ms;
                        let pick = t + cl.rng.range(0.5, SIDECAR_TICK_MS + 0.5);
                        self.push(pick, Msg::PickupClash { client: c, other: p, my_ts: now_ms as u32, other_ts: vts });
                    }
                }
            }
            if cheat == Some(Cheat::ParrySpam) {
                let cl = &mut self.clients[c];
                let now_ms = cl.clock(t);
                // Way past the client's own limit (the sidecar sends whatever Lua writes).
                let pick = t + cl.rng.range(0.5, SIDECAR_TICK_MS + 0.5);
                self.push(pick, Msg::PickupClash { client: c, other: p, my_ts: now_ms, other_ts: vts });
            }
            if !self.alive[c] || !self.alive[p] { continue; }
            if self.clients[c].deflect_until[p] > t { continue; }
            // Blade stopped by the body after a two-frame touch (see below).
            let st = self.clients[c].stop_until[p];
            if t > st && t < st + 200.0 { continue; }

            // Blade (swept) and hands on the displayed body.
            let mut hits: Vec<(usize, V3, f32, bool, f32, V3)> = Vec::new(); // (seg, loc, speed, is_hand)
            let mut min_true = f32::INFINITY;
            for cap in &caps {
                let mut best: Option<(f32, V3, usize)> = None;
                for (k, b) in sweep.iter().enumerate() {
                    let (d, _, _) = seg_seg(b.blade.base, b.blade.tip, cap.a, cap.b);
                    if d - cap.r - BLADE_R > 0.0 { continue; }
                    // First touch: bisect between the previous sub-step (outside)
                    // and this one, so the contact lies on the body surface
                    // where the blade met it (the physics ImpactPoint).
                    let mut bl = b.blade;
                    if k > 0 {
                        let (o, i) = (sweep[k - 1].blade, b.blade);
                        let (mut lo, mut hi) = (0.0f32, 1.0f32);
                        for _ in 0..12 {
                            let m = 0.5 * (lo + hi);
                            let x = Blade { base: lerp3(o.base, i.base, m), tip: lerp3(o.tip, i.tip, m) };
                            let (dm, _, _) = seg_seg(x.base, x.tip, cap.a, cap.b);
                            if dm - cap.r - BLADE_R > 0.0 { lo = m } else { hi = m }
                        }
                        bl = Blade { base: lerp3(o.base, i.base, hi), tip: lerp3(o.tip, i.tip, hi) };
                    }
                    let (d, s, u) = seg_seg(bl.base, bl.tip, cap.a, cap.b);
                    let on_axis = lerp3(cap.a, cap.b, u);
                    let on_blade = lerp3(bl.base, bl.tip, s);
                    let dir = if d > 1e-3 { norm(sub(on_blade, on_axis)) } else { norm(sub(bl.base, on_axis)) };
                    // Touching: the surface point facing the blade. Already inside
                    // (blade embedded, or the shown body moved onto it between
                    // frames): the contact manifold lies on the blade itself.
                    let loc = if d >= cap.r { add(on_axis, mul(dir, cap.r)) } else { on_blade };
                    best = Some((s, loc, k));
                    break;
                }
                for b in &truth {
                    let (d, _, _) = seg_seg(b.blade.base, b.blade.tip, cap.a, cap.b);
                    min_true = min_true.min(d - cap.r - BLADE_R);
                }
                if let Some((s, loc, k)) = best {
                    let k1 = k.max(1);
                    let (b0, b1) = (&sweep[k1 - 1], &sweep[k1]);
                    let pa = lerp3(b0.blade.base, b0.blade.tip, s);
                    let pb = lerp3(b1.blade.base, b1.blade.tip, s);
                    // (an action switch can teleport the scripted blade: physics can't)
                    let speed = len(sub(pb, pa)) / (FRAME_MS as f32 / SUB as f32) * 1000.0;
                    if speed > 4500.0 { continue; }
                    let va = mul(norm(sub(pb, pa)), speed);
                    hits.push((cap.seg, loc, speed, false, s, va));
                }
                // Hands (fists / pommel / clinch grabs) on the body.
                for h in [crate::posecodec::HAND_R, crate::posecodec::HAND_L] {
                    let hp = sweep[SUB].bones[h];
                    let (d, u) = point_seg(hp, cap.a, cap.b);
                    if d <= cap.r + 5.0 {
                        let on_axis = lerp3(cap.a, cap.b, u);
                        hits.push((cap.seg, add(on_axis, mul(norm(sub(hp, on_axis)), cap.r)), 150.0, true, 0.0, [0.0; 3]));
                    }
                }
            }
            let honest_contact = !hits.is_empty() && min_true <= 0.0 || hits.iter().all(|h| h.3) && !hits.is_empty();
            let eng = self.engagement_of(c, p, t);
            let lethal = eng.map_or(false, |e| e.1 == Kind::Trade);
            if !hits.is_empty() {
                // Contact id: consecutive frames on this target = one contact.
                let cid = {
                    let cl = &mut self.clients[c];
                    let ct = &mut cl.contact[p];
                    if t - ct.0 > 2.5 * FRAME_MS {
                        ct.1 = self.next_contact;
                        self.next_contact += 1;
                        // Physics: the body stops / deflects the blade. The scripted
                        // blade would pass through; the touch is this frame only,
                        // then no body contact from this swing for 200 ms.
                        cl.stop_until[p] = t + 0.5 * FRAME_MS;
                    }
                    let ct = &mut cl.contact[p];
                    ct.0 = t;
                    ct.1
                };
                let ats = self.clients[c].clock(t);
                let w = self.plan.fighters[c].weapon;
                let reach_cheat = matches!(cheat, Some(Cheat::ReachBlade) | Some(Cheat::ReachArm));
                // A cheating client's claims never count as honest (its reach
                // cheat only "needs" the lie where the true blade missed by
                // more than the server's tolerances: contact → blade 18 uu).
                let kind = match cheat { Some(ch) => ClaimKind::Cheat(ch), None => ClaimKind::Honest };
                // ReachArm: only a lie when the streamed hand is beyond an honest
                // full extension (a retracted arm stretched 45 uu reaches no
                // further than an honest straight one).
                let arm_ok = cheat != Some(Cheat::ReachArm) || {
                    let sp = self.stream_pose(c, t);
                    len(sub(sp.bones[crate::posecodec::HAND_R], sp.bones[crate::posecodec::UPPERARM_R])) > crate::lagcomp::ARM_REACH_MAX
                };
                let effective = reach_cheat && min_true > 25.0 && arm_ok;
                let _ = ARM;
                for (seg, loc, speed, is_hand, s_blade, va) in hits {
                    let bone = SEGS[seg].3;
                    let stab = speed < 900.0 && !is_hand && self.thrusting(c, t);
                    let armour = game::armour_over(&self.plan.fighters[p].armour, bone);
                    let class = if is_hand { damage::WeaponClass::Unarmed } else { w.class };
                    // The game's "Last Damage Taken" gate: a bone hit again within
                    // 0.2 s by a weaker blow takes nothing (sliding contact).
                    let key = (p, bone);
                    let gated = self.clients[c].bone_gate.get(&key).map_or(false, |&t0| t - t0 < 200.0);
                    if !gated { self.clients[c].bone_gate.insert(key, t); }
                    // Solo vs MP (blade contacts): one draw of the hidden physics; solo
                    // uses the TRUE relative motion, the stand-in measurement the
                    // servo-perturbed one (the stand-in's armour mirrors the victim's).
                    let mut dcd: Option<Dcd> = None;
                    let mut solo = 0.0f32;
                    let mut solo_vrel = 0.0f32;
                    let native = if gated { 0.0 } else if is_hand {
                        game::native_loss(class, speed, armour, stab, bone, &mut self.clients[c].rng)
                    } else {
                        let k = game::draw_contact(class, speed, stab, &mut self.clients[c].rng);
                        let vv = self.body_velocity(p, shown_t, seg);
                        let sv = self.cfg.servo_noise * SERVO_VEL_K;
                        let r = &mut self.clients[c].rng;
                        let servo = [r.gauss() as f32 * sv, r.gauss() as f32 * sv, r.gauss() as f32 * sv];
                        let vrel_t = len(sub(va, vv));
                        solo_vrel = vrel_t;
                        let vrel_s = len(sub(va, add(vv, servo)));
                        solo = game::contact_loss(&k, vrel_t, armour, stab, bone).0;
                        let (meas, hv, ni) = game::contact_loss(&k, vrel_s, armour, stab, bone);
                        dcd = Some(Dcd { dir: norm(va), hv, ni, vrel_s, k, armour });
                        meas
                    };
                    // Physical maximum ignoring the gate (the victim-side gate state is
                    // invisible to the server; a claim on a gated bone is judged as fresh).
                    // (blade contacts: HitVel ≤ max(weapon speed, hvf·relative speed): a body
                    // running onto the blade is the relative motion, not the blade's.)
                    let bound = game::bound(class, speed.max(solo_vrel), armour, stab, bone);
                    let kind_e = kind;
                    if kind_e == ClaimKind::Honest { self.contacts_honest.insert((c, cid)); }
                    self.clients[c].events[p].push(GdEvent {
                        t, ats, vts, bone, loc, raw: native * 40.0, loss: native, stab, contact: cid,
                        view_lag, kind: kind_e, effective, native, age_forge: 0, hand: is_hand, bound, gated, lie: if reach_cheat { min_true } else { 0.0 }, speed, s_blade,
                        dcd, solo, solo_vrel, seg, shown_t,
                    });
                }
                let _ = honest_contact;
                let _ = lethal;
            } else if min_true.is_finite() {
                self.cheat_claims(c, p, t, &caps, &dp, label, view_lag, &sweep, min_true);
            }
        }
    }

    fn thrusting(&self, c: usize, t: f64) -> bool {
        self.plan.fighters[c].acts.iter().any(|a| matches!(a, Act::Thrust { t0, dur, .. } if t >= *t0 - 20.0 && t <= *t0 + *dur + 20.0))
    }

    fn engagement_of(&self, a: usize, d: usize, t: f64) -> Option<(usize, Kind)> {
        self.plan.engagements.iter().enumerate()
            .filter(|(_, e)| e.attacker == a && e.defender == d && (t - e.t_c).abs() < 700.0)
            .min_by(|x, y| (t - x.1.t_c).abs().total_cmp(&(t - y.1.t_c).abs()))
            .map(|(i, e)| (i, e.kind))
    }

    /// Cheat claims that need no honest contact (backtrack, fake reach).
    #[allow(clippy::too_many_arguments)]
    fn cheat_claims(&mut self, c: usize, p: usize, t: f64, caps: &[Capsule], dp: &Pose, label: f64,
                    view_lag: f64, sweep: &[Pose], min_true: f32) {
        let Some(cheat) = self.clients[c].cheat else { return };
        let gap = if cheat == Cheat::Backtrack { 100.0 } else { 250.0 };
        if t - self.clients[c].cheat_last[p] < gap || !self.alive[p] { return; }
        let ats = self.clients[c].clock(t);
        let mut emit = |w: &mut World, vts: u32, loc: V3, bone: &'static str, lag: f64, ch: Cheat, lie: f32| {
            w.clients[c].cheat_last[p] = t;
            let cid = w.next_contact;
            w.next_contact += 1;
            *w.cheat_attempts.entry(ch).or_insert(0) += 1;
            w.clients[c].events[p].push(GdEvent {
                t, ats, vts, bone, loc, raw: 800.0, loss: 20.0, stab: false, contact: cid, view_lag: lag,
                kind: ClaimKind::Cheat(ch), effective: true, native: 0.0, age_forge: 0, hand: false, bound: 0.0, gated: false, lie, speed: 0.0, s_blade: 0.0, dcd: None, solo: 0.0, solo_vrel: 0.0, seg: 0, shown_t: 0.0,
            });
        };
        match cheat {
            Cheat::ReachFake if min_true > 25.0 && min_true <= 120.0 => {
                // Nearest body point to the blade's tip path.
                let mut best = (f32::INFINITY, [0.0; 3], 0usize);
                for cap in caps {
                    for b in sweep {
                        let (d, s, u) = seg_seg(b.blade.base, b.blade.tip, cap.a, cap.b);
                        if d < best.0 {
                            let ax = lerp3(cap.a, cap.b, u);
                            let bl = lerp3(b.blade.base, b.blade.tip, s);
                            best = (d, add(ax, mul(norm(sub(bl, ax)), cap.r)), cap.seg);
                        }
                    }
                }
                // Not a lie where its own body touches the victim there (an
                // unarmed bump is a real contact).
                let own = self.plan.fighters[c].true_caps(&sweep[sweep.len() - 1]);
                let touch = own.iter().map(|k| point_seg(best.1, k.a, k.b).0 - k.r).fold(f32::INFINITY, f32::min);
                if touch > 15.0 {
                    emit(self, label as u32, best.1, SEGS[best.2].3, view_lag, Cheat::ReachFake, min_true);
                }
            }
            Cheat::Backtrack => {
                let cp_rate = self.clients[p].rate;
                for back in [150.0, 225.0, 300.0, 400.0, 500.0] {
                    let old_label = label - back * cp_rate;
                    let old_t = self.clients[p].true_of(old_label);
                    let op = self.stream_pose(p, old_t);
                    let ocaps = self.plan.fighters[p].true_caps(&op);
                    for cap in &ocaps {
                        for b in sweep {
                            let (d, s, u) = seg_seg(b.blade.base, b.blade.tip, cap.a, cap.b);
                            if d - cap.r - BLADE_R <= 0.0 {
                                let ax = lerp3(cap.a, cap.b, u);
                                let bl = lerp3(b.blade.base, b.blade.tip, s);
                                let loc = add(ax, mul(norm(sub(bl, ax)), cap.r));
                                // Only a lie if the victim was ≥ 35 uu away (body tolerance
                                // 25 + 10) at EVERY time the server could honestly judge:
                                // the shown label ± 60 ms (view window).
                                let shown = honest_gap(self, p, label, loc);
                                if shown > 35.0 {
                                    emit(self, old_label as u32, loc, SEGS[cap.seg].3, t - old_t, Cheat::Backtrack, shown);
                                    return;
                                }
                            }
                        }
                    }
                }
            }
            _ => {}
        }
    }

    // ---- Lua tick (HSMPCombat flush_measurements + feedback) -----------------------

    fn lua_tick(&mut self, c: usize, t: f64) {
        let n = self.cfg.players;
        if self.cfg.health_model && self.stalled(c, t) {
            // Hung Lua: nothing measured, applied or sampled.
            for e in self.clients[c].events.iter_mut() { e.clear(); }
            return;
        }
        // Feedback (acks / applies) becomes visible to Lua now.
        let inbox: Vec<(usize, bool, bool)> = std::mem::take(&mut self.clients[c].lua_inbox);
        let mut applied_now = false;
        for (rec, is_apply, is_confirm) in inbox {
            {
                let r = &mut self.claims[rec];
                if is_apply { if r.applied_t.is_none() { r.applied_t = Some(t); } }
                else if is_confirm && r.confirm_t.is_none() { r.confirm_t = Some(t); }
            }
            if is_apply && self.cfg.health_model { applied_now |= self.apply_replay(c, rec, t); }
        }
        if self.cfg.health_model { self.vitals_lua(c, t, applied_now); }
        for p in 0..n {
            if p == c { continue; }
            let evs: Vec<GdEvent> = std::mem::take(&mut self.clients[c].events[p]);
            match self.cfg.policy {
                Policy::Legacy => {
                    for e in &evs { self.emit_claim(c, p, t, &[e.clone()]); }
                }
                Policy::Dedupe => self.dedupe_tick(c, p, t, evs),
            }
        }
    }

    /// HSMPCombat dedupe: ONE claim when a contact episode starts (this tick's
    /// events merged), then at most one continuation per CONT_MS while it
    /// lasts, flushed when the episode closes (EPISODE_GAP_MS without events).
    fn dedupe_tick(&mut self, c: usize, p: usize, t: f64, evs: Vec<GdEvent>) {
        // A cheater sends each fabricated contact as its own claim (merging it
        // into a real contact's claim would only hide it behind real geometry).
        let (fake, evs): (Vec<GdEvent>, Vec<GdEvent>) = evs.into_iter()
            .partition(|e| e.effective && e.lie > 0.0 && matches!(e.kind, ClaimKind::Cheat(Cheat::Backtrack) | ClaimKind::Cheat(Cheat::ReachFake)));
        for f in fake { self.emit_claim(c, p, t, &[f]); }
        // Episodes per (target, BONE): the game's damage gate is per bone, and
        // the victim replays one armour stage per claim — every body the blade
        // touched must reach the victim, each once per contact.
        let mut by_bone: Vec<(&'static str, Vec<GdEvent>)> = Vec::new();
        for e in evs {
            match by_bone.iter_mut().find(|x| x.0 == e.bone) { Some(x) => x.1.push(e), None => by_bone.push((e.bone, vec![e])) }
        }
        // Close episodes of this target that saw no call this tick.
        let open: Vec<&'static str> = self.clients[c].episodes.keys().filter(|k| k.0 == p).map(|k| k.1).collect();
        for b in open {
            if by_bone.iter().any(|x| x.0 == b) { continue; }
            let ep = self.clients[c].episodes.get_mut(&(p, b)).unwrap();
            if t - ep.last_event > EPISODE_GAP_MS {
                let acc = std::mem::take(&mut ep.acc);
                self.clients[c].episodes.remove(&(p, b));
                if !acc.is_empty() { self.emit_claim(c, p, t, &acc); }
            }
        }
        for (b, evs) in by_bone {
            let last_t = evs.iter().map(|e| e.t).fold(f64::MIN, f64::max);
            let ep = self.clients[c].episodes.entry((p, b)).or_default();
            if !ep.open || last_t - ep.last_event > EPISODE_GAP_MS {
                let acc = std::mem::take(&mut ep.acc);
                ep.open = true;
                ep.last_event = last_t;
                ep.last_sent = t;
                if !acc.is_empty() { self.emit_claim(c, p, t, &acc); }
                self.emit_claim(c, p, t, &evs);
                continue;
            }
            ep.last_event = last_t;
            ep.acc.extend(evs);
            if t - ep.last_sent >= CONT_MS {
                ep.last_sent = t;
                let acc = std::mem::take(&mut ep.acc);
                self.emit_claim(c, p, t, &acc);
            }
        }
    }

    /// One outbox line from events (params of the strongest, deltas summed).
    fn emit_claim(&mut self, c: usize, p: usize, t: f64, evs: &[GdEvent]) {
        // Nothing measured (every call swallowed by the game's per-bone gate):
        // HSMPCombat sends no claim.
        if evs.is_empty() || evs.iter().all(|e| e.gated) { return; }
        // Params of the strongest event of the NEWEST frame (a continuation must
        // carry a fresh ats: the server checks it against the arrival).
        // The claim carries the STRONGEST call merged (part whose Health loss
        // dominates — game kH order —, then raw damage) with its own ats / vts /
        // bone / location; HSMPCombat adds how long ago that was ("lage") so
        // the server's arrival check stays exact for an older event.
        let main = evs.iter()
            .max_by(|a, b| game::part_rank(a.bone).cmp(&game::part_rank(b.bone)).then(a.raw.total_cmp(&b.raw)))
            .unwrap().clone();
        // Each bone takes damage at most once per gate window (the strongest
        // blow): the merged claim's physical maximum is the sum over bones.
        let bound_sum: f32 = {
            let mut per: Vec<(&str, f32)> = Vec::new();
            for e in evs { match per.iter_mut().find(|x| x.0 == e.bone) { Some(x) => x.1 = x.1.max(e.bound), None => per.push((e.bone, e.bound)) } }
            per.iter().map(|x| x.1).sum()
        };
        let first_t = evs.iter().map(|e| e.t).fold(f64::MAX, f64::min);
        let mut loss: f32 = evs.iter().map(|e| e.loss).sum();
        let native: f32 = evs.iter().map(|e| e.native).sum();
        let cheat = self.clients[c].cheat;
        // Claims from a cheating client are never honest; `effective` marks
        // the ones that actually carry the lie (event-level for reach and
        // backtrack cheats, set below for the claim-level ones).
        let mut kind = match cheat { Some(ch) => ClaimKind::Cheat(ch), None => ClaimKind::Honest };
        let lie_events = evs.iter().any(|e| e.effective && matches!(e.kind, ClaimKind::Cheat(_)));
        let mut eff = match cheat {
            // The claim carries the main event's geometry: only a lie if that is.
            Some(Cheat::ReachBlade) | Some(Cheat::ReachArm) => main.effective && matches!(main.kind, ClaimKind::Cheat(_)),
            Some(Cheat::Backtrack) | Some(Cheat::ReachFake) => lie_events,
            Some(Cheat::DamageInflate) | Some(Cheat::Speedhack) => true,
            Some(Cheat::ParrySpam) | Some(Cheat::TsForge) | Some(Cheat::JitterInflate) | Some(Cheat::AckHold) | Some(Cheat::FakeParry)
            | Some(Cheat::GodMode) | None => false,
        };
        let eng = self.engagement_of(c, p, main.t);
        let lethal = eng.map_or(false, |e| e.1 == Kind::Trade) && kind == ClaimKind::Honest;
        let mut ats = main.ats;
        let mut vts = main.vts;
        let mut age_forge = 0u32;
        match cheat {
            Some(Cheat::DamageInflate) => { loss = 95.0; }
            Some(Cheat::TsForge) => {
                // Re-stamp the hit Δ ms off; only a lie if the blade at that
                // instant was ≥ 30 uu from the contact.
                let d = *self.clients[c].rng.pick(&[-400.0, -250.0, -150.0, 60.0, 120.0]);
                let tt = main.t + d;
                let bp = self.plan.fighters[c].pose(tt);
                let miss = point_seg(main.loc, bp.blade.base, bp.blade.tip).0;
                // (bodies touching there: an unarmed contact is real)
                let touch = self.plan.fighters[c].true_caps(&bp).iter()
                    .map(|k| point_seg(main.loc, k.a, k.b).0 - k.r).fold(f32::INFINITY, f32::min);
                ats = self.clients[c].clock(tt);
                vts = (main.vts as f64 + d * self.clients[p].rate).max(1.0) as u32;
                if d < 0.0 { age_forge = (-d) as u32; }
                eff = miss > 30.0 && touch > 15.0;
            }
            _ => {}
        }
        let _ = &mut kind;
        if let (ClaimKind::Cheat(ch), true) = (kind, eff) {
            if matches!(ch, Cheat::DamageInflate | Cheat::Speedhack | Cheat::TsForge) {
                *self.cheat_attempts.entry(ch).or_insert(0) += 1;
            }
        }
        let flags = if main.stab { damage::FLAG_STAB } else { 0 };
        let deltas: Vec<(u8, f32)> = {
                // Every field the stand-in measurement reports (game formula per
                // call; summed over the merged calls, the DRS bookkeeping fields
                // — Sustained / Last Damage Taken — are the largest DRS).
                let w = self.plan.fighters[c].weapon;
                let mut acc: std::collections::BTreeMap<u8, f32> = std::collections::BTreeMap::new();
                for e in evs {
                    let cls = if e.hand { damage::WeaponClass::Unarmed } else { w.class };
                    let f = game::fields_for(e.loss, e.speed, cls, e.bone, &mut self.clients[c].rng);
                    for (i, v) in f.d {
                        let x = acc.entry(i).or_insert(0.0);
                        if i == 15 || i == 17 { *x = x.max(v) } else { *x += v }
                    }
                }
                let hp = if lethal { 100.0 } else { loss.min(100.0) };
                if hp > 0.0 { acc.insert(combat::FIELD_HEALTH, -hp); } else { acc.remove(&combat::FIELD_HEALTH); }
                acc.into_iter().collect()
        };
        let hit = DamageEvent::new(crate::proto::Damage {
            hit_id: 0, target_peer_id: self.clients[p].pid, round: 1, bone: hsmp_ipc::layout::Str::new(main.bone),
            offset: [0.0; 3], location: main.loc, impulse: [0.0; 3], velocity: [0.0; 3], normal: [1.0, 0.0, 0.0],
            raw_damage: main.raw, cutting_power: 1.0, pain_rate: 1.0, draw_cut: 0.0,
            damage_out: loss.min(150.0), dism_blunt: 0, flags, age_ms: age_forge + (t - main.t).max(0.0) as u32,
            attacker_ts: ats, victim_view_ts: vts, victim_arm_ts: vts, ..Default::default()
        }, &crate::proto::deltas_of(&deltas));
        // Armour-stage claim (HSMPCombat FLAG_COMPLEX) from the main event.
        let mut hit = hit;
        if let Some(x) = main.dcd {
            hit.velocity = mul(x.dir, x.hv);
            hit.impulse = mul(x.dir, x.ni);
            hit.cutting_power = x.k.cp;
            hit.pain_rate = x.k.s;
            hit.damage_out = x.k.r;
            hit.raw_damage = x.vrel_s;
            hit.dism_blunt = 10 << 8;
            hit.flags |= damage::FLAG_COMPLEX;
            if cheat == Some(Cheat::DamageInflate) {
                // Inflate the armour-stage inputs too, and under-report the stand-in
                // speed (the server's rescale toward the real relative speed).
                hit.velocity = mul(hit.velocity, 4.0);
                hit.impulse = mul(hit.impulse, 4.0);
                hit.damage_out *= 3.0;
                hit.cutting_power = 0.0;
                hit.raw_damage = x.vrel_s / 5.0;
            }
        }
        let rec = self.claims.len();
        self.claims.push(ClaimRec {
            attacker: c, target: p, kind, t_hit: first_t, t_main: main.t, contact: main.contact, view_lag: main.view_lag,
            eng, lethal, claimed_loss: if lethal { 100.0 } else { loss.min(100.0) }, native_loss: native,
            booked_loss: None, outcome: None, confirm_t: None, applied_t: None, arrived_t: None, forwarded_t: None, effective: eff, hand: main.hand, n_events: evs.len(), bound_sum, lie: evs.iter().map(|e| e.lie).fold(0.0, f32::max), speed: main.speed, bone: main.bone, server_speed: None, parry_d: None, s_blade: main.s_blade,
            solo: if main.dcd.is_some() { Some(main.solo) } else { None }, mp: None,
            rel_dbg: main.dcd.map(|x| (main.solo_vrel, x.vrel_s, None, main.speed)),
            server_peak: None,
            geo: if kind == ClaimKind::Honest && !main.hand { Some((main.seg, main.shown_t, main.loc)) } else { None },
            pcell: main.dcd.map(|x| (w_class(&self.plan.fighters[c]), x.armour, game::part_rank(main.bone), x.k.hm, main.stab)),
        });
        self.base_age.push(hit.age_ms);
        self.pending_hits.push(hit);
        let pick = t + self.clients[c].rng.range(0.5, SIDECAR_TICK_MS + 0.5);
        self.push(pick, Msg::Pickup { client: c, rec });
    }
}


// ---- sidecar, server, run loop ----------------------------------------------------

impl World {
    /// Sidecar picked up a damage line: assign hit_id, send (reliable).
    fn sidecar_pickup(&mut self, c: usize, rec: usize, t: f64) {
        let cl = &mut self.clients[c];
        let id = cl.next_hit_id;
        cl.next_hit_id += 1;
        let mut hit = self.pending_hits[rec].clone();
        hit.hit_id = id;
        self.pending_hits[rec].hit_id = id;
        self.hitmap.insert((c, id), rec);
        cl.pending.insert(id, Pending { rec, created: t, last_sent: t, hit: hit.clone() });
        self.send_claim(c, rec, t);
    }

    fn send_claim(&mut self, c: usize, rec: usize, t: f64) {
        let rto = self.rto(c);
        for at in self.clients[c].up.send_reliable(t, rto) {
            self.push(at, Msg::SClaim { from: c, rec });
        }
    }

    fn sidecar_tick(&mut self, c: usize, t: f64) {
        if self.cfg.health_model && !self.stalled(c, t) { self.vitals_sidecar(c, t); }
        let mut resend = Vec::new();
        let mut give_up = Vec::new();
        for (id, p) in self.clients[c].pending.iter_mut() {
            if t - p.created > HIT_GIVE_UP_MS { give_up.push(*id); continue; }
            if t - p.last_sent >= RESEND_EVERY_MS {
                p.last_sent = t;
                resend.push((p.rec, (t - p.created) as u32));
            }
        }
        for id in give_up {
            if let Some(p) = self.clients[c].pending.remove(&id) {
                let r = &mut self.claims[p.rec];
                if r.outcome.is_none() { r.outcome = Some((false, "gave_up: never acked".into())); }
            }
        }
        for (rec, age) in resend {
            // age_ms = Lua's age of the hit when written (+ a TsForge lie) + the
            // sidecar's time since pickup.
            self.pending_hits[rec].age_ms = self.base_age[rec] + age;
            self.send_claim(c, rec, t);
        }
    }

    fn ctx(&self, a: usize, target: PeerId) -> Ctx {
        let tgt = self.clients.iter().position(|c| c.pid == target);
        Ctx {
            attacker_id: self.clients[a].pid,
            attacker_alive: self.alive[a],
            attacker_pos: None,
            target_exists: tgt.is_some(),
            target_alive: tgt.map_or(false, |i| self.alive[i]),
            target_pos: None,
            match_live: true,
            match_round: 1,
            // The sim's claims carry no pose context (match 0, life 0), like a pre-protocol-11 hit.
            match_id: 0,
            attacker_life: 0,
            target_life: 0,
        }
    }

    fn glue(&mut self, a: usize, rec: usize, mut hit: DamageEvent, v: Verdict, t: f64) {
        let target = self.claims[rec].target;
        let now = t as i64;
        let mut v = v;
        if v == Verdict::Forward {
            if !self.alive[target] {
                self.core.reject_decision(self.clients[a].pid, hit.hit_id, "round_over: round over / target down");
                v = Verdict::Ack { accepted: false, reason: "round_over: round over / target down".into() };
            } else {
                self.claims[rec].booked_loss = Some(combat::hit_loss(&hit));
                if self.cfg.health_model { self.ledger_forward(a, target, &hit, t); }
                if self.claims[rec].outcome.is_none() { self.claims[rec].outcome = Some((true, String::new())); }
                if self.claims[rec].forwarded_t.is_none() { self.claims[rec].forwarded_t = Some(t); }
                // The victim's replay: ITS armour stage on the approved inputs.
                if let Some((_, armour, _, hm, stab)) = self.claims[rec].pcell {
                    if hit.flags & damage::FLAG_COMPLEX != 0 {
                        let raw = game::deal(hit.damage_out, len(hit.velocity), armour, hit.cutting_power, hit.pain_rate);
                        self.claims[rec].mp = Some(game::health_loss(2.0 * hm * raw, stab, self.claims[rec].bone));
                    }
                }
                if std::env::var("SIM_DEBUG_CLASH").is_ok() {
                    let (vp, ap) = (self.clients[target].pid, self.clients[a].pid);
                    let log = self.core.lc.clash_log(vp);
                    let mine = self.core.lc.clash_log(ap);
                    eprintln!("FWD a{} v{} t={:.0} ats={} vclash={:?} aclash={:?} pred={:?}", a, target, t, hit.attacker_ts,
                        log.iter().filter(|e| e.1 == ap).map(|e| (e.0 - now, e.2, e.3 as i64 - hit.attacker_ts as i64, e.4)).collect::<Vec<_>>(),
                        mine.iter().filter(|e| e.1 == vp).map(|e| (e.0 - now, e.2 as i64 - hit.attacker_ts as i64, e.4)).collect::<Vec<_>>(),
                        None::<i64>);
                }
                // Trade engagements: both fighters are on their last legs (any
                // accepted hit kills; the principled cap keeps single blows small).
                if self.claims[rec].lethal && !self.cfg.health_model {
                    self.alive[target] = false;
                    self.dead_at[target] = t;
                    let pid = self.clients[target].pid;
                    self.core.note_death(pid, now);
                }
            }
        }
        // Early confirm (combat_glue): first time the claim is accepted,
        // forwarded or held.
        let apid = self.clients[a].pid;
        if Core::EARLY_CONFIRM && self.core.take_confirm(apid, hit.hit_id) {
            let rto = self.rto(a);
            for at in self.clients[a].down.send_reliable(t, rto) {
                self.push(at, Msg::CAck { to: a, hit_id: hit.hit_id, accepted: true, final_: false, rec });
            }
        }
        match v {
            Verdict::Hold | Verdict::Ignore => {}
            Verdict::Forward | Verdict::Reforward => {
                let rto = self.rto(target);
                self.pending_hits[rec] = { let mut h = hit.clone(); h.age_ms = self.pending_hits[rec].age_ms; h };
                for at in self.clients[target].down.send_reliable(t, rto) {
                    self.push(at, Msg::CDamage { to: target, attacker: a, hit_id: hit.hit_id, rec });
                }
                let _ = &mut hit;
            }
            Verdict::Ack { accepted, reason } => {
                if std::env::var("SIM_DEBUG_FAKE").is_ok() && reason == "parried"
                    && self.clients[target].cheat == Some(Cheat::FakeParry)
                {
                    let (vp, ap) = (self.clients[target].pid, self.clients[a].pid);
                    let tm = self.claims[rec].t_main;
                    eprintln!("FAKE-PARRIED a{} v{} t_main={:.0} ats={} now={:.0} fakes={:?} screen={:?}\n  vlog={:?}\n  alog={:?}",
                        a, target, tm, hit.attacker_ts, t,
                        self.fake_parries.iter().filter(|f| f.0 == target && f.1 == a && (f.2 - tm).abs() < 600.0).map(|f| (f.2 - tm, f.3)).collect::<Vec<_>>(),
                        self.screen_clashes.iter().filter(|s| ((s.0 == a && s.1 == target) || (s.0 == target && s.1 == a)) && (s.2 - tm).abs() < 600.0).map(|s| (s.0, s.2 - tm, s.3 - tm)).collect::<Vec<_>>(),
                        self.core.lc.clash_log(vp).iter().filter(|e| e.1 == ap).map(|e| (e.0 - t as i64, e.3 as i64 - hit.attacker_ts as i64, e.4)).collect::<Vec<_>>(),
                        self.core.lc.clash_log(ap).iter().filter(|e| e.1 == vp).map(|e| (e.0 - t as i64, e.2 as i64 - hit.attacker_ts as i64, e.4)).collect::<Vec<_>>());
                }
                let r = &mut self.claims[rec];
                if r.outcome.is_none() || !accepted {
                    // The server's final word (the client learns it on arrival;
                    // the record keeps the server's view for the metrics).
                    r.outcome = Some((accepted, reason));
                }
                let rto = self.rto(a);
                for at in self.clients[a].down.send_reliable(t, rto) {
                    self.push(at, Msg::CAck { to: a, hit_id: hit.hit_id, accepted, final_: true, rec });
                }
            }
        }
    }

    fn deliver(&mut self, m: Msg, t: f64) {
        let now = t as i64;
        match m {
            Msg::SPose { from, ts, t: ts_t } => {
                if self.cfg.record_on_tick {
                    match self.tick_records.iter_mut().find(|e| e.0 == from) {
                        Some(e) => if ts > e.1 { *e = (from, ts, ts_t); },
                        None => self.tick_records.push((from, ts, ts_t)),
                    }
                } else {
                    self.server_record(from, ts, ts_t, now);
                }
                if self.cfg.relay_on_tick {
                    self.tick_relays.push((from, ts, ts_t));
                } else {
                    self.relay_pose(from, ts, ts_t, t);
                }
            }
            Msg::CPose { to, from, ts, t: _ } => self.view_rx(to, from, ts, t),
            Msg::Pickup { client, rec } => self.sidecar_pickup(client, rec, t),
            Msg::PickupClash { client, other, my_ts, other_ts } => {
                // Two unreliable copies (combat_client.rs).
                for _ in 0..2 {
                    for at in self.clients[client].up.send(t) {
                        self.push(at, Msg::SClash { from: client, other, my_ts, other_ts });
                    }
                }
            }
            Msg::SClash { from, other, my_ts, other_ts } => {
                let (a, b) = (self.clients[from].pid, self.clients[other].pid);
                self.core.lc.record_clash(a, b, my_ts, other_ts, now);
            }
            Msg::SClaim { from, rec } => {
                let mut hit = self.pending_hits[rec].clone();
                if self.claims[rec].arrived_t.is_none() {
                    self.claims[rec].arrived_t = Some(t);
                    if std::env::var("SIM_DEBUG_RATE").is_ok() && self.clients[from].cheat == Some(Cheat::Speedhack) {
                        eprintln!("RATE t={:.0} suspect={:?}", t, self.core.lc.clock_rate_suspect(self.clients[from].pid));
                    }
                    if let crate::lagcomp::Eval::Accept(i) = self.core.lc.evaluate(self.clients[from].pid, &hit, now) {
                        self.claims[rec].server_speed = i.contact_speed;
                        self.claims[rec].server_peak = i.peak_speed.or(i.contact_speed);
                        if let Some(d) = self.claims[rec].rel_dbg.as_mut() { d.2 = i.rel_speed; }
                        self.claims[rec].parry_d = Some(i.parry_d);
                    }
                }
                let ctx = self.ctx(from, hit.target_peer_id);
                let v = self.core.on_claim(&ctx, &mut hit, now);
                self.glue(from, rec, hit, v, t);
            }
            Msg::CDamage { to, attacker, hit_id, rec } => {
                // Sidecar: always ack; apply once.
                let rto = self.rto(to);
                for at in self.clients[to].up.send_reliable(t, rto) {
                    self.push(at, Msg::SOwnerAck { from: to, attacker, hit_id });
                }
                if self.clients[to].seen.insert((attacker, hit_id)) {
                    self.clients[to].lua_inbox.push((rec, true, false));
                }
            }
            Msg::SOwnerAck { from, attacker, hit_id } => {
                if self.cfg.health_model { self.ledger_ack(from, attacker, hit_id, t); }
                let pid = self.clients[attacker].pid;
                if self.core.owner_ack(pid, hit_id, now) {
                    if let Some(&rec) = self.hitmap.get(&(attacker, hit_id)) {
                        let r = &mut self.claims[rec];
                        if r.outcome.is_none() { r.outcome = Some((true, String::new())); }
                        let rto = self.rto(attacker);
                        for at in self.clients[attacker].down.send_reliable(t, rto) {
                            self.push(at, Msg::CAck { to: attacker, hit_id, accepted: true, final_: true, rec });
                        }
                    }
                }
            }
            Msg::CAck { to, hit_id, accepted, final_, rec } => {
                if accepted && !final_ {
                    self.clients[to].lua_inbox.push((rec, false, true));
                } else if final_ {
                    self.clients[to].pending.remove(&hit_id);
                    if accepted { self.clients[to].lua_inbox.push((rec, false, true)); }
                }
            }
            Msg::CClash { to, other } => self.glints.push((to, other, t)),
            Msg::SVitals { from, seq, hp, dead, life } => self.vitals_server(from, seq, hp, dead, life, t),
            Msg::CVitals { to, from, seq, hp, life } => self.vitals_view(to, from, seq, hp, life, t),
            Msg::SDeath { from, life } => self.death_report(from, life, t),
        }
    }

    fn relay_pose(&mut self, from: usize, ts: u32, ts_t: f64, t: f64) {
        for to in 0..self.cfg.players {
            if to == from { continue; }
            for at in self.clients[to].down.send(t) {
                self.push(at, Msg::CPose { to, from, ts, t: ts_t });
            }
        }
    }

    fn server_tick(&mut self, t: f64) {
        let now = t as i64;
        for (from, ts, ts_t) in std::mem::take(&mut self.tick_records) {
            self.server_record(from, ts, ts_t, now);
        }
        for (from, ts, ts_t) in std::mem::take(&mut self.tick_relays) {
            self.relay_pose(from, ts, ts_t, t);
        }
        for (apid, hit, v) in self.core.flush(now) {
            let Some(a) = self.clients.iter().position(|c| c.pid == apid) else { continue };
            let Some(&rec) = self.hitmap.get(&(a, hit.hit_id)) else { continue };
            self.glue(a, rec, hit, v, t);
        }
        for (r, o) in self.core.judge_due(now) {
            let ri = self.clients.iter().position(|c| c.pid == r);
            let oi = self.clients.iter().position(|c| c.pid == o);
            if let (Some(ri), Some(oi)) = (ri, oi) {
                for (to, other) in [(ri, oi), (oi, ri)] {
                    let rto = self.rto(to);
                    for at in self.clients[to].down.send_reliable(t, rto) {
                        self.push(at, Msg::CClash { to, other });
                    }
                }
            }
        }
        // God-mode rule on the server tick (combat_glue::sweep_ledger). The
        // ledger's "round" is the victim's life here.
        if self.cfg.health_model {
            let now_i = self.health.inst(t);
            for c in 0..self.cfg.players {
                let (pid, life) = (self.clients[c].pid, self.health.life[c]);
                if self.health.ledger.sweep_one(pid, life, now_i).is_some() {
                    self.declare_death(c, t, health::DeathCause::LedgerGodMode);
                }
                self.note_godmode_flag(c, life, t);
            }
        }
        // Revive the dead after the trade window (the sim keeps fighting).
        for i in 0..self.cfg.players {
            if !self.cfg.health_model && !self.alive[i] && t - self.dead_at[i] > 1200.0 { self.alive[i] = true; }
        }
    }

    /// Connection-layer RTT samples (hsmp-net conn.rs) → lagcomp::note_rtt.
    fn rtt_feed(&mut self, t: f64) {
        for c in 0..self.cfg.players {
            // The connection layer samples every ack; a handful per feed here.
            // Acks are never held by a JitterInflate cheater (it delays only its
            // pose stream), so this is the transport's real jitter.
            // (Extra samples on clones of the links with their own random stream,
            // so the links' draws for the fight itself are unchanged.)
            let mut last = None;
            for k in 0..8u64 {
                let tk = t - 60.0 * k as f64;
                let (up, down) = if k == 0 {
                    (self.clients[c].up.send(tk), self.clients[c].down.send(tk))
                } else {
                    let (mut u, mut d) = (self.clients[c].up.clone(), self.clients[c].down.clone());
                    let salt = (t as u64).wrapping_mul(31).wrapping_add(k);
                    let (su, sd) = (u.rng().0, d.rng().0);
                    *u.rng() = super::Rng::new(su ^ salt);
                    *d.rng() = super::Rng::new(sd ^ salt.rotate_left(17));
                    (u.send(tk), d.send(tk))
                };
                if let (Some(u), Some(d)) = (up.first(), down.first()) {
                    // An AckHold cheater holds its acks like its stream.
                    let hold = if self.clients[c].cheat == Some(Cheat::AckHold) { self.clients[c].rng.range(0.0, 250.0) } else { 0.0 };
                    let rtt = (u - tk) + (d - tk) + 0.5 + hold;
                    let cl = &mut self.clients[c];
                    match cl.srtt {
                        None => { cl.srtt = Some(rtt); cl.rttvar = rtt / 2.0; }
                        Some(s) => {
                            cl.rttvar = 0.75 * cl.rttvar + 0.25 * (s - rtt).abs();
                            cl.srtt = Some(0.875 * s + 0.125 * rtt);
                        }
                    }
                    if k == 0 { last = Some(rtt); }
                }
            }
            let pid = self.clients[c].pid;
            if let Some(rtt) = last {
                self.core.lc.note_rtt(pid, rtt as f32, t as i64);
            }
            if self.clients[c].srtt.is_some() {
                self.core.lc.note_net_jitter(pid, self.clients[c].rttvar as f32, t as i64);
            }
        }
    }

    pub fn run(&mut self) {
        let end = self.cfg.dur_ms;
        let mut t = 0.0f64;
        let n = self.cfg.players;
        while t <= end + 2500.0 {
            // Network deliveries due by now.
            let lim = (t * 1000.0) as u64;
            while let Some((&k, _)) = self.q.iter().next() {
                if k.0 > lim { break; }
                let m = self.q.remove(&k).unwrap();
                self.deliver(m, k.0 as f64 / 1000.0);
            }
            if t <= end {
                for c in 0..n {
                    if t >= self.clients[c].next_frame {
                        let ft = self.clients[c].next_frame;
                        self.clients[c].next_frame += FRAME_MS;
                        self.client_frame(c, ft);
                    }
                    if t >= self.clients[c].next_lua {
                        let lt = self.clients[c].next_lua;
                        self.clients[c].next_lua += LUA_TICK_MS;
                        self.lua_tick(c, lt);
                    }
                }
            } else {
                for c in 0..n {
                    if t >= self.clients[c].next_lua {
                        let lt = self.clients[c].next_lua;
                        self.clients[c].next_lua += LUA_TICK_MS;
                        self.lua_tick(c, lt);
                    }
                }
            }
            for c in 0..n {
                if t >= self.clients[c].next_sidecar {
                    self.clients[c].next_sidecar += SIDECAR_TICK_MS;
                    self.sidecar_tick(c, t);
                }
            }
            if t >= self.next_tick {
                self.next_tick += self.cfg.server_tick_ms;
                self.server_tick(t);
            }
            if t >= self.next_rtt {
                self.next_rtt += 500.0;
                self.rtt_feed(t);
            }
            t += 1.0;
        }
    }
}

/// Distance from `loc` to `p`'s true body over the label ± 60 ms window
/// (victim clock) — what an honest judgement could have used.
fn honest_gap(w: &World, p: usize, label: f64, loc: V3) -> f32 {
    let mut best = f32::INFINITY;
    for k in -6..=6 {
        let tt = w.clients[p].true_of(label + 10.0 * k as f64 * w.clients[p].rate);
        let pose = w.stream_pose(p, tt);
        for cap in w.plan.fighters[p].true_caps(&pose) {
            best = best.min(point_seg(loc, cap.a, cap.b).0 - cap.r);
        }
    }
    best
}

impl World {
    /// True velocity (uu/s) of `p`'s body segment `seg` (midpoint) at true time t.
    fn body_velocity(&self, p: usize, t: f64, seg: usize) -> V3 {
        let f = &self.plan.fighters[p];
        let c1 = f.true_caps(&self.stream_pose(p, t + 8.0));
        let c0 = f.true_caps(&self.stream_pose(p, t - 8.0));
        let m = |c: &Capsule| lerp3(c.a, c.b, 0.5);
        mul(sub(m(&c1[seg]), m(&c0[seg])), 1000.0 / 16.0)
    }
}

fn w_class(f: &Fighter) -> damage::WeaponClass { f.weapon.class }
