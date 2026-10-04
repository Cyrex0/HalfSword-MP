//! One v5 connection (sans-IO).
//!
//! Data packet:
//! ```text
//! 0      u8   type = 0xB0
//! 1..9   u64  conn_id (LE)
//! 9..13  u32  pkt_seq   low 32 bits of the sender's packet number (nonce)
//! 13..17 u32  ack       low 32 bits of the highest packet number received
//! 17..21 u32  ack_bits  bit i = (ack - 1 - i) received
//! 21     u8   flags     0x01 KEY_PHASE, 0x02 ACK_VALID; other bits must be 0
//! 22..   ChaCha20-Poly1305(key = this direction's traffic key,
//!                          nonce = full packet number, aad = bytes 0..22)
//! ```
//! The header is cleartext but authenticated. The replay window moves only
//! after the AEAD check passed.

use std::collections::BTreeMap;

use chacha20poly1305::ChaCha20Poly1305;

use super::channel::{
    self, Delivery, FragRef, Inbox, Outbox, SendError, SendMode, Violation, ACK_DELAY_LEN, CK_ACK_DELAY, CK_PING,
    CK_RESET_TOKEN, RESET_TOKEN_LEN,
};
use super::crypto::{self, HandshakeSecrets, Secret};
use super::replay::{expand_seq, Check, ReplayWindow};
use super::wire::Reader;
use super::{ConnId, DATA_HEADER_LEN, PT_DATA, TAG_LEN};

pub const FLAG_KEY_PHASE: u8 = 0x01;
pub const FLAG_ACK: u8 = 0x02;
const FLAGS_KNOWN: u8 = FLAG_KEY_PHASE | FLAG_ACK;
/// Packets acked after this many newer ones are declared lost (starting
/// value; it grows when a loss turns out to have been reordering).
const PACKET_THRESHOLD: u64 = 3;
/// Largest packet reorder threshold (the ack field covers 32 packets).
const MAX_PACKET_THRESHOLD: u64 = 32;
/// Timer granularity (ms): the transport ticks every 5 ms.
pub const GRANULARITY_MS: u64 = 5;
/// Remembered declared-lost packets, to notice spurious losses.
const LOST_MEMORY: usize = 256;
/// Path-dead threshold once the server handed out a stateless-reset token:
/// a restarted server is then detected at once by the reset, so
/// path-dead only has to catch a vanished server and can ride out honest
/// Wi-Fi stalls of a few seconds instead of re-handshaking after 2 s.
pub const DEAD_AFTER_WITH_RESET_MS: u64 = 5_000;
/// Congestion control: pacing rate bounds (bytes/s) and the
/// additive increase (bytes/s gained per second while the pacer limits).
pub const CC_INITIAL_BPS: f64 = 1024.0 * 1024.0;
pub const CC_FLOOR_BPS: f64 = 160.0 * 1024.0;
pub const CC_MAX_BPS: f64 = 16.0 * 1024.0 * 1024.0;
pub const CC_AI_BPS_PER_S: f64 = 512.0 * 1024.0;
/// Multiplicative decrease on a loss event (at most one per RTT).
pub const CC_MD: f64 = 0.7;
/// Pacer burst: this many ms of the rate, at least 4 full datagrams.
pub const CC_BURST_MS: f64 = 10.0;
const MAX_BACKOFF: u32 = 4;
const CLOSE_REPEATS: u8 = 3;
const CLOSE_INTERVAL_MS: u64 = 30;
/// Ack at once after this many ack-eliciting packets (the ack field covers
/// the largest packet and the 32 before it).
pub const ACK_EVERY: u32 = 16;
/// Most ack delay a peer may claim (its ack timer 20 ms + a 5 ms transport
/// tick). A claim is clamped to this, so a peer can talk its RTT down by at
/// most this much.
pub const MAX_ACK_DELAY_MS: u64 = 25;
/// While nothing has been heard for half of `dead_after_ms`, keepalive PINGs
/// go out this often (more chances for one to get through under loss).
pub const PROBE_KEEPALIVE_MS: u64 = 250;

pub mod close_code {
    pub const NORMAL: u8 = 0;
    pub const TIMEOUT: u8 = 1;
    pub const PROTOCOL_VIOLATION: u8 = 2;
    pub const KICKED: u8 = 3;
    pub const SERVER_CLOSING: u8 = 4;
    pub const GAME_EXITED: u8 = 5;
    pub const REPLACED: u8 = 6;
    /// The server answered with a verified stateless reset (it no longer
    /// knows this connection: restarted, or reaped it). Re-handshake now.
    pub const RESET: u8 = 7;
}

#[derive(Debug, Clone)]
pub struct ConnConfig {
    /// No authenticated packet for this long closes the connection.
    pub idle_timeout_ms: u64,
    /// Send a PING when nothing was sent for this long.
    pub keepalive_ms: u64,
    /// Delay before an ack-only packet (acks normally piggyback).
    pub ack_delay_ms: u64,
    pub initial_rto_ms: u64,
    pub min_rto_ms: u64,
    pub max_rto_ms: u64,
    /// Key update after this many packets in one key phase ...
    pub rekey_packets: u64,
    /// ... or after this long.
    pub rekey_ms: u64,
    /// Path-dead detection (0 = off): we sent ack-eliciting packets but
    /// heard nothing at all for this long, so the path (or the server) is
    /// gone. Much shorter than `idle_timeout_ms`; clients use it to
    /// re-handshake after ~2 s instead of 10. The close
    /// code is TIMEOUT.
    pub dead_after_ms: u64,
    /// Pace data packets with the congestion controller's rate.
    pub pacing: bool,
    /// Lowest pacing rate (bytes/s); losses never push the rate below it.
    pub cc_floor_bps: f64,
}

impl Default for ConnConfig {
    fn default() -> Self {
        Self {
            idle_timeout_ms: 10_000,
            keepalive_ms: 1_000,
            ack_delay_ms: 20,
            initial_rto_ms: 300,
            min_rto_ms: 50,
            max_rto_ms: 2_000,
            rekey_packets: 1 << 30,
            rekey_ms: 3_600_000,
            dead_after_ms: 0,
            pacing: true,
            cc_floor_bps: CC_FLOOR_BPS,
        }
    }
}

/// Client default for `ConnConfig::dead_after_ms`.
pub const CLIENT_DEAD_AFTER_MS: u64 = 2_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Side {
    Client,
    Server,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConnState {
    Open,
    Closing { code: u8 },
    Closed { code: u8, by_peer: bool },
}

/// Why an incoming datagram was not accepted. None of these affects state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Drop {
    Truncated,
    NotData,
    WrongConn,
    BadFlags,
    Duplicate,
    TooOld,
    Auth,
    Closed,
}

#[derive(Debug, Default, Clone)]
pub struct ConnStats {
    pub pkts_sent: u64,
    pub pkts_recv: u64,
    pub bytes_sent: u64,
    pub bytes_recv: u64,
    pub pkts_lost: u64,
    pub retransmits: u64,
    pub dup_dropped: u64,
    pub old_dropped: u64,
    pub auth_failed: u64,
    pub key_updates_tx: u64,
    pub key_updates_rx: u64,
    pub srtt_ms: f64,
    pub rttvar_ms: f64,
    /// Lowest RTT sample (ms, after the peer's ack delay, like `srtt_ms`), 0
    /// before the first: the path RTT without queueing, so
    /// `srtt_ms - min_rtt_ms` estimates the standing queue.
    pub min_rtt_ms: f64,
    pub rto_ms: u64,
    /// Declared losses later acked after all (reordering, not loss).
    pub spurious_lost: u64,
    /// Current congestion-controller pacing rate (bytes/s).
    pub cc_rate_bps: f64,
    /// Polls the pacer held data back.
    pub paced: u64,
    /// Local stalls (the app did not drive the connection for > half the
    /// path-dead time) that restarted the path-dead clock.
    pub local_stalls: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DataHeader {
    pub conn_id: ConnId,
    pub seq: u32,
    pub ack: u32,
    pub ack_bits: u32,
    pub flags: u8,
}

/// Parse the cleartext header of a data packet (no authentication).
pub fn parse_header(dg: &[u8]) -> Option<DataHeader> {
    if dg.len() < DATA_HEADER_LEN + TAG_LEN {
        return None;
    }
    let mut r = Reader::new(dg);
    if r.u8().ok()? != PT_DATA {
        return None;
    }
    Some(DataHeader {
        conn_id: r.u64().ok()?,
        seq: r.u32().ok()?,
        ack: r.u32().ok()?,
        ack_bits: r.u32().ok()?,
        flags: r.u8().ok()?,
    })
}

struct TxKeys {
    secret: Secret,
    cipher: ChaCha20Poly1305,
    phase: bool,
    count: u64,
    started: u64,
}

struct RxKeys {
    secret: Secret,
    cipher: ChaCha20Poly1305,
    phase: bool,
    prev: Option<ChaCha20Poly1305>,
    next: Option<(Secret, ChaCha20Poly1305)>,
}

struct SentPacket {
    at: u64,
    frags: Vec<FragRef>,
}

pub struct Conn {
    side: Side,
    cid: ConnId,
    cfg: ConnConfig,
    tx: TxKeys,
    rx: RxKeys,
    next_seq: u64,
    recv: ReplayWindow,
    ack_pending: bool,
    ack_since: u64,
    /// Ack-eliciting packets received since our last packet (which carries
    /// the ack). At ACK_EVERY an ack goes out at once.
    unacked_rx: u32,
    sent: BTreeMap<u64, SentPacket>,
    largest_acked: Option<u64>,
    srtt: Option<f64>,
    rttvar: f64,
    backoff: u32,
    out: Outbox,
    inbox: Inbox,
    last_send: u64,
    last_recv: u64,
    state: ConnState,
    close_left: u8,
    close_next_at: u64,
    close_reason: String,
    violation: Option<Violation>,
    stats: ConnStats,
    /// Negotiated capability bits (`caps::ACK_DELAY`, `caps::RESET`).
    caps: u64,
    /// When the highest-numbered packet so far arrived (ack-delay base).
    largest_rx_at: u64,
    min_rtt: Option<f64>,
    /// Lowest RTT sample after the ack-delay correction.
    min_sample: Option<f64>,
    /// Last time an ack-eliciting packet went out (path-dead detection).
    last_eliciting_tx: u64,
    /// Server: the stateless-reset token still to deliver, and the recent
    /// packets that carried it (delivered once one of them is acked).
    reset_token_tx: Option<[u8; 16]>,
    token_pkts: Vec<u64>,
    token_delivered: bool,
    /// Client: the token the server handed out.
    reset_token_rx: Option<[u8; 16]>,
    /// Newest raw RTT sample (ms) and the adaptive reorder threshold.
    latest_rtt: f64,
    reorder_thresh: u64,
    /// Extra eighths of RTT the time threshold waits (grows on spurious loss).
    time_eighths: u64,
    lost_recent: std::collections::VecDeque<u64>,
    /// Congestion control: pacing rate, token bucket, bookkeeping.
    cc_rate: f64,
    /// Rate before the last decrease (restored when that loss was spurious).
    cc_prev_rate: f64,
    cc_tokens: f64,
    cc_refill_at: u64,
    cc_limited: bool,
    cc_last_decrease: u64,
    cc_last_increase: u64,
    /// Last `poll_transmit` (local-stall detection) and the start of the
    /// current silence for path-dead purposes.
    last_poll: u64,
    silence_base: u64,
    /// A PATH_CHALLENGE to answer, and the newest PATH_RESPONSE received.
    path_resp_tx: Option<[u8; 8]>,
    path_resp_rx: Option<[u8; 8]>,
    /// Scratch buffers reused across packets: the payload being assembled,
    /// and the decrypted payload of the packet being received.
    tx_buf: Vec<u8>,
    rx_buf: Vec<u8>,
}

impl Conn {
    pub fn new(side: Side, cid: ConnId, tx_secret: Secret, rx_secret: Secret, now: u64, cfg: ConnConfig) -> Self {
        Conn {
            side,
            cid,
            tx: TxKeys {
                secret: tx_secret,
                cipher: crypto::cipher(&tx_secret),
                phase: false,
                count: 0,
                started: now,
            },
            rx: RxKeys {
                secret: rx_secret,
                cipher: crypto::cipher(&rx_secret),
                phase: false,
                prev: None,
                next: None,
            },
            next_seq: 0,
            recv: ReplayWindow::new(),
            ack_pending: false,
            ack_since: now,
            unacked_rx: 0,
            sent: BTreeMap::new(),
            largest_acked: None,
            srtt: None,
            rttvar: 0.0,
            backoff: 0,
            out: Outbox::default(),
            inbox: Inbox::default(),
            last_send: now,
            last_recv: now,
            state: ConnState::Open,
            close_left: 0,
            close_next_at: now,
            close_reason: String::new(),
            violation: None,
            stats: ConnStats {
                rto_ms: cfg.initial_rto_ms,
                cc_rate_bps: CC_INITIAL_BPS.max(cfg.cc_floor_bps),
                ..Default::default()
            },
            latest_rtt: 0.0,
            reorder_thresh: PACKET_THRESHOLD,
            time_eighths: 1,
            lost_recent: std::collections::VecDeque::new(),
            cc_rate: CC_INITIAL_BPS.max(cfg.cc_floor_bps),
            cc_prev_rate: 0.0,
            cc_tokens: 4.0 * super::MAX_DATAGRAM as f64,
            cc_refill_at: now,
            cc_limited: false,
            cc_last_decrease: 0,
            cc_last_increase: now,
            last_poll: now,
            silence_base: now,
            path_resp_tx: None,
            path_resp_rx: None,
            tx_buf: Vec::new(),
            rx_buf: Vec::new(),
            cfg,
            caps: 0,
            largest_rx_at: now,
            min_rtt: None,
            min_sample: None,
            last_eliciting_tx: now,
            reset_token_tx: None,
            token_pkts: Vec::new(),
            token_delivered: false,
            reset_token_rx: None,
        }
    }

    /// Enable the negotiated transport features (`client_caps & server_caps`).
    pub fn set_caps(&mut self, caps: u64) {
        self.caps = caps;
        self.out.keyed = caps & super::caps::REL_KEY != 0;
    }
    pub fn caps(&self) -> u64 {
        self.caps
    }
    /// Server: deliver this stateless-reset token to the client (needs
    /// `caps::RESET`).
    pub fn set_reset_token(&mut self, token: [u8; 16]) {
        if self.caps & super::caps::RESET != 0 {
            self.reset_token_tx = Some(token);
        }
    }
    /// Client: the stateless-reset token the server handed out, if any.
    pub fn reset_token(&self) -> Option<[u8; 16]> {
        self.reset_token_rx
    }
    /// True once the client acked a packet that carried the reset token.
    pub fn reset_token_delivered(&self) -> bool {
        self.token_delivered
    }

    /// Did the (authenticated) header `h` acknowledge our packet `pkt`?
    /// Path validation: a packet from a new address that acks the probe we
    /// sent there proves the peer receives at that address.
    pub fn header_acks(&self, h: &DataHeader, pkt: u64) -> bool {
        if h.flags & FLAG_ACK == 0 || self.next_seq == 0 {
            return false;
        }
        let largest = expand_seq(h.ack, self.next_seq);
        if largest >= self.next_seq {
            return false;
        }
        if largest == pkt {
            return true;
        }
        pkt < largest && largest - 1 - pkt < 32 && (h.ack_bits >> (largest - 1 - pkt)) & 1 == 1
    }

    /// Size of the datagram `probe` builds (checked BEFORE sealing, so a
    /// probe that would be too large never consumes a packet number or the
    /// pending ack).
    pub fn probe_len(challenge: bool) -> usize {
        DATA_HEADER_LEN + TAG_LEN + if challenge { channel::PATH_TOKEN_LEN } else { 1 }
    }

    /// A path probe: a sealed packet carrying a PATH_CHALLENGE with
    /// `challenge` (peers with `caps::PATH_CHALLENGE`), else a PING (not
    /// tracked for loss: if the new path is dead it is simply never
    /// answered). Returns (packet number, datagram).
    pub fn probe(&mut self, now: u64, challenge: Option<[u8; 8]>) -> (u64, Vec<u8>) {
        let seq = self.next_seq;
        let mut p = Vec::with_capacity(channel::PATH_TOKEN_LEN);
        match challenge {
            Some(t) => {
                p.push(channel::CK_PATH_CHALLENGE);
                p.extend_from_slice(&t);
            }
            None => p.push(CK_PING),
        }
        let dg = self.seal_packet(now, &p, Vec::new(), false);
        (seq, dg)
    }

    /// The newest PATH_RESPONSE token received (taken).
    pub fn take_path_response(&mut self) -> Option<[u8; 8]> {
        self.path_resp_rx.take()
    }

    /// Does `dg` authenticate under this connection's receive keys? No state
    /// changes (used for duplicates: a genuine copy arriving on the current
    /// path proves that path still delivers).
    pub fn peek_auth(&self, dg: &[u8]) -> bool {
        let Some(h) = parse_header(dg) else { return false };
        if h.conn_id != self.cid {
            return false;
        }
        let seq = expand_seq(h.seq, self.recv.expected());
        let (aad, ct) = dg.split_at(DATA_HEADER_LEN);
        let phase = h.flags & FLAG_KEY_PHASE != 0;
        if phase == self.rx.phase {
            return crypto::open(&self.rx.cipher, seq, aad, ct).is_some()
                || self.rx.prev.as_ref().is_some_and(|p| crypto::open(p, seq, aad, ct).is_some());
        }
        let next = crypto::cipher(&crypto::next_secret(&self.rx.secret));
        crypto::open(&next, seq, aad, ct).is_some() || self.rx.prev.as_ref().is_some_and(|p| crypto::open(p, seq, aad, ct).is_some())
    }

    /// Smoothed RTT (ms), or the initial RTO before any sample.
    pub fn srtt_ms(&self) -> f64 {
        self.srtt.unwrap_or(self.cfg.initial_rto_ms as f64)
    }

    /// Build from handshake output (client sends with c2s, server with s2c).
    pub fn from_handshake(side: Side, s: &HandshakeSecrets, now: u64, cfg: ConnConfig) -> Self {
        match side {
            Side::Client => Conn::new(side, s.conn_id, s.c2s, s.s2c, now, cfg),
            Side::Server => Conn::new(side, s.conn_id, s.s2c, s.c2s, now, cfg),
        }
    }

    pub fn side(&self) -> Side {
        self.side
    }
    pub fn conn_id(&self) -> ConnId {
        self.cid
    }
    pub fn state(&self) -> &ConnState {
        &self.state
    }
    pub fn is_open(&self) -> bool {
        self.state == ConnState::Open
    }
    pub fn is_closed(&self) -> bool {
        matches!(self.state, ConnState::Closed { .. })
    }
    pub fn violation(&self) -> Option<Violation> {
        self.violation
    }
    pub fn last_recv_ms(&self) -> u64 {
        self.last_recv
    }
    pub fn stats(&self) -> ConnStats {
        let mut s = self.stats.clone();
        s.retransmits = self.out.retransmits;
        s.rto_ms = self.rto();
        s.cc_rate_bps = self.cc_rate;
        s.min_rtt_ms = self.min_sample.unwrap_or(0.0);
        s
    }
    /// Reliable messages queued or in flight (both reliable channels).
    pub fn reliable_pending(&self) -> usize {
        self.out.reliable_pending()
    }

    pub fn send(&mut self, mode: SendMode, data: Vec<u8>) -> Result<(), SendError> {
        if !self.is_open() {
            return Err(SendError::Closed);
        }
        self.out.send(mode, data)
    }

    /// Start a graceful close: the CLOSE chunk is sent up to 3 times.
    pub fn close(&mut self, now: u64, code: u8, reason: &str) {
        if self.is_open() {
            self.state = ConnState::Closing { code };
            self.close_left = CLOSE_REPEATS;
            self.close_next_at = now;
            self.close_reason = channel::truncate_utf8(reason, 100).to_string();
        }
    }

    /// Retransmission timeout:
    /// `srtt + max(4·rttvar, granularity) + MAX_ACK_DELAY_MS`. The peer may
    /// hold its ack up to its ack timer plus a transport tick (≤ MAX_ACK_DELAY_MS) before it goes out; with
    /// `caps::ACK_DELAY` that delay is subtracted from the RTT samples, so
    /// the timer must add it back (RFC 9002 §6.2.1). Without the capability
    /// the samples include a variable part of it; the term keeps the timer
    /// clear of it too.
    fn rto(&self) -> u64 {
        let base = match self.srtt {
            None => self.cfg.initial_rto_ms as f64,
            Some(s) => s + (4.0 * self.rttvar).max(GRANULARITY_MS as f64) + MAX_ACK_DELAY_MS as f64,
        };
        (base as u64).clamp(self.cfg.min_rto_ms, self.cfg.max_rto_ms)
    }

    /// Time threshold for a packet older than an acked one: RFC 9002's
    /// 9/8 · max(srtt, latest_rtt), plus granularity; the 1/8 margin grows
    /// when losses turn out spurious (jitter reorders).
    fn time_threshold(&self) -> u64 {
        let rtt = self.srtt.map_or(self.cfg.initial_rto_ms as f64, |s| s.max(self.latest_rtt));
        let mut t = rtt * (8 + self.time_eighths) as f64 / 8.0;
        // Once the link was seen reordering, never below srtt + 4·rttvar: the
        // jitter tail itself must not look like loss.
        if self.reordering() {
            if let Some(s) = self.srtt {
                t = t.max(s + 4.0 * self.rttvar);
            }
        }
        t as u64 + GRANULARITY_MS
    }

    /// A declared loss turned out spurious on this connection.
    fn reordering(&self) -> bool {
        self.time_eighths > 1
    }

    fn rtt_sample(&mut self, ms: f64) {
        self.latest_rtt = ms;
        self.min_sample = Some(self.min_sample.map_or(ms, |m| m.min(ms)));
        match self.srtt {
            None => {
                self.srtt = Some(ms);
                self.rttvar = ms / 2.0;
            }
            Some(s) => {
                self.rttvar = 0.75 * self.rttvar + 0.25 * (s - ms).abs();
                self.srtt = Some(0.875 * s + 0.125 * ms);
            }
        }
        self.stats.srtt_ms = self.srtt.unwrap_or(0.0);
        self.stats.rttvar_ms = self.rttvar;
    }

    /// Decrypt into `pt` (a reused buffer); false if no key opens it.
    fn open_rx(&mut self, seq: u64, phase: bool, aad: &[u8], ct: &[u8], pt: &mut Vec<u8>) -> bool {
        if phase == self.rx.phase {
            return crypto::open_into(&self.rx.cipher, seq, aad, ct, pt);
        }
        if self.rx.next.is_none() {
            let s = crypto::next_secret(&self.rx.secret);
            self.rx.next = Some((s, crypto::cipher(&s)));
        }
        let (ns, nc) = self.rx.next.as_ref().expect("set above");
        if crypto::open_into(nc, seq, aad, ct, pt) {
            let (ns, nc) = (*ns, nc.clone());
            let old = std::mem::replace(&mut self.rx.cipher, nc);
            self.rx.prev = Some(old);
            self.rx.secret = ns;
            self.rx.phase = phase;
            self.rx.next = None;
            self.stats.key_updates_rx += 1;
            return true;
        }
        self.rx.prev.as_ref().is_some_and(|p| crypto::open_into(p, seq, aad, ct, pt))
    }

    /// Process one incoming datagram addressed to this connection.
    pub fn recv(&mut self, now: u64, dg: &[u8]) -> Result<Vec<Delivery>, Drop> {
        if self.is_closed() {
            return Err(Drop::Closed);
        }
        let h = parse_header(dg).ok_or(if dg.first() == Some(&PT_DATA) {
            Drop::Truncated
        } else {
            Drop::NotData
        })?;
        if h.conn_id != self.cid {
            return Err(Drop::WrongConn);
        }
        if h.flags & !FLAGS_KNOWN != 0 {
            return Err(Drop::BadFlags);
        }
        let seq = expand_seq(h.seq, self.recv.expected());
        match self.recv.check(seq) {
            Check::Fresh => {}
            Check::Duplicate => {
                self.stats.dup_dropped += 1;
                return Err(Drop::Duplicate);
            }
            Check::TooOld => {
                self.stats.old_dropped += 1;
                return Err(Drop::TooOld);
            }
        }
        let (aad, ct) = dg.split_at(DATA_HEADER_LEN);
        let mut pt = std::mem::take(&mut self.rx_buf);
        if !self.open_rx(seq, h.flags & FLAG_KEY_PHASE != 0, aad, ct, &mut pt) {
            self.rx_buf = pt;
            self.stats.auth_failed += 1;
            return Err(Drop::Auth);
        }
        self.last_recv = now;
        self.stats.pkts_recv += 1;
        self.stats.bytes_recv += dg.len() as u64;
        let mut out = Vec::new();
        let parsed = self.inbox.on_payload(now, seq, &pt, &mut out);
        self.rx_buf = pt;
        // A payload whose fragments do not fit the reassembly limits is
        // left unprocessed and NOT marked received, so it is never acked and
        // the sender retransmits its reliable content (no protocol violation
        // for an honest burst under loss).
        let deferred = parsed.as_ref().is_ok_and(|i| i.deferred);
        if !deferred {
            if self.recv.highest().is_none_or(|hi| seq > hi) {
                self.largest_rx_at = now;
            }
            self.recv.mark(seq);
        }
        if h.flags & FLAG_ACK != 0 {
            let delay = parsed.as_ref().ok().and_then(|i| i.ack_delay_ms);
            self.on_ack(now, h.ack, h.ack_bits, delay);
        }
        match parsed {
            Ok(info) if info.deferred => {}
            Ok(info) => {
                if let Some(t) = info.path_challenge {
                    self.path_resp_tx = Some(t);
                }
                if let Some(t) = info.path_response {
                    self.path_resp_rx = Some(t);
                }
                if let Some(t) = info.reset_token {
                    if self.side == Side::Client {
                        self.reset_token_rx = Some(t);
                    }
                }
                if info.eliciting {
                    if !self.ack_pending {
                        self.ack_pending = true;
                        self.ack_since = now;
                    }
                    self.unacked_rx = self.unacked_rx.saturating_add(1);
                }
                if let Some((code, _reason)) = info.close {
                    self.state = ConnState::Closed { code, by_peer: true };
                }
            }
            Err(v) => {
                self.violation = Some(v);
                self.close(now, close_code::PROTOCOL_VIOLATION, "protocol violation");
            }
        }
        Ok(out)
    }

    fn on_ack(&mut self, now: u64, ack: u32, bits: u32, ack_delay_ms: Option<u16>) {
        if self.next_seq == 0 {
            return;
        }
        let largest = expand_seq(ack, self.next_seq);
        if largest >= self.next_seq {
            return; // acks a packet we never sent
        }
        if self.reset_token_tx.is_some() && !self.token_pkts.is_empty() {
            let acked = |s: u64| s == largest || (s < largest && largest - 1 - s < 32 && (bits >> (largest - 1 - s)) & 1 == 1);
            if self.token_pkts.iter().any(|&s| acked(s)) {
                self.reset_token_tx = None;
                self.token_pkts.clear();
                self.token_delivered = true;
            }
        }
        // Spurious loss: a packet already declared lost is acked after
        // all — the link reorders more than the thresholds allow. Widen them
        // (packet threshold doubles up to 32, time threshold +1/8 RTT up to
        // 3·RTT and never below srtt + 4·rttvar), once per event.
        if !self.lost_recent.is_empty() {
            let acked = |s: u64| s == largest || (s < largest && largest - 1 - s < 32 && (bits >> (largest - 1 - s)) & 1 == 1);
            let before = self.lost_recent.len();
            self.lost_recent.retain(|&s| !acked(s));
            let spurious = (before - self.lost_recent.len()) as u64;
            if spurious > 0 {
                self.stats.spurious_lost += spurious;
                self.reorder_thresh = (self.reorder_thresh * 2).min(MAX_PACKET_THRESHOLD);
                self.time_eighths = (self.time_eighths + 1).min(16);
                // The decrease that loss caused was a false signal: undo it.
                self.cc_rate = self.cc_rate.max(self.cc_prev_rate);
            }
        }
        let mut any = false;
        if let Some(p) = self.sent.remove(&largest) {
            // RTT sample minus the peer's (bounded) ack delay.
            let latest = now.saturating_sub(p.at) as f64;
            let min_rtt = self.min_rtt.map_or(latest, |m| m.min(latest));
            self.min_rtt = Some(min_rtt);
            // The claim is clamped to MAX_ACK_DELAY_MS: a lying peer moves
            // its RTT by at most that, never below the raw minimum minus it.
            // (No RFC 9002 "≥ min_rtt" guard: with ACK_EVERY = 16 nearly
            // every ack in a quiet lobby is delayed, so the raw minimum
            // includes the delay itself and the guard would undo the subtraction.)
            let mut sample = latest;
            if let Some(d) = ack_delay_ms {
                let d = (d as u64).min(MAX_ACK_DELAY_MS) as f64;
                sample = (latest - d).max(min_rtt - MAX_ACK_DELAY_MS as f64).max(1.0);
            }
            self.rtt_sample(sample);
            for f in p.frags {
                self.out.on_frag_acked(f);
            }
            any = true;
        }
        // Most bits repeat acks already processed: only packets at or above
        // the oldest one still outstanding can be newly acked.
        let oldest = self.sent.keys().next().copied().unwrap_or(u64::MAX);
        for i in 0..32u64 {
            if (bits >> i) & 1 == 1 && largest > i && largest - 1 - i >= oldest {
                if let Some(p) = self.sent.remove(&(largest - 1 - i)) {
                    for f in p.frags {
                        self.out.on_frag_acked(f);
                    }
                    any = true;
                }
            }
        }
        if self.largest_acked.is_none_or(|la| largest > la) {
            self.largest_acked = Some(largest);
        }
        if any {
            self.backoff = 0;
            // Additive increase, only while the pacer actually held
            // data back since the last increase (an application-limited
            // connection must not inflate its rate on idle acks).
            let dt = now.saturating_sub(self.cc_last_increase) as f64 / 1000.0;
            self.cc_last_increase = now;
            if self.cc_limited {
                self.cc_rate = (self.cc_rate + CC_AI_BPS_PER_S * dt.min(1.0)).min(CC_MAX_BPS);
                self.cc_limited = false;
            }
        }
    }

    /// A loss event: multiplicative decrease, at most once per RTT (one
    /// burst of losses is one congestion signal), never below the floor.
    fn on_congestion(&mut self, now: u64) {
        let rtt = self.srtt.unwrap_or(self.cfg.initial_rto_ms as f64).max(50.0) as u64;
        if now.saturating_sub(self.cc_last_decrease) >= rtt {
            self.cc_last_decrease = now;
            self.cc_prev_rate = self.cc_rate;
            self.cc_rate = (self.cc_rate * CC_MD).max(self.cfg.cc_floor_bps);
        }
    }

    /// Pacer refill; returns whether a data packet may go now.
    fn pacer_ready(&mut self, now: u64) -> bool {
        if !self.cfg.pacing {
            return true;
        }
        let burst = (self.cc_rate * CC_BURST_MS / 1000.0).max(4.0 * super::MAX_DATAGRAM as f64);
        let dt = now.saturating_sub(self.cc_refill_at) as f64 / 1000.0;
        self.cc_refill_at = now;
        self.cc_tokens = (self.cc_tokens + dt * self.cc_rate).min(burst);
        self.cc_tokens >= 0.0
    }

    /// When the pacer allows the next data packet.
    fn pacer_ready_at(&self, now: u64) -> u64 {
        if !self.cfg.pacing || self.cc_tokens >= 0.0 {
            return now;
        }
        now + ((-self.cc_tokens) / self.cc_rate * 1000.0).ceil() as u64
    }

    /// Loss detection. A tracked packet is lost when
    /// * a packet `reorder_thresh` (3, adaptive) numbers newer was acked, or
    /// * a newer packet was acked and it is older than the time threshold
    ///   (9/8 · max(srtt, latest_rtt), adaptive), or
    /// * it is older than the RTO (which covers the peer's ack delay): the
    ///   retransmission then acts as the probe, and the timer backs off.
    ///
    /// The predicate is monotone in the packet number (packets are sent in
    /// number order), so the scan stops at the first packet that is not lost
    /// (rather than visiting every outstanding packet on every poll).
    fn detect_losses(&mut self, now: u64) {
        let rto = self.rto() << self.backoff.min(MAX_BACKOFF);
        let tt = self.time_threshold();
        let la = self.largest_acked;
        let thr = self.reorder_thresh;
        let reo = self.reordering();
        let srtt = self.srtt.unwrap_or(0.0);
        let mut lost = Vec::new();
        let mut by_time = false;
        for (&seq, p) in &self.sent {
            let age = now.saturating_sub(p.at);
            // On a reordering link the packet count alone is not trusted:
            // a counted loss must also be at least one srtt old.
            let count = la.is_some_and(|la| la >= seq + thr) && (!reo || age as f64 >= srtt);
            let later = la.is_some_and(|la| la > seq) && age >= tt;
            let timeout = age >= rto;
            if !(count || later || timeout) {
                break;
            }
            lost.push(seq);
            by_time |= timeout && !count && !later;
        }
        let any = !lost.is_empty();
        for seq in lost {
            if let Some(p) = self.sent.remove(&seq) {
                self.stats.pkts_lost += 1;
                if self.lost_recent.len() >= LOST_MEMORY {
                    self.lost_recent.pop_front();
                }
                self.lost_recent.push_back(seq);
                for f in p.frags {
                    self.out.on_frag_lost(f, seq);
                }
            }
        }
        if any {
            self.on_congestion(now);
        }
        if by_time {
            self.backoff = (self.backoff + 1).min(MAX_BACKOFF);
        }
    }

    fn maybe_rekey(&mut self, now: u64) {
        if self.tx.count >= self.cfg.rekey_packets || now.saturating_sub(self.tx.started) >= self.cfg.rekey_ms {
            let s = crypto::next_secret(&self.tx.secret);
            self.tx.secret = s;
            self.tx.cipher = crypto::cipher(&s);
            self.tx.phase = !self.tx.phase;
            self.tx.count = 0;
            self.tx.started = now;
            self.stats.key_updates_tx += 1;
        }
    }

    /// `track`: ack-eliciting content to watch for loss (and RTT samples).
    fn seal_packet(&mut self, now: u64, payload: &[u8], frags: Vec<FragRef>, track: bool) -> Vec<u8> {
        let seq = self.next_seq;
        let mut flags = 0u8;
        if self.tx.phase {
            flags |= FLAG_KEY_PHASE;
        }
        let (ack, bits) = match self.recv.ack_fields() {
            Some(a) => {
                flags |= FLAG_ACK;
                a
            }
            None => (0, 0),
        };
        let mut dg = Vec::with_capacity(DATA_HEADER_LEN + payload.len() + TAG_LEN);
        dg.push(PT_DATA);
        dg.extend_from_slice(&self.cid.to_le_bytes());
        dg.extend_from_slice(&(seq as u32).to_le_bytes());
        dg.extend_from_slice(&ack.to_le_bytes());
        dg.extend_from_slice(&bits.to_le_bytes());
        dg.push(flags);
        dg.extend_from_slice(payload);
        crypto::seal_in_place(&self.tx.cipher, seq, DATA_HEADER_LEN, &mut dg);
        self.next_seq += 1;
        self.tx.count += 1;
        self.last_send = now;
        self.ack_pending = false;
        self.unacked_rx = 0;
        self.stats.pkts_sent += 1;
        self.stats.bytes_sent += dg.len() as u64;
        if track {
            self.sent.insert(seq, SentPacket { at: now, frags });
            self.last_eliciting_tx = now;
        }
        dg
    }

    /// Produce the next datagram to send, if any. Call until `None`.
    pub fn poll_transmit(&mut self, now: u64) -> Option<Vec<u8>> {
        // The payload is assembled in a buffer kept across calls: a poll
        // with nothing to send allocates nothing.
        let mut payload = std::mem::take(&mut self.tx_buf);
        payload.clear();
        payload.reserve(super::MAX_PLAINTEXT);
        let dg = self.poll_transmit_with(now, &mut payload);
        self.tx_buf = payload;
        dg
    }

    fn poll_transmit_with(&mut self, now: u64, payload: &mut Vec<u8>) -> Option<Vec<u8>> {
        match self.state {
            ConnState::Closed { .. } => return None,
            ConnState::Closing { code } => {
                if self.close_left == 0 {
                    self.state = ConnState::Closed { code, by_peer: false };
                    return None;
                }
                if now < self.close_next_at {
                    return None;
                }
                self.close_left -= 1;
                self.close_next_at = now + CLOSE_INTERVAL_MS;
                let mut p = Vec::new();
                channel::write_close(&mut p, code, &self.close_reason.clone());
                let dg = self.seal_packet(now, &p, Vec::new(), true);
                self.sent.clear();
                if self.close_left == 0 {
                    self.state = ConnState::Closed { code, by_peer: false };
                }
                return Some(dg);
            }
            ConnState::Open => {}
        }
        // The application did not drive this connection for more than
        // half the path-dead time (a stalled process: disk, AV, laptop
        // resume). The silence is OUR stall, not the path's: the path-dead
        // clock restarts now, so the server's replies still queued in the
        // socket get processed before anything is declared dead.
        let dead_after = self.dead_after();
        if dead_after > 0 && now.saturating_sub(self.last_poll) > dead_after / 2 {
            self.silence_base = now;
            self.stats.local_stalls += 1;
        }
        self.last_poll = now;
        let silent = now.saturating_sub(self.last_recv);
        let heard = self.last_recv.max(self.silence_base);
        // Path dead (client): we kept sending, nothing at all came back.
        let dead = dead_after > 0 && now.saturating_sub(heard) >= dead_after && self.last_eliciting_tx > heard;
        if silent >= self.cfg.idle_timeout_ms || dead {
            self.state = ConnState::Closed {
                code: close_code::TIMEOUT,
                by_peer: false,
            };
            return None;
        }
        self.inbox.expire(now);
        self.detect_losses(now);
        self.maybe_rekey(now);
        // A PATH_RESPONSE goes out at once, paced or not.
        if let Some(t) = self.path_resp_tx.take() {
            payload.push(channel::CK_PATH_RESPONSE);
            payload.extend_from_slice(&t);
        }
        // Pacing: data (Latest, reliable, retransmissions) waits for the
        // pacer; acks, PINGs and PATH_RESPONSEs do not.
        let has_data = self.out.latest_pending() > 0 || self.out.reliable_unsent();
        let frags = if !has_data {
            Vec::new()
        } else if self.pacer_ready(now) {
            self.out.fill(payload, self.next_seq)
        } else {
            self.cc_limited = true;
            self.stats.paced += 1;
            Vec::new()
        };
        if payload.is_empty() && now.saturating_sub(self.last_send) >= self.keepalive_now(now) {
            payload.push(CK_PING);
        }
        // An ack covers the largest packet + the 32 before it: ack at once
        // after ACK_EVERY eliciting packets, so a burst (the server sends one
        // datagram per relayed message) cannot outrun the ack range and turn
        // into spurious losses and retransmits.
        let ack_due = self.ack_pending
            && (now.saturating_sub(self.ack_since) >= self.cfg.ack_delay_ms || self.unacked_rx >= ACK_EVERY);
        if payload.is_empty() && !ack_due {
            return None;
        }
        let mut track = !payload.is_empty();
        // Optional chunks, only where they fit (never displacing data).
        if let Some(t) = self.reset_token_tx {
            if payload.len() + RESET_TOKEN_LEN <= super::MAX_PLAINTEXT {
                payload.push(CK_RESET_TOKEN);
                payload.extend_from_slice(&t);
                track = true;
                if self.token_pkts.len() >= 64 {
                    self.token_pkts.remove(0);
                }
                self.token_pkts.push(self.next_seq);
            }
        }
        if self.caps & super::caps::ACK_DELAY != 0
            && self.recv.highest().is_some()
            && payload.len() + ACK_DELAY_LEN <= super::MAX_PLAINTEXT
        {
            let d = now.saturating_sub(self.largest_rx_at).min(u16::MAX as u64) as u16;
            payload.push(CK_ACK_DELAY);
            payload.extend_from_slice(&d.to_le_bytes());
        }
        let dg = self.seal_packet(now, payload, frags, track);
        if self.cfg.pacing && has_data {
            self.cc_tokens -= dg.len() as f64;
        }
        Some(dg)
    }

    /// Keepalive period now: faster while the peer has been silent for half
    /// the path-dead time.
    fn keepalive_now(&self, now: u64) -> u64 {
        let d = self.dead_after();
        if d > 0 && now.saturating_sub(self.last_recv.max(self.silence_base)) >= d / 2 {
            self.cfg.keepalive_ms.min(PROBE_KEEPALIVE_MS)
        } else {
            self.cfg.keepalive_ms
        }
    }

    /// Effective path-dead time: `dead_after_ms`, raised to
    /// DEAD_AFTER_WITH_RESET_MS once the server's stateless-reset token is
    /// held (a restart is then detected by the reset itself).
    fn dead_after(&self) -> u64 {
        if self.cfg.dead_after_ms > 0 && self.reset_token_rx.is_some() {
            self.cfg.dead_after_ms.max(DEAD_AFTER_WITH_RESET_MS)
        } else {
            self.cfg.dead_after_ms
        }
    }

    /// Earliest time `poll_transmit` may have something to do.
    pub fn next_timeout(&self, now: u64) -> u64 {
        let mut t = self.last_recv + self.cfg.idle_timeout_ms;
        let d = self.dead_after();
        if d > 0 {
            let heard = self.last_recv.max(self.silence_base);
            t = t.min(heard + d / 2);
            t = t.min(heard + d);
        }
        t = t.min(self.last_send + self.keepalive_now(now));
        if self.ack_pending {
            t = t.min(self.ack_since + self.cfg.ack_delay_ms);
            if self.unacked_rx >= ACK_EVERY {
                t = now;
            }
        }
        if let Some((&seq, p)) = self.sent.iter().next() {
            t = t.min(p.at + (self.rto() << self.backoff.min(MAX_BACKOFF)));
            if self.largest_acked.is_some_and(|la| la > seq) {
                t = t.min(p.at + self.time_threshold());
            }
        }
        if self.path_resp_tx.is_some() || matches!(self.state, ConnState::Closing { .. }) {
            t = now;
        }
        if self.out.latest_pending() > 0 {
            t = t.min(self.pacer_ready_at(now));
        }
        t.max(now)
    }

    /// Test/fuzz hook: seal an arbitrary plaintext as the next packet.
    #[doc(hidden)]
    pub fn seal_raw(&mut self, now: u64, plaintext: &[u8]) -> Vec<u8> {
        let track = !plaintext.is_empty();
        self.seal_packet(now, plaintext, Vec::new(), track)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::net::channel::{CH_ORDERED, CH_RELIABLE};

    fn pair(cfg: ConnConfig) -> (Conn, Conn) {
        let s = crypto::derive(&[1; 32], &[2; 32], &[3; 32]);
        (
            Conn::from_handshake(Side::Client, &s, 0, cfg.clone()),
            Conn::from_handshake(Side::Server, &s, 0, cfg),
        )
    }

    #[test]
    fn tampered_header_or_body_is_rejected_without_moving_the_window() {
        let (mut c, mut s) = pair(ConnConfig::default());
        c.send(SendMode::Ordered, b"hello".to_vec()).unwrap();
        let dg = c.poll_transmit(0).unwrap();
        for i in 0..dg.len() {
            let mut t = dg.clone();
            t[i] ^= 0x40;
            assert!(s.recv(1, &t).is_err(), "tampered byte {i} accepted");
        }
        assert_eq!(s.stats().pkts_recv, 0);
        // The genuine packet is still accepted afterwards (window unmoved).
        let d = s.recv(1, &dg).unwrap();
        assert_eq!(d[0].data, b"hello");
        assert_eq!(d[0].channel, CH_ORDERED);
    }

    /// A burst of more than 33 packets with no reply in between
    /// is still fully acked (ack at once every ACK_EVERY packets), so the
    /// sender sees no spurious loss.
    #[test]
    fn a_long_burst_is_acked_before_it_outruns_the_ack_range() {
        let (mut c, mut s) = pair(ConnConfig { pacing: false, ..ConnConfig::default() }); // unpaced: one datagram per poll at t = 0
        // The server sends 100 reliable messages, one datagram each, all at
        // t = 0 (no ack delay elapses on the client).
        let mut acks = 0;
        for i in 0..100u32 {
            s.send(SendMode::Reliable, i.to_le_bytes().to_vec()).unwrap();
            let dg = s.poll_transmit(0).unwrap();
            c.recv(0, &dg).unwrap();
            while let Some(a) = c.poll_transmit(0) {
                s.recv(0, &a).unwrap();
                acks += 1;
            }
        }
        assert!(acks >= 100 / ACK_EVERY as usize, "acks {acks}");
        let _ = s.poll_transmit(1); // loss detection
        assert_eq!(s.stats().pkts_lost, 0, "no spurious loss");
        assert_eq!(s.stats().retransmits, 0);
        // The tail (< ACK_EVERY) is acked after the normal delay.
        let a = c.poll_transmit(ConnConfig::default().ack_delay_ms).unwrap();
        s.recv(30, &a).unwrap();
        assert!(s.sent.is_empty(), "everything acked");
    }

    #[test]
    fn replayed_datagram_is_rejected() {
        let (mut c, mut s) = pair(ConnConfig::default());
        c.send(SendMode::Reliable, b"once".to_vec()).unwrap();
        let dg = c.poll_transmit(0).unwrap();
        assert_eq!(s.recv(1, &dg).unwrap().len(), 1);
        assert_eq!(s.recv(2, &dg), Err(Drop::Duplicate));
        // Push the window more than 1024 packets ahead: then it is too old.
        for i in 0..1100u64 {
            c.send(SendMode::Latest { key: 1 }, vec![1]).unwrap();
            let d = c.poll_transmit(10 + i).unwrap();
            s.recv(10 + i, &d).unwrap();
        }
        assert_eq!(s.recv(2000, &dg), Err(Drop::TooOld));
        assert_eq!(s.stats().dup_dropped, 1);
        assert_eq!(s.stats().old_dropped, 1);
    }

    #[test]
    fn wrong_conn_and_garbage_are_dropped() {
        let (mut c, mut s) = pair(ConnConfig::default());
        c.send(SendMode::Reliable, b"x".to_vec()).unwrap();
        let mut dg = c.poll_transmit(0).unwrap();
        dg[1] ^= 1;
        assert_eq!(s.recv(0, &dg), Err(Drop::WrongConn));
        assert_eq!(s.recv(0, &[PT_DATA, 1, 2]), Err(Drop::Truncated));
        assert_eq!(s.recv(0, &[0x11; 64]), Err(Drop::NotData));
    }

    #[test]
    fn key_update_survives_reordering() {
        let cfg = ConnConfig {
            rekey_packets: 20,
            ..Default::default()
        };
        let (mut c, mut s) = pair(cfg);
        let mut dgs = Vec::new();
        for i in 0..200u32 {
            c.send(SendMode::Reliable, i.to_le_bytes().to_vec()).unwrap();
            dgs.push(c.poll_transmit(i as u64).unwrap());
        }
        // Swap neighbours so packets straddle every key-phase boundary.
        for i in (1..dgs.len() - 1).step_by(2) {
            dgs.swap(i, i + 1);
        }
        let mut got = 0;
        for d in &dgs {
            got += s.recv(500, d).expect("reordered packet across key update").len();
        }
        assert_eq!(got, 200);
        assert!(c.stats().key_updates_tx >= 9);
        assert_eq!(s.stats().key_updates_rx, c.stats().key_updates_tx);
    }

    #[test]
    fn idle_timeout_and_keepalive() {
        let (mut c, _s) = pair(ConnConfig::default());
        assert!(c.poll_transmit(500).is_none());
        let ping = c.poll_transmit(1000).expect("keepalive ping");
        assert!(ping.len() > DATA_HEADER_LEN + TAG_LEN);
        assert!(c.poll_transmit(10_000).is_none());
        assert_eq!(
            c.state(),
            &ConnState::Closed {
                code: close_code::TIMEOUT,
                by_peer: false
            }
        );
    }

    #[test]
    fn close_is_delivered_and_repeated() {
        let (mut c, mut s) = pair(ConnConfig::default());
        c.close(0, close_code::GAME_EXITED, "bye");
        let mut n = 0;
        let mut t = 0;
        while !c.is_closed() {
            if let Some(dg) = c.poll_transmit(t) {
                n += 1;
                let _ = s.recv(t, &dg);
            }
            t += 10;
        }
        assert_eq!(n, 3);
        assert_eq!(
            s.state(),
            &ConnState::Closed {
                code: close_code::GAME_EXITED,
                by_peer: true
            }
        );
        assert_eq!(c.send(SendMode::Reliable, vec![1]), Err(SendError::Closed));
    }

    /// The peer's ack delay is carried (caps::ACK_DELAY)
    /// and subtracted from the RTT sample, bounded so a peer cannot claim
    /// its way below the real path RTT.
    #[test]
    fn rtt_samples_subtract_the_peers_bounded_ack_delay() {
        let run = |caps: u64| {
            let (mut c, mut s) = pair(ConnConfig::default());
            c.set_caps(caps);
            s.set_caps(caps);
            // Path: 10 ms each way. The server sends at 0; the client acks
            // after its 20 ms ack timer; the server sees the ack at 40.
            for round in 0..20u64 {
                let t = round * 1_000;
                s.send(SendMode::Reliable, vec![1]).unwrap();
                let dg = s.poll_transmit(t).unwrap();
                c.recv(t + 10, &dg).unwrap();
                assert!(c.poll_transmit(t + 25).is_none(), "ack waits for the ack timer");
                let ack = c.poll_transmit(t + 30).unwrap();
                s.recv(t + 40, &ack).unwrap();
            }
            s.stats().srtt_ms
        };
        let plain = run(0);
        let adjusted = run(crate::net::caps::ACK_DELAY);
        assert!((plain - 40.0).abs() < 1.0, "without the chunk: {plain}");
        assert!((adjusted - 20.0).abs() < 1.0, "path RTT 20 ms, got {adjusted}");

        // A lying peer (claims 900 ms of ack delay on every ack): the claim is
        // clamped to MAX_ACK_DELAY_MS and never takes a sample below min_rtt.
        let (mut c, mut s) = pair(ConnConfig::default());
        s.set_caps(crate::net::caps::ACK_DELAY);
        for round in 0..20u64 {
            let t = round * 1_000;
            s.send(SendMode::Reliable, vec![1]).unwrap();
            let dg = s.poll_transmit(t).unwrap();
            c.recv(t + 50, &dg).unwrap();
            let mut p = vec![CK_ACK_DELAY];
            p.extend_from_slice(&900u16.to_le_bytes());
            let ack = c.seal_raw(t + 50, &p);
            s.recv(t + 100, &ack).unwrap();
        }
        let srtt = s.stats().srtt_ms;
        assert!(srtt >= 100.0 - MAX_ACK_DELAY_MS as f64 - 0.5, "talked down to {srtt}");
    }

    #[test]
    fn violation_closes_the_connection() {
        let (mut c, mut s) = pair(ConnConfig::default());
        let dg = c.seal_raw(0, &[0x77, 1, 2, 3]);
        assert!(s.recv(0, &dg).is_ok());
        assert_eq!(s.violation(), Some(Violation::UnknownChunk));
        assert!(matches!(
            s.state(),
            ConnState::Closing {
                code: close_code::PROTOCOL_VIOLATION
            }
        ));
        let _ = CH_RELIABLE;
    }
}
