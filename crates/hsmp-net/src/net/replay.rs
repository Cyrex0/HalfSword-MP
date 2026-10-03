//! 64-bit packet-sequence reconstruction and the 1024-packet replay window.
//!
//! The wire carries the low 32 bits of the sender's packet number
//! (`pkt_seq`). The receiver reconstructs the full 64-bit number closest to
//! the next expected one (QUIC RFC 9000 §A.3 with a 32-bit window), so the
//! wire value may wrap freely. The full number is the AEAD nonce.
//!
//! The window is updated only after a packet authenticated, so forged
//! packets can never move it. The same bitmap yields `ack`/`ack_bits`.

pub const REPLAY_WINDOW: u64 = 1024;
const WORDS: usize = (REPLAY_WINDOW / 64) as usize;

/// Full sequence from its low 32 bits, the candidate closest to `expected`.
pub fn expand_seq(wire: u32, expected: u64) -> u64 {
    const WIN: u64 = 1 << 32;
    const HWIN: u64 = WIN / 2;
    const MASK: u64 = WIN - 1;
    let candidate = (expected & !MASK) | wire as u64;
    if candidate + HWIN <= expected && candidate < (1u64 << 62) - WIN {
        candidate + WIN
    } else if candidate > expected + HWIN && candidate >= WIN {
        candidate - WIN
    } else {
        candidate
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Check {
    Fresh,
    Duplicate,
    TooOld,
}

#[derive(Clone, Debug, Default)]
pub struct ReplayWindow {
    highest: Option<u64>,
    bits: [u64; WORDS],
}

impl ReplayWindow {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn highest(&self) -> Option<u64> {
        self.highest
    }

    /// The next sequence a well-behaved sender would use (expansion anchor).
    pub fn expected(&self) -> u64 {
        self.highest.map_or(0, |h| h + 1)
    }

    fn idx(seq: u64) -> (usize, u32) {
        let i = (seq % REPLAY_WINDOW) as usize;
        (i / 64, (i % 64) as u32)
    }
    fn bit(&self, seq: u64) -> bool {
        let (w, b) = Self::idx(seq);
        (self.bits[w] >> b) & 1 == 1
    }
    fn set(&mut self, seq: u64) {
        let (w, b) = Self::idx(seq);
        self.bits[w] |= 1 << b;
    }
    fn clear(&mut self, seq: u64) {
        let (w, b) = Self::idx(seq);
        self.bits[w] &= !(1 << b);
    }

    pub fn check(&self, seq: u64) -> Check {
        match self.highest {
            None => Check::Fresh,
            Some(h) if seq > h => Check::Fresh,
            Some(h) if h - seq >= REPLAY_WINDOW => Check::TooOld,
            Some(_) if self.bit(seq) => Check::Duplicate,
            Some(_) => Check::Fresh,
        }
    }

    /// Record `seq`. Returns false (and changes nothing) unless it was fresh.
    pub fn mark(&mut self, seq: u64) -> bool {
        if self.check(seq) != Check::Fresh {
            return false;
        }
        match self.highest {
            Some(h) if seq <= h => {}
            Some(h) => {
                if seq - h >= REPLAY_WINDOW {
                    self.bits = [0; WORDS];
                } else {
                    for s in h + 1..=seq {
                        self.clear(s);
                    }
                }
                self.highest = Some(seq);
            }
            None => {
                self.bits = [0; WORDS];
                self.highest = Some(seq);
            }
        }
        self.set(seq);
        true
    }

    /// `(ack, ack_bits)`: ack = low 32 bits of the highest received;
    /// bit i set = `highest - 1 - i` was received.
    pub fn ack_fields(&self) -> Option<(u32, u32)> {
        let h = self.highest?;
        let mut b = 0u32;
        for i in 0..32u64 {
            if h > i && self.bit(h - 1 - i) {
                b |= 1 << i;
            }
        }
        Some((h as u32, b))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn window_edges() {
        let mut w = ReplayWindow::new();
        assert!(w.mark(2000));
        // Exactly at the far edge of the window: still acceptable.
        assert_eq!(w.check(2000 - (REPLAY_WINDOW - 1)), Check::Fresh);
        assert!(w.mark(2000 - (REPLAY_WINDOW - 1)));
        assert_eq!(w.check(2000 - (REPLAY_WINDOW - 1)), Check::Duplicate);
        // One past the edge: too old, even though never seen.
        assert_eq!(w.check(2000 - REPLAY_WINDOW), Check::TooOld);
        assert!(!w.mark(2000 - REPLAY_WINDOW));
        assert_eq!(w.check(2000), Check::Duplicate);
        assert_eq!(w.check(1999), Check::Fresh);
        // Seq 0 on a fresh window, and the zero sequence itself as a duplicate.
        let mut z = ReplayWindow::new();
        assert!(z.mark(0));
        assert!(!z.mark(0));
        assert_eq!(z.ack_fields(), Some((0, 0)));
    }

    #[test]
    fn advancing_clears_stale_bits() {
        let mut w = ReplayWindow::new();
        for s in 0..100 {
            assert!(w.mark(s));
        }
        // Jump exactly one window: old slot indices are reused for new seqs.
        assert!(w.mark(99 + REPLAY_WINDOW));
        for s in 100..99 + REPLAY_WINDOW {
            assert_eq!(w.check(s), Check::Fresh, "seq {s} must not inherit an old bit");
        }
        // Jump more than a window: everything older is too old.
        assert!(w.mark(10 * REPLAY_WINDOW));
        assert_eq!(w.check(10 * REPLAY_WINDOW - REPLAY_WINDOW), Check::TooOld);
        assert_eq!(w.check(10 * REPLAY_WINDOW - 1), Check::Fresh);
    }

    #[test]
    fn ack_bits_reflect_receipt() {
        let mut w = ReplayWindow::new();
        for s in [10u64, 12, 13, 15] {
            w.mark(s);
        }
        let (ack, bits) = w.ack_fields().unwrap();
        assert_eq!(ack, 15);
        // bit0 = 14 (missing), bit1 = 13, bit2 = 12, bit3 = 11 (missing), bit4 = 10
        assert_eq!(bits, 0b10110);
    }

    #[test]
    fn expand_handles_u32_wrap() {
        let lap = 1u64 << 32;
        // Near the top of lap 0, a small wire value is the next lap.
        assert_eq!(expand_seq(3, lap - 5), lap + 3);
        // Just after the wrap, a big wire value is late traffic from lap 0.
        assert_eq!(expand_seq(u32::MAX - 2, lap + 4), lap - 3);
        // Normal forward and backward cases.
        assert_eq!(expand_seq(100, 90), 100);
        assert_eq!(expand_seq(80, 90), 80);
        assert_eq!(expand_seq(0, 0), 0);
        assert_eq!(expand_seq(u32::MAX, 0), u32::MAX as u64);
    }

    #[test]
    fn window_works_across_wire_wraparound() {
        let mut w = ReplayWindow::new();
        let start = (1u64 << 32) - 600;
        for s in start..start + 1200 {
            let wire = s as u32;
            let full = expand_seq(wire, w.expected());
            assert_eq!(full, s);
            assert!(w.mark(full));
        }
        // Replays of pre-wrap and post-wrap packets are rejected.
        for s in [start + 700, (1u64 << 32) - 1, 1u64 << 32, start + 1199] {
            let full = expand_seq(s as u32, w.expected());
            assert_eq!(full, s);
            assert_ne!(w.check(full), Check::Fresh, "replay of {s} accepted");
        }
        // Something more than a window behind the newest is too old.
        let old = start + 1199 - REPLAY_WINDOW;
        assert_eq!(w.check(expand_seq(old as u32, w.expected())), Check::TooOld);
        // Out-of-order fresh traffic inside the window is still accepted.
        // (A real receiver starts at seq 0, so prime the window in lap 0.)
        let mut v = ReplayWindow::new();
        let base = (1u64 << 32) - 3;
        assert!(v.mark(base - 100));
        for s in [base + 5, base, base + 2, base + 4, base + 1, base + 3] {
            let full = expand_seq(s as u32, v.expected());
            assert_eq!(full, s);
            assert!(v.mark(full));
        }
        let (ack, bits) = v.ack_fields().unwrap();
        assert_eq!(ack as u64, (base + 5) & 0xFFFF_FFFF);
        assert_eq!(bits & 0b11111, 0b11111);
    }
}
