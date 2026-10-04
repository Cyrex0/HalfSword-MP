//! The server tick loop: held-hit flush, snapshots, timeouts, match_step,
//! match / session / character / result broadcasts, admin state after an
//! admin timed out. The per-tick state transitions live in `advance` (sync, unit- and
//! property-tested); the session snapshot in `session::session_due`.
//!
//! See docs/development/server-modules.md.

use super::*;

/// One tick of match flow under the lock: supervision, settle, countdown
/// transitions, then the session bookkeeping (seats, phase events). Returns
/// the match result to append to history.jsonl when a match just ended.
pub(super) fn advance(inner: &mut Inner) -> Option<MatchResult> {
    inner.server_tick = inner.server_tick.wrapping_add(1);
    let mut match_result: Option<MatchResult> = None;

    // Match supervision first: reconnect restore, drop -> pause/forfeit,
    // load barrier.
    match_step(inner);
    // No admin connected: all-ready players start the match.
    session::auto_start_step(inner);
    sync_spawn_plan(inner);

    // Simultaneous-kill settle: fix the round result at the deadline.
    if inner.settle_ticks > 0 {
        if inner.match_state == "roundover" {
            inner.settle_ticks -= 1;
            if inner.settle_ticks == 0 { finalize_round(inner); }
        } else {
            inner.settle_ticks = 0;
        }
    }

    // Countdown bookkeeping / state transitions. A countdown whose load
    // barrier is still open stays frozen.
    let frozen = inner.match_state == "countdown" && !inner.barrier_passed;
    if inner.countdown_ticks > 0 && !frozen {
        inner.countdown_ticks -= 1;
        if inner.countdown_ticks == 0 {
            let next = match inner.match_state.as_str() {
                "countdown" => Some("live"),
                "roundover" => Some("countdown"),
                "match_over" => Some("lobby"),
                "paused" => Some("paused_expired"),
                _ => None,
            };
            if next == Some("paused_expired") {
                let present = present_participants(inner);
                if present.len() == 1 {
                    forfeit_to(inner, present[0]);
                } else {
                    reset_to_lobby(inner);
                }
            } else if let Some(n) = next {
                if n == "live" {
                    go_live(inner);
                } else if n == "countdown" {
                    begin_countdown(inner, NEXT_ROUND_COUNTDOWN_TICKS);
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
        }
    }
    inner.round_deaths.clear();
    inner.settle_ticks = 0;
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
    inner.sess.live_tick = inner.server_tick;
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
        inner.settle_ticks = 0;
        inner.countdown_ticks = ROUNDOVER_TICKS;
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

pub async fn tick_loop(
    socket: Arc<UdpSocket>,
    state: Arc<ServerState>,
    tick_hz: u32,
) -> anyhow::Result<()> {
    let period = Duration::from_micros((1_000_000 / tick_hz as u64).max(1));
    let mut ticker = time::interval(period);
    ticker.set_missed_tick_behavior(time::MissedTickBehavior::Delay);

    const TIMEOUT_TICKS: u32 = 60 * 30;
    let deaths_every = tick_hz;
    let mut tick_since_deaths = 0u32;
    let mut tick_since_pings = 0u32;

    loop {
        ticker.tick().await;
        let t_tick = std::time::Instant::now();
        // Relay ranking + bandwidth plan (every 500 ms), here on the tick and
        // not inside the receive loop's relay path.
        state.relay.replan_with_paths(&state.net.addrs(), || state.net.path_samples());
        flush_held_hits(&socket, &state).await;
        interact_tick(&socket, &state).await;
        // Kit rules and per-peer kit revisions for the session snapshot
        // (loadout.rs owns them; read before the state lock, never nested).
        let kits = crate::loadout::session_view().await;
        let mut admin_promoted = false;
        let (timed_out, match_result, session_broadcast) = {
            let mut inner = state.inner.lock().await;
            inner.sess.kits = kits;
            let tick = inner.server_tick.wrapping_add(1);

            let match_result = advance(&mut inner);
            // Kit facts are frozen while a round is fought.
            let fighting = matches!(inner.match_state.as_str(), "live" | "paused")
                || (inner.match_state == "roundover" && inner.settle_ticks > 0);
            let ids: Vec<PeerId> = if fighting { inner.peers.values().map(|p| p.id).collect() } else { Vec::new() };
            // Spectators and the dead watch anyone: the relay ranks every
            // player nearest for them (their parked body says nothing).
            state.relay.set_free_viewers(inner.peers.iter().filter(|(_, p)| fighting && !p.alive).map(|(a, _)| *a));
            crate::loadout::set_round_lock(&ids);

            // Deaths are final and must reach everyone despite loss: repeat this
            // round's S2CDeath on change and periodically (1 s in the lobby,
            // 333 ms in a match; tiny; receivers dedup by (peer, round)).
            tick_since_deaths = tick_since_deaths.wrapping_add(1);
            let every = if inner.match_state == "lobby" { deaths_every } else { (deaths_every / 3).max(1) };
            let due = inner.match_state_dirty || tick_since_deaths >= every;
            if due { tick_since_deaths = 0; inner.match_state_dirty = false; }
            if due && matches!(inner.match_state.as_str(), "live" | "roundover" | "match_over") {
                let round = inner.match_round;
                let again: Vec<Vec<u8>> = inner.round_deaths.iter()
                    .map(|&(peer_id, killer, cause)| death_msg(peer_id, round, killer, cause))
                    .collect();
                for m in again { inner.out_msgs.push((None, m)); }
            }

            let mut timed: Vec<(SocketAddr, PeerId)> = Vec::new();
            let addrs: Vec<SocketAddr> = inner.peers.keys().copied().collect();

            // The `session` record: on change, else 1 Hz lobby / 3 Hz match.
            // The transport clock: the one the welcome's server_time_ms uses. Encoded
            // ONCE; every peer gets the same bytes.
            let now_ms = state.net.now_ms();
            let snap_msg = session::session_due(&mut inner, now_ms)
                .filter(|_| !addrs.is_empty())
                .map(|s| session_msg(&s));

            for (addr, p) in inner.peers.iter() {
                if tick.wrapping_sub(p.last_seen_tick) > TIMEOUT_TICKS {
                    timed.push((*addr, p.id));
                }
            }
            for (addr, _) in &timed {
                if let Some(gone) = inner.peers.remove(addr) {
                    session::forget_peer_locked(&mut inner, gone.id);
                    // As a clean leave: nobody is promoted; the listen host
                    // regains admin by its key when it reconnects.
                    if session::admin_left(&mut inner, &gone) || gone.is_admin { admin_promoted = true; }
                }
                state.relay.forget(addr);
            }
            // Keep the v5 transport in step with every removal path
            // (kick, ban, RCON, timeout) — no-op when unchanged.
            state.net.reconcile(inner.peers.keys());
            (timed, match_result, snap_msg.map(|m| (addrs, m)))
        };

        if let Some((addrs, msg)) = session_broadcast {
            broadcast_msg(&socket, &state, &addrs, msg).await;
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
        tick_since_pings = tick_since_pings.wrapping_add(1);
        if tick_since_pings >= tick_hz {
            tick_since_pings = 0;
            pings_tick(&socket, &state).await;
        }
        crate::perf::perf().tick(t_tick.elapsed().as_micros() as u32);
        crate::stats::tick(t_tick.elapsed().as_micros() as u32);
    }
}
