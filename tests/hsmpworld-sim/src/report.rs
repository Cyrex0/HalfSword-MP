//! Aggregation of `sim::Metrics` over seeds, and the report table.

use crate::sim::Metrics;
use std::collections::BTreeMap;

pub fn pct(v: &[f64], p: f64) -> f64 {
    if v.is_empty() { return 0.0; }
    let mut s = v.to_vec();
    s.sort_by(|a, b| a.total_cmp(b));
    let i = ((s.len() - 1) as f64 * p).round() as usize;
    s[i.min(s.len() - 1)]
}

/// The headline numbers of one profile (all seeds pooled).
#[derive(Debug, Default, Clone)]
pub struct Summary {
    pub profile: String,
    /// phase -> (moving p50 cm, moving p95 cm, path p95 cm, moving rot p95 deg, rest p95 cm, rest rot p95 deg, snaps)
    pub phases: BTreeMap<String, [f64; 7]>,
    /// check -> (max cm, max deg, bodies off by > 2 cm / 2 deg)
    pub checks: BTreeMap<String, (f64, f64, usize)>,
    pub flips: u32,
    pub fights: u32,
    pub conflict_ms: f64,
    pub hand_p95: f64,
    pub down_bps: f64,
    pub up_bps: f64,
    pub violations: Vec<String>,
}

pub fn summarize(profile: &str, runs: &[Metrics]) -> Summary {
    let mut s = Summary { profile: profile.into(), ..Default::default() };
    let mut names: Vec<&str> = Vec::new();
    for m in runs {
        for k in m.moving_d.keys().chain(m.rest_d.keys()) { if !names.contains(k) { names.push(k); } }
    }
    for n in names {
        let pool = |f: &dyn Fn(&Metrics) -> Option<&Vec<f64>>| -> Vec<f64> {
            runs.iter().filter_map(f).flat_map(|v| v.iter().copied()).collect()
        };
        let md = pool(&|m| m.moving_d.get(n));
        let ma = pool(&|m| m.moving_a.get(n));
        let pd = pool(&|m| m.path_d.get(n));
        let rd = pool(&|m| m.rest_d.get(n));
        let ra = pool(&|m| m.rest_a.get(n));
        let snaps: u32 = runs.iter().map(|m| m.snaps.get(n).copied().unwrap_or(0)).sum();
        s.phases.insert(n.to_string(), [pct(&md, 0.5), pct(&md, 0.95), pct(&pd, 0.95), pct(&ma, 0.95), pct(&rd, 0.95),
                                        pct(&ra, 0.95), snaps as f64 / runs.len().max(1) as f64]);
    }
    for m in runs {
        for (label, d, a, _, off) in &m.checks {
            let e = s.checks.entry(label.to_string()).or_insert((0.0, 0.0, 0));
            e.0 = e.0.max(*d);
            e.1 = e.1.max(*a);
            e.2 = e.2.max(*off);
        }
        s.flips += m.flips;
        s.fights += m.fights;
        s.conflict_ms += m.conflict_ms;
        s.down_bps += m.down_bps / runs.len() as f64;
        s.up_bps += m.up_bps / runs.len() as f64;
        s.violations.extend(m.violations.iter().cloned());
    }
    let hd: Vec<f64> = runs.iter().flat_map(|m| m.hand_d.iter().copied()).collect();
    s.hand_p95 = pct(&hd, 0.95);
    let n = runs.len().max(1) as f64;
    s.conflict_ms /= n;
    s
}

pub fn print(s: &Summary) {
    println!("== profile {} ==", s.profile);
    println!("  {:<9} {:>9} {:>9} {:>9} {:>9} {:>9} {:>9} {:>7}", "phase", "mov p50", "mov p95", "path p95", "rot p95",
             "rest p95", "rrot p95", "snaps");
    for (k, v) in &s.phases {
        println!("  {:<9} {:>8.1}c {:>8.1}c {:>8.1}c {:>8.1}d {:>8.2}c {:>8.2}d {:>7.1}", k, v[0], v[1], v[2], v[3], v[4], v[5], v[6]);
    }
    for (k, v) in &s.checks {
        println!("  check {:<12} max {:>7.2} cm {:>7.2} deg, {} bodies off", k, v.0, v.1, v.2);
    }
    println!("  ownership flips {} (fights {}), hold conflict {:.0} ms/run, held item vs stand-in hand p95 {:.1} cm",
             s.flips, s.fights, s.conflict_ms, s.hand_p95);
    println!("  world bandwidth per client: down {:.2} kB/s, up {:.2} kB/s", s.down_bps / 1024.0, s.up_bps / 1024.0);
    if !s.violations.is_empty() {
        println!("  VIOLATIONS: {}", s.violations.len());
        for v in s.violations.iter().take(8) { println!("    {v}"); }
    }
}

/// WORLD-2's pairing (hsmp-gate rules.rs `world2`), on the simulator's world_track events:
/// each sample of one client against the other clients' track of the same body at the same
/// server time (linear between their samples within 250 ms). Returns (moving cm, final rest cm).
pub fn pair_tracks(m: &Metrics) -> (Vec<f64>, Vec<f64>) {
    use std::collections::HashMap;
    // (client, nid) -> [(t, pos, rest)]
    let mut tr: HashMap<(usize, i64), Vec<(f64, [f64; 3], bool)>> = HashMap::new();
    for (c, _, name, f) in &m.events {
        if name != "world_track" { continue; }
        let kv: HashMap<&str, &str> = f.split(' ').filter_map(|p| p.split_once('=')).collect();
        let g = |k: &str| kv.get(k).and_then(|v| v.parse::<f64>().ok());
        let (Some(nid), Some(t), Some(x), Some(y), Some(z)) = (g("nid"), g("t"), g("x"), g("y"), g("z")) else { continue };
        tr.entry((*c, nid as i64)).or_default().push((t, [x, y, z], kv.get("rest") == Some(&"true")));
    }
    let at = |v: &Vec<(f64, [f64; 3], bool)>, t: f64| -> Option<[f64; 3]> {
        let i = v.iter().position(|s| s.0 >= t)?;
        if i == 0 { return (v[0].0 - t < 1.0).then_some(v[0].1); }
        let (a, b) = (v[i - 1], v[i]);
        if b.0 - a.0 > 250.0 { return None; }
        let k = (t - a.0) / (b.0 - a.0).max(1e-9);
        Some([a.1[0] + (b.1[0] - a.1[0]) * k, a.1[1] + (b.1[1] - a.1[1]) * k, a.1[2] + (b.1[2] - a.1[2]) * k])
    };
    let (mut moving, mut rest) = (Vec::new(), Vec::new());
    let keys: Vec<(usize, i64)> = tr.keys().copied().collect();
    for &(c, nid) in &keys {
        for &(c2, n2) in &keys {
            if n2 != nid || c2 <= c { continue; }
            let (a, b) = (&tr[&(c, nid)], &tr[&(c2, nid)]);
            for s in a.iter().filter(|s| !s.2) {
                if let Some(p) = at(b, s.0) { moving.push(crate::phys::dist(s.1, p)); }
            }
            if let (Some(la), Some(lb)) = (a.iter().rev().find(|s| s.2), b.iter().rev().find(|s| s.2)) {
                rest.push(crate::phys::dist(la.1, lb.1));
            }
        }
    }
    (moving, rest)
}
