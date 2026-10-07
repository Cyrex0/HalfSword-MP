local M=dofile(T.path("mods/HSMPCombat/Scripts/native_body_audit.lua"))
local function param(v)return {get=function()return v end}end
local function unwrap(v)if type(v)=="table" and v.get then return v:get()end;return v end
local function fn(v)return {ToString=function()return v end}end
local function arr(v,n)return {GetArrayNum=function()return n or #v end,
    ForEach=function(_,f)for i,x in ipairs(v)do f(i,param(x))end end}end
local function map(v)return {ForEach=function(_,f)for _,x in ipairs(v)do f(param(x[1]),param(x[2]))end end}end
local function obj(id,name,fields)
    local x=fields or {};x.IsValid=function()return true end;x.GetAddress=function()return id end
    x.GetFullName=function()return name end;return x
end
local physics=obj(30,"PhysicsAsset capsules")
local asset=obj(31,"SkeletalMesh Willie",{PhysicsAsset=physics})
local writes,bone_calls,com_calls=0,{},0
local mesh=obj(2,"SkeletalMeshComponent Mesh",{
    GetSkeletalMeshAsset=function()return asset end,GetCollisionEnabled=function()return 3 end,
    K2_GetComponentToWorld=function()return {Translation={X=1,Y=2,Z=3},Scale3D={X=1,Y=1,Z=1},Rotation={X=0,Y=0,Z=0,W=1}}end,
    GetBoneIndex=function(_,b)bone_calls[#bone_calls+1]=b:ToString();return b:ToString()=="hand_l" and 4 or -1 end,
    IsBoneHiddenByName=function(_,b)return b:ToString()=="hand_l" end,
    IsSimulatingPhysics=function()return true end,GetBoneMass=function(_,b,scaled)assert(scaled==true);return 2.5 end,
    GetCenterOfMass=function()com_calls=com_calls+1;return {X=3,Y=4,Z=5}end,
    SetSimulatePhysics=function()writes=writes+1 end,HideBoneByName=function()writes=writes+1 end})
local pawn=obj(1,"Willie owner",{Mesh=mesh,Health=100,["Arm_L Health"]=0,["Arm L Broken"]=false,
    ["Arm L Dislocated"]=true,["Dismembered Array"]=arr({}),["Dismembered Bones"]=arr({fn("hand_l")}),
    ["Dismembered Parts Map"]=map({{8,true},{3,false}}),["Spawn Bone"]=fn("lowerarm_l"),
    ["Dismember Child Parts"]=arr({mesh}),["Dismember Parent Parts"]=arr({}),["Worn Armor"]=arr({})})
local joint=obj(40,"PhysicsConstraint native arm",{IsBroken=function()return false end,
    GetCurrentSwing1=function()return 87.8 end,GetCurrentSwing2=function()return 178 end,GetCurrentTwist=function()return 0 end})
pawn["Dislocated Bone Constraint Arm L"]=joint
pawn["Dismembered Body Constraints"]=map({{fn("upperarm_l"),joint}})
local ctx={world="10@World arena",peer=7,match_id=11,round=3,life=130,pawn="Willie_1",actor=1,mesh=2}
local enabled,contexts,logs=false,0,{}
local a=M.new{enabled=function()return enabled end,unwrap=unwrap,fname=fn,
    context=function()contexts=contexts+1;return ctx end,log=function(f,...)logs[#logs+1]=string.format(f,...)end}
T.check(a.capture(pawn,"disabled")==nil and contexts==0 and #bone_calls==0,"disabled audit performs no native context or body reads")
enabled=true
local r=a.capture(pawn,"Get Damage POST",{pre="unavailable:Blueprint_POST",hit_id=91,parent_cid=88})
T.check(r.context.life==130 and r.context.world=="10@World arena" and r.context.mesh==2,"full life and native world/actor/mesh context retained")
T.check(r.health.Health==100 and r.health["Arm_L Health"]==0 and r.health["Head Health"]==nil,"fresh native part zero differs from unreadable health")
T.check(r.flags["Arm L Broken"]==false and r.flags["Arm L Dislocated"]==true and r.flags["Neck Snapped"]==nil,"individual native joint flag availability is independent")
T.check(r.dism_array.available and r.dism_array.count==0 and #r.dism_array.values==0,"complete empty legacy array is an observed zero")
T.check(r.dism_bones.available and r.dism_bones.values[1]=="hand_l","distinct native dismembered-bones array is recorded")
T.check(r.parts.available and r.parts.values[1].part==3 and r.parts.values[1].value==false and r.parts.values[2].part==8,"typed part map records actual enum keys and false values without bone mapping")
local hand=r.components.Mesh.bones[13]
T.check(hand.bone=="hand_l" and hand.index==4 and hand.hidden==true and hand.sim==true and hand.mass==2.5 and hand.com[3]==5,"exact native hidden/physics/mass/COM queries use FName bone and scaled mass")
T.check(r.components.Mesh.bones[1].index==-1 and r.components.Mesh.bones[1].hidden==nil,"absent bone index does not invent visible/intact state")
T.check(r.components.Mesh.physics_asset.address==30 and r.components.Mesh.asset.address==31,"effective native physics asset is read from actual skeletal asset")
T.check(r.components["Dismember Child Parts"].available and #r.components["Dismember Child Parts"].values==1,"actual same-pawn sever child components recorded")
T.check(r.constraints["Dislocated Bone Constraint Arm L"].broken==false
    and r.constraints["Dislocated Bone Constraint Arm L"].swing1==87.8,"actual native joint broken state and angles read independently")
T.check(r.constraints.dismembered.available and r.constraints.dismembered.values[1].bone=="upperarm_l"
    and r.constraints.dismembered.values[1].component.identity.address==40,"native FName-to-constraint map retains actual topology without part mapping")
T.check(logs[1]:find("hp_Arm_L_Health=0",1,true) and logs[1]:find("flag_Arm_L_Broken=false",1,true)
    and logs[1]:find("flag_Neck_Snapped=unavailable",1,true),"log preserves zero/false/unavailable separately")
T.check(logs[1]:find("dism_array=[]",1,true) and logs[1]:find("parts=[3:false,8:true]",1,true),"log separates actual empty array from typed sever map entries")
T.check(logs[1]:find("pre=unavailable:Blueprint_POST",1,true) and writes==0,"Blueprint POST never advertises cached PRE or writes native physics")
pawn["Dismembered Array"]=nil;pawn["Dismembered Bones"]=arr({fn("hand_l")},2)
local missing=a.capture(pawn,"missing")
T.check(not missing.dism_array.available and not missing.dism_bones.available and missing.dism_bones.values==nil,"missing/torn arrays do not become intact zero or partial entries")
pawn["Dismembered Parts Map"]=map({{8,0}})
T.check(not a.capture(pawn,"bad map").parts.available,"nonboolean native map value remains unavailable")
pawn["Dismembered Parts Map"]=map({{23,true}})
T.check(not a.capture(pawn,"bad enum").parts.available,"wire bone enum cannot masquerade as native part enum")
pawn["Dismembered Parts Map"]=map({})
T.check(a.capture(pawn,"empty map").parts.available,"successful empty typed map iteration remains explicit observed empty")
mesh.IsSimulatingPhysics=function()error("native state unavailable")end
local old_com=com_calls
T.check(a.capture(pawn,"unread sim").components.Mesh.bones[13].sim==nil and com_calls==old_com,"unreadable simulation never coerces false or reads a fake COM")
mesh.PhysicsAssetOverride=obj(32,"Override sever capsules")
T.check(a.capture(pawn,"override").components.Mesh.physics_asset.address==32,"native override takes precedence over skeletal asset physics")
local old=ctx
for _,k in ipairs({"actor","mesh","life","world"})do
    ctx={};for key,v in pairs(old)do ctx[key]=v end
    if k=="world" then ctx.world="" elseif k=="life" then ctx.life=0 else ctx[k]=99 end
    T.check(a.capture(pawn,"stale")==nil,"invalid exact "..k.." context is refused")
end
ctx=old
local changed_contexts=0
local b=M.new{enabled=function()return true end,unwrap=unwrap,fname=fn,context=function()
    changed_contexts=changed_contexts+1;local c={};for k,v in pairs(old)do c[k]=v end
    if changed_contexts>1 then c.life=131 end;return c end,log=function()error("mixed life logged")end}
T.check(b.capture(pawn,"transition")==nil,"context changing during native read drops mixed-life evidence")
local hooks={};local registrations=0
local function missing_class()error("class unavailable")end
T.check(not a.install(missing_class),"native sever hooks unavailable explicitly until class loads")
T.check(logs[#logs]:find("error=",1,true) and logs[#logs]:find("class_unavailable",1,true)
    and logs[#logs]:find('path="/Game/Character/Blueprints/Willie_BP.Willie_BP_C:Dismember Function Delayed"',1,true),
    "actual native hook failure and unsanitized exact reflected path remain explicit")
local failed_logs=#logs
a.install(missing_class)
T.check(#logs==failed_logs,"same native hook error retries without per-tick log flooding")
a.install(function()error("function flags unavailable")end)
T.check(#logs==failed_logs+2 and logs[#logs]:find("function_flags_unavailable",1,true),
    "changed native registration exception is reported independently for both functions")
T.check(a.install(function(path,callback,post)registrations=registrations+1;hooks[path]=callback
    T.check(path=="/Game/Character/Blueprints/Willie_BP.Willie_BP_C:Dismember Function Initiate"
        or path=="/Game/Character/Blueprints/Willie_BP.Willie_BP_C:Dismember Function Delayed",
        "hook registration uses exact reflected spaced function names")
    T.check(post==nil,"native Blueprint registration has only its actual POST callback")
    return registrations,registrations end),"both exact native sever hooks retry")
T.check(logs[#logs]:find("pre_id=2 post_id=2 error=none",1,true),"native hook IDs are reported only from successful registration return values")
for _,ids in ipairs({{}, {false,false}, {1,nil}, {1,2}, {-2147483649,-2147483649}, {2147483648,2147483648}, {1.5,1.5}})do
    local calls,ambiguous_logs=0,{}
    local unknown=M.new{enabled=function()return false end,log=function(f,...)ambiguous_logs[#ambiguous_logs+1]=string.format(f,...)end}
    local function register_unknown()calls=calls+1;return ids[1],ids[2]end
    T.check(not unknown.install(register_unknown) and calls==2
        and ambiguous_logs[1]:find("invoked=true registered=unavailable ambiguous=true",1,true),
        "successful invocation with unavailable or invalid Blueprint hook IDs is not coverage proof")
    unknown.install(register_unknown);unknown.clear();unknown.install(register_unknown)
    T.check(calls==2 and #ambiguous_logs==2,"ambiguous successful registration is never retried or duplicated across world-drop")
end
for _,id in ipairs({-2147483648,2147483647})do
    local boundary=M.new{enabled=function()return false end,log=function()end}
    T.check(boundary.install(function()return id,id end),"Blueprint IDs retain the pinned API's complete signed int32 return domain")
end
a.install(function()registrations=registrations+1 end)
T.check(registrations==2,"successful native sever registration occurs once per function")
local first=#logs
local prefix="/Game/Character/Blueprints/Willie_BP.Willie_BP_C:"
hooks[prefix..M.HOOKS[1]](param(pawn),param(mesh),param(8),nil,nil,nil,nil,param(mesh))
hooks[prefix..M.HOOKS[2]](param(pawn),param(mesh),param(8),nil,nil,nil,nil,param(mesh))
T.check(logs[first+1]:find("part=8 master=2 weapon=2",1,true) and logs[first+2]:find("part=8 master=2 weapon=2",1,true),"Blueprint hook argument order has no inserted ReturnValue")
local pair=logs[first+1]:match("pair=(%d+)")
T.check(pair and logs[first+2]:find("pair="..pair.." ",1,true),"Initiate/Delayed pair binds exact owner context, master mesh and native part")
hooks[prefix..M.HOOKS[1]](param(pawn),param(mesh),param(8),nil,nil,nil,nil,param(mesh));a.clear()
hooks[prefix..M.HOOKS[2]](param(pawn),param(mesh),param(8),nil,nil,nil,nil,param(mesh))
T.check(logs[#logs]:find("pair=unavailable",1,true),"world-drop clears sever callback correlation without retained UObjects")
T.check(writes==0,"all callback/readback paths remain read-only")
