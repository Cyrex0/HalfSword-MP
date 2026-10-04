//! G0: `cargo test --manifest-path crates/hsmp-combat-sim/Cargo.toml`.
//!
//! Drives the real server combat code with simulated fights and asserts the
//! combat quality targets:
//!   honest-hit acceptance ≥ 99 % loopback, ≥ 97 % typical, ≥ 93 % wifi;
//!   every cheat's false-accept ≤ 0.5 %;
//!   parries some screen showed are cancelled, honest hits are not;
//!   real trades (two blows within 100 ms) both land;
//!   no accepted hit judged against a victim pose older than the cap.
//! Run with `-- --nocapture` for the report tables.

use hsmp_combat_sim::sim::report::{cheat_table, quant, table, Stats};
use hsmp_combat_sim::sim::suite;
use hsmp_combat_sim::sim::world::{Policy, ALL_CHEATS};

const SEEDS: u64 = 3;

fn honest(name: &str) -> Stats {
    let s = suite::honest(name, SEEDS, true, Policy::Dedupe);
    println!("{}", table(std::slice::from_ref(&s)));
    s
}

fn common(s: &Stats, min_accept: f64) {
    assert!(s.honest >= 300, "{}: too few honest claims ({})", s.label, s.honest);
    assert!(s.accept_pct() >= min_accept, "{}: honest acceptance {:.2} % < {} % (rejects: {})",
        s.label, s.accept_pct(), min_accept, s.top_reasons(6));
    // The live-play regression: the game's own bookkeeping deltas (Last Damage
    // Taken = DRS) must never reject a hit again.
    assert!(!s.reasons.contains_key("bad_field"), "{}: bad_field rejects {:?}", s.label, s.reasons);
    assert!(s.false_cancel_pct() <= 0.5, "{}: honest hits cancelled as parried {:.2} %", s.label, s.false_cancel_pct());
    assert!(s.parry_claims == 0 || s.parry_cancel_pct() >= 90.0,
        "{}: only {:.1} % of hits through a shown parry cancelled", s.label, s.parry_cancel_pct());
    assert!(s.trades == 0 || s.trade_pct() >= 90.0, "{}: real trades mutual {:.0} %", s.label, s.trade_pct());
    assert!(s.claims_per_contact() <= 1.5, "{}: {:.2} claims per contact", s.label, s.claims_per_contact());
    // Fairness: the rewind cap (300 ms) plus the view-time tolerance.
    let lag_max = quant(&s.view_lag, 1.0);
    assert!(lag_max <= 300.0 + 60.0, "{}: accepted hit on a victim pose {:.0} ms old", s.label, lag_max);
    let clamped = 100.0 * s.clamped_honest as f64 / s.honest_ok.max(1) as f64;
    assert!(clamped <= 3.0, "{}: {:.1} % of honest hits had their damage clamped", s.label, clamped);
}

#[test]
#[ignore = "slow: cargo test -- --ignored"]
fn honest_acceptance_loopback() { common(&honest("loopback"), 99.0); }

#[test]
#[ignore = "slow: cargo test -- --ignored"]
fn honest_acceptance_typical() { common(&honest("typical"), 97.0); }

#[test]
#[ignore = "slow: cargo test -- --ignored"]
fn honest_acceptance_wifi() { common(&honest("wifi"), 93.0); }

#[test]
#[ignore = "slow: cargo test -- --ignored"]
fn cheats_rejected() {
    let s = suite::cheats("typical", SEEDS, Policy::Dedupe);
    let mut rows = Vec::new();
    for ch in ALL_CHEATS {
        let name = format!("{:?}", ch);
        let (n, k) = s.cheat.get(&name).copied().unwrap_or((0, 0));
        rows.push((name, n, k));
    }
    println!("{}", cheat_table(&rows));
    // Honest players in a cheater's lobby keep landing their hits.
    assert!(s.accept_pct() >= 97.0, "honest acceptance next to cheaters {:.2} % ({})", s.accept_pct(), s.top_reasons(6));
    for (name, n, k) in &rows {
        let pct = if *n == 0 { 0.0 } else { 100.0 * *k as f64 / *n as f64 };
        // ≤ 0.5 %; below 200 attempts one success is already > 0.5 %, so the
        // bound there is "at most one" (the report shows the counts).
        let ok = if *n >= 200 { pct <= 0.5 } else { *k <= 1 };
        assert!(ok, "{name}: {k}/{n} cheat claims got through ({pct:.2} %)");
    }
}

/// Until POSE codec v2 ships: the v1 stream with PhysicsHandle stand-ins
/// (6–78 uu tracking error, modelled as 20 uu σ) must still clear the bar.
#[test]
#[ignore = "slow: cargo test -- --ignored"]
fn honest_acceptance_v1_handle_standins() {
    let s = suite::honest_with("typical", SEEDS, false, Policy::Dedupe, Some(20.0));
    println!("{}", table(std::slice::from_ref(&s)));
    assert!(s.accept_pct() >= 97.0, "v1 typical: {:.2} % ({})", s.accept_pct(), s.top_reasons(6));
    assert!(!s.reasons.contains_key("bad_field"));
}

/// Solo parity (same damage as solo play): for every weapon class
/// × victim armour × body zone with enough contacts and non-trivial damage,
/// the median hits-to-kill of the victim's replay of the server-approved
/// Deal Complex Damage inputs is within ±10 % of what the same contacts did
/// in solo play (paired Monte Carlo, report::ttk).
#[test]
#[ignore = "slow: cargo test -- --ignored"]
fn damage_parity_with_solo() {
    use hsmp_combat_sim::sim::report::{parity_cell, parity_table, TTK_MAX_HITS};
    const MIN_N: usize = 20;
    let mut checked = 0;
    // (v1 = today's stream with PhysicsHandle stand-ins: the server only has the
    // weapon actor and hands, no blade sweep, so it cannot rescale the impact
    // to the real relative speed — down-only rescale + hard caps; ±35 % with 20 uu servo noise until POSE v2 ships.)
    for (p, v2, servo, tol) in [("typical", true, None, 0.10), ("wifi", true, None, 0.10), ("typical", false, Some(20.0), 0.35)] {
        let s = suite::honest_with(p, 6, v2, Policy::Dedupe, servo);
        let p = if v2 { p.to_string() } else { format!("{p} v1") };
        println!("{p}\n{}", parity_table(&s, MIN_N));
        for (k, v) in &s.parity {
            if v.len() < MIN_N { continue; }
            let (n, _, _, r, hs, hm) = parity_cell(v);
            if !hs.is_finite() { continue; }
            assert!(hs <= TTK_MAX_HITS as f64);
            checked += 1;
            assert!((1.0 - tol..=1.0 + tol).contains(&r), "{p} {k} (n={n}): hits-to-kill MP {hm:.1} vs solo {hs:.1} (×{r:.3})");
        }
    }
    assert!(checked >= 45, "only {checked} parity cells had enough samples");
}

/// End-to-end health replication (sim::world::health): victim replays, the
/// vitals stream (Lua 15 Hz + sidecar VitalsGate), the REAL server ledger,
/// deaths from the owner (reliable report / vitals) or the stall rule.
#[test]
#[ignore = "slow: cargo test -- --ignored"]
fn health_replication_end_to_end() {
    use hsmp_combat_sim::sim::report::quant;
    println!("{}", suite::HEALTH_HEADER);
    for p in ["typical", "wifi"] {
        for (label, stall) in [("honest", false), ("stalled owner", true)] {
            let h = suite::health(p, 2, true, stall, false);
            println!("{}", suite::health_table(&format!("{p} {label}"), &h));
            assert!(h.applies >= 400, "{p} {label}: only {} replays", h.applies);
            // (1) Health after 5 hits = solo (median ±5 %, p10/p90 within 20/25 %).
            let r = &h.n_hits_ratio;
            assert!(r.len() >= 20, "{p} {label}: {} lives with 5 hits", r.len());
            let (p10, p50, p90) = (quant(r, 0.1), quant(r, 0.5), quant(r, 0.9));
            assert!((0.95..=1.05).contains(&p50) && p10 >= 0.8 && p90 <= 1.25,
                "{p} {label}: HP lost after 5 hits MP/solo p10 {p10:.3} p50 {p50:.3} p90 {p90:.3}");
            // (2) Peers show the victim's HUD Health: never a value it did not
            // have, never cut by the ledger, ≤ one vitals tick + the link behind
            // (p99), and every loss healed by the repeats (max ≤ 0.5 s).
            assert_eq!(h.view_wrong, 0, "{p} {label}: viewers showed Health the victim never had");
            assert_eq!(h.ledger_cuts, 0, "{p} {label}: the ledger clamped honest vitals");
            if !stall {
                let (p99, max) = (quant(&h.view_stale, 0.99), quant(&h.view_stale, 1.0));
                assert!(p99 <= 150.0 && max <= 500.0, "{p} {label}: viewer behind the HUD p99 {p99:.0} max {max:.0} ms");
            }
            // (3) Each forwarded hit applied once; nothing else takes Health.
            assert_eq!(h.dup_applies, 0);
            assert!(h.echo_damage < 0.01, "{p} {label}: {:.1} HP lost beyond the replays", h.echo_damage);
            // (4) Deaths: only the owner's own (declared within 0.3 s) or the
            // stall rule; none missed.
            assert!(h.native_deaths >= 10, "{p} {label}: {} deaths", h.native_deaths);
            assert_eq!(h.false_deaths, 0, "{p} {label}: server deaths without a native death or a stall");
            assert_eq!(h.missed_deaths, 0, "{p} {label}: native deaths never declared");
            assert!(quant(&h.death_latency, 1.0) <= 300.0, "{p} {label}: death declared {:.0} ms after it happened", quant(&h.death_latency, 1.0));
            if stall { assert!(h.stall_deaths >= 1, "{p}: the stall rule never fired"); }
        }
    }
    // The checker sees a double application (echo not restored).
    let h = suite::health("typical", 1, true, false, true);
    assert!(h.echo_damage > 100.0, "echo-leak fault not detected ({:.1})", h.echo_damage);
}

/// Hit location survives the replay delay: the victim replays the blow on the
/// same body-relative spot the blade touched (bone-local offsets), so the same
/// armour layers apply as in solo. Re-adding a world offset, as HSMPCombat did
/// before hits were replayed in bone space, moves the spot by however much the victim turned and leaned
/// in between (often under a different layer, or deep inside the body).
#[test]
#[ignore = "slow: cargo test -- --ignored"]
fn hit_location_survives_the_replay_delay() {
    for p in ["typical", "wifi"] {
        let o = suite::location(p, SEEDS);
        let mut turn = o.turn.clone();
        turn.sort_by(|a, b| a.total_cmp(b));
        let q = |f: f64| turn.get(((turn.len() as f64 - 1.0) * f) as usize).copied().unwrap_or(0.0);
        println!("| {p} | {} blows | victim turn p50 {:.1}° p90 {:.1}° | world offset: other layer {:.1} %, inside {:.1} % | bone-local: other layer {:.2} %, inside {:.2} % |",
            o.n, q(0.5), q(0.9), o.pct(o.world_off), o.pct(o.world_inside), o.pct(o.local_off), o.pct(o.local_inside));
        assert!(o.n >= 300, "{p}: too few replayed blade blows ({})", o.n);
        assert!(o.pct(o.local_off) <= 0.5, "{p}: bone-local replay under another layer {:.2} %", o.pct(o.local_off));
        assert!(o.local_inside == 0, "{p}: bone-local replay inside the body {}", o.local_inside);
        assert!(o.world_off > 2 * o.local_off.max(1), "{p}: the world-offset baseline should be worse ({} vs {})", o.world_off, o.local_off);
    }
}

/// The server tick only schedules the held-hit flush, clash judging and the
/// ledger sweep: honest acceptance, parries, trades and cheat rejection are
/// the same at 30, 60 and 100 Hz (docs/development/tick-rate.md).
#[test]
#[ignore = "slow: cargo test -- --ignored"]
fn tick_rate_does_not_change_combat_outcomes() {
    let at = |hz: f64| {
        let t = 1000.0 / hz;
        let mut s = suite::honest_cfg("typical", SEEDS, &|c| c.server_tick_ms = t);
        s.label = format!("typical @ {hz} Hz");
        let c = suite::cheats_cfg("typical", 1, Policy::Dedupe, &|c| c.server_tick_ms = t);
        let (n, k) = c.cheat.values().fold((0, 0), |a, b| (a.0 + b.0, a.1 + b.1));
        (s, n, k)
    };
    let runs: Vec<_> = [30.0, 60.0, 100.0].into_iter().map(at).collect();
    println!("{}", table(&runs.iter().map(|r| r.0.clone()).collect::<Vec<_>>()));
    let base = runs[0].0.accept_pct();
    for (s, n, k) in &runs {
        common(s, 97.0);
        assert!((s.accept_pct() - base).abs() <= 0.5, "{}: acceptance {:.2} % vs {:.2} % at 30 Hz", s.label, s.accept_pct(), base);
        let fa = 100.0 * *k as f64 / (*n).max(1) as f64;
        assert!(fa <= 0.6, "{}: cheats false-accepted {:.2} %", s.label, fa);
    }
}
