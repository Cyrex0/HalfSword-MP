//! The sidecar's world tables (HSMPWorld v2), pure logic: no I/O, no clock, no statics.
//! `world_client.rs` owns the process-wide instance and the tasks around it; the world
//! simulator (`tests/hsmpworld-sim`) drives this same code with a simulated clock.
//!
//! Inbound server records are validated (`record::view`) and merged here; [`take_publish`]
//! turns what changed into the game's slot records. Everything is gated by (level, epoch):
//! an epoch change for our level clears the tables and tells the game (owners with
//! sync = false and the new epoch) so it restores its objects and re-syncs.

use crate::proto::PeerId;
use hsmp_ipc::layout::Bool;
use hsmp_ipc::record::{view, Invalid};
use hsmp_ipc::schema::world::{self as rec, DynEntry, ManifestEntry, OwnerRec, WorldObj, WorldSnap};
use std::collections::{BTreeMap, HashMap};

/// Drop owner samples not refreshed for this long from the published table.
pub const SAMPLE_TTL_MS: u64 = 10_000;
pub const MANIFEST_RESEND_MS: u64 = 1000;
pub const MANIFEST_MAX_TRIES: u32 = 10;
/// Server says it has more manifest entries than we do for this long -> re-sync.
pub const MANIFEST_GAP_MS: u64 = 3000;

#[derive(Clone, Copy)]
pub struct Sample { pub snap: WorldSnap, pub recv: u64 }

#[derive(Clone, Copy)]
pub enum Prop {
    Static(ManifestEntry),
    Dyn(DynEntry),
}

pub struct Proposal { pub entry: Prop, pub acked: bool, pub tries: u32, pub sent_at: u64 }

#[derive(Default)]
pub struct WorldRx {
    pub level: u32,
    pub epoch: u32,
    pub synced: bool,
    pub objs: HashMap<u32, Sample>,
    pub anchors: HashMap<u32, WorldObj>,
    pub owners: HashMap<u32, OwnerRec>,
    pub manifest: BTreeMap<u32, ManifestEntry>,
    pub dyns: BTreeMap<u32, DynEntry>,
    pub server_mlen: u32,
    pub gap_since: Option<u64>,
    /// Local proposals for (level, epoch) and outstanding request ids.
    pub proposals: HashMap<u32, Proposal>,
    pub req_ids: HashMap<u32, Vec<u32>>,
    pub next_req: u32,
    pub dirty_objs: bool,
    pub dirty_owners: bool,
    pub dirty_manifest: bool,
    pub dirty_dyn: bool,
    pub want_resync: bool,
    /// The latest verdict on one of our reports (the record payload as received).
    pub verdict: Option<Vec<u8>>,
}

impl WorldRx {
    pub fn reset(&mut self, level: u32, epoch: u32) {
        let next_req = self.next_req;
        *self = WorldRx { level, epoch, next_req, ..Default::default() };
        self.dirty_objs = true;
        self.dirty_owners = true;
        self.dirty_manifest = true;
        self.dirty_dyn = true;
    }
    pub fn known(&self) -> usize {
        self.manifest.len() + self.dyns.len()
    }
}

/// Wrap-safe "a is newer than b".
pub fn newer(a: u32, b: u32) -> bool { (a.wrapping_sub(b) as i32) > 0 }

/// Scope gate for an inbound message. Returns false to drop it. An epoch
/// change for our level clears everything and tells Lua (sync = 0 with the
/// new epoch) so it restores its objects and re-syncs.
pub fn scope(r: &mut WorldRx, level: u32, epoch: u32, is_sync: bool) -> bool {
    if r.level == 0 || level != r.level { return false; }
    if is_sync {
        if !r.synced || epoch != r.epoch {
            if epoch != r.epoch {
                let l = r.level;
                r.reset(l, epoch);
            }
            r.synced = true;
            r.dirty_owners = true;
            tracing::info!(level, epoch, "world sync reply received");
        }
        return true;
    }
    if !r.synced { return false; }
    if epoch != r.epoch {
        let l = r.level;
        tracing::info!(level, old = r.epoch, new = epoch, "world epoch changed (round reset); Lua will re-sync");
        r.reset(l, epoch);
        return false;
    }
    true
}

pub fn accept_sample(r: &mut WorldRx, s: WorldSnap, recv: u64, snapshot: bool) {
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

/// One world record from the server (`peer` = the wire header's peer: the sender of a
/// `world_state`), received at `t` ms. Validated here (`record::view`), merged into the tables.
pub fn on_record(r: &mut WorldRx, kind: u16, peer: PeerId, payload: &[u8], t: u64) -> Result<(), Invalid> {
    match kind {
        rec::K_WORLD_STATE => {
            let v = view::<rec::WorldStateHead>(payload)?;
            if !scope(r, v.head.level, v.head.epoch, false) { return Ok(()); }
            let (seq, ts) = (v.head.seq, v.head.ts);
            for o in v.rows.iter() {
                accept_sample(r, WorldSnap::new(peer, seq, ts, *o), t, false);
            }
        }
        rec::K_WORLD_SNAPSHOT => {
            let v = view::<rec::SnapHead>(payload)?;
            if !scope(r, v.head.level, v.head.epoch, false) { return Ok(()); }
            for s in v.rows.iter() {
                accept_sample(r, *s, t, true);
            }
        }
        rec::K_WORLD_OWNERS => {
            let v = view::<rec::OwnersHead>(payload)?;
            if !scope(r, v.head.level, v.head.epoch, v.head.sync.get()) { return Ok(()); }
            for o in v.rows.iter() {
                if let Some(old) = r.owners.get(&o.id) {
                    if !newer(o.ver, old.ver) { continue; }
                }
                r.owners.insert(o.id, *o);
                r.dirty_owners = true;
                // leased: its old rest pose is stale (the release brings the new one)
                if o.owner != 0 && o.mode < rec::MODE_STATE && r.anchors.remove(&o.id).is_some() { r.dirty_objs = true; }
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
                if !scope(r, v.head.level, v.head.epoch, false) { return Ok(()); }
                for e in v.rows.iter() {
                    if let std::collections::btree_map::Entry::Vacant(slot) = r.manifest.entry(e.id) {
                        slot.insert(*e);
                        r.dirty_manifest = true;
                    }
                }
                v.head.req
            } else {
                let v = view::<rec::DynHead>(payload)?;
                if !scope(r, v.head.level, v.head.epoch, false) { return Ok(()); }
                for e in v.rows.iter() {
                    if let std::collections::btree_map::Entry::Vacant(slot) = r.dyns.entry(e.id) {
                        slot.insert(*e);
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
            if !scope(r, v.head.level, v.head.epoch, false) { return Ok(()); }
            r.verdict = Some(payload.to_vec());
        }
        k => return Err(Invalid::Kind(k)),
    }
    Ok(())
}

/// The game asked for a world sync of `level`: the tables start over (same epoch until the
/// reply says otherwise).
pub fn on_sync_request(r: &mut WorldRx, level: u32) {
    let e = r.epoch;
    r.reset(level, e);
}

/// A re-sync to send now (the level), if one is due: a reconnect asked for it, or the server
/// listed more manifest entries than we hold for MANIFEST_GAP_MS.
pub fn resync_due(r: &mut WorldRx, t: u64) -> Option<u32> {
    let gap = r.synced && r.gap_since.map_or(false, |g| t.saturating_sub(g) > MANIFEST_GAP_MS);
    if gap { r.gap_since = Some(t); }
    let w = std::mem::take(&mut r.want_resync) || gap;
    (w && r.level != 0).then_some(r.level)
}

/// Proposals due for (re)sending, as framed `world_manifest` / `world_dyn` messages (at most
/// `PROPOSE_MAX` rows each, one request id per message).
pub fn due_proposals(r: &mut WorldRx, t: u64) -> Vec<Vec<u8>> {
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
pub fn add_proposals(r: &mut WorldRx, level: u32, epoch: u32, entries: impl Iterator<Item = Prop>) {
    if !(r.synced && level == r.level && epoch == r.epoch) { return; }
    for e in entries {
        let id = match &e { Prop::Static(x) => x.id, Prop::Dyn(x) => x.id };
        r.proposals.entry(id).or_insert(Proposal { entry: e, acked: false, tries: 0, sent_at: 0 });
    }
}

/// What changed since the last call, as (slot, record kind, payload) for the game. Owners and
/// the manifests come before the samples: Lua must know who owns a body before it sees its
/// samples. `force_objs` republishes the samples (the 1 s refresh that also applies the TTL).
pub fn take_publish(r: &mut WorldRx, t: u64, force_objs: bool) -> Vec<(&'static str, u16, Vec<u8>)> {
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
