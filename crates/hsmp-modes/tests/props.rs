//! Property tests for the mode framework (docs/development/subsystems/modes.md):
//! - exactly one result per started round (Void included), in round order;
//! - monotonic rounds (a stale/duplicate round start is refused);
//! - no last-standing winner while 2+ sides stand, and Live never persists
//!   with <= 1 side standing in a multi-side match;
//! - match results are consistent with the scores;
//! - deterministic replays and deterministic team balance.

use hsmp_modes::teams;
use hsmp_modes::zone::{Zone, ZoneTable};
use hsmp_modes::*;
use proptest::prelude::*;
use std::collections::BTreeSet;

#[derive(Clone, Debug)]
enum Op {
    Advance(u16),
    Kill(u8, u8),
    Suicide(u8),
    Damage(u8, u8, u8),
    Drop(u8),
    Rejoin(u8),
    TogglePing(u8),
    StartRound,
    StaleRoundStart,
    BarrierTimeout(u8),
}

fn op() -> impl Strategy<Value = Op> {
    prop_oneof![
        4 => (1u16..1500).prop_map(Op::Advance),
        // Long stalls: round limits, the sudden-death zone, pause expiry.
        1 => (20_000u16..40_000).prop_map(Op::Advance),
        5 => (0u8..8, 0u8..8).prop_map(|(a, b)| Op::Kill(a, b)),
        1 => (0u8..8).prop_map(Op::Suicide),
        2 => (0u8..8, 0u8..8, 0u8..60).prop_map(|(a, b, d)| Op::Damage(a, b, d)),
        1 => (0u8..8).prop_map(Op::Drop),
        1 => (0u8..8).prop_map(Op::Rejoin),
        1 => (0u8..8).prop_map(Op::TogglePing),
        4 => Just(Op::StartRound),
        1 => Just(Op::StaleRoundStart),
        1 => (0u8..255).prop_map(Op::BarrierTimeout),
    ]
}

#[derive(Clone, Copy, Debug)]
enum Kind {
    Duel,
    Ffa,
    Lts,
}

struct Sim {
    m: Box<dyn GameMode>,
    n: u32,
    roster: Vec<SeatView>,
    out: Vec<ModeEvent>,
    now: Ms,
    round: u32,
    seen_results: usize,
}

fn seat_infos(n: u32) -> Vec<SeatInfo> {
    (1..=n).map(|i| SeatInfo { seat: i, key: format!("k{i}"), rating: (i as i32 * 37) % 11 }).collect()
}

impl Sim {
    fn new(kind: Kind, n: u32, round_s: u32) -> Self {
        let (mut m, cfg): (Box<dyn GameMode>, ModeConfig) = match kind {
            Kind::Duel => (create("duel").unwrap(), ModeConfig::default().with("round_s", &round_s.to_string())),
            Kind::Ffa => (create("ffa").unwrap(), ModeConfig::default().with("round_s", &round_s.to_string())),
            Kind::Lts => (create("lts").unwrap(), ModeConfig::default().with("round_s", &round_s.to_string()).with("best_of", "3")),
        };
        m.on_config(&cfg, &seat_infos(n)).unwrap();
        let roster = (1..=n).map(|s| SeatView { seat: s, present: true, pos: Some([s as f32 * 50.0, 0.0, 0.0]), ledger_health: 100.0 - s as f32 }).collect();
        Sim { m, n, roster, out: vec![], now: 1_000, round: 0, seen_results: 0 }
    }

    fn seat(&self, i: u8) -> SeatId {
        (i as u32 % self.n) + 1
    }

    fn ctx_do<R>(&mut self, f: impl FnOnce(&mut dyn GameMode, &mut ModeCtx) -> R) -> R {
        let mut ctx = ModeCtx::new(self.now, &self.roster, &mut self.out);
        f(self.m.as_mut(), &mut ctx)
    }

    fn finished(&self) -> bool {
        matches!(self.m.status(), ModeStatus::MatchOver(_) | ModeStatus::Abandoned)
    }

    /// Sides that are alive (per the mode) and present (per the roster).
    fn standing(&self) -> BTreeSet<Side> {
        let hud = self.m.hud_section(self.now);
        hud.seats.iter()
            .filter(|r| r.alive && self.roster.iter().any(|v| v.seat == r.seat && v.present))
            .map(|r| if r.team == NO_TEAM { Side::Seat(r.seat) } else { Side::Team(r.team) })
            .collect()
    }

    fn participant_sides(&self) -> BTreeSet<Side> {
        self.m.hud_section(self.now).seats.iter()
            .map(|r| if r.team == NO_TEAM { Side::Seat(r.seat) } else { Side::Team(r.team) })
            .collect()
    }

    fn apply(&mut self, op: &Op) {
        match *op {
            Op::Advance(ms) => self.now += ms as Ms,
            Op::Kill(v, k) => {
                let (v, k) = (self.seat(v), self.seat(k));
                self.ctx_do(|m, c| m.on_death(c, v, Some(k), DEATH_DAMAGE));
            }
            Op::Suicide(v) => {
                let v = self.seat(v);
                self.ctx_do(|m, c| m.on_death(c, v, None, DEATH_VITALS));
            }
            Op::Damage(a, v, d) => {
                let (a, v) = (self.seat(a), self.seat(v));
                self.ctx_do(|m, c| m.on_damage(c, a, v, d as f32));
            }
            Op::Drop(i) => {
                let s = self.seat(i);
                if self.roster.iter().any(|v| v.seat == s) {
                    self.roster.retain(|v| v.seat != s);
                    self.ctx_do(|m, c| m.on_disconnect(c, s));
                }
            }
            Op::Rejoin(i) => {
                let s = self.seat(i);
                if !self.roster.iter().any(|v| v.seat == s) {
                    self.roster.push(SeatView { seat: s, present: true, pos: None, ledger_health: 100.0 });
                    let info = SeatInfo { seat: s, key: format!("k{s}"), rating: 0 };
                    self.ctx_do(|m, c| m.on_join(c, &info));
                }
            }
            Op::TogglePing(i) => {
                let s = self.seat(i);
                if let Some(v) = self.roster.iter_mut().find(|v| v.seat == s) {
                    v.present = !v.present;
                }
            }
            Op::StartRound => {
                let can = matches!(self.m.status(), ModeStatus::Waiting { .. } | ModeStatus::RoundOver(_));
                let fighters: Vec<SeatId> = self.roster.iter().filter(|v| v.present).map(|v| v.seat).collect();
                let next = self.round + 1;
                let ok = self.ctx_do(|m, c| m.on_round_start(c, next, &fighters));
                assert_eq!(ok, can, "round start accepted iff the mode is between rounds");
                if ok {
                    self.round = next;
                    assert_eq!(self.m.status(), &ModeStatus::Live { round: next });
                }
            }
            Op::StaleRoundStart => {
                let before = self.m.status().clone();
                let r = self.round;
                let fighters: Vec<SeatId> = self.roster.iter().map(|v| v.seat).collect();
                assert!(!self.ctx_do(|m, c| m.on_round_start(c, r, &fighters)), "stale round {r} accepted");
                assert_eq!(self.m.status(), &before);
            }
            Op::BarrierTimeout(mask) => {
                if matches!(self.m.status(), ModeStatus::Waiting { .. } | ModeStatus::RoundOver(_)) {
                    let loaded: Vec<SeatId> = self.roster.iter().filter(|v| v.present && (mask >> (v.seat % 8)) & 1 == 1).map(|v| v.seat).collect();
                    self.ctx_do(|m, c| m.on_barrier_timeout(c, &loaded));
                }
            }
        }
        // The reducer ticks continuously.
        self.ctx_do(|m, c| m.on_tick(c));
        self.check();
    }

    fn check(&mut self) {
        let status = self.m.status().clone();
        let results = self.m.round_results().to_vec();

        // Exactly one result per started round, in order.
        let open = matches!(status, ModeStatus::Live { .. } | ModeStatus::Settling { .. });
        let expected = if open { self.round - 1 } else { self.round };
        assert_eq!(results.len() as u32, expected, "status {:?} results {:?}", status, results);
        for (i, r) in results.iter().enumerate() {
            assert_eq!(r.round, i as u32 + 1, "results out of order: {:?}", results);
        }
        if let Some(rr) = self.m.round_result() {
            assert_eq!(Some(rr), results.last());
        }

        // New results: no last-standing winner while 2+ sides stand.
        let standing = self.standing();
        for r in &results[self.seen_results..] {
            match r.outcome {
                RoundOutcome::Won(side) if !r.tiebreak => {
                    assert_eq!(standing, [side].into_iter().collect::<BTreeSet<_>>(),"winner {:?} with standing {:?}", side, standing);
                }
                RoundOutcome::Draw if !r.tiebreak => assert_ne!(standing.len(), 1, "draw with one side standing"),
                _ => {}
            }
        }
        self.seen_results = results.len();

        // Live never persists with <= 1 side standing in a multi-side match.
        if let ModeStatus::Live { .. } = status {
            if self.participant_sides().len() >= 2 {
                assert!(standing.len() >= 2, "live with standing {:?}", standing);
            }
        }
        if let ModeStatus::Settling { until_ms, .. } = status {
            assert!(until_ms > self.now && until_ms <= self.now + 400);
        }

        // Match result consistency.
        match &status {
            ModeStatus::MatchOver(mr) => {
                assert_eq!(Some(mr), self.m.match_result());
                let hud = self.m.hud_section(self.now);
                if let (Some(w), MatchEnd::Score | MatchEnd::Forfeit) = (mr.winner, mr.end) {
                    let s = hud.sides.iter().find(|x| x.side == w).map(|x| x.score).unwrap_or(0);
                    assert!(s >= hud.target_wins, "winner below target");
                }
                if mr.end == MatchEnd::Score {
                    assert!(matches!(results.last().unwrap().outcome, RoundOutcome::Won(_)));
                }
            }
            _ => assert!(self.m.match_result().is_none()),
        }
        assert!(!status.combat_open() || matches!(status, ModeStatus::Live { .. } | ModeStatus::Settling { .. }));
    }
}

fn kind() -> impl Strategy<Value = (Kind, u32)> {
    prop_oneof![
        (2u32..=4).prop_map(|n| (Kind::Duel, n)),
        (3u32..=6).prop_map(|n| (Kind::Ffa, n)),
        (2u32..=6).prop_map(|n| (Kind::Lts, n)),
    ]
}

/// Guard against a vacuous harness: the op generator must actually reach
/// every interesting outcome.
#[test]
fn harness_reaches_every_outcome() {
    use proptest::strategy::ValueTree;
    use proptest::test_runner::{Config, TestRng, TestRunner};
    let mut runner = TestRunner::new_with_rng(Config::default(), TestRng::deterministic_rng(proptest::test_runner::RngAlgorithm::ChaCha));
    let strat = (kind(), prop_oneof![Just(0u32), 1u32..4], prop::collection::vec(op(), 1..120));
    let mut seen: BTreeSet<&'static str> = BTreeSet::new();
    for _ in 0..1500 {
        let ((k, n), round_s, ops) = strat.new_tree(&mut runner).unwrap().current();
        let mut sim = Sim::new(k, n, round_s);
        for o in &ops {
            if sim.finished() { break; }
            sim.apply(o);
            if matches!(sim.m.status(), ModeStatus::Paused { .. }) { seen.insert("paused"); }
            if matches!(sim.m.status(), ModeStatus::Waiting { replay: true }) { seen.insert("replay"); }
        }
        for r in sim.m.round_results() {
            seen.insert(match r.outcome {
                RoundOutcome::Won(_) if r.tiebreak => "tiebreak_win",
                RoundOutcome::Won(Side::Team(_)) => "team_win",
                RoundOutcome::Won(_) => "win",
                RoundOutcome::Draw => "draw",
                RoundOutcome::Void(_) => "void",
            });
        }
        if let Some(m) = sim.m.match_result() {
            seen.insert(match m.end { MatchEnd::Score => "match_score", MatchEnd::RoundCap => "match_cap", MatchEnd::Forfeit => "match_forfeit" });
        }
        if sim.m.status() == &ModeStatus::Abandoned { seen.insert("abandoned"); }
    }
    for want in ["paused", "replay", "tiebreak_win", "team_win", "win", "draw", "void", "match_score", "match_cap", "match_forfeit", "abandoned"] {
        assert!(seen.contains(want), "harness never reached {want}; saw {seen:?}");
    }
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 600, ..ProptestConfig::default() })]

    #[test]
    fn round_invariants((k, n) in kind(), round_s in prop_oneof![Just(0u32), 1u32..4], ops in prop::collection::vec(op(), 1..120)) {
        let mut sim = Sim::new(k, n, round_s);
        for o in &ops {
            if sim.finished() { break; }
            sim.apply(o);
        }
    }

    #[test]
    fn replays_are_deterministic((k, n) in kind(), ops in prop::collection::vec(op(), 1..80)) {
        let run = || {
            let mut sim = Sim::new(k, n, 2);
            for o in &ops {
                if sim.finished() { break; }
                sim.apply(o);
            }
            (sim.m.round_results().to_vec(), sim.m.match_result().cloned(), sim.m.hud_section(sim.now), sim.out)
        };
        prop_assert_eq!(run(), run());
    }

    #[test]
    fn team_balance_is_deterministic_and_even(
        ratings in prop::collection::vec(-500i32..3000, 1..33),
        n_teams in 2u8..=4,
        shuffle_seed in any::<u64>(),
    ) {
        let seats: Vec<SeatInfo> = ratings.iter().enumerate()
            .map(|(i, r)| SeatInfo { seat: i as u32 + 10, key: format!("key{:03}", (i * 7919) % 1000), rating: *r })
            .collect();
        let a = teams::balance(&seats, n_teams);
        // Same result for any input order.
        let mut shuffled = seats.clone();
        let mut x = shuffle_seed | 1;
        for i in (1..shuffled.len()).rev() {
            x ^= x << 13; x ^= x >> 7; x ^= x << 17;
            shuffled.swap(i, (x % (i as u64 + 1)) as usize);
        }
        prop_assert_eq!(&a, &teams::balance(&shuffled, n_teams));
        // Everyone exactly once, teams in range, sizes within one.
        prop_assert_eq!(a.len(), seats.len());
        prop_assert!(a.values().all(|t| (1..=n_teams).contains(t)));
        let sizes = teams::team_sizes(a.iter(), n_teams);
        let (lo, hi) = (sizes.values().min().unwrap(), sizes.values().max().unwrap());
        prop_assert!(hi - lo <= 1, "sizes {:?}", sizes);
        // Joiner pick is the smallest team.
        let table = sizes.iter().map(|(t, n)| (*t, (*n, 0u32))).collect();
        let pick = teams::pick_team(&table, n_teams);
        prop_assert_eq!(sizes[&pick], *lo);
    }

    #[test]
    fn zone_radius_monotonic_and_dps_sane(seed in any::<u64>(), times in prop::collection::vec(0u64..500_000, 1..60)) {
        let cands: Vec<[f32; 2]> = (0..200).map(|i| [((i * 53) % 76) as f32 * 100.0 - 3800.0, ((i * 29) % 76) as f32 * 100.0 - 3800.0]).collect();
        let z = Zone::seeded(ZoneTable::ambush16(), 0, [0.0, 0.0], &cands, seed).unwrap();
        let mut ts = times.clone();
        ts.sort_unstable();
        let mut prev = f32::INFINITY;
        for t in ts {
            let s = z.sample(t);
            prop_assert!(s.radius_cm <= prev + 1e-3);
            prop_assert!(s.radius_cm >= 0.0 && s.dps >= 0.0);
            prop_assert_eq!(s.finished, t >= z.end_ms());
            prev = s.radius_cm;
            // The centre is always inside: zero damage there.
            prop_assert_eq!(z.dps_at([s.center[0], s.center[1], 0.0], t), 0.0);
        }
    }
}
