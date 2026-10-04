//! Test only (`--emulate-nat`): the server's socket behaves as if a home router sat in
//! front of it. The emulated NAT keeps the port (endpoint-independent mapping) and filters
//! by address and port: a datagram from an endpoint the server has not sent to in the last
//! 60 s is dropped, as a router drops unsolicited inbound UDP. Traffic of an admitted flow
//! keeps it open. scripts/e2e-nat.sh uses it to prove that a direct join fails and a
//! punched join succeeds.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

const MAPPING_TIMEOUT: Duration = Duration::from_secs(60);

/// The filter state: when the inside last sent to (or heard from, on an open flow) each
/// outside endpoint.
#[derive(Default)]
pub struct Filter {
    open: HashMap<SocketAddr, Instant>,
}

impl Filter {
    pub fn note_outbound(&mut self, to: SocketAddr, now: Instant) {
        if self.open.len() > 10_000 {
            self.open.retain(|_, t| now.duration_since(*t) < MAPPING_TIMEOUT);
        }
        self.open.insert(to, now);
    }

    pub fn inbound_ok(&mut self, from: SocketAddr, now: Instant) -> bool {
        match self.open.get_mut(&from) {
            Some(at) if now.duration_since(*at) < MAPPING_TIMEOUT => {
                // an admitted flow: the server answers it, which keeps the pinhole open
                *at = now;
                true
            }
            _ => false,
        }
    }
}

static ON: AtomicBool = AtomicBool::new(false);
static DROPPED: AtomicU64 = AtomicU64::new(0);

fn filter() -> &'static Mutex<Filter> {
    static F: OnceLock<Mutex<Filter>> = OnceLock::new();
    F.get_or_init(Default::default)
}

pub fn set(on: bool) {
    ON.store(on, Ordering::Relaxed);
    if on {
        tracing::warn!("--emulate-nat: inbound datagrams from endpoints this server has not sent to are DROPPED (test only)");
    }
}

/// The server sends to `to` (STUN, probes): the emulated NAT opens for it.
pub fn note_outbound(to: SocketAddr) {
    if ON.load(Ordering::Relaxed) {
        filter().lock().unwrap_or_else(|e| e.into_inner()).note_outbound(to, Instant::now());
    }
}

/// Would the emulated NAT let this datagram in? Always true when emulation is off.
#[inline]
pub fn inbound_ok(from: SocketAddr) -> bool {
    if !ON.load(Ordering::Relaxed) {
        return true;
    }
    if filter().lock().unwrap_or_else(|e| e.into_inner()).inbound_ok(from, Instant::now()) {
        return true;
    }
    let n = DROPPED.fetch_add(1, Ordering::Relaxed);
    if n < 5 || n % 100 == 0 {
        tracing::info!(%from, dropped = n + 1, "emulated NAT: unsolicited inbound datagram dropped");
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn filters_by_address_and_port_and_expires() {
        let a: SocketAddr = "127.0.0.1:40000".parse().unwrap();
        let b: SocketAddr = "127.0.0.1:40001".parse().unwrap();
        let t = Instant::now();
        let mut f = Filter::default();
        assert!(!f.inbound_ok(a, t), "unsolicited");
        f.note_outbound(a, t);
        assert!(f.inbound_ok(a, t + Duration::from_secs(1)), "after the probe");
        assert!(!f.inbound_ok(b, t), "same IP, other port: still filtered");
        // traffic keeps it open, silence closes it
        assert!(f.inbound_ok(a, t + Duration::from_secs(50)));
        assert!(f.inbound_ok(a, t + Duration::from_secs(100)));
        assert!(!f.inbound_ok(a, t + Duration::from_secs(161)));
        // the global switch is off in tests: everything passes
        assert!(inbound_ok(b));
    }
}
