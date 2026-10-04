//! Session, match and connection (kinds `0x02xx`): the one binary form of everything about
//! the session end to end. The server builds these records, the sidecar copies
//! them into the game's slots / S2G ring as they are (and frames the game's G2S records for
//! the server as they are), the game reads / writes typed tables.
//!
//! - Network: `welcome`, `session` (the full idempotent snapshot: config, frozen config,
//!   roster rows with wins / alive / ready / waiting / spawn order, last result; it replaces
//!   the v4 match state and spawn plan), `pings`, `command` / `cmd_result`, `game_status`,
//!   `spawned`, `notice`, `kill_feed`, `kicked`, `server_closing`, `leave`, `chat` /
//!   `chat_in`, `admin_state`, `ping` / `pong`.
//! - Sidecar-local slot (no network message): `link` (status, my peer id, admin flag, link
//!   state, kick / reject reason, transport metrics). Republished on change only.
//! - Game-local bus keys: `director`, `conn_state`, `spectate`, `spawn_status`,
//!   `spawn_request`, `travel_request`, `travel_ack`, `ui_request`, `return_to_lobby`.

use super::{Dir, KindInfo, CAP_BUS, CAP_QUEUES, CAP_STATE};
use crate::layout::{Bool, Str};
use crate::record::Invalid;

/// Legacy codec kinds of this domain: none (all moved to records).
pub const KINDS: &[KindInfo] = &[];

// ---- sizes --------------------------------------------------------------------------------

/// Roster rows in a session snapshot (seats; connected players + seats kept for reconnects).
pub const MAX_ROSTER: usize = 64;
/// Rows of a `pings` record.
pub const MAX_PINGS: usize = 64;
/// Ban rows of an `admin_state` record (masked entries).
pub const MAX_BANS: usize = 64;
/// Arguments of a `notice`.
pub const NOTICE_ARGS: usize = 4;
/// `conn_state` actions.
pub const CONN_ACTIONS: usize = 4;
/// No seat / no winner.
pub const NO_SEAT: u8 = 0xFF;

// ---- records -------------------------------------------------------------------------------

crate::ipc_pod! {
    /// One session configuration (`v5::SessionConfig`): the live lobby config or the config
    /// frozen at START.
    pub struct SessionConfig {
        pub rev: u32,
        pub round_time_limit_s: u16,
        pub kit_budget: u16,
        /// `game_mode` code.
        pub mode: u8,
        pub best_of: u8,
        /// `team_rule` code; `teams` = team count for AUTO.
        pub team_rule: u8,
        pub teams: u8,
        /// `kit_mode` code.
        pub kit_mode: u8,
        pub kit_fairness: u8,
        pub max_fighters: u8,
        pub max_spectators: u8,
        pub countdown_s: u8,
        pub roundover_s: u8,
        pub matchover_s: u8,
        pub barrier_timeout_s: u8,
        /// `jip` code.
        pub join_in_progress: u8,
        pub _r: [u8; 3],
        /// Arena map name ("Map_Arena_Alley", ...).
        pub arena: Str<40>,
    }

    /// Head of the session snapshot (S2C `session`, copied into the game's `session` slot).
    /// Sent reliably-superseding on change, else 1 Hz in the lobby and 3 Hz in a match.
    /// Receivers drop `(epoch, seq) <= last`; a new epoch resets all tracking.
    pub struct SessionHead {
        /// The server instance (random at server boot).
        pub epoch: u64,
        /// New on every START; 0 in the lobby.
        pub match_id: u64,
        /// Server clock (ms, the Welcome's clock) when the phase ends; 0 = no deadline.
        pub phase_deadline_ms: u64,
        pub server_time_ms: u64,
        pub seq: u32,
        /// The current round (the round being loaded / counted down to is `round + 1`).
        pub round: u32,
        pub world_epoch: u32,
        /// `phase` code.
        pub phase: u8,
        /// Seat of the last round / match winner; `NO_SEAT` = draw / none.
        pub winner_seat: u8,
        /// `result_reason` code of the last result.
        pub result_reason: u8,
        /// `frozen` is valid (outside the lobby).
        pub has_frozen: Bool,
        /// Roster rows.
        pub n: u16,
        pub _r: u16,
        pub _r2: u32,
        /// The live (lobby) config.
        pub config: SessionConfig,
        /// The config frozen at START (valid if `has_frozen`).
        pub frozen: SessionConfig,
    }

    /// One seat of the roster (`v5::RosterEntry`, plus the load-barrier flag and the spawn
    /// order flattened). A dropped participant keeps its seat with `connected` false and
    /// `peer_id` 0.
    pub struct RosterRow {
        /// The current connection's peer id (0 while disconnected).
        pub peer_id: u32,
        /// Last round this player reported loaded on the right arena.
        pub loaded_round: u32,
        pub wins: u32,
        /// Spawn order for the current / pending round (`round << 8 | index`); 0 = none.
        pub spawn_id: u32,
        pub kit_rev: u64,
        /// First 8 bytes of SHA-256(player key).
        pub player_id: [u8; 8],
        /// Spawn point (cm, floor level; the client ground-snaps).
        pub spawn_pos: [f32; 3],
        pub spawn_yaw: f32,
        pub spawn_protect_ms: u32,
        pub seat: u8,
        /// `role` code.
        pub role: u8,
        pub team: u8,
        /// `admin_role` code.
        pub admin_role: u8,
        pub ready: Bool,
        pub alive: Bool,
        pub connected: Bool,
        /// The load barrier is waiting on this seat (phase LOADING).
        pub waiting: Bool,
        /// Index into the arena's candidate spawn list.
        pub spawn_slot: u8,
        pub _r: [u8; 3],
        pub nick: Str<32>,
    }

    /// First message after the handshake (S2C `welcome`; the sidecar keeps it).
    pub struct Welcome {
        /// Random at server boot; a change means "server restarted".
        pub server_epoch: u64,
        /// Server monotonic clock at send (ms).
        pub server_time_ms: u64,
        /// Negotiated capability bits (`net::caps`).
        pub caps: u64,
        pub peer_id: u32,
        /// Seat by player key; `NO_SEAT` = none yet.
        pub seat: u8,
        /// `role` code.
        pub role: u8,
        pub _r: u16,
        /// Display nick after server-side deduplication.
        pub nick: Str<32>,
    }

    /// Head of `pings`: every connected player's transport srtt, ~1 Hz.
    pub struct PingsHead {
        pub epoch: u64,
        pub server_time_ms: u64,
        pub n: u16,
        pub _r: [u16; 3],
    }

    pub struct PingRow {
        pub peer_id: u32,
        /// Smoothed RTT, ms; 0 = unknown.
        pub rtt_ms: u16,
        /// `NO_SEAT` = no seat.
        pub seat: u8,
        pub _r: u8,
    }

    /// A partial config update (`command` op SET_CONFIG): the `cfg` bits in `mask` say which
    /// fields are set.
    pub struct ConfigPatch {
        /// `cfg` bits.
        pub mask: u32,
        pub round_time_limit_s: u16,
        pub kit_budget: u16,
        pub mode: u8,
        pub best_of: u8,
        pub team_rule: u8,
        pub teams: u8,
        pub kit_mode: u8,
        pub kit_fairness: u8,
        pub max_fighters: u8,
        pub max_spectators: u8,
        pub join_in_progress: u8,
        pub _r: [u8; 3],
        pub _r2: u32,
        pub arena: Str<40>,
    }

    /// A typed, idempotent command (G2S from the game, framed C2S as it is). The sidecar
    /// resends it every 250 ms until its `cmd_result` arrives (give up after 5 s); the server
    /// answers a duplicate `(player, cmd_id)` with the cached result.
    pub struct Command {
        /// Never 0 (the game assigns it).
        pub cmd_id: u32,
        /// `SessionConfig.rev` the sender saw; 0 = don't care.
        pub expected_rev: u32,
        /// `cmd_op` code.
        pub op: u8,
        /// READY: the value (false = unready). START: force.
        pub flag: Bool,
        /// PROMOTE: `admin_role` code; SWITCH_ROLE: `role` code; SET_TEAM: the team.
        pub role: u8,
        /// VOTE: the choice.
        pub choice: u8,
        /// KICK / BAN / PROMOTE: the target peer.
        pub peer_id: u32,
        /// BAN: duration (0 = permanent).
        pub duration_s: u32,
        /// VOTE: the ballot.
        pub ballot: u32,
        /// PICK_ARENA: the arena; KICK / BAN: the reason; UNBAN: the masked entry or token.
        pub text: Str<64>,
        /// SET_CONFIG.
        pub patch: ConfigPatch,
    }

    /// Exactly one per `cmd_id` (S2C, pushed to the game as an S2G event as it is).
    pub struct CmdResult {
        pub cmd_id: u32,
        /// Config revision after the command.
        pub config_rev: u32,
        /// `cmd_reason` code; 0 when ok.
        pub reason_code: u16,
        pub ok: Bool,
        /// The command's `cmd_op` (echoed by the server).
        pub op: u8,
        pub _r: u32,
        /// Human-readable ("START refused: 1 of 2 ready").
        pub reason_text: Str<120>,
    }

    /// The game's state report (G2S from the Director ~1 Hz and on change, framed C2S): load
    /// barrier, game liveness, redundant death report, applied spawn order.
    pub struct GameStatus {
        pub match_id: u64,
        /// The round whose world is loaded (with `status_flag` LOADED).
        pub round: u32,
        /// Hash of the loaded UWorld instance.
        pub world_key: u32,
        /// `status_flag` bits.
        pub flags: u32,
        /// Last spawn order applied (0 = none).
        pub spawn_id: u32,
        /// `load_error` code; 0 = none.
        pub load_error: u8,
        pub _r: [u8; 7],
        /// Short name of the loaded world's arena; "" while in the menu.
        pub arena: Str<40>,
    }

    /// The pawn was placed on its spawn order (G2S from HSMPSync, framed C2S).
    pub struct Spawned {
        pub round: u32,
        pub slot: u32,
        /// Where it stands (cm).
        pub pos: [f32; 3],
        /// The point was clear (no displacement).
        pub clear: Bool,
        pub _r: [u8; 3],
    }

    /// A server notice (S2C, pushed to the game as it is). Idempotent by `event_id`.
    pub struct Notice {
        pub match_id: u64,
        pub event_id: u32,
        /// `notice` code.
        pub code: u16,
        pub _r: u16,
        /// HOST_LEFT: nick, new host. LOAD_FAILED: nick, error, seat, round. PLAYER_JOINED:
        /// nick, joined | rejoined. PLAYER_LEFT: nick, left | disconnected.
        pub args: [Str<48>; 4],
    }

    /// A kill (S2C, pushed to the game as it is). Idempotent by `event_id`.
    pub struct KillFeed {
        pub match_id: u64,
        pub event_id: u32,
        pub killer_seat: u8,
        pub victim_seat: u8,
        pub cause: u8,
        pub _r: u8,
        pub weapon: Str<48>,
    }

    /// Terminal for the sidecar: no automatic rejoin before `retry_after_s` (S2C; the sidecar
    /// shows it in `link`).
    pub struct Kicked {
        pub match_id: u64,
        pub event_id: u32,
        pub retry_after_s: u32,
        /// Handshake reject code (BANNED, ...); 0 = kicked.
        pub code: u8,
        pub _r: [u8; 7],
        pub reason: Str<120>,
    }

    /// Sent before the connection is closed SERVER_CLOSING (S2C; shown in `link`).
    pub struct ServerClosing {
        /// Restarts only: try again after this long; 0 = do not.
        pub reconnect_after_ms: u32,
        /// `closing_reason` code.
        pub reason: u8,
        pub _r: [u8; 3],
        pub text: Str<120>,
    }

    /// Leave on purpose (G2S: the game asks the sidecar to leave; C2S: the sidecar tells the
    /// server, then closes).
    pub struct Leave {
        /// `leave_reason` code.
        pub reason: u8,
        pub _r: [u8; 7],
    }

    /// A chat line (G2S, framed C2S).
    pub struct Chat {
        pub text: Str<256>,
    }

    /// A chat line from a player (or the server, `from_peer` 0) (S2C, pushed as it is).
    pub struct ChatIn {
        pub from_peer: u32,
        pub _r: u32,
        pub nick: Str<32>,
        pub text: Str<256>,
    }

    /// Who is admin (S2C, copied into the `admin` slot). An admin gets its OWN id and the
    /// masked ban list; everyone else the lowest-id connected admin (0 = none) and no bans.
    pub struct AdminStateHead {
        pub admin_peer: u32,
        pub n: u16,
        pub _r: u16,
    }

    pub struct BanRow {
        /// "203.0.x.x#1a2b3c4d".
        pub entry: Str<48>,
    }

    /// Clock probe (C2S every 2 s while connected).
    pub struct Ping {
        /// Client wall clock, ms.
        pub client_time_ms: u64,
    }

    /// Answer to `ping` (S2C).
    pub struct Pong {
        pub client_time_ms: u64,
        /// Server wall clock, ms.
        pub server_time_ms: u64,
    }

    /// The sidecar's own view of the connection (the `link` slot; no network message).
    /// Republished when any field changes (status, link state, attempt, admin, a new metrics
    /// sample every 2 s ...), never just to show it is alive (the header heartbeat does that).
    pub struct Link {
        /// The last Welcome's server epoch (0 = never welcomed).
        pub server_epoch: u64,
        /// Wall clock (ms) of the last metrics sample; 0 = none yet.
        pub metrics_wall_ms: u64,
        pub my_peer_id: u32,
        /// `sidecar_status` code.
        pub status: u8,
        /// `link_state` code.
        pub state: u8,
        pub is_admin: Bool,
        /// Kick: the handshake reject code (0 = kicked); reject: the reject code; server
        /// closed: the `closing_reason` code.
        pub reason_code: u8,
        /// Handshakes started since the link was last up (0 while up).
        pub attempt: u32,
        /// Until the next reconnect attempt, ms, at publish time.
        pub next_retry_ms: u32,
        /// Since the link went down, ms, at publish time (0 while up).
        pub down_ms: u32,
        /// Since the last datagram from the server, ms, at publish time.
        pub rx_age_ms: u32,
        /// Kicked: no automatic rejoin before this.
        pub retry_after_s: u32,
        pub rto_ms: u32,
        pub pkts_lost: u32,
        pub retransmits: u32,
        pub aead_failed: u32,
        /// Ping round trip, ms.
        pub rtt_ms: f32,
        /// Server clock - local clock, ms.
        pub clock_offset_ms: f32,
        pub srtt_ms: f32,
        /// RTT variation, ms.
        pub jitter_ms: f32,
        /// Lifetime loss, %.
        pub loss_pct: f32,
        /// Loss over the last 10 s, %; negative = no window yet.
        pub loss_pct_10s: f32,
        /// Our protocol version (shown with a reject).
        pub client_protocol: u16,
        pub _r: u16,
        /// Kick / reject / server-closed reason.
        pub reason: Str<120>,
    }

    // ---- game-local bus keys (Lua <-> Lua) -------------------------------------------------

    /// HSMPMatch Director heartbeat (bus `director`).
    pub struct DirectorState {
        /// Wall clock of the beat (os.time, s).
        pub hb: f64,
        pub round: u32,
        pub ready_round: u32,
        pub travel_n: u32,
        pub native_rewritten: u32,
        pub in_match: Bool,
        pub rematch: Bool,
        pub _r: [u8; 6],
        /// Director state ("Menu", "Travel", "Live", ...).
        pub state: Str<24>,
        /// The session phase it acts on.
        pub phase: Str<16>,
        /// Connection state ("ok", "reconnecting", "lost", "notice").
        pub conn: Str<16>,
        pub world_key: Str<64>,
        pub world: Str<48>,
        pub target: Str<48>,
        pub load_error: Str<24>,
        pub conn_reason: Str<32>,
    }

    /// The single connection state shown to the player (bus `conn_state`, Director).
    pub struct ConnState {
        /// Wall clock (os.time, s).
        pub wall: f64,
        pub seq: u32,
        pub elapsed_s: u32,
        pub remaining_s: u32,
        pub window_s: u32,
        /// The sidecar's reconnect attempt; -1 = unknown.
        pub attempt: i32,
        pub in_match: Bool,
        pub latched: Bool,
        pub _r: [u8; 2],
        pub state: Str<16>,
        pub reason: Str<32>,
        /// The sidecar status name.
        pub sidecar: Str<16>,
        /// Buttons ("reconnect", "menu"); "" = unused.
        pub actions: [Str<16>; 4],
        pub title: Str<64>,
        pub text: Str<256>,
    }

    /// The spectated player (bus `spectate`, HSMPMatch).
    pub struct Spectate {
        /// Peer id; 0 = not spectating.
        pub target: u32,
        pub round: u32,
        /// Players still alive.
        pub alive: u32,
        pub _r: u32,
        pub nick: Str<48>,
    }

    /// HSMPSync's placement report (bus `spawn_status`; the Director's evidence).
    pub struct SpawnStatus {
        /// Process clock (s) until which the pawn is protected (valid if `has_protect_until`;
        /// unbounded until Live otherwise).
        pub protect_until: f64,
        /// Process clock (s) of the report.
        pub t: f64,
        /// Destination (cm; valid if `has_dest`).
        pub pos: [f64; 3],
        /// Floor height under the destination (valid if `has_floor`).
        pub floor: f64,
        pub tol_cm: f32,
        pub protect_ms: u32,
        pub seq: u32,
        pub round: u32,
        /// 0 = none.
        pub spawn_id: u32,
        pub tries: u32,
        /// -1 = none.
        pub slot: i32,
        pub verified: Bool,
        pub clear: Bool,
        pub has_dest: Bool,
        pub has_floor: Bool,
        pub has_protect_until: Bool,
        pub _r: [u8; 7],
        pub arena: Str<48>,
        pub pawn: Str<64>,
        pub why: Str<32>,
        pub error: Str<128>,
    }

    /// The Director asks HSMPSync to (re-)place the pawn (bus `spawn_request`).
    pub struct SpawnRequest {
        pub seq: u32,
        pub round: u32,
        /// 0 = none.
        pub spawn_id: u32,
        pub _r: u32,
        pub arena: Str<48>,
        pub pawn: Str<64>,
        pub why: Str<96>,
    }

    /// HSMPMenu asks the Director to travel (bus `travel_request`).
    pub struct TravelRequest {
        /// Wall clock (os.time, s).
        pub t: f64,
        pub seq: u32,
        pub _r: u32,
        pub want: Str<32>,
        pub arena: Str<48>,
        pub reason: Str<96>,
        pub from: Str<24>,
    }

    /// The Director's answer to a travel request (bus `travel_ack`).
    pub struct TravelAck {
        pub seq: u32,
        pub _r: u32,
        pub status: Str<24>,
        pub arena: Str<48>,
        pub reason: Str<96>,
    }

    /// HSMPHud asks the Director for a UI action (bus `ui_request`).
    pub struct UiRequest {
        /// Wall clock (os.time, s).
        pub t: f64,
        pub seq: u32,
        pub _r: u32,
        pub want: Str<32>,
        pub reason: Str<96>,
        pub from: Str<24>,
    }

    /// The Director sent us back to the menu: HSMPMenu reopens the lobby once per `seq`
    /// (bus `return_to_lobby`).
    pub struct ReturnToLobby {
        pub seq: u32,
        pub _r: u32,
    }

    /// HSMPAvatars is about to swap possession for a native fallback spawn (bus `fallback_swap`):
    /// HSMPSync's placer keeps the placement and protection of pawn `keep` until `until_s`
    /// (process clock, s).
    pub struct FallbackSwap {
        pub until_s: f64,
        pub keep: Str<56>,
    }
}

// ---- kinds ---------------------------------------------------------------------------------

pub const K_WELCOME: u16 = 0x0210;
pub const K_SESSION: u16 = 0x0211;
pub const K_PINGS: u16 = 0x0212;
pub const K_COMMAND: u16 = 0x0213;
pub const K_CMD_RESULT: u16 = 0x0214;
pub const K_GAME_STATUS: u16 = 0x0215;
pub const K_SPAWNED: u16 = 0x0216;
pub const K_NOTICE: u16 = 0x0217;
pub const K_KILL_FEED: u16 = 0x0218;
pub const K_KICKED: u16 = 0x0219;
pub const K_SERVER_CLOSING: u16 = 0x021A;
pub const K_LEAVE: u16 = 0x021B;
pub const K_CHAT: u16 = 0x021C;
pub const K_CHAT_IN: u16 = 0x021D;
pub const K_ADMIN_STATE: u16 = 0x021E;
pub const K_PING: u16 = 0x021F;
pub const K_PONG: u16 = 0x0220;
pub const K_LINK: u16 = 0x0230;
pub const K_DIRECTOR: u16 = 0x0231;
pub const K_CONN_STATE: u16 = 0x0232;
pub const K_SPECTATE: u16 = 0x0233;
pub const K_SPAWN_STATUS: u16 = 0x0234;
pub const K_SPAWN_REQUEST: u16 = 0x0235;
pub const K_TRAVEL_REQUEST: u16 = 0x0236;
pub const K_TRAVEL_ACK: u16 = 0x0237;
pub const K_UI_REQUEST: u16 = 0x0238;
pub const K_RETURN_TO_LOBBY: u16 = 0x0239;
pub const K_FALLBACK_SWAP: u16 = 0x023A;

/// Supersede streams (`Chan::RelLatest` / `Chan::Latest`, `proto_v5::keys`).
pub const STREAM_SESSION: u8 = 0x80;
pub const STREAM_GAME_STATUS: u8 = 0x81;
pub const STREAM_PINGS: u8 = 0x87;
pub const STREAM_PING: u8 = 6;

// ---- code tables ---------------------------------------------------------------------------

/// `SessionHead.phase`.
pub mod phase {
    pub const LOBBY: u8 = 0;
    pub const LOADING: u8 = 1;
    pub const COUNTDOWN: u8 = 2;
    pub const LIVE: u8 = 3;
    pub const ROUND_OVER: u8 = 4;
    pub const MATCH_OVER: u8 = 5;
    pub const POST_MATCH: u8 = 6;
    pub const PAUSED: u8 = 7;
}

/// `SessionHead.result_reason`.
pub mod result_reason {
    pub const NONE: u8 = 0;
    pub const KILL: u8 = 1;
    pub const DRAW: u8 = 2;
    pub const FORFEIT: u8 = 3;
    pub const OPPONENT_LEFT: u8 = 4;
    pub const TIME_LIMIT: u8 = 5;
    pub const ABORTED: u8 = 6;
    /// Too few fighters loaded: the round was void.
    pub const LOAD_FAILED: u8 = 7;
}

/// `Command.op`.
pub mod cmd_op {
    pub const READY: u8 = 1;
    pub const START: u8 = 2;
    pub const ABORT: u8 = 3;
    pub const PICK_ARENA: u8 = 4;
    pub const SET_CONFIG: u8 = 5;
    pub const KICK: u8 = 6;
    pub const BAN: u8 = 7;
    pub const PROMOTE: u8 = 8;
    pub const VOTE: u8 = 9;
    pub const SWITCH_ROLE: u8 = 10;
    pub const SET_TEAM: u8 = 11;
    pub const UNBAN: u8 = 12;
    /// ABORT, then everyone's wins / alive / ready reset (the admin "reset match").
    pub const RESET_MATCH: u8 = 13;
    pub const MAX: u8 = RESET_MATCH;
}

/// `ConfigPatch.mask` bits.
pub mod cfg {
    pub const ARENA: u32 = 1 << 0;
    pub const MODE: u32 = 1 << 1;
    pub const BEST_OF: u32 = 1 << 2;
    pub const ROUND_TIME: u32 = 1 << 3;
    pub const TEAM_RULE: u32 = 1 << 4;
    pub const TEAMS: u32 = 1 << 5;
    pub const KIT_RULES: u32 = 1 << 6;
    pub const MAX_FIGHTERS: u32 = 1 << 7;
    pub const MAX_SPECTATORS: u32 = 1 << 8;
    pub const JIP: u32 = 1 << 9;
    pub const ALL: u32 = (1 << 10) - 1;
}

/// `CmdResult.reason_code`.
pub mod cmd_reason {
    pub const OK: u16 = 0;
    pub const NOT_ADMIN: u16 = 1;
    pub const WRONG_PHASE: u16 = 2;
    pub const NOT_ALL_READY: u16 = 3;
    pub const REV_MISMATCH: u16 = 4;
    pub const UNKNOWN_ARENA: u16 = 5;
    pub const INVALID_VALUE: u16 = 6;
    pub const UNKNOWN_PLAYER: u16 = 7;
    pub const NOT_ENOUGH_PLAYERS: u16 = 8;
    pub const RATE_LIMITED: u16 = 9;
    pub const UNSUPPORTED: u16 = 10;
    pub const MAX: u16 = UNSUPPORTED;
}

/// `GameStatus.flags` bits.
pub mod status_flag {
    /// The world for `(match_id, round, arena)` is loaded.
    pub const LOADED: u32 = 1 << 0;
    /// The spawn pipeline finished (pawn verified, dressed, placed).
    pub const READY: u32 = 1 << 1;
    pub const DEAD: u32 = 1 << 2;
    pub const IN_MENU: u32 = 1 << 3;
    pub const SPECTATING: u32 = 1 << 4;
    /// The game process is paused / minimised (informational).
    pub const BACKGROUND: u32 = 1 << 5;
    pub const ALL: u32 = (1 << 6) - 1;
}

/// `GameStatus.load_error`.
pub mod load_error {
    pub const NONE: u8 = 0;
    pub const TRAVEL_FAILED: u8 = 1;
    pub const WRONG_WORLD: u8 = 2;
    pub const NO_PAWN: u8 = 3;
    pub const VITALS: u8 = 4;
    pub const SPAWN_PLACE: u8 = 5;
    pub const DRESS: u8 = 6;
    pub const STAND_INS: u8 = 7;
    pub const TIMEOUT: u8 = 8;
    /// Any other error the game reports.
    pub const OTHER: u8 = 9;
    pub const MAX: u8 = OTHER;
}

/// `Notice.code`.
pub mod notice {
    pub const HOST_LEFT: u16 = 1;
    pub const ROLE_CHANGED: u16 = 2;
    pub const LOAD_FAILED: u16 = 3;
    pub const PLAYER_JOINED: u16 = 4;
    pub const PLAYER_LEFT: u16 = 5;
    pub const ADMIN_CHANGED: u16 = 6;
    pub const CONFIG_QUEUED: u16 = 7;
    pub const SUDDEN_DEATH: u16 = 8;
    /// Listen host only: how reachable the server is (args: state, port, public address, NAT kind).
    pub const NET_STATUS: u16 = 9;
}

/// `Link.status`.
pub mod sidecar_status {
    pub const CONNECTING: u8 = 0;
    pub const CONNECTED: u8 = 1;
    pub const RECONNECTING: u8 = 2;
    pub const REJECTED: u8 = 3;
    pub const KICKED: u8 = 4;
    pub const REPLACED: u8 = 5;
    pub const SERVER_CLOSED: u8 = 6;
    /// The sidecar's own exit (career saves restored).
    pub const ENDED: u8 = 7;
    pub const MAX: u8 = ENDED;
}

/// `Link.state`.
pub mod link_state {
    pub const CONNECTING: u8 = 0;
    pub const UP: u8 = 1;
    /// Connected, but nothing from the server for 2.5 s.
    pub const STALLED: u8 = 2;
    pub const RECONNECTING: u8 = 3;
    /// Kicked / rejected / replaced / server closed / left: never reconnects.
    pub const TERMINAL: u8 = 4;
    pub const MAX: u8 = TERMINAL;
}

pub const ENUMS: &[super::EnumInfo] = &[
    super::EnumInfo { name: "phase", values: &[("LOBBY", 0), ("LOADING", 1), ("COUNTDOWN", 2), ("LIVE", 3),
        ("ROUND_OVER", 4), ("MATCH_OVER", 5), ("POST_MATCH", 6), ("PAUSED", 7)] },
    super::EnumInfo { name: "game_mode", values: &[("DUEL", 0), ("FFA", 1), ("TEAM_ELIM", 2), ("KING_OF_HILL", 3)] },
    super::EnumInfo { name: "team_rule", values: &[("NONE", 0), ("AUTO", 1), ("FIXED", 2)] },
    super::EnumInfo { name: "jip", values: &[("SPECTATE", 0), ("NEXT_ROUND", 1), ("NEVER", 2)] },
    super::EnumInfo { name: "kit_mode", values: &[("FREE", 0), ("CLASSES", 1), ("CUSTOM", 2)] },
    super::EnumInfo { name: "role", values: &[("FIGHTER", 0), ("SPECTATOR", 1), ("QUEUED", 2)] },
    super::EnumInfo { name: "admin_role", values: &[("NONE", 0), ("MODERATOR", 1), ("ADMIN", 2), ("OWNER", 3)] },
    super::EnumInfo { name: "result_reason", values: &[("NONE", 0), ("KILL", 1), ("DRAW", 2), ("FORFEIT", 3),
        ("OPPONENT_LEFT", 4), ("TIME_LIMIT", 5), ("ABORTED", 6), ("LOAD_FAILED", 7)] },
    super::EnumInfo { name: "cmd_op", values: &[("READY", 1), ("START", 2), ("ABORT", 3), ("PICK_ARENA", 4),
        ("SET_CONFIG", 5), ("KICK", 6), ("BAN", 7), ("PROMOTE", 8), ("VOTE", 9), ("SWITCH_ROLE", 10),
        ("SET_TEAM", 11), ("UNBAN", 12), ("RESET_MATCH", 13)] },
    super::EnumInfo { name: "cfg", values: &[("ARENA", 1), ("MODE", 2), ("BEST_OF", 4), ("ROUND_TIME", 8),
        ("TEAM_RULE", 16), ("TEAMS", 32), ("KIT_RULES", 64), ("MAX_FIGHTERS", 128), ("MAX_SPECTATORS", 256),
        ("JIP", 512)] },
    super::EnumInfo { name: "cmd_reason", values: &[("OK", 0), ("NOT_ADMIN", 1), ("WRONG_PHASE", 2),
        ("NOT_ALL_READY", 3), ("REV_MISMATCH", 4), ("UNKNOWN_ARENA", 5), ("INVALID_VALUE", 6),
        ("UNKNOWN_PLAYER", 7), ("NOT_ENOUGH_PLAYERS", 8), ("RATE_LIMITED", 9), ("UNSUPPORTED", 10)] },
    super::EnumInfo { name: "status_flag", values: &[("LOADED", 1), ("READY", 2), ("DEAD", 4), ("IN_MENU", 8),
        ("SPECTATING", 16), ("BACKGROUND", 32)] },
    super::EnumInfo { name: "load_error", values: &[("NONE", 0), ("TRAVEL_FAILED", 1), ("WRONG_WORLD", 2),
        ("NO_PAWN", 3), ("VITALS", 4), ("SPAWN_PLACE", 5), ("DRESS", 6), ("STAND_INS", 7), ("TIMEOUT", 8),
        ("OTHER", 9)] },
    super::EnumInfo { name: "notice", values: &[("HOST_LEFT", 1), ("ROLE_CHANGED", 2), ("LOAD_FAILED", 3),
        ("PLAYER_JOINED", 4), ("PLAYER_LEFT", 5), ("ADMIN_CHANGED", 6), ("CONFIG_QUEUED", 7), ("SUDDEN_DEATH", 8),
        ("NET_STATUS", 9)] },
    super::EnumInfo { name: "closing_reason", values: &[("SHUTDOWN", 0), ("HOST_LEFT", 1), ("RESTART", 2), ("IDLE", 3)] },
    super::EnumInfo { name: "leave_reason", values: &[("USER", 0), ("GAME_EXITED", 1), ("SWITCH_SERVER", 2)] },
    super::EnumInfo { name: "sidecar_status", values: &[("CONNECTING", 0), ("CONNECTED", 1), ("RECONNECTING", 2),
        ("REJECTED", 3), ("KICKED", 4), ("REPLACED", 5), ("SERVER_CLOSED", 6), ("ENDED", 7)] },
    super::EnumInfo { name: "link_state", values: &[("CONNECTING", 0), ("UP", 1), ("STALLED", 2),
        ("RECONNECTING", 3), ("TERMINAL", 4)] },
];

// ---- checks --------------------------------------------------------------------------------

const WORLD_LIMIT: f32 = super::pose::WORLD_LIMIT;

fn check_config(c: &SessionConfig) -> Result<(), Invalid> {
    if c.mode > 3 {
        return Err(Invalid::Range("mode"));
    }
    if c.team_rule > 2 {
        return Err(Invalid::Range("team_rule"));
    }
    if c.kit_mode > 2 {
        return Err(Invalid::Range("kit_mode"));
    }
    if c.join_in_progress > 2 {
        return Err(Invalid::Range("join_in_progress"));
    }
    Ok(())
}

fn check_session(h: &SessionHead) -> Result<(), Invalid> {
    if h.phase > phase::PAUSED {
        return Err(Invalid::Range("phase"));
    }
    if h.result_reason > result_reason::LOAD_FAILED {
        return Err(Invalid::Range("result_reason"));
    }
    check_config(&h.config)?;
    if h.has_frozen.get() {
        check_config(&h.frozen)?;
    }
    Ok(())
}

fn check_roster(_h: &SessionHead, r: &RosterRow) -> Result<(), Invalid> {
    if r.role > 2 {
        return Err(Invalid::Range("role"));
    }
    if r.admin_role > 3 {
        return Err(Invalid::Range("admin_role"));
    }
    if r.spawn_pos.iter().any(|c| c.abs() > WORLD_LIMIT) {
        return Err(Invalid::Range("spawn_pos"));
    }
    if r.connected.get() && r.peer_id == 0 {
        return Err(Invalid::Range("peer_id"));
    }
    Ok(())
}

fn check_welcome(w: &Welcome) -> Result<(), Invalid> {
    if w.peer_id == 0 {
        return Err(Invalid::Range("peer_id"));
    }
    if w.role > 2 {
        return Err(Invalid::Range("role"));
    }
    Ok(())
}

fn check_command(c: &Command) -> Result<(), Invalid> {
    if c.cmd_id == 0 {
        return Err(Invalid::Range("cmd_id"));
    }
    if c.op == 0 || c.op > cmd_op::MAX {
        return Err(Invalid::Range("op"));
    }
    match c.op {
        cmd_op::PICK_ARENA if c.text.is_empty() => return Err(Invalid::Range("text")),
        cmd_op::KICK | cmd_op::BAN | cmd_op::PROMOTE if c.peer_id == 0 => return Err(Invalid::Range("peer_id")),
        cmd_op::PROMOTE if c.role > 3 => return Err(Invalid::Range("role")),
        cmd_op::SWITCH_ROLE if c.role > 2 => return Err(Invalid::Range("role")),
        cmd_op::UNBAN if c.text.is_empty() => return Err(Invalid::Range("text")),
        cmd_op::SET_CONFIG => {
            let p = &c.patch;
            if p.mask == 0 || p.mask & !cfg::ALL != 0 {
                return Err(Invalid::Range("mask"));
            }
            if p.mask & cfg::ARENA != 0 && p.arena.is_empty() {
                return Err(Invalid::Range("arena"));
            }
            if p.mask & cfg::MODE != 0 && p.mode > 3 {
                return Err(Invalid::Range("mode"));
            }
            if p.mask & cfg::TEAM_RULE != 0 && p.team_rule > 2 {
                return Err(Invalid::Range("team_rule"));
            }
            if p.mask & cfg::KIT_RULES != 0 && p.kit_mode > 2 {
                return Err(Invalid::Range("kit_mode"));
            }
            if p.mask & cfg::JIP != 0 && p.join_in_progress > 2 {
                return Err(Invalid::Range("join_in_progress"));
            }
        }
        _ => {}
    }
    Ok(())
}

fn check_cmd_result(r: &CmdResult) -> Result<(), Invalid> {
    if r.cmd_id == 0 {
        return Err(Invalid::Range("cmd_id"));
    }
    if r.op > cmd_op::MAX {
        return Err(Invalid::Range("op"));
    }
    Ok(())
}

fn check_game_status(g: &GameStatus) -> Result<(), Invalid> {
    if g.flags & !status_flag::ALL != 0 {
        return Err(Invalid::Range("flags"));
    }
    if g.load_error > load_error::MAX {
        return Err(Invalid::Range("load_error"));
    }
    Ok(())
}

fn check_spawned(s: &Spawned) -> Result<(), Invalid> {
    if s.round == 0 {
        return Err(Invalid::Range("round"));
    }
    if s.pos.iter().any(|c| c.abs() > WORLD_LIMIT) {
        return Err(Invalid::Range("pos"));
    }
    Ok(())
}

fn check_notice(n: &Notice) -> Result<(), Invalid> {
    if n.code == 0 {
        return Err(Invalid::Range("code"));
    }
    Ok(())
}

fn check_closing(c: &ServerClosing) -> Result<(), Invalid> {
    if c.reason > 3 {
        return Err(Invalid::Range("reason"));
    }
    Ok(())
}

fn check_leave(l: &Leave) -> Result<(), Invalid> {
    if l.reason > 2 {
        return Err(Invalid::Range("reason"));
    }
    Ok(())
}

fn check_chat(c: &Chat) -> Result<(), Invalid> {
    if c.text.as_str().is_none_or(|s| s.trim().is_empty()) {
        return Err(Invalid::Range("text"));
    }
    Ok(())
}

fn check_link(l: &Link) -> Result<(), Invalid> {
    if l.status > sidecar_status::MAX {
        return Err(Invalid::Range("status"));
    }
    if l.state > link_state::MAX {
        return Err(Invalid::Range("state"));
    }
    Ok(())
}

crate::record!(Welcome, kind = K_WELCOME, name = "welcome", check = check_welcome);
crate::record!(SessionHead, kind = K_SESSION, name = "session", rows = RosterRow, count = n, max = MAX_ROSTER,
    check = check_session, check_row = check_roster);
crate::record!(PingsHead, kind = K_PINGS, name = "pings", rows = PingRow, count = n, max = MAX_PINGS);
crate::record!(Command, kind = K_COMMAND, name = "command", check = check_command);
crate::record!(CmdResult, kind = K_CMD_RESULT, name = "cmd_result", check = check_cmd_result);
crate::record!(GameStatus, kind = K_GAME_STATUS, name = "game_status", check = check_game_status);
crate::record!(Spawned, kind = K_SPAWNED, name = "spawned", check = check_spawned);
crate::record!(Notice, kind = K_NOTICE, name = "notice", check = check_notice);
crate::record!(KillFeed, kind = K_KILL_FEED, name = "kill_feed");
crate::record!(Kicked, kind = K_KICKED, name = "kicked");
crate::record!(ServerClosing, kind = K_SERVER_CLOSING, name = "server_closing", check = check_closing);
crate::record!(Leave, kind = K_LEAVE, name = "leave", check = check_leave);
crate::record!(Chat, kind = K_CHAT, name = "chat", check = check_chat);
crate::record!(ChatIn, kind = K_CHAT_IN, name = "chat_in");
crate::record!(AdminStateHead, kind = K_ADMIN_STATE, name = "admin_state", rows = BanRow, count = n, max = MAX_BANS);
crate::record!(Ping, kind = K_PING, name = "ping");
crate::record!(Pong, kind = K_PONG, name = "pong");
crate::record!(Link, kind = K_LINK, name = "link", check = check_link);
crate::record!(DirectorState, kind = K_DIRECTOR, name = "director");
crate::record!(ConnState, kind = K_CONN_STATE, name = "conn_state");
crate::record!(Spectate, kind = K_SPECTATE, name = "spectate");
crate::record!(SpawnStatus, kind = K_SPAWN_STATUS, name = "spawn_status");
crate::record!(SpawnRequest, kind = K_SPAWN_REQUEST, name = "spawn_request");
crate::record!(TravelRequest, kind = K_TRAVEL_REQUEST, name = "travel_request");
crate::record!(TravelAck, kind = K_TRAVEL_ACK, name = "travel_ack");
crate::record!(UiRequest, kind = K_UI_REQUEST, name = "ui_request");
crate::record!(ReturnToLobby, kind = K_RETURN_TO_LOBBY, name = "return_to_lobby");
crate::record!(FallbackSwap, kind = K_FALLBACK_SWAP, name = "fallback_swap");

use super::flow::{C2S, G2S, LOCAL, S2C, S2G};
use super::Chan;

/// Record kinds of this domain (wire + shared memory).
pub const RECORDS: &[super::RecordInfo] = &[
    crate::record_info!(Welcome, cap = 0, flow = S2C, chan = Chan::Ordered,
        doc = "first message after the handshake (peer id, seat, epoch, caps, nick); kept by the sidecar"),
    crate::record_info!(SessionHead, cap = CAP_STATE, flow = S2C | S2G, chan = Chan::RelLatest(STREAM_SESSION),
        doc = "the full idempotent session snapshot (config, frozen config, roster, last result); copied into slot `session`"),
    crate::record_info!(PingsHead, cap = 0, flow = S2C, chan = Chan::RelLatest(STREAM_PINGS),
        doc = "every connected player's srtt (~1 Hz); the sidecar puts it into PeerDir rtt_ms"),
    crate::record_info!(Command, cap = CAP_QUEUES, flow = C2S | G2S, chan = Chan::Ordered,
        doc = "typed command (op + args + config patch); resent by the sidecar until its cmd_result"),
    crate::record_info!(CmdResult, cap = CAP_QUEUES, flow = S2C | S2G, chan = Chan::Ordered,
        doc = "exactly one result per cmd_id; pushed to the game as an event"),
    crate::record_info!(GameStatus, cap = CAP_QUEUES, flow = C2S | G2S, chan = Chan::RelLatest(STREAM_GAME_STATUS),
        doc = "the game's load / liveness / death / applied-spawn report (~1 Hz and on change)"),
    crate::record_info!(Spawned, cap = CAP_QUEUES, flow = C2S | G2S, chan = Chan::Ordered,
        doc = "the pawn was placed on its spawn order"),
    crate::record_info!(Notice, cap = CAP_QUEUES, flow = S2C | S2G, chan = Chan::Reliable,
        doc = "server notice (joined / left / host left / load failed ...); event"),
    crate::record_info!(KillFeed, cap = CAP_QUEUES, flow = S2C | S2G, chan = Chan::Reliable,
        doc = "kill feed line; event"),
    crate::record_info!(Kicked, cap = 0, flow = S2C, chan = Chan::Reliable,
        doc = "kicked / banned: terminal; the sidecar shows it in `link`"),
    crate::record_info!(ServerClosing, cap = 0, flow = S2C, chan = Chan::Ordered,
        doc = "the server closes (or restarts); shown in `link`"),
    crate::record_info!(Leave, cap = CAP_QUEUES, flow = C2S | G2S, chan = Chan::Ordered,
        doc = "leave on purpose (game -> sidecar: leave the server and exit; sidecar -> server)"),
    crate::record_info!(Chat, cap = CAP_QUEUES, flow = C2S | G2S, chan = Chan::Ordered, doc = "chat line out"),
    crate::record_info!(ChatIn, cap = CAP_QUEUES, flow = S2C | S2G, chan = Chan::Ordered,
        doc = "chat line in (from_peer 0 = the server); event"),
    crate::record_info!(AdminStateHead, cap = CAP_STATE, flow = S2C | S2G, chan = Chan::Ordered,
        doc = "who is admin (+ masked bans for an admin); copied into slot `admin`"),
    crate::record_info!(Ping, cap = 0, flow = C2S, chan = Chan::Latest(STREAM_PING), doc = "clock probe"),
    crate::record_info!(Pong, cap = 0, flow = S2C, chan = Chan::Latest(STREAM_PING), doc = "clock probe answer"),
    crate::record_info!(Link, cap = CAP_STATE, flow = S2G, chan = Chan::None,
        doc = "the sidecar's connection view: status, my peer id, admin, link state, kick / reject, metrics"),
    crate::record_info!(DirectorState, cap = CAP_BUS, flow = LOCAL, chan = Chan::None, doc = "bus: Director heartbeat"),
    crate::record_info!(ConnState, cap = CAP_BUS, flow = LOCAL, chan = Chan::None, doc = "bus: connection state shown to the player"),
    crate::record_info!(Spectate, cap = CAP_BUS, flow = LOCAL, chan = Chan::None, doc = "bus: spectated player"),
    crate::record_info!(SpawnStatus, cap = CAP_BUS, flow = LOCAL, chan = Chan::None, doc = "bus: HSMPSync placement report"),
    crate::record_info!(SpawnRequest, cap = CAP_BUS, flow = LOCAL, chan = Chan::None, doc = "bus: Director asks for a (re-)placement"),
    crate::record_info!(TravelRequest, cap = CAP_BUS, flow = LOCAL, chan = Chan::None, doc = "bus: Menu travel request"),
    crate::record_info!(TravelAck, cap = CAP_BUS, flow = LOCAL, chan = Chan::None, doc = "bus: Director travel answer"),
    crate::record_info!(UiRequest, cap = CAP_BUS, flow = LOCAL, chan = Chan::None, doc = "bus: HUD UI request"),
    crate::record_info!(ReturnToLobby, cap = CAP_BUS, flow = LOCAL, chan = Chan::None, doc = "bus: reopen the lobby once per seq"),
    crate::record_info!(FallbackSwap, cap = CAP_BUS, flow = LOCAL, chan = Chan::None, doc = "bus: a transient possession swap for a fallback spawn"),
];

const fn bus(name: &'static str, kind: u16, world_scoped: bool, doc: &'static str) -> super::SlotInfo {
    super::SlotInfo { name, kind, form: super::SlotForm::Bus, dir: Dir::Local, cap: CAP_BUS, world_scoped, doc }
}

/// Named record slots of this domain.
pub const SLOTS: &[super::SlotInfo] = &[
    super::SlotInfo { name: "session", kind: K_SESSION, form: super::SlotForm::Slot, dir: Dir::SidecarToGame, cap: CAP_STATE,
        world_scoped: false, doc: "the last accepted session snapshot, as received" },
    super::SlotInfo { name: "link", kind: K_LINK, form: super::SlotForm::Slot, dir: Dir::SidecarToGame, cap: CAP_STATE,
        world_scoped: false, doc: "the sidecar's connection view (replaces .sidecar.json .link.json .metrics.json .kicked.json .reject.json)" },
    super::SlotInfo { name: "admin", kind: K_ADMIN_STATE, form: super::SlotForm::Slot, dir: Dir::SidecarToGame, cap: CAP_STATE,
        world_scoped: false, doc: "the last admin state, as received (replaces .admin.json)" },
    bus("director", K_DIRECTOR, false, "replaces .director.json"),
    bus("conn_state", K_CONN_STATE, false, "replaces .conn_state.json"),
    bus("spectate", K_SPECTATE, false, "replaces .spectate.json"),
    bus("spawn_status", K_SPAWN_STATUS, true, "replaces .spawn_status.json"),
    bus("spawn_request", K_SPAWN_REQUEST, false, "replaces .spawn_request.json"),
    bus("travel_request", K_TRAVEL_REQUEST, false, "replaces .travel_request.json"),
    bus("travel_ack", K_TRAVEL_ACK, false, "replaces .travel_ack.json"),
    bus("ui_request", K_UI_REQUEST, false, "replaces .ui_request.json"),
    bus("return_to_lobby", K_RETURN_TO_LOBBY, false, "replaces .return_to_lobby.flag"),
    bus("fallback_swap", K_FALLBACK_SWAP, false, "replaces .fallback_swap.json"),
];

// ---- helpers (Rust writers / readers) ------------------------------------------------------

/// The session slot's body: head + the full roster capacity.
pub type SessionBuf = crate::record::VarBuf<SessionHead, MAX_ROSTER>;
/// The admin slot's body.
pub type AdminBuf = crate::record::VarBuf<AdminStateHead, MAX_BANS>;

impl SessionHead {
    /// The round being loaded / counted down to, else the current round.
    pub fn pending_round(&self) -> u32 {
        if matches!(self.phase, phase::LOADING | phase::COUNTDOWN) { self.round.wrapping_add(1) } else { self.round }
    }
    /// The arena in force: the frozen one in a match, else the lobby config's.
    pub fn arena(&self) -> &Str<40> {
        if self.has_frozen.get() && self.phase != phase::LOBBY { &self.frozen.arena } else { &self.config.arena }
    }
}

impl Command {
    /// A command of `op` with everything else zero.
    pub fn new(cmd_id: u32, op: u8) -> Command {
        Command { cmd_id, op, ..Default::default() }
    }
}

/// Phase name (logs, events): lobby, loading, countdown, live, roundover, match_over, post_match, paused.
pub fn phase_name(p: u8) -> &'static str {
    match p {
        phase::LOBBY => "lobby",
        phase::LOADING => "loading",
        phase::COUNTDOWN => "countdown",
        phase::LIVE => "live",
        phase::ROUND_OVER => "roundover",
        phase::MATCH_OVER => "match_over",
        phase::POST_MATCH => "post_match",
        phase::PAUSED => "paused",
        _ => "unknown",
    }
}

/// `Bool` from a `bool`.
#[inline]
pub fn b(v: bool) -> Bool {
    Bool::from(v)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::record::{to_payload, view};

    fn cfg(arena: &str) -> SessionConfig {
        SessionConfig { rev: 3, best_of: 3, kit_mode: 1, max_fighters: 8, countdown_s: 3, arena: Str::new(arena), ..Default::default() }
    }

    fn row(seat: u8, peer: u32) -> RosterRow {
        RosterRow {
            peer_id: peer, seat, wins: seat as u32, spawn_id: (2 << 8) | seat as u32, spawn_pos: [1.0, 2.0, 3.0],
            spawn_yaw: 90.0, spawn_protect_ms: 3000, connected: b(peer != 0), alive: b(true), nick: Str::new("Willie"),
            ..Default::default()
        }
    }

    fn head(n: usize) -> SessionHead {
        SessionHead {
            epoch: 0xDEAD_BEEF, match_id: 99, seq: 7, round: 1, phase: phase::LOADING, winner_seat: NO_SEAT,
            has_frozen: b(true), config: cfg("Map_Arena_Alley"), frozen: cfg("Map_Arena_Pit"), n: n as u16,
            ..Default::default()
        }
    }

    #[test]
    fn sizes_are_stable() {
        assert_eq!(core::mem::size_of::<SessionConfig>(), 64);
        assert_eq!(core::mem::size_of::<SessionHead>(), 184);
        assert_eq!(core::mem::size_of::<RosterRow>(), 96);
        assert_eq!(core::mem::size_of::<Command>(), 152);
        assert_eq!(core::mem::size_of::<Link>(), 208);
        for r in RECORDS.iter().filter(|r| r.flow & (G2S | S2G) != 0 && r.max_rows == 0) {
            assert!(r.head.size() <= crate::ring::MAX_PAYLOAD, "{} must fit a ring record", r.name);
        }
        // A 16-player snapshot is 1.7 KB on the wire (v5 bincode: ~2.3 KB).
        assert_eq!(crate::record::payload_len::<SessionHead>(16), 184 + 16 * 96);
        assert!(core::mem::size_of::<SessionBuf>() < 8 * 1024);
    }

    #[test]
    fn session_round_trip_and_checks() {
        let rows: Vec<RosterRow> = (1..=4).map(|s| row(s, if s == 3 { 0 } else { 10 + s as u32 })).collect();
        let p = to_payload(&head(0), &rows);
        let v = view::<SessionHead>(&p).unwrap();
        assert_eq!(v.head.n, 4);
        assert_eq!(v.rows[1].peer_id, 12);
        assert_eq!(v.head.pending_round(), 2);
        assert_eq!(v.head.arena(), "Map_Arena_Pit");
        let mut bad = head(0);
        bad.phase = 8;
        assert_eq!(view::<SessionHead>(&to_payload(&bad, &rows)).unwrap_err(), Invalid::Range("phase"));
        let mut bad = head(0);
        bad.frozen.mode = 9;
        assert_eq!(view::<SessionHead>(&to_payload(&bad, &rows)).unwrap_err(), Invalid::Range("mode"));
        bad.has_frozen = b(false);
        assert!(view::<SessionHead>(&to_payload(&bad, &rows)).is_ok(), "an invalid frozen config is ignored when absent");
        let mut r = rows.clone();
        r[0].spawn_pos[0] = 2.0e7;
        assert_eq!(view::<SessionHead>(&to_payload(&head(0), &r)).unwrap_err(), Invalid::Range("spawn_pos"));
        let mut r = rows.clone();
        r[0].peer_id = 0;
        assert_eq!(view::<SessionHead>(&to_payload(&head(0), &r)).unwrap_err(), Invalid::Range("peer_id"));
        let mut r = rows.clone();
        r[0].role = 3;
        assert_eq!(view::<SessionHead>(&to_payload(&head(0), &r)).unwrap_err(), Invalid::Range("role"));
        let mut r = rows;
        r[0].spawn_yaw = f32::NAN;
        assert!(matches!(view::<SessionHead>(&to_payload(&head(0), &r)), Err(Invalid::Float(_))));
    }

    #[test]
    fn command_checks() {
        let ok = |c: &Command| view::<Command>(&to_payload(c, &[])).map(|_| ());
        assert_eq!(ok(&Command::new(0, cmd_op::START)), Err(Invalid::Range("cmd_id")));
        assert_eq!(ok(&Command::new(1, 0)), Err(Invalid::Range("op")));
        assert_eq!(ok(&Command::new(1, cmd_op::MAX + 1)), Err(Invalid::Range("op")));
        assert!(ok(&Command::new(1, cmd_op::START)).is_ok());
        assert_eq!(ok(&Command::new(1, cmd_op::PICK_ARENA)), Err(Invalid::Range("text")));
        assert!(ok(&Command { text: Str::new("Map_Arena_Pit"), ..Command::new(1, cmd_op::PICK_ARENA) }).is_ok());
        assert_eq!(ok(&Command::new(1, cmd_op::KICK)), Err(Invalid::Range("peer_id")));
        assert_eq!(ok(&Command { peer_id: 2, role: 4, ..Command::new(1, cmd_op::PROMOTE) }), Err(Invalid::Range("role")));
        assert_eq!(ok(&Command::new(1, cmd_op::SET_CONFIG)), Err(Invalid::Range("mask")));
        let mut c = Command::new(1, cmd_op::SET_CONFIG);
        c.patch.mask = cfg::BEST_OF;
        c.patch.best_of = 5;
        assert!(ok(&c).is_ok());
        c.patch.mask = 1 << 12;
        assert_eq!(ok(&c), Err(Invalid::Range("mask")));
        c.patch.mask = cfg::KIT_RULES;
        c.patch.kit_mode = 3;
        assert_eq!(ok(&c), Err(Invalid::Range("kit_mode")));
        c.patch.mask = cfg::ARENA;
        assert_eq!(ok(&c), Err(Invalid::Range("arena")));
    }

    #[test]
    fn small_record_checks() {
        assert!(view::<GameStatus>(&to_payload(&GameStatus { flags: status_flag::LOADED | status_flag::DEAD, ..Default::default() }, &[])).is_ok());
        assert_eq!(view::<GameStatus>(&to_payload(&GameStatus { flags: 1 << 9, ..Default::default() }, &[])).unwrap_err(), Invalid::Range("flags"));
        assert_eq!(view::<GameStatus>(&to_payload(&GameStatus { load_error: 10, ..Default::default() }, &[])).unwrap_err(), Invalid::Range("load_error"));
        assert_eq!(view::<Spawned>(&to_payload(&Spawned::default(), &[])).unwrap_err(), Invalid::Range("round"));
        assert_eq!(view::<Spawned>(&to_payload(&Spawned { round: 1, pos: [0.0, 0.0, 1e8], ..Default::default() }, &[])).unwrap_err(), Invalid::Range("pos"));
        assert_eq!(view::<Chat>(&to_payload(&Chat { text: Str::new("   ") }, &[])).unwrap_err(), Invalid::Range("text"));
        assert!(view::<Chat>(&to_payload(&Chat { text: Str::new("gg") }, &[])).is_ok());
        assert_eq!(view::<Leave>(&to_payload(&Leave { reason: 3, _r: [0; 7] }, &[])).unwrap_err(), Invalid::Range("reason"));
        assert_eq!(view::<Notice>(&to_payload(&Notice::default(), &[])).unwrap_err(), Invalid::Range("code"));
        assert_eq!(view::<Welcome>(&to_payload(&Welcome::default(), &[])).unwrap_err(), Invalid::Range("peer_id"));
        assert_eq!(view::<Link>(&to_payload(&Link { status: 8, ..Default::default() }, &[])).unwrap_err(), Invalid::Range("status"));
        assert_eq!(view::<ServerClosing>(&to_payload(&ServerClosing { reason: 4, ..Default::default() }, &[])).unwrap_err(), Invalid::Range("reason"));
        assert_eq!(view::<CmdResult>(&to_payload(&CmdResult::default(), &[])).unwrap_err(), Invalid::Range("cmd_id"));
        let bans = [BanRow { entry: Str::new("203.0.x.x#1a2b3c4d") }; 3];
        let ap = to_payload(&AdminStateHead { admin_peer: 2, n: 0, _r: 0 }, &bans);
        let a = view::<AdminStateHead>(&ap).unwrap();
        assert_eq!((a.head.n, a.rows[2].entry.as_str()), (3, Some("203.0.x.x#1a2b3c4d")));
        let pings = [PingRow { peer_id: 3, rtt_ms: 48, seat: 1, _r: 0 }];
        assert_eq!(view::<PingsHead>(&to_payload(&PingsHead::default(), &pings)).unwrap().rows[0].rtt_ms, 48);
    }

    #[test]
    fn hostile_bytes_never_panic() {
        // Every kind of this domain: truncations, extensions and byte flips of a valid
        // payload are either refused or valid; never a panic.
        let samples: Vec<(u16, Vec<u8>)> = vec![
            (K_SESSION, to_payload(&head(0), &[row(1, 10), row(2, 0)])),
            (K_COMMAND, to_payload(&Command { text: Str::new("x"), ..Command::new(5, cmd_op::PICK_ARENA) }, &[])),
            (K_LINK, to_payload(&Link { status: 1, reason: Str::new("é"), ..Default::default() }, &[])),
            (K_ADMIN_STATE, to_payload(&AdminStateHead::default(), &[BanRow::default()])),
            (K_CONN_STATE, to_payload(&ConnState::default(), &[])),
        ];
        let mut rng = 0x1234_5678u32;
        for (kind, p) in samples {
            assert!(super::super::check_payload(kind, &p).is_ok(), "{kind:#x} sample valid");
            for cut in 0..p.len() {
                let _ = super::super::check_payload(kind, &p[..cut]);
            }
            let mut long = p.clone();
            long.extend_from_slice(&[0; 9]);
            assert!(super::super::check_payload(kind, &long).is_err());
            for _ in 0..2000 {
                let mut m = p.clone();
                rng ^= rng << 13;
                rng ^= rng >> 17;
                rng ^= rng << 5;
                let i = rng as usize % m.len();
                m[i] ^= (rng >> 8) as u8 | 1;
                let _ = super::super::check_payload(kind, &m);
            }
        }
    }
}
