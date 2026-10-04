//! `hsmp-combat-sim [--seeds N] [--profiles a,b,..] [--legacy] [--v1]`:
//! run the scenario suite and print the report tables (markdown).

use hsmp_combat_sim::sim::report::{cheat_table, table};
use hsmp_combat_sim::sim::suite;
use hsmp_combat_sim::sim::world::{Policy, ALL_CHEATS};

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let get = |k: &str| args.iter().position(|a| a == k).and_then(|i| args.get(i + 1)).cloned();
    let seeds: u64 = get("--seeds").and_then(|s| s.parse().ok()).unwrap_or(3);
    let profiles = get("--profiles").unwrap_or_else(|| "loopback,good,typical,wifi,intl,bad,far".into());
    let policy = if args.iter().any(|a| a == "--legacy") { Policy::Legacy } else { Policy::Dedupe };
    let v2 = !args.iter().any(|a| a == "--v1");
    if let Some(p) = get("--parrystats") {
        let (par, oth) = suite::parry_distances(&p, seeds);
        for i in 0..3 {
            let q = |v: &Vec<f32>, x: f64| hsmp_combat_sim::sim::report::quant(&v.iter().map(|y| *y as f64).collect::<Vec<_>>(), x);
            println!("component {}: parried n={} p50={:.0} p90={:.0} p99={:.0} max={:.0} | others n={} p10={:.0} p50={:.0}",
                i, par[i].len(), q(&par[i], 0.5), q(&par[i], 0.9), q(&par[i], 0.99), q(&par[i], 1.0), oth[i].len(), q(&oth[i], 0.1), q(&oth[i], 0.5));
        }
        return;
    }
    if let Some(p) = get("--health") {
        use hsmp_combat_sim::sim::suite;
        println!("{}", suite::HEALTH_HEADER);
        for (label, stall, leak) in [("honest", false, false), ("stalled owner", true, false), ("echo leak (fault)", false, true)] {
            let h = suite::health(&p, seeds, v2, stall, leak);
            println!("{}", suite::health_table(&format!("{p} {label}"), &h));
        }
        return;
    }
    if let Some(p) = get("--debug") {
        let np: usize = get("--players").and_then(|s| s.parse().ok()).unwrap_or(2);
        let seed: u64 = get("--seed").and_then(|s| s.parse().ok()).unwrap_or(17);
        let mut cfg = hsmp_combat_sim::sim::world::Config::new(suite::profile(&p), np, seed);
        cfg.health_model = std::env::var("SIM_HEALTH").is_ok();
        cfg.policy = policy;
        cfg.stream_v2 = v2;
        if let Some(c) = get("--cheat") {
            let ch = hsmp_combat_sim::sim::world::ALL_CHEATS.iter().find(|x| format!("{:?}", x) == c).copied().expect("cheat");
            cfg.cheats = vec![(0, ch), (2, ch)];
        }
        let mut w = hsmp_combat_sim::sim::world::World::new(cfg);
        w.run();
        debug_dump(&w, 60);
        debug_parried(&w, 30);
        debug_cheats(&w, 40);
        debug_clamped(&w, 30);
        debug_parry_miss(&w, 30);
        debug_counters(&w);
        debug_rel(&w);
        if std::env::var("SIM_HEALTH").is_ok() { debug_health(&w); }
        if let Ok(c) = std::env::var("SIM_CELL") { debug_cell(&w, &c); }
        if args.iter().any(|a| a == "--ok") { debug_ok(&w, 25); }
        return;
    }
    let mut rows = Vec::new();
    for p in profiles.split(',') {
        let t = std::time::Instant::now();
        let s = suite::honest(p, seeds, v2, policy);
        eprintln!("{p}: {:.1} s", t.elapsed().as_secs_f64());
        rows.push(s);
    }
    println!("{}", table(&rows));
    if args.iter().any(|a| a == "--parity") {
        for r in &rows { println!("{}
{}", r.label, hsmp_combat_sim::sim::report::parity_table(r, 10)); }
    }
    if !args.iter().any(|a| a == "--no-cheats") {
        let c = suite::cheats("typical", seeds, policy);
        let mut cr = Vec::new();
        for ch in ALL_CHEATS {
            let name = format!("{:?}", ch);
            let (n, k) = c.cheat.get(&name).copied().unwrap_or((0, 0));
            cr.push((name, n, k));
        }
        println!("{}", cheat_table(&cr));
        println!("{}", table(&[c]));
    }
}

#[allow(dead_code)]
pub fn debug_dump(w: &hsmp_combat_sim::sim::world::World, max: usize) {
    use hsmp_combat_sim::sim::world::ClaimKind;
    let mut k = 0;
    for r in &w.claims {
        if r.kind != ClaimKind::Honest { continue; }
        if let Some((true, _)) = r.outcome { continue; }
        k += 1;
        if k > max { break; }
        eprintln!("a{} -> v{} t={:.0} lag={:.0} eng={:?} hand={} n={} out={:?}", r.attacker, r.target, r.t_hit, r.view_lag, r.eng.map(|e| e.1), r.hand, r.n_events, r.outcome);
    }
}

#[allow(dead_code)]
pub fn debug_ok(w: &hsmp_combat_sim::sim::world::World, max: usize) {
    use hsmp_combat_sim::sim::world::ClaimKind;
    for r in w.claims.iter().filter(|r| r.kind == ClaimKind::Honest && matches!(r.outcome, Some((true, _)))).take(max) {
        let rel = |x: Option<f64>| x.map_or(-1.0, |v| v - r.t_hit);
        eprintln!("OK a{} -> v{} eng={:?} arrive {:.0} fwd {:.0} confirm {:.0} applied {:.0} lag {:.0}",
            r.attacker, r.target, r.eng.map(|e| e.1), rel(r.arrived_t), rel(r.forwarded_t), rel(r.confirm_t), rel(r.applied_t), r.view_lag);
    }
}

#[allow(dead_code)]
pub fn debug_parried(w: &hsmp_combat_sim::sim::world::World, max: usize) {
    use hsmp_combat_sim::sim::world::ClaimKind;
    let mut k = 0;
    for r in &w.claims {
        if r.kind != ClaimKind::Honest { continue; }
        if !matches!(&r.outcome, Some((false, s)) if s == "parried") { continue; }
        k += 1;
        if k > max { break; }
        let near: Vec<String> = w.screen_clashes.iter()
            .filter(|&&(c, p, t, _)| ((c == r.attacker && p == r.target) || (c == r.target && p == r.attacker)) && (t - r.t_hit).abs() < 1500.0)
            .map(|&(c, _, t, s)| if c == r.attacker { format!("A@{:+.0}", t - r.t_hit) } else { format!("V@{:+.0}(shows {:+.0})", t - r.t_hit, s - r.t_hit) }).collect();
        eprintln!("PARRIED a{} -> v{} t={:.0} eng={:?} n={} clashes [{}]", r.attacker, r.target, r.t_hit, r.eng.map(|e| e.1), r.n_events, near.join(" "));
    }
}

#[allow(dead_code)]
pub fn debug_cheats(w: &hsmp_combat_sim::sim::world::World, max: usize) {
    use hsmp_combat_sim::sim::world::ClaimKind;
    for r in w.claims.iter().filter(|r| matches!(r.kind, ClaimKind::Cheat(_)) && r.effective && hsmp_combat_sim::sim::report::cheat_success(r)).take(max) {
        let (pi, pm) = speed_probe(w, r.attacker, r.t_main, r.s_blade);
        eprintln!("CHEAT-OK {:?} a{} -> v{} t={:.0} lie={:.0} lag={:.0} claimed={:.1} booked={:?} mp={:?} rel={:?} bound={:.1} n={} bone={} speed={:.0} srv={:?} probe inst={:.0} max2={:.0} s={:.2} w={:?}",
            r.kind, r.attacker, r.target, r.t_main, r.lie, r.view_lag, r.claimed_loss, r.booked_loss, r.mp, r.rel_dbg, r.bound_sum, r.n_events, r.bone, r.speed, r.server_speed, pi, pm, r.s_blade, w.plan.fighters[r.attacker].weapon.class);
    }
}

#[allow(dead_code)]
pub fn speed_probe(w: &hsmp_combat_sim::sim::world::World, a: usize, t: f64, s: f32) -> (f32, f32) {
    let f = &w.plan.fighters[a];
    let pt = |tt: f64| { let p = f.pose(tt); let b = p.blade; [b.base[0] + (b.tip[0]-b.base[0])*s, b.base[1] + (b.tip[1]-b.base[1])*s, b.base[2] + (b.tip[2]-b.base[2])*s] };
    let d = |x: [f32;3], y: [f32;3]| ((x[0]-y[0]).powi(2)+(x[1]-y[1]).powi(2)+(x[2]-y[2]).powi(2)).sqrt();
    let inst = d(pt(t), pt(t - 1.0)) * 1000.0;
    let mut mx: f32 = 0.0;
    for k in 0..2 { let t1 = t - 16.67 * k as f64; mx = mx.max(d(pt(t1), pt(t1 - 16.67)) * 1000.0 / 16.67); }
    (inst, mx)
}

#[allow(dead_code)]
pub fn debug_clamped(w: &hsmp_combat_sim::sim::world::World, max: usize) {
    use hsmp_combat_sim::sim::world::ClaimKind;
    for r in w.claims.iter().filter(|r| r.kind == ClaimKind::Honest && !r.lethal && r.booked_loss.map_or(false, |b| b + 0.01 < r.claimed_loss)).take(max) {
        eprintln!("CLAMPED a{} -> v{} claimed={:.1} booked={:.1} bound={:.1} n={} bone={} speed={:.0} srv={:?} hand={} w={:?}",
            r.attacker, r.target, r.claimed_loss, r.booked_loss.unwrap_or(-1.0), r.bound_sum, r.n_events, r.bone, r.speed, r.server_speed, r.hand, w.plan.fighters[r.attacker].weapon.class);
    }
}

#[allow(dead_code)]
pub fn debug_parry_miss(w: &hsmp_combat_sim::sim::world::World, max: usize) {
    use hsmp_combat_sim::sim::world::ClaimKind;
    let mut k = 0;
    for r in &w.claims {
        if r.kind != ClaimKind::Honest || !matches!(r.outcome, Some((true, _))) { continue; }
        let near: Vec<String> = w.screen_clashes.iter()
            .filter(|&&(c, p, t, s)| (c == r.target && p == r.attacker && s >= r.t_main - 150.0 && s <= r.t_main + 17.0)
                || (c == r.attacker && p == r.target && t >= r.t_main - 150.0 && t <= r.t_main + 5.0))
            .map(|&(c, _, t, s)| if c == r.attacker { format!("A@{:+.0}", t - r.t_main) } else { format!("V@{:+.0}(shows {:+.0})", t - r.t_main, s - r.t_main) }).collect();
        if near.is_empty() { continue; }
        k += 1;
        if k > max { break; }
        let rel = |x: Option<f64>| x.map_or(-1.0, |v| v - r.t_main);
        eprintln!("PARRY-MISS a{} -> v{} t={:.0} arrive {:.0} fwd {:.0} clashes [{}]", r.attacker, r.target, r.t_main, rel(r.arrived_t), rel(r.forwarded_t), near.join(" "));
    }
}

#[allow(dead_code)]
pub fn debug_counters(w: &hsmp_combat_sim::sim::world::World) {
    for i in 0..w.cfg.players {
        let c = hsmp_combat_sim::validate::cheat::get(w.pid(i));
        eprintln!("COUNTERS p{} {:?}", i, c);
    }
}

#[allow(dead_code)]
pub fn debug_rel(w: &hsmp_combat_sim::sim::world::World) {
    let mut v = Vec::new();
    for r in &w.claims {
        if let (Some((t, s, Some(srv), pt)), Some(so), Some(mp)) = (r.rel_dbg, r.solo, r.mp) {
            v.push((srv / t, t, s, srv, pt, so, mp));
        }
    }
    v.sort_by(|a, b| a.0.total_cmp(&b.0));
    for x in v.iter().step_by((v.len() / 25).max(1)) {
        eprintln!("REL server/true={:.2} true={:.0} standin={:.0} server={:.0} point={:.0} solo={:.2} mp={:.2}", x.0, x.1, x.2, x.3, x.4, x.5, x.6);
    }
}

#[allow(dead_code)]
pub fn debug_cell(w: &hsmp_combat_sim::sim::world::World, cell: &str) {
    for r in &w.claims {
        if let (Some((cl, ar, z, _, _)), Some((t, s, srv, pt)), Some(so), Some(mp)) = (r.pcell, r.rel_dbg, r.solo, r.mp) {
            let zone = match z { 5 => "head", 4 => "neck", 3 => "upper", 2 => "lower", _ => "limb" };
            if format!("{:?}/{:?}/{}", cl, ar, zone) != cell { continue; }
            eprintln!("CELL true={:.0} standin={:.0} server={:?} point={:.0} srvspeed={:?} solo={:.2} mp={:.2} bone={}", t, s, srv, pt, r.server_speed, so, mp, r.bone);
        }
    }
}

#[allow(dead_code)]
pub fn debug_health(w: &hsmp_combat_sim::sim::world::World) {
    let l = w.health_log();
    for &(v, t, hp, shown) in l.ledger_cuts.iter().take(12) {
        eprintln!("CUT v{} t={:.0} sent={:.2} shown={:.2}", v, t, hp, shown);
        for a in l.applies.iter().filter(|a| a.victim == v && a.t > t - 1500.0 && a.t <= t) {
            eprintln!("    apply t={:.0} a{} id={} life={} mp={:.2} booked={:.2} hp {:.2}->{:.2}", a.t, a.attacker, a.hit_id, a.life, a.mp, a.booked, a.hp_before, a.hp_after);
        }
    }
}
