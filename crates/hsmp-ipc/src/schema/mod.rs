//! The one schema. One file per domain, each with a pre-allocated kind-id range and capability
//! bit. Changing anything here changes [`crate::LAYOUT_HASH`]; regenerate the C header and the
//! Lua schema with `hsmp-tools gen-ipc` (the `ipc-schema` gate check diffs them).
//!
//! Kind ids are u16: high byte = domain, low byte = role:
//! - `0x00..=0x3F` game-written slot or blob
//! - `0x40..=0x7F` sidecar-written slot or blob (per-peer ones included)
//! - `0x80..=0xBF` G2S ring message (game -> sidecar)
//! - `0xC0..=0xFF` S2G ring message (sidecar -> game)
//!
//! Everything is a typed `#[repr(C)]` struct: the records (ABI 2 / protocol v6:
//! [`RecordInfo`], [`SlotInfo`]) in slots, blobs, ring messages, bus keys and on the wire,
//! and three POD slots of the pose domain (`pose_lead`, `peer_dir`, `peer_play`; [`KindInfo`]).

pub mod bus;
pub mod combat;
pub mod dev;
pub mod interact;
pub mod loadout;
pub mod mods;
pub mod pose;
pub mod session;
pub mod world;

use crate::layout::{FieldDesc, IpcType, Pod, TypeDesc};

crate::ipc_pod! {
    /// Leads every slot and blob payload.
    pub struct SlotMeta {
        /// The writing side's epoch; readers drop data from a previous instance.
        pub writer_epoch: u64,
        /// `Header::world_epoch` when written (game-written data).
        pub world_epoch: u32,
        /// `Header::session_epoch` when written (sidecar-written data).
        pub session_epoch: u32,
        /// 0 = invalid / cleared (e.g. republished at world leave), 1 = valid.
        pub valid: u32,
        /// Writer's own sample counter.
        pub sample_seq: u32,
        /// Writer's monotonic clock, µs.
        pub t_us: u64,
    }
}

// ---- records (ABI 2 / protocol v6) ---------------------------------------------------------

/// Where a record kind may appear (bit set).
pub mod flow {
    /// Client (sidecar) -> server.
    pub const C2S: u8 = 1 << 0;
    /// Server -> client (sidecar).
    pub const S2C: u8 = 1 << 1;
    /// Game -> sidecar (G2S ring or a game-written slot / blob).
    pub const G2S: u8 = 1 << 2;
    /// Sidecar -> game (S2G ring or a sidecar-written slot / blob).
    pub const S2G: u8 = 1 << 3;
    /// Game-local (bus) or tool -> game (DevCtl).
    pub const LOCAL: u8 = 1 << 4;
    pub const NAMES: [(&str, u8); 5] = [("c2s", C2S), ("s2c", S2C), ("g2s", G2S), ("s2g", S2G), ("local", LOCAL)];
}

/// The hsmp-net channel of a network record kind. The server and sidecar map it onto
/// `hsmp_net::net::SendMode` (`Latest` / `RelLatest` keyed by `(stream, source peer)`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Chan {
    /// Not a network record.
    None,
    /// Unreliable, newest wins per (stream, peer).
    Latest(u8),
    /// Reliable, a newer one supersedes an unacked older one per (stream, peer).
    RelLatest(u8),
    /// Reliable, unordered.
    Reliable,
    /// Reliable, ordered.
    Ordered,
}

impl Chan {
    pub fn as_str(&self) -> &'static str {
        match self {
            Chan::None => "none",
            Chan::Latest(_) => "latest",
            Chan::RelLatest(_) => "rel_latest",
            Chan::Reliable => "reliable",
            Chan::Ordered => "ordered",
        }
    }
}

/// One record kind: its layout, where it travels and its validator. `layout` names the
/// record struct; several kinds may share one layout (e.g. `clash` and `touch`).
#[derive(Clone, Copy)]
pub struct RecordInfo {
    pub kind: u16,
    pub name: &'static str,
    /// The record (head) struct's name.
    pub layout: &'static str,
    pub head: &'static TypeDesc,
    pub row: Option<&'static TypeDesc>,
    pub count_field: Option<&'static str>,
    pub max_rows: usize,
    /// Shared-memory capability bit (0 = always negotiated).
    pub cap: u64,
    /// `flow::*` bits.
    pub flow: u8,
    pub chan: Chan,
    /// Validate an untrusted payload of this kind (the full [`crate::record::view`]).
    pub check: fn(&[u8]) -> Result<(), crate::record::Invalid>,
    pub doc: &'static str,
}

impl core::fmt::Debug for RecordInfo {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("RecordInfo").field("kind", &self.kind).field("name", &self.name).field("layout", &self.layout).finish()
    }
}

/// A [`RecordInfo`] for the record type `$t` (its own kind and name), or for an alias kind
/// that reuses `$t`'s layout.
#[macro_export]
macro_rules! record_info {
    ($t:ty, cap = $cap:expr, flow = $flow:expr, chan = $chan:expr, doc = $doc:expr $(,)?) => {
        $crate::record_info!(@ $t, <$t as $crate::record::Record>::KIND, <$t as $crate::record::Record>::NAME, $cap, $flow, $chan, $doc)
    };
    (alias $kind:expr, $name:expr, $t:ty, cap = $cap:expr, flow = $flow:expr, chan = $chan:expr, doc = $doc:expr $(,)?) => {
        $crate::record_info!(@ $t, $kind, $name, $cap, $flow, $chan, $doc)
    };
    (@ $t:ty, $kind:expr, $name:expr, $cap:expr, $flow:expr, $chan:expr, $doc:expr) => {
        $crate::schema::RecordInfo {
            kind: $kind,
            name: $name,
            layout: <$t as $crate::layout::IpcType>::DESC.name(),
            head: &<$t as $crate::layout::IpcType>::DESC,
            row: if <$t as $crate::record::Record>::MAX_ROWS == 0 { None } else {
                Some(&<<$t as $crate::record::Record>::Row as $crate::layout::IpcType>::DESC)
            },
            count_field: <$t as $crate::record::Record>::COUNT_FIELD,
            max_rows: <$t as $crate::record::Record>::MAX_ROWS,
            cap: $cap,
            flow: $flow,
            chan: $chan,
            check: |p| $crate::record::view::<$t>(p).map(|_| ()),
            doc: $doc,
        }
    };
}

/// Every record kind of every domain (ABI 2). Domain files contribute `RECORDS`.
pub fn records() -> impl Iterator<Item = &'static RecordInfo> {
    pose::RECORDS.iter()
        .chain(session::RECORDS)
        .chain(combat::RECORDS)
        .chain(world::RECORDS)
        .chain(loadout::RECORDS)
        .chain(interact::RECORDS)
        .chain(bus::RECORDS)
        .chain(dev::RECORDS)
        .chain(mods::RECORDS)
}

pub fn record_info(kind: u16) -> Option<&'static RecordInfo> {
    records().find(|r| r.kind == kind)
}

pub fn record_by_name(name: &str) -> Option<&'static RecordInfo> {
    records().find(|r| r.name == name)
}

/// Validate an untrusted payload of `kind` (unknown kinds are refused).
pub fn check_payload(kind: u16, payload: &[u8]) -> Result<(), crate::record::Invalid> {
    match record_info(kind) {
        Some(r) => (r.check)(payload),
        None => Err(crate::record::Invalid::Kind(kind)),
    }
}

/// A slot / blob payload in shared memory: the meta, the payload length and kind, then the
/// record body (a fixed record or a [`crate::record::VarBuf`]). The first `len` bytes of
/// `body` are the record payload (= the wire payload).
#[repr(C)]
#[derive(Clone, Copy)]
pub struct Stamped<T: Pod + bytemuck::Pod> {
    pub meta: SlotMeta,
    /// Payload bytes in `body` (0 = no value).
    pub len: u32,
    /// The record kind in `body`.
    pub kind: u32,
    pub body: T,
}

// SAFETY: repr(C), Pod fields, SlotMeta is 32 bytes and T's size is a multiple of 8 (no padding).
unsafe impl<T: Pod + bytemuck::Pod> Pod for Stamped<T> {}
unsafe impl<T: Pod + bytemuck::Pod> bytemuck::Zeroable for Stamped<T> {}
unsafe impl<T: Pod + bytemuck::Pod> bytemuck::Pod for Stamped<T> {}

impl<T: Pod + bytemuck::Pod> Default for Stamped<T> {
    fn default() -> Self {
        <Self as Pod>::zeroed()
    }
}

unsafe impl<T: Pod + bytemuck::Pod> IpcType for Stamped<T> {
    const DESC: TypeDesc = TypeDesc::Struct {
        name: "Stamped",
        size: core::mem::size_of::<Self>(),
        align: core::mem::align_of::<Self>(),
        pod: true,
        fields: &[
            FieldDesc { name: "meta", offset: core::mem::offset_of!(Self, meta), ty: &<SlotMeta as IpcType>::DESC },
            FieldDesc { name: "len", offset: core::mem::offset_of!(Self, len), ty: &<u32 as IpcType>::DESC },
            FieldDesc { name: "kind", offset: core::mem::offset_of!(Self, kind), ty: &<u32 as IpcType>::DESC },
            FieldDesc { name: "body", offset: core::mem::offset_of!(Self, body), ty: &T::DESC },
        ],
    };
}

impl<T: Pod + bytemuck::Pod> Stamped<T> {
    /// Body capacity in bytes.
    pub const CAP: usize = {
        assert!(core::mem::size_of::<T>().is_multiple_of(8));
        core::mem::size_of::<T>()
    };
    /// A zeroed value on the heap (bodies can be large).
    pub fn new_boxed() -> Box<Self> {
        crate::layout::boxed_zeroed::<Self>()
    }
    /// The payload bytes (length clamped to the capacity).
    pub fn payload(&self) -> &[u8] {
        let n = (self.len as usize).min(Self::CAP);
        &bytemuck::bytes_of(&self.body)[..n]
    }
    /// The payload bytes (legacy name).
    pub fn data(&self) -> &[u8] {
        self.payload()
    }
    /// Copy `data` in (legacy name of [`Stamped::set_payload`]).
    pub fn set(&mut self, kind: u16, data: &[u8]) -> bool {
        self.set_payload(kind, data)
    }
    /// Copy `payload` into the body. False (unchanged) if it does not fit.
    pub fn set_payload(&mut self, kind: u16, payload: &[u8]) -> bool {
        if payload.len() > Self::CAP {
            return false;
        }
        bytemuck::bytes_of_mut(&mut self.body)[..payload.len()].copy_from_slice(payload);
        self.len = payload.len() as u32;
        self.kind = kind as u32;
        true
    }
}

/// A shared-memory slot or blob seen as bytes: the generic machinery the native module, the
/// sidecar and the tools use for every record slot (no per-slot code).
pub trait RawSlot: Sync {
    /// Body capacity in bytes.
    fn cap(&self) -> usize;
    /// Size of the slot's `Stamped<T>` (the scratch it needs, in bytes).
    fn stamped_size(&self) -> usize;
    /// Change counter: the seqlock `seq` (slots) or the published generation (blobs).
    fn version(&self) -> u64;
    /// Publish `payload` of `kind` with `meta` (single writer). `scratch` is reused (resized
    /// as needed). False if the payload does not fit.
    fn put(&self, meta: SlotMeta, kind: u16, payload: &[u8], scratch: &mut Vec<u64>) -> bool;
    /// Read the newest value (slots: a consistent copy; blobs: take the newest publish, the
    /// single reader). Returns the meta, the kind, the version, and the payload in `out`.
    /// `None`: never written / nothing new for a blob / busy.
    fn get(&self, scratch: &mut Vec<u64>, out: &mut Vec<u8>) -> Option<(SlotMeta, u16, u64)>;
    /// Like [`RawSlot::get`] but never writes the segment (tools on a read-only mapping:
    /// `ipc-dump`). Blobs: the buffer the reader currently holds, or `None` before its first
    /// take (best effort; it may be torn while the reader swaps).
    fn peek(&self, scratch: &mut Vec<u64>, out: &mut Vec<u8>) -> Option<(SlotMeta, u16, u64)>;
}

/// `scratch` viewed as a `T` (grown as needed; a Vec<u64> buffer is 8-aligned).
pub fn scratch_for<T: bytemuck::Pod>(scratch: &mut Vec<u64>) -> &mut T {
    let size = core::mem::size_of::<T>();
    let words = size.div_ceil(8);
    if scratch.len() < words {
        scratch.resize(words, 0);
    }
    bytemuck::from_bytes_mut::<T>(&mut bytemuck::cast_slice_mut::<u64, u8>(&mut scratch[..words])[..size])
}

impl<T: Pod + bytemuck::Pod> RawSlot for crate::seqlock::SeqSlot<Stamped<T>> {
    fn cap(&self) -> usize {
        Stamped::<T>::CAP
    }
    fn stamped_size(&self) -> usize {
        core::mem::size_of::<Stamped<T>>()
    }
    fn version(&self) -> u64 {
        self.seq()
    }
    fn put(&self, meta: SlotMeta, kind: u16, payload: &[u8], scratch: &mut Vec<u64>) -> bool {
        let s = scratch_for::<Stamped<T>>(scratch);
        s.meta = meta;
        if !s.set_payload(kind, payload) {
            return false;
        }
        self.write(s);
        true
    }
    fn get(&self, scratch: &mut Vec<u64>, out: &mut Vec<u8>) -> Option<(SlotMeta, u16, u64)> {
        let s = scratch_for::<Stamped<T>>(scratch);
        let seq = self.read_into(s).ok()?;
        out.clear();
        out.extend_from_slice(s.payload());
        Some((s.meta, s.kind as u16, seq))
    }
    fn peek(&self, scratch: &mut Vec<u64>, out: &mut Vec<u8>) -> Option<(SlotMeta, u16, u64)> {
        self.get(scratch, out)
    }
}

impl<T: Pod + bytemuck::Pod> RawSlot for crate::triple::TripleBuf<Stamped<T>> {
    fn cap(&self) -> usize {
        Stamped::<T>::CAP
    }
    fn stamped_size(&self) -> usize {
        core::mem::size_of::<Stamped<T>>()
    }
    fn version(&self) -> u64 {
        self.published_gen() as u64
    }
    fn put(&self, meta: SlotMeta, kind: u16, payload: &[u8], scratch: &mut Vec<u64>) -> bool {
        let s = scratch_for::<Stamped<T>>(scratch);
        s.meta = meta;
        if !s.set_payload(kind, payload) {
            return false;
        }
        self.publish(s).is_some()
    }
    fn get(&self, scratch: &mut Vec<u64>, out: &mut Vec<u8>) -> Option<(SlotMeta, u16, u64)> {
        let s = scratch_for::<Stamped<T>>(scratch);
        let gen = self.take_into(s)?;
        out.clear();
        out.extend_from_slice(s.payload());
        Some((s.meta, s.kind as u16, gen as u64))
    }
    fn peek(&self, scratch: &mut Vec<u64>, out: &mut Vec<u8>) -> Option<(SlotMeta, u16, u64)> {
        let s = scratch_for::<Stamped<T>>(scratch);
        let gen = self.current_into(s);
        if gen == 0 {
            return None;
        }
        out.clear();
        out.extend_from_slice(s.payload());
        Some((s.meta, s.kind as u16, gen as u64))
    }
}

/// Where a record slot lives.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SlotForm {
    /// One seqlock slot.
    Slot,
    /// One triple buffer (large; single reader).
    Blob,
    /// One seqlock slot per peer slot (`PeerTable`).
    PeerSlot,
    /// One triple buffer per peer slot.
    PeerBlob,
    /// A game-local bus key (seqlock slot in the `Bus` region).
    Bus,
}

/// One named record slot. `kind` is the record it holds.
#[derive(Debug, Clone, Copy)]
pub struct SlotInfo {
    pub name: &'static str,
    pub kind: u16,
    pub form: SlotForm,
    /// Who writes it.
    pub dir: Dir,
    pub cap: u64,
    /// Republished invalid at `world_leaving` (game-written) / cleared (bus).
    pub world_scoped: bool,
    pub doc: &'static str,
}

/// Every named record slot of every domain.
pub fn slots() -> impl Iterator<Item = &'static SlotInfo> {
    pose::SLOTS.iter()
        .chain(session::SLOTS)
        .chain(combat::SLOTS)
        .chain(world::SLOTS)
        .chain(loadout::SLOTS)
        .chain(interact::SLOTS)
        .chain(bus::SLOTS)
        .chain(dev::SLOTS)
        .chain(mods::SLOTS)
}

pub fn slot_by_name(name: &str) -> Option<&'static SlotInfo> {
    slots().find(|s| s.name == name)
}

/// A named code table (enum values of a record field).
#[derive(Debug, Clone, Copy)]
pub struct EnumInfo {
    pub name: &'static str,
    pub values: &'static [(&'static str, u32)],
}

impl EnumInfo {
    pub fn name_of(&self, v: u32) -> Option<&'static str> {
        self.values.iter().find(|x| x.1 == v).map(|x| x.0)
    }
    pub fn value_of(&self, n: &str) -> Option<u32> {
        self.values.iter().find(|x| x.0 == n).map(|x| x.1)
    }
    /// Highest defined value (for range checks).
    pub fn max(&self) -> u32 {
        self.values.iter().map(|x| x.1).max().unwrap_or(0)
    }
}

/// Every code table of every domain.
pub fn enums() -> impl Iterator<Item = &'static EnumInfo> {
    pose::ENUMS.iter()
        .chain(session::ENUMS)
        .chain(combat::ENUMS)
        .chain(world::ENUMS)
        .chain(loadout::ENUMS)
        .chain(interact::ENUMS)
        .chain(bus::ENUMS)
        .chain(dev::ENUMS)
        .chain(mods::ENUMS)
}

pub fn enum_by_name(name: &str) -> Option<&'static EnumInfo> {
    enums().find(|e| e.name == name)
}

// ---- capability bits (u64, negotiated as AND) --------------------------------------------

pub const CAP_POSE: u64 = 1 << 0;
pub const CAP_STATE: u64 = 1 << 1;
pub const CAP_VITALS: u64 = 1 << 2;
pub const CAP_LOADOUT_KIT: u64 = 1 << 3;
pub const CAP_WORLD: u64 = 1 << 4;
pub const CAP_QUEUES: u64 = 1 << 5;
pub const CAP_INTERACT: u64 = 1 << 6;
pub const CAP_COMBAT: u64 = 1 << 7;
/// Game-local bus: the game sets it alone.
pub const CAP_BUS: u64 = 1 << 8;
pub const CAP_DEVCTL: u64 = 1 << 9;
pub const CAP_NATIVE_SAMPLE: u64 = 1 << 10;
pub const CAP_NATIVE_SERVO: u64 = 1 << 11;
pub const CAP_POSEPLAY_IN_GAME: u64 = 1 << 12;

pub const CAPS: &[(&str, u64)] = &[
    ("POSE", CAP_POSE), ("STATE", CAP_STATE), ("VITALS", CAP_VITALS), ("LOADOUT_KIT", CAP_LOADOUT_KIT),
    ("WORLD", CAP_WORLD), ("QUEUES", CAP_QUEUES), ("INTERACT", CAP_INTERACT), ("COMBAT", CAP_COMBAT),
    ("BUS", CAP_BUS), ("DEVCTL", CAP_DEVCTL), ("NATIVE_SAMPLE", CAP_NATIVE_SAMPLE),
    ("NATIVE_SERVO", CAP_NATIVE_SERVO), ("POSEPLAY_IN_GAME", CAP_POSEPLAY_IN_GAME),
];

// ---- frame() flags -----------------------------------------------------------------------

pub const FLAG_SESSION_CHANGED: u32 = 1 << 0;
pub const FLAG_EVENTS: u32 = 1 << 1;
pub const FLAG_PEERS_CHANGED: u32 = 1 << 2;
pub const FLAG_SIDECAR_RESET: u32 = 1 << 3;
pub const FLAG_WORLD_CHANGED: u32 = 1 << 4;
pub const FLAG_RESYNC: u32 = 1 << 5;
pub const FLAG_REFUSED: u32 = 1 << 6;
pub const FLAG_SIDECAR_LOST: u32 = 1 << 7;
pub const FLAG_SIDECAR_STALLED: u32 = 1 << 8;
pub const FLAG_STATE_CHANGED: u32 = 1 << 9;

pub const FLAGS: &[(&str, u32)] = &[
    ("SESSION_CHANGED", FLAG_SESSION_CHANGED), ("EVENTS", FLAG_EVENTS), ("PEERS_CHANGED", FLAG_PEERS_CHANGED),
    ("SIDECAR_RESET", FLAG_SIDECAR_RESET), ("WORLD_CHANGED", FLAG_WORLD_CHANGED), ("RESYNC", FLAG_RESYNC),
    ("REFUSED", FLAG_REFUSED), ("SIDECAR_LOST", FLAG_SIDECAR_LOST), ("SIDECAR_STALLED", FLAG_SIDECAR_STALLED),
    ("STATE_CHANGED", FLAG_STATE_CHANGED),
];

// ---- kinds -------------------------------------------------------------------------------

/// Where a kind lives.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Form {
    /// Typed seqlock slot (struct named in `KindInfo::ty`).
    Slot,
    /// The game-local bus (its keys are typed records: `SlotForm::Bus` slots).
    Bus,
    /// Per-peer typed seqlock slot (indexed by peer slot).
    PeerSlot,
}

/// Who writes a kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Dir {
    GameToSidecar,
    SidecarToGame,
    /// Game-local (bus) or a tool (dev control).
    Local,
}

#[derive(Debug, Clone, Copy)]
pub struct KindInfo {
    pub kind: u16,
    /// Lua-facing name (`IPC.put("vitals", t)`), also used in the tap and `ipc-dump`.
    pub name: &'static str,
    pub cap: u64,
    pub dir: Dir,
    pub form: Form,
    /// The legacy file this kind replaces (documentation, `ipc-dump`).
    pub replaces: &'static str,
}

/// Every kind, from every domain file.
pub fn kinds() -> impl Iterator<Item = &'static KindInfo> {
    pose::KINDS.iter()
        .chain(session::KINDS)
        .chain(combat::KINDS)
        .chain(world::KINDS)
        .chain(loadout::KINDS)
        .chain(interact::KINDS)
        .chain(bus::KINDS)
        .chain(dev::KINDS)
        .chain(mods::KINDS)
}

pub fn kind_info(kind: u16) -> Option<&'static KindInfo> {
    kinds().find(|k| k.kind == kind)
}

pub fn kind_by_name(name: &str) -> Option<&'static KindInfo> {
    kinds().find(|k| k.name == name)
}

/// Every named POD payload struct, for the C header and the Lua schema.
pub const PODS: &[&TypeDesc] = &[
    &<SlotMeta as IpcType>::DESC,
    &<crate::ring::RecordHeader as IpcType>::DESC,
    &<crate::wire::WireHdr as IpcType>::DESC,
    &<pose::Control as IpcType>::DESC,
    &<pose::PlayWeapon as IpcType>::DESC,
    &<pose::PoseLead as IpcType>::DESC,
    &<pose::PeerPlay as IpcType>::DESC,
    &<pose::PeerDirEntry as IpcType>::DESC,
    &<pose::PeerDir as IpcType>::DESC,
    &<bus::BusDir as IpcType>::DESC,
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kind_ids_are_unique_and_in_range() {
        let mut seen = std::collections::HashSet::new();
        for k in kinds() {
            assert!(seen.insert(k.kind), "duplicate kind {:#06x}", k.kind);
            let role = k.kind & 0xff;
            match (k.form, k.dir) {
                (_, Dir::GameToSidecar) | (_, Dir::Local) => assert!(role < 0x40, "{}", k.name),
                (_, Dir::SidecarToGame) => assert!((0x40..0x80).contains(&role), "{}", k.name),
            }
        }
        let mut names = std::collections::HashSet::new();
        for k in kinds() {
            assert!(names.insert(k.name), "duplicate kind name {}", k.name);
        }
    }

    /// Record kinds are unique, never collide with a POD kind id or name, and
    /// every record slot names a known record. Every name lives in one namespace on the Lua
    /// side (`IPC.put(name)` / `IPC.send(name)`), so a slot never shares a legacy kind's name.
    #[test]
    fn record_kinds_and_slots_are_consistent() {
        let legacy_ids: std::collections::HashSet<u16> = kinds().map(|k| k.kind).collect();
        let legacy_names: std::collections::HashSet<&str> = kinds().map(|k| k.name).collect();
        let mut ids = std::collections::HashSet::new();
        let mut names = std::collections::HashSet::new();
        for r in records() {
            assert!(r.kind != 0, "kind 0 is the legacy wire envelope");
            assert!(ids.insert(r.kind), "duplicate record kind {:#06x}", r.kind);
            assert!(names.insert(r.name), "duplicate record name {}", r.name);
            assert!(!legacy_ids.contains(&r.kind), "record {} reuses legacy kind id {:#06x}", r.name, r.kind);
            assert!(!legacy_names.contains(r.name), "record {} reuses a legacy kind name", r.name);
            assert!(r.head.size() <= crate::ring::MAX_PAYLOAD || r.flow & (flow::G2S | flow::S2G) == 0 || r.max_rows > 0,
                "record {} head does not fit a ring record", r.name);
        }
        for s in slots() {
            assert!(record_info(s.kind).is_some(), "slot {} holds an unknown record kind", s.name);
            assert!(!legacy_names.contains(s.name), "slot {} reuses a legacy kind name", s.name);
            assert!(!names.contains(s.name) || record_info(s.kind).is_some_and(|r| r.name == s.name),
                "slot {} shadows another record's name", s.name);
        }
    }
}
