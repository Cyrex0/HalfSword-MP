-- Historical cutting geometry. Only the private, non-physical UBoxComponent
-- created here is transformed; the actual striking component is never changed.
local M = {}
local function xyz(v)return {v.X,v.Y,v.Z}end
local function quat(v)return {v.X,v.Y,v.Z,v.W}end
local function mul(a,b)return {a[4]*b[1]+a[1]*b[4]+a[2]*b[3]-a[3]*b[2],a[4]*b[2]-a[1]*b[3]+a[2]*b[4]+a[3]*b[1],a[4]*b[3]+a[1]*b[2]-a[2]*b[1]+a[3]*b[4],a[4]*b[4]-a[1]*b[1]-a[2]*b[2]-a[3]*b[3]}end
local function conj(q)return {-q[1],-q[2],-q[3],q[4]}end
local function rot(q,p)local r=mul(mul(q,{p[1],p[2],p[3],0}),conj(q));return{r[1],r[2],r[3]}end
local function finite(v)return type(v)=="number" and v==v and math.abs(v)<1e6 end
function M.class_hash(name)
    local h=2166136261
    for i=1,#name do h=((h ~ name:byte(i))*16777619)&0xffffffff end
    return h
end
function M.capture(box,frame,bf)
    local ok,a=pcall(function()
        if not frame or not box or not box:IsValid() or box:GetClass():GetFName():ToString()~="BoxComponent" then error("missing original Box/bone frame") end
        local t=box:GetSocketTransform(FName("None"),0)
        -- Physical centimetres in the victim bone's rotation frame. Its
        -- socket scale and the transmitted skeleton-reference scale differ.
        local world=xyz(t.Translation)
        local p=rot(conj(frame.q),{world[1]-frame.p[1],world[2]-frame.p[2],world[3]-frame.p[3]})
        local q=mul(conj(frame.q),quat(t.Rotation))
        local s,e=xyz(t.Scale3D),xyz(box:GetUnscaledBoxExtent())
        local a={p[1],p[2],p[3],q[1],q[2],q[3],q[4],s[1],s[2],s[3],e[1],e[2],e[3]}
        for i,v in ipairs(a)do if not finite(v) then error("nonfinite original Box geometry") end end
        for i=1,3 do if math.abs(p[i])>600 or s[i]<1/1024 or s[i]>=16 or e[i]<=0 or e[i]>300 then error("unsupported original Box geometry") end end
        return a
    end)
    return ok and a or nil
end
function M.new(e)
    local cache,pool_world={},nil
    local function valid(o)return o and o:IsValid() end
    local function type_name(o)
        local c=valid(o) and o:GetClass()
        return valid(c) and c:GetFName():ToString() or nil
    end
    local function native_world(o)
        local world=valid(o) and o:GetWorld()
        return valid(world) and world:GetAddress() or nil
    end
    local function fresh(row,which)
        if not row or type(row[which])~="string" or row[which]=="" then return nil end
        local o=StaticFindObject(row[which])
        if valid(o) and o:GetAddress()==row[which.."_address"] then return o end
    end
    local function guarded(actor,box)
        if not valid(actor) or not valid(box) then return false end
        local ac,bc,owner=actor:GetClass(),box:GetClass(),box:GetOwner()
        return valid(ac) and valid(bc) and valid(owner) and ac:GetFName():ToString()=="Actor"
            and bc:GetFName():ToString()=="BoxComponent"
            and owner:GetAddress()==actor:GetAddress()
            and not actor:GetActorEnableCollision() and box:GetCollisionEnabled()==0
            and not box:IsSimulatingPhysics(FName("None"))
    end
    local out={}
    function out.component(pawn,a,frame,key)
        local created
        local ok,c=pcall(function()
            if not frame or type(a)~="table" or #a~=13 then error("historical cutting frame unavailable") end
            -- RVP snapshots transforms/extents synchronously (native
            -- 144A6E840) and deep-copies them into task storage (1449F4230).
            -- Its UObject remains alive in this bounded per-peer/hand pool.
            if not valid(pawn) then error("historical Box has no current native pawn") end
            local world=native_world(pawn)
            if not world then error("historical Box current native world unavailable") end
            if pool_world~=world then cache,pool_world={},world end
            local row=cache[key]
            if not row then
                local n=0;for _ in pairs(cache)do n=n+1 end
                if n>=128 then
                    -- Synchronous native snapshots permit reassignment after
                    -- the previous call returned, without freeing its UObject.
                    local old;old,row=next(cache);cache[old]=nil;cache[key]=row
                end
            end
            local actor,box=fresh(row,"actor"),fresh(row,"box")
            if row and not guarded(actor,box) then error("retained private Box lost native identity/collision guard") end
            if not guarded(actor,box) then
                local cls,bc=StaticFindObject("/Script/Engine.Actor"),StaticFindObject("/Script/Engine.BoxComponent")
                if not valid(cls) or not valid(bc) then error("native proxy classes unavailable") end
                local identity={Translation={X=0,Y=0,Z=0},Rotation={X=0,Y=0,Z=0,W=1},Scale3D={X=1,Y=1,Z=1}}
                local gs=e.UEHelpers.GetGameplayStatics()
                actor=gs:BeginDeferredActorSpawnFromClass(pawn,cls,identity,1,nil,2)
                if type_name(actor)~="Actor" then error("private Actor spawn failed") end
                created=actor
                -- Count ownership immediately, including failed partial construction.
                -- A pending destroy request does not prove native UObject retirement.
                cache[key]={actor_address=actor:GetAddress(),construction_failed=true}
                local owned_path=actor:GetFullName():match("^%S+ (.+)$")
                if not owned_path then error("private Actor stable identity unavailable") end
                cache[key]={actor=owned_path,actor_address=actor:GetAddress(),construction_failed=true}
                actor:SetActorEnableCollision(false);actor:SetActorHiddenInGame(true)
                gs:FinishSpawningActor(actor,identity,2)
                box=actor:AddComponentByClass(bc,true,identity,true)
                if type_name(box)~="BoxComponent" then error("private Box construction failed") end
                box:SetCollisionEnabled(0);box:SetSimulatePhysics(false);box:SetGenerateOverlapEvents(false)
                actor:FinishAddComponent(box,true,identity)
                if not guarded(actor,box) or native_world(actor)~=world then error("private Box collision/world guard failed") end
                local ap=actor:GetFullName():match("^%S+ (.+)$")
                local bp=box:GetFullName():match("^%S+ (.+)$")
                if not ap or not bp then error("private Box stable identity unavailable") end
                cache[key]={actor=ap,box=bp,actor_address=actor:GetAddress(),box_address=box:GetAddress()}
            end
            local delta=rot(frame.q,{a[1],a[2],a[3]})
            local p={frame.p[1]+delta[1],frame.p[2]+delta[2],frame.p[3]+delta[3]}
            local q=mul(frame.q,{a[4],a[5],a[6],a[7]})
            local t={Translation={X=p[1],Y=p[2],Z=p[3]},Rotation={X=q[1],Y=q[2],Z=q[3],W=q[4]},Scale3D={X=a[8],Y=a[9],Z=a[10]}}
            box:SetBoxExtent({X=a[11],Y=a[12],Z=a[13]},false)
            box:K2_SetWorldTransform(t,false,{},true)
            if not guarded(actor,box) then error("private Box lost collision guard") end
            local measured=box:GetSocketTransform(FName("None"),0)
            local ext=xyz(box:GetUnscaledBoxExtent())
            local mp,ms,mq=xyz(measured.Translation),xyz(measured.Scale3D),quat(measured.Rotation)
            for _,v in ipairs({mp[1],mp[2],mp[3],ms[1],ms[2],ms[3],ext[1],ext[2],ext[3],mq[1],mq[2],mq[3],mq[4]})do
                if not finite(v) then error("private Box nonfinite geometry readback") end
            end
            for i=1,3 do if math.abs(mp[i]-p[i])>0.01 or math.abs(ms[i]-a[i+7])>0.001 or math.abs(ext[i]-a[i+10])>0.001 then error("private Box geometry readback failed") end end
            local dot=0;for i=1,4 do dot=dot+mq[i]*q[i]end
            if math.abs(dot)<0.99999 then error("private Box rotation readback failed") end
            return box
        end)
        if not ok then
            -- No DCD/paint call has received a failed new factory object.
            -- Retire only the fresh exact private Actor, never a cached source.
            pcall(function()
                if type_name(created)=="Actor" then
                    created:K2_DestroyActor()
                end
            end) -- unsafe: exact engine Actor created synchronously above, never Willie or a queued paint source
            e.log("historical cutting Box unavailable: %s",tostring(c));return nil
        end
        return c
    end
    -- Actors belong to the old level. Retain no wrappers across world drops.
    -- A session drop does not destroy the native world. Keep its bounded pool;
    -- the next call replaces numeric identities only after native world changes.
    function out.clear(why)
        if why and why~="world changed" and why~="no valid world" then out.retire_world() end
    end
    -- Only a proven native travel invalidates same-address world generations.
    -- Session loss and transient controller misses keep the bounded pool.
    function out.retire_world()cache,pool_world={},nil end
    return out
end
return M
