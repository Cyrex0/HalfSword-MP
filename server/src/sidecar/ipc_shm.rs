//! `ShmLink`: the shared-memory game link (HSMP-SHM, docs/development/ipc-shared-memory.md).
//!
//! The game creates the segment and passes `--ipc shm:<name>`; this side opens it (never
//! creates), validates and attaches (`handshake::attach_sidecar`), then:
//! - one OS thread `hsmp-ipc` owns every sidecar-written slot, blob and the S2G ring, reads
//!   every game-written slot, blob and the G2S ring, beats the heartbeat and writes the tap.
//!   It waits on the g2s doorbell with a 1 ms timeout;
//! - `hsmp-poseplay` (pose.rs) is the single writer of the `PeerPlay` slots.
//!
//! Other threads hand data to `hsmp-ipc` through [`State`] (one short mutex per call), so
//! every primitive keeps exactly one writer thread.
//!
//! Every contract is a typed record: domain clients call [`ShmLink::post_record`]
//! (slots), [`ShmLink::push_record`] (S2G ring) and [`ShmLink::send_record`] (to the server);
//! G2S records go to `records_in`. No file is involved.

use std::collections::{HashMap, HashSet, VecDeque};
use std::io::Write as _;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use hsmp_ipc::handshake::{self, Refusal, SideParams};
use hsmp_ipc::header::{ctr, RefuseCode};
use hsmp_ipc::ring::{Pop, Record, MAX_PAYLOAD};
use hsmp_ipc::schema::pose::{PeerDir, PeerPlay, PoseLead, K_POSE, K_ROOT, K_WEAPON, MAX_PEER_SLOTS};
use hsmp_ipc::schema::session::*;
use hsmp_ipc::schema::{RawSlot, SlotMeta};
use hsmp_ipc::segment::Segment;
use hsmp_ipc::shm::{self, Access, Doorbell, Mapping};
use serde_json::Value as J;
use tracing::{debug, info, warn};

/// The streams this sidecar carries over shared memory.
pub const CAPS_OFFERED: u64 = hsmp_ipc::schema::CAP_POSE
    | hsmp_ipc::schema::CAP_STATE
    | hsmp_ipc::schema::CAP_VITALS
    | hsmp_ipc::schema::CAP_LOADOUT_KIT
    | hsmp_ipc::schema::CAP_WORLD
    | hsmp_ipc::schema::CAP_QUEUES
    | hsmp_ipc::schema::CAP_COMBAT
    | hsmp_ipc::schema::CAP_INTERACT;

/// S2G records kept in process while the ring is full.
pub const S2G_OVERFLOW: usize = 8192;
/// G2S `req_id` dedup window.
const DEDUP: usize = 512;
/// How long attach waits for the game's segment.
const ATTACH_WAIT: Duration = Duration::from_secs(2);

/// The record name of a v6 kind (tap / logs; `?` if unknown).
pub fn record_name(kind: u16) -> &'static str {
    hsmp_ipc::schema::record_info(kind).map_or("?", |r| r.name)
}

fn now_wall_ms() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis() as u64)
}

// ---- shared state between the callers and hsmp-ipc -----------------------------------------

#[derive(Default)]
struct State {
    /// S2G records waiting for ring space.
    s2g: VecDeque<S2gRec>,
    /// Sidecar-written record slots (ABI 2) waiting for the hsmp-ipc thread: (slot name,
    /// peer id for per-peer slots) -> (record kind, payload).
    rec_slots: HashMap<(&'static str, Option<u32>), (u16, Vec<u8>)>,
    /// Tap events from other threads.
    taps: VecDeque<(&'static str, J)>,
    too_big_logged: HashSet<u16>,
}

/// One S2G ring record waiting for space (the wire header fields + payload).
#[derive(Clone)]
struct S2gRec {
    kind: u16,
    aux: u16,
    peer: u32,
    bytes: Vec<u8>,
}

/// What the `hsmp-ipc` thread (and the domain clients) hand to the network forwarder.
pub enum Out {
    /// v6 record messages produced together (sent in one transmit, so they share datagrams).
    Msgs(Vec<Vec<u8>>),
}

struct Slots {
    by_peer: HashMap<u32, usize>,
    dir: PeerDir,
    released: [u64; MAX_PEER_SLOTS],
    clock: u64,
    nicks: HashMap<u32, String>,
    /// Peers of the last session roster (sync_roster frees those that leave it).
    roster: HashSet<u32>,
    dirty: bool,
}

impl Slots {
    fn new() -> Self {
        Slots { by_peer: HashMap::new(), dir: PeerDir::default(), released: [0; MAX_PEER_SLOTS], clock: 1, nicks: HashMap::new(), roster: HashSet::new(), dirty: true }
    }
    fn alloc(&mut self, peer: u32) -> Option<usize> {
        if peer == 0 {
            return None;
        }
        if let Some(s) = self.by_peer.get(&peer) {
            return Some(*s);
        }
        // The free slot released longest ago (a just-freed slot is reused last, so a
        // straggling write for the old peer is very unlikely to land in a reused slot).
        let s = (0..MAX_PEER_SLOTS).filter(|&i| self.dir.entries[i].active == 0).min_by_key(|&i| self.released[i])?;
        let e = &mut self.dir.entries[s];
        e.peer_id = peer;
        e.gen = e.gen.wrapping_add(1).max(1);
        e.active = 1;
        let nick = self.nicks.get(&peer).cloned().unwrap_or_else(|| format!("P{}", peer));
        e.set_nick(&nick);
        e.rtt_ms = 0;
        self.by_peer.insert(peer, s);
        self.dir.count = self.by_peer.len() as u32;
        self.dir.dir_gen = self.dir.dir_gen.wrapping_add(1);
        self.dirty = true;
        Some(s)
    }
    fn free(&mut self, peer: u32) -> Option<usize> {
        let s = self.by_peer.remove(&peer)?;
        self.dir.entries[s].active = 0;
        self.clock += 1;
        self.released[s] = self.clock;
        self.dir.count = self.by_peer.len() as u32;
        self.dir.dir_gen = self.dir.dir_gen.wrapping_add(1);
        self.dirty = true;
        Some(s)
    }
    /// The session roster's connected peers (excluding ourselves): every one gets a directory
    /// entry (lobby included) with the roster nick; a peer that was in an earlier roster and
    /// is gone now is freed. Peers seen only through their streams (not yet in a roster) are
    /// kept. Returns the freed peer ids.
    fn sync_roster(&mut self, peers: &[(u32, String)]) -> Vec<u32> {
        let now: HashSet<u32> = peers.iter().map(|p| p.0).collect();
        let gone: Vec<u32> = self.roster.iter().copied().filter(|p| !now.contains(p)).collect();
        for p in &gone {
            self.free(*p);
        }
        for (p, nick) in peers {
            self.nicks.insert(*p, nick.clone());
            if let Some(s) = self.alloc(*p) {
                if self.dir.entries[s].nick().as_ref() != nick.as_str() {
                    self.dir.entries[s].set_nick(nick);
                    self.dir.dir_gen = self.dir.dir_gen.wrapping_add(1);
                    self.dirty = true;
                }
            }
        }
        self.nicks.retain(|p, _| now.contains(p) || self.by_peer.contains_key(p));
        self.roster = now;
        gone
    }
    /// Server-measured RTTs (`pings`) into the directory entries.
    fn set_rtts(&mut self, rtts: &[(u32, u32)]) {
        for (p, rtt) in rtts {
            if let Some(&s) = self.by_peer.get(p) {
                if self.dir.entries[s].rtt_ms != *rtt {
                    self.dir.entries[s].rtt_ms = *rtt;
                    self.dirty = true;
                }
            }
        }
    }
    /// Free every peer (a new server session). Returns the freed ids.
    fn reset(&mut self) -> Vec<u32> {
        let all: Vec<u32> = self.by_peer.keys().copied().collect();
        for p in &all {
            self.free(*p);
        }
        self.roster.clear();
        self.nicks.clear();
        all
    }
}

/// The attached shared-memory link.
pub struct ShmLink {
    seg: &'static Segment,
    map_name: String,
    epoch: u64,
    st: Mutex<State>,
    slots: Mutex<Slots>,
    lead_bits: AtomicU32,
    play_seq: AtomicU32,
    stop: AtomicBool,
    stopped: AtomicBool,
    thread: Mutex<Option<std::thread::JoinHandle<()>>>,
    started: Instant,
    sample: AtomicU64,
    /// The forwarder (set by `start`): record messages from any thread.
    out_tx: OnceLock<tokio::sync::mpsc::Sender<Out>>,
}

/// What `attach` refused with.
pub type AttachError = Refusal;

static LINK: OnceLock<&'static ShmLink> = OnceLock::new();

/// The attached link, if `--ipc shm:` attached.
pub fn link() -> Option<&'static ShmLink> {
    LINK.get().copied()
}

/// The attached link if the hot streams (cap `POSE`) run over it.
pub fn pose_link() -> Option<&'static ShmLink> {
    link().filter(|l| l.has(hsmp_ipc::schema::CAP_POSE))
}

fn refusal(code: RefuseCode, detail: impl Into<String>) -> Refusal {
    Refusal { code, detail: detail.into() }
}

impl ShmLink {
    /// Open the game's segment and attach as the sidecar. Waits up to 2 s for
    /// the game to finish initialising it.
    pub fn attach(name: &str, parent_pid: Option<u32>) -> Result<&'static ShmLink, AttachError> {
        let parent = parent_pid.ok_or_else(|| refusal(RefuseCode::WrongParent, "--ipc shm requires --parent-pid"))?;
        let deadline = Instant::now() + ATTACH_WAIT;
        let map = loop {
            match Mapping::open(name, Access::ReadWrite) {
                Ok(m) => break m,
                Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied => {
                    return Err(refusal(RefuseCode::OwnerMismatch, e.to_string()));
                }
                Err(e) => {
                    if Instant::now() >= deadline {
                        return Err(refusal(RefuseCode::NotReady, format!("open {}: {}", name, e)));
                    }
                    std::thread::sleep(Duration::from_millis(20));
                }
            }
        };
        let map: &'static Mapping = Box::leak(Box::new(map));
        let seg = map.segment().ok_or_else(|| refusal(RefuseCode::SizeMismatch, format!("mapping is {} bytes", map.len())))?;
        let parent_ct = shm::process_create_time(parent).ok();
        let me = SideParams {
            pid: shm::current_pid(),
            create_time: shm::process_create_time(shm::current_pid()).unwrap_or(0),
            epoch: shm::random_u64(),
            caps: CAPS_OFFERED,
            build_id: concat!("hsmp-sidecar ", env!("CARGO_PKG_VERSION")),
        };
        let att = loop {
            match handshake::attach_sidecar(seg, map.len(), parent, parent_ct, &me) {
                Ok(a) => break a,
                Err(e) if e.code == RefuseCode::NotReady && Instant::now() < deadline => {
                    std::thread::sleep(Duration::from_millis(20));
                }
                Err(e) => return Err(e),
            }
        };
        info!(
            name, caps_effective = format!("{:#x}", att.caps_effective), game_epoch = format!("{:016x}", att.game_epoch),
            epoch = format!("{:016x}", me.epoch), attach_count = att.attach_count, "ipc: attached to the game's shared segment"
        );
        let l: &'static ShmLink = Box::leak(Box::new(ShmLink {
            seg,
            map_name: name.to_string(),
            epoch: me.epoch,
            st: Mutex::new(State::default()),
            slots: Mutex::new(Slots::new()),
            lead_bits: AtomicU32::new(0),
            play_seq: AtomicU32::new(0),
            stop: AtomicBool::new(false),
            stopped: AtomicBool::new(false),
            thread: Mutex::new(None),
            started: Instant::now(),
            sample: AtomicU64::new(0),
            out_tx: OnceLock::new(),
        }));
        let _ = LINK.set(l);
        Ok(l)
    }

    /// An in-process link over an already initialised segment (tests).
    #[cfg(test)]
    pub fn for_test(seg: &'static Segment, epoch: u64) -> &'static ShmLink {
        Box::leak(Box::new(ShmLink {
            seg,
            map_name: "<test>".into(),
            epoch,
            st: Mutex::new(State::default()),
            slots: Mutex::new(Slots::new()),
            lead_bits: AtomicU32::new(0),
            play_seq: AtomicU32::new(0),
            stop: AtomicBool::new(false),
            stopped: AtomicBool::new(false),
            thread: Mutex::new(None),
            started: Instant::now(),
            sample: AtomicU64::new(0),
            out_tx: OnceLock::new(),
        }))
    }

    /// The process-wide test link: an in-process segment (every cap negotiated) attached
    /// and installed as THE game link, so the modules' record calls reach shared memory in
    /// unit tests. The `hsmp-ipc` thread is not started: S2G records stay queued
    /// ([`ShmLink::test_records`]) and posted slots are readable with [`ShmLink::test_slot`].
    #[cfg(test)]
    pub fn test_global() -> &'static ShmLink {
        static L: OnceLock<&'static ShmLink> = OnceLock::new();
        L.get_or_init(|| {
            let m: &'static Mapping = Box::leak(Box::new(Mapping::anonymous()));
            let p = SideParams { pid: 4242, create_time: 1, epoch: 0x7e57_0001, caps: CAPS_OFFERED, build_id: "test-game" };
            // SAFETY: a fresh, exclusively owned in-process mapping.
            unsafe { handshake::init_game(m.segment_ptr(), m.len(), &p, 10_000_000) }.expect("init test segment");
            let seg = m.segment().expect("segment");
            let sc = SideParams { pid: 1, create_time: 2, epoch: 0x7e57_5c01, caps: CAPS_OFFERED, build_id: "test-sidecar" };
            handshake::attach_sidecar(seg, m.len(), 4242, Some(1), &sc).expect("attach test segment");
            let l = ShmLink::for_test(seg, 0x7e57_5c01);
            let _ = LINK.set(l);
            l
        })
    }

    /// Tests that use the process-wide test link and its peer slots (roster sync frees slots,
    /// session resets clear dedup tables): one at a time.
    #[cfg(test)]
    pub fn test_lock() -> std::sync::MutexGuard<'static, ()> {
        static L: Mutex<()> = Mutex::new(());
        L.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Tests: the queued typed S2G records of `kind` (aux, peer, payload), removed.
    #[cfg(test)]
    pub fn test_records(&self, kind: u16) -> Vec<(u16, u32, Vec<u8>)> {
        let mut st = self.lock();
        let mut out = Vec::new();
        st.s2g.retain(|r| {
            if r.kind != kind {
                return true;
            }
            out.push((r.aux, r.peer, r.bytes.clone()));
            false
        });
        out
    }

    /// Tests: the pending sidecar-written record slot `slot` (peer `peer`), if any.
    #[cfg(test)]
    pub fn test_slot(&self, slot: &'static str, peer: Option<u32>) -> Option<(u16, Vec<u8>)> {
        self.lock().rec_slots.get(&(slot, peer)).cloned()
    }

    pub fn segment(&self) -> &'static Segment {
        self.seg
    }

    /// Was `cap` negotiated (`game.caps & sidecar.caps`)? A stream whose cap is not
    /// negotiated is not carried: the link layer drops it and logs it once.
    pub fn has(&self, cap: u64) -> bool {
        self.seg.header.caps_effective.load(Ordering::Acquire) & cap != 0
    }

    #[allow(dead_code)]
    pub fn epoch(&self) -> u64 {
        self.epoch
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, State> {
        self.st.lock().unwrap_or_else(|e| e.into_inner())
    }
    fn slots(&self) -> std::sync::MutexGuard<'_, Slots> {
        self.slots.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// A `SlotMeta` for data this side writes now.
    pub fn meta(&self) -> SlotMeta {
        let h = &self.seg.header;
        SlotMeta {
            writer_epoch: self.epoch,
            world_epoch: h.world_epoch.load(Ordering::Acquire),
            session_epoch: h.session_epoch.load(Ordering::Acquire),
            valid: 1,
            sample_seq: self.sample.fetch_add(1, Ordering::Relaxed) as u32,
            t_us: self.started.elapsed().as_micros() as u64,
        }
    }

    /// The game's requested pose look-ahead (ms), from `PoseLead`.
    pub fn pose_lead(&self) -> f64 {
        f32::from_bits(self.lead_bits.load(Ordering::Relaxed)) as f64
    }

    /// The peer slot of `peer` (assigned on first use). For the `hsmp-poseplay` thread.
    pub fn slot_for(&self, peer: u32) -> Option<usize> {
        self.slots().alloc(peer)
    }

    /// Write one peer's playback sample. Only the `hsmp-poseplay` thread calls this.
    pub fn write_play(&self, slot: usize, p: &PeerPlay) {
        if let Some(s) = self.seg.peers.slots.get(slot) {
            s.play.write(p);
            self.play_seq.fetch_add(1, Ordering::Relaxed);
        }
    }

    /// Every new server session (Welcome): `session_epoch + 1`.
    pub fn on_welcome(&self) {
        self.seg.header.session_epoch.fetch_add(1, Ordering::AcqRel);
    }

    fn drop_peer_state(&self, peers: &[u32]) {
        if peers.is_empty() {
            return;
        }
        let mut st = self.lock();
        st.rec_slots.retain(|(_, p), _| p.map_or(true, |p| !peers.contains(&p)));
    }

    /// The session roster's connected peers (ourselves excluded) as (peer id, nick): ONE
    /// place for the peer directory's membership (lobby included). Peers that left the roster
    /// are freed with their per-peer state.
    pub fn sync_roster(&self, peers: &[(u32, String)]) {
        let gone = self.slots().sync_roster(peers);
        self.drop_peer_state(&gone);
    }

    /// Every peer's server-measured RTT (ms; 0 = unknown) into its directory entry.
    pub fn set_rtts(&self, rtts: &[(u32, u32)]) {
        self.slots().set_rtts(rtts);
    }

    /// A new server session (non-resume Welcome, new epoch): every peer slot is freed.
    pub fn reset_peers(&self) {
        let all = self.slots().reset();
        self.drop_peer_state(&all);
    }

    fn count(&self, idx: usize, n: u64) {
        self.seg.header.sidecar.count(idx, n);
    }

    // ---- typed records (ABI 2): any thread ----------------------------------------------------

    /// Queue one v6 record message to the server (`wire::message` framed). Messages queued
    /// by one ipc step go out in one transmit. Dropped (and counted) if the forwarder is full.
    pub fn send_record(&self, msg: Vec<u8>) {
        self.send_records(vec![msg]);
    }

    /// Queue several record messages to go out together.
    pub fn send_records(&self, msgs: Vec<Vec<u8>>) {
        if msgs.is_empty() {
            return;
        }
        match self.out_tx.get() {
            Some(tx) => {
                if tx.try_send(Out::Msgs(msgs)).is_err() {
                    self.count(ctr::OVERFLOW, 1);
                }
            }
            None => self.count(ctr::OVERFLOW, 1),
        }
    }

    /// Publish a sidecar-written record slot (`schema::SlotInfo` name; `peer` for per-peer
    /// slots, which allocates the peer's table slot). The hsmp-ipc thread writes it on its next
    /// step (the slot's single writer). The payload is the record as received / built.
    pub fn post_record(&self, slot: &'static str, peer: Option<u32>, kind: u16, payload: &[u8]) {
        if let Some(p) = peer {
            if self.slots().alloc(p).is_none() {
                return;
            }
        }
        self.lock().rec_slots.insert((slot, peer), (kind, payload.to_vec()));
    }

    /// Queue one typed S2G ring record (the wire header fields + payload), e.g. an S2C record
    /// the game must see as an event, copied as it is.
    pub fn push_record(&self, kind: u16, aux: u16, peer: u32, payload: &[u8]) {
        if payload.len() > MAX_PAYLOAD {
            self.count(ctr::TOO_BIG, 1);
            let mut st = self.lock();
            if st.too_big_logged.insert(kind) {
                warn!(kind, len = payload.len(), "ipc: record over the ring payload dropped (schema error)");
            }
            return;
        }
        let mut st = self.lock();
        if st.s2g.len() >= S2G_OVERFLOW {
            drop(st);
            self.count(ctr::OVERFLOW, 1);
            self.seg.header.resync_req.fetch_add(1, Ordering::AcqRel);
            return;
        }
        st.s2g.push_back(S2gRec { kind, aux, peer, bytes: payload.to_vec() });
        if st.taps.len() < 4096 {
            st.taps.push_back(("s2g", serde_json::json!({"kind": record_name(kind), "kind_id": kind, "peer": peer, "aux": aux,
                "v": hsmp_ipc::debug_json::record_to_json(kind, payload)})));
        }
    }

    // ---- the hsmp-ipc thread ----------------------------------------------------------------

    /// Start the `hsmp-ipc` thread. Record messages (root / weapon / pose of one step together) go to `tx`.
    pub fn start(&'static self, tx: tokio::sync::mpsc::Sender<Out>, tap: Option<std::path::PathBuf>) {
        let _ = self.out_tx.set(tx);
        let h = std::thread::Builder::new()
            .name("hsmp-ipc".into())
            .spawn(move || {
                let tap = tap.and_then(|p| match Tap::open(&p) {
                    Ok(t) => Some(t),
                    Err(e) => {
                        warn!(error = %e, path = ?p, "ipc: cannot open the tap");
                        None
                    }
                });
                let mut w = Worker::new(self, tap);
                w.attach_tap();
                while !self.stop.load(Ordering::Acquire) {
                    w.step();
                    w.wait();
                }
                w.step();
                w.finish();
                self.stopped.store(true, Ordering::Release);
            })
            .expect("spawn the hsmp-ipc thread");
        *self.thread.lock().unwrap_or_else(|e| e.into_inner()) = Some(h);
    }

    /// Final `link` record (status ENDED), last publish, detach. Waits up to `timeout` for
    /// the thread.
    pub fn shutdown(&self, final_link: Option<&[u8]>, timeout: Duration) {
        let running = self.thread.lock().map_or(false, |g| g.is_some()) && !self.stopped.load(Ordering::Acquire);
        if let Some(p) = final_link {
            if running {
                // The hsmp-ipc thread publishes it in its last step (the slot's single writer).
                self.post_record("link", None, K_LINK, p);
            } else if let Some(slot) = self.seg.slot_ref("link", 0) {
                // No writer thread (tests, a stuck thread): this thread is the only writer now.
                slot.put(self.meta(), K_LINK, p, &mut Vec::new());
            }
        }
        self.stop.store(true, Ordering::Release);
        let t0 = Instant::now();
        while !self.stopped.load(Ordering::Acquire) && t0.elapsed() < timeout {
            if self.thread.lock().map_or(true, |g| g.is_none()) {
                break;
            }
            std::thread::sleep(Duration::from_millis(2));
        }
        if !self.stopped.load(Ordering::Acquire) {
            // No thread (or it is stuck): publish from here so the game sees the end state.
            handshake::detach_sidecar(self.seg);
        }
    }
}

// ---- tap -----------------------------------------------------------------------------------

struct Tap {
    w: std::io::BufWriter<std::fs::File>,
    dirty: bool,
}

impl Tap {
    fn open(p: &std::path::Path) -> std::io::Result<Tap> {
        if let Some(d) = p.parent() {
            let _ = std::fs::create_dir_all(d);
        }
        let f = std::fs::OpenOptions::new().create(true).append(true).open(p)?;
        Ok(Tap { w: std::io::BufWriter::new(f), dirty: false })
    }
    fn ev(&mut self, ev: &str, mut fields: J) {
        if !fields.is_object() {
            fields = serde_json::json!({ "v": fields });
        }
        let o = fields.as_object_mut().expect("object");
        o.insert("t".into(), J::from(now_wall_ms()));
        o.insert("ev".into(), J::from(ev));
        let _ = writeln!(self.w, "{}", fields);
        self.dirty = true;
    }
    fn flush(&mut self) {
        if self.dirty {
            let _ = self.w.flush();
            self.dirty = false;
        }
    }
}

/// Write a refusal to the tap file (before any link exists).
pub fn tap_refusal(p: &std::path::Path, r: &Refusal) {
    if let Ok(mut t) = Tap::open(p) {
        t.ev("refuse", serde_json::json!({"code": r.code.as_str(), "detail": r.detail}));
        t.flush();
    }
}

// ---- worker ----------------------------------------------------------------------------------

#[derive(Default)]
struct Seen {
    /// Last version and tick of local_root, local_weapon, local_pose.
    hot: [u64; 3],
    hot_tick: [Option<u32>; 3],
    lead: u64,
    world_epoch: u32,
}

struct Worker {
    l: &'static ShmLink,
    tap: Option<Tap>,
    bell: Option<Doorbell>,
    bell_try: Option<Instant>,
    seen: Seen,
    dedup: HashSet<u64>,
    dedup_q: VecDeque<u64>,
    rec: Box<Record>,
    /// Scratch for typed record slot writes (`RawSlot::put`).
    scratch: Vec<u64>,
    /// The payload of the hot slot being read.
    hot_buf: Vec<u8>,
    // Preallocated blob buffers (never on the stack).
    /// The latest publish of each record slot not yet tapped (emitted at most 4 Hz).
    slot_taps: HashMap<&'static str, (u16, Vec<u8>)>,
    last_slot_tap: Option<Instant>,
    last_play_tap: Instant,
    last_stats: Instant,
    last_flush: Instant,
}


impl Worker {
    fn new(l: &'static ShmLink, tap: Option<Tap>) -> Worker {
        Worker {
            l,
            tap,
            bell: None,
            bell_try: None,
            seen: Seen::default(),
            dedup: HashSet::new(),
            dedup_q: VecDeque::new(),
            rec: Box::default(),
            scratch: Vec::new(),
            hot_buf: Vec::with_capacity(1024),
            slot_taps: HashMap::new(),
            last_slot_tap: None,
            last_play_tap: Instant::now(),
            last_stats: Instant::now(),
            last_flush: Instant::now(),
        }
    }

    fn attach_tap(&mut self) {
        let h = &self.l.seg.header;
        if let Some(t) = self.tap.as_mut() {
            t.ev(
                "attach",
                serde_json::json!({
                    "name": self.l.map_name, "abi": format!("{}.{}", hsmp_ipc::ABI_MAJOR, hsmp_ipc::ABI_MINOR),
                    "layout_hash": format!("{:016x}", hsmp_ipc::LAYOUT_HASH),
                    "caps_game": h.game.caps.load(Ordering::Acquire), "caps_sidecar": CAPS_OFFERED,
                    "caps_effective": h.caps_effective.load(Ordering::Acquire),
                    "game_pid": h.game.pid.load(Ordering::Acquire),
                    "game_epoch": format!("{:016x}", h.game.epoch.load(Ordering::Acquire)),
                    "sidecar_epoch": format!("{:016x}", self.l.epoch),
                }),
            );
            t.flush();
        }
    }

    fn wait(&mut self) {
        if self.bell.is_none() && self.bell_try.map_or(true, |t| t.elapsed() > Duration::from_secs(1)) {
            self.bell_try = Some(Instant::now());
            let (g2s, _) = shm::doorbell_names(&self.l.map_name);
            self.bell = Doorbell::open(&g2s).ok();
        }
        match &self.bell {
            Some(b) => {
                b.wait(1);
            }
            None => std::thread::sleep(Duration::from_millis(1)),
        }
    }

    /// One loop: heartbeat, inbound, outbound, tap.
    fn step(&mut self) {
        let seg = self.l.seg;
        seg.header.sidecar.beat(shm::qpc());
        self.seen.world_epoch = seg.header.world_epoch.load(Ordering::Acquire);
        self.inbound_hot();
        self.inbound_ring();
        self.outbound();
        self.taps();
    }

    fn fresh(&self, m: &SlotMeta, scoped: bool) -> bool {
        let h = &self.l.seg.header;
        m.valid == 1 && m.writer_epoch == h.game.epoch.load(Ordering::Acquire) && (!scoped || m.world_epoch == self.seen.world_epoch)
    }

    fn inbound_hot(&mut self) {
        if !self.l.has(hsmp_ipc::schema::CAP_POSE) {
            return;
        }
        // One pump step: root, weapon and pose of one sample go out together (one transmit,
        // so they share a datagram where they fit). The payloads are the records exactly as
        // the game wrote them; only the 8-byte wire header is added.
        let go = &self.l.seg.game_out;
        let mut msgs: Vec<Vec<u8>> = Vec::new();
        for (slot, kind) in [
            (&go.local_root as &dyn RawSlot, K_ROOT),
            (&go.local_weapon as &dyn RawSlot, K_WEAPON),
            (&go.local_pose as &dyn RawSlot, K_POSE),
        ] {
            if let Some(m) = self.hot_record(slot, kind) {
                msgs.push(m);
            }
        }
        self.l.send_records(msgs);
        let mut lead = PoseLead::default();
        if let Ok(Some(s)) = go.pose_lead.read_if_changed(self.seen.lead, &mut lead) {
            self.seen.lead = s;
            let v = if self.fresh(&lead.meta, false) && lead.lead_ms.is_finite() { lead.lead_ms.max(0.0) } else { 0.0 };
            self.l.lead_bits.store(v.to_bits(), Ordering::Relaxed);
        }
    }

    /// A changed hot slot as a framed message: fresh (this game instance and world), a new
    /// sample (every hot record leads with its u32 `tick`) and a valid record. The bytes are
    /// copied as they are.
    fn hot_record(&mut self, slot: &dyn RawSlot, kind: u16) -> Option<Vec<u8>> {
        let i = match kind { K_ROOT => 0, K_WEAPON => 1, _ => 2 };
        let ver = slot.version();
        if ver == self.seen.hot[i] {
            return None;
        }
        let (meta, k, ver) = slot.get(&mut self.scratch, &mut self.hot_buf)?;
        self.seen.hot[i] = ver;
        if k != kind || !self.fresh(&meta, true) || self.hot_buf.len() < 4 {
            return None;
        }
        let tick = u32::from_le_bytes([self.hot_buf[0], self.hot_buf[1], self.hot_buf[2], self.hot_buf[3]]);
        if self.seen.hot_tick[i] == Some(tick) {
            return None;
        }
        self.seen.hot_tick[i] = Some(tick);
        if let Err(e) = hsmp_ipc::schema::check_payload(kind, &self.hot_buf) {
            self.l.count(ctr::BAD_RECORD, 1);
            debug!(kind, error = %e, "ipc: invalid hot record from the game dropped");
            return None;
        }
        if kind == K_POSE {
            super::pose::note_tx(&self.hot_buf);
        }
        Some(hsmp_ipc::wire::message(kind, 0, 0, &self.hot_buf))
    }

    fn inbound_ring(&mut self) {
        let seg = self.l.seg;
        let ring = seg.g2s();
        let ge = seg.header.game.epoch.load(Ordering::Acquire);
        for _ in 0..256 {
            match ring.pop(ge, &mut self.rec) {
                Pop::Empty => break,
                Pop::StaleEpoch => self.l.count(ctr::STALE_EPOCH, 1),
                Pop::Bad => self.l.count(ctr::BAD_RECORD, 1),
                Pop::Resynced => self.l.count(ctr::RESYNC, 1),
                Pop::Record => {
                    self.l.count(ctr::MSGS_IN, 1);
                    let (kind, req) = (self.rec.hdr.kind, self.rec.hdr.req_id);
                    if req != 0 {
                        if !self.dedup.insert(req) {
                            continue;
                        }
                        self.dedup_q.push_back(req);
                        if self.dedup_q.len() > DEDUP {
                            if let Some(o) = self.dedup_q.pop_front() {
                                self.dedup.remove(&o);
                            }
                        }
                    }
                    // ABI 2: a typed record goes to its domain as it is (validated first).
                    if hsmp_ipc::schema::record_info(kind).is_some() {
                        let payload = self.rec.payload();
                        if let Err(e) = hsmp_ipc::schema::check_payload(kind, payload) {
                            self.l.count(ctr::BAD_RECORD, 1);
                            debug!(kind, error = %e, "ipc: invalid G2S record dropped");
                            continue;
                        }
                        if let Some(t) = self.tap.as_mut() {
                            t.ev("g2s", serde_json::json!({"kind": record_name(kind), "kind_id": kind, "req_id": req,
                                "v": hsmp_ipc::debug_json::record_to_json(kind, payload)}));
                        }
                        let hdr = self.rec.hdr;
                        super::records_in::on_g2s(self.l, &hdr, payload);
                        continue;
                    }
                    // Not a record kind: every G2S message is a typed record now.
                    self.l.count(ctr::BAD_RECORD, 1);
                    debug!(kind, "ipc: unknown G2S kind");
                }
            }
        }
    }

    fn outbound(&mut self) {
        let l = self.l;
        let seg = l.seg;
        // Take everything pending in one short lock.
        let (mut events, rec_slots) = {
            let mut st = l.lock();
            (std::mem::take(&mut st.s2g), std::mem::take(&mut st.rec_slots))
        };
        // ABI 2 record slots: the payload as it is, behind a fresh meta.
        for ((name, peer), (kind, payload)) in rec_slots {
            let idx = match peer {
                Some(p) => match l.slots().by_peer.get(&p).copied() {
                    Some(s) => s,
                    None => continue,
                },
                None => 0,
            };
            match seg.slot_ref(name, idx) {
                Some(slot) => {
                    if slot.put(l.meta(), kind, &payload, &mut self.scratch) {
                        l.count(ctr::SLOT_WRITES, 1);
                        if self.tap.is_some() {
                            self.slot_taps.insert(name, (kind, payload.clone()));
                        }
                    } else {
                        l.count(ctr::TOO_BIG, 1);
                        warn!(slot = name, len = payload.len(), "ipc: record over its slot capacity; not published");
                    }
                }
                None => {
                    l.count(ctr::BAD_RECORD, 1);
                    debug!(slot = name, "ipc: no such record slot");
                }
            }
        }
        {
            let mut sl = l.slots();
            if std::mem::take(&mut sl.dirty) {
                let mut d = sl.dir;
                d.meta = l.meta();
                seg.peers.dir.write(&d);
            }
        }
        // S2G: push in order; whatever does not fit waits (in order) for the next loop.
        let ring = seg.s2g();
        while let Some(r) = events.front() {
            match ring.push_msg(l.epoch, r.kind, r.aux, r.peer, 0, 0, &r.bytes) {
                Ok(_) => {
                    l.count(ctr::MSGS_OUT, 1);
                    events.pop_front();
                }
                Err(hsmp_ipc::ring::PushError::Full) => {
                    l.count(ctr::RING_FULL, 1);
                    break;
                }
                Err(_) => {
                    l.count(ctr::BAD_RECORD, 1);
                    events.pop_front();
                }
            }
        }
        seg.header.sidecar.high_water(ctr::RING_HIGH_WATER, ring.len());
        if !events.is_empty() {
            let mut st = l.lock();
            // Older records first, then whatever arrived meanwhile.
            while let Some(e) = events.pop_back() {
                st.s2g.push_front(e);
            }
        }
    }

    fn taps(&mut self) {
        let l = self.l;
        let pending: Vec<(&'static str, J)> = l.lock().taps.drain(..).collect();
        let Some(t) = self.tap.as_mut() else {
            if self.last_stats.elapsed() >= Duration::from_secs(5) {
                self.last_stats = Instant::now();
                log_stats(l);
            }
            return;
        };
        for (ev, v) in pending {
            t.ev(ev, v);
        }
        if !self.slot_taps.is_empty() && self.last_slot_tap.map_or(true, |t| t.elapsed() >= Duration::from_millis(250)) {
            self.last_slot_tap = Some(Instant::now());
            for (name, (kind, p)) in self.slot_taps.drain() {
                let rec = hsmp_ipc::schema::record_info(kind).map_or("?", |r| r.name);
                t.ev("slot", serde_json::json!({"slot": name, "kind": rec, "v": hsmp_ipc::debug_json::record_to_json(kind, &p)}));
            }
        }
        if self.last_play_tap.elapsed() >= Duration::from_secs(1) {
            self.last_play_tap = Instant::now();
            let dir = l.slots().dir;
            for (i, e) in dir.entries.iter().enumerate() {
                if e.active != 1 {
                    continue;
                }
                if let Ok((p, _)) = l.seg.peers.slots[i].play.read() {
                    let mode = hsmp_ipc::schema::pose::PLAY_MODES.iter().find(|m| m.1 == p.mode).map_or("?", |m| m.0);
                    t.ev("play", serde_json::json!({"peer": e.peer_id, "slot": i, "mode": mode, "play_seq": p.play_seq, "age": p.age, "delay": p.delay}));
                }
            }
        }
        if self.last_stats.elapsed() >= Duration::from_secs(5) {
            self.last_stats = Instant::now();
            let h = &l.seg.header;
            let names = ctr::NAMES;
            let m = |b: &hsmp_ipc::header::SideBlock| -> J {
                J::Object(names.iter().enumerate().map(|(i, n)| ((*n).to_string(), J::from(b.counter(i)))).collect())
            };
            t.ev("stats", serde_json::json!({"game": m(&h.game), "sidecar": m(&h.sidecar), "g2s_len": l.seg.g2s().len(), "s2g_len": l.seg.s2g().len()}));
            log_stats(l);
        }
        if self.last_flush.elapsed() >= Duration::from_millis(100) {
            self.last_flush = Instant::now();
            t.flush();
        }
    }

    fn finish(&mut self) {
        handshake::detach_sidecar(self.l.seg);
        if let Some(t) = self.tap.as_mut() {
            t.ev("detach", serde_json::json!({"reason": "exit"}));
            t.flush();
        }
    }
}

fn log_stats(l: &ShmLink) {
    let sb = &l.seg.header.sidecar;
    info!(
        msgs_in = sb.counter(ctr::MSGS_IN), msgs_out = sb.counter(ctr::MSGS_OUT), ring_full = sb.counter(ctr::RING_FULL),
        overflow = sb.counter(ctr::OVERFLOW), too_big = sb.counter(ctr::TOO_BIG), plays = l.play_seq.load(Ordering::Relaxed),
        "ipc stats"
    );
}

/// The forwarder for the record messages of the `hsmp-ipc` thread (newest-wins streams; a full
/// channel drops).
pub fn spawn_forwarder(
    sock: &std::sync::Arc<tokio::net::UdpSocket>,
    shared: &std::sync::Arc<tokio::sync::Mutex<super::SharedState>>,
) -> (tokio::sync::mpsc::Sender<Out>, tokio::task::JoinHandle<()>) {
    let (tx, mut rx) = tokio::sync::mpsc::channel::<Out>(256);
    let sock = sock.clone();
    let _ = shared;
    let h = tokio::spawn(async move {
        while let Some(o) = rx.recv().await {
            let r = match o {
                Out::Msgs(m) => super::send_msgs(&sock, m).await,
            };
            if let Err(e) = r {
                warn!(error = %e, "ipc: send failed");
            }
        }
        std::future::pending::<()>().await
    });
    (tx, h)
}

#[cfg(test)]
#[path = "ipc_shm_tests.rs"]
mod tests;
