//! Server mods, the server's half (docs/hosting/server-mods.md): the set built from
//! `--mods-dir` at startup, and serving it to joining players without hurting the ones
//! already playing.
//!
//! * The set is read once, validated (`manifest.rs`) and held in memory: what the manifest
//!   announces is exactly what is served, whatever happens to the folder later.
//! * A joining player pulls chunks (`mod_chunk_req`, at most [`MAX_QUEUE`] waiting per player).
//!   They are served from the serve loop (`server::mods_glue`), never from the receive path,
//!   within two token buckets: one per player (`--mods-rate-kbps`) and one for the whole
//!   server (`--mods-total-rate-kbps`), so joiners share a fixed slice of the upstream and the
//!   game traffic of the players in a match keeps the rest. Chunks travel on reliable
//!   channel 1, which the transport fills after the game streams.
//! * Each player may pull at most [`budget`] bytes per session (several resumes), then it is
//!   disconnected.

pub mod manifest;

use hsmp_ipc::schema::mods as rec;
use manifest::{Built, Limits};
use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;

pub type PeerId = u32;

/// Requests one player may have waiting.
pub const MAX_QUEUE: usize = 16;
/// Refused requests (bad file, queue full) before the player is treated as hostile.
pub const MAX_REFUSED: u32 = 64;

/// The server's configuration of its mods.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Config {
    pub limits: Limits,
    /// Seconds a joining player has to accept, download and load the set.
    pub timeout_s: u32,
    /// Bytes per second one player is served.
    pub peer_rate_bps: u64,
    /// Bytes per second every joining player together is served.
    pub total_rate_bps: u64,
}

impl Default for Config {
    fn default() -> Self {
        Config { limits: Limits::PROTOCOL, timeout_s: 300, peer_rate_bps: 2 << 20, total_rate_bps: 8 << 20 }
    }
}

/// A token bucket in bytes (server clock, ms).
#[derive(Debug, Clone)]
pub struct Bucket {
    rate_bps: f64,
    burst: f64,
    tokens: f64,
    at_ms: u64,
}

impl Bucket {
    pub fn new(rate_bps: u64, now_ms: u64) -> Bucket {
        // A burst always holds two chunks, else a slow rate could never send one.
        let burst = (rate_bps as f64 / 10.0).max(2.0 * rec::MAX_CHUNK as f64);
        Bucket { rate_bps: rate_bps as f64, burst, tokens: burst, at_ms: now_ms }
    }
    fn refill(&mut self, now_ms: u64) {
        let dt = now_ms.saturating_sub(self.at_ms) as f64 / 1000.0;
        self.at_ms = self.at_ms.max(now_ms);
        self.tokens = (self.tokens + dt * self.rate_bps).min(self.burst);
    }
    pub fn has(&mut self, n: usize, now_ms: u64) -> bool {
        self.refill(now_ms);
        self.tokens >= n as f64
    }
    pub fn take(&mut self, n: usize) {
        self.tokens -= n as f64;
    }
    pub fn give(&mut self, n: usize) {
        self.tokens = (self.tokens + n as f64).min(self.burst);
    }
}

/// What a chunk request turned into.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Req {
    Queued,
    /// Dropped (a file that does not exist, a full queue); the client asks again.
    Refused(&'static str),
    /// Over the byte budget or too many refusals: disconnect the player.
    Abuse(&'static str),
}

struct PeerQ {
    q: VecDeque<rec::ModChunkReq>,
    bucket: Bucket,
    /// Bytes queued or served this session.
    booked: u64,
    refused: u32,
}

/// The served set and the per-player transfer state.
pub struct Host {
    pub cfg: Config,
    pub built: Built,
    /// The two S2C messages (`mod_manifest`, `mod_files`), framed once.
    pub manifest_msg: Vec<u8>,
    pub files_msg: Vec<u8>,
    /// Some player is pending: the receive path checks the records gate (false = nobody is,
    /// the gate costs nothing). Set at once when a player becomes pending, cleared by the
    /// serve loop.
    pub any_pending: AtomicBool,
    serve: Mutex<Serve>,
}

struct Serve {
    peers: HashMap<PeerId, PeerQ>,
    global: Bucket,
    /// Round-robin start (fair between joiners).
    rr: usize,
}

/// Bytes one player may pull per session: three times the set, plus 4 MiB.
pub fn budget(total: u64) -> u64 {
    total.saturating_mul(3).saturating_add(4 << 20)
}

impl Host {
    pub fn new(cfg: Config, built: Built) -> Host {
        let (manifest_msg, files_msg) = manifest::to_messages(&built.manifest, rec::MAX_CHUNK as u32, cfg.timeout_s);
        Host {
            cfg,
            built,
            manifest_msg,
            files_msg,
            any_pending: AtomicBool::new(false),
            serve: Mutex::new(Serve { peers: HashMap::new(), global: Bucket::new(cfg.total_rate_bps, 0), rr: 0 }),
        }
    }

    fn serve(&self) -> std::sync::MutexGuard<'_, Serve> {
        self.serve.lock().unwrap_or_else(|e| e.into_inner())
    }

    pub fn set_hash(&self) -> [u8; 32] {
        self.built.manifest.set_hash
    }

    /// A player became pending (admission): the records gate turns on at once.
    pub fn mark_pending(&self) {
        self.any_pending.store(true, Ordering::Release);
    }

    /// A chunk request of `peer` (pending: the caller checked).
    pub fn request(&self, peer: PeerId, r: &rec::ModChunkReq, now_ms: u64) -> Req {
        let files = &self.built.data;
        let mut s = self.serve();
        let rate = self.cfg.peer_rate_bps;
        let p = s.peers.entry(peer).or_insert_with(|| PeerQ { q: VecDeque::new(), bucket: Bucket::new(rate, now_ms), booked: 0, refused: 0 });
        let refuse = |p: &mut PeerQ, why: &'static str| {
            p.refused += 1;
            if p.refused > MAX_REFUSED { Req::Abuse("too many bad mod requests") } else { Req::Refused(why) }
        };
        let Some(data) = files.get(r.file as usize) else { return refuse(p, "no such file") };
        if r.offset >= data.len() as u64 {
            return refuse(p, "offset past the end of the file");
        }
        if p.q.len() >= MAX_QUEUE {
            return refuse(p, "too many requests waiting");
        }
        let len = (r.len as u64).min(data.len() as u64 - r.offset);
        if p.booked + len > budget(self.built.manifest.total_bytes()) {
            return Req::Abuse("downloaded the server's mods too many times");
        }
        p.booked += len;
        p.q.push_back(rec::ModChunkReq { len: len as u32, ..*r });
        Req::Queued
    }

    /// Chunks due now, round robin over the players, within both buckets: (peer, request,
    /// framed `mod_chunk`). At most `max` per call.
    pub fn take_due(&self, now_ms: u64, max: usize) -> Vec<(PeerId, rec::ModChunkReq, Vec<u8>)> {
        let mut s = self.serve();
        let mut out = Vec::new();
        let mut ids: Vec<PeerId> = s.peers.iter().filter(|(_, p)| !p.q.is_empty()).map(|(id, _)| *id).collect();
        if ids.is_empty() {
            return out;
        }
        ids.sort_unstable();
        let start = s.rr % ids.len();
        ids.rotate_left(start);
        s.rr = s.rr.wrapping_add(1);
        let mut progress = true;
        while out.len() < max && progress {
            progress = false;
            for id in &ids {
                if out.len() >= max {
                    break;
                }
                let Serve { peers, global, .. } = &mut *s;
                let Some(p) = peers.get_mut(id) else { continue };
                let Some(r) = p.q.front().copied() else { continue };
                let n = r.len as usize;
                if !global.has(n, now_ms) {
                    return out; // the whole server's slice is used up for now
                }
                if !p.bucket.has(n, now_ms) {
                    continue;
                }
                global.take(n);
                p.bucket.take(n);
                p.q.pop_front();
                let data = &self.built.data[r.file as usize];
                let bytes = &data[r.offset as usize..r.offset as usize + n];
                let head = rec::ModChunkHead { offset: r.offset, req_id: r.req_id, n: 0, file: r.file, _r: [0; 6] };
                out.push((*id, r, hsmp_ipc::wire::encode(0, 0, &head, bytes)));
                progress = true;
            }
        }
        out
    }

    /// A chunk could not be queued on the connection (backpressure): put it back first in
    /// line and refund its tokens.
    pub fn retry(&self, peer: PeerId, r: rec::ModChunkReq) {
        let mut s = self.serve();
        let n = r.len as usize;
        s.global.give(n);
        if let Some(p) = s.peers.get_mut(&peer) {
            p.bucket.give(n);
            // The receive loop can refill the queue while send_out awaits
            // transport capacity. Preserve this original request without
            // exceeding the per-peer bound; the newest one can be asked again.
            if p.q.len() >= MAX_QUEUE {
                if let Some(dropped)=p.q.pop_back() {
                    p.booked=p.booked.saturating_sub(dropped.len as u64);
                }
            }
            p.q.push_front(r);
        }
    }

    /// Drop the waiting requests of `peer` (a new connection asks again), keeping its budget.
    pub fn clear_queue(&self, peer: PeerId) {
        if let Some(p) = self.serve().peers.get_mut(&peer) {
            let freed: u64 = p.q.iter().map(|r| r.len as u64).sum();
            p.booked = p.booked.saturating_sub(freed);
            p.q.clear();
        }
    }

    /// Forget every player not in `pending` (loaded, left).
    pub fn retain(&self, pending: impl Fn(PeerId) -> bool) {
        self.serve().peers.retain(|id, _| pending(*id));
    }

    /// Requests of `peer` waiting (tests).
    #[cfg(test)]
    pub fn queued(&self, peer: PeerId) -> usize {
        self.serve().peers.get(&peer).map_or(0, |p| p.q.len())
    }
}

#[cfg(test)]
mod tests {
    use super::manifest::*;
    use super::*;

    fn host(cfg: Config, sizes: &[usize]) -> Host {
        let files: Vec<(&str, Vec<u8>)> = sizes.iter().enumerate()
            .map(|(i, n)| (if i == 0 { MAIN } else { ["Scripts/a.lua", "Scripts/b.lua", "Scripts/c.lua"][i - 1] }, (0..*n).map(|x| x as u8).collect()))
            .collect();
        let fl: Vec<(&str, &str, &[u8])> = files.iter().map(|(p, b)| ("m", *p, b.as_slice())).collect();
        let d = super::manifest::tests::mods_dir(&fl);
        let (b, _) = build_from_dir(&d, &Limits::PROTOCOL).unwrap();
        let _ = std::fs::remove_dir_all(&d);
        Host::new(cfg, b)
    }

    fn req(file: u16, offset: u64, len: u32, id: u32) -> rec::ModChunkReq {
        rec::ModChunkReq { offset, req_id: id, len, file, _r: [0; 6] }
    }

    #[test]
    fn requests_are_checked_clipped_and_served_in_bytes() {
        let h = host(Config::default(), &[100_000]);
        assert_eq!(h.request(1, &req(0, 0, 32768, 1), 0), Req::Queued);
        assert_eq!(h.request(1, &req(0, 98_304, 32768, 2), 0), Req::Queued, "the last chunk is clipped to the file");
        assert!(matches!(h.request(1, &req(5, 0, 100, 3), 0), Req::Refused(_)), "no such file");
        assert!(matches!(h.request(1, &req(0, 100_000, 100, 4), 0), Req::Refused(_)), "past the end");
        let due = h.take_due(0, 16);
        assert_eq!(due.len(), 2);
        let (_, v) = hsmp_ipc::wire::decode::<rec::ModChunkHead>(&due[1].2).unwrap();
        assert_eq!((v.head.offset, v.rows.len(), v.head.req_id), (98_304, 100_000 - 98_304, 2));
        assert_eq!(v.rows[0], (98_304usize % 256) as u8);
        // a full queue refuses, and a flood of bad requests is abuse
        for i in 0..MAX_QUEUE as u32 { let _ = h.request(2, &req(0, 0, 1024, i), 0); }
        assert!(matches!(h.request(2, &req(0, 0, 1024, 99), 0), Req::Refused(_)));
        let mut last = Req::Queued;
        for i in 0..=MAX_REFUSED { last = h.request(3, &req(9, 0, 1, i), 0); }
        assert!(matches!(last, Req::Abuse(_)));
    }

    #[test]
    fn per_peer_and_global_rates_hold() {
        // 64 KiB/s per player, 128 KiB/s for the server; 3 players each ask for 1 MiB.
        let cfg = Config { peer_rate_bps: 64 << 10, total_rate_bps: 128 << 10, ..Config::default() };
        let h = host(cfg, &[1 << 20]);
        let mut served: HashMap<PeerId, u64> = HashMap::new();
        let mut next: HashMap<PeerId, u64> = HashMap::new();
        for t in (0..10_000u64).step_by(10) {
            for p in 1..=3u32 {
                let o = next.entry(p).or_insert(0);
                while h.queued(p) < 4 && *o < (1 << 20) {
                    assert_eq!(h.request(p, &req(0, *o, 32768, 0), t), Req::Queued);
                    *o += 32768;
                }
            }
            for (p, r, _) in h.take_due(t, 64) {
                *served.entry(p).or_default() += r.len as u64;
            }
        }
        let total: u64 = served.values().sum();
        // 10 s at 128 KiB/s plus the initial burst
        assert!(total <= 10 * (128 << 10) + 2 * 32768 + 32768, "server-wide rate exceeded: {total}");
        assert!(total >= 9 * (128 << 10), "the slice is used: {total}");
        for (p, b) in &served {
            assert!(*b <= 10 * (64 << 10) + 2 * 32768 + 32768, "peer {p} over its rate: {b}");
            assert!(*b >= (total / 3) * 2 / 3, "fair share: {served:?}");
        }
    }

    #[test]
    fn the_byte_budget_ends_repeated_downloads() {
        let h = host(Config::default(), &[1 << 20]);
        let mut got = None;
        let mut off = 0u64;
        for i in 0..1000u32 {
            let r = h.request(7, &req(0, off % (1 << 20), 32768, i), 0);
            if r != Req::Queued { got = Some(r); break; }
            off += 32768;
            let _ = h.take_due(i as u64 * 1000, 64);
        }
        assert!(matches!(got, Some(Req::Abuse(_))), "{got:?}");
        assert!(off <= budget(1 << 20) + 32768);
        // retry refunds and keeps the order; clearing frees the booked bytes
        let h = host(Config::default(), &[100_000]);
        h.request(1, &req(0, 0, 1000, 1), 0);
        h.request(1, &req(0, 1000, 1000, 2), 0);
        let due = h.take_due(0, 1);
        h.retry(1, due[0].1);
        assert_eq!(h.take_due(0, 2).iter().map(|d| d.1.req_id).collect::<Vec<_>>(), vec![1, 2]);
        h.request(1, &req(0, 0, 1000, 3), 0);
        h.clear_queue(1);
        assert_eq!(h.queued(1), 0);
        h.retain(|_| false);
        assert_eq!(h.queued(1), 0);
    }
    #[test]
    fn backpressure_retry_stays_bounded_when_receive_refills_queue() {
        let h=host(Config::default(),&[100_000]);
        let original=req(0,0,1024,1);
        assert_eq!(h.request(1,&original,0),Req::Queued);
        let sent=h.take_due(0,1);
        assert_eq!(sent.len(),1);
        for id in 2..=MAX_QUEUE as u32+1 {
            assert_eq!(h.request(1,&req(0,1024,1024,id),0),Req::Queued);
        }
        let booked=h.serve().peers[&1].booked;
        h.retry(1,sent[0].1);
        assert_eq!(h.queued(1),MAX_QUEUE);
        let s=h.serve();let p=&s.peers[&1];
        assert_eq!(p.q.front().unwrap().req_id,1,"original request retained first");
        assert_eq!(p.booked,booked-1024,"discarded unsent request refunded from transfer budget");
        assert!(p.q.iter().all(|r|r.req_id!=MAX_QUEUE as u32+1));
    }

}
