//! Sidecar side of the loadout domain (protocol v6 records, `hsmp_ipc::schema::loadout`).
//!
//! Outbound (one task, 250 ms): the game's record slots are read as bytes and sent as they
//! are, behind the 8-byte wire header:
//! - `loadout` (game blob): once per new version, and again on every new session; the
//!   reliable channel keeps the newest version per owner and fragments it (no chunking);
//! - `kit` (game slot): resent every 2 s until a `kit_verdict` for us acks its `seq`, then
//!   every 30 s (a server restart); immediately on change / new session;
//! - `kit_rules_req` (game slot, admin only): resent every 2 s until the server's rules
//!   match, at most 5 tries per request.
//!
//! Inbound (`handlers::handle_server_record`): `kit_verdict` -> per-peer slot `peer_kit`
//! (highest rev per peer), `kit_rules` -> slot `kit_rules` (highest rev), `loadout` -> per-peer
//! blob `peer_loadout` (newest version per peer). Each record is validated and copied into
//! shared memory as it arrived.

use crate::SharedState;
use hsmp_ipc::record::view;
use hsmp_ipc::schema::loadout::{BodyHead, Body2Head, Kit, KitRules, LoadoutHead, K_BODY, K_BODY2, K_KIT, K_KIT_RULES, K_KIT_RULES_REQ, K_KIT_VERDICT, K_LOADOUT};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::net::UdpSocket;
use tokio::sync::Mutex;
use tokio::task::JoinHandle;
use tokio::time;
use tracing::{debug, info, warn};

/// Bumped on every Welcome / new server epoch ([`reset_session`]): the
/// outbox task treats a new generation as "changed" and sends at once (a
/// restarted server may hand out the same peer id, so the id alone cannot
/// tell).
static GENERATION: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

fn generation() -> u64 {
    GENERATION.load(std::sync::atomic::Ordering::Relaxed)
}

/// "Due": never sent, or `every` has passed since `at`. `Option<Instant>`
/// instead of `Instant::now() - 1h`, which panics on Windows when the PC
/// booted less than an hour ago.
pub fn due(at: Option<Instant>, every: Duration) -> bool {
    at.map_or(true, |t| t.elapsed() >= every)
}

/// Resend an un-acked kit / rules request this often.
const KIT_RETRY: Duration = Duration::from_secs(2);
/// Re-send an acked kit this often anyway (covers a server restart that lost
/// it; the server just re-acks an unchanged kit without broadcasting).
const KIT_REFRESH: Duration = Duration::from_secs(30);
/// A non-admin's rules request is answered with the unchanged rules; give up
/// after this many tries.
const RULES_MAX_TRIES: u32 = 5;

/// Per-session high-water marks (inbound dedup + acks). A plain mutex: never held across
/// an await.
#[derive(Default)]
struct KitSync {
    /// Highest acked kit seq per peer (ours matters).
    acked: HashMap<u32, u32>,
    /// Highest kit rev written per peer.
    rev: HashMap<u32, u64>,
    /// Highest acknowledgement published at that revision. Saving an unchanged
    /// kit advances seq without changing rev, and the game still needs the ack.
    published_ack: HashMap<u32, u32>,
    rules_rev: u64,
    rules: Option<(u8, u16)>,
    /// Newest loadout version written per peer.
    loadout: HashMap<u32, u32>,
    /// Newest body version written per peer.
    body: HashMap<u32, u32>,
    body2: HashMap<u32, u32>,
}

fn sync() -> std::sync::MutexGuard<'static, KitSync> {
    static S: std::sync::OnceLock<std::sync::Mutex<KitSync>> = std::sync::OnceLock::new();
    S.get_or_init(Default::default).lock().unwrap_or_else(|e| e.into_inner())
}

/// Welcome / new epoch: forget every per-peer high-water mark of the old
/// server instance (kit revs, rules rev, acked kit seqs, loadout versions)
/// and make the outbox resend now.
pub fn reset_session() {
    GENERATION.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    *sync() = KitSync::default();
}

// ---- the game's slots ----------------------------------------------------------------------

/// Reads one game-written record slot through the attached link. `got` = a new valid value
/// of `kind` (payload in `out`), validated.
struct GameSlot {
    name: &'static str,
    kind: u16,
    scratch: Vec<u64>,
    out: Vec<u8>,
    last_ver: u64,
}

impl GameSlot {
    fn new(name: &'static str, kind: u16) -> GameSlot {
        GameSlot { name, kind, scratch: Vec::new(), out: Vec::new(), last_ver: 0 }
    }

    /// The current payload when it changed since the last call (a blob: when a new one was
    /// published). `None`: unchanged, absent, stale (another game instance) or invalid.
    fn poll(&mut self) -> Option<&[u8]> {
        let l = crate::ipc_shm::link()?;
        let seg = l.segment();
        let slot = seg.slot_ref(self.name, 0)?;
        if slot.version() == self.last_ver {
            return None;
        }
        let (meta, kind, ver) = slot.get(&mut self.scratch, &mut self.out)?;
        self.last_ver = ver;
        let game_epoch = seg.header.game.epoch.load(std::sync::atomic::Ordering::Acquire);
        if meta.valid != 1 || meta.writer_epoch != game_epoch || kind != self.kind {
            return None;
        }
        if let Err(e) = hsmp_ipc::schema::check_payload(kind, &self.out) {
            debug!(slot = self.name, error = %e, "game record invalid; not sent");
            return None;
        }
        Some(&self.out)
    }
}

pub fn spawn_outbox_task(sock: Arc<UdpSocket>, shared: Arc<Mutex<SharedState>>) -> JoinHandle<()> {
    tokio::spawn(async move {
        let mut ticker = time::interval(Duration::from_millis(250));
        let (mut lo_slot, mut kit_slot, mut rules_slot) =
            (GameSlot::new("loadout", K_LOADOUT), GameSlot::new("kit", K_KIT), GameSlot::new("kit_rules_req", K_KIT_RULES_REQ));
        // Latest valid payloads (a blob is taken once; slots are kept to resend).
        let (mut loadout, mut kit, mut rules): (Option<Vec<u8>>, Option<Vec<u8>>, Option<Vec<u8>>) = (None, None, None);
        let mut lo_sent: Option<(Vec<u8>, u32, u64)> = None; // (payload, pid, generation)
        let mut body_slot = GameSlot::new("body2", K_BODY2);
        let mut body: Option<Vec<u8>> = None;
        let mut body_sent: Option<(Vec<u8>, u32, u64)> = None;
        let mut body_sent_at: Option<Instant> = None;
        let mut kit_sent: Option<(Vec<u8>, u32, u64)> = None;
        let mut kit_sent_at: Option<Instant> = None; // never `now - 1h`
        let mut rules_sent: Option<(Vec<u8>, u32, u64)> = None;
        let mut rules_sent_at: Option<Instant> = None;
        let mut rules_tries = 0u32;
        loop {
            ticker.tick().await;
            if let Some(p) = lo_slot.poll() { loadout = Some(p.to_vec()); }
            if let Some(p) = kit_slot.poll() { kit = Some(p.to_vec()); }
            if let Some(p) = rules_slot.poll() { rules = Some(p.to_vec()); }
            if let Some(p) = body_slot.poll() { body = Some(p.to_vec()); }
            let (pid, connected, gen, is_admin) = {
                let s = shared.lock().await;
                (s.my_peer_id, s.status == "connected", generation(), crate::session_client::is_admin())
            };
            if !connected { continue; }

            // Appearance: once per version / session.
            if let Some(p) = &loadout {
                if lo_sent.as_ref().map_or(true, |(q, a, g)| q != p || (*a, *g) != (pid, gen)) {
                    let version = view::<LoadoutHead>(p).map(|v| v.head.version).unwrap_or(0);
                    info!(version, bytes = p.len(), "loadout sent");
                    send(&sock, K_LOADOUT, p).await;
                    lo_sent = Some((p.clone(), pid, gen));
                }
            }

            // Passport body: once per version / session, only to a server that reads it.
            if let Some(p) = &body {
                if crate::interact_client::has_cap(hsmp_net::net::caps::BODY2)
                    && (body_sent.as_ref().map_or(true, |(q, a, g)| q != p || (*a, *g) != (pid, gen))
                        || body_sent_at.map_or(true, |t| t.elapsed() >= Duration::from_secs(2)))
                {
                    let version = view::<Body2Head>(p).map(|v| v.head.version).unwrap_or(0);
                    info!(version, bytes = p.len(), "body sent");
                    send(&sock, K_BODY2, p).await;
                    body_sent = Some((p.clone(), pid, gen));
                    body_sent_at = Some(Instant::now());
                }
            }

            // Kit selection: until acked.
            if let Some(p) = &kit {
                if let Ok(v) = view::<Kit>(p) {
                    let seq = v.head.seq;
                    let acked = sync().acked.get(&pid).is_some_and(|a| *a >= seq);
                    let changed = kit_sent.as_ref().map_or(true, |(q, a, g)| q != p || (*a, *g) != (pid, gen));
                    if v.head.class.is_empty() {
                        // No selection made yet.
                    } else if changed || due(kit_sent_at, if acked { KIT_REFRESH } else { KIT_RETRY }) {
                        if changed {
                            info!(seq, class = %v.head.class.lossy(), armor = v.rows.len(), "kit sent");
                        } else if !acked {
                            debug!(seq, "kit not acked yet; resending");
                        }
                        send(&sock, K_KIT, p).await;
                        kit_sent = Some((p.clone(), pid, gen));
                        kit_sent_at = Some(Instant::now());
                    }
                }
            }

            // Only an admin's request can succeed; a request left from an earlier hosting
            // session must not spam refusals on someone else's server.
            if !is_admin { continue; }
            if let Some(p) = &rules {
                if let Ok(v) = view::<KitRules>(p) {
                    let (mode, budget) = (v.head.mode, v.head.budget);
                    let changed = rules_sent.as_ref().map_or(true, |(q, a, g)| q != p || (*a, *g) != (pid, gen));
                    if changed { rules_tries = 0; }
                    let matched = sync().rules == Some((mode, budget));
                    if v.head.seq != 0 && !matched && rules_tries < RULES_MAX_TRIES
                        && (changed || due(rules_sent_at, KIT_RETRY))
                    {
                        rules_tries += 1;
                        info!(mode, budget, try_n = rules_tries, "kit rules request sent");
                        send(&sock, K_KIT_RULES_REQ, p).await;
                        rules_sent_at = Some(Instant::now());
                    }
                    rules_sent = Some((p.clone(), pid, gen));
                }
            }
        }
    })
}

/// Frame a game record (header + its bytes as they are) and send it.
async fn send(sock: &UdpSocket, kind: u16, payload: &[u8]) {
    if let Err(e) = crate::send_msg(sock, hsmp_ipc::wire::message(kind, 0, 0, payload)).await {
        debug!(kind, error = %e, "send failed");
    }
}

// ---- inbound -------------------------------------------------------------------------------

/// `kit_verdict` about `peer`: track our ack, keep the highest rev, publish into `peer_kit`.
pub fn on_kit_verdict(peer: u32, payload: &[u8]) {
    let Ok(v) = view::<Kit>(payload) else {
        debug!(peer, "invalid kit_verdict dropped");
        return;
    };
    let (rev, ack) = (v.head.rev, v.head.seq);
    {
        let mut s = sync();
        if s.rev.get(&peer).is_some_and(|r| *r > rev) {
            return; // Reordered old state cannot replace the current kit or ack.
        }
        let a = s.acked.entry(peer).or_insert(0);
        *a = (*a).max(ack);
        if s.rev.get(&peer) == Some(&rev) && s.published_ack.get(&peer).is_some_and(|a| *a >= ack) {
            return; // Same kit revision and no newer acknowledgement.
        }
        let Some(l) = crate::ipc_shm::link() else { return };
        if !l.try_post_record("peer_kit", Some(peer), K_KIT_VERDICT, payload) { return; }
        // Commit only after queueing succeeds: a later retransmit can retry a
        // temporarily absent/full peer slot even when its seq was already acked.
        s.rev.insert(peer, rev);
        s.published_ack.insert(peer, ack);
    }
    if v.head.verdict == 0 {
        info!(peer, rev, class = %v.head.class.lossy(), "kit received");
    } else {
        warn!(peer, rev, class = %v.head.class.lossy(), verdict = v.head.verdict, reason = %v.head.reason.lossy(),
            "kit received (server replaced it)");
    }
}

/// `kit_rules`: remember them (the request resend stops when they match), publish newer ones.
pub fn on_kit_rules(payload: &[u8]) {
    let Ok(v) = view::<KitRules>(payload) else {
        debug!("invalid kit_rules dropped");
        return;
    };
    {
        let mut s = sync();
        s.rules = Some((v.head.mode, v.head.budget));
        if v.head.rev <= s.rules_rev { return; }
        s.rules_rev = v.head.rev;
    }
    if let Some(l) = crate::ipc_shm::link() {
        l.post_record("kit_rules", None, K_KIT_RULES, payload);
    }
    info!(mode = v.head.mode, budget = v.head.budget, rev = v.head.rev, "kit rules received");
}

/// `loadout` of `peer`: newest version only, published once into `peer_loadout`.
pub fn on_loadout(peer: u32, payload: &[u8]) {
    let Ok(v) = view::<LoadoutHead>(payload) else {
        debug!(peer, "invalid loadout dropped");
        return;
    };
    let version = v.head.version;
    {
        let mut s = sync();
        if s.loadout.get(&peer).is_some_and(|w| *w >= version) {
            return; // already have this or a newer version: duplicate / replay
        }
        s.loadout.insert(peer, version);
    }
    if let Some(l) = crate::ipc_shm::link() {
        l.post_record("peer_loadout", Some(peer), K_LOADOUT, payload);
    }
    info!(peer, version, bytes = payload.len(), rows = v.rows.len(), "loadout received");
}


/// `body` of `peer`: newest version only, published once into `peer_body`.
pub fn on_body(peer: u32, payload: &[u8]) {
    let Ok(v) = view::<BodyHead>(payload) else {
        debug!(peer, "invalid body dropped");
        return;
    };
    let version = v.head.version;
    if !body_fresh(peer, version) {
        return; // already have this or a newer version: duplicate / replay
    }
    if let Some(l) = crate::ipc_shm::link() {
        l.post_record("peer_body", Some(peer), K_BODY, payload);
    }
    info!(peer, version, bones = v.rows.len(), height = v.head.height_rate, muscle = v.head.muscle_rate, "body received");
}
pub fn on_body2(peer: u32, payload: &[u8]) {
    let Ok(v) = view::<Body2Head>(payload) else { return };
    let version = v.head.version;
    {
        let s = sync();
        if s.body2.get(&peer).is_some_and(|old| *old >= version) { return; }
        // Commit only after publishing succeeds, so a temporarily absent peer
        // slot can be filled by the server's later replay.
    }
    if let Some(l) = crate::ipc_shm::link() {
        if l.try_post_record("peer_body2", Some(peer), K_BODY2, payload) {
            sync().body2.insert(peer, version);
        }
    }
}

/// Remember `version` as `peer`'s newest body; false when it is not newer.
fn body_fresh(peer: u32, version: u32) -> bool {
    let mut s = sync();
    if s.body.get(&peer).is_some_and(|w| *w >= version) {
        return false;
    }
    s.body.insert(peer, version);
    true
}
#[cfg(test)]
mod kit_client_tests {
    use super::*;
    use hsmp_ipc::layout::Str;
    use hsmp_ipc::record::to_payload;
    use hsmp_ipc::schema::loadout::{ArmorRow, KitItem, ROW_PIECE};

    /// "Never sent" is None, not `Instant::now() - 1h` (which panics on
    /// Windows within an hour of boot). `due` never subtracts from an Instant.
    #[test]
    fn due_never_sent_is_due_without_instant_underflow() {
        assert!(due(None, Duration::from_secs(3600)));
        assert!(!due(Some(Instant::now()), KIT_RETRY));
        assert!(due(None, Duration::MAX), "None is due without any Instant arithmetic");
    }

    /// No `Instant::now() - <Duration>` anywhere in the sidecar sources.
    #[test]
    fn no_instant_minus_duration_in_sidecar_sources() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut files = vec![root.join("loadout_client.rs"), root.join("combat_client.rs"),
                             root.join("interact_client.rs"), root.join("world_client.rs")];
        for e in std::fs::read_dir(root.join("sidecar")).unwrap().flatten() { files.push(e.path()); }
        let pat = ["Instant::now()", " - "].concat();
        for f in files {
            let src = std::fs::read_to_string(&f).unwrap();
            for (i, l) in src.lines().enumerate() {
                let code = l.split("//").next().unwrap_or("");
                assert!(!(code.contains(&pat) && code.contains("Duration")),
                        "{}:{}: Instant minus Duration can panic: {}", f.display(), i + 1, l.trim());
            }
        }
    }

    fn verdict(class: &str, rev: u64, ack: u32) -> Vec<u8> {
        let mut k = Kit::default();
        k.class = Str::new(class);
        k.rev = rev;
        k.seq = ack;
        to_payload(&k, &[KitItem { id: Str::new("h_armet") }])
    }

    fn loadout(version: u32) -> Vec<u8> {
        let mut h = LoadoutHead::default();
        h.version = version;
        let mut r = ArmorRow::default();
        r.class = Str::new("@Armor/X/BP_Torso");
        r.flags = ROW_PIECE;
        to_payload(&h, &[r])
    }

    #[tokio::test]
    async fn saving_an_unchanged_kit_publishes_its_new_ack_without_a_new_revision() {
        let _serial = crate::ipc_shm::ShmLink::test_lock();
        let l = crate::ipc_shm::ShmLink::test_global();
        reset_session();
        on_kit_verdict(2, &verdict("knight", 50, 7));
        on_kit_verdict(2, &verdict("knight", 50, 8));
        assert_eq!(sync().acked.get(&2), Some(&8));
        assert_eq!(l.test_slot("peer_kit", Some(2)), Some((K_KIT_VERDICT, verdict("knight", 50, 8))),
            "the game and lab receive the repeated SAVE acknowledgement");
        on_kit_verdict(2, &verdict("knight", 50, 7));
        on_kit_verdict(2, &verdict("duelist", 49, 99));
        assert_eq!(sync().acked.get(&2), Some(&8), "old revision cannot poison the ack high-water mark");
        assert_eq!(l.test_slot("peer_kit", Some(2)), Some((K_KIT_VERDICT, verdict("knight", 50, 8))));
        on_kit_verdict(2, &verdict("duelist", 51, 8));
        assert_eq!(l.test_slot("peer_kit", Some(2)), Some((K_KIT_VERDICT, verdict("duelist", 51, 8))),
            "changed rules still publish a new revision at the same acknowledgement");
    }

    /// After a server restart revs restart at 1; a reset makes the
    /// sidecar accept (and publish) them again. Records reach their slots as they arrived.
    #[tokio::test]
    async fn reset_session_accepts_restarted_revs() {
        let _serial = crate::ipc_shm::ShmLink::test_lock();
        let l = crate::ipc_shm::ShmLink::test_global();
        on_kit_verdict(2, &verdict("knight", 50, 1));
        assert_eq!(sync().rev.get(&2), Some(&50));
        on_kit_verdict(2, &verdict("duelist", 40, 1));
        assert_eq!(l.test_slot("peer_kit", Some(2)).map(|s| s.1), Some(verdict("knight", 50, 1)), "older rev dropped");
        let mut kr = KitRules { rev: 40, seq: 0, budget: 100, mode: 1, _r: 0 };
        on_kit_rules(&to_payload(&kr, &[]));
        let g0 = generation();
        reset_session();
        assert!(generation() > g0, "outboxes see a new session");
        on_kit_verdict(2, &verdict("duelist", 1, 1));
        assert_eq!(sync().rev.get(&2), Some(&1), "rev 1 of the new server accepted");
        assert_eq!(l.test_slot("peer_kit", Some(2)), Some((K_KIT_VERDICT, verdict("duelist", 1, 1))));
        kr.rev = 1;
        kr.mode = 2;
        on_kit_rules(&to_payload(&kr, &[]));
        assert_eq!(sync().rules_rev, 1);
        assert_eq!(l.test_slot("kit_rules", None), Some((K_KIT_RULES, to_payload(&kr, &[]))));
        // Loadout versions too.
        on_loadout(3, &loadout(9));
        on_loadout(3, &loadout(2));
        assert_eq!(sync().loadout.get(&3), Some(&9), "older version dropped");
        assert_eq!(l.test_slot("peer_loadout", Some(3)), Some((K_LOADOUT, loadout(9))));
        reset_session();
        on_loadout(3, &loadout(2));
        assert_eq!(sync().loadout.get(&3), Some(&2));
        // Hostile bytes never reach a slot.
        on_loadout(4, &[1, 2, 3]);
        on_kit_verdict(4, &[0xff; 300]);
        assert!(l.test_slot("peer_loadout", Some(4)).is_none() && l.test_slot("peer_kit", Some(4)).is_none());
    }


    fn body_rec(version: u32, height: f32) -> Vec<u8> {
        use hsmp_ipc::schema::loadout::BodyBone;
        let mut h = BodyHead::default();
        h.version = version;
        h.height_rate = height;
        h.muscle_rate = 0.1;
        h.mass_scale_bp = 1.0;
        h.char_scale = [1.0; 3];
        to_payload(&h, &[BodyBone { bone: Str::new("pelvis"), mass: 6.0, mass_scale: 1.0 }])
    }

    /// A peer's passport body reaches `peer_body` once per newer version; hostile bytes
    /// never; the own `body` slot is read like every game slot.
    #[tokio::test]
    async fn body2_preserves_original_life_height_and_full_mass_rows_in_its_own_slot() {
        let _serial = crate::ipc_shm::ShmLink::test_lock();
        let l = crate::ipc_shm::ShmLink::test_global();
        let h = Body2Head { match_id:80,round:2,life:130,version:50,height_rate:0.9,muscle_rate:0.1,
            mass_scale_bp:1.0,char_scale:[1.0;3],native_height:0.76,actor_scale:[0.97;3],
            pawn:Str::new("Willie_BP_C_84"),..Default::default() };
        let rows = vec![hsmp_ipc::schema::loadout::BodyBone{bone:Str::new("pelvis"),mass:6.0,mass_scale:1.0};24];
        let p = to_payload(&h,&rows);
        on_body2(77,&p);
        assert_eq!(l.test_slot("peer_body2",Some(77)),Some((K_BODY2,p.clone())));
        on_body2(77,&to_payload(&Body2Head{version:49,life:2,..h},&rows));
        assert_eq!(l.test_slot("peer_body2",Some(77)),Some((K_BODY2,p.clone())));
        on_body(77,&body_rec(999,0.5));
        assert_eq!(l.test_slot("peer_body2",Some(77)),Some((K_BODY2,p.clone())),"legacy body cannot overwrite body2");
        let seg = l.segment();
        let epoch = seg.header.game.epoch.load(std::sync::atomic::Ordering::Acquire);
        let meta=hsmp_ipc::schema::SlotMeta{writer_epoch:epoch,valid:1,..Default::default()};
        let mut scratch=Vec::new();
        assert!(seg.slot_ref("body2",0).unwrap().put(meta,K_BODY2,&p,&mut scratch));
        assert_eq!(GameSlot::new("body2",K_BODY2).poll(),Some(p.as_slice()));
    }
    #[tokio::test]
    async fn body_records_reach_the_peer_slot_and_the_game_slot_is_read() {
        let _serial = crate::ipc_shm::ShmLink::test_lock();
        let l = crate::ipc_shm::ShmLink::test_global();
        on_body(7, &body_rec(5, 0.9));
        on_body(7, &body_rec(4, 0.2));
        assert_eq!(l.test_slot("peer_body", Some(7)), Some((K_BODY, body_rec(5, 0.9))), "older version dropped");
        on_body(7, &body_rec(6, 0.3));
        assert_eq!(l.test_slot("peer_body", Some(7)), Some((K_BODY, body_rec(6, 0.3))));
        on_body(8, &body_rec(1, 9.0)); // Height Rate out of range
        on_body(8, &[0u8; 7]);
        assert!(l.test_slot("peer_body", Some(8)).is_none());
        let seg = l.segment();
        let game_epoch = seg.header.game.epoch.load(std::sync::atomic::Ordering::Acquire);
        let meta = hsmp_ipc::schema::SlotMeta { writer_epoch: game_epoch, valid: 1, ..Default::default() };
        let mut scratch = Vec::new();
        let mut gs = GameSlot::new("body", K_BODY);
        assert!(seg.slot_ref("body", 0).unwrap().put(meta, K_BODY, &body_rec(2, 0.5), &mut scratch));
        assert_eq!(gs.poll(), Some(&body_rec(2, 0.5)[..]));
        assert!(gs.poll().is_none());
    }
    /// The outbox reads the game's slots as bytes: a fresh valid value is seen once, a value
    /// from another game instance (writer epoch) or of the wrong kind never.
    #[test]
    fn game_slot_reads_fresh_valid_values_once() {
        let l = crate::ipc_shm::ShmLink::test_global();
        let seg = l.segment();
        let game_epoch = seg.header.game.epoch.load(std::sync::atomic::Ordering::Acquire);
        let meta = |epoch| hsmp_ipc::schema::SlotMeta { writer_epoch: epoch, valid: 1, ..Default::default() };
        let mut scratch = Vec::new();
        let slot = seg.slot_ref("kit_rules_req", 0).unwrap();
        let req = to_payload(&KitRules { rev: 0, seq: 3, budget: 30, mode: 2, _r: 0 }, &[]);
        let mut gs = GameSlot::new("kit_rules_req", K_KIT_RULES_REQ);
        assert!(slot.put(meta(game_epoch ^ 1), K_KIT_RULES_REQ, &req, &mut scratch));
        assert!(gs.poll().is_none(), "another game instance's value");
        assert!(slot.put(meta(game_epoch), K_KIT_RULES_REQ, &req, &mut scratch));
        assert_eq!(gs.poll(), Some(&req[..]));
        assert!(gs.poll().is_none(), "unchanged: seen once");
        assert!(slot.put(meta(game_epoch), K_KIT_RULES_REQ, &[0u8; 16][..0], &mut scratch));
        assert!(gs.poll().is_none(), "an empty / invalid value is not sent");
    }
}
