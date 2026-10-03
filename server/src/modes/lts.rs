//! Last Team Standing, round-based.
//!
//! - 2..=4 teams (`teams`, default 2) of 1..=8. The last team with anyone
//!   standing wins the round; best of `best_of` (default 5). The engine is
//!   the Duel one grouped by team: settle, draw on a mutual wipe, pause when
//!   fewer than 2 teams are present, forfeit to the team that stayed.
//! - Teams: balanced at START (`teams::balance`); a joiner goes to the
//!   smallest team (ties: lower score, lower id) and fights from the next
//!   round; a reconnect keeps its seat and therefore its team. Admin moves
//!   only before the first round.
//! - Sides swap every round (`SpawnPolicy::TeamSides`).
//! - Friendly fire `ff` = 0 (deny, default) | 0..1 (scale) | 1 (allow).
//! - Uneven teams: the short team gets `budget_per_missing` (10) CUSTOM
//!   budget per missing member, or one class upgrade per missing member
//!   (max 3) under CLASSES ONLY. Team cloth tint is forced.
//! - Round limit `round_s` (300) then the sudden-death ring, then the
//!   tiebreak: most standing, then summed ledger health.
//! - Round cap `max_rounds` (default 2 * best_of, 0 = none) so draws cannot
//!   loop forever; at the cap the leader by wins then kills takes it.
//! - Downed-but-alive stays alive: only a declared death counts.

use super::round::{needed_wins, CoreCfg, MatchRule, RoundCore, SuddenDeath};
use super::teams;
use super::traits::*;
use super::zone::ZoneTable;
use std::collections::BTreeMap;

pub const INFO: ModeInfo = ModeInfo {
    id: "lts",
    label: "Last Team Standing",
    min_players: 2,
    max_players: 32,
    teams: TeamLayout::Teams { min: 2, max: 4 },
    round_based: true,
    needs_midround_respawn: false,
    needs_loot: false,
    needs_ai: false,
    map_tags: &["medium", "teams"],
};

pub const OPTS: &[&str] = &["teams", "best_of", "round_s", "ff", "budget_per_missing", "max_rounds"];

pub struct Lts {
    core: RoundCore,
    n_teams: u8,
    ff: f32,
    budget_per_missing: u16,
    /// Joiners seated on a team, waiting for their first round.
    pending: BTreeMap<SeatId, TeamId>,
}

impl Default for Lts {
    fn default() -> Self {
        Self::new()
    }
}

impl Lts {
    pub fn new() -> Self {
        Self {
            core: RoundCore::new(CoreCfg::new(MatchRule { wins: needed_wins(5), max_rounds: 10 })),
            n_teams: 2,
            ff: 0.0,
            budget_per_missing: 10,
            pending: BTreeMap::new(),
        }
    }
    pub fn core(&self) -> &RoundCore {
        &self.core
    }
    pub fn n_teams(&self) -> u8 {
        self.n_teams
    }

    fn team_or_pending(&self, seat: SeatId) -> TeamId {
        match self.core.team_of(seat) {
            NO_TEAM => self.pending.get(&seat).copied().unwrap_or(NO_TEAM),
            t => t,
        }
    }

    /// Members + score per team, for the join pick.
    fn join_table(&self) -> BTreeMap<TeamId, (u32, u32)> {
        let mut m: BTreeMap<TeamId, (u32, u32)> = (1..=self.n_teams).map(|t| (t, (0, self.core.score(Side::Team(t))))).collect();
        let seated = self.core.teams().into_iter().flatten().filter(|(s, _)| self.core.participants().contains(*s));
        for (_, t) in seated.chain(self.pending.iter()) {
            if let Some(e) = m.get_mut(t) {
                e.0 += 1;
            }
        }
        m
    }

    /// Present members per team (participants and pending joiners).
    fn present_sizes(&self, ctx: &ModeCtx) -> BTreeMap<TeamId, u32> {
        let mut m: BTreeMap<TeamId, u32> = (1..=self.n_teams).map(|t| (t, 0)).collect();
        for v in ctx.roster.iter().filter(|v| v.present) {
            let t = self.team_or_pending(v.seat);
            if let Some(c) = m.get_mut(&t) {
                *c += 1;
            }
        }
        m
    }
}

impl GameMode for Lts {
    fn info(&self) -> &ModeInfo {
        &INFO
    }

    fn on_config(&mut self, cfg: &ModeConfig, roster: &[SeatInfo]) -> Result<(), String> {
        cfg.check_known(OPTS)?;
        let n_teams = cfg.get::<u8>("teams", 2)?;
        if !(2..=4).contains(&n_teams) {
            return Err(format!("lts: teams must be 2..=4, got {n_teams}"));
        }
        let best_of = cfg.get::<u8>("best_of", 5)?.clamp(1, 15);
        let round_s = cfg.get::<u32>("round_s", 300)?;
        let ff = cfg.get::<f32>("ff", 0.0)?;
        if !(0.0..=1.0).contains(&ff) {
            return Err(format!("lts: ff must be 0..=1, got {ff}"));
        }
        let per_missing = cfg.get::<u16>("budget_per_missing", 10)?;
        let max_rounds = cfg.get::<u32>("max_rounds", 2 * best_of as u32)?.min(99);
        if roster.len() < n_teams as usize || roster.len() > INFO.max_players as usize {
            return Err(format!("lts: {} teams need {}..={} players, got {}", n_teams, n_teams, INFO.max_players, roster.len()));
        }
        let mut c = CoreCfg::new(MatchRule { wins: needed_wins(best_of), max_rounds });
        c.map_center = cfg.map_center;
        if round_s > 0 {
            c.sudden_death = Some(SuddenDeath { round_ms: round_s as Ms * 1000, table: ZoneTable::sudden_death() });
        }
        self.n_teams = n_teams;
        self.ff = ff;
        self.budget_per_missing = per_missing;
        self.pending.clear();
        self.core = RoundCore::new(c);
        let assign = teams::balance(roster, n_teams);
        self.core.start_match(roster.iter().map(|s| s.seat), Some(assign));
        Ok(())
    }

    fn on_round_start(&mut self, ctx: &mut ModeCtx, round: u32, fighters: &[SeatId]) -> bool {
        if !matches!(self.core.status(), ModeStatus::Waiting { .. } | ModeStatus::RoundOver(_)) || round <= self.core.round() {
            return false;
        }
        // Seat joiners on their team before the engine counts them.
        for &f in fighters {
            if self.core.participants().contains(&f) {
                continue;
            }
            let team = match self.pending.remove(&f) {
                Some(t) => t,
                None => teams::pick_team(&self.join_table(), self.n_teams),
            };
            self.core.add_participant(f, Some(team));
        }
        self.core.start_round(ctx, round, fighters)
    }
    fn on_tick(&mut self, ctx: &mut ModeCtx) {
        self.core.tick(ctx)
    }
    fn on_death(&mut self, ctx: &mut ModeCtx, victim: SeatId, killer: Option<SeatId>, cause: DeathCause) -> DeathOutcome {
        self.core.death(ctx, victim, killer, cause)
    }
    fn on_damage(&mut self, ctx: &mut ModeCtx, attacker: SeatId, victim: SeatId, amount: f32) {
        self.core.damage(ctx, attacker, victim, amount)
    }
    fn on_disconnect(&mut self, ctx: &mut ModeCtx, seat: SeatId) {
        self.pending.remove(&seat);
        self.core.disconnect(ctx, seat)
    }
    fn on_join(&mut self, _ctx: &mut ModeCtx, seat: &SeatInfo) -> JoinRole {
        if self.core.participants().contains(&seat.seat) || self.pending.contains_key(&seat.seat) {
            return JoinRole::NextRound; // reconnect: same seat, same team
        }
        if self.core.participants().len() + self.pending.len() >= INFO.max_players as usize {
            return JoinRole::Spectate;
        }
        let t = teams::pick_team(&self.join_table(), self.n_teams);
        self.pending.insert(seat.seat, t);
        JoinRole::NextRound
    }
    fn on_barrier_timeout(&mut self, ctx: &mut ModeCtx, loaded: &[SeatId]) {
        self.core.barrier_timeout(ctx, loaded)
    }
    fn round_result(&self) -> Option<&RoundResult> {
        self.core.results().last()
    }
    fn round_results(&self) -> &[RoundResult] {
        self.core.results()
    }
    fn match_result(&self) -> Option<&MatchResult> {
        self.core.match_result()
    }
    fn status(&self) -> &ModeStatus {
        self.core.status()
    }
    fn reset(&mut self) {
        self.pending.clear();
        self.core.reset()
    }

    fn spawn_policy(&self, _ctx: &ModeCtx, round: u32) -> SpawnPolicy {
        SpawnPolicy::TeamSides { sides: (1..=self.n_teams).map(|t| (t, teams::spawn_side(t, round, self.n_teams))).collect() }
    }

    fn kit_override(&self, ctx: &ModeCtx, seat: SeatId) -> KitOverride {
        let team = self.team_or_pending(seat);
        let sizes = self.present_sizes(ctx);
        KitOverride {
            policy: KitPolicy::PlayerChoice,
            budget_bonus: teams::uneven_budget(&sizes, self.budget_per_missing).get(&team).copied().unwrap_or(0),
            class_upgrade: teams::uneven_class_steps(&sizes, team),
            tint: teams::team_tint(team),
        }
    }

    fn friendly_fire(&self, attacker: SeatId, victim: SeatId) -> DamageVerdict {
        let (ta, tv) = (self.core.team_of(attacker), self.core.team_of(victim));
        if attacker == victim || ta == NO_TEAM || ta != tv {
            return DamageVerdict::Allow;
        }
        if self.ff <= 0.0 {
            DamageVerdict::Deny("friendly_fire")
        } else if self.ff >= 1.0 {
            DamageVerdict::Allow
        } else {
            DamageVerdict::Scale(self.ff)
        }
    }

    fn team_of(&self, seat: SeatId) -> TeamId {
        self.team_or_pending(seat)
    }

    fn admin_set_team(&mut self, seat: SeatId, team: TeamId) -> Result<(), String> {
        if !(1..=self.n_teams).contains(&team) {
            return Err(format!("team must be 1..={}", self.n_teams));
        }
        self.core.set_team(seat, team)
    }

    fn hud_section(&self, now_ms: Ms) -> ModeState {
        let mut s = self.core.hud(INFO.id, now_ms);
        // Every team gets a bar, even an empty one.
        for t in 1..=self.n_teams {
            if !s.sides.iter().any(|r| r.side == Side::Team(t)) {
                s.sides.push(SideRec { side: Side::Team(t), score: self.core.score(Side::Team(t)), alive: 0 });
            }
        }
        s.sides.sort_by_key(|r| r.side);
        s
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn seats(n: u32) -> Vec<SeatInfo> {
        (1..=n).map(|i| SeatInfo { seat: i, key: format!("p{i}"), rating: 0 }).collect()
    }
    fn roster(n: u32) -> Vec<SeatView> {
        (1..=n).map(|s| SeatView { seat: s, present: true, pos: None, ledger_health: 100.0 }).collect()
    }
    fn mates(m: &Lts, n: u32) -> (Vec<SeatId>, Vec<SeatId>) {
        let a: Vec<SeatId> = (1..=n).filter(|s| m.team_of(*s) == 1).collect();
        let b: Vec<SeatId> = (1..=n).filter(|s| m.team_of(*s) == 2).collect();
        (a, b)
    }

    #[test]
    fn team_wipe_wins_round_and_survivor_count_irrelevant() {
        let mut m = Lts::new();
        m.on_config(&ModeConfig::default().with("best_of", "3"), &seats(4)).unwrap();
        let r = roster(4);
        let (a, b) = mates(&m, 4);
        assert_eq!((a.len(), b.len()), (2, 2));
        let mut out = vec![];
        assert!(m.on_round_start(&mut ModeCtx::new(0, &r, &mut out), 1, &[1, 2, 3, 4]));
        m.on_death(&mut ModeCtx::new(100, &r, &mut out), b[0], Some(a[0]), DEATH_DAMAGE);
        m.on_death(&mut ModeCtx::new(200, &r, &mut out), a[1], Some(b[1]), DEATH_DAMAGE);
        assert!(matches!(m.status(), ModeStatus::Live { .. }), "both teams standing");
        m.on_death(&mut ModeCtx::new(300, &r, &mut out), b[1], Some(a[0]), DEATH_DAMAGE);
        m.on_tick(&mut ModeCtx::new(700, &r, &mut out));
        assert_eq!(m.round_result().unwrap().outcome, RoundOutcome::Won(Side::Team(1)));
    }

    #[test]
    fn friendly_fire_verdicts() {
        let mut m = Lts::new();
        m.on_config(&ModeConfig::default(), &seats(4)).unwrap();
        let (a, b) = mates(&m, 4);
        assert_eq!(m.friendly_fire(a[0], a[1]), DamageVerdict::Deny("friendly_fire"));
        assert_eq!(m.friendly_fire(a[0], b[0]), DamageVerdict::Allow);
        assert_eq!(m.friendly_fire(a[0], a[0]), DamageVerdict::Allow);
        m.on_config(&ModeConfig::default().with("ff", "0.5"), &seats(4)).unwrap();
        let (a, _) = mates(&m, 4);
        assert_eq!(m.friendly_fire(a[0], a[1]), DamageVerdict::Scale(0.5));
        assert!(m.on_config(&ModeConfig::default().with("ff", "2"), &seats(4)).is_err());
    }

    #[test]
    fn team_kill_gives_no_credit() {
        let mut m = Lts::new();
        m.on_config(&ModeConfig::default().with("ff", "1"), &seats(4)).unwrap();
        let r = roster(4);
        let (a, _) = mates(&m, 4);
        let mut out = vec![];
        m.on_round_start(&mut ModeCtx::new(0, &r, &mut out), 1, &[1, 2, 3, 4]);
        m.on_death(&mut ModeCtx::new(1, &r, &mut out), a[1], Some(a[0]), DEATH_DAMAGE);
        assert_eq!(m.core().stats(a[0]).kills, 0);
    }

    #[test]
    fn uneven_teams_get_budget_and_tint() {
        let mut m = Lts::new();
        m.on_config(&ModeConfig::default(), &seats(5)).unwrap();
        let r = roster(5);
        let (a, b) = mates(&m, 5);
        let (big, small) = if a.len() > b.len() { (a, b) } else { (b, a) };
        let out = &mut vec![];
        let ctx = ModeCtx::new(0, &r, out);
        let k_small = m.kit_override(&ctx, small[0]);
        let k_big = m.kit_override(&ctx, big[0]);
        assert_eq!((k_small.budget_bonus, k_small.class_upgrade), (10, 1));
        assert_eq!((k_big.budget_bonus, k_big.class_upgrade), (0, 0));
        assert_eq!(k_small.tint, teams::team_tint(m.team_of(small[0])));
        assert_ne!(k_small.tint, k_big.tint);
    }

    #[test]
    fn joiner_goes_to_smaller_team_and_fights_next_round() {
        let mut m = Lts::new();
        m.on_config(&ModeConfig::default(), &seats(3)).unwrap();
        let (a, b) = mates(&m, 3);
        let small = if a.len() < b.len() { 1 } else { 2 };
        let mut r = roster(3);
        let mut out = vec![];
        assert!(m.on_round_start(&mut ModeCtx::new(0, &r, &mut out), 1, &[1, 2, 3]));
        r.push(SeatView { seat: 9, present: true, pos: None, ledger_health: 100.0 });
        let role = m.on_join(&mut ModeCtx::new(10, &r, &mut out), &SeatInfo { seat: 9, key: "z".into(), rating: 0 });
        assert_eq!(role, JoinRole::NextRound);
        assert_eq!(m.team_of(9), small);
        assert!(!m.core().is_alive(9));
        // Finish round 1 (wipe the small team), then 9 fights round 2.
        let small_members: Vec<SeatId> = (1..=3).filter(|s| m.team_of(*s) == small).collect();
        for (i, s) in small_members.iter().enumerate() {
            m.on_death(&mut ModeCtx::new(100 + i as Ms, &r, &mut out), *s, None, DEATH_DAMAGE);
        }
        m.on_tick(&mut ModeCtx::new(1_000, &r, &mut out));
        assert!(matches!(m.status(), ModeStatus::RoundOver(_)));
        assert!(m.on_round_start(&mut ModeCtx::new(2_000, &r, &mut out), 2, &[1, 2, 3, 9]));
        assert!(m.core().is_alive(9));
        assert_eq!(m.team_of(9), small);
    }

    #[test]
    fn whole_team_dropping_pauses_then_forfeits() {
        let mut m = Lts::new();
        m.on_config(&ModeConfig::default(), &seats(4)).unwrap();
        let (a, b) = mates(&m, 4);
        let mut r = roster(4);
        let mut out = vec![];
        m.on_round_start(&mut ModeCtx::new(0, &r, &mut out), 1, &[1, 2, 3, 4]);
        r.retain(|v| v.seat != b[0]);
        m.on_disconnect(&mut ModeCtx::new(10, &r, &mut out), b[0]);
        assert!(matches!(m.status(), ModeStatus::Live { .. }), "team 2 still present");
        r.retain(|v| v.seat != b[1]);
        m.on_disconnect(&mut ModeCtx::new(20, &r, &mut out), b[1]);
        assert!(matches!(m.status(), ModeStatus::Paused { .. }));
        m.on_tick(&mut ModeCtx::new(30_020, &r, &mut out));
        let res = m.match_result().unwrap();
        assert_eq!((res.winner, res.end), (Some(Side::Team(m.team_of(a[0]))), MatchEnd::Forfeit));
    }

    #[test]
    fn admin_team_move_only_before_first_round() {
        let mut m = Lts::new();
        m.on_config(&ModeConfig::default(), &seats(4)).unwrap();
        let t = m.team_of(1);
        let other = 3 - t;
        m.admin_set_team(1, other).unwrap();
        assert_eq!(m.team_of(1), other);
        assert!(m.admin_set_team(1, 7).is_err());
        let r = roster(4);
        let mut out = vec![];
        m.on_round_start(&mut ModeCtx::new(0, &r, &mut out), 1, &[1, 2, 3, 4]);
        assert!(m.admin_set_team(1, t).is_err());
    }

    #[test]
    fn sides_rotate_and_hud_lists_every_team() {
        let mut m = Lts::new();
        m.on_config(&ModeConfig::default().with("teams", "3"), &seats(3)).unwrap();
        let r = roster(3);
        let out = &mut vec![];
        let ctx = ModeCtx::new(0, &r, out);
        assert_eq!(m.spawn_policy(&ctx, 1), SpawnPolicy::TeamSides { sides: vec![(1, 0), (2, 1), (3, 2)] });
        assert_eq!(m.spawn_policy(&ctx, 2), SpawnPolicy::TeamSides { sides: vec![(1, 1), (2, 2), (3, 0)] });
        let h = m.hud_section(0);
        assert_eq!(h.sides.len(), 3);
        assert_eq!(h.mode, "lts");
    }
}
