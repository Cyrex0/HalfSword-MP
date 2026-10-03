//! Header and handshake: every refusal path, reopen, poison, attach semantics.

use std::sync::atomic::Ordering;

use hsmp_ipc::handshake::{attach_sidecar, check_prefix, debug_corrupt_prefix, detach_sidecar, init_game, poison, GameOpen, SideParams};
use hsmp_ipc::header::{init_state, SideState};
use hsmp_ipc::schema::{CAP_POSE, CAP_QUEUES, CAP_STATE};
use hsmp_ipc::{Mapping, RefuseCode, SEGMENT_SIZE};

const GPID: u32 = 4242;
const GCT: u64 = 0xABCD;

fn game() -> SideParams<'static> {
    SideParams { pid: GPID, create_time: GCT, epoch: 11, caps: CAP_POSE | CAP_STATE | CAP_QUEUES, build_id: "game-build" }
}
fn sidecar(epoch: u64) -> SideParams<'static> {
    SideParams { pid: 99, create_time: 5, epoch, caps: CAP_POSE | CAP_QUEUES, build_id: "sidecar-build" }
}
fn fresh() -> Mapping {
    let m = Mapping::anonymous();
    assert_eq!(unsafe { init_game(m.segment_ptr(), m.len(), &game(), 10_000_000) }.unwrap(), GameOpen::Fresh);
    m
}

#[test]
fn not_ready_before_init() {
    let m = Mapping::anonymous();
    let s = m.segment().unwrap();
    assert_eq!(check_prefix(s, m.len()).unwrap_err().code, RefuseCode::NotReady);
    assert_eq!(attach_sidecar(s, m.len(), GPID, None, &sidecar(1)).unwrap_err().code, RefuseCode::NotReady);
}

#[test]
fn init_fields_and_reopen() {
    let m = fresh();
    let s = m.segment().unwrap();
    let h = &s.header;
    assert_eq!(h.prefix.init_state.load(Ordering::Acquire), init_state::READY);
    assert_eq!(h.game.pid.load(Ordering::Relaxed), GPID);
    assert_eq!(h.game.epoch.load(Ordering::Relaxed), 11);
    assert_eq!(h.game.state(), Some(SideState::Ready));
    assert_eq!(h.game.build_id(), "game-build");
    assert_eq!(h.world_epoch.load(Ordering::Relaxed), 1);
    assert_eq!(h.qpc_freq.load(Ordering::Relaxed), 10_000_000);
    assert!(s.state.world_remote.indices_valid());
    let gen0 = h.game_lua_gen.load(Ordering::Relaxed);
    assert_eq!(unsafe { init_game(m.segment_ptr(), m.len(), &game(), 1) }.unwrap(), GameOpen::Reopened);
    assert_eq!(h.game_lua_gen.load(Ordering::Relaxed), gen0 + 1);
    // Another process (different pid / create time) may not adopt it.
    let other = SideParams { pid: 1, ..game() };
    assert_eq!(unsafe { init_game(m.segment_ptr(), m.len(), &other, 1) }.unwrap_err().code, RefuseCode::WrongParent);
    let other = SideParams { create_time: 1, ..game() };
    assert_eq!(unsafe { init_game(m.segment_ptr(), m.len(), &other, 1) }.unwrap_err().code, RefuseCode::WrongParent);
}

#[test]
fn small_mapping_refused() {
    let m = Mapping::anonymous();
    assert_eq!(unsafe { init_game(m.segment_ptr(), SEGMENT_SIZE - 4096, &game(), 1) }.unwrap_err().code, RefuseCode::SizeMismatch);
    let m = fresh();
    assert_eq!(check_prefix(m.segment().unwrap(), SEGMENT_SIZE - 1).unwrap_err().code, RefuseCode::SizeMismatch);
}

#[test]
fn bad_magic_is_not_written_back() {
    let m = fresh();
    unsafe { debug_corrupt_prefix(m.segment_ptr(), Some(*b"NOTHSMP\0"), None, None, None) };
    let s = m.segment().unwrap();
    let e = attach_sidecar(s, m.len(), GPID, None, &sidecar(1)).unwrap_err();
    assert_eq!(e.code, RefuseCode::BadMagic);
    assert_eq!(e.code.exit_code(), 70);
    assert_eq!(s.header.prefix.refuse_code(), RefuseCode::None, "never write into foreign memory");
}

#[test]
fn abi_and_hash_mismatch() {
    let m = fresh();
    unsafe { debug_corrupt_prefix(m.segment_ptr(), None, Some(hsmp_ipc::ABI_MAJOR + 1), None, None) };
    let s = m.segment().unwrap();
    let e = attach_sidecar(s, m.len(), GPID, None, &sidecar(1)).unwrap_err();
    assert_eq!(e.code, RefuseCode::AbiMismatch);
    assert_eq!(e.code.exit_code(), 71);
    assert_eq!(s.header.prefix.refuse_code(), RefuseCode::AbiMismatch);
    assert!(s.header.prefix.refuse_detail().contains("hash"), "{}", s.header.prefix.refuse_detail());

    let m = fresh();
    unsafe { debug_corrupt_prefix(m.segment_ptr(), None, None, Some(hsmp_ipc::LAYOUT_HASH ^ 1), None) };
    assert_eq!(attach_sidecar(m.segment().unwrap(), m.len(), GPID, None, &sidecar(1)).unwrap_err().code, RefuseCode::AbiMismatch);

    let m = fresh();
    unsafe { debug_corrupt_prefix(m.segment_ptr(), None, None, None, Some(4096)) };
    assert_eq!(check_prefix(m.segment().unwrap(), m.len()).unwrap_err().code, RefuseCode::SizeMismatch);
}

#[test]
fn wrong_parent_pid_and_create_time() {
    let m = fresh();
    let s = m.segment().unwrap();
    let e = attach_sidecar(s, m.len(), GPID + 1, None, &sidecar(1)).unwrap_err();
    assert_eq!(e.code, RefuseCode::WrongParent);
    assert_eq!(e.code.exit_code(), 72);
    let e = attach_sidecar(s, m.len(), GPID, Some(GCT + 1), &sidecar(1)).unwrap_err();
    assert_eq!(e.code, RefuseCode::WrongParent);
    assert_eq!(s.header.sidecar.epoch.load(Ordering::Relaxed), 0, "a refused sidecar publishes nothing");
}

#[test]
fn attach_reattach_and_ring_reset() {
    let m = fresh();
    let s = m.segment().unwrap();
    // Game queued commands before any sidecar: the first attach drops them.
    s.g2s().push(11, 0x0280, 0, 1, b"stale").unwrap();
    let a = attach_sidecar(s, m.len(), GPID, Some(GCT), &sidecar(21)).unwrap();
    assert_eq!(a.caps_effective, CAP_POSE | CAP_QUEUES);
    assert_eq!(a.game_epoch, 11);
    assert_eq!(a.previous_sidecar_epoch, 0);
    assert_eq!(a.attach_count, 1);
    assert!(s.g2s().is_empty());
    assert_eq!(s.header.caps_effective.load(Ordering::Relaxed), CAP_POSE | CAP_QUEUES);
    assert_eq!(s.header.sidecar.state(), Some(SideState::Ready));
    assert_eq!(s.header.sidecar.build_id(), "sidecar-build");
    // Sidecar restart.
    s.g2s().push(11, 0x0280, 0, 2, b"to old sidecar").unwrap();
    let a2 = attach_sidecar(s, m.len(), GPID, Some(GCT), &sidecar(22)).unwrap();
    assert_eq!(a2.previous_sidecar_epoch, 21);
    assert_eq!(a2.attach_count, 2);
    assert!(s.g2s().is_empty());
    // A previous refusal is cleared by a good attach.
    let _ = attach_sidecar(s, m.len(), 1, None, &sidecar(23));
    assert_eq!(s.header.prefix.refuse_code(), RefuseCode::WrongParent);
    attach_sidecar(s, m.len(), GPID, None, &sidecar(24)).unwrap();
    assert_eq!(s.header.prefix.refuse_code(), RefuseCode::None);
    detach_sidecar(s);
    assert_eq!(s.header.sidecar.state(), Some(SideState::Closing));
}

#[test]
fn poisoned_refuses() {
    let m = fresh();
    let s = m.segment().unwrap();
    poison(s, "panic in put_pose");
    let e = attach_sidecar(s, m.len(), GPID, None, &sidecar(1)).unwrap_err();
    assert_eq!(e.code, RefuseCode::Poisoned);
    assert_eq!(s.header.prefix.refuse_detail(), "panic in put_pose");
    assert_eq!(unsafe { init_game(m.segment_ptr(), m.len(), &game(), 1) }.unwrap_err().code, RefuseCode::NotReady);
}

#[test]
fn refuse_codes_round_trip() {
    for c in hsmp_ipc::header::REFUSE_CODES {
        assert_eq!(RefuseCode::from_u32(*c as u32), Some(*c));
        if *c != RefuseCode::None {
            assert!((70..=78).contains(&c.exit_code()));
        }
    }
    assert_eq!(RefuseCode::from_u32(999), None);
}

#[test]
fn heartbeat_ages_and_counters() {
    let m = fresh();
    let h = &m.segment().unwrap().header;
    assert_eq!(h.game.hb_age_s(100, 10), None);
    h.game.beat(50);
    assert_eq!(h.game.hb_age_s(150, 100), Some(1.0));
    h.game.count(hsmp_ipc::header::ctr::RING_FULL, 3);
    h.game.high_water(hsmp_ipc::header::ctr::RING_HIGH_WATER, 7);
    h.game.high_water(hsmp_ipc::header::ctr::RING_HIGH_WATER, 5);
    assert_eq!(h.game.counter(hsmp_ipc::header::ctr::RING_FULL), 3);
    assert_eq!(h.game.counter(hsmp_ipc::header::ctr::RING_HIGH_WATER), 7);
    h.game.count(999, 1); // out of range: ignored
}

#[cfg(windows)]
#[test]
fn named_mapping_reopen_same_process_and_owner() {
    use hsmp_ipc::shm::{self, Access};
    let pid = shm::current_pid();
    let ct = shm::process_create_time(pid).unwrap();
    let name = format!("{}.hs{}", shm::mapping_name(pid, ct), shm::random_u64());
    let (a, existed) = Mapping::create(&name).unwrap();
    assert!(!existed);
    let p = SideParams { pid, create_time: ct, epoch: shm::random_u64(), caps: 0, build_id: "" };
    assert_eq!(unsafe { init_game(a.segment_ptr(), a.len(), &p, shm::qpc_freq()) }.unwrap(), GameOpen::Fresh);
    let (b, existed) = Mapping::create(&name).unwrap();
    assert!(existed);
    assert_eq!(unsafe { init_game(b.segment_ptr(), b.len(), &p, shm::qpc_freq()) }.unwrap(), GameOpen::Reopened);
    assert!(a.len() >= SEGMENT_SIZE);
    // Read-only views see the same data.
    let ro = Mapping::open(&name, Access::ReadOnly).unwrap();
    assert_eq!(ro.segment().unwrap().header.game.pid.load(Ordering::Relaxed), pid);
    assert_eq!(shm::parse_ipc_arg(&format!("shm:{}", name)), Some(name.as_str()));
    assert_eq!(shm::parse_ipc_arg("file"), None);
    assert_eq!(shm::parse_ipc_arg("shm:"), None);
    assert!(shm::random_u64() != shm::random_u64());
    assert!(shm::ProcessHandle::open(pid).unwrap().is_alive());
}
