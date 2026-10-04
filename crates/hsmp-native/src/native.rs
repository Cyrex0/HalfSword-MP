//! Process-global game-side IPC client: the mapping, epochs, the S2G event log with one
//! cursor per Lua state, blob caches and scratch buffers. Every method runs on the game
//! thread under the in-process lock (`api.rs`), and writes Lua results on the caller's stack.

use std::collections::HashMap;
use std::ffi::c_int;
use std::thread::ThreadId;

use hsmp_ipc::handshake::{self, SideParams};
use hsmp_ipc::header::{ctr, RefuseCode, SideState};
use hsmp_ipc::layout::{boxed_zeroed, Pod};
use hsmp_ipc::ring::{Pop, Record};
use hsmp_ipc::schema::bus::{BusDir, BusValue, BUS_KEYS};
use hsmp_ipc::schema::pose::{
    PeerDir, PeerPlay, MAX_PEER_SLOTS, NB, PEER_PLAY_HAS_CONTROL,
    PEER_PLAY_HAS_ROOT, PEER_PLAY_V2, PLAY_MODES, PLAY_SLOTS,
};
use hsmp_ipc::schema::{self as sc, SlotMeta};
use hsmp_ipc::segment::Segment;
use hsmp_ipc::shm::{self, Doorbell, Mapping, ProcessHandle};

use hsmp_ipc::{ABI_MAJOR, ABI_MINOR, LAYOUT_HASH};

use crate::lua::*;

/// Capabilities the game side offers by default: everything implemented
/// (HSMPWorld reads and writes the world blobs as tables, so WORLD is on).
/// `HSMP_IPC_GAME_CAPS` can still mask; `GAME_CAPS_ALL` lists everything implemented.
pub const GAME_CAPS: u64 = sc::CAP_POSE
    | sc::CAP_STATE
    | sc::CAP_VITALS
    | sc::CAP_LOADOUT_KIT
    | sc::CAP_QUEUES
    | sc::CAP_INTERACT
    | sc::CAP_COMBAT
    | sc::CAP_BUS
    | sc::CAP_DEVCTL
    | sc::CAP_WORLD;

/// Everything the binding implements; the `HSMP_IPC_GAME_CAPS` mask selects from this.
pub const GAME_CAPS_ALL: u64 = GAME_CAPS | sc::CAP_NATIVE_SAMPLE | sc::CAP_NATIVE_SERVO;

/// In-process event log size (events).
pub const EVENT_LOG: usize = 4096;
/// Scratch buffer for record slots (the world record blobs reach ~120 KiB).
const SCRATCH_BYTES: usize = 128 * 1024 + 64;

#[derive(Default)]
pub(crate) struct Cursor {
    next: u64,
    filter: Option<Vec<u16>>,
}

pub struct Native {
    pub(crate) map: Option<Mapping>,
    bell_g2s: Option<Doorbell>,
    _bell_s2g: Option<Doorbell>,
    pub(crate) epoch: u64,
    pub game_thread: Option<ThreadId>,
    pub poisoned: bool,
    pub(crate) wrote: bool,
    pub(crate) last_flags: u32,
    pub(crate) world_key: Option<Vec<u8>>,
    pub(crate) left_world: bool,
    pub(crate) sidecar_epoch: u64,
    pub(crate) sidecar: Option<ProcessHandle>,
    pub(crate) sidecar_lost: bool,
    pub(crate) last_proc_check: u64,
    pub(crate) qpc_freq: u64,
    pub(crate) seen_state_gens: [u32; 5],
    pub(crate) seen_dir_seq: u64,
    pub(crate) seen_resync: u32,
    pub(crate) sample_seq: u32,
    pub(crate) req_counter: u32,
    // event log
    pub(crate) log: Vec<Record>,
    pub(crate) log_head: u64,
    pub(crate) cursors: HashMap<usize, Cursor>,
    // caches and scratch
    pub(crate) bus_dir: Box<BusDir>,
    pub(crate) scratch: Vec<u64>,
    pub(crate) pose: Box<crate::pose_hot::PoseHot>,
    pub(crate) play: Box<PeerPlay>,
    pub(crate) dir: Box<PeerDir>,
    pub(crate) seen_mods: Vec<String>,
    // typed records, DevCtl fan-out, spawned processes (records.rs, dev.rs, proc.rs)
    pub(crate) rec: crate::records::RecState,
    pub(crate) dev: crate::dev::DevState,
    pub(crate) procs: crate::proc::ProcState,
    pub(crate) sample: crate::sample::SampleState,
}

impl Default for Native {
    fn default() -> Native {
        Native::new()
    }
}

impl Native {
    pub fn new() -> Native {
        Native {
            map: None,
            bell_g2s: None,
            _bell_s2g: None,
            epoch: 0,
            game_thread: None,
            poisoned: false,
            wrote: false,
            last_flags: 0,
            world_key: None,
            left_world: false,
            sidecar_epoch: 0,
            sidecar: None,
            sidecar_lost: false,
            last_proc_check: 0,
            qpc_freq: shm::qpc_freq().max(1),
            seen_state_gens: [0; 5],
            seen_dir_seq: 0,
            seen_resync: 0,
            sample_seq: 0,
            req_counter: 0,
            log: Vec::new(),
            log_head: 0,
            cursors: HashMap::new(),
            bus_dir: boxed_zeroed(),
            scratch: vec![0u64; SCRATCH_BYTES / 8],
            pose: Box::default(),
            play: boxed_zeroed(),
            dir: boxed_zeroed(),
            seen_mods: Vec::new(),
            rec: Default::default(),
            dev: Default::default(),
            procs: Default::default(),
            sample: Default::default(),
        }
    }

    /// The segment. The mapping is never dropped once created (it lives in the process-global
    /// state for the life of the process), so the reference is detached from `self`.
    pub fn seg(&self) -> Option<&'static Segment> {
        let s = self.map.as_ref()?.segment()? as *const Segment;
        // SAFETY: `self.map` is set once and never replaced or dropped while the process runs.
        Some(unsafe { &*s })
    }

    fn count(&self, idx: usize) {
        if let Some(s) = self.seg() {
            s.header.game.count(idx, 1);
        }
    }

    pub fn count_wrong_thread(&self) {
        self.count(ctr::WRONG_THREAD);
    }

    /// Poison the segment after a caught panic.
    pub fn poison(&mut self, why: &str) {
        self.poisoned = true;
        if let Some(s) = self.seg() {
            s.header.game.count(ctr::PANICS, 1);
            handshake::poison(s, why);
        }
    }

    // ---- registration ------------------------------------------------------------------

    /// A Lua state got the API: its cursor starts at "now".
    pub fn register_state(&mut self, main: usize) {
        self.cursors.insert(main, Cursor { next: self.log_head, filter: None });
        self.dev.register(main);
    }

    /// F registered a mod by name; a repeat name means UE4SS restarted the mods.
    pub fn note_mod(&mut self, name: &str) {
        if self.seen_mods.iter().any(|n| n == name) {
            if let Some(s) = self.seg() {
                s.header.game_lua_gen.fetch_add(1, std::sync::atomic::Ordering::AcqRel);
            }
            self.seen_mods.clear();
        }
        self.seen_mods.push(name.to_string());
    }

    // ---- helpers -----------------------------------------------------------------------

    fn now_qpc(&self) -> u64 {
        shm::qpc()
    }

    pub fn now_us(&self) -> i64 {
        (shm::qpc() as u128 * 1_000_000 / self.qpc_freq as u128) as i64
    }

    pub(crate) fn meta(&mut self) -> SlotMeta {
        self.sample_seq = self.sample_seq.wrapping_add(1);
        let (we, se) = self.seg().map_or((0, 0), |s| {
            (s.header.world_epoch.load(std::sync::atomic::Ordering::Acquire), s.header.session_epoch.load(std::sync::atomic::Ordering::Acquire))
        });
        SlotMeta { writer_epoch: self.epoch, world_epoch: we, session_epoch: se, valid: 1, sample_seq: self.sample_seq, t_us: self.now_us() as u64 }
    }

    /// The scratch area viewed as a `T` (alignment 8, size checked).
    fn scratch_as<T: Pod>(&mut self) -> &mut T {
        assert!(core::mem::size_of::<T>() <= self.scratch.len() * 8 && core::mem::align_of::<T>() <= 8);
        // SAFETY: size and alignment checked; Pod accepts any bytes.
        unsafe { &mut *(self.scratch.as_mut_ptr() as *mut T) }
    }

    // ---- ipc_open / info ---------------------------------------------------------------

    pub unsafe fn ipc_open(&mut self, L: *mut lua_State) -> c_int {
        unsafe {
            if let Some(m) = &self.map {
                push_str(L, m.name());
                return 1;
            }
            let pid = shm::current_pid();
            let ct = match shm::process_create_time(pid) {
                Ok(v) => v,
                Err(e) => return nil_err(L, &format!("create time: {}", e)),
            };
            let name = shm::mapping_name(pid, ct);
            let (m, existed) = match Mapping::create(&name) {
                Ok(v) => v,
                Err(e) => return nil_err(L, &format!("create mapping: {}", e)),
            };
            if existed {
                return nil_err(L, "ipc owned by another native copy in this process");
            }
            let caps = std::env::var("HSMP_IPC_GAME_CAPS")
                .ok()
                .and_then(|v| u64::from_str_radix(v.trim_start_matches("0x"), 16).ok())
                .map_or(GAME_CAPS, |mask| GAME_CAPS_ALL & mask)
                // Native sampling and servo are offered only with the engine-reflection vtable (main.dll on the pinned UE4SS).
                | if crate::reflect::vt().is_some() { sc::CAP_NATIVE_SAMPLE | sc::CAP_NATIVE_SERVO } else { 0 };
            self.epoch = shm::random_u64();
            let build = option_env!("HSMP_BUILD_ID").unwrap_or(env!("CARGO_PKG_VERSION"));
            let p = SideParams { pid, create_time: ct, epoch: self.epoch, caps, build_id: build };
            if let Err(r) = handshake::init_game(m.segment_ptr(), m.len(), &p, self.qpc_freq) {
                return nil_err(L, &r.to_string());
            }
            let (g2s, s2g) = shm::doorbell_names(&name);
            self.bell_g2s = Doorbell::create(&g2s).ok();
            self._bell_s2g = Doorbell::create(&s2g).ok();
            self.map = Some(m);
            push_str(L, &name);
            1
        }
    }

    pub unsafe fn ipc_info(&mut self, L: *mut lua_State) -> c_int {
        unsafe {
            use std::sync::atomic::Ordering::Acquire;
            lua_createtable(L, 0, 24);
            let t = lua_gettop(L);
            let Some(s) = self.seg() else {
                set_str(L, t, "state", if self.poisoned { "disabled" } else { "closed" });
                set_int(L, t, "abi_major", ABI_MAJOR as i64);
                set_int(L, t, "abi_minor", ABI_MINOR as i64);
                set_str(L, t, "layout_hash", &format!("{:016x}", LAYOUT_HASH));
                return 1;
            };
            let h = &s.header;
            let name = self.map.as_ref().map(|m| m.name().to_string()).unwrap_or_default();
            set_str(L, t, "name", &name);
            let st = if self.poisoned { "disabled" } else { h.game.state().map_or("?", |x| x.as_str()) };
            set_str(L, t, "state", st);
            set_int(L, t, "abi_major", ABI_MAJOR as i64);
            set_int(L, t, "abi_minor", ABI_MINOR as i64);
            set_str(L, t, "layout_hash", &format!("{:016x}", LAYOUT_HASH));
            set_int(L, t, "caps_game", h.game.caps.load(Acquire) as i64);
            set_int(L, t, "caps_sidecar", h.sidecar.caps.load(Acquire) as i64);
            set_int(L, t, "caps_effective", h.caps_effective.load(Acquire) as i64);
            set_str(L, t, "game_epoch", &format!("{:016x}", h.game.epoch.load(Acquire)));
            set_str(L, t, "sidecar_epoch", &format!("{:016x}", h.sidecar.epoch.load(Acquire)));
            set_int(L, t, "sidecar_pid", h.sidecar.pid.load(Acquire) as i64);
            set_str(L, t, "sidecar_state", h.sidecar.state().map_or("?", |x| x.as_str()));
            match h.sidecar.hb_age_s(self.now_qpc(), h.qpc_freq.load(Acquire)) {
                Some(a) => set_num(L, t, "sidecar_hb_age_s", a),
                None => set_nil(L, t, "sidecar_hb_age_s"),
            }
            set_int(L, t, "world_epoch", h.world_epoch.load(Acquire) as i64);
            set_int(L, t, "session_epoch", h.session_epoch.load(Acquire) as i64);
            set_int(L, t, "attach_count", h.attach_count.load(Acquire) as i64);
            set_int(L, t, "game_lua_gen", h.game_lua_gen.load(Acquire) as i64);
            set_int(L, t, "refuse_code", h.prefix.refuse_code() as i64);
            set_str(L, t, "refuse_detail", &h.prefix.refuse_detail());
            lua_createtable(L, 0, 16);
            let c = lua_gettop(L);
            for (i, n) in ctr::NAMES.iter().enumerate() {
                set_int(L, c, n, h.game.counter(i) as i64);
            }
            rawset_str(L, t, "counters");
            1
        }
    }

    // ---- frame -------------------------------------------------------------------------

    pub unsafe fn frame(&mut self, L: *mut lua_State) -> c_int {
        unsafe {
            if self.game_thread.is_none() {
                self.game_thread = Some(std::thread::current().id());
            }
            let flags = self.pump(arg_bytes(L, 1));
            lua_pushinteger(L, flags as i64);
            1
        }
    }

    fn pump(&mut self, world_key: Option<&[u8]>) -> u32 {
        use std::sync::atomic::Ordering::{AcqRel, Acquire};
        let Some(s) = self.seg() else { return 0 };
        let h = &s.header;
        let now = self.now_qpc();
        h.game.beat(now);
        let mut flags = 0u32;
        // world key
        if let Some(k) = world_key {
            match &self.world_key {
                Some(prev) if prev.as_slice() != k => {
                    if !self.left_world {
                        h.world_epoch.fetch_add(1, AcqRel);
                    }
                    flags |= sc::FLAG_WORLD_CHANGED;
                    self.sample.drop_world(false);
                    self.world_key = Some(k.to_vec());
                    self.left_world = false;
                }
                None => self.world_key = Some(k.to_vec()),
                _ => {}
            }
        }
        // doorbell: at most once per frame, only if something was written
        if self.wrote {
            if let Some(b) = &self.bell_g2s {
                b.ring();
            }
            self.wrote = false;
        }
        // sidecar attach / restart
        let ep = h.sidecar.epoch.load(Acquire);
        if ep != 0 && ep != self.sidecar_epoch {
            self.sidecar_epoch = ep;
            flags |= sc::FLAG_SIDECAR_RESET;
            let pid = h.sidecar.pid.load(Acquire);
            self.sidecar = ProcessHandle::open(pid).ok();
            self.sidecar_lost = false;
        }
        // sidecar death (1 Hz handle check, off the hot path)
        if now.wrapping_sub(self.last_proc_check) >= self.qpc_freq {
            self.last_proc_check = now;
            if let Some(p) = &self.sidecar {
                if !p.is_alive() {
                    self.sidecar = None;
                    self.sidecar_lost = true;
                    h.sidecar.set_state(SideState::Absent);
                    flags |= sc::FLAG_SIDECAR_LOST;
                }
            }
        }
        if self.sidecar_lost {
            flags |= sc::FLAG_SIDECAR_LOST;
        } else if h.sidecar.state() == Some(SideState::Ready) && h.sidecar.hb_age_s(now, h.qpc_freq.load(Acquire)).is_some_and(|a| a > 1.0) {
            flags |= sc::FLAG_SIDECAR_STALLED;
        }
        if h.prefix.refuse_code() != RefuseCode::None {
            flags |= sc::FLAG_REFUSED;
        }
        let rr = h.resync_req.load(Acquire);
        if rr != self.seen_resync {
            self.seen_resync = rr;
            flags |= sc::FLAG_RESYNC;
        }
        // state blobs / peer directory
        let gens = [
            // The session record slot (ABI 2): its seqlock seq is the change counter.
            s.state.session.seq() as u32,
            s.state.world_remote.published_gen(),
            s.state.world_owners.published_gen(),
            s.state.world_manifest.published_gen(),
            s.state.world_consistency.seq() as u32,
        ];
        if gens[0] != self.seen_state_gens[0] {
            flags |= sc::FLAG_SESSION_CHANGED;
        }
        if gens[1..] != self.seen_state_gens[1..] {
            flags |= sc::FLAG_STATE_CHANGED;
        }
        self.seen_state_gens = gens;
        let ds = s.peers.dir.seq();
        if ds != self.seen_dir_seq {
            self.seen_dir_seq = ds;
            flags |= sc::FLAG_PEERS_CHANGED;
        }
        self.dev_drain();
        let (n, resynced) = self.drain();
        if n > 0 {
            flags |= sc::FLAG_EVENTS;
        }
        if resynced {
            flags |= sc::FLAG_RESYNC;
        }
        self.last_flags = flags;
        flags
    }

    pub fn last_flags(&self) -> u32 {
        self.last_flags
    }

    /// Move every pending S2G record into the in-process event log.
    fn drain(&mut self) -> (usize, bool) {
        let Some(s) = self.seg() else { return (0, false) };
        if self.log.is_empty() {
            self.log = vec![Record::default(); EVENT_LOG];
        }
        let ep = s.header.sidecar.epoch.load(std::sync::atomic::Ordering::Acquire);
        let ring = s.s2g();
        s.header.game.high_water(ctr::RING_HIGH_WATER, ring.len());
        // Only a live, attached sidecar's records: `pop(0)` would accept any producer (epoch 0 =
        // no sidecar yet), and the epoch stays set after a death / detach. Records left in the
        // ring then wait; the next sidecar's epoch drops them as stale.
        let state = s.header.sidecar.state();
        if ep == 0 || self.sidecar_lost || matches!(state, Some(SideState::Closing) | Some(SideState::Absent) | None) {
            return (0, false);
        }
        let mut n = 0;
        let mut resynced = false;
        let mut rec = Record::default();
        for _ in 0..(2 * EVENT_LOG) {
            match ring.pop(ep, &mut rec) {
                Pop::Empty => break,
                Pop::Record => {
                    let i = (self.log_head % EVENT_LOG as u64) as usize;
                    self.log[i] = rec;
                    self.log_head += 1;
                    n += 1;
                    s.header.game.count(ctr::MSGS_IN, 1);
                }
                Pop::StaleEpoch => s.header.game.count(ctr::STALE_EPOCH, 1),
                Pop::Bad => s.header.game.count(ctr::BAD_RECORD, 1),
                Pop::Resynced => {
                    s.header.game.count(ctr::RESYNC, 1);
                    resynced = true;
                }
            }
        }
        (n, resynced)
    }

    // ---- world -------------------------------------------------------------------------

    pub unsafe fn world_leaving(&mut self, L: *mut lua_State) -> c_int {
        unsafe {
            use std::sync::atomic::Ordering::AcqRel;
            let Some(s) = self.seg() else { return nil_err(L, "not open") };
            let we = s.header.world_epoch.fetch_add(1, AcqRel) + 1;
            s.header.game.set_state(SideState::Loading);
            if let Ok((mut v, _)) = s.game_out.pose_lead.read() {
                v.meta.valid = 0;
                v.meta.world_epoch = we;
                s.game_out.pose_lead.write(&v);
            }
            self.invalidate_world_records(we);
            self.sample.drop_world(true);
            // world-scoped bus keys
            let n = (self.bus_dir.count as usize).min(BUS_KEYS);
            let mut clear = Vec::new();
            for i in 0..n {
                if self.bus_dir.world_scoped & (1u64 << i) != 0 {
                    clear.push(i);
                }
            }
            for i in clear {
                let mut meta = self.meta();
                meta.valid = 0;
                let b = self.scratch_as::<BusValue>();
                b.meta = meta;
                b.len = 0;
                b.kind = sc::bus::K_BUS as u32;
                let bp = b as *const BusValue;
                if let Some(s) = self.seg() {
                    s.bus.keys[i].write(&*bp);
                }
            }
            self.left_world = true;
            self.wrote = true;
            lua_pushboolean(L, 1);
            1
        }
    }

    pub unsafe fn world_ready(&mut self, L: *mut lua_State) -> c_int {
        unsafe {
            let Some(s) = self.seg() else { return nil_err(L, "not open") };
            s.header.game.set_state(SideState::Ready);
            self.sample.world_ready();
            if let Some(k) = arg_bytes(L, 1) {
                self.world_key = Some(k.to_vec());
                self.left_world = false;
            }
            lua_pushboolean(L, 1);
            1
        }
    }

    // ---- peers -------------------------------------------------------------------------

    /// Read the peer directory; `false` if absent / stale epoch.
    fn read_dir(&mut self) -> bool {
        let Some(s) = self.seg() else { return false };
        let ep = s.header.sidecar.epoch.load(std::sync::atomic::Ordering::Acquire);
        if ep == 0 || s.peers.dir.read_into(&mut self.dir).is_err() {
            return false;
        }
        self.dir.meta.writer_epoch == ep && self.dir.meta.valid != 0
    }

    pub unsafe fn peers(&mut self, L: *mut lua_State) -> c_int {
        unsafe {
            if !is_table(L, 1) {
                return nil_err(L, "bad");
            }
            let out = lua_absindex(L, 1);
            let mut n: i64 = 0;
            if self.read_dir() {
                let Some(s) = self.seg() else { return nil_err(L, "not open") };
                for (i, e) in self.dir.entries.iter().enumerate() {
                    if e.active != 1 {
                        continue;
                    }
                    n += 1;
                    if lua_rawgeti(L, out, n) != LUA_TTABLE {
                        pop(L, 1);
                        lua_createtable(L, 0, 8);
                        lua_pushvalue(L, -1);
                        lua_rawseti(L, out, n);
                    }
                    let t = lua_gettop(L);
                    let ps = &s.peers.slots[i];
                    set_int(L, t, "id", e.peer_id as i64);
                    set_int(L, t, "slot", i as i64);
                    set_int(L, t, "gen", e.gen as i64);
                    let nl = (e.nick_len as usize).min(e.nick.len());
                    let nick = String::from_utf8_lossy(&e.nick[..nl]);
                    set_str(L, t, "nick", &nick);
                    set_int(L, t, "rtt_ms", e.rtt_ms as i64);
                    set_int(L, t, "play_seq", ps.play.seq() as i64);
                    set_int(L, t, "root_seq", ps.root.seq() as i64);
                    set_int(L, t, "vitals_seq", ps.vitals.seq() as i64);
                    set_int(L, t, "kit_seq", ps.kit.seq() as i64);
                    pop(L, 1);
                }
            }
            trim_array(L, out, n);
            lua_pushinteger(L, n);
            1
        }
    }

    pub unsafe fn peer_play(&mut self, L: *mut lua_State) -> c_int {
        unsafe {
            let (Some(slot), true) = (arg_int(L, 1), is_table(L, 2)) else { return nil_err(L, "bad") };
            if slot < 0 || slot as usize >= MAX_PEER_SLOTS {
                return nil_err(L, "bad");
            }
            let Some(last) = opt_int(L, 3, -1) else { return nil_err(L, "bad") };
            let Some(s) = self.seg() else { return nil_err(L, "not open") };
            let ps = &s.peers.slots[slot as usize].play;
            let seq = ps.seq();
            if seq as i64 == last || seq == 0 {
                lua_pushnil(L);
                return 1;
            }
            let seq = match ps.read_into(&mut self.play) {
                Ok(v) => v,
                Err(_) => {
                    s.header.game.count(ctr::BUSY, 1);
                    lua_pushnil(L);
                    return 1;
                }
            };
            let ep = s.header.sidecar.epoch.load(std::sync::atomic::Ordering::Acquire);
            let p = &*self.play;
            if p.meta.writer_epoch != ep || p.meta.valid == 0 {
                lua_pushnil(L);
                return 1;
            }
            let out = lua_absindex(L, 2);
            set_int(L, out, "peer_id", p.peer_id as i64);
            set_int(L, out, "seq", p.play_seq as i64);
            let mode = PLAY_MODES.iter().find(|(_, v)| *v == p.mode).map_or("stale", |(n, _)| n);
            set_str(L, out, "mode", mode);
            set_int(L, out, "cut", p.cut as i64);
            set_num(L, out, "pt", p.pt);
            set_num(L, out, "age", p.age as f64);
            set_num(L, out, "delay", p.delay as f64);
            set_num(L, out, "jit", p.jit as f64);
            set_num(L, out, "lead", p.lead as f64);
            set_num(L, out, "st", p.st as f64);
            set_num(L, out, "iv", p.iv as f64);
            set_num(L, out, "k", p.k as f64);
            set_num(L, out, "rate", p.rate as f64);
            set_bool(L, out, "v2", p.flags & PEER_PLAY_V2 != 0);
            if p.flags & PEER_PLAY_HAS_ROOT != 0 {
                subtable(L, out, "_root", 4, 0);
                let t = lua_gettop(L);
                fill_array(L, t, p.root.iter().map(|&x| x as f64));
                rawset_str(L, out, "root");
            } else {
                // false, not nil: the key stays live, so re-adding it never rehashes (no GC churn).
                set_bool(L, out, "root", false);
            }
            set_int(L, out, "m", p.mask as i64);
            set_int(L, out, "vm", p.vmask as i64);
            subtable(L, out, "B", 13 * PLAY_SLOTS as c_int, 0);
            let t = lua_gettop(L);
            let mask = p.mask;
            fill_array(
                L,
                t,
                (0..PLAY_SLOTS).filter(|i| mask & (1 << i) != 0).flat_map(|i| p.b[i].iter().map(|&x| x as f64)),
            );
            pop(L, 1);
            subtable(L, out, "W", 16, 0);
            let t = lua_gettop(L);
            fill_array(
                L,
                t,
                (0..2usize)
                    .filter(|k| mask & (1 << (NB + k)) != 0 && p.w[*k].present != 0)
                    .flat_map(|k| {
                        let w = &p.w[k];
                        [w.hands as f64, w.class_id as f64, w.base[0] as f64, w.base[1] as f64, w.base[2] as f64, w.tip[0] as f64, w.tip[1] as f64, w.tip[2] as f64]
                    }),
            );
            pop(L, 1);
            if p.flags & PEER_PLAY_HAS_CONTROL != 0 {
                subtable(L, out, "_C", 37, 0);
                let t = lua_gettop(L);
                let c = &p.control;
                let it = [c.flags as f64, c.grip_r as f64, c.grip_l as f64]
                    .into_iter()
                    .chain(c.scalars.iter().map(|&x| x as f64))
                    .chain(c.aim.iter().map(|&x| x as f64))
                    .chain([c.ctrl_pitch as f64, c.ctrl_yaw as f64])
                    .chain(c.ik.iter().flat_map(|v| v.iter().map(|&x| x as f64)))
                    .chain([c.ik_world as f64]);
                fill_array(L, t, it);
                rawset_str(L, out, "C");
            } else {
                set_bool(L, out, "C", false);
            }
            s.header.game.count(ctr::SLOT_READS, 1);
            lua_pushinteger(L, seq as i64);
            1
        }
    }

    // ---- events ------------------------------------------------------------------------

    pub unsafe fn subscribe(&mut self, L: *mut lua_State) -> c_int {
        unsafe {
            let main = main_thread(L);
            let head = self.log_head;
            let filter = if is_table(L, 1) {
                let t = lua_absindex(L, 1);
                let n = lua_rawlen(L, t) as i64;
                let mut f = Vec::new();
                for i in 1..=n {
                    lua_rawgeti(L, t, i);
                    let k = arg_str(L, -1).and_then(|n| crate::records::subscribe_kind(&mut self.rec, n));
                    pop(L, 1);
                    match k {
                        Some(k) => f.push(k),
                        _ => return nil_err(L, "bad"),
                    }
                }
                Some(f)
            } else if lua_type(L, 1) <= LUA_TNIL {
                None
            } else {
                return nil_err(L, "bad");
            };
            self.cursors.entry(main).or_insert(Cursor { next: head, filter: None }).filter = filter;
            lua_pushboolean(L, 1);
            1
        }
    }

    pub unsafe fn poll(&mut self, L: *mut lua_State) -> c_int {
        unsafe {
            let (Some(max), true) = (opt_int(L, 1, 64), is_table(L, 2)) else { return nil_err(L, "bad") };
            let out = lua_absindex(L, 2);
            if self.seg().is_some() {
                self.drain();
            }
            let main = main_thread(L);
            let head = self.log_head;
            let oldest = head.saturating_sub(EVENT_LOG as u64);
            let mut cur = self.cursors.remove(&main).unwrap_or(Cursor { next: head, filter: None });
            let max = max.clamp(0, EVENT_LOG as i64);
            let mut n: i64 = 0;
            let entry = |L: *mut lua_State, n: i64| -> c_int {
                if lua_rawgeti(L, out, n) != LUA_TTABLE {
                    pop(L, 1);
                    lua_createtable(L, 0, 3);
                    lua_pushvalue(L, -1);
                    lua_rawseti(L, out, n);
                }
                lua_gettop(L)
            };
            if cur.next < oldest && n < max {
                n += 1;
                let t = entry(L, n);
                set_str(L, t, "kind", "resync");
                set_nil(L, t, "peer");
                set_nil(L, t, "aux");
                set_int(L, t, "req_id", 0);
                set_nil(L, t, "data");
                pop(L, 1);
                cur.next = oldest;
                self.count(ctr::RESYNC);
            }
            while n < max && cur.next < head {
                let rec = &self.log[(cur.next % EVENT_LOG as u64) as usize];
                cur.next += 1;
                if let Some(f) = &cur.filter {
                    if !f.contains(&rec.hdr.kind) {
                        continue;
                    }
                }
                n += 1;
                let t = entry(L, n);
                if !crate::records::fill_event(L, t, rec, &mut self.rec) {
                    if let Some(s) = self.seg() {
                        s.header.game.count(ctr::DECODE_ERR, 1);
                    }
                }
                pop(L, 1);
            }
            self.cursors.insert(main, cur);
            trim_array(L, out, n);
            lua_pushinteger(L, n);
            1
        }
    }
}
