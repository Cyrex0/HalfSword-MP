//! Server-side lag compensation v2 for owner-authoritative melee hits.
//!
//! Every player streams root / 16-bone pose / held-weapon samples stamped with
//! THEIR OWN sender clock (`os.clock()` ms). The server keeps ~1.2 s of each,
//! keyed by that clock, in rings with a SORTED insert (reordered UDP fills
//! holes instead of being dropped). Every sample also feeds a per-connection
//! CLOCK MAP: `offset = min(server_rx_ms − sender_ts)` over 2 s (the poseplay
//! estimator) and `jitter_p90` of the excess. Together with a per-connection
//! RTT (`note_rtt`, measured from S2CDamage→ack in combat.rs or fed by the
//! connection layer) the server PREDICTS what the attacker was displaying:
//!
//! ```text
//!   expected_view(victim) = attacker_ts + off_a − off_v     (victim clock)
//!                           − rtt_a                        (RTT/2 undoes the uplink
//!                                                           inside off_a, RTT/2 down)
//!                           − interp(interval_v + jitter_path_p90 + 6, 20..250)
//!                           − 1 frame
//! ```
//!
//! The claim's `victim_view_ts` is only a HINT, clamped into
//! `expected ± max(2·jitter_path_p90 + 1 frame, TOL_FLOOR_MS)`. A cheater
//! cannot pick the most favourable victim pose from a 600 ms buffet
//! (backtrack): the window is ~±30–90 ms around the server's own estimate.
//!
//! Fairness policy (deterministic; see `evaluate`, `parried`, `trade_ok`):
//! 1. Timestamps are REQUIRED whenever the victim has history (`NoData` only
//!    for victims that never streamed — legacy clients). The attacker's own ts
//!    must match the claim's arrival time through its clock map.
//! 2. Rewind: at most `max_rewind_ms` (300 ms; 400 on "high-latency allowed"
//!    servers) behind the victim's newest sample, net of a capped resend-age
//!    credit. Beyond the cap the DEFENDER wins.
//! 3. Geometry: the contact must lie within BODY_TOL of the victim's rewound
//!    body CAPSULES (per-bone radii), and
//!    - with a blade stream (`record_blade`, full-skeleton stream): the
//!      attacker's blade segment SWEPT over [t − 1 frame, t] must intersect a
//!      victim capsule within BODY_TOL and pass within SWEEP_TOL of the contact;
//!    - otherwise (current data): within reach of the attacker's rewound
//!      weapon actor / hands / limbs (legacy spheres).
//! 4. Blocks: the victim's rewound blade across the path of the contacting
//!    blade point cancels the hit outright. Clash REPORTS (either side) only
//!    cancel a hit once the server has VALIDATED them: rate-limited per
//!    (reporter, other), the other's time clamped like a hit's view time, and
//!    both rewound (swept) blades within the clash tolerance. A geometrically
//!    valid hit is held for the defender grace (combat.rs) only when a parry
//!    is plausible.
//! 5. Trades: an attacker whose death was reported at most TRADE_WINDOW_MS
//!    (attacker clock) before its own hit still lands that hit.
//!
//! Suspicious inputs (missing timestamps, far-off view hints, inconsistent
//! attacker clocks, clash spam, invalid clashes) bump `validate::cheat`.

#![allow(dead_code)]

use crate::posecodec::{self, PoseFrame, BONE_COUNT};
use crate::proto::{DamageEvent, PeerId};
use crate::validate::cheat::{self, Kind as CheatKind};
use crate::validate::rate::Bucket;
use std::collections::{HashMap, VecDeque};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

// ---- tunables ---------------------------------------------------------------

/// History kept per stream, in the owner's sender-clock ms.
pub const HISTORY_MS: u32 = 1200;
/// Default victim rewind cap.
pub const MAX_REWIND_MS: i64 = 300;
/// Rewind cap on servers that explicitly allow high latency.
pub const HIGH_LATENCY_REWIND_MS: i64 = 400;
/// The cap grows with the attacker's measured path (`ViewPrediction::honest_lag`)
/// plus the view tolerance and this slack, never past the ceiling: a 300 ms RTT player with a 150 ms
/// buffer still lands hits, a lag-switched view far behind does not.
pub const REWIND_SLACK_MS: i64 = 40;
pub const REWIND_CEILING_MS: i64 = 600;
/// A (clamped) view time may lead the newest sample by this much (the claim
/// can overtake the state stream; receivers extrapolate briefly).
pub const FUTURE_MS: i64 = 150;
/// The attacker's own ts may lead its newest recorded sample by this much
/// (hit report and state streams take different IPC paths).
pub const ATTACKER_LEAD_MS: i64 = 300;
/// One game frame (display + sidecar pickup), part of the prediction.
pub const FRAME_MS: i64 = 17;
/// Floor of the view-time tolerance (typically ±30–50 ms).
pub const TOL_FLOOR_MS: i64 = 30;
/// Ceiling of the view-time tolerance: a client inflating its own
/// stream jitter must not widen the window it picks the victim pose from.
pub const TOL_MAX_MS: i64 = 120;
/// Server-measured jitter bound. A stream's `jitter_p90` is
/// computed from the CLIENT's timestamps and arrival, so a client that holds
/// its pose packets or stamps them with noise controls it. The transport's
/// RTT variation (`rttvar`, from the connection layer's own acks) bounds what
/// the network explains: p90 one-way lateness ≈ 1.8·J for ±J uniform jitter
/// while rttvar ≈ 0.67·J (both directions), so K = 3 plus a floor, taken as
/// the max over the last NET_JITTER_WINDOW_MS (spikes).
pub const NET_JITTER_K: f32 = 3.0;
pub const NET_JITTER_FLOOR_MS: f32 = 15.0;
pub const NET_JITTER_WINDOW_MS: i64 = 10_000;
/// Most display delay a victim's own excess jitter can add that the rewind
/// cap then does NOT charge (`ViewPrediction::victim_excess`).
pub const VICTIM_EXCESS_MAX_MS: i64 = INTERP_MAX_MS as i64;
/// Most of a victim's own stream jitter (p90, ms) the rewind cap charges,
/// whatever its transport shows (a lag switch that also holds its
/// acks inflates the transport bound; honest p90 on a typical / Wi-Fi link
/// is 20–60 ms, spikes aside).
pub const VICTIM_JITTER_CHARGED_MS: f32 = 120.0;
/// A victim excess above this bumps its LagSwitch counter.
pub const LAG_SWITCH_FLAG_MS: i64 = 60;
/// RTT assumed before any measurement exists, and the extra tolerance then.
pub const DEFAULT_RTT_MS: i64 = 120;
pub const UNKNOWN_RTT_SLACK_MS: i64 = 100;
/// Client jitter-buffer model (poseplay.rs): interval + p90 + margin, clamped.
pub const INTERP_MARGIN_MS: f32 = 6.0;
pub const INTERP_MIN_MS: f32 = 20.0;
pub const INTERP_MAX_MS: f32 = 250.0;
/// Codec v2 buffer floor (poseplay DELAY_MIN_V2_MS).
pub const INTERP_MIN_V2_MS: f32 = 16.0;
/// A streamed blade shorter than this is degenerate (polearm / fist
/// tip scene at the grip) and rebuilt by `record_blade`.
pub const DEGENERATE_BLADE_UU: f32 = 10.0;
/// Clock-map window (same as poseplay CLOCK_WINDOW_MS).
pub const CLOCK_WINDOW_MS: i64 = 2000;
/// Lateness quantile and window of the receivers' buffer (poseplay JITTER_QUANTILE,
/// JITTER_WINDOW_MS).
pub const JITTER_QUANTILE: f32 = 0.95;
pub const JITTER_WINDOW_MS: i64 = 8000;
const JITTER_CAP: usize = 2048;
/// A hint further than tolerance + this from the prediction is suspicious.
pub const VIEW_OUTLIER_MS: i64 = 60;
/// The claim's mapped attacker time may lead its arrival by this much…
pub const ARRIVAL_LEAD_MS: i64 = 50;
/// …and lag it (beyond its own age + 2·jitter) by this much (IPC pickup).
pub const ARRIVAL_LATE_MS: i64 = 150;
/// Contact → victim capsule SURFACE: 15 uu + 10 uu for the receiving
/// stand-in's retargeted bone lengths and handle lag.
pub const BODY_TOL: f32 = 25.0;
/// Same for an attacker on the v1 stream (PhysicsHandle stand-ins, measured
/// error 6–78 uu; the sim's 20 uu-σ model needs ~60 for 97 %+).
pub const BODY_TOL_V1: f32 = 60.0;
/// A contact made by the attacker's body (fist, elbow, knee) must lie this
/// close to the attacker's OWN streamed capsules (exact geometry: no
/// retarget error, unlike the victim's body).
pub const UNARMED_TOL: f32 = 10.0;
/// Contact distance to the victim's root when no pose history exists.
pub const ROOT_TOL: f32 = 160.0;
/// Legacy reach spheres (no blade stream yet).
pub const WEAPON_REACH: f32 = 260.0;
pub const HAND_REACH: f32 = 280.0;
pub const LIMB_REACH: f32 = 60.0;
pub const ROOT_REACH: f32 = 450.0;
/// Swept blade: the contact must lie within this of the swept blade.
pub const SWEEP_TOL: f32 = 15.0;
/// Half-thickness of a blade/haft (added to capsule radii).
pub const BLADE_RADIUS: f32 = 3.0;
/// Sub-step length for sweeps (uu of endpoint travel).
const SWEEP_STEP_UU: f32 = 4.0;
const SWEEP_MAX_STEPS: usize = 64;
/// Geometric block: victim blade within this of the contacting blade's path.
pub const BLOCK_RADIUS: f32 = 8.0;
/// Nominal blade length from the gripping hand (fallback geometry).
pub const BLADE_LEN: f32 = 110.0;
/// Clash adjudication: both blades within this (blade stream: exact geometry)…
pub const CLASH_TOL: f32 = 10.0;
/// …or within this when blades are only estimated from hand + weapon actor.
pub const CLASH_TOL_FALLBACK: f32 = 40.0;
/// Clash reporter plausibility: the reporter's own streamed blade
/// tip may not move faster than this around its clash time (a fast strike's
/// tip tops out at MAX_STRIKE_SPEED; +33 % for stream noise). A blade snapped
/// onto the attacker's for one frame to fake a parry moves several times
/// faster.
pub const CLASH_BLADE_SPEED_MAX: f32 = 6000.0;
/// Clash report matches a hit if |clash time − hit.attacker_ts| ≤ this.
pub const CLASH_WINDOW_MS: i64 = 150;
/// …but at most this AFTER the hit: a blade that met the parry only after it
/// had already cut the body did not stop that cut. A victim's report names
/// the attacker ts its stand-in showed (one servo frame behind its label):
/// one frame of slack. The attacker's own report and its hit carry the same
/// clock: a clash in a LATER frame than the cut does not cancel it.
pub const CLASH_AFTER_MS: i64 = 17;
pub const CLASH_AFTER_OWN_MS: i64 = 5;
/// Only the VICTIM's clash report can cancel a hit. The attacker's
/// own screen already resolved the clash physically: a body contact after it
/// is a blow that got through, which solo play counts. A victim's clash does
/// not cancel either when the victim's own screen ALSO showed the attacker's
/// stand-in reaching its body (C2STouch) from TOUCH_BEFORE_CLASH_MS before
/// the clash (one frame of label noise) up to TOUCH_AFTER_MS after the hit:
/// a bind or a glance that landed. In one playtest 18 of 45 claims in
/// one 5 s window were cancelled, most by the attacker's own report at
/// dt 0..−50 ms (blade on the guard and on the body in the same frame).
pub const TOUCH_BEFORE_CLASH_MS: i64 = 17;
pub const TOUCH_AFTER_MS: i64 = 150;
/// Touch reports kept per victim (bounded; same TTL as clash reports).
pub const TOUCHES_MAX: usize = 64;
/// Clash reports per (reporter, other): burst and refill (client sends ≤ 10/s).
pub const CLASH_BUCKET_CAP: f32 = 5.0;
pub const CLASH_REFILL_PER_S: f32 = 10.0;
/// Per-reporter bound on the clash bucket map (one per other peer; the
/// server holds at most 64 players).
pub const CLASH_BUCKETS_MAX: usize = 64;
/// Clash-spam distrust: a reporter whose reports were judged geometrically
/// impossible DISTRUST_INVALID times within DISTRUST_WINDOW_MS has its
/// reports ignored for DISTRUST_MS (an honest client's reports are
/// screen contacts and validate; a spammer's near-misses can't all).
pub const DISTRUST_INVALID: usize = 4;
pub const DISTRUST_WINDOW_MS: i64 = 3000;
pub const DISTRUST_MS: i64 = 5000;
/// Delivery allowance on top of the rewind cap. The TOTAL rewind the server
/// performs (victim pose age at the moment the claim is judged = view lag +
/// claim delivery) may exceed `max_rewind` by this much: one resend
/// (combat_client RESEND_EVERY = 120 ms) or one transport RTO after a loss,
/// plus the Lua flush tick. Measured on the server clock — the client's
/// `age_ms` is NOT trusted for it (it could be forged).
pub const DELIVERY_CREDIT_MS: i64 = 130;
/// Former name (age-based credit), kept for callers.
pub const AGE_CREDIT_MS: u32 = DELIVERY_CREDIT_MS as u32;
/// Speedhack detection: the lower envelope of (server rx − sender ts) is kept
/// in RATE_BUCKET_MS buckets over RATE_BUCKETS buckets; its Theil–Sen slope
/// is the sender clock's rate error. Honest PC clocks drift < 0.05 %, so a
/// slope beyond RATE_MAX (5 %, = 50 ms per second) over ≥ RATE_MIN_SPAN_MS
/// with a straight-line fit (residual MAD ≤ RATE_FIT_MAD_MS: a route change
/// is a STEP, not a slope) is a client running its clock (game) at a
/// different speed.
pub const RATE_BUCKET_MS: i64 = 500;
pub const RATE_BUCKETS: usize = 16;
pub const RATE_MIN_SPAN_MS: i64 = 2000;
pub const RATE_MAX: f32 = 0.05;
pub const RATE_FIT_MAD_MS: f32 = 12.0;
/// A speedhack verdict holds this long after the last fit that showed it.
pub const RATE_STICKY_MS: i64 = 10_000;
/// Shoulder (upper arm bone) → hand distance an honest Willie can reach
/// (upper arm ≈ 30 + forearm ≈ 28 uu on the stock skeleton; +20 % for
/// ragdoll joint stretch). A streamed hand further out is a
/// reach-extension cheat.
pub const ARM_REACH_MAX: f32 = 70.0;
/// Pelvis → shoulder (upper arm bone) distance an honest Willie can have:
/// the pelvis → spine_01..05 → clavicle → upper-arm chain of
/// SK_Body_Man is 69.5 uu fully straight (posecodec_v2::REF_T; any bend only
/// shortens it) × the character scale k (HM 0.875–1.125; the codec allows
/// up to 1.5 = 104 uu), plus a little ragdoll joint stretch.
pub const SHOULDER_PELVIS_MAX: f32 = 110.0;
/// Defender grace (how long a hit with a plausible parry waits for the
/// victim's clash report): the victim sees the attacker's blade one display
/// delay after the claim left the attacker, reports on its next tick, and the
/// report travels up: rtt_v + interp + 2·jitter + IPC. Clamped to these.
pub const GRACE_MIN_MS: i64 = 120;
pub const GRACE_MAX_MS: i64 = 350;
pub const GRACE_DEFAULT_MS: i64 = 200;
/// Lua tick + sidecar pickup on the reporting side (clash / hit IPC path).
pub const IPC_MS: i64 = 40;
/// A clash report is judged eagerly (for the glint) once this old (both
/// blade streams have arrived by then on any link the 300 ms cap allows).
pub const CLASH_JUDGE_AFTER_MS: i64 = 120;
/// Early release of a held hit (`parry_window_clear`): the victim's blade
/// stayed this far from every attacker blade position around the hit while
/// the attack was on the victim's screen (clash tolerance 16 + 10 margin).
pub const PARRY_RELEASE_UU: f32 = 26.0;
/// Parry plausibility: victim blade this close to the attack path / contact.
pub const PARRY_PLAUSIBLE_UE: f32 = 70.0;
pub const PARRY_NEAR_CONTACT_UE: f32 = 160.0;
/// Weapon-actor / hand speed → blade-tip speed (lever arm) for fallback data.
pub const LEVER_FALLBACK: f32 = 2.0;
/// Largest shoulder-pivot lever (a polearm tip ~4.5 grip radii out).
pub const LEVER_MAX: f32 = 6.0;
/// Measured striking speeds are capped here (cm/s): a fast two-handed
/// swing's blade point; above it a stream is lying about its speed to
/// raise its damage cap.
pub const MAX_STRIKE_SPEED: f32 = crate::validate::damage::MAX_STRIKE_SPEED;
/// Approach-speed window before the hit, in frames: the blade meets the body
/// at the speed of its last free frames (2 frames = 33 ms at 60 Hz; longer
/// windows reach back to the faster middle of a decelerating thrust).
const SPEED_FRAMES: i64 = 2;
/// Frames before the contact `peak_speed` scans (≈ 50..100 ms of a swing; a
/// longer window lets a slowed blade's earlier speed bound an inflated claim).
const PEAK_FRAMES: i64 = 3;
/// A streamed pelvis may lie at most this far from the sender's
/// validated root (the root is the actor / capsule location; a ragdoll's
/// pelvis drifts from it), and move at most PELVIS_SPEED_MAX (+ slack)
/// between samples (a ragdoll launch; a sprint is ~700).
pub const POSE_ROOT_MAX: f32 = 250.0;
pub const PELVIS_SPEED_MAX: f32 = 2000.0;
pub const PELVIS_STEP_SLACK: f32 = 30.0;
/// Body capsules and the held blade's base reach this much further from the
/// root than the pelvis (head / feet / an outstretched arm).
pub const BODY_EXTENT: f32 = 150.0;
/// Pose / blade / capsule samples are recorded only while a
/// TIMESTAMPED root sample covers them; the root stream may lag the pose
/// stream by this much (a lost root packet, a ragdoll launch over the root
/// speed cap) before they are not.
pub const ROOT_COVER_LEAD_MS: i64 = 500;

/// Result of tying a streamed body sample to the sender's validated root.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Tie {
    Ok,
    /// Off the root (or a pelvis faster than a body): a teleport.
    Off,
    /// No timestamped root sample covers it: not judged, not recorded.
    Uncovered,
    /// A pose frame without a pelvis.
    NoPelvis,
}
/// Mutual-kill window on the attacker's own clock.
pub const TRADE_WINDOW_MS: i64 = 150;
/// How long (server time) a death keeps admitting trade hits.
pub const TRADE_SERVER_TTL: Duration = Duration::from_millis(1000);
const CLASH_TTL_MS: i64 = 3000;
/// A sender ts going back this far means the game restarted: drop history.
const RESET_BACKSTEP_MS: i64 = 2000;
const RING_CAP: usize = 256;
const CLOCK_CAP: usize = 512;
/// A stream silent this long restarts its clock map (stall / reconnect).
pub const CLOCK_GAP_RESET_MS: i64 = 1000;
/// RTT samples kept: the transport's srtt once a second plus damage→ack
/// samples, which arrive in bursts during a fight and must not push the
/// transport samples out of the window.
const RTT_SAMPLES: usize = 64;
const RTT_TTL_MS: i64 = 60_000;
/// The RTT is the minimum over this span ending at the newest sample: long
/// enough to hold several transport samples (the min filters the client
/// processing in damage→ack samples), short enough that a path that got
/// slower moves the prediction within seconds. The clock offsets use a 2 s
/// window; a longer RTT window put the two out of step after an RTT change.
const RTT_WINDOW_MS: i64 = 4_000;
pub const MAX_CAPSULES: usize = 24;

type V3 = [f32; 3];

/// Capsule axes over HERO_BONES with radii (torso ≈ 18, upper
/// limbs ≈ 9, lower limbs ≈ 7, head 12).
const SEGMENTS: &[(usize, usize, f32)] = {
    use posecodec::*;
    &[
        (PELVIS, SPINE_02, 18.0), (SPINE_02, SPINE_04, 18.0), (SPINE_04, HEAD, 12.0),
        (SPINE_04, UPPERARM_L, 9.0), (UPPERARM_L, LOWERARM_L, 9.0), (LOWERARM_L, HAND_L, 7.0),
        (SPINE_04, UPPERARM_R, 9.0), (UPPERARM_R, LOWERARM_R, 9.0), (LOWERARM_R, HAND_R, 7.0),
        (PELVIS, THIGH_L, 14.0), (THIGH_L, CALF_L, 9.0), (CALF_L, FOOT_L, 7.0),
        (PELVIS, THIGH_R, 14.0), (THIGH_R, CALF_R, 9.0), (CALF_R, FOOT_R, 7.0),
    ]
};
/// Radius of an isolated bone (its segment partner missing).
const LONE_BONE_R: f32 = 7.0;

// ---- server clock -------------------------------------------------------------

fn epoch() -> Instant {
    static E: OnceLock<Instant> = OnceLock::new();
    *E.get_or_init(Instant::now)
}
/// Server monotonic ms (process epoch).
pub fn now_ms() -> i64 { ms_at(Instant::now()) }
/// `Instant` → server ms (negative before the epoch).
pub fn ms_at(t: Instant) -> i64 {
    let e = epoch();
    match t.checked_duration_since(e) {
        Some(d) => d.as_millis() as i64,
        // floor() for instants before the epoch too (the epoch is set lazily, so
        // a caller can hold an Instant from before it): ms_at(t + n ms) − ms_at(t)
        // must be exactly n either side of the epoch.
        None => -(e.duration_since(t).as_nanos().div_ceil(1_000_000) as i64),
    }
}

// ---- samples ------------------------------------------------------------------

#[derive(Clone, Copy)]
struct Pose {
    p: [V3; BONE_COUNT],
    mask: u16,
}

/// One blade sample: grip-side base and tip, optional tip velocity (uu/s) as
/// streamed by the full-skeleton stream (docs/development/subsystems/replication.md).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Blade {
    pub base: V3,
    pub tip: V3,
    pub vel: Option<V3>,
}

/// Weapon pose plus bounded module envelopes. Modules can move independently
/// of the actor (e.g. the native flail's simulated Head and Link 1).
#[derive(Clone, Copy)]
struct WeaponShape {
    weapon_id: u8,
    p: V3,
    q: posecodec::v2::Quat,
    boxes: [posecodec::v2::WeaponBox; posecodec::v2::MAX_WEAPON_BOXES],
    n: usize,
}
fn registered_native_weapon_class(name:&str)->bool {
    let Some(leaf)=name.strip_suffix("_C") else {return false;};
    let extra=include_str!("native_melee_classes.json");
    // Eligible extras were individually extracted from the original PAK and
    // each is a direct ModularWeaponBP_C descendant (not traps/projectiles).
    if extra.lines().filter(|l|l.trim_start().starts_with("\"class\":")).any(|l|l.split('"').nth(3)==Some(name)) {return true;}
    include_str!("../../mods/HSMPLoadout/Scripts/hsmp_catalog.lua").lines()
        .filter(|l|l.trim_start().starts_with("W(\""))
        .filter_map(|l|l.split('"').nth(5)).any(|p|p.rsplit('/').next()==Some(leaf))
}
#[derive(Clone, Copy)]
struct BoneFrames { p: [V3; posecodec::v2::NB], q: [posecodec::v2::Quat; posecodec::v2::NB], scale: f32 }
impl Lerp for BoneFrames {
    fn lerp(a:&Self,b:&Self,f:f32)->Self {
        Self {p:std::array::from_fn(|i|lerp3(a.p[i],b.p[i],f)),
            q:std::array::from_fn(|i|hsmp_pose::poseplay::slerp(&a.q[i],&b.q[i],f)),scale:a.scale+(b.scale-a.scale)*f}
    }
}
impl WeaponShape {
    fn module(&self, component: u8) -> Option<&posecodec::v2::WeaponBox> {
        self.boxes[..self.n].iter().find(|b| b.component == component)
    }
    fn point(&self, p: V3) -> V3 { add(self.p, posecodec::v2::qrot(self.q, p)) }
    fn module_point(&self, component: u8, p: V3) -> Option<V3> {
        let b = self.module(component)?;
        if b.child_of!=0 {return None;}
        Some(self.point(add(b.p, posecodec::v2::qrot(b.q, p))))
    }
    fn nearest_module(&self, c: V3, component: u8) -> (f32, V3, u8) {
        use posecodec::v2::{qconj, qrot};
        let local = qrot(qconj(self.q), sub(c, self.p));
        let mut best = (f32::INFINITY, [0.0;3], 0);
        for b in &self.boxes[..self.n] {
            if b.child_of!=0 {continue;}
            if component != 0 && b.component != component { continue; }
            let p = qrot(qconj(b.q), sub(local, b.p));
            let near = std::array::from_fn(|i| p[i].clamp(-b.half[i], b.half[i]));
            let d = len(sub(p, near));
            // Retain the material point in this exact module's coordinates;
            // actor-relative coordinates lose motion at articulated joints.
            if d < best.0 { best = (d, near, b.component); }
        }
        best
    }
    #[cfg(test)]
    fn nearest(&self, c: V3, component: u8) -> (f32, V3) {
        let (d,p,_) = self.nearest_module(c,component);
        (d,p)
    }
}

#[derive(Clone, Copy)]
struct StrikerSet { extended: bool, shapes: [posecodec::v2::BodyStriker; posecodec::v2::MAX_BODY_STRIKERS], n: usize }
impl StrikerSet {
    fn empty(extended:bool)->Self {Self {extended,shapes:[posecodec::v2::BodyStriker::default();posecodec::v2::MAX_BODY_STRIKERS],n:0}}
    fn get(&self,part:u8,component:u8)->Option<posecodec::v2::BodyStriker> {self.shapes[..self.n].iter().find(|s|s.part==part && s.component==component).copied()}
}
fn striker_point(s:&posecodec::v2::BodyStriker,p:V3)->V3 {add(s.p,posecodec::v2::qrot(s.q,p))}
fn striker_nearest(s:&posecodec::v2::BodyStriker,c:V3)->(f32,V3) {
    let p=posecodec::v2::qrot(posecodec::v2::qconj(s.q),sub(c,s.p));
    let near=if s.kind==0 {let d=len(p); if d>s.half[0] {mul(p,s.half[0]/d)} else {p}}
        else {std::array::from_fn(|i|p[i].clamp(-s.half[i],s.half[i]))};
    (len(sub(p,near)),near)
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Capsule {
    pub a: V3,
    pub b: V3,
    pub r: f32,
}

#[derive(Clone, Copy)]
struct CapSet {
    c: [Capsule; MAX_CAPSULES],
    n: usize,
}

impl CapSet {
    fn iter(&self) -> impl Iterator<Item = &Capsule> { self.c[..self.n].iter() }
}

trait Lerp: Copy {
    fn lerp(a: &Self, b: &Self, f: f32) -> Self;
    /// Interpolation between samples `dt_ms` apart (default: linear).
    fn interp(a: &Self, b: &Self, f: f32, _dt_ms: f32) -> Self { Self::lerp(a, b, f) }
}
impl Lerp for StrikerSet {
    fn lerp(a:&Self,b:&Self,f:f32)->Self {
        let mut out=if f<0.5 {*a} else {*b};
        for s in &mut out.shapes[..out.n] {
            if let (Some(x),Some(y))=(a.get(s.part,s.component),b.get(s.part,s.component)) {
                if x.kind==y.kind {s.p=lerp3(x.p,y.p,f);s.q=hsmp_pose::poseplay::slerp(&x.q,&y.q,f);}
            }
        }
        out
    }
}
impl Lerp for WeaponShape {
    fn lerp(a: &Self, b: &Self, f: f32) -> Self {
        // A new weapon class can reuse the same native component ordinals.
        // Never synthesize a sweep between those unrelated components.
        if a.weapon_id != b.weapon_id || a.boxes.first().map(|b|b.class_hash)!=b.boxes.first().map(|b|b.class_hash) { return if f < 0.5 { *a } else { *b }; }
        // Dimensions remain a snapshot. Interpolate only the transforms of
        // matching native components, never between different array slots.
        let mut out = if f < 0.5 { *a } else { *b };
        out.p = lerp3(a.p, b.p, f);
        out.q = hsmp_pose::poseplay::slerp(&a.q, &b.q, f);
        for module in &mut out.boxes[..out.n] {
            if let (Some(x),Some(y)) = (a.module(module.component),b.module(module.component)) {
                if x.child_of!=y.child_of || x.native_scale.is_some()!=y.native_scale.is_some() {continue;}
                module.p = lerp3(x.p,y.p,f);
                module.q = hsmp_pose::poseplay::slerp(&x.q,&y.q,f);
            }
        }
        out
    }
}
impl Lerp for V3 {
    fn lerp(a: &V3, b: &V3, f: f32) -> V3 { lerp3(*a, *b, f) }
}
impl Lerp for Pose {
    fn lerp(a: &Pose, b: &Pose, f: f32) -> Pose {
        let mut out = *a;
        out.mask = a.mask & b.mask;
        for i in 0..BONE_COUNT { out.p[i] = lerp3(a.p[i], b.p[i], f); }
        out
    }
}
impl Lerp for Blade {
    fn lerp(a: &Blade, b: &Blade, f: f32) -> Blade {
        Blade {
            base: lerp3(a.base, b.base, f),
            tip: lerp3(a.tip, b.tip, f),
            vel: match (a.vel, b.vel) { (Some(x), Some(y)) => Some(lerp3(x, y, f)), _ => None },
        }
    }
    /// With streamed tip velocities on both samples the tip follows a cubic
    /// Hermite (the arc of a swing, not its chord: at 2500 uu/s and a 33 ms
    /// gap the chord cuts ~6 uu inside the arc). The base moves slowly and
    /// stays linear.
    fn interp(a: &Blade, b: &Blade, f: f32, dt_ms: f32) -> Blade {
        let mut o = Blade::lerp(a, b, f);
        if let (Some(va), Some(vb)) = (a.vel, b.vel) {
            let dt = dt_ms / 1000.0;
            if dt > 0.0 && dt <= 0.12 {
                let (f2, f3) = (f * f, f * f * f);
                let h00 = 2.0 * f3 - 3.0 * f2 + 1.0;
                let h10 = f3 - 2.0 * f2 + f;
                let h01 = -2.0 * f3 + 3.0 * f2;
                let h11 = f3 - f2;
                let tip = add(add(mul(a.tip, h00), mul(va, h10 * dt)), add(mul(b.tip, h01), mul(vb, h11 * dt)));
                // Bounded: never further than 25 % of the gap from the chord.
                let chord = lerp3(a.tip, b.tip, f);
                let gap = len(sub(b.tip, a.tip));
                let d = sub(tip, chord);
                let dl = len(d);
                o.tip = if dl <= 0.25 * gap + 1.0 { tip } else { add(chord, mul(d, (0.25 * gap + 1.0) / dl)) };
            }
        }
        o
    }
}
impl Lerp for CapSet {
    fn lerp(a: &CapSet, b: &CapSet, f: f32) -> CapSet {
        if a.n != b.n { return if f < 0.5 { *a } else { *b }; }
        let mut out = *a;
        for i in 0..a.n {
            out.c[i] = Capsule {
                a: lerp3(a.c[i].a, b.c[i].a, f),
                b: lerp3(a.c[i].b, b.c[i].b, f),
                r: a.c[i].r + (b.c[i].r - a.c[i].r) * f,
            };
        }
        out
    }
}

/// Sender-clock ring with sorted insert (tolerates reordering within the
/// history window; exact duplicates dropped; sender restart clears).
struct Ring<T> {
    q: VecDeque<(u32, T)>,
}

impl<T: Lerp> Ring<T> {
    fn new() -> Self { Ring { q: VecDeque::new() } }
    fn newest(&self) -> Option<u32> { self.q.back().map(|e| e.0) }
    fn oldest(&self) -> Option<u32> { self.q.front().map(|e| e.0) }
    fn is_empty(&self) -> bool { self.q.is_empty() }

    /// Returns false if the sample was dropped (duplicate / too old).
    fn push(&mut self, ts: u32, v: T) -> bool {
        if let Some(n) = self.newest() {
            if (ts as i64) + RESET_BACKSTEP_MS < n as i64 {
                self.q.clear(); // sender restarted (clock went back)
            } else if (n as i64) - (ts as i64) > HISTORY_MS as i64 {
                return false; // older than the window
            }
        }
        let i = self.q.partition_point(|e| e.0 < ts);
        if i < self.q.len() && self.q[i].0 == ts { return false; }
        self.q.insert(i, (ts, v));
        let n = self.newest().unwrap_or(ts);
        while self.q.len() > RING_CAP
            || self.q.front().map_or(false, |f| n.saturating_sub(f.0) > HISTORY_MS)
        {
            self.q.pop_front();
        }
        true
    }

    /// Interpolated value at sender time `t`. Clamps to the newest sample if
    /// `t` leads it by ≤ `lead_ms`; None if `t` is older than the history.
    fn sample(&self, t: u32, lead_ms: i64) -> Option<T> {
        let (n, o) = (self.newest()?, self.oldest()?);
        if t >= n {
            return if (t as i64 - n as i64) <= lead_ms { Some(self.q.back()?.1) } else { None };
        }
        if t < o { return None; }
        let i = self.q.partition_point(|e| e.0 <= t); // first strictly after t
        let (a, b) = (&self.q[i - 1], &self.q[i]);
        let f = (t - a.0) as f32 / (b.0 - a.0).max(1) as f32;
        Some(T::interp(&a.1, &b.1, f, (b.0 - a.0) as f32))
    }

    /// Like `sample` at a fractional sender time (sweeps sub-step a frame).
    fn sample_f(&self, t: f64, lead_ms: i64) -> Option<T> {
        let (n, o) = (self.newest()? as f64, self.oldest()? as f64);
        if t >= n {
            return if t - n <= lead_ms as f64 { Some(self.q.back()?.1) } else { None };
        }
        if t < o { return None; }
        let i = self.q.partition_point(|e| (e.0 as f64) <= t);
        let (a, b) = (&self.q[i - 1], &self.q[i]);
        let f = ((t - a.0 as f64) / (b.0 - a.0).max(1) as f64) as f32;
        Some(T::interp(&a.1, &b.1, f, (b.0 - a.0) as f32))
    }

    /// Median spacing of the newest samples (ms), if there are any.
    fn interval(&self) -> Option<f32> {
        let k = self.q.len().min(17);
        if k < 3 { return None; }
        let s = self.q.len() - k;
        let mut d: Vec<u32> = (s + 1..self.q.len()).map(|i| self.q[i].0 - self.q[i - 1].0).collect();
        d.sort_unstable();
        Some(d[d.len() / 2] as f32)
    }
}

/// Per-connection clock map: sender clock → server clock.
#[derive(Default)]
struct Clock {
    /// (server rx ms, rx − sender ts)
    s: VecDeque<(i64, i64)>,
    last_ts: Option<u32>,
    /// (server ms, rtt ms)
    rtt: VecDeque<(i64, f32)>,
    /// Clock-rate buckets: (bucket index = rx / RATE_BUCKET_MS, min(rx − ts)).
    rate: VecDeque<(i64, i64)>,
    /// Sticky speedhack verdict: (until server ms, rate error). A fit judged
    /// at each new bucket; once over RATE_MAX it holds RATE_STICKY_MS.
    flagged: Option<(i64, f32)>,
    /// (server ms, transport rttvar ms) from the connection layer.
    net_jit: VecDeque<(i64, f32)>,
    /// `jitter_p90` cache, cleared by every new sample (the relay asks for it
    /// once per relayed frame and viewer).
    j90: std::cell::Cell<Option<f32>>,
    /// (server rx ms, lateness above the offset at that moment) over
    /// JITTER_WINDOW_MS: the receivers size their buffers on this (poseplay).
    late: VecDeque<(i64, i64)>,
}

impl Clock {
    fn note(&mut self, rx: i64, ts: u32) {
        self.j90.set(None);
        // A stream silent this long (a stall, a reconnect: the session
        // resume keeps the peer id, so this state survives it) comes back on
        // a possibly different path: re-learn the offset and jitter from the
        // new samples instead of mixing both paths into one "jitter" (which
        // skewed the view prediction and flagged a lag switch for up to 2 s).
        // The rate buckets stay (a path change is a step, not a slope).
        if self.s.back().map_or(false, |b| rx - b.0 > CLOCK_GAP_RESET_MS) {
            self.s.clear();
            self.late.clear();
        }
        if let Some(l) = self.last_ts {
            if (ts as i64) + RESET_BACKSTEP_MS < l as i64 {
                self.s.clear();
                self.late.clear();
                self.rate.clear();
                self.flagged = None;
                self.last_ts = None;
            }
        }
        if self.last_ts.map_or(true, |l| ts > l) { self.last_ts = Some(ts); }
        let d = rx - ts as i64;
        self.s.push_back((rx, d));
        while self.s.len() > CLOCK_CAP || self.s.front().map_or(false, |f| rx - f.0 > CLOCK_WINDOW_MS) {
            self.s.pop_front();
        }
        let off = self.s.iter().map(|e| e.1).min().unwrap_or(d);
        self.late.push_back((rx, d - off));
        while self.late.len() > JITTER_CAP || self.late.front().map_or(false, |f| rx - f.0 > JITTER_WINDOW_MS) {
            self.late.pop_front();
        }
        let b = rx.div_euclid(RATE_BUCKET_MS);
        match self.rate.back_mut() {
            Some(last) if last.0 == b => last.1 = last.1.min(d),
            Some(last) if last.0 > b => {
                // Out-of-order server time (tests feed arbitrary rx): fold in.
                if let Some(e) = self.rate.iter_mut().find(|e| e.0 == b) { e.1 = e.1.min(d); }
            }
            _ => {
                self.rate.push_back((b, d));
                while self.rate.len() > RATE_BUCKETS { self.rate.pop_front(); }
                if let Some(e) = self.rate_error().filter(|e| e.abs() > RATE_MAX) {
                    self.flagged = Some((rx + RATE_STICKY_MS, e));
                }
            }
        }
        while self.rate.len() > RATE_BUCKETS { self.rate.pop_front(); }
    }

    /// Sender clock rate error (dimensionless: +0.5 = the sender's clock runs
    /// 1.5× fast) from the lower envelope, if enough span and a line fits.
    fn rate_error(&self) -> Option<f32> {
        let n = self.rate.len();
        if n < 5 { return None; }
        let span = (self.rate[n - 1].0 - self.rate[0].0) * RATE_BUCKET_MS;
        if span < RATE_MIN_SPAN_MS { return None; }
        let pts: Vec<(f32, f32)> = self.rate.iter()
            .map(|e| ((e.0 * RATE_BUCKET_MS) as f32, e.1 as f32)).collect();
        let mut slopes = Vec::with_capacity(n * (n - 1) / 2);
        for i in 0..n {
            for j in i + 1..n {
                let dx = pts[j].0 - pts[i].0;
                if dx > 0.0 { slopes.push((pts[j].1 - pts[i].1) / dx); }
            }
        }
        if slopes.is_empty() { return None; }
        slopes.sort_by(|a, b| a.total_cmp(b));
        let m = slopes[slopes.len() / 2];
        let mut res: Vec<f32> = pts.iter().map(|p| p.1 - m * p.0).collect();
        res.sort_by(|a, b| a.total_cmp(b));
        let med = res[res.len() / 2];
        let mut dev: Vec<f32> = res.iter().map(|r| (r - med).abs()).collect();
        dev.sort_by(|a, b| a.total_cmp(b));
        // A fast clock spreads samples within a bucket: allow 10 % of a
        // bucket of slope on top of the fixed residual.
        if dev[dev.len() / 2] > RATE_FIT_MAD_MS + 0.1 * m.abs() * RATE_BUCKET_MS as f32 { return None; }
        // rx − ts falls by (rate − 1) per ms when ts runs fast: error = −slope.
        Some(-m)
    }

    fn rate_suspect(&self) -> Option<f32> {
        let last = self.s.back().map(|e| e.0)?;
        match self.flagged {
            Some((until, e)) if last <= until => Some(e),
            _ => None,
        }
    }
    /// min(rx − ts): sender ts + offset = earliest server arrival.
    fn offset(&self) -> Option<i64> { self.s.iter().map(|e| e.1).min() }
    /// p90 of arrival lateness above the minimum (uplink jitter), ms.
    fn jitter_p90(&self) -> f32 {
        if let Some(j) = self.j90.get() { return j; }
        let j = self.jitter_p90_uncached();
        self.j90.set(Some(j));
        j
    }
    /// (The name is historical: like poseplay's buffer this is the JITTER_QUANTILE
    /// (p95) of the lateness over JITTER_WINDOW_MS.)
    fn jitter_p90_uncached(&self) -> f32 {
        if self.offset().is_none() { return 0.0; }
        let mut v: Vec<i64> = self.late.iter().map(|e| e.1.max(0)).collect();
        if v.is_empty() { return 0.0; }
        let i = ((v.len() as f32 * JITTER_QUANTILE).ceil() as usize).clamp(1, v.len()) - 1;
        let (_, q, _) = v.select_nth_unstable(i);
        *q as f32
    }
    fn note_rtt(&mut self, at: i64, ms: f32) {
        if !(ms.is_finite() && (0.0..5000.0).contains(&ms)) { return; }
        self.rtt.push_back((at, ms));
        while self.rtt.len() > RTT_SAMPLES { self.rtt.pop_front(); }
    }
    fn note_net_jitter(&mut self, at: i64, rttvar_ms: f32) {
        if !(rttvar_ms.is_finite() && (0.0..5000.0).contains(&rttvar_ms)) { return; }
        self.net_jit.push_back((at, rttvar_ms));
        while self.net_jit.len() > 64 || self.net_jit.front().map_or(false, |f| at - f.0 > NET_JITTER_WINDOW_MS) {
            self.net_jit.pop_front();
        }
    }
    /// Most stream jitter (p90, ms) the measured transport explains, if the
    /// connection layer reported any recently.
    fn net_bound(&self, now: i64) -> Option<f32> {
        self.net_jit.iter().filter(|e| now - e.0 <= NET_JITTER_WINDOW_MS)
            .map(|e| NET_JITTER_K * e.1 + NET_JITTER_FLOOR_MS).reduce(f32::max)
    }
    /// `jitter_p90` capped by what the transport explains.
    fn jitter_bounded(&self, now: i64) -> f32 {
        let j = self.jitter_p90();
        match self.net_bound(now) { Some(b) => j.min(b), None => j }
    }
    /// Minimum recent RTT (the display path runs at minimum delay + jitter
    /// buffer; ack timing adds client processing, which the min filters out).
    fn rtt(&self, now: i64) -> Option<f32> {
        let newest = self.rtt.iter().map(|e| e.0).max()?;
        self.rtt.iter().filter(|e| now - e.0 <= RTT_TTL_MS && newest - e.0 <= RTT_WINDOW_MS).map(|e| e.1).reduce(f32::min)
    }
}

struct Clash {
    at: i64,
    other: PeerId,
    my_ts: u32,
    other_ts: u32,
    /// Server verdict once judged (geometry needs both streams to arrive).
    valid: Option<bool>,
    /// A validated clash was already reported (glint) by `judge_due`.
    notified: bool,
}

struct PeerHist {
    context: Option<posecodec::v2::Context>,
    root: Ring<V3>,
    pose: Ring<Pose>,
    bone_frames: Ring<BoneFrames>,
    /// Accepted complete native frames, keyed by original frame-start stamp.
    /// Derivatives and physics step are required for exact delivered playback.
    native_frames: std::collections::BTreeMap<u32,hsmp_pose::poseplay::Frame>,
    weapon: Ring<V3>,
    blade: Ring<Blade>,
    offhand: Ring<Blade>,
    shapes: Ring<WeaponShape>,
    offhand_shapes: Ring<WeaponShape>,
    strikers: Ring<StrikerSet>,
    caps: Ring<CapSet>,
    clock: Clock,
    /// (server time, newest own ts) of the last reported death.
    death: Option<(i64, u32)>,
    clashes: VecDeque<Clash>,
    /// Touch reports of this peer as a victim: (server ms, the
    /// attacker, the attacker ts it displayed).
    touches: VecDeque<(i64, PeerId, u32)>,
    /// Clash-report token buckets per other peer.
    clash_bucket: HashMap<PeerId, Bucket>,
    /// Server ms of this peer's recent invalid clash reports.
    clash_invalid: VecDeque<i64>,
    /// Clash reports ignored until this server ms (spam distrust).
    distrust_until: i64,
    /// Sender ts of blade samples whose tip moved faster than
    /// CLASH_BLADE_SPEED_MAX from the previous sample (teleports).
    blade_jumps: VecDeque<u32>,
    /// Codec of the last skeletal frame (true = v2), if known.
    pose_v2: Option<bool>,
}

impl PeerHist {
    fn striker_contact(&self,c:V3,t0:f64,t1:f64,part:u8,component:u8)->Option<(f32,V3,f64)> {
        self.strikers.sample_f(t0,ATTACKER_LEAD_MS)?;
        self.strikers.sample_f(t1,ATTACKER_LEAD_MS)?;
        let n=(((t1-t0)/2.0).ceil() as usize).clamp(1,SWEEP_MAX_STEPS);
        let mut best=(f32::INFINITY,[0.0;3],t0);
        for i in 0..=n {
            let t=t0+(t1-t0)*i as f64/n as f64;
            if let Some(s)=self.strikers.sample_f(t,ATTACKER_LEAD_MS).and_then(|s|s.get(part,component)) {
                let (d,p)=striker_nearest(&s,c);
                if d<best.0 {best=(d,p,t);}
            }
        }
        best.0.is_finite().then_some(best)
    }
    fn striker_velocity(&self,part:u8,component:u8,p:V3,t:f64)->Option<V3> {
        let a=self.strikers.sample_f(t-1.0,ATTACKER_LEAD_MS)?.get(part,component)?;
        let b=self.strikers.sample_f(t+1.0,ATTACKER_LEAD_MS)?.get(part,component)?;
        Some(mul(sub(striker_point(&b,p),striker_point(&a,p)),500.0))
    }
    fn striker_peak(&self,part:u8,component:u8,p:V3,at:u32)->Option<f32> {
        let fr=self.frame(); let mut peak:Option<f32>=None;
        for k in 0..PEAK_FRAMES {
            let a=self.strikers.sample(ts_sub(at,(k+1)*fr),0).and_then(|s|s.get(part,component));
            let b=self.strikers.sample(ts_sub(at,k*fr),ATTACKER_LEAD_MS).and_then(|s|s.get(part,component));
            if let (Some(a),Some(b))=(a,b) {
                let v=len(sub(striker_point(&b,p),striker_point(&a,p)))*1000.0/fr as f32;
                peak=Some(peak.map_or(v,|old|old.max(v)));
            }
        }
        peak.map(|v|v.min(MAX_STRIKE_SPEED))
    }
    fn new() -> Self {
        PeerHist {
            context: None,
            root: Ring::new(), pose: Ring::new(), bone_frames: Ring::new(), native_frames:std::collections::BTreeMap::new(), weapon: Ring::new(), blade: Ring::new(), offhand: Ring::new(),
            shapes: Ring::new(), offhand_shapes: Ring::new(), strikers: Ring::new(),
            caps: Ring::new(), clock: Clock::default(), death: None, clashes: VecDeque::new(), touches: VecDeque::new(),
            clash_bucket: HashMap::new(), clash_invalid: VecDeque::new(), distrust_until: i64::MIN,
            blade_jumps: VecDeque::new(),
            pose_v2: None,
        }
    }
    /// Newest body sample (root / pose / capsules).
    fn newest(&self) -> Option<u32> {
        [self.root.newest(), self.pose.newest(), self.caps.newest()].into_iter().flatten().max()
    }
    fn newest_any(&self) -> Option<u32> {
        [self.newest(), self.weapon.newest(), self.blade.newest(), self.offhand.newest()].into_iter().flatten().max()
    }
    fn has_history(&self) -> bool { self.newest_any().is_some() }
    /// The viewed peer streams codec v2 (by its last frame; without codec
    /// information a blade stream means v2).
    fn is_v2(&self) -> bool { self.pose_v2.unwrap_or_else(|| self.blade.newest().is_some()) }
    fn interval(&self) -> f32 {
        self.pose.interval().or(self.root.interval()).or(self.blade.interval())
            .unwrap_or(33.0).clamp(4.0, 100.0)
    }
    /// One frame of the blade/weapon stream for sweeps (ms).
    fn frame(&self) -> i64 {
        let iv = self.blade.interval().or(self.weapon.interval()).or(self.pose.interval()).unwrap_or(33.0);
        (iv as i64).clamp(8, 50)
    }

    /// Body capsules at `t`: streamed capsules if they cover t, else the pose
    /// with the per-bone radius table.
    fn capsules(&self, t: u32, lead: i64) -> Option<CapSet> {
        if let Some(c) = self.caps.sample(t, lead) { return Some(c); }
        let p = self.pose.sample(t, lead)?;
        Some(pose_capsules(&p))
    }

    /// The streamed blade swept over sender times [t0, t1], sub-stepped so no
    /// endpoint moves more than SWEEP_STEP_UU between steps (each step is
    /// interpolated from the ring: Hermite on the tip when velocities are
    /// streamed). None without blade stream coverage.
    fn blade_sweep(&self, t0: f64, t1: f64, lead: i64) -> Option<Vec<Blade>> {
        blade_sweep(&self.blade, t0, t1, lead)
    }
}

/// A contact chooses one independently validated weapon history. Its geometry
/// and speed must come from that same weapon, including an offhand strike.
struct BladeView<'a> { hist: &'a PeerHist, blade: &'a Ring<Blade> }
impl std::ops::Deref for BladeView<'_> {
    type Target = PeerHist;
    fn deref(&self) -> &PeerHist { self.hist }
}
impl BladeView<'_> {
    fn offhand(&self) -> bool { std::ptr::eq(self.blade, &self.hist.offhand) }
    fn shapes(&self) -> &Ring<WeaponShape> {
        if self.offhand() { &self.hist.offhand_shapes } else { &self.hist.shapes }
    }
    fn shape_contact(&self, c: V3, t0: f64, t1: f64, component: u8) -> Option<(f32, V3, f64, u8, u8)> {
        let ring = self.shapes();
        let first=ring.sample_f(t0, ATTACKER_LEAD_MS)?;
        let weapon_id=first.weapon_id;
        let class_hash=first.boxes[0].class_hash;
        let last=ring.sample_f(t1,ATTACKER_LEAD_MS)?;
        if last.weapon_id != weapon_id || last.boxes[0].class_hash!=class_hash { return None; }
        let n = (((t1-t0)/2.0).ceil() as usize).clamp(1, SWEEP_MAX_STEPS);
        let mut best = (f32::INFINITY, [0.0;3], t0, 0, weapon_id);
        for k in 0..=n {
            let t = t0 + (t1-t0)*k as f64/n as f64;
            if let Some(s) = ring.sample_f(t, ATTACKER_LEAD_MS) {
                if s.weapon_id != weapon_id || s.boxes[0].class_hash!=class_hash { return None; }
                let (d,p,id) = s.nearest_module(c,component);
                if d < best.0 { best = (d,p,t,id,weapon_id); }
            }
        }
        best.0.is_finite().then_some(best)
    }
    fn shape_velocity(&self, weapon_id: u8, component: u8, p: V3, t: f64) -> Option<V3> {
        let a = self.shapes().sample_f(t-1.0, ATTACKER_LEAD_MS)?;
        let b = self.shapes().sample_f(t+1.0, ATTACKER_LEAD_MS)?;
        if a.weapon_id != weapon_id || b.weapon_id != weapon_id || a.boxes[0].class_hash!=b.boxes[0].class_hash { return None; }
        Some(mul(sub(b.module_point(component,p)?, a.module_point(component,p)?), 500.0))
    }
    fn shape_peak(&self, weapon_id: u8, component: u8, p: V3, at: u32) -> Option<f32> {
        let fr = self.frame();
        let mut peak: Option<f32> = None;
        for k in 0..PEAK_FRAMES {
            if let (Some(a), Some(b)) = (self.shapes().sample(ts_sub(at,(k+1)*fr),0), self.shapes().sample(ts_sub(at,k*fr),ATTACKER_LEAD_MS)) {
                if a.weapon_id != weapon_id || b.weapon_id != weapon_id || a.boxes[0].class_hash!=b.boxes[0].class_hash { continue; }
                let (Some(a),Some(b)) = (a.module_point(component,p),b.module_point(component,p)) else { continue };
                let v = len(sub(b,a))*1000.0/fr as f32;
                peak = Some(peak.map_or(v, |x| x.max(v)));
            }
        }
        peak.map(|v| v.min(MAX_STRIKE_SPEED))
    }
    fn blade_at(&self, t: u32, lead: i64) -> Option<(Blade, bool)> {
        if let Some(b) = self.blade.sample(t, lead) { return Some((b, true)); }
        if std::ptr::eq(self.blade, &self.hist.blade) { self.hist.blade_at(t, lead) } else { None }
    }
    fn blade_sweep(&self, t0: f64, t1: f64, lead: i64) -> Option<Vec<Blade>> {
        blade_sweep(self.blade, t0, t1, lead)
    }
}
fn blade_sweep(blade: &Ring<Blade>, t0: f64, t1: f64, lead: i64) -> Option<Vec<Blade>> {
        let b0 = blade.sample_f(t0, lead)?;
        let b1 = blade.sample_f(t1, lead)?;
        // Step count from the travel between the samples bracketing the span.
        let mut n = sweep_steps(&b0, &b1);
        if let Some(m) = blade.sample_f((t0 + t1) / 2.0, lead) {
            n = n.max(sweep_steps(&b0, &m) * 2).min(SWEEP_MAX_STEPS);
        }
        let mut out = Vec::with_capacity(n + 1);
        for k in 0..=n {
            let t = t0 + (t1 - t0) * k as f64 / n as f64;
            out.push(blade.sample_f(t, lead).unwrap_or(if k == 0 { b0 } else { b1 }));
        }
        Some(out)
}
impl PeerHist {
    /// Blade at `t`: streamed, else estimated from the weapon actor and the
    /// nearer gripping hand (nominal BLADE_LEN). bool = exact geometry.
    fn blade_at(&self, t: u32, lead: i64) -> Option<(Blade, bool)> {
        if let Some(b) = self.blade.sample(t, lead) { return Some((b, true)); }
        let w = self.weapon.sample(t, lead)?;
        let grip = self.pose.sample(t, lead).and_then(|p| nearer_hand(&p, w));
        Some((estimate_blade(grip, w), false))
    }
}

/// Receiver jitter buffer for `viewed`'s stream (poseplay.rs). Codec v2:
/// clamp(jitter_p90 + 6, 16, 250) — velocities fill a frame gap — but never
/// less than one RELAY interval + 6 when the relay decimates the pair
/// (at 7.5 Hz the receiver would otherwise extrapolate 133 ms); v1:
/// interval + p90 + 6, ≥ 20 (the interval the receiver gets, i.e. the relay
/// interval when known). The codec is the one of the viewed peer's LAST
/// skeletal frame (unarmed v2 players stream no blade and would otherwise
/// get the v1 formula, a 17–21 ms bias); without codec information, a blade
/// stream means v2. The display adds one physics frame (FRAME_MS
/// in the prediction).
fn display_delay(viewed: &PeerHist, jpath: f32, relay_iv: Option<f32>) -> f32 {
    if viewed.is_v2() {
        // poseplay (sidecar): j95 + margin + the part of the
        // received interval that extrapolation cannot cover (> one frame).
        let d = jpath + INTERP_MARGIN_MS;
        let d = match relay_iv { Some(iv) => d + (iv - FRAME_MS as f32).max(0.0), None => d };
        d.clamp(INTERP_MIN_V2_MS, INTERP_MAX_MS)
    } else {
        (relay_iv.unwrap_or_else(|| viewed.interval()) + jpath + INTERP_MARGIN_MS).clamp(INTERP_MIN_MS, INTERP_MAX_MS)
    }
}

// ---- relay view model ----------------------------------------------------------

/// Relayed frame stamps kept per (viewer, src): more than HISTORY_MS at 120 Hz.
const RELAY_LOG_CAP: usize = 160;
/// A pair's relay data older than this (server ms) is not used (the relay
/// stopped sending: the viewer's buffer has moved on).
pub const RELAY_MODEL_TTL_MS: i64 = 1000;
/// poseplay.rs mirror: starting buffer delay, grow / shrink gains per frame,
/// playback-clock slew limit and its time constant, hard re-sync.
const PLAY_DELAY_START_MS: f32 = 60.0;
const PLAY_DELAY_RISE: f32 = 0.3;
const PLAY_DELAY_FALL: f32 = 0.02;
const PLAY_SLEW_MAX: f32 = 0.10;
const PLAY_SLEW_TIME_MS: f32 = 200.0;
const PLAY_RESYNC_MS: f32 = 250.0;
/// poseplay EXTRAP_FULL_MS: past its newest frame the receiver extrapolates
/// at full velocity this long, then eases out and holds.
const PLAY_EXTRAP_MS: u32 = 34;

/// The server's mirror of one receiver's jitter buffer for one sender
/// (poseplay `Playback::update_delay` + its slewed playback clock):
/// `delay` rises fast and falls slowly toward the buffer target at every
/// frame the receiver gets; the delay actually displayed (`shown`) follows
/// it at most ±10 % of real time.
#[derive(Clone, Copy, Debug)]
struct DelayModel {
    delay: f32,
    shown: f32,
    at: i64,
}

impl DelayModel {
    fn new(now: i64) -> Self { DelayModel { delay: PLAY_DELAY_START_MS, shown: PLAY_DELAY_START_MS, at: now } }

    /// The displayed delay after `dt` ms of playback-clock slew toward `delay`.
    fn slewed(shown: f32, delay: f32, dt: f32) -> f32 {
        let mut s = shown;
        let mut left = dt.clamp(0.0, 1000.0);
        while left > 0.0 {
            let step = left.min(10.0);
            let gap = s - delay;
            if gap.abs() > PLAY_RESYNC_MS { return delay; }
            s -= step * (gap / PLAY_SLEW_TIME_MS).clamp(-PLAY_SLEW_MAX, PLAY_SLEW_MAX);
            left -= step;
        }
        s
    }

    /// A frame reached the receiver at `now` with buffer target `target`.
    fn on_frame(&mut self, now: i64, target: f32) {
        self.shown = Self::slewed(self.shown, self.delay, (now - self.at) as f32);
        self.at = now.max(self.at);
        let a = if target > self.delay { PLAY_DELAY_RISE } else { PLAY_DELAY_FALL };
        self.delay += (target - self.delay) * a;
    }

    /// The delay displayed at `now`.
    fn shown_at(&self, now: i64) -> f32 { Self::slewed(self.shown, self.delay, (now - self.at) as f32) }
}

/// What the relay sent one viewer of one sender.
struct RelayLog {
    /// Sender ts of the relayed frames, sorted.
    ts: VecDeque<u32>,
    /// Physical sample time of each relayed frame, retained even if its
    /// native geometry ages out. Sender steps can reorder these times.
    sample_ts: HashMap<u32, f64>,
    /// The pair's relay interval (ms).
    interval: f32,
    model: DelayModel,
}

/// Direction of a degenerate streamed blade: up the haft (the farther hand
/// toward the nearer, both gripping) or along the gripping forearm.
fn degenerate_dir(p: &Pose, grip: V3) -> Option<V3> {
    use posecodec::{HAND_L, HAND_R, LOWERARM_L, LOWERARM_R};
    let has = |i: usize| p.mask & (1 << i) != 0;
    let (near, far, fore) = if !has(HAND_L) || (has(HAND_R) && len(sub(p.p[HAND_R], grip)) <= len(sub(p.p[HAND_L], grip))) {
        (HAND_R, HAND_L, LOWERARM_R)
    } else {
        (HAND_L, HAND_R, LOWERARM_L)
    };
    if !has(near) { return None; }
    let d = if has(far) && len(sub(p.p[near], p.p[far])) > 8.0 && len(sub(p.p[near], p.p[far])) < 90.0 {
        sub(p.p[near], p.p[far])
    } else if has(fore) {
        sub(p.p[near], p.p[fore])
    } else {
        return None;
    };
    let l = len(d);
    if l < 1.0 { None } else { Some(mul(d, 1.0 / l)) }
}

fn nearer_hand(p: &Pose, w: V3) -> Option<V3> {
    [posecodec::HAND_R, posecodec::HAND_L].iter()
        .filter(|&&h| p.mask & (1 << h) != 0)
        .map(|&h| p.p[h])
        .min_by(|a, b| len(sub(*a, w)).total_cmp(&len(sub(*b, w))))
}

/// v1 striking-point speed from the weapon actor / hand speed: the swing
/// pivots about the shoulder, so the contact point moves |c − shoulder| /
/// |w − shoulder| times as fast (1..LEVER_MAX; LEVER_FALLBACK without a pose).
fn lever(p: Option<&Pose>, w: V3, c: V3) -> f32 {
    use posecodec::{HAND_L, HAND_R, UPPERARM_L, UPPERARM_R};
    let Some(p) = p else { return LEVER_FALLBACK };
    let has = |i: usize| p.mask & (1 << i) != 0;
    let d = |i: usize| if has(i) { len(sub(p.p[i], w)) } else { f32::INFINITY };
    let sh = if d(HAND_L) < d(HAND_R) { UPPERARM_L } else { UPPERARM_R };
    if !has(sh) { return LEVER_FALLBACK; }
    let (rc, rw) = (len(sub(c, p.p[sh])), len(sub(w, p.p[sh])));
    if rw > 5.0 && rc.is_finite() { (rc / rw).clamp(1.0, LEVER_MAX) } else { LEVER_FALLBACK }
}

/// Grip → weapon actor, extended to the nominal blade length.
fn estimate_blade(grip: Option<V3>, w: V3) -> Blade {
    match grip {
        Some(g) if len(sub(w, g)) >= 1.0 => {
            let dir = sub(w, g);
            let l = len(dir);
            Blade { base: g, tip: add(g, mul(dir, BLADE_LEN.max(l) / l)), vel: None }
        }
        _ => Blade { base: w, tip: w, vel: None },
    }
}

fn pose_capsules(p: &Pose) -> CapSet {
    let has = |i: usize| p.mask & (1 << i) != 0;
    let mut cs = CapSet { c: [Capsule { a: [0.0; 3], b: [0.0; 3], r: 0.0 }; MAX_CAPSULES], n: 0 };
    for &(a, b, r) in SEGMENTS {
        let c = if has(a) && has(b) {
            Capsule { a: p.p[a], b: p.p[b], r }
        } else if has(a) {
            Capsule { a: p.p[a], b: p.p[a], r: LONE_BONE_R }
        } else if has(b) {
            Capsule { a: p.p[b], b: p.p[b], r: LONE_BONE_R }
        } else {
            continue;
        };
        if cs.n < MAX_CAPSULES { cs.c[cs.n] = c; cs.n += 1; }
    }
    cs
}

/// Server knowledge of one connection, exposed for logging / RCON.
#[derive(Debug, Clone, PartialEq)]
pub struct ClockInfo {
    pub offset_ms: Option<i64>,
    pub jitter_p90_ms: f32,
    pub rtt_ms: Option<f32>,
    pub interval_ms: f32,
}

/// The server's estimate of what `viewer` displayed of `viewed` at
/// `viewer_ts` (viewed's sender clock), with the accepted tolerance.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ViewPrediction {
    pub expected: i64,
    pub tol: i64,
    pub rtt_known: bool,
    /// Part of the viewer's display delay of `viewed` caused by `viewed`'s
    /// OWN stream jitter beyond what its measured transport explains (lag
    /// switch / timestamp noise), ms. The defender-favouring rewind
    /// cap does not charge it: a victim that delays its own stream is shown
    /// late by everyone and is hit where it was shown.
    pub victim_excess: i64,
    /// The part of the excess the victim's measured transport does not
    /// explain either (the LagSwitch cheat counter, kept separate from
    /// the absolute cap in `victim_excess`).
    pub unexplained: i64,
    /// How old an honest viewer's display of `viewed` is on this path (ms): the
    /// viewer's measured RTT + the transport-explained buffer delay + a frame
    /// (0 while the RTT is unknown). Sizes the rewind cap for high-ping players.
    pub honest_lag: i64,
}

impl ViewPrediction {
    pub fn clamp(&self, hint: u32) -> u32 {
        (hint as i64).clamp(self.expected - self.tol, self.expected + self.tol).max(1) as u32
    }
    pub fn outlier(&self, hint: u32) -> bool {
        self.rtt_known && (hint as i64 - self.expected).abs() > self.tol + VIEW_OUTLIER_MS
    }
}

#[derive(Default)]
pub struct Store {
    peers: HashMap<PeerId, PeerHist>,
    /// 0 = default MAX_REWIND_MS.
    max_rewind_ms: i64,
    /// (viewer, src) -> what the relay sent viewer of src.
    relayed: HashMap<(PeerId, PeerId), RelayLog>,
}

/// Geometry that justified an accepted claim (logged by the server).
#[derive(Debug, Clone, PartialEq)]
pub struct Info {
    pub rewind_ms: i64,
    /// Victim view time actually used (server-clamped hint).
    pub view_ts: u32,
    /// The hint had to be clamped into the predicted window.
    pub view_clamped: bool,
    /// Contact → victim capsule surface (or normalised root) distance, uu.
    pub body_dist: f32,
    /// Contact → attacker weapon/hand/limb/swept blade distance, uu (-1 = no data).
    pub weapon_dist: f32,
    /// Judged with the swept blade test (blade stream present).
    pub swept: bool,
    /// The victim's blade was near enough to the attack that the victim's
    /// own client might still report a parry (clash): hold for the grace.
    pub parry_possible: bool,
    /// Victim blade → (contact, swept attacker blade, attack path) distances
    /// behind `parry_possible`, uu (∞ = not measured). Hit inspector data.
    pub parry_d: [f32; 3],
    /// Approach speed of the striking part from server history, uu/s.
    pub contact_speed: Option<f32>,
    /// Peak speed of the striking point over the PEAK_FRAMES stream frames
    /// before the contact (and at it), uu/s: the physical ceiling for the
    /// armour-stage inputs (the instantaneous `contact_speed` reads a blade
    /// already stopping in its target).
    pub peak_speed: Option<f32>,
    /// The contact was made by the attacker's body (fist, elbow, knee, …),
    /// not its blade: damage is capped as unarmed.
    pub unarmed: bool,
    /// Relative impact speed (uu/s): the attacker's striking point vs the
    /// victim's REPLICATED body at the contact (not the attacker's servo-driven
    /// stand-in). Bounds the impact inputs forwarded to the owner.
    pub rel_speed: Option<f32>,
    /// rel_speed comes from the streamed blade sweep (codec v2) or the body
    /// capsules (unarmed). False: the v1 blade estimate, too coarse to rescale
    /// the impact by (only the hard caps apply).
    pub rel_exact: bool,
    /// The cutting Box frame the owner replays (`hit_box_frame`), rebuilt by the server from
    /// the attacker's authenticated weapon Box and the victim's real bone as shown: the
    /// claim's own frame was measured on the stand-in, whose pose-sync error it carries.
    pub hit_box: Option<[f32; 13]>,
    /// How far the claim's frame (stand-in) was from that rebuilt frame: centre cm and
    /// rotation dot. Pose-sync telemetry; never a verdict.
    pub proxy_box_error: Option<(f32, f32)>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Eval {
    /// The victim never streamed (legacy client): caller applies only the
    /// legacy checks.
    NoData(&'static str),
    Reject(String),
    Accept(Info),
    /// The attacker's own stream does not cover its hit time yet (the claim
    /// overtook the pose stream): judge again shortly.
    Wait(&'static str),
}

// ---- geometry ------------------------------------------------------------

fn sub(a: V3, b: V3) -> V3 { [a[0] - b[0], a[1] - b[1], a[2] - b[2]] }
fn add(a: V3, b: V3) -> V3 { [a[0] + b[0], a[1] + b[1], a[2] + b[2]] }
fn mul(a: V3, s: f32) -> V3 { [a[0] * s, a[1] * s, a[2] * s] }
fn dot(a: V3, b: V3) -> f32 { a[0] * b[0] + a[1] * b[1] + a[2] * b[2] }
fn len(a: V3) -> f32 { dot(a, a).sqrt() }
fn lerp3(a: V3, b: V3, f: f32) -> V3 { add(a, mul(sub(b, a), f)) }
fn finite(v: &V3) -> bool { v.iter().all(|x| x.is_finite()) }

/// Distance from point p to segment ab.
pub(crate) fn point_seg(p: V3, a: V3, b: V3) -> f32 {
    let ab = sub(b, a);
    let l2 = dot(ab, ab);
    let t = if l2 < 1e-6 { 0.0 } else { (dot(sub(p, a), ab) / l2).clamp(0.0, 1.0) };
    len(sub(p, add(a, mul(ab, t))))
}

/// Parameter of the point on ab closest to p.
fn seg_param(p: V3, a: V3, b: V3) -> f32 {
    let ab = sub(b, a);
    let l2 = dot(ab, ab);
    if l2 < 1e-6 { 0.0 } else { (dot(sub(p, a), ab) / l2).clamp(0.0, 1.0) }
}

/// Closest distance between segments p1q1 and p2q2, plus the parameter of the
/// closest point on the first segment.
pub(crate) fn seg_seg(p1: V3, q1: V3, p2: V3, q2: V3) -> (f32, f32) {
    let (d1, d2, r) = (sub(q1, p1), sub(q2, p2), sub(p1, p2));
    let (a, e, f) = (dot(d1, d1), dot(d2, d2), dot(d2, r));
    let (s, t);
    if a < 1e-6 && e < 1e-6 {
        return (len(r), 0.0);
    }
    if a < 1e-6 {
        s = 0.0;
        t = (f / e).clamp(0.0, 1.0);
    } else {
        let c = dot(d1, r);
        if e < 1e-6 {
            t = 0.0;
            s = (-c / a).clamp(0.0, 1.0);
        } else {
            let b = dot(d1, d2);
            let denom = a * e - b * b;
            let mut s0 = if denom > 1e-6 { ((b * f - c * e) / denom).clamp(0.0, 1.0) } else { 0.0 };
            let mut t0 = (b * s0 + f) / e;
            if t0 < 0.0 { t0 = 0.0; s0 = (-c / a).clamp(0.0, 1.0); }
            else if t0 > 1.0 { t0 = 1.0; s0 = ((b - c) / a).clamp(0.0, 1.0); }
            s = s0;
            t = t0;
        }
    }
    let c1 = add(p1, mul(d1, s));
    let c2 = add(p2, mul(d2, t));
    (len(sub(c1, c2)), s)
}

/// Contact → nearest capsule surface (negative inside).
fn caps_dist(cs: &CapSet, p: V3) -> f32 {
    cs.iter().map(|c| point_seg(p, c.a, c.b) - c.r).fold(f32::INFINITY, f32::min)
}

/// Blade segment → nearest capsule surface.
fn blade_caps_dist(cs: &CapSet, b: &Blade) -> f32 {
    cs.iter().map(|c| seg_seg(b.base, b.tip, c.a, c.b).0 - c.r).fold(f32::INFINITY, f32::min)
}

fn sweep_steps(b0: &Blade, b1: &Blade) -> usize {
    let d = len(sub(b1.tip, b0.tip)).max(len(sub(b1.base, b0.base)));
    ((d / SWEEP_STEP_UU).ceil() as usize).clamp(1, SWEEP_MAX_STEPS)
}

/// Minimum of `f` over the blade swept linearly from b0 to b1 (endpoints
/// interpolated; sub-stepped so no endpoint moves more than SWEEP_STEP_UU).
fn sweep_min(b0: &Blade, b1: &Blade, mut f: impl FnMut(&Blade) -> f32) -> (f32, f32) {
    let n = sweep_steps(b0, b1);
    let mut best = (f32::INFINITY, 1.0);
    for k in 0..=n {
        let s = k as f32 / n as f32;
        let d = f(&Blade::lerp(b0, b1, s));
        if d < best.0 { best = (d, s); }
    }
    best
}

/// Closest approach of two blades both swept over the same interval.
pub(crate) fn swept_blades_dist(a0: &Blade, a1: &Blade, b0: &Blade, b1: &Blade) -> f32 {
    let n = sweep_steps(a0, a1).max(sweep_steps(b0, b1));
    (0..=n).map(|k| {
        let s = k as f32 / n as f32;
        let (a, b) = (Blade::lerp(a0, a1, s), Blade::lerp(b0, b1, s));
        seg_seg(a.base, a.tip, b.base, b.tip).0
    }).fold(f32::INFINITY, f32::min)
}

fn rel(a: u32, b: u32) -> i64 { a as i64 - b as i64 }
fn ts_sub(t: u32, ms: i64) -> u32 { (t as i64 - ms).max(1) as u32 }

// ---- store ---------------------------------------------------------------

impl Store {
    fn peer(&mut self, id: PeerId) -> &mut PeerHist {
        self.peers.entry(id).or_insert_with(PeerHist::new)
    }

    pub fn max_rewind(&self) -> i64 {
        if self.max_rewind_ms > 0 { self.max_rewind_ms } else { MAX_REWIND_MS }
    }

    /// Rewind cap: 300 ms default, up to 400 ms on high-latency servers.
    pub fn set_max_rewind(&mut self, ms: i64) {
        self.max_rewind_ms = ms.clamp(MAX_REWIND_MS, HIGH_LATENCY_REWIND_MS);
    }

    fn note_rx(&mut self, id: PeerId, ts: u32, now_ms: i64) {
        self.peer(id).clock.note(now_ms, ts);
    }

    pub fn record_root(&mut self, id: PeerId, ts: u32, pos: V3, now_ms: i64) {
        if ts != 0 && finite(&pos) {
            self.note_rx(id, ts, now_ms);
            self.peer(id).root.push(ts, pos);
        }
    }

    /// Record one pose frame. Returns whether the frame may be RELAYED to the
    /// other players: false for a frame that fails the pose-to-root
    /// tie (or has no pelvis); true for a recorded frame, an empty / untimed
    /// one, and a frame no timestamped root covers (relayed for display, but
    /// never recorded: lag comp does not use poses it cannot tie).
    pub fn record_pose(&mut self, id: PeerId, f: &PoseFrame, now_ms: i64) -> bool {
        if f.ts == 0 || f.bones.is_empty() { return true; }
        let mut pose = Pose { p: [[0.0; 3]; BONE_COUNT], mask: 0 };
        for (idx, p, _) in &f.bones {
            let i = *idx as usize;
            if i < BONE_COUNT && finite(p) {
                pose.p[i] = *p;
                pose.mask |= 1 << i;
            }
        }
        if pose.mask == 0 { return true; }
        self.note_rx(id, f.ts, now_ms);
        match self.pose_check(id, f.ts, &pose) {
            Tie::Off => {
                cheat::bump(id, CheatKind::Teleport);
                false
            }
            Tie::NoPelvis => false,
            Tie::Uncovered => true,
            Tie::Ok => {
                self.peer(id).pose.push(f.ts, pose);
                true
            }
        }
    }

    /// Is this pose tied to the sender's own
    /// validated (timestamped) root?
    /// * the pelvis is REQUIRED (a frame without it would skip the
    ///   check, so the other bones could be streamed anywhere);
    /// * the pelvis lies within POSE_ROOT_MAX of the root, every other bone
    ///   within POSE_ROOT_MAX + BODY_EXTENT, and the pelvis moved no faster
    ///   than a body can since the previous sample (else `Off`: a skeleton
    ///   streamed next to the victim while the root stays put);
    /// * no timestamped root covers ts (a peer streaming untimestamped
    ///   roots, or none at all, would get its poses recorded unjudged):
    ///   `Uncovered`, never recorded.
    fn pose_check(&self, id: PeerId, ts: u32, pose: &Pose) -> Tie {
        use posecodec::PELVIS;
        if pose.mask & (1 << PELVIS) == 0 { return Tie::NoPelvis; }
        let pelvis = pose.p[PELVIS];
        match self.root_tie(id, ts, pelvis, 0.0) {
            Tie::Ok => {}
            other => return other,
        }
        for i in 0..BONE_COUNT {
            if i != PELVIS && pose.mask & (1 << i) != 0 && self.root_tie(id, ts, pose.p[i], BODY_EXTENT) == Tie::Off {
                return Tie::Off;
            }
        }
        let Some(h) = self.peers.get(&id) else { return Tie::Ok };
        let i = h.pose.q.partition_point(|e| e.0 < ts);
        match if i > 0 { Some(&h.pose.q[i - 1]) } else { None } {
            Some((pt, pp)) if pp.mask & (1 << PELVIS) != 0 && *pt != ts => {
                let dt = (ts - pt) as f32 / 1000.0;
                if len(sub(pelvis, pp.p[PELVIS])) > PELVIS_SPEED_MAX * dt + PELVIS_STEP_SLACK { Tie::Off } else { Tie::Ok }
            }
            _ => Tie::Ok,
        }
    }

    /// `p` against `id`'s validated root at `ts`: further than POSE_ROOT_MAX +
    /// `extra` is `Off`. A root sample may lag the stream by up to
    /// ROOT_COVER_LEAD_MS (a lost / speed-capped root packet, a ragdoll
    /// launch); beyond FUTURE_MS the body may have moved on, at most at
    /// PELVIS_SPEED_MAX, which widens the bound. No root sample covering ts
    /// (no timestamped root at all, or a root stream that stopped):
    /// `Uncovered`.
    fn root_tie(&self, id: PeerId, ts: u32, p: V3, extra: f32) -> Tie {
        let Some(h) = self.peers.get(&id) else { return Tie::Uncovered };
        let Some(r) = h.root.sample(ts, ROOT_COVER_LEAD_MS) else { return Tie::Uncovered };
        let lead = h.root.newest().map_or(0, |n| rel(ts, n)).max(0);
        let stale = (lead - FUTURE_MS).max(0) as f32 / 1000.0 * PELVIS_SPEED_MAX;
        if len(sub(p, r)) > POSE_ROOT_MAX + extra + stale { Tie::Off } else { Tie::Ok }
    }

    /// Off the root (for blade / capsule samples): `Off`; `Uncovered` is
    /// reported separately by `root_tie`.
    fn off_root(&self, id: PeerId, ts: u32, p: V3, extra: f32) -> bool {
        self.root_tie(id, ts, p, extra) == Tie::Off
    }

    pub fn record_weapon(&mut self, id: PeerId, ts: u32, pos: V3, now_ms: i64) {
        if ts != 0 && finite(&pos) {
            self.note_rx(id, ts, now_ms);
            let p = self.peer(id);
            // Teleport marks for the weapon actor too (v1 clash fallback).
            let i = p.weapon.q.partition_point(|e| e.0 < ts);
            let fast = |a: &(u32, V3)| {
                let dt = (ts as i64 - a.0 as i64).abs().max(8) as f32;
                a.0 != ts && len(sub(pos, a.1)) * 1000.0 / dt > CLASH_BLADE_SPEED_MAX
            };
            let jump = (i > 0 && fast(&p.weapon.q[i - 1])) || (i < p.weapon.q.len() && fast(&p.weapon.q[i]));
            if p.weapon.push(ts, pos) && jump {
                p.blade_jumps.push_back(ts);
                let newest = p.weapon.newest().unwrap_or(ts).max(p.blade.newest().unwrap_or(0));
                while p.blade_jumps.len() > 64
                    || p.blade_jumps.front().map_or(false, |f| newest.saturating_sub(*f) > HISTORY_MS) {
                    p.blade_jumps.pop_front();
                }
            }
        }
    }

    /// Full-skeleton stream: blade base/tip (+ optional tip velocity) of the
    /// held weapon at sender time `ts`.
    pub fn record_blade(&mut self, id: PeerId, ts: u32, blade: Blade, now_ms: i64) {
        self.record_blade_hand(id, ts, blade, now_ms, false)
    }
    /// Some native tip scenes are only ~1 uu from the root. Both the blade
    /// history and its module bounds must use the same pose-derived segment.
    fn repair_blade(&self, id: PeerId, ts: u32, mut blade: Blade, offhand: bool) -> Option<Blade> {
        if len(sub(blade.tip, blade.base)) < DEGENERATE_BLADE_UU {
            let class = crate::validate::damage::weapon_class_for_hand(id, offhand);
            if class == crate::validate::damage::WeaponClass::Unarmed { return None; }
            let pose = self.peers.get(&id)?.pose.sample(ts, FUTURE_MS)?;
            let dir = degenerate_dir(&pose, blade.base)?;
            blade.tip = add(blade.base, mul(dir, crate::validate::pose::geom(class).nominal));
        }
        Some(blade)
    }
    fn record_weapon_shape(&mut self, id: PeerId, ts: u32, w: &posecodec::v2::Weapon, offhand: bool) -> bool {
        use crate::validate::damage::WeaponClass;
        let class = crate::validate::damage::weapon_class_for_hand(id,offhand);
        let g = crate::validate::pose::geom(class);
        // Module widths, not a radius granted along the entire blade. Shields
        // have broad faces; polearm/axe heads and sword guards are narrower.
        let width = match class {
            WeaponClass::Shield => 80.0, WeaponClass::Unknown => 60.0,
            WeaponClass::Polearm | WeaponClass::Axe => 40.0,
            WeaponClass::Sword | WeaponClass::Blunt => 30.0,
            WeaponClass::Dagger | WeaponClass::Unarmed => 20.0,
        };
        // A refused shape leaves the attacker without module history ("no_cover: original
        // source class unavailable" on every claim of that weapon): say why, rate-limited.
        let refuse = |why: &str| {
            if crate::validate::rate::log_ok("weapon_shape_refused") {
                tracing::warn!(peer_id = id, ?class, offhand, why, "weapon shape refused; its claims will find no module history");
            }
            false
        };
        if ts == 0 || w.boxes.is_empty() || w.boxes.len() > posecodec::v2::MAX_WEAPON_BOXES || !finite(&w.p) { return refuse("empty, oversized or non-finite"); }
        match self.root_tie(id, ts, w.p, BODY_EXTENT) {
            Tie::Ok => {}
            Tie::Off => return refuse("weapon off the root (too far)"),
            Tie::Uncovered => return refuse("no root history at the sample time"),
            Tie::NoPelvis => return refuse("no pelvis sample at the sample time"),
        }
        let (base, tip) = posecodec::v2::blade_world(w);
        let raw = Blade { base, tip, vel: None };
        // A compact module can still fit the raw bounds without a skeletal
        // sample (e.g. a shield). Extending those bounds requires the same
        // authenticated pose used to repair the blade, never the box itself.
        let blade = self.repair_blade(id, ts, raw, offhand).unwrap_or(raw);
        let inv = posecodec::v2::qconj(w.q);
        let base = posecodec::v2::qrot(inv, sub(blade.base, w.p));
        let tip = posecodec::v2::qrot(inv, sub(blade.tip, w.p));
        // The envelope runs from the hilt's end to the tip: grip and pommel modules sit
        // behind the blade base (the guard) by up to the class's grip_max. Measured live: a
        // modular T3 arming sword's grip corner 31.5 cm behind its base was refused against
        // the blade-only segment (width 30), so every claim of that sword found no history.
        let axis = sub(tip, base);
        let alen = len(axis);
        let base = if alen > 1e-3 { sub(base, std::array::from_fn(|k| axis[k] / alen * g.grip_max)) } else { base };
        let mut s = WeaponShape { weapon_id:w.id, p: w.p, q: w.q, boxes: [posecodec::v2::WeaponBox::default(); posecodec::v2::MAX_WEAPON_BOXES], n: w.boxes.len() };
        for (i,b) in w.boxes.iter().enumerate() {
            if !b.valid() { return refuse("invalid box"); }
            if w.boxes.iter().filter(|p|p.component==b.component).count()!=1
                || (b.child_of!=0 && !w.boxes.iter().any(|p|p.component==b.child_of && p.child_of==0)) {return refuse("duplicate component or orphan cutting child");}
            for bits in 0..8 {
                let corner = std::array::from_fn(|k| if bits & (1<<k) == 0 { -b.half[k] } else { b.half[k] });
                let p = add(b.p, posecodec::v2::qrot(b.q,corner));
                if len(sub(p,base)) > g.blade_max + g.grip_max + width || point_seg(p,base,tip) > width {
                    if crate::validate::rate::log_ok("weapon_shape_refused") {
                        tracing::warn!(peer_id = id, ?class, offhand, component = b.component, from_base = len(sub(p,base)),
                            blade_max = g.blade_max, off_blade = point_seg(p,base,tip), width, blade_len = len(sub(tip,base)),
                            "weapon shape refused: module corner outside the class envelope; its claims will find no module history");
                    }
                    return false;
                }
            }
            s.boxes[i] = *b;
        }
        let p = self.peer(id);
        let ring = if offhand { &mut p.offhand_shapes } else { &mut p.shapes };
        ring.push(ts,s)
    }
    fn record_body_strikers(&mut self,id:PeerId,ts:u32,f:&posecodec::v2::Full) {
        let p=self.peer(id);
        let mut frame=hsmp_pose::poseplay::Frame::from_v2(0,f);
        // Preserve actual fractional native time. Synthetic/local callers may
        // supply the authoritative key separately without rewriting Full.ts.
        if !f.ts.is_finite() || f.ts.floor() as u32!=ts {
            frame.ts=ts as f64+frame.extra.as_ref().map_or(0.0,|e|e.step);
        }
        p.native_frames.insert(ts,frame);
        let newest=*p.native_frames.keys().next_back().unwrap_or(&ts);
        p.native_frames.retain(|t,_|newest.saturating_sub(*t)<=HISTORY_MS);
        while p.native_frames.len()>RING_CAP {
            if let Some(first)=p.native_frames.keys().next().copied(){p.native_frames.remove(&first);}else{break;}
        }
        self.peer(id).bone_frames.push(ts,BoneFrames {p:std::array::from_fn(|i|f.bones[i].p),q:std::array::from_fn(|i|f.bones[i].q),scale:f.k});
        let mut set=StrikerSet::empty(f.strikers.is_some());
        if let Some(shapes)=&f.strikers {
            for s in shapes.iter().take(posecodec::v2::MAX_BODY_STRIKERS) {
                if !s.valid() {continue;}
                let bone=f.bones[s.bone().unwrap()];
                let p=add(bone.p,posecodec::v2::qrot(bone.q,s.p));
                if self.root_tie(id,ts,p,BODY_EXTENT)!=Tie::Ok {continue;}
                set.shapes[set.n]=posecodec::v2::BodyStriker {p,q:posecodec::v2::qmul(bone.q,s.q),..*s};set.n+=1;
            }
        }
        self.peer(id).strikers.push(ts,set);
    }
    pub fn record_blade_hand(&mut self, id: PeerId, ts: u32, blade: Blade, now_ms: i64, offhand: bool) {
        let ok = finite(&blade.base) && finite(&blade.tip) && blade.vel.map_or(true, |v| finite(&v))
            && len(sub(blade.tip, blade.base)) <= 400.0;
        if ts == 0 || !ok { return; }
        // The held blade stays within arm's reach of the validated root
        // (and only a blade a timestamped root covers is recorded).
        match self.root_tie(id, ts, blade.base, BODY_EXTENT) {
            Tie::Ok => {}
            t => {
                self.note_rx(id, ts, now_ms);
                if t == Tie::Off { cheat::bump(id, CheatKind::Teleport); }
                return;
            }
        }
        let Some(blade) = self.repair_blade(id, ts, blade, offhand)
            else { self.note_rx(id, ts, now_ms); return };
        self.note_rx(id, ts, now_ms);
        let p = self.peer(id);
        let ring = if offhand { &mut p.offhand } else { &mut p.blade };
        // Teleport marks: the tip jumped faster than a blade can move from
        // its neighbouring samples (reordered UDP: either side).
        let i = ring.q.partition_point(|e| e.0 < ts);
        let fast = |a: &(u32, Blade), b: (u32, &Blade)| {
            let dt = (b.0 as i64 - a.0 as i64).abs().max(8) as f32;
            len(sub(b.1.tip, a.1.tip)) * 1000.0 / dt > CLASH_BLADE_SPEED_MAX
        };
        let jump = (i > 0 && fast(&ring.q[i - 1], (ts, &blade)))
            || (i < ring.q.len() && ring.q[i].0 != ts && fast(&ring.q[i], (ts, &blade)));
        let inserted = ring.push(ts, blade);
        if inserted && jump {
            p.blade_jumps.push_back(ts);
            let newest = p.blade.newest().into_iter().chain(p.offhand.newest()).max().unwrap_or(ts);
            while p.blade_jumps.len() > 64
                || p.blade_jumps.front().map_or(false, |f| newest.saturating_sub(*f) > HISTORY_MS) {
                p.blade_jumps.pop_front();
            }
        }
    }

    /// Full-skeleton stream: per-bone body capsules at sender time `ts`
    /// (keep the same order frame to frame; at most MAX_CAPSULES).
    pub fn record_capsules(&mut self, id: PeerId, ts: u32, caps: &[Capsule], now_ms: i64) {
        if ts == 0 || caps.is_empty() { return; }
        let mut cs = CapSet { c: [Capsule { a: [0.0; 3], b: [0.0; 3], r: 0.0 }; MAX_CAPSULES], n: 0 };
        for c in caps.iter().take(MAX_CAPSULES) {
            if finite(&c.a) && finite(&c.b) && c.r.is_finite() && (0.0..=40.0).contains(&c.r) {
                cs.c[cs.n] = *c;
                cs.n += 1;
            }
        }
        if cs.n > 0 {
            self.note_rx(id, ts, now_ms);
            // The body capsules stay on the validated root (and a
            // timestamped root must cover them).
            if cs.iter().any(|c| self.off_root(id, ts, c.a, BODY_EXTENT) || self.off_root(id, ts, c.b, BODY_EXTENT)) {
                cheat::bump(id, CheatKind::Teleport);
                return;
            }
            if cs.n > 0 && self.root_tie(id, ts, cs.c[0].a, f32::INFINITY) == Tie::Uncovered { return; }
            self.peer(id).caps.push(ts, cs);
        }
    }

    pub fn note_rtt(&mut self, id: PeerId, rtt_ms: f32, now_ms: i64) {
        self.peer(id).clock.note_rtt(now_ms, rtt_ms);
    }

    pub fn clock_info(&self, id: PeerId, now_ms: i64) -> Option<ClockInfo> {
        let p = self.peers.get(&id)?;
        Some(ClockInfo {
            offset_ms: p.clock.offset(),
            jitter_p90_ms: p.clock.jitter_p90(),
            rtt_ms: p.clock.rtt(now_ms),
            interval_ms: p.interval(),
        })
    }

    pub fn note_death(&mut self, id: PeerId, now_ms: i64) {
        let p = self.peer(id);
        // The attacker clock at the death: its newest sample, or the server
        // time mapped through its clock map when that is newer.
        let mapped = p.clock.offset().map(|o| (now_ms - o).max(1) as u32);
        let ts = [p.newest(), mapped].into_iter().flatten().max().unwrap_or(0);
        p.death = Some((now_ms, ts));
    }

    pub fn note_alive(&mut self, id: PeerId) {
        if let Some(p) = self.peers.get_mut(&id) { p.death = None; }
    }

    pub fn forget(&mut self, id: PeerId) {
        self.peers.remove(&id);
        self.relayed.retain(|(v, s), _| *v != id && *s != id);
    }

    pub fn has_pose_context(&self,id:PeerId,match_id:u64,round:u32,life:u16)->bool {
        self.peers.get(&id).is_some_and(|p|p.context==Some(posecodec::v2::Context{match_id,round,life}))
    }
    pub fn has_accepted_pose_context(&self,id:PeerId,match_id:u64,round:u32,life:u16)->bool {
        self.has_pose_context(id,match_id,round,life)
            && self.peers.get(&id).is_some_and(|p| !p.pose.q.is_empty())
    }

    /// Caller checks the context against the authoritative spawn generation first.
    /// Root/clock are only positional bounds, never evidence of a new life's body.
    pub fn bind_pose_context(&mut self,id:PeerId,context:Option<posecodec::v2::Context>)->bool {
        let p=self.peer(id);
        if p.context==context {return true;}
        if p.context.is_some() && context.is_none() {return false;}
        if let (Some(old),Some(new))=(p.context,context) {
            if old.match_id==new.match_id && (new.round,new.life)<(old.round,old.life) {return false;}
        }
        p.context=context;
        p.pose=Ring::new(); p.bone_frames=Ring::new(); p.native_frames.clear(); p.weapon=Ring::new(); p.blade=Ring::new(); p.offhand=Ring::new();
        p.shapes=Ring::new(); p.offhand_shapes=Ring::new(); p.strikers=Ring::new(); p.caps=Ring::new();
        p.clashes.clear(); p.touches.clear(); p.blade_jumps.clear();
        self.relayed.retain(|(v,s),_|*v!=id && *s!=id);
        true
    }

    /// Codec of `id`'s last skeletal frame.
    pub fn note_pose_codec(&mut self, id: PeerId, v2: bool) {
        self.peer(id).pose_v2 = Some(v2);
    }

    /// The relay forwarded `src`'s frame `ts` to `viewer`, which gets `src`'s
    /// frames every `interval_ms`: remember the stamp (lag comp
    /// judges `viewer`'s hits on `src` against what it was sent) and advance
    /// the server's model of `viewer`'s buffer delay for `src`.
    pub fn note_relayed(&mut self, viewer: PeerId, src: PeerId, ts: u32, interval_ms: u16, now_ms: i64) {
        if ts == 0 || viewer == src { return; }
        let Some(s) = self.peers.get(&src) else { return };
        let iv = if interval_ms > 0 { interval_ms as f32 } else { s.interval() };
        let jv = s.clock.jitter_p90();
        let ja = self.peers.get(&viewer).map_or(0.0, |a| a.clock.jitter_bounded(now_ms));
        let target = display_delay(s, (ja * ja + jv * jv).sqrt(), Some(iv));
        let sample_ts = s.native_frames.get(&ts).map_or(ts as f64, |f| f.ts);
        let log = self.relayed.entry((viewer, src)).or_insert_with(|| RelayLog {
            ts: VecDeque::new(), sample_ts: HashMap::new(), interval: iv, model: DelayModel::new(now_ms),
        });
        if let Some(&n) = log.ts.back() {
            if (ts as i64) + RESET_BACKSTEP_MS < n as i64 {
                // The sender's clock went back (game restart): start over.
                log.ts.clear();
                log.sample_ts.clear();
                log.model = DelayModel::new(now_ms);
            }
        }
        if now_ms - log.model.at > RELAY_MODEL_TTL_MS {
            // The relay paused for this pair: the receiver re-syncs.
            log.model = DelayModel::new(now_ms);
        }
        let i = log.ts.partition_point(|&x| x < ts);
        if i < log.ts.len() && log.ts[i] == ts { return; }
        log.ts.insert(i, ts);
        log.sample_ts.insert(ts, sample_ts);
        let newest = *log.ts.back().unwrap_or(&ts);
        while log.ts.len() > RELAY_LOG_CAP || log.ts.front().map_or(false, |&f| newest.saturating_sub(f) > HISTORY_MS) {
            if let Some(old) = log.ts.pop_front() { log.sample_ts.remove(&old); }
        }
        log.interval = iv;
        log.model.on_frame(now_ms, target);
    }

    /// Fresh relay data of the pair, if any.
    fn relay_log(&self, viewer: PeerId, src: PeerId, now_ms: i64) -> Option<&RelayLog> {
        self.relayed.get(&(viewer, src)).filter(|l| !l.ts.is_empty() && now_ms - l.model.at <= RELAY_MODEL_TTL_MS)
    }

    /// `viewer`'s buffer delay of `viewed` (ms): the modelled one when the
    /// relay reports the pair, else the instantaneous formula.
    fn view_delay(&self, viewer: PeerId, viewed: PeerId, v: &PeerHist, jpath: f32, now_ms: i64) -> f32 {
        match self.relay_log(viewer, viewed, now_ms) {
            Some(l) => l.model.shown_at(now_ms),
            None => display_delay(v, jpath, None),
        }
    }

    /// `src`'s body capsules as `viewer` displayed them at `t`:
    /// interpolated between the two frames the relay actually sent it around
    /// `t` (past its newest one: extrapolated briefly, then held, like
    /// poseplay). Without relay data: the full-rate history.
    fn shown_capsules(&self, viewer: PeerId, src: PeerId, v: &PeerHist, t: u32, lead: i64, now_ms: i64) -> Option<CapSet> {
        if v.pose_v2==Some(true) || !v.native_frames.is_empty() || v.context.is_some() {
            let shown=self.shown_bone_frames(viewer,src,v,t,lead,now_ms).ok()?;
            let mut full=posecodec::v2::Full::default();full.k=shown.scale;
            for i in 0..posecodec::v2::NB {full.bones[i].p=shown.p[i];full.bones[i].q=shown.q[i];}
            // Use exactly the same v2 body construction/radii as ingestion,
            // including the crown, hands and rotated/scaled foot-to-ball axes.
            let caps=posecodec::v2::extras_of(&full).caps;
            let mut out=CapSet {c:[Capsule{a:[0.0;3],b:[0.0;3],r:0.0};MAX_CAPSULES],n:caps.len()};
            for (i,(a,b,r)) in caps.into_iter().enumerate() {out.c[i]=Capsule{a,b,r};}
            return Some(out);
        }
        // Historical v1 fixtures/clients have no native frame stream. Native
        // production contacts above never use this legacy approximation.
        let full = || v.capsules(t, lead);
        let Some(log) = self.relay_log(viewer, src, now_ms) else { return full() };
        let i = log.ts.partition_point(|&x| x <= t);
        if i == 0 { return full(); }
        let ta = log.ts[i - 1];
        if i == log.ts.len() {
            return v.capsules(t.min(ta.saturating_add(PLAY_EXTRAP_MS)), lead).or_else(full);
        }
        let tb = log.ts[i];
        match (v.capsules(ta, 0), v.capsules(tb, 0)) {
            (Some(a), Some(b)) => Some(CapSet::lerp(&a, &b, (t - ta) as f32 / (tb - ta).max(1) as f32)),
            _ => full(),
        }
    }

    /// Cutting geometry must use the victim frames actually relayed to this
    /// attacker, like shown_capsules. A full-rate intermediate frame that the
    /// attacker never received is not its displayed victim bone frame.
    fn shown_native_sample(&self, viewer:PeerId,src:PeerId,v:&PeerHist,t:u32,now_ms:i64)->Result<hsmp_pose::poseplay::DeliveredSample,&'static str> {
        let log=self.relay_log(viewer,src,now_ms).ok_or("no relay log for displayed bone history")?;
        let mut delivered:Vec<(u32,f64)>=log.ts.iter().map(|ts|(*ts,log.sample_ts[ts])).collect();
        delivered.sort_by(|a,b|a.1.total_cmp(&b.1).then(a.0.cmp(&b.0)));
        let i=delivered.partition_point(|&(_,at)|at<=t as f64);
        // The enclosing delivered endpoints (and last predecessor for
        // extrapolation) must still exist. Older unrelated relay entries may
        // legitimately have aged out of the full-rate cache.
        if i>0 {v.native_frames.get(&delivered[i-1].0).ok_or("delivered frame before the view time not cached")?;}
        if i<delivered.len() {v.native_frames.get(&delivered[i].0).ok_or("delivered frame after the view time not cached")?;}
        if i==delivered.len() && i>1 {v.native_frames.get(&delivered[i-2].0).ok_or("delivered predecessor for extrapolation not cached")?;}
        let mut frames=VecDeque::new();
        for ts in &log.ts {
            let Some(frame)=v.native_frames.get(ts) else {continue;};
            if frame.extra.as_ref().ok_or("cached frame without native extras")?.context!=v.context {return Err("a delivered frame belongs to another life");}
            let i=frames.partition_point(|f:&hsmp_pose::poseplay::Frame|f.ts<frame.ts);
            if frames.get(i).is_some_and(|f|f.ts==frame.ts){continue;}
            frames.insert(i,frame.clone());
        }
        let Some(sample)=hsmp_pose::poseplay::sample_delivered(&frames,t as f64) else {
            if crate::validate::rate::log_ok("delivered_sample") {
                tracing::info!(viewer, src, detail = %hsmp_pose::poseplay::explain_delivered(&frames, t as f64),
                    "delivered frames do not sample at the view time");
            }
            return Err("delivered frames do not sample at the view time");
        };
        Ok(sample)
    }

    fn shown_bone_frames(&self,viewer:PeerId,src:PeerId,v:&PeerHist,t:u32,_lead:i64,now_ms:i64)->Result<BoneFrames,&'static str> {
        let sample=self.shown_native_sample(viewer,src,v,t,now_ms)?;
        let b=sample.bones;
        Ok(BoneFrames {p:std::array::from_fn(|i|[b[i][0],b[i][1],b[i][2]]),
            q:std::array::from_fn(|i|[b[i][3],b[i][4],b[i][5],b[i][6]]),
            scale:sample.extra.as_ref().ok_or("sample without native extras")?.k})
    }

    fn shown_blade(&self,viewer:PeerId,src:PeerId,v:&PeerHist,t:u32,offhand:bool,now_ms:i64)->Option<(Blade,bool)> {
        if v.pose_v2==Some(true) || !v.native_frames.is_empty() || v.context.is_some() {
            let sample=self.shown_native_sample(viewer,src,v,t,now_ms).ok()?;
            let slot=if offhand {1} else {0};
            let (_,_,base,tip)=sample.extra.as_ref()?.weapons[slot]?;
            let xf=sample.bones[hsmp_pose::poseplay::WPN_R+slot];
            let p=[xf[0],xf[1],xf[2]];let q=[xf[3],xf[4],xf[5],xf[6]];
            return Some((Blade {base:add(p,posecodec::v2::qrot(q,base)),tip:add(p,posecodec::v2::qrot(q,tip)),vel:None},true));
        }
        let ring=if offhand {&v.offhand} else {&v.blade};
        (BladeView{hist:v,blade:ring}).blade_at(t,FUTURE_MS)
    }

    /// What did `viewer` display of `viewed` at its own time `viewer_ts`?
    /// None without clock data for both.
    pub fn predict_view(&self, viewer: PeerId, viewed: PeerId, viewer_ts: u32, now_ms: i64) -> Option<ViewPrediction> {
        let a = self.peers.get(&viewer)?;
        let v = self.peers.get(&viewed)?;
        let (off_a, off_v) = (a.clock.offset()?, v.clock.offset()?);
        // Victim uplink (its stream's real lateness: that is what every
        // receiver buffers) + attacker downlink, with the attacker's uplink
        // jitter as its proxy, bounded by the attacker's measured transport
        // (an attacker inflating its own jitter must not shift the view).
        let ja = a.clock.jitter_bounded(now_ms);
        let (jv, jv_t) = (v.clock.jitter_p90(), v.clock.jitter_bounded(now_ms));
        // The transport bound comes from the victim's own acks, so a
        // lag switch that holds its acks too inflates rttvar and the bound with
        // it. The share of the victim's stream jitter the rewind cap still
        // charges is also capped absolutely (VICTIM_JITTER_CHARGED_MS): beyond
        // that a victim is shown late by everyone and is hit where it was shown,
        // whether the jitter is a cheat or an awful link (an honest victim on
        // such a link simply gets less defender benefit; its attackers' hits
        // are still judged against what they saw).
        let jv_b = jv_t.min(VICTIM_JITTER_CHARGED_MS);
        let jpath = (ja * ja + jv * jv).sqrt();
        let jpath_b = (ja * ja + jv_b * jv_b).sqrt();
        let jpath_t = (ja * ja + jv_t * jv_t).sqrt();
        // With relay data the viewer's modelled buffer delay of this
        // sender (decimation interval and the receiver's rise-fast /
        // fall-slow / slewed dynamics); else the instantaneous formula.
        let iv = self.relay_log(viewer, viewed, now_ms).map(|l| l.interval);
        let formula = display_delay(v, jpath, iv);
        let interp = self.view_delay(viewer, viewed, v, jpath, now_ms);
        let victim_excess = ((formula - display_delay(v, jpath_b, iv)).round() as i64).clamp(0, VICTIM_EXCESS_MAX_MS);
        // Only the part the measured transport does not explain is suspicious.
        let unexplained = ((formula - display_delay(v, jpath_t, iv)).round() as i64).clamp(0, VICTIM_EXCESS_MAX_MS);
        let (rtt, slack, known) = match a.clock.rtt(now_ms) {
            Some(r) => (r, 0, true),
            None => (DEFAULT_RTT_MS as f32, UNKNOWN_RTT_SLACK_MS, false),
        };
        let expected = viewer_ts as i64 + off_a - off_v - rtt.round() as i64 - interp.round() as i64 - FRAME_MS;
        // The window follows the transport-explained jitter only, with a fixed
        // ceiling (never widened by client-controlled lateness).
        // (1.8 x the p95 path jitter = the former 2 x p90: the same window width.)
        let tol = ((1.8 * jpath_b).round() as i64 + FRAME_MS).clamp(TOL_FLOOR_MS, TOL_MAX_MS) + slack;
        // The viewer's modelled buffer, less what the victim's own excess jitter added.
        let buf = display_delay(v, jpath_b, iv).max(interp - victim_excess as f32);
        let honest_lag = if known { rtt.round() as i64 + buf.round() as i64 + FRAME_MS } else { 0 };
        Some(ViewPrediction { expected, tol, rtt_known: known, victim_excess, unexplained, honest_lag })
    }

    /// Transport RTT variation of `id`'s connection (ms), from the connection
    /// layer (server: once a second; simulator: its link model). Bounds the
    /// stream jitter lag comp believes.
    pub fn note_net_jitter(&mut self, id: PeerId, rttvar_ms: f32, now_ms: i64) {
        self.peer(id).clock.note_net_jitter(now_ms, rttvar_ms);
    }

    /// Record a clash report (rate-limited; judged lazily in `parried`, once
    /// both blade streams have arrived).
    pub fn record_clash(&mut self, reporter: PeerId, other: PeerId, my_ts: u32, other_ts: u32, now_ms: i64) {
        if reporter == other || my_ts == 0 || other_ts == 0 { return; }
        // `other` is client-chosen: only peers the server has history for
        // (connected, streaming) can be clashed with, and the per-other
        // bucket map is bounded (no memory growth from forged ids).
        if !self.peers.get(&other).map_or(false, |o| o.has_history()) { return; }
        let p = self.peer(reporter);
        if now_ms < p.distrust_until {
            cheat::bump(reporter, CheatKind::ParryRateLimited);
            return;
        }
        if p.clash_bucket.len() >= CLASH_BUCKETS_MAX && !p.clash_bucket.contains_key(&other) {
            // Evict the least recently used bucket.
            if let Some(k) = p.clash_bucket.iter().min_by_key(|e| e.1.at_ms).map(|e| *e.0) { p.clash_bucket.remove(&k); }
        }
        let b = p.clash_bucket.entry(other).or_insert(Bucket::full(CLASH_BUCKET_CAP, now_ms));
        if !b.take(CLASH_BUCKET_CAP, CLASH_REFILL_PER_S, now_ms) {
            cheat::bump(reporter, CheatKind::ParryRateLimited);
            return;
        }
        while p.clashes.front().map_or(false, |c| now_ms - c.at > CLASH_TTL_MS) {
            p.clashes.pop_front();
        }
        if p.clashes.len() < 64 {
            p.clashes.push_back(Clash { at: now_ms, other, my_ts, other_ts, valid: None, notified: false });
        }
    }

    /// Record a victim's touch report: `victim`'s screen showed
    /// `attacker`'s stand-in reach its body while displaying attacker time
    /// `other_ts`. Only ever keeps a hit (see `parried`), so it needs no
    /// geometry check; it is bounded per victim and ages out with the clashes.
    pub fn record_touch(&mut self, victim: PeerId, attacker: PeerId, _my_ts: u32, other_ts: u32, now_ms: i64) {
        if victim == attacker || other_ts == 0 { return; }
        if !self.peers.get(&attacker).map_or(false, |o| o.has_history()) { return; }
        let p = self.peer(victim);
        while p.touches.front().map_or(false, |t| now_ms - t.0 > CLASH_TTL_MS) { p.touches.pop_front(); }
        if p.touches.len() >= TOUCHES_MAX { p.touches.pop_front(); }
        p.touches.push_back((now_ms, attacker, other_ts));
    }

    /// Did `victim` report `attacker`'s stand-in on its body between the
    /// clash it reported (attacker time `clash_ts`, its own label) and the
    /// hit (`attacker_ts` + TOUCH_AFTER_MS)? Then the blow got through.
    fn touched_after(&self, victim: PeerId, attacker: PeerId, clash_ts: u32, attacker_ts: u32, now_ms: i64) -> bool {
        let Some(v) = self.peers.get(&victim) else { return false };
        v.touches.iter().any(|&(at, a, t)| {
            a == attacker && now_ms - at <= CLASH_TTL_MS
                && t as i64 >= clash_ts as i64 - TOUCH_BEFORE_CLASH_MS
                && t as i64 <= attacker_ts as i64 + TOUCH_AFTER_MS
        })
    }

    /// Server adjudication of one clash report: the reporter's blade at its
    /// own time and the other's blade at the (clamped) displayed time, both
    /// swept over ±1 frame, must come within the clash tolerance.
    /// Returns (verdict, other's time in the other's clock); None = the
    /// streams do not cover the instants yet (judge again later — a clash
    /// report can overtake the pose stream it is about).
    fn judge_clash(&self, reporter: PeerId, c: &Clash, now_ms: i64) -> (Option<bool>, u32) {
        let (v, t, _) = self.judge_clash_x(reporter, c, now_ms);
        (v, t)
    }

    /// `judge_clash` plus whether both blades were exact (streamed) geometry.
    /// The REPORTER's own stream must also be physically plausible at its
    /// clash time: blade length and grip for its weapon class,
    /// hands within arm's reach, and a blade tip no faster than
    /// CLASH_BLADE_SPEED_MAX — a blade snapped onto the attacker's to fake a
    /// parry fails it.
    fn judge_clash_x(&self, reporter: PeerId, c: &Clash, now_ms: i64) -> (Option<bool>, u32, bool) {
        let other_t = match self.predict_view(reporter, c.other, c.my_ts, now_ms) {
            Some(pv) => pv.clamp(c.other_ts),
            None => return (None, c.other_ts, false),
        };
        let (Some(r), Some(o)) = (self.peers.get(&reporter), self.peers.get(&c.other)) else {
            return (None, other_t, false);
        };
        // Exact coverage first; once the data had every chance to arrive,
        // accept the newest sample (a lost tail is not the reporter's fault).
        let fr = r.frame().max(o.frame());
        let lead = if now_ms - c.at > 2 * CLASH_JUDGE_AFTER_MS { ATTACKER_LEAD_MS } else { 2 * fr };
        let blades = |h: &BladeView<'_>, t: u32| -> Option<(Blade, Blade, bool)> {
            let (b0, e0) = h.blade_at(ts_sub(t, fr), lead)?;
            let (b1, e1) = h.blade_at(t.saturating_add(fr as u32), lead)?;
            Some((b0, b1, e0 && e1))
        };
        let mut covered = false;
        let mut exact_covered = false;
        for rr in [&r.blade, &r.offhand] {
            let rv = BladeView { hist: r, blade: rr };
            let Some((ra, rb, re)) = blades(&rv, c.my_ts) else { continue };
            for (offhand,oo) in [false,true].into_iter().zip([&o.blade, &o.offhand]) {
                let ov = BladeView { hist: o, blade: oo };
                let shown=if o.pose_v2==Some(true) || !o.native_frames.is_empty() || o.context.is_some() {
                    self.shown_blade(reporter,c.other,o,ts_sub(other_t,fr),offhand,now_ms)
                        .zip(self.shown_blade(reporter,c.other,o,other_t.saturating_add(fr as u32),offhand,now_ms))
                        .map(|((a,ea),(b,eb))|(a,b,ea && eb))
                } else {blades(&ov,other_t)};
                let Some((oa, ob, oe)) = shown else { continue };
                // A missing main-hand stream must not substitute its legacy
                // estimated weapon ahead of a real offhand's exact geometry.
                if (!re && r.offhand.newest().is_some()) || (!oe && o.offhand.newest().is_some()) { continue; }
                covered = true;
                exact_covered |= re && oe;
                if self.reach_violation_blade(reporter, &rv, c.my_ts).is_some() || blade_too_fast(r, c.my_ts, fr)
                    || (re && blade_view_unholdable(reporter, &rv, c.my_ts))
                    || (!re && weapon_out_of_reach(reporter, r, c.my_ts)) { continue; }
                let tol = if re && oe { CLASH_TOL + 2.0 * BLADE_RADIUS } else { CLASH_TOL_FALLBACK };
                if swept_blades_dist(&ra, &rb, &oa, &ob) <= tol {
                    return (Some(true), other_t, re && oe);
                }
            }
        }
        (if covered { Some(false) } else { None }, other_t, exact_covered)
    }

    fn note_invalid_clash(&mut self, reporter: PeerId, now_ms: i64) {
        cheat::bump(reporter, CheatKind::ParryInvalid);
        let p = self.peer(reporter);
        p.clash_invalid.push_back(now_ms);
        while p.clash_invalid.front().map_or(false, |t| now_ms - t > DISTRUST_WINDOW_MS) { p.clash_invalid.pop_front(); }
        if p.clash_invalid.len() >= DISTRUST_INVALID {
            p.distrust_until = now_ms + DISTRUST_MS;
            // Pending reports of a distrusted reporter no longer count either.
            for c in p.clashes.iter_mut() { if c.valid.is_none() { c.valid = Some(false); } }
        }
    }

    /// Did a SERVER-VALIDATED clash between `victim` and `attacker` happen
    /// around attacker_ts (attacker clock)? Either side may report: the
    /// victim reports the attacker's ts it displayed (clamped to the server's
    /// prediction), the attacker reports its own ts.
    pub fn parried(&mut self, victim: PeerId, attacker: PeerId, attacker_ts: u32, now_ms: i64) -> bool {
        if attacker_ts == 0 { return false; }
        // The victim's reports only (see TOUCH_AFTER_MS). The
        // attacker's own clash reports still drive the glint (`judge_due`).
        for (reporter, other) in [(victim, attacker)] {
            let n = self.peers.get(&reporter).map_or(0, |p| p.clashes.len());
            for i in 0..n {
                let c = &self.peers[&reporter].clashes[i];
                if c.other != other || now_ms - c.at > CLASH_TTL_MS || c.valid == Some(false) { continue; }
                // Time in the attacker's clock: the victim's report names the
                // attacker's displayed ts, the attacker's report its own.
                let raw_t = if reporter == victim { c.other_ts } else { c.my_ts };
                if (raw_t as i64 - attacker_ts as i64).abs() > CLASH_WINDOW_MS + 100 { continue; }
                let (ok, other_t, exact) = self.judge_clash_x(reporter, c, now_ms);
                // A victim that streams its blade cancels a hit only on exact
                // (streamed) geometry on both sides: the estimated-blade
                // tolerance is far too wide to trust the side that benefits.
                // (v1 victims, no blade stream, keep the estimated fallback,
                // with their weapon actor held to reach and teleport checks.)
                let v2 = self.peers.get(&reporter).map_or(false, |p| p.blade.newest().is_some() || p.offhand.newest().is_some());
                let ok = ok.map(|v| (v, exact));
                let ok = match ok {
                    Some((true, false)) if reporter == victim && v2 => continue,
                    o => o.map(|x| x.0),
                };
                let t = if reporter == victim { other_t } else { c.my_ts };
                let first = c.valid.is_none();
                let Some(ok) = ok else { continue };
                if let Some(p) = self.peers.get_mut(&reporter) {
                    p.clashes[i].valid = Some(ok);
                }
                if !ok {
                    if first { self.note_invalid_clash(reporter, now_ms); }
                    continue;
                }
                let dt = t as i64 - attacker_ts as i64;
                let after = if reporter == victim { CLASH_AFTER_MS } else { CLASH_AFTER_OWN_MS };
                if dt >= -CLASH_WINDOW_MS && dt <= after {
                    // The victim's screen showed the blade reach its body after
                    // the clash: a bind / glance that landed, not a parry.
                    let clash_label = self.peers[&reporter].clashes[i].other_ts;
                    if self.touched_after(victim, attacker, clash_label, attacker_ts, now_ms) { continue; }
                    return true;
                }
            }
        }
        false
    }

    /// Judge every clash report older than CLASH_JUDGE_AFTER_MS that has not
    /// been judged yet. Returns the NEWLY validated ones as (reporter, other),
    /// at most one per unordered pair per call, so the server can confirm the
    /// clash to both players (glint / deflection feedback).
    pub fn judge_due(&mut self, now_ms: i64) -> Vec<(PeerId, PeerId)> {
        let mut out: Vec<(PeerId, PeerId)> = Vec::new();
        let ids: Vec<PeerId> = self.peers.keys().copied().collect();
        for reporter in ids {
            let n = self.peers.get(&reporter).map_or(0, |p| p.clashes.len());
            for i in 0..n {
                let c = &self.peers[&reporter].clashes[i];
                if c.notified || c.valid == Some(false) || now_ms - c.at < CLASH_JUDGE_AFTER_MS { continue; }
                let (v, _) = match c.valid {
                    Some(true) => (Some(true), 0),
                    _ => self.judge_clash(reporter, c, now_ms),
                };
                let other = c.other;
                let first = c.valid.is_none();
                let Some(ok) = v else { continue };
                if let Some(p) = self.peers.get_mut(&reporter) {
                    p.clashes[i].valid = Some(ok);
                    p.clashes[i].notified = ok;
                }
                if !ok {
                    if first { self.note_invalid_clash(reporter, now_ms); }
                    continue;
                }
                let dup = out.iter().any(|&(a, b)| (a, b) == (reporter, other) || (a, b) == (other, reporter));
                if !dup { out.push((reporter, other)); }
            }
        }
        // The other side's report of the same clash needs no second glint.
        for &(a, b) in &out {
            for (r, o) in [(a, b), (b, a)] {
                if let Some(p) = self.peers.get_mut(&r) {
                    for c in p.clashes.iter_mut().filter(|c| c.other == o && now_ms - c.at <= 300) {
                        if c.valid != Some(false) { c.notified = true; }
                    }
                }
            }
        }
        out
    }

    /// Defender grace for a hit on `victim` by `attacker`: how long the
    /// victim's clash report about it can take to arrive after the claim
    /// (victim RTT + its display delay of the attacker + 2·jitter + IPC).
    pub fn grace_ms(&self, victim: PeerId, attacker: PeerId, now_ms: i64) -> i64 {
        let (Some(v), Some(a)) = (self.peers.get(&victim), self.peers.get(&attacker)) else {
            return GRACE_DEFAULT_MS;
        };
        let Some(rtt) = v.clock.rtt(now_ms) else { return GRACE_DEFAULT_MS };
        let (ja, jv) = (a.clock.jitter_p90(), v.clock.jitter_p90());
        let jpath = (ja * ja + jv * jv).sqrt();
        let interp = self.view_delay(victim, attacker, a, jpath, now_ms);
        let g = rtt + interp + 2.0 * jpath + IPC_MS as f32;
        (g.round() as i64).clamp(GRACE_MIN_MS, GRACE_MAX_MS)
    }

    /// Early release of a held hit: did the victim's blade come anywhere near
    /// the attacker's blade while the victim's screen showed this attack?
    /// The victim displays attacker time `ats` at victim time
    /// `see = ats + off_a − off_v + rtt_v + interp + frame` (predict_view
    /// solved for the viewer). Once the victim's blade stream covers
    /// see ± (tol + 2 frames), every victim blade there is compared with
    /// every attacker blade over [ats − tol − frame, ats + frame]: if none
    /// came within PARRY_RELEASE_UU, no parry can have happened on the
    /// victim's screen and the hit needs no further wait.
    /// None = not covered yet; Some(true) = clear; Some(false) = blades close.
    pub fn parry_window_clear(&self, victim: PeerId, attacker: PeerId, ats: u32, now_ms: i64) -> Option<bool> {
        let (v, a) = (self.peers.get(&victim)?, self.peers.get(&attacker)?);
        // The early-release optimisation scans the main weapon only. With
        // either offhand present, retain the normal bounded defender grace;
        // both-hand clash validation then decides it using exact geometry.
        if v.offhand.newest().is_some() || a.offhand.newest().is_some() { return None; }
        let (off_a, off_v) = (a.clock.offset()?, v.clock.offset()?);
        let rtt = v.clock.rtt(now_ms)?;
        let (ja, jv) = (a.clock.jitter_p90(), v.clock.jitter_p90());
        let jpath = (ja * ja + jv * jv).sqrt();
        let interp = self.view_delay(victim, attacker, a, jpath, now_ms);
        let see = ats as i64 + off_a - off_v + rtt.round() as i64 + interp.round() as i64 + FRAME_MS;
        let tol = ((2.0 * jpath).round() as i64 + FRAME_MS).max(TOL_FLOOR_MS);
        let fr = a.frame().max(v.frame());
        let (t0, t1) = (see - tol - 2 * fr, see + tol + 2 * fr);
        if (v.blade.newest().or(v.weapon.newest())? as i64) < t1 { return None; }
        let mut vbs = Vec::new();
        let mut exact = true;
        let mut t = t0;
        while t <= t1 {
            let (b, e) = v.blade_at(t.max(1) as u32, 0)?;
            exact &= e;
            vbs.push(b);
            t += 4;
        }
        let mut abs = Vec::new();
        let mut s = ats as i64 - tol - fr;
        while s <= ats as i64 + fr {
            let (b, e) = a.blade_at(s.max(1) as u32, ATTACKER_LEAD_MS)?;
            exact &= e;
            abs.push(b);
            s += 4;
        }
        let lim = if exact { PARRY_RELEASE_UU } else { CLASH_TOL_FALLBACK + 20.0 };
        for vb in &vbs {
            for ab in &abs {
                if seg_seg(vb.base, vb.tip, ab.base, ab.tip).0 <= lim { return Some(false); }
            }
        }
        Some(true)
    }

    /// Hit inspector / diagnostics: `reporter`'s recent clash reports as
    /// (server ms, other, my_ts, other_ts, verdict).
    pub fn clash_log(&self, reporter: PeerId) -> Vec<(i64, PeerId, u32, u32, Option<bool>)> {
        self.peers.get(&reporter)
            .map(|p| p.clashes.iter().map(|c| (c.at, c.other, c.my_ts, c.other_ts, c.valid)).collect())
            .unwrap_or_default()
    }

    /// Sender clock rate error of `id` if it looks like a speedhack.
    pub fn clock_rate_suspect(&self, id: PeerId) -> Option<f32> {
        self.peers.get(&id).and_then(|p| p.clock.rate_suspect())
    }

    /// A dead attacker's hit still counts if it happened (attacker clock)
    /// no later than TRADE_WINDOW_MS after its reported death.
    pub fn trade_ok(&self, attacker: PeerId, attacker_ts: u32, now_ms: i64) -> bool {
        if attacker_ts == 0 { return false; }
        match self.peers.get(&attacker).and_then(|p| p.death) {
            Some((at, death_ts)) => {
                now_ms - at <= TRADE_SERVER_TTL.as_millis() as i64
                    && rel(attacker_ts, death_ts) <= TRADE_WINDOW_MS
                    && rel(death_ts, attacker_ts) <= HISTORY_MS as i64
            }
            None => false,
        }
    }

    /// Approach speed (uu/s) of the attacker's striking part before `at`:
    /// max over SPEED_FRAMES frames. Blade stream: the blade point nearest the
    /// contact (or the streamed tip velocity); else weapon actor / hands × lever.
    fn contact_speed(&self, a: &BladeView<'_>, at: u32, contact: V3, hint: Option<(f32, f64)>) -> Option<f32> {
        let fr = a.frame();
        let mut best: Option<f32> = None;
        let mut put = |v: f32| if v.is_finite() { best = Some(best.map_or(v, |b: f32| b.max(v))) };
        if let Some((s, tc)) = hint {
            // Blade stream: the contacting point's velocity AT the contact instant
            // (the game uses the weapon's physics velocity at the collision; a
            // window max over-reads a blade decelerating into its target ~2×).
            // Central difference over ±1 ms of the arc-interpolated stream, at
            // the touch and 4 ms before it.
            let mut inst: Option<f32> = None;
            for t in [tc] {
                if let (Some(b1), Some(b0)) = (a.blade.sample_f(t + 1.0, ATTACKER_LEAD_MS), a.blade.sample_f(t - 1.0, ATTACKER_LEAD_MS)) {
                    let v = len(sub(lerp3(b1.base, b1.tip, s), lerp3(b0.base, b0.tip, s))) * 500.0;
                    if v.is_finite() { inst = Some(inst.map_or(v, |x: f32| x.max(v))); }
                }
            }
            if let Some(v) = inst { return Some(v.min(MAX_STRIKE_SPEED)); }
        }
        if let Some(b) = a.blade.sample(at, ATTACKER_LEAD_MS) {
            // Speed of the blade POINT that made contact (the streamed tip
            // velocity would overstate a strike near the hilt).
            // Blade parameter of the contact: from the sweep step that touched
            // (the end-of-frame blade has rotated on; projecting onto it would
            // measure a point nearer the tip).
            let s = hint.map(|h| h.0).unwrap_or_else(|| seg_param(contact, b.base, b.tip));
            for k in 0..SPEED_FRAMES {
                let (t1, t0) = (ts_sub(at, k * fr), ts_sub(at, (k + 1) * fr));
                if let (Some(b1), Some(b0)) = (a.blade.sample(t1, ATTACKER_LEAD_MS), a.blade.sample(t0, 0)) {
                    let p1 = lerp3(b1.base, b1.tip, s);
                    let p0 = lerp3(b0.base, b0.tip, s);
                    put(len(sub(p1, p0)) * 1000.0 / fr as f32);
                }
            }
            return best.map(|v| v.min(MAX_STRIKE_SPEED));
        }
        for k in 0..SPEED_FRAMES {
            let (t1, t0) = (ts_sub(at, k * fr), ts_sub(at, (k + 1) * fr));
            if let (Some(w1), Some(w0)) = (a.weapon.sample(t1, ATTACKER_LEAD_MS), a.weapon.sample(t0, 0)) {
                put(len(sub(w1, w0)) * 1000.0 / fr as f32 * lever(a.pose.sample(t1, ATTACKER_LEAD_MS).as_ref(), w1, contact));
            } else if let (Some(p1), Some(p0)) = (a.pose.sample(t1, ATTACKER_LEAD_MS), a.pose.sample(t0, 0)) {
                for h in [posecodec::HAND_L, posecodec::HAND_R] {
                    if p1.mask & p0.mask & (1 << h) != 0 {
                        put(len(sub(p1.p[h], p0.p[h])) * 1000.0 / fr as f32 * lever(Some(&p1), p1.p[h], contact));
                    }
                }
            }
        }
        best.map(|v| v.min(MAX_STRIKE_SPEED))
    }

    /// Relative impact speed at the contact: attacker striking-point velocity
    /// (blade point at the touch, or the body part) minus the victim's body
    /// velocity at the contact (its nearest capsule point around the view
    /// time). Central differences over ±8 ms of the streams.
    #[allow(clippy::too_many_arguments)]
    fn relative_speed(&self, a: &BladeView<'_>, v: &PeerHist, at: u32, view: u32, c: V3, hint: Option<(f32, f64)>, unarmed: bool) -> Option<f32> {
        let va: V3 = match (hint, unarmed) {
            (Some((s, tc)), false) => {
                let b1 = a.blade.sample_f(tc + 1.0, ATTACKER_LEAD_MS)?;
                let b0 = a.blade.sample_f(tc - 1.0, ATTACKER_LEAD_MS)?;
                mul(sub(lerp3(b1.base, b1.tip, s), lerp3(b0.base, b0.tip, s)), 500.0)
            }
            (_, true) => cap_point_velocity(a, at, c)?,
            // No sweep hint (v1 / hand-derived blade): the same sources as
            // contact_speed — the blade point, else the weapon actor or the
            // hands × LEVER_FALLBACK — as the largest frame-difference vector.
            (None, false) => match self.striking_velocity(a, at, c) {
                Some(v) => v,
                None => cap_point_velocity(a, at, c)?,
            },
        };
        let vv = cap_point_velocity(v, view, c).unwrap_or([0.0; 3]);
        let r = len(sub(va, vv));
        if r.is_finite() { Some(r.min(MAX_STRIKE_SPEED)) } else { None }
    }

    /// Velocity of the attacker's striking point without a sweep hint (see
    /// contact_speed's fallbacks): the largest of the last SPEED_FRAMES frame
    /// differences.
    fn striking_velocity(&self, a: &BladeView<'_>, at: u32, c: V3) -> Option<V3> {
        let fr = a.frame();
        let k_ms = 1000.0 / fr as f32;
        let mut best: Option<V3> = None;
        let mut put = |v: V3| if len(v).is_finite() && best.map_or(true, |b| len(v) > len(b)) { best = Some(v) };
        let blade = a.blade.sample(at, ATTACKER_LEAD_MS);
        let s = blade.map(|b| seg_param(c, b.base, b.tip));
        for k in 0..SPEED_FRAMES {
            let (t1, t0) = (ts_sub(at, k * fr), ts_sub(at, (k + 1) * fr));
            if let Some(s) = s {
                if let (Some(b1), Some(b0)) = (a.blade.sample(t1, ATTACKER_LEAD_MS), a.blade.sample(t0, 0)) {
                    put(mul(sub(lerp3(b1.base, b1.tip, s), lerp3(b0.base, b0.tip, s)), k_ms));
                }
            } else if let (Some(w1), Some(w0)) = (a.weapon.sample(t1, ATTACKER_LEAD_MS), a.weapon.sample(t0, 0)) {
                put(mul(sub(w1, w0), k_ms * lever(a.pose.sample(t1, ATTACKER_LEAD_MS).as_ref(), w1, c)));
            } else if let (Some(p1), Some(p0)) = (a.pose.sample(t1, ATTACKER_LEAD_MS), a.pose.sample(t0, 0)) {
                for h in [posecodec::HAND_L, posecodec::HAND_R] {
                    if p1.mask & p0.mask & (1 << h) != 0 {
                        put(mul(sub(p1.p[h], p0.p[h]), k_ms * lever(Some(&p1), p1.p[h], c)));
                    }
                }
            }
        }
        best
    }

    /// Speed (uu/s) of the attacker's body part nearest the contact (unarmed
    /// contacts): that capsule's closest point over SPEED_FRAMES frames.
    /// Peak speed of the striking point over the PEAK_FRAMES frames up to the
    /// contact (frame differences of the streams): the blade point at `s`
    /// (contact parameter; projected when unknown), else the weapon actor or
    /// the hands × lever, or the nearest body capsule when `unarmed`.
    fn peak_speed(&self, a: &BladeView<'_>, at: u32, c: V3, s: Option<f32>, unarmed: bool) -> Option<f32> {
        let fr = a.frame();
        let k_ms = 1000.0 / fr as f32;
        let mut best: Option<f32> = None;
        let mut put = |v: f32| if v.is_finite() { best = Some(best.map_or(v, |b: f32| b.max(v))) };
        if unarmed {
            let cs = a.capsules(at, ATTACKER_LEAD_MS)?;
            let (mut bi, mut bd, mut bu) = (0usize, f32::INFINITY, 0.0f32);
            for (i, k) in cs.iter().enumerate() {
                let d = point_seg(c, k.a, k.b) - k.r;
                if d < bd { bd = d; bi = i; bu = seg_param(c, k.a, k.b); }
            }
            for k in 0..PEAK_FRAMES {
                let (t1, t0) = (ts_sub(at, k * fr), ts_sub(at, (k + 1) * fr));
                if let (Some(c1), Some(c0)) = (a.capsules(t1, ATTACKER_LEAD_MS), a.capsules(t0, 0)) {
                    if bi < c1.n && bi < c0.n {
                        put(len(sub(lerp3(c1.c[bi].a, c1.c[bi].b, bu), lerp3(c0.c[bi].a, c0.c[bi].b, bu))) * k_ms);
                    }
                }
            }
            return best.map(|v| v.min(MAX_STRIKE_SPEED));
        }
        let s = s.or_else(|| a.blade.sample(at, ATTACKER_LEAD_MS).map(|b| seg_param(c, b.base, b.tip)));
        for k in 0..PEAK_FRAMES {
            let (t1, t0) = (ts_sub(at, k * fr), ts_sub(at, (k + 1) * fr));
            if let (Some(s), Some(b1), Some(b0)) = (s, a.blade.sample(t1, ATTACKER_LEAD_MS), a.blade.sample(t0, 0)) {
                put(len(sub(lerp3(b1.base, b1.tip, s), lerp3(b0.base, b0.tip, s))) * k_ms);
            } else if let (Some(w1), Some(w0)) = (a.weapon.sample(t1, ATTACKER_LEAD_MS), a.weapon.sample(t0, 0)) {
                put(len(sub(w1, w0)) * k_ms * lever(a.pose.sample(t1, ATTACKER_LEAD_MS).as_ref(), w1, c));
            }
        }
        best.map(|v| v.min(MAX_STRIKE_SPEED))
    }

    fn body_part_speed(&self, a: &PeerHist, at: u32, c: V3) -> Option<f32> {
        let fr = a.frame();
        let cs = a.capsules(at, ATTACKER_LEAD_MS)?;
        let (mut bi, mut bd, mut bu) = (0usize, f32::INFINITY, 0.0f32);
        for (i, k) in cs.iter().enumerate() {
            let d = point_seg(c, k.a, k.b) - k.r;
            if d < bd { bd = d; bi = i; bu = seg_param(c, k.a, k.b); }
        }
        let mut best: Option<f32> = None;
        for k in 0..SPEED_FRAMES {
            let (t1, t0) = (ts_sub(at, k * fr), ts_sub(at, (k + 1) * fr));
            if let (Some(c1), Some(c0)) = (a.capsules(t1, ATTACKER_LEAD_MS), a.capsules(t0, 0)) {
                if bi < c1.n && bi < c0.n {
                    let p1 = lerp3(c1.c[bi].a, c1.c[bi].b, bu);
                    let p0 = lerp3(c0.c[bi].a, c0.c[bi].b, bu);
                    let v = len(sub(p1, p0)) * 1000.0 / fr as f32;
                    if v.is_finite() { best = Some(best.map_or(v, |b: f32| b.max(v))); }
                }
            }
        }
        best.map(|v| v.min(MAX_STRIKE_SPEED))
    }

    /// Rewind both players and judge the contact (`now_ms`: first arrival).
    ///
    /// Reject reasons start with a stable code (`code: details`), which
    /// HSMPCombat groups by in its `combat_quality` counters.
    pub fn evaluate(&self, attacker: PeerId, hit: &DamageEvent, now_ms: i64) -> Eval {
        self.evaluate_opts(attacker, hit, now_ms, true)
    }

    /// `evaluate`; with `allow_lead` false a claim whose hit time the
    /// attacker's stream does not cover yet gets `Eval::Wait` instead of
    /// being judged against the newest (older) sample.
    pub fn evaluate_opts(&self, attacker: PeerId, hit: &DamageEvent, now_ms: i64, allow_lead: bool) -> Eval {
        let Some(v) = self.peers.get(&hit.target_peer_id).filter(|v| v.has_history()) else {
            return Eval::NoData("no victim history");
        };
        // 1. Timestamps are mandatory once the victim streams.
        if hit.victim_view_ts == 0 || hit.attacker_ts == 0 {
            cheat::bump(attacker, CheatKind::NoTimestamp);
            return Eval::Reject("no_ts: missing timestamps (victim has history)".into());
        }
        let Some(a) = self.peers.get(&attacker).filter(|a| a.has_history()) else {
            return Eval::Reject("no_history: no attacker history".into());
        };
        if let Some(e) = a.clock.rate_suspect() {
            cheat::bump(attacker, CheatKind::ClockRate);
            return Eval::Reject(format!("clock_rate: attacker clock runs {:+.0} % off server time", e * 100.0));
        }
        let at = hit.attacker_ts;
        let an = a.newest_any().unwrap_or(at);
        if rel(at, an) > ATTACKER_LEAD_MS {
            return Eval::Reject(format!("ts_future: attacker ts {} ms ahead of its stream", rel(at, an)));
        }
        if rel(an, at) > HISTORY_MS as i64 {
            return Eval::Reject("ts_old: attacker ts older than history".into());
        }
        // The attacker's ts must agree with the claim's arrival.
        if let Some(off_a) = a.clock.offset() {
            let mapped = at as i64 + off_a;
            let late = now_ms - mapped - hit.age_ms as i64;
            let slack = ARRIVAL_LATE_MS + 2 * a.clock.jitter_bounded(now_ms).round() as i64;
            if mapped > now_ms + ARRIVAL_LEAD_MS || late > slack {
                cheat::bump(attacker, CheatKind::TsInconsistent);
                return Eval::Reject(format!("ts_inconsistent: attacker ts inconsistent with arrival ({} ms)", now_ms - mapped));
            }
        }
        if !allow_lead {
            let cover = if a.blade.newest().is_some() { a.blade.newest() } else { a.newest_any() };
            if cover.map_or(true, |n| rel(at, n) > 0) {
                return Eval::Wait("attacker stream does not cover its hit yet");
            }
        }
        let Some(newest) = v.newest().or(v.newest_any()) else { return Eval::NoData("no victim history") };

        // 2. Server-predicted view time; the client's ts is only a hint.
        let Some(pred) = self.predict_view(attacker, hit.target_peer_id, at, now_ms) else {
            return Eval::Reject("no_clock: no clock map".into());
        };
        let view = pred.clamp(hit.victim_view_ts);
        let arm_hint = if hit.victim_arm_ts != 0 { hit.victim_arm_ts } else { hit.victim_view_ts };
        let arm = pred.clamp(arm_hint);
        let view_clamped = view != hit.victim_view_ts;
        if pred.outlier(hit.victim_view_ts) {
            cheat::bump(attacker, CheatKind::ViewHintOutlier);
        }
        let lead = rel(view, newest);
        if lead > FUTURE_MS {
            return Eval::Reject(format!("future: victim view ts {} ms in the future", lead));
        }
        // Rewind, measured on the server's clock maps (never from the
        // client's age_ms): view lag = how old the displayed victim pose was
        // at the attacker's hit instant; total = how old it is now, when the
        // server applies the decision (view lag + delivery of the claim).
        let (off_a, off_v) = (a.clock.offset().unwrap_or(0), v.clock.offset().unwrap_or(0));
        let view_lag = (at as i64 + off_a - off_v - view as i64).max(0);
        let total = (now_ms - off_v - view as i64).max(0);
        // A victim whose clock runs off (speedhack) is displayed late by everyone
        // (its samples look ever later to their jitter buffers): it forfeits the
        // defender-favouring rewind cap instead of becoming unhittable.
        // A high-ping attacker honestly sees the victim RTT + buffer late: the cap
        // covers that path plus the view-time tolerance and REWIND_SLACK_MS, up to
        // REWIND_CEILING_MS.
        let cap = if v.clock.rate_suspect().is_some() { HISTORY_MS as i64 } else {
            self.max_rewind().max((pred.honest_lag + pred.tol + REWIND_SLACK_MS).min(REWIND_CEILING_MS))
        };
        // The same for a victim that delays its own stream (lag switch /
        // timestamp noise): the cap only charges the share of the
        // view lag the network and the attacker explain, not the display
        // delay the victim's excess jitter added for every receiver.
        let excess = pred.victim_excess;
        if pred.unexplained > LAG_SWITCH_FLAG_MS {
            cheat::bump(hit.target_peer_id, CheatKind::LagSwitch);
        }
        if view_lag - excess > cap {
            return Eval::Reject(format!("rewind_cap: rewind {} ms > cap {} ms (favour defender)", view_lag - excess, cap));
        }
        if total - excess > cap + DELIVERY_CREDIT_MS {
            return Eval::Reject(format!("rewind_cap: judged {} ms after the victim pose > cap {} + {} ms delivery (favour defender)",
                total - excess, cap, DELIVERY_CREDIT_MS));
        }
        let rewind = view_lag;

        // 3. Victim body at the rewound time (body at view, arms at arm).
        let c = hit.location;
        // The victim body the attacker was SENT (decimated relay), not the
        // full-rate history it never saw.
        let tgt = hit.target_peer_id;
        let vcaps = self.shown_capsules(attacker, tgt, v, view, FUTURE_MS, now_ms);
        let vcaps_arm = if arm != view { self.shown_capsules(attacker, tgt, v, arm, FUTURE_MS, now_ms) } else { None };
        let body = match (&vcaps, &vcaps_arm) {
            (Some(b), Some(x)) => caps_dist(b, c).min(caps_dist(x, c)),
            (Some(b), None) | (None, Some(b)) => caps_dist(b, c),
            (None, None) if v.pose_v2==Some(true) || !v.native_frames.is_empty() || v.context.is_some() => {
                return Eval::Reject("no_cover: delivered victim body history does not cover view ts".into());
            }
            (None, None) => match v.root.sample(view, FUTURE_MS) {
                Some(r) => {
                    let d = len(sub(c, r));
                    if d > ROOT_TOL {
                        return Eval::Reject(format!("body_miss: contact {:.0} uu from rewound victim root", d));
                    }
                    d - ROOT_TOL + BODY_TOL // normalised into the body scale for logging
                }
                None => return Eval::Reject("no_cover: victim history does not cover view ts".into()),
            },
        };
        // v1 clients (no blade stream) show their stand-ins with PhysicsHandles
        // that miss the sender's pose by 6–78 uu: their contacts get that slack.
        let body_tol = if a.blade.newest().is_some() { BODY_TOL } else { BODY_TOL_V1 };
        if body > body_tol {
            return Eval::Reject(format!("body_miss: contact {:.0} uu from rewound victim body (rewind {} ms)", body, rewind));
        }

        // 4. Attacker's striking part at its own hit time.
        let fr = a.frame();
        let distance = |ring: &Ring<Blade>| blade_sweep(ring, at as f64 - fr as f64, at as f64, ATTACKER_LEAD_MS)
            .map_or(f32::INFINITY, |bs| bs.iter().map(|b| point_seg(c, b.base, b.tip)).fold(f32::INFINITY, f32::min));
        let native = hit.flags & crate::validate::damage::FLAG_COMPLEX != 0;
        let body_contact = native && (hit.flags & crate::validate::damage::FLAG_WEAPON == 0
            || hit.dism_blunt & (crate::validate::damage::SOURCE_FIST | crate::validate::damage::SOURCE_FEET) != 0);
        let source_hand = if native { hit.dism_blunt & (crate::validate::damage::SOURCE_LEFT | crate::validate::damage::SOURCE_RIGHT) } else { 0 };
        let blade = if source_hand == crate::validate::damage::SOURCE_LEFT { &a.offhand }
            else if source_hand == crate::validate::damage::SOURCE_RIGHT { &a.blade }
            else if distance(&a.offhand) < distance(&a.blade) { &a.offhand } else { &a.blade };
        let a = BladeView { hist: a, blade };
        let component=((hit.dism_blunt >> crate::validate::damage::SOURCE_COMPONENT_SHIFT)&15) as u8;
        let (mut hit_box, mut proxy_box_error) = (None, None);
        if native && component!=0 {
            let class=hit.source_class.as_str().unwrap_or("");
            if body_contact {
                let expected=if hit.dism_blunt & crate::validate::damage::SOURCE_FEET!=0 {"Weapon_Feet_C"} else {"Weapon_Fists_C"};
                if class!=expected {return Eval::Reject("source_class: native body striker class mismatch".into());}
                if hit.dism_blunt & crate::validate::damage::HIT_BOX_MASK!=0 {return Eval::Reject("hit_box: native body cutting Box history unavailable".into());}
            } else {
                if !registered_native_weapon_class(class) {return Eval::Reject("source_class: unregistered native weapon class".into());}
                let Some(shape)=a.shapes().sample(at,ATTACKER_LEAD_MS) else {
                    return if allow_lead {Eval::Reject("no_cover: original source class unavailable".into())} else {Eval::Wait("original source class not covered yet")};
                };
                if shape.module(component).is_none_or(|b|b.class_hash==0 || b.class_hash!=posecodec::v2::class_hash(class)) {
                    return Eval::Reject("source_class: original hand/component class mismatch".into());
                }
                if shape.module(component).is_some_and(|b|b.child_of!=0) {
                    return Eval::Reject("source_role: cutting child cannot be a striking module".into());
                }
                let ordinal=((hit.dism_blunt >> crate::validate::damage::HIT_BOX_SHIFT)&15) as u8;
                if ordinal!=0 {
                    let Some(box_shape)=shape.module(ordinal) else {return Eval::Reject("hit_box: original component absent from history".into());};
                    let Some(native_scale)=box_shape.native_scale else {return Eval::Reject("hit_box: original component is not native Box".into());};
                    if box_shape.child_of!=0 && shape.module(box_shape.child_of).is_none_or(|p|p.child_of!=0) {
                        return Eval::Reject("hit_box: original cutting parent absent from history".into());
                    }
                    let Some(bone)=posecodec::v2::BONES.iter().position(|b|b.eq_ignore_ascii_case(hit.bone_str())) else {return Eval::Reject("hit_box: unknown victim bone".into());};
                    let vf=match self.shown_bone_frames(attacker,tgt,v,if (9..=16).contains(&bone) {arm} else {view},FUTURE_MS,now_ms) {
                        Ok(vf)=>vf,
                        Err(why)=>return if allow_lead {Eval::Reject(format!("no_cover: historical victim bone frame absent ({why})"))} else {Eval::Wait("historical victim bone frame not covered yet")},
                    };
                    let h=&hit.hit_box_frame;
                    let norm=h[3..7].iter().map(|v|v*v).sum::<f32>();
                    if !h.iter().all(|v|v.is_finite()) || !(0.99..=1.01).contains(&norm) {
                        return Eval::Reject("hit_box: cutting geometry differs from original hand/component history".into());
                    }
                    let center=add(vf.p[bone],posecodec::v2::qrot(vf.q[bone],[h[0],h[1],h[2]]));
                    let expected_center=shape.point(box_shape.p);
                    let rotation=posecodec::v2::qmul(vf.q[bone],[h[3],h[4],h[5],h[6]]);
                    let expected_rotation=posecodec::v2::qmul(shape.q,box_shape.q);
                    let expected_norm=expected_rotation.iter().map(|v|v*v).sum::<f32>();
                    let dot=rotation.iter().zip(expected_rotation).map(|(a,b)|a*b).sum::<f32>().abs()/(norm*expected_norm).sqrt();
                    let center_error=len(sub(center,expected_center));
                    let scale_error:V3=std::array::from_fn(|i|(h[7+i]-native_scale[i]).abs());
                    let extent_error:V3=std::array::from_fn(|i|(h[10+i]*h[7+i]-box_shape.half[i]).abs());
                    // The Box itself must be the attacker's own native Box (identity, scale, extent).
                    if (0..3).any(|i|scale_error[i]>1.0/1024.0 || extent_error[i]>0.2) {
                        return Eval::Reject(format!("hit_box: cutting geometry differs from original hand/component history; center_cm={center_error:.3} rotation_dot={dot:.6} scale_delta={scale_error:?} extent_cm={extent_error:?} attacker_ts={at} victim_ts={} component={component} box={ordinal}",if (9..=16).contains(&bone) {arm} else {view}));
                    }
                    // Where it was: the attacker's authenticated Box in the world. The contact
                    // need NOT lie on it: ModularWeaponBP writes `Hit Box Collision` only when the
                    // struck module has a Box child and never resets it, so a haft or grip contact
                    // carries the head's Box from an earlier module (native-cutting-box proof).
                    // The contact itself is checked against the striking module below
                    // (module_miss); live, 35 of 77 honest polearm claims landed 48-178 cm from
                    // their retained Box and were refused when this was a rejection.
                    let en=expected_norm.sqrt();
                    let eq=[expected_rotation[0]/en,expected_rotation[1]/en,expected_rotation[2]/en,expected_rotation[3]/en];
                    let local=posecodec::v2::qrot(posecodec::v2::qconj(eq),sub(c,expected_center));
                    let outside=len(std::array::from_fn(|i|(local[i].abs()-box_shape.half[i]).max(0.0)));
                    if outside>BODY_TOL && crate::validate::rate::log_ok("hit_box_retained") {
                        tracing::info!(attacker, outside_cm = outside, component, ordinal, "native HitBox retained from another module: contact off the cutting Box");
                    }
                    // The replay frame relative to the victim's real bone, not the stand-in's
                    // (whose rotation error, up to 90 deg at the wrist, it would carry).
                    let bq=posecodec::v2::qconj(vf.q[bone]);
                    let rp=posecodec::v2::qrot(bq,sub(expected_center,vf.p[bone]));
                    let rq=posecodec::v2::qmul(bq,eq);
                    hit_box=Some([rp[0],rp[1],rp[2],rq[0],rq[1],rq[2],rq[3],h[7],h[8],h[9],h[10],h[11],h[12]]);
                    proxy_box_error=Some((center_error,dot));
                }
            }
        }
        if !body_contact && source_hand != 0 && a.is_v2() && a.blade.sample(at, ATTACKER_LEAD_MS).is_none() {
            // A named left-hand claim cannot fall back to the legacy weapon
            // actor (which is normally the right hand). Let an overtaken pose
            // arrive, then refuse a claim whose own hand is still uncovered.
            return if allow_lead { Eval::Reject("no_cover: source hand weapon history does not cover hit".into()) }
                else { Eval::Wait("source hand weapon history does not cover hit yet") };
        }
        if let Some(r) = self.reach_violation_parts(attacker, &a, at, !body_contact) {
            cheat::bump(attacker, CheatKind::ReachImplausible);
            return Eval::Reject(r);
        }
        let weapon_dist: f32;
        let mut attack_from: Option<V3> = None;
        let mut swept = false;
        let mut sweep: Option<(Blade, Blade)> = None;
        let mut unarmed = false;
        let mut s_contact: Option<(f32, f64)> = None;
        let mut module_contact: Option<(u8, u8, V3, f64)> = None;
        let mut striker_contact: Option<(u8,V3,f64)> = None;
        let component = if native { ((hit.dism_blunt >> 21) & 15) as u8 } else { 0 };
        let shape_hit = if body_contact { None } else { a.shape_contact(c,at as f64-fr as f64,at as f64,component) };
        if !body_contact && component != 0 {
            match shape_hit {
                None => return if allow_lead { Eval::Reject("no_cover: striking module shape unavailable".into()) }
                    else { Eval::Wait("striking module shape not covered yet") },
                Some((d,_,_,_,_)) if d > SWEEP_TOL => return Eval::Reject(format!("module_miss: contact {:.0} uu from striking module {}",d,component)),
                _ => {}
            }
        }
        if body_contact {
            let named=hit.dism_blunt & (crate::validate::damage::SOURCE_FIST|crate::validate::damage::SOURCE_FEET)!=0;
            // Root/held-weapon packets can arrive before this source's body
            // shape packet. Do not permanently reject against a held old shape
            // while the reliable claim still has time to wait for its history.
            if named && !allow_lead && a.strikers.newest().is_none_or(|ts|rel(at,ts)>0) {
                return Eval::Wait("exact native body striker hit time not covered yet");
            }
            let extended=a.strikers.sample(at,ATTACKER_LEAD_MS).or_else(||a.strikers.q.back().map(|x|x.1)).is_some_and(|s|s.extended);
            if named && !extended {
                return if allow_lead {Eval::Reject("no_cover: exact native body striker history unavailable".into())} else {Eval::Wait("exact native body striker history not covered yet")};
            }
            let d=if named && extended {
                let side=match source_hand {crate::validate::damage::SOURCE_RIGHT=>1,crate::validate::damage::SOURCE_LEFT=>2,_=>0};
                if side==0 || component==0 {return Eval::Reject("body_striker_identity: missing native side/component".into());}
                let part=side+if hit.dism_blunt & crate::validate::damage::SOURCE_FEET!=0 {2} else {0};
                let Some((d,p,t))=a.striker_contact(c,at as f64-fr as f64,at as f64,part,component) else {
                    return if allow_lead {Eval::Reject("no_cover: named body striker unavailable".into())} else {Eval::Wait("named body striker not covered yet")};
                };
                if d>UNARMED_TOL {return Eval::Reject(format!("body_striker_miss: contact {:.0} uu from native part {} component {}",d,part,component));}
                striker_contact=Some((part,p,t)); swept=true;
                d
            } else {
                let d=a.capsules(at,ATTACKER_LEAD_MS).map_or(f32::INFINITY,|cs|caps_dist(&cs,c));
                if d>UNARMED_TOL {return Eval::Reject(format!("body_strike_miss: contact {:.0} uu from attacker's body",d));}
                d
            };
            unarmed = true;
            weapon_dist = d;
        } else if let Some((d,p,t,id,weapon_id)) = shape_hit.filter(|x| x.0 <= SWEEP_TOL) {
            // Native Head/Guard/Grip contacts use the actual component's
            // bounded envelope. A head offset from the shaft is not a miss.
            // Victim contact was already independently checked above.
            swept = true;
            weapon_dist = d;
            module_contact = Some((weapon_id,id,p,t));
            attack_from = a.shapes().sample_f(at as f64-fr as f64,ATTACKER_LEAD_MS)
                .filter(|s|s.weapon_id==weapon_id).and_then(|s|s.module_point(id,p));
        } else if let Some(bs) = a.blade_sweep((at as f64) - fr as f64, at as f64, ATTACKER_LEAD_MS) {
            // Swept blade (arc-interpolated between stream samples) against
            // the rewound capsules.
            swept = true;
            let ds: Vec<f32> = bs.iter().map(|b| point_seg(c, b.base, b.tip)).collect();
            let dc = ds.iter().copied().fold(f32::INFINITY, f32::min);
            // The touch is the FIRST sweep step that reaches the contact (the
            // blade may linger there after the impact): its time is the impact.
            let sc = ds.iter().position(|d| *d <= dc + 3.0).unwrap_or(0);
            let mut db = f32::INFINITY;
            for cs in [&vcaps, &vcaps_arm].into_iter().flatten() {
                for b in &bs { db = db.min(blade_caps_dist(cs, b)); }
            }
            if dc > SWEEP_TOL + BLADE_RADIUS {
                // Not the blade: a fist, elbow, knee, foot or shoulder (clinch,
                // pommel-less punches) — the contact must touch the attacker's
                // OWN body capsules at its hit time instead.
                let body_d = a.capsules(at, ATTACKER_LEAD_MS).map_or(f32::INFINITY, |cs| caps_dist(&cs, c));
                if body_d > UNARMED_TOL {
                    return Eval::Reject(format!("blade_miss: contact {:.0} uu from swept attacker blade, {:.0} uu from its body", dc, body_d));
                }
                unarmed = true;
            } else if vcaps.is_some() && db > BODY_TOL + BLADE_RADIUS {
                return Eval::Reject(format!("blade_miss: swept blade misses rewound victim capsules by {:.0} uu", db));
            }
            weapon_dist = dc;
            // Path of the contacting blade point over the last frame.
            let bc = bs[sc];
            let s = seg_param(c, bc.base, bc.tip);
            let n = (bs.len().max(2) - 1) as f64;
            s_contact = Some((s, at as f64 - fr as f64 + fr as f64 * sc as f64 / n));
            let b0 = bs[0];
            attack_from = Some(lerp3(b0.base, b0.tip, s));
            sweep = Some((b0, *bs.last().unwrap_or(&b0)));
        } else {
            let wpos = a.weapon.sample(at, ATTACKER_LEAD_MS);
            let apose = a.pose.sample(at, ATTACKER_LEAD_MS);
            let mut best_ok = false;
            let mut best = f32::INFINITY;
            let mut checked = false;
            if let Some(w) = wpos {
                checked = true;
                let d = len(sub(c, w));
                best = best.min(d);
                best_ok |= d <= WEAPON_REACH;
                attack_from = Some(w);
            }
            if let Some(p) = &apose {
                checked = true;
                for h in [posecodec::HAND_L, posecodec::HAND_R] {
                    if p.mask & (1 << h) != 0 {
                        let d = len(sub(c, p.p[h]));
                        best = best.min(d);
                        best_ok |= d <= HAND_REACH;
                    }
                }
                let d = caps_dist(&pose_capsules(p), c);
                best_ok |= d <= LIMB_REACH;
            }
            if !checked {
                if let Some(r) = a.root.sample(at, ATTACKER_LEAD_MS) {
                    checked = true;
                    let d = len(sub(c, r));
                    best = best.min(d);
                    best_ok |= d <= ROOT_REACH;
                }
            }
            if !checked {
                return Eval::Reject("no_cover: attacker history does not cover its ts".into());
            }
            if !best_ok {
                return Eval::Reject(format!("reach: contact {:.0} uu from rewound attacker weapon", best));
            }
            weapon_dist = best;
        }

        // 5. Geometric block / parry plausibility: victim's blade at arm time.
        let mut parry_possible = false;
        let mut parry_d = [f32::INFINITY; 3];
        for offhand in [false,true] {
          if let Some((vb, exact)) = self.shown_blade(attacker,tgt,v,arm,offhand,now_ms) {
            // The parrying part near the contact: the outer half of a
            // streamed blade, else the weapon actor (the hand is always near
            // the body and proves nothing).
            let (pa, pb) = if exact {
                (lerp3(vb.base, vb.tip, 0.5), vb.tip)
            } else {
                let w = v.weapon.sample(arm, FUTURE_MS).unwrap_or(vb.tip);
                (w, vb.tip)
            };
            parry_d[0] = parry_d[0].min(point_seg(c, pa, pb));
            if parry_d[0] <= PARRY_NEAR_CONTACT_UE {
                parry_possible = true;
            }
            if let Some((b0, b1)) = &sweep {
                let d = sweep_min(b0, b1, |b| seg_seg(b.base, b.tip, pa, pb).0).0;
                parry_d[1] = parry_d[1].min(d);
                if d <= PARRY_PLAUSIBLE_UE { parry_possible = true; }
            }
            if let Some(from) = attack_from {
                // The blade beyond the hand (the hand itself can't parry).
                let (d, _) = seg_seg(from, c, pa, pb);
                parry_d[2] = parry_d[2].min(d);
                if d <= PARRY_PLAUSIBLE_UE { parry_possible = true; }
            }
          }
        }

        if unarmed {parry_possible=false;}
        let striker_velocity=striker_contact.and_then(|(part,p,t)|a.striker_velocity(part,component,p,t));
        let module_velocity = module_contact.and_then(|(weapon_id,id,p,t)| a.shape_velocity(weapon_id,id,p,t));
        let contact_speed = if striker_contact.is_some() {striker_velocity.map(|v|len(v).min(MAX_STRIKE_SPEED))}
            else if module_contact.is_some() { module_velocity.map(|v|len(v).min(MAX_STRIKE_SPEED)) }
            else if unarmed { self.body_part_speed(&a, at, c) } else { self.contact_speed(&a, at, c, s_contact) };
        let rel_speed = if striker_contact.is_some() {
            striker_velocity.zip(cap_point_velocity(v,view,c)).map(|(a,v)|len(sub(a,v)).min(MAX_STRIKE_SPEED))
        } else if module_contact.is_some() {
            module_velocity.zip(cap_point_velocity(v,view,c)).map(|(a,v)|len(sub(a,v)).min(MAX_STRIKE_SPEED))
        } else { self.relative_speed(&a, v, at, view, c, s_contact, unarmed) };
        let peak = if let Some((part,p,_))=striker_contact {a.striker_peak(part,component,p,at)}
            else if let Some((weapon_id,id,p,_))=module_contact { a.shape_peak(weapon_id,id,p,at) }
            else { self.peak_speed(&a, at, c, s_contact.map(|h| h.0), unarmed) };
        let peak_speed = [contact_speed, peak]
            .into_iter().flatten().reduce(f32::max);
        Eval::Accept(Info {
            rewind_ms: rewind, view_ts: view, view_clamped, body_dist: body, weapon_dist, swept,
            parry_possible, parry_d, contact_speed, peak_speed, unarmed, rel_speed, rel_exact: unarmed || s_contact.is_some() || module_contact.is_some(),
            hit_box, proxy_box_error,
        })
    }

    /// Reach plausibility of the attacker's own stream at `at`:
    /// streamed blade no longer than its weapon
    /// class allows and held by a hand; hands within arm's reach of the
    /// shoulders. Some(reason) = implausible.
    fn reach_violation(&self, attacker: PeerId, a: &PeerHist, at: u32) -> Option<String> {
        self.reach_violation_blade(attacker, &BladeView { hist: a, blade: &a.blade }, at)
    }
    fn reach_violation_blade(&self, attacker: PeerId, a: &BladeView<'_>, at: u32) -> Option<String> {
        self.reach_violation_parts(attacker, a, at, true)
    }
    fn reach_violation_parts(&self, attacker: PeerId, a: &BladeView<'_>, at: u32, weapon: bool) -> Option<String> {
        let pose = a.pose.sample(at, ATTACKER_LEAD_MS);
        if let Some(p) = &pose {
            // The shoulders ride on the torso. A hand within arm's
            // reach of a shoulder proved nothing when the shoulder itself could
            // be streamed anywhere (a "parry aimbot" moving shoulder, hand and
            // blade smoothly onto the attacker's blade path).
            if p.mask & (1 << posecodec::PELVIS) != 0 {
                for s in [posecodec::UPPERARM_L, posecodec::UPPERARM_R] {
                    if p.mask & (1 << s) != 0 {
                        let d = len(sub(p.p[s], p.p[posecodec::PELVIS]));
                        if d > SHOULDER_PELVIS_MAX {
                            return Some(format!("reach: shoulder {:.0} uu from the pelvis (max {:.0})", d, SHOULDER_PELVIS_MAX));
                        }
                    }
                }
            }
            for (s, h) in [(posecodec::UPPERARM_L, posecodec::HAND_L), (posecodec::UPPERARM_R, posecodec::HAND_R)] {
                if p.mask & (1 << s) != 0 && p.mask & (1 << h) != 0 {
                    let d = len(sub(p.p[h], p.p[s]));
                    if d > ARM_REACH_MAX {
                        return Some(format!("reach: hand {:.0} uu from its shoulder (max {:.0})", d, ARM_REACH_MAX));
                    }
                }
            }
        }
        if !weapon { return None; }
        let b = a.blade.sample(at, ATTACKER_LEAD_MS)?;
        let g = crate::validate::pose::geom(crate::validate::damage::weapon_class_for_hand(attacker, a.offhand()));
        let l = len(sub(b.tip, b.base));
        if l > g.blade_max {
            return Some(format!("reach: blade {:.0} uu long (weapon class max {:.0})", l, g.blade_max));
        }
        if let Some(p) = &pose {
            let grip = [posecodec::HAND_L, posecodec::HAND_R].iter()
                .filter(|&&h| p.mask & (1 << h) != 0)
                .map(|&h| point_seg(p.p[h], b.base, b.tip))
                .fold(f32::INFINITY, f32::min);
            if grip.is_finite() && grip > g.grip_max {
                return Some(format!("reach: blade {:.0} uu from both hands (max {:.0})", grip, g.grip_max));
            }
        }
        None
    }
}

// ---- global store (server) ------------------------------------------------

fn store() -> &'static Mutex<Store> {
    static S: OnceLock<Mutex<Store>> = OnceLock::new();
    S.get_or_init(|| Mutex::new(Store::default()))
}

/// Direct access for tests and diagnostics.
pub(crate) fn with_store<R>(f: impl FnOnce(&mut Store) -> R) -> R { f(&mut store().lock().unwrap()) }

pub fn record_root(id: PeerId, ts: u32, pos: V3) { store().lock().unwrap().record_root(id, ts, pos, now_ms()) }
pub fn record_weapon(id: PeerId, ts: u32, pos: V3) { store().lock().unwrap().record_weapon(id, ts, pos, now_ms()) }
/// Decode a C2SSkeletalState payload into history (malformed → ignored).
/// Returns whether the frame may be relayed to the other players (a
/// frame that fails the pose-to-root tie is not relayed).
/// (An undecodable frame is relayed anyway: receivers decode with the same
/// codec and drop it.)
pub fn record_skeletal(id: PeerId, bones: &[u8]) -> bool {
    match posecodec::decode(bones) {
        Some(f) => {
            let mut s = store().lock().unwrap();
            s.note_pose_codec(id, posecodec::v2::is_v2(bones));
            s.record_pose(id, &f, now_ms())
        }
        None => true,
    }
}
/// A decoded codec v2 frame (protocol v6 `pose` record, decoded once by the caller) into
/// history; same rules as [`record_skeletal`].
pub fn has_pose_context(id:PeerId,match_id:u64,round:u32,life:u16)->bool {
    store().lock().unwrap().has_pose_context(id,match_id,round,life)
}
pub fn has_accepted_pose_context(id:PeerId,match_id:u64,round:u32,life:u16)->bool {
    store().lock().unwrap().has_accepted_pose_context(id,match_id,round,life)
}
pub fn record_skeletal_v2(id: PeerId, f: &posecodec::v2::Full) -> bool {
    let p = posecodec::v2::to_v1(f);
    let mut s = store().lock().unwrap();
    if !s.bind_pose_context(id,f.context) {return false;}
    s.note_pose_codec(id, true);
    let ok=s.record_pose(id, &p, now_ms());
    if ok {s.record_body_strikers(id,p.ts,f);}
    ok
}
/// The relay forwarded `src`'s skeletal frame stamped `ts` (sender clock) to
/// `viewer`, which currently receives `src`'s frames every `interval_ms`.
/// Lag comp judges `viewer`'s hits against the frames it was
/// actually sent, not the full-rate history, and models `viewer`'s buffer
/// delay of `src` from them. `interval_ms` 0 = unknown (the sender's
/// own frame interval is used).
pub fn note_relayed(viewer: PeerId, src: PeerId, ts: u32, interval_ms: u16) {
    store().lock().unwrap().note_relayed(viewer, src, ts, interval_ms, now_ms())
}
pub fn record_blade(id: PeerId, ts: u32, blade: Blade) { store().lock().unwrap().record_blade(id, ts, blade, now_ms()) }
pub fn record_blades(id: PeerId, ts: u32, main: Option<Blade>, offhand: Option<Blade>) {
    let mut s = store().lock().unwrap();
    let now = now_ms();
    for (secondary, blade) in [(false, main), (true, offhand)] {
        if let Some(b) = blade { s.record_blade_hand(id, ts, b, now, secondary); }
        else if let Some(p) = s.peers.get_mut(&id) {
            let ring = if secondary { &mut p.offhand } else { &mut p.blade };
            if ring.newest().map_or(true, |n| ts >= n) { ring.q.clear(); }
        }
    }
}
pub fn record_weapon_shapes(id: PeerId, ts: u32, weapons: &[posecodec::v2::Weapon]) {
    let mut s = store().lock().unwrap();
    for offhand in [false,true] {
        let w = weapons.iter().find(|w| if offhand { w.hands & 1 == 0 && w.hands & 2 != 0 } else { w.hands & 1 != 0 });
        let recorded = w.is_some_and(|w| s.record_weapon_shape(id,ts,w,offhand));
        // No shape is also a timestamped sample of absence: do not retain a
        // former broad axe head after a weapon/module change.
        if let Some(p)=s.peers.get_mut(&id) {
            let ring=if offhand { &mut p.offhand_shapes } else { &mut p.shapes };
            if ring.newest().map_or(true,|n|ts>n) && !recorded { ring.q.clear(); }
        }
    }
}
pub fn record_capsules(id: PeerId, ts: u32, caps: &[Capsule]) {
    store().lock().unwrap().record_capsules(id, ts, caps, now_ms())
}
/// Per-connection RTT sample (ms) from any source (damage ack, conn layer).
pub fn note_rtt(id: PeerId, rtt_ms: f32) { store().lock().unwrap().note_rtt(id, rtt_ms, now_ms()) }
/// Connection-layer RTT variation (ms) of `id` (see `Store::note_net_jitter`).
pub fn note_net_jitter(id: PeerId, rttvar_ms: f32) { store().lock().unwrap().note_net_jitter(id, rttvar_ms, now_ms()) }
pub fn clock_info(id: PeerId) -> Option<ClockInfo> { store().lock().unwrap().clock_info(id, now_ms()) }
pub fn set_max_rewind(ms: i64) { store().lock().unwrap().set_max_rewind(ms) }
pub fn note_death(id: PeerId) { store().lock().unwrap().note_death(id, now_ms()) }
/// A respawned player: its last death no longer opens a trade window.
pub fn note_alive(id: PeerId) { store().lock().unwrap().note_alive(id) }
pub fn forget(id: PeerId) {
    store().lock().unwrap().forget(id);
    crate::validate::damage::forget(id);
}
pub fn record_clash(reporter: PeerId, other: PeerId, my_ts: u32, other_ts: u32) {
    store().lock().unwrap().record_clash(reporter, other, my_ts, other_ts, now_ms())
}
pub fn record_touch(victim: PeerId, attacker: PeerId, my_ts: u32, other_ts: u32) {
    store().lock().unwrap().record_touch(victim, attacker, my_ts, other_ts, now_ms())
}
pub fn evaluate(attacker: PeerId, hit: &DamageEvent) -> Eval { evaluate_at(attacker, hit, Instant::now()) }
pub fn evaluate_at(attacker: PeerId, hit: &DamageEvent, now: Instant) -> Eval {
    store().lock().unwrap().evaluate(attacker, hit, ms_at(now))
}
pub fn parried(victim: PeerId, attacker: PeerId, attacker_ts: u32, now: Instant) -> bool {
    store().lock().unwrap().parried(victim, attacker, attacker_ts, ms_at(now))
}
pub fn trade_ok(attacker: PeerId, attacker_ts: u32, now: Instant) -> bool {
    store().lock().unwrap().trade_ok(attacker, attacker_ts, ms_at(now))
}
/// Newly server-validated clashes as (reporter, other): the server tick
/// confirms each to both players (combat_glue::notify_clashes).
pub fn judge_due() -> Vec<(PeerId, PeerId)> { store().lock().unwrap().judge_due(now_ms()) }

// Interaction-channel geometry (docs/development/subsystems/interact.md).
// Explicit path: crates/hsmp-combat-sim includes this file via #[path], and an
// outer `mod x;` would then be looked up next to the includer, not here.
#[path = "lagcomp/interact_query.rs"]
mod interact_query;
pub use interact_query::{interact_distance, Geo};

#[cfg(test)]
mod tests;

/// The streamed blade of `h` teleported (tip faster than
/// CLASH_BLADE_SPEED_MAX between samples) within 2 frames of `t`: a blade
/// snapped onto another for the reported frame to fake a parry is not a
/// block. Only exact (streamed) samples are judged; no data = no
/// teleport.
fn blade_too_fast(h: &PeerHist, t: u32, fr: i64) -> bool {
    let (lo, hi) = (t as i64 - 2 * fr, t as i64 + 2 * fr);
    h.blade_jumps.iter().any(|&j| (lo..=hi).contains(&(j as i64)))
}

/// Clash reporter: is the streamed blade at `t` held by its hilt? For
/// one-handed / hafted-head classes the blade BASE (the weapon's root scene,
/// at the grip) must lie within the class's grip distance of a hand — an
/// attacker's blade copied onto the reporter's stream passes near its hand
/// with its hilt in the attacker's. Polearms (and the unknown game gear) may
/// be held anywhere along the haft: segment distance, as `reach_violation`.
#[cfg(test)]
fn blade_unholdable(id: PeerId, h: &PeerHist, t: u32) -> bool {
    blade_view_unholdable(id, &BladeView { hist: h, blade: &h.blade }, t)
}
fn blade_view_unholdable(id: PeerId, h: &BladeView<'_>, t: u32) -> bool {
    let (Some(b), Some(p)) = (h.blade.sample(t, ATTACKER_LEAD_MS), h.pose.sample(t, ATTACKER_LEAD_MS)) else { return false };
    let class = crate::validate::damage::weapon_class_for_hand(id, h.offhand());
    let g = crate::validate::pose::geom(class);
    let hands = [posecodec::HAND_L, posecodec::HAND_R];
    let d = hands.iter().filter(|&&k| p.mask & (1 << k) != 0).map(|&k| match class {
        crate::validate::damage::WeaponClass::Polearm | crate::validate::damage::WeaponClass::Unknown => point_seg(p.p[k], b.base, b.tip),
        _ => len(sub(p.p[k], b.base)),
    }).fold(f32::INFINITY, f32::min);
    d.is_finite() && d > g.grip_max
}

/// v1 clash fallback (no blade stream): the weapon actor `id` streams at `t`
/// is further from its nearer hand than its weapon class's blade plus grip
/// (an actor streamed onto the attacker's blade).
fn weapon_out_of_reach(id: PeerId, h: &PeerHist, t: u32) -> bool {
    let (Some(w), Some(p)) = (h.weapon.sample(t, ATTACKER_LEAD_MS), h.pose.sample(t, ATTACKER_LEAD_MS)) else { return false };
    let Some(g) = nearer_hand(&p, w) else { return false };
    let geom = crate::validate::pose::geom(crate::validate::damage::weapon_class(id));
    len(sub(w, g)) > geom.blade_max + geom.grip_max
}

/// Velocity (uu/s) of the point of `h`'s nearest capsule to `c` at `t`
/// (central difference over ±8 ms of its capsule / pose stream).
fn cap_point_velocity(h: &PeerHist, t: u32, c: V3) -> Option<V3> {
    let cs = h.capsules(t, FUTURE_MS)?;
    let (mut bi, mut bd, mut bu) = (0usize, f32::INFINITY, 0.0f32);
    for (i, k) in cs.iter().enumerate() {
        let d = point_seg(c, k.a, k.b) - k.r;
        if d < bd { bd = d; bi = i; bu = seg_param(c, k.a, k.b); }
    }
    let c1 = h.capsules(t.saturating_add(8), FUTURE_MS)?;
    let c0 = h.capsules(ts_sub(t, 8), FUTURE_MS)?;
    if bi >= c1.n || bi >= c0.n { return None; }
    let p1 = lerp3(c1.c[bi].a, c1.c[bi].b, bu);
    let p0 = lerp3(c0.c[bi].a, c0.c[bi].b, bu);
    Some(mul(sub(p1, p0), 1000.0 / 16.0))
}
