//! HSMP-SHM backend tests: an in-process fake game (`Mapping::anonymous` +
//! `handshake::init_game`) against the real `ShmLink` + `Worker`.

#[cfg(test)]
use super::*;
use hsmp_ipc::handshake::{init_game, SideParams};
use hsmp_ipc::schema::pose::{PeerPlay, PoseLead, PEER_PLAY_HAS_CONTROL, PEER_PLAY_V2};

const GAME_EPOCH: u64 = 0x6a6e_0001;

struct Fake {
    l: &'static ShmLink,
    w: Worker,
    rx: tokio::sync::mpsc::Receiver<Out>,
}

fn fake() -> Fake {
    fake_caps(CAPS_OFFERED)
}

fn fake_caps(game_caps: u64) -> Fake {
    let m: &'static Mapping = Box::leak(Box::new(Mapping::anonymous()));
    let p = SideParams { pid: 4242, create_time: 1, epoch: GAME_EPOCH, caps: game_caps, build_id: "test-game" };
    unsafe { init_game(m.segment_ptr(), m.len(), &p, 10_000_000) }.unwrap();
    let seg = m.segment().unwrap();
    let sc = SideParams { pid: 1, create_time: 2, epoch: 0x5c_0001, caps: CAPS_OFFERED, build_id: "test-sidecar" };
    handshake::attach_sidecar(seg, m.len(), 4242, Some(1), &sc).unwrap();
    let l = ShmLink::for_test(seg, 0x5c_0001);
    let (tx, rx) = tokio::sync::mpsc::channel(64);
    l.out_tx.set(tx).unwrap();
    let w = Worker::new(l, None);
    Fake { l, w, rx }
}

impl Fake {
    fn seg(&self) -> &'static Segment {
        self.l.seg
    }
    /// A game-side SlotMeta for the current world.
    fn gmeta(&self) -> SlotMeta {
        SlotMeta { writer_epoch: GAME_EPOCH, world_epoch: self.seg().header.world_epoch.load(Ordering::Acquire), session_epoch: 0, valid: 1, sample_seq: 0, t_us: 0 }
    }
    fn step(&mut self) {
        self.w.step();
    }
    /// The next batch of v6 record messages the worker handed to the network.
    fn msgs(&mut self) -> Option<Vec<Vec<u8>>> {
        match self.rx.try_recv().ok()? {
            Out::Msgs(m) => Some(m),
        }
    }
    /// A sidecar-written record slot as the game reads it: (meta, kind, payload).
    fn slot(&self, name: &str) -> Option<(SlotMeta, u16, Vec<u8>)> {
        let (mut scratch, mut out) = (Vec::new(), Vec::new());
        let (m, k, _) = self.seg().slot_ref(name, 0)?.get(&mut scratch, &mut out)?;
        Some((m, k, out))
    }
    /// A typed G2S record from the game.
    fn game_rec(&self, kind: u16, req: u64, payload: &[u8]) {
        self.seg().g2s().push(GAME_EPOCH, kind, 0, req, payload).unwrap();
    }
}

fn session_payload(seq: u32, peers: &[(u32, &str)]) -> Vec<u8> {
    use hsmp_ipc::schema::session::{RosterRow, SessionHead};
    let rows: Vec<RosterRow> = peers
        .iter()
        .enumerate()
        .map(|(i, (p, n))| RosterRow { peer_id: *p, seat: i as u8 + 1, connected: true.into(), nick: hsmp_ipc::layout::Str::new(n), ..Default::default() })
        .collect();
    hsmp_ipc::record::to_payload(&SessionHead { epoch: 5, seq, ..Default::default() }, &rows)
}

#[test]
fn session_records_reach_their_slots_as_they_are() {
    use hsmp_ipc::schema::session::{AdminStateHead, BanRow, Link, K_ADMIN_STATE, K_LINK, K_SESSION};
    let mut f = fake();
    let p = session_payload(9, &[(3, "Héloïse"), (5, "B")]);
    f.l.post_record("session", None, K_SESSION, &p);
    let link = Link { my_peer_id: 3, status: 1, reason: hsmp_ipc::layout::Str::new("x"), ..Default::default() };
    f.l.post_record("link", None, K_LINK, hsmp_ipc::bytemuck::bytes_of(&link));
    let admin = hsmp_ipc::record::to_payload(&AdminStateHead { admin_peer: 3, n: 0, _r: 0 }, &[BanRow { entry: hsmp_ipc::layout::Str::new("1.2.x.x#00") }]);
    f.l.post_record("admin", None, K_ADMIN_STATE, &admin);
    f.step();
    let (m, k, got) = f.slot("session").expect("session published");
    assert_eq!((k, &got), (K_SESSION, &p), "byte for byte");
    assert_eq!(m.writer_epoch, f.l.epoch);
    assert!(hsmp_ipc::record::view::<hsmp_ipc::schema::session::SessionHead>(&got).is_ok());
    let (_, k, got) = f.slot("link").unwrap();
    assert_eq!((k, got.as_slice()), (K_LINK, hsmp_ipc::bytemuck::bytes_of(&link)));
    let (_, k, got) = f.slot("admin").unwrap();
    assert_eq!((k, &got), (K_ADMIN_STATE, &admin));
    // Every Welcome starts a session epoch; the next publish carries it.
    let e0 = f.seg().header.session_epoch.load(Ordering::Acquire);
    f.l.on_welcome();
    f.l.post_record("session", None, K_SESSION, &session_payload(10, &[]));
    f.step();
    assert_eq!(f.seg().header.session_epoch.load(Ordering::Acquire), e0 + 1);
    assert_eq!(f.slot("session").unwrap().0.session_epoch, e0 + 1);
    // Shutdown without a running thread writes the final link record itself.
    let fin = Link { status: 7, state: 4, ..Default::default() };
    f.l.shutdown(Some(hsmp_ipc::bytemuck::bytes_of(&fin)), Duration::from_millis(1));
    let (_, _, got) = f.slot("link").unwrap();
    assert_eq!(hsmp_ipc::record::view::<Link>(&got).unwrap().head.status, 7, "ENDED");
}

#[test]
fn the_peer_directory_follows_the_roster() {
    let mut f = fake();
    // Peer 9 is known only from its stream (not in any roster yet).
    let pr9 = hsmp_ipc::schema::pose::PeerRoot { peer_id: 9, _r: 0, root: root_rec(1.0) };
    f.l.post_record("peer_root", Some(9), hsmp_ipc::schema::pose::K_PEER_ROOT, bytemuck::bytes_of(&pr9));
    f.l.sync_roster(&[(7, "Zoë".into()), (8, "Bob".into())]);
    f.l.set_rtts(&[(7, 48), (8, 0), (42, 99)]);
    let vp = |seq: u32| hsmp_ipc::record::to_payload(&hsmp_ipc::schema::combat::Vitals { seq, ..Default::default() }, &[]);
    f.l.post_record("peer_vitals", Some(7), hsmp_ipc::schema::combat::K_VITALS, &vp(3));
    f.step();
    let (dir, _) = f.seg().peers.dir.read().unwrap();
    let (s7, s8) = (dir.slot_of(7).expect("lobby peer 7 has an entry"), dir.slot_of(8).unwrap());
    assert_eq!((dir.entries[s7].nick().as_ref(), dir.entries[s7].rtt_ms), ("Zoë", 48));
    assert_eq!(dir.entries[s8].nick(), "Bob");
    assert!(dir.slot_of(9).is_some());
    assert_eq!(dir.count, 3);
    // A rename follows the roster; peer 7 leaves the roster: freed with its per-peer state.
    f.l.post_record("peer_vitals", Some(7), hsmp_ipc::schema::combat::K_VITALS, &vp(4));
    f.l.sync_roster(&[(8, "Bobby".into())]);
    f.step();
    let (dir, _) = f.seg().peers.dir.read().unwrap();
    assert!(dir.slot_of(7).is_none(), "left the roster: freed");
    assert_eq!(dir.entries[s8].nick(), "Bobby");
    assert!(dir.slot_of(9).is_some(), "a stream-only peer is not freed by the roster");
    assert!(f.l.lock().rec_slots.keys().all(|(_, p)| *p != Some(7)), "its pending vitals are dropped");
    // A new server session frees everyone.
    f.l.reset_peers();
    f.step();
    let (dir, _) = f.seg().peers.dir.read().unwrap();
    assert_eq!(dir.count, 0);
    assert!(dir.entries.iter().all(|e| e.active == 0));
}

/// A game write of a hot record (what HSMPNative's put_* does).
fn game_put(f: &Fake, slot: &'static str, meta: SlotMeta, kind: u16, payload: &[u8]) {
    let mut scratch = Vec::new();
    assert!(f.seg().slot_ref(slot, 0).unwrap().put(meta, kind, payload, &mut scratch));
}

fn root_rec(tick: f64) -> hsmp_ipc::schema::pose::Root {
    let mut r=hsmp_pose::sample::root(&[tick, 1234.7, 100.0, -200.0, 90.5, 0.0, 90.0, 0.0, 1.0, 2.0, 3.0], 1_700_000_000_000);
    r.match_id=1;r.round=1;r.life=1;r
}

fn pose_rec(tick: f64) -> Vec<u8> {
    let f = hsmp_pose::posecodec::v2::encode(&hsmp_pose::posecodec::v2::Full { ts: 1000.25, ..Default::default() });
    hsmp_ipc::record::to_payload(&hsmp_ipc::schema::pose::PoseHead { tick: tick as u32, n: 0, _r: 0 }, &f)
}

#[test]
fn root_weapon_lead_round_trip_with_epoch_and_world_checks() {
    let mut f = fake();
    let r = root_rec(5.0);
    game_put(&f, "local_root", f.gmeta(), K_ROOT, bytemuck::bytes_of(&r));
    f.step();
    let m = f.msgs().expect("root sent");
    assert_eq!(m, vec![hsmp_ipc::wire::message(K_ROOT, 0, 0, bytemuck::bytes_of(&r))], "the record bytes, framed, as written");
    // Same tick again: not resent.
    game_put(&f, "local_root", f.gmeta(), K_ROOT, bytemuck::bytes_of(&r));
    f.step();
    assert!(f.msgs().is_none());
    // A sample from the previous world is dropped; so are invalid and foreign-epoch ones.
    let stale = f.gmeta();
    f.seg().header.world_epoch.fetch_add(1, Ordering::AcqRel);
    let r6 = root_rec(6.0);
    game_put(&f, "local_root", stale, K_ROOT, bytemuck::bytes_of(&r6));
    f.step();
    assert!(f.msgs().is_none(), "stale world_epoch");
    let mut m0 = f.gmeta();
    m0.valid = 0;
    game_put(&f, "local_root", m0, K_ROOT, bytemuck::bytes_of(&r6));
    f.step();
    assert!(f.msgs().is_none(), "valid = 0");
    let mut me = f.gmeta();
    me.writer_epoch = 99;
    game_put(&f, "local_root", me, K_ROOT, bytemuck::bytes_of(&r6));
    f.step();
    assert!(f.msgs().is_none(), "old game epoch");
    // A scribbled record (NaN position) never leaves.
    let mut bad = root_rec(7.0);
    bad.pos[0] = f32::NAN;
    game_put(&f, "local_root", f.gmeta(), K_ROOT, bytemuck::bytes_of(&bad));
    f.step();
    assert!(f.msgs().is_none(), "invalid record");
    game_put(&f, "local_root", f.gmeta(), K_ROOT, bytemuck::bytes_of(&r6));
    f.step();
    let m = f.msgs().unwrap();
    assert_eq!(hsmp_ipc::wire::decode::<hsmp_ipc::schema::pose::Root>(&m[0]).unwrap().1.head.tick, 6);

    let w = hsmp_pose::sample::weapon(&[3.0, 50.0, 9.0, 2.0, 1.0, 2.0, 3.0, 10.0, 20.0, 30.0, 0.5, 0.5, 0.5]);
    game_put(&f, "local_weapon", f.gmeta(), K_WEAPON, bytemuck::bytes_of(&w));
    f.step();
    let m = f.msgs().unwrap();
    let (_, v) = hsmp_ipc::wire::decode::<hsmp_ipc::schema::pose::Weapon>(&m[0]).unwrap();
    assert_eq!((v.head.tick, v.head.ts, v.head.weapon_id, v.head.held), (3, 50, 9, 2));

    go_lead(&f);
    f.step();
    assert_eq!(f.l.pose_lead(), 7.5);
}

fn go_lead(f: &Fake) {
    f.seg().game_out.pose_lead.write(&PoseLead { meta: f.gmeta(), lead_ms: 7.5, _r: 0 });
}

/// One sample (root + weapon + pose written before one pump step) leaves as ONE batch, which
/// the forwarder queues together and sends in one transmit (one datagram where it fits).
#[test]
fn one_sample_is_one_batch_and_the_pose_bytes_are_untouched() {
    let mut f = fake();
    let r = root_rec(42.0);
    let w = hsmp_pose::sample::weapon(&[42.0, 1000.0, 9.0, 1.0, 1.0, 2.0, 3.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0]);
    let p = pose_rec(42.0);
    game_put(&f, "local_root", f.gmeta(), K_ROOT, bytemuck::bytes_of(&r));
    game_put(&f, "local_weapon", f.gmeta(), K_WEAPON, bytemuck::bytes_of(&w));
    game_put(&f, "local_pose", f.gmeta(), K_POSE, &p);
    f.step();
    let m = f.msgs().expect("one batch");
    assert_eq!(m.len(), 3);
    assert_eq!(m[2], hsmp_ipc::wire::message(K_POSE, 0, 0, &p), "the frame is framed, never re-encoded");
    assert!(f.msgs().is_none(), "nothing else");
    // A pose record whose head claims a frame shorter than the v2 header is refused.
    let short = hsmp_ipc::record::to_payload(&hsmp_ipc::schema::pose::PoseHead { tick: 43, n: 0, _r: 0 }, &[0xFF; 10]);
    game_put(&f, "local_pose", f.gmeta(), K_POSE, &short);
    f.step();
    assert!(f.msgs().is_none());
}

#[test]
fn combat_pose_evidence_retains_only_exact_transmitted_frames() {
    let mut f=fake();
    let path=std::env::temp_dir().join(format!("hsmp-combat-pose-{}-{}.jsonl",std::process::id(),rand::random::<u64>()));
    f.w.tap=Some(Tap::open(&path).unwrap());
    f.w.tap_pose_frames=true;
    let p=pose_rec(42.0);
    game_put(&f,"local_pose",f.gmeta(),K_POSE,&p);
    f.step();
    assert_eq!(f.msgs().unwrap(),vec![hsmp_ipc::wire::message(K_POSE,0,0,&p)]);
    game_put(&f,"local_pose",f.gmeta(),K_POSE,&p);
    f.step();
    assert!(f.msgs().is_none(),"duplicate tick has no new transmission/evidence");
    let short=hsmp_ipc::record::to_payload(&hsmp_ipc::schema::pose::PoseHead{tick:43,n:0,_r:0},&[0xff;10]);
    game_put(&f,"local_pose",f.gmeta(),K_POSE,&short);
    f.step();
    assert!(f.msgs().is_none(),"invalid pose has no transmission/evidence");
    f.w.tap.as_mut().unwrap().flush();
    let text=std::fs::read_to_string(&path).unwrap();
    let rows:Vec<J>=text.lines().map(|l|serde_json::from_str(l).unwrap()).filter(|v:&J|v["ev"]=="pose_tx").collect();
    assert_eq!(rows.len(),1);
    assert_eq!(hex::decode(rows[0]["payload_hex"].as_str().unwrap()).unwrap(),p);
    assert_eq!(rows[0]["tick"],42);
    assert_eq!(rows[0]["world_epoch"],f.gmeta().world_epoch);
    f.w.tap=None;
    std::fs::remove_file(&path).unwrap();
}

#[test]
fn peer_dir_root_codec_slots_and_slot_release() {
    let mut f = fake();
    f.l.sync_roster(&[(7, "Zoë".into())]);
    let pr = hsmp_ipc::schema::pose::PeerRoot { peer_id: 7, _r: 0, root: root_rec(10.0) };
    f.l.post_record("peer_root", Some(7), hsmp_ipc::schema::pose::K_PEER_ROOT, bytemuck::bytes_of(&pr));
    let mut vit = hsmp_ipc::schema::combat::Vitals { seq: 3, ..Default::default() };
    vit.v[0] = hsmp_ipc::schema::combat::vitals_q(0, 80.5);
    let vp = hsmp_ipc::record::to_payload(&vit, &[]);
    f.l.post_record("peer_vitals", Some(7), hsmp_ipc::schema::combat::K_VITALS, &vp);
    let kit = { let mut k = hsmp_ipc::schema::loadout::Kit::default(); k.rev = 2; k.class = hsmp_ipc::layout::Str::new("knight"); hsmp_ipc::record::to_payload(&k, &[]) };
    let lo = { let mut h = hsmp_ipc::schema::loadout::LoadoutHead::default(); h.version = 4; hsmp_ipc::record::to_payload(&h, &[]) };
    f.l.post_record("peer_kit", Some(7), hsmp_ipc::schema::loadout::K_KIT_VERDICT, &kit);
    f.l.post_record("peer_loadout", Some(7), hsmp_ipc::schema::loadout::K_LOADOUT, &lo);
    f.step();
    let (dir, _) = f.seg().peers.dir.read().unwrap();
    let s = dir.slot_of(7).expect("peer 7 has a slot");
    let e = &dir.entries[s];
    assert_eq!((e.active, e.peer_id), (1, 7));
    assert_eq!(e.nick(), "Zoë");
    assert_eq!(dir.meta.writer_epoch, f.l.epoch);
    let ps = &f.seg().peers.slots[s];
    let mut sc = Vec::new();
    let mut out = Vec::new();
    let (meta, kind, _) = f.seg().slot_ref("peer_root", s).unwrap().get(&mut sc, &mut out).unwrap();
    assert_eq!((meta.writer_epoch, kind), (f.l.epoch, hsmp_ipc::schema::pose::K_PEER_ROOT));
    assert_eq!(out, bytemuck::bytes_of(&pr), "the relayed record + peer id, as they are");
    hsmp_ipc::record::view::<hsmp_ipc::schema::pose::PeerRoot>(&out).unwrap();
    let (v, _) = ps.vitals.read().unwrap();
    assert_eq!((v.kind as u16, v.payload()), (hsmp_ipc::schema::combat::K_VITALS, &vp[..]), "the record as posted");
    assert_eq!(v.meta.writer_epoch, f.l.epoch);
    let (mut sc, mut out) = (Vec::new(), Vec::new());
    let (_, kk, _) = f.seg().slot_ref("peer_kit", s).unwrap().get(&mut sc, &mut out).unwrap();
    assert_eq!((kk, &out[..]), (hsmp_ipc::schema::loadout::K_KIT_VERDICT, &kit[..]), "the record as it arrived");
    let (_, lk, _) = f.seg().slot_ref("peer_loadout", s).unwrap().get(&mut sc, &mut out).unwrap();
    assert_eq!((lk, &out[..]), (hsmp_ipc::schema::loadout::K_LOADOUT, &lo[..]));

    // The peer leaves the roster: its slot goes inactive; a new peer never lands in it first.
    let gen = e.gen;
    f.l.sync_roster(&[]);
    let pr8 = hsmp_ipc::schema::pose::PeerRoot { peer_id: 8, _r: 0, root: root_rec(1.0) };
    f.l.post_record("peer_root", Some(8), hsmp_ipc::schema::pose::K_PEER_ROOT, bytemuck::bytes_of(&pr8));
    f.step();
    let (dir, _) = f.seg().peers.dir.read().unwrap();
    assert_eq!(dir.entries[s].active, 0);
    let s8 = dir.slot_of(8).unwrap();
    assert_ne!(s8, s, "a just-released slot is reused last");
    assert_eq!(dir.count, 1);
    // Reuse bumps the generation.
    for p in 100..131 {
        f.l.slot_for(p);
    }
    f.step();
    let (dir, _) = f.seg().peers.dir.read().unwrap();
    assert_eq!(dir.entries[s].active, 1);
    assert!(dir.entries[s].gen > gen);
    assert!(f.l.slot_for(999).is_none(), "32 slots");
}

#[test]
fn peer_play_slot_carries_the_play_line_fields() {
    let f = fake();
    let mut s = hsmp_pose::poseplay::Sample {
        pt: 1500.25, lead: 3.0, mode: hsmp_pose::poseplay::Mode::Extrap, age: 12.0, delay: 40.0, jitter: 2.5, interval: 16.7,
        cut: 4, mask: 0b11 | (1 << hsmp_pose::poseplay::WPN_R), bones: [[0.0; 7]; hsmp_pose::poseplay::SLOTS], root: Some(([1.0, 2.0, 3.0], 90.0)),
        vmask: 0b1, vel: [[0.0; 6]; hsmp_pose::poseplay::SLOTS], v2: true, rate: 1.0,
        extra: Some(std::sync::Arc::new(hsmp_pose::poseplay::Extra {
            context: Some(hsmp_pose::posecodec::v2::Context{match_id:0xfedcba9876543210,round:9,life:3}),
            weapons: [Some((1, 77, [0.0, 0.0, 1.0], [0.0, 0.0, 90.0])), None],
            control: Some(hsmp_pose::posecodec::v2::Control { flags: 9, grip_r: 2, ik_world: [true, false, false, false], ..Default::default() }),
            k: 1.02, clock_ts: None, step: 8.0,
        })),
    };
    s.bones[1] = [1.0, 2.0, 3.0, 0.0, 0.0, 0.0, 1.0];
    s.vel[1] = [4.0, 5.0, 6.0, 7.0, 8.0, 9.0];
    let mut p = super::super::pose::peer_play_of(7, 33, &s);
    p.meta = f.l.meta();
    let slot = f.l.slot_for(7).unwrap();
    f.l.write_play(slot, &p);
    let (g, _): (PeerPlay, u64) = f.seg().peers.slots[slot].play.read().unwrap();
    assert!(g.has_context.get());
    assert_eq!((g.match_id,g.round,g.life),(0xfedcba9876543210,9,3));
    assert_eq!((g.peer_id, g.play_seq, g.mode, g.cut), (7, 33, 1, 4));
    assert_eq!(g.pt, 1500.25);
    assert_eq!((g.delay, g.jit, g.lead, g.st, g.iv, g.k), (40.0, 2.5, 3.0, 8.0, 16.7, 1.02));
    assert_eq!(g.flags & PEER_PLAY_V2, PEER_PLAY_V2);
    assert_eq!(g.flags & PEER_PLAY_HAS_CONTROL, PEER_PLAY_HAS_CONTROL);
    assert_eq!(g.root, [1.0, 2.0, 3.0, 90.0]);
    assert_eq!(&g.b[1], &[1.0, 2.0, 3.0, 0.0, 0.0, 0.0, 1.0, 4.0, 5.0, 6.0, 7.0, 8.0, 9.0]);
    assert_eq!((g.w[0].present, g.w[0].hands, g.w[0].class_id, g.w[0].tip[2]), (1, 1, 77, 90.0));
    assert_eq!(g.w[1].present, 0);
    assert_eq!((g.control.flags, g.control.grip_r, g.control.ik_world), (9, 2, 1));
    assert_eq!(g.meta.writer_epoch, f.l.epoch);
}

#[test]
fn g2s_queues_dedup_requests_and_stale_epochs() {
    let mut f = fake();
    // Typed session records: framed as they are (a command is also kept for its resends).
    use hsmp_ipc::schema::session::{cmd_op, Command, GameStatus, K_COMMAND, K_GAME_STATUS};
    let gs = hsmp_ipc::record::to_payload(&GameStatus { match_id: 9, round: 2, life: 1, flags: 1, ..Default::default() }, &[]);
    f.game_rec(K_GAME_STATUS, 1, &gs);
    f.game_rec(K_GAME_STATUS, 1, &gs); // duplicate req_id
    f.seg().g2s().push(GAME_EPOCH ^ 1, K_GAME_STATUS, 0, 2, &gs).unwrap(); // a dead game instance
    f.game_rec(0x7FFE, 3, b"{\"kind\":\"match\"}"); // not a record kind: dropped
    let cmd = hsmp_ipc::record::to_payload(&Command::new(880_001, cmd_op::START), &[]);
    f.game_rec(K_COMMAND, 6, &cmd);
    f.game_rec(K_COMMAND, 7, &[0u8; 8]); // not a valid command: dropped by the validator
    f.step();
    let mut sent = Vec::new();
    while let Some(m) = f.msgs() {
        sent.extend(m);
    }
    assert_eq!(sent.len(), 2, "game_status + command");
    assert_eq!(sent[0], hsmp_ipc::wire::message(K_GAME_STATUS, 0, 0, &gs));
    assert_eq!(sent[1], hsmp_ipc::wire::message(K_COMMAND, 0, 0, &cmd));
    let sb = &f.seg().header.sidecar;
    assert_eq!(sb.counter(ctr::BAD_RECORD), 2, "the unknown kind + the invalid command");
    assert_eq!(sb.counter(ctr::STALE_EPOCH), 1);
    assert_eq!(sb.counter(ctr::MSGS_IN), 5);
}

#[test]
fn s2g_events_in_order_with_overflow_and_resync() {
    use hsmp_ipc::schema::combat::{Death, K_DEATH};
    let mut f = fake();
    // Typed S2G records (push_record): pushed in order with their wire header fields.
    let d = |round: u32| hsmp_ipc::record::to_payload(&Death::new(2, round, 1, 0), &[]);
    let pop = |f: &Fake| -> Option<(u16, u32, Vec<u8>)> {
        let mut r = Record::default();
        match f.seg().s2g().pop(f.l.epoch, &mut r) {
            Pop::Record => Some((r.hdr.kind, r.hdr.peer, r.payload().to_vec())),
            Pop::Empty => None,
            other => panic!("unexpected {other:?}"),
        }
    };
    f.l.push_record(K_DEATH, 0, 2, &d(1));
    f.l.push_record(K_DEATH, 0, 2, &d(2));
    f.step();
    assert_eq!(pop(&f), Some((K_DEATH, 2, d(1))));
    assert_eq!(pop(&f), Some((K_DEATH, 2, d(2))));
    assert_eq!(pop(&f), None);
    // Oversized: dropped and counted, never a fragment.
    f.l.push_record(K_DEATH, 0, 2, &[0u8; 600]);
    f.step();
    assert_eq!(pop(&f), None);
    assert_eq!(f.seg().header.sidecar.counter(ctr::TOO_BIG), 1);
    // A stalled game: the ring fills (1024), the rest waits in process (8192), beyond
    // that records are dropped and a resync is requested.
    let total = 1024 + S2G_OVERFLOW + 10;
    for i in 0..total {
        f.l.push_record(K_DEATH, 0, 2, &d(i as u32));
        if i % 512 == 0 {
            f.step();
        }
    }
    f.step();
    assert!(f.seg().header.resync_req.load(Ordering::Acquire) >= 1);
    assert!(f.seg().header.sidecar.counter(ctr::OVERFLOW) >= 10);
    // Order is kept across the ring and the overflow.
    let mut last = -1i64;
    let mut n = 0;
    loop {
        while let Some((_, _, p)) = pop(&f) {
            let i = hsmp_ipc::record::view::<Death>(&p).unwrap().head.round as i64;
            assert!(i > last, "{i} after {last}");
            last = i;
            n += 1;
        }
        if f.l.lock().s2g.is_empty() {
            break;
        }
        f.step();
    }
    assert!(n >= 1024 + S2G_OVERFLOW - 600, "{n}");
}

#[test]
fn streams_without_a_negotiated_cap_are_dropped() {
    use hsmp_ipc::schema::{CAP_COMBAT, CAP_POSE, CAP_WORLD};
    // A game that holds WORLD and COMBAT back (the native module's phase gate).
    let f = fake_caps(CAPS_OFFERED & !CAP_WORLD & !CAP_COMBAT);
    assert!(f.l.has(CAP_POSE) && !f.l.has(CAP_WORLD) && !f.l.has(CAP_COMBAT));
    // No POSE: the hot streams are not read at all.
    let mut g = fake_caps(CAPS_OFFERED & !CAP_POSE);
    game_put(&g, "local_root", g.gmeta(), K_ROOT, bytemuck::bytes_of(&root_rec(1.0)));
    g.step();
    assert!(g.msgs().is_none());
}
