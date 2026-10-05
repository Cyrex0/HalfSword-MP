//! Game-mode tests (modes.rs): teams, friendly fire, King of the hill, roulette / brawl
//! kits, deathmatch respawns, the round clock, the records and the compatibility gate.
//! Matches run through the real round flow (`start_match`, `tick::advance`).

use super::*;
use crate::server::match_core::round_tests::peer;
use crate::server::tick::advance;
use hsmp_ipc::record::view;

/// Peer ids of these tests (the negotiated-caps map is process-wide): BASE + test * 16 + n.
const BASE: PeerId = 91_000;

/// A server with `n` connected, ready players (distinct keys, seats 1..=n) whose clients
/// negotiated `caps`, on Map_Arena_Pit.
fn server(test: u32, n: u32, caps: u64) -> ServerState {
    let st = ServerState::new(16);
    {
        let mut i = st.inner.try_lock().unwrap();
        for k in 1..=n {
            let id = BASE + test * 16 + k;
            let mut p = peer(id, &format!("p{k}"));
            p.player_key = [k as u8; 32];
            p.last_valid_pos = Some([0.0, 0.0, 0.0]);
            i.peers.insert(addr_of(id), p);
            crate::interact::note_caps(id, caps);
        }
        i.match_arena = "Map_Arena_Pit".into();
        session::reconcile_seats(&mut i);
    }
    st
}

fn addr_of(id: PeerId) -> SocketAddr { format!("127.0.0.1:{}", 10_000 + (id % 50_000)).parse().unwrap() }
fn id(test: u32, k: u32) -> PeerId { BASE + test * 16 + k }
fn key(k: u32) -> session::PlayerKey { [k as u8; 32] }
fn all_caps() -> u64 { caps::MODES | caps::ZONE }

fn set_mode(i: &mut Inner, c: ModeCfg) { i.modes.cfg = c; }

/// Run the flow for `ms` in 50 ms steps (every client's transport keeps talking).
fn run(i: &mut Inner, ms: u64) {
    let end = i.now_ms + ms;
    while i.now_ms < end {
        let t = (i.now_ms + 50).min(end);
        for p in i.peers.values_mut() { p.last_seen_ms = t; }
        advance(i, t);
    }
}

/// START (forced) and run to Live.
fn go_live(i: &mut Inner) {
    let r = session::start_match(i, true, "test");
    assert!(r.ok, "{:?}", r);
    run(i, FIRST_COUNTDOWN_MS + 100);
    assert_eq!(i.match_state, "live");
}

fn wins(i: &Inner, pid: PeerId) -> u32 { i.peers.values().find(|p| p.id == pid).unwrap().wins }
fn alive(i: &Inner, pid: PeerId) -> bool { i.peers.values().find(|p| p.id == pid).unwrap().alive }
fn pos(i: &mut Inner, pid: PeerId, at: [f32; 3]) {
    i.peers.values_mut().find(|p| p.id == pid).unwrap().last_valid_pos = Some(at);
}

// ---- config -------------------------------------------------------------------------------

#[test]
fn config_defaults_and_names() {
    let c = ModeCfg::default();
    assert_eq!((c.mode, c.team_count(), c.rule(), c.round_time()), (gm::DUEL, 0, v5::TeamRule::NONE, 0));
    let te = ModeCfg { mode: gm::TEAM_ELIM, teams: 3, ..c };
    assert_eq!((te.team_count(), te.rule()), (3, v5::TeamRule::AUTO), "team elimination always has teams");
    let ffa = ModeCfg { mode: gm::FFA, team_rule: v5::TeamRule::FIXED, ..c };
    assert_eq!(ffa.team_count(), 0, "FFA never has teams");
    let tdm = ModeCfg { mode: gm::DEATHMATCH, team_rule: v5::TeamRule::FIXED, teams: 9, ..c };
    assert_eq!((tdm.team_count(), tdm.round_time()), (MAX_TEAMS, DM_TIME_S));
    assert_eq!(ModeCfg { mode: gm::KING_OF_HILL, ..c }.round_time(), KOTH_TIME_S);
    assert_eq!(ModeCfg { mode: gm::KING_OF_HILL, round_time_s: 90, ..c }.round_time(), 90);
    for m in 0..=gm::MAX {
        assert_eq!(parse_mode(mode_name(m)), Some(m));
    }
    assert_eq!(parse_mode("Best of 5"), None);
    assert_eq!(parse_mode(" King_Of_Hill "), Some(gm::KING_OF_HILL));
    assert!(!needs_modes_cap(gm::DUEL) && !needs_modes_cap(gm::FFA));
    assert!((gm::TEAM_ELIM..=gm::MAX).all(needs_modes_cap));
}

#[test]
fn modes_that_old_clients_cannot_play_are_gated() {
    let st = server(1, 2, 0);
    let mut i = st.inner.try_lock().unwrap();
    // Connected beta.5 clients: duel / FFA only.
    assert!(set_cfg(&mut i, Some(gm::FFA), None, None, None, "t").is_ok());
    let e = set_cfg(&mut i, Some(gm::DEATHMATCH), None, None, None, "t").unwrap_err();
    assert_eq!(e.0, CmdReason::UNSUPPORTED);
    assert!(e.1.contains("p1, p2"), "{}", e.1);
    assert_eq!(i.modes.cfg.mode, gm::FFA);
    assert_eq!(join_refusal(&i, 0), None, "an old client may join a duel / FFA server");
    // Once every client has the cap the mode is accepted; old joiners are then refused.
    for k in 1..=2 { crate::interact::note_caps(id(1, k), caps::MODES); }
    assert!(set_cfg(&mut i, Some(gm::BRAWL), None, None, None, "t").is_ok());
    let why = join_refusal(&i, 0).expect("refused");
    assert!(why.contains("Brawl") && why.contains("update HSMP"), "{why}");
    assert_eq!(join_refusal(&i, caps::MODES), None);
    // Value checks.
    assert_eq!(set_cfg(&mut i, Some(gm::MAX + 1), None, None, None, "t").unwrap_err().0, CmdReason::INVALID_VALUE);
    assert_eq!(set_cfg(&mut i, None, Some(3), None, None, "t").unwrap_err().0, CmdReason::INVALID_VALUE);
    assert_eq!(set_cfg(&mut i, None, None, Some(5), None, "t").unwrap_err().0, CmdReason::INVALID_VALUE);
    assert_eq!(set_cfg(&mut i, None, None, None, Some(MAX_ROUND_TIME_S + 1), "t").unwrap_err().0, CmdReason::INVALID_VALUE);
    assert_eq!(set_option(&mut i, mode_opt::KOTH_TARGET, 5, "t").unwrap_err().0, CmdReason::INVALID_VALUE);
    assert!(set_option(&mut i, mode_opt::KOTH_TARGET, 90, "t").is_ok());
    assert!(set_option(&mut i, mode_opt::FRIENDLY_FIRE, 1, "t").is_ok());
    assert_eq!(set_option(&mut i, mode_opt::RESPAWN_S, 0, "t").unwrap_err().0, CmdReason::INVALID_VALUE);
    assert_eq!((i.modes.cfg.koth_target_s, i.modes.cfg.friendly_fire), (90, true));
}

#[test]
fn the_session_config_carries_the_mode() {
    let st = server(2, 2, all_caps());
    let mut i = st.inner.try_lock().unwrap();
    let rev = { session::refresh_config_rev(&mut i); i.sess.config_rev };
    set_mode(&mut i, ModeCfg { mode: gm::TEAM_ELIM, teams: 2, round_time_s: 120, ..Default::default() });
    let c = session::live_config(&i);
    assert_eq!((c.mode, c.team_rule, c.teams, c.round_time_limit_s), (gm::TEAM_ELIM, v5::TeamRule::AUTO, 2, 120));
    session::refresh_config_rev(&mut i);
    assert_eq!(i.sess.config_rev, rev + 1, "a mode change bumps the config revision");
}

// ---- teams --------------------------------------------------------------------------------

#[test]
fn auto_teams_are_balanced_by_seat_and_fixed_picks_are_kept() {
    let st = server(3, 5, all_caps());
    let mut i = st.inner.try_lock().unwrap();
    set_mode(&mut i, ModeCfg { mode: gm::TEAM_ELIM, teams: 2, ..Default::default() });
    go_live(&mut i);
    let t: Vec<u8> = (1..=5).map(|k| i.modes.teams[&key(k)]).collect();
    assert_eq!(t, vec![1, 2, 1, 2, 1], "seat order, smallest team first");
    // The roster carries the team (HSMPCombat's native same-team behaviour reads it).
    let s = session::build_session(&i, 0);
    assert!(s.rows.iter().all(|r| r.team == i.modes.teams[&r_key(&i, r.peer_id)]));
    reset_to_lobby(&mut i);
    assert!(i.modes.teams.is_empty() && i.modes.run.is_none());

    // FIXED: picks are kept, the rest fill the smallest team.
    set_mode(&mut i, ModeCfg { mode: gm::TEAM_ELIM, team_rule: v5::TeamRule::FIXED, teams: 3, ..Default::default() });
    pick_team(&mut i, key(1), 3).unwrap();
    pick_team(&mut i, key(2), 3).unwrap();
    assert_eq!(pick_team(&mut i, key(3), 4).unwrap_err().0, CmdReason::INVALID_VALUE);
    let s = session::build_session(&i, 0);
    assert_eq!(s.rows.iter().find(|r| r.seat == 1).unwrap().team, 3, "lobby picks show in the roster");
    go_live(&mut i);
    let t: Vec<u8> = (1..=5).map(|k| i.modes.teams[&key(k)]).collect();
    assert_eq!(t, vec![3, 3, 1, 2, 1]);
}

fn r_key(i: &Inner, pid: PeerId) -> session::PlayerKey {
    session::peer_key(i.peers.values().find(|p| p.id == pid).unwrap())
}

#[test]
fn fixed_teams_need_a_player_on_every_team() {
    let st = server(4, 3, all_caps());
    let mut i = st.inner.try_lock().unwrap();
    set_mode(&mut i, ModeCfg { mode: gm::KING_OF_HILL, team_rule: v5::TeamRule::FIXED, teams: 2, ..Default::default() });
    for k in 1..=3 { pick_team(&mut i, key(k), 1).unwrap(); }
    let r = session::start_match(&mut i, true, "test");
    assert!(!r.ok);
    assert_eq!(r.code, CmdReason::NOT_ENOUGH_PLAYERS);
    assert!(r.text.contains("team Blue has no players"), "{}", r.text);
    assert_eq!(i.match_state, "lobby");
    assert!(i.participants.is_empty());
    // AUTO teams have no picks.
    set_mode(&mut i, ModeCfg { mode: gm::TEAM_ELIM, ..Default::default() });
    assert_eq!(pick_team(&mut i, key(1), 1).unwrap_err().0, CmdReason::UNSUPPORTED);
    set_mode(&mut i, ModeCfg::default());
    assert_eq!(pick_team(&mut i, key(1), 1).unwrap_err().0, CmdReason::UNSUPPORTED, "duel has no teams");
}

#[test]
fn last_team_standing_wins_and_every_member_scores() {
    let st = server(5, 4, all_caps());
    let mut i = st.inner.try_lock().unwrap();
    set_mode(&mut i, ModeCfg { mode: gm::TEAM_ELIM, teams: 2, ..Default::default() });
    i.best_of = 3;
    go_live(&mut i);
    // Teams: seats 1, 3 = Red; 2, 4 = Blue.
    let (r1, b1, r2, b2) = (id(5, 1), id(5, 2), id(5, 3), id(5, 4));
    assert!(declare_death(&mut i, r1, b1, DEATH_DAMAGE));
    assert_eq!(i.match_state, "live", "Red still has a fighter");
    assert!(declare_death(&mut i, r2, b2, DEATH_DAMAGE));
    assert_eq!(i.match_state, "roundover");
    run(&mut i, SETTLE_MS + 50);
    assert_eq!(i.modes.team_wins, [0, 1, 0, 0]);
    assert_eq!((wins(&i, b1), wins(&i, b2), wins(&i, r1)), (1, 1, 0), "the dead Blue member scores too");
    assert_eq!(i.modes.winner_team, 2);
    assert_eq!(i.modes.result, mode_result::ELIMINATION);
    assert!([b1, b2].contains(&i.last_winner));
    // Round 2: Blue wins again -> match over (best of 3).
    run(&mut i, ROUNDOVER_MS + NEXT_ROUND_COUNTDOWN_MS + 200);
    assert_eq!(i.match_state, "live");
    assert!(declare_death(&mut i, r1, b2, DEATH_DAMAGE));
    assert!(declare_death(&mut i, r2, b2, DEATH_DAMAGE));
    run(&mut i, SETTLE_MS + 50);
    assert_eq!(i.match_state, "match_over");
    let (_, name, w) = i.sess.match_winner.clone().unwrap();
    assert_eq!((name.as_str(), w), ("Team Blue", 2));
}

#[test]
fn a_teammate_dropping_does_not_pause_a_whole_team_does() {
    let st = server(6, 4, all_caps());
    let mut i = st.inner.try_lock().unwrap();
    set_mode(&mut i, ModeCfg { mode: gm::TEAM_ELIM, teams: 2, ..Default::default() });
    go_live(&mut i);
    // Red (seats 1, 3): seat 1 drops -> 1v2 goes on.
    let red1 = i.peers.remove(&addr_of(id(6, 1))).unwrap();
    run(&mut i, 100);
    assert_eq!(i.match_state, "live");
    // The other Red drops too: no Red present -> pause for a reconnect.
    i.peers.remove(&addr_of(id(6, 3)));
    run(&mut i, 100);
    assert_eq!(i.match_state, "paused");
    // A Red comes back: the round resumes.
    i.peers.insert(addr_of(id(6, 1)), red1);
    run(&mut i, 100);
    assert_eq!(i.match_state, "live");
    // It drops again with its pause spent: Red loses the round, then the match
    // pauses for Red's return and the grace runs out: Blue wins by forfeit.
    i.peers.remove(&addr_of(id(6, 1)));
    run(&mut i, SETTLE_MS + 200);
    assert_eq!(i.modes.team_wins, [0, 1, 0, 0]);
    assert_eq!(i.match_state, "paused");
    run(&mut i, 30_100);
    assert_eq!(i.match_state, "match_over");
    assert_eq!(i.match_reason, "forfeit");
    assert_eq!(i.modes.team_wins[1], 2);
    assert_eq!(wins(&i, id(6, 4)), 2);
}

#[test]
fn friendly_fire_is_refused_between_teammates_unless_allowed() {
    let st = server(7, 4, all_caps());
    let mut i = st.inner.try_lock().unwrap();
    set_mode(&mut i, ModeCfg { mode: gm::TEAM_ELIM, teams: 2, ..Default::default() });
    go_live(&mut i);
    let (r1, b1, r2) = (id(7, 1), id(7, 2), id(7, 3));
    assert_eq!(hit_refusal(&i, r1, r2), Some("friendly fire"));
    assert_eq!(hit_refusal(&i, r1, b1), None);
    i.modes.run.as_mut().unwrap().friendly_fire = true;
    assert_eq!(hit_refusal(&i, r1, r2), None);
    reset_to_lobby(&mut i);
    set_mode(&mut i, ModeCfg { mode: gm::FFA, ..Default::default() });
    go_live(&mut i);
    assert_eq!(hit_refusal(&i, r1, r2), None, "no teams in FFA");
}

#[test]
fn kills_and_the_kill_feed() {
    let st = server(8, 4, all_caps());
    let mut i = st.inner.try_lock().unwrap();
    crate::interact::note_caps(id(8, 4), 0); // an old client: no kill feed
    set_mode(&mut i, ModeCfg { mode: gm::TEAM_ELIM, teams: 2, ..Default::default() });
    go_live(&mut i);
    i.out_msgs.clear();
    let (r1, b1, r2) = (id(8, 1), id(8, 2), id(8, 3));
    assert!(declare_death(&mut i, r1, r2, DEATH_DAMAGE)); // a team kill: no credit
    assert!(declare_death(&mut i, r2, b1, DEATH_DAMAGE)); // an enemy kill
    let s = |k: u32| i.modes.stats.get(&key(k)).copied().unwrap_or_default();
    assert_eq!((s(3).kills, s(2).kills, s(1).deaths, s(3).deaths), (0, 1, 1, 1));
    let feeds: Vec<(SocketAddr, rec::KillFeed)> = i.out_msgs.iter().filter_map(|(to, m)| {
        let (h, p) = wire::split(m).ok()?;
        (h.kind == rec::K_KILL_FEED).then(|| (to.unwrap(), view::<rec::KillFeed>(p).unwrap().head()))
    }).collect();
    assert_eq!(feeds.len(), 2 * 3, "two kills to the three peers with caps::MODES");
    assert!(feeds.iter().all(|(to, _)| *to != addr_of(id(8, 4))));
    assert!(feeds.iter().any(|(_, k)| k.victim_seat == 3 && k.killer_seat == 2 && k.cause == DEATH_DAMAGE));
    assert!(feeds.iter().any(|(_, k)| k.victim_seat == 1 && k.killer_seat == 3), "a team kill is still shown");
}

// ---- King of the hill ----------------------------------------------------------------------

#[test]
fn every_arena_has_a_hill_inside_its_bounds() {
    for (arena, _) in crate::spawns::MAP_DATA {
        let z = derived_zone(arena).unwrap_or_else(|| panic!("{arena}: no zone"));
        assert!((250.0..=600.0).contains(&z.radius_cm), "{arena}: {}", z.radius_cm);
        let t = crate::spawns::table(arena).unwrap();
        // The centre is the spawn centroid: every spawn is around it, none is inside.
        let near = t.points.iter().filter(|&&p| z.contains(t.all[p].pos)).count();
        assert!(near < t.points.len(), "{arena}: every spawn point is on the hill");
    }
    let o = parse_zone_overrides("Map_Arena_Pit:10,20,30,400; bad; Map_Arena_Alley:1,2,3,-1");
    assert_eq!(o.len(), 1);
    assert_eq!(o["Map_Arena_Pit"].center, [10.0, 20.0, 30.0]);
    let z = Zone { center: [0.0, 0.0, 0.0], radius_cm: 300.0, half_height_cm: 300.0 };
    assert!(z.contains([299.0, 0.0, 250.0]) && !z.contains([301.0, 0.0, 0.0]) && !z.contains([0.0, 0.0, 400.0]));
}

#[test]
fn the_side_alone_on_the_hill_scores_contested_scores_nothing() {
    let st = server(9, 3, all_caps());
    let mut i = st.inner.try_lock().unwrap();
    set_mode(&mut i, ModeCfg { mode: gm::KING_OF_HILL, koth_target_s: 10, ..Default::default() });
    go_live(&mut i);
    let z = i.modes.zone.unwrap();
    let (a, b, c) = (id(9, 1), id(9, 2), id(9, 3));
    let far = [z.center[0] + 5000.0, z.center[1], z.center[2]];
    for p in [a, b, c] { pos(&mut i, p, far); }
    pos(&mut i, a, z.center);
    run(&mut i, 4000);
    let score = |i: &Inner, k: u32| i.modes.stats[&key(k)].score_ms;
    assert!((3900..=4000).contains(&score(&i, 1)), "{}", score(&i, 1));
    assert_eq!(i.modes.hold.holder, Some(Side::Player(a)));
    // Contested: nobody scores.
    pos(&mut i, b, z.center);
    run(&mut i, 3000);
    assert!(i.modes.hold.contested && i.modes.hold.holder.is_none());
    assert!(score(&i, 1) <= 4000 && score(&i, 2) == 0);
    // The zone record shows it to ZONE clients.
    let rec = zone_record(&i).unwrap();
    let (_, p) = wire::split(&rec).unwrap();
    let zr = view::<rec::ZoneState>(p).unwrap().head;
    assert!(zr.contested.get() && zr.inside == 2 && zr.holder_seat == rec::NO_SEAT);
    // b leaves the hill: a reaches 10 s and wins the round.
    pos(&mut i, b, far);
    run(&mut i, 6500);
    assert_eq!(i.match_state, "roundover");
    run(&mut i, SETTLE_MS + 50);
    assert_eq!((i.last_winner, wins(&i, a)), (a, 1));
    assert_eq!(i.modes.result, mode_result::OBJECTIVE);
    let _ = c;
}

#[test]
fn hill_clock_decides_by_points_and_deaths_still_eliminate() {
    let st = server(10, 2, all_caps());
    let mut i = st.inner.try_lock().unwrap();
    set_mode(&mut i, ModeCfg { mode: gm::KING_OF_HILL, round_time_s: 5, koth_target_s: 60, ..Default::default() });
    go_live(&mut i);
    let z = i.modes.zone.unwrap();
    let (a, b) = (id(10, 1), id(10, 2));
    pos(&mut i, a, [z.center[0] + 5000.0, z.center[1], z.center[2]]);
    pos(&mut i, b, z.center);
    run(&mut i, 5200);
    run(&mut i, SETTLE_MS + 50);
    assert_eq!(i.last_winner, b, "most points at the clock");
    assert_eq!(i.modes.result, mode_result::TIME_LIMIT);
    assert_eq!(i.match_reason, "time_limit");
    assert_eq!(session::build_session(&i, 0).head.result_reason, v5::ResultReason::TIME_LIMIT);
    // Next round: a kill ends it as in any elimination round.
    run(&mut i, ROUNDOVER_MS + NEXT_ROUND_COUNTDOWN_MS + 200);
    assert_eq!(i.match_state, "live");
    assert!(declare_death(&mut i, b, a, DEATH_DAMAGE));
    run(&mut i, SETTLE_MS + 50);
    assert_eq!((i.last_winner, i.modes.result), (a, mode_result::ELIMINATION));
}

#[test]
fn team_hill_points_belong_to_the_team() {
    let st = server(11, 4, all_caps());
    let mut i = st.inner.try_lock().unwrap();
    set_mode(&mut i, ModeCfg { mode: gm::KING_OF_HILL, team_rule: v5::TeamRule::AUTO, koth_target_s: 10, ..Default::default() });
    go_live(&mut i);
    let z = i.modes.zone.unwrap();
    let far = [z.center[0] + 5000.0, z.center[1], z.center[2]];
    for k in 1..=4 { pos(&mut i, id(11, k), far); }
    // Red (seats 1 and 3) take turns on the hill: the points add up for Red.
    pos(&mut i, id(11, 1), z.center);
    run(&mut i, 6000);
    pos(&mut i, id(11, 1), far);
    pos(&mut i, id(11, 3), z.center);
    run(&mut i, 4200);
    run(&mut i, SETTLE_MS + 50);
    assert_eq!(i.modes.winner_team, 1);
    assert_eq!(i.modes.team_wins[0], 1);
}

#[test]
fn no_round_clock_means_no_time_limit() {
    let st = server(12, 2, 0);
    let mut i = st.inner.try_lock().unwrap();
    go_live(&mut i);
    assert!(i.modes.clock_ms.is_none());
    run(&mut i, 600_000);
    assert_eq!(i.match_state, "live", "a duel without a round clock never times out");
}

#[test]
fn an_elimination_round_clock_ends_on_most_standing() {
    let st = server(13, 3, 0);
    let mut i = st.inner.try_lock().unwrap();
    set_mode(&mut i, ModeCfg { mode: gm::FFA, round_time_s: 30, ..Default::default() });
    go_live(&mut i);
    run(&mut i, 30_100);
    run(&mut i, SETTLE_MS + 50);
    assert_eq!(i.match_reason, "draw", "three standing players: a draw");
    assert_eq!(i.modes.result, mode_result::DRAW);
}

// ---- roulette / brawl ----------------------------------------------------------------------

#[test]
fn roulette_kit_is_shared_deterministic_and_from_the_catalogue() {
    use crate::loadout::catalog::{self, Kind};
    let mut seen = HashSet::new();
    for round in 1..=60 {
        let k = roulette_kit(0xABCD, round);
        assert_eq!(k, roulette_kit(0xABCD, round), "same match + round, same kit");
        let w = catalog::item(k.r).expect("a catalogue weapon");
        assert!(w.kind == Kind::Weapon && w.group != "shield");
        assert!(k.l.is_empty());
        assert!(k.armor.iter().all(|a| catalog::item(a).is_some_and(|i| i.kind == Kind::Armor)));
        // One armour piece per slot group (Set Up Armor takes one per slot).
        let mut groups: Vec<&str> = k.armor.iter().map(|a| catalog::item(a).unwrap().group).collect();
        groups.sort_unstable();
        let n = groups.len();
        groups.dedup();
        assert_eq!(groups.len(), n, "{:?}", k.armor);
        assert_eq!(k.selection().r(), k.r);
        seen.insert(k.r);
    }
    assert!(seen.len() > 20, "the weapon varies by round ({} seen)", seen.len());
    let b = brawl_kit();
    assert!(b.r.is_empty() && b.l.is_empty() && b.armor.iter().all(|a| catalog::item(a).unwrap().cost == 0));
    assert_eq!(crate::validate::damage::class_of(b.r), crate::validate::damage::WeaponClass::Unarmed);
}

#[test]
fn the_round_kit_changes_each_countdown_and_is_lifted_in_the_lobby() {
    let st = server(14, 2, all_caps());
    let mut i = st.inner.try_lock().unwrap();
    set_mode(&mut i, ModeCfg { mode: gm::ROULETTE, ..Default::default() });
    let g0 = i.modes.kit_gen;
    go_live(&mut i);
    let k1 = i.modes.kit.clone().expect("a roulette kit");
    assert_eq!(k1, roulette_kit(i.sess.match_id, 1));
    assert_ne!(i.modes.kit_gen, g0);
    let (h, _) = mode_record(&i, 0);
    assert_eq!(h.kit_r.as_str(), Some(k1.r));
    assert!(declare_death(&mut i, id(14, 2), id(14, 1), DEATH_DAMAGE));
    run(&mut i, SETTLE_MS + ROUNDOVER_MS + 200);
    assert_eq!(i.match_state, "countdown");
    assert_eq!(i.modes.kit, Some(roulette_kit(i.sess.match_id, 2)));
    let g = i.modes.kit_gen;
    reset_to_lobby(&mut i);
    assert!(i.modes.kit.is_none() && i.modes.kit_gen != g, "the lobby lifts the kit");
    // Brawl: fists for everyone.
    set_mode(&mut i, ModeCfg { mode: gm::BRAWL, ..Default::default() });
    go_live(&mut i);
    assert_eq!(i.modes.kit, Some(brawl_kit()));
}

// ---- deathmatch ----------------------------------------------------------------------------

#[test]
fn deathmatch_respawns_on_the_clients_placement_report() {
    let st = server(15, 2, all_caps());
    let mut i = st.inner.try_lock().unwrap();
    set_mode(&mut i, ModeCfg { mode: gm::DEATHMATCH, round_time_s: 120, respawn_s: 2, ..Default::default() });
    go_live(&mut i);
    let (a, b) = (id(15, 1), id(15, 2));
    // b's game reports (a real client): its respawn waits for its placement.
    let now = i.now_ms;
    let m = i.match_peers.entry(b).or_default();
    m.aware = true;
    m.loaded_round = 1;
    m.last_ping_ms = now;
    assert!(declare_death(&mut i, b, a, DEATH_DAMAGE));
    assert_eq!(i.match_state, "live", "a deathmatch death never ends the round");
    assert!(!alive(&i, b));
    run(&mut i, 1000);
    assert_eq!(i.modes.respawns[&b].spawn_id, 0, "no order before the delay");
    run(&mut i, 1100);
    let order = i.modes.respawns[&b].spawn_id;
    assert_eq!(order, respawn_spawn_id(1, 2));
    assert_eq!(order >> 8, 1, "clients read spawn_id >> 8 as the round");
    let plan = *i.spawn_plan.iter().find(|s| s.peer_id == b).unwrap();
    assert_eq!(plan.spawn_id, order, "the roster carries the respawn order");
    let row = session::build_session(&i, 0).rows.into_iter().find(|r| r.peer_id == b).unwrap();
    assert_eq!(row.spawn_id, order);
    // The game reloads for 30 s (no pings): it does not count as gone.
    run(&mut i, 30_000);
    assert_eq!(i.match_state, "live");
    assert!(!alive(&i, b));
    // The placement report revives it, with a short protection.
    let gs = rec::GameStatus { match_id: i.sess.match_id, round: 1, life: 2, flags: v5::status_flags::LOADED, spawn_id: order,
        arena: hsmp_ipc::layout::Str::new("Map_Arena_Pit"), ..Default::default() };
    for bad in [
        rec::GameStatus { flags: 0, ..gs },
        rec::GameStatus { life: 0, ..gs },
        rec::GameStatus { life: 1, ..gs },
        rec::GameStatus { flags: v5::status_flags::LOADED | v5::status_flags::DEAD, ..gs },
        rec::GameStatus { load_error: 1, ..gs },
        rec::GameStatus { match_id: gs.match_id + 1, ..gs },
        rec::GameStatus { round: 2, ..gs },
        rec::GameStatus { spawn_id: order - 1, ..gs },
        rec::GameStatus { arena: hsmp_ipc::layout::Str::new("Map_Arena_Yard"), ..gs },
        rec::GameStatus { arena: Default::default(), ..gs },
    ] {
        game_status_in_rec(&mut i, b, &bad);
        assert!(!alive(&i, b), "unverified status cannot revive a native pawn");
    }
    let died = game_status_in_rec(&mut i, b, &gs);
    assert!(!died);
    assert!(alive(&i, b));
    assert!(i.round_deaths.iter().all(|d| d.0 != b), "the old death is not repeated");
    assert_eq!(hit_refusal(&i, a, b), Some("respawn protection"));
    assert!(!declare_death(&mut i, b, a, DEATH_DAMAGE), "no death inside the protection");
    run(&mut i, RESPAWN_PROTECT_MS + 50);
    let stale_dead = rec::GameStatus { flags: v5::status_flags::LOADED | v5::status_flags::DEAD, ..gs };
    assert!(!game_status_in_rec(&mut i, b, &stale_dead), "unscoped DEAD is diagnostic only, even after protection");
    assert!(alive(&i, b), "an old corpse's ping cannot kill the new life");
    assert_eq!(hit_refusal(&i, a, b), None);
    let s = i.modes.stats[&key(2)];
    assert_eq!((s.life, s.deaths), (2, 1));
    assert_eq!(i.modes.stats[&key(1)].round_kills, 1);
}

/// What `on_game_status` does under the lock for a `game_status` record.
fn game_status_in_rec(i: &mut Inner, pid: PeerId, gs: &rec::GameStatus) -> bool {
    let loaded = if game_status_placed(i, pid, gs) { gs.round } else { 0 };
    game_status_in(i, pid, true, loaded, None, gs.flags & v5::status_flags::DEAD != 0)
}

#[test]
fn body_snapshot_authentication_requires_the_current_accepted_pose_generation() {
    let st = server(43, 2, all_caps());
    let mut i = st.inner.try_lock().unwrap();
    go_live(&mut i);
    let b = id(43, 2);
    let m = i.sess.match_id;
    assert!(!body_context_matches(&i,b,m,1,1), "a snapshot cannot authenticate ahead of its pose");
    let f = crate::posecodec::v2::Full { context:Some(crate::posecodec::v2::Context{match_id:m,round:1,life:1}),
        ts:100.0,..Default::default() };
    crate::lagcomp::record_skeletal_v2(b,&f);
    assert!(!body_context_matches(&i,b,m,1,1), "an uncovered first pose cannot authenticate body geometry");
    crate::lagcomp::record_root(b,100,[0.0;3]);
    assert!(crate::lagcomp::record_skeletal_v2(b,&f));
    assert!(body_context_matches(&i,b,m,1,1));
    assert!(!body_context_matches(&i,b,m+1,1,1));
    assert!(!body_context_matches(&i,b,m,2,1));
    assert!(!body_context_matches(&i,b,m,1,2));
    i.modes.stats.get_mut(&key(2)).unwrap().life = 2;
    assert!(!body_context_matches(&i,b,m,1,1), "previous-life accepted pose cannot authenticate a current body");
    assert!(!body_context_matches(&i,b,m,1,2), "new life also waits for its own pose");
}

#[test]
fn countdown_placement_uses_fresh_life_one_instead_of_the_previous_round_life() {
    let st = server(42, 2, all_caps());
    let mut i = st.inner.try_lock().unwrap();
    assert!(session::start_match(&mut i, true, "test").ok);
    let b = id(42, 2);
    i.modes.stats.entry(key(2)).or_default().life = 130;
    let order = i.spawn_plan.iter().find(|s| s.peer_id == b).unwrap().spawn_id;
    let fresh = rec::GameStatus { match_id: i.sess.match_id, round: i.match_round + 1, life: 1,
        flags: v5::status_flags::LOADED, spawn_id: order,
        arena: hsmp_ipc::layout::Str::new("Map_Arena_Pit"), ..Default::default() };
    let failure = rec::GameStatus { flags: 0, load_error: 1, ..fresh };
    game_status_load_error(&mut i, b, &rec::GameStatus { round: 0, ..failure });
    game_status_load_error(&mut i, b, &rec::GameStatus { match_id: fresh.match_id + 1, ..failure });
    assert!(!i.sess.load_errors.contains_key(&b), "old round/match failures cannot start a pending load retry timer");
    game_status_load_error(&mut i, b, &failure);
    assert_eq!(i.sess.load_errors[&b].0, fresh.round);
    assert!(!game_status_placed(&mut i, b, &rec::GameStatus { life: 130, ..fresh }));
    assert!(game_status_placed(&mut i, b, &fresh));
    game_status_in_rec(&mut i, b, &fresh);
    assert_eq!(i.match_peers[&b].loaded_round, fresh.round);
}

#[test]
fn deathmatch_wrapped_spawn_id_still_requires_the_full_original_life() {
    let st = server(41, 2, all_caps());
    let mut i = st.inner.try_lock().unwrap();
    set_mode(&mut i, ModeCfg { mode: gm::DEATHMATCH, respawn_s: 1, ..Default::default() });
    go_live(&mut i);
    let (a, b) = (id(41, 1), id(41, 2));
    i.modes.stats.get_mut(&key(2)).unwrap().life = 129;
    i.match_peers.entry(b).or_default().aware = true;
    assert!(declare_death(&mut i, b, a, DEATH_DAMAGE));
    run(&mut i, 1100);
    let order = i.modes.respawns[&b].spawn_id;
    assert_eq!(order, respawn_spawn_id(1, 2));
    let old = rec::GameStatus { match_id: i.sess.match_id, round: 1, life: 2,
        flags: v5::status_flags::LOADED, spawn_id: order,
        arena: hsmp_ipc::layout::Str::new("Map_Arena_Pit"), ..Default::default() };
    assert!(!game_status_placed(&mut i, b, &old));
    assert!(!alive(&i, b));
    assert!(game_status_placed(&mut i, b, &rec::GameStatus { life: 130, ..old }));
    assert!(alive(&i, b));
}

#[test]
fn deathmatch_life_exhaustion_never_reuses_the_last_native_generation() {
    let st = server(40, 2, all_caps());
    let mut i = st.inner.try_lock().unwrap();
    set_mode(&mut i, ModeCfg { mode: gm::DEATHMATCH, respawn_s: 1, ..Default::default() });
    go_live(&mut i);
    let (a, b) = (id(40, 1), id(40, 2));
    i.modes.stats.get_mut(&key(2)).unwrap().life = u16::MAX;
    assert!(declare_death(&mut i, b, a, DEATH_DAMAGE));
    run(&mut i, 1100);
    assert!(!alive(&i, b));
    assert!(!i.modes.respawns.contains_key(&b));
    assert_eq!(peer_life(&i, b), u16::MAX);
}

#[test]
fn deathmatch_clock_most_kills_wins_and_a_tie_goes_to_sudden_death() {
    let st = server(18, 3, all_caps());
    let mut i = st.inner.try_lock().unwrap();
    set_mode(&mut i, ModeCfg { mode: gm::DEATHMATCH, round_time_s: 60, respawn_s: 1, ..Default::default() });
    i.best_of = 3;
    go_live(&mut i);
    let (a, b, c) = (id(18, 1), id(18, 2), id(18, 3));
    // Headless clients (no game reports): respawned as soon as ordered.
    assert!(declare_death(&mut i, b, a, DEATH_DAMAGE));
    run(&mut i, 1100);
    assert!(alive(&i, b), "respawned");
    assert!(declare_death(&mut i, c, a, DEATH_DAMAGE));
    run(&mut i, 60_000);
    run(&mut i, SETTLE_MS + 100);
    assert_eq!((i.last_winner, i.modes.result), (a, mode_result::KILLS));
    assert_eq!(i.match_reason, "time_limit");
    // Round 2: tied at the clock -> sudden death; b's kill decides it.
    run(&mut i, ROUNDOVER_MS + NEXT_ROUND_COUNTDOWN_MS + 200);
    assert_eq!(i.match_state, "live");
    assert_eq!(i.modes.stats[&key(1)].round_kills, 0, "kills this round start over");
    assert_eq!(i.modes.stats[&key(1)].kills, 2, "match kills are kept");
    assert!(declare_death(&mut i, c, a, DEATH_DAMAGE));
    assert!(declare_death(&mut i, a, b, DEATH_DAMAGE));
    run(&mut i, 60_000);
    assert_eq!(i.match_state, "live", "tied: sudden death");
    assert!(i.modes.sudden_death.is_some());
    let (h, _) = mode_record(&i, i.now_ms);
    assert!(h.sudden_death.get() && h.round_end_ms > i.now_ms);
    run(&mut i, 3000);
    assert!(declare_death(&mut i, c, b, DEATH_DAMAGE));
    run(&mut i, 100);
    run(&mut i, SETTLE_MS + 50);
    assert_eq!((i.last_winner, i.modes.result), (b, mode_result::SUDDEN_DEATH));
    // Round 3: nobody kills in sudden death -> a draw.
    run(&mut i, ROUNDOVER_MS + NEXT_ROUND_COUNTDOWN_MS + 200);
    run(&mut i, 60_000 + SUDDEN_DEATH_MS + 100);
    run(&mut i, SETTLE_MS + 50);
    assert_eq!((i.match_reason.as_str(), i.modes.result), ("draw", mode_result::DRAW));
}

#[test]
fn respawn_order_points_away_from_the_living() {
    let t = crate::spawns::table("Map_Arena_Pit").unwrap();
    let pts: Vec<[f32; 3]> = t.points.iter().map(|&p| t.all[p].pos).collect();
    let (_, p, _) = crate::spawns::respawn_point("Map_Arena_Pit", &[pts[0]]).unwrap();
    let far = pts.iter().map(|q| ((q[0] - pts[0][0]).powi(2) + (q[1] - pts[0][1]).powi(2)).sqrt()).fold(0.0f32, f32::max);
    let d = ((p[0] - pts[0][0]).powi(2) + (p[1] - pts[0][1]).powi(2)).sqrt();
    assert!(d >= far - 1.0, "the farthest point from the living ({d} of {far})");
    assert_eq!(crate::spawns::respawn_point("Map_Arena_Pit", &[pts[0]]), crate::spawns::respawn_point("Map_Arena_Pit", &[pts[0]]));
    assert!(crate::spawns::respawn_point("nowhere", &[]).is_none());
}

// ---- records -------------------------------------------------------------------------------

#[test]
fn mode_records_go_to_capable_peers_on_change_and_periodically() {
    let st = server(16, 3, 0);
    let mut i = st.inner.try_lock().unwrap();
    crate::interact::note_caps(id(16, 1), caps::MODES | caps::ZONE);
    crate::interact::note_caps(id(16, 2), caps::MODES);
    set_mode(&mut i, ModeCfg { mode: gm::KING_OF_HILL, ..Default::default() });
    go_live(&mut i);
    let mut out = Vec::new();
    i.modes.dirty = true;
    let t0 = i.now_ms;
    records_due(&mut i, t0 + MODE_EVERY_MS, &mut out);
    let kinds: Vec<(SocketAddr, u16)> = out.iter().map(|(a, m)| (a.unwrap(), wire::split(m).unwrap().0.kind)).collect();
    assert!(kinds.contains(&(addr_of(id(16, 1)), rec::K_MODE)) && kinds.contains(&(addr_of(id(16, 1)), rec::K_ZONE)));
    assert!(kinds.contains(&(addr_of(id(16, 2)), rec::K_MODE)) && !kinds.contains(&(addr_of(id(16, 2)), rec::K_ZONE)));
    assert!(kinds.iter().all(|(a, _)| *a != addr_of(id(16, 3))), "no mode records for an old client");
    // Nothing new right after; a change goes out after the minimum gap.
    out.clear();
    let t = t0 + MODE_EVERY_MS + 10;
    records_due(&mut i, t, &mut out);
    assert!(out.is_empty());
    i.modes.dirty = true;
    records_due(&mut i, t + MODE_MIN_GAP_MS, &mut out);
    assert!(!out.is_empty());
    // The record's rows: seat order, kills / deaths, the hill.
    let (_, p) = wire::split(&out[0].1).unwrap();
    let v = view::<rec::ModeHead>(p).unwrap();
    assert_eq!((v.head.mode, v.head.n as usize, v.head.round_time_s), (gm::KING_OF_HILL, 3, KOTH_TIME_S));
    assert_eq!(v.rows.iter().map(|r| r.seat).collect::<Vec<_>>(), vec![1, 2, 3]);
    assert!(v.head.round_end_ms > 0, "the round clock is running");
}

#[test]
fn no_mode_records_without_capable_peers() {
    let st = server(17, 2, 0);
    let mut i = st.inner.try_lock().unwrap();
    go_live(&mut i);
    let mut out = Vec::new();
    i.modes.dirty = true;
    let t = i.now_ms + 10 * MODE_EVERY_MS;
    records_due(&mut i, t, &mut out);
    assert!(out.is_empty());
}
