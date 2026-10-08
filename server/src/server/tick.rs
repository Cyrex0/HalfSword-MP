//! The server tick loop: held-hit flush, snapshots, timeouts, match_step,
//! match / session / character / result broadcasts, admin state after an
//! admin timed out. The per-tick state transitions live in `advance` (sync, unit- and
//! property-tested); the session snapshot in `session::session_due`.
//!
//! See docs/development/server-modules.md.

use super::*;

/// One tick of match flow under the lock at server time `now_ms`:
/// supervision, settle, countdown transitions, then the session bookkeeping
/// (seats, phase events). Timers run on elapsed time, not on the number of
/// calls, so the outcome is the same at any tick rate. Returns the match
/// result to append to history.jsonl when a match just ended.
pub(super) fn advance(inner: &mut Inner, now_ms: u64) -> Option<MatchResult> {
    inner.server_tick = inner.server_tick.wrapping_add(1);
    let dt = now_ms.saturating_sub(inner.now_ms);
    inner.now_ms = inner.now_ms.max(now_ms);
    let mut match_result: Option<MatchResult> = None;

    // Match supervision first: reconnect restore, drop -> pause/forfeit,
    // load barrier.
    match_step(inner);
    // No admin connected: all-ready players start the match.
    session::auto_start_step(inner);
    sync_spawn_plan(inner);

    // Simultaneous-kill settle: fix the round result at the deadline.
    if inner.settle_ms > 0 {
        if inner.match_state == "roundover" {
            inner.settle_ms = inner.settle_ms.saturating_sub(dt);
            if inner.settle_ms == 0 { finalize_round(inner); }
        } else {
            inner.settle_ms = 0;
        }
    }

    // The mode's own round end (King of the hill target, the round clock,
    // sudden death): the same settle as the last side standing.
    if modes::step(inner, dt) && inner.match_state == "live" {
        begin_settle(inner, standing(inner).len());
    }

    // Countdown bookkeeping / state transitions. A countdown whose load
    // barrier is still open stays frozen.
    let frozen = inner.match_state == "countdown" && !inner.barrier_passed;
    if inner.countdown_ms > 0 && !frozen {
        inner.countdown_ms = inner.countdown_ms.saturating_sub(dt);
        if inner.countdown_ms == 0 {
            let next = match inner.match_state.as_str() {
                "countdown" => Some("live"),
                "roundover" => Some("countdown"),
                "match_over" => Some("lobby"),
                "paused" => Some("paused_expired"),
                _ => None,
            };
            if next == Some("paused_expired") {
                // One side (player or team) still here wins by forfeit.
                if let Some((_, pid)) = modes::single_side(inner, |p| is_present(inner, p)) {
                    forfeit_to(inner, pid);
                } else {
                    reset_to_lobby(inner);
                }
            } else if let Some(n) = next {
                if n == "live" {
                    go_live(inner);
                } else if n == "countdown" {
                    begin_countdown(inner, NEXT_ROUND_COUNTDOWN_MS);
                } else {
                    // match_over → lobby: the history.jsonl line + reset wins.
                    // The winner fixed when the match ended (it may have
                    // left since); not whoever has the most wins among the
                    // peers still connected.
                    let winner = inner.sess.match_winner.take().filter(|_| !inner.peers.is_empty());
                    if let Some((wid, wnick, _ww)) = winner {
                        let sb: Vec<(PeerId, String, u32)> = inner.peers.values()
                            .map(|p| (p.id, p.nick.clone(), p.wins)).collect();
                        let ts = std::time::SystemTime::now()
                            .duration_since(std::time::UNIX_EPOCH)
                            .map(|d| d.as_millis() as u64).unwrap_or(0);
                        match_result = Some(MatchResult {
                            round_num: inner.match_round, best_of: inner.best_of,
                            winner_peer_id: wid, winner_nick: wnick,
                            scoreboard: sb, time_utc_ms: ts,
                        });
                    }
                    reset_to_lobby(inner);
                    for p in inner.peers.values_mut() { p.wins = 0; }
                }
            }
        }
    }

    session::reconcile_seats(inner);
    session::observe_phase(inner);
    match_result
}

/// countdown → live: the round starts.
fn go_live(inner: &mut Inner) {
    inner.match_state = "live".into();
    inner.match_round = inner.match_round.wrapping_add(1);
    // Loaded participants fight; stragglers that missed the barrier and
    // non-participants sit the round out.
    let aware = barrier_session(inner);
    let round = inner.match_round;
    // Late joiners (spectating since they joined) whose game has this round's
    // arena loaded fight from now on.
    if aware && !inner.participants.is_empty() {
        let late: Vec<(session::PlayerKey, String)> = inner.peers.values()
            .filter(|p| !inner.participants.contains(&session::peer_key(p)))
            .filter(|p| inner.match_peers.get(&p.id)
                .map(|m| m.aware && m.loaded_round >= round).unwrap_or(false))
            .map(|p| (session::peer_key(p), p.nick.clone()))
            .collect();
        for (key, nick) in late {
            info!(%nick, round, "match: late joiner enters the next round");
            inner.participants.push(key);
            modes::seat_late(inner, key);
        }
    }
    inner.round_deaths.clear();
    inner.settle_ms = 0;
    crate::combat::ledger_begin_round(round);
    let alive_by_addr: Vec<(SocketAddr, bool)> = inner.peers.iter().map(|(a, p)| {
        let loaded = !aware || inner.match_peers.get(&p.id)
            .map(|m| m.aware && m.loaded_round >= round).unwrap_or(false);
        (*a, loaded && is_participant(inner, p))
    }).collect();
    for (a, alive) in alive_by_addr {
        if let Some(p) = inner.peers.get_mut(&a) { p.alive = alive; }
    }
    inner.match_reason.clear();
    inner.sess.live_ms = inner.now_ms;
    modes::on_live(inner);
    // Too few fighters loaded (the others failed to load or missed the 45 s
    // barrier): the round is void — no winner, nobody loses — instead of a
    // lone fighter "winning" it or the match stalling. Repeated voids end the
    // match (back to the lobby) rather than loop.
    let fighters = inner.peers.values().filter(|p| p.alive).count();
    let needed = if inner.participants.len() >= 2 { 2 } else { 1 };
    if aware && fighters < needed {
        inner.sess.void_streak += 1;
        warn!(round, fighters, streak = inner.sess.void_streak, "match: round void (too few fighters loaded)");
        crate::events::emit("round_void", serde_json::json!({
            "round": round, "fighters": fighters, "streak": inner.sess.void_streak, "match_id": inner.sess.match_id,
        }));
        if inner.sess.void_streak >= MAX_VOID_ROUNDS {
            warn!("match: players keep failing to load; back to lobby");
            push_server_chat(inner, "players keep failing to load the arena: back to the lobby");
            reset_to_lobby(inner);
            return;
        }
        inner.match_state = "roundover".into();
        inner.match_reason = "load_failed".into();
        inner.last_winner = 0;
        inner.settle_ms = 0;
        inner.countdown_ms = ROUNDOVER_MS;
        inner.match_state_dirty = true;
        return;
    }
    inner.sess.void_streak = 0;
    info!(round = inner.match_round, "match: live");
    // Spawns: the plan made at countdown start (S2CMatchState `spawns`, the
    // S2CSession roster) stays in force for this round.
    inner.match_state_dirty = true;
}

/// The history.jsonl line of a finished match.
pub(super) struct MatchResult {
    pub round_num: u32,
    pub best_of: u8,
    pub winner_peer_id: PeerId,
    pub winner_nick: String,
    pub scoreboard: Vec<(PeerId, String, u32)>,
    pub time_utc_ms: u64,
}

/// Map transport RTTs (by address) to peers: (peer id, srtt) for lag comp and
/// the `pings` record (every connected peer; 0 = no sample yet).
pub(super) fn build_pings(inner: &Inner, rtts: &[(SocketAddr, f32)], epoch: u64, now_ms: u64)
    -> (Vec<(PeerId, f32)>, Vec<u8>) {
    let by_addr: HashMap<SocketAddr, f32> = rtts.iter().copied().collect();
    let mut samples = Vec::new();
    let mut rows: Vec<hsmp_ipc::schema::session::PingRow> = inner.peers.iter().map(|(a, p)| {
        let rtt = by_addr.get(a).copied().unwrap_or(0.0);
        if rtt > 0.0 { samples.push((p.id, rtt)); }
        hsmp_ipc::schema::session::PingRow {
            peer_id: p.id,
            rtt_ms: rtt.round().clamp(0.0, u16::MAX as f32) as u16,
            seat: session::seat_of_peer(inner, p.id).unwrap_or(0xFF),
            _r: 0,
        }
    }).collect();
    rows.sort_by_key(|e| e.peer_id);
    (samples, pings_msg(epoch, now_ms, &rows))
}

/// About once a second: feed every peer's transport srtt to lag comp (a
/// player who was never hit would otherwise have only a 120 ms guess), and send the `pings`
/// record to the peers that negotiated `caps::PING` (lobby ping column).
async fn pings_tick(socket: &UdpSocket, state: &Arc<ServerState>) {
    let rtts = state.net.srtt_by_addr();
    let vars = state.net.rttvar_by_addr();
    let now_ms = state.net.now_ms();
    let (samples, msg, to) = {
        let inner = state.inner.lock().await;
        let (samples, msg) = build_pings(&inner, &rtts, inner.sess.epoch, now_ms);
        // Transport jitter bounds the stream jitter lag comp believes.
        for (a, v) in &vars {
            if let Some(p) = inner.peers.get(a) { crate::lagcomp::note_net_jitter(p.id, *v); }
        }
        let to: Vec<SocketAddr> = inner.peers.iter()
            .filter(|(_, p)| crate::interact::peer_caps(p.id) & hsmp_net::net::caps::PING != 0)
            .map(|(a, _)| *a).collect();
        (samples, msg, to)
    };
    for (id, rtt) in samples { crate::lagcomp::note_rtt(id, rtt); }
    if to.is_empty() { return; }
    broadcast_msg(socket, state, &to, msg).await;
}

/// Append one finished match to history.jsonl (append-only; server-side log).
async fn write_history(r: &MatchResult) {
    use tokio::io::AsyncWriteExt;
    if let Ok(mut f) = tokio::fs::OpenOptions::new().create(true).append(true).open("history.jsonl").await {
        let line = serde_json::json!({
            "time_utc_ms": r.time_utc_ms, "round": r.round_num, "best_of": r.best_of,
            "winner_id": r.winner_peer_id, "winner_nick": r.winner_nick,
            "scoreboard": r.scoreboard.iter().map(|(id, nick, w)| serde_json::json!([id, nick, w])).collect::<Vec<_>>(),
        }).to_string() + "\n";
        let _ = f.write_all(line.as_bytes()).await;
    }
}

/// A peer silent this long (no authenticated packet at all) is dropped.
pub(super) const PEER_TIMEOUT_MS: u64 = 60_000;

/// Peers silent for longer than PEER_TIMEOUT_MS at the current tick (`tick_locked` does the
/// same into its kept buffer).
#[cfg_attr(not(test), allow(dead_code))]
pub(super) fn timed_out(inner: &Inner) -> Vec<(SocketAddr, PeerId)> {
    inner.peers.iter()
        .filter(|(_, p)| inner.now_ms.saturating_sub(p.last_seen_ms) > PEER_TIMEOUT_MS)
        .map(|(a, p)| (*a, p.id))
        .collect()
}
/// Repeat of this round's S2CDeath records: 1 s in the lobby, 333 ms in a match.
const DEATHS_REPEAT_LOBBY_MS: u64 = 1000;
const DEATHS_REPEAT_MATCH_MS: u64 = 333;
/// Transport RTTs to lag comp and the `pings` record.
const PINGS_EVERY_MS: u64 = 1000;
/// Accepted `--tick-hz` range.
pub const TICK_HZ_MIN: u32 = 20;
pub const TICK_HZ_MAX: u32 = 240;

/// The tick period for `tick_hz` (clamped to the accepted range).
pub fn tick_period(tick_hz: u32) -> Duration {
    Duration::from_nanos(1_000_000_000 / tick_hz.clamp(TICK_HZ_MIN, TICK_HZ_MAX) as u64)
}

/// Buffers and timestamps the tick keeps from one tick to the next, so a steady tick
/// allocates nothing (`tick_locked` reuses them).
#[derive(Default)]
pub(super) struct TickScratch {
    /// Kit view filled before the state lock, swapped into `Inner::sess.kits`.
    kits: KitView,
    /// The roulette / brawl kit generation loadout.rs was last told about.
    kit_gen: u32,
    ids: Vec<PeerId>,
    timed: Vec<(SocketAddr, PeerId)>,
    deaths_sent_ms: u64,
}

/// What the locked part of a tick leaves for the sends after the lock.
pub(super) struct TickOut {
    pub timed_out: Vec<(SocketAddr, PeerId)>,
    pub match_result: Option<MatchResult>,
    /// The `session` record and who gets it.
    pub session: Option<(Vec<SocketAddr>, Vec<u8>)>,
    pub admin_promoted: bool,
    /// A new imposed round kit (None inside = players' own kits again) for loadout.rs.
    pub kit: Option<Option<crate::loadout::KitSel>>,
}

/// The part of a tick that runs under the state lock at `now` (transport clock): match flow
/// (`advance`), the kit round lock, free viewers for the relay, death repeats, the `session`
/// record, timeouts.
pub(super) fn tick_locked(state: &ServerState, inner: &mut Inner, sc: &mut TickScratch, now: u64) -> TickOut {
    if inner.native.is_some() {
        super::native_glue::tick(inner, now);
        let gone: Vec<_> = inner.peers.iter().filter(|(_, p)| now.saturating_sub(p.last_seen_ms) > PEER_TIMEOUT_MS).map(|(a, p)| (*a, p.id)).collect();
        for (addr, id) in &gone { inner.peers.remove(addr); session::forget_peer_locked(inner, *id); state.relay.forget(addr); }
        state.net.reconcile(&inner.peers);
        return TickOut { timed_out: gone, match_result: None, session: None, admin_promoted: false, kit: None };
    }
    std::mem::swap(&mut inner.sess.kits, &mut sc.kits);
    let match_result = advance(inner, now);
    // Kit facts are frozen while a round is fought.
    let fighting = matches!(inner.match_state.as_str(), "live" | "paused")
        || (inner.match_state == "roundover" && inner.settle_ms > 0);
    sc.ids.clear();
    if fighting { sc.ids.extend(inner.peers.values().map(|p| p.id)); }
    // Spectators and the dead watch anyone: the relay ranks every
    // player nearest for them (their parked body says nothing).
    state.relay.set_free_viewers(inner.peers.iter().filter(|(_, p)| fighting && !p.alive).map(|(a, _)| *a));
    crate::loadout::set_round_lock(&sc.ids);

    // Deaths are final and must reach everyone despite loss: repeat this
    // round's S2CDeath on change and periodically (1 s in the lobby,
    // 333 ms in a match; tiny; receivers dedup by (peer, round)).
    let every = if inner.match_state == "lobby" { DEATHS_REPEAT_LOBBY_MS } else { DEATHS_REPEAT_MATCH_MS };
    let due = inner.match_state_dirty || now.saturating_sub(sc.deaths_sent_ms) >= every;
    if due { sc.deaths_sent_ms = now; inner.match_state_dirty = false; }
    if due && matches!(inner.match_state.as_str(), "live" | "roundover" | "match_over") {
        let round = inner.match_round;
        let again: Vec<Vec<u8>> = inner.round_deaths.iter()
            .map(|&(peer_id, killer, cause, life)| combat_glue::scoped_death_msg(peer_id,round,killer,cause,inner.sess.match_id,life))
            .collect();
        for m in again { inner.out_msgs.push((None, m)); }
    }

    // The `session` record: on change, else 1 Hz lobby / 3 Hz match.
    // The transport clock: the one the welcome's server_time_ms uses. Encoded
    // ONCE; every peer gets the same bytes.
    let session = session::session_due(inner, now)
        .filter(|_| !inner.peers.is_empty())
        .map(|s| (inner.peers.keys().copied().collect::<Vec<_>>(), session_msg(&s)));
    // The `mode` / `zone` records (peers with caps::MODES / ZONE only).
    let mut mode_out = std::mem::take(&mut inner.out_msgs);
    modes::records_due(inner, now, &mut mode_out);
    inner.out_msgs = mode_out;
    // Roulette / brawl: a new round kit for loadout.rs (applied after the lock).
    let kit = (sc.kit_gen != inner.modes.kit_gen).then(|| {
        sc.kit_gen = inner.modes.kit_gen;
        inner.modes.kit.as_ref().map(|k| k.selection())
    });

    // `timed_out`, into the kept buffer.
    sc.timed.clear();
    sc.timed.extend(inner.peers.iter()
        .filter(|(_, p)| inner.now_ms.saturating_sub(p.last_seen_ms) > PEER_TIMEOUT_MS)
        .map(|(a, p)| (*a, p.id)));
    let mut admin_promoted = false;
    for (addr, _) in &sc.timed {
        if let Some(gone) = inner.peers.remove(addr) {
            session::forget_peer_locked(inner, gone.id);
            // As a clean leave: nobody is promoted; the listen host
            // regains admin by its key when it reconnects.
            if session::admin_left(inner, &gone) || gone.is_admin { admin_promoted = true; }
        }
        state.relay.forget(addr);
    }
    // Keep the v5 transport in step with every removal path
    // (kick, ban, RCON, timeout) — no-op when unchanged.
    state.net.reconcile(&inner.peers);
    let timed_out = if sc.timed.is_empty() { Vec::new() } else { std::mem::take(&mut sc.timed) };
    TickOut { timed_out, match_result, session, admin_promoted, kit }
}

pub async fn tick_loop(
    socket: Arc<UdpSocket>,
    state: Arc<ServerState>,
    tick_hz: u32,
) -> anyhow::Result<()> {
    let mut ticker = time::interval(tick_period(tick_hz));
    ticker.set_missed_tick_behavior(time::MissedTickBehavior::Delay);

    let mut pings_sent_ms = 0u64;
    let mut sc = TickScratch::default();
    let mut addrs: Vec<SocketAddr> = Vec::new();

    loop {
        ticker.tick().await;
        let t_tick = std::time::Instant::now();
        // Relay ranking + bandwidth plan (every 500 ms), here on the tick and
        // not inside the receive loop's relay path.
        state.net.addrs_into(&mut addrs);
        state.relay.replan_with_paths(&addrs, || state.net.path_samples());
        flush_held_hits(&socket, &state).await;
        interact_tick(&socket, &state).await;
        // Kit rules and per-peer kit revisions for the session snapshot
        // (loadout.rs owns them; read before the state lock, never nested).
        crate::loadout::session_view_into(&mut sc.kits).await;
        let TickOut { timed_out, match_result, session: session_broadcast, admin_promoted, kit } = {
            let mut inner = state.inner.lock().await;
            let now = state.net.now_ms();
            tick_locked(&state, &mut inner, &mut sc, now)
        };

        if let Some((addrs, msg)) = session_broadcast {
            broadcast_msg(&socket, &state, &addrs, msg).await;
        }
        if let Some(k) = kit {
            crate::loadout::impose(&socket, &state, k).await;
        }
        if let Some(r) = match_result {
            write_history(&r).await;
        }
        flush_out(&socket, &state).await;
        let had_timeouts = !timed_out.is_empty();
        for (addr, id) in timed_out {
            crate::loadout::forget(id).await;
            // (The next session snapshot's roster no longer lists it: no PeerLeft.)
            info!(peer_id = id, %addr, "peer timed out");
        }
        // An admin timed out (peer_leave does the same for a clean leave,
        // done under the lock above): everyone gets the new admin state; with
        // no admin left, READY players start the match (auto_start_step).
        if had_timeouts && admin_promoted {
            let (id, addrs) = {
                let mut inner = state.inner.lock().await;
                inner.match_state_dirty = true;
                (inner.admin_peer_id, inner.peers.keys().copied().collect::<Vec<_>>())
            };
            info!(admin_now = id, "an admin timed out; no one is promoted (the listen host regains admin by key)");
            broadcast_admin_state(&socket, &state, &addrs).await;
        }
        let now = state.net.now_ms();
        if now.saturating_sub(pings_sent_ms) >= PINGS_EVERY_MS {
            pings_sent_ms = now;
            pings_tick(&socket, &state).await;
        }
        crate::perf::perf().tick(t_tick.elapsed().as_micros() as u32);
        crate::stats::tick(t_tick.elapsed().as_micros() as u32);
    }
}
#[cfg(test)]
mod alloc_tests {
    use super::*;
    use crate::server::match_core::round_tests::peer;

    /// A server with `n` connected peers (real, distinct player keys).
    fn server(n: u32) -> ServerState {
        let st = ServerState::new(n as usize);
        {
            let mut i = st.inner.try_lock().unwrap();
            for id in 1..=n {
                let addr: SocketAddr = format!("203.0.113.{id}:{}", 40_000 + id).parse().unwrap();
                let mut p = peer(id, &format!("player{id}"));
                p.player_key = [id as u8; 32];
                p.ready = false;
                i.peers.insert(addr, p);
            }
        }
        st
    }

    /// Everyone fights round 1 and every game pings (no pause, no stall, no transition).
    fn go_live(i: &mut Inner) {
        i.participants = i.peers.values().map(session::peer_key).collect();
        i.match_state = "live".into();
        i.match_round = 1;
        i.barrier_passed = true;
        let ids: Vec<PeerId> = i.peers.values().map(|p| p.id).collect();
        for id in ids {
            i.match_peers.insert(id, MatchPeer { aware: true, loaded_round: 1, last_ping_ms: 0, spawned_round: 1 });
        }
    }

    /// Allocations per tick and microseconds per tick of `tick_locked` over `ticks` ticks
    /// (ticks that send the `session` record are left out of the allocation count).
    /// The clock runs at 60 Hz from `clock`.
    fn run(st: &ServerState, sc: &mut TickScratch, clock: &mut u64, ticks: u32) -> (f64, f64) {
        let mut i = st.inner.try_lock().unwrap();
        let mut allocs = 0u64;
        let mut counted = 0u32;
        let t0 = std::time::Instant::now();
        for _ in 0..ticks {
            *clock += 1000 / 60;
            let now = *clock;
            for p in i.peers.values_mut() { p.last_seen_ms = now; }
            for m in i.match_peers.values_mut() { m.last_ping_ms = now; }
            let (n, out) = crate::alloc_count::count(|| tick_locked(st, &mut i, sc, now));
            assert!(out.timed_out.is_empty() && out.match_result.is_none());
            if out.session.is_none() {
                allocs += n;
                counted += 1;
            }
        }
        let us = t0.elapsed().as_secs_f64() * 1e6 / ticks as f64;
        (allocs as f64 / counted.max(1) as f64, us)
    }

    /// The steady tick (8, 16 and 32 players, lobby and live round) allocates nothing.
    #[test]
    fn steady_tick_does_not_allocate() {
        let mut bad = Vec::new();
        for (n, live) in [(8, false), (8, true), (16, false), (16, true), (32, false), (32, true)] {
            let st = server(n);
            if live { go_live(&mut st.inner.try_lock().unwrap()); }
            let mut sc = TickScratch::default();
            let mut clock = 1_000_000u64;
            run(&st, &mut sc, &mut clock, 50); // warm-up: first snapshot, seats, scratch capacity
            // The best of five runs: the machine may be busy with other work.
            let (mut allocs, mut us) = (0.0f64, f64::MAX);
            for _ in 0..5 {
                let (a, t) = run(&st, &mut sc, &mut clock, 2000);
                allocs = allocs.max(a);
                us = us.min(t);
            }
            let phase = if live { "live" } else { "lobby" };
            println!("tick_locked {n} players {phase}: {allocs:.1} allocations/tick, {us:.1} us/tick");
            assert_eq!(st.inner.try_lock().unwrap().match_state, phase);
            if allocs > 0.0 { bad.push((n, phase, allocs)); }
        }
        assert!(bad.is_empty(), "the steady tick allocates: {bad:?}");
    }
}
