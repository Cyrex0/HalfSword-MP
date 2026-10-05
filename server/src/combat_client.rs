//! Sidecar side of combat replication (server model: combat.rs; records:
//! hsmp-ipc schema/combat.rs). Everything is one binary representation: the game
//! writes a record, this side copies its bytes behind an 8-byte wire header (filling
//! ids / rounds / ages / seqs in place where its logic needs to), and the records the
//! server sends are copied into the S2G ring or a peer slot as they are.
//!
//! Game -> server (G2S ring / the `vitals` slot, `records_in.rs`):
//!   `damage`        a claim (`cid` = the game's claim id). Gets a `hit_id` and the
//!                   current `round` here, `age_ms` = lage + time held here on every
//!                   (re)send; resent every RESEND_EVERY until a FINAL verdict, given
//!                   up after HIT_GIVE_UP (a local FINAL "timeout" verdict), at most
//!                   MAX_PENDING_HITS pending (else a local FINAL "queue_full").
//!   `clash`/`touch` parry / contact evidence, framed as they are (clash twice).
//!   `death_report`  our own death (HSMPCombat's native death, HSMPSync's Health
//!                   poll): one per round, resent every DEATH_RESEND until `death_ack`,
//!                   given up after DEATH_GIVE_UP.
//!   `vitals` slot   our vitals (quantised by the game): offered to the VitalsGate
//!                   (deadband, 50 ms rate cap, urgent flags / parts, 1 s keyframe,
//!                   repeats at +120 / +360 ms); `seq` patched with this side's seq.
//! Server -> game (`handle_server_record` -> `on_server_record`):
//!   `damage_in`     a hit on OUR pawn: always acked (`damage_ack`), pushed S2G once
//!                   per (attacker, hit_id).
//!   `hitfx_in`      an accepted hit on another player: pushed S2G once per hit.
//!   `damage_verdict` CONFIRM / FINAL for our claims (`cid` patched in from the pending
//!                   table; FINAL ends the resends) and CLASH (pushed as it is).
//!   `death`         a server-declared death, pushed once per (epoch, match, peer,
//!                   round) with `match_id` / `wall_ms` filled in place.
//!   `vitals`        a peer's vitals (`WireHdr.peer` = owner), highest seq wins
//!                   (wrap-safe; a big jump back = the peer's sidecar restarted),
//!                   written into that peer's `peer_vitals` slot.

use crate::proto::vitals::{self, Vitals};
use crate::SharedState;
use hsmp_ipc::record::view;
use hsmp_ipc::schema::combat::{
    Damage, DamageAck, DamageVerdict, Death, DeathAck, DeathReport, K_CLASH, K_DAMAGE, K_DAMAGE_IN,
    K_DAMAGE_VERDICT, K_DEATH, K_DEATH_ACK, K_DEATH_REPORT, K_HITFX_IN, K_TOUCH, K_VITALS, VERDICT_CLASH,
    VERDICT_CONFIRM, VERDICT_FINAL,
};
use hsmp_ipc::wire;
use hsmp_ipc::schema::combat::{ReplayOutcome,K_REPLAY_OUTCOME,K_REPLAY_OUTCOME_ACK};
use std::collections::{HashMap, HashSet, VecDeque};

use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};
use tokio::net::UdpSocket;
use tokio::sync::Mutex;
use tokio::task::JoinHandle;
use tokio::time;
use tracing::{debug, info, warn};

const TICK: Duration = Duration::from_millis(5);
const RESEND_EVERY: Duration = Duration::from_millis(120);
const HIT_GIVE_UP: Duration = Duration::from_secs(2);
const OUTCOME_GIVE_UP:Duration=Duration::from_secs(10);
const DEATH_RESEND: Duration = Duration::from_millis(150);
const DEATH_GIVE_UP: Duration = Duration::from_secs(10);
const SEEN_CAP: usize = 4096;
const MAX_PENDING_HITS: usize = 256;
/// Most a claim may have waited in the game before its send (ms).
const LAGE_MAX: u32 = 1000;
/// Vitals rate cap (20 Hz) and keyframe period.
pub const VITALS_MIN_INTERVAL: Duration = Duration::from_millis(50);
pub const VITALS_KEYFRAME: Duration = Duration::from_millis(1000);
/// A significant frame is repeated this long after it (the newest state):
/// the stream is unreliable, and a lost hit frame otherwise left peers
/// showing the old Health until the 1 s keyframe (sim: up to 2 s stale).
pub const VITALS_REPEAT: [Duration; 2] = [Duration::from_millis(120), Duration::from_millis(360)];

/// Byte offsets (in a framed message: the 8-byte wire header + the record) of the
/// fields this side fills in place.
const OFF_HIT_ID: usize = wire::HDR + core::mem::offset_of!(Damage, hit_id);
const OFF_AGE: usize = wire::HDR + core::mem::offset_of!(Damage, age_ms);

/// Owner-side send policy for the vitals stream. The game offers every frame it
/// writes; `poll` (every sidecar tick) says what to send now. A held frame
/// stays pending, so the newest state always goes out eventually.
#[derive(Default)]
pub struct VitalsGate {
    sent: Option<(Vitals, Instant)>,
    pending: Option<Vitals>,
    /// (last significant send, next VITALS_REPEAT index)
    repeat: Option<(Instant, usize)>,
}

impl VitalsGate {
    pub fn offer(&mut self, f: Vitals) {
        self.pending = Some(f);
    }

    pub fn poll(&mut self, now: Instant) -> Option<Vitals> {
        if let Some(cand) = self.pending.as_ref() {
            let (go, significant) = match &self.sent {
                None => (true, true),
                Some((last, at)) => {
                    let el = now.duration_since(*at);
                    let urgent = vitals::urgent_vs(cand, last);
                    let differs = vitals::differs(cand, last);
                    (urgent || (differs && el >= VITALS_MIN_INTERVAL) || el >= VITALS_KEYFRAME, urgent || differs)
                }
            };
            if go {
                let f = self.pending.take()?;
                self.sent = Some((f, now));
                if significant { self.repeat = Some((now, 0)); }
                return Some(f);
            }
        }
        // Redundancy: repeat the newest state after a significant send.
        let (at, k) = self.repeat?;
        if k >= VITALS_REPEAT.len() { self.repeat = None; return None; }
        if now < at + VITALS_REPEAT[k] { return None; }
        self.repeat = Some((at, k + 1));
        let f = match self.pending.take() { Some(p) => p, None => self.sent.as_ref()?.0 };
        self.sent = Some((f, now));
        Some(f)
    }
}

struct PendingHit {
    /// The framed C2S `damage` message (hit_id / round filled; age_ms per send).
    msg: Vec<u8>,
    /// The game's claim id (verdicts), 0 = none.
    cid: u32,
    /// ms between the hit and the game's send.
    lage: u32,
    created: Instant,
    last_sent: Option<Instant>,
    sends: u32,
}

struct PendingDeath {
    death_id: u32,
    report: DeathReport,
    created: Instant,
    last_sent: Option<Instant>,
}

type OutcomeKey=(u64,u32,u32,u32,u16);
fn outcome_key(r:&ReplayOutcome)->OutcomeKey {(r.match_id,r.round,r.attacker,r.hit_id,r.victim_life)}
struct PendingOutcome {record:ReplayOutcome,created:Instant,last_sent:Option<Instant>}
struct IncomingHit {attacker:u32,payload:Vec<u8>,created:Instant,last_queued:Option<Instant>}
#[derive(Default)]
struct Side {
    next_hit_id: u32,
    next_death_id: u32,
    pending_hits: HashMap<u32, PendingHit>,
    /// G2S callback order, independent of randomized map iteration, identical
    /// creation times, and hit-id wrap. Bounded by MAX_PENDING_HITS.
    pending_order: VecDeque<u32>,
    /// A death report from the game: Some(its round, 0 = the current one).
    death_requested: Option<DeathReport>,
    pending_death: Option<PendingDeath>,
    seen: VecDeque<(u32, u32)>,
    seen_set: HashSet<(u32, u32)>,
    /// `hitfx_in` already pushed (attacker, hit_id).
    fx_seen: VecDeque<(u32, u32)>,
    fx_seen_set: HashSet<(u32, u32)>,
    acks_out: Vec<(u32, u32)>,
    outcomes:HashMap<OutcomeKey,PendingOutcome>,
    outcome_order:VecDeque<OutcomeKey>,
    incoming:VecDeque<IncomingHit>,
    vitals_seq_out: u32,
    vitals_seq_in: HashMap<u32, u32>,
    vitals_context_in: HashMap<u32,(u64,u32,u16)>,
    vitals_gate: VitalsGate,
    vitals_sent: u64,
    /// Messages produced off the combat task (G2S evidence), sent with its next batch.
    out: Vec<Vec<u8>>,
}

impl Side {
    fn remove_hit(&mut self, id: u32) -> Option<PendingHit> {
        let hit = self.pending_hits.remove(&id)?;
        self.pending_order.retain(|queued| *queued != id);
        Some(hit)
    }
}

fn side() -> &'static std::sync::Mutex<Side> {
    static S: OnceLock<std::sync::Mutex<Side>> = OnceLock::new();
    S.get_or_init(|| std::sync::Mutex::new(Side { next_hit_id: 1, next_death_id: 1, ..Default::default() }))
}

fn lock() -> std::sync::MutexGuard<'static, Side> {
    side().lock().unwrap_or_else(|e| e.into_inner())
}

/// The (server epoch, match id) deaths are scoped to (from the session snapshot).
static MATCH: std::sync::Mutex<(u64, u64)> = std::sync::Mutex::new((0, 0));

/// Session snapshot accepted: the current server instance and match.
pub fn set_match(epoch: u64, match_id: u64) {
    *MATCH.lock().unwrap_or_else(|e| e.into_inner()) = (epoch, match_id);
}

fn current_match() -> (u64, u64) {
    *MATCH.lock().unwrap_or_else(|e| e.into_inner())
}

/// Authoritative deaths already pushed, keyed (epoch, match id, peer, round).
fn deaths_seen() -> &'static std::sync::Mutex<VecDeque<(u64, u64, u32, u32, u16)>> {
    static S: OnceLock<std::sync::Mutex<VecDeque<(u64, u64, u32, u32, u16)>>> = OnceLock::new();
    S.get_or_init(|| std::sync::Mutex::new(VecDeque::new()))
}

/// Welcome / new server epoch: peer ids and vitals seqs
/// restart with the server, and so do match ids and rounds. Forget the old
/// instance's high-water marks and death keys.
pub fn reset_session() {
    {
        let mut s = lock();
        s.vitals_seq_in.clear();
        s.vitals_context_in.clear();
        s.incoming.clear();s.outcomes.clear();s.outcome_order.clear();
        s.pending_death=None;s.death_requested=None;
        s.seen.clear();
        s.seen_set.clear();
        s.fx_seen.clear();
        s.fx_seen_set.clear();
    }
    deaths_seen().lock().unwrap_or_else(|e| e.into_inner()).clear();
}

/// The current round (the session snapshot's, lock-free; SESSION accessor).

fn link() -> Option<&'static crate::ipc_shm::ShmLink> {
    crate::ipc_shm::link()
}

/// Push a typed S2G record for the game.
fn to_game(kind: u16, peer: u32, payload: &[u8]) -> bool {
    link().is_some_and(|l|l.push_record(kind,0,peer,payload))
}

/// A local verdict for the game (timeout / queue_full): the same record the server sends.
fn local_final(cid: u32, reason: &str) {
    if cid == 0 {
        return;
    }
    let v = DamageVerdict::new(0, cid, VERDICT_FINAL, false, reason);
    to_game(K_DAMAGE_VERDICT, 0, hsmp_ipc::bytemuck::bytes_of(&v));
}

// ---- game -> server -------------------------------------------------------------------------

/// A validated G2S record of the combat domain (`records_in::on_g2s`, hsmp-ipc thread:
/// short locks only).
pub fn on_g2s(kind: u16, payload: &[u8]) {
    match kind {
        K_REPLAY_OUTCOME => {
            let Ok(v)=view::<ReplayOutcome>(payload) else {return};
            let r=v.head();let key=outcome_key(&r);
            let mut s=lock();
            s.incoming.retain(|i|view::<Damage>(&i.payload).map_or(true,|d| !(i.attacker==r.attacker
                && d.head.hit_id==r.hit_id && d.head.match_id==r.match_id && d.head.round==r.round && d.head.victim_life==r.victim_life)));
            if let Some(old)=s.outcomes.get(&key) {
                if hsmp_ipc::bytemuck::bytes_of(&old.record)!=payload {warn!(hit_id=r.hit_id,"conflicting game replay outcome ignored");}
                return;
            }
            if s.outcomes.len()>=4096 {warn!("native outcome queue full");return;}
            s.outcomes.insert(key,PendingOutcome{record:r,created:Instant::now(),last_sent:None});
            s.outcome_order.push_back(key);
        }
        K_DAMAGE => {
            let Ok(v) = view::<Damage>(payload) else { return };
            let (cid, lage) = (v.head.cid, v.head.lage_ms.min(LAGE_MAX));
            drop(v);
            let mut s = lock();
            if s.pending_hits.len() >= MAX_PENDING_HITS {
                drop(s);
                warn!("combat: pending hit queue full; dropping new hit");
                // Close the game's claim (else it waits forever).
                local_final(cid, "queue_full");
                return;
            }
            let id = s.next_hit_id;
            s.next_hit_id = s.next_hit_id.wrapping_add(1).max(1);
            let mut msg = wire::message(K_DAMAGE, 0, 0, payload);
            msg[OFF_HIT_ID..OFF_HIT_ID + 4].copy_from_slice(&id.to_le_bytes());
            // Original match/round/life belong to the native callback; never relabel them.
            s.pending_hits.insert(id, PendingHit { msg, cid, lage, created: Instant::now(), last_sent: None, sends: 0 });
            s.pending_order.push_back(id);
        }
        K_CLASH | K_TOUCH => {
            let msg = wire::message(kind, 0, 0, payload);
            let mut s = lock();
            if kind == K_CLASH {
                debug!("combat: weapon clash reported");
                // Tiny: a second copy rides out a reordering / single loss burst.
                s.out.push(msg.clone());
            }
            s.out.push(msg);
        }
        K_DEATH_REPORT => {
            let Ok(v) = view::<DeathReport>(payload) else { return };
            lock().death_requested = Some(v.head());
        }
        k => debug!(kind = k, "combat: G2S record kind not sent by the game; dropped"),
    }
}

/// The game's own `vitals` slot, read on change (any thread: seqlock reader).
struct VitalsReader {
    ver: u64,
    seq: Option<u32>,
    scratch: Vec<u64>,
    buf: Vec<u8>,
}

impl VitalsReader {
    fn poll(&mut self) -> Option<Vitals> {
        let l = link()?;
        let seg = l.segment();
        let slot = seg.slot_ref("vitals", 0)?;
        let ver = slot.version();
        if ver == self.ver {
            return None;
        }
        self.ver = ver;
        let (meta, kind, _) = slot.get(&mut self.scratch, &mut self.buf)?;
        let h = &seg.header;
        let fresh = meta.valid == 1
            && meta.writer_epoch == h.game.epoch.load(std::sync::atomic::Ordering::Acquire)
            && meta.world_epoch == h.world_epoch.load(std::sync::atomic::Ordering::Acquire);
        if !fresh || kind != K_VITALS {
            return None;
        }
        let f = view::<Vitals>(&self.buf).ok()?.head();
        if self.seq == Some(f.seq) {
            return None;
        }
        self.seq = Some(f.seq);
        Some(f)
    }
}

pub fn spawn_task(sock: Arc<UdpSocket>, _shared: Arc<Mutex<SharedState>>) -> JoinHandle<()> {
    tokio::spawn(async move {
        let mut vr = VitalsReader { ver: 0, seq: None, scratch: Vec::new(), buf: Vec::new() };
        let mut vitals_log_at = Instant::now();
        let mut ticker = time::interval(TICK);
        ticker.set_missed_tick_behavior(time::MissedTickBehavior::Skip);
        loop {
            ticker.tick().await;
            let fresh_vitals = vr.poll();
            let msgs = step(Instant::now(), fresh_vitals, &mut vitals_log_at);
            if !msgs.is_empty() {
                if let Err(e) = crate::net::send_msgs(&sock, msgs).await {
                    debug!(error = %e, "combat send failed");
                }
            }
        }
    })
}

/// One combat tick: everything due now, as framed record messages (one transmit).
fn step(now: Instant, fresh_vitals: Option<Vitals>, vitals_log_at: &mut Instant) -> Vec<Vec<u8>> {
    let mut finals: Vec<u32> = Vec::new();
    let mut sends = {
        let mut s = lock();
        let mut sends = std::mem::take(&mut s.out);
        if let Some(f) = fresh_vitals {
            s.vitals_gate.offer(f);
        }

        let mut expired = Vec::new();
        let order: Vec<u32> = s.pending_order.iter().copied().collect();
        for id in order {
            let Some(p) = s.pending_hits.get_mut(&id) else { continue };
            if now.duration_since(p.created) > HIT_GIVE_UP {
                expired.push(id);
                continue;
            }
            if p.last_sent.map_or(true, |t| now.duration_since(t) >= RESEND_EVERY) {
                let age = p.lage.saturating_add(now.duration_since(p.created).as_millis().min(u32::MAX as u128) as u32);
                p.msg[OFF_AGE..OFF_AGE + 4].copy_from_slice(&age.to_le_bytes());
                p.last_sent = Some(now);
                p.sends += 1;
                sends.push(p.msg.clone());
            }
        }
        for id in expired {
            if let Some(p) = s.remove_hit(id) {
                warn!(hit_id = id, sends = p.sends, "combat: hit never acked; giving up");
                // A final answer for every claim.
                finals.push(p.cid);
            }
        }

        if let Some(mut report) = s.death_requested.take() {
            let round=report.round;
            let same_round = s.pending_death.as_ref().map_or(false, |d| d.report.round==report.round
                && d.report.match_id==report.match_id && d.report.life==report.life);
            if !same_round {
                let id = s.next_death_id;
                s.next_death_id = s.next_death_id.wrapping_add(1).max(1);
                info!(death_id = id, round, "combat: own death queued");
                report.death_id=id;
                s.pending_death = Some(PendingDeath { death_id:id,report,created:now,last_sent:None });
            }
        }
        let mut drop_death = false;
        if let Some(d) = s.pending_death.as_mut() {
            if now.duration_since(d.created) > DEATH_GIVE_UP {
                warn!(death_id = d.death_id, "combat: death never acked; giving up");
                drop_death = true;
            } else if d.last_sent.map_or(true, |t| now.duration_since(t) >= DEATH_RESEND) {
                d.last_sent = Some(now);
                sends.push(wire::encode(0, 0, &d.report, &[]));
            }
        }
        if drop_death { s.pending_death = None; }

        for (attacker, hit_id) in std::mem::take(&mut s.acks_out) {
            sends.push(wire::encode(0, 0, &DamageAck { attacker, hit_id }, &[]));
        }

        let keys:Vec<_>=s.outcome_order.iter().copied().collect();
        let mut expired_outcomes=Vec::new();let mut sent_outcomes=0;
        for key in keys {
            let Some(p)=s.outcomes.get_mut(&key) else {continue};
            if now.duration_since(p.created)>OUTCOME_GIVE_UP {expired_outcomes.push(key);continue;}
            if sent_outcomes<64 && p.last_sent.map_or(true,|t|now.duration_since(t)>=RESEND_EVERY) {
                p.last_sent=Some(now);sent_outcomes+=1;
                sends.push(wire::encode(0,0,&p.record,&[]));
            }
        }
        for key in expired_outcomes {s.outcomes.remove(&key);s.outcome_order.retain(|k|*k!=key);}
        // Original first-delivery order is preserved. Later IPC retries are
        // harmless because the Lua native-attempt cache executes each id once.
        s.incoming.retain(|i|now.duration_since(i.created)<=HIT_GIVE_UP);
        for i in s.incoming.iter_mut() {
            if i.last_queued.map_or(true,|t|now.duration_since(t)>=RESEND_EVERY) {
                if !to_game(K_DAMAGE_IN,i.attacker,&i.payload) {break;}
                i.last_queued=Some(now);
            }
        }
        if let Some(mut f) = s.vitals_gate.poll(now) {
            s.vitals_seq_out = s.vitals_seq_out.wrapping_add(1);
            s.vitals_sent += 1;
            f.seq = s.vitals_seq_out;
            sends.push(wire::encode(0, 0, &f, &[]));
        }
        if now.duration_since(*vitals_log_at) >= Duration::from_secs(10) {
            *vitals_log_at = now;
            if s.vitals_sent > 0 {
                info!(frames = s.vitals_sent, per_s = s.vitals_sent as f32 / 10.0, "vitals: sent frames in the last 10 s");
                s.vitals_sent = 0;
            }
        }
        sends
    };
    for cid in finals {
        local_final(cid, "timeout");
    }
    sends.retain(|m| !m.is_empty());
    sends
}

// ---- server -> game -------------------------------------------------------------------------

/// A record of the combat domain from the server (validated here, then copied).
pub fn on_server_record(h: wire::WireHdr, payload: &[u8]) {
    if let Err(e) = hsmp_ipc::schema::check_payload(h.kind, payload) {
        debug!(kind = h.kind, error = %e, "combat: invalid record from the server dropped");
        return;
    }
    match h.kind {
        K_DAMAGE_IN => on_damage_in(h.peer, payload),
        K_REPLAY_OUTCOME_ACK => {
            if let Ok(v)=view::<ReplayOutcome>(payload) {
                let r=v.head();let key=outcome_key(&r);
                let mut s=lock();
                if s.outcomes.get(&key).is_some_and(|p|hsmp_ipc::bytemuck::bytes_of(&p.record)==payload) {
                    s.outcomes.remove(&key);s.outcome_order.retain(|k|*k!=key);
                }
                drop(s);to_game(K_REPLAY_OUTCOME_ACK,h.peer,payload);
            }
        }
        K_REPLAY_OUTCOME => {to_game(K_REPLAY_OUTCOME,h.peer,payload);},
        K_HITFX_IN => on_hit_fx(h.peer, payload),
        K_DAMAGE_VERDICT => on_verdict(payload),
        K_DEATH_ACK => {
            if let Ok(v) = view::<DeathAck>(payload) {
                on_death_ack(v.head.death_id);
            }
        }
        K_DEATH => on_death(payload),
        K_VITALS => on_vitals(h.peer, payload),
        k => debug!(kind = k, "combat: record kind the server does not send; dropped"),
    }
}

/// Remember `key` in a bounded dedup window; false if it was there already.
fn first_time(q: &mut VecDeque<(u32, u32)>, set: &mut HashSet<(u32, u32)>, key: (u32, u32)) -> bool {
    if !set.insert(key) {
        return false;
    }
    q.push_back(key);
    while q.len() > SEEN_CAP {
        if let Some(old) = q.pop_front() { set.remove(&old); }
    }
    true
}

/// `damage_in`: a hit on OUR pawn. Pushed once per (attacker, hit_id); always acked.
fn on_damage_in(attacker: u32, payload: &[u8]) {
    let Ok(v) = view::<Damage>(payload) else { return };
    let (hit_id, bone, dmg) = (v.head.hit_id, v.head.bone, v.head.damage_out);
    let is_new = {
        let mut s = lock();
        s.acks_out.push((attacker, hit_id));
        let s = &mut *s;
        first_time(&mut s.seen, &mut s.seen_set, (attacker, hit_id))
    };
    if is_new {
        let mut s=lock();
        if s.incoming.len()>=4096 {warn!(attacker,hit_id,"combat game delivery queue full");return;}
        // Enqueue once immediately for low latency, retain until Lua outcome.
        let queued= !s.incoming.iter().any(|i|i.last_queued.is_none()) && to_game(K_DAMAGE_IN,attacker,payload);
        s.incoming.push_back(IncomingHit{attacker,payload:payload.to_vec(),created:Instant::now(),
            last_queued:queued.then(Instant::now)});
        info!(attacker, hit_id, bone = bone.as_str().unwrap_or(""), dmg, "combat: hit on us");
    }
}

/// `hitfx_in`: an accepted hit on ANOTHER player, pushed once per
/// (attacker, hit_id); HSMPCombat replays it on its stand-in of the victim.
fn on_hit_fx(attacker: u32, payload: &[u8]) {
    let Ok(v) = view::<Damage>(payload) else { return };
    let (hit_id, victim) = (v.head.hit_id, v.head.target_peer_id);
    let is_new = {
        let mut s = lock();
        let s = &mut *s;
        first_time(&mut s.fx_seen, &mut s.fx_seen_set, (attacker, hit_id))
    };
    if is_new {
        to_game(K_HITFX_IN, attacker, payload);
        debug!(attacker, victim, hit_id, "combat: hit fx");
    }
}

/// `damage_verdict`: CONFIRM (not final: keep resending), FINAL (stop), CLASH.
fn on_verdict(payload: &[u8]) {
    let Ok(v) = view::<DamageVerdict>(payload) else { return };
    let mut rec = v.head();
    match rec.kind {
        VERDICT_CLASH => {
            to_game(K_DAMAGE_VERDICT, rec.peer, payload);
            return;
        }
        VERDICT_CONFIRM => {
            match lock().pending_hits.get(&rec.hit_id).map(|p| p.cid) {
                Some(cid) if cid != 0 => rec.cid = cid,
                _ => return,
            }
        }
        _ => {
            let Some(p) = lock().remove_hit(rec.hit_id) else { return };
            if rec.ok.get() {
                debug!(hit_id = rec.hit_id, sends = p.sends, "combat: hit delivered");
            } else {
                info!(hit_id = rec.hit_id, reason = rec.reason.as_str().unwrap_or(""), "combat: hit rejected by server");
            }
            if p.cid == 0 {
                return;
            }
            rec.cid = p.cid;
        }
    }
    to_game(K_DAMAGE_VERDICT, 0, hsmp_ipc::bytemuck::bytes_of(&rec));
}

/// `death` (authoritative, re-sent ~3 Hz while the round lasts): pushed ONCE per
/// (epoch, match, peer, round) with the match id and the receive time filled in.
fn on_death(payload: &[u8]) {
    let Ok(v) = view::<Death>(payload) else { return };
    let mut d = v.head();
    // Scoped by match: the server restarts rounds at 1 every match.
    let (epoch, match_id) = current_match();
    if d.match_id!=match_id {
        return;
    }
    // Preserve server-authenticated original match and pawn life.
    d.wall_ms = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |t| t.as_millis() as u64);
    if !deliver_death_once(epoch, &d, || to_game(K_DEATH, d.peer_id, hsmp_ipc::bytemuck::bytes_of(&d))) {
        return;
    }
    info!(peer_id = d.peer_id, round = d.round, killer = d.killer, cause = d.cause, "combat: authoritative death");
}

/// Commit dedup only after the game IPC accepted the outcome. A full ring
/// leaves it eligible for the server's next reliable resend.
fn deliver_death_once(epoch: u64, d: &Death, deliver: impl FnOnce() -> bool) -> bool {
    let mut seen = deaths_seen().lock().unwrap_or_else(|e| e.into_inner());
    let key = (epoch, d.match_id, d.peer_id, d.round, d.life);
    if seen.contains(&key) || !deliver() { return false; }
    seen.push_back(key);
    while seen.len() > 256 { seen.pop_front(); }
    true
}

/// Records the key; false when this death was already pushed.
#[cfg(test)]
pub(crate) fn death_is_new_life(epoch: u64, match_id: u64, peer_id: u32, round: u32,life:u16) -> bool {
    let mut seen = deaths_seen().lock().unwrap_or_else(|e| e.into_inner());
    let key = (epoch, match_id, peer_id, round,life);
    if seen.contains(&key) { return false; }
    seen.push_back(key);
    while seen.len() > 256 { seen.pop_front(); }
    true
}

#[cfg(test)]
pub(crate) fn death_is_new(epoch:u64,match_id:u64,peer_id:u32,round:u32)->bool {
    death_is_new_life(epoch,match_id,peer_id,round,0)
}

/// A deathmatch respawn (the `mode` record's life count of `peer_id` went up): its next
/// death in the same round is news again.
pub fn forget_death(peer_id: u32, round: u32) {
    // Real life keys remain sticky across respawns; only legacy unit fixtures use zero.
    deaths_seen().lock().unwrap_or_else(|e| e.into_inner()).retain(|k| !(k.2 == peer_id && k.3 == round && k.4==0));
}

pub fn on_death_ack(death_id: u32) {
    let mut s = lock();
    if s.pending_death.as_ref().map_or(false, |d| d.death_id == death_id) {
        info!(death_id, "combat: death acknowledged");
        s.pending_death = None;
    }
}

/// Latest-wins by seq per peer (wrapping compare; a big backwards jump means
/// the peer's sidecar restarted). Records `seq` when accepted.
#[cfg(test)]
fn accept_vitals_seq(peer_id: u32, seq: u32) -> bool {
    accept_vitals_context(peer_id,seq,(0,0,0))
}

fn accept_vitals_context(peer_id:u32,seq:u32,context:(u64,u32,u16))->bool {
    let mut s=lock();
    if let Some(previous)=s.vitals_context_in.get(&peer_id).copied() {
        if context.0==previous.0 && (context.1,context.2)<(previous.1,previous.2) {return false;}
        if previous!=context {s.vitals_seq_in.remove(&peer_id);}
    }
    s.vitals_context_in.insert(peer_id,context);
    let last = s.vitals_seq_in.get(&peer_id).copied();
    let newer = match last {
        None => true,
        Some(l) => { let d = seq.wrapping_sub(l); d != 0 && d < u32::MAX / 2 }
    };
    let restarted = last.map_or(false, |l| l.wrapping_sub(seq) > 1000 && l.wrapping_sub(seq) < u32::MAX / 2);
    if !newer && !restarted {
        return false;
    }
    s.vitals_seq_in.insert(peer_id, seq);
    true
}

/// `vitals` of peer `peer` -> its `peer_vitals` slot (HSMPCombat mirrors it onto the
/// stand-in; HSMPAvatars / HSMPHud read it).
fn on_vitals(peer: u32, payload: &[u8]) {
    let Ok(v) = view::<Vitals>(payload) else { return };
    if peer == 0 || v.head.match_id!=current_match().1
        || !accept_vitals_context(peer,v.head.seq,(v.head.match_id,v.head.round,v.head.life)) {
        return;
    }
    if let Some(l) = link() {
        l.post_record("peer_vitals", Some(peer), K_VITALS, payload);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use hsmp_ipc::layout::Str;
    use hsmp_ipc::record::to_payload;

    /// The handler tests share the process-wide combat state and the test link.
    fn serial() -> std::sync::MutexGuard<'static, ()> {
        crate::ipc_shm::ShmLink::test_lock()
    }

    fn claim(cid: u32) -> Vec<u8> {
        let d = Damage { cid, target_peer_id: 4, lage_ms: 12, bone: Str::new("neck_01"), flags: 32, damage_out: 0.85,
                         velocity: [1500.0, 0.0, 0.0], attacker_ts: 5000, ..Default::default() };
        to_payload(&d, &[])
    }

    fn frame(hp: f32, stamina: f32, flags: u16) -> Vitals {
        let mut f = vitals::unknown();
        for i in 0..vitals::N { vitals::set(&mut f, i, 100.0); }
        vitals::set(&mut f, vitals::I_HEALTH, hp);
        vitals::set(&mut f, vitals::I_STAMINA, stamina);
        f.v[18] = vitals::UNKNOWN;
        f.flags = flags;
        f
    }

    /// The whole claim path in one test (process-wide state): a G2S claim gets its hit_id,
    /// round and age filled in place and is resent until the FINAL verdict, whose cid is
    /// patched in; a CONFIRM keeps it pending; an unknown claim times out with a local
    /// verdict; clash / touch / death reports are framed as they are.
    #[test]
    fn claim_send_order_survives_equal_times_wrap_resends_and_removal() {
        let _g = serial();
        *lock() = Side { next_hit_id: u32::MAX - 1, next_death_id: 1, ..Default::default() };
        let t0 = Instant::now();
        for cid in [379, 380, 381, 382] { on_g2s(K_DAMAGE, &claim(cid)); }
        {
            let mut s = lock();
            for p in s.pending_hits.values_mut() { p.created = t0; }
            assert_eq!(s.pending_order.len(), 4);
        }
        let ids = |batch: Vec<Vec<u8>>| -> Vec<(u32, u32)> {
            batch.iter().filter(|m| wire::kind_of(m) == K_DAMAGE)
                .map(|m| { let (_, v) = wire::decode::<Damage>(m).unwrap(); (v.head.hit_id, v.head.cid) }).collect()
        };
        let expected = vec![(u32::MAX - 1, 379), (u32::MAX, 380), (1, 381), (2, 382)];
        let mut log_at = t0;
        assert_eq!(ids(step(t0, None, &mut log_at)), expected, "first-send callback order");
        assert_eq!(ids(step(t0 + RESEND_EVERY, None, &mut log_at)), expected, "resend callback order");
        let final_payload = to_payload(&DamageVerdict::new(u32::MAX, 0, VERDICT_FINAL, true, ""), &[]);
        on_server_record(wire::WireHdr { kind: K_DAMAGE_VERDICT, aux: 0, peer: 0 }, &final_payload);
        let remaining = vec![(u32::MAX - 1, 379), (1, 381), (2, 382)];
        assert_eq!(ids(step(t0 + RESEND_EVERY * 2, None, &mut log_at)), remaining);
        assert_eq!(lock().pending_order.len(), 3, "FINAL removes the order entry");
        step(t0 + HIT_GIVE_UP + Duration::from_millis(1), None, &mut log_at);
        assert!(lock().pending_hits.is_empty());
        assert!(lock().pending_order.is_empty(), "expiration removes order entries");
        *lock() = Side { next_hit_id: 1, next_death_id: 1, ..Default::default() };
    }

    #[test]
    fn claims_verdicts_and_reports() {
        let _g = serial();
        let l = crate::ipc_shm::ShmLink::test_global();
        let mut log_at = Instant::now();
        let t0 = Instant::now();
        on_g2s(K_DAMAGE, &claim(7));
        let sent = step(t0, None, &mut log_at);
        let m = sent.iter().find(|m| wire::kind_of(m) == K_DAMAGE).expect("claim sent");
        let (_, v) = wire::decode::<Damage>(m).unwrap();
        let hit_id = v.head.hit_id;
        assert!(hit_id != 0 && v.head.cid == 7 && v.head.target_peer_id == 4);
        assert_eq!(v.head.bone, "neck_01");
        assert!(v.head.age_ms >= 12, "age = lage + held");
        assert!(step(t0 + Duration::from_millis(50), None, &mut log_at).iter().all(|m| wire::kind_of(m) != K_DAMAGE), "not before the resend period");
        assert!(step(t0 + Duration::from_millis(130), None, &mut log_at).iter().any(|m| wire::kind_of(m) == K_DAMAGE), "resent");
        // CONFIRM: cid patched, still pending.
        let c = to_payload(&DamageVerdict::new(hit_id, 0, VERDICT_CONFIRM, true, "confirm"), &[]);
        on_server_record(wire::WireHdr { kind: K_DAMAGE_VERDICT, aux: 0, peer: 0 }, &c);
        let ev = l.test_records(K_DAMAGE_VERDICT);
        let got = view::<DamageVerdict>(&ev.last().unwrap().2).unwrap().head();
        assert_eq!((got.cid, got.kind), (7, VERDICT_CONFIRM));
        assert!(lock().pending_hits.contains_key(&hit_id));
        // FINAL reject: cid patched, pending gone, no more resends.
        let f = to_payload(&DamageVerdict::new(hit_id, 0, VERDICT_FINAL, false, "range: out of range (900 > 800)"), &[]);
        on_server_record(wire::WireHdr { kind: K_DAMAGE_VERDICT, aux: 0, peer: 0 }, &f);
        let got = view::<DamageVerdict>(&l.test_records(K_DAMAGE_VERDICT).last().unwrap().2).unwrap().head();
        assert_eq!((got.cid, got.ok.get(), got.code), (7, false, 16));
        assert!(!lock().pending_hits.contains_key(&hit_id));
        // A verdict for an unknown hit is dropped (a resend's duplicate answer).
        on_server_record(wire::WireHdr { kind: K_DAMAGE_VERDICT, aux: 0, peer: 0 }, &f);
        assert!(l.test_records(K_DAMAGE_VERDICT).is_empty());
        // Timeout: a local FINAL verdict for the game.
        on_g2s(K_DAMAGE, &claim(8));
        step(t0, None, &mut log_at);
        step(t0 + Duration::from_secs(3), None, &mut log_at);
        let got = view::<DamageVerdict>(&l.test_records(K_DAMAGE_VERDICT).last().unwrap().2).unwrap().head();
        assert_eq!((got.cid, got.kind, got.code), (8, VERDICT_FINAL, hsmp_ipc::schema::combat::REASON_TIMEOUT));
        // Clash twice, touch once, framed as the game wrote them.
        let c = to_payload(&hsmp_ipc::schema::combat::Clash { other_peer_id: 3, my_ts: 10, other_ts: 9, _r: 0 }, &[]);
        on_g2s(K_CLASH, &c);
        on_g2s(K_TOUCH, &c);
        let sent = step(t0 + Duration::from_secs(4), None, &mut log_at);
        assert_eq!(sent.iter().filter(|m| wire::kind_of(m) == K_CLASH).count(), 2);
        assert_eq!(sent.iter().filter(|m| wire::kind_of(m) == K_TOUCH).count(), 1);
        assert_eq!(&sent.iter().find(|m| wire::kind_of(m) == K_TOUCH).unwrap()[wire::HDR..], &c[..]);
        // Death report: one per round, resent until acked.
        on_g2s(K_DEATH_REPORT, &to_payload(&DeathReport { death_id: 0, round: 3, ..Default::default() }, &[]));
        let sent = step(t0 + Duration::from_secs(5), None, &mut log_at);
        let (_, d) = wire::decode::<DeathReport>(sent.iter().find(|m| wire::kind_of(m) == K_DEATH_REPORT).unwrap()).unwrap();
        assert_eq!(d.head.round, 3);
        let id = d.head.death_id;
        assert!(step(t0 + Duration::from_millis(5200), None, &mut log_at).iter().any(|m| wire::kind_of(m) == K_DEATH_REPORT));
        on_death_ack(id);
        assert!(step(t0 + Duration::from_millis(5400), None, &mut log_at).iter().all(|m| wire::kind_of(m) != K_DEATH_REPORT));
    }

    /// damage_in: acked always, pushed once; hitfx_in pushed once; deaths once per
    /// (match, peer, round) with match_id / wall_ms filled.
    #[test]
    fn inbound_dedup_and_fill() {
        let _g = serial();
        let l = crate::ipc_shm::ShmLink::test_global();
        let mut d = Damage { hit_id: 91_001, target_peer_id: 4, bone: Str::new("head"), ..Default::default() };
        let p = to_payload(&d, &[]);
        let hdr = wire::WireHdr { kind: K_DAMAGE_IN, aux: 0, peer: 9 };
        on_server_record(hdr, &p);
        on_server_record(hdr, &p); // a resend
        let got = l.test_records(K_DAMAGE_IN);
        assert_eq!(got.len(), 1);
        assert_eq!((got[0].1, &got[0].2[..]), (9, &p[..]), "copied as it is, peer = attacker");
        let acks = std::mem::take(&mut lock().acks_out);
        assert_eq!(acks.iter().filter(|a| **a == (9, 91_001)).count(), 2, "every copy acked");
        d.hit_id = 91_002;
        let fx = to_payload(&d, &[]);
        on_server_record(wire::WireHdr { kind: K_HITFX_IN, aux: 0, peer: 9 }, &fx);
        on_server_record(wire::WireHdr { kind: K_HITFX_IN, aux: 0, peer: 9 }, &fx);
        assert_eq!(l.test_records(K_HITFX_IN).len(), 1);
        // Hostile bytes: dropped before anything happens.
        on_server_record(hdr, &p[..p.len() - 1]);
        assert!(l.test_records(K_DAMAGE_IN).is_empty());

        let e = 0xD1E5_0000 + std::process::id() as u64;
        set_match(e, 101);
        let mut original=Death::new(2,1,3,1);original.match_id=101;original.life=1;
        let dm=to_payload(&original,&[]);
        on_server_record(wire::WireHdr { kind: K_DEATH, aux: 0, peer: 2 }, &dm);
        on_server_record(wire::WireHdr { kind: K_DEATH, aux: 0, peer: 2 }, &dm);
        let got = l.test_records(K_DEATH);
        assert_eq!(got.len(), 1, "~3 Hz resend of the same death");
        let v = view::<Death>(&got[0].2).unwrap().head();
        assert_eq!((v.peer_id, v.round, v.match_id), (2, 1, 101));
        assert!(v.wall_ms > 0);
        set_match(e, 102);
        on_server_record(wire::WireHdr { kind: K_DEATH, aux: 0, peer: 2 }, &dm);
        assert!(l.test_records(K_DEATH).is_empty(), "old authenticated match cannot be rebound on receipt");
        original.match_id=102;
        on_server_record(wire::WireHdr {kind:K_DEATH,aux:0,peer:2}, &to_payload(&original,&[]));
        assert_eq!(l.test_records(K_DEATH).len(),1,"actual new match death accepted");
    }

    #[test]
    fn native_defeat_delivery_retries_full_game_ring_before_dedup() {
        let d = Death { match_id: 891, peer_id: 89101, round: 3, life: 130,
            cause: 5, ..Default::default() };
        assert!(!deliver_death_once(891, &d, || false));
        assert!(deliver_death_once(891, &d, || true), "failed IPC must not consume the outcome");
        assert!(!deliver_death_once(891, &d, || panic!("duplicate must not enter IPC")));
        let next = Death { life: 131, ..d };
        assert!(deliver_death_once(891, &next, || true));
    }

    #[test]
    fn deaths_are_scoped_by_match_and_epoch() {
        let e = 0xD1E6_0000 + std::process::id() as u64;
        assert!(death_is_new(e, 100, 2, 1));
        assert!(!death_is_new(e, 100, 2, 1));
        assert!(death_is_new(e, 100, 2, 2));
        assert!(death_is_new(e, 101, 2, 1));
        assert!(death_is_new(e + 1, 100, 2, 1), "server restart: new epoch");
        // A respawn in the same round: the next death of that peer is news again.
        forget_death(2, 1);
        assert!(death_is_new(e, 100, 2, 1));
        assert!(!death_is_new(e, 100, 2, 2), "other rounds are kept");
        // A restarted server's vitals seqs start at 1 again.
        let peer = 0x00AB_0000 + (std::process::id() & 0xFFFF);
        assert!(accept_vitals_seq(peer, 500));
        assert!(!accept_vitals_seq(peer, 3), "older within the session: dropped");
        reset_session();
        assert!(accept_vitals_seq(peer, 3), "after Welcome / new epoch: accepted");
    }

    #[test]
    fn peer_vitals_land_in_the_peer_slot() {
        let _g = serial();
        let l = crate::ipc_shm::ShmLink::test_global();
        let peer = 0x00AC_0000 + (std::process::id() & 0xFFFF);
        let mut f = frame(55.5, 12.0, vitals::F_FALLEN);
        f.seq = 99;f.match_id=current_match().1;
        let p = to_payload(&f, &[]);
        on_server_record(wire::WireHdr { kind: K_VITALS, aux: 0, peer }, &p);
        let (k, got) = l.test_slot("peer_vitals", Some(peer)).expect("posted");
        assert_eq!((k, &got[..]), (K_VITALS, &p[..]), "the record as it came");
        f.seq = 98; // older: not posted again
        f.v[0] = 0;
        on_server_record(wire::WireHdr { kind: K_VITALS, aux: 0, peer }, &to_payload(&f, &[]));
        assert_eq!(l.test_slot("peer_vitals", Some(peer)).unwrap().1, p);
    }

    #[test]
    fn vitals_gate_deadband_rate_and_keyframe() {
        let t0 = Instant::now();
        let ms = |n: u64| t0 + Duration::from_millis(n);
        let mut g = VitalsGate::default();
        let st = |f: &Vitals| vitals::get(f, vitals::I_STAMINA).unwrap();
        assert!(g.poll(t0).is_none(), "nothing offered");
        g.offer(frame(100.0, 100.0, 0));
        assert!(g.poll(t0).is_some(), "first frame goes at once");
        assert!(g.poll(ms(1)).is_none(), "nothing pending");
        // A significant frame is repeated at +120 and +360 ms (lost datagrams).
        assert!(g.poll(ms(119)).is_none());
        assert_eq!(g.poll(ms(120)).expect("repeat 1").health(), Some(100.0));
        assert!(g.poll(ms(200)).is_none());
        // Stamina regen jitter below the band: no send of its own; the repeat
        // carries the newest state.
        g.offer(frame(100.0, 99.8, 0));
        assert!(g.poll(ms(300)).is_none());
        assert!((st(&g.poll(ms(360)).expect("repeat 2")) - 99.8).abs() < 0.02);
        g.offer(frame(100.0, 99.7, 0));
        assert!(g.poll(ms(999)).is_none(), "repeats done; below the band");
        assert!(g.poll(ms(1359)).is_none());
        let k = g.poll(ms(1360)).expect("keyframe");
        assert!((st(&k) - 99.7).abs() < 0.02);
        // A hit: significant, but inside the 50 ms rate cap -> waits, then goes.
        g.offer(frame(70.0, 99.8, 0));
        assert!(g.poll(ms(1380)).is_none());
        assert_eq!(g.poll(ms(1410)).unwrap().health(), Some(70.0));
        // Newer offer replaces the pending one (latest wins).
        g.offer(frame(60.0, 99.8, 0));
        g.offer(frame(50.0, 99.8, 0));
        assert!(g.poll(ms(1420)).is_none());
        assert_eq!(g.poll(ms(1460)).unwrap().health(), Some(50.0));
        // Fallen flag: urgent, ignores the rate cap.
        g.offer(frame(50.0, 99.8, vitals::F_FALLEN));
        assert!(g.poll(ms(1461)).is_some());
        // Steady state: 20 Hz stamina drain sends ≤ 20 frames/s.
        let mut sent = 0;
        for i in 0..200u64 {
            g.offer(frame(50.0, 99.0 - i as f32 * 0.4, vitals::F_FALLEN)); // 0.4/5 ms = 80/s
            if g.poll(ms(2000 + i * 5)).is_some() { sent += 1; }
        }
        assert!((18..=21).contains(&sent), "sent {}", sent);
    }

    /// Decoder fuzz of the sidecar handlers: arbitrary bytes and mutations of valid records
    /// of every combat kind, as server records and as G2S records. Nothing panics; an
    /// invalid payload never reaches the game or the server.
    #[test]
    fn handlers_survive_hostile_bytes() {
        let _g = serial();
        use rand::{Rng, SeedableRng};
        let l = crate::ipc_shm::ShmLink::test_global();
        let mut rng = rand::rngs::StdRng::seed_from_u64(0xC0BA7);
        let seeds: Vec<(u16, Vec<u8>)> = vec![
            (K_DAMAGE_IN, claim(1)), (K_HITFX_IN, claim(2)), (K_DAMAGE, claim(3)),
            (K_DAMAGE_VERDICT, to_payload(&DamageVerdict::new(5, 0, VERDICT_FINAL, false, "x"), &[])),
            (K_DEATH, to_payload(&Death::new(2, 1, 3, 1), &[])), (K_VITALS, to_payload(&frame(50.0, 50.0, 0), &[])),
            (K_DEATH_REPORT, to_payload(&DeathReport { death_id: 0, round: 1, ..Default::default() }, &[])),
            (K_CLASH, to_payload(&hsmp_ipc::schema::combat::Clash { other_peer_id: 3, my_ts: 1, other_ts: 2, _r: 0 }, &[])),
        ];
        for i in 0..4000 {
            let (k, mut d) = seeds[i % seeds.len()].clone();
            if i % 5 == 0 { d = (0..rng.gen_range(0..300)).map(|_| rng.gen()).collect(); }
            for _ in 0..rng.gen_range(0..4) {
                if !d.is_empty() { let j = rng.gen_range(0..d.len()); d[j] = rng.gen(); }
            }
            if rng.gen_bool(0.1) { d.truncate(rng.gen_range(0..=d.len())); }
            let valid = hsmp_ipc::schema::check_payload(k, &d).is_ok();
            on_server_record(wire::WireHdr { kind: k, aux: 0, peer: rng.gen_range(0..5) }, &d);
            if valid { on_g2s(k, &d); }
        }
        // Whatever reached the game is a valid record of its kind.
        for k in [K_DAMAGE_IN, K_HITFX_IN, K_DAMAGE_VERDICT, K_DEATH] {
            for (_, _, p) in l.test_records(k) {
                assert!(hsmp_ipc::schema::check_payload(k, &p).is_ok(), "invalid {k:#06x} pushed to the game");
            }
        }
        lock().pending_hits.clear();
        lock().pending_order.clear();
    }

    /// The vitals record on the wire: 48 B + the 8-byte header (the codec frame was
    /// 43 B + a bincode envelope of 16 B + the 8-byte v6 header = 67 B).
    #[test]
    fn full_game_ring_cannot_reorder_first_native_deliveries() {
        let _g=serial();let l=crate::ipc_shm::ShmLink::test_global();
        *lock()=Side{next_hit_id:1,next_death_id:1,..Default::default()};
        l.test_records(K_TOUCH);l.test_records(K_DAMAGE_IN);
        for _ in 0..crate::ipc_shm::S2G_OVERFLOW {l.push_record(K_TOUCH,0,0,&[]);}
        let first=Damage{target_peer_id:4,hit_id:379,match_id:7,round:2,victim_life:1,bone:Str::new("pelvis"),..Default::default()};
        let second=Damage{hit_id:380,..first};
        on_damage_in(700,&to_payload(&first,&[]));
        assert!(lock().incoming.front().unwrap().last_queued.is_none());
        l.test_records(K_TOUCH);
        on_damage_in(700,&to_payload(&second,&[]));
        assert!(l.test_records(K_DAMAGE_IN).is_empty(),"later hit cannot use ring capacity ahead of held earlier hit");
        let now=Instant::now();let mut log=now;step(now,None,&mut log);
        let ids:Vec<_>=l.test_records(K_DAMAGE_IN).into_iter().map(|(_,_,p)|view::<Damage>(&p).unwrap().head.hit_id).collect();
        assert_eq!(ids,vec![379,380]);
        let r=ReplayOutcome{match_id:7,round:2,attacker:700,hit_id:379,victim_life:1,status:2,..Default::default()};
        on_g2s(K_REPLAY_OUTCOME,&to_payload(&r,&[]));
        assert_eq!(lock().incoming.len(),1,"Lua outcome settles IPC delivery retry independently of transport ACK");
        *lock()=Side{next_hit_id:1,next_death_id:1,..Default::default()};
    }

    #[test]
    fn original_life_receipts_and_vitals_survive_retries_without_rebinding() {
        let _g=serial();let l=crate::ipc_shm::ShmLink::test_global();
        *lock()=Side{next_hit_id:1,next_death_id:1,..Default::default()};
        let r=ReplayOutcome{match_id:80,round:2,attacker:7,hit_id:9,victim_life:3,status:1,
            observed_fields:1,health_delta:-2.5,..Default::default()};
        on_g2s(K_REPLAY_OUTCOME,&to_payload(&r,&[]));
        let t=Instant::now();let mut log=t;
        let batches=[step(t,None,&mut log),step(t+RESEND_EVERY,None,&mut log)];
        for b in batches {let m=b.iter().find(|m|wire::kind_of(m)==K_REPLAY_OUTCOME).unwrap();
            assert_eq!(wire::decode::<ReplayOutcome>(m).unwrap().1.head.match_id,80);}
        let mut wrong=r;wrong.health_delta=-99.0;
        on_server_record(wire::WireHdr{kind:K_REPLAY_OUTCOME_ACK,aux:0,peer:0},&to_payload(&wrong,&[]));
        assert_eq!(lock().outcomes.len(),1,"conflicting ACK cannot settle native receipt");
        on_server_record(wire::WireHdr{kind:K_REPLAY_OUTCOME_ACK,aux:0,peer:0},&to_payload(&r,&[]));
        assert!(lock().outcomes.is_empty());l.test_records(K_REPLAY_OUTCOME_ACK);
        assert!(accept_vitals_context(700,500,(80,2,3)));
        assert!(accept_vitals_context(700,1,(80,2,4)),"new life resets seq gate");
        assert!(!accept_vitals_context(700,501,(80,2,3)),"late old life cannot poison new sequence");
        let report=DeathReport{match_id:80,round:2,life:3,..Default::default()};
        on_g2s(K_DEATH_REPORT,&to_payload(&report,&[]));
        let b=step(t,None,&mut log);let m=b.iter().find(|m|wire::kind_of(m)==K_DEATH_REPORT).unwrap();
        let original=wire::decode::<DeathReport>(m).unwrap().1.head;
        assert_eq!((original.match_id,original.round,original.life),(80,2,3));
        assert!(death_is_new_life(800,80,700,2,3));forget_death(700,2);
        assert!(!death_is_new_life(800,80,700,2,3),"respawn does not forget actual old life");
        assert!(death_is_new_life(800,80,700,2,4));
        *lock()=Side{next_hit_id:1,next_death_id:1,..Default::default()};
    }

    #[test]
    fn vitals_message_size() {
        let mut f = frame(87.0, 61.0, 0);
        f.seq = 1;
        assert_eq!(wire::encode(0, 0, &f, &[]).len(), 72);
    }
}
