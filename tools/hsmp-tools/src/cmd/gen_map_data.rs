//! `hsmp-tools gen-map-data [--check]`: generate the MP map data from
//! docs/arena_static. Port of the old Python generator, with byte-identical
//! output.
//!
//! Outputs (both generated, never hand-edited):
//!   server/data/maps/<Map>.json            loaded by the server (spawns.rs, include_str!)
//!   mods/shared/hsmp_arenas.lua  the Lua copy (a cargo test checks equality)
//!
//! Per map: every BP_SpawnerPoint_Willies with absolute coordinates, yaw, team
//! tag, game/play modes and a `valid` verdict, derived `overflow` points, the
//! spawn centre, rough bounds and a kill_z. See docs/development/subsystems/spawns.md.
//!
//! Spawn validity (only `valid` points are ever assigned):
//!   * a point whose `Works in these Play Modes` is false for every play mode is
//!     DISABLED (Pit/Yard corner points outside the ring, the Cellar side rooms,
//!     the Narrow-Passage-only points);
//!   * a point at the world origin is an unplaced template;
//!   * a point closer than BARRIER_CLEAR_CM to a barrier actor (fence, gate,
//!     door, portcullis) is pushed straight away from it until it has that
//!     clearance (Slums spawn 0 sits 82 cm from a flimsy fence); one ON a barrier
//!     is dropped.
//! Overflow points (for more players than valid points; Cellar has 4): the
//! midpoint of two valid points on the same floor (|dz| < 40 cm, 250..900 cm
//! apart), >= 150 cm from every other point, not within 150 cm of a trap/fence
//! and not over a pit (a trap > 3 m below within 2 m: the Pit's spike pit).
//! Hazards from dynamic sublevels (Narrow-Passage spike rows, lighting variants)
//! are ignored: those sublevels are not loaded in the MP combat mode.
//!
//! --check exits 1 if a committed output differs from a fresh run.

use anyhow::{Context, Result};
use hsmp_tools::paths;
use hsmp_tools::pyfmt::{dumps_indent, fixed, py_repr, py_round, PyVal};
use serde_json::Value;
use std::path::{Path, PathBuf};

#[derive(clap::Args)]
pub struct Args {
    /// Exit 1 if a committed output differs from a fresh run, or if any spawn
    /// the committed data marks valid (or any overflow point) fails the
    /// geometry rules (writes nothing)
    #[arg(long)]
    check: bool,
    /// Print the per-spawn geometry table (local floor, centre distance)
    #[arg(long)]
    report: bool,
}

// --- spawn geometry rules (no spawning on ledges or roofs) -----------------------
//
// docs/arena_static has no collision geometry, so the floor near a point is
// estimated from what designers put ON floors: every spawner (enabled or not)
// and the floor-resting world objects (weapons, quivers, traps, fences,
// destructibles, containers, dummies) within FLOOR_R_CM in XY and
// FLOOR_DZ_CM vertically, excluding dynamic sublevels. A spawn more than
// MAX_ABOVE_FLOOR_CM above the lowest of them is on a roof, a ledge or a
// gallery. A spawn farther than max(OUTLIER_MIN_CM, OUTLIER_K x median) from
// the centroid of the valid points is outside the arena proper (the disabled
// Pit/Yard corner points would be). Runtime ground snaps measured in
// the Slums agree with the data: sp4 floor 853 (data 855.3),
// sp7 874 (data 886.1).
const FLOOR_R_CM: f64 = 500.0;
const FLOOR_DZ_CM: f64 = 400.0;
const MAX_ABOVE_FLOOR_CM: f64 = 200.0;
const OUTLIER_MIN_CM: f64 = 1500.0;
const OUTLIER_K: f64 = 2.5;
const FLOOR_CATEGORIES: [&str; 7] = ["weapon", "quiver", "trap", "destructible", "fence", "container", "training_dummy"];

/// Floor evidence of one map: (label, position).
fn floor_evidence(d: &Value) -> Vec<(String, [f64; 3])> {
    let mut out = vec![];
    for sp in arr(d, "spawn_points") {
        let l = xyz(&sp["location"]);
        if l[0].abs() + l[1].abs() + l[2].abs() >= 1.0 {
            out.push((s(sp, "name").replace("BP_SpawnerPoint_Willies_C_", "sp"), l));
        }
    }
    for o in arr(d, "world_objects") {
        let cat = o.get("category").and_then(|v| v.as_str()).unwrap_or("");
        if !FLOOR_CATEGORIES.contains(&cat) || !truthy(o.get("location")) {
            continue;
        }
        if o.get("via").and_then(|v| v.as_str()).unwrap_or("").contains("LevelStreamingDynamic") {
            continue;
        }
        let l = xyz(&o["location"]);
        if l[0].abs() + l[1].abs() > 1.0 {
            out.push((format!("{cat}:{}", s(o, "class")), l));
        }
    }
    out
}

/// Lowest floor evidence near `p` (excluding `p` itself): (z, label, n).
fn local_floor(ev: &[(String, [f64; 3])], p: &[f64; 3]) -> Option<(f64, String, usize)> {
    let mut best: Option<(f64, String)> = None;
    let mut n = 0;
    for (lab, q) in ev {
        let dxy = hypot(q[0] - p[0], q[1] - p[1]);
        if dxy < 1.0 && (q[2] - p[2]).abs() < 1.0 {
            continue; // the point itself
        }
        if dxy <= FLOOR_R_CM && (q[2] - p[2]).abs() <= FLOOR_DZ_CM {
            n += 1;
            if best.as_ref().map(|b| q[2] < b.0).unwrap_or(true) {
                best = Some((q[2], lab.clone()));
            }
        }
    }
    best.map(|(z, l)| (z, l, n))
}

/// One geometry verdict per point: None = fine, Some(reason) = reject.
struct GeoRow {
    id: String,
    pos: [f64; 3],
    floor: Option<(f64, String, usize)>,
    centre_d: f64,
    verdict: Option<String>,
}

fn median(mut v: Vec<f64>) -> f64 {
    if v.is_empty() {
        return 0.0;
    }
    v.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let n = v.len();
    if n % 2 == 1 { v[n / 2] } else { (v[n / 2 - 1] + v[n / 2]) / 2.0 }
}

/// Geometry check of a list of (id, pos) points of one map against its static data.
fn geometry(d: &Value, pts: &[(String, [f64; 3])]) -> Vec<GeoRow> {
    let ev = floor_evidence(d);
    let n = pts.len().max(1) as f64;
    let cx = pts.iter().map(|p| p.1[0]).sum::<f64>() / n;
    let cy = pts.iter().map(|p| p.1[1]).sum::<f64>() / n;
    let dists: Vec<f64> = pts.iter().map(|p| hypot(p.1[0] - cx, p.1[1] - cy)).collect();
    let lim = OUTLIER_MIN_CM.max(OUTLIER_K * median(dists.clone()));
    pts.iter()
        .zip(dists)
        .map(|((id, pos), cd)| {
            let floor = local_floor(&ev, pos);
            let mut verdict = None;
            if let Some((fz, lab, _)) = &floor {
                if pos[2] - fz > MAX_ABOVE_FLOOR_CM {
                    verdict = Some(format!("{:.0} cm above the local floor ({lab} at z {:.0}): roof/ledge", pos[2] - fz, fz));
                }
            }
            if verdict.is_none() && cd > lim {
                verdict = Some(format!("{:.0} cm from the arena centre (limit {:.0}): outside the arena", cd, lim));
            }
            GeoRow { id: id.clone(), pos: *pos, floor, centre_d: cd, verdict }
        })
        .collect()
}

fn load_static(src_dir: &Path, name: &str) -> Result<Value> {
    let p = src_dir.join(format!("Map_Arena_{name}.json"));
    serde_json::from_str(&paths::read_text(&p)?).with_context(|| format!("parse {}", p.display()))
}

/// The spacing rule on one map's assignable points (valid + overflow).
fn spacing_violations(name: &str, pts: &[[f64; 3]], committed_max: Option<&Value>) -> Vec<String> {
    let mut bad = vec![];
    let cap = capacity(pts);
    let floor = CAPACITY_FLOOR.iter().find(|(n, _)| *n == name).map(|(_, c)| *c).unwrap_or(RULE_MAX_PLAYERS);
    let need = floor.max(MIN_CAPACITY);
    if cap < need {
        let n = cap + 1;
        bad.push(format!(
            "Map_Arena_{name} spacing: no {n} spawns are {:.0} cm apart (max_players {cap}, required {need})",
            sep_for(n)
        ));
    }
    if cap > floor && floor < RULE_MAX_PLAYERS {
        bad.push(format!("Map_Arena_{name} spacing: CAPACITY_FLOOR {floor} is stale (the points now host {cap})"));
    }
    let committed = committed_max.and_then(|v| v.as_u64()).map(|v| v as usize);
    if committed != Some(cap) {
        bad.push(format!("Map_Arena_{name} spacing: max_players {committed:?} in the map file, {cap} computed"));
    }
    bad
}

/// The committed server/data/maps/<Map>.json: every valid spawn and overflow
/// point must pass the geometry rules and the spacing rule. Returns the violations.
fn check_committed(root: &Path) -> Result<Vec<String>> {
    let src = root.join("docs").join("arena_static");
    let mut bad = vec![];
    for (name, _) in ARENAS {
        let p = root.join("server").join("data").join("maps").join(format!("Map_Arena_{name}.json"));
        let Ok(text) = paths::read_text(&p) else {
            bad.push(format!("Map_Arena_{name}: {} missing", paths::rel(root, &p)));
            continue;
        };
        let m: Value = serde_json::from_str(&text).with_context(|| format!("parse {}", p.display()))?;
        let mut pts: Vec<(String, [f64; 3])> = arr(&m, "spawns")
            .iter()
            .filter(|sp| sp.get("valid").and_then(|v| v.as_bool()) == Some(true))
            .map(|sp| (s(sp, "id").to_string(), xyz(&sp["pos"])))
            .collect();
        let nvalid = pts.len();
        pts.extend(arr(&m, "overflow").iter().map(|o| (s(o, "id").to_string(), xyz(&o["pos"]))));
        bad.extend(spacing_violations(name, &pts.iter().map(|p| p.1).collect::<Vec<_>>(), m.get("max_players")));
        let d = load_static(&src, name)?;
        // the centroid is the VALID points' (overflow points must not move it)
        let mut rows = geometry(&d, &pts[..nvalid]);
        let all = geometry(&d, &pts);
        rows.extend(all.into_iter().skip(nvalid).map(|mut r| {
            // overflow: floor rule only (they are midpoints of valid points)
            if r.verdict.as_deref().map(|v| v.contains("outside the arena")).unwrap_or(false) {
                r.verdict = None;
            }
            r
        }));
        for r in rows {
            if let Some(v) = r.verdict {
                bad.push(format!("Map_Arena_{name} {} ({:.0},{:.0},{:.0}): {v}", r.id, r.pos[0], r.pos[1], r.pos[2]));
            }
        }
    }
    Ok(bad)
}

fn print_report(root: &Path) -> Result<()> {
    let src = root.join("docs").join("arena_static");
    for (name, _) in ARENAS {
        let d = load_static(&src, name)?;
        let sp = spawns(&d);
        let valid: Vec<(String, [f64; 3])> = sp.iter().filter(|p| p.valid).map(|p| (p.id.clone(), p.pos)).collect();
        println!("Map_Arena_{name}:");
        for r in geometry(&d, &valid) {
            let fl = match &r.floor {
                Some((z, lab, n)) => format!("floor {:.0} (+{:.0}) by {lab} [{n} pts]", z, r.pos[2] - z),
                None => "floor ? (no evidence within 5 m)".to_string(),
            };
            println!(
                "  {:>5} ({:>7.0},{:>7.0},{:>6.0})  {fl}  centre {:.0}  {}",
                r.id,
                r.pos[0],
                r.pos[1],
                r.pos[2],
                r.centre_d,
                r.verdict.as_deref().unwrap_or("ok")
            );
        }
    }
    Ok(())
}

const ARENAS: [(&str, &str); 7] = [
    ("Alley", "Alley"),
    ("Pit", "Pit"),
    ("Yard", "Yard"),
    ("Slums", "Slums"),
    ("Cellar", "Cellar"),
    ("LordsHall", "Lord's Hall"),
    ("EastTower", "East Tower"),
];
// --- MP spawn spacing -------------------------------------------------------------
//
// Any two spawns of one round must be at least SEP_SMALL_CM apart for 2-4
// players and SEP_LARGE_CM for 5-8 (server/src/spawns.rs min_sep_cm, same
// numbers). `max_players` in each map file is the most players for which a
// subset of valid + overflow points meets that; --check fails if it is below
// MIN_CAPACITY on any arena, below the declared CAPACITY_FLOOR, or stale.
const SEP_SMALL_CM: f64 = 400.0;
const SEP_LARGE_CM: f64 = 300.0;
const RULE_MAX_PLAYERS: usize = 8;
/// Every arena must host a 2v2 (4 players) at 4 m.
const MIN_CAPACITY: usize = 4;
/// Arenas that cannot host 8 at 3 m: the honest limit (anything lower is a regression).
/// Cellar: one 5.8 x 5.6 m room; its best 5 points (4 corners + centre) are only 298 cm apart.
const CAPACITY_FLOOR: [(&str, usize); 1] = [("Cellar", 4)];

// Spawners the native game disables for every play mode but that stand on the
// arena floor proper, inside the box of the valid points (checked), facing the
// room: usable for MP spacing. Each is checked against the geometry rules like
// any valid point.
//   Pit sp10 (356,-359): the missing SE point of the 5 m ring (sp1/sp4/sp5 are
//     its mirror images); sp11-14 (the corners outside the ring) stay excluded.
//   Cellar sp4/sp5/sp7/sp8: the main room's corner spawners, each facing the
//     room centre; sp6/sp9/sp10 (the east side room) stay excluded.
const MP_ROOM_POINTS: [(&str, &[&str]); 2] = [("Pit", &["sp10"]), ("Cellar", &["sp4", "sp5", "sp7", "sp8"])];
const MP_ROOM_WHY: &str = "mp: arena-floor spawner (native play-mode flags off)";

fn sep_for(n: usize) -> f64 {
    match n {
        0 | 1 => 0.0,
        2..=4 => SEP_SMALL_CM,
        _ => SEP_LARGE_CM,
    }
}

fn dist3(a: &[f64; 3], b: &[f64; 3]) -> f64 {
    ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2) + (a[2] - b[2]).powi(2)).sqrt()
}

/// A `k`-subset of `pts` whose pairs are all >= sep apart exists (exhaustive DFS).
fn spaced(pts: &[[f64; 3]], k: usize, sep: f64) -> bool {
    fn dfs(pts: &[[f64; 3]], start: usize, k: usize, sep: f64, cur: &mut Vec<usize>) -> bool {
        if cur.len() == k {
            return true;
        }
        for i in start..pts.len() {
            if pts.len() - i < k - cur.len() {
                return false;
            }
            if cur.iter().all(|&o| dist3(&pts[o], &pts[i]) >= sep - 0.5) {
                cur.push(i);
                if dfs(pts, i + 1, k, sep, cur) {
                    return true;
                }
                cur.pop();
            }
        }
        false
    }
    k <= pts.len() && dfs(pts, 0, k, sep, &mut Vec::new())
}

/// Most players (1..=8) whose spawns can all meet the spacing rule.
fn capacity(pts: &[[f64; 3]]) -> usize {
    let mut cap = 1;
    for n in 2..=RULE_MAX_PLAYERS {
        if !spaced(pts, n, sep_for(n)) {
            break;
        }
        cap = n;
    }
    cap
}

const BARRIER_WORDS: [&str; 6] =["Fence", "Gate", "Door", "Portcullis", "Bars", "Barrier"];
const BARRIER_CLEAR_CM: f64 = 120.0;
const OVERFLOW_MIN: f64 = 250.0;
const OVERFLOW_MAX: f64 = 900.0;
const OVERFLOW_SEP: f64 = 150.0;
const KILL_Z_MARGIN: f64 = 1500.0;

fn r1(v: f64) -> f64 {
    py_round(v, 1)
}

/// Python truthiness of a JSON value.
fn truthy(v: Option<&Value>) -> bool {
    match v {
        None | Some(Value::Null) | Some(Value::Bool(false)) => false,
        Some(Value::Number(n)) => n.as_f64().map(|x| x != 0.0).unwrap_or(true),
        Some(Value::String(s)) => !s.is_empty(),
        Some(Value::Array(a)) => !a.is_empty(),
        Some(Value::Object(o)) => !o.is_empty(),
        _ => true,
    }
}

fn f(v: &Value) -> f64 {
    v.as_f64().unwrap_or(0.0)
}

fn s<'a>(o: &'a Value, k: &str) -> &'a str {
    o.get(k).and_then(|v| v.as_str()).unwrap_or("")
}

fn arr<'a>(o: &'a Value, k: &str) -> &'a [Value] {
    o.get(k).and_then(|v| v.as_array()).map(|a| a.as_slice()).unwrap_or(&[])
}

fn xyz(v: &Value) -> [f64; 3] {
    let a = v.as_array().map(|a| a.as_slice()).unwrap_or(&[]);
    [a.first().map(f).unwrap_or(0.0), a.get(1).map(f).unwrap_or(0.0), a.get(2).map(f).unwrap_or(0.0)]
}

/// Python 3.12+ `sum()` of floats (Neumaier compensated summation).
fn py_sum(xs: &[f64]) -> f64 {
    let (mut s, mut c) = (0.0f64, 0.0f64);
    for &x in xs {
        let t = s + x;
        if s.abs() >= x.abs() {
            c += (s - t) + x;
        } else {
            c += (x - t) + s;
        }
        s = t;
    }
    if c != 0.0 && c.is_finite() {
        s + c
    } else {
        s
    }
}

fn hypot(a: f64, b: f64) -> f64 {
    a.hypot(b)
}

fn enabled(ep: &Value, key: &str) -> Vec<String> {
    arr(ep, key)
        .iter()
        .filter(|e| truthy(e.get("Value")))
        .map(|e| {
            let k = s(e, "Key");
            let last = k.rsplit('(').next().unwrap_or("");
            last.trim_end_matches(')').to_string()
        })
        .collect()
}

fn barriers(d: &Value) -> Vec<(String, [f64; 3])> {
    let mut out = vec![];
    for o in arr(d, "world_objects").iter().chain(arr(d, "other_actors")) {
        let cls = s(o, "class");
        if (o.get("category").and_then(|v| v.as_str()) == Some("fence") || BARRIER_WORDS.iter().any(|w| cls.contains(w)))
            && truthy(o.get("location"))
        {
            out.push((cls.to_string(), xyz(&o["location"])));
        }
    }
    out
}

fn hazards(d: &Value) -> Vec<[f64; 3]> {
    arr(d, "world_objects")
        .iter()
        .filter(|o| {
            matches!(o.get("category").and_then(|v| v.as_str()), Some("trap") | Some("fence"))
                && truthy(o.get("location"))
                && !o.get("via").and_then(|v| v.as_str()).unwrap_or("").contains("LevelStreamingDynamic")
        })
        .map(|o| xyz(&o["location"]))
        .collect()
}

fn spawn_num(src: &str) -> i64 {
    let d: String = src.chars().filter(|c| c.is_ascii_digit()).collect();
    d.parse().unwrap_or(0)
}

struct Spawn {
    id: String,
    pos: [f64; 3],
    yaw: f64,
    team: i64,
    game_modes: Vec<String>,
    play_modes: Vec<String>,
    valid: bool,
    why: String,
}

struct Overflow {
    id: String,
    pos: [f64; 3],
}

fn spawns(d: &Value) -> Vec<Spawn> {
    let bars = barriers(d);
    let empty = Value::Object(Default::default());
    let mut out = vec![];
    for sp in arr(d, "spawn_points") {
        let ep = sp.get("effective_properties").unwrap_or(&empty);
        let [mut x, mut y, z] = xyz(&sp["location"]);
        let src = s(sp, "name").replace("BP_SpawnerPoint_Willies_C_", "sp");
        let pm = enabled(ep, "Works in these Play Modes");
        let mut why = String::new();
        if pm.is_empty() {
            why = "disabled for every play mode".into();
        } else if x.abs() + y.abs() + z.abs() < 1.0 {
            why = "unplaced template at origin".into();
        } else {
            for (cls, [bx, by, bz]) in &bars {
                let dd = hypot(x - bx, y - by);
                if dd < BARRIER_CLEAR_CM && (z - bz).abs() < 200.0 {
                    if dd < 1.0 {
                        why = format!("on barrier {cls}");
                        break;
                    }
                    let k = (BARRIER_CLEAR_CM - dd) / dd;
                    let nx = x + (x - bx) * k;
                    let ny = y + (y - by) * k;
                    x = nx;
                    y = ny;
                }
            }
        }
        let team = match ep.get("Spawn Team Int") {
            Some(v) if truthy(Some(v)) => v.as_i64().unwrap_or_else(|| f(v).trunc() as i64),
            _ => 0,
        };
        out.push(Spawn {
            id: src,
            pos: [r1(x), r1(y), r1(z)],
            yaw: r1(f(&sp["rotation"]["yaw"])),
            team,
            game_modes: enabled(ep, "Works in these Game Modes"),
            play_modes: pm,
            valid: why.is_empty(),
            why,
        });
    }
    out.sort_by_key(|p| spawn_num(&p.id)); // stable, like Python
    out
}

fn near_hazard(m: &[f64; 3], hz: &[[f64; 3]]) -> bool {
    for [hx, hy, hzz] in hz {
        let dh = hypot(m[0] - hx, m[1] - hy);
        if dh < 150.0 && (m[2] - hzz).abs() < 200.0 {
            return true;
        }
        if dh < 200.0 && m[2] - hzz > 300.0 {
            return true;
        }
    }
    false
}

fn overflow(pts: &[&Spawn], hz: &[[f64; 3]]) -> Vec<Overflow> {
    let mut out: Vec<Overflow> = vec![];
    for i in 0..pts.len() {
        for j in i + 1..pts.len() {
            let (a, b) = (pts[i].pos, pts[j].pos);
            if (a[2] - b[2]).abs() >= 40.0 {
                continue;
            }
            let dist = hypot(a[0] - b[0], a[1] - b[1]);
            if !(OVERFLOW_MIN <= dist && dist <= OVERFLOW_MAX) {
                continue;
            }
            let m = [r1((a[0] + b[0]) / 2.0), r1((a[1] + b[1]) / 2.0), a[2].max(b[2])];
            if near_hazard(&m, hz) {
                continue;
            }
            let ok = pts.iter().map(|q| q.pos).chain(out.iter().map(|q| q.pos)).all(|q| hypot(m[0] - q[0], m[1] - q[1]) >= OVERFLOW_SEP);
            if ok {
                out.push(Overflow { id: format!("mid({},{})", pts[i].id, pts[j].id), pos: m });
            }
        }
    }
    out
}

struct MapData {
    map: String,
    label: String,
    source: String,
    centre: [f64; 3],
    bmin: [f64; 3],
    bmax: [f64; 3],
    kill_z: f64,
    spawns: Vec<Spawn>,
    overflow: Vec<Overflow>,
    max_players: usize,
}

fn min_of(v: &[f64]) -> f64 {
    v.iter().cloned().fold(f64::INFINITY, |a, b| if b < a { b } else { a })
}
fn max_of(v: &[f64]) -> f64 {
    v.iter().cloned().fold(f64::NEG_INFINITY, |a, b| if b > a { b } else { a })
}

fn map_data(src_dir: &Path, name: &str, label: &str) -> Result<MapData> {
    let d = load_static(src_dir, name)?;
    let mut sp = spawns(&d);
    // MP arena-floor points (see MP_ROOM_POINTS): only inside the box of the
    // natively valid points (+50 cm) and only if they were merely flag-disabled.
    if let Some((_, ids)) = MP_ROOM_POINTS.iter().find(|(n, _)| *n == name) {
        let v: Vec<[f64; 3]> = sp.iter().filter(|p| p.valid).map(|p| p.pos).collect();
        let lo = |i: usize| v.iter().map(|p| p[i]).fold(f64::INFINITY, f64::min) - 50.0;
        let hi = |i: usize| v.iter().map(|p| p[i]).fold(f64::NEG_INFINITY, f64::max) + 50.0;
        for p in sp.iter_mut() {
            if ids.contains(&p.id.as_str()) && !p.valid && p.why == "disabled for every play mode" {
                let inside = (0..2).all(|i| p.pos[i] >= lo(i) && p.pos[i] <= hi(i));
                if inside {
                    p.valid = true;
                    p.why = MP_ROOM_WHY.to_string();
                }
            }
        }
    }
    // Geometry rules: a valid point on a roof/ledge or outside the arena is excluded.
    let cand: Vec<(String, [f64; 3])> = sp.iter().filter(|p| p.valid).map(|p| (p.id.clone(), p.pos)).collect();
    for r in geometry(&d, &cand) {
        if let Some(v) = r.verdict {
            if let Some(p) = sp.iter_mut().find(|p| p.id == r.id) {
                p.valid = false;
                p.why = v;
            }
        }
    }
    let sp = sp;
    let valid: Vec<&Spawn> = sp.iter().filter(|p| p.valid).collect();
    let extra = overflow(&valid, &hazards(&d));
    let xs: Vec<f64> = valid.iter().map(|p| p.pos[0]).collect();
    let ys: Vec<f64> = valid.iter().map(|p| p.pos[1]).collect();
    let zs: Vec<f64> = valid.iter().map(|p| p.pos[2]).collect();
    let mut locs: Vec<[f64; 3]> = arr(&d, "world_objects")
        .iter()
        .filter(|o| truthy(o.get("location")))
        .map(|o| xyz(&o["location"]))
        .filter(|l| l[0].abs() + l[1].abs() > 1.0)
        .collect();
    locs.extend(sp.iter().map(|p| p.pos));
    let n = valid.len() as f64;
    let max_players = capacity(&valid.iter().map(|p| p.pos).chain(extra.iter().map(|o| o.pos)).collect::<Vec<_>>());
    let mut bmin = [0.0; 3];
    let mut bmax = [0.0; 3];
    for i in 0..3 {
        let col: Vec<f64> = locs.iter().map(|l| l[i]).collect();
        bmin[i] = r1(min_of(&col));
        bmax[i] = r1(max_of(&col));
    }
    Ok(MapData {
        map: format!("Map_Arena_{name}"),
        label: label.to_string(),
        source: format!("docs/arena_static/Map_Arena_{name}.json"),
        centre: [r1(py_sum(&xs) / n), r1(py_sum(&ys) / n), r1(min_of(&zs))],
        bmin,
        bmax,
        kill_z: r1(min_of(&zs) - KILL_Z_MARGIN),
        spawns: sp,
        max_players,
        overflow: extra,
    })
}

fn v3(p: &[f64; 3]) -> PyVal {
    PyVal::List(p.iter().map(|&x| PyVal::Float(x)).collect())
}

fn strs(v: &[String]) -> PyVal {
    PyVal::List(v.iter().map(|s| PyVal::Str(s.clone())).collect())
}

fn to_py(m: &MapData) -> PyVal {
    let spawns = m
        .spawns
        .iter()
        .map(|p| {
            PyVal::Dict(vec![
                ("id".into(), p.id.as_str().into()),
                ("pos".into(), v3(&p.pos)),
                ("yaw".into(), p.yaw.into()),
                ("team".into(), p.team.into()),
                ("game_modes".into(), strs(&p.game_modes)),
                ("play_modes".into(), strs(&p.play_modes)),
                ("valid".into(), p.valid.into()),
                ("why".into(), p.why.as_str().into()),
            ])
        })
        .collect();
    let overflow =
        m.overflow.iter().map(|p| PyVal::Dict(vec![("id".into(), p.id.as_str().into()), ("pos".into(), v3(&p.pos))])).collect();
    PyVal::Dict(vec![
        ("schema".into(), "hsmp-mapdata/1".into()),
        ("map".into(), m.map.as_str().into()),
        ("label".into(), m.label.as_str().into()),
        ("source".into(), m.source.as_str().into()),
        ("centre".into(), v3(&m.centre)),
        ("bounds".into(), PyVal::Dict(vec![("min".into(), v3(&m.bmin)), ("max".into(), v3(&m.bmax))])),
        ("kill_z".into(), m.kill_z.into()),
        ("spawns".into(), PyVal::List(spawns)),
        ("overflow".into(), PyVal::List(overflow)),
        ("max_players".into(), PyVal::Int(m.max_players as i64)),
    ])
}

fn lua_num(v: f64) -> String {
    fixed(v, 1)
}

fn emit_lua(maps: &[MapData]) -> String {
    let mut l: Vec<String> = vec![
        "-- @generated by `hsmp-tools gen-map-data` (tools/hsmp-tools) from docs/arena_static. DO NOT EDIT.".into(),
        "-- The MP arena catalogue + spawn data (server copy: server/data/maps/*.json;".into(),
        "-- a cargo test keeps both equal). See docs/development/subsystems/spawns.md.".into(),
        "local A = {}".into(),
    ];
    for m in maps {
        let c = &m.centre;
        l.push(format!(
            "A[\"{}\"] = {{ label = \"{}\", kill_z = {}, centre = {{ {} }},",
            m.map,
            m.label,
            lua_num(m.kill_z),
            c.iter().map(|&v| lua_num(v)).collect::<Vec<_>>().join(", ")
        ));
        l.push("  spawns = {".into());
        for p in &m.spawns {
            let [x, y, z] = p.pos;
            l.push(format!(
                "    {{ id = \"{}\", x = {}, y = {}, z = {}, yaw = {}, team = {}, valid = {} }},",
                p.id,
                lua_num(x),
                lua_num(y),
                lua_num(z),
                lua_num(p.yaw),
                p.team,
                if p.valid { "true" } else { "false" }
            ));
        }
        l.push("  },".into());
        l.push("  overflow = {".into());
        for p in &m.overflow {
            let [x, y, z] = p.pos;
            l.push(format!("    {{ id = \"{}\", x = {}, y = {}, z = {} }},", p.id, lua_num(x), lua_num(y), lua_num(z)));
        }
        l.push("  },".into());
        l.push("}".into());
    }
    l.push("return A".into());
    l.join("\n") + "\n"
}

/// (maps, [(path, contents)]) of a fresh run.
fn outputs(root: &Path) -> Result<(Vec<MapData>, Vec<(PathBuf, String)>)> {
    let src = root.join("docs").join("arena_static");
    let out_dir = root.join("server").join("data").join("maps");
    let out_lua = root.join("mods").join("shared").join("hsmp_arenas.lua");
    let maps: Vec<MapData> = ARENAS.iter().map(|(n, l)| map_data(&src, n, l)).collect::<Result<_>>()?;
    let mut files: Vec<(PathBuf, String)> =
        maps.iter().map(|m| (out_dir.join(format!("{}.json", m.map)), dumps_indent(&to_py(m), 1) + "\n")).collect();
    files.push((out_lua, emit_lua(&maps)));
    Ok((maps, files))
}

pub fn run(a: Args) -> Result<i32> {
    let root = paths::repo_root()?;
    if a.report {
        print_report(&root)?;
        if !a.check {
            return Ok(0);
        }
    }
    let (maps, files) = outputs(&root)?;
    if a.check {
        let mut stale = vec![];
        for (p, t) in &files {
            // universal-newline compare (git autocrlf may check files out with CRLF)
            let cur = std::fs::read(p).ok().map(|b| String::from_utf8_lossy(&b).replace("\r\n", "\n"));
            if cur.as_deref() != Some(t.as_str()) {
                stale.push(p);
            }
        }
        for p in &stale {
            println!("stale: {}", paths::rel(&root, p));
        }
        let geo = check_committed(&root)?;
        for g in &geo {
            println!("spawn geometry: {g}");
        }
        println!(
            "{}",
            if stale.is_empty() && geo.is_empty() {
                "map data up to date; every assignable spawn passes the geometry rules"
            } else if stale.is_empty() {
                "a committed spawn fails the geometry rules (fix the source data or the rule)"
            } else {
                "run: hsmp-tools gen-map-data"
            }
        );
        return Ok(if stale.is_empty() && geo.is_empty() { 0 } else { 1 });
    }
    for (p, t) in &files {
        if let Some(d) = p.parent() {
            std::fs::create_dir_all(d)?;
        }
        std::fs::write(p, t.as_bytes()).with_context(|| format!("write {}", p.display()))?;
    }
    for m in &maps {
        let v = m.spawns.iter().filter(|p| p.valid).count();
        let bad: Vec<String> = m.spawns.iter().filter(|p| !p.valid).map(|p| format!("{} ({})", p.id, p.why)).collect();
        println!(
            "{}: {} valid, {} overflow, max_players {} (spawns >= 4 m apart up to 4, >= 3 m up to 8), kill_z {}; excluded: {}",
            m.map,
            v,
            m.overflow.len(),
            m.max_players,
            py_repr(m.kill_z),
            if bad.is_empty() { "none".to_string() } else { bad.join(", ") }
        );
    }
    println!("wrote {} files", files.len());
    Ok(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn sp(name: &str, x: f64, y: f64, z: f64) -> Value {
        json!({"name": format!("BP_SpawnerPoint_Willies_C_{name}"), "location": [x, y, z], "rotation": {"yaw": 0.0},
               "effective_properties": {"Works in these Play Modes": [{"Key": "E::A (Free Mode)", "Value": true}]}})
    }

    fn arena() -> Value {
        json!({
            "spawn_points": [sp("0", 0.0, 0.0, 10.0), sp("1", 400.0, 0.0, 12.0), sp("2", 0.0, 400.0, 8.0),
                             sp("3", -400.0, 0.0, 10.0), sp("4", 0.0, -400.0, 330.0), sp("5", 9000.0, 0.0, 10.0)],
            "world_objects": [
                {"class": "BP_Weapon_Tool_Shovel_A_C", "category": "weapon", "location": [50.0, -380.0, 5.0]},
                {"class": "BP_Fence_Flimsy_Small_C", "category": "fence", "location": [9050.0, 30.0, 8.0]},
                {"class": "BP_Fence_Flimsy_Small_C", "category": "fence", "location": [60.0, -420.0, 2.0],
                 "via": "streaming:LevelStreamingDynamic_3"}
            ]
        })
    }

    #[test]
    fn spawn_on_a_ledge_and_outside_the_arena_are_rejected() {
        let d = arena();
        let pts: Vec<(String, [f64; 3])> = spawns(&d).iter().map(|p| (p.id.clone(), p.pos)).collect();
        let rows = geometry(&d, &pts);
        let v = |id: &str| rows.iter().find(|r| r.id == id).and_then(|r| r.verdict.clone());
        assert!(v("sp0").is_none() && v("sp1").is_none() && v("sp2").is_none() && v("sp3").is_none());
        let ledge = v("sp4").expect("sp4 is 3.2 m above the shovel lying under it");
        assert!(ledge.contains("above the local floor") && ledge.contains("roof/ledge"), "{ledge}");
        let out = v("sp5").expect("sp5 is 90 m away");
        assert!(out.contains("outside the arena"), "{out}");
    }

    #[test]
    fn dynamic_sublevel_objects_are_not_floor_evidence() {
        let d = arena();
        let ev = floor_evidence(&d);
        assert!(!ev.iter().any(|(_, p)| p[2] == 2.0), "the Narrow-Passage style sublevel fence is ignored");
        let f = local_floor(&ev, &[0.0, -400.0, 330.0]).unwrap();
        assert_eq!(f.0, 5.0);
    }

    #[test]
    fn spacing_rule_capacity_and_violations() {
        // a 3 x 3 grid, 350 cm pitch: every pair >= 350 cm
        let mut g = vec![];
        for i in 0..3 {
            for j in 0..3 {
                g.push([i as f64 * 350.0, j as f64 * 350.0, 0.0]);
            }
        }
        assert_eq!(capacity(&g), 8, "4 corners are 700 cm apart (2-4 players) and any 8 are >= 350 (5-8)");
        let small = [[0.0, 0.0, 0.0], [390.0, 0.0, 0.0], [195.0, 100.0, 0.0]];
        assert_eq!(capacity(&small), 1, "3.9 m is not 4 m: not even a duel");
        let v = spacing_violations("Yard", &small, Some(&json!(1)));
        assert!(v.iter().any(|s| s.contains("no 2 spawns are 400 cm apart")), "{v:?}");
        let v = spacing_violations("Yard", &g, Some(&json!(7)));
        assert!(v.len() == 1 && v[0].contains("max_players Some(7) in the map file, 8 computed"), "{v:?}");
        assert!(spacing_violations("Yard", &g, Some(&json!(8))).is_empty());
        let v = spacing_violations("Cellar", &g, Some(&json!(8)));
        assert!(v.iter().any(|s| s.contains("CAPACITY_FLOOR 4 is stale")), "{v:?}");
    }

    #[test]
    fn mp_room_points_are_assignable_and_spaced() {
        let root = paths::repo_root().unwrap();
        let src = root.join("docs").join("arena_static");
        let cellar = map_data(&src, "Cellar", "Cellar").unwrap();
        for id in ["sp4", "sp5", "sp7", "sp8"] {
            let p = cellar.spawns.iter().find(|p| p.id == id).unwrap();
            assert!(p.valid && p.why == MP_ROOM_WHY, "Cellar {id}: {}", p.why);
        }
        for id in ["sp6", "sp9", "sp10"] {
            assert!(!cellar.spawns.iter().find(|p| p.id == id).unwrap().valid, "Cellar side room {id} stays out");
        }
        assert_eq!(cellar.max_players, 4);
        let pit = map_data(&src, "Pit", "Pit").unwrap();
        for p in &pit.spawns {
            let n = spawn_num(&p.id);
            assert_eq!(p.valid, n <= 7 || n == 10, "Pit {}", p.id);
        }
        assert_eq!(pit.max_players, 8);
    }

    #[test]
    fn committed_map_data_passes_the_geometry_rules() {
        let root = paths::repo_root().unwrap();
        let bad = check_committed(&root).unwrap();
        assert!(bad.is_empty(), "{bad:?}");
    }
}
