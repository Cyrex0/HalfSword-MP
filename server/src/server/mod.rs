//! Server state machine, recv loop, and tick loop.
//!
//! Match flow:
//!   lobby → (all ready + start) → countdown(3s) → live → (someone dies) →
//!   roundover(3s) → either match_over(best-of hit) or countdown → ...
//!   match_over → lobby (reset)
//!
//! Admin model:
//!   Never "the first to join": the listen host's player key
//!   (OWNER), configured admin keys (--admin-key / --admins-file / RCON ADMIN
//!   ADD) and admin grants; see admin.rs. With no admin connected, ready
//!   players start the match themselves (session::auto_start_step).
//!   Admins can kick/ban/promote; they see banned IPs masked (RCON: full).
//!   Bans are per-IP (socket_addr.ip()).

use crate::proto::{self, PeerId};
use crate::proto::v5;
use std::collections::{HashMap, HashSet, VecDeque};
use std::net::{IpAddr, SocketAddr};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use tokio::net::UdpSocket;
use tokio::sync::Mutex;
use tokio::time;
use tracing::{debug, info, warn};

mod dispatch;
mod session;
mod match_core;
mod admin;
mod broadcast;
mod tick;
mod combat_glue;
mod world_glue;
mod interact_glue; // interaction channel (docs/development/subsystems/interact.md)
mod records; // protocol v6: typed record messages, per-domain dispatch
mod pose_glue; // protocol v6: root / weapon / pose records
mod session_records; // session domain records (0x02xx): builders + C2S handlers
mod modes; // game modes: teams, King of the hill, roulette / brawl kits, deathmatch respawns
mod mods_glue; // server mods (0x09xx): join gating, manifest, chunk serving
mod native_glue;

// Siblings share each other's items through `use super::*`.
use session::*;
use match_core::*;
use admin::*;
use broadcast::*;
use combat_glue::*;
use interact_glue::*;
use session_records::*;

// Public API: re-exported at `crate::server::*`.
pub use dispatch::recv_loop;
pub(crate) use session::peer_leave;
pub use match_core::seed_arena;
#[allow(unused_imports)] // only used inside this module tree
pub use match_core::{DEFAULT_ARENA, SPAWN_SLOT_SENTINEL_Z, normalize_arena};
#[allow(unused_imports)] // only used inside this module tree
pub(crate) use match_core::{DEATH_DAMAGE, DEATH_DEFEAT, DEATH_LEFT, DEATH_REPORTED, DEATH_SURRENDER, DEATH_VITALS};
pub(crate) use match_core::body_context_matches;
pub use admin::{load_banlist, set_banlist_path, configure_admins, AdminOpts};
pub(crate) use admin::rcon_admin;
pub(crate) use admin::save_banlist;
pub(crate) use broadcast::{broadcast_admin_state, send_out};
pub(crate) use session_records::{broadcast_msg, chat_to, server_chat_msg};
pub(crate) use records::refused;
pub use broadcast::shutdown;
pub use tick::{tick_loop, TICK_HZ_MIN, TICK_HZ_MAX};
pub use session::net_status_notices;
pub use mods_glue::serve_loop as mods_serve_loop;
// Session layer: typed commands, snapshot, RCON match/debug verbs.
pub(crate) use session::{configure_session, rcon_debug_kill, rcon_status, run_command, seat_peer, Actor, KitView, SessionOpts};
pub(crate) use modes::{mode_label, parse_mode, ModeCfg};
#[derive(Debug, Clone)]
pub struct PeerState {
    pub id: PeerId,
    pub nick: String,
    /// v5 connection id (transport); the peer table stays keyed by address.
    pub cid: u64,
    /// Ed25519 player identity from the v5 handshake (seats, wins and bans
    /// key on this).
    pub player_key: [u8; 32],
    /// Server clock (ms) of the last packet heard from this peer.
    pub last_seen_ms: u64,
    /// The last accepted `root` record of this peer.
    pub last_root: Option<hsmp_ipc::schema::pose::Root>,
    pub ready: bool,
    pub last_valid_pos: Option<[f32; 3]>,
    /// Arrival time (server clock, ms) of the last accepted root.
    pub last_valid_ms: u64,
    pub wins: u32,
    pub alive: bool,
    pub is_admin: bool,
}


pub struct ServerState {
    inner: Mutex<Inner>,
    max_peers: usize,
    /// Protocol v5 transport (handshake, connections, channels). Kept in step
    /// with `Inner::peers` on join/leave and reconciled every tick.
    pub(crate) net: crate::net::Net,
    /// Relevance thinning + per-client bandwidth budget for stream relays.
    pub(crate) relay: crate::relay::Relay,
    /// Peers with relayed messages queued while the receive loop handles one datagram (None =
    /// not batching). Flushed after it, so records a sender sent together leave together.
    relay_batch: std::sync::Mutex<Option<Vec<SocketAddr>>>,
    /// Server mods (`--mods-dir`; mods_glue.rs): set once at startup, never when there are none.
    pub(crate) mods: std::sync::OnceLock<Arc<crate::server_mods::Host>>,
}

pub(crate) struct Inner {
    native: Option<native_glue::NativeCore>,
    pub peers: HashMap<SocketAddr, PeerState>,
    next_peer_id: PeerId,
    /// Ticks run so far (stats only: no timer counts ticks).
    server_tick: u32,
    /// Server clock (ms, `Net::now_ms`) at the start of the current tick. Every
    /// match timer is real time on this clock, so `--tick-hz` changes only how
    /// often they are checked.
    now_ms: u64,
    pub admin_peer_id: PeerId,
    pub banned_ips: HashSet<IpAddr>,
    /// Who is admin (owner / configured / granted keys), admin.rs.
    pub admins: admin::AdminPolicy,
    /// If set, every change to banned_ips is persisted to this path.
    pub banlist_path: Option<PathBuf>,

    match_state: String,
    match_arena: String,
    match_round: u32,
    /// Time left on the phase timer (countdown, result screens, pause grace).
    countdown_ms: u64,
    best_of: u8,
    match_state_dirty: bool,

    // ---- match loop that survives drops and reconnects (see `match_step`) ----
    /// Per-peer game-client reports from the `ping:<loaded_round>:<dead>` verb.
    match_peers: HashMap<PeerId, MatchPeer>,
    /// Countdown is frozen until every barrier-aware participant has loaded
    /// the arena for the pending round (or `barrier_deadline_ms` passes).
    barrier_passed: bool,
    barrier_deadline_ms: u64,
    /// Player keys playing this match (set at START). Keyed by identity, not
    /// peer id, so a reconnect (new peer id) rejoins the same seat.
    participants: Vec<session::PlayerKey>,
    /// Authoritative wins per participant key; restored onto reconnects.
    wins_by_key: HashMap<session::PlayerKey, u32>,
    /// Winner of the last finished round/match (0 = draw / none).
    last_winner: PeerId,
    /// Why the match is where it is: "", "draw", "forfeit", "opponent_left".
    match_reason: String,
    /// Server-authoritative spawn plan (spawns.rs) for `spawn_round`; rides
    /// in every S2CMatchState. Computed when a countdown begins.
    spawn_round: u32,
    spawn_plan: Vec<crate::spawns::SpawnAssign>,

    // ---- authoritative deaths + simultaneous-kill settle (combat) ----
    /// Deaths declared this round: (peer, killer, cause). Re-broadcast as
    /// S2CDeath with every match-state broadcast until the next round.
    round_deaths: Vec<(PeerId, PeerId, u8, u16)>,
    /// Above 0 while a finished round "settles": the last-standing player is
    /// provisional; a trade hit / death landing in this window makes it a
    /// draw. The result is published only when this reaches 0.
    settle_ms: u64,
    /// Pause budget: Live pauses each player key took this match,
    /// all Live pauses this match, and whether the current pause interrupted
    /// a Live round (it then resumes that same round, never replays it).
    pauses_by_key: HashMap<session::PlayerKey, u32>,
    match_pauses: u32,
    paused_from_live: bool,
    /// Server -> peer record messages (v6, `hsmp_ipc::wire` framed) produced under the lock
    /// (None = everyone); drained by `flush_out` after the lock is released.
    out_msgs: Vec<(Option<SocketAddr>, Vec<u8>)>,
    /// Session layer: epoch, snapshot seq, match id,
    /// frozen config, seats by key, command results cache.
    sess: session::SessionCore,
    /// Game modes (modes.rs): config, teams, scores, round clock, respawns, round kit.
    modes: modes::ModeCore,
    /// Players still loading the server's mods (peer id -> since, server ms; mods_glue.rs).
    pub(crate) mods_pending: HashMap<PeerId, u64>,
}

/// What a peer's GAME (not just its sidecar) last told us.
#[derive(Debug, Clone, Default)]
pub(crate) struct MatchPeer {
    /// Sent at least one ping: a real game client that takes part in the
    /// load barrier. Headless/test peers never ping and never block it.
    aware: bool,
    /// Round number whose arena this client has loaded and spawned into.
    loaded_round: u32,
    last_ping_ms: u64,
    /// Last round this client reported its pawn placed (`spawned:` verb).
    spawned_round: u32,
}

impl ServerState {
    /// A server with a throw-away transport identity (tests).
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn new(max_peers: usize) -> Self {
        Self::with_net(max_peers, crate::net::Net::ephemeral())
    }

    pub fn with_net(max_peers: usize, net: crate::net::Net) -> Self {
        Self {
            inner: Mutex::new(Inner {
                native: None,
                peers: HashMap::new(),
                next_peer_id: 1,
                server_tick: 0,
                now_ms: 0,
                admin_peer_id: 0,
                banned_ips: HashSet::new(),
                admins: admin::AdminPolicy::default(),
                banlist_path: None,
                match_state: "lobby".to_string(),
                match_arena: "default".to_string(),
                match_round: 0,
                countdown_ms: 0,
                best_of: 3,
                match_state_dirty: false,
                match_peers: HashMap::new(),
                barrier_passed: true,
                barrier_deadline_ms: 0,
                participants: Vec::new(),
                wins_by_key: HashMap::new(),
                last_winner: 0,
                match_reason: String::new(),
                spawn_round: 0,
                spawn_plan: Vec::new(),
                round_deaths: Vec::new(),
                settle_ms: 0,
                pauses_by_key: HashMap::new(),
                match_pauses: 0,
                paused_from_live: false,
                out_msgs: Vec::new(),
                sess: session::SessionCore::new(max_peers),
                modes: modes::ModeCore::default(),
                mods_pending: HashMap::new(),
            }),
            max_peers,
            net,
            relay: crate::relay::Relay::default(),
            relay_batch: std::sync::Mutex::new(None),
            mods: std::sync::OnceLock::new(),
        }
    }

    pub(crate) fn with_native(max_peers: usize, net: crate::net::Net, bridge: Arc<crate::native_service::Bridge>, arena: &str) -> Self {
        let state = Self::with_net(max_peers, net);
        if let Ok(mut inner) = state.inner.try_lock() {
            inner.match_arena = arena.to_owned();
            inner.native = Some(native_glue::NativeCore::new(bridge, state.net.epoch(), arena));
        }
        state
    }

    /// True while no match runs (config, kit rules may change).
    pub(crate) async fn in_lobby(&self) -> bool {
        self.inner.lock().await.match_state == "lobby"
    }

    /// Expose the inner lock to the RCON module (same-crate only).
    pub(crate) fn lock(&self) -> &Mutex<Inner> { &self.inner }

    /// Non-blocking read of the lobby-picked arena for the server browser
    /// (None while the match lock is busy).
    pub(crate) fn current_arena(&self) -> Option<String> {
        self.inner.try_lock().ok().map(|i| i.match_arena.clone())
    }

    /// Non-blocking read of the live best-of for the server browser's mode
    /// label (None while the match lock is busy).
    pub(crate) fn current_best_of(&self) -> Option<u8> {
        self.inner.try_lock().ok().map(|i| i.best_of)
    }

    /// Non-blocking read of the game mode in force (the frozen one in a match) for the
    /// server browser's label (None while the match lock is busy).
    pub(crate) fn current_mode(&self) -> Option<u8> {
        self.inner.try_lock().ok().map(|i| i.modes.now().mode)
    }
}

