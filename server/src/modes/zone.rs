//! Shrinking zone: pure, deterministic, unit-tested. Used by the Duel/LTS
//! sudden-death ring now and by BR later.
//!
//! Units: positions and radii in UE centimetres, times in server ms, damage
//! in Willie Health per second (Health = 100). The zone is a vertical
//! cylinder: only X/Y count.
//!
//! A table is a list of phases. Phase `i` first WAITS `wait_ms` at radius
//! `r_from_cm` around `centers[i]`, then SHRINKS over `shrink_ms` to
//! `r_to_cm` while the centre slides linearly to `centers[i + 1]`. Outside
//! the circle a player takes `dps` of phase `i` for the whole phase. After
//! the last phase the zone is `finished`: it stays at the final circle and
//! the last phase's dps.

use super::traits::{Ms, SeatId};
use std::collections::BTreeMap;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ZonePhase {
    pub wait_ms: Ms,
    pub shrink_ms: Ms,
    pub r_from_cm: f32,
    pub r_to_cm: f32,
    pub dps: f32,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ZoneTable {
    pub name: &'static str,
    pub phases: Vec<ZonePhase>,
}

impl ZoneTable {
    /// Rows of (wait s, shrink s, radius from m, radius to m, dps).
    pub fn from_metres(name: &'static str, rows: &[(u32, u32, f32, f32, f32)]) -> Self {
        let phases = rows.iter().map(|&(w, s, a, b, d)| ZonePhase {
            wait_ms: w as Ms * 1000,
            shrink_ms: s as Ms * 1000,
            r_from_cm: a * 100.0,
            r_to_cm: b * 100.0,
            dps: d,
        }).collect();
        Self { name, phases }
    }

    /// Forest Ambush, 16 players.
    pub fn ambush16() -> Self {
        Self::from_metres("ambush16", &[
            (90, 0, 38.0, 38.0, 0.0),
            (60, 45, 38.0, 26.0, 1.0),
            (45, 40, 26.0, 16.0, 2.0),
            (30, 30, 16.0, 9.0, 4.0),
            (20, 20, 9.0, 4.0, 7.0),
            (10, 30, 4.0, 0.0, 12.0),
        ])
    }

    /// Duel / LTS sudden death: a 3 m ring around the map
    /// centre shrinks to 1.5 m over 30 s; outside costs 5 Health/s.
    pub fn sudden_death() -> Self {
        Self::from_metres("sudden_death", &[(0, 30, 3.0, 1.5, 5.0)])
    }

    pub fn total_ms(&self) -> Ms {
        self.phases.iter().map(|p| p.wait_ms + p.shrink_ms).sum()
    }

    /// Radii non-negative and non-increasing, phases continuous
    /// (`r_from[i+1] == r_to[i]`), damage non-negative and non-decreasing.
    pub fn validate(&self) -> Result<(), String> {
        if self.phases.is_empty() {
            return Err(format!("zone table {}: no phases", self.name));
        }
        for (i, p) in self.phases.iter().enumerate() {
            if !(p.r_to_cm >= 0.0 && p.r_to_cm <= p.r_from_cm) {
                return Err(format!("zone table {}: phase {i} radius grows or is negative", self.name));
            }
            if p.dps.is_nan() || p.dps < 0.0 {
                return Err(format!("zone table {}: phase {i} negative dps", self.name));
            }
            if p.shrink_ms == 0 && p.r_to_cm != p.r_from_cm {
                return Err(format!("zone table {}: phase {i} jumps radius with no shrink time", self.name));
            }
            if i > 0 {
                let prev = &self.phases[i - 1];
                if (p.r_from_cm - prev.r_to_cm).abs() > 1e-3 {
                    return Err(format!("zone table {}: phase {i} is not continuous with {}", self.name, i - 1));
                }
                if p.dps < prev.dps {
                    return Err(format!("zone table {}: phase {i} dps decreases", self.name));
                }
            }
        }
        Ok(())
    }
}

/// The zone at one instant.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ZoneSample {
    pub phase: usize,
    pub center: [f32; 2],
    pub radius_cm: f32,
    /// Where the current phase ends up.
    pub next_center: [f32; 2],
    pub next_radius_cm: f32,
    pub shrinking: bool,
    /// Server ms at which the current wait/shrink segment ends
    /// (= the zone end once finished).
    pub segment_end_ms: Ms,
    pub dps: f32,
    pub finished: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Zone {
    table: ZoneTable,
    start_ms: Ms,
    /// `phases.len() + 1` centres: phase `i` shrinks from `centers[i]` to
    /// `centers[i + 1]`.
    centers: Vec<[f32; 2]>,
}

fn dist2(a: [f32; 2], b: [f32; 2]) -> f32 {
    let (dx, dy) = (a[0] - b[0], a[1] - b[1]);
    (dx * dx + dy * dy).sqrt()
}

fn lerp2(a: [f32; 2], b: [f32; 2], f: f32) -> [f32; 2] {
    [a[0] + (b[0] - a[0]) * f, a[1] + (b[1] - a[1]) * f]
}

/// Tiny deterministic generator (xorshift64*), so the zone needs no RNG crate
/// and a published seed reproduces every centre.
#[derive(Clone, Copy, Debug)]
pub struct ZoneRng(u64);

impl ZoneRng {
    pub fn new(seed: u64) -> Self {
        Self(seed ^ 0x9E37_79B9_7F4A_7C15 | 1)
    }
    pub fn next_u64(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }
}

/// Pick the next centre: a candidate within `r_cur - r_next` of `cur` (so the
/// next circle lies inside the current one). Deterministic for a given rng
/// state and candidate order. Falls back to `cur` when none qualifies.
pub fn pick_next_center(cur: [f32; 2], r_cur: f32, r_next: f32, candidates: &[[f32; 2]], rng: &mut ZoneRng) -> [f32; 2] {
    let max_d = (r_cur - r_next).max(0.0);
    let ok: Vec<[f32; 2]> = candidates.iter().copied().filter(|c| dist2(*c, cur) <= max_d).collect();
    if ok.is_empty() {
        return cur;
    }
    ok[(rng.next_u64() % ok.len() as u64) as usize]
}

impl Zone {
    /// Explicit centres (`phases.len() + 1` of them). Every phase's end circle
    /// must lie inside its start circle.
    pub fn new(table: ZoneTable, start_ms: Ms, centers: Vec<[f32; 2]>) -> Result<Self, String> {
        table.validate()?;
        if centers.len() != table.phases.len() + 1 {
            return Err(format!("zone: need {} centres, got {}", table.phases.len() + 1, centers.len()));
        }
        for (i, p) in table.phases.iter().enumerate() {
            if dist2(centers[i], centers[i + 1]) > p.r_from_cm - p.r_to_cm + 1e-2 {
                return Err(format!("zone: phase {i} end circle leaves its start circle"));
            }
        }
        Ok(Self { table, start_ms, centers })
    }

    /// Every phase centred on `c` (sudden death).
    pub fn fixed(table: ZoneTable, start_ms: Ms, c: [f32; 2]) -> Result<Self, String> {
        let n = table.phases.len() + 1;
        Self::new(table, start_ms, vec![c; n])
    }

    /// Centres drawn from `candidates` with a seeded rng (BR).
    pub fn seeded(table: ZoneTable, start_ms: Ms, c0: [f32; 2], candidates: &[[f32; 2]], seed: u64) -> Result<Self, String> {
        table.validate()?;
        let mut rng = ZoneRng::new(seed);
        let mut centers = vec![c0];
        for p in &table.phases {
            let cur = *centers.last().unwrap();
            centers.push(pick_next_center(cur, p.r_from_cm, p.r_to_cm, candidates, &mut rng));
        }
        Self::new(table, start_ms, centers)
    }

    pub fn table(&self) -> &ZoneTable {
        &self.table
    }
    pub fn start_ms(&self) -> Ms {
        self.start_ms
    }
    pub fn end_ms(&self) -> Ms {
        self.start_ms + self.table.total_ms()
    }
    pub fn finished(&self, now: Ms) -> bool {
        now >= self.end_ms()
    }

    pub fn sample(&self, now: Ms) -> ZoneSample {
        let mut t = now.saturating_sub(self.start_ms);
        let mut seg_start = self.start_ms;
        for (i, p) in self.table.phases.iter().enumerate() {
            let (c0, c1) = (self.centers[i], self.centers[i + 1]);
            if t < p.wait_ms {
                return ZoneSample {
                    phase: i, center: c0, radius_cm: p.r_from_cm, next_center: c1, next_radius_cm: p.r_to_cm,
                    shrinking: false, segment_end_ms: seg_start + p.wait_ms, dps: p.dps, finished: false,
                };
            }
            if t < p.wait_ms + p.shrink_ms {
                let f = (t - p.wait_ms) as f32 / p.shrink_ms as f32;
                return ZoneSample {
                    phase: i, center: lerp2(c0, c1, f), radius_cm: p.r_from_cm + (p.r_to_cm - p.r_from_cm) * f,
                    next_center: c1, next_radius_cm: p.r_to_cm, shrinking: true,
                    segment_end_ms: seg_start + p.wait_ms + p.shrink_ms, dps: p.dps, finished: false,
                };
            }
            t -= p.wait_ms + p.shrink_ms;
            seg_start += p.wait_ms + p.shrink_ms;
        }
        let last = self.table.phases.len() - 1;
        let p = &self.table.phases[last];
        let c = self.centers[last + 1];
        ZoneSample {
            phase: last, center: c, radius_cm: p.r_to_cm, next_center: c, next_radius_cm: p.r_to_cm,
            shrinking: false, segment_end_ms: self.end_ms(), dps: p.dps, finished: true,
        }
    }

    /// Strictly outside the circle (on the edge counts as inside).
    pub fn is_outside(&self, pos: [f32; 3], now: Ms) -> bool {
        let s = self.sample(now);
        dist2([pos[0], pos[1]], s.center) > s.radius_cm
    }

    /// Damage per second at `pos` (0 inside).
    pub fn dps_at(&self, pos: [f32; 3], now: Ms) -> f32 {
        let s = self.sample(now);
        if dist2([pos[0], pos[1]], s.center) > s.radius_cm { s.dps } else { 0.0 }
    }

    /// Distance to the edge: positive outside, negative inside (HUD).
    pub fn edge_distance_cm(&self, pos: [f32; 3], now: Ms) -> f32 {
        let s = self.sample(now);
        dist2([pos[0], pos[1]], s.center) - s.radius_cm
    }
}

/// Server backstop for zone damage: every `STEP_MS`, each
/// listed seat whose position has been outside for at least `GRACE_MS`
/// takes `dps * STEP_MS / 1000` as a synthetic ledger entry. The client
/// applies the same grace, so both agree within one RTT.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ZoneDamage {
    next_step_ms: Option<Ms>,
    outside_since: BTreeMap<SeatId, Ms>,
}

pub const ZONE_STEP_MS: Ms = 100;
pub const ZONE_GRACE_MS: Ms = 500;
/// Catch-up bound after a stalled tick (2 s of steps); older steps are dropped.
const MAX_CATCHUP_STEPS: u32 = 20;

impl ZoneDamage {
    pub fn new() -> Self {
        Self::default()
    }

    /// Run every whole step up to `now`. `seats` = alive seats with a known
    /// position (the latest sample is used for every pending step). Returns
    /// the damage per seat for this call (only non-zero entries, seat order).
    pub fn step(&mut self, zone: &Zone, now: Ms, seats: &[(SeatId, [f32; 3])]) -> Vec<(SeatId, f32)> {
        let mut ts = *self.next_step_ms.get_or_insert(zone.start_ms());
        if now >= ts && (now - ts) / ZONE_STEP_MS > MAX_CATCHUP_STEPS as Ms {
            ts = now - MAX_CATCHUP_STEPS as Ms * ZONE_STEP_MS;
        }
        self.outside_since.retain(|s, _| seats.iter().any(|(x, _)| x == s));
        let mut dmg: BTreeMap<SeatId, f32> = BTreeMap::new();
        while ts <= now {
            let sample = zone.sample(ts);
            for &(seat, pos) in seats {
                let out = dist2([pos[0], pos[1]], sample.center) > sample.radius_cm;
                if out {
                    let since = *self.outside_since.entry(seat).or_insert(ts);
                    if ts - since >= ZONE_GRACE_MS && sample.dps > 0.0 {
                        *dmg.entry(seat).or_insert(0.0) += sample.dps * ZONE_STEP_MS as f32 / 1000.0;
                    }
                } else {
                    self.outside_since.remove(&seat);
                }
            }
            ts += ZONE_STEP_MS;
        }
        self.next_step_ms = Some(ts);
        dmg.into_iter().filter(|(_, d)| *d > 0.0).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builtin_tables_are_valid() {
        ZoneTable::ambush16().validate().unwrap();
        ZoneTable::sudden_death().validate().unwrap();
        assert_eq!(ZoneTable::ambush16().total_ms(), 420_000);
        assert_eq!(ZoneTable::sudden_death().total_ms(), 30_000);
    }

    #[test]
    fn invalid_tables_rejected() {
        let grow = ZoneTable::from_metres("x", &[(1, 1, 5.0, 6.0, 1.0)]);
        assert!(grow.validate().is_err());
        let gap = ZoneTable::from_metres("x", &[(1, 1, 5.0, 4.0, 1.0), (1, 1, 3.0, 2.0, 1.0)]);
        assert!(gap.validate().is_err());
        let less_dps = ZoneTable::from_metres("x", &[(1, 1, 5.0, 4.0, 2.0), (1, 1, 4.0, 2.0, 1.0)]);
        assert!(less_dps.validate().is_err());
        let jump = ZoneTable::from_metres("x", &[(1, 0, 5.0, 4.0, 2.0)]);
        assert!(jump.validate().is_err());
        assert!(ZoneTable { name: "e", phases: vec![] }.validate().is_err());
    }

    #[test]
    fn phase_walk_and_interpolation() {
        let z = Zone::fixed(ZoneTable::ambush16(), 1_000, [0.0, 0.0]).unwrap();
        let s = z.sample(0); // before start = start
        assert_eq!((s.phase, s.radius_cm, s.shrinking), (0, 3800.0, false));
        let s = z.sample(1_000 + 90_000); // phase 1 wait begins
        assert_eq!((s.phase, s.radius_cm, s.shrinking, s.dps), (1, 3800.0, false, 1.0));
        assert_eq!(s.segment_end_ms, 1_000 + 150_000);
        let s = z.sample(1_000 + 150_000 + 22_500); // half of phase 1 shrink
        assert!(s.shrinking);
        assert!((s.radius_cm - 3200.0).abs() < 1e-3, "{}", s.radius_cm);
        let s = z.sample(1_000 + 420_000);
        assert!(s.finished && s.radius_cm == 0.0 && s.dps == 12.0);
        assert!(z.finished(1_000 + 420_000) && !z.finished(1_000 + 419_999));
    }

    #[test]
    fn radius_never_grows_and_center_stays_inside() {
        let cands: Vec<[f32; 2]> = (0..400).map(|i| [((i * 37) % 60) as f32 * 100.0 - 3000.0, ((i * 91) % 60) as f32 * 100.0 - 3000.0]).collect();
        for seed in 0..20 {
            let z = Zone::seeded(ZoneTable::ambush16(), 0, [0.0, 0.0], &cands, seed).unwrap();
            let mut prev = z.sample(0);
            let mut t = 0;
            while t <= z.end_ms() + 1_000 {
                let s = z.sample(t);
                assert!(s.radius_cm <= prev.radius_cm + 1e-3, "seed {seed} t {t}");
                // The current circle always contains the circle it shrinks to.
                assert!(dist2(s.center, s.next_center) <= s.radius_cm - s.next_radius_cm + 0.05);
                prev = s;
                t += 250;
            }
        }
    }

    #[test]
    fn seeded_is_deterministic() {
        let cands: Vec<[f32; 2]> = (0..50).map(|i| [i as f32 * 40.0, -(i as f32) * 25.0]).collect();
        let a = Zone::seeded(ZoneTable::ambush16(), 0, [0.0, 0.0], &cands, 42).unwrap();
        let b = Zone::seeded(ZoneTable::ambush16(), 0, [0.0, 0.0], &cands, 42).unwrap();
        assert_eq!(a, b);
    }

    #[test]
    fn outside_and_dps() {
        let z = Zone::fixed(ZoneTable::sudden_death(), 0, [100.0, 0.0]).unwrap();
        assert!(!z.is_outside([100.0, 299.0, 0.0], 0));
        assert!(!z.is_outside([100.0, 300.0, 50_000.0], 0), "edge is inside; Z ignored");
        assert!(z.is_outside([100.0, 301.0, 0.0], 0));
        assert_eq!(z.dps_at([100.0, 301.0, 0.0], 0), 5.0);
        assert_eq!(z.dps_at([100.0, 0.0, 0.0], 0), 0.0);
        // At the end the ring is 150 cm.
        assert!(z.is_outside([100.0, 200.0, 0.0], 30_000));
        assert!((z.edge_distance_cm([100.0, 200.0, 0.0], 30_000) - 50.0).abs() < 1e-3);
    }

    #[test]
    fn damage_respects_grace_and_rate() {
        let z = Zone::fixed(ZoneTable::from_metres("t", &[(0, 0, 1.0, 1.0, 2.0)]), 0, [0.0, 0.0]).unwrap();
        let mut d = ZoneDamage::new();
        let out = [(7u32, [500.0f32, 0.0, 0.0])];
        let inside = [(7u32, [0.0f32, 0.0, 0.0])];
        // Steps at 0,100,...,1000 (11 steps); grace covers 0..400 => 6 damaging steps.
        let r = d.step(&z, 1_000, &out);
        assert_eq!(r.len(), 1);
        assert!((r[0].1 - 6.0 * 0.2).abs() < 1e-4, "{:?}", r);
        // Stepping inside resets the grace.
        assert!(d.step(&z, 1_100, &inside).is_empty());
        let r = d.step(&z, 1_500, &out); // outside since 1200: 1200..1500 under grace
        assert!(r.is_empty(), "{:?}", r);
        let r = d.step(&z, 1_700, &out); // 1700 is 500 ms after 1200
        assert!((r[0].1 - 0.2).abs() < 1e-4);
        // A seat not listed (dead) gets nothing and loses its grace state.
        assert!(d.step(&z, 2_000, &[]).is_empty());
    }

    #[test]
    fn damage_catch_up_is_bounded() {
        let z = Zone::fixed(ZoneTable::from_metres("t", &[(0, 0, 1.0, 1.0, 10.0)]), 0, [0.0, 0.0]).unwrap();
        let mut d = ZoneDamage::new();
        let r = d.step(&z, 60_000, &[(1, [999.0, 0.0, 0.0])]);
        // At most 21 steps considered, minus grace.
        assert!(r[0].1 <= 21.0 * 1.0 + 1e-3, "{:?}", r);
    }

    #[test]
    fn next_center_inside_or_fallback() {
        let mut rng = ZoneRng::new(1);
        assert_eq!(pick_next_center([0.0, 0.0], 100.0, 90.0, &[[500.0, 0.0]], &mut rng), [0.0, 0.0]);
        let c = pick_next_center([0.0, 0.0], 100.0, 50.0, &[[500.0, 0.0], [30.0, 30.0]], &mut rng);
        assert_eq!(c, [30.0, 30.0]);
    }
}
