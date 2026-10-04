//! Server side of the loadout domain (protocol v6 records, schema `hsmp_ipc::schema::loadout`).
//!
//! - `loadout` (appearance): versioned *state*. The owner's sidecar sends one record per
//!   version (reliable, newest per owner; the transport fragments it). The server validates
//!   it in place, keeps the newest version per peer as the framed message (`peer` = owner),
//!   relays it to everyone else once and replays it to late joiners.
//! - `kit` (class / gear selection, docs/development/subsystems/classes-loadout.md): validated against the host's
//!   rules and the shared catalogue, replaced by a class default when invalid, and broadcast
//!   as `kit_verdict` (`peer` = owner). `kit_rules_req` (admin) changes the rules, broadcast
//!   as `kit_rules`.

use crate::proto::PeerId;
use crate::server::ServerState;
use hsmp_ipc::layout::Str;
use hsmp_ipc::record::{to_payload, view, Invalid};
use hsmp_ipc::schema::loadout::{
    check_loadout_rows, Kit, KitRules, LoadoutHead, K_KIT, K_KIT_RULES_REQ, K_KIT_VERDICT,
    BodyHead, K_BODY, K_LOADOUT, VERDICT_ACCEPTED, VERDICT_DEFAULT, VERDICT_REPLACED,
};
use hsmp_ipc::wire::{self, WireHdr};
use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};
use tokio::net::UdpSocket;
use tokio::sync::Mutex;
use tracing::{debug, info};

pub use catalog::KitSel;
pub use hsmp_ipc::schema::loadout::{MODE_CLASSES, MODE_CUSTOM, MODE_FREE};

/// Loadout versions accepted per peer per second (normal traffic: one per gear change).
const MAX_LOADOUTS_PER_SEC: u32 = 8;

/// Send one framed record message to `addr` on its kind's channel.
async fn send_msg(socket: &UdpSocket, state: &Arc<ServerState>, addr: SocketAddr, msg: Vec<u8>) {
    let out = state.net.send_msg(addr, msg);
    crate::server::send_out(socket, state, out).await;
}

async fn send_all(socket: &UdpSocket, state: &Arc<ServerState>, addrs: &[SocketAddr], msg: &[u8]) {
    for a in addrs {
        send_msg(socket, state, *a, msg.to_vec()).await;
    }
}

/// One v6 record of this domain (kinds `0x05xx`) from `from` (`server/records.rs`).
pub async fn handle_record(
    socket: &Arc<UdpSocket>,
    state: &Arc<ServerState>,
    from: SocketAddr,
    h: WireHdr,
    payload: &[u8],
) -> anyhow::Result<()> {
    match h.kind {
        K_KIT => {
            let v = view::<Kit>(payload).map_err(crate::server::refused)?;
            let sel = KitSel::from_record(&v.head, &v.rows);
            on_kit(socket, state, from, v.head.seq, sel).await;
        }
        K_KIT_RULES_REQ => {
            let v = view::<KitRules>(payload).map_err(crate::server::refused)?;
            on_kit_rules(socket, state, from, v.head.mode, v.head.budget).await;
        }
        K_LOADOUT => {
            let v = view::<LoadoutHead>(payload).map_err(crate::server::refused)?;
            check_loadout_rows(&v.rows).map_err(crate::server::refused)?;
            on_loadout(socket, state, from, v.head.version, payload).await;
        }
        K_BODY => {
            let v = view::<BodyHead>(payload).map_err(crate::server::refused)?;
            on_body(socket, state, from, v.head.version, payload).await;
        }
        k => {
            // kit_verdict / kit_rules are server -> client only.
            debug!(%from, kind = k, "loadout: record kind not accepted from a client; dropped");
            return Err(crate::server::refused(Invalid::Kind(k)));
        }
    }
    Ok(())
}

// ---- appearance (loadout records) ----------------------------------------------------------

struct Stored {
    version: u32,
    /// The framed relay message (`peer` = owner): replayed as it is.
    msg: Vec<u8>,
    window_start: Instant,
    window_count: u32,
}

fn store() -> &'static Mutex<HashMap<PeerId, Stored>> {
    static S: OnceLock<Mutex<HashMap<PeerId, Stored>>> = OnceLock::new();
    S.get_or_init(|| Mutex::new(HashMap::new()))
}

/// What a loadout version from a peer does to its stored state.
#[derive(Debug, PartialEq, Eq)]
enum LoadoutStep {
    /// Stored and to be relayed.
    Relay,
    /// Older than the stored version, or the same bytes again: dropped.
    Stale,
    RateLimited,
}

fn store_loadout(st: &mut HashMap<PeerId, Stored>, pid: PeerId, version: u32, payload: &[u8], now: Instant) -> LoadoutStep {
    store_versioned(st, K_LOADOUT, pid, version, payload, now)
}

/// `store_loadout` for any versioned per-owner record of this domain (`loadout`, `body`).
fn store_versioned(st: &mut HashMap<PeerId, Stored>, kind: u16, pid: PeerId, version: u32, payload: &[u8], now: Instant) -> LoadoutStep {
    let e = st.entry(pid).or_insert_with(|| Stored { version: 0, msg: Vec::new(), window_start: now, window_count: 0 });
    if now.duration_since(e.window_start) >= Duration::from_secs(1) {
        e.window_start = now;
        e.window_count = 0;
    }
    e.window_count += 1;
    if e.window_count > MAX_LOADOUTS_PER_SEC {
        return LoadoutStep::RateLimited;
    }
    if !e.msg.is_empty() && (version < e.version || (version == e.version && &e.msg[wire::HDR..] == payload)) {
        return LoadoutStep::Stale;
    }
    e.version = version;
    e.msg = wire::message(kind, 0, pid, payload);
    LoadoutStep::Relay
}

/// One validated `loadout` record from `from`: store the newest version, relay it once.
async fn on_loadout(socket: &UdpSocket, state: &Arc<ServerState>, from: SocketAddr, version: u32, payload: &[u8]) {
    let (pid, others) = {
        let inner = state.lock().lock().await;
        let Some(p) = inner.peers.get(&from) else { return };
        let others: Vec<SocketAddr> = inner.peers.keys().filter(|a| **a != from).copied().collect();
        (p.id, others)
    };
    let msg = {
        let mut st = store().lock().await;
        match store_loadout(&mut st, pid, version, payload, Instant::now()) {
            LoadoutStep::Relay => st[&pid].msg.clone(),
            LoadoutStep::Stale => {
                debug!(peer_id = pid, version, "stale / duplicate loadout dropped");
                return;
            }
            LoadoutStep::RateLimited => {
                debug!(peer_id = pid, "loadout rate-limited");
                return;
            }
        }
    };
    info!(peer_id = pid, version, bytes = payload.len(), "loadout stored");
    send_all(socket, state, &others, &msg).await;
}


// ---- passport body (body records, caps::BODY) ----------------------------------------------

fn body_store() -> &'static Mutex<HashMap<PeerId, Stored>> {
    static S: OnceLock<Mutex<HashMap<PeerId, Stored>>> = OnceLock::new();
    S.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Players that get a `body` record of `owner`: every other one that negotiated
/// `caps::BODY` (a beta.4 sidecar never sees the record).
fn body_pick(peers: impl Iterator<Item = (SocketAddr, PeerId)>, owner: PeerId, caps: impl Fn(PeerId) -> u64) -> Vec<SocketAddr> {
    let mut out: Vec<SocketAddr> = peers
        .filter(|&(_, id)| id != owner && caps(id) & hsmp_net::net::caps::BODY != 0)
        .map(|(a, _)| a)
        .collect();
    out.sort();
    out
}

/// One validated `body` record from `from`: store the newest version, relay it once to the
/// players that can read it.
async fn on_body(socket: &UdpSocket, state: &Arc<ServerState>, from: SocketAddr, version: u32, payload: &[u8]) {
    let (pid, others) = {
        let inner = state.lock().lock().await;
        let Some(p) = inner.peers.get(&from) else { return };
        let others = body_pick(inner.peers.iter().map(|(a, p)| (*a, p.id)), p.id, crate::interact::peer_caps);
        (p.id, others)
    };
    let msg = {
        let mut st = body_store().lock().await;
        match store_versioned(&mut st, K_BODY, pid, version, payload, Instant::now()) {
            LoadoutStep::Relay => st[&pid].msg.clone(),
            LoadoutStep::Stale | LoadoutStep::RateLimited => {
                debug!(peer_id = pid, version, "body record dropped (stale, duplicate or rate-limited)");
                return;
            }
        }
    };
    info!(peer_id = pid, version, receivers = others.len(), "body stored");
    send_all(socket, state, &others, &msg).await;
}
/// Peer left: drop its stored loadout (peer ids are never reused, so they would otherwise
/// accumulate per reconnect).
pub async fn forget(pid: PeerId) {
    store().lock().await.remove(&pid);
    body_store().lock().await.remove(&pid);
    kit_gate().lock().unwrap_or_else(|e| e.into_inner()).pending.remove(&pid);
}

// ---- kit facts frozen during a round ---------------------------------------
//
// The damage / reach / armour model (`validate::damage`) judges a player by
// its kit facts. A kit is dressed in game only at the next spawn, so a
// C2SKit during a round must not change the facts at once (a poleaxe and full
// plate claimed while holding the spawn sword in cloth). From Live to the end
// of the round's settle the facts are frozen; a newer kit is held and
// applied when the round ends (the next spawn uses it).

#[derive(Default)]
struct KitGate {
    /// Peers whose kit facts are frozen right now.
    locked: std::collections::HashSet<PeerId>,
    pending: HashMap<PeerId, KitSel>,
}

fn kit_gate() -> &'static std::sync::Mutex<KitGate> {
    static G: std::sync::OnceLock<std::sync::Mutex<KitGate>> = std::sync::OnceLock::new();
    G.get_or_init(Default::default)
}

fn kit_facts_set(pid: PeerId, kit: &KitSel) {
    let mut g = kit_gate().lock().unwrap_or_else(|e| e.into_inner());
    if g.locked.contains(&pid) {
        g.pending.insert(pid, *kit);
    } else {
        g.pending.remove(&pid);
        crate::validate::damage::set_kit(pid, kit);
    }
}

/// Server tick: the peers fighting a round right now (Live, Paused, or a
/// finished round settling); empty otherwise. A peer leaving the set gets
/// its held kit applied.
pub fn set_round_lock(fighting: &[PeerId]) {
    let mut g = kit_gate().lock().unwrap_or_else(|e| e.into_inner());
    // Called every tick: the unchanged set returns without allocating.
    if fighting.len() == g.locked.len() && fighting.iter().all(|p| g.locked.contains(p)) { return; }
    let now: std::collections::HashSet<PeerId> = fighting.iter().copied().collect();
    if now == g.locked { return; }
    let freed: Vec<PeerId> = g.locked.difference(&now).copied().collect();
    g.locked = now;
    for pid in freed {
        if let Some(kit) = g.pending.remove(&pid) {
            crate::validate::damage::set_kit(pid, &kit);
        }
    }
}

/// Kit facts held back for `pid` (tests / diagnostics).
#[cfg(test)]
fn kit_pending(pid: PeerId) -> Option<KitSel> {
    kit_gate().lock().unwrap_or_else(|e| e.into_inner()).pending.get(&pid).cloned()
}

// =====================================================================
// Classes / gear selection
// =====================================================================

impl KitSel {
    /// The `kit_verdict` message for `peer`.
    fn verdict_msg(&self, peer: PeerId, rev: u64, ack_seq: u32, verdict: u8, reason: &str) -> Vec<u8> {
        let mut h = self.0.head;
        h.rev = rev;
        h.seq = ack_seq;
        h.verdict = verdict;
        h.reason = Str::new(reason);
        wire::message(K_KIT_VERDICT, 0, peer, &to_payload(&h, self.0.used()))
    }
}

pub const MIN_BUDGET: u16 = 1;
pub const MAX_BUDGET: u16 = 200;
pub const DEFAULT_BUDGET: u16 = 30;
/// Cosmetic index limits: [face, hair colour, cloth tint, reserved].
pub const COSMETIC_MAX: [u8; 4] = [7, 7, 7, 0];
const KIT_MSGS_PER_SEC: u32 = 8;
/// Every live kit + the rules are re-broadcast this often so a lost datagram
/// only delays convergence (UDP; no per-receiver acks).
const KIT_REBROADCAST: Duration = Duration::from_secs(10);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rules {
    pub mode: u8,
    pub budget: u16,
}

impl Rules {
    pub fn sanitized(mode: u8, budget: u16) -> Rules {
        Rules {
            mode: if mode > MODE_CUSTOM { MODE_FREE } else { mode },
            budget: budget.clamp(MIN_BUDGET, MAX_BUDGET),
        }
    }

    /// From HSMP_KIT_MODE (free|classes|custom|0|1|2) + HSMP_KIT_BUDGET.
    fn from_env() -> Rules {
        let mode = match std::env::var("HSMP_KIT_MODE").unwrap_or_default().trim().to_ascii_lowercase().as_str() {
            "classes" | "1" => MODE_CLASSES,
            "custom" | "2" => MODE_CUSTOM,
            _ => MODE_FREE,
        };
        let budget = std::env::var("HSMP_KIT_BUDGET").ok()
            .and_then(|b| b.trim().parse::<u16>().ok()).unwrap_or(DEFAULT_BUDGET);
        Rules::sanitized(mode, budget)
    }
}

/// Pure validation of a selection under `rules`. Err = human-readable reason. Field sizes
/// and the armour count are bounded by the record itself (`Str<32>`, 16 rows).
pub fn check_kit(k: &KitSel, rules: Rules) -> Result<(), String> {
    use catalog::Kind;
    if k.class() == "none" {
        // "Keep the game's own gear": only meaningful when nothing is enforced.
        return if rules.mode == MODE_FREE && k.armor_len() == 0 && k.r().is_empty() && k.l().is_empty() {
            Ok(())
        } else {
            Err("game-default gear is only allowed in FREE mode".into())
        };
    }
    let Some(class) = catalog::class(k.class()) else {
        return Err(format!("unknown class '{}'", k.class()));
    };
    let mut groups: Vec<&str> = Vec::new();
    for a in k.armor() {
        match catalog::item(a) {
            Some(it) if it.kind == Kind::Armor => {
                if groups.contains(&it.group) {
                    return Err(format!("two items in slot {}", it.group));
                }
                groups.push(it.group);
            }
            _ => return Err(format!("unknown armour '{}'", a)),
        }
    }
    let weapon = |id: &str| -> Result<Option<&'static catalog::Item>, String> {
        if id.is_empty() { return Ok(None); }
        match catalog::item(id) {
            Some(it) if it.kind == Kind::Weapon => Ok(Some(it)),
            _ => Err(format!("unknown weapon '{}'", id)),
        }
    };
    let r = weapon(k.r())?;
    let l = weapon(k.l())?;
    if r.is_some_and(|w| w.group == "shield") {
        return Err("shields go in the left hand".into());
    }
    if l.is_some_and(|w| w.group == "2h") {
        return Err("two-handed weapons go in the right hand".into());
    }
    if r.is_some_and(|w| w.group == "2h") && l.is_some() {
        return Err("left hand must be empty with a two-handed weapon".into());
    }
    match rules.mode {
        MODE_CLASSES => {
            let mut want: Vec<&str> = class.armor.to_vec();
            let mut got: Vec<&str> = k.armor().collect();
            want.sort_unstable();
            got.sort_unstable();
            if want != got || class.weapon_r != k.r() || class.weapon_l != k.l() {
                return Err("classes only: the class kit was edited".into());
            }
        }
        MODE_CUSTOM => {
            let cost = catalog::cost(k);
            if cost > rules.budget as u32 {
                return Err(format!("over budget ({} > {})", cost, rules.budget));
            }
        }
        _ => {}
    }
    Ok(())
}

/// The kit a player gets when their selection is invalid: their class's own
/// kit if it is legal under `rules`, else the default class, else the most
/// expensive class that is legal (peasant fits any budget >= 1).
pub fn fallback_kit(class_id: &str, rules: Rules) -> KitSel {
    if let Some(k) = catalog::class(class_id) {
        let sel = catalog::class_selection(k);
        if check_kit(&sel, rules).is_ok() { return sel; }
    }
    if let Some(k) = catalog::class(catalog::DEFAULT_CLASS) {
        let sel = catalog::class_selection(k);
        if check_kit(&sel, rules).is_ok() { return sel; }
    }
    catalog::CLASSES.iter()
        .map(catalog::class_selection)
        .filter(|s| check_kit(s, rules).is_ok())
        .max_by_key(catalog::cost)
        .unwrap_or_else(|| catalog::class_selection(&catalog::CLASSES[catalog::CLASSES.len() - 1]))
}

pub fn sanitize_cosmetic(c: [u8; 4]) -> [u8; 4] {
    let mut out = [0u8; 4];
    for i in 0..4 {
        out[i] = if c[i] > COSMETIC_MAX[i] { 0 } else { c[i] };
    }
    out
}

/// Validate `req`; returns (kit to broadcast, verdict, reason).
pub fn resolve_kit(req: &KitSel, rules: Rules) -> (KitSel, u8, String) {
    let cosmetic = sanitize_cosmetic(req.cos());
    match check_kit(req, rules) {
        Ok(()) => (req.with_cos(cosmetic), VERDICT_ACCEPTED, String::new()),
        Err(reason) => (fallback_kit(req.class(), rules).with_cos(cosmetic), VERDICT_REPLACED, reason),
    }
}

/// Kit for a peer that never sent a selection (every mode: an MP fighter must
/// never spawn bare-handed in whatever the career save left on the character).
fn default_kit(rules: Rules) -> KitSel {
    fallback_kit(catalog::DEFAULT_CLASS, rules)
}

const DEFAULT_REASON: &str = "no selection received; default class";

struct KitEntry {
    /// What the player asked for (None = default assigned by the server).
    requested: Option<KitSel>,
    ack_seq: u32,
    kit: KitSel,
    verdict: u8,
    reason: String,
    rev: u64,
    window_start: Instant,
    window_count: u32,
}

impl KitEntry {
    fn msg(&self, peer_id: PeerId) -> Vec<u8> {
        self.kit.verdict_msg(peer_id, self.rev, self.ack_seq, self.verdict, &self.reason)
    }
}

struct KitStore {
    rules: Rules,
    rules_rev: u64,
    last_rev: u64,
    kits: HashMap<PeerId, KitEntry>,
    task_started: bool,
}

impl KitStore {
    /// Monotonic, and wall-clock-ms based so it keeps increasing across a
    /// server restart (receivers keep the highest rev they have seen).
    fn next_rev(&mut self) -> u64 {
        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as u64).unwrap_or(0);
        self.last_rev = (self.last_rev + 1).max(now);
        self.last_rev
    }

    /// The `kit_rules` message.
    fn rules_msg(&self) -> Vec<u8> {
        let r = KitRules { rev: self.rules_rev, seq: 0, budget: self.rules.budget, mode: self.rules.mode, _r: 0 };
        wire::encode(0, 0, &r, &[])
    }

    /// Apply a resolved kit to `pid`'s entry; returns true when what everyone
    /// sees changed (and a new rev was assigned).
    fn set_kit(&mut self, pid: PeerId, kit: KitSel, verdict: u8, reason: String) -> bool {
        let changed = match self.kits.get(&pid) {
            Some(e) => e.kit != kit || e.verdict != verdict,
            None => true,
        };
        let rev = if changed { self.next_rev() } else { 0 };
        // The damage / reach plausibility model judges this player's claims and
        // streamed blade by the kit it was actually given.
        // While a round is being fought the facts stay as the
        // player spawned with; a change waits for the next round.
        kit_facts_set(pid, &kit);
        let e = self.kits.entry(pid).or_insert_with(|| KitEntry {
            requested: None, ack_seq: 0, kit, verdict: 0,
            reason: String::new(), rev: 0, window_start: Instant::now(), window_count: 0,
        });
        e.kit = kit;
        e.verdict = verdict;
        e.reason = reason;
        if changed { e.rev = rev; }
        changed
    }

    /// Give every live peer without a kit the default class kit (all modes).
    /// Returns the peers that got one.
    fn assign_defaults(&mut self, live: &[PeerId]) -> Vec<PeerId> {
        let mut fresh = Vec::new();
        for pid in live {
            if self.kits.contains_key(pid) { continue; }
            let k = default_kit(self.rules);
            self.set_kit(*pid, k, VERDICT_DEFAULT, DEFAULT_REASON.into());
            fresh.push(*pid);
        }
        fresh
    }

    /// Re-resolve every stored kit after a rules change. Returns changed peers.
    fn revalidate_all(&mut self) -> Vec<PeerId> {
        let rules = self.rules;
        let mut changed = Vec::new();
        let ids: Vec<PeerId> = self.kits.keys().copied().collect();
        for id in ids {
            match self.kits[&id].requested {
                Some(req) => {
                    let (k, v, r) = resolve_kit(&req, rules);
                    if self.set_kit(id, k, v, r) { changed.push(id); }
                }
                None => {
                    if self.set_kit(id, default_kit(rules), VERDICT_DEFAULT, DEFAULT_REASON.into()) { changed.push(id); }
                }
            }
        }
        changed
    }
}

fn kit_store() -> &'static Mutex<KitStore> {
    static S: OnceLock<Mutex<KitStore>> = OnceLock::new();
    S.get_or_init(|| {
        let rules = Rules::from_env();
        info!(mode = rules.mode, budget = rules.budget, "kit rules (startup)");
        Mutex::new(KitStore { rules, rules_rev: 1, last_rev: 1, kits: HashMap::new(), task_started: false })
    })
}

/// (addr, id, is_admin) of every live peer.
async fn live_peers(state: &Arc<ServerState>) -> Vec<(SocketAddr, PeerId, bool)> {
    let inner = state.lock().lock().await;
    inner.peers.iter().map(|(a, p)| (*a, p.id, p.is_admin)).collect()
}

/// `kit`: validate, store, broadcast on change (or just re-ack a resend).
pub async fn on_kit(socket: &Arc<UdpSocket>, state: &Arc<ServerState>, from: SocketAddr, seq: u32, kit: KitSel) {
    let peers = live_peers(state).await;
    let Some(&(_, pid, _)) = peers.iter().find(|(a, _, _)| *a == from) else { return };
    let addrs: Vec<SocketAddr> = peers.iter().map(|(a, _, _)| *a).collect();
    ensure_kit_task(socket, state).await;
    let (msg, broadcast, log) = {
        let mut st = kit_store().lock().await;
        let rules = st.rules;
        let now = Instant::now();
        let mut duplicate = false;
        if let Some(e) = st.kits.get_mut(&pid) {
            if now.duration_since(e.window_start) >= Duration::from_secs(1) {
                e.window_start = now;
                e.window_count = 0;
            }
            e.window_count += 1;
            if e.window_count > KIT_MSGS_PER_SEC {
                debug!(peer_id = pid, "kit rate-limited");
                return;
            }
            duplicate = e.requested.is_some() && seq <= e.ack_seq;
        }
        if duplicate {
            (st.kits[&pid].msg(pid), false, None)
        } else {
            let (k, verdict, reason) = resolve_kit(&kit, rules);
            let changed = st.set_kit(pid, k, verdict, reason.clone());
            let e = st.kits.get_mut(&pid).expect("kit entry");
            e.requested = Some(kit);
            e.ack_seq = seq;
            (e.msg(pid), changed, Some((k, verdict, reason)))
        }
    };
    if broadcast {
        if let Some((k, verdict, reason)) = log {
            if verdict == VERDICT_ACCEPTED {
                info!(peer_id = pid, seq, class = %k.class(), cost = catalog::cost(&k), "kit accepted");
            } else {
                info!(peer_id = pid, seq, class = %k.class(), %reason, "kit replaced by class default");
            }
        }
        send_all(socket, state, &addrs, &msg).await;
    } else {
        // Unchanged: the sender still needs the ack (ack_seq) to stop resending.
        send_msg(socket, state, from, msg).await;
    }
}

/// `kit_rules_req`: admin only. Re-validates every stored kit under the new rules.
/// Takes effect on each client's next spawn / round reset.
pub async fn on_kit_rules(socket: &Arc<UdpSocket>, state: &Arc<ServerState>, from: SocketAddr, mode: u8, budget: u16) {
    let peers = live_peers(state).await;
    let Some(&(_, pid, is_admin)) = peers.iter().find(|(a, _, _)| *a == from) else { return };
    let addrs: Vec<SocketAddr> = peers.iter().map(|(a, _, _)| *a).collect();
    let live: Vec<PeerId> = peers.iter().map(|(_, id, _)| *id).collect();
    ensure_kit_task(socket, state).await;
    // Kit rules are part of the config frozen for a match (SetConfig refuses
    // them outside the lobby); this path must not bypass that.
    let why = if !is_admin { Some("not admin") } else if !state.in_lobby().await { Some("match running") } else { None };
    if let Some(why) = why {
        let rules_msg = kit_store().lock().await.rules_msg();
        crate::server::chat_to(socket, state, from, &format!("kit rules refused: {}", why)).await;
        send_msg(socket, state, from, rules_msg).await;
        info!(peer_id = pid, why, "kit rules refused");
        return;
    }
    apply_rules(socket, state, &addrs, &live, pid, mode, budget).await;
}

/// Typed command path (`Command::SetConfig{kit_rules}`, RCON `KIT`): the
/// caller already checked admin + lobby (kit rules are frozen per match).
pub async fn set_rules(socket: &Arc<UdpSocket>, state: &Arc<ServerState>, mode: u8, budget: u16) {
    let peers = live_peers(state).await;
    let addrs: Vec<SocketAddr> = peers.iter().map(|(a, _, _)| *a).collect();
    let live: Vec<PeerId> = peers.iter().map(|(_, id, _)| *id).collect();
    ensure_kit_task(socket, state).await;
    apply_rules(socket, state, &addrs, &live, 0, mode, budget).await;
}

/// `S2CSession`: the current rules and every stored kit's revision.
#[cfg_attr(not(test), allow(dead_code))]
pub async fn session_view() -> crate::server::KitView {
    let st = kit_store().lock().await;
    crate::server::KitView {
        mode: st.rules.mode,
        budget: st.rules.budget,
        revs: st.kits.iter().map(|(id, e)| (*id, e.rev)).collect(),
    }
}

/// `session_view` into a view the caller keeps (the tick reuses its map's storage).
pub async fn session_view_into(v: &mut crate::server::KitView) {
    let st = kit_store().lock().await;
    v.mode = st.rules.mode;
    v.budget = st.rules.budget;
    v.revs.clear();
    v.revs.extend(st.kits.iter().map(|(id, e)| (*id, e.rev)));
}

async fn apply_rules(socket: &Arc<UdpSocket>, state: &Arc<ServerState>, addrs: &[SocketAddr], live: &[PeerId],
                     pid: PeerId, mode: u8, budget: u16) {
    let new = Rules::sanitized(mode, budget);
    let out: Vec<Vec<u8>> = {
        let mut st = kit_store().lock().await;
        let mut out = Vec::new();
        if st.rules != new {
            st.rules = new;
            st.rules_rev = st.next_rev();
            info!(peer_id = pid, mode = new.mode, budget = new.budget, "kit rules changed");
            let mut changed = st.revalidate_all();
            changed.extend(st.assign_defaults(live));
            for id in changed {
                if live.contains(&id) { out.push(st.kits[&id].msg(id)); }
            }
        }
        out.insert(0, st.rules_msg());
        out
    };
    for m in &out {
        send_all(socket, state, addrs, m).await;
    }
}

/// Join / reconnect: the kit rules, every live peer's kit and every other live peer's newest
/// loadout to `to`; enforce defaults. Loadouts of peers that have left are never replayed.
pub async fn replay_to(socket: &Arc<UdpSocket>, state: &Arc<ServerState>, to: SocketAddr) {
    ensure_kit_task(socket, state).await;
    let peers = live_peers(state).await;
    let addrs: Vec<SocketAddr> = peers.iter().map(|(a, _, _)| *a).collect();
    let live: Vec<PeerId> = peers.iter().map(|(_, id, _)| *id).collect();
    let (to_joiner, to_all) = {
        let mut st = kit_store().lock().await;
        let fresh = st.assign_defaults(&live);
        let mut to_joiner = vec![st.rules_msg()];
        for id in &live {
            if fresh.contains(id) { continue; }
            if let Some(e) = st.kits.get(id) { to_joiner.push(e.msg(*id)); }
        }
        let to_all: Vec<Vec<u8>> = fresh.iter().map(|id| st.kits[id].msg(*id)).collect();
        (to_joiner, to_all)
    };
    let others: Vec<PeerId> = peers.iter().filter(|(a, _, _)| *a != to).map(|(_, id, _)| *id).collect();
    let loadouts: Vec<Vec<u8>> = {
        let st = store().lock().await;
        others.iter().filter_map(|pid| st.get(pid).filter(|e| !e.msg.is_empty()).map(|e| e.msg.clone())).collect()
    };
    // Passport bodies, only to a joiner that can read them.
    let joiner = peers.iter().find(|(a, _, _)| *a == to).map(|(_, id, _)| *id);
    let bodies: Vec<Vec<u8>> = match joiner {
        Some(j) if crate::interact::peer_caps(j) & hsmp_net::net::caps::BODY != 0 => {
            let st = body_store().lock().await;
            others.iter().filter_map(|pid| st.get(pid).filter(|e| !e.msg.is_empty()).map(|e| e.msg.clone())).collect()
        }
        _ => Vec::new(),
    };
    if to_joiner.len() > 1 || !loadouts.is_empty() || !bodies.is_empty() {
        info!(%to, kits = to_joiner.len() - 1, loadouts = loadouts.len(), bodies = bodies.len(), "replaying kits / loadouts to joiner");
    }
    for m in to_joiner.into_iter().chain(loadouts).chain(bodies) {
        send_msg(socket, state, to, m).await;
    }
    for m in &to_all {
        send_all(socket, state, &addrs, m).await;
    }
}

/// Periodic re-broadcast of rules + live kits (lost-datagram repair) and
/// cleanup of kits of peers that left. Started once, on first use.
async fn ensure_kit_task(socket: &Arc<UdpSocket>, state: &Arc<ServerState>) {
    {
        let mut st = kit_store().lock().await;
        if st.task_started { return; }
        st.task_started = true;
    }
    let socket = socket.clone();
    let state = state.clone();
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(KIT_REBROADCAST);
        tick.tick().await;
        loop {
            tick.tick().await;
            let peers = live_peers(&state).await;
            if peers.is_empty() { continue; }
            let addrs: Vec<SocketAddr> = peers.iter().map(|(a, _, _)| *a).collect();
            let live: Vec<PeerId> = peers.iter().map(|(_, id, _)| *id).collect();
            let msgs: Vec<Vec<u8>> = {
                let mut st = kit_store().lock().await;
                st.kits.retain(|id, _| live.contains(id));
                st.assign_defaults(&live);
                let mut v = vec![st.rules_msg()];
                v.extend(st.kits.iter().map(|(id, e)| e.msg(*id)));
                v
            };
            for m in &msgs {
                send_all(&socket, &state, &addrs, m).await;
            }
        }
    });
}

/// The shared item catalogue. Canonical copy (with class paths and labels):
/// `mods/HSMPLoadout/Scripts/hsmp_catalog.lua`; the test
/// `catalog_matches_lua` fails if the two disagree.
pub mod catalog {
    use hsmp_ipc::layout::Str;
    use hsmp_ipc::schema::loadout::{Kit, KitBuf, KitItem};

    /// A kit selection held by the server: the `kit` record itself (head + armour rows) with the
    /// verdict fields cleared (`rev`, `seq`, `verdict`, `reason` live in the server's `KitEntry`). Two
    /// selections are equal when their record bytes are.
    #[derive(Clone, Copy)]
    pub struct KitSel(pub KitBuf);

    impl KitSel {
        /// Build a selection (ids longer than 32 bytes are truncated; at most 16 armour ids).
        pub fn new(class: &str, r: &str, l: &str, armor: &[&str], cos: [u8; 4]) -> KitSel {
            let mut h = Kit::default();
            h.class = Str::new(class);
            h.r = Str::new(r);
            h.l = Str::new(l);
            h.cos = cos;
            let rows: Vec<KitItem> = armor.iter().map(|a| KitItem { id: Str::new(a) }).collect();
            let mut b = *KitBuf::new_boxed();
            b.set(&h, &rows);
            KitSel(b)
        }

        /// The selection of a validated `kit` record (its verdict fields dropped).
        pub fn from_record(head: &Kit, rows: &[KitItem]) -> KitSel {
            let mut h = *head;
            h.rev = 0;
            h.seq = 0;
            h.verdict = 0;
            h.reason = Str::default();
            let mut b = *KitBuf::new_boxed();
            b.set(&h, rows);
            KitSel(b)
        }

        pub fn class(&self) -> &str {
            self.0.head.class.as_str().unwrap_or("")
        }
        pub fn r(&self) -> &str {
            self.0.head.r.as_str().unwrap_or("")
        }
        pub fn l(&self) -> &str {
            self.0.head.l.as_str().unwrap_or("")
        }
        pub fn cos(&self) -> [u8; 4] {
            self.0.head.cos
        }
        pub fn armor(&self) -> impl Iterator<Item = &str> {
            self.0.used().iter().map(|i| i.id.as_str().unwrap_or(""))
        }
        pub fn armor_len(&self) -> usize {
            self.0.n()
        }
        pub fn with_cos(mut self, cos: [u8; 4]) -> KitSel {
            self.0.head.cos = cos;
            self
        }
    }

    impl PartialEq for KitSel {
        fn eq(&self, o: &KitSel) -> bool {
            self.0.payload() == o.0.payload()
        }
    }

    impl core::fmt::Debug for KitSel {
        fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
            f.debug_struct("KitSel").field("class", &self.class()).field("r", &self.r()).field("l", &self.l())
                .field("armor", &self.armor().collect::<Vec<_>>()).field("cos", &self.cos()).finish()
        }
    }


    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub enum Kind { Armor, Weapon }

    #[derive(Debug)]
    pub struct Item {
        pub id: &'static str,
        pub kind: Kind,
        /// Armour: slot group ("head", ...). Weapon: hand class ("1h", "2h", "dagger", "shield").
        pub group: &'static str,
        pub cost: u16,
    }

    #[derive(Debug)]
    pub struct ClassKit {
        pub id: &'static str,
        pub weapon_r: &'static str,
        pub weapon_l: &'static str,
        pub armor: &'static [&'static str],
    }

    pub const DEFAULT_CLASS: &str = "man_at_arms";

    pub const ITEMS: &[Item] = &[
        Item { id: "h_hat1", kind: Kind::Armor, group: "head", cost: 0 },
        Item { id: "h_hat2", kind: Kind::Armor, group: "head", cost: 0 },
        Item { id: "h_hat3", kind: Kind::Armor, group: "head", cost: 0 },
        Item { id: "h_hat4", kind: Kind::Armor, group: "head", cost: 0 },
        Item { id: "h_cap1", kind: Kind::Armor, group: "head", cost: 1 },
        Item { id: "h_cap2", kind: Kind::Armor, group: "head", cost: 1 },
        Item { id: "h_cap3", kind: Kind::Armor, group: "head", cost: 1 },
        Item { id: "h_cap4", kind: Kind::Armor, group: "head", cost: 1 },
        Item { id: "h_kettle1", kind: Kind::Armor, group: "head", cost: 3 },
        Item { id: "h_kettle2", kind: Kind::Armor, group: "head", cost: 3 },
        Item { id: "h_kettle3", kind: Kind::Armor, group: "head", cost: 3 },
        Item { id: "h_kettle4", kind: Kind::Armor, group: "head", cost: 3 },
        Item { id: "h_kettle5", kind: Kind::Armor, group: "head", cost: 3 },
        Item { id: "h_kettle_c", kind: Kind::Armor, group: "head", cost: 3 },
        Item { id: "h_kettle_f", kind: Kind::Armor, group: "head", cost: 3 },
        Item { id: "h_kettle_g", kind: Kind::Armor, group: "head", cost: 3 },
        Item { id: "h_eisen_aa", kind: Kind::Armor, group: "head", cost: 3 },
        Item { id: "h_eisen_ab", kind: Kind::Armor, group: "head", cost: 3 },
        Item { id: "h_eisen_b", kind: Kind::Armor, group: "head", cost: 3 },
        Item { id: "h_eisen_g", kind: Kind::Armor, group: "head", cost: 3 },
        Item { id: "h_sallet1", kind: Kind::Armor, group: "head", cost: 4 },
        Item { id: "h_sallet2", kind: Kind::Armor, group: "head", cost: 4 },
        Item { id: "h_sallet3", kind: Kind::Armor, group: "head", cost: 4 },
        Item { id: "h_sallet4", kind: Kind::Armor, group: "head", cost: 4 },
        Item { id: "h_sallet_oa1", kind: Kind::Armor, group: "head", cost: 4 },
        Item { id: "h_sallet_oa2", kind: Kind::Armor, group: "head", cost: 4 },
        Item { id: "h_sallet_oa3", kind: Kind::Armor, group: "head", cost: 4 },
        Item { id: "h_sallet_sa1", kind: Kind::Armor, group: "head", cost: 5 },
        Item { id: "h_sallet_sa3", kind: Kind::Armor, group: "head", cost: 5 },
        Item { id: "h_sallet_sa4", kind: Kind::Armor, group: "head", cost: 5 },
        Item { id: "h_sallet_sb3", kind: Kind::Armor, group: "head", cost: 5 },
        Item { id: "h_sallet_sb4", kind: Kind::Armor, group: "head", cost: 5 },
        Item { id: "h_sallet_sc1", kind: Kind::Armor, group: "head", cost: 5 },
        Item { id: "h_sallet_sc2", kind: Kind::Armor, group: "head", cost: 5 },
        Item { id: "h_sallet_va1", kind: Kind::Armor, group: "head", cost: 5 },
        Item { id: "h_sallet_va2", kind: Kind::Armor, group: "head", cost: 5 },
        Item { id: "h_sallet_vc2", kind: Kind::Armor, group: "head", cost: 5 },
        Item { id: "h_sallet_vd2", kind: Kind::Armor, group: "head", cost: 5 },
        Item { id: "h_barbute1", kind: Kind::Armor, group: "head", cost: 5 },
        Item { id: "h_barbute_b", kind: Kind::Armor, group: "head", cost: 5 },
        Item { id: "h_armet", kind: Kind::Armor, group: "head", cost: 6 },
        Item { id: "bv_1", kind: Kind::Armor, group: "bevor", cost: 3 },
        Item { id: "bv_2", kind: Kind::Armor, group: "bevor", cost: 3 },
        Item { id: "bv_15", kind: Kind::Armor, group: "bevor", cost: 3 },
        Item { id: "bv_16", kind: Kind::Armor, group: "bevor", cost: 3 },
        Item { id: "bv_17", kind: Kind::Armor, group: "bevor", cost: 3 },
        Item { id: "bv_18", kind: Kind::Armor, group: "bevor", cost: 3 },
        Item { id: "bv_19", kind: Kind::Armor, group: "bevor", cost: 3 },
        Item { id: "bv_20", kind: Kind::Armor, group: "bevor", cost: 3 },
        Item { id: "bv_21", kind: Kind::Armor, group: "bevor", cost: 3 },
        Item { id: "bv_22", kind: Kind::Armor, group: "bevor", cost: 3 },
        Item { id: "n_standart", kind: Kind::Armor, group: "collar", cost: 2 },
        Item { id: "s_pauldron", kind: Kind::Armor, group: "shoulders", cost: 4 },
        Item { id: "s_spaulder2", kind: Kind::Armor, group: "shoulders", cost: 3 },
        Item { id: "s_spaulder3", kind: Kind::Armor, group: "shoulders", cost: 3 },
        Item { id: "s_spaulder4", kind: Kind::Armor, group: "shoulders", cost: 3 },
        Item { id: "s_spaulder5", kind: Kind::Armor, group: "shoulders", cost: 3 },
        Item { id: "s_spaulder6", kind: Kind::Armor, group: "shoulders", cost: 3 },
        Item { id: "s_spaulder7", kind: Kind::Armor, group: "shoulders", cost: 3 },
        Item { id: "s_spaulder8", kind: Kind::Armor, group: "shoulders", cost: 3 },
        Item { id: "s_spaulder_a", kind: Kind::Armor, group: "shoulders", cost: 3 },
        Item { id: "ar_harness1", kind: Kind::Armor, group: "arms", cost: 1 },
        Item { id: "ar_harness2", kind: Kind::Armor, group: "arms", cost: 1 },
        Item { id: "ar_harness3", kind: Kind::Armor, group: "arms", cost: 1 },
        Item { id: "ar_vambrace1", kind: Kind::Armor, group: "arms", cost: 3 },
        Item { id: "ar_vambrace2", kind: Kind::Armor, group: "arms", cost: 3 },
        Item { id: "ar_vambrace3", kind: Kind::Armor, group: "arms", cost: 3 },
        Item { id: "ar_vambrace4", kind: Kind::Armor, group: "arms", cost: 3 },
        Item { id: "ar_vambrace5", kind: Kind::Armor, group: "arms", cost: 3 },
        Item { id: "ar_vambrace6", kind: Kind::Armor, group: "arms", cost: 3 },
        Item { id: "ar_vambrace7", kind: Kind::Armor, group: "arms", cost: 3 },
        Item { id: "g_gauntlet1", kind: Kind::Armor, group: "hands", cost: 3 },
        Item { id: "g_gauntlet4", kind: Kind::Armor, group: "hands", cost: 3 },
        Item { id: "g_gauntlet10", kind: Kind::Armor, group: "hands", cost: 3 },
        Item { id: "g_half1", kind: Kind::Armor, group: "hands", cost: 2 },
        Item { id: "g_half2", kind: Kind::Armor, group: "hands", cost: 2 },
        Item { id: "b_shirt", kind: Kind::Armor, group: "body", cost: 0 },
        Item { id: "b_tunic", kind: Kind::Armor, group: "body", cost: 0 },
        Item { id: "b_doublet1", kind: Kind::Armor, group: "body", cost: 1 },
        Item { id: "b_doublet2", kind: Kind::Armor, group: "body", cost: 1 },
        Item { id: "b_doublet3", kind: Kind::Armor, group: "body", cost: 1 },
        Item { id: "b_arming", kind: Kind::Armor, group: "body", cost: 1 },
        Item { id: "b_arming2", kind: Kind::Armor, group: "body", cost: 1 },
        Item { id: "b_gambeson", kind: Kind::Armor, group: "body", cost: 2 },
        Item { id: "b_jack", kind: Kind::Armor, group: "body", cost: 2 },
        Item { id: "m_hauberk", kind: Kind::Armor, group: "mail", cost: 4 },
        Item { id: "c_breast1", kind: Kind::Armor, group: "chest", cost: 5 },
        Item { id: "c_breast2", kind: Kind::Armor, group: "chest", cost: 5 },
        Item { id: "c_breast3", kind: Kind::Armor, group: "chest", cost: 5 },
        Item { id: "c_breast4", kind: Kind::Armor, group: "chest", cost: 5 },
        Item { id: "c_breast5", kind: Kind::Armor, group: "chest", cost: 5 },
        Item { id: "c_breast6", kind: Kind::Armor, group: "chest", cost: 5 },
        Item { id: "c_breast10", kind: Kind::Armor, group: "chest", cost: 5 },
        Item { id: "c_breast18", kind: Kind::Armor, group: "chest", cost: 5 },
        Item { id: "c_brust1", kind: Kind::Armor, group: "chest", cost: 5 },
        Item { id: "c_brust2", kind: Kind::Armor, group: "chest", cost: 5 },
        Item { id: "c_cuirass1", kind: Kind::Armor, group: "chest", cost: 6 },
        Item { id: "c_cuirass2", kind: Kind::Armor, group: "chest", cost: 6 },
        Item { id: "c_gothic5", kind: Kind::Armor, group: "chest", cost: 7 },
        Item { id: "c_gothic6", kind: Kind::Armor, group: "chest", cost: 7 },
        Item { id: "t_tabard", kind: Kind::Armor, group: "tabard", cost: 0 },
        Item { id: "wa_faulds", kind: Kind::Armor, group: "waist", cost: 2 },
        Item { id: "l_hosen1", kind: Kind::Armor, group: "legs", cost: 0 },
        Item { id: "l_hosen2", kind: Kind::Armor, group: "legs", cost: 0 },
        Item { id: "l_hosen3", kind: Kind::Armor, group: "legs", cost: 0 },
        Item { id: "l_trousers1", kind: Kind::Armor, group: "legs", cost: 0 },
        Item { id: "l_trousers2", kind: Kind::Armor, group: "legs", cost: 0 },
        Item { id: "th_cuisse", kind: Kind::Armor, group: "thighs", cost: 3 },
        Item { id: "th_cuisse2", kind: Kind::Armor, group: "thighs", cost: 3 },
        Item { id: "th_cuisse3", kind: Kind::Armor, group: "thighs", cost: 3 },
        Item { id: "th_cuisse4", kind: Kind::Armor, group: "thighs", cost: 3 },
        Item { id: "sh_greaves", kind: Kind::Armor, group: "shins", cost: 3 },
        Item { id: "sh_poleyn", kind: Kind::Armor, group: "shins", cost: 2 },
        Item { id: "f_shoes1", kind: Kind::Armor, group: "feet", cost: 0 },
        Item { id: "f_shoes2", kind: Kind::Armor, group: "feet", cost: 0 },
        Item { id: "f_shoes3", kind: Kind::Armor, group: "feet", cost: 0 },
        Item { id: "w_arming1", kind: Kind::Weapon, group: "1h", cost: 3 },
        Item { id: "w_arming2", kind: Kind::Weapon, group: "1h", cost: 4 },
        Item { id: "w_arming3", kind: Kind::Weapon, group: "1h", cost: 5 },
        Item { id: "w_falchion_s1", kind: Kind::Weapon, group: "1h", cost: 3 },
        Item { id: "w_falchion_s2", kind: Kind::Weapon, group: "1h", cost: 4 },
        Item { id: "w_falchion_s3", kind: Kind::Weapon, group: "1h", cost: 5 },
        Item { id: "w_falchion_l1", kind: Kind::Weapon, group: "1h", cost: 4 },
        Item { id: "w_falchion_l2", kind: Kind::Weapon, group: "1h", cost: 5 },
        Item { id: "w_falchion_l3", kind: Kind::Weapon, group: "1h", cost: 6 },
        Item { id: "w_bastard1", kind: Kind::Weapon, group: "1h", cost: 5 },
        Item { id: "w_bastard2", kind: Kind::Weapon, group: "1h", cost: 6 },
        Item { id: "w_bastard3", kind: Kind::Weapon, group: "1h", cost: 7 },
        Item { id: "w_longsword1", kind: Kind::Weapon, group: "2h", cost: 6 },
        Item { id: "w_longsword2", kind: Kind::Weapon, group: "2h", cost: 7 },
        Item { id: "w_longsword3", kind: Kind::Weapon, group: "2h", cost: 8 },
        Item { id: "w_greatsword", kind: Kind::Weapon, group: "2h", cost: 9 },
        Item { id: "w_dagger1", kind: Kind::Weapon, group: "dagger", cost: 1 },
        Item { id: "w_dagger2", kind: Kind::Weapon, group: "dagger", cost: 2 },
        Item { id: "w_dagger3", kind: Kind::Weapon, group: "dagger", cost: 2 },
        Item { id: "w_rondel", kind: Kind::Weapon, group: "dagger", cost: 2 },
        Item { id: "w_axe", kind: Kind::Weapon, group: "1h", cost: 3 },
        Item { id: "w_axe2h", kind: Kind::Weapon, group: "2h", cost: 6 },
        Item { id: "w_waraxe1", kind: Kind::Weapon, group: "2h", cost: 4 },
        Item { id: "w_waraxe2", kind: Kind::Weapon, group: "2h", cost: 4 },
        Item { id: "w_waraxe3", kind: Kind::Weapon, group: "2h", cost: 4 },
        Item { id: "w_waraxe4", kind: Kind::Weapon, group: "2h", cost: 4 },
        Item { id: "w_waraxe5", kind: Kind::Weapon, group: "2h", cost: 4 },
        Item { id: "w_waraxe6", kind: Kind::Weapon, group: "2h", cost: 4 },
        Item { id: "w_hatchet_a1", kind: Kind::Weapon, group: "1h", cost: 2 },
        Item { id: "w_hatchet_a2", kind: Kind::Weapon, group: "1h", cost: 2 },
        Item { id: "w_hatchet_b1", kind: Kind::Weapon, group: "1h", cost: 2 },
        Item { id: "w_hatchet_b2", kind: Kind::Weapon, group: "1h", cost: 2 },
        Item { id: "w_hatchet_c1", kind: Kind::Weapon, group: "1h", cost: 2 },
        Item { id: "w_hatchet_c2", kind: Kind::Weapon, group: "1h", cost: 2 },
        Item { id: "w_hatchet_d1", kind: Kind::Weapon, group: "1h", cost: 2 },
        Item { id: "w_halberd_a", kind: Kind::Weapon, group: "2h", cost: 6 },
        Item { id: "w_halberd_b", kind: Kind::Weapon, group: "2h", cost: 6 },
        Item { id: "w_halberd_c", kind: Kind::Weapon, group: "2h", cost: 6 },
        Item { id: "w_halberd_d", kind: Kind::Weapon, group: "2h", cost: 6 },
        Item { id: "w_billhook_a", kind: Kind::Weapon, group: "2h", cost: 5 },
        Item { id: "w_billhook_b", kind: Kind::Weapon, group: "2h", cost: 5 },
        Item { id: "w_billhook_c", kind: Kind::Weapon, group: "2h", cost: 5 },
        Item { id: "w_billhook_d", kind: Kind::Weapon, group: "2h", cost: 5 },
        Item { id: "w_spear_a", kind: Kind::Weapon, group: "2h", cost: 4 },
        Item { id: "w_spear_b", kind: Kind::Weapon, group: "2h", cost: 4 },
        Item { id: "w_spear_c", kind: Kind::Weapon, group: "2h", cost: 4 },
        Item { id: "w_spear_d", kind: Kind::Weapon, group: "2h", cost: 4 },
        Item { id: "w_staff", kind: Kind::Weapon, group: "2h", cost: 1 },
        Item { id: "w_warstaff_a", kind: Kind::Weapon, group: "2h", cost: 3 },
        Item { id: "w_warstaff_b", kind: Kind::Weapon, group: "2h", cost: 3 },
        Item { id: "w_flail_a", kind: Kind::Weapon, group: "2h", cost: 5 },
        Item { id: "w_flail_b", kind: Kind::Weapon, group: "2h", cost: 5 },
        Item { id: "w_flail_c", kind: Kind::Weapon, group: "2h", cost: 5 },
        Item { id: "w_flail_d", kind: Kind::Weapon, group: "2h", cost: 5 },
        Item { id: "w_messer_a", kind: Kind::Weapon, group: "1h", cost: 2 },
        Item { id: "w_messer_a2", kind: Kind::Weapon, group: "1h", cost: 2 },
        Item { id: "w_messer_b", kind: Kind::Weapon, group: "1h", cost: 2 },
        Item { id: "w_messer_c", kind: Kind::Weapon, group: "1h", cost: 2 },
        Item { id: "w_messer_d", kind: Kind::Weapon, group: "1h", cost: 2 },
        Item { id: "w_messer_e", kind: Kind::Weapon, group: "1h", cost: 2 },
        Item { id: "w_messer_f", kind: Kind::Weapon, group: "1h", cost: 2 },
        Item { id: "w_mace_ls", kind: Kind::Weapon, group: "1h", cost: 3 },
        Item { id: "w_mace_ms", kind: Kind::Weapon, group: "1h", cost: 4 },
        Item { id: "w_mace_hs", kind: Kind::Weapon, group: "1h", cost: 5 },
        Item { id: "w_mace_ma", kind: Kind::Weapon, group: "1h", cost: 4 },
        Item { id: "w_mace_ha", kind: Kind::Weapon, group: "1h", cost: 5 },
        Item { id: "w_mace_ll", kind: Kind::Weapon, group: "2h", cost: 4 },
        Item { id: "w_mace_ml", kind: Kind::Weapon, group: "2h", cost: 5 },
        Item { id: "w_mace_hl", kind: Kind::Weapon, group: "2h", cost: 6 },
        Item { id: "w_hammer_ls", kind: Kind::Weapon, group: "1h", cost: 3 },
        Item { id: "w_hammer_ms", kind: Kind::Weapon, group: "1h", cost: 4 },
        Item { id: "w_hammer_hs", kind: Kind::Weapon, group: "1h", cost: 5 },
        Item { id: "w_hammer_ma", kind: Kind::Weapon, group: "1h", cost: 4 },
        Item { id: "w_hammer_ha", kind: Kind::Weapon, group: "1h", cost: 5 },
        Item { id: "w_hammer_ll", kind: Kind::Weapon, group: "2h", cost: 4 },
        Item { id: "w_hammer_ml", kind: Kind::Weapon, group: "2h", cost: 5 },
        Item { id: "w_hammer_hl", kind: Kind::Weapon, group: "2h", cost: 6 },
        Item { id: "w_poleaxe_l", kind: Kind::Weapon, group: "2h", cost: 5 },
        Item { id: "w_poleaxe_m", kind: Kind::Weapon, group: "2h", cost: 6 },
        Item { id: "w_poleaxe_h", kind: Kind::Weapon, group: "2h", cost: 7 },
        Item { id: "t_axe_a", kind: Kind::Weapon, group: "2h", cost: 1 },
        Item { id: "t_axe_b", kind: Kind::Weapon, group: "2h", cost: 1 },
        Item { id: "t_axe_c", kind: Kind::Weapon, group: "1h", cost: 1 },
        Item { id: "t_axe_d", kind: Kind::Weapon, group: "1h", cost: 1 },
        Item { id: "t_hammer_a", kind: Kind::Weapon, group: "1h", cost: 1 },
        Item { id: "t_hammer_b", kind: Kind::Weapon, group: "1h", cost: 1 },
        Item { id: "t_hammer_c", kind: Kind::Weapon, group: "1h", cost: 1 },
        Item { id: "t_mallet_a", kind: Kind::Weapon, group: "1h", cost: 1 },
        Item { id: "t_mallet_b", kind: Kind::Weapon, group: "1h", cost: 1 },
        Item { id: "t_mallet_c", kind: Kind::Weapon, group: "1h", cost: 1 },
        Item { id: "t_knife_a", kind: Kind::Weapon, group: "dagger", cost: 0 },
        Item { id: "t_knife_b", kind: Kind::Weapon, group: "dagger", cost: 0 },
        Item { id: "t_knife_c", kind: Kind::Weapon, group: "dagger", cost: 0 },
        Item { id: "t_chisel_b", kind: Kind::Weapon, group: "dagger", cost: 0 },
        Item { id: "t_chisel_c", kind: Kind::Weapon, group: "dagger", cost: 0 },
        Item { id: "t_scissors", kind: Kind::Weapon, group: "dagger", cost: 0 },
        Item { id: "t_tongs", kind: Kind::Weapon, group: "1h", cost: 0 },
        Item { id: "t_sickle_a", kind: Kind::Weapon, group: "1h", cost: 1 },
        Item { id: "t_sickle_b", kind: Kind::Weapon, group: "1h", cost: 1 },
        Item { id: "t_sickle_c", kind: Kind::Weapon, group: "1h", cost: 1 },
        Item { id: "t_sickle_d", kind: Kind::Weapon, group: "1h", cost: 1 },
        Item { id: "t_sickle_e", kind: Kind::Weapon, group: "2h", cost: 1 },
        Item { id: "t_hoe_a", kind: Kind::Weapon, group: "2h", cost: 1 },
        Item { id: "t_hoe_b", kind: Kind::Weapon, group: "1h", cost: 1 },
        Item { id: "t_pitchfork_a", kind: Kind::Weapon, group: "2h", cost: 1 },
        Item { id: "t_pitchfork_b", kind: Kind::Weapon, group: "2h", cost: 1 },
        Item { id: "t_scythe", kind: Kind::Weapon, group: "2h", cost: 2 },
        Item { id: "t_shovel_a", kind: Kind::Weapon, group: "2h", cost: 1 },
        Item { id: "t_shovel_b", kind: Kind::Weapon, group: "2h", cost: 1 },
        Item { id: "t_pickaxe_a", kind: Kind::Weapon, group: "2h", cost: 1 },
        Item { id: "t_pickaxe_b", kind: Kind::Weapon, group: "2h", cost: 1 },
        Item { id: "t_maul", kind: Kind::Weapon, group: "2h", cost: 2 },
        Item { id: "t_rake", kind: Kind::Weapon, group: "2h", cost: 0 },
        Item { id: "t_flail", kind: Kind::Weapon, group: "2h", cost: 1 },
        Item { id: "t_lid", kind: Kind::Weapon, group: "shield", cost: 0 },
        Item { id: "i_lid", kind: Kind::Weapon, group: "shield", cost: 0 },
        Item { id: "i_stool", kind: Kind::Weapon, group: "2h", cost: 0 },
        Item { id: "i_candle_big", kind: Kind::Weapon, group: "2h", cost: 0 },
        Item { id: "i_candle_small", kind: Kind::Weapon, group: "1h", cost: 0 },
        Item { id: "i_lantern", kind: Kind::Weapon, group: "1h", cost: 0 },
        Item { id: "s_buckler", kind: Kind::Weapon, group: "shield", cost: 2 },
        Item { id: "s_buckler2", kind: Kind::Weapon, group: "shield", cost: 2 },
        Item { id: "s_buckler3", kind: Kind::Weapon, group: "shield", cost: 3 },
        Item { id: "s_bossgrip", kind: Kind::Weapon, group: "shield", cost: 3 },
        Item { id: "s_targe", kind: Kind::Weapon, group: "shield", cost: 4 },
        Item { id: "s_pavise_l", kind: Kind::Weapon, group: "shield", cost: 4 },
        Item { id: "s_pavise_h", kind: Kind::Weapon, group: "shield", cost: 5 },
        Item { id: "s_pavise_t", kind: Kind::Weapon, group: "shield", cost: 6 },
    ];
    pub const CLASSES: &[ClassKit] = &[
        ClassKit { id: "knight", weapon_r: "w_longsword3", weapon_l: "", armor: &["h_armet", "bv_1", "s_pauldron", "ar_vambrace1", "g_gauntlet1", "b_arming2", "c_cuirass1", "wa_faulds", "l_hosen2", "th_cuisse", "sh_greaves", "f_shoes2"] },
        ClassKit { id: "man_at_arms", weapon_r: "w_poleaxe_m", weapon_l: "", armor: &["h_sallet_oa1", "n_standart", "b_gambeson", "m_hauberk", "t_tabard", "g_half1", "l_hosen2", "f_shoes2"] },
        ClassKit { id: "duelist", weapon_r: "w_arming3", weapon_l: "s_buckler3", armor: &["h_hat2", "b_arming", "g_half2", "l_hosen1", "f_shoes1"] },
        ClassKit { id: "brute", weapon_r: "w_axe2h", weapon_l: "", armor: &["h_cap1", "b_gambeson", "g_half1", "l_trousers1", "f_shoes3"] },
        ClassKit { id: "peasant", weapon_r: "t_pitchfork_b", weapon_l: "", armor: &["b_tunic", "l_hosen3"] },
    ];

    pub fn item(id: &str) -> Option<&'static Item> {
        ITEMS.iter().find(|i| i.id == id)
    }

    pub fn class(id: &str) -> Option<&'static ClassKit> {
        CLASSES.iter().find(|c| c.id == id)
    }

    pub fn class_selection(k: &ClassKit) -> KitSel {
        KitSel::new(k.id, k.weapon_r, k.weapon_l, k.armor, [0; 4])
    }

    /// Sum of item costs; unknown ids cost 0 (they are rejected elsewhere).
    pub fn cost(sel: &KitSel) -> u32 {
        let c = |id: &str| item(id).map_or(0, |i| i.cost as u32);
        c(sel.r()) + c(sel.l()) + sel.armor().map(c).sum::<u32>()
    }
}

#[cfg(test)]
mod kit_tests {
    use super::*;
    use hsmp_ipc::schema::loadout::K_KIT_RULES;

    fn rules(mode: u8, budget: u16) -> Rules { Rules::sanitized(mode, budget) }

    fn class_sel(id: &str) -> KitSel { catalog::class_selection(catalog::class(id).unwrap()) }

    fn sel(class: &str, r: &str, l: &str, armor: &[&str]) -> KitSel { KitSel::new(class, r, l, armor, [0; 4]) }

    /// `k` with its hands / armour edited.
    fn edit(k: &KitSel, r: Option<&str>, l: Option<&str>, add: &[&str]) -> KitSel {
        let mut armor: Vec<&str> = k.armor().collect();
        armor.extend_from_slice(add);
        KitSel::new(k.class(), r.unwrap_or(k.r()), l.unwrap_or(k.l()), &armor, k.cos())
    }

    fn store(mode: u8, budget: u16) -> KitStore {
        KitStore { rules: rules(mode, budget), rules_rev: 1, last_rev: 1, kits: HashMap::new(), task_started: true }
    }

    /// Exploit regression: a kit with a poleaxe during a fought
    /// round does not change the damage model's view until the round ends.
    #[test]
    fn kit_facts_are_frozen_while_a_round_is_fought() {
        use crate::validate::damage::{self, WeaponClass};
        let mut st = store(MODE_FREE, 30);
        let pid = 0x4E21;
        damage::forget(pid);
        st.set_kit(pid, sel("custom", "w_arming1", "", &[]), 0, String::new());
        let spawned = damage::weapon_class(pid);
        assert_ne!(spawned, WeaponClass::Polearm);
        set_round_lock(&[pid]);
        let pole = sel("custom", "w_poleaxe_m", "", &[]);
        st.set_kit(pid, pole, 0, String::new());
        assert_eq!(damage::weapon_class(pid), spawned, "facts frozen mid-round");
        assert_eq!(kit_pending(pid), Some(pole));
        assert_eq!(st.kits[&pid].kit.r(), "w_poleaxe_m", "the request itself is kept (dressed next spawn)");
        set_round_lock(&[]);
        assert_eq!(damage::weapon_class(pid), WeaponClass::Polearm, "applied when the round ends");
        assert_eq!(kit_pending(pid), None);
        damage::forget(pid);
    }

    /// The kit a player is given (or defaulted to) is the one the
    /// damage / reach model judges it by.
    #[test]
    fn kits_feed_the_damage_model() {
        use crate::validate::damage::{self, WeaponClass};
        let mut st = store(MODE_FREE, 30);
        let pid = 0x4E12;
        damage::forget(pid);
        st.set_kit(pid, sel("custom", "w_poleaxe_m", "", &[]), 0, String::new());
        assert_eq!(damage::weapon_class(pid), WeaponClass::Polearm);
        let q = 0x4E13;
        damage::forget(q);
        st.assign_defaults(&[q]);
        let wr = st.kits[&q].kit.r().to_string();
        assert_ne!(damage::weapon_class(q), WeaponClass::Unknown, "default kit {wr}");
        damage::forget(pid);
        damage::forget(q);
    }

    #[test]
    fn every_class_is_valid_in_classes_and_free_mode() {
        for c in catalog::CLASSES {
            let s = catalog::class_selection(c);
            assert_eq!(check_kit(&s, rules(MODE_CLASSES, 30)), Ok(()), "{}", c.id);
            assert_eq!(check_kit(&s, rules(MODE_FREE, 30)), Ok(()), "{}", c.id);
        }
    }

    #[test]
    fn class_costs_are_balanced() {
        let cost = |id| catalog::cost(&class_sel(id));
        assert!(cost("knight") > cost("man_at_arms"));
        assert!(cost("man_at_arms") > cost("duelist"));
        assert!(cost("duelist") >= cost("brute"));
        assert!(cost("brute") > cost("peasant"));
        assert_eq!(cost("knight"), 42);
        assert_eq!(cost("man_at_arms"), 20);
        assert_eq!(cost("peasant"), 1);
    }

    #[test]
    fn classes_mode_rejects_edited_kit_and_falls_back_to_class_default() {
        let s = edit(&class_sel("duelist"), None, None, &["c_gothic5"]);
        let (k, verdict, reason) = resolve_kit(&s, rules(MODE_CLASSES, 30));
        assert_eq!(verdict, VERDICT_REPLACED);
        assert!(reason.contains("classes only"), "{reason}");
        assert_eq!(k, class_sel("duelist"));
        let s = edit(&class_sel("knight"), Some("w_greatsword"), None, &[]);
        assert!(check_kit(&s, rules(MODE_CLASSES, 30)).is_err());
    }

    #[test]
    fn classes_mode_ignores_armor_order() {
        let k = class_sel("knight");
        let mut armor: Vec<&str> = k.armor().collect();
        armor.reverse();
        let s = KitSel::new(k.class(), k.r(), k.l(), &armor, [0; 4]);
        assert_eq!(check_kit(&s, rules(MODE_CLASSES, 30)), Ok(()));
    }

    #[test]
    fn custom_mode_enforces_budget() {
        let s = class_sel("duelist"); // 11 points
        assert_eq!(check_kit(&s, rules(MODE_CUSTOM, 11)), Ok(()));
        assert!(check_kit(&s, rules(MODE_CUSTOM, 10)).unwrap_err().contains("over budget"));
        let s = edit(&s, None, None, &["c_gothic5"]); // +7 = 18
        assert_eq!(catalog::cost(&s), 18);
        assert_eq!(check_kit(&s, rules(MODE_CUSTOM, 20)), Ok(()));
        let (k, v, _) = resolve_kit(&s, rules(MODE_CUSTOM, 12));
        assert_eq!(v, VERDICT_REPLACED);
        assert_eq!(k, class_sel("duelist")); // the duelist kit itself fits 12
    }

    #[test]
    fn over_budget_class_falls_back_to_best_fitting_class() {
        assert_eq!(fallback_kit("knight", rules(MODE_CUSTOM, 30)).class(), "man_at_arms");
        let k = fallback_kit("knight", rules(MODE_CUSTOM, 12));
        assert!(catalog::cost(&k) <= 12);
        assert!(k.class() == "duelist" || k.class() == "brute", "{}", k.class());
        assert_eq!(fallback_kit("knight", rules(MODE_CUSTOM, 1)).class(), "peasant");
    }

    #[test]
    fn unknown_items_and_classes_are_rejected() {
        let s = edit(&class_sel("brute"), None, None, &["h_crown_of_god"]);
        assert!(check_kit(&s, rules(MODE_FREE, 30)).unwrap_err().contains("unknown armour"));
        let s = edit(&class_sel("brute"), Some("w_lightsaber"), None, &[]);
        assert!(check_kit(&s, rules(MODE_FREE, 30)).unwrap_err().contains("unknown weapon"));
        let b = class_sel("brute");
        let armor: Vec<&str> = b.armor().collect();
        let s = KitSel::new("wizard", b.r(), b.l(), &armor, [0; 4]);
        let (k, v, _) = resolve_kit(&s, rules(MODE_FREE, 30));
        assert_eq!(v, VERDICT_REPLACED);
        assert_eq!(k.class(), catalog::DEFAULT_CLASS);
        // A weapon id in an armour slot is unknown armour.
        let s = edit(&class_sel("brute"), None, None, &["w_axe"]);
        assert!(check_kit(&s, rules(MODE_FREE, 30)).is_err());
    }

    #[test]
    fn slot_and_hand_rules() {
        let s = edit(&class_sel("peasant"), None, None, &["h_hat1", "h_armet"]);
        assert!(check_kit(&s, rules(MODE_FREE, 30)).unwrap_err().contains("two items"));
        let p = class_sel("peasant");
        let s = edit(&p, Some("w_longsword1"), Some("s_buckler"), &[]);
        assert!(check_kit(&s, rules(MODE_FREE, 30)).unwrap_err().contains("left hand must be empty"));
        let s = edit(&p, Some("s_buckler"), Some(""), &[]);
        assert!(check_kit(&s, rules(MODE_FREE, 30)).unwrap_err().contains("left hand"));
        let s = edit(&p, Some("w_dagger1"), Some("w_greatsword"), &[]);
        assert!(check_kit(&s, rules(MODE_FREE, 30)).unwrap_err().contains("right hand"));
        let s = edit(&p, Some("w_dagger1"), Some("w_rondel"), &[]); // dual daggers are fine
        assert_eq!(check_kit(&s, rules(MODE_FREE, 30)), Ok(()));
    }

    #[test]
    fn game_default_only_in_free_mode() {
        let none = sel("none", "", "", &[]);
        assert_eq!(check_kit(&none, rules(MODE_FREE, 30)), Ok(()));
        let (k, v, _) = resolve_kit(&none, rules(MODE_CLASSES, 30));
        assert_eq!(v, VERDICT_REPLACED);
        assert_eq!(k.class(), catalog::DEFAULT_CLASS);
        let cheat = sel("none", "", "", &["c_gothic5"]);
        assert!(check_kit(&cheat, rules(MODE_FREE, 30)).is_err());
    }

    /// Oversized fields cannot be represented (`Str<32>`, 16 rows): the record refuses them
    /// before any of this runs; cosmetics out of range are zeroed, not refused.
    #[test]
    fn record_bounds_and_cosmetics() {
        let long = "x".repeat(5000);
        let k = sel(&long, "", "", &[]);
        assert_eq!(k.class().len(), 32);
        let many: Vec<&str> = (0..40).map(|_| "b_tunic").collect();
        assert_eq!(sel("peasant", "", "", &many).armor_len(), 16);
        let mut h = Kit::default();
        h.n = 17;
        let mut p = hsmp_ipc::bytemuck::bytes_of(&h).to_vec();
        p.extend(std::iter::repeat_n(0u8, 17 * 32));
        assert!(view::<Kit>(&p).is_err(), "17 armour rows refused");
        let p = class_sel("peasant");
        let armor: Vec<&str> = p.armor().collect();
        let s = KitSel::new("peasant", p.r(), p.l(), &armor, [3, 200, 7, 9]);
        let (k, v, _) = resolve_kit(&s, rules(MODE_CLASSES, 30));
        assert_eq!(v, VERDICT_ACCEPTED);
        assert_eq!(k.cos(), [3, 0, 7, 0]);
    }

    #[test]
    fn rules_are_sanitized() {
        assert_eq!(Rules::sanitized(9, 0), Rules { mode: MODE_FREE, budget: MIN_BUDGET });
        assert_eq!(Rules::sanitized(MODE_CUSTOM, 60000).budget, MAX_BUDGET);
    }

    #[test]
    fn store_defaults_revalidation_and_revs() {
        let mut st = store(MODE_FREE, 30);
        assert_eq!(st.assign_defaults(&[3]), vec![3], "free mode still arms unchosen peers");
        assert_eq!(st.kits[&3].kit.class(), "man_at_arms");
        st.kits.remove(&3);
        let (k, v, r) = resolve_kit(&class_sel("knight"), st.rules);
        assert!(st.set_kit(1, k, v, r));
        st.kits.get_mut(&1).unwrap().requested = Some(class_sel("knight"));
        let rev1 = st.kits[&1].rev;
        let (k, v, r) = resolve_kit(&class_sel("knight"), st.rules);
        assert!(!st.set_kit(1, k, v, r));
        assert_eq!(st.kits[&1].rev, rev1);
        st.rules = rules(MODE_CUSTOM, 30);
        let changed = st.revalidate_all();
        assert_eq!(changed, vec![1]);
        assert_eq!(st.kits[&1].kit.class(), "man_at_arms");
        assert_eq!(st.kits[&1].verdict, VERDICT_REPLACED);
        assert!(st.kits[&1].rev > rev1);
        assert_eq!(st.assign_defaults(&[1, 2]), vec![2]);
        assert_eq!(st.kits[&2].verdict, VERDICT_DEFAULT);
        st.rules = rules(MODE_FREE, 30);
        st.revalidate_all();
        assert_eq!(st.kits[&1].kit.class(), "knight");
        assert_eq!(st.kits[&1].verdict, VERDICT_ACCEPTED);
        assert_eq!(st.kits[&2].kit.class(), "man_at_arms");
    }

    /// The verdict a peer gets is the `kit` record itself (alias kind `kit_verdict`), with
    /// rev / ack / verdict / reason filled and `peer` = the owner; it validates in place.
    #[test]
    fn verdict_and_rules_messages_are_records() {
        let mut st = store(MODE_CLASSES, 30);
        let req = edit(&class_sel("duelist"), None, None, &["c_gothic5"]);
        let (k, v, r) = resolve_kit(&req, st.rules);
        st.set_kit(7, k, v, r);
        st.kits.get_mut(&7).unwrap().ack_seq = 41;
        let m = st.kits[&7].msg(7);
        let (h, p) = wire::split(&m).unwrap();
        assert_eq!((h.kind, h.peer), (K_KIT_VERDICT, 7));
        hsmp_ipc::schema::check_payload(h.kind, p).unwrap();
        let vw = view::<Kit>(p).unwrap();
        assert_eq!(vw.head.seq, 41);
        assert_eq!(vw.head.verdict, VERDICT_REPLACED);
        assert!(vw.head.reason.lossy().contains("classes only"));
        assert_eq!(vw.head.class, "duelist");
        assert_eq!(vw.rows.len(), class_sel("duelist").armor_len());
        assert!(m.len() < 1000, "one datagram ({} B)", m.len());
        let rm = st.rules_msg();
        let (h, p) = wire::split(&rm).unwrap();
        assert_eq!(h.kind, K_KIT_RULES);
        assert_eq!(view::<KitRules>(p).unwrap().head.mode, MODE_CLASSES);
        // The selection the server stores never keeps the client's verdict fields.
        let mut head = req.0.head;
        head.rev = 99;
        head.verdict = 2;
        head.reason = Str::new("spoof");
        assert_eq!(KitSel::from_record(&head, req.0.used()), req);
    }

    #[test]
    fn loadout_versions_newest_wins_and_duplicates_drop() {
        let mut st = HashMap::new();
        let t0 = Instant::now();
        let a = vec![1u8; 40];
        let b = vec![2u8; 40];
        assert_eq!(store_loadout(&mut st, 5, 10, &a, t0), LoadoutStep::Relay);
        let (h, p) = wire::split(&st[&5].msg).unwrap();
        assert_eq!((h.kind, h.peer, p), (K_LOADOUT, 5, &a[..]));
        assert_eq!(store_loadout(&mut st, 5, 10, &a, t0), LoadoutStep::Stale, "same bytes again");
        assert_eq!(store_loadout(&mut st, 5, 9, &b, t0), LoadoutStep::Stale, "older version");
        assert_eq!(store_loadout(&mut st, 5, 10, &b, t0), LoadoutStep::Relay, "same version, new content (owner restart)");
        assert_eq!(store_loadout(&mut st, 5, 11, &a, t0), LoadoutStep::Relay);
        for v in 12..20 {
            let _ = store_loadout(&mut st, 5, v, &a, t0);
        }
        assert_eq!(store_loadout(&mut st, 5, 30, &a, t0), LoadoutStep::RateLimited);
        assert_eq!(store_loadout(&mut st, 5, 31, &a, t0 + Duration::from_secs(1)), LoadoutStep::Relay);
    }

    #[test]
    fn catalog_ids_unique_and_classes_reference_real_items() {
        let mut ids: Vec<&str> = catalog::ITEMS.iter().map(|i| i.id).collect();
        ids.sort_unstable();
        let n = ids.len();
        ids.dedup();
        assert_eq!(n, ids.len(), "duplicate item ids");
        for c in catalog::CLASSES {
            for a in c.armor { assert!(catalog::item(a).is_some(), "{} {}", c.id, a); }
        }
        // Every id fits the record's Str<32>.
        assert!(catalog::ITEMS.iter().all(|i| i.id.len() <= 32));
        assert!(catalog::CLASSES.iter().all(|c| c.id.len() <= 32 && c.armor.len() <= 16));
    }

    /// The Lua catalogue (what the UI offers and the applier resolves) must
    /// list exactly the ids/groups/costs/kits the server validates against.
    #[test]
    fn catalog_matches_lua() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../mods/HSMPLoadout/Scripts/hsmp_catalog.lua");
        let Ok(src) = std::fs::read_to_string(&path) else {
            eprintln!("skipping: {} not found", path.display());
            return;
        };
        let quoted = |line: &str| -> Vec<String> {
            line.split('"').skip(1).step_by(2).map(str::to_string).collect()
        };
        let mut lua_items = Vec::new();
        let mut lua_classes = Vec::new();
        for line in src.lines() {
            let t = line.trim_start();
            if t.starts_with("A(\"") || t.starts_with("W(\"") {
                let q = quoted(t);
                let cost: u16 = t.split(',').nth(2).unwrap().trim().parse().unwrap();
                lua_items.push((q[0].clone(), t.starts_with('A'), q[1].clone(), cost));
            } else if t.starts_with("K(\"") {
                let q = quoted(t);
                let inner = &t[t.find('{').unwrap() + 1..t.find('}').unwrap()];
                lua_classes.push((q[0].clone(), q[2].clone(), q[3].clone(), quoted(inner)));
            }
        }
        let rs_items: Vec<(String, bool, String, u16)> = catalog::ITEMS.iter()
            .map(|i| (i.id.to_string(), i.kind == catalog::Kind::Armor, i.group.to_string(), i.cost))
            .collect();
        assert_eq!(lua_items, rs_items, "hsmp_catalog.lua items differ from loadout.rs catalog::ITEMS");
        let rs_classes: Vec<(String, String, String, Vec<String>)> = catalog::CLASSES.iter()
            .map(|c| (c.id.into(), c.weapon_r.into(), c.weapon_l.into(),
                      c.armor.iter().map(|s| s.to_string()).collect()))
            .collect();
        assert_eq!(lua_classes, rs_classes, "hsmp_catalog.lua classes differ from loadout.rs");
        assert!(src.contains(&format!("C.default_class = \"{}\"", catalog::DEFAULT_CLASS)));
        assert!(src.contains(&format!("C.default_budget = {}", DEFAULT_BUDGET)));
    }
}

#[cfg(test)]
mod record_tests {
    use super::*;
    use hsmp_net::net::SendMode;
    use hsmp_ipc::schema::loadout::K_KIT_RULES;
    use hsmp_net::proto_v5::keys::{self, key};
    use rand::{Rng, SeedableRng};

    /// The schema's channels are the v5 stream keys the transport always used.
    #[test]
    fn record_channels_match_the_stream_keys() {
        use crate::proto::record_mode;
        assert_eq!(record_mode(K_KIT_VERDICT, 7), Some(SendMode::ReliableLatest { key: key(keys::KIT, 7) }));
        assert_eq!(record_mode(K_KIT, 0), Some(SendMode::ReliableLatest { key: key(keys::KIT, 0) }));
        assert_eq!(record_mode(K_KIT_RULES, 0), Some(SendMode::ReliableLatest { key: keys::KIT_RULES }));
        assert_eq!(record_mode(K_KIT_RULES_REQ, 0), Some(SendMode::Ordered));
        assert_eq!(record_mode(K_LOADOUT, 9), Some(SendMode::ReliableLatest { key: key(keys::LOADOUT, 9) }));
        assert_eq!(record_mode(hsmp_ipc::schema::loadout::K_KIT_STATUS, 0), None, "a bus record never travels");
    }


    /// `body` travels on its own per-owner stream, only to peers that negotiated
    /// caps::BODY (a beta.4 sidecar never gets a record kind it does not know), and its
    /// store keeps the newest version per owner as the loadout store does.
    #[test]
    fn body_records_reach_only_capable_peers_newest_first() {
        use crate::proto::record_mode;
        use hsmp_net::net::caps;
        let k = record_mode(K_BODY, 9);
        assert_eq!(k, Some(SendMode::ReliableLatest { key: key(keys::BODY, 9) }));
        for other in [K_LOADOUT, K_KIT, hsmp_ipc::schema::interact::K_INTERACT_GRAB_R, hsmp_ipc::schema::interact::K_INTERACT_GRAB_L] {
            assert_ne!(record_mode(other, 9), k, "kind {other:#x} shares the body stream");
        }
        assert_eq!(keys::BODY, hsmp_ipc::schema::loadout::hsmp_net_keys::BODY);
        let a = |n: u16| -> SocketAddr { format!("127.0.0.1:{n}").parse().unwrap() };
        let peers = vec![(a(1), 3), (a(2), 4), (a(3), 5), (a(4), 6)];
        let capsf = |id: PeerId| if id == 6 { caps::HIT_FX } else { caps::BODY | caps::HIT_FX };
        assert_eq!(body_pick(peers.clone().into_iter(), 4, capsf), vec![a(1), a(3)], "never the owner, never a beta.4 peer");
        let mut st = HashMap::new();
        let t0 = Instant::now();
        let p = vec![7u8; 80];
        assert_eq!(store_versioned(&mut st, K_BODY, 4, 3, &p, t0), LoadoutStep::Relay);
        let (h, q) = wire::split(&st[&4].msg).unwrap();
        assert_eq!((h.kind, h.peer, q), (K_BODY, 4, &p[..]));
        assert_eq!(store_versioned(&mut st, K_BODY, 4, 2, &[1u8; 80], t0), LoadoutStep::Stale);
    }
    /// Mutated kit records that pass the record check go through the whole validator
    /// (catalogue, rules, fallback) without a panic, and always resolve to a legal kit.
    #[test]
    fn hostile_kits_resolve_to_legal_kits() {
        let mut rng = rand::rngs::StdRng::seed_from_u64(0x0510);
        let base = catalog::class_selection(catalog::class("knight").unwrap());
        let good = to_payload(&base.0.head, base.0.used());
        let mut accepted = 0;
        for i in 0..20_000 {
            let mut d = good.clone();
            for _ in 0..rng.gen_range(1..6) {
                let j = rng.gen_range(0..d.len());
                d[j] = if i % 3 == 0 { rng.gen() } else { [0u8, b'a', b'_', 0xff, 16][rng.gen_range(0..5)] };
            }
            let Ok(v) = view::<Kit>(&d) else { continue };
            accepted += 1;
            let sel = KitSel::from_record(&v.head, &v.rows);
            for mode in [MODE_FREE, MODE_CLASSES, MODE_CUSTOM] {
                let r = Rules::sanitized(mode, 30);
                let (k, verdict, _) = resolve_kit(&sel, r);
                if verdict != VERDICT_ACCEPTED {
                    assert_eq!(check_kit(&k, r), Ok(()), "fallback {k:?} illegal under {r:?}");
                }
                let m = k.verdict_msg(3, 1, v.head.seq, verdict, "x");
                let (h, p) = wire::split(&m).unwrap();
                hsmp_ipc::schema::check_payload(h.kind, p).unwrap();
            }
        }
        assert!(accepted > 100, "the mutations reach the validator ({accepted})");
    }
}

/// Bytes per loadout version, today's chunked JSON vs the record (report numbers:
/// `cargo test -p hsmp-server --bin hsmp-server loadout_bytes -- --nocapture`).
#[cfg(test)]
mod measure {
    use hsmp_ipc::layout::{Bool, Str};
    use hsmp_ipc::schema::loadout::*;

    /// A knight's worn appearance: 12 pieces, each with its passport, a longsword passport.
    fn knight() -> (Vec<(u8, String)>, String) {
        let pieces: Vec<(u8, String)> = (0..12u8)
            .map(|i| (i + 1, format!("@Armor/Blueprints/Modular_Armor/BP_Armor_Modular_Core_Piece_{i:02}_Visor_A_001")))
            .collect();
        let weapon = "@Weapons/Blueprints/Built_Weapons/Swords/BP_Longsword_Tier3".to_string();
        (pieces, weapon)
    }

    #[test]
    fn loadout_bytes() {
        let (pieces, weapon) = knight();
        let col = |a: f32| serde_json::json!([a, 0.37, 0.29, 1]);
        // Today: HSMPLoadout jenc({p, a, w}) with {"v":N, ...} (sorted keys, arrays).
        let pass = |slot: u8, path: &str| serde_json::json!([path, 1234, 0, 2, 3, 0, col(0.0), col(0.22), col(0.42), col(0.3),
            col(0.2), 11, 2, 0, 1, 12.5, slot, 1, 0, 0, 0, 0, 3]);
        let mod_path = |m: &str| format!("@Weapons/Blueprints/Weapon_Modules/{m}/BP_{m}_Longsword_03");
        let wpass = serde_json::json!([weapon, 77, "Longsword_03", mod_path("HeadSub"), "", mod_path("Head"), mod_path("Guard"),
            mod_path("Pommel"), mod_path("Grip"), [1, 1, 1.05], [1, 1, 1], [1, 1.1, 1], [1, 1, 1], 1, 1, 1, 1, 3, 0, 2, 1,
            col(0.3), col(0.2), 45, 3]);
        let json = serde_json::json!({
            "v": 25_000_000u64,
            "p": pieces.iter().map(|(s, p)| serde_json::json!([s, p])).collect::<Vec<_>>(),
            "a": pieces.iter().map(|(s, p)| serde_json::json!([s, pass(*s, p)])).collect::<Vec<_>>(),
            "w": { "R": wpass },
        })
        .to_string();
        let chunks = json.len().div_ceil(900);
        // C2SLoadout{version, chunk, total, data}: bincode tag 4 + 4 + 1 + 1 + len 8, + v6 header 8.
        let old_wire = json.len() + chunks * (4 + 4 + 1 + 1 + 8 + 8);
        // The record.
        let mut h = LoadoutHead::default();
        h.version = 25_000_000;
        h.flags = LOADOUT_HAS_R;
        h.r.class = Str::new(&weapon);
        h.r.head = Str::new(&mod_path("Head"));
        let rows: Vec<ArmorRow> = pieces.iter().map(|(s, p)| {
            let mut r = ArmorRow::default();
            r.class = Str::new(p);
            r.slot = *s;
            r.flags = ROW_PIECE | ROW_PASSPORT;
            r.rust = Bool::TRUE;
            r
        }).collect();
        let rec = hsmp_ipc::record::to_payload(&h, &rows);
        let new_wire = rec.len() + hsmp_ipc::wire::HDR;
        println!("loadout knight (12 pieces + passports, 1 weapon): today JSON {} B in {} chunks = {} B on the wire per send, \
                  resent every 5 s; record {} B (+8 header) = {} B, sent once per version", json.len(), chunks, old_wire, rec.len(), new_wire);
        // Steady state over a 5-minute round with one gear change: 60 periodic sends today vs 1.
        println!("5 min, 1 version: today {} B upstream (x N-1 relayed), record {} B", old_wire * 60, new_wire);
        assert!(new_wire < 64 * 1024);
    }
}
