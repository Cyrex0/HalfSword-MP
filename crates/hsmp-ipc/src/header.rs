//! Segment header: a frozen 128-byte prefix that never changes across ABI
//! versions, then one block per side (each written only by its owner), then shared epochs.

use core::sync::atomic::{AtomicU32, AtomicU64, Ordering};

pub const MAGIC: [u8; 8] = *b"HSMPSHM\0";
pub const PREFIX_SIZE: u16 = 128;

/// `HeaderPrefix::init_state`.
pub mod init_state {
    pub const ZERO: u32 = 0;
    pub const INITIALISING: u32 = 1;
    pub const READY: u32 = 2;
    pub const POISONED: u32 = 3;
}

/// `SideBlock::state`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
pub enum SideState {
    Absent = 0,
    Starting = 1,
    Ready = 2,
    /// Game only: between `world_leaving` and `world_ready`.
    Loading = 3,
    Closing = 4,
}

impl SideState {
    pub fn from_u32(v: u32) -> Option<Self> {
        Some(match v {
            0 => Self::Absent,
            1 => Self::Starting,
            2 => Self::Ready,
            3 => Self::Loading,
            4 => Self::Closing,
            _ => return None,
        })
    }
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Absent => "absent",
            Self::Starting => "starting",
            Self::Ready => "ready",
            Self::Loading => "loading",
            Self::Closing => "closing",
        }
    }
}

/// Why a side refused the segment (`HeaderPrefix::refuse_code`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
pub enum RefuseCode {
    None = 0,
    /// Not an HSMP segment.
    BadMagic = 1,
    /// ABI major or layout hash differs (and is not in `COMPAT_HASHES`).
    AbiMismatch = 2,
    /// `game.pid` / `game.create_time` do not match the sidecar's `--parent-pid`.
    WrongParent = 3,
    /// The game never reached `init_state == READY` within the attach timeout.
    NotReady = 4,
    /// The mapping's owner SID is not the current user.
    OwnerMismatch = 5,
    /// Mapped size smaller than the header claims.
    SizeMismatch = 6,
    /// `init_state == POISONED` (a native panic disabled IPC).
    Poisoned = 7,
    /// Another sidecar is attached and alive.
    Busy = 8,
}

impl RefuseCode {
    pub fn from_u32(v: u32) -> Option<Self> {
        Some(match v {
            0 => Self::None,
            1 => Self::BadMagic,
            2 => Self::AbiMismatch,
            3 => Self::WrongParent,
            4 => Self::NotReady,
            5 => Self::OwnerMismatch,
            6 => Self::SizeMismatch,
            7 => Self::Poisoned,
            8 => Self::Busy,
            _ => return None,
        })
    }
    pub fn as_str(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::BadMagic => "bad_magic",
            Self::AbiMismatch => "abi_mismatch",
            Self::WrongParent => "wrong_parent",
            Self::NotReady => "not_ready",
            Self::OwnerMismatch => "owner_mismatch",
            Self::SizeMismatch => "size_mismatch",
            Self::Poisoned => "poisoned",
            Self::Busy => "busy",
        }
    }
    /// Sidecar process exit code for this refusal (70..=78).
    pub fn exit_code(self) -> i32 {
        match self {
            Self::None => 0,
            other => 69 + other as i32,
        }
    }
}

pub const REFUSE_CODES: &[RefuseCode] = &[
    RefuseCode::None, RefuseCode::BadMagic, RefuseCode::AbiMismatch, RefuseCode::WrongParent, RefuseCode::NotReady,
    RefuseCode::OwnerMismatch, RefuseCode::SizeMismatch, RefuseCode::Poisoned, RefuseCode::Busy,
];

/// `SideBlock::counters` indices: diagnostics only, each side counts its own.
pub mod ctr {
    pub const TORN_RETRY: usize = 0;
    pub const BUSY: usize = 1;
    pub const RING_FULL: usize = 2;
    pub const OVERFLOW: usize = 3;
    pub const RESYNC: usize = 4;
    pub const STALE_EPOCH: usize = 5;
    pub const BAD_RECORD: usize = 6;
    pub const WRONG_THREAD: usize = 7;
    pub const DECODE_ERR: usize = 8;
    pub const TOO_BIG: usize = 9;
    pub const MSGS_OUT: usize = 10;
    pub const MSGS_IN: usize = 11;
    pub const SLOT_WRITES: usize = 12;
    pub const SLOT_READS: usize = 13;
    pub const RING_HIGH_WATER: usize = 14;
    pub const PANICS: usize = 15;
    pub const NAMES: [&str; 16] = [
        "torn_retry", "busy", "ring_full", "overflow", "resync", "stale_epoch", "bad_record", "wrong_thread",
        "decode_err", "too_big", "msgs_out", "msgs_in", "slot_writes", "slot_reads", "ring_high_water", "panics",
    ];
}

crate::ipc_layout! {
    /// FROZEN FOREVER: 128 bytes, identical in every ABI version.
    #[repr(align(64))]
    pub struct HeaderPrefix {
        pub magic: [u8; 8],
        pub abi_major: u16,
        pub abi_minor: u16,
        pub prefix_size: u16,
        pub header_size: u16,
        pub segment_size: u64,
        pub layout_hash: u64,
        pub init_state: AtomicU32,
        pub refuse_code: AtomicU32,
        /// ASCII, NUL-padded (stored as words so either side can write it atomically).
        pub refuse_detail: [AtomicU64; 8],
        pub _pad: [u8; 24],
    }

    /// One per side; written only by that side.
    #[repr(align(64))]
    pub struct SideBlock {
        pub pid: AtomicU32,
        pub state: AtomicU32,
        /// Random per process start; 0 = never attached.
        pub epoch: AtomicU64,
        /// FILETIME of the process creation (PID-reuse guard).
        pub create_time: AtomicU64,
        pub caps: AtomicU64,
        /// +1 per game frame / per sidecar ipc loop.
        pub hb_count: AtomicU64,
        /// QueryPerformanceCounter at the last beat.
        pub hb_qpc: AtomicU64,
        /// `git describe` of the build, ASCII NUL-padded.
        pub build_id: [AtomicU64; 4],
        pub counters: [AtomicU64; 16],
    }

    #[repr(align(4096))]
    pub struct Header {
        pub prefix: HeaderPrefix,
        pub game: SideBlock,
        pub sidecar: SideBlock,
        /// Written by the sidecar at attach: `game.caps & sidecar.caps`.
        pub caps_effective: AtomicU64,
        /// QueryPerformanceFrequency (written by the game at init), for heartbeat ages.
        pub qpc_freq: AtomicU64,
        /// Game: +1 at every world leave.
        pub world_epoch: AtomicU32,
        /// Game: +1 on UE4SS "restart all mods".
        pub game_lua_gen: AtomicU32,
        /// Sidecar: +1 on every new server session (Welcome).
        pub session_epoch: AtomicU32,
        /// Sidecar: +1 when S2G overflowed; the game re-reads all state.
        pub resync_req: AtomicU32,
        /// Sidecar: +1 on every attach (also on re-attach by a restarted sidecar).
        pub attach_count: AtomicU32,
        pub _r: u32,
    }
}

const _: () = assert!(core::mem::size_of::<HeaderPrefix>() == PREFIX_SIZE as usize);
const _: () = assert!(core::mem::size_of::<Header>() == 4096);

/// Store an ASCII string into NUL-padded atomic words (truncated).
pub fn store_str(words: &[AtomicU64], s: &str) {
    let mut buf = vec![0u8; words.len() * 8];
    let n = s.len().min(buf.len());
    buf[..n].copy_from_slice(&s.as_bytes()[..n]);
    for (i, w) in words.iter().enumerate() {
        let mut b = [0u8; 8];
        b.copy_from_slice(&buf[i * 8..i * 8 + 8]);
        w.store(u64::from_le_bytes(b), Ordering::Relaxed);
    }
}

/// Load a NUL-padded string from atomic words (lossy UTF-8).
pub fn load_str(words: &[AtomicU64]) -> String {
    let mut buf = Vec::with_capacity(words.len() * 8);
    for w in words {
        buf.extend_from_slice(&w.load(Ordering::Relaxed).to_le_bytes());
    }
    let n = buf.iter().position(|&b| b == 0).unwrap_or(buf.len());
    String::from_utf8_lossy(&buf[..n]).into_owned()
}

impl SideBlock {
    pub fn state(&self) -> Option<SideState> {
        SideState::from_u32(self.state.load(Ordering::Acquire))
    }
    pub fn set_state(&self, s: SideState) {
        self.state.store(s as u32, Ordering::Release);
    }
    /// Heartbeat (one per frame / loop): count + QPC stamp. No syscall (QPC is user-mode).
    pub fn beat(&self, qpc: u64) {
        self.hb_count.fetch_add(1, Ordering::Relaxed);
        self.hb_qpc.store(qpc, Ordering::Release);
    }
    pub fn count(&self, idx: usize, n: u64) {
        if let Some(c) = self.counters.get(idx) {
            c.fetch_add(n, Ordering::Relaxed);
        }
    }
    pub fn counter(&self, idx: usize) -> u64 {
        self.counters.get(idx).map_or(0, |c| c.load(Ordering::Relaxed))
    }
    /// Raise a high-water counter to at least `v`.
    pub fn high_water(&self, idx: usize, v: u64) {
        if let Some(c) = self.counters.get(idx) {
            c.fetch_max(v, Ordering::Relaxed);
        }
    }
    pub fn build_id(&self) -> String {
        load_str(&self.build_id)
    }
    /// Seconds since the last heartbeat, given the current QPC and its frequency.
    /// `None` if the side never beat.
    pub fn hb_age_s(&self, now_qpc: u64, qpc_freq: u64) -> Option<f64> {
        let last = self.hb_qpc.load(Ordering::Acquire);
        if last == 0 || qpc_freq == 0 {
            return None;
        }
        Some(now_qpc.saturating_sub(last) as f64 / qpc_freq as f64)
    }
}

impl HeaderPrefix {
    pub fn refuse_code(&self) -> RefuseCode {
        RefuseCode::from_u32(self.refuse_code.load(Ordering::Acquire)).unwrap_or(RefuseCode::None)
    }
    pub fn refuse_detail(&self) -> String {
        load_str(&self.refuse_detail)
    }
}
