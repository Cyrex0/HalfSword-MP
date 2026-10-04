//! S2C handlers: act on one server message, a v6 record, by handing it to its domain
//! (session_client.rs for session, match and connection; combat, world, loadout, pose and
//! interact have their own modules). The transport (`net.rs`) decrypts, orders and splits.
//!
//! See docs/development/server-modules.md.

use super::*;
use hsmp_ipc::schema::session as rs;

/// A Welcome or a new server epoch: every per-peer dedup / high-water mark
/// of the old server instance is void (peer ids, kit revs, loadout versions
/// and vitals seqs restart at 1 after a server restart).
/// Tests that drive welcome / session records through the process-wide state set this, so
/// their resets never clear the combat / loadout dedup other tests run on in parallel.
#[cfg(test)]
pub(super) static TEST_NO_DEDUP_RESET: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

pub(super) fn reset_session_dedup() {
    #[cfg(test)]
    if TEST_NO_DEDUP_RESET.load(std::sync::atomic::Ordering::Relaxed) {
        return;
    }
    loadout_client::reset_session();
    combat_client::reset_session();
}

/// One v6 record message from the server (protocol v6): validated in place
/// and handed to its domain, which copies it into shared memory as it is (a slot, a blob or
/// an S2G ring record carrying the header's kind / aux / peer). Domains add their kinds here.
pub(super) async fn handle_server_record(
    h: hsmp_ipc::wire::WireHdr,
    payload: &[u8],
    shared: &Arc<Mutex<SharedState>>,
) -> Result<()> {
    use hsmp_ipc::schema::pose::{PoseHead, Root, K_POSE, K_ROOT};
    match h.kind {
        // ---- session, match, connection (session_client.rs) ----
        rs::K_WELCOME => session_client::on_welcome(payload, shared).await?,
        rs::K_SESSION => session_client::on_session(payload)?,
        rs::K_PINGS => session_client::on_pings(payload)?,
        rs::K_CMD_RESULT => session_client::on_cmd_result(payload)?,
        rs::K_NOTICE | rs::K_KILL_FEED => session_client::on_event(h.kind, payload)?,
        rs::K_KICKED => session_client::on_kicked(payload, shared).await?,
        rs::K_SERVER_CLOSING => session_client::on_server_closing(payload, shared).await?,
        rs::K_CHAT_IN => session_client::on_chat_in(payload)?,
        rs::K_ADMIN_STATE => session_client::on_admin_state(payload, shared).await?,
        rs::K_PONG => session_client::on_pong(payload)?,
        // Grabs and shoves (interact_client.rs): into the S2G ring as `interact`.
        k if hsmp_ipc::schema::interact::is_interact_kind(k) => interact_client::on_s2c(h, payload),
        // ---- combat (combat_client.rs): copied into the S2G ring / peer slot as they are ----
        k if k >> 8 == 0x03 => combat_client::on_server_record(h, payload),
        // World replication (0x04xx): validated and merged into the world tables (world_client.rs).
        0x0400..=0x04FF => {
            if let Err(e) = world_client::on_record(h.kind, h.peer, payload) {
                debug!(kind = h.kind, error = %e, "v6 world record refused");
            }
        }
        // loadout domain (loadout_client.rs)
        hsmp_ipc::schema::loadout::K_KIT_VERDICT => loadout_client::on_kit_verdict(h.peer, payload),
        hsmp_ipc::schema::loadout::K_KIT_RULES => loadout_client::on_kit_rules(payload),
        hsmp_ipc::schema::loadout::K_LOADOUT => loadout_client::on_loadout(h.peer, payload),
        hsmp_ipc::schema::loadout::K_BODY => loadout_client::on_body(h.peer, payload),
        // ---- pose (protocol v6; schema/pose.rs) ----
        K_ROOT => {
            let v = match hsmp_ipc::record::view::<Root>(payload) {
                Ok(v) => v,
                Err(e) => { debug!(error = %e, "relayed root refused"); return Ok(()); }
            };
            let peer_id = h.peer;
            if peer_id == 0 { return Ok(()); }
            on_root(peer_id, &v.head, payload);
        }
        K_POSE => {
            let v = match hsmp_ipc::record::view::<PoseHead>(payload) {
                Ok(v) => v,
                Err(e) => { debug!(error = %e, "relayed pose refused"); return Ok(()); }
            };
            if h.peer != 0 { on_pose(h.peer, v.head.tick, &v.rows, h.aux); }
        }
        k => {
            let r = hsmp_ipc::schema::check_payload(k, payload);
            debug!(kind = k, valid = r.is_ok(), len = payload.len(), "v6 record of a kind this sidecar does not handle; dropped");
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use hsmp_ipc::record::to_payload;
    use hsmp_ipc::wire;

    /// The welcome / session tests share process-wide session state: one at a time.
    fn serial() -> std::sync::MutexGuard<'static, ()> {
        crate::ipc_shm::ShmLink::test_lock()
    }

    fn shared(id: u32) -> Arc<Mutex<SharedState>> {
        TEST_NO_DEDUP_RESET.store(true, std::sync::atomic::Ordering::Relaxed);
        Arc::new(Mutex::new(SharedState { my_peer_id: id, status: "connecting", is_admin: false }))
    }

    /// A resume Welcome (same server, same id) keeps the other players' pose
    /// buffers; it is handled as a typed record.
    #[tokio::test]
    async fn resume_welcome_keeps_stand_ins() {
        let _serial = serial();
        let sh = shared(0);
        let welcome = |id: u32| to_payload(&rs::Welcome { peer_id: id, seat: 1, server_epoch: 0x5EED_0042, nick: hsmp_ipc::layout::Str::new("me"), ..Default::default() }, &[]);
        const OTHER: u32 = 991_005;
        // First Welcome of this epoch (a fresh session), then the resume.
        session_client::on_welcome(&welcome(3), &sh).await.unwrap();
        with_play(|m| { m.insert(OTHER, PeerPlay::new()); });
        let msg = wire::message(rs::K_WELCOME, 0, 0, &welcome(3));
        let (h, p) = wire::split(&msg).unwrap();
        handle_server_record(h, p, &sh).await.unwrap();
        assert!(with_play(|m| m.contains_key(&OTHER)), "resume keeps the stand-in's buffer");
        assert_eq!(sh.lock().await.my_peer_id, 3);
        assert_eq!(sh.lock().await.status, "connected");
        with_play(|m| { m.remove(&OTHER); });
        // A malformed welcome is refused, never acted on.
        assert!(handle_server_record(wire::WireHdr { kind: rs::K_WELCOME, aux: 0, peer: 0 }, &[0; 7], &sh).await.is_err());
    }

    fn rec(kind: u16, payload: &[u8]) -> (wire::WireHdr, Vec<u8>) {
        (wire::WireHdr { kind, aux: 0, peer: 0 }, payload.to_vec())
    }

    /// The session domain's server records reach the game as they are: the `session` and
    /// `admin` slots byte for byte, cmd_result / notice / kill_feed / chat_in as S2G records
    /// (deduplicated), pings into the peer directory; then hostile bytes and mutations of
    /// every kind never panic and never publish an invalid record. One test: the process
    /// link and the session tracker are global.
    #[tokio::test]
    async fn session_records_reach_the_game_as_they_are_and_hostile_ones_never_do() {
        let _serial = serial();
        use hsmp_ipc::layout::Str;
        use hsmp_ipc::record::view;
        let l = ipc_shm::ShmLink::test_global();
        let sh = shared(0);
        let w = to_payload(&rs::Welcome { peer_id: 40, server_epoch: 0xE90C_0001, nick: Str::new("me"), ..Default::default() }, &[]);
        let (h, p) = rec(rs::K_WELCOME, &w);
        handle_server_record(h, &p, &sh).await.unwrap();
        assert_eq!(session_client::my_peer_id(), 40);
        // A snapshot: copied as it is; the roster (minus ourselves) is the peer directory.
        let rows = [
            rs::RosterRow { peer_id: 40, seat: 1, connected: true.into(), nick: Str::new("me"), ..Default::default() },
            rs::RosterRow { peer_id: 41, seat: 2, connected: true.into(), nick: Str::new("Zoë"), ..Default::default() },
            rs::RosterRow { peer_id: 0, seat: 3, nick: Str::new("away"), ..Default::default() },
        ];
        let head = rs::SessionHead { epoch: 0xE90C_0001, seq: 5, round: 3, match_id: 77, phase: rs::phase::LIVE, ..Default::default() };
        let sp = to_payload(&head, &rows);
        let (h, p) = rec(rs::K_SESSION, &sp);
        handle_server_record(h, &p, &sh).await.unwrap();
        assert_eq!(l.test_slot("session", None), Some((rs::K_SESSION, sp.clone())), "byte for byte");
        assert_eq!((session_client::round(), session_client::now().match_id), (3, 77));
        let dir = l.segment().peers.dir.read().map(|d| d.0).ok();
        let _ = dir; // the dir is written by the ipc thread; the slot table is the truth here
        assert!(l.slot_for(41).is_some());
        // A stale (older) snapshot is dropped.
        let old = to_payload(&rs::SessionHead { seq: 4, round: 9, ..head }, &[]);
        let (h, p) = rec(rs::K_SESSION, &old);
        handle_server_record(h, &p, &sh).await.unwrap();
        assert_eq!(session_client::round(), 3);
        assert_eq!(l.test_slot("session", None).unwrap().1, sp);
        // Events: once per id / cmd_id, as they are.
        let n = to_payload(&rs::Notice { event_id: 9001, code: rs::notice::PLAYER_JOINED, ..Default::default() }, &[]);
        for _ in 0..2 {
            let (h, p) = rec(rs::K_NOTICE, &n);
            handle_server_record(h, &p, &sh).await.unwrap();
        }
        assert_eq!(l.test_records(rs::K_NOTICE), vec![(0, 0, n.clone())]);
        let r = to_payload(&rs::CmdResult { cmd_id: 770_001, ok: true.into(), ..Default::default() }, &[]);
        for _ in 0..2 {
            let (h, p) = rec(rs::K_CMD_RESULT, &r);
            handle_server_record(h, &p, &sh).await.unwrap();
        }
        // (other tests share the process-wide link: count only this test's cmd_id)
        assert_eq!(l.test_records(rs::K_CMD_RESULT).iter().filter(|x| x.2 == r).count(), 1);
        let c = to_payload(&rs::ChatIn { from_peer: 41, text: Str::new("gg"), ..Default::default() }, &[]);
        let (h, p) = rec(rs::K_CHAT_IN, &c);
        handle_server_record(h, &p, &sh).await.unwrap();
        assert_eq!(l.test_records(rs::K_CHAT_IN), vec![(0, 0, c)]);
        // Admin: slot + flag.
        let a = to_payload(&rs::AdminStateHead { admin_peer: 40, n: 0, _r: 0 }, &[]);
        let (h, p) = rec(rs::K_ADMIN_STATE, &a);
        handle_server_record(h, &p, &sh).await.unwrap();
        assert!(session_client::is_admin() && sh.lock().await.is_admin);
        assert_eq!(l.test_slot("admin", None), Some((rs::K_ADMIN_STATE, a)));
        assert!(session_client::link_now().is_admin.get());

        // Hostile bytes: arbitrary bytes and single-byte mutations of valid messages, every
        // kind of the domain, through the S2C handler and the G2S path.
        let samples: Vec<(u16, Vec<u8>)> = vec![
            (rs::K_WELCOME, w), (rs::K_SESSION, sp), (rs::K_NOTICE, n), (rs::K_CMD_RESULT, r),
            (rs::K_PINGS, to_payload(&rs::PingsHead::default(), &[rs::PingRow { peer_id: 41, rtt_ms: 30, seat: 2, _r: 0 }])),
            (rs::K_KILL_FEED, to_payload(&rs::KillFeed { event_id: 5, ..Default::default() }, &[])),
            (rs::K_KICKED, vec![0; 3]), (rs::K_SERVER_CLOSING, vec![9; 5]),
            (rs::K_PONG, to_payload(&rs::Pong { client_time_ms: 1, server_time_ms: 2 }, &[])),
            (rs::K_COMMAND, to_payload(&rs::Command { text: Str::new("Map_Arena_Pit"), ..rs::Command::new(660_001, rs::cmd_op::PICK_ARENA) }, &[])),
            (rs::K_GAME_STATUS, to_payload(&rs::GameStatus::default(), &[])),
        ];
        let mut x = 0x9e37_79b9u32;
        let mut rnd = || { x ^= x << 13; x ^= x >> 17; x ^= x << 5; x };
        for (kind, valid) in &samples {
            for i in 0..400 {
                let mut m = valid.clone();
                if i % 4 == 0 {
                    let len = (rnd() % 600) as usize;
                    m = (0..len).map(|_| rnd() as u8).collect();
                } else if !m.is_empty() {
                    let j = rnd() as usize % m.len();
                    m[j] ^= (rnd() as u8) | 1;
                    if i % 7 == 0 { m.truncate(rnd() as usize % m.len()); }
                }
                let _ = handle_server_record(wire::WireHdr { kind: *kind, aux: 0, peer: 0 }, &m, &sh).await;
                if hsmp_ipc::schema::check_payload(*kind, &m).is_ok() {
                    // The G2S path only ever sees validated records (the ipc thread checks).
                    let hdr = hsmp_ipc::ring::RecordHeader { kind: *kind, ..Default::default() };
                    super::super::records_in::on_g2s(l, &hdr, &m);
                }
            }
        }
        // Nothing invalid was ever published.
        for slot in ["session", "admin", "link"] {
            if let Some((k, p)) = l.test_slot(slot, None) {
                assert!(hsmp_ipc::schema::check_payload(k, &p).is_ok(), "{slot} holds an invalid record");
            }
        }
        for k in [rs::K_NOTICE, rs::K_KILL_FEED, rs::K_CMD_RESULT, rs::K_CHAT_IN] {
            for (_, _, p) in l.test_records(k) {
                assert!(hsmp_ipc::schema::check_payload(k, &p).is_ok(), "an invalid {k:#x} reached the ring");
            }
        }
        assert!(view::<rs::Link>(hsmp_ipc::bytemuck::bytes_of(&session_client::link_now())).is_ok());
    }

    /// A relayed pose record (codec v2 frame, aux = the relay interval) feeds the
    /// jitter buffer, decoded once, and hands it the relay interval; a frame that is not
    /// v2 is dropped.
    #[tokio::test]
    async fn relayed_pose_record_sets_the_interval_hint() {
        let _serial = serial(); // the play map is process-wide; a fresh welcome clears it
        const P: u32 = 991_077;
        let shared = shared(1);
        let frame = posecodec::v2::encode(&posecodec::v2::Full { ts: 1234.0, ..Default::default() });
        let head = hsmp_ipc::schema::pose::PoseHead { tick: 5, n: 0, _r: 0 };
        let msg = hsmp_ipc::wire::encode(133, P, &head, &frame);
        let (h, p) = hsmp_ipc::wire::split(&msg).unwrap();
        handle_server_record(h, p, &shared).await.unwrap();
        let (n, iv) = with_play(|m| m.get(&P).map(|p| (p.pb.frames.len(), p.pb.interval_hint)).unwrap());
        assert_eq!((n, iv), (1, Some(133.0)));
        let v1 = posecodec::encode(1300, &[(0u8, [0.0, 0.0, 100.0, 0.0, 0.0, 0.0, 1.0])]);
        let mut v1p = vec![0u8; 24];
        v1p.extend_from_slice(&v1);
        let msg = hsmp_ipc::wire::encode(133, P, &head, &v1p);
        let (h, p) = hsmp_ipc::wire::split(&msg).unwrap();
        handle_server_record(h, p, &shared).await.unwrap();
        assert_eq!(with_play(|m| m.get(&P).map(|p| p.pb.frames.len()).unwrap()), 1, "a non-v2 frame is dropped");
        with_play(|m| { m.remove(&P); });
    }
}
