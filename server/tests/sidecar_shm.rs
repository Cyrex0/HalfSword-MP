//! HSMP-SHM end to end: this test process plays the game (it creates and initialises a
//! named segment with `hsmp-ipc`) and spawns the real `hsmp-sidecar --ipc shm:<name>`:
//! the attach handshake, the published link record, the G2S leave record, the clean
//! detach, the tap, the refusals (incl. no `--ipc` at all), "no state files", and a round
//! trip of pose / root / chat through a real server.
#![cfg(windows)]

mod common;
use common::*;

use std::process::{Command, Stdio};
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

use hsmp_ipc::record::{to_payload, view};
use hsmp_ipc::schema::session::{sidecar_status, Leave, Link, K_LEAVE};
use hsmp_ipc::shm;
use hsmp_ipc::{RefuseCode, Segment, SideState};

#[test]
fn attach_publish_leave_detach() {
    let g = Game::new("ok");
    let mut child = g.spawn(shm::current_pid());
    let s = g.seg();
    assert!(until(10, || s.header.sidecar.state() == Some(SideState::Ready)), "sidecar never attached");
    let caps = s.header.caps_effective.load(Ordering::Acquire);
    assert_ne!(caps & hsmp_ipc::schema::CAP_POSE, 0);
    assert_ne!(caps & hsmp_ipc::schema::CAP_QUEUES, 0);
    assert_eq!(caps & hsmp_ipc::schema::CAP_BUS, 0, "the bus is game-local");
    let sc_epoch = s.header.sidecar.epoch.load(Ordering::Acquire);
    assert_ne!(sc_epoch, 0);
    // The heartbeat runs (≤ 1 ms loop).
    let hb0 = s.header.sidecar.hb_count.load(Ordering::Acquire);
    assert!(until(5, || s.header.sidecar.hb_count.load(Ordering::Acquire) > hb0 + 50), "no heartbeat");
    // The sidecar's link record (no --status-file needed) arrives in its slot.
    let mut link: Option<Link> = None;
    assert!(until(10, || { link = g.link(); link.is_some() }), "no link record");
    let (meta, _, _) = g.slot("link").unwrap();
    assert_eq!(meta.writer_epoch, sc_epoch);
    assert_eq!(link.unwrap().status, sidecar_status::CONNECTING);
    // Back to menu: the game sends a leave request; the sidecar leaves and detaches.
    let payload = to_payload(&Leave { reason: 0, _r: [0; 7] }, &[]);
    s.g2s().push(g.epoch, K_LEAVE, 0, 1, &payload).unwrap();
    let (code, err) = wait_exit(&mut child, 20);
    assert_eq!(code, Some(0), "stderr:\n{err}");
    assert_eq!(s.header.sidecar.state(), Some(SideState::Closing));
    // The terminal link record (status ENDED) is the last thing published.
    assert_eq!(g.link().unwrap().status, sidecar_status::ENDED);
    // Tap: attach, the leave record, a link slot publish, detach.
    let tap = tap_events(&g.dir);
    let evs: Vec<&str> = tap.iter().filter_map(|e| e["ev"].as_str()).collect();
    assert_eq!(evs.first(), Some(&"attach"), "{evs:?}");
    assert!(tap.iter().any(|e| e["ev"] == "g2s" && e["kind_id"] == K_LEAVE && e["v"]["reason"] == 0), "{evs:?}");
    assert!(tap.iter().any(|e| e["ev"] == "slot" && e["slot"] == "link"), "{evs:?}");
    assert_eq!(evs.last(), Some(&"detach"), "{evs:?}");
    // No IPC state files in the state dir.
    let left: Vec<String> = std::fs::read_dir(&g.dir).unwrap().flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect();
    for f in &left {
        let ipc = [".sidecar.json", ".match.json", ".session.json", ".link.json", ".metrics.json", ".spawns.json", ".pose_play", "me_remote", ".deaths.log", ".notices.jsonl", ".damage_in.jsonl", ".hitfx_in.jsonl", ".combat_feedback.jsonl", ".interact_in.jsonl"];
        assert!(!ipc.iter().any(|p| f.starts_with(p)), "IPC file {f} written with --ipc shm; dir: {left:?}");
    }
    let _ = std::fs::remove_dir_all(&g.dir);
}

#[test]
fn wrong_parent_is_refused_with_exit_72() {
    let g = Game::new("parent");
    // PID 4 = System: never the game that created the segment.
    let mut child = g.spawn(4);
    let (code, err) = wait_exit(&mut child, 20);
    assert_eq!(code, Some(RefuseCode::WrongParent.exit_code()), "stderr:\n{err}");
    assert_eq!(g.seg().header.prefix.refuse_code(), RefuseCode::WrongParent);
    assert_eq!(g.seg().header.sidecar.state(), Some(SideState::Absent), "never attached");
    let tap = tap_events(&g.dir);
    assert!(tap.iter().any(|e| e["ev"] == "refuse" && e["code"] == "wrong_parent"), "{tap:?}");
    let _ = std::fs::remove_dir_all(&g.dir);
}

#[test]
fn layout_mismatch_is_refused_with_exit_71() {
    let g = Game::new("abi");
    // A game built from another schema: same magic and major, other layout hash.
    unsafe {
        let pre = std::ptr::addr_of_mut!((*g.map.segment_ptr()).header.prefix.layout_hash);
        pre.write_volatile(0x0bad_0bad_0bad_0bad);
    }
    let mut child = g.spawn(shm::current_pid());
    let (code, err) = wait_exit(&mut child, 20);
    assert_eq!(code, Some(RefuseCode::AbiMismatch.exit_code()), "stderr:\n{err}");
    assert_eq!(g.seg().header.prefix.refuse_code(), RefuseCode::AbiMismatch);
    assert!(g.seg().header.prefix.refuse_detail().contains("0bad0ba"), "{}", g.seg().header.prefix.refuse_detail());
    let _ = std::fs::remove_dir_all(&g.dir);
}

#[test]
fn missing_segment_is_refused() {
    let dir = std::env::temp_dir().join(format!("hsmp_sidecar_shm_missing_{}", std::process::id()));
    let _ = std::fs::create_dir_all(&dir);
    let mut child = Command::new(env!("CARGO_BIN_EXE_hsmp-sidecar"))
        .args(["--server", "127.0.0.1:9", "--state-dir"])
        .arg(&dir)
        .args(["--parent-pid", &shm::current_pid().to_string(), "--ipc", "shm:Local\\HSMP.ipc.1.nope.0"])
        .env("HSMP_CAREER_GUARD", "0")
        .env("HSMP_IDENTITY_DIR", &dir)
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let (code, err) = wait_exit(&mut child, 20);
    assert_eq!(code, Some(RefuseCode::NotReady.exit_code()), "stderr:\n{err}");
    let _ = std::fs::remove_dir_all(&dir);
}

// ---- two games, two sidecars, one real server ---------------------------------------------

fn gmeta(s: &Segment, epoch: u64) -> hsmp_ipc::schema::SlotMeta {
    hsmp_ipc::schema::SlotMeta { writer_epoch: epoch, world_epoch: s.header.world_epoch.load(Ordering::Acquire), valid: 1, ..Default::default() }
}

#[test]
fn pose_root_and_chat_round_trip_through_a_real_server() {
    use hsmp_ipc::schema::pose::{PeerRoot, PoseBuf, K_POSE, K_ROOT, PEER_PLAY_V2};
    use hsmp_ipc::schema::RawSlot;
    use hsmp_ipc::schema::session::{
        cmd_op, phase, status_flag, Command, GameStatus, RosterRow, SessionHead, K_CHAT, K_CHAT_IN, K_COMMAND, K_GAME_STATUS,
    };
    let root = std::env::temp_dir().join(format!("hsmp_sidecar_shm_rt_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    let (_sv, addr) = server(&root);
    let (ga, gb) = (Game::new("rta"), Game::new("rtb"));
    let side = |g: &Game, nick: &str| {
        spawn_logged(env!("CARGO_BIN_EXE_hsmp-sidecar"), &g.sidecar_args(&addr, nick, shm::current_pid()), &g.dir, &g.dir.join("sidecar.log"))
    };
    let _a = side(&ga, "Alpha");
    assert!(ga.wait_connected(1, 30), "A never connected; see {}", ga.dir.join("sidecar.log").display());
    let _b = side(&gb, "Bravo");
    assert!(gb.wait_connected(2, 30), "B never connected; see {}", gb.dir.join("sidecar.log").display());

    // Game A: a still pose at a fixed spot, 60 Hz, the way HSMPSync's fast_send writes it.
    let (sa, sb) = (ga.seg(), gb.seg());
    let mut bones = [0f64; hsmp_pose::sample::BONE_NUMS];
    for i in 0..23 {
        bones[i * 13..i * 13 + 13].copy_from_slice(&[1000.0 + i as f64, -500.0, 100.0 + 4.0 * i as f64, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0]);
    }
    let mut enc = hsmp_pose::sample::Scratch::default();
    let mut buf = PoseBuf::new_boxed();
    let mut scratch = Vec::new();
    let t0 = Instant::now();
    let mut tick = 0u32;
    let mut found: Option<(usize, hsmp_ipc::schema::pose::PeerPlay)> = None;
    let mut lobby_root = None;
    let mut nick = String::new();
    // B's view of A's root slot: the newest PeerRoot from peer 1, if any.
    let peer_root = |scratch: &mut Vec<u64>| -> Option<PeerRoot> {
        let (dir, _) = sb.peers.dir.read().ok()?;
        let mut out = Vec::new();
        sb.peers.slots[dir.slot_of(1)?].root.get(scratch, &mut out)?;
        let v = hsmp_ipc::record::view::<PeerRoot>(&out).ok()?;
        (v.head.peer_id == 1 && v.head.root.tick > 0).then(|| v.head())
    };
    // Lobby: pose and roster flow, but a root without its match context is not relayed.
    // The directory entry starts as "P1" when the first pose lands and gets the real nick
    // once the session roster arrives, which can be later, so wait for both.
    while t0.elapsed() < Duration::from_secs(30) && (found.is_none() || nick != "Alpha") {
        tick += 1;
        let ts = 1000.0 + t0.elapsed().as_secs_f64() * 1000.0;
        // The way HSMPNative put_pose / put_root build and publish the records.
        let a = hsmp_pose::sample::PoseArgs { tick: tick as f64, ts, dt: 16.6, b: &bones, w: &[], c: None };
        assert!(hsmp_pose::sample::encode_pose(&a, &mut enc, &mut buf));
        assert!(sa.game_out.local_pose.put(gmeta(sa, ga.epoch), K_POSE, buf.payload(), &mut scratch));
        let r = hsmp_pose::sample::root(&[tick as f64, ts, 1000.0, -500.0, 100.0, 0.0, 90.0, 0.0, 0.0, 0.0, 0.0], 0);
        assert!(sa.game_out.local_root.put(gmeta(sa, ga.epoch), K_ROOT, bytemuck::bytes_of(&r), &mut scratch));
        // Game B: the peer directory, then that slot's playback and root.
        if let Ok((dir, _)) = sb.peers.dir.read() {
            if let Some(s) = dir.slot_of(1) {
                nick = dir.entries[s].nick().into_owned();
                if let Ok((p, _)) = sb.peers.slots[s].play.read() {
                    if p.peer_id == 1 && p.play_seq > 5 && p.mask & 1 == 1 {
                        found = Some((s, p));
                    }
                }
            }
        }
        lobby_root = lobby_root.or_else(|| peer_root(&mut scratch));
        std::thread::sleep(Duration::from_millis(16));
    }
    let (_, p) = found.expect("B never got A's playback in its PeerPlay slot");
    assert_eq!(p.flags & PEER_PLAY_V2, PEER_PLAY_V2);
    assert_eq!(p.meta.writer_epoch, sb.header.sidecar.epoch.load(Ordering::Acquire));
    for k in 0..3 {
        assert!((p.b[0][k] as f64 - bones[k]).abs() < 1.0, "pelvis {:?} vs sent {:?}", &p.b[0][..3], &bones[..3]);
    }
    assert_eq!(nick, "Alpha", "nick from the roster");
    assert!(lobby_root.is_none(), "a lobby root without match context was relayed: {lobby_root:?}");

    // Both games READY (G2S command records); with no admin the server auto-starts the match.
    for (g, s, id) in [(&ga, sa, 10u32), (&gb, sb, 11)] {
        let cmd = Command { flag: true.into(), ..Command::new(id, cmd_op::READY) };
        s.g2s().push(g.epoch, K_COMMAND, 0, id as u64, &to_payload(&cmd, &[])).unwrap();
    }
    // A's spawn order for the pending round, as the sidecar publishes it in the session slot.
    let mut order: Option<(SessionHead, RosterRow)> = None;
    until(30, || {
        order = ga.slot("session").and_then(|(_, _, p)| {
            let v = view::<SessionHead>(&p).ok()?;
            let h = v.head();
            let row = *v.rows.iter().find(|r| r.peer_id == 1 && r.spawn_id != 0)?;
            (h.match_id != 0 && matches!(h.phase, phase::LOADING | phase::COUNTDOWN)).then_some((h, row))
        });
        order.is_some()
    });
    let (sess, row) = order.expect("no match with a spawn order for A; see server.log");
    let (round, spawn) = (sess.pending_round(), row.spawn_pos);
    // Game A loaded the arena and placed its pawn on that order (the Director's game status,
    // ~4 Hz); then roots stamped with the original context, standing on the spawn point.
    let gs = GameStatus {
        match_id: sess.match_id, round, life: 1, spawn_id: row.spawn_id,
        flags: status_flag::LOADED | status_flag::READY, arena: *sess.arena(), ..Default::default()
    };
    let gs = to_payload(&gs, &[]);
    let at = [spawn[0] as f64, spawn[1] as f64, spawn[2] as f64 + 100.0];
    let mut root_seen = None;
    let t1 = Instant::now();
    let mut req = 100u64;
    while t1.elapsed() < Duration::from_secs(30) && root_seen.is_none() {
        if tick.is_multiple_of(15) {
            req += 1;
            sa.g2s().push(ga.epoch, K_GAME_STATUS, 0, req, &gs).unwrap();
        }
        tick += 1;
        let ts = 1000.0 + t0.elapsed().as_secs_f64() * 1000.0;
        let mut r = hsmp_pose::sample::root(&[tick as f64, ts, at[0], at[1], at[2], 0.0, 90.0, 0.0, 0.0, 0.0, 0.0], 0);
        (r.match_id, r.round, r.life) = (sess.match_id, round, 1);
        assert!(sa.game_out.local_root.put(gmeta(sa, ga.epoch), K_ROOT, bytemuck::bytes_of(&r), &mut scratch));
        root_seen = peer_root(&mut scratch);
        std::thread::sleep(Duration::from_millis(16));
    }
    let rr = root_seen.expect("B never got A's placed root");
    let yaw = hsmp_pose::sample::quat_yaw(rr.root.rot);
    for k in 0..3 {
        assert!((rr.root.pos[k] as f64 - at[k]).abs() < 0.01, "{:?} vs sent {at:?}", rr.root.pos);
    }
    assert!((yaw - 90.0).abs() < 0.1, "yaw {yaw}");
    assert_eq!((rr.root.match_id, rr.root.round, rr.root.life), (sess.match_id, round, 1));

    // Chat: A's G2S chat record reaches B as an S2G chat_in record.
    let mut rec = hsmp_ipc::ring::Record::default();
    let sc_b = sb.header.sidecar.epoch.load(Ordering::Acquire);
    while !matches!(sb.s2g().pop(sc_b, &mut rec), hsmp_ipc::ring::Pop::Empty) {}
    let msg = to_payload(&hsmp_ipc::schema::session::Chat { text: hsmp_ipc::layout::Str::new("hello über") }, &[]);
    sa.g2s().push(ga.epoch, K_CHAT, 0, 77, &msg).unwrap();
    let mut got = None;
    until(30, || {
        while let hsmp_ipc::ring::Pop::Record = sb.s2g().pop(sc_b, &mut rec) {
            if rec.hdr.kind == K_CHAT_IN {
                got = view::<hsmp_ipc::schema::session::ChatIn>(rec.payload()).ok().map(|v| v.head());
            }
        }
        got.is_some()
    });
    let v = got.expect("no chat_in on B");
    assert_eq!(v.text.as_str(), Some("hello über"));
    assert_eq!((v.from_peer, v.nick.as_str()), (1, Some("Alpha")));
    // Neither sidecar wrote IPC files.
    for g in [&ga, &gb] {
        for e in std::fs::read_dir(&g.dir).unwrap().flatten() {
            let n = e.file_name().to_string_lossy().into_owned();
            assert!(!(n.starts_with(".pose_play") || n.starts_with("me_remote") || n == ".sidecar.json" || n == ".chat.log"), "{n}");
        }
    }
}
