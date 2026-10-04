//! Pose, root, weapon and the peer directory (kinds `0x01xx`, cap `POSE`).
//!
//! The hot streams are ABI 2 records: the game writes `root` / `weapon` /
//! `pose` (a codec v2 frame) once, at the source (HSMPNative `put_root` / `put_weapon` /
//! `put_pose`, `hsmp_pose::sample`), into the slots `local_root` / `local_weapon` /
//! `local_pose`; the sidecar frames the same bytes for the server; the server validates in
//! place and relays them (peer / aux patched); the receiving sidecar copies a relayed root
//! into the per-peer `peer_root` slot and feeds the pose to its jitter buffer. `PoseLead`,
//! `PeerPlay` and `PeerDir` are sidecar- / game-local typed slots (evaluated playback, not
//! wire data).

use super::{Dir, Form, KindInfo, SlotMeta, CAP_POSE};

/// Codec-v2 bones (HSMPSync `POSE_BONES`, `posecodec_v2::BONES`).
pub const NB: usize = 23;
/// Playback slots: the 23 bones + weapon_r + weapon_l (`poseplay::SLOTS`).
pub const PLAY_SLOTS: usize = NB + 2;
/// Floats per bone sample: px,py,pz, qx,qy,qz,qw, vx,vy,vz (uu/s), wx,wy,wz (deg/s).
pub const XF: usize = 13;
/// Peer slots in the peer table (indexed by slot, not peer id).
pub const MAX_PEER_SLOTS: usize = 32;
/// Nick capacity in bytes (UTF-8).
pub const NICK_BYTES: usize = 48;

crate::ipc_pod! {
    /// The control layer sampled from the Willie (`posecodec_v2::Control`).
    pub struct Control {
        /// Bit i = `CONTROL_FLAGS[i]` is true.
        pub flags: u32,
        pub grip_r: u32,
        pub grip_l: u32,
        /// Bit i: `ik[i]` is a world position (else raw component value).
        pub ik_world: u32,
        /// `CONTROL_SCALARS` order.
        pub scalars: [f32; 16],
        pub aim: [f32; 3],
        pub ctrl_pitch: f32,
        pub ctrl_yaw: f32,
        /// R out end, L out end, R out joint, L out joint.
        pub ik: [[f32; 3]; 4],
        pub _r: u32,
    }

    /// The game's requested playback look-ahead (replaces `.pose_lead`).
    pub struct PoseLead {
        pub meta: SlotMeta,
        pub lead_ms: f32,
        pub _r: u32,
    }

    /// Weapon geometry of one playback weapon slot (play line "W").
    pub struct PlayWeapon {
        pub present: u32,
        pub hands: u32,
        pub class_id: u32,
        pub _r: u32,
        /// Blade base and tip in weapon space.
        pub base: [f32; 3],
        pub tip: [f32; 3],
    }

    /// One remote peer's sampled playback (replaces `.pose_play<id>.json`). Written only by
    /// the sidecar's `hsmp-poseplay` thread.
    pub struct PeerPlay {
        pub meta: SlotMeta,
        pub peer_id: u32,
        /// `poseplay::Mode` as u32: see `PLAY_MODES`.
        pub mode: u32,
        /// Discontinuity counter: the game snaps the body when it changes.
        pub cut: u32,
        /// bit0 v2 (velocities present), bit1 `root` valid, bit2 `control` valid.
        pub flags: u32,
        /// The sidecar's per-peer play sequence (play line "seq").
        pub play_seq: u64,
        /// Sender-clock time the pose was evaluated at, ms (sub-ms; play line "ptf").
        pub pt: f64,
        pub age: f32,
        pub delay: f32,
        pub jit: f32,
        pub lead: f32,
        /// Sender physics step (play line "st"; 0 = none).
        pub st: f32,
        /// Frame interval the buffer is sized for (play line "iv").
        pub iv: f32,
        pub k: f32,
        /// Playback-clock rate (1 = real time; below 1 while the buffer starves, above
        /// while it catches up). 0 = unknown (an older sidecar).
        pub rate: f32,
        /// Slot mask (bit i = `b[i]` valid) and velocity mask.
        pub mask: u32,
        pub vmask: u32,
        /// Root: x, y, z, yaw.
        pub root: [f32; 4],
        /// `PLAY_SLOTS` slots: 23 bones, weapon_r, weapon_l; `XF` floats each.
        pub b: [[f32; 13]; 25],
        pub w: [PlayWeapon; 2],
        pub control: Control,
        pub _r2: u32,
    }

    /// One peer-table slot assignment.
    pub struct PeerDirEntry {
        pub peer_id: u32,
        /// Bumped by the sidecar every time the slot is (re)assigned.
        pub gen: u32,
        /// 1 = slot in use.
        pub active: u32,
        pub nick_len: u32,
        pub nick: [u8; 48],
        /// Server-measured smoothed RTT to this peer, ms (the `pings` record; our own entry:
        /// the transport srtt); 0 = unknown.
        pub rtt_ms: u32,
        pub _r_rtt: u32,
    }

    /// The peer directory (sidecar-written): peer id -> slot.
    pub struct PeerDir {
        pub meta: SlotMeta,
        pub count: u32,
        /// Bumped on every change of any entry.
        pub dir_gen: u32,
        pub entries: [PeerDirEntry; 32],
    }
}

/// `poseplay::Mode` values in `PeerPlay::mode`.
pub const PLAY_MODES: &[(&str, u32)] = &[("interp", 0), ("extrap", 1), ("hold", 2), ("stale", 3)];

pub const PEER_PLAY_V2: u32 = 1 << 0;
pub const PEER_PLAY_HAS_ROOT: u32 = 1 << 1;
pub const PEER_PLAY_HAS_CONTROL: u32 = 1 << 2;

pub const K_POSE_LEAD: u16 = 0x0104;
pub const K_PEER_DIR: u16 = 0x0140;
pub const K_PEER_PLAY: u16 = 0x0141;

pub const KINDS: &[KindInfo] = &[
    KindInfo { kind: K_POSE_LEAD, name: "pose_lead", cap: CAP_POSE, dir: Dir::GameToSidecar, form: Form::Slot, replaces: ".pose_lead" },
    KindInfo { kind: K_PEER_DIR, name: "peer_dir", cap: CAP_POSE, dir: Dir::SidecarToGame, form: Form::Slot, replaces: "me_remote<id>.json (liveness)" },
    KindInfo { kind: K_PEER_PLAY, name: "peer_play", cap: CAP_POSE, dir: Dir::SidecarToGame, form: Form::PeerSlot, replaces: ".pose_play<id>.json" },
];

impl PeerDirEntry {
    /// The nick as UTF-8, invalid sequences replaced with U+FFFD, length clamped.
    pub fn nick(&self) -> std::borrow::Cow<'_, str> {
        let n = (self.nick_len as usize).min(NICK_BYTES);
        String::from_utf8_lossy(&self.nick[..n])
    }

    /// Set the nick, truncated on a UTF-8 boundary to `NICK_BYTES`.
    pub fn set_nick(&mut self, s: &str) {
        let mut n = s.len().min(NICK_BYTES);
        while n > 0 && !s.is_char_boundary(n) {
            n -= 1;
        }
        self.nick = [0; NICK_BYTES];
        self.nick[..n].copy_from_slice(&s.as_bytes()[..n]);
        self.nick_len = n as u32;
    }
}

impl PeerDir {
    /// Slot of `peer_id`, if active.
    pub fn slot_of(&self, peer_id: u32) -> Option<usize> {
        self.entries.iter().position(|e| e.active == 1 && e.peer_id == peer_id)
    }
}

// ---- ABI 2 records (protocol v6) ----------------------------------------------------------

crate::ipc_pod! {
    /// Root (capsule) transform of one player at one sample: wire kind `root` (C2S from the
    /// owner, S2C relayed with `WireHdr::peer` = the owner). The native module writes it in
    /// this form (rotator -> quaternion once, at the source).
    pub struct Root {
        /// Owner's sample counter.
        pub tick: u32,
        /// Sender game clock, ms (the pose / weapon timeline).
        pub ts: u32,
        /// Sender wall clock at sampling, ms (one-way latency instrumentation).
        pub send_wall_ms: u64,
        /// World position, cm.
        pub pos: [f32; 3],
        /// Orientation quaternion x, y, z, w.
        pub rot: [f32; 4],
        /// Linear velocity, cm/s.
        pub vel: [f32; 3],
    }

    /// The held weapon's transform (wire kind `weapon`, C2S only: the server keeps it for lag
    /// compensation; peers get the weapon inside the pose frame).
    pub struct Weapon {
        pub tick: u32,
        pub ts: u32,
        pub weapon_id: u16,
        /// 0 none, 1 right, 2 left.
        pub held: u8,
        pub _r: u8,
        pub _r2: u32,
        pub pos: [f32; 3],
        pub rot: [f32; 4],
        pub vel: [f32; 3],
    }

    /// Head of a pose frame (wire kind `pose`): `n` bytes of pose codec v2 follow as rows. The
    /// game's native module quantises once, at the source; the bytes are never re-encoded.
    /// Relayed S2C with `WireHdr::peer` = owner and `WireHdr::aux` = the relay interval, ms.
    pub struct PoseHead {
        pub tick: u32,
        /// Frame bytes (the row count).
        pub n: u16,
        pub _r: u16,
    }

    /// A remote player's root as the receiving sidecar publishes it (per-peer slot
    /// `peer_root`): the relayed `root` record exactly as it arrived, plus its owner.
    pub struct PeerRoot {
        pub peer_id: u32,
        pub _r: u32,
        pub root: Root,
    }
}

/// Largest pose codec v2 frame (bytes). The largest frame the encoder can produce (2 weapons,
/// control, every translation override, the step byte) is 518 B.
pub const POSE_FRAME_MAX: usize = 640;

pub const K_ROOT: u16 = 0x0110;
pub const K_WEAPON: u16 = 0x0111;
pub const K_POSE: u16 = 0x0112;
pub const K_PEER_ROOT: u16 = 0x0113;

/// The game-written pose frame slot body (head + the full frame capacity).
pub type PoseBuf = crate::record::VarBuf<PoseHead, POSE_FRAME_MAX>;

/// World coordinate bound (cm): beyond this a position is garbage.
pub const WORLD_LIMIT: f32 = 1.0e7;

fn quat_ok(q: &[f32; 4]) -> bool {
    let n = q.iter().map(|c| c * c).sum::<f32>();
    (0.25..=2.25).contains(&n)
}

fn pos_ok(p: &[f32; 3]) -> bool {
    p.iter().all(|c| c.abs() <= WORLD_LIMIT)
}

fn check_root(r: &Root) -> Result<(), crate::record::Invalid> {
    use crate::record::Invalid;
    if !pos_ok(&r.pos) {
        return Err(Invalid::Range("pos"));
    }
    if !quat_ok(&r.rot) {
        return Err(Invalid::Range("rot"));
    }
    Ok(())
}

fn check_weapon(w: &Weapon) -> Result<(), crate::record::Invalid> {
    use crate::record::Invalid;
    if w.held > 2 {
        return Err(Invalid::Range("held"));
    }
    if !pos_ok(&w.pos) {
        return Err(Invalid::Range("pos"));
    }
    if !quat_ok(&w.rot) {
        return Err(Invalid::Range("rot"));
    }
    Ok(())
}

fn check_pose(h: &PoseHead) -> Result<(), crate::record::Invalid> {
    // A v2 frame is at least its 24-byte header (the server and the receivers also decode it).
    if h.n < 24 {
        return Err(crate::record::Invalid::Range("n"));
    }
    Ok(())
}

fn check_peer_root(r: &PeerRoot) -> Result<(), crate::record::Invalid> {
    if r.peer_id == 0 {
        return Err(crate::record::Invalid::Range("peer_id"));
    }
    check_root(&r.root)
}

crate::record!(Root, kind = K_ROOT, name = "root", check = check_root);
crate::record!(Weapon, kind = K_WEAPON, name = "weapon", check = check_weapon);
crate::record!(PoseHead, kind = K_POSE, name = "pose", rows = u8, count = n, max = POSE_FRAME_MAX, check = check_pose);
crate::record!(PeerRoot, kind = K_PEER_ROOT, name = "peer_root", check = check_peer_root);

crate::ipc_pod! {
    /// Which in-world Willie stands in for which peer (bus key `puppets`, HSMPAvatars ->
    /// HSMPCombat / HSMPLoadout / HSMPInteract / HSMPWorld / HSMPMatch). Look up with
    /// `FindAllOf("Willie_BP_C")` + `GetFName():ToString()`.
    pub struct Puppets {
        pub n: u16,
        pub _r: [u16; 3],
    }

    pub struct PuppetRow {
        pub peer: u32,
        pub _r: u32,
        /// The stand-in's actor FName.
        pub name: crate::layout::Str<56>,
    }

    /// The SENDER-clock time each peer's stand-in is showing (bus key `playback`, HSMPAvatars
    /// -> HSMPCombat: the server rewinds both players to it). Body, arms and weapon share one
    /// timeline (`body_ts == arm_ts`); `local_ms` is when that sample was read, so the reader
    /// can advance it.
    pub struct Playback {
        pub n: u16,
        pub _r: [u16; 3],
    }

    pub struct PlaybackRow {
        pub peer: u32,
        pub _r: u32,
        pub body_ts: f64,
        pub arm_ts: f64,
        pub local_ms: f64,
    }
}

pub const K_PUPPETS: u16 = 0x0150;
pub const K_PLAYBACK: u16 = 0x0151;

fn check_puppet_row(_h: &Puppets, r: &PuppetRow) -> Result<(), crate::record::Invalid> {
    if r.peer == 0 {
        return Err(crate::record::Invalid::Range("peer"));
    }
    Ok(())
}

fn check_playback_row(_h: &Playback, r: &PlaybackRow) -> Result<(), crate::record::Invalid> {
    if r.peer == 0 {
        return Err(crate::record::Invalid::Range("peer"));
    }
    Ok(())
}

crate::record!(Puppets, kind = K_PUPPETS, name = "puppets", rows = PuppetRow, count = n, max = MAX_PEER_SLOTS,
    check_row = check_puppet_row);
crate::record!(Playback, kind = K_PLAYBACK, name = "playback", rows = PlaybackRow, count = n, max = MAX_PEER_SLOTS,
    check_row = check_playback_row);

/// Record kinds of this domain (wire + shared memory).
pub const RECORDS: &[super::RecordInfo] = &[
    crate::record_info!(Root, cap = CAP_POSE, flow = super::flow::C2S | super::flow::S2C | super::flow::G2S,
        chan = super::Chan::Latest(1), doc = "root transform (owner sample; relayed with peer = owner)"),
    crate::record_info!(Weapon, cap = CAP_POSE, flow = super::flow::C2S | super::flow::G2S,
        chan = super::Chan::Latest(3), doc = "held weapon transform (server lag compensation only)"),
    crate::record_info!(PoseHead, cap = CAP_POSE, flow = super::flow::C2S | super::flow::S2C | super::flow::G2S,
        chan = super::Chan::Latest(2), doc = "pose codec v2 frame (rows = bytes); S2C aux = relay interval ms"),
    crate::record_info!(PeerRoot, cap = CAP_POSE, flow = super::flow::S2G, chan = super::Chan::None,
        doc = "a remote player's relayed root + its peer id (per-peer slot)"),
    crate::record_info!(Puppets, cap = super::CAP_BUS, flow = super::flow::LOCAL, chan = super::Chan::None,
        doc = "bus: which Willie stands in for which peer (HSMPAvatars -> the other mods)"),
    crate::record_info!(Playback, cap = super::CAP_BUS, flow = super::flow::LOCAL, chan = super::Chan::None,
        doc = "bus: the sender time each stand-in shows (HSMPAvatars -> HSMPCombat lag compensation)"),
];

/// Named record slots of this domain.
pub const SLOTS: &[super::SlotInfo] = &[
    super::SlotInfo { name: "local_root", kind: K_ROOT, form: super::SlotForm::Slot, dir: Dir::GameToSidecar, cap: CAP_POSE,
        world_scoped: true, doc: "the local player's root (put_root; the sidecar sends it as it is)" },
    super::SlotInfo { name: "local_weapon", kind: K_WEAPON, form: super::SlotForm::Slot, dir: Dir::GameToSidecar, cap: CAP_POSE,
        world_scoped: true, doc: "the local held weapon (put_weapon; server lag compensation only)" },
    super::SlotInfo { name: "local_pose", kind: K_POSE, form: super::SlotForm::Slot, dir: Dir::GameToSidecar, cap: CAP_POSE,
        world_scoped: true, doc: "the local pose as a codec v2 frame (put_pose encodes it once)" },
    super::SlotInfo { name: "peer_root", kind: K_PEER_ROOT, form: super::SlotForm::PeerSlot, dir: Dir::SidecarToGame, cap: CAP_POSE,
        world_scoped: false, doc: "a remote player's root (the relayed root record + peer id)" },
    super::SlotInfo { name: "puppets", kind: K_PUPPETS, form: super::SlotForm::Bus, dir: Dir::Local, cap: super::CAP_BUS,
        world_scoped: true, doc: "stand-in actor FName per peer (replaces .puppets.json)" },
    super::SlotInfo { name: "playback", kind: K_PLAYBACK, form: super::SlotForm::Bus, dir: Dir::Local, cap: super::CAP_BUS,
        world_scoped: true, doc: "sender time shown per stand-in (replaces .playback.json)" },
];

/// Code tables of this domain (Lua: `S.ENUMS.<name>.<VALUE>`, C: `HSMP_<NAME>_<VALUE>`).
pub const ENUMS: &[super::EnumInfo] = &[];

#[cfg(test)]
mod tests {
    use super::*;
    use crate::record::{to_payload, view, Invalid};

    fn root() -> Root {
        Root { tick: 7, ts: 1000, send_wall_ms: 1_700_000_000_000, pos: [100.0, -50.0, 95.0], rot: [0.0, 0.0, std::f32::consts::FRAC_1_SQRT_2, std::f32::consts::FRAC_1_SQRT_2], vel: [300.0, 0.0, 0.0] }
    }

    #[test]
    fn root_weapon_peer_root_round_trip_and_ranges() {
        let r = root();
        let p = to_payload(&r, &[]);
        assert_eq!(p.len(), 56);
        assert_eq!(view::<Root>(&p).unwrap().head(), r);
        assert!(matches!(view::<Root>(&p[..55]), Err(Invalid::Short { .. })));
        let mut bad = r;
        bad.pos[2] = f32::NAN;
        assert_eq!(view::<Root>(&to_payload(&bad, &[])).unwrap_err(), Invalid::Float("pos"));
        bad = r;
        bad.pos[0] = 2.0e7;
        assert_eq!(view::<Root>(&to_payload(&bad, &[])).unwrap_err(), Invalid::Range("pos"));
        bad = r;
        bad.rot = [0.0; 4];
        assert_eq!(view::<Root>(&to_payload(&bad, &[])).unwrap_err(), Invalid::Range("rot"));
        bad.rot = [2.0, 0.0, 0.0, 0.0];
        assert_eq!(view::<Root>(&to_payload(&bad, &[])).unwrap_err(), Invalid::Range("rot"));

        let w = Weapon { tick: 1, ts: 2, weapon_id: 77, held: 2, _r: 0, _r2: 0, pos: [1.0, 2.0, 3.0], rot: [0.0, 0.0, 0.0, 1.0], vel: [0.0; 3] };
        let p = to_payload(&w, &[]);
        assert_eq!(p.len(), 56);
        assert_eq!(view::<Weapon>(&p).unwrap().head(), w);
        let mut bw = w;
        bw.held = 3;
        assert_eq!(view::<Weapon>(&to_payload(&bw, &[])).unwrap_err(), Invalid::Range("held"));
        bw = w;
        bw.pos[1] = -1.0e8;
        assert_eq!(view::<Weapon>(&to_payload(&bw, &[])).unwrap_err(), Invalid::Range("pos"));
        bw = w;
        bw.rot = [0.1, 0.0, 0.0, 0.1];
        assert_eq!(view::<Weapon>(&to_payload(&bw, &[])).unwrap_err(), Invalid::Range("rot"));

        let pr = PeerRoot { peer_id: 3, _r: 0, root: r };
        let p = to_payload(&pr, &[]);
        assert_eq!(p.len(), 64);
        assert_eq!(&p[8..], bytemuck::bytes_of(&r), "peer_root = peer id + the root record bytes");
        assert_eq!(view::<PeerRoot>(&p).unwrap().head(), pr);
        let mut bp = pr;
        bp.peer_id = 0;
        assert_eq!(view::<PeerRoot>(&to_payload(&bp, &[])).unwrap_err(), Invalid::Range("peer_id"));
        bp = pr;
        bp.root.rot = [0.0; 4];
        assert_eq!(view::<PeerRoot>(&to_payload(&bp, &[])).unwrap_err(), Invalid::Range("rot"));
    }

    #[test]
    fn pose_record_sizes_and_hostile_counts() {
        let h = PoseHead { tick: 9, n: 0, _r: 0 };
        let frame = [0xFFu8, 0xFF, 0xFF, 0x02].iter().copied().chain(std::iter::repeat(7u8).take(300)).collect::<Vec<_>>();
        let p = to_payload(&h, &frame);
        assert_eq!(p.len(), 8 + 304);
        let v = view::<PoseHead>(&p).unwrap();
        assert_eq!((v.head.tick, v.head.n as usize, &v.rows[..]), (9, 304, &frame[..]));
        // too short to be a v2 frame
        assert_eq!(view::<PoseHead>(&to_payload(&h, &frame[..23])).unwrap_err(), Invalid::Range("n"));
        // count over the capacity / count and length disagree
        let mut big = p.clone();
        big[4..6].copy_from_slice(&((POSE_FRAME_MAX + 1) as u16).to_le_bytes());
        assert!(matches!(view::<PoseHead>(&big), Err(Invalid::Rows { .. })));
        let mut short = p.clone();
        short.pop();
        assert!(matches!(view::<PoseHead>(&short), Err(Invalid::Size { .. })));
        // the slot buffer's prefix is the wire payload
        let mut b = PoseBuf::new_boxed();
        b.set(&h, &frame);
        assert_eq!(b.payload(), &p[..]);
    }

    #[test]
    fn slots_and_records_are_registered() {
        for (s, k) in [("local_root", K_ROOT), ("local_weapon", K_WEAPON), ("local_pose", K_POSE), ("peer_root", K_PEER_ROOT)] {
            let i = super::super::slot_by_name(s).unwrap();
            assert_eq!(i.kind, k);
            assert!(super::super::record_info(k).is_some());
        }
        assert!(super::super::kind_by_name("local_root").is_none(), "the legacy kind is gone");
        let r = super::super::record_info(K_WEAPON).unwrap();
        assert_eq!(r.flow & super::super::flow::S2C, 0, "weapon is never relayed");
    }
}
