//! The typed record API: `put` / `get` / `peer` / `send` / `poll` data /
//! `bus_put` / `bus_get` for the record and slot names of the schema registries
//! (`hsmp_ipc::schema::{records, slots}`), and the generic `world_leaving` invalidation.
//!
//! Every name is resolved once into a hash map. Slots are reached through `Segment::slot` + `schema::RawSlot`, so a new slot
//! needs no code here. Writes are marshalled by [`crate::marshal`] and validated with the
//! record's own check, so the game can never write a record the sidecar / server would refuse.
//!
//! Blob forms (triple buffers) have a single reader: for sidecar-written blobs that is this
//! module (the bytes are cached per version and served to every Lua state); for game-written
//! blobs it is the sidecar, so a read-back serves the copy this module published.

use std::collections::HashMap;
use std::ffi::c_int;
use std::hash::{BuildHasherDefault, Hasher};

use hsmp_ipc::header::ctr;
use hsmp_ipc::ring::{Record, MAX_PAYLOAD};
use hsmp_ipc::schema::bus::{BUS_KEYS, BUS_KEY_BYTES, BUS_VALUE_BYTES};
use hsmp_ipc::schema::pose::MAX_PEER_SLOTS;
use hsmp_ipc::schema::{self as sc, flow, Dir, RawSlot, RecordInfo, SlotForm, SlotInfo, SlotMeta};
use hsmp_ipc::segment::Segment;

use crate::lua::*;
use crate::marshal::{self, MErr};
use crate::native::Native;

/// FNV-1a: names are short; SipHash is not needed against our own schema.
#[derive(Default)]
pub struct Fnv(u64);
impl Hasher for Fnv {
    fn finish(&self) -> u64 {
        self.0
    }
    fn write(&mut self, bytes: &[u8]) {
        let mut h = if self.0 == 0 { 0xcbf2_9ce4_8422_2325 } else { self.0 };
        for &b in bytes {
            h ^= b as u64;
            h = h.wrapping_mul(0x0000_0100_0000_01b3);
        }
        self.0 = h;
    }
}
type FnvMap<K, V> = HashMap<K, V, BuildHasherDefault<Fnv>>;

/// A slot registered by a test (not in the segment): its info and one `RawSlot` per peer
/// slot (one for single forms). See [`crate::api::register_test_schema`].
pub struct TestSlot {
    pub info: SlotInfo,
    pub raw: &'static [&'static dyn RawSlot],
}

#[derive(Clone, Copy)]
pub(crate) struct SlotEnt {
    pub info: &'static SlotInfo,
    pub rec: &'static RecordInfo,
    test: Option<usize>,
    idx: usize,
}

#[derive(Default, Clone, Copy)]
struct Entry {
    record: Option<&'static RecordInfo>,
    slot: Option<usize>,
}

struct Cached {
    ver: u64,
    epoch: u64,
    ok: bool,
    bytes: Vec<u8>,
}

/// What a slot read found.
enum Read {
    /// Never written / no valid value.
    None,
    /// Unchanged since `last`.
    Same(u64),
    /// A new value; the payload is in `RecState::rd`.
    Data(u64),
}

/// Typed-record state (process-global, inside `Native`).
#[derive(Default)]
pub struct RecState {
    built: bool,
    names: FnvMap<&'static str, Entry>,
    kinds: FnvMap<u16, &'static RecordInfo>,
    slots: Vec<SlotEnt>,
    test_records: &'static [RecordInfo],
    test_slots: &'static [TestSlot],
    /// Marshal output (reused).
    pub buf: Vec<u8>,
    /// Read payload (reused).
    pub rd: Vec<u8>,
    /// `RawSlot` scratch (reused).
    pub scratch: Vec<u64>,
    blobs: FnvMap<(usize, usize), Cached>,
}

impl RecState {
    fn build(&mut self) {
        if self.built {
            return;
        }
        self.built = true;
        self.names.clear();
        self.kinds.clear();
        self.slots.clear();
        self.blobs.clear();
        for r in sc::records().chain(self.test_records.iter()) {
            self.names.entry(r.name).or_default().record = Some(r);
            self.kinds.insert(r.kind, r);
        }
        let schema = sc::slots().map(|s| (s, None));
        let tests = self.test_slots.iter().enumerate().map(|(i, t)| (&t.info, Some(i)));
        for (info, test) in schema.chain(tests) {
            let Some(rec) = self.kinds.get(&info.kind).copied() else { continue };
            let idx = self.slots.len();
            self.slots.push(SlotEnt { info, rec, test, idx });
            self.names.entry(info.name).or_default().slot = Some(idx);
        }
    }

    fn entry(&mut self, name: &[u8]) -> Entry {
        self.build();
        std::str::from_utf8(name).ok().and_then(|n| self.names.get(n).copied()).unwrap_or_default()
    }

    /// The record of a ring / slot kind.
    pub fn record_of(&mut self, kind: u16) -> Option<&'static RecordInfo> {
        self.build();
        self.kinds.get(&kind).copied()
    }

    /// The record kind of `name` if it may arrive as an S2G event.
    pub fn s2g_record_kind(&mut self, name: &str) -> Option<u16> {
        let e = self.entry(name.as_bytes());
        e.record.filter(|r| r.flow & flow::S2G != 0).map(|r| r.kind)
    }

    fn raw(&self, seg: &'static Segment, e: &SlotEnt, peer: usize) -> Option<&'static dyn RawSlot> {
        match e.test {
            Some(i) => self.test_slots.get(i)?.raw.get(peer).copied(),
            None => seg.slot(e.info, peer),
        }
    }

    pub(crate) fn set_test_schema(&mut self, records: &'static [RecordInfo], slots: &'static [TestSlot]) {
        self.test_records = records;
        self.test_slots = slots;
        self.built = false;
    }

    /// Every slot: (entry, raw) pairs for the generic world-leave invalidation.
    fn world_scoped_game_slots(&mut self) -> Vec<SlotEnt> {
        self.build();
        self.slots
            .iter()
            .filter(|e| e.info.world_scoped && e.info.dir != Dir::SidecarToGame && matches!(e.info.form, SlotForm::Slot | SlotForm::Blob))
            .copied()
            .collect()
    }
}

#[inline]
unsafe fn refuse(L: *mut lua_State, e: MErr) -> c_int {
    unsafe { nil_err(L, &e.message()) }
}

#[inline]
unsafe fn opt_out(L: *mut lua_State, idx: c_int) -> Option<c_int> {
    unsafe { is_table(L, idx).then(|| lua_absindex(L, idx)) }
}

impl Native {
    fn sidecar_epoch_cur(&self) -> u64 {
        self.seg().map_or(0, |s| s.header.sidecar.epoch.load(std::sync::atomic::Ordering::Acquire))
    }

    fn caps_eff(&self) -> u64 {
        self.seg().map_or(0, |s| s.header.caps_effective.load(std::sync::atomic::Ordering::Acquire))
    }

    fn bump(&self, c: usize) {
        if let Some(s) = self.seg() {
            s.header.game.count(c, 1);
        }
    }

    /// Marshal the table at `idx` as `r` into `self.rec.buf` and validate it.
    unsafe fn marshal_checked(&mut self, L: *mut lua_State, idx: c_int, r: &RecordInfo) -> Result<(), MErr> {
        let res = unsafe { marshal::table_to_record(L, idx, r, &mut self.rec.buf) }.and_then(|_| marshal::validate(r, &self.rec.buf));
        if res == Err(MErr::TooBig) {
            self.bump(ctr::TOO_BIG);
        }
        res
    }

    // ---- put / get ---------------------------------------------------------------------------

    /// `put(slot, t)` for game-written (or game-local) record slots.
    pub unsafe fn r_put(&mut self, L: *mut lua_State) -> c_int {
        unsafe {
            let e = match arg_bytes(L, 1) {
                Some(n) => self.rec.entry(n),
                None => Entry::default(),
            };
            let Some(si) = e.slot else { return nil_err(L, "bad") };
            let Some(seg) = self.seg() else { return nil_err(L, "not open") };
            let se = self.rec.slots[si];
            if !matches!(se.info.dir, Dir::GameToSidecar | Dir::Local) || !matches!(se.info.form, SlotForm::Slot | SlotForm::Blob) {
                return nil_err(L, "bad");
            }
            if se.info.dir == Dir::GameToSidecar {
                let eff = self.caps_eff();
                if se.info.cap != 0 && eff != 0 && eff & se.info.cap == 0 {
                    return nil_err(L, "cap");
                }
            }
            if let Err(err) = self.marshal_checked(L, 2, se.rec) {
                return refuse(L, err);
            }
            let Some(raw) = self.rec.raw(seg, &se, 0) else { return nil_err(L, "bad") };
            if self.rec.buf.len() > raw.cap() {
                self.bump(ctr::TOO_BIG);
                return nil_err(L, "too_big");
            }
            let meta = self.meta();
            let rs = &mut self.rec;
            if !raw.put(meta, se.rec.kind, &rs.buf, &mut rs.scratch) {
                return nil_err(L, "too_big");
            }
            if se.info.form == SlotForm::Blob {
                // The sidecar is the blob's reader; keep our own copy for read-backs.
                let c = rs.blobs.entry((se.idx, 0)).or_insert(Cached { ver: 0, epoch: 0, ok: false, bytes: Vec::new() });
                c.ver = raw.version();
                c.epoch = self.epoch;
                c.ok = true;
                c.bytes.clear();
                c.bytes.extend_from_slice(&rs.buf);
            }
            seg.header.game.count(ctr::SLOT_WRITES, 1);
            self.wrote = true;
            lua_pushboolean(L, 1);
            1
        }
    }

    /// Read slot `se` (peer slot `peer`) into `self.rec.rd`.
    fn read_slot(&mut self, seg: &'static Segment, se: SlotEnt, peer: usize, last: i64) -> Read {
        let Some(raw) = self.rec.raw(seg, &se, peer) else { return Read::None };
        let v = raw.version();
        if v == 0 {
            return Read::None;
        }
        let sidecar = se.info.dir == Dir::SidecarToGame;
        let ep = if sidecar { self.sidecar_epoch_cur() } else { self.epoch };
        let blob = matches!(se.info.form, SlotForm::Blob | SlotForm::PeerBlob);
        if blob {
            if sidecar {
                let stale = self.rec.blobs.get(&(se.idx, peer)).is_none_or(|c| c.ver != v || c.epoch != ep);
                if stale {
                    let rs = &mut self.rec;
                    if let Some((meta, kind, gen)) = raw.get(&mut rs.scratch, &mut rs.rd) {
                        let ok = meta.writer_epoch == ep
                            && meta.valid != 0
                            && kind == se.rec.kind
                            && !rs.rd.is_empty()
                            && (se.rec.check)(&rs.rd).is_ok();
                        let c = rs.blobs.entry((se.idx, peer)).or_insert(Cached { ver: 0, epoch: 0, ok: false, bytes: Vec::new() });
                        c.ver = gen;
                        c.epoch = ep;
                        c.ok = ok;
                        c.bytes.clear();
                        if ok {
                            c.bytes.extend_from_slice(&rs.rd);
                        } else if meta.valid != 0 && meta.writer_epoch == ep {
                            seg.header.game.count(ctr::BAD_RECORD, 1);
                        }
                    }
                }
            }
            let rs = &mut self.rec;
            let Some(c) = rs.blobs.get(&(se.idx, peer)) else { return Read::None };
            if !c.ok || c.epoch != ep {
                return Read::None;
            }
            if c.ver as i64 == last {
                return Read::Same(c.ver);
            }
            rs.rd.clear();
            rs.rd.extend_from_slice(&c.bytes);
            return Read::Data(c.ver);
        }
        if v as i64 == last {
            return Read::Same(v);
        }
        let rs = &mut self.rec;
        let Some((meta, kind, seq)) = raw.get(&mut rs.scratch, &mut rs.rd) else {
            seg.header.game.count(ctr::BUSY, 1);
            return Read::None;
        };
        if meta.writer_epoch != ep || meta.valid == 0 || rs.rd.is_empty() {
            return Read::None;
        }
        if kind != se.rec.kind || (se.rec.check)(&rs.rd).is_err() {
            seg.header.game.count(ctr::BAD_RECORD, 1);
            return Read::None;
        }
        seg.header.game.count(ctr::SLOT_READS, 1);
        Read::Data(seq)
    }

    /// `get(slot, last, out?)` -> `ver, t` | `ver` (unchanged) | `nil` (no value).
    pub unsafe fn r_get(&mut self, L: *mut lua_State) -> c_int {
        unsafe {
            let e = match arg_bytes(L, 1) {
                Some(n) => self.rec.entry(n),
                None => return nil_err(L, "bad"),
            };
            let Some(si) = e.slot else { return nil_err(L, "bad") };
            let se = self.rec.slots[si];
            if !matches!(se.info.form, SlotForm::Slot | SlotForm::Blob) {
                return nil_err(L, "bad");
            }
            let Some(last) = opt_int(L, 2, -1) else { return nil_err(L, "bad") };
            let out = opt_out(L, 3);
            let Some(seg) = self.seg() else { return nil_err(L, "not open") };
            match self.read_slot(seg, se, 0, last) {
                Read::None => {
                    lua_pushnil(L);
                    1
                }
                Read::Same(v) => {
                    lua_pushinteger(L, v as i64);
                    1
                }
                Read::Data(v) => {
                    lua_pushinteger(L, v as i64);
                    marshal::record_to_table(L, se.rec, &self.rec.rd, out);
                    2
                }
            }
        }
    }

    /// `peer(slot, peer_slot, last, out?)` for per-peer record slots.
    pub unsafe fn r_peer(&mut self, L: *mut lua_State) -> c_int {
        unsafe {
            let e = match arg_bytes(L, 1) {
                Some(n) => self.rec.entry(n),
                None => Entry::default(),
            };
            let Some(si) = e.slot else { return nil_err(L, "bad") };
            let se = self.rec.slots[si];
            if !matches!(se.info.form, SlotForm::PeerSlot | SlotForm::PeerBlob) {
                return nil_err(L, "bad");
            }
            let Some(ps) = arg_int(L, 2).filter(|&p| p >= 0 && (p as usize) < MAX_PEER_SLOTS) else { return nil_err(L, "bad") };
            let Some(last) = opt_int(L, 3, -1) else { return nil_err(L, "bad") };
            let out = opt_out(L, 4);
            let Some(seg) = self.seg() else { return nil_err(L, "not open") };
            match self.read_slot(seg, se, ps as usize, last) {
                Read::None | Read::Same(_) => {
                    lua_pushnil(L);
                    1
                }
                Read::Data(v) => {
                    lua_pushinteger(L, v as i64);
                    marshal::record_to_table(L, se.rec, &self.rec.rd, out);
                    2
                }
            }
        }
    }

    // ---- send / events -------------------------------------------------------------------------

    /// `send(kind, t)`: a G2S record kind.
    pub unsafe fn r_send(&mut self, L: *mut lua_State) -> c_int {
        unsafe {
            let e = match arg_bytes(L, 1) {
                Some(n) => self.rec.entry(n),
                None => Entry::default(),
            };
            let Some(r) = e.record else { return nil_err(L, "bad") };
            if self.seg().is_none() {
                return nil_err(L, "not open");
            }
            if r.flow & flow::G2S == 0 {
                return nil_err(L, "bad");
            }
            if r.cap != 0 && self.caps_eff() & r.cap == 0 {
                return nil_err(L, "cap");
            }
            if let Err(err) = self.marshal_checked(L, 2, r) {
                return refuse(L, err);
            }
            if self.rec.buf.len() > MAX_PAYLOAD {
                self.bump(ctr::TOO_BIG);
                return nil_err(L, "too_big");
            }
            self.req_counter = self.req_counter.wrapping_add(1);
            let req_id = (((self.epoch as u32) & 0x7fff_ffff) as u64) << 32 | self.req_counter as u64;
            let Some(s) = self.seg() else { return nil_err(L, "not open") };
            match s.g2s().push_msg(self.epoch, r.kind, 0, 0, 0, req_id, &self.rec.buf) {
                Ok(_) => {
                    s.header.game.count(ctr::MSGS_OUT, 1);
                    s.header.game.high_water(ctr::RING_HIGH_WATER, s.g2s().len());
                    self.wrote = true;
                    lua_pushinteger(L, req_id as i64);
                    1
                }
                Err(hsmp_ipc::ring::PushError::Full) => {
                    s.header.game.count(ctr::RING_FULL, 1);
                    nil_err(L, "full")
                }
                Err(hsmp_ipc::ring::PushError::TooBig) => nil_err(L, "too_big"),
                Err(hsmp_ipc::ring::PushError::Corrupt) => nil_err(L, "bad"),
            }
        }
    }

    // ---- bus -------------------------------------------------------------------------------------

    /// The bus slot of `key`, registering it (with its world-scoped bit) if new.
    pub(crate) fn bus_index(&mut self, key: &str, world_scoped: bool) -> Result<usize, &'static str> {
        if key.is_empty() || key.len() > BUS_KEY_BYTES {
            return Err("bad");
        }
        if let Some(i) = self.bus_dir.find(key) {
            return Ok(i);
        }
        let i = self.bus_dir.count as usize;
        if i >= BUS_KEYS {
            return Err("full");
        }
        self.bus_dir.names[i] = [0; BUS_KEY_BYTES];
        self.bus_dir.names[i][..key.len()].copy_from_slice(key.as_bytes());
        if world_scoped {
            self.bus_dir.world_scoped |= 1u64 << i;
        }
        self.bus_dir.count += 1;
        if let Some(s) = self.seg() {
            s.bus.dir.write(&self.bus_dir);
        }
        Ok(i)
    }

    /// The bus-form record slot named `key`, if any.
    fn bus_slot(&mut self, L: *mut lua_State) -> Option<SlotEnt> {
        // SAFETY: plain stack read.
        let e = self.rec.entry(unsafe { arg_bytes(L, 1) }?);
        let se = self.rec.slots[e.slot?];
        (se.info.form == SlotForm::Bus).then_some(se)
    }

    /// `bus_put(key, t)`: a typed bus key (any other key: "bad").
    pub unsafe fn r_bus_put(&mut self, L: *mut lua_State) -> c_int {
        unsafe {
            let Some(se) = self.bus_slot(L) else { return nil_err(L, "bad") };
            if self.seg().is_none() {
                return nil_err(L, "not open");
            }
            if let Err(err) = self.marshal_checked(L, 2, se.rec) {
                return refuse(L, err);
            }
            if self.rec.buf.len() > BUS_VALUE_BYTES {
                self.bump(ctr::TOO_BIG);
                return nil_err(L, "too_big");
            }
            let idx = match self.bus_index(se.info.name, se.info.world_scoped) {
                Ok(i) => i,
                Err(e) => return nil_err(L, e),
            };
            let meta = self.meta();
            let Some(s) = self.seg() else { return nil_err(L, "not open") };
            let rs = &mut self.rec;
            if !s.bus.keys[idx].put(meta, se.rec.kind, &rs.buf, &mut rs.scratch) {
                return nil_err(L, "too_big");
            }
            s.header.game.count(ctr::SLOT_WRITES, 1);
            self.wrote = true;
            lua_pushboolean(L, 1);
            1
        }
    }

    /// `bus_get(key, last, out?)` -> `ver, t` | `ver` (unchanged) | `0` (no value).
    pub unsafe fn r_bus_get(&mut self, L: *mut lua_State) -> c_int {
        unsafe {
            let Some(se) = self.bus_slot(L) else { return nil_err(L, "bad") };
            let Some(last) = opt_int(L, 2, -1) else { return nil_err(L, "bad") };
            let out = opt_out(L, 3);
            let Some(s) = self.seg() else { return nil_err(L, "not open") };
            let Some(idx) = self.bus_dir.find(se.info.name) else {
                lua_pushinteger(L, 0);
                return 1;
            };
            let slot = &s.bus.keys[idx];
            let seq = slot.seq();
            if seq == 0 {
                lua_pushinteger(L, 0);
                return 1;
            }
            if seq as i64 == last {
                lua_pushinteger(L, seq as i64);
                return 1;
            }
            let rs = &mut self.rec;
            let got = RawSlot::get(slot, &mut rs.scratch, &mut rs.rd);
            let Some((meta, kind, seq)) = got else {
                s.header.game.count(ctr::BUSY, 1);
                lua_pushinteger(L, 0);
                return 1;
            };
            if meta.valid == 0 || rs.rd.is_empty() || kind != se.rec.kind || (se.rec.check)(&rs.rd).is_err() {
                if meta.valid != 0 && !rs.rd.is_empty() {
                    s.header.game.count(ctr::BAD_RECORD, 1);
                }
                lua_pushinteger(L, 0);
                return 1;
            }
            lua_pushinteger(L, seq as i64);
            marshal::record_to_table(L, se.rec, &rs.rd, out);
            2
        }
    }

    // ---- world leave -----------------------------------------------------------------------------

    /// Republish every world-scoped game-written record slot invalid (`valid = 0`, no payload),
    /// generically from `SlotInfo`. World-scoped typed bus keys are cleared by the bus-key
    /// loop in `world_leaving` (their `BusDir` bit is set at registration).
    pub(crate) fn invalidate_world_records(&mut self, world_epoch: u32) {
        let Some(seg) = self.seg() else { return };
        for se in self.rec.world_scoped_game_slots() {
            let Some(raw) = self.rec.raw(seg, &se, 0) else { continue };
            if raw.version() == 0 {
                continue;
            }
            let mut meta: SlotMeta = self.meta();
            meta.valid = 0;
            meta.world_epoch = world_epoch;
            let rs = &mut self.rec;
            raw.put(meta, se.rec.kind, &[], &mut rs.scratch);
            if let Some(c) = rs.blobs.get_mut(&(se.idx, 0)) {
                c.ok = false;
                c.ver = raw.version();
            }
        }
    }
}

/// Fill the poll entry table `t` for ring record `rec`: `kind`, `req_id`, `peer`, `aux`,
/// `data`. Returns false if the payload did not validate or the kind is not a record
/// (`data` is then nil).
///
/// # Safety
/// `L` valid; `t` an absolute index of a table.
pub(crate) unsafe fn fill_event(L: *mut lua_State, t: c_int, rec: &Record, rs: &mut RecState) -> bool {
    unsafe {
        let kind = rec.hdr.kind;
        set_int(L, t, "req_id", rec.hdr.req_id as i64);
        set_int(L, t, "peer", rec.hdr.peer as i64);
        set_int(L, t, "aux", rec.hdr.aux as i64);
        let Some(r) = rs.record_of(kind) else {
            set_str(L, t, "kind", "unknown");
            set_nil(L, t, "data");
            return false;
        };
        set_str(L, t, "kind", r.name);
        let p = rec.payload();
        if (r.check)(p).is_ok() && marshal::record_to_table(L, r, p, None) {
            rawset_str(L, t, "data");
            return true;
        }
        set_nil(L, t, "data");
        false
    }
}

/// The S2G record kind of `name` (`subscribe` filters).
pub(crate) fn subscribe_kind(rs: &mut RecState, name: &str) -> Option<u16> {
    rs.s2g_record_kind(name)
}
