//! Combat glue (protocol v6 records, schema/combat.rs): validate the combat
//! records in place, act on combat.rs verdicts (forward / ack / hold, ledger,
//! lethal-hit deaths), relay vitals, and flush lag-compensated held hits each tick.
//!
//! Records in (C2S): `damage` (a claim), `damage_ack` (the victim's sidecar got
//! it), `clash` / `touch` (parry evidence), `death_report`, `vitals`.
//! Records out (S2C): `damage_in` to the victim's owner and `hitfx_in` to every
//! other player after a changed native owner outcome (the approved claim,
//! `WireHdr.peer` = attacker), `damage_verdict`
//! to the attacker, `death_ack`, `death` (via `Inner::out_msgs`), `vitals` (relayed,
//! `WireHdr.peer` = owner, Health clamped in place to the ledger's ceiling).
//!
//! Feedback to the attacker (HSMPCombat cues, combat_quality counters):
//! - early verdict CONFIRM the first time a claim is geometrically accepted
//!   (forwarded, or held for a parry) — the hit-confirm cue arrives <= RTT;
//!   the FINAL verdict follows the owner's ack;
//! - FINAL {ok: false, reason: "code: details"} on reject ("parried" when a
//!   validated clash cancelled it: parry glint);
//! - CLASH {peer: other} to BOTH players of a newly server-validated clash.

use super::*;
use hsmp_ipc::record::view;
use hsmp_ipc::schema::combat::{
    Clash, DamageAck, DamageVerdict, Death, DeathAck, DeathReport, Vitals, K_CLASH, K_DAMAGE, K_DAMAGE_ACK,
    K_DAMAGE_IN, K_DEATH_REPORT, K_HITFX_IN, K_TOUCH, K_VITALS, VERDICT_CLASH, VERDICT_CONFIRM, VERDICT_FINAL,
};
use hsmp_ipc::wire;

fn reported_outcome_cause(reason: u8, mode: u8) -> Option<u8> {
    match reason {
        0 => Some(DEATH_REPORTED),
        1 if mode == hsmp_ipc::schema::session::game_mode::BRAWL => Some(DEATH_DEFEAT),
        2 => Some(DEATH_SURRENDER),
        _ => None,
    }
}

/// An unscoped death (match 0, life 0), as tests build it.
#[cfg(test)]
pub(super) fn death_msg(peer_id: PeerId, round: u32, killer: PeerId, cause: u8) -> Vec<u8> {
    wire::encode(0, peer_id, &Death::new(peer_id, round, killer, cause), &[])
}

/// A server-declared death as a v6 message (`WireHdr.peer` = the victim), scoped to the match and life.
pub(super) fn scoped_death_msg(peer_id:PeerId,round:u32,killer:PeerId,cause:u8,match_id:u64,life:u16)->Vec<u8> {
    let mut d=Death::new(peer_id,round,killer,cause);
    d.match_id=match_id; d.life=life;
    wire::encode(0,peer_id,&d,&[])
}

/// A claim verdict for the attacker.
pub(super) fn verdict_msg(hit_id: u32, kind: u8, ok: bool, reason: &str) -> Vec<u8> {
    wire::encode(0, 0, &DamageVerdict::new(hit_id, 0, kind, ok, reason), &[])
}

/// Send one record message to `addr` (outside every lock).
pub(super) async fn send_rec(socket: &UdpSocket, state: &Arc<ServerState>, addr: SocketAddr, msg: Vec<u8>) {
    let out = state.net.send_msg(addr, msg);
    send_out(socket, state, out).await;
}

/// The combat domain's v6 records from a client (records.rs `handle`). Every payload
/// is validated in place first; a refused one is dropped (and counted).
pub(super) async fn handle_record(
    socket: &Arc<UdpSocket>,
    state: &Arc<ServerState>,
    from: SocketAddr,
    h: wire::WireHdr,
    payload: &[u8],
) -> anyhow::Result<()> {
    match h.kind {
        hsmp_ipc::schema::combat::K_REPLAY_OUTCOME => {
            use hsmp_ipc::schema::combat::{ReplayOutcome,K_REPLAY_OUTCOME,K_REPLAY_OUTCOME_ACK};
            let r=view::<ReplayOutcome>(payload).map_err(super::records::refused)?.head();
            let Some(victim)=touch_peer(state,from).await else {return Ok(())};
            let Some(fresh)=crate::combat::on_replay_outcome(victim,r) else {return Ok(())};
            send_rec(socket,state,from,wire::message(K_REPLAY_OUTCOME_ACK,0,victim,payload)).await;
            if fresh {
                info!(victim,attacker=r.attacker,hit_id=r.hit_id,match_id=r.match_id,round=r.round,life=r.victim_life,
                    status=r.status,fields=r.observed_fields,hp_delta=r.health_delta,"owner native replay outcome");
                let addr={let inner=state.inner.lock().await;inner.peers.iter().find(|(_,p)|p.id==r.attacker).map(|(a,_)|*a)};
                if let Some(addr)=addr {send_rec(socket,state,addr,wire::message(K_REPLAY_OUTCOME,0,victim,payload)).await;}
                if let Some(hit)=crate::combat::native_effect_hit(victim,r.attacker,r.hit_id) {
                    let to={
                        let inner=state.inner.lock().await;
                        if hit.match_id==inner.sess.match_id && hit.round==inner.match_round
                            && hit.attacker_life==modes::peer_life(&inner,r.attacker)
                            && hit.victim_life==modes::peer_life(&inner,victim) {
                            hit_fx_recipients(&inner.peers,victim)
                        } else {Vec::new()}
                    };
                    let msg=hit.msg(K_HITFX_IN,r.attacker);
                    for addr in to {send_rec(socket,state,addr,msg.clone()).await;}
                }
            }
        }
        K_DAMAGE => {
            let mut hit = {
                let v = view::<crate::proto::Damage>(payload).map_err(super::records::refused)?;
                crate::proto::DamageEvent::from_view(&v)
            };
            let (verdict, attacker_id, target_addr) = {
                let inner = state.inner.lock().await;
                let Some(a) = inner.peers.get(&from) else { return Ok(()); };
                let target = inner.peers.iter().find(|(_, p)| p.id == hit.target_peer_id);
                let ctx = crate::combat::Ctx {
                    attacker_id: a.id,
                    attacker_alive: a.alive,
                    attacker_pos: a.last_valid_pos,
                    target_exists: target.is_some(),
                    target_alive: target.map_or(false, |(_, p)| p.alive),
                    target_pos: target.and_then(|(_, p)| p.last_valid_pos),
                    // While a finished round settles, trade hits from the
                    // just-killed player (lagcomp::trade_ok) may still land
                    // on the provisional winner; everyone else is dead.
                    match_live: combat_open(&inner),
                    match_round: inner.match_round,
                    match_id: inner.sess.match_id,
                    attacker_life: modes::peer_life(&inner,a.id),
                    target_life: modes::peer_life(&inner,hit.target_peer_id),
                };
                (crate::combat::on_damage(&ctx, &mut hit), a.id, target.map(|(addr, _)| *addr))
            };
            dispatch_damage(socket, state, Some(from), attacker_id, target_addr, hit, verdict).await;
        }
        K_CLASH | K_TOUCH => {
            let c = view::<Clash>(payload).map_err(super::records::refused)?.head();
            let Some(sid) = touch_peer(state, from).await else { return Ok(()); };
            if h.kind == K_CLASH {
                crate::lagcomp::record_clash(sid, c.other_peer_id, c.my_ts, c.other_ts);
                debug!(peer_id = sid, other = c.other_peer_id, c.my_ts, c.other_ts, "clash recorded");
            } else {
                // The sender is the VICTIM of `other_peer_id`'s blow on its screen.
                crate::lagcomp::record_touch(sid, c.other_peer_id, c.my_ts, c.other_ts);
                debug!(peer_id = sid, other = c.other_peer_id, c.my_ts, c.other_ts, "touch recorded");
            }
        }
        K_DAMAGE_ACK => {
            let a = view::<DamageAck>(payload).map_err(super::records::refused)?.head();
            // Only a connected peer, and only for hits it is the victim of
            // (anyone else's ack would end re-delivery and pin the victim's
            // RTT near 0).
            let Some(victim) = touch_peer(state, from).await else { return Ok(()) };
            crate::combat::ledger_ack(victim, a.attacker, a.hit_id);
            if crate::combat::on_owner_ack(victim, a.attacker, a.hit_id) {
                let attacker_addr = {
                    let inner = state.inner.lock().await;
                    inner.peers.iter().find(|(_, p)| p.id == a.attacker).map(|(x, _)| *x)
                };
                if let Some(x) = attacker_addr {
                    send_rec(socket, state, x, verdict_msg(a.hit_id, VERDICT_FINAL, true, "")).await;
                }
            }
        }
        K_DEATH_REPORT => {
            let d = view::<DeathReport>(payload).map_err(super::records::refused)?.head();
            // A defeat eliminates the fighter, but never invents HP damage or
            // a biological death. Unknown report reasons cannot eliminate anyone.
            // Round-scoped + idempotent: a resend from an earlier round never
            // kills a player in the current one; a resend after the death was
            // already counted is answered by the periodic death re-send.
            {
                let mut inner = state.inner.lock().await;
                let mode = inner.modes.run.map_or(hsmp_ipc::schema::session::game_mode::DUEL, |c| c.mode);
                let cause = reported_outcome_cause(d.reason, mode);
                let cur = inner.match_round;
                if let Some(pid) = inner.peers.get(&from).map(|p| p.id) {
                    if let Some(cause) = cause.filter(|_| d.round == cur && d.match_id == inner.sess.match_id
                        && d.life == modes::peer_life(&inner,pid) && !modes::respawning(&inner,pid)) {
                        let killer = crate::combat::ledger_last_attacker(pid, cur);
                        if declare_death(&mut inner, pid, killer, cause) {
                            info!(peer_id = pid, death_id = d.death_id, round = d.round, reason=d.reason, "native outcome (reliable report)");
                        }
                    } else {
                        debug!(peer_id = pid, death_id = d.death_id, round = d.round, cur, "death ignored (stale round)");
                    }
                }
            }
            send_rec(socket, state, from, wire::encode(0, 0, &DeathAck { death_id: d.death_id, _r: 0 }, &[])).await;
            flush_out(socket, state).await;
        }
        // Vitals: budgeted per sender (Kind::Vitals), fed to the
        // ledger, relayed only when newer than the last relayed seq, through the
        // relay's per-recipient bandwidth budget. The relayed bytes are the
        // owner's record with its Health clamped in place to the ledger ceiling.
        K_VITALS => {
            let mut f: Vitals = view::<Vitals>(payload).map_err(super::records::refused)?.head();
            let pid = {
                let mut inner = state.inner.lock().await;
                let Some(pid) = inner.peers.get(&from).map(|p| p.id) else { return Ok(()); };
                if !super::dispatch::within_budget(state, pid, crate::validate::rate::Kind::Vitals, 1.0) {
                    return Ok(());
                }
                let round = inner.match_round;
                if f.match_id != inner.sess.match_id || f.round != round || f.life != modes::peer_life(&inner,pid)
                    || modes::respawning(&inner,pid) { return Ok(()); }
                if let Some(v) = crate::combat::vitals_in(pid, round, &mut f) {
                    super::dispatch::on_ledger_verdict(&mut inner, pid, round, &v, f.dead());
                }
                if !crate::combat::vitals_relay_fresh(pid, 1, f.seq) { return Ok(()); }
                pid
            };
            let msg = wire::encode(0, pid, &f, &[]);
            relay_record(socket, state, from, crate::relay::Stream::Vitals, msg).await;
            flush_out(socket, state).await;
        }
        k => {
            // An S2C-only combat kind from a client: validated anyway, then dropped.
            let r = hsmp_ipc::schema::check_payload(k, payload);
            debug!(%from, kind = k, valid = r.is_ok(), "combat record a client may not send; dropped");
        }
    }
    Ok(())
}

/// Act on a combat verdict: forward the hit to its owner or answer the
/// attacker. `attacker_addr` None = look it up (tick-driven resolution).
pub(super) async fn dispatch_damage(
    socket: &UdpSocket,
    state: &Arc<ServerState>,
    attacker_addr: Option<SocketAddr>,
    attacker_id: PeerId,
    target_addr: Option<SocketAddr>,
    hit: crate::proto::DamageEvent,
    verdict: crate::combat::Verdict,
) {
    let mut verdict = verdict;
    if verdict == crate::combat::Verdict::Forward {
        // First forward (immediately, or after a parry hold): the round may
        // have ended / the target died meanwhile. Otherwise book the hit in
        // the authoritative ledger; a lethal one kills NOW, server-side.
        let mut inner = state.inner.lock().await;
        let round = inner.match_round;
        let target_alive = inner.peers.values().any(|p| p.id == hit.target_peer_id && p.alive);
        // The mode's verdict: no friendly fire between teammates (unless the option
        // allows it), no hits on / from a player inside its respawn protection.
        let refused = modes::hit_refusal(&inner, attacker_id, hit.target_peer_id);
        if !combat_open(&inner) || hit.round != round || !target_alive
            || hit.match_id != inner.sess.match_id || hit.attacker_life != modes::peer_life(&inner,attacker_id)
            || hit.victim_life != modes::peer_life(&inner,hit.target_peer_id) {
            crate::combat::reject_decision(attacker_id, hit.hit_id, "round over / target down");
            verdict = crate::combat::Verdict::Ack { accepted: false, reason: "round over / target down".into() };
        } else if let Some(why) = refused {
            debug!(attacker_id, target = hit.target_peer_id, hit_id = hit.hit_id, why, "hit refused by the game mode");
            crate::combat::reject_decision(attacker_id, hit.hit_id, why);
            verdict = crate::combat::Verdict::Ack { accepted: false, reason: why.into() };
        } else {
            let v = crate::combat::ledger_forward(attacker_id, round, &hit);
            info!(attacker_id, target = hit.target_peer_id, hit_id = hit.hit_id,
                  bone = hit.bone_str(), dmg = hit.damage_out, raw = hit.raw_damage,
                  hp_loss = crate::combat::hit_loss(&hit), est_hp = v.est,
                  age_ms = hit.age_ms, "damage accepted");
            if v.lethal && declare_death(&mut inner, hit.target_peer_id, attacker_id, DEATH_DAMAGE) {
                info!(victim = hit.target_peer_id, killer = attacker_id, round, est_hp = v.est,
                      "KILL (server ledger): lethal hit accepted");
            }
        }
    }
    if verdict == crate::combat::Verdict::Reforward {
        let inner=state.inner.lock().await;
        if hit.match_id!=inner.sess.match_id || hit.round!=inner.match_round
            || hit.attacker_life!=modes::peer_life(&inner,attacker_id)
            || hit.victim_life!=modes::peer_life(&inner,hit.target_peer_id) {
            crate::combat::reject_decision(attacker_id,hit.hit_id,"stale_life");
            verdict=crate::combat::Verdict::Ack {accepted:false,reason:"stale_life".into()};
        }
    }
    // Cosmetic effects wait for K_REPLAY_OUTCOME. Forward/transport ACK proves
    // delivery only; a stale or refused native replay must never paint a wound.
    // Early confirm (first acceptance, forwarded or held).
    if crate::combat::take_confirm(attacker_id, hit.hit_id) {
        let addr = match attacker_addr {
            Some(a) => Some(a),
            None => {
                let inner = state.inner.lock().await;
                inner.peers.iter().find(|(_, p)| p.id == attacker_id).map(|(a, _)| *a)
            }
        };
        if let Some(a) = addr {
            send_rec(socket, state, a, verdict_msg(hit.hit_id, VERDICT_CONFIRM, true, crate::combat::CONFIRM_REASON)).await;
        }
    }
    match verdict {
        crate::combat::Verdict::Hold | crate::combat::Verdict::Ignore => {}
        crate::combat::Verdict::Forward | crate::combat::Verdict::Reforward => {
            if let Some(t) = target_addr {
                send_rec(socket, state, t, hit.msg(K_DAMAGE_IN, attacker_id)).await;
            }
        }
        crate::combat::Verdict::Ack { accepted, reason } => {
            crate::stats::combat(accepted, &reason);
            if !accepted && crate::validate::rate::log_ok("damage_rejected") {
                warn!(attacker_id, target = hit.target_peer_id, hit_id = hit.hit_id,
                      %reason, "damage rejected");
            }
            if let Some(a) = attacker_addr {
                send_rec(socket, state, a, verdict_msg(hit.hit_id, VERDICT_FINAL, accepted, &reason)).await;
            }
        }
    }
    flush_out(socket, state).await;
}

/// Players that get `hitfx_in` for a hit on `victim`: everyone else who is
/// connected and negotiated caps::HIT_FX (the attacker included: its own
/// stand-in of the victim then shows the server-approved wound).
pub(super) fn hit_fx_recipients(peers: &HashMap<SocketAddr, PeerState>, victim: PeerId) -> Vec<SocketAddr> {
    hit_fx_pick(peers.iter().map(|(a, p)| (*a, p.id)), victim, crate::interact::peer_caps)
}

/// `hit_fx_recipients` over (address, peer id) pairs and a caps lookup.
pub(super) fn hit_fx_pick(peers: impl Iterator<Item = (SocketAddr, PeerId)>, victim: PeerId,
                          caps: impl Fn(PeerId) -> u64) -> Vec<SocketAddr> {
    let mut out: Vec<SocketAddr> = peers
        .filter(|&(_, id)| id != victim && caps(id) & hsmp_net::net::caps::HIT_FX != 0)
        .map(|(a, _)| a)
        .collect();
    out.sort();
    out
}

#[cfg(test)]
mod hit_fx_tests {
    use super::*;
    #[test]
    fn native_defeat_is_distinct_from_death_and_unknown_reasons_are_rejected() {
        let brawl = hsmp_ipc::schema::session::game_mode::BRAWL;
        assert_eq!(reported_outcome_cause(0, brawl), Some(DEATH_REPORTED));
        assert_eq!(reported_outcome_cause(1, brawl), Some(DEATH_DEFEAT));
        for mode in 0..=6 {
            assert_eq!(reported_outcome_cause(0, mode), Some(DEATH_REPORTED));
            assert_eq!(reported_outcome_cause(2, mode), Some(DEATH_SURRENDER));
            if mode != brawl { assert_eq!(reported_outcome_cause(1, mode), None); }
        }
        for reason in 3..=u8::MAX { assert_eq!(reported_outcome_cause(reason, brawl), None); }
        let report = DeathReport { death_id: 7, match_id: 81, round: 3, life: 130,
            reason: 1, ..Default::default() };
        let bytes = wire::encode(0, 9, &report, &[]);
        let (_, decoded) = wire::decode::<DeathReport>(&bytes).unwrap();
        assert_eq!((decoded.head.reason, decoded.head.match_id, decoded.head.round, decoded.head.life),
            (1, 81, 3, 130));
        assert_eq!(std::mem::size_of::<DeathReport>(), 24);
    }
    #[test]
    fn hit_fx_goes_to_every_other_capable_player() {
        let a = |n: u16| -> SocketAddr { format!("127.0.0.1:{n}").parse().unwrap() };
        let peers = vec![(a(1), 3), (a(2), 4), (a(3), 5), (a(4), 6)];
        let caps = |id: PeerId| if id == 6 { 0 } else { hsmp_net::net::caps::HIT_FX };
        // victim 4: attacker 3 and spectator 5 get it; 6 never negotiated it.
        assert_eq!(hit_fx_pick(peers.clone().into_iter(), 4, caps), vec![a(1), a(3)]);
        assert!(hit_fx_pick(peers.into_iter().filter(|p| p.1 == 4), 4, caps).is_empty(), "never to the victim");
    }

    #[test]
    fn server_built_records_are_valid() {
        let dm = death_msg(2, 3, 1, 1);
        let (h, v) = wire::decode::<Death>(&dm).unwrap();
        assert_eq!((h.peer, v.head.peer_id, v.head.round, v.head.killer, v.head.cause), (2, 2, 3, 1, 1));
        let vm = verdict_msg(9, VERDICT_FINAL, false, "parried");
        let (_, v) = wire::decode::<DamageVerdict>(&vm).unwrap();
        assert_eq!((v.head.hit_id, v.head.ok.get(), v.head.code), (9, false, 3));
        let mut d = crate::proto::DamageEvent::default();
        d.target_peer_id = 4;
        d.set_bone("head");
        d.set_delta_pairs(&[(0, -5.0)]);
        let m = d.msg(K_DAMAGE_IN, 7);
        let (h, p) = wire::split(&m).unwrap();
        assert_eq!((h.kind, h.peer), (K_DAMAGE_IN, 7));
        let v = view::<crate::proto::Damage>(p).unwrap();
        assert_eq!(crate::proto::DamageEvent::from_view(&v), d);
    }
}

/// Lag-compensated hits whose defender grace ran out (or whose stream
/// coverage arrived) since the last tick, and newly validated clashes.
pub(super) async fn flush_held_hits(socket: &UdpSocket, state: &Arc<ServerState>) {
    notify_clashes(socket, state).await;
    sweep_ledger(socket, state).await;
    let due = crate::combat::flush_pending();
    if due.is_empty() { return; }
    for (attacker_id, hit, verdict) in due {
        let (a_addr, t_addr) = {
            let inner = state.inner.lock().await;
            let find = |id: PeerId| inner.peers.iter().find(|(_, p)| p.id == id).map(|(a, _)| *a);
            (find(attacker_id), find(hit.target_peer_id))
        };
        dispatch_damage(socket, state, a_addr, attacker_id, t_addr, hit, verdict).await;
    }
}

/// God-mode rule on the tick: a player whose server-validated
/// damage is lethal and who has not reported its death within the grace is
/// declared dead (DEATH_DAMAGE), without waiting for its next vitals.
async fn sweep_ledger(socket: &UdpSocket, state: &Arc<ServerState>) {
    let any = {
        let mut inner = state.inner.lock().await;
        if !combat_open(&inner) { return; }
        let round = inner.match_round;
        let due = crate::combat::ledger_sweep(round);
        for (pid, v) in &due {
            super::dispatch::on_ledger_verdict(&mut inner, *pid, round, v, false);
        }
        !due.is_empty()
    };
    if any { flush_out(socket, state).await; }
}

/// Confirm each newly server-validated clash to both players (their
/// HSMPCombat plays the clash glint): a CLASH verdict with `peer` = the other.
async fn notify_clashes(socket: &UdpSocket, state: &Arc<ServerState>) {
    let pairs = crate::lagcomp::judge_due();
    if pairs.is_empty() { return; }
    for (a, b) in pairs {
        let (aa, ba) = {
            let inner = state.inner.lock().await;
            let find = |id: PeerId| inner.peers.iter().find(|(_, p)| p.id == id).map(|(x, _)| *x);
            (find(a), find(b))
        };
        for (to, other) in [(aa, b), (ba, a)] {
            if let Some(addr) = to {
                let mut v = DamageVerdict::new(0, 0, VERDICT_CLASH, true, crate::combat::CLASH_REASON);
                v.peer = other;
                send_rec(socket, state, addr, wire::encode(0, 0, &v, &[])).await;
            }
        }
    }
    flush_out(socket, state).await;
}

/// The combat records through the real server state and transport: a claim reaches the
/// victim as `damage_in` (peer = attacker) and the attacker gets CONFIRM then, on the
/// victim's ack, FINAL ok; vitals are relayed with peer = owner; a death report is acked
/// and the death broadcast to everyone.
#[cfg(test)]
mod record_flow_tests {
    use super::*;
    use super::super::dispatch::resume_tests::live_duel;
    use hsmp_ipc::schema::combat::{Damage, K_DEATH};

    fn recs(t: &super::super::dispatch::resume_tests::TClient, kind: u16) -> Vec<&Vec<u8>> {
        t.records.iter().filter(|m| wire::kind_of(m) == kind).collect()
    }

    #[tokio::test]
    async fn duel_rejects_automatic_ko_but_accepts_scoped_manual_surrender() {
        let (socket, state, _ca, cb) = live_duel(76_201).await;
        let bid = cb.welcome_id().unwrap();
        let report = {
            let mut inner = state.inner.lock().await;
            inner.sess.match_id = 82;
            inner.modes.run = Some(super::super::modes::ModeCfg {
                mode: hsmp_ipc::schema::session::game_mode::DUEL, ..Default::default() });
            DeathReport { death_id: 19, match_id: 82, round: 1,
                life: super::super::modes::peer_life(&inner, bid), reason: 1, ..Default::default() }
        };
        let ko = wire::encode(0, 0, &report, &[]);
        let (h, p) = wire::split(&ko).unwrap();
        handle_record(&socket, &state, cb.addr, h, p).await.unwrap();
        assert!(state.inner.lock().await.peers.values().find(|p| p.id == bid).unwrap().alive);
        let surrendered = wire::encode(0, 0, &DeathReport { reason: 2, ..report }, &[]);
        let (h, p) = wire::split(&surrendered).unwrap();
        handle_record(&socket, &state, cb.addr, h, p).await.unwrap();
        let inner = state.inner.lock().await;
        assert!(!inner.peers.values().find(|p| p.id == bid).unwrap().alive);
        assert_eq!(inner.round_deaths.len(), 1);
        assert_eq!(inner.round_deaths[0].2, DEATH_SURRENDER);
    }

    #[tokio::test]
    async fn scoped_native_defeat_eliminates_once_without_claiming_hp_damage() {
        let (socket, state, _ca, cb) = live_duel(76_101).await;
        let bid = cb.welcome_id().unwrap();
        let report = {
            let mut inner = state.inner.lock().await;
            inner.sess.match_id = 81;
            inner.modes.run = Some(super::super::modes::ModeCfg {
                mode: hsmp_ipc::schema::session::game_mode::BRAWL, ..Default::default() });
            DeathReport { death_id: 17, match_id: 81, round: 1,
                life: super::super::modes::peer_life(&inner, bid), reason: 1, ..Default::default() }
        };
        // Unknown reasons and delayed reports from a different pawn life
        // cannot end the currently living fighter's bout.
        for invalid in [DeathReport { reason: 3, ..report },
            DeathReport { life: report.life + 1, ..report },
            DeathReport { match_id: 80, ..report }] {
            let bytes = wire::encode(0, 0, &invalid, &[]);
            let (h, p) = wire::split(&bytes).unwrap();
            handle_record(&socket, &state, cb.addr, h, p).await.unwrap();
            assert!(state.inner.lock().await.peers.values().find(|p| p.id == bid).unwrap().alive);
        }
        let bytes = wire::encode(0, 0, &report, &[]);
        let (h, p) = wire::split(&bytes).unwrap();
        handle_record(&socket, &state, cb.addr, h, p).await.unwrap();
        handle_record(&socket, &state, cb.addr, h, p).await.unwrap();
        let inner = state.inner.lock().await;
        assert!(!inner.peers.values().find(|p| p.id == bid).unwrap().alive);
        assert_eq!(inner.round_deaths.len(), 1, "reliable retry cannot count a second defeat");
        assert_eq!(inner.round_deaths[0].2, DEATH_DEFEAT);
        assert_eq!(inner.match_state, "roundover");
    }

    #[tokio::test]
    async fn claim_ack_vitals_and_death_records_flow() {
        let (socket, state, mut ca, mut cb) = live_duel(75_001).await;
        let (aid, bid) = (ca.welcome_id().unwrap(), cb.welcome_id().unwrap());
        let claim = Damage { hit_id: 11, cid: 5, target_peer_id: bid, round: 1, bone: hsmp_ipc::layout::Str::new("spine_02"),
                             damage_out: 12.0, raw_damage: 40.0, normal: [1.0, 0.0, 0.0], location: [10.0, 0.0, 0.0], ..Default::default() };
        let m = wire::encode(0, 0, &claim, &[]);
        let (h, p) = wire::split(&m).unwrap();
        super::super::records::handle(&socket, &state, ca.addr, h, p).await.unwrap();
        for _ in 0..4 { ca.pump(&socket, &state).await; cb.pump(&socket, &state).await; }
        let got = recs(&cb, K_DAMAGE_IN);
        assert_eq!(got.len(), 1, "the victim got the hit");
        let mut x = got[0].clone();
        x[..2].copy_from_slice(&K_DAMAGE.to_le_bytes());
        let (hh, v) = wire::decode::<Damage>(&x).unwrap();
        assert_eq!((hh.peer, v.head.hit_id, v.head.target_peer_id, v.head.bone.as_str()), (aid, 11, bid, Some("spine_02")));
        let confirm = recs(&ca, hsmp_ipc::schema::combat::K_DAMAGE_VERDICT);
        let (_, cv) = wire::decode::<DamageVerdict>(confirm[0]).unwrap();
        assert_eq!((cv.head.hit_id, cv.head.kind), (11, VERDICT_CONFIRM));
        // The victim's sidecar acks: the attacker gets the FINAL verdict.
        let ack = wire::encode(0, 0, &DamageAck { attacker: aid, hit_id: 11 }, &[]);
        let (h, p) = wire::split(&ack).unwrap();
        super::super::records::handle(&socket, &state, cb.addr, h, p).await.unwrap();
        for _ in 0..4 { ca.pump(&socket, &state).await; }
        let finals: Vec<DamageVerdict> = recs(&ca, hsmp_ipc::schema::combat::K_DAMAGE_VERDICT).iter()
            .map(|m| wire::decode::<DamageVerdict>(m).unwrap().1.head()).filter(|v| v.kind == VERDICT_FINAL).collect();
        assert_eq!(finals.len(), 1);
        assert!(finals[0].ok.get() && finals[0].hit_id == 11);

        // Vitals from A are relayed to B with peer = A, as they came.
        let mut f = crate::proto::vitals::unknown();
        f.seq = 7;f.round=1;
        {let inner=state.inner.lock().await;f.match_id=inner.sess.match_id;f.life=super::super::modes::peer_life(&inner,aid);}
        crate::proto::vitals::set(&mut f, 0, 88.0);
        let vm = wire::encode(0, 0, &f, &[]);
        let (h, p) = wire::split(&vm).unwrap();
        super::super::records::handle(&socket, &state, ca.addr, h, p).await.unwrap();
        for _ in 0..4 { cb.pump(&socket, &state).await; }
        let got = recs(&cb, K_VITALS);
        assert_eq!(got.len(), 1);
        let (hh, v) = wire::decode::<Vitals>(got[0]).unwrap();
        assert_eq!((hh.peer, v.head.seq, v.head.health()), (aid, 7, Some(88.0)));
        assert_eq!(&got[0][wire::HDR..], p, "relayed byte for byte (no clamp needed)");

        // B reports its own death: acked; the death goes to everyone.
        let dm = wire::encode(0, 0, &DeathReport { death_id: 4, round: 1, ..Default::default() }, &[]);
        let (h, p) = wire::split(&dm).unwrap();
        super::super::records::handle(&socket, &state, cb.addr, h, p).await.unwrap();
        for _ in 0..4 { ca.pump(&socket, &state).await; cb.pump(&socket, &state).await; }
        let (_, a) = wire::decode::<DeathAck>(recs(&cb, hsmp_ipc::schema::combat::K_DEATH_ACK)[0]).unwrap();
        assert_eq!(a.head.death_id, 4);
        for t in [&ca, &cb] {
            let (_, d) = wire::decode::<Death>(recs(t, K_DEATH)[0]).unwrap();
            assert_eq!((d.head.peer_id, d.head.round, d.head.cause), (bid, 1, DEATH_REPORTED));
        }
        // A hostile record is refused, not acted on.
        assert!(super::super::records::handle(&socket, &state, ca.addr, h, &p[..3]).await.is_err());
    }
}
