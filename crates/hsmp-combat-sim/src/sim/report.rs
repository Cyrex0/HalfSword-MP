//! Metrics over finished worlds and the report tables.
//!
//! Honest acceptance counts every claim an honest client sent for a contact
//! that really happened on its screen, EXCEPT claims that a death made moot
//! (target / attacker down, counted under trades) and claims through a parry
//! that some screen really showed (counted under clash correctness). A claim
//! never answered counts as rejected.

use super::script::Kind;
use super::world::*;
use crate::combat;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::fmt::Write;

#[derive(Default, Clone, Debug)]
pub struct Stats {
    pub label: String,
    pub runs: u32,
    pub honest: u32,
    pub honest_ok: u32,
    pub reasons: BTreeMap<String, u32>,
    pub contacts: u32,
    pub honest_claims_all: u32,
    /// claims through a parry some screen showed / of them cancelled
    pub parry_claims: u32,
    pub parry_cancelled: u32,
    /// honest claims cancelled as "parried" with no clash on any screen
    pub false_cancel: u32,
    pub trades: u32,
    pub trades_mutual: u32,
    pub view_lag: Vec<f64>,
    pub behind: Vec<f64>,
    pub confirm: Vec<f64>,
    pub clamped_honest: u32,
    pub glints_true: u32,
    pub glints_false: u32,
    /// cheat → (effective claims, successes)
    pub cheat: BTreeMap<String, (u32, u32)>,
    /// Solo parity: "class/armour/zone" → [(solo Health loss, MP Health loss)]
    /// per accepted honest blade hit.
    pub parity: BTreeMap<String, Vec<(f32, f32)>>,
}

fn reason_code(r: &str) -> String { r.split(':').next().unwrap_or(r).trim().to_string() }

fn pct(a: u32, b: u32) -> f64 { if b == 0 { f64::NAN } else { 100.0 * a as f64 / b as f64 } }

pub fn quant(v: &[f64], q: f64) -> f64 {
    if v.is_empty() { return f64::NAN; }
    let mut s = v.to_vec();
    s.sort_by(|a, b| a.total_cmp(b));
    s[(((s.len() as f64) * q).ceil() as usize).clamp(1, s.len()) - 1]
}

impl Stats {
    pub fn new(label: &str) -> Stats { Stats { label: label.into(), ..Default::default() } }

    pub fn accept_pct(&self) -> f64 { pct(self.honest_ok, self.honest) }
    pub fn false_cancel_pct(&self) -> f64 { pct(self.false_cancel, self.honest + self.false_cancel) }
    pub fn parry_cancel_pct(&self) -> f64 { pct(self.parry_cancelled, self.parry_claims) }
    pub fn trade_pct(&self) -> f64 { pct(self.trades_mutual, self.trades) }
    pub fn claims_per_contact(&self) -> f64 {
        if self.contacts == 0 { f64::NAN } else { self.honest_claims_all as f64 / self.contacts as f64 }
    }
    pub fn cheat_pct(&self, name: &str) -> (u32, f64) {
        let (n, k) = self.cheat.get(name).copied().unwrap_or((0, 0));
        (n, pct(k, n))
    }

    /// Fold one finished world in.
    pub fn add(&mut self, w: &World) {
        self.runs += 1;
        // The claim went through a parry some screen showed: the DEFENDER saw
        // the attacker's blade (one display delay after the swing) meet its
        // own, or the ATTACKER's own blade met the shown guard just before.
        // Exact ground truth (the sim knows which attacker instant each
        // screen showed): the server should cancel a hit whose blade met a
        // parry on the DEFENDER's screen at attacker time [hit − 150 ms,
        // hit + 1 frame], or on the ATTACKER's own screen in the same window
        // (its own clock; a clash in a later frame comes after the cut).
        let clash_near = |a: usize, b: usize, t0: f64| clash_truth(w, a, b, t0);
        let spammers: HashSet<usize> = w.cfg.cheats.iter().filter(|c| c.1 == Cheat::ParrySpam).map(|c| c.0).collect();
        let cheaters = |k: Cheat| -> HashSet<usize> { w.cfg.cheats.iter().filter(|c| c.1 == k).map(|c| c.0).collect() };
        let (laggers, fakers) = (cheaters(Cheat::JitterInflate), cheaters(Cheat::FakeParry));
        let ack_holders = cheaters(Cheat::AckHold);
        let mut contacts: HashSet<(usize, u64, &str)> = HashSet::new();
        // JitterInflate / AckHold count BLOWS (attacker, contact): one blow claims
        // every bone it touched, and one clash cancels them all.
        let mut lag_blows: HashSet<(&str, usize, u64)> = HashSet::new();
        // Trade pairs → per side (earliest true time its lethal blow landed on its
        // own screen, any of its lethal claims accepted). A real trade is two
        // blows landing within 100 ms of each other: both must count.
        let mut trade: HashMap<(usize, usize, usize), [(f64, bool); 2]> = HashMap::new();
        for r in &w.claims {
            let out = r.outcome.clone().unwrap_or((false, "unresolved".into()));
            match r.kind {
                ClaimKind::Honest => {
                    self.honest_claims_all += 1;
                    contacts.insert((r.attacker, r.contact, r.bone));
                    let code = if out.0 { String::new() } else { reason_code(&out.1) };
                    if r.lethal {
                        // Trades: both strikes of a Trade engagement.
                        if let Some((ei, Kind::Trade)) = r.eng {
                            let e = &w.plan.engagements[ei];
                            let key = (e.attacker.min(e.defender), e.attacker.max(e.defender), (e.t_c / 2000.0) as usize);
                            let ent = trade.entry(key).or_insert([(f64::INFINITY, false); 2]);
                            let side = if r.attacker == key.0 { 0 } else { 1 };
                            if code != "parried" {
                                ent[side].0 = ent[side].0.min(r.t_main);
                                ent[side].1 |= out.0;
                            }
                        }
                    }
                    if matches!(code.as_str(), "target_down" | "attacker_down" | "round_over") { continue; }
                    if spammers.contains(&r.target) {
                        let e = self.cheat.entry("ParrySpam".into()).or_insert((0, 0));
                        e.0 += 1;
                        if code == "parried" && !clash_near(r.attacker, r.target, r.t_main) { e.1 += 1; }
                    }
                    // FakeParry: honest hits on the faker whose swing it faked a block
                    // against (frames showing the attacker at [hit − 150, hit + 17], the
                    // server's clash window), every one of them a blade no hand of its
                    // could hold (a holdable one is a block the server cannot tell from
                    // a real one).
                    let faked: Vec<bool> = w.fake_parries.iter()
                        .filter(|f| f.0 == r.target && f.1 == r.attacker && f.4 >= r.t_main - 150.0 && f.4 <= r.t_main + 17.0)
                        .map(|f| f.3).collect();
                    if fakers.contains(&r.target) && !faked.is_empty() && faked.iter().all(|e| *e) {
                        let e = self.cheat.entry("FakeParry".into()).or_insert((0, 0));
                        e.0 += 1;
                        if code == "parried" && !clash_near(r.attacker, r.target, r.t_main) { e.1 += 1; }
                    }
                    if clash_near(r.attacker, r.target, r.t_main) {
                        self.parry_claims += 1;
                        if code == "parried" { self.parry_cancelled += 1; }
                        continue;
                    }
                    if code == "parried" { self.false_cancel += 1; }
                    // JitterInflate: every honest hit on the lag-switching victim;
                    // the cheat succeeds when it is rejected.
                    if laggers.contains(&r.target) && lag_blows.insert(("JitterInflate", r.attacker, r.contact)) {
                        let e = self.cheat.entry("JitterInflate".into()).or_insert((0, 0));
                        e.0 += 1;
                        if !out.0 { e.1 += 1; }
                    }
                    // AckHold: the same, with the acks held too.
                    if ack_holders.contains(&r.target) && lag_blows.insert(("AckHold", r.attacker, r.contact)) {
                        let e = self.cheat.entry("AckHold".into()).or_insert((0, 0));
                        e.0 += 1;
                        if !out.0 { e.1 += 1; }
                    }
                    self.honest += 1;
                    if out.0 {
                        self.honest_ok += 1;
                        if let (Some(s), Some(m), Some((cl, ar, z, _, _)), false) = (r.solo, r.mp, r.pcell, r.lethal) {
                            let zone = match z { 5 => "head", 4 => "neck", 3 => "upper", 2 => "lower", _ => "limb" };
                            self.parity.entry(format!("{:?}/{:?}/{}", cl, ar, zone)).or_default().push((s, m));
                            if std::env::var("SIM_CELL").map_or(false, |c| c == format!("{:?}/{:?}/{}", cl, ar, zone)) && (s - m).abs() > 1.0 {
                                eprintln!("PAR solo={:.2} mp={:.2} rel={:?} speed={:.0} srv={:?} bone={} n_ev={}", s, m, r.rel_dbg, r.speed, r.server_speed, r.bone, r.n_events);
                            }
                        }
                        self.view_lag.push(r.view_lag);
                        if let Some(a) = r.applied_t { self.behind.push(a - (r.t_main - r.view_lag)); }
                        if let Some(c) = r.confirm_t { self.confirm.push(c - r.t_main); }
                        // Clamped: armour-stage claims (solo parity) whose replay did
                        // clearly less than solo (the server cut the honest inputs);
                        // others: the booked deltas cut below the claim.
                        if let (Some(s), Some(m)) = (r.solo, r.mp) {
                            if !r.lethal && m + 0.5 < 0.75 * s { self.clamped_honest += 1; }
                        } else if let Some(b) = r.booked_loss {
                            if !r.lethal && b + 0.01 < r.claimed_loss { self.clamped_honest += 1; }
                        }
                    } else {
                        *self.reasons.entry(code).or_insert(0) += 1;
                    }
                }
                ClaimKind::Cheat(ch) => {
                    if !r.effective { continue; }
                    let success = cheat_success(r);
                    let e = self.cheat.entry(format!("{:?}", ch)).or_insert((0, 0));
                    e.0 += 1;
                    if success { e.1 += 1; }
                }
            }
        }
        // GodMode (health model): lives where the ignored replays added up to
        // GODMODE_MARGIN × a Health bar; success = no server death within
        // GODMODE_SUCCESS_MS after that.
        if w.cfg.health_model {
            use super::world::health::GODMODE_SUCCESS_MS;
            let l = w.health_log();
            for &(c, life, t) in &l.godmode_lethal {
                if t + GODMODE_SUCCESS_MS > w.cfg.dur_ms + 2500.0 { continue; }
                let e = self.cheat.entry("GodMode".into()).or_insert((0, 0));
                e.0 += 1;
                // Caught = declared dead (enforcing servers) or flagged
                // (the default is detection: the round is not decided by
                // an estimate) within GODMODE_SUCCESS_MS.
                let killed = l.server_deaths.iter().any(|x| x.0 == c && x.1 == life && x.2 <= t + GODMODE_SUCCESS_MS);
                let flagged = l.godmode_flags.iter().any(|x| x.0 == c && x.1 == life && x.2 <= t + GODMODE_SUCCESS_MS);
                if !killed && !flagged { e.1 += 1; }
            }
        }
        self.contacts += contacts.len() as u32;
        for (_, s) in trade {
            if !(s[0].0.is_finite() && s[1].0.is_finite() && (s[0].0 - s[1].0).abs() <= 100.0) { continue; }
            self.trades += 1;
            if s[0].1 && s[1].1 { self.trades_mutual += 1; }
        }
        for &(to, other, t) in &w.glints {
            if w.screen_clashes.iter().any(|&(c, p, tc, _)| ((c == to && p == other) || (c == other && p == to)) && tc <= t && tc >= t - 1000.0) { self.glints_true += 1 } else { self.glints_false += 1 }
        }
    }

    pub fn top_reasons(&self, k: usize) -> String {
        let mut v: Vec<(&String, &u32)> = self.reasons.iter().collect();
        v.sort_by(|a, b| b.1.cmp(a.1).then(a.0.cmp(b.0)));
        v.iter().take(k).map(|(r, n)| format!("{r} {n}")).collect::<Vec<_>>().join(", ")
    }
}

pub fn table(rows: &[Stats]) -> String {
    let mut s = String::new();
    let _ = writeln!(s, "| scenario | runs | honest claims | accepted % | rejects (top) | claims/contact | parry cancelled % | false cancel % | trades mutual % | view lag p50/p95/max ms | victim feels it p95 ms | confirm p50/p95 ms | honest clamped |");
    let _ = writeln!(s, "|---|---|---|---|---|---|---|---|---|---|---|---|---|");
    for r in rows {
        let _ = writeln!(s, "| {} | {} | {} | {:.2} | {} | {:.2} | {:.1} ({}) | {:.2} | {:.0} ({}) | {:.0}/{:.0}/{:.0} | {:.0} | {:.0}/{:.0} | {} |",
            r.label, r.runs, r.honest, r.accept_pct(), if r.reasons.is_empty() { "-".into() } else { r.top_reasons(4) },
            r.claims_per_contact(), r.parry_cancel_pct(), r.parry_claims, r.false_cancel_pct(),
            r.trade_pct(), r.trades,
            quant(&r.view_lag, 0.5), quant(&r.view_lag, 0.95), quant(&r.view_lag, 1.0),
            quant(&r.behind, 0.95), quant(&r.confirm, 0.5), quant(&r.confirm, 0.95), r.clamped_honest);
    }
    s
}

pub fn cheat_table(rows: &[(String, u32, u32)]) -> String {
    let mut s = String::new();
    let _ = writeln!(s, "| cheat | effective claims | accepted | false-accept % |");
    let _ = writeln!(s, "|---|---|---|---|");
    for (n, a, k) in rows {
        let _ = writeln!(s, "| {} | {} | {} | {:.2} |", n, a, k, pct(*k, *a));
    }
    s
}

/// Did an effective cheat claim get what it lied for? Accepted, and for
/// damage inflation: booked above the contact's physical maximum beyond the
/// server's documented tolerance (speed ×1.2, tick merge ×1.5).
pub fn cheat_success(r: &ClaimRec) -> bool {
    let ok = matches!(r.outcome, Some((true, _)));
    match r.kind {
        // (anything up to the 2 HP measurement floor is never clamped).
        // Armour-stage claims: what the victim's replay of the approved inputs did.
        // (+60 uu/s: the stream-measured contact speed's absolute precision at
        // 30 Hz, which matters for slow hand bumps only.)
        ClaimKind::Cheat(Cheat::DamageInflate) => ok && r.mp.or(r.booked_loss).map_or(false, |b| b > (1.8 * r.bound_sum * (1.0 + 60.0 / r.speed.max(1.0)) + 0.5).max(crate::validate::damage::LOSS_FLOOR_MEASURE + 0.01)),
        // A fabricated contact accepted at the measurement floor gained nothing
        // (e.g. bodies really touching: booked as an unarmed bump).
        _ => ok && r.booked_loss.map_or(false, |b| b > crate::validate::damage::LOSS_FLOOR_MEASURE + 0.01),
    }
}

/// Ground truth: a blade-on-blade contact on the DEFENDER's screen at
/// attacker time [t0 − 150, t0 + 17] or on the ATTACKER's own screen in
/// [t0 − 150, t0 + 5] (see `Stats::add`).
pub fn clash_truth(w: &World, a: usize, b: usize, t0: f64) -> bool {
    w.screen_clashes.iter().any(|&(c, p, t, shown)| {
        (c == b && p == a && shown >= t0 - 150.0 && shown <= t0 + 17.0)
            || (c == a && p == b && t >= t0 - 150.0 && t <= t0 + 5.0)
    })
}

pub fn parried_truth(w: &World, r: &ClaimRec) -> bool { clash_truth(w, r.attacker, r.target, r.t_main) }

/// One parity cell: (n, median solo, median MP, MP/solo of the median
/// hits-to-kill, median hits-to-kill solo, MP). Hits-to-kill is a paired Monte
/// Carlo: the same random sequence of the cell's contacts (each one a solo loss
/// and the MP loss of the same contact) is dealt until 100 Health is gone;
/// INFINITY when solo needs more than TTK_MAX_HITS (a trivial cell).
pub fn parity_cell(v: &[(f32, f32)]) -> (usize, f64, f64, f64, f64, f64) {
    let s: Vec<f64> = v.iter().map(|x| x.0 as f64).collect();
    let m: Vec<f64> = v.iter().map(|x| x.1 as f64).collect();
    let (ms, mm) = (quant(&s, 0.5), quant(&m, 0.5));
    let (hs, hm) = ttk(v);
    (v.len(), ms, mm, if hs.is_finite() && hs > 0.0 { hm / hs } else { f64::NAN }, hs, hm)
}

pub const TTK_MAX_HITS: usize = 60;
const TTK_TRIALS: usize = 801;

/// Median hits-to-kill (solo, MP) of a cell; deterministic (fixed-seed LCG).
pub fn ttk(v: &[(f32, f32)]) -> (f64, f64) {
    if v.is_empty() { return (f64::INFINITY, f64::INFINITY); }
    let mut x: u64 = 0x9E37_79B9_7F4A_7C15;
    let (mut a, mut b) = (Vec::new(), Vec::new());
    for _ in 0..TTK_TRIALS {
        let (mut hs, mut hm, mut fs, mut fm) = (0.0f64, 0.0f64, 0.0f64, 0.0f64);
        let mut k = 0;
        while (hs < 100.0 || hm < 100.0) && k < 4 * TTK_MAX_HITS {
            x = x.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            let (s, m) = v[((x >> 33) as usize) % v.len()];
            k += 1;
            // Fractional: the killing hit counts for the share of it needed.
            if hs < 100.0 { let r = 100.0 - hs; hs += s as f64; fs = (k - 1) as f64 + if hs >= 100.0 { r / s as f64 } else { 1.0 }; }
            if hm < 100.0 { let r = 100.0 - hm; hm += m as f64; fm = (k - 1) as f64 + if hm >= 100.0 { r / m as f64 } else { 1.0 }; }
        }
        a.push(if hs >= 100.0 { fs } else { f64::INFINITY });
        b.push(if hm >= 100.0 { fm } else { f64::INFINITY });
    }
    a.sort_by(|p, q| p.total_cmp(q));
    b.sort_by(|p, q| p.total_cmp(q));
    let (s, m) = (a[a.len() / 2], b[b.len() / 2]);
    if s > TTK_MAX_HITS as f64 { (f64::INFINITY, m) } else { (s, m) }
}

pub fn parity_table(s: &Stats, min_n: usize) -> String {
    let mut o = String::new();
    let _ = writeln!(o, "| weapon/armour/zone | hits | median Health loss solo | MP | MP/solo hits-to-kill | hits-to-kill solo | MP |");
    let _ = writeln!(o, "|---|---|---|---|---|---|---|");
    for (k, v) in &s.parity {
        if v.len() < min_n { continue; }
        let (n, ms, mm, r, hs, hm) = parity_cell(v);
        let _ = writeln!(o, "| {k} | {n} | {ms:.2} | {mm:.2} | {r:.3} | {hs:.1} | {hm:.1} |");
    }
    o
}
