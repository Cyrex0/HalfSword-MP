//! Server and client endpoints: the glue a socket loop drives.
//!
//! Server loop sketch:
//! ```ignore
//! match ep.handle(now, from, &buf[..n]) {
//!     Incoming::Reply(bytes) => sock.send_to(&bytes, from),
//!     Incoming::AuthRequest(p) => match policy(&p) {          // ban, full, password, seat
//!         Ok(()) => { let cid = ep.accept(now, p); ep.send(cid, SendMode::Ordered, welcome) }
//!         Err((code, text)) => sock.send_to(&ep.reject(now, p, code, &text), from),
//!     },
//!     Incoming::Data { cid, deliveries, migrated_from, probe } => {
//!         if let Some(p) = probe { sock.send_to(&p, from) }      // path validation
//!         if let Some(old) = migrated_from { rekey(old, from) }   // NAT rebind
//!         for d in deliveries { dispatch(cid, d) }
//!     }
//!     Incoming::Dropped(_) => {}
//! }
//! for (addr, dg) in ep.poll_transmit(now) { sock.send_to(&dg, addr) }
//! for (cid, addr, state) in ep.reap_closed() { peer_leave(cid) }
//! ```

use std::collections::{HashMap, VecDeque};
use std::net::{IpAddr, SocketAddr};

use ed25519_dalek::SigningKey;
use rand::rngs::StdRng;
use rand::{CryptoRng, RngCore, SeedableRng};
use x25519_dalek::{PublicKey, StaticSecret};

use super::channel::{Delivery, SendError, SendMode};
use super::conn::{self, close_code, parse_header, Conn, ConnConfig, ConnState, Side};
use super::crypto::HandshakeSecrets;
use super::handshake::{self, AuthPayload, ChallengeCore, HelloCore, HelloOutcome, HsError, ServerHandshake, ServerHsConfig};
use super::{classify, ConnId, DatagramKind};

/// Recently seen client ephemerals are remembered this long (> cookie TTL).
const RECENT_AUTH_MS: u64 = 35_000;
const MAX_RECENT: usize = 65_536;
/// Replay-memory entries one source (`ip_key`: IPv4 address, IPv6 /64) may
/// hold. A single host cannot fill the fail-closed memory and lock
/// everyone else out: that takes MAX_RECENT /
/// MAX_RECENT_PER_IP = 1024 cookie-proven sources. 64 covers a LAN party
/// behind one NAT reconnecting several times within 35 s.
const MAX_RECENT_PER_IP: usize = 64;
/// Per-source Auth budget, charged after the (cheap, HMAC) cookie check and
/// before any Diffie-Hellman or signature work. An honest client sends one
/// Auth plus resends at 250 ms doubling.
const AUTH_RATE_PER_S: f32 = 5.0;
const AUTH_BURST: f32 = 10.0;
/// Bound of the per-source Auth bucket table (only cookie-proven sources get
/// an entry). Full of active sources: new sources fail closed.
const MAX_AUTH_BUCKETS: usize = 65_536;
const REJECT_RESENDS: u8 = 3;

pub type ServerConfig = ServerHsConfig;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DropReason {
    Garbage,
    LegacyV4,
    Query,
    Handshake(HsError),
    DuplicateAuth,
    UnknownConn,
    AddrMismatch,
    Conn(conn::Drop),
    /// The replay memory of recent Auths is full of unexpired entries: the
    /// Auth is refused (fail closed) rather than evicting an entry, which
    /// would let a captured Auth be replayed (nonce reuse).
    RecentFull,
    /// The source (`ip_key`) used up its Auth budget or its share of the
    /// replay memory.
    AuthRateLimited,
}

pub enum Incoming {
    Dropped(DropReason),
    /// Send these bytes back to the sender (Challenge, PreReject, cached reject).
    Reply(Vec<u8>),
    /// A cryptographically valid Auth; the application decides.
    AuthRequest(Box<PendingAuth>),
    Data {
        cid: ConnId,
        deliveries: Vec<Delivery>,
        /// Path migration (NAT rebind): the connection's address just
        /// moved from this old address to the sender's (path validated: the
        /// sender acked a probe sent to it). The application re-keys its
        /// per-address state.
        migrated_from: Option<SocketAddr>,
        /// Send this datagram to the sender: a path probe for an
        /// authenticated packet that arrived from a not-yet-validated
        /// address. Its deliveries are genuine (AEAD) and are delivered; only
        /// our sends wait for the path to validate.
        probe: Option<Vec<u8>>,
    },
}

/// A verified Auth awaiting the application's admission decision.
pub struct PendingAuth {
    pub addr: SocketAddr,
    pub cid: ConnId,
    pub version: u16,
    pub caps: u64,
    pub th: [u8; 32],
    pub hello: HelloCore,
    pub payload: AuthPayload,
    secrets: HandshakeSecrets,
    auth_len: usize,
}

impl PendingAuth {
    pub fn player_key(&self) -> [u8; 32] {
        self.payload.player_key
    }
    pub fn nick(&self) -> &str {
        &self.payload.nick
    }
}

enum RecentOutcome {
    Accepted,
    Rejected { dg: Vec<u8>, resends: u8 },
}

struct Recent {
    expires: u64,
    outcome: RecentOutcome,
}

/// Replay memory of recent Auth ephemerals. Entries all live RECENT_AUTH_MS,
/// so insertion order is expiry order: pruning pops expired entries off the
/// front of a queue (O(1) amortised per Auth), never scans the whole map.
#[derive(Default)]
struct RecentMemory {
    map: HashMap<[u8; 32], Recent>,
    /// (expires, eph, source key), oldest first.
    queue: VecDeque<(u64, [u8; 32], IpAddr)>,
    per_ip: HashMap<IpAddr, usize>,
}

impl RecentMemory {
    fn len(&self) -> usize {
        self.map.len()
    }

    fn prune(&mut self, now: u64) {
        while let Some(&(exp, eph, ip)) = self.queue.front() {
            if exp > now {
                break;
            }
            self.queue.pop_front();
            if self.map.get(&eph).is_some_and(|r| r.expires == exp) {
                self.map.remove(&eph);
            }
            if let Some(n) = self.per_ip.get_mut(&ip) {
                *n -= 1;
                if *n == 0 {
                    self.per_ip.remove(&ip);
                }
            }
        }
    }

    fn per_ip(&self, ip: IpAddr) -> usize {
        self.per_ip.get(&ip).copied().unwrap_or(0)
    }

    fn insert(&mut self, now: u64, eph: [u8; 32], ip: IpAddr, outcome: RecentOutcome) {
        let expires = now + RECENT_AUTH_MS;
        if self.map.insert(eph, Recent { expires, outcome }).is_some() {
            // Same ephemeral again (cannot happen: duplicates return early);
            // the older queue entry no longer removes it (expiry mismatch).
        }
        self.queue.push_back((expires, eph, ip));
        *self.per_ip.entry(ip).or_insert(0) += 1;
    }
}

/// Token bucket per cookie-proven source for Auths.
#[derive(Default)]
struct AuthBuckets {
    b: HashMap<IpAddr, (f32, u64)>,
    pruned_at: u64,
}

impl AuthBuckets {
    fn allow(&mut self, now: u64, ip: IpAddr) -> bool {
        let refill = |t: f32, at: u64| (t + now.saturating_sub(at) as f32 / 1000.0 * AUTH_RATE_PER_S).min(AUTH_BURST);
        if !self.b.contains_key(&ip) && self.b.len() >= MAX_AUTH_BUCKETS {
            if now.saturating_sub(self.pruned_at) >= 1_000 {
                self.pruned_at = now;
                self.b.retain(|_, (t, at)| refill(*t, *at) < AUTH_BURST);
            }
            if self.b.len() >= MAX_AUTH_BUCKETS {
                return false; // fail closed
            }
        }
        let e = self.b.entry(ip).or_insert((AUTH_BURST, now));
        e.0 = refill(e.0, e.1);
        e.1 = now;
        if e.0 < 1.0 {
            return false;
        }
        e.0 -= 1.0;
        true
    }
}

struct Slot {
    addr: SocketAddr,
    conn: Conn,
    probe: Option<Probe>,
    /// Last time an authentic packet (fresh, or a verified duplicate)
    /// arrived from `addr`: the current path is still delivering.
    last_path_rx: u64,
    /// Times of this connection's recent migrations (rate limit).
    migrations: VecDeque<u64>,
}

/// An unvalidated new path of a connection.
struct Probe {
    addr: SocketAddr,
    /// Packet numbers of the probes sent there (peers without
    /// `caps::PATH_CHALLENGE`: an ack of one validates).
    pkts: Vec<u64>,
    /// PATH_CHALLENGE tokens sent there (peers with the capability: only a
    /// sealed PATH_RESPONSE echoing one, from that address, validates).
    tokens: Vec<[u8; 8]>,
    last_at: u64,
    validated: bool,
}

/// Probes to one candidate address: at most one per PROBE_EVERY_MS and
/// MAX_PROBE_PKTS outstanding (each ≤ the authenticated packet that caused
/// it, so a probe can never amplify).
const PROBE_EVERY_MS: u64 = 50;
const MAX_PROBE_PKTS: usize = 8;
/// A validated new path is taken only after the current path has been
/// silent this long (at least; 2·srtt when larger): while the old path still
/// delivers, the "new path" is a racing copy, not a rebind.
const MIGRATE_SILENCE_MIN_MS: u64 = 100;
/// Migrations of one connection: at least this far apart ...
const MIGRATE_MIN_GAP_MS: u64 = 1_000;
/// ... and at most this many per minute.
const MIGRATE_MAX_PER_MIN: usize = 5;
/// Stateless resets, all sources together (each ≤ the request).
const RESET_RATE_PER_S: f64 = 50.0;
const RESET_BURST: f64 = 50.0;
/// ... and per source (`ip_key`), so junk from one host cannot starve the
/// honest resets after a restart.
const RESET_PER_SRC_PER_S: f64 = 5.0;
const RESET_PER_SRC_BURST: f64 = 5.0;
const RESET_SOURCES: usize = 4096;

#[derive(Debug, Default, Clone)]
pub struct EndpointStats {
    pub preauth_in: u64,
    pub preauth_bytes_in: u64,
    pub preauth_out: u64,
    pub preauth_bytes_out: u64,
    pub accepted: u64,
    pub rejected: u64,
    pub dropped: u64,
    /// Connections whose address moved (validated NAT rebinds).
    pub migrations: u64,
    pub probes_sent: u64,
    pub resets_sent: u64,
}

pub struct ServerEndpoint {
    hs: ServerHandshake,
    rng: StdRng,
    conns: HashMap<ConnId, Slot>,
    recent: RecentMemory,
    auth_buckets: AuthBuckets,
    conn_cfg: ConnConfig,
    stats: EndpointStats,
    /// Cap of `recent` (MAX_RECENT; smaller in tests).
    recent_cap: usize,
    /// Per-source cap of `recent` (MAX_RECENT_PER_IP; smaller in tests).
    recent_ip_cap: usize,
    reset_key: [u8; 32],
    reset_tokens: f64,
    reset_refill_at: u64,
    /// Per-source stateless-reset buckets (ip_key -> (tokens, at)).
    reset_src: HashMap<IpAddr, (f64, u64)>,
}

impl ServerEndpoint {
    pub fn new(cfg: ServerConfig, conn_cfg: ConnConfig, now: u64, seed: Option<u64>) -> Self {
        let mut rng = match seed {
            Some(s) => StdRng::seed_from_u64(s),
            None => StdRng::from_entropy(),
        };
        let reset_key = super::crypto::reset_key(&cfg.static_secret);
        let hs = ServerHandshake::new(cfg, now, &mut rng);
        ServerEndpoint {
            reset_key,
            reset_tokens: RESET_BURST,
            reset_refill_at: now,
            reset_src: HashMap::new(),
            hs,
            rng,
            conns: HashMap::new(),
            recent: RecentMemory::default(),
            auth_buckets: AuthBuckets::default(),
            conn_cfg,
            stats: EndpointStats::default(),
            recent_cap: MAX_RECENT,
            recent_ip_cap: MAX_RECENT_PER_IP,
        }
    }

    pub fn static_public(&self) -> [u8; 32] {
        self.hs.static_public()
    }
    pub fn conn_count(&self) -> usize {
        self.conns.len()
    }
    /// Per-client state that exists before a connection is accepted.
    pub fn preauth_state(&self) -> usize {
        self.recent.len()
    }
    pub fn stats(&self) -> &EndpointStats {
        &self.stats
    }
    pub fn conn(&self, cid: ConnId) -> Option<&Conn> {
        self.conns.get(&cid).map(|s| &s.conn)
    }
    pub fn conn_mut(&mut self, cid: ConnId) -> Option<&mut Conn> {
        self.conns.get_mut(&cid).map(|s| &mut s.conn)
    }
    pub fn addr_of(&self, cid: ConnId) -> Option<SocketAddr> {
        self.conns.get(&cid).map(|s| s.addr)
    }
    pub fn conn_ids(&self) -> Vec<ConnId> {
        self.conns.keys().copied().collect()
    }

    fn drop_(&mut self, r: DropReason) -> Incoming {
        self.stats.dropped += 1;
        Incoming::Dropped(r)
    }

    fn reply(&mut self, dg: Vec<u8>) -> Incoming {
        self.stats.preauth_out += 1;
        self.stats.preauth_bytes_out += dg.len() as u64;
        Incoming::Reply(dg)
    }

    pub fn handle(&mut self, now: u64, from: SocketAddr, dg: &[u8]) -> Incoming {
        match classify(dg) {
            DatagramKind::Data => self.handle_data(now, from, dg),
            DatagramKind::Hello => {
                self.stats.preauth_in += 1;
                self.stats.preauth_bytes_in += dg.len() as u64;
                match self.hs.on_hello(now, from, dg, &mut self.rng) {
                    Ok(HelloOutcome::Challenge(r)) | Ok(HelloOutcome::Reject(r)) => self.reply(r),
                    Err(e) => self.drop_(DropReason::Handshake(e)),
                }
            }
            DatagramKind::Auth => {
                self.stats.preauth_in += 1;
                self.stats.preauth_bytes_in += dg.len() as u64;
                self.handle_auth(now, from, dg)
            }
            DatagramKind::Query => self.drop_(DropReason::Query),
            DatagramKind::LegacyV4 => self.drop_(DropReason::LegacyV4),
            _ => self.drop_(DropReason::Garbage),
        }
    }

    fn handle_auth(&mut self, now: u64, from: SocketAddr, dg: &[u8]) -> Incoming {
        let src = super::ip_key(from.ip());
        self.recent.prune(now);
        // Per-source budget, charged once the cookie proved the address
        // (a spoofer cannot drain someone else's) and before any DH work.
        // A known ephemeral (resent / replayed Auth) is answered from the
        // replay memory BEFORE the budget is charged or any DH is done,
        // so a LAN group reconnecting together does not burn its
        // shared budget on resends.
        let buckets = &mut self.auth_buckets;
        let recent = &self.recent;
        let mut replayed: Option<[u8; 32]> = None;
        let gate = |_: &SocketAddr, eph: &[u8; 32]| {
            if recent.map.contains_key(eph) {
                replayed = Some(*eph);
                Err(HsError::Replayed)
            } else if buckets.allow(now, src) {
                Ok(())
            } else {
                Err(HsError::RateLimited)
            }
        };
        let res = self.hs.on_auth_gated(now, from, dg, &mut self.rng, gate);
        let eph = match res {
            Ok(ref ok) => ok.hello.eph_pub,
            Err(HsError::Replayed) => replayed.expect("set by the gate"),
            Err(HsError::RateLimited) => return self.drop_(DropReason::AuthRateLimited),
            Err(e) => return self.drop_(DropReason::Handshake(e)),
        };
        // The address is proven (cookie); the sender holds the keys, or
        // repeats an Auth we already answered.
        if let Some(r) = self.recent.map.get_mut(&eph) {
            if let RecentOutcome::Rejected { dg: rej, resends } = &mut r.outcome {
                if *resends < REJECT_RESENDS {
                    *resends += 1;
                    let rej = rej.clone();
                    return self.reply(rej);
                }
            }
            return self.drop_(DropReason::DuplicateAuth);
        }
        let Ok(ok) = res else {
            // Replayed, but the entry expired between the gate and here.
            return self.drop_(DropReason::DuplicateAuth);
        };
        let cid = ok.secrets.conn_id;
        if self.conns.contains_key(&cid) {
            return self.drop_(DropReason::DuplicateAuth);
        }
        // Fail closed: a full replay memory (expired entries were just
        // pruned) refuses new Auths instead of evicting a live entry, so a
        // flood of handshakes can never re-open a captured Auth for replay.
        if self.recent.per_ip(src) >= self.recent_ip_cap {
            return self.drop_(DropReason::AuthRateLimited);
        }
        if self.recent.len() >= self.recent_cap {
            return self.drop_(DropReason::RecentFull);
        }
        Incoming::AuthRequest(Box::new(PendingAuth {
            addr: from,
            cid,
            version: ok.version,
            caps: ok.caps,
            th: ok.th,
            hello: ok.hello,
            payload: ok.payload,
            secrets: ok.secrets,
            auth_len: dg.len(),
        }))
    }

    fn remember(&mut self, now: u64, addr: SocketAddr, eph: [u8; 32], outcome: RecentOutcome) {
        // Never evict an unexpired entry (that would let a captured Auth be
        // replayed). `handle_auth` refuses new Auths while the memory is
        // full, so it can only exceed the cap by the Auths already handed
        // to the application.
        self.recent.prune(now);
        self.recent.insert(now, eph, super::ip_key(addr.ip()), outcome);
    }

    /// Tests: a small replay memory, an entry for a key, a connection gone.
    #[cfg(test)]
    pub(crate) fn test_set_recent_cap(&mut self, cap: usize) {
        self.recent_cap = cap;
    }
    #[cfg(test)]
    pub(crate) fn test_remember(&mut self, now: u64, eph: [u8; 32]) {
        let a: SocketAddr = "192.0.2.1:1".parse().expect("addr");
        self.remember(now, a, eph, RecentOutcome::Accepted);
    }
    /// (map entries, queue entries): the queue never holds more than the map
    /// plus not-yet-popped expired entries.
    #[cfg(test)]
    pub(crate) fn test_recent_sizes(&self) -> (usize, usize) {
        (self.recent.map.len(), self.recent.queue.len())
    }
    #[cfg(test)]
    pub(crate) fn test_rotated_at(&self) -> u64 {
        self.hs.rotated_at()
    }
    #[cfg(test)]
    pub(crate) fn test_drop_conn(&mut self, cid: ConnId) {
        self.conns.remove(&cid);
    }

    /// Admit the client: allocates the connection.
    pub fn accept(&mut self, now: u64, p: Box<PendingAuth>) -> ConnId {
        self.remember(now, p.addr, p.hello.eph_pub, RecentOutcome::Accepted);
        let mut conn = Conn::from_handshake(Side::Server, &p.secrets, now, self.conn_cfg.clone());
        conn.set_caps(p.caps);
        conn.set_reset_token(super::crypto::reset_token(&self.reset_key, p.cid));
        // Never overwrite a live connection (same keys, seq 0 would
        // reuse nonces). `handle_auth` already refuses a known cid; this is
        // the backstop for an admission path that accepts twice.
        if self.conns.contains_key(&p.cid) {
            return p.cid;
        }
        self.conns.insert(
            p.cid,
            Slot {
                addr: p.addr,
                conn,
                probe: None,
                last_path_rx: now,
                migrations: VecDeque::new(),
            },
        );
        self.stats.accepted += 1;
        p.cid
    }

    /// Refuse the client: returns a sealed reject (<= the Auth's size) to send.
    pub fn reject(&mut self, now: u64, p: Box<PendingAuth>, code: u8, text: &str) -> Vec<u8> {
        let dg = handshake::auth_reject_datagram(&p.secrets, code, text, p.auth_len);
        self.remember(
            now,
            p.addr,
            p.hello.eph_pub,
            RecentOutcome::Rejected {
                dg: dg.clone(),
                resends: 0,
            },
        );
        self.stats.rejected += 1;
        self.stats.preauth_out += 1;
        self.stats.preauth_bytes_out += dg.len() as u64;
        dg
    }

    fn handle_data(&mut self, now: u64, from: SocketAddr, dg: &[u8]) -> Incoming {
        let Some(h) = parse_header(dg) else {
            return self.drop_(DropReason::Conn(conn::Drop::Truncated));
        };
        let Some(slot) = self.conns.get_mut(&h.conn_id) else {
            // A connection we do not know (restarted server, reaped
            // connection): a stateless reset the client can verify, so it
            // re-handshakes at once instead of after its timeout.
            if let Some(r) = self.stateless_reset(now, from, h.conn_id, dg.len()) {
                self.stats.dropped += 1;
                self.stats.resets_sent += 1;
                return Incoming::Reply(r);
            }
            return self.drop_(DropReason::UnknownConn);
        };
        let deliveries = match slot.conn.recv(now, dg) {
            Ok(d) => d,
            Err(conn::Drop::Duplicate) if slot.addr == from => {
                // A genuine copy arriving late on the current path (an
                // off-path attacker raced a copy from elsewhere): the current
                // path still delivers, which blocks a migration.
                if slot.conn.peek_auth(dg) {
                    slot.last_path_rx = now;
                }
                return self.drop_(DropReason::Conn(conn::Drop::Duplicate));
            }
            Err(e) => return self.drop_(DropReason::Conn(e)),
        };
        // Authenticated (AEAD) and fresh (replay window) from here on.
        let response = slot.conn.take_path_response();
        if slot.addr == from {
            // The current path is alive: forget a pending candidate (a
            // reordered straggler from an old address, NAT flapping).
            slot.last_path_rx = now;
            slot.probe = None;
            return Incoming::Data { cid: h.conn_id, deliveries, migrated_from: None, probe: None };
        }
        // Path migration: an authenticated packet from a new
        // address. Its messages are delivered; our sends stay on the current
        // path until (1) the new path is validated — a PATH_CHALLENGE echoed
        // from there (caps::PATH_CHALLENGE), or for older peers an ack of a
        // probe sent there — and (2) the current path has gone silent for
        // max(2·srtt, 100 ms) (a racing copy from an off-path forwarder never
        // satisfies this while the real client keeps talking), and (3) the
        // connection has not migrated within the last second / 5 times a
        // minute, and (4) no other live connection uses the new address.
        let challenge = slot.conn.caps() & super::caps::PATH_CHALLENGE != 0;
        if let Some(p) = slot.probe.as_mut().filter(|p| p.addr == from) {
            let ok = if challenge {
                response.is_some_and(|r| p.tokens.iter().any(|t| super::crypto::ct_eq(t, &r)))
            } else {
                p.pkts.iter().any(|&pkt| slot.conn.header_acks(&h, pkt))
            };
            p.validated |= ok;
        }
        let validated = slot.probe.as_ref().is_some_and(|p| p.addr == from && p.validated);
        if validated {
            let need = ((2.0 * slot.conn.srtt_ms()) as u64).max(MIGRATE_SILENCE_MIN_MS);
            let old_silent = now.saturating_sub(slot.last_path_rx) >= need;
            while slot.migrations.front().is_some_and(|&t| now.saturating_sub(t) >= 60_000) {
                slot.migrations.pop_front();
            }
            let rate_ok = slot.migrations.len() < MIGRATE_MAX_PER_MIN
                && slot.migrations.back().is_none_or(|&t| now.saturating_sub(t) >= MIGRATE_MIN_GAP_MS);
            if old_silent && rate_ok {
                let cid = h.conn_id;
                let taken = self.conns.iter().any(|(c, s)| *c != cid && s.addr == from && !s.conn.is_closed());
                let slot = self.conns.get_mut(&cid).expect("present");
                if taken {
                    // Never displace another live connection.
                    slot.probe = None;
                    return Incoming::Data { cid, deliveries, migrated_from: None, probe: None };
                }
                let old = std::mem::replace(&mut slot.addr, from);
                slot.probe = None;
                slot.last_path_rx = now;
                slot.migrations.push_back(now);
                self.stats.migrations += 1;
                return Incoming::Data { cid, deliveries, migrated_from: Some(old), probe: None };
            }
            return Incoming::Data { cid: h.conn_id, deliveries, migrated_from: None, probe: None };
        }
        let due = match &slot.probe {
            Some(p) if p.addr == from => now.saturating_sub(p.last_at) >= PROBE_EVERY_MS,
            _ => true,
        };
        let mut probe = None;
        // The probe's size is known before sealing: one that would be larger
        // than the packet that caused it is never built.
        if due && Conn::probe_len(challenge) <= dg.len() {
            let token = challenge.then(|| {
                let mut t = [0u8; 8];
                self.rng.fill_bytes(&mut t);
                t
            });
            let slot = self.conns.get_mut(&h.conn_id).expect("present");
            let (pkt, dg_probe) = slot.conn.probe(now, token);
            let mut p = match slot.probe.take() {
                Some(p) if p.addr == from => p,
                _ => Probe { addr: from, pkts: Vec::new(), tokens: Vec::new(), last_at: now, validated: false },
            };
            if p.pkts.len() >= MAX_PROBE_PKTS {
                p.pkts.remove(0);
            }
            p.pkts.push(pkt);
            if let Some(t) = token {
                if p.tokens.len() >= MAX_PROBE_PKTS {
                    p.tokens.remove(0);
                }
                p.tokens.push(t);
            }
            p.last_at = now;
            slot.probe = Some(p);
            self.stats.probes_sent += 1;
            probe = Some(dg_probe);
        }
        Incoming::Data { cid: h.conn_id, deliveries, migrated_from: None, probe }
    }

    /// The reset for `cid` if the budgets allow and it is no larger than the
    /// request.
    fn stateless_reset(&mut self, now: u64, from: SocketAddr, cid: ConnId, req_len: usize) -> Option<Vec<u8>> {
        if req_len < super::RESET_LEN + 1 {
            return None;
        }
        let dt = now.saturating_sub(self.reset_refill_at) as f64 / 1000.0;
        self.reset_refill_at = self.reset_refill_at.max(now);
        self.reset_tokens = (self.reset_tokens + dt * RESET_RATE_PER_S).min(RESET_BURST);
        if self.reset_tokens < 1.0 {
            return None;
        }
        // Per-source share, bounded table; full of busy sources: refuse.
        let src = super::ip_key(from.ip());
        let refill = |t: f64, at: u64| (t + now.saturating_sub(at) as f64 / 1000.0 * RESET_PER_SRC_PER_S).min(RESET_PER_SRC_BURST);
        if !self.reset_src.contains_key(&src) && self.reset_src.len() >= RESET_SOURCES {
            self.reset_src.retain(|_, (t, at)| refill(*t, *at) < RESET_PER_SRC_BURST);
            if self.reset_src.len() >= RESET_SOURCES {
                return None;
            }
        }
        let e = self.reset_src.entry(src).or_insert((RESET_PER_SRC_BURST, now));
        e.0 = refill(e.0, e.1);
        e.1 = now;
        if e.0 < 1.0 {
            return None;
        }
        e.0 -= 1.0;
        self.reset_tokens -= 1.0;
        Some(reset_datagram(cid, &super::crypto::reset_token(&self.reset_key, cid)))
    }

    pub fn send(&mut self, cid: ConnId, mode: SendMode, data: Vec<u8>) -> Result<(), SendError> {
        self.conns.get_mut(&cid).ok_or(SendError::NotConnected)?.conn.send(mode, data)
    }

    pub fn close(&mut self, now: u64, cid: ConnId, code: u8, reason: &str) {
        if let Some(s) = self.conns.get_mut(&cid) {
            s.conn.close(now, code, reason);
        }
    }

    /// All datagrams due now, across every connection. This is the timer
    /// path, so it also rotates the ephemeral handshake key: rotating only on
    /// handshake traffic would leave an idle server on one key indefinitely, beyond
    /// the documented forward-secrecy bound.
    pub fn poll_transmit(&mut self, now: u64) -> Vec<(SocketAddr, Vec<u8>)> {
        self.hs.rotate_if_due(now, &mut self.rng);
        let mut out = Vec::new();
        for s in self.conns.values_mut() {
            while let Some(dg) = s.conn.poll_transmit(now) {
                out.push((s.addr, dg));
            }
        }
        out
    }

    /// Datagrams due now for one connection (relay cut-through).
    pub fn poll_transmit_one(&mut self, now: u64, cid: ConnId) -> Vec<(SocketAddr, Vec<u8>)> {
        let mut out = Vec::new();
        if let Some(s) = self.conns.get_mut(&cid) {
            while let Some(dg) = s.conn.poll_transmit(now) {
                out.push((s.addr, dg));
            }
        }
        out
    }

    /// Remove closed connections; returns what was removed.
    pub fn reap_closed(&mut self) -> Vec<(ConnId, SocketAddr, ConnState)> {
        let dead: Vec<ConnId> = self.conns.iter().filter(|(_, s)| s.conn.is_closed()).map(|(c, _)| *c).collect();
        dead.into_iter()
            .filter_map(|c| self.conns.remove(&c).map(|s| (c, s.addr, s.conn.state().clone())))
            .collect()
    }
}

/// `PT_RESET | conn_id | token` (RESET_LEN bytes).
pub fn reset_datagram(cid: ConnId, token: &[u8; 16]) -> Vec<u8> {
    let mut v = Vec::with_capacity(super::RESET_LEN);
    v.push(super::PT_RESET);
    v.extend_from_slice(&cid.to_le_bytes());
    v.extend_from_slice(token);
    v
}

// ---------------------------------------------------------------------------
// Client
// ---------------------------------------------------------------------------

/// `argon2(password, pwd_salt)` provider.
pub type PasswordKeyFn = Box<dyn Fn(&[u8; 16]) -> [u8; 32] + Send>;

pub struct ClientConfig {
    /// Ed25519 seed of the per-install player identity.
    pub player_seed: [u8; 32],
    pub nick: String,
    pub content_hash: [u8; 32],
    pub build: String,
    pub version_min: u16,
    pub version_max: u16,
    pub caps: u64,
    /// Expected server static key (master listing / pinned). `None` = TOFU.
    pub pinned_server_key: Option<[u8; 32]>,
    pub want_role: u8,
    pub resume_token: Option<[u8; 16]>,
    pub party_token: Option<[u8; 16]>,
    /// `argon2(password, pwd_salt)` provider, called when the server asks.
    pub password_key: Option<PasswordKeyFn>,
    /// Handshake retransmission: first interval, then doubling to 2 s.
    pub resend_ms: u64,
    /// Give up the handshake after this long.
    pub handshake_timeout_ms: u64,
}

impl ClientConfig {
    pub fn new(player_seed: [u8; 32], nick: &str) -> Self {
        ClientConfig {
            player_seed,
            nick: nick.into(),
            content_hash: [0; 32],
            build: String::new(),
            version_min: super::VERSION_MIN,
            version_max: super::VERSION_MAX,
            caps: super::caps::SUPPORTED,
            pinned_server_key: None,
            want_role: handshake::role::FIGHTER,
            resume_token: None,
            party_token: None,
            password_key: None,
            resend_ms: 250,
            handshake_timeout_ms: 10_000,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ClientEvent {
    /// First authenticated packet from the server arrived.
    Connected {
        server_key: [u8; 32],
        version: u16,
        caps: u64,
        conn_id: ConnId,
    },
    /// `authenticated` = false for a pre-cookie (cleartext) reject.
    Rejected {
        code: u8,
        text: String,
        authenticated: bool,
    },
    Message(Delivery),
    Closed {
        code: u8,
        by_peer: bool,
    },
    Failed(&'static str),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Phase {
    AwaitChallenge,
    AwaitWelcome,
    Connected,
    Done,
}

pub struct Client {
    cfg: ClientConfig,
    player: SigningKey,
    eph: StaticSecret,
    hello: HelloCore,
    hello_bytes: Vec<u8>,
    hello_dg: Vec<u8>,
    phase: Phase,
    auth_dg: Vec<u8>,
    secrets: Option<HandshakeSecrets>,
    conn: Option<Conn>,
    conn_cfg: ConnConfig,
    started: u64,
    resend_at: u64,
    resend_iv: u64,
    server_key: [u8; 32],
    version: u16,
    caps: u64,
    events: VecDeque<ClientEvent>,
}

impl Client {
    pub fn new<R: RngCore + CryptoRng>(cfg: ClientConfig, conn_cfg: ConnConfig, now: u64, rng: &mut R) -> Self {
        let eph = StaticSecret::random_from_rng(&mut *rng);
        let hello = HelloCore {
            version_min: cfg.version_min,
            version_max: cfg.version_max,
            caps: cfg.caps,
            eph_pub: PublicKey::from(&eph).to_bytes(),
            content_hash: cfg.content_hash,
            build: cfg.build.clone(),
        };
        let hello_bytes = hello.encode();
        let hello_dg = handshake::hello_datagram(&hello);
        Client {
            player: SigningKey::from_bytes(&cfg.player_seed),
            eph,
            hello,
            hello_bytes,
            hello_dg,
            phase: Phase::AwaitChallenge,
            auth_dg: Vec::new(),
            secrets: None,
            conn: None,
            conn_cfg,
            started: now,
            resend_at: now,
            resend_iv: cfg.resend_ms,
            server_key: [0; 32],
            version: 0,
            caps: 0,
            events: VecDeque::new(),
            cfg,
        }
    }

    pub fn player_key(&self) -> [u8; 32] {
        self.player.verifying_key().to_bytes()
    }
    pub fn is_connected(&self) -> bool {
        self.phase == Phase::Connected
    }
    pub fn conn(&self) -> Option<&Conn> {
        self.conn.as_ref()
    }
    pub fn conn_mut(&mut self) -> Option<&mut Conn> {
        self.conn.as_mut()
    }
    /// Test hook (e2e "no key material in cleartext"): the derived handshake
    /// secrets, once the Challenge was processed.
    #[doc(hidden)]
    pub fn debug_secrets(&self) -> Option<&HandshakeSecrets> {
        self.secrets.as_ref()
    }
    pub fn poll_event(&mut self) -> Option<ClientEvent> {
        self.events.pop_front()
    }

    /// Queue a message. Allowed once the Auth is out; it is transmitted
    /// after the server's first packet confirms the connection.
    pub fn send(&mut self, mode: SendMode, data: Vec<u8>) -> Result<(), SendError> {
        match self.conn.as_mut() {
            Some(c) => c.send(mode, data),
            None => Err(SendError::NotConnected),
        }
    }

    pub fn close(&mut self, now: u64, code: u8, reason: &str) {
        if let Some(c) = self.conn.as_mut() {
            c.close(now, code, reason);
        }
    }

    fn fail(&mut self, ev: ClientEvent) {
        self.phase = Phase::Done;
        self.events.push_back(ev);
    }

    pub fn handle(&mut self, now: u64, dg: &[u8]) {
        match (self.phase, classify(dg)) {
            (Phase::AwaitChallenge, DatagramKind::Challenge) => {
                let pwd = self.cfg.password_key.as_ref();
                let (nick, role, resume, party) = (
                    self.cfg.nick.clone(),
                    self.cfg.want_role,
                    self.cfg.resume_token,
                    self.cfg.party_token,
                );
                let res = handshake::client_on_challenge(
                    &self.eph,
                    &self.hello,
                    &self.hello_bytes,
                    dg,
                    self.cfg.pinned_server_key,
                    &self.player,
                    |th: &[u8; 32], ch: &ChallengeCore| AuthPayload {
                        want_role: role,
                        resume_token: resume,
                        party_token: party,
                        pwd_proof: if ch.flags & handshake::FLAG_NEEDS_PASSWORD != 0 {
                            pwd.map(|f| handshake::password_proof(&f(&ch.pwd_salt), th))
                        } else {
                            None
                        },
                        nick,
                        ..Default::default()
                    },
                );
                match res {
                    Ok(a) => {
                        let mut conn = Conn::from_handshake(Side::Client, &a.secrets, now, self.conn_cfg.clone());
                        self.secrets = Some(a.secrets);
                        self.auth_dg = a.datagram;
                        self.server_key = a.server_static;
                        self.version = a.version;
                        self.caps = self.cfg.caps & self.caps_from_challenge(dg);
                        conn.set_caps(self.caps);
                        self.conn = Some(conn);
                        self.phase = Phase::AwaitWelcome;
                        self.resend_at = now;
                        self.resend_iv = self.cfg.resend_ms;
                    }
                    Err(HsError::ServerKeyMismatch) => self.fail(ClientEvent::Failed("server key does not match the pinned key")),
                    Err(_) => {} // forged or stale challenge: ignore, keep waiting
                }
            }
            (Phase::AwaitChallenge, DatagramKind::PreReject) => {
                if let Ok(r) = handshake::parse_pre_reject(dg) {
                    if r.echo == self.hello.echo() {
                        self.fail(ClientEvent::Rejected {
                            code: r.code,
                            text: r.text,
                            authenticated: false,
                        });
                    }
                }
            }
            (Phase::AwaitWelcome, DatagramKind::AuthReject) => {
                if let Some((code, text)) = self.secrets.as_ref().and_then(|s| handshake::open_auth_reject(s, dg)) {
                    self.conn = None;
                    self.fail(ClientEvent::Rejected {
                        code,
                        text,
                        authenticated: true,
                    });
                }
            }
            (Phase::AwaitWelcome | Phase::Connected, DatagramKind::Data) => {
                let Some(c) = self.conn.as_mut() else { return };
                let Ok(ds) = c.recv(now, dg) else { return };
                if self.phase == Phase::AwaitWelcome {
                    self.phase = Phase::Connected;
                    self.events.push_back(ClientEvent::Connected {
                        server_key: self.server_key,
                        version: self.version,
                        caps: self.caps,
                        conn_id: c.conn_id(),
                    });
                }
                for d in ds {
                    self.events.push_back(ClientEvent::Message(d));
                }
                if let ConnState::Closed { code, by_peer } = c.state().clone() {
                    self.phase = Phase::Done;
                    self.events.push_back(ClientEvent::Closed { code, by_peer });
                }
            }
            (Phase::Connected, DatagramKind::Reset) => {
                // Verified stateless reset: the server no longer knows this
                // connection (restart, reaped). Only the token the server
                // handed out over the encrypted connection is accepted, so an
                // on-path observer cannot forge one.
                let Some(c) = self.conn.as_ref() else { return };
                let Some(t) = c.reset_token() else { return };
                if dg.len() == super::RESET_LEN
                    && dg[1..9] == c.conn_id().to_le_bytes()
                    && super::crypto::ct_eq(&dg[9..25], &t)
                {
                    self.conn = None;
                    self.phase = Phase::Done;
                    self.events.push_back(ClientEvent::Closed { code: close_code::RESET, by_peer: true });
                }
            }
            _ => {}
        }
    }

    fn caps_from_challenge(&self, dg: &[u8]) -> u64 {
        handshake::parse_challenge(dg).map(|(_, c)| c.server_caps).unwrap_or(0)
    }

    fn handshake_resend(&mut self, now: u64) -> Option<Vec<u8>> {
        if now.saturating_sub(self.started) >= self.cfg.handshake_timeout_ms {
            self.conn = None;
            self.fail(ClientEvent::Failed("no answer from the server"));
            return None;
        }
        if now < self.resend_at {
            return None;
        }
        self.resend_at = now + self.resend_iv;
        self.resend_iv = (self.resend_iv * 2).min(2_000);
        Some(if self.phase == Phase::AwaitChallenge {
            self.hello_dg.clone()
        } else {
            self.auth_dg.clone()
        })
    }

    pub fn poll_transmit(&mut self, now: u64) -> Option<Vec<u8>> {
        match self.phase {
            Phase::AwaitChallenge | Phase::AwaitWelcome => self.handshake_resend(now),
            Phase::Connected => {
                let c = self.conn.as_mut()?;
                let dg = c.poll_transmit(now);
                if let ConnState::Closed { code, by_peer } = c.state().clone() {
                    if dg.is_none() {
                        self.phase = Phase::Done;
                        self.events.push_back(ClientEvent::Closed { code, by_peer });
                    }
                }
                dg
            }
            Phase::Done => None,
        }
    }
}
