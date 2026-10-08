//! Fuzz (stable toolchain, proptest-driven): arbitrary bytes as a whole segment through every
//! reader. Nothing may panic or read out of bounds; a scribbled segment yields wrong data or
//! an error, never a fault.

use hsmp_ipc::handshake::check_prefix;
use hsmp_ipc::layout::Pod;
use hsmp_ipc::ring::Record;
use hsmp_ipc::schema::pose::{PeerDir, PeerPlay};
use hsmp_ipc::schema::RawSlot;
use hsmp_ipc::segment::Segment;
use hsmp_ipc::{Mapping, SEGMENT_SIZE};
use proptest::prelude::*;

/// Fill the mapping from a seed (xorshift; 4.6 MiB per case is too much for proptest to
/// generate directly), then overlay `patch` bytes at `at` for targeted corruption.
fn scribble(m: &Mapping, seed: u64, density: u8, patches: &[(usize, Vec<u8>)]) {
    let mut x = seed | 1;
    let p = m.as_ptr();
    for i in 0..SEGMENT_SIZE {
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        // density 255 = every byte random; lower = mostly zero (keeps protocols "almost valid").
        let b = if (x >> 56) as u8 <= density { x as u8 } else { 0 };
        // SAFETY: in bounds of the anonymous mapping; no other reference is live during fill.
        unsafe { *p.add(i) = b };
    }
    for (at, bytes) in patches {
        for (k, b) in bytes.iter().enumerate() {
            let i = (at + k) % SEGMENT_SIZE;
            // SAFETY: as above.
            unsafe { *p.add(i) = *b };
        }
    }
}

/// Every named record slot (ABI 2, `schema::SlotInfo`) through the generic byte reader and the
/// record validator: a scribbled slot is refused, never trusted.
fn record_slots(s: &Segment) {
    let (mut scratch, mut out) = (Vec::new(), Vec::new());
    for info in hsmp_ipc::schema::slots() {
        let Some(slot) = s.slot(info, 0) else { continue };
        let _ = slot.version();
        if let Some((_, kind, _)) = slot.get(&mut scratch, &mut out) {
            let _ = hsmp_ipc::schema::check_payload(kind, &out);
            let _ = hsmp_ipc::schema::check_payload(info.kind, &out);
        }
    }
}

fn read_everything(s: &Segment) {
    let _ = check_prefix(s, SEGMENT_SIZE);
    let _ = s.header.prefix.refuse_code();
    let _ = s.header.prefix.refuse_detail();
    let _ = s.header.game.state();
    let _ = s.header.sidecar.build_id();
    let _ = s.header.sidecar.hb_age_s(u64::MAX, 0);
    let _ = s.header.sidecar.hb_age_s(0, 1);
    let go = &s.game_out;
    let (mut sc, mut out) = (Vec::new(), Vec::new());
    for (slot, kind) in [(&go.local_root as &dyn RawSlot, 0x0110u16), (&go.local_weapon, 0x0111), (&go.local_pose, 0x0112)] {
        if slot.get(&mut sc, &mut out).is_some() {
            let _ = hsmp_ipc::schema::check_payload(kind, &out);
        }
    }
    let _ = go.pose_lead.read();
    // Typed record slots (every domain): read as bytes, validated as their record.
    for info in hsmp_ipc::schema::slots() {
        let peers = if matches!(info.form, hsmp_ipc::schema::SlotForm::PeerSlot | hsmp_ipc::schema::SlotForm::PeerBlob) { 4 } else { 1 };
        for p in 0..peers {
            if let Some(slot) = s.slot_ref(info.name, p) {
                let (mut scratch, mut out) = (Vec::new(), Vec::new());
                if let Some((_, kind, _)) = slot.get(&mut scratch, &mut out) {
                    let _ = hsmp_ipc::schema::check_payload(kind, &out);
                }
            }
        }
    }
    let mut dir = PeerDir::zeroed();
    if s.peers.dir.read_into(&mut dir).is_ok() {
        for e in dir.entries.iter() {
            let _ = e.nick();
        }
        let _ = dir.slot_of(7);
    }
    let mut pp = PeerPlay::zeroed();
    for p in s.peers.slots.iter() {
        let _ = p.play.read_into(&mut pp);
        if p.root.get(&mut sc, &mut out).is_some() {
            let _ = hsmp_ipc::schema::check_payload(0x0113, &out);
        }
    }
    record_slots(s);
    let mut rec = Record::zeroed();
    for _ in 0..4 {
        if s.g2s().pop(0, &mut rec) == hsmp_ipc::ring::Pop::Record {
            let _ = hsmp_ipc::schema::check_payload(rec.hdr.kind, rec.payload());
        }
        if s.s2g().pop(123, &mut rec) == hsmp_ipc::ring::Pop::Record {
            let _ = hsmp_ipc::schema::check_payload(rec.hdr.kind, rec.payload());
        }
        let _ = s.devctl().pop(0, &mut rec);
    }
    let t = s.s2g().tail();
    for q in t.saturating_sub(4)..t.saturating_add(2) {
        let _ = s.s2g().peek(q, &mut rec);
    }
    let _ = s.g2s().len();
    let mut bd = hsmp_ipc::schema::bus::BusDir::zeroed();
    if s.bus.dir.read_into(&mut bd).is_ok() {
        let _ = bd.find("puppets");
        for i in 0..70 {
            let _ = bd.name(i);
        }
    }
    for k in s.bus.keys.iter().take(8) {
        if let Some((_, kind, _)) = k.get(&mut sc, &mut out) {
            let _ = hsmp_ipc::schema::check_payload(kind, &out);
        }
    }
    // Writers on a scribbled segment must not fault either (they may refuse).
    let _ = s.state.world_owners.publish(&hsmp_ipc::segment::WorldOwnersBuf::new_boxed());
    let _ = s.s2g().push(1, 1, 0, 0, b"x");
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(48))]
    #[test]
    #[ignore = "slow: cargo test -- --ignored"]
    fn random_segment_never_faults(seed in any::<u64>(), density in prop_oneof![Just(255u8), Just(16u8), Just(1u8)],
                                   patches in prop::collection::vec((0..SEGMENT_SIZE, prop::collection::vec(any::<u8>(), 1..64)), 0..16)) {
        let m = Mapping::anonymous();
        scribble(&m, seed, density, &patches);
        read_everything(m.segment().unwrap());
    }
}

#[test]
#[ignore = "slow: cargo test -- --ignored"]
fn all_ones_segment_never_faults() {
    let m = Mapping::anonymous();
    for i in 0..SEGMENT_SIZE {
        // SAFETY: in bounds; no other reference live.
        unsafe { *m.as_ptr().add(i) = 0xff };
    }
    read_everything(m.segment().unwrap());
}
