//! Seqlock "latest value" slot. One writer thread in one process, any number of
//! readers that never block the writer. Payload words are copied with relaxed atomics.

use core::cell::UnsafeCell;
use core::sync::atomic::{fence, AtomicU64, Ordering};

use crate::layout::{atomic_load_words, atomic_store_words, FieldDesc, IpcType, Pod, TypeDesc};

/// How often a reader retries a slot whose writer is mid-write before giving up with `Busy`.
pub const READ_ATTEMPTS: u32 = 4;

/// A seqlock slot holding one `T`. `seq` is even when stable, odd while a write is in
/// progress, and 0 when the slot was never written.
#[repr(C, align(64))]
pub struct SeqSlot<T: Pod> {
    seq: AtomicU64,
    _pad: [u64; 7],
    data: UnsafeCell<T>,
}

// SAFETY: every access to `data` goes through atomic word copies; `seq` is atomic.
unsafe impl<T: Pod> Sync for SeqSlot<T> {}

unsafe impl<T: Pod> IpcType for SeqSlot<T> {
    const DESC: TypeDesc = TypeDesc::Struct {
        name: "SeqSlot",
        size: core::mem::size_of::<Self>(),
        align: core::mem::align_of::<Self>(),
        pod: false,
        fields: &[
            FieldDesc { name: "seq", offset: core::mem::offset_of!(Self, seq), ty: &<u64 as IpcType>::DESC },
            FieldDesc { name: "data", offset: core::mem::offset_of!(Self, data), ty: &T::DESC },
        ],
    };
}

/// Why a read returned no value.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReadError {
    /// Never written (seq == 0).
    Empty,
    /// The writer was mid-write on every attempt (or died mid-write). Keep the last good value.
    Busy,
}

impl<T: Pod> SeqSlot<T> {
    /// A standalone slot on the heap (tests, tools). Slots in the segment need no constructor.
    pub fn new_boxed() -> Box<Self> {
        // SAFETY: all-zero = never written.
        unsafe { crate::layout::boxed_zeroed_raw::<Self>() }
    }

    /// The current sequence number (one load). Compare with the last consumed value to get a
    /// cheap "changed?" check.
    #[inline]
    pub fn seq(&self) -> u64 {
        self.seq.load(Ordering::Acquire)
    }

    /// Odd sequence that starts the next write after `s` (a dead writer left `s` odd; a
    /// scribbled `s` near the top wraps to a fresh start, which readers see as a change).
    #[inline]
    fn start_seq(s: u64) -> u64 {
        if s >= u64::MAX - 2 {
            1
        } else if s % 2 == 1 {
            s
        } else {
            s + 1
        }
    }

    /// Publish `value`. Must only be called by the slot's single writer.
    pub fn write(&self, value: &T) {
        let s = self.seq.load(Ordering::Relaxed);
        // A predecessor that died mid-write left `s` odd: the next even value after it
        // re-initialises the slot.
        let start = Self::start_seq(s);
        self.seq.store(start, Ordering::Relaxed);
        fence(Ordering::Release);
        // SAFETY: `data` is inside `self`, 8-aligned (align(64) struct, data at offset 64).
        unsafe { atomic_store_words(value, self.data.get()) };
        fence(Ordering::Release);
        self.seq.store(start + 1, Ordering::Release);
    }

    /// Read a consistent copy into `out`. Returns the sequence number it was read at.
    pub fn read_into(&self, out: &mut T) -> Result<u64, ReadError> {
        for _ in 0..READ_ATTEMPTS {
            let s1 = self.seq.load(Ordering::Acquire);
            if s1 == 0 {
                return Err(ReadError::Empty);
            }
            if s1 % 2 == 1 {
                core::hint::spin_loop();
                continue;
            }
            // SAFETY: as in `write`.
            unsafe { atomic_load_words(self.data.get(), out) };
            fence(Ordering::Acquire);
            if self.seq.load(Ordering::Relaxed) == s1 {
                return Ok(s1);
            }
        }
        Err(ReadError::Busy)
    }

    /// Read a consistent copy by value (for small `T`).
    pub fn read(&self) -> Result<(T, u64), ReadError> {
        let mut v = T::zeroed();
        let s = self.read_into(&mut v)?;
        Ok((v, s))
    }

    /// Read only if `seq` moved past `last`. `Ok(None)` = unchanged.
    pub fn read_if_changed(&self, last: u64, out: &mut T) -> Result<Option<u64>, ReadError> {
        if self.seq.load(Ordering::Acquire) == last {
            return Ok(None);
        }
        self.read_into(out).map(Some)
    }

    /// Writer-side test hook: leave the slot as a writer that died between the odd and the
    /// even store (`ipc-stress --die-mid-write`).
    #[doc(hidden)]
    pub fn begin_write_and_abandon(&self) {
        let s = self.seq.load(Ordering::Relaxed);
        let start = Self::start_seq(s);
        self.seq.store(start, Ordering::Release);
    }
}
