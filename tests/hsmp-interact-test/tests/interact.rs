//! Offline tests of the HSMPInteract mod (interaction channel) under Lua 5.4
//! (mlua) with a mocked UE4SS (`tests/mock_ue.lua`). No game needed.
//!
//!     cargo test -p hsmp-interact-test
//!
//! * `interact_core.lua`: bone-name map (and the wire index = the schema's
//!   `hero_bone` table), bone-frame round trip, the typed `interact` record
//!   tables both ways, impulse accumulator (threshold, window, damage dedupe),
//!   application clamps, the `pose_yield` bus record.
//! * detection -> record (`main.lua` + mocks): my pawn's grab state on a
//!   stand-in -> typed `interact` sends (grab_start / grab_update / grab_end)
//!   with the bone-frame grip and the yield request; body-hit hook contacts ->
//!   impulse records (oriented away from me, summed, deduped against damage
//!   hits, ignored for weapons, for my own pawn and for non-stand-ins); server
//!   denials stop the updates and the yield.
//! * record -> application: impulses land on MY simulated mesh at the bone
//!   point with the caps; grabs attach a soft, pooled PhysicsHandle on my
//!   bone pulled toward the grabber's STAND-IN hand (lead-clamped), and are
//!   released on grab_end, lease timeout, breaking free and pawn change.
//! * world guard: a level change drops every cache without touching a freed
//!   object, and events queued meanwhile are skipped; nothing runs without an
//!   MP session.
//!
//! The sidecar's side is the typed HSMPNative mock (lua-tests/lib/
//! hsmp_native_records.lua): `HSMPNative._rec.sends` are the G2S records the
//! mod sent, `HSMPNative.sc_rec_event("interact", t, peer)` injects an S2G
//! record, `HSMPNative.sc_get("pose_yield")` reads the bus record. Session,
//! match, puppets and spawn status stay on the file mock.

use mlua::{FromLuaMulti, Lua};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};

const MOCK: &str = include_str!("mock_ue.lua");
const BODY_HIT: &str = "/Game/Character/Blueprints/Willie_BP.Willie_BP_C:BndEvt__BP_ThirdPersonCharacter_Mesh_K2Node_ComponentBoundEvent_0_ComponentHitSignature__DelegateSignature";
const GET_DAMAGE: &str = "/Game/Character/Blueprints/Willie_BP.Willie_BP_C:Get Damage";

/// Test-side helpers over the typed mock (Lua).
const HELPERS: &str = r#"
    IX = require('interact_core')
    -- The `interact` records the mod sent (data tables), optionally of one kind.
    function OUTS(kind)
        local r = {}
        for _, m in ipairs(HSMPNative._rec.sends) do
            if m.kind == "interact" and (kind == nil or m.data.kind == IX.KIND[kind]) then r[#r + 1] = m.data end
        end
        return r
    end
    -- One S2G `interact` record from `from` (0 = the server).
    function IN(from, kind, id, hand, bone, pt, v, ts, extra)
        local t = { kind = IX.KIND[kind] or kind, id = id, hand = hand,
                    bone = (type(bone) == "string") and IX.BONE_INDEX[bone] or bone,
                    point = pt or { 0, 0, 0 }, vector = v or { 0, 0, 0 }, ts = ts or 0 }
        for k, x in pairs(extra or {}) do t[k] = x end
        HSMPNative.sc_rec_event("interact", t, from)
    end
    -- Rows of the pose_yield bus record (nil = never written).
    function YIELD()
        local t = HSMPNative.sc_get("pose_yield")
        return t and t.rows
    end
"#;

fn scripts_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../mods/HSMPInteract/Scripts")
}
fn lua_path(p: &Path) -> String { p.to_string_lossy().replace('\\', "/") }

static DIR_N: AtomicU32 = AtomicU32::new(0);

struct T {
    lua: Lua,
    sd: PathBuf,
    tick: u32,
}

impl Drop for T {
    fn drop(&mut self) { let _ = fs::remove_dir_all(&self.sd); }
}

/// One `interact` record the mod sent.
#[derive(Debug, Clone, Copy)]
struct Rec {
    target: f64,
    gid: f64,
    hand: f64,
    bone: f64,
    pt: [f64; 3],
    v: [f64; 3],
    ts: f64,
}

impl T {
    /// Mocked world: me = peer 1 (Willie_BP_C_100 at the origin, facing +x,
    /// simulated "SK_Skeleton"), peer 2's stand-in Willie_BP_C_200 60 uu in
    /// front, facing me; peer 3's stand-in far away. Match live.
    fn new() -> T { T::new_settle(0.0) }
    /// `settle_s`: shared/hsmp_wg SETTLE_S for this run. The generic
    /// tests use 0 so their hard-coded timestamps keep their meaning; the
    /// settle test runs the real 2 s.
    fn new_settle(settle_s: f64) -> T {
        let sd = std::env::temp_dir().join(format!("hsmp_interact_{}_{}", std::process::id(),
                                                   DIR_N.fetch_add(1, Ordering::SeqCst)));
        let _ = fs::remove_dir_all(&sd);
        fs::create_dir_all(&sd).unwrap();
        let lua = Lua::new();
        lua.load(MOCK).set_name("@mock_ue.lua").exec().expect("mock loads");
        let t = T { lua, sd, tick: 0 };
        t.exec(&format!(r#"
            M.env.HSMP_STATE_DIR = "{sd}"
            ME, MY = M.willie("Willie_BP_C_100", {{0, 0, 100}}, 1, "SK_Skeleton")
            M.pc.Pawn = ME
            FOE, FOE_MESH = M.willie("Willie_BP_C_200", {{60, 0, 100}}, -1, "Mesh")
            FAR, FAR_MESH = M.willie("Willie_BP_C_300", {{900, 0, 100}}, -1, "Mesh")
            PROP = new_obj("StaticMeshComponent", "Barrel_Mesh")
            rawset(PROP, "owner", new_obj("BP_Barrel_C", "BP_Barrel_C_1"))
            SWORD = new_obj("ModularWeaponBP_C", "ModularWeaponBP_C_7")
            FLOOR_ACTOR = new_obj("StaticMeshActor", "Floor_12")
            FLOOR = new_obj("StaticMeshComponent", "Floor_12_Mesh")
            rawset(FLOOR, "owner", FLOOR_ACTOR)
            MYSWORD = new_obj("ModularWeaponBP_C", "ModularWeaponBP_C_3")
            MYSWORD_MESH = new_obj("StaticMeshComponent", "ModularWeaponBP_C_3_Blade")
            rawset(MYSWORD_MESH, "owner", MYSWORD)
            ME["Weapon R"] = MYSWORD
            -- I step into the stand-in at 3 m/s (closing velocity for shoves)
            rawset(MY, "vel", {{ 300, 0, 0 }})
        "#, sd = lua_path(&t.sd)));
        let sp = lua_path(&scripts_dir());
        t.exec(&format!("package.path = \"{sp}/?.lua;{sp}/../../shared/?.lua;\" .. package.path"));
        // the file-backed HSMPNative mock with the typed-record mixin (tools/hsmp-tools/lua-tests/lib)
        t.exec(&format!("HSMP_TEST_LIB = \"{sp}/../../../tools/hsmp-tools/lua-tests/lib\"; HSMPNative = dofile(HSMP_TEST_LIB .. \"/hsmp_native_filemock.lua\").new()"));
        // HSMPAvatars' stand-ins (bus key "puppets", typed rows)
        t.exec(r#"HSMPNative.bus_put("puppets", { rows = { { peer = 2, name = "Willie_BP_C_200" }, { peer = 3, name = "Willie_BP_C_300" } } })"#);
        t.match_state("live", 1);
        if settle_s >= 0.0 { t.exec(&format!("require('hsmp_wg').SETTLE_S = {settle_s}")); }
        let mut t = t;
        t.session(true);
        let main = fs::read_to_string(scripts_dir().join("main.lua")).expect("main.lua");
        t.lua.load(&main).set_name(format!("@{sp}/main.lua")).exec().expect("main.lua runs");
        t.exec(HELPERS);
        // world guard settles (first check drops), hooks registered; the
        // shared liveness rule (link CONNECTED + a fresh header heartbeat) is
        // seen by the 1 s session refresh (1110: the 33 ms tick still follows the
        // first 8 ms frame of a run(16))
        t.run(1110);
        t
    }

    fn ev<R: FromLuaMulti>(&self, e: &str) -> R {
        self.lua.load(format!("return {e}")).eval::<R>().unwrap_or_else(|x| panic!("eval `{e}`: {x}"))
    }
    fn n(&self, e: &str) -> f64 { self.ev::<f64>(&format!("tonumber({e}) or -1")) }
    fn s(&self, e: &str) -> String { self.ev::<String>(&format!("tostring({e})")) }
    fn exec(&self, code: &str) { self.lua.load(code).exec().unwrap_or_else(|e| panic!("exec `{code}`: {e}")) }

    /// One S2G `interact` record (Lua call arguments after `from`).
    fn inject(&self, from: u32, args: &str) { self.exec(&format!("IN({from}, {args})")); }
    /// Every `interact` record sent, of `kind` ("" = any).
    fn outs(&self, kind: &str) -> Vec<Rec> {
        let q = if kind.is_empty() { "OUTS()".to_string() } else { format!("OUTS('{kind}')") };
        let n = self.n(&format!("#{q}")) as usize;
        (1..=n).map(|i| {
            let f = |k: &str| self.n(&format!("{q}[{i}].{k}"));
            let a = |k: &str| [self.n(&format!("{q}[{i}].{k}[1]")), self.n(&format!("{q}[{i}].{k}[2]")), self.n(&format!("{q}[{i}].{k}[3]"))];
            Rec { target: f("target_peer"), gid: f("gid"), hand: f("hand"), bone: f("bone"), pt: a("point"), v: a("vector"), ts: f("ts") }
        }).collect()
    }
    fn count(&self, kind: &str) -> usize { self.n(&format!("#OUTS('{kind}')")) as usize }
    /// The pose_yield rows: (peer, until_ms, gain, cap_lin, cap_ang); None = never written.
    fn yields(&self) -> Option<Vec<(f64, f64, f64, f64, f64)>> {
        if self.s("YIELD()") == "nil" { return None; }
        let n = self.n("#YIELD()") as usize;
        Some((1..=n).map(|i| {
            let f = |k: &str| self.n(&format!("YIELD()[{i}].{k}"));
            (f("peer"), f("until_ms"), f("gain"), f("cap_lin"), f("cap_ang"))
        }).collect())
    }
    fn no_yield(&self) -> bool { self.yields().map_or(true, |y| y.is_empty()) }

    /// The sidecar's typed link record (sidecar_status CONNECTED = 1 /
    /// CONNECTING = 0, link_state UP = 1 / CONNECTING = 0) and a fresh header heartbeat.
    fn session(&mut self, connected: bool) {
        self.tick += 1;
        let c = if connected { 1 } else { 0 };
        self.exec(&format!("HSMPNative.sc_put('link', {{ status = {c}, state = {c}, my_peer_id = 1 }}); HSMPNative._st.hb_age = 0.01"));
        self.exec("HSMPNative.sc_peer_dir({ { id = 2, nick = 'Bravo' } })");
    }
    /// The server's session snapshot (legacy state name -> phase code).
    fn match_state(&self, state: &str, round: u32) {
        let phase = match state { "countdown" => 2, "live" => 3, "roundover" => 4, "match_over" => 5, "paused" => 7, _ => 0 };
        self.exec(&format!("HSMPNative.sc_put('session', {{ phase = {phase}, round = {round}, winner_seat = 255 }})"));
    }
    /// HSMPSync's spawn_status bus record (`until` None = unbounded until Live).
    fn spawn_status(&self, pawn: &str, until: Option<f64>) {
        let (has, pu) = match until { Some(u) => ("true", u), None => ("false", 0.0) };
        self.exec(&format!("HSMP_IPC.bus_put('spawn_status', {{ seq = 1, round = 1, verified = true, pawn = '{pawn}', has_protect_until = {has}, protect_until = {pu} }})"));
    }
    fn run(&mut self, ms: u32) {
        let mut left = ms;
        while left > 0 {
            let step = left.min(500);
            self.session(true);
            self.exec(&format!("M.run({step})"));
            left -= step;
        }
    }
    fn logs(&self) -> String { self.ev::<String>("M.logtext()") }
}

fn near(a: [f64; 3], b: [f64; 3], tol: f64) -> bool { (0..3).all(|i| (a[i] - b[i]).abs() <= tol) }

const LOWERARM_L: f64 = 5.0;
const HEAD: f64 = 3.0;
const SPINE_04: f64 = 2.0;
const FOOT_R: f64 = 15.0;

// --- pure core ------------------------------------------------------------------------------

fn core() -> Lua {
    let lua = Lua::new();
    let sp = lua_path(&scripts_dir());
    lua.load(format!("package.path = \"{sp}/?.lua;\" .. package.path; C = require('interact_core'); \
                      S = dofile(\"{sp}/../../shared/hsmp_ipc_schema.lua\")")).exec().unwrap();
    lua
}

#[test]
fn core_bone_map_and_frames() {
    let lua = core();
    let s = |e: &str| lua.load(format!("return tostring({e})")).eval::<String>().unwrap();
    for (raw, hero) in [("lowerarm_l", "Lowerarm_L"), ("Hand_R", "Hand_R"), ("upperarm_twist_01_r", "Upperarm_R"),
                        ("clavicle_l", "Upperarm_L"), ("index_02_r", "Hand_R"), ("neck_01", "Head"),
                        ("spine_01", "Pelvis"), ("spine_03", "Spine_04"), ("ball_l", "Foot_L"),
                        ("calf_twist_01_r", "Calf_R"), ("thigh_l", "Thigh_L"), ("pelvis", "Pelvis")] {
        assert_eq!(s(&format!("C.map_bone('{raw}')")), hero, "{raw}");
    }
    for raw in ["None", "", "weapon_socket", "tail"] {
        assert_eq!(s(&format!("C.map_bone('{raw}')")), "nil", "{raw}");
    }
    // World -> bone frame -> world is exact for a rotated bone.
    let ok: bool = lua.load(r#"
        local q = { 0.2, -0.4, 0.3, math.sqrt(1 - 0.04 - 0.16 - 0.09) }
        local bp, p = { 10, -20, 130 }, { 25, 5, 150 }
        local l = C.to_local(bp, q, p)
        local back = C.to_world(bp, q, l)
        local r90 = C.qrot({ 0, 0, math.sqrt(0.5), math.sqrt(0.5) }, { 1, 0, 0 })   -- 90 deg about Z
        return C.dist(back, p) < 1e-6 and math.abs(C.len(l) - C.dist(p, bp)) < 1e-6
           and math.abs(r90[1]) < 1e-9 and math.abs(r90[2] - 1) < 1e-9
    "#).eval().unwrap();
    assert!(ok);
}

#[test]
fn core_records_both_ways_and_the_schema_tables() {
    let lua = core();
    let ok: bool = lua.load(r#"
        -- the wire index of a bone and the kind codes are the schema's (schema/interact.rs)
        for i, b in ipairs(C.HERO) do
            assert(S.ENUMS.hero_bone[b] == i - 1 and C.bone_index(b) == i - 1 and C.bone_name(i - 1) == b, b)
        end
        for name, k in pairs(C.KIND) do assert(S.ENUMS.interact_kind[name:upper()] == k, name) end
        assert(C.bone_name(16) == nil and C.bone_name(-1) == nil and C.bone_name(1.5) == nil)
        -- G2S record tables
        local r = C.grab_rec("grab_start", 2, 7, 1, "Lowerarm_L", {1, 2.5, -3}, {10, 20, 30}, 1234.9)
        assert(r.kind == 1 and r.target_peer == 2 and r.gid == 7 and r.hand == 1 and r.bone == 5
               and r.point[2] == 2.5 and r.vector[3] == 30 and r.ts == 1234)
        local u = C.grab_rec("grab_update", 2, 7, 1, "Lowerarm_L", {1, 2.5, -3}, {10, 20, 30}, 5)
        assert(u.kind == 2)
        local e = C.grab_end_rec(2, 7, 0, 5)
        assert(e.kind == 3 and e.gid == 7 and e.hand == 0 and e.target_peer == 2 and e.ts == 5)
        local i = C.impulse_rec(3, "Head", {0, 0, 0}, {5, 0, 0}, 9)
        assert(i.kind == 4 and i.target_peer == 3 and i.bone == 3 and i.vector[1] == 5 and i.ts == 9)
        -- S2G records -> events
        local ev = C.from_rec(2, { kind = 1, id = 12, hand = 0, bone = 5, point = {1, -2.5, 0},
                                   vector = {100, 200, 300}, ts = 999, gid = 0, target_peer = 1 })
        assert(ev.kind == "grab_start" and ev.from == 2 and ev.id == 12 and ev.bone == "Lowerarm_L"
               and ev.pt[2] == -2.5 and ev.v[3] == 300 and ev.ts == 999 and ev.gid == nil)
        local d = C.from_rec(0, { kind = 5, target_peer = 2, id = 3, gid = 41, hand = 1, bone = 0,
                                  point = {0, 0, 0}, vector = {0, 0, 0} })
        assert(d.kind == "grab_denied" and d.from == 0 and d.gid == 41 and d.target == 2 and d.hand == 1)
        -- refused: unknown bone / hand / kind, no sender, not a table
        local ok_t = { kind = 4, id = 1, hand = 0, bone = 3, point = {0, 0, 0}, vector = {1, 0, 0} }
        assert(C.from_rec(2, ok_t) ~= nil)
        local function with(k, v) local t = {}; for a, b in pairs(ok_t) do t[a] = b end; t[k] = v; return t end
        assert(C.from_rec(2, with("bone", 16)) == nil, "bone")
        assert(C.from_rec(2, with("hand", 5)) == nil, "hand")
        assert(C.from_rec(2, with("kind", 9)) == nil, "kind")
        assert(C.from_rec(nil, ok_t) == nil, "from")
        assert(C.from_rec(2, "garbage") == nil)
        -- a server answer needs no bone
        assert(C.from_rec(0, with("bone", 16)) ~= nil)
        return true
    "#).eval().unwrap();
    assert!(ok);
}

#[test]
fn core_accumulator_threshold_window_and_dedupe() {
    let lua = core();
    let ok: bool = lua.load(r#"
        local A = C.new_acc({ min = 1500, window = 50 })
        local function f(now, frame, dmg) return C.acc_flush(A, now, frame, dmg) end
        -- one strong contact: waits for the next frame, then goes at once
        C.acc_add(A, 2, {2000,0,0}, {60,0,125}, "Spine_02", 1000, 10)
        assert(#f(1000, 10) == 0, "same frame waits")
        local o = f(1008, 11)
        assert(#o == 1 and o[1].peer == 2 and o[1].bone == "Spine_02" and o[1].v[1] == 2000 and o[1].ts == 1000)
        -- the window: contacts within 50 ms are summed, sent when it closes
        C.acc_add(A, 2, {700,0,0}, {60,0,125}, "Spine_02", 1010, 12)
        C.acc_add(A, 2, {900,100,0}, {60,0,150}, "Spine_04", 1020, 13)
        assert(#f(1040, 15) == 0, "window open")
        o = f(1060, 17)
        assert(#o == 1 and o[1].v[1] == 1600 and o[1].v[2] == 100 and o[1].bone == "Spine_04", "summed")
        -- weak touches add up within one window, then expire
        C.acc_add(A, 3, {600,0,0}, {0,0,0}, "Head", 2000, 30)
        C.acc_add(A, 3, {600,0,0}, {0,0,0}, "Head", 2010, 31)
        assert(#f(2020, 32) == 0, "1200 < min")
        C.acc_add(A, 3, {600,0,0}, {0,0,0}, "Head", 2030, 33)
        o = f(2040, 34)
        assert(#o == 1 and o[1].v[1] == 1800, "slow push adds up")
        C.acc_add(A, 4, {100,0,0}, {0,0,0}, "Head", 3000, 40)
        assert(#f(3100, 50) == 0 and A.peers[4] == nil, "weak touch expired")
        -- a contact in the frame of a damage hit is dropped (combat owns it)
        C.acc_add(A, 5, {5000,0,0}, {0,0,0}, "Head", 4000, 60)
        o = f(4010, 61, function(peer, f0, f1) return peer == 5 and f0 <= 60 and 60 <= f1 end)
        assert(#o == 1 and o[1].suppressed and A.peers[5] == nil, "deduped")
        -- sustained contact every frame still flushes after one window
        for i = 0, 7 do C.acc_add(A, 6, {300,0,0}, {0,0,0}, "Pelvis", 5000 + i * 8, 70 + i) end
        o = f(5056, 77)
        assert(#o == 1 and o[1].peer == 6 and o[1].v[1] == 2400, "sustained")
        -- orientation: the impulse always pushes the stand-in away from me
        local v = C.orient_away({-100,0,0}, {0,0,0}, {60,0,0})
        assert(v[1] == 100)
        return true
    "#).eval().unwrap();
    assert!(ok);
}

#[test]
fn core_application_clamps_and_yield_record() {
    let lua = core();
    let ok: bool = lua.load(r#"
        local v, l, capped = C.apply_impulse({3000,0,0}, 1.0, 15000, 10, 800)
        assert(v[1] == 3000 and not capped)
        v, l, capped = C.apply_impulse({90000,0,0}, 1.0, 15000, 10, 800)
        assert(capped and math.abs(l - 8000) < 1e-6 and math.abs(v[1] - 8000) < 1e-6, "mass cap")
        v, l, capped = C.apply_impulse({0,90000,0}, 1.0, 15000, 100, 800)
        assert(capped and math.abs(v[2] - 15000) < 1e-6, "magnitude cap")
        v = C.apply_impulse({1000,0,0}, 0.5, 15000, nil, 800)
        assert(v[1] == 500, "gain, unknown mass")
        local t, d = C.grab_target({0,0,0}, {100,0,0}, 40)
        assert(t[1] == 40 and d == 100, "lead clamp")
        t, d = C.grab_target({0,0,0}, {10,0,0}, 40)
        assert(t[1] == 10 and d == 10)
        local y = {}
        C.yield_merge(y, 2, 1300, 0.35, 300, 300)
        C.yield_merge(y, 2, 1200, 0.15, 150, 150)
        C.yield_merge(y, 3, 900, 0.15, 150, 150)
        local r = C.yield_rec(y)
        assert(#r.rows == 2, "one row per peer")
        local a, b = r.rows[1], r.rows[2]
        assert(a.peer == 2 and a.until_ms == 1300 and a.gain == 0.15 and a.cap_lin == 150 and a.cap_ang == 150, "merged: latest, softest")
        assert(b.peer == 3 and b.until_ms == 900)
        assert(C.yield_same(r, C.yield_rec(y)) and not C.yield_same(r, { rows = {} }) and not C.yield_same(r, nil))
        assert(C.yield_prune(y, 1000) and y[3] == nil and y[2] ~= nil)
        assert(not C.yield_same(r, C.yield_rec(y)), "a pruned row is a change")
        assert(not C.yield_prune(y, 1300) and next(y) == nil)
        return true
    "#).eval().unwrap();
    assert!(ok);
}

// --- detection -> record ---------------------------------------------------------------------

#[test]
fn grab_on_a_standin_streams_start_updates_end_and_yields() {
    let mut t = T::new();
    assert!(t.logs().contains("body hit hook registered"), "{}", t.logs());
    // The stand-in's forearm is rotated 90 deg about Z (bone frame != world).
    t.exec("rawset(FOE_MESH, 'quats', { Lowerarm_L = { 0, 0, math.sqrt(0.5), math.sqrt(0.5) } })");
    // I grab its left forearm with my right hand.
    t.exec("ME['Grabbed R'] = true; ME['Grab Component R'] = FOE_MESH; ME['Grab Bone R'] = fname('lowerarm_l')");
    t.run(16);
    let s = t.outs("grab_start");
    assert_eq!(s.len(), 1, "{:?}", t.outs(""));
    let r = s[0];
    assert!(r.target == 2.0 && r.hand == 0.0 && r.bone == LOWERARM_L && r.gid == 1.0, "{r:?}");
    // grip = my Hand_R (45,-20,140) in the frame of its Lowerarm_L (40,-25,140), rotated 90 deg:
    // world delta (5,5,0) -> local (5,-5,0).
    assert!(near(r.pt, [5.0, -5.0, 0.0], 0.01), "{r:?}");
    assert!(near(r.v, [45.0, -20.0, 140.0], 0.01), "hand world position: {r:?}");
    // While held: updates every ~100 ms and a yield request for peer 2.
    t.run(450);
    let u = t.count("grab_update");
    assert!((3..=5).contains(&u), "{u} updates in 450 ms");
    assert!(t.outs("grab_update").iter().all(|x| x.gid == 1.0 && x.bone == LOWERARM_L));
    let y = t.yields().expect("pose_yield written");
    assert_eq!(y.len(), 1, "{y:?}");
    let (peer, until, gain, lin, ang) = y[0];
    assert!(peer == 2.0 && (gain - 0.15).abs() < 1e-6 && lin == 150.0 && ang == 150.0, "{y:?}");
    assert!(until > t.n("M.now") && until <= t.n("M.now") + 300.0, "yield until {until}");
    // Let go: one grab_end, the yield rows disappear shortly after.
    t.exec("ME['Grabbed R'] = false");
    t.run(16);
    let e = t.outs("grab_end");
    assert_eq!(e.len(), 1);
    assert!(e[0].target == 2.0 && e[0].gid == 1.0, "{:?}", e[0]);
    t.run(200);
    assert_eq!(t.yields(), Some(vec![]), "yield cleared (an empty pose_yield record)");
    // A second grab gets a new gid.
    t.exec("ME['Grabbed L'] = true; ME['Grab Component L'] = FOE_MESH; ME['Grab Bone L'] = fname('head')");
    t.run(16);
    let s = t.outs("grab_start");
    assert!(s.len() == 2 && s[1].gid == 2.0 && s[1].hand == 1.0 && s[1].bone == HEAD, "{s:?}");
    assert!(!t.logs().contains(" error (#"), "{}", t.logs());
    assert_eq!(t.s("next(HSMPNative._rec.refused)"), "nil", "every record marshals");
}

#[test]
fn grabs_on_props_or_while_denied_send_nothing_more() {
    let mut t = T::new();
    t.exec("ME['Grabbed R'] = true; ME['Grab Component R'] = PROP; ME['Grab Bone R'] = fname('None')");
    t.run(200);
    assert!(t.outs("").is_empty(), "prop grab: {:?}", t.outs(""));
    // Unknown skeleton bone name: the nearest hero bone to my hand is used.
    t.exec("ME['Grab Component R'] = FOE_MESH; ME['Grab Bone R'] = fname('weird_bone_99')");
    t.run(16);
    let s = t.outs("grab_start");
    // (my Hand_R (45,-20,140) is 7 uu from the stand-in's Lowerarm_L (40,-25,140))
    assert!(s.len() == 1 && s[0].bone == LOWERARM_L, "{s:?}");
    // The server denies it (peer 0, our gid back): no more updates, the yield stops at once.
    t.inject(0, r#""grab_denied", 1, 0, 0, nil, nil, 0, { target_peer = 2, gid = 1 }"#);
    t.run(16);
    let n = t.count("grab_update");
    t.run(400);
    assert_eq!(t.count("grab_update"), n, "no updates after the denial");
    assert!(t.no_yield(), "no yield for a denied grab: {:?}", t.yields());
    // Releasing a denied grab sends no end (the server already closed it).
    t.exec("ME['Grabbed R'] = false");
    t.run(16);
    assert_eq!(t.count("grab_end"), 0);
    assert!(t.logs().contains("denied by the server"));
}

#[test]
fn body_contacts_on_standins_become_impulses() {
    let mut t = T::new();
    // My body hits peer 2's stand-in: normal impulse reported toward me (-x).
    t.exec(&format!(r#"
        M.fire("{BODY_HIT}", M.param(FOE), M.param(FOE_MESH), M.param(ME), M.param(MY),
               M.param({{ X = -2000, Y = 0, Z = 0 }}),
               M.param({{ MyBoneName = fname("spine_03"), ImpactPoint = {{ X = 55, Y = 0, Z = 150 }} }}))
    "#));
    t.run(16);
    let i = t.outs("impulse");
    assert_eq!(i.len(), 1, "{:?}\n{}", t.outs(""), t.logs());
    assert!(i[0].target == 2.0 && i[0].bone == SPINE_04, "{:?}", i[0]);
    assert!(near(i[0].v, [2000.0, 0.0, 0.0], 0.01), "oriented away from me: {:?}", i[0]);
    // Point in the stand-in's Spine_04 frame (60,0,150): (-5,0,0).
    assert!(near(i[0].pt, [-5.0, 0.0, 0.0], 0.01), "{:?}", i[0]);
    assert!(i[0].ts > 0.0 && i[0].ts <= t.n("M.now"), "my clock: {}", i[0].ts);
    // A shove also asks the stand-in to yield briefly.
    let y = t.yields().unwrap();
    assert!(y.len() == 1 && (y[0].2 - 0.35).abs() < 1e-6 && y[0].3 == 300.0 && y[0].4 == 300.0, "{y:?}");

    // Not mine / not a stand-in / weak / weapon: nothing.
    let n0 = t.count("impulse");
    t.run(100);
    t.exec(&format!(r#"
        local hit = M.param({{ MyBoneName = fname("head"), ImpactPoint = {{ X = 60, Y = 0, Z = 175 }} }})
        M.fire("{BODY_HIT}", M.param(FOE), M.param(FOE_MESH), M.param(SWORD), M.param(SWORD), M.param({{ X = 9000, Y = 0, Z = 0 }}), hit)
        M.fire("{BODY_HIT}", M.param(ME), M.param(MY), M.param(FOE), M.param(FOE_MESH), M.param({{ X = 9000, Y = 0, Z = 0 }}), hit)
        M.fire("{BODY_HIT}", M.param(PROP), M.param(PROP), M.param(ME), M.param(MY), M.param({{ X = 9000, Y = 0, Z = 0 }}), hit)
        M.fire("{BODY_HIT}", M.param(FOE), M.param(FOE_MESH), M.param(ME), M.param(MY), M.param({{ X = 400, Y = 0, Z = 0 }}), hit)
    "#));
    t.run(200);
    assert_eq!(t.count("impulse"), n0, "{:?}", t.outs(""));

    // A real damage hit from my body in the same frame (live round): dropped.
    t.exec(&format!(r#"
        M.fire("{BODY_HIT}", M.param(FOE), M.param(FOE_MESH), M.param(ME), M.param(MY), M.param({{ X = 3000, Y = 0, Z = 0 }}),
               M.param({{ MyBoneName = fname("head"), ImpactPoint = {{ X = 60, Y = 0, Z = 175 }} }}))
        M.fire("{GET_DAMAGE}", M.param(FOE), nil, nil, nil, nil, M.param(fname("head")), M.param(25.0),
               nil, nil, nil, nil, nil, nil, M.param(MY))
    "#));
    t.run(100);
    assert_eq!(t.count("impulse"), n0, "deduped against the damage hit");
    // Same in the lobby: damage is not replicated there, so the shove goes.
    t.match_state("lobby", 0);
    t.run(600);
    t.exec(&format!(r#"
        M.fire("{BODY_HIT}", M.param(FOE), M.param(FOE_MESH), M.param(ME), M.param(MY), M.param({{ X = 3000, Y = 0, Z = 0 }}),
               M.param({{ MyBoneName = fname("head"), ImpactPoint = {{ X = 60, Y = 0, Z = 175 }} }}))
        M.fire("{GET_DAMAGE}", M.param(FOE), nil, nil, nil, nil, M.param(fname("head")), M.param(25.0),
               nil, nil, nil, nil, nil, nil, M.param(MY))
    "#));
    t.run(30);
    assert_eq!(t.count("impulse"), n0 + 1);
    assert_eq!(t.outs("impulse").last().unwrap().bone, HEAD);
    assert!(!t.logs().contains(" error (#"), "{}", t.logs());
}

/// One body-hit hook call on peer 2's stand-in: (other actor, other component,
/// impulse x).
fn hit_foe(t: &T, other_actor: &str, other_comp: &str, x: f64) {
    t.exec(&format!(r#"
        M.fire("{BODY_HIT}", M.param(FOE), M.param(FOE_MESH), M.param({other_actor}), M.param({other_comp}),
               M.param({{ X = {x}, Y = 0, Z = 0 }}),
               M.param({{ MyBoneName = fname("foot_r"), BoneName = fname("calf_l"), ImpactPoint = {{ X = 60, Y = -10, Z = 10 }} }}))
    "#));
}

#[test]
fn shoves_need_my_body_closing_nearby_and_unprotected() {
    // Stand-in feet touching the floor are never shoves, nor is any contact
    // while the pawns are apart.
    let mut t = T::new();
    let shoves = |t: &T| t.count("impulse");
    // Floor / world contact: never a shove, even if the hook's OtherActor
    // claims to be my pawn (the component decides, by identity).
    hit_foe(&t, "FLOOR_ACTOR", "FLOOR", 9000.0);
    hit_foe(&t, "ME", "FLOOR", 9000.0);
    hit_foe(&t, "ME", "PROP", 9000.0);
    t.run(200);
    assert_eq!(shoves(&t), 0, "floor contact: {:?}", t.outs(""));
    assert!(t.no_yield(), "no yield from floor contacts");
    // My body, closing at 3 m/s, 60 uu apart: a shove.
    hit_foe(&t, "ME", "MY", 3000.0);
    t.run(200);
    assert_eq!(shoves(&t), 1, "local pawn contact: {}", t.logs());
    assert_eq!(t.outs("impulse")[0].bone, FOOT_R);
    // A NON-damaging contact of the weapon my pawn holds (a push with the
    // blade, no Get Damage) counts as my body too.
    t.exec("rawset(MYSWORD_MESH, 'vel', { 400, 0, 0 })");
    hit_foe(&t, "MYSWORD", "MYSWORD_MESH", 3000.0);
    t.run(200);
    assert_eq!(shoves(&t), 2, "non-damaging held weapon contact");
    // A DAMAGING sword hit (Get Damage with HitByComponent = the blade,
    // whose owner is the ModularWeaponBP actor, not my pawn) is HSMPCombat's
    // claim: it must not also go out as a shove (the victim would be knocked back twice).
    t.match_state("live", 1);
    t.run(600);
    hit_foe(&t, "MYSWORD", "MYSWORD_MESH", 3000.0);
    t.exec(&format!(r#"
        M.fire("{GET_DAMAGE}", M.param(FOE), nil, nil, nil, nil, M.param(fname("head")), M.param(25.0),
               nil, nil, nil, nil, nil, nil, M.param(MYSWORD_MESH))
    "#));
    t.run(200);
    assert_eq!(shoves(&t), 2, "damaging sword hit (blade owned by the weapon actor): no shove");
    // A damaging hit by SOMEONE ELSE's sword does not suppress my own shove.
    hit_foe(&t, "ME", "MY", 3000.0);
    t.exec(&format!(r#"
        local blade = new_obj("StaticMeshComponent", "ModularWeaponBP_C_7_Blade"); rawset(blade, "owner", SWORD)
        M.fire("{GET_DAMAGE}", M.param(FOE), nil, nil, nil, nil, M.param(fname("head")), M.param(25.0),
               nil, nil, nil, nil, nil, nil, M.param(blade))
    "#));
    t.run(200);
    assert_eq!(shoves(&t), 3, "a foreign weapon's damage does not eat my shove");
    t.match_state("lobby", 0);
    t.run(600);
    let base = shoves(&t);
    // every record went out with its own ring request id
    let n = t.n("#HSMPNative._rec.sends") as usize;
    let mut ids: Vec<i64> = (1..=n).map(|i| t.n(&format!("HSMPNative._rec.sends[{i}].req_id")) as i64).collect();
    assert!(ids.iter().all(|&x| x > 0));
    let len = ids.len();
    ids.sort_unstable();
    ids.dedup();
    assert_eq!(ids.len(), len, "request ids unique");
    let shoves = move |t: &T| t.count("impulse") - base + 2;
    // No closing velocity (resting / sliding touch): none.
    t.exec("rawset(MY, 'vel', { 0, 0, 0 })");
    hit_foe(&t, "ME", "MY", 3000.0);
    t.run(200);
    assert_eq!(shoves(&t), 2, "no closing velocity");
    // The stand-in running into me closes too.
    t.exec("rawset(FOE_MESH, 'vel', { -300, 0, 0 })");
    hit_foe(&t, "ME", "MY", 3000.0);
    t.run(200);
    assert_eq!(shoves(&t), 3, "stand-in closing on me");
    t.exec("rawset(FOE_MESH, 'vel', { 0, 0, 0 }); rawset(MY, 'vel', { 300, 0, 0 })");
    // Pawns apart (pelvises 340 uu): none.
    t.exec("M.move_bone(FOE_MESH, 'Pelvis', { 340, 0, 100 })");
    hit_foe(&t, "ME", "MY", 3000.0);
    t.run(200);
    assert_eq!(shoves(&t), 3, "apart pawns");
    t.exec("M.move_bone(FOE_MESH, 'Pelvis', { 60, 0, 100 })");
    // Spawn protection (HSMPSync spawn_status bus record, os.clock seconds): none
    // sent, none applied.
    let until = t.n("M.now") / 1000.0 + 5.0;
    t.spawn_status("", Some(until));
    t.run(200);
    hit_foe(&t, "ME", "MY", 3000.0);
    t.inject(2, r#""impulse", 1, 0, "Head", {0,0,0}, {3000,0,0}, 1"#);
    t.run(200);
    assert_eq!(shoves(&t), 3, "spawn-protected: not sent");
    assert_eq!(t.n("#M.impulses"), 0.0, "spawn-protected: not applied");
    // verified + protect_until:null (HSMPSync, placement until Live) is protected too.
    t.spawn_status("", None);
    t.run(200);
    hit_foe(&t, "ME", "MY", 3000.0);
    t.run(200);
    assert_eq!(shoves(&t), 3, "verified, protect_until null: still protected");
    // The same status re-published keeps the answer.
    t.spawn_status("", None);
    t.run(200);
    hit_foe(&t, "ME", "MY", 3000.0);
    t.run(200);
    assert_eq!(shoves(&t), 3, "re-published status: still protected");
    // A status for another pawn protects nothing (checked with a live window).
    let until2 = t.n("M.now") / 1000.0 + 5.0;
    t.spawn_status("Willie_BP_C_OLD", Some(until2));
    t.run(200);
    t.exec("rawset(MY, 'vel', { 300, 0, 0 })");
    hit_foe(&t, "ME", "MY", 3000.0);
    t.run(300);
    assert_eq!(shoves(&t), 4, "status of another pawn: not protected");
    let shoves = { let b = t.count("impulse"); move |t: &T| t.count("impulse") - b + 3 };
    t.spawn_status("", Some(0.0));
    t.run(200);
    // Per-pair rate cap: a strong contact every frame for 1 s.
    let before = shoves(&t);
    for _ in 0..125 {
        hit_foe(&t, "ME", "MY", 3000.0);
        t.run(8);
    }
    t.run(200);
    let n = shoves(&t) - before;
    assert!((3..=8).contains(&n), "{n} shoves in 1 s of continuous contact");
    assert!(!t.logs().contains(" error (#"), "{}", t.logs());
}

// --- record -> application ---------------------------------------------------------------------

#[test]
fn impulses_land_on_my_body_with_caps() {
    let mut t = T::new();
    t.inject(2, r#""impulse", 1, 0, "Spine_02", {0, 0, 5}, {3000, 0, 0}, 10"#);
    t.run(10);
    assert_eq!(t.n("#M.impulses"), 1.0, "applied within a frame: {}", t.logs());
    assert_eq!(t.s("M.impulses[1].mesh"), "Willie_BP_C_100_SK_Skeleton");
    assert_eq!(t.s("M.impulses[1].bone"), "Spine_02");
    assert_eq!(t.n("M.impulses[1].imp[1]"), 3000.0);
    // My Spine_02 is at (0,0,125): + (0,0,5).
    assert_eq!((t.n("M.impulses[1].at[1]"), t.n("M.impulses[1].at[3]")), (0.0, 130.0));
    // A huge one is capped by the bone mass (10 kg * 800 cm/s).
    t.inject(2, r#""impulse", 2, 0, "Head", {0, 0, 0}, {0, 90000, 0}, 11"#);
    t.run(10);
    assert_eq!(t.n("#M.impulses"), 2.0);
    assert!((t.n("M.impulses[2].imp[2]") - 8000.0).abs() < 1e-6);
    // An unknown bone index / a bad hand is ignored.
    t.inject(2, r#""impulse", 3, 0, 16, {0, 0, 0}, {1, 0, 0}, 1"#);
    t.inject(2, r#""impulse", 4, 7, "Head", {0, 0, 0}, {1, 0, 0}, 1"#);
    t.run(20);
    assert_eq!(t.n("#M.impulses"), 2.0);
    // Events of other kinds on this state's cursor never reach the mod's queue.
    t.exec("HSMPNative.sc_rec_event('dev_cmd', { op = 1, key = 'x' }, 0)");
    t.run(10);
    assert!(!t.logs().contains(" error (#"), "{}", t.logs());
}

#[test]
fn grabs_pull_my_bone_toward_the_grabbers_standin_hand() {
    let mut t = T::new();
    // Peer 2 grabs my left forearm (20,25,140) with its right hand; its
    // stand-in's Hand_R is at (60-45, 20, 140) = (15,20,140) on my screen.
    t.inject(2, r#""grab_start", 5, 0, "Lowerarm_L", {2, 0, 0}, {500, 500, 500}, 10"#);
    t.run(10);
    assert_eq!(t.n("#M.handles"), 1.0, "one handle on my pawn: {}", t.logs());
    assert_eq!(t.s("M.grabs[1].mesh"), "Willie_BP_C_100_SK_Skeleton");
    assert_eq!(t.s("M.grabs[1].bone"), "Lowerarm_L");
    assert_eq!((t.n("M.grabs[1].at[1]"), t.n("M.grabs[1].at[2]")), (22.0, 25.0), "grip at bone + pt");
    assert_eq!(t.n("rawget(M.handles[1], 'lin_k')"), 1500.0, "soft");
    assert_eq!(t.n("rawget(M.handles[1], 'ang_k')"), 0.0, "position only");
    // Target: toward the stand-in hand (15,20,140), not the streamed fallback (500,...).
    let tg = [t.n("rawget(M.handles[1], 'target')[1]"), t.n("rawget(M.handles[1], 'target')[2]"), t.n("rawget(M.handles[1], 'target')[3]")];
    assert!(near(tg, [15.0, 20.0, 140.0], 0.01), "{tg:?}");
    // The grabber's stand-in hand moves 100 uu away: the target leads by at most 40 uu.
    t.exec("M.move_bone(FOE_MESH, 'Hand_R', { 22, 125, 140 })");
    t.run(10);
    let tg = [t.n("rawget(M.handles[1], 'target')[1]"), t.n("rawget(M.handles[1], 'target')[2]")];
    assert!(near([tg[0], tg[1], 0.0], [22.0, 65.0, 0.0], 0.01), "lead clamp: {tg:?}");
    // Updates keep it; no update for 1.5 s releases it (lease).
    t.inject(2, r#""grab_update", 5, 0, "Lowerarm_L", {2, 0, 0}, {0, 0, 0}, 20"#);
    t.exec("M.move_bone(FOE_MESH, 'Hand_R', { 22, 60, 140 })");
    t.run(1000);
    assert!(t.s("M.handles[1].GrabbedComponent") != "nil", "held");
    t.run(700);
    assert_eq!(t.s("M.handles[1].GrabbedComponent"), "nil", "lease expired");
    assert!(t.logs().contains("lease expired"));
    // A new grab reuses the pooled handle; grab_end releases it.
    t.inject(2, r#""grab_start", 6, 1, "Hand_R", {0, 0, 0}, {0, 0, 0}, 30"#);
    t.run(10);
    assert_eq!(t.n("#M.handles"), 1.0, "pooled");
    assert!(t.s("M.handles[1].GrabbedComponent") != "nil");
    t.inject(2, r#""grab_end", 6, 1, "Hand_R", {0, 0, 0}, {0, 0, 0}, 40"#);
    t.run(10);
    assert_eq!(t.s("M.handles[1].GrabbedComponent"), "nil");
    assert!(t.logs().contains("grabber let go"));
    assert!(!t.logs().contains(" error (#"), "{}", t.logs());
}

#[test]
fn i_can_break_free_and_a_new_pawn_drops_old_grabs() {
    let mut t = T::new();
    t.inject(2, r#""grab_start", 1, 0, "Lowerarm_L", {0, 0, 0}, {0, 0, 0}, 1"#);
    t.run(10);
    assert!(t.s("M.handles[1].GrabbedComponent") != "nil");
    // The grabber's hand is 300 uu from my forearm for > 400 ms: broke free.
    t.exec("M.move_bone(FOE_MESH, 'Hand_R', { 320, 25, 140 })");
    t.run(300);
    assert!(t.s("M.handles[1].GrabbedComponent") != "nil", "not yet");
    for _ in 0..3 {
        t.inject(2, r#""grab_update", 1, 0, "Lowerarm_L", {0, 0, 0}, {0, 0, 0}, 2"#);
        t.run(100);
    }
    assert_eq!(t.s("M.handles[1].GrabbedComponent"), "nil", "broke free: {}", t.logs());
    assert!(t.logs().contains("broke free"));
    // Grab again, then my pawn changes (respawn in the same world).
    t.exec("M.move_bone(FOE_MESH, 'Hand_R', { 15, 20, 140 })");
    t.inject(2, r#""grab_start", 2, 0, "Lowerarm_L", {0, 0, 0}, {0, 0, 0}, 3"#);
    t.run(10);
    assert!(t.s("M.handles[1].GrabbedComponent") != "nil");
    t.exec("ME2, MY2 = M.willie('Willie_BP_C_101', {0, 0, 100}, 1, 'SK_Skeleton'); M.pc.Pawn = ME2");
    t.run(80);
    assert_eq!(t.s("M.handles[1].GrabbedComponent"), "nil", "old pawn's grab released");
    assert!(t.logs().contains("my pawn changed"));
}

// --- world guard, session gating ---------------------------------------------------------------

#[test]
fn level_change_drops_everything_untouched_and_skips_stale_events() {
    let mut t = T::new();
    t.exec("ME['Grabbed R'] = true; ME['Grab Component R'] = FOE_MESH; ME['Grab Bone R'] = fname('lowerarm_l')");
    t.inject(2, r#""grab_start", 1, 1, "Lowerarm_R", {0, 0, 0}, {0, 0, 0}, 1"#);
    t.run(50);
    assert_eq!(t.count("grab_start"), 1);
    assert!(t.s("M.handles[1].GrabbedComponent") != "nil");
    // Round reset: the same arena re-opens; every old object is freed.
    t.exec("M.premap(); M.kill_all(); M.new_world('Map_Arena_Pit')");
    // Events arriving while the new world has no pawn yet are stale.
    t.inject(2, r#""impulse", 9, 0, "Head", {0, 0, 0}, {3000, 0, 0}, 5"#);
    t.run(300);
    assert_eq!(t.ev::<Vec<String>>("M.dead_touch"), Vec::<String>::new(), "freed objects touched");
    assert_eq!(t.count("grab_end"), 1, "my grab ended with the world");
    // New world, new pawn and stand-in: the stale impulse was skipped, new ones apply.
    t.exec(r#"
        ME, MY = M.willie("Willie_BP_C_100", {0, 0, 100}, 1, "SK_Skeleton"); M.pc.Pawn = ME
        FOE, FOE_MESH = M.willie("Willie_BP_C_200", {60, 0, 100}, -1, "Mesh")
    "#);
    t.run(300);
    assert_eq!(t.n("#M.impulses"), 0.0, "stale impulse skipped");
    t.inject(2, r#""impulse", 10, 0, "Head", {0, 0, 0}, {3000, 0, 0}, 6"#);
    t.run(20);
    assert_eq!(t.n("#M.impulses"), 1.0);
    assert_eq!(t.s("M.impulses[1].mesh"), "Willie_BP_C_100_SK_Skeleton");
    assert_eq!(t.ev::<Vec<String>>("M.dead_touch"), Vec::<String>::new());
}

#[test]
fn nothing_happens_without_an_mp_session_and_loops_are_shimmed() {
    let mut t = T::new();
    t.session(false);
    t.exec("M.run(1100)");
    t.exec("ME['Grabbed R'] = true; ME['Grab Component R'] = FOE_MESH; ME['Grab Bone R'] = fname('head')");
    t.inject(2, r#""impulse", 1, 0, "Head", {0, 0, 0}, {3000, 0, 0}, 1"#);
    t.exec("M.run(300)");
    assert!(t.outs("").is_empty(), "{:?}", t.outs(""));
    assert_eq!(t.n("#M.impulses"), 0.0);
    let logs = t.logs();
    assert!(!logs.contains("raw LoopAsync must be shimmed") && !logs.contains("ERR"), "{logs}");
}

/// A grab on my body is released when the MP session goes down (link
/// stalled / reconnecting): the handle must not pull my bone through it.
#[test]
fn a_grab_on_me_lets_go_when_the_session_drops() {
    let mut t = T::new();
    t.inject(2, r#""grab_start", 5, 0, "Lowerarm_L", {0, 0, 0}, {0, 0, 0}, 10"#);
    t.run(10);
    assert!(t.s("M.handles[1].GrabbedComponent") != "nil", "held: {}", t.logs());
    // the sidecar stalls: status reconnecting, then no heartbeat
    t.session(false);
    t.exec("M.run(1200)");
    assert_eq!(t.s("M.handles[1].GrabbedComponent"), "nil", "released while the session is down: {}", t.logs());
    assert!(t.logs().contains("MP session down"), "{}", t.logs());
    assert_eq!(t.ev::<Vec<String>>("M.dead_touch"), Vec::<String>::new());
}

#[test]
fn a_sidecar_that_stops_beating_is_no_session() {
    // Interact uses the shared liveness rule (hsmp_session): a
    // "connected" link a crashed sidecar left behind sends nothing.
    let t = T::new();
    t.exec("HSMPNative._st.hb_age = 1e9"); // the sidecar stops beating
    t.exec("M.run(6500)");
    t.exec(&format!(r#"
        M.fire("{BODY_HIT}", M.param(FOE), M.param(FOE_MESH), M.param(ME), M.param(MY),
               M.param({{ X = -2000, Y = 0, Z = 0 }}),
               M.param({{ MyBoneName = fname("spine_03"), ImpactPoint = {{ X = 55, Y = 0, Z = 150 }} }}))
    "#));
    t.exec("M.run(200)");
    assert_eq!(t.count("impulse"), 0, "a dead sidecar's link is no session: {:?}", t.outs(""));
}

#[test]
fn grabs_honour_spawn_protection_both_ways() {
    // As with shoves, grabs are off while I am spawn-protected: I start
    // none, and a peer's grab on me is not attached.
    let mut t = T::new();
    t.spawn_status("", None);
    t.run(200);
    t.exec("ME['Grabbed R'] = true; ME['Grab Component R'] = FOE_MESH; ME['Grab Bone R'] = fname('lowerarm_l')");
    t.run(100);
    assert_eq!(t.count("grab_start"), 0, "protected: no grab_start {:?}", t.outs(""));
    t.exec("ME['Grabbed R'] = false");
    t.inject(2, r#""grab_start", 5, 0, "Lowerarm_L", {2, 0, 0}, {500, 500, 500}, 10"#);
    t.run(50);
    assert_eq!(t.n("#M.handles"), 0.0, "protected: a grab on me is not attached: {}", t.logs());
    // protection ends: a NEW grab works, the old one stays closed
    t.spawn_status("", Some(0.0));
    t.run(200);
    t.inject(2, r#""grab_update", 5, 0, "Lowerarm_L", {2, 0, 0}, {0, 0, 0}, 20"#);
    t.run(20);
    assert_eq!(t.n("#M.handles"), 0.0, "the grab refused under protection stays closed");
    t.inject(2, r#""grab_start", 6, 0, "Lowerarm_L", {2, 0, 0}, {500, 500, 500}, 30"#);
    t.run(20);
    assert_eq!(t.n("#M.handles"), 1.0, "unprotected: a new grab attaches: {}", t.logs());
    t.exec("ME['Grabbed R'] = true; ME['Grab Component R'] = FOE_MESH; ME['Grab Bone R'] = fname('lowerarm_l')");
    t.run(50);
    assert_eq!(t.count("grab_start"), 1, "unprotected: my grab starts");
}

#[test]
fn standins_are_not_scanned_before_the_world_settled() {
    // No FindAllOf over Willies (nor a name read on what it returns)
    // in the first 2 s of a world: the crash window after a round reload.
    let mut t = T::new_settle(2.0);
    t.exec("M.fao_calls = 0; local f = FindAllOf; FindAllOf = function(c) if c == 'Willie_BP_C' then M.fao_calls = M.fao_calls + 1 end return f(c) end");
    // T::new ran 1.1 s; the guard first saw the world once the session went
    // live (~1 s in), so the settle ends near 3 s
    t.run(700);
    assert_eq!(t.n("M.fao_calls"), 0.0, "no Willie scan before 2 s");
    hit_foe(&t, "ME", "MY", 3000.0);
    t.run(16);
    assert_eq!(t.count("impulse"), 0, "stand-ins unknown before the settle: no shove");
    t.run(1400);
    assert!(t.n("M.fao_calls") >= 1.0, "scanned once settled");
    t.exec("rawset(MY, 'vel', { 300, 0, 0 })");
    hit_foe(&t, "ME", "MY", 3000.0);
    t.run(200);
    assert_eq!(t.count("impulse"), 1, "settled: the stand-in is resolved, shoves work: {}", t.logs());
}
