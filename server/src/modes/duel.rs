//! Duel: the original server.rs match loop, re-homed with identical
//! behaviour. Every rule below is lifted from `server/src/server.rs`
//! at commit c180540; line numbers refer to that file.
//!
//! | Rule | server.rs | Here |
//! |---|---|---|
//! | `best_of` default 3, clamped 1..=15, frozen outside the lobby | L213, L1180-1188 | `on_config` (config is frozen by construction: only START applies it) |
//! | Win target `needed_wins = (best_of + 1) / 2` | L1358 | `round::needed_wins` |
//! | START seats everyone connected; wins cleared | L938-952 | `on_config` -> `RoundCore::start_match` |
//! | Live: alive = loaded participants; loaded late joiners become participants | L1662-1699 | `on_round_start(fighters)` |
//! | Death only while combat is open (live, or settling); once per round | L1245-1278 | `RoundCore::death` |
//! | Round ends when 0 stand, or (2+ participants) <= 1 stands; solo round ends only at death | L1280-1297 | `RoundCore::check_round_end` |
//! | 400 ms trade settle (match_core `SETTLE_MS`); combat stays open | L1242-1243, L1246-1248 | `SETTLE_MS`, `ModeStatus::Settling` |
//! | Settle deadline: exactly 1 standing wins (+1), else draw (mutual kill) | L1299-1331 | `RoundCore::finalize` |
//! | A draw scores nobody and never ends the match | L1322-1329 | `RoundCore::record` (`max_rounds = 0`) |
//! | Wins >= target => match over | L1313, L1397-1408 | `RoundCore::record` |
//! | Standing = alive AND present (connected, game pinging while live) | L1250-1257, L1373-1386 | `RoundCore::standing_sides` + `SeatView.present` |
//! | 2+ participants, < 2 present => pause 30 s (settle cancelled); 0 present => lobby | L1457-1471, L196-197 | `RoundCore::supervise` |
//! | Paused: 0 present => lobby; 2+ present => replay the round (countdown) | L1445-1455 | `supervise` -> `Waiting { replay: true }` |
//! | Pause expiry: 1 present => forfeit to it, else lobby | L1651-1660 | `RoundCore::tick` |
//! | Forfeit raises the winner to the win target; reason "forfeit" | L1410-1421 | `RoundCore::forfeit` |
//! | Barrier timeout with < 2 loaded (2+ seated) => forfeit / lobby; else stragglers sit out | L1473-1491 | `RoundCore::barrier_timeout` |
//! | 3+ players: a round whose others left ends without a pause | L1494-1498 | `supervise` -> `check_round_end` |
//! | A leaver is not declared dead; a reconnect spectates until next live | L1237-1239, L413, L424-426 | `RoundCore::disconnect` |
//! | Reconnect keeps wins (by nick) | L1430-1440 | by construction: scores are keyed by stable seat |
//! | Match winner at match_over -> lobby = max wins | L1703-1720 | `MatchResult.winner` |
//!
//! The reducer keeps (not mode meaning): countdown 3 s, round-over 4 s,
//! match-over 5 s (L1348-1356), the load barrier itself (L1473-1491),
//! `S2CDeath` resend, the state seq and history.jsonl.
//!
//! "Duel" accepts 1..=8 players (solo test rounds and 3+
//! last-man-standing). The optional round limit + sudden-death ring is
//! available via `round_s` but OFF by default, so the default is identical.

use super::round::{needed_wins, CoreCfg, MatchRule, RoundCore, SuddenDeath};
use super::traits::*;
use super::zone::ZoneTable;

pub const INFO: ModeInfo = ModeInfo {
    id: "duel",
    label: "Duel",
    min_players: 1,
    max_players: 8,
    teams: TeamLayout::None,
    round_based: true,
    needs_midround_respawn: false,
    needs_loot: false,
    needs_ai: false,
    map_tags: &["small"],
};

pub const OPTS: &[&str] = &["best_of", "round_s"];

pub struct Duel {
    core: RoundCore,
    best_of: u8,
}

impl Default for Duel {
    fn default() -> Self {
        Self::new()
    }
}

impl Duel {
    pub fn new() -> Self {
        Self { core: RoundCore::new(CoreCfg::new(MatchRule { wins: needed_wins(3), max_rounds: 0 })), best_of: 3 }
    }
    pub fn best_of(&self) -> u8 {
        self.best_of
    }
    pub fn core(&self) -> &RoundCore {
        &self.core
    }
}

impl GameMode for Duel {
    fn info(&self) -> &ModeInfo {
        &INFO
    }

    fn on_config(&mut self, cfg: &ModeConfig, roster: &[SeatInfo]) -> Result<(), String> {
        cfg.check_known(OPTS)?;
        // server.rs L1181: clamp(1, 15).
        let best_of = cfg.get::<u8>("best_of", 3)?.clamp(1, 15);
        let round_s = cfg.get::<u32>("round_s", 0)?;
        if roster.is_empty() || roster.len() > INFO.max_players as usize {
            return Err(format!("duel needs 1..={} players, got {}", INFO.max_players, roster.len()));
        }
        let mut c = CoreCfg::new(MatchRule { wins: needed_wins(best_of), max_rounds: 0 });
        c.map_center = cfg.map_center;
        if round_s > 0 {
            c.sudden_death = Some(SuddenDeath { round_ms: round_s as Ms * 1000, table: ZoneTable::sudden_death() });
        }
        self.best_of = best_of;
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
        self.core.reset()
    }

    fn spawn_policy(&self, _ctx: &ModeCtx, _round: u32) -> SpawnPolicy {
        SpawnPolicy::Fixed
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
    //! The server.rs `round_tests` (L2146-2255), re-run against the mode.
    use super::*;

    fn seats(n: u32) -> Vec<SeatInfo> {
        (1..=n).map(|i| SeatInfo { seat: i, key: format!("p{i}"), rating: 0 }).collect()
    }
    fn view(seat: SeatId) -> SeatView {
        SeatView { seat, present: true, pos: None, ledger_health: 100.0 }
    }

    struct H {
        m: Duel,
        roster: Vec<SeatView>,
        out: Vec<ModeEvent>,
        now: Ms,
    }
    impl H {
        fn live(n: u32, best_of: u8) -> Self {
            let mut m = Duel::new();
            m.on_config(&ModeConfig::default().with("best_of", &best_of.to_string()), &seats(n)).unwrap();
            let mut h = H { m, roster: (1..=n).map(view).collect(), out: vec![], now: 1_000 };
            let f: Vec<SeatId> = (1..=n).collect();
            assert!(h.with(|m, c| m.on_round_start(c, 1, &f)));
            h
        }
        fn with<R>(&mut self, f: impl FnOnce(&mut Duel, &mut ModeCtx) -> R) -> R {
            let mut ctx = ModeCtx::new(self.now, &self.roster, &mut self.out);
            f(&mut self.m, &mut ctx)
        }
        fn kill(&mut self, v: SeatId, k: SeatId) -> DeathOutcome {
            self.with(|m, c| m.on_death(c, v, Some(k), DEATH_DAMAGE))
        }
        fn advance(&mut self, ms: Ms) {
            self.now += ms;
            self.with(|m, c| m.on_tick(c));
        }
        fn settle(&mut self) {
            for _ in 0..20 {
                self.advance(33);
            }
        }
        fn drop_seat(&mut self, s: SeatId) {
            self.roster.retain(|v| v.seat != s);
            self.with(|m, c| m.on_disconnect(c, s));
        }
    }

    #[test]
    fn kill_settles_then_one_winner() {
        let mut h = H::live(2, 3);
        assert_eq!(h.kill(2, 1), DeathOutcome::OutForRound);
        assert!(matches!(h.m.status(), ModeStatus::Settling { until_ms: 1_400, .. }));
        assert_eq!(h.m.status().phase_str(), "roundover");
        assert!(h.m.round_result().is_none(), "no result while settling");
        assert!(h.m.status().combat_open(), "trade window open");
        assert_eq!(h.kill(2, 1), DeathOutcome::Ignored, "idempotent");
        h.advance(399);
        assert!(matches!(h.m.status(), ModeStatus::Settling { .. }), "399 ms: still settling");
        h.advance(1);
        assert_eq!(h.m.round_result().unwrap().outcome, RoundOutcome::Won(Side::Seat(1)));
        assert_eq!(h.m.core().score(Side::Seat(1)), 1);
        assert!(!h.m.status().combat_open());
        assert_eq!(h.kill(1, 2), DeathOutcome::Ignored, "late report after result");
        assert_eq!(h.m.round_results().len(), 1);
    }

    #[test]
    fn mutual_kill_in_window_is_a_draw_in_either_order() {
        for first in [1u32, 2] {
            let mut h = H::live(2, 3);
            let second = 3 - first;
            h.kill(first, second);
            h.advance(100); // trade hit lands 100 ms later
            assert_eq!(h.kill(second, first), DeathOutcome::OutForRound);
            h.settle();
            let r = h.m.round_result().unwrap();
            assert_eq!(r.outcome, RoundOutcome::Draw);
            assert!(matches!(h.m.status(), ModeStatus::RoundOver(_)));
            assert_eq!(h.m.core().score(Side::Seat(1)) + h.m.core().score(Side::Seat(2)), 0);
        }
    }

    #[test]
    fn deaths_outside_an_open_round_are_ignored() {
        let mut m = Duel::new();
        m.on_config(&ModeConfig::default(), &seats(2)).unwrap();
        let roster = vec![view(1), view(2)];
        let mut out = vec![];
        let mut c = ModeCtx::new(0, &roster, &mut out);
        assert_eq!(m.on_death(&mut c, 1, Some(2), DEATH_REPORTED), DeathOutcome::Ignored);
    }

    #[test]
    fn three_player_round_ends_when_others_leave() {
        let mut h = H::live(3, 3);
        h.kill(2, 1);
        assert!(matches!(h.m.status(), ModeStatus::Live { .. }), "two still standing");
        h.drop_seat(3); // 3p continues: no pause
        assert!(matches!(h.m.status(), ModeStatus::Settling { .. }));
        h.settle();
        assert_eq!(h.m.round_result().unwrap().outcome, RoundOutcome::Won(Side::Seat(1)));
    }

    #[test]
    fn match_point_ends_match() {
        let mut h = H::live(2, 1);
        h.with(|m, c| m.on_death(c, 1, Some(2), DEATH_VITALS));
        h.settle();
        let r = h.m.match_result().unwrap();
        assert_eq!((r.winner, r.end), (Some(Side::Seat(2)), MatchEnd::Score));
        assert_eq!(h.m.status().phase_str(), "match_over");
    }

    #[test]
    fn best_of_three_needs_two() {
        let mut h = H::live(2, 3);
        h.kill(2, 1);
        h.settle();
        assert!(h.m.match_result().is_none());
        assert!(h.with(|m, c| m.on_round_start(c, 2, &[1, 2])));
        h.kill(2, 1);
        h.settle();
        assert_eq!(h.m.match_result().unwrap().winner, Some(Side::Seat(1)));
    }

    #[test]
    fn drop_pauses_then_replays_or_forfeits() {
        // Opponent drops mid-round: pause, round voided, settle cancelled.
        let mut h = H::live(2, 3);
        h.kill(2, 1); // settling
        h.drop_seat(1);
        assert!(matches!(h.m.status(), ModeStatus::Paused { until_ms: 31_000, .. }));
        assert_eq!(h.m.round_result().unwrap().outcome, RoundOutcome::Void(VoidReason::Paused));
        // Back within 30 s: replay.
        h.roster.push(view(1));
        h.advance(5_000);
        assert_eq!(h.m.status(), &ModeStatus::Waiting { replay: true });
        assert!(h.with(|m, c| m.on_round_start(c, 2, &[1, 2])));

        // Not back: forfeit to the one present at expiry.
        let mut h = H::live(2, 3);
        h.drop_seat(2);
        h.advance(29_999);
        assert!(matches!(h.m.status(), ModeStatus::Paused { .. }));
        h.advance(1);
        let r = h.m.match_result().unwrap();
        assert_eq!((r.winner, r.end), (Some(Side::Seat(1)), MatchEnd::Forfeit));
        assert_eq!(h.m.core().score(Side::Seat(1)), 2, "raised to needed wins");

        // Everyone gone: lobby.
        let mut h = H::live(2, 3);
        h.drop_seat(2);
        h.drop_seat(1);
        assert_eq!(h.m.status(), &ModeStatus::Abandoned);
        assert_eq!(h.m.status().phase_str(), "lobby");
    }

    #[test]
    fn stale_ping_counts_as_absent_but_not_dead() {
        let mut h = H::live(3, 3);
        h.roster[2].present = false; // seat 3 stops pinging
        h.kill(2, 1);
        assert!(matches!(h.m.status(), ModeStatus::Settling { .. }), "3 not standing while absent");
        h.roster[2].present = true; // pings resume inside the settle window
        h.settle();
        assert_eq!(h.m.round_result().unwrap().outcome, RoundOutcome::Draw, "two standing at the deadline => draw");
    }

    #[test]
    fn barrier_timeout_forfeits_or_proceeds() {
        let mut m = Duel::new();
        m.on_config(&ModeConfig::default(), &seats(2)).unwrap();
        let roster = vec![view(1), view(2)];
        let mut out = vec![];
        let mut c = ModeCtx::new(0, &roster, &mut out);
        m.on_barrier_timeout(&mut c, &[1, 2]);
        assert_eq!(m.status(), &ModeStatus::Waiting { replay: false });
        m.on_barrier_timeout(&mut c, &[2]);
        assert_eq!(m.match_result().unwrap().winner, Some(Side::Seat(2)));
    }

    #[test]
    fn solo_round_ends_only_on_death() {
        let mut h = H::live(1, 3);
        h.advance(10_000);
        assert!(matches!(h.m.status(), ModeStatus::Live { .. }));
        h.with(|m, c| m.on_death(c, 1, None, DEATH_REPORTED));
        h.settle();
        assert_eq!(h.m.round_result().unwrap().outcome, RoundOutcome::Draw);
    }

    #[test]
    fn sudden_death_tiebreak_by_ledger_health() {
        let mut m = Duel::new();
        let cfg = ModeConfig { map_center: Some([0.0, 0.0, 0.0]), ..Default::default() }.with("round_s", "10");
        m.on_config(&cfg, &seats(2)).unwrap();
        let mut roster = vec![view(1), view(2)];
        roster[0].pos = Some([0.0, 0.0, 0.0]);
        roster[1].pos = Some([1000.0, 0.0, 0.0]); // outside the 3 m ring
        roster[0].ledger_health = 40.0;
        roster[1].ledger_health = 90.0;
        let mut out = vec![];
        let mut now = 0;
        {
            let mut c = ModeCtx::new(now, &roster, &mut out);
            assert!(m.on_round_start(&mut c, 1, &[1, 2]));
        }
        while now < 41_000 {
            now += 50;
            let mut c = ModeCtx::new(now, &roster, &mut out);
            m.on_tick(&mut c);
        }
        assert!(out.iter().any(|e| matches!(e, ModeEvent::ZoneDamage { seat: 2, .. })));
        assert!(!out.iter().any(|e| matches!(e, ModeEvent::ZoneDamage { seat: 1, .. })));
        let r = m.round_result().unwrap();
        assert!(r.tiebreak);
        assert_eq!(r.outcome, RoundOutcome::Won(Side::Seat(2)), "reducer never applied the damage; 90 > 40");
        assert_eq!(r.decided_at_ms, 40_000);
    }

    #[test]
    fn hud_section_reports_scores_and_settle_deadline() {
        let mut h = H::live(2, 3);
        h.kill(2, 1);
        let s = h.m.hud_section(h.now);
        assert_eq!((s.mode.as_str(), s.round, s.state, s.deadline_ms, s.target_wins), ("duel", 1, RoundStateTag::Settling, 1_400, 2));
        assert_eq!(s.seats.iter().find(|r| r.seat == 1).unwrap().kills, 1);
        h.settle();
        let s = h.m.hud_section(h.now);
        assert_eq!(s.sides.iter().find(|r| r.side == Side::Seat(1)).unwrap().score, 1);
        assert!(matches!(s.last_result, Some(RoundResult { outcome: RoundOutcome::Won(Side::Seat(1)), .. })));
    }
}
