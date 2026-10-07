local M=dofile(T.path("mods/HSMPCombat/Scripts/native_protection_audit.lua"))
local logs,enabled,reads={},false,0
local function obj(name,id,fields)
    local o=fields or {}
    o.IsValid=function()return true end
    o.GetAddress=function()return id end
    o.GetFullName=function()return name end
    return o
end
local function param(x)return{get=function()return x end}end
local function unwrap(p)if type(p)=="table" and p.get then return p:get() end;return p end
local function arr(items)
    return{GetArrayNum=function()return #items end,ForEach=function(_,f)for i,v in ipairs(items)do f(i,param(v))end end}
end
local core=obj("Class /Game/Armor.Armor_C",1)
local pass={ArmorCore_3_F6B7C69C4BD7D9720DB91EB635EE2B43=core,
    ID_54_C6BBB1A64A3828B5AB1D8E804EC7C8F7=7,Slot_30_7561CB484566A4512003EA96ED44F88D=5,
    CoreRemoved_12_5CFF8F6D4A05C15812594CAF6771C66B=false,
    Module1_5_46B7198E4341C93CBF6AE989EF9898E4=2}
local bs=obj("BodySetup",3,{CollisionTraceFlag=1,AggGeom={BoxElems=arr({1,2}),ConvexElems=arr({})}})
local mesh=obj("StaticMesh Armor",2,{BodySetup=bs})
local comp=obj("StaticMeshComponent latest",4,{StaticMesh=mesh,
    ComponentTags=arr({{ToString=function()return "defB10" end},{ToString=function()return "dens7800" end}}),
    GetCollisionEnabled=function()return 1 end,GetCollisionObjectType=function()return 11 end,
    GetCollisionProfileName=function()return{ToString=function()return "ArmorProxy" end}end,
    K2_GetComponentToWorld=function()return{Translation={X=1000,Y=0,Z=-10000000},Scale3D={X=1,Y=1,Z=1},Rotation={X=0,Y=0,Z=0,W=1}}end})
local pawn=obj("Willie local",5,{["Armor Collision Meshes"]=arr({obj("old",6,{StaticMesh=mesh}),comp}),
    ["Armor Passports related to Armor Collision Meshes"]={ForEach=function(_,f)f(param(comp),param(pass))end}})
local def={Blunt_2_0C001DBE4C8B7C85C68641B217A18F10=10,
    Cut_9_722ACAC246B5F7600CAC84AFF9188F9D=200,
    Stab_6_C3AA0F1D4B183BE20371069F7FDCA346=75,
    Density_8_B3D4247C4C7FD0F8EE8F0C8D74E1A90D=7800}
local a=M.new{enabled=function()return enabled end,unwrap=function(p)reads=reads+1;return unwrap(p)end,
    context=function()return{peer=8,match_id=9,round=2,life=3}end,
    log=function(fmt,...)logs[#logs+1]=string.format(fmt,...)end}
a.capture(param(pawn),param(mesh),param(def),param(pass))
T.check(reads==0 and #logs==0,"disabled probe touches no native params")
enabled=true;a.capture(param(pawn),param(mesh),param(def),param(pass))
local s=logs[1] or ""
T.check(s:find("evidence=armor_construction_only",1,true) and s:find("peer=8 match_id=9 round=2 life=3",1,true),"construction evidence has exact life context")
T.check(s:find("defB=10 defC=200 defS=75 dens=7800",1,true),"exact per-mesh native protection doubles")
T.check(s:find("component=StaticMeshComponent_latest component_index=2",1,true) and s:find("mapped_passport_id=7 mapped_slot=5",1,true),"newest native mesh matched to actual passport")
T.check(s:find("tags=defB10,dens7800",1,true) and s:find("object_type=11",1,true),"native collision tags/channel copied")
T.check(s:find("BoxElems=2",1,true) and s:find("ConvexElems=0",1,true) and s:find("SphereElems=unavailable",1,true),"shape counts distinguish absent reflection from zero")
T.check(s:find("posZ=-10000000",1,true) and s:find("quatW=1",1,true),"native proxy transform copied without asset loads")
local calls,hook=0,nil
T.check(not a.install(function()calls=calls+1;error("class not loaded")end),"registration fails safely until class loads")
T.check(a.install(function(path,f)calls=calls+1;hook=f;T.check(path==M.HOOK,"exact reflected hook path")end),"native hook retries after unavailable class")
a.install(function()calls=calls+1 end)
T.check(calls==2,"successful hook installed once")
hook(param(pawn),param(mesh),param(def),param(pass))
T.check(#logs==2,"installed POST callback records construction")
def.Cut_9_722ACAC246B5F7600CAC84AFF9188F9D=0/0
a.capture(pawn,nil,def,nil)
T.check(logs[3]:find("defC=unavailable",1,true) and logs[3]:find("core=unavailable",1,true),"unavailable/nonfinite data never invented as zero")
local broken=setmetatable({IsValid=function()return true end},{__index=function()error("unavailable reflection")end})
T.check(pcall(a.capture,broken,nil,nil,nil),"reflection failures cannot interrupt native gameplay")
local before_reads=reads
enabled=false
T.check(a.trace(nil,nil,nil,nil,nil,nil,nil,nil)==nil and reads==before_reads,"disabled native trace touches no output parameters")
enabled=true
local mat=obj("PhysicalMaterial Flesh",12,{SurfaceType=1})
local hit={Component=param(comp),PhysMaterial=param(mat),BoneName={ToString=function()return "spine_03"end},
    Distance=1.25,bBlockingHit=false,bStartPenetrating=true}
local traced=a.trace(param({X=1,Y=2,Z=3}),param({X=4,Y=5,Z=6}),.1,arr({11}),true,arr({hit}),false,true)
T.check(traced and traced:find("objects:11,radius:0.10000000000000001,complex:true,ignore_self:false,out:true,hits:1",1,true),"trace records actual native query and output signature",traced)
T.check(traced and traced:find("component:StaticMeshComponent_latest|mesh:StaticMesh_Armor|surface:1|tags:defB10,dens7800",1,true),"native weak hit objects resolve to actual mesh surface and protection tags",traced)
T.check(traced and traced:find("bone:spine_03|distance:1.25|blocking:false|initial_overlap:true",1,true),"native trace keeps ordered hit geometry flags",traced)
local dead_reads=0
local invalid={IsValid=function()return false end,GetFullName=function()dead_reads=dead_reads+1;error("invalid weak target")end}
local missing=a.trace(nil,nil,nil,arr({11}),true,arr({{Component=param(invalid),PhysMaterial=param(invalid)}}),false,false)
T.check(missing and missing:find("component:unavailable|mesh:unavailable|surface:unavailable",1,true) and dead_reads==0,"expired native weak hit target is never dereferenced or replaced")

-- Exercise the real bridge with the pinned UE4SS native POST convention:
-- context, return value, reflected parameters. A bool return must never be
-- mistaken for the WorldContextObject and silently discard every trace.
local source=assert(io.open(T.path("mods/HSMPCombat/Scripts/main.lua"),"r"))
local text=source:read("*a");source:close()
local bridge=assert(text:match("(function C3%.native_trace_post.-\nend)"))
local trace={pawn=5}
local state={native_probe=true,replay_trace=trace,armor_audit=a}
local env={C3=state,replaying=true,WG={check=function()return true end,settled=function()return true end},
    pv=unwrap,addr_of=function(o)return o:GetAddress()end}
setmetatable(env,{__index=_G})
assert(load(bridge,"native_trace_post","t",env))()
local result=state.native_trace_post(param(obj("Engine context",90)),param(true),param(pawn),
    param({X=1,Y=2,Z=3}),param({X=4,Y=5,Z=6}),.1,arr({11}),true,arr({}),0,
    arr({hit}),false,nil,nil,5)
T.check(result==nil and trace.proxy_trace_calls==1,"native POST bridge preserves return value and observes exactly the owner trace")
T.check(trace.proxy_trace_samples[1]:find("out:true,hits:1",1,true),"native POST return/output arguments reach actual ordered-hit evidence")
state.native_trace_post(nil,param(false),param(obj("other pawn",99)))
T.check(trace.proxy_trace_calls==1,"trace from another native receiver cannot be attributed to owner replay")
state.native_probe=false
T.check(pcall(state.native_trace_post,nil,param(true),nil),"disabled bridge reads no native world parameter")
