-- Actual collision-module mesh envelopes sampled in weapon space each frame.
-- No UObject survives a callback. Boxes stay local to each module, so a broad
-- axe/polearm head does not give its entire shaft the head's collision width.
local M, cache, epoch = {}, {}, nil
local src=(debug.getinfo(1,"S").source or ""):gsub("^@","")
local dir=src:match("^(.*)[/\\]") or "."
local okmod,Modules=pcall(require,"native_weapon_modules")
if not okmod then
    okmod,Modules=pcall(dofile,dir.."/native_weapon_modules.lua")
    if not okmod then okmod,Modules=pcall(dofile,dir.."/../../shared/native_weapon_modules.lua") end
end
local function xyz(v) return { v.X, v.Y, v.Z } end
local function quat(v) return { v.X, v.Y, v.Z, v.W } end
local function conj(q) return { -q[1],-q[2],-q[3],q[4] } end
local function mul(a,b)
    return { a[4]*b[1]+a[1]*b[4]+a[2]*b[3]-a[3]*b[2],
        a[4]*b[2]-a[1]*b[3]+a[2]*b[4]+a[3]*b[1],
        a[4]*b[3]+a[1]*b[2]-a[2]*b[1]+a[3]*b[4],
        a[4]*b[4]-a[1]*b[1]-a[2]*b[2]-a[3]*b[3] }
end
local function rot(q,v)
    local r = mul(mul(q,{v[1],v[2],v[3],0}),conj(q))
    return {r[1],r[2],r[3]}
end
local function finite(x) return type(x)=="number" and x==x and math.abs(x)<1e6 end
local function class_hash(name)
    local h=2166136261
    for i=1,#name do h=((h ~ name:byte(i))*16777619)&0xffffffff end
    return h
end

function M.of(w, world, now, log)
    if world ~= epoch then cache, epoch = {}, world end
    local addr = w:GetAddress()
    local name = w:GetFName():ToString()
    local old = cache[addr]
    -- A static mesh COMPONENT can simulate independently (native flail Head
    -- and Link 1 do). Never cache its actor-relative transform across frames.
    local boxes, missing = {}, 0
    pcall(function() boxes.class_hash=class_hash(w:GetClass():GetFName():ToString()) end)
    local ok, why = pcall(function()
        local wt = w:GetTransform()
        local wp, inv = xyz(wt.Translation), conj(quat(wt.Rotation))
        if not okmod then error("native module resolver unavailable") end
        local rows,reason=Modules.of(w)
        if not rows then error(reason) end
        for _,row in ipairs(rows)do
            local ordinal,c=row.id,row.component
            local good,active = pcall(function()
                local lo, hi = {X=0,Y=0,Z=0}, {X=0,Y=0,Z=0}
                local cls, native_scale
                pcall(function() cls=c:GetClass():GetFName():ToString() end)
                if cls=="SkeletalMeshComponent" then
                    local asset=c:GetSkeletalMeshAsset()
                    if not asset or not asset:IsValid() then return false end
                    error("active skeletal weapon module bounds unsupported")
                end
                if cls=="StaticMeshComponent" then
                    local asset=c.StaticMesh
                    if not asset or not asset:IsValid() then return false end
                end
                if cls=="BoxComponent" then
                    local ext=c:GetUnscaledBoxExtent()
                    lo,hi={X=-ext.X,Y=-ext.Y,Z=-ext.Z},{X=ext.X,Y=ext.Y,Z=ext.Z}
                    native_scale=true
                else c:GetLocalBounds(lo, hi) end
                local t = c:GetSocketTransform(FName("None"), 0)
                local cp, cq, scale = xyz(t.Translation), quat(t.Rotation), xyz(t.Scale3D)
                lo, hi = xyz(lo), xyz(hi)
                local center, half = {}, {}
                for i=1,3 do
                    if not (finite(lo[i]) and finite(hi[i]) and finite(scale[i])) or hi[i]<lo[i] then error("invalid local bounds") end
                    center[i] = (lo[i]+hi[i])*0.5*scale[i]
                    half[i] = (hi[i]-lo[i])*0.5*math.abs(scale[i])
                    if half[i]>300 then error("oversized module") end
                    if native_scale and (scale[i]<1/1024 or scale[i]>=16) then error("unsupported native Box scale") end
                end
                -- Empty inherited mesh slots are real native array entries,
                -- but have no collision envelope. Keep their ordinal reserved.
                if half[1]+half[2]+half[3]<=0 then return false end
                local p = rot(cq, center)
                for i=1,3 do p[i]=p[i]+cp[i]-wp[i] end
                p = rot(inv,p)
                local q = mul(inv,cq)
                for i=1,3 do if not finite(p[i]) or math.abs(p[i])>400 then error("module outside weapon") end end
                if #boxes >= 12*15 then error("active module capacity exceeded") end
                for _,v in ipairs({ordinal,p[1],p[2],p[3],q[1],q[2],q[3],q[4],half[1],half[2],half[3]}) do boxes[#boxes+1]=v end
                for i=1,3 do boxes[#boxes+1]=native_scale and scale[i] or 0 end
                boxes[#boxes+1]=row.child_of
                return true
            end)
            if not good then error("complete native geometry unavailable: module "..ordinal.." child_of="..row.child_of.." activeRows="..(#boxes/15)..": "..tostring(active)) end
            if not active then missing=missing+1 end
        end
    end)
    if not ok then boxes={}; missing=missing+1 end
    if log and (not old or #old.boxes~=#boxes or old.missing~=missing) then
        log("weapon bounds: %d module/cutting row(s), %d unavailable%s", #boxes/15, missing, ok and "" or (": "..tostring(why)))
    end
    cache[addr]={at=now,boxes=boxes,missing=missing,name=name}
    return boxes
end
return M
