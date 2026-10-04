-- Damage parity: the same blow does the same damage in MP as in solo.
--
--     hsmp-tools lua-test damage_parity
--
-- The native path is a model of the decompiled Willie_BP
-- (docs/development/subsystems/combat.md, docs/development/subsystems/combat-parity.md):
--   "Deal Complex Damage": the per-bone contact gate, the hit point mapped into
--     the hit bone's space, the proxy trace through the armour layers that
--     cover THAT spot of the bone (layers stack), Raw from Rigidity, Hit
--     Velocity, Cutting Power and Stab Rate, then
--   "Get Damage": Invulnerable, DRS = 2·hm·Raw, the per-bone Last Damage Taken
--     gate (reset 0.2 s after the last blow that passed it), the impulse / DRS
--     gate, Health with the per-part floor, bleeding, consciousness (with the
--     'Weapon'-tag factor on light blows), pain and its stumble direction,
--     part health, blood.
-- UE4SS calls a Blueprint hook's callback AFTER the body; the model does too.
--
-- SOLO: the attacker's weapon / fist / foot hits the victim.
-- MP:   it hits the victim's STAND-IN on the attacker's screen. The stand-in
--       is posed as the victim was a moment ago (here: turned 70° and leaning
--       away from where the victim is when the replay runs), wears different
--       armour and is Invulnerable. HSMPCombat claims the armour-stage
--       inputs, the server forwards them (honest blow: unchanged), and the
--       victim replays them natively on its own pawn.
--
-- Proven against the REAL HSMPCombat main.lua:
--   1. weapon (sword cut, sword thrust, axe, mace, polearm haft, fist, kick)
--      x armour (cloth, padded, mail, plate, mixed) x part (head, torso, arm,
--      leg) x spot (front, side, back): the victim's Health, Consciousness,
--      Bleeding, Pain, part Health, blood marks and stumble direction change
--      exactly as in solo (table printed);
--   2. the stand-in takes nothing, and every call its contact gate let through
--      is one claim;
--   3. a graze and a harder frame in one tick both land, as in solo;
--   4. blows far apart on the attacker's clock whose replays arrive bunched do
--      not gate each other; frames of one contact still do, also when their
--      replays arrive further apart than the game's 0.2 s reset;
--   4b. a stand-in that took a blow natively (Invulnerable reset) keeps its
--      contact gate, so its claims stay those of solo;
--   5. a light weapon blow keeps the 'Weapon' consciousness factor;
--   6. the echo, a rejected blow, and the blood on the stand-in (as before).

local T = T
local STATE = T.tmpdir("hsmp_parity_")
-- HSMP_PARITY_SRC (global, set by a wrapper) runs the same suite against another
-- main.lua, e.g. the previous release, for a before / after table.
local SRC = rawget(_G, "HSMP_PARITY_SRC") or T.path("mods/HSMPCombat/Scripts/main.lua")
local REPORT_ONLY = rawget(_G, "HSMP_PARITY_SRC") ~= nil

HSMP_COMBAT_TEST = {}
CLOCK = 100.0
os.clock = function() return CLOCK end
os.getenv = function(k) if k == "HSMP_STATE_DIR" then return STATE end return nil end
LOGS = {}
print = function(s) LOGS[#LOGS + 1] = s end
RegisterHook = function() end
LoopAsync = function() end
ExecuteInGameThread = function(f) f() end
StaticFindObject = function() return nil end
FindAllOf = function() return nil end
function FName(s) return { s = s, ToString = function(self) return self.s end } end

-- ---- vector / quaternion helpers (the mock engine's own math) ----------------------
local function v(x, y, z) return { X = x, Y = y, Z = z } end
local function len(a) return math.sqrt(a.X * a.X + a.Y * a.Y + a.Z * a.Z) end
local function scale(a, k) return v(a.X * k, a.Y * k, a.Z * k) end
local function add(a, b) return v(a.X + b.X, a.Y + b.Y, a.Z + b.Z) end
local function sub(a, b) return v(a.X - b.X, a.Y - b.Y, a.Z - b.Z) end
local function qmul(a, b)
    return { a[4] * b[1] + a[1] * b[4] + a[2] * b[3] - a[3] * b[2],
             a[4] * b[2] - a[1] * b[3] + a[2] * b[4] + a[3] * b[1],
             a[4] * b[3] + a[1] * b[2] - a[2] * b[1] + a[3] * b[4],
             a[4] * b[4] - a[1] * b[1] - a[2] * b[2] - a[3] * b[3] }
end
local function qaxis(ax, deg)
    local h = math.rad(deg) / 2
    local s = math.sin(h)
    return { ax[1] * s, ax[2] * s, ax[3] * s, math.cos(h) }
end
local function qrot(q, a, inv)
    local qx, qy, qz, qw = q[1], q[2], q[3], q[4]
    if inv then qx, qy, qz = -qx, -qy, -qz end
    local tx, ty, tz = 2 * (qy * a.Z - qz * a.Y), 2 * (qz * a.X - qx * a.Z), 2 * (qx * a.Y - qy * a.X)
    return v(a.X + qw * tx + (qy * tz - qz * ty), a.Y + qw * ty + (qz * tx - qx * tz), a.Z + qw * tz + (qx * ty - qy * tx))
end
local function mrc(x, a0, a1, b0, b1)   -- MapRangeClamped
    local t = (a1 == a0) and 0 or (x - a0) / (a1 - a0)
    t = math.max(0, math.min(1, t))
    return b0 + (b1 - b0) * t
end

-- ---- body: bones, parts, pose ----------------------------------------------------------
-- body part: 0 Head 1 Neck 2 ArmR 3 ArmL 4 Upper 5 Lower 6 LegR 7 LegL
local BONES = {   -- rest position relative to the pelvis (local +X = front), surface radius
    head = { v(0, 0, 70), 11, 0 }, neck_01 = { v(0, 0, 58), 6, 1 }, spine_03 = { v(0, 0, 35), 16, 4 },
    spine_02 = { v(0, 0, 18), 15, 5 }, lowerarm_r = { v(5, 25, 30), 5, 2 }, upperarm_l = { v(0, -22, 45), 6, 3 },
    thigh_l = { v(0, -10, -20), 8, 7 }, calf_r = { v(0, 10, -55), 6, 6 },
}
local function part_of(bone) local b = BONES[bone:lower()] or BONES[bone]; return b and b[3] or 4 end
local KH  = { [0] = 15, 7.5, 0.1, 0.1, 5, 2.5, 0.1, 0.1 }
local THR = { [0] = 1, 2.5, 15, 15, 5, 10, 15, 15 }
local KB  = { [0] = 5, 20, 0.1, 0.1, 5, 3, 0.1, 0.1 }
local KC  = { [0] = 10, 1, 0, 0, 1, 0.5, 0, 0 }
local KL  = { [0] = 3, 1, 1, 1, 1, 0.5, 1, 1 }
local BL  = { [0] = 1, 0.1, 0.1, 0.1, 0.25, 0.1, 0.1, 0.1 }
local PH  = { [0] = "Head Health", "Neck Health", "Arm_R Health", "Arm_L Health", "Body Upper Health",
              "Body Lower Health", "Leg_R Health", "Leg_L Health" }

-- ---- armour: pieces cover parts AND spots (d = unit direction in bone space) -------------
-- { Def Blunt, Def Cut, Def Stab } (armor_protection.txt) and the covered spots.
local function front(d) return d.X > -0.2 end
local function back(d) return d.X <= -0.2 end
local function all() return true end
local function not_face(d) return not (d.X > 0.55 and math.abs(d.Z) < 0.6) end
local function outer(d) return d.X > -0.5 end
local PIECES = {
    shirt     = { 1, 0.5, 0.25, { [4] = all, [5] = all, [2] = all, [3] = all } },
    hosen     = { 1, 0.5, 0.25, { [6] = all, [7] = all } },
    gambeson  = { 5, 20, 2, { [4] = all, [5] = all, [2] = all, [3] = all } },
    padhosen  = { 5, 20, 2, { [6] = all, [7] = all } },
    hauberk   = { 1, 150, 15, { [1] = all, [4] = all, [5] = all, [2] = all, [3] = all } },
    breast    = { 15, 250, 100, { [4] = front, [5] = front } },
    backplate = { 15, 250, 100, { [4] = back, [5] = back } },
    sallet    = { 22, 275, 125, { [0] = not_face } },
    vambrace  = { 15, 250, 100, { [2] = outer, [3] = outer } },
    cuisse    = { 15, 250, 100, { [6] = front, [7] = front } },
}
local ARMOUR = {
    { name = "cloth",  set = { "shirt", "hosen" } },
    { name = "padded", set = { "gambeson", "padhosen" } },
    { name = "mail",   set = { "gambeson", "hauberk", "padhosen" } },
    { name = "plate",  set = { "gambeson", "hauberk", "breast", "backplate", "sallet", "vambrace", "cuisse", "padhosen" } },
    { name = "mixed",  set = { "shirt", "breast", "sallet", "hauberk", "hosen" } },   -- open back, mail arms, cloth legs
}
local function layers(w, part, d)
    local B, C, S = 0, 0, 0
    for _, n in ipairs(w.armour) do
        local p = PIECES[n]
        local cov = p[4][part]
        if cov and cov(d) then B, C, S = B + p[1], C + p[2], S + p[3] end
    end
    return B, C, S
end

-- ---- the native model (decompiled Willie_BP) ---------------------------------------------
local api       -- HSMPCombat test api (set below)
local function cb(name, ...) if api then api[name](...) end end   -- the UE4SS after-callback

local function bone_frame(w, bone)
    local b = BONES[bone:lower()] or BONES.spine_03
    return add(w.pos, qrot(w.q, scale(b[1], w.s))), w.q, w.s, b[2]
end

local function get_damage(self, Impulse, Velocity, Location, Normal, bone, raw, cp, inside, mesh, dism,
                          lower, shock, hitby, stab, hitbox, painrate, hitflesh, draw)
    (function()
        self["Was Just Touched"] = true
        if self.Invulnerable then return end
        local b = bone:ToString()
        -- Reset Last Damage Taken: a RetriggerableDelay that fires 0.2 s after the
        -- last pass. A value written after it fired stays (the delay is spent).
        local fire = (self.ldt_at or -1e9) + 0.2
        if CLOCK >= fire and (self.ldt_w or -1e9) < fire then self["Last Damage Taken"] = 0 end
        local drs = raw * 2 * self.hm
        local lb = self["Last Damaged Bone"] and self["Last Damaged Bone"]:ToString() or "None"
        if not (drs >= self["Last Damage Taken"] * (draw + 1) or b ~= lb) then return end
        self["Last Damage Taken"], self["Last Damaged Bone"], self.ldt_at = drs, FName(b), CLOCK
        self["Sustained Damage"] = self["Sustained Damage"] + (inside and 0 or drs)
        if not (len(Impulse) > (lower and 0 or 1000) or inside or drs > 333 or len(Velocity) * (draw + 1) > 1000) then return end
        local p = part_of(b)
        local hpd = drs > 1000 or inside
        local hp = hpd and drs * KH[p] * 0.00025 or 0
        local h0 = self.Health
        local nh = h0 - hp
        self.Health = math.max(0, (nh >= THR[p]) and nh or ((h0 <= THR[p]) and h0 or THR[p]))
        self.Bleeding = self.Bleeding + drs * KB[p] * 0.0001 * math.max(cp, 1)
        -- M4: 'Get Up Rate' etc. scale consciousness loss of light WEAPON blows.
        local weapon = false
        pcall(function() weapon = hitby and hitby:ComponentHasTag(FName("Weapon")) == true end)
        local m = mrc(drs, 0, 1000, weapon and mrc(self["Dead Weight Scale"], 0, 1, 1, 0.1) or 1, 1)
        local m4 = mrc(self["Get Up Rate"], 0, 1, 1, m)
        local F = (4 * drs + len(Impulse)) / 5
        self.Consciousness = self.Consciousness - F * KC[p] * 0.005 * m4
        local add_pain = drs * 0.001 * mrc(cp, 0, 1000, 1, 10) * painrate
        if add_pain > self["Current Pain Threshold"] then
            self["Current Pain Threshold"], self["Flinch Index"] = add_pain, ({ [0] = 4, 6, 3, 2, 6, 5, 1, 0 })[p]
        end
        self.Pain = self.Pain + add_pain
        if len(Velocity) > 0 then   -- Pain Stumble Immediate += normalize(Velocity)·20·Add Pain
            local s = self["Pain Stumble Immediate"]
            self["Pain Stumble Immediate"] = add(s, scale(Velocity, 20 * add_pain / len(Velocity)))
        end
        local L = hpd and drs * KL[p] * 0.0025 or 0
        if p == 1 then L = L * mrc(cp, 0, 1000, 1, 25) end
        self[PH[p]] = math.max(0, math.min(100, self[PH[p]] - L))
        if drs >= 10000 then self["All Body Tonus"] = math.max(0, self["All Body Tonus"] - F * 0.005) end
        local blood = drs * mrc(cp, 0, 100, BL[p], 25) * 0.001 * math.max(0.25, math.min(2, self["Blood Rate"]))
            * mrc(cp, 0, 10, hitflesh and 0.1 or 0, 1)
        if blood >= 1 then self.gore[#self.gore + 1] = string.format("%s:%.0f", b, blood) end
    end)()
    cb("on_get_damage", self, Impulse, Velocity, Location, Normal, bone, raw, cp, inside, mesh, dism,
        lower, shock, hitby, stab, hitbox, painrate, hitflesh, draw)
end

local function deal_complex(self, hitc, coll, bone, loc, nrm, vel, imp, cp, stab, rig, blunt, lower,
                            dparent, kick, hitbox, xhv, draw)
    (function()
        local b = bone:ToString()
        if CLOCK - (self.lcd_at or -1e9) >= 0.1 then self["Last Complex Damage Impulse"] = 0 end
        local lb = self["Last Complex Damage Bone"] and self["Last Complex Damage Bone"]:ToString() or "None"
        local g = len(imp) * (cp + 1)
        if not (g >= self["Last Complex Damage Impulse"] or b ~= lb) then return end
        if self["Last Complex Damage Impulse"] == 0 then self.lcd_at = CLOCK end
        self["Last Complex Damage Impulse"], self["Last Complex Damage Bone"] = g, FName(b)
        -- TransformToBoneSpace(Hit Bone, Location) -> proxy trace toward the bone
        local p, q, s, r = bone_frame(self, b)
        local lp = scale(qrot(q, sub(loc, p), true), 1 / s)
        local d = len(lp)
        if d < 0.5 * r then return end           -- deep inside: the trace finds nothing
        local B, C, S = layers(self, part_of(b), scale(lp, 1 / d))
        local vh = len(vel)
        local e = cp * vh * rig
        local resist = (C * (1 - stab) + S * stab) * 1000
        local frac = e > 0 and math.max(0, (e - resist) / e) or 0
        local dmg = math.max(0, rig * vh * (1 - frac) - B * 100) + rig * vh * frac
        get_damage(self, scale(imp, rig * kick), scale(vel, rig * kick), loc, nrm, bone, dmg, frac * cp, false,
            hitc, blunt, lower, dparent, coll, false, hitbox, 1, true, draw)
    end)()
    cb("on_complex", self, hitc, coll, bone, loc, nrm, vel, imp, cp, stab, rig, blunt, lower,
        dparent, kick, hitbox, xhv, draw)
end

local addr = 5000
local function valid() return true end
local function mk_mesh(w)
    return {
        IsValid = valid,
        GetSocketLocation = function(_, fn) local p = bone_frame(w, fn:ToString()); return p end,
        GetSocketTransform = function(_, fn)
            local p, q, s = bone_frame(w, fn:ToString())
            return { Translation = p, Rotation = { X = q[1], Y = q[2], Z = q[3], W = q[4] }, Scale3D = v(s, s, s) }
        end,
    }
end
local function mk_willie(name, armour, hm, pose)
    addr = addr + 1
    pose = pose or {}
    local w = {
        __name = name, __addr = addr, armour = armour or {}, hm = hm or 1.0, gore = {},
        pos = pose.pos or v(0, 0, 100), q = pose.q or { 0, 0, 0, 1 }, s = pose.s or 1.0,
        Health = 100, ["Head Health"] = 100, ["Neck Health"] = 100, ["Body Upper Health"] = 100,
        ["Body Lower Health"] = 100, ["Back Health"] = 100, ["Arm_R Health"] = 100, ["Arm_L Health"] = 100,
        ["Leg_R Health"] = 100, ["Leg_L Health"] = 100, ["Head Health (Crush)"] = 100,
        Consciousness = 100, ["Consciousness 2 (Legs)"] = 100, Stamina = 100, Exhaustion = 0,
        Bleeding = 0, ["Blood Rate"] = 1.5, Pain = 0, ["Sustained Damage"] = 0, ["Damage Taken"] = 0,
        ["Last Damage Taken"] = 0, ["Last Complex Damage Impulse"] = 0, ["Last Complex Damage Bone"] = FName("None"),
        ["Last Damaged Bone"] = FName("None"), ["Force Death"] = false, DED = false,
        Headless = false, Invulnerable = false, ["All Body Tonus"] = 56, ["Flinch Index"] = 6,
        ["Current Pain Threshold"] = 0, ["Dismembered Array"] = {}, ["Get Up Rate"] = 1, ["Dead Weight Scale"] = 1,
        ["Pain Stumble Immediate"] = v(0, 0, 0),
    }
    -- Last Damage Taken lives behind the metatable so the model knows when it
    -- was last written (the reset delay above).
    w.__ldt, w["Last Damage Taken"] = 0, nil
    setmetatable(w, {
        __index = function(t, k) if k == "Last Damage Taken" then return rawget(t, "__ldt") end end,
        __newindex = function(t, k, x)
            if k == "Last Damage Taken" then rawset(t, "__ldt", x); rawset(t, "ldt_w", CLOCK) else rawset(t, k, x) end
        end,
    })
    w.IsValid = valid
    w.GetAddress = function(self) return self.__addr end
    w.GetFName = function(self) return FName(self.__name) end
    w.GetClass = function() return { GetFName = function() return FName("Willie_BP_C") end } end
    w.Mesh = mk_mesh(w)
    w["Deal Complex Damage"] = function(self, hitc, coll, bone, loc, nrm, vel, imp, cp, stab, rig, blunt, lower, dparent, kick, hb, xhv, draw)
        deal_complex(self, hitc, coll, bone, loc, nrm, vel, imp, cp, stab, rig, blunt, lower, dparent, kick, hb, xhv, draw)
    end
    w["Get Damage"] = function() error("an armour-stage claim must never be replayed through Get Damage") end
    return w
end
-- A weapon actor held by `holder` (its collision component is tagged 'Weapon').
local function mk_weapon(holder)
    local wp = { IsValid = valid }
    local comp = { IsValid = valid, ComponentHasTag = function(_, t) return t:ToString() == "Weapon" end }
    comp.GetOwner = function() return wp end
    wp.GetClass = function() return { GetFName = function() return FName("ModularWeaponBP_C") end } end
    wp["Parent Actor"] = holder
    wp["Collision Components Array"] = { ForEach = function(_, f) f(1, { get = function() return comp end }) end }
    wp.K2_GetRootComponent = function() return comp end
    wp.comp = comp
    return wp
end
local function body_comp(actor)
    return { IsValid = valid, GetOwner = function() return actor end, ComponentHasTag = function() return false end }
end

-- One blow: `b` gives the hit in the TARGET's bone space (spot direction,
-- velocity direction); the world blow follows the target's current pose.
local SPOTS = { front = v(1, 0, 0), side = v(0, 1, 0), back = v(-1, 0, 0) }
local function strike(target, attacker, b, at_clock)
    if at_clock then CLOCK = at_clock end
    local p, q, s, r = bone_frame(target, b.bone)
    local dir = SPOTS[b.spot or "front"]
    local loc = add(p, qrot(q, scale(dir, r * s)))
    local vdir = qrot(q, scale(dir, -1))                 -- into the body
    local vel = scale(vdir, b.vel)
    local imp = scale(vdir, b.imp or b.vel * 0.6)
    local coll = b.weapon and attacker.weapon.comp or body_comp(attacker)
    deal_complex(target, target.Mesh, coll, FName(b.bone), loc, qrot(q, dir), vel, imp, b.cut, b.stab, b.rig,
        b.dism or 0, false, false, b.kick or 1, nil, false, b.draw or 0)
end

-- ---- boot HSMPCombat --------------------------------------------------------------
local ATT = mk_willie("Willie_BP_C_ATT")
ATT.weapon = mk_weapon(ATT)
ATT["Weapon R"] = ATT.weapon
PC = { Pawn = ATT, IsValid = valid }
package.preload["UEHelpers"] = function()
    return { GetPlayerController = function() return PC end, GetWorld = function() return nil end }
end
package.path = T.path("mods/shared") .. "/?.lua;" .. package.path
local fn, err = load(T.read(SRC))
T.check(fn ~= nil, "HSMPCombat loads", err)
if not fn then return end
fn()
api = HSMP_COMBAT_TEST.api
api.set_world("world#1")
local NAT = rawget(_G, "HSMPNative")
local IPCF = rawget(_G, "HSMP_IPC")
local ES = IPCF.S.ENUMS
local function hb(age) NAT._st.hb_age = age; IPCF.refresh_info(true) end
hb(0.01)
local function sidecar(status, id) NAT.sc_put("link", { status = ES.sidecar_status[status:upper()], state = ES.link_state.UP, my_peer_id = id }) end
local PHASE = { lobby = 0, countdown = 2, live = 3, roundover = 4, match_over = 5, paused = 7 }
local SESS = { phase = 0, round = 0, winner_seat = 255, rows = {} }
local function publish() NAT.sc_put("session", SESS) end
local function match(state, round) SESS.phase, SESS.round = PHASE[state], round; publish() end
match("live", 3)
api.refresh_match()
sidecar("connected", 1); api.refresh_session()
sidecar("connected", 1); api.refresh_session()
T.check(api.session().connected, "session live")
api.set_my_peer_id(1)

local WATCH = { "Health", "Consciousness", "Bleeding", "Pain", "Head Health", "Neck Health", "Body Upper Health",
                "Body Lower Health", "Arm_R Health", "Arm_L Health", "Leg_R Health", "Leg_L Health",
                "Force Death", "DED", "Headless", "Flinch Index" }
local function state_of(w)
    local s = {}
    for _, k in ipairs(WATCH) do s[k] = w[k] end
    s.gore = table.concat(w.gore, ",")
    -- the stumble in the victim's own frame (solo and MP victims may face differently)
    local st = qrot(w.q, w["Pain Stumble Immediate"], true)
    s.stumble_x, s.stumble_y, s.stumble_z = st.X, st.Y, st.Z
    return s
end
local function delta(a, b)
    local d = {}
    for k, x in pairs(b) do
        if type(x) == "number" then
            local y = x - a[k]
            if math.abs(y) > 1e-9 then d[k] = y end
        elseif x ~= a[k] then d[k] = tostring(a[k]) .. "->" .. tostring(x) end
    end
    return d
end
local function same(a, b)
    for k, x in pairs(a) do
        local y = b[k]
        if type(x) == "number" then
            -- f32 on the wire, doubles in solo: relative 1e-5 (+1e-4 absolute)
            if type(y) ~= "number" or math.abs(x - y) > 1e-5 * math.max(1, math.abs(x)) + 1e-4 then return false, k end
        elseif x ~= y then return false, k end
    end
    for k, y in pairs(b) do
        if a[k] == nil and not (type(y) == "number" and math.abs(y) <= 1e-4) then return false, k end
    end
    return true
end

local hit_id = 0
local RL = dofile(T.path("tools/hsmp-tools/lua-tests/lib/hsmp_native_records.lua"))
local function serve(claim, target)
    hit_id = hit_id + 1
    local t = RL.deep(claim)
    t.hit_id, t.round, t.target_peer_id = hit_id, 3, target or 2
    local r, e = RL.marshal(rawget(_G, "HSMP_IPC").S, "damage_in", t)
    assert(r, "serve: " .. tostring(e))
    return r
end
local SENT = {}
local function drain() for _, m in ipairs(HSMPNative.sc_rec_drain()) do SENT[#SENT + 1] = m end end
local function claims()
    drain()
    local out = {}
    for _, m in ipairs(SENT) do if m.kind == "damage" then out[#out + 1] = m.data end end
    return out
end
local function tick(dt)
    CLOCK = CLOCK + dt
    api.set_tick(math.floor(CLOCK * 30))
    api.flush_claims()
end

-- Poses: where the victim is when the replay runs, and how its stand-in stood
-- on the attacker's screen when the blow landed (turned 70°, leaning 15°).
local VICTIM_POSE = { pos = v(300, 40, 100), q = { 0, 0, 0, 1 }, s = 1.0 }
local STANDIN_POSE = { pos = v(310, 30, 100), q = qmul(qaxis({ 0, 0, 1 }, 70), qaxis({ 0, 1, 0 }, 15)), s = 1.0 }

-- One scenario. `blows` = { {b, t_attacker}, ... } struck in order (each its
-- own tick unless `same_tick`); the replays then run in one victim tick
-- (bunched) unless `spread`.
local function run(blows, opts)
    opts = opts or {}
    CLOCK = CLOCK + 10
    local t0 = CLOCK
    -- SOLO: the blows on the victim at its pose, at their own times.
    api.set_puppets({}, {}, {})
    local VS = mk_willie("Willie_BP_C_SOLO", opts.armour, opts.hm or 0.95, VICTIM_POSE)
    local s0 = state_of(VS)
    for _, e in ipairs(blows) do strike(VS, ATT, e[1], t0 + e[2]) end
    local solo = delta(s0, state_of(VS))

    -- MP, attacker screen: the stand-in, other armour and pose, Invulnerable.
    CLOCK = t0 + 5
    local t1 = CLOCK
    local SI = mk_willie("Willie_BP_C_SI", { "breast", "sallet" }, 1.1, STANDIN_POSE)
    SI.Health = 1000
    api.set_puppets({ ["Willie_BP_C_SI"] = 2 }, { [2] = SI }, nil)
    api.C3.protect(SI)
    if opts.leak then
        -- the game reset Invulnerable: the stand-in takes the blow natively and
        -- HSMPCombat puts it back from the tick's baseline
        SI.Invulnerable = false
        api.C3.baseline(SI)
    end
    PC.Pawn = ATT
    local si0 = state_of(SI)
    local n0 = #claims()
    for _, e in ipairs(blows) do
        strike(SI, ATT, e[1], t1 + e[2])
        if not opts.same_tick then tick(0.001) end
    end
    tick(1.0)
    local mine = {}
    local cl = claims()
    for i = n0 + 1, #cl do mine[#mine + 1] = cl[i] end
    local si_touched = next(delta(si0, state_of(SI))) ~= nil

    -- MP, victim screen: its own pawn at the replay pose; the attacker's stand-in.
    CLOCK = t1 + 20
    local VM = mk_willie("Willie_BP_C_MP", opts.armour, opts.hm or 0.95, VICTIM_POSE)
    local SA = mk_willie("Willie_BP_C_SA")
    SA.weapon = mk_weapon(SA)
    SA["Weapon R"] = SA.weapon
    PC.Pawn = VM
    api.set_puppets({ ["Willie_BP_C_SA"] = 3 }, { [3] = SA }, nil)   -- the attacker is peer 3
    api.C3.baseline(VM)
    local m0 = state_of(VM)
    local res = {}
    if not opts.reject then
        for _, c in ipairs(mine) do
            res[#res + 1] = api.apply_hit(serve(c, 2), 3)
            if opts.spread then CLOCK = CLOCK + 0.3 end
        end
    end
    local mp = delta(m0, state_of(VM))
    PC.Pawn = ATT
    return solo, mp, mine, si_touched, res, SI
end

-- ---- 1. the grid --------------------------------------------------------------------
-- rig / velocities in the range the game produces (subsystems/combat.md):
local WEAPONS = {
    { name = "sword cut",    vel = 1500, cut = 95, stab = 0.05, rig = 0.80, weapon = true },
    { name = "sword thrust", vel = 1100, cut = 90, stab = 1.0, rig = 0.85, weapon = true },
    { name = "axe",          vel = 1400, cut = 85, stab = 0.0, rig = 1.05, weapon = true },
    { name = "mace",         vel = 1300, cut = 0,  stab = 0.0, rig = 3.1, weapon = true, dism = 1 },
    { name = "polearm haft", vel = 1200, cut = 0,  stab = 0.0, rig = 2.4, weapon = true },
    { name = "fist",         vel = 900, imp = 900, cut = 8, stab = 0.0, rig = 1.6 },
    { name = "kick",         vel = 800, imp = 800, cut = 0, stab = 0.0, rig = 0.5, kick = 10, weapon = true },
}
local PARTS = { { "head", "head" }, { "torso", "spine_03" }, { "arm", "lowerarm_r" }, { "leg", "thigh_l" } }

local function f(d, k) local x = d[k]; return type(x) == "number" and string.format("%+.2f", x) or "0" end
local rows = { "| weapon | armour | part | spot | path | Health | Consciousness | Bleeding | Pain | part Health | blood |",
               "|---|---|---|---|---|---|---|---|---|---|---|" }
local n, bad, one_claim, standin_clean, nonzero = 0, 0, true, true, 0
-- per weapon x armour: { blows, identical, identical damage (stumble direction aside), max |Health| error }
local SUM, SUM_ORDER = {}, {}
local function no_dir(d) local c = {}; for k, x in pairs(d) do if not k:find("^stumble") then c[k] = x end end; return c end
for _, ar in ipairs(ARMOUR) do
    for _, w in ipairs(WEAPONS) do
        for _, pt in ipairs(PARTS) do
            for _, spot in ipairs({ "front", "side", "back" }) do
                local b = {}
                for k, x in pairs(w) do b[k] = x end
                b.bone, b.spot = pt[2], spot
                local solo, mp, mine, si_touched = run({ { b, 0 } }, { armour = ar.set })
                n = n + 1
                local ok, k = same(solo, mp)
                local key = w.name .. " | " .. ar.name
                local sm = SUM[key]
                if not sm then sm = { 0, 0, 0, 0 }; SUM[key] = sm; SUM_ORDER[#SUM_ORDER + 1] = key end
                sm[1] = sm[1] + 1
                if ok then sm[2] = sm[2] + 1 end
                if same(no_dir(solo), no_dir(mp)) then sm[3] = sm[3] + 1 end
                sm[4] = math.max(sm[4], math.abs((solo.Health or 0) - (mp.Health or 0)), math.abs((solo.Consciousness or 0) - (mp.Consciousness or 0)))
                if not ok then
                    bad = bad + 1
                    if bad <= 12 then
                        T.log(string.format("MISMATCH %s / %s / %s / %s at %s: solo %s mp %s", w.name, ar.name, pt[1], spot,
                            tostring(k), T.repr(solo), T.repr(mp)))
                    end
                end
                if (solo.Health or 0) ~= 0 or (solo.Consciousness or 0) ~= 0 or (solo.Bleeding or 0) ~= 0 then nonzero = nonzero + 1 end
                if #mine ~= 1 then one_claim = false; T.log(w.name .. "/" .. ar.name .. ": claims " .. #mine) end
                if si_touched then standin_clean = false end
                if spot ~= "side" then
                    local pk = PH[part_of(b.bone)]
                    for _, row in ipairs({ { "solo", solo }, { "MP", mp } }) do
                        local d = row[2]
                        rows[#rows + 1] = string.format("| %s | %s | %s | %s | %s | %s | %s | %s | %s | %s | %s |", w.name, ar.name,
                            pt[1], spot, row[1], f(d, "Health"), f(d, "Consciousness"), f(d, "Bleeding"), f(d, "Pain"), f(d, pk),
                            d.gore and (select(2, d.gore:gsub(":", ":")) .. " mark(s)") or "0")
                    end
                end
            end
        end
    end
end
do
    local s = { "| weapon | armour | blows | identical | same damage (direction aside) | max Health / Consciousness error |",
                "|---|---|---|---|---|---|" }
    for _, key in ipairs(SUM_ORDER) do
        local sm = SUM[key]
        s[#s + 1] = string.format("| %s | %d | %d | %d | %.2f |", key, sm[1], sm[2], sm[3], sm[4])
    end
    T.log("\nParity summary (4 parts x 3 spots per cell):\n" .. table.concat(s, "\n") .. "\n")
end
T.log(string.format("\nDamage parity, solo vs MP: %d of %d blows identical (stand-in turned 70°, leaning 15°, other armour):\n",
    n - bad, n) .. table.concat(rows, "\n") .. "\n")
if REPORT_ONLY then return end
T.check(bad == 0, string.format("1. every weapon x armour x part x spot (%d blows): MP == solo, field by field", n),
    string.format("%d mismatched", bad))
T.check(nonzero > n / 2, "1. most of the grid really does damage (equal is not 'both zero')", nonzero)
T.check(one_claim, "2. one clean blow = exactly one claim")
T.check(standin_clean, "2. the Invulnerable stand-in takes nothing from the blow it claims")

-- 1b. the stand-in a different size from the victim (bone-space offsets are scale-free)
do
    local save = STANDIN_POSE.s
    STANDIN_POSE.s = 1.12
    local b = { bone = "head", spot = "side", vel = 1300, cut = 0, stab = 0, rig = 3.1, weapon = true }
    local solo, mp = run({ { b, 0 } }, { armour = ARMOUR[5].set })
    STANDIN_POSE.s = save
    T.check(same(solo, mp), "1b. a taller stand-in: the blow still lands on the same spot", T.repr({ solo = solo, mp = mp }))
end

-- ---- 3. two frames of one contact in one tick ---------------------------------------
do
    local graze = { bone = "spine_03", spot = "front", vel = 500, imp = 200, cut = 40, stab = 0, rig = 0.8, weapon = true }
    local hard = { bone = "spine_03", spot = "front", vel = 1600, cut = 95, stab = 0, rig = 0.8, weapon = true }
    local solo, mp, mine = run({ { graze, 0 }, { hard, 0.016 } }, { armour = ARMOUR[2].set, same_tick = true })
    T.check(#mine == 2, "3. a graze then a harder frame in one tick: two claims", #mine)
    T.check(same(solo, mp), "3. ...and both land, as in solo", T.repr({ solo = solo, mp = mp }))
    -- a weaker frame inside the gate window is stopped on the stand-in, as on the victim in solo
    local soft = { bone = "spine_03", spot = "front", vel = 700, cut = 30, stab = 0, rig = 0.8, weapon = true }
    solo, mp, mine = run({ { hard, 0 }, { soft, 0.016 } }, { armour = ARMOUR[2].set, same_tick = true })
    T.check(#mine == 1 and same(solo, mp), "3. a weaker second frame: one claim, solo applies one", T.repr({ n = #mine, solo = solo, mp = mp }))
end

-- ---- 4. blows far apart, replays bunched ------------------------------------------------
do
    local b1 = { bone = "head", spot = "front", vel = 1500, cut = 0, stab = 0, rig = 3.1, weapon = true }
    local b2 = { bone = "head", spot = "front", vel = 900, cut = 0, stab = 0, rig = 3.1, weapon = true }
    local solo, mp, mine = run({ { b1, 0 }, { b2, 0.35 } }, { armour = ARMOUR[1].set })
    T.check(#mine == 2, "4. two blows 350 ms apart: two claims", #mine)
    T.check((solo.Consciousness or 0) < 0 and same(solo, mp),
        "4. ...replayed in the same victim tick (parry hold, jitter): the second is not gated, as in solo",
        T.repr({ solo = solo, mp = mp }))
    -- frames 40 ms apart on the attacker's clock, the second weaker: gated in both
    local b3 = { bone = "head", spot = "front", vel = 1500, cut = 0, stab = 0, rig = 3.1, weapon = true }
    local b4 = { bone = "head", spot = "front", vel = 1450, imp = 1200, cut = 0, stab = 0, rig = 3.1, weapon = true }
    solo, mp, mine = run({ { b3, 0 }, { b4, 0.15 } }, { armour = ARMOUR[1].set })
    T.check(same(solo, mp), "4. a weaker blow 150 ms after the first is gated on the victim as in solo",
        T.repr({ n = #mine, solo = solo, mp = mp }))
    -- the same two blows, the second replay 300 ms after the first (jitter, a
    -- parry hold): the game's own 0.2 s reset has fired on the victim by then
    solo, mp, mine = run({ { b3, 0 }, { b4, 0.15 } }, { armour = ARMOUR[1].set, spread = true })
    T.check(#mine == 2 and same(solo, mp), "4. ...also when its replay arrives 300 ms after the first",
        T.repr({ n = #mine, solo = solo, mp = mp }))
    solo, mp, mine = run({ { b1, 0 }, { b2, 0.35 } }, { armour = ARMOUR[1].set, spread = true })
    T.check(#mine == 2 and same(solo, mp), "4. ...and blows 350 ms apart replayed 300 ms apart both land",
        T.repr({ n = #mine, solo = solo, mp = mp }))
end

-- ---- 4b. the stand-in's Invulnerable did not hold ---------------------------------------
do
    -- A hard frame on the open back plate of the stand-in, then one with less
    -- impulse (stopped by Deal Complex Damage's contact gate in solo) but more
    -- speed (it would pass Get Damage's gate).
    local hard = { bone = "spine_03", spot = "back", vel = 1600, cut = 95, stab = 0, rig = 0.8, weapon = true }
    local fast = { bone = "spine_03", spot = "back", vel = 1700, imp = 300, cut = 95, stab = 0, rig = 0.8, weapon = true }
    local solo, mp, mine = run({ { hard, 0 }, { fast, 0.016 } }, { armour = ARMOUR[2].set, same_tick = true })
    T.check(#mine == 1 and same(solo, mp), "4b. a frame the contact gate stops: one claim, as in solo",
        T.repr({ n = #mine, solo = solo, mp = mp }))
    solo, mp, mine = run({ { hard, 0 }, { fast, 0.016 } }, { armour = ARMOUR[2].set, same_tick = true, leak = true })
    T.check(#mine == 1 and same(solo, mp),
        "4b. ...also when the stand-in took the first frame natively and was put back (its contact gate is kept)",
        T.repr({ n = #mine, solo = solo, mp = mp }))
end

-- ---- 5. light weapon blow: Get Damage's 'Weapon' factor ----------------------------
do
    local b = { bone = "head", spot = "side", vel = 280, cut = 0, stab = 0, rig = 1.0, weapon = true, imp = 1100 }
    local solo, mp = run({ { b, 0 } }, { armour = ARMOUR[1].set })
    T.check((solo.Consciousness or 0) < 0 and same(solo, mp), "5. a light weapon blow: consciousness as in solo (weapon tag)",
        T.repr({ solo = solo, mp = mp }))
    local fist = { bone = "head", spot = "side", vel = 280, cut = 0, stab = 0, rig = 1.0, imp = 1100 }
    solo, mp = run({ { fist, 0 } }, { armour = ARMOUR[1].set })
    T.check(same(solo, mp), "5. ...and the same light blow by a fist (no weapon tag)", T.repr({ solo = solo, mp = mp }))
end

-- ---- 6. echo, rejected blow, blood on the stand-in -----------------------------------
do
    -- the stand-in's own copy of a body blow lands on the victim first: undone
    local VM = mk_willie("Willie_BP_C_ECHO", ARMOUR[2].set, 1, VICTIM_POSE)
    local SA = mk_willie("Willie_BP_C_SA3")
    PC.Pawn = VM
    api.set_puppets({ ["Willie_BP_C_SA3"] = 1 }, { [1] = SA }, nil)
    api.C3.baseline(VM)
    local m0 = state_of(VM)
    HSMPNative.bus_put("playback", { rows = { { peer = 1, body_ts = 5000, arm_ts = 5100, local_ms = math.floor(CLOCK * 1000) } } })
    api.set_tick(math.floor(CLOCK * 30) + 7)
    strike(VM, SA, { bone = "head", spot = "front", vel = 900, imp = 900, cut = 0, stab = 0, rig = 1.0 })
    T.check(next(delta(m0, state_of(VM))) == nil, "6. the stand-in's echo on the victim is undone", T.repr(delta(m0, state_of(VM))))
    drain()
    local touch = T.filter(SENT, function(m) return m.kind == "touch" end)
    T.check(#touch >= 1 and touch[#touch].data.other_peer_id == 1, "6. the echo is reported as a touch")
    PC.Pawn = ATT

    local b = { bone = "neck_01", spot = "front", vel = 1500, cut = 100, stab = 0, rig = 0.85, weapon = true }
    local _, mp = run({ { b, 0 } }, { armour = ARMOUR[1].set, reject = true })
    T.check(next(mp) == nil, "6. a rejected blow leaves the victim exactly as it was", T.repr(mp))

    local _, _, mine, _, _, SI = run({ { b, 0 } }, { armour = ARMOUR[1].set })
    SI.gore = {}
    local s0 = state_of(SI)
    s0.gore = nil
    api.set_puppets({ ["Willie_BP_C_SI"] = 2 }, { [2] = SI }, nil)
    local r = api.C3.apply_fx(serve(mine[1], 2), 1)
    local s1 = state_of(SI)
    s1.gore = nil
    T.check(T.contains(r, "replayed") and #SI.gore >= 1, "6. the accepted blow's blood appears on the stand-in", T.repr({ r, SI.gore }))
    T.check(same(s0, s1) and SI.Invulnerable == true and SI.Health >= 1000,
        "6. ...its vitals stay the owner's (restored) and it is Invulnerable again", T.repr(delta(s0, s1)))
    T.check(T.contains(api.C3.apply_fx(serve(mine[1], 1), 2), "own hit"), "6. a hit on me is never replayed as fx")
    local SA2 = mk_willie("Willie_BP_C_SA2")
    SA2["Weapon R"] = { IsValid = valid, ["Temp Disable Damage"] = false }
    SA2["Weapon L"] = { IsValid = valid, ["Temp Disable Damage"] = false }
    api.C3.protect(SA2)
    T.check(SA2["Weapon R"]["Temp Disable Damage"] == true and SA2["Weapon L"]["Temp Disable Damage"] == true
        and SA2.Invulnerable == true, "6. stand-in weapons get the game's damage gate, the stand-in Invulnerable")
end
