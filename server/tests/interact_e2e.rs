//! Interaction channel end to end (docs/development/subsystems/interact.md): a real
//! hsmp-server and two real hsmp-sidecars on localhost, driven only through the two games'
//! shared segments with the typed `interact` record (what HSMPInteract sends / polls):
//!
//!   A: G2S `interact` record  ->  C2S `interact` (wire id patched in place)  ->  server
//!   validation  ->  S2C `interact` (peer = initiator)  ->  B: S2G `interact` (peer copied)
//!   (and denials / server ends back to A with peer 0 and A's own grab id in `gid`)
//!
//! Cases: impulse relay, impulse clamp, grab start/update/end, server lease expiry
//! (GRAB_END to both), denial of a grab on a peer that is not connected, no relay of a
//! self-targeted / off-bone event, and the delivery latency on localhost.
#![cfg(windows)]

mod common;
use common::*;

use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

use hsmp_ipc::record::view;
use hsmp_ipc::ring::{Pop, Record};
use hsmp_ipc::schema::interact::{kind as K, Interact, HERO_BONES, K_INTERACT};

fn wait_for(what: &str, timeout: Duration, mut f: impl FnMut() -> bool) {
    let t0 = Instant::now();
    while t0.elapsed() < timeout {
        if f() {
            return;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    panic!("timed out waiting for {what}");
}

/// Strip ANSI colour sequences (tracing's default formatter).
fn nocolor(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut it = s.chars().peekable();
    while let Some(c) = it.next() {
        if c == '\x1b' {
            for d in it.by_ref() {
                if d.is_ascii_alphabetic() {
                    break;
                }
            }
        } else {
            out.push(c);
        }
    }
    out
}

fn bone(name: &str) -> u8 {
    HERO_BONES.iter().position(|b| *b == name).unwrap() as u8
}

/// A game's S2G `interact` records (peer, record), drained from its ring as they come.
struct Inbox<'a> {
    g: &'a Game,
    got: Vec<(u32, Interact)>,
}

impl<'a> Inbox<'a> {
    fn new(g: &'a Game) -> Self {
        Inbox { g, got: Vec::new() }
    }
    fn drain(&mut self) {
        let s = self.g.seg();
        let sc = s.header.sidecar.epoch.load(Ordering::Acquire);
        let mut rec = Record::default();
        loop {
            match s.s2g().pop(sc, &mut rec) {
                Pop::Empty => break,
                Pop::Record if rec.hdr.kind == K_INTERACT => {
                    let e = view::<Interact>(rec.payload()).expect("the sidecar pushes valid records").head();
                    self.got.push((rec.hdr.peer, e));
                }
                _ => {}
            }
        }
    }
    /// Every record so far from `from` matching `f`.
    fn find(&mut self, from: u32, f: impl Fn(&Interact) -> bool) -> Vec<Interact> {
        self.drain();
        self.got.iter().filter(|(p, e)| *p == from && f(e)).map(|(_, e)| *e).collect()
    }
}

#[test]
fn interaction_channel_relays_validates_and_expires() {
    let root = std::env::temp_dir().join(format!("hsmp_interact_e2e_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    let (_sv, addr) = server(&root);
    let (ga, gb) = (Game::new("ia"), Game::new("ib"));
    let me = hsmp_ipc::shm::current_pid();
    let side = |g: &Game, nick: &str| {
        spawn_logged(env!("CARGO_BIN_EXE_hsmp-sidecar"), &g.sidecar_args(&addr, nick, me), &g.dir, &g.dir.join("sidecar.log"))
    };
    let _a = side(&ga, "Alpha");
    assert!(ga.wait_connected(1, 10), "A never connected as peer 1");
    let _b = side(&gb, "Bravo");
    assert!(gb.wait_connected(2, 10), "B never connected as peer 2");
    // INTERACT (bit 2) was negotiated on both connections.
    for g in [&ga, &gb] {
        let log = g.dir.join("sidecar.log");
        let l = nocolor(&std::fs::read_to_string(&log).unwrap_or_default());
        let line = l
            .lines()
            .find(|x| x.contains("v5 connected"))
            .unwrap_or_else(|| panic!("{}: no connect line:\n{l}", log.display()));
        let caps: u64 = line
            .split("caps=")
            .nth(1)
            .and_then(|s| s.split_whitespace().next())
            .and_then(|s| s.parse().ok())
            .unwrap_or_else(|| panic!("no caps in {line}"));
        assert!(caps & 4 != 0, "INTERACT not negotiated: {line}");
    }
    let mut req = 0u64;
    let mut out = |e: Interact| {
        req += 1;
        ga.seg().g2s().push(ga.epoch, K_INTERACT, 0, req, hsmp_ipc::bytemuck::bytes_of(&e)).expect("g2s push");
    };
    let (mut ia, mut ib) = (Inbox::new(&ga), Inbox::new(&gb));
    let ev = |kind: u8, target: u32, gid: u32, hand: u8, b: &str, pt: [f32; 3], v: [f32; 3], ts: u32| Interact {
        kind, target_peer: target, gid, hand, bone: bone(b), point: pt, vector: v, ts, ..Default::default()
    };

    // 1. Impulse: relayed to the owner of the body, with the contact data.
    let t0 = Instant::now();
    out(ev(K::IMPULSE, 2, 0, 0, "Spine_02", [0.0, 0.0, 5.0], [3000.0, 0.0, 0.0], 1000));
    wait_for("impulse at B", Duration::from_secs(5), || !ib.find(1, |e| e.kind == K::IMPULSE).is_empty());
    let latency = t0.elapsed();
    let e = ib.find(1, |e| e.kind == K::IMPULSE)[0];
    assert_eq!((e.bone, e.point, e.vector[0], e.target_peer), (bone("Spine_02"), [0.0, 0.0, 5.0], 3000.0, 2), "{e:?}");
    assert!(e.id != 0, "the sidecar minted an impulse sequence: {e:?}");
    eprintln!("impulse localhost delivery (segment -> server -> segment): {latency:?}");
    assert!(latency < Duration::from_millis(500), "impulse took {latency:?}");

    // 2. A huge impulse is clamped by the server (never launches a body).
    out(ev(K::IMPULSE, 2, 0, 0, "Pelvis", [0.0; 3], [90000.0, 0.0, 0.0], 1010));
    wait_for("clamped impulse at B", Duration::from_secs(5), || {
        !ib.find(1, |e| e.kind == K::IMPULSE && e.bone == bone("Pelvis")).is_empty()
    });
    let e = ib.find(1, |e| e.bone == bone("Pelvis"))[0];
    assert!(e.vector[0] <= 15000.0 + 0.5, "{e:?}");

    // 3. Grab start / update / end.
    out(ev(K::GRAB_START, 2, 1, 0, "Lowerarm_L", [2.0, 0.0, 1.0], [10.0, 20.0, 140.0], 1100));
    wait_for("grab_start at B", Duration::from_secs(5), || !ib.find(1, |e| e.kind == K::GRAB_START).is_empty());
    let e = ib.find(1, |e| e.kind == K::GRAB_START)[0];
    assert!(e.bone == bone("Lowerarm_L") && e.hand == 0 && e.vector == [10.0, 20.0, 140.0], "{e:?}");
    assert_eq!(e.gid, 0, "the initiator's Lua grab id stays with the initiator");
    let wire_id = e.id;
    assert!(wire_id != 0);
    out(ev(K::GRAB_UPDATE, 2, 1, 0, "Lowerarm_L", [2.0, 0.0, 1.0], [15.0, 20.0, 140.0], 1200));
    wait_for("grab_update at B", Duration::from_secs(5), || !ib.find(1, |e| e.kind == K::GRAB_UPDATE).is_empty());
    assert_eq!(ib.find(1, |e| e.kind == K::GRAB_UPDATE)[0].id, wire_id);
    out(ev(K::GRAB_END, 2, 1, 0, "Pelvis", [0.0; 3], [0.0; 3], 1300));
    wait_for("grab_end at B", Duration::from_secs(5), || !ib.find(1, |e| e.kind == K::GRAB_END).is_empty());
    assert_eq!(ib.find(1, |e| e.kind == K::GRAB_END)[0].id, wire_id);

    // 4. A grab that is never refreshed is ended by the server lease on both sides; A gets
    //    its own grab id back in `gid`.
    out(ev(K::GRAB_START, 2, 2, 1, "Hand_R", [0.0; 3], [0.0; 3], 1400));
    wait_for("second grab at B", Duration::from_secs(5), || ib.find(1, |e| e.kind == K::GRAB_START).len() == 2);
    let t_start = Instant::now();
    wait_for("lease end at B", Duration::from_secs(5), || ib.find(1, |e| e.kind == K::GRAB_END).len() == 2);
    wait_for("lease end at A", Duration::from_secs(5), || {
        !ia.find(0, |e| e.kind == K::GRAB_END && e.gid == 2 && e.hand == 1).is_empty()
    });
    let lease = t_start.elapsed();
    assert!(lease >= Duration::from_millis(800), "lease ended after {lease:?}");

    // 5. A grab on a peer that is not connected is denied back to the grabber.
    out(ev(K::GRAB_START, 9, 3, 0, "Head", [0.0; 3], [0.0; 3], 1500));
    wait_for("denial at A", Duration::from_secs(5), || {
        !ia.find(0, |e| e.kind == K::GRAB_DENIED && e.gid == 3 && e.target_peer == 9).is_empty()
    });
    // ... and a late update of the denied grab is not sent (it would revive it).
    out(ev(K::GRAB_UPDATE, 9, 3, 0, "Head", [0.0; 3], [0.0; 3], 1510));

    // 6. Self-targeted / off-bone events go nowhere: after a marker event arrives, B has
    //    seen nothing from them.
    out(ev(K::IMPULSE, 1, 0, 0, "Head", [0.0; 3], [500.0, 0.0, 0.0], 1600));
    out(ev(K::IMPULSE, 2, 0, 0, "Head", [500.0, 0.0, 0.0], [500.0, 0.0, 0.0], 1601));
    out(ev(K::IMPULSE, 2, 0, 0, "Foot_R", [0.0; 3], [123.0, 0.0, 0.0], 1602));
    wait_for("marker impulse at B", Duration::from_secs(5), || !ib.find(1, |e| e.bone == bone("Foot_R")).is_empty());
    assert!(ib.find(1, |e| e.bone == bone("Head")).is_empty(), "self / off-bone events were relayed");
    // A sees nothing about its own impulses, and exactly one denial.
    assert!(ia.find(1, |_| true).is_empty() && ia.find(2, |_| true).is_empty());
    assert_eq!(ia.find(0, |e| e.kind == K::GRAB_DENIED).len(), 1);

    let sv = std::fs::read_to_string(root.join("server.log")).unwrap_or_default();
    assert!(sv.contains("grab started"), "server log:\n{sv}");
    // Neither sidecar wrote an IPC file.
    for g in [&ga, &gb] {
        assert_eq!(ipc_files(&g.dir), Vec::<String>::new());
    }
    let _ = std::fs::remove_dir_all(&root);
}
