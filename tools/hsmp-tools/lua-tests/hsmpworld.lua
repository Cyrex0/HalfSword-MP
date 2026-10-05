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
    { rot = { 0, 90, 0 }, vel = { 100.4, -0.5, 0.6 }, flags = 8, want_rot = { 0, 0, 32767 }, want_vel = { 100, 0, 1 }, want_flags = 8 | (2 << 6),
      -- z and w are both sin(45 deg): which one is dropped depends on the C library's last bit
      -- (glibc's gives w; the Rust reference, in f32, gives z). Both decode to the same rotation.
      alt_flags = 8 | (3 << 6) },
    { rot = { 10, 20, 30 }, vel = { 5, -6, 1e9 }, flags = 2, want_rot = { -11089, -5917, 6714 }, want_vel = { 5, -6, 32767 }, want_flags = 2 | (3 << 6) },
    { rot = { -45, 170, -120 }, vel = { 0, 0, 0 }, flags = 1 | 0x30, want_rot = { -5602, 19986, 17165 }, want_vel = { 0, 0, 0 }, want_flags = 1 | (1 << 6) },
}
for i, g in ipairs(GOLD) do
    local o = Q.qobj(7, { X = 0, Y = 0, Z = 0 }, { Pitch = g.rot[1], Yaw = g.rot[2], Roll = g.rot[3] },
                     { X = g.vel[1], Y = g.vel[2], Z = g.vel[3] }, g.flags)
    local ok = (o.flags == g.want_flags or o.flags == g.alt_flags) and T.eq(o.vel, g.want_vel)
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

T.log("== native physics failures retain the previous replicated state")
do
    local body = mkbody({ X = 150, Y = 0, Z = 90 }, true)
    local actor = mkobj("Prop_1", "Prop_C", 0x8801)
    local o = { actor = actor, body = body, kind = "prop", sim = true, sim0 = true,
                pos = { X = 150, Y = 0, Z = 90 }, rot = { Pitch = 0, Yaw = 0, Roll = 0 }, buf = {} }
    local setter = body.SetSimulatePhysics
    body.SetSimulatePhysics = function() error("native freeze refused") end
    T.check(not W.set_sim(o, false) and o.sim == true, "failed native freeze does not invent kinematic state")
    body.SetSimulatePhysics = function() end
    T.check(not W.set_sim(o, false) and o.sim == true, "silently refused native freeze is verified")
    body.SetSimulatePhysics = setter
    T.check(W.set_sim(o, false) and o.sim == false, "successful freeze updates simulation state")
    o.kin = true
    body.SetSimulatePhysics = function() error("native restore refused") end
    T.check(not W.become_local(o, "free") and o.kin == true and o.sim == false,
        "failed release retains follower ownership so physics restoration can retry")
    o.pin_kin, o.pinned_at = true, 10
    T.check(not W.W2.unpin(o, true) and o.pin_kin == true and o.pinned_at == 10,
        "failed unpin retains restoration ownership and deadline")
    o.pin_kin = nil
    body.SetSimulatePhysics = setter
    T.check(W.become_local(o, "free") and o.kin == false and o.sim == true, "release retries native restoration")
    local oldpos, oldrot = o.pos, o.rot
    body.K2_SetWorldLocationAndRotation = function() error("native teleport refused") end
    T.check(not W.teleport(o, { X = 175, Y = 0, Z = 90 }, oldrot) and o.pos == oldpos,
        "failed native teleport never claims that the body reached the target")
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
T.log("== placement and restoration failures retain transition ownership")
do
    W2.new_level("World /Game/Maps/Arenas/Map_Arena_Pit.Map_Arena_Pit", 0xA900, 90000)
    local wn = W2.W()
    local body = mkbody({ X = 100, Y = 0, Z = 50 }, true)
    body.GetFName = function() return { ToString = function() return "SM" end } end
    local actor = mkobj("Barrel_901", "BP_Barrel_C", 9901)
    local native_world = { IsValid = function() return true end,
        GetFullName = function() return wn.name end, GetAddress = function() return wn.addr end }
    actor.GetWorld = function() return native_world end
    body.GetWorld = function() return native_world end
    actor.K2_GetComponentsByClass = function() return { { get = function() return body end } } end
    local o = { nid = 901, lid = 901, kind = "prop", cls = "BP_Barrel_C", comp = "SM", actor = actor,
        actor_addr = 9901, body = body, sim = true, sim0 = true, dyn_owner = 0,
        spawn_pos = { X = 110, Y = 0, Z = 50 }, spawn_rot = { Pitch = 0, Yaw = 0, Roll = 0 },
        anchor_pos = { X = 110, Y = 0, Z = 50 }, anchor_rot = { Pitch = 0, Yaw = 0, Roll = 0 },
        pos = { X = 100, Y = 0, Z = 50 }, rot = { Pitch = 0, Yaw = 0, Roll = 0 }, vel = { X = 0, Y = 0, Z = 0 },
        buf = {}, probed_at = os.clock() * 1000, last_probe = -999, last_read = -999 }
    wn.objs = { o }; wn.by_nid[901] = o
    local setter, place = body.SetSimulatePhysics, body.K2_SetWorldLocationAndRotation
    body.SetSimulatePhysics = function() error("freeze refused") end
    T.check(not W2.restore_body(o) and o.restore_pending and body.sim,
        "spawn restoration stops before placement when freezing fails")
    body.SetSimulatePhysics = setter
    body.K2_SetWorldLocationAndRotation = function() error("spawn placement refused") end
    T.check(not W2.restore_body(o) and o.restore_pending and not body.sim,
        "spawn placement failure keeps ownership of the frozen body")
    local placements = 0
    body.K2_SetWorldLocationAndRotation = function() placements = placements + 1; if placements == 2 then error("sleep placement refused") end end
    T.check(not W2.restore_body(o) and o.restore_pending and body.sim,
        "failed final rest placement is retained after original simulation is restored")
    body.K2_SetWorldLocationAndRotation = place
    T.check(W2.restore_body(o) and not o.restore_pending,
        "spawn restoration retries the whole transition after final rest failure")
    body.K2_SetWorldLocationAndRotation = function() error("placement refused") end
    T.check(not W2.pin_anchor(o, 10) and o.pin_pending and not o.pin_kin and body.sim == false,
        "freeze followed by failed pin placement remains pending")
    body.K2_SetWorldLocationAndRotation = place
    T.check(W2.pin_anchor(o, 20) and o.pin_pending == nil and o.pin_kin and o.pinned_at == 20,
        "pending pin completes only after placement succeeds")
    o.pin_kin, o.kin = nil, true
    o.buf = { { flags = 1, pos = o.spawn_pos, q = { 0, 0, 0, 1 }, vel = { X = 0, Y = 0, Z = 0 } } }
    body.K2_SetWorldLocationAndRotation = function() error("final rest refused") end
    T.check(not W2.become_local(o, "free") and o.kin and body.sim == false,
        "asleep release does not enable physics at the wrong location")
    body.K2_SetWorldLocationAndRotation = place
    body.SetSimulatePhysics = function(_, on) if on then error("restore refused") else setter(body, on) end end
    local original_buf = o.buf
    T.check(not W2.restore_all("failure regression") and o.nid == 901 and o.buf == original_buf and o.kin and o.restore_pending,
        "round reset retains binding, stream and ownership until physics restoration succeeds")
    body.SetSimulatePhysics = setter
    T.check(W2.restore_all("recovery regression") and not o.kin and o.nid == nil and not o.restore_pending and body.sim,
        "round reset retries and commits after native restoration succeeds")
    o.kin, o.sim, body.sim = true, false, false
    body.SetSimulatePhysics = function() error("release refused") end
    T.check(not W2.release_world("failed release") and o.kin and not body.sim,
        "session release retains failed physics ownership")
    local q = W2.release_retries()
    local _, retry = next(q)
    T.check(retry and retry.actor == nil and retry.body == nil and retry.actor_addr == 9901 and retry.body_name == "SM",
        "session retry stores plain world and actor/component lineage without UObjects")
    local old_find, old_static, old_settled = FindAllOf, StaticFindObject, W2.WG.settled
    FindAllOf = function() return { actor } end
    StaticFindObject = function() return { IsValid = function() return true end } end
    W2.WG.settled = function() return true end
    body.SetSimulatePhysics = setter
    W2.W2.service_release_retries(wn.name, wn.addr, os.clock() * 1000)
    T.check(body.sim and next(q) == nil, "same-world release retry resolves fresh lineage and restores physics")
    body.sim, o.sim, o.kin = false, false, true
    body.SetSimulatePhysics = function() error("release refused") end
    W2.release_world("lineage regression")
    local sets = #body.sets
    W2.W2.service_release_retries(wn.name, "another guard key", os.clock() * 1000)
    T.check(next(q) == nil and #body.sets == sets, "world generation mismatch discards retries without native writes")
    W2.release_world("delayed retry regression")
    local _, delayed = next(q)
    local retry_at = os.clock() * 1000
    for i = 1, 12 do W2.W2.service_release_retries(wn.name, wn.addr, retry_at + i * 250) end
    T.check(next(q) ~= nil and delayed.tries == 12, "restoration failure beyond three seconds retains plain identity ownership")
    W2.W2.service_release_retries(wn.name, wn.addr, delayed.next_ms - 1)
    T.check(delayed.tries == 12, "persistent native failure backs off rather than querying every callback")
    body.SetSimulatePhysics = setter
    W2.W2.service_release_retries(wn.name, wn.addr, delayed.next_ms)
    T.check(body.sim and next(q) == nil, "late native recovery restores physics instead of leaving the object frozen")
    body.sim, o.sim, o.kin = false, false, true
    body.SetSimulatePhysics = function() error("release refused") end
    W2.release_world("reclaimed retry regression")
    wn.lineage = wn.lineage + 1
    body.SetSimulatePhysics = setter
    local reclaimed_sets = #body.sets
    W2.W2.service_release_retries(wn.name, wn.addr, os.clock() * 1000)
    T.check(next(q) == nil and not body.sim and #body.sets == reclaimed_sets,
        "old release lease cannot unfreeze a body reclaimed by a new replication lifetime")
    body.SetSimulatePhysics = function() error("release refused") end
    W2.release_world("teardown regression")
    T.check(next(q) ~= nil, "retry exists before teardown")
    local teardown_sets = #body.sets
    W2.WG.travel("retry teardown")
    T.check(next(W2.release_retries()) == nil and teardown_sets == #body.sets, "world teardown forgets pending identities untouched")
    W2.WG.travel_from = nil
    local batch = W2.release_retries()
    local queries = 0
    FindAllOf = function() queries = queries + 1; return {} end
    for i = 1, 5 do
        batch["budget" .. i] = { world_name = wn.name, world_key = wn.addr, next_ms = 0,
            tries = 0, cls = "MissingProp_C", actor_addr = i, name = "missing" .. i }
    end
    W2.W2.service_release_retries(wn.name, wn.addr, 1000)
    T.check(queries == 4, "many pending releases bound expensive native inventory queries per callback")
    W2.W2.service_release_retries(wn.name, wn.addr, 1001)
    T.check(queries == 5, "deferred release identities get the next callback without starvation")
    for key in pairs(batch) do batch[key] = nil end
    FindAllOf, StaticFindObject, W2.WG.settled = old_find, old_static, old_settled
end

T.log("== an ambiguous controller lookup loss preserves native restoration ownership")
do
    local name, native_address = "World /Game/Maps/Arenas/Map_Arena_Pit.Map_Arena_Pit", 44200
    local key = name .. "@" .. native_address .. "#OldPC"
    local restored_key = name .. "@" .. native_address .. "#NewPC"
    W2.new_level(name, key, 100000)
    local body = mkbody({ X = 100, Y = 0, Z = 50 }, true)
    body.GetFName = function() return { ToString = function() return "SM" end } end
    local actor = mkobj("Recover_Barrel", "BP_Barrel_C", 9950)
    local world = { IsValid = function() return true end,
        GetFullName = function() return name end, GetAddress = function() return native_address end }
    actor.GetWorld = function() return world end
    body.GetWorld = function() return world end
    actor.K2_GetComponentsByClass = function() return { { get = function() return body end } } end
    local o = { actor = actor, actor_addr = 9950, body = body, cls = "BP_Barrel_C", kind = "prop", sim0 = true, sim = true }
    T.check(W2.set_sim(o, false) and not body.sim, "freezing captures original simulation before losing current object references")
    local old_find, old_static, old_settled = FindAllOf, StaticFindObject, W2.WG.settled
    local actor_addr, body_addr, cached_reads = actor.GetAddress, body.GetAddress, 0
    actor.GetAddress = function() cached_reads = cached_reads + 1; error("cached actor touched during drop") end
    body.GetAddress = function() cached_reads = cached_reads + 1; error("cached body touched during drop") end
    W2.WG.drop("no valid world")
    actor.GetAddress, body.GetAddress = actor_addr, body_addr
    T.check(cached_reads == 0 and W2.W() == nil and not body.sim,
        "ambiguous guard drop discards wrappers without reading or writing old native objects")
    T.check(W2.W2.recovery_pending(name, restored_key), "controller name changes retain recovery for the same native world")
    FindAllOf = function() return { actor } end
    StaticFindObject = function() return { IsValid = function() return true end } end
    W2.WG.settled = function() return true end
    local before = #body.sets
    actor.GetWorld = function() return { IsValid = function() return true end,
        GetFullName = function() return "another world" end, GetAddress = function() return native_address end } end
    W2.W2.service_release_retries(name, restored_key, 100000)
    T.check(not body.sim and #body.sets == before and W2.W2.recovery_pending(name, restored_key),
        "matching actor address and name cannot authorise a write in the wrong native world")
    actor.GetWorld = function() return world end
    W2.W2.service_release_retries(name, restored_key, 100250)
    T.check(body.sim and not W2.W2.recovery_pending(name, restored_key),
        "fresh exact native lookup restores original simulation before rescanning a frozen follower")
    W2.new_level(name, restored_key, 100500)
    T.check(W2.set_sim(o, false), "removed-body scenario starts from an owned freeze")
    W2.WG.drop("world changed")
    local is_valid = body.IsValid
    actor.K2_GetComponentsByClass = function() return {} end
    body.IsValid = function() error("removed body wrapper touched") end
    before = #body.sets
    W2.W2.service_release_retries(name, restored_key, 100750)
    T.check(not W2.W2.recovery_pending(name, restored_key) and #body.sets == before,
        "successful fresh enumeration of a removed original body releases recovery without touching its old wrapper")
    body.IsValid = is_valid
    actor.K2_GetComponentsByClass = function() return { { get = function() return body end } } end
    body.sim, o.sim = true, true
    W2.new_level(name, restored_key, 101000)
    T.check(W2.set_sim(o, false), "a new freeze has fresh restoration ownership")
    before = #body.sets
    W2.WG.travel("real LoadMap")
    T.check(next(W2.release_retries()) == nil and #body.sets == before,
        "confirmed travel discards the plain ledger without touching the old world")
    W2.WG.travel_from = nil
    W2.new_level(name, key, 102000)
    local copy = mkobj("Remote_Sword", "ModularWeaponBP_Sword_C", 9970)
    copy.GetWorld = function() return world end
    local destroys = 0
    copy.K2_DestroyActor = function() destroys = destroys + 1 end -- refused/deferred native request
    local copied = { actor = copy, actor_addr = 9970, body = body, cls = "ModularWeaponBP_Sword_C",
        kind = "weapon", sim0 = true, sim = true, spawned_by_us = true }
    T.check(W2.W2.track_copy(copied), "a created copy retains plain cleanup ownership before any guard loss")
    W2.WG.drop("world changed")
    FindAllOf = function() return { copy } end
    W2.W2.service_release_retries(name, restored_key, 102000)
    T.check(destroys == 1 and W2.W2.recovery_pending(name, restored_key),
        "a protected destroy call cannot release recovery for a still-present copy")
    W2.W2.service_release_retries(name, restored_key, 102250)
    T.check(destroys == 2 and W2.W2.recovery_pending(name, restored_key),
        "controller identity changes cannot discard an unconfirmed cleanup lease")
    FindAllOf = function() return {} end
    W2.W2.service_release_retries(name, restored_key, 102500)
    T.check(not W2.W2.recovery_pending(name, restored_key),
        "fresh native absence confirms copy cleanup and permits rescanning")
    W2.new_level(name, restored_key, 103000)
    body.sim = true
    copied.spawned_by_us = false
    T.check(W2.set_sim(copied, false), "pathless weapon component starts with original simulation ownership")
    W2.WG.drop("world changed")
    copy.GetAttachedActors = function() end
    copy.K2_GetComponentsByClass = function() return {} end
    FindAllOf = function() return { copy } end
    W2.W2.service_release_retries(name, restored_key, 103250)
    T.check(not W2.W2.recovery_pending(name, restored_key),
        "complete fresh weapon and attached-component enumeration proves a pathless original body absent")
    W2.new_level(name, restored_key, 104000)
    copied.spawned_by_us, copied.probed_at = true, os.clock() * 1000
    copied.dead = nil
    W2.W().objs = { copied }
    W2.W2.track_copy(copied)
    copy.K2_DestroyActor = function() error("native destruction refused") end
    W2.release_world("thrown copy cleanup")
    T.check(next(W2.release_retries()) ~= nil and copied.actor == nil,
        "failed native destruction queues plain copy cleanup before releasing the cached wrapper")
    FindAllOf = function() return {} end
    W2.W2.service_release_retries(name, restored_key, 104250)
    T.check(next(W2.release_retries()) == nil, "later fresh absence completes failed copy cleanup")
    FindAllOf, StaticFindObject, W2.WG.settled = old_find, old_static, old_settled
end

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

-- A dropped modular weapon must survive the other mod stripping its hand
-- actor first. Keep only plain record data, scoped to the current arena.
do
    W2.new_level("World /Game/Maps/Arenas/Map_Arena_Pit.Map_Arena_Pit", 0xA123, 90000)
    local world = W2.W()
    world.puppets = { { id = 2 } }
    local path = "/Game/Assets/Weapons/Sword.Sword_C"
    local pass = { class = "@Weapons/Sword", head = "@Weapons/Blade", head_size = { 1.2, 0.8, 1.1 },
        color_wood = { 0.1, 0.2, 0.3, 1 }, mass_head = 1.7, mat_steel = 3 }
    local rec = { flags = 1, r = pass }
    local prior_ipc = W2.W2.ipc
    W2.W2.ipc = function() return { peer_rec = function(slot, peer)
        T.check(slot == "peer_loadout" and peer == 2, "passport cache uses the dropping peer's appearance")
        return rec
    end } end
    W2.cache_peer_passports()
    rec = { flags = 0 }
    W2.cache_peer_passports()
    T.check(world.peer_passports[2][path] == pass, "hand removal retains the last received modules for the dropped item")
    W2.W2.ipc = prior_ipc
    local prior_find = _G.StaticFindObject
    _G.StaticFindObject = function(p) return { IsValid = function() return true end, path = p,
        GetFullName = function() return "BlueprintGeneratedClass " .. p end } end
    local decoded = W2.record_passport(pass)
    T.check(decoded.HeadModule_11_62DF53134688807E1DA7F4A20E9F7139.path == "/Game/Assets/Weapons/Blade.Blade_C",
        "dropped item uses the owner's actual blade class")
    T.check(decoded.HeadSize_21_2D425E61473B8F64FBAB51B223459D57.X == 1.2 and
        decoded.CustomMassScaleHead_30_B95872A242AD944E2CE4D493F718F9D7 == 1.7,
        "dropped item preserves module size and physics mass")
    decoded.Name_57_3729B51148E846FE8DD336B9419BCEE1 = { ToString = function() return "my sword" end }
    local captured = W2.encode_passport({ ["Weapon Passport"] = decoded })
    T.check(captured.head == "@Weapons/Blade" and captured.head_size[1] == 1.2 and captured.mass_head == 1.7,
        "source actor passport captures exact modules and compresses UClass paths rather than their metaclass")
    local helpers = require("UEHelpers")
    local old_world, old_gs = helpers.GetWorld, helpers.GetGameplayStatics
    local spawned = { IsValid = function() return true end }
    local finished_passport
    helpers.GetWorld = function() return { IsValid = function() return true end } end
    helpers.GetGameplayStatics = function() return {
        IsValid = function() return true end,
        BeginDeferredActorSpawnFromClass = function() return spawned end,
        FinishSpawningActor = function(_, actor) finished_passport = actor["Weapon Passport"] end,
    } end
    local dropped, why = W2.spawn_remote_item({ id = 0x80020001, dyn = 2, class = path,
        pos = { X = 100, Y = 0, Z = 10 } })
    T.check(dropped == spawned and why == "received peer passport" and
        finished_passport.HeadModule_11_62DF53134688807E1DA7F4A20E9F7139.path == "/Game/Assets/Weapons/Blade.Blade_C",
        "destroyed stand-in hand still yields the real modular blade before actor construction")
    local other = {}; for k, v in pairs(pass) do other[k] = v end
    other.head = "@Weapons/OtherBlade"
    local manifest = Q.manifest_from({ level = 1, epoch = 1, rows = {} }, { level = 1, epoch = 1, rows = {
        { id = 0x80020002, chash = 1, pos = { 100, 0, 10 }, dyn_owner = 2, class_path = path,
          has_passport = true, passport = other } } })
    W2.spawn_remote_item(manifest.e[1])
    T.check(finished_passport.HeadModule_11_62DF53134688807E1DA7F4A20E9F7139.path == "/Game/Assets/Weapons/OtherBlade.OtherBlade_C",
        "per-id dynamic passport beats an old same-class hand cache, including late join")
    local previous_passport = finished_passport
    local failed_copy, reason = W2.spawn_remote_item({ id = 0x80020003, class = path,
        pos = { X = 100, Y = 0, Z = 10 }, src = { ["Weapon Passport"] = nil } })
    T.check(failed_copy == nil and reason == "source weapon passport not ready" and finished_passport == previous_passport,
        "pickup race never finishes a class-default replacement when the original passport cannot be copied")
    helpers.GetWorld, helpers.GetGameplayStatics = old_world, old_gs
    _G.StaticFindObject = prior_find
    W2.new_level("World /Game/Maps/Arenas/Map_Arena_Pit.Map_Arena_Pit", 0xA124, 91000)
    T.check(next(W2.W().peer_passports) == nil, "arena reload drops cached weapon appearances")
end

do
    local function component(name)
        return {IsValid=function()return true end,GetFName=function()return{ToString=function()return name end}end}
    end
    local z,a=component("Z_Constraint"),component("A_Constraint")
    local actor={K2_GetComponentsByClass=function()return{{get=function()return z end},{get=function()return a end}}end}
    W2.W().pcc_class=component("PhysicsConstraintComponent")
    local constraints=W2.W2.actor_constraints(actor)
    T.check(#constraints==2 and constraints[1][1]=="A_Constraint" and constraints[2][1]=="Z_Constraint",
        "native copied constraint return array preserves deterministic bit order")
    T.check(constraints[1][2]==a and constraints[2][2]==z,"constraint enumeration retains exact fresh UObject identities")
    local missing=T.read(WORLD):gsub('K.NA = load_module%("native_array"%)','K.NA = nil')
    T.check(assert(load(missing,"@"..rel(WORLD)))()==nil,"missing required native-array dependency disables World explicitly")
end
