//! World sync under latency (G0): the mixed scenario (push, chain push, carry and throw,
//! kick, a contested pickup, a hand-over while moving, a late joiner, a round reset) per
//! netsim profile, judged on what the players would see. `-- --nocapture` prints the
//! report (docs/development/subsystems/world-replication.md, "Measuring sync").

use hsmpworld_sim::{net::Profile, report, sim};

/// (profile, path p95 of moving followers cm, free-moving p95 cm, snaps per run)
const LIMITS: &[(&str, f64, f64, f64)] = &[
    ("lan", 15.0, 70.0, 0.5),
    ("typical", 20.0, 120.0, 1.0),
    ("intl", 35.0, 150.0, 1.0),
    ("far", 45.0, 260.0, 3.0),
];

fn run(profile: &str, seeds: u64) -> (report::Summary, Vec<sim::Metrics>) {
    let prof = Profile::get(profile);
    let runs: Vec<sim::Metrics> = (0..seeds).map(|s| sim::Sim::new(prof, sim::mixed(), 1000 + s, false).run()).collect();
    let s = report::summarize(profile, &runs);
    report::print(&s);
    (s, runs)
}

/// WORLD-2's limit on the paired world_track samples of moving bodies (p95 cm): the gate's
/// `world2_limit` for the same profile.
fn world2_limit(p: &str) -> f64 {
    match p { "lan" => 80.0, "typical" => 130.0, "intl" => 170.0, _ => 260.0 }
}

#[test]
fn world_sync_holds_at_every_profile() {
    for &(p, path95, free95, snaps) in LIMITS {
        let (s, runs) = run(p, 2);
        assert!(s.violations.is_empty(), "{p}: {:?}", s.violations);
        // Same world: every body at rest matches on every screen (settled, late joiner, after
        // the round reset).
        for (k, (d, a, off)) in &s.checks {
            assert!(*d <= 1.0 && *a <= 1.0 && *off == 0, "{p}: check {k}: {d:.2} cm {a:.2} deg, {off} bodies off");
        }
        let all = s.phases["all"];
        assert!(all[4] <= 0.5 && all[5] <= 0.5, "{p}: resting bodies drift apart: {:.2} cm {:.2} deg", all[4], all[5]);
        // Moving: followers trace the owner's path; flight is shown near the present.
        assert!(all[2] <= path95, "{p}: follower path p95 {:.1} cm > {path95}", all[2]);
        let free = s.phases["free"];
        assert!(free[1] <= free95, "{p}: free-moving p95 {:.1} cm > {free95}", free[1]);
        // Smooth: corrections are blended (snaps per run), ownership never fights.
        assert!(all[6] <= snaps, "{p}: {:.1} snaps per run", all[6]);
        // WORLD-2's own evidence (world_track, paired like the gate does) holds too
        let (mut mv, mut rest) = (Vec::new(), Vec::new());
        for m in &runs {
            let (a, b) = report::pair_tracks(m);
            mv.extend(a);
            rest.extend(b);
        }
        let mv95 = report::pct(&mv, 0.95);
        println!("  WORLD-2 pairing: {} moving pairs p95 {mv95:.1} cm, {} rest pairs max {:.2} cm", mv.len(), rest.len(),
                 rest.iter().cloned().fold(0.0, f64::max));
        assert!(mv.len() > 100 && rest.len() > 5, "{p}: world_track pairs: {} moving, {} rest", mv.len(), rest.len());
        assert!(mv95 <= world2_limit(p), "{p}: world_track p95 {mv95:.1} cm");
        assert!(rest.iter().all(|d| *d <= 2.0), "{p}: final rest poses differ: {:?}", rest.iter().filter(|d| **d > 2.0).collect::<Vec<_>>());
        assert_eq!(s.fights, 0, "{p}: ownership fights");
        assert!(s.flips >= 2, "{p}: the hand-over while moving did not happen ({} flips)", s.flips);
        // A contested pickup resolves within about a round trip (both pawns never keep it).
        assert!(s.conflict_ms <= 500.0, "{p}: both players held the axe for {:.0} ms", s.conflict_ms);
        // Bandwidth: well inside the 16 KB/s per-recipient world budget.
        assert!(s.down_bps <= 4.0 * 1024.0 && s.up_bps <= 2.0 * 1024.0, "{p}: {:.0} B/s down, {:.0} up", s.down_bps, s.up_bps);
    }
}
