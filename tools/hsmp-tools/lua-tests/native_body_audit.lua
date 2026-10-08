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
    x.GetFullName=function()return name end;x.GetFName=function()return fn(name)end;return x
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
local world=obj(10,"World arena")
pawn.GetWorld=function()return world end
local parent=obj(50,"SceneComponent arm parent",{GetWorld=function()return world end})
local primitive_class=obj(51,"Class /Script/Engine.PrimitiveComponent")
local box_class=obj(52,"Class /Script/Engine.BoxComponent")
local mesh_class=obj(53,"Class /Script/Engine.SkeletalMeshComponent")
local function xf(x)
    return {Translation={X=x,Y=2,Z=3},Scale3D={X=1,Y=2,Z=3},Rotation={X=0,Y=0,Z=0,W=1}}
end
local function geometry(c,kind,x,tags)
    c.GetClass=function()return kind=="box" and box_class or kind=="mesh" and mesh_class or primitive_class end
    c.IsA=function(_,wanted)
        return wanted=="/Script/Engine.PrimitiveComponent" or wanted=="/Script/Engine.BoxComponent" and kind=="box"
            or wanted=="/Script/Engine.SkeletalMeshComponent" and kind=="mesh"
    end
    c.GetWorld=function()return world end;c.GetOwner=function()return pawn end
    c.GetAttachParent=function()return parent end;c.GetAttachSocketName=function()return fn("lowerarm_l")end
    c.ComponentTags=arr(tags or {fn("Arm_L"),fn("HP34.5")})
    c.K2_GetComponentToWorld=function()return xf(x)end
    c.GetCollisionEnabled=function()return 3 end
    if kind=="box"then c.GetUnscaledBoxExtent=function()return {X=4,Y=500,Z=6}end end
    return c
end
geometry(mesh,"mesh",1,{fn("Dismember Pass")})
local attach=geometry(obj(60,"PrimitiveComponent attach marker"),"primitive",4)
local marker1=geometry(obj(61,"PrimitiveComponent near marker"),"primitive",5,{fn("Arm_L"),fn("HP90")})
local marker2=geometry(obj(62,"PrimitiveComponent far marker"),"primitive",6,{fn("Arm_L"),fn("HP0")})
local box1=geometry(obj(63,"BoxComponent cut one"),"box",7)
local box2=geometry(obj(64,"BoxComponent cut two"),"box",8)
local weapon=geometry(obj(65,"PrimitiveComponent source blade"),"primitive",9)
pawn["Currently Dismembered Part"],pawn["Currently Dismembered Mesh"],pawn["Setup Armor in Process"]=6,mesh,false
local function sever(event,part,markers,b1,b2)
    return a.sever_snapshot(event,param(pawn),param(mesh),param(part or 8),param(attach),
        param(markers or arr({marker2,marker1})),param(b1 or box1),param(b2 or box2),param(weapon))
end
local initial=sever(M.HOOKS[1])
T.check(initial.sever_inputs.part.available and initial.sever_inputs.part.value==8
    and initial.sever_inputs.current_part.value==6,"native callback part and mutable selected part remain separate POST evidence")
local inputs=initial.sever_inputs
T.check(inputs.version==1 and inputs.read_complete and inputs.observation=="Blueprint_POST" and inputs.eligibility=="unavailable",
    "all seven actual Blueprint inputs can be captured without claiming native eligibility or PRE marker wear")
T.check(inputs.master.identity.address==2 and inputs.attach_marker.identity.address==60
    and inputs.box1.identity.address==63 and inputs.box2.identity.address==64 and inputs.weapon.identity.address==65,
    "seven-input callback retains distinct typed master, attach, box and weapon identities in SDK parameter order")
T.check(inputs.master.class_match and inputs.master.asset.address==31 and inputs.master.physics_asset.address==32,
    "actual master body section and effective physics asset are read independently from the owner's primary mesh")
T.check(inputs.attach_marker.parent.address==50 and inputs.attach_marker.socket=="lowerarm_l"
    and inputs.attach_marker.tags.values[2]=="HP34.5","native marker attachment and exact HP/part tags are copied without interpreting them as damage eligibility")
T.check(inputs.overlapped_markers.available and inputs.overlapped_markers.count==2
    and inputs.overlapped_markers.values[1].index==1 and inputs.overlapped_markers.values[1].identity.address==62
    and inputs.overlapped_markers.values[2].identity.address==61,"pinned one-based native marker-array order is preserved without sorting")
T.check(inputs.box1.extent.available and inputs.box1.extent.Y==500 and inputs.box1.transform.scale.Y==2,
    "native unscaled box extent is distinguished from transform scale")
box1.K2_GetComponentToWorld=function()return xf(77)end
local delayed=sever(M.HOOKS[2],8,nil,box2,box1)
T.check(delayed.sever_inputs.box1.identity.address==64 and delayed.sever_inputs.box2.transform.translation.X==77
    and inputs.box1.transform.translation.X==7,"Delayed takes fresh selected-box order and transforms without mutating the Initiate snapshot")
T.check(logs[#logs]:find("sever_observation=Blueprint_POST sever_eligibility=unavailable",1,true)
    and logs[#logs]:find("sever_markers_available=true sever_markers_n=2",1,true)
    and logs[#logs]:find("unscaled_extent:true/4/500/6",1,true),"native sever log includes explicit POST-only geometry and actual ordered marker availability")
local exact_tags={"Arm L","Arm_L",'HP|;,="\\\n\t[]:/',string.rep("x",300),"臂左"}
local native_tags={};for _,v in ipairs(exact_tags)do native_tags[#native_tags+1]=fn(v)end
attach.ComponentTags=arr(native_tags)
local exact=sever(M.HOOKS[1])
local persisted=logs[#logs]:match("sever_attach_marker=([^ ]+)")
local json_tags=persisted and persisted:match("tags:true/5/(%b[])")
local decoded=json_tags and T.json_decode(json_tags)
T.check(decoded and decoded[1]==exact_tags[1] and decoded[2]==exact_tags[2] and decoded[1]~=decoded[2],
    "persisted quoted tags keep native spaces distinct from underscores")
T.check(decoded and decoded[3]==exact_tags[3] and decoded[5]==exact_tags[5]
    and not json_tags:find("\n",1,true),"persisted tags round-trip delimiters, quotes, backslashes, control bytes and UTF-8 exactly without splitting log rows")
T.check(decoded and decoded[4]==exact_tags[4] and #decoded[4]==300
    and exact.sever_inputs.attach_marker.tags.available and exact.sever_inputs.read_complete,
    "available tag longer than the display-token limit persists in full")
attach.ComponentTags=arr({fn(string.rep("x",M.SEVER_TAG_BYTES+1))})
local oversized=sever(M.HOOKS[1])
T.check(not oversized.sever_inputs.attach_marker.tags.available and oversized.sever_inputs.attach_marker.tags.values==nil
    and oversized.sever_inputs.attach_marker.tags.truncated and not oversized.sever_inputs.read_complete,
    "over-limit native tag makes the collection explicitly unavailable rather than publishing a truncated exact tag")
T.check(logs[#logs]:find("tags_encoding:json|tags_byte_limit:1024|tags_truncated:true|tags_reason:tag_byte_limit_exceeded",1,true),
    "persisted tag limit failure reports its bound and truncation reason")
attach.ComponentTags=arr({fn("Arm_L"),fn("HP34.5")})
local partial=sever(M.HOOKS[1],8,arr({marker1},2))
T.check(not partial.sever_inputs.overlapped_markers.available and partial.sever_inputs.overlapped_markers.values==nil
    and partial.health.Health==100,"partial native marker-array iteration stays unavailable while fresh body health is retained")
local overflow=sever(M.HOOKS[1],8,{GetArrayNum=function()return 65 end,
    ForEach=function()error("over-limit native marker array iterated")end})
T.check(not overflow.sever_inputs.overlapped_markers.available and overflow.sever_inputs.overlapped_markers.count==65
    and overflow.sever_inputs.overlapped_markers.values==nil,"observed over-limit native count is retained without iterating or truncating the marker array")
local stopped=sever(M.HOOKS[1],8,{GetArrayNum=function()return 2 end,
    ForEach=function(_,f)f(1,param(marker1));error("native reference array interrupted")end})
T.check(not stopped.sever_inputs.overlapped_markers.available and stopped.sever_inputs.overlapped_markers.values==nil,
    "failed native out/reference array read never publishes its partial first marker")
local tag_count=0
marker1.ComponentTags={GetArrayNum=function()tag_count=tag_count+1;return tag_count end,
    ForEach=function(_,f)f(1,param(fn("HP50")))end}
local moving_tags=sever(M.HOOKS[1])
T.check(not moving_tags.sever_inputs.overlapped_markers.values[2].tags.available,
    "native marker tags changing count across iteration remain explicitly unavailable")
marker1.ComponentTags=arr({fn("Arm_L"),fn("HP90")})
local bad_type=sever(M.HOOKS[1],8,nil,weapon)
T.check(not bad_type.sever_inputs.box1.available and bad_type.sever_inputs.box1.class_match==false
    and bad_type.sever_inputs.box1.extent==nil,"wrong actual component class cannot become a valid box or guessed extent")
weapon.GetWorld=function()return obj(99,"World departed")end
local wrong_world=sever(M.HOOKS[1])
T.check(not wrong_world.sever_inputs.weapon.available and not wrong_world.sever_inputs.read_complete,
    "callback weapon in a foreign native world cannot supply current geometry")
weapon.GetWorld=function()return world end
box1.GetUnscaledBoxExtent=function()error("native extent unavailable")end
local no_extent=sever(M.HOOKS[1])
T.check(no_extent.sever_inputs.box1.available and not no_extent.sever_inputs.box1.extent.available
    and no_extent.sever_inputs.box1.extent.X==nil and not no_extent.sever_inputs.read_complete,
    "unreadable box extent never falls back to a default or guessed cut size")
box1.GetUnscaledBoxExtent=function()return {X=4,Y=500,Z=6}end
marker1.ComponentTags=nil
local no_tags=sever(M.HOOKS[1])
T.check(no_tags.sever_inputs.overlapped_markers.available and not no_tags.sever_inputs.overlapped_markers.read_complete
    and not no_tags.sever_inputs.overlapped_markers.values[2].tags.available,
    "valid array identity/count does not hide unavailable individual marker tags")
marker1.ComponentTags=arr({fn("Arm_L"),fn("HP90")})
local invalid_part=a.sever_snapshot(M.HOOKS[1],param(pawn),param(mesh),param(23))
T.check(not invalid_part.sever_inputs.part.available and invalid_part.sever_inputs.part.value==nil
    and not invalid_part.sever_inputs.overlapped_markers.available,"wire bone key and missing callback inputs never become native part zero or an empty array")
local function plain(v)
    if type(v)=="table"then
        if getmetatable(v)then return false end
        for k,x in pairs(v)do if not plain(k) or not plain(x)then return false end end
        return true
    end
    return type(v)=="string" or type(v)=="number" or type(v)=="boolean" or v==nil
end
T.check(plain(inputs),"sever evidence contains only copied scalar identities/geometry and no native objects or wrappers")
local disabled_unwraps=0
local disabled=M.new{enabled=function()return false end,unwrap=function()disabled_unwraps=disabled_unwraps+1;error("disabled callback touched")end}
T.check(disabled.sever_snapshot(M.HOOKS[1],param(pawn))==nil and disabled_unwraps==0,
    "disabled native sever audit unwraps none of the seven input wrappers")
local original_enabled=enabled
local before_drop=#logs
box1.K2_GetComponentToWorld=function()enabled=false;return xf(7)end
T.check(sever(M.HOOKS[1])==nil and #logs==before_drop,"world/admission loss during geometry drops the whole row rather than mixing lifetimes")
enabled=original_enabled;box1.K2_GetComponentToWorld=function()return xf(7)end
local source_ctx={};for k,v in pairs(ctx)do source_ctx[k]=v end;source_ctx.peer=2;source_ctx.side="source"
local source=M.new{enabled=function()return true end,unwrap=unwrap,fname=fn,topology_reader=TOPOLOGY,
    context=function()return nil end,source_context=function()return source_ctx end,
    log=function(f,...)logs[#logs+1]=string.format(f,...)end}
T.check(source.capture(pawn,"manual source") == nil,"manual/replay body admission stays owner-only when source callback admission exists")
local source_row=source.sever_snapshot(M.HOOKS[1],param(pawn),param(mesh),param(8),param(attach),
    param(arr({marker2,marker1})),param(box1),param(box2),param(weapon))
T.check(source_row.context.peer==2 and source_row.context.side=="source" and source_row.sever_inputs.read_complete
    and logs[#logs]:find("sever_side=source",1,true),"source callback uses separately supplied full-scope diagnostic admission with no owner fallback")
-- Existing hook-order/correlation checks still use the actual production bridge.
first=#logs
hooks[prefix..M.HOOKS[1]](param(pawn),param(mesh),param(8),nil,nil,nil,nil,param(mesh))
hooks[prefix..M.HOOKS[2]](param(pawn),param(mesh),param(8),nil,nil,nil,nil,param(mesh))
T.check(logs[first+1]:find("part=8 master=2 weapon=2",1,true) and logs[first+2]:find("part=8 master=2 weapon=2",1,true),"Blueprint hook argument order has no inserted ReturnValue")
local pair=logs[first+1]:match("pair=(%d+)")
T.check(pair and logs[first+2]:find("pair="..pair.." ",1,true),"Initiate/Delayed pair binds exact owner context, master mesh and native part")
hooks[prefix..M.HOOKS[1]](param(pawn),param(mesh),param(8),nil,nil,nil,nil,param(mesh));a.clear()
hooks[prefix..M.HOOKS[2]](param(pawn),param(mesh),param(8),nil,nil,nil,nil,param(mesh))
T.check(logs[#logs]:find("pair=unavailable",1,true),"world-drop clears sever callback correlation without retained UObjects")
T.check(writes==0,"all callback/readback paths remain read-only")
-- Native dislocation samples use the actual parent-bone-space getter pair,
-- never the component/world transform already logged for general body audit.
local poses={upperarm_r={X=20,Y=2,Z=3},upperarm_l={X=15,Y=2,Z=3}}
local socket_calls={}
local driver=geometry(obj(70,"SkeletalMeshComponent DriverSkeleton"),"mesh",1000)
local old_bone_index=mesh.GetBoneIndex
mesh.GetBoneIndex=function(_,b)return poses[b:ToString()] and 12 or -1 end
driver.GetBoneIndex=mesh.GetBoneIndex
local function socket(c,b,space)
    local bone=b:ToString();socket_calls[#socket_calls+1]={id=c:GetAddress(),bone=bone,space=space}
    assert(space==3 and (bone=="upperarm_r" or bone=="upperarm_l"),"exact native getter inputs")
    return {Translation=c==mesh and poses[bone] or {X=0,Y=2,Z=3}}
end
mesh.GetSocketTransform=socket;driver.GetSocketTransform=socket
pawn.DriverSkeleton=driver;pawn["Bone Snapping"]=1.5;pawn["Block Spine Breaking"]=false
pawn["Arm R Dislocated"]=false;pawn["Arm L Dislocated"]=false
pawn["Dismembered Parts Map"]=map({{3,false},{6,false},{8,true}})
local observed=a.capture(pawn,"dislocation inputs")
local di=observed.dislocation_inputs
T.check(di.available and di.read_complete and di.space==3 and di.space_name=="RTS_ParentBoneSpace"
    and #socket_calls==4,"two proved arm bones are sampled once per component using SDK parent-bone space3")
T.check(di.pawn.address==1 and di.pawn.fname=="Willie_1" and di.mesh.identity.address==2
    and di.driver.identity.address==70 and di.driver.owner.address==1 and di.driver.owner.world==ctx.world,
    "dislocation readback binds actual pawn, Mesh, DriverSkeleton and owner/world identities")
T.check(di.bone_snapping.available and di.bone_snapping.value==1.5 and di.bone_snapping.read_type=="number"
    and di.bone_snapping.native_type=="double" and di.block_spine_breaking.value==false,
    "strict typed native snapping and spine guard retain values and availability")
T.check(di.bones[1].native_part==3 and di.bones[1].part.value==false and di.bones[1].distance==20
    and di.bones[1].sampled_predicates_match==true and di.bones[2].native_part==6
    and di.bones[2].distance==15 and di.bones[2].sampled_predicates_match==false,
    "native arm predicates use exact map keys and strict distance greater than15 without world offsets")
T.check(di.eligibility=="unavailable:sample_only" and plain(di)
    and logs[#logs]:find("dislocation_space_name=RTS_ParentBoneSpace",1,true)
    and logs[#logs]:find("sampled_predicates_match:true",1,true),
    "sample log labels the proved native space and copied evidence without asserting native event eligibility")
local function sampled()return a.capture(pawn,"dislocation edge").dislocation_inputs end
pawn["Bone Snapping"]=0.1
T.check(sampled().bones[1].sampled_predicates_match==false,"native BoneSnapping threshold is strict greater than0.1")
pawn["Bone Snapping"]=nil
local unavailable=sampled()
T.check(not unavailable.bone_snapping.available and unavailable.bone_snapping.read_type=="nil"
    and not unavailable.read_complete and unavailable.bones[1].sampled_predicates_match==nil,
    "missing BoneSnapping cannot advertise a matched predicate or substitute zero")
for _,bad in ipairs({"1.5",false,math.huge,0/0})do
    pawn["Bone Snapping"]=bad;unavailable=sampled()
    T.check(not unavailable.bone_snapping.available and unavailable.bones[1].sampled_predicates_match==nil,
        "nonnumeric/nonfinite snapping input stays unavailable")
end
pawn["Bone Snapping"]=1.5;pawn["Block Spine Breaking"]=0
T.check(not sampled().block_spine_breaking.available,"nonboolean native spine guard stays unavailable")
pawn["Block Spine Breaking"]=true
T.check(sampled().bones[1].sampled_predicates_match==false,"available native spine guard blocks the sampled conjunction")
pawn["Block Spine Breaking"]=false;pawn["Arm R Dislocated"]=true
T.check(sampled().bones[1].sampled_predicates_match==false,"already-dislocated native arm blocks the sampled conjunction")
pawn["Arm R Dislocated"]=false;pawn["Dismembered Parts Map"]=map({{3,true},{6,false}})
T.check(sampled().bones[1].sampled_predicates_match==false,"positive exact native part3 ledger blocks the sampled conjunction")
pawn["Dismembered Parts Map"]=map({{6,false}});unavailable=sampled()
T.check(unavailable.bones[1].part.present==false and not unavailable.bones[1].part.available
    and unavailable.bones[1].sampled_predicates_match==nil,"missing native arm map key never becomes a verified false entry")
pawn["Dismembered Parts Map"]=map({{3,0},{6,false}})
T.check(not sampled().bones[1].part.available,"malformed native part map cannot provide dislocation predicates")
pawn["Dismembered Parts Map"]=map({{3,false},{6,false}})
poses.upperarm_r.X=math.huge;unavailable=sampled()
T.check(not unavailable.bones[1].mesh.available and unavailable.bones[1].distance==nil
    and unavailable.bones[1].sampled_predicates_match==nil,"nonfinite native translation cannot become a distance")
poses.upperarm_r.X="20"
T.check(not sampled().bones[1].mesh.available,"numeric strings are not native transform coordinates")
poses.upperarm_r.X=20;driver.GetBoneIndex=function()return -1 end
local old_socket_count=#socket_calls;unavailable=sampled()
T.check(not unavailable.bones[1].driver.available and #socket_calls==old_socket_count+2,
    "missing driver bone index prevents fallback socket-transform reads")
driver.GetBoneIndex=mesh.GetBoneIndex
driver.GetOwner=function()return obj(99,"Willie foreign")end;old_socket_count=#socket_calls
T.check(not sampled().available and #socket_calls==old_socket_count,"wrong driver owner refuses all native position reads")
driver.GetOwner=function()return pawn end;driver.GetWorld=function()return obj(99,"World departed")end
T.check(not sampled().available and #socket_calls==old_socket_count,"wrong driver world refuses all native position reads")
driver.GetWorld=function()return world end;pawn.DriverSkeleton=nil
T.check(not sampled().available and #socket_calls==old_socket_count,"missing DriverSkeleton never substitutes Mesh")
pawn.DriverSkeleton=mesh
T.check(not sampled().available and #socket_calls==old_socket_count,"same component cannot masquerade as independent driver and simulated Mesh")
pawn.DriverSkeleton=driver
driver.GetSocketTransform=function(c,b,space)
    local result=socket(c,b,space);pawn.DriverSkeleton=geometry(obj(71,"SkeletalMeshComponent replacement driver"),"mesh",1);return result
end
unavailable=sampled()
T.check(not unavailable.available and unavailable.reason=="native identity changed" and #unavailable.bones==0,
    "driver reference changing during getter read invalidates the sample without relabeling old positions")
pawn.DriverSkeleton=driver;driver.GetSocketTransform=function(c,b,space)
    local result=socket(c,b,space);ctx.life=ctx.life+1;return result
end
local before_scope_logs=#logs
T.check(a.capture(pawn,"mid sample life transition")==nil and #logs==before_scope_logs,
    "scope change during a native arm getter drops the entire body row")
ctx.life=ctx.life-1;driver.GetSocketTransform=socket;mesh.GetBoneIndex=old_bone_index
T.check(writes==0,"dislocation diagnostic invokes only proved native getters and performs no physical writes")
