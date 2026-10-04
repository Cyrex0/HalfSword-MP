//! Token buckets for the rate limits (time is passed in: the core has no clock).

#[derive(Debug, Clone, Copy)]
pub(crate) struct Bucket {
    tokens: f64,
    at: u64,
}

impl Bucket {
    pub(crate) fn full(cap: f64, now: u64) -> Bucket {
        Bucket { tokens: cap, at: now }
    }

    pub(crate) fn take(&mut self, now: u64, cap: f64, every_ms: u64) -> bool {
        let dt = now.saturating_sub(self.at) as f64;
        self.tokens = (self.tokens + dt / every_ms.max(1) as f64).min(cap);
        self.at = now;
        if self.tokens >= 1.0 {
            self.tokens -= 1.0;
            true
        } else {
            false
        }
    }

    pub(crate) fn is_full(&self, now: u64, cap: f64, every_ms: u64) -> bool {
        self.tokens + now.saturating_sub(self.at) as f64 / every_ms.max(1) as f64 >= cap
    }
}
