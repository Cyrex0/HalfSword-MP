-- HSMPAvatars' pure stand-in math: play-sample parsing, quaternions, the velocity
-- servo, joint-consistent targets, target advance / aim, the reference skeleton and
-- the dev tune knobs. No UE calls and no state: main.lua loads it with
-- load_module("avatars_pure"), and the offline tests (lua-test avatars / framecost,
-- crates/hsmp-native servo_parity, tools/pose-truth) run it as it ships.
-- ============================================================================
-- BEGIN PURE  (no UE calls; unit-tested by `hsmp-tools lua-test`)
-- ============================================================================
local PURE = {}

-- Parent of each hero bone for the length-preserving retarget. Spine_04 ->
-- Upperarm skips the clavicle: that offset is near-rigid.
PURE.PARENT = {
    Spine_02 = "Pelvis", Spine_04 = "Spine_02", Head = "Spine_04",
    Upperarm_L = "Spine_04", Lowerarm_L = "Upperarm_L", Hand_L = "Lowerarm_L",
    Upperarm_R = "Spine_04", Lowerarm_R = "Upperarm_R", Hand_R = "Lowerarm_R",
    Thigh_L = "Pelvis", Calf_L = "Thigh_L", Foot_L = "Calf_L",
    Thigh_R = "Pelvis", Calf_R = "Thigh_R", Foot_R = "Calf_R",
}
PURE.ORDER = {   -- parents before children
    "Spine_02", "Spine_04", "Head",
    "Upperarm_L", "Lowerarm_L", "Hand_L", "Upperarm_R", "Lowerarm_R", "Hand_R",
    "Thigh_L", "Calf_L", "Foot_L", "Thigh_R", "Calf_R", "Foot_R",
}
PURE.LEN_RATIO_MIN, PURE.LEN_RATIO_MAX = 0.6, 1.6

local NUM = "(%-?[%d%.]+)"
local XF7 = "^" .. NUM .. "," .. NUM .. "," .. NUM .. "," .. NUM .. "," .. NUM .. "," .. NUM .. "," .. NUM .. "$"

-- Parse one codec-v1 pose_play line (the JSON text form). Returns nil (missing / torn / malformed),
-- "same" (seq == last_seq: nothing new), or a table.
function PURE.parse_play(line, last_seq)
    if not line or not line:find('"end":1}%s*$') then return nil end
    local seq = tonumber(line:match('^{"peer_id":%d+,"seq":(%d+)'))
    if not seq then return nil end
    if seq == last_seq then return "same" end
    local t = {
        seq   = seq,
        pt    = tonumber(line:match('"pt":(%-?%d+)')) or 0,
        mode  = line:match('"mode":"(%a+)"') or "?",
        age   = tonumber(line:match('"age":' .. NUM)) or -1,
        delay = tonumber(line:match('"delay":' .. NUM)) or 0,
        jit   = tonumber(line:match('"jit":' .. NUM)) or 0,
        cut   = tonumber(line:match('"cut":(%d+)')) or 0,
        bones = {}, nbones = 0,
    }
    local rx, ry, rz, ryaw = line:match('"root":%[' .. NUM .. ',' .. NUM .. ',' .. NUM .. ',' .. NUM .. '%]')
    if rx then t.root = { tonumber(rx), tonumber(ry), tonumber(rz), tonumber(ryaw) } end
    local body = line:match('"bones":(%b{})')
    if body then
        for name, vals in body:gmatch('"([%w_]+)":%[([^%]]+)%]') do
            local a, b, c, d, e, f, g = vals:match(XF7)
            if g then
                t.bones[name] = { tonumber(a), tonumber(b), tonumber(c), tonumber(d), tonumber(e), tonumber(f), tonumber(g) }
                t.nbones = t.nbones + 1
            end
        end
    end
    return t
end

local function d3(a, b)
    local dx, dy, dz = a[1] - b[1], a[2] - b[2], a[3] - b[3]
    return math.sqrt(dx * dx + dy * dy + dz * dz)
end
PURE.d3 = d3

-- Which remote hand holds the weapon: the nearer one (right on a tie-ish).
function PURE.anchor_hand(b)
    local w, r, l = b.Weapon, b.Hand_R, b.Hand_L
    if not w then return nil end
    if r and l then
        if d3(w, l) * 1.25 < d3(w, r) then return "Hand_L" end
        return "Hand_R"
    end
    return (r and "Hand_R") or (l and "Hand_L") or nil
end

-- Length-preserving retarget: keep every remote bone DIRECTION (and
-- rotation) but re-impose the stand-in's own bone lengths `lens` (bone ->
-- uu from its parent), walking parents first from the pelvis. Targets then
-- always fit the stand-in's skeleton, so the handles can't stretch a limb
-- whatever the sender's proportions, quantisation or interpolation did.
-- The weapon keeps its remote offset from its (moved) hand.
function PURE.retarget(src, lens)
    local out = {}
    for k, v in pairs(src) do out[k] = v end
    if not src.Pelvis then return out end
    for _, bn in ipairs(PURE.ORDER) do
        local par = PURE.PARENT[bn]
        local s, sp, np = src[bn], src[par], out[par]
        if s and sp and np then
            local dx, dy, dz = s[1] - sp[1], s[2] - sp[2], s[3] - sp[3]
            local r = 1.0
            local L = lens and lens[bn]
            if L then
                local rl = math.sqrt(dx * dx + dy * dy + dz * dz)
                if rl > 1e-3 then
                    r = L / rl
                    if r < PURE.LEN_RATIO_MIN or r > PURE.LEN_RATIO_MAX then r = 1.0 end
                end
            end
            out[bn] = { np[1] + dx * r, np[2] + dy * r, np[3] + dz * r, s[4], s[5], s[6], s[7] }
        end
    end
    local hand = PURE.anchor_hand(src)
    if hand and out[hand] then
        local w, h, nh = src.Weapon, src[hand], out[hand]
        out.Weapon = { nh[1] + (w[1] - h[1]), nh[2] + (w[2] - h[2]), nh[3] + (w[3] - h[3]), w[4], w[5], w[6], w[7] }
        out._wpn_hand = hand
    end
    return out
end

-- FQuat -> FRotator (degrees), same formula as UE's FQuat::Rotator().
function PURE.quat_to_rot(x, y, z, w)
    local sing = z * x - w * y
    local yaw_y = 2 * (w * z + x * y)
    local yaw_x = 1 - 2 * (y * y + z * z)
    local r2d = 180 / math.pi
    local yaw = math.atan(yaw_y, yaw_x) * r2d
    local pitch, roll
    if sing < -0.4999995 then
        pitch = -90
        roll = (-yaw - 2 * math.atan(x, w) * r2d + 180) % 360 - 180
    elseif sing > 0.4999995 then
        pitch = 90
        roll = (yaw - 2 * math.atan(x, w) * r2d + 180) % 360 - 180
    else
        pitch = math.asin(2 * sing) * r2d
        roll = math.atan(-2 * (w * x + y * z), 1 - 2 * (x * x + y * y)) * r2d
    end
    return { Pitch = pitch, Yaw = yaw, Roll = roll }
end

function PURE.clamp(v, lo, hi) if v < lo then return lo elseif v > hi then return hi end return v end

-- ---- codec v2 playback (crates/hsmp-pose/src/poseplay.rs write_v2) -----------
-- Slots: the 23 codec-v2 bones (crates/hsmp-pose/src/posecodec_v2.rs BONES), then the
-- right and left weapon. Slot index here = poseplay slot + 1.
PURE.V2_SLOTS = {
    "pelvis", "spine_01", "spine_02", "spine_03", "spine_04", "spine_05",
    "neck_01", "neck_02", "head",
    "clavicle_l", "upperarm_l", "lowerarm_l", "hand_l",
    "clavicle_r", "upperarm_r", "lowerarm_r", "hand_r",
    "thigh_l", "calf_l", "foot_l",
    "thigh_r", "calf_r", "foot_r",
    "weapon_r", "weapon_l",
}
PURE.V2_NB = 23
PURE.V2_WPN_R, PURE.V2_WPN_L = 24, 25
PURE.V2_NOBODY = { [2] = true }   -- spine_01: no physics body
-- Parent slot of each v2 bone (posecodec_v2.rs PARENT + 1; pelvis = itself).
PURE.V2_PARENT = { 1, 1, 2, 3, 4, 5, 6, 7, 8, 6, 10, 11, 12, 6, 14, 15, 16, 1, 18, 19, 1, 21, 22 }

local function numlist(s)
    local t, n = {}, 0
    if s then for x in s:gmatch("[^,]+") do n = n + 1; t[n] = tonumber(x) end end
    return t, n
end

-- Parse a v2 pose_play line (the JSON text form, positional arrays). Returns nil, "same", or
-- { v2 = true, seq, pt, mode, age, delay, jit, cut, root, slots = {[slot] = {13}},
--   weapons = {[24|25] = {hands,id,bx,by,bz,tx,ty,tz}}, control = {...} | nil }.
function PURE.parse_play2(line, last_seq)
    if not line or not line:find('"end":1}%s*$') then return nil end
    local seq = tonumber(line:match('^{"peer_id":%d+,"seq":(%d+)'))
    if not seq then return nil end
    if seq == last_seq then return "same" end
    local t = {
        v2 = true, seq = seq,
        match_id = tonumber(line:match('"match_id":(%d+)')),
        round = tonumber(line:match('"round":(%d+)')),
        life = tonumber(line:match('"life":(%d+)')),
        has_context = line:match('"has_context":(true)') ~= nil,
        pt    = tonumber(line:match('"ptf":(%-?[%d%.]+)')) or tonumber(line:match('"pt":(%-?%d+)')) or 0,
        mode  = line:match('"mode":"(%a+)"') or "?",
        age   = tonumber(line:match('"age":(%-?[%d%.]+)')) or -1,
        delay = tonumber(line:match('"delay":(%-?[%d%.]+)')) or 0,
        jit   = tonumber(line:match('"jit":(%-?[%d%.]+)')) or 0,
        cut   = tonumber(line:match('"cut":(%d+)')) or 0,
        slots = {}, nbones = 0, weapons = {},
        lead  = tonumber(line:match('"lead":(%-?[%d%.]+)')) or 0,
        iv    = tonumber(line:match('"iv":(%-?[%d%.]+)')) or -1,
        st    = tonumber(line:match('"st":(%-?[%d%.]+)')) or 0,
    }
    local rx, ry, rz, ryaw = line:match('"root":%[(%-?[%d%.]+),(%-?[%d%.]+),(%-?[%d%.]+),(%-?[%d%.]+)%]')
    if rx then t.root = { tonumber(rx), tonumber(ry), tonumber(rz), tonumber(ryaw) } end
    local m = tonumber(line:match('"m":(%d+)')) or 0
    local B, nb = numlist(line:match('"B":%[([^%]]*)%]'))
    local k = 0
    for s = 1, #PURE.V2_SLOTS do
        if (m // (1 << (s - 1))) % 2 == 1 then
            if k + 13 > nb then return nil end   -- torn / inconsistent
            local r = {}
            for j = 1, 13 do r[j] = B[k + j] end
            k = k + 13
            t.slots[s] = r
            if s <= PURE.V2_NB then t.nbones = t.nbones + 1 end
        end
    end
    local W, nw = numlist(line:match('"W":%[([^%]]*)%]'))
    local wi = 0
    for s = PURE.V2_WPN_R, PURE.V2_WPN_L do
        if t.slots[s] and wi + 8 <= nw then
            t.weapons[s] = { W[wi + 1], W[wi + 2], W[wi + 3], W[wi + 4], W[wi + 5], W[wi + 6], W[wi + 7], W[wi + 8] }
            wi = wi + 8
        end
    end
    local C = line:match('"C":%[([^%]]*)%]')
    if C then t.control = numlist(C) end
    return t
end

-- The same table parse_play2 builds, from HSMPNative.peer_play's reused
-- output table (the play-line fields as numbers; B / W / C flat). No string,
-- no pattern. The result is a fresh table, or `into` refilled in place (its
-- slot / weapon / root / control tables reused): callers keep it as p.last
-- while `o` is refilled next frame, so they alternate two of them per peer.
do   -- (one block: the main chunk is near the 200-locals limit)
local function fill(dst, src, base, n)   -- dst[1..n] = src[base+1 .. base+n]
    return table.move(src, base + 1, base + n, 1, dst or {})
end
-- Pose data belongs to one native life, including poses cached between reads.
function PURE.displayed_pose(pose, pawn, label, at)
    return {label=label,at=at,pawn=pawn,has_context=pose.has_context,
        match_id=pose.match_id,round=pose.round,life=pose.life}
end

function PURE.playback_row(peer, shown, now, allow_stale)
    if not shown or shown.has_context~=true or not shown.match_id or shown.match_id==0
        or not shown.life or shown.life<1 or type(shown.pawn)~="string" or shown.pawn==""
        or not shown.at or now-shown.at<0 or (not allow_stale and now-shown.at>=250) then return nil end
    return {peer=peer,body_ts=math.floor(shown.label),arm_ts=math.floor(shown.label),local_ms=math.floor(shown.at),
        match_id=shown.match_id,round=shown.round,life=shown.life,pawn=shown.pawn}
end

function PURE.pose_context_ok(o, session, mode, peer)
    if type(session) ~= "table" or (session.match_id or 0) == 0 then return true end
    if type(o) ~= "table" or o.has_context ~= true then return false end
    local round = session.round or 0
    if session.state == "countdown" or session.state == "paused" then
        if (session.spawn_round or 0) > 0 then round = session.spawn_round end
    end
    local life = 1
    if type(mode) == "table" and mode.match_id == session.match_id and mode.round == round then
        local row = mode.rows and mode.rows[peer]
        if row and (row.life or 0) > 0 then life = row.life end
        if session.state == "live" and not (row and (row.life or 0) > 0) then return false end
    elseif session.state == "live" then
        return false
    end
    return o.match_id == session.match_id and o.round == round and o.life == life
end

function PURE.play_from_out(o, into)
    if type(o) ~= "table" or type(o.B) ~= "table" then return nil end
    local t = into or { slots = {}, weapons = {} }
    t.v2, t.seq, t.pt, t.mode = true, o.seq, tonumber(o.pt) or 0, o.mode or "?"
    t.match_id, t.round, t.life, t.has_context = o.match_id, o.round, o.life, o.has_context == true
    t.age, t.delay, t.jit = tonumber(o.age) or -1, tonumber(o.delay) or 0, tonumber(o.jit) or 0
    t.cut, t.nbones = tonumber(o.cut) or 0, 0
    t.lead, t.iv, t.st, t.rate = tonumber(o.lead) or 0, tonumber(o.iv) or -1, tonumber(o.st) or 0, tonumber(o.rate) or 1
    t.read_at = nil
    local spare = t.spare or {}   -- slot / weapon tables of slots absent in this sample, for reuse
    t.spare = into and spare or nil
    local r, root = o.root, nil
    if type(r) == "table" then
        local x, w = r[1] or r.x, r[4] or r.yaw
        if x and w then
            root = t.root or spare.root or {}
            root[1], root[2], root[3], root[4] = x, r[2] or r.y, r[3] or r.z, w
        end
    end
    if t.root and not root then spare.root = t.root end
    t.root = root
    local slots, m, B, k = t.slots, math.tointeger(o.m) or 0, o.B, 0
    for s = 1, #PURE.V2_SLOTS do
        if (m >> (s - 1)) & 1 == 1 then
            if B[k + 13] == nil then return nil end   -- inconsistent: no evidence
            if into then
                slots[s] = fill(slots[s] or spare[s], B, k, 13)
                spare[s] = nil
            else
                slots[s] = { B[k + 1], B[k + 2], B[k + 3], B[k + 4], B[k + 5], B[k + 6], B[k + 7],
                             B[k + 8], B[k + 9], B[k + 10], B[k + 11], B[k + 12], B[k + 13] }
            end
            k = k + 13
            if s <= PURE.V2_NB then t.nbones = t.nbones + 1 end
        elseif slots[s] then
            spare[s], slots[s] = slots[s], nil
        end
    end
    local W, wi, wpn = o.W, 0, t.weapons
    for s = PURE.V2_WPN_R, PURE.V2_WPN_L do
        local had = wpn[s]
        wpn[s] = nil
        if type(W) == "table" and slots[s] and W[wi + 8] ~= nil then
            wpn[s] = fill(into and (had or spare[-s]), W, wi, 8)
            if into then spare[-s] = nil end
            wi = wi + 8
        elseif had and into then
            spare[-s] = had
        end
    end
    if type(o.C) == "table" and #o.C > 0 then
        local c = into and (t.control or spare.control) or {}
        table.move(o.C, 1, #o.C, 1, c)
        for i = #o.C + 1, #c do c[i] = nil end
        t.control = c
    else
        if t.control and into then spare.control = t.control end
        t.control = nil
    end
    return t
end
end

-- Weapon class tag, same as HSMPSync class_tag (1..255).
function PURE.class_tag(name)
    local h = 5381
    for i = 1, #(name or "") do h = (h * 33 + name:byte(i)) % 4294967296 end
    return h % 255 + 1
end

-- Quaternion helpers (x, y, z, w tables).
function PURE.qmul(a, b)
    return { a[4] * b[1] + a[1] * b[4] + a[2] * b[3] - a[3] * b[2],
             a[4] * b[2] - a[1] * b[3] + a[2] * b[4] + a[3] * b[1],
             a[4] * b[3] + a[1] * b[2] - a[2] * b[1] + a[3] * b[4],
             a[4] * b[4] - a[1] * b[1] - a[2] * b[2] - a[3] * b[3] }
end
function PURE.qrot(q, v)
    local x, y, z, w = q[1], q[2], q[3], q[4]
    local tx, ty, tz = 2 * (y * v[3] - z * v[2]), 2 * (z * v[1] - x * v[3]), 2 * (x * v[2] - y * v[1])
    return { v[1] + w * tx + (y * tz - z * ty), v[2] + w * ty + (z * tx - x * tz), v[3] + w * tz + (x * ty - y * tx) }
end
function PURE.qconj(q) return { -q[1], -q[2], -q[3], q[4] } end
function PURE.qangle(a, b)   -- degrees between two rotations
    local d = math.abs(a[1] * b[1] + a[2] * b[2] + a[3] * b[3] + a[4] * b[4])
    if d > 1 then d = 1 end
    return math.deg(2 * math.acos(d))
end

-- Velocity servo for one rigid body (replication.md "Driver").
--   cur  = { px,py,pz, qx,qy,qz,qw } current bone transform (world)
--   tg   = { 13 } target bone transform + origin velocity (uu/s) + angular (deg/s)
--   com  = body centre of mass in the bone frame
--   dt   = predicted physics step (s); cap_lin / cap_ang bound the correction
-- Returns the COM linear velocity (uu/s) and angular velocity (deg/s) that
-- carry the body exactly onto the target in one step (free body), with the
-- part beyond the replicated motion clamped (contact safety).
function PURE.servo(cur, tg, com, dt, cap_lin, cap_ang, gain)
    gain = gain or 1
    -- qrot(cq, com), qrot(tq, com) and qmul(tq, qconj(cq)) written out as scalars
    -- (same operations in the same order, no tables: this runs for every body of
    -- every stand-in each frame on the Lua path).
    local cx, cy, cz, cw = cur[4], cur[5], cur[6], cur[7]
    local qx, qy, qz, qw = tg[4], tg[5], tg[6], tg[7]
    local m1, m2, m3 = com[1], com[2], com[3]
    local ux, uy, uz = 2 * (cy * m3 - cz * m2), 2 * (cz * m1 - cx * m3), 2 * (cx * m2 - cy * m1)
    local cc1, cc2, cc3 = m1 + cw * ux + (cy * uz - cz * uy), m2 + cw * uy + (cz * ux - cx * uz), m3 + cw * uz + (cx * uy - cy * ux)
    ux, uy, uz = 2 * (qy * m3 - qz * m2), 2 * (qz * m1 - qx * m3), 2 * (qx * m2 - qy * m1)
    local tc1, tc2, tc3 = m1 + qw * ux + (qy * uz - qz * uy), m2 + qw * uy + (qz * ux - qx * uz), m3 + qw * uz + (qx * uy - qy * ux)
    local vx = (tg[1] + tc1 - cur[1] - cc1) / dt
    local vy = (tg[2] + tc2 - cur[2] - cc2) / dt
    local vz = (tg[3] + tc3 - cur[3] - cc3) / dt
    local nx, ny, nz = -cx, -cy, -cz
    local e1 = qw * nx + qx * cw + qy * nz - qz * ny
    local e2 = qw * ny - qx * nz + qy * cw + qz * nx
    local e3 = qw * nz + qx * ny - qy * nx + qz * cw
    local e4 = qw * cw - qx * nx - qy * ny - qz * nz
    if e4 < 0 then e1, e2, e3, e4 = -e1, -e2, -e3, -e4 end
    local s = math.sqrt(e1 * e1 + e2 * e2 + e3 * e3)
    local wx, wy, wz = 0, 0, 0
    if s > 1e-9 then
        local k = math.deg(2 * math.atan(s, e4)) / s / dt
        wx, wy, wz = e1 * k, e2 * k, e3 * k
    end
    -- Replicated motion (feed-forward) at the COM: v_com = v_origin + w x r.
    local wr = math.pi / 180
    local ax, ay, az = (tg[11] or 0) * wr, (tg[12] or 0) * wr, (tg[13] or 0) * wr
    local fx = (tg[8] or 0) + ay * tc3 - az * tc2
    local fy = (tg[9] or 0) + az * tc1 - ax * tc3
    local fz = (tg[10] or 0) + ax * tc2 - ay * tc1
    -- Feed-forward the replicated motion; correct only `gain` of the remaining
    -- error per step (1 = deadbeat; < 1 filters frame-to-frame noise).
    local dx, dy, dz = (vx - fx) * gain, (vy - fy) * gain, (vz - fz) * gain
    vx, vy, vz = fx + dx, fy + dy, fz + dz
    local dl = math.sqrt(dx * dx + dy * dy + dz * dz)
    if dl > cap_lin then local kk = cap_lin / dl; vx, vy, vz = fx + dx * kk, fy + dy * kk, fz + dz * kk end
    local gx, gy, gz = (wx - (tg[11] or 0)) * gain, (wy - (tg[12] or 0)) * gain, (wz - (tg[13] or 0)) * gain
    wx, wy, wz = (tg[11] or 0) + gx, (tg[12] or 0) + gy, (tg[13] or 0) + gz
    local gl = math.sqrt(gx * gx + gy * gy + gz * gz)
    if gl > cap_ang then local kk = cap_ang / gl; wx, wy, wz = (tg[11] or 0) + gx * kk, (tg[12] or 0) + gy * kk, (tg[13] or 0) + gz * kk end
    return vx, vy, vz, wx, wy, wz, dl, gl
end

-- Joint-consistent targets: the owner's rotations, but every bone's position
-- rebuilt from its parent's target with the STAND-IN's own parent offset
-- (`loc[i]`, parent frame). The owner's joints can be a few uu stretched or
-- dislocated (grips, hits: the codec sends those offsets) while the
-- stand-in's are rigid; servoing to positions it cannot reach makes the
-- solver trade them against rotations (measured: clavicles 21-24 deg, hands
-- 30-50 deg held off while positions were only 2-7 uu off). Bones without
-- a measured offset, and the pelvis, keep the owner's position.
function PURE.fk_retarget(targets, loc)
    local out = {}
    for s, t in pairs(targets) do out[s] = t end
    for i = 2, PURE.V2_NB do
        local t, par, l = targets[i], out[PURE.V2_PARENT[i]], loc[i]
        if t and par and l then
            local o = PURE.qrot({ par[4], par[5], par[6], par[7] }, l)
            local n = {}
            for k = 1, #t do n[k] = t[k] end
            n[1], n[2], n[3] = par[1] + o[1], par[2] + o[2], par[3] + o[3]
            out[i] = n
        end
    end
    return out
end

-- fk_retarget_in does it in place, for target tables built this frame (no
-- caller keeps the pre-retarget table). Parents come before children
-- (V2_PARENT[i] < i), so out[parent] is already rebuilt either way.
do
local function qrot3(x, y, z, w, v)   -- PURE.qrot as scalars
    local tx, ty, tz = 2 * (y * v[3] - z * v[2]), 2 * (z * v[1] - x * v[3]), 2 * (x * v[2] - y * v[1])
    return v[1] + w * tx + (y * tz - z * ty), v[2] + w * ty + (z * tx - x * tz), v[3] + w * tz + (x * ty - y * tx)
end
function PURE.fk_retarget_in(out, loc)
    for i = 2, PURE.V2_NB do
        local t, par, l = out[i], out[PURE.V2_PARENT[i]], loc[i]
        if t and par and l then
            local ox, oy, oz = qrot3(par[4], par[5], par[6], par[7], l)
            t[1], t[2], t[3] = par[1] + ox, par[2] + oy, par[3] + oz
        end
    end
    return out
end

-- Advance a v2 target by `ms` with its own velocities (frames without a new
-- pose_play record); bounded by the caller. Scalar math, one result table
-- (`dst` when given): this runs for 25 slots x 2 per stand-in per frame.
local function adv_rot(tg, s)   -- the rotation advanced by s seconds of its angular velocity
    local wx, wy, wz = math.rad(tg[11]) * s, math.rad(tg[12]) * s, math.rad(tg[13]) * s
    local a = math.sqrt(wx * wx + wy * wy + wz * wz)
    local bx, by, bz, bw = tg[4], tg[5], tg[6], tg[7]
    if a > 1e-9 then
        local h = math.sin(a / 2) / a
        local ax, ay, az, aw = wx * h, wy * h, wz * h, math.cos(a / 2)
        return aw * bx + ax * bw + ay * bz - az * by,
               aw * by - ax * bz + ay * bw + az * bx,
               aw * bz + ax * by - ay * bx + az * bw,
               aw * bw - ax * bx - ay * by - az * bz
    end
    return bx, by, bz, bw
end
local function adv_pos(tg, ms, acc)   -- position after ms (with the acceleration when given)
    local s = ms / 1000
    local x, y, z = tg[1] + tg[8] * s, tg[2] + tg[9] * s, tg[3] + tg[10] * s
    if acc then
        local h = 0.5 * s * s
        x, y, z = x + acc[1] * h, y + acc[2] * h, z + acc[3] * h
    end
    return x, y, z
end
function PURE.advance(tg, ms, acc, dst)
    local s = ms / 1000
    local x, y, z = adv_pos(tg, ms, acc)
    local qx, qy, qz, qw = adv_rot(tg, s)
    local vx, vy, vz = tg[8], tg[9], tg[10]
    if acc then vx, vy, vz = vx + acc[1] * s, vy + acc[2] * s, vz + acc[3] * s end
    if not dst then   -- one constructor: no rehash while it grows
        return { x, y, z, qx, qy, qz, qw, vx, vy, vz, tg[11], tg[12], tg[13] }
    end
    dst[1], dst[2], dst[3], dst[4], dst[5], dst[6], dst[7] = x, y, z, qx, qy, qz, qw
    dst[8], dst[9], dst[10], dst[11], dst[12], dst[13] = vx, vy, vz, tg[11], tg[12], tg[13]
    return dst
end
-- The servo's aim for one step: the pose at `ma` ms past the sample, with
-- the CHORD velocity from `ms` (where the body stands now) to `ma` as its
-- feed-forward, so a body that moves exactly with the feed-forward lands on
-- the aim whatever the step's real length.
function PURE.aim(tg, ms, ma, acc, dst)
    local e = PURE.advance(tg, ma, acc, dst)
    local h = (ma - ms) / 1000
    if h > 1e-4 then
        local bx, by, bz = adv_pos(tg, ms, acc)
        e[8], e[9], e[10] = (e[1] - bx) / h, (e[2] - by) / h, (e[3] - bz) / h
    end
    return e
end
-- Bounded acceleration estimate (uu/s^2): a lost or reordered sample must not
-- turn into a wild curve. clamp_acc3 writes into `dst`.
function PURE.clamp_acc3(dst, x, y, z)
    local l = math.sqrt(x * x + y * y + z * z)
    local lim = 40000
    if l > lim then local k = lim / l; x, y, z = x * k, y * k, z * k end
    dst[1], dst[2], dst[3] = x, y, z
    return dst
end
function PURE.clamp_acc(a)
    local l = math.sqrt(a[1] * a[1] + a[2] * a[2] + a[3] * a[3])
    local lim = 40000
    if l > lim then local k = lim / l; return { a[1] * k, a[2] * k, a[3] * k } end
    return a
end
-- Degrees between two rotations given as scalars (PURE.qangle without the tables).
function PURE.qangle8(ax, ay, az, aw, bx, by, bz, bw)
    local d = math.abs(ax * bx + ay * by + az * bz + aw * bw)
    if d > 1 then d = 1 end
    return math.deg(2 * math.acos(d))
end
end
-- Dev tuning knobs of the v2 servo (dev_cmd TUNE records, `hsmp-tools ipc-ctl --pid <game>
-- tune <key> <value>`). Flags are on when the value is nonzero (stored 1 / 0;
-- "tdiag" is stored 1 / nil, its readers test presence); numbers are clamped to a sane range.
PURE.TUNE_FLAGS = { servo = true, wpn = true, world = true, ghost = true, clock = true, motors = true, grips = true,
                    retarget = true, tonus = true, stamp = true, noacc = true, plant = true, v1aim = true, tdiag = true,
                    native_servo = true, native_neutralise = true, native_wservo = true, downed_world = true }   -- native servo A/B
PURE.TUNE_RANGE = { cap_lin = { 0, 20000 }, cap_ang = { 0, 20000 }, lead = { -500, 500 }, gain = { 0, 1 },
                    leg_gain = { 0, 1 }, lat = { 0, 500 }, limits = { 0, 180 }, bench = { 1, 1000 },
                    impact_dv = { 0, 5000 }, impact_ms = { 0, 1000 } }
-- The stored value for knob `key` set to `num`, or nil, "unknown" / "bad".
function PURE.tune_value(key, num)
    if type(num) ~= "number" or num ~= num or num == math.huge or num == -math.huge then return nil, "bad" end
    if PURE.TUNE_FLAGS[key] then
        if key == "tdiag" then return (num ~= 0) and 1 or false end
        return (num ~= 0) and 1 or 0
    end
    local r = PURE.TUNE_RANGE[key]
    if not r then return nil, "unknown" end
    local v = math.max(r[1], math.min(r[2], num))
    if key == "bench" then v = math.floor(v) end
    return v
end
-- The change-log line of a tune table.
function PURE.tune_key(t)
    local ks = {}
    for k, v in pairs(t) do if k ~= "key" and v ~= nil and v ~= false then ks[#ks + 1] = k .. "=" .. tostring(v) end end
    table.sort(ks)
    return table.concat(ks, " ")
end
-- ============================================================================

-- SK_Body_Man reference offsets, parent space, uu (posecodec_v2 REF_T; a hsmp-tools
-- test keeps the two equal).
PURE.V2_REF_T = {
    { 0, 0, 0 },
    { 0, -0.23, 3.67 }, { 0, 1.60, 6.60 }, { 0, 1.42, 7.10 }, { 0, 0.21, 8.52 }, { 0, -3.00, 19.41 },
    { 0, -0.61, 11.87 }, { 0, 0.61, 5.02 }, { 0, 0, 4.91 },
    { 1.43, -1.60, 5.44 }, { 17.81, 0, 0 }, { 27.77, -0.01, 0.01 }, { 27.25, 0, 0 },
    { -1.43, -1.60, 5.44 }, { -17.81, 0, 0 }, { -27.77, -0.01, 0.01 }, { -27.25, 0, 0 },
    { 9.97, 0.26, -2.35 }, { 2.36, 2.60, -43.20 }, { 1.76, -2.60, -42.10 },
    { -9.97, 0.26, -2.35 }, { -2.36, 2.60, -43.20 }, { -1.76, -2.60, -42.10 },
}
function PURE.len3(v) return math.sqrt(v[1] * v[1] + v[2] * v[2] + v[3] * v[3]) end

-- The character scale of measured parent offsets: median of measured / reference
-- length over the bones longer than 5 uu (1 when nothing was measured).
function PURE.ref_scale(loc)
    local r = {}
    for i = 2, #PURE.V2_REF_T do
        local ref, m = PURE.len3(PURE.V2_REF_T[i]), loc and loc[i]
        if m and ref > 5 then r[#r + 1] = PURE.len3(m) / ref end
    end
    if #r == 0 then return 1 end
    table.sort(r)
    return r[math.floor((#r + 1) / 2)]
end

-- Parent offsets measured on the stand-in, checked against the reference
-- skeleton: an offset taken from a stretched or dislocated body (a reused Willie,
-- a ragdoll mid-fall) would make every later target stretched as well. An
-- offset more than 15 % (and 2 uu) off the scaled reference, or missing, is
-- replaced by the reference. Returns the offsets and how many were replaced.
function PURE.ref_loc(loc)
    local k = PURE.ref_scale(loc)
    local out, fixed = {}, 0
    for i = 2, #PURE.V2_REF_T do
        local ref = PURE.V2_REF_T[i]
        local want = PURE.len3(ref) * k
        local m = loc and loc[i]
        local expected = { ref[1] * k, ref[2] * k, ref[3] * k }
        -- Equal length does not mean an intact joint: a pooled/ragdolled
        -- neck can be displaced sideways. Caching that direction permanently
        -- makes the servo aim at a bent skeleton on every later frame.
        if m and PURE.d3(m, expected) <= math.max(2, 0.15 * want) then
            out[i] = m
        else
            out[i] = expected
            fixed = fixed + 1
        end
    end
    return out, fixed, k
end

-- Largest joint stretch of a pose: |child - parent| against the reference length
-- (uu), over the bones present in `pos` (index -> {x, y, z}). Returns it and the
-- bone index.
function PURE.stretch(pos, ref_len)
    local worst, bone = 0, nil
    for i = 2, #PURE.V2_PARENT do
        local a, b, l = pos[i], pos[PURE.V2_PARENT[i]], ref_len[i]
        if a and b and l then
            local d = math.abs(math.sqrt((a[1] - b[1]) ^ 2 + (a[2] - b[2]) ^ 2 + (a[3] - b[3]) ^ 2) - l)
            if d > worst then worst, bone = d, i end
        end
    end
    return worst, bone
end
-- ============================================================================
-- END PURE
-- ============================================================================

return PURE
