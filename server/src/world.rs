//! Server-side world-object replication (HSMPWorld v2). Pure logic, no I/O:
//! the async glue in server.rs feeds messages in and sends what comes out.
//! Design: docs/development/subsystems/world-replication.md.
//!
//! * Scope: every record lives in a (level, epoch) bucket. `level` = FNV-1a
//!   of the UWorld path, `epoch` = global counter bumped when every arena is
//!   reloaded (round countdown, back to lobby). A bump drops all buckets, so
//!   a round starts from the map's initial state on every client.
//! * Manifest: the canonical list of replicable bodies.
//!   - Known arenas: the SERVER builds the manifest
//!     from the statically extracted level data (`docs/arena_static` ->
//!     `world_static_table.rs`, `StaticLevel::build`) with the same id
//!     formula the clients use, so a level bucket starts out seeded. Client
//!     proposals are then accepted only as ALIASES: folded into the nearest
//!     server entry with the same class+component hash within ALIAS_R, or,
//!     for bodies whose position isn't known statically (multi-body props,
//!     chest loot), accepted only near a static object of the arena
//!     (VOUCH_R). Everything else is rejected (and still acked).
//!   - Other levels (custom maps, hub): clients submit their deterministic
//!     ids; the first submission wins and a near-duplicate (same hash within
//!     MATCH_R) is folded into the existing id.
//!   - Dynamic items (dropped loadout weapons) use an id namespace tied to
//!     the dropping peer.
//! * Ownership: a body is FREE (server keeps its rest pose) or leased to one
//!   peer that touches/holds it. Leases are renewed by the owner's state
//!   stream and expire after LEASE or on disconnect. Hold beats touch; a
//!   holder can't be robbed; first claim processed wins otherwise.
//! * States: accepted only from the lease holder (a state for a FREE body is
//!   an implicit touch claim), sanity-checked (speed, hold radius), coalesced
//!   per recipient, relevance-thinned by distance and capped by a per-
//!   recipient byte budget. Rest poses are keyframed round-robin to heal loss.
//! * Same world on every screen:
//!   - Initial state: the first peer that proposes settled rest poses
//!     (`CLAIM_INIT`) for a (level, epoch) becomes its INIT AUTHORITY; its
//!     poses become the anchors every client forces before it reports Ready.
//!     One client's settled world, never a mix of two.
//!   - Actor state records (`CLAIM_STATE`): per state-bearing actor (planks,
//!     barrels, fences, levers, gates, traps, chests ...) a 7-bit word:
//!     bits 0-5 = constraint k of the group broken (sticky for the epoch),
//!     bit 6 = the kind's flag (lever Loaded, trap activated, lid open).
//!     The server orders and persists them; they travel as owner records
//!     with `mode = MODE_STATE | bits`, so sync/heal/late join get them.
//!   - Consistency: clients report quantised per-body states (a `world_hash`
//!     record); the server diffs the latest reports of two peers, answers each
//!     reporter with a `world_verdict`, re-sends the anchors of mismatched free
//!     bodies and logs `world_consistency`.
//!
//! Wire (protocol v6, `hsmp_ipc::schema::world`): every message is a typed record
//! the client's game wrote; handlers get borrowed, validated rows and the fan-out
//! is built from the same rows ([`Msg::encode`]).

use crate::proto::PeerId;
use hsmp_ipc::schema::world as rec;
pub use hsmp_ipc::schema::world::{
    OwnerRec, WorldObj, WorldSnap, CLAIM_INIT, CLAIM_STATE, HS_ALIVE, HS_SETTLED, HS_STATE, MM_POSE, MM_PRESENCE,
    MM_STATE, MODE_FREE, MODE_HOLD_L, MODE_HOLD_R, MODE_STATE, MODE_TOUCH, WF_ASLEEP, WF_HELD, WF_LEFT,
};
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};

/// Bits 0-5: constraint broken (sticky). Bit 6: the kind's flag (toggles).
pub const ST_BREAK_MASK: u8 = 0x3F;
pub const ST_FLAG: u8 = 0x40;
/// Consistency tolerances: a free body at rest may differ this much between
/// two clients before it counts as a mismatch (anchors are forced exactly).
pub const POS_TOL: f32 = 5.0;
pub const ROT_TOL: f32 = 3.0;
/// Reports older than this are not compared.
pub const HASH_MAX_AGE: Duration = Duration::from_secs(12);
/// Mismatch rows per verdict (the head carries the full count).
pub const VERDICT_MAX: usize = 24;
const _: () = assert!(VERDICT_MAX <= rec::VERDICT_MAX);

pub const LEASE: Duration = Duration::from_millis(3000);
/// Same class+component within this radius (cm) = the same body.
pub const MATCH_R: f32 = 60.0;
/// Server-built levels: a client body with the same class+component hash
/// within this radius of a canonical entry is that entry (HSMPWorld binds
/// with the same radius, FUZZY_R in main.lua).
pub const ALIAS_R: f32 = 120.0;
/// Server-built levels: a client proposal whose position isn't known
/// statically must lie this close to a static object of the arena.
pub const VOUCH_R: f32 = 500.0;
/// Server-built levels: cap on accepted client-only (vouched) entries.
pub const MAX_CLIENT_STATIC: usize = 512;
pub const MAX_STATIC: usize = 1024;
pub const MAX_DYN_PER_PEER: usize = 32;
pub const MAX_OBJS_PER_STATE: usize = 40;
/// Objects may not move faster than this between accepted states (cm/s),
/// plus a fixed slack for jitter. Thrown weapons stay well below 60 m/s.
const MAX_SPEED: f32 = 6000.0;
const SPEED_SLACK: f32 = 300.0;
/// A held body must stay within this distance of its holder's root (cm).
const HOLD_RADIUS: f32 = 600.0;
/// Touch hand-over: a touch claim takes another peer's touch lease when the claimer's root is
/// within TAKE_R of the body and at least TAKE_MARGIN nearer than the owner's, and the lease is
/// at least TAKE_MIN_AGE old (no flip-flopping between two pushers).
pub const TAKE_R: f32 = 200.0;
pub const TAKE_MARGIN: f32 = 60.0;
pub const TAKE_MIN_AGE: Duration = Duration::from_millis(400);
/// ... and only a body slower than this (cm/s): a body flying or sliding fast stays with its owner
/// (the new owner would start it from its own, older view of it).
pub const TAKE_SPEED: f32 = 150.0;
/// Positions outside ±this (cm) are garbage.
const WORLD_BOUND: f32 = 2.0e6;

/// One state "packet" per recipient: the per-sender `world_state` messages sent together
/// (keeps one datagram < ~1100 B on the wire).
pub const MAX_BODY_BYTES: usize = 1000;
/// One `WorldObj` row.
const OBJ_BYTES: usize = core::mem::size_of::<WorldObj>();
/// One per-sender `world_state` message around its rows: 8 B wire header + 24 B head +
/// 7 B unreliable chunk header.
const GROUP_BYTES: usize = 8 + core::mem::size_of::<rec::WorldStateHead>() + 7;
/// Owner records re-sent per heal (a big table rotates through).
pub const OWNERS_PER_HEAL: usize = 140;

/// Per-recipient world downstream budget (bytes/s) and burst.
pub const BUDGET_BPS: f64 = 16.0 * 1024.0;
const BUDGET_BURST: f64 = 8.0 * 1024.0;
/// Bytes a sealed datagram adds around the body (see relay::SEAL_OVERHEAD + token).
const WIRE_OVERHEAD: usize = 16 + 4 + 16 + 12 + 8 + 16;

const FAR1: f32 = 3000.0;
const FAR2: f32 = 6000.0;
pub const HEAL_EVERY: Duration = Duration::from_millis(2000);
pub const KEYFRAME_EVERY: Duration = Duration::from_millis(250);
const KEYFRAMES_PER_TICK: usize = 6;
const CLAIM_RATE: f64 = 20.0;
const CLAIM_BURST: f64 = 40.0;
const HINT_EVERY: Duration = Duration::from_millis(1000);
/// Most levels held at once (level ids are client-chosen).
pub const MAX_LEVELS: usize = 32;
/// C2SWorldSync per peer: burst / refill per second.
const SYNC_BURST: f64 = 6.0;
const SYNC_RATE: f64 = 1.0;

pub fn dist(a: [f32; 3], b: [f32; 3]) -> f32 {
    ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2) + (a[2] - b[2]).powi(2)).sqrt()
}

/// Manifest entry as the server keeps it: a replicable body as discovered at level load
/// (static scene object; wire row `ManifestEntry`) or as dropped by its owner (dynamic
/// item; wire row `DynEntry`, the only one that carries a class path).
#[derive(Debug, Clone, PartialEq)]
pub struct WorldEntry {
    pub id: u32,
    /// FNV-1a of "<class short name>|<component name>".
    pub chash: u32,
    /// Spawn position (static) / drop position (dynamic), cm.
    pub pos: [f32; 3],
    /// 0 = static scene object; else the peer that dropped this item.
    pub dyn_owner: PeerId,
    /// Dynamic items only: class path to spawn ("/Game/...X.X_C"), ≤ 200 B.
    pub class: String,
    pub passport: Option<hsmp_ipc::schema::loadout::WeaponPass>,
}

impl WorldEntry {
    pub fn from_static(r: &rec::ManifestEntry) -> WorldEntry {
        WorldEntry { id: r.id, chash: r.chash, pos: r.pos, dyn_owner: 0, class: String::new(), passport: None }
    }
    pub fn from_dyn(r: &rec::DynEntry) -> WorldEntry {
        WorldEntry { id: r.id, chash: r.chash, pos: r.pos, dyn_owner: r.dyn_owner, class: r.class_path.lossy().into_owned(),
            passport: r.has_passport.get().then_some(r.passport) }
    }
    fn is_dyn(&self) -> bool {
        self.dyn_owner != 0 || self.id & rec::DYN_ID_BIT != 0
    }
    fn static_row(&self) -> rec::ManifestEntry {
        rec::ManifestEntry { id: self.id, chash: self.chash, pos: self.pos, _r: 0 }
    }
    fn dyn_row(&self) -> rec::DynEntry {
        rec::DynEntry { id: self.id, chash: self.chash, pos: self.pos, dyn_owner: self.dyn_owner,
                        class_path: hsmp_ipc::layout::Str::new(&self.class),
                        has_passport: hsmp_ipc::layout::Bool::from(self.passport.is_some()), _r: [0; 7],
                        passport: self.passport.unwrap_or_default() }
    }
}

/// Dynamic-item id namespace: high bit + low 15 bits of the dropping peer.
pub fn dyn_id_ok(id: u32, peer: PeerId) -> bool {
    id & 0x8000_0000 != 0 && (id >> 16) & 0x7FFF == peer & 0x7FFF
}

/// A dynamic item's class must be a Blueprint weapon class path, never a
/// pawn (receivers spawn it), and it can't be at the world origin (that is
/// a destroyed / never-finished actor, not a drop).
pub fn dyn_entry_ok(e: &WorldEntry) -> bool {
    e.class.len() <= 200 && e.class.starts_with("/Game/") && e.class.ends_with("_C")
        && !e.class.to_ascii_lowercase().contains("willie")
        && e.class.chars().all(|c| c.is_ascii_graphic())
        && !at_origin(e.pos)
}

// ---- server-built manifests ------------------------------------------------

/// One arena of the generated static table.
pub struct StaticArena {
    /// UWorld full name exactly as HSMPWorld hashes it ("World /Game/...").
    pub world: &'static str,
    pub objs: &'static [StaticObj],
}

/// One statically known world object.
pub struct StaticObj {
    pub class: &'static str,
    /// "SM" / "W" for root bodies (entry = true); "" for vouch-only objects.
    pub comp: &'static str,
    /// FName in its level.
    pub name: &'static str,
    /// Outer level path ("/Game/...:PersistentLevel"); "" if unknown.
    pub level: &'static str,
    pub pos: [f32; 3],
    /// Canonical manifest entry (body position known), else vouch point only.
    pub entry: bool,
}

#[allow(clippy::all)]
mod static_table {
    use super::{StaticArena, StaticObj};
    include!("world_static_table.rs");
}
pub use static_table::STATIC_ARENAS;

/// FNV-1a (32 bit), identical to HSMPWorld's `fnv1a`.
pub fn fnv1a(s: &str) -> u32 {
    let mut h: u32 = 2166136261;
    for b in s.bytes() {
        h ^= b as u32;
        h = h.wrapping_mul(16777619);
    }
    h
}

/// Lua `stable_name`: level-placed FNames are the same on every client;
/// runtime spawns carry 2147xxxxxx-style suffixes.
pub fn stable_name(name: &str) -> bool {
    if name.is_empty() || name.starts_with("Default__") { return false; }
    let digits: String = name.rsplit('_').next().unwrap_or("").to_string();
    if !name.contains('_') || digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return true;
    }
    digits.parse::<u64>().map_or(false, |n| n < 1_000_000_000)
}

/// Within 1 cm of the world origin on every axis: a destroyed / never-built
/// actor's pose (HSMPWorld `at_origin`), never a real body.
pub fn at_origin(p: [f32; 3]) -> bool { p.iter().all(|v| v.abs() < 1.0) }

/// Arena box margins around the static objects (same as HSMPWorld).
const BOUNDS_MARGIN_XY: f32 = 3000.0;
const BOUNDS_MARGIN_DOWN: f32 = 3000.0;
const BOUNDS_MARGIN_UP: f32 = 5000.0;

/// Lua `cell` (50 cm spawn cells).
fn cell(v: f32) -> i64 { (v as f64 / 50.0 + 0.5).floor() as i64 }

/// Lua `%.0f` of a coordinate (tie-break key).
fn tie(p: [f32; 3]) -> String {
    format!("{:.0},{:.0},{:.0}", p[0] as f64, p[1] as f64, p[2] as f64)
}

/// The server-built manifest of one level.
#[derive(Default, Debug)]
pub struct StaticLevel {
    #[allow(dead_code)]
    pub world: String,
    pub entries: Vec<WorldEntry>,
    /// Every static object's position (entries included).
    pub vouch: Vec<[f32; 3]>,
    /// Arena box (min, max) with margins; None if too few static objects.
    pub bounds: Option<([f32; 3], [f32; 3])>,
}

impl StaticLevel {
    /// Canonical entries with HSMPWorld's id rule (`assign_ids` in main.lua):
    /// key = class|comp|(levelpath:FName or @cell), sorted by (key, rounded
    /// position), id = FNV(key) & 0x7FFFFFFF, collisions -> FNV(key#n).
    pub fn build(world: &str, objs: &[StaticObj]) -> StaticLevel {
        let mut c: Vec<(String, String, &StaticObj)> = objs.iter().filter(|o| o.entry && !o.comp.is_empty())
            .map(|o| {
                let wh = if stable_name(o.name) {
                    format!("{}:{}", o.level, o.name)
                } else {
                    format!("@{},{},{}", cell(o.pos[0]), cell(o.pos[1]), cell(o.pos[2]))
                };
                (format!("{}|{}|{}", o.class, o.comp, wh), tie(o.pos), o)
            }).collect();
        c.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| a.1.cmp(&b.1)));
        let mut taken = HashSet::new();
        let mut entries = Vec::with_capacity(c.len());
        for (key, _, o) in c {
            let mut id = fnv1a(&key) & 0x7FFF_FFFF;
            let mut n = 1;
            while taken.contains(&id) || id == 0 {
                n += 1;
                id = fnv1a(&format!("{key}#{n}")) & 0x7FFF_FFFF;
            }
            taken.insert(id);
            entries.push(WorldEntry {
                id, chash: fnv1a(&format!("{}|{}", o.class, o.comp)), pos: o.pos, dyn_owner: 0, class: String::new(), passport: None,
            });
        }
        let vouch: Vec<[f32; 3]> = objs.iter().map(|o| o.pos).filter(|p| !at_origin(*p)).collect();
        let bounds = (vouch.len() >= 3).then(|| {
            let mut lo = [f32::MAX; 3];
            let mut hi = [f32::MIN; 3];
            for p in &vouch {
                for k in 0..3 { lo[k] = lo[k].min(p[k]); hi[k] = hi[k].max(p[k]); }
            }
            ([lo[0] - BOUNDS_MARGIN_XY, lo[1] - BOUNDS_MARGIN_XY, lo[2] - BOUNDS_MARGIN_DOWN],
             [hi[0] + BOUNDS_MARGIN_XY, hi[1] + BOUNDS_MARGIN_XY, hi[2] + BOUNDS_MARGIN_UP])
        });
        StaticLevel { world: world.to_string(), entries, vouch, bounds }
    }

    /// Inside the arena (static objects' box plus margins), when known.
    pub fn contains(&self, p: [f32; 3]) -> bool {
        self.bounds.map_or(true, |(lo, hi)| (0..3).all(|k| p[k] >= lo[k] && p[k] <= hi[k]))
    }

    pub fn vouches(&self, pos: [f32; 3]) -> bool {
        self.vouch.iter().any(|v| dist(*v, pos) < VOUCH_R)
    }
}

pub type Statics = Arc<HashMap<u32, Arc<StaticLevel>>>;

/// Build a level-hash -> manifest map (level hash = FNV-1a of the UWorld
/// full name, as HSMPWorld computes it).
pub fn build_statics(arenas: &[StaticArena]) -> Statics {
    Arc::new(arenas.iter()
        .map(|a| (fnv1a(a.world), Arc::new(StaticLevel::build(a.world, a.objs))))
        .collect())
}

/// The built-in arenas (generated table), built once.
pub fn builtin_statics() -> Statics {
    static S: OnceLock<Statics> = OnceLock::new();
    S.get_or_init(|| build_statics(STATIC_ARENAS)).clone()
}

// ---- consistency codec --------------------------------------------------------
// Bit-identical to HSMPWorld's Lua (`pack_quat`, `body_hash`, `world_hash`);
// both suites check the same golden vectors.

/// Lua `math.floor(v + 0.5)` (also used for the quaternion components so
/// Rust and Lua round ties the same way).
fn round_half_up(v: f64) -> f64 { (v + 0.5).floor() }

/// Smallest-three quaternion in 32 bits: bits 30-31 = index of the dropped
/// (largest) component, made positive; the other three in 10 bits each,
/// scaled from [-1/√2, 1/√2], first remaining component in bits 20-29.
#[allow(dead_code)] // HSMPWorld packs (Lua); the golden-vector tests mirror it here
pub fn pack_quat(q: [f32; 4]) -> u32 {
    let mut d = q.map(|v| v as f64);
    let n = d.iter().map(|v| v * v).sum::<f64>().sqrt();
    if !(n > 1e-9 && n.is_finite()) {
        d = [0.0, 0.0, 0.0, 1.0];
    } else {
        for v in d.iter_mut() { *v /= n; }
    }
    let mut big = 0;
    for i in 1..4 {
        if d[i].abs() > d[big].abs() { big = i; }
    }
    let s = if d[big] < 0.0 { -1.0 } else { 1.0 };
    let r = std::f64::consts::FRAC_1_SQRT_2;
    let mut out = (big as u32) << 30;
    let mut k = 0;
    for (i, v) in d.iter().enumerate() {
        if i == big { continue; }
        let x = (s * v).clamp(-r, r);
        let u = round_half_up((x / r + 1.0) * 511.5).clamp(0.0, 1023.0) as u32;
        out |= u << (20 - 10 * k);
        k += 1;
    }
    out
}

pub fn unpack_quat(p: u32) -> [f32; 4] {
    let big = (p >> 30) as usize;
    let r = std::f64::consts::FRAC_1_SQRT_2;
    let mut q = [0f64; 4];
    let mut k = 0;
    let mut ss = 0.0;
    for (i, slot) in q.iter_mut().enumerate() {
        if i == big { continue; }
        let u = ((p >> (20 - 10 * k)) & 1023) as f64;
        *slot = (u / 511.5 - 1.0) * r;
        ss += *slot * *slot;
        k += 1;
    }
    q[big] = (1.0 - ss).max(0.0).sqrt();
    q.map(|v| v as f32)
}

/// Angle between two orientations, degrees (double cover handled).
pub fn quat_angle_deg(a: [f32; 4], b: [f32; 4]) -> f32 {
    let dot: f64 = (0..4).map(|i| a[i] as f64 * b[i] as f64).sum::<f64>().abs().min(1.0);
    (2.0 * dot.acos()).to_degrees() as f32
}

fn cm_i32(v: f32) -> i32 { round_half_up(v as f64).clamp(i32::MIN as f64, i32::MAX as f64) as i32 }

/// One body's (or actor state's) quantised state hash: FNV-1a over
/// `<u32 id><i32 x><i32 y><i32 z><u32 q><u8 status>` little-endian, the
/// position rounded to whole cm, `q` = packed quaternion (bodies) or state
/// bits (actor states).
pub fn body_hash(id: u32, pos: [f32; 3], q: u32, status: u8) -> u32 {
    let mut b = Vec::with_capacity(21);
    b.extend_from_slice(&id.to_le_bytes());
    for v in pos { b.extend_from_slice(&cm_i32(v).to_le_bytes()); }
    b.extend_from_slice(&q.to_le_bytes());
    b.push(status);
    fnv1a_bytes(&b)
}

fn fnv1a_bytes(b: &[u8]) -> u32 {
    let mut h: u32 = 2166136261;
    for x in b {
        h ^= *x as u32;
        h = h.wrapping_mul(16777619);
    }
    h
}

/// World hash: FNV-1a over `<u32 id><u32 body_hash>` of every entry, in
/// ascending id order (the caller passes them sorted).
pub fn world_hash(bodies: &[HashBody]) -> u32 {
    let mut b = Vec::with_capacity(bodies.len() * 8);
    for e in bodies {
        b.extend_from_slice(&e.id.to_le_bytes());
        b.extend_from_slice(&body_hash(e.id, e.pos, e.q, e.status).to_le_bytes());
    }
    fnv1a_bytes(&b)
}

/// One entry of a client's consistency report.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct HashBody {
    pub id: u32,
    /// Owner/state record version the client knows (only equal versions compare).
    pub ver: u32,
    /// HS_* bits.
    pub status: u8,
    pub pos: [f32; 3],
    /// Packed quaternion (bodies) or state bits (HS_STATE).
    pub q: u32,
}

impl HashBody {
    fn comparable(&self) -> bool { self.status & (HS_ALIVE | HS_SETTLED) == HS_ALIVE | HS_SETTLED }
    fn missing(&self) -> bool { self.status & HS_ALIVE == 0 }
    /// One `world_hash` row.
    pub fn from_row(r: &rec::HashRow) -> HashBody {
        HashBody { id: r.id, ver: r.ver, status: r.status, pos: r.pos, q: r.q }
    }
    #[cfg(test)]
    pub fn row(&self) -> rec::HashRow {
        rec::HashRow { id: self.id, ver: self.ver, pos: self.pos, q: self.q, status: self.status, _r: [0; 3], _r2: 0 }
    }
}

/// One mismatched body of a verdict (the wire row).
pub type Mismatch = rec::Mismatch;

fn mismatch(id: u32, kind: u8, dpos: f32, dang: f32) -> Mismatch {
    Mismatch { id, kind, _r: [0; 3], dpos, dang }
}

/// Diff of two reports over the bodies both can judge (same record version,
/// at rest on both, or at rest on one and missing on the other).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Compared {
    pub compared: u32,
    /// World hash of each side over the compared set.
    pub hash_a: u32,
    pub hash_b: u32,
    pub mismatched: Vec<Mismatch>,
}

/// `a`/`b` sorted by id (HashReport keeps them sorted).
pub fn compare_reports(a: &[HashBody], b: &[HashBody]) -> Compared {
    let mut out = Compared::default();
    let (mut ha, mut hb) = (Vec::new(), Vec::new());
    let mut j = 0;
    for ea in a {
        while j < b.len() && b[j].id < ea.id { j += 1; }
        let Some(eb) = b.get(j).filter(|eb| eb.id == ea.id) else { continue };
        if ea.ver != eb.ver || (ea.status ^ eb.status) & HS_STATE != 0 { continue; }
        let mm = if ea.comparable() && eb.comparable() {
            if ea.status & HS_STATE != 0 {
                (ea.q != eb.q).then_some(mismatch(ea.id, MM_STATE, 0.0, 0.0))
            } else {
                let dpos = dist(ea.pos, eb.pos);
                let dang = quat_angle_deg(unpack_quat(ea.q), unpack_quat(eb.q));
                (dpos > POS_TOL || dang > ROT_TOL).then_some(mismatch(ea.id, MM_POSE, dpos, dang))
            }
        } else if (ea.comparable() && eb.missing()) || (eb.comparable() && ea.missing()) {
            Some(mismatch(ea.id, MM_PRESENCE, 0.0, 0.0))
        } else {
            continue; // moving / owned on one side, or missing on both
        };
        out.compared += 1;
        ha.push(*ea);
        hb.push(*eb);
        if let Some(m) = mm { out.mismatched.push(m); }
    }
    out.hash_a = world_hash(&ha);
    out.hash_b = world_hash(&hb);
    out
}

/// A peer's latest complete consistency report.
#[derive(Debug, Clone)]
#[allow(dead_code)] // seq / client_hash: diagnostics
pub struct HashReport {
    pub seq: u32,
    pub t: Instant,
    pub client_hash: u32,
    pub bodies: Vec<HashBody>,
}

/// Outcome of one comparison (the glue logs it and emits `world_consistency`).
#[derive(Debug, Clone, PartialEq)]
pub struct Verdict {
    pub level: u32,
    pub epoch: u32,
    pub peer: PeerId,
    pub other: PeerId,
    pub seq: u32,
    pub compared: u32,
    /// Exact quantised hash equality over the compared set.
    pub hash_equal: bool,
    /// The reporter's own hash matches the server's recomputation (codec check).
    pub codec_ok: bool,
    pub mismatched: Vec<Mismatch>,
}

impl Verdict {
    /// Same world (within tolerance) for every body both peers can judge.
    pub fn hash_match(&self) -> bool { self.mismatched.is_empty() }

    /// The `world_verdict` record: the counts in the head, up to VERDICT_MAX mismatch rows.
    pub fn record(&self) -> (rec::VerdictHead, Vec<Mismatch>) {
        let h = rec::VerdictHead {
            level: self.level, epoch: self.epoch, seq: self.seq, other: self.other, compared: self.compared,
            mismatched_n: self.mismatched.len() as u32, hash_match: self.hash_match().into(),
            hash_equal: self.hash_equal.into(), n: 0, _r: 0,
        };
        (h, self.mismatched.iter().take(VERDICT_MAX).copied().collect())
    }
}

/// Server-ordered state of one state-bearing actor (group).
#[derive(Debug, Clone, Copy)]
pub struct StateRec {
    pub bits: u8,
    pub by: PeerId,
    pub ver: u32,
}

/// Merge a client's reported bits into the record: breaks are sticky, the
/// flag takes the reported value.
pub fn merge_state(old: u8, reported: u8) -> u8 {
    (old & ST_BREAK_MASK) | (reported & ST_BREAK_MASK) | (reported & ST_FLAG)
}

#[derive(Debug, Clone, Copy)]
pub struct Lease {
    pub owner: PeerId,
    pub mode: u8,
    pub ver: u32,
    pub renewed: Instant,
    /// When this owner got the lease (touch hand-over waits TAKE_MIN_AGE).
    pub since: Instant,
    /// Last accepted position + time (speed check); None until first state.
    pub last: Option<([f32; 3], Instant)>,
}

#[derive(Debug, Clone, Copy)]
struct Cached {
    snap: WorldSnap,
}

#[derive(Default)]
pub struct Level {
    pub manifest: Vec<WorldEntry>,
    idx: HashMap<u32, usize>,
    dyn_count: HashMap<PeerId, usize>,
    pub leases: HashMap<u32, Lease>,
    /// Released bodies: id -> ver of the release record.
    freed: HashMap<u32, u32>,
    cache: HashMap<u32, Cached>,
    /// Newest pending state per id since the last flush.
    pending: HashMap<u32, (PeerId, u32, u32, WorldObj)>,
    keyframe_rr: usize,
    heal_rr: usize,
    flushes: u32,
    /// Server-built manifest this level was seeded from (strict alias mode).
    stat: Option<Arc<StaticLevel>>,
    /// Client-only entries accepted in strict mode.
    client_static: usize,
    /// Actor-state records: state id -> record.
    pub states: HashMap<u32, StateRec>,
    /// Initial-state authority of this (level, epoch) and its accepted count.
    pub init_by: Option<PeerId>,
    init_count: usize,
    /// Accepted initial anchors and release poses not yet fanned out (flushed by tick).
    init_out: Vec<WorldSnap>,
    /// Consistency reports: the latest per peer.
    pub hash_latest: HashMap<PeerId, HashReport>,
}

impl Level {
    fn seeded(stat: Option<&Arc<StaticLevel>>) -> Level {
        let mut lv = Level::default();
        if let Some(s) = stat {
            for e in &s.entries {
                lv.idx.insert(e.id, lv.manifest.len());
                lv.manifest.push(e.clone());
            }
            lv.stat = Some(s.clone());
        }
        lv
    }
    /// A pose a body may be streamed at / anchored to: finite, not at the
    /// origin, inside the arena box on server-built levels.
    pub fn pos_ok(&self, p: [f32; 3]) -> bool {
        p.iter().all(|v| v.is_finite() && v.abs() < WORLD_BOUND) && !at_origin(p)
            && self.stat.as_ref().map_or(true, |s| s.contains(p))
    }
    /// True if this level's manifest is server-built.
    #[allow(dead_code)]
    pub fn server_built(&self) -> bool { self.stat.is_some() }
    fn find_near_r(&self, chash: u32, pos: [f32; 3], r: f32) -> Option<&WorldEntry> {
        self.manifest.iter()
            .filter(|e| e.dyn_owner == 0 && e.chash == chash && dist(e.pos, pos) < r)
            .min_by(|a, b| dist(a.pos, pos).total_cmp(&dist(b.pos, pos)))
    }
    fn rec(&self, id: u32) -> WorldOwnerRec {
        if let Some(s) = self.states.get(&id) {
            return OwnerRec::new(id, s.by, s.ver, MODE_STATE | s.bits);
        }
        match self.leases.get(&id) {
            Some(l) => OwnerRec::new(id, l.owner, l.ver, l.mode),
            None => OwnerRec::new(id, 0, self.freed.get(&id).copied().unwrap_or(0), MODE_FREE),
        }
    }
    /// An id used as a body (lease, release or cached pose): never a state id.
    fn is_body(&self, id: u32) -> bool {
        self.leases.contains_key(&id) || self.freed.contains_key(&id) || self.cache.contains_key(&id)
    }
    pub fn records(&self) -> Vec<WorldOwnerRec> {
        let mut v: Vec<WorldOwnerRec> = self.leases.keys().chain(self.freed.keys()).chain(self.states.keys())
            .map(|id| self.rec(*id)).collect();
        v.sort_by_key(|r| r.id);
        v.dedup_by_key(|r| r.id);
        v
    }
}

struct PeerW {
    level: u32,
    epoch: u32,
    /// C2SWorldSync budget (each sync replies with the whole level).
    sync_tokens: f64,
    sync_t: Instant,
    claim_tokens: f64,
    claim_t: Instant,
    budget: f64,
    budget_t: Instant,
    last_hint: Option<Instant>,
}

#[derive(Default, Debug, Clone, Copy)]
pub struct Stats {
    pub states_in: u64,
    pub states_rejected: u64,
    pub objs_out: u64,
    pub objs_budget_dropped: u64,
    pub objs_thinned: u64,
    pub claims: u64,
    pub claims_granted: u64,
    pub leases_expired: u64,
    /// Server-built levels: client proposals folded into a canonical entry.
    pub manifest_aliased: u64,
    /// Client proposals refused (strict alias mode, bad dynamic items).
    pub manifest_rejected: u64,
    /// Initial anchors accepted / refused, actor-state changes,
    /// consistency verdicts and how many found mismatches.
    pub init_accepted: u64,
    pub init_refused: u64,
    pub state_changes: u64,
    pub verdicts: u64,
    pub verdicts_mismatch: u64,
    /// C2SWorldSync refused (per-peer budget, or every level slot occupied).
    pub syncs_limited: u64,
    /// Levels dropped to make room (nobody in them).
    pub levels_evicted: u64,
}

pub struct World {
    pub epoch: u32,
    ver: u32,
    pub levels: HashMap<u32, Level>,
    peers: HashMap<PeerId, PeerW>,
    last_heal: Option<Instant>,
    last_keyframe: Option<Instant>,
    pub stats: Stats,
    /// Server-built manifests by level hash.
    statics: Statics,
    /// Consistency verdicts since the glue last drained them (logging).
    pub verdicts: Vec<Verdict>,
    /// Each peer's root position from the last tick (touch hand-over arbitration).
    positions: HashMap<PeerId, [f32; 3]>,
}

/// Owner record (the wire row).
pub type WorldOwnerRec = OwnerRec;

/// One sender's objects inside a state packet (one `world_state` message, peer = sender).
#[derive(Debug, Clone, PartialEq)]
pub struct WorldGroup {
    pub peer_id: PeerId,
    pub seq: u32,
    pub ts: u32,
    pub objects: Vec<WorldObj>,
}

/// One outbound world message to one peer, built from the record rows. [`Msg::encode`]
/// frames it as v6 record messages (`hsmp_ipc::schema::world`); a `States` packet's
/// messages go out together (one datagram).
#[derive(Debug, Clone, PartialEq)]
pub enum Msg {
    /// A coalesced state packet: one `world_state` message per sender group.
    States { level: u32, epoch: u32, groups: Vec<WorldGroup> },
    /// `world_owners` (also the empty stale-epoch hint).
    Owners { level: u32, epoch: u32, sync: bool, manifest_len: u32, owners: Vec<OwnerRec> },
    /// `world_snapshot`: cached transforms / anchors (sender 0).
    Snapshot { level: u32, epoch: u32, objects: Vec<WorldSnap> },
    /// Canonical manifest entries: static ones as `world_manifest`, dynamic ones as
    /// `world_dyn`, both with `req` (the request answered, 0 = broadcast / sync).
    Manifest { level: u32, epoch: u32, req: u32, entries: Vec<WorldEntry> },
    /// `world_verdict` on the receiver's report.
    Verdict(rec::VerdictHead, Vec<Mismatch>),
}

impl Msg {
    /// The framed v6 record messages (`[WireHdr][record]`); several when a set is larger than
    /// one record's capacity.
    pub fn encode(&self) -> Vec<Vec<u8>> {
        use hsmp_ipc::layout::Bool;
        use hsmp_ipc::wire::encode;
        let mut out = Vec::new();
        match self {
            Msg::States { level, epoch, groups } => {
                for g in groups {
                    let h = rec::WorldStateHead { level: *level, epoch: *epoch, seq: g.seq, ts: g.ts, n: 0, _r: 0, _r2: 0 };
                    for c in g.objects.chunks(rec::STATE_MAX) {
                        out.push(encode(0, g.peer_id, &h, c));
                    }
                }
            }
            Msg::Owners { level, epoch, sync, manifest_len, owners } => {
                let h = rec::OwnersHead { level: *level, epoch: *epoch, manifest_len: *manifest_len, n: 0,
                                          sync: Bool::from(*sync), _r: 0 };
                if owners.is_empty() {
                    out.push(encode(0, 0, &h, &[]));
                }
                for c in owners.chunks(rec::OWNERS_MAX) {
                    out.push(encode(0, 0, &h, c));
                }
            }
            Msg::Snapshot { level, epoch, objects } => {
                let h = rec::SnapHead { level: *level, epoch: *epoch, n: 0, _r: 0, _r2: 0 };
                for c in objects.chunks(rec::SNAPS_MAX) {
                    out.push(encode(0, 0, &h, c));
                }
            }
            Msg::Manifest { level, epoch, req, entries } => {
                let st: Vec<rec::ManifestEntry> = entries.iter().filter(|e| !e.is_dyn()).map(WorldEntry::static_row).collect();
                let dy: Vec<rec::DynEntry> = entries.iter().filter(|e| e.is_dyn()).map(WorldEntry::dyn_row).collect();
                let hs = rec::ManifestHead { level: *level, epoch: *epoch, req: *req, n: 0, _r: 0 };
                let hd = rec::DynHead { level: *level, epoch: *epoch, req: *req, n: 0, _r: 0 };
                for c in st.chunks(rec::MANIFEST_MAX) {
                    out.push(encode(0, 0, &hs, c));
                }
                for c in dy.chunks(rec::DYN_WIRE_MAX) {
                    out.push(encode(0, 0, &hd, c));
                }
                // A request is always answered, even when every entry was refused.
                if out.is_empty() && *req != 0 {
                    out.push(encode(0, 0, &hs, &[]));
                }
            }
            Msg::Verdict(h, rows) => out.push(encode(0, 0, h, rows)),
        }
        out
    }
}

/// Outbound message to one peer.
pub type Out = (PeerId, Msg);

pub enum ClaimResult {
    /// Not in this peer's (level, epoch) or unknown id: answer with a hint.
    Stale,
    Unknown,
    RateLimited,
    /// `changed` = the record changed (broadcast) or not (ack the requester).
    Done { rec: WorldOwnerRec, changed: bool },
    /// CLAIM_INIT accepted: the anchor is fanned out by the next tick.
    Anchored,
    /// CLAIM_INIT / CLAIM_STATE refused (not the init authority, already
    /// anchored, owned, or the id is used as the other kind). Nothing to send.
    Ignored,
}

impl Default for World {
    fn default() -> Self { Self::new() }
}

impl World {
    /// World with the built-in server-built arena manifests.
    pub fn new() -> Self { Self::with_statics(builtin_statics()) }

    pub fn with_statics(statics: Statics) -> Self {
        World { epoch: 1, ver: 0, levels: HashMap::new(), peers: HashMap::new(),
                last_heal: None, last_keyframe: None, stats: Stats::default(), statics, verdicts: Vec::new(), positions: HashMap::new() }
    }

    /// Level bucket, seeded from the server-built manifest when one exists.
    fn level_mut(&mut self, level: u32) -> &mut Level {
        let statics = &self.statics;
        self.levels.entry(level).or_insert_with(|| Level::seeded(statics.get(&level)))
    }

    /// Every arena is about to be reloaded (round countdown / lobby): drop
    /// all world state. Returns hint messages so in-level clients learn the
    /// new epoch even before their reload (they reset locally and re-sync).
    pub fn bump_epoch(&mut self) -> Vec<Out> {
        self.epoch = self.epoch.wrapping_add(1).max(1);
        self.levels.clear();
        let epoch = self.epoch;
        self.peers.iter().map(|(p, w)| (*p, Msg::Owners {
            level: w.level, epoch, sync: false, manifest_len: 0, owners: Vec::new(),
        })).collect()
    }

    fn in_scope(&self, peer: PeerId, level: u32, epoch: u32) -> bool {
        epoch == self.epoch && self.peers.get(&peer).map_or(false, |w| w.level == level && w.epoch == epoch)
    }

    /// Rate-limited "your epoch is stale" hint (empty owners, current epoch).
    pub fn stale_hint(&mut self, peer: PeerId, level: u32, now: Instant) -> Option<Out> {
        let epoch = self.epoch;
        if let Some(w) = self.peers.get_mut(&peer) {
            if w.last_hint.map_or(false, |t| now.duration_since(t) < HINT_EVERY) { return None; }
            w.last_hint = Some(now);
        }
        Some((peer, Msg::Owners { level, epoch, sync: false, manifest_len: 0, owners: Vec::new() }))
    }

    fn peers_in(&self, level: u32) -> Vec<PeerId> {
        let mut v: Vec<PeerId> = self.peers.iter()
            .filter(|(_, w)| w.level == level && w.epoch == self.epoch)
            .map(|(p, _)| *p).collect();
        v.sort_unstable();
        v
    }

    /// C2SWorldSync: register the peer in (level, current epoch) and reply
    /// with owners (sync=true), the full manifest and cached transforms.
    pub fn sync(&mut self, peer: PeerId, level: u32, now: Instant) -> Vec<Out> {
        let epoch = self.epoch;
        let w = self.peers.entry(peer).or_insert(PeerW {
            level, epoch, sync_tokens: SYNC_BURST, sync_t: now, claim_tokens: CLAIM_BURST, claim_t: now,
            budget: BUDGET_BURST, budget_t: now, last_hint: None,
        });
        // Per-peer sync budget: an honest sidecar syncs once per
        // level load and at most every 3 s on a manifest gap.
        let dt = now.saturating_duration_since(w.sync_t).as_secs_f64();
        w.sync_t = w.sync_t.max(now);
        w.sync_tokens = (w.sync_tokens + dt * SYNC_RATE).min(SYNC_BURST);
        if w.sync_tokens < 1.0 {
            self.stats.syncs_limited += 1;
            return Vec::new();
        }
        w.sync_tokens -= 1.0;
        // Level ids are client-chosen: never keep more than MAX_LEVELS. A new
        // level evicts every level nobody else is in (the sender is leaving
        // its old one); if every slot is occupied by players, it is refused
        // and the sender stays where it was.
        if !self.levels.contains_key(&level) && self.levels.len() >= MAX_LEVELS {
            let occupied: HashSet<u32> = self.peers.iter()
                .filter(|(p, w)| **p != peer && w.epoch == epoch).map(|(_, w)| w.level).collect();
            let before = self.levels.len();
            self.levels.retain(|id, _| occupied.contains(id));
            self.stats.levels_evicted += (before - self.levels.len()) as u64;
            if self.levels.len() >= MAX_LEVELS {
                self.stats.syncs_limited += 1;
                return Vec::new();
            }
        }
        if let Some(w) = self.peers.get_mut(&peer) {
            w.level = level;
            w.epoch = epoch;
        }
        let lv = self.level_mut(level);
        let mut out = Vec::new();
        let recs = lv.records();
        let mlen = lv.manifest.len() as u32;
        // One message each (the transport fragments up to 64 KiB): owners first (sync=true),
        // then the whole manifest, then the cached transforms.
        out.push((peer, Msg::Owners { level, epoch, sync: true, manifest_len: mlen, owners: recs }));
        if !lv.manifest.is_empty() {
            out.push((peer, Msg::Manifest { level, epoch, req: 0, entries: lv.manifest.clone() }));
        }
        let snaps: Vec<WorldSnap> = lv.cache.values().map(|c| c.snap).collect();
        if !snaps.is_empty() {
            out.push((peer, Msg::Snapshot { level, epoch, objects: snaps }));
        }
        out
    }

    /// C2SWorldManifest. Returns the reply (canonical entry per submitted
    /// one) and a broadcast of newly added entries to the other peers.
    pub fn manifest(&mut self, peer: PeerId, level: u32, epoch: u32, req: u32,
                    entries: Vec<WorldEntry>, now: Instant) -> Vec<Out> {
        if !self.in_scope(peer, level, epoch) {
            return self.stale_hint(peer, level, now).into_iter().collect();
        }
        let mut aliased = 0u64;
        let mut rejected = 0u64;
        let lv = self.level_mut(level);
        let mut reply = Vec::new();
        let mut added = Vec::new();
        for mut e in entries.into_iter().take(rec::PROPOSE_MAX) {
            if !e.pos.iter().all(|p| p.is_finite() && p.abs() < WORLD_BOUND) { rejected += 1; continue; }
            if let Some(&i) = lv.idx.get(&e.id) {
                reply.push(lv.manifest[i].clone());
                continue;
            }
            if e.is_dyn() {
                // Dynamic item: id namespace, owner and class must match the sender.
                let n = lv.dyn_count.get(&peer).copied().unwrap_or(0);
                if e.dyn_owner != peer || !dyn_id_ok(e.id, peer) || n >= MAX_DYN_PER_PEER || !dyn_entry_ok(&e)
                    || !lv.pos_ok(e.pos) {
                    rejected += 1;
                    continue;
                }
                lv.dyn_count.insert(peer, n + 1);
            } else {
                e.class.clear();
                let strict = lv.stat.clone();
                let r = if strict.is_some() { ALIAS_R } else { MATCH_R };
                if let Some(ex) = lv.find_near_r(e.chash, e.pos, r) {
                    reply.push(ex.clone());
                    if strict.is_some() { aliased += 1; }
                    continue;
                }
                if let Some(st) = strict {
                    // Server-built level: only bodies near a static object of
                    // this arena may extend the manifest (multi-body props,
                    // chest loot); anything else is refused.
                    if !st.vouches(e.pos) || lv.client_static >= MAX_CLIENT_STATIC {
                        rejected += 1;
                        continue;
                    }
                    lv.client_static += 1;
                }
                if lv.manifest.len() >= MAX_STATIC + MAX_DYN_PER_PEER * 16 { rejected += 1; continue; }
            }
            lv.idx.insert(e.id, lv.manifest.len());
            lv.manifest.push(e.clone());
            reply.push(e.clone());
            added.push(e);
        }
        self.stats.manifest_aliased += aliased;
        self.stats.manifest_rejected += rejected;
        // Always ack the request, even when every entry was refused, so the
        // sidecar stops resending it.
        // (`Msg::encode` answers an empty reply with an empty `world_manifest`.)
        let mut out: Vec<Out> = vec![(peer, Msg::Manifest { level, epoch, req, entries: reply })];
        if !added.is_empty() {
            for p in self.peers_in(level).into_iter().filter(|p| *p != peer) {
                out.push((p, Msg::Manifest { level, epoch, req: 0, entries: added.clone() }));
            }
        }
        out
    }

    fn take_claim_token(&mut self, peer: PeerId, now: Instant) -> bool {
        let Some(w) = self.peers.get_mut(&peer) else { return false };
        let dt = now.duration_since(w.claim_t).as_secs_f64();
        w.claim_t = now;
        w.claim_tokens = (w.claim_tokens + dt * CLAIM_RATE).min(CLAIM_BURST);
        if w.claim_tokens < 1.0 { return false; }
        w.claim_tokens -= 1.0;
        true
    }

    /// Ownership arbitration (see module docs).
    pub fn claim(&mut self, peer: PeerId, level: u32, epoch: u32, id: u32, mode: u8,
                 rest: Option<WorldObj>, now: Instant) -> ClaimResult {
        if !self.in_scope(peer, level, epoch) { return ClaimResult::Stale; }
        if !self.levels.get(&level).map_or(false, |l| l.idx.contains_key(&id)) { return ClaimResult::Unknown; }
        // Initial anchors come in one burst per level load: not rate-limited
        // (bounded by the manifest size instead).
        if mode == CLAIM_INIT { return self.init_anchor(peer, level, id, rest); }
        if !self.take_claim_token(peer, now) { return ClaimResult::RateLimited; }
        if mode == CLAIM_STATE {
            let bits = rest.map_or(0, |r| r.vel[0].clamp(0, 0x7F) as u8);
            return self.set_state(peer, level, id, bits);
        }
        if self.levels[&level].states.contains_key(&id) { return ClaimResult::Ignored; }
        self.stats.claims += 1;
        let mode = mode.min(MODE_HOLD_L);
        let ver = self.ver.wrapping_add(1).max(1);
        let lv = self.levels.get_mut(&level).unwrap();
        let cur = lv.leases.get(&id).copied().filter(|l| now.duration_since(l.renewed) <= LEASE);
        let mut changed = false;
        if mode == MODE_FREE {
            if cur.map(|l| l.owner) == Some(peer) {
                lv.leases.remove(&id);
                lv.freed.insert(id, ver);
                if let Some(mut r) = rest.filter(|r| lv.pos_ok(r.pos)) {
                    r.id = id;
                    r.flags |= WF_ASLEEP;
                    let snap = WorldSnap::new(0, 0, 0, r);
                    lv.cache.insert(id, Cached { snap });
                    // the new anchor goes out with the next tick, not a keyframe round later
                    lv.init_out.push(snap);
                }
                changed = true;
            }
        } else {
            let grant = match cur {
                None => true,
                Some(l) if l.owner == peer => true,
                // Hold beats touch; nobody can rob a holder; a touch goes to the nearer pusher.
                Some(l) => (l.mode == MODE_TOUCH && mode >= MODE_HOLD_R)
                    || (l.mode == MODE_TOUCH && mode == MODE_TOUCH && touch_take(&self.positions, lv, id, &l, peer, now)),
            };
            if grant {
                match lv.leases.get_mut(&id) {
                    Some(l) if cur.is_some() && l.owner == peer => {
                        l.renewed = now;
                        if l.mode != mode { l.mode = mode; l.ver = ver; changed = true; }
                    }
                    _ => {
                        lv.leases.insert(id, Lease { owner: peer, mode, ver, renewed: now, since: now, last: None });
                        lv.freed.remove(&id);
                        changed = true;
                    }
                }
            }
        }
        if changed {
            self.ver = ver;
            self.stats.claims_granted += 1;
        }
        let rec = self.levels[&level].rec(id);
        ClaimResult::Done { rec, changed }
    }

    /// The init authority of a level, if it is still in that (level, epoch).
    fn live_init_by(&self, level: u32) -> Option<PeerId> {
        let p = self.levels.get(&level)?.init_by?;
        let w = self.peers.get(&p)?;
        (w.level == level && w.epoch == self.epoch).then_some(p)
    }

    /// CLAIM_INIT: the first peer to propose initial poses for a (level,
    /// epoch) becomes its init authority; only its proposals become anchors,
    /// and only for bodies that have neither an anchor nor an owner yet.
    fn init_anchor(&mut self, peer: PeerId, level: u32, id: u32, rest: Option<WorldObj>) -> ClaimResult {
        let auth = self.live_init_by(level);
        let lv = self.levels.get_mut(&level).unwrap();
        let ok = auth.map_or(true, |p| p == peer)
            && !lv.leases.contains_key(&id) && !lv.cache.contains_key(&id) && !lv.states.contains_key(&id)
            && lv.init_count < lv.manifest.len().max(1) * 2;
        let Some(mut r) = rest.filter(|r| ok && lv.pos_ok(r.pos)) else {
            self.stats.init_refused += 1;
            return ClaimResult::Ignored;
        };
        lv.init_by = Some(peer);
        lv.init_count += 1;
        r.id = id;
        r.flags = (r.flags | WF_ASLEEP) & !WF_HELD;
        let snap = WorldSnap::new(0, 0, 0, r);
        lv.cache.insert(id, Cached { snap });
        lv.init_out.push(snap);
        self.stats.init_accepted += 1;
        ClaimResult::Anchored
    }

    /// CLAIM_STATE: merge a peer's local actor-state bits into the record
    /// (breaks sticky, flag last-writer-wins) and version the change.
    fn set_state(&mut self, peer: PeerId, level: u32, id: u32, bits: u8) -> ClaimResult {
        let lv = self.levels.get_mut(&level).unwrap();
        if lv.is_body(id) { return ClaimResult::Ignored; }
        let old = lv.states.get(&id).map(|s| s.bits);
        let new = merge_state(old.unwrap_or(0), bits);
        if old == Some(new) || (old.is_none() && new == 0) {
            return ClaimResult::Done { rec: lv.rec(id), changed: false };
        }
        let ver = self.ver.wrapping_add(1).max(1);
        self.ver = ver;
        lv.states.insert(id, StateRec { bits: new, by: peer, ver });
        self.stats.state_changes += 1;
        ClaimResult::Done { rec: lv.rec(id), changed: true }
    }

    /// A consistency report (`world_hash`: one message, the whole report): diffs it against
    /// the most recent report of another peer in the level and answers the reporter with a
    /// `world_verdict`. Mismatched free bodies get their anchors re-sent to both.
    pub fn hash_report(&mut self, peer: PeerId, level: u32, epoch: u32, seq: u32, client_hash: u32,
                       rows: &[rec::HashRow], now: Instant) -> Vec<Out> {
        if !self.in_scope(peer, level, epoch) {
            return self.stale_hint(peer, level, now).into_iter().collect();
        }
        let others = self.peers_in(level);
        let lv = self.level_mut(level);
        let mut bodies: Vec<HashBody> = rows.iter()
            .filter(|r| r.pos.iter().all(|v| v.abs() < WORLD_BOUND))
            .map(HashBody::from_row).collect();
        bodies.sort_by_key(|b| b.id);
        bodies.dedup_by_key(|b| b.id);
        let own: Vec<HashBody> = bodies.iter().copied().filter(|b| b.comparable() || b.missing()).collect();
        let codec_ok = world_hash(&own) == client_hash;
        let report = HashReport { seq, t: now, client_hash, bodies };
        // The most recent fresh report of another peer in this level.
        let other = lv.hash_latest.iter()
            .filter(|(p, r)| **p != peer && others.contains(p) && now.duration_since(r.t) <= HASH_MAX_AGE)
            .max_by_key(|(_, r)| r.t)
            .map(|(p, r)| (*p, compare_reports(&report.bodies, &r.bodies)));
        lv.hash_latest.insert(peer, report);
        let Some((other, cmp)) = other else { return Vec::new() };
        // Heal: re-send the anchors of mismatched free bodies to both peers.
        let heal: Vec<WorldSnap> = cmp.mismatched.iter()
            .filter(|m| m.kind == MM_POSE && !lv.leases.contains_key(&m.id))
            .filter_map(|m| lv.cache.get(&m.id).map(|c| WorldSnap { sender: 0, ..c.snap }))
            .collect();
        let v = Verdict { level, epoch, peer, other, seq, compared: cmp.compared,
                          hash_equal: cmp.hash_a == cmp.hash_b, codec_ok, mismatched: cmp.mismatched };
        let (vh, vrows) = v.record();
        let mut out = vec![(peer, Msg::Verdict(vh, vrows))];
        if !heal.is_empty() {
            for p in [peer, other] {
                out.push((p, Msg::Snapshot { level, epoch, objects: heal.clone() }));
            }
        }
        self.stats.verdicts += 1;
        if !v.hash_match() { self.stats.verdicts_mismatch += 1; }
        if self.verdicts.len() < 256 { self.verdicts.push(v); }
        out
    }

    /// Owner-record broadcast to everyone in the level (after a change).
    pub fn owners_to_level(&self, level: u32, recs: &[WorldOwnerRec]) -> Vec<Out> {
        let mlen = self.levels.get(&level).map_or(0, |l| l.manifest.len() as u32);
        let mut out = Vec::new();
        for p in self.peers_in(level) {
            out.push((p, Msg::Owners { level, epoch: self.epoch, sync: false, manifest_len: mlen, owners: recs.to_vec() }));
        }
        out
    }

    /// C2SWorldState. `owner_pos` = the sender's last valid root position.
    /// Returns the number of accepted objects plus owner-record changes
    /// (implicit claims) to broadcast.
    pub fn state(&mut self, peer: PeerId, level: u32, epoch: u32, seq: u32, ts: u32,
                 objects: &[WorldObj], owner_pos: Option<[f32; 3]>, now: Instant)
                 -> (usize, Vec<WorldOwnerRec>) {
        if !self.in_scope(peer, level, epoch) { return (0, Vec::new()); }
        let mut changed = Vec::new();
        let mut accepted = 0;
        let mut ver = self.ver;
        let Some(lv) = self.levels.get_mut(&level) else { return (0, Vec::new()) };
        for &o in objects.iter().take(MAX_OBJS_PER_STATE) {
            let mut o = o;
            self.stats.states_in += 1;
            if !lv.idx.contains_key(&o.id) || !lv.pos_ok(o.pos) || lv.states.contains_key(&o.id) {
                self.stats.states_rejected += 1;
                continue;
            }
            // Validate before the implicit touch claim below: a state the
            // server refuses (a held body far from its holder) must not still
            // take ownership of the body away from everyone else.
            if o.flags & WF_HELD != 0 {
                if let Some(op) = owner_pos {
                    if dist(op, o.pos) > HOLD_RADIUS { self.stats.states_rejected += 1; continue; }
                }
            }
            let live = lv.leases.get(&o.id).copied().filter(|l| now.duration_since(l.renewed) <= LEASE);
            let held_mode = if o.flags & WF_HELD == 0 { MODE_TOUCH } else if o.flags & WF_LEFT != 0 { MODE_HOLD_L } else { MODE_HOLD_R };
            match live {
                // my touch lease, and I am holding it now: it is a hold from this state on
                Some(l) if l.owner == peer && l.mode == MODE_TOUCH && held_mode != MODE_TOUCH => {
                    ver = ver.wrapping_add(1).max(1);
                    if let Some(m) = lv.leases.get_mut(&o.id) { m.mode = held_mode; m.ver = ver; }
                    changed.push(OwnerRec::new(o.id, peer, ver, held_mode));
                }
                Some(l) if l.owner == peer => {}
                Some(_) => { self.stats.states_rejected += 1; continue; }
                None => {
                    // Implicit claim of a free (or lapsed) body: a hold if the state says it is
                    // held (else a later pickup would take it from its first holder), else a touch.
                    let mode = held_mode;
                    ver = ver.wrapping_add(1).max(1);
                    lv.leases.insert(o.id, Lease { owner: peer, mode, ver, renewed: now, since: now, last: None });
                    lv.freed.remove(&o.id);
                    changed.push(OwnerRec::new(o.id, peer, ver, mode));
                }
            }
            let l = lv.leases.get_mut(&o.id).unwrap();
            if let Some((lp, lt)) = l.last {
                let dt = now.duration_since(lt).as_secs_f32();
                if dist(lp, o.pos) > MAX_SPEED * dt + SPEED_SLACK {
                    self.stats.states_rejected += 1;
                    continue;
                }
            }
            l.last = Some((o.pos, now));
            l.renewed = now;
            o.flags &= !0x30; // reserved bits
            let snap = WorldSnap::new(peer, seq, ts, o);
            lv.cache.insert(o.id, Cached { snap });
            match lv.pending.get(&o.id) {
                // Keep a pending "asleep" frame over a newer awake one? No:
                // newest wins; rest poses are re-sent by keyframes anyway.
                Some(&(s, q, _, _)) if s == peer && (seq.wrapping_sub(q) as i32) < 0 => {}
                _ => { lv.pending.insert(o.id, (peer, seq, ts, o)); }
            }
            accepted += 1;
        }
        self.ver = ver;
        (accepted, changed)
    }

    /// Periodic work (call ~30 Hz): lease expiry, coalesced fan-out with
    /// relevance + budget, owner-table heal, rest-pose keyframes.
    /// `present` = connected peer ids, `positions` = their root positions.
    pub fn tick(&mut self, now: Instant, present: &HashSet<PeerId>,
                positions: &HashMap<PeerId, [f32; 3]>) -> Vec<Out> {
        let mut out = Vec::new();
        self.peers.retain(|p, _| present.contains(p));
        self.positions = positions.clone();

        // ---- lease expiry (timeout or owner gone) ----
        let mut expired: Vec<(u32, Vec<WorldOwnerRec>)> = Vec::new();
        let mut ver = self.ver;
        for (lid, lv) in self.levels.iter_mut() {
            let dead: Vec<u32> = lv.leases.iter()
                .filter(|(_, l)| !present.contains(&l.owner) || now.duration_since(l.renewed) > LEASE)
                .map(|(id, _)| *id).collect();
            if dead.is_empty() { continue; }
            let mut recs = Vec::new();
            for id in dead {
                lv.leases.remove(&id);
                ver = ver.wrapping_add(1).max(1);
                lv.freed.insert(id, ver);
                // Its last streamed state becomes the anchor.
                if let Some(c) = lv.cache.get_mut(&id) { c.snap.sender = 0; }
                recs.push(OwnerRec::new(id, 0, ver, MODE_FREE));
                self.stats.leases_expired += 1;
            }
            expired.push((*lid, recs));
        }
        self.ver = ver;
        for (lid, recs) in expired { out.extend(self.owners_to_level(lid, &recs)); }

        // ---- initial anchors: fan out, release a departed authority ----
        let level_ids: Vec<u32> = self.levels.keys().copied().collect();
        for lid in level_ids {
            let auth = self.live_init_by(lid);
            let recips = self.peers_in(lid);
            let epoch = self.epoch;
            let lv = self.levels.get_mut(&lid).unwrap();
            if lv.init_by.is_some() && auth.is_none() {
                // The authority left mid-init: the next proposer completes it.
                lv.init_by = None;
            }
            if lv.init_out.is_empty() { continue; }
            let snaps = std::mem::take(&mut lv.init_out);
            for p in &recips {
                out.push((*p, Msg::Snapshot { level: lid, epoch, objects: snaps.clone() }));
            }
        }

        // ---- coalesced, relevance-thinned, budgeted state fan-out ----
        let epoch = self.epoch;
        let level_ids: Vec<u32> = self.levels.keys().copied().collect();
        for lid in level_ids {
            let recips = self.peers_in(lid);
            let lv = self.levels.get_mut(&lid).unwrap();
            lv.flushes = lv.flushes.wrapping_add(1);
            if lv.pending.is_empty() { continue; }
            let pending: Vec<(PeerId, u32, u32, WorldObj)> = lv.pending.drain().map(|(_, v)| v).collect();
            let flush = lv.flushes;
            for dst in recips {
                let dpos = positions.get(&dst).copied();
                // (priority, sender, seq, ts, obj); lower priority value = more important.
                let mut cand: Vec<(f32, PeerId, u32, u32, WorldObj)> = Vec::new();
                for &(s, q, t, o) in &pending {
                    if s == dst { continue; }
                    let d = dpos.map_or(0.0, |p| dist(p, o.pos));
                    // Rest frames always go; held items only thin beyond FAR2.
                    let urgent = o.flags & WF_ASLEEP != 0 || (o.flags & WF_HELD != 0 && d <= FAR2);
                    let every = if d > FAR2 { 4 } else if d > FAR1 { 2 } else { 1 };
                    if !urgent && every > 1 && !flush.is_multiple_of(every) {
                        self.stats.objs_thinned += 1;
                        continue;
                    }
                    let prio = if o.flags & WF_HELD != 0 { d * 0.5 } else { d };
                    cand.push((prio, s, q, t, o));
                }
                if cand.is_empty() { continue; }
                cand.sort_by(|a, b| a.0.total_cmp(&b.0));
                let Some(w) = self.peers.get_mut(&dst) else { continue };
                let dt = now.duration_since(w.budget_t).as_secs_f64();
                w.budget_t = now;
                w.budget = (w.budget + dt * BUDGET_BPS).min(BUDGET_BURST);
                let n_cand = cand.len();
                let mut groups: Vec<WorldGroup> = Vec::new();
                let mut bytes = 0usize;
                let mut sent = 0usize;
                // Invariant: budget ≥ cost of the packet being built.
                for (_, s, q, t, o) in cand {
                    let mut add = OBJ_BYTES + if groups.iter().any(|g| g.peer_id == s && g.seq == q) { 0 } else { GROUP_BYTES };
                    if bytes + add > MAX_BODY_BYTES {
                        w.budget -= (bytes + WIRE_OVERHEAD) as f64;
                        out.push((dst, Msg::States { level: lid, epoch, groups: std::mem::take(&mut groups) }));
                        bytes = 0;
                        add = OBJ_BYTES + GROUP_BYTES;
                    }
                    if w.budget < (bytes + add + WIRE_OVERHEAD) as f64 { break; }
                    push_obj(&mut groups, &mut bytes, s, q, t, o);
                    sent += 1;
                }
                if !groups.is_empty() {
                    w.budget -= (bytes + WIRE_OVERHEAD) as f64;
                    out.push((dst, Msg::States { level: lid, epoch, groups }));
                }
                self.stats.objs_out += sent as u64;
                self.stats.objs_budget_dropped += (n_cand - sent) as u64;
            }
        }

        // ---- owner-table heal (repairs lost owner updates) ----
        if self.last_heal.map_or(true, |t| now.duration_since(t) >= HEAL_EVERY) {
            self.last_heal = Some(now);
            let level_ids: Vec<u32> = self.levels.keys().copied().collect();
            for lid in level_ids {
                let recips = self.peers_in(lid);
                if recips.is_empty() { continue; }
                let lv = self.levels.get_mut(&lid).unwrap();
                let recs = lv.records();
                let mlen = lv.manifest.len() as u32;
                // Rotate through big tables: ≤ OWNERS_PER_HEAL records per heal.
                let take = OWNERS_PER_HEAL;
                let slice: Vec<WorldOwnerRec> = if recs.len() <= take { recs } else {
                    let start = lv.heal_rr % recs.len();
                    lv.heal_rr = (start + take) % recs.len();
                    recs.iter().cycle().skip(start).take(take).copied().collect()
                };
                for p in recips {
                    out.push((p, Msg::Owners { level: lid, epoch, sync: false, manifest_len: mlen, owners: slice.clone() }));
                }
            }
        }

        // ---- rest-pose keyframes for free bodies (heal lost final frames) ----
        if self.last_keyframe.map_or(true, |t| now.duration_since(t) >= KEYFRAME_EVERY) {
            self.last_keyframe = Some(now);
            let level_ids: Vec<u32> = self.levels.keys().copied().collect();
            for lid in level_ids {
                let recips = self.peers_in(lid);
                let lv = self.levels.get_mut(&lid).unwrap();
                let mut free: Vec<u32> = lv.cache.keys().copied().filter(|id| !lv.leases.contains_key(id)).collect();
                if free.is_empty() || recips.is_empty() { continue; }
                free.sort_unstable();
                let start = lv.keyframe_rr % free.len();
                let n = KEYFRAMES_PER_TICK.min(free.len());
                lv.keyframe_rr = (start + n) % free.len();
                let snaps: Vec<WorldSnap> = (0..n).map(|k| {
                    let mut s = lv.cache[&free[(start + k) % free.len()]].snap;
                    s.sender = 0;
                    s
                }).collect();
                for p in recips {
                    out.push((p, Msg::Snapshot { level: lid, epoch, objects: snaps.clone() }));
                }
            }
        }
        out
    }

    pub fn peer_count(&self) -> usize { self.peers.len() }
}

/// A touch claim by `peer` on a body another peer's touch lease `l` holds: the claimer's root
/// must be near the body and clearly nearer than the owner's (see TAKE_R).
fn touch_take(pos: &HashMap<PeerId, [f32; 3]>, lv: &Level, id: u32, l: &Lease, peer: PeerId, now: Instant) -> bool {
    if now.saturating_duration_since(l.since) < TAKE_MIN_AGE { return false; }
    let Some(obj) = lv.cache.get(&id).map(|c| c.snap.obj) else { return false };
    let v = obj.vel.map(|x| x as f32);
    if (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt() > TAKE_SPEED { return false; }
    let body = obj.pos;
    let (Some(me), Some(them)) = (pos.get(&peer), pos.get(&l.owner)) else { return false };
    let (dm, dt) = (dist(*me, body), dist(*them, body));
    dm <= TAKE_R && dm + TAKE_MARGIN <= dt
}

fn push_obj(groups: &mut Vec<WorldGroup>, bytes: &mut usize, s: PeerId, q: u32, t: u32, o: WorldObj) {
    // one group per sender batch: each row keeps its own sample time
    match groups.iter_mut().find(|g| g.peer_id == s && g.seq == q) {
        Some(g) => { g.objects.push(o); *bytes += OBJ_BYTES; }
        None => {
            groups.push(WorldGroup { peer_id: s, seq: q, ts: t, objects: vec![o] });
            *bytes += OBJ_BYTES + GROUP_BYTES;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use hsmp_ipc::schema::world::WF_SIM;

    fn ent(id: u32, chash: u32, pos: [f32; 3]) -> WorldEntry {
        WorldEntry { id, chash, pos, dyn_owner: 0, class: String::new(), passport: None }
    }
    fn obj(id: u32, pos: [f32; 3], flags: u8) -> WorldObj {
        WorldObj::from_parts(id, pos, [0.0, 0.0, 0.0], [0.0; 3], flags)
    }
    fn setup(n: u32) -> (World, Instant) {
        let mut w = World::new();
        let t = Instant::now();
        for p in 1..=n { w.sync(p, 77, t); }
        let e = w.epoch;
        w.manifest(1, 77, e, 1, vec![ent(10, 5, [0.0, 0.0, 0.0]), ent(11, 5, [500.0, 0.0, 0.0]),
                                     ent(12, 6, [0.0, 900.0, 0.0])], t);
        (w, t)
    }
    fn done(r: ClaimResult) -> (WorldOwnerRec, bool) {
        match r { ClaimResult::Done { rec, changed } => (rec, changed), _ => panic!("claim not done") }
    }
    fn present(ids: &[PeerId]) -> HashSet<PeerId> { ids.iter().copied().collect() }

    /// One client cycling client-chosen level ids cannot grow
    /// the level map; levels other players are in survive; syncs are
    /// budgeted per peer.
    #[test]
    fn level_map_is_bounded_and_syncs_are_budgeted() {
        let (mut w, t) = setup(2); // peers 1 and 2 in level 77 with a manifest
        for i in 0..2_000u32 {
            let now = t + Duration::from_secs(i as u64 + 1);
            let out = w.sync(1, 1_000 + i, now);
            assert!(!out.is_empty(), "an honest pace is never limited");
            let e = w.epoch;
            w.manifest(1, 1_000 + i, e, 1, vec![ent(10_000 + i, 5, [100.0, 0.0, 0.0])], now);
            assert!(w.levels.len() <= MAX_LEVELS, "{}", w.levels.len());
        }
        assert!(w.levels.contains_key(&77), "peer 2's level is kept");
        assert_eq!(w.levels[&77].manifest.len(), 3);
        assert!(w.levels.contains_key(&2_999), "the sender's current level exists");
        // A burst at one instant: only SYNC_BURST are served.
        let now = t + Duration::from_secs(10_000);
        let served = (0..50).filter(|k| !w.sync(2, 77 + (k % 2), now).is_empty()).count();
        assert_eq!(served as f64, SYNC_BURST);
        assert!(w.stats.syncs_limited > 0 && w.stats.levels_evicted > 0);
    }

    /// A refused state (a 'held' body far from its holder) takes no lease:
    /// granting the implicit touch claim before the hold check would let a
    /// client lock any free body away from everyone by spamming it.
    #[test]
    fn refused_held_state_takes_no_lease() {
        let (mut w, t) = setup(2);
        let e = w.epoch;
        let far_owner = Some([5_000.0, 0.0, 100.0]);
        let (n, recs) = w.state(1, 77, e, 1, 1, &[obj(11, [500.0, 0.0, 0.0], WF_HELD)], far_owner, t);
        assert_eq!((n, recs.len()), (0, 0));
        assert!(!w.levels[&77].leases.contains_key(&11), "no lease from a refused state");
        // Peer 2 can still take it.
        let (n, _) = w.state(2, 77, e, 1, 1, &[obj(11, [505.0, 0.0, 0.0], 0)], Some([600.0, 0.0, 0.0]), t);
        assert_eq!(n, 1);
        assert_eq!(w.levels[&77].leases[&11].owner, 2);
    }

    #[test]
    fn first_claim_wins_and_second_is_refused() {
        let (mut w, t) = setup(2);
        let e = w.epoch;
        let (r1, c1) = done(w.claim(1, 77, e, 10, MODE_TOUCH, None, t));
        assert!(c1 && r1.owner == 1 && r1.mode == MODE_TOUCH);
        let (r2, c2) = done(w.claim(2, 77, e, 10, MODE_TOUCH, None, t));
        assert!(!c2 && r2.owner == 1, "touch can't steal touch");
        // Idempotent re-claim by the owner renews without a new version.
        let (r3, c3) = done(w.claim(1, 77, e, 10, MODE_TOUCH, None, t));
        assert!(!c3 && r3.ver == r1.ver);
    }

    #[test]
    fn hold_beats_touch_but_holder_cannot_be_robbed() {
        let (mut w, t) = setup(3);
        let e = w.epoch;
        done(w.claim(1, 77, e, 10, MODE_TOUCH, None, t));
        let (r, c) = done(w.claim(2, 77, e, 10, MODE_HOLD_R, None, t));
        assert!(c && r.owner == 2 && r.mode == MODE_HOLD_R, "pickup steals a touched body");
        let (r, c) = done(w.claim(3, 77, e, 10, MODE_HOLD_L, None, t));
        assert!(!c && r.owner == 2, "can't rob a holder");
        // Holder drops it: mode change to touch bumps the version.
        let (r2, c2) = done(w.claim(2, 77, e, 10, MODE_TOUCH, None, t));
        assert!(c2 && r2.ver > r.ver && r2.mode == MODE_TOUCH);
    }

    #[test]
    fn release_stores_rest_pose_and_frees() {
        let (mut w, t) = setup(2);
        let e = w.epoch;
        done(w.claim(1, 77, e, 10, MODE_TOUCH, None, t));
        // Non-owner release is a no-op.
        let (r, c) = done(w.claim(2, 77, e, 10, MODE_FREE, None, t));
        assert!(!c && r.owner == 1);
        let rest = obj(999, [3.0, 4.0, 5.0], 0);
        let (r, c) = done(w.claim(1, 77, e, 10, MODE_FREE, Some(rest), t));
        assert!(c && r.owner == 0 && r.mode == MODE_FREE && r.ver > 0);
        let snap = w.levels[&77].cache[&10].snap;
        assert_eq!(snap.sender, 0);
        assert_eq!(snap.obj.id, 10, "rest pose re-keyed to the claimed id");
        assert!(snap.obj.flags & WF_ASLEEP != 0);
        // Anyone may claim it now.
        let (r, c) = done(w.claim(2, 77, e, 10, MODE_TOUCH, None, t));
        assert!(c && r.owner == 2);
    }

    #[test]
    fn lease_expires_without_stream_and_on_disconnect() {
        let (mut w, t) = setup(2);
        let e = w.epoch;
        done(w.claim(1, 77, e, 10, MODE_HOLD_R, None, t));
        done(w.claim(2, 77, e, 11, MODE_TOUCH, None, t));
        // Streaming renews peer 1's lease.
        let t1 = t + Duration::from_millis(2500);
        let (n, _) = w.state(1, 77, e, 1, 0, &[obj(10, [0.0, 0.0, 10.0], WF_HELD)], None, t1);
        assert_eq!(n, 1);
        let t2 = t + Duration::from_millis(3500);
        let out = w.tick(t2, &present(&[1, 2]), &HashMap::new());
        assert!(w.levels[&77].leases.contains_key(&10), "renewed lease alive");
        assert!(!w.levels[&77].leases.contains_key(&11), "silent lease expired");
        assert!(out.iter().any(|(_, b)| matches!(b, Msg::Owners { owners, .. }
            if owners.iter().any(|r| r.id == 11 && r.owner == 0))));
        // Owner disconnects -> freed on the next tick, cached pose = anchor.
        let out = w.tick(t2 + Duration::from_millis(10), &present(&[2]), &HashMap::new());
        assert!(!w.levels[&77].leases.contains_key(&10));
        assert_eq!(w.levels[&77].cache[&10].snap.sender, 0);
        assert!(out.iter().all(|(p, _)| *p == 2), "departed peer gets nothing");
    }

    #[test]
    fn expired_lease_can_be_taken_over() {
        let (mut w, t) = setup(2);
        let e = w.epoch;
        done(w.claim(1, 77, e, 10, MODE_HOLD_R, None, t));
        let late = t + LEASE + Duration::from_millis(1);
        let (r, c) = done(w.claim(2, 77, e, 10, MODE_TOUCH, None, late));
        assert!(c && r.owner == 2);
    }

    #[test]
    fn states_only_from_owner_and_free_state_is_implicit_claim() {
        let (mut w, t) = setup(2);
        let e = w.epoch;
        let (n, ch) = w.state(1, 77, e, 1, 50, &[obj(10, [1.0, 0.0, 0.0], WF_SIM)], None, t);
        assert_eq!((n, ch.len()), (1, 1));
        assert_eq!(ch[0].owner, 1);
        let (n, ch) = w.state(2, 77, e, 1, 50, &[obj(10, [2.0, 0.0, 0.0], 0)], None, t);
        assert_eq!((n, ch.len()), (0, 0), "non-owner state dropped");
        // Unknown id is dropped.
        let (n, _) = w.state(1, 77, e, 2, 100, &[obj(4242, [0.0; 3], 0)], None, t);
        assert_eq!(n, 0);
    }

    #[test]
    fn speed_and_hold_radius_sanity() {
        let (mut w, t) = setup(1);
        let e = w.epoch;
        w.state(1, 77, e, 1, 0, &[obj(10, [1.0, 0.0, 0.0], 0)], None, t);
        let t1 = t + Duration::from_millis(100);
        let (n, _) = w.state(1, 77, e, 2, 100, &[obj(10, [5000.0, 0.0, 0.0], 0)], None, t1);
        assert_eq!(n, 0, "50 m in 0.1 s is a teleport");
        let (n, _) = w.state(1, 77, e, 3, 100, &[obj(10, [800.0, 0.0, 0.0], 0)], None, t1);
        assert_eq!(n, 1, "8 m in 0.1 s is within cap+slack");
        let (n, _) = w.state(1, 77, e, 4, 150, &[obj(10, [800.0, 0.0, 0.0], WF_HELD)],
                             Some([800.0, 2000.0, 0.0]), t1);
        assert_eq!(n, 0, "held 20 m from the holder");
    }

    /// Touch hand-over on contact: the nearer pusher gets a slow body; never a fast one,
    /// never a fresh lease, never a holder's.
    #[test]
    fn touch_lease_goes_to_the_nearer_pusher_of_a_slow_body() {
        let (mut w, t) = setup(2);
        let e = w.epoch;
        let pos = |a: [f32; 3], b: [f32; 3]| -> HashMap<PeerId, [f32; 3]> { [(1, a), (2, b)].into_iter().collect() };
        // peer 1 pushes body 10 (at the origin), then walks off; peer 2 is next to it
        w.state(1, 77, e, 1, 0, &[obj(10, [5.0, 0.0, 0.0], WF_SIM)], None, t);
        w.tick(t, &present(&[1, 2]), &pos([400.0, 0.0, 0.0], [80.0, 0.0, 0.0]));
        let (r, ch) = done(w.claim(2, 77, e, 10, MODE_TOUCH, None, t + Duration::from_millis(100)));
        assert!(!ch && r.owner == 1, "a lease younger than TAKE_MIN_AGE stays");
        let t1 = t + TAKE_MIN_AGE + Duration::from_millis(10);
        w.state(1, 77, e, 2, 50, &[obj(10, [5.0, 0.0, 0.0], WF_SIM)], None, t1);
        w.tick(t1, &present(&[1, 2]), &pos([400.0, 0.0, 0.0], [80.0, 0.0, 0.0]));
        let (r, ch) = done(w.claim(2, 77, e, 10, MODE_TOUCH, None, t1));
        assert!(ch && r.owner == 2 && r.mode == MODE_TOUCH, "the nearer pusher takes it");
        // a fast body stays with its owner
        let t2 = t1 + TAKE_MIN_AGE + Duration::from_millis(10);
        let fast = WorldObj::from_parts(10, [5.0, 0.0, 0.0], [0.0; 3], [400.0, 0.0, 0.0], WF_SIM);
        w.state(2, 77, e, 3, 100, &[fast], None, t2);
        w.tick(t2, &present(&[1, 2]), &pos([60.0, 0.0, 0.0], [300.0, 0.0, 0.0]));
        let (r, ch) = done(w.claim(1, 77, e, 10, MODE_TOUCH, None, t2));
        assert!(!ch && r.owner == 2, "a body faster than TAKE_SPEED is not handed over");
        // not nearer by TAKE_MARGIN: no hand-over
        w.state(2, 77, e, 4, 150, &[obj(10, [5.0, 0.0, 0.0], WF_SIM)], None, t2);
        w.tick(t2, &present(&[1, 2]), &pos([100.0, 0.0, 0.0], [130.0, 0.0, 0.0]));
        let (r, ch) = done(w.claim(1, 77, e, 10, MODE_TOUCH, None, t2));
        assert!(!ch && r.owner == 2, "only a clearly nearer pusher");
        // a holder is never robbed by a touch
        done(w.claim(1, 77, e, 11, MODE_HOLD_R, None, t2));
        w.tick(t2 + TAKE_MIN_AGE * 2, &present(&[1, 2]), &pos([2000.0, 0.0, 0.0], [500.0, 0.0, 0.0]));
        let (r, ch) = done(w.claim(2, 77, e, 11, MODE_TOUCH, None, t2 + TAKE_MIN_AGE * 2));
        assert!(!ch && r.owner == 1, "holds are never taken by a touch");
    }

    /// A held state takes (or upgrades to) a hold lease, so a later pickup by someone else
    /// cannot take the item from its first holder ("hold beats touch").
    #[test]
    fn held_states_claim_a_hold_not_a_touch() {
        let (mut w, t) = setup(2);
        let e = w.epoch;
        let (_, ch) = w.state(1, 77, e, 1, 0, &[obj(10, [5.0, 0.0, 0.0], WF_HELD)], Some([0.0; 3]), t);
        assert_eq!((ch[0].owner, ch[0].mode), (1, MODE_HOLD_R), "implicit claim from a held state is a hold");
        let (r, chg) = done(w.claim(2, 77, e, 10, MODE_HOLD_L, None, t));
        assert!(!chg && r.owner == 1, "the second pickup loses");
        // my touch lease, then I pick it up: the first held state upgrades it
        done(w.claim(1, 77, e, 11, MODE_TOUCH, None, t));
        let (_, ch) = w.state(1, 77, e, 2, 50, &[obj(11, [500.0, 0.0, 0.0], WF_HELD | WF_LEFT)], Some([500.0, 0.0, 0.0]), t);
        assert_eq!((ch.len(), ch[0].mode), (1, MODE_HOLD_L));
        let (r, chg) = done(w.claim(2, 77, e, 11, MODE_HOLD_R, None, t));
        assert!(!chg && r.owner == 1);
    }

    /// A release fans its rest pose out with the next tick (late joiners and every follower get
    /// the new anchor at once, not a keyframe round later); a relayed packet keeps one group per
    /// sender batch, so each row carries its own sample time.
    #[test]
    fn release_anchor_goes_out_next_tick_and_groups_keep_their_ts() {
        let (mut w, t) = setup(2);
        let e = w.epoch;
        done(w.claim(1, 77, e, 10, MODE_TOUCH, None, t));
        let rest = obj(10, [30.0, 0.0, 0.0], WF_SIM);
        let (r, ch) = done(w.claim(1, 77, e, 10, MODE_FREE, Some(rest), t));
        assert!(ch && r.owner == 0);
        let out = w.tick(t, &present(&[1, 2]), &HashMap::new());
        for p in [1, 2] {
            assert!(out.iter().any(|(d, m)| *d == p && matches!(m, Msg::Snapshot { objects, .. }
                if objects.iter().any(|s| s.sender == 0 && s.obj.id == 10 && s.obj.pos[0] == 30.0))), "peer {p} gets the anchor");
        }
        // two batches of one sender in one flush: two groups, each with its own ts
        w.state(1, 77, e, 5, 100, &[obj(11, [500.0, 0.0, 0.0], WF_SIM)], None, t);
        w.state(1, 77, e, 6, 133, &[obj(12, [0.0, 900.0, 0.0], WF_SIM)], None, t);
        let out = w.tick(t + Duration::from_millis(40), &present(&[1, 2]), &HashMap::new());
        let g: Vec<(u32, u32, u32)> = out.iter().filter(|(d, _)| *d == 2).flat_map(|(_, m)| match m {
            Msg::States { groups, .. } => groups.iter().map(|g| (g.seq, g.ts, g.objects[0].id)).collect(),
            _ => Vec::new(),
        }).collect();
        assert!(g.contains(&(5, 100, 11)) && g.contains(&(6, 133, 12)), "{g:?}");
    }

    #[test]
    fn epoch_scoping_and_bump() {
        let (mut w, t) = setup(2);
        let e = w.epoch;
        done(w.claim(1, 77, e, 10, MODE_TOUCH, None, t));
        let hints = w.bump_epoch();
        assert_eq!(hints.len(), 2);
        assert!(hints.iter().all(|(_, b)| matches!(b, Msg::Owners { epoch, sync: false, .. } if *epoch == e + 1)));
        assert!(w.levels.is_empty(), "round reset drops all world state");
        assert!(matches!(w.claim(1, 77, e, 10, MODE_TOUCH, None, t), ClaimResult::Stale));
        let (n, _) = w.state(1, 77, e, 1, 0, &[obj(10, [0.0; 3], 0)], None, t);
        assert_eq!(n, 0, "stale-epoch states dropped");
        assert!(w.stale_hint(1, 77, t).is_some());
        assert!(w.stale_hint(1, 77, t).is_none(), "hint rate-limited");
        // Re-sync puts the peer in the new epoch with an empty level.
        let out = w.sync(1, 77, t);
        assert!(matches!(&out[0].1, Msg::Owners { sync: true, epoch, manifest_len: 0, .. } if *epoch == e + 1));
    }

    #[test]
    fn manifest_dedupes_near_entries_and_validates_dyn() {
        let (mut w, t) = setup(2);
        let e = w.epoch;
        // Peer 2 computed a different id for body 10 (cell straddle): folded.
        let out = w.manifest(2, 77, e, 9, vec![ent(99, 5, [30.0, 0.0, 0.0]), ent(13, 7, [0.0, 0.0, 0.0])], t);
        let reply: Vec<&WorldEntry> = out.iter().filter(|(p, b)| *p == 2 && matches!(b, Msg::Manifest { req: 9, .. }))
            .flat_map(|(_, b)| match b { Msg::Manifest { entries, .. } => entries.iter(), _ => unreachable!() }).collect();
        assert_eq!(reply.iter().map(|e| e.id).collect::<Vec<_>>(), vec![10, 13]);
        assert!(out.iter().any(|(p, b)| *p == 1 && matches!(b, Msg::Manifest { req: 0, entries, .. } if entries.len() == 1 && entries[0].id == 13)));
        assert_eq!(w.levels[&77].manifest.len(), 4);
        // Dynamic items: id namespace + owner + class enforced.
        // (Not at the origin: that is a destroyed actor, see dyn_entry_ok.)
        let good = WorldEntry { id: 0x8000_0000 | (2 << 16) | 1, chash: 1, pos: [40.0, 0.0, 0.0], dyn_owner: 2,
                                class: "/Game/Assets/Weapons/X.X_C".into(), passport: None };
        let forged = WorldEntry { id: 0x8000_0000 | (1 << 16) | 1, dyn_owner: 1, ..good.clone() };
        let noclass = WorldEntry { id: good.id + 1, class: "C:/evil".into(), ..good.clone() };
        w.manifest(2, 77, e, 10, vec![good.clone(), forged, noclass], t);
        let ids: Vec<u32> = w.levels[&77].manifest.iter().map(|e| e.id).collect();
        assert!(ids.contains(&good.id));
        assert_eq!(ids.len(), 5);
        // Dyn cap per peer.
        let many: Vec<WorldEntry> = (2..100).map(|k| WorldEntry { id: 0x8000_0000 | (2 << 16) | k, ..good.clone() }).collect();
        w.manifest(2, 77, e, 11, many, t);
        assert_eq!(w.levels[&77].dyn_count[&2], MAX_DYN_PER_PEER);
    }

    #[test]
    fn fanout_coalesces_excludes_sender_and_respects_budget() {
        let (mut w, t) = setup(3);
        let e = w.epoch;
        w.state(1, 77, e, 1, 50, &[obj(10, [1.0, 0.0, 0.0], WF_SIM)], None, t);
        w.state(2, 77, e, 1, 60, &[obj(11, [500.0, 0.0, 0.0], WF_SIM)], None, t);
        let out = w.tick(t, &present(&[1, 2, 3]), &HashMap::new());
        let states: Vec<&(PeerId, Msg)> = out.iter().filter(|(_, b)| matches!(b, Msg::States { .. })).collect();
        // Peer 3 gets one coalesced packet with two groups; 1 and 2 each get the other's.
        let to3: Vec<&Vec<WorldGroup>> = states.iter().filter(|(p, _)| *p == 3)
            .map(|(_, b)| match b { Msg::States { groups, .. } => groups, _ => unreachable!() }).collect();
        assert_eq!(to3.len(), 1);
        assert_eq!(to3[0].len(), 2);
        for (p, b) in &states {
            if let Msg::States { groups, .. } = b {
                assert!(groups.iter().all(|g| g.peer_id != *p), "no echo to sender");
            }
        }
        // Budget: flood 600 distinct bodies in one tick; a recipient gets ≤ burst.
        let mut w2 = World::new();
        w2.sync(1, 5, t); w2.sync(2, 5, t);
        let ents: Vec<WorldEntry> = (0..600).map(|i| ent(1000 + i, i, [i as f32 * 100.0, 0.0, 0.0])).collect();
        for c in ents.chunks(200) { w2.manifest(1, 5, w2.epoch, 1, c.to_vec(), t); }
        for c in (0..600u32).collect::<Vec<_>>().chunks(40) {
            let objs: Vec<WorldObj> = c.iter().map(|i| obj(1000 + i, [*i as f32 * 100.0, 0.0, 0.0], WF_HELD)).collect();
            w2.state(1, 5, w2.epoch, 1, 0, &objs, None, t);
        }
        let out = w2.tick(t, &present(&[1, 2]), &HashMap::new());
        let mut bytes = 0usize;
        for (p, b) in &out {
            if let Msg::States { .. } = b {
                assert_eq!(*p, 2);
                // The packet: one message per sender group, each + its 7 B chunk header.
                let n: usize = b.encode().iter().map(|m| m.len() + 7).sum();
                assert!(n <= MAX_BODY_BYTES, "packet {n} B too big");
                bytes += n;
            }
        }
        assert!(bytes as f64 <= BUDGET_BURST + 1.0, "sent {bytes} B over burst");
        assert!(w2.stats.objs_budget_dropped > 0);
    }

    #[test]
    fn relevance_thinning_by_distance() {
        let (mut w, t) = setup(2);
        let e = w.epoch;
        let mut pos = HashMap::new();
        pos.insert(2, [4000.0f32, 900.0, 0.0]);
        let mut got = 0;
        for k in 0..8u32 {
            w.state(1, 77, e, k, k * 50, &[obj(12, [0.0, 900.0, k as f32], WF_SIM)], None, t + Duration::from_millis(k as u64 * 50));
            let out = w.tick(t + Duration::from_millis(k as u64 * 50), &present(&[1, 2]), &pos);
            got += out.iter().filter(|(_, b)| matches!(b, Msg::States { .. })).count();
        }
        assert_eq!(got, 4, "moving body 40 m away at half rate");
        let mut got = 0;
        for k in 8..16u32 {
            w.state(1, 77, e, k, k * 50, &[obj(12, [0.0, 900.0, k as f32], WF_HELD)], None, t + Duration::from_millis(k as u64 * 50));
            let out = w.tick(t + Duration::from_millis(k as u64 * 50), &present(&[1, 2]), &pos);
            got += out.iter().filter(|(_, b)| matches!(b, Msg::States { .. })).count();
        }
        assert_eq!(got, 8, "held bodies not thinned inside 60 m");
        pos.insert(2, [100000.0f32, 0.0, 0.0]);
        let mut got = 0;
        for k in 16..24u32 {
            w.state(1, 77, e, k, k * 50, &[obj(12, [0.0, 900.0, k as f32], WF_HELD)], None, t + Duration::from_millis(k as u64 * 50));
            let out = w.tick(t + Duration::from_millis(k as u64 * 50), &present(&[1, 2]), &pos);
            got += out.iter().filter(|(_, b)| matches!(b, Msg::States { .. })).count();
        }
        assert_eq!(got, 2, "held body 1 km away at quarter rate");
    }

    #[test]
    fn keyframes_and_heal_cover_free_bodies() {
        let (mut w, t) = setup(2);
        let e = w.epoch;
        done(w.claim(1, 77, e, 10, MODE_TOUCH, None, t));
        done(w.claim(1, 77, e, 10, MODE_FREE, Some(obj(10, [7.0, 7.0, 7.0], 0)), t));
        let out = w.tick(t, &present(&[1, 2]), &HashMap::new());
        assert!(out.iter().any(|(p, b)| *p == 2 && matches!(b, Msg::Snapshot { objects, .. }
            if objects.iter().any(|s| s.obj.id == 10 && s.sender == 0))));
        assert!(out.iter().any(|(p, b)| *p == 2 && matches!(b, Msg::Owners { owners, manifest_len: 3, .. }
            if owners.iter().any(|r| r.id == 10 && r.owner == 0))));
        // Late joiner gets owners (sync), manifest and the anchor.
        let out = w.sync(9, 77, t);
        assert!(matches!(&out[0].1, Msg::Owners { sync: true, .. }));
        assert!(out.iter().any(|(_, b)| matches!(b, Msg::Manifest { entries, .. } if entries.len() == 3)));
        assert!(out.iter().any(|(_, b)| matches!(b, Msg::Snapshot { .. })));
    }

    #[test]
    fn claim_rate_limit() {
        let (mut w, t) = setup(1);
        let e = w.epoch;
        let mut limited = 0;
        for _ in 0..100 {
            if matches!(w.claim(1, 77, e, 10, MODE_TOUCH, None, t), ClaimResult::RateLimited) { limited += 1; }
        }
        assert_eq!(limited, 100 - CLAIM_BURST as usize);
    }

    #[test]
    fn dropped_passports_keep_item_identity_through_sync_and_loss() {
        use hsmp_ipc::layout::Str;
        use hsmp_ipc::schema::loadout::WeaponPass;
        use hsmp_net::net::{crypto, Conn, ConnConfig, SendMode, Side};
        let (mut w, t) = setup(2);
        let epoch = w.epoch;
        let make = |i: u32| {
            let mut passport = WeaponPass::default();
            passport.class = Str::new("@Weapons/X");
            passport.head = Str::new(&format!("@Weapons/Blade{i}"));
            passport.mass_head = i as f32 + 1.0;
            WorldEntry { id: 0x80010000 | i, chash: 1, pos: [40.0 + i as f32, 0.0, 5.0], dyn_owner: 1,
                class: "/Game/Assets/Weapons/X.X_C".into(), passport: Some(passport) }
        };
        w.manifest(1, 77, epoch, 1, vec![make(1), make(2)], t);
        // A peer syncing after both drops gets each item's distinct modules,
        // even though the original owner now holds no weapon of that class.
        let sync = w.sync(2, 77, t);
        let rows: Vec<_> = sync.iter().flat_map(|(_, m)| m.encode()).filter_map(|m| {
            let (h, body) = hsmp_ipc::wire::split(&m).ok()?;
            (h.kind == rec::K_WORLD_DYN).then(|| hsmp_ipc::record::view::<rec::DynHead>(body).unwrap().rows.to_vec())
        }).flatten().collect();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].passport.head, "@Weapons/Blade1");
        assert_eq!(rows[1].passport.head, "@Weapons/Blade2");
        assert!(rows.iter().all(|r| r.has_passport.get()));
        // A full multi-peer late-join manifest exercises the actual encrypted
        // ordered fragmentation/reassembly path with alternate packet loss.
        let entries: Vec<_> = (1..=96).map(make).collect();
        let frames = Msg::Manifest { level: 77, epoch, req: 0, entries }.encode();
        assert_eq!(frames.len(), 3);
        assert!(frames.iter().all(|f| f.len() <= hsmp_net::net::frag::MAX_MESSAGE));
        let sec = crypto::derive(&[1; 32], &[2; 32], &[3; 32]);
        let mut sender = Conn::from_handshake(Side::Server, &sec, 0, ConnConfig::default());
        let mut receiver = Conn::from_handshake(Side::Client, &sec, 0, ConnConfig::default());
        for frame in &frames { sender.send(SendMode::Ordered, frame.clone()).unwrap(); }
        let (mut got, mut n) = (Vec::new(), 0u64);
        for now in (0..60_000).step_by(5) {
            while let Some(packet) = sender.poll_transmit(now) {
                n += 1;
                if n % 2 == 0 { continue; }
                got.extend(receiver.recv(now, &packet).unwrap_or_default().into_iter().map(|d| d.data));
            }
            while let Some(ack) = receiver.poll_transmit(now) { let _ = sender.recv(now, &ack); }
            if got.len() == frames.len() { break; }
        }
        assert_eq!(got, frames, "exact passports survive fragmented delivery and loss");
        assert!(sender.is_open() && receiver.is_open());
    }

    #[test]
    fn every_outbound_message_is_a_valid_record_that_fits_its_channel() {
        let (mut w, t) = setup(2);
        let e = w.epoch;
        let ents: Vec<WorldEntry> = (0..300).map(|i| WorldEntry { id: 0x8000_0000 | (1 << 16) | i, chash: i,
            pos: [i as f32 * 200.0, 0.0, 0.0], dyn_owner: 1, class: "/Game/".to_string() + &"x".repeat(190), passport: None }).collect();
        let mut out = w.manifest(1, 77, e, 1, ents, t);
        for i in 0..200u32 { w.levels.get_mut(&77).unwrap().freed.insert(5000 + i, i); }
        out.extend(w.tick(t, &present(&[1, 2]), &HashMap::new()));
        out.extend(w.sync(2, 77, t));
        let mut states = 0;
        for (_, b) in &out {
            for m in b.encode() {
                let (h, p) = hsmp_ipc::wire::split(&m).unwrap();
                hsmp_ipc::schema::check_payload(h.kind, p).unwrap_or_else(|e| panic!("kind {:#06x}: {e}", h.kind));
                match hsmp_ipc::schema::record_info(h.kind).unwrap().chan {
                    hsmp_ipc::schema::Chan::Latest(_) => {
                        states += 1;
                        assert!(m.len() <= hsmp_net::net::channel::MAX_UNRELIABLE, "unreliable {} B", m.len());
                    }
                    _ => assert!(m.len() <= 64 * 1024, "reliable {} B", m.len()),
                }
            }
        }
        let _ = states;
    }

    // ---- server-built manifests ----

    const TEST_WORLD: &str = "World /Game/Maps/Test/Map_T.Map_T";
    static TEST_OBJS: &[StaticObj] = &[
        StaticObj { class: "StaticMeshActor", comp: "SM", name: "StaticMeshActor_3", level: "/Game/Maps/Test/Map_T.Map_T:PersistentLevel", pos: [554.0, 1409.0, 3.0], entry: true },
        StaticObj { class: "ModularWeaponBP_Spear_D_C", comp: "W", name: "ModularWeaponBP_Spear_D_C_0", level: "/Game/Maps/Test/Map_T.Map_T:PersistentLevel", pos: [529.0, 1385.0, 45.0], entry: true },
        StaticObj { class: "BP_Barrel_Destructable_Constraits_C", comp: "", name: "Barrel_1", level: "/Game/Maps/Test/Map_T.Map_T:PersistentLevel", pos: [-2000.0, 0.0, 0.0], entry: false },
    ];
    fn static_world() -> (World, u32, Instant) {
        let lvl = fnv1a(TEST_WORLD);
        let mut w = World::with_statics(build_statics(&[StaticArena { world: TEST_WORLD, objs: TEST_OBJS }]));
        let t = Instant::now();
        w.sync(1, lvl, t);
        w.sync(2, lvl, t);
        (w, lvl, t)
    }
    fn reply_ids(out: &[Out], peer: PeerId, req: u32) -> Vec<u32> {
        out.iter().filter(|(p, b)| *p == peer && matches!(b, Msg::Manifest { req: r, .. } if *r == req))
            .flat_map(|(_, b)| match b { Msg::Manifest { entries, .. } => entries.iter().map(|e| e.id).collect::<Vec<_>>(), _ => vec![] })
            .collect()
    }

    #[test]
    fn id_rule_matches_hsmpworld_in_game() {
        // Values from a real in-game .world_scan.txt (EastTower)
        // and the HSMPWorld log ("level hash 774262434").
        assert_eq!(fnv1a("World /Game/Maps/Arenas/Map_Arena_EastTower.Map_Arena_EastTower"), 774262434);
        let st = &builtin_statics()[&774262434];
        let ids: HashSet<u32> = st.entries.iter().map(|e| e.id).collect();
        assert!(ids.contains(&882811775), "StaticMeshActor_3 exact id");
        assert!(ids.contains(&339205676), "BP_Weapon_Trap child actor exact id");
        assert!(ids.contains(&781250945), "placed ModularWeaponBP_Spear_D_C_0 exact id");
        let e = st.entries.iter().find(|e| e.id == 882811775).unwrap();
        assert_eq!(e.chash, fnv1a("StaticMeshActor|SM"));
        assert!(dist(e.pos, [554.0, 1409.0, 0.0]) < 10.0, "static pose = in-game discovery pose");
        assert!(stable_name("StaticMeshActor_12") && stable_name("Chain") && stable_name("BP_X_GEN_VARIABLE_Y_CAT_1092"));
        assert!(!stable_name("ModularWeaponBP_C_2147481324") && !stable_name("Default__X"));
    }

    #[test]
    fn builtin_statics_cover_every_arena_deterministically() {
        let s = builtin_statics();
        for a in ["Alley", "Pit", "Yard", "Slums", "Cellar", "LordsHall", "EastTower", "Ambush_Test"] {
            let world = format!("World /Game/Maps/Arenas/Map_Arena_{a}.Map_Arena_{a}");
            let lv = s.get(&fnv1a(&world)).unwrap_or_else(|| panic!("{a} missing"));
            assert!(!lv.vouch.is_empty(), "{a} has static objects");
            let ids: HashSet<u32> = lv.entries.iter().map(|e| e.id).collect();
            assert_eq!(ids.len(), lv.entries.len(), "{a}: ids distinct");
            assert!(lv.entries.iter().all(|e| e.id != 0 && e.id & 0x8000_0000 == 0 && e.dyn_owner == 0),
                    "{a}: static namespace");
        }
        // Rebuilding gives identical ids (no HashMap-order or float noise).
        let again = build_statics(STATIC_ARENAS);
        for (k, v) in s.iter() {
            let a: Vec<(u32, u32)> = v.entries.iter().map(|e| (e.id, e.chash)).collect();
            let b: Vec<(u32, u32)> = again[k].entries.iter().map(|e| (e.id, e.chash)).collect();
            assert_eq!(a, b);
        }
        // Every manifest is one record message (static rows only) and fits the static cap.
        for v in s.values() {
            assert!(v.entries.len() <= MAX_STATIC);
            let m = Msg::Manifest { level: 1, epoch: 1, req: 0, entries: v.entries.clone() }.encode();
            assert_eq!(m.len(), 1);
            assert!(m[0].len() <= 8 + 16 + 24 * MAX_STATIC);
            assert_eq!(hsmp_ipc::wire::kind_of(&m[0]), rec::K_WORLD_MANIFEST);
        }
        // Colliding keys get ordinals, like Lua assign_ids.
        static DUP: &[StaticObj] = &[
            StaticObj { class: "C", comp: "SM", name: "ModularWeaponBP_C_2147480001", level: "", pos: [1.0, 2.0, 3.0], entry: true },
            StaticObj { class: "C", comp: "SM", name: "ModularWeaponBP_C_2147480002", level: "", pos: [4.0, 2.0, 3.0], entry: true },
        ];
        let d = StaticLevel::build("w", DUP);
        assert_eq!(d.entries[0].id, fnv1a("C|SM|@0,0,0") & 0x7FFF_FFFF);
        assert_eq!(d.entries[1].id, fnv1a("C|SM|@0,0,0#2") & 0x7FFF_FFFF);
    }

    #[test]
    fn server_built_manifest_is_seeded_before_any_client() {
        let (mut w, lvl, t) = static_world();
        assert!(w.levels[&lvl].server_built());
        assert_eq!(w.levels[&lvl].manifest.len(), 2);
        let out = w.sync(3, lvl, t);
        assert!(matches!(&out[0].1, Msg::Owners { sync: true, manifest_len: 2, .. }));
        assert!(out.iter().any(|(_, b)| matches!(b, Msg::Manifest { entries, .. } if entries.len() == 2)));
        // Claims on server ids work without any client proposal.
        let e = w.epoch;
        let id = w.levels[&lvl].manifest[0].id;
        let (r, c) = done(w.claim(1, lvl, e, id, MODE_TOUCH, None, t));
        assert!(c && r.owner == 1);
        // A round reset re-seeds the same ids on the next sync.
        let before: Vec<u32> = w.levels[&lvl].manifest.iter().map(|e| e.id).collect();
        w.bump_epoch();
        w.sync(1, lvl, t);
        let after: Vec<u32> = w.levels[&lvl].manifest.iter().map(|e| e.id).collect();
        assert_eq!(before, after);
        assert!(w.levels[&lvl].leases.is_empty());
    }

    #[test]
    fn client_entries_are_only_aliases_on_server_built_levels() {
        let (mut w, lvl, t) = static_world();
        let e = w.epoch;
        let sm = fnv1a("StaticMeshActor|SM");
        let spear = fnv1a("ModularWeaponBP_Spear_D_C|W");
        let canon_sm = w.levels[&lvl].manifest.iter().find(|x| x.chash == sm).unwrap().id;
        let canon_sp = w.levels[&lvl].manifest.iter().find(|x| x.chash == spear).unwrap().id;
        // Respawned weapon (runtime name -> cell id) 90 cm off: folded into
        // the canonical id (beyond the 60 cm first-client radius).
        let out = w.manifest(1, lvl, e, 7, vec![ent(4242, spear, [529.0, 1475.0, 45.0]), ent(4243, sm, [560.0, 1400.0, 3.0])], t);
        assert_eq!(reply_ids(&out, 1, 7), vec![canon_sp, canon_sm]);
        assert_eq!(w.levels[&lvl].manifest.len(), 2, "aliases add nothing");
        assert_eq!(w.stats.manifest_aliased, 2);
        assert!(!out.iter().any(|(p, _)| *p == 2), "nothing new to broadcast");
        // Same class far from the static pose, or an unknown body nowhere
        // near any static object: refused, but the request is still acked.
        let out = w.manifest(1, lvl, e, 8, vec![ent(5000, spear, [5000.0, 5000.0, 0.0]), ent(5001, 99, [9000.0, 0.0, 0.0])], t);
        assert!(reply_ids(&out, 1, 8).is_empty());
        assert!(out.iter().any(|(p, b)| *p == 1 && matches!(b, Msg::Manifest { req: 8, entries, .. } if entries.is_empty())),
                "empty ack");
        assert_eq!(w.levels[&lvl].manifest.len(), 2);
        assert_eq!(w.stats.manifest_rejected, 2);
        // A multi-body piece next to a static barrel (position not known
        // statically) is vouched: first proposal wins, later ones alias it.
        let stave = fnv1a("BP_Barrel_Destructable_Constraits_C|Stave3");
        let out = w.manifest(1, lvl, e, 9, vec![ent(6000, stave, [-1980.0, 30.0, 40.0])], t);
        assert_eq!(reply_ids(&out, 1, 9), vec![6000]);
        assert!(out.iter().any(|(p, b)| *p == 2 && matches!(b, Msg::Manifest { req: 0, entries, .. } if entries[0].id == 6000)));
        let out = w.manifest(2, lvl, e, 3, vec![ent(6001, stave, [-1990.0, 40.0, 40.0])], t);
        assert_eq!(reply_ids(&out, 2, 3), vec![6000]);
        assert_eq!(w.levels[&lvl].manifest.len(), 3);
        // Dynamic items keep working on server-built levels.
        let dynok = WorldEntry { id: 0x8000_0000 | (1 << 16) | 1, chash: 1, pos: [10.0, 0.0, 0.0], dyn_owner: 1,
                                 class: "/Game/Assets/Weapons/X.X_C".into(), passport: None };
        let out = w.manifest(1, lvl, e, 10, vec![dynok.clone()], t);
        assert_eq!(reply_ids(&out, 1, 10), vec![dynok.id]);
    }

    #[test]
    fn origin_and_out_of_arena_poses_never_become_states_or_anchors() {
        // Regression (seen in game): a destroyed kit weapon was
        // streamed and released "at rest" at (0,0,0); every client then
        // anchored the item at the origin.
        let (mut w, t) = setup(2);
        let e = w.epoch;
        let (n, ch) = w.state(1, 77, e, 1, 0, &[obj(10, [0.3, -0.2, 0.0], WF_SIM)], None, t);
        assert_eq!((n, ch.len()), (0, 0), "origin state rejected (and is no implicit claim)");
        done(w.claim(1, 77, e, 11, MODE_TOUCH, None, t));
        let (r, c) = done(w.claim(1, 77, e, 11, MODE_FREE, Some(obj(11, [0.0; 3], 0)), t));
        assert!(c && r.owner == 0, "released");
        assert!(!w.levels[&77].cache.contains_key(&11), "origin rest pose is not an anchor");
        // Server-built level: the arena box (static objects + margins).
        let (mut w, lvl, t) = static_world();
        let e = w.epoch;
        let id = w.levels[&lvl].manifest[0].id;
        let (n, _) = w.state(1, lvl, e, 1, 0, &[obj(id, [554.0, 1409.0, 3.0], WF_SIM)], None, t);
        assert_eq!(n, 1, "inside the arena");
        let t1 = t + Duration::from_secs(60);
        let (n, _) = w.state(1, lvl, e, 2, 0, &[obj(id, [554.0, 1409.0, -9000.0], WF_SIM)], None, t1);
        assert_eq!(n, 0, "fell out of the world: rejected");
        let b = w.levels[&lvl].stat.as_ref().unwrap().bounds.unwrap();
        assert!(b.0[0] <= -2000.0 - BOUNDS_MARGIN_XY + 0.1 && b.1[0] >= 554.0 + BOUNDS_MARGIN_XY - 0.1);
    }

    /// The generated table must match docs/arena_static. Regenerate with
    /// `HSMP_REGEN_WORLD_STATIC=1 cargo test static_table` (skipped where
    /// docs/ isn't present, e.g. the Docker build context).
    #[test]
    fn static_table_is_generated_from_arena_static() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
        let src = root.join("docs").join("arena_static");
        if !src.exists() {
            eprintln!("skip: {} not present", src.display());
            return;
        }
        let text = gen_static_table(&src);
        let out = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src").join("world_static_table.rs");
        if std::env::var_os("HSMP_REGEN_WORLD_STATIC").is_some() {
            std::fs::write(&out, &text).unwrap();
            return;
        }
        let cur = std::fs::read_to_string(&out).unwrap_or_default().replace("\r\n", "\n");
        assert!(cur == text, "world_static_table.rs is stale: run HSMP_REGEN_WORLD_STATIC=1 cargo test static_table");
    }

    /// Generator of world_static_table.rs. Per arena, the bodies
    /// HSMPWorld discovers in-game:
    ///   entry=true  canonical manifest entry, body = actor root, so its pose
    ///               is known statically: simulating StaticMeshActors ("SM")
    ///               and every ModularWeaponBP_C subclass ("W": placed weapons;
    ///               survivors of Clean Up Map are re-spawned at the same
    ///               transform with runtime names and bind by alias);
    ///   entry=false vouch point only (multi-body / BP props whose body poses
    ///               aren't known statically: barrels, planks, fences, chains,
    ///               chests, candles, traps ...).
    fn gen_static_table(src: &std::path::Path) -> String {
        fn game_path(pkg: &str) -> Option<String> {
            let p = pkg.replace('\\', "/");
            let i = p.find("/Content/")?;
            let rest = &p[i + "/Content/".len()..];
            Some(format!("/Game/{}", rest.rsplit_once('.').map_or(rest, |(a, _)| a)))
        }
        fn leaf(gp: &str) -> &str { gp.rsplit('/').next().unwrap_or(gp) }
        fn rs(s: &str) -> String { format!("{:?}", s) }
        fn r1(v: f64) -> String {
            let x = (v * 10.0).round() / 10.0;
            let x = if x == 0.0 { 0.0 } else { x };
            format!("{:?}", x)
        }
        let mut files: Vec<std::path::PathBuf> = std::fs::read_dir(src).unwrap()
            .filter_map(|e| e.ok().map(|e| e.path()))
            .filter(|p| p.file_name().and_then(|n| n.to_str())
                .map_or(false, |n| n.starts_with("Map_Arena_") && n.ends_with(".json")))
            .collect();
        files.sort();
        files.push(src.join("all_maps").join("Maps__Arenas__Map_Arena_Ambush_Test.json"));
        let mut out = String::from(
            "// @generated from docs/arena_static by world.rs tests::gen_static_table. DO NOT EDIT.\n\
             // Regenerate: HSMP_REGEN_WORLD_STATIC=1 cargo test static_table\n\n\
             pub static STATIC_ARENAS: &[StaticArena] = &[\n");
        for f in files {
            let d: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&f).unwrap()).unwrap();
            let pkg = d["package"].as_str().unwrap();
            let gp = game_path(pkg).unwrap();
            let world = format!("World {}.{}", gp, leaf(&gp));
            let mut rows = Vec::new();
            for o in d["world_objects"].as_array().unwrap() {
                let cat = o["category"].as_str().unwrap_or("");
                if cat == "character" { continue; }
                let cls = o["class"].as_str().unwrap_or("");
                let chain: Vec<&str> = o["class_chain"].as_array().map(|a| a.iter().filter_map(|v| v.as_str()).collect())
                    .unwrap_or_default();
                let sim = o["simulate_physics"].as_bool() == Some(true);
                let (comp, entry) = if cls == "StaticMeshActor" {
                    if !sim { continue; }
                    ("SM", true)
                } else if chain.contains(&"ModularWeaponBP_C") {
                    ("W", true)
                } else {
                    ("", false)
                };
                let via = o["via"].as_str().unwrap_or("persistent");
                let lp = if via.starts_with("level_instance") { String::new() } else {
                    game_path(o["source"].as_str().unwrap_or(pkg))
                        .map_or(String::new(), |g| format!("{}.{}:PersistentLevel", g, leaf(&g)))
                };
                let loc: Vec<f64> = o["location"].as_array().unwrap().iter().map(|v| v.as_f64().unwrap()).collect();
                rows.push((cls.to_string(), comp, o["name"].as_str().unwrap_or("").to_string(), lp, loc, entry, cat.to_string()));
            }
            rows.sort_by(|a, b| (&a.0, a.1, &a.3, &a.2).cmp(&(&b.0, b.1, &b.3, &b.2))
                .then(a.4[0].total_cmp(&b.4[0])).then(a.4[1].total_cmp(&b.4[1])).then(a.4[2].total_cmp(&b.4[2])));
            out.push_str(&format!("    StaticArena {{ world: {}, objs: &[\n", rs(&world)));
            for (cls, comp, name, lp, loc, entry, cat) in rows {
                out.push_str(&format!(
                    "        StaticObj {{ class: {}, comp: {}, name: {}, level: {}, pos: [{}, {}, {}], entry: {} }},  // {}\n",
                    rs(&cls), rs(comp), rs(&name), rs(&lp), r1(loc[0]), r1(loc[1]), r1(loc[2]), entry, cat));
            }
            out.push_str("    ] },\n");
        }
        out.push_str("];\n");
        out
    }

    #[test]
    fn dynamic_items_reject_pawns_origin_and_bad_paths() {
        let (mut w, t) = setup(1);
        let e = w.epoch;
        let good = WorldEntry { id: 0x8000_0000 | (1 << 16) | 1, chash: 1, pos: [100.0, 0.0, 5.0], dyn_owner: 1,
                                class: "/Game/Assets/Weapons/Blueprints/Built_Weapons/ModularWeaponBP_LongSword_T3.ModularWeaponBP_LongSword_T3_C".into(), passport: None };
        let pawn = WorldEntry { id: good.id + 1, class: "/Game/Character/Blueprints/Willie_BP.Willie_BP_C".into(), ..good.clone() };
        let origin = WorldEntry { id: good.id + 2, pos: [0.2, -0.3, 0.0], ..good.clone() };
        let notclass = WorldEntry { id: good.id + 3, class: "/Game/Assets/Weapons/X".into(), ..good.clone() };
        let spaced = WorldEntry { id: good.id + 4, class: "/Game/A B.A B_C".into(), ..good.clone() };
        let out = w.manifest(1, 77, e, 5, vec![good.clone(), pawn, origin, notclass, spaced], t);
        assert_eq!(reply_ids(&out, 1, 5), vec![good.id]);
        assert_eq!(w.stats.manifest_rejected, 4);
        assert!(dyn_entry_ok(&good));
    }

    // ---- identical worlds ----

    fn wobj(id: u32, pos: [f32; 3], bits: i16) -> WorldObj {
        let mut o = obj(id, pos, 0);
        o.vel = [bits, 0, 0];
        o
    }
    fn snaps_to(out: &[Out], peer: PeerId) -> Vec<WorldSnap> {
        out.iter().filter(|(p, _)| *p == peer)
            .flat_map(|(_, b)| match b { Msg::Snapshot { objects, .. } => objects.clone(), _ => vec![] })
            .collect()
    }

    /// Golden vectors shared with tools/hsmp-tools/lua-tests/world_state.lua
    /// (HSMPWorld's Lua codec must produce the same numbers).
    #[test]
    fn consistency_codec_golden_vectors() {
        let r2 = std::f32::consts::FRAC_1_SQRT_2;
        assert_eq!(pack_quat([0.0, 0.0, 0.0, 1.0]), 3 << 30 | 512 << 20 | 512 << 10 | 512);
        assert_eq!(pack_quat([0.0, 0.0, r2, r2]), 2 << 30 | 512 << 20 | 512 << 10 | 1023, "yaw 90: a tie keeps the first largest (z)");
        let n = (0.01f32 + 0.04 + 0.09 + 0.927 * 0.927).sqrt();
        let q = [0.1f32 / n, -0.2 / n, 0.3 / n, -0.927 / n];
        let p = pack_quat(q);
        assert_eq!(p >> 30, 3, "dropped = w");
        let u = unpack_quat(p);
        assert!(quat_angle_deg(q, u) < 0.3, "10-bit smallest-three ~0.16 deg/component");
        assert!(quat_angle_deg(q, q.map(|v| -v)) < 1e-3, "double cover");
        assert_eq!(pack_quat([f32::NAN, 0.0, 0.0, 0.0]), pack_quat([0.0, 0.0, 0.0, 1.0]), "garbage -> identity");
        assert_eq!(body_hash(10, [1.5, -2.4, 3.0], 7, 3), GOLD_BODY_HASH);
        let bodies = [HashBody { id: 10, ver: 1, status: 3, pos: [1.5, -2.4, 3.0], q: 7 },
                      HashBody { id: 11, ver: 0, status: 0, pos: [0.0; 3], q: 0 }];
        assert_eq!(world_hash(&bodies), GOLD_WORLD_HASH);
        assert_eq!(world_hash(&[]), 2166136261);
        assert_eq!(merge_state(0b0000_0101, 0b0100_0010), 0b0100_0111, "breaks sticky, flag set");
        assert_eq!(merge_state(0b0100_0111, 0b0000_0000), 0b0000_0111, "flag cleared, breaks stay");
    }
    const GOLD_BODY_HASH: u32 = 1760769165;
    const GOLD_WORLD_HASH: u32 = 3136834322;

    fn hb(id: u32, ver: u32, status: u8, pos: [f32; 3], yaw: f32) -> HashBody {
        let q = hsmp_ipc::schema::world::rot_to_quat(0.0, yaw, 0.0);
        HashBody { id, ver, status, pos, q: pack_quat(q) }
    }

    #[test]
    fn compare_reports_finds_the_differing_ids() {
        const S: u8 = HS_ALIVE | HS_SETTLED;
        let a = vec![hb(1, 0, S, [0.0; 3], 0.0), hb(2, 3, S, [100.0, 0.0, 0.0], 90.0), hb(3, 0, S, [200.0, 0.0, 0.0], 0.0),
                     hb(4, 0, S, [300.0, 0.0, 0.0], 0.0), hb(5, 1, S, [0.0; 3], 0.0), hb(6, 0, HS_ALIVE, [9.0; 3], 0.0),
                     HashBody { id: 7, ver: 2, status: S | HS_STATE, pos: [5.0; 3], q: 0b0000_0001 },
                     hb(8, 0, S, [800.0, 0.0, 0.0], 0.0), hb(9, 0, 0, [0.0; 3], 0.0)];
        let mut b = a.clone();
        let same = compare_reports(&a, &b);
        assert_eq!((same.compared, same.mismatched.len()), (7, 0), "1,2,3,4,5,7,8 (6 busy, 9 missing on both)");
        assert_eq!(same.hash_a, same.hash_b);
        b[1].pos[0] += 2.0;                       // 2 cm: inside tolerance, but the quantised hash differs
        b[2].pos[2] += 40.0;                      // door/prop 40 cm off: pose mismatch
        b[3] = hb(4, 0, S, [300.0, 0.0, 0.0], 25.0); // hinge 25 deg further open: pose mismatch
        b[4].ver = 2;                             // other record version: not comparable
        b[5].status = S;                          // moving on one side: not comparable
        b[6].q = 0b0000_0011;                     // another constraint broken there: state mismatch
        b[7].status = 0;                          // destroyed / missing there: presence mismatch
        let c = compare_reports(&a, &b);
        let ids: Vec<(u32, u8)> = c.mismatched.iter().map(|m| (m.id, m.kind)).collect();
        assert_eq!(ids, vec![(3, MM_POSE), (4, MM_POSE), (7, MM_STATE), (8, MM_PRESENCE)]);
        assert!((c.mismatched[0].dpos - 40.0).abs() < 0.01);
        assert!((c.mismatched[1].dang - 25.0).abs() < 0.5, "hinge angle difference {}", c.mismatched[1].dang);
        assert_eq!(c.compared, 6, "1,2,3,4,7,8 (5 other version, 6 busy, 9 missing on both)");
        assert_ne!(c.hash_a, c.hash_b);
        // Within tolerance only: no mismatch, hashes still differ (quantisation).
        let mut d = a.clone();
        d[1].pos[0] += 2.0;
        let c = compare_reports(&a, &d);
        assert!(c.mismatched.is_empty() && c.hash_a != c.hash_b);
    }

    #[test]
    fn init_authority_anchors_one_clients_settled_world() {
        let (mut w, t) = setup(3);
        let e = w.epoch;
        let rest = |x: f32| Some(obj(0, [x, 1.0, 2.0], WF_SIM));
        assert!(matches!(w.claim(1, 77, e, 10, CLAIM_INIT, rest(5.0), t), ClaimResult::Anchored));
        assert_eq!(w.levels[&77].init_by, Some(1));
        // Another peer's proposals are refused while the authority is in the level.
        assert!(matches!(w.claim(2, 77, e, 11, CLAIM_INIT, rest(6.0), t), ClaimResult::Ignored));
        // The authority can't re-anchor an anchored body.
        assert!(matches!(w.claim(1, 77, e, 10, CLAIM_INIT, rest(9.0), t), ClaimResult::Ignored));
        // Not rate-limited: a full arena's worth in one burst.
        for _ in 0..60 { w.claim(1, 77, e, 12, CLAIM_INIT, rest(7.0), t); }
        let out = w.tick(t, &present(&[1, 2, 3]), &HashMap::new());
        for p in [1, 2, 3] {
            let s = snaps_to(&out, p);
            assert!(s.iter().any(|s| s.obj.id == 10 && s.sender == 0 && s.obj.flags & WF_ASLEEP != 0 && s.obj.pos[0] == 5.0),
                    "peer {p} gets the initial anchor (the proposer too)");
            assert!(s.iter().any(|s| s.obj.id == 12) && !s.iter().any(|s| s.obj.id == 11));
        }
        // A leased body is never re-anchored.
        done(w.claim(2, 77, e, 11, MODE_TOUCH, None, t));
        // Authority leaves mid-init: the next proposer completes the rest.
        w.tick(t, &present(&[2, 3]), &HashMap::new());
        assert_eq!(w.levels[&77].init_by, None);
        assert!(matches!(w.claim(3, 77, e, 11, CLAIM_INIT, rest(1.0), t), ClaimResult::Ignored), "owned by 2");
        done(w.claim(2, 77, e, 11, MODE_FREE, None, t));
        assert!(matches!(w.claim(3, 77, e, 11, CLAIM_INIT, rest(1.0), t), ClaimResult::Anchored));
        // Late joiner: the anchors arrive with the sync.
        let out = w.sync(9, 77, t);
        assert!(out.iter().any(|(_, b)| matches!(b, Msg::Snapshot { objects, .. } if objects.iter().any(|s| s.obj.id == 10))));
        // Origin proposals never become anchors; a new epoch starts over.
        w.bump_epoch();
        let e2 = w.epoch;
        for p in [1, 2] { w.sync(p, 77, t); }
        w.manifest(1, 77, e2, 1, vec![ent(10, 5, [0.0, 0.0, 0.0])], t);
        assert!(matches!(w.claim(2, 77, e2, 10, CLAIM_INIT, Some(obj(0, [0.1, 0.0, 0.0], 0)), t), ClaimResult::Ignored));
        assert!(matches!(w.claim(2, 77, e2, 10, CLAIM_INIT, rest(3.0), t), ClaimResult::Anchored));
        assert_eq!(w.levels[&77].init_by, Some(2));
    }

    #[test]
    fn actor_states_are_server_ordered_sticky_and_synced() {
        let (mut w, t) = setup(2);
        let e = w.epoch;
        // id 12 is the state id of a lever-gated portcullis (bit 0 = its constraint).
        let (r, c) = done(w.claim(1, 77, e, 12, CLAIM_STATE, Some(wobj(0, [1.0; 3], 0b0000_0001)), t));
        assert!(c && r.mode == MODE_STATE | 1 && r.owner == 1);
        let out = w.owners_to_level(77, &[r]);
        assert_eq!(out.len(), 2, "broadcast to both peers");
        // A stale peer reporting "intact" can't un-break it; the flag toggles.
        let (r2, c2) = done(w.claim(2, 77, e, 12, CLAIM_STATE, Some(wobj(0, [1.0; 3], ST_FLAG as i16)), t));
        assert!(c2 && r2.mode == MODE_STATE | ST_FLAG | 1 && r2.ver > r.ver);
        let (_, c3) = done(w.claim(2, 77, e, 12, CLAIM_STATE, Some(wobj(0, [1.0; 3], (ST_FLAG | 1) as i16)), t));
        assert!(!c3, "no change, no broadcast");
        // An all-zero first report creates no record.
        let (_, c4) = done(w.claim(1, 77, e, 11, CLAIM_STATE, Some(wobj(0, [1.0; 3], 0)), t));
        assert!(!c4 && !w.levels[&77].states.contains_key(&11));
        // Kinds don't mix: no lease on a state id, no state on a body id.
        assert!(matches!(w.claim(1, 77, e, 12, MODE_TOUCH, None, t), ClaimResult::Ignored));
        let (n, _) = w.state(1, 77, e, 1, 0, &[obj(12, [1.0, 0.0, 0.0], 0)], None, t);
        assert_eq!(n, 0, "a state id is never streamed as a body");
        done(w.claim(1, 77, e, 10, MODE_TOUCH, None, t));
        assert!(matches!(w.claim(1, 77, e, 10, CLAIM_STATE, Some(wobj(0, [0.0; 3], 1)), t), ClaimResult::Ignored));
        // Late joiner gets it with the owner table; heal rotates it too.
        let out = w.sync(9, 77, t);
        assert!(out.iter().any(|(_, b)| matches!(b, Msg::Owners { sync: true, owners, .. }
            if owners.iter().any(|o| o.id == 12 && o.mode == MODE_STATE | ST_FLAG | 1))));
        // Round reset: everything intact again.
        w.bump_epoch();
        w.sync(1, 77, t);
        assert!(w.levels[&77].states.is_empty());
    }

    fn report(w: &mut World, peer: PeerId, seq: u32, bodies: &[HashBody], t: Instant) -> Vec<Out> {
        let mut sorted = bodies.to_vec();
        sorted.sort_by_key(|b| b.id);
        let own: Vec<HashBody> = sorted.iter().copied().filter(|b| b.comparable() || b.missing()).collect();
        let rows: Vec<rec::HashRow> = bodies.iter().map(HashBody::row).collect();
        let e = w.epoch;
        w.hash_report(peer, 77, e, seq, world_hash(&own), &rows, t)
    }
    fn verdict_to(out: &[Out], peer: PeerId) -> Option<(rec::VerdictHead, Vec<Mismatch>)> {
        out.iter().find_map(|(p, b)| match b {
            Msg::Verdict(h, rows) if *p == peer => Some((*h, rows.clone())),
            _ => None,
        })
    }

    #[test]
    fn hash_reports_produce_verdicts_and_heal_anchors() {
        const S: u8 = HS_ALIVE | HS_SETTLED;
        let (mut w, t) = setup(2);
        let e = w.epoch;
        assert!(matches!(w.claim(1, 77, e, 10, CLAIM_INIT, Some(obj(0, [0.0, 0.0, 5.0], WF_SIM)), t), ClaimResult::Anchored));
        let many: Vec<HashBody> = (0..80).map(|i| hb(1000 + i, 0, S, [i as f32 * 10.0, 5.0, 5.0], 0.0)).collect();
        let mut a = many.clone();
        a.push(hb(10, 0, S, [0.0, 0.0, 5.0], 0.0));
        // First report: nobody to compare with yet, no verdict, nothing in the manifest.
        let out = report(&mut w, 1, 1, &a, t);
        assert!(verdict_to(&out, 1).is_none());
        assert_eq!(w.levels[&77].manifest.len(), 3, "reports never extend the manifest");
        assert_eq!(w.levels[&77].hash_latest[&1].bodies.len(), 81, "one message carries the whole report");
        // Peer 2: same world -> hash_match, exact hashes.
        let out = report(&mut w, 2, 1, &a, t);
        let (h, rows) = verdict_to(&out, 2).unwrap();
        assert!(h.hash_match.get() && h.hash_equal.get() && rows.is_empty());
        assert_eq!((h.seq, h.other, h.compared, h.mismatched_n), (1, 1, 81, 0));
        let vd = w.verdicts.pop().unwrap();
        assert!(vd.hash_match() && vd.hash_equal && vd.codec_ok && vd.peer == 2 && vd.other == 1);
        // Peer 2 again, body 10 knocked 30 cm off its anchor on its screen.
        let mut b = a.clone();
        b.last_mut().unwrap().pos[0] = 30.0;
        let out = report(&mut w, 2, 2, &b, t);
        let (h, rows) = verdict_to(&out, 2).unwrap();
        assert!(!h.hash_match.get(), "hash_match false");
        assert_eq!((rows[0].id, rows[0].kind), (10, MM_POSE));
        assert!((rows[0].dpos - 30.0).abs() < 0.01);
        for p in [1, 2] {
            assert!(snaps_to(&out, p).iter().any(|s| s.obj.id == 10 && s.sender == 0 && s.obj.pos[2] == 5.0),
                    "anchor of the mismatched free body re-sent to peer {p}");
        }
        let vd = w.verdicts.pop().unwrap();
        assert_eq!(vd.mismatched.iter().map(|m| m.id).collect::<Vec<_>>(), vec![10]);
        // A stale report (other epoch) gets a hint, not a verdict.
        let rows_a: Vec<rec::HashRow> = a.iter().map(HashBody::row).collect();
        let out = w.hash_report(1, 77, e + 5, 3, 0, &rows_a, t);
        assert!(verdict_to(&out, 1).is_none());
        // Reports older than HASH_MAX_AGE aren't compared.
        let late = t + HASH_MAX_AGE + Duration::from_secs(1);
        let out = report(&mut w, 1, 4, &a, late);
        assert!(verdict_to(&out, 1).is_none(), "peer 2's report is too old");
        let out = report(&mut w, 2, 5, &a, late + Duration::from_secs(1));
        assert!(verdict_to(&out, 2).is_some(), "peer 1's report is 1 s old");
        // Many mismatches: VERDICT_MAX rows, the head carries the full count; one valid record.
        let worse: Vec<HashBody> = a.iter().map(|x| HashBody { pos: [x.pos[0] + 100.0, x.pos[1], x.pos[2]], ..*x }).collect();
        let t3 = late + HASH_MAX_AGE * 2 + Duration::from_secs(5);
        report(&mut w, 1, 6, &a, t3);
        let out = report(&mut w, 2, 6, &worse, t3);
        let (h, rows) = verdict_to(&out, 2).unwrap();
        assert_eq!(rows.len(), VERDICT_MAX);
        assert_eq!(h.mismatched_n as usize, 81, "the head carries the full count");
        let m = Msg::Verdict(h, rows).encode();
        assert_eq!(m.len(), 1);
        let (wh, p) = hsmp_ipc::wire::split(&m[0]).unwrap();
        assert_eq!(wh.kind, rec::K_WORLD_VERDICT);
        assert!(hsmp_ipc::schema::check_payload(wh.kind, p).is_ok() && m[0].len() <= 8 + 32 + 16 * VERDICT_MAX);
    }

    /// Bytes per second of the state fan-out, record format vs the protocol-v5 bincode
    /// bodies it replaced (`S2CWorldState { level, epoch, groups }`: 4 B variant tag + 4 + 4 +
    /// 8 B group count + per group 4 + 4 + 4 + 8 B object count + 29 B per object, in an
    /// 8 B kind-0 envelope). Printed for comparison; asserts the per-message rules.
    #[test]
    fn fanout_bytes_vs_legacy() {
        fn legacy_len(groups: &[WorldGroup]) -> usize {
            8 + 4 + 4 + 4 + 8 + groups.iter().map(|g| 20 + 29 * g.objects.len()).sum::<usize>()
        }
        let o = |id| obj(id, [100.0, 0.0, 0.0], WF_SIM);
        // (senders, objects per sender) per 30 Hz flush to one recipient.
        for (senders, per) in [(1usize, 4usize), (1, 10), (2, 10), (3, 10), (4, 7)] {
            let groups: Vec<WorldGroup> = (0..senders as u32)
                .map(|s| WorldGroup { peer_id: s + 1, seq: 1, ts: 1, objects: (0..per as u32).map(|i| o(100 + i)).collect() })
                .collect();
            let m = Msg::States { level: 1, epoch: 1, groups: groups.clone() }.encode();
            // Each message also costs a 7 B unreliable chunk header (1 legacy message).
            let new: usize = m.iter().map(|x| x.len() + 7).sum();
            let old = legacy_len(&groups) + 7;
            eprintln!("world fan-out {senders} sender(s) x {per} objs: record {new} B, legacy {old} B; at 30 Hz {:.1} vs {:.1} KiB/s",
                      new as f64 * 30.0 / 1024.0, old as f64 * 30.0 / 1024.0);
            assert_eq!(m.len(), senders);
            assert_eq!(new, senders * GROUP_BYTES + senders * per * OBJ_BYTES);
        }
    }
}
