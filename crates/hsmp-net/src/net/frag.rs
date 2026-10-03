//! Fragmentation of reliable messages and bounded reassembly.
//!
//! A reliable message larger than one chunk is split into 2..=64 fragments
//! (`FRAG` chunks). The receiver reassembles per `(channel, msg_id)` within
//! hard limits; any violation is a protocol error (the peer is authenticated,
//! so only a broken or hostile peer can trip them).

use std::collections::HashMap;

pub const MAX_FRAGS: usize = 64;
/// Largest reliable message after reassembly.
pub const MAX_MESSAGE: usize = 64 * 1024;
/// Concurrent partially received messages per connection.
pub const MAX_PARTIALS: usize = 64;
/// Bytes held in partial messages per connection.
pub const MAX_REASM_BYTES: usize = 512 * 1024;
/// A partial message untouched for this long is discarded.
pub const PARTIAL_TTL_MS: u64 = 30_000;
/// When the partial table is full, a channel-1 partial idle this long may be
/// evicted to make room: channel 1 carries `ReliableLatest`, whose sender
/// dropped a superseded message (it never sent the rest), so its partial
/// would otherwise sit out the whole TTL and, under churn and loss, push an
/// honest peer over MAX_PARTIALS (a protocol violation).
/// A current sender never abandons a message it has started to
/// send, so orphans only come from older builds; for current peers the
/// table fills only with live messages and this eviction does not trigger
/// short of 64 concurrent fragmented messages.
pub const EVICT_IDLE_MS: u64 = 5_000;
const EVICTABLE_CH: u8 = super::channel::CH_RELIABLE;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FragError {
    BadCount,
    BadIndex,
    Empty,
    CountMismatch,
    TooLarge,
    TooManyPartials,
    BufferFull,
}

/// Split `data` into pieces of at most `frag_size` bytes.
pub fn split(data: &[u8], frag_size: usize) -> Vec<Vec<u8>> {
    data.chunks(frag_size.max(1)).map(|c| c.to_vec()).collect()
}

struct Partial {
    count: u8,
    have: u64,
    got: u8,
    parts: Vec<Option<Vec<u8>>>,
    bytes: usize,
    touched: u64,
}

#[derive(Default)]
pub struct Reassembler {
    partial: HashMap<(u8, u32), Partial>,
    bytes: usize,
}

impl Reassembler {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn partials(&self) -> usize {
        self.partial.len()
    }

    pub fn bytes(&self) -> usize {
        self.bytes
    }

    pub fn contains(&self, ch: u8, id: u32) -> bool {
        self.partial.contains_key(&(ch, id))
    }

    /// Add one fragment. `Ok(Some(msg))` when the message is complete,
    /// `Ok(None)` when more fragments are needed or this one is a duplicate.
    /// On error nothing is changed.
    pub fn insert(&mut self, now: u64, ch: u8, id: u32, idx: u8, count: u8, data: &[u8]) -> Result<Option<Vec<u8>>, FragError> {
        if count < 2 || count as usize > MAX_FRAGS {
            return Err(FragError::BadCount);
        }
        if idx >= count {
            return Err(FragError::BadIndex);
        }
        if data.is_empty() {
            return Err(FragError::Empty);
        }
        if data.len() > MAX_MESSAGE {
            return Err(FragError::TooLarge);
        }
        match self.partial.get(&(ch, id)) {
            Some(p) => {
                if p.count != count {
                    return Err(FragError::CountMismatch);
                }
                if p.have & (1u64 << idx) != 0 {
                    let p = self.partial.get_mut(&(ch, id)).expect("present");
                    p.touched = now;
                    return Ok(None);
                }
                if p.bytes + data.len() > MAX_MESSAGE {
                    return Err(FragError::TooLarge);
                }
                if self.bytes + data.len() > MAX_REASM_BYTES {
                    return Err(FragError::BufferFull);
                }
            }
            None => {
                if self.partial.len() >= MAX_PARTIALS || self.bytes + data.len() > MAX_REASM_BYTES {
                    self.evict_idle(now, data.len());
                }
                if self.partial.len() >= MAX_PARTIALS {
                    return Err(FragError::TooManyPartials);
                }
                if self.bytes + data.len() > MAX_REASM_BYTES {
                    return Err(FragError::BufferFull);
                }
                self.partial.insert(
                    (ch, id),
                    Partial {
                        count,
                        have: 0,
                        got: 0,
                        parts: vec![None; count as usize],
                        bytes: 0,
                        touched: now,
                    },
                );
            }
        }
        let p = self.partial.get_mut(&(ch, id)).expect("inserted above");
        p.have |= 1u64 << idx;
        p.got += 1;
        p.bytes += data.len();
        p.touched = now;
        p.parts[idx as usize] = Some(data.to_vec());
        self.bytes += data.len();
        if p.got < p.count {
            return Ok(None);
        }
        let p = self.partial.remove(&(ch, id)).expect("present");
        self.bytes -= p.bytes;
        let mut msg = Vec::with_capacity(p.bytes);
        for part in p.parts.into_iter().flatten() {
            msg.extend_from_slice(&part);
        }
        Ok(Some(msg))
    }

    /// Make room for a new partial of `incoming` bytes by evicting the
    /// oldest channel-1 partials idle for at least EVICT_IDLE_MS (superseded
    /// ReliableLatest messages). Channel-2 partials are never evicted.
    fn evict_idle(&mut self, now: u64, incoming: usize) {
        self.evict_idle_for(now, 1, incoming);
    }

    /// Evict idle channel-1 partials until `new_partials` more partials and
    /// `incoming` more bytes fit (or nothing evictable is left).
    pub(crate) fn evict_idle_for(&mut self, now: u64, new_partials: usize, incoming: usize) {
        while self.partial.len() + new_partials > MAX_PARTIALS || self.bytes + incoming > MAX_REASM_BYTES {
            let victim = self
                .partial
                .iter()
                .filter(|((ch, _), p)| *ch == EVICTABLE_CH && now.saturating_sub(p.touched) >= EVICT_IDLE_MS)
                .min_by_key(|(_, p)| p.touched)
                .map(|(k, _)| *k);
            let Some(k) = victim else { return };
            if let Some(p) = self.partial.remove(&k) {
                self.bytes -= p.bytes;
            }
        }
    }

    /// Drop partial messages idle for `PARTIAL_TTL_MS`.
    pub fn expire(&mut self, now: u64) {
        let mut freed = 0;
        self.partial.retain(|_, p| {
            let keep = now.saturating_sub(p.touched) < PARTIAL_TTL_MS;
            if !keep {
                freed += p.bytes;
            }
            keep
        });
        self.bytes -= freed;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reassembles_in_any_order_with_duplicates() {
        let msg: Vec<u8> = (0..5000u32).map(|i| (i * 7) as u8).collect();
        let parts = split(&msg, 1152);
        assert_eq!(parts.len(), 5);
        let mut r = Reassembler::new();
        let order = [3usize, 0, 4, 0, 1, 3, 2];
        let mut out = None;
        for (n, &i) in order.iter().enumerate() {
            let res = r.insert(n as u64, 1, 9, i as u8, 5, &parts[i]).unwrap();
            if res.is_some() {
                out = res;
            }
        }
        assert_eq!(out.unwrap(), msg);
        assert_eq!(r.partials(), 0);
        assert_eq!(r.bytes(), 0);
    }

    #[test]
    fn rejects_malformed_fragments_without_side_effects() {
        let mut r = Reassembler::new();
        assert_eq!(r.insert(0, 1, 1, 0, 1, b"x"), Err(FragError::BadCount));
        assert_eq!(r.insert(0, 1, 1, 0, 65, b"x"), Err(FragError::BadCount));
        assert_eq!(r.insert(0, 1, 1, 3, 3, b"x"), Err(FragError::BadIndex));
        assert_eq!(r.insert(0, 1, 1, 0, 3, b""), Err(FragError::Empty));
        assert_eq!(r.partials(), 0);
        assert_eq!(r.insert(0, 1, 1, 0, 3, b"abc"), Ok(None));
        assert_eq!(r.insert(0, 1, 1, 1, 4, b"abc"), Err(FragError::CountMismatch));
        assert_eq!(r.bytes(), 3);
    }

    #[test]
    fn enforces_message_and_buffer_limits() {
        let mut r = Reassembler::new();
        let big = vec![1u8; 40 * 1024];
        assert_eq!(r.insert(0, 2, 1, 0, 2, &big), Ok(None));
        // Second half would push the message past MAX_MESSAGE.
        assert_eq!(r.insert(0, 2, 1, 1, 2, &big), Err(FragError::TooLarge));
        // Fill the buffer with distinct partial messages.
        let chunk = vec![2u8; 60 * 1024];
        let mut id = 2;
        loop {
            match r.insert(0, 2, id, 0, 2, &chunk) {
                Ok(None) => id += 1,
                Err(FragError::BufferFull) => break,
                other => panic!("unexpected {other:?}"),
            }
        }
        assert!(r.bytes() <= MAX_REASM_BYTES);
        // Expiry frees everything.
        r.expire(PARTIAL_TTL_MS + 1);
        assert_eq!((r.partials(), r.bytes()), (0, 0));
    }

    #[test]
    fn caps_concurrent_partials() {
        let mut r = Reassembler::new();
        for id in 0..MAX_PARTIALS as u32 {
            assert_eq!(r.insert(0, 1, id, 0, 2, b"a"), Ok(None));
        }
        assert_eq!(r.insert(0, 1, 999, 0, 2, b"a"), Err(FragError::TooManyPartials));
        // Completing one frees a slot.
        assert_eq!(r.insert(0, 1, 0, 1, 2, b"b"), Ok(Some(b"ab".to_vec())));
        assert_eq!(r.insert(0, 1, 999, 0, 2, b"a"), Ok(None));
    }

    /// Superseded channel-1 partials (the sender never sends the
    /// rest) do not push an honest peer over MAX_PARTIALS; idle ones are
    /// evicted. Channel-2 partials are never evicted (still an error).
    #[test]
    fn idle_superseded_partials_are_evicted_not_fatal() {
        let mut r = Reassembler::new();
        for id in 0..MAX_PARTIALS as u32 {
            assert_eq!(r.insert(0, 1, id, 0, 2, b"a"), Ok(None));
        }
        // Fresh: no room yet (an attack, or real concurrency).
        assert_eq!(r.insert(100, 1, 999, 0, 2, b"a"), Err(FragError::TooManyPartials));
        // Touch one so it is not the oldest; idle ones make room, oldest first.
        assert_eq!(r.insert(1, 1, 5, 0, 2, b"a"), Ok(None)); // duplicate: refreshes `touched`
        assert_eq!(r.insert(EVICT_IDLE_MS, 1, 999, 0, 2, b"a"), Ok(None));
        assert_eq!(r.partials(), MAX_PARTIALS);
        assert!(r.contains(1, 999) && r.contains(1, 5));
        // Ordered channel: never evicted.
        let mut r = Reassembler::new();
        for id in 0..MAX_PARTIALS as u32 {
            assert_eq!(r.insert(0, 2, id, 0, 2, b"a"), Ok(None));
        }
        assert_eq!(r.insert(EVICT_IDLE_MS * 2, 2, 999, 0, 2, b"a"), Err(FragError::TooManyPartials));
    }
}
