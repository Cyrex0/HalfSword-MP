use rand::{rngs::StdRng, Rng, SeedableRng};
use serde::Serialize;

pub fn quantile(xs: &[f64], q: f64) -> Option<f64> {
    if xs.is_empty() {
        return None;
    }
    let mut x = xs.to_vec();
    x.sort_by(f64::total_cmp);
    let p = (x.len() - 1) as f64 * q.clamp(0.0, 1.0);
    let i = p.floor() as usize;
    Some(x[i] + (x[p.ceil() as usize] - x[i]) * (p - i as f64))
}

#[derive(Debug, Serialize)]
pub struct Interval {
    pub estimate: f64,
    pub low: f64,
    pub high: f64,
    pub samples_a: usize,
    pub samples_b: usize,
}

/// Independent bootstrap of run/window samples. Caller must not treat correlated
/// per-hit measurements as independent trials. Seeded for reproducible reviews.
pub fn difference(a: &[f64], b: &[f64], q: f64) -> Option<Interval> {
    let statistic = |x: &[f64]| {
        if q < 0.0 {
            if x.is_empty() {
                None
            } else {
                Some(x.iter().sum::<f64>() / x.len() as f64)
            }
        } else {
            quantile(x, q)
        }
    };
    let estimate = statistic(b)? - statistic(a)?;
    let mut rng = StdRng::seed_from_u64(0x48534d50);
    let mut deltas = Vec::with_capacity(2000);
    for _ in 0..2000 {
        let aa: Vec<_> = (0..a.len()).map(|_| a[rng.gen_range(0..a.len())]).collect();
        let bb: Vec<_> = (0..b.len()).map(|_| b[rng.gen_range(0..b.len())]).collect();
        deltas.push(statistic(&bb)? - statistic(&aa)?);
    }
    Some(Interval {
        estimate,
        low: quantile(&deltas, 0.025)?,
        high: quantile(&deltas, 0.975)?,
        samples_a: a.len(),
        samples_b: b.len(),
    })
}

/// Paired resampling preserves the native/replay association. Undefined when
/// native change sums to zero; zero damage is never called perfect parity.
pub fn ratio(pairs: &[[f64; 2]]) -> Option<Interval> {
    let native: f64 = pairs.iter().map(|p| p[0]).sum();
    if native.abs() < 1e-9 {
        return None;
    }
    let estimate = pairs.iter().map(|p| p[1]).sum::<f64>() / native;
    let mut rng = StdRng::seed_from_u64(0x50414952);
    let mut samples = Vec::new();
    for _ in 0..2000 {
        let (mut n, mut r) = (0.0, 0.0);
        for _ in pairs {
            let p = pairs[rng.gen_range(0..pairs.len())];
            n += p[0];
            r += p[1];
        }
        if n.abs() > 1e-9 {
            samples.push(r / n);
        }
    }
    Some(Interval {
        estimate,
        low: quantile(&samples, 0.025)?,
        high: quantile(&samples, 0.975)?,
        samples_a: pairs.len(),
        samples_b: pairs.len(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn evidence_not_zero_damage() {
        assert!(ratio(&[[0.0, 0.0]]).is_none());
        let r = ratio(&[[-2.0, -2.0], [-3.0, -3.0]]).unwrap();
        assert_eq!((r.estimate, r.low, r.high), (1.0, 1.0, 1.0));
        let d = difference(&[1.0; 20], &[3.0; 20], 0.9).unwrap();
        assert_eq!((d.estimate, d.low, d.high), (2.0, 2.0, 2.0));
    }
}
