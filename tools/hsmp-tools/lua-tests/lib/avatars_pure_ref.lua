-- avatars_pure_ref.lua -- HSMPAvatars' PURE target math as it was before the per-frame
-- allocation work (0.1.0-beta.4), kept verbatim as the reference the allocation-free
-- versions must match bit for bit (lua-test framecost, case pure_equal).
--
--   local R = dofile(".../avatars_pure_ref.lua")(PURE)   -- PURE: the mod's constants
return function(PURE)
local R = { V2_SLOTS = PURE.V2_SLOTS, V2_NB = PURE.V2_NB, V2_WPN_R = PURE.V2_WPN_R, V2_WPN_L = PURE.V2_WPN_L,
            V2_PARENT = PURE.V2_PARENT }
function R.play_from_out(o)
    if type(o) ~= "table" or type(o.B) ~= "table" then return nil end
    local t = {
        v2 = true, seq = o.seq, pt = tonumber(o.pt) or 0, mode = o.mode or "?",
        age = tonumber(o.age) or -1, delay = tonumber(o.delay) or 0, jit = tonumber(o.jit) or 0,
        cut = tonumber(o.cut) or 0, slots = {}, nbones = 0, weapons = {},
        lead = tonumber(o.lead) or 0, iv = tonumber(o.iv) or -1, st = tonumber(o.st) or 0, rate = tonumber(o.rate) or 1,
    }
    local r = o.root
    if type(r) == "table" then
        t.root = { r[1] or r.x, r[2] or r.y, r[3] or r.z, r[4] or r.yaw }
        if not (t.root[1] and t.root[4]) then t.root = nil end
    end
    local m, B, k = math.tointeger(o.m) or 0, o.B, 0
    for s = 1, #R.V2_SLOTS do
        if (m >> (s - 1)) & 1 == 1 then
            if B[k + 13] == nil then return nil end   -- inconsistent: no evidence
            t.slots[s] = { B[k + 1], B[k + 2], B[k + 3], B[k + 4], B[k + 5], B[k + 6], B[k + 7],
                           B[k + 8], B[k + 9], B[k + 10], B[k + 11], B[k + 12], B[k + 13] }
            k = k + 13
            if s <= R.V2_NB then t.nbones = t.nbones + 1 end
        end
    end
    local W, wi = o.W, 0
    if type(W) == "table" then
        for s = R.V2_WPN_R, R.V2_WPN_L do
            if t.slots[s] and W[wi + 8] ~= nil then
                t.weapons[s] = { W[wi + 1], W[wi + 2], W[wi + 3], W[wi + 4], W[wi + 5], W[wi + 6], W[wi + 7], W[wi + 8] }
                wi = wi + 8
            end
        end
    end
    if type(o.C) == "table" and #o.C > 0 then t.control = table.move(o.C, 1, #o.C, 1, {}) end
    return t
end

function R.qmul(a, b)
    return { a[4] * b[1] + a[1] * b[4] + a[2] * b[3] - a[3] * b[2],
             a[4] * b[2] - a[1] * b[3] + a[2] * b[4] + a[3] * b[1],
             a[4] * b[3] + a[1] * b[2] - a[2] * b[1] + a[3] * b[4],
             a[4] * b[4] - a[1] * b[1] - a[2] * b[2] - a[3] * b[3] }
end
function R.qrot(q, v)
    local x, y, z, w = q[1], q[2], q[3], q[4]
    local tx, ty, tz = 2 * (y * v[3] - z * v[2]), 2 * (z * v[1] - x * v[3]), 2 * (x * v[2] - y * v[1])
    return { v[1] + w * tx + (y * tz - z * ty), v[2] + w * ty + (z * tx - x * tz), v[3] + w * tz + (x * ty - y * tx) }
end
function R.qconj(q) return { -q[1], -q[2], -q[3], q[4] } end
function R.qangle(a, b)   -- degrees between two rotations
    local d = math.abs(a[1] * b[1] + a[2] * b[2] + a[3] * b[3] + a[4] * b[4])
    if d > 1 then d = 1 end
    return math.deg(2 * math.acos(d))
end
function R.fk_retarget(targets, loc)
    local out = {}
    for s, t in pairs(targets) do out[s] = t end
    for i = 2, R.V2_NB do
        local t, par, l = targets[i], out[R.V2_PARENT[i]], loc[i]
        if t and par and l then
            local o = R.qrot({ par[4], par[5], par[6], par[7] }, l)
            local n = {}
            for k = 1, #t do n[k] = t[k] end
            n[1], n[2], n[3] = par[1] + o[1], par[2] + o[2], par[3] + o[3]
            out[i] = n
        end
    end
    return out
end

-- Advance a v2 target by `ms` with its own velocities (frames without a new
-- pose_play record); bounded by the caller.
function R.advance(tg, ms, acc)
    local s = ms / 1000
    local r = { tg[1] + tg[8] * s, tg[2] + tg[9] * s, tg[3] + tg[10] * s }
    if acc then
        local h = 0.5 * s * s
        r[1], r[2], r[3] = r[1] + acc[1] * h, r[2] + acc[2] * h, r[3] + acc[3] * h
        s = ms / 1000
        local w = R.advance(tg, ms)
        return { r[1], r[2], r[3], w[4], w[5], w[6], w[7], tg[8] + acc[1] * s, tg[9] + acc[2] * s, tg[10] + acc[3] * s, tg[11], tg[12], tg[13] }
    end
    local wx, wy, wz = math.rad(tg[11]) * s, math.rad(tg[12]) * s, math.rad(tg[13]) * s
    local a = math.sqrt(wx * wx + wy * wy + wz * wz)
    local q = { tg[4], tg[5], tg[6], tg[7] }
    if a > 1e-9 then
        local h = math.sin(a / 2) / a
        q = R.qmul({ wx * h, wy * h, wz * h, math.cos(a / 2) }, q)
    end
    return { r[1], r[2], r[3], q[1], q[2], q[3], q[4], tg[8], tg[9], tg[10], tg[11], tg[12], tg[13] }
end
-- The servo's aim for one step: the pose at `ma` ms past the sample, with
-- the CHORD velocity from `ms` (where the body stands now) to `ma` as its
-- feed-forward, so a body that moves exactly with the feed-forward lands on
-- the aim whatever the step's real length.
function R.aim(tg, ms, ma, acc)
    local e = R.advance(tg, ma, acc)
    local h = (ma - ms) / 1000
    if h > 1e-4 then
        local b = R.advance(tg, ms, acc)
        e[8], e[9], e[10] = (e[1] - b[1]) / h, (e[2] - b[2]) / h, (e[3] - b[3]) / h
    end
    return e
end
-- Bounded acceleration estimate (uu/s^2): a lost or reordered sample must not
-- turn into a wild curve.
function R.clamp_acc(a)
    local l = math.sqrt(a[1] * a[1] + a[2] * a[2] + a[3] * a[3])
    local lim = 40000
    if l > lim then local k = lim / l; return { a[1] * k, a[2] * k, a[3] * k } end
    return a
end
return R
end
