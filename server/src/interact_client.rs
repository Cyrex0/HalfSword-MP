//! Sidecar side of the interaction channel (docs/development/subsystems/interact.md; server
//! model: `interact/`). Capability `caps::INTERACT`: nothing is sent unless the connection
//! negotiated it.
//!
//! One record, `hsmp_ipc::schema::interact::Interact`, in both directions:
//!
//! - G2S `interact` (HSMPInteract, what WE did to a stand-in) -> [`on_g2s`]: the sidecar mints
//!   the wire grab id (monotonic for the sidecar's lifetime, so a Lua reload can never reuse an
//!   id the server already closed), patches it into `id` IN PLACE and frames the same bytes.
//!   One live grab per hand; a late update of a closed grab is dropped. A grab update goes
//!   out as `interact_grab_r` / `interact_grab_l`: the reliable channel supersedes an unacked
//!   update of the same hand.
//! - S2C (any of the three kinds) -> [`on_s2c`]: validated and pushed into the S2G ring as
//!   `interact` with the header's peer (= the initiator, 0 = the server). For a server answer
//!   about OUR grab (denial / server end) the mod's own grab id is patched back into `gid` and
//!   the grab is closed here.
//!
//! No JSON, no bone names: the bone travels as its `HERO_BONES` index (the mod maps names).

use hsmp_ipc::record::{view, Invalid};
use hsmp_ipc::schema::interact::{self as ix, kind as K, Interact};
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::OnceLock;
use tracing::{debug, info, warn};

/// Negotiated capabilities of the current connection (0 = not connected).
static CAPS: AtomicU64 = AtomicU64::new(0);

/// Transport: the connection came up with these negotiated caps.
pub fn set_caps(caps: u64) {
    CAPS.store(caps, Ordering::Relaxed);
}

pub fn negotiated() -> bool {
    CAPS.load(Ordering::Relaxed) & hsmp_net::net::caps::INTERACT != 0
}

fn kind_name(k: u8) -> &'static str {
    match k {
        K::GRAB_START => "grab_start",
        K::GRAB_UPDATE => "grab_update",
        K::GRAB_END => "grab_end",
        K::IMPULSE => "impulse",
        K::GRAB_DENIED => "grab_denied",
        _ => "unknown",
    }
}

fn bone_name(b: u8) -> &'static str {
    ix::HERO_BONES.get(b as usize).copied().unwrap_or("?")
}

/// Lua grab id <-> wire grab id, per hand.
#[derive(Debug, Default)]
pub struct Ids {
    next_wire: u32,
    next_impulse: u32,
    /// (hand, lua gid) -> (wire id, target)
    by_lua: HashMap<(u8, u32), (u32, u32)>,
    /// wire id -> (hand, lua gid)
    by_wire: HashMap<u32, (u8, u32)>,
    /// Grabs that ended (server denial / end, or our own end): a late `grab_update` for one
    /// must not mint a new wire id and revive it.
    closed: std::collections::VecDeque<(u8, u32)>,
}

const CLOSED_CAP: usize = 64;

impl Ids {
    fn close(&mut self, key: (u8, u32)) {
        if !self.closed.contains(&key) {
            self.closed.push_back(key);
            while self.closed.len() > CLOSED_CAP {
                self.closed.pop_front();
            }
        }
    }
    fn is_closed(&self, key: (u8, u32)) -> bool {
        self.closed.contains(&key)
    }
    fn reopen(&mut self, key: (u8, u32)) {
        self.closed.retain(|k| *k != key);
    }

    fn wire_for(&mut self, hand: u8, gid: u32, target: u32) -> u32 {
        if let Some((w, t)) = self.by_lua.get(&(hand, gid)) {
            if *t == target {
                return *w;
            }
        }
        self.next_wire = self.next_wire.wrapping_add(1).max(1);
        let w = self.next_wire;
        // One live grab per hand: forget older mappings of this hand.
        let stale: Vec<(u8, u32)> = self.by_lua.keys().filter(|(h, _)| *h == hand).copied().collect();
        for k in stale {
            if let Some((ow, _)) = self.by_lua.remove(&k) {
                self.by_wire.remove(&ow);
            }
        }
        self.by_lua.insert((hand, gid), (w, target));
        self.by_wire.insert(w, (hand, gid));
        w
    }

    fn end(&mut self, hand: u8, gid: u32) -> Option<(u32, u32)> {
        let (w, t) = self.by_lua.remove(&(hand, gid))?;
        self.by_wire.remove(&w);
        self.close((hand, gid));
        Some((w, t))
    }

    /// The (hand, Lua gid) of one of our wire grab ids (server answers).
    pub fn lua_gid(&self, wire: u32) -> Option<(u8, u32)> {
        self.by_wire.get(&wire).copied()
    }

    fn forget_wire(&mut self, wire: u32) {
        if let Some(k) = self.by_wire.remove(&wire) {
            self.by_lua.remove(&k);
            self.close(k);
        }
    }

    /// One G2S record -> the record to send, `id` (and for an end, `target_peer`) patched in
    /// place. `None`: nothing to send (a client-only kind, an end / update of a grab we never
    /// started or already closed).
    pub fn outbound(&mut self, ev: &mut Interact) -> Option<()> {
        let (hand, g) = (ev.hand, ev.gid);
        match ev.kind {
            K::IMPULSE => {
                self.next_impulse = self.next_impulse.wrapping_add(1).max(1);
                ev.id = self.next_impulse;
                ev.hand = 0;
            }
            K::GRAB_START | K::GRAB_UPDATE => {
                if ev.target_peer == 0 {
                    return None;
                }
                if ev.kind == K::GRAB_START {
                    self.reopen((hand, g)); // a new grab (a Lua reload reuses gids)
                } else if self.is_closed((hand, g)) {
                    return None; // late update of an ended grab
                }
                ev.id = self.wire_for(hand, g, ev.target_peer);
            }
            K::GRAB_END => {
                let (w, t) = self.end(hand, g)?;
                ev.id = w;
                ev.target_peer = t;
            }
            _ => return None, // GRAB_DENIED is the server's
        }
        if ev.target_peer == 0 {
            return None;
        }
        Some(())
    }

    /// One S2C record from `from` (0 = the server) -> the record for the game. A server answer
    /// about our grab gets our Lua gid patched in and closes the grab here.
    pub fn inbound(&mut self, from: u32, ev: &mut Interact) {
        if from == 0 {
            ev.gid = self.lua_gid(ev.id).map_or(0, |(_, g)| g);
            self.forget_wire(ev.id);
        } else {
            ev.gid = 0; // the initiator's own id means nothing here
        }
    }
}

fn ids() -> &'static std::sync::Mutex<Ids> {
    static S: OnceLock<std::sync::Mutex<Ids>> = OnceLock::new();
    S.get_or_init(|| std::sync::Mutex::new(Ids::default()))
}

/// Events sent since the last stats line (logged by [`on_g2s`] about every 10 s).
static SENT: AtomicU64 = AtomicU64::new(0);

/// A G2S `interact` record from the game (validated by the ipc thread): mint / map the wire id
/// and frame it for the server. Runs on the `hsmp-ipc` thread (no blocking).
pub fn on_g2s(l: &'static crate::ipc_shm::ShmLink, payload: &[u8]) {
    let Some(msg) = outbound_msg(payload) else { return };
    l.send_record(msg);
}

/// The C2S message for one G2S payload, or `None` (not negotiated / nothing to send).
pub fn outbound_msg(payload: &[u8]) -> Option<Vec<u8>> {
    static WARNED: AtomicBool = AtomicBool::new(false);
    if !negotiated() {
        if !WARNED.swap(true, Ordering::Relaxed) {
            warn!("interact: the server did not negotiate INTERACT; interactions stay local");
        }
        return None;
    }
    let mut ev = match view::<Interact>(payload) {
        Ok(v) => v.head(),
        Err(e) => {
            debug!(error = %e, "interact: invalid G2S record dropped");
            return None;
        }
    };
    ids().lock().unwrap_or_else(|e| e.into_inner()).outbound(&mut ev)?;
    if ev.kind == K::GRAB_START || ev.kind == K::GRAB_END {
        info!(target = ev.target_peer, kind = kind_name(ev.kind), id = ev.id, hand = ev.hand,
              bone = bone_name(ev.bone), "interact: our grab");
    }
    let n = SENT.fetch_add(1, Ordering::Relaxed) + 1;
    if n.is_multiple_of(500) {
        info!(sent = n, "interact: events sent");
    }
    Some(hsmp_ipc::wire::message(ix::wire_kind(&ev), 0, 0, hsmp_ipc::bytemuck::bytes_of(&ev)))
}

/// One S2C interaction record (`h.peer` = initiator, 0 = server): the S2G record for the
/// game (`interact`, peer copied), or the refusal.
pub fn inbound_record(from: u32, payload: &[u8]) -> Result<Interact, Invalid> {
    let mut ev = view::<Interact>(payload)?.head();
    ids().lock().unwrap_or_else(|e| e.into_inner()).inbound(from, &mut ev);
    Ok(ev)
}

/// S2C (receive path): hand the record to HSMPInteract through the S2G ring.
pub fn on_s2c(h: hsmp_ipc::wire::WireHdr, payload: &[u8]) {
    let ev = match inbound_record(h.peer, payload) {
        Ok(e) => e,
        Err(e) => {
            debug!(kind = h.kind, error = %e, "interact: invalid S2C record dropped");
            return;
        }
    };
    if let Some(l) = crate::ipc_shm::link() {
        l.push_record(ix::K_INTERACT, 0, h.peer, hsmp_ipc::bytemuck::bytes_of(&ev));
    }
    match ev.kind {
        K::IMPULSE => debug!(from = h.peer, bone = bone_name(ev.bone), "interact: impulse on us"),
        K::GRAB_UPDATE => {}
        _ => info!(from = h.peer, kind = kind_name(ev.kind), id = ev.id, bone = bone_name(ev.bone), "interact"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use hsmp_ipc::schema::interact::{K_INTERACT, K_INTERACT_GRAB_L, K_INTERACT_GRAB_R};

    fn g(kind: u8, target: u32, gid: u32, hand: u8, bone: u8) -> Interact {
        Interact { kind, target_peer: target, gid, hand, bone, point: [1.0, 2.0, 3.0], vector: [10.0, 20.0, 30.0],
                   ts: 5000, ..Default::default() }
    }

    fn out(ids: &mut Ids, mut e: Interact) -> Option<Interact> {
        ids.outbound(&mut e).map(|_| e)
    }

    #[test]
    fn hero_bones_match_posecodec() {
        assert_eq!(&ix::HERO_BONES[..], crate::posecodec::HERO_BONES);
    }

    #[test]
    fn grabs_get_monotonic_wire_ids() {
        let mut ids = Ids::default();
        let s = out(&mut ids, g(K::GRAB_START, 2, 7, 1, 5)).unwrap();
        assert_eq!((s.kind, s.target_peer, s.id, s.hand, s.bone, s.point, s.vector, s.ts, s.gid),
                   (K::GRAB_START, 2, 1, 1, 5, [1.0, 2.0, 3.0], [10.0, 20.0, 30.0], 5000, 7));
        let u = out(&mut ids, g(K::GRAB_UPDATE, 2, 7, 1, 5)).unwrap();
        assert_eq!((u.kind, u.id), (K::GRAB_UPDATE, 1));
        assert_eq!(ids.lua_gid(1), Some((1, 7)));
        let e = out(&mut ids, Interact { target_peer: 0, ..g(K::GRAB_END, 2, 7, 1, 0) }).unwrap();
        assert_eq!((e.kind, e.id, e.target_peer), (K::GRAB_END, 1, 2), "end: target from the mapping");
        // Ending again / an unknown grab: nothing to send.
        assert!(out(&mut ids, g(K::GRAB_END, 2, 7, 1, 0)).is_none());
        // A Lua reload restarts its gids at 1: the wire id still grows.
        let s2 = out(&mut ids, g(K::GRAB_START, 2, 7, 1, 6)).unwrap();
        assert_eq!(s2.id, 2);
        // An update for a grab whose start we never saw opens a new wire id.
        let u2 = out(&mut ids, g(K::GRAB_UPDATE, 3, 9, 0, 3)).unwrap();
        assert_eq!((u2.id, u2.bone), (3, 3));
        // Client-only kinds: the game cannot send a denial; a grab needs a target.
        assert!(out(&mut ids, g(K::GRAB_DENIED, 2, 1, 0, 0)).is_none());
        assert!(out(&mut ids, g(K::GRAB_START, 0, 11, 0, 0)).is_none());
    }

    /// After the server denied our grab, a late grab_update already in the ring must not
    /// mint a new wire id (the server would treat it as a fresh grab).
    #[test]
    fn late_update_after_denial_does_not_revive_the_grab() {
        let mut ids = Ids::default();
        let s = out(&mut ids, g(K::GRAB_START, 2, 5, 0, 3)).unwrap();
        let mut d = Interact { kind: K::GRAB_DENIED, target_peer: 2, id: s.id, ..Default::default() };
        ids.inbound(0, &mut d);
        assert_eq!(d.gid, 5, "our Lua gid back");
        assert!(out(&mut ids, g(K::GRAB_UPDATE, 2, 5, 0, 3)).is_none());
        // A new grab with the same gid (Lua reload) is a fresh start.
        let s2 = out(&mut ids, g(K::GRAB_START, 2, 5, 0, 3)).unwrap();
        assert!(s2.id > s.id);
        assert!(out(&mut ids, g(K::GRAB_UPDATE, 2, 5, 0, 3)).is_some());
    }

    #[test]
    fn impulses_get_a_sequence_and_hand_zero() {
        let mut ids = Ids::default();
        let i = out(&mut ids, Interact { hand: 1, ..g(K::IMPULSE, 4, 0, 1, 1) }).unwrap();
        assert_eq!((i.kind, i.target_peer, i.bone, i.id, i.hand), (K::IMPULSE, 4, 1, 1, 0));
        assert_eq!(out(&mut ids, g(K::IMPULSE, 4, 0, 0, 1)).unwrap().id, 2);
        assert!(out(&mut ids, g(K::IMPULSE, 0, 0, 0, 1)).is_none(), "no target");
    }

    #[test]
    fn inbound_answers_carry_our_gid_and_close_the_grab() {
        let mut ids = Ids::default();
        let s = out(&mut ids, g(K::GRAB_START, 2, 41, 1, 3)).unwrap();
        let mut d = Interact { kind: K::GRAB_DENIED, gid: 999, ..s };
        ids.inbound(0, &mut d);
        assert_eq!((d.kind, d.gid, d.id), (K::GRAB_DENIED, 41, s.id));
        // ... and the mapping is gone afterwards (its end is not sent again).
        assert!(out(&mut ids, g(K::GRAB_END, 2, 41, 1, 0)).is_none());
        // An event from a peer about our body: its gid is meaningless here.
        let mut p = Interact { gid: 77, ..s };
        ids.inbound(3, &mut p);
        assert_eq!((p.gid, p.id), (0, s.id));
        // An unknown wire id from the server: gid 0.
        let mut u = Interact { kind: K::GRAB_END, id: 12345, gid: 5, ..s };
        ids.inbound(0, &mut u);
        assert_eq!(u.gid, 0);
    }

    #[test]
    fn outbound_messages_are_the_record_behind_the_header() {
        set_caps(hsmp_net::net::caps::INTERACT);
        let e = g(K::GRAB_START, 2, 100, 0, 5);
        let m = outbound_msg(hsmp_ipc::bytemuck::bytes_of(&e)).unwrap();
        let (h, p) = hsmp_ipc::wire::split(&m).unwrap();
        assert_eq!((h.kind, h.peer, p.len()), (K_INTERACT, 0, 48));
        let v = view::<Interact>(p).unwrap().head();
        assert!(v.id != 0 && v.gid == 100 && v.bone == 5 && v.point == e.point);
        // Grab updates take their hand's supersede stream.
        let m = outbound_msg(hsmp_ipc::bytemuck::bytes_of(&g(K::GRAB_UPDATE, 2, 100, 0, 5))).unwrap();
        assert_eq!(hsmp_ipc::wire::split(&m).unwrap().0.kind, K_INTERACT_GRAB_R);
        let m = outbound_msg(hsmp_ipc::bytemuck::bytes_of(&g(K::GRAB_UPDATE, 2, 101, 1, 5))).unwrap();
        assert_eq!(hsmp_ipc::wire::split(&m).unwrap().0.kind, K_INTERACT_GRAB_L);
        // Hostile payloads are dropped, never panic.
        assert!(outbound_msg(&[1, 2, 3]).is_none());
        let mut bad = hsmp_ipc::bytemuck::bytes_of(&e).to_vec();
        bad[2] = 200; // bone
        assert!(outbound_msg(&bad).is_none());
        assert!(inbound_record(0, &bad).is_err());
        assert!(inbound_record(2, hsmp_ipc::bytemuck::bytes_of(&e)).is_ok());
    }
}

