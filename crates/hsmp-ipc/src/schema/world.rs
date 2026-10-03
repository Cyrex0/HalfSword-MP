//! World object replication (kinds `0x04xx`, cap `WORLD`; server side: server/src/world.rs).
//!
//! One binary representation: HSMPWorld writes these records (quantised once,
//! at the source: smallest-three quaternions, i16 velocities) into its shared-memory blobs
//! or sends them through the G2S ring; the sidecar frames the bytes; the server validates
//! them in place and builds its fan-out from the same rows; the receiving sidecar keeps its
//! scope / ordering tables over the rows and publishes them, unchanged, for the game.
//!
//! Every record is scoped by `(level, epoch)`: `level` = FNV-1a of the UWorld path,
//! `epoch` = the server's world epoch (bumped whenever every arena is reloaded).

use super::{Chan, Dir, EnumInfo, KindInfo, RecordInfo, SlotForm, SlotInfo, CAP_WORLD};
use crate::layout::{Bool, Str};
use crate::record::Invalid;

/// Legacy codec kinds of this domain: none (every world contract is a record).
pub const KINDS: &[KindInfo] = &[];

// ---- object flags (`WorldObj::flags`) ----------------------------------------------------

pub const WF_ASLEEP: u8 = 1;
pub const WF_HELD: u8 = 2;
pub const WF_LEFT: u8 = 4;
pub const WF_SIM: u8 = 8;
/// Bits 6-7: index of the quaternion component the smallest-three encoding dropped.
pub const WF_QSHIFT: u8 = 6;
pub const WF_QMASK: u8 = 0xC0;
/// Flags a client may set (the quaternion index bits are part of the rotation).
pub const WF_USER: u8 = WF_ASLEEP | WF_HELD | WF_LEFT | WF_SIM;

// ---- claim / owner modes ------------------------------------------------------------------

pub const MODE_FREE: u8 = 0;
pub const MODE_TOUCH: u8 = 1;
pub const MODE_HOLD_R: u8 = 2;
pub const MODE_HOLD_L: u8 = 3;
/// Claim mode: an actor-state report (`rest.vel[0]` = the state bits).
pub const CLAIM_STATE: u8 = 4;
/// Claim mode: the sender's settled initial rest pose of a free body.
pub const CLAIM_INIT: u8 = 5;
/// Owner-record mode of an actor-state record: `MODE_STATE | bits` (bits 0..=0x7F).
pub const MODE_STATE: u8 = 0x80;

/// Hash-report row status bits.
pub const HS_ALIVE: u8 = 1;
pub const HS_SETTLED: u8 = 2;
pub const HS_STATE: u8 = 4;

/// Verdict mismatch kinds.
pub const MM_POSE: u8 = 1;
pub const MM_PRESENCE: u8 = 2;
pub const MM_STATE: u8 = 3;

/// Dynamic-item id namespace bit.
pub const DYN_ID_BIT: u32 = 0x8000_0000;

/// Row capacities (one reliable message carries the whole set: the transport fragments up
/// to 64 KiB; unreliable `world_state` stays within one datagram).
pub const STATE_MAX: usize = 32;
pub const OWNERS_MAX: usize = 2048;
pub const SNAPS_MAX: usize = 1024;
pub const MANIFEST_MAX: usize = 1536;
pub const DYN_MAX: usize = 256;
pub const HASH_MAX: usize = 2000;
pub const VERDICT_MAX: usize = 64;
pub const REMOTE_MAX: usize = 2560;
pub const HELD_MAX: usize = 32;
/// Manifest proposal rows per C2S message (the server handles at most this many per message;
/// the sidecar splits larger sets, one request id each).
pub const PROPOSE_MAX: usize = 256;
/// Dynamic item class path bytes.
pub const CLASS_BYTES: usize = 200;

crate::ipc_pod! {
    /// One replicated body at one sample (32 bytes), quantised by its owner's game.
    pub struct WorldObj {
        /// Network id (manifest id; high bit = dynamic item namespace).
        pub id: u32,
        /// World position, cm.
        pub pos: [f32; 3],
        /// Orientation, smallest-three: the three smaller components (the dropped, largest one
        /// made positive) scaled by 32767·√2; the dropped index is in `flags` bits 6-7.
        pub rot: [i16; 3],
        /// Linear velocity, cm/s (clamped to ±32767). A `CLAIM_STATE` claim: `vel[0]` = bits.
        pub vel: [i16; 3],
        /// `WF_*` bits | dropped quaternion index << 6.
        pub flags: u8,
        pub _r: [u8; 3],
    }

    /// `world_state` head: one sender's batch (C2S from the owner; S2C relayed per sender
    /// with `WireHdr::peer` = the sender). Rows: `WorldObj`.
    pub struct WorldStateHead {
        pub level: u32,
        pub epoch: u32,
        /// Sender batch counter (newest wins per sender).
        pub seq: u32,
        /// Sender game clock, ms (the interpolation timeline).
        pub ts: u32,
        pub n: u16,
        pub _r: u16,
        pub _r2: u32,
    }

    /// `world_claim` (C2S, reliable by resend): ownership request / release (with the final
    /// rest pose) / actor-state report / initial rest-pose proposal.
    pub struct WorldClaim {
        pub level: u32,
        pub epoch: u32,
        /// The game's request counter (diagnostics).
        pub req: u32,
        pub id: u32,
        /// `world_mode` code: 0 release, 1 touch, 2 hold R, 3 hold L, 4 state, 5 init.
        pub mode: u8,
        pub has_rest: Bool,
        pub _r: u16,
        pub _r2: u32,
        pub rest: WorldObj,
    }

    /// `world_sync` (C2S): level load / late join / reconnect / epoch change.
    pub struct WorldSync {
        pub level: u32,
        pub _r: u32,
    }

    /// One owner record. `owner` 0 = free; `mode` `world_mode` 0..=3, or `MODE_STATE | bits`.
    pub struct OwnerRec {
        pub id: u32,
        pub owner: u32,
        /// Higher wins per id.
        pub ver: u32,
        pub mode: u8,
        pub _r: [u8; 3],
    }

    /// `world_owners` head (S2C; also the sidecar's merged table for the game).
    pub struct OwnersHead {
        pub level: u32,
        pub epoch: u32,
        /// Server manifest length (a client with fewer entries re-syncs).
        pub manifest_len: u32,
        pub n: u16,
        /// The reply to a `world_sync` (the sidecar: synced).
        pub sync: Bool,
        pub _r: u8,
    }

    /// A cached transform. `sender` 0 = the server's rest-pose anchor of a free body.
    pub struct WorldSnap {
        pub sender: u32,
        pub seq: u32,
        pub ts: u32,
        pub _r: u32,
        pub obj: WorldObj,
    }

    /// `world_snapshot` head (S2C): cached transforms / anchors. Rows: `WorldSnap`.
    pub struct SnapHead {
        pub level: u32,
        pub epoch: u32,
        pub n: u16,
        pub _r: u16,
        pub _r2: u32,
    }

    /// One static manifest entry (a scene body found at level load).
    pub struct ManifestEntry {
        pub id: u32,
        /// FNV-1a of "<class short name>|<component name>".
        pub chash: u32,
        /// Spawn position, cm.
        pub pos: [f32; 3],
        pub _r: u32,
    }

    /// `world_manifest` head: C2S proposals (`req` != 0, acked by the reply with the same
    /// `req`), S2C canonical entries (`req` = the request answered, 0 = broadcast / sync).
    pub struct ManifestHead {
        pub level: u32,
        pub epoch: u32,
        pub req: u32,
        pub n: u16,
        pub _r: u16,
    }

    /// One dynamic manifest entry (an item that left its owner's hand).
    pub struct DynEntry {
        pub id: u32,
        pub chash: u32,
        /// Drop position, cm.
        pub pos: [f32; 3],
        /// The peer that dropped it.
        pub dyn_owner: u32,
        /// Class path to spawn ("/Game/...X.X_C").
        pub class_path: Str<200>,
    }

    /// `world_dyn` head: as `world_manifest`, for dynamic entries.
    pub struct DynHead {
        pub level: u32,
        pub epoch: u32,
        pub req: u32,
        pub n: u16,
        pub _r: u16,
    }

    /// One body (or actor state) of a consistency report.
    pub struct HashRow {
        pub id: u32,
        /// Owner / state record version the client knows (only equal versions compare).
        pub ver: u32,
        /// Position rounded to 0.1 cm.
        pub pos: [f32; 3],
        /// Packed 32-bit quaternion (bodies) or the state bits (`HS_STATE`).
        pub q: u32,
        /// `HS_*` bits.
        pub status: u8,
        pub _r: [u8; 3],
        pub _r2: u32,
    }

    /// `world_hash` head (C2S, once per (level, epoch, seq)): the client's consistency report.
    pub struct HashHead {
        pub level: u32,
        pub epoch: u32,
        pub seq: u32,
        /// The client's world hash over its settled / missing rows.
        pub hash: u32,
        pub n: u16,
        pub _r: u16,
        pub _r2: u32,
    }

    /// One mismatched body of a verdict.
    pub struct Mismatch {
        pub id: u32,
        /// `MM_*`.
        pub kind: u8,
        pub _r: [u8; 3],
        /// Position (cm) and orientation (deg) difference.
        pub dpos: f32,
        pub dang: f32,
    }

    /// `world_verdict` (S2C; the sidecar keeps the latest in `world_consistency`): the
    /// server's comparison of our report `seq` with peer `other`'s.
    pub struct VerdictHead {
        pub level: u32,
        pub epoch: u32,
        pub seq: u32,
        pub other: u32,
        pub compared: u32,
        /// Every mismatch (the rows carry at most `VERDICT_MAX`).
        pub mismatched_n: u32,
        pub hash_match: Bool,
        pub hash_equal: Bool,
        pub n: u16,
        pub _r: u32,
    }

    /// `world_remote` head (sidecar -> game): the newest sample per body plus the server's
    /// anchors (rows with `sender` 0). Rows: `WorldSnap`.
    pub struct RemoteHead {
        pub level: u32,
        pub epoch: u32,
        pub n: u32,
        pub _r: u32,
    }

    /// One world item a remote peer holds (game-local bus: HSMPWorld -> Avatars / Loadout).
    pub struct HeldRow {
        pub peer: u32,
        /// Network id of the world item.
        pub nid: u32,
        /// 0 = right hand, 1 = left hand.
        pub hand: u8,
        pub _r: [u8; 7],
        /// The world actor's FName in this game.
        pub actor: Str<64>,
    }

    /// `world_held` head (bus key `world_held`).
    pub struct HeldHead {
        pub n: u16,
        pub _r: u16,
        pub _r2: u32,
    }
}

pub const K_WORLD_STATE: u16 = 0x0410;
pub const K_WORLD_CLAIM: u16 = 0x0411;
pub const K_WORLD_SYNC: u16 = 0x0412;
pub const K_WORLD_OWNERS: u16 = 0x0413;
pub const K_WORLD_SNAPSHOT: u16 = 0x0414;
pub const K_WORLD_MANIFEST: u16 = 0x0415;
pub const K_WORLD_DYN: u16 = 0x0416;
pub const K_WORLD_HASH: u16 = 0x0417;
pub const K_WORLD_VERDICT: u16 = 0x0418;
pub const K_WORLD_REMOTE: u16 = 0x0419;
pub const K_WORLD_HELD: u16 = 0x041A;

/// World coordinate bound (cm): beyond this a position is garbage.
pub const WORLD_LIMIT: f32 = super::pose::WORLD_LIMIT;

fn pos_ok(p: &[f32; 3]) -> bool {
    p.iter().all(|c| c.abs() <= WORLD_LIMIT)
}

/// Generic row check of one object (also nested in claims and snapshots).
pub fn check_obj(o: &WorldObj) -> Result<(), Invalid> {
    if o.id == 0 {
        return Err(Invalid::Range("id"));
    }
    if !pos_ok(&o.pos) {
        return Err(Invalid::Range("pos"));
    }
    if o.rot.contains(&i16::MIN) {
        return Err(Invalid::Range("rot"));
    }
    if o.flags & !(WF_USER | WF_QMASK) != 0 {
        return Err(Invalid::Range("flags"));
    }
    Ok(())
}

fn check_state_row(_h: &WorldStateHead, o: &WorldObj) -> Result<(), Invalid> {
    check_obj(o)
}

fn check_claim(c: &WorldClaim) -> Result<(), Invalid> {
    if c.id == 0 {
        return Err(Invalid::Range("id"));
    }
    if c.mode > CLAIM_INIT {
        return Err(Invalid::Range("mode"));
    }
    if c.has_rest.get() {
        // The claimed id is authoritative; the rest pose's own id may be left 0.
        check_obj(&WorldObj { id: c.id, ..c.rest })?;
    }
    Ok(())
}

fn check_owner_row(_h: &OwnersHead, r: &OwnerRec) -> Result<(), Invalid> {
    if r.id == 0 {
        return Err(Invalid::Range("id"));
    }
    if r.mode > MODE_HOLD_L && r.mode < MODE_STATE {
        return Err(Invalid::Range("mode"));
    }
    Ok(())
}

fn check_snap_row(_h: &SnapHead, s: &WorldSnap) -> Result<(), Invalid> {
    check_obj(&s.obj)
}

fn check_remote_row(_h: &RemoteHead, s: &WorldSnap) -> Result<(), Invalid> {
    check_obj(&s.obj)
}

fn check_manifest_row(_h: &ManifestHead, e: &ManifestEntry) -> Result<(), Invalid> {
    if e.id == 0 || e.id & DYN_ID_BIT != 0 {
        return Err(Invalid::Range("id"));
    }
    if !pos_ok(&e.pos) {
        return Err(Invalid::Range("pos"));
    }
    Ok(())
}

fn check_dyn_row(_h: &DynHead, e: &DynEntry) -> Result<(), Invalid> {
    if e.id & DYN_ID_BIT == 0 {
        return Err(Invalid::Range("id"));
    }
    if e.dyn_owner == 0 {
        return Err(Invalid::Range("dyn_owner"));
    }
    if e.class_path.is_empty() {
        return Err(Invalid::Range("class_path"));
    }
    if !pos_ok(&e.pos) {
        return Err(Invalid::Range("pos"));
    }
    Ok(())
}

fn check_hash_row(_h: &HashHead, r: &HashRow) -> Result<(), Invalid> {
    if r.id == 0 {
        return Err(Invalid::Range("id"));
    }
    if r.status & !(HS_ALIVE | HS_SETTLED | HS_STATE) != 0 {
        return Err(Invalid::Range("status"));
    }
    if !pos_ok(&r.pos) {
        return Err(Invalid::Range("pos"));
    }
    Ok(())
}

fn check_verdict_row(_h: &VerdictHead, m: &Mismatch) -> Result<(), Invalid> {
    if m.id == 0 {
        return Err(Invalid::Range("id"));
    }
    if !(MM_POSE..=MM_STATE).contains(&m.kind) {
        return Err(Invalid::Range("kind"));
    }
    Ok(())
}

fn check_held_row(_h: &HeldHead, r: &HeldRow) -> Result<(), Invalid> {
    if r.hand > 1 {
        return Err(Invalid::Range("hand"));
    }
    Ok(())
}

crate::record!(WorldStateHead, kind = K_WORLD_STATE, name = "world_state", rows = WorldObj, count = n, max = STATE_MAX,
    check_row = check_state_row);
crate::record!(WorldClaim, kind = K_WORLD_CLAIM, name = "world_claim", check = check_claim);
crate::record!(WorldSync, kind = K_WORLD_SYNC, name = "world_sync");
crate::record!(OwnersHead, kind = K_WORLD_OWNERS, name = "world_owners", rows = OwnerRec, count = n, max = OWNERS_MAX,
    check_row = check_owner_row);
crate::record!(SnapHead, kind = K_WORLD_SNAPSHOT, name = "world_snapshot", rows = WorldSnap, count = n, max = SNAPS_MAX,
    check_row = check_snap_row);
crate::record!(ManifestHead, kind = K_WORLD_MANIFEST, name = "world_manifest", rows = ManifestEntry, count = n,
    max = MANIFEST_MAX, check_row = check_manifest_row);
crate::record!(DynHead, kind = K_WORLD_DYN, name = "world_dyn", rows = DynEntry, count = n, max = DYN_MAX,
    check_row = check_dyn_row);
crate::record!(HashHead, kind = K_WORLD_HASH, name = "world_hash", rows = HashRow, count = n, max = HASH_MAX,
    check_row = check_hash_row);
crate::record!(VerdictHead, kind = K_WORLD_VERDICT, name = "world_verdict", rows = Mismatch, count = n, max = VERDICT_MAX,
    check_row = check_verdict_row);
crate::record!(RemoteHead, kind = K_WORLD_REMOTE, name = "world_remote", rows = WorldSnap, count = n, max = REMOTE_MAX,
    check_row = check_remote_row);
crate::record!(HeldHead, kind = K_WORLD_HELD, name = "world_held", rows = HeldRow, count = n, max = HELD_MAX,
    check_row = check_held_row);

use super::flow::{C2S, G2S, LOCAL, S2C, S2G};

/// `hsmp_net::proto_v5::keys::WORLD` (the Latest stream of `world_state`).
const STREAM_WORLD: u8 = 5;

/// Record kinds of this domain (wire + shared memory).
pub const RECORDS: &[RecordInfo] = &[
    crate::record_info!(WorldStateHead, cap = CAP_WORLD, flow = C2S | S2C | G2S, chan = Chan::Latest(STREAM_WORLD),
        doc = "owner's transform batch (S2C: per sender, peer = sender, relevance / budget filtered)"),
    crate::record_info!(WorldClaim, cap = CAP_WORLD, flow = C2S | G2S, chan = Chan::Reliable,
        doc = "ownership claim / release with rest pose / actor state / initial pose"),
    crate::record_info!(WorldSync, cap = CAP_WORLD, flow = C2S | G2S, chan = Chan::Reliable,
        doc = "world sync request (level load, reconnect, epoch change)"),
    crate::record_info!(OwnersHead, cap = CAP_WORLD, flow = S2C | S2G, chan = Chan::Reliable,
        doc = "owner records (higher ver wins); the sidecar's merged table for the game"),
    crate::record_info!(SnapHead, cap = CAP_WORLD, flow = S2C, chan = Chan::Reliable,
        doc = "cached transforms / rest-pose anchors (sender 0)"),
    crate::record_info!(ManifestHead, cap = CAP_WORLD, flow = C2S | S2C | G2S | S2G, chan = Chan::Ordered,
        doc = "static manifest entries: proposals (req) / canonical (reply to req, or 0)"),
    crate::record_info!(DynHead, cap = CAP_WORLD, flow = C2S | S2C | G2S | S2G, chan = Chan::Ordered,
        doc = "dynamic manifest entries (dropped items, with their class path)"),
    crate::record_info!(HashHead, cap = CAP_WORLD, flow = C2S | G2S, chan = Chan::Ordered,
        doc = "consistency report (once per level, epoch, seq)"),
    crate::record_info!(VerdictHead, cap = CAP_WORLD, flow = S2C | S2G, chan = Chan::Ordered,
        doc = "consistency verdict on one of our reports"),
    crate::record_info!(RemoteHead, cap = CAP_WORLD, flow = S2G, chan = Chan::None,
        doc = "newest sample per body + anchors (sidecar -> game)"),
    crate::record_info!(HeldHead, cap = CAP_WORLD, flow = LOCAL, chan = Chan::None,
        doc = "world items remote peers hold (bus: HSMPWorld -> Avatars, Loadout)"),
];

/// Named record slots of this domain.
pub const SLOTS: &[SlotInfo] = &[
    SlotInfo { name: "world_out", kind: K_WORLD_STATE, form: SlotForm::Blob, dir: Dir::GameToSidecar, cap: CAP_WORLD,
        world_scoped: true, doc: "owned bodies' states (sent once per seq)" },
    SlotInfo { name: "world_manifest_out", kind: K_WORLD_MANIFEST, form: SlotForm::Blob, dir: Dir::GameToSidecar,
        cap: CAP_WORLD, world_scoped: true, doc: "discovered static bodies to propose" },
    SlotInfo { name: "world_dyn_out", kind: K_WORLD_DYN, form: SlotForm::Blob, dir: Dir::GameToSidecar, cap: CAP_WORLD,
        world_scoped: true, doc: "my dropped items to propose" },
    SlotInfo { name: "world_hash", kind: K_WORLD_HASH, form: SlotForm::Blob, dir: Dir::GameToSidecar, cap: CAP_WORLD,
        world_scoped: true, doc: "consistency report (sent once per seq)" },
    SlotInfo { name: "world_remote", kind: K_WORLD_REMOTE, form: SlotForm::Blob, dir: Dir::SidecarToGame,
        cap: CAP_WORLD, world_scoped: false, doc: "newest sample per body + anchors" },
    SlotInfo { name: "world_owners", kind: K_WORLD_OWNERS, form: SlotForm::Blob, dir: Dir::SidecarToGame,
        cap: CAP_WORLD, world_scoped: false, doc: "merged owner records; sync = synced" },
    SlotInfo { name: "world_manifest", kind: K_WORLD_MANIFEST, form: SlotForm::Blob, dir: Dir::SidecarToGame,
        cap: CAP_WORLD, world_scoped: false, doc: "canonical static manifest (sorted by id)" },
    SlotInfo { name: "world_dyn", kind: K_WORLD_DYN, form: SlotForm::Blob, dir: Dir::SidecarToGame, cap: CAP_WORLD,
        world_scoped: false, doc: "canonical dynamic manifest (sorted by id)" },
    SlotInfo { name: "world_consistency", kind: K_WORLD_VERDICT, form: SlotForm::Slot, dir: Dir::SidecarToGame,
        cap: CAP_WORLD, world_scoped: false, doc: "the latest verdict on our reports" },
    SlotInfo { name: "world_held", kind: K_WORLD_HELD, form: SlotForm::Bus, dir: Dir::Local, cap: CAP_WORLD,
        world_scoped: true, doc: "remote peers' held world items" },
];

/// Code tables of this domain (Lua: `S.ENUMS.<name>.<VALUE>`, C: `HSMP_<NAME>_<VALUE>`).
pub const ENUMS: &[EnumInfo] = &[
    EnumInfo {
        name: "world_mode",
        values: &[("FREE", 0), ("TOUCH", 1), ("HOLD_R", 2), ("HOLD_L", 3), ("STATE", 4), ("INIT", 5)],
    },
    EnumInfo {
        name: "world_flag",
        values: &[("ASLEEP", WF_ASLEEP as u32), ("HELD", WF_HELD as u32), ("LEFT", WF_LEFT as u32), ("SIM", WF_SIM as u32)],
    },
    EnumInfo { name: "world_mm", values: &[("POSE", 1), ("PRESENCE", 2), ("STATE", 3)] },
];

// ---- quantisation (the canonical form; HSMPWorld's Lua writes it bit-identically) ---------

const QSCALE: f32 = 32767.0 * core::f32::consts::SQRT_2;

/// UE `FRotator::Quaternion` (degrees in, [x, y, z, w] out).
pub fn rot_to_quat(pitch: f32, yaw: f32, roll: f32) -> [f32; 4] {
    let h = std::f64::consts::PI / 360.0;
    let (sp, cp) = (pitch as f64 * h).sin_cos();
    let (sy, cy) = (yaw as f64 * h).sin_cos();
    let (sr, cr) = (roll as f64 * h).sin_cos();
    [
        (cr * sp * sy - sr * cp * cy) as f32,
        (-cr * sp * cy - sr * cp * sy) as f32,
        (cr * cp * sy - sr * sp * cy) as f32,
        (cr * cp * cy + sr * sp * sy) as f32,
    ]
}

/// Smallest-three: the three kept components (i16) and the dropped index.
pub fn pack_quat(q: [f32; 4]) -> ([i16; 3], u8) {
    let n = (q.iter().map(|c| c * c).sum::<f32>()).sqrt();
    let q = if n > 1e-6 && n.is_finite() { q.map(|c| c / n) } else { [0.0, 0.0, 0.0, 1.0] };
    let mut big = 0usize;
    for i in 1..4 {
        if q[i].abs() > q[big].abs() {
            big = i;
        }
    }
    let s = if q[big] < 0.0 { -1.0 } else { 1.0 };
    let mut out = [0i16; 3];
    let mut k = 0;
    for (i, c) in q.iter().enumerate() {
        if i == big {
            continue;
        }
        // Lua: math.floor(x + 0.5) (round half up), the same in both.
        out[k] = ((c * s * QSCALE) + 0.5).floor().clamp(-32767.0, 32767.0) as i16;
        k += 1;
    }
    (out, big as u8)
}

pub fn unpack_quat(c: [i16; 3], big: u8) -> [f32; 4] {
    let big = (big & 3) as usize;
    let v = c.map(|x| x as f32 / QSCALE);
    let rest = (1.0 - v.iter().map(|x| x * x).sum::<f32>()).max(0.0).sqrt();
    let mut q = [0f32; 4];
    let mut k = 0;
    for (i, slot) in q.iter_mut().enumerate() {
        if i == big {
            *slot = rest;
        } else {
            *slot = v[k];
            k += 1;
        }
    }
    q
}

/// cm/s -> i16 (round half up, clamped; non-finite -> 0).
pub fn quant_vel(v: f32) -> i16 {
    if v.is_finite() {
        (v + 0.5).floor().clamp(-32767.0, 32767.0) as i16
    } else {
        0
    }
}

impl WorldObj {
    /// Quantise (rotator in degrees, velocity cm/s): what HSMPWorld writes (tests, tools).
    pub fn from_parts(id: u32, pos: [f32; 3], rot_deg: [f32; 3], vel: [f32; 3], flags: u8) -> WorldObj {
        let (c, big) = pack_quat(rot_to_quat(rot_deg[0], rot_deg[1], rot_deg[2]));
        let pos = pos.map(|p| if p.is_finite() { p.clamp(-WORLD_LIMIT, WORLD_LIMIT) } else { 0.0 });
        WorldObj { id, pos, rot: c, vel: vel.map(quant_vel), flags: (flags & WF_USER) | (big << WF_QSHIFT), _r: [0; 3] }
    }
    pub fn quat(&self) -> [f32; 4] {
        unpack_quat(self.rot, self.flags >> WF_QSHIFT)
    }
    pub fn user_flags(&self) -> u8 {
        self.flags & WF_USER
    }
    pub fn has(&self, f: u8) -> bool {
        self.flags & f != 0
    }
}

impl WorldSnap {
    pub fn new(sender: u32, seq: u32, ts: u32, obj: WorldObj) -> WorldSnap {
        WorldSnap { sender, seq, ts, _r: 0, obj }
    }
}

impl OwnerRec {
    pub fn new(id: u32, owner: u32, ver: u32, mode: u8) -> OwnerRec {
        OwnerRec { id, owner, ver, mode, _r: [0; 3] }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::record::{to_payload, view, VarBuf};

    fn obj(id: u32) -> WorldObj {
        WorldObj::from_parts(id, [100.0, -50.5, 20.0], [10.0, 20.0, 30.0], [5.0, -6.0, 1e9], WF_HELD)
    }

    #[test]
    fn sizes_are_the_contract() {
        use core::mem::size_of;
        assert_eq!(size_of::<WorldObj>(), 32);
        assert_eq!(size_of::<WorldStateHead>(), 24);
        assert_eq!(size_of::<WorldClaim>(), 56);
        assert_eq!(size_of::<OwnerRec>(), 16);
        assert_eq!(size_of::<WorldSnap>(), 48);
        assert_eq!(size_of::<ManifestEntry>(), 24);
        assert_eq!(size_of::<DynEntry>(), 224);
        assert_eq!(size_of::<HashRow>(), 32);
        assert_eq!(size_of::<Mismatch>(), 16);
        assert_eq!(size_of::<VerdictHead>(), 32);
        assert_eq!(size_of::<HeldRow>(), 80);
        // One world_state message (8 B wire header + head + rows) fits one datagram
        // (hsmp-net MAX_UNRELIABLE = 1155); a full reliable set fits one 64 KiB message.
        assert!(8 + 24 + STATE_MAX * 32 <= 1155);
        for (head, row, max) in [(16, 16, OWNERS_MAX), (16, 48, SNAPS_MAX), (16, 24, MANIFEST_MAX), (16, 224, DYN_MAX),
                                 (24, 32, HASH_MAX), (32, 16, VERDICT_MAX)] {
            assert!(8 + head + row * max <= 64 * 1024, "{head} + {row} x {max}");
        }
        // The bus key fits a bus value.
        assert!(8 + 80 * HELD_MAX <= super::super::bus::BUS_VALUE_BYTES);
    }

    #[test]
    fn state_round_trip_and_row_checks() {
        let h = WorldStateHead { level: 7, epoch: 3, seq: 9, ts: 1234, n: 0, _r: 0, _r2: 0 };
        let rows = [obj(1), obj(2)];
        let p = to_payload(&h, &rows);
        assert_eq!(p.len(), 24 + 64);
        let v = view::<WorldStateHead>(&p).unwrap();
        assert_eq!((v.head.seq, v.rows.len(), v.rows[1].id), (9, 2, 2));
        let mut bad = rows;
        bad[1].id = 0;
        assert_eq!(view::<WorldStateHead>(&to_payload(&h, &bad)).unwrap_err(), Invalid::Range("id"));
        bad = rows;
        bad[0].flags |= 0x10;
        assert_eq!(view::<WorldStateHead>(&to_payload(&h, &bad)).unwrap_err(), Invalid::Range("flags"));
        bad = rows;
        bad[0].rot[2] = i16::MIN;
        assert_eq!(view::<WorldStateHead>(&to_payload(&h, &bad)).unwrap_err(), Invalid::Range("rot"));
        bad = rows;
        bad[0].pos[0] = 2.0e7;
        assert_eq!(view::<WorldStateHead>(&to_payload(&h, &bad)).unwrap_err(), Invalid::Range("pos"));
        bad = rows;
        bad[0].pos[1] = f32::NAN;
        assert_eq!(view::<WorldStateHead>(&to_payload(&h, &bad)).unwrap_err(), Invalid::Float("pos"));
        let many = vec![obj(5); STATE_MAX + 1];
        let mut hp = bytemuck::bytes_of(&WorldStateHead { n: (STATE_MAX + 1) as u16, ..h }).to_vec();
        hp.extend_from_slice(bytemuck::cast_slice(&many));
        assert!(matches!(view::<WorldStateHead>(&hp), Err(Invalid::Rows { .. })));
    }

    #[test]
    fn claim_owner_manifest_dyn_hash_verdict_held_checks() {
        let c = WorldClaim { level: 1, epoch: 2, req: 3, id: 9, mode: MODE_FREE, has_rest: Bool::TRUE, _r: 0, _r2: 0,
                             rest: WorldObj { id: 0, ..obj(9) } };
        assert!(view::<WorldClaim>(&to_payload(&c, &[])).is_ok(), "rest id 0 is fine (the claim id rules)");
        assert_eq!(view::<WorldClaim>(&to_payload(&WorldClaim { mode: 6, ..c }, &[])).unwrap_err(), Invalid::Range("mode"));
        assert_eq!(view::<WorldClaim>(&to_payload(&WorldClaim { id: 0, ..c }, &[])).unwrap_err(), Invalid::Range("id"));
        let mut b = to_payload(&c, &[]);
        b[17] = 2; // has_rest
        assert_eq!(view::<WorldClaim>(&b).unwrap_err(), Invalid::Bool("has_rest"));
        let mut r = c;
        r.rest.flags = 0x20;
        assert!(view::<WorldClaim>(&to_payload(&r, &[])).is_err());
        r.has_rest = Bool::FALSE;
        assert!(view::<WorldClaim>(&to_payload(&r, &[])).is_ok(), "no rest: the rest bytes are not judged");

        let oh = OwnersHead { level: 1, epoch: 1, manifest_len: 4, n: 0, sync: Bool::TRUE, _r: 0 };
        assert!(view::<OwnersHead>(&to_payload(&oh, &[OwnerRec::new(1, 2, 3, MODE_HOLD_L), OwnerRec::new(2, 0, 1, MODE_STATE | 0x41)])).is_ok());
        assert_eq!(view::<OwnersHead>(&to_payload(&oh, &[OwnerRec::new(1, 2, 3, 9)])).unwrap_err(), Invalid::Range("mode"));

        let mh = ManifestHead { level: 1, epoch: 1, req: 5, n: 0, _r: 0 };
        let e = ManifestEntry { id: 10, chash: 7, pos: [1.0, 2.0, 3.0], _r: 0 };
        assert!(view::<ManifestHead>(&to_payload(&mh, &[e])).is_ok());
        assert!(view::<ManifestHead>(&to_payload(&mh, &[ManifestEntry { id: DYN_ID_BIT | 1, ..e }])).is_err(), "dynamic ids go in world_dyn");

        let dh = DynHead { level: 1, epoch: 1, req: 0, n: 0, _r: 0 };
        let d = DynEntry { id: DYN_ID_BIT | (2 << 16) | 1, chash: 1, pos: [5.0; 3], dyn_owner: 2, class_path: Str::new("/Game/A.A_C") };
        let dp = to_payload(&dh, &[d]);
        assert_eq!(view::<DynHead>(&dp).unwrap().rows[0].class_path, "/Game/A.A_C");
        assert_eq!(view::<DynHead>(&to_payload(&dh, &[DynEntry { class_path: Str::new(""), ..d }])).unwrap_err(), Invalid::Range("class_path"));
        assert_eq!(view::<DynHead>(&to_payload(&dh, &[DynEntry { dyn_owner: 0, ..d }])).unwrap_err(), Invalid::Range("dyn_owner"));
        let mut s = to_payload(&dh, &[d]);
        s[16 + 24 + 150] = b'x'; // a byte after the NUL padding of `class_path`
        assert_eq!(view::<DynHead>(&s).unwrap_err(), Invalid::Str("class_path"));

        let hh = HashHead { level: 1, epoch: 1, seq: 4, hash: 99, n: 0, _r: 0, _r2: 0 };
        let row = HashRow { id: 3, ver: 1 << 30, pos: [1.5, 0.0, 0.0], q: 0xC000_0000, status: HS_ALIVE | HS_SETTLED, _r: [0; 3], _r2: 0 };
        assert_eq!(view::<HashHead>(&to_payload(&hh, &[row])).unwrap().rows[0].ver, 1 << 30, "full 32-bit versions");
        assert_eq!(view::<HashHead>(&to_payload(&hh, &[HashRow { status: 8, ..row }])).unwrap_err(), Invalid::Range("status"));

        let vh = VerdictHead { level: 1, epoch: 1, seq: 4, other: 2, compared: 10, mismatched_n: 1, hash_match: Bool::FALSE,
                               hash_equal: Bool::FALSE, n: 0, _r: 0 };
        let m = Mismatch { id: 77, kind: MM_POSE, _r: [0; 3], dpos: 12.5, dang: 4.0 };
        assert!(view::<VerdictHead>(&to_payload(&vh, &[m])).is_ok());
        assert_eq!(view::<VerdictHead>(&to_payload(&vh, &[Mismatch { kind: 0, ..m }])).unwrap_err(), Invalid::Range("kind"));

        let held = HeldHead { n: 0, _r: 0, _r2: 0 };
        let hr = HeldRow { peer: 2, nid: 123, hand: 1, _r: [0; 7], actor: Str::new("Sword_9") };
        assert!(view::<HeldHead>(&to_payload(&held, &[hr])).is_ok());
        assert_eq!(view::<HeldHead>(&to_payload(&held, &[HeldRow { hand: 2, ..hr }])).unwrap_err(), Invalid::Range("hand"));
    }

    #[test]
    fn varbuf_blobs_hold_full_sets() {
        let mut b = VarBuf::<OwnersHead, OWNERS_MAX>::new_boxed();
        let recs: Vec<OwnerRec> = (1..=OWNERS_MAX as u32).map(|i| OwnerRec::new(i, 0, i, 0)).collect();
        b.set(&OwnersHead { level: 1, epoch: 1, manifest_len: 9, n: 0, sync: Bool::TRUE, _r: 0 }, &recs);
        assert_eq!(b.payload().len(), 16 + 16 * OWNERS_MAX);
        assert!(view::<OwnersHead>(b.payload()).is_ok());
    }

    /// Golden vectors of the canonical quantisation: tools/hsmp-tools/lua-tests/hsmpworld.lua
    /// checks HSMPWorld's Lua writer (W2.qobj) against the same numbers.
    #[test]
    fn quantisation_golden_vectors() {
        let cases: [([f32; 3], [f32; 3], u8, [i16; 3], [i16; 3], u8); 4] = [
            ([0.0, 0.0, 0.0], [0.0, 0.0, 0.0], 0, [0, 0, 0], [0, 0, 0], 3 << 6),
            ([0.0, 90.0, 0.0], [100.4, -0.5, 0.6], WF_SIM, [0, 0, 32767], [100, 0, 1], WF_SIM | (2 << 6)),
            ([10.0, 20.0, 30.0], [5.0, -6.0, 1e9], WF_HELD, [-11089, -5917, 6714], [5, -6, 32767], WF_HELD | (3 << 6)),
            ([-45.0, 170.0, -120.0], [0.0; 3], WF_ASLEEP | 0x30, [-5602, 19986, 17165], [0; 3], WF_ASLEEP | (1 << 6)),
        ];
        let mut bad = Vec::new();
        for (rot, vel, flags, want_rot, want_vel, want_flags) in cases {
            let o = WorldObj::from_parts(1, [0.0; 3], rot, vel, flags);
            if (o.rot, o.vel, o.flags) != (want_rot, want_vel, want_flags) {
                bad.push(format!("rot {rot:?}: {:?} {:?} {}", o.rot, o.vel, o.flags));
            }
            assert!(check_obj(&o).is_ok());
        }
        assert!(bad.is_empty(), "{bad:#?}");
    }

    #[test]
    fn quat_round_trip_is_tight() {
        let mut worst = 0.0f64;
        for p in (-89..=89).step_by(7) {
            for y in (-180..=180).step_by(11) {
                for r in (-180..=180).step_by(13) {
                    let q = rot_to_quat(p as f32, y as f32, r as f32);
                    let (c, i) = pack_quat(q);
                    let u = unpack_quat(c, i);
                    let chord = |s: f64| (0..4).map(|k| (q[k] as f64 - s * u[k] as f64).powi(2)).sum::<f64>().sqrt();
                    let ch = chord(1.0).min(chord(-1.0)).min(2.0);
                    worst = worst.max(4.0 * (ch / 2.0).asin().to_degrees());
                }
            }
        }
        assert!(worst < 0.01, "worst error {worst} deg");
        let bad = WorldObj::from_parts(1, [f32::NAN, f32::INFINITY, 0.0], [f32::NAN, 0.0, 0.0], [f32::NAN; 3], 0);
        assert!(bad.pos.iter().all(|p| p.is_finite()) && bad.vel == [0, 0, 0]);
        let n: f32 = bad.quat().iter().map(|c| c * c).sum();
        assert!((n - 1.0).abs() < 1e-3);
    }
}
