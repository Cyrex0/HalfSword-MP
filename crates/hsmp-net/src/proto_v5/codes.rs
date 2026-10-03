//! Numeric codes used inside v5 messages. Codes are `u8`/`u16` on the wire
//! (cheaper than bincode's 4-byte enum tags) and append-only.
#![allow(non_snake_case)]

/// `S2CSession.phase`.
pub mod Phase {
    pub const LOBBY: u8 = 0;
    pub const LOADING: u8 = 1;
    pub const COUNTDOWN: u8 = 2;
    pub const LIVE: u8 = 3;
    pub const ROUND_OVER: u8 = 4;
    pub const MATCH_OVER: u8 = 5;
    pub const POST_MATCH: u8 = 6;
    pub const PAUSED: u8 = 7;

    pub fn name(p: u8) -> &'static str {
        match p {
            LOBBY => "lobby",
            LOADING => "loading",
            COUNTDOWN => "countdown",
            LIVE => "live",
            ROUND_OVER => "roundover",
            MATCH_OVER => "match_over",
            POST_MATCH => "post_match",
            PAUSED => "paused",
            _ => "unknown",
        }
    }
}

/// `SessionConfig.mode`.
pub mod Mode {
    pub const DUEL: u8 = 0;
    pub const FFA: u8 = 1;
    pub const TEAM_ELIM: u8 = 2;
    pub const KING_OF_HILL: u8 = 3;
}

/// `SessionConfig.team_rule`.
pub mod TeamRule {
    pub const NONE: u8 = 0;
    pub const AUTO: u8 = 1;
    pub const FIXED: u8 = 2;
}

/// `SessionConfig.join_in_progress`.
pub mod Jip {
    pub const SPECTATE: u8 = 0;
    pub const NEXT_ROUND: u8 = 1;
    pub const NEVER: u8 = 2;
}

/// `RosterEntry.role`, `S2CWelcome.role`, `Command::SwitchRole`.
pub mod Role {
    pub const FIGHTER: u8 = 0;
    pub const SPECTATOR: u8 = 1;
    pub const QUEUED: u8 = 2;
}

/// `RosterEntry.admin_role`, `Command::Promote`.
pub mod AdminRole {
    pub const NONE: u8 = 0;
    pub const MODERATOR: u8 = 1;
    pub const ADMIN: u8 = 2;
    pub const OWNER: u8 = 3;
}

/// `RoundResult.reason`.
pub mod ResultReason {
    pub const NONE: u8 = 0;
    pub const KILL: u8 = 1;
    pub const DRAW: u8 = 2;
    pub const FORFEIT: u8 = 3;
    pub const OPPONENT_LEFT: u8 = 4;
    pub const TIME_LIMIT: u8 = 5;
    pub const ABORTED: u8 = 6;
}

/// `S2CCommandResult.reason_code`.
pub mod CmdReason {
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
}

/// `C2SGameStatus.flags`.
pub mod status_flags {
    /// The world for `(match_id, round, arena)` is loaded.
    pub const LOADED: u32 = 1 << 0;
    /// The spawn pipeline finished (pawn verified, dressed, placed).
    pub const READY: u32 = 1 << 1;
    pub const DEAD: u32 = 1 << 2;
    pub const IN_MENU: u32 = 1 << 3;
    pub const SPECTATING: u32 = 1 << 4;
    /// The game process is paused/minimised (informational).
    pub const BACKGROUND: u32 = 1 << 5;
}

/// `C2SGameStatus.load_error`.
pub mod LoadError {
    pub const NONE: u8 = 0;
    pub const TRAVEL_FAILED: u8 = 1;
    pub const WRONG_WORLD: u8 = 2;
    pub const NO_PAWN: u8 = 3;
    pub const VITALS: u8 = 4;
    pub const SPAWN_PLACE: u8 = 5;
    pub const DRESS: u8 = 6;
    pub const STAND_INS: u8 = 7;
    pub const TIMEOUT: u8 = 8;
}

/// `Event::Notice.code`.
pub mod Notice {
    pub const HOST_LEFT: u16 = 1;
    pub const ROLE_CHANGED: u16 = 2;
    pub const LOAD_FAILED: u16 = 3;
    pub const PLAYER_JOINED: u16 = 4;
    pub const PLAYER_LEFT: u16 = 5;
    pub const ADMIN_CHANGED: u16 = 6;
    pub const CONFIG_QUEUED: u16 = 7;
    pub const SUDDEN_DEATH: u16 = 8;
}

/// `S2CServerClosing.reason`.
pub mod ClosingReason {
    pub const SHUTDOWN: u8 = 0;
    pub const HOST_LEFT: u8 = 1;
    pub const RESTART: u8 = 2;
    pub const IDLE: u8 = 3;
}

/// `C2SLeave.reason`.
pub mod LeaveReason {
    pub const USER: u8 = 0;
    pub const GAME_EXITED: u8 = 1;
    pub const SWITCH_SERVER: u8 = 2;
}
