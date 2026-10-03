//! Lock-free triple buffer for large state blobs. One writer, one reader, in
//! any two processes; neither ever waits or retries, and neither can see a buffer the
//! other side is writing.
//!
//! Control word `middle`: bits 0..2 = buffer index, bit 2 = DIRTY, bits 8..32 = generation.
//! The writer's back index and the reader's front index live in the segment too (each
//! written only by its owner), so a restarted process resumes without a reset handshake.
//!
//! [`TripleBuf::init`] must run once at segment creation (the game does it for every blob,
//! including the ones the sidecar writes), before either side touches the buffer.

use core::cell::UnsafeCell;
use core::sync::atomic::{AtomicU32, Ordering};

use crate::layout::{atomic_load_words, atomic_store_words, FieldDesc, IpcType, Pod, TypeDesc};

const IDX_MASK: u32 = 0b11;
const DIRTY: u32 = 0b100;
const GEN_SHIFT: u32 = 8;

#[repr(C, align(64))]
pub struct TripleBuf<T: Pod> {
    middle: AtomicU32,
    /// Writer-owned: the buffer the writer fills next.
    w_back: AtomicU32,
    /// Writer-owned: the generation of the last publish.
    w_gen: AtomicU32,
    /// Reader-owned: the buffer the reader holds.
    r_front: AtomicU32,
    /// Reader-owned: the generation of the buffer in `r_front`.
    r_gen: AtomicU32,
    _pad: [u32; 11],
    bufs: [UnsafeCell<T>; 3],
}

// SAFETY: buffers are handed over through the atomic `middle` swap; copies are atomic words.
unsafe impl<T: Pod> Sync for TripleBuf<T> {}

unsafe impl<T: Pod> IpcType for TripleBuf<T> {
    const DESC: TypeDesc = TypeDesc::Struct {
        name: "TripleBuf",
        size: core::mem::size_of::<Self>(),
        align: core::mem::align_of::<Self>(),
        pod: false,
        fields: &[
            FieldDesc { name: "middle", offset: core::mem::offset_of!(Self, middle), ty: &<u32 as IpcType>::DESC },
            FieldDesc { name: "w_back", offset: core::mem::offset_of!(Self, w_back), ty: &<u32 as IpcType>::DESC },
            FieldDesc { name: "w_gen", offset: core::mem::offset_of!(Self, w_gen), ty: &<u32 as IpcType>::DESC },
            FieldDesc { name: "r_front", offset: core::mem::offset_of!(Self, r_front), ty: &<u32 as IpcType>::DESC },
            FieldDesc { name: "r_gen", offset: core::mem::offset_of!(Self, r_gen), ty: &<u32 as IpcType>::DESC },
            FieldDesc { name: "bufs", offset: core::mem::offset_of!(Self, bufs), ty: &<[T; 3] as IpcType>::DESC },
        ],
    };
}


impl<T: Pod> TripleBuf<T> {
    /// A standalone, initialised triple buffer on the heap (tests, tools).
    pub fn new_boxed() -> Box<Self> {
        // SAFETY: all-zero is valid; `init` then sets the indices.
        let b = unsafe { crate::layout::boxed_zeroed_raw::<Self>() };
        b.init();
        b
    }

    /// Initial indices: back = 0, middle = 1 (clean, gen 0), front = 2.
    pub fn init(&self) {
        self.w_back.store(0, Ordering::Relaxed);
        self.w_gen.store(0, Ordering::Relaxed);
        self.r_front.store(2, Ordering::Relaxed);
        self.r_gen.store(0, Ordering::Relaxed);
        self.middle.store(1, Ordering::Release);
    }

    /// Generation of the newest published buffer (0 = never published). One load.
    #[inline]
    pub fn published_gen(&self) -> u32 {
        self.middle.load(Ordering::Acquire) >> GEN_SHIFT
    }

    /// True if the indices are a permutation of 0..3. Diagnostic only: while the reader is
    /// mid-take they briefly are not.
    pub fn indices_valid(&self) -> bool {
        let m = self.middle.load(Ordering::Acquire) & IDX_MASK;
        let b = self.w_back.load(Ordering::Relaxed);
        let f = self.r_front.load(Ordering::Relaxed);
        m < 3 && b < 3 && f < 3 && m != b && m != f && b != f
    }

    /// An index that is neither `a` nor `b` (either may be out of range; they may be equal).
    fn other_than(a: u32, b: u32) -> u32 {
        (0..3).find(|&c| c != a && c != b).unwrap_or(0)
    }

    /// Writer: copy `value` into the back buffer and publish it. Returns the new generation,
    /// or `None` if the control word is corrupt (scribbled segment; nothing is published).
    ///
    /// Restart repair: in steady state the writer's back index is never the one in `middle`.
    /// It can be only after a writer died between its swap and storing `w_back`; the new
    /// writer then picks another buffer. The writer must not compare with `r_front`: while
    /// the reader is between its CAS and its `r_front` store, `r_front` briefly names the
    /// buffer that is already back in `middle`, and such a "repair" would steal the buffer
    /// the reader just took. A reader that died in that window is repaired by the reader.
    pub fn publish(&self, value: &T) -> Option<u32> {
        let m = self.middle.load(Ordering::Acquire);
        let mi = m & IDX_MASK;
        if mi >= 3 {
            return None;
        }
        let mut back = self.w_back.load(Ordering::Relaxed);
        if back >= 3 || back == mi {
            back = Self::other_than(mi, self.r_front.load(Ordering::Acquire));
            self.w_back.store(back, Ordering::Relaxed);
        }
        // SAFETY: `back` < 3 and is owned by the writer until it is swapped into `middle`.
        unsafe { atomic_store_words(value, self.bufs[back as usize].get()) };
        // Generations only move forward, also across a writer restart.
        let last = self.w_gen.load(Ordering::Relaxed).max(m >> GEN_SHIFT);
        let gen = last.wrapping_add(1) & 0x00ff_ffff;
        let gen = if gen == 0 { 1 } else { gen };
        let prev = self.middle.swap(back | DIRTY | (gen << GEN_SHIFT), Ordering::AcqRel);
        self.w_gen.store(gen, Ordering::Relaxed);
        let pi = prev & IDX_MASK;
        // `pi == back` only on a scribbled control word; the next publish repairs `back`.
        self.w_back.store(if pi < 3 { pi } else { Self::other_than(back, back) }, Ordering::Relaxed);
        Some(gen)
    }

    /// Reader: if a newer buffer was published, take it and copy it into `out`.
    /// Returns `Some(gen)` when `out` was filled with new data, `None` when nothing new.
    pub fn take_into(&self, out: &mut T) -> Option<u32> {
        let mut m = self.middle.load(Ordering::Acquire);
        if m & DIRTY == 0 {
            return None;
        }
        let mut front = self.r_front.load(Ordering::Relaxed);
        let wb = self.w_back.load(Ordering::Acquire);
        // The reader is the only thread that takes, so it is not mid-take here; in steady
        // state its front is then neither in `middle` nor the writer's back. Either happens
        // only after a reader died between its CAS and storing `r_front`.
        if front >= 3 || front == (m & IDX_MASK) || front == wb {
            front = Self::other_than(m & IDX_MASK, wb);
            self.r_front.store(front, Ordering::Release);
        }
        // CAS, not swap: the generation bits stay in the control word, so `published_gen`
        // keeps reporting the newest publish after the reader took it.
        let prev = loop {
            if m & DIRTY == 0 {
                return None;
            }
            let new = front | (m & !(IDX_MASK | DIRTY));
            match self.middle.compare_exchange_weak(m, new, Ordering::AcqRel, Ordering::Acquire) {
                Ok(p) => break p,
                Err(cur) => m = cur,
            }
        };
        let gen = prev >> GEN_SHIFT;
        let idx = prev & IDX_MASK;
        if idx >= 3 || idx == front {
            return None;
        }
        self.r_front.store(idx, Ordering::Release);
        self.r_gen.store(gen, Ordering::Relaxed);
        // SAFETY: `idx` < 3 and is owned by the reader now.
        unsafe { atomic_load_words(self.bufs[idx as usize].get(), out) };
        Some(gen)
    }

    /// Reader: copy the buffer currently held (the last taken generation) into `out`.
    /// Returns its generation, 0 if nothing was ever taken.
    pub fn current_into(&self, out: &mut T) -> u32 {
        let front = self.r_front.load(Ordering::Relaxed);
        if front >= 3 {
            return 0;
        }
        // SAFETY: the reader owns `front`.
        unsafe { atomic_load_words(self.bufs[front as usize].get(), out) };
        self.r_gen.load(Ordering::Relaxed)
    }
}
