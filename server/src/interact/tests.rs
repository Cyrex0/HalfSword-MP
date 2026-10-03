//! Interaction-channel validation: shape, context, rate limits, reach,
//! impulse caps / budget, grab leases and ordering on the unordered channel.

use super::*;
use crate::posecodec::{LOWERARM_L, PELVIS};

const A: PeerId = 1; // initiator
const B: PeerId = 2; // target
const C: PeerId = 3;

fn ctx() -> Ctx {
    Ctx { initiator_caps: true, initiator_alive: true, target_present: true, target_caps: true, frozen: false,
          roots: Some(([0.0; 3], [80.0, 0.0, 0.0])) }
}

fn ev(kind: u8, target: PeerId, id: u32, hand: u8) -> Interact {
    Interact { kind, target_peer: target, id, hand, bone: LOWERARM_L as u8, point: [3.0, 0.0, 1.0],
               vector: [10.0, 20.0, 150.0], ts: 1000, ..Default::default() }
}

fn imp(target: PeerId, v: [f32; 3]) -> Interact {
    Interact { kind: K::IMPULSE, target_peer: target, id: 1, hand: 0, bone: PELVIS as u8, point: [0.0; 3],
               vector: v, ts: 1000, ..Default::default() }
}

const NEAR: Geo = Geo::Dist(20.0);
fn near(_: &Interact) -> Geo { NEAR }
fn far(_: &Interact) -> Geo { Geo::Dist(250.0) }
fn nodata(_: &Interact) -> Geo { Geo::NoData }

fn kinds(v: &Verdict) -> Vec<(PeerId, PeerId, u8)> {
    v.routes.iter().map(|r| (r.to, r.from, r.ev.kind)).collect()
}

#[test]
fn malformed_events_are_dropped_without_routes() {
    let mut ix = Interactions::default();
    let bad = [
        Interact { kind: K::GRAB_DENIED, ..ev(K::GRAB_START, B, 1, 0) },
        Interact { kind: 99, ..ev(K::GRAB_START, B, 1, 0) },
        ev(K::GRAB_START, A, 1, 0),                                    // self
        ev(K::GRAB_START, 0, 1, 0),                                    // nobody
        Interact { hand: 2, ..ev(K::GRAB_START, B, 1, 0) },
        Interact { bone: 16, ..ev(K::GRAB_START, B, 1, 0) },
        Interact { point: [f32::NAN, 0.0, 0.0], ..ev(K::GRAB_START, B, 1, 0) },
        Interact { vector: [0.0, f32::INFINITY, 0.0], ..imp(B, [1.0; 3]) },
        Interact { point: [61.0, 0.0, 0.0], ..ev(K::GRAB_START, B, 1, 0) }, // off the bone
        ev(K::GRAB_START, B, 0, 0),                                    // id 0
    ];
    for e in bad {
        let v = ix.on_c2s(A, e, &ctx(), near, 0);
        assert!(v.routes.is_empty(), "{e:?} -> {v:?}");
        assert!(v.note.is_some());
    }
    assert!(ix.active().is_empty());
    // A peer that did not negotiate INTERACT is ignored outright.
    let v = ix.on_c2s(A, ev(K::GRAB_START, B, 1, 0), &Ctx { initiator_caps: false, ..ctx() }, near, 0);
    assert!(v.routes.is_empty());
}

#[test]
fn grab_start_update_end_round_trip() {
    let mut ix = Interactions::default();
    let v = ix.on_c2s(A, ev(K::GRAB_START, B, 7, 1), &ctx(), near, 0);
    assert_eq!(kinds(&v), vec![(B, A, K::GRAB_START)]);
    assert_eq!(v.routes[0].ev.point, [3.0, 0.0, 1.0]);
    assert_eq!(ix.active(), vec![(A, 1, B)]);
    // Updates refresh and forward, without a geometry query (victim struggles).
    let v = ix.on_c2s(A, ev(K::GRAB_UPDATE, B, 7, 1), &ctx(), |_| panic!("no reach check on refresh"), 100);
    assert_eq!(kinds(&v), vec![(B, A, K::GRAB_UPDATE)]);
    // A duplicate start is ignored.
    assert!(ix.on_c2s(A, ev(K::GRAB_START, B, 7, 1), &ctx(), near, 150).routes.is_empty());
    let v = ix.on_c2s(A, ev(K::GRAB_END, B, 7, 1), &ctx(), near, 200);
    assert_eq!(kinds(&v), vec![(B, A, K::GRAB_END)]);
    assert!(ix.active().is_empty());
    // After the end, a late update / start of that id is stale.
    assert!(ix.on_c2s(A, ev(K::GRAB_UPDATE, B, 7, 1), &ctx(), near, 210).routes.is_empty());
    assert!(ix.on_c2s(A, ev(K::GRAB_START, B, 7, 1), &ctx(), near, 220).routes.is_empty());
    assert_eq!(ix.stats.grabs, 1);
}

#[test]
fn grab_end_is_honoured_even_when_frozen_or_dead() {
    let mut ix = Interactions::default();
    ix.on_c2s(A, ev(K::GRAB_START, B, 1, 0), &ctx(), near, 0);
    let dead_frozen = Ctx { initiator_alive: false, frozen: true, ..ctx() };
    let v = ix.on_c2s(A, ev(K::GRAB_END, B, 1, 0), &dead_frozen, near, 10);
    assert_eq!(kinds(&v), vec![(B, A, K::GRAB_END)]);
}

#[test]
fn unordered_channel_end_before_start_and_update_before_start() {
    let mut ix = Interactions::default();
    // END overtakes START: the START that arrives later is stale.
    assert!(ix.on_c2s(A, ev(K::GRAB_END, B, 3, 0), &ctx(), near, 0).routes.is_empty());
    assert!(ix.on_c2s(A, ev(K::GRAB_START, B, 3, 0), &ctx(), near, 5).routes.is_empty());
    assert!(ix.active().is_empty());
    // UPDATE overtakes START: it opens the grab (forwarded as a START), the
    // START that follows is a duplicate.
    let v = ix.on_c2s(A, ev(K::GRAB_UPDATE, B, 4, 0), &ctx(), near, 10);
    assert_eq!(kinds(&v), vec![(B, A, K::GRAB_START)]);
    assert!(ix.on_c2s(A, ev(K::GRAB_START, B, 4, 0), &ctx(), near, 12).routes.is_empty());
    assert_eq!(ix.active(), vec![(A, 0, B)]);
    // An older id never replaces the active one.
    assert!(ix.on_c2s(A, ev(K::GRAB_UPDATE, B, 2, 0), &ctx(), near, 20).routes.is_empty());
    assert_eq!(ix.active(), vec![(A, 0, B)]);
}

#[test]
fn a_new_grab_with_the_same_hand_ends_the_old_one() {
    let mut ix = Interactions::default();
    ix.on_c2s(A, ev(K::GRAB_START, B, 1, 0), &ctx(), near, 0);
    let v = ix.on_c2s(A, ev(K::GRAB_START, C, 2, 0), &ctx(), near, 300);
    assert_eq!(kinds(&v), vec![(B, A, K::GRAB_END), (C, A, K::GRAB_START)]);
    assert_eq!(v.routes[0].ev.id, 1);
    assert_eq!(ix.active(), vec![(A, 0, C)]);
    // Both hands at once are fine.
    let v = ix.on_c2s(A, ev(K::GRAB_START, B, 1, 1), &ctx(), near, 400);
    assert_eq!(kinds(&v), vec![(B, A, K::GRAB_START)]);
    assert_eq!(ix.active(), vec![(A, 0, C), (A, 1, B)]);
}

#[test]
fn out_of_reach_grab_is_denied_to_the_grabber() {
    let mut ix = Interactions::default();
    let v = ix.on_c2s(A, ev(K::GRAB_START, B, 5, 0), &ctx(), far, 0);
    assert_eq!(kinds(&v), vec![(A, 0, K::GRAB_DENIED)]);
    assert_eq!(v.routes[0].ev.id, 5);
    assert_eq!(v.routes[0].ev.target_peer, B);
    assert!(v.note.as_deref().unwrap().contains("out of reach"), "{v:?}");
    // Its updates are then stale (no second denial).
    assert!(ix.on_c2s(A, ev(K::GRAB_UPDATE, B, 5, 0), &ctx(), near, 10).routes.is_empty());
    assert_eq!(ix.stats.denied, 1);
}

#[test]
fn without_pose_history_the_root_distance_decides() {
    let mut ix = Interactions::default();
    let v = ix.on_c2s(A, ev(K::GRAB_START, B, 1, 0), &ctx(), nodata, 0);
    assert_eq!(kinds(&v), vec![(B, A, K::GRAB_START)], "roots 80 uu apart");
    let far_roots = Ctx { roots: Some(([0.0; 3], [0.0, 900.0, 0.0])), ..ctx() };
    let v = ix.on_c2s(A, ev(K::GRAB_START, B, 2, 1), &far_roots, nodata, 10);
    assert_eq!(kinds(&v), vec![(A, 0, K::GRAB_DENIED)]);
    let v = ix.on_c2s(A, imp(B, [100.0, 0.0, 0.0]), &far_roots, nodata, 20);
    assert!(v.routes.is_empty());
    // No roots either (legacy / first frames): accepted.
    let v = ix.on_c2s(A, imp(B, [100.0, 0.0, 0.0]), &Ctx { roots: None, ..ctx() }, nodata, 30);
    assert_eq!(kinds(&v), vec![(B, A, K::IMPULSE)]);
}

#[test]
fn context_refusals() {
    let mut ix = Interactions::default();
    for (c, why) in [
        (Ctx { initiator_alive: false, ..ctx() }, "initiator not alive"),
        (Ctx { target_present: false, ..ctx() }, "target not connected"),
        (Ctx { frozen: true, ..ctx() }, "match frozen"),
        (Ctx { target_caps: false, ..ctx() }, "target cannot receive"),
    ] {
        let v = ix.on_c2s(A, imp(B, [100.0, 0.0, 0.0]), &c, near, 0);
        assert!(v.routes.is_empty() && v.note.as_deref().unwrap().contains(why), "{v:?}");
    }
    // A refused START is denied (so the grabber's stand-in stops yielding).
    let v = ix.on_c2s(A, ev(K::GRAB_START, B, 9, 0), &Ctx { frozen: true, ..ctx() }, near, 0);
    assert_eq!(kinds(&v), vec![(A, 0, K::GRAB_DENIED)]);
    // An active grab whose update fails the context is denied AND released
    // on the target.
    ix.on_c2s(A, ev(K::GRAB_START, B, 10, 1), &ctx(), near, 0);
    let v = ix.on_c2s(A, ev(K::GRAB_UPDATE, B, 10, 1), &Ctx { initiator_alive: false, ..ctx() }, near, 50);
    assert_eq!(kinds(&v), vec![(A, 0, K::GRAB_DENIED), (B, A, K::GRAB_END)]);
    assert!(ix.active().is_empty());
}

#[test]
fn impulse_is_clamped_per_event_and_by_the_target_budget() {
    let mut ix = Interactions::default();
    // 30 000 → MAX_IMPULSE, direction kept.
    let v = ix.on_c2s(A, imp(B, [0.0, 30_000.0, 0.0]), &ctx(), near, 0);
    assert_eq!(kinds(&v), vec![(B, A, K::IMPULSE)]);
    let got = v.routes[0].ev.vector;
    assert!((got[1] - MAX_IMPULSE).abs() < 1.0 && got[0] == 0.0, "{got:?}");
    assert!(v.note.as_deref().unwrap().contains("clamped"));
    // Burst budget RECV_BURST: the second full-size impulse is cut.
    let v = ix.on_c2s(C, imp(B, [MAX_IMPULSE, 0.0, 0.0]), &ctx(), near, 0);
    let x = v.routes[0].ev.vector[0];
    assert!((x - (RECV_BURST - MAX_IMPULSE)).abs() < 1.0, "x={x}");
    // Exhausted: nothing more this instant, from anyone.
    assert!(ix.on_c2s(A, imp(B, [100.0, 0.0, 0.0]), &ctx(), near, 0).routes.is_empty());
    // Refills at RECV_BUDGET per second.
    let v = ix.on_c2s(A, imp(B, [5000.0, 0.0, 0.0]), &ctx(), near, 200);
    assert_eq!(v.routes[0].ev.vector[0], 5000.0);
    // Over a sustained second, a body never receives more than the budget.
    let mut ix = Interactions::default();
    let mut total = 0.0;
    for i in 0..1000 {
        for from in [A, C] {
            let v = ix.on_c2s(from, imp(B, [MAX_IMPULSE, 0.0, 0.0]), &ctx(), near, i);
            total += v.routes.iter().map(|r| r.ev.vector[0]).sum::<f32>();
        }
    }
    assert!(total <= RECV_BURST + RECV_BUDGET + 1.0, "total {total}");
}

#[test]
fn impulse_rate_limit_and_reach() {
    let mut ix = Interactions::default();
    let mut sent = 0;
    for _ in 0..50 {
        if !ix.on_c2s(A, imp(B, [10.0, 0.0, 0.0]), &ctx(), near, 0).routes.is_empty() { sent += 1; }
    }
    assert_eq!(sent, IMPULSE_RATE.0 as usize, "burst");
    // 20/s afterwards: 100 ms → 2 more.
    let n = (0..10).filter(|_| !ix.on_c2s(A, imp(B, [10.0, 0.0, 0.0]), &ctx(), near, 100).routes.is_empty()).count();
    assert_eq!(n, 2);
    let mut ix = Interactions::default();
    let v = ix.on_c2s(A, imp(B, [10.0, 0.0, 0.0]), &ctx(), far, 0);
    assert!(v.routes.is_empty() && v.note.unwrap().contains("out of reach"));
    assert!(ix.on_c2s(A, imp(B, [0.1, 0.0, 0.0]), &ctx(), near, 0).routes.is_empty(), "empty impulse");
}

#[test]
fn grab_start_rate_limit_denies() {
    let mut ix = Interactions::default();
    let mut denied = 0;
    for id in 1..=10u32 {
        let v = ix.on_c2s(A, ev(K::GRAB_START, B, id, 0), &ctx(), near, 0);
        if kinds(&v).contains(&(A, 0, K::GRAB_DENIED)) { denied += 1; }
    }
    assert_eq!(denied, 10 - GRAB_START_RATE.0 as usize);
}

#[test]
fn update_flood_is_thinned_but_keeps_the_lease() {
    let mut ix = Interactions::default();
    ix.on_c2s(A, ev(K::GRAB_START, B, 1, 0), &ctx(), near, 0);
    let fwd = (0..100).filter(|_| !ix.on_c2s(A, ev(K::GRAB_UPDATE, B, 1, 0), &ctx(), near, 900).routes.is_empty()).count();
    assert!(fwd <= GRAB_UPDATE_RATE.0 as usize + 36, "fwd {fwd}");
    // Lease refreshed at 900 → still alive at 1800.
    assert!(ix.expire(1800).is_empty());
    assert_eq!(ix.active(), vec![(A, 0, B)]);
}

#[test]
fn lease_expiry_ends_the_grab_on_both_sides() {
    let mut ix = Interactions::default();
    ix.on_c2s(A, ev(K::GRAB_START, B, 1, 0), &ctx(), near, 0);
    ix.on_c2s(A, ev(K::GRAB_UPDATE, B, 1, 0), &ctx(), near, 500);
    assert!(ix.expire(1500).is_empty(), "within the lease");
    let r = ix.expire(1501);
    let k: Vec<_> = r.iter().map(|r| (r.to, r.from, r.ev.kind, r.ev.id)).collect();
    assert_eq!(k, vec![(B, A, K::GRAB_END, 1), (A, 0, K::GRAB_END, 1)]);
    assert!(ix.active().is_empty());
    assert_eq!(ix.stats.expired, 1);
    // The grabber's late update can't resurrect it.
    assert!(ix.on_c2s(A, ev(K::GRAB_UPDATE, B, 1, 0), &ctx(), near, 1600).routes.is_empty());
}

#[test]
fn leaving_ends_every_grab_involving_the_peer() {
    let mut ix = Interactions::default();
    ix.on_c2s(A, ev(K::GRAB_START, B, 1, 0), &ctx(), near, 0);   // A grabs B
    ix.on_c2s(C, ev(K::GRAB_START, B, 1, 1), &ctx(), near, 0);   // C grabs B
    ix.on_c2s(B, ev(K::GRAB_START, A, 1, 0), &ctx(), near, 0);   // B grabs A
    let mut r: Vec<_> = ix.forget(B).iter().map(|r| (r.to, r.from, r.ev.kind)).collect();
    r.sort_unstable();
    // B's own grab ends on A; A's and C's grabs on B end on A and C (from the server).
    assert_eq!(r, vec![(A, 0, K::GRAB_END), (A, B, K::GRAB_END), (C, 0, K::GRAB_END)]);
    assert!(ix.active().is_empty());
}

#[test]
fn caps_registry() {
    note_caps(41, hsmp_net::net::caps::INTERACT | hsmp_net::net::caps::MODES);
    note_caps(42, hsmp_net::net::caps::MODES);
    assert!(has_interact(41));
    assert!(!has_interact(42));
    assert!(!has_interact(43));
    on_leave(41);
    assert!(!has_interact(41));
}

/// When the target streams history, an event the geometry
/// refuses (ts = 0 / uncovered) is dropped even with the roots close
/// together; it never falls back to the coarse root check.
#[test]
fn refused_geometry_never_falls_back_to_roots() {
    let mut ix = Interactions::default();
    let refused = |_: &Interact| Geo::Refused("no_ts: missing timestamp (target has history)");
    let v = ix.on_c2s(A, Interact { ts: 0, ..imp(B, [100.0, 0.0, 0.0]) }, &ctx(), refused, 0);
    assert!(v.routes.is_empty(), "{v:?}");
    let v = ix.on_c2s(A, Interact { ts: 0, ..ev(K::GRAB_START, B, 1, 0) }, &ctx(), refused, 10);
    assert_eq!(kinds(&v), vec![(A, 0, K::GRAB_DENIED)], "the grabber is told");
    assert!(ix.active().is_empty());
}

/// Records from the wire (protocol v6): hostile and mutated payloads go through
/// `record::view` and then the judge; nothing panics, and whatever is forwarded is a valid
/// record that never targets its sender and never carries more than `MAX_IMPULSE`.
#[test]
fn hostile_records_through_view_and_judge() {
    use hsmp_ipc::record::view;
    let mut ix = Interactions::default();
    let mut s: u64 = 0xC0FF_EE11;
    let mut rnd = move || { s ^= s << 13; s ^= s >> 7; s ^= s << 17; s };
    let seeds: Vec<Vec<u8>> = [ev(K::GRAB_START, B, 1, 0), ev(K::GRAB_UPDATE, B, 2, 1), ev(K::GRAB_END, B, 1, 0),
                               imp(B, [20_000.0, 0.0, 0.0]), imp(C, [300.0, 0.0, 0.0])]
        .iter().map(|e| hsmp_ipc::bytemuck::bytes_of(e).to_vec()).collect();
    let (mut accepted, mut routed) = (0, 0);
    for i in 0..30_000i64 {
        let mut d = seeds[(rnd() as usize) % seeds.len()].clone();
        for _ in 0..(rnd() % 4) {
            let j = (rnd() as usize) % d.len();
            d[j] = rnd() as u8;
        }
        if rnd() % 16 == 0 { d.truncate((rnd() as usize) % d.len()); }
        let Ok(v) = view::<Interact>(&d) else { continue };
        accepted += 1;
        let from = 1 + (rnd() % 3) as PeerId;
        let geo = |_: &Interact| if i % 3 == 0 { Geo::NoData } else { NEAR };
        let verdict = ix.on_c2s(from, v.head(), &ctx(), geo, i * 7);
        for r in &verdict.routes {
            routed += 1;
            assert!(r.to != r.from, "{r:?}");
            assert!(view::<Interact>(hsmp_ipc::bytemuck::bytes_of(&r.ev)).is_ok(), "forwarded an invalid record {r:?}");
            if r.ev.kind == K::IMPULSE {
                let m = (r.ev.vector.iter().map(|x| x * x).sum::<f32>()).sqrt();
                assert!(m <= MAX_IMPULSE + 0.5, "{m}");
            }
        }
        let _ = ix.expire(i * 7);
    }
    assert!(accepted > 1000 && routed > 100, "{accepted} accepted, {routed} routed");
}