-- Native fist/foot primitive geometry, sampled in the same callback as the pose.
-- No transform cache: a kicking foot collider can move relative to its bone.
-- Runs at the sample rate, so it allocates nothing in steady state: one reused output table
-- (its consumers, IPC.put_pose / sample_local, copy it before the next sample), module-level
-- callbacks instead of per-call closures, and scalar quaternion math with the same operation
-- order as the table form (q * (v,0) * conj(q), then conj(bone) * component).
local M = {}
local ENTRIES = { { "Weapon R", "hand_r" }, { "Weapon L", "hand_l" }, { "Foot R Weapon", "foot_r" }, { "Foot L Weapon", "foot_l" } }
local CAPACITY = 8 * 13
local out, n = {}, 0
local errors, nerr = {}, 0
local last_error, last_count
local names = {}
local function fname(s) local f = names[s]; if f == nil then f = FName(s); names[s] = f end; return f end
local function is_valid(o) return o:IsValid() end
local function valid(o) if not o then return false end local ok, v = pcall(is_valid, o); return ok and v == true end
local function finite(x) return type(x) == "number" and x == x and math.abs(x) < 1e6 end
local function note(s) nerr = nerr + 1; errors[nerr] = s end

-- The bone being sampled: its world position and the conjugate of its rotation.
local part, ordinal = 0, 0
local bx, by, bz, ix, iy, iz, iw = 0, 0, 0, 0, 0, 0, 1

local function sample_component(c, kind)
    if ordinal > 15 or n >= CAPACITY then error("striker identity/capacity exceeded") end
    local t = c:GetSocketTransform(fname("None"), 0)
    local tr, tq = t.Translation, t.Rotation
    local hx, hy, hz
    if kind == 0 then
        local r = c:GetScaledSphereRadius(); hx, hy, hz = r, r, r
    else
        local e, s = c:GetUnscaledBoxExtent(), t.Scale3D
        hx, hy, hz = e.X * math.abs(s.X), e.Y * math.abs(s.Y), e.Z * math.abs(s.Z)
    end
    -- p = inv * (d, 0) * conj(inv), d = component - bone
    local v1, v2, v3 = tr.X - bx, tr.Y - by, tr.Z - bz
    local m1 = iw * v1 + ix * 0 + iy * v3 - iz * v2
    local m2 = iw * v2 - ix * v3 + iy * 0 + iz * v1
    local m3 = iw * v3 + ix * v2 - iy * v1 + iz * 0
    local m4 = iw * 0 - ix * v1 - iy * v2 - iz * v3
    local c1, c2, c3, c4 = -ix, -iy, -iz, iw
    local p1 = m4 * c1 + m1 * c4 + m2 * c3 - m3 * c2
    local p2 = m4 * c2 - m1 * c3 + m2 * c4 + m3 * c1
    local p3 = m4 * c3 + m1 * c2 - m2 * c1 + m3 * c4
    -- q = inv * component rotation
    local qx, qy, qz, qw = tq.X, tq.Y, tq.Z, tq.W
    local q1 = iw * qx + ix * qw + iy * qz - iz * qy
    local q2 = iw * qy - ix * qz + iy * qw + iz * qx
    local q3 = iw * qz + ix * qy - iy * qx + iz * qw
    local q4 = iw * qw - ix * qx - iy * qy - iz * qz
    if not finite(p1) or math.abs(p1) > 48 or not finite(hx) or hx <= 0 or hx > 32
        or not finite(p2) or math.abs(p2) > 48 or not finite(hy) or hy <= 0 or hy > 32
        or not finite(p3) or math.abs(p3) > 48 or not finite(hz) or hz <= 0 or hz > 32 then
        error("striker outside anatomical bounds")
    end
    out[n + 1], out[n + 2], out[n + 3] = part, ordinal, kind
    out[n + 4], out[n + 5], out[n + 6] = p1, p2, p3
    out[n + 7], out[n + 8], out[n + 9], out[n + 10] = q1, q2, q3, q4
    out[n + 11], out[n + 12], out[n + 13] = hx, hy, hz
    n = n + 13
end

local entry_name
local function each_component(_, e)
    ordinal = ordinal + 1
    local c = e:get()
    if not valid(c) then return end
    local cls = c:GetClass():GetFName():ToString()
    local kind = cls:find("SphereComponent", 1, true) and 0 or (cls:find("BoxComponent", 1, true) and 1 or nil)
    if kind == nil then return end
    local ok, err = pcall(sample_component, c, kind)
    if not ok then note(entry_name .. ":" .. ordinal .. ":" .. tostring(err)) end
end

local function sample_part(pawn, mesh, i)
    local entry = ENTRIES[i]
    local actor = pawn[entry[1]]
    if not valid(actor) then return end
    if i <= 2 and not actor:GetClass():GetFName():ToString():find("Weapon_Fists", 1, true) then return end
    local bt = mesh:GetSocketTransform(fname(entry[2]), 0)
    local tr, rq = bt.Translation, bt.Rotation
    bx, by, bz = tr.X, tr.Y, tr.Z
    ix, iy, iz, iw = -rq.X, -rq.Y, -rq.Z, rq.W
    part, ordinal, entry_name = i, 0, entry[1]
    actor["Collision Components Array"]:ForEach(each_component)
end

-- The pawn's fist and foot primitives, 13 numbers each (part, ordinal, kind, bone-relative
-- position, rotation, half extents). The returned table is reused by the next call.
function M.of(pawn, mesh, log)
    local prev = n
    n, nerr = 0, 0
    for i = 1, #ENTRIES do
        local ok, why = pcall(sample_part, pawn, mesh, i)
        if not ok then note(ENTRIES[i][1] .. ":" .. tostring(why)) end
    end
    for k = n + 1, prev do out[k] = nil end
    for k = nerr + 1, #errors do errors[k] = nil end
    local err = nerr == 0 and "" or table.concat(errors, "; ", 1, nerr)
    if log and (err ~= last_error or n ~= last_count) then log("body strikers: %d native shape(s); unavailable=%s", n / 13, err == "" and "none" or err) end
    last_error, last_count = err, n
    return out
end
return M
