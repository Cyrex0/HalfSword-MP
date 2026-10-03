//! Interaction-channel geometry (docs/development/subsystems/interact.md): how far the initiator's
//! hand (grab) or body (impulse) was from the target bone it claims to have
//! touched, both rewound like a hit. The initiator at its own `ts`; the
//! target at the server-predicted instant the initiator was displaying
//! (`predict_view`), searched over ± the prediction tolerance.

use super::*;

/// Result of an interaction distance query.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Geo {
    /// No pose history / clock map for one side (legacy client, first frames).
    NoData,
    /// Closest distance found (uu). Grab: hand centre → bone; impulse:
    /// initiator body capsule surface → bone.
    Dist(f32),
    /// The target streams history but the event cannot be judged against it
    /// (no / uncovered timestamp, no initiator history): refuse, never fall
    /// back to the coarse root distance.
    Refused(&'static str),
}

impl Store {
    /// `hand`: Some(posecodec bone index of the grabbing hand) for a grab,
    /// None for a body-contact impulse.
    pub fn interact_distance(&self, initiator: PeerId, target: PeerId, bone: usize, hand: Option<usize>,
                             ts: u32, now_ms: i64) -> Geo {
        if bone >= BONE_COUNT || hand.map_or(false, |h| h >= BONE_COUNT) {
            return Geo::NoData;
        }
        // Like a hit (lagcomp::evaluate): once the target streams history the
        // initiator's timestamp is mandatory and must be covered by its own
        // stream. Otherwise ts = 0 (or garbage) would fall back to the coarse
        // 4 m root check.
        let target_hist = self.peers.get(&target).map_or(false, |v| v.has_history());
        let missing = |why: &'static str| if target_hist { Geo::Refused(why) } else { Geo::NoData };
        if ts == 0 { return missing("no_ts: missing timestamp (target has history)"); }
        let Some(v) = self.peers.get(&target) else { return Geo::NoData };
        let Some(a) = self.peers.get(&initiator) else { return missing("no_history: no initiator history") };
        // A stale ts: an event older than the rewind cap (+ the
        // claim delivery credit) behind the initiator's own stream is refused
        // like a hit's, instead of rewinding the target past its history.
        if let Some(n) = a.newest_any() {
            if rel(n, ts) > self.max_rewind() + DELIVERY_CREDIT_MS {
                return missing("ts_old: interaction ts older than the rewind cap");
            }
        }
        let Some(ap) = a.pose.sample(ts, FUTURE_MS) else { return missing("ts_uncovered: ts not covered by the initiator's pose stream") };
        let Some(pred) = self.predict_view(initiator, target, ts, now_ms) else { return missing("no_view: no view prediction") };
        let caps = if hand.is_none() { Some(pose_capsules(&ap)) } else { None };
        let mut best = f32::INFINITY;
        for k in -2i64..=2 {
            let t = (pred.expected + pred.tol * k / 2).max(1) as u32;
            let Some(vp) = v.pose.sample(t, FUTURE_MS) else { continue };
            if vp.mask & (1 << bone) == 0 { continue; }
            let pt = vp.p[bone];
            let d = match (hand, &caps) {
                (Some(h), _) if ap.mask & (1 << h) != 0 => len(sub(ap.p[h], pt)),
                (None, Some(cs)) if cs.n > 0 => caps_dist(cs, pt).max(0.0),
                _ => continue,
            };
            best = best.min(d);
        }
        // No target pose covers the predicted instant: with target history this
        // is a refusal, never the coarse 4 m root fallback.
        if best.is_finite() { Geo::Dist(best) } else { missing("no_cover: target history does not cover the view") }
    }
}

/// Global-store wrapper (server dispatch).
pub fn interact_distance(initiator: PeerId, target: PeerId, bone: usize, hand: Option<usize>, ts: u32) -> Geo {
    store().lock().unwrap().interact_distance(initiator, target, bone, hand, ts, now_ms())
}

#[cfg(test)]
mod tests {
    use super::*;
    use posecodec::*;

    /// Standing Willie at `root`, arms forward (+x).
    fn frame(ts: u32, root: V3) -> PoseFrame {
        let o = |x: f32, y: f32, z: f32| [root[0] + x, root[1] + y, root[2] + z];
        let p = [
            o(0., 0., 0.), o(0., 0., 25.), o(0., 0., 50.), o(0., 0., 75.),
            o(0., 20., 48.), o(20., 25., 40.), o(40., 20., 40.),
            o(0., -20., 48.), o(25., -25., 40.), o(45., -20., 40.),
            o(0., 10., -10.), o(0., 10., -50.), o(0., 10., -90.),
            o(0., -10., -10.), o(0., -10., -50.), o(0., -10., -90.),
        ];
        PoseFrame { ts, bones: p.iter().enumerate().map(|(i, v)| (i as u8, *v, [0.0, 0.0, 0.0, 1.0])).collect() }
    }

    /// Both peers stream 60 Hz poses for 1 s on clocks offset by 5000 ms;
    /// the server receives each frame 20 ms after it was taken.
    fn store_with(a_root: V3, v_root: V3) -> (Store, u32, i64) {
        let mut s = Store::default();
        let mut now = 100_000i64;
        let mut ts_a = 10_000u32;
        for _ in 0..60 {
            s.record_root(1, ts_a, a_root, now);
            s.record_root(2, ts_a + 5000, v_root, now);
            s.record_pose(1, &frame(ts_a, a_root), now);
            s.record_pose(2, &frame(ts_a + 5000, v_root), now);
            now += 16;
            ts_a += 16;
        }
        s.note_rtt(1, 40.0, now);
        s.note_rtt(2, 40.0, now);
        (s, ts_a - 16, now)
    }

    #[test]
    fn grab_reach_uses_the_hand_and_the_rewound_bone() {
        // Victim 60 uu in front.
        let (s, ts, now) = store_with([0.0, 0.0, 100.0], [60.0, 0.0, 100.0]);
        let Geo::Dist(d) = s.interact_distance(1, 2, LOWERARM_R, Some(HAND_R), ts, now) else { panic!("no data") };
        // victim Lowerarm_R at (85,-25,140), attacker Hand_R at (45,-20,140)
        assert!((d - 40.3).abs() < 1.0, "d={d}");
        // Far away victim: distance grows.
        let (s, ts, now) = store_with([0.0, 0.0, 100.0], [600.0, 0.0, 100.0]);
        let Geo::Dist(d) = s.interact_distance(1, 2, LOWERARM_R, Some(HAND_R), ts, now) else { panic!("no data") };
        assert!(d > 500.0, "d={d}");
    }

    #[test]
    fn impulse_reach_uses_the_body_capsules() {
        let (s, ts, now) = store_with([0.0, 0.0, 100.0], [30.0, 0.0, 100.0]);
        // Victim pelvis 30 uu in front: 12 uu from the attacker's torso
        // capsule surface (radius 18).
        let Geo::Dist(d) = s.interact_distance(1, 2, PELVIS, None, ts, now) else { panic!() };
        assert!((d - 12.0).abs() < 0.5, "d={d}");
        let (s, ts, now) = store_with([0.0, 0.0, 100.0], [10.0, 0.0, 100.0]);
        assert_eq!(s.interact_distance(1, 2, PELVIS, None, ts, now), Geo::Dist(0.0), "inside: 0");
        let (s, ts, now) = store_with([0.0, 0.0, 100.0], [300.0, 0.0, 100.0]);
        let Geo::Dist(d) = s.interact_distance(1, 2, PELVIS, None, ts, now) else { panic!() };
        assert!(d > 200.0, "d={d}");
    }

    #[test]
    fn missing_history_is_no_data() {
        let (s, ts, now) = store_with([0.0; 3], [50.0, 0.0, 0.0]);
        assert_eq!(s.interact_distance(1, 3, PELVIS, None, ts, now), Geo::NoData, "unknown target");
        assert_eq!(s.interact_distance(1, 2, 16, None, ts, now), Geo::NoData, "bad bone");
        // The target streams history, so a missing / uncovered ts
        // is refused instead of falling back to the 4 m root check.
        assert!(matches!(s.interact_distance(1, 2, PELVIS, None, ts + 10_000, now), Geo::Refused(_)), "ts far in the future");
        assert!(matches!(s.interact_distance(1, 2, PELVIS, None, 0, now), Geo::Refused(_)), "no ts");
        assert!(matches!(s.interact_distance(7, 2, PELVIS, None, ts, now), Geo::Refused(_)), "initiator without history");
        // A ts ~1.1 s old is still inside the initiator's ring
        // (so its pose resolves) but the predicted target instant is older
        // than the target's history: refused, not the 4 m root fallback.
        assert!(matches!(s.interact_distance(1, 2, PELVIS, None, ts - 1_100, now), Geo::Refused(_)), "stale ts");
        assert!(matches!(s.interact_distance(1, 2, PELVIS, None, ts - 600, now), Geo::Refused(_)), "older than the rewind cap");
        // No target history (legacy client, first frames): no data, the root fallback applies.
        let s = Store::default();
        assert_eq!(s.interact_distance(1, 2, PELVIS, None, 0, now), Geo::NoData);
    }
}
