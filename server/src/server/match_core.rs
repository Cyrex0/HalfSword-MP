//! Match core: arena registry, match verbs, authoritative deaths + settle,
//! the round-flow state machine (`match_step`), load barrier, spawn plan use,
//! and the round-flow unit tests.
//!
//! See docs/development/server-modules.md.

use super::*;

/// Spawns are server-authoritative (spawns.rs,
/// docs/development/subsystems/spawns.md): the plan with absolute
/// coordinates rides in S2CMatchState. The arena list keeps
/// slot markers `[slot, 0, SPAWN_SLOT_SENTINEL_Z]` only for legacy names that
/// have no spawn table (informational; nothing places pawns from it).
pub const SPAWN_SLOT_SENTINEL_Z: f32 = -999_999.0;
const SPAWN_SLOTS: u8 = 8;

/// Half Sword arena maps offered in the lobby picker, plus legacy names kept
/// so existing `pick_arena:` / `set_arena:` verbs still resolve.
const ARENA_NAMES: [&str; 11] = [
    "default",
    "Map_Arena_Alley",
    "Map_Arena_Pit",
    "Map_Arena_Yard",
    "Map_Arena_Slums",
    "Map_Arena_Cellar",
    "Map_Arena_LordsHall",
    "Map_Arena_EastTower",
    "village",
    "arena",
    "tower",
];

/// Arena used when nobody picked one (never "default" once a match starts).
pub const DEFAULT_ARENA: &str = "Map_Arena_Alley";

/// Accepts "Map_Arena_Pit", "/Game/Maps/Arenas/Map_Arena_Pit" or
/// "/Game/Maps/Arenas/Map_Arena_Pit.Map_Arena_Pit" and returns the short name.
pub fn normalize_arena(s: &str) -> String {
    let s = s.trim();
    let base = s.rsplit('/').next().unwrap_or(s);
    base.split('.').next().unwrap_or(base).to_string()
}

/// Seed the lobby arena from `--map` / HSMP_LOBBY_MAP at startup.
pub async fn seed_arena(state: &Arc<ServerState>, map: &str) {
    let name = normalize_arena(map);
    if arena_registry().iter().any(|(n, _, _)| *n == name) {
        state.inner.lock().await.match_arena = name.clone();
        info!(arena = %name, "lobby arena (startup)");
    }
}

fn spawn_slots(n: u8) -> Vec<[f32; 3]> {
    (0..n).map(|i| [i as f32, 0.0, SPAWN_SLOT_SENTINEL_Z]).collect()
}

pub(super) fn arena_registry() -> Vec<(String, Vec<[f32; 3]>, u8)> {
    ARENA_NAMES
        .iter()
        .map(|n| match crate::spawns::table(n) {
            Some(t) => (n.to_string(), t.points.iter().map(|&i| t.all[i].pos).collect(), SPAWN_SLOTS),
            None => (n.to_string(), spawn_slots(SPAWN_SLOTS), SPAWN_SLOTS),
        })
        .collect()
}

/// Resolve a user/RCON arena name against the registry: exact short name
/// ("Map_Arena_Pit", a path), case-insensitive, or without the prefix ("Pit").
pub(crate) fn resolve_arena(name: &str) -> Option<String> {
    let n = normalize_arena(name);
    if n.is_empty() { return None; }
    let reg = arena_registry();
    let lower = n.to_ascii_lowercase();
    let prefixed = format!("map_arena_{}", lower);
    reg.iter().map(|(r, _, _)| r)
        .find(|r| **r == n)
        .or_else(|| reg.iter().map(|(r, _, _)| r).find(|r| r.to_ascii_lowercase() == lower))
        .or_else(|| reg.iter().map(|(r, _, _)| r).find(|r| r.to_ascii_lowercase() == prefixed))
        .cloned()
}

/// Slowest acceptable arena (re)load before stragglers are dropped (45 s).
pub(super) const BARRIER_TIMEOUT_MS: u64 = 45_000;
/// A game that stops pinging (crash / hang) during a LIVE round counts as gone
/// after 20 s, even if its sidecar process is still connected. Not applied
/// while clients are loading (countdown/roundover): a level load blocks the
/// game thread for many seconds on slow PCs; the load barrier's own deadline
/// covers those phases.
const GAME_PING_TIMEOUT_MS: u64 = 20_000;
/// How long a match pauses for a dropped participant to reconnect (30 s).
const RECONNECT_GRACE_MS: u64 = 30_000;
/// A client that reported a load error (Director `load_error`) gets ONE retry
/// window of 10 s to still report loaded; then it sits the round out. The
/// barrier never waits longer than BARRIER_TIMEOUT_MS (45 s) in total.
pub(super) const LOAD_RETRY_MS: u64 = 10_000;
/// `protect_ms` carried in every spawn order (at least this; wire field kept
/// for older clients). Server and client protection both end at Live
/// (`spawn_protected`); the value only bounds a client's protection when its
/// placement fails and it never sees Live.
pub(super) const SPAWN_PROTECT_MS: u16 = 3000;
/// Rounds voided in a row because fewer than the needed fighters loaded:
/// after this many the match returns to the lobby instead of looping.
pub(super) const MAX_VOID_ROUNDS: u32 = 3;
/// A placement report may arrive this long after the round went live (1 s).
pub(super) const PLACE_LATE_MS: u64 = 1000;

/// A load error a client reported (the `game_status` record).
/// Only the first error of a pending round starts the retry window.
pub(super) fn note_load_error(inner: &mut Inner, id: PeerId, error: &str) {
    let e = error.trim();
    if e.is_empty() || e == "0" || e.eq_ignore_ascii_case("none") || e.eq_ignore_ascii_case("nil") { return; }
    if session::phase_code(inner) != v5::Phase::LOADING { return; }
    let pending = inner.match_round + 1;
    if inner.match_peers.get(&id).map_or(false, |m| m.loaded_round >= pending) { return; }
    let now = inner.now_ms;
    let fresh = inner.sess.load_errors.get(&id).map_or(true, |(r, _, _)| *r != pending);
    if fresh {
        warn!(peer_id = id, round = pending, error = %e, "client load error: one retry window, then it sits the round out");
        inner.sess.load_errors.insert(id, (pending, e.to_string(), now));
    }
}

/// `game_status.load_error` code -> the Director's name.
pub(super) fn load_error_name(code: u8) -> &'static str {
    use v5::LoadError as L;
    match code {
        L::NONE => "",
        L::TRAVEL_FAILED => "travel_failed",
        L::WRONG_WORLD => "wrong_world",
        L::NO_PAWN => "no_pawn",
        L::VITALS => "vitals",
        L::SPAWN_PLACE => "spawn_timeout",
        L::DRESS => "kit_error",
        L::STAND_INS => "stand_ins",
        L::TIMEOUT => "timeout",
        _ => "load_error",
    }
}

/// The client placed its pawn on its order for `round` (`spawned:` verb or
/// C2SGameStatus.spawn_id): starts its spawn protection.
pub(super) fn note_placed(inner: &mut Inner, id: PeerId, round: u32) {
    if round == 0 || round < inner.match_round { return; }
    // Only a placement for the round being loaded / counted down, or one
    // arriving within PLACE_LATE_MS of it going live, starts protection:
    // otherwise a `spawned:<current round>` sent mid-round would grant 3 s
    // of ignored deaths on demand.
    let pending = matches!(inner.match_state.as_str(), "countdown") && round == inner.match_round + 1;
    let just_live = inner.match_state == "live" && round == inner.match_round
        && inner.now_ms.saturating_sub(inner.sess.live_ms) <= PLACE_LATE_MS;
    if !pending && !just_live { return; }
    let now = inner.now_ms;
    let e = inner.sess.placed.entry(id).or_insert((round, now));
    if e.0 != round { *e = (round, now); }
    // The pawn was teleported onto its spawn: the root speed cap starts over
    // from there (the cap is a body speed, not a fixed 300 m/s).
    for p in inner.peers.values_mut().filter(|p| p.id == id) { p.last_valid_pos = None; }
}

/// True while `pid` is under spawn protection (against "insta died at round
/// start"): from its placement report for the round being counted
/// down until the round goes Live. The client ends its protection at Live
/// too, so from Live on there is no protected attacker and no
/// protected victim: normal combat for everyone, on both sides at the same
/// instant. (Damage and deaths during the countdown never count anyway:
/// `combat_open` is false and claims are refused as `not_live`.)
pub(super) fn spawn_protected(inner: &Inner, pid: PeerId) -> bool {
    let Some(&(round, _placed)) = inner.sess.placed.get(&pid) else { return false };
    inner.match_state == "countdown" && round == inner.match_round + 1
}

/// The client is "loaded" for the round it reports only when the arena it
/// names is the frozen one; a report without an arena (old
/// clients, headless tools) is trusted.
pub(super) fn arena_matches(inner: &Inner, arena: Option<&str>) -> bool {
    match arena.map(str::trim) {
        None | Some("") => true,
        Some(a) => {
            let want = inner.sess.frozen.as_ref().and_then(|c| c.arena.as_str()).unwrap_or(inner.match_arena.as_str());
            normalize_arena(a).eq_ignore_ascii_case(want)
        }
    }
}

/// One game-status report (the `game_status` record): load
/// barrier, game liveness, redundant death report. Returns true when the
/// report says the player died in the current live round.
pub(super) fn game_status_in(inner: &mut Inner, id: PeerId, alive: bool, loaded: u32, arena: Option<&str>, dead: bool) -> bool {
    let now = inner.now_ms;
    let right_arena = arena_matches(inner, arena);
    let mp = inner.match_peers.entry(id).or_default();
    if !mp.aware { info!(peer_id = id, "game client joined load barrier"); }
    mp.aware = true;
    mp.last_ping_ms = now;
    if right_arena {
        if mp.loaded_round != loaded {
            mp.loaded_round = loaded;
            inner.match_state_dirty = true;
        }
    } else if loaded > mp.loaded_round {
        debug!(peer_id = id, loaded, arena = ?arena, "load report on the wrong arena: not counted");
    }
    // Round-tagged: a delayed ping from the previous round's corpse must not
    // kill us in the round that just went live.
    dead && alive && inner.match_state == "live" && loaded == inner.match_round
}

/// The `game_status` record (session_records.rs): counted only for this match, round and
/// arena.
pub(crate) async fn on_game_status(socket: &Arc<UdpSocket>, state: &Arc<ServerState>, from: SocketAddr, gs: &hsmp_ipc::schema::session::GameStatus) {
    let died = {
        let mut inner = state.inner.lock().await;
        let Some((id, alive)) = inner.peers.get(&from).map(|p| (p.id, p.alive)) else { return };
        let this_match = gs.match_id == inner.sess.match_id || inner.match_state == "lobby";
        let loaded = if this_match && gs.flags & v5::status_flags::LOADED != 0 { gs.round } else { 0 };
        let arena = if this_match { Some(gs.arena.as_str().unwrap_or("")) } else { Some("") };
        let dead = this_match && gs.flags & v5::status_flags::DEAD != 0;
        if !this_match {
            // Liveness only: a stale match's load must not pass the barrier.
            let now = inner.now_ms;
            let mp = inner.match_peers.entry(id).or_default();
            mp.aware = true;
            mp.last_ping_ms = now;
            false
        } else {
            // The spawn order this client applied = its placement report.
            let order = inner.spawn_plan.iter().find(|s| s.peer_id == id).map(|s| s.spawn_id);
            if gs.spawn_id != 0 && order == Some(gs.spawn_id) {
                note_placed(&mut inner, id, gs.spawn_id >> 8);
            }
            let died = game_status_in(&mut inner, id, alive, loaded, arena, dead);
            if gs.load_error != 0 { note_load_error(&mut inner, id, load_error_name(gs.load_error)); }
            died
        }
    };
    if died {
        let _ = handle_player_died(socket, state, from).await;
    }
}

/// RCON `DEBUG KILL <seat>` (--debug-verbs only): the server declares the
/// player at `seat` dead in the live round, exactly as a reported death.
pub(crate) fn debug_kill(inner: &mut Inner, seat: u8) -> Result<String, String> {
    if inner.match_state != "live" {
        return Err(format!("not live (phase {})", session::phase_label(session::phase_code(inner))));
    }
    let Some(pid) = session::peer_at_seat(inner, seat) else {
        return Err(format!("no connected player at seat {}", seat));
    };
    // An admin action: not subject to spawn protection.
    if declare_death_unprotected(inner, pid, 0, DEATH_REPORTED) {
        info!(seat, peer_id = pid, round = inner.match_round, "DEBUG KILL (rcon)");
        Ok(format!("killed seat {} (peer {}) in round {}", seat, pid, inner.match_round))
    } else {
        Err(format!("seat {} is already dead", seat))
    }
}

pub(super) async fn handle_player_died(
    socket: &UdpSocket,
    state: &Arc<ServerState>,
    from: SocketAddr,
) -> anyhow::Result<()> {
    // Legacy/verb death paths (C2SPlayerDied, died:<round>, ping dead flag):
    // same idempotent, authoritative path as C2SDeath.
    {
        let mut inner = state.inner.lock().await;
        let Some(pid) = inner.peers.get(&from).map(|p| p.id) else { return Ok(()); };
        let killer = crate::combat::ledger_last_attacker(pid, inner.match_round);
        declare_death(&mut inner, pid, killer, DEATH_REPORTED);
    }
    flush_out(socket, state).await;
    Ok(())
}

// ---- authoritative deaths + round result --------------------------------------
//
// Deaths are declared ONLY here, by the server, from: the owner's own report
// (C2SDeath / died verb / ping), the owner's vitals, the server damage ledger
// (a lethal accepted hit, combat.rs), or leaving mid-round. A declared death
// is final for the round: the player is out (hits from/to it are refused,
// except trade hits inside the settle window) and every client gets the same
// S2CDeath + scoreboard.
//
// Round end: when at most one participant is left standing, the round
// SETTLES for SETTLE_MS before its result is fixed. Trade hits swung by the
// just-killed player no later than lagcomp::TRADE_WINDOW_MS after its death
// can still kill the provisional winner meanwhile. At the settle deadline:
// exactly one participant alive → they win; nobody alive → draw. Same rule
// for every arrival order, so simultaneous kills resolve deterministically
// (a mutual kill is a draw, never "whoever's packet won").

pub(crate) const DEATH_REPORTED: u8 = 0;
pub(crate) const DEATH_DAMAGE: u8 = 1;
pub(crate) const DEATH_VITALS: u8 = 2;
/// A participant that dropped out of a Live round with its pause budget spent
/// (`drop_loses_round`). Within the budget a duel pauses for its
/// reconnect instead; 3+ player rounds just stop counting it as standing.
pub(crate) const DEATH_LEFT: u8 = 3;
/// 400 ms: covers DEFENDER_GRACE (200 ms) + trade hit transit.
pub(super) const SETTLE_MS: u64 = 400;

/// Hits and deaths count: the round is live, or a finished round is settling.
pub(super) fn combat_open(inner: &Inner) -> bool {
    inner.match_state == "live" || (inner.match_state == "roundover" && inner.settle_ms > 0)
}

/// Connected participants of this match that are still standing.
fn standing(inner: &Inner) -> Vec<PeerId> {
    inner.peers.values()
        .filter(|p| p.alive && is_present(inner, p))
        .map(|p| p.id)
        .collect()
}

/// `standing(inner).len()` without the list (checked every tick of a live round).
fn standing_count(inner: &Inner) -> usize {
    inner.peers.values().filter(|p| p.alive && is_present(inner, p)).count()
}

/// Mark `pid` dead for the current round. Returns false if it was already
/// dead / unknown or no round is open (late packet, native flow, stand-in).
pub(super) fn declare_death(inner: &mut Inner, pid: PeerId, killer: PeerId, cause: u8) -> bool {
    // Spawn protection: a death report or ledger kill right after the
    // player's placement (insta-death at round start) does not count.
    if combat_open(inner) && spawn_protected(inner, pid) {
        let alive = inner.peers.values().any(|p| p.id == pid && p.alive);
        if alive {
            warn!(peer_id = pid, killer, cause, round = inner.match_round,
                  "death ignored: inside the spawn-protection window");
            crate::events::emit("death_ignored", serde_json::json!({
                "peer_id": pid, "killer": killer, "cause": cause, "round": inner.match_round,
                "why": "spawn_protection",
            }));
        }
        return false;
    }
    declare_death_unprotected(inner, pid, killer, cause)
}

/// `declare_death` without spawn protection (admin / debug kills).
pub(super) fn declare_death_unprotected(inner: &mut Inner, pid: PeerId, killer: PeerId, cause: u8) -> bool {
    if !combat_open(inner) {
        debug!(peer_id = pid, state = %inner.match_state, "death ignored: no open round");
        return false;
    }
    let round = inner.match_round;
    let Some(p) = inner.peers.values_mut().find(|p| p.id == pid) else { return false };
    if !p.alive { return false; }
    p.alive = false;
    crate::lagcomp::note_death(pid);
    inner.round_deaths.push((pid, killer, cause));
    inner.match_state_dirty = true;
    inner.out_msgs.push((None, death_msg(pid, round, killer, cause)));
    info!(peer_id = pid, killer, cause, round, settling = inner.settle_ms > 0,
          "DEATH declared (authoritative)");
    if inner.match_state == "live" { check_round_end(inner, true); }
    true
}

/// Live round: start the settle once ≤ 1 participant is standing (a solo
/// test round only ends when its player dies).
fn check_round_end(inner: &mut Inner, after_death: bool) {
    if inner.match_state != "live" { return; }
    let alive = standing_count(inner);
    let multi = inner.participants.len() >= 2;
    if alive == 0 || (multi && alive <= 1) {
        if !after_death { info!(alive, "match: round ends (players left / timed out)"); }
        inner.match_state = "roundover".into();
        inner.match_reason = "pending".into();
        inner.last_winner = 0;
        inner.countdown_ms = 0; // runs once the result is fixed
        inner.settle_ms = SETTLE_MS;
        inner.match_state_dirty = true;
        info!(round = inner.match_round, alive, "match: round over, settling {} ms for trades",
              SETTLE_MS);
        // Report Live -> RoundOver now (a death arrives outside the tick).
        session::observe_phase(inner);
    }
}

/// Settle deadline: fix the round result (see the block comment above).
pub(super) fn finalize_round(inner: &mut Inner) {
    inner.settle_ms = 0;
    let alive = standing(inner);
    let round = inner.match_round;
    let text;
    if alive.len() == 1 {
        let winner = alive[0];
        let mut wins = 0u32;
        let mut nick = String::new();
        let mut key = None;
        for p in inner.peers.values_mut() {
            if p.id == winner { p.wins += 1; wins = p.wins; nick = p.nick.clone(); key = Some(session::peer_key(p)); }
        }
        if let Some(k) = key { inner.wins_by_key.insert(k, wins); }
        let match_over = wins >= needed_wins(inner);
        end_round(inner, winner, "", match_over);
        info!(round, winner_id = winner, winner_nick = %nick, wins, match_over,
              deaths = ?inner.round_deaths, "ROUND RESULT (authoritative)");
        text = if match_over {
            format!("MATCH OVER — {} wins ({} rounds)", nick, wins)
        } else {
            format!("round {} — {} wins ({} total)", round, nick, wins)
        };
    } else {
        // Nobody (mutual kill / trade inside the window) — or, defensively,
        // several (cannot happen: settle starts at ≤ 1) — is a draw.
        end_round(inner, 0, "draw", false);
        info!(round, standing = alive.len(), deaths = ?inner.round_deaths,
              "ROUND RESULT (authoritative): draw");
        text = format!("round {} — draw (simultaneous kill)", round);
    }
    push_server_chat(inner, &text);
}

/// First countdown after START (3 s), run after the load barrier.
pub(super) const FIRST_COUNTDOWN_MS: u64 = 3000;
/// Countdown between rounds (3 s). Clients reload the arena when the
/// countdown state begins (HSMPMatch); the load barrier holds this timer
/// until every game client has loaded, so it does not pad for load time.
pub(super) const NEXT_ROUND_COUNTDOWN_MS: u64 = 3000;
/// Result screens: round over 4 s, match over 5 s (e2e T29 times the latter).
pub(super) const ROUNDOVER_MS: u64 = 4000;
pub(super) const MATCH_OVER_MS: u64 = 5000;

fn needed_wins(inner: &Inner) -> u32 { (inner.best_of as u32 + 1) / 2 }

/// Seat holders of this match are keyed by player key, not by nick or
/// peer id. No participants yet (lobby) means everyone.
pub(super) fn is_participant(inner: &Inner, p: &PeerState) -> bool {
    inner.participants.is_empty() || inner.participants.contains(&session::peer_key(p))
}

/// Any participant's game has pinged: barrier + liveness rules apply. Headless
/// clients (e2e harness) never ping, so neither rule applies to them.
pub(super) fn barrier_session(inner: &Inner) -> bool {
    inner.peers.values().any(|p| {
        is_participant(inner, p)
            && inner.match_peers.get(&p.id).map(|m| m.aware).unwrap_or(false)
    })
}

/// Participants currently connected whose game is still alive. Game-ping
/// staleness only counts during a live round (see GAME_PING_TIMEOUT_MS).
pub(super) fn present_participants(inner: &Inner) -> Vec<PeerId> {
    inner.peers.values().filter(|p| is_present(inner, p)).map(|p| p.id).collect()
}

/// `present_participants(inner).len()` without the list (match_step runs every tick).
fn present_count(inner: &Inner) -> usize {
    inner.peers.values().filter(|p| is_present(inner, p)).count()
}

/// A connected participant whose game is still alive (see `present_participants`).
fn is_present(inner: &Inner, p: &PeerState) -> bool {
    let check_pings = inner.match_state == "live";
    is_participant(inner, p)
        && match inner.match_peers.get(&p.id) {
            Some(m) if m.aware && check_pings => inner.now_ms.saturating_sub(m.last_ping_ms) <= GAME_PING_TIMEOUT_MS,
            _ => true,
        }
        // Resume: a game client silent for 3 s mid-Live is away.
        && session::link_ok(inner, p)
}

/// Present participants that have not loaded the pending round's arena yet.
pub(super) fn barrier_waiting_on(inner: &Inner) -> Vec<PeerId> {
    if !barrier_session(inner) { return Vec::new(); }
    let pending = inner.match_round + 1;
    present_participants(inner).into_iter()
        .filter(|id| inner.match_peers.get(id).map(|m| !m.aware || m.loaded_round < pending).unwrap_or(true))
        .collect()
}

fn end_round(inner: &mut Inner, winner: PeerId, reason: &str, match_over: bool) {
    inner.last_winner = winner;
    inner.match_reason = reason.to_string();
    if match_over {
        // The match winner as of now: it may leave during MATCH OVER, and the
        // result must not then name whoever is still connected.
        inner.sess.match_winner = inner.peers.values().find(|p| p.id == winner)
            .map(|p| (p.id, p.nick.clone(), p.wins));
        inner.match_state = "match_over".into();
        inner.countdown_ms = MATCH_OVER_MS;
    } else {
        inner.match_state = "roundover".into();
        inner.countdown_ms = ROUNDOVER_MS;
    }
    inner.match_state_dirty = true;
}

/// Award the match to the last participant standing after the others left.
pub(super) fn forfeit_to(inner: &mut Inner, winner: PeerId) {
    let needed = needed_wins(inner);
    let mut key = None;
    for p in inner.peers.values_mut() {
        if p.id == winner { p.wins = p.wins.max(needed); key = Some(session::peer_key(p)); }
    }
    let w = needed;
    if let Some(k) = key { inner.wins_by_key.insert(k, w); }
    end_round(inner, winner, "forfeit", true);
    info!(winner_id = winner, "match over by forfeit");
}

/// Per-tick match supervision that must hold over the public internet:
/// restores reconnected players' wins, pauses when a participant drops, ends
/// the match by forfeit if they don't come back, and runs the load barrier.
pub(super) fn match_step(inner: &mut Inner) {
    // The result screen is supervised too: the last player leaving it (e.g.
    // right after a forfeit) leaves nobody to show it to, so the empty server
    // is back in the lobby now instead of running out MATCH_OVER_MS with
    // the frozen config of a match nobody is in (proptest seed in
    // proptest-regressions/server/session_tests.txt).
    if inner.match_state == "match_over" && inner.peers.is_empty() {
        info!("match: everyone left during the result screen; back to lobby");
        reset_to_lobby(inner);
        return;
    }
    let active = matches!(inner.match_state.as_str(), "countdown" | "live" | "roundover" | "paused");
    if !active { return; }

    // Reconnect: same player key, new peer id -> same seat, same score.
    let restore: Vec<(SocketAddr, u32)> = inner.peers.iter()
        .filter_map(|(a, p)| inner.wins_by_key.get(&session::peer_key(p)).filter(|w| **w > p.wins).map(|w| (*a, *w)))
        .collect();
    for (a, w) in restore {
        if let Some(p) = inner.peers.get_mut(&a) {
            info!(peer_id = p.id, nick = %p.nick, wins = w, "match: reconnected participant restored");
            p.wins = w;
        }
        inner.match_state_dirty = true;
    }

    let multi = inner.participants.len() >= 2;
    let present = present_count(inner);

    // Nobody of this match is left (also a solo match whose only player
    // left, or an empty server): back to the lobby instead of "live" forever.
    if present == 0 {
        info!(state = %inner.match_state, "match: no participant left; back to lobby");
        reset_to_lobby(inner);
        return;
    }

    if inner.match_state == "paused" {
        if present >= 2 {
            inner.match_reason.clear();
            if std::mem::take(&mut inner.paused_from_live) {
                // The interrupted round continues where it stopped
                // (positions, health, ledger): a drop never buys a fresh round.
                info!(round = inner.match_round, "match: participant back; the round resumes (no replay)");
                crate::events::emit("round_resumed", serde_json::json!({
                    "round": inner.match_round, "match_id": inner.sess.match_id,
                }));
                inner.match_state = "live".into();
                inner.countdown_ms = 0;
                inner.match_state_dirty = true;
            } else {
                info!("match: participant back; next round");
                begin_countdown(inner, NEXT_ROUND_COUNTDOWN_MS);
            }
        }
        return; // expiry (forfeit) handled by the countdown transition
    }

    // A finished round settles first: its result is fixed before any pause.
    let settling = inner.match_state == "roundover" && inner.settle_ms > 0;
    if multi && inner.match_state != "match_over" && present < 2 && !settling {
        if present == 0 {
            info!("match: all participants gone; back to lobby");
            reset_to_lobby(inner);
            return;
        }
        if inner.match_state == "live" {
            // A drop during a Live round may pause it only within
            // the budget (one per player per match, MAX_MATCH_PAUSES in all).
            // Beyond it the dropper loses the round, as if dead.
            let droppers = absent_participants(inner, &present_participants(inner));
            let budget_ok = inner.match_pauses < MAX_MATCH_PAUSES
                && droppers.iter().all(|k| inner.pauses_by_key.get(k).copied().unwrap_or(0) < PAUSES_PER_PLAYER);
            if !budget_ok {
                drop_loses_round(inner, &droppers);
                return;
            }
            for k in &droppers { *inner.pauses_by_key.entry(*k).or_insert(0) += 1; }
            inner.match_pauses += 1;
            inner.paused_from_live = true;
            info!(present, pauses = inner.match_pauses,
                  "match: participant dropped mid-round; pausing for its reconnect (its one pause this match)");
        } else {
            info!(present, state = %inner.match_state, "match: participant dropped; pausing for reconnect");
        }
        inner.settle_ms = 0;
        inner.match_state = "paused".into();
        inner.match_reason = "opponent_left".into();
        inner.countdown_ms = RECONNECT_GRACE_MS;
        inner.barrier_passed = true;
        inner.match_state_dirty = true;
        return;
    }

    // Load barrier: hold the countdown until every game client has the arena
    // loaded for the pending round; drop stragglers at the deadline.
    if inner.match_state == "countdown" && !inner.barrier_passed {
        let waiting = barrier_waiting_on(inner);
        let pending = inner.match_round + 1;
        let now = inner.now_ms;
        // Never more than 45 s in total, and a client that reported a load
        // error gets one 10 s retry window: then it sits this round out
        // (a spectator for the round, not a loss) instead of stalling everyone.
        let deadline_passed = now >= inner.barrier_deadline_ms;
        let failed: Vec<PeerId> = waiting.iter().copied().filter(|id| {
            deadline_passed || inner.sess.load_errors.get(id)
                .map_or(false, |(r, _, t)| *r == pending && now.saturating_sub(*t) >= LOAD_RETRY_MS)
        }).collect();
        if waiting.is_empty() {
            inner.barrier_passed = true;
            inner.match_state_dirty = true;
            info!(round = pending, "match: all clients loaded; countdown running");
        } else if failed.len() == waiting.len() {
            for id in failed {
                let error = inner.sess.load_errors.get(&id).map(|(_, e, _)| e.clone())
                    .unwrap_or_else(|| "timeout".to_string());
                sit_out(inner, id, &error);
            }
            warn!(round = pending, sat_out = inner.sess.sat_out.len(), deadline_passed,
                  "match: load barrier released without the clients that failed to load");
            inner.barrier_passed = true;
            inner.match_state_dirty = true;
        }
    }

    // Live round whose other fighters left / stopped pinging (3+ players, so
    // no pause): end it instead of leaving one player alone forever.
    if inner.match_state == "live" && multi {
        check_round_end(inner, false);
    }
}

/// Live pauses one player may take per match.
pub(super) const PAUSES_PER_PLAYER: u32 = 1;
/// Live pauses per match in all, whoever drops (hard cap).
pub(super) const MAX_MATCH_PAUSES: u32 = 2;

/// Participant keys of this match that are not present (gone or stalled).
fn absent_participants(inner: &Inner, present: &[PeerId]) -> Vec<session::PlayerKey> {
    let here: HashSet<session::PlayerKey> = inner.peers.values()
        .filter(|p| present.contains(&p.id))
        .map(session::peer_key)
        .collect();
    inner.participants.iter().filter(|k| !here.contains(*k)).copied().collect()
}

/// A participant that drops out of a Live round with no pause
/// left loses the round, as if dead (killer = its last attacker). Its
/// connection may still be open (a stall) — it is out for this round either
/// way, and coming back later does not revive it.
fn drop_loses_round(inner: &mut Inner, droppers: &[session::PlayerKey]) {
    let round = inner.match_round;
    let ids: Vec<PeerId> = inner.peers.values()
        .filter(|p| p.alive && droppers.contains(&session::peer_key(p)))
        .map(|p| p.id)
        .collect();
    warn!(round, ?ids, pauses = inner.match_pauses,
          "match: participant dropped mid-round with no pause left; it loses the round");
    crate::events::emit("drop_forfeit_round", serde_json::json!({
        "round": round, "peers": ids, "match_id": inner.sess.match_id,
    }));
    for id in ids {
        let killer = crate::combat::ledger_last_attacker(id, round);
        declare_death_unprotected(inner, id, killer, DEATH_LEFT);
    }
    // A dropper whose connection is already gone is simply not standing.
    check_round_end(inner, false);
}

/// `id` failed to load the pending round: it spectates this round (its
/// `loaded_round` stays behind, so `go_live` leaves it out; no death, no
/// loss). Everyone is told who and why (Notice LOAD_FAILED: nick, error).
pub(super) fn sit_out(inner: &mut Inner, id: PeerId, error: &str) {
    if inner.sess.sat_out.iter().any(|(p, _, _)| *p == id) { return; }
    let nick = inner.peers.values().find(|p| p.id == id).map(|p| p.nick.clone()).unwrap_or_default();
    let seat = session::seat_of_peer(inner, id);
    let round = inner.match_round + 1;
    warn!(peer_id = id, %nick, round, %error, "match: client failed to load; it sits this round out");
    crate::events::emit("load_failed", serde_json::json!({
        "peer_id": id, "seat": seat, "nick": nick, "error": error, "round": round,
        "match_id": inner.sess.match_id,
    }));
    let seat_s = seat.map(|s| s.to_string()).unwrap_or_default();
    push_notice(inner, v5::Notice::LOAD_FAILED, &[&nick, error, &seat_s, &round.to_string()]);
    push_server_chat(inner, &format!("{} failed to load: {} (sits out round {})", nick, error, round));
    inner.sess.sat_out.push((id, nick, error.to_string()));
}

pub(super) fn begin_countdown(inner: &mut Inner, ms: u64) {
    inner.sess.load_errors.clear();
    inner.sess.sat_out.clear();
    inner.match_state = "countdown".into();
    inner.countdown_ms = ms;
    inner.barrier_passed = false;
    inner.barrier_deadline_ms = inner.now_ms + BARRIER_TIMEOUT_MS;
    inner.settle_ms = 0;
    inner.match_state_dirty = true;
    info!(round = inner.match_round + 1, "match: countdown begins (waiting for clients to load)");
    plan_spawns(inner);
}

/// Spawn plan for the pending round (docs/development/subsystems/spawns.md):
/// every connected peer gets a distinct point of the locked arena, chosen deterministically for
/// maximum separation. Clients place their pawn only on this plan.
/// The order is by seat (stable per player key across reconnects), not by
/// peer id or HashMap order.
fn plan_spawns(inner: &mut Inner) {
    let round = inner.match_round + 1;
    // Every pawn is about to be teleported onto its new spawn.
    for p in inner.peers.values_mut() { p.last_valid_pos = None; }
    session::reconcile_seats(inner);
    // spawns::assign orders by its `peer` field: pass the seat there, then map
    // each assignment back to the peer id.
    let mut by_seat: HashMap<u32, PeerId> = HashMap::new();
    let seats: Vec<crate::spawns::Seat> = inner.peers.values()
        .map(|p| {
            let seat = inner.sess.seats.get(&session::peer_key(p)).copied().unwrap_or(255) as u32;
            let order = (seat << 24) | (p.id & 0x00FF_FFFF);
            by_seat.insert(order, p.id);
            crate::spawns::Seat { peer: order, team: 0 }
        })
        .collect();
    let mut plan = crate::spawns::assign(&inner.match_arena, round, &seats);
    for a in plan.iter_mut() {
        if let Some(pid) = by_seat.get(&a.peer_id) { a.peer_id = *pid; }
        a.protect_ms = a.protect_ms.max(SPAWN_PROTECT_MS);
    }
    inner.spawn_plan = plan;
    inner.spawn_round = round;
    if inner.spawn_plan.is_empty() {
        warn!(arena = %inner.match_arena, round, "spawn plan: no spawn table for this arena; clients keep the native spawn");
    }
    for s in &inner.spawn_plan {
        let (src, derived) = crate::spawns::describe(&inner.match_arena, s.slot);
        info!(round, peer_id = s.peer_id, slot = s.slot, src, derived, x = s.pos[0], y = s.pos[1], z = s.pos[2],
              yaw = s.yaw, arena = %inner.match_arena, "spawn plan");
    }
    inner.match_state_dirty = true;
}

/// While the countdown runs (clients loading/placing): seat peers that joined
/// or reconnected, free the seats of peers that left. Existing seats never
/// move, so a client that is already placed stays valid.
pub(super) fn sync_spawn_plan(inner: &mut Inner) {
    if inner.match_state != "countdown" || inner.spawn_round != inner.match_round + 1 { return; }
    let mut ids: Vec<PeerId> = inner.peers.values().map(|p| p.id).collect();
    ids.sort_unstable();
    let before = inner.spawn_plan.len();
    inner.spawn_plan.retain(|s| ids.contains(&s.peer_id));
    let mut changed = inner.spawn_plan.len() != before;
    let arena = inner.match_arena.clone();
    for id in ids {
        if crate::spawns::add_seat(&arena, inner.spawn_round, &mut inner.spawn_plan, id) {
            if let Some(s) = inner.spawn_plan.last_mut() { s.protect_ms = s.protect_ms.max(SPAWN_PROTECT_MS); }
            if let Some(s) = inner.spawn_plan.last() {
                info!(round = inner.spawn_round, peer_id = id, slot = s.slot, x = s.pos[0], y = s.pos[1], z = s.pos[2],
                      "spawn plan: seat added during countdown");
            }
            changed = true;
        }
    }
    if changed { inner.match_state_dirty = true; }
}

pub(super) fn reset_to_lobby(inner: &mut Inner) {
    inner.match_state = "lobby".into();
    inner.countdown_ms = 0;
    inner.settle_ms = 0;
    inner.round_deaths.clear();
    inner.match_round = 0;
    inner.spawn_round = 0;
    inner.spawn_plan.clear();
    inner.barrier_passed = true;
    inner.participants.clear();
    inner.wins_by_key.clear();
    inner.sess.load_errors.clear();
    inner.sess.sat_out.clear();
    inner.sess.placed.clear();
    inner.pauses_by_key.clear();
    inner.match_pauses = 0;
    inner.paused_from_live = false;
    inner.sess.void_streak = 0;
    for mp in inner.match_peers.values_mut() { mp.loaded_round = 0; }
    // Wins belong to the match: the lobby starts from zero (the match result,
    // when there is one, was taken before this reset).
    for p in inner.peers.values_mut() { p.ready = false; p.alive = true; p.wins = 0; }
    inner.match_state_dirty = true;
    session::on_lobby(inner);
}


#[cfg(test)]
pub(super) mod round_tests {
    use super::*;

    /// Also the PeerState fixture of session_tests.rs (one constructor site).
    pub(crate) fn peer(id: PeerId, nick: &str) -> PeerState {
        PeerState {
            id, nick: nick.into(), cid: 0, player_key: [0; 32],
            last_seen_ms: 0, last_root: None,
            ready: true, last_valid_pos: None, last_valid_ms: 0, wins: 0, alive: true,
            is_admin: false,
        }
    }

    fn live_duel() -> ServerState {
        let st = ServerState::new(8);
        {
            let mut i = st.inner.try_lock().unwrap();
            for (n, id) in [("a", 1u32), ("b", 2)] {
                let addr: SocketAddr = format!("127.0.0.1:{}", 9000 + id).parse().unwrap();
                i.peers.insert(addr, peer(id, n));
            }
            i.participants = vec![session::provisional_key("a"), session::provisional_key("b")];
            i.match_state = "live".into();
            i.match_round = 1;
        }
        st
    }

    fn settle(i: &mut Inner) {
        if i.settle_ms > 0 {
            i.settle_ms = 0;
            finalize_round(i);
        }
    }

    fn wins(i: &Inner, id: PeerId) -> u32 { i.peers.values().find(|p| p.id == id).unwrap().wins }

    #[test]
    fn countdown_publishes_distinct_spawn_plan_and_seats_late_peers() {
        let st = live_duel();
        let mut i = st.inner.try_lock().unwrap();
        i.match_arena = "Map_Arena_Pit".into();
        begin_countdown(&mut i, NEXT_ROUND_COUNTDOWN_MS);
        assert_eq!(i.spawn_round, 2);
        assert_eq!(i.spawn_plan.len(), 2);
        assert_ne!(i.spawn_plan[0].slot, i.spawn_plan[1].slot);
        let first = i.spawn_plan.clone();
        // Recomputing the same round gives the same plan (idempotent resend).
        plan_spawns(&mut i);
        assert_eq!(i.spawn_plan, first);
        // A peer joining during the countdown gets a free seat; nobody moves.
        let addr: SocketAddr = "127.0.0.1:9100".parse().unwrap();
        i.peers.insert(addr, peer(7, "c"));
        sync_spawn_plan(&mut i);
        assert_eq!(&i.spawn_plan[..2], &first[..]);
        assert_eq!(i.spawn_plan.len(), 3);
        let slots: HashSet<u8> = i.spawn_plan.iter().map(|s| s.slot).collect();
        assert_eq!(slots.len(), 3);
        // Leaving frees the seat; back in the lobby the plan is gone.
        i.peers.remove(&addr);
        sync_spawn_plan(&mut i);
        assert_eq!(i.spawn_plan.len(), 2);
        reset_to_lobby(&mut i);
        assert!(i.spawn_plan.is_empty() && i.spawn_round == 0);
    }

    #[test]
    fn kill_settles_then_one_winner() {
        let st = live_duel();
        let mut i = st.inner.try_lock().unwrap();
        assert!(declare_death(&mut i, 2, 1, DEATH_DAMAGE));
        assert_eq!(i.match_state, "roundover");
        assert_eq!(i.match_reason, "pending");
        assert_eq!(i.last_winner, 0, "no winner published while settling");
        assert!(combat_open(&i), "trade window open");
        assert!(!declare_death(&mut i, 2, 1, DEATH_REPORTED), "idempotent");
        settle(&mut i);
        assert_eq!(i.last_winner, 1);
        assert_eq!(wins(&i, 1), 1);
        assert!(!combat_open(&i));
        // A late death report after the result is fixed changes nothing.
        assert!(!declare_death(&mut i, 1, 2, DEATH_REPORTED));
        assert_eq!(i.last_winner, 1);
        assert!(i.out_msgs.iter().any(|(_, m)| *m == death_msg(2, 1, 1, 1)));
    }

    #[test]
    fn mutual_kill_in_window_is_a_draw_in_either_order() {
        for first in [1u32, 2] {
            let st = live_duel();
            let mut i = st.inner.try_lock().unwrap();
            let second = 3 - first;
            assert!(declare_death(&mut i, first, second, DEATH_DAMAGE));
            i.settle_ms -= 100; // trade hit lands 100 ms later
            assert!(declare_death(&mut i, second, first, DEATH_DAMAGE));
            settle(&mut i);
            assert_eq!(i.match_state, "roundover");
            assert_eq!(i.match_reason, "draw");
            assert_eq!(i.last_winner, 0);
            assert_eq!(wins(&i, 1) + wins(&i, 2), 0);
        }
    }

    #[test]
    fn deaths_outside_an_open_round_are_ignored() {
        let st = live_duel();
        let mut i = st.inner.try_lock().unwrap();
        i.match_state = "countdown".into();
        assert!(!declare_death(&mut i, 1, 2, DEATH_REPORTED));
        assert!(i.peers.values().all(|p| p.alive));
    }

    #[test]
    fn three_player_round_ends_when_others_leave() {
        let st = live_duel();
        let mut i = st.inner.try_lock().unwrap();
        let addr: SocketAddr = "127.0.0.1:9003".parse().unwrap();
        i.peers.insert(addr, peer(3, "c"));
        i.participants.push(session::provisional_key("c"));
        assert!(declare_death(&mut i, 2, 1, DEATH_DAMAGE));
        assert_eq!(i.match_state, "live", "two still standing");
        i.peers.remove(&addr); // c drops mid-round; a duel would pause, 3p continues
        match_step(&mut i);
        assert_eq!(i.match_state, "roundover");
        settle(&mut i);
        assert_eq!(i.last_winner, 1);
    }

    /// A drop in a Live duel pauses it once per player; the
    /// player coming back resumes the SAME round (no countdown, no fresh
    /// spawn, health/ledger untouched).
    #[test]
    fn first_drop_pauses_and_the_same_round_resumes() {
        let st = live_duel();
        let mut i = st.inner.try_lock().unwrap();
        let b: SocketAddr = "127.0.0.1:9002".parse().unwrap();
        let pb = i.peers.remove(&b).unwrap();
        match_step(&mut i);
        assert_eq!(i.match_state, "paused");
        i.peers.insert(b, pb);
        match_step(&mut i);
        assert_eq!(i.match_state, "live", "resumed, not replayed");
        assert_eq!(i.match_round, 1, "same round");
        assert!(i.peers.values().all(|p| p.alive));
        assert!(i.spawn_plan.is_empty(), "no new spawn plan");
    }

    /// Exploit regression: the 5-hp duellist who drops again (no
    /// pause left) loses the round instead of voiding it; coming back does not
    /// revive it.
    #[test]
    fn second_drop_by_the_same_player_loses_the_round() {
        let st = live_duel();
        let mut i = st.inner.try_lock().unwrap();
        let b: SocketAddr = "127.0.0.1:9002".parse().unwrap();
        for attempt in 0..2 {
            let pb = i.peers.remove(&b).unwrap();
            match_step(&mut i);
            if attempt == 0 {
                assert_eq!(i.match_state, "paused");
                i.peers.insert(b, pb);
                match_step(&mut i);
                assert_eq!(i.match_state, "live");
            } else {
                // (Its PeerState is gone: a return is a fresh, dead join.)
                assert_eq!(i.match_state, "roundover", "no second pause");
                drop(pb);
            }
        }
        // The dropper's connection was gone: not standing; the result settles.
        settle(&mut i);
        assert_eq!(i.last_winner, 1);
        assert_eq!(wins(&i, 1), 1);
    }

    /// A stalled (still connected) dropper with no pause left is declared
    /// dead (DEATH_LEFT), so its return inside the settle window is no draw.
    #[test]
    fn stalled_dropper_without_budget_is_declared_dead() {
        let st = live_duel();
        let mut i = st.inner.try_lock().unwrap();
        let key_b = session::provisional_key("b");
        i.pauses_by_key.insert(key_b, PAUSES_PER_PLAYER);
        // b is a game client that stopped talking 4 s ago.
        i.now_ms = 40_000;
        for p in i.peers.values_mut() { p.last_seen_ms = if p.id == 2 { 40_000 - 4000 } else { 40_000 }; }
        for id in [1u32, 2] {
            let m = i.match_peers.entry(id).or_default();
            m.aware = true;
            m.last_ping_ms = if id == 2 { 40_000 - 4000 } else { 40_000 };
        }
        match_step(&mut i);
        assert_eq!(i.match_state, "roundover");
        assert!(i.round_deaths.iter().any(|&(p, _, c)| p == 2 && c == DEATH_LEFT));
        // It comes back during the settle: still the loser.
        for p in i.peers.values_mut() { p.last_seen_ms = 40_000; }
        settle(&mut i);
        assert_eq!(i.last_winner, 1);
    }

    /// The match-wide hard cap: after MAX_MATCH_PAUSES pauses nobody pauses.
    #[test]
    fn pauses_have_a_match_wide_hard_cap() {
        let st = live_duel();
        let mut i = st.inner.try_lock().unwrap();
        i.match_pauses = MAX_MATCH_PAUSES;
        let a: SocketAddr = "127.0.0.1:9001".parse().unwrap();
        i.peers.remove(&a);
        match_step(&mut i);
        assert_eq!(i.match_state, "roundover");
        settle(&mut i);
        assert_eq!(i.last_winner, 2);
        // A new match starts with a fresh budget.
        reset_to_lobby(&mut i);
        assert_eq!((i.match_pauses, i.pauses_by_key.len()), (0, 0));
    }

    /// A pause that runs out the grace without the player's return is a
    /// forfeit (a missing Leave after a timeout counts as leaving).
    #[test]
    fn pause_grace_expiry_is_a_forfeit() {
        let st = live_duel();
        let mut i = st.inner.try_lock().unwrap();
        let b: SocketAddr = "127.0.0.1:9002".parse().unwrap();
        i.peers.remove(&b);
        match_step(&mut i);
        assert_eq!(i.match_state, "paused");
        i.countdown_ms = 1;
        let t = i.now_ms + 33;
        super::super::tick::advance(&mut i, t);
        assert_eq!(i.match_state, "match_over");
        assert_eq!(i.match_reason, "forfeit");
        assert_eq!(i.last_winner, 1);
    }

    #[test]
    fn match_point_ends_match() {
        let st = live_duel();
        let mut i = st.inner.try_lock().unwrap();
        i.best_of = 1;
        declare_death(&mut i, 1, 2, DEATH_VITALS);
        settle(&mut i);
        assert_eq!(i.match_state, "match_over");
        assert_eq!(i.last_winner, 2);
    }
}
