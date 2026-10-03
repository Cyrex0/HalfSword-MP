-- Minimal UE4SS 3.0.1 / UE 5.4 mock for HSMPWorld tests (see src/lib.rs).
--
-- Faithful to the game where it matters:
--   * UFunction calls are arity-checked like UE4SS ("UFunction expected N
--     parameters, received M"): GameplayStatics::FinishSpawningActor has 3.
--   * BeginDeferredActorSpawnFromClass returns an actor that is NOT built
--     yet (components unregistered, at the origin) until FinishSpawningActor.
--   * K2_DestroyActor makes the actor pending-kill (IsValid false); any later
--     call or property access on it is recorded as a violation (in the game
--     it is a use-after-free once GC runs).
--   * Willies are pooled: K2_DestroyActor on one is a no-op that snaps it to
--     the origin; doing it at all is recorded as a violation.

local M = {
    logs = {}, violations = {}, spawned = {}, events = {},
    classes = {}, loadable = {}, next_addr = 0x10000,
    finish_fails = false,
}

local function violation(fmt, ...)
    M.violations[#M.violations + 1] = string.format(fmt, ...)
end

local function ev(...)
    M.events[#M.events + 1] = table.concat({ ... }, " ")
end

-- name -> { n = arity or nil (not a UFunction), fn = impl }
local function make_obj(kind, name, methods, props)
    M.next_addr = M.next_addr + 0x10
    local o = { _kind = kind, _name = name, _valid = true, _addr = M.next_addr, _props = props or {} }
    local meta = {}
    meta.__index = function(self, k)
        local m = methods[k]
        if m then
            return function(me, ...)
                if me ~= self then error("method " .. k .. " called without ':'") end
                if not rawget(self, "_valid") and k ~= "IsValid" then
                    violation("call %s on invalid %s %s", k, kind, name)
                end
                local argc = select("#", ...)
                if m.n and argc ~= m.n then
                    error(string.format("[UFunction::setup_metamethods -> __call] UFunction expected %d parameters, received %d", m.n, argc))
                end
                return m.fn(self, ...)
            end
        end
        if not rawget(self, "_valid") then violation("read %s on invalid %s %s", tostring(k), kind, name) end
        return rawget(self, "_props")[k]
    end
    meta.__newindex = function(self, k, v)
        if not rawget(self, "_valid") then violation("write %s on invalid %s %s", tostring(k), kind, name) end
        rawget(self, "_props")[k] = v
        ev("set", name, tostring(k))
    end
    return setmetatable(o, meta)
end

local function common(extra)
    local t = {
        IsValid = { fn = function(self)
            -- IsValid on a GARBAGE-COLLECTED object reads freed memory in the game
            -- (an access violation pcall cannot catch): a violation here.
            if rawget(self, "_freed") then violation("IsValid on freed %s %s", rawget(self, "_kind"), rawget(self, "_name")) end
            return rawget(self, "_valid")
        end },
        GetAddress = { fn = function(self) return rawget(self, "_addr") end },
        GetFName = { fn = function(self) local n = rawget(self, "_name"); return { ToString = function() return n end } end },
        GetFullName = { fn = function(self) return (rawget(self, "_fullname") or (rawget(self, "_kind") .. " " .. rawget(self, "_name"))) end },
        HasAnyFlags = { fn = function() return false end },
    }
    for k, v in pairs(extra or {}) do t[k] = v end
    return t
end

function M.FName(s) return { ToString = function() return s end, _fname = s } end

-- ---- classes ------------------------------------------------------------------
M.weapon_base = make_obj("Class", "ModularWeaponBP_C", common({}))
M.classes["/Game/Assets/Weapons/Blueprints/ModularWeaponBP.ModularWeaponBP_C"] = M.weapon_base

function M.make_class(path, is_weapon)
    local short = path:match("%.([^%.]+)$")
    local cdo = make_obj("Object", "Default__" .. short, common({
        IsA = { fn = function(_, c) return is_weapon and c == M.weapon_base end },
    }))
    local cls = make_obj("Class", short, common({
        GetCDO = { fn = function() return cdo end },
    }))
    rawset(cls, "_fullname", "BlueprintGeneratedClass " .. path)
    rawset(cls, "_path", path)
    return cls
end

-- ---- components / actors ------------------------------------------------------------
local function v3(x, y, z) return { X = x, Y = y, Z = z } end

local function make_component(owner_name, cname)
    local c
    c = make_obj("StaticMeshComponent", owner_name .. "." .. cname, common({
        IsSimulatingPhysics = { n = 1, fn = function(self) return rawget(self, "_sim") end },
        SetSimulatePhysics = { n = 1, fn = function(self, on)
            rawset(self, "_sim", on); ev("sim", rawget(self, "_name"), tostring(on)) end },
        K2_GetComponentLocation = { n = 0, fn = function(self) local p = rawget(self, "_pos"); return v3(p.X, p.Y, p.Z) end },
        K2_GetComponentRotation = { n = 0, fn = function(self) local r = rawget(self, "_rot"); return { Pitch = r.Pitch, Yaw = r.Yaw, Roll = r.Roll } end },
        K2_SetWorldLocationAndRotation = { n = 4 + 1, fn = function(self, pos, rot, sweep, hit, teleport)
            if sweep ~= false or teleport ~= true then violation("teleport without bTeleport / with sweep") end
            rawset(self, "_pos", v3(pos.X, pos.Y, pos.Z))
            rawset(self, "_rot", { Pitch = rot.Pitch, Yaw = rot.Yaw, Roll = rot.Roll })
            ev("tp", rawget(self, "_name"), string.format("%.0f,%.0f,%.0f", pos.X, pos.Y, pos.Z),
               rawget(self, "_sim") and "sim" or "kin")
        end },
        SetPhysicsLinearVelocity = { n = 3, fn = function(self, v)
            if not rawget(self, "_sim") then violation("set velocity on a kinematic body") end
            rawset(self, "_vel", v3(v.X, v.Y, v.Z)) end },
        SetPhysicsAngularVelocityInDegrees = { n = 3, fn = function() end },
        PutRigidBodyToSleep = { n = 1, fn = function(self) ev("sleep", rawget(self, "_name")) end },
        GetPhysicsLinearVelocity = { n = 1, fn = function(self) local v = rawget(self, "_vel") or v3(0, 0, 0); return v3(v.X, v.Y, v.Z) end },
    }))
    rawset(c, "_pos", v3(0, 0, 0))
    rawset(c, "_rot", { Pitch = 0, Yaw = 0, Roll = 0 })
    rawset(c, "_sim", false)
    return c
end

local level = make_obj("Level", "PersistentLevel", common({
    GetClass = { fn = function() return { GetFName = function() return { ToString = function() return "Level" end } end } end },
}))
rawset(level, "_fullname", "Level /Game/Maps/Arenas/Map_Arena_Alley.Map_Arena_Alley:PersistentLevel")

local function actor_methods(extra)
    local t = common({
        GetClass = { fn = function(self) return rawget(self, "_class") end },
        GetOuter = { fn = function() return level end },
        K2_GetActorLocation = { n = 0, fn = function(self) local b = rawget(self, "_props").BaseMesh; return b and b:K2_GetComponentLocation() or v3(0, 0, 0) end },
        K2_GetRootComponent = { n = 0, fn = function(self) return rawget(self, "_props").BaseMesh end },
        GetAttachParentActor = { n = 0, fn = function(self) return rawget(self, "_attached") end },
        SetActorHiddenInGame = { n = 1, fn = function(self, b) rawset(self, "_hidden", b); ev("hide", rawget(self, "_name")) end },
        SetActorEnableCollision = { n = 1, fn = function(self, b) rawset(self, "_collision", b); ev("nocollide", rawget(self, "_name")) end },
        SetActorTickEnabled = { n = 1, fn = function(self, b) rawset(self, "_tick", b) end },
        K2_DestroyActor = { n = 0, fn = function(self)
            local cls = rawget(self, "_class")
            if cls and rawget(cls, "_name"):find("Willie", 1, true) then
                violation("K2_DestroyActor on pooled Willie %s", rawget(self, "_name"))
                return
            end
            ev("destroy", rawget(self, "_name"))
            M.kill(self)
        end },
    })
    for k, v in pairs(extra or {}) do t[k] = v end
    return t
end

-- Pending kill: actor and its components become invalid.
function M.kill(a)
    rawset(a, "_valid", false)
    for _, v in pairs(rawget(a, "_props")) do
        if type(v) == "table" and rawget(v, "_kind") == "StaticMeshComponent" then rawset(v, "_valid", false) end
    end
end

-- Destroyed AND garbage-collected: any touch, IsValid included, is a crash in game.
function M.free(a)
    M.kill(a)
    rawset(a, "_freed", true)
    for _, v in pairs(rawget(a, "_props")) do
        if type(v) == "table" and rawget(v, "_kind") == "StaticMeshComponent" then rawset(v, "_freed", true) end
    end
end

function M.passport(seed)
    return {
        WeaponClass_54_B478ECF7499977809745A3973AD678EC = M.weapon_base,
        ID_70_C02CF656483647A1933EEA96314B78A6 = seed,
        Name_57_3729B51148E846FE8DD336B9419BCEE1 = M.FName("Blade" .. seed),
        HeadSubModule1_7_ABBFD017411F42A4950B1C9F2360A30D = M.weapon_base,
        HeadSubModule2_9_90AAA8304C7794E1BF814C9354A1A7E9 = M.weapon_base,
        HeadModule_11_62DF53134688807E1DA7F4A20E9F7139 = M.weapon_base,
        GuardModule_13_6DD2B06245505E53B529D090333012F0 = M.weapon_base,
        PommelModule_15_561B01324BFCD4360DAE9A95299BB9D6 = M.weapon_base,
        GripModule_18_F4DF51EB4E742195B8C6BAB17E4C5DB4 = M.weapon_base,
        HeadSize_21_2D425E61473B8F64FBAB51B223459D57 = v3(seed, 1, 1),
        GuardSize_23_5A1AA0E04708E86FEFF61E974DDA8704 = v3(1, 1, 1),
        GripSize_25_AC1660814C4C25C521AAA8830FE8ECCF = v3(1, 1, 1),
        PommelSize_27_660CC00C49C26D503E16B2BC58CE115E = v3(1, 1, 1),
        CustomMassScaleHead_30_B95872A242AD944E2CE4D493F718F9D7 = 1,
        CustomMassScaleGuard_51_3A9024E74306B7BB5D186087011D1927 = 1,
        CustomMassScaleGrip_32_0EAADEE0419C05C6DB38F0AE134A9B10 = 1,
        CustomMassScalePommel_34_0AB28D814BDEF17D408D0DAA3A453173 = 1,
        MaterialMetalSteel_37_AB7A28C94B176CF81A6C8BA34AC57C36 = 2,
        MaterialMetalColored_39_DC2EAC244758A8D82855CC940784A1D2 = 0,
        MaterialWeood_41_E0B3C8DB48943B878AEFA3AB01E7B99A = 1,
        MaterialLeather_43_41D1114148FDB4FE4DACC8A2F4CA9FEB = 3,
        ColorWood_46_F3AE05AD4495EBCD1D354C8025D7C743 = { R = 0.5, G = 0.25, B = 0, A = 1 },
        ColorLeather_48_DC45F07E4C0C3280278212A7158EE638 = { R = 0, G = 0, B = 0, A = 1 },
        Price_60_83FE5A624EA188485BBE4E9C8606AEE5 = 100 + seed,
        Tier_67_05026E6F43B7300AA8BACC9D9F9AB461 = 3,
    }
end

-- A finished (constructed) weapon actor at pos.
function M.make_weapon(cls, name, pos, sim)
    local props = { ["Is Held"] = false, ["Simulates Physics"] = sim and true or false,
                    ["Weapon Passport"] = M.passport(1) }
    local a = make_obj("Actor", name, actor_methods(), props)
    rawset(a, "_class", cls)
    local b = make_component(name, "BaseMesh")
    rawset(b, "_pos", v3(pos.X, pos.Y, pos.Z))
    rawset(b, "_sim", sim and true or false)
    props.BaseMesh = b
    rawset(a, "_finished", true)
    return a
end

function M.make_willie(name)
    local cls = make_obj("Class", "Willie_BP_C", common({}))
    local w = make_obj("Actor", name, actor_methods(), {})
    rawset(w, "_class", cls)
    local b = make_component(name, "Mesh")
    w._props.BaseMesh = b
    return w
end

-- ---- GameplayStatics -----------------------------------------------------------------
M.world = make_obj("World", "Map_Arena_Alley", common({}))
rawset(M.world, "_fullname", "World /Game/Maps/Arenas/Map_Arena_Alley.Map_Arena_Alley")

M.gs = make_obj("GameplayStatics", "Default__GameplayStatics", common({
    -- UE 5.4: (WorldContextObject, ActorClass, SpawnTransform,
    --          CollisionHandlingOverride, Owner, TransformScaleMethod)
    BeginDeferredActorSpawnFromClass = { n = 6, fn = function(_, world, cls, t, coll, owner, scale)
        if world ~= M.world then error("bad world context") end
        if type(t) ~= "table" or not t.Translation or not t.Rotation or not t.Scale3D then error("bad FTransform") end
        if t.Rotation.W == nil then error("FTransform.Rotation must be an FQuat {X,Y,Z,W}") end
        local short = rawget(cls, "_name")
        local name = short .. "_" .. tostring(2147480000 + #M.spawned)
        local a = make_obj("Actor", name, actor_methods(), { ["Is Held"] = false, ["Simulates Physics"] = false,
                                                              ["Weapon Passport"] = M.passport(0) })
        rawset(a, "_class", cls)
        rawset(a, "_deferred", true)
        rawset(a, "_begin_scale", scale)
        a._props.BaseMesh = make_component(name, "BaseMesh")   -- unregistered: at the origin, no physics
        M.spawned[#M.spawned + 1] = a
        ev("begin", name)
        return a
    end },
    -- UE 5.4: (Actor, SpawnTransform, TransformScaleMethod)
    FinishSpawningActor = { n = 3, fn = function(_, a, t, scale)
        if not rawget(a, "_deferred") then error("FinishSpawningActor on a non-deferred actor") end
        if M.finish_fails then error("construction script failed") end
        rawset(a, "_deferred", false)
        rawset(a, "_finished", true)
        rawset(a, "_finish_scale", scale)
        rawset(a, "_finish_t", t)
        local p = rawget(a, "_props")
        rawset(a, "_passport_at_finish", p["Weapon Passport"])
        rawset(a, "_simphys_at_finish", p["Simulates Physics"])
        local b = p.BaseMesh
        rawset(b, "_pos", v3(t.Translation.X, t.Translation.Y, t.Translation.Z))
        rawset(b, "_sim", p["Simulates Physics"] == true)   -- BeginPlay applies "Simulates Physics"
        ev("finish", rawget(a, "_name"))
        return a
    end },
}))

M.pc = make_obj("PlayerController", "PC_0", common({ GetWorld = { fn = function() return M.world end } }), { Pawn = nil })

function M.install(G)
    G.FName = M.FName
    G.StaticFindObject = function(path) return M.classes[path] end
    G.LoadAsset = function(pkg)
        for path, cls in pairs(M.loadable) do
            if path:gsub("%.[^%./]+$", "") == pkg then M.classes[path] = cls end
        end
    end
    G.FindAllOf = function() return {} end
    G.print = function(s) M.logs[#M.logs + 1] = s end
    package.preload["UEHelpers"] = function()
        return {
            GetWorld = function() return M.world end,
            GetGameplayStatics = function() return M.gs end,
            GetPlayerController = function() return M.pc end,
        }
    end
end

function M.log_has(pat)
    for _, l in ipairs(M.logs) do if l:find(pat) then return true end end
    return false
end

function M.events_of(name)
    local out = {}
    for _, e in ipairs(M.events) do if e:find(name, 1, true) then out[#out + 1] = e end end
    return out
end

return M
