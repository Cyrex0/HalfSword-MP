-- HSMPWorld's pure helpers (no UE calls): ids, rotations, the typed world records
-- and the identical-worlds codec (bit-identical to server/src/world.rs). main.lua
-- loads this with load_module("world_pure") and calls install with its test-hook table T,
-- its constants K and its record helpers W2, which this fills in; the return value
-- holds the helpers main.lua uses as locals. Unit-tested by `hsmp-tools lua-test
-- hsmpworld` / `world_state` through those tables.
local M = {}
function M.install(T, K, W2)
local FUZZY_R          = 120     -- manifest binding radius, same class+comp
local CLASSLESS_R      = 40      -- weapon class mismatch fallback radius
local QUANT            = 50      -- spawn-cell size for runtime-spawned ids

local function fnv1a(s)
    local h = 2166136261
    for i = 1, #s do
        h = h ~ s:byte(i)
        h = (h * 16777619) & 0xFFFFFFFF
    end
    return h
end
T.fnv1a = fnv1a

local function clock_ms() return os.clock() * 1000 end
local function v3(x, y, z) return { X = x, Y = y, Z = z } end
local function dist3(a, b)
    local dx, dy, dz = a.X - b.X, a.Y - b.Y, a.Z - b.Z
    return math.sqrt(dx * dx + dy * dy + dz * dz)
end
local function vlen(v) return math.sqrt(v.X * v.X + v.Y * v.Y + v.Z * v.Z) end
-- Never hand NaN / inf to the sidecar (world_client.rs rejects non-finite
-- values). Non-finite -> 0.
local function F(x)
    x = tonumber(x) or 0
    if x ~= x or x == math.huge or x == -math.huge then return 0 end
    return x
end
T.F = F
local function wrap180(d) return (d + 180) % 360 - 180 end
local function rot_diff(a, b)
    return math.max(math.abs(wrap180(a.Pitch - b.Pitch)), math.abs(wrap180(a.Yaw - b.Yaw)),
                    math.abs(wrap180(a.Roll - b.Roll)))
end
local function lerp(a, b, t) return a + (b - a) * t end
T.dist3 = dist3

-- UE FQuat::Rotator.
local function quat_to_rot(x, y, z, w)
    local r2d = 180 / math.pi
    local sing = z * x - w * y
    local yaw = math.atan(2 * (w * z + x * y), 1 - 2 * (y * y + z * z)) * r2d
    local pitch, roll
    if sing < -0.4999995 then
        pitch = -90
        roll = wrap180(-yaw - 2 * math.atan(x, w) * r2d)
    elseif sing > 0.4999995 then
        pitch = 90
        roll = wrap180(yaw - 2 * math.atan(x, w) * r2d)
    else
        pitch = math.asin(math.max(-1, math.min(1, 2 * sing))) * r2d
        roll = math.atan(-2 * (w * x + y * z), 1 - 2 * (x * x + y * y)) * r2d
    end
    return { Pitch = pitch, Yaw = yaw, Roll = roll }
end
T.quat_to_rot = quat_to_rot
local function rot_to_quat_t(r)
    local h = math.pi / 360
    local sp, cp = math.sin(r.Pitch * h), math.cos(r.Pitch * h)
    local sy, cy = math.sin(r.Yaw * h), math.cos(r.Yaw * h)
    local sr, cr = math.sin(r.Roll * h), math.cos(r.Roll * h)
    return { X = cr * sp * sy - sr * cp * cy, Y = -cr * sp * cy - sr * cp * sy,
             Z = cr * cp * sy - sr * sp * cy, W = cr * cp * cy + sr * sp * sy }
end
T.rot_to_quat_t = rot_to_quat_t

-- Normalised lerp along the short arc.
local function nlerp(a, b, t)
    local d = a[1] * b[1] + a[2] * b[2] + a[3] * b[3] + a[4] * b[4]
    local s = d < 0 and -1 or 1
    local q = { lerp(a[1], s * b[1], t), lerp(a[2], s * b[2], t), lerp(a[3], s * b[3], t), lerp(a[4], s * b[4], t) }
    local n = math.sqrt(q[1] * q[1] + q[2] * q[2] + q[3] * q[3] + q[4] * q[4])
    if n < 1e-9 then return b end
    return { q[1] / n, q[2] / n, q[3] / n, q[4] / n }
end
T.nlerp = nlerp

-- Level-placed actors have the same FName on every client; runtime spawns get
-- 2147xxxxxx-style suffixes.
local function stable_name(name)
    if not name or name:find("^Default__") then return false end
    local n = tonumber(name:match("_(%d+)$"))
    return (n == nil) or (n < 1000000000)
end
T.stable_name = stable_name

local function cell(v) return math.floor(v / QUANT + 0.5) end

-- Deterministic ids for a batch of candidates {cls, comp, levelpath, name,
-- pos}. Candidates are ordered by their key so collisions get the same
-- ordinal on every client. `taken` = ids already used in this level.
local function assign_ids(cands, taken)
    for _, c in ipairs(cands) do
        local where
        if c.name and stable_name(c.name) then
            where = (c.levelpath or "") .. ":" .. c.name
        else
            where = string.format("@%d,%d,%d", cell(c.pos.X), cell(c.pos.Y), cell(c.pos.Z))
        end
        c.key = c.cls .. "|" .. c.comp .. "|" .. where
        c.tie = string.format("%.0f,%.0f,%.0f", c.pos.X, c.pos.Y, c.pos.Z)
        c.chash = fnv1a(c.cls .. "|" .. c.comp)
    end
    table.sort(cands, function(a, b)
        if a.key ~= b.key then return a.key < b.key end
        return a.tie < b.tie
    end)
    for _, c in ipairs(cands) do
        local id = fnv1a(c.key) & 0x7FFFFFFF        -- high bit = dynamic namespace
        local n = 1
        while taken[id] or id == 0 do
            n = n + 1
            id = fnv1a(c.key .. "#" .. n) & 0x7FFFFFFF
        end
        taken[id] = true
        c.lid = id
    end
    return cands
end
T.assign_ids = assign_ids

-- --- typed records (crates/hsmp-ipc/src/schema/world.rs) ----------------------------------
-- The game writes the canonical, quantised form ONCE, here; nobody re-encodes it.
-- WorldObj = { id, pos = {x, y, z} (cm, 0.1), rot = smallest-three i16 x3, vel = i16 cm/s x3,
--              flags = WF_* | dropped quaternion index << 6 }.
K.QSCALE = 32767 * math.sqrt(2)
K.WF_QSHIFT = 6

-- Rotator (degrees) -> WorldObj.rot + the dropped index (schema world::pack_quat).
function W2.pack_q16(r)
    local h = math.pi / 360
    local sp, cp = math.sin(F(r.Pitch) * h), math.cos(F(r.Pitch) * h)
    local sy, cy = math.sin(F(r.Yaw) * h), math.cos(F(r.Yaw) * h)
    local sr, cr = math.sin(F(r.Roll) * h), math.cos(F(r.Roll) * h)
    local q = { cr * sp * sy - sr * cp * cy, -cr * sp * cy - sr * cp * sy,
                cr * cp * sy - sr * sp * cy, cr * cp * cy + sr * sp * sy }
    local n = math.sqrt(q[1] * q[1] + q[2] * q[2] + q[3] * q[3] + q[4] * q[4])
    if not (n > 1e-6 and n < math.huge) then q, n = { 0, 0, 0, 1 }, 1 end
    local big = 1
    for i = 2, 4 do if math.abs(q[i]) > math.abs(q[big]) then big = i end end
    local s = q[big] < 0 and -1 or 1
    local out = {}
    for i = 1, 4 do
        if i ~= big then
            local v = math.floor(q[i] / n * s * K.QSCALE + 0.5)
            out[#out + 1] = math.max(-32767, math.min(32767, v))
        end
    end
    return out, big - 1
end

-- WorldObj.rot + flags -> quaternion {x, y, z, w} (schema world::unpack_quat).
function W2.unpack_q16(c, flags)
    local big = ((flags or 0) >> K.WF_QSHIFT) & 3
    local v = { (c[1] or 0) / K.QSCALE, (c[2] or 0) / K.QSCALE, (c[3] or 0) / K.QSCALE }
    local rest = math.sqrt(math.max(0, 1 - v[1] * v[1] - v[2] * v[2] - v[3] * v[3]))
    local q, k = {}, 1
    for i = 0, 3 do
        if i == big then q[i + 1] = rest else q[i + 1] = v[k]; k = k + 1 end
    end
    return q
end

-- cm/s -> i16 (round half up, clamped; non-finite -> 0).
function W2.v16(x) return math.max(-32767, math.min(32767, math.floor(F(x) + 0.5))) end

-- One WorldObj row (pos rounded to 0.1 cm, inside the schema's WORLD_LIMIT).
function W2.wpos(x) return math.max(-1e7, math.min(1e7, W2.rnd(x, 10))) end
function W2.qobj(id, pos, rot, vel, flags)
    local r, big = W2.pack_q16(rot)
    local v = vel or { X = 0, Y = 0, Z = 0 }
    return { id = id, pos = { W2.wpos(pos.X), W2.wpos(pos.Y), W2.wpos(pos.Z) }, rot = r,
             vel = { W2.v16(v.X), W2.v16(v.Y), W2.v16(v.Z) }, flags = ((flags or 0) & 0x0F) | (big << K.WF_QSHIFT) }
end

-- Readers: the sidecar's records as the shapes the logic below uses.
function W2.owners_from(t)
    if type(t) ~= "table" then return nil end
    local r = { level = t.level or 0, epoch = t.epoch or 0, sync = t.sync == true, mlen = t.manifest_len or 0, o = {} }
    for _, e in ipairs(t.rows or {}) do
        r.o[e.id] = { owner = e.owner, ver = e.ver, mode = e.mode }
    end
    return r
end
-- Static entries (world_manifest) and dynamic ones (world_dyn: dyn = the dropping peer).
function W2.manifest_from(t, td)
    if type(t) ~= "table" then return nil end
    local r = { level = t.level or 0, epoch = t.epoch or 0, e = {} }
    for _, e in ipairs(t.rows or {}) do
        r.e[#r.e + 1] = { id = e.id, chash = e.chash, pos = v3(e.pos[1], e.pos[2], e.pos[3]), dyn = 0, class = "" }
    end
    if type(td) == "table" and td.level == r.level and td.epoch == r.epoch then
        for _, e in ipairs(td.rows or {}) do
            r.e[#r.e + 1] = { id = e.id, chash = e.chash, pos = v3(e.pos[1], e.pos[2], e.pos[3]), dyn = e.dyn_owner,
                              class = e.class_path or "", passport = e.has_passport == true and e.passport or nil }
        end
    end
    return r
end
-- world_remote rows: { sender, seq, ts, obj }; sender 0 = the server's anchor of a free body.
function W2.remote_from(t)
    if type(t) ~= "table" then return nil end
    local r = { level = t.level or 0, epoch = t.epoch or 0, rows = {} }
    for _, s in ipairs(t.rows or {}) do
        local o = s.obj
        if type(o) == "table" then
            r.rows[#r.rows + 1] = {
                id = o.id, sender = s.sender, seq = s.seq, ts = s.ts, pos = v3(o.pos[1], o.pos[2], o.pos[3]),
                q = W2.unpack_q16(o.rot, o.flags), vel = v3(o.vel[1], o.vel[2], o.vel[3]), flags = o.flags & 0x0F,
            }
        end
    end
    return r
end

-- Bind manifest entries to local records. `objs` = array of records with
-- lid, chash, kind, spawn_pos, nid (nil = unbound). Returns counts and the
-- entries left unmatched (dynamic ones are left to the caller).
local function bind_manifest(entries, objs, by_lid, by_nid)
    local c = { exact = 0, alias = 0, mismatch = 0, unmatched = 0, dyn = {}, unmatched_ids = {} }
    for _, e in ipairs(entries) do
        if not by_nid[e.id] then
            if e.dyn ~= 0 then
                c.dyn[#c.dyn + 1] = e
            else
                local o = by_lid[e.id]
                if o and (o.nid or o.chash ~= e.chash) then o = nil end
                local how = "exact"
                if not o then
                    local best, bd = nil, FUZZY_R
                    for _, x in ipairs(objs) do
                        if not x.nid and x.dyn_owner == 0 and x.chash == e.chash then
                            local d = dist3(x.spawn_pos, e.pos)
                            if d < bd then best, bd = x, d end
                        end
                    end
                    o, how = best, "alias"
                end
                if not o then
                    local best, bd = nil, CLASSLESS_R
                    for _, x in ipairs(objs) do
                        if not x.nid and x.dyn_owner == 0 and x.kind == "weapon" then
                            local d = dist3(x.spawn_pos, e.pos)
                            if d < bd then best, bd = x, d end
                        end
                    end
                    o, how = best, "mismatch"
                end
                if o then
                    o.nid = e.id
                    by_nid[e.id] = o
                    c[how] = c[how] + 1
                    if how ~= "exact" then o.bind_how = how end
                else
                    c.unmatched = c.unmatched + 1
                    c.unmatched_ids[#c.unmatched_ids + 1] = e.id
                end
            end
        end
    end
    return c
end
T.bind_manifest = bind_manifest

-- --- identical-worlds pure helpers (codec bit-identical to server/src/world.rs) ---
K.SQRT_HALF = math.sqrt(0.5)

-- Smallest-three quaternion in 32 bits (world.rs pack_quat). q = {x,y,z,w}.
function W2.pack_quat(q)
    local d = { q[1] or 0, q[2] or 0, q[3] or 0, q[4] or 1 }
    local n = math.sqrt(d[1] * d[1] + d[2] * d[2] + d[3] * d[3] + d[4] * d[4])
    if not (n > 1e-9 and n < math.huge) then
        d = { 0, 0, 0, 1 }
    else
        for i = 1, 4 do d[i] = d[i] / n end
    end
    local big = 1
    for i = 2, 4 do if math.abs(d[i]) > math.abs(d[big]) then big = i end end
    local s = d[big] < 0 and -1 or 1
    local out = (big - 1) << 30
    local k = 0
    for i = 1, 4 do
        if i ~= big then
            local x = math.max(-K.SQRT_HALF, math.min(K.SQRT_HALF, s * d[i]))
            local u = math.floor((x / K.SQRT_HALF + 1) * 511.5 + 0.5)
            u = math.max(0, math.min(1023, u))
            out = out | (math.tointeger(u) << (20 - 10 * k))
            k = k + 1
        end
    end
    return out
end
T.pack_quat = W2.pack_quat

function W2.cm_int(v) return math.tointeger(math.floor(v + 0.5)) or 0 end

-- world.rs body_hash: FNV-1a of <u32 id><i32 x><i32 y><i32 z><u32 q><u8 status>.
function W2.body_hash(id, pos, q, status)
    return fnv1a(string.pack("<I4i4i4i4I4B", id, W2.cm_int(pos.X), W2.cm_int(pos.Y), W2.cm_int(pos.Z), q & 0xFFFFFFFF, status))
end
T.body_hash = W2.body_hash

-- world.rs world_hash: FNV-1a of <u32 id><u32 body_hash> over rows sorted by id.
function W2.world_hash(rows)
    local parts = {}
    for i, r in ipairs(rows) do parts[i] = string.pack("<I4I4", r.id, W2.body_hash(r.id, r.pos, r.q, r.status)) end
    return fnv1a(table.concat(parts))
end
T.world_hash = W2.world_hash

function W2.quat_mul(a, b)  -- Hamilton product a*b, {x,y,z,w}
    return { a[4] * b[1] + a[1] * b[4] + a[2] * b[3] - a[3] * b[2],
             a[4] * b[2] - a[1] * b[3] + a[2] * b[4] + a[3] * b[1],
             a[4] * b[3] + a[1] * b[2] - a[2] * b[1] + a[3] * b[4],
             a[4] * b[4] - a[1] * b[1] - a[2] * b[2] - a[3] * b[3] }
end
function W2.quat_conj(a) return { -a[1], -a[2], -a[3], a[4] } end
T.quat_mul = W2.quat_mul

-- World-space angular velocity (deg/s) that turns qa into qb in dt_ms: what
-- a hinged door / lever / lid was doing between the owner's last two
-- samples. Applied when a followed body gets its physics back, so a door
-- swinging when its lease ends keeps swinging the same way on every screen.
function W2.ang_vel_deg(qa, qb, dt_ms)
    if not qa or not qb or not dt_ms or dt_ms <= 0 then return { X = 0, Y = 0, Z = 0 } end
    local d = W2.quat_mul(qb, W2.quat_conj(qa))
    if d[4] < 0 then d = { -d[1], -d[2], -d[3], -d[4] } end
    local w = math.max(-1, math.min(1, d[4]))
    local ang = 2 * math.acos(w)
    local s = math.sqrt(math.max(0, 1 - w * w))
    if s < 1e-6 or ang < 1e-6 then return { X = 0, Y = 0, Z = 0 } end
    local k = math.deg(ang) / (dt_ms / 1000) / s
    return { X = d[1] * k, Y = d[2] * k, Z = d[3] * k }
end
T.ang_vel_deg = W2.ang_vel_deg

-- State-bearing actor kinds (inventory in world-replication.md). nil = no actor state.
function W2.state_kind(cls)
    if not cls then return nil end
    if cls:find("^ST_LeverActivated") then return "gate" end
    if cls:find("^ST_Lever") then return "lever" end
    if cls == "Trap_BP_C" then return "trap" end
    if cls:find("^BP_Container_") then return "chest" end
    if cls:find("^BP_Fence_") then return "fence" end
    if cls:find("Destruct") or cls:find("^BP_Structure_Plank") or cls:find("^BP_Structure_Board") then return "destructible" end
    if cls:find("^Chain_BP") or cls:find("^Trap_Kettle_BP") or cls:find("Cooking_Utencil")
        or cls:find("Training_Dummy") then return "constrained" end
    return nil
end
T.state_kind = W2.state_kind

-- An attachment that does not make an actor "held": a piece of a compound
-- structure (never a Willie, never a candle in its sconce).
function W2.attach_ok(parent_cls)
    if not parent_cls or parent_cls:find("Willie", 1, true) then return false end
    for _, p in ipairs(K.STRUCT_PARENTS) do if parent_cls:find(p) then return true end end
    return false
end
T.attach_ok = W2.attach_ok

-- Local state bits of one constraint group: bit k = constraint k broken
-- (destroyed counts as broken), bit 6 = the kind's flag (group 0 only).
function W2.group_bits(broken, flag)
    local b = 0
    for k, isb in ipairs(broken) do
        if isb and k <= 6 then b = b | (1 << (k - 1)) end
    end
    if flag then b = b | K.ST_FLAG end
    return b
end
T.group_bits = W2.group_bits

-- What applying the server's bits to a local group must do: constraints
-- to break (indices, 1-based) and whether the kind's flag must be raised.
function W2.state_todo(srv, loc)
    local brk = {}
    for k = 1, 6 do
        local m = 1 << (k - 1)
        if srv & m ~= 0 and loc & m == 0 then brk[#brk + 1] = k end
    end
    return brk, (srv & K.ST_FLAG ~= 0) and (loc & K.ST_FLAG == 0)
end
T.state_todo = W2.state_todo

-- The world_consistency slot (a world_verdict record) as the shape read_consistency uses.
function W2.consistency_from(t)
    if type(t) ~= "table" then return nil end
    local r = { level = t.level or 0, epoch = t.epoch or 0, seq = t.seq or 0, peer = t.other or 0,
                hash_match = t.hash_match == true, hash_equal = t.hash_equal == true,
                compared = t.compared or 0, mismatched_n = t.mismatched_n or 0, rows = {} }
    for _, m in ipairs(t.rows or {}) do
        r.rows[#r.rows + 1] = { id = m.id, kind = m.kind, dpos = m.dpos, dang = m.dang }
    end
    return r
end

-- The facade (shared/hsmp_ipc.lua) or nil (not loaded: nothing is sent).
function W2.ipc() return rawget(_G, "HSMP_IPC") end


return {
    fnv1a = fnv1a, clock_ms = clock_ms, v3 = v3, dist3 = dist3, vlen = vlen, F = F, rot_diff = rot_diff,
    quat_to_rot = quat_to_rot, rot_to_quat_t = rot_to_quat_t, stable_name = stable_name,
    assign_ids = assign_ids, bind_manifest = bind_manifest,
}
end
return M
