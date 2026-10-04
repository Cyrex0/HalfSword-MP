//! Protocol v5 transport glue for the server (docs/development/protocol.md).
//! The crate `hsmp-net` is sans-IO; this module owns the one `ServerEndpoint`,
//! maps addresses to connections and turns record messages into sealed
//! datagrams. Callers send the returned datagrams themselves, after the lock
//! is released.
//!
//! Locking: `ep` is a `std::sync::Mutex` held only for one decrypt, one
//! send+flush or one timer pass. It is never held across an `.await`.

use crate::proto;
use hsmp_net::net::{
    self as hn, close_code, ConnConfig, ConnId, ConnState, Incoming, PendingAuth, SendError, SendMode,
    ServerConfig, ServerEndpoint,
};
use std::collections::HashMap;
use std::net::{IpAddr, SocketAddr};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, RwLock};
use std::time::Instant;
use tracing::{debug, warn};

pub type Out = Vec<(SocketAddr, Vec<u8>)>;

/// Server-side transport idle timeout (the server uses 20 s so a
/// duel pause or an 8 s blackout keeps the connection).
pub const SERVER_IDLE_MS: u64 = 20_000;
/// Bounded v4 "outdated" reply: at most once per IP per this long.
const V4_REJECT_EVERY_MS: u64 = 10_000;
/// Hard cap of the per-IP v4 reject memory. When full of fresh entries the
/// server fails closed (no reply) instead of scanning per packet.
const V4_REJECT_IPS: usize = 4096;
/// Global budget for v4 rejects (all sources together): spoofed sources can
/// neither make the server a reflector nor make the map churn faster.
const V4_REJECT_RATE_PER_S: f64 = 20.0;
const V4_REJECT_BURST: f64 = 40.0;
/// The full-map prune runs at most this often.
const V4_PRUNE_EVERY_MS: u64 = 1_000;

/// The send mode a `len`-byte message actually uses: a channel-0 `Latest`
/// message too big for one datagram goes Reliable (fragmented) instead.
fn fit_mode(mode: SendMode, len: usize) -> SendMode {
    match mode {
        SendMode::Latest { .. } if len > hsmp_net::net::channel::MAX_UNRELIABLE => SendMode::Reliable,
        m => m,
    }
}

/// v4 reject limiter: one global token bucket, then a bounded per-IP memory.
/// Every packet costs O(1); the O(map) prune runs at most once a second.
struct V4Rejects {
    per_ip: HashMap<IpAddr, u64>,
    tokens: f64,
    refill_at: u64,
    pruned_at: u64,
}

impl V4Rejects {
    fn new() -> Self {
        V4Rejects { per_ip: HashMap::new(), tokens: V4_REJECT_BURST, refill_at: 0, pruned_at: 0 }
    }

    /// True if `ip` may get a reject now (and books it).
    fn allow(&mut self, now: u64, ip: IpAddr) -> bool {
        let ip = crate::ipkey::ip_key(ip); // IPv6 per /64
        let dt = now.saturating_sub(self.refill_at) as f64 / 1000.0;
        self.refill_at = self.refill_at.max(now);
        self.tokens = (self.tokens + dt * V4_REJECT_RATE_PER_S).min(V4_REJECT_BURST);
        if self.tokens < 1.0 {
            return false;
        }
        if let Some(t) = self.per_ip.get(&ip) {
            if now.saturating_sub(*t) < V4_REJECT_EVERY_MS {
                return false;
            }
        }
        if self.per_ip.len() >= V4_REJECT_IPS && !self.per_ip.contains_key(&ip) {
            if now.saturating_sub(self.pruned_at) >= V4_PRUNE_EVERY_MS {
                self.pruned_at = now;
                self.per_ip.retain(|_, t| now.saturating_sub(*t) < V4_REJECT_EVERY_MS);
            }
            if self.per_ip.len() >= V4_REJECT_IPS {
                return false; // fail closed
            }
        }
        self.tokens -= 1.0;
        self.per_ip.insert(ip, now);
        true
    }
}
/// The v4 reject text. Starts with OUTDATED so a reply truncated to the
/// request size still says what is wrong.
pub const V4_REJECT_TEXT: &str = "OUTDATED: this server runs a newer HSMP protocol. Update your HSMP mod.";

/// What one received datagram asks the application to do.
pub enum Action {
    /// Nothing for the application (handshake reply, drop, ack-only ...).
    None,
    /// A cryptographically valid Auth: run admission (`admit`).
    Auth(Box<PendingAuth>),
    /// Authenticated messages from a connected peer (possibly none: a PING
    /// or ack still proves liveness). `peer` is the address the peer is
    /// known by (its `Inner::peers` key), which differs from the datagram's
    /// source while a NAT rebind is being validated. `migrated_to`: the
    /// connection's path just moved to this address; the caller
    /// re-keys its per-address state and then calls `commit_migration`.
    Data { deliveries: Vec<hn::Delivery>, peer: SocketAddr, migrated_to: Option<SocketAddr> },
}

/// Address ↔ connection maps. `by_addr` is keyed by the address the
/// application knows a peer by; `by_cid` is its inverse. `aliases` keep a
/// migrated peer's old address routable for a short while, so messages
/// already addressed to it (collected before the re-key) still go out.
#[derive(Default)]
struct Maps {
    by_addr: HashMap<SocketAddr, ConnId>,
    by_cid: HashMap<ConnId, SocketAddr>,
    aliases: HashMap<SocketAddr, (ConnId, u64)>,
}

/// Period of the per-connection transport summary log line.
const CONN_REPORT_MS: u64 = 10_000;

/// How long an old address stays routable after a migration.
const ALIAS_MS: u64 = 5_000;

#[derive(Default, Debug, Clone)]
pub struct NetCounters {
    pub dropped_legacy: u64,
    pub dropped_unknown_conn: u64,
    pub dropped_addr_mismatch: u64,
    pub dropped_replay: u64,
    pub dropped_auth_fail: u64,
    pub dropped_handshake: u64,
    pub dropped_garbage: u64,
    pub backpressure: u64,
    pub v4_rejects: u64,
    pub migrations: u64,
}

pub struct Net {
    ep: Mutex<ServerEndpoint>,
    maps: RwLock<Maps>,
    t0: Instant,
    epoch: u64,
    static_pub: [u8; 32],
    v4_rejects: Mutex<V4Rejects>,
    counters: Mutex<NetCounters>,
    pkts_in: AtomicU64,
    conn_report_at: AtomicU64,
}

impl Net {
    pub fn new(static_secret: [u8; 32], content_hash: Option<[u8; 32]>) -> Self {
        let mut cfg = ServerConfig::new(static_secret);
        cfg.content_hash = content_hash;
        // Interaction channel (docs/development/subsystems/interact.md), offered on top of the base set.
        cfg.caps |= hn::caps::INTERACT;
        // Per-player RTT for the lobby (S2CPings to peers that ask for it).
        cfg.caps |= hn::caps::PING;
        // Combat hit effects (C2STouch / S2CHitFx).
        cfg.caps |= hn::caps::HIT_FX;
        // Passport bodies for stand-ins (body records).
        cfg.caps |= hn::caps::BODY;
        // Game modes: the `mode` / `kill_feed` records, and the King of the hill `zone`.
        cfg.caps |= hn::caps::MODES | hn::caps::ZONE;
        let conn_cfg = ConnConfig { idle_timeout_ms: SERVER_IDLE_MS, ..ConnConfig::default() };
        let ep = ServerEndpoint::new(cfg, conn_cfg, 0, None);
        let static_pub = ep.static_public();
        Net {
            ep: Mutex::new(ep),
            maps: RwLock::new(Maps::default()),
            t0: Instant::now(),
            epoch: rand::random::<u64>() | 1,
            static_pub,
            v4_rejects: Mutex::new(V4Rejects::new()),
            counters: Mutex::new(NetCounters::default()),
            pkts_in: AtomicU64::new(0),
            conn_report_at: AtomicU64::new(0),
        }
    }

    /// A transport with a throw-away identity (tests, tools).
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn ephemeral() -> Self {
        Net::new(rand::random(), None)
    }

    /// Milliseconds since this transport started (the one shared clock).
    pub fn now_ms(&self) -> u64 {
        self.t0.elapsed().as_millis() as u64
    }
    /// Random at boot; a change tells clients the server restarted.
    pub fn epoch(&self) -> u64 {
        self.epoch
    }
    pub fn static_public(&self) -> [u8; 32] {
        self.static_pub
    }
    pub fn fingerprint(&self) -> String {
        hn::handshake::player_fingerprint(&self.static_pub)
    }

    fn ep(&self) -> std::sync::MutexGuard<'_, ServerEndpoint> {
        self.ep.lock().unwrap_or_else(|e| e.into_inner())
    }
    fn maps(&self) -> std::sync::RwLockReadGuard<'_, Maps> {
        self.maps.read().unwrap_or_else(|e| e.into_inner())
    }
    fn maps_mut(&self) -> std::sync::RwLockWriteGuard<'_, Maps> {
        self.maps.write().unwrap_or_else(|e| e.into_inner())
    }
    /// The connection of `addr` (the peer's key, or a fresh alias of a
    /// peer that just migrated).
    fn cid(&self, addr: &SocketAddr) -> Option<ConnId> {
        let m = self.maps();
        m.by_addr.get(addr).copied().or_else(|| {
            let now = self.now_ms();
            m.aliases.get(addr).filter(|(_, until)| *until > now).map(|(c, _)| *c)
        })
    }
    fn count(&self, f: impl FnOnce(&mut NetCounters)) {
        f(&mut self.counters.lock().unwrap_or_else(|e| e.into_inner()));
    }

    /// Feed one received datagram. Returns what the application must do and
    /// the datagrams to send back (handshake replies), never larger than the
    /// request before the cookie is proven.
    pub fn handle(&self, from: SocketAddr, dg: &[u8]) -> (Action, Out) {
        self.pkts_in.fetch_add(1, Ordering::Relaxed);
        let now = self.now_ms();
        let inc = self.ep().handle(now, from, dg);
        match inc {
            Incoming::Reply(r) => {
                // a Hello refused before any state (protocol or content mismatch)
                if let Ok(p) = hn::handshake::parse_pre_reject(&r) {
                    let why = match p.code {
                        hn::handshake::reject_code::VERSION => "version",
                        hn::handshake::reject_code::CONTENT => "content",
                        _ => "other",
                    };
                    crate::stats::refused(why);
                    if crate::validate::rate::log_ok("join_refused") {
                        tracing::info!(%from, code = p.code, why, text = %p.text, "join refused: {why} mismatch");
                    }
                }
                (Action::None, vec![(from, r)])
            }
            Incoming::AuthRequest(p) => (Action::Auth(p), Vec::new()),
            Incoming::Data { cid, deliveries, migrated_from, probe } => {
                // The peer key of this connection (the address admission
                // registered, until a migration is committed).
                let Some(peer) = self.maps().by_cid.get(&cid).copied() else {
                    return (Action::None, Vec::new());
                };
                let out = probe.map(|p| vec![(from, p)]).unwrap_or_default();
                let migrated_to = migrated_from.map(|old| {
                    self.count(|c| c.migrations += 1);
                    tracing::info!(%old, new = %from, peer = %peer, "v5: connection migrated (NAT rebind / new path validated)");
                    from
                });
                (Action::Data { deliveries, peer, migrated_to }, out)
            }
            Incoming::Dropped(r) => {
                use hn::endpoint::DropReason as D;
                let mut out = Vec::new();
                match r {
                    D::LegacyV4 => {
                        self.count(|c| c.dropped_legacy += 1);
                        if let Some(rej) = self.v4_reject(now, from, dg) {
                            out.push((from, rej));
                        }
                    }
                    D::UnknownConn => self.count(|c| c.dropped_unknown_conn += 1),
                    D::AddrMismatch => self.count(|c| c.dropped_addr_mismatch += 1),
                    D::Conn(hn::conn::Drop::Duplicate) | D::Conn(hn::conn::Drop::TooOld) | D::DuplicateAuth => {
                        self.count(|c| c.dropped_replay += 1)
                    }
                    D::Conn(hn::conn::Drop::Auth) => self.count(|c| c.dropped_auth_fail += 1),
                    D::Handshake(_) => self.count(|c| c.dropped_handshake += 1),
                    _ => self.count(|c| c.dropped_garbage += 1),
                }
                debug!(%from, reason = ?r, len = dg.len(), "v5: datagram dropped");
                (Action::None, out)
            }
        }
    }

    /// One bounded v4-format `S2CReject` for an old client's cleartext
    /// `C2SJoin`: reply <= request, at most once per IP per 10 s.
    fn v4_reject(&self, now: u64, from: SocketAddr, req: &[u8]) -> Option<Vec<u8>> {
        // v4 `Datagram::Clear(Envelope{ZERO_TOKEN, Body::C2SJoin{..}})`:
        // u32 0 | 16 zero bytes | u32 0 (C2SJoin) | ...
        if req.len() < 24 || req[..4] != [0, 0, 0, 0] || req[20..24] != [0, 0, 0, 0] {
            return None;
        }
        // Bounded and O(1) per packet: a global budget, then
        // once per IP per 10 s; fail closed when the memory is full.
        if !self.v4_rejects.lock().unwrap_or_else(|e| e.into_inner()).allow(now, hn::ip_key(from.ip())) {
            return None;
        }
        // Datagram::Clear | ZERO_TOKEN | Body::S2CReject (tag 5) | String.
        const HEAD: usize = 4 + 16 + 4 + 8;
        if req.len() <= HEAD {
            return None;
        }
        let mut text = V4_REJECT_TEXT.as_bytes();
        let room = req.len() - HEAD;
        if text.len() > room {
            let mut end = room;
            while end > 0 && !V4_REJECT_TEXT.is_char_boundary(end) {
                end -= 1;
            }
            text = &text[..end];
        }
        let mut v = Vec::with_capacity(HEAD + text.len());
        v.extend_from_slice(&0u32.to_le_bytes());
        v.extend_from_slice(&[0u8; 16]);
        v.extend_from_slice(&5u32.to_le_bytes());
        v.extend_from_slice(&(text.len() as u64).to_le_bytes());
        v.extend_from_slice(text);
        self.count(|c| c.v4_rejects += 1);
        Some(v)
    }

    /// Admit a verified Auth: allocates the connection for `p.addr`.
    pub fn accept(&self, p: Box<PendingAuth>) -> ConnId {
        let now = self.now_ms();
        let addr = p.addr;
        let cid = self.ep().accept(now, p);
        let old = {
            let mut m = self.maps_mut();
            m.aliases.remove(&addr);
            let old = m.by_addr.insert(addr, cid);
            if let Some(o) = old {
                m.by_cid.remove(&o);
            }
            m.by_cid.insert(cid, addr);
            old
        };
        if let Some(old) = old.filter(|o| *o != cid) {
            self.ep().close(now, old, close_code::REPLACED, "replaced");
        }
        cid
    }

    /// The peer known as `old` now lives at `new` (its connection migrated):
    /// re-key the address maps. Call it under the peer-table lock, in the
    /// same critical section that re-keys `Inner::peers`, so `reconcile`
    /// never sees one without the other. `old` stays routable for ALIAS_MS.
    /// A live connection known at `new` is never displaced: the
    /// migration is refused (false, maps unchanged). A dead one (closed,
    /// not reaped yet) is replaced. Returns true when committed.
    pub fn commit_migration(&self, old: SocketAddr, new: SocketAddr) -> bool {
        let now = self.now_ms();
        // Never hold the endpoint lock under the maps lock.
        let other = {
            let m = self.maps();
            let Some(&cid) = m.by_addr.get(&old) else { return false };
            m.by_addr.get(&new).copied().filter(|c| *c != cid)
        };
        if other.is_some_and(|d| self.ep().conn(d).is_some_and(|c| !c.is_closed())) {
            return false;
        }
        let mut m = self.maps_mut();
        let Some(&cid) = m.by_addr.get(&old) else { return false };
        let displaced = m.by_addr.get(&new).copied().filter(|c| *c != cid);
        if let Some(d) = displaced {
            m.by_cid.remove(&d);
        }
        m.by_addr.remove(&old);
        m.by_addr.insert(new, cid);
        m.by_cid.insert(cid, new);
        m.aliases.retain(|_, (_, until)| *until > now);
        m.aliases.remove(&new);
        if m.aliases.len() < 4096 {
            m.aliases.insert(old, (cid, now + ALIAS_MS));
        }
        drop(m);
        if let Some(d) = displaced {
            self.ep().close(now, d, close_code::REPLACED, "address taken over");
        }
        true
    }

    /// A live (open) connection is known at `addr`.
    pub fn live_at(&self, addr: SocketAddr) -> bool {
        let Some(cid) = self.maps().by_addr.get(&addr).copied() else { return false };
        self.ep().conn(cid).is_some_and(|c| !c.is_closed())
    }

    /// Refuse a verified Auth: the sealed reject to send to `p.addr`.
    pub fn reject(&self, p: Box<PendingAuth>, code: u8, text: &str) -> Out {
        let addr = p.addr;
        let now = self.now_ms();
        vec![(addr, self.ep().reject(now, p, code, text))]
    }

    /// Queue one v6 record message (`hsmp_ipc::wire` framed) for `addr` on its kind's
    /// channel (keyed by the header's peer). An unknown / local-only kind is dropped.
    pub fn send_msg(&self, addr: SocketAddr, msg: Vec<u8>) -> Out {
        let (kind, peer) = match hsmp_ipc::wire::split(&msg) {
            Ok((h, _)) => (h.kind, h.peer),
            Err(_) => return Vec::new(),
        };
        match proto::record_mode(kind, peer) {
            Some(mode) => self.send_bytes(addr, mode, msg),
            None => {
                debug!(%addr, kind, "v6: not a network record kind; dropped");
                Vec::new()
            }
        }
    }

    /// Queue several v6 record messages for `addr` (each on its kind's channel), then
    /// transmit once: messages produced together share datagrams. Unknown kinds are dropped.
    pub fn send_msgs(&self, addr: SocketAddr, msgs: Vec<Vec<u8>>) -> Out {
        let Some(cid) = self.cid(&addr) else { return Vec::new() };
        let now = self.now_ms();
        let mut ep = self.ep();
        let mut backpressure = 0;
        for msg in msgs {
            let Ok((h, _)) = hsmp_ipc::wire::split(&msg) else { continue };
            let Some(mode) = proto::record_mode(h.kind, h.peer) else {
                debug!(%addr, kind = h.kind, "v6: not a network record kind; dropped");
                continue;
            };
            match ep.send(cid, fit_mode(mode, msg.len()), msg) {
                Ok(()) => {}
                Err(SendError::Backpressure) => {
                    backpressure += 1;
                    debug!(%addr, "v6: reliable backlog full; message dropped");
                }
                Err(e) => debug!(%addr, ?e, "v6: send refused"),
            }
        }
        let out = ep.poll_transmit_one(now, cid);
        drop(ep);
        if backpressure > 0 {
            self.count(|c| c.backpressure += backpressure);
        }
        out
    }

    /// Relay path: the body is serialised once by the caller.
    pub fn send_bytes(&self, addr: SocketAddr, mode: SendMode, bytes: Vec<u8>) -> Out {
        if !self.queue_bytes(addr, mode, bytes) {
            return Vec::new();
        }
        self.flush(addr)
    }

    /// Queue a message for `addr` without transmitting it: messages queued for one peer
    /// before its `flush` share datagrams. False if it was dropped.
    pub fn queue_bytes(&self, addr: SocketAddr, mode: SendMode, bytes: Vec<u8>) -> bool {
        let Some(cid) = self.cid(&addr) else { return false };
        let mut ep = self.ep();
        // A channel-0 message too big for one datagram (a large voice frame)
        // travels reliably and fragmented instead. Decided up front so the
        // buffer is moved, not cloned for a retry.
        let r = ep.send(cid, fit_mode(mode, bytes.len()), bytes);
        match r {
            Ok(()) => true,
            Err(SendError::Backpressure) => {
                drop(ep);
                self.count(|c| c.backpressure += 1);
                debug!(%addr, "v5: reliable backlog full; message dropped");
                false
            }
            Err(e) => {
                debug!(%addr, ?e, "v5: send refused");
                false
            }
        }
    }

    /// The datagrams of everything queued for `addr`.
    pub fn flush(&self, addr: SocketAddr) -> Out {
        let Some(cid) = self.cid(&addr) else { return Vec::new() };
        let now = self.now_ms();
        self.ep().poll_transmit_one(now, cid)
    }

    /// Close `addr`'s connection (CLOSE is repeated by `tick`). Returns the
    /// first CLOSE datagram.
    pub fn close(&self, addr: SocketAddr, code: u8, reason: &str) -> Out {
        let Some(cid) = self.cid(&addr) else { return Vec::new() };
        let now = self.now_ms();
        let mut ep = self.ep();
        ep.close(now, cid, code, reason);
        ep.poll_transmit_one(now, cid)
    }

    /// Start closing `addr`'s connection without flushing (safe under any
    /// lock); the CLOSE datagrams go out with the next `tick`.
    pub fn close_later(&self, addr: &SocketAddr, code: u8) {
        if let Some(cid) = self.cid(addr) {
            let now = self.now_ms();
            self.ep().close(now, cid, code, "closed");
        }
    }

    /// Session resume: the peer known as `addr` continues on a
    /// new connection at another address. Unmap `addr` now (so reaping the
    /// old connection never reports the resumed peer as gone) and close the
    /// old connection with `code`. Returns the CLOSE datagrams to send.
    pub fn retire(&self, addr: SocketAddr, code: u8, reason: &str) -> Out {
        let cid = {
            let mut m = self.maps_mut();
            m.aliases.remove(&addr);
            let Some(cid) = m.by_addr.remove(&addr) else { return Vec::new() };
            m.by_cid.remove(&cid);
            cid
        };
        let now = self.now_ms();
        let mut ep = self.ep();
        ep.close(now, cid, code, reason);
        ep.poll_transmit_one(now, cid)
            .into_iter()
            .collect()
    }

    /// Timer pass: acks, retransmits, keepalives, CLOSE repeats; reap closed
    /// connections. Returns the datagrams and the addresses whose (current)
    /// connection ended, with its final state.
    pub fn tick(&self) -> (Out, Vec<(SocketAddr, ConnState)>) {
        let now = self.now_ms();
        let (out, reaped) = {
            let mut ep = self.ep();
            let out = ep.poll_transmit(now);
            // Every 10 s: per-connection transport health summed over all
            // connections (loss, retransmits, spurious losses, pacing; the
            // network measurements read this line).
            let last = self.conn_report_at.load(Ordering::Relaxed);
            if now >= last + CONN_REPORT_MS {
                self.conn_report_at.store(now, Ordering::Relaxed);
                let mut t = hn::ConnStats::default();
                let mut min_rate = f64::INFINITY;
                for c in ep.conn_ids() {
                    let Some(s) = ep.conn(c).map(|c| c.stats()) else { continue };
                    t.pkts_sent += s.pkts_sent;
                    t.pkts_recv += s.pkts_recv;
                    t.pkts_lost += s.pkts_lost;
                    t.retransmits += s.retransmits;
                    t.spurious_lost += s.spurious_lost;
                    t.paced += s.paced;
                    t.bytes_sent += s.bytes_sent;
                    min_rate = min_rate.min(s.cc_rate_bps);
                }
                if ep.conn_count() > 0 {
                    tracing::info!(conns = ep.conn_count(), pkts_sent = t.pkts_sent, pkts_recv = t.pkts_recv,
                        pkts_lost = t.pkts_lost, retransmits = t.retransmits, spurious_lost = t.spurious_lost,
                        paced = t.paced, bytes_sent = t.bytes_sent, cc_rate_min_kbs = (min_rate / 1024.0) as u64,
                        "v5 connection stats (per-connection totals)");
                }
            }
            (out, ep.reap_closed())
        };
        let mut dead = Vec::new();
        if !reaped.is_empty() {
            let mut m = self.maps_mut();
            for (cid, _path, st) in reaped {
                m.aliases.retain(|_, (c, _)| *c != cid);
                // Keyed by the peer's address (the path may already have
                // moved before the re-key was committed).
                if let Some(key) = m.by_cid.remove(&cid) {
                    if m.by_addr.get(&key) == Some(&cid) {
                        m.by_addr.remove(&key);
                        dead.push((key, st));
                    }
                }
            }
        }
        (out, dead)
    }

    /// Smoothed RTT (ms) of every connection that has a sample yet.
    pub fn srtt_by_addr(&self) -> Vec<(SocketAddr, f32)> {
        let m: Vec<(SocketAddr, ConnId)> = self.maps().by_addr.iter().map(|(a, c)| (*a, *c)).collect();
        let ep = self.ep();
        m.into_iter()
            .filter_map(|(a, c)| ep.conn(c).map(|cn| (a, cn.stats().srtt_ms as f32)))
            .filter(|(_, r)| r.is_finite() && *r > 0.0)
            .collect()
    }

    /// RTT variation (ms, the RFC 6298 `rttvar`) of every connection with an
    /// RTT sample: the server-measured jitter that bounds what lag comp
    /// believes of a client's stream jitter.
    pub fn rttvar_by_addr(&self) -> Vec<(SocketAddr, f32)> {
        let m: Vec<(SocketAddr, ConnId)> = self.maps().by_addr
            .iter().map(|(a, c)| (*a, *c)).collect();
        let ep = self.ep();
        m.into_iter()
            .filter_map(|(a, c)| ep.conn(c).map(|cn| (a, cn.stats())))
            .filter(|(_, s)| s.srtt_ms > 0.0 && s.rttvar_ms.is_finite())
            .map(|(a, s)| (a, s.rttvar_ms as f32))
            .collect()
    }

    /// Loss and RTT counters of every connection, keyed by peer address: what
    /// the relay reads to notice a congested path to a recipient.
    pub fn path_samples(&self) -> Vec<(SocketAddr, crate::relay::PathSample)> {
        let m: Vec<(SocketAddr, ConnId)> = self.maps().by_addr.iter().map(|(a, c)| (*a, *c)).collect();
        let ep = self.ep();
        m.into_iter()
            .filter_map(|(a, c)| ep.conn(c).map(|cn| (a, cn.stats())))
            .map(|(a, s)| (a, crate::relay::PathSample {
                pkts_sent: s.pkts_sent,
                // Losses later acked after all were reordering, not loss.
                pkts_lost: s.pkts_lost.saturating_sub(s.spurious_lost),
                srtt_ms: s.srtt_ms,
                min_rtt_ms: s.min_rtt_ms,
            }))
            .collect()
    }

    /// Every connection's transport counters, keyed by peer address (the 10 s stats line).
    pub fn conn_stats_by_addr(&self) -> Vec<(SocketAddr, hn::ConnStats)> {
        let m: Vec<(SocketAddr, ConnId)> = self.maps().by_addr.iter().map(|(a, c)| (*a, *c)).collect();
        let ep = self.ep();
        m.into_iter().filter_map(|(a, c)| ep.conn(c).map(|cn| (a, cn.stats()))).collect()
    }

    /// Addresses with a live connection.
    pub fn addrs(&self) -> Vec<SocketAddr> {
        self.maps().by_addr.keys().copied().collect()
    }

    /// `addrs` into a buffer the caller keeps (the tick reuses its own).
    pub fn addrs_into(&self, out: &mut Vec<SocketAddr>) {
        out.clear();
        out.extend(self.maps().by_addr.keys().copied());
    }

    /// Close every connection whose address is no longer a peer (kick, ban,
    /// RCON and timeout paths that removed the peer directly). Cheap when
    /// nothing changed. Call it under the peer-table lock (admission accepts
    /// under the same lock, so a fresh connection always has its peer). The
    /// CLOSE datagrams go out with the next `tick`.
    pub fn reconcile<V>(&self, peers: &HashMap<SocketAddr, V>) {
        // Runs every tick: the unchanged case allocates nothing.
        let stale: Vec<(SocketAddr, ConnId)> = {
            let mm = self.maps();
            let m = &mm.by_addr;
            if m.len() == peers.len() && m.keys().all(|a| peers.contains_key(a)) {
                return;
            }
            m.iter().filter(|(a, _)| !peers.contains_key(*a)).map(|(a, c)| (*a, *c)).collect()
        };
        let now = self.now_ms();
        let mut ep = self.ep();
        for (addr, cid) in stale {
            ep.close(now, cid, close_code::NORMAL, "removed");
            debug!(%addr, "v5: closing the connection of a removed peer");
        }
    }

    pub fn counters(&self) -> NetCounters {
        self.counters.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }
    /// (connections, pre-auth state entries, AEAD failures summed over
    /// connections, endpoint stats).
    pub fn stats(&self) -> (usize, usize, u64, hn::endpoint::EndpointStats) {
        let ep = self.ep();
        let auth_failed: u64 = ep.conn_ids().iter().filter_map(|c| ep.conn(*c)).map(|c| c.stats().auth_failed).sum();
        (ep.conn_count(), ep.preauth_state(), auth_failed, ep.stats().clone())
    }
}

/// Load the server's X25519 static secret, creating it on first run.
pub fn load_or_create_key(path: &Path) -> anyhow::Result<[u8; 32]> {
    if let Ok(b) = std::fs::read(path) {
        if b.len() == 32 {
            let mut k = [0u8; 32];
            k.copy_from_slice(&b);
            return Ok(k);
        }
        warn!(path = %path.display(), len = b.len(), "server key file malformed; replacing it");
    }
    let k: [u8; 32] = {
        use rand::RngCore;
        let mut k = [0u8; 32];
        rand::rngs::OsRng.fill_bytes(&mut k);
        k
    };
    if let Some(dir) = path.parent().filter(|d| !d.as_os_str().is_empty()) {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, k)?;
    std::fs::rename(&tmp, path)?;
    Ok(k)
}

/// Default location of the server identity: `$HSMP_STATE_DIR`, else
/// `%LOCALAPPDATA%\HSMP`, else the working directory.
pub fn default_key_path() -> PathBuf {
    let base = std::env::var_os("HSMP_STATE_DIR")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("LOCALAPPDATA").filter(|v| !v.is_empty()).map(|d| PathBuf::from(d).join("HSMP")));
    match base {
        Some(b) => b.join("server_identity.key"),
        None => PathBuf::from("server_identity.key"),
    }
}

/// Test helpers: a real v5 client driven against `Net` without sockets.
#[cfg(test)]
pub(crate) mod testkit {
    use super::*;
    use hsmp_net::net::{Client, ClientConfig};

    /// Any small reliable message (a v6 `leave` record).
    pub(crate) fn test_msg() -> Vec<u8> {
        hsmp_ipc::wire::encode(0, 0, &hsmp_ipc::schema::session::Leave { reason: 0, _r: [0; 7] }, &[])
    }

    /// Handshake a client at `addr`; returns it connected (the Auth was
    /// accepted with `Net::accept`) and its connection id.
    pub(crate) fn connect(net: &Net, addr: SocketAddr, seed: u8) -> (Client, ConnId) {
        let mut c = Client::new(ClientConfig::new([seed; 32], "t"), ConnConfig::default(), net.now_ms(), &mut rand::rngs::OsRng);
        let mut cid = None;
        for _ in 0..20 {
            let now = net.now_ms();
            while let Some(dg) = c.poll_transmit(now) {
                let (act, out) = net.handle(addr, &dg);
                if let Action::Auth(p) = act {
                    cid = Some(net.accept(p));
                    // The server's first packet confirms the connection.
                    for (_, d) in net.send_msg(addr, test_msg()) { c.handle(now, &d); }
                }
                for (_, r) in out { c.handle(now, &r); }
            }
            if c.is_connected() { break; }
        }
        assert!(c.is_connected(), "test client did not connect");
        (c, cid.expect("accepted"))
    }

    /// Flush the client and feed its datagrams to `net` as coming from
    /// `from`; datagrams the server sends back to `from` are handed to the
    /// client. Returns the actions.
    pub(crate) fn pump(net: &Net, c: &mut Client, from: SocketAddr) -> Vec<Action> {
        let mut acts = Vec::new();
        let now = net.now_ms();
        while let Some(dg) = c.poll_transmit(now) {
            let (act, out) = net.handle(from, &dg);
            for (to, r) in out {
                if to == from { c.handle(now, &r); }
            }
            acts.push(act);
        }
        acts
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use testkit::test_msg;

    /// A NAT rebind keeps the same connection. Data from
    /// the new address is delivered at once under the peer's known address;
    /// the path moves when the client acks the probe; the old address stays
    /// routable briefly and `reconcile` never closes the moved connection.
    #[test]
    fn nat_rebind_keeps_the_connection_and_reports_the_migration() {
        use hsmp_net::net::SendMode;
        let net = Net::ephemeral();
        let old: SocketAddr = "203.0.113.5:40000".parse().unwrap();
        let new: SocketAddr = "203.0.113.5:41234".parse().unwrap();
        let (mut c, cid) = testkit::connect(&net, old, 3);
        // The NAT rebinds; the client keeps sending (from `new`).
        c.send(SendMode::Reliable, test_msg()).unwrap();
        let acts = testkit::pump(&net, &mut c, new);
        let first = acts.iter().find_map(|a| match a { Action::Data { deliveries, peer, migrated_to } => Some((deliveries.len(), *peer, *migrated_to)), _ => None });
        assert_eq!(first, Some((1, old, None)), "delivered at once, under the known address, not yet moved");
        // The client got the probe; its answer (next packet) validates the path,
        // which is taken once the old path has been silent for 100 ms.
        std::thread::sleep(std::time::Duration::from_millis(120));
        c.send(SendMode::Reliable, test_msg()).unwrap();
        let acts = testkit::pump(&net, &mut c, new);
        let moved = acts.iter().any(|a| matches!(a, Action::Data { peer, migrated_to: Some(n), .. } if *peer == old && *n == new));
        assert!(moved, "migration reported");
        // Not committed yet: sends to the known address still route.
        assert!(!net.send_msg(old, test_msg()).is_empty());
        assert!(net.commit_migration(old, new));
        assert_eq!(net.addrs(), vec![new]);
        let out = net.send_msg(new, test_msg());
        assert!(!out.is_empty() && out.iter().all(|(a, _)| *a == new), "server now sends to the new path");
        assert!(!net.send_msg(old, test_msg()).is_empty(), "old address is an alias for a while");
        net.reconcile(&HashMap::from([(new, ())]));
        assert!(net.ep().conn(cid).is_some_and(|c| c.is_open()), "reconcile keeps the moved connection");
        assert_eq!(net.counters().migrations, 1);
    }

    /// Data for a connection the server does not know draws a bounded,
    /// verifiable stateless reset (≤ the request).
    #[test]
    fn unknown_connection_gets_a_small_stateless_reset() {
        let key: [u8; 32] = rand::random();
        let net = Net::new(key, None);
        let a: SocketAddr = "198.51.100.9:5000".parse().unwrap();
        let (mut c, _cid) = testkit::connect(&net, a, 4);
        // Server restart: same identity key, no connections.
        let net2 = Net::new(key, None);
        c.send(hsmp_net::net::SendMode::Reliable, vec![1, 2, 3]).unwrap();
        let now = net2.now_ms();
        let dg = c.poll_transmit(now).expect("data");
        let (_, out) = net2.handle(a, &dg);
        assert_eq!(out.len(), 1);
        assert!(out[0].1.len() <= dg.len());
        assert_eq!(out[0].1[0], hn::PT_RESET);
    }

    #[test]
    fn v4_join_gets_one_bounded_outdated_reject() {
        let net = Net::ephemeral();
        let from: SocketAddr = "127.0.0.1:5000".parse().unwrap();
        // v4 Datagram::Clear(Envelope{ZERO_TOKEN, C2SJoin{4, "Willie"}}).
        let mut req = vec![0u8; 4 + 16 + 4];
        req.extend_from_slice(&4u32.to_le_bytes());
        req.extend_from_slice(&6u64.to_le_bytes());
        req.extend_from_slice(b"Willie");
        let (_, out) = net.handle(from, &req);
        assert_eq!(out.len(), 1);
        let rej = &out[0].1;
        assert!(rej.len() <= req.len(), "{} > {}", rej.len(), req.len());
        assert_eq!(&rej[20..24], &5u32.to_le_bytes(), "S2CReject tag");
        assert!(String::from_utf8_lossy(&rej[32..]).starts_with("OUTDATED"));
        // Rate-limited per IP.
        assert!(net.handle(from, &req).1.is_empty());
        // Non-join v4 datagrams get nothing.
        let other: SocketAddr = "127.0.0.2:5000".parse().unwrap();
        let mut enc = vec![1u8, 0, 0, 0];
        enc.extend_from_slice(&[0u8; 60]);
        assert!(net.handle(other, &enc).1.is_empty());
    }

    /// Spoofed v4 joins from many sources get a bounded number
    /// of replies (no reflection) and the per-packet cost stays O(1) even
    /// with a full memory.
    #[test]
    fn v4_reject_flood_is_bounded_and_fails_closed() {
        let mut l = V4Rejects::new();
        let ip = |n: u32| IpAddr::from(std::net::Ipv4Addr::from(0x0A00_0000 | n));
        // 10 000 spoofed sources in the same second: only the burst is answered.
        let replies = (0..10_000u32).filter(|n| l.allow(1_000, ip(*n))).count();
        assert_eq!(replies as f64, V4_REJECT_BURST);
        // Over 60 s at 5k pps the global budget holds (rate * time + burst).
        let mut replies = 0usize;
        for k in 0..300_000u32 {
            if l.allow(1_000 + k as u64 / 5, ip(20_000 + k)) { replies += 1; }
        }
        assert!(replies as f64 <= V4_REJECT_RATE_PER_S * 61.0 + V4_REJECT_BURST, "{replies}");
        assert!(l.per_ip.len() <= V4_REJECT_IPS);
        // A full memory of fresh entries: refuse, without growing.
        let mut l = V4Rejects::new();
        for n in 0..V4_REJECT_IPS as u32 { l.per_ip.insert(ip(n), 5_000); }
        l.tokens = V4_REJECT_BURST;
        assert!(!l.allow(5_000, ip(999_999)), "fail closed when full");
        assert_eq!(l.per_ip.len(), V4_REJECT_IPS);
        // Once the entries age out, the prune frees room again.
        assert!(l.allow(5_000 + V4_REJECT_EVERY_MS + V4_PRUNE_EVERY_MS, ip(999_999)));
        assert_eq!(l.per_ip.len(), 1);
    }

    /// The oversize fallback is decided before the send (the buffer is
    /// moved once), with the same outcome as a TooLarge retry.
    #[test]
    fn oversize_latest_goes_reliable_without_a_retry_copy() {
        // (hsmp-net's channel tests pin Outbox::send's TooLarge boundary at
        // exactly MAX_UNRELIABLE + 1.)
        use hsmp_net::net::channel::MAX_UNRELIABLE;
        let latest = SendMode::Latest { key: 3 };
        assert!(matches!(fit_mode(latest, MAX_UNRELIABLE), SendMode::Latest { key: 3 }));
        assert!(matches!(fit_mode(latest, MAX_UNRELIABLE + 1), SendMode::Reliable));
        assert!(matches!(fit_mode(SendMode::Reliable, 10), SendMode::Reliable));
    }

    /// One IPv6 host (a /64) gets one reject per window, however
    /// many addresses of its prefix it uses.
    #[test]
    fn v4_reject_memory_keys_ipv6_by_slash_64() {
        let mut l = V4Rejects::new();
        let v6 = |h: u16| IpAddr::from(std::net::Ipv6Addr::new(0x2001, 0xdb8, 9, 9, h, 0, 0, 1));
        assert!(l.allow(1_000, v6(1)));
        assert!(!l.allow(1_001, v6(2)), "same /64 = same host");
        assert!(l.allow(1_002, IpAddr::from(std::net::Ipv6Addr::new(0x2001, 0xdb8, 9, 10, 0, 0, 0, 1))));
    }

    /// The relay's congestion input: one sample per peer, keyed by its
    /// address, with the RTT once the client acked something.
    #[test]
    fn path_samples_cover_every_peer() {
        let net = Net::ephemeral();
        let a: SocketAddr = "198.51.100.20:6000".parse().unwrap();
        let (mut c, _) = testkit::connect(&net, a, 9);
        for _ in 0..3 {
            for (_, d) in net.send_msg(a, test_msg()) { c.handle(net.now_ms(), &d); }
            std::thread::sleep(std::time::Duration::from_millis(25));
            let _ = testkit::pump(&net, &mut c, a);
        }
        let s = net.path_samples();
        assert_eq!(s.len(), 1);
        assert_eq!(s[0].0, a);
        assert!(s[0].1.pkts_sent >= 3, "{:?}", s[0].1);
        assert!(s[0].1.srtt_ms > 0.0 && s[0].1.min_rtt_ms > 0.0, "{:?}", s[0].1);
        assert!(s[0].1.min_rtt_ms <= s[0].1.srtt_ms + 1e-9);
    }

    #[test]
    fn unknown_peers_get_nothing_and_sends_are_noops() {
        let net = Net::ephemeral();
        let a: SocketAddr = "127.0.0.1:5001".parse().unwrap();
        assert!(net.send_msg(a, test_msg()).is_empty());
        // Data for an unknown connection: at most a 25-byte stateless reset.
        let out = net.handle(a, &[0xB0; 60]).1;
        assert!(out.iter().all(|(_, r)| r.len() == hn::RESET_LEN && r[0] == hn::PT_RESET));
        assert!(net.handle(a, &[0xA1; 100]).1.is_empty(), "short hello dropped");
        assert_eq!(net.stats().0, 0);
    }
}
