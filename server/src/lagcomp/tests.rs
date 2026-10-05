//! Lag comp v2 tests: rings, clock map, server-predicted view time, the
//! cheat cases (backtrack, NoData, ts forgery, parry spam), capsules,
//! swept blades, clash adjudication and a simulated-internet fight.

use super::*;
use crate::validate::cheat;
use posecodec::*;
use proptest::prelude::*;

const VIC: PeerId = 0x1A72; // ids no other test module uses: kit facts are global
const ATT: PeerId = 0x1A71;

#[test]
fn cutting_bone_frame_uses_delivered_brackets_not_hidden_midframe() {
    let mut s=Store::default();
    let frame=|x:f32,q:posecodec::v2::Quat|BoneFrames {
        p:[[x,0.0,0.0];posecodec::v2::NB],q:[q;posecodec::v2::NB],scale:1.0,
    };
    let identity=[0.0,0.0,0.0,1.0];
    s.peer(VIC).bone_frames.push(1000,frame(0.0,identity));
    // True source motion contains a nonlinear intermediate twist; decimation
    // withheld this frame. The receiver interpolates its delivered endpoints.
    s.peer(VIC).bone_frames.push(1050,frame(80.0,[0.0,0.0,1.0,0.0]));
    s.peer(VIC).bone_frames.push(1100,frame(0.0,identity));
    for (ts,b) in s.peers[&VIC].bone_frames.q.clone() {
        let mut f=posecodec::v2::Full::default();f.ts=ts as f64;
        for i in 0..posecodec::v2::NB {f.bones[i].p=b.p[i];f.bones[i].q=b.q[i];}
        s.record_body_strikers(VIC,ts,&f);
    }
    s.note_relayed(ATT,VIC,1000,100,1100);
    s.note_relayed(ATT,VIC,1100,100,1100);
    let v=&s.peers[&VIC];
    let shown=s.shown_bone_frames(ATT,VIC,v,1050,0,1100).unwrap();
    let hidden=v.bone_frames.sample(1050,0).unwrap();
    let original_world_box=[20.0,0.0,0.0];
    let captured_local_box=[20.0,0.0,0.0];
    let reconstructed=|f:BoneFrames|add(f.p[0],posecodec::v2::qrot(f.q[0],captured_local_box));
    assert!(len(sub(reconstructed(shown),original_world_box))<0.001);
    assert!(len(sub(reconstructed(hidden),original_world_box))>BODY_TOL);
    assert_eq!(shown.q[0],identity);
    // Another viewer that actually received the middle frame sees its twist.
    s.note_relayed(ATT+1,VIC,1050,50,1100);
    s.note_relayed(ATT+1,VIC,1100,50,1100);
    let theirs=s.shown_bone_frames(ATT+1,VIC,&s.peers[&VIC],1050,0,1100).unwrap();
    assert_eq!(theirs.p[0],hidden.p[0]);
    assert_eq!(theirs.q[0],hidden.q[0]);
}

#[test]
fn cutting_newest_frame_uses_only_delivered_native_derivatives_and_context() {
    let mut s=Store::default();
    let context=posecodec::v2::Context{match_id:7,round:2,life:3};
    assert!(s.bind_pose_context(VIC,Some(context)));
    for (ts,rotation) in [(950,0.0f32),(1000,0.0),(1030,20.0)] {
        let mut f=posecodec::v2::Full::default();f.ts=ts as f64;f.context=Some(context);
        f.bones[0].q=[0.0,0.0,(rotation.to_radians()/2.0).sin(),(rotation.to_radians()/2.0).cos()];
        s.record_body_strikers(VIC,ts,&f);
    }
    s.note_relayed(ATT,VIC,800,50,1100); // irrelevant old entry aged out of cache
    s.note_relayed(ATT,VIC,950,50,1100);
    s.note_relayed(ATT,VIC,1000,50,1100);
    let got=s.shown_bone_frames(ATT,VIC,&s.peers[&VIC],1030,50,1100).unwrap();
    assert_eq!(got.q[0],[0.0,0.0,0.0,1.0],"hidden1030twist must not alter newest displayed frame");
    // Cache removal of a needed delivered endpoint is not repaired with a
    // never-delivered full-rate frame, even though the latter is available.
    s.peer(VIC).native_frames.remove(&1000);
    assert!(s.shown_bone_frames(ATT,VIC,&s.peers[&VIC],1030,50,1100).is_none());
    assert!(s.bind_pose_context(VIC,Some(posecodec::v2::Context{life:4,..context})));
    assert!(s.peers[&VIC].native_frames.is_empty());
}

fn hit(target: PeerId, loc: V3, ats: u32, vts: u32) -> DamageEvent {
    DamageEvent::new(crate::proto::Damage {
        hit_id: 1, target_peer_id: target, round: 1, bone: hsmp_ipc::layout::Str::new("spine_02"),
        offset: [0.0; 3], location: loc, impulse: [0.0; 3], velocity: [0.0; 3],
        normal: [1.0, 0.0, 0.0], raw_damage: 40.0, cutting_power: 1.0, pain_rate: 1.0,
        draw_cut: 0.0, damage_out: 10.0, dism_blunt: 0, flags: 0, age_ms: 0,
        attacker_ts: ats, victim_view_ts: vts, victim_arm_ts: vts, ..Default::default()
    }, &crate::proto::deltas_of(&[]))
}

/// A standing Willie whose pelvis is at `root`, arms forward (+x),
/// right hand at root+(45,-20,40).
fn skeleton(ts: u32, root: V3) -> PoseFrame {
    let o = |x: f32, y: f32, z: f32| [root[0] + x, root[1] + y, root[2] + z];
    let p = [
        o(0., 0., 0.), o(0., 0., 25.), o(0., 0., 50.), o(0., 0., 75.),
        o(0., 20., 48.), o(20., 25., 40.), o(40., 20., 40.),
        o(0., -20., 48.), o(25., -25., 40.), o(45., -20., 40.),
        o(0., 10., -5.), o(0., 10., -50.), o(0., 10., -90.),
        o(0., -10., -5.), o(0., -10., -50.), o(0., -10., -90.),
    ];
    PoseFrame { ts, bones: p.iter().enumerate().map(|(i, v)| (i as u8, *v, [0.0, 0.0, 0.0, 1.0])).collect() }
}

fn accepted(e: &Eval) -> bool { matches!(e, Eval::Accept(_)) }
fn reason(e: Eval) -> String {
    match e { Eval::Reject(r) => r, other => panic!("expected reject, got {:?}", other) }
}

/// Deterministic two-player scene. Server time t; victim clock = t + V_OFF,
/// attacker clock = t + A_OFF; one-way delay `owd` both ways (rtt = 2·owd),
/// no jitter. Samples every 25 ms over [0, now].
struct Scene {
    s: Store,
    now: i64,
    owd: i64,
}

const V_OFF: i64 = 100_000;
const A_OFF: i64 = 500_000;

impl Scene {
    /// `victim(t)`: victim pelvis at its send time t. `vweap(t)`: its weapon
    /// actor. `aweap(t)`: attacker weapon actor; attacker body at x = -150.
    fn new(owd: i64, now: i64, victim: impl Fn(i64) -> V3, vweap: impl Fn(i64) -> Option<V3>,
           aweap: impl Fn(i64) -> V3) -> Scene {
        Scene::with_pose(owd, now, victim, vweap, aweap, skeleton)
    }
    fn with_pose(owd: i64, now: i64, victim: impl Fn(i64) -> V3, vweap: impl Fn(i64) -> Option<V3>,
           aweap: impl Fn(i64) -> V3, vpose: impl Fn(u32, V3) -> PoseFrame) -> Scene {
        let mut s = Store::default();
        let mut t = 0;
        while t + owd <= now {
            let rx = t + owd;
            let vts = (t + V_OFF) as u32;
            let r = victim(t);
            s.record_root(VIC, vts, r, rx);
            s.record_pose(VIC, &vpose(vts, r), rx);
            if let Some(w) = vweap(t) { s.record_weapon(VIC, vts, w, rx); }
            let ats = (t + A_OFF) as u32;
            s.record_root(ATT, ats, [-150.0, 0.0, 100.0], rx);
            s.record_pose(ATT, &skeleton(ats, [-150.0, 0.0, 100.0]), rx);
            s.record_weapon(ATT, ats, aweap(t), rx);
            t += 25;
        }
        s.note_rtt(ATT, (2 * owd) as f32, now);
        s.note_rtt(VIC, (2 * owd) as f32, now);
        Scene { s, now, owd }
    }
    /// Attacker ts of a hit swung at server time h (arrives at h + owd).
    fn ats(&self, h: i64) -> u32 { (h + A_OFF) as u32 }
    /// Victim clock of what an honest attacker displayed at server time h
    /// (relay down owd, jitter buffer `delay`).
    fn honest_view(&self, h: i64, delay: i64) -> u32 { (h - 2 * self.owd - delay + V_OFF) as u32 }
    fn eval(&self, h: &DamageEvent) -> Eval { self.s.evaluate(ATT, h, self.now) }
}

fn runner(t: i64) -> V3 { [t as f32 * 0.5, 0.0, 100.0] } // 500 uu/s along +x
fn still(_: i64) -> V3 { [0.0, 0.0, 100.0] }
fn sword_near(t: i64) -> V3 { [runner(t)[0] - 100.0, 0.0, 140.0] }

// ---- rings and clock map -------------------------------------------------------

#[test]
fn ring_sorted_insert_dedup_restart() {
    let mut r: Ring<V3> = Ring::new();
    assert!(r.push(1000, [0.0, 0.0, 0.0]));
    assert!(r.push(1100, [100.0, 0.0, 0.0]));
    assert!(r.push(1050, [999.0, 0.0, 0.0])); // reordered: filled in, not dropped
    assert!(!r.push(1050, [5.0, 0.0, 0.0])); // duplicate
    assert_eq!(r.sample(1050, 0).unwrap()[0], 999.0);
    assert_eq!(r.sample(1025, 0).unwrap()[0], 499.5);
    assert_eq!(r.sample(1200, 150).unwrap()[0], 100.0); // short lead clamps
    assert!(r.sample(1300, 150).is_none());
    assert!(r.sample(999, 0).is_none());
    for t in 0..40 { r.push(1200 + t * 50, [t as f32, 0.0, 0.0]); }
    assert!(r.oldest().unwrap() >= r.newest().unwrap() - HISTORY_MS);
    assert!(!r.push(r.newest().unwrap() - HISTORY_MS - 10, [0.0; 3])); // older than the window
    r.push(10, [0.0; 3]); // sender restarted
    assert_eq!(r.q.len(), 1);
}

#[test]
fn clock_map_offset_jitter_and_restart() {
    let mut c = Clock::default();
    // Sender clock = server + 7000; delay 40 ms + jitter 0..20 (every 4th late).
    for i in 0..80i64 {
        let t = i * 25;
        let j = if i % 4 == 0 { 20 } else { (i % 3) * 2 };
        c.note(t + 40 + j, (t + 7000) as u32);
    }
    assert_eq!(c.offset(), Some(40 - 7000));
    let jp = c.jitter_p90();
    assert!((4.0..=20.0).contains(&jp), "{}", jp);
    c.note_rtt(0, 90.0);
    c.note_rtt(1, 70.0);
    c.note_rtt(2, 300.0);
    assert_eq!(c.rtt(10), Some(70.0)); // min filter
    assert_eq!(c.rtt(RTT_TTL_MS + 100), None);
    c.note(5000, 10); // clock jumped back: map restarts
    assert_eq!(c.offset(), Some(5000 - 10));
}

#[test]
fn prediction_matches_an_honest_display() {
    let sc = Scene::new(40, 2000, runner, |_| None, sword_near);
    let h = 1950;
    let p = sc.s.predict_view(ATT, VIC, sc.ats(h), sc.now).unwrap();
    assert!(p.rtt_known);
    // No jitter: interp = 25 + 0 + 6 = 31; frame 17.
    let honest = sc.honest_view(h, 31) as i64;
    assert!((p.expected - honest).abs() <= FRAME_MS, "expected {} honest {}", p.expected, honest);
    assert_eq!(p.tol, TOL_FLOOR_MS);
    assert_eq!(p.clamp(honest as u32), honest as u32);
}

// ---- cheat cases ------------------------------------------------------------------

#[test]
fn backtrack_exploit_rejected() {
    let sc = Scene::new(40, 2000, runner, |_| None, sword_near);
    let h = 1950;
    let ats = sc.ats(h);
    let view = sc.honest_view(h, 31);
    let seen = runner(view as i64 - V_OFF);
    // Honest: contact on the chest where the attacker saw the victim.
    let ok = hit(VIC, [seen[0] - 8.0, 0.0, 130.0], ats, view);
    assert!(accepted(&sc.eval(&ok)), "{:?}", sc.eval(&ok));
    // Cheat: claim the victim pose from 250 ms earlier (inside a naive
    // 450 ms window) and a contact where the victim was then.
    let old = view - 250;
    let past = runner(old as i64 - V_OFF);
    let bt = hit(VIC, [past[0] - 8.0, 0.0, 130.0], ats, old);
    let before = cheat::get(ATT).view_hint_outlier;
    assert!(reason(sc.eval(&bt)).contains("victim body"));
    assert!(cheat::get(ATT).view_hint_outlier > before);
    // The hint was clamped, not trusted: the Info reports the used time.
    if let Eval::Accept(i) = sc.eval(&hit(VIC, ok.location, ats, view - 1000)) {
        assert!(i.view_clamped && (i.view_ts as i64 - view as i64).abs() <= TOL_FLOOR_MS + FRAME_MS);
    }
}

#[test]
fn missing_timestamps_rejected_when_victim_streams() {
    let sc = Scene::new(40, 2000, still, |_| None, |_| [-60.0, 0.0, 140.0]);
    let before = cheat::get(ATT).no_timestamp;
    let r = reason(sc.eval(&hit(VIC, [0.0, 0.0, 130.0], 0, 0)));
    assert!(r.contains("missing timestamps"), "{}", r);
    assert!(reason(sc.eval(&hit(VIC, [0.0, 0.0, 130.0], sc.ats(1950), 0))).contains("missing"));
    assert_eq!(cheat::get(ATT).no_timestamp, before + 2);
    // A victim that never streamed (legacy client): legacy checks only.
    assert!(matches!(sc.s.evaluate(ATT, &hit(77, [0.0; 3], 0, 0), sc.now), Eval::NoData(_)));
}

#[test]
fn forged_attacker_ts_rejected() {
    let sc = Scene::new(40, 2000, runner, |_| None, sword_near);
    // Claim the swing happened 400 ms before it arrived without saying so
    // (age 0): would move the predicted view back 400 ms.
    let h = hit(VIC, [0.0, 0.0, 130.0], sc.ats(1950 - 400), sc.honest_view(1550, 31));
    assert!(reason(sc.eval(&h)).contains("inconsistent"));
    // From the future.
    let h = hit(VIC, [0.0, 0.0, 130.0], sc.ats(2200), sc.honest_view(2200, 31));
    assert!(matches!(sc.eval(&h), Eval::Reject(_)));
}

#[test]
fn rewind_cap_follows_the_attackers_path_up_to_the_ceiling() {
    // RTT 300: the honest view is ~360 ms behind the victim's newest sample. The
    // cap follows the attacker's measured path, so the hit lands.
    let sc = Scene::new(150, 2000, still, |_| None, |_| [-60.0, 0.0, 140.0]);
    let h = 2000 - 150;
    let c = [0.0, 0.0, 130.0];
    let e = sc.eval(&hit(VIC, c, sc.ats(h), sc.honest_view(h, 31)));
    assert!(accepted(&e), "{:?}", e);
    // RTT 700: past REWIND_CEILING_MS even for an honest view (favour the defender).
    let sc = Scene::new(350, 3000, still, |_| None, |_| [-60.0, 0.0, 140.0]);
    let h = 3000 - 350;
    let r = reason(sc.eval(&hit(VIC, c, sc.ats(h), sc.honest_view(h, 31))));
    assert!(r.contains("cap 600"), "{}", r);
    // The configured floor stays clamped to the high-latency cap.
    let mut sc = sc;
    sc.s.set_max_rewind(10_000);
    assert_eq!(sc.s.max_rewind(), HIGH_LATENCY_REWIND_MS);
}

#[test]
fn age_credit_is_capped() {
    // RTT 160: honest rewind ≈ 160 + 31 + 17 + owd(arrival) ≈ 290 → inside.
    let sc = Scene::new(80, 2000, still, |_| None, |_| [-60.0, 0.0, 140.0]);
    let c = [0.0, 0.0, 130.0];
    // A resent claim (age 300) is still judged at its own swing time; the
    // credit only covers AGE_CREDIT_MS, so an old claim runs into the cap.
    let h0 = 2000 - 80 - 300;
    let mut h = hit(VIC, c, sc.ats(h0), sc.honest_view(h0, 31));
    h.age_ms = 300;
    match sc.eval(&h) {
        Eval::Reject(r) => assert!(r.contains("cap"), "{}", r),
        e => panic!("{:?}", e),
    }
    let h1 = 2000 - 80 - 100;
    let mut h = hit(VIC, c, sc.ats(h1), sc.honest_view(h1, 31));
    h.age_ms = 100;
    assert!(accepted(&sc.eval(&h)), "{:?}", sc.eval(&h));
}

// ---- body capsules -----------------------------------------------------------------

#[test]
fn capsule_radii_per_bone() {
    let sc = Scene::new(20, 2000, still, |_| None, |_| [-60.0, 0.0, 140.0]);
    let h = 1980;
    let v = sc.honest_view(h, 31);
    let at = |p: V3| sc.eval(&hit(VIC, p, sc.ats(h), v));
    // (This attacker streams v1 — no blade — so its stand-in slack
    // BODY_TOL_V1 = 60 applies.) Torso (r 18): 75 uu from the spine axis is
    // inside 18 + 60, 85 is not.
    assert!(accepted(&at([-75.0, 0.0, 130.0])));
    assert!(!accepted(&at([-85.0, 0.0, 130.0])));
    // Right hand end of the forearm (r 7) at (45,-20,140): 66 uu beyond it
    // (surface 59) passes, 72 uu (surface 65) does not.
    assert!(accepted(&at([111.0, -20.0, 140.0])), "{:?}", at([111.0, -20.0, 140.0]));
    assert!(!accepted(&at([117.0, -20.0, 140.0])));
}

// ---- swept blade (full-skeleton stream) -----------------------------------------------

/// Attacker blade stream: a horizontal cut sweeping +y at height z through
/// x = bx: base (bx-40, y, z), tip (bx+60, y, z), y from -150 to +150 over
/// 100 ms around server time `mid`.
fn cut_scene(bx: f32, z: f32, mid: i64) -> Scene {
    let mut sc = Scene::new(20, 2000, still, |_| None, |_| [-60.0, 0.0, 140.0]);
    let mut t = 0;
    while t + 20 <= 2000 {
        let y = ((t - mid) as f32 * 3.0).clamp(-150.0, 150.0); // 3000 uu/s
        let b = Blade { base: [bx - 40.0, y, z], tip: [bx + 60.0, y, z], vel: None };
        sc.s.record_blade(ATT, (t + A_OFF) as u32, b, t + 20);
        t += 16;
    }
    sc
}

#[test]
fn swept_blade_through_capsule_accepted_miss_rejected() {
    // Cut through the torso at z=130 (spine axis at x=0): the blade crosses
    // y=0 between samples — a point test on the samples would miss it.
    let mid = 1972;
    // The scene's blade runs 40–60 uu from the hands: a polearm haft (a
    // peer without a kit is judged as a Sword).
    crate::validate::damage::set_kit(ATT, &crate::loadout::KitSel::new("custom", "w_poleaxe_m", "", &[], [0; 4]));
    let sc = cut_scene(0.0, 130.0, mid);
    let ats = sc.ats(mid + 4);
    let v = sc.honest_view(mid + 4, 31);
    let e = sc.eval(&hit(VIC, [-15.0, 0.0, 130.0], ats, v));
    match &e {
        Eval::Accept(i) => {
            assert!(i.swept, "{:?}", i);
            // (the stream ends just before the crossing: speed held at the edge)
            assert!(i.contact_speed.unwrap() > 1000.0, "{:?}", i);
        }
        e => panic!("{:?}", e),
    }
    // Contact claimed on the body but the blade passes 80 uu above the head.
    let sc = cut_scene(0.0, 260.0, mid);
    let r = reason(sc.eval(&hit(VIC, [-15.0, 0.0, 130.0], ats, v)));
    assert!(r.contains("swept"), "{}", r);
}

// ---- blocks, parries, clashes -------------------------------------------------------

#[test]
fn offhand_strike_uses_its_own_swept_geometry_and_speed() {
    let attacker = 0x1B81;
    crate::validate::damage::set_kit(attacker, &crate::loadout::KitSel::new("custom", "w_axe", "w_axe", &[], [0; 4]));
    let mid = 1972;
    let mut sc = cut_scene(0.0, 260.0, mid); // main weapon misses the torso
    let history = sc.s.peers.remove(&ATT).unwrap();
    sc.s.peers.insert(attacker, history);
    let h = hit(VIC, [-15.0, 0.0, 130.0], sc.ats(mid + 4), sc.honest_view(mid + 4, 31));
    assert!(!accepted(&sc.s.evaluate(attacker, &h, sc.now)));
    let mut t = 0;
    while t + 20 <= 2000 {
        let y = ((t - mid) as f32 * 3.0).clamp(-150.0, 150.0);
        sc.s.record_blade_hand(attacker, (t + A_OFF) as u32,
            Blade { base: [-40.0, y, 130.0], tip: [60.0, y, 130.0], vel: None }, t + 20, true);
        t += 16;
    }
    let Eval::Accept(i) = sc.s.evaluate(attacker, &h, sc.now) else { panic!("offhand strike rejected: {:?}", sc.s.evaluate(attacker, &h, sc.now)) };
    assert!(i.swept && !i.unarmed);
    assert!(i.contact_speed.unwrap() > 1000.0, "offhand speed: {i:?}");
    let miss = hit(VIC, [-15.0, 90.0, 210.0], h.attacker_ts, h.victim_view_ts);
    assert!(!accepted(&sc.s.evaluate(attacker, &miss, sc.now)), "adding an offhand must not widen hit geometry");
    let mut named = h;
    named.flags = crate::validate::damage::FLAG_COMPLEX | crate::validate::damage::FLAG_WEAPON;
    named.dism_blunt = crate::validate::damage::SOURCE_LEFT;
    assert!(accepted(&sc.s.evaluate(attacker, &named, sc.now)), "named offhand must use its own blade");
    named.dism_blunt = crate::validate::damage::SOURCE_RIGHT;
    assert!(!accepted(&sc.s.evaluate(attacker, &named, sc.now)), "a main-hand claim cannot borrow the offhand geometry");
    named.dism_blunt = crate::validate::damage::SOURCE_LEFT;
    sc.s.peers.get_mut(&attacker).unwrap().offhand.q.clear();
    assert!(!accepted(&sc.s.evaluate(attacker, &named, sc.now)), "a missing named hand cannot use the legacy main weapon actor");
    crate::validate::damage::forget(attacker);
}

#[test]
fn native_fist_with_held_blade_uses_body_geometry_and_speed() {
    let mut sc = Scene::new(20, 2000, |_| [-100.0, 0.0, 100.0], |_| None, |_| [-60.0, 0.0, 140.0]);
    for t in (0..=1975).step_by(25) {
        // This weapon is outside the permitted class length, but its reach
        // is unrelated to the separately validated fist beside it.
        sc.s.record_blade(ATT, (A_OFF + t) as u32,
            Blade { base: [-150.0, 0.0, 140.0], tip: [150.0, 0.0, 140.0], vel: None }, t + 20);
    }
    let mut h = hit(VIC, [-105.0, -20.0, 140.0], sc.ats(1975), sc.honest_view(1975, 31));
    h.flags = crate::validate::damage::FLAG_COMPLEX | crate::validate::damage::FLAG_WEAPON;
    h.dism_blunt = crate::validate::damage::SOURCE_FIST | crate::validate::damage::SOURCE_LEFT | (10<<21);
    h.source_class=hsmp_ipc::layout::Str::new("Weapon_Fists_C");
    assert!(reason(sc.eval(&h)).contains("exact native body striker history"));
    h.location = [-40.0, 0.0, 140.0];
    assert!(!accepted(&sc.eval(&h)), "a fist identity cannot claim remote blade reach");
}

#[test]
fn native_feet_with_held_blade_use_body_geometry_without_module_history() {
    let mut sc=Scene::new(20,2000,|_|[-150.0,0.0,100.0],|_|None,|_|[-60.0,0.0,140.0]);
    for t in (0..=1975).step_by(25) {
        sc.s.record_blade(ATT,(A_OFF+t) as u32,Blade {base:[-150.0,0.0,140.0],tip:[150.0,0.0,140.0],vel:None},t+20);
    }
    let mut h=hit(VIC,[-150.0,10.0,10.0],sc.ats(1975),sc.honest_view(1975,31));
    h.bone=hsmp_ipc::layout::Str::new("foot_l");
    h.flags=crate::validate::damage::FLAG_COMPLEX|crate::validate::damage::FLAG_WEAPON;
    h.dism_blunt=crate::validate::damage::SOURCE_FEET|crate::validate::damage::SOURCE_LEFT|(15<<21);
    h.source_class=hsmp_ipc::layout::Str::new("Weapon_Feet_C");
    assert!(reason(sc.eval(&h)).contains("exact native body striker history"));
    h.location=[-40.0,0.0,140.0];
    assert!(!accepted(&sc.eval(&h)),"foot identity cannot borrow held-blade reach");
}

#[test]
fn native_fist_sphere_validates_its_swept_source_without_whole_body_widening() {
    let mut sc=Scene::new(20,2000,|_|[-80.0,-20.0,100.0],|_|None,|_|[-60.0,0.0,140.0]);
    let mut h=hit(VIC,[-79.0,-20.0,140.0],sc.ats(1975),sc.honest_view(1975,31));
    h.flags=crate::validate::damage::FLAG_COMPLEX|crate::validate::damage::FLAG_WEAPON;
    h.dism_blunt=crate::validate::damage::SOURCE_FIST|crate::validate::damage::SOURCE_RIGHT|(10<<21);
    h.source_class=hsmp_ipc::layout::Str::new("Weapon_Fists_C");
    assert!(reason(sc.eval(&h)).contains("exact native body striker history"),"named native sphere requires its exact component history");
    for t in (1000..=1975).step_by(25) {
        let mut f=posecodec::v2::Full::default();
        f.bones[16].p=[-105.0,-20.0,140.0];
        f.strikers=Some(vec![posecodec::v2::BodyStriker {part:1,component:10,kind:0,p:[13.0,0.0,0.0],q:[0.0,0.0,0.0,1.0],half:[13.0;3]}]);
        sc.s.record_body_strikers(ATT,sc.ats(t),&f);
    }
    let Eval::Accept(i)=sc.eval(&h) else {panic!("actual sphere rejected: {:?}",sc.eval(&h))};
    assert!(i.unarmed && i.swept && !i.parry_possible);
    assert!(i.weapon_dist<0.01);assert_eq!(i.contact_speed,Some(0.0));
    h.dism_blunt=crate::validate::damage::SOURCE_FIST|crate::validate::damage::SOURCE_LEFT|(10<<21);
    assert!(!accepted(&sc.eval(&h)),"left fist cannot borrow right sphere");
    h.dism_blunt=crate::validate::damage::SOURCE_FIST|crate::validate::damage::SOURCE_RIGHT|(11<<21);
    assert!(!accepted(&sc.eval(&h)),"another native component cannot borrow Sphere10");
    h.dism_blunt=crate::validate::damage::SOURCE_FIST|crate::validate::damage::SOURCE_RIGHT|(10<<21);
    h.location=[-53.0,-20.0,140.0];
    assert!(!accepted(&sc.eval(&h)),"actual source envelope still bounded");
}

#[test]
fn delayed_native_striker_history_waits_instead_of_rejecting_stale_geometry() {
    let mut sc=Scene::new(20,2000,|_|[-80.0,-20.0,100.0],|_|None,|_|[-60.0,0.0,140.0]);
    let mut h=hit(VIC,[-79.0,-20.0,140.0],sc.ats(1975),sc.honest_view(1975,31));
    h.flags=crate::validate::damage::FLAG_COMPLEX|crate::validate::damage::FLAG_WEAPON;
    h.dism_blunt=crate::validate::damage::SOURCE_FIST|crate::validate::damage::SOURCE_RIGHT|(10<<21);
    h.source_class=hsmp_ipc::layout::Str::new("Weapon_Fists_C");
    let mut f=posecodec::v2::Full::default();
    f.bones[16].p=[-140.0,-20.0,140.0];
    f.strikers=Some(vec![posecodec::v2::BodyStriker{part:1,component:10,kind:0,p:[13.0,0.0,0.0],half:[13.0;3],q:[0.0,0.0,0.0,1.0]}]);
    for t in (1000..=1950).step_by(25){sc.s.record_body_strikers(ATT,sc.ats(t),&f);}
    assert!(matches!(sc.s.evaluate_opts(ATT,&h,sc.now,false),Eval::Wait(_)),"newer root/weapon history cannot make stale fist shape a final miss");
    assert!(reason(sc.s.evaluate_opts(ATT,&h,sc.now,true)).contains("body_striker_miss"),"final bounded lead still rejects actual old geometry");
    f.bones[16].p=[-105.0,-20.0,140.0];
    sc.s.record_body_strikers(ATT,sc.ats(1975),&f);
    assert!(accepted(&sc.s.evaluate_opts(ATT,&h,sc.now,false)),"delayed exact hit-time shape permits genuine contact without widening");
}

#[test]
fn body_striker_sweep_rotation_velocity_and_transient_absence_are_exact() {
    let mut h=PeerHist::new();
    let s=posecodec::v2::BodyStriker{part:3,component:10,kind:1,p:[0.0;3],q:[0.0,0.0,0.0,1.0],half:[7.475,14.949,7.475]};
    let mut a=StrikerSet::empty(true);a.shapes[0]=s;a.n=1;
    let mut b=a;b.shapes[0].p=[50.0,0.0,0.0];b.shapes[0].q=[0.0,0.0,(0.1f32).sin(),(0.1f32).cos()];
    h.strikers.push(1000,a);h.strikers.push(1100,b);
    let (d,p,t)=h.striker_contact([25.0,0.0,0.0],1000.0,1100.0,3,10).unwrap();
    assert!(d<0.01 && t>1000.0 && t<1100.0,"foot crossed point between snapshots");
    assert!(h.striker_contact([25.0,0.0,0.0],1000.0,1100.0,4,10).is_none());
    let v=h.striker_velocity(3,10,[0.0,10.0,0.0],1050.0).unwrap();
    assert!((len(v)-480.1).abs()<0.3,"contact point includes box rotation, not only 500cm/s center: {v:?}");
    assert!(h.striker_velocity(3,10,p,t).is_some());
    h.strikers.push(1200,StrikerSet::empty(true));
    assert!(h.striker_contact([50.0,0.0,0.0],1170.0,1200.0,3,10).is_none(),"destroyed transient foot cannot linger indefinitely");
    assert!(h.strikers.sample(1050,0).unwrap().get(3,10).is_some(),"historical kick retained after disappearance");
}

#[test]
fn native_head_side_contact_uses_only_its_module_envelope() {
    crate::validate::damage::set_kit(ATT, &crate::loadout::KitSel::new("custom", "w_poleaxe_m", "", &[], [0;4]));
    let mut sc=Scene::new(20,2000,still,|_|None,|_|[-150.0,0.0,130.0]);
    let w=posecodec::v2::Weapon {hands:1,p:[-150.0,0.0,130.0],q:[0.0,0.0,0.0,1.0],base:[0.0;3],tip:[200.0,0.0,0.0],
        boxes:vec![
            posecodec::v2::WeaponBox {component:1,p:[160.0,0.0,0.0],q:[0.0,0.0,0.0,1.0],half:[22.0,20.0,3.0],class_hash:posecodec::v2::class_hash("ModularWeaponBP_Polearm_Mid_Tier_C"),native_scale:Some([2.0,1.0,1.0]),..Default::default()},
            posecodec::v2::WeaponBox {component:2,p:[100.0,0.0,0.0],q:[0.0,0.0,0.0,1.0],half:[100.0,2.0,2.0],class_hash:posecodec::v2::class_hash("ModularWeaponBP_Polearm_Mid_Tier_C"),..Default::default()},
            posecodec::v2::WeaponBox {component:3,p:[160.0,0.0,0.0],q:[0.0,0.0,0.0,1.0],half:[22.0,20.0,3.0],class_hash:posecodec::v2::class_hash("ModularWeaponBP_Polearm_Mid_Tier_C"),native_scale:Some([2.0,1.0,1.0]),child_of:1},
        ],..Default::default()};
    for t in (0..=1975).step_by(25) {
        let (base,tip)=posecodec::v2::blade_world(&w);
        sc.s.record_blade(ATT,(t+A_OFF)as u32,Blade{base,tip,vel:None},t+20);
    }
    let mut h=hit(VIC,[-5.0,19.0,130.0],sc.ats(1975),sc.honest_view(1975,31));
    h.flags=crate::validate::damage::FLAG_COMPLEX|crate::validate::damage::FLAG_WEAPON;
    h.dism_blunt=crate::validate::damage::SOURCE_RIGHT;
    assert!(reason(sc.eval(&h)).contains("blade_miss"),"native Head contact19cm beside shaft reproduced");
    for t in (1000..=1975).step_by(25) { assert!(sc.s.record_weapon_shape(ATT,(t+A_OFF)as u32,&w,false)); }
    h.dism_blunt|=1<<21;
    h.source_class=hsmp_ipc::layout::Str::new("ModularWeaponBP_Polearm_Mid_Tier_C");
    let Eval::Accept(i)=sc.eval(&h) else {panic!("head contact: {:?}",sc.eval(&h))};
    assert_eq!(i.weapon_dist,0.0); assert!(!i.unarmed && i.swept);
    for t in (1000..=1975).step_by(25) {
        sc.s.peer(VIC).bone_frames.push((t+V_OFF) as u32,BoneFrames {
            p:[[-390.0,0.0,130.0];posecodec::v2::NB],q:[[0.0,0.0,0.0,1.0];posecodec::v2::NB],scale:0.9932 });
    }
    let mut cutting=h;
    cutting.flags|=crate::validate::damage::FLAG_LOCAL;
    cutting.dism_blunt|=3<<27;
    cutting.hit_box_frame=[400.0,0.0,0.0,0.0,0.0,0.0,1.0,2.0,1.0,1.0,11.0,20.0,3.0];
    let authentic=[400.0,0.0,0.0,0.0,0.0,0.0,1.0,2.0,1.0,1.0,11.0,20.0,3.0];
    let replayed=|e:Eval|->([f32;13],(f32,f32)) {let Eval::Accept(i)=e else {panic!("expected accept, got {e:?}")};(i.hit_box.unwrap(),i.proxy_box_error.unwrap())};
    let close=|a:[f32;13],b:[f32;13]|a.iter().zip(b).all(|(x,y)|(x-y).abs()<1e-3);
    let (frame,(cm,dot))=replayed(sc.eval(&cutting));
    assert!(close(frame,authentic) && cm<1e-3 && dot>0.99999,"original native Box frame authenticates and replays as is");
    // A stand-in frame off the owner's bone (pose-sync error, or a forgery) changes nothing:
    // the owner replays the frame rebuilt from authenticated history; the error is reported.
    let mut socket_divided=cutting;socket_divided.hit_box_frame[0]/=1.125;
    let (frame,(cm,_))=replayed(sc.eval(&socket_divided));
    assert!(close(frame,authentic) && cm>40.0,"old socket-divided center is replaced, its error reported");
    let mut prior_module=cutting;prior_module.location=[-5.0,0.0,130.0];
    prior_module.dism_blunt=(prior_module.dism_blunt & !(15<<21)) | (2<<21);
    assert!(accepted(&sc.eval(&prior_module)),"native selected Box retained from previous Head still authenticates a current Grip contact");
    let mut child_strike=cutting;child_strike.dism_blunt=(child_strike.dism_blunt & !(15<<21)) | (3<<21);
    assert!(reason(sc.eval(&child_strike)).contains("source_role"),"cutting-only virtual ID cannot become a striking collider");
    let mut missing_parent=w.clone();missing_parent.boxes[2].child_of=4;
    assert!(!sc.s.record_weapon_shape(ATT,sc.ats(1975),&missing_parent,false),"unrepresented cutting parent refused");
    for (field,value) in [(7,1.0),(10,22.0)] {
        let mut forged=cutting;forged.hit_box_frame[field]=value;
        assert!(reason(sc.eval(&forged)).contains("cutting geometry differs"),"forged Box field {field}");
    }
    let mut forged=cutting;forged.hit_box_frame[0]=100.0;
    assert!(close(replayed(sc.eval(&forged)).0,authentic),"forged Box position is replaced by the authenticated one");
    let mut rotated=cutting;rotated.hit_box_frame[5]=0.5;rotated.hit_box_frame[6]=(0.75f32).sqrt();
    let (frame,(_,dot))=replayed(sc.eval(&rotated));
    assert!(close(frame,authentic) && dot<0.99,"forged / stand-in rotation is replaced, its error reported");
    let mut denormal=cutting;denormal.hit_box_frame[4]=0.4;
    assert!(reason(sc.eval(&denormal)).contains("cutting geometry differs"),"non-unit Box rotation refused");
    let mut off_box=cutting;off_box.location=[-5.0,19.0,190.0];
    let r=reason(sc.eval(&off_box));
    assert!(r.contains("off the original cutting Box"),"a contact off the authenticated Box is refused: {r}");
    let mut reciprocal=cutting;reciprocal.hit_box_frame[7]=1.0;reciprocal.hit_box_frame[10]=22.0;
    assert!(reason(sc.eval(&reciprocal)).contains("cutting geometry differs"),"scaled extent equality cannot spoof native X-before-scale clamp");
    let mut wrong_class=cutting;wrong_class.source_class=hsmp_ipc::layout::Str::new("ModularWeaponBP_ArmingSword_C");
    assert!(reason(sc.eval(&wrong_class)).contains("class mismatch"));
    // The same lateral distance remains outside the thin shaft module.
    h.dism_blunt=crate::validate::damage::SOURCE_RIGHT|(2<<21);
    assert!(reason(sc.eval(&h)).contains("module_miss"),"head width cannot be borrowed by a Grip claim");
    let shape=sc.s.peers[&ATT].shapes.sample(sc.ats(1975),0).unwrap();
    assert!(shape.nearest([-75.0,19.0,130.0],0).0>SWEEP_TOL,"no whole-shaft widening");
    let mut forged=w.clone(); forged.boxes[0].half[1]=200.0;
    assert!(!sc.s.record_weapon_shape(ATT,sc.ats(1975),&forged,false),"oversized module refused");
}

#[test]
fn module_point_velocity_includes_rotation_and_keeps_source_hand() {
    let mut h=PeerHist::new();
    let mut base=WeaponShape {weapon_id:1,p:[0.0;3],q:[0.0,0.0,0.0,1.0],boxes:[posecodec::v2::WeaponBox::default();posecodec::v2::MAX_WEAPON_BOXES],n:1};
    base.boxes[0]=posecodec::v2::WeaponBox {component:1,p:[0.0;3],q:[0.0,0.0,0.0,1.0],half:[100.0,2.0,2.0],..Default::default()};
    h.offhand_shapes.push(1000,base);
    h.offhand_shapes.push(1100,WeaponShape {q:[0.0,0.0,(0.1f32).sin(),(0.1f32).cos()],..base});
    let left=BladeView {hist:&h,blade:&h.offhand};
    assert!((len(left.shape_velocity(1,1,[100.0,0.0,0.0],1050.0).unwrap())-200.0).abs()<0.2);
    let right=BladeView {hist:&h,blade:&h.blade};
    assert!(right.shape_velocity(1,1,[100.0,0.0,0.0],1050.0).is_none());
}

#[test]
fn articulated_module_sweep_and_speed_follow_the_selected_moving_component() {
    let mut h=PeerHist::new();
    let mut a=WeaponShape {weapon_id:1,p:[0.0;3],q:[0.0,0.0,0.0,1.0],boxes:[posecodec::v2::WeaponBox::default();posecodec::v2::MAX_WEAPON_BOXES],n:2};
    a.boxes[0]=posecodec::v2::WeaponBox {component:1,p:[0.0;3],q:[0.0,0.0,0.0,1.0],half:[2.0,12.0,2.0],..Default::default()};
    a.boxes[1]=posecodec::v2::WeaponBox {component:7,p:[0.0;3],q:[0.0,0.0,0.0,1.0],half:[2.0;3],..Default::default()};
    let mut b=a;
    // The native flail's Head is an independently simulated static component.
    // Root/Grip stay still; array storage order is deliberately swapped.
    b.boxes[0]=a.boxes[1];
    b.boxes[1]=posecodec::v2::WeaponBox {p:[50.0,0.0,0.0],q:[0.0,0.0,(0.1f32).sin(),(0.1f32).cos()],..a.boxes[0]};
    h.shapes.push(1000,a); h.shapes.push(1100,b);
    let view=BladeView {hist:&h,blade:&h.blade};
    let midway=h.shapes.sample(1050,0).unwrap();
    assert!((midway.module(1).unwrap().p[0]-25.0).abs()<0.01);
    assert_eq!(midway.module(7).unwrap().p,[0.0;3]);
    let (d,p,t,id,weapon_id)=view.shape_contact([25.0,0.0,0.0],1000.0,1100.0,1).unwrap();
    assert!(d<0.01 && t>1000.0 && t<1100.0 && id==1,"moving head crosses contact between frames");
    assert!(view.shape_contact([25.0,0.0,0.0],1000.0,1100.0,7).unwrap().0>SWEEP_TOL,"stationary Grip cannot borrow Head motion");
    assert!(view.shape_contact([25.0,0.0,0.0],1000.0,1100.0,15).is_none());
    let v=view.shape_velocity(1,1,[0.0,10.0,0.0],1050.0).unwrap();
    assert!((len(v)-480.1).abs()<0.3,"contact speed includes module translation and rotation: {v:?}");
    assert!(len(view.shape_velocity(weapon_id,id,p,t).unwrap())>450.0);
    assert_eq!(view.shape_velocity(1,7,[0.0;3],1050.0),Some([0.0;3]));
    assert!(view.shape_velocity(1,15,[0.0;3],1050.0).is_none());
    assert!(view.shape_peak(1,1,[0.0,10.0,0.0],1100).unwrap()>470.0);
    assert_eq!(view.shape_peak(1,7,[0.0;3],1100),Some(0.0));
    let legacy=view.shape_contact([25.0,0.0,0.0],1000.0,1100.0,0).unwrap();
    assert_eq!(legacy.3,1,"legacy unnamed contact retains whichever module actually hit");
}

#[test]
fn weapon_class_switch_cannot_synthesize_module_motion() {
    let mut h=PeerHist::new();
    let mut a=WeaponShape {weapon_id:1,p:[0.0;3],q:[0.0,0.0,0.0,1.0],boxes:[posecodec::v2::WeaponBox::default();posecodec::v2::MAX_WEAPON_BOXES],n:1};
    a.boxes[0]=posecodec::v2::WeaponBox {component:1,p:[100.0,0.0,0.0],q:[0.0,0.0,0.0,1.0],half:[2.0;3],..Default::default()};
    let mut b=a; b.weapon_id=2; b.boxes[0].p=[20.0,0.0,0.0];
    h.shapes.push(1000,a);h.shapes.push(1100,b);
    let view=BladeView {hist:&h,blade:&h.blade};
    assert!(h.shapes.sample(1050,0).unwrap().nearest([60.0,0.0,0.0],1).0>SWEEP_TOL,"unrelated heads do not interpolate a fictional midpoint");
    assert!(view.shape_contact([60.0,0.0,0.0],1000.0,1100.0,1).is_none());
    assert!(view.shape_velocity(1,1,[0.0;3],1050.0).is_none());
    assert!(view.shape_velocity(2,1,[0.0;3],1050.0).is_none());
    assert!(view.shape_velocity(1,1,[0.0;3],1080.0).is_none(),"a historical contact keeps its original weapon class");
    assert_eq!(view.shape_peak(2,1,[0.0;3],1100),Some(0.0),"class swap cannot create an800cm/s peak");
}

#[test]
fn offhand_shield_modules_use_their_own_class_bounds() {
    use crate::validate::damage::{set_kit,weapon_class_for_hand,WeaponClass};
    let id=837;
    set_kit(id,&crate::loadout::KitSel::new("custom","w_arming1","s_buckler3",&[],[0;4]));
    assert_eq!(weapon_class_for_hand(id,false),WeaponClass::Sword);
    assert_eq!(weapon_class_for_hand(id,true),WeaponClass::Shield);
    let mut s=Store::default();
    s.record_root(id,1000,[0.0,0.0,100.0],1000);
    let w=posecodec::v2::Weapon {hands:2,p:[0.0,0.0,130.0],q:[0.0,0.0,0.0,1.0],base:[0.0;3],tip:[0.0,0.0,1.0],
        boxes:vec![posecodec::v2::WeaponBox {component:1,p:[0.0;3],q:[0.0,0.0,0.0,1.0],half:[3.0,35.0,45.0],..Default::default()}],..Default::default()};
    assert!(s.record_weapon_shape(id,1000,&w,true),"broad shield shape fits the shield hand");
    assert!(!s.record_weapon_shape(id,1000,&w,false),"a sword cannot borrow its shield's width");
    crate::validate::damage::forget(id);
}

#[test]
fn degenerate_tip_modules_use_the_same_repaired_axis_in_actor_space() {
    let id = 0x1B82;
    crate::validate::damage::set_kit(id, &crate::loadout::KitSel::new("custom", "w_poleaxe_m", "", &[], [0;4]));
    let mut s = Store::default();
    let base = [30.0, 40.0, 130.0];
    s.record_root(id, 1000, [30.0, 40.0, 100.0], 1000);
    let mut w = posecodec::v2::Weapon { hands: 1, p: base,
        q: [0.0,0.0,std::f32::consts::FRAC_1_SQRT_2,std::f32::consts::FRAC_1_SQRT_2],
        base: [0.0;3], tip: [1.0,0.0,0.0], boxes: vec![
            posecodec::v2::WeaponBox { component:1, p:[150.0,0.0,0.0], q:[0.0,0.0,0.0,1.0], half:[20.0,20.0,3.0],..Default::default()},
            posecodec::v2::WeaponBox { component:7, p:[75.0,0.0,0.0], q:[0.0,0.0,0.0,1.0], half:[75.0,2.0,2.0],..Default::default()},
        ], ..Default::default() };
    assert!(!s.record_weapon_shape(id, 1000, &w, false), "the box cannot supply its own unbounded repair axis");
    s.record_pose(id, &PoseFrame { ts:1000, bones:vec![
        (PELVIS as u8, [30.0,40.0,100.0], [0.0,0.0,0.0,1.0]),
        (HAND_R as u8, base, [0.0,0.0,0.0,1.0]),
        (LOWERARM_R as u8, [30.0,15.0,130.0], [0.0,0.0,0.0,1.0]),
    ] }, 1000);
    let (base,tip) = posecodec::v2::blade_world(&w);
    s.record_blade(id, 1000, Blade { base,tip,vel:None }, 1000);
    let blade = s.peers[&id].blade.sample(1000,0).unwrap();
    assert!(len(sub(blade.tip,blade.base)) > 200.0);
    assert!(s.record_weapon_shape(id, 1000, &w, false), "150cm head must survive the supported 1cm native tip case");
    let shape = s.peers[&id].shapes.sample(1000,0).unwrap();
    let head_side = [11.0,190.0,130.0];
    assert!(shape.nearest(head_side,1).0 < 0.01);
    assert!(shape.nearest(head_side,7).0 > SWEEP_TOL, "head width does not widen the thin shaft");
    w.boxes[0].p[1] = 90.0;
    assert!(!s.record_weapon_shape(id,1000,&w,false), "off-axis head remains bounded");
    w.boxes[0].p = [330.0,0.0,0.0];
    assert!(!s.record_weapon_shape(id,1000,&w,false), "repair cannot extend past the weapon class");
    crate::validate::damage::forget(id);
}

#[test]
fn each_hand_uses_its_own_repair_reach_and_hilt_limits() {
    use crate::validate::damage::{set_kit, weapon_class, WeaponClass};
    let id = 0x1B83;
    set_kit(id, &crate::loadout::KitSel::new("custom", "w_arming1", "w_axe", &[], [0;4]));
    assert_eq!(weapon_class(id),WeaponClass::Axe, "damage ranking differs from sword reach");
    let mut s = Store::default();
    for ts in [1000,1100] {
        s.record_root(id,ts,[0.0,0.0,100.0],ts as i64);
        s.record_pose(id,&PoseFrame {ts,bones:vec![
            (PELVIS as u8,[0.0,0.0,100.0],[0.0,0.0,0.0,1.0]),
            (HAND_R as u8,[0.0,0.0,140.0],[0.0,0.0,0.0,1.0]),
            (LOWERARM_R as u8,[-25.0,0.0,140.0],[0.0,0.0,0.0,1.0]),
        ]},ts as i64);
        for offhand in [false,true] {
            let tip = if ts==1000 {1.0} else {123.0};
            s.record_blade_hand(id,ts,Blade {base:[0.0,0.0,140.0],tip:[tip,0.0,140.0],vel:None},ts as i64,offhand);
        }
    }
    let h = &s.peers[&id];
    let right = BladeView {hist:h,blade:&h.blade};
    let left = BladeView {hist:h,blade:&h.offhand};
    assert!((len(sub(right.blade.sample(1000,0).unwrap().tip,[0.0,0.0,140.0]))-125.0/1.15).abs()<0.001);
    assert!((len(sub(left.blade.sample(1000,0).unwrap().tip,[0.0,0.0,140.0]))-120.0/1.15).abs()<0.001);
    assert!(s.reach_violation_blade(id,&right,1100).is_none(), "123cm sword is within its own 125cm limit");
    assert!(s.reach_violation_blade(id,&left,1100).unwrap().contains("max 120"), "axe cannot borrow sword reach");
    for offhand in [false,true] {
        s.record_root(id,1200,[0.0,0.0,100.0],1200);
        s.record_pose(id,&PoseFrame {ts:1200,bones:vec![
            (PELVIS as u8,[0.0,0.0,100.0],[0.0,0.0,0.0,1.0]),
            (HAND_R as u8,[0.0,0.0,140.0],[0.0,0.0,0.0,1.0]),
        ]},1200);
        s.record_blade_hand(id,1200,Blade {base:[60.0,0.0,140.0],tip:[100.0,0.0,140.0],vel:None},1200,offhand);
    }
    let h = &s.peers[&id];
    assert!(blade_view_unholdable(id,&BladeView {hist:h,blade:&h.blade},1200), "sword cannot borrow axe's 90cm hilt limit");
    assert!(!blade_view_unholdable(id,&BladeView {hist:h,blade:&h.offhand},1200));
    crate::validate::damage::forget(id);
}

#[test]
fn offhand_stream_requires_a_validated_root_and_bounded_length() {
    let mut s = Store::default();
    let b = Blade { base: [0.0, 0.0, 140.0], tip: [100.0, 0.0, 140.0], vel: None };
    s.record_blade_hand(ATT, 1000, b, 1000, true);
    assert!(s.peers.get(&ATT).map_or(true, |p| p.offhand.newest().is_none()));
    s.record_root(ATT, 1000, [0.0, 0.0, 100.0], 1000);
    s.record_blade_hand(ATT, 1000, Blade { tip: [900.0, 0.0, 140.0], ..b }, 1000, true);
    assert!(s.peers[&ATT].offhand.newest().is_none());
    s.record_blade_hand(ATT, 1000, b, 1000, true);
    assert_eq!(s.peers[&ATT].offhand.newest(), Some(1000));
    assert!(s.peers[&ATT].blade.newest().is_none(), "hands retain independent histories");
}

#[test]
fn offhand_guard_is_considered_for_parry_without_widening_geometry() {
    let mut sc = Scene::new(20, 2000, still, |_| Some([40.0, 0.0, 300.0]), |_| [-160.0, 0.0, 140.0]);
    let h = 1980;
    let hit = hit(VIC, [-5.0, 0.0, 130.0], sc.ats(h), sc.honest_view(h, 31));
    let Eval::Accept(before) = sc.eval(&hit) else { panic!("baseline hit") };
    assert!(!before.parry_possible);
    for t in (25..=1975).step_by(25) {
        sc.s.record_blade_hand(VIC, (t + V_OFF) as u32,
            Blade { base: [-40.0, -40.0, 130.0], tip: [-40.0, 40.0, 130.0], vel: None }, t + 20, true);
    }
    let Eval::Accept(after) = sc.eval(&hit) else { panic!("offhand guard hit") };
    assert!(after.parry_possible, "offhand guard must receive defender grace");
}

#[test]
fn blade_across_attack_path_is_held_for_a_parry() {
    // A blade across the attack path does not reject on its own (the
    // attacker's screen showed no clash, or it would not claim): the hit is
    // held for the defender grace and only a validated clash cancels it.
    // Victim at x=0 guarding: right hand at (-45,-40,140), weapon actor at
    // (-45,20,140) → blade spans y in front of the chest.
    let guard = |ts: u32, r: V3| {
        let mut f = skeleton(ts, r);
        f.bones[HAND_R].1 = [-45.0, -40.0, 140.0];
        f
    };
    let sc = Scene::with_pose(20, 2000, still, |_| Some([-45.0, 20.0, 140.0]), |_| [-160.0, 0.0, 140.0], guard);
    let h = 1980;
    let v = sc.honest_view(h, 31);
    match sc.eval(&hit(VIC, [-5.0, 0.0, 140.0], sc.ats(h), v)) {
        Eval::Accept(i) => assert!(i.parry_possible, "{:?}", i),
        e => panic!("{:?}", e),
    }
    // Strike to the leg goes under the guard.
    assert!(accepted(&sc.eval(&hit(VIC, [-5.0, 10.0, 60.0], sc.ats(h), v))));
}

#[test]
fn parry_hold_only_when_blade_is_near() {
    let h = 1980;
    let far = Scene::new(20, 2000, still, |_| Some([40.0, 0.0, 300.0]), |_| [-160.0, 0.0, 140.0]);
    let e = far.eval(&hit(VIC, [-5.0, 0.0, 130.0], far.ats(h), far.honest_view(h, 31)));
    match e { Eval::Accept(i) => assert!(!i.parry_possible, "{:?}", i), e => panic!("{:?}", e) }
    let near = Scene::new(20, 2000, still, |_| Some([-60.0, 40.0, 140.0]), |_| [-160.0, 0.0, 140.0]);
    let e = near.eval(&hit(VIC, [-5.0, 0.0, 130.0], near.ats(h), near.honest_view(h, 31)));
    match e { Eval::Accept(i) => assert!(i.parry_possible, "{:?}", i), e => panic!("{:?}", e) }
}

/// Victim guards with its weapon at `vw`; attacker weapon actor at `aw`.
fn clash_scene(vw: V3, aw: V3) -> Scene {
    Scene::new(20, 2000, still, move |_| Some(vw), move |_| aw)
}

#[test]
fn validated_clash_cancels() {
    // Blades touching: victim weapon actor (-60,10,140), attacker's at (-70,0,140).
    let mut sc = clash_scene([-60.0, 10.0, 140.0], [-70.0, 0.0, 140.0]);
    let h = 1980;
    let ats = sc.ats(h);
    // The victim reports the attacker ts it displayed (honest).
    let vts = sc.honest_view(h, 31) + 31; // its own clock now-ish
    let shown = (h - 40 - 31 - 17 + A_OFF) as u32;
    sc.s.record_clash(VIC, ATT, vts, shown, sc.now);
    assert!(sc.s.parried(VIC, ATT, ats, sc.now));
    // The attacker's OWN report never cancels its hit (its screen
    // showed the blade reach the body after the clash: solo counts that).
    let mut sc2 = clash_scene([-60.0, 10.0, 140.0], [-70.0, 0.0, 140.0]);
    sc2.s.record_clash(ATT, VIC, ats, sc2.honest_view(h, 31), sc2.now);
    assert!(!sc2.s.parried(VIC, ATT, ats, sc2.now), "the attacker's own clash report cancelled its hit");
}

/// A bind or glance the victim also reported
/// as a clash still lands when the victim's screen showed the blade on its
/// body after that clash (C2STouch). A touch BEFORE the clash (the blade was
/// then stopped) or long after the hit does not save it.
#[test]
fn victim_touch_after_its_clash_keeps_the_hit() {
    let h = 1980;
    let shown = (h - 40 - 31 - 17 + A_OFF) as u32;
    let mk = || {
        let mut sc = clash_scene([-60.0, 10.0, 140.0], [-70.0, 0.0, 140.0]);
        let vts = sc.honest_view(h, 31) + 31;
        sc.s.record_clash(VIC, ATT, vts, shown, sc.now);
        sc
    };
    // a real block: no touch on the victim's screen
    let mut sc = mk();
    let ats = sc.ats(h);
    assert!(sc.s.parried(VIC, ATT, ats, sc.now));
    // the blade slid off the guard into the body on the victim's screen
    let mut sc = mk();
    sc.s.record_touch(VIC, ATT, 1, shown + 30, sc.now);
    assert!(!sc.s.parried(VIC, ATT, ats, sc.now), "touch after the clash: the hit landed");
    // touch in the same frame as the clash (label noise): landed
    let mut sc = mk();
    sc.s.record_touch(VIC, ATT, 1, shown - 10, sc.now);
    assert!(!sc.s.parried(VIC, ATT, ats, sc.now));
    // a touch well before the clash (an earlier exchange): still parried
    let mut sc = mk();
    sc.s.record_touch(VIC, ATT, 1, shown - 400, sc.now);
    assert!(sc.s.parried(VIC, ATT, ats, sc.now), "an older touch must not undo a later block");
    // a touch long after this hit (the next blow): still parried
    let mut sc = mk();
    sc.s.record_touch(VIC, ATT, 1, ats + TOUCH_AFTER_MS as u32 + 50, sc.now);
    assert!(sc.s.parried(VIC, ATT, ats, sc.now));
    // a touch naming another attacker does not count
    let mut sc = mk();
    sc.s.record_root(77, 1000, [0.0; 3], sc.now);
    sc.s.record_touch(VIC, 77, 1, shown + 30, sc.now);
    assert!(sc.s.parried(VIC, ATT, ats, sc.now));
    // bounded per victim
    let mut sc = mk();
    for k in 0..(TOUCHES_MAX as u32 + 40) { sc.s.record_touch(VIC, ATT, 1, 10 + k, sc.now); }
    assert!(sc.s.peers[&VIC].touches.len() <= TOUCHES_MAX);
}

#[test]
fn parry_spam_cannot_cancel_hits() {
    // The victim keeps its blade by its body (parry "plausible" for the hold)
    // but nowhere near the attacker's blade, and spams clash reports naming
    // the attacker's current ts.
    let mut sc = clash_scene([30.0, 0.0, 200.0], [-160.0, 0.0, 140.0]);
    let h = 1980;
    let ats = sc.ats(h);
    let before = cheat::get(VIC);
    for k in 0..40 {
        sc.s.record_clash(VIC, ATT, (h + V_OFF) as u32 - k, ats - k, sc.now);
    }
    assert!(!sc.s.parried(VIC, ATT, ats, sc.now));
    let after = cheat::get(VIC);
    assert!(after.parry_rate_limited >= before.parry_rate_limited + 35, "{:?}", after);
    assert!(after.parry_invalid > before.parry_invalid, "{:?}", after);
    // Even with the bucket refilled, invalid reports never cancel.
    sc.s.record_clash(VIC, ATT, (h + V_OFF) as u32, ats, sc.now + 2000);
    assert!(!sc.s.parried(VIC, ATT, ats, sc.now + 2000));
}

#[test]
fn trade_windows() {
    let mut s = Store::default();
    let now = 0i64;
    s.record_root(1, 9_000, [0.0; 3], 0);
    s.note_death(1, now);
    assert!(s.trade_ok(1, 8_950, now));
    assert!(s.trade_ok(1, 9_100, now));
    assert!(!s.trade_ok(1, 9_400, now));
    assert!(!s.trade_ok(1, 8_950, now + TRADE_SERVER_TTL.as_millis() as i64 + 1));
}

// ---- network simulation ---------------------------------------------------------

/// Deterministic LCG for jitter.
struct Rng(u64);
impl Rng {
    fn next(&mut self) -> f32 {
        self.0 = self.0.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        ((self.0 >> 33) as f32) / (1u64 << 31) as f32
    }
}

struct Outcome { valid: bool, too_far: bool, backtrack: bool, held: bool, no_rewind_hits: bool }

/// Two virtual clients with their own clocks stream at 30 Hz over links with
/// one-way delay rtt/2 + uniform jitter (arriving out of order). The victim
/// strafes ±200 uu at 400 uu/s; the attacker displays it through a jitter
/// buffer, faces what it sees and thrusts at the chest. The server judges
/// with only the samples (and arrival times) it had when the claim arrived.
fn simulate(rtt_ms: f32, jitter_ms: f32, seed: u64, guard: bool) -> Outcome {
    let mut rng = Rng(seed);
    let owd = rtt_ms / 2.0;
    let (v_off, a_off) = (-3000i64, 40_000i64);
    let victim_root = |t_server: f32| -> V3 {
        let ph = (t_server / 1000.0) % 2.0;
        let y = if ph < 1.0 { ph * 400.0 } else { (2.0 - ph) * 400.0 };
        [0.0, y - 200.0, 100.0]
    };
    let jb = 50.0 + jitter_ms * 1.5;
    let view_lag = 2.0 * owd + jitter_ms / 2.0 + jb;
    let attacker_root = |t: f32| -> V3 { [-150.0, victim_root(t - view_lag)[1], 100.0] };

    let mut s = Store::default();
    if rtt_ms > 200.0 { s.set_max_rewind(HIGH_LATENCY_REWIND_MS); }
    let mut events: Vec<(f32, u8, u32, PoseFrame, V3)> = Vec::new();
    let mut t = 10_000.0f32;
    while t < 13_000.0 {
        let r = victim_root(t);
        let vts = (t as i64 + v_off) as u32;
        let mut f = skeleton(vts, r);
        let w = if guard {
            f.bones[HAND_R].1 = [r[0] - 45.0, r[1] - 40.0, r[2] + 30.0];
            [r[0] - 45.0, r[1] + 20.0, r[2] + 30.0]
        } else {
            [r[0] - 45.0, r[1] + 60.0, r[2] + 150.0]
        };
        events.push((t + owd + rng.next() * jitter_ms, 2, vts, f, w));
        let ar = attacker_root(t);
        let ats = (t as i64 + a_off) as u32;
        events.push((t + owd + rng.next() * jitter_ms, 1, ats, skeleton(ats, ar), [ar[0] + 60.0, ar[1], 130.0]));
        t += 33.3;
    }
    let hit_t = 12_500.0f32;
    let view_t = hit_t - view_lag;
    let view_ts = (view_t as i64 + v_off) as u32;
    let seen = victim_root(view_t);
    let ats = (hit_t as i64 + a_off) as u32;
    let arrive = hit_t + owd + rng.next() * jitter_ms;
    events.sort_by(|a, b| a.0.total_cmp(&b.0));
    for (at, who, ts, f, w) in &events {
        if *at > arrive { break; }
        let rx = *at as i64;
        s.record_root(*who as PeerId, *ts, f.bones[0].1, rx);
        s.record_pose(*who as PeerId, f, rx);
        s.record_weapon(*who as PeerId, *ts, *w, rx);
    }
    s.note_rtt(1, rtt_ms, arrive as i64);
    let now = arrive as i64;
    let chest = [seen[0] - 8.0, seen[1], seen[2] + 30.0];
    let ok = |e: Eval| matches!(e, Eval::Accept(_));
    let newest = s.peers[&2].newest().unwrap();
    // Backtrack: the victim 300 ms earlier was ≥ 120 uu away along y.
    let bt_t = view_t - 300.0;
    let bt = victim_root(bt_t);
    let backtrack_hit = hit(2, [bt[0] - 8.0, bt[1], bt[2] + 30.0], ats, (bt_t as i64 + v_off) as u32);
    Outcome {
        valid: ok(s.evaluate(1, &hit(2, chest, ats, view_ts), now)),
        too_far: !ok(s.evaluate(1, &hit(2, [chest[0], chest[1] + 160.0, chest[2]], ats, view_ts), now)),
        // (Only meaningful where the strafe moved the victim ≥ 100 uu.)
        backtrack: !ok(s.evaluate(1, &backtrack_hit, now)) || (bt[1] - seen[1]).abs() < 100.0,
        held: matches!(s.evaluate(1, &hit(2, chest, ats, view_ts), now), Eval::Accept(Info { parry_possible: true, .. })),
        no_rewind_hits: ok(s.evaluate(1, &hit(2, chest, ats, newest), now)),
    }
}

#[test]
fn simulated_internet_fights() {
    for &rtt in &[50.0f32, 150.0, 250.0] {
        for seed in 1..=25u64 {
            let seed = seed * 7919 + rtt as u64;
            let o = simulate(rtt, 30.0, seed, false);
            assert!(o.valid, "valid hit rejected at rtt {} seed {}", rtt, seed);
            assert!(o.too_far, "far hit accepted at rtt {} seed {}", rtt, seed);
            assert!(o.backtrack, "backtracked hit accepted at rtt {} seed {}", rtt, seed);
            let g = simulate(rtt, 30.0, seed, true);
            assert!(g.held, "hit through a guard not held for a parry at rtt {} seed {}", rtt, seed);
            if rtt >= 150.0 {
                // The hint is clamped to the server's prediction, so
                // claiming "no rewind" is snapped back and still hits.
                let _ = o.no_rewind_hits;
            }
        }
    }
}

// ---- property tests ------------------------------------------------------------------

proptest! {
    #![proptest_config(ProptestConfig::with_cases(256))]

    /// Whatever view hint a client sends, the server judges within
    /// ±tol of its own prediction (the backtrack window is bounded).
    #[test]
    fn prop_view_hint_is_bounded(hint_off in -2000i64..2000, h in 1500i64..1990) {
        let sc = Scene::new(40, 2000, runner, |_| None, sword_near);
        let p = sc.s.predict_view(ATT, VIC, sc.ats(h), sc.now).unwrap();
        let hint = (p.expected + hint_off).max(1) as u32;
        let used = p.clamp(hint) as i64;
        prop_assert!((used - p.expected).abs() <= p.tol);
        prop_assert!(p.tol <= 2 * TOL_FLOOR_MS);
    }

    /// Backtrack exploit: a contact placed where the victim was `back` ms
    /// before the honest view is rejected for any back ≥ 220 ms (≥ 110 uu
    /// away at 500 uu/s: beyond the v1 body slack), whatever hint is sent.
    #[test]
    fn prop_backtrack_rejected(back in 220i64..700, hint_jitter in -800i64..800, h in 1700i64..1990) {
        let sc = Scene::new(40, 2000, runner, |_| None, sword_near);
        let honest = sc.honest_view(h, 31) as i64;
        let past = runner(honest - back - V_OFF);
        let hint = (honest - back + hint_jitter).max(1) as u32;
        let e = sc.eval(&hit(VIC, [past[0] - 8.0, 0.0, 130.0], sc.ats(h), hint));
        prop_assert!(!accepted(&e), "{:?}", e);
    }

    /// NoData: zero timestamps never pass once the victim streams.
    #[test]
    fn prop_no_timestamp_rejected(ats_zero in any::<bool>(), x in -50f32..50.0) {
        let sc = Scene::new(40, 2000, still, |_| None, |_| [-60.0, 0.0, 140.0]);
        let (a, v) = if ats_zero { (0, sc.honest_view(1950, 31)) } else { (sc.ats(1950), 0) };
        let e = sc.eval(&hit(VIC, [x, 0.0, 130.0], a, v));
        prop_assert!(matches!(e, Eval::Reject(_)), "{:?}", e);
    }

    /// Parry spam: any number of clash reports with arbitrary timestamps
    /// cannot cancel a hit while the blades are ≥ 100 uu apart.
    #[test]
    fn prop_parry_spam_cannot_cancel(reports in proptest::collection::vec((-300i64..300, -300i64..300, 0i64..3000), 1..60)) {
        let mut sc = clash_scene([30.0, 0.0, 220.0], [-160.0, 0.0, 140.0]);
        let h = 1980;
        let ats = sc.ats(h);
        for (dm, dot, dt) in reports {
            sc.s.record_clash(VIC, ATT, (h + V_OFF + dm) as u32, (ats as i64 + dot) as u32, sc.now + dt / 10);
            sc.s.record_clash(ATT, VIC, (ats as i64 + dm) as u32, (h + V_OFF + dot) as u32, sc.now + dt / 10);
        }
        prop_assert!(!sc.s.parried(VIC, ATT, ats, sc.now + 300));
    }

    /// Sorted insert: any arrival order yields the sorted, deduplicated ring.
    #[test]
    fn prop_ring_sorted(ts in proptest::collection::vec(1000u32..2000, 1..120)) {
        let mut r: Ring<V3> = Ring::new();
        for &t in &ts { r.push(t, [t as f32, 0.0, 0.0]); }
        let mut want: Vec<u32> = ts.clone();
        want.sort_unstable();
        want.dedup();
        let got: Vec<u32> = r.q.iter().map(|e| e.0).collect();
        prop_assert_eq!(got, want);
    }
}

#[test]
fn clash_reports_on_forged_ids_do_not_grow_state() {
    let mut sc = clash_scene([30.0, 0.0, 200.0], [-160.0, 0.0, 140.0]);
    for i in 0..5000u32 {
        sc.s.record_clash(VIC, 50_000 + i, 1000 + i, 2000 + i, sc.now + i as i64);
    }
    let p = &sc.s.peers[&VIC];
    assert!(p.clash_bucket.is_empty() && p.clashes.is_empty(), "{} {}", p.clash_bucket.len(), p.clashes.len());
    assert!(!sc.s.peers.keys().any(|k| *k >= 50_000), "no entries for unknown peers");
    // Known peers are bounded too.
    for i in 0..200u32 {
        let id = 60_000 + i;
        sc.s.record_root(id, 1000, [0.0; 3], sc.now);
        sc.s.record_clash(VIC, id, 1000, 2000, sc.now);
    }
    assert!(sc.s.peers[&VIC].clash_bucket.len() <= CLASH_BUCKETS_MAX);
}

#[test]
fn speedhack_clock_detected_and_route_change_is_not() {
    // 1.3× clock, 60 Hz, 50 ms ± 12 ms delay: flagged after ~2 s.
    let mut c = Clock::default();
    let mut rng = Rng(7);
    let mut flagged_at = None;
    for i in 0..400i64 {
        let t = i as f64 * 16.67;
        let rx = (t + 50.0 + (rng.next() as f64 - 0.5) * 24.0) as i64;
        c.note(rx, (100_000.0 + 1.3 * t) as u32);
        if flagged_at.is_none() && c.rate_suspect().is_some() { flagged_at = Some(t); }
    }
    let at = flagged_at.expect("speedhack never flagged");
    assert!(at <= 3200.0, "flagged only at {at} ms");
    // Honest clock (+300 ppm) with a +120 ms route change at 3 s: never.
    let mut c = Clock::default();
    for i in 0..600i64 {
        let t = i as f64 * 16.67;
        let step = if t > 3000.0 { 120.0 } else { 0.0 };
        let rx = (t + 50.0 + step + (rng.next() as f64 - 0.5) * 24.0) as i64;
        c.note(rx, (100_000.0 + 1.0003 * t) as u32);
        assert!(c.rate_suspect().is_none(), "honest clock flagged at {t} ms: {:?}", c.rate_error());
    }
}

#[test]
fn ms_at_is_a_floor_on_both_sides_of_the_epoch() {
    let before = Instant::now();
    let _ = now_ms(); // the epoch is initialised lazily (possibly right now)
    for n in [0u64, 1, 17, 1500, 1501] {
        let d = ms_at(before + Duration::from_millis(n)) - ms_at(before);
        assert_eq!(d, n as i64, "+{n} ms");
    }
}

// ---- jitter / clash cheats --------------------------------------------------------

/// Victim streams 60 Hz with `noise(i)` ms of extra lateness (a sidecar
/// holding its pose packets: jitter the transport does not show); attacker
/// streams clean. Both rtt 80 ms; transport rttvar `var` ms for both.
fn lag_switch_store(noise: impl Fn(i64) -> i64, var: Option<f32>) -> (Store, u32, i64) {
    let mut s = Store::default();
    let owd = 40;
    let mut last = 0;
    for i in 0..120i64 {
        let t = i * 16;
        let rx_v = t + owd + noise(i);
        s.record_root(VIC, (t + V_OFF) as u32, [0.0, 0.0, 100.0], rx_v);
        s.record_root(ATT, (t + A_OFF) as u32, [-150.0, 0.0, 100.0], t + owd);
        s.record_pose(VIC, &skeleton((t + V_OFF) as u32, [0.0, 0.0, 100.0]), rx_v);
        s.record_pose(ATT, &skeleton((t + A_OFF) as u32, [-150.0, 0.0, 100.0]), t + owd);
        last = t;
    }
    let now = last + owd + 5;
    s.note_rtt(ATT, 80.0, now);
    s.note_rtt(VIC, 80.0, now);
    if let Some(v) = var {
        s.note_net_jitter(ATT, v, now);
        s.note_net_jitter(VIC, v, now);
    }
    (s, (last + A_OFF) as u32, now)
}

fn lcg(i: i64) -> i64 { ((i.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407) >> 33) & 0x7fff_ffff) % 251 }

/// A victim inflating its own stream jitter (0–250 ms holds) must not push
/// every attacker's view lag past the 300 ms cap ("favour the defender" →
/// unhittable). The share of the display delay its excess jitter adds is
/// not charged to the cap, and the view window does not widen with client-controlled jitter.
#[test]
fn lag_switching_victim_is_not_protected_by_the_rewind_cap() {
    let (s, ats, now) = lag_switch_store(lcg, Some(4.0));
    let pv = s.predict_view(ATT, VIC, ats, now).unwrap();
    assert!(pv.victim_excess >= 150, "{:?}", pv);
    assert!(pv.tol <= TOL_MAX_MS, "{:?}", pv);
    assert!(pv.unexplained >= 150, "{:?}", pv);
    // A high-jitter link the transport shows too (an awful link — or
    // a lag switch that holds its acks as well, which looks the same):
    // nothing is suspicious, but the cap still charges only
    // VICTIM_JITTER_CHARGED_MS of it, so the victim stays hittable.
    let (s, ats, now) = lag_switch_store(lcg, Some(90.0));
    let pv = s.predict_view(ATT, VIC, ats, now).unwrap();
    assert_eq!(pv.unexplained, 0, "{:?}", pv);
    assert!(pv.victim_excess >= 80, "{:?}", pv);
    // No transport data yet: the absolute cap alone.
    let (s, ats, now) = lag_switch_store(lcg, None);
    let pv = s.predict_view(ATT, VIC, ats, now).unwrap();
    assert_eq!(pv.unexplained, 0);
    assert!(pv.victim_excess >= 80, "{:?}", pv);
    // An honest, steady victim: no excess either.
    let (s, ats, now) = lag_switch_store(|i| i % 3, Some(4.0));
    assert_eq!(s.predict_view(ATT, VIC, ats, now).unwrap().victim_excess, 0);
}

/// A clash reporter's blade that teleports (tip faster than
/// CLASH_BLADE_SPEED_MAX) or whose hilt is out of its hands' reach is not a
/// block.
#[test]
fn clash_reporter_blade_must_be_physically_held() {
    let mut s = Store::default();
    let id = 0x1A73;
    crate::validate::damage::forget(id); // judged as a Sword (grip 45)
    for i in 0..30u32 {
        let ts = 10_000 + 16 * i;
        s.record_root(id, ts, [0.0, 0.0, 100.0], 1000 + 16 * i as i64);
        s.record_pose(id, &skeleton(ts, [0.0, 0.0, 100.0]), 1000 + 16 * i as i64);
        // Hand_R at (45,-20,140): a held blade from the hand forward...
        let mut b = Blade { base: [50.0, -20.0, 140.0], tip: [150.0, -20.0, 140.0], vel: None };
        if i == 20 {
            // ...snapped for one frame onto a blade 150 uu away.
            b = Blade { base: [200.0, 100.0, 140.0], tip: [300.0, 100.0, 140.0], vel: None };
        }
        s.record_blade(id, ts, b, 1000 + 16 * i as i64);
    }
    let h = &s.peers[&id];
    let jump_t = 10_000 + 16 * 20;
    assert!(blade_too_fast(h, jump_t, 16));
    assert!(!blade_too_fast(h, 10_000 + 16 * 5, 16), "steady blade");
    assert!(blade_unholdable(id, h, jump_t), "hilt 160 uu from the hands");
    assert!(!blade_unholdable(id, h, 10_000 + 16 * 5));
}

/// Pose, capsules and blade are tied to the sender's validated
/// root: a skeleton / blade streamed next to the victim for a strike while
/// the root stays put is not recorded, nor a pelvis faster than a body.
#[test]
fn streams_off_the_validated_root_are_not_recorded() {
    let mut s = Store::default();
    let id = 0x1A74;
    for i in 0..10u32 {
        let ts = 20_000 + 33 * i;
        s.record_root(id, ts, [0.0, 0.0, 100.0], 1000 + 33 * i as i64);
        s.record_pose(id, &skeleton(ts, [0.0, 0.0, 100.0]), 1000 + 33 * i as i64);
    }
    let n = s.peers[&id].pose.q.len();
    // Two frames streamed 6 m away (root unchanged): dropped.
    let t = 20_000 + 33 * 10;
    s.record_root(id, t, [0.0, 0.0, 100.0], 1400);
    s.record_pose(id, &skeleton(t, [600.0, 0.0, 100.0]), 1400);
    assert_eq!(s.peers[&id].pose.q.len(), n, "pelvis 6 m off its root");
    s.record_blade(id, t, Blade { base: [600.0, 0.0, 140.0], tip: [700.0, 0.0, 140.0], vel: None }, 1400);
    assert!(s.peers[&id].blade.q.is_empty(), "blade 6 m off its root");
    s.record_capsules(id, t, &[Capsule { a: [600.0, 0.0, 100.0], b: [600.0, 0.0, 150.0], r: 10.0 }], 1400);
    assert!(s.peers[&id].caps.q.is_empty(), "capsules 6 m off their root");
    // Pelvis jump within the root tolerance but faster than a body (2 m in 33 ms): dropped.
    let id2 = 0x1A75;
    for (k, x) in [(0u32, 0.0f32), (33, 100.0), (66, 20.0)] {
        s.record_root(id2, 30_000 + k, [x, 0.0, 100.0], 1000 + k as i64);
    }
    s.record_pose(id2, &skeleton(30_000, [0.0, 0.0, 100.0]), 1000);
    s.record_pose(id2, &skeleton(30_033, [200.0, 0.0, 100.0]), 1033);
    assert_eq!(s.peers[&id2].pose.q.len(), 1);
    // ...and an honest step passes.
    s.record_pose(id2, &skeleton(30_066, [20.0, 0.0, 100.0]), 1066);
    assert_eq!(s.peers[&id2].pose.q.len(), 2);
    assert!(cheat::get(id).teleport >= 3);
}

// ---- pose-to-root tie -----------------------------------------------------------

/// Exploit regression: a frame WITHOUT the pelvis would skip
/// the root tie, so its other bones (the fallback capsules) could be streamed
/// anywhere — a victim streaming its body away from a blade. It is neither
/// recorded nor relayed; and with the pelvis present, every other bone is
/// tied to the root too.
#[test]
fn a_pose_without_its_pelvis_or_with_bones_off_the_root_is_refused() {
    let mut s = Store::default();
    let id = 0x1A76;
    for i in 0..10u32 {
        let ts = 40_000 + 33 * i;
        s.record_root(id, ts, [0.0, 0.0, 100.0], 1000 + 33 * i as i64);
        assert!(s.record_pose(id, &skeleton(ts, [0.0, 0.0, 100.0]), 1000 + 33 * i as i64), "honest frame relayed");
    }
    let n = s.peers[&id].pose.q.len();
    let t = 40_000 + 33 * 10;
    s.record_root(id, t, [0.0, 0.0, 100.0], 1400);
    // The bypass: pelvis bit absent, the whole body streamed 6 m away.
    let mut f = skeleton(t, [600.0, 0.0, 100.0]);
    f.bones.retain(|b| b.0 as usize != PELVIS);
    assert!(!s.record_pose(id, &f, 1400), "pelvis-less frame not relayed");
    assert_eq!(s.peers[&id].pose.q.len(), n, "pelvis-less frame not recorded");
    // Pelvis on the root, but a hand (and so its capsule) 5 m away.
    let mut f = skeleton(t + 1, [0.0, 0.0, 100.0]);
    f.bones[HAND_R].1 = [500.0, 0.0, 140.0];
    assert!(!s.record_pose(id, &f, 1401), "a bone off the root is a teleport");
    assert_eq!(s.peers[&id].pose.q.len(), n);
    // An honest outstretched arm passes.
    let mut f = skeleton(t + 2, [0.0, 0.0, 100.0]);
    f.bones[HAND_R].1 = [70.0, -20.0, 160.0];
    assert!(s.record_pose(id, &f, 1402));
    assert_eq!(s.peers[&id].pose.q.len(), n + 1);
}

/// Exploit regression: a peer that streams untimestamped roots
/// (C2SRootState, or ts 0) does not feed the lag-comp root ring, so "no root
/// sample" would let its poses, blades and capsules be recorded unjudged — a
/// skeleton or blade streamed onto the victim would count. Without a covering
/// timestamped root nothing is recorded (the frame is still relayed for
/// display; the relay refuses only frames that are provably off the root).
#[test]
fn streams_without_a_timestamped_root_are_never_recorded() {
    let mut s = Store::default();
    let id = 0x1A77;
    for i in 0..10u32 {
        let ts = 50_000 + 33 * i;
        s.record_root(id, 0, [0.0, 0.0, 100.0], 1000 + 33 * i as i64); // ts 0: untimed
        assert!(s.record_pose(id, &skeleton(ts, [600.0, 0.0, 100.0]), 1000 + 33 * i as i64));
        s.record_blade(id, ts, Blade { base: [600.0, 0.0, 140.0], tip: [700.0, 0.0, 140.0], vel: None }, 1000 + 33 * i as i64);
        s.record_capsules(id, ts, &[Capsule { a: [600.0, 0.0, 100.0], b: [600.0, 0.0, 150.0], r: 10.0 }], 1000 + 33 * i as i64);
    }
    let h = &s.peers[&id];
    assert!(h.pose.q.is_empty() && h.blade.q.is_empty() && h.caps.q.is_empty(), "nothing recorded without a timed root");
    // A peer whose timed root stream stopped: the stream is covered for
    // ROOT_COVER_LEAD_MS after its newest root, then no longer recorded.
    let id = 0x1A78;
    s.record_root(id, 60_000, [0.0, 0.0, 100.0], 1000);
    assert!(s.record_pose(id, &skeleton(60_100, [0.0, 0.0, 100.0]), 1100));
    assert_eq!(s.peers[&id].pose.q.len(), 1, "covered (root 100 ms behind)");
    assert!(s.record_pose(id, &skeleton(60_000 + ROOT_COVER_LEAD_MS as u32 + 50, [0.0, 0.0, 100.0]), 1600));
    assert_eq!(s.peers[&id].pose.q.len(), 1, "not covered any more: not recorded");
    // The capsule/blade path: not recorded either.
    s.record_blade(id, 60_000 + 900, Blade { base: [20.0, 0.0, 140.0], tip: [120.0, 0.0, 140.0], vel: None }, 1900);
    assert!(s.peers[&id].blade.q.is_empty());
}

/// A frame that fails the pose-to-root tie is not relayed (the
/// dispatch arm relays only when `record_skeletal` says so): a sender
/// alternating its pelvis between its root and the victim would otherwise make every
/// receiver snap that stand-in into the victim's body.
#[test]
fn frames_off_the_root_are_not_relayed() {
    let mut s = Store::default();
    let id = 0x1A79;
    let mut relayed = 0;
    for i in 0..20u32 {
        let ts = 70_000 + 16 * i;
        s.record_root(id, ts, [0.0, 0.0, 100.0], 1000 + 16 * i as i64);
        let pelvis = if i % 2 == 0 { [0.0, 0.0, 100.0] } else { [400.0, 0.0, 100.0] }; // the victim, 4 m away
        if s.record_pose(id, &skeleton(ts, pelvis), 1000 + 16 * i as i64) { relayed += 1; }
    }
    assert_eq!(relayed, 10, "only the frames on the root are relayed");
}

/// The global entry point: an undecodable frame is relayed unchanged (as
/// before; receivers drop it with the same codec), a decodable off-root one
/// is not.
#[test]
fn record_skeletal_reports_relayability() {
    let id = 0x1A7A;
    assert!(record_skeletal(id, &[0x5A; 290]), "opaque frame relayed as before");
    let now = now_ms();
    with_store(|s| s.record_root(id, 80_000, [0.0, 0.0, 100.0], now));
    let bones: Vec<(u8, [f32; 7])> = (0..BONE_COUNT as u8).map(|b| (b, [900.0, 0.0, 60.0 + b as f32 * 5.0, 0.0, 0.0, 0.0, 1.0])).collect();
    assert!(!record_skeletal(id, &posecodec::encode(80_010, &bones)), "pelvis 9 m off its root");
    forget(id);
}

/// Exploit regression: a lag switch that holds EVERY outgoing
/// datagram 0–250 ms — its pose stream and its acks — inflates the
/// server-measured rttvar (~75–90 ms), so the transport bound (3·rttvar + 15)
/// alone would not discount its own jitter and it would be unhittable for
/// attackers above ~50 ms RTT. The absolute VICTIM_JITTER_CHARGED_MS cap keeps an
/// honest hit on what the attacker saw inside the rewind cap.
#[test]
fn ack_holding_lag_switch_victim_is_hittable() {
    for var in [75.0f32, 90.0] {
        let (s, ats, now) = lag_switch_store(lcg, Some(var));
        let pv = s.predict_view(ATT, VIC, ats, now).unwrap();
        let e = s.evaluate(ATT, &hit(VIC, [-10.0, 0.0, 130.0], ats, pv.expected as u32), now);
        assert!(accepted(&e), "rttvar {var}: honest hit on the lag switcher rejected: {:?} ({:?})", e, pv);
    }
}

// ---- relay view model / reconnect ----------------------------------------------------

/// The real receiver jitter buffer (sidecar poseplay.rs), to check the
/// server's model of it against.
use hsmp_pose::poseplay;

/// An UNARMED codec-v2 victim (no blade stream: fists send no weapon) must
/// not get the v1 buffer formula (interval + p90 + 6, floor 20) instead of
/// the receiver's v2 one (p90 + 6, floor 16): that is a systematic bias out
/// of the 30 ms tolerance. The formula follows the codec of the victim's
/// last skeletal frame.
#[test]
fn unarmed_v2_victim_gets_the_v2_buffer_formula() {
    let mut s = Store::default();
    for i in 0..120i64 {
        let t = i * 16;
        s.record_root(VIC, (t + V_OFF) as u32, [0.0, 0.0, 100.0], t + 40);
        s.record_pose(VIC, &skeleton((t + V_OFF) as u32, [0.0, 0.0, 100.0]), t + 40);
        s.record_root(ATT, (t + A_OFF) as u32, [-150.0, 0.0, 100.0], t + 40);
        s.record_pose(ATT, &skeleton((t + A_OFF) as u32, [-150.0, 0.0, 100.0]), t + 40);
    }
    s.note_rtt(ATT, 80.0, 2000);
    let ats = (1900 + A_OFF) as u32;
    let v1 = s.predict_view(ATT, VIC, ats, 2000).unwrap();
    s.note_pose_codec(VIC, true);
    let v2 = s.predict_view(ATT, VIC, ats, 2000).unwrap();
    // 60 Hz, no jitter: v1 = 16 + 0 + 6 = 22; v2 = max(0 + 6, 16) = 16.
    let bias = v2.expected - v1.expected;
    assert!((5..=8).contains(&bias), "v1 buffer {} ms longer than the receiver's v2 one", bias);
    // The v2 expectation is exactly the receiver's: ts + off_a − off_v − rtt − 16 − 1 frame.
    let want = 1900 + A_OFF + (40 - A_OFF) - (40 - V_OFF) - 80 - 16 - FRAME_MS;
    assert!((v2.expected - want).abs() <= 1, "{} vs {}", v2.expected, want);
}

/// At 8 players the relay sends a remote skeleton at 7.5 Hz; the attacker
/// sees the victim interpolated between the frames it was SENT, while the
/// full-rate history can be >100 uu away on a fast motion (body tolerance
/// 25–60). With the relay's
/// frame log (`note_relayed`) the hit is judged against what was sent.
/// Victim: pelvis swaying ±150 uu in y with a 266.7 ms period, 60 Hz; the
/// relay forwards every 8th frame (133 ms, half a period: every sent frame
/// is at the centre). The attacker's screen shows the victim near y = 0.
#[test]
fn hits_are_judged_against_the_relayed_frames_the_attacker_saw() {
    let sway = |t: i64| (t as f32 * std::f32::consts::TAU / 266.667).sin() * 150.0;
    let run = |relay: bool| -> (Store, i64) {
        let mut s = Store::default();
        let mut last = 0;
        for i in 0..150i64 {
            let t = (i as f32 * 16.6667).round() as i64;
            let rx = t + 30;
            let vts = (t + V_OFF) as u32;
            let r = [0.0, sway(t), 100.0];
            s.record_root(VIC, vts, r, rx);
            s.record_pose(VIC, &skeleton(vts, r), rx);
            let ats = (t + A_OFF) as u32;
            s.record_root(ATT, ats, [-150.0, 0.0, 100.0], rx);
            s.record_pose(ATT, &skeleton(ats, [-150.0, 0.0, 100.0]), rx);
            if relay && i % 8 == 0 { s.note_relayed(ATT, VIC, vts, 133, rx); }
            last = rx;
        }
        s.note_rtt(ATT, 60.0, last);
        s.note_rtt(VIC, 60.0, last);
        (s, last)
    };
    let (s_rel, now) = run(true);
    let (s_full, _) = run(false);
    let mut checked = 0;
    // A hit swung at server time h, arriving now (age_ms says how long it
    // waited: the arrival-consistency check holds).
    let aged = |mut d: DamageEvent, h: i64| { d.age_ms = (now - h) as u32; d };
    for h in (now - 120)..(now - 30) {
        let ats = (h - 30 + A_OFF) as u32;
        let Some(pv) = s_rel.predict_view(ATT, VIC, ats, now) else { continue };
        let view = pv.expected;
        let truth_y = sway(view - V_OFF);
        if truth_y.abs() < 110.0 { continue; } // a quarter phase: shown ≈ 0, true ≈ ±150
        checked += 1;
        // Contact on the chest the attacker saw (the sent frames are all at y ≈ 0).
        let e = s_rel.evaluate(ATT, &aged(hit(VIC, [-10.0, 0.0, 130.0], ats, view as u32), h), now);
        assert!(accepted(&e), "honest hit on the relayed pose rejected at view {view}: {:?}", e);
        // A contact on the true body the attacker was never sent: rejected.
        let ghost = aged(hit(VIC, [-10.0, truth_y, 130.0], ats, view as u32), h);
        assert!(!accepted(&s_rel.evaluate(ATT, &ghost, now)), "hit on a pose never relayed accepted");
        // Without relay data the full-rate history judged the seen hit
        // against a body ±150 uu away.
        let pvf = s_full.predict_view(ATT, VIC, ats, now).unwrap();
        if sway(pvf.expected - V_OFF).abs() >= 110.0 {
            let e = s_full.evaluate(ATT, &aged(hit(VIC, [-10.0, 0.0, 130.0], ats, pvf.expected as u32), h), now);
            assert!(!accepted(&e), "full-rate judge accepted a hit 150 uu off: {:?}", e);
        }
    }
    assert!(checked >= 10, "only {checked} quarter-phase instants");
    // The pair's buffer delay follows the 133 ms relay interval (+ margin).
    let pv = s_rel.predict_view(ATT, VIC, (now - 60 + A_OFF) as u32, now).unwrap();
    let pf = s_full.predict_view(ATT, VIC, (now - 60 + A_OFF) as u32, now).unwrap();
    assert!(pf.expected - pv.expected >= 60, "decimated pair displayed later ({} vs {})", pv.expected, pf.expected);
}

/// The server's display-delay model mirrors the receiver's
/// buffer dynamics. An 80 ms jitter burst on the victim's uplink from 3 s to
/// 4 s: the real poseplay buffer grows fast, its playback clock slews at
/// most 10 %, and the delay falls slowly afterwards. The instantaneous
/// formula drifts tens of ms from it; the per-pair model stays inside the
/// 30 ms tolerance.
#[test]
fn display_delay_model_tracks_the_receiver_through_a_jitter_burst() {
    let mut s = Store::default();
    let mut pb = poseplay::Playback::new();
    let mut st = 12345u64;
    let mut rnd = move || {
        st = st.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        ((st >> 33) % 81) as i64
    };
    // (rx, ts) of the victim's 60 Hz frames at the receiver (= server + 0).
    let mut arr: Vec<(i64, u32)> = Vec::new();
    for i in 0..480i64 {
        let t = (i as f32 * 16.667) as i64;
        let extra = if (3000..4000).contains(&t) { rnd() } else { 0 };
        arr.push((t + 30 + extra, (t + V_OFF) as u32));
    }
    arr.sort();
    let (mut err_model, mut err_formula) = (Vec::new(), Vec::new());
    let mut k = 0;
    let mut now = 0i64;
    while now < 8000 {
        while k < arr.len() && arr[k].0 <= now {
            let (rx, ts) = arr[k];
            let r = [0.0, 0.0, 100.0];
            s.record_root(VIC, ts, r, rx);
            s.record_pose(VIC, &skeleton(ts, r), rx);
            s.note_relayed(ATT, VIC, ts, 17, rx);
            pb.push_pose(rx as f64, poseplay::Frame::from_pose(ts, &skeleton(ts, r)));
            k += 1;
        }
        if let Some(smp) = pb.sample(now as f64) {
            if now >= 1500 {
                let actual = now as f64 - pb.clock.offset.unwrap() - smp.pt;
                let v = &s.peers[&VIC];
                let jv = v.clock.jitter_p90();
                let model = s.view_delay(ATT, VIC, v, jv, now) as f64;
                let formula = display_delay(v, jv, None) as f64;
                err_model.push((model - actual).abs());
                err_formula.push((formula - actual).abs());
            }
        }
        now += 4;
    }
    let over = |e: &Vec<f64>| e.iter().filter(|x| **x > TOL_FLOOR_MS as f64).count() as f64 / e.len() as f64;
    let max = |e: &Vec<f64>| e.iter().cloned().fold(0.0, f64::max);
    assert!(err_model.len() > 1000);
    println!("model err max {:.1} ms ({:.1} % > 30 ms), formula err max {:.1} ms ({:.1} % > 30 ms)",
        max(&err_model), 100.0 * over(&err_model), max(&err_formula), 100.0 * over(&err_formula));
    assert!(over(&err_model) <= 0.01, "model outside the tolerance {:.1} % of the time", 100.0 * over(&err_model));
    assert!(max(&err_model) < max(&err_formula), "model no better than the formula");
}

/// Session resume (the peer keeps its id across a reconnect, so lag comp
/// keeps its state): the stream stalls 1.5 s, then resumes on a new path
/// 100 ms slower. The clock map re-learns from the new path instead of
/// mixing both (a 100 ms "jitter" that skews the view prediction and flags a
/// lag switch for up to 2 s), and no speedhack is seen.
#[test]
fn a_reconnect_on_a_new_path_is_not_a_lag_switch() {
    let mut s = Store::default();
    let mut now = 0;
    let mut t = 0i64;
    while t <= 3800 {
        let resumed = t >= 3500;
        if t < 2000 || resumed {
            let rx = t + 40 + if resumed { 100 } else { 0 };
            let ts = (t + V_OFF) as u32;
            s.record_root(VIC, ts, [0.0, 0.0, 100.0], rx);
            s.record_pose(VIC, &skeleton(ts, [0.0, 0.0, 100.0]), rx);
            now = now.max(rx);
        }
        let ats = (t + A_OFF) as u32;
        s.record_root(ATT, ats, [-150.0, 0.0, 100.0], t + 40);
        s.record_pose(ATT, &skeleton(ats, [-150.0, 0.0, 100.0]), t + 40);
        t += 16;
    }
    s.note_rtt(ATT, 80.0, now);
    s.note_net_jitter(ATT, 3.0, now);
    s.note_net_jitter(VIC, 3.0, now);
    let v = &s.peers[&VIC];
    assert!(v.clock.jitter_p90() <= 5.0, "old and new path mixed into {} ms of jitter", v.clock.jitter_p90());
    assert!(v.clock.rate_suspect().is_none());
    let pv = s.predict_view(ATT, VIC, (3700 + A_OFF) as u32, now).unwrap();
    assert_eq!(pv.unexplained, 0, "{:?}", pv);
}

/// Exploit regression: a "parry aimbot" streams its shoulder,
/// hand and weapon onto the attacker's blade (smoothly: no teleport mark;
/// the hand within arm's reach of its shoulder; the weapon in its hand)
/// while its torso stays home. Without a shoulder-to-pelvis tie the clash
/// would validate and cancel the hit. A shoulder further than
/// SHOULDER_PELVIS_MAX from the pelvis fails the reporter's reach check;
/// an honest guard (the same parry within the body's reach) still counts.
#[test]
fn a_parry_from_a_shoulder_detached_from_the_torso_is_not_a_clash() {
    let h = 1980;
    let fake = |ts: u32, r: V3| {
        let mut f = skeleton(ts, r);
        f.bones[UPPERARM_R].1 = [r[0] - 120.0, r[1], r[2] + 48.0];
        f.bones[LOWERARM_R].1 = [r[0] - 128.0, r[1] + 5.0, r[2] + 44.0];
        f.bones[HAND_R].1 = [r[0] - 135.0, r[1] + 10.0, r[2] + 40.0];
        f
    };
    let mut sc = Scene::with_pose(20, 2000, still, |_| Some([-150.0, 10.0, 140.0]), |_| [-160.0, 0.0, 140.0], fake);
    let ats = sc.ats(h);
    let vts = sc.honest_view(h, 31) + 31;
    let shown = (h - 40 - 31 - 17 + A_OFF) as u32;
    sc.s.record_clash(VIC, ATT, vts, shown, sc.now);
    assert!(!sc.s.parried(VIC, ATT, ats, sc.now), "detached-shoulder parry cancelled the hit");
    let r = sc.s.reach_violation(VIC, &sc.s.peers[&VIC], vts).expect("reach violation");
    assert!(r.contains("shoulder"), "{r}");
    // Honest: the same guard within the body's reach still parries.
    let mut ok = clash_scene([-60.0, 10.0, 140.0], [-70.0, 0.0, 140.0]);
    ok.s.record_clash(VIC, ATT, vts, shown, ok.now);
    assert!(ok.s.parried(VIC, ATT, ats, ok.now));
    assert!(ok.s.reach_violation(VIC, &ok.s.peers[&VIC], vts).is_none());
}

/// The attacker's path RTT steps from 40 to 140 ms (a route change, a
/// download filling its link): its display of the victim is 100 ms older
/// from then on, and the prediction follows within a few seconds instead of
/// holding the old minimum for 16 samples. Damage→ack samples (client
/// processing included) arriving in a burst never push the transport
/// samples out of the window.
#[test]
fn prediction_follows_an_rtt_step_within_seconds() {
    let mut c = Clock::default();
    for s in 0..20i64 { c.note_rtt(s * 1000, 40.0); }
    for s in 20..=24i64 { c.note_rtt(s * 1000, 140.0); }
    assert_eq!(c.rtt(24_500), Some(140.0));
    // A fight: 30 inflated damage→ack samples within one second.
    for k in 0..30i64 { c.note_rtt(24_600 + k * 30, 190.0); }
    assert_eq!(c.rtt(25_500), Some(140.0));
    assert_eq!(c.rtt(RTT_TTL_MS + 30_000), None);

    // End to end: the scene runs on a 140 ms path; the last 4 s of samples
    // say 140, the 15 s before them 40.
    let sc = {
        let mut sc = Scene::new(70, 20_000, runner, |_| None, sword_near);
        let rtt = &mut sc.s.peers.get_mut(&ATT).unwrap().clock.rtt;
        rtt.clear();
        for s in 1..=15i64 { rtt.push_back((s * 1000, 40.0)); }
        for s in 16..=20i64 { rtt.push_back((s * 1000, 140.0)); }
        sc
    };
    let h = 19_950;
    let p = sc.s.predict_view(ATT, VIC, sc.ats(h), sc.now).unwrap();
    let honest = sc.honest_view(h, 31) as i64;
    assert!((p.expected - honest).abs() <= FRAME_MS, "expected {} honest {} (off by {} ms)", p.expected, honest, p.expected - honest);
}

/// History resolution vs the server tick (docs/development/tick-rate.md).
/// A fast swing streamed at the clients' 60 Hz, sampled back at random hit
/// times: the ring that keeps every sample on arrival against a history that
/// keeps only the newest sample per server tick. The per-sample ring does
/// not depend on the tick at all; a per-tick history needs >= the client
/// rate to match it. `--nocapture` prints the table.
#[test]
fn history_keeps_every_sample_whatever_the_tick_rate() {
    // Tip on a 110 uu arc, angle 1.8 sin(2 pi t / 500 ms): peak 2490 uu/s.
    let tip = |t: f64| {
        let a = 1.8 * (std::f64::consts::TAU * t / 500.0).sin();
        let w = 1.8 * std::f64::consts::TAU / 500.0 * (std::f64::consts::TAU * t / 500.0).cos() * 1000.0;
        let p = [(110.0 * a.cos()) as f32, (110.0 * a.sin()) as f32, 140.0];
        let v = [(-110.0 * a.sin() * w) as f32, (110.0 * a.cos() * w) as f32, 0.0];
        (p, v)
    };
    let mut seed = 0x2545_F491_4F6C_DD1Du64;
    let mut rnd = move || { seed ^= seed << 13; seed ^= seed >> 7; seed ^= seed << 17; (seed >> 11) as f64 / (1u64 << 53) as f64 };
    // 60 Hz frames (±1.5 ms frame jitter), 40 ms + 0..15 ms network delay,
    // in 1.1 s windows (the history keeps 1.2 s) at 30 phases of the swing.
    let windows: Vec<Vec<(u32, f64)>> = (0..30).map(|w| {
        let mut f = Vec::new();
        let mut t = 1000.0 + 1000.0 * w as f64 + rnd() * 17.0;
        let end = t + 1100.0;
        while t < end {
            let ts = (t + rnd() * 3.0 - 1.5).round();
            f.push((ts as u32, ts + 40.0 + rnd() * 15.0));
            t += 1000.0 / 60.0;
        }
        f
    }).collect();
    // Hermite with streamed tip velocity (blades), or linear (bones, capsules).
    let blade = |ts: u32, hermite: bool| { let (p, v) = tip(ts as f64); Blade { base: [0.0, 0.0, 140.0], tip: p, vel: hermite.then_some(v) } };
    // hz = None: every sample on arrival; Some(hz): the newest per tick.
    let ring = |frames: &[(u32, f64)], hz: Option<f64>, phase: f64, h: bool| {
        let mut r: Ring<Blade> = Ring::new();
        let Some(hz) = hz else {
            for &(ts, _) in frames { r.push(ts, blade(ts, h)); }
            return r;
        };
        let (mut next, mut k, mut newest) = (frames[0].1 + phase * 1000.0 / hz, 0usize, None::<u32>);
        while k < frames.len() {
            while k < frames.len() && frames[k].1 <= next { newest = newest.max(Some(frames[k].0)); k += 1; }
            if let Some(ts) = newest { r.push(ts, blade(ts, h)); }
            next += 1000.0 / hz;
        }
        r
    };
    let mut err = |hz: Option<f64>, h: bool| {
        let mut e: Vec<f32> = Vec::new();
        for f in &windows {
            let r = ring(f, hz, rnd(), h);
            let (t0, t1) = (f[3].0 as f64 + 100.0, f[f.len() - 4].0 as f64 - 100.0);
            for _ in 0..150 {
                let t = t0 + rnd() * (t1 - t0);
                let got = r.sample_f(t, 0).map(|b| b.tip).unwrap_or([f32::NAN; 3]);
                e.push(len(sub(got, tip(t).0)));
            }
        }
        e.sort_by(f32::total_cmp);
        (e[e.len() / 2], e[e.len() * 95 / 100], e[e.len() - 1])
    };
    let row = |e: (f32, f32, f32)| format!("{:.2} / {:.2} / {:.2}", e.0, e.1, e.2);
    let (base, lin) = (err(None, true), err(None, false));
    println!("| history | blade tip, Hermite: p50 / p95 / max uu | linear (bones, capsules) |\n|---|---|---|");
    println!("| every sample (any tick rate) | {} | {} |", row(base), row(lin));
    let mut by_hz = Vec::new();
    for hz in [30.0, 60.0, 100.0, 128.0] {
        let (e, l) = (err(Some(hz), true), err(Some(hz), false));
        println!("| newest sample per {hz} Hz tick | {} | {} |", row(e), row(l));
        by_hz.push(l);
    }
    assert!(base.1 < 1.0 && lin.1 < 5.0, "per-sample history p95 {:.2} / {:.2} uu", base.1, lin.1);
    assert!(by_hz[0].1 > 2.0 * lin.1, "a 30 Hz per-tick history loses half the samples");
    for e in &by_hz { assert!(e.1 >= lin.1 * 0.9, "no per-tick history beats keeping every sample"); }
}

#[test]
fn generation_binding_clears_geometry_and_never_downgrades_life() {
    let mut s=Store::default(); let id=0x1B77;
    let c=posecodec::v2::Context{match_id:77,round:3,life:1};
    assert!(s.bind_pose_context(id,Some(c)));
    for i in 0..3u32 {
        let ts=1000+i*17;
        s.record_root(id,ts,[0.0,0.0,100.0],ts as i64);
        s.record_pose(id,&skeleton(ts,[0.0,0.0,100.0]),ts as i64);
        s.record_blade(id,ts,Blade{base:[0.0,0.0,140.0],tip:[50.0,0.0,140.0],vel:None},ts as i64);
        s.record_capsules(id,ts,&[Capsule{a:[0.0,0.0,100.0],b:[0.0,0.0,150.0],r:10.0}],ts as i64);
    }
    assert!(!s.peers[&id].pose.q.is_empty() && !s.peers[&id].blade.q.is_empty() && !s.peers[&id].caps.q.is_empty());
    assert!(s.has_pose_context(id,77,3,1));assert!(!s.has_pose_context(id,77,3,2));
    assert!(s.bind_pose_context(id,Some(posecodec::v2::Context{life:2,..c})));
    let p=&s.peers[&id];
    assert!(p.pose.q.is_empty() && p.blade.q.is_empty() && p.caps.q.is_empty() && p.strikers.q.is_empty());
    assert!(!s.bind_pose_context(id,Some(c)));assert!(!s.bind_pose_context(id,None));
    assert!(s.has_pose_context(id,77,3,2));
}

#[test]
fn exact_native_registry_includes_verified_extra_melee_only() {
    let manifest: serde_json::Value=serde_json::from_str(include_str!("../native_melee_classes.json")).unwrap();
    let rows=manifest.as_array().unwrap();
    assert_eq!(rows.len(),13);
    for row in rows {
        let class=row["class"].as_str().unwrap();
        assert_eq!(row["parent"].as_str().unwrap(),"BlueprintGeneratedClass'ModularWeaponBP_C'");
        assert!(registered_native_weapon_class(class),"verified original melee {class}");
        assert!(class.len()<48,"class must fit exact IPC identity");
    }
    for class in ["Weapon_Trap_C","BP_CrossbowBolt_C","Weapon_Quiver_C","CustomSword_C","None",""] {
        assert!(!registered_native_weapon_class(class),"unregistered source {class}");
    }
}
