//! Server-side combat replication: anti-cheat validation, dedup and ack
//! bookkeeping for `damage` claim record (owner-authoritative damage relay).
//!
//! Reliability model (internet: 50–250 ms RTT, jitter, 1–5 % loss,
//! reordering, duplicates):
//! - The attacker's sidecar resends `damage` claim record until it gets FINAL `damage_verdict`.
//! - The verdict for `(attacker, hit_id)` is made ONCE, on first arrival, and
//!   cached. Resends never re-validate (positions may have changed since),
//!   they only re-forward to the owner (while it hasn't acked) or re-ack.
//! - The owner dedups by `(attacker, hit_id)` and always acks, so a hit is
//!   applied at most once and, unless it expires, at least once.
//!
//! Lag compensation (lagcomp.rs): claims carrying sender timestamps are also
//! judged against the rewound victim/attacker poses. A geometrically valid
//! timestamped hit is HELD for DEFENDER_GRACE only when the victim's blade
//! was near the attack (a parry the victim's client sees ~its RTT later could
//! still cancel it); it resolves on the next resend or the server tick
//! (`flush_pending`). All other hits are forwarded immediately. The view
//! time is server-predicted, timestamps are mandatory once
//! the victim streams, and only server-validated clashes cancel hits.
//!
//! Damage plausibility (`validate::damage`): an accepted hit's
//! Health loss is capped by weapon class × contact speed (server history) ×
//! victim armour before it is forwarded or booked; resends get the approved
//! (clamped) event. Suspicious claims bump `validate::cheat` counters. The
//! damage_in → owner-ack time is fed to `lagcomp::note_rtt` (Karn's rule).
//!
//! Authoritative deaths (`Ledger`): the server keeps an UPPER-BOUND estimate
//! of each player's Health for the round:
//!   est = last owner vitals (anti-cheat ceiling applied)
//!         − Σ Health loss of accepted hits the owner has not yet reflected.
//! A forwarded hit counts as reflected once a vitals packet arrives
//! REFLECT_AFTER after the owner's sidecar acked it (the game applies a hit
//! and reports vitals in the same 33 ms Lua tick). If the owner's game
//! stalls, is alt-tabbed or lies, its hits stay unreflected, so a lethal hit
//! drives est ≤ 0 and the SERVER declares the death — the victim cannot keep
//! fighting until its own client notices. Owner-reported deaths and dead
//! vitals still count too (bleed-out, head trauma, falls).
//!
//! Vitals stream v2 (`vitals` records, docs/development/subsystems/vitals.md): full frames
//! (limbs, consciousness, stamina, bleeding, flags, severed parts) are
//! sanitized here (`vitals_frame_in`), fed to the same ledger (`on_frame`:
//! Health + the DED flag) and relayed with Health clamped to the ledger's
//! ceiling.

use crate::lagcomp::{self, Eval, Store};
use crate::validate::cheat::{self, Kind as CheatKind};
use crate::validate::damage;
use crate::validate::rate::Bucket;
use crate::proto::vitals::{self, Vitals};
use crate::proto::{DamageEvent, PeerId};
use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

/// Max ms between the hit on the attacker's screen and its first arrival
/// here. Covers the attacker's resends under loss; real network delay is on
/// top of this and is tolerated by the distance slack below.
pub const LAG_WINDOW_MS: u32 = 350;
/// Stop re-forwarding an accepted hit to the owner after this long.
pub const FORWARD_TTL: Duration = Duration::from_millis(1500);
/// Sword reach + bodies, in UE units (1 UE = 1 cm).
const BASE_RANGE_UE: f32 = 500.0;
/// Lag compensation: two players moving ~6.6 m/s for ~300 ms of latency.
const LAG_SLACK_UE: f32 = 400.0;
const RAW_DAMAGE_CAP: f32 = 20_000.0;
/// Willie Health starts at 100.
const DAMAGE_OUT_CAP: f32 = 150.0;
const MAX_BONE_LEN: usize = 32;
/// Per-field damage deltas (HSMPCombat FIELDS table): count, index range, and
/// magnitude caps. Deltas are SANITISED, never a reason to reject an honest
/// hit (live play: "bad damage deltas" rejected nearly every damaging hit,
/// because Get Damage writes the hit's DRS — thousands — into the
/// bookkeeping fields below):
/// - BOOKKEEPING fields are dropped: #15 "Sustained Damage" and #17 "Last
///   Damage Taken" are Get Damage's own gate state (= DRS, reset after
///   0.2 s), not damage; the owner's native replay sets them itself.
/// - unknown indices and non-finite values are dropped;
/// - health-type fields (0..100 on a Willie) are clamped to ±HEALTH_DELTA_CAP,
///   the rest (pain, bleeding, blood rate, stamina) to ±DELTA_ABS_CAP.
///
/// Anything dropped or clamped bumps the attacker's `DamageClamped` counter.
const MAX_DELTAS: usize = 24;
const DELTA_FIELDS: usize = 24;
const DELTA_ABS_CAP: f32 = 500.0;
const HEALTH_DELTA_CAP: f32 = 150.0;
pub const BOOKKEEPING_FIELDS: [u8; 2] = [15, 17];
/// FIELDS indices of health-type (0..100) fields: Health, part healths,
/// Consciousness, Head Health (Crush), Consciousness 2 (Legs).
const HEALTH_FIELDS: [u8; 13] = [0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 18];

/// Sanitise a claim's deltas in place; returns how many were dropped or
/// clamped for something other than being bookkeeping.
pub fn sanitize_deltas(hit: &mut DamageEvent) -> u32 {
    hit.retain_deltas(|d| !BOOKKEEPING_FIELDS.contains(&d.i));
    let before = hit.deltas().len();
    hit.retain_deltas(|d| (d.i as usize) < DELTA_FIELDS && d.v.is_finite());
    let mut odd = (before - hit.deltas().len()) as u32;
    debug_assert!(hit.deltas().len() <= MAX_DELTAS);
    for d in hit.deltas_mut() {
        let cap = if HEALTH_FIELDS.contains(&d.i) { HEALTH_DELTA_CAP } else { DELTA_ABS_CAP };
        if d.v.abs() > cap { d.v = d.v.clamp(-cap, cap); odd += 1; }
    }
    odd
}
/// Legacy delta reports have a small budget. Native armour-stage reports are
/// per bone and per call passing the game's contact gate, not per swing; a
/// clinch can produce several valid contacts in a single physics frame.
const BUCKET_CAP: f32 = 20.0;
const BUCKET_REFILL_PER_S: f32 = 20.0;
const COMPLEX_BUCKET_CAP: f32 = 120.0;
const COMPLEX_REFILL_PER_S: f32 = 120.0;
const DECISION_TTL: Duration = Duration::from_secs(10);
/// Every FIRST arrival of a claim (valid or not) costs one of these before
/// any validation (otherwise rejected claims would cost nothing and each be
/// stored, logged and answered). Over budget: dropped silently (no
/// decision, no reply). Includes headroom for rejected native contacts.
const CLAIM_BUCKET_CAP: f32 = 160.0;
const CLAIM_REFILL_PER_S: f32 = 160.0;
/// Sticky decisions kept per attacker (DECISION_TTL × the claim budget is
/// plus the initial burst is <= 1760). Beyond it: dropped silently.
pub const MAX_DECISIONS_PER_ATTACKER: usize = 1760;
/// Default / test defender grace. The live value is per victim
/// (`lagcomp::Store::grace_ms`: victim RTT + display delay + 2·jitter + IPC,
/// clamped to 120..350 ms).
#[cfg(test)]
pub const DEFENDER_GRACE: Duration = Duration::from_millis(200);
/// A claim can overtake the attacker's own pose stream (reliable vs
/// unreliable channel, different IPC paths): it waits this long for the
/// stream sample of its hit frame before being judged against the newest
/// sample instead (2 frames + wifi jitter).
pub const WAIT_MAX_MS: i64 = 80;
/// Reason the server sends with an EARLY positive S2CDamageAck (hit accepted
/// and forwarded to its owner; not final, the attacker keeps resending until
/// the final ack). HSMPCombat plays its confirmed-hit cue on it.
pub const CONFIRM_REASON: &str = "confirm";
/// Reason prefix of the clash confirmation sent as S2CDamageAck{hit_id: 0}.
pub const CLASH_REASON: &str = "clash";

fn ms(d: Duration) -> i64 { d.as_millis() as i64 }

/// What the server knows about attacker/target/match when a hit first arrives.
pub struct Ctx {
    pub attacker_id: PeerId,
    pub attacker_alive: bool,
    pub attacker_pos: Option<[f32; 3]>,
    pub target_exists: bool,
    pub target_alive: bool,
    pub target_pos: Option<[f32; 3]>,
    pub match_live: bool,
    pub match_round: u32,
    pub match_id: u64,
    pub attacker_life: u16,
    pub target_life: u16,
}

#[derive(Debug, PartialEq)]
pub enum Verdict {
    /// Accepted geometrically, waiting out the defender grace: send nothing
    /// (the attacker keeps resending; `flush_pending` resolves it).
    Hold,
    /// First arrival, accepted: forward `damage_in` to the owner.
    Forward,
    /// Duplicate of an accepted hit the owner hasn't acked yet: forward again.
    Reforward,
    /// Final answer for the attacker (reject, or accepted-and-owner-acked,
    /// or expired). Send FINAL `damage_verdict`.
    Ack { accepted: bool, reason: String },
    /// Over the attacker's claim budget / decision cap (a flood): send
    /// nothing, store nothing.
    Ignore,
}

struct Decision {
    accepted: bool,
    reason: String,
    /// Server ms of the decision (of the forward, for held hits).
    decided_at: i64,
    /// First arrival (server ms).
    first_at: i64,
    /// Passed the field checks, waiting for the attacker's stream to cover
    /// its hit time (lag comp `Eval::Wait`), at most WAIT_MAX_MS.
    waiting: Option<DamageEvent>,
    target_acked: bool,
    owner_outcome: Option<hsmp_ipc::schema::combat::ReplayOutcome>,
    /// Held for the defender grace (accepted so far, not yet forwarded).
    pending: Option<DamageEvent>,
    /// Defender grace of a held hit, ms.
    grace_ms: i64,
    /// Only weapon contacts can be vetoed by a weapon-on-weapon clash.
    parryable: bool,
    /// The server-approved event (damage clamped by `validate::damage`):
    /// every (re)forward sends this, never the attacker's resend.
    approved: Option<DamageEvent>,
    /// First forward to the owner, for the RTT sample on its ack (Karn:
    /// no sample once re-forwarded).
    forwarded_at: Option<i64>,
    reforwarded: bool,
    /// The early "confirm" ack was handed out (`take_confirm`).
    confirmed: bool,
}

/// Claim bookkeeping (decisions, rate buckets). Time is server ms
/// (`lagcomp::now_ms` domain); the lag-comp store is passed in, so the
/// combat simulator (crates/hsmp-combat-sim) drives exactly this code with
/// its own clock and store.
#[derive(Default)]
pub struct Engine {
    decisions: HashMap<(PeerId, u32), Decision>,
    buckets: HashMap<PeerId, Bucket>,
    complex_buckets: HashMap<PeerId, Bucket>,
    /// First-arrival claim budget per attacker (before validation).
    claim_buckets: HashMap<PeerId, Bucket>,
    /// Live decisions per attacker (bounded by MAX_DECISIONS_PER_ATTACKER).
    per_attacker: HashMap<PeerId, usize>,
    /// Decisions still waiting for stream coverage or held for the defender
    /// grace: the only ones `flush_pending` visits every tick.
    active: std::collections::BTreeSet<(PeerId, u32)>,
    last_prune: Option<i64>,
    /// Open stuck-blade continuations by (attacker, parent cid): bound to the parent once,
    /// kept for INSIDE_MAX_MS (the parent decision itself is pruned after DECISION_TTL).
    inside: HashMap<(PeerId, u32), InsideOpen>,
}

#[derive(Clone)]
struct InsideOpen { parent_at: i64, parent: DamageEvent, first: i64, calls: u32 }

/// A stuck blade (native Constraint_Weapon_Stuck) calls the victim's Get Damage with
/// Inside every tick while embedded (plus head / bone / snap events): at most this many
/// calls per second and this long after its parent hit.
pub const INSIDE_MAX_PER_S: f32 = 90.0;
pub const INSIDE_MAX_MS: i64 = 30_000;
/// A stuck blade's Get Damage inputs are force-driven, not impact-driven: measured live
/// (316 native Inside calls, arming sword vs clothing) raw up to 167,935 and cutting power up
/// to 80,750. Bounds with headroom over that; the ordinary RAW_DAMAGE_CAP does not apply.
pub const INSIDE_RAW_CAP: f32 = 1_000_000.0;
pub const INSIDE_CUT_CAP: f32 = 200_000.0;

fn engine() -> &'static Mutex<Engine> {
    static S: OnceLock<Mutex<Engine>> = OnceLock::new();
    S.get_or_init(|| Mutex::new(Engine::default()))
}

fn dist(a: [f32; 3], b: [f32; 3]) -> f32 {
    let (dx, dy, dz) = (a[0] - b[0], a[1] - b[1], a[2] - b[2]);
    (dx * dx + dy * dy + dz * dz).sqrt()
}

fn finite3(v: [f32; 3]) -> bool {
    v.iter().all(|x| x.is_finite())
}

/// Stable code of a reject reason ("code: details" → "code").
#[allow(dead_code)]
pub fn reason_code(reason: &str) -> &str {
    reason.split(':').next().unwrap_or(reason).trim()
}

fn validate(lc: &Store, ctx: &Ctx, hit: &DamageEvent, now_ms: i64) -> Option<String> {
    let range = BASE_RANGE_UE + LAG_SLACK_UE;
    // Trades: a hit swung no later than the attacker's own death (+ window)
    // still lands, so mutual kills both count.
    if !ctx.attacker_alive && !lc.trade_ok(ctx.attacker_id, hit.attacker_ts, now_ms) {
        return Some("attacker_down: attacker down".into());
    }
    if hit.target_peer_id == ctx.attacker_id {
        return Some("self_hit: self-hit".into());
    }
    if !ctx.target_exists {
        return Some("no_target: no such target".into());
    }
    if !ctx.target_alive {
        return Some("target_down: target already down".into());
    }
    if !ctx.match_live {
        return Some("not_live: match not live".into());
    }
    if hit.match_id != ctx.match_id || hit.attacker_life != ctx.attacker_life || hit.victim_life != ctx.target_life {
        return Some("stale_life: hit match or pawn generation changed".into());
    }
    if hit.round != ctx.match_round {
        return Some(format!("stale_round: stale round {} (now {})", hit.round, ctx.match_round));
    }
    if hit.age_ms > LAG_WINDOW_MS {
        return Some(format!("too_late: too late ({} ms > {} ms)", hit.age_ms, LAG_WINDOW_MS));
    }
    if hit.bone.is_empty() || hit.bone.len() > MAX_BONE_LEN {
        return Some("bad_field: bad bone".into());
    }
    if let Err(e)=hsmp_ipc::schema::combat::check_damage(&hit.h) {
        return Some(format!("bad_field: native contact schema {e:?}"));
    }
    if hit.dism_blunt & damage::DAMAGE_PARENT != 0 && hit.flags & damage::FLAG_COMPLEX == 0 {
        return Some("bad_field: DamageParent requires native contact".into());
    }
    let continuation = hit.parent_cid != 0 && hit.flags & damage::FLAG_INSIDE != 0 && hit.flags & damage::FLAG_COMPLEX == 0;
    if hit.dism_blunt & (damage::SOURCE_COMPONENT_MASK | damage::SOURCE_FEET | damage::HIT_BOX_MASK) != 0
        && hit.flags & (damage::FLAG_COMPLEX | damage::FLAG_WEAPON) != (damage::FLAG_COMPLEX | damage::FLAG_WEAPON)
        && !(continuation && hit.flags & damage::FLAG_WEAPON != 0
            && hit.dism_blunt & (damage::SOURCE_FEET | damage::HIT_BOX_MASK) == 0) {
        return Some("bad_field: component identity requires native weapon contact".into());
    }
    if hit.flags & damage::FLAG_COMPLEX != 0 {
        let source = hit.dism_blunt & damage::SOURCE_MASK;
        let feet = source & damage::SOURCE_FEET != 0;
        if hit.dism_blunt < 0
            || (hit.dism_blunt & damage::HIT_BOX_MASK != 0 && (source & damage::SOURCE_COMPONENT_MASK == 0
                || source & (damage::SOURCE_LEFT | damage::SOURCE_RIGHT) == 0))
            || (feet && (source & damage::SOURCE_FIST != 0 || source & (damage::SOURCE_LEFT | damage::SOURCE_RIGHT) == 0
                || source & damage::SOURCE_COMPONENT_MASK == 0))
            || source & (damage::SOURCE_LEFT | damage::SOURCE_RIGHT) == (damage::SOURCE_LEFT | damage::SOURCE_RIGHT)
            || (source != 0 && hit.flags & damage::FLAG_WEAPON == 0) {
            return Some("bad_field: invalid native contact source".into());
        }
    }
    let scalars = [hit.raw_damage, hit.cutting_power, hit.pain_rate, hit.draw_cut, hit.damage_out];
    if !scalars.iter().all(|x| x.is_finite())
        || ![hit.offset, hit.location, hit.impulse, hit.velocity, hit.normal].iter().all(|v| finite3(*v))
    {
        return Some("bad_field: non-finite field".into());
    }
    let raw_cap = if hit.parent_cid != 0 { INSIDE_RAW_CAP } else { RAW_DAMAGE_CAP };
    if !(0.0..=raw_cap).contains(&hit.raw_damage) {
        return Some(format!("bad_field: raw damage {:.0} out of range", hit.raw_damage));
    }
    if !(0.0..=DAMAGE_OUT_CAP).contains(&hit.damage_out) {
        return Some(format!("bad_field: damage {:.1} out of range", hit.damage_out));
    }
    if dist(hit.offset, [0.0; 3]) > 200.0 {
        return Some("bad_field: hit offset too far from bone".into());
    }
    if let (Some(a), Some(t)) = (ctx.attacker_pos, ctx.target_pos) {
        let d = dist(a, t);
        if d > range {
            return Some(format!("range: out of range ({:.0} > {:.0})", d, range));
        }
        let dl = dist(hit.location, t);
        if dl > range {
            return Some(format!("range: hit point away from target ({:.0})", dl));
        }
    }
    None
}

impl Engine {
    // A continuation can be approved while its parent is in defender grace.
    // Check again before every delivery, including cached retransmissions.
    fn rejected_parent(&self, attacker: PeerId, hit: &DamageEvent) -> Option<String> {
        if hit.parent_cid == 0 { return None; }
        self.decisions.iter().find_map(|(k, d)| {
            let parent = d.approved.as_ref().or(d.waiting.as_ref()).or(d.pending.as_ref())?;
            (k.0 == attacker && parent.cid == hit.parent_cid && parent.parent_cid == 0
                && !d.accepted && d.waiting.is_none()).then(|| format!("no_parent: parent rejected ({})", d.reason))
        })
    }

    /// A FLAG_INSIDE claim with `parent_cid`: the attacker's accepted, forwarded hit with that
    /// cid on the same victim, match, round, both lives and bone (the constraint moves a pelvis
    /// hit to spine_02) is its authentication; the native call's own bounds (raw / cut / draw
    /// / pain) and the per-constraint rate and lifetime caps bound it.
    fn inside_continuation(&mut self, ctx: &Ctx, hit: &DamageEvent, now_ms: i64) -> Result<bool, String> {
        if hit.flags & damage::FLAG_INSIDE == 0 || hit.flags & damage::FLAG_COMPLEX != 0 {
            return Err("bad_field: a continuation is an Inside Get Damage call".into());
        }
        let same_bone = |a: &str, b: &str| a.eq_ignore_ascii_case(b)
            || (a.eq_ignore_ascii_case("pelvis") && b.eq_ignore_ascii_case("spine_02"))
            // Native @36442 rebinds Bone Name 2 to overlapping bones. The
            // same exact constraint moves lowerarm_l -> hand_l in inside-2
            // UE4SS.log:1694,1709. No unmeasured adjacent-bone aliases.
            || (a.eq_ignore_ascii_case("lowerarm_l") && b.eq_ignore_ascii_case("hand_l"));
        let key = (ctx.attacker_id, hit.parent_cid);
        if let Some(reason) = self.rejected_parent(ctx.attacker_id, hit) {
            self.inside.remove(&key);
            return Err(reason);
        }
        if self.inside.get(&key).is_some_and(|o| now_ms - o.parent_at > INSIDE_MAX_MS) {
            self.inside.remove(&key);
            return Err("too_late: stuck blade outlived its continuation window".into());
        }
        self.inside.retain(|_, o| now_ms - o.parent_at <= INSIDE_MAX_MS);
        if !self.inside.contains_key(&key) {
            // accepted (forwarded, or held for the defender grace: delivery after it is ordered
            // by blocks_delivery); still waiting for stream coverage: not decided yet (Hold)
            let found = self.decisions.iter().find(|(k, d)| k.0 == ctx.attacker_id && d.accepted
                && d.approved.as_ref().is_some_and(|p| p.cid == hit.parent_cid && p.parent_cid == 0))
                .and_then(|(_, d)| d.approved.map(|p| (d.first_at, p)));
            let Some((parent_at, parent)) = found else {
                if self.decisions.iter().any(|(k, d)| k.0 == ctx.attacker_id && d.waiting.as_ref().is_some_and(|p| p.cid == hit.parent_cid)) {
                    return Ok(false);
                }
                return Err("no_parent: stuck-blade call without its accepted parent hit".into())
            };
            self.inside.insert(key, InsideOpen { parent_at, parent, first: now_ms, calls: 0 });
        }
        let open = self.inside.get(&key).expect("bound above").clone();
        let p = &open.parent;
        if hit.flags & damage::FLAG_WEAPON != p.flags & damage::FLAG_WEAPON
            || (p.flags & damage::FLAG_WEAPON != 0 && (hit.dism_blunt & damage::SOURCE_MASK != p.dism_blunt & damage::SOURCE_MASK
                || hit.source_class != p.source_class)) {
            return Err("no_parent: continuation weapon module differs from its parent".into());
        }
        if !(p.target_peer_id == hit.target_peer_id && p.match_id == hit.match_id && p.round == hit.round
            && p.attacker_life == hit.attacker_life && p.victim_life == hit.victim_life && same_bone(p.bone_str(), hit.bone_str())) {
            return Err(format!("no_parent: not the parent hit's victim, life or bone (parent {} {} life {}, call {} {} life {})",
                p.target_peer_id, p.bone_str(), p.victim_life, hit.target_peer_id, hit.bone_str(), hit.victim_life));
        }
        let parent_at = open.parent_at;
        if !(0.0..=INSIDE_RAW_CAP).contains(&hit.raw_damage) || !(0.0..=INSIDE_CUT_CAP).contains(&hit.cutting_power)
            || !(0.0..=1.0).contains(&hit.draw_cut) || !(0.0..=1.0).contains(&hit.pain_rate) || hit.damage_out != 0.0 {
            return Err("bad_field: stuck-blade call outside the native bounds".into());
        }
        if now_ms - parent_at > INSIDE_MAX_MS {
            return Err("too_late: stuck blade outlived its continuation window".into());
        }
        let o = self.inside.get_mut(&key).expect("bound above");
        o.calls += 1;
        let secs = ((now_ms - o.first) as f32 / 1000.0).max(1.0);
        if o.calls as f32 > INSIDE_MAX_PER_S * secs {
            cheat::bump(ctx.attacker_id, CheatKind::ClaimFlood);
            return Err("rate_limited: stuck-blade calls above the native tick rate".into());
        }
        Ok(true)
    }

    fn take_token(&mut self, attacker: PeerId, now_ms: i64, complex: bool) -> bool {
        let (buckets, cap, refill) = if complex {
            (&mut self.complex_buckets, COMPLEX_BUCKET_CAP, COMPLEX_REFILL_PER_S)
        } else { (&mut self.buckets, BUCKET_CAP, BUCKET_REFILL_PER_S) };
        buckets.entry(attacker).or_insert(Bucket::full(cap, now_ms)).take(cap, refill, now_ms)
    }

    /// Decide what to do with a `damage` claim record (first arrival or resend). On
    /// Forward / Reforward `hit` has been replaced by the server-approved
    /// event (damage capped by the plausibility model): forward THAT.
    pub fn on_damage(&mut self, lc: &mut Store, ctx: &Ctx, hit: &mut DamageEvent, now_ms: i64) -> Verdict {
        let key = (ctx.attacker_id, hit.hit_id);
        if let Some(approved) = self.decisions.get(&key).filter(|d|d.accepted).and_then(|d|d.approved) {
            if let Some(reason) = self.rejected_parent(ctx.attacker_id, &approved) {
                tracing::info!(attacker=ctx.attacker_id,target=approved.target_peer_id,hit_id=approved.hit_id,parent_cid=approved.parent_cid,
                    bone=approved.bone_str(),source=approved.dism_blunt & damage::SOURCE_MASK,stage="delivery",reason,
                    "stuck blade rejected");
                self.reject_decision(ctx.attacker_id, approved.hit_id, &reason);
                return Verdict::Ack { accepted: false, reason };
            }
        }
        let mut v = self.on_damage_inner(lc, ctx, hit, now_ms);
        if matches!(&v, Verdict::Ack { accepted: false, reason } if reason == "parried") {
            self.reject_decision(ctx.attacker_id, hit.hit_id, "parried");
        }
        if v == Verdict::Forward && self.blocks_delivery(key, hit) {
            self.defer_delivery(key, *hit);
            v = Verdict::Hold;
        }
        if self.decisions.get(&key).map_or(false, |d| d.waiting.is_some() || d.pending.is_some()) {
            self.active.insert(key);
        }
        v
    }

    /// Earlier contacts from this attacker to this owner must settle first:
    /// native Get Damage's previous-bone gate makes application order semantic.
    fn blocks_delivery(&self, key: (PeerId, u32), hit: &DamageEvent) -> bool {
        self.active.range((key.0, 0)..key).any(|k| self.decisions.get(k).and_then(|d|
            d.waiting.as_ref().or(d.pending.as_ref())).map_or(false, |h|
                h.target_peer_id == hit.target_peer_id && h.round == hit.round))
    }
    fn defer_delivery(&mut self, key: (PeerId, u32), hit: DamageEvent) {
        if let Some(d) = self.decisions.get_mut(&key) {
            d.pending = Some(hit);
            d.forwarded_at = None;
        }
        self.active.insert(key);
    }

    fn prune(&mut self, now_ms: i64) {
        if self.last_prune.map_or(true, |t| now_ms - t > 2000) {
            self.decisions.retain(|_, d| now_ms - d.decided_at < ms(DECISION_TTL));
            self.per_attacker.clear();
            for k in self.decisions.keys() { *self.per_attacker.entry(k.0).or_insert(0) += 1; }
            let decisions = &self.decisions;
            self.active.retain(|k| decisions.contains_key(k));
            self.last_prune = Some(now_ms);
        }
    }

    fn on_damage_inner(&mut self, lc: &mut Store, ctx: &Ctx, hit: &mut DamageEvent, now_ms: i64) -> Verdict {
        self.prune(now_ms);
        let key = (ctx.attacker_id, hit.hit_id);
        // A retransmit cannot inherit an old decision after respawn/rematch.
        // Check its original callback context before consulting the sticky cache.
        if hit.match_id!=ctx.match_id || hit.round!=ctx.match_round
            || hit.attacker_life!=ctx.attacker_life || hit.victim_life!=ctx.target_life {
            return Verdict::Ack{accepted:false,reason:"stale_life: original callback context changed".into()};
        }
        if self.decisions.get(&key).and_then(|d|d.approved.as_ref()).is_some_and(|old|
            old.match_id!=hit.match_id || old.round!=hit.round || old.attacker_life!=hit.attacker_life || old.victim_life!=hit.victim_life) {
            self.decisions.remove(&key);self.active.remove(&key);
            if let Some(n)=self.per_attacker.get_mut(&ctx.attacker_id) {*n=n.saturating_sub(1);}
        }
        if let Some(d) = self.decisions.get_mut(&key) {
            if d.waiting.is_some() {
                // Still waiting for the attacker's stream to cover its hit.
                return match settle_waiting(lc, ctx.attacker_id, d, now_ms) {
                    Some((v, ev)) => { *hit = ev; v }
                    None => Verdict::Hold,
                };
            }
            if d.pending.is_some() {
                return match resolve(lc, ctx.attacker_id, d, now_ms) {
                    Some((v, ev)) => {
                        *hit = ev;
                        v
                    }
                    None => Verdict::Hold,
                };
            }
            if let Some(ev) = &d.approved {
                *hit = *ev;
            }
            if !d.accepted {
                return Verdict::Ack { accepted: false, reason: d.reason.clone() };
            }
            if d.target_acked {
                return Verdict::Ack { accepted: true, reason: String::new() };
            }
            if now_ms - d.decided_at > ms(FORWARD_TTL) {
                return Verdict::Ack { accepted: false, reason: "expired: owner never acked".into() };
            }
            d.reforwarded = true;
            return Verdict::Reforward;
        }
        // First arrival: every claim pays for its own bookkeeping, before any
        // validation, and an attacker's decisions are bounded.
        let a = ctx.attacker_id;
        let ok = self.claim_buckets.entry(a).or_insert(Bucket::full(CLAIM_BUCKET_CAP, now_ms))
            .take(CLAIM_BUCKET_CAP, CLAIM_REFILL_PER_S, now_ms);
        if !ok || self.per_attacker.get(&a).copied().unwrap_or(0) >= MAX_DECISIONS_PER_ATTACKER {
            cheat::bump(a, CheatKind::ClaimFlood);
            return Verdict::Ignore;
        }
        *self.per_attacker.entry(a).or_insert(0) += 1;
        if hit.parent_cid != 0 {
            // A stuck-blade continuation: no contact of its own to rewind (the blade is inside
            // the body); it stands on its accepted parent hit.
            let mut d = Decision { accepted: false, reason: String::new(), decided_at: now_ms, first_at: now_ms, target_acked: false,
                owner_outcome: None, waiting: None, pending: None, grace_ms: 0, parryable: false, approved: None,
                forwarded_at: None, reforwarded: false, confirmed: false };
            let checked = match validate(lc, ctx, hit, now_ms) { Some(r) => Err(r), None => self.inside_continuation(ctx, hit, now_ms) };
            let verdict = match checked {
                Err(r) => { d.reason = r.clone(); Verdict::Ack { accepted: false, reason: r } }
                // its parent is still waiting for stream coverage: decide on the resend
                Ok(false) => { if let Some(n) = self.per_attacker.get_mut(&a) { *n = n.saturating_sub(1); } return Verdict::Hold; }
                Ok(true) => { d.accepted = true; d.approved = Some(*hit); d.forwarded_at = Some(now_ms); Verdict::Forward }
            };
            match &verdict {
                Verdict::Forward=>tracing::info!(attacker=ctx.attacker_id,target=hit.target_peer_id,hit_id=hit.hit_id,
                    parent_cid=hit.parent_cid,bone=hit.bone_str(),source=hit.dism_blunt & damage::SOURCE_MASK,
                    class=hit.source_class.as_str().unwrap_or(""),stage="decision","stuck blade accepted"),
                Verdict::Ack{accepted:false,reason}=>tracing::info!(attacker=ctx.attacker_id,target=hit.target_peer_id,
                    hit_id=hit.hit_id,parent_cid=hit.parent_cid,bone=hit.bone_str(),source=hit.dism_blunt & damage::SOURCE_MASK,
                    stage="decision",reason,"stuck blade rejected"),
                _=>{},
            }
            self.decisions.insert(key, d);
            return verdict;
        }
        if sanitize_deltas(hit) > 0 {
            cheat::bump(ctx.attacker_id, CheatKind::DamageClamped);
        }
        let mut reason = validate(lc, ctx, hit, now_ms);
        if reason.is_none() && !self.take_token(ctx.attacker_id, now_ms, hit.flags & damage::FLAG_COMPLEX != 0) {
            reason = Some("rate_limited: rate-limited".into());
        }
        let mut d = Decision {
            accepted: false,
            reason: String::new(),
            decided_at: now_ms,
            first_at: now_ms,
            target_acked: false,
            owner_outcome: None,
            waiting: None,
            pending: None,
            grace_ms: 0,
            parryable: true,
            approved: None,
            forwarded_at: None,
            reforwarded: false,
            confirmed: false,
        };
        let verdict = match reason {
            Some(r) => {
                d.reason = r.clone();
                Verdict::Ack { accepted: false, reason: r }
            }
            None => {
                d.waiting = Some(*hit);
                match settle_waiting(lc, ctx.attacker_id, &mut d, now_ms) {
                    Some((v, ev)) => { *hit = ev; v }
                    None => Verdict::Hold,
                }
            }
        };
        self.decisions.insert(key, d);
        verdict
    }

    /// Held hits whose defender grace is over and claims whose stream
    /// coverage arrived: (attacker, event, Forward | Ack).
    pub fn flush_pending(&mut self, lc: &mut Store, now_ms: i64) -> Vec<(PeerId, DamageEvent, Verdict)> {
        // Only the waiting / held decisions (never a walk of the whole map).
        let mut out = Vec::new();
        let keys: Vec<(PeerId, u32)> = self.active.iter().copied().collect();
        for key in keys {
            let attacker = key.0;
            if let Some(hit) = self.decisions.get(&key).and_then(|d| d.pending) {
                if let Some(reason) = self.rejected_parent(attacker, &hit) {
                    tracing::info!(attacker,target=hit.target_peer_id,hit_id=hit.hit_id,parent_cid=hit.parent_cid,
                        bone=hit.bone_str(),source=hit.dism_blunt & damage::SOURCE_MASK,stage="delivery",reason,
                        "stuck blade rejected");
                    self.reject_decision(attacker, key.1, &reason);
                    self.active.remove(&key);
                    out.push((attacker, hit, Verdict::Ack { accepted: false, reason }));
                    continue;
                }
            }
            if self.decisions.get(&key).and_then(|d| d.pending.as_ref())
                .map_or(false, |h| self.blocks_delivery(key, h)) { continue; }
            let Some(d) = self.decisions.get_mut(&key) else { self.active.remove(&key); continue };
            let result = if d.waiting.is_some() {
                settle_waiting(lc, attacker, d, now_ms)
            } else if d.pending.is_some() {
                resolve(lc, attacker, d, now_ms)
            } else { None };
            let done = d.waiting.is_none() && d.pending.is_none();
            if let Some((v, hit)) = result {
                if let Verdict::Ack { accepted: false, reason } = &v {
                    self.reject_decision(attacker, hit.hit_id, reason);
                }
                if v == Verdict::Forward && self.blocks_delivery(key, &hit) {
                    self.defer_delivery(key, hit);
                    continue;
                }
                if hit.parent_cid!=0 && v==Verdict::Forward {
                    tracing::info!(attacker,target=hit.target_peer_id,hit_id=hit.hit_id,parent_cid=hit.parent_cid,
                        bone=hit.bone_str(),source=hit.dism_blunt & damage::SOURCE_MASK,stage="delivery","stuck blade delivered");
                }
                if v != Verdict::Hold { out.push((attacker, hit, v)); }
            }
            if done { self.active.remove(&key); }
        }
        out.sort_by_key(|(a, h, _)| (*a, h.hit_id)); // deterministic order
        out
    }

    /// See the global `reject_decision`.
    pub fn reject_decision(&mut self, attacker: PeerId, hit_id: u32, reason: &str) {
        if let Some(d) = self.decisions.get_mut(&(attacker, hit_id)) {
            d.accepted = false;
            d.reason = reason.to_string();
            d.pending = None;
            d.waiting = None;
        }
        self.inside.retain(|(a, _), open| *a != attacker || open.parent.hit_id != hit_id);
    }

    /// The owner acked `(attacker, hit_id)` (trusted caller: the combat
    /// simulator, which knows the ack comes from the victim). True if the
    /// attacker should now get its final positive FINAL `damage_verdict`.
    #[allow(dead_code)]
    pub fn on_owner_ack(&mut self, lc: &mut Store, attacker: PeerId, hit_id: u32, now_ms: i64) -> bool {
        let Some(victim) = self.decisions.get(&(attacker, hit_id))
            .and_then(|d| d.approved.as_ref()).map(|ev| ev.target_peer_id) else { return false };
        self.on_owner_ack_by(lc, victim, attacker, hit_id, now_ms)
    }

    /// `acker` acked `(attacker, hit_id)`. Only the hit's victim (the peer the
    /// approved event was forwarded to) may ack it, and only once it was
    /// actually forwarded (not while held for the defender grace): anyone
    /// else's ack would end the re-delivery and poison the victim's RTT
    /// sample. True if the attacker should now get its final
    /// positive FINAL `damage_verdict`.
    pub fn on_owner_ack_by(&mut self, lc: &mut Store, acker: PeerId, attacker: PeerId, hit_id: u32, now_ms: i64) -> bool {
        match self.decisions.get_mut(&(attacker, hit_id)) {
            Some(d) if d.accepted && d.pending.is_none() && d.waiting.is_none()
                && d.approved.as_ref().map_or(false, |ev| ev.target_peer_id == acker) => {
                if !d.target_acked && !d.reforwarded {
                    // Owner RTT sample for the lag-comp view prediction (its
                    // sidecar acks on receipt).
                    if let (Some(at), Some(ev)) = (d.forwarded_at, &d.approved) {
                        lc.note_rtt(ev.target_peer_id, (now_ms - at) as f32, now_ms);
                    }
                }
                d.target_acked = true;
                true
            }
            _ => false,
        }
    }

    /// Authenticated, idempotent native attempt receipt. Some(true) is new,
    /// Some(false) is the exact same cached outcome; None is invalid/conflicting.
    pub fn on_replay_outcome_by(&mut self,victim:PeerId,r:hsmp_ipc::schema::combat::ReplayOutcome)->Option<bool> {
        let d=self.decisions.get_mut(&(r.attacker,r.hit_id))?;
        let hit=d.approved.as_ref()?;
        if !d.accepted || d.waiting.is_some() || d.pending.is_some() || d.forwarded_at.is_none()
            || hit.target_peer_id!=victim || hit.match_id!=r.match_id || hit.round!=r.round
            || hit.victim_life!=r.victim_life {return None;}
        if let Some(previous)=d.owner_outcome {
            return (hsmp_ipc::bytemuck::bytes_of(&previous)==hsmp_ipc::bytemuck::bytes_of(&r)).then_some(false);
        }
        d.owner_outcome=Some(r);Some(true)
    }

    /// Cosmetic replay requires the owner's native result, never its sidecar's
    /// transport ACK. The glue calls this only for a fresh immutable outcome.
    pub fn native_effect_hit(&self,victim:PeerId,attacker:PeerId,hit_id:u32)->Option<DamageEvent> {
        let d=self.decisions.get(&(attacker,hit_id))?;
        let hit=d.approved?;
        let outcome=d.owner_outcome?;
        (d.accepted && d.forwarded_at.is_some() && d.pending.is_none() && d.waiting.is_none()
            && hit.target_peer_id==victim && outcome.status==hsmp_ipc::schema::combat::REPLAY_CHANGED
            // FIELDS 0..14 and 18 are native injury/vitals. Sustained Damage,
            // Damage Taken and Last Damage Taken (15..17) only bookkeep gates.
            && outcome.observed_fields & (0x7fff | (1<<18))!=0 && hit.flags & damage::FLAG_COMPLEX!=0
            && hit.flags & damage::FLAG_INSIDE==0).then_some(hit)
    }

    /// One-shot: true the first time `(attacker, hit_id)` is known to be
    /// geometrically accepted (forwarded, or held for the defender grace).
    /// The glue then sends the attacker an early S2CDamageAck{accepted: true,
    /// reason: CONFIRM_REASON} (its hit-confirm cue arrives ≤ RTT); a parry
    /// that cancels a held hit later arrives as the final "parried" ack.
    pub fn take_confirm(&mut self, attacker: PeerId, hit_id: u32) -> bool {
        match self.decisions.get_mut(&(attacker, hit_id)) {
            Some(d) if d.accepted && d.waiting.is_none() && !d.confirmed => { d.confirmed = true; true }
            _ => false,
        }
    }
}

pub fn on_replay_outcome(victim:PeerId,r:hsmp_ipc::schema::combat::ReplayOutcome)->Option<bool> {
    engine().lock().unwrap().on_replay_outcome_by(victim,r)
}

pub fn native_effect_hit(victim:PeerId,attacker:PeerId,hit_id:u32)->Option<DamageEvent> {
    engine().lock().unwrap().native_effect_hit(victim,attacker,hit_id)
}

/// Judge a claim that passed the field checks: lag comp (waiting while the
/// attacker's own stream does not cover its hit yet, at most WAIT_MAX),
/// clash cancellation, defender hold, damage cap. None = still waiting.
/// On a decision `d` is filled in and the waiting claim consumed.
fn settle_waiting(lc: &mut Store, attacker: PeerId, d: &mut Decision, now_ms: i64) -> Option<(Verdict, DamageEvent)> {
    let mut hit = d.waiting?;
    let allow_lead = now_ms - d.first_at >= WAIT_MAX_MS;
    let mut reason: Option<String> = None;
    let mut hold = false;
    let mut speed = None;
    let mut unarmed = false;
    let mut rel: Option<f32> = None;
    let mut rel_exact = true;
    let mut peak: Option<f32> = None;
    let mut geometry_accepted=false;
    let contexts_ready = hit.match_id==0 || (lc.has_pose_context(attacker,hit.match_id,hit.round,hit.attacker_life)
        && lc.has_pose_context(hit.target_peer_id,hit.match_id,hit.round,hit.victim_life));
    if !contexts_ready && !allow_lead {return None;}
    let evaluated=if contexts_ready {lc.evaluate_opts(attacker,&hit,now_ms,allow_lead)}
        else {Eval::Reject("stale_life: scoped source pose unavailable".into())};
    match evaluated {
        Eval::Wait(_) => return None,
        Eval::Reject(r) => {
            if crate::validate::rate::log_ok("lagcomp_reject") { tracing::info!(attacker, target = hit.target_peer_id, hit_id = hit.hit_id, "lagcomp reject: {}", r); }
            cheat::bump(attacker, CheatKind::GeometryReject);
            reason = Some(r);
        }
        Eval::Accept(i) => {
            geometry_accepted=true;
            speed = i.contact_speed;
            peak = i.peak_speed.or(i.contact_speed);
            unarmed = i.unarmed;
            rel = i.rel_speed;
            rel_exact = i.rel_exact;
            if let Some(hb) = i.hit_box { hit.hit_box_frame = hb; }
            if let Some((cm, dot)) = i.proxy_box_error {
                if (cm > lagcomp::BODY_TOL || dot < 0.99) && crate::validate::rate::log_ok("proxy_box_error") {
                    tracing::info!(attacker, target = hit.target_peer_id, hit_id = hit.hit_id, center_cm = cm, rotation_dot = dot,
                        "proxy box frame differs from the owner's bone (pose sync); replayed on the owner's frame");
                }
            }
            if i.view_clamped {
                tracing::debug!(attacker, hit_id = hit.hit_id, hint = hit.victim_view_ts,
                    used = i.view_ts, "lagcomp: view hint clamped to the server prediction");
            }
            if !i.unarmed && lc.parried(hit.target_peer_id, attacker, hit.attacker_ts, now_ms) {
                // The clash report already arrived: cancel without waiting.
                if crate::validate::rate::log_ok("lagcomp_parry") {
                    tracing::info!(attacker, target = hit.target_peer_id, hit_id = hit.hit_id,
                        "lagcomp parry: hit cancelled by weapon clash");
                }
                reason = Some("parried".into());
            } else if !i.unarmed && i.parry_possible {
                if crate::validate::rate::log_ok("lagcomp_accept") {
                    tracing::info!(attacker, target = hit.target_peer_id, hit_id = hit.hit_id,
                        rewind_ms = i.rewind_ms, body_dist = i.body_dist, weapon_dist = i.weapon_dist,
                        "lagcomp accept (held: parry plausible)");
                }
                hold = true;
            } else if crate::validate::rate::log_ok("lagcomp_accept") {
                tracing::info!(attacker, target = hit.target_peer_id, hit_id = hit.hit_id,
                    rewind_ms = i.rewind_ms, body_dist = i.body_dist, weapon_dist = i.weapon_dist,
                    "lagcomp accept (no parry possible: forwarded now)");
            }
        }
        Eval::NoData(_) => {}
    }
    d.waiting = None;
    d.parryable = !unarmed;
    d.decided_at = now_ms;
    let source_class=if reason.is_none() {
        match damage::accepted_source_class(attacker,&hit,unarmed,geometry_accepted) {
            Ok(class)=>Some(class),
            Err(why)=>{reason=Some(why.into());None},
        }
    } else {None};
    let accepted = reason.is_none();
    if accepted {
        let class=source_class.expect("accepted damage has authenticated envelope");
        // The server caps the damage; the attacker's figure is a claim.
        damage::clamp_hit_with_class(attacker, &mut hit, speed, class);
        // Solo parity: the owner replays these natively; bound them by the
        // physics of THIS contact (real relative speed).
        let len = |v: [f32; 3]| (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
        let (v0, i0, vrs) = (len(hit.velocity), len(hit.impulse), hit.raw_damage);
        let c = damage::clamp_impact_with_class(&mut hit, peak, rel, rel_exact, class);
        if hit.flags & damage::FLAG_COMPLEX != 0 && crate::validate::rate::log_ok("impact_rescale") {
            // (`class`: the hit_vel_factor calibration groups these lines by it)
            tracing::info!(attacker, target = hit.target_peer_id, hit_id = hit.hit_id, bone = hit.bone_str(), class = ?class,
                vel_claimed = v0, vel_forwarded = len(hit.velocity), imp_claimed = i0, imp_forwarded = len(hit.impulse),
                standin_rel = vrs, server_striking = ?speed, server_peak = ?peak, server_relative = ?rel, rel_exact, factor = c.factor,
                cut = hit.cutting_power, rig = hit.damage_out, "combat: impact rescale");
        }
        // COMPLEX|INSIDE with no parent is an authenticated penetration origin
        // whose native DCD gate suppressed damage. It may bind a constraint,
        // but the owner acknowledges it without applying/painting a blow.
        if hit.flags & (damage::FLAG_COMPLEX | damage::FLAG_INSIDE) == (damage::FLAG_COMPLEX | damage::FLAG_INSIDE) {
            hit.damage_out = 0.0;
            hit.set_deltas(&[]);
        }
    }
    let held = accepted && hold;
    d.accepted = accepted;
    d.reason = reason.clone().unwrap_or_default();
    d.grace_ms = if held { lc.grace_ms(hit.target_peer_id, attacker, now_ms) } else { 0 };
    d.pending = if held { Some(hit) } else { None };
    d.approved = if accepted { Some(hit) } else { None };
    d.forwarded_at = if accepted && !held { Some(now_ms) } else { None };
    let v = match reason {
        None if held => {
            // The victim's own stream may already rule a parry out.
            if let Some(r) = resolve(lc, attacker, d, now_ms) { return Some(r); }
            Verdict::Hold
        }
        None => Verdict::Forward,
        Some(r) => Verdict::Ack { accepted: false, reason: r },
    };
    Some((v, hit))
}

/// Resolve a held hit once its defender grace has elapsed: parried ->
/// rejected (Ack false), else forwarded. Returns the verdict and the event.
fn resolve(lc: &mut Store, attacker: PeerId, d: &mut Decision, now_ms: i64) -> Option<(Verdict, DamageEvent)> {
    if now_ms - d.decided_at < d.grace_ms {
        // Early release: the victim's own stream already shows its blade was
        // nowhere near the attack while it was on its screen.
        let h = d.pending.as_ref()?;
        if lc.parry_window_clear(h.target_peer_id, attacker, h.attacker_ts, now_ms) != Some(true) {
            return None;
        }
    }
    let hit = d.pending.take()?;
    d.decided_at = now_ms; // FORWARD_TTL counts from the actual forward
    if d.parryable && lc.parried(hit.target_peer_id, attacker, hit.attacker_ts, now_ms) {
        if crate::validate::rate::log_ok("lagcomp_parry") {
            tracing::info!(attacker, target = hit.target_peer_id, hit_id = hit.hit_id,
                "lagcomp parry: hit cancelled by weapon clash");
        }
        d.accepted = false;
        d.reason = "parried".into();
        return Some((Verdict::Ack { accepted: false, reason: d.reason.clone() }, hit));
    }
    d.forwarded_at = Some(now_ms);
    Some((Verdict::Forward, hit))
}

// ---- global engine (server) ---------------------------------------------------

/// Decide what to do with a `damage` claim record (first arrival or resend). On
/// Forward / Reforward `hit` has been replaced by the server-approved event
/// (damage capped by the plausibility model): forward THAT to the owner.
pub fn on_damage(ctx: &Ctx, hit: &mut DamageEvent) -> Verdict {
    on_damage_at(ctx, hit, Instant::now())
}

fn on_damage_at(ctx: &Ctx, hit: &mut DamageEvent, now: Instant) -> Verdict {
    let mut e = engine().lock().unwrap();
    lagcomp::with_store(|lc| e.on_damage(lc, ctx, hit, lagcomp::ms_at(now)))
}

/// Held hits whose defender grace is over: (attacker, event, Forward | Ack).
/// Called from the server tick so hits land even if resends were lost.
pub fn flush_pending() -> Vec<(PeerId, DamageEvent, Verdict)> {
    flush_pending_at(Instant::now())
}

fn flush_pending_at(now: Instant) -> Vec<(PeerId, DamageEvent, Verdict)> {
    let mut e = engine().lock().unwrap();
    lagcomp::with_store(|lc| e.flush_pending(lc, lagcomp::ms_at(now)))
}

/// Turn a not-yet-delivered decision into a sticky rejection (the round
/// ended or the target died while the hit was held / in flight), so the
/// attacker's resends get the same answer.
pub fn reject_decision(attacker: PeerId, hit_id: u32, reason: &str) {
    engine().lock().unwrap().reject_decision(attacker, hit_id, reason)
}

/// Peer left: drop its claim rate bucket (decisions expire by TTL).
pub fn engine_forget(peer: PeerId) {
    let mut e = engine().lock().unwrap();
    e.buckets.remove(&peer);
    e.complex_buckets.remove(&peer);
    e.claim_buckets.remove(&peer);
}

/// See `Engine::take_confirm`.
pub fn take_confirm(attacker: PeerId, hit_id: u32) -> bool {
    engine().lock().unwrap().take_confirm(attacker, hit_id)
}

/// Peer `acker` acked `(attacker, hit_id)`. Returns true if the attacker
/// should now get its positive FINAL `damage_verdict` (the hit was accepted and the
/// acker is its victim; see `Engine::on_owner_ack_by`).
pub fn on_owner_ack(acker: PeerId, attacker: PeerId, hit_id: u32) -> bool {
    let mut e = engine().lock().unwrap();
    lagcomp::with_store(|lc| e.on_owner_ack_by(lc, acker, attacker, hit_id, lagcomp::now_ms()))
}

// ---- authoritative health ledger --------------------------------------------

/// Index of `Health` in the shared HSMPCombat FIELDS table.
pub const FIELD_HEALTH: u8 = 0;
/// Health before the first vitals of a round arrives (MP restores the pawn
/// to the Willie_BP defaults at every spawn; Willie Health is 100).
pub const HP_DEFAULT: f32 = 100.0;
/// Vitals above this are clamped (no legit Willie has more).
pub const HP_CAP: f32 = 150.0;
/// Per-hit Health loss counted by the ledger (one blow can kill: ≤ 100 hp).
pub const HIT_LOSS_CAP: f32 = 100.0;
/// A vitals packet arriving this long after the owner acked a hit reflects it.
pub const REFLECT_AFTER: Duration = Duration::from_millis(250);
/// Max believable natural Health regeneration (anti-cheat ceiling slack).
/// The vitals ceiling only bounds HEALING (Willie Health does not
/// regenerate in solo play). It is not "the last report minus the
/// server's estimate of each hit": Half Sword floors Health per body part
/// (a torso hit never takes it below 5, a limb hit below 15, and a hit at or
/// below the floor leaves it unchanged) and gates repeated contacts, so an
/// honest owner often keeps reporting 100 after hits the estimate books at
/// 5-10 HP each. A ceiling built from the estimate shows peers a falling bar
/// the victim never had (one playtest logged 169 such clamps of honest
/// owners at 100 HP).
pub const REGEN_PER_S: f32 = 1.0;
/// An owner without vitals for this long is stalled: the ledger's booked
/// losses may then declare its death (vitals heartbeat is 1 s; one lost
/// heartbeat on a lossy link must not look like a stall, so two of them +
/// slack).
pub const OWNER_STALL: Duration = Duration::from_millis(2500);
/// God-mode rule: once the server-validated damage of a life is
/// lethal on its own, an owner that keeps reporting itself alive for this
/// much longer is declared dead. The hits counted are already REFLECT_AFTER
/// past their ack (or FORWARD_TTL past an unacked forward), and an honest
/// owner whose replays were lethal reports its death at once (dead flag:
/// VitalsGate sends flag changes immediately, plus the reliable C2SDeath),
/// so this only absorbs a vitals frame that was produced before the replay.
/// Honest safety rests on the bound being an under-estimate, not on the grace.
pub const GODMODE_GRACE: Duration = Duration::from_millis(300);
/// The god-mode rule DETECTS (cheat counter `GodMode`, one
/// warning per life) but does not kill unless a server enables it
/// (`Ledger::enforce_godmode`). Its premise, "an honest owner's replays take
/// at least the server's estimate, so it is dead before the bound reaches
/// 0", is false for Half Sword: Health never drops below the hit part's
/// floor (thr 1..15) through Get Damage, contacts within 0.1-0.2 s on a bone
/// are gated, and death comes from Snap Neck, dismemberment or the
/// consciousness cap, not from summed Health loss. Enforced in a playtest,
/// the rule ended all three rounds by killing an owner that was at 92, 44
/// and 100 HP in its own game.
pub const GODMODE_ENFORCE_DEFAULT: bool = false;
/// Natural Health regeneration credited to the owner by the god-mode bound.
/// Willie Health does not regenerate in solo play (only consciousness and
/// stamina do); this is slack for float drift / a game update, small enough
/// that it cannot outpace a fight (REGEN_PER_S, 20, is the ceiling's slack
/// for what PEERS are shown and would hide any sustained damage).
pub const GODMODE_REGEN_PER_S: f32 = 0.5;
/// Share of a non-armour-stage claim's booked loss the god-mode bound
/// trusts. Those claims are replayed through the victim's Get Damage with the
/// stand-in's raw input; the victim's own height factor may be as low as
/// HM_MIN where the stand-in's was HM_MAX. (Armour-stage claims are booked at
/// the replay's own HM_MIN already.)
pub const GODMODE_SIMPLE_TRUST: f32 = damage::HM_MIN / damage::HM_MAX;

/// Health loss the ledger books for one hit: the attacker-measured Health
/// delta, or (no deltas at all) its damage_out. Armour-stage claims
/// (FLAG_COMPLEX): the owner's replay decides the damage, so the booking must
/// not exceed it (an over-booking lowers the vitals ceiling below the owner's
/// real Health and peers are shown less than its HUD): the smaller of the
/// stand-in measurement and the server's own replay estimate at HM_MIN.
pub fn hit_loss(hit: &DamageEvent) -> f32 {
    let d = hit.deltas().iter().find(|d| d.i == FIELD_HEALTH).map(|d| d.v);
    let mut loss = match d {
        Some(v) => -v,
        None if hit.deltas().is_empty() && hit.flags & damage::FLAG_COMPLEX == 0 => hit.damage_out,
        None => 0.0,
    };
    if hit.flags & damage::FLAG_COMPLEX != 0 {
        let est = damage::replay_loss(hit, damage::armour_on(hit.target_peer_id, hit.bone_str()), damage::HM_MIN);
        loss = if d.is_some() { loss.min(est) } else { est };
    }
    if loss.is_finite() { loss.clamp(0.0, HIT_LOSS_CAP) } else { 0.0 }
}

/// Health loss of an accepted hit the owner's own replay surely did at least
/// (the god-mode bound): `hit_loss` for armour-stage claims (booked at the
/// replay's HM_MIN), a GODMODE_SIMPLE_TRUST share of it otherwise.
pub fn trusted_loss(hit: &DamageEvent) -> f32 {
    if hit.flags & damage::FLAG_COMPLEX != 0 {
        // The server's own replay of the approved armour-stage inputs through
        // the victim's armour at the weakest Willie (HM_MIN): the owner's
        // native replay of the same inputs takes at least this. (The ledger's
        // booking also takes the min with the stand-in's servo-noisy
        // measurement, which under-reads by ~2×: too slack for this bound.)
        let est = damage::replay_loss(hit, damage::armour_on(hit.target_peer_id, hit.bone_str()), damage::HM_MIN);
        if est.is_finite() { est.clamp(0.0, HIT_LOSS_CAP) } else { 0.0 }
    } else {
        hit_loss(hit) * GODMODE_SIMPLE_TRUST
    }
}

struct LedgerHit {
    attacker: PeerId,
    hit_id: u32,
    loss: f32,
    /// `trusted_loss` (god-mode bound).
    trusted: f32,
    fwd_at: Instant,
    acked_at: Option<Instant>,
    /// Already counted into `Life::taken` (forwarded FORWARD_TTL ago, never acked).
    counted: bool,
}

struct Life {
    round: u32,
    /// Start of this life (the round), for the god-mode regen credit.
    start_at: Instant,
    /// First alive report of the life (≤ HP_CAP): the god-mode bound's base.
    start_hp: Option<f32>,
    /// Σ trusted loss of the hits the owner surely applied (acked and
    /// REFLECT_AFTER past, or forwarded FORWARD_TTL ago and never acked).
    taken: f32,
    /// When the god-mode bound (start − taken + regen) first reached 0 while
    /// the owner still reported itself alive.
    lethal_since: Option<Instant>,
    /// The life's death was decided (owner reported it, or the god-mode rule).
    forced: bool,
    /// Accepted owner-reported Health (None until the round's first vitals).
    base: Option<f32>,
    base_at: Instant,
    /// Vitals seq at the round start: anything not newer belongs to an
    /// earlier round (reordered UDP) and is ignored.
    seq_floor: Option<u32>,
    last_seq: Option<u32>,
    /// Owner reported itself alive in this round (gates "dead" vitals).
    alive_seen: bool,
    unreflected: Vec<LedgerHit>,
    last_attacker: PeerId,
    /// A hit was forwarded in this life (before that, any Health up to
    /// HP_CAP is believed: the spawn heal runs after the first reports).
    hit_seen: bool,
    /// The god-mode bound reached 0 while the owner kept reporting alive
    /// (counted once per life; a kill only with `enforce_godmode`).
    godmode_flagged: bool,
    /// Most Health the owner may report (the vitals ceiling): HP_CAP at the
    /// round start, + regen, − every hit's booked loss ONCE when surely
    /// reflected, capped after each report at report + not-yet-reflected.
    allow: f32,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LedgerVerdict {
    pub est: f32,
    pub lethal: bool,
    /// The death is the SERVER's verdict against an owner that still reports
    /// itself alive (god-mode rule): cause DEATH_DAMAGE, cheat counter.
    pub forced: bool,
    /// The Health peers are shown: the report clamped to the ledger ceiling.
    pub hp: f32,
}

pub struct Ledger {
    lives: HashMap<PeerId, Life>,
    /// Kill an owner whose god-mode bound stays ≤ 0 (see GODMODE_ENFORCE_DEFAULT).
    pub enforce_godmode: bool,
}

impl Default for Ledger {
    fn default() -> Self { Ledger { lives: HashMap::new(), enforce_godmode: GODMODE_ENFORCE_DEFAULT } }
}

/// God-mode suspicion without enforcement: one counter bump and one warning
/// per life (the operator sees it; the round is not decided by an estimate).
fn flag_godmode(l: &mut Life, peer: PeerId, round: u32) {
    if l.godmode_flagged { return; }
    l.godmode_flagged = true;
    crate::validate::cheat::bump(peer, crate::validate::cheat::Kind::GodMode);
    tracing::warn!(peer, round, taken = l.taken,
        "god-mode suspect: server-estimated damage exceeds a Health bar while the owner reports alive (not enforced)");
}

fn seq_newer(a: u32, b: u32) -> bool {
    let d = a.wrapping_sub(b);
    d != 0 && d < u32::MAX / 2
}

impl Ledger {
    fn life(&mut self, peer: PeerId, round: u32, now: Instant) -> &mut Life {
        let l = self.lives.entry(peer).or_insert_with(|| Life {
            round, start_at: now, start_hp: None, taken: 0.0, lethal_since: None, forced: false,
            base: None, base_at: now, seq_floor: None, last_seq: None,
            alive_seen: false, unreflected: Vec::new(), last_attacker: 0, allow: HP_CAP,
            hit_seen: false, godmode_flagged: false,
        });
        if l.round != round {
            l.round = round;
            l.start_at = now;
            l.start_hp = None;
            l.taken = 0.0;
            l.forced = false;
            l.lethal_since = None;
            l.base = None;
            l.base_at = now;
            l.seq_floor = l.last_seq;
            l.alive_seen = false;
            l.unreflected.clear();
            l.last_attacker = 0;
            l.allow = HP_CAP;
            l.hit_seen = false;
            l.godmode_flagged = false;
        }
        l
    }

    fn est(l: &Life) -> f32 {
        l.base.unwrap_or(HP_DEFAULT) - l.unreflected.iter().map(|h| h.loss).sum::<f32>()
    }

    /// God-mode bound of a life: the most Health the owner can still have
    /// after the hits it surely applied (server-validated, solo-parity
    /// under-estimates), with a small regen credit.
    fn bound(l: &Life, now: Instant) -> f32 {
        let regen = GODMODE_REGEN_PER_S * now.duration_since(l.start_at).as_secs_f32();
        (l.start_hp.unwrap_or(HP_DEFAULT) + regen).min(HP_CAP) - l.taken
    }

    /// An accepted hit was forwarded to its owner (first time only).
    pub fn on_forward(&mut self, attacker: PeerId, round: u32, hit: &DamageEvent, now: Instant) -> LedgerVerdict {
        let loss = hit_loss(hit);
        let trusted = trusted_loss(hit);
        let l = self.life(hit.target_peer_id, round, now);
        l.last_attacker = attacker;
        l.hit_seen = true;
        if (loss > 0.0 || trusted > 0.0) && !l.unreflected.iter().any(|h| h.attacker == attacker && h.hit_id == hit.hit_id) {
            l.unreflected.push(LedgerHit { attacker, hit_id: hit.hit_id, loss, trusted, fwd_at: now, acked_at: None, counted: false });
        }
        let est = Self::est(l);
        // Solo parity: the owner's native replay decides its damage, the
        // booked losses are only the attacker's stand-in measurement. They kill
        // only an owner that stopped reporting vitals (stalled / alt-tabbed).
        // An owner that keeps reporting while ignoring its damage is caught
        // by the god-mode bound in `on_vitals`.
        let stalled = now.duration_since(l.base_at) >= OWNER_STALL;
        LedgerVerdict { est, lethal: est <= 0.0 && stalled, forced: false, hp: l.base.unwrap_or(HP_DEFAULT) }
    }

    /// The owner's sidecar acked the hit (it is about to be applied in-game).
    pub fn on_ack(&mut self, victim: PeerId, attacker: PeerId, hit_id: u32, now: Instant) {
        if let Some(l) = self.lives.get_mut(&victim) {
            for h in l.unreflected.iter_mut() {
                if h.attacker == attacker && h.hit_id == hit_id && h.acked_at.is_none() {
                    h.acked_at = Some(now);
                }
            }
        }
    }

    /// Owner vitals (unreliable, latest-wins by seq). Returns None for stale
    /// or reordered packets.
    pub fn on_vitals(&mut self, peer: PeerId, round: u32, seq: u32, hp: f32, dead: bool, now: Instant) -> Option<LedgerVerdict> {
        if !hp.is_finite() { return None; }
        let enforce = self.enforce_godmode;
        let l = self.life(peer, round, now);
        if let Some(last) = l.last_seq {
            let restarted = last.wrapping_sub(seq) > 1000 && last.wrapping_sub(seq) < u32::MAX / 2;
            if !seq_newer(seq, last) && !restarted { return None; }
            if restarted { l.seq_floor = None; }
        }
        l.last_seq = Some(seq);
        if let Some(floor) = l.seq_floor {
            if !seq_newer(seq, floor) { return None; }
        }
        let owner_dead = dead || hp <= 0.0;
        if owner_dead && !l.alive_seen {
            // Not yet seen alive this round (leftover corpse of the previous
            // round / still respawning): carries no information.
            let est = Self::est(l);
            return Some(LedgerVerdict { est, lethal: false, forced: false, hp: hp.min(HP_CAP) });
        }
        // Hits applied before this vitals was produced.
        let mut reflected = 0.0f32;
        let mut taken = 0.0f32;
        l.unreflected.retain_mut(|h| match h.acked_at {
            Some(t) if now.duration_since(t) >= REFLECT_AFTER => {
                reflected += h.loss;
                if !h.counted { taken += h.trusted; }
                false
            }
            // Forwarded, never acked: the owner's sidecar acks on receipt, so
            // a reliable damage_in unacked after FORWARD_TTL is an owner that
            // drops its damage (god mode by not acking). Count it now.
            None if !h.counted && now.duration_since(h.fwd_at) >= FORWARD_TTL => {
                h.counted = true;
                taken += h.trusted;
                true
            }
            _ => true,
        });
        l.taken += taken;
        // The life's starting Health: a fresh Willie's (HP_DEFAULT; every MP
        // spawn restores the Willie_BP defaults). Never the owner's own first
        // report: a liar's "150" would buy it 50 % more server-validated damage
        // before the god-mode rule fires. Never less either: that
        // report may already include hits that `taken` counts too.
        if !owner_dead && l.start_hp.is_none() { l.start_hp = Some(HP_DEFAULT); }
        let reported = hp.min(HP_CAP);
        let _ = reflected;
        // Ceiling: the owner's report is the truth peers are shown
        // (solo parity: the victim's own native replay decides its Health).
        // Only HEALING is bounded: once a hit was forwarded this life, Health
        // may rise at most REGEN_PER_S above the last accepted value. A report
        // that stays high after estimated damage is NOT cut (Half Sword's
        // per-part Health floors and contact gates make that honest).
        let ceiling = if l.hit_seen {
            let regen = REGEN_PER_S * now.duration_since(l.base_at).as_secs_f32();
            (l.base.unwrap_or(HP_DEFAULT) + regen).min(HP_CAP)
        } else {
            HP_CAP
        };
        if reported > ceiling + 0.5 {
            tracing::warn!(peer, round, reported, ceiling, "vitals: Health rose faster than any heal; clamped");
        }
        l.base = Some(reported.min(ceiling));
        l.base_at = now;
        l.allow = ceiling;
        if !owner_dead { l.alive_seen = true; }
        let est = Self::est(l);
        // Fresh vitals are the truth (solo parity): the owner's own death
        // counts here; the ceiling still clamps what peers are shown...
        let mut forced = false;
        if owner_dead {
            l.forced = true; // its own death: the god-mode rule has nothing to add
        } else if !l.forced {
            // ...unless the damage the owner surely applied is lethal by itself
            // and it keeps reporting alive past the grace (god mode).
            if Self::bound(l, now) <= 0.0 {
                match l.lethal_since {
                    None => l.lethal_since = Some(now),
                    Some(t) if now.duration_since(t) >= GODMODE_GRACE => {
                        if enforce { forced = true; l.forced = true; }
                        else { flag_godmode(l, peer, round); }
                    }
                    Some(_) => {}
                }
            } else {
                l.lethal_since = None;
            }
        }
        Some(LedgerVerdict { est, lethal: (owner_dead && l.alive_seen) || forced, forced, hp: l.base.unwrap_or(HP_DEFAULT) })
    }

    /// The god-mode rule flagged `peer`'s current life (detection only unless
    /// `enforce_godmode`).
    #[allow(dead_code)] // hsmp-combat-sim and the tests
    pub fn godmode_flagged(&self, peer: PeerId) -> bool {
        self.lives.get(&peer).map_or(false, |l| l.godmode_flagged)
    }

    /// God-mode bound of `peer` this round (diagnostics / tests).
    #[allow(dead_code)] // hsmp-combat-sim and the tests
    pub fn godmode_bound(&self, peer: PeerId, now: Instant) -> Option<f32> {
        self.lives.get(&peer).map(|l| Self::bound(l, now))
    }

    /// Server tick: the god-mode rule without waiting for the owner's next
    /// vitals (a god-mode client reports an unchanging Health, so only its
    /// 1 s heartbeat). Every life seen alive in `round` whose bound —
    /// counting the hits that are surely applied by now — has been ≤ 0 for
    /// GODMODE_GRACE without the owner reporting its death is returned
    /// (once). An honest owner whose replays were that lethal is dead and
    /// says so within the grace; declaring it is correct either way.
    pub fn sweep(&mut self, round: u32, now: Instant) -> Vec<(PeerId, LedgerVerdict)> {
        let mut ids: Vec<PeerId> = self.lives.keys().copied().collect();
        ids.sort_unstable();
        ids.into_iter().filter_map(|p| self.sweep_one(p, round, now).map(|v| (p, v))).collect()
    }

    /// `sweep` for one player.
    pub fn sweep_one(&mut self, peer: PeerId, round: u32, now: Instant) -> Option<LedgerVerdict> {
        let l = self.lives.get_mut(&peer)?;
        if l.round != round || !l.alive_seen || l.forced { return None; }
        let pending: f32 = l.unreflected.iter().filter(|h| !h.counted && match h.acked_at {
            Some(t) => now.duration_since(t) >= REFLECT_AFTER,
            None => now.duration_since(h.fwd_at) >= FORWARD_TTL,
        }).map(|h| h.trusted).sum();
        if Self::bound(l, now) - pending > 0.0 { l.lethal_since = None; return None; }
        let since = *l.lethal_since.get_or_insert(now);
        if now.duration_since(since) < GODMODE_GRACE { return None; }
        if !self.enforce_godmode {
            flag_godmode(l, peer, round);
            return None;
        }
        l.forced = true;
        Some(LedgerVerdict { est: Self::est(l), lethal: true, forced: true, hp: l.base.unwrap_or(HP_DEFAULT) })
    }

    /// Live transition: start every known peer's round now, so vitals sent
    /// before this instant (seq ≤ current) can never count for the new round.
    pub fn begin_round(&mut self, round: u32, now: Instant) {
        let ids: Vec<PeerId> = self.lives.keys().copied().collect();
        for id in ids {
            // Tests only: the ledger is process-global; peers with ids from
            // ISOLATED_TEST_IDS on belong to tests that run rounds of their own.
            #[cfg(test)]
            if id >= ISOLATED_TEST_IDS && round < ISOLATED_TEST_IDS { continue; }
            self.life(id, round, now);
        }
    }

    /// A new life for `peer` inside `round` (deathmatch respawn): as a round start, for
    /// this peer only. Vitals sent before now (seq <= the last seen) never count for it.
    pub fn respawn(&mut self, peer: PeerId, round: u32, now: Instant) {
        if let Some(l) = self.lives.get_mut(&peer) { l.round = round.wrapping_sub(1); }
        self.life(peer, round, now);
    }

    pub fn last_attacker(&self, victim: PeerId, round: u32) -> PeerId {
        self.lives.get(&victim).filter(|l| l.round == round).map_or(0, |l| l.last_attacker)
    }

    pub fn forget(&mut self, peer: PeerId) { self.lives.remove(&peer); }

    /// Vitals stream v2 (docs/development/subsystems/vitals.md): feed a sanitized frame's Health
    /// and dead flag through `on_vitals` (same seq/round/ceiling rules), then
    /// clamp the frame's Health to the accepted value, so peers are shown
    /// the server's believed health, never a cheater's ceiling-busting claim.
    /// None = no ledger information (stale/reordered, or no Health and not dead).
    pub fn on_frame(&mut self, peer: PeerId, round: u32, seq: u32, f: &mut Vitals, now: Instant) -> Option<LedgerVerdict> {
        let dead = f.dead();
        let hp = match f.health() {
            Some(h) => h,
            None if dead => 0.0,
            None => return None,
        };
        let v = self.on_vitals(peer, round, seq, hp, dead, now)?;
        if let Some(base) = self.lives.get(&peer).and_then(|l| l.base) {
            if f.health().map_or(false, |h| h > base) {
                vitals::set(f, vitals::I_HEALTH, base.max(0.0));
            }
        }
        Some(v)
    }
}

/// Server side of a `vitals` record (already validated: flags, parts and value ranges):
/// feed the ledger and clamp its Health to the ledger's ceiling IN PLACE (the relay then
/// copies these bytes). None = no ledger information.
pub fn vitals_in(peer: PeerId, round: u32, f: &mut Vitals) -> Option<LedgerVerdict> {
    // Neck Health gates the Snap Neck rule of the damage cap.
    damage::note_neck_health(peer, vitals::get(f, vitals::I_NECK).unwrap_or(f32::NAN));
    let seq = f.seq;
    ledger().lock().unwrap().on_frame(peer, round, seq, f, Instant::now())
}

fn ledger() -> &'static Mutex<Ledger> {
    static L: OnceLock<Mutex<Ledger>> = OnceLock::new();
    L.get_or_init(|| {
        // Operators may opt back in to god-mode KILLS (detection is always on).
        let enforce = std::env::var("HSMP_GODMODE_ENFORCE").map_or(GODMODE_ENFORCE_DEFAULT, |v| v.trim() == "1");
        if enforce { tracing::warn!("HSMP_GODMODE_ENFORCE=1: the god-mode rule declares deaths"); }
        Mutex::new(Ledger { enforce_godmode: enforce, ..Ledger::default() })
    })
}

/// Turn god-mode enforcement on / off for the process-wide ledger.
#[cfg(test)]
pub fn ledger_set_enforce_godmode(on: bool) { ledger().lock().unwrap().enforce_godmode = on; }

/// Latest-wins relay gate for the vitals streams: a vitals
/// packet is relayed only when its seq is newer than the last one relayed for
/// that (peer, stream). Duplicates, reordered and replayed packets are
/// dropped instead of fanned out to every peer. A seq far behind (> 1000) is
/// the owner's sidecar restarting and starts over.
#[derive(Default)]
pub struct RelaySeq {
    last: HashMap<(PeerId, u8), u32>,
}

impl RelaySeq {
    pub fn fresh(&mut self, peer: PeerId, stream: u8, seq: u32) -> bool {
        if let Some(&l) = self.last.get(&(peer, stream)) {
            let back = l.wrapping_sub(seq);
            let restarted = back > 1000 && back < u32::MAX / 2;
            if !seq_newer(seq, l) && !restarted { return false; }
        }
        self.last.insert((peer, stream), seq);
        true
    }
    pub fn forget(&mut self, peer: PeerId) { self.last.retain(|k, _| k.0 != peer); }
}

fn relay_seq() -> &'static Mutex<RelaySeq> {
    static R: OnceLock<Mutex<RelaySeq>> = OnceLock::new();
    R.get_or_init(Default::default)
}

/// See `RelaySeq::fresh`. `stream`: 1 = the vitals record stream.
pub fn vitals_relay_fresh(peer: PeerId, stream: u8, seq: u32) -> bool {
    relay_seq().lock().unwrap_or_else(|e| e.into_inner()).fresh(peer, stream, seq)
}

pub fn ledger_forward(attacker: PeerId, round: u32, hit: &DamageEvent) -> LedgerVerdict {
    ledger().lock().unwrap().on_forward(attacker, round, hit, Instant::now())
}
pub fn ledger_ack(victim: PeerId, attacker: PeerId, hit_id: u32) {
    ledger().lock().unwrap().on_ack(victim, attacker, hit_id, Instant::now())
}
#[cfg(test)]
pub fn ledger_vitals(peer: PeerId, round: u32, seq: u32, hp: f32, dead: bool) -> Option<LedgerVerdict> {
    ledger().lock().unwrap().on_vitals(peer, round, seq, hp, dead, Instant::now())
}
pub fn ledger_last_attacker(victim: PeerId, round: u32) -> PeerId {
    ledger().lock().unwrap().last_attacker(victim, round)
}
pub fn ledger_forget(peer: PeerId) {
    ledger().lock().unwrap().forget(peer);
    relay_seq().lock().unwrap_or_else(|e| e.into_inner()).forget(peer);
}
/// Tests: peer ids (and rounds) from here on are isolated from other tests'
/// round starts in the process-global ledger (see `Ledger::begin_round`).
#[cfg(test)]
pub(crate) const ISOLATED_TEST_IDS: PeerId = 70_000;
pub fn ledger_begin_round(round: u32) { ledger().lock().unwrap().begin_round(round, Instant::now()) }
/// A deathmatch respawn: `peer` starts a new life inside `round`.
pub fn ledger_respawn(peer: PeerId, round: u32) { ledger().lock().unwrap().respawn(peer, round, Instant::now()) }
/// Server tick: god-mode verdicts due now (see `Ledger::sweep`).
pub fn ledger_sweep(round: u32) -> Vec<(PeerId, LedgerVerdict)> { ledger().lock().unwrap().sweep(round, Instant::now()) }

#[cfg(test)]
mod tests {
    use super::*;

    fn ctx(attacker: PeerId) -> Ctx {
        Ctx {
            attacker_id: attacker,
            attacker_alive: true,
            attacker_pos: Some([0.0, 0.0, 100.0]),
            target_exists: true,
            target_alive: true,
            target_pos: Some([150.0, 0.0, 100.0]),
            match_live: true,
            match_round: 1, match_id: 0, attacker_life: 0, target_life: 0,
        }
    }

    /// A stuck blade's Inside Get Damage calls stand on the attacker's accepted parent hit.
    #[test]
    fn stuck_blade_continuation_needs_its_accepted_parent() {
        let mut e = Engine::default();
        let mut lc = Store::default();
        let c = ctx(41);
        // the parent: an accepted, forwarded hit with the game's cid 7 on bone pelvis
        let mut parent = hit(100);
        parent.cid = 7;
        parent.bone = hsmp_ipc::layout::Str::new("pelvis");
        e.decisions.insert((41, 100), Decision { accepted: true, reason: String::new(), decided_at: 0, first_at: 0,
            waiting: None, target_acked: true, owner_outcome: None, pending: None, grace_ms: 0, parryable: true,
            approved: Some(parent), forwarded_at: Some(0), reforwarded: false, confirmed: false });
        let inside = |id: u32, parent_cid: u32, bone: &str| {
            let mut h = hit(id);
            h.flags = damage::FLAG_INSIDE;
            h.parent_cid = parent_cid;
            h.bone = hsmp_ipc::layout::Str::new(bone);
            h.raw_damage = 167_935.0; h.cutting_power = 80_750.0; h.draw_cut = 0.5; h.pain_rate = 0.3; h.damage_out = 0.0;   // measured native maxima
            h
        };
        // the constraint moves a pelvis hit to spine_02 (stuck-weapon-native)
        let mut h = inside(101, 7, "spine_02");
        assert_eq!(e.on_damage(&mut lc, &c, &mut h, 1000), Verdict::Forward, "continuation of the accepted parent is forwarded");
        let mut orphan = inside(102, 8, "spine_02");
        assert!(matches!(e.on_damage(&mut lc, &c, &mut orphan, 1000), Verdict::Ack { accepted: false, ref reason } if reason.starts_with("no_parent")));
        let mut other_bone = inside(103, 7, "head");
        assert!(matches!(e.on_damage(&mut lc, &c, &mut other_bone, 1000), Verdict::Ack { accepted: false, .. }), "the stuck bone is the parent's");
        let mut not_inside = inside(104, 7, "spine_02");
        not_inside.flags = 0;
        assert!(matches!(e.on_damage(&mut lc, &c, &mut not_inside, 1000), Verdict::Ack { accepted: false, .. }), "only Inside calls continue");
        let mut booked = inside(105, 7, "spine_02");
        booked.damage_out = 30.0;
        assert!(matches!(e.on_damage(&mut lc, &c, &mut booked, 1000), Verdict::Ack { accepted: false, .. }), "a continuation books nothing itself");
        // a parent held for the defender grace already binds (delivery is ordered after it) ...
        let mut held_parent = hit(110);
        held_parent.cid = 9;
        e.decisions.insert((41, 110), Decision { accepted: true, reason: String::new(), decided_at: 900, first_at: 900,
            waiting: None, target_acked: false, owner_outcome: None, pending: Some(held_parent), grace_ms: 150, parryable: true,
            approved: Some(held_parent), forwarded_at: None, reforwarded: false, confirmed: false });
        let mut on_held = inside(111, 9, "spine_02");
        assert!(!matches!(e.on_damage(&mut lc, &c, &mut on_held, 1000), Verdict::Ack { accepted: false, .. }), "a held parent binds its continuation");
        // ... a parent still waiting for stream coverage leaves its continuation undecided
        let mut waiting_parent = hit(120);
        waiting_parent.cid = 10;
        e.decisions.insert((41, 120), Decision { accepted: false, reason: String::new(), decided_at: 900, first_at: 900,
            waiting: Some(waiting_parent), target_acked: false, owner_outcome: None, pending: None, grace_ms: 0, parryable: true,
            approved: None, forwarded_at: None, reforwarded: false, confirmed: false });
        let mut on_waiting = inside(121, 10, "spine_02");
        assert_eq!(e.on_damage(&mut lc, &c, &mut on_waiting, 1000), Verdict::Hold, "decided on the resend once the parent is");
        // the native tick rate bounds a constraint's calls
        let mut over = 0;
        for k in 0..200u32 {
            let mut h = inside(200 + k, 7, "spine_02");
            if e.on_damage(&mut lc, &c, &mut h, 2000) != Verdict::Forward { over += 1; }
        }
        assert!(over > 100 && over < 200, "calls above the native tick rate are refused ({over})");
        // still bound 15 s on (past DECISION_TTL, the parent decision itself is pruned)
        let mut held = inside(500, 7, "spine_02");
        assert_eq!(e.on_damage(&mut lc, &c, &mut held, 15_000), Verdict::Forward, "a blade stuck past the decision TTL continues");
        let mut late = inside(501, 7, "spine_02");
        assert!(matches!(e.on_damage(&mut lc, &c, &mut late, INSIDE_MAX_MS + 1), Verdict::Ack { accepted: false, ref reason } if reason.starts_with("too_late")));
    }

    #[test]
    fn stuck_blade_continuation_retains_exact_native_module() {
        let mut e=Engine::default();let mut lc=Store::default();let c=ctx(988);
        let mut parent=hit(1);parent.cid=7;
        parent.bone=hsmp_ipc::layout::Str::new("lowerarm_l");
        parent.flags=damage::FLAG_COMPLEX|damage::FLAG_WEAPON;
        parent.dism_blunt=damage::SOURCE_RIGHT|(1<<damage::SOURCE_COMPONENT_SHIFT);
        parent.source_class=hsmp_ipc::layout::Str::new("ModularWeaponBP_ArmingSword_T3_C");
        e.decisions.insert((988,1),Decision {accepted:true,reason:String::new(),decided_at:0,first_at:0,
            waiting:None,target_acked:true,owner_outcome:None,pending:None,grace_ms:0,parryable:true,
            approved:Some(parent),forwarded_at:Some(0),reforwarded:false,confirmed:false});
        let mut inside=parent;inside.hit_id=2;inside.cid=8;inside.parent_cid=7;
        inside.flags=damage::FLAG_INSIDE|damage::FLAG_WEAPON;inside.damage_out=0.0;
        inside.bone=hsmp_ipc::layout::Str::new("hand_l");
        assert_eq!(e.on_damage(&mut lc,&c,&mut inside,100),Verdict::Forward,
            "measured same-constraint lowerarm_l to hand_l rebind retains its module");
        for edit in 0..5 {
            let mut bad=inside;bad.hit_id=3+edit;
            match edit {
                0=>bad.dism_blunt=damage::SOURCE_RIGHT|(2<<damage::SOURCE_COMPONENT_SHIFT),
                1=>bad.source_class=hsmp_ipc::layout::Str::new("ModularWeaponBP_ArmingSword_T2_C"),
                2=>bad.dism_blunt=damage::SOURCE_LEFT|(1<<damage::SOURCE_COMPONENT_SHIFT),
                3=>bad.bone=hsmp_ipc::layout::Str::new("hand_r"),
                _=>{bad.flags=damage::FLAG_INSIDE;bad.dism_blunt=0;bad.source_class=hsmp_ipc::layout::Str::new("");},
            }
            assert!(matches!(e.on_damage(&mut lc,&c,&mut bad,100),Verdict::Ack{accepted:false,..}),
                "continuation cannot borrow another source or unmeasured bone ({edit})");
        }
        e.reject_decision(988,1,"parried");
        assert!(matches!(e.on_damage(&mut lc,&c,&mut inside,101),Verdict::Ack{accepted:false,..}),
            "cached retransmit of continuation cannot outlive parent rejection");
    }

    #[test]
    fn stuck_blade_deferred_continuation_drops_when_parent_is_parried() {
        lagcomp_world(992,991);
        let c=ctx_at(991);let mut e=Engine::default();let now=lagcomp::now_ms();
        lagcomp::with_store(|lc| {
            let mut parent=ts_hit(1,992);parent.cid=7;
            assert_eq!(e.on_damage(lc,&c,&mut parent,now),Verdict::Hold);
            let mut inside=parent;inside.hit_id=2;inside.cid=8;inside.parent_cid=7;
            inside.flags=damage::FLAG_INSIDE;inside.damage_out=0.0;
            assert_eq!(e.on_damage(lc,&c,&mut inside,now),Verdict::Hold,
                "continuation waits behind the parent's defender grace");
            lc.record_clash(992,991,5300,9180,now+20);
            let out=e.flush_pending(lc,now+ms(DEFENDER_GRACE)+1);
            assert_eq!(out.len(),2,"both parent and deferred child receive final rejection");
            assert!(out.iter().all(|(_,_,v)|matches!(v,Verdict::Ack{accepted:false,..})),"{out:?}");
            assert!(e.inside.is_empty(),"parried origin leaves no cached constraint authorization");
            let mut later=inside;later.hit_id=3;
            assert!(matches!(e.on_damage(lc,&c,&mut later,now+400),Verdict::Ack{accepted:false,..}));
        });
    }

    #[test]
    fn suppressed_native_origin_books_no_damage() {
        let mut origin=hit(1);origin.flags=damage::FLAG_COMPLEX|damage::FLAG_INSIDE|damage::FLAG_WEAPON;
        origin.damage_out=0.0;origin.set_deltas(&[]);
        origin.velocity=[10_000.0,0.0,0.0];origin.impulse=[10_000.0,0.0,0.0];origin.cutting_power=100.0;
        assert_eq!(hit_loss(&origin),0.0);
        assert_eq!(trusted_loss(&origin),0.0);
    }

    fn hit(id: u32) -> DamageEvent {
        DamageEvent::new(crate::proto::Damage {
            hit_id: id,
            target_peer_id: 2,
            round: 1,
            bone: hsmp_ipc::layout::Str::new("spine_02"),
            offset: [1.0, 2.0, 3.0],
            location: [140.0, 0.0, 120.0],
            impulse: [0.0; 3],
            velocity: [0.0; 3],
            normal: [1.0, 0.0, 0.0],
            raw_damage: 40.0,
            cutting_power: 1.0,
            pain_rate: 1.0,
            draw_cut: 0.0,
            damage_out: 12.0,
            dism_blunt: 0,
            flags: 0,
            age_ms: 0,
            attacker_ts: 0,
            victim_view_ts: 0,
            victim_arm_ts: 0, ..Default::default()
        }, &crate::proto::deltas_of(&[(2, -12.5), (11, 3.0)]))
    }

    #[test]
    fn owner_native_outcome_is_authenticated_immutable_and_life_scoped() {
        use hsmp_ipc::schema::combat::*;
        let mut e=Engine::default();let mut lc=Store::default();let c=ctx(87);let mut h=hit(88);
        assert_eq!(e.on_damage_inner(&mut lc,&c,&mut h,100),Verdict::Forward);
        let r=ReplayOutcome{match_id:0,round:1,attacker:87,hit_id:88,victim_life:0,status:REPLAY_CHANGED,
            observed_fields:1,health_delta:-7.0,..Default::default()};
        assert_eq!(e.on_replay_outcome_by(99,r),None,"only authenticated victim can attest native result");
        let mut stale=r;stale.victim_life=2;
        assert_eq!(e.on_replay_outcome_by(2,stale),None,"wrong native victim life refused");
        assert_eq!(e.on_replay_outcome_by(2,r),Some(true));
        assert_eq!(e.on_replay_outcome_by(2,r),Some(false),"lost receipt ACK is idempotent");
        let mut conflicting=r;conflicting.health_delta = -20.0;
        assert_eq!(e.on_replay_outcome_by(2,conflicting),None,"recorded native result cannot be revised");
        let mut new=c;new.target_life=1;
        assert!(matches!(e.on_damage_inner(&mut lc,&new,&mut h,101),Verdict::Ack{accepted:false,..}),
            "cached delivery approval cannot revive old-life callback");
        new=ctx(87);new.match_id=999;
        assert!(matches!(e.on_damage_inner(&mut lc,&new,&mut h,102),Verdict::Ack{accepted:false,..}),
            "same round in another match cannot relabel callback");
    }

    #[test]
    fn native_effects_wait_for_changed_owner_result_not_transport_ack() {
        use hsmp_ipc::schema::combat::*;
        let mut e=Engine::default();let mut lc=Store::default();let c=ctx(986);
        for status in REPLAY_CHANGED..=REPLAY_EXPIRED {
            let mut h=hit(u32::from(status));h.flags=damage::FLAG_COMPLEX;
            assert_eq!(e.on_damage_inner(&mut lc,&c,&mut h,100),Verdict::Forward);
            assert!(e.native_effect_hit(2,986,h.hit_id).is_none(),"geometric approval cannot paint");
            assert!(e.on_owner_ack_by(&mut lc,2,986,h.hit_id,110));
            assert!(e.native_effect_hit(2,986,h.hit_id).is_none(),"sidecar transport ACK cannot paint");
            let r=ReplayOutcome{match_id:h.match_id,round:h.round,attacker:986,hit_id:h.hit_id,
                victim_life:h.victim_life,status,observed_fields:if status==REPLAY_CHANGED {1}else{0},
                health_delta:if status==REPLAY_CHANGED {-1.0}else{0.0},..Default::default()};
            assert_eq!(e.on_replay_outcome_by(99,r),None,"another peer cannot authorize a wound");
            assert_eq!(e.on_replay_outcome_by(2,r),Some(true));
            assert_eq!(e.native_effect_hit(2,986,h.hit_id).is_some(),status==REPLAY_CHANGED);
            assert_eq!(e.on_replay_outcome_by(2,r),Some(false),"duplicate receipt cannot trigger another broadcast");
            if status==REPLAY_CHANGED {
                let approved=e.native_effect_hit(2,986,h.hit_id).unwrap();
                assert_eq!(approved.h,h.h,"cosmetic call retains exact approved native inputs");
                e.decisions.get_mut(&(986,h.hit_id)).unwrap().approved.as_mut().unwrap().flags|=damage::FLAG_INSIDE;
                assert!(e.native_effect_hit(2,986,h.hit_id).is_none(),"suppressed origin never paints");
                e.reject_decision(986,h.hit_id,"stale_life");
                assert!(e.native_effect_hit(2,986,h.hit_id).is_none(),"rejected accepted hit cannot paint");
            }
        }
        for (id,fields,expected) in [(8,(1<<15)|(1<<16)|(1<<17),false),(9,1<<13,true)] {
            let mut h=hit(id);h.flags=damage::FLAG_COMPLEX;
            assert_eq!(e.on_damage_inner(&mut lc,&c,&mut h,100),Verdict::Forward);
            let r=ReplayOutcome{match_id:h.match_id,round:h.round,attacker:986,hit_id:h.hit_id,
                victim_life:h.victim_life,status:REPLAY_CHANGED,observed_fields:fields,health_delta:0.0,..Default::default()};
            assert_eq!(e.on_replay_outcome_by(2,r),Some(true));
            assert_eq!(e.native_effect_hit(2,986,h.hit_id).is_some(),expected,
                "bookkeeping cannot authorize wounds; native injury can without main Health loss");
        }
    }

    #[test]
    fn bad_deltas_are_sanitised_not_rejected() {
        // Live play: Get Damage writes the hit's DRS (thousands) into "Last
        // Damage Taken" (#17) / "Sustained Damage" (#15); a strict check
        // would reject nearly every damaging hit as "bad damage deltas".
        let c = ctx(106);
        let before = cheat::get(106).damage_clamped;
        let mut h = hit(1);
        h.set_delta_pairs(&[(FIELD_HEALTH, -5.0), (3, -12.0), (13, 14.0), (15, 4200.0), (17, 4200.0)]);
        assert_eq!(on_damage_at(&c, &mut h, Instant::now()), Verdict::Forward);
        assert_eq!(h.delta_pairs(), vec![(FIELD_HEALTH, -5.0), (3, -12.0), (13, 14.0)]);
        assert_eq!(cheat::get(106).damage_clamped, before, "bookkeeping is not suspicious");
        // Unknown index / NaN dropped, absurd magnitudes clamped — still forwarded.
        let mut h = hit(2); h.set_delta_pairs(&[(99, -1.0), (1, f32::NAN), (2, -9999.0)]);
        assert_eq!(on_damage_at(&c, &mut h, Instant::now()), Verdict::Forward);
        assert_eq!(h.delta_pairs(), vec![(2, -150.0)]);
        assert!(cheat::get(106).damage_clamped > before);
    }

    #[test]
    fn accept_then_dedup_then_ack() {
        let c = ctx(101);
        let t0 = Instant::now();
        assert_eq!(on_damage_at(&c, &mut hit(1), t0), Verdict::Forward);
        // Resend before the owner acks: re-forward, never re-validate.
        assert_eq!(on_damage_at(&c, &mut hit(1), t0), Verdict::Reforward);
        assert!(on_owner_ack(2, 101, 1));
        assert_eq!(on_damage_at(&c, &mut hit(1), t0),
                   Verdict::Ack { accepted: true, reason: String::new() });
    }

    #[test]
    fn verdict_is_sticky_for_resends() {
        let mut c = ctx(102);
        c.target_alive = false;
        let v = on_damage_at(&c, &mut hit(7), Instant::now());
        assert!(matches!(v, Verdict::Ack { accepted: false, .. }));
        // Target state changes later; the resend keeps the original verdict.
        c.target_alive = true;
        assert!(matches!(on_damage_at(&c, &mut hit(7), Instant::now()), Verdict::Ack { accepted: false, .. }));
    }

    #[test]
    fn rejects_stale_round_late_far_and_overdamage() {
        let c = ctx(103);
        let mut h = hit(1); h.round = 0;
        assert!(matches!(on_damage_at(&c, &mut h, Instant::now()), Verdict::Ack { accepted: false, .. }));
        let mut h = hit(2); h.age_ms = LAG_WINDOW_MS + 1;
        assert!(matches!(on_damage_at(&c, &mut h, Instant::now()), Verdict::Ack { accepted: false, .. }));
        let mut c2 = ctx(103); c2.target_pos = Some([5000.0, 0.0, 0.0]);
        assert!(matches!(on_damage_at(&c2, &mut hit(3), Instant::now()), Verdict::Ack { accepted: false, .. }));
        let mut h = hit(4); h.damage_out = 999.0;
        assert!(matches!(on_damage_at(&c, &mut h, Instant::now()), Verdict::Ack { accepted: false, .. }));
    }

    #[test]
    fn rate_limit_bursts_then_refills() {
        let c = ctx(104);
        let t0 = Instant::now();
        let mut accepted = 0;
        for i in 0..20 {
            if on_damage_at(&c, &mut hit(100 + i), t0) == Verdict::Forward { accepted += 1; }
        }
        assert_eq!(accepted, BUCKET_CAP as u32);
        let later = t0 + Duration::from_millis(500);
        assert_eq!(on_damage_at(&c, &mut hit(200), later), Verdict::Forward);
    }

    #[test]
    fn native_contacts_have_a_separate_bounded_budget() {
        let mut e = Engine::default();
        let attacker = 104;
        for _ in 0..COMPLEX_BUCKET_CAP as usize {
            assert!(e.take_token(attacker, 0, true));
        }
        assert!(!e.take_token(attacker, 0, true));
        // Native contact volume never consumes the older delta-report budget.
        for _ in 0..BUCKET_CAP as usize {
            assert!(e.take_token(attacker, 0, false));
        }
        assert!(!e.take_token(attacker, 0, false));
        for _ in 0..(COMPLEX_REFILL_PER_S / 2.0) as usize {
            assert!(e.take_token(attacker, 500, true));
        }
        assert!(!e.take_token(attacker, 500, true));
    }

    #[test]
    fn native_source_metadata_is_bounded_and_consistent() {
        let mut e = Engine::default();
        let mut lc = Store::default();
        let c = ctx(490);
        for (id, source, flags) in [(1, -1, damage::FLAG_COMPLEX),
            (2, 1 << 27, damage::FLAG_COMPLEX | damage::FLAG_WEAPON),
            (3, damage::SOURCE_LEFT | damage::SOURCE_RIGHT, damage::FLAG_COMPLEX | damage::FLAG_WEAPON),
            (4, damage::SOURCE_FIST, damage::FLAG_COMPLEX),
            (6, 1 << 21, damage::FLAG_WEAPON),
            (10, damage::DAMAGE_PARENT, 0),
            (7, damage::SOURCE_FEET | damage::SOURCE_FIST | damage::SOURCE_LEFT | (10 << 21), damage::FLAG_COMPLEX | damage::FLAG_WEAPON),
            (8, damage::SOURCE_FEET | (10 << 21), damage::FLAG_COMPLEX | damage::FLAG_WEAPON)] {
            let mut h = hit(id); h.flags = flags; h.dism_blunt = source;
            assert!(matches!(e.on_damage(&mut lc, &c, &mut h, 0), Verdict::Ack { accepted: false, reason }
                if reason.starts_with("bad_field:")));
        }
        let mut h = hit(5); h.flags = damage::FLAG_COMPLEX | damage::FLAG_WEAPON;
        h.dism_blunt = damage::SOURCE_FIST | damage::SOURCE_LEFT | (15 << 21);
        h.source_class=hsmp_ipc::layout::Str::new("Weapon_Fists_C");
        assert_eq!(e.on_damage(&mut lc, &c, &mut h, 0), Verdict::Forward);
        assert_eq!(h.dism_blunt & damage::SOURCE_MASK, damage::SOURCE_FIST | damage::SOURCE_LEFT | (15 << 21));
        h.hit_id = 9;
        h.dism_blunt = damage::SOURCE_FEET | damage::SOURCE_RIGHT | (10 << 21);
        h.source_class=hsmp_ipc::layout::Str::new("Weapon_Feet_C");
        assert_eq!(e.on_damage(&mut lc, &c, &mut h, 0), Verdict::Forward);
        assert_eq!(h.dism_blunt & damage::SOURCE_MASK, damage::SOURCE_FEET | damage::SOURCE_RIGHT | (10 << 21));
        h.hit_id = 11; h.flags = damage::FLAG_COMPLEX; h.dism_blunt = damage::DAMAGE_PARENT;
        assert_eq!(e.on_damage(&mut lc, &c, &mut h, 0), Verdict::Forward);
        assert_eq!(h.dism_blunt & damage::DAMAGE_PARENT, damage::DAMAGE_PARENT);
    }

    #[test]
    fn later_contact_cannot_overtake_an_earlier_held_contact() {
        let mut e = Engine::default();
        let mut lc = Store::default();
        let c = ctx(491);
        let mut first = hit(1);
        assert_eq!(e.on_damage(&mut lc, &c, &mut first, 0), Verdict::Forward);
        // The geometry stage's held state (defender grace); subsequent contacts
        // arrive without a parry and used to bypass this older contact.
        let d = e.decisions.get_mut(&(491, 1)).unwrap();
        d.pending = d.approved;
        d.forwarded_at = None;
        d.grace_ms = 200;
        e.active.insert((491, 1));
        let mut second = hit(2);
        assert_eq!(e.on_damage(&mut lc, &c, &mut second, 10), Verdict::Hold);
        assert_eq!(e.on_damage(&mut lc, &c, &mut second, 50), Verdict::Hold, "resend cannot bypass order");
        let mut other = hit(3); other.target_peer_id = 3;
        assert_eq!(e.on_damage(&mut lc, &c, &mut other, 60), Verdict::Forward, "another victim is independent");
        assert!(e.flush_pending(&mut lc, 100).is_empty());
        let out = e.flush_pending(&mut lc, 200);
        assert_eq!(out.into_iter().map(|(_, h, v)| (h.hit_id, v)).collect::<Vec<_>>(),
            vec![(1, Verdict::Forward), (2, Verdict::Forward)]);
    }

    #[test]
    fn unbound_fist_claim_is_rejected_despite_a_simultaneous_weapon_clash() {
        lagcomp_world(494, 493);
        lagcomp::record_root(493, 9200, [100.0, 0.0, 100.0]);
        lagcomp::record_capsules(493, 9200, &[lagcomp::Capsule { a: [100.0, 0.0, 100.0], b: [100.0, 0.0, 110.0], r: 10.0 }]);
        lagcomp::record_clash(494, 493, 5300, 9190);
        let mut h = ts_hit(1, 494);
        h.flags = damage::FLAG_COMPLEX | damage::FLAG_WEAPON;
        h.dism_blunt = damage::SOURCE_FIST | damage::SOURCE_LEFT;
        h.location = [100.0, 0.0, 105.0];
        let c = ctx_at(493);
        assert!(matches!(on_damage_at(&c, &mut h, Instant::now()), Verdict::Ack {accepted:false,..}));
    }

    /// A flood of invalid claims with fresh hit ids costs a token
    /// each before validation; the decisions kept per attacker are bounded;
    /// flush_pending visits only the waiting / held ones.
    #[test]
    fn invalid_claim_flood_is_bounded() {
        let mut e = Engine::default();
        let mut lc = Store::default();
        let mut c = ctx(7);
        c.match_live = false; // every claim fails validation (`not_live`)
        let mut replies = 0;
        let mut ignored = 0;
        for k in 0..10_000u32 {
            let now = k as i64 / 10; // 10 000 claims/s for 1 s
            match e.on_damage(&mut lc, &c, &mut hit(k + 1), now) {
                Verdict::Ack { accepted: false, .. } => replies += 1,
                Verdict::Ignore => ignored += 1,
                v => panic!("{v:?}"),
            }
        }
        assert!(replies <= (CLAIM_BUCKET_CAP + CLAIM_REFILL_PER_S) as usize + 1, "{replies} replies");
        assert!(ignored >= 10_000 - (CLAIM_BUCKET_CAP + CLAIM_REFILL_PER_S) as usize - 1);
        assert!(e.decisions.len() <= replies);
        // 60 s of flood: never more than the per-attacker cap.
        for k in 0..60_000u32 {
            let _ = e.on_damage(&mut lc, &c, &mut hit(20_000 + k), 1_000 + k as i64);
        }
        assert!(e.decisions.len() <= MAX_DECISIONS_PER_ATTACKER, "{}", e.decisions.len());
        assert!(e.active.is_empty(), "rejects are never visited by the tick");
        assert!(e.flush_pending(&mut lc, 70_000).is_empty());
        // Another attacker is unaffected.
        let c2 = ctx(8);
        assert_eq!(e.on_damage(&mut lc, &c2, &mut hit(1), 70_000), Verdict::Forward);
    }

    #[test]
    fn unacked_forward_expires() {
        let c = ctx(105);
        let t0 = Instant::now();
        assert_eq!(on_damage_at(&c, &mut hit(1), t0), Verdict::Forward);
        let late = t0 + FORWARD_TTL + Duration::from_millis(1);
        assert!(matches!(on_damage_at(&c, &mut hit(1), late), Verdict::Ack { accepted: false, .. }));
    }

    /// Victim stands at x=150 (bones stacked up the z axis); attacker's
    /// weapon at x=60.
    /// `guard`: the victim's weapon sits next to the contact (a parry is
    /// plausible, so accepted hits are held for the defender grace).
    /// Samples arrive at the server 20 ms after they were sent (server ms
    /// relative to "now"), so the clock map is exact; both RTTs are 40 ms.
    /// `guard`: the attacker's blade also rests against the victim's (a
    /// clash report is geometrically valid).
    fn lagcomp_world_g(victim: PeerId, attacker: PeerId, guard: bool) {
        let aw = if guard { [100.0, 50.0, 150.0] } else { [60.0, 0.0, 120.0] };
        let vw = if guard { [120.0, 30.0, 120.0] } else { [170.0, 0.0, 400.0] };
        lagcomp_world_w(victim, attacker, aw, vw)
    }

    /// Same world with explicit attacker / victim weapon-actor positions.
    fn lagcomp_world_w(victim: PeerId, attacker: PeerId, aw: [f32; 3], vw: [f32; 3]) {
        let n = lagcomp::now_ms();
        lagcomp::with_store(|s| {
            for i in 0..10u32 {
                let rx = n - 300 + 30 * i as i64 + 20;
                let bones: Vec<(u8, [f32; 7])> = (0..crate::posecodec::BONE_COUNT as u8)
                    .map(|b| (b, [150.0, 0.0, 60.0 + b as f32 * 5.0, 0.0, 0.0, 0.0, 1.0]))
                    .collect();
                let f = crate::posecodec::decode(&crate::posecodec::encode(5000 + i * 30, &bones)).unwrap();
                s.record_root(victim, 5000 + i * 30, [150.0, 0.0, 60.0], rx);
                s.record_pose(victim, &f, rx);
                s.record_weapon(attacker, 9000 + i * 30, aw, rx);
                s.record_weapon(victim, 5000 + i * 30, vw, rx);
            }
            s.note_rtt(attacker, 40.0, n);
            s.note_rtt(victim, 40.0, n);
        });
    }

    fn lagcomp_world(victim: PeerId, attacker: PeerId) { lagcomp_world_g(victim, attacker, true) }

    #[test]
    fn lagcomp_hit_without_possible_parry_is_not_delayed() {
        lagcomp_world_g(342, 341, false);
        let c = ctx_at(341);
        assert_eq!(on_damage_at(&c, &mut ts_hit(1, 342), Instant::now()), Verdict::Forward);
    }

    #[test]
    fn known_clash_cancels_immediately() {
        lagcomp_world(352, 351);
        lagcomp::record_clash(352, 351, 5300, 9190);
        let c = ctx_at(351);
        assert_eq!(on_damage_at(&c, &mut ts_hit(1, 352), Instant::now()),
                   Verdict::Ack { accepted: false, reason: "parried".into() });
    }

    fn lhit(target: PeerId, id: u32, health_delta: f32) -> DamageEvent {
        let mut h = hit(id);
        h.target_peer_id = target;
        h.set_delta_pairs(&[(FIELD_HEALTH, health_delta), (10, -5.0)]);
        h
    }

    /// A deathmatch respawn starts a new life inside the same round: the last
    /// attacker and the booked losses are gone, and only vitals newer than the
    /// respawn count.
    #[test]
    fn ledger_respawn_starts_a_new_life_in_the_same_round() {
        let mut l = Ledger::default();
        let t0 = Instant::now();
        l.on_vitals(2, 1, 1, 100.0, false, t0);
        l.on_forward(1, 1, &lhit(2, 1, -60.0), t0 + Duration::from_millis(100));
        assert!(l.on_vitals(2, 1, 2, 0.0, true, t0 + Duration::from_millis(200)).unwrap().lethal);
        assert_eq!(l.last_attacker(2, 1), 1);
        l.respawn(2, 1, t0 + Duration::from_millis(300));
        assert_eq!(l.last_attacker(2, 1), 0);
        assert!(l.on_vitals(2, 1, 2, 0.0, true, t0 + Duration::from_millis(400)).is_none(), "a stale dead frame of the old life");
        let v = l.on_vitals(2, 1, 3, 100.0, false, t0 + Duration::from_millis(500)).unwrap();
        assert!(!v.lethal);
    }

    #[test]
    fn ledger_never_kills_a_reporting_owner_from_booked_losses() {
        // Solo parity: an inflated stand-in measurement (-95, -95) must not
        // kill an owner whose own vitals say it is fine.
        let mut l = Ledger::default();
        let t0 = Instant::now();
        l.on_vitals(2, 1, 1, 100.0, false, t0);
        assert!(!l.on_forward(1, 1, &lhit(2, 1, -95.0), t0 + Duration::from_millis(100)).lethal);
        assert!(!l.on_forward(1, 1, &lhit(2, 2, -95.0), t0 + Duration::from_millis(200)).lethal);
        l.on_ack(2, 1, 1, t0 + Duration::from_millis(200));
        l.on_ack(2, 1, 2, t0 + Duration::from_millis(200));
        let v = l.on_vitals(2, 1, 2, 91.0, false, t0 + Duration::from_millis(600)).unwrap();
        assert!(!v.lethal, "{:?}", v);
        // The owner's own death still counts.
        assert!(l.on_vitals(2, 1, 3, 0.0, true, t0 + Duration::from_millis(700)).unwrap().lethal);
    }

    #[test]
    fn impact_inputs_bounded_by_the_real_relative_speed() {
        // A stand-in whose servo body rushed into the blade measured raw 9000;
        // a sharp sword edge (CP 120) at 1500 uu/s against a still victim can
        // do at most rig 1.05 (the pommel, the class maximum) · 1.1 · max(1500, 1.8 · 1500) ≈ 3119.
        damage::set_kit(461, &kit("w_arming1", &[]));
        let mut h = hit(1);
        h.raw_damage = 9000.0;
        h.cutting_power = 120.0;
        h.velocity = [9000.0, 0.0, 0.0];
        h.impulse = [12000.0, 0.0, 0.0];
        let c = damage::clamp_impact(461, &mut h, Some(1500.0), Some(1500.0), false);
        assert!((h.raw_damage - c.raw_max).abs() < 0.1 && c.raw_max < 3200.0, "{:?} {}", c, h.raw_damage);
        assert!(h.velocity[0] < 9000.0 * c.factor + 0.1 && h.impulse[0] < 12000.0 * c.factor + 0.1);
        // A blunt flat hit (CP 0) may reach 2.33x as much; honest values pass.
        let mut h = hit(2);
        h.raw_damage = 2000.0;
        h.cutting_power = 0.0;
        let c = damage::clamp_impact(461, &mut h, Some(1500.0), Some(1500.0), false);
        assert_eq!(c.factor, 1.0);
        assert_eq!(h.raw_damage, 2000.0);
    }

    #[test]
    fn complex_impact_forwarded_as_measured_within_the_ceiling() {
        // The attacker's native Deal Complex Damage inputs are
        // the solo computation and pass unchanged unless physically impossible
        // (a two-way rescale toward lag comp's speed would move real blows up
        // to 12x either way).
        damage::set_kit(462, &kit("w_arming1", &[]));
        let mk = |vel: f32, imp: f32| {
            let mut h = hit(3);
            h.flags |= damage::FLAG_COMPLEX;
            h.raw_damage = 57.0;   // stand-in relative speed
            h.damage_out = 0.85;
            h.cutting_power = 60.0;
            h.impulse = [imp, 0.0, 0.0];
            h.velocity = [vel, 0.0, 0.0];
            h
        };
        // A rubber-banded frame (live #39: vel 381 = 0.1 · imp 3169) with a
        // 1300 uu/s peak swing: untouched (a rescale toward lag comp would raise vel to 3205).
        let mut h = mk(381.0, 3169.0);
        let c = damage::clamp_impact_ex(462, &mut h, Some(1300.0), Some(1280.0), true, false);
        assert_eq!((h.velocity[0], h.impulse[0], c.factor), (381.0, 3169.0, 1.0));
        // An under-reading stand-in is never scaled UP toward the server speed.
        let mut h = mk(1400.0, 2000.0);
        damage::clamp_impact_ex(462, &mut h, Some(1500.0), Some(30.0), true, false);
        assert_eq!((h.velocity[0], h.impulse[0]), (1400.0, 2000.0));
        // The PEAK striking speed bounds (a blade stopping in its target reads
        // 12 uu/s relative at the touch).
        let mut h = mk(400.0, 500.0);
        damage::clamp_impact_ex(462, &mut h, Some(400.0), Some(12.0), true, false);
        assert_eq!((h.velocity[0], h.impulse[0]), (400.0, 500.0));
        // A fabricated claim is capped: Hit Velocity ≤ max(peak, hvf · rel)
        // (sword 1.8), Hit Impulse ≤ 12 · peak.
        let mut h = mk(9000.0, 12000.0);
        let c = damage::clamp_impact_ex(462, &mut h, Some(1500.0), Some(1500.0), true, false);
        assert!((h.velocity[0] - 2700.0).abs() < 0.5 && h.impulse[0] == 12000.0 && (c.factor - 0.3).abs() < 0.01, "{:?} {:?} {:?}", h.velocity, h.impulse, c);
        // A servo over-read (stand-in relative speed above the peak) is removed.
        let mut h = mk(1500.0, 1500.0);
        h.raw_damage = 3000.0;
        damage::clamp_impact_ex(462, &mut h, Some(1500.0), Some(1400.0), true, false);
        assert!((h.impulse[0] - 750.0).abs() < 0.5 && (h.velocity[0] - 1275.0).abs() < 0.5, "{:?} {:?}", h.impulse, h.velocity);
        // ... but not on the v1 lever estimate (too coarse to judge by).
        let mut h = mk(1500.0, 1500.0);
        h.raw_damage = 3000.0;
        damage::clamp_impact_ex(462, &mut h, Some(1500.0), Some(1400.0), false, false);
        assert_eq!(h.impulse[0], 1500.0);
    }

    #[test]
    fn ledger_kills_when_owner_never_reflects() {
        // Victim alt-tabbed: its game sends no vitals, its sidecar still acks.
        let mut l = Ledger::default();
        let t0 = Instant::now();
        assert!(l.on_vitals(2, 1, 10, 100.0, false, t0).is_some());
        assert!(!l.on_forward(1, 1, &lhit(2, 1, -60.0), t0).lethal);
        l.on_ack(2, 1, 1, t0);
        let v = l.on_forward(1, 1, &lhit(2, 2, -45.0), t0 + OWNER_STALL + Duration::from_millis(500));
        assert!(v.lethal, "{:?}", v);
    }

    /// An owner that ignores its damage and keeps reporting
    /// `health = 100, dead = false` must not be immortal (lethal only on its
    /// own death). Once the server-validated damage alone is lethal and the
    /// owner keeps reporting alive past the grace, the server declares it.
    #[test]
    fn ledger_kills_a_god_mode_owner_that_keeps_reporting_alive() {
        // Enforced only where a server opts in (GODMODE_ENFORCE_DEFAULT).
        let enforced = || Ledger { enforce_godmode: true, ..Ledger::default() };
        let mut l = enforced();
        let t0 = Instant::now();
        let ms = |n: u64| t0 + Duration::from_millis(n);
        assert!(!l.on_vitals(2, 1, 1, 100.0, false, t0).unwrap().lethal);
        // Two 70 hp blows, trusted at HM_MIN / HM_MAX (≈ 54 each).
        let (h1, h2) = (lhit(2, 1, -70.0), lhit(2, 2, -70.0));
        let (b1, b2) = (trusted_loss(&h1), trusted_loss(&h2));
        assert!(b1 + b2 >= 100.0, "test needs lethal bookings ({b1} + {b2})");
        l.on_forward(1, 1, &h1, ms(100));
        l.on_ack(2, 1, 1, ms(120));
        l.on_forward(1, 1, &h2, ms(600));
        l.on_ack(2, 1, 2, ms(620));
        // The liar keeps reporting full health at 15 Hz.
        let mut seq = 2;
        let mut killed_at = None;
        for t in (700..4000).step_by(66) {
            seq += 1;
            let v = l.on_vitals(2, 1, seq, 100.0, false, ms(t)).unwrap();
            if v.lethal { assert!(v.forced); killed_at = Some(t); break; }
        }
        let t = killed_at.expect("god mode never declared");
        // Lethal once the second hit was REFLECT_AFTER past its ack, + the grace.
        let g = GODMODE_GRACE.as_millis() as u64;
        assert!(t >= 620 + 250 + g && t <= 620 + 250 + g + 150, "declared at {t} ms");
        // On the server tick too, without waiting for the liar's next vitals
        // (it reports an unchanging Health: one heartbeat a second).
        let mut l = enforced();
        l.on_vitals(2, 1, 1, 100.0, false, t0);
        l.on_forward(1, 1, &lhit(2, 1, -70.0), ms(100));
        l.on_ack(2, 1, 1, ms(120));
        l.on_forward(1, 1, &lhit(2, 2, -70.0), ms(600));
        l.on_ack(2, 1, 2, ms(620));
        let mut swept = None;
        for t in (600..3000).step_by(33) {
            if let Some((p, v)) = l.sweep(1, ms(t)).into_iter().next() {
                assert_eq!(p, 2);
                assert!(v.forced && v.lethal);
                swept = Some(t);
                break;
            }
        }
        let t = swept.expect("sweep never declared");
        assert!(t >= 620 + 250 + g && t <= 620 + 250 + g + 70, "swept at {t} ms (33 ms ticks)");
        assert!(l.sweep(1, ms(4000)).is_empty(), "declared once");
        // An owner that reported its own death is never forced afterwards.
        let mut l = enforced();
        l.on_vitals(3, 1, 1, 100.0, false, t0);
        l.on_forward(1, 1, &lhit(3, 1, -100.0), ms(100));
        l.on_forward(1, 1, &lhit(3, 2, -100.0), ms(110));
        l.on_ack(3, 1, 1, ms(120));
        l.on_ack(3, 1, 2, ms(130));
        assert!(l.on_vitals(3, 1, 2, 0.0, true, ms(200)).unwrap().lethal);
        assert!(l.sweep(1, ms(2000)).is_empty());
    }

    /// The god-mode rule never kills an honest owner: its own replay of the
    /// approved inputs took at least the booked loss, so when the bound
    /// reaches 0 the owner is already dead (and says so); short of that it is
    /// never forced, however long it keeps reporting.
    #[test]
    fn god_mode_rule_spares_honest_owners() {
        let mut l = Ledger::default();
        let t0 = Instant::now();
        let ms = |n: u64| t0 + Duration::from_millis(n);
        l.on_vitals(2, 1, 1, 100.0, false, t0);
        let mut hp = 100.0f32;
        let mut seq = 1;
        for k in 0..9u32 {
            // Each hit's replay did 1.2× the booked (trusted) loss.
            let h = lhit(2, k + 1, -10.0);
            let booked = trusted_loss(&h);
            l.on_forward(1, 1, &h, ms(1000 * k as u64));
            l.on_ack(2, 1, k + 1, ms(1000 * k as u64 + 20));
            hp -= 1.2 * booked;
            for d in (100..1000).step_by(100) {
                seq += 1;
                let v = l.on_vitals(2, 1, seq, hp, false, ms(1000 * k as u64 + d)).unwrap();
                assert!(!v.lethal, "honest owner at {hp} hp killed");
            }
        }
        // Non-armour-stage claims are only trusted at HM_MIN / HM_MAX.
        assert!((trusted_loss(&lhit(2, 99, -10.0)) - 10.0 * GODMODE_SIMPLE_TRUST).abs() < 1e-4);
        // A hit that is never acked (owner drops its damage) counts after FORWARD_TTL.
        let mut l = Ledger::default();
        l.on_vitals(3, 1, 1, 100.0, false, t0);
        let mut h = lhit(3, 1, -100.0);
        h.flags |= damage::FLAG_COMPLEX;
        let booked = trusted_loss(&h);
        l.on_forward(1, 1, &h, ms(0));
        l.on_vitals(3, 1, 2, 100.0, false, ms(FORWARD_TTL.as_millis() as u64 + 10));
        let b = l.godmode_bound(3, ms(FORWARD_TTL.as_millis() as u64 + 10)).unwrap();
        assert!((b - (100.0 - booked)).abs() < 2.0, "unacked hit counted: bound {b}, booked {booked}");
    }

    /// Exploit regression: a liar whose first alive report says 150 hp must
    /// not start the god-mode bound at 150 and survive 50 % more
    /// server-validated damage. The bound starts at a Willie's 100 whatever
    /// the first report says, so the same damage that kills a 100-hp liar
    /// kills a 150-hp one at the same instant.
    #[test]
    fn god_mode_bound_starts_at_a_willies_health_not_the_liars_report() {
        let t0 = Instant::now();
        let ms = |n: u64| t0 + Duration::from_millis(n);
        let killed = |first: f32| {
            let mut l = Ledger { enforce_godmode: true, ..Ledger::default() };
            l.on_vitals(2, 1, 1, first, false, t0);
            assert_eq!(l.godmode_bound(2, t0).unwrap(), HP_DEFAULT, "bound base for a first report of {first}");
            let (h1, h2) = (lhit(2, 1, -70.0), lhit(2, 2, -70.0));
            l.on_forward(1, 1, &h1, ms(100));
            l.on_ack(2, 1, 1, ms(120));
            l.on_forward(1, 1, &h2, ms(600));
            l.on_ack(2, 1, 2, ms(620));
            (700..4000).step_by(33).find(|&t| !l.sweep(1, ms(t)).is_empty())
        };
        let honest_base = killed(100.0).expect("100-hp liar declared");
        let liar = killed(150.0).expect("150-hp liar declared (was immortal against 2 blows)");
        assert_eq!(liar, honest_base);
    }

    /// Duplicates / reordered vitals are not relayed again.
    #[test]
    fn vitals_relay_gate_is_latest_wins_and_survives_a_restart() {
        let mut r = RelaySeq::default();
        assert!(r.fresh(1, 1, 10));
        assert!(!r.fresh(1, 1, 10), "duplicate");
        assert!(!r.fresh(1, 1, 9), "reordered");
        assert!(r.fresh(1, 0, 9), "per stream");
        assert!(r.fresh(1, 1, 5_000));
        assert!(r.fresh(1, 1, 1), "sidecar restart (seq far behind) starts over");
        r.forget(1);
        assert!(r.fresh(1, 1, 1));
    }

    #[test]
    fn ledger_ceiling_never_cuts_an_owner_that_reports_in_the_replay_tick() {
        // The owner replays the hit and reports in the same tick (50 ms after the
        // ack): the next frames must pass untouched (a ceiling that subtracts
        // the hit a second time would show peers 20 instead of 60).
        let mut l = Ledger::default();
        let t0 = Instant::now();
        let ms = |n: u64| t0 + Duration::from_millis(n);
        let mut f = frame(100.0, 0);
        l.on_frame(2, 1, 1, &mut f, t0);
        l.on_forward(1, 1, &lhit(2, 1, -40.0), ms(10));
        l.on_ack(2, 1, 1, ms(10));
        for (seq, at) in [(2u32, 60u64), (3, 130), (4, 400), (5, 1400)] {
            let mut f = frame(60.0, 0);
            l.on_frame(2, 1, seq, &mut f, ms(at)).unwrap();
            assert_eq!(hp_of(&f), 60.0, "frame at {at} ms cut");
        }
        // ...and a heal from 60 to 100 (no Willie heals mid-round) is clamped.
        l.on_forward(1, 1, &lhit(2, 2, -30.0), ms(1500));
        l.on_ack(2, 1, 2, ms(1500));
        let mut f = frame(100.0, 0);
        l.on_frame(2, 1, 6, &mut f, ms(1800)).unwrap();
        assert!(hp_of(&f) <= 61.0, "{:?}", f.v[0]);
    }

    /// Regression: an honest owner whose native replays leave Health at 100
    /// (Half Sword's part floors and contact gates) while the server's
    /// estimate books 5-10 HP per hit must not be shown to peers at the
    /// estimate ("I see his health drop, on his screen it doesn't") or be
    /// killed by the god-mode rule. Peers see exactly its report and it is
    /// never killed for it; the rule only flags.
    #[test]
    fn honest_owner_at_full_health_after_estimated_hits_is_shown_and_kept() {
        let mut l = Ledger::default();
        let t0 = Instant::now();
        let ms = |n: u64| t0 + Duration::from_millis(n);
        let mut f = frame(100.0, 0);
        l.on_frame(2, 1, 1, &mut f, t0);
        let mut seq = 1;
        for k in 0..40u32 {
            let mut h = lhit(2, k + 1, -9.2);
            h.flags |= damage::FLAG_COMPLEX;
            h.set_deltas(&[]);
            h.damage_out = 2.0;
            h.velocity = [1500.0, 0.0, 0.0];
            h.set_bone("spine_03");
            assert!(hit_loss(&h) > 1.0, "the estimate books a loss ({})", hit_loss(&h));
            l.on_forward(1, 1, &h, ms(300 * k as u64));
            l.on_ack(2, 1, k + 1, ms(300 * k as u64 + 20));
            for d in [100u64, 200] {
                seq += 1;
                let mut f = frame(100.0, 0);
                let v = l.on_frame(2, 1, seq, &mut f, ms(300 * k as u64 + d)).unwrap();
                assert_eq!(hp_of(&f), 100.0, "peers shown less than the owner's 100 after hit {k}");
                assert!(!v.lethal && !v.forced, "honest owner killed after hit {k}");
            }
        }
        for t in (12_000..16_000).step_by(33) { assert!(l.sweep(1, ms(t)).is_empty(), "swept at {t}"); }
        assert!(l.godmode_flagged(2), "the estimate-vs-report gap is still flagged for the operator");
        // An owner whose Health really drops is shown its drop at once.
        let mut f = frame(41.5, 0);
        l.on_frame(2, 1, seq + 1, &mut f, ms(16_100)).unwrap();
        assert_eq!(hp_of(&f), 41.5);
    }

    #[test]
    fn ledger_does_not_double_count_reflected_hits_and_allows_regen() {
        let mut l = Ledger::default();
        let t0 = Instant::now();
        l.on_vitals(2, 1, 1, 100.0, false, t0);
        l.on_forward(1, 1, &lhit(2, 1, -40.0), t0);
        l.on_ack(2, 1, 1, t0);
        // Vitals produced before the apply but arriving early: hit stays pending.
        let v = l.on_vitals(2, 1, 2, 100.0, false, t0 + Duration::from_millis(50)).unwrap();
        assert!((v.est - 60.0).abs() < 0.01, "{:?}", v);
        // Vitals after the apply reflects it: est = reported, not reported − 40.
        let v = l.on_vitals(2, 1, 3, 60.0, false, t0 + Duration::from_millis(400)).unwrap();
        assert!((v.est - 60.0).abs() < 0.01, "{:?}", v);
        // A heal faster than REGEN_PER_S (none exists mid-round) is not believed...
        let v = l.on_vitals(2, 1, 4, 70.0, false, t0 + Duration::from_millis(1400)).unwrap();
        assert!(v.hp <= 61.5, "{:?}", v);
        // ...nor a client claiming full health right after a reflected hit.
        l.on_forward(1, 1, &lhit(2, 2, -50.0), t0 + Duration::from_millis(1500));
        l.on_ack(2, 1, 2, t0 + Duration::from_millis(1500));
        let v = l.on_vitals(2, 1, 5, 100.0, false, t0 + Duration::from_millis(1800)).unwrap();
        assert!(v.hp <= 62.0 && v.est <= 62.0, "{:?}", v);
        assert!(!v.lethal);
        assert_eq!(l.last_attacker(2, 1), 1);
    }

    #[test]
    fn ledger_dead_vitals_are_round_scoped_and_ordered() {
        let mut l = Ledger::default();
        let t0 = Instant::now();
        l.on_vitals(2, 1, 50, 0.0, true, t0); // died in round 1
        // Round 2: a reordered round-1 packet (seq 49/50) must not kill.
        assert!(l.on_vitals(2, 2, 50, 0.0, true, t0).is_none());
        assert!(l.on_vitals(2, 2, 49, 0.0, true, t0).is_none());
        // A dead report before any alive report this round is ignored too.
        let v = l.on_vitals(2, 2, 51, 0.0, true, t0).unwrap();
        assert!(!v.lethal, "{:?}", v);
        // begin_round floors the seq: older packets never count afterwards.
        l.begin_round(3, t0);
        assert!(l.on_vitals(2, 3, 51, 100.0, false, t0).is_none());
        let mut l = Ledger::default();
        l.on_vitals(3, 1, 7, 100.0, false, t0);
        l.on_vitals(3, 2, 8, 100.0, false, t0);
        assert!(l.on_vitals(3, 2, 9, 0.0, true, t0).unwrap().lethal);
    }

    #[test]
    fn hit_loss_rules() {
        let mut h = hit(1);
        h.set_delta_pairs(&[(FIELD_HEALTH, -30.0)]);
        assert_eq!(hit_loss(&h), 30.0);
        h.set_delta_pairs(&[(FIELD_HEALTH, -500.0)]);
        assert_eq!(hit_loss(&h), HIT_LOSS_CAP);
        h.set_delta_pairs(&[(10, -20.0)]);
        assert_eq!(hit_loss(&h), 0.0); // consciousness only: no Health loss
        h.set_deltas(&[]);
        h.damage_out = 12.0;
        assert_eq!(hit_loss(&h), 12.0);
    }

    fn frame(hp: f32, flags: u16) -> Vitals {
        let mut f = vitals::unknown();
        for i in 0..vitals::N { vitals::set(&mut f, i, 100.0); }
        vitals::set(&mut f, vitals::I_HEALTH, hp);
        vitals::set(&mut f, vitals::I_STAMINA, 80.0);
        f.flags = flags;
        f
    }

    fn hp_of(f: &Vitals) -> f32 { f.health().unwrap() }

    #[test]
    fn vitals_frame_feeds_ledger_and_clamps_relay() {
        let mut l = Ledger::default();
        let t0 = Instant::now();
        let mut f = frame(100.0, 0);
        assert!(!l.on_frame(2, 1, 1, &mut f, t0).unwrap().lethal);
        // A 40 hp hit is forwarded and acked, then reflected by a later frame.
        l.on_forward(1, 1, &lhit(2, 1, -40.0), t0);
        l.on_ack(2, 1, 1, t0);
        let mut f = frame(60.0, 0);
        let v = l.on_frame(2, 1, 2, &mut f, t0 + Duration::from_millis(400)).unwrap();
        assert!((v.est - 60.0).abs() < 0.01, "{:?}", v);
        assert_eq!(hp_of(&f), 60.0);
        // A lying owner heals itself to full right after: the RELAYED Health
        // is clamped to the heal bound; other fields pass untouched.
        l.on_forward(1, 1, &lhit(2, 2, -30.0), t0 + Duration::from_millis(500));
        l.on_ack(2, 1, 2, t0 + Duration::from_millis(500));
        let mut f = frame(100.0, 0);
        l.on_frame(2, 1, 3, &mut f, t0 + Duration::from_millis(800)).unwrap();
        assert!(hp_of(&f) <= 61.0, "{:?}", f.v[0]);
        assert_eq!(vitals::get(&f, vitals::I_STAMINA).unwrap(), 80.0);
        // Reordered frame: no ledger information, frame untouched.
        let mut f = frame(100.0, 0);
        assert!(l.on_frame(2, 1, 2, &mut f, t0 + Duration::from_millis(900)).is_none());
        // Dead flag (DED) kills even with Health still above 0 (head trauma).
        let mut f = frame(35.0, vitals::F_DEAD | vitals::F_DOWNED);
        assert!(l.on_frame(2, 1, 4, &mut f, t0 + Duration::from_secs(1)).unwrap().lethal);
    }

    #[test]
    fn vitals_frame_without_health_is_relayed_not_booked() {
        let mut l = Ledger::default();
        let mut f = vitals::unknown();
        vitals::set(&mut f, vitals::I_STAMINA, 50.0);
        assert!(l.on_frame(9, 1, 1, &mut f, Instant::now()).is_none());
        // Dead flag without Health still counts once the owner was seen alive.
        let mut a = frame(100.0, 0);
        l.on_frame(9, 1, 2, &mut a, Instant::now());
        let mut d = vitals::unknown();
        d.flags = vitals::F_DEAD;
        assert!(l.on_frame(9, 1, 3, &mut d, Instant::now()).unwrap().lethal);
    }

    #[test]
    fn vitals_in_clamps_in_place() {
        let mut f = frame(100.0, 0);
        f.seq = 1;
        assert!(vitals_in(77, 1, &mut f).is_some());
        assert_eq!(hp_of(&f), 100.0);
    }

    fn ts_hit(id: u32, target: PeerId) -> DamageEvent {
        let mut h = hit(id);
        h.target_peer_id = target;
        h.attacker_ts = 9200;
        h.victim_view_ts = 5200;
        h.victim_arm_ts = 5200;
        h
    }

    fn ctx_at(attacker: PeerId) -> Ctx {
        let mut c = ctx(attacker);
        c.target_pos = Some([150.0, 0.0, 100.0]);
        c
    }

    #[test]
    fn lagcomp_hit_is_held_then_forwarded() {
        lagcomp_world(312, 311);
        let c = ctx_at(311);
        let t0 = Instant::now();
        assert_eq!(on_damage_at(&c, &mut ts_hit(1, 312), t0), Verdict::Hold);
        assert_eq!(on_damage_at(&c, &mut ts_hit(1, 312), t0 + Duration::from_millis(50)), Verdict::Hold);
        let out: Vec<_> = flush_pending_at(t0 + DEFENDER_GRACE)
            .into_iter().filter(|(a, _, _)| *a == 311).collect();
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].2, Verdict::Forward);
        assert_eq!(on_damage_at(&c, &mut ts_hit(1, 312), t0 + DEFENDER_GRACE), Verdict::Reforward);
        // A miss on the rewound body is rejected outright.
        let mut h = ts_hit(2, 312);
        h.location = [150.0, 120.0, 120.0];
        assert!(matches!(on_damage_at(&c, &mut h, t0), Verdict::Ack { accepted: false, .. }));
    }

    #[test]
    fn lagcomp_parry_cancels_held_hit() {
        lagcomp_world(322, 321);
        let c = ctx_at(321);
        let t0 = Instant::now();
        assert_eq!(on_damage_at(&c, &mut ts_hit(1, 322), t0), Verdict::Hold);
        // Victim's client saw the attacker's blade (attacker ts 9180) hit its own.
        lagcomp::record_clash(322, 321, 5300, 9180);
        let v = on_damage_at(&c, &mut ts_hit(1, 322), t0 + DEFENDER_GRACE);
        assert_eq!(v, Verdict::Ack { accepted: false, reason: "parried".into() });
    }

    #[test]
    fn trade_lets_a_just_killed_attacker_land() {
        lagcomp_world(332, 331);
        lagcomp::record_root(331, 9200, [0.0, 0.0, 100.0]);
        lagcomp::note_death(331);
        let mut c = ctx_at(331);
        c.attacker_alive = false;
        assert_eq!(on_damage_at(&c, &mut ts_hit(1, 332), Instant::now()), Verdict::Hold);
        // Without a timestamp the dead attacker is refused.
        assert!(matches!(on_damage_at(&c, &mut hit(2), Instant::now()), Verdict::Ack { accepted: false, .. }));
    }

    fn kit(weapon: &str, armor: &[&str]) -> crate::loadout::KitSel {
        crate::loadout::KitSel::new("custom", weapon, "", armor, [0; 4])
    }

    #[test]
    fn oversized_damage_is_clamped_before_forward_and_ledger() {
        // Legacy path (victim never streamed): no speed known (a fast swing is
        // assumed), but a 1h sword on a breastplate over the belly can't take 95 hp.
        damage::set_kit(411, &kit("w_arming1", &[]));
        damage::set_kit(412, &kit("", &["c_cuirass1"]));
        let c = ctx(411);
        let mut h = lhit(412, 1, -95.0);
        let before = cheat::get(411).damage_clamped;
        assert_eq!(on_damage_at(&c, &mut h, Instant::now()), Verdict::Forward);
        let cap = damage::loss_cap_bone(damage::WeaponClass::Sword, Some(damage::MAX_STRIKE_SPEED),
            damage::Armour::Plate, false, "spine_02");
        assert!(cap < 60.0, "{}", cap);
        assert!((hit_loss(&h) - cap).abs() < 0.01, "{} vs {}", hit_loss(&h), cap);
        assert_eq!(h.raw_damage, 40.0, "impact inputs are clamp_impact's business");
        assert_eq!(cheat::get(411).damage_clamped, before + 1);
        // A resend gets the approved (clamped) event again, not its own claim.
        let mut again = lhit(412, 1, -95.0);
        assert_eq!(on_damage_at(&c, &mut again, Instant::now()), Verdict::Reforward);
        assert!((hit_loss(&again) - cap).abs() < 0.01);
        // The ledger books the clamped loss.
        let mut l = Ledger::default();
        let v = l.on_forward(411, 1, &h, Instant::now());
        assert!((v.est - (HP_DEFAULT - cap)).abs() < 0.01, "{:?}", v);
        // A leg hit loses `Leg Health`, barely any Health (kH 0.1): a 60 hp
        // leg claim is clamped to a fraction of a hit point.
        let mut h = lhit(412, 2, -60.0);
        h.set_bone("calf_l");
        assert_eq!(on_damage_at(&c, &mut h, Instant::now()), Verdict::Forward);
        assert!(hit_loss(&h) < 2.0, "{}", hit_loss(&h));
    }

    #[test]
    fn slow_contact_caps_damage_from_server_speed() {
        // Lag-compensated hit from a weapon that has not moved: contact speed
        // 0 → only the low-speed floor of the sword's damage is plausible.
        lagcomp_world_g(422, 421, false);
        damage::set_kit(421, &kit("w_longsword1", &[]));
        let c = ctx_at(421);
        let mut h = ts_hit(1, 422);
        h.set_delta_pairs(&[(FIELD_HEALTH, -100.0)]);
        assert_eq!(on_damage_at(&c, &mut h, Instant::now()), Verdict::Forward);
        let cap = damage::loss_cap(damage::WeaponClass::Sword, Some(0.0), damage::Armour::None, false);
        assert!(hit_loss(&h) <= cap + 0.01 && cap < 50.0, "{} {}", hit_loss(&h), cap);
    }

    #[test]
    fn untimestamped_hit_on_streaming_victim_is_rejected() {
        lagcomp_world_g(432, 431, false);
        let c = ctx_at(431);
        let mut h = hit(1);
        h.target_peer_id = 432;
        match on_damage_at(&c, &mut h, Instant::now()) {
            Verdict::Ack { accepted: false, reason } => assert!(reason.contains("missing timestamps"), "{}", reason),
            v => panic!("{:?}", v),
        }
    }

    #[test]
    fn parry_spam_does_not_cancel_held_hit() {
        // Victim blade near its body (hit held for the grace) but 60+ uu from
        // the attacker's blade: its clash spam is judged invalid.
        lagcomp_world_w(442, 441, [60.0, 0.0, 120.0], [120.0, 30.0, 120.0]);
        let c = ctx_at(441);
        let t0 = Instant::now();
        let v = on_damage_at(&c, &mut ts_hit(1, 442), t0);
        assert_eq!(v, Verdict::Hold);
        for k in 0..30 { lagcomp::record_clash(442, 441, 5300 - k, 9200 - k); }
        // Forward (or Reforward if a parallel test's flush already resolved
        // it) — never "parried".
        let v = on_damage_at(&c, &mut ts_hit(1, 442), t0 + DEFENDER_GRACE);
        assert!(matches!(v, Verdict::Forward | Verdict::Reforward), "{:?}", v);
    }

    #[test]
    fn owner_ack_feeds_rtt() {
        let c = ctx(451);
        let t0 = Instant::now();
        let mut h = hit(1);
        h.target_peer_id = 452;
        assert_eq!(on_damage_at(&c, &mut h, t0), Verdict::Forward);
        assert!(on_owner_ack(452, 451, 1));
        assert!(lagcomp::clock_info(452).and_then(|i| i.rtt_ms).is_some());
    }

    /// Only the victim may ack a hit. The attacker (or a third
    /// peer) acking its own hit neither ends re-delivery nor feeds the
    /// victim's RTT.
    #[test]
    fn only_the_victim_can_ack_a_hit() {
        let c = ctx(461);
        let t0 = Instant::now();
        let mut h = hit(1);
        h.target_peer_id = 462;
        assert_eq!(on_damage_at(&c, &mut h, t0), Verdict::Forward);
        assert!(!on_owner_ack(461, 461, 1), "the attacker cannot ack its own hit");
        assert!(!on_owner_ack(999, 461, 1), "a bystander cannot ack it");
        assert!(lagcomp::clock_info(462).and_then(|i| i.rtt_ms).is_none(), "no RTT sample from a forged ack");
        let mut h = hit(1);
        h.target_peer_id = 462;
        assert_eq!(on_damage_at(&c, &mut h, t0), Verdict::Reforward, "still re-delivered to the victim");
        assert!(on_owner_ack(462, 461, 1), "the victim's ack counts");
    }
}
