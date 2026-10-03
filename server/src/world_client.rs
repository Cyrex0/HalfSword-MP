//! Sidecar side of world-object replication (HSMPWorld v2), protocol v6: typed records end
//! to end (`hsmp_ipc::schema::world`; design in server/src/world.rs). No JSON, no codec, no
//! re-quantisation: HSMPWorld writes the records, this module copies their bytes to the
//! server and the server's records into the game's slots, keeping its tables over the rows.
//!
//! game -> sidecar (shared memory, `schema::world::SLOTS`):
//!   `world_out` (blob, `world_state`)          owned bodies' states: framed once per new
//!                                              (seq, epoch) and sent as they are.
//!   `world_manifest_out` / `world_dyn_out`     discovered bodies / my dropped items: each
//!                                              entry is proposed (re)sent until a reply with
//!                                              its request id arrives (`PROPOSE_MAX` per msg).
//!   `world_hash` (blob, `world_hash`)          consistency report: sent as it is once per
//!                                              (level, epoch, seq), while synced.
//!   G2S `world_claim` / `world_sync`           sent as they are (`on_g2s`); a sync request
//!                                              resets the tables first.
//! sidecar -> game:
//!   `world_owners`    merged owner records (higher ver wins), `sync` = synced, `manifest_len`.
//!   `world_manifest`  canonical static entries (sorted by id; insert-only).
//!   `world_dyn`       canonical dynamic entries (sorted by id; insert-only).
//!   `world_remote`    newest sample per body (+ sender-0 anchor rows), 10 s TTL.
//!   `world_consistency` the server's latest `world_verdict`, copied as it is.
//!
//! Scope: everything is gated by (level, epoch); an epoch change for our level clears the
//! tables and tells the game (owners with sync = false and the new epoch) so it restores its
//! objects and re-syncs; a manifest gap (the server lists more entries than we hold for
//! 3 s) re-syncs.

use crate::proto::PeerId;
use crate::SharedState;
use hsmp_ipc::layout::Bool;
use hsmp_ipc::record::{view, Invalid};
use hsmp_ipc::schema::world::{self as rec, DynEntry, ManifestEntry, OwnerRec, WorldObj, WorldSnap};
use std::collections::{BTreeMap, HashMap};

use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};
use tokio::net::UdpSocket;
use tokio::sync::Mutex;
use tokio::task::JoinHandle;
use tokio::time;
use tracing::{debug, info};

/// Drop owner samples not refreshed for this long from the published table.
const SAMPLE_TTL_MS: u64 = 10_000;
const MANIFEST_RESEND_MS: u64 = 1000;
const MANIFEST_MAX_TRIES: u32 = 10;
/// Server says it has more manifest entries than we do for this long -> re-sync.
const MANIFEST_GAP_MS: u64 = 3000;

#[derive(Clone, Copy)]
struct Sample { snap: WorldSnap, recv: u64 }

#[derive(Clone, Copy)]
enum Prop {
    Static(ManifestEntry),
    Dyn(DynEntry),
}

struct Proposal { entry: Prop, acked: bool, tries: u32, sent_at: u64 }

#[derive(Default)]
struct WorldRx {
    level: u32,
    epoch: u32,
    synced: bool,
    objs: HashMap<u32, Sample>,
    anchors: HashMap<u32, WorldObj>,
    owners: HashMap<u32, OwnerRec>,
    manifest: BTreeMap<u32, ManifestEntry>,
    dyns: BTreeMap<u32, DynEntry>,
    server_mlen: u32,
    gap_since: Option<u64>,
    /// Local proposals for (level, epoch) and outstanding request ids.
    proposals: HashMap<u32, Proposal>,
    req_ids: HashMap<u32, Vec<u32>>,
    next_req: u32,
    dirty_objs: bool,
    dirty_owners: bool,
    dirty_manifest: bool,
    dirty_dyn: bool,
    want_resync: bool,
    /// The latest verdict on one of our reports (the record payload as received).
    verdict: Option<Vec<u8>>,
}

impl WorldRx {
    fn reset(&mut self, level: u32, epoch: u32) {
        let next_req = self.next_req;
        *self = WorldRx { level, epoch, next_req, ..Default::default() };
        self.dirty_objs = true;
        self.dirty_owners = true;
        self.dirty_manifest = true;
        self.dirty_dyn = true;
    }
    fn known(&self) -> usize {
        self.manifest.len() + self.dyns.len()
    }
}

fn rx() -> &'static std::sync::Mutex<WorldRx> {
    static R: OnceLock<std::sync::Mutex<WorldRx>> = OnceLock::new();
    R.get_or_init(|| std::sync::Mutex::new(WorldRx::default()))
}

fn now_ms() -> u64 {
    static T0: OnceLock<Instant> = OnceLock::new();
    T0.get_or_init(Instant::now).elapsed().as_millis() as u64
}

/// Wrap-safe "a is newer than b".
fn newer(a: u32, b: u32) -> bool { (a.wrapping_sub(b) as i32) > 0 }

/// Scope gate for an inbound message. Returns false to drop it. An epoch
/// change for our level clears everything and tells Lua (sync = 0 with the
/// new epoch) so it restores its objects and re-syncs.
fn scope(r: &mut WorldRx, level: u32, epoch: u32, is_sync: bool) -> bool {
    if r.level == 0 || level != r.level { return false; }
    if is_sync {
        if !r.synced || epoch != r.epoch {
            if epoch != r.epoch {
                let l = r.level;
                r.reset(l, epoch);
            }
            r.synced = true;
            r.dirty_owners = true;
            info!(level, epoch, "world sync reply received");
        }
        return true;
    }
    if !r.synced { return false; }
    if epoch != r.epoch {
        let l = r.level;
        info!(level, old = r.epoch, new = epoch, "world epoch changed (round reset); Lua will re-sync");
        r.reset(l, epoch);
        return false;
    }
    true
}

fn accept_sample(r: &mut WorldRx, s: WorldSnap, recv: u64, snapshot: bool) {
    if s.sender == 0 {
        r.anchors.insert(s.obj.id, s.obj);
        r.dirty_objs = true;
        return;
    }
    match r.objs.get(&s.obj.id) {
        Some(_) if snapshot => return,
        Some(old) if old.snap.sender == s.sender && !newer(s.seq, old.snap.seq)
            && old.snap.seq.wrapping_sub(s.seq) < 1000 => return,
        _ => {}
    }
    r.objs.insert(s.obj.id, Sample { snap: s, recv });
    r.dirty_objs = true;
}

/// Every Welcome (first join, reconnect, server restart): ask the server for
/// a full world sync of the current level on the next pass (otherwise a
/// reconnect would keep the old session's world state).
pub fn on_welcome() {
    rx().lock().unwrap().want_resync = true;
}

/// One world record from the server (`peer` = the wire header's peer: the sender of a
/// `world_state`). Validated here (`record::view`), merged into the tables. Synchronous.
pub fn on_record(kind: u16, peer: PeerId, payload: &[u8]) -> Result<(), Invalid> {
    let t = now_ms();
    let mut r = rx().lock().unwrap();
    match kind {
        rec::K_WORLD_STATE => {
            let v = view::<rec::WorldStateHead>(payload)?;
            if !scope(&mut r, v.head.level, v.head.epoch, false) { return Ok(()); }
            let (seq, ts) = (v.head.seq, v.head.ts);
            for o in v.rows.iter() {
                accept_sample(&mut r, WorldSnap::new(peer, seq, ts, *o), t, false);
            }
        }
        rec::K_WORLD_SNAPSHOT => {
            let v = view::<rec::SnapHead>(payload)?;
            if !scope(&mut r, v.head.level, v.head.epoch, false) { return Ok(()); }
            for s in v.rows.iter() {
                accept_sample(&mut r, *s, t, true);
            }
        }
        rec::K_WORLD_OWNERS => {
            let v = view::<rec::OwnersHead>(payload)?;
            if !scope(&mut r, v.head.level, v.head.epoch, v.head.sync.get()) { return Ok(()); }
            for o in v.rows.iter() {
                if let Some(old) = r.owners.get(&o.id) {
                    if !newer(o.ver, old.ver) { continue; }
                }
                r.owners.insert(o.id, *o);
                r.dirty_owners = true;
            }
            let mlen = v.head.manifest_len;
            if r.server_mlen != mlen {
                r.dirty_owners = true;
            }
            r.server_mlen = mlen;
            if (mlen as usize) > r.known() {
                r.gap_since.get_or_insert(t);
            } else {
                r.gap_since = None;
            }
        }
        rec::K_WORLD_MANIFEST | rec::K_WORLD_DYN => {
            let req = if kind == rec::K_WORLD_MANIFEST {
                let v = view::<rec::ManifestHead>(payload)?;
                if !scope(&mut r, v.head.level, v.head.epoch, false) { return Ok(()); }
                for e in v.rows.iter() {
                    if !r.manifest.contains_key(&e.id) {
                        r.manifest.insert(e.id, *e);
                        r.dirty_manifest = true;
                    }
                }
                v.head.req
            } else {
                let v = view::<rec::DynHead>(payload)?;
                if !scope(&mut r, v.head.level, v.head.epoch, false) { return Ok(()); }
                for e in v.rows.iter() {
                    if !r.dyns.contains_key(&e.id) {
                        r.dyns.insert(e.id, *e);
                        r.dirty_dyn = true;
                    }
                }
                v.head.req
            };
            if req != 0 {
                if let Some(ids) = r.req_ids.remove(&req) {
                    for id in ids {
                        if let Some(p) = r.proposals.get_mut(&id) { p.acked = true; }
                    }
                }
            }
            if r.known() >= r.server_mlen as usize { r.gap_since = None; }
        }
        rec::K_WORLD_VERDICT => {
            let v = view::<rec::VerdictHead>(payload)?;
            if !scope(&mut r, v.head.level, v.head.epoch, false) { return Ok(()); }
            r.verdict = Some(payload.to_vec());
        }
        k => return Err(Invalid::Kind(k)),
    }
    Ok(())
}

/// G2S `world_claim` / `world_sync` from the game (hsmp-ipc thread, already validated):
/// framed and sent as they are. A sync request resets the tables for its level first.
pub fn on_g2s(l: &'static crate::ipc_shm::ShmLink, kind: u16, payload: &[u8]) {
    if kind == rec::K_WORLD_SYNC {
        let Ok(v) = view::<rec::WorldSync>(payload) else { return };
        let mut r = rx().lock().unwrap();
        let e = r.epoch;
        r.reset(v.head.level, e);
        debug!(level = v.head.level, "world sync requested");
    }
    l.send_record(hsmp_ipc::wire::message(kind, 0, 0, payload));
}

/// Proposals due for (re)sending, as framed `world_manifest` / `world_dyn` messages (at most
/// `PROPOSE_MAX` rows each, one request id per message).
fn due_proposals(r: &mut WorldRx, t: u64) -> Vec<Vec<u8>> {
    if !r.synced { return Vec::new(); }
    let known: Vec<u32> = r.manifest.keys().chain(r.dyns.keys()).copied().collect();
    for id in known { if let Some(p) = r.proposals.get_mut(&id) { p.acked = true; } }
    let mut due: Vec<Prop> = r.proposals.values_mut()
        .filter(|p| !p.acked && p.tries < MANIFEST_MAX_TRIES && t.saturating_sub(p.sent_at) >= MANIFEST_RESEND_MS)
        .map(|p| { p.tries += 1; p.sent_at = t; p.entry })
        .collect();
    if due.is_empty() { return Vec::new(); }
    let id_of = |p: &Prop| match p { Prop::Static(e) => e.id, Prop::Dyn(e) => e.id };
    due.sort_by_key(id_of);
    let st: Vec<ManifestEntry> = due.iter().filter_map(|p| if let Prop::Static(e) = p { Some(*e) } else { None }).collect();
    let dy: Vec<DynEntry> = due.iter().filter_map(|p| if let Prop::Dyn(e) = p { Some(*e) } else { None }).collect();
    let (level, epoch) = (r.level, r.epoch);
    let mut out = Vec::new();
    fn next_req(r: &mut WorldRx, ids: Vec<u32>) -> u32 {
        r.next_req = r.next_req.wrapping_add(1).max(1);
        let req = r.next_req;
        r.req_ids.insert(req, ids);
        req
    }
    for c in st.chunks(rec::PROPOSE_MAX) {
        let req = next_req(r, c.iter().map(|e| e.id).collect());
        let h = rec::ManifestHead { level, epoch, req, n: 0, _r: 0 };
        out.push(hsmp_ipc::wire::encode(0, 0, &h, c));
    }
    for c in dy.chunks(rec::PROPOSE_MAX) {
        let req = next_req(r, c.iter().map(|e| e.id).collect());
        let h = rec::DynHead { level, epoch, req, n: 0, _r: 0 };
        out.push(hsmp_ipc::wire::encode(0, 0, &h, c));
    }
    if r.req_ids.len() > 256 { r.req_ids.clear(); }
    out
}

/// The game's proposals (the `world_manifest_out` / `world_dyn_out` records): new entries
/// join the proposal table when they are for our synced (level, epoch).
fn add_proposals(r: &mut WorldRx, level: u32, epoch: u32, entries: impl Iterator<Item = Prop>) {
    if !(r.synced && level == r.level && epoch == r.epoch) { return; }
    for e in entries {
        let id = match &e { Prop::Static(x) => x.id, Prop::Dyn(x) => x.id };
        r.proposals.entry(id).or_insert(Proposal { entry: e, acked: false, tries: 0, sent_at: 0 });
    }
}

/// What changed since the last call, as (slot, record kind, payload) for the game. Owners and
/// the manifests come before the samples: Lua must know who owns a body before it sees its
/// samples. `force_objs` republishes the samples (the 1 s refresh that also applies the TTL).
fn take_publish(r: &mut WorldRx, t: u64, force_objs: bool) -> Vec<(&'static str, u16, Vec<u8>)> {
    use hsmp_ipc::record::to_payload;
    let mut out = Vec::new();
    let (level, epoch) = (r.level, r.epoch);
    if let Some(v) = r.verdict.take() {
        out.push(("world_consistency", rec::K_WORLD_VERDICT, v));
    }
    if std::mem::take(&mut r.dirty_manifest) {
        let rows: Vec<ManifestEntry> = r.manifest.values().take(rec::MANIFEST_MAX).copied().collect();
        out.push(("world_manifest", rec::K_WORLD_MANIFEST,
                  to_payload(&rec::ManifestHead { level, epoch, req: 0, n: 0, _r: 0 }, &rows)));
    }
    if std::mem::take(&mut r.dirty_dyn) {
        let rows: Vec<DynEntry> = r.dyns.values().take(rec::DYN_MAX).copied().collect();
        out.push(("world_dyn", rec::K_WORLD_DYN, to_payload(&rec::DynHead { level, epoch, req: 0, n: 0, _r: 0 }, &rows)));
    }
    if std::mem::take(&mut r.dirty_owners) {
        let mut rows: Vec<OwnerRec> = r.owners.values().copied().collect();
        rows.sort_by_key(|o| o.id);
        rows.truncate(rec::OWNERS_MAX);
        let h = rec::OwnersHead { level, epoch, manifest_len: r.server_mlen, n: 0, sync: Bool::from(r.synced), _r: 0 };
        out.push(("world_owners", rec::K_WORLD_OWNERS, to_payload(&h, &rows)));
    }
    r.objs.retain(|_, s| t.saturating_sub(s.recv) < SAMPLE_TTL_MS);
    if std::mem::take(&mut r.dirty_objs) || force_objs {
        let mut rows: Vec<WorldSnap> = r.objs.values().map(|s| s.snap).collect();
        rows.sort_by_key(|s| s.obj.id);
        let mut anchors: Vec<WorldSnap> = r.anchors.values().map(|o| WorldSnap::new(0, 0, 0, *o)).collect();
        anchors.sort_by_key(|s| s.obj.id);
        rows.extend(anchors);
        rows.truncate(rec::REMOTE_MAX);
        out.push(("world_remote", rec::K_WORLD_REMOTE,
                  to_payload(&rec::RemoteHead { level, epoch, n: 0, _r: 0 }, &rows)));
    }
    out
}

/// A game-written world slot's newest record, if it is fresh: written by this game instance
/// (writer epoch), valid, and for the current world (world epoch). The sidecar is the only
/// reader of these blobs.
fn read_game(l: &crate::ipc_shm::ShmLink, slot: &'static str, scratch: &mut Vec<u64>, out: &mut Vec<u8>) -> Option<u16> {
    use std::sync::atomic::Ordering::Acquire;
    let seg = l.segment();
    let (meta, kind, _) = seg.slot_ref(slot, 0)?.get(scratch, out)?;
    let h = &seg.header;
    let fresh = meta.valid == 1 && meta.writer_epoch == h.game.epoch.load(Acquire) && meta.world_epoch == h.world_epoch.load(Acquire);
    fresh.then_some(kind)
}

/// Outbound + state-publishing tasks. Detached; they live as long as the sidecar.
pub fn spawn_tasks(
    sock: Arc<UdpSocket>,
    shared: Arc<Mutex<SharedState>>,
) -> JoinHandle<()> {
    let _ = shared;
    tokio::spawn(async move {
        let mut ticker = time::interval(Duration::from_millis(25));
        ticker.set_missed_tick_behavior(time::MissedTickBehavior::Skip);
        let mut last_seq: Option<(u32, u32)> = None;
        let mut last_hash: Option<(u32, u32, u32)> = None;
        let mut last_write = 0u64;
        let mut stats_t = now_ms();
        let (mut st_sent, mut st_msgs) = (0u64, 0u64);
        let (mut scratch, mut buf) = (Vec::new(), Vec::new());
        loop {
            ticker.tick().await;
            let t = now_ms();
            let Some(l) = crate::ipc_shm::link().filter(|l| l.has(hsmp_ipc::schema::CAP_WORLD)) else { continue };
            let mut msgs: Vec<Vec<u8>> = Vec::new();

            // ---- manifest gap / reconnect re-sync ----
            let resync = {
                let mut r = rx().lock().unwrap();
                let gap = r.synced && r.gap_since.map_or(false, |g| t.saturating_sub(g) > MANIFEST_GAP_MS);
                if gap { r.gap_since = Some(t); }
                let w = std::mem::take(&mut r.want_resync) || gap;
                if w && r.level != 0 { Some(r.level) } else { None }
            };
            if let Some(level) = resync {
                info!(level, "world manifest gap / reconnect: re-syncing");
                msgs.push(hsmp_ipc::wire::encode(0, 0, &rec::WorldSync { level, _r: 0 }, &[]));
            }

            // ---- manifest proposals (reliable by resend until acked) ----
            if read_game(l, "world_manifest_out", &mut scratch, &mut buf) == Some(rec::K_WORLD_MANIFEST) {
                if let Ok(v) = view::<rec::ManifestHead>(&buf) {
                    let mut r = rx().lock().unwrap();
                    add_proposals(&mut r, v.head.level, v.head.epoch, v.rows.iter().map(|e| Prop::Static(*e)));
                }
            }
            if read_game(l, "world_dyn_out", &mut scratch, &mut buf) == Some(rec::K_WORLD_DYN) {
                if let Ok(v) = view::<rec::DynHead>(&buf) {
                    let mut r = rx().lock().unwrap();
                    add_proposals(&mut r, v.head.level, v.head.epoch, v.rows.iter().map(|e| Prop::Dyn(*e)));
                }
            }
            msgs.extend(due_proposals(&mut rx().lock().unwrap(), t));

            // ---- transforms (unreliable, newest only): the game's record as it is ----
            if read_game(l, "world_out", &mut scratch, &mut buf) == Some(rec::K_WORLD_STATE) {
                if let Ok(v) = view::<rec::WorldStateHead>(&buf) {
                    let key = (v.head.seq, v.head.epoch);
                    if last_seq != Some(key) && !v.rows.is_empty() {
                        last_seq = Some(key);
                        st_sent += v.rows.len() as u64;
                        msgs.push(hsmp_ipc::wire::message(rec::K_WORLD_STATE, 0, 0, &buf));
                    }
                }
            }

            // ---- consistency report (reliable, once per (level, epoch, seq)) ----
            if read_game(l, "world_hash", &mut scratch, &mut buf) == Some(rec::K_WORLD_HASH) {
                if let Ok(v) = view::<rec::HashHead>(&buf) {
                    let (level, epoch, seq) = (v.head.level, v.head.epoch, v.head.seq);
                    let ok = {
                        let r = rx().lock().unwrap();
                        r.synced && level == r.level && epoch == r.epoch
                    };
                    if ok && last_hash != Some((level, epoch, seq)) {
                        last_hash = Some((level, epoch, seq));
                        msgs.push(hsmp_ipc::wire::message(rec::K_WORLD_HASH, 0, 0, &buf));
                    }
                }
            }
            if !msgs.is_empty() {
                st_msgs += msgs.len() as u64;
                let _ = crate::send_msgs(&sock, msgs).await;
            }

            // ---- inbound tables -> the game's slots ----
            let publish = {
                let mut r = rx().lock().unwrap();
                let force = t.saturating_sub(last_write) > 1000;
                let p = take_publish(&mut r, t, force);
                if p.iter().any(|(s, _, _)| *s == "world_remote") { last_write = t; }
                p
            };
            for (slot, kind, payload) in publish {
                l.post_record(slot, None, kind, &payload);
            }

            if t.saturating_sub(stats_t) >= 10_000 {
                stats_t = t;
                if st_sent > 0 || st_msgs > 0 {
                    let r = rx().lock().unwrap();
                    debug!(level = r.level, epoch = r.epoch, synced = r.synced, sent_objs = st_sent, msgs = st_msgs,
                           manifest = r.manifest.len(), dyn_items = r.dyns.len(), owners = r.owners.len(),
                           samples = r.objs.len(), "world sidecar 10s");
                }
                st_sent = 0;
                st_msgs = 0;
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use hsmp_ipc::record::to_payload;
    use hsmp_ipc::schema::world::{WF_ASLEEP, WF_HELD};

    fn fresh(level: u32) {
        let mut r = rx().lock().unwrap();
        *r = WorldRx::default();
        r.level = level;
    }
    fn obj(id: u32, x: f32) -> WorldObj { WorldObj::from_parts(id, [x, 0.0, 0.0], [0.0, 90.0, 0.0], [1.0, 2.0, 3.0], WF_HELD) }
    fn state(level: u32, epoch: u32, seq: u32, ts: u32, objs: &[WorldObj]) -> Vec<u8> {
        to_payload(&rec::WorldStateHead { level, epoch, seq, ts, n: 0, _r: 0, _r2: 0 }, objs)
    }
    fn owners(level: u32, epoch: u32, sync: bool, mlen: u32, o: &[OwnerRec]) -> Vec<u8> {
        to_payload(&rec::OwnersHead { level, epoch, manifest_len: mlen, n: 0, sync: Bool::from(sync), _r: 0 }, o)
    }
    fn man(level: u32, epoch: u32, req: u32, ids: &[u32]) -> Vec<u8> {
        let rows: Vec<ManifestEntry> = ids.iter().map(|&id| ManifestEntry { id, chash: 1, pos: [0.0; 3], _r: 0 }).collect();
        to_payload(&rec::ManifestHead { level, epoch, req, n: 0, _r: 0 }, &rows)
    }
    fn published(p: &[(&'static str, u16, Vec<u8>)], slot: &str) -> Option<Vec<u8>> {
        p.iter().find(|(s, _, _)| *s == slot).map(|(_, _, b)| b.clone())
    }

    // All scenarios in one test: the module state is a process-wide static.
    #[test]
    fn scope_epoch_ordering_proposals_and_published_records() {
        fresh(5);
        // Nothing before the sync reply.
        on_record(rec::K_WORLD_STATE, 2, &state(5, 3, 1, 10, &[obj(1, 1.0)])).unwrap();
        assert!(rx().lock().unwrap().objs.is_empty());
        on_record(rec::K_WORLD_OWNERS, 0, &owners(5, 3, true, 0, &[])).unwrap();
        assert!(rx().lock().unwrap().synced);
        // Other level ignored.
        on_record(rec::K_WORLD_STATE, 2, &state(6, 3, 1, 10, &[obj(1, 1.0)])).unwrap();
        assert!(rx().lock().unwrap().objs.is_empty());
        // Newest seq per sender wins; reordered older dropped; the sender is the wire peer.
        on_record(rec::K_WORLD_STATE, 2, &state(5, 3, 5, 50, &[obj(1, 5.0)])).unwrap();
        on_record(rec::K_WORLD_STATE, 2, &state(5, 3, 4, 40, &[obj(1, 4.0)])).unwrap();
        assert_eq!(rx().lock().unwrap().objs[&1].snap.obj.pos[0], 5.0);
        assert_eq!(rx().lock().unwrap().objs[&1].snap.sender, 2);
        // Snapshot never overrides live; anchors are separate.
        let snaps = [WorldSnap::new(2, 1, 0, obj(1, 9.0)), WorldSnap::new(0, 0, 0, WorldObj { flags: WF_ASLEEP | (3 << 6), ..obj(1, 7.0) })];
        on_record(rec::K_WORLD_SNAPSHOT, 0, &to_payload(&rec::SnapHead { level: 5, epoch: 3, n: 0, _r: 0, _r2: 0 }, &snaps)).unwrap();
        {
            let r = rx().lock().unwrap();
            assert_eq!(r.objs[&1].snap.obj.pos[0], 5.0);
            assert_eq!(r.anchors[&1].pos[0], 7.0);
        }
        // Owner records: higher ver wins.
        on_record(rec::K_WORLD_OWNERS, 0, &owners(5, 3, false, 2, &[OwnerRec::new(1, 2, 9, 2)])).unwrap();
        on_record(rec::K_WORLD_OWNERS, 0, &owners(5, 3, false, 2, &[OwnerRec::new(1, 0, 8, 0)])).unwrap();
        assert_eq!(rx().lock().unwrap().owners[&1].owner, 2);
        assert!(rx().lock().unwrap().gap_since.is_some(), "server has 2 entries, we have 0");
        // A manifest reply acks the proposal and closes the gap.
        {
            let mut r = rx().lock().unwrap();
            r.proposals.insert(1, Proposal { entry: Prop::Static(ManifestEntry { id: 1, chash: 1, pos: [0.0; 3], _r: 0 }),
                                             acked: false, tries: 1, sent_at: 0 });
            r.req_ids.insert(42, vec![1]);
        }
        on_record(rec::K_WORLD_MANIFEST, 0, &man(5, 3, 42, &[1, 2])).unwrap();
        {
            let r = rx().lock().unwrap();
            assert!(r.proposals[&1].acked);
            assert_eq!(r.manifest.len(), 2);
            assert!(r.gap_since.is_none());
        }
        // The published records: what the game reads, valid as they are.
        let p = take_publish(&mut rx().lock().unwrap(), now_ms(), false);
        let names: Vec<&str> = p.iter().map(|(s, _, _)| *s).collect();
        assert!(names.iter().position(|s| *s == "world_owners") < names.iter().position(|s| *s == "world_remote"),
                "owners before samples: {names:?}");
        for (slot, kind, payload) in &p {
            hsmp_ipc::schema::check_payload(*kind, payload).unwrap_or_else(|e| panic!("{slot}: {e}"));
            assert_eq!(hsmp_ipc::schema::slot_by_name(slot).unwrap().kind, *kind);
        }
        let rem = published(&p, "world_remote").unwrap();
        let v = view::<rec::RemoteHead>(&rem).unwrap();
        assert_eq!(v.rows.len(), 2, "one live sample + one anchor");
        assert_eq!((v.rows[0].sender, v.rows[0].seq, v.rows[0].ts, v.rows[0].obj.pos[0]), (2, 5, 50, 5.0));
        assert_eq!((v.rows[1].sender, v.rows[1].obj.pos[0]), (0, 7.0));
        let q = v.rows[0].obj.quat();
        let h = std::f32::consts::FRAC_1_SQRT_2; // a 90 degree yaw quaternion
        assert!((q[2] - h).abs() < 1e-4 && (q[3] - h).abs() < 1e-4, "{q:?}");
        let ow = published(&p, "world_owners").unwrap();
        let v = view::<rec::OwnersHead>(&ow).unwrap();
        assert!(v.head.sync.get() && v.head.manifest_len == 2 && v.rows[0].ver == 9);
        let m = published(&p, "world_manifest").unwrap();
        assert_eq!(view::<rec::ManifestHead>(&m).unwrap().rows.iter().map(|e| e.id).collect::<Vec<_>>(), vec![1, 2]);
        assert!(take_publish(&mut rx().lock().unwrap(), now_ms(), false).is_empty(), "nothing new: nothing published");

        // Proposals: only for the synced (level, epoch); resent until acked; PROPOSE_MAX per message.
        {
            let mut r = rx().lock().unwrap();
            add_proposals(&mut r, 5, 9, std::iter::once(Prop::Static(ManifestEntry { id: 77, chash: 1, pos: [0.0; 3], _r: 0 })));
            assert!(!r.proposals.contains_key(&77), "another epoch: ignored");
            let many = (1000..1000 + rec::PROPOSE_MAX as u32 + 10).map(|id| Prop::Static(ManifestEntry { id, chash: 1, pos: [0.0; 3], _r: 0 }));
            add_proposals(&mut r, 5, 3, many);
            let d = DynEntry { id: rec::DYN_ID_BIT | (2 << 16) | 1, chash: 3, pos: [1.0; 3], dyn_owner: 2,
                               class_path: hsmp_ipc::layout::Str::new("/Game/A.A_C") };
            add_proposals(&mut r, 5, 3, std::iter::once(Prop::Dyn(d)));
            let msgs = due_proposals(&mut r, 10_000);
            assert_eq!(msgs.len(), 3, "two static messages + one dynamic");
            let mut reqs = Vec::new();
            for m in &msgs {
                let (wh, pl) = hsmp_ipc::wire::split(m).unwrap();
                hsmp_ipc::schema::check_payload(wh.kind, pl).unwrap();
                reqs.push(u32::from_le_bytes(pl[8..12].try_into().unwrap()));
            }
            assert!(reqs.iter().all(|q| *q != 0 && r.req_ids.contains_key(q)));
            assert!(due_proposals(&mut r, 10_500).is_empty(), "not due before MANIFEST_RESEND_MS");
            assert_eq!(due_proposals(&mut r, 11_000).len(), 3, "resent until acked");
        }

        // A verdict is copied as it is into world_consistency.
        let vh = rec::VerdictHead { level: 5, epoch: 3, seq: 9, other: 2, compared: 10, mismatched_n: 1,
                                    hash_match: Bool::FALSE, hash_equal: Bool::FALSE, n: 0, _r: 0 };
        let vp = to_payload(&vh, &[rec::Mismatch { id: 77, kind: 1, _r: [0; 3], dpos: 12.5, dang: 4.0 }]);
        on_record(rec::K_WORLD_VERDICT, 0, &vp).unwrap();
        let p = take_publish(&mut rx().lock().unwrap(), now_ms(), false);
        assert_eq!(published(&p, "world_consistency").unwrap(), vp);
        assert!(rx().lock().unwrap().manifest.len() == 2, "a verdict never lands in the manifest");

        // Hostile bytes are refused, nothing changes.
        let mut bad = state(5, 3, 6, 60, &[obj(1, 6.0)]);
        bad[24 + 28] = 0x10; // reserved flag bit
        assert!(on_record(rec::K_WORLD_STATE, 2, &bad).is_err());
        assert!(on_record(rec::K_WORLD_STATE, 2, &bad[..30]).is_err());
        assert!(on_record(0x04FF, 2, &[]).is_err());
        assert_eq!(rx().lock().unwrap().objs[&1].snap.seq, 5);

        // Epoch change for our level: everything cleared, Lua told via the owners record.
        on_record(rec::K_WORLD_OWNERS, 0, &owners(5, 4, false, 0, &[])).unwrap();
        {
            let mut r = rx().lock().unwrap();
            assert_eq!(r.epoch, 4);
            assert!(!r.synced && r.objs.is_empty() && r.owners.is_empty() && r.manifest.is_empty() && r.dirty_owners);
            let p = take_publish(&mut r, now_ms(), false);
            let ow = published(&p, "world_owners").unwrap();
            let v = view::<rec::OwnersHead>(&ow).unwrap();
            assert!(!v.head.sync.get() && v.head.epoch == 4 && v.rows.is_empty());
        }

        // Every Welcome (reconnect) asks for a world re-sync.
        rx().lock().unwrap().want_resync = false;
        on_welcome();
        assert!(rx().lock().unwrap().want_resync);
        // Request ids never 0.
        let mut r = WorldRx { next_req: u32::MAX, synced: true, level: 1, ..Default::default() };
        add_proposals(&mut r, 1, 0, std::iter::once(Prop::Static(ManifestEntry { id: 3, chash: 1, pos: [0.0; 3], _r: 0 })));
        let m = due_proposals(&mut r, 5000);
        let (_, pl) = hsmp_ipc::wire::split(&m[0]).unwrap();
        assert_eq!(u32::from_le_bytes(pl[8..12].try_into().unwrap()), 1);
    }
}
