//! The public server list as a pure state machine (no I/O, no clock). The Cloudflare Worker
//! (master-cf/) keeps one of these inside its Durable Object, loads it from SQLite on wake and
//! writes back the `Effect`s each call returns.
//!
//! Rules:
//! - Every register, heartbeat and delete is signed with the server's listing key (auth.rs);
//!   a listing belongs to that key. `ts` must grow per listing (replay guard).
//! - The listed host is the request's source address; the registrant only picks the port.
//! - One listing per host:port. The same key re-registering refreshes it; another key gets
//!   409 until the listing expires.
//! - A heartbeat from another address drops the listing (410) so the server registers again
//!   from its new address.
//! - Caps: servers in total, per host (IPv4 or IPv6 /64), per network (IPv4 /24, IPv6 /48).
//!   A full list refuses newcomers; it never evicts live entries.
//! - Rate limits: registrations per host and globally (token buckets), heartbeats per listing.
//!   They live in memory only; losing them on a restart is harmless.
//! - Optional announcements (server up / down) are debounced per listing and capped globally.
//! - Punch relay (punch.rs): the host of a listing may hold a signed listen socket; a punch
//!   request reaches it only for the requester's own endpoint, rate-limited per requester,
//!   per listing and globally. `Entry::punch` is true only while that socket is open.

use crate::auth;
use crate::bucket::Bucket;
use crate::fields::{self, DeleteReq, HeartbeatReq, RegisterReq, RegisterResp, MAX_BODY_BYTES, MIN_PORT};
use crate::ip;
use crate::punch;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, HashMap};
use std::net::IpAddr;

#[derive(Debug, Clone)]
pub struct Config {
    /// A listing without a heartbeat for this long is gone.
    pub ttl_s: u64,
    /// Told to every server at registration.
    pub heartbeat_s: u64,
    pub max_servers: usize,
    pub max_per_host: usize,
    pub max_per_net: usize,
    /// Registrations per host: bucket size and one token per `register_every_ms`.
    pub register_burst: f64,
    pub register_every_ms: u64,
    pub global_register_burst: f64,
    pub global_register_every_ms: u64,
    /// A heartbeat sooner than this after the last accepted one is refused.
    pub heartbeat_min_gap_ms: u64,
    /// Registration `ts` must be within this of the master's clock.
    pub ts_skew_ms: u64,
    pub announce: bool,
    /// After an "up" for a listing, the next "up" waits at least this long.
    pub flap_window_ms: u64,
    pub announce_burst: f64,
    pub announce_every_ms: u64,
}

impl Config {
    /// The public list: `heartbeat_s` and `ttl_s` come from the deployment (wrangler.toml).
    pub fn public(heartbeat_s: u64, ttl_s: u64, announce: bool) -> Config {
        let heartbeat_s = heartbeat_s.clamp(5, 600);
        let ttl_s = ttl_s.max(heartbeat_s * 2);
        Config {
            ttl_s,
            heartbeat_s,
            max_servers: 500,
            max_per_host: 8,
            max_per_net: 32,
            register_burst: 6.0,
            register_every_ms: 60_000,
            global_register_burst: 60.0,
            global_register_every_ms: 1_000,
            heartbeat_min_gap_ms: (heartbeat_s * 1000 / 4).max(2_000),
            ts_skew_ms: 10 * 60_000,
            announce,
            flap_window_ms: 30 * 60_000,
            announce_burst: 10.0,
            announce_every_ms: 60_000,
        }
    }
}

/// One stored listing (persisted as JSON by the Worker).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Listing {
    pub server_id: String,
    pub listing_key: String,
    pub host: String,
    pub port: u16,
    pub name: String,
    pub mode: String,
    pub map: String,
    pub players: u32,
    pub max_players: u32,
    pub proto_ver: u32,
    pub proto_min: u32,
    pub proto_max: u32,
    pub server_key: String,
    pub pwd_protected: bool,
    pub version: String,
    pub region: String,
    #[serde(default)]
    pub content_hash: String,
    pub registered_ms: u64,
    pub last_seen_ms: u64,
    pub last_ts: u64,
    /// `fields::NAT_KINDS`, "" = not reported.
    #[serde(default)]
    pub nat: String,
    /// The server said it takes punch requests.
    #[serde(default)]
    pub punch: bool,
}

/// One `GET /v1/servers` entry. Same fields as hsmp-master's, plus `server_key`,
/// `proto_min` and `proto_max`. `ping_ms` / `reachable` stay 0 / false: a Worker cannot
/// send UDP, and the browser measures both itself.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Entry {
    pub server_id: String,
    pub name: String,
    pub host: String,
    pub port: u16,
    pub mode: String,
    pub map: String,
    pub players: u32,
    pub max_players: u32,
    pub proto_ver: u32,
    pub proto_min: u32,
    pub proto_max: u32,
    pub server_key: String,
    pub pwd_protected: bool,
    pub version: String,
    pub region: String,
    #[serde(default)]
    pub content_hash: String,
    pub ping_ms: u32,
    pub reachable: bool,
    pub last_seen_utc_ms: u64,
    pub age_s: u64,
    /// How the server is reachable (`fields::NAT_KINDS`, "" = not reported).
    #[serde(default)]
    pub nat: String,
    /// A punch request reaches this server right now (its listen socket is open).
    #[serde(default)]
    pub punch: bool,
}

/// Announcement bookkeeping for one listing (persisted so a restart does not repeat "up").
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AnnounceState {
    pub key: String,
    /// The last message sent was "up".
    pub up: bool,
    pub at_ms: u64,
    pub last_up_ms: u64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Announcement {
    pub up: bool,
    pub name: String,
    pub mode: String,
    pub map: String,
    pub players: u32,
    pub max_players: u32,
    pub region: String,
    pub version: String,
    pub addr: String,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Effect {
    Put(Listing),
    Remove(String),
    PutAnnounce(AnnounceState),
    RemoveAnnounce(String),
    Announce(Announcement),
}

#[derive(Debug, Clone, PartialEq)]
pub struct Reply {
    pub status: u16,
    pub body: String,
    pub json: bool,
}

impl Reply {
    fn text(status: u16, msg: &str) -> Reply {
        Reply { status, body: msg.to_string(), json: false }
    }
    fn json<T: Serialize>(status: u16, v: &T) -> Reply {
        Reply { status, body: serde_json::to_string(v).unwrap_or_default(), json: true }
    }
    fn empty() -> Reply {
        Reply { status: 204, body: String::new(), json: false }
    }
}

/// Bound for the in-memory rate-limit maps (a flood of addresses resets them, never grows them).
const MAX_BUCKETS: usize = 20_000;
/// Announcement states for listings that went away are forgotten after this.
const ANNOUNCE_KEEP_MS: u64 = 7 * 24 * 3600 * 1000;

/// Deterministic listing id: the same server at the same address keeps its id.
pub fn server_id(listing_key: &str, host: &str, port: u16) -> String {
    let mut h = Sha256::new();
    h.update(b"hsmp listing\n");
    h.update(listing_key.as_bytes());
    h.update(b"\n");
    h.update(host.as_bytes());
    h.update(b"\n");
    h.update(port.to_string().as_bytes());
    hex::encode(&h.finalize()[..16])
}

pub struct Registry {
    cfg: Config,
    rows: BTreeMap<String, Listing>,
    announce: HashMap<String, AnnounceState>,
    reg_rate: HashMap<IpAddr, Bucket>,
    global_rate: Bucket,
    announce_rate: Bucket,
    last_hb: HashMap<String, u64>,
    /// Listings whose host holds a punch listen socket right now (the Worker rebuilds this
    /// from its hibernated WebSockets on wake).
    listeners: std::collections::HashSet<String>,
    /// Newest accepted listen `ts` per listing (replay guard for the listen request).
    listen_ts: HashMap<String, u64>,
    punch_rate: punch::Limiter,
}

/// A punch request the master accepted: send `msg` down `server_id`'s listen socket.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PunchOrder {
    pub server_id: String,
    pub msg: String,
}

impl Registry {
    pub fn new(cfg: Config, rows: Vec<Listing>, announce: Vec<AnnounceState>, now: u64) -> Registry {
        Registry {
            global_rate: Bucket::full(cfg.global_register_burst, now),
            announce_rate: Bucket::full(cfg.announce_burst, now),
            rows: rows.into_iter().map(|r| (r.server_id.clone(), r)).collect(),
            announce: announce.into_iter().map(|a| (a.key.clone(), a)).collect(),
            reg_rate: HashMap::new(),
            last_hb: HashMap::new(),
            listeners: Default::default(),
            listen_ts: HashMap::new(),
            punch_rate: punch::Limiter::new(punch::Limits::default(), now),
            cfg,
        }
    }

    /// The host of `server_id` opened (`true`) or lost (`false`) its listen socket.
    pub fn set_listener(&mut self, server_id: &str, live: bool) {
        if live {
            if self.rows.contains_key(server_id) {
                self.listeners.insert(server_id.to_string());
            }
        } else {
            self.listeners.remove(server_id);
        }
    }

    pub fn is_listening(&self, server_id: &str) -> bool {
        self.listeners.contains(server_id)
    }

    /// `GET /v1/punch/listen/{id}?ts=..` (`path_and_query` exactly as requested, signed by
    /// the listing key over an empty body). Ok = accept the WebSocket.
    pub fn listen(&mut self, now: u64, path_and_query: &str, sig: Option<&str>) -> Result<String, Reply> {
        let Some((id, ts)) = punch::parse_listen_path(path_and_query) else {
            return Err(Reply::text(400, "expected /v1/punch/listen/<id>?ts=<unix ms>"));
        };
        let Some(row) = self.rows.get(&id).filter(|l| !self.stale(l, now)) else {
            return Err(Reply::text(404, "unknown or expired listing (register first)"));
        };
        if now.abs_diff(ts) > punch::LISTEN_SKEW_MS {
            return Err(Reply::text(400, "clock skew: ts is more than 5 minutes off"));
        }
        if auth::verify(&row.listing_key, sig, "GET", path_and_query, b"").is_err() {
            return Err(Reply::text(403, "bad signature"));
        }
        if self.listen_ts.get(&id).is_some_and(|last| ts <= *last) {
            return Err(Reply::text(409, "stale ts"));
        }
        if self.listen_ts.len() >= MAX_BUCKETS {
            self.listen_ts.clear();
        }
        self.listen_ts.insert(id.clone(), ts);
        Ok(id)
    }

    /// `POST /v1/punch` from `ip`. On 202 the order says what to send to which host.
    pub fn punch(&mut self, now: u64, ip: IpAddr, body: &[u8]) -> (Reply, Option<PunchOrder>) {
        let ip = ip::canonical(ip);
        if body.len() > MAX_BODY_BYTES {
            return (Reply::text(413, "body too large"), None);
        }
        let Ok(req) = serde_json::from_slice::<punch::PunchReq>(body) else {
            return (Reply::text(400, "bad JSON (host, port, endpoint)"), None);
        };
        let ep = match punch::check_request(&req, ip) {
            Ok(ep) => ep,
            Err((code, why)) => return (Reply::text(code, why), None),
        };
        if !self.punch_rate.requester_ok(now, ip) {
            return (Reply::text(429, "punch throttled (this address)"), None);
        }
        let host = req.host.trim().trim_start_matches('[').trim_end_matches(']');
        let host = host.parse::<IpAddr>().map(|h| ip::canonical(h).to_string()).unwrap_or_else(|_| host.to_string());
        let Some(row) = self.rows.values().find(|l| l.host == host && l.port == req.port && !self.stale(l, now)) else {
            return (Reply::text(404, "no such server in the list"), None);
        };
        let id = row.server_id.clone();
        if !self.listeners.contains(&id) {
            return (Reply::text(409, "this server does not take punch requests (update it, or it lost its link to the list)"), None);
        }
        if !self.punch_rate.server_ok(now, &id) {
            return (Reply::text(429, "punch throttled (this server)"), None);
        }
        let msg = punch::PunchMsg { t: "punch".into(), to: ep.to_string(), nonce: req.nonce.clone() };
        let msg = serde_json::to_string(&msg).unwrap_or_default();
        (Reply::json(202, &punch::PunchResp { sent: true }), Some(PunchOrder { server_id: id, msg }))
    }

    pub fn config(&self) -> &Config {
        &self.cfg
    }

    pub fn len(&self) -> usize {
        self.rows.len()
    }

    pub fn is_empty(&self) -> bool {
        self.rows.is_empty()
    }

    fn stale(&self, l: &Listing, now: u64) -> bool {
        now.saturating_sub(l.last_seen_ms) > self.cfg.ttl_s * 1000
    }

    /// When the oldest listing expires (for the sweep alarm).
    pub fn next_expiry_ms(&self) -> Option<u64> {
        self.rows.values().map(|l| l.last_seen_ms + self.cfg.ttl_s * 1000 + 1).min()
    }

    /// Live listings, newest heartbeat first.
    pub fn list(&self, now: u64) -> Vec<Entry> {
        let mut v: Vec<&Listing> = self.rows.values().filter(|l| !self.stale(l, now)).collect();
        v.sort_by(|a, b| b.last_seen_ms.cmp(&a.last_seen_ms).then(a.server_id.cmp(&b.server_id)));
        v.into_iter()
            .map(|l| Entry {
                server_id: l.server_id.clone(),
                name: l.name.clone(),
                host: l.host.clone(),
                port: l.port,
                mode: l.mode.clone(),
                map: l.map.clone(),
                players: l.players,
                max_players: l.max_players,
                proto_ver: l.proto_ver,
                proto_min: l.proto_min,
                proto_max: l.proto_max,
                server_key: l.server_key.clone(),
                pwd_protected: l.pwd_protected,
                version: l.version.clone(),
                region: l.region.clone(),
                content_hash: l.content_hash.clone(),
                ping_ms: 0,
                reachable: false,
                last_seen_utc_ms: l.last_seen_ms,
                age_s: now.saturating_sub(l.last_seen_ms) / 1000,
                nat: l.nat.clone(),
                punch: l.punch && self.listeners.contains(&l.server_id),
            })
            .collect()
    }

    /// Drop expired listings and forgotten bookkeeping.
    pub fn sweep(&mut self, now: u64) -> Vec<Effect> {
        let mut fx = Vec::new();
        self.sweep_into(now, &mut fx);
        fx
    }

    fn sweep_into(&mut self, now: u64, fx: &mut Vec<Effect>) {
        let gone: Vec<String> = self.rows.values().filter(|l| self.stale(l, now)).map(|l| l.server_id.clone()).collect();
        for id in gone {
            if let Some(l) = self.rows.remove(&id) {
                fx.push(Effect::Remove(id.clone()));
                self.announce_down(now, &l, fx);
            }
            self.last_hb.remove(&id);
            self.listeners.remove(&id);
            self.listen_ts.remove(&id);
        }
        let (cap, every) = (self.cfg.register_burst, self.cfg.register_every_ms);
        self.reg_rate.retain(|_, b| !b.is_full(now, cap, every));
        self.punch_rate.sweep(now);
        let live: std::collections::HashSet<String> = self.rows.values().map(announce_key).collect();
        let forget: Vec<String> = self
            .announce
            .values()
            .filter(|a| !live.contains(&a.key) && now.saturating_sub(a.at_ms) > ANNOUNCE_KEEP_MS)
            .map(|a| a.key.clone())
            .collect();
        for k in forget {
            self.announce.remove(&k);
            fx.push(Effect::RemoveAnnounce(k));
        }
    }

    fn rate_ok(&mut self, now: u64, ip: IpAddr) -> bool {
        if self.reg_rate.len() >= MAX_BUCKETS {
            self.reg_rate.clear();
        }
        let (cap, every) = (self.cfg.register_burst, self.cfg.register_every_ms);
        let host = self.reg_rate.entry(ip::host_key(ip)).or_insert_with(|| Bucket::full(cap, now));
        if !host.take(now, cap, every) {
            return false;
        }
        self.global_rate.take(now, self.cfg.global_register_burst, self.cfg.global_register_every_ms)
    }

    /// `POST /v1/register` from `ip` (the request's source address) with the raw body and the
    /// `x-hsmp-sig` header.
    pub fn register(&mut self, now: u64, ip: IpAddr, body: &[u8], sig: Option<&str>) -> (Reply, Vec<Effect>) {
        let mut fx = Vec::new();
        let ip = ip::canonical(ip);
        if body.len() > MAX_BODY_BYTES {
            return (Reply::text(413, "body too large"), fx);
        }
        if !self.rate_ok(now, ip) {
            return (Reply::text(429, "register throttled"), fx);
        }
        let Ok(req) = serde_json::from_slice::<RegisterReq>(body) else {
            return (Reply::text(400, "bad JSON or missing name/port"), fx);
        };
        let (Some(key), Some(ts)) = (req.listing_key.as_deref(), req.ts) else {
            return (Reply::text(400, "signed registration required (update hsmp-server)"), fx);
        };
        let key = key.to_ascii_lowercase();
        if now.abs_diff(ts) > self.cfg.ts_skew_ms {
            return (Reply::text(400, "clock skew: ts is more than 10 minutes off (sync the server clock)"), fx);
        }
        if auth::verify(&key, sig, "POST", "/v1/register", body).is_err() {
            return (Reply::text(403, "bad signature"), fx);
        }
        let f = match fields::validate_register(&req, MIN_PORT) {
            Ok(f) => f,
            Err(why) => return (Reply::text(400, why), fx),
        };
        self.sweep_into(now, &mut fx);
        let host = ip.to_string();
        let existing = self.rows.values().find(|l| l.host == host && l.port == f.port).cloned();
        if let Some(old) = &existing {
            if old.listing_key != key {
                return (Reply::text(409, "this address and port are listed by another server key"), fx);
            }
            if ts <= old.last_ts {
                return (Reply::text(409, "stale ts"), fx);
            }
        } else {
            let hk = ip::host_key(ip);
            let nk = ip::net_key(ip);
            let parse = |h: &str| h.parse::<IpAddr>().ok();
            let per_host = self.rows.values().filter(|l| parse(&l.host).map(ip::host_key) == Some(hk)).count();
            let per_net = self.rows.values().filter(|l| parse(&l.host).map(ip::net_key) == Some(nk)).count();
            if per_host >= self.cfg.max_per_host || per_net >= self.cfg.max_per_net {
                return (Reply::text(429, "too many servers from this address"), fx);
            }
            if self.rows.len() >= self.cfg.max_servers {
                return (Reply::text(503, "server list full"), fx);
            }
        }
        let id = server_id(&key, &host, f.port);
        let l = Listing {
            server_id: id.clone(),
            listing_key: key,
            host,
            port: f.port,
            name: f.name,
            mode: f.mode,
            map: f.map,
            players: f.players,
            max_players: f.max_players,
            proto_ver: f.proto_ver,
            proto_min: f.proto_min,
            proto_max: f.proto_max,
            server_key: f.server_key,
            pwd_protected: f.pwd_protected,
            version: f.version,
            region: f.region,
            content_hash: f.content_hash,
            registered_ms: existing.as_ref().map(|o| o.registered_ms).unwrap_or(now),
            last_seen_ms: now,
            last_ts: ts,
            nat: f.nat,
            punch: f.punch,
        };
        self.rows.insert(id.clone(), l.clone());
        fx.push(Effect::Put(l.clone()));
        if existing.is_none() {
            self.announce_up(now, &l, &mut fx);
        }
        let resp = RegisterResp { server_id: id, ttl_s: self.cfg.ttl_s, secret: String::new(), heartbeat_s: Some(self.cfg.heartbeat_s) };
        (Reply::json(201, &resp), fx)
    }

    /// `POST /v1/heartbeat/{id}`.
    pub fn heartbeat(&mut self, now: u64, ip: IpAddr, id: &str, body: &[u8], sig: Option<&str>) -> (Reply, Vec<Effect>) {
        let mut fx = Vec::new();
        if body.len() > MAX_BODY_BYTES {
            return (Reply::text(413, "body too large"), fx);
        }
        let Some(row) = self.rows.get(id).cloned() else {
            return (Reply::text(410, "unknown or expired"), fx);
        };
        if self.stale(&row, now) {
            self.sweep_into(now, &mut fx);
            return (Reply::text(410, "expired"), fx);
        }
        if let Some(prev) = self.last_hb.get(id) {
            if now.saturating_sub(*prev) < self.cfg.heartbeat_min_gap_ms {
                return (Reply::text(429, "heartbeat throttled"), fx);
            }
        }
        let Ok(req) = serde_json::from_slice::<HeartbeatReq>(body) else {
            return (Reply::text(400, "bad JSON"), fx);
        };
        let Some(ts) = req.ts else {
            return (Reply::text(403, "signature required"), fx);
        };
        if auth::verify(&row.listing_key, sig, "POST", &format!("/v1/heartbeat/{id}"), body).is_err() {
            return (Reply::text(403, "bad signature"), fx);
        }
        if ts <= row.last_ts {
            return (Reply::text(409, "stale ts"), fx);
        }
        if ip::canonical(ip).to_string() != row.host {
            // The server's address changed: it registers again from the new one.
            self.rows.remove(id);
            self.last_hb.remove(id);
            self.listeners.remove(id);
            fx.push(Effect::Remove(id.to_string()));
            return (Reply::text(410, "address changed; register again"), fx);
        }
        let mut l = row;
        l.players = req.players.min(l.max_players);
        if let Some(m) = fields::heartbeat_map(req.map.as_deref()) {
            l.map = m;
        }
        if let Some(m) = fields::heartbeat_mode(req.mode.as_deref()) {
            l.mode = m;
        }
        if req.nat.is_some() {
            l.nat = fields::nat_kind(req.nat.as_deref());
        }
        l.last_seen_ms = now;
        l.last_ts = ts;
        self.rows.insert(id.to_string(), l.clone());
        self.last_hb.insert(id.to_string(), now);
        fx.push(Effect::Put(l));
        (Reply::empty(), fx)
    }

    /// `DELETE /v1/servers/{id}`.
    pub fn delete(&mut self, now: u64, id: &str, body: &[u8], sig: Option<&str>) -> (Reply, Vec<Effect>) {
        let mut fx = Vec::new();
        if body.len() > MAX_BODY_BYTES {
            return (Reply::text(413, "body too large"), fx);
        }
        let Some(row) = self.rows.get(id).cloned() else {
            return (Reply::text(404, "unknown"), fx);
        };
        let Ok(req) = serde_json::from_slice::<DeleteReq>(body) else {
            return (Reply::text(400, "bad JSON"), fx);
        };
        let Some(ts) = req.ts else {
            return (Reply::text(403, "signature required"), fx);
        };
        if auth::verify(&row.listing_key, sig, "DELETE", &format!("/v1/servers/{id}"), body).is_err() {
            return (Reply::text(403, "bad signature"), fx);
        }
        if ts <= row.last_ts {
            return (Reply::text(409, "stale ts"), fx);
        }
        self.rows.remove(id);
        self.last_hb.remove(id);
        self.listeners.remove(id);
        fx.push(Effect::Remove(id.to_string()));
        self.announce_down(now, &row, &mut fx);
        (Reply::empty(), fx)
    }

    fn announce_up(&mut self, now: u64, l: &Listing, fx: &mut Vec<Effect>) {
        if !self.cfg.announce {
            return;
        }
        let key = announce_key(l);
        let allowed = match self.announce.get(&key) {
            None => true,
            // "up" already stands in the channel (its "down" was never sent)
            Some(s) if s.up => false,
            Some(s) => now.saturating_sub(s.last_up_ms) >= self.cfg.flap_window_ms,
        };
        if !allowed || !self.announce_rate.take(now, self.cfg.announce_burst, self.cfg.announce_every_ms) {
            return;
        }
        let s = AnnounceState { key: key.clone(), up: true, at_ms: now, last_up_ms: now };
        self.announce.insert(key, s.clone());
        fx.push(Effect::PutAnnounce(s));
        fx.push(Effect::Announce(announcement(l, true)));
    }

    fn announce_down(&mut self, now: u64, l: &Listing, fx: &mut Vec<Effect>) {
        if !self.cfg.announce {
            return;
        }
        let key = announce_key(l);
        let Some(s) = self.announce.get_mut(&key) else { return };
        if !s.up {
            return;
        }
        s.up = false;
        s.at_ms = now;
        let s = s.clone();
        fx.push(Effect::PutAnnounce(s));
        // Over the global cap the "down" is recorded but not sent.
        if self.announce_rate.take(now, self.cfg.announce_burst, self.cfg.announce_every_ms) {
            fx.push(Effect::Announce(announcement(l, false)));
        }
    }
}

/// Announcements are per server key and port, so a server whose address changes does not
/// announce itself again.
fn announce_key(l: &Listing) -> String {
    format!("{}:{}", l.listing_key, l.port)
}

fn announcement(l: &Listing, up: bool) -> Announcement {
    Announcement {
        up,
        name: l.name.clone(),
        mode: l.mode.clone(),
        map: l.map.clone(),
        players: l.players,
        max_players: l.max_players,
        region: l.region.clone(),
        version: l.version.clone(),
        addr: if l.host.contains(':') { format!("[{}]:{}", l.host, l.port) } else { format!("{}:{}", l.host, l.port) },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ed25519_dalek::SigningKey;
    use serde_json::json;

    const T0: u64 = 1_800_000_000_000;

    fn cfg() -> Config {
        Config::public(120, 360, true)
    }

    fn reg() -> Registry {
        Registry::new(cfg(), vec![], vec![], T0)
    }

    fn ip(s: &str) -> IpAddr {
        s.parse().unwrap()
    }

    fn key(n: u8) -> SigningKey {
        auth::listing_key(&[n; 32])
    }

    fn reg_body(sk: &SigningKey, port: u16, ts: u64) -> Vec<u8> {
        serde_json::to_vec(&json!({
            "name": "EU Duels", "host": "198.51.100.200", "port": port, "mode": "Best of 3",
            "map": "Map_Arena_Pit", "players": 1, "max_players": 8, "proto_ver": 5,
            "proto_min": 5, "proto_max": 5, "server_key": "cd".repeat(32), "pwd_protected": false,
            "version": "0.2.0", "region": "EU", "nonce": "n", "hmac": "",
            "listing_key": auth::public_hex(sk), "ts": ts,
        }))
        .unwrap()
    }

    fn register(r: &mut Registry, now: u64, from: &str, sk: &SigningKey, port: u16, ts: u64) -> (Reply, Vec<Effect>) {
        let b = reg_body(sk, port, ts);
        let sig = auth::sign(sk, "POST", "/v1/register", &b);
        r.register(now, ip(from), &b, Some(&sig))
    }

    fn hb(r: &mut Registry, now: u64, from: &str, sk: &SigningKey, id: &str, players: u32, ts: u64) -> Reply {
        let b = serde_json::to_vec(&json!({"players": players, "map": "Map_Arena_Yard", "mode": "Best of 5", "nonce": "x", "hmac": "", "ts": ts})).unwrap();
        let sig = auth::sign(sk, "POST", &format!("/v1/heartbeat/{id}"), &b);
        r.heartbeat(now, ip(from), id, &b, Some(&sig)).0
    }

    fn del(r: &mut Registry, now: u64, sk: &SigningKey, id: &str, ts: u64) -> (Reply, Vec<Effect>) {
        let b = serde_json::to_vec(&json!({"nonce": "d", "hmac": "", "ts": ts})).unwrap();
        let sig = auth::sign(sk, "DELETE", &format!("/v1/servers/{id}"), &b);
        r.delete(now, id, &b, Some(&sig))
    }

    fn id_of(rep: &Reply) -> String {
        let v: RegisterResp = serde_json::from_str(&rep.body).unwrap();
        v.server_id
    }

    fn announced(fx: &[Effect]) -> Vec<bool> {
        fx.iter().filter_map(|e| if let Effect::Announce(a) = e { Some(a.up) } else { None }).collect()
    }

    #[test]
    fn register_lists_the_source_address_and_answers_the_old_shape() {
        let mut r = reg();
        let sk = key(1);
        let (rep, fx) = register(&mut r, T0, "203.0.113.5", &sk, 7777, T0);
        assert_eq!(rep.status, 201, "{}", rep.body);
        let v: serde_json::Value = serde_json::from_str(&rep.body).unwrap();
        assert_eq!(v["ttl_s"], 360);
        assert_eq!(v["heartbeat_s"], 120);
        assert_eq!(v["secret"], "");
        assert_eq!(v["server_id"].as_str().unwrap().len(), 32);
        assert!(matches!(fx[0], Effect::Put(_)));
        let l = r.list(T0 + 5_000);
        assert_eq!(l.len(), 1);
        // never the client-claimed host
        assert_eq!(l[0].host, "203.0.113.5");
        assert_eq!(l[0].server_key, "cd".repeat(32));
        assert_eq!(l[0].age_s, 5);
        // the JSON the browser reads has every field hsmp-master lists
        let j = serde_json::to_value(&l[0]).unwrap();
        for k in ["server_id", "name", "host", "port", "mode", "map", "players", "max_players", "proto_ver", "pwd_protected",
                  "version", "region", "ping_ms", "reachable", "last_seen_utc_ms", "age_s"] {
            assert!(j.get(k).is_some(), "missing {k}");
        }
        // IPv4-mapped IPv6 is listed as IPv4
        let (rep, _) = register(&mut r, T0, "::ffff:198.51.100.7", &key(2), 7777, T0);
        assert_eq!(rep.status, 201);
        assert!(r.list(T0).iter().any(|e| e.host == "198.51.100.7"));
    }

    #[test]
    fn unsigned_forged_and_skewed_registrations_are_refused() {
        let mut r = reg();
        let sk = key(1);
        // legacy unsigned body
        let legacy = br#"{"name":"x","port":7777,"nonce":"n","hmac":""}"#;
        assert_eq!(r.register(T0, ip("203.0.113.5"), legacy, None).0.status, 400);
        // signed by another key than the one it names
        let b = reg_body(&sk, 7777, T0);
        let sig = auth::sign(&key(2), "POST", "/v1/register", &b);
        assert_eq!(r.register(T0, ip("203.0.113.6"), &b, Some(&sig)).0.status, 403);
        // no signature header
        assert_eq!(r.register(T0, ip("203.0.113.7"), &b, None).0.status, 403);
        // clock 11 minutes off
        assert_eq!(register(&mut r, T0, "203.0.113.8", &sk, 7777, T0 - 11 * 60_000).0.status, 400);
        // port below 1024
        assert_eq!(register(&mut r, T0, "203.0.113.9", &sk, 53, T0).0.status, 400);
        assert!(r.is_empty());
        // oversized body
        assert_eq!(r.register(T0, ip("203.0.113.10"), &vec![b' '; MAX_BODY_BYTES + 1], None).0.status, 413);
    }

    #[test]
    fn only_the_key_holder_can_heartbeat_or_delete() {
        let mut r = reg();
        let (sk, thief) = (key(1), key(9));
        let id = id_of(&register(&mut r, T0, "203.0.113.5", &sk, 7777, T0).0);
        // the thief shares the IP (CGNAT) and knows the public id
        assert_eq!(hb(&mut r, T0 + 60_000, "203.0.113.5", &thief, &id, 0, T0 + 60_000).status, 403);
        assert_eq!(del(&mut r, T0 + 60_000, &thief, &id, T0 + 60_000).0.status, 403);
        // and cannot take over the address with its own key
        assert_eq!(register(&mut r, T0 + 60_000, "203.0.113.5", &thief, 7777, T0 + 60_000).0.status, 409);
        // unsigned heartbeat (an old hsmp-server)
        let b = br#"{"players":1,"nonce":"n","hmac":"aaaa"}"#;
        assert_eq!(r.heartbeat(T0 + 60_000, ip("203.0.113.5"), &id, b, None).0.status, 403);
        // the owner can
        assert_eq!(hb(&mut r, T0 + 60_000, "203.0.113.5", &sk, &id, 3, T0 + 60_000).status, 204);
        let e = &r.list(T0 + 60_000)[0];
        assert_eq!((e.players, e.map.as_str(), e.mode.as_str()), (3, "Map_Arena_Yard", "Best of 5"));
        assert_eq!(del(&mut r, T0 + 120_000, &sk, &id, T0 + 120_000).0.status, 204);
        assert!(r.list(T0 + 120_000).is_empty());
        assert_eq!(del(&mut r, T0 + 120_000, &sk, &id, T0 + 130_000).0.status, 404);
    }

    #[test]
    fn replays_and_stale_timestamps_are_refused() {
        let mut r = reg();
        let sk = key(1);
        let id = id_of(&register(&mut r, T0, "203.0.113.5", &sk, 7777, T0).0);
        let b = serde_json::to_vec(&json!({"players": 2, "ts": T0 + 1})).unwrap();
        let sig = auth::sign(&sk, "POST", &format!("/v1/heartbeat/{id}"), &b);
        assert_eq!(r.heartbeat(T0 + 60_000, ip("203.0.113.5"), &id, &b, Some(&sig)).0.status, 204);
        // the same request again, after the throttle gap
        assert_eq!(r.heartbeat(T0 + 120_000, ip("203.0.113.5"), &id, &b, Some(&sig)).0.status, 409);
        // a re-registration with an old ts
        assert_eq!(register(&mut r, T0 + 120_000, "203.0.113.5", &sk, 7777, T0).0.status, 409);
    }

    #[test]
    fn heartbeats_are_throttled_and_expiry_forces_reregistration() {
        let mut r = reg();
        let sk = key(1);
        let id = id_of(&register(&mut r, T0, "203.0.113.5", &sk, 7777, T0).0);
        assert_eq!(hb(&mut r, T0 + 40_000, "203.0.113.5", &sk, &id, 1, T0 + 40_000).status, 204);
        assert_eq!(hb(&mut r, T0 + 41_000, "203.0.113.5", &sk, &id, 1, T0 + 41_000).status, 429);
        // past the TTL: hidden at once, heartbeat answers 410
        let late = T0 + 40_000 + 361_000;
        assert!(r.list(late).is_empty());
        assert_eq!(hb(&mut r, late, "203.0.113.5", &sk, &id, 1, late).status, 410);
        assert!(r.is_empty());
        assert_eq!(r.next_expiry_ms(), None);
    }

    #[test]
    fn a_new_source_address_drops_the_listing() {
        let mut r = reg();
        let sk = key(1);
        let id = id_of(&register(&mut r, T0, "203.0.113.5", &sk, 7777, T0).0);
        let (rep, fx) = {
            let b = serde_json::to_vec(&json!({"players": 1, "ts": T0 + 60_000})).unwrap();
            let sig = auth::sign(&sk, "POST", &format!("/v1/heartbeat/{id}"), &b);
            r.heartbeat(T0 + 60_000, ip("198.51.100.1"), &id, &b, Some(&sig))
        };
        assert_eq!(rep.status, 410);
        assert_eq!(fx, vec![Effect::Remove(id)]);
        assert_eq!(register(&mut r, T0 + 60_000, "198.51.100.1", &sk, 7777, T0 + 60_001).0.status, 201);
        assert_eq!(r.list(T0 + 60_000)[0].host, "198.51.100.1");
    }

    #[test]
    fn restart_keeps_the_id_and_one_entry() {
        let mut r = reg();
        let sk = key(1);
        let a = id_of(&register(&mut r, T0, "203.0.113.5", &sk, 7777, T0).0);
        let b = id_of(&register(&mut r, T0 + 5_000, "203.0.113.5", &sk, 7777, T0 + 5_000).0);
        assert_eq!(a, b);
        assert_eq!(r.len(), 1);
        // another port is another listing
        assert_eq!(register(&mut r, T0 + 6_000, "203.0.113.5", &sk, 7778, T0 + 6_000).0.status, 201);
        assert_eq!(r.len(), 2);
    }

    #[test]
    fn caps_per_host_per_network_and_in_total() {
        let mut c = cfg();
        c.max_servers = 40;
        c.register_burst = 1000.0;
        let mut r = Registry::new(c, vec![], vec![], T0);
        for p in 0..8u16 {
            assert_eq!(register(&mut r, T0, "203.0.113.5", &key(1), 7000 + p, T0).0.status, 201);
        }
        assert_eq!(register(&mut r, T0, "203.0.113.5", &key(1), 7100, T0).0.status, 429, "per host");
        // the same /24 from other addresses, up to 32
        let mut n = 8;
        for h in 6..=40u8 {
            let s = register(&mut r, T0, &format!("203.0.113.{h}"), &key(h), 7777, T0).0.status;
            if n < 32 { assert_eq!(s, 201); n += 1; } else { assert_eq!(s, 429, "per /24"); }
        }
        // IPv6: one /64 is one host, one /48 one network
        for k in 0..8u16 {
            let a = format!("2001:db8:1:2::{}", k + 1);
            assert_eq!(register(&mut r, T0, &a, &key(100 + k as u8), 7777, T0).0.status, 201);
        }
        assert_eq!(register(&mut r, T0, "2001:db8:1:2::99", &key(200), 7777, T0).0.status, 429);
        assert_eq!(r.len(), 40);
        assert_eq!(register(&mut r, T0, "192.0.2.1", &key(201), 7777, T0).0.status, 503, "list full");
        // full refuses newcomers but expired entries make room
        let later = T0 + 361_000;
        assert_eq!(register(&mut r, later, "192.0.2.1", &key(201), 7777, later).0.status, 201);
    }

    #[test]
    fn registration_rate_per_host_and_global() {
        let mut r = reg();
        for p in 0..6u16 {
            assert_eq!(register(&mut r, T0, "203.0.113.5", &key(1), 7000 + p, T0 + p as u64).0.status, 201);
        }
        assert_eq!(register(&mut r, T0, "203.0.113.5", &key(1), 7010, T0 + 10).0.status, 429);
        // one token per minute comes back
        assert_eq!(register(&mut r, T0 + 60_000, "203.0.113.5", &key(1), 7010, T0 + 60_000).0.status, 201);
        // global: 60 at once, then about one per second
        let mut c = cfg();
        c.max_servers = 1000;
        c.max_per_net = 1000;
        let mut g = Registry::new(c, vec![], vec![], T0);
        let ok = (0..80u32)
            .filter(|i| {
                let a = format!("10.{}.{}.1", i / 200, i % 200);
                register(&mut g, T0, &a, &key(*i as u8), 7777, T0).0.status == 201
            })
            .count();
        assert_eq!(ok, 60);
    }

    #[test]
    fn announcements_are_debounced_and_capped() {
        let mut r = reg();
        let sk = key(1);
        let (_, fx) = register(&mut r, T0, "203.0.113.5", &sk, 7777, T0);
        assert_eq!(announced(&fx), vec![true]);
        let id = r.list(T0)[0].server_id.clone();
        // a restart (same key, same address) is not news
        assert_eq!(announced(&register(&mut r, T0 + 1_000, "203.0.113.5", &sk, 7777, T0 + 1_000).1), Vec::<bool>::new());
        // shut down: "down"
        assert_eq!(announced(&del(&mut r, T0 + 2_000, &sk, &id, T0 + 2_000).1), vec![false]);
        // flapping back within 30 minutes: silent, and so is its next "down"
        let (_, fx) = register(&mut r, T0 + 60_000, "203.0.113.5", &sk, 7777, T0 + 60_000);
        assert!(announced(&fx).is_empty());
        assert!(announced(&del(&mut r, T0 + 61_000, &sk, &id, T0 + 61_000).1).is_empty());
        // after the window it is announced again; expiry sends the "down"
        let t = T0 + 31 * 60_000;
        assert_eq!(announced(&register(&mut r, t, "203.0.113.5", &sk, 7777, t).1), vec![true]);
        assert_eq!(announced(&r.sweep(t + 361_000)), vec![false]);
        // global cap: 10 at once
        let mut g = reg();
        let n: usize = (0..30u8).map(|i| announced(&register(&mut g, T0, &format!("10.0.{i}.1"), &key(i), 7777, T0).1).len()).sum();
        assert_eq!(n, 10);
        // announcements off: nothing
        let mut q = Registry::new(Config::public(120, 360, false), vec![], vec![], T0);
        assert!(announced(&register(&mut q, T0, "203.0.113.5", &sk, 7777, T0).1).is_empty());
    }

    #[test]
    fn state_survives_a_reload() {
        let mut r = reg();
        let sk = key(1);
        let mut rows = Vec::new();
        let mut ann = Vec::new();
        let (_, fx) = register(&mut r, T0, "203.0.113.5", &sk, 7777, T0);
        for e in fx {
            match e {
                Effect::Put(l) => rows.push(l),
                Effect::PutAnnounce(a) => ann.push(a),
                _ => {}
            }
        }
        let mut r2 = Registry::new(cfg(), rows, ann, T0 + 1_000);
        assert_eq!(r2.list(T0 + 1_000).len(), 1);
        let id = r2.list(T0)[0].server_id.clone();
        // replay of the original registration is still refused, the owner still works
        assert_eq!(register(&mut r2, T0 + 1_000, "203.0.113.5", &sk, 7777, T0).0.status, 409);
        assert_eq!(hb(&mut r2, T0 + 60_000, "203.0.113.5", &sk, &id, 2, T0 + 60_000).status, 204);
        // the "up" was already sent before the reload
        let (_, fx) = del(&mut r2, T0 + 70_000, &sk, &id, T0 + 70_000);
        assert_eq!(announced(&fx), vec![false]);
    }

    fn listen(r: &mut Registry, now: u64, sk: &SigningKey, id: &str, ts: u64) -> Result<String, Reply> {
        let p = punch::listen_path(id, ts);
        let sig = auth::sign(sk, "GET", &p, b"");
        r.listen(now, &p, Some(&sig))
    }

    fn punch_req(r: &mut Registry, now: u64, from: &str, host: &str, port: u16, ep: &str) -> (Reply, Option<PunchOrder>) {
        let b = serde_json::to_vec(&json!({"host": host, "port": port, "endpoint": ep, "nonce": "c0ffee"})).unwrap();
        r.punch(now, ip(from), &b)
    }

    #[test]
    fn listen_socket_is_signed_fresh_and_not_replayable() {
        let mut r = reg();
        let (sk, thief) = (key(1), key(9));
        let id = id_of(&register(&mut r, T0, "203.0.113.5", &sk, 7777, T0).0);
        assert_eq!(listen(&mut r, T0, &thief, &id, T0).unwrap_err().status, 403);
        assert_eq!(listen(&mut r, T0, &sk, &id, T0 - 6 * 60_000).unwrap_err().status, 400, "clock skew");
        assert_eq!(listen(&mut r, T0, &sk, &"ab".repeat(16), T0).unwrap_err().status, 404);
        assert_eq!(listen(&mut r, T0, &sk, &id, T0), Ok(id.clone()));
        assert_eq!(listen(&mut r, T0, &sk, &id, T0).unwrap_err().status, 409, "replay");
        assert_eq!(listen(&mut r, T0 + 1, &sk, &id, T0 + 1), Ok(id.clone()), "a reconnect");
        // no signature at all, a malformed path
        assert_eq!(r.listen(T0, &punch::listen_path(&id, T0 + 2), None).unwrap_err().status, 403);
        assert_eq!(r.listen(T0, "/v1/punch/listen/x", None).unwrap_err().status, 400);
    }

    #[test]
    fn punch_reaches_only_a_listening_server_and_only_the_requester() {
        let mut r = reg();
        let sk = key(1);
        let b = {
            let mut v: serde_json::Value = serde_json::from_slice(&reg_body(&sk, 7777, T0)).unwrap();
            v["nat"] = json!("cone");
            v["punch"] = json!(true);
            serde_json::to_vec(&v).unwrap()
        };
        let sig = auth::sign(&sk, "POST", "/v1/register", &b);
        let id = id_of(&r.register(T0, ip("203.0.113.5"), &b, Some(&sig)).0);
        let e = &r.list(T0)[0];
        assert_eq!((e.nat.as_str(), e.punch), ("cone", false), "no listen socket yet");
        // not listening: 409, unknown server: 404
        assert_eq!(punch_req(&mut r, T0, "198.51.100.20", "203.0.113.5", 7777, "198.51.100.20:40000").0.status, 409);
        assert_eq!(punch_req(&mut r, T0, "198.51.100.20", "203.0.113.5", 7778, "198.51.100.20:40000").0.status, 404);
        r.set_listener(&id, true);
        assert!(r.list(T0)[0].punch);
        let (rep, order) = punch_req(&mut r, T0, "198.51.100.20", "203.0.113.5", 7777, "198.51.100.20:40000");
        assert_eq!(rep.status, 202, "{}", rep.body);
        let o = order.unwrap();
        assert_eq!(o.server_id, id);
        assert_eq!(o.msg, r#"{"t":"punch","to":"198.51.100.20:40000","nonce":"c0ffee"}"#);
        // aiming at someone else is refused before anything is sent
        let (rep, order) = punch_req(&mut r, T0, "198.51.100.20", "203.0.113.5", 7777, "192.0.2.99:40000");
        assert_eq!((rep.status, order), (403, None));
        // a heartbeat updates the kind; a delete closes the relay
        assert_eq!(hb(&mut r, T0 + 60_000, "203.0.113.5", &sk, &id, 1, T0 + 60_000).status, 204);
        assert_eq!(del(&mut r, T0 + 70_000, &sk, &id, T0 + 70_000).0.status, 204);
        assert!(!r.is_listening(&id));
        // a listener for an unknown listing is not recorded
        r.set_listener("ffff", true);
        assert!(!r.is_listening("ffff"));
    }

    #[test]
    fn punch_is_rate_limited_per_requester_and_per_server() {
        let mut r = reg();
        let id = id_of(&register(&mut r, T0, "203.0.113.5", &key(1), 7777, T0).0);
        r.set_listener(&id, true);
        let ok = |r: &mut Registry, from: &str, now| punch_req(r, now, from, "203.0.113.5", 7777, &format!("{from}:40000")).0.status;
        let n = (0..10).filter(|_| ok(&mut r, "198.51.100.20", T0) == 202).count();
        assert_eq!(n, 6, "per-address burst");
        assert_eq!(ok(&mut r, "198.51.100.20", T0 + 10_000), 202, "one more after 10 s");
        // many joiners at one server: the per-server bucket (12) caps the fan-in
        let n = (1..=30u8).filter(|i| ok(&mut r, &format!("192.0.2.{i}"), T0 + 10_000) == 202).count();
        // 12 per server, refilled one per 2 s: 6 used at T0, +5 by T0 + 10 s, 1 used again
        assert_eq!(n, 10);
    }

    #[test]
    fn config_bounds() {
        let c = Config::public(1, 1, false);
        assert_eq!(c.heartbeat_s, 5);
        assert_eq!(c.ttl_s, 10);
        assert_eq!(c.heartbeat_min_gap_ms, 2_000);
        let c = Config::public(120, 360, false);
        assert_eq!(c.heartbeat_min_gap_ms, 30_000);
    }
}
