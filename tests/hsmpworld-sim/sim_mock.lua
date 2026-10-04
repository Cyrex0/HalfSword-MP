-- sim_mock.lua: the slice of UE4SS / UE 5.4 that HSMPWorld touches, backed by the
-- simulator's per-client physics (PHYS.*) and clock / pawn state (SIM.*), both provided by
-- the Rust host (tests/hsmpworld-sim/src/game.rs). One copy per client Lua state.
--
-- Faithful where sync depends on it: weapons are ModularWeaponBP_C actors whose BaseMesh
-- simulates unless a Willie holds them ("Is Held" / "Parent Actor"); props are
-- StaticMeshActors with a simulating StaticMeshComponent; a teleport keeps the body's
-- velocity unless the caller sets it; the world changes address on an arena reload.

local M = { weapons = {}, props = {}, puppets = {}, next_addr = 0x10000, spawned = 0, world_n = 1 }

local function v3(x, y, z) return { X = x, Y = y, Z = z } end
local function FN(s) return { ToString = function() return s end } end
M.FN = FN
local function addr() M.next_addr = M.next_addr + 0x40; return M.next_addr end

local MAP = "/Game/Maps/SimArena"
local function world_name() return "World " .. MAP .. ".SimArena" end

-- ---- classes ------------------------------------------------------------------
local function class(short, path, is_weapon)
    local c = { _short = short, _path = path }
    local cdo = { IsA = function(_, b) return is_weapon and b == M.weapon_base end }
    function c:IsValid() return true end
    function c:GetFName() return FN(short) end
    function c:GetFullName() return "BlueprintGeneratedClass " .. path end
    function c:GetCDO() return cdo end
    function c:GetAddress() return 0x777 end
    return c
end
M.weapon_base = class("ModularWeaponBP_C", "/Game/Assets/Weapons/Blueprints/ModularWeaponBP.ModularWeaponBP_C", true)
M.classes = { [M.weapon_base._path] = M.weapon_base }
M.smc = class("StaticMeshComponent", "/Script/Engine.StaticMeshComponent", false)
M.classes["/Script/Engine.StaticMeshComponent"] = M.smc
M.sma = class("StaticMeshActor", "/Script/Engine.StaticMeshActor", false)
M.willie_cls = class("Willie_BP_C", "/Game/Character/Blueprints/Willie_BP.Willie_BP_C", false)
function M.weapon_class(short)
    local path = "/Game/Assets/Weapons/Blueprints/Built_Weapons/" .. short:gsub("_C$", "") .. "." .. short
    local c = M.classes[path]
    if not c then c = class(short, path, true); M.classes[path] = c end
    return c
end

local LEVEL = {}
function LEVEL:GetClass() return { GetFName = function() return FN("Level") end } end
function LEVEL:GetFullName() return "Level " .. MAP .. ".SimArena:PersistentLevel" end
function LEVEL:IsValid() return true end

-- ---- components -----------------------------------------------------------------
local Comp = {}
Comp.__index = Comp
function Comp:IsValid() return self._valid and PHYS.alive(self._idx) end
function Comp:GetAddress() return self._addr end
function Comp:GetFName() return FN(self._cname) end
function Comp:IsSimulatingPhysics() return PHYS.is_sim(self._idx) end
function Comp:SetSimulatePhysics(on) PHYS.set_sim(self._idx, on == true) end
function Comp:K2_GetComponentLocation() local x, y, z = PHYS.pos(self._idx); return v3(x, y, z) end
function Comp:K2_GetComponentRotation() local p, y, r = PHYS.rot(self._idx); return { Pitch = p, Yaw = y, Roll = r } end
function Comp:K2_SetWorldLocationAndRotation(p, r, sweep, hit, tp)
    PHYS.set_tf(self._idx, p.X, p.Y, p.Z, r.Pitch, r.Yaw, r.Roll)
end
function Comp:SetPhysicsLinearVelocity(v) PHYS.set_vel(self._idx, v.X, v.Y, v.Z) end
function Comp:SetPhysicsAngularVelocityInDegrees(w) PHYS.set_angvel(self._idx, w.X, w.Y, w.Z) end
function Comp:PutRigidBodyToSleep() PHYS.sleep(self._idx) end
function Comp:GetPhysicsLinearVelocity() local x, y, z = PHYS.vel(self._idx); return v3(x, y, z) end
function Comp:AddImpulse(v) PHYS.impulse(self._idx, v.X, v.Y, v.Z) end
local function comp(cname, idx)
    return setmetatable({ _idx = idx, _cname = cname, _valid = true, _addr = addr(), Mobility = 2 }, Comp)
end

-- ---- actors ---------------------------------------------------------------------------
local Actor = {}
Actor.__index = Actor
function Actor:IsValid() return self._valid end
function Actor:GetAddress() return self._addr end
function Actor:GetFName() return FN(self._name) end
function Actor:GetFullName() return self._cls._short .. " " .. MAP .. ".SimArena:PersistentLevel." .. self._name end
function Actor:HasAnyFlags() return false end
function Actor:GetClass() return self._cls end
function Actor:GetOuter() return LEVEL end
function Actor:K2_GetRootComponent() return self._root end
function Actor:K2_GetActorLocation()
    if self._loc then return self._loc() end
    return self._root and self._root:K2_GetComponentLocation() or v3(0, 0, 0)
end
function Actor:GetAttachParentActor() return self._parent end
function Actor:SetActorHiddenInGame(b) self._hidden = b end
function Actor:SetActorEnableCollision(b) self._collide = b end
function Actor:SetActorTickEnabled(b) end
function Actor:K2_DestroyActor()
    if self._cls == M.willie_cls then SIM.violation("K2_DestroyActor on a Willie"); return end
    self._valid = false
    if self._root then self._root._valid = false; PHYS.kill(self._root._idx) end
end
function Actor:K2_GetComponentsByClass() return nil end

local function actor(cls, name)
    return setmetatable({ _cls = cls, _name = name, _valid = true, _addr = addr() }, Actor)
end

function M.passport(seed)
    return { ID_70_C02CF656483647A1933EEA96314B78A6 = seed }
end

-- A scene weapon whose BaseMesh is body `idx`.
function M.add_weapon(idx, short, name)
    local a = actor(M.weapon_class(short), name)
    a._root = comp("BaseMesh", idx)
    a.BaseMesh = a._root
    a["Is Held"] = false
    a["Simulates Physics"] = true
    a["Weapon Passport"] = M.passport(idx)
    M.weapons[#M.weapons + 1] = a
    return a
end

-- A physics prop (StaticMeshActor with a simulating StaticMeshComponent).
function M.add_prop(idx, name)
    local a = actor(M.sma, name)
    a._root = comp("StaticMeshComponent0", idx)
    a.StaticMeshComponent = a._root
    M.props[#M.props + 1] = a
    return a
end

-- My pawn and the stand-ins.
M.pawn = actor(M.willie_cls, "Willie_BP_C_1")
M.pawn._loc = function() local x, y, z = SIM.pawn_pos(); return v3(x, y, z) end
function M.add_puppet(peer)
    local a = actor(M.willie_cls, "Willie_BP_C_" .. tostring(2147482000 + peer))
    a._loc = function() local x, y, z = SIM.puppet_pos(peer); return v3(x, y, z) end
    M.puppets[peer] = a
    return a
end
function M.remove_puppet(peer) M.puppets[peer] = nil end

-- The pawn picks a weapon actor up / lets it go (what the game's own pickup / drop does).
function M.hold(a, side)
    M.pawn["Weapon " .. side] = a
    a["Is Held"] = true
    a["Parent Actor"] = M.pawn
    a["Last Parent"] = M.pawn
end
function M.release(a, side)
    if M.pawn["Weapon " .. side] == a then M.pawn["Weapon " .. side] = nil end
    a["Is Held"] = false
    a["Parent Actor"] = nil
end
function M.held_idx(side)
    local a = M.pawn["Weapon " .. side]
    return a and a._valid and a._root and a._root._idx or nil
end

-- ---- engine surface ---------------------------------------------------------------------
M.world = { _addr = 0x5000 }
function M.world:IsValid() return true end
function M.world:GetFullName() return world_name() end
function M.world:GetAddress() return self._addr end

M.pc = { Pawn = M.pawn }
function M.pc:IsValid() return true end
function M.pc:GetWorld() return M.world end
function M.pc:GetFName() return FN("PlayerController_0") end

M.gs = {}
function M.gs:IsValid() return true end
function M.gs:BeginDeferredActorSpawnFromClass(world, cls, t, coll, owner, scale)
    local q = t.Rotation
    local idx = PHYS.spawn(t.Translation.X, t.Translation.Y, t.Translation.Z, q.X, q.Y, q.Z, q.W)
    M.spawned = M.spawned + 1
    local a = actor(cls, cls._short .. "_" .. tostring(2147480000 + M.spawned))
    a._root = comp("BaseMesh", idx)
    a.BaseMesh = a._root
    a["Is Held"] = false
    a["Simulates Physics"] = false
    a._deferred = true
    return a
end
function M.gs:FinishSpawningActor(a, t, scale)
    a._deferred = false
    local q = t.Rotation
    PHYS.place_q(a._root._idx, t.Translation.X, t.Translation.Y, t.Translation.Z, q.X, q.Y, q.Z, q.W)
    PHYS.set_sim(a._root._idx, a["Simulates Physics"] == true)
    M.weapons[#M.weapons + 1] = a
    return a
end

-- An arena reload: a new UWorld (new address); the host rebuilds the bodies and calls
-- add_weapon / add_prop again.
function M.reload()
    M.world_n = M.world_n + 1
    M.world._addr = 0x5000 + M.world_n * 0x1000
    for _, a in ipairs(M.weapons) do a._valid = false; if a._root then a._root._valid = false end end
    for _, a in ipairs(M.props) do a._valid = false; if a._root then a._root._valid = false end end
    M.weapons, M.props = {}, {}
    M.pawn["Weapon R"], M.pawn["Weapon L"] = nil, nil
end

function M.install(G)
    G.FName = FN
    G.StaticFindObject = function(path) return M.classes[path] end
    G.LoadAsset = function() end
    G.FindAllOf = function(name)
        local out = {}
        if name == "ModularWeaponBP_C" then
            for _, a in ipairs(M.weapons) do if a._valid then out[#out + 1] = a end end
        elseif name == "StaticMeshActor" then
            for _, a in ipairs(M.props) do if a._valid then out[#out + 1] = a end end
        elseif name == "Willie_BP_C" then
            out[1] = M.pawn
            for _, a in pairs(M.puppets) do out[#out + 1] = a end
        else
            return nil
        end
        return out
    end
    G.print = function(s) SIM.log(s) end
    G.RegisterHook = function() end
    G.RegisterLoadMapPreHook = function() end
    os.clock = function() return SIM.clock_s() end
    -- harness runs turn on HSMPWorld's world_track events (WORLD-2)
    local getenv = os.getenv
    os.getenv = function(k) if k == "HSMP_WORLD_TRACK" then return "1" end return getenv(k) end
    package.preload["UEHelpers"] = function()
        return {
            GetWorld = function() return M.world end,
            GetGameplayStatics = function() return M.gs end,
            GetPlayerController = function() return M.pc end,
        }
    end
    -- Structured events go to the host (the gate's events in game).
    package.preload["hsmp_log"] = function()
        return { init = function() end, event = function(name, f) SIM.event(name, f) end }
    end
    -- The IPC facade is the host bridge below (HSMP_IPC), installed after main.lua loads.
    package.preload["hsmp_ipc"] = function() return { init = function() end } end
    package.preload["hsmp_session"] = function() return nil end
end

-- The HSMP_IPC facade HSMPWorld uses, over the host: records go to SIM.*, slots come back
-- through M.slot_set (one generation per publish, as the native module).
M.slots, M.bus = {}, {}
function M.slot_set(slot, t)
    local s = M.slots[slot]
    M.slots[slot] = { t = t, gen = (s and s.gen or 0) + 1 }
end
M.ipc = {
    send = function(kind, data) SIM.g2s(kind, data); return true end,
    put = function(slot, data) SIM.put(slot, data); return true end,
    rec = function(slot) local s = M.slots[slot]; if s then return s.t, s.gen end return nil end,
    bus_table = function(key) return M.bus[key] end,
    bus_put = function(key, t) M.bus[key] = t; SIM.bus_put(key, t); return true end,
}

return M
