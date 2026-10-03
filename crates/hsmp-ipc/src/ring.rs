//! SPSC message ring in shared memory. Power-of-two slots of 512 bytes:
//! a 40-byte record header plus up to 472 bytes of payload. `tail` is written only by the
//! producer, `head` only by the consumer; both live in the segment, so either side can
//! restart without coordination:
//! - a restarted producer continues from `tail` with its new epoch, and the consumer drops
//!   records whose `producer_epoch` is not the producer's current epoch;
//! - a restarted consumer calls [`Ring::reset_consumer`] (`head = tail`).

use core::cell::UnsafeCell;
use core::sync::atomic::{AtomicU64, Ordering};

use crate::layout::{atomic_load_words, atomic_store_words, FieldDesc, IpcType, TypeDesc};

/// Bytes per ring slot.
pub const SLOT_BYTES: usize = 512;
/// Largest payload one record carries.
pub const MAX_PAYLOAD: usize = SLOT_BYTES - core::mem::size_of::<RecordHeader>();

crate::ipc_pod! {
    /// Header of every ring record. `kind`, `aux` and `peer` are the fields of the v6 wire
    /// header ([`crate::wire::WireHdr`]): a record moves between the network and the ring as
    /// a copy.
    pub struct RecordHeader {
        pub len: u16,
        pub kind: u16,
        pub aux: u16,
        pub flags: u16,
        /// The peer the record is about / from (S2G: the wire header's peer).
        pub peer: u32,
        pub _r: u32,
        pub producer_epoch: u64,
        /// Ring position of this record (monotonic per ring); the consumer checks it equals
        /// its `head`, which catches gaps, duplicates and scribbles in one compare.
        pub seq: u64,
        pub req_id: u64,
    }

    /// One ring slot / one record as the consumer sees it.
    pub struct Record {
        pub hdr: RecordHeader,
        pub payload: [u8; 472],
    }
}

const _: () = assert!(core::mem::size_of::<Record>() == SLOT_BYTES);

impl Record {
    /// The payload bytes (length clamped to the capacity).
    pub fn payload(&self) -> &[u8] {
        let n = (self.hdr.len as usize).min(MAX_PAYLOAD);
        &self.payload[..n]
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PushError {
    /// The consumer is `N` records behind.
    Full,
    /// Payload over [`MAX_PAYLOAD`]: a schema error. Large data belongs in a blob.
    TooBig,
    /// `head`/`tail` are inconsistent (scribbled segment).
    Corrupt,
}

/// Result of one [`Ring::pop`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pop {
    /// Nothing pending.
    Empty,
    /// `out` holds a valid record from the expected producer epoch.
    Record,
    /// A record from another producer epoch (a dead predecessor) was skipped.
    StaleEpoch,
    /// A malformed record (bad seq or length) was skipped.
    Bad,
    /// `head`/`tail` were inconsistent; the consumer jumped to `tail`.
    Resynced,
}

#[repr(C, align(64))]
pub struct Ring<const N: usize> {
    tail: AtomicU64,
    _p0: [u64; 7],
    head: AtomicU64,
    _p1: [u64; 7],
    slots: [UnsafeCell<Record>; N],
}

// SAFETY: slot ownership is handed over by the Release/Acquire head/tail protocol; copies
// are atomic words.
unsafe impl<const N: usize> Sync for Ring<N> {}

unsafe impl<const N: usize> IpcType for Ring<N> {
    const DESC: TypeDesc = TypeDesc::Struct {
        name: "Ring",
        size: core::mem::size_of::<Self>(),
        align: core::mem::align_of::<Self>(),
        pod: false,
        fields: &[
            FieldDesc { name: "tail", offset: core::mem::offset_of!(Self, tail), ty: &<u64 as IpcType>::DESC },
            FieldDesc { name: "head", offset: core::mem::offset_of!(Self, head), ty: &<u64 as IpcType>::DESC },
            FieldDesc { name: "slots", offset: core::mem::offset_of!(Self, slots), ty: &<[Record; N] as IpcType>::DESC },
        ],
    };
}

impl<const N: usize> Ring<N> {
    const POW2: () = assert!(N.is_power_of_two() && N >= 2);

    pub const CAPACITY: usize = N;

    /// A standalone ring on the heap (tests, tools).
    pub fn new_boxed() -> Box<Self> {
        // SAFETY: all-zero = empty ring.
        unsafe { crate::layout::boxed_zeroed_raw::<Self>() }
    }

    /// Producer: append one record. Returns its ring position (`seq`).
    pub fn push(&self, producer_epoch: u64, kind: u16, flags: u16, req_id: u64, payload: &[u8]) -> Result<u64, PushError> {
        self.push_msg(producer_epoch, kind, 0, 0, flags, req_id, payload)
    }

    /// Producer: append one record with the wire fields `aux` and `peer`.
    #[allow(clippy::too_many_arguments)]
    pub fn push_msg(&self, producer_epoch: u64, kind: u16, aux: u16, peer: u32, flags: u16, req_id: u64, payload: &[u8]) -> Result<u64, PushError> {
        let () = Self::POW2;
        if payload.len() > MAX_PAYLOAD {
            return Err(PushError::TooBig);
        }
        let t = self.tail.load(Ordering::Relaxed);
        let h = self.head.load(Ordering::Acquire);
        if h > t || t == u64::MAX {
            return Err(PushError::Corrupt);
        }
        if t - h >= N as u64 {
            return Err(PushError::Full);
        }
        let mut rec = Record::default();
        rec.hdr = RecordHeader { len: payload.len() as u16, kind, aux, flags, peer, _r: 0, producer_epoch, seq: t, req_id };
        rec.payload[..payload.len()].copy_from_slice(payload);
        // SAFETY: slot t % N is free (t - h < N) and owned by the producer until `tail` moves.
        unsafe { atomic_store_words(&rec, self.slots[(t % N as u64) as usize].get()) };
        self.tail.store(t + 1, Ordering::Release);
        Ok(t)
    }

    /// Consumer: take the next record into `out`. `expected_epoch` = the producer side's
    /// current epoch (0 = accept any).
    ///
    /// A consumer that loads the producer epoch from the header and then pops must not pop
    /// with an epoch of 0 (no producer attached yet) if it validates the records against that
    /// epoch: a producer that attaches in between hands it records of the new epoch, which
    /// `0` accepts. Skip consuming until the epoch is known (`ipc-stress` scenario 2 / 4 / 5,
    /// `--race-ms`).
    pub fn pop(&self, expected_epoch: u64, out: &mut Record) -> Pop {
        let () = Self::POW2;
        let h = self.head.load(Ordering::Relaxed);
        let t = self.tail.load(Ordering::Acquire);
        if h == t {
            return Pop::Empty;
        }
        if t < h || t - h > N as u64 {
            self.head.store(t, Ordering::Release);
            return Pop::Resynced;
        }
        // SAFETY: slot h % N was published by the producer (h < t) and is not reused before
        // `head` passes it.
        unsafe { atomic_load_words(self.slots[(h % N as u64) as usize].get(), out) };
        self.head.store(h + 1, Ordering::Release);
        if out.hdr.seq != h || out.hdr.len as usize > MAX_PAYLOAD {
            return Pop::Bad;
        }
        if expected_epoch != 0 && out.hdr.producer_epoch != expected_epoch {
            return Pop::StaleEpoch;
        }
        Pop::Record
    }

    /// Records waiting (as seen by whoever calls it).
    pub fn len(&self) -> u64 {
        let t = self.tail.load(Ordering::Acquire);
        let h = self.head.load(Ordering::Acquire);
        t.saturating_sub(h)
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Producer position (next seq).
    pub fn tail(&self) -> u64 {
        self.tail.load(Ordering::Acquire)
    }

    /// Consumer position (next seq to read).
    pub fn head(&self) -> u64 {
        self.head.load(Ordering::Acquire)
    }

    /// A restarted consumer: everything pending was addressed to its predecessor.
    pub fn reset_consumer(&self) {
        self.head.store(self.tail.load(Ordering::Acquire), Ordering::Release);
    }

    /// Read-only peek at the record at ring position `seq` (tools: `ipc-dump`). Racy by
    /// nature; returns `None` if the position is no longer (or not yet) in the ring.
    pub fn peek(&self, seq: u64, out: &mut Record) -> bool {
        let t = self.tail.load(Ordering::Acquire);
        if seq >= t || t - seq > N as u64 {
            return false;
        }
        // SAFETY: in bounds; a concurrent overwrite is caught by the seq compare below.
        unsafe { atomic_load_words(self.slots[(seq % N as u64) as usize].get(), out) };
        out.hdr.seq == seq
    }
}
