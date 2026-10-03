//! Server-authoritative spawn assignment (docs/development/subsystems/spawns.md).
//!
//! Map data (`server/data/maps/<Map>.json`, compiled in) is generated from
//! `docs/arena_static` by `hsmp-tools gen-map-data` (tools/hsmp-tools), together with the Lua copy
//! `mods/shared/hsmp_arenas.lua` (equality is a cargo test). Only
//! spawners marked `valid` (open, enabled for a play mode, clear of barriers)
//! are used, then derived `overflow` points for arenas with fewer points than
//! players. The server gives every player a DISTINCT absolute spawn order
//! `{spawn_id, pos, yaw, protect_ms}` per round:
//!
//!   * deterministic: the result depends only on (arena, round, seats); the
//!     seat order passed in does not matter;
//!   * minimum spacing (idle pawns too close knock each other down): any two spawns
//!     of a round are at least `min_sep_cm(n)` apart, 4 m for 2-4 players and
//!     3 m for 5-8, whenever the arena's points allow it (`capacity()`; every
//!     arena allows it for 2-4, `gen-map-data --check` enforces the data side);
//!   * maximum separation: the chosen subset maximises the smallest pairwise
//!     distance (2..8 players), preferring subsets no wider than
//!     `MAX_SPREAD_CM` so a duel on a long map (Alley, 45 m) does not start
//!     with a minute of walking; real points before derived ones unless only
//!     derived ones meet the spacing;
//!   * players rotate over the chosen points every round (they swap sides);
//!   * team-aware: with teams, each team gets one contiguous side of the
//!     chosen set (sides swap every round);
//!   * facing: everyone faces the centroid of the other players' spawns
//!     (two players face each other), or the arena centre when alone.
//!
//! The plan rides in every `S2CMatchState` (resent, idempotent), so every
//! client knows every spawn. Clients place the pawn only on this command.

use crate::proto::PeerId;
use serde::Deserialize;

/// One player's spawn order for a round (docs/development/subsystems/spawns.md).
/// Absolute: `pos` is the map-data point (floor level, cm), which the client ground-snaps and
/// clearance-checks. `spawn_id` is unique per (round, seat): round << 8 | index. `slot`
/// indexes the arena's candidate list (valid spawners, then overflow).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SpawnAssign {
    pub peer_id: PeerId,
    pub spawn_id: u32,
    pub slot: u8,
    pub pos: [f32; 3],
    pub yaw: f32,
    pub protect_ms: u16,
}
use std::sync::OnceLock;

#[derive(Debug, Deserialize)]
pub struct SpawnPoint {
    #[serde(rename = "id")]
    pub src: String,
    pub pos: [f32; 3],
    #[serde(default)]
    pub yaw: f32,
    #[serde(default)]
    pub team: u8,
    #[serde(default = "yes")]
    pub valid: bool,
    #[serde(skip)]
    pub derived: bool,
}

fn yes() -> bool { true }

#[derive(Debug, Deserialize)]
pub struct ArenaSpawns {
    #[serde(rename = "map")]
    pub arena: String,
    pub centre: [f32; 3],
    /// Below this a pawn has fallen out of the map (respawn modes).
    #[allow(dead_code)]
    pub kill_z: f32,
    /// Every spawner of the map (valid or not), as generated.
    #[serde(rename = "spawns")]
    pub all: Vec<SpawnPoint>,
    pub overflow: Vec<SpawnPoint>,
    /// The valid spawners, in file order (filled after parsing).
    #[serde(skip)]
    pub points: Vec<usize>,
    /// Most players whose spawns meet `min_sep_cm` on this arena (2..=8),
    /// computed after parsing (the map file's `max_players` is the generator's
    /// copy of the same rule; a test keeps both equal).
    #[serde(skip)]
    pub capacity: usize,
    #[serde(default)]
    #[allow(dead_code)] // read by the tests (equals `capacity`)
    pub max_players: usize,
}

/// Generated map data, compiled into the binary (no runtime file lookup).
pub const MAP_DATA: [(&str, &str); 7] = [
    ("Map_Arena_Alley", include_str!("../data/maps/Map_Arena_Alley.json")),
    ("Map_Arena_Pit", include_str!("../data/maps/Map_Arena_Pit.json")),
    ("Map_Arena_Yard", include_str!("../data/maps/Map_Arena_Yard.json")),
    ("Map_Arena_Slums", include_str!("../data/maps/Map_Arena_Slums.json")),
    ("Map_Arena_Cellar", include_str!("../data/maps/Map_Arena_Cellar.json")),
    ("Map_Arena_LordsHall", include_str!("../data/maps/Map_Arena_LordsHall.json")),
    ("Map_Arena_EastTower", include_str!("../data/maps/Map_Arena_EastTower.json")),
];

fn tables() -> &'static [ArenaSpawns] {
    static T: OnceLock<Vec<ArenaSpawns>> = OnceLock::new();
    T.get_or_init(|| {
        MAP_DATA.iter().map(|(name, json)| {
            let mut a: ArenaSpawns = serde_json::from_str(json)
                .unwrap_or_else(|e| panic!("server/data/maps/{name}.json: {e}"));
            assert_eq!(a.arena, *name, "map data file/name mismatch");
            a.points = (0..a.all.len()).filter(|&i| a.all[i].valid).collect();
            for o in &mut a.overflow { o.derived = true; }
            a.capacity = compute_capacity(&a);
            a
        }).collect()
    })
}

/// Preferred maximum distance between any two spawns of one round.
pub const MAX_SPREAD_CM: f32 = 1600.0;
/// Most players the spacing rule is defined for (beyond: offset rings).
pub const MAX_RULE_PLAYERS: usize = 8;

/// Minimum distance between any two spawns of a round with `n` players
/// (idle pawns 1.5 m apart knocked each other down before the
/// round started, and Half Sword drops held weapons on a knockdown).
pub fn min_sep_cm(n: usize) -> f32 {
    match n {
        0 | 1 => 0.0,
        2..=4 => 400.0,
        5..=MAX_RULE_PLAYERS => 300.0,
        _ => 0.0,
    }
}

/// Most players (2..=8) whose spawns can all meet `min_sep_cm` on `arena`
/// (0 for an unknown arena). Modes / the lobby can use it to warn or refuse.
#[allow(dead_code)] // for the lobby / modes (SESS): warn or refuse above it
pub fn capacity(arena: &str) -> usize {
    table(arena).map(|t| t.capacity).unwrap_or(0)
}

fn compute_capacity(t: &ArenaSpawns) -> usize {
    let pts: Vec<[f32; 3]> = t.points.iter().map(|&i| t.all[i].pos).chain(t.overflow.iter().map(|p| p.pos)).collect();
    let pool: Vec<usize> = (0..pts.len()).collect();
    let mut cap = 1;
    for n in 2..=MAX_RULE_PLAYERS.min(pts.len()) {
        if feasible(&pts, &pool, n, min_sep_cm(n), None).is_none() { break; }
        cap = n;
    }
    cap
}
/// Exhaustive subset search up to this many combinations, greedy beyond.
const EXHAUSTIVE_LIMIT: u64 = 20_000;
/// Lateral step for players beyond every real + derived point (the client's
/// clearance spiral resolves the rest).
const WRAP_STEP_CM: f32 = 120.0;
/// Spawn protection carried in each order. 0: rounds start from a frozen
/// countdown, so nobody can be hit at the spawn. Respawn modes set it later.
pub const PROTECT_MS: u16 = 0;

/// One player to place. `team` 0 = no team.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Seat {
    pub peer: PeerId,
    pub team: u8,
}

pub fn table(arena: &str) -> Option<&'static ArenaSpawns> {
    tables().iter().find(|a| a.arena == arena)
}

/// Table point behind a slot (for logs): (source spawner, derived?).
pub fn describe(arena: &str, slot: u8) -> (&'static str, bool) {
    table(arena)
        .and_then(|t| candidates(t).get(slot as usize).map(|p| (p.src.as_str(), p.derived)))
        .unwrap_or(("?", false))
}

/// Every candidate point of an arena: valid spawners first, then overflow.
/// Slot = index into this list.
fn candidates(t: &'static ArenaSpawns) -> Vec<&'static SpawnPoint> {
    t.points.iter().map(|&i| &t.all[i]).chain(t.overflow.iter()).collect()
}

fn dist(a: [f32; 3], b: [f32; 3]) -> f32 {
    let (dx, dy, dz) = (a[0] - b[0], a[1] - b[1], a[2] - b[2]);
    (dx * dx + dy * dy + dz * dz).sqrt()
}

fn binom(n: usize, k: usize) -> u64 {
    if k > n { return 0; }
    let k = k.min(n - k);
    let mut r: u64 = 1;
    for i in 0..k {
        r = r.saturating_mul((n - i) as u64) / (i as u64 + 1);
    }
    r
}

/// Rank of a subset for `sep`: (meets the spacing, fits the spread cap, min
/// pairwise distance, sum). Compared in that order by `better`.
type Rank = (bool, bool, f32, f32);

fn rank(pts: &[[f32; 3]], idx: &[usize], sep: f32) -> Rank {
    let (fits, min_d, sum) = score(pts, idx);
    (idx.len() < 2 || min_d >= sep - 0.5, fits, min_d, sum)
}

fn better(a: Rank, b: Rank) -> bool {
    if a.0 != b.0 { return a.0; }
    if a.1 != b.1 { return a.1; }
    if (a.2 - b.2).abs() > 0.5 { return a.2 > b.2; }
    a.3 > b.3 + 0.5
}

/// First (lexicographic, deterministic) `k`-subset of `pool` whose pairs are
/// all at least `sep` apart (and at most `cap` when given): a depth-first
/// clique search, exhaustive, fast on <= 40 points.
fn feasible(pts: &[[f32; 3]], pool: &[usize], k: usize, sep: f32, cap: Option<f32>) -> Option<Vec<usize>> {
    fn ok(pts: &[[f32; 3]], a: usize, b: usize, sep: f32, cap: Option<f32>) -> bool {
        let d = dist(pts[a], pts[b]);
        d >= sep - 0.5 && cap.map_or(true, |c| d <= c)
    }
    fn dfs(pts: &[[f32; 3]], pool: &[usize], start: usize, k: usize, sep: f32, cap: Option<f32>, cur: &mut Vec<usize>) -> bool {
        if cur.len() == k { return true; }
        for i in start..pool.len() {
            if pool.len() - i < k - cur.len() { return false; }
            let c = pool[i];
            if cur.iter().all(|&o| ok(pts, o, c, sep, cap)) {
                cur.push(c);
                if dfs(pts, pool, i + 1, k, sep, cap, cur) { return true; }
                cur.pop();
            }
        }
        false
    }
    if k > pool.len() { return None; }
    let mut cur = Vec::with_capacity(k);
    if dfs(pts, pool, 0, k, sep, cap, &mut cur) { Some(cur) } else { None }
}

/// Best `k`-subset of `pool` for `sep`: exhaustive or greedy max-min, then,
/// if that misses the spacing, any subset that meets it (inside the spread
/// cap first).
fn pick(pts: &[[f32; 3]], pool: &[usize], k: usize, sep: f32) -> Vec<usize> {
    let best = choose(pts, &[], pool, k, sep);
    if rank(pts, &best, sep).0 { return best; }
    feasible(pts, pool, k, sep, Some(MAX_SPREAD_CM))
        .or_else(|| feasible(pts, pool, k, sep, None))
        .unwrap_or(best)
}

/// Score of a subset: (fits the spread cap, min pairwise distance, sum).
fn score(pts: &[[f32; 3]], idx: &[usize]) -> (bool, f32, f32) {
    let (mut min_d, mut max_d, mut sum) = (f32::MAX, 0.0f32, 0.0f32);
    for i in 0..idx.len() {
        for j in i + 1..idx.len() {
            let d = dist(pts[idx[i]], pts[idx[j]]);
            min_d = min_d.min(d);
            max_d = max_d.max(d);
            sum += d;
        }
    }
    if idx.len() < 2 { min_d = 0.0; }
    (max_d <= MAX_SPREAD_CM, min_d, sum)
}

/// Choose `k` indices out of `pool` (indices into `pts`), all of `fixed`
/// included, maximising separation. Deterministic (first best in
/// lexicographic order wins ties).
fn choose(pts: &[[f32; 3]], fixed: &[usize], pool: &[usize], k: usize, sep: f32) -> Vec<usize> {
    let need = k.saturating_sub(fixed.len());
    if need == 0 { return fixed[..k].to_vec(); }
    if need >= pool.len() {
        let mut v = fixed.to_vec();
        v.extend_from_slice(pool);
        return v;
    }
    if binom(pool.len(), need) <= EXHAUSTIVE_LIMIT {
        let mut best: Option<(Vec<usize>, Rank)> = None;
        let mut comb: Vec<usize> = (0..need).collect();
        loop {
            let mut idx = fixed.to_vec();
            idx.extend(comb.iter().map(|&c| pool[c]));
            let s = rank(pts, &idx, sep);
            if best.as_ref().map(|(_, bs)| better(s, *bs)).unwrap_or(true) {
                best = Some((idx, s));
            }
            // Next combination in lexicographic order.
            let mut i = need;
            let mut more = false;
            while i > 0 {
                i -= 1;
                if comb[i] < pool.len() - need + i { more = true; break; }
            }
            if !more { return best.map(|b| b.0).unwrap_or_default(); }
            comb[i] += 1;
            for j in i + 1..need { comb[j] = comb[j - 1] + 1; }
        }
    }
    // Greedy farthest-point for very large pools.
    let mut chosen = fixed.to_vec();
    let mut rest: Vec<usize> = pool.to_vec();
    if chosen.is_empty() {
        // Start from the pair furthest apart within the cap.
        let mut bp = (rest[0], rest[1], -1.0f32);
        for i in 0..rest.len() {
            for j in i + 1..rest.len() {
                let d = dist(pts[rest[i]], pts[rest[j]]);
                if d <= MAX_SPREAD_CM && d > bp.2 { bp = (rest[i], rest[j], d); }
            }
        }
        chosen.push(bp.0);
        if need >= 2 { chosen.push(bp.1); }
        rest.retain(|r| !chosen.contains(r));
    }
    while chosen.len() < k && !rest.is_empty() {
        let (mut bi, mut bd) = (0usize, -1.0f32);
        for (i, &r) in rest.iter().enumerate() {
            let d = chosen.iter().map(|&c| dist(pts[r], pts[c])).fold(f32::MAX, f32::min);
            if d > bd { bi = i; bd = d; }
        }
        chosen.push(rest.remove(bi));
    }
    chosen
}

fn yaw_towards(from: [f32; 3], to: [f32; 3]) -> Option<f32> {
    let (dx, dy) = (to[0] - from[0], to[1] - from[1]);
    if dx.abs() + dy.abs() < 1.0 { return None; }
    Some(dy.atan2(dx).to_degrees())
}

/// Face the centroid of everybody else's spawn (arena centre when alone).
fn face(plan: &mut [SpawnAssign], centre: [f32; 3]) {
    let n = plan.len();
    let all: Vec<[f32; 3]> = plan.iter().map(|s| s.pos).collect();
    for (i, s) in plan.iter_mut().enumerate() {
        let target = if n < 2 {
            centre
        } else {
            let mut c = [0.0f32; 3];
            for (j, p) in all.iter().enumerate() {
                if j != i { for a in 0..3 { c[a] += p[a] / (n - 1) as f32; } }
            }
            c
        };
        if let Some(y) = yaw_towards(s.pos, target) { s.yaw = y; }
    }
}

/// Canonical order of the chosen points: by angle around their centroid,
/// starting at the smallest slot. Neighbours in this order are neighbours
/// in space, so rotating by one each round moves everyone to the next spot.
fn ring_order(pts: &[[f32; 3]], mut idx: Vec<usize>) -> Vec<usize> {
    if idx.len() < 3 { idx.sort_unstable(); return idx; }
    let n = idx.len() as f32;
    let cx = idx.iter().map(|&i| pts[i][0]).sum::<f32>() / n;
    let cy = idx.iter().map(|&i| pts[i][1]).sum::<f32>() / n;
    let ang = |i: usize| (pts[i][1] - cy).atan2(pts[i][0] - cx);
    idx.sort_by(|&a, &b| ang(a).partial_cmp(&ang(b)).unwrap().then(a.cmp(&b)));
    let start = idx.iter().enumerate().min_by_key(|(_, &v)| v).map(|(p, _)| p).unwrap_or(0);
    idx.rotate_left(start);
    idx
}

/// Order points along the team axis (team 1 side first): the axis runs from
/// the table's team-1 points to its team-2 points when the arena has native
/// team tags, else along the widest pair of the chosen points.
fn side_order(cand: &[&SpawnPoint], pts: &[[f32; 3]], mut idx: Vec<usize>) -> Vec<usize> {
    let centroid = |team: u8| -> Option<[f32; 2]> {
        let v: Vec<&&SpawnPoint> = cand.iter().filter(|p| p.team == team).collect();
        if v.is_empty() { return None; }
        let n = v.len() as f32;
        Some([v.iter().map(|p| p.pos[0]).sum::<f32>() / n, v.iter().map(|p| p.pos[1]).sum::<f32>() / n])
    };
    let axis = match (centroid(1), centroid(2)) {
        (Some(a), Some(b)) => [b[0] - a[0], b[1] - a[1]],
        _ => {
            let (mut ba, mut bb, mut bd) = (idx[0], idx[0], -1.0f32);
            for &i in &idx { for &j in &idx {
                let d = dist(pts[i], pts[j]);
                if i < j && d > bd { ba = i; bb = j; bd = d; }
            } }
            [pts[bb][0] - pts[ba][0], pts[bb][1] - pts[ba][1]]
        }
    };
    let proj = |i: usize| pts[i][0] * axis[0] + pts[i][1] * axis[1];
    idx.sort_by(|&a, &b| proj(a).partial_cmp(&proj(b)).unwrap().then(a.cmp(&b)));
    idx
}

fn wrap_pos(pts: &[[f32; 3]], order: &[usize], placed: &[SpawnAssign]) -> [f32; 3] {
    let mut best = (pts[order[0]], -1.0f32);
    for ring in 1..=3 {
        for &o in order {
            for k in 0..8 {
                let a = (k as f32) * std::f32::consts::FRAC_PI_4;
                let b = pts[o];
                let c = [b[0] + a.cos() * WRAP_STEP_CM * ring as f32, b[1] + a.sin() * WRAP_STEP_CM * ring as f32, b[2]];
                let d = placed.iter().map(|s| dist(c, s.pos)).fold(f32::MAX, f32::min);
                if d > best.1 + 0.5 { best = (c, d); }
            }
        }
        if best.1 >= WRAP_STEP_CM { break; }
    }
    best.0
}

/// Unique per (round, seat): round in the high bits, creation index low.
fn spawn_id(round: u32, index: usize) -> u32 { (round << 8) | (index as u32 & 0xFF) }

/// Deterministic, distinct spawn assignment for one round.
/// Unknown arena (no table): empty plan, clients keep the native spawn.
pub fn assign(arena: &str, round: u32, seats: &[Seat]) -> Vec<SpawnAssign> {
    let Some(t) = table(arena) else { return Vec::new(); };
    if seats.is_empty() { return Vec::new(); }
    let mut seats = seats.to_vec();
    seats.sort_by_key(|s| (s.team, s.peer));
    seats.dedup_by_key(|s| s.peer);
    let cand = candidates(t);
    let pts: Vec<[f32; 3]> = cand.iter().map(|p| p.pos).collect();
    let n = seats.len();
    let sep = min_sep_cm(n);
    let real: Vec<usize> = (0..t.points.len()).collect();
    let all: Vec<usize> = (0..cand.len()).collect();
    let k = n.min(cand.len());
    // Real spawners first; derived points join only when the real ones are
    // too few or cannot meet the spacing.
    let mut chosen = if n <= real.len() { pick(&pts, &real, n, sep) } else { Vec::new() };
    if chosen.len() < k || !rank(&pts, &chosen, sep).0 {
        let alt = pick(&pts, &all, k, sep);
        if chosen.len() < k || better(rank(&pts, &alt, sep), rank(&pts, &chosen, sep)) { chosen = alt; }
    }
    let teams = seats.iter().any(|s| s.team != 0);
    let mut order = if teams { side_order(&cand, &pts, chosen) } else { ring_order(&pts, chosen) };
    let m = order.len();
    if teams {
        if round % 2 == 1 { order.reverse(); } // teams swap sides every round
    } else {
        order.rotate_left(round as usize % m); // everyone moves on every round
    }
    let mut plan: Vec<SpawnAssign> = Vec::with_capacity(seats.len());
    for (i, s) in seats.iter().enumerate() {
        let slot = order[i % m];
        let p = cand[slot];
        let pos = if i < m {
            p.pos
        } else {
            // More players than points: the offset (8 directions, growing
            // rings) around a chosen point that is furthest from everyone
            // placed so far. The client clearance spiral resolves the rest.
            wrap_pos(&pts, &order, &plan)
        };
        plan.push(SpawnAssign {
            peer_id: s.peer, spawn_id: spawn_id(round, i), slot: slot.min(255) as u8,
            pos, yaw: p.yaw, protect_ms: PROTECT_MS,
        });
    }
    face(&mut plan, t.centre);
    plan
}

/// Add a seat to an existing plan without moving anyone (a player who joined
/// or reconnected during the countdown): the free point furthest from every
/// occupied one. Returns false if the peer already has a seat.
pub fn add_seat(arena: &str, round: u32, plan: &mut Vec<SpawnAssign>, peer: PeerId) -> bool {
    if plan.iter().any(|s| s.peer_id == peer) { return false; }
    let Some(t) = table(arena) else { return false; };
    let cand = candidates(t);
    let used: Vec<usize> = plan.iter().map(|s| s.slot as usize).collect();
    let free_real = (0..t.points.len()).filter(|i| !used.contains(i));
    let free_over = (t.points.len()..cand.len()).filter(|i| !used.contains(i));
    let pick = |it: &mut dyn Iterator<Item = usize>| -> Option<usize> {
        let mut best: Option<(usize, f32)> = None;
        for i in it {
            let d = plan.iter().map(|s| dist(cand[i].pos, s.pos)).fold(f32::MAX, f32::min);
            if best.map(|(_, bd)| d > bd + 0.5).unwrap_or(true) { best = Some((i, d)); }
        }
        best.map(|(i, _)| i)
    };
    // A real point first, unless only a derived one keeps the spacing.
    let sep = min_sep_cm(plan.len() + 1);
    let gap = |i: usize| plan.iter().map(|s| dist(cand[i].pos, s.pos)).fold(f32::MAX, f32::min);
    let slot = match (pick(&mut free_real.into_iter()), pick(&mut free_over.into_iter())) {
        (Some(r), Some(o)) if gap(r) < sep - 0.5 && gap(o) > gap(r) + 0.5 => Some(o),
        (r, o) => r.or(o),
    };
    let (slot, pos, yaw) = match slot {
        Some(i) => (i, cand[i].pos, cand[i].yaw),
        None => {
            let pts: Vec<[f32; 3]> = cand.iter().map(|p| p.pos).collect();
            let order: Vec<usize> = (0..cand.len()).collect();
            let pos = wrap_pos(&pts, &order, plan);
            let i = plan.len() % cand.len();
            (i, pos, cand[i].yaw)
        }
    };
    let next = plan.iter().map(|s| (s.spawn_id & 0xFF) as usize + 1).max().unwrap_or(0);
    let mut s = SpawnAssign {
        peer_id: peer, spawn_id: spawn_id(round, next), slot: slot as u8, pos, yaw, protect_ms: PROTECT_MS,
    };
    if !plan.is_empty() {
        let n = plan.len() as f32;
        let c = [
            plan.iter().map(|p| p.pos[0]).sum::<f32>() / n,
            plan.iter().map(|p| p.pos[1]).sum::<f32>() / n,
            plan.iter().map(|p| p.pos[2]).sum::<f32>() / n,
        ];
        if let Some(y) = yaw_towards(s.pos, c) { s.yaw = y; }
    }
    plan.push(s);
    true
}

#[cfg(test)]
mod spawn_tests {
    use super::*;

    const ARENAS: [&str; 7] = [
        "Map_Arena_Alley", "Map_Arena_Pit", "Map_Arena_Yard", "Map_Arena_Slums",
        "Map_Arena_Cellar", "Map_Arena_LordsHall", "Map_Arena_EastTower",
    ];

    fn seats(ids: &[PeerId]) -> Vec<Seat> { ids.iter().map(|&p| Seat { peer: p, team: 0 }).collect() }

    fn min_sep(plan: &[SpawnAssign]) -> f32 {
        let mut m = f32::MAX;
        for i in 0..plan.len() { for j in i + 1..plan.len() { m = m.min(dist(plan[i].pos, plan[j].pos)); } }
        m
    }

    #[test]
    fn every_mp_arena_has_a_table() {
        for a in ARENAS {
            let t = table(a).unwrap_or_else(|| panic!("no spawn table for {a}"));
            assert!(t.points.len() >= 4, "{a}: only {} points", t.points.len());
            assert!(t.points.iter().all(|&i| t.all[i].valid && !t.all[i].derived) && t.overflow.iter().all(|p| p.derived));
            assert!(candidates(t).len() >= 8, "{a}: fewer than 8 candidates");
        }
        assert!(table("default").is_none());
    }

    #[test]
    fn distinct_for_2_to_8_players_every_arena_every_round() {
        for a in ARENAS {
            for n in 2..=8u32 {
                let ids: Vec<PeerId> = (1..=n).map(|i| i * 3 + 7).collect();
                for round in 1..=6 {
                    let plan = assign(a, round, &seats(&ids));
                    assert_eq!(plan.len(), n as usize, "{a} n={n}");
                    let mut slots: Vec<u8> = plan.iter().map(|s| s.slot).collect();
                    slots.sort_unstable(); slots.dedup();
                    assert_eq!(slots.len(), n as usize, "{a} n={n} round={round}: shared slot");
                    assert!(min_sep(&plan) >= 140.0, "{a} n={n}: spawns {:.0} cm apart", min_sep(&plan));
                    let mut peers: Vec<PeerId> = plan.iter().map(|s| s.peer_id).collect();
                    peers.sort_unstable();
                    assert_eq!(peers, ids);
                }
            }
        }
    }

    /// Players must not be knocked down (and disarmed) at spawn: spawns of a
    /// round are >= 4 m apart for 2-4 players and >= 3 m for 5-8, on every
    /// arena up to its capacity; every arena hosts 4; only the Cellar (one
    /// 5.8 m room) stops below 8. capacity() == the generator's max_players.
    #[test]
    fn spawns_meet_the_spacing_rule_on_every_arena() {
        assert_eq!((min_sep_cm(2), min_sep_cm(4), min_sep_cm(5), min_sep_cm(8)), (400.0, 400.0, 300.0, 300.0));
        for a in ARENAS {
            let t = table(a).unwrap();
            let cap = capacity(a);
            assert_eq!(cap, t.max_players, "{a}: server capacity vs map file max_players");
            assert!(cap >= 4, "{a}: capacity {cap}");
            assert_eq!(cap, if a == "Map_Arena_Cellar" { 4 } else { 8 }, "{a}");
            for n in 2..=cap as u32 {
                for round in 1..=4 {
                    let ids: Vec<PeerId> = (1..=n).map(|i| i * 5 + 1).collect();
                    let plan = assign(a, round, &seats(&ids));
                    let m = min_sep(&plan);
                    assert!(m >= min_sep_cm(n as usize) - 0.5, "{a} n={n} round={round}: spawns only {m:.0} cm apart");
                }
            }
            // Beyond capacity: still the best effort (distinct, as far apart as the points allow).
            for n in cap as u32 + 1..=8 {
                let plan = assign(a, 1, &seats(&(1..=n).collect::<Vec<_>>()));
                assert!(min_sep(&plan) >= 140.0, "{a} n={n}");
            }
        }
        assert_eq!(capacity("default"), 0);
    }

    #[test]
    fn a_seat_added_during_the_countdown_keeps_the_spacing() {
        for a in ARENAS {
            let mut plan = assign(a, 1, &seats(&[1, 2]));
            assert!(add_seat(a, 1, &mut plan, 3));
            if capacity(a) >= 3 {
                assert!(min_sep(&plan) >= 400.0 - 0.5, "{a}: 3rd seat {:.0} cm from the others", min_sep(&plan));
            }
        }
    }

    #[test]
    fn deterministic_and_order_independent() {
        for a in ARENAS {
            let x = assign(a, 3, &seats(&[4, 9, 2, 17]));
            let y = assign(a, 3, &seats(&[17, 2, 9, 4]));
            assert_eq!(x, y, "{a}");
            assert_eq!(x, assign(a, 3, &seats(&[4, 9, 2, 17])));
        }
    }

    #[test]
    fn duel_spawns_are_far_apart_and_face_each_other() {
        for a in ARENAS {
            let plan = assign(a, 1, &seats(&[1, 2]));
            let d = dist(plan[0].pos, plan[1].pos);
            // Best pair under the spread cap: at least as far as any real pair
            // that also fits under the cap.
            let t = table(a).unwrap();
            let mut best = 0.0f32;
            let real = &candidates(t)[..t.points.len()];
            for p in real { for q in real {
                let e = dist(p.pos, q.pos);
                if e <= MAX_SPREAD_CM && e > best { best = e; }
            } }
            assert!((d - best).abs() < 1.0, "{a}: duel {d:.0} cm, best {best:.0} cm");
            assert!(d >= 550.0, "{a}: duel spawns only {d:.0} cm apart"); // Cellar is 5.8 m across
            let y0 = yaw_towards(plan[0].pos, plan[1].pos).unwrap();
            let y1 = yaw_towards(plan[1].pos, plan[0].pos).unwrap();
            assert!((plan[0].yaw - y0).abs() < 0.01 && (plan[1].yaw - y1).abs() < 0.01, "{a}");
        }
    }

    #[test]
    fn max_separation_beats_every_other_subset_on_pit() {
        // Exhaustive check against brute force for 3 and 4 players.
        let t = table("Map_Arena_Pit").unwrap();
        let pts: Vec<[f32; 3]> = candidates(t)[..t.points.len()].iter().map(|p| p.pos).collect();
        for n in [3usize, 4] {
            let plan = assign("Map_Arena_Pit", 1, &seats(&(1..=n as u32).collect::<Vec<_>>()));
            let got = min_sep(&plan);
            let mut best = 0.0f32;
            let k = pts.len();
            for mask in 0u32..(1 << k) {
                if mask.count_ones() as usize != n { continue; }
                let idx: Vec<usize> = (0..k).filter(|i| mask & (1 << i) != 0).collect();
                let (fits, m, _) = score(&pts, &idx);
                if fits && m > best { best = m; }
            }
            assert!((got - best).abs() < 1.0, "n={n}: got {got:.0}, best {best:.0}");
        }
    }

    #[test]
    fn players_rotate_between_rounds() {
        let a = assign("Map_Arena_Yard", 1, &seats(&[1, 2]));
        let b = assign("Map_Arena_Yard", 2, &seats(&[1, 2]));
        assert_eq!(a[0].slot, b[1].slot);
        assert_eq!(a[1].slot, b[0].slot);
    }

    #[test]
    fn overflow_and_wrap_stay_distinct() {
        // Pit has 8 real points: 10 players need derived ones; 14 wrap.
        let pit = table("Map_Arena_Pit").unwrap();
        let plan = assign("Map_Arena_Pit", 1, &seats(&(1..=10).collect::<Vec<_>>()));
        assert!(plan.iter().any(|s| s.slot as usize >= pit.points.len()), "derived points join beyond the real ones");
        let mut slots: Vec<u8> = plan.iter().map(|s| s.slot).collect();
        slots.sort_unstable(); slots.dedup();
        assert_eq!(slots.len(), 10);
        let big = assign("Map_Arena_Cellar", 1, &seats(&(1..=12).collect::<Vec<_>>()));
        assert!(min_sep(&big) >= 100.0, "wrapped players {:.0} cm apart", min_sep(&big));
    }

    #[test]
    fn teams_take_opposite_sides_and_swap() {
        let s = vec![
            Seat { peer: 1, team: 1 }, Seat { peer: 2, team: 1 },
            Seat { peer: 3, team: 2 }, Seat { peer: 4, team: 2 },
        ];
        let plan = assign("Map_Arena_Cellar", 2, &s);
        let c = |team: u8, p: &[SpawnAssign]| -> [f32; 3] {
            let v: Vec<_> = p.iter().filter(|x| s.iter().any(|y| y.peer == x.peer_id && y.team == team)).collect();
            [v.iter().map(|x| x.pos[0]).sum::<f32>() / 2.0, v.iter().map(|x| x.pos[1]).sum::<f32>() / 2.0, 0.0]
        };
        // Team members closer to each other than to the other team's centre.
        let (c1, c2) = (c(1, &plan), c(2, &plan));
        for x in &plan {
            let t = s.iter().find(|y| y.peer == x.peer_id).unwrap().team;
            let (own, other) = if t == 1 { (c1, c2) } else { (c2, c1) };
            assert!(dist(x.pos, own) < dist(x.pos, other));
        }
        let next = assign("Map_Arena_Cellar", 3, &s);
        let n1 = c(1, &next);
        assert!(dist(n1, c2) < dist(n1, c1), "team 1 moved to team 2's side");
    }

    #[test]
    fn add_seat_keeps_existing_and_picks_a_free_point() {
        let mut plan = assign("Map_Arena_EastTower", 1, &seats(&[1, 2]));
        let before = plan.clone();
        assert!(add_seat("Map_Arena_EastTower", 1, &mut plan, 9));
        assert!(!add_seat("Map_Arena_EastTower", 1, &mut plan, 9));
        assert_eq!(&plan[..2], &before[..]);
        assert_eq!(plan.len(), 3);
        assert!(plan[2].slot != plan[0].slot && plan[2].slot != plan[1].slot);
        assert!(min_sep(&plan) >= 300.0);
    }

    /// The Lua copy (shared/hsmp_arenas.lua) must equal the server data:
    /// same maps, same spawners (id, position, yaw, team, valid), same
    /// overflow points. Regenerate both with `hsmp-tools gen-map-data`.
    #[test]
    fn lua_arena_catalogue_matches_server_map_data() {
        let lua = include_str!("../../mods/shared/hsmp_arenas.lua");
        let field = |line: &str, key: &str| -> String {
            let k = format!("{key} = ");
            let rest = &line[line.find(&k).unwrap_or_else(|| panic!("{key} missing: {line}")) + k.len()..];
            if let Some(q) = rest.strip_prefix('"') {
                return q.split('"').next().unwrap().to_string();
            }
            rest.split([',', ' ', '}']).next().unwrap().to_string()
        };
        let mut lua_maps: Vec<(String, Vec<String>)> = Vec::new();
        for line in lua.lines() {
            let l = line.trim();
            if let Some(rest) = l.strip_prefix("A[\"") {
                lua_maps.push((rest.split('"').next().unwrap().to_string(), Vec::new()));
            } else if l.starts_with("{ id = ") {
                let m = &mut lua_maps.last_mut().expect("entry before map").1;
                let mut row = format!("{}|{}|{}|{}", field(l, "id"), field(l, "x"), field(l, "y"), field(l, "z"));
                if l.contains("yaw = ") {
                    row += &format!("|{}|{}|{}", field(l, "yaw"), field(l, "team"), field(l, "valid"));
                }
                m.push(row);
            }
        }
        let f = |v: f32| format!("{v:.1}");
        let rust: Vec<(String, Vec<String>)> = tables().iter().map(|t| {
            let mut rows: Vec<String> = t.all.iter().map(|p| format!("{}|{}|{}|{}|{}|{}|{}",
                p.src, f(p.pos[0]), f(p.pos[1]), f(p.pos[2]), f(p.yaw), p.team, p.valid)).collect();
            rows.extend(t.overflow.iter().map(|p| format!("{}|{}|{}|{}", p.src, f(p.pos[0]), f(p.pos[1]), f(p.pos[2]))));
            (t.arena.clone(), rows)
        }).collect();
        assert_eq!(lua_maps, rust, "hsmp_arenas.lua differs from server/data/maps: run hsmp-tools gen-map-data");
    }

    #[test]
    fn disabled_and_template_spawners_are_never_valid() {
        // Pit sp8..sp14 and Cellar sp4..sp10 are disabled for every play mode;
        // the arena-floor ones (Pit sp10 on the ring, the Cellar main-room
        // corners sp4/5/7/8) are used for MP spacing (gen-map-data MP_ROOM_POINTS).
        let pit = table("Map_Arena_Pit").unwrap();
        for p in &pit.all {
            let n: u32 = p.src[2..].parse().unwrap();
            assert_eq!(p.valid, !(8..=14).contains(&n) || n == 10, "Pit {}", p.src);
        }
        let cellar = table("Map_Arena_Cellar").unwrap();
        assert_eq!(cellar.points.len(), 8);
        for p in &cellar.all {
            let n: u32 = p.src[2..].parse().unwrap();
            assert_eq!(p.valid, ![6, 9, 10].contains(&n), "Cellar {} (the east side room stays out)", p.src);
        }
        for a in ["Map_Arena_Alley", "Map_Arena_Pit", "Map_Arena_Yard", "Map_Arena_Slums",
                  "Map_Arena_Cellar", "Map_Arena_LordsHall", "Map_Arena_EastTower"] {
            for p in candidates(table(a).unwrap()) {
                assert!(p.pos[0].abs() + p.pos[1].abs() > 1.0, "{a} {} at origin", p.src);
                assert!(p.pos[2] > table(a).unwrap().kill_z + 1000.0);
            }
        }
    }

    #[test]
    fn unknown_arena_gives_empty_plan() {
        assert!(assign("default", 1, &seats(&[1, 2])).is_empty());
    }
}
