//! hsmp-pose-truth: compare what a stand-in showed against what its owner's
//! body really did, at the same sender time.
//!
//! Inputs (written in-game when HSMP_POSE_PROBE=1, docs/development/subsystems/replication.md):
//!   sender   `<state>/.probe_send.txt`: every game frame of the owner,
//!            `S <sender ms> <23 × px py pz qx qy qz qw> [W bx by bz tx ty tz] [U <utc ms>]`
//!   receiver `<state>/.probe_recv<id>.txt`: every game frame of the stand-in,
//!            `R <sender ms it was driven to> <23 × ...> [W ...] [U <utc ms>]`
//! Bone order = crates/hsmp-pose/src/posecodec_v2.rs BONES. Both games run on one PC in
//! the same arena, so world coordinates and the UTC clock compare directly.
//!
//! For every receiver frame the sender's pose at the label time is
//! interpolated from its per-frame record (linear / slerp between frames
//! ≤ 40 ms apart) and the per-bone position and rotation errors and the
//! blade tip error are measured. Motion-to-display latency = receiver UTC
//! when the pose was shown − sender UTC when that pose happened.
//! Frames are classed by a phases file (`name start_ms end_ms`, sender clock;
//! HSMPDiag motion script) or by the sender's motion, and checked against
//! the acceptance targets: bone ≤ 3 uu and ≤ 3 deg, blade tip ≤ 5 uu.

use std::collections::BTreeMap;

pub const BONES: [&str; 23] = [
    "pelvis", "spine_01", "spine_02", "spine_03", "spine_04", "spine_05",
    "neck_01", "neck_02", "head",
    "clavicle_l", "upperarm_l", "lowerarm_l", "hand_l",
    "clavicle_r", "upperarm_r", "lowerarm_r", "hand_r",
    "thigh_l", "calf_l", "foot_l",
    "thigh_r", "calf_r", "foot_r",
];
const NB: usize = 23;
/// spine_01 has no physics body (animation-driven on both ends): reported, not judged.
fn judged(i: usize) -> bool { i != 1 }
const ARM: [usize; 8] = [9, 10, 11, 12, 13, 14, 15, 16];
const HANDS: [usize; 2] = [12, 16];

#[derive(Clone, Debug)]
pub struct Rec { pub ts: f64, pub b: Vec<[f64; 7]>, pub w: Option<[f64; 6]>, pub utc: Option<f64>, pub vis: bool }

pub fn parse(text: &str, tag: &str) -> Vec<Rec> {
    let mut out = Vec::new();
    for line in text.lines() {
        let mut it = line.split_whitespace();
        if it.next() != Some(tag) { continue; }
        let Some(ts) = it.next().and_then(|x| x.parse::<f64>().ok()) else { continue };
        let rest: Vec<&str> = it.collect();
        if rest.len() < NB * 7 { continue; }
        let mut b = Vec::with_capacity(NB);
        let mut ok = true;
        for i in 0..NB {
            let mut x = [0f64; 7];
            for k in 0..7 { match rest[i * 7 + k].parse::<f64>() { Ok(v) => x[k] = v, Err(_) => ok = false } }
            b.push(x);
        }
        if !ok { continue; }
        let (mut w, mut utc, mut vis) = (None, None, true);
        let mut j = NB * 7;
        while j < rest.len() {
            match rest[j] {
                "W" if j + 6 < rest.len() => {
                    let v: Vec<f64> = rest[j + 1..j + 7].iter().filter_map(|x| x.parse().ok()).collect();
                    if v.len() == 6 { w = Some([v[0], v[1], v[2], v[3], v[4], v[5]]); }
                    j += 7;
                }
                "U" if j + 1 < rest.len() => { utc = rest[j + 1].parse::<f64>().ok().filter(|u| *u >= 0.0); j += 2; }
                "V" if j + 1 < rest.len() => { vis = rest[j + 1] != "0"; j += 2; }
                _ => j += 1,
            }
        }
        out.push(Rec { ts, b, w, utc, vis });
    }
    out.sort_by(|a, b| a.ts.partial_cmp(&b.ts).unwrap());
    out.dedup_by(|a, b| a.ts == b.ts);
    out
}

fn d3(a: &[f64], b: &[f64]) -> f64 { ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2) + (a[2] - b[2]).powi(2)).sqrt() }
fn qang(a: &[f64], b: &[f64]) -> f64 {
    let la = (a[0] * a[0] + a[1] * a[1] + a[2] * a[2] + a[3] * a[3]).sqrt();
    let lb = (b[0] * b[0] + b[1] * b[1] + b[2] * b[2] + b[3] * b[3]).sqrt();
    let d = ((a[0] * b[0] + a[1] * b[1] + a[2] * b[2] + a[3] * b[3]) / (la * lb)).abs().min(1.0);
    2.0 * d.acos().to_degrees()
}
fn slerp(a: &[f64], b: &[f64], t: f64) -> [f64; 4] {
    let mut d = a[0] * b[0] + a[1] * b[1] + a[2] * b[2] + a[3] * b[3];
    let s = if d < 0.0 { d = -d; -1.0 } else { 1.0 };
    let (wa, wb) = if d > 0.9995 { (1.0 - t, t * s) } else {
        let th = d.min(1.0).acos();
        (((1.0 - t) * th).sin() / th.sin(), (t * th).sin() / th.sin() * s)
    };
    let mut q = [a[0] * wa + b[0] * wb, a[1] * wa + b[1] * wb, a[2] * wa + b[2] * wb, a[3] * wa + b[3] * wb];
    let l = (q.iter().map(|c| c * c).sum::<f64>()).sqrt();
    for c in q.iter_mut() { *c /= l; }
    q
}

/// The sender's pose (and UTC) at time `t` (None outside the record or across a gap).
pub fn truth_at(s: &[Rec], t: f64) -> Option<(Vec<[f64; 7]>, Option<[f64; 6]>, Option<f64>)> {
    let i = s.partition_point(|r| r.ts <= t);
    if i == 0 || i >= s.len() { return None; }
    let (a, b) = (&s[i - 1], &s[i]);
    if b.ts - a.ts > 40.0 { return None; }
    let u = (t - a.ts) / (b.ts - a.ts);
    let bones = (0..NB).map(|k| {
        let (x, y) = (&a.b[k], &b.b[k]);
        let q = slerp(&x[3..7], &y[3..7], u);
        [x[0] + (y[0] - x[0]) * u, x[1] + (y[1] - x[1]) * u, x[2] + (y[2] - x[2]) * u, q[0], q[1], q[2], q[3]]
    }).collect();
    let w = match (a.w, b.w) {
        (Some(x), Some(y)) => { let mut o = [0.0; 6]; for k in 0..6 { o[k] = x[k] + (y[k] - x[k]) * u; } Some(o) }
        _ => None,
    };
    let utc = match (a.utc, b.utc) { (Some(x), Some(y)) => Some(x + (y - x) * u), _ => None };
    Some((bones, w, utc))
}

/// Sender motion at `t`: (pelvis speed, pelvis vz, fastest hand speed, tip speed), uu/s.
fn motion(s: &[Rec], t: f64) -> Option<(f64, f64, f64, f64)> {
    let (a, wa, _) = truth_at(s, t - 25.0)?;
    let (b, wb, _) = truth_at(s, t + 25.0)?;
    let v = |i: usize| d3(&a[i], &b[i]) / 0.05;
    let tip = match (wa, wb) { (Some(x), Some(y)) => d3(&x[3..6], &y[3..6]) / 0.05, _ => 0.0 };
    Some((v(0), (b[0][2] - a[0][2]) / 0.05, v(12).max(v(16)), tip))
}

#[derive(Default)]
pub struct Acc {
    pub pos: Vec<f64>, pub rot: Vec<f64>, pub arm_pos: Vec<f64>, pub arm_rot: Vec<f64>,
    pub hand_pos: Vec<f64>, pub hand_rot: Vec<f64>, pub tip: Vec<f64>, pub lat: Vec<f64>,
    pub hand_speed: Vec<f64>, pub frames: usize, pub frames_ok: usize,
    /// Per body: sum of squared second differences (stand-in on display time, truth on its own time).
    pub hf_r: Vec<f64>, pub hf_t: Vec<f64>, pub hf_n: f64,
    /// Stand-in foot speed (uu/s) while the sender's foot is planted (< 5 uu/s).
    pub foot_slide: Vec<f64>,
}

pub fn pct(v: &mut Vec<f64>, p: f64) -> f64 {
    if v.is_empty() { return f64::NAN; }
    v.sort_by(|a, b| a.partial_cmp(b).unwrap());
    v[((v.len() as f64 - 1.0) * p).round() as usize]
}

pub struct Report { pub classes: BTreeMap<String, Acc>, pub per_bone: Vec<(Vec<f64>, Vec<f64>)>, pub matched: usize, pub unmatched: usize,
    /// Owner idle => stand-in idle: stand-in hand/pelvis speed (uu/s) on frames where the sender is still.
    pub idle_standin_speed: Vec<f64>,
    /// RMS (uu) of the stand-in pelvis/hands about their mean over idle runs (<= 5 s each).
    pub idle_rms: Vec<f64> }

pub fn compare(s: &[Rec], r: &[Rec], phases: &[(String, f64, f64)]) -> Report {
    let mut rep = Report { classes: BTreeMap::new(), per_bone: vec![(Vec::new(), Vec::new()); NB], matched: 0, unmatched: 0, idle_standin_speed: Vec::new(), idle_rms: Vec::new() };

    for (ri, rr) in r.iter().enumerate() {
        // Owner idle => stand-in idle (no extrapolation run-away / buffer loops).
        // Owner idle => stand-in idle: stand-in motion over the same 50 ms window
        // the sender motion is measured on (one frame is too noisy at 0.2 uu).
        if let (Some((pel, _, hand, _)), Some(a)) = (motion(s, rr.ts), r.iter().rev().find(|x| x.ts <= rr.ts - 50.0 && x.ts >= rr.ts - 80.0)) {
            if pel < 5.0 && hand < 10.0 {
                let dt = (rr.ts - a.ts) / 1000.0;
                let v = [0usize, 12, 16].iter().map(|&i| d3(&rr.b[i], &a.b[i]) / dt).fold(0.0, f64::max);
                rep.idle_standin_speed.push(v);
            }
        }
        let Some((tb, tw, tutc)) = truth_at(s, rr.ts) else { rep.unmatched += 1; continue };
        rep.matched += 1;
        let mot = motion(s, rr.ts);
        let class = phases.iter().find(|(_, a, b)| rr.ts >= *a && rr.ts <= *b).map(|p| p.0.clone()).unwrap_or_else(|| {
            match mot {
                Some((_, vz, _, _)) if vz < -150.0 => "falling".to_string(),
                Some((_, _, hand, tip)) if hand > 500.0 || tip > 900.0 => "swinging".to_string(),
                Some((pel, _, _, _)) if pel > 60.0 => "walking".to_string(),
                Some((pel, _, hand, _)) if pel < 30.0 && hand < 80.0 => "idle".to_string(),
                Some(_) => "other".to_string(),
                None => "edge".to_string(),
            }
        });
        let acc = rep.classes.entry(class).or_default();
        acc.frames += 1;
        if let Some((_, _, hand, _)) = mot { acc.hand_speed.push(hand); }
        if let (Some(ru), Some(su)) = (rr.utc, tutc) { acc.lat.push(ru - su); }
        // JITTER: second differences of every body, stand-in on its display
        // clock (receiver UTC), truth at the same labels on the sender clock.
        if ri >= 1 && ri + 1 < r.len() {
            let (a, c) = (&r[ri - 1], &r[ri + 1]);
            if let (Some(ua), Some(ub), Some(uc)) = (a.utc, rr.utc, c.utc) {
                let (d1, d2) = ((ub - ua) / 1000.0, (uc - ub) / 1000.0);
                let (l1, l2) = ((rr.ts - a.ts) / 1000.0, (c.ts - rr.ts) / 1000.0);
                if d1 > 0.003 && d2 > 0.003 && d1 < 0.05 && d2 < 0.05 && l1 > 0.001 && l2 > 0.001 {
                    if let (Some((ta, _, _)), Some((tc, _, _))) = (truth_at(s, a.ts), truth_at(s, c.ts)) {
                        if acc.hf_r.is_empty() { acc.hf_r = vec![0.0; NB]; acc.hf_t = vec![0.0; NB]; }
                        acc.hf_n += 1.0;
                        for i in 0..NB {
                            let mut sr = 0.0; let mut st = 0.0;
                            for k in 0..3 {
                                let ar = ((c.b[i][k] - rr.b[i][k]) / d2 - (rr.b[i][k] - a.b[i][k]) / d1) / ((d1 + d2) / 2.0);
                                let at = ((tc[i][k] - tb[i][k]) / l2 - (tb[i][k] - ta[i][k]) / l1) / ((l1 + l2) / 2.0);
                                sr += ar * ar; st += at * at;
                            }
                            acc.hf_r[i] += sr; acc.hf_t[i] += st;
                        }
                    }
                }
            }
        }
        // MOONWALK: stand-in foot speed while the sender's foot is planted.
        for f in [19usize, 22] {
            if let (Some((t0, _, _)), Some((t1, _, _))) = (truth_at(s, rr.ts - 25.0), truth_at(s, rr.ts + 25.0)) {
                if d3(&t0[f], &t1[f]) / 0.05 < 5.0 {
                    if let Some(p) = r[..ri].iter().rev().find(|x| x.ts <= rr.ts - 40.0 && x.ts >= rr.ts - 80.0) {
                        acc.foot_slide.push(d3(&rr.b[f], &p.b[f]) / ((rr.ts - p.ts) / 1000.0));
                    }
                }
            }
        }
        let mut ok = true;
        for i in 0..NB {
            let e = d3(&rr.b[i], &tb[i]);
            let a = qang(&rr.b[i][3..7], &tb[i][3..7]);
            rep.per_bone[i].0.push(e);
            rep.per_bone[i].1.push(a);
            if judged(i) {
                acc.pos.push(e);
                acc.rot.push(a);
                if e > 3.0 || a > 3.0 { ok = false; }
            }
            if ARM.contains(&i) { acc.arm_pos.push(e); acc.arm_rot.push(a); }
            if HANDS.contains(&i) { acc.hand_pos.push(e); acc.hand_rot.push(a); }
        }
        if let (Some(x), Some(y)) = (rr.w, tw) {
            let e = d3(&x[3..6], &y[3..6]);
            acc.tip.push(e);
            if e > 5.0 { ok = false; }
        }
        if ok { acc.frames_ok += 1; }
    }
    // IDLE: the stand-in must be still while its owner is (RMS about the mean).
    let mut run: Vec<&Rec> = Vec::new();
    let flush = |run: &mut Vec<&Rec>, out: &mut Vec<f64>| {
        if run.len() >= 20 && run[run.len() - 1].ts - run[0].ts >= 1000.0 {
            let mut sum = 0.0; let mut n = 0.0;
            for &i in &[0usize, 12, 16] {
                let mut m = [0.0f64; 3];
                for x in run.iter() { for k in 0..3 { m[k] += x.b[i][k]; } }
                for k in 0..3 { m[k] /= run.len() as f64; }
                for x in run.iter() { let d = d3(&x.b[i], &m); sum += d * d; n += 1.0; }
            }
            out.push((sum / n).sqrt());
        }
        run.clear();
    };
    for rr in r {
        let still = match motion(s, rr.ts) { Some((pel, _, hand, _)) => pel < 2.0 && hand < 4.0, None => false };
        if still && run.last().map_or(true, |p| rr.ts - p.ts < 100.0) && run.first().map_or(true, |f| rr.ts - f.ts < 5000.0) {
            run.push(rr);
        } else {
            flush(&mut run, &mut rep.idle_rms);
            if still { run.push(rr); }
        }
    }
    flush(&mut run, &mut rep.idle_rms);
    rep
}

fn f3(v: &mut Vec<f64>) -> String { format!("{:6.2} {:6.2} {:7.2}", pct(v, 0.5), pct(v, 0.95), pct(v, 1.0)) }

pub fn print(rep: &mut Report) {
    let n = rep.idle_standin_speed.len();
    let p95 = pct(&mut rep.idle_standin_speed, 0.95);
    println!("smoothness / planted feet (targets: HF ratio <= 1.2 per body p95, planted-foot speed p95 <= 5 uu/s;");
    println!("  HF ratio = stand-in / owner acceleration energy with a visibility floor of 1000 uu/s^2 per axis = 0.05 uu at 70 fps; raw = no floor):");
    const HF_FLOOR: f64 = 3.0 * 1000.0 * 1000.0;
    for (k, a) in rep.classes.iter_mut() {
        let fl = HF_FLOOR * a.hf_n;
        let mut ratios: Vec<f64> = (0..a.hf_r.len()).filter(|&i| judged(i)).map(|i| ((a.hf_r[i] + fl) / (a.hf_t[i] + fl)).sqrt()).collect();
        let mut raw: Vec<f64> = (0..a.hf_r.len()).filter(|&i| judged(i) && a.hf_t[i] > 0.0).map(|i| (a.hf_r[i] / a.hf_t[i]).sqrt()).collect();
        let worst = ratios.iter().cloned().fold(0.0, f64::max);
        let wi = (0..a.hf_r.len()).filter(|&i| judged(i)).max_by(|&x, &y| ((a.hf_r[x] + fl) / (a.hf_t[x] + fl)).total_cmp(&((a.hf_r[y] + fl) / (a.hf_t[y] + fl)))).map(|i| BONES[i]).unwrap_or("-");
        println!("  {:<10} HF ratio p50 {:5.2} p95 {:5.2} max {:5.2} ({}) raw p50 {:5.2} | planted-foot speed p50 {:5.1} p95 {:5.1} uu/s (n {})",
            k, pct(&mut ratios, 0.5), pct(&mut ratios, 0.95), worst, wi, pct(&mut raw, 0.5), pct(&mut a.foot_slide, 0.5), pct(&mut a.foot_slide, 0.95), a.foot_slide.len());
    }
    println!("owner idle => stand-in idle: {} frames with the owner still, stand-in pelvis/hand speed p95 {:.1} uu/s -> {}", n, p95, if n == 0 { "n/a" } else if p95 <= 30.0 { "PASS" } else { "FAIL" });
    let nr = rep.idle_rms.len();
    let worst = rep.idle_rms.iter().cloned().fold(0.0, f64::max);
    println!("idle stillness: {} idle runs (1-5 s) with the owner still, stand-in RMS about its mean p50 {:.2} max {:.2} uu -> {}", nr, pct(&mut rep.idle_rms, 0.5), worst, if nr == 0 { "n/a" } else if worst <= 0.5 { "PASS" } else { "FAIL" });
    println!("matched {} receiver frames ({} outside the sender record)", rep.matched, rep.unmatched);
    println!("{:<10} {:>6} {:>7} {:>7} | {:<22}| {:<22}| {:<22}| {:<22}| {:<22}| {:<22}| {}",
        "class", "frames", "in-spec", "hand/s", "all bones uu p50/95/max", "all bones deg", "arm chain uu", "arm chain deg",
        "hands uu", "hands deg", "tip uu | latency ms p50/p95");
    for (k, a) in rep.classes.iter_mut() {
        let hs = pct(&mut a.hand_speed, 0.95);
        println!("{:<10} {:>6} {:>6.1}% {:>7.0} | {} | {} | {} | {} | {} | {} | {} | {:5.1} {:5.1}",
            k, a.frames, 100.0 * a.frames_ok as f64 / a.frames.max(1) as f64, hs,
            f3(&mut a.pos), f3(&mut a.rot), f3(&mut a.arm_pos), f3(&mut a.arm_rot),
            f3(&mut a.hand_pos), f3(&mut a.hand_rot), f3(&mut a.tip), pct(&mut a.lat, 0.5), pct(&mut a.lat, 0.95));
    }
    println!("per bone (all frames)  pos uu p50/p95/max | rot deg p50/p95/max");
    for (i, (p, r)) in rep.per_bone.iter_mut().enumerate() {
        println!("  {:<11} {} | {}{}", BONES[i], f3(p), f3(r), if judged(i) { "" } else { "  (no body, not judged)" });
    }
}

fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    if a.len() < 2 {
        eprintln!("usage: hsmp-pose-truth <.probe_send.txt> <.probe_recv<id>.txt> [--phases file] [--from ms] [--to ms]");
        std::process::exit(2);
    }
    let mut s = parse(&std::fs::read_to_string(&a[0]).expect("sender file"), "S");
    let mut r = parse(&std::fs::read_to_string(&a[1]).expect("receiver file"), "R");
    let mut phases = Vec::new();
    let mut i = 2;
    while i + 1 < a.len() {
        match a[i].as_str() {
            "--phases" => {
                if let Ok(t) = std::fs::read_to_string(&a[i + 1]) {
                    for l in t.lines() {
                        let v: Vec<&str> = l.split_whitespace().collect();
                        if v.len() == 3 { if let (Ok(x), Ok(y)) = (v[1].parse(), v[2].parse()) { phases.push((v[0].to_string(), x, y)); } }
                    }
                }
            }
            "--from" => { if let Ok(t) = a[i + 1].parse::<f64>() { r.retain(|x| x.ts >= t); } }
            "--to" => { if let Ok(t) = a[i + 1].parse::<f64>() { r.retain(|x| x.ts <= t); } }
            // Process start (unix ms) of each game: U = os.clock ms since start.
            "--send-start" => { if let Ok(t) = a[i + 1].parse::<f64>() { for x in s.iter_mut() { x.utc = x.utc.map(|u| u + t); } } }
            "--recv-start" => { if let Ok(t) = a[i + 1].parse::<f64>() { for x in r.iter_mut() { x.utc = x.utc.map(|u| u + t); } } }
            _ => { i += 1; continue; }
        }
        i += 2;
    }
    println!("sender {} frames ({:.0}..{:.0} ms), receiver {} frames", s.len(), s.first().map(|x| x.ts).unwrap_or(0.0),
        s.last().map(|x| x.ts).unwrap_or(0.0), r.len());
    // Frames from a window that was not rendering (minimized / hidden) are not
    // measured: bones do not refresh there, the numbers would be meaningless.
    let (ns, nr) = (s.iter().filter(|x| !x.vis).count(), r.iter().filter(|x| !x.vis).count());
    let frac = (ns as f64 / s.len().max(1) as f64).max(nr as f64 / r.len().max(1) as f64);
    println!("visibility: {} sender + {} receiver frames not rendered ({:.1}%) -> {}", ns, nr, 100.0 * frac,
        if frac > 0.05 { "INVALID RUN (window minimized / hidden)" } else { "ok" });
    r.retain(|x| x.vis);
    let mut rep = compare(&s, &r, &phases);
    print(&mut rep);
}

#[cfg(test)]
mod tests {
    use super::*;
    fn rec(tag: &str, ts: f64, x: f64, utc: f64) -> String {
        let mut s = format!("{} {}", tag, ts);
        for i in 0..NB { s += &format!(" {} {} {} 0 0 0 1", x + i as f64, 0.0, 100.0); }
        s + &format!(" W {} 0 0 {} 0 0 U {}", x, x + 100.0, utc)
    }
    #[test]
    fn interpolates_the_truth_scores_and_measures_latency() {
        let send: String = (0..100).map(|k| rec("S", k as f64 * 10.0, k as f64 * 1.0, 1_000_000.0 + k as f64 * 10.0) + "\n").collect();
        let recv = rec("R", 505.0, 50.5 + 1.0, 1_000_000.0 + 505.0 + 37.0) + "\n";
        let s = parse(&send, "S");
        let r = parse(&recv, "R");
        assert_eq!((s.len(), r.len()), (100, 1));
        let mut rep = compare(&s, &r, &[("test".into(), 0.0, 1000.0)]);
        let a = rep.classes.get_mut("test").unwrap();
        assert_eq!(a.frames, 1);
        assert!((pct(&mut a.pos, 1.0) - 1.0).abs() < 1e-9);
        assert!((pct(&mut a.tip, 1.0) - 1.0).abs() < 1e-9);
        assert!((pct(&mut a.lat, 0.5) - 37.0).abs() < 1e-6);
        assert_eq!(a.frames_ok, 1);
    }
}
