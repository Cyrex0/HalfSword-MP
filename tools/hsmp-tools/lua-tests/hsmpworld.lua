-- Parse-check every HSMP Lua mod with Lua 5.4 and unit-test the pure logic of
-- HSMPWorld (ids, codecs, manifest binding) without the game.
--
--     hsmp-tools lua-test hsmpworld

local T = T
local MODS = T.path("mods")
local WORLD = MODS .. "/HSMPWorld/Scripts/main.lua"
local function rel(p) return p:sub(#T.root + 2) end

-- ---- 1. every mod compiles under Lua 5.4 --------------------------------------
for _, path in ipairs(T.glob(MODS, "*/Scripts/*.lua")) do
    local f, err = load(T.read(path), "@" .. rel(path))
    T.check(f ~= nil, "parse " .. rel(path) .. ": " .. tostring(err))
end

-- ---- 2. load HSMPWorld in test mode ------------------------------------------
package.preload["UEHelpers"] = function() return {} end
-- shared/hsmp_wg.lua (HSMPWorld's world guard; deploy copies it into Scripts/)
package.path = MODS .. "/shared/?.lua;" .. package.path
HSMP_WORLD_TEST = true
function FName(s) return s end
local W = assert(load(T.read(WORLD), "@" .. rel(WORLD)))()
T.check(W ~= nil, "HSMPWorld returns its test table when HSMP_WORLD_TEST is set")

local function fnv1a(s)
    local h = 2166136261
    for i = 1, #s do
        h = h ~ s:byte(i)
        h = (h * 16777619) & 0xFFFFFFFF
    end
    return h
end

for _, s in ipairs({ "", "a", "ModularWeaponBP_C|W", "World /Game/Maps/Arenas/Map_Arena_Alley.Map_Arena_Alley" }) do
    T.check(W.fnv1a(s) == fnv1a(s), string.format("fnv1a(%q)", s))
end

-- UE FRotator::Quaternion (same as proto.rs wcodec::rot_to_quat).
local function rot_to_quat(p, y, r)
    local h = math.pi / 360
    local sp, cp = math.sin(p * h), math.cos(p * h)
    local sy, cy = math.sin(y * h), math.cos(y * h)
    local sr, cr = math.sin(r * h), math.cos(r * h)
    return cr * sp * sy - sr * cp * cy, -cr * sp * cy - sr * cp * sy,
        cr * cp * sy - sr * sp * cy, cr * cp * cy + sr * sp * sy
end

-- Python's (a - b + 540) % 360 - 180 (Lua % on floats is also floored).
local function adiff(a, b) return math.abs((a - b + 540) % 360 - 180) end

math.randomseed(7)
local function uniform(a, b) return a + (b - a) * math.random() end
local function randint(a, b) return math.random(a, b) end

local worst = 0.0
for _ = 1, 2000 do
    local p, y, r = uniform(-89, 89), uniform(-180, 180), uniform(-180, 180)
    local rot = W.quat_to_rot(rot_to_quat(p, y, r))
    worst = math.max(worst, adiff(p, rot.Pitch), adiff(y, rot.Yaw), adiff(r, rot.Roll))
end
T.check(worst < 1e-6, string.format("quat_to_rot inverts rot_to_quat (worst %s deg)", worst))
local g = W.quat_to_rot(rot_to_quat(90, 30, 0))
T.check(math.abs(g.Pitch - 90) < 1e-3, "gimbal pitch +90 handled")

local q0, q1 = { 0, 0, 0, 1 }, { rot_to_quat(0, 90, 0) }
local mid = W.nlerp(q0, q1, 0.5)
local n = math.sqrt(mid[1] ^ 2 + mid[2] ^ 2 + mid[3] ^ 2 + mid[4] ^ 2)
T.check(math.abs(n - 1) < 1e-9, "nlerp normalised")
T.check(adiff(W.quat_to_rot(mid[1], mid[2], mid[3], mid[4]).Yaw, 45) < 1e-6, "nlerp halfway = 45 deg yaw")
local neg = T.map({ rot_to_quat(0, 90, 0) }, function(c) return -c end)
local mid2 = W.nlerp(q0, neg, 0.5)
T.check(adiff(W.quat_to_rot(mid2[1], mid2[2], mid2[3], mid2[4]).Yaw, 45) < 1e-6, "nlerp takes the short arc")

T.check(T.truthy(W.stable_name("StaticMeshActor_12")) and T.truthy(W.stable_name("BP_Candle_C_3"))
    and T.truthy(W.stable_name("Chain")), "level-placed names are stable")
T.check(not T.truthy(W.stable_name("ModularWeaponBP_C_2147481324")) and not T.truthy(W.stable_name("Default__X")),
    "runtime / CDO names are not stable")

-- ---- 3. deterministic ids -------------------------------------------------------
local function cand(cls, comp, name, x, y, z, lp)
    return { cls = cls, comp = comp, name = name, levelpath = lp or "Level /Game/Maps/A.A:PersistentLevel",
             pos = { X = x, Y = y, Z = z } }
end

local base = {
    cand("ModularWeaponBP_C", "W", "ModularWeaponBP_C_2147480001", 100, 200, 30),
    cand("ModularWeaponBP_C", "W", "ModularWeaponBP_C_2147480002", 104, 203, 31),   -- same cell, other body
    cand("BP_Weapon_Tool_Shovel_A_C", "W", "BP_Weapon_Tool_Shovel_A_C_5", 0, 0, 0),
    cand("StaticMeshActor", "SM", "StaticMeshActor_7", 10, 10, 10),
    cand("StaticMeshActor", "SM", "StaticMeshActor_7", 900, 10, 10, "Level /Game/Maps/B.B:PersistentLevel"),
    cand("BP_Barrel_Destructable_Constraits_C", "Stave3", "BP_Barrel_Destructable_Constraits_C_1", 5, 5, 5),
    cand("BP_Barrel_Destructable_Constraits_C", "Stave4", "BP_Barrel_Destructable_Constraits_C_1", 5, 5, 5),
}

local function copy(t) local c = {}; for k, v in pairs(t) do c[k] = v end; return c end

local function ids_for(order, names_differ)
    local cs = {}
    for _, c in ipairs(order) do
        local d = copy(c)
        if names_differ and not T.truthy(W.stable_name(d.name)) then
            d.name = d.name:sub(1, -5) .. tostring(randint(1000, 9999))   -- runtime names differ per client
        end
        cs[#cs + 1] = d
    end
    local out = W.assign_ids(cs, {})
    local m = {}
    for i = 1, #cs do
        local o = out[i]
        m[tostring(o.cls) .. "\0" .. tostring(o.comp) .. "\0" .. tostring(o.tie) .. "\0" .. tostring(o.levelpath)] = o.lid
    end
    return m
end

local ref = ids_for(base)
for k = 0, 19 do
    local order = copy(base)
    for i = #order, 2, -1 do
        local j = math.random(1, i)
        order[i], order[j] = order[j], order[i]
    end
    T.check(T.eq(ids_for(order, true), ref), string.format("ids identical across clients (shuffle %d)", k))
end
local vals, distinct, in_range = {}, {}, true
for _, v in pairs(ref) do
    vals[#vals + 1] = v
    distinct[v] = true
    if not (0 < v and v < 2 ^ 31) then in_range = false end
end
T.check(T.len(distinct) == #base, "all ids distinct (same-cell twins get ordinals)")
T.check(in_range, "static ids stay out of the dynamic namespace (high bit)")
local taken = {}
local first = W.assign_ids({ copy(base[4]) }, taken)[1].lid
local again = W.assign_ids({ copy(base[4]) }, taken)[1].lid
T.check(first ~= again, "a later scan never reuses a taken id")

-- ---- 4. typed records (crates/hsmp-ipc/src/schema/world.rs) ------------------
-- The canonical quantisation (W2.qobj) against the Rust golden vectors
-- (schema/world.rs tests::quantisation_golden_vectors; Lua computes in doubles, so a
-- component may differ by one unit at a rounding edge).
local Q = W.W2
local GOLD = {
    { rot = { 0, 0, 0 }, vel = { 0, 0, 0 }, flags = 0, want_rot = { 0, 0, 0 }, want_vel = { 0, 0, 0 }, want_flags = 3 << 6 },
    { rot = { 0, 90, 0 }, vel = { 100.4, -0.5, 0.6 }, flags = 8, want_rot = { 0, 0, 32767 }, want_vel = { 100, 0, 1 }, want_flags = 8 | (2 << 6) },
    { rot = { 10, 20, 30 }, vel = { 5, -6, 1e9 }, flags = 2, want_rot = { -11089, -5917, 6714 }, want_vel = { 5, -6, 32767 }, want_flags = 2 | (3 << 6) },
    { rot = { -45, 170, -120 }, vel = { 0, 0, 0 }, flags = 1 | 0x30, want_rot = { -5602, 19986, 17165 }, want_vel = { 0, 0, 0 }, want_flags = 1 | (1 << 6) },
}
for i, g in ipairs(GOLD) do
    local o = Q.qobj(7, { X = 0, Y = 0, Z = 0 }, { Pitch = g.rot[1], Yaw = g.rot[2], Roll = g.rot[3] },
                     { X = g.vel[1], Y = g.vel[2], Z = g.vel[3] }, g.flags)
    local ok = o.flags == g.want_flags and T.eq(o.vel, g.want_vel)
    for k = 1, 3 do if math.abs(o.rot[k] - g.want_rot[k]) > 1 then ok = false end end
    T.check(ok, "qobj golden vector " .. i, T.repr(o))
end
local o9 = Q.qobj(9, { X = 0 / 0, Y = 1e9, Z = 1.26 }, { Pitch = 0 / 0, Yaw = 0, Roll = 0 }, { X = 0 / 0, Y = -1e9, Z = 3 }, 0xFF)
T.check(o9.pos[1] == 0 and o9.pos[3] == 1.3 and T.eq(o9.vel, { 0, -32767, 3 }) and o9.flags & 0x30 == 0,
    "qobj: non-finite -> 0, 0.1 cm positions, clamped velocities, only WF_USER flags", T.repr(o9))
-- round trip: unpack(pack(rot)) is the same orientation (angle from the chord |a -+ b|)
local qworst = 0
for _ = 1, 500 do
    local p, y, r = uniform(-89, 89), uniform(-180, 180), uniform(-180, 180)
    local a = { rot_to_quat(p, y, r) }
    local o = Q.qobj(1, { X = 0, Y = 0, Z = 0 }, { Pitch = p, Yaw = y, Roll = r }, nil, 0)
    local b = Q.unpack_q16(o.rot, o.flags)
    local c1, c2 = 0, 0
    for k = 1, 4 do c1 = c1 + (a[k] - b[k]) ^ 2; c2 = c2 + (a[k] + b[k]) ^ 2 end
    local ch = math.min(math.sqrt(c1), math.sqrt(c2), 2)
    qworst = math.max(qworst, math.deg(4 * math.asin(ch / 2)))
end
T.check(qworst < 0.01, "smallest-three i16 round trip (worst " .. qworst .. " deg)")

-- Readers: the sidecar's records (the typed mock's table shape) -> the logic's shapes.
local ow = Q.owners_from({ level = 5, epoch = 3, sync = true, manifest_len = 12, n = 2,
    rows = { { id = 10, owner = 2, ver = 7, mode = 2 }, { id = 11, owner = 0, ver = 9, mode = 0 } } })
T.check(ow.level == 5 and ow.epoch == 3 and ow.sync == true and ow.mlen == 12, "owners head")
T.check(ow.o[10].owner == 2 and ow.o[10].mode == 2 and ow.o[11].ver == 9, "owners rows")
T.check(Q.owners_from({ level = 5, epoch = 4, sync = false, manifest_len = 0, rows = {} }).sync == false, "sync=false")
local m = Q.manifest_from({ level = 5, epoch = 3, rows = { { id = 10, chash = 99, pos = { 1.5, -2.0, 3.0 } } } },
    { level = 5, epoch = 3, rows = { { id = 2147549185, chash = 7, pos = { -1000, 0, 5.5 }, dyn_owner = 1, class_path = "/Game/A/B.B_C" } } })
T.check(m.e[1].id == 10 and m.e[1].chash == 99 and m.e[1].pos.Y == -2.0 and m.e[1].dyn == 0, "manifest static row")
T.check(m.e[2].id == 2147549185 and m.e[2].dyn == 1 and m.e[2]["class"] == "/Game/A/B.B_C" and m.e[2].pos.X == -1000,
    "manifest dynamic row (world_dyn)")
local m2 = Q.manifest_from({ level = 5, epoch = 3, rows = {} }, { level = 5, epoch = 2, rows = { { id = 1, chash = 1, pos = { 0, 0, 0 }, dyn_owner = 1, class_path = "x" } } })
T.check(#m2.e == 0, "dynamic entries of another epoch are ignored")
local yaw90 = Q.qobj(10, { X = 1, Y = 2, Z = 3 }, { Pitch = 0, Yaw = 90, Roll = 0 }, { X = 4, Y = 5, Z = 6 }, 2)
local r = Q.remote_from({ level = 5, epoch = 3, rows = {
    { sender = 2, seq = 3, ts = 4, obj = yaw90 },
    { sender = 0, seq = 0, ts = 0, obj = Q.qobj(11, { X = 7, Y = 7, Z = 7 }, { Pitch = 0, Yaw = 0, Roll = 0 }, nil, 9) } } })
T.check(r.rows[1].id == 10 and r.rows[1].sender == 2 and r.rows[1].ts == 4 and r.rows[1].flags == 2
    and math.abs(r.rows[1].q[3] - math.sqrt(0.5)) < 1e-4 and math.abs(r.rows[1].q[4] - math.sqrt(0.5)) < 1e-4
    and r.rows[1].vel.Z == 6, "remote sample row (quaternion unpacked)", T.repr(r.rows[1]))
T.check(r.rows[2].sender == 0 and r.rows[2].flags == 9 and r.rows[2].q[4] > 0.9999, "remote anchor row")
local v = Q.consistency_from({ level = 5, epoch = 3, seq = 9, other = 2, compared = 10, mismatched_n = 1, hash_match = false,
    hash_equal = false, rows = { { id = 77, kind = 1, dpos = 12.5, dang = 4 } } })
T.check(v.peer == 2 and v.seq == 9 and v.hash_match == false and v.rows[1].id == 77 and v.rows[1].dpos == 12.5, "verdict")
-- Every WorldObj HSMPWorld builds is a valid record for the native module (the mock marshals by the schema).
do
    local Rm = dofile(T.path("tools/hsmp-tools/lua-tests/lib/hsmp_native_records.lua"))
    local S = dofile(T.path("mods/shared/hsmp_ipc_schema.lua"))
    local t, err = Rm.marshal(S, "world_state", { level = 1, epoch = 1, seq = 1, ts = 1, rows = { yaw90, o9 } })
    T.check(t ~= nil and t.n == 2 and t.rows[1].rot[3] == yaw90.rot[3], "a world_state of qobj rows marshals", tostring(err))
end

-- ---- 5. manifest binding -----------------------------------------------------------
local function rec(lid, chash, x, kind)
    return { lid = lid, chash = chash, kind = kind or "weapon", dyn_owner = 0, spawn_pos = { X = x, Y = 0, Z = 0 } }
end

local objs = { rec(1, 50, 0), rec(2, 50, 1000), rec(3, 60, 2000), rec(4, 70, 3000, "prop") }
local lt = objs
local by_lid = {}
for _, ob in ipairs(objs) do by_lid[ob.lid] = ob end
local by_nid = {}

local function ent(i, ch, x, dyn, cls)
    return { id = i, chash = ch, pos = { X = x, Y = 0, Z = 0 }, dyn = dyn or 0, class = cls or "" }
end

local c = W.bind_manifest({ ent(1, 50, 0), ent(99, 50, 1030), ent(77, 61, 2010), ent(55, 70, 9000),
                            ent(2147549185, 50, 0, 1, "/Game/X.X_C") }, lt, by_lid, by_nid)
T.check(c.exact == 1 and objs[1].nid == 1, "exact bind")
T.check(c.alias == 1 and objs[2].nid == 99, "straddled cell binds by class+distance (alias)")
T.check(c.mismatch == 1 and objs[3].nid == 77, "weapon with another class at the same spot binds (mismatch)")
T.check(c.unmatched == 1 and objs[4].nid == nil, "far entry stays unmatched; props never class-mismatch bind")
T.check(#c.dyn == 1 and c.dyn[1].id == 2147549185, "dynamic entries handed back for spawning")
local c2 = W.bind_manifest({ ent(1, 50, 0), ent(99, 50, 1030) }, lt, by_lid, by_nid)
T.check(c2.exact == 0 and c2.alias == 0, "re-binding is idempotent")

-- ---- session debounce, re-arm window, settle --------------------------------
-- A second HSMPWorld instance with its own state dir (files are real here).
local SD2 = T.tmpdir("hsmpworld_fw1_")
local real_getenv = os.getenv
os.getenv = function(k) if k == "HSMP_STATE_DIR" then return SD2 end return real_getenv(k) end
local W2 = assert(load(T.read(WORLD), "@" .. rel(WORLD)))()
os.getenv = real_getenv

-- The world state follows a debounced connection.
-- Typed session state: the sidecar's `link` record, the server's `session`
-- record and HSMPSync's `spawn_status` bus record in the native mock; the header
-- heartbeat makes a sidecar live.
local NAT = rawget(_G, "HSMPNative")
local IPCF = rawget(_G, "HSMP_IPC")
local ES = IPCF.S.ENUMS
local function hb(age) NAT._st.hb_age = age; IPCF.refresh_info(true) end
local function sidecar(status, id) NAT.sc_put("link", { status = ES.sidecar_status[status:upper()], state = ES.link_state.UP, my_peer_id = id }) end
local PHASE = { lobby = 0, countdown = 2, live = 3, roundover = 4, match_over = 5, paused = 7 }
local function match(state, round) NAT.sc_put("session", { phase = PHASE[state], round = round, winner_seat = 255 }) end
local ss_seq = 0
local function spawn_status(pawn, protect_until)
    ss_seq = ss_seq + 1
    IPCF.bus_put("spawn_status", { seq = ss_seq, pawn = pawn, has_protect_until = protect_until ~= nil, protect_until = protect_until or 0 })
end
hb(1e9)
sidecar("connected", 2)
W2.refresh_session(0)
T.check(W2.sess.connected == false, "a 'connected' link without a sidecar heartbeat is not connected")
hb(0.01)
W2.refresh_session(1000)
T.check(W2.sess.live == true and W2.sess.my_id == 2, "connected -> live")
sidecar("connected", 2)
W2.refresh_session(2000)
T.check(W2.sess.live == true and W2.sess.connected == true, "an unchanged re-publish: no change at all")
sidecar("reconnecting", 2)
W2.refresh_session(3000)
T.check(W2.sess.connected == false and W2.sess.live == true, "a reconnecting blip is debounced (still live)")
sidecar("connected", 2)
W2.refresh_session(4000)
T.check(W2.sess.live == true and W2.sess.down_since == nil, "back before the debounce: never left")
sidecar("reconnecting", 2)
W2.refresh_session(5000); W2.refresh_session(6500)
T.check(W2.sess.live == true, "1.5 s down: still live")
W2.refresh_session(8100)
T.check(W2.sess.live == false, ">= 3 s down: not live (the world state is released)")

-- A quick reconnect (new peer id, well inside the debounce) is a new
-- session: W is released and dropped, the next tick re-syncs.
sidecar("connected", 2)
W2.refresh_session(9000)
T.check(W2.sess.live == true and W2.sess.id_changed == nil, "same id back: no session change")
local WN = "World /Game/Maps/Arenas/Map_Arena_Alley.Map_Arena_Alley"
W2.new_level(WN, 0x77, 9000)
W2.W().synced = true
sidecar("reconnecting", 2)
W2.refresh_session(9500)
sidecar("connected", 5)
W2.refresh_session(10000)
T.check(W2.sess.live == true and W2.sess.id_changed and W2.sess.id_changed.from == 2 and W2.sess.id_changed.to == 5,
    "peer id 2 -> 5 inside the debounce: flagged as a session change", T.repr(W2.sess.id_changed))
T.check(W2.apply_session_change(WN, 0x77) == true and W2.W() == nil and W2.sess.id_changed == nil,
    "the replicated world is dropped (a fresh W re-syncs next tick)")
T.check(W2.apply_session_change(WN, 0x77) == false, "applied once")
T.check(W2.W() == nil, "world not ready until re-synced (a fresh W)")

-- The kit re-arm window (countdown / spawn protection) is tracked so
-- a drop inside it is not registered as a world item.
match("countdown", 2)
spawn_status("W", nil)
sidecar("connected", 5)
W2.refresh_session(12000)
T.check(W2.sess.match_state == "countdown" and W2.sess.protected == true, "re-arm window seen (countdown, protect_until null)")
match("live", 2)
spawn_status("W", os.clock() - 1)
W2.refresh_session(13000)
T.check(W2.sess.match_state == "live" and W2.sess.protected == false, "live and protection over: drops register again")
T.check(T.contains(T.read(WORLD), "during the kit re-arm window"), "track_my_items skips drops in the window")

-- A "connected" link whose sidecar stopped beating (dead sidecar) is not a
-- session: after the debounce the world state is released.
do
    sidecar("connected", 5)
    W2.refresh_session(20000)
    T.check(W2.sess.connected == true and W2.sess.live == true, "fresh heartbeat: connected")
    hb(7)
    W2.refresh_session(21000)
    T.check(W2.sess.connected == false, "a 'connected' link whose sidecar stopped beating 7 s ago: not connected (dead sidecar)")
    W2.refresh_session(24500)
    T.check(W2.sess.live == false, "... and after the 3 s debounce not live (no SP world sync)")
    hb(0.01)
end

-- Non-finite numbers never reach the JSON files
T.check(W2.F(0 / 0) == 0 and W2.F(math.huge) == 0 and W2.F(-math.huge) == 0 and W2.F(12.5) == 12.5, "F() sanitises NaN / inf")

-- The init "settled" test compares against a sample >= INIT_SAMPLE_MS old
local o = { pos = { X = 0, Y = 0, Z = 100 }, rot = { Pitch = 0, Yaw = 0, Roll = 0 }, vel = { X = 0, Y = 0, Z = -500 } }
T.check(W2.init_still(o, 0) == nil, "first sample: no verdict")
o.pos = { X = 0, Y = 0, Z = 99.9 }
T.check(W2.init_still(o, 50) == nil, "younger than the sample interval: no verdict")
T.check(W2.init_still(o, 200) == false, "falling (speed above SETTLE_SPEED) is not still")
o.vel = { X = 0, Y = 0, Z = 0 }
T.check(W2.init_still(o, 400) == true, "same pose, no speed, 200 ms later: still")
o.pos = { X = 30, Y = 0, Z = 99.9 }
T.check(W2.init_still(o, 600) == false, "moved since the sample: not still")

-- ---------------------------------------------------------------------------
-- World settle, own drops, the re-arm window close, pinned bodies
local function mkobj(name, cls, addr_, props)
    local o = props or {}
    o.IsValid = function() return true end
    o.GetAddress = function() return addr_ end
    o.GetFName = function() return { ToString = function() return name end } end
    o.GetClass = function()
        return { GetFName = function() return { ToString = function() return cls end } end,
                 GetFullName = function() return "BlueprintGeneratedClass /Game/W/" .. cls end }
    end
    o.GetAttachParentActor = function() return nil end
    return o
end
local function mkbody(pos, sim)
    local b = { sim = sim, sets = {} }
    b.IsValid = function() return true end
    b.GetAddress = function() return 0xB0D1 end
    b.IsSimulatingPhysics = function() return b.sim end
    b.SetSimulatePhysics = function(_, on) b.sim = on; b.sets[#b.sets + 1] = on end
    b.K2_GetComponentLocation = function() return { X = pos.X, Y = pos.Y, Z = pos.Z } end
    b.K2_GetComponentRotation = function() return { Pitch = 0, Yaw = 0, Roll = 0 } end
    b.K2_SetWorldLocationAndRotation = function() end
    b.SetPhysicsLinearVelocity = function() end
    b.SetPhysicsAngularVelocityInDegrees = function() end
    b.PutRigidBodyToSleep = function() end
    return b
end

T.log("== HSMPWorld walks no actor in the first WG.SETTLE_S of a world")
do
    local UEH = require("UEHelpers")
    local WGt = W2.WG
    local wname = "World /Game/Maps/Arenas/Map_Arena_Pit.Map_Arena_Pit"
    local world = { IsValid = function() return true end, GetFullName = function() return wname end,
                    GetAddress = function() return 0x99 end }
    local pc = { IsValid = function() return true end, GetWorld = function() return world end,
                 GetFName = function() return { ToString = function() return "PlayerController_9" end } end }
    UEH.GetPlayerController = function() return pc end
    UEH.GetWorld = function() return world end
    local walks, names = {}, 0
    local willie = mkobj("Willie_BP_C_5", "Willie_BP_C", 0x5005)
    local gf = willie.GetFName
    willie.GetFName = function(...) names = names + 1; return gf(...) end
    _G.FindAllOf = function(c) walks[#walks + 1] = c; if c == "Willie_BP_C" then return { willie } end return {} end
    -- a live session (connected + a fresh heartbeat), same peer id as before
    sidecar("connected", 5); W2.refresh_session(31000)
    T.check(W2.sess.live == true, "session live for the settle test")
    HSMPNative.bus_put("puppets", { rows = { { peer = 2, name = "Willie_BP_C_5" } } })
    for _ = 1, 20 do W2.on_tick() end
    local Wn = W2.W()
    T.check(Wn ~= nil and Wn.name == wname, "a replicated world exists for the new arena")
    if not Wn then return end
    Wn.synced, Wn.puppet_next = true, 0   -- even a synced world (owners file from disk) waits
    for _ = 1, 20 do W2.on_tick() end
    T.check(not WGt.settled(), "inside the settle window")
    T.check(#walks == 0 and names == 0, "no FindAllOf and no name read before the world settled", T.repr(walks))
    T.check(Wn.scan == nil and Wn.scans_done == 0, "no discovery scan before the settle")
    -- the window passes
    WGt.key_at = WGt.key_at - 3
    W2.on_tick()
    Wn.scan_t0 = Wn.scan_t0 - 1000
    Wn.puppet_next = 0
    W2.on_tick()
    T.check(T.contains(table.concat(walks, ","), "ModularWeaponBP_C"), "after the settle the discovery scan runs", T.repr(walks))
    T.check(T.contains(table.concat(walks, ","), "Willie_BP_C"), "after the settle the stand-in walk runs")
    -- no stand-ins listed: no walk at all
    HSMPNative.bus_put("puppets", { rows = {} })
    local n0 = #T.filter(walks, function(c) return c == "Willie_BP_C" end)
    for _ = 1, 3 do Wn.puppet_next = 0; W2.on_tick() end
    local n1 = #T.filter(walks, function(c) return c == "Willie_BP_C" end)
    T.check(n1 == n0, "an empty puppets set never walks the Willies", n1 - n0)
    UEH.GetPlayerController, UEH.GetWorld = nil, nil
    _G.FindAllOf = nil
end

T.log("== drop window = the kit re-arm window (fresh, own pawn); entry kept while suppressed")
local DROP_NID
do
    local WN2 = "World /Game/Maps/Arenas/Map_Arena_Alley.Map_Arena_Alley"
    W2.new_level(WN2, 0x55, 1000)
    local Wn = W2.W()
    Wn.synced = true
    W2.sess.my_id = 5
    local H = W2.hands
    H.pawn = mkobj("Willie_ME", "Willie_BP_C", 0x7000)
    H.pawn_loc = { X = 100, Y = 0, Z = 100 }
    H.R, H.L, H.Ra, H.La = nil, nil, nil, nil
    local body = mkbody({ X = 150, Y = 0, Z = 90 }, true)
    local wpn = mkobj("ModularWeaponBP_Sword_C_1", "ModularWeaponBP_Sword_C", 7001, { ["Is Held"] = false, BaseMesh = body })
    Wn.my_items[7001] = { actor = wpn, side = "R", loose_since = 0 }
    match("countdown", 2)
    W2.track_my_items(5000)
    T.check(Wn.my_items[7001] ~= nil and Wn.my_items[7001].suppressed == "countdown" and Wn.by_addr[7001] == nil,
        "countdown: not a world item, the entry is KEPT")
    match("live", 2)
    spawn_status("Willie_ME", os.clock() + 100)
    W2.track_my_items(5100)
    T.check(Wn.my_items[7001] ~= nil and Wn.by_addr[7001] == nil, "my pawn's spawn protection (read fresh): still suppressed")
    local inwin = W2.drop_window(mkobj("Willie_OTHER", "Willie_BP_C", 1))
    T.check(inwin == false, "another pawn's protection is not my window (as kit.rearm_window)")
    spawn_status("Willie_ME", os.clock() - 1)
    W2.track_my_items(5200)
    local o = Wn.by_addr[7001]
    T.check(o ~= nil and o.nid and o.dyn_owner == 5 and Wn.my_items[7001] == nil,
        "still loose when the window closes -> registered as my world item")
    DROP_NID = o and o.nid
    T.check(DROP_NID and W2.sess.my_nids[DROP_NID] == Wn.level, "the id I created is remembered (per level)")
end

T.log("== after a peer-id change my own drops are re-adopted, never spawned as a peer's copy")
do
    local WN2 = "World /Game/Maps/Arenas/Map_Arena_Alley.Map_Arena_Alley"
    local Wn = W2.W()
    local o = Wn.by_nid[DROP_NID]
    o.probed_at = os.clock() * 1000
    W2.sess.id_changed = { from = 5, to = 6 }
    W2.sess.my_id = 6
    T.check(W2.apply_session_change(WN2, 0x55) == true and W2.W() == nil, "session change drops W")
    T.check(W2.sess.mine[DROP_NID] ~= nil, "my live drop is kept for re-adoption")
    W2.new_level(WN2, 0x55, 2000)
    local Wb = W2.W()
    W2.sess.my_nids[0x80057777] = Wb.level   -- an own drop that no longer exists here
    W2.bind_dyn({
        { id = DROP_NID, dyn = 5, chash = 1, pos = { X = 150, Y = 0, Z = 90 }, class = "/Game/W/ModularWeaponBP_Sword_C" },
        { id = 0x80057777, dyn = 5, chash = 1, pos = { X = 0, Y = 50, Z = 90 }, class = "/Game/W/ModularWeaponBP_Sword_C" },
        { id = 0x80030001, dyn = 3, chash = 1, pos = { X = 9, Y = 9, Z = 90 }, class = "/Game/W/ModularWeaponBP_Axe_C" },
    }, 2100)
    local a = Wb.by_nid[DROP_NID]
    T.check(a ~= nil and a.adopted and a.actor_addr == 7001 and not a.spawned_by_us and Wb.dyn_queue[DROP_NID] == nil,
        "my own drop under my old id binds my existing actor (no copy)")
    T.check(Wb.dyn_queue[0x80057777] == nil and Wb.dyn_failed[0x80057777] == true,
        "an own drop that is gone here is never spawned as a copy")
    T.check(Wb.dyn_queue[0x80030001] ~= nil, "a real peer's drop is still queued for a local copy")
end

T.log("== a kinematic pin never outlives the free state")
do
    local Wn = W2.W()
    local function pinned(nid)
        local b = mkbody({ X = 0, Y = 0, Z = 50 }, false)
        local act = mkobj("Barrel_" .. nid, "BP_Barrel_C", 9000 + nid)
        local o = { nid = nid, lid = nid, kind = "prop", cls = "BP_Barrel_C", comp = "SM", actor = act, body = b,
                    sim0 = true, sim = false, pin_kin = true, pinned_at = 1, probed_at = os.clock() * 1000,
                    anchor_pos = { X = 0, Y = 0, Z = 50 }, anchor_rot = { Pitch = 0, Yaw = 0, Roll = 0 },
                    pos = { X = 0, Y = 0, Z = 50 }, rot = { Pitch = 0, Yaw = 0, Roll = 0 }, vel = { X = 0, Y = 0, Z = 0 },
                    dyn_owner = 0, buf = {}, prio = 0, last_read = -999, last_probe = -999, spawn_pos = { X = 0, Y = 0, Z = 50 },
                    spawn_rot = { Pitch = 0, Yaw = 0, Roll = 0 } }
        Wn.objs[#Wn.objs + 1] = o
        Wn.by_nid[nid] = o
        return o, b
    end
    -- leaves the free state: a peer owns it now
    local o1, b1 = pinned(11)
    Wn.owners[11] = { owner = 3, mode = 1 }
    W2.update_body(o1, 9000, false)
    T.check(o1.pin_kin == nil and o1.pinned_at == nil and b1.sets[1] == true, "owned by a peer: un-pinned, scene physics restored first")
    -- a new anchor while pinned is applied (not ignored forever)
    local o2, b2 = pinned(12)
    o2.anchor_new = true
    W2.update_body(o2, 9000, false)
    T.check(o2.pin_kin == nil and b2.sets[1] == true and o2.anchor_new == false, "a new anchor un-pins and is applied")
    -- the W2 helper itself
    local o3, b3 = pinned(13)
    T.check(W2.W2.unpin(o3, true) == true and o3.pin_kin == nil and b3.sim == true, "unpin restores sim0")
    -- session end in this world: release_world gives pinned props their physics back
    local o4, b4 = pinned(14)
    W2.release_world("test")
    T.check(o4.pin_kin == nil and o4.pinned_at == nil and b4.sim == true, "release_world un-pins (physics back)")
    T.check(T.contains(T.read(WORLD), "o.kin, o.pin_kin, o.pinned_at = false, nil, nil"),
        "restore_all clears the pin as well")
end

-- ---- world_follow.lua: timelines, dead reckoning, blending -----------------------------
T.log("== world_follow: a body somebody else simulates")
do
    local FW = dofile(MODS .. "/HSMPWorld/Scripts/world_follow.lua")
    local function v(x, y, z) return { X = x, Y = y, Z = z } end
    local Q0 = { 0, 0, 0, 1 }
    local function smp(t, x, z, vx, vz, flags) return { t = t, pos = v(x, 0, z), q = Q0, vel = v(vx, 0, vz), flags = flags or 8 } end

    -- clock: offset = the fastest packet; the playout delay grows at once with lateness and
    -- shrinks slowly
    local c = FW.clock_new()
    for k = 0, 9 do FW.clock_note(c, 1000 + 50 * k, 5000 + 50 * k + 40) end
    T.check(c.off == 4040 and c.delay >= FW.D_MIN and c.delay <= FW.SEND_MS + 20, "steady stream: offset = path, delay ~ one interval",
        string.format("off=%s delay=%.1f", tostring(c.off), c.delay))
    FW.clock_note(c, 1500, 5500 + 40 + 120)   -- two packets 120 ms late (above the p90)
    FW.clock_note(c, 1550, 5550 + 40 + 120)
    T.check(c.delay >= FW.SEND_MS + 100, "late packets raise the delay at once", tostring(c.delay))
    local d0 = c.delay
    for k = 11, 20 do FW.clock_note(c, 1000 + 50 * k, 5000 + 50 * k + 40) end
    T.check(c.delay < d0 and c.delay > d0 - 40, "and it shrinks back slowly", tostring(c.delay))
    T.check(c.delay_fast < c.delay, "held / contact bodies play closer to the owner (stand-in timeline)")
    local c2 = FW.clock_new()
    FW.clock_note(c2, 100, 5000); FW.clock_note(c2, 150, 9000)
    T.check(c2.off == 8850, "a clock jump (reload, new session) starts the window over", tostring(c2.off))

    -- dead reckoning: ballistic in the air, clamped at the floor; friction on the ground
    local p = FW.extrapolate(smp(0, 0, 100, 500, 300), 200, nil, nil, 0)
    T.check(math.abs(p.X - 100) < 1e-6 and math.abs(p.Z - (100 + 60 - 0.5 * FW.G * 0.04)) < 1e-6, "airborne: ballistic", T.repr(p))
    p = FW.extrapolate(smp(0, 0, 20, 0, -600), 300, nil, nil, 5)
    T.check(p.Z == 5, "a landing is clamped at the floor estimate, never through it", T.repr(p))
    p = FW.extrapolate(smp(0, 0, 20, 200, 0), 1000, nil, nil, 0)
    T.check(math.abs(p.X - 200 * 200 / FW.SLIDE_DECEL / 2) < 1e-6 and p.Z == 20, "sliding: friction stops it", T.repr(p))
    p = FW.extrapolate(smp(0, 0, 20, 200, 0, 9), 500)
    T.check(p.X == 0, "an asleep sample is not extrapolated")

    -- blending: a new sample never makes the shown pose jump; the correction decays
    local cc = { off = 0, delay = 100, delay_fast = 60 }
    local st, buf = {}, { smp(0, 0, 0, 0, 0), smp(50, 10, 0, 0, 0) }
    local function show(now, dist) return FW.pose(st, buf, cc, { now = now, dt = 0.016, lead = 0, owner_dist = dist or 50 }) end
    local a = show(100)
    FW.before_change(st, buf)
    buf[#buf + 1] = smp(100, 80, 0, 0, 0)     -- a correction: the body is 60 cm further than the old stream said
    local b = show(116)
    T.check(math.abs(b.X - a.X) < 5, "a new sample shifts the target, not the screen", string.format("%.1f -> %.1f", a.X, b.X))
    for k = 1, 60 do b = show(116 + 16 * k) end
    T.check(math.abs(b.X - 80) < 0.5, "the correction blends away", tostring(b.X))
    T.check(st.u == 0, "a body at its owner stays in the owner's timeline")

    -- free flight: catch up to the present without a jump (at most 1.5x the speed)
    local st2, b2 = {}, {}
    for k = 0, 6 do b2[#b2 + 1] = smp(k * 50, k * 30, 100, 600, 0) end
    local cf = { off = 0, delay = 100, delay_fast = 60 }
    local last, maxstep = nil, 0
    for k = 0, 40 do
        local now = 300 + 16 * k
        local q = FW.pose(st2, b2, cf, { now = now, dt = 0.016, lead = 150, owner_dist = 1000 })
        if last then maxstep = math.max(maxstep, q.X - last.X) end
        last = q
    end
    T.check(st2.u == 1, "flying free of its owner: shown in the present")
    T.check(maxstep <= 600 * 0.016 * 1.6, "catch-up is continuous (no snap)", tostring(maxstep))
    -- held again: back to the owner's timeline (blended)
    FW.before_change(st2, b2)
    b2[#b2 + 1] = smp(400, 240, 100, 0, 0, 8 | 2)
    local h = FW.pose(st2, b2, cf, { now = 950, dt = 0.016, lead = 150, owner_dist = 1000 })
    T.check(st2.u == 0 and math.abs(h.X - last.X) < 15, "a held body returns to the owner's timeline without a jump", T.repr(h))

    -- a new owner: blend from what is on screen
    local st3 = {}
    FW.pose(st3, { smp(0, 0, 0, 0, 0) }, cc, { now = 100, dt = 0.016, owner_dist = 50 })
    FW.rebase(st3)
    local r = FW.pose(st3, { smp(1000, 100, 0, 0, 0) }, { off = -900, delay = 100, delay_fast = 60 }, { now = 100, dt = 0.016, owner_dist = 50 })
    T.check(math.abs(r.X) < 1e-6, "rebased onto another sender: no jump", T.repr(r))
    T.check(T.contains(T.read(WORLD), "K.FW = load_module(\"world_follow\")"), "main.lua uses world_follow.lua")
end
