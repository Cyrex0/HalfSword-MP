-- Native sampling / servo offline test (option F only): HSMPNative.sample_local against the mock UE4SS.dll's
-- fake engine (cpp/test/mock_ue4ss.cpp). Checks verification, the sampled values, byte
-- identity of the pose record with the Lua path (put_pose with the same numbers), the weak
-- pointer / world rules, and measures the native cost per sample.
-- With MOCK_SOFT=1 the mock adds a Soft* param: verification must refuse.
local N = assert(HSMPNative, "HSMPNative missing")
local function check(c, msg) if not c then error("FAIL: " .. msg, 2) end end
local function repr(v, d)
    d = d or 0
    if type(v) ~= "table" then return tostring(v) end
    if d > 3 then return "{...}" end
    local k = {}
    for x in pairs(v) do k[#k + 1] = x end
    table.sort(k, function(a, b) return tostring(a) < tostring(b) end)
    local p = {}
    for _, x in ipairs(k) do p[#p + 1] = tostring(x) .. "=" .. repr(v[x], d + 1) end
    return "{" .. table.concat(p, ",") .. "}"
end
local function same(a, b)
    if type(a) ~= type(b) then return false end
    if type(a) ~= "table" then return a == b end
    for k, v in pairs(a) do if not same(v, b[k]) then return false end end
    for k in pairs(b) do if a[k] == nil then return false end end
    return true
end

assert(N.ipc_open())
N.frame("w1")

local BONES = {
    "pelvis", "spine_01", "spine_02", "spine_03", "spine_04", "spine_05",
    "neck_01", "neck_02", "head",
    "clavicle_l", "upperarm_l", "lowerarm_l", "hand_l",
    "clavicle_r", "upperarm_r", "lowerarm_r", "hand_r",
    "thigh_l", "calf_l", "foot_l",
    "thigh_r", "calf_r", "foot_r",
}
local r, e = N.sample_local({})
check(r == nil and e == "not configured", "before config: " .. tostring(e))
assert(N.sample_config({
    bones = BONES, nobody = { spine_01 = true },
    flags = { "R_Guarding", "L_Guarding", "Any_Guarding", "Not A Flag" },
    scalars = { "All Body Tonus", "Stamina", "Missing Scalar" },
    ik = { "R Out End Pos", "Soft Thing" },
    grip_r = "R_GripType_Current", grip_l = "L_GripType_Current",
    aim = "Aim Vector", ctrl_rot = "Current Control Rotation",
    weapon_base = "Root Scene", weapon_tip = "TippyTipScene",
}))
local st = N.sample_status()
check(st.available == true and st.configured == true, "status " .. repr(st))

local mesh, pawn, weapon, hr = mock_world()
local a = {
    mesh = mesh, pawn = pawn, pose_tick = 1, pose_ts = 1000.5, dt = 16, k = 0, control = true,
    w1 = weapon, h1 = 1, t1 = 77, w2 = 0, h2 = 0, t2 = 0,
    root_pawn = pawn, root_tick = 1, root_ts = 1000,
    weapon_actor = weapon, weapon_tick = 1, weapon_ts = 1000, weapon_id = 5, weapon_held = 1,
}

if os.getenv("MOCK_SOFT") then
    local m, err = N.sample_local(a)
    check(m == nil and err and err:find("^disabled:") and err:find("Soft"), "Soft* param refused: " .. tostring(err))
    check(N.sample_status().why:find("Soft"), "status.why names the refusal")
    print("[harness_sample] Soft* param refused: " .. err)
    return
end

local mask, err = N.sample_local(a)
check(mask == 7 and err == nil, "sample_local mask " .. tostring(mask) .. " " .. tostring(err) .. " " .. repr(N.sample_status()))
st = N.sample_status()
check(st.verified == true and st.samples == 1, "verified " .. repr(st))

local _, root = N.get("local_root", -1)
check(root and root.pos[1] == 100 and root.pos[3] == 300 and root.vel[2] == 8 and root.tick == 1, "root " .. repr(root))
local _, wpn = N.get("local_weapon", -1)
check(wpn and wpn.weapon_id == 5 and wpn.held == 1 and wpn.pos[2] == 200, "weapon " .. repr(wpn))
local _, pose_native = N.get("local_pose", -1)
check(pose_native and pose_native.tick == 1, "pose " .. repr(pose_native))

-- The Lua path with the same numbers (what HSMPSync would have sampled) -> the same record.
local b = {}
for i, name in ipairs(BONES) do
    local n = mock_fname(name)
    local base = (i - 1) * 13
    local v = { n * 10, n + 1, -n, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0 }
    if name ~= "spine_01" then
        v[8], v[9], v[10] = n * 10 + 1, n, 2
        v[11], v[12], v[13] = n, 0.5, -1
    end
    for k = 1, 13 do b[base + k] = v[k] end
end
local px, py, pz = hr * 10 + 10, hr + 1, -hr
local w = { 1, 77, px, py, pz, 0, 0, 0.6, 0.8, px + 1, 0, 2, 0, 0.5, -1, 1, 2, 3, 4, 5, 6 }
local c = { 5, 3, 2, 0.75, 42.5 }
for i = 6, 37 do c[i] = 0 end
c[20], c[21], c[22] = 1, 0, 0
c[23], c[24] = 10, 20
c[25], c[26], c[27] = 5, 6, 7
assert(N.put_pose(1, 1000.5, 16, 0, b, w, c))
local _, pose_lua = N.get("local_pose", -1)
check(same(pose_native, pose_lua), "native pose == Lua-path pose\nnative " .. repr(pose_native) .. "\nlua    " .. repr(pose_lua))
print("[harness_sample] native sample == Lua path (root, weapon, pose record identical)")

-- A dead weapon is skipped (weak pointer), the rest is still written.
local w2 = {}
for k, v in pairs(a) do w2[k] = v end
mock_kill(weapon)
mask, err = N.sample_local(w2)
check(mask == 5 and err == "skip:weapon", "dead weapon: " .. tostring(mask) .. " " .. tostring(err))

-- World leave: nothing resolves until world_ready; then it verifies again.
assert(N.world_leaving())
local m3, e3 = N.sample_local(a)
check(m3 == nil and e3 == "world", "between leave and ready: " .. tostring(e3))
assert(N.world_ready("w2"))
w2.w1, w2.weapon_actor = 0, 0
mask, err = N.sample_local(w2)
check(mask == 5 and err == nil, "after world_ready: " .. tostring(mask) .. " " .. tostring(err))

-- Cost: the full pose sample (23 bones, 22 bodies, control), native side only (the mock's
-- ProcessEvent is a few instructions; in the game each call costs the engine's thunk).
local pa = { mesh = mesh, pawn = pawn, pose_tick = 2, pose_ts = 1, dt = 16, k = 0, control = true, w1 = 0, w2 = 0 }
for _ = 1, 200 do N.sample_local(pa) end
local _, _, _, _, pe0 = mock_world()
local n = 5000
local t0 = now_us()
for i = 1, n do pa.pose_tick = i; N.sample_local(pa) end
local us = (now_us() - t0) / n
local _, _, _, _, pe1 = mock_world()
print(string.format("TIMING sample_local pose (23 bones + control)  %8.3f us/sample, %d ProcessEvent calls/sample", us, (pe1 - pe0) // n))
print(string.format("TIMING native status avg %.3f us", N.sample_status().avg_us))
-- Native servo: the stand-in body loop (22 bodies: GetSocketTransform + 2 velocity sets each).
assert(N.servo_config({ bones = BONES }))
local aim, com = {}, {}
for i = 1, 23 do
    if i ~= 2 then
        local n = mock_fname(BONES[i])
        com[i] = { 1, 2, 3 }
        aim[i] = { n * 10 + 5, n + 2, -n + 1, 0, 0, 0.1, 0.99498743710662, 10, 20, 30, 1, 2, 3 }
    end
end
local sa, so = { mesh = mesh, dt = 1 / 60, cap_lin = 900, cap_ang = 900, gain = 0.8, leg_gain = 0.5, aim = aim, com = com }, {}
local nd, se = N.servo_bodies(sa, so)
check(nd == 22, "servo_bodies drives 22 bodies: " .. tostring(nd) .. " " .. tostring(se) .. " " .. repr(N.servo_status()))
local pel = mock_fname("pelvis")
check(so.c[1] == pel * 10 and so.c[7] == 1 and #so.v >= 3, "servo out arrays " .. repr(so.c[1]))
for _ = 1, 200 do N.servo_bodies(sa, so) end
local _, _, _, _, s0 = mock_world()
t0 = now_us()
for _ = 1, n do N.servo_bodies(sa, so) end
us = (now_us() - t0) / n
local _, _, _, _, s1 = mock_world()
print(string.format("TIMING servo_bodies (22 bodies)  %8.3f us/stand-in frame, %d ProcessEvent calls", us, (s1 - s0) // n))
-- Native neutralise (26 BP floats + 2 bools + the motor call).
local zero = {}
for i = 0, 25 do zero[#zero + 1] = string.format("Neut%02d", i) end
assert(N.neutralise_config({ zero = zero, set_true = { "L_Guarding" }, set_false = { "R_Guarding" } }))
local na = { actor = pawn, motors_off = true, mesh = mesh }
local nw, ne = N.neutralise(na)
check(nw == 29, "neutralise writes 26 + 2 + motors: " .. tostring(nw) .. " " .. tostring(ne) .. " " .. repr(N.neutralise_status()))
for _ = 1, 200 do N.neutralise(na) end
t0 = now_us()
for _ = 1, n do N.neutralise(na) end
us = (now_us() - t0) / n
print(string.format("TIMING neutralise (28 BP variables + motors)  %8.3f us/stand-in frame", us))
-- Held-weapon servo: one held weapon (GetTransform + the two sets on its root).
mock_kill(weapon, false)   -- alive again (killed for the weak-pointer check above)
local _, _, _, _, _, wroot = mock_world()
local wa, wo = { actor = weapon, root = wroot, aim = { 5, 6, 7, 0, 0, 0.1, 0.99498743710662, 10, 20, 30, 1, 2, 3 }, com = { 1, 2, 3 },
    dt = 1 / 60, cap_lin = 900, cap_ang = 900, gain = 0.8 }, {}
local wok, werr = N.servo_weapon(wa, wo)
check(wok == true and #wo.x == 7 and #wo.v == 3, "servo_weapon: " .. tostring(werr))
for _ = 1, 200 do N.servo_weapon(wa, wo) end
t0 = now_us()
for _ = 1, n do N.servo_weapon(wa, wo) end
us = (now_us() - t0) / n
print(string.format("TIMING servo_weapon (1 weapon)  %8.3f us", us))
print("[harness_sample] PASS")
