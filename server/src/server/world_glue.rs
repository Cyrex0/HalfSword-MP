//! World-object replication (HSMPWorld v2): async glue around world.rs.
//!
//! Protocol v6: the world kinds (`0x04xx`, `hsmp_ipc::schema::world`) arrive as typed records
//! the client's game wrote; [`handle_record`] validates them in place and hands the borrowed
//! rows to world.rs; what comes out is framed by `world::Msg::encode`.

use super::*;
use hsmp_ipc::record::view;
use hsmp_ipc::schema::world as rec;

// ---------------------------------------------------------------------------
// World-object replication (HSMPWorld v2) — async glue around world.rs.
//
// world.rs holds all rules (manifest, leases, epochs, relevance, budget) as
// pure, unit-tested logic; this section only maps peers <-> addresses, feeds
// messages in and sends what comes out. A dedicated 30 Hz task (started on
// the first world message) flushes coalesced states, expires leases, heals
// owner tables, keyframes rest poses, and bumps the world epoch whenever the
// match enters a countdown or returns to the lobby (every client reloads its
// arena then, so every world object is back at its initial state).
// ---------------------------------------------------------------------------

const WORLD_TICK: Duration = Duration::from_millis(33);

fn world() -> &'static std::sync::Mutex<crate::world::World> {
    static W: std::sync::OnceLock<std::sync::Mutex<crate::world::World>> = std::sync::OnceLock::new();
    W.get_or_init(|| std::sync::Mutex::new(crate::world::World::new()))
}

/// Sender's peer id + last valid root position.
async fn world_sender(state: &Arc<ServerState>, from: SocketAddr) -> Option<(PeerId, Option<[f32; 3]>)> {
    let inner = state.inner.lock().await;
    inner.peers.get(&from).map(|p| (p.id, p.last_valid_pos))
}

/// Send world output: each `Msg`'s record messages go out together (a state packet's
/// per-sender messages share a datagram; two packets for one peer never share a transmit,
/// so a sender's second chunk never supersedes its first in the Latest queue).
async fn world_send(socket: &UdpSocket, state: &Arc<ServerState>, out: Vec<crate::world::Out>) {
    if out.is_empty() { return; }
    let addr_of: HashMap<PeerId, SocketAddr> = {
        let inner = state.inner.lock().await;
        inner.peers.iter().map(|(a, p)| (p.id, *a)).collect()
    };
    for (pid, msg) in out {
        if let Some(a) = addr_of.get(&pid) {
            let dg = state.net.send_msgs(*a, msg.encode());
            send_out(socket, state, dg).await;
        }
    }
}

fn world_ensure_task(socket: &Arc<UdpSocket>, state: &Arc<ServerState>) {
    static STARTED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
    if STARTED.swap(true, std::sync::atomic::Ordering::SeqCst) { return; }
    let (socket, state) = (socket.clone(), state.clone());
    tokio::spawn(async move {
        let mut ticker = time::interval(WORLD_TICK);
        ticker.set_missed_tick_behavior(time::MissedTickBehavior::Skip);
        let mut last_phase: Option<(String, u32)> = None;
        let mut last_log = std::time::Instant::now();
        loop {
            ticker.tick().await;
            let (present, positions, phase) = {
                let inner = state.inner.lock().await;
                let present: HashSet<PeerId> = inner.peers.values().map(|p| p.id).collect();
                let positions: HashMap<PeerId, [f32; 3]> = inner.peers.values()
                    .filter_map(|p| p.last_valid_pos.map(|v| (p.id, v))).collect();
                (present, positions, (inner.match_state.clone(), inner.match_round))
            };
            let now = std::time::Instant::now();
            let mut out = Vec::new();
            {
                let mut w = world().lock().unwrap();
                // Fresh arenas everywhere: countdown entry or back to lobby.
                let reload = match &last_phase {
                    None => false,
                    Some((s, r)) => (phase.0 == "countdown" && (s != "countdown" || *r != phase.1))
                        || (phase.0 == "lobby" && s != "lobby"),
                };
                if reload {
                    out.extend(w.bump_epoch());
                    info!(epoch = w.epoch, state = %phase.0, round = phase.1 + 1, "world epoch bumped (arenas reload; objects reset)");
                }
                last_phase = Some(phase);
                out.extend(w.tick(now, &present, &positions));
                for v in std::mem::take(&mut w.verdicts) { world_log_verdict(&v); }
                if now.duration_since(last_log) >= Duration::from_secs(10) {
                    last_log = now;
                    let s = std::mem::take(&mut w.stats);
                    if s.init_accepted + s.init_refused + s.state_changes + s.verdicts > 0 {
                        info!(epoch = w.epoch, init_accepted = s.init_accepted, init_refused = s.init_refused,
                              state_changes = s.state_changes, verdicts = s.verdicts, mismatching = s.verdicts_mismatch,
                              "world 10s (identical worlds)");
                    }
                    if s.states_in > 0 || s.claims > 0 {
                        let leases: usize = w.levels.values().map(|l| l.leases.len()).sum();
                        let objs: usize = w.levels.values().map(|l| l.manifest.len()).sum();
                        info!(epoch = w.epoch, peers = w.peer_count(), manifest = objs, leases,
                              states_in = s.states_in, rejected = s.states_rejected, objs_out = s.objs_out,
                              thinned = s.objs_thinned, budget_dropped = s.objs_budget_dropped,
                              claims = s.claims, granted = s.claims_granted, expired = s.leases_expired,
                              "world 10s");
                    }
                }
            }
            world_send(&socket, &state, out).await;
        }
    });
}

/// Identical-worlds consistency verdict -> log line + `world_consistency` event
/// (the gate's evidence that both screens show the same world; see
/// docs/development/subsystems/world-replication.md).
fn world_log_verdict(v: &crate::world::Verdict) {
    let ids: Vec<u32> = v.mismatched.iter().map(|m| m.id).collect();
    if v.hash_match() {
        debug!(level = v.level, epoch = v.epoch, peer_id = v.peer, other = v.other, compared = v.compared,
               hash_equal = v.hash_equal, "world consistency ok");
    } else {
        let detail: Vec<String> = v.mismatched.iter().take(12)
            .map(|m| format!("{}:{}:{:.0}cm/{:.0}deg", m.id, ["?", "pose", "presence", "state"][m.kind.min(3) as usize], m.dpos, m.dang))
            .collect();
        warn!(level = v.level, epoch = v.epoch, peer_id = v.peer, other = v.other, compared = v.compared,
              mismatched = ids.len(), ids = ?detail, "world consistency MISMATCH (worlds differ between these peers)");
    }
    if !v.codec_ok {
        debug!(peer_id = v.peer, "world consistency: client hash differs from the server's recomputation");
    }
    crate::events::emit("world_consistency", serde_json::json!({
        "level": v.level, "epoch": v.epoch, "peer": v.peer, "other": v.other, "seq": v.seq,
        "compared": v.compared, "hash_match": v.hash_match(), "hash_equal": v.hash_equal,
        "mismatched": ids,
        "kinds": v.mismatched.iter().map(|m| m.kind).collect::<Vec<u8>>(),
    }));
}

/// One world record from a client (`0x04xx`). Validated in place (`record::view`: sizes,
/// finite floats, id / flag masks, mode ranges, world bounds); the rows are handed to
/// world.rs borrowed. A kind a client may not send is refused.
pub(super) async fn handle_record(
    socket: &Arc<UdpSocket>,
    state: &Arc<ServerState>,
    from: SocketAddr,
    kind: u16,
    payload: &[u8],
) -> anyhow::Result<()> {
    world_ensure_task(socket, state);
    let now = std::time::Instant::now();
    match kind {
        rec::K_WORLD_STATE => {
            let v = view::<rec::WorldStateHead>(payload).map_err(super::records::refused)?;
            let Some((peer, pos)) = world_sender(state, from).await else { return Ok(()) };
            let h = v.head();
            let out = {
                let mut w = world().lock().unwrap();
                if h.epoch != w.epoch {
                    w.stale_hint(peer, h.level, now).into_iter().collect()
                } else {
                    let (_, changed) = w.state(peer, h.level, h.epoch, h.seq, h.ts, &v.rows, pos, now);
                    if changed.is_empty() { Vec::new() } else { w.owners_to_level(h.level, &changed) }
                }
            };
            world_send(socket, state, out).await;
        }
        rec::K_WORLD_CLAIM => {
            let v = view::<rec::WorldClaim>(payload).map_err(super::records::refused)?;
            let c = v.head();
            let Some((peer, _)) = world_sender(state, from).await else { return Ok(()) };
            let rest = c.has_rest.get().then_some(c.rest);
            let (level, epoch, req, id, mode) = (c.level, c.epoch, c.req, c.id, c.mode);
            let out = {
                let mut w = world().lock().unwrap();
                match w.claim(peer, level, epoch, id, mode, rest, now) {
                    crate::world::ClaimResult::Done { rec, changed } => {
                        debug!(peer_id = peer, req, id, mode, owner = rec.owner, ver = rec.ver, changed, "world claim");
                        if changed {
                            w.owners_to_level(level, &[rec])
                        } else {
                            let e = w.epoch;
                            let mlen = w.levels.get(&level).map_or(0, |l| l.manifest.len() as u32);
                            vec![(peer, crate::world::Msg::Owners { level, epoch: e, sync: false, manifest_len: mlen, owners: vec![rec] })]
                        }
                    }
                    crate::world::ClaimResult::Stale => w.stale_hint(peer, level, now).into_iter().collect(),
                    // Identical-worlds initial anchors are fanned out by the world tick.
                    crate::world::ClaimResult::Anchored | crate::world::ClaimResult::Ignored => Vec::new(),
                    crate::world::ClaimResult::Unknown | crate::world::ClaimResult::RateLimited => {
                        debug!(peer_id = peer, req, id, "world claim ignored (unknown id or rate limit)");
                        Vec::new()
                    }
                }
            };
            world_send(socket, state, out).await;
        }
        rec::K_WORLD_SYNC => {
            let v = view::<rec::WorldSync>(payload).map_err(super::records::refused)?;
            let level = v.head.level;
            let Some((peer, _)) = world_sender(state, from).await else { return Ok(()) };
            let out = {
                let mut w = world().lock().unwrap();
                let out = w.sync(peer, level, now);
                info!(peer_id = peer, level, epoch = w.epoch, messages = out.len(), "world sync served");
                out
            };
            world_send(socket, state, out).await;
        }
        rec::K_WORLD_MANIFEST | rec::K_WORLD_DYN => {
            let (level, epoch, req, entries) = if kind == rec::K_WORLD_MANIFEST {
                let v = view::<rec::ManifestHead>(payload).map_err(super::records::refused)?;
                (v.head.level, v.head.epoch, v.head.req, v.rows.iter().map(crate::world::WorldEntry::from_static).collect::<Vec<_>>())
            } else {
                let v = view::<rec::DynHead>(payload).map_err(super::records::refused)?;
                (v.head.level, v.head.epoch, v.head.req, v.rows.iter().map(crate::world::WorldEntry::from_dyn).collect::<Vec<_>>())
            };
            let Some((peer, _)) = world_sender(state, from).await else { return Ok(()) };
            let out = {
                let mut w = world().lock().unwrap();
                let n = entries.len();
                let out = w.manifest(peer, level, epoch, req, entries, now);
                let total = w.levels.get(&level).map_or(0, |l| l.manifest.len());
                debug!(peer_id = peer, level, req, submitted = n, total, "world manifest");
                out
            };
            world_send(socket, state, out).await;
        }
        rec::K_WORLD_HASH => {
            let v = view::<rec::HashHead>(payload).map_err(super::records::refused)?;
            let h = v.head();
            let Some((peer, _)) = world_sender(state, from).await else { return Ok(()) };
            let out = {
                let mut w = world().lock().unwrap();
                w.hash_report(peer, h.level, h.epoch, h.seq, h.hash, &v.rows, now)
            };
            world_send(socket, state, out).await;
        }
        k => {
            // Server -> client kinds (owners, snapshot, verdict) and game-local ones: never
            // accepted from a client (validated anyway, so a fuzzer only reaches the validator).
            let _ = hsmp_ipc::schema::check_payload(k, payload);
            return Err(super::records::refused(hsmp_ipc::record::Invalid::Kind(k)));
        }
    }
    Ok(())
}
