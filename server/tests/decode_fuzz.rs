//! Decode-path fuzz soak for the server/sidecar.
//!
//! Compiles the self-contained codec modules straight from src/ (the server is
//! a binary crate, so they are included by path, the same way the sidecar
//! reuses proto.rs) and feeds them random + mutated untrusted bytes:
//!
//! * `proto::decode_msg`   - every received channel message (server and sidecar): the wire
//!   split, then `schema::check_payload` of its kind (kind 0 refused like any unknown kind)
//! * combat records       - every schema/combat.rs kind through `check_payload`
//! * `posecodec::decode`     - C2SSkeletalState / S2CSkeletalBroadcast bones
//! * `query::parse_request` / `parse_reply` - the server-browser query (pre-auth)
//! * world records (`hsmp_ipc::schema::world`, protocol v6) - every kind through
//!   `schema::check_payload`, and whatever validates through the server's world
//!   logic (`world.rs`: state, claim, manifest, hash report, tick, encode)
//!
//! The quick run (HSMP_FUZZ_SECS unset -> 1 s per target) is part of
//! `cargo test`; the soak is `HSMP_FUZZ_SECS=300 cargo test --release
//! --test decode_fuzz -- --nocapture --test-threads 1`.

#![allow(dead_code, unused_imports)]

use hsmp_pose::posecodec;
#[path = "../src/proto.rs"]
mod proto;
#[path = "../src/query.rs"]
mod query;
#[path = "../src/world.rs"]
mod world;

use rand::rngs::StdRng;
use rand::{Rng, SeedableRng};
use std::panic::{self, AssertUnwindSafe};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

fn budget() -> Duration {
    let s = std::env::var("HSMP_FUZZ_SECS").ok().and_then(|v| v.parse::<u64>().ok()).unwrap_or(1);
    Duration::from_secs(s)
}

fn mutate(rng: &mut StdRng, d: &mut Vec<u8>, corpus: &[Vec<u8>]) {
    for _ in 0..rng.gen_range(1..6) {
        match rng.gen_range(0..8) {
            0 if !d.is_empty() => {
                let i = rng.gen_range(0..d.len());
                d[i] ^= 1 << rng.gen_range(0..8);
            }
            1 if !d.is_empty() => {
                let i = rng.gen_range(0..d.len());
                d[i] = [0u8, 1, 0x7F, 0x80, 0xFF, 0xFE, rng.gen()][rng.gen_range(0..7)];
            }
            2 if !d.is_empty() => {
                let n = rng.gen_range(0..d.len());
                d.truncate(n);
            }
            3 if d.len() >= 8 => {
                // extreme u32/u64 length prefixes and float bit patterns
                let i = rng.gen_range(0..d.len() - 7);
                let v: u64 = [0, 1, 0xFFFF_FFFF, u64::MAX, 0x7FC0_0000, 0x7F80_0000, 0xFF80_0000, 1 << 40][rng.gen_range(0..8)];
                d[i..i + 8].copy_from_slice(&v.to_le_bytes());
            }
            4 if !corpus.is_empty() => {
                let o = &corpus[rng.gen_range(0..corpus.len())];
                let cut = rng.gen_range(0..=d.len());
                let from = rng.gen_range(0..=o.len());
                d.truncate(cut);
                d.extend_from_slice(&o[from..]);
            }
            5 if d.len() < 4096 => {
                let at = rng.gen_range(0..=d.len());
                for k in 0..rng.gen_range(1..16) {
                    d.insert(at + k, rng.gen());
                }
            }
            _ => {
                if d.len() < 4096 {
                    d.push(rng.gen());
                }
            }
        }
    }
}

/// Run `f` for `budget` over mutated corpus / random inputs; returns
/// (iterations, distinct panics with the input that produced them).
fn soak(name: &str, corpus: &[Vec<u8>], f: &dyn Fn(&[u8])) -> (u64, Vec<String>) {
    let last: Arc<Mutex<String>> = Arc::new(Mutex::new(String::new()));
    let l2 = last.clone();
    let prev = panic::take_hook();
    panic::set_hook(Box::new(move |info| {
        let msg = info
            .payload()
            .downcast_ref::<&str>()
            .map(|s| s.to_string())
            .or_else(|| info.payload().downcast_ref::<String>().cloned())
            .unwrap_or_default();
        let loc = info.location().map(|l| format!("{}:{}", l.file(), l.line())).unwrap_or_default();
        *l2.lock().unwrap_or_else(|e| e.into_inner()) = format!("{msg} @ {loc}");
    }));
    let mut rng = StdRng::seed_from_u64(0x5EED ^ name.len() as u64);
    let t0 = Instant::now();
    let b = budget();
    let (mut iters, mut panics, mut seen) = (0u64, vec![], std::collections::BTreeSet::new());
    while t0.elapsed() < b {
        for _ in 0..64 {
            let mut d: Vec<u8> = if corpus.is_empty() || rng.gen_range(0..4) == 0 {
                (0..rng.gen_range(0..1500)).map(|_| rng.gen()).collect()
            } else {
                corpus[rng.gen_range(0..corpus.len())].clone()
            };
            mutate(&mut rng, &mut d, corpus);
            iters += 1;
            if panic::catch_unwind(AssertUnwindSafe(|| f(&d))).is_err() {
                let m = last.lock().unwrap_or_else(|e| e.into_inner()).clone();
                if seen.insert(m.clone()) && panics.len() < 16 {
                    let hex: String = d.iter().take(256).map(|x| format!("{x:02x}")).collect();
                    panics.push(format!("{m}\n      input ({} B): {hex}", d.len()));
                }
            }
        }
    }
    panic::set_hook(prev);
    println!("fuzz {name:<14} {:>5.0}s {iters:>10} iters  {} panic(s)", b.as_secs_f64(), panics.len());
    for p in &panics {
        println!("   PANIC {p}");
    }
    (iters, panics)
}

fn msg_corpus() -> Vec<Vec<u8>> {
    let mut rng = StdRng::seed_from_u64(0xB0D1);
    let mut v = vec![];
    // Every registered record kind (and kind 0, the retired bincode envelope) behind a
    // WireHdr, with zero and random payloads.
    let kinds: Vec<u16> = std::iter::once(0).chain(hsmp_ipc::schema::records().map(|r| r.kind)).collect();
    for k in kinds {
        let mut z = hsmp_ipc::wire::message(k, 0, 3, &[0u8; 96]);
        v.push(z.clone());
        z.truncate(hsmp_ipc::wire::HDR);
        z.extend((0..160).map(|_| rng.gen::<u8>()));
        v.push(z);
    }
    v
}

/// One received channel message, as the server and the sidecars take it: the wire split,
/// then the record gate of its kind. Kind 0 is refused like any unknown kind.
fn msg_target(d: &[u8]) {
    if let Ok((h, p)) = proto::decode_msg(d) {
        let r = hsmp_ipc::schema::check_payload(h.kind, p);
        if h.kind == 0 {
            assert!(r.is_err(), "kind 0 (the retired bincode envelope) is not a record");
        }
        if r.is_ok() {
            // Anything accepted re-frames to the same bytes.
            assert_eq!(hsmp_ipc::wire::message(h.kind, h.aux, h.peer, p), d);
        }
    }
}

#[test]
fn fuzz_decode_msg() {
    let (_, p) = soak("decode_msg", &msg_corpus(), &msg_target);
    assert!(p.is_empty());
}

/// Every combat record kind (schema/combat.rs): arbitrary bytes and mutations of valid
/// messages through the wire split and `schema::check_payload` (the server's and the
/// sidecars' only gate), then what the server does with an accepted claim / vitals record
/// (owned copy, re-frame, re-validate).
#[test]
fn fuzz_combat_records() {
    use hsmp_ipc::record::{to_payload, view};
    use hsmp_ipc::schema::combat::*;
    use hsmp_ipc::wire;
    let claim = Damage { hit_id: 7, cid: 3, target_peer_id: 2, round: 1, bone: hsmp_ipc::layout::Str::new("neck_01"),
                         flags: 32, location: [1.0, 2.0, 3.0], ..Default::default() };
    let mut vit = proto::vitals::unknown();
    proto::vitals::set(&mut vit, 0, 87.5);
    vit.dism = 1 << 8;
    let rows = [DamageDelta::new(0, -5.0), DamageDelta::new(11, 2.0)];
    let corpus: Vec<Vec<u8>> = vec![
        wire::encode(0, 0, &claim, &rows),
        wire::message(K_DAMAGE_IN, 0, 4, &to_payload(&claim, &[])),
        wire::message(K_HITFX_IN, 0, 4, &to_payload(&claim, &rows)),
        wire::encode(0, 0, &DamageVerdict::new(7, 3, VERDICT_FINAL, false, "range: x"), &[]),
        wire::encode(0, 0, &DamageAck { attacker: 4, hit_id: 7 }, &[]),
        wire::encode(0, 0, &Clash { other_peer_id: 4, my_ts: 1, other_ts: 2, _r: 0 }, &[]),
        wire::message(K_TOUCH, 0, 0, &to_payload(&Clash { other_peer_id: 4, my_ts: 1, other_ts: 2, _r: 0 }, &[])),
        wire::encode(0, 0, &DeathReport { death_id: 1, round: 2, ..Default::default() }, &[]),
        wire::encode(0, 0, &DeathAck { death_id: 1, _r: 0 }, &[]),
        wire::encode(0, 2, &Death::new(2, 1, 3, 1), &[]),
        wire::encode(0, 2, &vit, &[]),
        wire::encode(0, 0, &StandinDead { wall: 1, n: 0, _r: [0; 3] }, &[StandinDeadRow::new(2, "Willie_BP_C_1")]),
    ];
    for m in &corpus {
        let (h, p) = wire::split(m).unwrap();
        assert!(hsmp_ipc::schema::check_payload(h.kind, p).is_ok(), "corpus kind {:#06x}", h.kind);
    }
    let (_, p) = soak("combat_records", &corpus, &|d| {
        let Ok((h, p)) = wire::split(d) else { return };
        // Any kind of the combat range (a mutated kind too): unknown ones are refused.
        let kind = 0x0300 | (h.kind & 0xFF);
        if hsmp_ipc::schema::check_payload(kind, p).is_err() {
            return;
        }
        match kind {
            K_DAMAGE | K_DAMAGE_IN | K_HITFX_IN => {
                let v = view::<Damage>(p).expect("check_payload passed");
                let e = proto::DamageEvent::from_view(&v);
                assert!(e.deltas().len() <= MAX_DELTAS);
                let again = e.msg(K_DAMAGE_IN, 9);
                let (_, q) = wire::split(&again).unwrap();
                assert!(hsmp_ipc::schema::check_payload(K_DAMAGE_IN, q).is_ok(), "re-framed claim invalid");
                assert_eq!(q, p, "a claim re-frames to the same bytes");
            }
            K_VITALS => {
                let mut f = view::<Vitals>(p).expect("check_payload passed").head();
                let _ = (f.health(), f.dead(), f.dism_names().count());
                let hp = f.health().unwrap_or(0.0).min(50.0);
                proto::vitals::set(&mut f, 0, hp);
                assert!(hsmp_ipc::schema::check_payload(K_VITALS, hsmp_ipc::bytemuck::bytes_of(&f)).is_ok());
            }
            _ => {}
        }
    });
    assert!(p.is_empty());
}

#[test]
fn fuzz_posecodec_decode() {
    let bones: Vec<(u8, [f32; 7])> =
        (0..posecodec::WIRE_COUNT as u8).map(|i| (i, [100.0 + i as f32, -50.0, 90.0, 0.0, 0.0, 0.0, 1.0])).collect();
    let corpus = vec![posecodec::encode(1, &bones), posecodec::encode(2, &bones[..3])];
    let (_, p) = soak("posecodec", &corpus, &|d| {
        if let Some(f) = posecodec::decode(d) {
            assert!(f.bones.len() <= 255);
            for (_, p, q) in &f.bones {
                assert!(p.iter().chain(q.iter()).all(|x| x.is_finite()), "non-finite decoded pose");
            }
        }
    });
    assert!(p.is_empty());
}

#[test]
fn fuzz_query() {
    let info = query::QueryInfo::default();
    let corpus = vec![query::build_request(42), query::build_reply(42, &info)];
    let (_, p) = soak("query", &corpus, &|d| {
        let _ = query::is_request(d);
        let _ = query::parse_request(d);
        if let Some((n, i)) = query::parse_reply(d) {
            let r = query::build_reply(n, &i);
            assert!(r.len() <= query::REQ_LEN, "reply larger than a request");
        }
    });
    assert!(p.is_empty());
}

/// v6 interaction records (`interact`, `interact_grab_r` / `_l`): whole wire messages through
/// `wire::split` + `schema::check_payload` + `record::view`. An accepted record always holds
/// in-range, finite values (the server's `interact::` judge and the sidecar's id mapping run
/// on it; their own fuzz lives in `interact::tests` / `interact_client::tests`).
#[test]
fn fuzz_interact_records() {
    use hsmp_ipc::schema::interact::*;
    let ev = Interact { kind: kind::GRAB_START, hand: 1, bone: 5, target_peer: 2, id: 9, ts: 1000,
                        point: [1.0, 2.0, 3.0], vector: [10.0, 20.0, 140.0], gid: 3, ..Default::default() };
    let corpus: Vec<Vec<u8>> = [K_INTERACT, K_INTERACT_GRAB_R, K_INTERACT_GRAB_L]
        .iter()
        .flat_map(|&k| {
            [kind::GRAB_START, kind::GRAB_UPDATE, kind::GRAB_END, kind::IMPULSE, kind::GRAB_DENIED]
                .map(|c| hsmp_ipc::wire::message(k, 0, 3, hsmp_ipc::bytemuck::bytes_of(&Interact { kind: c, ..ev })))
        })
        .collect();
    let (_, p) = soak("interact", &corpus, &|d| {
        let Ok((h, payload)) = hsmp_ipc::wire::split(d) else { return };
        let ok = hsmp_ipc::schema::check_payload(h.kind, payload).is_ok();
        if !is_interact_kind(h.kind) {
            return;
        }
        match hsmp_ipc::record::view::<Interact>(payload) {
            Ok(v) => {
                assert!(ok);
                let e = v.head();
                assert!((kind::GRAB_START..=kind::GRAB_DENIED).contains(&e.kind) && e.hand <= 1 && e.bone < 16);
                assert!(e.point.iter().all(|x| x.is_finite() && x.abs() <= LOCAL_BOUND));
                assert!(e.vector.iter().all(|x| x.is_finite() && x.abs() <= VECTOR_BOUND));
                assert!(e.target_peer < PEER_LIMIT);
            }
            Err(_) => assert!(!ok),
        }
    });
    assert!(p.is_empty());
}

/// Valid messages of every world kind (`[WireHdr][record]`), the mutation corpus.
fn world_corpus() -> Vec<Vec<u8>> {
    use hsmp_ipc::layout::{Bool, Str};
    use hsmp_ipc::schema::world::*;
    use hsmp_ipc::wire::encode;
    let o = |id| WorldObj::from_parts(id, [100.0, 50.0, 10.0], [10.0, 20.0, 30.0], [5.0, -6.0, 7.0], WF_HELD | WF_SIM);
    let snap = |id| WorldSnap::new(0, 0, 0, o(id));
    vec![
        encode(0, 0, &WorldStateHead { level: 77, epoch: 1, seq: 3, ts: 9, n: 0, _r: 0, _r2: 0 }, &[o(10), o(11)]),
        encode(0, 0, &WorldClaim { level: 77, epoch: 1, req: 1, id: 10, mode: MODE_FREE, has_rest: Bool::TRUE, _r: 0, _r2: 0, rest: o(10) }, &[]),
        encode(0, 0, &WorldClaim { level: 77, epoch: 1, req: 2, id: 12, mode: CLAIM_INIT, has_rest: Bool::TRUE, _r: 0, _r2: 0, rest: o(12) }, &[]),
        encode(0, 0, &WorldSync { level: 77, _r: 0 }, &[]),
        encode(0, 0, &ManifestHead { level: 77, epoch: 1, req: 4, n: 0, _r: 0 },
               &[ManifestEntry { id: 10, chash: 5, pos: [100.0, 50.0, 10.0], _r: 0 }, ManifestEntry { id: 13, chash: 7, pos: [400.0, 0.0, 0.0], _r: 0 }]),
        encode(0, 0, &DynHead { level: 77, epoch: 1, req: 5, n: 0, _r: 0 },
               &[DynEntry { id: DYN_ID_BIT | (1 << 16) | 1, chash: 1, pos: [40.0, 0.0, 0.0], dyn_owner: 1, class_path: Str::new("/Game/W/X.X_C"), ..DynEntry::default() }]),
        encode(0, 0, &HashHead { level: 77, epoch: 1, seq: 1, hash: 99, n: 0, _r: 0, _r2: 0 },
               &[HashRow { id: 10, ver: 1, pos: [1.0, 2.0, 3.0], q: 0xC000_0000, status: HS_ALIVE | HS_SETTLED, _r: [0; 3], _r2: 0 }]),
        encode(0, 0, &OwnersHead { level: 77, epoch: 1, manifest_len: 2, n: 0, sync: Bool::TRUE, _r: 0 }, &[OwnerRec::new(10, 1, 1, MODE_TOUCH)]),
        encode(0, 0, &SnapHead { level: 77, epoch: 1, n: 0, _r: 0, _r2: 0 }, &[snap(10), snap(11)]),
        encode(0, 0, &VerdictHead { level: 77, epoch: 1, seq: 1, other: 2, compared: 3, mismatched_n: 1, hash_match: Bool::FALSE,
                                    hash_equal: Bool::FALSE, n: 0, _r: 0 }, &[Mismatch { id: 10, kind: MM_POSE, _r: [0; 3], dpos: 1.0, dang: 2.0 }]),
        encode(0, 0, &RemoteHead { level: 77, epoch: 1, n: 0, _r: 0 }, &[snap(10)]),
        encode(0, 0, &HeldHead { n: 0, _r: 0, _r2: 0 }, &[HeldRow { peer: 2, nid: 10, hand: 0, _r: [0; 7], actor: Str::new("Sword_1") }]),
    ]
}

/// Arbitrary bytes and mutations of valid world messages: the validator never panics, and
/// whatever passes it never panics the server's world logic (the state the dispatcher would
/// act on), whose output is again valid records.
#[test]
fn fuzz_world_records() {
    use hsmp_ipc::record::view;
    use hsmp_ipc::schema::world as rec;
    let w = std::sync::Mutex::new(world::World::new());
    let t0 = Instant::now();
    {
        let mut g = w.lock().unwrap();
        for p in 1..=2 { g.sync(p, 77, t0); }
    }
    let (n, p) = soak("world_records", &world_corpus(), &|d| {
        let Ok((h, payload)) = hsmp_ipc::wire::split(d) else { return };
        let ok = hsmp_ipc::schema::check_payload(h.kind, payload).is_ok();
        if !ok { return; }
        let mut g = w.lock().unwrap_or_else(|e| e.into_inner());
        let now = t0 + Duration::from_millis(g.peer_count() as u64);
        let e = g.epoch;
        let out = match h.kind {
            rec::K_WORLD_STATE => {
                let v = view::<rec::WorldStateHead>(payload).unwrap();
                let (_, ch) = g.state(1, v.head.level, e, v.head.seq, v.head.ts, &v.rows, Some([100.0, 50.0, 10.0]), now);
                g.owners_to_level(v.head.level, &ch)
            }
            rec::K_WORLD_CLAIM => {
                let v = view::<rec::WorldClaim>(payload).unwrap();
                let c = v.head();
                let _ = g.claim(2, c.level, e, c.id, c.mode, c.has_rest.get().then_some(c.rest), now);
                Vec::new()
            }
            rec::K_WORLD_SYNC => {
                let v = view::<rec::WorldSync>(payload).unwrap();
                let _ = v.head.level;
                g.sync(1, 77, now)
            }
            rec::K_WORLD_MANIFEST => {
                let v = view::<rec::ManifestHead>(payload).unwrap();
                g.manifest(1, v.head.level, e, v.head.req, v.rows.iter().map(world::WorldEntry::from_static).collect(), now)
            }
            rec::K_WORLD_DYN => {
                let v = view::<rec::DynHead>(payload).unwrap();
                g.manifest(1, v.head.level, e, v.head.req, v.rows.iter().map(world::WorldEntry::from_dyn).collect(), now)
            }
            rec::K_WORLD_HASH => {
                let v = view::<rec::HashHead>(payload).unwrap();
                let peer = 1 + (v.head.seq & 1);
                g.hash_report(peer, v.head.level, e, v.head.seq, v.head.hash, &v.rows, now)
            }
            _ => Vec::new(),
        };
        let mut all = out;
        all.extend(g.tick(now, &[1u32, 2].into_iter().collect(), &std::collections::HashMap::new()));
        for (_, m) in all {
            for msg in m.encode() {
                let (wh, pl) = hsmp_ipc::wire::split(&msg).unwrap();
                assert!(hsmp_ipc::schema::check_payload(wh.kind, pl).is_ok(), "the server built an invalid {:#06x}", wh.kind);
            }
        }
    });
    assert!(n > 0 && p.is_empty(), "{p:?}");
}

// ---- protocol v6: the session domain's records (0x02xx) ---------------------------------

/// One valid framed message of every session record kind (and a few variants).
fn session_corpus() -> Vec<Vec<u8>> {
    use hsmp_ipc::layout::Str;
    use hsmp_ipc::schema::session as rec;
    use hsmp_ipc::wire::encode;
    let cfg = rec::SessionConfig { rev: 3, best_of: 3, kit_mode: 1, max_fighters: 8, countdown_s: 3, arena: Str::new("Map_Arena_Pit"), ..Default::default() };
    let row = |seat: u8, peer: u32| rec::RosterRow {
        peer_id: peer, seat, wins: 1, spawn_id: (2 << 8) | seat as u32, spawn_pos: [1.0, 2.0, 3.0], spawn_yaw: 90.0,
        spawn_protect_ms: 3000, connected: (peer != 0).into(), alive: true.into(), waiting: (seat == 2).into(),
        nick: Str::new("Willie (2)"), player_id: [seat; 8], ..Default::default()
    };
    let head = rec::SessionHead { epoch: 77, match_id: 9, seq: 4, round: 1, phase: rec::phase::LOADING, winner_seat: rec::NO_SEAT,
        has_frozen: true.into(), config: cfg, frozen: cfg, phase_deadline_ms: 5000, server_time_ms: 1000, ..Default::default() };
    let mut set = rec::Command::new(9, rec::cmd_op::SET_CONFIG);
    set.patch.mask = rec::cfg::BEST_OF | rec::cfg::KIT_RULES | rec::cfg::ARENA;
    set.patch.best_of = 5;
    set.patch.arena = Str::new("Pit");
    vec![
        encode(0, 0, &rec::Welcome { server_epoch: 1, server_time_ms: 2, caps: 3, peer_id: 4, seat: 1, role: 0, _r: 0, nick: Str::new("Willie") }, &[]),
        encode(0, 0, &head, &[row(1, 10), row(2, 11), row(3, 0)]),
        encode(0, 0, &rec::SessionHead { phase: rec::phase::LOBBY, ..head }, &[]),
        encode(0, 0, &rec::PingsHead { epoch: 1, server_time_ms: 2, n: 0, _r: [0; 3] }, &[rec::PingRow { peer_id: 3, rtt_ms: 40, seat: 1, _r: 0 }]),
        encode(0, 0, &rec::Command { flag: true.into(), ..rec::Command::new(7, rec::cmd_op::READY) }, &[]),
        encode(0, 0, &rec::Command { text: Str::new("Map_Arena_Pit"), ..rec::Command::new(8, rec::cmd_op::PICK_ARENA) }, &[]),
        encode(0, 0, &rec::Command { peer_id: 4, text: Str::new("afk"), ..rec::Command::new(8, rec::cmd_op::KICK) }, &[]),
        encode(0, 0, &set, &[]),
        encode(0, 0, &rec::CmdResult { cmd_id: 7, ok: true.into(), op: 1, reason_text: Str::new("ok"), ..Default::default() }, &[]),
        encode(0, 0, &rec::GameStatus { match_id: 9, round: 2, life: 1, flags: rec::status_flag::LOADED, load_error: 5, arena: Str::new("Map_Arena_Pit"), ..Default::default() }, &[]),
        encode(0, 0, &rec::Spawned { round: 2, slot: 1, pos: [1.0, 2.0, 3.0], clear: true.into(), _r: [0; 3] }, &[]),
        encode(0, 0, &rec::Notice { code: 3, event_id: 1, args: [Str::new("Mate"), Str::new("x"), Str::new("2"), Str::new("2")], ..Default::default() }, &[]),
        encode(0, 0, &rec::KillFeed { event_id: 2, killer_seat: 1, victim_seat: 2, weapon: Str::new("sword"), ..Default::default() }, &[]),
        encode(0, 0, &rec::Kicked { reason: Str::new("banned"), code: 3, ..Default::default() }, &[]),
        encode(0, 0, &rec::ServerClosing { reason: 1, text: Str::new("Host closed the server"), ..Default::default() }, &[]),
        encode(0, 0, &rec::Leave { reason: 1, _r: [0; 7] }, &[]),
        encode(0, 0, &rec::Chat { text: Str::new("gg wp é") }, &[]),
        encode(0, 3, &rec::ChatIn { from_peer: 3, _r: 0, nick: Str::new("Willie"), text: Str::new("gg") }, &[]),
        encode(0, 0, &rec::AdminStateHead { admin_peer: 2, n: 0, _r: 0 }, &[rec::BanRow { entry: Str::new("203.0.x.x#1a2b3c4d") }]),
        encode(0, 0, &rec::Ping { client_time_ms: 5 }, &[]),
        encode(0, 0, &rec::Pong { client_time_ms: 5, server_time_ms: 6 }, &[]),
    ]
}

/// What the server and the sidecar do with an untrusted v6 message of the session domain:
/// split the header, validate by kind (`schema::check_payload`), then the typed view the
/// handlers act on; an accepted record re-frames to the same bytes.
fn session_target(d: &[u8]) {
    use hsmp_ipc::record::{to_payload, view};
    use hsmp_ipc::schema::session as rec;
    let Ok((h, p)) = proto::decode_msg(d) else { return };
    let ok = hsmp_ipc::schema::check_payload(h.kind, p).is_ok();
    macro_rules! typed {
        ($t:ty) => {{
            let v = view::<$t>(p);
            assert_eq!(v.is_ok(), ok, "check_payload and view agree");
            if let Ok(v) = v {
                assert_eq!(to_payload(&v.head(), &v.rows), p, "an accepted record re-frames to the same bytes");
            }
        }};
    }
    match h.kind {
        rec::K_WELCOME => typed!(rec::Welcome),
        rec::K_SESSION => typed!(rec::SessionHead),
        rec::K_PINGS => typed!(rec::PingsHead),
        rec::K_COMMAND => typed!(rec::Command),
        rec::K_CMD_RESULT => typed!(rec::CmdResult),
        rec::K_GAME_STATUS => typed!(rec::GameStatus),
        rec::K_SPAWNED => typed!(rec::Spawned),
        rec::K_NOTICE => typed!(rec::Notice),
        rec::K_KILL_FEED => typed!(rec::KillFeed),
        rec::K_KICKED => typed!(rec::Kicked),
        rec::K_SERVER_CLOSING => typed!(rec::ServerClosing),
        rec::K_LEAVE => typed!(rec::Leave),
        rec::K_CHAT => typed!(rec::Chat),
        rec::K_CHAT_IN => typed!(rec::ChatIn),
        rec::K_ADMIN_STATE => typed!(rec::AdminStateHead),
        rec::K_PING => typed!(rec::Ping),
        rec::K_PONG => typed!(rec::Pong),
        rec::K_LINK => typed!(rec::Link),
        _ => {}
    }
    if ok {
        // The relay patches peer / aux in place; still the same record.
        let mut m = d.to_vec();
        hsmp_ipc::wire::set_peer(&mut m, 7);
        hsmp_ipc::wire::set_aux(&mut m, 9);
        let (h2, p2) = hsmp_ipc::wire::split(&m).unwrap();
        assert!(hsmp_ipc::schema::check_payload(h2.kind, p2).is_ok());
    }
}

#[test]
fn fuzz_session_records() {
    let corpus = session_corpus();
    for m in &corpus {
        let (h, p) = hsmp_ipc::wire::split(m).unwrap();
        assert!(hsmp_ipc::schema::check_payload(h.kind, p).is_ok(), "corpus record {:#06x} is valid", h.kind);
    }
    let (_, p) = soak("session_rec", &corpus, &session_target);
    assert!(p.is_empty());
}

// ---- protocol v6 records: loadout domain (kit, kit_verdict, kit_rules[_req], loadout) ----

fn loadout_corpus() -> Vec<Vec<u8>> {
    use hsmp_ipc::layout::Str;
    use hsmp_ipc::schema::loadout::*;
    let mut kit = Kit::default();
    kit.class = Str::new("knight");
    kit.r = Str::new("w_longsword3");
    kit.seq = 7;
    let items: Vec<KitItem> = ["h_armet", "bv_1", "s_pauldron"].iter().map(|s| KitItem { id: Str::new(s) }).collect();
    let rules = KitRules { rev: 3, seq: 1, budget: 30, mode: MODE_CUSTOM, _r: 0 };
    let mut h = LoadoutHead::default();
    h.version = 99;
    h.flags = LOADOUT_HAS_R;
    h.r.class = Str::new("@Weapons/Blueprints/Built_Weapons/Swords/BP_Sword_Arming_3");
    h.r.head_size = [1.0; 3];
    let rows: Vec<ArmorRow> = (0..6u8)
        .map(|i| {
            let mut r = ArmorRow::default();
            r.class = Str::new("@Armor/Blueprints/Modular_Armor/BP_Armor_Modular_Core_Torso_A");
            r.slot = i;
            r.flags = ROW_PIECE | ROW_PASSPORT;
            r.fabric1 = [0.4, 0.3, 0.2, 1.0];
            r
        })
        .collect();
    vec![
        hsmp_ipc::wire::encode(0, 0, &kit, &items),
        hsmp_ipc::wire::message(K_KIT_VERDICT, 0, 4, &hsmp_ipc::record::to_payload(&kit, &items)),
        hsmp_ipc::wire::encode(0, 0, &rules, &[]),
        hsmp_ipc::wire::message(K_KIT_RULES_REQ, 0, 0, &hsmp_ipc::record::to_payload(&rules, &[])),
        hsmp_ipc::wire::encode(0, 4, &h, &rows),
    ]
}

/// Every loadout-domain message, arbitrary and mutated: the v6 split, the schema check
/// (`check_payload`) and the typed views the server / sidecar handlers use never panic, and
/// whatever passes `check_payload` also passes the handler's own view.
#[test]
fn fuzz_loadout_records() {
    use hsmp_ipc::record::view;
    use hsmp_ipc::schema::loadout::*;
    let (_, p) = soak("loadout_rec", &loadout_corpus(), &|d| {
        let Ok((h, payload)) = proto::decode_msg(d) else { return };
        let ok = hsmp_ipc::schema::check_payload(h.kind, payload).is_ok();
        let _ = proto::record_mode(h.kind, h.peer);
        match h.kind {
            K_KIT | K_KIT_VERDICT => assert_eq!(view::<Kit>(payload).is_ok(), ok),
            K_KIT_RULES | K_KIT_RULES_REQ => assert_eq!(view::<KitRules>(payload).is_ok(), ok),
            K_LOADOUT => {
                if let Ok(v) = view::<LoadoutHead>(payload) {
                    assert!(ok);
                    let _ = check_loadout_rows(&v.rows);
                    let mut b = LoadoutBuf::new_boxed();
                    assert!(b.load(payload).is_ok());
                    assert_eq!(b.payload(), payload);
                }
            }
            _ => {}
        }
    });
    assert!(p.is_empty());
}
