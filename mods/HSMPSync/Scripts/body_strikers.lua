-- Native fist/foot primitive geometry, sampled in the same callback as the pose.
-- No transform cache: a kicking foot collider can move relative to its bone.
local M = {}
local function xyz(v)return {v.X,v.Y,v.Z}end
local function quat(v)return {v.X,v.Y,v.Z,v.W}end
local function conj(q)return {-q[1],-q[2],-q[3],q[4]}end
local function mul(a,b)return {a[4]*b[1]+a[1]*b[4]+a[2]*b[3]-a[3]*b[2],a[4]*b[2]-a[1]*b[3]+a[2]*b[4]+a[3]*b[1],a[4]*b[3]+a[1]*b[2]-a[2]*b[1]+a[3]*b[4],a[4]*b[4]-a[1]*b[1]-a[2]*b[2]-a[3]*b[3]}end
local function rot(q,v)local r=mul(mul(q,{v[1],v[2],v[3],0}),conj(q));return {r[1],r[2],r[3]}end
local function valid(o)local ok,v=pcall(function()return o and o:IsValid()end);return ok and v==true end
local function finite(x)return type(x)=="number" and x==x and math.abs(x)<1e6 end
local last_error,last_count
function M.of(pawn,mesh,log)
    local out,errors={},{}
    for part,entry in ipairs({{"Weapon R","hand_r"},{"Weapon L","hand_l"},{"Foot R Weapon","foot_r"},{"Foot L Weapon","foot_l"}}) do
        local ok,why=pcall(function()
            local actor=pawn[entry[1]]
            if not valid(actor) then return end
            if part<=2 and not actor:GetClass():GetFName():ToString():find("Weapon_Fists",1,true) then return end
            local bt=mesh:GetSocketTransform(FName(entry[2]),0)
            local bp,inv=xyz(bt.Translation),conj(quat(bt.Rotation))
            local ordinal=0
            actor["Collision Components Array"]:ForEach(function(_,e)
                ordinal=ordinal+1
                local c=e:get()
                if not valid(c) then return end
                local cls=c:GetClass():GetFName():ToString()
                local kind=cls:find("SphereComponent",1,true) and 0 or (cls:find("BoxComponent",1,true) and 1 or nil)
                if kind==nil then return end
                local success,err=pcall(function()
                    if ordinal>15 or #out>=8*13 then error("striker identity/capacity exceeded") end
                    local t=c:GetSocketTransform(FName("None"),0)
                    local cp,cq,scale=xyz(t.Translation),quat(t.Rotation),xyz(t.Scale3D)
                    local half
                    if kind==0 then local r=c:GetScaledSphereRadius();half={r,r,r}
                    else local e=xyz(c:GetUnscaledBoxExtent());half={e[1]*math.abs(scale[1]),e[2]*math.abs(scale[2]),e[3]*math.abs(scale[3])} end
                    local p=rot(inv,{cp[1]-bp[1],cp[2]-bp[2],cp[3]-bp[3]})
                    local q=mul(inv,cq)
                    for i=1,3 do
                        if not finite(p[i]) or math.abs(p[i])>48 or not finite(half[i]) or half[i]<=0 or half[i]>32 then error("striker outside anatomical bounds") end
                    end
                    for _,v in ipairs({part,ordinal,kind,p[1],p[2],p[3],q[1],q[2],q[3],q[4],half[1],half[2],half[3]})do out[#out+1]=v end
                end)
                if not success then errors[#errors+1]=entry[1]..":"..ordinal..":"..tostring(err) end
            end)
        end)
        if not ok then errors[#errors+1]=entry[1]..":"..tostring(why) end
    end
    local err=table.concat(errors,"; ")
    if log and (err~=last_error or #out~=last_count) then log("body strikers: %d native shape(s); unavailable=%s",#out/13,err=="" and "none" or err) end
    last_error,last_count=err,#out
    return out
end
return M
