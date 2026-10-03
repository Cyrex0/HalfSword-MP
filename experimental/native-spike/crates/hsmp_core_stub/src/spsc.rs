//! Bounded, allocation-free single-producer / single-consumer ring of
//! datagram-sized slots.
//!
//! Contract: at most one thread calls `push` and at most one thread calls
//! `pop` at any moment. The C ABI in `lib.rs` enforces this with a tiny
//! uncontended try-lock per side, so a misbehaving caller (two Lua states on
//! two threads) degrades to "busy" instead of corrupting memory.

use std::cell::UnsafeCell;
use std::sync::atomic::{AtomicUsize, Ordering};

/// Max payload per message. Matches the internet-grade UDP budget (~1200 B).
pub const SLOT_BYTES: usize = 1200;

#[repr(align(64))]
struct CachePadded<T>(T);

struct Slot {
    len: UnsafeCell<u16>,
    data: UnsafeCell<[u8; SLOT_BYTES]>,
}

pub struct Spsc {
    slots: Box<[Slot]>,
    mask: usize,
    /// Next index to read. Written only by the consumer.
    head: CachePadded<AtomicUsize>,
    /// Next index to write. Written only by the producer.
    tail: CachePadded<AtomicUsize>,
}

// SAFETY: slot contents are handed over with Release/Acquire on head/tail;
// the producer only touches slot[tail], the consumer only slot[head], and the
// indices never alias while the slot is owned by the other side.
unsafe impl Sync for Spsc {}
unsafe impl Send for Spsc {}

#[derive(Debug, PartialEq, Eq)]
pub enum PushError {
    Full,
    TooBig,
}

#[derive(Debug, PartialEq, Eq)]
pub enum PopError {
    Empty,
    /// Caller's buffer is too small; the message stays queued.
    TooSmall(usize),
}

impl Spsc {
    /// `capacity` is rounded up to a power of two.
    pub fn new(capacity: usize) -> Self {
        let cap = capacity.max(2).next_power_of_two();
        let slots = (0..cap)
            .map(|_| Slot { len: UnsafeCell::new(0), data: UnsafeCell::new([0u8; SLOT_BYTES]) })
            .collect::<Vec<_>>()
            .into_boxed_slice();
        Self {
            slots,
            mask: cap - 1,
            head: CachePadded(AtomicUsize::new(0)),
            tail: CachePadded(AtomicUsize::new(0)),
        }
    }

    pub fn capacity(&self) -> usize {
        self.mask + 1
    }

    pub fn len(&self) -> usize {
        let tail = self.tail.0.load(Ordering::Acquire);
        let head = self.head.0.load(Ordering::Acquire);
        tail.wrapping_sub(head)
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Producer side.
    pub fn push(&self, msg: &[u8]) -> Result<(), PushError> {
        if msg.len() > SLOT_BYTES {
            return Err(PushError::TooBig);
        }
        let tail = self.tail.0.load(Ordering::Relaxed);
        let head = self.head.0.load(Ordering::Acquire);
        if tail.wrapping_sub(head) > self.mask {
            return Err(PushError::Full);
        }
        let slot = &self.slots[tail & self.mask];
        // SAFETY: slot[tail] is not visible to the consumer until the Release
        // store below, and the consumer has released it (head > tail - cap).
        unsafe {
            std::ptr::copy_nonoverlapping(msg.as_ptr(), slot.data.get() as *mut u8, msg.len());
            *slot.len.get() = msg.len() as u16;
        }
        self.tail.0.store(tail.wrapping_add(1), Ordering::Release);
        Ok(())
    }

    /// Consumer side. Copies one message into `out` and returns its length.
    pub fn pop(&self, out: &mut [u8]) -> Result<usize, PopError> {
        let head = self.head.0.load(Ordering::Relaxed);
        let tail = self.tail.0.load(Ordering::Acquire);
        if head == tail {
            return Err(PopError::Empty);
        }
        let slot = &self.slots[head & self.mask];
        // SAFETY: the Acquire load of tail makes the producer's writes to
        // slot[head] visible; the producer will not reuse it until we
        // Release-store head + 1.
        let n = unsafe { *slot.len.get() } as usize;
        if out.len() < n {
            return Err(PopError::TooSmall(n));
        }
        unsafe {
            std::ptr::copy_nonoverlapping(slot.data.get() as *const u8, out.as_mut_ptr(), n);
        }
        self.head.0.store(head.wrapping_add(1), Ordering::Release);
        Ok(n)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    #[test]
    fn fifo_and_full() {
        let q = Spsc::new(4);
        assert_eq!(q.capacity(), 4);
        for i in 0..4u8 {
            q.push(&[i; 3]).unwrap();
        }
        assert_eq!(q.push(b"x"), Err(PushError::Full));
        let mut b = [0u8; 8];
        for i in 0..4u8 {
            assert_eq!(q.pop(&mut b), Ok(3));
            assert_eq!(&b[..3], &[i; 3]);
        }
        assert_eq!(q.pop(&mut b), Err(PopError::Empty));
    }

    #[test]
    fn too_big_and_too_small() {
        let q = Spsc::new(2);
        assert_eq!(q.push(&[0u8; SLOT_BYTES + 1]), Err(PushError::TooBig));
        q.push(&[7u8; 10]).unwrap();
        let mut small = [0u8; 4];
        assert_eq!(q.pop(&mut small), Err(PopError::TooSmall(10)));
        let mut big = [0u8; 16];
        assert_eq!(q.pop(&mut big), Ok(10));
    }

    #[test]
    fn cross_thread_order_and_integrity() {
        const N: u32 = 200_000;
        let q = Arc::new(Spsc::new(64));
        let p = q.clone();
        let prod = std::thread::spawn(move || {
            let mut i = 0u32;
            while i < N {
                let mut msg = [0u8; 64];
                msg[..4].copy_from_slice(&i.to_le_bytes());
                let len = 4 + (i % 60) as usize;
                for (k, b) in msg[4..len].iter_mut().enumerate() {
                    *b = (i as usize + k) as u8;
                }
                if p.push(&msg[..len]).is_ok() {
                    i += 1;
                } else {
                    std::hint::spin_loop();
                }
            }
        });
        let mut expect = 0u32;
        let mut b = [0u8; SLOT_BYTES];
        while expect < N {
            match q.pop(&mut b) {
                Ok(n) => {
                    let i = u32::from_le_bytes(b[..4].try_into().unwrap());
                    assert_eq!(i, expect);
                    assert_eq!(n, 4 + (i % 60) as usize);
                    for k in 0..n - 4 {
                        assert_eq!(b[4 + k], (i as usize + k) as u8);
                    }
                    expect += 1;
                }
                Err(PopError::Empty) => std::hint::spin_loop(),
                Err(e) => panic!("{e:?}"),
            }
        }
        prod.join().unwrap();
        assert!(q.is_empty());
    }
}
