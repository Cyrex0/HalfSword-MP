//! Game modes (docs/development/subsystems/modes.md): the rules on top of the round flow
//! of match_core.rs.
//!
//! * the mode config (lobby-editable, frozen at START): mode, teams (AUTO / FIXED), the
//!   round clock and the mode options (King of the hill target, friendly fire, respawn delay);
//! * teams: assignment at START (balanced, or the players' lobby picks), late joiners to the
//!   smallest team, a side per team for the round end, wins per team;
//! * the server's friendly-fire verdict on the hit path;
//! * King of the hill: a zone per arena, points for standing in it alone (validated roots);
//! * weapon roulette / brawl: the kit imposed on everyone for the round (loadout.rs);
//! * timed deathmatch: respawn orders, revive on the client's placement report;
//! * kills / deaths, the `kill_feed`, `mode` and `zone` records for peers with
//!   `caps::MODES` / `caps::ZONE`.
//!
//! Everything here is sync and runs under the state lock; match_core.rs calls the hooks.

use super::*;
use hsmp_ipc::layout::{Bool, Str};
use hsmp_ipc::schema::session::{self as rec, game_mode as gm, mode_opt, mode_result};
use hsmp_ipc::wire;
use hsmp_net::net::caps;
use v5::CmdReason;

/// Teams a mode can field.
pub(crate) const MAX_TEAMS: u8 = rec::MAX_TEAMS;
/// Team names (chat lines, RCON, the HUD uses its own).
pub(crate) const TEAM_NAMES: [&str; 4] = ["Red", "Blue", "Green", "Gold"];
/// King of the hill: seconds held alone that win a round (default, accepted range).
pub(crate) const KOTH_TARGET_S: u16 = 60;
pub(crate) const KOTH_TARGET_RANGE: std::ops::RangeInclusive<u16> = 10..=600;
/// Round clock when the config leaves it at 0: deathmatch 5 min, King of the hill 4 min.
pub(crate) const DM_TIME_S: u16 = 300;
pub(crate) const KOTH_TIME_S: u16 = 240;
/// Longest round clock a config accepts (30 min).
pub(crate) const MAX_ROUND_TIME_S: u16 = 1800;
/// Deathmatch: death -> respawn order (default, accepted range).
pub(crate) const RESPAWN_S: u8 = 3;
pub(crate) const RESPAWN_RANGE: std::ops::RangeInclusive<u8> = 1..=30;
/// A respawn order whose placement the client never reports is issued again after this.
pub(crate) const RESPAWN_LOAD_MS: u64 = 60_000;
/// After a revive: no damage to or from the player, no death (the reloaded world settles).
pub(crate) const RESPAWN_PROTECT_MS: u64 = 2000;
/// Sudden death after a tied deathmatch clock: the next kill decides, else a draw after this.
pub(crate) const SUDDEN_DEATH_MS: u64 = 60_000;
/// The `mode` / `zone` records go out on change, at most this often, and at least every
/// `MODE_EVERY_MS` in a match (they ride the reliable-latest channel: newest wins).
const MODE_MIN_GAP_MS: u64 = 100;
const MODE_EVERY_MS: u64 = 1000;

/// The mode config (`SessionConfig` mode fields + the options SET_OPTION sets).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ModeCfg {
    pub mode: u8,
    pub team_rule: u8,
    /// Team count asked for (2..=4); used when the mode has teams.
    pub teams: u8,
    /// 0 = the mode's default (none, or DM_TIME_S / KOTH_TIME_S).
    pub round_time_s: u16,
    pub koth_target_s: u16,
    pub friendly_fire: bool,
    pub respawn_s: u8,
}

impl Default for ModeCfg {
    fn default() -> Self {
        ModeCfg {
            mode: gm::DUEL, team_rule: v5::TeamRule::NONE, teams: 2, round_time_s: 0,
            koth_target_s: KOTH_TARGET_S, friendly_fire: false, respawn_s: RESPAWN_S,
        }
    }
}

impl ModeCfg {
    /// Teams in play (0 = every player for themselves). Team elimination always has teams;
    /// duel and FFA never; the other modes when the team rule asks for them.
    pub fn team_count(&self) -> u8 {
        match self.mode {
            gm::DUEL | gm::FFA => 0,
            gm::TEAM_ELIM => self.teams.clamp(2, MAX_TEAMS),
            _ if self.team_rule != v5::TeamRule::NONE => self.teams.clamp(2, MAX_TEAMS),
            _ => 0,
        }
    }
    /// The team rule in force (`NONE` without teams; team elimination defaults to AUTO).
    pub fn rule(&self) -> u8 {
        if self.team_count() == 0 { v5::TeamRule::NONE }
        else if self.team_rule == v5::TeamRule::NONE { v5::TeamRule::AUTO }
        else { self.team_rule }
    }
    /// The round clock in force (s; 0 = none).
    pub fn round_time(&self) -> u16 {
        match (self.mode, self.round_time_s) {
            (gm::DEATHMATCH, 0) => DM_TIME_S,
            (gm::KING_OF_HILL, 0) => KOTH_TIME_S,
            (_, t) => t,
        }
    }
    pub fn respawns(&self) -> bool { self.mode == gm::DEATHMATCH }
    pub fn koth(&self) -> bool { self.mode == gm::KING_OF_HILL }
}

/// The mode's id (`--mode`, RCON, logs).
pub(crate) fn mode_name(m: u8) -> &'static str {
    match m {
        gm::FFA => "ffa",
        gm::TEAM_ELIM => "teams",
        gm::KING_OF_HILL => "koth",
        gm::ROULETTE => "roulette",
        gm::BRAWL => "brawl",
        gm::DEATHMATCH => "deathmatch",
        _ => "duel",
    }
}

/// The mode's name for players.
pub(crate) fn mode_label(m: u8) -> &'static str {
    match m {
        gm::FFA => "Free for all",
        gm::TEAM_ELIM => "Team elimination",
        gm::KING_OF_HILL => "King of the hill",
        gm::ROULETTE => "Weapon roulette",
        gm::BRAWL => "Brawl",
        gm::DEATHMATCH => "Deathmatch",
        _ => "Duel",
    }
}

/// `--mode` / RCON / HSMP_SERVER_MODE names -> code (None = unknown).
pub(crate) fn parse_mode(s: &str) -> Option<u8> {
    Some(match s.trim().to_ascii_lowercase().as_str() {
        "duel" | "lms" | "last_standing" => gm::DUEL,
        "ffa" | "ffa_lms" | "free_for_all" => gm::FFA,
        "teams" | "team" | "lts" | "team_elim" | "team_elimination" => gm::TEAM_ELIM,
        "koth" | "king" | "king_of_hill" | "king_of_the_hill" => gm::KING_OF_HILL,
        "roulette" | "weapon_roulette" => gm::ROULETTE,
        "brawl" | "fists" => gm::BRAWL,
        "dm" | "deathmatch" | "timed_deathmatch" => gm::DEATHMATCH,
        _ => return None,
    })
}

/// Every mode but duel / FFA needs `caps::MODES` on every peer (older clients refuse
/// mode codes above KING_OF_HILL, and none of them shows teams, scores or respawns).
pub(crate) fn needs_modes_cap(m: u8) -> bool {
    !matches!(m, gm::DUEL | gm::FFA)
}

/// Who wins a round: a player, or a team.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) enum Side {
    Player(PeerId),
    Team(u8),
}

/// Why a round is ending, when it is not the last side standing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RoundEnd {
    /// King of the hill: this side reached the target.
    Objective(Side),
    /// The round clock ran out.
    Time,
    /// The kill that broke a tied deathmatch clock.
    SuddenDeath(Side),
    /// Sudden death ran out without a kill.
    SuddenDeathOver,
}

/// Per-player numbers (by player key: a reconnect keeps them).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct Stats {
    pub kills: u16,
    pub deaths: u16,
    pub round_kills: u16,
    /// Lives started this round (1 + respawns).
    pub life: u16,
    /// King of the hill: ms held alone this round.
    pub score_ms: u64,
}

/// A deathmatch respawn in progress.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Respawn {
    /// Server ms the order is (or was) issued.
    pub at_ms: u64,
    /// The order's spawn id (0 = not issued yet).
    pub spawn_id: u32,
}

/// The imposed kit of a weapon roulette / brawl round.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RoundKit {
    pub class: &'static str,
    pub r: &'static str,
    pub l: &'static str,
    pub armor: &'static [&'static str],
    pub label: String,
}

impl RoundKit {
    pub fn selection(&self) -> crate::loadout::KitSel {
        crate::loadout::KitSel::new(self.class, self.r, self.l, self.armor, [0; 4])
    }
}

/// King of the hill zone: a vertical cylinder.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Zone {
    pub center: [f32; 3],
    pub radius_cm: f32,
    pub half_height_cm: f32,
}

impl Zone {
    pub fn contains(&self, p: [f32; 3]) -> bool {
        let (dx, dy) = (p[0] - self.center[0], p[1] - self.center[1]);
        dx * dx + dy * dy <= self.radius_cm * self.radius_cm && (p[2] - self.center[2]).abs() <= self.half_height_cm
    }
}

/// Who holds the zone right now (the `zone` record).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct Hold {
    pub holder: Option<Side>,
    pub contested: bool,
    pub inside: u8,
}

/// Mode state of the server (`Inner::modes`).
#[derive(Debug, Default)]
pub(crate) struct ModeCore {
    /// The lobby config (what the next START freezes).
    pub cfg: ModeCfg,
    /// The config frozen at START (None in the lobby).
    pub run: Option<ModeCfg>,
    /// FIXED teams: the players' lobby picks (1..=teams), by player key.
    pub picks: HashMap<session::PlayerKey, u8>,
    /// This match's teams, by player key.
    pub teams: HashMap<session::PlayerKey, u8>,
    pub team_wins: [u32; 4],
    /// King of the hill: ms each team held the zone alone this round.
    pub team_score_ms: [u64; 4],
    pub stats: HashMap<session::PlayerKey, Stats>,
    /// Round clock left (ms) while it runs (counts only while Live; a pause stops it).
    pub clock_ms: Option<u64>,
    pub sudden_death: Option<u64>,
    pub end: Option<RoundEnd>,
    /// `mode_result` of the last round, its winning team.
    pub result: u8,
    pub winner_team: u8,
    /// Roulette / brawl kit of the pending / current round, and its generation (loadout.rs
    /// applies a new generation after the lock).
    pub kit: Option<RoundKit>,
    pub kit_gen: u32,
    pub respawns: HashMap<PeerId, Respawn>,
    /// Revived players: no damage / death until this server ms.
    pub protect: HashMap<PeerId, u64>,
    pub zone: Option<Zone>,
    pub hold: Hold,
    pub dirty: bool,
    seq: u32,
    sent_ms: u64,
}

impl ModeCore {
    /// The config in force: the frozen one in a match, else the lobby's.
    pub fn now(&self) -> &ModeCfg { self.run.as_ref().unwrap_or(&self.cfg) }
}

// ---- config ---------------------------------------------------------------------------------

/// Defaults from the command line / environment (main.rs `--mode` and friends).
pub(crate) fn configure(inner: &mut Inner, cfg: ModeCfg) {
    inner.modes.cfg = cfg;
    inner.match_state_dirty = true;
    info!(mode = mode_name(cfg.mode), teams = cfg.team_count(), rule = cfg.rule(), round_time_s = cfg.round_time(),
          koth_target_s = cfg.koth_target_s, ff = cfg.friendly_fire, respawn_s = cfg.respawn_s, "game mode (startup)");
}

/// The connected peers whose client lacks `caps::MODES` (nicks), for the compatibility gate.
pub(crate) fn legacy_peers(inner: &Inner) -> Vec<String> {
    let mut v: Vec<String> = inner.peers.values()
        .filter(|p| crate::interact::peer_caps(p.id) & caps::MODES == 0)
        .map(|p| p.nick.clone())
        .collect();
    v.sort();
    v
}

/// A joiner without `caps::MODES`: refused while the lobby or the running match uses a mode
/// that needs it. The reject text.
pub(crate) fn join_refusal(inner: &Inner, peer_caps: u64) -> Option<String> {
    if peer_caps & caps::MODES != 0 { return None; }
    let m = [inner.modes.now().mode, inner.modes.cfg.mode].into_iter().find(|m| needs_modes_cap(*m))?;
    Some(format!("this server is playing {}, which needs a newer HSMP: update HSMP to join", mode_label(m)))
}

/// SET_CONFIG's mode fields (lobby only; checked by the caller). Err = (reason code, text).
pub(crate) fn set_cfg(inner: &mut Inner, mode: Option<u8>, team_rule: Option<u8>, teams: Option<u8>,
                      round_time_s: Option<u16>, by: &str) -> Result<(), (u16, String)> {
    let mut c = inner.modes.cfg;
    if let Some(m) = mode {
        if m > gm::MAX { return Err((CmdReason::INVALID_VALUE, format!("unknown mode {}", m))); }
        c.mode = m;
    }
    if let Some(r) = team_rule {
        if r > v5::TeamRule::FIXED { return Err((CmdReason::INVALID_VALUE, "team rule must be 0 none, 1 auto, 2 fixed".into())); }
        c.team_rule = r;
    }
    if let Some(t) = teams {
        if !(2..=MAX_TEAMS).contains(&t) { return Err((CmdReason::INVALID_VALUE, format!("teams must be 2..={}", MAX_TEAMS))); }
        c.teams = t;
    }
    if let Some(t) = round_time_s {
        if t > MAX_ROUND_TIME_S { return Err((CmdReason::INVALID_VALUE, format!("round time must be 0..={} s", MAX_ROUND_TIME_S))); }
        c.round_time_s = t;
    }
    if needs_modes_cap(c.mode) && c.mode != inner.modes.cfg.mode {
        let old = legacy_peers(inner);
        if !old.is_empty() {
            return Err((CmdReason::UNSUPPORTED, format!("{} needs a newer HSMP on every player ({} must update)",
                mode_label(c.mode), old.join(", "))));
        }
    }
    if c != inner.modes.cfg {
        info!(by = %by, mode = mode_name(c.mode), teams = c.team_count(), rule = c.rule(), round_time_s = c.round_time(),
              "game mode set");
        crate::events::emit("mode_set", serde_json::json!({
            "mode": mode_name(c.mode), "teams": c.team_count(), "team_rule": c.rule(), "round_time_s": c.round_time(), "by": by,
        }));
        if c.rule() != v5::TeamRule::FIXED || c.team_count() != inner.modes.cfg.team_count() {
            inner.modes.picks.retain(|_, t| *t <= c.team_count());
        }
        inner.modes.cfg = c;
        inner.modes.dirty = true;
        inner.match_state_dirty = true;
    }
    Ok(())
}

/// SET_OPTION (lobby only; admin checked by the caller).
pub(crate) fn set_option(inner: &mut Inner, opt: u8, value: u32, by: &str) -> Result<String, (u16, String)> {
    let c = &mut inner.modes.cfg;
    let text = match opt {
        mode_opt::KOTH_TARGET => {
            let v = u16::try_from(value).ok().filter(|v| KOTH_TARGET_RANGE.contains(v)).ok_or_else(|| (CmdReason::INVALID_VALUE,
                format!("the King of the hill target must be {}..={} s", KOTH_TARGET_RANGE.start(), KOTH_TARGET_RANGE.end())))?;
            c.koth_target_s = v;
            format!("King of the hill target {} s", v)
        }
        mode_opt::FRIENDLY_FIRE => {
            if value > 1 { return Err((CmdReason::INVALID_VALUE, "friendly fire is 0 (off) or 1 (on)".into())); }
            c.friendly_fire = value == 1;
            format!("friendly fire {}", if value == 1 { "on" } else { "off" })
        }
        mode_opt::RESPAWN_S => {
            let v = u8::try_from(value).ok().filter(|v| RESPAWN_RANGE.contains(v)).ok_or_else(|| (CmdReason::INVALID_VALUE,
                format!("the respawn delay must be {}..={} s", RESPAWN_RANGE.start(), RESPAWN_RANGE.end())))?;
            c.respawn_s = v;
            format!("respawn delay {} s", v)
        }
        _ => return Err((CmdReason::INVALID_VALUE, format!("unknown option {}", opt))),
    };
    info!(by = %by, option = opt, value, "{}", text);
    inner.modes.dirty = true;
    inner.match_state_dirty = true;
    Ok(text)
}

/// SET_TEAM in the lobby: `who` picks team `team` (0 = no pick: the smallest team at START).
pub(crate) fn pick_team(inner: &mut Inner, who: session::PlayerKey, team: u8) -> Result<String, (u16, String)> {
    let c = inner.modes.cfg;
    if c.team_count() == 0 {
        return Err((CmdReason::UNSUPPORTED, format!("{} has no teams", mode_label(c.mode))));
    }
    if c.rule() != v5::TeamRule::FIXED {
        return Err((CmdReason::UNSUPPORTED, "teams are balanced automatically (team rule AUTO)".into()));
    }
    if team > c.team_count() {
        return Err((CmdReason::INVALID_VALUE, format!("team must be 0..={}", c.team_count())));
    }
    if team == 0 { inner.modes.picks.remove(&who); } else { inner.modes.picks.insert(who, team); }
    inner.modes.dirty = true;
    inner.match_state_dirty = true;
    Ok(if team == 0 { "no team picked".into() } else { format!("team {}", TEAM_NAMES[team as usize - 1]) })
}

// ---- teams ----------------------------------------------------------------------------------

/// Team of a player key in the match (or its lobby pick); 0 = none.
pub(crate) fn team_of_key(inner: &Inner, k: &session::PlayerKey) -> u8 {
    if inner.modes.run.is_some() {
        inner.modes.teams.get(k).copied().unwrap_or(0)
    } else if inner.modes.cfg.rule() == v5::TeamRule::FIXED {
        inner.modes.picks.get(k).copied().unwrap_or(0)
    } else {
        0
    }
}

pub(crate) fn team_of_peer(inner: &Inner, pid: PeerId) -> u8 {
    if inner.modes.run.map_or(0, |c| c.team_count()) == 0 { return 0; }
    inner.peers.values().find(|p| p.id == pid).map_or(0, |p| inner.modes.teams.get(&session::peer_key(p)).copied().unwrap_or(0))
}

/// The side a peer fights for.
pub(crate) fn side_of(inner: &Inner, p: &PeerState) -> Side {
    if inner.modes.run.map_or(0, |c| c.team_count()) > 0 {
        if let Some(&t) = inner.modes.teams.get(&session::peer_key(p)) {
            if t != 0 { return Side::Team(t); }
        }
    }
    Side::Player(p.id)
}

/// Seat order of `keys` (seatless keys last, then by key).
fn by_seat(inner: &Inner, keys: &mut [session::PlayerKey]) {
    keys.sort_by_key(|k| (inner.sess.seats.get(k).copied().unwrap_or(255), *k));
}

/// The smallest team (fewest members among `count`), ties to the lowest number.
fn smallest(count: &[usize; 4], teams: u8) -> u8 {
    (1..=teams).min_by_key(|t| (count[*t as usize - 1], *t)).unwrap_or(1)
}

/// START: freeze the config and seat every participant on a team. Err = the refusal text
/// (a FIXED team without players, fewer players than teams).
pub(crate) fn on_start(inner: &mut Inner) -> Result<(), String> {
    let c = inner.modes.cfg;
    let n = c.team_count();
    let mut teams: HashMap<session::PlayerKey, u8> = HashMap::new();
    if n > 0 {
        let mut keys = inner.participants.clone();
        by_seat(inner, &mut keys);
        let mut count = [0usize; 4];
        if c.rule() == v5::TeamRule::FIXED {
            for k in &keys {
                if let Some(&t) = inner.modes.picks.get(k).filter(|t| (1..=n).contains(*t)) {
                    teams.insert(*k, t);
                    count[t as usize - 1] += 1;
                }
            }
        }
        for k in &keys {
            if teams.contains_key(k) { continue; }
            let t = smallest(&count, n);
            teams.insert(*k, t);
            count[t as usize - 1] += 1;
        }
        if keys.len() > 1 {
            if let Some(empty) = (1..=n).find(|t| count[*t as usize - 1] == 0) {
                return Err(if keys.len() < n as usize {
                    format!("{} teams need at least {} players", n, n)
                } else {
                    format!("team {} has no players", TEAM_NAMES[empty as usize - 1])
                });
            }
        }
        info!(teams = n, rule = c.rule(), sizes = ?&count[..n as usize], "match teams");
    }
    let m = &mut inner.modes;
    m.run = Some(c);
    m.teams = teams;
    m.team_wins = [0; 4];
    m.stats.clear();
    m.result = mode_result::NONE;
    m.winner_team = 0;
    m.zone = if c.koth() { zone_for(&inner.match_arena) } else { None };
    m.dirty = true;
    if c.koth() && m.zone.is_none() {
        warn!(arena = %inner.match_arena, "King of the hill: no zone for this arena (no map data); the round clock decides");
    }
    Ok(())
}

/// A late joiner enters the match (go_live): its team is its FIXED pick, else the
/// smallest team by connected members.
pub(crate) fn seat_late(inner: &mut Inner, k: session::PlayerKey) {
    let Some(c) = inner.modes.run else { return };
    let n = c.team_count();
    if n == 0 || inner.modes.teams.contains_key(&k) { return; }
    let pick = (c.rule() == v5::TeamRule::FIXED).then(|| inner.modes.picks.get(&k).copied()).flatten().filter(|t| (1..=n).contains(t));
    let t = pick.unwrap_or_else(|| {
        let mut count = [0usize; 4];
        for p in inner.peers.values() {
            if let Some(&t) = inner.modes.teams.get(&session::peer_key(p)) { count[t as usize - 1] += 1; }
        }
        smallest(&count, n)
    });
    info!(team = t, "match: late joiner seated on a team");
    inner.modes.teams.insert(k, t);
    inner.modes.dirty = true;
}

/// Sides among the standing participants: (distinct sides, whether the match has 2+ sides).
/// Allocation-free (runs every tick of a live round).
pub(crate) fn standing_sides(inner: &Inner, standing: impl Fn(&Inner, &PeerState) -> bool) -> (usize, bool) {
    let teams_on = inner.modes.run.map_or(0, |c| c.team_count()) > 0;
    if !teams_on {
        let n = inner.peers.values().filter(|p| standing(inner, p)).count();
        return (n, inner.participants.len() >= 2);
    }
    let mut mask = 0u8;
    let mut solo = 0usize;
    for p in inner.peers.values().filter(|p| standing(inner, p)) {
        match inner.modes.teams.get(&session::peer_key(p)) {
            Some(&t) if t != 0 => mask |= 1 << (t - 1),
            _ => solo += 1,
        }
    }
    let mut all = 0u8;
    let mut loose = 0usize;
    for k in &inner.participants {
        match inner.modes.teams.get(k) {
            Some(&t) if t != 0 => all |= 1 << (t - 1),
            _ => loose += 1,
        }
    }
    (mask.count_ones() as usize + solo, all.count_ones() as usize + loose >= 2)
}

/// The one side every peer passing `f` belongs to, if they all share one (None: none or several).
pub(crate) fn single_side(inner: &Inner, f: impl Fn(&PeerState) -> bool) -> Option<(Side, PeerId)> {
    let mut found: Option<(Side, PeerId)> = None;
    for p in inner.peers.values().filter(|p| f(p)) {
        let s = side_of(inner, p);
        match found {
            None => found = Some((s, p.id)),
            Some((o, _)) if o == s => {}
            Some(_) => return None,
        }
    }
    found
}

pub(crate) fn side_name(inner: &Inner, s: Side) -> String {
    match s {
        Side::Team(t) => format!("Team {}", TEAM_NAMES.get(t as usize - 1).copied().unwrap_or("?")),
        Side::Player(pid) => inner.peers.values().find(|p| p.id == pid).map(|p| p.nick.clone()).unwrap_or_else(|| format!("peer {}", pid)),
    }
}

// ---- hits, deaths, kills --------------------------------------------------------------------

/// The server's verdict on a hit before it is booked: a reason to drop it, or None.
/// Friendly fire (teammates, unless the option allows it) and the respawn protection.
pub(crate) fn hit_refusal(inner: &Inner, attacker: PeerId, target: PeerId) -> Option<&'static str> {
    if protected(inner, target) || protected(inner, attacker) { return Some("respawn protection"); }
    let c = inner.modes.run?;
    if c.team_count() == 0 || c.friendly_fire || attacker == target { return None; }
    let (a, t) = (team_of_peer(inner, attacker), team_of_peer(inner, target));
    (a != 0 && a == t).then_some("friendly fire")
}

/// A revived player inside its protection window.
pub(crate) fn protected(inner: &Inner, pid: PeerId) -> bool {
    inner.modes.protect.get(&pid).is_some_and(|&until| inner.now_ms < until)
}

/// A declared death (declare_death_unprotected): stats, the kill feed, a deathmatch respawn.
pub(crate) fn on_death(inner: &mut Inner, pid: PeerId, killer: PeerId, cause: u8) {
    let Some(c) = inner.modes.run else { return };
    let key_of = |inner: &Inner, id: PeerId| inner.peers.values().find(|p| p.id == id).map(session::peer_key);
    let victim = key_of(inner, pid);
    let credit = killer != 0 && killer != pid && {
        let (a, v) = (team_of_peer(inner, killer), team_of_peer(inner, pid));
        a == 0 || a != v
    };
    if let Some(k) = victim { inner.modes.stats.entry(k).or_default().deaths += 1; }
    if credit {
        if let Some(k) = key_of(inner, killer) {
            let s = inner.modes.stats.entry(k).or_default();
            s.kills += 1;
            s.round_kills += 1;
        }
    }
    if c.respawns() && inner.match_state == "live" {
        let at = inner.now_ms + c.respawn_s as u64 * 1000;
        inner.modes.respawns.insert(pid, Respawn { at_ms: at, spawn_id: 0 });
    }
    // Sudden death: the kill that makes one side lead ends the round.
    if inner.modes.sudden_death.is_some() && credit && inner.match_state == "live" {
        if let Some(side) = leader(inner) {
            inner.modes.end = Some(RoundEnd::SuddenDeath(side));
        }
    }
    kill_feed(inner, pid, killer, cause);
    inner.modes.dirty = true;
}

/// The `kill_feed` record to every peer with `caps::MODES`.
fn kill_feed(inner: &mut Inner, victim: PeerId, killer: PeerId, cause: u8) {
    let to: Vec<SocketAddr> = inner.peers.iter()
        .filter(|(_, p)| crate::interact::peer_caps(p.id) & caps::MODES != 0)
        .map(|(a, _)| *a)
        .collect();
    if to.is_empty() { return; }
    let event_id = inner.sess.next_event_id();
    let seat = |id: PeerId| if id == 0 { rec::NO_SEAT } else { session::seat_of_peer(inner, id).unwrap_or(rec::NO_SEAT) };
    let weapon = inner.modes.kit.as_ref().map(|k| k.r).unwrap_or("");
    let k = rec::KillFeed {
        match_id: inner.sess.match_id, event_id, killer_seat: seat(killer), victim_seat: seat(victim), cause, _r: 0,
        weapon: Str::new(weapon),
    };
    let m = wire::encode(0, 0, &k, &[]);
    for a in to { inner.out_msgs.push((Some(a), m.clone())); }
}

/// The side with the most points this round, if exactly one leads (deathmatch kills,
/// King of the hill ms).
fn leader(inner: &Inner) -> Option<Side> {
    let mut best: Option<(u64, Side)> = None;
    let mut tie = false;
    for (s, v) in side_scores(inner) {
        match best {
            Some((b, _)) if v < b => {}
            Some((b, _)) if v == b => tie = true,
            _ => { best = Some((v, s)); tie = false; }
        }
    }
    if tie { None } else { best.map(|(_, s)| s) }
}

/// Round score per side (deathmatch: kills, King of the hill: ms held alone). Teams: every
/// team of the match, its members' kills summed (also those who left); players: the
/// connected participants.
pub(crate) fn side_scores(inner: &Inner) -> Vec<(Side, u64)> {
    let Some(c) = inner.modes.run else { return Vec::new() };
    let stat = |k: &session::PlayerKey| inner.modes.stats.get(k).copied().unwrap_or_default();
    let mut out: Vec<(Side, u64)> = Vec::new();
    if c.team_count() > 0 {
        for t in 1..=c.team_count() {
            let v = if c.koth() {
                inner.modes.team_score_ms[t as usize - 1]
            } else {
                inner.modes.teams.iter().filter(|(_, tt)| **tt == t).map(|(k, _)| stat(k).round_kills as u64).sum()
            };
            out.push((Side::Team(t), v));
        }
    } else {
        for p in inner.peers.values().filter(|p| inner.participants.contains(&session::peer_key(p))) {
            let st = stat(&session::peer_key(p));
            out.push((Side::Player(p.id), if c.koth() { st.score_ms } else { st.round_kills as u64 }));
        }
    }
    out.sort();
    out
}

// ---- round flow -----------------------------------------------------------------------------

/// Countdown for `round`: the imposed kit of a roulette / brawl round.
pub(crate) fn on_countdown(inner: &mut Inner, round: u32) {
    let Some(c) = inner.modes.run else { return };
    let kit = match c.mode {
        gm::ROULETTE => Some(roulette_kit(inner.sess.match_id, round)),
        gm::BRAWL => Some(brawl_kit()),
        _ => None,
    };
    if kit != inner.modes.kit {
        if let Some(k) = &kit {
            info!(round, class = k.class, r = k.r, armour = k.armor.len(), label = %k.label, "round kit imposed");
            push_server_chat(inner, &format!("round {}: everyone fights with {}", round, k.label));
        }
        inner.modes.kit = kit;
        inner.modes.kit_gen = inner.modes.kit_gen.wrapping_add(1);
        inner.modes.dirty = true;
    }
}

/// Countdown -> live: per-round state starts over.
pub(crate) fn on_live(inner: &mut Inner) {
    let Some(c) = inner.modes.run else { return };
    let keys: Vec<session::PlayerKey> = inner.participants.clone();
    for k in keys {
        let s = inner.modes.stats.entry(k).or_default();
        s.round_kills = 0;
        s.score_ms = 0;
        s.life = 1;
    }
    let m = &mut inner.modes;
    m.team_score_ms = [0; 4];
    m.clock_ms = (c.round_time() > 0).then(|| c.round_time() as u64 * 1000);
    m.sudden_death = None;
    m.end = None;
    m.respawns.clear();
    m.protect.clear();
    m.hold = Hold::default();
    m.dirty = true;
}

/// Every tick while Live (`advance`): the round clock, the zone, respawn orders. Returns
/// true when the round must end now (`m.end` says why).
pub(crate) fn step(inner: &mut Inner, dt: u64) -> bool {
    let Some(c) = inner.modes.run else { return false };
    if inner.match_state != "live" { return false; }
    if inner.modes.end.is_some() { return true; }
    if c.koth() { zone_step(inner, c, dt); }
    if c.respawns() { respawn_step(inner); }
    if inner.modes.end.is_some() { return true; }
    if let Some(left) = inner.modes.sudden_death {
        let left = left.saturating_sub(dt);
        inner.modes.sudden_death = Some(left);
        if left == 0 {
            inner.modes.end = Some(RoundEnd::SuddenDeathOver);
            return true;
        }
        return false;
    }
    if let Some(left) = inner.modes.clock_ms {
        let left = left.saturating_sub(dt);
        inner.modes.clock_ms = Some(left);
        if left == 0 {
            if c.respawns() && leader(inner).is_none() && side_scores(inner).len() >= 2 {
                info!(round = inner.match_round, "deathmatch: tied at the clock; sudden death");
                inner.modes.sudden_death = Some(SUDDEN_DEATH_MS);
                inner.modes.dirty = true;
                push_notice(inner, v5::Notice::SUDDEN_DEATH, &["kills tied: the next kill wins"]);
                push_server_chat(inner, "Time! Kills are tied: sudden death, the next kill wins");
                return false;
            }
            inner.modes.end = Some(RoundEnd::Time);
            return true;
        }
    }
    false
}

/// The settle deadline: who won the round. (winner, `match_reason`, `mode_result`).
pub(crate) fn decide(inner: &Inner, standing: &[PeerId]) -> (Option<Side>, &'static str, u8) {
    let c = inner.modes.run.unwrap_or_default();
    let side_of_id = |id: PeerId| inner.peers.values().find(|p| p.id == id).map(|p| side_of(inner, p));
    match inner.modes.end {
        Some(RoundEnd::Objective(s)) => (Some(s), "", mode_result::OBJECTIVE),
        Some(RoundEnd::SuddenDeath(s)) => (Some(s), "", mode_result::SUDDEN_DEATH),
        Some(RoundEnd::SuddenDeathOver) => (None, "draw", mode_result::DRAW),
        Some(RoundEnd::Time) => {
            let w = if c.respawns() || c.koth() {
                leader(inner)
            } else {
                // Elimination modes: the side with the most players standing.
                let mut count: Vec<(Side, usize)> = Vec::new();
                for s in standing.iter().filter_map(|id| side_of_id(*id)) {
                    match count.iter_mut().find(|(o, _)| *o == s) { Some(e) => e.1 += 1, None => count.push((s, 1)) }
                }
                count.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
                match count.as_slice() {
                    [(s, _)] => Some(*s),
                    [(s, a), (_, b), ..] if a > b => Some(*s),
                    _ => None,
                }
            };
            let r = if c.respawns() { mode_result::KILLS } else { mode_result::TIME_LIMIT };
            match w { Some(s) => (Some(s), "time_limit", r), None => (None, "draw", mode_result::DRAW) }
        }
        None => {
            let mut sides: Vec<Side> = standing.iter().filter_map(|id| side_of_id(*id)).collect();
            sides.sort();
            sides.dedup();
            match sides.as_slice() {
                [s] => (Some(*s), "", mode_result::ELIMINATION),
                _ => (None, "draw", mode_result::DRAW),
            }
        }
    }
}

/// Book a team's round win: the team's count and every member's wins (connected or not).
/// Returns the team's wins.
pub(crate) fn team_won(inner: &mut Inner, t: u8) -> u32 {
    inner.modes.team_wins[t as usize - 1] += 1;
    let w = inner.modes.team_wins[t as usize - 1];
    set_team_wins(inner, t, w);
    w
}

/// Every member of team `t` shows `w` wins.
pub(crate) fn set_team_wins(inner: &mut Inner, t: u8, w: u32) {
    inner.modes.team_wins[t as usize - 1] = w;
    let members: Vec<session::PlayerKey> = inner.modes.teams.iter().filter(|(_, tt)| **tt == t).map(|(k, _)| *k).collect();
    for k in members {
        inner.wins_by_key.insert(k, w);
        for p in inner.peers.values_mut().filter(|p| session::peer_key(p) == k) { p.wins = w; }
    }
}

/// A connected member of team `t` that stands for it in the session head (lowest seat).
pub(crate) fn team_rep(inner: &Inner, t: u8) -> PeerId {
    inner.peers.values()
        .filter(|p| inner.modes.teams.get(&session::peer_key(p)) == Some(&t))
        .min_by_key(|p| (session::seat_of_peer(inner, p.id).unwrap_or(255), p.id))
        .map_or(0, |p| p.id)
}

/// The round result is fixed (finalize_round).
pub(crate) fn on_result(inner: &mut Inner, winner: Option<Side>, result: u8) {
    inner.modes.result = result;
    inner.modes.winner_team = match winner { Some(Side::Team(t)) => t, _ => 0 };
    inner.modes.end = None;
    inner.modes.clock_ms = None;
    inner.modes.sudden_death = None;
    inner.modes.respawns.clear();
    inner.modes.dirty = true;
}

/// Back in the lobby: the match's mode state is gone.
pub(crate) fn on_lobby(inner: &mut Inner) {
    let m = &mut inner.modes;
    m.run = None;
    m.teams.clear();
    m.team_wins = [0; 4];
    m.team_score_ms = [0; 4];
    m.stats.clear();
    m.clock_ms = None;
    m.sudden_death = None;
    m.end = None;
    m.respawns.clear();
    m.protect.clear();
    m.zone = None;
    m.hold = Hold::default();
    if m.kit.is_some() {
        m.kit = None;
        m.kit_gen = m.kit_gen.wrapping_add(1);
    }
    m.dirty = true;
}

// ---- King of the hill ----------------------------------------------------------------------

/// `HSMP_KOTH_ZONES="Map_Arena_Pit:x,y,z,r;..."` (cm) overrides the derived zones.
fn zone_overrides() -> &'static HashMap<String, Zone> {
    static Z: std::sync::OnceLock<HashMap<String, Zone>> = std::sync::OnceLock::new();
    Z.get_or_init(|| parse_zone_overrides(&std::env::var("HSMP_KOTH_ZONES").unwrap_or_default()))
}

pub(crate) fn parse_zone_overrides(s: &str) -> HashMap<String, Zone> {
    let mut out = HashMap::new();
    for part in s.split(';').map(str::trim).filter(|p| !p.is_empty()) {
        let Some((map, nums)) = part.split_once(':') else { warn!(entry = part, "HSMP_KOTH_ZONES: expected Map:x,y,z,r"); continue };
        let v: Vec<f32> = nums.split(',').filter_map(|n| n.trim().parse().ok()).collect();
        match v.as_slice() {
            [x, y, z, r] if v.iter().all(|f| f.is_finite()) && *r > 0.0 => {
                out.insert(normalize_arena(map), Zone { center: [*x, *y, *z], radius_cm: *r, half_height_cm: ZONE_HALF_HEIGHT_CM });
            }
            _ => warn!(entry = part, "HSMP_KOTH_ZONES: expected Map:x,y,z,r (cm)"),
        }
    }
    out
}

/// Height tolerance of the zone around its centre.
pub(crate) const ZONE_HALF_HEIGHT_CM: f32 = 300.0;

/// The zone of an arena: the override, else the centroid of its valid spawn points with a
/// radius of a third of their mean distance from it (250..=600 cm).
pub(crate) fn zone_for(arena: &str) -> Option<Zone> {
    if let Some(z) = zone_overrides().get(arena) { return Some(*z); }
    derived_zone(arena)
}

pub(crate) fn derived_zone(arena: &str) -> Option<Zone> {
    let t = crate::spawns::table(arena)?;
    let pts: Vec<[f32; 3]> = t.points.iter().map(|&i| t.all[i].pos).collect();
    if pts.is_empty() { return None; }
    let n = pts.len() as f32;
    let c = [pts.iter().map(|p| p[0]).sum::<f32>() / n, pts.iter().map(|p| p[1]).sum::<f32>() / n,
             pts.iter().map(|p| p[2]).sum::<f32>() / n];
    let mean = pts.iter().map(|p| ((p[0] - c[0]).powi(2) + (p[1] - c[1]).powi(2)).sqrt()).sum::<f32>() / n;
    Some(Zone { center: c, radius_cm: (mean / 3.0).clamp(250.0, 600.0), half_height_cm: ZONE_HALF_HEIGHT_CM })
}

/// Who stands in the zone (alive participants, by their last validated root); the side
/// alone in it scores `dt`, and reaching the target ends the round.
fn zone_step(inner: &mut Inner, c: ModeCfg, dt: u64) {
    let Some(z) = inner.modes.zone else { return };
    let mut holder: Option<Side> = None;
    let mut contested = false;
    let mut inside = 0u8;
    for p in inner.peers.values() {
        if !p.alive || !inner.participants.contains(&session::peer_key(p)) { continue; }
        if !p.last_valid_pos.is_some_and(|pos| z.contains(pos)) { continue; }
        inside = inside.saturating_add(1);
        let s = side_of(inner, p);
        match holder {
            None => holder = Some(s),
            Some(h) if h != s => contested = true,
            _ => {}
        }
    }
    let hold = Hold { holder: if contested { None } else { holder }, contested, inside };
    if hold != inner.modes.hold {
        inner.modes.hold = hold;
        inner.modes.dirty = true;
    }
    let Some(side) = hold.holder else { return };
    let target = c.koth_target_s as u64 * 1000;
    let score = match side {
        Side::Team(t) => {
            let s = &mut inner.modes.team_score_ms[t as usize - 1];
            *s += dt;
            *s
        }
        Side::Player(pid) => {
            let Some(k) = inner.peers.values().find(|p| p.id == pid).map(session::peer_key) else { return };
            let s = inner.modes.stats.entry(k).or_default();
            s.score_ms += dt;
            s.score_ms
        }
    };
    // Whole seconds change the HUD.
    if (score / 1000) != (score.saturating_sub(dt) / 1000) { inner.modes.dirty = true; }
    if score >= target {
        info!(round = inner.match_round, ?side, score_ms = score, "King of the hill: target reached");
        inner.modes.end = Some(RoundEnd::Objective(side));
    }
}

// ---- weapon roulette / brawl ---------------------------------------------------------------

fn splitmix(mut x: u64) -> u64 {
    x = x.wrapping_add(0x9E37_79B9_7F4A_7C15);
    x = (x ^ (x >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    x = (x ^ (x >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    x ^ (x >> 31)
}

/// Clothes for "no armour" (the peasant's).
const PLAIN_CLOTHES: &[&str] = &["b_tunic", "l_hosen3"];

/// A readable name for a catalogue id ("w_poleaxe_m" -> "poleaxe m"); the HUD shows the
/// game's own label when it knows the id.
fn item_label(id: &str) -> String {
    let s = id.split_once('_').map_or(id, |(_, rest)| rest);
    s.replace('_', " ")
}

/// The round's roulette kit: one weapon of the catalogue (never a shield) and one armour set
/// (a class's, or none), the same for everyone, from (match id, round) alone.
pub(crate) fn roulette_kit(match_id: u64, round: u32) -> RoundKit {
    use crate::loadout::catalog::{Kind, CLASSES, ITEMS};
    let weapons: Vec<&'static str> = ITEMS.iter().filter(|i| i.kind == Kind::Weapon && i.group != "shield").map(|i| i.id).collect();
    let h = splitmix(match_id ^ (round as u64).wrapping_mul(0xA24B_AED4_963E_E407));
    let r = weapons[(h % weapons.len() as u64) as usize];
    let a = (splitmix(h) % (CLASSES.len() as u64 + 1)) as usize;
    let (armor, armour_label): (&'static [&'static str], String) = if a == 0 {
        (PLAIN_CLOTHES, "no armour".into())
    } else {
        (CLASSES[a - 1].armor, format!("{} armour", CLASSES[a - 1].id.replace('_', " ")))
    };
    RoundKit { class: "roulette", r, l: "", armor, label: format!("{}, {}", item_label(r), armour_label) }
}

/// Brawl: fists, no armour.
pub(crate) fn brawl_kit() -> RoundKit {
    RoundKit { class: "brawl", r: "", l: "", armor: PLAIN_CLOTHES, label: "fists, no armour".into() }
}

// ---- deathmatch respawns -------------------------------------------------------------------

/// The respawn order's spawn id: the current round in the high bits (clients take
/// `spawn_id >> 8` as the round), 0x80 | life in the low byte (round-start orders use the
/// low byte as an index below 0x80).
pub(crate) fn respawn_spawn_id(round: u32, life: u16) -> u32 {
    (round << 8) | 0x80 | (life as u32 & 0x7F)
}

/// Issue the respawn orders that are due: a free spawn point far from the living, a new
/// spawn order in the plan (the session roster carries it), the life count.
fn respawn_step(inner: &mut Inner) {
    let now = inner.now_ms;
    let due: Vec<PeerId> = inner.modes.respawns.iter()
        .filter(|(_, r)| (r.spawn_id == 0 && now >= r.at_ms) || (r.spawn_id != 0 && now >= r.at_ms + RESPAWN_LOAD_MS))
        .map(|(id, _)| *id)
        .collect();
    for pid in due {
        let Some(p) = inner.peers.values().find(|p| p.id == pid) else { inner.modes.respawns.remove(&pid); continue };
        if p.alive { inner.modes.respawns.remove(&pid); continue; }
        let key = session::peer_key(p);
        let late = inner.modes.respawns.get(&pid).is_some_and(|r| r.spawn_id != 0);
        let avoid: Vec<[f32; 3]> = inner.peers.values().filter(|q| q.alive && q.id != pid).filter_map(|q| q.last_valid_pos)
            .chain(inner.modes.respawns.values().filter(|r| r.spawn_id != 0)
                .filter_map(|r| inner.spawn_plan.iter().find(|s| s.spawn_id == r.spawn_id).map(|s| s.pos)))
            .collect();
        let life = {
            let s = inner.modes.stats.entry(key).or_default();
            s.life = s.life.saturating_add(1);
            s.life
        };
        let round = inner.match_round;
        let spawn_id = respawn_spawn_id(round, life);
        let pick = crate::spawns::respawn_point(&inner.match_arena, &avoid);
        let mut order = crate::spawns::SpawnAssign {
            peer_id: pid, spawn_id, slot: 0, pos: [0.0; 3], yaw: 0.0, protect_ms: SPAWN_PROTECT_MS,
        };
        if let Some((slot, pos, yaw)) = pick { order.slot = slot; order.pos = pos; order.yaw = yaw; }
        match inner.spawn_plan.iter_mut().find(|s| s.peer_id == pid) {
            Some(s) => *s = order,
            None => inner.spawn_plan.push(order),
        }
        inner.modes.respawns.insert(pid, Respawn { at_ms: now, spawn_id });
        inner.modes.dirty = true;
        inner.match_state_dirty = true;
        info!(round, peer_id = pid, life, spawn_id, slot = order.slot, x = order.pos[0], y = order.pos[1], z = order.pos[2],
              reissued = late, "deathmatch: respawn order");
        crate::events::emit("respawn_order", serde_json::json!({
            "peer_id": pid, "round": round, "life": life, "spawn_id": spawn_id, "match_id": inner.sess.match_id,
            "reissued": late,
        }));
        // A client whose game never reports (headless tools) has no world to reload.
        let aware = inner.match_peers.get(&pid).is_some_and(|m| m.aware);
        if !aware { revive(inner, pid); }
    }
}

/// The client placed its pawn on spawn order `spawn_id` (`game_status`): a pending
/// respawn order of that id revives the player. True when it did.
pub(crate) fn on_placed(inner: &mut Inner, pid: PeerId, spawn_id: u32) -> bool {
    if spawn_id == 0 || inner.match_state != "live" { return false; }
    if !inner.modes.respawns.get(&pid).is_some_and(|r| r.spawn_id == spawn_id) { return false; }
    revive(inner, pid);
    true
}

/// Back in the round: alive, a fresh damage ledger life, protected for RESPAWN_PROTECT_MS.
fn revive(inner: &mut Inner, pid: PeerId) {
    inner.modes.respawns.remove(&pid);
    let round = inner.match_round;
    let Some(p) = inner.peers.values_mut().find(|p| p.id == pid) else { return };
    p.alive = true;
    // Teleported onto its spawn: the root speed cap starts over.
    p.last_valid_pos = None;
    inner.round_deaths.retain(|d| d.0 != pid);
    inner.modes.protect.insert(pid, inner.now_ms + RESPAWN_PROTECT_MS);
    crate::combat::ledger_respawn(pid, round);
    crate::lagcomp::note_alive(pid);
    inner.modes.dirty = true;
    inner.match_state_dirty = true;
    info!(peer_id = pid, round, "deathmatch: respawned");
    crate::events::emit("respawned", serde_json::json!({ "peer_id": pid, "round": round, "match_id": inner.sess.match_id }));
}

/// A dead player loading its respawn: its game may be silent for a level load.
pub(crate) fn respawning(inner: &Inner, pid: PeerId) -> bool {
    inner.modes.respawns.get(&pid).is_some_and(|r| r.spawn_id != 0)
}

// ---- records --------------------------------------------------------------------------------

/// The `mode` record (and the `zone` record) when due: on change (at most every
/// MODE_MIN_GAP_MS) or every MODE_EVERY_MS outside the lobby, to the peers with the cap.
/// Allocation-free when nobody has `caps::MODES`.
pub(crate) fn records_due(inner: &mut Inner, now: u64, out: &mut Vec<(Option<SocketAddr>, Vec<u8>)>) {
    if !inner.peers.values().any(|p| crate::interact::peer_caps(p.id) & caps::MODES != 0) { return; }
    let gap = now.saturating_sub(inner.modes.sent_ms);
    let periodic = inner.match_state != "lobby" && gap >= MODE_EVERY_MS;
    if !(periodic || (inner.modes.dirty && gap >= MODE_MIN_GAP_MS) || inner.modes.sent_ms == 0) { return; }
    inner.modes.dirty = false;
    inner.modes.sent_ms = now.max(1);
    inner.modes.seq = inner.modes.seq.wrapping_add(1);
    let (head, rows) = mode_record(inner, now);
    let m = wire::encode(0, 0, &head, &rows);
    let zone = zone_record(inner);
    for (a, p) in inner.peers.iter() {
        let c = crate::interact::peer_caps(p.id);
        if c & caps::MODES == 0 { continue; }
        out.push((Some(*a), m.clone()));
        if let (Some(z), true) = (&zone, c & caps::ZONE != 0) { out.push((Some(*a), z.clone())); }
    }
}

/// The `mode` record as of `now`.
pub(crate) fn mode_record(inner: &Inner, now: u64) -> (rec::ModeHead, Vec<rec::ModeRow>) {
    let m = &inner.modes;
    let c = *m.now();
    let mut h = rec::ModeHead {
        match_id: inner.sess.match_id, server_time_ms: now, seq: m.seq, round: inner.match_round,
        team_wins: m.team_wins, target_s: c.koth_target_s, round_time_s: c.round_time(), mode: c.mode,
        team_rule: c.rule(), teams: c.team_count(), friendly_fire: Bool::from(c.friendly_fire),
        sudden_death: Bool::from(m.sudden_death.is_some()), respawn_s: c.respawn_s, result: m.result,
        winner_team: m.winner_team, ..Default::default()
    };
    let clock = m.sudden_death.or(m.clock_ms).filter(|_| inner.match_state == "live" || inner.match_state == "paused");
    if let Some(left) = clock { h.round_end_ms = now + left; }
    if let Some(k) = &m.kit {
        h.kit_r = Str::new(k.r);
        h.kit_l = Str::new(k.l);
        h.kit_label = Str::new(&k.label);
    }
    let mut seats: Vec<(u8, session::PlayerKey)> = inner.sess.seats.iter().map(|(k, s)| (*s, *k)).collect();
    seats.sort_unstable();
    let mut rows = Vec::with_capacity(seats.len());
    for (seat, key) in seats.into_iter().take(rec::MAX_MODE_ROWS) {
        let p = inner.peers.values().find(|p| session::peer_key(p) == key);
        let st = m.stats.get(&key).copied().unwrap_or_default();
        let team = team_of_key(inner, &key);
        let pid = p.map_or(0, |p| p.id);
        let alive = p.is_some_and(|p| p.alive);
        let resp = m.respawns.get(&pid);
        if team != 0 {
            if alive && inner.participants.contains(&key) && inner.match_state != "lobby" { h.team_alive[team as usize - 1] += 1; }
            let ti = team as usize - 1;
            h.team_score[ti] = if c.koth() { m.team_score_ms[ti] as u32 } else { h.team_score[ti] + st.round_kills as u32 };
        }
        rows.push(rec::ModeRow {
            peer_id: pid,
            score: if c.koth() { st.score_ms.min(u32::MAX as u64) as u32 } else { st.round_kills as u32 },
            kills: st.kills, deaths: st.deaths, round_kills: st.round_kills, life: st.life, seat, team,
            alive: Bool::from(alive), respawning: Bool::from(resp.is_some()),
            in_zone: Bool::from(alive && m.zone.is_some_and(|z| p.and_then(|p| p.last_valid_pos).is_some_and(|pos| z.contains(pos)))),
            _r: [0; 3],
            respawn_at_ms: resp.map_or(0, |r| r.at_ms),
        });
    }
    h.n = rows.len() as u16;
    (h, rows)
}

/// The `zone` record (None without a zone).
fn zone_record(inner: &Inner) -> Option<Vec<u8>> {
    let z = inner.modes.zone?;
    let hold = inner.modes.hold;
    let (holder_seat, holder_team) = match hold.holder {
        Some(Side::Player(pid)) => (session::seat_of_peer(inner, pid).unwrap_or(rec::NO_SEAT), 0),
        Some(Side::Team(t)) => (rec::NO_SEAT, t),
        None => (rec::NO_SEAT, 0),
    };
    let r = rec::ZoneState {
        match_id: inner.sess.match_id, center: z.center, radius_cm: z.radius_cm, half_height_cm: z.half_height_cm,
        round: inner.match_round, holder_seat, holder_team, contested: Bool::from(hold.contested), inside: hold.inside,
        _r: 0, arena: Str::new(&inner.match_arena),
    };
    Some(wire::encode(0, 0, &r, &[]))
}

#[cfg(test)]
#[path = "modes_tests.rs"]
mod tests;
