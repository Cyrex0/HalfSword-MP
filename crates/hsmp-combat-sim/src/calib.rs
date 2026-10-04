//! Calibration of `hit_vel_factor`, the server's Hit Velocity ceiling for armour-stage claims
//! (`validate::damage::clamp_complex`): Hit Velocity ≤ max(peak striking speed,
//! hit_vel_factor(class) · relative speed). The game sets Hit Velocity to the larger of the
//! weapon's COM speed and the contact's normal impulse, and the impulse scales with the
//! masses, so the factor is "how far above the relative speed can an honest normal impulse
//! go", plus the server's own error on the relative speed.
//!
//! Inputs (one [`Sample`] each):
//! - in-game `[HSMPParity] DCD on ...` lines (every Deal Complex Damage the game makes, solo
//!   or on a stand-in, with |Hit Velocity|, |Hit Impulse| and the relative speed of the two
//!   bodies at the call): the physics ratio |Hit Velocity| / relative speed;
//! - server `combat: impact rescale` lines (honest MP fights): what the ceiling compares,
//!   the claimed Hit Velocity against the server's peak and relative speeds;
//! - the combat sim's honest armour-stage claims (`sim_samples`): the same comparison with
//!   the true relative speed known, so the server's measurement error is separate.
//!
//! `report` gives per class the ratio quantiles, how many honest blows the shipped factor
//! clamps, and a recommendation: the 99.9th percentile of the needed factor × `MARGIN`,
//! only from at least `MIN_N` samples. Run with
//! `cargo run --release -p hsmp-combat-sim -- --hvf-logs <files>` / `-- --hvf <profile>`.

use crate::validate::damage::{class_of, hit_vel_factor, WeaponClass};

/// Relative speeds below this are not used (the ratio is noise at a resting contact).
pub const REL_MIN: f32 = 200.0;
/// Headroom over the 99.9th percentile of the needed factor.
pub const MARGIN: f32 = 1.1;
/// Fewer samples than this per class give no recommendation.
pub const MIN_N: usize = 200;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    /// `[HSMPParity] DCD on` (the game's own call; `rel` is the bodies' relative speed).
    Game,
    /// `combat: impact rescale` (server; `rel` / `peak` are lag comp's).
    Server,
    /// Combat sim honest claim (server rel / peak, `true_rel` known).
    Sim,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Sample {
    pub source: Source,
    pub class: WeaponClass,
    /// |Hit Velocity| the game passed (claimed, before any server clamp).
    pub vel: f32,
    pub imp: f32,
    /// Relative speed the ceiling uses (game: the bodies'; server / sim: lag comp's).
    pub rel: f32,
    /// Server peak striking speed (None for game lines).
    pub peak: Option<f32>,
    /// Sim only: the true relative speed.
    pub true_rel: Option<f32>,
}

impl Sample {
    /// The factor this blow needs so the ceiling `max(peak, hvf · relative speed)` does not
    /// cut it: |Hit Velocity| / relative speed (None: the peak striking speed already covers
    /// it, or the relative speed is too small to say).
    pub fn needed(&self) -> Option<f32> {
        if !(self.rel.is_finite() && self.rel >= REL_MIN && self.vel.is_finite()) { return None; }
        if let Some(p) = self.peak { if self.vel <= p { return None; } }
        Some(self.vel / self.rel)
    }

    /// The shipped ceiling with factor `hvf` cuts this blow.
    pub fn clamped_at(&self, hvf: f32) -> bool {
        self.needed().is_some_and(|n| n > hvf)
    }

    /// The looser ceiling `hvf · max(peak, relative speed)` would cut it (tried: it spares
    /// honest blows lag comp under-reads, but lets inflated claims through in the sim).
    pub fn clamped_alt(&self, hvf: f32) -> bool {
        self.needed().is_some() && self.vel > hvf * self.peak.map_or(self.rel, |p| p.max(self.rel))
    }

    /// Sim only: the shipped ceiling against the TRUE relative speed would cut it (no
    /// measurement error).
    pub fn clamped_true(&self, hvf: f32) -> Option<bool> {
        let t = self.true_rel?;
        Some(self.needed().is_some() && self.vel > self.peak.unwrap_or(0.0).max(hvf * t))
    }
}

/// `key=value` tokens of a log line (tracing fields, HSMPParity's `|vel|=...`).
fn field<'a>(line: &'a str, key: &str) -> Option<&'a str> {
    let mut rest = line;
    while let Some(i) = rest.find(key) {
        let before = rest[..i].chars().last();
        let after = &rest[i + key.len()..];
        if (before.is_none() || before == Some(' ') || before == Some('|')) && after.starts_with('=') {
            let v = &after[1..];
            let end = v.find([' ', ',', ':']).unwrap_or(v.len());
            return Some(&v[..end]);
        }
        rest = after;
    }
    None
}

/// A number, also inside `Some(..)`; `None` / garbage -> None.
fn num(s: Option<&str>) -> Option<f32> {
    let s = s?.trim();
    let s = s.strip_prefix("Some(").and_then(|x| x.strip_suffix(')')).unwrap_or(s);
    s.parse::<f32>().ok().filter(|x| x.is_finite())
}

/// Weapon class from a weapon actor / class name in a game log ("ModularWeaponBP_Mace_T2_C",
/// a Willie for body contacts).
pub fn class_of_name(name: &str) -> WeaponClass {
    let n = name.to_ascii_lowercase();
    if n.starts_with("willie") { return WeaponClass::Unarmed; }
    class_of(&n)
}

/// Server `Debug` name of a class ("Sword").
fn class_of_debug(s: &str) -> Option<WeaponClass> {
    use WeaponClass::*;
    Some(match s {
        "Unknown" => Unknown, "Unarmed" => Unarmed, "Dagger" => Dagger, "Sword" => Sword, "Axe" => Axe,
        "Blunt" => Blunt, "Polearm" => Polearm, "Shield" => Shield,
        _ => return None,
    })
}

/// One `[HSMPParity] DCD on <willie> bone=<b> by <owner class>[ wp=<actor>]: |vel|=.. |imp|=..
/// rel=.. ...` line.
pub fn parse_parity_line(line: &str) -> Option<Sample> {
    let i = line.find("DCD on ")?;
    let l = &line[i..];
    let by = l.find(" by ").map(|j| &l[j + 4..])?;
    let owner = by.split([' ', ':']).next().unwrap_or("");
    let wp = field(l, "wp").unwrap_or("");
    let class = if wp.is_empty() { class_of_name(owner) } else {
        match class_of_name(wp) { WeaponClass::Unknown => class_of_name(owner), c => c }
    };
    Some(Sample {
        source: Source::Game, class,
        vel: num(field(l, "|vel|"))?, imp: num(field(l, "|imp|"))?, rel: num(field(l, "rel"))?,
        peak: None, true_rel: None,
    })
}

/// One server `combat: impact rescale` line (it carries `class` since this calibration).
pub fn parse_rescale_line(line: &str) -> Option<Sample> {
    if !line.contains("combat: impact rescale") { return None; }
    let class = field(line, "class").and_then(class_of_debug)?;
    Some(Sample {
        source: Source::Server, class,
        vel: num(field(line, "vel_claimed"))?, imp: num(field(line, "imp_claimed")).unwrap_or(0.0),
        rel: num(field(line, "server_relative"))?, peak: num(field(line, "server_peak")),
        true_rel: None,
    })
}

/// Every sample in a log (UE4SS.log, a server log, or both).
pub fn parse_log(text: &str) -> Vec<Sample> {
    text.lines().filter_map(|l| parse_parity_line(l).or_else(|| parse_rescale_line(l))).collect()
}

/// Honest armour-stage claims of a finished sim world.
pub fn sim_samples(w: &crate::sim::world::World) -> Vec<Sample> {
    use crate::sim::world::ClaimKind;
    let len = |v: [f32; 3]| (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
    let mut out = Vec::new();
    for (i, r) in w.claims.iter().enumerate() {
        if r.kind != ClaimKind::Honest || r.hand { continue; }
        let (Some((true_rel, _, Some(srv), _)), Some((class, ..))) = (r.rel_dbg, r.pcell) else { continue };
        let h = &w.pending_hits[i];
        out.push(Sample {
            source: Source::Sim, class, vel: len(h.velocity), imp: len(h.impulse), rel: srv,
            peak: r.server_peak, true_rel: Some(true_rel),
        });
    }
    out
}


/// Honest fights on `profile` (`seeds` × every suite shape, blade-stream stand-ins): every
/// honest armour-stage claim as a sample.
pub fn sim_run(profile: &str, seeds: u64) -> Vec<Sample> {
    use crate::sim::suite::{profile as prof, SHAPES};
    use crate::sim::world::{Config, World};
    let mut out = Vec::new();
    for seed in 0..seeds {
        for (k, &(n, dur)) in SHAPES.iter().enumerate() {
            let mut cfg = Config::new(prof(profile), n, 1000 * seed + k as u64 + 17);
            cfg.dur_ms = dur;
            let mut w = World::new(cfg);
            w.run();
            out.extend(sim_samples(&w));
        }
    }
    out
}
/// Quantile `q` of `v` (nearest rank; NaN when empty).
pub fn quantile(v: &[f32], q: f64) -> f32 {
    if v.is_empty() { return f32::NAN; }
    let mut s = v.to_vec();
    s.sort_by(|a, b| a.total_cmp(b));
    s[(((s.len() as f64) * q).ceil() as usize).clamp(1, s.len()) - 1]
}

#[derive(Debug, Clone, PartialEq)]
pub struct ClassRow {
    pub class: WeaponClass,
    pub source: Source,
    /// All samples / those where the ceiling binds (`needed` is Some).
    pub n: usize,
    pub n_binding: usize,
    pub p50: f32,
    pub p99: f32,
    pub p999: f32,
    pub max: f32,
    pub shipped: f32,
    /// Share of all samples the shipped factor cuts.
    pub clamped_shipped: f64,
    /// Share the looser ceiling hvf · max(peak, relative speed) would cut.
    pub clamped_alt: f64,
    /// Sim only: share the shipped factor cuts against the true relative speed.
    pub clamped_true: Option<f64>,
    /// p99.9 × MARGIN, when n_binding ≥ MIN_N.
    pub recommended: Option<f32>,
    /// Sim only: p99 of true / server relative speed (the measurement's share).
    pub net_p99: Option<f32>,
}

/// Per class and source.
pub fn report(samples: &[Sample]) -> Vec<ClassRow> {
    use WeaponClass::*;
    let mut rows = Vec::new();
    for source in [Source::Game, Source::Server, Source::Sim] {
        for class in [Dagger, Sword, Axe, Blunt, Polearm, Shield, Unarmed, Unknown] {
            let s: Vec<&Sample> = samples.iter().filter(|x| x.source == source && x.class == class).collect();
            if s.is_empty() { continue; }
            let need: Vec<f32> = s.iter().filter_map(|x| x.needed()).collect();
            let shipped = hit_vel_factor(class);
            let clamped = s.iter().filter(|x| x.clamped_at(shipped)).count();
            let alt = s.iter().filter(|x| x.clamped_alt(shipped)).count();
            let tru: Vec<bool> = s.iter().filter_map(|x| x.clamped_true(shipped)).collect();
            let p999 = quantile(&need, 0.999);
            let net: Vec<f32> = s.iter().filter_map(|x| x.true_rel.filter(|_| x.rel >= REL_MIN).map(|t| t / x.rel)).collect();
            rows.push(ClassRow {
                class, source, n: s.len(), n_binding: need.len(),
                p50: quantile(&need, 0.5), p99: quantile(&need, 0.99), p999, max: quantile(&need, 1.0),
                shipped, clamped_shipped: clamped as f64 / s.len() as f64,
                clamped_alt: alt as f64 / s.len() as f64,
                clamped_true: (!tru.is_empty()).then(|| tru.iter().filter(|b| **b).count() as f64 / tru.len() as f64),
                recommended: (need.len() >= MIN_N).then_some(p999 * MARGIN),
                net_p99: (!net.is_empty()).then(|| quantile(&net, 0.99)),
            });
        }
    }
    rows
}

/// The report as a markdown table.
pub fn table(rows: &[ClassRow]) -> String {
    let mut s = String::from("| source | class | n | binding | needed p50 | p99 | p99.9 | max | shipped | clamped by shipped | ... vs true rel | ... with hvf·max(peak, rel) | recommended | net p99 |\n|---|---|---|---|---|---|---|---|---|---|---|---|---|---|\n");
    for r in rows {
        let f = |x: f32| if x.is_finite() { format!("{x:.2}") } else { "-".into() };
        s.push_str(&format!("| {:?} | {:?} | {} | {} | {} | {} | {} | {} | {:.1} | {:.2} % | {} | {:.2} % | {} | {} |\n",
            r.source, r.class, r.n, r.n_binding, f(r.p50), f(r.p99), f(r.p999), f(r.max), r.shipped,
            100.0 * r.clamped_shipped, r.clamped_true.map_or("-".into(), |x| format!("{:.2} %", 100.0 * x)), 100.0 * r.clamped_alt,
            r.recommended.map_or("(n < 200)".into(), |x| format!("{x:.2}")),
            r.net_p99.map_or("-".into(), f)));
    }
    s
}
