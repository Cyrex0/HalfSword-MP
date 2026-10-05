//! Sidecar session client (protocol v6 records; docs/development/ipc-shared-memory.md):
//! the client half of the session, the match and the connection.
//!
//! Every server record of this domain (kinds `0x02xx`, `hsmp_ipc::schema::session`) is
//! validated in place and handed to the game AS IT IS:
//! * `session`: ordered by `(epoch, seq)` (stale ones dropped, a new epoch resets every
//!   tracker), copied into the game's `session` slot; the peer directory follows its roster
//!   (`ShmLink::sync_roster`); the in-process accessors ([`now`], [`round`], ...) follow it.
//! * `cmd_result`, `notice`, `kill_feed`, `chat_in`: deduplicated and pushed to the S2G ring.
//! * `admin_state`: copied into the `admin` slot; `pings`: RTTs into the peer directory.
//! * `welcome`, `kicked`, `server_closing`, `pong` and the transport (net.rs) feed the ONE
//!   `link` record (status, link state, my peer id, admin, kick / reject reason, metrics),
//!   published only when it changes.
//!
//! Game -> server: a G2S `command` is framed as it is (`wire::message`) and resent every
//! 250 ms until its result arrives (give up after 5 s); a duplicate cmd_id is not resent
//! once answered. `--events`: `phase`, `session_epoch`, `cmd_sent`, `cmd_result`,
//! `cmd_timeout`, `notice`.

use super::*;
use crate::events;
use hsmp_ipc::record::view;
use hsmp_ipc::schema::session::{self as rs, cmd_op, link_state, phase, sidecar_status};
use serde_json::json;
use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicU8, Ordering};
use std::time::Instant;

/// Resend period / give-up for unanswered commands.
const CMD_RESEND: Duration = Duration::from_millis(250);
const CMD_GIVE_UP: Duration = Duration::from_secs(5);
/// Event-id dedup window (notices, kill feed).
const EVENT_WINDOW: usize = 128;
/// Answered cmd_id window.
const ANSWERED_WINDOW: usize = 256;

struct Pending {
    /// The framed message as first sent (resent byte for byte).
    msg: Vec<u8>,
    op: u8,
    first: Instant,
    last: Instant,
    tries: u32,
}

#[derive(Default)]
struct ClientSess {
    /// Last accepted snapshot (epoch, seq).
    last: Option<(u64, u32)>,
    phase: Option<u8>,
    /// Deathmatch: each peer's life count in the last `mode` record (round, life).
    lives: HashMap<u32, (u32, u16)>,
    pending: HashMap<u32, Pending>,
    /// Recently answered cmd ids (a late duplicate result is not pushed again).
    answered: VecDeque<u32>,
    event_ids: VecDeque<u32>,
}

fn st() -> &'static std::sync::Mutex<ClientSess> {
    static S: std::sync::OnceLock<std::sync::Mutex<ClientSess>> = std::sync::OnceLock::new();
    S.get_or_init(|| std::sync::Mutex::new(ClientSess::default()))
}

fn with<R>(f: impl FnOnce(&mut ClientSess) -> R) -> R {
    let mut g = st().lock().unwrap_or_else(|e| e.into_inner());
    f(&mut g)
}

// ---- the in-process accessors (lock-free; any thread) --------------------------------------

static EPOCH: AtomicU64 = AtomicU64::new(0);
static MATCH_ID: AtomicU64 = AtomicU64::new(0);
static ROUND: AtomicU32 = AtomicU32::new(0);
static PHASE: AtomicU8 = AtomicU8::new(phase::LOBBY);
static MY_PEER: AtomicU32 = AtomicU32::new(0);
static IS_ADMIN: AtomicBool = AtomicBool::new(false);

/// The accepted session as other clients need it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(dead_code)]
pub(crate) struct SessionNow {
    pub epoch: u64,
    pub match_id: u64,
    pub round: u32,
    pub phase: u8,
    pub my_peer_id: u32,
}

#[allow(dead_code)]
pub(crate) fn now() -> SessionNow {
    SessionNow {
        epoch: EPOCH.load(Ordering::Acquire),
        match_id: MATCH_ID.load(Ordering::Acquire),
        round: ROUND.load(Ordering::Acquire),
        phase: PHASE.load(Ordering::Acquire),
        my_peer_id: MY_PEER.load(Ordering::Acquire),
    }
}

/// The current round of the last accepted snapshot (the server's match round).
#[cfg(test)]
pub(crate) fn round() -> u32 {
    ROUND.load(Ordering::Acquire)
}

/// Our peer id (0 before the first Welcome).
pub(crate) fn my_peer_id() -> u32 {
    MY_PEER.load(Ordering::Acquire)
}

/// Is this player admin (the server's `admin_state`)?
#[allow(dead_code)]
pub(crate) fn is_admin() -> bool {
    IS_ADMIN.load(Ordering::Acquire)
}

// ---- the link record (one owner) -------------------------------------------------------------

struct LinkPub {
    cur: rs::Link,
    last: Option<rs::Link>,
}

fn link_pub() -> &'static std::sync::Mutex<LinkPub> {
    static L: std::sync::OnceLock<std::sync::Mutex<LinkPub>> = std::sync::OnceLock::new();
    L.get_or_init(|| {
        std::sync::Mutex::new(LinkPub {
            cur: rs::Link {
                status: sidecar_status::CONNECTING,
                state: link_state::CONNECTING,
                loss_pct_10s: -1.0,
                client_protocol: proto::PROTOCOL_VERSION as u16,
                ..Default::default()
            },
            last: None,
        })
    })
}

/// The link record without the values that only describe "now" (ages, countdowns): a change
/// of those alone is not a reason to publish.
fn link_key(l: &rs::Link) -> rs::Link {
    rs::Link { rx_age_ms: 0, next_retry_ms: 0, down_ms: 0, ..*l }
}

/// Update the link record; it is published (slot `link`) only when it changed.
pub(super) fn link_update(f: impl FnOnce(&mut rs::Link)) -> bool {
    let publish = {
        let mut g = link_pub().lock().unwrap_or_else(|e| e.into_inner());
        f(&mut g.cur);
        let changed = g.last.map_or(true, |p| link_key(&p) != link_key(&g.cur));
        if changed {
            g.last = Some(g.cur);
            Some(g.cur)
        } else {
            None
        }
    };
    match publish {
        Some(l) => {
            LINK_PUBLISHES.fetch_add(1, Ordering::Relaxed);
            if let Some(sl) = ipc_shm::link() {
                sl.post_record("link", None, rs::K_LINK, hsmp_ipc::bytemuck::bytes_of(&l));
            }
            true
        }
        None => false,
    }
}

/// How many times the link record was published (tests, measurements).
pub(super) static LINK_PUBLISHES: AtomicU64 = AtomicU64::new(0);

/// The current link record (tests, logs).
#[allow(dead_code)]
pub(super) fn link_now() -> rs::Link {
    link_pub().lock().unwrap_or_else(|e| e.into_inner()).cur
}

/// The terminal link record written at exit (career saves restored first): status ENDED.
pub(super) fn final_link() -> Vec<u8> {
    let l = rs::Link {
        status: sidecar_status::ENDED,
        state: link_state::TERMINAL,
        my_peer_id: 0,
        is_admin: false.into(),
        ..link_now()
    };
    hsmp_ipc::bytemuck::bytes_of(&l).to_vec()
}

/// The `sidecar_status` code of a status name.
pub(super) fn status_code(s: &str) -> u8 {
    match s {
        "connected" => sidecar_status::CONNECTED,
        "reconnecting" => sidecar_status::RECONNECTING,
        "rejected" => sidecar_status::REJECTED,
        "kicked" => sidecar_status::KICKED,
        "replaced" => sidecar_status::REPLACED,
        "server_closed" => sidecar_status::SERVER_CLOSED,
        "ended" => sidecar_status::ENDED,
        _ => sidecar_status::CONNECTING,
    }
}

/// The sidecar status changed (`SharedState.status` and the link record).
pub(super) async fn set_status(shared: &Arc<Mutex<SharedState>>, status: &'static str) {
    shared.lock().await.status = status;
    link_update(|l| l.status = status_code(status));
}

/// Kick / reject / server-closed reason into the link record.
pub(super) fn set_reason(reason: &str, code: u8, retry_after_s: u32) {
    link_update(|l| {
        l.reason = hsmp_ipc::layout::Str::new(reason);
        l.reason_code = code;
        l.retry_after_s = retry_after_s;
    });
}

// ---- welcome -----------------------------------------------------------------------------------

/// The server epoch of the last Welcome (a new epoch = server restart).
static SERVER_EPOCH: AtomicU64 = AtomicU64::new(0);

/// Is this Welcome a session resume? The same server instance (epoch) handed back
/// the peer id we already had. A server restart, a first join or an old server that assigns
/// a new id on every re-join are not.
pub(super) fn welcome_is_resume(prev_epoch: u64, epoch: u64, my_id: u32, new_id: u32) -> bool {
    prev_epoch != 0 && prev_epoch == epoch && my_id != 0 && my_id == new_id
}

/// A fresh session (first join, reconnect with a new id, a restarted server): event ids,
/// per-peer dedup and the peer directory restart.
fn reset_tracking() {
    with(|c| c.event_ids.clear());
    super::handlers::reset_session_dedup();
}

/// `welcome`.
pub(super) async fn on_welcome(payload: &[u8], shared: &Arc<Mutex<SharedState>>) -> Result<()> {
    let w = view::<rs::Welcome>(payload).map_err(|e| anyhow::anyhow!("welcome: {e}"))?.head();
    info!(peer_id = w.peer_id, nick = %w.nick.lossy(), epoch = %format!("{:016x}", w.server_epoch), "welcome from relay");
    let prev_epoch = SERVER_EPOCH.swap(w.server_epoch, Ordering::Relaxed);
    if prev_epoch != w.server_epoch {
        info!("new server epoch: match tracking reset");
    }
    let resume = {
        let mut s = shared.lock().await;
        let resume = welcome_is_resume(prev_epoch, w.server_epoch, s.my_peer_id, w.peer_id);
        s.my_peer_id = w.peer_id;
        s.status = "connected";
        // The admin flag follows in admin_state right after.
        s.is_admin = false;
        resume
    };
    if resume {
        info!(peer_id = w.peer_id, "session resumed (same server, same peer id): stand-ins and tracking kept");
        events::emit("session_resumed", json!({"peer_id": w.peer_id}));
    }
    MY_PEER.store(w.peer_id, Ordering::Release);
    IS_ADMIN.store(false, Ordering::Release);
    // Server mods: hold the session back from the game until they are loaded (before any
    // later record of this connection is handled).
    super::mods_client::on_welcome(w.server_epoch, w.caps);
    link_update(|l| {
        l.my_peer_id = w.peer_id;
        l.server_epoch = w.server_epoch;
        l.status = sidecar_status::CONNECTED;
        l.is_admin = false.into();
        l.reason = Default::default();
        l.reason_code = 0;
        l.retry_after_s = 0;
    });
    world_client::on_welcome(); // a reconnect re-syncs the world state
    if !resume {
        reset_tracking();
        // New session: peer ids may be reassigned; never blend old bodies.
        with_play(|m| m.clear());
        if let Some(l) = ipc_shm::link() {
            l.reset_peers();
        }
    }
    // HSMP-SHM: every Welcome starts a new session epoch (session-scoped slots).
    if let Some(l) = ipc_shm::link() {
        l.on_welcome();
        // A server without game modes never sends `mode` / `zone`: clear what an earlier
        // server left in the slots (seq 0 / radius 0 = none).
        if w.caps & hsmp_net::net::caps::MODES == 0 {
            l.post_record("mode", None, rs::K_MODE, &hsmp_ipc::record::to_payload(&rs::ModeHead::default(), &[]));
        }
        if w.caps & hsmp_net::net::caps::ZONE == 0 {
            l.post_record("zone", None, rs::K_ZONE, &hsmp_ipc::record::to_payload(&rs::ZoneState::default(), &[]));
        }
    }
    Ok(())
}

/// `mode` / `zone` (game modes): validated, into the `mode` / `zone` slot as they are.
pub(super) fn on_mode(kind: u16, payload: &[u8]) -> Result<()> {
    let slot = if kind == rs::K_MODE {
        let v = view::<rs::ModeHead>(payload).map_err(|e| anyhow::anyhow!("mode: {e}"))?;
        debug!(mode = v.head.mode, round = v.head.round, rows = v.rows.len(), "mode state");
        // A respawned peer (its life count went up in this round) may die again in the
        // same round: its next `death` record must not be swallowed as a resend.
        let round = v.head.round;
        let respawned: Vec<u32> = with(|c| {
            v.rows.iter().filter(|r| r.peer_id != 0).filter_map(|r| {
                let prev = c.lives.insert(r.peer_id, (round, r.life));
                matches!(prev, Some((pr, pl)) if pr == round && r.life > pl).then_some(r.peer_id)
            }).collect()
        });
        for p in respawned {
            info!(peer_id = p, round, "peer respawned: its next death in this round is news");
            combat_client::forget_death(p, round);
        }
        "mode"
    } else {
        view::<rs::ZoneState>(payload).map_err(|e| anyhow::anyhow!("zone: {e}"))?;
        "zone"
    };
    if let Some(l) = ipc_shm::link() {
        l.post_record(slot, None, kind, payload);
    }
    Ok(())
}

// ---- snapshot ordering ------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Order {
    /// Newer snapshot of the same server instance.
    Newer,
    /// Older or duplicate: drop.
    Stale,
    /// First snapshot, or a server restart (new epoch): reset all tracking.
    NewEpoch,
}

/// The receiver rule of the protocol: drop `(epoch, seq) <= last`; a new epoch resets
/// everything.
pub(super) fn order(last: Option<(u64, u32)>, epoch: u64, seq: u32) -> Order {
    match last {
        None => Order::NewEpoch,
        Some((e, _)) if e != epoch => Order::NewEpoch,
        Some((_, s)) if seq > s => Order::Newer,
        _ => Order::Stale,
    }
}

/// `session`: ordered, then copied into the game's `session` slot as it is.
pub(super) fn on_session(payload: &[u8]) -> Result<()> {
    let v = view::<rs::SessionHead>(payload).map_err(|e| anyhow::anyhow!("session: {e}"))?;
    let h = v.head();
    let (ord, old_phase) = with(|c| {
        let o = order(c.last, h.epoch, h.seq);
        if o != Order::Stale {
            c.last = Some((h.epoch, h.seq));
        }
        if o == Order::NewEpoch {
            c.phase = None;
        }
        (o, c.phase)
    });
    match ord {
        Order::Stale => {
            debug!(epoch = h.epoch, seq = h.seq, "stale session snapshot dropped");
            return Ok(());
        }
        Order::NewEpoch => {
            info!(epoch = h.epoch, seq = h.seq, "session: new server epoch; tracking reset");
            reset_tracking();
            if let Some(l) = ipc_shm::link() {
                l.reset_peers();
            }
            events::emit("session_epoch", json!({"epoch": h.epoch, "seq": h.seq}));
        }
        Order::Newer => {}
    }
    EPOCH.store(h.epoch, Ordering::Release);
    MATCH_ID.store(h.match_id, Ordering::Release);
    ROUND.store(h.round, Ordering::Release);
    PHASE.store(h.phase, Ordering::Release);
    // Deaths are deduped per (epoch, match, peer, round).
    combat_client::set_match(h.epoch, h.match_id);
    if super::mods_client::holds_session() {
        // The server's mods are not loaded yet: the game stays out of the session (it would
        // travel into a running match); the newest snapshot goes in when they are.
        *held_session() = Some(payload.to_vec());
    } else {
        post_session(payload, &v);
    }
    if old_phase != Some(h.phase) {
        with(|c| c.phase = Some(h.phase));
        let from = old_phase.map(rs::phase_name);
        let arena = h.arena().lossy().into_owned();
        info!(from = ?from, to = rs::phase_name(h.phase), round = h.round, match_id = h.match_id, "session phase");
        events::emit("phase", json!({
            "from": from, "to": rs::phase_name(h.phase), "match_id": h.match_id,
            "round": h.pending_round(), "epoch": h.epoch, "arena": arena,
        }));
    }
    Ok(())
}

/// The newest snapshot held back while the server's mods load (mods_client.rs).
fn held_session() -> std::sync::MutexGuard<'static, Option<Vec<u8>>> {
    static H: std::sync::OnceLock<std::sync::Mutex<Option<Vec<u8>>>> = std::sync::OnceLock::new();
    H.get_or_init(Default::default).lock().unwrap_or_else(|e| e.into_inner())
}

/// The snapshot into the game's `session` slot and its roster into the peer directory.
fn post_session(payload: &[u8], v: &hsmp_ipc::record::View<'_, rs::SessionHead>) {
    if let Some(l) = ipc_shm::link() {
        l.post_record("session", None, rs::K_SESSION, payload);
        let me = my_peer_id();
        let peers: Vec<(u32, String)> = v
            .rows
            .iter()
            .filter(|r| r.connected.get() && r.peer_id != 0 && r.peer_id != me)
            .map(|r| (r.peer_id, r.nick.lossy().into_owned()))
            .collect();
        l.sync_roster(&peers);
    }
}

/// The server's mods are loaded (or it has none): the held snapshot reaches the game.
pub(super) fn release_held_session() {
    let Some(p) = held_session().take() else { return };
    if let Ok(v) = view::<rs::SessionHead>(&p) {
        info!("server mods loaded: the session reaches the game");
        post_session(&p, &v);
    }
}

/// `pings`: every connected player's RTT into the peer directory.
pub(super) fn on_pings(payload: &[u8]) -> Result<()> {
    let v = view::<rs::PingsHead>(payload).map_err(|e| anyhow::anyhow!("pings: {e}"))?;
    let rtts: Vec<(u32, u32)> = v.rows.iter().filter(|r| r.peer_id != 0).map(|r| (r.peer_id, r.rtt_ms as u32)).collect();
    if let Some(l) = ipc_shm::link() {
        l.set_rtts(&rtts);
    }
    Ok(())
}

// ---- events -------------------------------------------------------------------------------------

/// A fresh event id (true) or a duplicate (false).
fn fresh_event(id: u32) -> bool {
    with(|c| {
        if c.event_ids.contains(&id) {
            return false;
        }
        c.event_ids.push_back(id);
        while c.event_ids.len() > EVENT_WINDOW {
            c.event_ids.pop_front();
        }
        true
    })
}

fn push(kind: u16, payload: &[u8]) {
    if let Some(l) = ipc_shm::link() {
        l.push_record(kind, 0, 0, payload);
    }
}

/// `notice` / `kill_feed`: once per event id, to the game as they are.
pub(super) fn on_event(kind: u16, payload: &[u8]) -> Result<()> {
    let id = match kind {
        rs::K_NOTICE => view::<rs::Notice>(payload).map(|v| v.head.event_id),
        _ => view::<rs::KillFeed>(payload).map(|v| v.head.event_id),
    }
    .map_err(|e| anyhow::anyhow!("event {kind:#x}: {e}"))?;
    if !fresh_event(id) {
        return Ok(());
    }
    let j = hsmp_ipc::debug_json::record_to_json(kind, payload);
    info!(notice = %j, "server notice");
    events::emit("notice", j);
    push(kind, payload);
    Ok(())
}

/// `chat_in`: to the game as it is.
pub(super) fn on_chat_in(payload: &[u8]) -> Result<()> {
    let v = view::<rs::ChatIn>(payload).map_err(|e| anyhow::anyhow!("chat_in: {e}"))?;
    info!(from_peer_id = v.head.from_peer, nick = %v.head.nick.lossy(), len = v.head.text.len(), "chat recv");
    push(rs::K_CHAT_IN, payload);
    Ok(())
}

/// `admin_state`: the `admin` slot as it is; the admin flag.
pub(super) async fn on_admin_state(payload: &[u8], shared: &Arc<Mutex<SharedState>>) -> Result<()> {
    let v = view::<rs::AdminStateHead>(payload).map_err(|e| anyhow::anyhow!("admin_state: {e}"))?;
    if let Some(l) = ipc_shm::link() {
        l.post_record("admin", None, rs::K_ADMIN_STATE, payload);
    }
    let me = my_peer_id();
    let admin = me != 0 && v.head.admin_peer == me;
    IS_ADMIN.store(admin, Ordering::Release);
    shared.lock().await.is_admin = admin;
    link_update(|l| l.is_admin = admin.into());
    Ok(())
}

/// `kicked`: terminal (no automatic rejoin).
pub(super) async fn on_kicked(payload: &[u8], shared: &Arc<Mutex<SharedState>>) -> Result<()> {
    let k = view::<rs::Kicked>(payload).map_err(|e| anyhow::anyhow!("kicked: {e}"))?.head();
    warn!(code = k.code, reason = %k.reason.lossy(), retry_after_s = k.retry_after_s, "kicked by the server");
    set_terminal();
    set_reason(&k.reason.lossy(), k.code, k.retry_after_s);
    link_update(|l| l.state = link_state::TERMINAL);
    set_status(shared, "kicked").await;
    Ok(())
}

/// `server_closing`: terminal unless it announced a restart.
pub(super) async fn on_server_closing(payload: &[u8], shared: &Arc<Mutex<SharedState>>) -> Result<()> {
    let c = view::<rs::ServerClosing>(payload).map_err(|e| anyhow::anyhow!("server_closing: {e}"))?.head();
    warn!(reason = c.reason, text = %c.text.lossy(), reconnect_after_ms = c.reconnect_after_ms, "server closing");
    super::net::on_server_closing(c.reconnect_after_ms);
    if c.reconnect_after_ms == 0 {
        // HSMPMatch shows the reason ("Host closed the server").
        set_reason(&c.text.lossy(), c.reason, 0);
        set_status(shared, "server_closed").await;
    }
    Ok(())
}

// ---- metrics (pong) ------------------------------------------------------------------------------

/// Window of the windowed loss figure.
pub(super) const LOSS_WINDOW_MS: u64 = 10_000;

/// Sliding window over the connection's (sent, lost) counters.
#[derive(Default)]
pub(super) struct LossWindow {
    s: VecDeque<(u64, u64, u64)>,
}

impl LossWindow {
    /// Add a sample (ms, packets sent, packets lost); returns the loss % over the last
    /// LOSS_WINDOW_MS, None until a second sample exists. A counter that went back (new
    /// connection) restarts the window.
    pub(super) fn push(&mut self, now_ms: u64, sent: u64, lost: u64) -> Option<f64> {
        if self.s.back().map_or(false, |b| sent < b.1 || lost < b.2) {
            self.s.clear();
        }
        self.s.push_back((now_ms, sent, lost));
        while self.s.len() > 2 && self.s.get(1).map_or(false, |x| now_ms.saturating_sub(x.0) >= LOSS_WINDOW_MS) {
            self.s.pop_front();
        }
        let (b, n) = (self.s.front()?, self.s.back()?);
        if self.s.len() < 2 {
            return None;
        }
        let ds = n.1 - b.1;
        Some(if ds == 0 { 0.0 } else { (n.2 - b.2) as f64 * 100.0 / ds as f64 })
    }
}

fn loss_window() -> &'static std::sync::Mutex<LossWindow> {
    static W: std::sync::OnceLock<std::sync::Mutex<LossWindow>> = std::sync::OnceLock::new();
    W.get_or_init(Default::default)
}

/// The metrics of one pong into a link record (HSMPHud's net chip: rtt, loss, jitter, rx age).
pub(super) fn apply_metrics(l: &mut rs::Link, rtt_ms: u64, clock_offset_ms: i64, now_ms: u64,
                            st: &hsmp_net::net::ConnStats, rx_age_ms: u64, loss_10s: Option<f64>) {
    l.rtt_ms = rtt_ms as f32;
    l.clock_offset_ms = clock_offset_ms as f32;
    l.metrics_wall_ms = now_ms;
    l.loss_pct = if st.pkts_sent == 0 { 0.0 } else { (st.pkts_lost as f64 * 100.0 / st.pkts_sent as f64) as f32 };
    l.loss_pct_10s = loss_10s.map_or(-1.0, |x| x as f32);
    l.jitter_ms = finite(st.rttvar_ms as f32);
    l.srtt_ms = finite(st.srtt_ms as f32);
    l.rto_ms = st.rto_ms.min(u32::MAX as u64) as u32;
    l.pkts_lost = st.pkts_lost.min(u32::MAX as u64) as u32;
    l.retransmits = st.retransmits.min(u32::MAX as u64) as u32;
    l.aead_failed = st.auth_failed.min(u32::MAX as u64) as u32;
    l.rx_age_ms = rx_age_ms.min(u32::MAX as u64) as u32;
}

fn finite(x: f32) -> f32 {
    if x.is_finite() { x } else { 0.0 }
}

/// `pong`: RTT and server clock offset of our ping; transport metrics with it.
pub(super) fn on_pong(payload: &[u8]) -> Result<()> {
    let p = view::<rs::Pong>(payload).map_err(|e| anyhow::anyhow!("pong: {e}"))?.head();
    let now_ms = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0);
    let rtt_ms = now_ms.saturating_sub(p.client_time_ms);
    // Server clock offset estimate: midpoint of the trip.
    let clock_offset_ms = p.server_time_ms as i64 - (p.client_time_ms as i64 + rtt_ms as i64 / 2);
    let st = conn_stats().unwrap_or_default();
    let w = loss_window().lock().unwrap_or_else(|e| e.into_inner()).push(now_ms, st.pkts_sent, st.pkts_lost);
    let rx = rx_age_ms();
    link_update(|l| apply_metrics(l, rtt_ms, clock_offset_ms, now_ms, &st, rx, w));
    Ok(())
}

// ---- commands -------------------------------------------------------------------------------------

fn op_name(op: u8) -> &'static str {
    match op {
        cmd_op::READY => "ready",
        cmd_op::START => "start",
        cmd_op::ABORT => "abort",
        cmd_op::PICK_ARENA => "pick_arena",
        cmd_op::SET_CONFIG => "set_config",
        cmd_op::KICK => "kick",
        cmd_op::BAN => "ban",
        cmd_op::PROMOTE => "promote",
        cmd_op::VOTE => "vote",
        cmd_op::SWITCH_ROLE => "switch_role",
        cmd_op::SET_TEAM => "set_team",
        cmd_op::UNBAN => "unban",
        cmd_op::RESET_MATCH => "reset_match",
        _ => "?",
    }
}

fn send(msg: Vec<u8>) {
    if let Some(l) = ipc_shm::link() {
        l.send_record(msg);
    }
}

/// A G2S `command` (validated by the ipc thread): framed as it is, then resent until its
/// result arrives. Returns the message to send now; `None` for a cmd_id already answered
/// (never sent again) or bytes that are not a command. Never blocks.
pub(super) fn submit(payload: &[u8]) -> Option<Vec<u8>> {
    let Ok(v) = view::<rs::Command>(payload) else { return None };
    let (cmd_id, op) = (v.head.cmd_id, v.head.op);
    let now = Instant::now();
    let send_now = with(|s| {
        if s.answered.contains(&cmd_id) {
            return None;
        }
        let p = s.pending.entry(cmd_id).or_insert_with(|| Pending {
            msg: hsmp_ipc::wire::message(rs::K_COMMAND, 0, 0, payload),
            op, first: now, last: now, tries: 0,
        });
        p.tries += 1;
        p.last = now;
        Some((p.msg.clone(), p.tries))
    });
    let Some((msg, tries)) = send_now else {
        debug!(cmd_id, "command already answered; not resent");
        return None;
    };
    if tries == 1 {
        info!(cmd_id, kind = op_name(op), "command sent");
        events::emit("cmd_sent", json!({"cmd": op_name(op), "cmd_id": cmd_id}));
    }
    Some(msg)
}

/// `cmd_result`: one per cmd_id to the game, as it is.
pub(super) fn on_cmd_result(payload: &[u8]) -> Result<()> {
    let r = view::<rs::CmdResult>(payload).map_err(|e| anyhow::anyhow!("cmd_result: {e}"))?.head();
    let (op, fresh, ms) = with(|c| {
        let p = c.pending.remove(&r.cmd_id);
        let seen = c.answered.contains(&r.cmd_id);
        if !seen {
            c.answered.push_back(r.cmd_id);
            while c.answered.len() > ANSWERED_WINDOW {
                c.answered.pop_front();
            }
        }
        let ms = p.as_ref().map(|p| p.first.elapsed().as_millis() as u64);
        (p.map(|p| p.op).unwrap_or(r.op), !seen, ms)
    });
    if !fresh {
        debug!(cmd_id = r.cmd_id, "duplicate command result ignored");
        return Ok(());
    }
    push(rs::K_CMD_RESULT, payload);
    let ok = r.ok.get();
    info!(cmd_id = r.cmd_id, ok, reason = %r.reason_text.lossy(), "command result");
    let code = hsmp_ipc::schema::enum_by_name("cmd_reason").and_then(|e| e.name_of(r.reason_code as u32)).unwrap_or("UNKNOWN");
    events::emit("cmd_result", json!({
        "cmd": op_name(op), "cmd_id": r.cmd_id, "ok": ok,
        "reason": if ok { serde_json::Value::Null } else { json!(r.reason_text.lossy()) },
        "code": code, "config_rev": r.config_rev, "ms": ms,
    }));
    Ok(())
}

/// The resend loop (detached).
pub(super) fn spawn_tasks() {
    tokio::spawn(async move {
        let mut t = time::interval(Duration::from_millis(50));
        loop {
            t.tick().await;
            let now = Instant::now();
            let (resend, gave_up) = with(|s| {
                let mut resend = Vec::new();
                let mut gave_up = Vec::new();
                s.pending.retain(|id, p| {
                    if now.duration_since(p.first) >= CMD_GIVE_UP {
                        gave_up.push((*id, p.op, p.tries));
                        return false;
                    }
                    if now.duration_since(p.last) >= CMD_RESEND {
                        p.last = now;
                        p.tries += 1;
                        resend.push(p.msg.clone());
                    }
                    true
                });
                (resend, gave_up)
            });
            for (id, op, tries) in gave_up {
                warn!(cmd_id = id, kind = op_name(op), tries, "command unanswered after 5 s; giving up");
                events::emit("cmd_timeout", json!({"cmd": op_name(op), "cmd_id": id, "tries": tries}));
            }
            for m in resend {
                send(m);
            }
        }
    });
}

// ---- G2S records that go to the server as they are -------------------------------------------------

/// `game_status`, `spawned`, `chat`: framed as they are.
pub(super) fn frame(kind: u16, payload: &[u8]) -> Vec<u8> {
    hsmp_ipc::wire::message(kind, 0, 0, payload)
}

static LEAVE_REASON: AtomicU8 = AtomicU8::new(0);

fn leave_notify() -> &'static tokio::sync::Notify {
    static N: std::sync::OnceLock<tokio::sync::Notify> = std::sync::OnceLock::new();
    N.get_or_init(tokio::sync::Notify::new)
}

/// G2S `leave`: the game asked us to leave the server (BACK TO MENU / LEAVE MATCH).
pub(super) fn request_leave(payload: &[u8]) {
    let reason = view::<rs::Leave>(payload).map(|v| v.head.reason).unwrap_or(0);
    LEAVE_REASON.store(reason, Ordering::Release);
    info!(reason, "leave requested by the game; leaving the server");
    crate::events::emit("leave_request", json!({"reason": reason}));
    leave_notify().notify_one();
}

/// Resolves when the game asked to leave; returns its `leave_reason` code.
pub(super) async fn leave_requested() -> u8 {
    leave_notify().notified().await;
    LEAVE_REASON.load(Ordering::Acquire)
}

#[cfg(test)]
mod tests {
    use super::*;
    use hsmp_ipc::layout::Str;
    use hsmp_ipc::record::to_payload;

    /// Deathmatch: the `mode` record shows a peer's life count going up in the round, so
    /// its next death record in that round reaches the game (not swallowed as a resend).
    #[test]
    fn a_respawn_reopens_the_peers_death_dedup() {
        let e = 0xD1E7_0000 + std::process::id() as u64;
        let mode = |life: u16| to_payload(&rs::ModeHead { round: 4, seq: 1, ..Default::default() },
                                          &[rs::ModeRow { peer_id: 88_001, life, seat: 1, ..Default::default() }]);
        on_mode(rs::K_MODE, &mode(1)).unwrap();
        assert!(crate::combat_client::death_is_new(e, 7, 88_001, 4));
        on_mode(rs::K_MODE, &mode(1)).unwrap();
        assert!(!crate::combat_client::death_is_new(e, 7, 88_001, 4), "same life: a resend");
        on_mode(rs::K_MODE, &mode(2)).unwrap();
        assert!(crate::combat_client::death_is_new(e, 7, 88_001, 4), "the next life's death");
    }

    #[test]
    fn snapshot_order_rule() {
        assert_eq!(order(None, 5, 1), Order::NewEpoch);
        assert_eq!(order(Some((5, 3)), 5, 4), Order::Newer);
        assert_eq!(order(Some((5, 3)), 5, 3), Order::Stale, "duplicate");
        assert_eq!(order(Some((5, 3)), 5, 2), Order::Stale, "reordered");
        assert_eq!(order(Some((5, 900)), 6, 1), Order::NewEpoch, "restart: low seq accepted");
    }

    /// Replay: reordered, duplicated and epoch-changed snapshots; the accepted sequence only
    /// moves forward within an epoch.
    #[test]
    fn replay_only_moves_forward() {
        let feed = [(1u64, 1u32), (1, 3), (1, 2), (1, 3), (1, 4), (2, 1), (1, 5), (2, 2), (2, 2), (2, 7)];
        let mut last = None;
        let mut accepted = vec![];
        for (e, s) in feed {
            if order(last, e, s) != Order::Stale {
                last = Some((e, s));
                accepted.push((e, s));
            }
        }
        assert_eq!(accepted, vec![(1, 1), (1, 3), (1, 4), (2, 1), (1, 5), (2, 2), (2, 7)]);
    }

    /// Resume = a Welcome for the same epoch: snapshots keep moving forward (no reset, no
    /// drop); a new epoch (server restart) resets tracking.
    #[test]
    fn reconnect_same_epoch_keeps_ordering_restart_resets() {
        let last = Some((77u64, 40u32));
        assert_eq!(order(last, 77, 41), Order::Newer, "after a resume the server's seq continues");
        assert_eq!(order(last, 77, 40), Order::Stale, "a resend from before the outage is dropped");
        assert_eq!(order(last, 78, 1), Order::NewEpoch, "server restart");
    }

    /// Only a same-epoch, same-id Welcome is a resume.
    #[test]
    fn welcome_resume_rule() {
        assert!(welcome_is_resume(7, 7, 3, 3));
        assert!(!welcome_is_resume(7, 7, 3, 4), "new id: a fresh session");
        assert!(!welcome_is_resume(7, 8, 3, 3), "server restarted");
        assert!(!welcome_is_resume(0, 7, 0, 1), "first join");
        assert!(!welcome_is_resume(7, 7, 0, 1), "never had an id");
    }

    /// The HUD's net indicator fields; the windowed loss figure.
    #[test]
    fn metrics_fill_the_link_record() {
        let st = hsmp_net::net::ConnStats { pkts_sent: 200, pkts_lost: 3, rttvar_ms: 4.5, srtt_ms: 40.0, ..Default::default() };
        let mut l = rs::Link::default();
        apply_metrics(&mut l, 42, -3, 1000, &st, 17, None);
        assert_eq!((l.rtt_ms, l.clock_offset_ms, l.loss_pct, l.jitter_ms, l.rx_age_ms, l.loss_pct_10s), (42.0, -3.0, 1.5, 4.5, 17, -1.0));
        assert_eq!(l.metrics_wall_ms, 1000);
        apply_metrics(&mut l, 1, 0, 2000, &hsmp_net::net::ConnStats::default(), 0, Some(12.5));
        assert_eq!((l.loss_pct, l.loss_pct_10s), (0.0, 12.5));
        assert!(view::<rs::Link>(hsmp_ipc::bytemuck::bytes_of(&l)).is_ok());
    }

    #[test]
    fn loss_window_is_the_last_ten_seconds() {
        let mut w = LossWindow::default();
        assert_eq!(w.push(0, 100_000, 100), None);
        assert_eq!(w.push(8_000, 110_000, 100), Some(0.0));
        let x = w.push(10_000, 112_000, 500).unwrap();
        assert!((x - 400.0 * 100.0 / 12_000.0).abs() < 1e-6, "{x}");
        w.push(16_000, 118_000, 500);
        assert_eq!(w.push(22_000, 124_000, 500), Some(0.0));
        assert_eq!(w.push(23_000, 10, 0), None, "a new connection restarts the window");
    }

    /// The link record is published on a change of its content only; ages alone do not count.
    #[test]
    fn link_publishes_on_change_only() {
        // (Other tests update the process-wide record in parallel: check the rule itself.)
        let a = rs::Link { attempt: 3, rx_age_ms: 100, down_ms: 7, next_retry_ms: 900, ..Default::default() };
        let b = rs::Link { rx_age_ms: 2600, down_ms: 2500, next_retry_ms: 0, ..a };
        assert_eq!(link_key(&a), link_key(&b), "ages alone never republish");
        assert_ne!(link_key(&a), link_key(&rs::Link { attempt: 4, ..a }));
        assert_ne!(link_key(&a), link_key(&rs::Link { state: link_state::STALLED, ..a }));
        let n0 = LINK_PUBLISHES.load(Ordering::Relaxed);
        link_update(|l| l.attempt = l.attempt.wrapping_add(1));
        assert!(LINK_PUBLISHES.load(Ordering::Relaxed) > n0, "a real change publishes");
        let f = final_link();
        let v = view::<rs::Link>(&f).unwrap();
        assert_eq!((v.head.status, v.head.state, v.head.my_peer_id), (sidecar_status::ENDED, link_state::TERMINAL, 0));
        assert_eq!(status_code("server_closed"), sidecar_status::SERVER_CLOSED);
        assert_eq!(status_code("whatever"), sidecar_status::CONNECTING);
    }

    /// Measurement (`cargo test -p hsmp-server --bin hsmp-sidecar session_snapshot_cost --
    /// --ignored --nocapture`): sidecar work per session snapshot: validate + copy into the
    /// slot.
    #[test]
    #[ignore]
    fn session_snapshot_cost() {
        use hsmp_ipc::schema::RawSlot;
        let slot = hsmp_ipc::seqlock::SeqSlot::<hsmp_ipc::schema::Stamped<rs::SessionBuf>>::new_boxed();
        let mut scratch = Vec::new();
        for n in [2usize, 8, 16] {
            let rows: Vec<rs::RosterRow> = (0..n).map(|i| rs::RosterRow {
                peer_id: 10 + i as u32, seat: i as u8 + 1, wins: 1, spawn_id: 513, spawn_pos: [1.0, 2.0, 3.0],
                connected: true.into(), alive: true.into(), nick: Str::new("Willie"), ..Default::default()
            }).collect();
            let p = to_payload(&rs::SessionHead { epoch: 1, seq: 1, ..Default::default() }, &rows);
            let iters = 20_000;
            let t = Instant::now();
            for _ in 0..iters {
                let v = view::<rs::SessionHead>(&p).unwrap();
                std::hint::black_box(&v);
                slot.put(hsmp_ipc::schema::SlotMeta::default(), rs::K_SESSION, &p, &mut scratch);
            }
            let new_us = t.elapsed().as_secs_f64() * 1e6 / iters as f64;
            println!("session snapshot, {n:2} players: {new_us:6.2} us ({} B record)", p.len());
        }
    }

    /// Commands: the same bytes are resent; an answered cmd_id is never sent again.
    #[test]
    fn command_dedup_by_cmd_id() {
        let c = rs::Command { text: Str::new("Map_Arena_Pit"), ..rs::Command::new(990_001, cmd_op::PICK_ARENA) };
        let p = to_payload(&c, &[]);
        let sent = submit(&p).unwrap();
        let first = with(|s| s.pending.get(&990_001).map(|p| (p.msg.clone(), p.tries))).unwrap();
        assert_eq!(sent, first.0);
        assert_eq!(&first.0[hsmp_ipc::wire::HDR..], &p[..], "framed as it is");
        assert_eq!(hsmp_ipc::wire::kind_of(&first.0), rs::K_COMMAND);
        assert_eq!(submit(&p), Some(first.0.clone()), "a duplicate before the result is resent, byte for byte");
        let r = rs::CmdResult { cmd_id: 990_001, ok: true.into(), op: cmd_op::PICK_ARENA, ..Default::default() };
        on_cmd_result(&to_payload(&r, &[])).unwrap();
        assert!(with(|s| !s.pending.contains_key(&990_001)));
        assert!(submit(&p).is_none(), "answered: never sent again");
        assert!(submit(&[1, 2, 3]).is_none(), "not a command");
    }
}
