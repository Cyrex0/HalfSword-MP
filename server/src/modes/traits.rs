//! The `GameMode` trait, `ModeCtx`, and every type that crosses the
//! reducer <-> mode boundary. Times are server-clock milliseconds (`Ms`),
//! never ticks.
//!
//! The mode never sends packets or touches sockets. It reads the roster view
//! the reducer lends it through `ModeCtx`, pushes `ModeEvent`s into
//! `ctx.out`, and exposes its decisions through `status()`, `round_result()`
//! and `match_result()`. The reducer reconciles its phase from `status()`
//! after every hook call (docs/development/subsystems/modes.md).

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::str::FromStr;

/// Server-clock milliseconds (monotonic, the reducer's clock).
pub type Ms = u64;
/// Stable roster seat. The reducer allocates it at START and keeps it across
/// reconnects (same `player_key` => same seat). Modes key every score
/// by seat, so a reconnect restores the whole seat for free.
pub type SeatId = u32;
/// 0 = no team (FFA), 1..=8 teams, 255 = spectator.
pub type TeamId = u8;
pub const NO_TEAM: TeamId = 0;
pub const SPECTATOR_TEAM: TeamId = 255;
pub const MAX_TEAMS: u8 = 8;

/// Death causes. Values 0..=3 are the ones `server.rs` already puts on the
/// wire in `S2CDeath.cause` (server.rs L1234-1241); 4 is new for the zone.
pub type DeathCause = u8;
pub const DEATH_REPORTED: DeathCause = 0;
pub const DEATH_DAMAGE: DeathCause = 1;
pub const DEATH_VITALS: DeathCause = 2;
pub const DEATH_LEFT: DeathCause = 3;
pub const DEATH_ZONE: DeathCause = 4;

/// Who scores: a single seat (Duel, FFA) or a team (LTS).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum Side {
    Seat(SeatId),
    Team(TeamId),
}

/// Identity of a seat, given at `on_config` / `on_join`. `key` is the stable
/// identity string (player_key fingerprint, or the nick without one) and
/// is the deterministic tie-break for team balance.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SeatInfo {
    pub seat: SeatId,
    pub key: String,
    /// Skill estimate for balance (Elo later; 0 when unknown).
    pub rating: i32,
}

/// What the reducer lends the mode about one connected seat, every call.
#[derive(Clone, Debug, PartialEq)]
pub struct SeatView {
    pub seat: SeatId,
    /// Connected, and (during a live round) its game is still pinging.
    /// Exactly server.rs `present_participants` (L1375-1386).
    pub present: bool,
    /// Last accepted root position (UE cm), if any.
    pub pos: Option<[f32; 3]>,
    /// Server ledger upper-bound health estimate (Willie Health = 100).
    pub ledger_health: f32,
}

/// Something the mode wants the reducer to do or publish. Drained by the
/// reducer after the hook returns (same pattern as `Inner::out`/`flush_out`).
#[derive(Clone, Debug, PartialEq)]
pub enum ModeEvent {
    /// A HUD banner / server chat line.
    Banner { text: String },
    /// Kill feed entry (`S2CEvent::KillFeed`).
    KillFeed { victim: SeatId, killer: Option<SeatId>, cause: DeathCause, assists: Vec<SeatId> },
    /// Environmental damage to add to the server ledger as a synthetic entry
    /// (attacker 0, cause `DEATH_ZONE`). A lethal sum is declared as a death
    /// by the reducer's normal ledger path, which then calls `on_death`.
    ZoneDamage { seat: SeatId, amount: f32 },
}

/// The borrow of reducer state handed to every hook.
pub struct ModeCtx<'a> {
    pub now_ms: Ms,
    /// Every connected seat (participants and spectators).
    pub roster: &'a [SeatView],
    pub out: &'a mut Vec<ModeEvent>,
}

impl<'a> ModeCtx<'a> {
    pub fn new(now_ms: Ms, roster: &'a [SeatView], out: &'a mut Vec<ModeEvent>) -> Self {
        Self { now_ms, roster, out }
    }
    pub fn view(&self, seat: SeatId) -> Option<&SeatView> {
        self.roster.iter().find(|v| v.seat == seat)
    }
    pub fn is_present(&self, seat: SeatId) -> bool {
        self.view(seat).map(|v| v.present).unwrap_or(false)
    }
    pub fn emit(&mut self, ev: ModeEvent) {
        self.out.push(ev);
    }
}

/// Frozen match configuration (`SetConfig.mode_opts`, `set_mode_opt:k=v`).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ModeConfig {
    pub opts: BTreeMap<String, String>,
    /// Arena centre from MapData (UE cm); the sudden-death ring centres on it.
    pub map_center: Option<[f32; 3]>,
}

impl ModeConfig {
    pub fn with(mut self, k: &str, v: &str) -> Self {
        self.opts.insert(k.to_string(), v.to_string());
        self
    }
    /// Parse `k` or return `default`. A present but unparsable value is an error.
    pub fn get<T: FromStr>(&self, k: &str, default: T) -> Result<T, String> {
        match self.opts.get(k) {
            None => Ok(default),
            Some(v) => v.trim().parse::<T>().map_err(|_| format!("bad value for {k}: '{v}'")),
        }
    }
    /// Reject keys this mode does not understand (admin typo feedback).
    pub fn check_known(&self, known: &[&str]) -> Result<(), String> {
        for k in self.opts.keys() {
            if !known.contains(&k.as_str()) {
                return Err(format!("unknown mode option '{k}' (known: {})", known.join(", ")));
            }
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TeamLayout {
    None,
    Teams { min: u8, max: u8 },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ModeInfo {
    /// Wire/browser id: "duel", "ffa", "lts", ...
    pub id: &'static str,
    pub label: &'static str,
    pub min_players: u8,
    pub max_players: u8,
    pub teams: TeamLayout,
    /// true => reload-per-round respawn path (every mode in this crate).
    pub round_based: bool,
    pub needs_midround_respawn: bool,
    pub needs_loot: bool,
    pub needs_ai: bool,
    pub map_tags: &'static [&'static str],
}

/// Decision for a seat that connects while a match runs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum JoinRole {
    /// Spectate now; fight from the next round the reducer lists it as a
    /// fighter in `on_round_start` (server.rs L1671-1682).
    NextRound,
    Spectate,
}

/// What a declared death means for the victim.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DeathOutcome {
    /// Not counted: no open round, or the seat is not alive (duplicate,
    /// stale, spectator). The reducer must NOT broadcast `S2CDeath`.
    /// Mirrors `declare_death` returning false (server.rs L1261-1268).
    Ignored,
    /// Out until the next round (every round-based mode).
    OutForRound,
    RespawnAfter(Ms),
    RespawnAtWave,
    Eliminated,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum DamageVerdict {
    Allow,
    Scale(f32),
    Deny(&'static str),
}

#[derive(Clone, Debug, PartialEq)]
pub struct SpawnOrder {
    pub seat: SeatId,
    pub pos: [f32; 3],
    pub yaw: f32,
    pub protect_ms: u32,
}

#[derive(Clone, Debug, PartialEq)]
pub enum SpawnPolicy {
    /// Spawn table order, maximum separation (as in spawns.rs).
    Fixed,
    /// Team `t` uses map side `side` this round (sides rotate per round).
    TeamSides { sides: Vec<(TeamId, u8)> },
    FarthestFromEnemies { min_cm: f32 },
    Custom(Vec<SpawnOrder>),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum KitPolicy {
    /// The player's own selection, validated under the session kit rules.
    PlayerChoice,
    ForceClass(String),
    Budget(u16),
    /// "Start in Pants".
    Naked,
}

/// Per-seat kit override. The reducer applies whichever compensation fits
/// the frozen session kit rules: `budget_bonus` under CUSTOM,
/// `class_upgrade` steps under CLASSES ONLY (teams::upgrade_class).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct KitOverride {
    pub policy: KitPolicy,
    pub budget_bonus: u16,
    pub class_upgrade: u8,
    /// Forced cloth tint index (`cosmetic[2]`), e.g. the team colour.
    pub tint: Option<u8>,
}

impl Default for KitOverride {
    fn default() -> Self {
        Self { policy: KitPolicy::PlayerChoice, budget_bonus: 0, class_upgrade: 0, tint: None }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum VoidReason {
    /// A participant dropped; the round is replayed after the pause.
    Paused,
    /// Everyone left; back to the lobby.
    Abandoned,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum RoundOutcome {
    Won(Side),
    /// Nobody left standing at the settle deadline (mutual kill), or a
    /// sudden-death tiebreak with no unique leader.
    Draw,
    /// The round never got a result (pause / abandon). Not counted.
    Void(VoidReason),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RoundResult {
    pub round: u32,
    pub outcome: RoundOutcome,
    pub decided_at_ms: Ms,
    /// Decided by the sudden-death tiebreak rather than last-standing.
    pub tiebreak: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum MatchEnd {
    /// A side reached the win target.
    Score,
    /// The round cap was reached; leader by wins, then kills.
    RoundCap,
    /// The other side(s) left and did not come back (pause expiry / barrier).
    Forfeit,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MatchResult {
    /// None = match draw (round cap with a tied leader).
    pub winner: Option<Side>,
    pub end: MatchEnd,
    /// Last round number started.
    pub rounds: u32,
    pub scores: Vec<(Side, u32)>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum PauseReason {
    OpponentLeft,
}

/// The mode's view of the match flow. The reducer maps it to its phase
/// (the reconcile table in docs/development/subsystems/modes.md).
#[derive(Clone, Debug, PartialEq)]
pub enum ModeStatus {
    /// Not configured / reset: lobby.
    Idle,
    /// Match running, no round in progress, nothing to show: run a countdown.
    /// `replay` = a pause was resolved by the players returning.
    Waiting { replay: bool },
    Live { round: u32 },
    /// <= 1 side standing; the result is provisional until `until_ms` and
    /// combat stays open for trade hits (server.rs L1226-1232).
    Settling { round: u32, until_ms: Ms },
    /// The round result is fixed; show it, then countdown.
    RoundOver(RoundResult),
    MatchOver(MatchResult),
    Paused { until_ms: Ms, reason: PauseReason },
    /// Everyone left: back to the lobby.
    Abandoned,
}

impl ModeStatus {
    /// Hits and deaths count (server.rs `combat_open`, L1246-1248).
    pub fn combat_open(&self) -> bool {
        matches!(self, ModeStatus::Live { .. } | ModeStatus::Settling { .. })
    }
    /// The legacy phase string (`S2CMatchState.state`).
    pub fn phase_str(&self) -> &'static str {
        match self {
            ModeStatus::Idle | ModeStatus::Abandoned => "lobby",
            ModeStatus::Waiting { .. } => "countdown",
            ModeStatus::Live { .. } => "live",
            ModeStatus::Settling { .. } | ModeStatus::RoundOver(_) => "roundover",
            ModeStatus::MatchOver(_) => "match_over",
            ModeStatus::Paused { .. } => "paused",
        }
    }
}

// ---- HUD section (S2CSession.mode) ------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum RoundStateTag {
    Idle,
    Waiting,
    Live,
    Settling,
    RoundOver,
    MatchOver,
    Paused,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SideRec {
    pub side: Side,
    pub score: u32,
    pub alive: u8,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SeatRec {
    pub seat: SeatId,
    pub team: TeamId,
    pub kills: u16,
    pub deaths: u16,
    pub assists: u16,
    pub alive: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ZoneHud {
    pub phase: u8,
    pub center: [f32; 2],
    pub radius_cm: f32,
    pub next_center: [f32; 2],
    pub next_radius_cm: f32,
    pub shrinking: bool,
    /// Server-clock ms at which the current wait/shrink segment ends.
    pub segment_end_ms: Ms,
    pub dps: f32,
}

/// The tagged objective part of the section (`None`,
/// `Hill`, `Wave`, `Bracket`, `Zone`; only the ones that exist so far).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Objective {
    None,
    Zone(ZoneHud),
}

/// `S2CSession.mode`: full state, idempotent, same for everyone.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ModeState {
    pub mode: String,
    pub round: u32,
    pub state: RoundStateTag,
    /// Settle / pause / round-limit deadline (server ms), 0 = none.
    pub deadline_ms: Ms,
    pub target_wins: u32,
    /// 0 = no round cap.
    pub max_rounds: u32,
    pub sides: Vec<SideRec>,
    pub seats: Vec<SeatRec>,
    pub last_result: Option<RoundResult>,
    pub objective: Objective,
}

// ---- the trait ----------------------------------------------------------------

/// A game mode: a pure state machine over `ModeCtx`. All hooks are called
/// under the reducer's lock and must never block.
pub trait GameMode: Send {
    fn info(&self) -> &ModeInfo;

    // ---- lifecycle ----
    /// START: validate + freeze options, seat the participants (teams). Resets
    /// all match state. Ok => status `Waiting { replay: false }`.
    fn on_config(&mut self, cfg: &ModeConfig, roster: &[SeatInfo]) -> Result<(), String>;
    /// countdown -> live. `fighters` are the seats that loaded this round's
    /// arena (late joiners included; stragglers excluded). Returns false and
    /// changes nothing if `round` is not newer than the last round started or
    /// no round may start now.
    fn on_round_start(&mut self, ctx: &mut ModeCtx, round: u32, fighters: &[SeatId]) -> bool;
    /// Every reducer tick (`ctx.now_ms`): supervision (pause / abandon /
    /// forfeit), the settle deadline, round limit and zone.
    fn on_tick(&mut self, ctx: &mut ModeCtx);
    fn on_death(&mut self, ctx: &mut ModeCtx, victim: SeatId, killer: Option<SeatId>, cause: DeathCause) -> DeathOutcome;
    /// Accepted damage (after `friendly_fire`), for assists. Optional.
    fn on_damage(&mut self, _ctx: &mut ModeCtx, _attacker: SeatId, _victim: SeatId, _amount: f32) {}
    /// A seat's connection is gone (`ctx.roster` already excludes it).
    fn on_disconnect(&mut self, ctx: &mut ModeCtx, seat: SeatId);
    /// A seat connected mid-match (new or reconnecting).
    fn on_join(&mut self, ctx: &mut ModeCtx, seat: &SeatInfo) -> JoinRole;
    /// The load barrier timed out; `loaded` are the present seats that loaded.
    fn on_barrier_timeout(&mut self, ctx: &mut ModeCtx, loaded: &[SeatId]);
    /// The latest fixed round result (`None` before the first one).
    fn round_result(&self) -> Option<&RoundResult>;
    /// Every round result of this match, one per started round, in order.
    fn round_results(&self) -> &[RoundResult];
    fn match_result(&self) -> Option<&MatchResult>;
    fn status(&self) -> &ModeStatus;
    /// match_over -> lobby: forget the match (status `Idle`).
    fn reset(&mut self);

    // ---- policy hooks ----
    fn spawn_policy(&self, ctx: &ModeCtx, round: u32) -> SpawnPolicy;
    fn kit_override(&self, ctx: &ModeCtx, seat: SeatId) -> KitOverride;
    fn friendly_fire(&self, attacker: SeatId, victim: SeatId) -> DamageVerdict;
    fn team_of(&self, _seat: SeatId) -> TeamId {
        NO_TEAM
    }
    /// Admin `team:<seat>:<n>`; only before the first round.
    fn admin_set_team(&mut self, _seat: SeatId, _team: TeamId) -> Result<(), String> {
        Err("this mode has no teams".into())
    }

    // ---- presentation ----
    fn hud_section(&self, now_ms: Ms) -> ModeState;
}
