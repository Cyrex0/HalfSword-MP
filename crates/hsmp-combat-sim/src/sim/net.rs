//! Link model. Profiles are `hsmp-tools netsim`'s PROFILES (tools/hsmp-tools/
//! src/cmd/netsim.rs, docs/development/subsystems/replication.md), applied per direction exactly
//! like the proxy: delay + uniform ±jitter (+ spike while a spike window is
//! open: every `spike_every` s for SPIKE_LEN_MS), loss %, duplicate %.
//! Reordering falls out of jitter.
//!
//! Reliable messages (damage, damage_in, acks, deaths: records on `Chan::Reliable`
//! channel) are retransmitted by the connection layer after an RTO
//! (hsmp-net conn.rs: srtt + 4·rttvar, ≥ 50 ms) until one copy gets through.

use super::Rng;

/// (name, delay ms, jitter ms, loss %, dup %, spike every s, spike ms)
pub const PROFILES: &[(&str, f64, f64, f64, f64, f64, f64)] = &[
    ("lan", 1.0, 0.5, 0.0, 0.0, 0.0, 0.0),
    ("good", 20.0, 4.0, 0.2, 0.1, 0.0, 0.0),
    ("typical", 50.0, 12.0, 1.0, 0.3, 0.0, 0.0),
    ("wifi", 35.0, 25.0, 2.0, 0.5, 7.0, 120.0),
    ("intl", 88.0, 30.0, 0.3, 0.1, 0.0, 0.0),
    ("bad", 110.0, 40.0, 5.0, 1.0, 10.0, 200.0),
    // A far server (~300 ms RTT, 50 ms jitter, 2 % loss): the high-ping case of SMOOTH-1.
    ("far", 150.0, 50.0, 2.0, 0.3, 0.0, 0.0),
    ("awful", 180.0, 70.0, 10.0, 2.0, 5.0, 300.0),
];
/// netsim --spike-len default.
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
        let p = PROFILES.iter().find(|p| p.0 == name).unwrap_or_else(|| panic!("profile {name}"));
        Profile { name: p.0, delay: p.1, jitter: p.2, loss: p.3, dup: p.4, spike_every: p.5, spike_ms: p.6 }
    }
    /// "loopback": same machine, no proxy (IPC + scheduler only).
    pub fn loopback() -> Profile {
        Profile { name: "loopback", delay: 0.3, jitter: 0.3, loss: 0.0, dup: 0.0, spike_every: 0.0, spike_ms: 0.0 }
    }
    /// Nominal RTT through a client's two links (no spikes).
    pub fn rtt(&self) -> f64 { 2.0 * self.delay }
}

/// One direction of one client's path to the server.
#[derive(Clone)]
pub struct Link {
    pub p: Profile,
    /// Spike phase (each proxy starts its spike clock at its own time).
    pub phase_ms: f64,
    rng: Rng,
}

impl Link {
    pub fn new(p: Profile, rng: &mut Rng) -> Link {
        let phase_ms = rng.range(0.0, p.spike_every.max(1.0) * 1000.0);
        Link { p, phase_ms, rng: rng.fork(7) }
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
        (self.p.delay + self.spike_at(t) + j).max(0.0)
    }

    /// Unreliable datagram sent at `t`: arrival times (empty = lost; two
    /// entries = duplicated).
    pub fn send(&mut self, t: f64) -> Vec<f64> {
        if self.rng.f() * 100.0 < self.p.loss {
            return Vec::new();
        }
        let copies = if self.rng.f() * 100.0 < self.p.dup { 2 } else { 1 };
        (0..copies).map(|_| t + self.one_delay(t)).collect()
    }

    /// Reliable message sent at `t`: first arrival (retransmits every RTO
    /// until a copy survives), plus a possible duplicate.
    pub fn send_reliable(&mut self, t: f64, rto: f64) -> Vec<f64> {
        let mut at = t;
        for _ in 0..40 {
            let a = self.send(at);
            if !a.is_empty() {
                return a;
            }
            at += rto;
        }
        Vec::new()
    }

    /// Connection-layer RTO for this path (srtt + 4·rttvar ≈ RTT + 2·jitter·2).
    pub fn rto(&self, other: &Link) -> f64 {
        (self.p.delay + other.p.delay + 2.0 * (self.p.jitter + other.p.jitter)).max(50.0)
    }

    pub fn rng(&mut self) -> &mut Rng { &mut self.rng }
}
