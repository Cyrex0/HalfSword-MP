//! Micro-benchmarks (ignored by default):
//! `cargo test --release -p hsmp-ipc --all-features --test bench -- --ignored --nocapture`

use std::time::Instant;

use hsmp_ipc::layout::Pod;
use hsmp_ipc::ring::Record;
use hsmp_ipc::schema::pose::{PeerPlay, PoseBuf};
use hsmp_ipc::schema::Stamped;
use hsmp_ipc::Mapping;

fn time<F: FnMut()>(name: &str, n: u32, mut f: F) -> f64 {
    for _ in 0..n / 10 {
        f();
    }
    let t = Instant::now();
    for _ in 0..n {
        f();
    }
    let us = t.elapsed().as_secs_f64() * 1e6 / n as f64;
    println!("{:<40} {:>9.3} us/op", name, us);
    us
}

#[test]
#[ignore]
fn bench_primitives() {
    let m = Mapping::anonymous();
    let s = m.segment().unwrap();
    let mut pose = Stamped::<PoseBuf>::zeroed();
    pose.len = 380;
    time("SeqSlot<Stamped<PoseBuf>> write (0.7 KiB)", 200_000, || s.game_out.local_pose.write(&pose));
    let mut out = Stamped::<PoseBuf>::zeroed();
    time("SeqSlot<Stamped<PoseBuf>> read", 200_000, || {
        let _ = s.game_out.local_pose.read_into(&mut out);
    });
    let play = PeerPlay::zeroed();
    s.peers.slots[0].play.write(&play);
    let mut pout = PeerPlay::zeroed();
    time("SeqSlot<PeerPlay> read (1.6 KiB)", 200_000, || {
        let _ = s.peers.slots[0].play.read_into(&mut pout);
    });
    time("SeqSlot unchanged check (one load)", 1_000_000, || {
        let _ = s.peers.slots[0].play.seq();
    });
    let payload = [7u8; 64];
    let mut rec = Record::zeroed();
    time("Ring push+pop (64 B payload)", 500_000, || {
        s.g2s().push(1, 0x0280, 0, 0, &payload).unwrap();
        let _ = s.g2s().pop(1, &mut rec);
    });
    let mut cb = hsmp_ipc::segment::WorldOwnersBuf::new_boxed();
    let rows: Vec<hsmp_ipc::schema::world::OwnerRec> = (1..=1000).map(|i| hsmp_ipc::schema::world::OwnerRec::new(i, 0, i, 0)).collect();
    let head = hsmp_ipc::schema::world::OwnersHead { level: 1, epoch: 1, manifest_len: 0, n: 0, sync: hsmp_ipc::layout::Bool::TRUE, _r: 0 };
    cb.set_payload(0x0413, &hsmp_ipc::record::to_payload(&head, &rows));
    let mut cout = hsmp_ipc::segment::WorldOwnersBuf::new_boxed();
    time("TripleBuf<world_owners 32 KiB> publish+take", 20_000, || {
        s.state.world_owners.publish(&cb).unwrap();
        let _ = s.state.world_owners.take_into(&mut cout);
    });
    time("TripleBuf unchanged check (one load)", 1_000_000, || {
        let _ = s.state.world_owners.published_gen();
    });
}
