-- interact_core.lua -- pure logic of HSMPInteract (no UE calls): bone names,
-- bone-frame math, the typed `interact` record tables (bone name <-> index),
-- the contact impulse accumulator, the application clamps and the
-- `pose_yield` bus record. Unit-tested offline (tests/hsmp-interact-test).

local C = {}

-- Same order as server/src/posecodec.rs HERO_BONES (the wire index).
C.HERO = {
    "Pelvis", "Spine_02", "Spine_04", "Head",
    "Upperarm_L", "Lowerarm_L", "Hand_L",
    "Upperarm_R", "Lowerarm_R", "Hand_R",
    "Thigh_L", "Calf_L", "Foot_L",
    "Thigh_R", "Calf_R", "Foot_R",
}
local HERO_LC = {}
for _, b in ipairs(C.HERO) do HERO_LC[b:lower()] = b end

-- Canonical hero bone name for a skeleton bone name, or nil.
function C.hero_name(name)
    if type(name) ~= "string" then return nil end
    return HERO_LC[name:lower()]
end

-- Any skeleton bone name -> the hero bone that carries it (twist bones,
-- clavicles, fingers, neck, extra spine segments), or nil if unknown.
function C.map_bone(name)
    if type(name) ~= "string" or name == "" or name == "None" then return nil end
    local h = C.hero_name(name)
    if h then return h end
    local n = name:lower()
    local side = n:match("_([lr])$") or n:match("_([lr])_") or n:match("^([lr])_")
    local S = side and side:upper() or nil
    local function sided(base) return S and (base .. "_" .. S) or nil end
    if n:find("upperarm") or n:find("clavicle") or n:find("shoulder") then return sided("Upperarm") end
    if n:find("lowerarm") or n:find("forearm") or n:find("elbow") then return sided("Lowerarm") end
    if n:find("hand") or n:find("thumb") or n:find("index") or n:find("middle") or n:find("ring")
        or n:find("pinky") or n:find("finger") or n:find("palm") or n:find("wrist") then return sided("Hand") end
    if n:find("thigh") then return sided("Thigh") end
    if n:find("calf") or n:find("knee") or n:find("shin") then return sided("Calf") end
    if n:find("foot") or n:find("ball") or n:find("toe") or n:find("ankle") then return sided("Foot") end
    if n:find("head") or n:find("neck") or n:find("jaw") or n:find("eye") then return "Head" end
    if n:find("spine_0[34]") or n:find("spine_05") or n:find("chest") then return "Spine_04" end
    if n:find("spine") then return n:find("spine_01") and "Pelvis" or "Spine_02" end
    if n:find("pelvis") or n:find("hip") or n:find("root") then return "Pelvis" end
    return nil
end

C.HANDS = { [0] = "R", [1] = "L" }
C.HAND_BONE = { [0] = "Hand_R", [1] = "Hand_L" }

-- --- vectors / quaternions ({x,y,z} arrays; quaternions {X,Y,Z,W} or arrays) ---

function C.v(x, y, z) return { x, y, z } end
function C.sub(a, b) return { a[1] - b[1], a[2] - b[2], a[3] - b[3] } end
function C.add(a, b) return { a[1] + b[1], a[2] + b[2], a[3] + b[3] } end
function C.scale(a, k) return { a[1] * k, a[2] * k, a[3] * k } end
function C.len(a) return math.sqrt(a[1] * a[1] + a[2] * a[2] + a[3] * a[3]) end
function C.dot(a, b) return a[1] * b[1] + a[2] * b[2] + a[3] * b[3] end
function C.dist(a, b) return C.len(C.sub(a, b)) end

function C.finite(x) return type(x) == "number" and x == x and x > -1e7 and x < 1e7 end
function C.finite3(a) return type(a) == "table" and C.finite(a[1]) and C.finite(a[2]) and C.finite(a[3]) end

-- Shorten `a` to at most `maxlen`.
function C.clamp_len(a, maxlen)
    local l = C.len(a)
    if l > maxlen and l > 0 then return C.scale(a, maxlen / l), l end
    return { a[1], a[2], a[3] }, l
end

local function qparts(q)
    if q[1] ~= nil then return q[1], q[2], q[3], q[4] end
    return q.X, q.Y, q.Z, q.W
end

-- Rotate v by unit quaternion q.
function C.qrot(q, v)
    local x, y, z, w = qparts(q)
    -- t = 2 * cross(q.xyz, v); v' = v + w t + cross(q.xyz, t)
    local tx = 2 * (y * v[3] - z * v[2])
    local ty = 2 * (z * v[1] - x * v[3])
    local tz = 2 * (x * v[2] - y * v[1])
    return { v[1] + w * tx + (y * tz - z * ty),
             v[2] + w * ty + (z * tx - x * tz),
             v[3] + w * tz + (x * ty - y * tx) }
end

-- Rotate v by the inverse of q.
function C.qrot_inv(q, v)
    local x, y, z, w = qparts(q)
    return C.qrot({ -x, -y, -z, w }, v)
end

-- World point -> bone-local frame, and back.
function C.to_local(bone_pos, bone_q, p) return C.qrot_inv(bone_q, C.sub(p, bone_pos)) end
function C.to_world(bone_pos, bone_q, lp) return C.add(bone_pos, C.qrot(bone_q, lp)) end

-- --- records (the typed `interact` record, schema/interact.rs) ---------------
-- G2S IPC.send("interact", t) / S2G IPC.events("interact"): one table shape,
-- { kind, hand, bone, target_peer, id, ts, point, vector, gid }; the bone is
-- its HERO index (0-based), the kind a code. No JSON, no regex.

C.KIND = { grab_start = 1, grab_update = 2, grab_end = 3, impulse = 4, grab_denied = 5 }
C.KIND_NAME = {}
for n, k in pairs(C.KIND) do C.KIND_NAME[k] = n end

-- Hero bone name <-> wire index (HERO order, 0-based).
C.BONE_INDEX = {}
for i, b in ipairs(C.HERO) do C.BONE_INDEX[b] = i - 1 end
function C.bone_index(name) return C.BONE_INDEX[name] end
function C.bone_name(idx)
    if math.type(idx) ~= "integer" then return nil end
    return C.HERO[idx + 1]
end

local function ms(ts) return math.floor(tonumber(ts) or 0) end
local function v3(a) return { a[1], a[2], a[3] } end

-- A grab start / update record (`kind` = "grab_start" | "grab_update").
function C.grab_rec(kind, target, gid, hand, bone, pt, v, ts)
    return { kind = C.KIND[kind], target_peer = target, gid = gid, hand = hand, bone = C.BONE_INDEX[bone],
             point = v3(pt), vector = v3(v), ts = ms(ts) }
end

function C.grab_end_rec(target, gid, hand, ts)
    return { kind = C.KIND.grab_end, target_peer = target, gid = gid, hand = hand, ts = ms(ts) }
end

function C.impulse_rec(target, bone, pt, v, ts)
    return { kind = C.KIND.impulse, target_peer = target, bone = C.BONE_INDEX[bone], point = v3(pt),
             vector = v3(v), ts = ms(ts) }
end

-- One S2G `interact` record (`peer` = the ring record's peer: the initiator, 0 = the
-- server about OUR grab) -> event table { kind (name), from, id, hand, bone (name),
-- pt, v, ts, target, gid }, or nil (unknown kind / bone).
function C.from_rec(peer, t)
    if type(t) ~= "table" then return nil end
    local kind = C.KIND_NAME[t.kind]
    local from = math.tointeger(peer)
    if not kind or not from then return nil end
    local hand = t.hand or 0
    if hand ~= 0 and hand ~= 1 then return nil end
    local e = { kind = kind, from = from, id = t.id, hand = hand, bone = C.bone_name(t.bone),
                pt = t.point, v = t.vector, ts = t.ts, target = t.target_peer,
                gid = (t.gid ~= nil and t.gid ~= 0) and t.gid or nil }
    if from ~= 0 then
        if not e.bone then return nil end
        if kind ~= "grab_end" and not (C.finite3(e.pt) and C.finite3(e.v)) then return nil end
    end
    return e
end
-- --- contact impulse accumulator (attacker side) ------------------------------
-- Body contacts of OUR pawn on a stand-in arrive as per-frame hit events. The
-- first contact over the threshold is sent at the next frame flush (lowest
-- latency); further contacts with that peer are summed over a window and
-- sent as one impulse when it closes. Contacts in a frame where the same
-- peer also took a damage hit from us are dropped (combat replicates those).

function C.new_acc(opts)
    return { min = opts.min or 1500, window = opts.window or 50, peers = {} }
end

local function fresh(now, frame, open)
    return { sum = { 0, 0, 0 }, pw = { 0, 0, 0 }, w = 0, bones = {}, n = 0, t0 = nil,
             frame0 = frame, frame1 = frame, open_at = open and now or nil }
end

-- One contact: impulse vector `imp` (world, already pointing into the
-- stand-in), at world point `p` on hero bone `bone`.
function C.acc_add(acc, peer, imp, p, bone, now, frame)
    if not (C.finite3(imp) and C.finite3(p) and bone) then return end
    local a = acc.peers[peer]
    if not a then a = fresh(now, frame, false); acc.peers[peer] = a end
    local m = C.len(imp)
    if a.n == 0 then a.t0, a.frame0 = now, frame end
    a.sum = C.add(a.sum, imp)
    a.pw = C.add(a.pw, C.scale(p, m))
    a.w = a.w + m
    a.bones[bone] = (a.bones[bone] or 0) + m
    a.frame1 = frame
    a.n = a.n + 1
end

-- Due impulses: list of { peer, bone, point (world), v, ts } (or
-- { peer, suppressed = true }). `damaged(peer, f0, f1)` -> true if a damage
-- hit from us on that peer happened in frames f0..f1. Contacts of the
-- current frame wait one frame (a damage hook that fires after the contact
-- hook still suppresses them) unless they have been accumulating for a whole
-- window (sustained pushing).
--   * sum >= min: sent, and a window opens: contacts during it are summed
--     and sent when it closes;
--   * below min: kept for one window (a slow push adds up), then dropped.
function C.acc_flush(acc, now, frame, damaged)
    local out = {}
    for peer, a in pairs(acc.peers) do
        local window_open = a.open_at ~= nil and now - a.open_at < acc.window
        if a.n > 0 and not window_open and (a.frame1 < frame or now - a.t0 >= acc.window) then
            if damaged and damaged(peer, a.frame0 - 1, a.frame1 + 1) then
                out[#out + 1] = { peer = peer, suppressed = true }
                acc.peers[peer] = nil
            elseif C.len(a.sum) >= acc.min and a.w > 0 then
                local best, bw = nil, -1
                for b, w in pairs(a.bones) do
                    if w > bw or (w == bw and b < best) then best, bw = b, w end
                end
                out[#out + 1] = { peer = peer, bone = best, point = C.scale(a.pw, 1 / a.w), v = a.sum, ts = a.t0 }
                acc.peers[peer] = fresh(now, frame, true)
            elseif now - a.t0 >= acc.window then
                acc.peers[peer] = nil   -- too weak: a touch, not a shove
            end
        elseif a.n == 0 and not window_open then
            acc.peers[peer] = nil
        end
    end
    table.sort(out, function(x, y) return x.peer < y.peer end)
    return out
end

-- The contact normal impulse is unsigned in practice: make it push the
-- stand-in away from our body (`from` = our contacting body point, `to` = the
-- stand-in bone).
function C.orient_away(imp, from, to)
    local d = C.sub(to, from)
    if C.dot(imp, d) < 0 then return C.scale(imp, -1) end
    return imp
end

-- --- application (owner side) -------------------------------------------------

-- Impulse to apply on our own bone: gain, magnitude cap, and a velocity-change
-- cap for the bone's mass (no launches). Returns vector, magnitude, capped?
function C.apply_impulse(v, gain, max_imp, mass, max_dv)
    local w = C.scale(v, gain or 1)
    local capped = false
    local l = C.len(w)
    if l > max_imp then w = C.scale(w, max_imp / l); l = max_imp; capped = true end
    if mass and mass > 0 and max_dv and l / mass > max_dv then
        w = C.scale(w, (max_dv * mass) / l); l = max_dv * mass; capped = true
    end
    return w, l, capped
end

-- Force-limited grab: the handle target never leads the grip point by more
-- than `lead` uu (force <= stiffness x lead). Returns target, distance.
function C.grab_target(grip, anchor, lead)
    local d = C.sub(anchor, grip)
    local cl, l = C.clamp_len(d, lead)
    return C.add(grip, cl), l
end

-- --- pose_yield (bus key, typed; HSMPAvatars compliance hook) -----------------
-- { rows = { { peer, until_ms, gain, cap_lin, cap_ang }, ... } }; until_ms on the
-- os.clock()*1000 clock every UE4SS mod shares.
-- `y` = { [peer] = { until=, gain=, lin=, ang= } } -> the record table (rows sorted by peer).
function C.yield_rec(y)
    local ids = {}
    for id in pairs(y) do ids[#ids + 1] = id end
    table.sort(ids)
    local rows = {}
    for _, id in ipairs(ids) do
        local e = y[id]
        rows[#rows + 1] = { peer = id, until_ms = math.floor(e["until"]), gain = e.gain,
                            cap_lin = e.lin, cap_ang = e.ang }
    end
    return { rows = rows }
end

-- Same rows (change detection without a text form).
function C.yield_same(a, b)
    if not (a and b) or #a.rows ~= #b.rows then return false end
    for i, r in ipairs(a.rows) do
        local o = b.rows[i]
        if o.peer ~= r.peer or o.until_ms ~= r.until_ms or o.gain ~= r.gain or o.cap_lin ~= r.cap_lin
            or o.cap_ang ~= r.cap_ang then return false end
    end
    return true
end
-- Merge a yield request: the latest deadline, the softest gain/caps.
function C.yield_merge(y, peer, until_ms, gain, lin, ang)
    local e = y[peer]
    if not e then
        y[peer] = { ["until"] = until_ms, gain = gain, lin = lin, ang = ang }
        return
    end
    if until_ms > e["until"] then e["until"] = until_ms end
    if gain < e.gain then e.gain = gain end
    if lin < e.lin then e.lin = lin end
    if ang < e.ang then e.ang = ang end
end

-- Drop expired requests; true if anything is left.
function C.yield_prune(y, now)
    local any = false
    for id, e in pairs(y) do
        if e["until"] <= now then y[id] = nil else any = true end
    end
    return any
end

return C
