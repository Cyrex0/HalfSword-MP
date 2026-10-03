//! Interaction channel, server side (docs/development/subsystems/interact.md;
//! capability `caps::INTERACT`).
//!
//! Each player simulates only their own body; everyone else is a stand-in
//! servoed to its owner's pose. A grab or shove on a stand-in therefore never
//! reaches the real body: the opponent feels like infinite mass. This module
//! closes the loop: the initiator's client detects the contact on the
//! stand-in, the server validates it here, and the owner of the affected
//! body applies it to their real pawn (impulse, or a force-limited grab
//! handle anchored on the grabber's stand-in hand).
//!
//! Validation, cheap checks first:
//! 1. shape: known kind, bone < 16, hand ≤ 1, finite numbers, grip/contact
//!    point within `MAX_POINT` of the bone, grab id ≠ 0, not self;
//! 2. context: both sides negotiated INTERACT, initiator alive, target
//!    connected, match not frozen (countdown / paused);
//! 3. per-initiator rate limits (token buckets): impulses, grab starts,
//!    grab updates;
//! 4. geometry against the lag-comp history (`lagcomp::interact_distance`):
//!    the grabbing hand / the initiator's body must have been within reach of
//!    the claimed bone at the instant the initiator was displaying; without
//!    pose history the coarse root distance applies;
//! 5. impulse magnitude: clamped per event (`MAX_IMPULSE`) and against a
//!    per-TARGET budget (`RECV_BUDGET` per second, any number of initiators),
//!    so no combination of claims can launch a body.
//!
//! Grabs are leases: the grabber's sidecar refreshes them (GRAB_UPDATE,
//! ~10 Hz). A grab with no refresh for `GRAB_LEASE_MS`, or whose grabber or
//! target leaves, is ended by the server (GRAB_END to both sides).
//! Ordering (channel 1 is unordered): per hand, grab ids only grow; a START
//! or UPDATE with an id ≤ the last ended one is stale and dropped, an UPDATE
//! that overtakes its START opens the grab.
//!
//! Pure logic: the server glue (`server/interact_glue.rs`) supplies the
//! context, the geometry and the clock; the tests drive it directly.

#![allow(dead_code)]

use crate::proto::PeerId;
pub use hsmp_ipc::schema::interact::{kind as K, Interact};
use crate::validate::rate::Bucket;
use std::collections::HashMap;

#[cfg(test)]
mod tests;

// ---- tunables ----------------------------------------------------------------

/// Largest impulse one event may carry (kg·cm/s). Larger claims are clamped.
/// A hard shove moves an 80 kg Willie ~1.5 m/s: 12 000.
pub const MAX_IMPULSE: f32 = 15_000.0;
/// Impulse a single body may receive per second, all initiators together.
pub const RECV_BUDGET: f32 = 40_000.0;
/// Burst of that budget available at once.
pub const RECV_BURST: f32 = 20_000.0;
/// Grip / contact point offset from the bone origin (bone frame), uu.
pub const MAX_POINT: f32 = 60.0;
/// Grabbing hand → grabbed bone at the displayed instant (uu). A forearm is
/// ~25 uu long, so the hand on a bone's far end plus the pose/prediction
/// error (the same kind of slack as ROOT_TOL) stays well inside this.
pub const GRAB_REACH: f32 = 90.0;
/// Initiator body surface → contacted bone (uu).
pub const IMPULSE_REACH: f32 = 90.0;
/// Coarse root distance used without pose history (uu).
pub const ROOT_REACH: f32 = 400.0;
/// A grab without GRAB_UPDATE for this long is ended by the server.
pub const GRAB_LEASE_MS: i64 = 1000;
/// Rate limits per initiator: (burst, per second).
pub const IMPULSE_RATE: (f32, f32) = (10.0, 20.0);
pub const GRAB_START_RATE: (f32, f32) = (4.0, 4.0);
pub const GRAB_UPDATE_RATE: (f32, f32) = (20.0, 40.0);

// ---- inputs / outputs ------------------------------------------------------------

/// What the glue knows about the two peers right now.
#[derive(Debug, Clone, Copy, Default)]
pub struct Ctx {
    /// Initiator negotiated `caps::INTERACT`.
    pub initiator_caps: bool,
    pub initiator_alive: bool,
    /// Target is connected.
    pub target_present: bool,
    /// Target negotiated `caps::INTERACT` (else nothing can be delivered).
    pub target_caps: bool,
    /// Countdown / pause: bodies are frozen, nothing applies.
    pub frozen: bool,
    /// Last validated roots (initiator, target), for the no-history fallback.
    pub roots: Option<([f32; 3], [f32; 3])>,
}

/// Geometry answer for one event (from `lagcomp::interact_distance`).
pub use crate::lagcomp::Geo;

/// One message to send: the record `ev` to peer `to`, with `WireHdr::peer = from`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Route {
    pub to: PeerId,
    pub from: PeerId,
    pub ev: Interact,
}

/// Outcome of one C2S `interact` record.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Verdict {
    pub routes: Vec<Route>,
    /// Why (part of) the event was refused or altered (logged / counted).
    pub note: Option<String>,
}

impl Verdict {
    fn drop(why: impl Into<String>) -> Self { Verdict { routes: vec![], note: Some(why.into()) } }
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct Grab {
    id: u32,
    target: PeerId,
    bone: u8,
    started_ms: i64,
    last_ms: i64,
}

/// Magnitude budget (token bucket in kg·cm/s, partial grants).
#[derive(Debug, Clone, Copy)]
struct Budget {
    tokens: f32,
    at_ms: i64,
}

impl Budget {
    fn grant(&mut self, want: f32, now_ms: i64) -> f32 {
        let dt = (now_ms - self.at_ms).max(0) as f32 / 1000.0;
        self.tokens = (self.tokens + dt * RECV_BUDGET).min(RECV_BURST);
        self.at_ms = self.at_ms.max(now_ms);
        let g = want.min(self.tokens).max(0.0);
        self.tokens -= g;
        g
    }
}

#[derive(Debug, Clone)]
struct PeerIx {
    impulse: Bucket,
    grab_start: Bucket,
    grab_update: Bucket,
    /// Impulse this peer's body may still receive.
    recv: Budget,
    grabs: [Option<Grab>; 2],
    /// Highest grab id ended / denied / superseded, per hand.
    ended: [u32; 2],
}

impl PeerIx {
    fn new(now_ms: i64) -> Self {
        PeerIx {
            impulse: Bucket::full(IMPULSE_RATE.0, now_ms),
            grab_start: Bucket::full(GRAB_START_RATE.0, now_ms),
            grab_update: Bucket::full(GRAB_UPDATE_RATE.0, now_ms),
            recv: Budget { tokens: RECV_BURST, at_ms: now_ms },
            grabs: [None, None],
            ended: [0, 0],
        }
    }
}

/// Counters for logs / RCON.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Stats {
    pub impulses: u64,
    pub impulses_clamped: u64,
    pub grabs: u64,
    pub denied: u64,
    pub dropped: u64,
    pub expired: u64,
}

#[derive(Debug, Default)]
pub struct Interactions {
    peers: HashMap<PeerId, PeerIx>,
    pub stats: Stats,
}

fn finite3(v: &[f32; 3]) -> bool { v.iter().all(|x| x.is_finite()) }
fn len3(v: &[f32; 3]) -> f32 { (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt() }
fn dist3(a: &[f32; 3], b: &[f32; 3]) -> f32 { len3(&[a[0] - b[0], a[1] - b[1], a[2] - b[2]]) }

/// posecodec index of the grabbing hand (0 right, 1 left).
pub fn hand_bone(hand: u8) -> usize {
    if hand == 1 { crate::posecodec::HAND_L } else { crate::posecodec::HAND_R }
}

impl Interactions {
    fn peer(&mut self, id: PeerId, now_ms: i64) -> &mut PeerIx {
        self.peers.entry(id).or_insert_with(|| PeerIx::new(now_ms))
    }

    /// Validate one C2S `interact` record from `from`. `geo` answers the reach query
    /// (called at most once, only for events that need it).
    pub fn on_c2s(&mut self, from: PeerId, ev: Interact, ctx: &Ctx, geo: impl FnOnce(&Interact) -> Geo,
                  now_ms: i64) -> Verdict {
        let v = self.judge(from, ev, ctx, geo, now_ms);
        if v.routes.iter().any(|r| r.ev.kind == K::GRAB_DENIED) {
            self.stats.denied += 1;
        } else if v.routes.is_empty() {
            self.stats.dropped += 1;
        }
        v
    }

    fn judge(&mut self, from: PeerId, mut ev: Interact, ctx: &Ctx, geo: impl FnOnce(&Interact) -> Geo,
             now_ms: i64) -> Verdict {
        // 1. shape
        if !ctx.initiator_caps { return Verdict::drop("initiator did not negotiate INTERACT"); }
        if !matches!(ev.kind, K::GRAB_START | K::GRAB_UPDATE | K::GRAB_END | K::IMPULSE) {
            return Verdict::drop(format!("kind {} not allowed from a client", ev.kind));
        }
        if ev.target_peer == from || ev.target_peer == 0 { return Verdict::drop("bad target"); }
        if ev.hand > 1 || ev.bone as usize >= crate::posecodec::BONE_COUNT {
            return Verdict::drop("bad hand / bone");
        }
        if !finite3(&ev.point) || !finite3(&ev.vector) { return Verdict::drop("non-finite"); }
        if ev.kind != K::IMPULSE && ev.id == 0 { return Verdict::drop("grab id 0"); }
        if ev.kind != K::GRAB_END && len3(&ev.point) > MAX_POINT {
            return Verdict::drop(format!("point {:.0} uu off the bone", len3(&ev.point)));
        }
        let h = ev.hand as usize;

        // GRAB_END is always honoured for a grab we know (also when frozen /
        // dead): it only ever releases.
        if ev.kind == K::GRAB_END {
            let p = self.peer(from, now_ms);
            return match p.grabs[h] {
                Some(g) if g.id == ev.id => {
                    p.grabs[h] = None;
                    p.ended[h] = p.ended[h].max(ev.id);
                    ev.target_peer = g.target;
                    ev.bone = g.bone;
                    if ctx.target_caps && ctx.target_present {
                        Verdict { routes: vec![Route { to: g.target, from, ev }], note: None }
                    } else {
                        Verdict::drop("grab ended; target gone")
                    }
                }
                _ => {
                    // Its START may still be in flight: remember the end.
                    p.ended[h] = p.ended[h].max(ev.id);
                    Verdict::drop("end of an unknown grab")
                }
            };
        }

        // 2. context
        if !ctx.initiator_alive { return self.refuse(from, ev, "initiator not alive"); }
        if !ctx.target_present { return self.refuse(from, ev, "target not connected"); }
        if ctx.frozen { return self.refuse(from, ev, "match frozen"); }
        if !ctx.target_caps { return self.refuse(from, ev, "target cannot receive interactions"); }

        match ev.kind {
            K::IMPULSE => self.impulse(from, ev, ctx, geo, now_ms),
            _ => self.grab(from, ev, ctx, geo, now_ms),
        }
    }

    /// A refused START / UPDATE-as-start is denied to the grabber (its
    /// stand-in stops yielding); anything else is just dropped.
    fn refuse(&mut self, from: PeerId, ev: Interact, why: &str) -> Verdict {
        if ev.kind == K::GRAB_START || ev.kind == K::GRAB_UPDATE {
            let p = self.peers.entry(from).or_insert_with(|| PeerIx::new(0));
            let h = ev.hand as usize;
            let active = matches!(p.grabs[h], Some(g) if g.id == ev.id);
            if active {
                p.grabs[h] = None;
            }
            if ev.id > p.ended[h] || active {
                p.ended[h] = p.ended[h].max(ev.id);
                let mut v = Verdict::drop(why);
                v.routes.push(Route { to: from, from: 0, ev: Interact { kind: K::GRAB_DENIED, ..ev } });
                if active {
                    // The target is holding this grab: release it there too.
                    v.routes.push(Route { to: ev.target_peer, from, ev: Interact { kind: K::GRAB_END, ..ev } });
                }
                return v;
            }
        }
        Verdict::drop(why)
    }

    /// Reach check: geometry if available, else the coarse root distance.
    fn reach(ev: &Interact, ctx: &Ctx, geo: impl FnOnce(&Interact) -> Geo, limit: f32) -> Result<(), String> {
        match geo(ev) {
            Geo::Dist(d) if d > limit => Err(format!("out of reach ({:.0} uu > {:.0})", d, limit)),
            Geo::Dist(_) => Ok(()),
            Geo::Refused(why) => Err(why.to_string()),
            Geo::NoData => match ctx.roots {
                Some((a, t)) if dist3(&a, &t) > ROOT_REACH => {
                    Err(format!("roots {:.0} uu apart (> {:.0})", dist3(&a, &t), ROOT_REACH))
                }
                _ => Ok(()),
            },
        }
    }

    fn impulse(&mut self, from: PeerId, mut ev: Interact, ctx: &Ctx, geo: impl FnOnce(&Interact) -> Geo,
               now_ms: i64) -> Verdict {
        if !self.peer(from, now_ms).impulse.take(IMPULSE_RATE.0, IMPULSE_RATE.1, now_ms) {
            return Verdict::drop("impulse rate-limited");
        }
        if let Err(e) = Self::reach(&ev, ctx, geo, IMPULSE_REACH) {
            return Verdict::drop(format!("impulse {}", e));
        }
        let mag = len3(&ev.vector);
        if mag < 1.0 { return Verdict::drop("empty impulse"); }
        let mut note = None;
        let mut want = mag;
        if mag > MAX_IMPULSE {
            want = MAX_IMPULSE;
            note = Some(format!("impulse {:.0} clamped to {:.0}", mag, MAX_IMPULSE));
            self.stats.impulses_clamped += 1;
        }
        let got = self.peer(ev.target_peer, now_ms).recv.grant(want, now_ms);
        if got < 1.0 { return Verdict::drop("target impulse budget exhausted"); }
        if got < want - 0.5 {
            note = Some(format!("impulse {:.0} cut to {:.0} by the target budget", mag, got));
            self.stats.impulses_clamped += 1;
        }
        let k = got / mag;
        ev.vector = [ev.vector[0] * k, ev.vector[1] * k, ev.vector[2] * k];
        self.stats.impulses += 1;
        Verdict { routes: vec![Route { to: ev.target_peer, from, ev }], note }
    }

    fn grab(&mut self, from: PeerId, ev: Interact, ctx: &Ctx, geo: impl FnOnce(&Interact) -> Geo,
            now_ms: i64) -> Verdict {
        let h = ev.hand as usize;
        let cur = self.peer(from, now_ms).grabs[h];
        let ended = self.peer(from, now_ms).ended[h];

        // Refresh of the active grab: lease + forward (rate-limited, no new
        // geometry check: the victim may be struggling away).
        if let Some(g) = cur {
            if g.id == ev.id {
                if ev.kind == K::GRAB_START { return Verdict::drop("duplicate grab start"); }
                if ev.target_peer != g.target { return Verdict::drop("grab update for another target"); }
                let p = self.peer(from, now_ms);
                p.grabs[h] = Some(Grab { last_ms: now_ms, bone: ev.bone, ..g });
                if !p.grab_update.take(GRAB_UPDATE_RATE.0, GRAB_UPDATE_RATE.1, now_ms) {
                    return Verdict::drop("grab update rate-limited (lease kept)");
                }
                return Verdict { routes: vec![Route { to: g.target, from, ev }], note: None };
            }
        }
        if ev.id <= ended || cur.map_or(false, |g| ev.id < g.id) { return Verdict::drop("stale grab id"); }

        // A new grab (START, or an UPDATE that overtook its START).
        if !self.peer(from, now_ms).grab_start.take(GRAB_START_RATE.0, GRAB_START_RATE.1, now_ms) {
            return self.refuse(from, ev, "grab rate-limited");
        }
        if let Err(e) = Self::reach(&ev, ctx, geo, GRAB_REACH) {
            return self.refuse(from, ev, &format!("grab {}", e));
        }
        let mut routes = Vec::new();
        let p = self.peer(from, now_ms);
        if let Some(old) = cur {
            // The same hand grabbed something else: the old grab is over.
            p.ended[h] = p.ended[h].max(old.id);
            routes.push(Route {
                to: old.target, from,
                ev: Interact { kind: K::GRAB_END, target_peer: old.target, id: old.id, bone: old.bone, ..ev },
            });
        }
        // Every older id of this hand is stale from now on.
        p.ended[h] = p.ended[h].max(ev.id - 1);
        p.grabs[h] = Some(Grab { id: ev.id, target: ev.target_peer, bone: ev.bone, started_ms: now_ms, last_ms: now_ms });
        self.stats.grabs += 1;
        routes.push(Route { to: ev.target_peer, from, ev: Interact { kind: K::GRAB_START, ..ev } });
        Verdict { routes, note: None }
    }

    /// Grabs whose lease ran out: GRAB_END to the target (from the grabber)
    /// and to the grabber (from the server, 0).
    pub fn expire(&mut self, now_ms: i64) -> Vec<Route> {
        let mut out = Vec::new();
        for (&id, p) in self.peers.iter_mut() {
            for h in 0..2 {
                if let Some(g) = p.grabs[h] {
                    if now_ms - g.last_ms > GRAB_LEASE_MS {
                        p.grabs[h] = None;
                        p.ended[h] = p.ended[h].max(g.id);
                        let ev = end_ev(g, h as u8);
                        out.push(Route { to: g.target, from: id, ev });
                        out.push(Route { to: id, from: 0, ev });
                        self.stats.expired += 1;
                    }
                }
            }
        }
        out
    }

    /// Peer `id` left: its grabs end on their targets; grabs on its body end
    /// on their grabbers.
    pub fn forget(&mut self, id: PeerId) -> Vec<Route> {
        let mut out = Vec::new();
        if let Some(p) = self.peers.remove(&id) {
            for (h, g) in p.grabs.iter().enumerate() {
                if let Some(g) = g {
                    out.push(Route { to: g.target, from: id, ev: end_ev(*g, h as u8) });
                }
            }
        }
        for (&gid, p) in self.peers.iter_mut() {
            for h in 0..2 {
                if let Some(g) = p.grabs[h] {
                    if g.target == id {
                        p.grabs[h] = None;
                        p.ended[h] = p.ended[h].max(g.id);
                        out.push(Route { to: gid, from: 0, ev: end_ev(g, h as u8) });
                    }
                }
            }
        }
        out
    }

    /// Active grabs as (grabber, hand, target) (diagnostics, tests).
    pub fn active(&self) -> Vec<(PeerId, u8, PeerId)> {
        let mut v: Vec<_> = self.peers.iter()
            .flat_map(|(&id, p)| p.grabs.iter().enumerate().filter_map(move |(h, g)| g.map(|g| (id, h as u8, g.target))))
            .collect();
        v.sort_unstable();
        v
    }
}

fn end_ev(g: Grab, hand: u8) -> Interact {
    Interact { kind: K::GRAB_END, target_peer: g.target, id: g.id, hand, bone: g.bone, ..Default::default() }
}

// ---- global instance (server) ------------------------------------------------------

fn global() -> &'static std::sync::Mutex<Interactions> {
    static S: std::sync::OnceLock<std::sync::Mutex<Interactions>> = std::sync::OnceLock::new();
    S.get_or_init(|| std::sync::Mutex::new(Interactions::default()))
}

pub fn with<R>(f: impl FnOnce(&mut Interactions) -> R) -> R {
    f(&mut global().lock().unwrap_or_else(|e| e.into_inner()))
}

/// Negotiated capabilities per connected peer id (set at admission).
fn caps_map() -> &'static std::sync::Mutex<HashMap<PeerId, u64>> {
    static S: std::sync::OnceLock<std::sync::Mutex<HashMap<PeerId, u64>>> = std::sync::OnceLock::new();
    S.get_or_init(|| std::sync::Mutex::new(HashMap::new()))
}

pub fn note_caps(id: PeerId, caps: u64) {
    caps_map().lock().unwrap_or_else(|e| e.into_inner()).insert(id, caps);
}

/// The negotiated capability bits of a connected peer (0 = unknown).
pub fn peer_caps(id: PeerId) -> u64 {
    caps_map().lock().unwrap_or_else(|e| e.into_inner()).get(&id).copied().unwrap_or(0)
}

pub fn has_interact(id: PeerId) -> bool {
    caps_map().lock().unwrap_or_else(|e| e.into_inner()).get(&id)
        .map_or(false, |c| c & hsmp_net::net::caps::INTERACT != 0)
}

/// Peer left: drop its caps and end every grab involving it.
pub fn on_leave(id: PeerId) -> Vec<Route> {
    caps_map().lock().unwrap_or_else(|e| e.into_inner()).remove(&id);
    with(|ix| ix.forget(id))
}

/// Server tick: forget peers that are no longer connected (left, timed out,
/// replaced), then expire grab leases. Returns the GRAB_ENDs to send.
pub fn tick(present: &[PeerId], now_ms: i64) -> Vec<Route> {
    let gone: Vec<PeerId> = {
        let mut caps = caps_map().lock().unwrap_or_else(|e| e.into_inner());
        let mut gone: Vec<PeerId> = caps.keys().copied().filter(|id| !present.contains(id)).collect();
        caps.retain(|id, _| present.contains(id));
        gone.extend(with(|ix| ix.peers.keys().copied().filter(|id| !present.contains(id)).collect::<Vec<_>>()));
        gone.sort_unstable();
        gone.dedup();
        gone
    };
    let mut out = Vec::new();
    for id in gone {
        out.extend(with(|ix| ix.forget(id)));
    }
    out.extend(with(|ix| ix.expire(now_ms)));
    // Never address a peer that is gone.
    out.retain(|r| present.contains(&r.to));
    out
}
