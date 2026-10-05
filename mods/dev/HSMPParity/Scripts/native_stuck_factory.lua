-- Dev-only, NOT wired into Parity or production combat. The host must provide
-- fresh native lookup and an explicit offline quarantine registry. No UObject
-- is retained by this module. Primary recipe: native-stuck-reconstruction-20261005.
local M = {}
local CLASS = "/Game/Blueprints/Utility/Constraint_Weapon_Stuck_BP.Constraint_Weapon_Stuck_BP_C"
local CLASS_NAME = "Constraint_Weapon_Stuck_BP_C"
local RELEASE = CLASS .. ":Release Constraint"
local TAG = "HSMP_Penetration_Quarantine"
local VECTOR_FIELDS={ ["Start Weapon Impact"]=true,["Initial Impact Point"]=true,["Edge Direction"]=true,["Initiail Norm"]=true,["Box Extent"]=true }
local VALUE_FIELDS={ ["Thrust?"]=true,BoxCollision=true,["False Egde"]=true,["Spikes (temp)"]=true,
    ["Edge Sharpness"]=true,["Tip Sharpness"]=true,["Draw Cut"]=true,["Edge Alignment"]=true,
    ["Dismember Cut Level"]=true,["Material Density"]=true }
local REF_FIELDS={ ["Component 1 (Weapon)"]=true,["Component 2 (Body)"]=true,Tip=true,Base=true,
    ["My Weapon"]=true,["Hit Actor"]=true,["Weapon Hit Module"]=true,["Hit Box Collision"]=true }
local lease -- one obligation across factory instances, even after failed cleanup
local function valid(x) return x and x:IsValid() == true end
local function finite(x) return type(x)=="number" and x==x and math.abs(x)<math.huge end
local function bounded(x,a,b) return finite(x) and x>=a and x<=b end
local function copy(x,depth)
    depth=(depth or 0)+1; assert(depth<12,"plain input nesting")
    if type(x)~="table" then
        assert(type(x)=="string" or type(x)=="boolean" or finite(x) or x==nil,"plain data only")
        return x
    end
    assert(getmetatable(x)==nil,"plain input metatable")
    local y={}; for k,v in pairs(x)do assert(type(k)=="string" or type(k)=="number","plain key");y[k]=copy(v,depth)end
    return y
end
local function eq(a,b)
    if type(a)~="table" or type(b)~="table" then return a==b end
    for k,v in pairs(a)do if not eq(v,b[k])then return false end end
    for k in pairs(b)do if a[k]==nil then return false end end
    return true
end
local function vector(v,limit)
    return type(v)=="table" and bounded(v.X,-limit,limit) and bounded(v.Y,-limit,limit) and bounded(v.Z,-limit,limit)
end
local function world_id(w)
    assert(valid(w),"world invalid")
    return {name=w:GetFullName(),address=w:GetAddress()}
end
local function identity(a)
    assert(valid(a),"invalid native object")
    local c=a:GetClass();assert(valid(c),"invalid native class")
    return {name=a:GetFName():ToString(),full_name=a:GetFullName(),address=a:GetAddress(),class=c:GetFName():ToString()}
end
local function same(a,b) return valid(a) and valid(b) and a:GetAddress()==b:GetAddress() end
local function xyz(v) return {X=v.X,Y=v.Y,Z=v.Z}end
local function token_ok(t)
    return type(t)=="table" and type(t.run)=="string" and #t.run>0 and #t.run<=128
        and type(t.case)=="string" and #t.case>0 and #t.case<=128
        and bounded(t.generation,1,2147483647) and t.generation%1==0
end
function M.new(e)
    for _,k in ipairs({"context","find","lookup","resolve","is_a","is_player","is_proxy","quarantined","members","now","log","fname","call","add_unique"})do
        assert(type(e[k])=="function","explicit adapter required: "..k)
    end
    local function context(r)
        local c=e.context()
        assert(c and c.offline==true,"offline quarantine only")
        assert(token_ok(c.token) and eq(c.token,r.token),"original test token mismatch")
        assert(eq(world_id(c.world),r.world),"original native world mismatch")
        return c
    end
    local function fresh(id,r)
        assert(type(id)=="table" and type(id.full_name)=="string" and #id.full_name>0,"native identity required")
        local a=e.find(id.full_name)
        assert(valid(a) and eq(identity(a),id),"fresh native identity mismatch")
        assert(eq(world_id(a:GetWorld()),r.world),"native object world mismatch")
        return a
    end
    local function actor(id,r)
        local a=fresh(id,r)
        assert(a:ActorHasTag(TAG)==true and e.quarantined(a,r.token)==true,"explicit quarantine ownership required")
        assert(e.is_player(a)==false and e.is_proxy(a)==false,"players and network proxies prohibited")
        return a
    end
    local function member(a,field,wanted)
        local all=e.members(a,field);assert(type(all)=="table" and #all<=128,"native member enumeration failed")
        for _,v in ipairs(all)do if same(v,wanted)then return true end end
        return false
    end
    local function refs(r)
        local c=context(r)
        local owner,weapon,victim=actor(r.owner,r),actor(r.weapon,r),actor(r.victim,r)
        assert(not same(owner,victim) and not same(weapon,victim),"distinct quarantine actors required")
        assert(e.is_a(owner,"Willie_BP_C")==true and e.is_a(victim,"Willie_BP_C")==true,"native Willie actors required")
        assert(owner.Player==false and victim.Player==false,"native Player prohibited")
        assert(e.is_a(weapon,"ModularWeaponBP_C")==true,"native modular weapon required")
        assert(same(weapon:GetOwner(),owner) and same(weapon["Parent Actor"],owner),"native weapon ownership mismatch")
        local p={}
        for _,k in ipairs({"weapon_body","target_body","tip","base","module"})do p[k]=fresh(r.components[k],r)end
        for _,k in ipairs({"weapon_body","module"})do
            assert(e.is_a(p[k],"PrimitiveComponent")==true and same(p[k]:GetOwner(),weapon),"weapon primitive mismatch")
        end
        assert(e.is_a(p.weapon_body,"StaticMeshComponent")==true and same(p.weapon_body,weapon.BaseMesh),"exact native BaseMesh required")
        assert(p.weapon_body:IsSimulatingPhysics(e.fname("Grip"))==true,"native source physical body unavailable")
        assert(e.is_a(p.target_body,"SkeletalMeshComponent")==true and same(p.target_body:GetOwner(),victim),"exact victim skeletal body required")
        assert(type(r.hit_bone)=="string" and #r.hit_bone>0 and #r.hit_bone<=64,"original hit bone required")
        local bone=r.hit_bone=="pelvis" and "spine_02" or r.hit_bone
        assert(p.target_body:GetBoneIndex(e.fname(bone))>=0 and p.target_body:GetBoneIndex(e.fname(r.hit_bone))>=0,"native target bone absent")
        assert(p.target_body:IsSimulatingPhysics(e.fname(bone))==true,"native target physical body unavailable")
        assert(e.is_a(p.tip,"SceneComponent")==true and same(p.tip:GetOwner(),weapon) and member(weapon,"Tips",p.tip),"exact selected native tip required")
        assert(e.is_a(p.base,"SceneComponent")==true and same(p.base,weapon.Base),"exact native base required")
        assert(member(weapon,"Collision Components Array",p.module),"source module absent from native array")
        if r.components.hitbox then
            p.hitbox=fresh(r.components.hitbox,r)
            assert(e.is_a(p.hitbox,"BoxComponent")==true and same(p.hitbox:GetOwner(),weapon)
                and same(p.hitbox,weapon["Hit Box Collision"]),"exact native cutting box required")
        end
        local v=r.values
        assert(type(v)=="table","native creation values required")
        for k in pairs(v)do assert(VECTOR_FIELDS[k] or VALUE_FIELDS[k],"unknown native creation property")end
        for _,k in ipairs({"Thrust?","BoxCollision","False Egde","Spikes (temp)"})do assert(type(v[k])=="boolean","typed native bool: "..k)end
        for _,k in ipairs({"Edge Sharpness","Tip Sharpness"})do assert(bounded(v[k],0,100),"native sharpness range")end
        assert(bounded(v["Draw Cut"],0,1) and bounded(v["Edge Alignment"],0,1),"native alignment range")
        assert(bounded(v["Dismember Cut Level"],0,99) and v["Dismember Cut Level"]%1==0,"native dismember level")
        assert(bounded(v["Material Density"],0.000001,1000000),"finite positive native material density")
        for _,k in ipairs({"Start Weapon Impact","Initial Impact Point","Edge Direction","Initiail Norm","Box Extent"})do
            assert(vector(v[k],1000000),"finite native vector: "..k)
        end
        for _,k in ipairs({"X","Y","Z"})do assert(v["Box Extent"][k]>=0,"negative box extent")end
        assert(not v.BoxCollision or p.hitbox,"native cutting box missing")
        if p.hitbox then assert(eq(xyz(p.hitbox.BoxExtent),v["Box Extent"]),"original native box extent mismatch")end
        local t=r.transform
        assert(type(t)=="table" and vector(t.Translation,1000000) and eq(t.Translation,v["Initial Impact Point"]),"original spawn location mismatch")
        assert(eq(t.Scale3D,{X=1,Y=1,Z=1}),"native constraint scale must be one")
        local q=t.Rotation;assert(type(q)=="table" and vector(q,1) and bounded(q.W,-1,1),"finite native spawn quaternion")
        assert(math.abs(q.X*q.X+q.Y*q.Y+q.Z*q.Z+q.W*q.W-1)<0.0001,"normalized native spawn quaternion")
        assert(valid(c.gs),"gameplay statics unavailable")
        local cls=e.resolve(CLASS);assert(valid(cls),"native constraint class unavailable")
        assert(cls:GetFName():ToString()==CLASS_NAME,"wrong native constraint class")
        local release=e.resolve(RELEASE);assert(valid(release),"verified native Release Constraint unavailable; inputs only")
        assert(release:GetFullName()=="Function "..RELEASE,"wrong native release function")
        return c,owner,weapon,victim,p,bone,cls
    end
    local function fields(r,weapon,victim,p,bone)
        local v=copy(r.values)
        v["Component 1 (Weapon)"],v["Bone Name 1"]=p.weapon_body,e.fname("Grip")
        v["Component 2 (Body)"],v["Bone Name 2"]=p.target_body,e.fname(bone)
        v.Tip,v.Base,v["My Weapon"],v["Hit Actor"]=p.tip,p.base,weapon,victim
        v["Weapon Hit Module"]=p.module
        -- nil is explicitly assigned below: Lua tables cannot retain nil slots.
        return v
    end
    local function present(r,id)
        context(r)
        local all=e.lookup(CLASS_NAME);assert(type(all)=="table","complete native enumeration unavailable")
        for _,a in pairs(all)do
            if valid(a) and eq(world_id(a:GetWorld()),r.world)then
                if not id then return a end
                if eq(identity(a),id)then return a end
                assert(a:GetAddress()~=id.address,"constraint address reused")
            end
        end
    end
    local out={}
    function out.stage(request)
        local ok,r=pcall(function()local r=copy(request);assert(token_ok(r.token),"full original test token required");refs(r);return r end)
        if not ok then return nil,tostring(r)end
        return r -- plain immutable-by-copy request, revalidated again at execution
    end
    function out.execute(request)
        local ok,why=pcall(function()
            assert(not lease,"unresolved native cleanup obligation")
            local r=copy(request)
            local c,_,weapon,victim,p,bone,cls=refs(r)
            assert(not present(r),"native constraint already exists in test world")
            e.log("STUCK_FACTORY boundary=begin token=%s/%s/%s",r.token.run,r.token.case,r.token.generation)
            -- A native call can fail after allocation. Reserve before Begin so an
            -- unknown result cannot authorize a second allocation.
            lease={request=r,stage="begin_requested",deadline=e.now()+1,steps=0}
            local a=c.gs:BeginDeferredActorSpawnFromClass(c.world,cls,r.transform,1,nil,0)
            assert(valid(a),"native deferred creation failed")
            -- Record obligation before writing any property or invoking construction.
            lease.id=identity(a);lease.stage="deferred"
            assert(lease.id.class==CLASS_NAME and eq(world_id(a:GetWorld()),r.world),"spawned native class/world mismatch")
            local f=fields(r,weapon,victim,p,bone)
            for k,v in pairs(f)do a[k]=v end
            a["Hit Box Collision"]=p.hitbox
            if p.hitbox then assert(same(a["Hit Box Collision"],p.hitbox),"native cutting box readback")
            else assert(not valid(a["Hit Box Collision"]),"unexpected native cutting box")end
            -- Exactly25 properties, all populated before Finish and any BeginPlay.
            for k,v in pairs(f)do
                local got=a[k]
                if REF_FIELDS[k] then assert(same(got,v),"native reference readback: "..k)
                elseif VECTOR_FIELDS[k] then assert(eq(xyz(got),v),"native value readback: "..k)
                elseif k=="Bone Name 1" or k=="Bone Name 2" then assert(got:ToString()==v:ToString(),"native bone readback")
                else assert(got==v,"native scalar readback: "..k)end
            end
            lease.stage="finish_requested"
            c.gs:FinishSpawningActor(a,r.transform,0)
            lease.stage="finished"
            assert(member(weapon,"Stuck Constraints Array",a)==false,"duplicate native membership")
            -- Array mutation is adapter-owned because UE4SS arrays are not Lua arrays.
            e.add_unique(weapon,"Stuck Constraints Array",a)
            assert(member(weapon,"Stuck Constraints Array",a),"native membership readback")
            assert(a:K2_AttachToComponent(p.target_body,e.fname(r.hit_bone),1,1,1,true)==true,"native attachment failed")
            weapon["Jammed Tip"]=p.tip
            e.log("STUCK_FACTORY boundary=constructed actor=%s",lease.id.full_name)
        end)
        return ok,ok and nil or tostring(why)
    end
    function out.cleanup()
        if not lease then return true end
        local ok,result=pcall(function()
            local r=lease.request
            assert(lease.id and lease.id.class==CLASS_NAME,"ambiguous native allocation; retain obligation for isolated process cleanup")
            local a=present(r,lease.id)
            if not a then lease=nil;return true end -- only complete fresh absence discharges ownership
            if not lease.released and lease.stage~="deferred" then
                local _,_,weapon,victim,p=refs(r) -- native release dereferences these; never invoke with stale dependencies
                assert(valid(a.Constraint),"native initialization pending; retain cleanup obligation")
                assert(same(a["My Weapon"],weapon) and same(a["Hit Actor"],victim),"native constraint ownership changed")
                assert(same(a["Component 1 (Weapon)"],p.weapon_body) and same(a["Component 2 (Body)"],p.target_body)
                    and same(a.Tip,p.tip) and same(a.Base,p.base) and same(a["Weapon Hit Module"],p.module),"native constraint endpoints changed")
                assert(p.target_body:GetBoneIndex(a["Bone Name 2"])>=0,"rebound native target bone unavailable")
                e.call(a,"Release Constraint") -- cooked zero-argument wrapper44998; no guessed signature
                lease.released=true
            end
            -- A never-finished deferred actor has no native constraint to release.
            a:K2_DestroyActor() -- unsafe: ok exact factory-created non-Willie class, fresh identity+world, release above
            lease.destroy_requested=true
            return false -- never dereference a wrapper after destruction or treat return as absence
        end)
        return ok and result==true,ok and nil or tostring(result)
    end
    function out.tick()
        if not lease then return true end
        lease.steps=lease.steps+1
        if lease.destroy_requested or e.now()>=lease.deadline or lease.steps>=120 then return out.cleanup()end
        local ok,err=pcall(function()refs(lease.request);assert(present(lease.request,lease.id),"native actor disappeared")end)
        if not ok then return false,tostring(err)end
        return false
    end
    function out.status() return lease and copy(lease) or nil end
    return out
end
return M
