//! Channels and the chunk codec.
//!
//! * Channel 0, unreliable-latest: messages carry a `key` (stream x entity).
//!   The sender keeps only the newest unsent message per key; the receiver
//!   drops a message older (by packet number) than the newest it delivered
//!   for that key. Never fragmented, never retransmitted.
//! * Channel 1, reliable-unordered: delivered once, in arrival order.
//!   `ReliableLatest{key}` cancels retransmission of an older unacked
//!   message with the same key (used for idempotent snapshots).
//! * Channel 2, reliable-ordered: delivered once, in send order.
//!
//! Reliable messages get a per-channel `msg_id`. At most `SEND_WINDOW`
//! ids beyond the oldest unacked one, and at most `MAX_INFLIGHT_BYTES`, are
//! in flight; the rest waits in the queue.

use std::collections::{BTreeMap, HashMap, VecDeque};

use super::frag::{self, FragError, Reassembler, MAX_FRAGS, MAX_MESSAGE};
use super::replay::{Check, ReplayWindow};
use super::wire::{Put, Reader};
use super::MAX_PLAINTEXT;

pub const CH_UNRELIABLE: u8 = 0;
pub const CH_RELIABLE: u8 = 1;
pub const CH_ORDERED: u8 = 2;

pub const SEND_WINDOW: u32 = 256;
pub const RECV_WINDOW: u32 = 1024;
pub const MAX_INFLIGHT_BYTES: usize = 256 * 1024;
pub const MAX_QUEUED_BYTES: usize = 1 << 20;
pub const MAX_ORDERED_BUFFER: usize = 1 << 20;
pub const MAX_LATEST_QUEUE: usize = 1024;
pub const MAX_LATEST_KEYS: usize = 4096;

// Chunk kinds (first byte of each chunk in the decrypted payload).
pub const CK_PADDING: u8 = 0x00;
pub const CK_UNREL: u8 = 0x01;
pub const CK_REL: u8 = 0x02;
pub const CK_FRAG: u8 = 0x03;
pub const CK_PING: u8 = 0x04;
pub const CK_CLOSE: u8 = 0x05;
/// `u16` ms the acked packet (the header's `ack`) waited at its receiver
/// before this packet carried the ack. Only sent when `caps::ACK_DELAY` was
/// negotiated; not ack-eliciting.
pub const CK_ACK_DELAY: u8 = 0x06;
/// 16-byte stateless-reset token (server → client, `caps::RESET`).
pub const CK_RESET_TOKEN: u8 = 0x07;
/// 8 random bytes the receiver must echo in a `PATH_RESPONSE` from the
/// address it was sent to (path validation, `caps::PATH_CHALLENGE`).
/// Ack-eliciting.
pub const CK_PATH_CHALLENGE: u8 = 0x08;
/// Echo of a `PATH_CHALLENGE`'s 8 bytes. Ack-eliciting.
pub const CK_PATH_RESPONSE: u8 = 0x09;
pub const ACK_DELAY_LEN: usize = 3;
pub const RESET_TOKEN_LEN: usize = 17;
/// Channel-byte flag of a keyed REL/FRAG chunk (`caps::REL_KEY`): a u32
/// supersede key follows `msg_id`. Channel 1 only.
pub const CH_KEYED: u8 = 0x80;
pub const KEY_LEN: usize = 4;
/// Supersede keys a receiver tracks (cleared when full, like channel 0).
pub const MAX_REL_KEYS: usize = 4096;
/// Chunk length of PATH_CHALLENGE / PATH_RESPONSE (kind + 8 bytes).
pub const PATH_TOKEN_LEN: usize = 9;
/// Fragmented reliable messages a sender keeps started but incomplete at
/// once (both channels together). The receiver holds at most
/// `frag::MAX_PARTIALS` (64) partials, so an honest sender can never push it
/// over, however the network drops fragments.
pub const MAX_FRAG_INFLIGHT: usize = 32;

pub const UNREL_HDR: usize = 1 + 4 + 2;
pub const REL_HDR: usize = 1 + 1 + 4 + 2;
pub const FRAG_HDR: usize = 1 + 1 + 4 + 1 + 1 + 2;
/// Largest channel-0 message (1155 B).
pub const MAX_UNRELIABLE: usize = MAX_PLAINTEXT - UNREL_HDR;
/// Largest reliable message sent without fragmentation (1154 B).
pub const MAX_SINGLE: usize = MAX_PLAINTEXT - REL_HDR;
/// Fragment payload size (1152 B).
pub const FRAG_DATA: usize = MAX_PLAINTEXT - FRAG_HDR;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SendMode {
    /// Channel 0; newest message per key wins.
    Latest { key: u32 },
    /// Channel 1.
    Reliable,
    /// Channel 1; supersedes older unacked messages with the same key.
    ReliableLatest { key: u32 },
    /// Channel 2.
    Ordered,
}

impl SendMode {
    pub fn channel(self) -> u8 {
        match self {
            SendMode::Latest { .. } => CH_UNRELIABLE,
            SendMode::Reliable | SendMode::ReliableLatest { .. } => CH_RELIABLE,
            SendMode::Ordered => CH_ORDERED,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Delivery {
    pub channel: u8,
    /// Channel-0 key; 0 for reliable channels.
    pub key: u32,
    pub data: Vec<u8>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SendError {
    Empty,
    TooLarge,
    Backpressure,
    Closed,
    NotConnected,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Violation {
    Malformed,
    UnknownChunk,
    BadChannel,
    WindowExceeded,
    BufferFull,
    Frag(FragError),
}

// ---------------------------------------------------------------------------
// Send side
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum FragState {
    Unsent,
    InFlight { pkt: u64 },
    Acked,
}

pub(crate) struct OutFrag {
    data: Vec<u8>,
    state: FragState,
    sends: u32,
}

pub(crate) struct OutMsg {
    frags: Vec<OutFrag>,
    acked: usize,
    key: Option<u32>,
    /// The key travels on the wire (keyed chunks, caps::REL_KEY).
    wire_key: Option<u32>,
    started: bool,
    bytes: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct FragRef {
    pub ch: u8,
    pub id: u32,
    pub idx: u8,
}

#[derive(Default)]
struct ReliableOut {
    next_id: u32,
    msgs: BTreeMap<u32, OutMsg>,
    started_bytes: usize,
}

impl ReliableOut {
    fn base(&self) -> u32 {
        self.msgs.keys().next().copied().unwrap_or(self.next_id)
    }
    fn remove(&mut self, id: u32) -> Option<OutMsg> {
        let m = self.msgs.remove(&id)?;
        if m.started {
            self.started_bytes -= m.bytes;
        }
        Some(m)
    }
}

#[derive(Default)]
pub(crate) struct Outbox {
    rel: [ReliableOut; 2],
    latest: VecDeque<(u32, Vec<u8>)>,
    queued_bytes: usize,
    pub retransmits: u64,
    /// `caps::REL_KEY` negotiated: ReliableLatest chunks carry their key.
    pub keyed: bool,
}

fn rel_index(ch: u8) -> usize {
    (ch - 1) as usize
}

impl Outbox {
    pub fn send(&mut self, mode: SendMode, data: Vec<u8>) -> Result<(), SendError> {
        if data.is_empty() {
            return Err(SendError::Empty);
        }
        match mode {
            SendMode::Latest { key } => {
                if data.len() > MAX_UNRELIABLE {
                    return Err(SendError::TooLarge);
                }
                if let Some(slot) = self.latest.iter_mut().find(|(k, _)| *k == key) {
                    slot.1 = data;
                } else {
                    if self.latest.len() >= MAX_LATEST_QUEUE {
                        self.latest.pop_front();
                    }
                    self.latest.push_back((key, data));
                }
                Ok(())
            }
            _ => {
                if data.len() > MAX_MESSAGE {
                    return Err(SendError::TooLarge);
                }
                if self.queued_bytes + data.len() > MAX_QUEUED_BYTES {
                    return Err(SendError::Backpressure);
                }
                let ch = mode.channel();
                let key = match mode {
                    SendMode::ReliableLatest { key } => Some(key),
                    _ => None,
                };
                let r = &mut self.rel[rel_index(ch)];
                if let Some(key) = key {
                    // A fragmented message whose first fragments are already
                    // on the wire is finished, not abandoned: the receiver
                    // cannot tell an abandoned partial from a plain Reliable
                    // one still being retransmitted, so orphans would force
                    // it to evict blindly.
                    let old: Vec<u32> = r
                        .msgs
                        .iter()
                        .filter(|(_, m)| m.key == Some(key) && !(m.started && m.frags.len() > 1))
                        .map(|(id, _)| *id)
                        .collect();
                    for id in old {
                        if let Some(m) = r.remove(id) {
                            self.queued_bytes -= m.bytes;
                        }
                    }
                }
                let wire_key = key.filter(|_| self.keyed);
                let extra = if wire_key.is_some() { KEY_LEN } else { 0 };
                let out_frag = |d| OutFrag {
                    data: d,
                    state: FragState::Unsent,
                    sends: 0,
                };
                let bytes = data.len();
                let frags: Vec<OutFrag> = if data.len() <= MAX_SINGLE - extra {
                    vec![out_frag(data)]
                } else {
                    frag::split(&data, FRAG_DATA - extra).into_iter().map(out_frag).collect()
                };
                debug_assert!(frags.len() <= MAX_FRAGS);
                let id = r.next_id;
                r.next_id = r.next_id.wrapping_add(1);
                r.msgs.insert(
                    id,
                    OutMsg {
                        frags,
                        acked: 0,
                        key,
                        wire_key,
                        started: false,
                        bytes,
                    },
                );
                self.queued_bytes += bytes;
                Ok(())
            }
        }
    }

    pub fn reliable_pending(&self) -> usize {
        self.rel.iter().map(|r| r.msgs.len()).sum()
    }

    pub fn latest_pending(&self) -> usize {
        self.latest.len()
    }

    pub fn on_frag_acked(&mut self, f: FragRef) {
        if f.ch != CH_RELIABLE && f.ch != CH_ORDERED {
            return;
        }
        let r = &mut self.rel[rel_index(f.ch)];
        let Some(m) = r.msgs.get_mut(&f.id) else { return };
        let Some(fr) = m.frags.get_mut(f.idx as usize) else { return };
        if fr.state == FragState::Acked {
            return;
        }
        fr.state = FragState::Acked;
        m.acked += 1;
        if m.acked == m.frags.len() {
            if let Some(m) = r.remove(f.id) {
                self.queued_bytes -= m.bytes;
            }
        }
    }

    pub fn on_frag_lost(&mut self, f: FragRef, pkt: u64) {
        if f.ch != CH_RELIABLE && f.ch != CH_ORDERED {
            return;
        }
        let r = &mut self.rel[rel_index(f.ch)];
        let Some(m) = r.msgs.get_mut(&f.id) else { return };
        let Some(fr) = m.frags.get_mut(f.idx as usize) else { return };
        if fr.state == (FragState::InFlight { pkt }) {
            fr.state = FragState::Unsent;
        }
    }

    /// True if any reliable fragment waits to be (re)sent.
    pub fn reliable_unsent(&self) -> bool {
        self.rel
            .iter()
            .any(|r| r.msgs.values().any(|m| m.frags.iter().any(|f| f.state == FragState::Unsent)))
    }

    /// Started, incomplete fragmented messages (both channels).
    fn frag_inflight(&self) -> usize {
        self.rel
            .iter()
            .map(|r| r.msgs.values().filter(|m| m.started && m.frags.len() > 1 && m.acked < m.frags.len()).count())
            .sum()
    }

    /// Append chunks to `buf` (up to `MAX_PLAINTEXT` total) for packet `pkt`.
    /// Returns the reliable fragments written.
    ///
    /// Channel 0 (Latest: pose, root, vitals) goes first: under
    /// pacing it must not wait behind a reliable backlog. Its queue holds only
    /// the newest message per key, so it never starves the reliable channels
    /// for long. Then channel 2, then channel 1.
    pub fn fill(&mut self, buf: &mut Vec<u8>, pkt: u64) -> Vec<FragRef> {
        // In queue order; what does not fit stays queued.
        self.latest.retain(|(key, data)| {
            if buf.len() + UNREL_HDR + data.len() <= MAX_PLAINTEXT {
                write_unrel(buf, *key, data);
                false
            } else {
                true
            }
        });
        let mut refs = Vec::new();
        let mut frag_inflight = self.frag_inflight();
        for ch in [CH_ORDERED, CH_RELIABLE] {
            let r = &mut self.rel[rel_index(ch)];
            let limit = r.base() as u64 + SEND_WINDOW as u64;
            let mut started_bytes = r.started_bytes;
            for (&id, m) in r.msgs.iter_mut() {
                if id as u64 >= limit {
                    break;
                }
                if !m.started && started_bytes > 0 && started_bytes + m.bytes > MAX_INFLIGHT_BYTES {
                    break;
                }
                let count = m.frags.len();
                // At most MAX_FRAG_INFLIGHT fragmented messages started
                // and incomplete; a new one waits (later single-chunk
                // messages still go: the receiver reorders channel 2).
                if !m.started && count > 1 && frag_inflight >= MAX_FRAG_INFLIGHT {
                    continue;
                }
                for (idx, fr) in m.frags.iter_mut().enumerate() {
                    if fr.state != FragState::Unsent {
                        continue;
                    }
                    let kx = if m.wire_key.is_some() { KEY_LEN } else { 0 };
                    let need = if count == 1 { REL_HDR } else { FRAG_HDR } + kx + fr.data.len();
                    if buf.len() + need > MAX_PLAINTEXT {
                        continue;
                    }
                    if count == 1 {
                        write_rel_k(buf, ch, id, m.wire_key, &fr.data);
                    } else {
                        write_frag_k(buf, ch, id, m.wire_key, idx as u8, count as u8, &fr.data);
                    }
                    if fr.sends > 0 {
                        self.retransmits += 1;
                    }
                    fr.sends += 1;
                    fr.state = FragState::InFlight { pkt };
                    refs.push(FragRef { ch, id, idx: idx as u8 });
                    if !m.started {
                        m.started = true;
                        started_bytes += m.bytes;
                        if count > 1 {
                            frag_inflight += 1;
                        }
                    }
                }
                if buf.len() + REL_HDR >= MAX_PLAINTEXT {
                    break;
                }
            }
            r.started_bytes = started_bytes;
        }
        refs
    }
}

pub(crate) fn write_unrel(buf: &mut Vec<u8>, key: u32, data: &[u8]) {
    buf.put_u8(CK_UNREL);
    buf.put_u32(key);
    buf.put_u16(data.len() as u16);
    buf.put(data);
}

pub(crate) fn write_rel(buf: &mut Vec<u8>, ch: u8, id: u32, data: &[u8]) {
    buf.put_u8(CK_REL);
    buf.put_u8(ch);
    buf.put_u32(id);
    buf.put_u16(data.len() as u16);
    buf.put(data);
}

/// REL chunk, keyed when `key` is Some: `ch | CH_KEYED` and the u32 key
/// right after `msg_id`.
pub(crate) fn write_rel_k(buf: &mut Vec<u8>, ch: u8, id: u32, key: Option<u32>, data: &[u8]) {
    let Some(k) = key else { return write_rel(buf, ch, id, data) };
    buf.put_u8(CK_REL);
    buf.put_u8(ch | CH_KEYED);
    buf.put_u32(id);
    buf.put_u32(k);
    buf.put_u16(data.len() as u16);
    buf.put(data);
}

/// FRAG chunk, keyed when `key` is Some.
pub(crate) fn write_frag_k(buf: &mut Vec<u8>, ch: u8, id: u32, key: Option<u32>, idx: u8, count: u8, data: &[u8]) {
    let Some(k) = key else { return write_frag(buf, ch, id, idx, count, data) };
    buf.put_u8(CK_FRAG);
    buf.put_u8(ch | CH_KEYED);
    buf.put_u32(id);
    buf.put_u32(k);
    buf.put_u8(idx);
    buf.put_u8(count);
    buf.put_u16(data.len() as u16);
    buf.put(data);
}

pub(crate) fn write_frag(buf: &mut Vec<u8>, ch: u8, id: u32, idx: u8, count: u8, data: &[u8]) {
    buf.put_u8(CK_FRAG);
    buf.put_u8(ch);
    buf.put_u32(id);
    buf.put_u8(idx);
    buf.put_u8(count);
    buf.put_u16(data.len() as u16);
    buf.put(data);
}

/// The channel byte of a REL/FRAG chunk: (channel, keyed?). A keyed chunk
/// is valid on channel 1 only.
fn read_ch_key(r: &mut Reader<'_>) -> Result<(u8, Option<()>), Violation> {
    let b = r.u8().map_err(|_| Violation::Malformed)?;
    if b & CH_KEYED == 0 {
        return Ok((b, None));
    }
    let ch = b & !CH_KEYED;
    if ch != CH_RELIABLE {
        return Err(Violation::BadChannel);
    }
    Ok((ch, Some(())))
}

pub(crate) fn write_close(buf: &mut Vec<u8>, code: u8, reason: &str) {
    let r = truncate_utf8(reason, 100);
    buf.put_u8(CK_CLOSE);
    buf.put_u8(code);
    buf.put_u8(r.len() as u8);
    buf.put(r.as_bytes());
}

pub(crate) fn truncate_utf8(s: &str, max: usize) -> &str {
    if s.len() <= max {
        return s;
    }
    let mut end = max;
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    &s[..end]
}

// ---------------------------------------------------------------------------
// Receive side
// ---------------------------------------------------------------------------

#[derive(Debug, Default)]
pub(crate) struct PayloadInfo {
    pub eliciting: bool,
    pub close: Option<(u8, String)>,
    pub ack_delay_ms: Option<u16>,
    pub reset_token: Option<[u8; 16]>,
    pub path_challenge: Option<[u8; 8]>,
    pub path_response: Option<[u8; 8]>,
    /// The payload was NOT processed: its fragments would push the
    /// reassembly table over its limits. The packet must not be acked,
    /// so the sender retransmits its reliable content later.
    pub deferred: bool,
}

#[derive(Default)]
pub(crate) struct Inbox {
    ch1: ReplayWindow,
    ch2_next: u32,
    ch2_buf: BTreeMap<u32, Vec<u8>>,
    ch2_bytes: usize,
    reasm: Reassembler,
    latest: HashMap<u32, u64>,
    /// Keyed channel-1 messages: highest msg_id delivered per key.
    rel_keys: HashMap<u32, u32>,
}

impl Inbox {
    fn is_dup(&self, ch: u8, id: u32) -> Result<bool, Violation> {
        match ch {
            CH_RELIABLE => Ok(self.ch1.check(id as u64) != Check::Fresh),
            CH_ORDERED => {
                if id < self.ch2_next || self.ch2_buf.contains_key(&id) {
                    Ok(true)
                } else if id - self.ch2_next >= RECV_WINDOW {
                    Err(Violation::WindowExceeded)
                } else {
                    Ok(false)
                }
            }
            _ => Err(Violation::BadChannel),
        }
    }

    fn on_reliable(&mut self, ch: u8, id: u32, key: Option<u32>, data: Vec<u8>, out: &mut Vec<Delivery>) -> Result<(), Violation> {
        match ch {
            CH_RELIABLE => {
                // An older copy of a superseded keyed message that
                // arrives after a newer one is consumed, not delivered.
                let stale = key.is_some_and(|k| self.rel_keys.get(&k).is_some_and(|&hi| id < hi));
                if self.ch1.mark(id as u64) && !stale {
                    if let Some(k) = key {
                        if self.rel_keys.len() >= MAX_REL_KEYS && !self.rel_keys.contains_key(&k) {
                            self.rel_keys.clear();
                        }
                        self.rel_keys.insert(k, id);
                    }
                    out.push(Delivery {
                        channel: CH_RELIABLE,
                        key: 0,
                        data,
                    });
                }
            }
            CH_ORDERED => {
                if self.is_dup(ch, id)? {
                    return Ok(());
                }
                if self.ch2_bytes + data.len() > MAX_ORDERED_BUFFER {
                    return Err(Violation::BufferFull);
                }
                self.ch2_bytes += data.len();
                self.ch2_buf.insert(id, data);
                while let Some(d) = self.ch2_buf.remove(&self.ch2_next) {
                    self.ch2_bytes -= d.len();
                    self.ch2_next = self.ch2_next.wrapping_add(1);
                    out.push(Delivery {
                        channel: CH_ORDERED,
                        key: 0,
                        data: d,
                    });
                }
            }
            _ => return Err(Violation::BadChannel),
        }
        Ok(())
    }

    pub fn expire(&mut self, now: u64) {
        self.reasm.expire(now);
    }

    /// Would the fragments in `payload` fit the reassembly limits (partials,
    /// bytes)? A fragment that starts a new partial counts once per message.
    /// Malformed payloads answer true: the real parse reports them.
    fn frags_fit(&mut self, now: u64, payload: &[u8]) -> bool {
        let mut r = Reader::new(payload);
        let mut new_ids: Vec<(u8, u32)> = Vec::new();
        let mut bytes = 0usize;
        let scan = (|| -> Option<()> {
            while !r.is_empty() {
                match r.u8().ok()? {
                    CK_PADDING | CK_CLOSE => break,
                    CK_UNREL => {
                        r.u32().ok()?;
                        let n = r.u16().ok()? as usize;
                        r.bytes(n).ok()?;
                    }
                    CK_REL => {
                        let chb = r.u8().ok()?;
                        r.u32().ok()?;
                        if chb & CH_KEYED != 0 {
                            r.u32().ok()?;
                        }
                        let n = r.u16().ok()? as usize;
                        r.bytes(n).ok()?;
                    }
                    CK_FRAG => {
                        let chb = r.u8().ok()?;
                        let ch = chb & !CH_KEYED;
                        let id = r.u32().ok()?;
                        if chb & CH_KEYED != 0 {
                            r.u32().ok()?;
                        }
                        r.u8().ok()?;
                        r.u8().ok()?;
                        let n = r.u16().ok()? as usize;
                        r.bytes(n).ok()?;
                        if self.is_dup(ch, id) != Ok(false) {
                            continue;
                        }
                        bytes += n;
                        if !self.reasm.contains(ch, id) && !new_ids.contains(&(ch, id)) {
                            new_ids.push((ch, id));
                        }
                    }
                    CK_PING => {}
                    CK_ACK_DELAY => {
                        r.u16().ok()?;
                    }
                    CK_RESET_TOKEN => {
                        r.bytes(16).ok()?;
                    }
                    CK_PATH_CHALLENGE | CK_PATH_RESPONSE => {
                        r.bytes(8).ok()?;
                    }
                    _ => return None,
                }
            }
            Some(())
        })();
        if scan.is_none() || (new_ids.is_empty() && bytes == 0) {
            return true;
        }
        let fits = |rs: &Reassembler| {
            rs.partials() + new_ids.len() <= frag::MAX_PARTIALS && rs.bytes() + bytes <= frag::MAX_REASM_BYTES
        };
        if fits(&self.reasm) {
            return true;
        }
        self.reasm.evict_idle_for(now, new_ids.len(), bytes);
        fits(&self.reasm)
    }

    /// Parse a decrypted payload of packet `pkt`, appending deliveries.
    pub fn on_payload(&mut self, now: u64, pkt: u64, payload: &[u8], out: &mut Vec<Delivery>) -> Result<PayloadInfo, Violation> {
        let mut info = PayloadInfo::default();
        if !self.frags_fit(now, payload) {
            info.deferred = true;
            return Ok(info);
        }
        let mut r = Reader::new(payload);
        let m = |_| Violation::Malformed;
        while !r.is_empty() {
            let kind = r.u8().map_err(m)?;
            match kind {
                CK_PADDING => break,
                CK_UNREL => {
                    info.eliciting = true;
                    let key = r.u32().map_err(m)?;
                    let len = r.u16().map_err(m)? as usize;
                    let data = r.bytes(len).map_err(m)?;
                    if len == 0 {
                        return Err(Violation::Malformed);
                    }
                    let fresh = self.latest.get(&key).is_none_or(|&p| pkt > p);
                    if fresh {
                        if self.latest.len() >= MAX_LATEST_KEYS && !self.latest.contains_key(&key) {
                            self.latest.clear();
                        }
                        self.latest.insert(key, pkt);
                        out.push(Delivery {
                            channel: CH_UNRELIABLE,
                            key,
                            data: data.to_vec(),
                        });
                    }
                }
                CK_REL => {
                    info.eliciting = true;
                    let (ch, key) = read_ch_key(&mut r)?;
                    let id = r.u32().map_err(m)?;
                    let key = match key { Some(()) => Some(r.u32().map_err(m)?), None => None };
                    let len = r.u16().map_err(m)? as usize;
                    let data = r.bytes(len).map_err(m)?;
                    if len == 0 {
                        return Err(Violation::Malformed);
                    }
                    if !self.is_dup(ch, id)? {
                        self.on_reliable(ch, id, key, data.to_vec(), out)?;
                    }
                }
                CK_FRAG => {
                    info.eliciting = true;
                    let (ch, key) = read_ch_key(&mut r)?;
                    let id = r.u32().map_err(m)?;
                    let key = match key { Some(()) => Some(r.u32().map_err(m)?), None => None };
                    let idx = r.u8().map_err(m)?;
                    let count = r.u8().map_err(m)?;
                    let len = r.u16().map_err(m)? as usize;
                    let data = r.bytes(len).map_err(m)?;
                    if self.is_dup(ch, id)? {
                        continue;
                    }
                    if let Some(msg) = self.reasm.insert(now, ch, id, idx, count, data).map_err(Violation::Frag)? {
                        self.on_reliable(ch, id, key, msg, out)?;
                    }
                }
                CK_PING => info.eliciting = true,
                CK_ACK_DELAY => info.ack_delay_ms = Some(r.u16().map_err(m)?),
                CK_RESET_TOKEN => {
                    info.eliciting = true;
                    info.reset_token = Some(r.array().map_err(m)?);
                }
                CK_PATH_CHALLENGE => {
                    info.eliciting = true;
                    info.path_challenge = Some(r.array().map_err(m)?);
                }
                CK_PATH_RESPONSE => {
                    info.eliciting = true;
                    info.path_response = Some(r.array().map_err(m)?);
                }
                CK_CLOSE => {
                    let code = r.u8().map_err(m)?;
                    let len = r.u8().map_err(m)? as usize;
                    let reason = r.bytes(len).map_err(m)?;
                    info.close = Some((code, String::from_utf8_lossy(reason).into_owned()));
                    break;
                }
                _ => return Err(Violation::UnknownChunk),
            }
        }
        Ok(info)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn drain(ob: &mut Outbox, pkt: u64) -> (Vec<u8>, Vec<FragRef>) {
        let mut buf = Vec::new();
        let refs = ob.fill(&mut buf, pkt);
        (buf, refs)
    }

    #[test]
    fn latest_replaces_unsent_and_drops_stale() {
        let mut ob = Outbox::default();
        ob.send(SendMode::Latest { key: 7 }, vec![1]).unwrap();
        ob.send(SendMode::Latest { key: 7 }, vec![2]).unwrap();
        ob.send(SendMode::Latest { key: 8 }, vec![3]).unwrap();
        let (buf, refs) = drain(&mut ob, 0);
        assert!(refs.is_empty());
        let mut ib = Inbox::default();
        let mut out = vec![];
        ib.on_payload(0, 5, &buf, &mut out).unwrap();
        assert_eq!(out.len(), 2);
        assert_eq!(
            out[0],
            Delivery {
                channel: 0,
                key: 7,
                data: vec![2]
            }
        );
        // An older packet's message for key 7 is stale and dropped.
        let mut old = Vec::new();
        write_unrel(&mut old, 7, &[9]);
        out.clear();
        ib.on_payload(0, 4, &old, &mut out).unwrap();
        assert!(out.is_empty());
        ib.on_payload(0, 6, &old, &mut out).unwrap();
        assert_eq!(out.len(), 1);
    }

    #[test]
    fn send_limits() {
        let mut ob = Outbox::default();
        assert_eq!(ob.send(SendMode::Reliable, vec![]), Err(SendError::Empty));
        assert_eq!(
            ob.send(SendMode::Latest { key: 1 }, vec![0; MAX_UNRELIABLE + 1]),
            Err(SendError::TooLarge)
        );
        assert_eq!(ob.send(SendMode::Ordered, vec![0; MAX_MESSAGE + 1]), Err(SendError::TooLarge));
        for _ in 0..16 {
            ob.send(SendMode::Ordered, vec![0; MAX_MESSAGE]).unwrap();
        }
        assert_eq!(ob.send(SendMode::Ordered, vec![0; 10]), Err(SendError::Backpressure));
    }

    #[test]
    fn supersede_cancels_older_unacked() {
        let mut ob = Outbox::default();
        ob.send(SendMode::ReliableLatest { key: 1 }, vec![1; 10]).unwrap();
        let (_, refs) = drain(&mut ob, 0);
        assert_eq!(refs.len(), 1);
        ob.send(SendMode::ReliableLatest { key: 1 }, vec![2; 10]).unwrap();
        assert_eq!(ob.reliable_pending(), 1, "older snapshot no longer retransmitted");
        // Loss of the packet carrying the cancelled one is harmless.
        ob.on_frag_lost(refs[0], 0);
        let (buf, refs2) = drain(&mut ob, 1);
        assert_eq!(refs2.len(), 1);
        assert_eq!(refs2[0].id, 1);
        let mut ib = Inbox::default();
        let mut out = vec![];
        ib.on_payload(0, 1, &buf, &mut out).unwrap();
        assert_eq!(out[0].data, vec![2; 10]);
    }

    /// A superseded ReliableLatest message that is
    /// already partly on the wire is completed, so the receiver never holds
    /// an orphan partial (and never has to guess which partial to evict).
    #[test]
    fn supersede_never_orphans_a_started_fragmented_message() {
        let mut ob = Outbox::default();
        let big1 = vec![1u8; 3 * FRAG_DATA];
        ob.send(SendMode::ReliableLatest { key: 9 }, big1.clone()).unwrap();
        let (first, refs) = drain(&mut ob, 0);
        assert_eq!(refs.len(), 1, "one fragment per packet");
        // Superseded while in flight: not abandoned.
        let big2 = vec![2u8; 3 * FRAG_DATA];
        ob.send(SendMode::ReliableLatest { key: 9 }, big2.clone()).unwrap();
        assert_eq!(ob.reliable_pending(), 2);
        let mut ib = Inbox::default();
        let mut out = vec![];
        ib.on_payload(0, 0, &first, &mut out).unwrap();
        for pkt in 1..20 {
            let (buf, _) = drain(&mut ob, pkt);
            if buf.is_empty() {
                break;
            }
            ib.on_payload(0, pkt, &buf, &mut out).unwrap();
        }
        assert_eq!(out.len(), 2);
        assert!(out.iter().any(|d| d.data == big1) && out.iter().any(|d| d.data == big2));
        assert_eq!(ib.reasm.partials(), 0, "no orphan partial left behind");
        // An unsent superseded message is still dropped (single or fragmented).
        let mut ob = Outbox::default();
        ob.send(SendMode::ReliableLatest { key: 3 }, vec![1; 3 * FRAG_DATA]).unwrap();
        ob.send(SendMode::ReliableLatest { key: 3 }, vec![2; 10]).unwrap();
        assert_eq!(ob.reliable_pending(), 1);
    }

    #[test]
    fn window_holds_back_messages() {
        let mut ob = Outbox::default();
        for i in 0..(SEND_WINDOW + 10) {
            ob.send(SendMode::Ordered, i.to_le_bytes().to_vec()).unwrap();
        }
        let mut sent = 0;
        for pkt in 0..100 {
            let (_, refs) = drain(&mut ob, pkt);
            sent += refs.len();
        }
        assert_eq!(sent, SEND_WINDOW as usize, "only the window is in flight");
        // Ack the first message: one more becomes eligible.
        ob.on_frag_acked(FragRef {
            ch: CH_ORDERED,
            id: 0,
            idx: 0,
        });
        let (_, refs) = drain(&mut ob, 200);
        assert_eq!(refs.len(), 1);
        assert_eq!(refs[0].id, SEND_WINDOW);
    }

    #[test]
    fn ordered_receive_window_and_violations() {
        let mut ib = Inbox::default();
        let mut out = vec![];
        let mut p = Vec::new();
        write_rel(&mut p, CH_ORDERED, 1, b"b");
        write_rel(&mut p, CH_ORDERED, 0, b"a");
        write_rel(&mut p, CH_ORDERED, 0, b"a");
        ib.on_payload(0, 0, &p, &mut out).unwrap();
        assert_eq!(
            out.iter().map(|d| d.data.clone()).collect::<Vec<_>>(),
            vec![b"a".to_vec(), b"b".to_vec()]
        );
        let mut far = Vec::new();
        write_rel(&mut far, CH_ORDERED, 2 + RECV_WINDOW, b"x");
        assert_eq!(ib.on_payload(0, 1, &far, &mut out).unwrap_err(), Violation::WindowExceeded);
        let mut bad = Vec::new();
        write_rel(&mut bad, 3, 0, b"x");
        assert_eq!(ib.on_payload(0, 2, &bad, &mut out).unwrap_err(), Violation::BadChannel);
        assert_eq!(ib.on_payload(0, 3, &[0x77], &mut out).unwrap_err(), Violation::UnknownChunk);
        assert_eq!(ib.on_payload(0, 4, &[CK_REL, 1, 0], &mut out).unwrap_err(), Violation::Malformed);
        let mut fr = Vec::new();
        write_frag(&mut fr, CH_RELIABLE, 0, 5, 2, b"x");
        assert_eq!(
            ib.on_payload(0, 5, &fr, &mut out).unwrap_err(),
            Violation::Frag(FragError::BadIndex)
        );
    }
}
