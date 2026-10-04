-- The owner's passport body on its stand-ins (HSMPCombat standin_body.lua):
--
--     hsmp-tools lua-test standin_body
--
--   * the owner's Height Rate, Muscle Rate, "Mass Scale (Set in BP)",
--     "Character Scale (Set in BP)" and bone masses become one `body` record in
--     the game slot `body`, a new version only when the body changed;
--   * a peer's `peer_body` record gives its stand-in the owner's bone masses
--     (SetMassScale on the stand-in's Mesh, from the mass the stand-in has now)
--     and rates; the geometry is not touched; a mass the game puts back is set
--     again and counted;
--   * it runs from HSMPCombat's own tick (owner every 2 s, stand-ins once a
--     second), and a world drop forgets the stand-ins.

local T = T
local STATE = T.tmpdir("hsmp_body_test_")
local SRC = T.path("mods/HSMPCombat/Scripts/main.lua")

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

local function valid() return true end
-- A Willie's Mesh: each body's mass = its base mass x its mass scale. `calls`
-- records every SetMassScale (the name must be an FName).
local function mk_mesh(base, scale)
    local m = { base = base, scale = {}, calls = {} }
    for b, s in pairs(scale or {}) do m.scale[b] = s end
    m.IsValid = valid
    m.GetBoneMass = function(self, fn, scaled)
        local b = fn:ToString()
        if not self.base[b] then return 0 end
        return self.base[b] * ((scaled and self.scale[b]) or (scaled and 1) or 1)
    end
    m.GetMassScale = function(self, fn) local b = fn:ToString(); return self.base[b] and (self.scale[b] or 1) or 0 end
    m.SetMassScale = function(self, fn, s)
        assert(type(fn) == "table" and fn.ToString, "SetMassScale needs an FName")
        self.scale[fn:ToString()] = s
        self.calls[#self.calls + 1] = fn:ToString()
    end
    m.GetSocketLocation = function() return { X = 0, Y = 0, Z = 0 } end
    m.GetSocketTransform = function()
        return { Translation = { X = 0, Y = 0, Z = 0 }, Rotation = { X = 0, Y = 0, Z = 0, W = 1 }, Scale3D = { X = 1, Y = 1, Z = 1 } }
    end
    return m
end
local addr = 7000
local function mk_willie(name, body, mesh)
    addr = addr + 1
    local w = {
        __name = name, __addr = addr, Health = 100, Invulnerable = false, ["Team Int"] = 1,
        ["Height Rate"] = body.h, ["Muscle Rate"] = body.m, ["Mass Scale (Set in BP)"] = body.ms,
        ["Character Scale (Set in BP)"] = { X = body.cs, Y = body.cs, Z = body.cs },
    }
    w.IsValid = valid
    w.GetAddress = function(self) return self.__addr end
    w.GetFName = function(self) return FName(self.__name) end
    w.GetClass = function() return { GetFName = function() return FName("Willie_BP_C") end } end
    w.Mesh = mesh
    return w
end

-- The owner: light, a little short. Its stand-in on the other screen: a pooled
-- foe with 2-2.6x the bone masses (as measured in game).
local OWNER_BASE = { pelvis = 6.05, spine_03 = 4.1, head = 3.2, upperarm_r = 1.9, thigh_l = 5.0 }
local FOE_BASE = { pelvis = 15.7, spine_03 = 9.0, head = 7.1, upperarm_r = 4.9, thigh_l = 11.5, calf_r = 3.0 }
local ME = mk_willie("Willie_BP_C_ME", { h = 0.946, m = 0.018, ms = 1.005, cs = 1.0 }, mk_mesh(OWNER_BASE, { spine_03 = 1.2 }))
local SI_MESH = mk_mesh(FOE_BASE, { pelvis = 1.21 })
local SI = mk_willie("Willie_BP_C_44", { h = 0.66, m = 0.85, ms = 1.21, cs = 0.93 }, SI_MESH)
PC = { Pawn = ME, IsValid = valid }
package.preload["UEHelpers"] = function()
    return { GetPlayerController = function() return PC end, GetWorld = function() return nil end }
end
package.path = T.path("mods/shared") .. "/?.lua;" .. T.path("mods/HSMPCombat/Scripts") .. "/?.lua;" .. package.path
local fn, err = load(T.read(SRC))
T.check(fn ~= nil, "HSMPCombat loads", err)
if not fn then return end
fn()
local api = HSMP_COMBAT_TEST.api
local C3 = api.C3
T.check(C3.BODY ~= nil, "standin_body.lua is loaded next to main.lua")
if not C3.BODY then return end
local B = C3.BODY
api.set_world("world#1")
local NAT = rawget(_G, "HSMPNative")
local IPCF = rawget(_G, "HSMP_IPC")

-- ---- owner side ------------------------------------------------------------------
local function own_rec() return (IPCF.rec("body")) end
C3.body_publish(ME)
local r = own_rec()
T.check(r ~= nil and r.version > 0, "the own body is written to the `body` slot", T.repr(r))
if not r then return end
T.check(math.abs(r.height_rate - 0.946) < 1e-4 and math.abs(r.muscle_rate - 0.018) < 1e-4
    and math.abs(r.mass_scale_bp - 1.005) < 1e-4 and T.eq(r.char_scale, { 1.0, 1.0, 1.0 }),
    "rates, BP mass scale and character scale as the pawn has them", T.repr(r))
local by = {}
for _, row in ipairs(r.rows) do by[row.bone] = row end
T.check(#r.rows == 5 and by.pelvis and math.abs(by.pelvis.mass - 6.05) < 1e-4
    and math.abs(by.spine_03.mass - 4.1 * 1.2) < 1e-4 and math.abs(by.spine_03.mass_scale - 1.2) < 1e-5,
    "one row per simulated body the mesh has: scaled mass and mass scale", T.repr(r.rows))
local v1 = r.version
C3.body_publish(ME)
T.check(own_rec().version == v1, "an unchanged body is not a new version")
ME.Mesh.scale.thigh_l = 1.3   -- armour put on: heavier legs
C3.body_publish(ME)
local r2 = own_rec()
T.check(r2.version > v1, "a changed body is a new, higher version", T.repr({ v1, r2.version }))
do
    local bad = mk_willie("Willie_BP_C_BAD", { h = 0.5, m = 0.5, ms = 1, cs = 1 }, mk_mesh({}))
    local before = own_rec().version
    C3.body_publish(bad)
    T.check(own_rec().version == before, "no readable bone mass: nothing written")
end

-- ---- stand-in side ---------------------------------------------------------------
NAT.sc_put("peer_body", r2, 2)
IPCF.peer_dir(true)
api.set_puppets({ ["Willie_BP_C_44"] = 2 }, { [2] = SI }, nil)
C3.body_apply(2, SI)
local function mass(m, b) return m:GetBoneMass(FName(b), true) end
local worst = 0
for b, base in pairs(OWNER_BASE) do
    local want = base * (ME.Mesh.scale[b] or 1)
    worst = math.max(worst, math.abs(mass(SI_MESH, b) - want) / want)
end
T.check(worst < 1e-6, "every owner bone: the stand-in now has the owner's mass", worst)
T.check(math.abs(mass(SI_MESH, "calf_r") - 3.0) < 1e-9, "a body the owner did not send keeps its mass")
T.check(SI["Height Rate"] == r2.height_rate and SI["Muscle Rate"] == r2.muscle_rate
    and SI["Mass Scale (Set in BP)"] == r2.mass_scale_bp, "rates and the BP mass scale follow the owner")
T.check(SI["Character Scale (Set in BP)"].X == 0.93, "the geometry is not touched (Avatars measures it once)")
local n_calls = #SI_MESH.calls
C3.body_apply(2, SI)
T.check(#SI_MESH.calls == n_calls, "applied again: nothing to set")
SI_MESH.scale.pelvis = 1.21   -- the game put the pooled foe's mass back
C3.body_apply(2, SI)
T.check(#SI_MESH.calls == n_calls + 1 and math.abs(mass(SI_MESH, "pelvis") - 6.05) < 1e-6 and B.fight_count() == 1,
    "a mass the game reset is set again and counted", T.repr({ calls = SI_MESH.calls, fights = B.fight_count() }))
T.check(T.contains(table.concat(LOGS, "\n"), "the game reset 1 before"), "...and logged")

-- ---- from HSMPCombat's tick ------------------------------------------------------
do
    SI_MESH.scale.head = 3.0
    local n0 = #SI_MESH.calls
    api.set_tick(29)          -- (tick + peer) % 30 ~= 0
    api.update_standins()
    T.check(#SI_MESH.calls == n0, "stand-ins are checked once a second, not every tick")
    api.set_tick(28)          -- 28 + 2
    api.update_standins()
    T.check(#SI_MESH.calls == n0 + 1 and SI.Invulnerable == true, "update_standins applies the body (and still protects)",
        T.repr(SI_MESH.calls))
    api.wg_drop("test")
    T.check(next(B.applied) == nil and B.fight_count() == 0, "a world drop forgets the stand-ins (names are reused)")
end
