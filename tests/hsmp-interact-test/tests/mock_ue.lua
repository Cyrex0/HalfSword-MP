-- Mocked UE4SS 3.0.1 + Half Sword Willies for the HSMPInteract offline tests
-- (tests/interact.rs). Objects are tables with a metatable: methods come
-- from `Methods`, other fields are properties. After M.kill_all() (the old
-- world is freed) ANY access to an old object (IsValid included) is recorded in
-- M.dead_touch; the tests require it to stay empty (world guard).

M = { now = 0, frame_ms = 8, loops = {}, frames = {}, delayed = {}, seq = 0, logs = {}, dead_touch = {},
      objs = {}, addr = 0, env = {}, hooks = {}, impulses = {}, handles = {}, grabs = {}, willies = {} }

local Methods = {}
local ObjMT = {}
ObjMT.__index = function(t, k)
    -- IsValid() on a freed object is a touch too (it reads freed memory in the game)
    if rawget(t, "__dead") then table.insert(M.dead_touch, tostring(rawget(t, "__name")) .. "." .. k) end
    local m = Methods[k]; if m then return m end
    return rawget(t, "__props")[k]
end
ObjMT.__newindex = function(t, k, v)
    if rawget(t, "__dead") then table.insert(M.dead_touch, tostring(rawget(t, "__name")) .. "." .. k .. "=") end
    t.__props[k] = v
end

function new_obj(cls, name)
    M.addr = M.addr + 1
    local o = setmetatable({ __cls = cls, __name = name, __props = {}, __addr = M.addr }, ObjMT)
    table.insert(M.objs, o)
    return o
end

-- FName: a string-like object with ToString.
function fname(s) return { ToString = function() return s end } end
function FName(s) return s end
FNAME_Add = 1

Methods.IsValid = function(self)
    if rawget(self, "__dead") then error("access violation: IsValid on freed " .. tostring(rawget(self, "__name"))) end
    return true
end
Methods.GetFName = function(self) return fname(rawget(self, "__name")) end
Methods.GetAddress = function(self) return rawget(self, "__addr") end
Methods.GetClass = function(self) local c = rawget(self, "__cls"); return { GetFName = function() return fname(c) end } end
Methods.GetFullName = function(self) return rawget(self, "fullname") or ("Obj " .. tostring(rawget(self, "__name"))) end
Methods.GetWorld = function() return M.world end
Methods.GetOwner = function(self) return rawget(self, "owner") end

-- Skeletal mesh: rawget(self, "bones")[name] = {x,y,z}, "quats"[name] = {x,y,z,w}.
local function bone_of(self, b)
    local bs = rawget(self, "bones") or {}
    if bs[b] then return b end
    for k in pairs(bs) do if k:lower() == tostring(b):lower() then return k end end
    return nil
end
Methods.GetSocketLocation = function(self, b)
    local k = bone_of(self, b); if not k then return nil end
    local p = rawget(self, "bones")[k]
    return { X = p[1], Y = p[2], Z = p[3] }
end
Methods.GetSocketQuaternion = function(self, b)
    local k = bone_of(self, b)
    local q = k and (rawget(self, "quats") or {})[k] or { 0, 0, 0, 1 }
    return { X = q[1], Y = q[2], Z = q[3], W = q[4] }
end
Methods.IsSimulatingPhysics = function(self) return rawget(self, "sim") == true end
Methods.IsVisible = function(self) return rawget(self, "vis") ~= false end
-- rawget(self, "vel") = {x,y,z} for every body of the component (default rest).
Methods.GetPhysicsLinearVelocity = function(self)
    local v = rawget(self, "vel") or { 0, 0, 0 }
    return { X = v[1], Y = v[2], Z = v[3] }
end
Methods.GetBoneMass =function(self, b) return (rawget(self, "mass") or {})[b] or 10.0 end
Methods.AddImpulseAtLocation = function(self, imp, at, b)
    table.insert(M.impulses, { mesh = rawget(self, "__name"), imp = { imp.X, imp.Y, imp.Z }, at = { at.X, at.Y, at.Z }, bone = b })
end
Methods.AddComponentByClass = function(self, cls)
    local h = new_obj("PhysicsHandleComponent", "PhysicsHandle_" .. tostring(#M.handles + 1))
    rawset(h, "owner", self)
    table.insert(M.handles, h)
    return h
end
-- PhysicsHandleComponent
Methods.GrabComponentAtLocation = function(self, mesh, b, at)
    if not bone_of(mesh, b) then return end
    self.GrabbedComponent = mesh
    rawset(self, "bone", b)
    rawset(self, "grab_at", { at.X, at.Y, at.Z })
    table.insert(M.grabs, { h = rawget(self, "__name"), mesh = rawget(mesh, "__name"), bone = b, at = { at.X, at.Y, at.Z } })
end
Methods.SetTargetLocation = function(self, t)
    rawset(self, "target", { t.X, t.Y, t.Z })
    rawset(self, "targets", (rawget(self, "targets") or 0) + 1)
end
Methods.ReleaseComponent = function(self) self.GrabbedComponent = nil; rawset(self, "released", (rawget(self, "released") or 0) + 1) end
Methods.SetLinearStiffness = function(self, k) rawset(self, "lin_k", k) end
Methods.SetLinearDamping = function(self, d) rawset(self, "lin_d", d) end
Methods.SetAngularStiffness = function(self, k) rawset(self, "ang_k", k) end
Methods.SetAngularDamping = function(self, d) rawset(self, "ang_d", d) end

-- A Willie: actor + simulated mesh (`field`) with the 16 hero bones laid out
-- around `root` facing +x (or -x when `facing` = -1).
local HERO_OFFS = {
    Pelvis = { 0, 0, 0 }, Spine_02 = { 0, 0, 25 }, Spine_04 = { 0, 0, 50 }, Head = { 0, 0, 75 },
    Upperarm_L = { 0, 20, 48 }, Lowerarm_L = { 20, 25, 40 }, Hand_L = { 40, 20, 40 },
    Upperarm_R = { 0, -20, 48 }, Lowerarm_R = { 25, -25, 40 }, Hand_R = { 45, -20, 40 },
    Thigh_L = { 0, 10, -10 }, Calf_L = { 0, 10, -50 }, Foot_L = { 0, 10, -90 },
    Thigh_R = { 0, -10, -10 }, Calf_R = { 0, -10, -50 }, Foot_R = { 0, -10, -90 },
}
function M.willie(name, root, facing, field)
    local w = new_obj("Willie_BP_C", name)
    local m = new_obj("SkeletalMeshComponent", name .. "_" .. (field or "Mesh"))
    rawset(m, "owner", w)
    rawset(m, "sim", true)
    local bones = {}
    for b, o in pairs(HERO_OFFS) do
        bones[b] = { root[1] + (facing or 1) * o[1], root[2] + (facing or 1) * o[2], root[3] + o[3] }
    end
    rawset(m, "bones", bones)
    w[field or "Mesh"] = m
    rawset(w, "mesh", m)
    table.insert(M.willies, w)
    return w, m
end
function M.move_bone(mesh, b, p) rawget(mesh, "bones")[b] = p end

function M.new_world(name)
    M.world = new_obj("World", "world")
    rawset(M.world, "fullname", "World /Game/Maps/Arenas/" .. name .. "." .. name)
    M.pc = new_obj("PC", "PC_" .. tostring(M.addr))
    M.willies = {}
end
M.new_world("Map_Arena_Pit")

package.preload["UEHelpers"] = function()
    return { GetPlayerController = function() return M.pc end, GetWorld = function() return M.world end }
end

M.ph_class = new_obj("Class", "PhysicsHandleComponent")
rawset(M.ph_class, "__persist", true)
function StaticFindObject(path)
    if path == "/Script/Engine.PhysicsHandleComponent" then return M.ph_class end
    return nil
end
function FindAllOf(c)
    if c == "Willie_BP_C" then
        local out = {}
        for _, w in ipairs(M.willies) do if not rawget(w, "__dead") then out[#out + 1] = w end end
        return #out > 0 and out or nil
    end
    return nil
end

-- Scheduling on a fake clock (ms). Raw LoopAsync / ExecuteWithDelay raise:
-- the mod must route them through its game-thread shim.
function LoopInGameThreadWithDelay(ms, fn) M.seq = M.seq + 1; M.loops[M.seq] = { ms = ms, fn = fn, due = M.now + ms }; return M.seq end
function ExecuteInGameThreadWithDelay(ms, fn) M.seq = M.seq + 1; M.delayed[M.seq] = { fn = fn, due = M.now + ms }; return M.seq end
function CancelDelayedAction(h) M.loops[h] = nil; M.delayed[h] = nil end
function ExecuteInGameThread(fn) fn() end
function LoopInGameThreadAfterFrames(n, fn) table.insert(M.frames, fn) end
function LoopAsync() error("raw LoopAsync must be shimmed") end
function ExecuteWithDelay() error("raw ExecuteWithDelay must be shimmed") end
function RegisterKeyBind() end
function RegisterKeyBindAsync() end
function RegisterLoadMapPreHook(fn) M.premap = fn end
function RegisterHook(path, fn)
    M.hooks[path] = M.hooks[path] or {}
    table.insert(M.hooks[path], fn)
end
function M.fire(path, ...)
    for _, fn in ipairs(M.hooks[path] or {}) do fn(...) end
end
-- A hook parameter as UE4SS passes it (RemoteUnrealParam with :get()).
function M.param(v) return { get = function() return v end } end

print = function(s) table.insert(M.logs, s) end
os.clock = function() return M.now / 1000 end
local _getenv = os.getenv
os.getenv = function(k) if M.env[k] ~= nil then return M.env[k] end return _getenv(k) end

M.frame_n = 0
function M.run(ms)
    local target = M.now + ms
    while M.now < target do
        M.now = M.now + 1
        if M.now % M.frame_ms == 0 then
            M.frame_n = M.frame_n + 1
            for _, fn in ipairs(M.frames) do
                local ok, e = pcall(fn); if not ok then table.insert(M.logs, "FRAME ERR " .. tostring(e)) end
            end
        end
        local due = {}
        for h, l in pairs(M.loops) do if l.due <= M.now then due[#due + 1] = h end end
        table.sort(due)
        for _, h in ipairs(due) do
            local l = M.loops[h]
            if l then l.due = M.now + l.ms; local ok, e = pcall(l.fn); if not ok then table.insert(M.logs, "LOOP ERR " .. tostring(e)) end end
        end
        due = {}
        for h, d in pairs(M.delayed) do if d.due <= M.now then due[#due + 1] = h end end
        table.sort(due)
        for _, h in ipairs(due) do
            local d = M.delayed[h]
            if d then M.delayed[h] = nil; local ok, e = pcall(d.fn); if not ok then table.insert(M.logs, "DELAY ERR " .. tostring(e)) end end
        end
    end
end

function M.kill_all()   -- the old world is freed: any later touch is recorded
    for _, o in ipairs(M.objs) do
        if not rawget(o, "__persist") and rawget(o, "__cls") ~= "Class" then rawset(o, "__dead", true) end
    end
end

function M.logtext() return table.concat(M.logs, "\n") end
