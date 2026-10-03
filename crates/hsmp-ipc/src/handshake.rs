//! Segment initialisation, validation and attach. OS-agnostic: works on
//! any mapping, including [`crate::shm::Mapping::anonymous`] in tests.

use core::sync::atomic::Ordering;

use crate::header::{init_state, store_str, RefuseCode, SideBlock, SideState, MAGIC, PREFIX_SIZE};
use crate::segment::{Segment, COMPAT_HASHES, LAYOUT_HASH, SEGMENT_SIZE};
use crate::{ABI_MAJOR, ABI_MINOR};

/// A refusal with its human-readable detail (also written to the header when possible).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Refusal {
    pub code: RefuseCode,
    pub detail: String,
}

impl core::fmt::Display for Refusal {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "{}: {}", self.code.as_str(), self.detail)
    }
}
impl std::error::Error for Refusal {}

fn refusal(code: RefuseCode, detail: impl Into<String>) -> Refusal {
    Refusal { code, detail: detail.into() }
}

/// What a side publishes about itself.
#[derive(Debug, Clone)]
pub struct SideParams<'a> {
    pub pid: u32,
    /// FILETIME of this process' creation.
    pub create_time: u64,
    /// Random, non-zero, per process start.
    pub epoch: u64,
    pub caps: u64,
    pub build_id: &'a str,
}

/// How [`init_game`] found the segment.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GameOpen {
    /// Fresh zero pages: initialised now.
    Fresh,
    /// Already initialised by this same process (a mod reload); `game_lua_gen` was bumped.
    Reopened,
}

fn publish_side(b: &SideBlock, p: &SideParams<'_>) {
    b.set_state(SideState::Starting);
    b.pid.store(p.pid, Ordering::Relaxed);
    b.create_time.store(p.create_time, Ordering::Relaxed);
    b.caps.store(p.caps, Ordering::Relaxed);
    store_str(&b.build_id, p.build_id);
    b.epoch.store(p.epoch, Ordering::Release);
}

/// Game: initialise a freshly created segment, or adopt one this process created earlier.
///
/// # Safety
/// `seg` must point to a mapping of at least `SEGMENT_SIZE` bytes, 4 KiB aligned, that this
/// process created (fresh pages are zero). No other thread of this process may be using it.
pub unsafe fn init_game(seg: *mut Segment, mapped_len: usize, p: &SideParams<'_>, qpc_freq: u64) -> Result<GameOpen, Refusal> {
    if mapped_len < SEGMENT_SIZE {
        return Err(refusal(RefuseCode::SizeMismatch, format!("mapped {} < segment {}", mapped_len, SEGMENT_SIZE)));
    }
    // SAFETY: caller guarantees a valid, aligned mapping.
    let s: &Segment = unsafe { &*seg };
    let pre = &s.header.prefix;
    match pre.init_state.compare_exchange(init_state::ZERO, init_state::INITIALISING, Ordering::AcqRel, Ordering::Acquire) {
        Ok(_) => {}
        Err(init_state::READY) => {
            check_prefix(s, mapped_len)?;
            let g = &s.header.game;
            if g.pid.load(Ordering::Relaxed) != p.pid || g.create_time.load(Ordering::Relaxed) != p.create_time {
                return Err(refusal(RefuseCode::WrongParent, "segment belongs to another game process"));
            }
            s.header.game_lua_gen.fetch_add(1, Ordering::AcqRel);
            return Ok(GameOpen::Reopened);
        }
        Err(other) => return Err(refusal(RefuseCode::NotReady, format!("init_state {}", other))),
    }
    // SAFETY: we own the INITIALISING state; nobody reads the plain prefix fields before
    // READY is published with Release below.
    unsafe {
        let h = core::ptr::addr_of_mut!((*seg).header.prefix);
        core::ptr::addr_of_mut!((*h).magic).write_volatile(MAGIC);
        core::ptr::addr_of_mut!((*h).abi_major).write_volatile(ABI_MAJOR);
        core::ptr::addr_of_mut!((*h).abi_minor).write_volatile(ABI_MINOR);
        core::ptr::addr_of_mut!((*h).prefix_size).write_volatile(PREFIX_SIZE);
        core::ptr::addr_of_mut!((*h).header_size).write_volatile(core::mem::size_of::<crate::header::Header>() as u16);
        core::ptr::addr_of_mut!((*h).segment_size).write_volatile(SEGMENT_SIZE as u64);
        core::ptr::addr_of_mut!((*h).layout_hash).write_volatile(LAYOUT_HASH);
    }
    s.init_blobs();
    s.header.qpc_freq.store(qpc_freq, Ordering::Relaxed);
    s.header.world_epoch.store(1, Ordering::Relaxed);
    publish_side(&s.header.game, p);
    s.header.game.set_state(SideState::Ready);
    pre.init_state.store(init_state::READY, Ordering::Release);
    Ok(GameOpen::Fresh)
}

/// Check magic, ABI major, layout hash and size. Never writes.
pub fn check_prefix(s: &Segment, mapped_len: usize) -> Result<(), Refusal> {
    let pre = &s.header.prefix;
    let st = pre.init_state.load(Ordering::Acquire);
    if st == init_state::POISONED {
        return Err(refusal(RefuseCode::Poisoned, "the game disabled IPC after a native fault"));
    }
    if st != init_state::READY {
        return Err(refusal(RefuseCode::NotReady, format!("init_state {}", st)));
    }
    // SAFETY: READY was published with Release after the prefix was written.
    let (magic, major, minor, size, hash, psize) = unsafe {
        (
            core::ptr::addr_of!(pre.magic).read_volatile(),
            core::ptr::addr_of!(pre.abi_major).read_volatile(),
            core::ptr::addr_of!(pre.abi_minor).read_volatile(),
            core::ptr::addr_of!(pre.segment_size).read_volatile(),
            core::ptr::addr_of!(pre.layout_hash).read_volatile(),
            core::ptr::addr_of!(pre.prefix_size).read_volatile(),
        )
    };
    if magic != MAGIC {
        return Err(refusal(RefuseCode::BadMagic, "not an HSMP segment"));
    }
    if psize != PREFIX_SIZE {
        return Err(refusal(RefuseCode::AbiMismatch, format!("prefix size {} != {}", psize, PREFIX_SIZE)));
    }
    if major != ABI_MAJOR || (hash != LAYOUT_HASH && !COMPAT_HASHES.contains(&hash)) {
        return Err(refusal(
            RefuseCode::AbiMismatch,
            format!("mine abi {}.{} hash {:016x}; segment abi {}.{} hash {:016x}", ABI_MAJOR, ABI_MINOR, LAYOUT_HASH, major, minor, hash),
        ));
    }
    if (size as usize) > mapped_len || (size as usize) < SEGMENT_SIZE {
        return Err(refusal(RefuseCode::SizeMismatch, format!("segment {} mapped {} need {}", size, mapped_len, SEGMENT_SIZE)));
    }
    Ok(())
}

/// Record a refusal in the header. Not for `BadMagic` (never write into foreign memory) and
/// not for `Poisoned` (keep the game's own detail about the fault).
pub fn write_refusal(s: &Segment, r: &Refusal) {
    if r.code == RefuseCode::BadMagic || r.code == RefuseCode::Poisoned {
        return;
    }
    store_str(&s.header.prefix.refuse_detail, &r.detail);
    s.header.prefix.refuse_code.store(r.code as u32, Ordering::Release);
}

/// What the sidecar learned at attach.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Attached {
    pub caps_effective: u64,
    pub game_epoch: u64,
    /// The epoch of the sidecar that was attached before (0 = first attach).
    pub previous_sidecar_epoch: u64,
    pub attach_count: u32,
}

/// Sidecar: validate the segment and attach. `parent_create_time = None` skips the
/// creation-time check (tests); the PID is always checked.
///
/// On refusal the code and detail are written to the header (except `BadMagic`); the caller
/// exits with [`RefuseCode::exit_code`].
pub fn attach_sidecar(s: &Segment, mapped_len: usize, parent_pid: u32, parent_create_time: Option<u64>, p: &SideParams<'_>) -> Result<Attached, Refusal> {
    let r = (|| {
        check_prefix(s, mapped_len)?;
        let g = &s.header.game;
        let gpid = g.pid.load(Ordering::Acquire);
        if gpid != parent_pid {
            return Err(refusal(RefuseCode::WrongParent, format!("segment game pid {} != parent pid {}", gpid, parent_pid)));
        }
        if let Some(ct) = parent_create_time {
            let gct = g.create_time.load(Ordering::Acquire);
            if gct != ct {
                return Err(refusal(RefuseCode::WrongParent, format!("game create time {:x} != parent {:x}", gct, ct)));
            }
        }
        Ok(())
    })();
    if let Err(e) = r {
        write_refusal(s, &e);
        return Err(e);
    }
    let h = &s.header;
    let previous = h.sidecar.epoch.load(Ordering::Acquire);
    // Consumer restart: everything pending in G2S was addressed to the predecessor.
    s.g2s().reset_consumer();
    publish_side(&h.sidecar, p);
    let game_caps = h.game.caps.load(Ordering::Acquire);
    let eff = game_caps & p.caps;
    h.caps_effective.store(eff, Ordering::Release);
    h.prefix.refuse_code.store(RefuseCode::None as u32, Ordering::Release);
    let n = h.attach_count.fetch_add(1, Ordering::AcqRel) + 1;
    h.sidecar.set_state(SideState::Ready);
    Ok(Attached { caps_effective: eff, game_epoch: h.game.epoch.load(Ordering::Acquire), previous_sidecar_epoch: previous, attach_count: n })
}

/// Sidecar: orderly detach (sets `Closing`; the game treats it like a death).
pub fn detach_sidecar(s: &Segment) {
    s.header.sidecar.set_state(SideState::Closing);
}

/// Game: poison the segment after a caught native panic; both sides stop using it.
pub fn poison(s: &Segment, detail: &str) {
    store_str(&s.header.prefix.refuse_detail, detail);
    s.header.prefix.refuse_code.store(RefuseCode::Poisoned as u32, Ordering::Release);
    s.header.prefix.init_state.store(init_state::POISONED, Ordering::Release);
}

/// Test hook (`ipc-stress`, tests): overwrite prefix fields of an initialised segment to
/// provoke refusals.
///
/// # Safety
/// `seg` must point to a live, writable segment mapping.
#[doc(hidden)]
pub unsafe fn debug_corrupt_prefix(seg: *mut Segment, magic: Option<[u8; 8]>, abi_major: Option<u16>, layout_hash: Option<u64>, segment_size: Option<u64>) {
    // SAFETY: caller guarantees a writable mapping; volatile writes to plain prefix fields.
    unsafe {
        let h = core::ptr::addr_of_mut!((*seg).header.prefix);
        if let Some(m) = magic {
            core::ptr::addr_of_mut!((*h).magic).write_volatile(m);
        }
        if let Some(a) = abi_major {
            core::ptr::addr_of_mut!((*h).abi_major).write_volatile(a);
        }
        if let Some(x) = layout_hash {
            core::ptr::addr_of_mut!((*h).layout_hash).write_volatile(x);
        }
        if let Some(s) = segment_size {
            core::ptr::addr_of_mut!((*h).segment_size).write_volatile(s);
        }
    }
}
