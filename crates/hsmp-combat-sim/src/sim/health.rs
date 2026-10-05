//! End-to-end health replication (child module of `world`, `Config::health_model`).
//!
//! Models what the combat path alone does not: the victim's OWN game state and
//! the vitals stream that shows it to everyone else.
//!
//!   victim Lua    applies each forwarded hit ONCE as its native replay of the
//!                 server-approved Deal Complex Damage inputs through its own
//!                 armour (the only damage, solo parity); the attacker's
//!                 stand-in blow on its pawn (the echo) is restored by
//!                 HSMPCombat. Health ≤ 0 → native death.
//!   vitals        Lua samples its pawn every 2nd tick (15 Hz) and in the tick
//!                 of a replay; deadband 0.01 HP or a 1 s heartbeat → sidecar
//!                 VitalsGate (≥ 50 ms between sends, flag changes at once) →
//!                 unreliable `vitals` record.
//!   server        the REAL `combat::Ledger` (on_forward / on_ack / on_frame):
//!                 the relayed frame's Health is ledger-clamped; a death is
//!                 declared on the owner's own death (vitals) or by the stall
//!                 rule (no vitals for OWNER_STALL and booked est ≤ 0).
//!   viewers       keep the highest (life, seq) per peer: what the attacker's
//!                 HUD shows of the victim.
//!
//! The ledger's "round" is the victim's life (the sim respawns the dead after
//! RESPAWN_MS so fights go on).

use super::*;
use crate::combat::Ledger;
use crate::proto::vitals;
use std::time::{Duration, Instant};

pub const RESPAWN_MS: f64 = 1500.0;
/// HSMPCombat vitals sampling: every 2nd Lua tick (~15 Hz).
pub const VITALS_EVERY_TICKS: u64 = 2;
pub const VITALS_DEADBAND: f32 = 0.01;
pub const VITALS_HEARTBEAT_MS: f64 = 1000.0;
/// Sidecar VitalsGate (combat_client.rs).
pub const VITALS_MIN_INTERVAL_MS: f64 = 50.0;
pub const VITALS_KEYFRAME_MS: f64 = 1000.0;
/// combat_client::VITALS_REPEAT: a significant frame is repeated (newest state).
pub const VITALS_REPEAT_MS: [f64; 2] = [120.0, 360.0];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DeathCause {
    /// The owner's vitals said dead (its native death).
    Vitals,
    /// The owner's reliable death report (C2SDeath, resent until acked).
    Reported,
    /// A forwarded hit with the owner stalled and booked est ≤ 0.
    LedgerStall,
    /// The god-mode rule: validated damage lethal, owner still reporting alive.
    LedgerGodMode,
}

#[derive(Clone, Debug)]
pub struct Apply {
    pub victim: usize,
    pub attacker: usize,
    pub hit_id: u32,
    pub life: u32,
    pub t: f64,
    /// Health the victim's replay took (MP) and the same contacts in solo.
    pub mp: f32,
    pub solo: f32,
    pub hp_before: f32,
    pub hp_after: f32,
    /// What the server ledger booked for it.
    pub booked: f32,
}

#[derive(Default, Clone)]
pub struct HealthLog {
    pub applies: Vec<Apply>,
    /// Second application of one (victim, attacker, hit_id): must never happen.
    pub dup_applies: u32,
    /// Fault counter: Health the injected echo leak took (checker self-test).
    pub echo_damage: f32,
    /// Victim HUD Health changes: (victim, life, t, hp).
    pub hp_changes: Vec<(usize, u32, f64, f32)>,
    /// Viewer HUD updates: (viewer, victim, life, t, hp shown).
    pub views: Vec<(usize, usize, u32, f64, f32)>,
    /// Frames whose relayed Health the ledger cut below what the owner sent.
    pub ledger_cuts: Vec<(usize, f64, f32, f32)>,
    pub server_deaths: Vec<(usize, u32, f64, DeathCause)>,
    pub native_deaths: Vec<(usize, u32, f64)>,
    /// Victim → last vitals send (true time) per life, for the stall check.
    pub vitals_sent: Vec<(usize, u32, f64)>,
    /// GodMode cheater: (cheater, life, t) when the replays it ignored would
    /// have taken GODMODE_MARGIN × its Health (see `GODMODE_MARGIN`).
    pub godmode_lethal: Vec<(usize, u32, f64)>,
    /// (client, life, t) when the server's god-mode rule flagged it (by default
    /// only detection; a kill only with `Ledger::enforce_godmode`).
    pub godmode_flags: Vec<(usize, u32, f64)>,
}

/// A GodMode attempt is "effective" once the hits it ignored add up to this
/// many times a full Health bar: the server's god-mode bound trusts only the
/// solo-parity under-estimate of each hit (armour-stage bookings at HM_MIN,
/// simple claims at HM_MIN / HM_MAX), so a margin of HM_MAX / HM_MIN ≈ 1.29
/// (+1 %) is the documented tolerance.
pub const GODMODE_MARGIN: f32 = 1.3;
/// ...and it counts as a success if it is still fighting this long after.
pub const GODMODE_SUCCESS_MS: f64 = 2000.0;

#[derive(Clone, Debug)]
pub struct Vit {
    pub seq: u32,
    pub pending: Option<(f32, bool)>,
    pub sent: Option<(f32, bool, f64)>,
    pub repeat: Option<(f64, usize)>,
    pub last_hp: f32,
    pub last_dead: bool,
    pub last_hb: f64,
    pub ticks: u64,
}

pub struct HealthState {
    pub ledger: Ledger,
    pub t0: Instant,
    pub hp: Vec<f32>,
    pub life: Vec<u32>,
    pub native_dead: Vec<Option<f64>>,
    pub respawn_at: Vec<Option<f64>>,
    pub vit: Vec<Vit>,
    /// [viewer][victim] → (life, seq) shown
    pub view: Vec<Vec<(u32, u32)>>,
    pub applied: std::collections::HashSet<(usize, usize, u32)>,
    pub log: HealthLog,
    /// GodMode: the Health its ignored replays would have left (per life).
    pub would_be: Vec<f32>,
}

impl HealthState {
    pub fn new(n: usize) -> HealthState {
        HealthState {
            ledger: Ledger::default(),
            t0: Instant::now(),
            hp: vec![100.0; n],
            life: vec![1; n],
            native_dead: vec![None; n],
            respawn_at: vec![None; n],
            vit: (0..n).map(|_| Vit { seq: 0, pending: Some((100.0, false)), sent: None, repeat: None, last_hp: 100.0, last_dead: false, last_hb: 0.0, ticks: 0 }).collect(),
            view: vec![vec![(0, 0); n]; n],
            applied: Default::default(),
            log: HealthLog::default(),
            would_be: vec![100.0; n],
        }
    }
    pub fn inst(&self, t: f64) -> Instant { self.t0 + Duration::from_micros((t.max(0.0) * 1000.0) as u64) }
}

impl World {
    pub(super) fn stalled(&self, c: usize, t: f64) -> bool {
        self.cfg.stall.map_or(false, |(i, a, b)| i == c && t >= a && t < b)
    }

    /// The victim's Lua applies a forwarded hit (once): its native replay.
    pub(super) fn apply_replay(&mut self, c: usize, rec: usize, t: f64) -> bool {
        let attacker = self.claims[rec].attacker;
        let hit = self.pending_hits[rec].clone();
        let h = &mut self.health;
        if h.native_dead[c].is_some() { return false; }
        if !h.applied.insert((c, attacker, hit.hit_id)) {
            h.log.dup_applies += 1;
            return false;
        }
        let r = &self.claims[rec];
        // The replay of the approved inputs through the victim's armour.
        let mut mp = match (r.pcell, hit.flags & damage::FLAG_COMPLEX != 0) {
            (Some((_, armour, _, hm, stab)), true) => {
                let raw = game::deal(hit.damage_out, len(hit.velocity), armour, hit.cutting_power, hit.pain_rate);
                game::health_loss(2.0 * hm * raw, stab, r.bone)
            }
            // Hand / fallback claims: the booked (capped) loss.
            _ => combat::hit_loss(&hit),
        };
        let mut solo = r.solo.unwrap_or(r.native_loss);
        let hp0 = h.hp[c];
        if self.clients[c].cheat == Some(Cheat::GodMode) {
            // Acked (the sidecar acks on receipt) but never applied: its HUD
            // and vitals stay where they are. Track what it should have.
            h.would_be[c] -= mp;
            let life = h.life[c];
            if h.would_be[c] <= 100.0 * (1.0 - GODMODE_MARGIN)
                && !h.log.godmode_lethal.iter().any(|x| x.0 == c && x.1 == life)
            {
                h.log.godmode_lethal.push((c, life, t));
                if std::env::var("SIM_DEBUG_GOD").is_ok() {
                    let pid = self.clients[c].pid;
                    eprintln!("GODMODE-LETHAL c{} life{} t={:.0} would_be={:.1} bound={:?}", c, life, t, h.would_be[c],
                        h.ledger.godmode_bound(pid, h.inst(t)));
                }
            }
            return false;
        }
        if r.lethal {
            // Trade engagement: both were on their last legs; the blow kills.
            mp = mp.max(hp0);
            solo = solo.max(hp0);
        }
        let mut hp1 = hp0 - mp;
        if self.cfg.echo_leak {
            // Fault injection: the stand-in's blow on my pawn not restored.
            hp1 -= r.native_loss;
            h.log.echo_damage += r.native_loss;
        }
        h.hp[c] = hp1;
        let life = h.life[c];
        h.log.applies.push(Apply { victim: c, attacker, hit_id: hit.hit_id, life, t, mp, solo, hp_before: hp0, hp_after: hp1, booked: combat::hit_loss(&hit) });
        if (hp1 - hp0).abs() > 1e-6 { h.log.hp_changes.push((c, life, t, hp1.max(0.0))); }
        if hp1 <= 0.0 {
            h.native_dead[c] = Some(t);
            h.log.native_deaths.push((c, life, t));
            if h.respawn_at[c].is_none() { h.respawn_at[c] = Some(t + RESPAWN_MS); }
            // HSMPCombat death event → sidecar → reliable C2SDeath.
            let rto = self.rto(c);
            let pick = t + self.clients[c].rng.range(0.5, SIDECAR_TICK_MS + 0.5);
            for at in self.clients[c].up.send_reliable(pick, rto) {
                self.push(at, Msg::SDeath { from: c, life });
            }
        }
        true
    }

    /// HSMPCombat vitals sampling (Lua side) + respawn.
    pub(super) fn vitals_lua(&mut self, c: usize, t: f64, applied_now: bool) {
        let h = &mut self.health;
        if let Some(at) = h.respawn_at[c] {
            if t >= at {
                h.respawn_at[c] = None;
                h.native_dead[c] = None;
                h.life[c] += 1;
                h.hp[c] = 100.0;
                h.would_be[c] = 100.0;
                h.log.hp_changes.push((c, h.life[c], t, 100.0));
                self.alive[c] = true;
                h.vit[c].last_dead = !h.vit[c].last_dead; // force a frame
            }
        }
        let v = &mut h.vit[c];
        v.ticks += 1;
        if v.ticks % VITALS_EVERY_TICKS != 0 && !applied_now { return; }
        let hp = h.hp[c].max(0.0);
        let dead = h.native_dead[c].is_some();
        if (hp - v.last_hp).abs() > VITALS_DEADBAND || dead != v.last_dead || t - v.last_hb >= VITALS_HEARTBEAT_MS {
            v.pending = Some((hp, dead));
            v.last_hp = hp;
            v.last_dead = dead;
            v.last_hb = t;
        }
    }

    /// Sidecar VitalsGate: what to send now.
    pub(super) fn vitals_sidecar(&mut self, c: usize, t: f64) {
        let h = &mut self.health;
        let v = &mut h.vit[c];
        let mut out: Option<(f32, bool)> = None;
        if let Some((hp, dead)) = v.pending {
            let (go, sig) = match v.sent {
                None => (true, true),
                Some((shp, sdead, at)) => {
                    let sig = dead != sdead || (hp - shp).abs() > VITALS_DEADBAND;
                    (dead != sdead || (sig && t - at >= VITALS_MIN_INTERVAL_MS) || t - at >= VITALS_KEYFRAME_MS, sig)
                }
            };
            if go {
                v.pending = None;
                if sig { v.repeat = Some((t, 0)); }
                out = Some((hp, dead));
            }
        }
        if out.is_none() {
            if let Some((at, k)) = v.repeat {
                if k >= VITALS_REPEAT_MS.len() { v.repeat = None; }
                else if t >= at + VITALS_REPEAT_MS[k] {
                    v.repeat = Some((at, k + 1));
                    out = v.pending.take().or(v.sent.map(|s| (s.0, s.1)));
                }
            }
        }
        let Some((hp, dead)) = out else { return };
        v.sent = Some((hp, dead, t));
        v.seq += 1;
        let (seq, life) = (v.seq, h.life[c]);
        h.log.vitals_sent.push((c, life, t));
        for at in self.clients[c].up.send(t) {
            self.push(at, Msg::SVitals { from: c, seq, hp, dead, life, ..Default::default() });
        }
    }

    /// Server: the `vitals` record → ledger (on_frame) → relay the clamped frame.
    pub(super) fn vitals_server(&mut self, from: usize, seq: u32, hp: f32, dead: bool, life: u32, t: f64) {
        let pid = self.clients[from].pid;
        // The owner's `vitals` record (quantised at the source, as the game writes it).
        let mut f = vitals::unknown();
        f.seq = seq;
        vitals::set(&mut f, vitals::I_HEALTH, hp);
        if dead { f.flags |= vitals::F_DEAD; }
        let now = self.health.inst(t);
        let verdict = self.health.ledger.on_frame(pid, life, seq, &mut f, now);
        self.note_godmode_flag(from, life, t);
        let shown = f.health().unwrap_or(hp);
        if shown + 0.05 < hp { self.health.log.ledger_cuts.push((from, t, hp, shown)); }
        if let Some(v) = verdict {
            if v.lethal && life == self.health.life[from] {
                let cause = if v.forced { DeathCause::LedgerGodMode } else { DeathCause::Vitals };
                if v.forced && std::env::var("SIM_DEBUG_GOD").is_ok() {
                    eprintln!("GODMODE-DEATH c{} life{} t={:.0} hp={} cheat={:?} bound={:?}", from, life, t, hp, self.clients[from].cheat,
                        self.health.ledger.godmode_bound(pid, now));
                }
                self.declare_death(from, t, cause);
            }
        }
        for to in 0..self.cfg.players {
            if to == from { continue; }
            for at in self.clients[to].down.send(t) {
                self.push(at, Msg::CVitals { to, from, seq, hp: shown, life, ..Default::default() });
            }
        }
    }

    /// The server's god-mode rule flags (counter + warning) instead
    /// of killing by default; record when it first flagged this life.
    pub(super) fn note_godmode_flag(&mut self, c: usize, life: u32, t: f64) {
        if life != self.health.life[c] { return; }
        let pid = self.clients[c].pid;
        if self.health.ledger.godmode_flagged(pid) && !self.health.log.godmode_flags.iter().any(|x| x.0 == c && x.1 == life) {
            self.health.log.godmode_flags.push((c, life, t));
        }
    }

    pub(super) fn death_report(&mut self, from: usize, life: u32, t: f64) {
        if life == self.health.life[from] { self.declare_death(from, t, DeathCause::Reported); }
    }

    pub(super) fn vitals_view(&mut self, to: usize, from: usize, seq: u32, hp: f32, life: u32, t: f64) {
        let cur = self.health.view[to][from];
        if (life, seq) > cur {
            self.health.view[to][from] = (life, seq);
            self.health.log.views.push((to, from, life, t, hp));
        }
    }

    pub(super) fn ledger_forward(&mut self, a: usize, target: usize, hit: &DamageEvent, t: f64) {
        let life = self.health.life[target];
        let now = self.health.inst(t);
        let v = self.health.ledger.on_forward(self.clients[a].pid, life, hit, now);
        if v.lethal { self.declare_death(target, t, DeathCause::LedgerStall); }
    }

    pub(super) fn ledger_ack(&mut self, victim: usize, attacker: usize, hit_id: u32, t: f64) {
        let now = self.health.inst(t);
        let (vp, ap) = (self.clients[victim].pid, self.clients[attacker].pid);
        self.health.ledger.on_ack(vp, ap, hit_id, now);
    }

    pub(super) fn declare_death(&mut self, victim: usize, t: f64, cause: DeathCause) {
        if !self.alive[victim] { return; }
        self.alive[victim] = false;
        self.dead_at[victim] = t;
        let pid = self.clients[victim].pid;
        self.core.note_death(pid, t as i64);
        let life = self.health.life[victim];
        self.health.log.server_deaths.push((victim, life, t, cause));
        // A server-declared death (stall rule) with the pawn still up: the
        // owner is killed / respawned with the round like any death.
        if self.health.respawn_at[victim].is_none() { self.health.respawn_at[victim] = Some(t + RESPAWN_MS); }
    }

    pub fn health_log(&self) -> &HealthLog { &self.health.log }
}

/// End-to-end checks over one or more runs (see tests/combat_sim.rs).
#[derive(Default, Clone, Debug)]
pub struct HealthStats {
    pub applies: usize,
    pub dup_applies: u32,
    /// Victim HUD Health lost beyond its replays (echo not restored, double
    /// application): must be 0.
    pub echo_damage: f32,
    /// Lives with ≥ N applied hits: Σ MP / Σ solo after the first N.
    pub n_hits_ratio: Vec<f64>,
    /// Every 10 ms, per (viewer, victim): how long ago the victim's HUD held
    /// the Health the viewer shows (0 = current).
    pub view_stale: Vec<f64>,
    /// Samples where the viewer showed a value the victim's HUD never had.
    pub view_wrong: usize,
    /// Frames the ledger relayed below the owner's own Health.
    pub ledger_cuts: usize,
    pub server_deaths: usize,
    pub native_deaths: usize,
    /// Server deaths with neither a native death before them (same life) nor
    /// a stalled owner: must be 0.
    pub false_deaths: usize,
    /// Native deaths the server never declared within 1 s (owner not stalled).
    pub missed_deaths: usize,
    pub stall_deaths: usize,
    /// Server death − native death (ms).
    pub death_latency: Vec<f64>,
}

pub const N_HITS: usize = 5;

impl HealthStats {
    pub fn add(&mut self, w: &World) {
        let l = &w.health.log;
        let n = w.cfg.players;
        self.applies += l.applies.len();
        self.dup_applies += l.dup_applies;
        // (3) Health the victim lost to anything but its replays (from the HUD
        // records, independent of the fault counter).
        self.echo_damage += l.applies.iter().map(|a| (a.hp_before - a.hp_after - a.mp).max(0.0)).sum::<f32>();
        // (1) Health after N hits vs solo, per life.
        let mut by_life: std::collections::BTreeMap<(usize, u32), Vec<&Apply>> = Default::default();
        for a in &l.applies { by_life.entry((a.victim, a.life)).or_default().push(a); }
        for v in by_life.values() {
            if v.len() < N_HITS || v.iter().take(N_HITS).any(|a| a.hp_before < a.mp || a.hp_before < a.solo) { continue; }
            let (m, s): (f64, f64) = v.iter().take(N_HITS).fold((0.0, 0.0), |acc, a| (acc.0 + a.mp as f64, acc.1 + a.solo as f64));
            if s > 1.0 { self.n_hits_ratio.push(m / s); }
        }
        // (2) Viewer agreement: sampled every 10 ms.
        let end = w.cfg.dur_ms + 2500.0;
        for victim in 0..n {
            let mut tl: Vec<(f64, u32, f32)> = vec![(0.0, 1, 100.0)];
            tl.extend(l.hp_changes.iter().filter(|x| x.0 == victim).map(|x| (x.2, x.1, x.3)));
            for viewer in 0..n {
                if viewer == victim { continue; }
                let vs: Vec<(f64, u32, f32)> = l.views.iter().filter(|x| x.0 == viewer && x.1 == victim).map(|x| (x.3, x.2, x.4)).collect();
                if vs.is_empty() { continue; }
                let (mut i, mut j) = (0usize, 0usize);
                let mut tau = vs[0].0;
                while tau <= end {
                    while i + 1 < vs.len() && vs[i + 1].0 <= tau { i += 1; }
                    while j + 1 < tl.len() && tl[j + 1].0 <= tau { j += 1; }
                    let (_, sl, sh) = vs[i];
                    let same = |e: &(f64, u32, f32)| e.1 == sl && (e.2.max(0.0) - sh).abs() < 0.05;
                    if same(&tl[j]) {
                        self.view_stale.push(0.0);
                    } else if let Some(k) = (0..j).rev().find(|&k| same(&tl[k])) {
                        self.view_stale.push(tau - tl[k + 1].0);
                        if std::env::var("SIM_STALE").is_ok() && tau - tl[k + 1].0 > 1200.0 { eprintln!("STALE viewer{} victim{} tau={:.0} shown=({},{:.2}) since {:.0} cur=({},{:.2})", viewer, victim, tau, sl, sh, tl[k + 1].0, tl[j].1, tl[j].2); }
                    } else {
                        self.view_wrong += 1;
                    }
                    tau += 10.0;
                }
            }
        }
        self.ledger_cuts += l.ledger_cuts.len();
        // (4) Deaths.
        let stalled_at = |v: usize, t: f64| -> bool {
            let last = l.vitals_sent.iter().filter(|x| x.0 == v && x.2 <= t).map(|x| x.2).fold(f64::MIN, f64::max);
            t - last >= crate::combat::OWNER_STALL.as_millis() as f64
        };
        self.server_deaths += l.server_deaths.len();
        self.native_deaths += l.native_deaths.len();
        for &(v, life, t, cause) in &l.server_deaths {
            let native = l.native_deaths.iter().find(|x| x.0 == v && x.1 == life && x.2 <= t);
            match native {
                Some(x) => self.death_latency.push(t - x.2),
                // (Or the owner really is hung: its last vitals before the stall
                // were lost on a lossy link, so the server's 2.5 s ran out sooner.)
                None if cause == DeathCause::LedgerStall && (stalled_at(v, t) || w.stalled(v, t)) => self.stall_deaths += 1,
                // The god-mode rule (server tick) on an owner whose Lua is hung (it never
                // applies the lethal replay) or a GodMode cheater: what it is for.
                None if cause == DeathCause::LedgerGodMode && (w.stalled(v, t) || w.clients[v].cheat == Some(Cheat::GodMode)) => self.stall_deaths += 1,
                None => { if std::env::var("SIM_DEBUG_GOD").is_ok() { eprintln!("FALSE-DEATH v{} life{} t={:.0} {:?} natives={:?}", v, life, t, cause, l.native_deaths.iter().filter(|x| x.0 == v).collect::<Vec<_>>()); } self.false_deaths += 1 }
            }
        }
        for &(v, life, t) in &l.native_deaths {
            let declared = l.server_deaths.iter().any(|x| x.0 == v && x.1 == life && x.2 <= t + 1000.0);
            if !declared && !stalled_at(v, t) && t + 1000.0 <= w.cfg.dur_ms + 2500.0 { self.missed_deaths += 1; }
        }
    }
}
