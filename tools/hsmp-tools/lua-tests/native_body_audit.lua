local M=dofile(T.path("mods/HSMPCombat/Scripts/native_body_audit.lua"))
local TOPOLOGY=dofile(T.path("mods/HSMPCombat/Scripts/native_topology_audit.lua"))
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
pawn.GetFName=function()return fn("Willie_1")end
for _,k in ipairs(TOPOLOGY.FLAGS)do pawn[k]=false end
local joint=obj(40,"PhysicsConstraint native arm",{IsBroken=function()return false end,
    GetCurrentSwing1=function()return 87.8 end,GetCurrentSwing2=function()return 178 end,GetCurrentTwist=function()return 0 end})
pawn["Dislocated Bone Constraint Arm L"]=joint
pawn["Dismembered Body Constraints"]=map({{fn("upperarm_l"),joint}})
local ctx={world="10@World arena",peer=7,match_id=11,round=3,life=130,pawn="Willie_1",actor=1,mesh=2}
local enabled,contexts,logs=false,0,{}
local a=M.new{enabled=function()return enabled end,unwrap=unwrap,fname=fn,topology_reader=TOPOLOGY,
    context=function()contexts=contexts+1;return ctx end,log=function(f,...)logs[#logs+1]=string.format(f,...)end}
T.check(a.capture(pawn,"disabled")==nil and contexts==0 and #bone_calls==0,"disabled audit performs no native context or body reads")
enabled=true
local r=a.capture(pawn,"Get Damage POST",{pre="unavailable:Blueprint_POST",hit_id=91,parent_cid=88})
T.check(r.context.life==130 and r.context.world=="10@World arena" and r.context.mesh==2,"full life and native world/actor/mesh context retained")
T.check(r.health.Health==100 and r.health["Arm_L Health"]==0 and r.health["Head Health"]==nil,"fresh native part zero differs from unreadable health")
T.check(r.flags["Arm L Broken"]==false and r.flags["Arm L Dislocated"]==true and r.flags["Neck Snapped"]==nil,"individual native joint flag availability is independent")
T.check(r.dism_array.available and r.dism_array.count==0 and #r.dism_array.values==0,"complete empty legacy array is observed independently from the positive native ledger")
T.check(r.dism_bones.available and r.dism_bones.values[1]=="hand_l","distinct native dismembered-bones array is recorded")
T.check(r.parts.available and r.parts.values[1].part==3 and r.parts.values[1].value==false and r.parts.values[2].part==8,"typed part map records actual enum keys and false values without bone mapping")
T.check(r.topology.available and r.topology.version==1 and r.topology.read_complete
    and r.topology.part_enum==TOPOLOGY.PART_ENUM,"production body snapshot calls the reviewed reader with complete native identity and availability")
T.check(r.parts.present_mask==((1<<3)|(1<<8)) and r.parts.true_mask==1<<8
    and r.parts.count_check=="iteration","native fifteen-part presence and true values remain distinct from a bone mask")
T.check(r.topology.flags.Headless.available and r.flags.Headless==false and r.topology.sever_mask==nil,
    "available false structural flag and positive native ledger never synthesize an intact or sever bone mask")
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
T.check(logs[1]:find("topology_available=true topology_version=1 topology_read_complete=true",1,true)
    and logs[1]:find("native_part_present_mask=264 native_part_true_mask=256",1,true)
    and logs[1]:find("native_part_enum="..TOPOLOGY.PART_ENUM,1,true)
    and logs[1]:find("topology_flag_Headless_available=true",1,true),
    "body log identifies versioned native enum proof and independent structural availability")
T.check(logs[1]:find("pre=unavailable:Blueprint_POST",1,true) and writes==0,"Blueprint POST never advertises cached PRE or writes native physics")
pawn["Dismembered Array"]=nil;pawn["Dismembered Bones"]=arr({fn("hand_l")},2)
local missing=a.capture(pawn,"missing")
T.check(not missing.dism_array.available and not missing.dism_bones.available and missing.dism_bones.values==nil,"missing/torn arrays do not become intact zero or partial entries")
T.check(missing.health.Health==100 and missing.topology.available and not missing.topology.read_complete
    and missing.parts.available and missing.topology.flags.Headless.available,
    "incomplete topology preserves current HP and independent valid native map and flag evidence")
pawn.Headless=nil
local unknown_flag=a.capture(pawn,"missing flag")
T.check(not unknown_flag.topology.flags.Headless.available and unknown_flag.flags.Headless==nil
    and unknown_flag.health.Health==100 and unknown_flag.parts.true_mask==1<<8,
    "unavailable structural flag remains unknown without stopping scalar health or valid positive topology")
T.check(logs[#logs]:find("topology_flag_Headless_available=false",1,true)
    and logs[#logs]:find("flag_Headless=unavailable",1,true),"native flag log never coerces a failed read to false")
pawn.Headless=false
pawn["Dismembered Parts Map"]=map({{8,0}})
T.check(not a.capture(pawn,"bad map").parts.available,"nonboolean native map value remains unavailable")
pawn["Dismembered Parts Map"]=map({{23,true}})
T.check(not a.capture(pawn,"bad enum").parts.available,"wire bone enum cannot masquerade as native part enum")
pawn["Dismembered Parts Map"]=map({})
T.check(a.capture(pawn,"empty map").parts.available,"successful empty typed map iteration remains explicit observed empty")
-- The standalone probe must work without an optional reader, but it may not
-- revive the legacy count-only topology path or advertise a synthetic zero.
local absent_logs,legacy_reads={},0
pawn["Dismembered Array"]={GetArrayNum=function()legacy_reads=legacy_reads+1;return 0 end,
    ForEach=function()legacy_reads=legacy_reads+1 end}
local function audit_with(reader)
    return M.new{enabled=function()return true end,unwrap=unwrap,fname=fn,topology_reader=reader,
        context=function()return ctx end,log=function(f,...)absent_logs[#absent_logs+1]=string.format(f,...)end}
end
local no_reader=audit_with(nil).capture(pawn,"reader absent")
T.check(no_reader.health.Health==100 and not no_reader.topology.available
    and no_reader.topology.reason=="reader unavailable","missing optional reader keeps native scalar diagnostics and explicitly unavailable topology")
T.check(not no_reader.dism_array.available and not no_reader.dism_bones.available and not no_reader.parts.available
    and no_reader.parts.count==nil and no_reader.parts.present_mask==nil and no_reader.parts.true_mask==nil
    and no_reader.flags.Headless==nil and legacy_reads==0,"missing reader never reads a legacy fallback or substitutes empty topology and flags")
T.check(absent_logs[#absent_logs]:find("topology_available=false",1,true)
    and absent_logs[#absent_logs]:find("native_part_present_mask=unavailable native_part_true_mask=unavailable",1,true)
    and absent_logs[#absent_logs]:find("parts=unavailable",1,true)
    and not absent_logs[#absent_logs]:find("parts=[]",1,true),"unconfigured body log clearly separates unavailable topology from observed native emptiness")
local thrown=audit_with({read=function()error("reader unavailable")end}).capture(pawn,"reader failure")
T.check(thrown.health.Health==100 and not thrown.topology.available and thrown.parts.values==nil,
    "reader failure cannot erase native health or masquerade as a complete topology snapshot")
pawn.GetFName=function()return fn("Willie_foreign")end
local foreign=a.capture(pawn,"foreign native name")
T.check(foreign.health.Health==100 and not foreign.topology.available
    and foreign.topology.reason=="native identity unavailable or changed" and legacy_reads==0,
    "actual native FName mismatch fails the production reader before collection reads")
pawn.GetFName=function()return fn("Willie_1")end
pawn["Dismembered Array"]=arr({})
local captured=a.capture(pawn,"old life")
local saved_ctx=ctx
ctx={};for k,v in pairs(saved_ctx)do ctx[k]=v end;ctx.life=131
pawn["Dismembered Parts Map"]=map({{14,true}})
local next_life=a.capture(pawn,"new life")
T.check(next_life.topology.context.life==131 and next_life.parts.true_mask==1<<14
    and captured.topology.context.life==130 and captured.parts.true_mask==0,
    "same actor new life reads fresh topology without relabeling or retaining the previous native ledger")
ctx=saved_ctx;pawn["Dismembered Parts Map"]=map({})
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
local b=M.new{enabled=function()return true end,unwrap=unwrap,fname=fn,topology_reader=TOPOLOGY,context=function()
    changed_contexts=changed_contexts+1;local c={};for k,v in pairs(old)do c[k]=v end
    if changed_contexts>1 then c.life=131 end;return c end,log=function()error("mixed life logged")end}
T.check(b.capture(pawn,"transition")==nil,"context changing during native read drops mixed-life evidence")
local hooks={};local registrations=0
-- Pinned LuaMod.cpp:88 uses find-first + substr, not a starts-with test.
local function native_lookup(path)
    local _,last=path:find("Function ",1,true)
    return last and path:sub(last+1) or path
end
local function hook_path(event)return "/Game/Character/Blueprints/Willie_BP.Willie_BP_C:"..event end
for i,event in ipairs(M.HOOKS)do
    T.check(native_lookup(hook_path(event))==(i==1 and "Initiate" or "Delayed"),
        "unprefixed embedded Function name reproduces actual bare native lookup failure")
    T.check(native_lookup("Function "..hook_path(event))==hook_path(event),
        "leading supported Function prefix preserves the full reflected target through pinned parser")
end
local function missing_class()error("class unavailable")end
T.check(not a.install(missing_class),"native sever hooks unavailable explicitly until class loads")
T.check(logs[#logs]:find("error=",1,true) and logs[#logs]:find("class_unavailable",1,true)
    and logs[#logs]:find('path="Function /Game/Character/Blueprints/Willie_BP.Willie_BP_C:Dismember Function Delayed"',1,true),
    "actual native hook failure and unsanitized exact reflected path remain explicit")
local failed_logs=#logs
a.install(missing_class)
T.check(#logs==failed_logs,"same native hook error retries without per-tick log flooding")
a.install(function()error("function flags unavailable")end)
T.check(#logs==failed_logs+2 and logs[#logs]:find("function_flags_unavailable",1,true),
    "changed native registration exception is reported independently for both functions")
T.check(a.install(function(path,callback,post)registrations=registrations+1
    local resolved=native_lookup(path)
    T.check(resolved==hook_path(M.HOOKS[1]) or resolved==hook_path(M.HOOKS[2]),
        "actual registration argument resolves to exact reflected spaced function through native parser")
    hooks[resolved]=callback
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
