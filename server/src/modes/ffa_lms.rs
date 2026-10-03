//! FFA — Last Man Standing, round-based.
//!
//! - 3..=8 players. Last alive wins the round (the Duel engine: settle,
//!   draw on mutual kill, pause/forfeit, all identical).
//! - First to `wins` (default 3) takes the match; after `rounds` decided
//!   rounds (default 5, 0 = no cap) the most wins takes it, kills break ties,
//!   a remaining tie is a match draw.
//! - Third-party protection: kill credit is the ledger's last attacker (the
//!   reducer's `killer`); +1 assist to every other seat that did at least
//!   `assist_min_dmg` (20) within `assist_window_s` (10 s).
//! - Spawn spread: `FarthestFromEnemies { spread_m = 6 }`.
//! - Optional `round_s` sudden-death ring (off by default).

use super::round::{AssistRule, CoreCfg, MatchRule, RoundCore, SuddenDeath};
use super::traits::*;
use super::zone::ZoneTable;

pub const INFO: ModeInfo = ModeInfo {
    id: "ffa",
    label: "Free-for-all: Last Man Standing",
    min_players: 3,
    max_players: 8,
    teams: TeamLayout::None,
    round_based: true,
    needs_midround_respawn: false,
    needs_loot: false,
    needs_ai: false,
    map_tags: &["medium"],
};

pub const OPTS: &[&str] = &["wins", "rounds", "round_s", "assist_window_s", "assist_min_dmg", "spread_m"];

pub struct FfaLms {
    core: RoundCore,
    spread_m: f32,
}

impl Default for FfaLms {
    fn default() -> Self {
        Self::new()
    }
}

impl FfaLms {
    pub fn new() -> Self {
        Self { core: RoundCore::new(CoreCfg::new(MatchRule { wins: 3, max_rounds: 5 })), spread_m: 6.0 }
    }
    pub fn core(&self) -> &RoundCore {
        &self.core
    }
}

impl GameMode for FfaLms {
    fn info(&self) -> &ModeInfo {
        &INFO
    }

    fn on_config(&mut self, cfg: &ModeConfig, roster: &[SeatInfo]) -> Result<(), String> {
        cfg.check_known(OPTS)?;
        let wins = cfg.get::<u32>("wins", 3)?.clamp(1, 50);
        let rounds = cfg.get::<u32>("rounds", 5)?.min(99);
        let round_s = cfg.get::<u32>("round_s", 0)?;
        let window_s = cfg.get::<u32>("assist_window_s", 10)?.min(60);
        let min_dmg = cfg.get::<f32>("assist_min_dmg", 20.0)?;
        let spread_m = cfg.get::<f32>("spread_m", 6.0)?;
        if min_dmg.is_nan() || min_dmg < 0.0 || spread_m.is_nan() || spread_m < 0.0 {
            return Err("assist_min_dmg and spread_m must be >= 0".into());
        }
        let n = roster.len();
        if n < INFO.min_players as usize || n > INFO.max_players as usize {
            return Err(format!("ffa needs {}..={} players, got {n}", INFO.min_players, INFO.max_players));
        }
        let mut c = CoreCfg::new(MatchRule { wins, max_rounds: rounds });
        c.map_center = cfg.map_center;
        c.assists = Some(AssistRule { window_ms: window_s as Ms * 1000, min_damage: min_dmg });
        if round_s > 0 {
            c.sudden_death = Some(SuddenDeath { round_ms: round_s as Ms * 1000, table: ZoneTable::sudden_death() });
        }
        self.spread_m = spread_m;
        self.core = RoundCore::new(c);
        self.core.start_match(roster.iter().map(|s| s.seat), None);
        Ok(())
    }

    fn on_round_start(&mut self, ctx: &mut ModeCtx, round: u32, fighters: &[SeatId]) -> bool {
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
        self.core.disconnect(ctx, seat)
    }
    fn on_join(&mut self, _ctx: &mut ModeCtx, _seat: &SeatInfo) -> JoinRole {
        if self.core.participants().len() >= INFO.max_players as usize { JoinRole::Spectate } else { JoinRole::NextRound }
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
        self.core.reset()
    }

    fn spawn_policy(&self, _ctx: &ModeCtx, _round: u32) -> SpawnPolicy {
        SpawnPolicy::FarthestFromEnemies { min_cm: self.spread_m * 100.0 }
    }
    fn kit_override(&self, _ctx: &ModeCtx, _seat: SeatId) -> KitOverride {
        KitOverride::default()
    }
    fn friendly_fire(&self, _attacker: SeatId, _victim: SeatId) -> DamageVerdict {
        DamageVerdict::Allow
    }

    fn hud_section(&self, now_ms: Ms) -> ModeState {
        self.core.hud(INFO.id, now_ms)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn seats(n: u32) -> Vec<SeatInfo> {
        (1..=n).map(|i| SeatInfo { seat: i, key: format!("p{i}"), rating: 0 }).collect()
    }

    fn run_round(m: &mut FfaLms, roster: &[SeatView], out: &mut Vec<ModeEvent>, now: &mut Ms, round: u32, deaths: &[(SeatId, SeatId)]) {
        let fighters: Vec<SeatId> = roster.iter().map(|v| v.seat).collect();
        assert!(m.on_round_start(&mut ModeCtx::new(*now, roster, out), round, &fighters));
        for &(v, k) in deaths {
            *now += 1_000;
            m.on_death(&mut ModeCtx::new(*now, roster, out), v, Some(k), DEATH_DAMAGE);
        }
        *now += 500;
        m.on_tick(&mut ModeCtx::new(*now, roster, out));
    }

    #[test]
    fn rejects_bad_config() {
        let mut m = FfaLms::new();
        assert!(m.on_config(&ModeConfig::default(), &seats(2)).is_err());
        assert!(m.on_config(&ModeConfig::default().with("bogus", "1"), &seats(4)).is_err());
        assert!(m.on_config(&ModeConfig::default().with("wins", "x"), &seats(4)).is_err());
        assert!(m.on_config(&ModeConfig::default(), &seats(4)).is_ok());
    }

    #[test]
    fn last_standing_wins_round_and_first_to_wins_takes_match() {
        let mut m = FfaLms::new();
        m.on_config(&ModeConfig::default().with("wins", "2"), &seats(4)).unwrap();
        let roster: Vec<SeatView> = (1..=4).map(|s| SeatView { seat: s, present: true, pos: None, ledger_health: 100.0 }).collect();
        let (mut out, mut now) = (vec![], 0);
        run_round(&mut m, &roster, &mut out, &mut now, 1, &[(2, 1), (3, 4)]);
        assert!(matches!(m.status(), ModeStatus::Live { .. }), "1 and 4 standing");
        now += 1_000;
        m.on_death(&mut ModeCtx::new(now, &roster, &mut out), 4, Some(1), DEATH_DAMAGE);
        now += 400;
        m.on_tick(&mut ModeCtx::new(now, &roster, &mut out));
        assert_eq!(m.round_result().unwrap().outcome, RoundOutcome::Won(Side::Seat(1)));
        assert_eq!(m.core().stats(1).kills, 2);
        run_round(&mut m, &roster, &mut out, &mut now, 2, &[(2, 1), (3, 1), (4, 1)]);
        assert_eq!(m.match_result().unwrap().winner, Some(Side::Seat(1)));
    }

    #[test]
    fn round_cap_most_wins_then_kills() {
        let mut m = FfaLms::new();
        m.on_config(&ModeConfig::default().with("wins", "5").with("rounds", "2"), &seats(3)).unwrap();
        let roster: Vec<SeatView> = (1..=3).map(|s| SeatView { seat: s, present: true, pos: None, ledger_health: 100.0 }).collect();
        let (mut out, mut now) = (vec![], 0);
        // Round 1: 1 wins with 2 kills. Round 2: 2 wins with 1 kill (3 kills 1 first).
        run_round(&mut m, &roster, &mut out, &mut now, 1, &[(2, 1), (3, 1)]);
        run_round(&mut m, &roster, &mut out, &mut now, 2, &[(1, 3), (3, 2)]);
        let r = m.match_result().unwrap();
        assert_eq!(r.end, MatchEnd::RoundCap);
        assert_eq!(r.winner, Some(Side::Seat(1)), "1-1 on wins; seat 1 has 2 kills vs 1");
    }

    #[test]
    fn assists_for_third_parties() {
        let mut m = FfaLms::new();
        m.on_config(&ModeConfig::default(), &seats(3)).unwrap();
        let roster: Vec<SeatView> = (1..=3).map(|s| SeatView { seat: s, present: true, pos: None, ledger_health: 100.0 }).collect();
        let mut out = vec![];
        assert!(m.on_round_start(&mut ModeCtx::new(0, &roster, &mut out), 1, &[1, 2, 3]));
        m.on_damage(&mut ModeCtx::new(1_000, &roster, &mut out), 3, 2, 25.0);
        m.on_damage(&mut ModeCtx::new(2_000, &roster, &mut out), 1, 2, 80.0);
        m.on_death(&mut ModeCtx::new(2_100, &roster, &mut out), 2, Some(1), DEATH_DAMAGE);
        assert_eq!((m.core().stats(1).kills, m.core().stats(3).assists), (1, 1));
        assert!(out.iter().any(|e| matches!(e, ModeEvent::KillFeed { victim: 2, killer: Some(1), assists, .. } if assists == &vec![3])));
        // Outside the window: no assist.
        m.on_damage(&mut ModeCtx::new(3_000, &roster, &mut out), 3, 1, 50.0);
        m.on_death(&mut ModeCtx::new(13_001, &roster, &mut out), 1, Some(2), DEATH_DAMAGE);
        assert_eq!(m.core().stats(3).assists, 1);
    }
}
