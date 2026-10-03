-- hsmp_native_pose.lua -- the pose hot paths of the HSMPNative mocks (put_root / put_weapon /
-- put_pose), on the typed-record mixin (hsmp_native_records.lua).
--
-- The real module (crates/hsmp-native/src/pose_hot.rs, hsmp_pose::sample) builds the ABI 2
-- records once, at the source: `root` (rotator -> quaternion, ts in whole ms), `weapon`, and
-- `pose` = the codec v2 frame. The mock builds root / weapon the same way into the
-- `local_root` / `local_weapon` slots; it does not run the codec, so `local_pose` holds the
-- frame's 24-byte header only, and the call's arguments are kept in `N._hot.pose`.
--
--   local P = require("hsmp_native_pose")
--   P.install(N, S, caps_ok)    -- caps_ok(cap_bit) -> bool (the negotiated caps)

local P = {}

-- UE FRotator (degrees) -> FQuat (x, y, z, w), as FRotator::Quaternion().
function P.rot_to_quat(p, y, r)
    local d = math.pi / 360
    local sp, cp = math.sin(p * d), math.cos(p * d)
    local sy, cy = math.sin(y * d), math.cos(y * d)
    local sr, cr = math.sin(r * d), math.cos(r * d)
    return { cr * sp * sy - sr * cp * cy, -cr * sp * cy - sr * cp * sy, cr * cp * sy - sr * sp * cy, cr * cp * cy + sr * sp * sy }
end

local function nums(...)
    local a = table.pack(...)
    for i = 1, a.n do
        if type(a[i]) ~= "number" then return nil end
    end
    return a
end

local function deep(v)
    if type(v) ~= "table" then return v end
    local o = {}
    for k, x in pairs(v) do o[k] = deep(x) end
    return o
end

local function wall_ms() return math.floor(os.time() * 1000) end

-- The 24-byte header of the codec v2 frame the native module would encode for this sample
-- (magic, ts ms + 1/256 ms, flags, pelvis xyz f32, skeleton scale k = 1), as record rows. The
-- bit stream after it is the real encoder's (hsmp_pose::sample); the mock does not run it.
function P.frame_header(ts, dt, b, w, c)
    local ms = math.floor(ts)
    local nw = type(w) == "table" and math.min(#w // 21, 2) or 0
    local step = (type(dt) == "number" and dt > 0 and dt <= 250) and math.floor(dt + 0.5) or 0
    local flags = (type(c) == "table" and 1 or 0) | (nw << 1) | (step > 0 and 8 or 0)
    local s = string.pack("<BBBBI4BBfffI2", 0xFF, 0xFF, 0xFF, 0x02, ms & 0xFFFFFFFF, math.floor((ts - ms) * 256),
        flags, b[1], b[2], b[3], 32768)
    local rows = {}
    for i = 1, #s do rows[i] = s:byte(i) end
    return rows
end

function P.install(N, S, caps_ok)
    N._hot = { pose = nil, n = { local_root = 0, local_weapon = 0, local_pose = 0 } }
    local function ok_slot(name)
        local s = S.SLOTS and S.SLOTS[name]
        if not s then return nil, "bad" end
        if not caps_ok(s.cap) then return nil, "cap" end
        return s
    end
    local function put(name, t)
        local ok, e = N.put(name, t)
        if ok then N._hot.n[name] = N._hot.n[name] + 1 end
        return ok, e
    end
    function N.put_root(tick, ts, px, py, pz, p, y, r, vx, vy, vz)
        local s, e = ok_slot("local_root"); if not s then return nil, e end
        if not nums(tick, ts, px, py, pz, p, y, r, vx, vy, vz) then return nil, "bad" end
        return put("local_root", { tick = tick, ts = ts, send_wall_ms = wall_ms(), pos = { px, py, pz },
            rot = P.rot_to_quat(p, y, r), vel = { vx, vy, vz } })
    end
    function N.put_weapon(tick, ts, id, held, px, py, pz, p, y, r, vx, vy, vz)
        local s, e = ok_slot("local_weapon"); if not s then return nil, e end
        if not nums(tick, ts, id, held, px, py, pz, p, y, r, vx, vy, vz) then return nil, "bad" end
        if held < 0 or held > 2 then return nil, "bad:held" end
        return put("local_weapon", { tick = tick, ts = ts, weapon_id = id, held = held, pos = { px, py, pz },
            rot = P.rot_to_quat(p, y, r), vel = { vx, vy, vz } })
    end
    function N.put_pose(tick, ts, dt, kk, b, w, c)
        local s, e = ok_slot("local_pose"); if not s then return nil, e end
        if type(tick) ~= "number" or type(ts) ~= "number" or type(b) ~= "table" then return nil, "bad" end
        for i = 1, 23 * 13 do
            local x = b[i]
            if type(x) ~= "number" then return nil, "bad" end
            if x ~= x or x == math.huge or x == -math.huge then return nil, "bad:b" end
        end
        N._hot.pose = { tick = tick, ts = ts, dt = dt, k = kk, b = deep(b), w = deep(w), c = deep(c), b_ref = b, w_ref = w }
        return put("local_pose", { tick = tick, rows = P.frame_header(ts, dt, b, w, c) })
    end
    return N
end

return P
