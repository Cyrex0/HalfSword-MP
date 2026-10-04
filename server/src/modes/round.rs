//! `RoundCore`: the last-standing round engine shared by Duel, FFA-LMS and
//! LTS. It is the original `server.rs` match loop re-expressed in milliseconds,
//! generalised from "participants" to "sides" (a seat, or a team). With
//! seat grouping it reproduces server.rs exactly; every lifted rule cites the
//! server.rs lines it came from (commit c180540; see docs/development/subsystems/modes.md).

use super::traits::*;
use super::zone::{Zone, ZoneDamage, ZoneTable};
use std::collections::{BTreeMap, BTreeSet};

/// Same as match_core `SETTLE_MS` (400 ms). Covers
/// DEFENDER_GRACE (200 ms) plus trade-hit transit.
pub const SETTLE_MS: Ms = 400;
/// Same as match_core `RECONNECT_GRACE_MS` (30 s).
pub const RECONNECT_GRACE_MS: Ms = 30_000;

/// server.rs L1358: `needed_wins = (best_of + 1) / 2`.
pub fn needed_wins(best_of: u8) -> u32 {
    (best_of as u32).div_ceil(2)
}

/// When the match ends.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MatchRule {
    /// A side reaching this many round wins takes the match.
    pub wins: u32,
    /// After this many decided rounds (wins + draws), the leader by wins,
    /// then kills, takes it; a tie is a match draw. 0 = no cap (Duel).
    pub max_rounds: u32,
}

#[derive(Clone, Debug, PartialEq)]
pub struct SuddenDeath {
    /// Round time limit; 0 = off.
    pub round_ms: Ms,
    pub table: ZoneTable,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AssistRule {
    pub window_ms: Ms,
    pub min_damage: f32,
}

#[derive(Clone, Debug, PartialEq)]
pub struct CoreCfg {
    pub settle_ms: Ms,
    pub pause_grace_ms: Ms,
    pub rule: MatchRule,
    pub sudden_death: Option<SuddenDeath>,
    pub assists: Option<AssistRule>,
    /// Sudden-death ring centre (MapData), else the standing players' centroid.
    pub map_center: Option<[f32; 3]>,
}

impl CoreCfg {
    pub fn new(rule: MatchRule) -> Self {
        Self { settle_ms: SETTLE_MS, pause_grace_ms: RECONNECT_GRACE_MS, rule, sudden_death: None, assists: None, map_center: None }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SeatStats {
    pub kills: u16,
    pub deaths: u16,
    pub assists: u16,
}

#[derive(Clone, Debug)]
pub struct RoundCore {
    pub cfg: CoreCfg,
    /// Some => team grouping (LTS); None => every seat is its own side.
    teams: Option<BTreeMap<SeatId, TeamId>>,
    /// server.rs `participants` (L146-148), by seat instead of nick.
    participants: BTreeSet<SeatId>,
    /// server.rs `PeerState.alive`.
    alive: BTreeSet<SeatId>,
    status: ModeStatus,
    /// Last round started (server.rs `match_round`).
    round: u32,
    live_since_ms: Ms,
    /// Round wins per side (server.rs `wins` / `wins_by_nick`).
    scores: BTreeMap<Side, u32>,
    stats: BTreeMap<SeatId, SeatStats>,
    results: Vec<RoundResult>,
    match_result: Option<MatchResult>,
    /// victim -> attacker -> (damage this round, last hit ms).
    damage: BTreeMap<SeatId, BTreeMap<SeatId, (f32, Ms)>>,
    sd: Option<(Zone, ZoneDamage)>,
}

impl RoundCore {
    pub fn new(cfg: CoreCfg) -> Self {
        Self {
            cfg,
            teams: None,
            participants: BTreeSet::new(),
            alive: BTreeSet::new(),
            status: ModeStatus::Idle,
            round: 0,
            live_since_ms: 0,
            scores: BTreeMap::new(),
            stats: BTreeMap::new(),
            results: Vec::new(),
            match_result: None,
            damage: BTreeMap::new(),
            sd: None,
        }
    }

    // ---- accessors --------------------------------------------------------

    pub fn status(&self) -> &ModeStatus {
        &self.status
    }
    pub fn round(&self) -> u32 {
        self.round
    }
    pub fn results(&self) -> &[RoundResult] {
        &self.results
    }
    pub fn match_result(&self) -> Option<&MatchResult> {
        self.match_result.as_ref()
    }
    pub fn participants(&self) -> &BTreeSet<SeatId> {
        &self.participants
    }
    pub fn is_alive(&self, seat: SeatId) -> bool {
        self.alive.contains(&seat)
    }
    pub fn score(&self, side: Side) -> u32 {
        self.scores.get(&side).copied().unwrap_or(0)
    }
    pub fn stats(&self, seat: SeatId) -> SeatStats {
        self.stats.get(&seat).copied().unwrap_or_default()
    }
    pub fn team_of(&self, seat: SeatId) -> TeamId {
        self.teams.as_ref().and_then(|t| t.get(&seat).copied()).unwrap_or(NO_TEAM)
    }
    pub fn teams(&self) -> Option<&BTreeMap<SeatId, TeamId>> {
        self.teams.as_ref()
    }
    pub fn side_of(&self, seat: SeatId) -> Side {
        match &self.teams {
            Some(t) => Side::Team(t.get(&seat).copied().unwrap_or(NO_TEAM)),
            None => Side::Seat(seat),
        }
    }
    pub fn zone(&self) -> Option<&Zone> {
        self.sd.as_ref().map(|(z, _)| z)
    }
    pub fn live_since_ms(&self) -> Ms {
        self.live_since_ms
    }

    /// Sides with a seat alive AND present (server.rs `standing`, L1250-1257).
    pub fn standing_sides(&self, ctx: &ModeCtx) -> BTreeSet<Side> {
        self.alive.iter().filter(|s| ctx.is_present(**s)).map(|s| self.side_of(*s)).collect()
    }
    /// Sides with a participant present, alive or not
    /// (server.rs `present_participants`, L1375-1386).
    pub fn present_sides(&self, ctx: &ModeCtx) -> BTreeSet<Side> {
        self.participants.iter().filter(|s| ctx.is_present(**s)).map(|s| self.side_of(*s)).collect()
    }
    /// server.rs `multi = participants.len() >= 2` (L1285, L1442), by side.
    pub fn multi(&self) -> bool {
        let sides: BTreeSet<Side> = self.participants.iter().map(|s| self.side_of(*s)).collect();
        sides.len() >= 2
    }

    // ---- match / roster ---------------------------------------------------

    /// START (server.rs "start" verb, L938-952): participants = everyone
    /// seated, wins cleared, countdown. `teams` = Some for team modes.
    pub fn start_match(&mut self, seats: impl IntoIterator<Item = SeatId>, teams: Option<BTreeMap<SeatId, TeamId>>) {
        let cfg = self.cfg.clone();
        *self = Self::new(cfg);
        self.participants = seats.into_iter().collect();
        self.teams = teams;
        self.status = ModeStatus::Waiting { replay: false };
    }

    /// Back to the lobby (server.rs `reset_to_lobby`, L1555-1569).
    pub fn reset(&mut self) {
        let cfg = self.cfg.clone();
        *self = Self::new(cfg);
    }

    pub fn match_running(&self) -> bool {
        !matches!(self.status, ModeStatus::Idle | ModeStatus::Abandoned | ModeStatus::MatchOver(_))
    }

    /// Seat a late joiner (team modes pass its team). Takes effect for
    /// standing only once it is a fighter in `start_round`.
    pub fn add_participant(&mut self, seat: SeatId, team: Option<TeamId>) {
        self.participants.insert(seat);
        if let (Some(t), Some(map)) = (team, self.teams.as_mut()) {
            map.entry(seat).or_insert(t);
        }
    }

    /// Lobby-only team move (admin `team:` verb).
    pub fn set_team(&mut self, seat: SeatId, team: TeamId) -> Result<(), String> {
        if self.round > 0 || !matches!(self.status, ModeStatus::Waiting { .. }) {
            return Err("teams can only be changed before the first round".into());
        }
        match self.teams.as_mut() {
            Some(map) if map.contains_key(&seat) => {
                map.insert(seat, team);
                Ok(())
            }
            Some(_) => Err(format!("seat {seat} is not in this match")),
            None => Err("this mode has no teams".into()),
        }
    }

    // ---- round flow -------------------------------------------------------

    /// countdown -> live (server.rs L1662-1699): round += 1, late joiners that
    /// loaded become participants, alive = loaded participants, deaths and
    /// settle cleared, ledger round begins (reducer).
    pub fn start_round(&mut self, ctx: &mut ModeCtx, round: u32, fighters: &[SeatId]) -> bool {
        if !matches!(self.status, ModeStatus::Waiting { .. } | ModeStatus::RoundOver(_)) || round <= self.round {
            return false;
        }
        for &f in fighters {
            // Team modes seat joiners first (on_join / mode start_round).
            self.participants.insert(f);
        }
        self.alive = fighters.iter().copied().filter(|f| self.participants.contains(f)).collect();
        self.round = round;
        self.live_since_ms = ctx.now_ms;
        self.damage.clear();
        self.sd = None;
        self.status = ModeStatus::Live { round };
        true
    }

    /// server.rs `declare_death` (L1259-1278): only while combat is open, only
    /// once per seat per round; a death while live may start the settle.
    pub fn death(&mut self, ctx: &mut ModeCtx, victim: SeatId, killer: Option<SeatId>, cause: DeathCause) -> DeathOutcome {
        if !self.status.combat_open() {
            return DeathOutcome::Ignored;
        }
        if !self.alive.remove(&victim) {
            return DeathOutcome::Ignored;
        }
        self.stats.entry(victim).or_default().deaths += 1;
        let vside = self.side_of(victim);
        // Kill credit: a different side's participant (no credit for self,
        // environment or team kills).
        let killer = killer.filter(|k| *k != victim);
        if let Some(k) = killer {
            if self.participants.contains(&k) && self.side_of(k) != vside {
                self.stats.entry(k).or_default().kills += 1;
            }
        }
        let mut assists = Vec::new();
        if let (Some(rule), Some(hits)) = (self.cfg.assists, self.damage.get(&victim)) {
            for (&att, &(dmg, last)) in hits {
                if Some(att) != killer && att != victim && dmg >= rule.min_damage
                    && ctx.now_ms.saturating_sub(last) <= rule.window_ms
                    && self.side_of(att) != vside
                {
                    assists.push(att);
                }
            }
            for a in &assists {
                self.stats.entry(*a).or_default().assists += 1;
            }
        }
        ctx.emit(ModeEvent::KillFeed { victim, killer, cause, assists });
        if matches!(self.status, ModeStatus::Live { .. }) {
            self.check_round_end(ctx);
        }
        DeathOutcome::OutForRound
    }

    pub fn damage(&mut self, ctx: &ModeCtx, attacker: SeatId, victim: SeatId, amount: f32) {
        if !self.status.combat_open() || attacker == victim || amount.is_nan() || amount <= 0.0 {
            return;
        }
        let e = self.damage.entry(victim).or_default().entry(attacker).or_insert((0.0, 0));
        e.0 += amount;
        e.1 = ctx.now_ms;
    }

    /// server.rs `check_round_end` (L1280-1297): while live, start the settle
    /// once no side stands, or (2+ sides seated) at most one does. A solo
    /// test round only ends when its player dies.
    pub fn check_round_end(&mut self, ctx: &ModeCtx) {
        let ModeStatus::Live { round } = self.status else { return };
        let alive = self.standing_sides(ctx).len();
        if alive == 0 || (self.multi() && alive <= 1) {
            self.sd = None;
            self.status = ModeStatus::Settling { round, until_ms: ctx.now_ms + self.cfg.settle_ms };
        }
    }

    /// Per-tick work, in server.rs tick order (L1626-1660): supervision
    /// (`match_step`), then the settle deadline, then the pause expiry.
    /// Sudden death runs after supervision.
    pub fn tick(&mut self, ctx: &mut ModeCtx) {
        self.supervise(ctx);
        self.sudden_death_step(ctx);
        match self.status {
            ModeStatus::Settling { until_ms, .. } if ctx.now_ms >= until_ms => self.finalize(ctx),
            ModeStatus::Paused { until_ms, .. } if ctx.now_ms >= until_ms => {
                // server.rs L1654-1660: exactly one present -> forfeit to it,
                // else back to the lobby.
                let present = self.present_sides(ctx);
                if present.len() == 1 {
                    let side = *present.iter().next().unwrap();
                    self.forfeit(side);
                } else {
                    self.abandon(ctx);
                }
            }
            _ => {}
        }
    }

    /// server.rs `match_step` (L1423-1499), minus the reconnect restore
    /// (seats are stable, so scores survive a reconnect by construction) and
    /// the load barrier (reducer; see `barrier_timeout`).
    pub fn supervise(&mut self, ctx: &mut ModeCtx) {
        // L1427: active = countdown | live | roundover | paused.
        let active = matches!(self.status,
            ModeStatus::Waiting { .. } | ModeStatus::Live { .. } | ModeStatus::Settling { .. }
            | ModeStatus::RoundOver(_) | ModeStatus::Paused { .. });
        if !active {
            return;
        }
        let present = self.present_sides(ctx);
        if let ModeStatus::Paused { .. } = self.status {
            // L1445-1455: everyone gone -> lobby; 2+ back -> replay the round.
            if present.is_empty() {
                self.abandon(ctx);
            } else if present.len() >= 2 {
                self.status = ModeStatus::Waiting { replay: true };
            }
            return;
        }
        // L1457-1471: a multi-side match with < 2 sides present pauses for a
        // reconnect (settle cancelled), or returns to the lobby if empty.
        if self.multi() && present.len() < 2 {
            if present.is_empty() {
                self.abandon(ctx);
            } else {
                self.void_open_round(ctx, VoidReason::Paused);
                self.status = ModeStatus::Paused { until_ms: ctx.now_ms + self.cfg.pause_grace_ms, reason: PauseReason::OpponentLeft };
            }
            return;
        }
        // L1494-1498: 3+ player round whose others left / stopped pinging.
        if self.multi() {
            self.check_round_end(ctx);
        }
    }

    /// A seat's connection is gone. server.rs never declares a death for a
    /// leaver (L1237-1239); the seat stops counting via `present`, and a
    /// reconnect joins as not-alive (L413, L424-426), so the seat leaves the
    /// alive set now. Then supervise immediately.
    pub fn disconnect(&mut self, ctx: &mut ModeCtx, seat: SeatId) {
        self.alive.remove(&seat);
        self.supervise(ctx);
    }

    /// server.rs L1473-1491: the load barrier deadline passed. With 2+ sides
    /// seated but fewer than 2 loaded: forfeit to the one loaded side, or
    /// back to the lobby; otherwise stragglers sit the round out (the reducer
    /// leaves them out of `fighters`).
    pub fn barrier_timeout(&mut self, ctx: &mut ModeCtx, loaded: &[SeatId]) {
        if !matches!(self.status, ModeStatus::Waiting { .. } | ModeStatus::RoundOver(_)) {
            return;
        }
        let sides: BTreeSet<Side> = loaded.iter().filter(|s| self.participants.contains(s)).map(|s| self.side_of(*s)).collect();
        if self.multi() && sides.len() < 2 {
            match sides.iter().next() {
                Some(&side) => self.forfeit(side),
                None => self.abandon(ctx),
            }
        }
    }

    /// server.rs `finalize_round` (L1299-1331): exactly one side standing at
    /// the settle deadline wins the round; anything else is a draw.
    fn finalize(&mut self, ctx: &mut ModeCtx) {
        let standing = self.standing_sides(ctx);
        let outcome = if standing.len() == 1 { RoundOutcome::Won(*standing.iter().next().unwrap()) } else { RoundOutcome::Draw };
        self.record(ctx, outcome, false);
    }

    /// Fix the current round's result and decide whether the match is over
    /// (server.rs L1305-1314: `wins >= needed_wins` -> match_over; a draw
    /// never ends the match unless a round cap is set).
    fn record(&mut self, ctx: &mut ModeCtx, outcome: RoundOutcome, tiebreak: bool) {
        let result = RoundResult { round: self.round, outcome, decided_at_ms: ctx.now_ms, tiebreak };
        self.results.push(result);
        self.sd = None;
        if let RoundOutcome::Won(side) = outcome {
            *self.scores.entry(side).or_insert(0) += 1;
            if self.score(side) >= self.cfg.rule.wins {
                return self.end_match(Some(side), MatchEnd::Score);
            }
        }
        let decided = self.results.iter().filter(|r| !matches!(r.outcome, RoundOutcome::Void(_))).count() as u32;
        if self.cfg.rule.max_rounds > 0 && decided >= self.cfg.rule.max_rounds {
            let winner = self.leader();
            return self.end_match(winner, MatchEnd::RoundCap);
        }
        self.status = ModeStatus::RoundOver(result);
    }

    /// Unique leader by (round wins, kills); None on a tie.
    fn leader(&self) -> Option<Side> {
        let sides: BTreeSet<Side> = self.participants.iter().map(|s| self.side_of(*s)).collect();
        let key = |side: &Side| {
            let kills: u32 = self.participants.iter().filter(|s| self.side_of(**s) == *side).map(|s| self.stats(*s).kills as u32).sum();
            (self.score(*side), kills)
        };
        let best = sides.iter().map(key).max()?;
        let mut top = sides.iter().filter(|s| key(s) == best);
        let first = *top.next()?;
        if top.next().is_some() { None } else { Some(first) }
    }

    fn end_match(&mut self, winner: Option<Side>, end: MatchEnd) {
        let sides: BTreeSet<Side> = self.participants.iter().map(|s| self.side_of(*s)).collect();
        let scores = sides.iter().map(|s| (*s, self.score(*s))).collect();
        let r = MatchResult { winner, end, rounds: self.round, scores };
        self.match_result = Some(r.clone());
        self.status = ModeStatus::MatchOver(r);
    }

    /// server.rs `forfeit_to` (L1410-1421): the winner's score is raised to
    /// at least the win target; match over with reason "forfeit".
    pub fn forfeit(&mut self, side: Side) {
        let need = self.cfg.rule.wins;
        let e = self.scores.entry(side).or_insert(0);
        *e = (*e).max(need);
        self.end_match(Some(side), MatchEnd::Forfeit);
    }

    fn abandon(&mut self, ctx: &mut ModeCtx) {
        self.void_open_round(ctx, VoidReason::Abandoned);
        self.sd = None;
        self.status = ModeStatus::Abandoned;
    }

    /// A round that is live or settling gets a Void result (no score), so
    /// every started round has exactly one result.
    fn void_open_round(&mut self, ctx: &ModeCtx, why: VoidReason) {
        if matches!(self.status, ModeStatus::Live { .. } | ModeStatus::Settling { .. }) {
            self.results.push(RoundResult { round: self.round, outcome: RoundOutcome::Void(why), decided_at_ms: ctx.now_ms, tiebreak: false });
            self.sd = None;
        }
    }

    // ---- sudden death (not in server.rs) ---------------------------------

    fn sudden_death_step(&mut self, ctx: &mut ModeCtx) {
        let ModeStatus::Live { .. } = self.status else { return };
        let Some(sdcfg) = self.cfg.sudden_death.clone() else { return };
        if sdcfg.round_ms == 0 {
            return;
        }
        if self.sd.is_none() {
            if ctx.now_ms < self.live_since_ms + sdcfg.round_ms {
                return;
            }
            let c = self.cfg.map_center.map(|c| [c[0], c[1]]).unwrap_or_else(|| self.standing_centroid(ctx));
            let start = self.live_since_ms + sdcfg.round_ms;
            match Zone::fixed(sdcfg.table.clone(), start, c) {
                Ok(z) => {
                    self.sd = Some((z, ZoneDamage::new()));
                    ctx.emit(ModeEvent::Banner { text: "SUDDEN DEATH — stay inside the ring".into() });
                }
                Err(_) => return,
            }
        }
        let seats: Vec<(SeatId, [f32; 3])> = self.alive.iter()
            .filter_map(|s| ctx.view(*s).filter(|v| v.present).and_then(|v| v.pos).map(|p| (*s, p)))
            .collect();
        let finished = {
            let (zone, dmg) = self.sd.as_mut().unwrap();
            for (seat, amount) in dmg.step(zone, ctx.now_ms, &seats) {
                ctx.emit(ModeEvent::ZoneDamage { seat, amount });
            }
            zone.finished(ctx.now_ms)
        };
        if finished {
            // Tiebreak: most standing seats, then the
            // highest summed ledger health; no unique leader => draw.
            let standing = self.standing_sides(ctx);
            let key = |side: &Side| -> (usize, i64) {
                let seats: Vec<SeatId> = self.alive.iter().copied().filter(|s| ctx.is_present(*s) && self.side_of(*s) == *side).collect();
                let hp: f32 = seats.iter().map(|s| ctx.view(*s).map(|v| v.ledger_health).unwrap_or(0.0)).sum();
                (seats.len(), (hp * 100.0).round() as i64)
            };
            let best = standing.iter().map(key).max();
            let top: Vec<Side> = standing.iter().copied().filter(|s| Some(key(s)) == best).collect();
            let outcome = if top.len() == 1 { RoundOutcome::Won(top[0]) } else { RoundOutcome::Draw };
            self.record(ctx, outcome, true);
        }
    }

    fn standing_centroid(&self, ctx: &ModeCtx) -> [f32; 2] {
        let pts: Vec<[f32; 3]> = self.alive.iter().filter_map(|s| ctx.view(*s).and_then(|v| v.pos)).collect();
        if pts.is_empty() {
            return [0.0, 0.0];
        }
        let n = pts.len() as f32;
        [pts.iter().map(|p| p[0]).sum::<f32>() / n, pts.iter().map(|p| p[1]).sum::<f32>() / n]
    }

    // ---- HUD ---------------------------------------------------------------

    pub fn hud(&self, mode: &str, now_ms: Ms) -> ModeState {
        let (state, deadline_ms) = match &self.status {
            ModeStatus::Idle | ModeStatus::Abandoned => (RoundStateTag::Idle, 0),
            ModeStatus::Waiting { .. } => (RoundStateTag::Waiting, 0),
            ModeStatus::Live { .. } => {
                let d = self.cfg.sudden_death.as_ref().filter(|s| s.round_ms > 0)
                    .map(|s| match self.zone() { Some(z) => z.end_ms(), None => self.live_since_ms + s.round_ms })
                    .unwrap_or(0);
                (RoundStateTag::Live, d)
            }
            ModeStatus::Settling { until_ms, .. } => (RoundStateTag::Settling, *until_ms),
            ModeStatus::RoundOver(_) => (RoundStateTag::RoundOver, 0),
            ModeStatus::MatchOver(_) => (RoundStateTag::MatchOver, 0),
            ModeStatus::Paused { until_ms, .. } => (RoundStateTag::Paused, *until_ms),
        };
        let sides: BTreeSet<Side> = self.participants.iter().map(|s| self.side_of(*s)).collect();
        let sides = sides.into_iter().map(|side| SideRec {
            side,
            score: self.score(side),
            alive: self.alive.iter().filter(|s| self.side_of(**s) == side).count().min(255) as u8,
        }).collect();
        let seats = self.participants.iter().map(|&seat| {
            let st = self.stats(seat);
            SeatRec { seat, team: self.team_of(seat), kills: st.kills, deaths: st.deaths, assists: st.assists, alive: self.alive.contains(&seat) }
        }).collect();
        let objective = match self.zone() {
            Some(z) => {
                let s = z.sample(now_ms);
                Objective::Zone(ZoneHud {
                    phase: s.phase as u8, center: s.center, radius_cm: s.radius_cm, next_center: s.next_center,
                    next_radius_cm: s.next_radius_cm, shrinking: s.shrinking, segment_end_ms: s.segment_end_ms, dps: s.dps,
                })
            }
            None => Objective::None,
        };
        ModeState {
            mode: mode.to_string(),
            round: self.round,
            state,
            deadline_ms,
            target_wins: self.cfg.rule.wins,
            max_rounds: self.cfg.rule.max_rounds,
            sides,
            seats,
            last_result: self.results.iter().rev().find(|r| !matches!(r.outcome, RoundOutcome::Void(_))).copied(),
            objective,
        }
    }
}
