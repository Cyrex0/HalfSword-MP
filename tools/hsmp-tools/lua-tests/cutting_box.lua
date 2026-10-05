local M=dofile(T.path("mods/HSMPCombat/Scripts/cutting_box.lua"))
FName=function(s)return {ToString=function()return s end}end
local function cls(name)return {IsValid=function()return true end,GetFName=function()return FName(name)end}end
local function vector(a)return {X=a[1],Y=a[2],Z=a[3]}end
local bf={to_local=function(f,p)return {(p[1]-f.p[1])/f.s,(p[2]-f.p[2])/f.s,(p[3]-f.p[3])/f.s}end,
    to_world=function(f,p)return {f.p[1]+p[1]*f.s,f.p[2]+p[2]*f.s,f.p[3]+p[3]*f.s}end}
local t={Translation={X=14,Y=24,Z=34},Rotation={X=0,Y=0,Z=0,W=1},Scale3D={X=2,Y=1.25,Z=0.75}}
local source={IsValid=function()return true end,GetClass=function()return cls("BoxComponent")end,
    GetSocketTransform=function()return t end,GetUnscaledBoxExtent=function()return {X=30,Y=3,Z=12}end}
local frame={p={10,20,30},q={0,0,0,1},s=2}
local a=M.capture(source,frame,bf)
T.check(a and #a==13 and a[1]==4 and a[7]==1 and a[8]==2 and a[11]==30,
    "snapshot retains physical-centimetre bone-rotation center and native extent separately from world scale")
t.Translation.X=999
T.check(a[1]==4,"queued original cutting frame is immutable numeric data")
T.check(M.capture(source,nil,bf)==nil,"missing original victim frame cannot invent cutting geometry")
source.GetClass=function()return cls("SphereComponent")end
T.check(M.capture(source,frame,bf)==nil,"only actual native Box geometry is captured")
source.GetClass=function()return cls("BoxComponent")end
do
    local original=t.Translation
    t.Translation={X=210,Y=20,Z=30}
    local socket={p={10,20,30},q={0,0,0,1},s=1.125}
    local physical=M.capture(source,socket,bf)
    T.check(physical and physical[1]==200,"native socket scale1.125 does not divide physical cutting-center offset")
    t.Translation=original
end
local objects,spawns,adds,damages,original_changes={},0,0,0,0
local bc,ac=cls("BoxComponent"),cls("Actor")
StaticFindObject=function(path)if path=="/Script/Engine.Actor"then return ac elseif path=="/Script/Engine.BoxComponent"then return bc end;return objects[path]end
local world={IsValid=function()return true end,GetAddress=function()return 1 end}
local level={IsValid=function()return true end,GetOuter=function()return world end}
local pawn={IsValid=function()return true end,GetWorld=function()return world end}
local gs={}
function gs:BeginDeferredActorSpawnFromClass(context,c,xf,handling,owner)
    T.check(context==pawn and c==ac and owner==nil,"proxy actor has exact native class and survives pawn replacement independently")
    spawns=spawns+1
    local aid=100+spawns
    local path="World.Proxy"..spawns
    local actor={enabled=true,IsValid=function()return true end,GetClass=function()return ac end,
        GetAddress=function()return aid end,GetFullName=function()return "Actor "..path end,GetWorld=function()return world end}
    function actor:SetActorEnableCollision(v)self.enabled=v end
    function actor:SetActorHiddenInGame(v)self.hidden=v end
    function actor:GetActorEnableCollision()return self.enabled end
    function actor:AddComponentByClass(c,manual,xf,deferred)
        T.check(c==bc and deferred and manual,"private native Box is deferred until collision guards are applied")
        adds=adds+1
        local bid=1000+adds
        local bp=path..".Box"
        local box={enabled=3,sim=true,IsValid=function()return true end,GetClass=function()return bc end,
            GetAddress=function()return bid end,GetOwner=function()return actor end,GetFullName=function()return "BoxComponent "..bp end}
        function box:SetCollisionEnabled(v)self.enabled=v end
        function box:SetSimulatePhysics(v)self.sim=v end
        function box:SetGenerateOverlapEvents(v)self.overlap=v end
        function box:GetCollisionEnabled()return self.enabled end
        function box:IsSimulatingPhysics()return self.sim end
        function box:SetBoxExtent(v,update)self.ext={X=v.X,Y=v.Y,Z=v.Z};T.check(update==false,"private extent write creates no overlap event")end
        function box:K2_SetWorldTransform(v,sweep)self.t=v;T.check(sweep==false,"historical proxy does not sweep into gameplay bodies")end
        function box:GetSocketTransform()return self.t end
        function box:GetUnscaledBoxExtent()return self.ext end
        objects[bp]=box
        return box
    end
    function actor:FinishAddComponent(box)
        T.check(not self.enabled and box.enabled==0 and box.sim==false and box.overlap==false,
            "proxy is collision-disabled and non-physical before native component registration")
    end
    objects[path]=actor
    return actor
end
function gs:FinishSpawningActor(actor)T.check(not actor.enabled and actor.hidden,"empty private actor remains hidden and collision-disabled through construction")end
local logs={}
local replay=M.new({UEHelpers={GetGameplayStatics=function()return gs end},bf=bf,log=function(...)logs[#logs+1]=string.format(...)end})
local box=replay.component(pawn,a,frame,"2:R")
T.check(box~=nil and box.ext.X==30 and box.t.Scale3D.X==2 and box.t.Translation.X==14,
    "private replay reconstructs historical position and preserves native unscaled clamp before world scale")
local prior=frame.s
frame.s=0.9932
T.check(replay.component(pawn,a,frame,"2:R")==box and box.t.Translation.X==14,
    "median skeleton scale cannot rescale the original physical-centimetre cutting center")
frame.s=prior
local remirror={p={100,200,300},q={0,0,math.sqrt(0.5),math.sqrt(0.5)},s=0.9932}
T.check(replay.component(pawn,a,remirror,"2:R")==box and math.abs(box.t.Translation.X-96)<0.001
    and math.abs(box.t.Translation.Y-204)<0.001 and box.t.Translation.Z==304,
    "remirrored victim bone position and rotation reconstruct physical center without reference scale")
local b={3,4,5,0,0,0,1,1.5,1,1,20,2,8}
local reused=replay.component(pawn,b,frame,"2:R")
T.check(reused==box and spawns==1 and adds==1,"repeated approved cuts reuse one bounded retained peer/hand proxy")
T.check(box.ext.X==20 and box.t.Translation.X==13,"reused proxy snapshots next physical historical geometry after previous synchronous call")
local offhand=replay.component(pawn,a,frame,"2:L")
T.check(offhand~=box and spawns==2,"different original hand receives its own retained proxy")
T.check(t.Translation.X==999 and damages==0 and original_changes==0,"factory never invokes damage or mutates original native weapon")
T.check(replay.component(pawn,{},frame,"2:R")==nil,"missing historical frame fails before native mutation")
local catalog=dofile(T.path("mods/HSMPLoadout/Scripts/hsmp_catalog.lua"))
local hashes,count={},0
for _,item in pairs(catalog.items)do if item.kind=="weapon"then
    local name=item.path:match("([^/]+)$").."_C"
    local h=M.class_hash(name)
    T.check(not hashes[h] or hashes[h]==name,"full native class fingerprint has no catalogue collision: "..name)
    hashes[h]=name;count=count+1
end end
T.check(count==128,"every configured weapon class participates in source identity collision audit")

replay.clear()
T.check(replay.component(pawn,a,frame,"2:R")==box and spawns==2,"session reconnect retains same native-world pool identity")
local extras=dofile(T.path("mods/dev/HSMPParity/Scripts/native_extra_melee.lua"))
for _,item in pairs(extras.items)do
    local name=item.path:match("([^/]+)$").."_C"
    local hash=M.class_hash(name)
    T.check(not hashes[hash] or hashes[hash]==name,"extra eligible native melee class has no identity collision: "..name)
    hashes[hash]=name
end

local old_add=gs.BeginDeferredActorSpawnFromClass
local before_fail=spawns
function gs:BeginDeferredActorSpawnFromClass(...)
    local actor=old_add(self,...)
    actor.AddComponentByClass=function()error("native component factory transient failure")end
    actor.K2_DestroyActor=function()end -- asynchronous destroy request is not retirement proof
    return actor
end
T.check(replay.component(pawn,a,frame,"failed:R")==nil,"partial component construction fails closed")
T.check(spawns==before_fail+1,"partial factory owns one fresh actor")
T.check(replay.component(pawn,a,frame,"failed:R")==nil and spawns==before_fail+1,"failed construction retains counted identity and never reallocates on repeated claims")

gs.BeginDeferredActorSpawnFromClass=old_add
local malformed=replay.component(pawn,a,frame,"nan:R")
local readback=malformed.GetSocketTransform
malformed.GetSocketTransform=function(self)local t=readback(self);t.Translation.X=0/0;return t end
T.check(replay.component(pawn,a,frame,"nan:R")==nil,"nonfinite native geometry readback fails closed")
malformed.GetSocketTransform=readback
local before_travel=spawns
replay.retire_world()
T.check(replay.component(pawn,a,frame,"2:R")~=box and spawns==before_travel+1,"proven native travel invalidates private identity despite UWorld address reuse")
