//! The standard scenario suite (shared by the G0 test and the CLI).

use super::net::Profile;
use super::report::Stats;
use super::script::Mix;
use super::world::*;

/// Fight shapes per seed: (players, duration ms).
pub const SHAPES: &[(usize, f64)] = &[(2, 40_000.0), (4, 30_000.0), (8, 20_000.0), (3, 30_000.0)];

pub fn profile(name: &str) -> Profile {
    if name == "loopback" { Profile::loopback() } else { Profile::get(name) }
}

/// Honest fights on `profile`: `seeds` × every shape.
pub fn honest(name: &str, seeds: u64, v2: bool, policy: Policy) -> Stats {
    honest_with(name, seeds, v2, policy, None)
}

/// `honest` with an explicit stand-in tracking error (uu, 1σ).
pub fn honest_with(name: &str, seeds: u64, v2: bool, policy: Policy, servo: Option<f32>) -> Stats {
    honest_cfg(name, seeds, &|cfg| {
        cfg.stream_v2 = v2;
        cfg.policy = policy;
        if let Some(n) = servo { cfg.servo_noise = n; }
    })
}

/// Honest fights on `profile` with every config changed by `tweak`.
pub fn honest_cfg(name: &str, seeds: u64, tweak: &dyn Fn(&mut Config)) -> Stats {
    let mut s = Stats::new(name);
    for seed in 0..seeds {
        for (k, &(n, dur)) in SHAPES.iter().enumerate() {
            let mut cfg = Config::new(profile(name), n, 1000 * seed + k as u64 + 17);
            cfg.dur_ms = dur;
            tweak(&mut cfg);
            let mut w = World::new(cfg);
            w.run();
            s.add(&w);
        }
    }
    s
}

/// One cheater per fight (4 players: cheater, its victim and two honest),
/// every cheat kind, `seeds` fights each. Returns the folded stats (cheat
/// rows in `Stats::cheat`).
pub fn cheats(name: &str, seeds: u64, policy: Policy) -> Stats {
    cheats_cfg(name, seeds, policy, &|_| {})
}

/// `cheats` with every config changed by `tweak`.
pub fn cheats_cfg(name: &str, seeds: u64, policy: Policy, tweak: &dyn Fn(&mut Config)) -> Stats {
    let mut s = Stats::new(name);
    for (ci, &ch) in ALL_CHEATS.iter().enumerate() {
        // A backtrack only lies when the victim moved: give it retreats and
        // more fights to find them.
        // God mode only lies once its ignored damage is lethal: more fights.
        let (runs, mix) = match ch {
            Cheat::Backtrack => (seeds * 4, Mix::only(super::script::Kind::Clean)),
            Cheat::GodMode => (seeds * 6, Mix::standard()),
            _ => (seeds, Mix::standard()),
        };
        for seed in 0..runs {
            let mut cfg = Config::new(profile(name), 4, 77_000 + 100 * ci as u64 + seed);
            cfg.dur_ms = 30_000.0;
            cfg.policy = policy;
            cfg.mix = mix;
            // God mode needs the victims' own game and the vitals path.
            cfg.health_model = ch == Cheat::GodMode;
            // Two cheaters of the same kind (one per pair).
            cfg.cheats = vec![(0, ch), (2, ch)];
            tweak(&mut cfg);
            let mut w = World::new(cfg);
            w.run();
            s.add(&w);
        }
    }
    s
}

/// Distribution of the server's parry distances (`Info::parry_d`) for honest
/// claims through a parry some screen showed vs all others (tuning data for
/// lagcomp::PARRY_* thresholds). Returns (parried, others) per component.
pub fn parry_distances(name: &str, seeds: u64) -> ([Vec<f32>; 3], [Vec<f32>; 3]) {
    let mut par: [Vec<f32>; 3] = Default::default();
    let mut oth: [Vec<f32>; 3] = Default::default();
    for seed in 0..seeds {
        for (k, &(n, dur)) in SHAPES.iter().enumerate() {
            let mut cfg = Config::new(profile(name), n, 1000 * seed + k as u64 + 17);
            cfg.dur_ms = dur;
            let mut w = World::new(cfg);
            w.run();
            for r in &w.claims {
                if r.kind != ClaimKind::Honest { continue; }
                let Some(d) = r.parry_d else { continue };
                let p = super::report::parried_truth(&w, r);
                for i in 0..3 { if p { par[i].push(d[i]) } else { oth[i].push(d[i]) } }
            }
        }
    }
    (par, oth)
}

/// End-to-end health replication (`Config::health_model`): `seeds` × every
/// shape on `name`, optionally with a stalled owner (fighter 1 from 6 s to
/// 16 s) or the echo-leak fault injected.
pub fn health(name: &str, seeds: u64, v2: bool, stall: bool, echo_leak: bool) -> health::HealthStats {
    let mut s = health::HealthStats::default();
    for seed in 0..seeds {
        for (k, &(n, dur)) in SHAPES.iter().enumerate() {
            let mut cfg = Config::new(profile(name), n, 5000 * seed + k as u64 + 23);
            cfg.dur_ms = dur;
            cfg.stream_v2 = v2;
            cfg.health_model = true;
            cfg.echo_leak = echo_leak;
            if stall { cfg.stall = Some((1, 6_000.0, 16_000.0)); }
            let mut w = World::new(cfg);
            w.run();
            s.add(&w);
        }
    }
    s
}

pub fn health_table(label: &str, h: &health::HealthStats) -> String {
    use super::report::quant;
    let r = &h.n_hits_ratio;
    format!("| {label} | {} | {} | {:.0} | {:.3} / {:.3} / {:.3} | {:.0} / {:.0} / {:.0} | {} | {} | {} / {} | {} | {} | {} | {:.0} / {:.0} |",
        h.applies, h.dup_applies, h.echo_damage,
        quant(r, 0.1), quant(r, 0.5), quant(r, 0.9),
        quant(&h.view_stale, 0.95), quant(&h.view_stale, 0.99), quant(&h.view_stale, 1.0),
        h.view_wrong, h.ledger_cuts, h.server_deaths, h.native_deaths, h.false_deaths, h.missed_deaths, h.stall_deaths,
        quant(&h.death_latency, 0.5), quant(&h.death_latency, 1.0))
}

pub const HEALTH_HEADER: &str = "| scenario | replays | double applied | HUD HP lost beyond replays | HP lost after 5 hits MP/solo p10/p50/p90 | viewer behind victim HUD ms p95/p99/max | viewer showed a value never held | ledger cut frames | server / native deaths | false deaths | missed deaths | stall-rule deaths | death declared after native p50/max ms |\n|---|---|---|---|---|---|---|---|---|---|---|---|---|";

/// Hit location across the replay delay on `profile` (honest fights, every shape).
pub fn location(name: &str, seeds: u64) -> super::frame::LocParity {
    let mut o = super::frame::LocParity::default();
    for seed in 0..seeds {
        for (k, &(n, dur)) in SHAPES.iter().enumerate() {
            let mut cfg = Config::new(profile(name), n, 1000 * seed + k as u64 + 17);
            cfg.dur_ms = dur;
            let mut w = World::new(cfg);
            w.run();
            o.add(&super::frame::location_parity(&w));
        }
    }
    o
}
