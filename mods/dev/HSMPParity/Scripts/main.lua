-- HSMPParity (dev only): in-game damage parity experiments, driven from the
-- DevCtl ring with no OS input:
--
--     hsmp-tools ipc-ctl --pid <game> autotest parity "<experiment> [args]"
--
-- Experiments (results: [HSMPParity] lines in UE4SS.log and
-- <state_dir>/.parity_results.txt):
--   kit            my pawn's and every stand-in's armour as Deal Complex Damage
--                  sees it: equipped classes, worn meshes, proxy collision tags
--                  (defB / defC / defS / dens), bone mass scales, Team Int,
--                  passport height / weight, Invulnerable.
--                  For the separate Avatars dev command `autotest bodyheight
--                  "<remote peer> <owner passport height>"`, capture kit before,
--                  during its 2-second window and after automatic restoration.
--                  This is a geometry experiment, not a production height fix.
--   spots [deg]    the same blow on my own pawn three ways, per bone and spot:
--                  solo (the spot itself), "world" (the spot a world-space offset
--                  lands on when the victim turned `deg`, default 70, since the
--                  attacker saw it: HSMPCombat before the bone-space fix) and "local" (the
--                  bone-space offset carried over: HSMPCombat now). Every field
--                  is put back after each blow.
--   near <peer>    stand 85 uu in front of peer's stand-in, facing it.
--   swing <speed>  push my held weapon at the nearest stand-in's chest at
--                  <speed> uu/s for three frames. Constraints can prevent
--                  movement; delivery alone is not evidence of a hit.
--   arm <r|l> <s>  hold the actual arm input axis for up to 2 seconds. It
--                  overrides that pawn's native input callback, then releases
--                  back to normal controls. Contacts must be verified in DCD.
--
-- Run only inside an MP session: there the career save is redirected
-- (shared/hsmp_saveguard.lua). The experiments refuse to run otherwise.

local UEHelpers = require("UEHelpers")

if LoopInGameThreadWithDelay and ExecuteInGameThreadWithDelay and CancelDelayedAction then
    ExecuteWithDelay = function(ms, fn) return ExecuteInGameThreadWithDelay(ms, fn) end
    LoopAsync = function(ms, fn)
        local h
        h = LoopInGameThreadWithDelay(ms, function()
            local ok, stop = pcall(fn)
            if ok and stop == true and h then CancelDelayedAction(h) end
        end)
        return h
    end
end

local function trim(s) return s and s:match("^%s*(.-)%s*$") or nil end
local STATE_DIR = (trim(os.getenv("HSMP_STATE_DIR")) or "hsmp_state"):gsub("\\", "/")
local function Log(fmt, ...)
    local s = string.format(fmt, ...)
    print("[HSMPParity] " .. s .. "\n")
    local f = io.open(STATE_DIR .. "/.parity_results.txt", "ab")
    if f then f:write(os.date("!%H:%M:%S "), s, "\n"); f:close() end
end

local function load_module(name)
    local ok, mod = pcall(require, name)
    if ok and type(mod) == "table" then return mod end
    local src = ((debug and debug.getinfo and debug.getinfo(1, "S").source) or ""):gsub("^@", "")
    local dir = src:match("^(.*)[/\\]") or "."
    for _, path in ipairs({ dir .. "/" .. name .. ".lua", dir .. "/../../../shared/" .. name .. ".lua" }) do
        local ok2, mod2 = pcall(dofile, path)
        if ok2 and type(mod2) == "table" then return mod2 end
    end
    return nil
end
local HW = load_module("hsmp_wg")
if not HW then Log("FATAL: shared/hsmp_wg.lua missing"); return end
local WG = HW.new({ log = Log, UEHelpers = UEHelpers })
do
    local m = load_module("hsmp_ipc")
    if m then m.init({ mod = "HSMPParity", state_dir = STATE_DIR, log = Log }) end
end
local HSESS = load_module("hsmp_session")
local SESS = HSESS and HSESS.new({})

-- ---- helpers ---------------------------------------------------------------------------
local function valid(o) local ok, r = pcall(function() return o and o:IsValid() end); return ok and r == true end
local function nm(o) if not valid(o) then return "<invalid>" end; local s; pcall(function() s = o:GetFName():ToString() end); return s or "?" end
local function num(x) return tonumber(x) or 0 end
local function vec(v) local t; pcall(function() t = { v.X, v.Y, v.Z } end); return t or { 0, 0, 0 } end
local function V(t) return { X = t[1], Y = t[2], Z = t[3] } end
local function f3(t) return string.format("(%.1f,%.1f,%.1f)", t[1], t[2], t[3]) end
local function me_pawn()
    local pc = WG.pc()
    if not valid(pc) then return nil end
    local p = WG.ai_pawn and WG.ai_pawn() or pc.Pawn
    if valid(p) then return p end
    return WG.ai_pawn()   -- `ai on`: the game's own AI drives our pawn
end
local function bp_call(obj, name, ...)
    local fn
    pcall(function() fn = obj[name] end)
    if fn == nil then return false, "unresolved" end
    local args = table.pack(...)
    return pcall(function() fn(obj, table.unpack(args, 1, args.n)) end)
end

-- Bone frames (the same math as HSMPCombat BF).
local BF = {}
function BF.of(mesh, bone)
    local f
    pcall(function()
        local t = mesh:GetSocketTransform(FName(bone), 0)
        local p, q = t.Translation, t.Rotation
        local s = 1
        pcall(function() s = tonumber(t.Scale3D.X) or 1 end)
        local n = math.sqrt(q.X * q.X + q.Y * q.Y + q.Z * q.Z + q.W * q.W)
        f = { p = { p.X, p.Y, p.Z }, q = { q.X / n, q.Y / n, q.Z / n, q.W / n }, s = s }
    end)
    return f
end
function BF.rot(q, v, inv)
    local qx, qy, qz, qw = q[1], q[2], q[3], q[4]
    if inv then qx, qy, qz = -qx, -qy, -qz end
    local tx, ty, tz = 2 * (qy * v[3] - qz * v[2]), 2 * (qz * v[1] - qx * v[3]), 2 * (qx * v[2] - qy * v[1])
    return { v[1] + qw * tx + (qy * tz - qz * ty), v[2] + qw * ty + (qz * tx - qx * tz), v[3] + qw * tz + (qx * ty - qy * tx) }
end
local function zrot(deg, v)
    local a = math.rad(deg)
    local c, s = math.cos(a), math.sin(a)
    return { c * v[1] - s * v[2], s * v[1] + c * v[2], v[3] }
end

-- Every field a blow changes that this harness puts back.
local FIELDS = {
    "Health", "Head Health", "Neck Health", "Body Upper Health", "Body Lower Health", "Back Health",
    "Arm_R Health", "Arm_L Health", "Leg_R Health", "Leg_L Health", "Consciousness", "Consciousness Cap",
    "Head Health (Crush)", "Bleeding", "Pain", "Blood Rate", "Sustained Damage", "Damage Taken",
    "Last Damage Taken", "Consciousness 2 (Legs)", "Stamina", "Exhaustion", "Last Complex Damage Impulse",
    "Was Just Touched", "All Body Tonus", "Head Tonus", "Arm R Tonus", "Arm L Tonus", "Upper Body Tonus",
    "Leg R Tonus", "Leg L Tonus", "Get Up Rate", "Angelic Touch", "Getup Animation State", "Camera Shake",
    "Force Death", "Current Pain Threshold", "Fear", "Pain Shock Rate", "Flinch Index", "Pain Head",
    "Pain Neck", "Pain Arm R", "Pain Arm L", "Pain Upper Body", "Pain Lower Body", "Pain Leg R", "Pain Leg L",
    "Ball Pain", "Liver Pain",
}
local REPORT = { "Health", "Consciousness", "Bleeding", "Pain", "Head Health", "Neck Health", "Body Upper Health",
                 "Body Lower Health", "Arm_R Health", "Leg_L Health", "Head Health (Crush)" }
local function snap(w)
    local s = {}
    for _, k in ipairs(FIELDS) do pcall(function() s[k] = w[k] end) end
    pcall(function() s.ldb = w["Last Damaged Bone"]:ToString() end)
    pcall(function() s.lcb = w["Last Complex Damage Bone"]:ToString() end)
    pcall(function() local v = w["Pain Stumble Immediate"]; s.stumble = { v.X, v.Y, v.Z } end)
    return s
end
local function put(w, s)
    for _, k in ipairs(FIELDS) do if s[k] ~= nil then pcall(function() w[k] = s[k] end) end end
    if s.ldb then pcall(function() w["Last Damaged Bone"] = FName(s.ldb) end) end
    if s.lcb then pcall(function() w["Last Complex Damage Bone"] = FName(s.lcb) end) end
    if s.stumble then pcall(function() w["Pain Stumble Immediate"] = V(s.stumble) end) end
end
local function dfields(a, b)
    local out = {}
    for _, k in ipairs(REPORT) do
        local x, y = tonumber(a[k]), tonumber(b[k])
        if x and y and math.abs(y - x) > 0.005 then out[#out + 1] = string.format("%s%+.2f", k, y - x) end
    end
    return out
end

-- ---- kit ---------------------------------------------------------------------------------
local MASS_BONES = { "pelvis", "spine_03", "spine_05", "head", "upperarm_r", "lowerarm_r", "hand_r", "thigh_l", "calf_l" }
local function kit_of(label, w)
    local parts = {}
    local function add(k, v) parts[#parts + 1] = k .. "=" .. tostring(v) end
    local function precise3(v) return string.format("(%.4f,%.4f,%.4f)",v.X,v.Y,v.Z) end
    pcall(function() add("team", w["Team Int"]) end)
    pcall(function() add("invuln", w.Invulnerable) end)
    pcall(function() add("mass_scale_bp", string.format("%.3f", num(w["Mass Scale (Set in BP)"]))) end)
    pcall(function() add("armour_weight", string.format("%.2f", num(w["Armor Weight Body"]))) end)
    for _, k in ipairs({"All Weapons Weights","Armor Weight Head","Armor Weight Arm R","Armor Weight Arm L","Armor Weight Legs",
        "Weapons on Waist Weight","Shield On Back Weight","Shield On Shoulder Weight"}) do
        pcall(function() add(k:gsub("%s",""),string.format("%.4f",num(w[k]))) end)
    end
    pcall(function() add("actor_scale",precise3(w:GetActorScale3D())) end)
    pcall(function() add("mesh_relative_scale",precise3(w.Mesh.RelativeScale3D)) end)
    pcall(function() add("passport_height",w["Character Passport"]["Height_21_0EB204DF4978B92AD0ED188FD32EEC7B"]) end)
    pcall(function() add("passport_weight",w["Character Passport"]["Weight_23_65E4C6534D14653F96EB739F159E58CD"]) end)
    pcall(function()
        w["Currently Equipped Armor"]:ForEach(function(k,v)
            local pass = v:get()
            local row = {"slot=" .. tostring(k:get())}
            local fields = {
                {"class","ArmorCore_3_F6B7C69C4BD7D9720DB91EB635EE2B43"},
                {"removed","CoreRemoved_12_5CFF8F6D4A05C15812594CAF6771C66B"},
                {"module1","Module1_5_46B7198E4341C93CBF6AE989EF9898E4"},
                {"module2","Module2_7_5B7940B84CFD673B25103D96E0AFEEB0"},
                {"module3","Module3_9_E282C465414F6D4EF2A8039FBA847AD2"},
                {"steel","SteelType_84_7BA6626740476C2CD69648847A1E592F"},
                {"metal","MetalPiecesType_81_203BFD454D41FA24B0B5C5838898AA60"},
            }
            for _,f in ipairs(fields) do pcall(function()
                local x = pass[f[2]]
                if f[1] == "class" and valid(x) then x = x:GetFullName() end
                row[#row+1] = f[1] .. "=" .. tostring(x)
            end) end
            Log("KIT_ARMOUR %s %s",label,table.concat(row," "))
        end)
    end)
    for _, k in ipairs({ "Height Rate", "Muscle Rate", "Is Zombie?", "Fallen" }) do
        local x; pcall(function() x = w[k] end)
        add(k:gsub("[^%w]", ""), x == nil and "?" or tostring(x))
    end
    local eq = {}
    pcall(function()
        w["Currently Equipped Armor"]:ForEach(function(k) eq[#eq + 1] = tostring(k:get()) end)
    end)
    table.sort(eq)
    add("equipped", #eq .. "[" .. table.concat(eq, " ") .. "]")
    local worn = 0
    pcall(function() worn = #w["Worn Armor"] end)
    add("worn", worn)
    local tags = {}
    pcall(function()
        w["Armor Collision Meshes"]:ForEach(function(_, e)
            local c = e:get()
            if not valid(c) then return end
            local t = {}
            pcall(function() c.ComponentTags:ForEach(function(_, g) t[#t + 1] = g:get():ToString() end) end)
            local keep = {}
            for _, g in ipairs(t) do if g:find("^def") or g:find("^dens") or g:find("_") then keep[#keep + 1] = g end end
            table.sort(keep)
            tags[#tags + 1] = table.concat(keep, ",")
            table.sort(t)
            pcall(function()
                local a = c:GetAttachSocketName():ToString()
                local tr = c:GetSocketTransform(FName("None"),0)
                local asset = "?"
                pcall(function() if valid(c.StaticMesh) then asset=c.StaticMesh:GetFullName() end end)
                pcall(function() if valid(c.SkeletalMesh) then asset=c.SkeletalMesh:GetFullName() end end)
                Log("KIT_PROXY %s bone=%s asset=%s p=%s scale=%s tags=[%s]",label,a,asset,
                    precise3(tr.Translation),precise3(tr.Scale3D),table.concat(t,","))
            end)
        end)
    end)
    table.sort(tags)
    add("proxies", #tags .. "{" .. table.concat(tags, " | ") .. "}")
    local ms = {}
    pcall(function()
        for _, b in ipairs(MASS_BONES) do
            local m = w.Mesh:GetMassScale(FName(b))
            local bm = w.Mesh:GetBoneMass(FName(b), false)
            local scaled = w.Mesh:GetBoneMass(FName(b), true)
            ms[#ms + 1] = string.format("%s=%.3f/%.2fkg/scaled=%.2fkg", b, num(m), num(bm), num(scaled))
        end
    end)
    add("mass", table.concat(ms, " "))
    pcall(function() add("mesh_mass", string.format("%.2fkg", num(w.Mesh:GetMass()))) end)
    pcall(function()
        local s = w.Mesh:K2_GetComponentScale()
        add("mesh_scale", string.format("%.3f,%.3f,%.3f", num(s.X), num(s.Y), num(s.Z)))
    end)
    local neck = {}
    for _, pair in ipairs({ { "spine_05", "neck_01" }, { "neck_01", "neck_02" }, { "neck_02", "head" } }) do
        pcall(function()
            local a, b = w.Mesh:GetSocketLocation(FName(pair[1])), w.Mesh:GetSocketLocation(FName(pair[2]))
            neck[#neck + 1] = string.format("%s=%.2fcm", pair[2], math.sqrt((a.X-b.X)^2 + (a.Y-b.Y)^2 + (a.Z-b.Z)^2))
            local q = w.Mesh:GetSocketTransform(FName(pair[1]),0).Rotation
            local x,y,z = b.X-a.X,b.Y-a.Y,b.Z-a.Z
            local qx,qy,qz,qw = -q.X,-q.Y,-q.Z,q.W
            local tx,ty,tz = 2*(qy*z-qz*y),2*(qz*x-qx*z),2*(qx*y-qy*x)
            Log("KIT_NECK %s parent=%s child=%s parent_frame_offset=(%.4f,%.4f,%.4f) world_a=%s world_b=%s",
                label,pair[1],pair[2],x+qw*tx+qy*tz-qz*ty,y+qw*ty+qz*tx-qx*tz,z+qw*tz+qx*ty-qy*tx,
                f3(vec(a)),f3(vec(b)))
        end)
    end
    add("neck_joints", table.concat(neck, " "))
    local injury = {}
    for _, key in ipairs({ "Health", "Consciousness", "Consciousness Cap", "Consciousness 2 (Legs)", "Bleeding", "Pain", "Fallen", "Head Health", "Head Health (Crush)", "Neck Health", "Back Health", "Arm_R Health", "Arm_L Health",
                           "Leg_R Health", "Leg_L Health", "Headless", "Neck Dislocated", "Spine Dislocated",
                           "Neck Snapped", "Back Broken", "Current Game Mode Enum", "Force Disable Dismemberment", "Invulnerable", "DED", "Force Death" }) do
        pcall(function() injury[#injury + 1] = key:gsub("%s", "") .. ":" .. tostring(w[key]) end)
    end
    pcall(function()
        w["Dismembered Array"]:ForEach(function(_, e) injury[#injury + 1] = "missing:" .. e:get():ToString() end)
    end)
    add("injury", table.concat(injury, " "))
    for _, field in ipairs({ "Weapon R", "Weapon L", "Weapon R_0", "Weapon L_0" }) do
        pcall(function()
            local wp = w[field]
            if not valid(wp) then return end
            local points = { nm(wp) }
            local function point(label, obj, actor)
                pcall(function()
                    if not valid(obj) then return end
                    local v = actor and obj:K2_GetActorLocation() or obj:K2_GetComponentLocation()
                    points[#points+1] = string.format("%s:(%.2f,%.2f,%.2f)", label, v.X, v.Y, v.Z)
                end)
            end
            point("actor", wp, true)
            point("base", wp["Root Scene"], false)
            point("tip", wp.TippyTipScene, false)
            add(field:gsub("%s", ""), table.concat(points, " "))
        end)
    end
    for _,field in ipairs({"Weapon R","Weapon L","Weapon Slot R 1","Weapon Slot R 2","Weapon Slot L 1","Weapon Slot L 2","Weapon Slot Back"}) do
        pcall(function()
            local wp = w[field]
            if valid(wp) and valid(wp.BaseMesh) then
                Log("KIT_WEIGHT %s field=%s actor=%s mass=%.4f gripR=%s gripL=%s held=%s",label,field,nm(wp),wp.BaseMesh:GetMass(),
                    tostring(wp["Grip R Hand Default"]),tostring(wp["Grip L Hand Default"]),tostring(wp["Is Held"]))
            end
        end)
    end
    Log("KIT %s %s %s", label, nm(w), table.concat(parts, " "))
end
local function exp_kit()
    pcall(function()
        local gi = UEHelpers.GetGameInstance()
        if valid(gi) then
            Log("KIT_MODE game_instance=%s native_game_mode=%s native_play_mode=%s",
                nm(gi), tostring(gi["Current Game Mode Enum"]), tostring(gi["Current Play Mode"]))
        end
    end)
    local me = me_pawn()
    if me then kit_of("me", me) end
    local ipc = rawget(_G, "HSMP_IPC")
    local pt = ipc and ipc.bus_table and ipc.bus_table("puppets")
    local want = {}
    for _, r in ipairs((pt and pt.rows) or {}) do
        if r.peer and r.peer ~= 0 and type(r.name) == "string" then want[r.name] = r.peer end
    end
    local ws = FindAllOf("Willie_BP_C") or {}
    for _, w in pairs(ws) do
        if valid(w) and want[nm(w)] then kit_of("standin_peer_" .. want[nm(w)], w) end
    end
end

-- ---- spots ------------------------------------------------------------------------------
local BONES = { "head", "spine_03", "spine_05", "lowerarm_r", "upperarm_l", "thigh_l", "calf_r" }
-- spot directions in the bone's own frame (UE skeleton bones: X along the bone)
local DIRS = { { "pY", { 0, 1, 0 } }, { "nY", { 0, -1, 0 } }, { "pZ", { 0, 0, 1 } }, { "nZ", { 0, 0, -1 } } }
-- (name, Hit Velocity, Hit Impulse, Cutting Power, Stab Rate, Rigidity, Kick, weapon)
local BLOWS = {
    { "mace",  1300, 900, 0, 0, 3.0, 1, true },
    { "cut",   1500, 600, 95, 0.05, 0.8, 1, true },
    { "stab",  1100, 500, 90, 1.0, 0.85, 1, true },
    { "fist",   900, 900, 8, 0, 1.6, 1, false },
}
local function my_weapon_comp(me)
    local c
    pcall(function()
        local wp = me["Weapon R"]
        if not valid(wp) then return end
        local arr = wp["Collision Components Array"]
        if arr then arr:ForEach(function(_, e) if not c then c = e:get() end end) end
        if not valid(c) then c = wp:K2_GetRootComponent() end
    end)
    if c and valid(c) then
        local tag = false
        pcall(function() tag = c:ComponentHasTag(FName("Weapon")) == true end)
        return c, tag
    end
    return nil, false
end
-- One Deal Complex Damage on my own pawn; returns the field changes, then puts every field back.
local function blow(me, mesh, bone, loc, nrm, vel, imp, b, coll)
    local s0 = snap(me)
    pcall(function() me["Last Complex Damage Impulse"] = 0; me["Last Damage Taken"] = 0 end)
    local outs = { {}, {}, {}, {}, {}, {} }
    local ok, err = bp_call(me, "Deal Complex Damage", mesh, coll, FName(bone), V(loc), V(nrm), V(vel), V(imp),
        b[4], b[5], b[6], 0, false, false, b[7], nil, false, 0.0,
        outs[1], outs[2], outs[3], outs[4], outs[5], outs[6])
    local s1 = snap(me)
    put(me, s0)
    local d = dfields(s0, s1)
    return ok, err, d, s1
end
local function exp_spots(arg)
    local me = me_pawn()
    if not me then Log("spots: no pawn"); return end
    local deg = tonumber(arg) or 70
    local mesh = me.Mesh
    local coll, tagged = my_weapon_comp(me)
    Log("spots: turn %.0f deg, my weapon component %s (tag Weapon=%s)", deg, coll and nm(coll) or "none", tostring(tagged))
    local n, n_world, n_local = 0, 0, 0
    for _, bone in ipairs(BONES) do
        local fr = BF.of(mesh, bone)
        if not fr then Log("spots: no frame for %s", bone) else
            for _, dd in ipairs(DIRS) do
                -- the surface point: from 40 uu out along the spot direction, the
                -- distance GetClosestPointOnCollision reports back to the body
                local dw = BF.rot(fr.q, dd[2])
                local far = { fr.p[1] + 40 * dw[1], fr.p[2] + 40 * dw[2], fr.p[3] + 40 * dw[3] }
                local dist
                pcall(function() dist = tonumber(mesh:GetClosestPointOnCollision(V(far), {}, FName(bone))) end)
                if not dist or dist < 0 then dist = 30 end
                local loc = { far[1] - dist * dw[1], far[2] - dist * dw[2], far[3] - dist * dw[3] }
                for _, b in ipairs(BLOWS) do
                    local vel = { -dw[1] * b[2], -dw[2] * b[2], -dw[3] * b[2] }
                    local imp = { -dw[1] * b[3], -dw[2] * b[3], -dw[3] * b[3] }
                    local c = b[8] and coll or nil
                    local ok, err, solo = blow(me, mesh, bone, loc, dw, vel, imp, b, c)
                    -- world: the stand-in stood `deg` turned; its world offset re-added here
                    local off = { loc[1] - fr.p[1], loc[2] - fr.p[2], loc[3] - fr.p[3] }
                    local lw = zrot(deg, off)
                    local locw = { fr.p[1] + lw[1], fr.p[2] + lw[2], fr.p[3] + lw[3] }
                    local _, _, world = blow(me, mesh, bone, locw, zrot(deg, dw), zrot(deg, vel), zrot(deg, imp), b, c)
                    -- local: the stand-in's frame turned `deg`; bone-space offset carried over
                    local qs = { 0, 0, math.sin(math.rad(deg) / 2), math.cos(math.rad(deg) / 2) }
                    local qsi = { 0, 0, 0, 1 }
                    do  -- stand-in frame = Rz(deg) * my frame
                        local a, q = qs, fr.q
                        qsi = { a[4] * q[1] + a[1] * q[4] + a[2] * q[3] - a[3] * q[2],
                                a[4] * q[2] - a[1] * q[3] + a[2] * q[4] + a[3] * q[1],
                                a[4] * q[3] + a[1] * q[2] - a[2] * q[1] + a[3] * q[4],
                                a[4] * q[4] - a[1] * q[1] - a[2] * q[2] - a[3] * q[3] }
                    end
                    local loc_si = { fr.p[1] + lw[1], fr.p[2] + lw[2], fr.p[3] + lw[3] }   -- the same spot on the turned stand-in
                    local offl = BF.rot(qsi, { loc_si[1] - fr.p[1], loc_si[2] - fr.p[2], loc_si[3] - fr.p[3] }, true)
                    local back = BF.rot(fr.q, offl)
                    local locl = { fr.p[1] + back[1], fr.p[2] + back[2], fr.p[3] + back[3] }
                    local vl = BF.rot(fr.q, BF.rot(qsi, zrot(deg, vel), true))
                    local il = BF.rot(fr.q, BF.rot(qsi, zrot(deg, imp), true))
                    local nl = BF.rot(fr.q, BF.rot(qsi, zrot(deg, dw), true))
                    local _, _, loc_res = blow(me, mesh, bone, locl, nl, vl, il, b, c)
                    local s, w, l = table.concat(solo, " "), table.concat(world, " "), table.concat(loc_res, " ")
                    n = n + 1
                    if w == s then n_world = n_world + 1 end
                    if l == s then n_local = n_local + 1 end
                    Log("SPOT %s %s %s ok=%s%s | solo [%s] | world [%s]%s | local [%s]%s", bone, dd[1], b[1], tostring(ok),
                        ok and "" or (" " .. tostring(err)), s, w, w == s and " =" or " DIFF", l, l == s and " =" or " DIFF")
                end
            end
        end
    end
    Log("SPOTS done: %d blows; world offset same as solo %d, bone-local same as solo %d", n, n_world, n_local)
end

-- ---- near / swing -------------------------------------------------------------------------
local function standin_of(peer)
    local ipc = rawget(_G, "HSMP_IPC")
    local pt = ipc and ipc.bus_table and ipc.bus_table("puppets")
    local name
    for _, r in ipairs((pt and pt.rows) or {}) do
        if (peer == nil and r.peer and r.peer ~= 0) or r.peer == peer then name = r.name; peer = r.peer; break end
    end
    if not name then return nil end
    for _, w in pairs(FindAllOf("Willie_BP_C") or {}) do
        if valid(w) and nm(w) == name then return w, peer end
    end
    return nil
end
local function exp_near(arg)
    local me = me_pawn()
    local si, peer = standin_of(tonumber(arg))
    if not (me and si) then Log("near: no pawn or stand-in"); return end
    local p = vec(si:K2_GetActorLocation())
    local f = vec(si:GetActorForwardVector())
    local dest = { p[1] + 85 * f[1], p[2] + 85 * f[2], p[3] }
    pcall(function() me:K2_SetActorLocation(V(dest), false, {}, true) end)
    local yaw = math.deg(math.atan(-f[2], -f[1]))
    pcall(function() me:K2_SetActorRotation({ Pitch = 0, Yaw = yaw, Roll = 0 }, true) end)
    Log("near: me -> %s, facing stand-in of peer %s", f3(dest), tostring(peer))
end
local swing = nil
local function exp_swing(arg)
    local me = me_pawn()
    local si, peer = standin_of(nil)
    if not (me and si) then Log("swing: no pawn or stand-in"); return end
    local wp; pcall(function() wp = me["Weapon R"] end)
    local root; pcall(function() root = wp:K2_GetRootComponent() end)
    if not valid(root) then Log("swing: no weapon in my right hand"); return end
    swing = { root = root, si = si, peer = peer, speed = tonumber(arg) or 1200, frames = 3, key = WG.key }
    Log("swing: %s at stand-in of peer %s, %.0f uu/s", nm(wp), tostring(peer), swing.speed)
end
local function swing_tick()
    local s = swing
    if not s then return end
    if s.key ~= WG.key or s.frames <= 0 then swing = nil; return end
    s.frames = s.frames - 1
    pcall(function()
        local tgt = vec(s.si.Mesh:GetSocketLocation(FName("spine_03")))
        local at = vec(s.root:K2_GetComponentLocation())
        local d = { tgt[1] - at[1], tgt[2] - at[2], tgt[3] - at[3] }
        local l = math.sqrt(d[1] * d[1] + d[2] * d[2] + d[3] * d[3])
        if l < 1 then return end
        s.root:SetPhysicsLinearVelocity(V({ d[1] / l * s.speed, d[2] / l * s.speed, d[3] / l * s.speed }), false, FName("None"))
    end)
end

-- ---- capture: every Deal Complex Damage the game makes (inputs, for calibration) -----------
local captured = 0
local function on_dcd(selfp, hc, cc, bone, loc, nrm, vel, imp, cp, stab, rig, blunt, lower, dp, kick)
    captured = captured + 1
    if captured > 3000 then return end   -- (enough for a calibration run: calib MIN_N per class)
    local function g(p) local v; pcall(function() v = p:get() end); return v end
    local w = g(selfp)
    local b = "?"; pcall(function() b = g(bone):ToString() end)
    local c = g(cc)
    local actor
    if valid(c) then pcall(function() actor = c:GetOwner() end) end
    if not valid(actor) then actor = nil end
    -- A null UObject is still a truthy userdata; GetClass on it faults in
    -- native UE4SS code even inside pcall (e.g. a dropped weapon contact).
    local owner = "?"; if actor then pcall(function() owner = actor:GetClass():GetFName():ToString() end) end
    -- the weapon actor (its name carries the weapon type: hit_vel_factor calibration)
    local wp = "?"; if actor then pcall(function() wp = actor:GetFName():ToString() end) end
    local cv, wv = { 0, 0, 0 }, { 0, 0, 0 }
    if valid(c) then pcall(function() cv = vec(c:GetPhysicsLinearVelocity(FName("None"))) end) end
    local hit = g(hc)
    if valid(hit) then pcall(function() wv = vec(hit:GetPhysicsLinearVelocity(g(bone))) end) end
    local rel = math.sqrt((cv[1] - wv[1]) ^ 2 + (cv[2] - wv[2]) ^ 2 + (cv[3] - wv[3]) ^ 2)
    local function L(p) local t = vec(g(p)); return math.sqrt(t[1] ^ 2 + t[2] ^ 2 + t[3] ^ 2) end
    Log("DCD on %s bone=%s by %s wp=%s: |vel|=%.0f |imp|=%.0f rel=%.0f cut=%.1f stab=%.2f rig=%.2f kick=%.1f",
        nm(w), b, owner, wp, L(vel), L(imp), rel, num(g(cp)), num(g(stab)), num(g(rig)), num(g(kick)))
    if actor and valid(c) then pcall(function()
        local cls=c:GetClass():GetFName():ToString()
        if not (cls:find("SphereComponent",1,true) or cls:find("BoxComponent",1,true) or cls:find("CapsuleComponent",1,true)) then return end
        local t=c:GetSocketTransform(FName("None"),0)
        local p,q,scale=t.Translation,t.Rotation,t.Scale3D
        local contact=vec(g(loc))
        local detail=""
        if cls:find("SphereComponent",1,true) then
            detail=string.format("radius=%.3f",c:GetScaledSphereRadius())
        elseif cls:find("BoxComponent",1,true) then
            local e=c:GetUnscaledBoxExtent()
            detail=string.format("half=(%.3f,%.3f,%.3f)",e.X*math.abs(scale.X),e.Y*math.abs(scale.Y),e.Z*math.abs(scale.Z))
        else detail=string.format("radius=%.3f halfheight=%.3f",c:GetScaledCapsuleRadius(),c:GetScaledCapsuleHalfHeight()) end
        local physical="unavailable"
        pcall(function()
            local com=c:GetCenterOfMass(FName("None"))
            physical=string.format("com=(%.3f,%.3f,%.3f) simulating=%s mass=%.5f",com.X,com.Y,com.Z,tostring(c:IsSimulatingPhysics(FName("None"))),c:GetMass())
        end)
        local parent=actor["Parent Actor"]
        local link="unknown"
        local own=false
        if valid(parent) then
            local me=me_pawn()
            own=valid(me) and me:GetAddress()==parent:GetAddress()
            for _,entry in ipairs({{"Weapon R","hand_r"},{"Weapon L","hand_l"},{"Foot R Weapon","foot_r"},{"Foot L Weapon","foot_l"}}) do pcall(function()
                local held=parent[entry[1]]
                if valid(held) and held:GetAddress()==actor:GetAddress() then
                    local mesh=parent.Mesh
                    if valid(mesh) then
                        local bt=mesh:GetSocketTransform(FName(entry[2]),0)
                        link=string.format("%s bone_p=(%.3f,%.3f,%.3f)",entry[1],bt.Translation.X,bt.Translation.Y,bt.Translation.Z)
                        pcall(function()
                            local bc=mesh:GetCenterOfMass(FName(entry[2]))
                            link=link..string.format(" bone_com=(%.3f,%.3f,%.3f)",bc.X,bc.Y,bc.Z)
                        end)
                    end
                end
            end) end
        end
        Log("DCD primitive wp=%s comp=%s class=%s at_ms=%d contact=(%.3f,%.3f,%.3f) p=(%.3f,%.3f,%.3f) q=(%.5f,%.5f,%.5f,%.5f) %s physics=%s parent=%s own_source=%s link=%s",
            wp,nm(c),cls,math.floor(os.clock()*1000),contact[1],contact[2],contact[3],p.X,p.Y,p.Z,q.X,q.Y,q.Z,q.W,detail,physical,nm(parent),tostring(own),link)
    end) end
    -- Capture the weapon geometry in the collision callback itself. A later
    -- KIT snapshot cannot distinguish a missing haft/guard from a pose that
    -- moved after this contact. `along` deliberately remains unclamped: a
    -- negative value identifies a contact behind the streamed root scene.
    if actor and owner ~= "Willie_BP_C" then pcall(function()
        local base, tip = actor["Root Scene"], actor.TippyTipScene
        if not (valid(base) and valid(tip)) then return end
        local a, z, p = vec(base:K2_GetComponentLocation()), vec(tip:K2_GetComponentLocation()), vec(g(loc))
        local d = { z[1] - a[1], z[2] - a[2], z[3] - a[3] }
        local l2 = d[1]^2 + d[2]^2 + d[3]^2
        if l2 < 1 then return end
        local u = ((p[1]-a[1])*d[1] + (p[2]-a[2])*d[2] + (p[3]-a[3])*d[3]) / l2
        local v = math.max(0, math.min(1, u))
        local ds, dl = 0, 0
        for i = 1, 3 do
            ds = ds + (p[i] - a[i] - v*d[i])^2
            dl = dl + (p[i] - a[i] - u*d[i])^2
        end
        Log("DCD geometry wp=%s comp=%s at_ms=%d contact=(%.2f,%.2f,%.2f) base=(%.2f,%.2f,%.2f) tip=(%.2f,%.2f,%.2f) along=%.3f segment=%.2f axis=%.2f",
            wp, nm(c), math.floor(os.clock()*1000), p[1],p[2],p[3], a[1],a[2],a[3], z[1],z[2],z[3], u, math.sqrt(ds), math.sqrt(dl))
    end) end
end
local hooked = false

-- ---- command channel -------------------------------------------------------------------------
local ARM_AXIS = {
    r = "InpAxisEvt_Right Arm Axis_K2Node_InputAxisEvent_2",
    l = "InpAxisEvt_Left Arm Axis_K2Node_InputAxisEvent_3",
}
local arm_drive, arm_injecting = nil, false
local arm_hooks = {}
local function arm_axis_after(which, selfp)
    local s = arm_drive
    if arm_injecting or not s or s.hand ~= which then return end
    if WG.key ~= s.key or os.clock() >= s.until_t then
        arm_drive = nil -- this native callback already restored normal input
        Log("arm: finished hand=%s injected_frames=%d", s.hand, s.frames)
        return
    end
    local me = me_pawn()
    local w; pcall(function() w = selfp:get() end)
    if not (valid(me) and valid(w) and nm(me) == s.name and nm(w) == s.name) then return end
    arm_injecting = true
    local ok, err = bp_call(w, ARM_AXIS[which], 1.0)
    arm_injecting = false
    if not ok then
        arm_drive = nil
        Log("arm: input failed: %s", tostring(err))
        return
    end
    s.frames = s.frames + 1
end
local function exp_arm(arg)
    local hand, seconds = tostring(arg):match("^([rl])%s*([%d%.]*)$")
    if not hand then Log("arm: expected r or l, optional duration"); return end
    local me = me_pawn()
    if not valid(me) then Log("arm: no pawn"); return end
    if not arm_hooks[hand] then
        local ok, err = pcall(function()
            RegisterHook("/Game/Character/Blueprints/Willie_BP.Willie_BP_C:" .. ARM_AXIS[hand],
                function(selfp) pcall(arm_axis_after, hand, selfp) end)
        end)
        if not ok then Log("arm: native input hook unavailable: %s", tostring(err)); return end
        arm_hooks[hand] = true
    end
    arm_drive = { hand=hand, name=nm(me), key=WG.key, frames=0,
        until_t=os.clock()+math.max(0.1, math.min(2, tonumber(seconds) or 0.5)) }
    Log("arm: started hand=%s pawn=%s (real input, verify native contacts)", hand, nm(me))
end
local function exp_bounds()
    local me = me_pawn()
    if not valid(me) then Log("BOUNDS no pawn"); return end
    for _, field in ipairs({ "Weapon R", "Weapon L" }) do pcall(function()
        local wp = me[field]
        if not valid(wp) then return end
        local arr = wp["Collision Components Array"]
        if not arr then Log("BOUNDS %s no collision array", nm(wp)); return end
        arr:ForEach(function(_, e)
            local c = e:get()
            if not valid(c) then return end
            local lo, hi = { X = 0, Y = 0, Z = 0 }, { X = 0, Y = 0, Z = 0 }
            local ok, why = pcall(function() c:GetLocalBounds(lo, hi) end)
            if not ok then Log("BOUNDS wp=%s comp=%s unavailable=%s", nm(wp), nm(c), tostring(why)); return end
            local t = c:GetSocketTransform(FName("None"), 0)
            local p, q, s = t.Translation, t.Rotation, t.Scale3D
            Log("BOUNDS wp=%s comp=%s lo=(%.3f,%.3f,%.3f) hi=(%.3f,%.3f,%.3f) p=(%.3f,%.3f,%.3f) q=(%.5f,%.5f,%.5f,%.5f) scale=(%.3f,%.3f,%.3f)",
                nm(wp), nm(c), lo.X,lo.Y,lo.Z, hi.X,hi.Y,hi.Z, p.X,p.Y,p.Z, q.X,q.Y,q.Z,q.W, s.X,s.Y,s.Z)
        end)
    end) end
end
local function exp_colliders(arg, probe_weapon)
    local me
    if tonumber(arg) then me=standin_of(tonumber(arg)) else me=me_pawn() end
    if not valid(me) and not valid(probe_weapon) then Log("COLLIDERS no pawn"); return end
    local function get(f) local ok,v=pcall(f); if ok then return v end end
    local function vec(v) return v and string.format("(%.3f,%.3f,%.3f)",v.X,v.Y,v.Z) or "none" end
    local function xf(c,bone)
        local t=c:GetSocketTransform(bone or FName("None"),0)
        return string.format("p=%s q=(%.5f,%.5f,%.5f,%.5f) scale=%s",vec(t.Translation),t.Rotation.X,t.Rotation.Y,t.Rotation.Z,t.Rotation.W,vec(t.Scale3D))
    end
    local function each(arr,f)
        local n=0
        arr:ForEach(function(_,e)
            n=n+1
            if n>256 then return end
            local value=get(function() return e:get() end) or e
            f(value,n)
        end)
        return n
    end
    local function aggregate(bs,c,tag,bone)
        if not valid(bs) then Log("COLLIDERS %s no_body_setup",tag); return end
        local ag=bs.AggGeom
        Log("COLLIDERS %s setup=%s bone=%s trace=%s %s",tag,nm(bs),tostring(bone),tostring(get(function()return bs.CollisionTraceFlag end)),xf(c,bone))
        for _,kind in ipairs({"SphereElems","BoxElems","SphylElems","ConvexElems","TaperedCapsuleElems","LevelSetElems","SkinnedLevelSetElems"}) do
            local ok,n=pcall(function() return each(ag[kind],function(s,i)
                local detail=""
                if kind=="BoxElems" then detail=string.format("center=%s rot=(%.3f,%.3f,%.3f) size=(%.3f,%.3f,%.3f)",vec(s.Center),s.Rotation.Pitch,s.Rotation.Yaw,s.Rotation.Roll,s.X,s.Y,s.Z)
                elseif kind=="SphereElems" then detail=string.format("center=%s radius=%.3f",vec(s.Center),s.Radius)
                elseif kind=="SphylElems" or kind=="TaperedCapsuleElems" then
                    detail=string.format("center=%s rot=(%.3f,%.3f,%.3f) length=%.3f radius=%s radius0=%s radius1=%s",vec(s.Center),s.Rotation.Pitch,s.Rotation.Yaw,s.Rotation.Roll,s.Length,tostring(get(function()return s.Radius end)),tostring(get(function()return s.Radius0 end)),tostring(get(function()return s.Radius1 end)))
                elseif kind=="ConvexElems" then
                    local vertices=each(s.VertexData,function()end)
                    detail=string.format("vertices=%d min=%s max=%s local_p=%s local_scale=%s",vertices,vec(s.ElemBox.Min),vec(s.ElemBox.Max),vec(s.Transform.Translation),vec(s.Transform.Scale3D))
                end
                Log("COLLIDERS %s %s[%d] enabled=%s %s",tag,kind,i,tostring(get(function()return s.CollisionEnabled end)),detail)
            end) end)
            Log("COLLIDERS %s %s count=%s",tag,kind,ok and tostring(n) or ("ERROR:"..tostring(n)))
        end
    end
    for _,field in ipairs(probe_weapon and {"inventory"} or {"Weapon R","Weapon L","Foot R Weapon","Foot L Weapon"}) do
        local ok,why=pcall(function()
            local wp=probe_weapon or me[field]
            if not valid(wp) then Log("COLLIDERS field=%s absent",field); return end
            local count=each(wp["Collision Components Array"],function(c,ordinal)
                local tag=string.format("field=%s wp=%s ordinal=%d comp=%s",field,nm(wp),ordinal,nm(c))
                local good,err=pcall(function()
                    if not valid(c) then Log("COLLIDERS %s invalid",tag); return end
                    local class=nm(c:GetClass())
                    Log("COLLIDERS %s class=%s enabled=%s %s",tag,class,tostring(c:GetCollisionEnabled()),xf(c))
                    if class:find("BoxComponent",1,true) then Log("COLLIDERS %s primitive_box_half=%s",tag,vec(c:GetUnscaledBoxExtent()))
                    elseif class:find("SphereComponent",1,true) then Log("COLLIDERS %s primitive_sphere_radius=%.3f",tag,c:GetUnscaledSphereRadius())
                    elseif class:find("CapsuleComponent",1,true) then Log("COLLIDERS %s primitive_capsule_radius=%.3f halfheight=%.3f",tag,c:GetUnscaledCapsuleRadius(),c:GetUnscaledCapsuleHalfHeight())
                    end
                    local mesh=get(function()return c.StaticMesh end)
                    if valid(mesh) then aggregate(mesh.BodySetup,c,tag,FName("None")); return end
                    mesh=get(function()return c:GetSkeletalMeshAsset() end)
                    if valid(mesh) then
                        local pa=get(function()return c.PhysicsAssetOverride end)
                        if not valid(pa) then pa=mesh:GetPhysicsAsset() end
                        if not valid(pa) then Log("COLLIDERS %s skeletal_no_physics_asset mesh=%s",tag,nm(mesh)); return end
                        local bodies=each(pa.SkeletalBodySetups,function(bs,bi)
                            aggregate(bs,c,tag.." body="..bi,bs.BoneName)
                        end)
                        Log("COLLIDERS %s physics_asset=%s bodies=%d",tag,nm(pa),bodies)
                        return
                    end
                    local bs=get(function()return c.ShapeBodySetup end)
                    if valid(bs) then aggregate(bs,c,tag,FName("None")) end
                end)
                if not good then Log("COLLIDERS %s ERROR=%s",tag,tostring(err)) end
            end)
            Log("COLLIDERS field=%s wp=%s array_count=%d",field,nm(wp),count)
        end)
        if not ok then Log("COLLIDERS field=%s ERROR=%s",field,tostring(why)) end
    end
end
local inventory
local function exp_inventory(arg)
    if inventory and arg=="stop" then inventory.stop(); return end
    if arg=="status" then
        if inventory then inventory.status(WG.key) else Log("INVENTORY status no_owned_actor_in_current_world") end
        return
    end
    local module=load_module("collider_inventory")
    if not module then Log("INVENTORY diagnostic_module_missing"); return end
    local live_lookup=load_module("inventory_world_lookup")
    if not live_lookup then Log("INVENTORY exact_world_lookup_missing");return end
    local src=debug.getinfo(1,"S").source:gsub("^@","")
    local dir=src:match("^(.*)[/\\]") or "."
    local catalog
    for _,p in ipairs({dir.."/../../HSMPLoadout/Scripts/hsmp_catalog.lua",dir.."/../../../HSMPLoadout/Scripts/hsmp_catalog.lua"}) do
        local ok,v=pcall(dofile,p)
        if ok and type(v)=="table" then catalog=v; break end
    end
    if not catalog then Log("INVENTORY canonical_catalogue_unavailable"); return end
    if arg=="extra" then
        catalog=load_module("native_extra_melee")
        if not catalog then Log("INVENTORY proven_extra_melee_manifest_unavailable");return end
        arg=""
    end
    inventory=inventory or module.new({log=Log,
        lookup=function(class) return FindAllOf(class) or {} end,
        lookup_world=function(ref)
            return live_lookup.query({world=WG.world,valid=valid,find=StaticFindObject,
                gameplay_statics=UEHelpers.GetGameplayStatics},ref)
        end,
        resolve=function(path)
            local c=StaticFindObject(path)
            if not valid(c) then LoadAsset(path:gsub("_C$","")); c=StaticFindObject(path) end
            return c
        end,
        context=function()
            local me=me_pawn()
            if not valid(me) then return nil end
            local p=me:K2_GetActorLocation()
            return WG.world(),UEHelpers.GetGameplayStatics(),{
                Translation={X=p.X,Y=p.Y,Z=p.Z+10000},Rotation={X=0,Y=0,Z=0,W=1},Scale3D={X=1,Y=1,Z=1}}
        end,
        inspect=function(actor,id) Log("INVENTORY inspect class=%s actor=%s",id,nm(actor)); exp_colliders(nil,actor) end,
    })
    inventory.start(catalog,WG.key,arg)
end
local cut_proxy,cut_second
local function cutproxy_report(stage,c,pawn)
    if not valid(c) then Log("CUTPROXY stage=%s passed=false factory_unavailable",stage);return end
    local a=c:GetOwner()
    local t=c:GetSocketTransform(FName("None"),0)
    local e=c:GetUnscaledBoxExtent()
    Log("CUTPROXY stage=%s world=%s pawn=%s pawn_address=%s actor=%s actor_class=%s box=%s box_class=%s actor_collision=%s box_collision=%s simulating=%s extent=%s scale=%s position=%s",
        stage,tostring(WG.key),nm(pawn),tostring(pawn:GetAddress()),a:GetFullName(),a:GetClass():GetFName():ToString(),
        c:GetFullName(),c:GetClass():GetFName():ToString(),tostring(a:GetActorEnableCollision()),tostring(c:GetCollisionEnabled()),
        tostring(c:IsSimulatingPhysics(FName("None"))),f3(vec(e)),f3(vec(t.Scale3D)),f3(vec(t.Translation)))
end
local function exp_cutproxy()
    local pawn=me_pawn()
    if not valid(pawn) then Log("CUTPROXY passed=false no_current_pawn");return end
    if not cut_proxy then
        local src=debug.getinfo(1,"S").source:gsub("^@","")
        local dir=src:match("^(.*)[/\\]") or "."
        local module
        for _,path in ipairs({dir.."/../../HSMPCombat/Scripts/cutting_box.lua",dir.."/../../../HSMPCombat/Scripts/cutting_box.lua"})do
            local ok,m=pcall(dofile,path)
            if ok and type(m)=="table" and type(m.new)=="function" then module=m;break end
        end
        if not module then Log("CUTPROXY passed=false exact_combat_factory_module_missing");return end
        cut_proxy=module.new({UEHelpers=UEHelpers,log=Log,bf={to_world=function(f,p)
            local v=BF.rot(f.q,{p[1]*f.s,p[2]*f.s,p[3]*f.s})
            return {f.p[1]+v[1],f.p[2]+v[2],f.p[3]+v[3]}
        end}})
    end
    local p=pawn:K2_GetActorLocation()
    local frame={p={p.X,p.Y,p.Z},q={0,0,0,1},s=1}
    local c=cut_proxy.component(pawn,{20,10,0,0,0,0,1,2,1,1,30,2,12},frame,"native-factory-smoke")
    cutproxy_report("first",c,pawn)
    if not valid(c) then return end
    cut_second={key=WG.key,pawn=pawn:GetAddress(),name=nm(pawn),box=c:GetAddress(),frame=frame}
end
local function cutproxy_tick()
    local s=cut_second;cut_second=nil
    if not s then return end
    local pawn=me_pawn()
    if WG.key~=s.key or not valid(pawn) or pawn:GetAddress()~=s.pawn or nm(pawn)~=s.name then
        Log("CUTPROXY passed=false context_changed_before_reuse");return
    end
    local c=cut_proxy.component(pawn,{25,12,5,0,0,0,1,1.5,1.25,0.75,20,3,16},s.frame,"native-factory-smoke")
    cutproxy_report("second",c,pawn)
    Log("CUTPROXY passed=%s reused_slot=%s native_damage_calls=0 live_weapon_mutations=0",
        tostring(valid(c) and c:GetAddress()==s.box),tostring(valid(c) and c:GetAddress()==s.box))
end
-- Dev-only boundary check: hold the native severe-KO condition, then let
-- Willie's own delayed loss logic run. Never call Lose Match/Death or send
-- an outcome record here. This checks outcome wiring, not punch strength.
local defeat_probe
local function exp_defeat()
    local pawn,view,mode=me_pawn(),HSESS and HSESS.view(),HSESS and HSESS.mode()
    if not (valid(pawn) and view and view.phase==3 and mode and mode.match_id==view.match_id) then
        Log("DEFEAT_PROBE refused: requires a live isolated multiplayer round");return
    end
    if mode.id~="brawl" then Log("DEFEAT_PROBE refused: knockout boundary requires Brawl");return end
    local row=mode.rows and mode.rows[view.my_peer_id]
    local ipc=rawget(_G,"HSMP_IPC")
    local status=ipc and ipc.bus_table and ipc.bus_table("spawn_status")
    if not (row and row.alive and row.life>0 and mode.round==view.round and status and status.verified==true
        and status.match_id==view.match_id and status.round==view.round and status.life==row.life and status.pawn==nm(pawn)) then
        Log("DEFEAT_PROBE refused: original placed life unavailable");return
    end
    local cap,hp,player,dead,give,zombie,native_mode
    pcall(function() cap=tonumber(pawn["Consciousness Cap"]);hp=tonumber(pawn.Health);player=pawn.Player;dead=pawn.DED;give=pawn["Give Up"];zombie=pawn["Is Zombie?"];native_mode=tonumber(pawn["Current Game Mode Enum"]) end)
    if not (cap and hp and hp>0 and player==true and dead~=true and give==false and zombie~=true and native_mode~=5) or defeat_probe then
        Log("DEFEAT_PROBE refused: native living player/cap unavailable or probe active");return
    end
    defeat_probe={key=WG.key,pawn=pawn:GetAddress(),name=nm(pawn),cap=cap,hp=hp,
        match_id=view.match_id,round=view.round,life=row.life,peer=view.my_peer_id,until_t=os.clock()+12}
    Log("DEFEAT_PROBE started pawn=%s match=%s round=%s Health=%.3f native_cap_condition=-60 no_damage_records=true",
        nm(pawn),tostring(view.match_id),tostring(view.round),hp)
end
local function defeat_tick()
    local s=defeat_probe;if not s then return end
    local pawn,view,mode=me_pawn(),HSESS and HSESS.view(),HSESS and HSESS.mode()
    if WG.key~=s.key or not valid(pawn) or pawn:GetAddress()~=s.pawn or nm(pawn)~=s.name then
        defeat_probe=nil;Log("DEFEAT_PROBE cancelled: pawn/world changed; old objects untouched");return
    end
    local row=mode and mode.rows and mode.rows[s.peer]
    if not view or view.match_id~=s.match_id or view.round~=s.round or view.my_peer_id~=s.peer
        or not mode or mode.match_id~=s.match_id or mode.round~=s.round or not row or row.life~=s.life then
        defeat_probe=nil;Log("DEFEAT_PROBE cancelled: original life changed; old cap untouched");return
    end
    local give,dead,hp
    pcall(function()give=pawn["Give Up"];dead=pawn.DED;hp=pawn.Health end)
    if give==true then
        defeat_probe=nil
        Log("DEFEAT_PROBE native_loss_seen pawn=%s Health=%s DED=%s elapsed_condition_complete=true no_damage_records=true",
            s.name,tostring(hp),tostring(dead));return
    end
    if view.phase~=3 or not row.alive or os.clock()>=s.until_t then
        defeat_probe=nil;pcall(function()pawn["Consciousness Cap"]=s.cap end)
        Log("DEFEAT_PROBE stopped without native loss; original cap restored");return
    end
    pcall(function()pawn["Consciousness Cap"]=-60 end)
end
WG.on_drop(function(why)
    cut_second=nil
    defeat_probe=nil
    if cut_proxy then cut_proxy.clear(why) end
end)
local function exp_modules(arg)
    local pawn=tonumber(arg) and standin_of(tonumber(arg)) or me_pawn()
    local resolver=load_module("native_weapon_modules")
    if not valid(pawn) or not resolver then Log("MODULES unavailable pawn/resolver");return end
    for _,field in ipairs({"Weapon R","Weapon L"}) do
        local ok,err=pcall(function()
            local weapon=pawn[field]
            if not valid(weapon) then Log("MODULES field=%s no weapon",field);return end
            local rows,why=resolver.of(weapon)
            if not rows then Log("MODULES field=%s weapon=%s REFUSED reason=%s",field,nm(weapon),tostring(why));return end
            local selected=weapon["Hit Box Collision"]
            Log("MODULES field=%s weapon=%s rows=%d selected=%s",field,nm(weapon),#rows,nm(selected))
            for _,r in ipairs(rows) do
                local c=r.component
                local is_selected=valid(selected) and selected:GetAddress()==c:GetAddress()
                Log("MODULES id=%d child_of=%d component=%s class=%s selected=%s",r.id,r.child_of,nm(c),nm(c:GetClass()),tostring(is_selected))
            end
        end)
        if not ok then Log("MODULES field=%s native inspection failed: %s",field,tostring(err)) end
    end
end
local function exp_components()
    local pawn=me_pawn()
    local cls=StaticFindObject("/Script/Engine.PhysicsConstraintComponent")
    if not valid(pawn) or not valid(cls) then Log("COMPONENTS no native pawn/class");return end
    local actors={pawn}
    for _,field in ipairs({"Weapon R","Weapon L"}) do
        local ok,a=pcall(function()return pawn[field]end)
        if ok and valid(a) then actors[#actors+1]=a end
    end
    for _,actor in ipairs(actors) do
        local ok,err=pcall(function()
            local result=actor:K2_GetComponentsByClass(cls)
            if type(result)~="table" then error("unexpected native returned array: "..type(result)) end
            if #result>64 then error("native component diagnostic capacity exceeded") end
            Log("COMPONENTS actor=%s returned=table count=%d",nm(actor),#result)
            for i,wrapped in ipairs(result) do
                local c=wrapped:get()
                Log("COMPONENTS actor=%s ordinal=%d unwrapped=%s class=%s",nm(actor),i,nm(c),valid(c) and nm(c:GetClass()) or "invalid")
            end
        end)
        if not ok then Log("COMPONENTS actor=%s REFUSED reason=%s",nm(actor),tostring(err)) end
    end
end
local fist_factory
local function exp_fists(arg)
    local pawn=me_pawn()
    local function context(p)
        local view,mode=HSESS and HSESS.view(),HSESS and HSESS.mode()
        local ipc=rawget(_G,"HSMP_IPC")
        local status=ipc and ipc.bus_table("spawn_status")
        local row=mode and view and mode.rows and mode.rows[view.my_peer_id]
        if not valid(p) or p:GetAddress()~=pawn:GetAddress() or not row or not view
            or not status or status.verified~=true or status.pawn~=nm(p)
            or mode.match_id~=view.match_id or mode.round~=view.round
            or status.match_id~=mode.match_id or status.round~=mode.round or status.life~=row.life then return nil end
        return {match_id=mode.match_id,round=mode.round,life=row.life}
    end
    local original=valid(pawn) and context(pawn)
    if not original then Log("FISTPROBE passed=false original_verified_context_unavailable");return end
    local src=debug.getinfo(1,"S").source:gsub("^@","")
    local dir=src:match("^(.*)[/\\]") or "."
    local factory
    for _,path in ipairs({dir.."/../../HSMPCombat/Scripts/replay_fists.lua",dir.."/../../../HSMPCombat/Scripts/replay_fists.lua"})do
        local ok,m=pcall(dofile,path);if ok and type(m)=="table" then factory=m;break end
    end
    local smoke=load_module("replay_fists_smoke")
    if not factory or not smoke then Log("FISTPROBE passed=false exact_factory_missing");return end
    -- Refresh context callback; no UObject is retained after this command.
    fist_factory=fist_factory or factory.new({UEHelpers=UEHelpers,log=Log,
        world_key=function()return WG.key end,context=function(p)
            local current=me_pawn()
            if not valid(current) or not valid(p) or current:GetAddress()~=p:GetAddress() then return nil end
            local view,mode=HSESS.view(),HSESS.mode()
            local ipc=rawget(_G,"HSMP_IPC");local status=ipc and ipc.bus_table("spawn_status")
            local row=mode and view and mode.rows and mode.rows[view.my_peer_id]
            if not row or not status or status.verified~=true or status.pawn~=nm(p)
                or mode.match_id~=view.match_id or mode.round~=view.round
                or status.match_id~=mode.match_id or status.round~=mode.round or status.life~=row.life then return nil end
            return {match_id=mode.match_id,round=mode.round,life=row.life}
        end})
    local row,reason=smoke.run({authorized=true,factory=fist_factory,context=context},pawn,arg~="r",original)
    if not row then Log("FISTPROBE passed=false reason=%s",tostring(reason));return end
    Log("FISTPROBE passed=true pawn=%s actor=%s component=%s left=%s radius=%s scaled_radius=%s scale=%s collision=%s held_fields_unchanged=%s native_damage_calls=0",
        row.pawn,row.actor,row.component,tostring(row.left),tostring(row.radius),tostring(row.scaled_radius),f3(row.scale),tostring(row.collision),tostring(row.held_fields_unchanged))
end
local DIAGNOSTIC_CONTEXT=load_module("diagnostic_context")
local function diagnostic_snapshot(peer)
    if not DIAGNOSTIC_CONTEXT or not WG.check() or not WG.settled() then return nil end
    local ipc=rawget(_G,"HSMP_IPC")
    local view,mode=HSESS and HSESS.view(),HSESS and HSESS.mode()
    local status,pawn
    if peer==0 then
        status=ipc and ipc.bus_table("spawn_status")
        -- Placement diagnostics follow the assigned source even during the
        -- native stand-in's temporary PlayerController possession swap.
        if status and type(status.pawn)=="string" and status.pawn~="" then
            for _,candidate in pairs(FindAllOf("Willie_BP_C") or {})do
                if valid(candidate) and nm(candidate)==status.pawn then pawn=candidate;break end
            end
        end
    else
        pawn=standin_of(peer)
        for _,row in ipairs((ipc and ipc.bus_table("playback") or {}).rows or {})do
            if row.peer==peer then status=row;break end
        end
    end
    if not valid(pawn) then return nil end
    local mesh,world=pawn.Mesh,WG.world()
    if not valid(mesh) or not valid(world) then return nil end
    local id=DIAGNOSTIC_CONTEXT.resolve({peer=peer,view=view,mode=mode,status=status,
        pawn=nm(pawn),address=pawn:GetAddress(),mesh=nm(mesh),mesh_address=mesh:GetAddress(),
        world=tostring(world:GetAddress()).."@"..world:GetFullName(),world_key=WG.key})
    return id,pawn,mesh
end
local box_observer
local box_instance=os.getenv("HSMP_INST")
if box_instance~="1" and box_instance~="2" then box_instance="unavailable" end
local function box_log(fmt,...)Log("inst=%s "..fmt,box_instance,...)end
local box_playback={logs=0}
function box_playback.trace(play,now,generation)
    local row=type(play)=="table" and play or nil
    local function finite(v)return type(v)=="number" and v==v and math.abs(v)<math.huge and v or nil end
    local sample=finite(row and rawget(row,"local_ms"))
    now=finite(now)
    return {local_ms=sample,sample_now_ms=now,age_ms=sample and now and finite(now-sample),generation=finite(generation)}
end
function box_playback.scalar(v,kind)
    if kind=="number" and type(v)=="number" and v==v and math.abs(v)<math.huge then return tostring(v),true end
    if kind=="string" and type(v)=="string" and v~="" then
        -- Lua %q can quote LF with a backslash plus physical LF. Escape controls first.
        local text=v:sub(1,128):gsub("%c",function(c)return string.format("\\x%02X",c:byte())end)
        return string.format("%q",text),true
    end
    return "unknown",false
end
function box_playback.refuse(reason,detail,peer,shown,play,now,generation)
    -- Existing scalar snapshots only: explaining a refusal performs no native reads.
    -- Fixed total budget, no world/command reset; strings and row size are bounded too.
    local trace=box_playback.trace(play,now,generation)
    if box_playback.logs>=32 then return nil,reason,trace end
    box_playback.logs=box_playback.logs+1
    local actual_row=type(play)=="table" and play or nil
    local fields={}
    for _,key in ipairs({"pawn","match_id","round","life"})do
        local kind=key=="pawn" and "string" or "number"
        local actual,aa=box_playback.scalar(actual_row and rawget(actual_row,key),kind)
        local expected,ea=box_playback.scalar(shown and shown[key],kind)
        fields[#fields+1]=string.format("actual_%s=%s actual_%s_available=%s expected_%s=%s expected_%s_available=%s",
            key,actual,key,tostring(aa),key,expected,key,tostring(ea))
    end
    local timestamp=actual_row and rawget(actual_row,"local_ms")
    local local_ms,la=box_playback.scalar(timestamp,"number")
    local at,na=box_playback.scalar(now,"number")
    local age,ga=box_playback.scalar(la and na and now-timestamp or nil,"number")
    box_log("BOXOBS_PLAYBACK reason=%s detail=%s peer=%s record=%d %s local_ms=%s local_ms_available=%s now=%s now_available=%s age_ms=%s age_available=%s authority=false",
        reason,detail,tostring(peer),box_playback.logs,table.concat(fields," "),local_ms,tostring(la),at,tostring(na),age,tostring(ga))
    return nil,reason,trace
end
function box_playback.check(peer,shown,pending)
    local ipc=rawget(_G,"HSMP_IPC")
    local ok,play,generation=pcall(function()
        local rows,gen
        if ipc then rows,gen=ipc.bus_table("playback")end
        for _,r in ipairs((rows or {}).rows or {})do if r.peer==peer then return r,gen end end
        return nil,gen
    end)
    local now=os.clock()*1000
    if not ok then return box_playback.refuse("playback_unavailable","read_failed",peer,shown,nil,now) end
    if not play then return box_playback.refuse("playback_unavailable","row_missing",peer,shown,nil,now,generation) end
    if type(play.local_ms)~="number" then return box_playback.refuse("playback_unavailable","local_ms_unavailable",peer,shown,play,now,generation) end
    -- Preserve the original predicate order/coercion; diagnostic availability is not admission.
    if now<play.local_ms then return box_playback.refuse("playback_timefuture","future",peer,shown,play,now,generation) end
    if now-play.local_ms>250 then
        if pending then
            -- Pending setup may observe a stale row, but cannot borrow a different life/actor tuple.
            for _,key in ipairs({"pawn","match_id","round","life"})do
                if play[key]~=shown[key] then return box_playback.refuse("playback_"..(key=="match_id" and "match" or key),
                    play[key]==nil and "field_missing" or "field_changed",peer,shown,play,now,generation) end
            end
        end
        return box_playback.refuse("playback_timeage","age_over250",peer,shown,play,now,generation)
    end
    for _,key in ipairs({"pawn","match_id","round","life"})do
        if play[key]~=shown[key] then
            return box_playback.refuse("playback_"..(key=="match_id" and "match" or key),
                play[key]==nil and "field_missing" or "field_changed",peer,shown,play,now,generation)
        end
    end
    return true,nil,box_playback.trace(play,now,generation)
end
local box_session,box_session_ipc
local function box_observer_live()
    if not HSESS then return false end
    local ipc=rawget(_G,"HSMP_IPC")
    if not ipc or not ipc.N or type(ipc.N.ipc_info)~="function" then return false end
    if not box_session then
        -- The shared facade may return cached healthy info even after a forced
        -- read fails. This diagnostic admission requires the current native header.
        box_session_ipc={rec=function(...)
            local current=rawget(_G,"HSMP_IPC")
            if current and current.rec then return current.rec(...) end
        end,refresh_info=function()
            local current=rawget(_G,"HSMP_IPC")
            local n=current and current.N
            if not n or type(n.ipc_info)~="function" then return nil end
            local ok,info=pcall(n.ipc_info)
            if not ok or type(info)~="table" or (info.sidecar_state~="ready" and info.sidecar_state~=2) then return nil end
            local age=info.sidecar_hb_age_s
            if type(age)~="number" or age~=age or age<0 or age>=math.huge then return nil end
            return info
        end}
        box_session=HSESS.new({ipc=box_session_ipc})
    end
    box_session_ipc.S=ipc.S
    box_session:poll(true)
    return box_session:live()==true
end
local function box_observer_snapshot(peer,side,pending)
    if not box_observer_live() then return nil,"session_start" end
    local own,source=diagnostic_snapshot(0)
    local shown,pawn,mesh=diagnostic_snapshot(peer)
    local view,mode=HSESS and HSESS.view(),HSESS and HSESS.mode()
    local row=mode and mode.rows and mode.rows[peer]
    local owner_row=mode and view and mode.rows and mode.rows[view.my_peer_id]
    if not own then return nil,"owner_context" end
    if own.placement_verified~="true" then return nil,"owner_unverified" end
    if not shown then return nil,"displayed_context" end
    if not row or not row.alive or row.respawning then return nil,"victim_mode" end
    if not owner_row or not owner_row.alive or owner_row.respawning then return nil,"owner_mode" end
    if not view or view.phase~=3 or not mode then return nil,"live_phase" end
    if own.match_id~=shown.match_id or own.round~=shown.round then return nil,"context_tuple" end
    local ipc=rawget(_G,"HSMP_IPC")
    local playback_ok,playback_reason,playback_trace=box_playback.check(peer,shown,pending==true)
    if not playback_ok and not (pending==true and playback_reason=="playback_timeage") then return nil,playback_reason,playback_trace,false end
    local stream,slot={},ipc and ipc.peer_slot and ipc.peer_slot(peer)
    if slot==nil or not ipc.peer_play then return nil,"peer_stream_unavailable" end
    ipc.peer_play(slot,stream)
    if stream.has_context~=true or stream.peer_id~=peer or stream.match_id~=shown.match_id
        or stream.round~=shown.round or stream.life~=shown.life then return nil,"peer_stream_context" end
    if stream.mode~="interp" and stream.mode~="extrap" then return nil,"peer_stream_mode" end
    if type(stream.age)~="number" or stream.age~=stream.age or math.abs(stream.age)>250 then return nil,"peer_stream_age" end
    local weapon=source[side=="l" and "Weapon L" or "Weapon R"]
    local grip=source[side=="l" and "L_GripType_Current" or "R_GripType_Current"]
    if not valid(weapon) then return nil,"held_weapon" end
    if type(grip)~="number" or grip<=0 or grip>=math.huge or grip%1~=0 then return nil,"held_grip" end
    local box=weapon["Hit Box Collision"]
    if not valid(box) then return nil,"held_box" end
    if not valid(box:GetOwner()) or box:GetOwner():GetAddress()~=weapon:GetAddress() then return nil,"box_owner" end
    local resolver=load_module("native_weapon_modules")
    local rows=resolver and resolver.of(weapon)
    local found=false
    for _,r in ipairs(rows or {})do if valid(r.component) and r.component:GetAddress()==box:GetAddress() then found=true end end
    if not found then return nil,rows and "box_membership" or "native_modules" end
    local world=WG.world()
    if not valid(world) then return nil,"world" end
    if not valid(mesh:GetOwner()) or mesh:GetOwner():GetAddress()~=pawn:GetAddress() then return nil,"mesh_owner" end
    for i,o in ipairs({source,pawn,weapon})do
        local w=o:GetWorld();if not valid(w) or w:GetAddress()~=world:GetAddress() then return nil,"actor_world_"..i end
    end
    local function ref(o)
        local full=o:GetFullName()
        local path=type(full)=="string" and full:match("^%S+ (/.+)$")
        if not path then return nil end
        return {path=path,address=o:GetAddress()}
    end
    local result={world=ref(world),pawn=ref(pawn),mesh=ref(mesh),box=ref(box),box_owner=ref(weapon),
        match_id=shown.match_id,round=shown.round,life=shown.life,world_key=WG.key,
        owner_life=own.life,owner_pawn=own.address,owner_mesh=own.mesh_address,peer=peer,side=side,grip=grip}
    for _,k in ipairs({"world","pawn","mesh","box","box_owner"})do
        if not result[k] then return nil,"identity_"..k end
    end
    local fresh_own=diagnostic_snapshot(0)
    local fresh_shown=diagnostic_snapshot(peer)
    if not DIAGNOSTIC_CONTEXT.same(own,fresh_own) or not DIAGNOSTIC_CONTEXT.same(shown,fresh_shown)
        or not valid(source[side=="l" and "Weapon L" or "Weapon R"])
        or source[side=="l" and "Weapon L" or "Weapon R"]:GetAddress()~=weapon:GetAddress()
        or not valid(weapon["Hit Box Collision"]) or weapon["Hit Box Collision"]:GetAddress()~=box:GetAddress()
        or source[side=="l" and "L_GripType_Current" or "R_GripType_Current"]~=grip then return nil,"revalidation" end
    if not box_observer_live() then return nil,"session_end" end
    -- A pending context-only snapshot is never eligibility when its original playback is stale.
    return result,playback_reason,playback_trace,playback_ok==true
end
local function exp_boxobserve(arg)
    if not box_observer then
        local m,log=load_module("native_box_observer"),load_module("hsmp_log")
        if not m or not log then box_log("BOXOBS unavailable dev observer/logger");return end
        box_observer=m.new({developer=function()return os.getenv("HSMP_DEV")=="1"end,
            native=function()local n=rawget(_G,"HSMPNative");return n and n.box_probe end,
            snapshot=box_observer_snapshot,clock_ms=function()return os.clock()*1000 end,register=RegisterHook,log=box_log,emit=function(row)
                -- FName/weak bits remain exact even beyond JSON's integer range.
                for _,k in ipairs({"world","pawn","mesh","box","box_owner","function"})do
                    for key,value in pairs(row[k] or {})do if type(value)=="number" then row[k][key]=tostring(value) end end
                end
                row.instance=box_instance
                box_log("BOXOBS_PAIR %s",log.encode(row))
            end})
    end
    box_observer.command(arg)
end
WG.on_drop(function()if box_observer then box_observer.drop()end end)
local function exp_frames(arg)
    local peer,bone=tostring(arg):match("^(%d+)%s*(%S*)")
    peer=tonumber(peer) or 0
    local probe,joints=load_module("body_frame_probe"),load_module("body_joint_dictionary")
    if not probe or not joints then Log("BODYFRAME refused probe/dictionary unavailable");return end
    local original,_,mesh=diagnostic_snapshot(peer)
    if not original then Log("BODYFRAME refused original context unavailable");return end
    local physics,library
    pcall(function()local c=StaticFindObject("/Script/Engine.PhysicsObjectBlueprintLibrary");if valid(c) then physics=c:GetCDO() end end)
    pcall(function()local c=StaticFindObject("/Script/Engine.ConstraintInstanceBlueprintLibrary");if valid(c) then library=c:GetCDO() end end)
    local allowed={pelvis=true,spine_01=true}
    for _,j in ipairs(joints)do allowed[j.parent],allowed[j.child]=true,true end
    local bones=bone and bone~="" and {bone} or {"lowerarm_r","hand_r","lowerarm_l","hand_l","neck_01","neck_02","head"}
    local result,why=probe.capture(mesh,original,bones,{physics=physics,constraint_library=valid(library) and library or nil,joints=joints,
        fname=FName,allowed_bone=function(n)return allowed[n]==true end,now=function()return os.clock()*1000 end,
        current=function(id,m)
            local fresh=diagnostic_snapshot(peer)
            return DIAGNOSTIC_CONTEXT.same(id,fresh) and m:GetAddress()==fresh.mesh_address and nm(m)==fresh.mesh
        end})
    if not result then Log("BODYFRAME refused %s",tostring(why));return end
    Log("BODYFRAME_CONTEXT peer=%s pawn=%s actor_address=%s mesh=%s mesh_address=%s match=%s round=%s life=%s world=%s display_time=%s placement_verified=%s read_only=true",
        tostring(peer),original.pawn,tostring(original.address),original.mesh,tostring(original.mesh_address),tostring(original.match_id),
        tostring(original.round),tostring(original.life),original.world,tostring(original.display_time),original.placement_verified)
    for _,r in ipairs(result.rows)do
        local s,p,a=r.socket,r.physics,r.joint_angles_deg
        local function q(v)return v and string.format("(%.5f,%.5f,%.5f,%.5f)",v[1],v[2],v[3],v[4]) or "unavailable" end
        Log("BODYFRAME peer=%s pawn=%s match=%s round=%s life=%s bone=%s socket_q=%s physics_q=%s mass=%s angles=%s physics_verified=false read_only=true",
            tostring(peer),result.pawn,tostring(result.match_id),tostring(result.round),tostring(result.life),r.bone,q(s and s.q),q(p and p.q),tostring(r.mass),
            a and string.format("(%.3f,%.3f,%.3f)",a.swing1,a.twist,a.swing2) or "unavailable")
        for k,v in pairs(r.errors)do Log("BODYFRAME_ERROR bone=%s field=%s reason=%s",r.bone,k,v) end
    end
    for _,r in ipairs(result.joints or {})do
        local c=r.current
        if c then Log("BODYJOINT peer=%s pawn=%s name=%s parent=%s child=%s swing1=%s swing2=%s twist=%s strength=%s damping=%s force=%s read_only=true",
            tostring(peer),result.pawn,r.name,c.parent,c.child,tostring(c.swing1_limit),tostring(c.swing2_limit),tostring(c.twist_limit),
            tostring(c.angular_strength),tostring(c.angular_damping),tostring(c.angular_force_limit)) end
        for k,v in pairs(r.errors)do Log("BODYJOINT_ERROR name=%s field=%s reason=%s",r.name,k,v) end
    end
end
-- Snapshot native simulation and grip endpoints without retaining any wrappers.
local function weapon_state_hand_current(original,pawn)
    if not box_observer_live() then return false,"session_current" end
    local fresh,actor,mesh=diagnostic_snapshot(original.peer)
    if not DIAGNOSTIC_CONTEXT.same(original,fresh) then return false,"context_current" end
    if not valid(pawn) or pawn:GetAddress()~=fresh.address or nm(pawn)~=fresh.pawn
        or not valid(actor) or actor:GetAddress()~=fresh.address then return false,"actor_current" end
    local world=WG.world()
    local native_world=pawn:GetWorld()
    if not valid(world) or not valid(native_world) or native_world:GetAddress()~=world:GetAddress()
        or tostring(world:GetAddress()).."@"..world:GetFullName()~=original.world then return false,"world_current" end
    local owner=mesh:GetOwner()
    if not valid(owner) or owner:GetAddress()~=fresh.address or nm(owner)~=fresh.pawn then return false,"mesh_owner_current" end
    -- All observations are synchronous. The same full identity and current
    -- native header are checked again after the six bounded field reads.
    if not box_observer_live() then return false,"session_recheck" end
    return true
end
local function exp_weaponstate(arg)
    local probe=load_module("weapon_state_probe")
    if not probe then Log("weaponstate: diagnostic module unavailable");return end
    local requested=tonumber(arg)
    if arg~="" and (not requested or requested<0 or requested%1~=0) then Log("weaponstate: expected [peer], 0=own");return end
    local ipc=rawget(_G,"HSMP_IPC")
    local peers={}
    if requested then peers[1]=requested
    else
        peers[1]=0
        for _,row in ipairs((ipc.bus_table("puppets") or {}).rows or {}) do peers[#peers+1]=row.peer end
    end
    for _,peer in ipairs(peers) do
        local original,pawn=diagnostic_snapshot(peer)
        if not original then Log("weaponstate: peer %s refused original verified context unavailable",tostring(peer))
        else
            local result,why=probe.capture(pawn,original,{fname=FName,class=function(name)return StaticFindObject("/Script/Engine."..name)end,
                instance=box_instance,hand_current=weapon_state_hand_current,
                current=function(id)
                    return DIAGNOSTIC_CONTEXT.same(id,diagnostic_snapshot(peer))
                end})
            if result then Log("WEAPONSTATE_CONTEXT peer=%s mesh=%s mesh_address=%s placement_verified=%s read_only=true",
                tostring(peer),original.mesh,tostring(original.mesh_address),original.placement_verified) end
            if result then probe.emit(result,Log) else Log("weaponstate: peer %s refused %s",tostring(peer),tostring(why)) end
        end
    end
end
-- `ai on [peer]` / `ai off`: hand my pawn to the game's own fighter AI (AI_BP_C, the solo
-- opponent) and point it at a stand-in. AI_BP_C acts only on the pawn it possesses (its
-- MoveToLocation, "My Pawn" from K2_GetPawn) and Willie_BP steers from "AI Control Rotation"
-- only while "Player" is false; AI init destroys a controller whose pawn is player 0's. So:
-- the PlayerController lets go, SpawnDefaultController() makes the class-default AI_BP_C
-- possess it, Player = false, Event Initialize AI, Event Get Into Combat State(target).
-- The mods find our pawn through WG.ai_pawn meanwhile (shared/hsmp_wg.lua). `off` reverses.
local AI_AUTO, AI_PENDING = nil, nil
local AI_READY = load_module("hsmp_spawn_ready")
local AI_PROOF = AI_READY and AI_READY.new()
local AI_INTENT
WG.on_drop(function() AI_INTENT = nil end) -- plain identity/state only; never retain controller wrappers
-- AI_BP_C ReceiveTick supplies new combat intent. Stop only its controller,
-- leaving Willie physics, damage continuations, pose and vitals running. These
-- native methods and the hard BrainComponent ObjectProperty are in the SDK.
local function ai_intent_tick()
    if not WG.settled() then AI_INTENT = nil; return end
    local me = WG.ai_pawn()
    local ipc = rawget(_G, "HSMP_IPC")
    local st = HW.verified_ai_status and HW.verified_ai_status(ipc)
    if not valid(me) or not st or st.pawn ~= nm(me) then AI_INTENT = nil; return end
    local c, pawn_address, controller_address
    pcall(function() c = me.Controller; pawn_address = me:GetAddress(); controller_address = c:GetAddress() end)
    if not valid(c) or not pawn_address or not controller_address then AI_INTENT = nil; return end
    local key = table.concat({ tostring(WG.key), tostring(st.match_id), tostring(st.round), tostring(st.life),
        tostring(st.spawn_id), st.pawn, tostring(pawn_address), tostring(controller_address) }, ":")
    if AI_INTENT and AI_INTENT.key ~= key then AI_INTENT = nil end
    local view, mode = HSESS and HSESS.view(), HSESS and HSESS.mode()
    local own = mode and view and mode.rows and mode.rows[view.my_peer_id]
    local live = SESS and SESS:live() and view and view.state == "live" and view.match_id == st.match_id
        and view.round == st.round and mode and mode.match_id == st.match_id and mode.round == st.round
        and own and own.life == st.life and own.alive ~= false and not own.respawning
    if live and not AI_INTENT then return end
    local now = os.clock()
    if AI_INTENT and AI_INTENT.live == live and now < AI_INTENT.retry_at then return end
    if not AI_INTENT then
        local ok, ticking = pcall(function() return c:IsActorTickEnabled() end)
        if not ok or type(ticking) ~= "boolean" then
            Log("AI_INTENT refused pawn=%s controller=%s reason=native controller tick state unavailable", st.pawn, nm(c)); return
        end
        AI_INTENT = { key=key, original_tick=ticking, retry_at=0 }
    end
    local state = AI_INTENT
    state.live = live
    local brain, brain_address
    pcall(function() brain = c.BrainComponent; if valid(brain) then brain_address = brain:GetAddress() end end)
    if live then
        local restart = "not_stopped"
        if state.brain_stopped then
            if valid(brain) and brain_address == state.brain_address then
                local ok = pcall(function() brain:RestartLogic() end)
                restart = ok and "submitted" or "failed"
                if ok then state.brain_stopped = nil end
            else state.brain_stopped = nil; restart = "identity_changed" end
        end
        local set_ok = not state.original_tick or pcall(function() c:SetActorTickEnabled(true) end)
        local read_ok, ticking = pcall(function() return c:IsActorTickEnabled() end)
        local restored = set_ok and read_ok and ticking == state.original_tick and not state.brain_stopped
        Log("AI_INTENT resume pawn=%s controller=%s match=%s round=%s life=%s tick=%s restored=%s brain_restart=%s",
            st.pawn, nm(c), tostring(st.match_id), tostring(st.round), tostring(st.life), tostring(ticking), tostring(restored), restart)
        if restored then AI_INTENT = nil else state.retry_at = now + .5 end
        return
    end
    if state.paused then return end
    local move_ok = pcall(function() c:StopMovement() end)
    local stop = "not_running"
    if not state.brain_stopped and valid(brain) and brain_address then
        local ok, running = pcall(function() return brain:IsRunning() end)
        if ok and running == true then
            local stopped = pcall(function() brain:StopLogic(FString("HSMP duel is not Live")) end)
            stop = stopped and "submitted" or "failed"
            if stopped then state.brain_stopped, state.brain_address = true, brain_address end
        elseif not ok then stop = "state_unavailable" end
    elseif not valid(brain) then stop = "unavailable"
    elseif state.brain_stopped then stop = "submitted" end
    local set_ok = pcall(function() c:SetActorTickEnabled(false) end)
    local read_ok, ticking = pcall(function() return c:IsActorTickEnabled() end)
    state.paused = move_ok and set_ok and read_ok and ticking == false and stop ~= "failed" and stop ~= "state_unavailable"
    state.retry_at = state.paused and 0 or now + .5
    Log("AI_INTENT pause pawn=%s controller=%s match=%s round=%s life=%s phase=%s movement_stop=%s tick=%s paused=%s brain_stop=%s",
        st.pawn, nm(c), tostring(st.match_id), tostring(st.round), tostring(st.life), tostring(view and view.state),
        tostring(move_ok), tostring(ticking), tostring(state.paused), stop)
end
local function ai_target(peer)
    if peer then return standin_of(peer) end
    local ipc = rawget(_G, "HSMP_IPC")
    local view, mode = HSESS and HSESS.view(), HSESS and HSESS.mode()
    local own = mode and view and mode.rows and mode.rows[view.my_peer_id]
    for _, row in ipairs((ipc and ipc.bus_table("puppets") or {}).rows or {}) do
        local remote = mode and mode.rows and mode.rows[row.peer]
        if own and remote and remote.alive ~= false and not remote.respawning
            and not (own.team and own.team > 0 and own.team == remote.team) then return standin_of(row.peer) end
    end
end
local function ai_native_ready(actor)
    local native, best, best_score = {}, nil, nil
    pcall(function() native.alive = actor.Health > 0 and actor.DED == false end)
    for _, field in ipairs({ "SK_Skeleton", "BoneCore", "DriverSkeleton", "Mesh" }) do
        pcall(function()
            local mesh = actor[field]
            if valid(mesh) then
                local sim = mesh:IsSimulatingPhysics(FName("Pelvis")) == true
                local score = (sim and 2 or 0) + (mesh:IsVisible() == true and 1 or 0)
                if best_score == nil or score > best_score then best, best_score = mesh, score end
            end
        end)
    end
    pcall(function()
        local collision = best and best:GetCollisionEnabled()
        native.collision = actor:GetActorEnableCollision() == true
            and best:IsSimulatingPhysics(FName("Pelvis")) == true and (collision == 2 or collision == 3)
    end)
    return native
end
local function ai_readiness(pc, me, si, peer)
    local ipc = rawget(_G, "HSMP_IPC")
    local st = HW.verified_ai_status and HW.verified_ai_status(ipc)
    local view, mode = HSESS and HSESS.view(), HSESS and HSESS.mode()
    if not st or st.pawn ~= nm(me) or not view or view.state ~= "live" then return nil, "own verified Live pawn" end
    local own = mode and mode.rows and mode.rows[view.my_peer_id]
    local remote = mode and mode.rows and mode.rows[peer]
    if not own or own.alive == false or own.respawning or not remote or remote.alive == false or remote.respawning
        or mode.match_id ~= st.match_id or mode.round ~= st.round then return nil, "current fighter life" end
    if own.team and own.team > 0 and own.team == remote.team then return nil, "target is a teammate" end
    if not AI_PROOF or not ipc.sample_status then return nil, "spawn proof unavailable" end
    local playback
    for _, row in ipairs((ipc.bus_table("playback") or {}).rows or {}) do if row.peer == peer then playback = row; break end end
    local source, slot = {}, ipc.peer_slot and ipc.peer_slot(peer)
    if slot ~= nil and ipc.peer_play then ipc.peer_play(slot, source) end
    local my_team, si_team
    pcall(function() my_team, si_team = me["Team Int"], si["Team Int"] end)
    if type(my_team) ~= "number" or type(si_team) ~= "number" or my_team == si_team then return nil, "native target team not initialized" end
    local sample = ipc.sample_status()
    local native_world
    pcall(function()
        local world = WG.world and WG.world()
        if valid(world) then native_world = tostring(world:GetAddress()) .. "@" .. world:GetFullName() end
    end)
    local ok, why = AI_PROOF:check({ world = WG.key, native_world = native_world, qualify_settle = true,
        now_ms = os.clock() * 1000, own = st,
        root = ipc.rec("local_root"), pose = sample and sample.pose, vitals = ipc.rec("vitals"), remotes = {
            { peer = peer, pawn = nm(si), match_id = st.match_id, round = st.round, life = remote.life,
                playback = playback, source = source, vitals = ipc.peer_rec("peer_vitals", peer), native = ai_native_ready(si) },
        } })
    if not ok then return nil, why end
    return st
end
local function ai_takeover(pc, me, si)
    local name = nm(me)
    pcall(function() pc:UnPossess() end)
    local ok, err = pcall(function() me:SpawnDefaultController() end)
    local c; pcall(function() c = me.Controller end)
    local cls = "none"; pcall(function() cls = c:GetClass():GetFName():ToString() end)
    if not (ok and valid(c) and cls == "AI_BP_C") then
        pcall(function() pc:Possess(me) end)
        Log("ai: refused - SpawnDefaultController gave %s (%s); control returned to the player", cls, tostring(err)); return
    end
    pcall(function() me.Player = false end)
    pcall(function() pc:SetViewTargetWithBlend(me, 0.0, 0, 0.0, false) end)
    local i1 = bp_call(c, "Event Initialize AI")
    local i2 = bp_call(c, "Event Get Into Combat State", 30.0, si, si:K2_GetActorLocation())
    Log("ai: %s now driven by %s (init=%s combat=%s) target %s team %s vs %s", name, nm(c), tostring(i1), tostring(i2), nm(si), tostring(me["Team Int"]), tostring(si["Team Int"]))
end
local function exp_ai(arg)
    local mode, peer = tostring(arg):match("^(%a+)%s*(%d*)$")
    local pc = WG.pc()
    if not valid(pc) then Log("ai: no PlayerController"); return end
    if mode == "on" then
        if valid(WG.ai_pawn()) then Log("ai: verified own fighter is already AI-driven"); return end
        AI_PENDING = { peer = tonumber(peer), world = WG.key }
        Log("ai: on requested; waiting for current fighter pose, vitals and collision")
    elseif mode == "off" then
        AI_AUTO, AI_PENDING = nil, nil
        AI_INTENT = nil
        if AI_PROOF then AI_PROOF:reset() end
        local me = WG.ai_pawn()
        if not me then Log("ai: no AI-driven pawn of ours"); return end
        local c; pcall(function() c = me.Controller end)
        pcall(function() c:UnPossess() end)
        pcall(function() c:K2_DestroyActor() end) -- unsafe: ok the AI_BP_C controller we spawned for our own pawn, never a Willie
        pcall(function() me.Player = true end)
        local ok = pcall(function() pc:Possess(me) end)
        Log("ai: %s back to the player (possess=%s)", nm(me), tostring(ok))
    elseif mode == "auto" then
        -- sticky: every round (a round reload makes a fresh pawn) is fought by the AI
        AI_AUTO = { tried = nil }
        AI_PENDING = nil
        Log("ai: auto - every Live round from now on is fought by the game's own AI")
    else
        Log("ai: expected on [peer] | off | auto")
    end
end
-- Bind a takeover to its verified world/match/round/life/pawn, never to a
-- temporary possession created by the native stand-in fallback.
local function ai_auto_tick()
    ai_intent_tick()
    if not (AI_AUTO or AI_PENDING) or not (SESS and SESS:live()) or not WG.settled() then return end
    if AI_PENDING and AI_PENDING.world ~= WG.key then AI_PENDING = nil end
    if not (AI_AUTO or AI_PENDING) or valid(WG.ai_pawn()) then return end
    local pc = WG.pc()
    local p = valid(pc) and pc.Pawn or nil
    if not valid(p) then return end   -- already AI-driven (or no pawn yet)
    local view = HSESS and HSESS.view and HSESS.view()
    if not (view and view.state == "live") then return end
    local si, peer = ai_target(AI_PENDING and AI_PENDING.peer)
    if not si then return end
    local context, why = ai_readiness(pc, p, si, peer)
    if not context then
        local owner = AI_PENDING or AI_AUTO
        if owner.wait ~= why then owner.wait = why; Log("ai: waiting for %s", tostring(why)) end
        return
    end
    local key = tostring(WG.key) .. ":" .. context.match_id .. ":" .. context.round .. ":" .. context.life .. ":" .. context.pawn
    if AI_AUTO and AI_AUTO.tried == key then return end
    if AI_AUTO then AI_AUTO.tried = key; AI_AUTO.wait = nil end
    AI_PENDING = nil
    ai_takeover(pc, p, si)
end
local driver
local function exp_drive(arg)
    if not driver then
        local m = load_module("duel_driver")
        if not m then Log("drive: duel_driver.lua missing"); return end
        driver = m.new({ log = Log, loop = LoopAsync, now = os.clock, world = function() return WG.key end,
            pawn = function() local pc = WG.pc(); return valid(pc) and pc or nil, me_pawn() end,
            standin = function() return standin_of(nil) end })
    end
    driver.start(arg)
end
local EXPS = { kit = exp_kit, spots = exp_spots, near = exp_near, swing = exp_swing, arm = exp_arm, bounds = exp_bounds, colliders = exp_colliders, modules=exp_modules, components=exp_components, inventory = exp_inventory, cutproxy=exp_cutproxy, fists=exp_fists, frames=exp_frames, weaponstate=exp_weaponstate, defeat=exp_defeat, drive=exp_drive, ai=exp_ai,boxobserve=exp_boxobserve }
if rawget(_G, "HSMP_PARITY_TEST") then
    HSMP_PARITY_TEST.arm = exp_arm
    HSMP_PARITY_TEST.state = function() return arm_drive end
    HSMP_PARITY_TEST.ai = exp_ai
    HSMP_PARITY_TEST.ai_tick = ai_auto_tick
    HSMP_PARITY_TEST.frames = exp_frames
    HSMP_PARITY_TEST.weaponstate = exp_weaponstate
    HSMP_PARITY_TEST.boxobserve = exp_boxobserve
end
local dev_out = {}
LoopAsync(33, function()
    if not WG.check() then return false end
    if not hooked then
        hooked = pcall(function()
            RegisterHook("/Game/Character/Blueprints/Willie_BP.Willie_BP_C:Deal Complex Damage", function(...) pcall(on_dcd, ...) end)
        end)
    end
    swing_tick()
    pcall(ai_auto_tick)
    cutproxy_tick()
    defeat_tick()
    if box_observer then box_observer.tick()end
    if inventory then inventory.tick(WG.key,WG.settled() and SESS and SESS:live()) end
    local ipc = rawget(_G, "HSMP_IPC")
    if not (ipc and ipc.dev_poll) then return false end
    for i = 1, ipc.dev_poll(8, dev_out) do
        local d = dev_out[i] and (dev_out[i].data or dev_out[i])
        if type(d) == "table" and d.op == 1 and d.key == "parity" then
            local exp, rest = tostring(d.arg or ""):match("^(%S+)%s*(.*)$")
            local fn = exp and EXPS[exp]
            if SESS then SESS:poll(true) end
            if not fn then Log("unknown experiment '%s'", tostring(d.arg))
            elseif not (SESS and SESS:live()) then Log("refused '%s': not in an MP session (the career save is only redirected there)", exp)
            elseif not WG.settled() then Log("refused '%s': world not settled", exp)
            else
                local ok, err = pcall(fn, rest)
                if not ok then Log("%s failed: %s", exp, tostring(err)) end
            end
        end
    end
    return false
end)

Log("loaded; state_dir=%s (dev_cmd autotest parity <kit|spots|near|swing|arm>)", STATE_DIR)
