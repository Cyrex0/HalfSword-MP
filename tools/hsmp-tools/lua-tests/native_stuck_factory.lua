local PATH="mods/dev/HSMPParity/Scripts/native_stuck_factory.lua"
local CLASS="/Game/Blueprints/Utility/Constraint_Weapon_Stuck_BP.Constraint_Weapon_Stuck_BP_C"
local TAG="HSMP_Penetration_Quarantine"
local function fixture()
    local M=dofile(T.path(PATH))
    local f={objects={},actors={},now=0,begins=0,finishes=0,releases=0,destroys=0,writes=0,players={},proxies={}}
    local function fname(s)return {ToString=function()return s end}end
    local sequence=0
    local function obj(class,name)
        sequence=sequence+1
        local a={class=class,name=name,id=sequence,tags={},Player=false,sim=true}
        function a:IsValid()return not self.invalid end
        function a:GetAddress()return self.id end
        function a:GetFName()return fname(self.name)end
        function a:GetFullName()return self.class.." /Test."..self.name end
        function a:GetClass()return {IsValid=function()return true end,GetFName=function()return fname(self.class)end}end
        function a:GetWorld()return f.world end
        function a:GetOwner()return self.owner end
        function a:ActorHasTag(t)return self.tags[t]==true end
        function a:IsSimulatingPhysics(b)self.lastbone=b:ToString();return self.sim end
        function a:GetBoneIndex(b)return b:ToString()=="head" and 1 or b:ToString()=="spine_02" and 2 or b:ToString()=="pelvis" and 0 or -1 end
        f.objects[a:GetFullName()]=a
        return a
    end
    f.world=obj("World","quarantine")
    f.owner=obj("Willie_BP_C","attacker");f.victim=obj("Willie_BP_C","victim");f.weapon=obj("ModularWeaponBP_C","weapon")
    for _,a in ipairs({f.owner,f.victim,f.weapon})do a.tags[TAG]=true; a.registered=true end
    f.weapon.owner=f.owner;f.weapon["Parent Actor"]=f.owner
    f.body=obj("StaticMeshComponent","BaseMesh");f.body.owner=f.weapon;f.weapon.BaseMesh=f.body
    f.mesh=obj("SkeletalMeshComponent","Mesh");f.mesh.owner=f.victim
    f.tip=obj("SceneComponent","Tip");f.tip.owner=f.weapon;f.weapon.Tips={f.tip}
    f.base=obj("SceneComponent","Base");f.base.owner=f.weapon;f.weapon.Base=f.base
    f.module=obj("StaticMeshComponent","Head");f.module.owner=f.weapon;f.weapon["Collision Components Array"]={f.module}
    f.box=obj("BoxComponent","CutBox");f.box.owner=f.weapon;f.box.BoxExtent={X=2,Y=1,Z=20};f.weapon["Hit Box Collision"]=f.box
    f.weapon["Stuck Constraints Array"]={}
    local function id(a)return {name=a.name,full_name=a:GetFullName(),class=a.class,address=a.id}end
    local token={run="offline-fixture",case="penetration",generation=7}
    local gs={IsValid=function()return true end}
    function gs:BeginDeferredActorSpawnFromClass(w,c,t,collision,owner,scale)
        f.begins=f.begins+1
        T.check(w==f.world and c==f.cls and collision==1 and owner==nil and scale==0,"exact native deferred signature")
        local a=obj("Constraint_Weapon_Stuck_BP_C","Constraint")
        a.props={}
        setmetatable(a,{__index=function(x,k)return x.props[k]end,__newindex=function(x,k,v)x.props[k]=v;f.writes=f.writes+1 end})
        function a:K2_AttachToComponent(c,b,l,r,s,weld)
            T.check(c==f.mesh and b:ToString()==f.req.hit_bone and l==1 and r==1 and s==1 and weld,"native KeepWorld original hit bone attachment")
            return not f.attach_fail
        end
        function a:K2_DestroyActor()
            f.destroys=f.destroys+1
            T.check(f.releases>0 or f.finishes==0,"native release precedes finished actor destruction")
            if not f.destroy_noop then f.actors={}; f.objects[self:GetFullName()]=nil end
        end
        f.created=a; f.actors={a}; f.writes=0
        if f.begin_fail then error("native Begin failed after allocation")end
        return a
    end
    function gs:FinishSpawningActor(a,t,scale)
        f.finishes=f.finishes+1
        T.check(f.writes==25,"all25 native creation properties assigned before Finish")
        T.check(a["My Weapon"]==f.weapon and a["Hit Actor"]==f.victim and a["Weapon Hit Module"]==f.module,"exact source and target native references")
        T.check(a["Component 1 (Weapon)"]==f.body and a["Bone Name 1"]:ToString()=="Grip","native static BaseMesh/Grip semantics preserved")
        T.check(a["Bone Name 2"]:ToString()==(f.req.hit_bone=="pelvis" and "spine_02" or f.req.hit_bone),"native pelvis remap preserved")
        if f.finish_fail then error("native finish failure")end
    end
    f.cls={IsValid=function()return true end,GetFName=function()return fname("Constraint_Weapon_Stuck_BP_C")end}
    local release={IsValid=function()return not f.no_release end,GetFullName=function()return "Function "..CLASS..":Release Constraint"end}
    f.env={
        now=function()return f.now end,log=function()end,fname=fname,
        context=function()return {offline=not f.online,world=f.world,token=token,gs=gs}end,
        find=function(path)return f.objects[path]end,
        lookup=function(c)assert(c=="Constraint_Weapon_Stuck_BP_C");if f.lookup_fail then error("enumeration failed")end;return f.actors end,
        resolve=function(path)if path==CLASS then return f.cls end;assert(path==CLASS..":Release Constraint");return release end,
        is_a=function(a,c)return a.class==c or c=="PrimitiveComponent" and (a.class=="StaticMeshComponent" or a.class=="BoxComponent")end,
        is_player=function(a)return f.players[a]==true end,is_proxy=function(a)return f.proxies[a]==true end,
        quarantined=function(a,t)return a.registered==true and t.run==token.run and t.case==token.case and t.generation==token.generation end,
        members=function(a,k)return a[k]end,
        add_unique=function(a,k,v)a[k][#a[k]+1]=v end,
        call=function(a,name,...)
            T.check(name=="Release Constraint" and select("#",...)==0,"proven zeroargument native release")
            if f.release_fail then error("release failed")end
            f.releases=f.releases+1
        end,
    }
    f.req={token={run=token.run,case=token.case,generation=token.generation},world={name=f.world:GetFullName(),address=f.world.id},
        owner=id(f.owner),weapon=id(f.weapon),victim=id(f.victim),hit_bone="head",
        components={weapon_body=id(f.body),target_body=id(f.mesh),tip=id(f.tip),base=id(f.base),module=id(f.module),hitbox=id(f.box)},
        values={ ["Thrust?"]=false,BoxCollision=true,["False Egde"]=false,["Spikes (temp)"]=false,
            ["Edge Sharpness"]=60,["Tip Sharpness"]=40,["Draw Cut"]=0.25,["Edge Alignment"]=0.9,
            ["Dismember Cut Level"]=2,["Material Density"]=1000,
            ["Start Weapon Impact"]={X=0,Y=300,Z=0},["Initial Impact Point"]={X=10,Y=20,Z=30},
            ["Edge Direction"]={X=0,Y=1,Z=0},["Initiail Norm"]={X=1,Y=0,Z=0},["Box Extent"]={X=2,Y=1,Z=20}},
        transform={Translation={X=10,Y=20,Z=30},Rotation={X=0,Y=0,Z=0,W=1},Scale3D={X=1,Y=1,Z=1}}}
    f.api=M.new(f.env)
    function f.initialize() f.created.Constraint={IsValid=function()return true end}end
    return f
end
local f=fixture()
local staged=f.api.stage(f.req)
T.check(staged and f.begins==0,"StageInputs performs no native mutation")
f.req.values["Edge Sharpness"]=1
T.check(staged.values["Edge Sharpness"]==60,"staged original inputs are independent plain copies")
f.req=staged
T.check(f.api.execute(staged),"exact quarantined native fixture constructs")
T.check(not f.api.execute(staged) and f.begins==1,"one unresolved constraint blocks another factory execution")
T.check(not f.api.cleanup() and f.destroys==0,"deferred BeginPlay must initialize before native Release")
f.initialize();f.destroy_noop=true
T.check(not f.api.cleanup() and f.releases==1 and f.destroys==1,"destroy noop retains identity obligation")
T.check(f.api.status()~=nil,"destroy return never proves disappearance")
f.actors={}
T.check(f.api.cleanup() and f.api.status()==nil,"fresh complete absence discharges obligation")

local rejects={
    {"missing victim quarantine tag",function(x)x.victim.tags[TAG]=nil end},
    {"missing weapon quarantine tag",function(x)x.weapon.tags[TAG]=nil end},
    {"missing owner quarantine tag",function(x)x.owner.tags[TAG]=nil end},
    {"tag alone without registry",function(x)x.owner.registered=false end},
    {"native player even if tagged",function(x)x.victim.Player=true end},
    {"current player even with stale native flag",function(x)x.players[x.owner]=true end},
    {"network proxy even if tagged",function(x)x.proxies[x.victim]=true end},
    {"online context",function(x)x.online=true end},
    {"old original test generation",function(x)x.req.token.generation=6 end},
    {"old native world",function(x)x.req.world.address=999 end},
    {"reused component identity",function(x)x.req.components.module.address=999 end},
    {"wrong actual weapon owner",function(x)x.weapon.owner=x.victim end},
    {"missing target bone",function(x)x.req.hit_bone="absent" end},
    {"kinematic source body",function(x)x.body.sim=false end},
    {"kinematic victim body",function(x)x.mesh.sim=false end},
    {"unlisted source tip",function(x)x.weapon.Tips={}end},
    {"unlisted native source module",function(x)x.weapon["Collision Components Array"]={}end},
    {"arbitrary native property injection",function(x)x.req.values.Health=100 end},
    {"nonfinite density",function(x)x.req.values["Material Density"]=0/0 end},
    {"missing native release function",function(x)x.no_release=true end},
    {"changed original box extent",function(x)x.box.BoxExtent.X=7 end},
}
for _,case in ipairs(rejects)do
    local x=fixture();case[2](x)
    T.check(not x.api.execute(x.req) and x.begins==0,"pre-Begin refusal: "..case[1])
end

f=fixture();f.req.hit_bone="pelvis"
T.check(f.api.execute(f.req),"pelvis uses original attach bone and remapped constrained spine")
f.initialize();f.release_fail=true
T.check(not f.api.cleanup() and f.destroys==0 and f.api.status()~=nil,"failed native release never destroys through unsafe dependencies")
f.release_fail=false;f.lookup_fail=true
T.check(not f.api.cleanup() and f.destroys==0,"enumeration error never means native absence")
f.lookup_fail=false
local old=f.created
local fresh={};for k,v in pairs(old)do fresh[k]=v end
setmetatable(fresh,getmetatable(old));f.actors={fresh};f.objects[fresh:GetFullName()]=fresh
old.IsValid=function()error("old native wrapper dereferenced")end
f.created=fresh
T.check(not f.api.cleanup() and f.releases==1,"cleanup uses freshly resolved wrapper without touching old UObject")
T.check(f.api.cleanup(),"fresh replacement wrapper destruction confirmed next callback")

f=fixture();f.finish_fail=true
T.check(not f.api.execute(f.req) and f.api.status()~=nil,"partial Finish failure retains original cleanup obligation")
T.check(not f.api.cleanup() and f.destroys==0,"uncertain initialization cannot invoke native release blindly")
f.initialize();f.api.cleanup();T.check(f.api.cleanup(),"failed Finish can be released after native initialization evidence")

f=fixture();f.api.execute(f.req);f.initialize();f.now=2
T.check(not f.api.tick() and f.releases==1,"one second native lifetime bound requests exact cleanup")
T.check(f.api.tick(),"bounded lease completes only after disappearance")
f=fixture();f.api.execute(f.req);f.initialize()
f.objects[f.req.components.target_body.full_name]=nil
T.check(not f.api.cleanup() and f.destroys==0,"vanished target preserves obligation without invoking native code on dangling references")
T.check(f.api.status().request.token.generation==7,"cleanup status retains plain original token")
f=fixture();f.begin_fail=true
T.check(not f.api.execute(f.req) and f.begins==1 and f.api.status()~=nil,"ambiguous Begin allocation reserves cleanup obligation")
T.check(not f.api.execute(f.req) and f.begins==1,"unknown native Begin outcome never permits duplicate allocation")
T.check(not f.api.cleanup() and f.destroys==0,"unknown native actor identity cannot be guessed for destruction")
