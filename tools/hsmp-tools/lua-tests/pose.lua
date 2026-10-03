-- Unit tests for the pure-Lua pose code in HSMPAvatars (and a Lua 5.4 syntax
-- check of HSMPSync + HSMPAvatars), run outside the game.
--
--     hsmp-tools lua-test pose [-- path/to/.pose_play<id>.json ...]
--
-- Extra paths are real files written by hsmp-sidecar; each must parse.
-- The block between "-- BEGIN PURE" and "-- END PURE" in HSMPAvatars must not
-- call UE; it is loaded here verbatim.

local T = T
local real_files = { ... }

local AV = T.path("mods/HSMPAvatars/Scripts/main.lua")
local SY = T.path("mods/HSMPSync/Scripts/main.lua")

-- ---- syntax ------------------------------------------------------------------
for _, path in ipairs({ AV, SY }) do
    local f, e = load(T.read(path))
    local modname = path:match("([^/]+)/Scripts/[^/]+$")
    T.check(f ~= nil, "syntax " .. modname, tostring(e))
end

-- ---- PURE block ----------------------------------------------------------------
local src = T.read(AV)
local a = src:find("-- BEGIN PURE", 1, true)
local b = src:find("-- END PURE", 1, true)
local pure = src:sub(a, b - 1)
local has_ue = false
for _, k in ipairs({ "FName(", ":IsValid", "StaticFindObject", "FindAllOf", "UEHelpers" }) do
    if T.contains(pure, k) then has_ue = true end
end
T.check(not has_ue, "PURE block has no UE calls")

local PURE = assert(load(pure .. "\nreturn PURE", "@PURE"))()
local d3 = PURE.d3

local LINE = '{"peer_id":2,"seq":41,"pt":123456,"mode":"interp","age":18,"delay":31.5,"jit":6.2,"cut":3,'
    .. '"root":[100.0,-50.5,90.0,45.00],"bones":{"Pelvis":[100.0,-50.0,95.0,0.0000,0.0000,0.0000,1.0000],'
    .. '"Spine_02":[100.0,-50.0,115.0,0.0000,0.0000,0.0000,1.0000],'
    .. '"Spine_04":[100.0,-50.0,140.0,0.0000,0.0000,0.0000,1.0000],'
    .. '"Upperarm_R":[100.0,-30.0,140.0,0.0000,0.0000,0.0000,1.0000],'
    .. '"Lowerarm_R":[100.0,0.0,140.0,0.0000,0.0000,0.0000,1.0000],'
    .. '"Hand_R":[100.0,30.0,140.0,0.0000,0.0000,0.7071,0.7071],'
    .. '"Hand_L":[80.0,-70.0,120.0,0.0000,0.0000,0.0000,1.0000],'
    .. '"Weapon":[100.0,80.0,140.0,-0.5000,0.5000,0.5000,0.5000]},"end":1}'

local t = PURE.parse_play(LINE, nil)
T.check(type(t) == "table", "parse: table")
T.check(t.seq == 41 and t.pt == 123456 and t.mode == "interp" and t.cut == 3
    and math.abs(t.delay - 31.5) < 1e-9 and math.abs(t.jit - 6.2) < 1e-9 and t.age == 18, "parse: header")
T.check(t.root[1] == 100.0 and t.root[2] == -50.5 and t.root[4] == 45.0, "parse: root")
T.check(t.nbones == 8 and t.bones.Weapon[4] == -0.5 and t.bones.Hand_R[7] == 0.7071, "parse: bones")
T.check(PURE.parse_play(LINE, 41) == "same", "parse: same seq short-circuits")
T.check(PURE.parse_play(LINE:sub(1, -13), nil) == nil, "parse: torn line rejected")
T.check(PURE.parse_play("", nil) == nil and PURE.parse_play(nil, nil) == nil, "parse: empty / nil rejected")
T.check(PURE.parse_play('{"x":1,"end":1}', nil) == nil, "parse: no seq rejected")
local neg = LINE:gsub('"Hand_L":%[80%.0,%-70%.0,120%.0', '"Hand_L":[-80.5,-70.0,-120.25')
T.check(PURE.parse_play(neg, nil).bones.Hand_L[3] == -120.25, "parse: negatives")
local stale = '{"peer_id":1,"seq":2,"pt":0,"mode":"stale","age":-1,"delay":60.0,"jit":0.0,"cut":0,"bones":{},"end":1}'
local ts = PURE.parse_play(stale, nil)
T.check(ts.mode == "stale" and ts.root == nil and ts.nbones == 0, "parse: stale, no root, no bones")

-- ---- retarget -----------------------------------------------------------------
local src_b = t.bones
-- Stand-in: forearm 20 % shorter, upper arm same, spine same.
local lens = { Spine_02 = 20.0, Spine_04 = 25.0, Upperarm_R = 20.0, Lowerarm_R = 30.0 * 1.0, Hand_R = 30.0 * 0.8 }
local out = PURE.retarget(src_b, lens)
T.check(out.Pelvis[3] == 95.0, "retarget: pelvis untouched")
T.check(math.abs(d3(out.Hand_R, out.Lowerarm_R) - 24.0) < 1e-6 and math.abs(d3(out.Lowerarm_R, out.Upperarm_R) - 30.0) < 1e-6,
    "retarget: lengths imposed")
-- Direction preserved.
local dx = (out.Hand_R[2] - out.Lowerarm_R[2]) / d3(out.Hand_R, out.Lowerarm_R)
T.check(math.abs(dx - 1.0) < 1e-9, "retarget: direction kept")
T.check(out.Hand_R[7] == 0.7071, "retarget: rotation kept")
T.check(out._wpn_hand == "Hand_R", "retarget: weapon anchored to right hand")
local w_off = { out.Weapon[1] - out.Hand_R[1], out.Weapon[2] - out.Hand_R[2], out.Weapon[3] - out.Hand_R[3] }
T.check(math.abs(w_off[2] - 50.0) < 1e-9 and math.abs(w_off[1]) < 1e-9, "retarget: weapon keeps its hand offset")
T.check(out.Weapon[4] == -0.5, "retarget: weapon rotation kept")
-- Absurd ratios are ignored (measurement glitch, dislocated stand-in).
local bad = PURE.retarget(src_b, { Hand_R = 300.0 })
T.check(math.abs(d3(bad.Hand_R, bad.Lowerarm_R) - 30.0) < 1e-9, "retarget: absurd length ignored")
-- Missing parent: bone left as sent.
local partial = { Pelvis = src_b.Pelvis, Hand_R = src_b.Hand_R }
local pr = PURE.retarget(partial, lens)
T.check(pr.Hand_R[2] == 30.0, "retarget: orphan bone untouched")
-- No pelvis: copy.
local nop = PURE.retarget({ Hand_R = src_b.Hand_R }, lens)
T.check(nop.Hand_R[2] == 30.0, "retarget: no pelvis -> passthrough")
-- Left-hand weapon.
local lw = {}
for k, v in pairs(src_b) do lw[k] = v end
lw.Weapon = { 80.0, -95.0, 120.0, 0, 0, 0, 1 }
T.check(PURE.anchor_hand(lw) == "Hand_L", "anchor: left hand when closer")

-- ---- quat -> rotator ---------------------------------------------------------------
local r = PURE.quat_to_rot(0.0, 0.0, 0.7071068, 0.7071068)
T.check(math.abs(r.Yaw - 90.0) < 1e-3 and math.abs(r.Pitch) < 1e-3 and math.abs(r.Roll) < 1e-3, "quat_to_rot: yaw 90")
r = PURE.quat_to_rot(0.0, 0.7071068, 0.0, 0.7071068)
T.check(math.abs(math.abs(r.Pitch) - 90.0) < 1e-3, "quat_to_rot: pitch gimbal")

-- ---- optional real files -------------------------------------------------------------
for _, p in ipairs(real_files) do
    local txt = T.read(p) or ""
    local line = txt:match("^[^\n]*") or ""
    local tt = PURE.parse_play(line, nil)
    T.check(type(tt) == "table" and tt.nbones >= 1, "real file parses: " .. (p:match("[^/\\]+$") or p), line:sub(1, 120))
end
