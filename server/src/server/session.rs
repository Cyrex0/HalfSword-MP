//! Session & roster: peer leave (admin promotion), the per-peer liveness /
//! speed-capped root bookkeeping of the peer table, and the session
//! layer:
//!
//! * identity: `PlayerKey` per peer, nick deduplication ("Willie (2)"), seats
//!   keyed by player key (reclaimed on reconnect);
//! * the `session` record built from server state (epoch, seq, phase +
//!   deadline, live and frozen config, roster rows by seat), framed once and sent
//!   at 1 Hz in the lobby, 3 Hz in a match and immediately on change (`session_due`);
//! * `command` records (op + typed args + config patch) with exactly one `cmd_result`
//!   per `(player, cmd_id)`, the last 64 cached per player (`run_command`); RCON
//!   goes through the same `apply_command`;
//! * phase-transition events for the gate (`observe_phase`, `--events`).

use super::*;
use crate::events;
use rand::RngCore;
use serde_json::json;
use v5::{AdminRole, CmdReason, Jip, Phase, ResultReason, Role, TeamRule};
use hsmp_ipc::layout::{Bool, Str};
use hsmp_ipc::schema::session as rec;
use rec::{cfg, cmd_op};

pub(crate) async fn peer_leave(
    socket: &UdpSocket,
    state: &Arc<ServerState>,
    from: SocketAddr,
) -> anyhow::Result<()> {
    let (removed_id, admin_gone, addrs) = {
        let mut inner = state.inner.lock().await;
        let removed = inner.peers.remove(&from);
        state.net.close_later(&from, hsmp_net::net::close_code::NORMAL);
        state.relay.forget(&from);
        super::dispatch::forget_stream_limits(&from);
        let Some(peer) = removed else { return Ok(()); };
        // An admin left: nobody inherits admin; the listen
        // host gets it back by key when it reconnects (`refresh_admins`).
        let host_gone = admin_left(&mut inner, &peer);
        // Notices for everyone still here (HUD toasts): left / disconnected,
        // and the host hand-over.
        let on_purpose = inner.sess.leaving.remove(&peer.id);
        forget_peer_locked(&mut inner, peer.id);
        if !inner.peers.is_empty() {
            push_notice(&mut inner, v5::Notice::PLAYER_LEFT, &[&peer.nick, if on_purpose { "left" } else { "disconnected" }]);
            if host_gone {
                // "The host <nick> left" (no successor: nobody is promoted).
                push_notice(&mut inner, v5::Notice::HOST_LEFT, &[&peer.nick, ""]);
            }
        }
        inner.match_state_dirty = true;
        let addrs: Vec<SocketAddr> = inner.peers.keys().copied().collect();
        (peer.id, host_gone || peer.is_admin, addrs)
    };
    crate::loadout::forget(removed_id).await;
    // (The roster of the next session snapshot no longer lists the peer: no PeerLeft.)
    flush_out(socket, state).await;
    if admin_gone {
        info!(peer_id = removed_id, "an admin left (nobody is promoted in its place)");
        broadcast_admin_state(socket, state, &addrs).await;
    }
    info!(peer_id = removed_id, %from, "peer left");
    crate::stats::left();
    Ok(())
}

/// Everything the server keeps per peer id, dropped in one place when a peer
/// is removed (clean leave, timeout). Peer ids are never reused,
/// so anything left behind grows with every reconnect. The loadout chunk
/// store (async lock) is dropped by the caller with `loadout::forget`.
pub(crate) fn forget_peer_locked(inner: &mut Inner, id: PeerId) {
    inner.match_peers.remove(&id);
    inner.sess.load_errors.remove(&id);
    inner.sess.placed.remove(&id);
    inner.sess.stalled.remove(&id);
    inner.sess.leaving.remove(&id);
    crate::lagcomp::forget(id); // + validate::damage (kit facts, neck health)
    crate::combat::ledger_forget(id);
    crate::combat::engine_forget(id);
    crate::validate::cheat::forget(id);
    super::dispatch::forget_limits(id);
}

/// `gone` (already removed from the peer table) left: recompute the admins
/// (nobody is promoted in its place: a listen host that drops gets admin
/// back by its key when it returns, and nobody else ever inherits it).
/// True when the listen host (OWNER) left.
pub(crate) fn admin_left(inner: &mut Inner, gone: &PeerState) -> bool {
    let was_owner = inner.admins.role(&peer_key(gone)) == AdminRole::OWNER;
    if gone.is_admin || gone.id == inner.admin_peer_id {
        refresh_admins(inner);
        info!(peer_id = gone.id, owner = was_owner, admin_now = inner.admin_peer_id, "an admin left; no one is promoted");
    }
    was_owner
}

/// Speed-capped root acceptance (anti-teleport). Returns the sender's peer id
/// when the update is valid and should be relayed.
fn dist3(a: [f32; 3], b: [f32; 3]) -> f32 {
    ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2) + (a[2] - b[2]).powi(2)).sqrt()
}

pub(super) async fn accept_root(state: &Arc<ServerState>, from: SocketAddr, position: [f32; 3], stored: hsmp_ipc::schema::pose::Root) -> Option<PeerId> {
    let accepted = {
        let mut inner = state.inner.lock().await;
        let st = inner.server_tick;
        // The configured tick rate.
        let hz = inner.sess.tick_hz.max(1) as f32;
        let p = inner.peers.get_mut(&from)?;
        p.last_seen_tick = st;
        // NaN-safe: a non-finite / out-of-world position is
        // refused, also as the first packet, and a NaN speed never passes.
        // The burst is a per-peer distance bucket on the server clock (not a
        // fresh allowance per packet, which ten roots bunched in one tick could
        // each claim), plus the plain speed rule for a long step
        // whose time since the last accepted root covers it (no lock-out).
        let id = p.id;
        let accept = match p.last_valid_pos {
            None => crate::validate::input::root_step_ok(None, position, 0.0),
            Some(prev) => {
                let dt_s = st.wrapping_sub(p.last_valid_tick) as f32 / hz;
                let d = dist3(prev, position);
                crate::validate::input::root_step_ok(Some((prev, 1.0)), position, f32::INFINITY)
                    && d.is_finite()
                    && (super::dispatch::within_budget(state, id, crate::validate::rate::Kind::RootDist, d)
                        || (dt_s > 0.0 && d <= crate::validate::input::ROOT_SPEED_MAX * dt_s
                            && { super::dispatch::drain_budget(state, id, crate::validate::rate::Kind::RootDist); true }))
            }
        };
        let prev = p.last_valid_pos;
        if !accept {
            // 30/s for an honest ragdoll launch: budgeted.
            if crate::validate::rate::log_ok("root_speed_cap") {
                warn!(peer_id = p.id, ?position, ?prev, "speed cap exceeded (or non-finite / out-of-world position); rejected");
            } else {
                debug!(peer_id = p.id, "root rejected (log budget)");
            }
        }
        if accept {
            p.last_valid_pos = Some(position);
            p.last_valid_tick = st;
            p.last_root = Some(stored);
            Some(p.id)
        } else { None }
    };
    if accepted.is_some() { state.relay.update_pos(from, position); }
    accepted
}

/// Liveness bookkeeping for stream packets that need no validation.
pub(super) async fn touch_peer(state: &Arc<ServerState>, from: SocketAddr) -> Option<PeerId> {
    let mut inner = state.inner.lock().await;
    let st = inner.server_tick;
    let p = inner.peers.get_mut(&from)?;
    p.last_seen_tick = st;
    Some(p.id)
}

// =============================================================================
// Identity
// =============================================================================

/// Ed25519 public key (docs/development/protocol.md). Seats, wins and admin are keyed
/// by it; the nick is cosmetic.
pub type PlayerKey = [u8; 32];

/// Until the v5 handshake delivers the real key, a peer's identity is derived
/// from its deduplicated nick: unique among connected peers, and stable for a
/// player who reconnects under the same nick.
pub(crate) fn provisional_key(nick: &str) -> PlayerKey {
    use sha2::{Digest, Sha256};
    let mut h = Sha256::new();
    h.update(b"HSMP provisional player key\0");
    h.update(nick.as_bytes());
    h.finalize().into()
}

/// The identity seats / wins / admin / command dedup are keyed by: the v5
/// handshake's Ed25519 `player_key`. An all-zero key (no identity: unit-test
/// fixtures, tools) falls back to the provisional key of the nick.
pub(crate) fn peer_key(p: &PeerState) -> PlayerKey {
    if p.player_key != [0u8; 32] { p.player_key } else { provisional_key(&p.nick) }
}

/// Display id = SHA-256(player_key)[0..8] (docs/development/protocol.md).
pub(crate) fn player_id(k: &PlayerKey) -> [u8; 8] {
    use sha2::{Digest, Sha256};
    let d = Sha256::digest(k);
    let mut out = [0u8; 8];
    out.copy_from_slice(&d[..8]);
    out
}

/// `player_id` remembered per thread: every roster row of the `session` snapshot (built each
/// tick) needs one, and the hash would dominate the tick.
fn player_id_cached(k: &PlayerKey) -> [u8; 8] {
    use std::cell::RefCell;
    thread_local! {
        static IDS: RefCell<HashMap<PlayerKey, [u8; 8]>> = RefCell::new(HashMap::new());
    }
    IDS.with(|m| {
        let mut m = m.borrow_mut();
        if let Some(id) = m.get(k) { return *id; }
        if m.len() >= 4096 { m.clear(); }
        *m.entry(*k).or_insert_with(|| player_id(k))
    })
}

pub(crate) fn player_id_hex(k: &PlayerKey) -> String {
    hex::encode(player_id(k))
}

const NICK_MAX: usize = 32;

fn clip_bytes(s: &str, max: usize) -> &str {
    if s.len() <= max { return s; }
    let mut i = max;
    while !s.is_char_boundary(i) { i -= 1; }
    &s[..i]
}

/// Printable, trimmed, at most 32 bytes; empty becomes "Willie".
pub(crate) fn clean_nick(raw: &str) -> String {
    let s: String = raw.chars().filter(|c| !c.is_control()).collect();
    let s = clip_bytes(s.trim(), NICK_MAX).trim().to_string();
    if s.is_empty() { "Willie".into() } else { s }
}

/// First of `base`, `base (2)`, `base (3)` ... that no name in `taken` uses
/// (case-insensitive). The result stays within 32 bytes.
pub(crate) fn dedup_nick_among<'a>(taken: impl IntoIterator<Item = &'a str>, wanted: &str) -> String {
    let base = clean_nick(wanted);
    let taken: HashSet<String> = taken.into_iter().map(|t| t.to_lowercase()).collect();
    if !taken.contains(&base.to_lowercase()) { return base; }
    for n in 2u32.. {
        let suffix = format!(" ({n})");
        let stem = clip_bytes(&base, NICK_MAX - suffix.len()).trim_end();
        let cand = format!("{stem}{suffix}");
        if !taken.contains(&cand.to_lowercase()) { return cand; }
    }
    unreachable!()
}

/// The nick a joining peer gets: deduplicated against every other connected
/// peer (a peer re-joining from the same address keeps its own nick).
/// (dispatch.rs admit() dedups against its own `taken` list with the same rule.)
#[cfg_attr(not(test), allow(dead_code))]
pub(super) fn dedup_nick(inner: &Inner, from: SocketAddr, wanted: &str) -> String {
    let taken = inner.peers.iter().filter(|(a, _)| **a != from).map(|(_, p)| p.nick.as_str());
    let nick = dedup_nick_among(taken, wanted);
    if nick != wanted {
        info!(wanted = %wanted, nick = %nick, "nick deduplicated");
    }
    nick
}

// =============================================================================
// Session core
// =============================================================================

/// Kit rules + per-peer kit revisions, copied from loadout.rs every tick.
#[derive(Debug, Clone, Default)]
pub(crate) struct KitView {
    pub mode: u8,
    pub budget: u16,
    pub revs: HashMap<PeerId, u64>,
}

/// Options from the command line (main.rs).
#[derive(Debug, Clone)]
pub(crate) struct SessionOpts {
    pub debug_verbs: bool,
    pub tick_hz: u32,
    /// Advertised mode string ("duel", "ffa", ...).
    pub mode: String,
}

/// Results cached per player: a duplicate `cmd_id` gets the same answer.
pub(crate) const CMD_CACHE: usize = 64;

pub(crate) struct SessionCore {
    /// `server_epoch`: the transport's (`state.net.epoch()`, the same value
    /// S2CWelcome carries), set by `configure_session`; random in tests.
    pub epoch: u64,
    /// Last sent `S2CSession.seq`.
    pub seq: u32,
    /// New on every START; 0 in the lobby.
    pub match_id: u64,
    pub frozen: Option<rec::SessionConfig>,
    pub config_rev: u32,
    cfg_sig: Option<(String, u8, u8, u16)>,
    /// Seat order and roster rows `session_due` builds into every tick.
    snap_bufs: (Vec<(u8, PlayerKey)>, Vec<rec::RosterRow>),
    /// Seat (1-based) per player key; kept for a participant while a match
    /// runs so a reconnect reclaims it.
    pub seats: HashMap<PlayerKey, u8>,
    pub nicks: HashMap<PlayerKey, String>,
    pub admin_key: Option<PlayerKey>,
    /// No-admin auto start: server tick at which the lobby
    /// starts the match because every player is ready (`auto_start_step`).
    /// None = not armed.
    pub auto_start_at: Option<u32>,
    /// Winner of the match that just ended (id, nick, wins), fixed when it
    /// ended; the S2CMatchResult names it even if it left meanwhile.
    pub match_winner: Option<(PeerId, String, u32)>,
    cmd_cache: HashMap<PlayerKey, VecDeque<rec::CmdResult>>,
    /// Phase code last reported (events + immediate snapshot).
    pub last_phase: u8,
    /// The match ended by ABORT (`last_result.reason`).
    pub aborted: bool,
    /// The match id / frozen arena of the match that just ended (the
    /// `-> Lobby` phase event still names it).
    ended: (u64, Option<String>),
    deadline: (u8, u64),
    last_sent: Option<SessionSnap>,
    last_sent_ms: u64,
    force_send: bool,
    pub kits: KitView,
    pub debug_verbs: bool,
    pub tick_hz: u32,
    pub max_peers: u8,
    pub mode: u8,
    /// First load error a client reported while loading: peer -> (pending
    /// round, error, server tick). One retry window, then it sits out.
    pub load_errors: HashMap<PeerId, (u32, String, u32)>,
    /// Peers that sat out the pending round after failing to load (nick, error).
    pub sat_out: Vec<(PeerId, String, String)>,
    /// Placement report per peer: (round, server tick) — spawn protection.
    pub placed: HashMap<PeerId, (u32, u32)>,
    /// Server tick of the last countdown -> live.
    pub live_tick: u32,
    /// Consecutive rounds voided because too few fighters loaded.
    pub void_streak: u32,
    next_event_id: u32,
    /// Participants whose link is stalled right now (`observe_links`).
    pub stalled: HashSet<PeerId>,
    /// Peers leaving on purpose (C2SLeave), for the PLAYER_LEFT wording.
    pub leaving: HashSet<PeerId>,
}

fn random_id53() -> u64 {
    (rand::thread_rng().next_u64() & ((1u64 << 53) - 1)).max(1)
}

impl SessionCore {
    pub fn new(max_peers: usize) -> Self {
        SessionCore {
            epoch: random_id53(),
            seq: 0,
            match_id: 0,
            frozen: None,
            config_rev: 1,
            cfg_sig: None,
            snap_bufs: (Vec::new(), Vec::new()),
            seats: HashMap::new(),
            nicks: HashMap::new(),
            admin_key: None,
            auto_start_at: None,
            match_winner: None,
            cmd_cache: HashMap::new(),
            last_phase: Phase::LOBBY,
            aborted: false,
            ended: (0, None),
            deadline: (Phase::LOBBY, 0),
            last_sent: None,
            last_sent_ms: 0,
            force_send: false,
            kits: KitView::default(),
            debug_verbs: false,
            tick_hz: 30,
            max_peers: max_peers.min(255) as u8,
            mode: v5::Mode::DUEL,
            load_errors: HashMap::new(),
            sat_out: Vec::new(),
            placed: HashMap::new(),
            live_tick: 0,
            void_streak: 0,
            next_event_id: 0,
            stalled: HashSet::new(),
            leaving: HashSet::new(),
        }
    }

    /// Ids for `S2CEvent` (idempotent by id on the client).
    pub fn next_event_id(&mut self) -> u32 {
        self.next_event_id = self.next_event_id.wrapping_add(1).max(1);
        self.next_event_id
    }

    fn cached(&self, k: &PlayerKey, cmd_id: u32) -> Option<rec::CmdResult> {
        self.cmd_cache.get(k)?.iter().find(|r| r.cmd_id == cmd_id).copied()
    }

    fn remember(&mut self, k: PlayerKey, r: rec::CmdResult) {
        if self.cmd_cache.len() > 512 && !self.cmd_cache.contains_key(&k) {
            // Bound the map: drop caches of players without a seat.
            let seated: HashSet<PlayerKey> = self.seats.keys().copied().collect();
            self.cmd_cache.retain(|key, _| seated.contains(key));
        }
        let q = self.cmd_cache.entry(k).or_default();
        q.push_back(r);
        while q.len() > CMD_CACHE { q.pop_front(); }
    }
}

pub(crate) fn mode_code(mode: &str) -> u8 {
    match mode.trim().to_ascii_lowercase().as_str() {
        "ffa" | "lms" | "ffa_lms" => v5::Mode::FFA,
        "teams" | "team" | "lts" | "team_elim" => v5::Mode::TEAM_ELIM,
        "koth" | "king" | "king_of_hill" => v5::Mode::KING_OF_HILL,
        _ => v5::Mode::DUEL,
    }
}

/// main.rs: apply the command-line options.
pub(crate) async fn configure_session(state: &Arc<ServerState>, o: SessionOpts) {
    let mut inner = state.inner.lock().await;
    // One epoch and one clock for Welcome and every snapshot.
    inner.sess.epoch = state.net.epoch();
    inner.sess.debug_verbs = o.debug_verbs;
    inner.sess.tick_hz = o.tick_hz.max(1);
    inner.sess.mode = mode_code(&o.mode);
    let epoch = inner.sess.epoch;
    info!(epoch, debug_verbs = o.debug_verbs, "session layer ready");
    events::emit("server_start", json!({"epoch": epoch, "debug_verbs": o.debug_verbs}));
}

// ---- phase ------------------------------------------------------------------

/// The v5 phase of the current string state. `Loading` is the countdown whose
/// load barrier has not passed yet.
pub(crate) fn phase_code(inner: &Inner) -> u8 {
    match inner.match_state.as_str() {
        "countdown" if !inner.barrier_passed => Phase::LOADING,
        "countdown" => Phase::COUNTDOWN,
        "live" => Phase::LIVE,
        "roundover" => Phase::ROUND_OVER,
        "match_over" => Phase::MATCH_OVER,
        "paused" => Phase::PAUSED,
        _ => Phase::LOBBY,
    }
}

/// Event / log label (the gate's vocabulary: Lobby, Loading, Live, RoundOver ...).
pub(crate) fn phase_label(p: u8) -> &'static str {
    match p {
        Phase::LOBBY => "Lobby",
        Phase::LOADING => "Loading",
        Phase::COUNTDOWN => "Countdown",
        Phase::LIVE => "Live",
        Phase::ROUND_OVER => "RoundOver",
        Phase::MATCH_OVER => "MatchOver",
        Phase::POST_MATCH => "PostMatch",
        Phase::PAUSED => "Paused",
        _ => "Unknown",
    }
}

/// The round a phase is about: the pending round while loading / counting
/// down, else the current one.
pub(crate) fn phase_round(inner: &Inner, p: u8) -> u32 {
    if matches!(p, Phase::LOADING | Phase::COUNTDOWN) { inner.match_round + 1 } else { inner.match_round }
}

/// Report a phase change (log, `--events`, immediate snapshot). Called after
/// every tick and every command; returns the new phase when it changed.
pub(crate) fn observe_phase(inner: &mut Inner) -> Option<u8> {
    let p = phase_code(inner);
    let from = inner.sess.last_phase;
    if p == from { return None; }
    inner.sess.last_phase = p;
    inner.sess.force_send = true;
    inner.match_state_dirty = true;
    let round = phase_round(inner, p);
    // Back in the lobby the event still names the match that just ended.
    let (match_id, frozen) = if p == Phase::LOBBY && inner.sess.match_id == 0 {
        inner.sess.ended.clone()
    } else {
        (inner.sess.match_id, inner.sess.frozen.as_ref().map(|c| c.arena.lossy().into_owned()))
    };
    info!(from = phase_label(from), to = phase_label(p), match_id, round,
          frozen_arena = ?frozen, "session phase");
    events::emit("phase", json!({
        "from": phase_label(from), "to": phase_label(p),
        "match_id": match_id, "round": round,
        "arena": frozen.clone().unwrap_or_else(|| inner.match_arena.clone()),
        "frozen_arena": frozen, "epoch": inner.sess.epoch,
    }));
    Some(p)
}

// ---- config -----------------------------------------------------------------

/// The arena START would lock (the lobby's "default" means DEFAULT_ARENA).
fn effective_arena(a: &str) -> &str {
    if a.starts_with("Map_Arena_") { a } else { DEFAULT_ARENA }
}

/// The live (lobby) config, assembled from the server state.
pub(crate) fn live_config(inner: &Inner) -> rec::SessionConfig {
    let s = &inner.sess;
    let hz = s.tick_hz.max(1);
    let secs = |ticks: u32| (ticks / hz).min(255) as u8;
    rec::SessionConfig {
        rev: s.config_rev,
        arena: Str::new(effective_arena(&inner.match_arena)),
        mode: s.mode,
        best_of: inner.best_of,
        round_time_limit_s: 0,
        team_rule: TeamRule::NONE,
        teams: 0,
        kit_mode: s.kits.mode,
        kit_budget: s.kits.budget,
        kit_fairness: 0,
        max_fighters: s.max_peers,
        max_spectators: 0,
        countdown_s: secs(FIRST_COUNTDOWN_TICKS),
        roundover_s: secs(ROUNDOVER_TICKS),
        matchover_s: secs(MATCH_OVER_TICKS),
        barrier_timeout_s: secs(BARRIER_TIMEOUT_TICKS),
        // Late joiners spectate, then fight from the next round they loaded.
        join_in_progress: Jip::NEXT_ROUND,
        _r: [0; 3],
    }
}

/// Bump `config_rev` when any configurable field changed (from any path:
/// commands, legacy verbs, RCON, the kit-rules channel).
pub(crate) fn refresh_config_rev(inner: &mut Inner) {
    // Every tick: compared in place, the arena is copied only when something changed.
    let same = |old: &(String, u8, u8, u16)| old.0 == inner.match_arena && old.1 == inner.best_of
        && old.2 == inner.sess.kits.mode && old.3 == inner.sess.kits.budget;
    if inner.sess.cfg_sig.as_ref().is_some_and(same) { return; }
    let sig = (inner.match_arena.clone(), inner.best_of, inner.sess.kits.mode, inner.sess.kits.budget);
    match &inner.sess.cfg_sig {
        Some(_) => {
            inner.sess.config_rev = inner.sess.config_rev.wrapping_add(1).max(1);
            inner.sess.cfg_sig = Some(sig);
        }
        None => inner.sess.cfg_sig = Some(sig),
    }
}

/// START: new match id, config frozen for the whole match.
fn on_match_start(inner: &mut Inner) {
    refresh_config_rev(inner);
    let cfg = live_config(inner);
    inner.sess.match_id = random_id53();
    inner.sess.frozen = Some(cfg);
    inner.sess.aborted = false;
    inner.sess.match_winner = None;
}

/// Back in the lobby: the frozen config and the match id are gone.
pub(crate) fn on_lobby(inner: &mut Inner) {
    if inner.sess.match_id != 0 {
        inner.sess.ended = (inner.sess.match_id, inner.sess.frozen.as_ref().map(|c| c.arena.lossy().into_owned()));
    }
    inner.sess.frozen = None;
    inner.sess.match_id = 0;
}

// ---- seats ------------------------------------------------------------------

/// Keep `seats` in step with the peer table: free the seats of players that
/// are gone (in a match, a participant's seat is kept for its reconnect), seat
/// every connected player that has none (lowest free seat, by peer id order).
pub(crate) fn reconcile_seats(inner: &mut Inner) {
    // Runs every tick: when nothing changed (the usual case) only the admin key is refreshed,
    // without building the sets below.
    if seats_in_step(inner) {
        if let Some(p) = inner.peers.values().filter(|p| p.is_admin).max_by_key(|p| p.id) {
            inner.sess.admin_key = Some(peer_key(p));
        }
    } else {
        reseat(inner);
    }
    observe_links(inner);
}

/// Every connected player holds a seat under its current nick, and no seat or nick is held
/// by a key that should lose it (`reseat` would change nothing but the admin key).
fn seats_in_step(inner: &Inner) -> bool {
    let in_lobby = inner.match_state == "lobby";
    let kept = |k: &PlayerKey| inner.peers.values().any(|p| peer_key(p) == *k) || (!in_lobby && inner.participants.contains(k));
    inner.peers.values().all(|p| {
        let k = peer_key(p);
        inner.sess.seats.contains_key(&k) && inner.sess.nicks.get(&k) == Some(&p.nick)
    }) && inner.sess.seats.keys().all(kept) && inner.sess.nicks.keys().all(kept)
}

fn reseat(inner: &mut Inner) {
    let in_lobby = inner.match_state == "lobby";
    let mut conn: Vec<(PeerId, PlayerKey, String, bool)> = inner.peers.values()
        .map(|p| (p.id, peer_key(p), p.nick.clone(), p.is_admin))
        .collect();
    conn.sort_by_key(|c| c.0);
    let connected: HashSet<PlayerKey> = conn.iter().map(|c| c.1).collect();
    let keep: HashSet<PlayerKey> = if in_lobby {
        connected.clone()
    } else {
        connected.iter().chain(inner.participants.iter()).copied().collect()
    };
    let before = inner.sess.seats.len();
    inner.sess.seats.retain(|k, _| keep.contains(k));
    inner.sess.nicks.retain(|k, _| keep.contains(k));
    let mut changed = before != inner.sess.seats.len();
    for (_, key, nick, admin) in conn {
        if !inner.sess.seats.contains_key(&key) {
            let used: HashSet<u8> = inner.sess.seats.values().copied().collect();
            if let Some(seat) = (1u8..=254).find(|s| !used.contains(s)) {
                inner.sess.seats.insert(key, seat);
                changed = true;
            }
        }
        inner.sess.nicks.insert(key, nick);
        if admin { inner.sess.admin_key = Some(key); }
    }
    if changed { inner.match_state_dirty = true; }
}

// ---- resume -------------------------------------------------------------------

/// A participant whose game has gone silent this long during a LIVE round is
/// treated as away (its connection may still be open: the transport idle
/// timeout is 10 s client side / 20 s server side). A duel then pauses for
/// its return (RECONNECT_GRACE), and the same seat and wins continue.
pub(crate) const LINK_STALL_TICKS: u32 = 3 * 30;

/// Last tick this peer was heard from: any authenticated packet
/// (`last_seen_tick`, refreshed at most every 500 ms) or a game ping.
fn last_heard(inner: &Inner, p: &PeerState) -> u32 {
    let ping = inner.match_peers.get(&p.id).map(|m| m.last_ping_tick).unwrap_or(0);
    let tick = inner.server_tick;
    // the more recent of the two (wrapping tick arithmetic)
    if tick.wrapping_sub(ping) < tick.wrapping_sub(p.last_seen_tick) { ping } else { p.last_seen_tick }
}

/// The link of a game client counts as up. Only real game clients (they
/// ping) during a live round are judged; headless peers and every other
/// phase are left to the transport and the load barrier.
pub(crate) fn link_ok(inner: &Inner, p: &PeerState) -> bool {
    // live: a silent client pauses the duel; paused: it stays away until it speaks again
    if inner.match_state != "live" && inner.match_state != "paused" { return true; }
    let aware = inner.match_peers.get(&p.id).map(|m| m.aware).unwrap_or(false);
    if !aware { return true; }
    inner.server_tick.wrapping_sub(last_heard(inner, p)) <= LINK_STALL_TICKS
}

/// Edge detector for link stalls (called every tick from `reconcile_seats`):
/// logs + events `link_stall` and, when the same connection comes back,
/// `seat_restored` (same seat, same wins: nothing was released).
fn observe_links(inner: &mut Inner) {
    // Every tick: the unchanged case allocates nothing.
    let stalled_now = inner.peers.values().filter(|p| !link_ok(inner, p)).count();
    if stalled_now == inner.sess.stalled.len()
        && inner.peers.values().all(|p| link_ok(inner, p) || inner.sess.stalled.contains(&p.id)) {
        return;
    }
    let mut now_stalled: HashSet<PeerId> = HashSet::new();
    for p in inner.peers.values() {
        if !link_ok(inner, p) { now_stalled.insert(p.id); }
    }
    let gone: Vec<PeerId> = inner.sess.stalled.difference(&now_stalled).copied().collect();
    let new: Vec<PeerId> = now_stalled.difference(&inner.sess.stalled).copied().collect();
    for id in new {
        if let Some(p) = inner.peers.values().find(|p| p.id == id) {
            let seat = inner.sess.seats.get(&peer_key(p)).copied();
            warn!(peer_id = id, nick = %p.nick, ?seat, "participant link stalled (no packet for 3 s); its seat is kept");
            events::emit("link_stall", json!({"peer_id": id, "seat": seat, "round": inner.match_round}));
        }
        inner.match_state_dirty = true;
    }
    for id in gone {
        if let Some(p) = inner.peers.values().find(|p| p.id == id) {
            let key = peer_key(p);
            let seat = inner.sess.seats.get(&key).copied().unwrap_or(0);
            let saved = inner.wins_by_key.get(&key).copied();
            info!(peer_id = id, nick = %p.nick, seat, wins = p.wins, "participant link back (same connection, same seat)");
            events::emit("seat_restored", json!({
                "player_key": player_id_hex(&key), "seat": seat, "old_seat": seat, "same_seat": true,
                "wins": p.wins, "same_wins": saved.map_or(true, |w| w == p.wins),
                "match_id": inner.sess.match_id, "how": "link_back",
            }));
        }
        inner.match_state_dirty = true;
    }
    inner.sess.stalled = now_stalled;
}

/// A participant LEFT on purpose (C2SLeave: the player chose LEAVE MATCH /
/// BACK TO MENU, or quit). Unlike a dropped link there is nothing to wait
/// for: in a duel the other player wins by forfeit now (no 30 s pause); with
/// more players the leaver is out of the match. Everyone gets a notice.
/// Call BEFORE `peer_leave` (the peer must still be in the table).
pub(crate) fn on_deliberate_leave(inner: &mut Inner, from: SocketAddr, reason: u8) {
    let Some(p) = inner.peers.get(&from) else { return };
    let (id, nick, key) = (p.id, p.nick.clone(), peer_key(p));
    let active = matches!(inner.match_state.as_str(), "countdown" | "live" | "roundover" | "paused");
    let participant = inner.participants.contains(&key);
    info!(peer_id = id, %nick, reason, active, participant, "player left on purpose");
    inner.sess.leaving.insert(id);   // peer_leave words the PLAYER_LEFT notice "left"
    let match_id = inner.sess.match_id;
    if !(active && participant) { return; }
    inner.participants.retain(|k| *k != key);
    inner.wins_by_key.remove(&key);
    let others: Vec<PeerId> = inner.peers.values()
        .filter(|q| q.id != id && inner.participants.contains(&peer_key(q)))
        .map(|q| q.id)
        .collect();
    if others.len() == 1 && inner.match_state != "match_over" {
        info!(winner = others[0], leaver = id, "match: opponent left on purpose; forfeit");
        events::emit("forfeit", json!({"winner": others[0], "leaver": id, "match_id": match_id}));
        forfeit_to(inner, others[0]);
    }
    inner.match_state_dirty = true;
}

/// The listen host (HSMP_LISTEN_HOST=1: the server runs inside a player's
/// game session, started by HSMPMenu HOST) leaving on purpose ends the server.
pub(crate) fn host_closes_on_leave(inner: &Inner, from: SocketAddr) -> bool {
    let listen = std::env::var("HSMP_LISTEN_HOST").map(|v| v.trim() == "1").unwrap_or(false);
    // Only the host itself (OWNER): an admin it granted leaving closes nothing.
    listen && inner.peers.get(&from).map_or(false, |p| inner.admins.role(&peer_key(p)) == AdminRole::OWNER)
}

/// The owner's (the listen host's) NET_STATUS notice: is the router port open, what is the
/// public address (nat/). Sent to `only` (a joining owner) or to every connected owner.
pub(crate) fn push_net_status(inner: &mut Inner, only: Option<SocketAddr>) {
    let owners: Vec<SocketAddr> = inner
        .peers
        .iter()
        .filter(|(a, p)| only.map_or(true, |o| o == **a) && inner.admins.role(&peer_key(p)) == AdminRole::OWNER)
        .map(|(a, _)| *a)
        .collect();
    if owners.is_empty() {
        return;
    }
    let args = crate::nat::status().notice_args();
    let a: Vec<&str> = args.iter().map(String::as_str).collect();
    for o in owners {
        let event_id = inner.sess.next_event_id();
        let m = super::session_records::notice_msg(event_id, inner.sess.match_id, v5::Notice::NET_STATUS, &a);
        inner.out_msgs.push((Some(o), m));
    }
}

/// Re-send NET_STATUS to the owner whenever what it says changes.
pub async fn net_status_notices(state: Arc<ServerState>) {
    let mut rx = crate::nat::subscribe();
    let mut last = crate::nat::status().notice_args();
    while rx.changed().await.is_ok() {
        let now = crate::nat::status().notice_args();
        if now == last {
            continue;
        }
        last = now;
        push_net_status(&mut *state.inner.lock().await, None);
    }
}

/// A player arrived: notice for everyone else (HUD toast "<nick> joined").
fn notice_joined(inner: &mut Inner, nick: &str, rejoined: bool) {
    push_notice(inner, v5::Notice::PLAYER_JOINED, &[nick, if rejoined { "rejoined" } else { "joined" }]);
}

/// Seat of a connected peer.
pub(crate) fn seat_of_peer(inner: &Inner, pid: PeerId) -> Option<u8> {
    let p = inner.peers.values().find(|p| p.id == pid)?;
    inner.sess.seats.get(&peer_key(p)).copied()
}

/// Peer id at `seat` (connected peers only).
pub(crate) fn peer_at_seat(inner: &Inner, seat: u8) -> Option<PeerId> {
    inner.peers.values().find(|p| inner.sess.seats.get(&peer_key(p)) == Some(&seat)).map(|p| p.id)
}

/// After a peer joined (dispatch): seat it, and when this key already held a
/// seat (a reconnect) restore its wins and report `seat_restored`.
pub(super) fn on_joined(inner: &mut Inner, from: SocketAddr) {
    let Some(key) = inner.peers.get(&from).map(peer_key) else { return };
    let had = inner.sess.seats.get(&key).copied();
    let active = inner.match_state != "lobby";
    let saved = inner.wins_by_key.get(&key).copied();
    let mut restored_wins = None;
    if active && inner.participants.contains(&key) {
        if let (Some(w), Some(p)) = (saved, inner.peers.get_mut(&from)) {
            p.wins = w;
            restored_wins = Some(w);
        }
    }
    reconcile_seats(inner);
    if let Some(nick) = inner.peers.get(&from).map(|p| p.nick.clone()) {
        if inner.peers.len() > 1 { notice_joined(inner, &nick, had.is_some()); }
    }
    push_net_status(inner, Some(from));
    if let Some(old) = had {
        let seat = inner.sess.seats.get(&key).copied().unwrap_or(0);
        let wins = inner.peers.get(&from).map(|p| p.wins).unwrap_or(0);
        info!(seat, same_seat = seat == old, wins, "seat restored for reconnecting player");
        events::emit("seat_restored", json!({
            "player_key": player_id_hex(&key), "seat": seat, "old_seat": old,
            "same_seat": seat == old, "wins": wins,
            "same_wins": saved.map_or(true, |w| restored_wins == Some(w)),
            "match_id": inner.sess.match_id,
        }));
    }
}

/// A peer's session resumed on a new connection (dispatch `resume`):
/// same id, seat, wins, alive state and per-id records. Reported like a
/// seat restore (the gate's vocabulary) and as `session_resumed`; everyone's
/// snapshot goes out now.
pub(super) fn on_resumed(inner: &mut Inner, from: SocketAddr, old: SocketAddr) {
    let Some(p) = inner.peers.get(&from) else { return };
    let (key, id, wins, alive, nick) = (peer_key(p), p.id, p.wins, p.alive, p.nick.clone());
    reconcile_seats(inner);
    let seat = inner.sess.seats.get(&key).copied().unwrap_or(0);
    inner.sess.force_send = true;
    inner.match_state_dirty = true;
    info!(peer_id = id, %nick, seat, wins, alive, %old, new = %from, "seat kept: session resumed");
    events::emit("seat_restored", json!({
        "player_key": player_id_hex(&key), "seat": seat, "old_seat": seat, "same_seat": true,
        "wins": wins, "same_wins": true, "match_id": inner.sess.match_id, "how": "resume",
    }));
    events::emit("session_resumed", json!({
        "peer_id": id, "seat": seat, "alive": alive, "round": inner.match_round,
        "match_id": inner.sess.match_id, "same_addr": old == from,
    }));
}

// ---- snapshot ---------------------------------------------------------------

fn result_reason(inner: &Inner) -> u8 {
    if inner.sess.aborted { return ResultReason::ABORTED; }
    match inner.match_reason.as_str() {
        "draw" => ResultReason::DRAW,
        "forfeit" => ResultReason::FORFEIT,
        "opponent_left" => ResultReason::OPPONENT_LEFT,
        "load_failed" => rec::result_reason::LOAD_FAILED,
        "" if inner.last_winner != 0 => ResultReason::KILL,
        _ => ResultReason::NONE,
    }
}

fn deadline_candidate(inner: &Inner, phase: u8, now_ms: u64) -> u64 {
    let hz = inner.sess.tick_hz.max(1) as u64;
    let ticks = match phase {
        Phase::LOADING => {
            let left = inner.barrier_deadline.wrapping_sub(inner.server_tick);
            if left < u32::MAX / 2 { left } else { 0 }
        }
        Phase::COUNTDOWN | Phase::ROUND_OVER | Phase::MATCH_OVER | Phase::PAUSED => inner.countdown_ticks,
        // No-admin auto start armed (`auto_start_step`): when it fires.
        Phase::LOBBY => match inner.sess.auto_start_at {
            Some(at) => { let left = at.wrapping_sub(inner.server_tick); if left < u32::MAX / 2 { left.max(1) } else { 0 } }
            None => 0,
        },
        _ => 0,
    } as u64;
    if ticks == 0 { 0 } else { now_ms + ticks * 1000 / hz }
}

/// Keep `phase_deadline_ms` stable between snapshots (a few ms of tick jitter
/// must not count as a change), but follow real changes.
fn update_deadline(inner: &mut Inner, now_ms: u64) {
    let p = phase_code(inner);
    let c = deadline_candidate(inner, p, now_ms);
    let (op, od) = inner.sess.deadline;
    if op != p || (c == 0) != (od == 0) || c.abs_diff(od) > 250 {
        inner.sess.deadline = (p, c);
    }
}

/// The full snapshot (seq and server time as given; `session_due` stamps them): the
/// `session` record's head and roster rows, by seat.
pub(crate) fn build_session(inner: &Inner, now_ms: u64) -> SessionSnap {
    let mut rows = Vec::new();
    let head = fill_session(inner, now_ms, &mut Vec::new(), &mut rows);
    SessionSnap { head, rows }
}

/// `build_session` into buffers the caller keeps (`session_due` runs every tick and reuses
/// them): returns the head, `rows` holds the roster.
fn fill_session(inner: &Inner, now_ms: u64, seats: &mut Vec<(u8, PlayerKey)>, rows: &mut Vec<rec::RosterRow>) -> rec::SessionHead {
    let s = &inner.sess;
    let phase = phase_code(inner);
    let config = live_config(inner);
    let frozen = if phase == Phase::LOBBY { None } else { Some(s.frozen.unwrap_or(config)) };
    seats.clear();
    seats.extend(s.seats.iter().map(|(k, v)| (*v, *k)));
    seats.sort_unstable_by_key(|x| x.0);
    // A linear search: a roster is small, and a map would be rebuilt every tick.
    let by_key = |k: &PlayerKey| inner.peers.values().find(|p| peer_key(p) == *k);
    let in_match = phase != Phase::LOBBY;
    let waiting: Vec<PeerId> = if phase == Phase::LOADING { barrier_waiting_on(inner) } else { Vec::new() };
    rows.clear();
    for &(seat, key) in seats.iter().take(rec::MAX_ROSTER) {
        let participant = inner.participants.contains(&key);
        let role = if !in_match || participant { Role::FIGHTER } else { Role::SPECTATOR };
        let mut r = rec::RosterRow { seat, role, player_id: player_id_cached(&key), ..Default::default() };
        match by_key(&key) {
            Some(p) => {
                let mp = inner.match_peers.get(&p.id);
                r.peer_id = p.id;
                r.nick = Str::new(&p.nick);
                r.ready = Bool::from(p.ready);
                r.loaded_round = mp.map(|m| m.loaded_round).unwrap_or(0);
                r.alive = Bool::from(p.alive);
                r.wins = p.wins;
                if let Some(a) = inner.spawn_plan.iter().find(|a| a.peer_id == p.id) {
                    r.spawn_id = a.spawn_id;
                    r.spawn_pos = a.pos;
                    r.spawn_yaw = a.yaw;
                    r.spawn_protect_ms = a.protect_ms as u32;
                    r.spawn_slot = a.slot;
                }
                r.kit_rev = s.kits.revs.get(&p.id).copied().unwrap_or(0);
                // OWNER = the listen host, ADMIN = configured / granted.
                r.admin_role = if p.is_admin { inner.admins.role(&key).max(AdminRole::ADMIN) } else { AdminRole::NONE };
                r.connected = Bool::TRUE;
                r.waiting = Bool::from(waiting.contains(&p.id));
            }
            None => {
                // A participant whose connection dropped: the seat waits for it.
                r.nick = Str::new(s.nicks.get(&key).map(String::as_str).unwrap_or(""));
                r.wins = inner.wins_by_key.get(&key).copied().unwrap_or(0);
            }
        }
        rows.push(r);
    }
    let winner_seat = if inner.last_winner != 0 { seat_of_peer(inner, inner.last_winner).unwrap_or(rec::NO_SEAT) } else { rec::NO_SEAT };
    let head = rec::SessionHead {
        epoch: s.epoch,
        match_id: if in_match { s.match_id } else { 0 },
        phase_deadline_ms: if s.deadline.0 == phase { s.deadline.1 } else { deadline_candidate(inner, phase, now_ms) },
        server_time_ms: now_ms,
        seq: s.seq,
        round: inner.match_round,
        // TODO: world_glue's epoch is private to that module; 0 = unknown.
        world_epoch: 0,
        phase,
        winner_seat,
        result_reason: result_reason(inner),
        has_frozen: Bool::from(frozen.is_some()),
        n: rows.len() as u16,
        _r: 0,
        _r2: 0,
        config,
        frozen: frozen.unwrap_or_default(),
    };
    head
}

/// Same snapshot content apart from the per-send stamps (seq, server time).
fn same_content(a: &SessionSnap, b: &SessionSnap) -> bool {
    same_parts(a, &b.head, &b.rows)
}

fn same_parts(a: &SessionSnap, head: &rec::SessionHead, rows: &[rec::RosterRow]) -> bool {
    let mut x = a.head;
    x.seq = head.seq;
    x.server_time_ms = head.server_time_ms;
    x == *head && a.rows == rows
}

/// Snapshot to broadcast now, if any: on change, else every 1 s in the lobby
/// and every 333 ms in a match. Stamps `seq` (strictly increasing).
pub(crate) fn session_due(inner: &mut Inner, now_ms: u64) -> Option<SessionSnap> {
    refresh_config_rev(inner);
    update_deadline(inner, now_ms);
    // Built every tick into kept buffers; copied out only when it is sent.
    let (mut seats, mut rows) = std::mem::take(&mut inner.sess.snap_bufs);
    let mut head = fill_session(inner, now_ms, &mut seats, &mut rows);
    let every = if head.phase == Phase::LOBBY { 1000 } else { 333 };
    let changed = inner.sess.last_sent.as_ref().map_or(true, |l| !same_parts(l, &head, &rows));
    if !changed && !inner.sess.force_send && now_ms < inner.sess.last_sent_ms + every {
        inner.sess.snap_bufs = (seats, rows);
        return None;
    }
    inner.sess.seq = inner.sess.seq.wrapping_add(1);
    head.seq = inner.sess.seq;
    let snap = SessionSnap { head, rows: rows.clone() };
    inner.sess.snap_bufs = (seats, rows);
    inner.sess.last_sent = Some(snap.clone());
    inner.sess.last_sent_ms = now_ms;
    inner.sess.force_send = false;
    Some(snap)
}

// =============================================================================
// Commands
// =============================================================================

/// Who issued a command.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Actor {
    Peer(SocketAddr),
    /// RCON / console: always admin, never deduplicated.
    Rcon,
}

/// Side effects that need sends (run after the state lock is released).
#[derive(Debug, Default)]
pub(crate) struct CmdEffects {
    /// (target, ban, reason, by)
    pub kicks: Vec<(SocketAddr, bool, String, String)>,
    pub kit_rules: Option<(u8, u16)>,
    pub admin_changed: bool,
    /// A ban was lifted: persist the banlist, send the admin state.
    pub unbanned: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Outcome {
    pub ok: bool,
    pub code: u16,
    pub text: String,
}

fn ok(text: impl Into<String>) -> Outcome { Outcome { ok: true, code: CmdReason::OK, text: text.into() } }
fn refuse(code: u16, text: impl Into<String>) -> Outcome { Outcome { ok: false, code, text: text.into() } }

/// Short name for logs and events ("start", "pick_arena", "best_of" ...).
pub(crate) fn cmd_name(c: &rec::Command) -> &'static str {
    match c.op {
        cmd_op::READY if c.flag.get() => "ready",
        cmd_op::READY => "unready",
        cmd_op::START => "start",
        cmd_op::ABORT => "abort",
        cmd_op::PICK_ARENA => "pick_arena",
        cmd_op::SET_CONFIG => match c.patch.mask {
            cfg::BEST_OF => "best_of",
            cfg::KIT_RULES => "kit_rules",
            _ => "set_config",
        },
        cmd_op::KICK => "kick",
        cmd_op::BAN => "ban",
        cmd_op::PROMOTE => "promote",
        cmd_op::VOTE => "vote",
        cmd_op::SWITCH_ROLE => "switch_role",
        cmd_op::SET_TEAM => "set_team",
        cmd_op::UNBAN => "unban",
        cmd_op::RESET_MATCH => "reset_match",
        _ => "unknown",
    }
}

pub(crate) fn reason_name(code: u16) -> &'static str {
    match code {
        CmdReason::OK => "OK",
        CmdReason::NOT_ADMIN => "NOT_ADMIN",
        CmdReason::WRONG_PHASE => "WRONG_PHASE",
        CmdReason::NOT_ALL_READY => "NOT_ALL_READY",
        CmdReason::REV_MISMATCH => "REV_MISMATCH",
        CmdReason::UNKNOWN_ARENA => "UNKNOWN_ARENA",
        CmdReason::INVALID_VALUE => "INVALID_VALUE",
        CmdReason::UNKNOWN_PLAYER => "UNKNOWN_PLAYER",
        CmdReason::NOT_ENOUGH_PLAYERS => "NOT_ENOUGH_PLAYERS",
        CmdReason::RATE_LIMITED => "RATE_LIMITED",
        CmdReason::UNSUPPORTED => "UNSUPPORTED",
        _ => "UNKNOWN",
    }
}

/// Highest best-of a command accepts (the gate runs 10 rounds with BESTOF 19).
pub(crate) const MAX_BEST_OF: u8 = 31;

/// Apply one command to the state (the single command path for the game's typed
/// `command` records and RCON). Acts on the borrowed record. Pure apart from `fx`; never
/// sends.
pub(crate) fn apply_command(inner: &mut Inner, actor: Actor, cmd: &rec::Command, fx: &mut CmdEffects) -> Outcome {
    let (is_admin, me, by) = match actor {
        Actor::Rcon => (true, None, "rcon".to_string()),
        Actor::Peer(a) => match inner.peers.get(&a) {
            Some(p) => (p.is_admin, Some(a), p.nick.clone()),
            None => return refuse(CmdReason::UNKNOWN_PLAYER, "not connected"),
        },
    };
    let lobby = inner.match_state == "lobby";
    let admin_only = !matches!(cmd.op, cmd_op::READY | cmd_op::VOTE | cmd_op::SWITCH_ROLE | cmd_op::SET_TEAM);
    if admin_only && !is_admin {
        return refuse(CmdReason::NOT_ADMIN, "only the host can change the map or start/abort the match");
    }
    let text = cmd.text.as_str().unwrap_or("").trim();
    match cmd.op {
        cmd_op::READY => {
            let v = cmd.flag.get();
            let Some(a) = me else { return refuse(CmdReason::UNSUPPORTED, "ready is a player command") };
            let flipped = inner.peers.get_mut(&a).and_then(|p| {
                if p.ready != v { p.ready = v; Some((p.id, p.nick.clone())) } else { None }
            });
            if let Some((id, nick)) = flipped {
                inner.match_state_dirty = true;
                info!(peer_id = id, nick = %nick, "{}", if v { "ready" } else { "unready" });
            }
            ok(if v { "ready" } else { "not ready" })
        }
        cmd_op::START => start_match(inner, cmd.flag.get(), &by),
        cmd_op::ABORT | cmd_op::RESET_MATCH => {
            let reset = cmd.op == cmd_op::RESET_MATCH;
            let r = if lobby {
                ok("already in the lobby")
            } else {
                info!(by = %by, state = %inner.match_state, "match aborted; back to lobby");
                reset_to_lobby(inner);
                inner.sess.aborted = true;
                ok("aborted")
            };
            if reset {
                // The admin's "reset match": wins, alive and ready start over too.
                for p in inner.peers.values_mut() { p.wins = 0; p.alive = true; p.ready = false; }
                inner.match_state_dirty = true;
            }
            r
        }
        cmd_op::PICK_ARENA => {
            if !lobby { return refuse(CmdReason::WRONG_PHASE, "map can only be changed in the lobby"); }
            let Some(arena) = resolve_arena(text) else {
                return refuse(CmdReason::UNKNOWN_ARENA, format!("unknown arena: {}", normalize_arena(text)));
            };
            set_arena(inner, arena, &by);
            ok(inner.match_arena.clone())
        }
        cmd_op::SET_CONFIG => set_config(inner, cmd.expected_rev, &cmd.patch, fx, &by),
        cmd_op::KICK | cmd_op::BAN => {
            let ban = cmd.op == cmd_op::BAN;
            let Some(addr) = inner.peers.iter().find(|(_, p)| p.id == cmd.peer_id).map(|(a, _)| *a) else {
                return refuse(CmdReason::UNKNOWN_PLAYER, format!("no such peer id={}", cmd.peer_id));
            };
            let reason = if text.is_empty() {
                if ban { "banned".to_string() } else { "kicked".to_string() }
            } else { text.to_string() };
            fx.kicks.push((addr, ban, reason, by));
            ok(if ban { "banned" } else { "kicked" })
        }
        cmd_op::PROMOTE => {
            let Some(key) = inner.peers.values().find(|p| p.id == cmd.peer_id).map(peer_key) else {
                return refuse(CmdReason::UNKNOWN_PLAYER, format!("no such peer id={}", cmd.peer_id));
            };
            if cmd.role < AdminRole::ADMIN {
                return refuse(CmdReason::UNSUPPORTED, "only promotion to admin is supported");
            }
            // A grant: the target becomes an admin too; the
            // granting admin (and the listen host) stay admins. Not persisted.
            inner.admins.grant(key);
            fx.admin_changed = refresh_admins(inner) || fx.admin_changed;
            info!(target_id = cmd.peer_id, by = %by, "promoted to admin");
            ok("promoted")
        }
        cmd_op::UNBAN => {
            // The admin sees masked entries ("203.0.x.x#1a2b3c4d"): unban by that entry,
            // its token, or the plain IP.
            match resolve_ban(inner, text) {
                Some(ip) if inner.banned_ips.remove(&ip) => {
                    info!(%ip, by = %by, "ip unbanned");
                    fx.unbanned = true;
                    ok("unbanned")
                }
                _ => refuse(CmdReason::UNKNOWN_PLAYER, format!("no such ban: {}", text)),
            }
        }
        _ => refuse(CmdReason::UNSUPPORTED, format!("{} is not supported yet", cmd_name(cmd))),
    }
}

/// START: every connected player takes part (all ready, or one player, or
/// `force`). The single start path of an admin's command, RCON and the
/// no-admin auto start (`auto_start_step`).
pub(crate) fn start_match(inner: &mut Inner, force: bool, by: &str) -> Outcome {
    if inner.match_state != "lobby" {
        return refuse(CmdReason::WRONG_PHASE, format!("match already running ({})", inner.match_state));
    }
    let total = inner.peers.len();
    let ready_count = inner.peers.values().filter(|p| p.ready).count();
    if total == 0 {
        return refuse(CmdReason::NOT_ENOUGH_PLAYERS, "no players connected");
    }
    if !(force || total == 1 || ready_count == total) {
        return refuse(CmdReason::NOT_ALL_READY,
            format!("start blocked: {} of {} peers ready", ready_count, total));
    }
    inner.sess.auto_start_at = None;
    let keys: Vec<PlayerKey> = inner.peers.values().map(peer_key).collect();
    inner.participants = keys;
    inner.pauses_by_key.clear();
    inner.match_pauses = 0;
    inner.paused_from_live = false;
    inner.wins_by_key.clear();
    inner.last_winner = 0;
    inner.match_reason.clear();
    for p in inner.peers.values_mut() { p.wins = 0; p.alive = true; }
    for mp in inner.match_peers.values_mut() { mp.loaded_round = 0; }
    // Every client loads exactly the server's arena: never leave it to
    // each client's local default.
    if !inner.match_arena.starts_with("Map_Arena_") {
        inner.match_arena = DEFAULT_ARENA.to_string();
    }
    info!(arena = %inner.match_arena, force, by = %by, "match start: arena locked");
    on_match_start(inner);
    begin_countdown(inner, FIRST_COUNTDOWN_TICKS);
    ok(format!("starting on {}", inner.match_arena))
}

/// SET_CONFIG (lobby only): arena, best-of, kit rules (the `cfg` bits of the patch's mask).
fn set_config(inner: &mut Inner, expected_rev: u32, p: &rec::ConfigPatch, fx: &mut CmdEffects, by: &str) -> Outcome {
    if inner.match_state != "lobby" {
        return refuse(CmdReason::WRONG_PHASE, "match config is frozen until the lobby");
    }
    refresh_config_rev(inner);
    if expected_rev != 0 && expected_rev != inner.sess.config_rev {
        return refuse(CmdReason::REV_MISMATCH,
            format!("config changed (rev {} != {})", inner.sess.config_rev, expected_rev));
    }
    let has = |bit: u32| p.mask & bit != 0;
    let arena = if has(cfg::ARENA) {
        let a = p.arena.as_str().unwrap_or("");
        match resolve_arena(a) {
            Some(x) => Some(x),
            None => return refuse(CmdReason::UNKNOWN_ARENA, format!("unknown arena: {}", normalize_arena(a))),
        }
    } else { None };
    if has(cfg::BEST_OF) && (p.best_of == 0 || p.best_of > MAX_BEST_OF) {
        return refuse(CmdReason::INVALID_VALUE, format!("best_of must be 1..={}", MAX_BEST_OF));
    }
    if has(cfg::KIT_RULES) && p.kit_mode > 2 {
        return refuse(CmdReason::INVALID_VALUE, "kit mode must be 0 free, 1 classes, 2 custom");
    }
    let cur = live_config(inner);
    let unsupported = [
        ("mode", has(cfg::MODE) && p.mode != cur.mode),
        ("round_time_limit_s", has(cfg::ROUND_TIME) && p.round_time_limit_s != cur.round_time_limit_s),
        ("team_rule", has(cfg::TEAM_RULE) && p.team_rule != cur.team_rule),
        ("teams", has(cfg::TEAMS) && p.teams != cur.teams),
        ("max_fighters", has(cfg::MAX_FIGHTERS) && p.max_fighters != cur.max_fighters),
        ("max_spectators", has(cfg::MAX_SPECTATORS) && p.max_spectators != cur.max_spectators),
        ("join_in_progress", has(cfg::JIP) && p.join_in_progress != cur.join_in_progress),
    ];
    if let Some((f, _)) = unsupported.iter().find(|(_, bad)| *bad) {
        return refuse(CmdReason::UNSUPPORTED, format!("{} is not configurable yet", f));
    }
    if let Some(a) = arena { set_arena(inner, a, by); }
    if has(cfg::BEST_OF) {
        if inner.best_of != p.best_of { info!(best_of = p.best_of, by = %by, "best-of set"); }
        inner.best_of = p.best_of;
        inner.match_state_dirty = true;
    }
    if has(cfg::KIT_RULES) {
        // Applied by loadout.rs after the lock (it revalidates kits);
        // the snapshot picks the new rules up on the next tick.
        fx.kit_rules = Some((p.kit_mode, p.kit_budget));
    }
    ok("config updated")
}

// ---- no-admin auto start -------------------------------------------------------

/// Seconds between "everyone is ready" and the start when no admin is here.
pub(crate) const AUTO_START_S: u32 = 5;
/// Fewest connected players an auto start needs (a lone player on a public
/// server is not a match; with an admin, START (SOLO) still works).
pub(crate) const AUTO_START_MIN: usize = 2;

fn lobby_chat(inner: &mut Inner, text: String) {
    push_server_chat(inner, &text);
}

/// Every lobby tick: with no admin connected (a dedicated server without
/// configured admins, or a listen server whose host dropped) the players
/// start the match themselves. When at least AUTO_START_MIN players are
/// connected and all of them are READY, a AUTO_START_S countdown runs (shown
/// as the lobby's `phase_deadline_ms` and a chat line); anyone un-readying,
/// joining unready or an admin arriving cancels it. The start is the
/// admin's START (`start_match`), so a server without an admin never sits
/// in the lobby forever.
pub(crate) fn auto_start_step(inner: &mut Inner) {
    if inner.match_state != "lobby" {
        inner.sess.auto_start_at = None;
        return;
    }
    let n = inner.peers.len();
    let ready = inner.peers.values().filter(|p| p.ready).count();
    let eligible = inner.admin_peer_id == 0 && n >= AUTO_START_MIN && ready == n;
    let tick = inner.server_tick;
    match (eligible, inner.sess.auto_start_at) {
        (true, None) => {
            let at = tick.wrapping_add(AUTO_START_S * inner.sess.tick_hz.max(1));
            inner.sess.auto_start_at = Some(at);
            inner.match_state_dirty = true;
            info!(players = n, delay_s = AUTO_START_S, "no admin: everyone is ready, auto start armed");
            events::emit("auto_start", json!({"state": "armed", "players": n, "delay_s": AUTO_START_S}));
            lobby_chat(inner, format!("Everyone is ready: the match starts in {} s", AUTO_START_S));
        }
        (false, Some(_)) => {
            inner.sess.auto_start_at = None;
            inner.match_state_dirty = true;
            info!(players = n, ready, admin = inner.admin_peer_id, "auto start cancelled");
            events::emit("auto_start", json!({"state": "cancelled", "players": n}));
            lobby_chat(inner, if inner.admin_peer_id != 0 { "Auto start cancelled: the host starts the match".into() }
                else { "Auto start cancelled: not everyone is ready".into() });
        }
        (true, Some(at)) if tick.wrapping_sub(at) < u32::MAX / 2 => {
            let r = start_match(inner, false, "auto");
            inner.sess.auto_start_at = None;
            events::emit("auto_start", json!({"state": if r.ok { "started" } else { "refused" }, "players": n,
                "arena": inner.match_arena, "detail": r.text}));
        }
        _ => {}
    }
}

fn set_arena(inner: &mut Inner, arena: String, by: &str) {
    if inner.match_arena != arena {
        info!(arena = %arena, by = %by, "arena picked by host");
        events::emit("arena_picked", json!({"arena": arena, "by": by}));
    }
    inner.match_arena = arena;
    inner.match_state_dirty = true;
}

/// The locked part of a command: dedup by `(player key, cmd_id)`, apply, config rev, phase
/// events, the `cmd_result` event, caching. `cmd.cmd_id == 0` is an RCON / server-internal
/// command: never cached, and the result's text is set even when ok. Returns the result
/// and whether it was applied now (false: a cached duplicate).
pub(crate) fn command_locked(inner: &mut Inner, actor: Actor, cmd: &rec::Command, fx: &mut CmdEffects) -> (rec::CmdResult, bool) {
    let cmd_id = (cmd.cmd_id != 0).then_some(cmd.cmd_id);
    let who = match actor {
        Actor::Peer(a) => inner.peers.get(&a).map(|p| (peer_key(p), p.nick.clone(), p.id)),
        Actor::Rcon => None,
    };
    let cached = match (&who, cmd_id) {
        (Some((k, _, _)), Some(id)) => inner.sess.cached(k, id),
        _ => None,
    };
    if let Some(r) = cached {
        debug!(cmd_id = r.cmd_id, ok = r.ok.get(), "duplicate command: cached result");
        events::emit("cmd_dup", json!({"cmd_id": r.cmd_id, "ok": r.ok.get()}));
        return (r, false);
    }
    let out = apply_command(inner, actor, cmd, fx);
    refresh_config_rev(inner);
    observe_phase(inner);
    let mut r = rec::CmdResult {
        cmd_id: cmd.cmd_id,
        config_rev: inner.sess.config_rev,
        reason_code: out.code,
        ok: Bool::from(out.ok),
        op: cmd.op,
        _r: 0,
        reason_text: Str::new(if out.ok { "" } else { &out.text }),
    };
    let (player, seat, peer) = match &who {
        Some((k, nick, pid)) => (nick.clone(), inner.sess.seats.get(k).copied(), *pid),
        None => (if actor == Actor::Rcon { "rcon".into() } else { String::new() }, None, 0),
    };
    if out.ok {
        info!(cmd = cmd_name(cmd), ?cmd_id, %player, detail = %out.text, "command accepted");
    } else {
        info!(cmd = cmd_name(cmd), ?cmd_id, %player, reason = reason_name(out.code), text = %out.text, "command refused");
    }
    events::emit("cmd_result", json!({
        "player": player, "seat": seat, "peer_id": peer,
        "source": match (actor, cmd_id) { (Actor::Rcon, _) => "rcon", (_, Some(_)) => "command", _ => "internal" },
        "cmd": cmd_name(cmd), "cmd_id": cmd_id, "ok": out.ok,
        "reason": if out.ok { serde_json::Value::Null } else { json!(out.text) },
        "code": reason_name(out.code), "config_rev": inner.sess.config_rev,
    }));
    match (&who, cmd_id) {
        (Some((k, _, _)), Some(_)) => inner.sess.remember(*k, r),
        // RCON / internal get the text even when ok (no wire result).
        _ => r.reason_text = Str::new(&out.text),
    }
    (r, true)
}

/// Run one command end to end: dedup, apply, phase events, the `cmd_result` record to the
/// sender (a peer's command with a cmd_id), the side effects. `cmd.cmd_id == 0` (RCON):
/// no result is sent and nothing is cached.
pub(crate) async fn run_command(socket: &Arc<UdpSocket>, state: &Arc<ServerState>, actor: Actor, cmd: &rec::Command) -> rec::CmdResult {
    let mut fx = CmdEffects::default();
    let (result, fresh) = {
        let mut inner = state.inner.lock().await;
        command_locked(&mut inner, actor, cmd, &mut fx)
    };
    if let (Actor::Peer(a), true) = (actor, cmd.cmd_id != 0) {
        send_msg_to(socket, state, a, cmd_result_msg(&result)).await;
    }
    if fresh {
        for (addr, ban, reason, by) in fx.kicks {
            kick_peer(socket, state, addr, ban, &reason, &by).await;
        }
        if let Some((mode, budget)) = fx.kit_rules {
            crate::loadout::set_rules(socket, state, mode, budget).await;
        }
        if fx.unbanned {
            save_banlist(state).await;
        }
        if fx.admin_changed || fx.unbanned {
            let addrs = { state.inner.lock().await.peers.keys().copied().collect::<Vec<_>>() };
            broadcast_admin_state(socket, state, &addrs).await;
        }
        flush_out(socket, state).await;
    }
    result
}

/// RCON `DEBUG KILL <seat>`: only with `--debug-verbs` (the gate's round driver).
pub(crate) async fn rcon_debug_kill(socket: &Arc<UdpSocket>, state: &Arc<ServerState>, seat: u8) -> Result<String, String> {
    let r = {
        let mut inner = state.inner.lock().await;
        if !inner.sess.debug_verbs {
            return Err("debug verbs are disabled (start hsmp-server with --debug-verbs)".into());
        }
        let r = debug_kill(&mut inner, seat);
        observe_phase(&mut inner);
        r
    };
    events::emit("debug_kill", json!({"seat": seat, "ok": r.is_ok(), "detail": match &r { Ok(s) | Err(s) => s.clone() }}));
    flush_out(socket, state).await;
    r
}

/// RCON `STATUS`: one JSON line (phase, round, config, roster).
pub(crate) async fn rcon_status(state: &Arc<ServerState>) -> String {
    let inner = state.inner.lock().await;
    let now = state.net.now_ms();
    let s = build_session(&inner, now);
    let h = &s.head;
    let roster: Vec<serde_json::Value> = s.rows.iter().map(|r| json!({
        "seat": r.seat, "peer_id": r.peer_id, "nick": r.nick.lossy(), "ready": r.ready.get(), "alive": r.alive.get(),
        "wins": r.wins, "loaded_round": r.loaded_round, "connected": r.connected.get(),
        "admin": r.admin_role >= AdminRole::ADMIN,
        "role": match r.role { Role::FIGHTER => "fighter", Role::SPECTATOR => "spectator", _ => "queued" },
    })).collect();
    json!({
        "epoch": h.epoch, "phase": Phase::name(h.phase), "phase_label": phase_label(h.phase),
        "round": h.round, "match_id": h.match_id, "arena": h.config.arena.lossy(),
        "frozen_arena": h.has_frozen.get().then(|| h.frozen.arena.lossy().into_owned()), "best_of": h.config.best_of,
        "config_rev": h.config.rev, "peers": inner.peers.len(),
        "waiting_on": s.rows.iter().filter(|r| r.waiting.get()).map(|r| r.seat).collect::<Vec<u8>>(),
        "debug_verbs": inner.sess.debug_verbs, "roster": roster,
    }).to_string()
}

#[cfg(test)]
#[path = "session_tests.rs"]
mod tests;
