//! Deterministic impaired link for tests: loss, duplication, delay with
//! jitter (which reorders), seeded so failures reproduce.

use rand::rngs::StdRng;
use rand::{Rng, SeedableRng};

pub struct Link {
    rng: StdRng,
    pub loss: f64,
    pub dup: f64,
    pub delay_ms: u64,
    pub jitter_ms: u64,
    q: Vec<(u64, u64, Vec<u8>)>,
    order: u64,
    pub sent: u64,
    pub delivered: u64,
}

impl Link {
    pub fn new(seed: u64, loss: f64, dup: f64, delay_ms: u64, jitter_ms: u64) -> Self {
        Link {
            rng: StdRng::seed_from_u64(seed),
            loss,
            dup,
            delay_ms,
            jitter_ms,
            q: Vec::new(),
            order: 0,
            sent: 0,
            delivered: 0,
        }
    }

    pub fn perfect() -> Self {
        Link::new(0, 0.0, 0.0, 1, 0)
    }

    pub fn push(&mut self, now: u64, dg: Vec<u8>) {
        self.sent += 1;
        if self.rng.gen_bool(self.loss) {
            return;
        }
        let copies = if self.rng.gen_bool(self.dup) { 2 } else { 1 };
        for _ in 0..copies {
            let j = if self.jitter_ms > 0 {
                self.rng.gen_range(0..=self.jitter_ms)
            } else {
                0
            };
            self.order += 1;
            self.q.push((now + self.delay_ms + j, self.order, dg.clone()));
        }
    }

    pub fn pop_due(&mut self, now: u64) -> Vec<Vec<u8>> {
        let mut due: Vec<(u64, u64, Vec<u8>)> = Vec::new();
        let mut i = 0;
        while i < self.q.len() {
            if self.q[i].0 <= now {
                due.push(self.q.swap_remove(i));
            } else {
                i += 1;
            }
        }
        due.sort_by_key(|(t, o, _)| (*t, *o));
        self.delivered += due.len() as u64;
        due.into_iter().map(|(_, _, d)| d).collect()
    }
}
