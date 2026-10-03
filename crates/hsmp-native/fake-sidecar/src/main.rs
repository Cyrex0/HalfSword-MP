//! `hsmp-fake-sidecar`: test-only stand-in for the sidecar's shared-memory side, used by the
//! HSMPNative harness (crates/hsmp-native/cpp/test). Not shipped.
//!
//! usage: hsmp-fake-sidecar --ipc shm:<name> --parent-pid <pid> [--epoch <hex>] [--peers N] [--run-ms N]
//!
//! Attaches like the real sidecar, then at 1 kHz: heartbeat, the peer directory (N peers),
//! a fresh `PeerPlay` + `PeerRoot` per peer, the session blob every 250 ms, an S2G `notice`
//! every 100 ms, and every G2S record echoed back as an S2G `cmd_result` with the same
//! `req_id` and `{echo = <payload>, kind = <name>}`. Exits when the parent dies, after
//! `--run-ms`, or with the refusal's exit code.

use std::time::{Duration, Instant};

use hsmp_ipc::handshake::{attach_sidecar, SideParams};
use hsmp_ipc::layout::boxed_zeroed;
use hsmp_ipc::ring::{Pop, Record};
use hsmp_ipc::schema::pose::{PeerDir, PeerPlay, PeerRoot, PEER_PLAY_HAS_CONTROL, PEER_PLAY_HAS_ROOT, PEER_PLAY_V2};
use hsmp_ipc::schema::{self as sc, SlotMeta};
use hsmp_ipc::shm::{self, Access, Mapping, ProcessHandle};

fn arg(args: &[String], k: &str) -> Option<String> {
    args.iter().position(|a| a == k).and_then(|i| args.get(i + 1).cloned())
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let Some(name) = arg(&args, "--ipc").as_deref().and_then(shm::parse_ipc_arg).map(str::to_string) else {
        eprintln!("usage: hsmp-fake-sidecar --ipc shm:<name> --parent-pid <pid> [--epoch hex] [--peers N] [--run-ms N]");
        std::process::exit(2);
    };
    let parent: u32 = arg(&args, "--parent-pid").and_then(|v| v.parse().ok()).unwrap_or(0);
    let epoch = arg(&args, "--epoch").and_then(|v| u64::from_str_radix(&v, 16).ok()).unwrap_or_else(shm::random_u64);
    let peers: usize = arg(&args, "--peers").and_then(|v| v.parse().ok()).unwrap_or(7).min(32);
    let run_ms: u64 = arg(&args, "--run-ms").and_then(|v| v.parse().ok()).unwrap_or(120_000);

    let map = match Mapping::open(&name, Access::ReadWrite) {
        Ok(m) => m,
        Err(e) => {
            eprintln!("fake-sidecar: open {}: {}", name, e);
            std::process::exit(3);
        }
    };
    let s = map.segment().expect("segment");
    let pid = shm::current_pid();
    let p = SideParams { pid, create_time: shm::process_create_time(pid).unwrap_or(0), epoch, caps: u64::MAX, build_id: "fake-sidecar" };
    let parent_ct = shm::process_create_time(parent).ok();
    if let Err(r) = attach_sidecar(s, map.len(), parent, parent_ct, &p) {
        eprintln!("fake-sidecar: refused: {}", r);
        std::process::exit(r.code.exit_code());
    }
    let parent_h = ProcessHandle::open(parent).ok();
    let meta = |seq: u32| SlotMeta { writer_epoch: epoch, valid: 1, sample_seq: seq, ..Default::default() };

    let mut dir = boxed_zeroed::<PeerDir>();
    dir.meta = meta(0);
    dir.count = peers as u32;
    for i in 0..peers {
        let e = &mut dir.entries[i];
        e.active = 1;
        e.peer_id = 100 + i as u32;
        e.gen = 1;
        e.set_nick(&format!("peer{}", i));
    }
    s.peers.dir.write(&dir);

    let t0 = Instant::now();
    let mut play = boxed_zeroed::<PeerPlay>();
    let mut scratch: Vec<u64> = Vec::new();
    let mut rec = Record::default();
    let (mut tick, mut notices) = (0u64, 0i64);
    let (mut next_session, mut next_notice, mut next_parent) = (Duration::ZERO, Duration::ZERO, Duration::ZERO);
    loop {
        let el = t0.elapsed();
        if el.as_millis() as u64 > run_ms {
            break;
        }
        if el >= next_parent {
            next_parent = el + Duration::from_millis(250);
            if parent_h.as_ref().is_some_and(|h| !h.is_alive()) {
                break;
            }
        }
        tick += 1;
        s.header.sidecar.beat(shm::qpc());
        for i in 0..peers {
            play.meta = meta(tick as u32);
            play.peer_id = 100 + i as u32;
            play.play_seq = tick;
            play.mode = 0;
            play.pt = el.as_secs_f64() * 1000.0;
            play.flags = PEER_PLAY_V2 | PEER_PLAY_HAS_ROOT | if tick % 2 == 0 { PEER_PLAY_HAS_CONTROL } else { 0 };
            play.mask = (1 << 25) - 1;
            play.vmask = play.mask;
            play.root = [i as f32, 0.0, 0.0, 0.0];
            for b in 0..25 {
                for j in 0..13 {
                    play.b[b][j] = (tick as f32) + (b * 13 + j) as f32;
                }
            }
            play.w[0].present = 1;
            play.w[0].hands = 1;
            s.peers.slots[i].play.write(&play);
            let root = hsmp_ipc::schema::pose::Root { tick: tick as u32, pos: [i as f32, 0.0, 0.0], rot: [0.0, 0.0, 0.0, 1.0], ..Default::default() };
            let r = PeerRoot { peer_id: 100 + i as u32, _r: 0, root };
            hsmp_ipc::schema::RawSlot::put(&s.peers.slots[i].root, meta(tick as u32), hsmp_ipc::schema::pose::K_PEER_ROOT, bytemuck::bytes_of(&r), &mut scratch);
        }
        if el >= next_session {
            next_session = el + Duration::from_millis(250);
            // The sidecar's link view (a record slot; republished at 4 Hz here as a load test).
            let link = sc::session::Link { my_peer_id: 1, status: sc::session::sidecar_status::CONNECTED,
                state: sc::session::link_state::UP, attempt: tick as u32, ..Default::default() };
            if let Some(slot) = s.slot_ref("link", 0) {
                slot.put(meta(tick as u32), sc::session::K_LINK, &hsmp_ipc::record::to_payload(&link, &[]), &mut scratch);
            }
        }
        if el >= next_notice {
            next_notice = el + Duration::from_millis(100);
            notices += 1;
            let n = sc::session::Notice { event_id: notices as u32, code: sc::session::notice::PLAYER_JOINED, ..Default::default() };
            let _ = s.s2g().push(epoch, sc::session::K_NOTICE, 0, 0, &hsmp_ipc::record::to_payload(&n, &[]));
        }
        loop {
            match s.g2s().pop(0, &mut rec) {
                Pop::Empty => break,
                Pop::Record => {
                    let kind = sc::record_info(rec.hdr.kind).map_or("?", |r| r.name);
                    // A typed cmd_result echoing "<kind>:<text>" (a validated `chat` record's
                    // text; other kinds echo "<kind>:").
                    let text = if rec.hdr.kind == sc::session::K_CHAT {
                        hsmp_ipc::record::view::<sc::session::Chat>(rec.payload()).ok().and_then(|v| v.head().text.as_str().map(str::to_string))
                    } else {
                        None
                    };
                    let r = sc::session::CmdResult { cmd_id: rec.hdr.req_id.max(1) as u32, ok: true.into(),
                        reason_text: hsmp_ipc::layout::Str::new(&format!("{}:{}", kind, text.unwrap_or_default())), ..Default::default() };
                    let _ = s.s2g().push(epoch, sc::session::K_CMD_RESULT, 0, rec.hdr.req_id, &hsmp_ipc::record::to_payload(&r, &[]));
                }
                _ => {}
            }
        }
        std::thread::sleep(Duration::from_millis(1));
    }
    hsmp_ipc::handshake::detach_sidecar(s);
}
