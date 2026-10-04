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
--   spots [deg]    the same blow on my own pawn three ways, per bone and spot:
--                  solo (the spot itself), "world" (the spot a world-space offset
--                  lands on when the victim turned `deg`, default 70, since the
--                  attacker saw it: HSMPCombat before the bone-space fix) and "local" (the
--                  bone-space offset carried over: HSMPCombat now). Every field
--                  is put back after each blow.
--   near <peer>    stand 85 uu in front of peer's stand-in, facing it.
--   swing <speed>  push my held weapon at the nearest stand-in's chest at
--                  <speed> uu/s for three frames (a real physics blow through
--                  the whole MP path; HSMPCombat logs the claim).
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
local function nm(o) local s; pcall(function() s = o:GetFName():ToString() end); return s or "?" end
local function num(x) return tonumber(x) or 0 end
local function vec(v) local t; pcall(function() t = { v.X, v.Y, v.Z } end); return t or { 0, 0, 0 } end
local function V(t) return { X = t[1], Y = t[2], Z = t[3] } end
local function f3(t) return string.format("(%.1f,%.1f,%.1f)", t[1], t[2], t[3]) end
local function me_pawn()
    local pc = WG.pc()
    if not valid(pc) then return nil end
    local p = pc.Pawn
    return valid(p) and p or nil
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
    pcall(function() add("team", w["Team Int"]) end)
    pcall(function() add("invuln", w.Invulnerable) end)
    pcall(function() add("mass_scale_bp", string.format("%.3f", num(w["Mass Scale (Set in BP)"]))) end)
    pcall(function() add("armour_weight", string.format("%.2f", num(w["Armor Weight Body"]))) end)
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
        end)
    end)
    table.sort(tags)
    add("proxies", #tags .. "{" .. table.concat(tags, " | ") .. "}")
    local ms = {}
    pcall(function()
        for _, b in ipairs(MASS_BONES) do
            local m = w.Mesh:GetMassScale(FName(b))
            local bm = w.Mesh:GetBoneMass(FName(b), true)
            ms[#ms + 1] = string.format("%s=%.3f/%.2fkg", b, num(m), num(bm))
        end
    end)
    add("mass", table.concat(ms, " "))
    Log("KIT %s %s %s", label, nm(w), table.concat(parts, " "))
end
local function exp_kit()
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
    if captured > 400 then return end
    local function g(p) local v; pcall(function() v = p:get() end); return v end
    local w = g(selfp)
    local b = "?"; pcall(function() b = g(bone):ToString() end)
    local c = g(cc)
    local owner = "?"; pcall(function() owner = c:GetOwner():GetClass():GetFName():ToString() end)
    local cv, wv = { 0, 0, 0 }, { 0, 0, 0 }
    pcall(function() cv = vec(c:GetPhysicsLinearVelocity(FName("None"))) end)
    pcall(function() wv = vec(g(hc):GetPhysicsLinearVelocity(g(bone))) end)
    local rel = math.sqrt((cv[1] - wv[1]) ^ 2 + (cv[2] - wv[2]) ^ 2 + (cv[3] - wv[3]) ^ 2)
    local function L(p) local t = vec(g(p)); return math.sqrt(t[1] ^ 2 + t[2] ^ 2 + t[3] ^ 2) end
    Log("DCD on %s bone=%s by %s: |vel|=%.0f |imp|=%.0f rel=%.0f cut=%.1f stab=%.2f rig=%.2f kick=%.1f",
        nm(w), b, owner, L(vel), L(imp), rel, num(g(cp)), num(g(stab)), num(g(rig)), num(g(kick)))
end
local hooked = false

-- ---- command channel -------------------------------------------------------------------------
local EXPS = { kit = exp_kit, spots = exp_spots, near = exp_near, swing = exp_swing }
local dev_out = {}
LoopAsync(33, function()
    if not WG.check() then return false end
    if not hooked then
        hooked = pcall(function()
            RegisterHook("/Game/Character/Blueprints/Willie_BP.Willie_BP_C:Deal Complex Damage", function(...) pcall(on_dcd, ...) end)
        end)
    end
    swing_tick()
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

Log("loaded; state_dir=%s (dev_cmd autotest parity <kit|spots|near|swing>)", STATE_DIR)
