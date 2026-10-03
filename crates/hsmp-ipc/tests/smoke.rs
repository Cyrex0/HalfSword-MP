//! API smoke tests (the API freeze). The full unit / property / stress suites live next to
//! them (IPC-CORE).


use hsmp_ipc::handshake::{attach_sidecar, check_prefix, init_game, GameOpen, SideParams};
use hsmp_ipc::ring::Pop;
use hsmp_ipc::schema::pose::PoseLead;
use hsmp_ipc::schema::{CAP_POSE, CAP_STATE};
use hsmp_ipc::seqlock::ReadError;
use hsmp_ipc::{Mapping, RefuseCode};

fn game(epoch: u64) -> SideParams<'static> {
    SideParams { pid: 4242, create_time: 0x1234, epoch, caps: CAP_POSE | CAP_STATE, build_id: "test-game" }
}

#[test]
fn init_attach_and_streams() {
    let m = Mapping::anonymous();
    let r = unsafe { init_game(m.segment_ptr(), m.len(), &game(7), 1_000_000) }.unwrap();
    assert_eq!(r, GameOpen::Fresh);
    let s = m.segment().unwrap();
    check_prefix(s, m.len()).unwrap();
    // Re-open by the same process.
    assert_eq!(unsafe { init_game(m.segment_ptr(), m.len(), &game(7), 1_000_000) }.unwrap(), GameOpen::Reopened);

    // Wrong parent.
    let sc = SideParams { pid: 1, create_time: 2, epoch: 99, caps: CAP_POSE, build_id: "sc" };
    let e = attach_sidecar(s, m.len(), 1111, None, &sc).unwrap_err();
    assert_eq!(e.code, RefuseCode::WrongParent);
    assert_eq!(s.header.prefix.refuse_code(), RefuseCode::WrongParent);
    let a = attach_sidecar(s, m.len(), 4242, Some(0x1234), &sc).unwrap();
    assert_eq!(a.caps_effective, CAP_POSE);
    assert_eq!(s.header.prefix.refuse_code(), RefuseCode::None);

    // Seqlock slot.
    let slot = &s.game_out.pose_lead;
    assert_eq!(slot.read().err(), Some(ReadError::Empty));
    let mut v = PoseLead::default();
    v.lead_ms = 5.0;
    slot.write(&v);
    let (got, seq) = slot.read().unwrap();
    assert_eq!(got.lead_ms, 5.0);
    assert_eq!(seq, 2);
    slot.begin_write_and_abandon();
    assert_eq!(slot.read().err(), Some(ReadError::Busy));
    slot.write(&v);
    assert!(slot.read().is_ok());

    // Triple buffer of a record blob (world_owners).
    use hsmp_ipc::schema::world::{OwnerRec, OwnersHead};
    let tb = &s.state.world_owners;
    let mut out = hsmp_ipc::segment::WorldOwnersBuf::new_boxed();
    assert_eq!(tb.take_into(&mut out), None);
    let mut b = hsmp_ipc::segment::WorldOwnersBuf::new_boxed();
    let head = OwnersHead { level: 3, epoch: 1, manifest_len: 0, n: 0, sync: hsmp_ipc::layout::Bool::TRUE, _r: 0 };
    assert!(b.set_payload(0x0413, &hsmp_ipc::record::to_payload(&head, &[OwnerRec::new(7, 3, 1, 1)])));
    assert_eq!(tb.publish(&b), Some(1));
    assert_eq!(tb.publish(&b), Some(2));
    assert_eq!(tb.take_into(&mut out), Some(2));
    assert_eq!(tb.take_into(&mut out), None);
    assert_eq!(tb.published_gen(), 2);
    let back = hsmp_ipc::record::view::<OwnersHead>(out.payload()).unwrap();
    assert_eq!((back.head.level, back.rows[0].owner), (3, 3));

    // Ring.
    let ring = s.g2s();
    let mut rec = Default::default();
    assert_eq!(ring.pop(99, &mut rec), Pop::Empty);
    ring.push(7, 0x0280, 0, 1, b"hello").unwrap();
    ring.push(6, 0x0280, 0, 2, b"old").unwrap();
    assert_eq!(ring.pop(7, &mut rec), Pop::Record);
    assert_eq!(rec.payload(), b"hello");
    assert_eq!(ring.pop(7, &mut rec), Pop::StaleEpoch);
    assert_eq!(ring.push(7, 1, 0, 0, &[0u8; 481]), Err(hsmp_ipc::ring::PushError::TooBig));
}

#[test]
fn generators_produce_output() {
    let h = hsmp_ipc::gen::c_header();
    assert!(h.contains("HSMP_IPC_LAYOUT_HASH"));
    assert!(h.contains("typedef struct hsmp_PoseHead"));
    let l = hsmp_ipc::gen::lua_schema();
    assert!(l.contains("local_pose"));
    assert!(l.ends_with("return S\n"));
}

#[cfg(windows)]
#[test]
fn named_mapping_create_open() {
    use hsmp_ipc::shm::{self, Access};
    let pid = shm::current_pid();
    let ct = shm::process_create_time(pid).unwrap();
    let name = format!("{}.smoke{}", shm::mapping_name(pid, ct), shm::random_u64());
    let (g, existed) = Mapping::create(&name).unwrap();
    assert!(!existed);
    let p = SideParams { pid, create_time: ct, epoch: shm::random_u64(), caps: CAP_POSE, build_id: "g" };
    unsafe { init_game(g.segment_ptr(), g.len(), &p, shm::qpc_freq()) }.unwrap();
    let sc = Mapping::open(&name, Access::ReadWrite).unwrap();
    let s = sc.segment().unwrap();
    let sp = SideParams { pid, create_time: ct, epoch: 5, caps: CAP_POSE, build_id: "s" };
    attach_sidecar(s, sc.len(), pid, Some(ct), &sp).unwrap();
    let ro = Mapping::open(&name, Access::ReadOnly).unwrap();
    check_prefix(ro.segment().unwrap(), ro.len()).unwrap();
    let (gb, _) = shm::doorbell_names(&name);
    let bell = shm::Doorbell::create(&gb).unwrap();
    let bell2 = shm::Doorbell::open(&gb).unwrap();
    bell.ring();
    assert!(bell2.wait(100));
    assert!(!bell2.wait(1));
    assert!(Mapping::open("Local\\HSMP.ipc.does.not.exist", Access::ReadOnly).is_err());
}
