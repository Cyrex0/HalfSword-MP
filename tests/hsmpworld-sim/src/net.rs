//! Link model: `hsmp-tools netsim`'s profiles (tools/hsmp-tools/src/cmd/netsim.rs), applied
//! per direction like the proxy: delay + uniform ±jitter (+ a spike while a spike window is
//! open), loss %, duplicate %. Reordering falls out of jitter.
//!
//! Reliable messages (claims, owners, snapshots, manifests, syncs, verdicts) are retransmitted
//! by the connection layer after an RTO until a copy gets through (hsmp-net conn.rs:
//! srtt + 4·rttvar, at least 50 ms) and are handed over in order per link.

use crate::Rng;

/// (name, delay ms, jitter ms, loss %, dup %, spike every s, spike ms)
pub const PROFILES: &[(&str, f64, f64, f64, f64, f64, f64)] = &[
    ("lan", 1.0, 0.5, 0.0, 0.0, 0.0, 0.0),
    ("typical", 50.0, 12.0, 1.0, 0.3, 0.0, 0.0),
    // A long, healthy internet route (~176 ms RTT), as netsim's `intl`.
    ("intl", 88.0, 30.0, 0.3, 0.1, 0.0, 0.0),
    // ~300 ms RTT, 50 ms jitter, 2 % loss, as netsim's `far`.
    ("far", 150.0, 50.0, 2.0, 0.3, 0.0, 0.0),
    // netsim's own `bad` (220 ms RTT, 5 % loss, 200 ms spikes every 10 s).
    ("bad", 110.0, 40.0, 5.0, 1.0, 10.0, 200.0),
];
pub const SPIKE_LEN_MS: f64 = 300.0;

#[derive(Clone, Copy, Debug)]
pub struct Profile {
    pub name: &'static str,
    pub delay: f64,
    pub jitter: f64,
    pub loss: f64,
    pub dup: f64,
    pub spike_every: f64,
    pub spike_ms: f64,
}

impl Profile {
    pub fn get(name: &str) -> Profile {
        let p = PROFILES.iter().find(|p| p.0 == name).unwrap_or_else(|| panic!("unknown profile {name}"));
        Profile { name: p.0, delay: p.1, jitter: p.2, loss: p.3, dup: p.4, spike_every: p.5, spike_ms: p.6 }
    }
    pub fn rtt(&self) -> f64 { 2.0 * self.delay }
}

/// One direction of one client's path to the server.
pub struct Link {
    pub p: Profile,
    phase_ms: f64,
    rng: Rng,
    /// Reliable channel: arrival time of the last message handed over (in order).
    rel_last: f64,
    pub bytes: u64,
}

impl Link {
    pub fn new(p: Profile, rng: &mut Rng) -> Link {
        let phase_ms = rng.range(0.0, p.spike_every.max(1.0) * 1000.0);
        Link { p, phase_ms, rng: rng.fork(7), rel_last: 0.0, bytes: 0 }
    }

    fn spike_at(&self, t: f64) -> f64 {
        if self.p.spike_every > 0.0 && ((t + self.phase_ms) / 1000.0 % self.p.spike_every) * 1000.0 < SPIKE_LEN_MS {
            self.p.spike_ms
        } else {
            0.0
        }
    }

    fn one_delay(&mut self, t: f64) -> f64 {
        let j = if self.p.jitter > 0.0 { self.rng.range(-self.p.jitter, self.p.jitter) } else { 0.0 };
        (self.p.delay + self.spike_at(t) + j).max(0.2)
    }

    /// Unreliable datagram of `n` bytes sent at `t`: arrival times (empty = lost; two = duplicated).
    pub fn send(&mut self, t: f64, n: usize) -> Vec<f64> {
        self.bytes += n as u64 + 28;
        if self.rng.f() * 100.0 < self.p.loss {
            return Vec::new();
        }
        let mut v = vec![t + self.one_delay(t)];
        if self.rng.f() * 100.0 < self.p.dup {
            v.push(t + self.one_delay(t));
        }
        v
    }

    /// Reliable, ordered message sent at `t`: its hand-over time.
    pub fn send_reliable(&mut self, t: f64, n: usize) -> f64 {
        let rto = (self.p.rtt() * 1.5 + 4.0 * self.p.jitter).max(50.0);
        let mut at = t;
        loop {
            self.bytes += n as u64 + 28;
            if self.rng.f() * 100.0 >= self.p.loss {
                break;
            }
            at += rto;
        }
        let arrive = (at + self.one_delay(at)).max(self.rel_last);
        self.rel_last = arrive;
        arrive
    }
}
