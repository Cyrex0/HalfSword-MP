//! Grabs and shoves (kinds `0x06xx`, cap `INTERACT`; docs/development/subsystems/interact.md).
//!
//! One record, [`Interact`], carries every interaction event in every direction:
//!
//! - G2S (`IPC.send("interact", t)`): what HSMPInteract did to a stand-in. `gid` is the mod's
//!   own grab id; `id` is ignored (the sidecar mints the wire grab id and patches it in place).
//! - C2S: the same bytes, `id` = the sidecar's wire grab id (impulse sequence for impulses).
//!   A grab update travels as `interact_grab_r` / `interact_grab_l` (same layout) so the
//!   reliable channel supersedes an unacked update per grabbing hand.
//! - S2C: `WireHdr::peer` = the initiator (0 = the server, about OUR grab: a denial or a
//!   server-side end). The server forwards the validated (possibly clamped) record.
//! - S2G (`IPC.events("interact")`): the S2C record copied as it is, the ring record's `peer`
//!   = `WireHdr::peer`. For server answers the sidecar patches `gid` back to the mod's id.
//!
//! The bone travels as its index in `HERO_BONES` (= `posecodec::HERO_BONES`, Lua `C.HERO`).
//!
//! `pose_yield` (bus key, game-local): HSMPInteract asks HSMPAvatars to let a stand-in yield
//! while it is grabbed / shoved (one row per peer).

use super::{flow, Chan, Dir, EnumInfo, RecordInfo, SlotForm, SlotInfo, CAP_BUS, CAP_INTERACT};
use crate::record::Invalid;

// ---- the interaction record -----------------------------------------------------------------

crate::ipc_pod! {
    /// One interaction event (48 bytes). Positions are UE units (cm).
    pub struct Interact {
        /// `interact_kind` code.
        pub kind: u8,
        /// Grabbing hand: 0 right, 1 left (impulses: 0).
        pub hand: u8,
        /// Affected bone: index into [`HERO_BONES`].
        pub bone: u8,
        pub _r: u8,
        /// The peer whose body is affected (the grabbed / shoved one).
        pub target_peer: u32,
        /// Wire grab id (per initiator connection, minted by its sidecar, never 0) / impulse
        /// sequence number. Set by the sidecar; the game's value is ignored.
        pub id: u32,
        /// Initiator's sender clock, ms (the pose stream's ts).
        pub ts: u32,
        /// Grip / contact point in the affected bone's local frame.
        pub point: [f32; 3],
        /// Impulse: world impulse (kg·cm/s). Grab: the grabber's hand world position at `ts`
        /// (fallback anchor when its stand-in hand is unknown).
        pub vector: [f32; 3],
        /// The initiating mod's own grab id (G2S; echoed back in S2G server answers).
        pub gid: u32,
        pub _r2: u32,
    }
}

/// `Interact::kind` codes (append only).
pub mod kind {
    pub const GRAB_START: u8 = 1;
    pub const GRAB_UPDATE: u8 = 2;
    pub const GRAB_END: u8 = 3;
    pub const IMPULSE: u8 = 4;
    /// Server -> initiator: its grab was refused (`target_peer`, `id`).
    pub const GRAB_DENIED: u8 = 5;
}

/// Hero bones in wire-index order (= `server::posecodec::HERO_BONES`, Lua `interact_core.HERO`).
pub const HERO_BONES: [&str; 16] = [
    "Pelvis", "Spine_02", "Spine_04", "Head",
    "Upperarm_L", "Lowerarm_L", "Hand_L",
    "Upperarm_R", "Lowerarm_R", "Hand_R",
    "Thigh_L", "Calf_L", "Foot_L",
    "Thigh_R", "Calf_R", "Foot_R",
];

/// Bone-local contact / grip points (cm): anything further is garbage.
pub const LOCAL_BOUND: f32 = 1.0e4;
/// Impulses (kg·cm/s) and grab anchors (world cm).
pub const VECTOR_BOUND: f32 = 1.0e8;
/// Peer ids are 24-bit on the wire (channel keys).
pub const PEER_LIMIT: u32 = 1 << 24;

fn within(v: &[f32; 3], b: f32) -> bool {
    v.iter().all(|x| x.abs() <= b)
}

fn check_interact(e: &Interact) -> Result<(), Invalid> {
    if !(kind::GRAB_START..=kind::GRAB_DENIED).contains(&e.kind) {
        return Err(Invalid::Range("kind"));
    }
    if e.hand > 1 {
        return Err(Invalid::Range("hand"));
    }
    if e.bone as usize >= HERO_BONES.len() {
        return Err(Invalid::Range("bone"));
    }
    if e.target_peer >= PEER_LIMIT {
        return Err(Invalid::Range("target_peer"));
    }
    if !within(&e.point, LOCAL_BOUND) {
        return Err(Invalid::Range("point"));
    }
    if !within(&e.vector, VECTOR_BOUND) {
        return Err(Invalid::Range("vector"));
    }
    Ok(())
}

pub const K_INTERACT: u16 = 0x0610;
/// Wire-only aliases of [`K_INTERACT`] for `GRAB_UPDATE` (per-hand supersede streams).
pub const K_INTERACT_GRAB_R: u16 = 0x0611;
pub const K_INTERACT_GRAB_L: u16 = 0x0612;

/// Supersede streams of the grab-update aliases (`proto_v5::keys::key(stream, peer)`):
/// keyed per grabbing hand (C2S) and per (initiator, hand) (S2C).
pub const STREAM_GRAB_R: u8 = 0x86;
pub const STREAM_GRAB_L: u8 = 0x88;

crate::record!(Interact, kind = K_INTERACT, name = "interact", check = check_interact);

/// Is `k` one of this domain's network kinds (the record or a grab-update alias)?
pub fn is_interact_kind(k: u16) -> bool {
    matches!(k, K_INTERACT | K_INTERACT_GRAB_R | K_INTERACT_GRAB_L)
}

/// The wire kind an interaction record travels as.
pub fn wire_kind(e: &Interact) -> u16 {
    match (e.kind, e.hand) {
        (kind::GRAB_UPDATE, 0) => K_INTERACT_GRAB_R,
        (kind::GRAB_UPDATE, _) => K_INTERACT_GRAB_L,
        _ => K_INTERACT,
    }
}

// ---- pose_yield (game-local bus) --------------------------------------------------------------

crate::ipc_pod! {
    /// One stand-in that should yield (HSMPAvatars' compliance hook).
    pub struct YieldRow {
        /// Until this time on the shared process clock (`os.clock() * 1000`, ms).
        pub until_ms: u64,
        /// Peer id of the stand-in.
        pub peer: u32,
        /// Servo gain while yielding (0..1).
        pub gain: f32,
        /// Linear / angular speed caps while yielding (uu/s, deg/s).
        pub cap_lin: f32,
        pub cap_ang: f32,
    }

    /// The `pose_yield` bus key: `n` rows of [`YieldRow`] (`t.rows` in Lua).
    pub struct PoseYield {
        pub n: u32,
        pub _r: u32,
    }
}

pub const K_POSE_YIELD: u16 = 0x0620;
/// One row per peer slot at most.
pub const YIELD_MAX: usize = super::pose::MAX_PEER_SLOTS;

fn check_yield_row(_h: &PoseYield, r: &YieldRow) -> Result<(), Invalid> {
    if r.peer == 0 || r.peer >= PEER_LIMIT {
        return Err(Invalid::Range("peer"));
    }
    if !(0.0..=1.0).contains(&r.gain) {
        return Err(Invalid::Range("gain"));
    }
    if !(0.0..=1.0e5).contains(&r.cap_lin) || !(0.0..=1.0e5).contains(&r.cap_ang) {
        return Err(Invalid::Range("cap"));
    }
    Ok(())
}

crate::record!(PoseYield, kind = K_POSE_YIELD, name = "pose_yield", rows = YieldRow, count = n, max = YIELD_MAX,
    check_row = check_yield_row);

// ---- tables ---------------------------------------------------------------------------------------

/// Legacy codec kinds of this domain: none (`interact` / `interact_in` moved to [`Interact`]).
pub const KINDS: &[super::KindInfo] = &[];

/// Record kinds of this domain (wire + shared memory).
pub const RECORDS: &[RecordInfo] = &[
    crate::record_info!(Interact, cap = CAP_INTERACT, flow = flow::C2S | flow::S2C | flow::G2S | flow::S2G,
        chan = Chan::Reliable, doc = "interaction event (grab start / update / end, impulse, grab denied); S2C peer = initiator (0 = server)"),
    crate::record_info!(alias K_INTERACT_GRAB_R, "interact_grab_r", Interact, cap = CAP_INTERACT, flow = flow::C2S | flow::S2C,
        chan = Chan::RelLatest(STREAM_GRAB_R), doc = "grab update of the right hand (wire alias of interact)"),
    crate::record_info!(alias K_INTERACT_GRAB_L, "interact_grab_l", Interact, cap = CAP_INTERACT, flow = flow::C2S | flow::S2C,
        chan = Chan::RelLatest(STREAM_GRAB_L), doc = "grab update of the left hand (wire alias of interact)"),
    crate::record_info!(PoseYield, cap = CAP_BUS, flow = flow::LOCAL, chan = Chan::None,
        doc = "stand-ins that yield to a local grab / shove (HSMPInteract -> HSMPAvatars)"),
];

/// Named record slots of this domain.
pub const SLOTS: &[SlotInfo] = &[SlotInfo {
    name: "pose_yield",
    kind: K_POSE_YIELD,
    form: SlotForm::Bus,
    dir: Dir::Local,
    cap: CAP_BUS,
    world_scoped: true,
    doc: "HSMPInteract's yield requests, read by HSMPAvatars",
}];

/// Code tables of this domain (Lua: `S.ENUMS.<name>.<VALUE>`, C: `HSMP_<NAME>_<VALUE>`).
pub const ENUMS: &[EnumInfo] = &[
    EnumInfo {
        name: "interact_kind",
        values: &[
            ("GRAB_START", kind::GRAB_START as u32),
            ("GRAB_UPDATE", kind::GRAB_UPDATE as u32),
            ("GRAB_END", kind::GRAB_END as u32),
            ("IMPULSE", kind::IMPULSE as u32),
            ("GRAB_DENIED", kind::GRAB_DENIED as u32),
        ],
    },
    EnumInfo {
        name: "hero_bone",
        values: &[
            ("Pelvis", 0), ("Spine_02", 1), ("Spine_04", 2), ("Head", 3),
            ("Upperarm_L", 4), ("Lowerarm_L", 5), ("Hand_L", 6),
            ("Upperarm_R", 7), ("Lowerarm_R", 8), ("Hand_R", 9),
            ("Thigh_L", 10), ("Calf_L", 11), ("Foot_L", 12),
            ("Thigh_R", 13), ("Calf_R", 14), ("Foot_R", 15),
        ],
    },
];

#[cfg(test)]
mod tests {
    use super::*;
    use crate::record::{to_payload, view};

    fn ev() -> Interact {
        Interact { kind: kind::GRAB_START, hand: 1, bone: 5, target_peer: 2, id: 7, ts: 1234,
                   point: [1.0, -2.5, 3.0], vector: [10.0, 20.0, 140.0], gid: 41, ..Default::default() }
    }

    #[test]
    fn interact_layout_and_round_trip() {
        assert_eq!(core::mem::size_of::<Interact>(), 48);
        let p = to_payload(&ev(), &[]);
        assert_eq!(p.len(), 48);
        let v = view::<Interact>(&p).unwrap();
        assert_eq!(v.head(), ev());
        // Unaligned input: copied, same value.
        let mut un = vec![0u8];
        un.extend_from_slice(&p);
        assert_eq!(view::<Interact>(&un[1..]).unwrap().head(), ev());
        assert!(view::<Interact>(&p[..47]).is_err());
        let mut long = p.clone();
        long.push(0);
        assert!(matches!(view::<Interact>(&long), Err(Invalid::Size { .. })));
        for name in ["interact", "interact_grab_r", "interact_grab_l"] {
            let r = super::super::record_by_name(name).unwrap();
            assert_eq!(r.layout, "Interact");
            assert!((r.check)(&p).is_ok(), "{name}");
        }
    }

    #[test]
    fn interact_range_checks() {
        let bad = |e: Interact| view::<Interact>(&to_payload(&e, &[])).unwrap_err();
        assert_eq!(bad(Interact { kind: 0, ..ev() }), Invalid::Range("kind"));
        assert_eq!(bad(Interact { kind: 6, ..ev() }), Invalid::Range("kind"));
        assert_eq!(bad(Interact { hand: 2, ..ev() }), Invalid::Range("hand"));
        assert_eq!(bad(Interact { bone: 16, ..ev() }), Invalid::Range("bone"));
        assert_eq!(bad(Interact { target_peer: 1 << 24, ..ev() }), Invalid::Range("target_peer"));
        assert_eq!(bad(Interact { point: [0.0, 2.0e4, 0.0], ..ev() }), Invalid::Range("point"));
        assert_eq!(bad(Interact { vector: [0.0, 0.0, -2.0e8], ..ev() }), Invalid::Range("vector"));
        assert_eq!(bad(Interact { point: [f32::NAN, 0.0, 0.0], ..ev() }), Invalid::Float("point"));
        assert_eq!(bad(Interact { vector: [0.0, f32::INFINITY, 0.0], ..ev() }), Invalid::Float("vector"));
        for k in kind::GRAB_START..=kind::GRAB_DENIED {
            assert!(view::<Interact>(&to_payload(&Interact { kind: k, ..ev() }, &[])).is_ok());
        }
    }

    #[test]
    fn grab_updates_travel_per_hand() {
        assert_eq!(wire_kind(&Interact { kind: kind::GRAB_UPDATE, hand: 0, ..ev() }), K_INTERACT_GRAB_R);
        assert_eq!(wire_kind(&Interact { kind: kind::GRAB_UPDATE, hand: 1, ..ev() }), K_INTERACT_GRAB_L);
        for k in [kind::GRAB_START, kind::GRAB_END, kind::IMPULSE, kind::GRAB_DENIED] {
            assert_eq!(wire_kind(&Interact { kind: k, ..ev() }), K_INTERACT);
        }
        assert!(is_interact_kind(K_INTERACT_GRAB_L) && !is_interact_kind(K_POSE_YIELD));
        assert_ne!(STREAM_GRAB_R, STREAM_GRAB_L);
    }

    #[test]
    fn hero_bone_enum_matches_the_list() {
        let e = super::super::enum_by_name("hero_bone").unwrap();
        for (i, b) in HERO_BONES.iter().enumerate() {
            assert_eq!(e.value_of(b), Some(i as u32));
        }
        assert_eq!(e.values.len(), HERO_BONES.len());
    }

    #[test]
    fn pose_yield_rows_round_trip_and_checks() {
        let row = |peer: u32| YieldRow { until_ms: 123_456, peer, gain: 0.15, cap_lin: 150.0, cap_ang: 150.0 };
        let p = to_payload(&PoseYield::default(), &[row(2), row(3)]);
        assert_eq!(p.len(), 8 + 2 * 24);
        let v = view::<PoseYield>(&p).unwrap();
        assert_eq!(v.head.n, 2);
        assert_eq!(&*v.rows, &[row(2), row(3)]);
        assert!(view::<PoseYield>(&to_payload(&PoseYield::default(), &[])).is_ok(), "no yields = zero rows");
        let bad = |r: YieldRow| view::<PoseYield>(&to_payload(&PoseYield::default(), &[r])).unwrap_err();
        assert_eq!(bad(row(0)), Invalid::Range("peer"));
        assert_eq!(bad(YieldRow { gain: 1.5, ..row(2) }), Invalid::Range("gain"));
        assert_eq!(bad(YieldRow { cap_ang: -1.0, ..row(2) }), Invalid::Range("cap"));
        assert_eq!(bad(YieldRow { gain: f32::NAN, ..row(2) }), Invalid::Float("gain"));
        // Hostile count.
        let mut h = bytemuck::bytes_of(&PoseYield { n: 33, _r: 0 }).to_vec();
        h.extend(std::iter::repeat_n(0u8, 33 * 24));
        assert!(matches!(view::<PoseYield>(&h), Err(Invalid::Rows { n: 33, .. })));
        let s = super::super::slot_by_name("pose_yield").unwrap();
        assert_eq!((s.kind, s.form), (K_POSE_YIELD, SlotForm::Bus));
        assert!(crate::record::payload_len::<PoseYield>(YIELD_MAX) <= super::super::bus::BUS_VALUE_BYTES);
    }

    /// Arbitrary bytes and mutations of valid records never panic the validator, and an
    /// accepted record always satisfies every range.
    #[test]
    fn decoder_fuzz() {
        let mut s: u64 = 0x1A9_5EED;
        let mut rnd = move || {
            s ^= s << 13;
            s ^= s >> 7;
            s ^= s << 17;
            s
        };
        let seeds = [
            to_payload(&ev(), &[]),
            to_payload(&Interact { kind: kind::IMPULSE, hand: 0, ..ev() }, &[]),
            to_payload(&PoseYield::default(), &[YieldRow { until_ms: 1, peer: 2, gain: 0.3, cap_lin: 1.0, cap_ang: 1.0 }]),
        ];
        for i in 0..20_000 {
            let mut d: Vec<u8> = if i % 5 == 0 {
                (0..(rnd() % 96) as usize).map(|_| rnd() as u8).collect()
            } else {
                seeds[i % seeds.len()].clone()
            };
            for _ in 0..(rnd() % 6) {
                if !d.is_empty() {
                    let j = (rnd() as usize) % d.len();
                    d[j] = rnd() as u8;
                }
            }
            for k in [K_INTERACT, K_INTERACT_GRAB_R, K_INTERACT_GRAB_L, K_POSE_YIELD] {
                let _ = super::super::check_payload(k, &d);
            }
            if let Ok(v) = view::<Interact>(&d) {
                let e = v.head();
                assert!(check_interact(&e).is_ok());
                assert!(e.point.iter().chain(e.vector.iter()).all(|x| x.is_finite()));
            }
            if let Ok(v) = view::<PoseYield>(&d) {
                assert!(v.rows.len() <= YIELD_MAX && v.rows.iter().all(|r| r.peer != 0 && r.gain.is_finite()));
            }
        }
    }
}
