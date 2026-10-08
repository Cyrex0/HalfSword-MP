-- Actual collision-module mesh envelopes sampled in weapon space each frame.
-- No UObject survives a callback. Boxes stay local to each module, so a broad
-- axe/polearm head does not give its entire shaft the head's collision width.
-- Runs at the sample rate for each held weapon, so the steady state allocates nothing here:
-- two alternating output tables per weapon address (the pose writers copy the current one
-- synchronously; the previous sample's table is never touched by the next sample),
-- module-level callbacks instead of per-call closures, and scalar quaternion math with the
-- same operation order as the table form.
local M, cache, epoch = {}, {}, nil
local src=(debug.getinfo(1,"S").source or ""):gsub("^@","")
local dir=src:match("^(.*)[/\\]") or "."
local okmod,Modules=pcall(require,"native_weapon_modules")
if not okmod then
    okmod,Modules=pcall(dofile,dir.."/native_weapon_modules.lua")
    if not okmod then okmod,Modules=pcall(dofile,dir.."/../../shared/native_weapon_modules.lua") end
end
local CAPACITY = 12 * 15
local function finite(x) return type(x)=="number" and x==x and math.abs(x)<1e6 end
local hashes = {}
local function class_hash(name)
    local h = hashes[name]
    if h then return h end
    h=2166136261
    for i=1,#name do h=((h ~ name:byte(i))*16777619)&0xffffffff end
    hashes[name] = h
    return h
end
local none
local function fname_none() none = none or FName("None"); return none end

-- q * (v,0) * conj(q), written out as the table form computed it.
local function rot(qx,qy,qz,qw, v1,v2,v3)
    local m1 = qw*v1+qx*0+qy*v3-qz*v2
    local m2 = qw*v2-qx*v3+qy*0+qz*v1
    local m3 = qw*v3+qx*v2-qy*v1+qz*0
    local m4 = qw*0-qx*v1-qy*v2-qz*v3
    local c1, c2, c3, c4 = -qx, -qy, -qz, qw
    return m4*c1+m1*c4+m2*c3-m3*c2,
        m4*c2-m1*c3+m2*c4+m3*c1,
        m4*c3+m1*c2-m2*c1+m3*c4
end

-- One axis of a module's local bounds: its scaled center and half extent.
local function axis(lo, hi, s, native_scale)
    if not (finite(lo) and finite(hi) and finite(s)) or hi<lo then error("invalid local bounds") end
    local center, half = (lo+hi)*0.5*s, (hi-lo)*0.5*math.abs(s)
    if half>300 then error("oversized module") end
    if native_scale and (s<1/1024 or s>=16) then error("unsupported native Box scale") end
    return center, half
end

-- The weapon being sampled (weapon-space origin and inverse rotation) and its output.
local boxes, nb, missing = nil, 0, 0
local wx, wy, wz, ix, iy, iz, iw = 0, 0, 0, 0, 0, 0, 1
local row_ordinal, row_child = 0, 0
local LO, HI = {X=0,Y=0,Z=0}, {X=0,Y=0,Z=0}

local function component_class(c) return c:GetClass():GetFName():ToString() end
local function weapon_class_hash(w) boxes.class_hash = class_hash(w:GetClass():GetFName():ToString()) end

local function sample_module(c)
    local okc, cls = pcall(component_class, c)
    if not okc then cls = nil end
    if cls=="SkeletalMeshComponent" then
        local asset=c:GetSkeletalMeshAsset()
        if not asset or not asset:IsValid() then return false end
        error("active skeletal weapon module bounds unsupported")
    end
    if cls=="StaticMeshComponent" then
        local asset=c.StaticMesh
        if not asset or not asset:IsValid() then return false end
    end
    local lx, ly, lz, hx, hy, hz, native_scale
    if cls=="BoxComponent" then
        local ext=c:GetUnscaledBoxExtent()
        lx, ly, lz, hx, hy, hz = -ext.X, -ext.Y, -ext.Z, ext.X, ext.Y, ext.Z
        native_scale=true
    else
        LO.X, LO.Y, LO.Z, HI.X, HI.Y, HI.Z = 0, 0, 0, 0, 0, 0
        c:GetLocalBounds(LO, HI)
        lx, ly, lz, hx, hy, hz = LO.X, LO.Y, LO.Z, HI.X, HI.Y, HI.Z
    end
    local t = c:GetSocketTransform(fname_none(), 0)
    local tr, tq, ts = t.Translation, t.Rotation, t.Scale3D
    local cx, cy, cz = tr.X, tr.Y, tr.Z
    local qx, qy, qz, qw = tq.X, tq.Y, tq.Z, tq.W
    local sx, sy, sz = ts.X, ts.Y, ts.Z
    local c1, h1 = axis(lx, hx, sx, native_scale)
    local c2, h2 = axis(ly, hy, sy, native_scale)
    local c3, h3 = axis(lz, hz, sz, native_scale)
    -- Empty inherited mesh slots are real native array entries,
    -- but have no collision envelope. Keep their ordinal reserved.
    if h1+h2+h3<=0 then return false end
    local p1, p2, p3 = rot(qx,qy,qz,qw, c1,c2,c3)
    p1, p2, p3 = rot(ix,iy,iz,iw, p1+cx-wx, p2+cy-wy, p3+cz-wz)
    local q1 = iw*qx+ix*qw+iy*qz-iz*qy
    local q2 = iw*qy-ix*qz+iy*qw+iz*qx
    local q3 = iw*qz+ix*qy-iy*qx+iz*qw
    local q4 = iw*qw-ix*qx-iy*qy-iz*qz
    if not finite(p1) or math.abs(p1)>400 or not finite(p2) or math.abs(p2)>400
        or not finite(p3) or math.abs(p3)>400 then error("module outside weapon") end
    if nb >= CAPACITY then error("active module capacity exceeded") end
    local b = boxes
    b[nb+1], b[nb+2], b[nb+3], b[nb+4] = row_ordinal, p1, p2, p3
    b[nb+5], b[nb+6], b[nb+7], b[nb+8] = q1, q2, q3, q4
    b[nb+9], b[nb+10], b[nb+11] = h1, h2, h3
    if native_scale then b[nb+12], b[nb+13], b[nb+14] = sx, sy, sz
    else b[nb+12], b[nb+13], b[nb+14] = 0, 0, 0 end
    b[nb+15] = row_child
    nb = nb + 15
    return true
end

local function sample_weapon(w)
    local wt = w:GetTransform()
    local tr, rq = wt.Translation, wt.Rotation
    wx, wy, wz = tr.X, tr.Y, tr.Z
    ix, iy, iz, iw = -rq.X, -rq.Y, -rq.Z, rq.W
    if not okmod then error("native module resolver unavailable") end
    local rows,reason=Modules.of(w)
    if not rows then error(reason) end
    for _,row in ipairs(rows) do
        row_ordinal, row_child = row.id, row.child_of
        local good,active = pcall(sample_module, row.component)
        if not good then error("complete native geometry unavailable: module "..row_ordinal.." child_of="..row_child.." activeRows="..(nb/15)..": "..tostring(active)) end
        if not active then missing=missing+1 end
    end
end

-- The weapon's module/cutting rows, 15 numbers each, plus `class_hash`. The table is reused
-- two samples later for the same weapon address.
function M.of(w, world, now, log)
    if world ~= epoch then cache, epoch = {}, world end
    local addr = w:GetAddress()
    local name = w:GetFName():ToString()
    local e = cache[addr]
    local first = e == nil
    if first then e = { bufs = { {}, {} }, flip = 1, count = 0, missing = 0 }; cache[addr] = e end
    e.flip = 3 - e.flip
    local out = e.bufs[e.flip]
    -- A static mesh COMPONENT can simulate independently (native flail Head
    -- and Link 1 do). Never reuse its actor-relative transform across frames.
    boxes, nb, missing = out, 0, 0
    out.class_hash = nil
    pcall(weapon_class_hash, w)
    local ok, why = pcall(sample_weapon, w)
    if not ok then nb = 0; out.class_hash = nil; missing = missing + 1 end
    for k = nb + 1, #out do out[k] = nil end
    if log and (first or e.count ~= nb or e.missing ~= missing) then
        log("weapon bounds: %d module/cutting row(s), %d unavailable%s", nb/15, missing, ok and "" or (": "..tostring(why)))
    end
    e.at, e.count, e.missing, e.name = now, nb, missing, name
    boxes = nil
    return out
end
return M
