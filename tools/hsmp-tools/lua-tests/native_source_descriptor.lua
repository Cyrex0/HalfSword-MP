local D=dofile("mods/HSMPMatch/Scripts/native_source_descriptor.lua")
local Adapter=dofile("mods/HSMPMatch/Scripts/native_source_adapter.lua")
local function plain(v)if type(v)~="table"then return v end;local out={};for k,x in pairs(v)do out[k]=plain(x)end;return out end
local function fixture(path)local f=assert(io.open(path,"rb"));local s=f:read("*a");f:close();return plain(T.json_decode(s))end
local recipe=fixture("tools/hsmp-tools/lua-tests/fixtures/native_source_recipe.json")
-- The JSON fixture explicitly declares null. The harness maps JSON null to nil;
-- actual source tables must retain this required not-applicable sentinel.
recipe.components[1].spline_profile=false
local armor=fixture("tools/hsmp-tools/lua-tests/fixtures/native_armor_passport.json")
-- Native ForEach passes GetParam wrappers for the actual hard enum/bool/struct
-- inner properties. These fixtures reproduce that documented wrapper contract.
local function wrapped(v,kind)return {type=function()return kind or "RemoteUnrealParam"end,get=function()return v end}end
local Array=dofile("mods/HSMPMatch/Scripts/native_source_array.lua")
local array_env={guard=function()end,context="Actor.K2_GetComponentsByClass(MeshComponent) collect owner=FixturePawn"}
local function collect_return(value,max)return Array.collect(value,max or 2,"return",array_env,function(v)return v end)end
T.check(T.eq(collect_return({wrapped(17),wrapped(23)})[2],23),"known return-table numeric keys copy typed inner values")
local bad_keys={"outTable",0,1.5,3,setmetatable({},{__tostring=function()error("unsafe key formatter",0)end})}
for _,key in ipairs(bad_keys)do
    local ok,reason=pcall(collect_return,{[1]=wrapped(17),[key]=false})
    T.check(not ok and reason:find('getter="Actor.K2_GetComponentsByClass(MeshComponent)',1,true)
        and reason:find('key_type='..type(key),1,true) and reason:find('value_type=boolean',1,true),
        "unexpected returned key refuses with producer/key/value context: "..type(key))
end
local _,bounded_reason=pcall(collect_return,{[string.rep("x",10000)]=false})
T.check(#bounded_reason<450 and bounded_reason:find('[truncated]',1,true),"native array key evidence is bounded")
local _,hole_reason=pcall(collect_return,{[2]=wrapped(23)})
T.check(hole_reason:find('native returned array hole',1,true) and hole_reason:find('index=1',1,true),"native sparse array refusal identifies missing index")
local function map(rows)
    return setmetatable({ForEach=function(_,fn)for _,r in ipairs(rows)do fn(wrapped(r.slot),wrapped(r.value))end end},
        {__len=function()return #rows end})
end
local function raw(kind,record)
    local out={}
    for _,fd in ipairs(D.FIELDS[kind])do
        local value=record[fd[3]]
        if fd[2]=="vec"or fd[2]=="color"then
            local v={};for i,k in ipairs(fd[2]=="vec"and {"X","Y","Z"}or {"R","G","B","A"})do v[k]=value[i]end;value=v
        elseif fd[2]=="name"then local n=value;value={ToString=function()return n end}
        elseif fd[2]=="class"then local p=value;value={GetAddress=function()return p==""and 0 or 99 end,IsValid=function()return p~=""end,GetFullName=function()return "BlueprintGeneratedClass "..p end}
        elseif fd[2]=="flags"then value=map(value)
        elseif fd[2]=="equipment"then
            local e={};for key,name in pairs({armor="ArmorinSlots_5_BD7AC6CB43FBB2FDB943E7864486F358",sheaths="WeaponsinSlots_11_B42349384F5EF74DE78A7F870D89656A",hands="WeaponinHands_23_B3FE643741AF91A6DFE51888205C0F05"})do
                local rows={};for _,r in ipairs(value[key])do rows[#rows+1]={slot=r.slot,value=raw(key=="armor"and "armor"or "weapon",r.passport)}end;e[name]=map(rows)
            end;value=e
        end
        out[fd[1]]=value
    end
    return out
end
local scope=true
local native_character=raw("character",recipe.passport)
local native_armor=raw("armor",armor)
local env={guard=function()return scope end,unwrap=function(v)return v:get()end,
    class_path=function(v)return v:GetAddress()==0 and ""or v:GetFullName():match("^%S+%s+(.+)$")end,
    character=function()return native_character end,actor_class=function()return recipe.actor_class end,team=function()return recipe.team end,
    current_armor=function()return map({{slot=armor.pslot,value=native_armor}})end,
    construction=function()return recipe.construction end,
    live_weapons=function()return {weapons={},hands={},sheaths={}}end,
    render=function()return {components=recipe.components,topology=recipe.topology}end}
T.check(T.eq(#D.FIELDS.character,11),"all eleven native character fields captured")
T.check(T.eq(#D.FIELDS.armor,24),"native armor includes sparse SlotsBlocked")
T.check(T.eq(#D.FIELDS.weapon,25),"all native weapon fields captured")
local source,why=D.capture(env)
T.check(source~=nil,"complete synthetic source captures: "..tostring(why))
T.check(T.eq(source.passport.height,recipe.passport.height),"native character double never rounded")
T.check(T.eq(source.equipment.armor[1].passport.price,armor.price),"native armor price double preserved")
T.check(T.eq(source.equipment.armor[1].passport.slots_blocked[1].value,false),"explicit false blocked slot preserved")
T.check(T.eq(source.equipment.armor[1].passport.slots_blocked[2].slot,5),"sparse blocked-slot keys preserved")
T.check(T.eq(#source.passport.equipment.armor,0),"consumed construction armor does not replace live armor")
do
local construction_passport=plain(recipe.passport)
local empty_armor=plain(armor);empty_armor.class="";empty_armor.pslot=0
construction_passport.equipment.armor={{slot=0,passport=plain(armor)},{slot=7,passport=empty_armor}}
local original_character=env.character
env.character=function()return raw("character",construction_passport)end
local independent,independent_reason=D.capture(env)
T.check(independent~=nil,"native construction map permits independent key and passport slot: "..tostring(independent_reason))
T.check(T.eq(independent.passport.equipment.armor,construction_passport.equipment.armor),"all populated/empty armor occurrences and all24 copied fields remain exact")
T.check(independent.passport.equipment.armor[1].slot~=independent.passport.equipment.armor[1].passport.pslot
    and independent.passport.equipment.armor[2].slot==7 and independent.passport.equipment.armor[2].passport.pslot==0,
    "construction map keys never overwrite either native passport Slot")
T.check(T.eq(independent.equipment.armor,source.equipment.armor),"independent construction observations never replace actual live armor")
-- Use the harvest boundary instead of guessing a field-read count.
env.character=function()return raw("character",construction_passport)end
env.phase=function(stage,edge,detail)
    if stage=="harvest"and edge=="enter"and detail.pass==2 then
        construction_passport.equipment.armor[2].passport.price=armor.price+1
    end
end
T.check(D.capture(env)==nil,"mutation of an empty construction row still refuses unequal full harvests")
env.character=original_character;env.phase=nil
end
do
local doublet=plain(armor)
doublet.class="/Game/Assets/Armor/Blueprints/Modular_Armor/BP_Armor_Modular_Core_Body_Doublet_Arming.BP_Armor_Modular_Core_Body_Doublet_Arming_C"
doublet.pslot=2
local native_doublet=raw("armor",doublet)
local original_live_armor=env.current_armor
env.current_armor=function()return map({{slot=12,value=native_doublet}})end
local observed,observed_reason=D.capture(env)
T.check(observed~=nil,"actual-shaped native Doublet actor slot12/passport slot2 captures: "..tostring(observed_reason))
T.check(observed.equipment.armor[1].slot==12 and observed.equipment.armor[1].passport.pslot==2,
    "source preserves both independent raw live enum bytes")
T.check(T.eq(observed.equipment.armor[1].passport,doublet),"all24 native Doublet passport fields survive independent live key capture")
env.phase=function(stage,edge,detail)
    if stage=="harvest"and edge=="enter"and detail.pass==2 then native_doublet["Slot_30_7561CB484566A4512003EA96ED44F88D"]=3 end
end
T.check(D.capture(env)==nil,"independent live passport-slot mutation still refuses unequal harvests")
env.current_armor=original_live_armor;env.phase=nil
end
local passport_phases={}
env.phase=function(stage,edge,detail)passport_phases[#passport_phases+1]={stage=stage,edge=edge,detail=detail}end
T.check(D.capture(env)~=nil,"coarse phases preserve exact two-pass source capture")
local passport_passes={}
for _,event in ipairs(passport_phases)do if event.stage=="passport"and event.edge=="enter"then passport_passes[#passport_passes+1]=event.detail.pass end end
T.check(#passport_passes==2 and passport_passes[1]==1 and passport_passes[2]==2,"rare passport diagnostics identify both exact harvest passes")
local character_getter=env.character
env.character=function()
    local last=passport_phases[#passport_phases]
    if not last or last.stage~="passport"or last.edge~="enter"then error("passport entry missing before native getter",0)end
    error("synthetic passport getter refused",0)
end
T.check(D.capture(env)==nil and passport_phases[#passport_phases].stage=="harvest"
    and passport_phases[#passport_phases].detail.ok==false,"native passport failure retains prior phase entry and capture refusal")
env.character=character_getter;env.phase=nil
native_armor[D.FIELDS.armor[2][1]]=123
T.check(T.eq(source.equipment.armor[1].passport.id,armor.id),"captured passport is detached from native storage")
native_armor[D.FIELDS.armor[2][1]]=armor.id
local hair_name=D.FIELDS.character[11][1]
local hair=native_character[hair_name];native_character[hair_name]=nil
T.check(D.capture(env)==nil,"missing hair length refuses capture instead of zero")
native_character[hair_name]=hair
local color_name=D.FIELDS.armor[7][1];local alpha=native_armor[color_name].A;native_armor[color_name].A=nil
T.check(D.capture(env)==nil,"missing color alpha refuses capture instead of one")
native_armor[color_name].A=alpha
env.current_armor=function()return map({{slot=2,value=native_armor},{slot=2,value=native_armor}})end
T.check(D.capture(env)==nil,"duplicate native map key refuses capture")
env.current_armor=function()return map({{slot=2,value=native_armor}})end
local reads=0;env.team=function()reads=reads+1;return reads end
T.check(D.capture(env)==nil,"same-pawn native recipe mutation refuses two-pass capture")
env.team=function()return recipe.team end
scope=false
T.check(D.capture(env)==nil,"world drop refuses source reads")
scope=true
local exact_a={value=1.000000000000001};local exact_b={value=1.000000000000002}
T.check(D.signature(exact_a)~=D.signature(exact_b),"descriptor equality preserves native double differences")
do
T.check(D.MAX_RECIPE_NODES==512*1024,"production plain/signature node budget derives from encoded512KiB tokens")
local wide_components={}
for i=1,64 do
    local c=plain(recipe.components[1]);c.id=i;c.parent=i==1 and 0 or 1;c.name="Offline full dictionary component "..i
    if i>1 then c.role="attachment"end
    c.bones={}
    for j=1,512 do c.bones[j]={name=string.format("offline_full_render_dictionary_bone_%03d",j-1),parent=j==1 and -1 or j-2}end
    wide_components[i]=c
end
local original_render=env.render
env.render=function()return {components=wide_components,topology=recipe.topology}end
local wide_source,wide_reason=D.capture(env)
T.check(wide_source~=nil,"production complete capture admits64x512 bones beyond old node ceiling: "..tostring(wide_reason))
local wide_bones=0;for _,c in ipairs(wide_source.components)do wide_bones=wide_bones+#c.bones end
T.check(wide_bones==32768 and T.eq(wide_source.components,wide_components),"all32768 bone occurrences and component fields survive production plain copies")
local wide_signature=D.signature(wide_source)
wide_components[64].bones[512].name="offline_changed_final_render_bone"
T.check(D.signature({components=wide_components,topology=recipe.topology})~=D.signature({components=wide_source.components,topology=recipe.topology}),
    "wide signature retains final-component final-bone mutation")
T.check(wide_signature==D.signature(wide_source),"captured wide recipe stays detached from later native-shaped source mutation")
env.phase=function(stage,edge,detail)
    if stage=="harvest"and edge=="enter"and detail.pass==2 then wide_components[64].bones[512].parent=0 end
end
T.check(D.capture(env)==nil,"wide final-bone mutation refuses unequal two-harvest capture")
env.render=original_render;env.phase=nil
local too_deep={};local cursor=too_deep;for _=1,25 do cursor.child={};cursor=cursor.child end
T.check(not pcall(D.signature,too_deep),"expanded production budget retains depth24 refusal")
local cyclic={};cyclic.self=cyclic
T.check(not pcall(D.signature,cyclic),"expanded production budget retains bounded cycle refusal")
local too_many={};for i=1,D.MAX_RECIPE_NODES do too_many[i]=false end
T.check(not pcall(D.signature,too_many),"one table plus512KiB values refuses token-derived node overflow")
too_many=nil
end
T.check(D.signature({value=-0.0})~=D.signature({value=0.0}),"native signed zero preserved in source equality")

-- Exercise the actual source adapter with fresh native-style bindings.
local current_address=10
local pawn={GetClass=function()return {GetAddress=function()return 20 end,IsValid=function()return true end,
    GetFullName=function()return "BlueprintGeneratedClass "..recipe.actor_class end}end,
    GetActorScale3D=function()return {X=0.9375,Y=0.9375,Z=0.9375}end,
    ["Character Passport"]=native_character,["Currently Equipped Armor"]=map({{slot=2,value=native_armor}}),
    ["Team Int"]=2,["Is Zombie?"]=false,["Scale Mutation Inhibitor"]=0.5,["Spawn in Pants"]=true,
    ["Bolts in Quiver"]=0,["Blossfechten Gear"]=false,["Height Rate"]=0.5,["Muscle Rate"]=0.25,
    ["Mass Scale (Set in BP)"]=1.0625,["Character Scale (Set in BP)"]={X=1,Y=1,Z=1},
    ["Dismembered Parts Map"]=map({}),["Dismemberment In Process"]=false,
    ["Dismembered Bones"]={GetArrayNum=function()return 0 end,ForEach=function()end}}
local condition_names={"HeadHealth_2_61859BB444171EF8952E0FA5DD8628EE","NeckHealth_4_C658DC6A4BD1988C40F1A5B3C4F8F4EE",
    "ArmRHealth_9_A65DD4C14ACBF6030A2B3AAD90FD0CFD","ArmLHealth_11_32345C31454A51B3CDE618918B9574F6",
    "BodyUpperHealth_16_F71EA0C742135DC3B4F71EA3FEF07C46","BodyLowerHealth_18_37C008FF4FA0C0E5F5E09C9F0C174FE3",
    "LegRHealth_13_D50D4E174859A541DBEA66963D162E12","LegLHealth_15_41C766B5460596C0804EA5B4B8F8EB36"}
pawn["Start Body Condition"]={};for _,n in ipairs(condition_names)do pawn["Start Body Condition"][n]=100 end
local WG={check=function()return scope end,settled=function()return true end,token=function()return {}end,same=function()return scope end}
local adapter_phases={}
local adapter=Adapter.new({WG=WG,phase=function(context,stage,edge,detail)adapter_phases[#adapter_phases+1]={context=context,stage=stage,edge=edge,detail=detail}end,
    resolve=function(index)return {index=index,world_key="fixture",pc_address=9,pc_name="PC",pawn_address=current_address,pawn_name="Fixture",pawn=pawn,world={GetAddress=function()return 1 end}}end,
    capture_render=function(_,bindings)return {components=recipe.components,bindings={{id=1,address=11,owner=0}},
        topology={detached={},gore={},vertex_state="unavailable"}}end})
local phase_context={epoch=7,id=9,incarnation=11,dir_seq=13,revision=17,frame_seq=19}
local actual,bindings=adapter.capture(0,phase_context)
T.check(actual~=nil,"actual source adapter entry captures typed native-style fields")
T.check(T.eq(actual.team,2),"actual source Team Int copied without assumed team")
T.check(T.eq(actual.topology.vertex_state,"unavailable"),"unverified vertex state remains incomplete")
T.check(T.eq(bindings.pawn,10),"engine addresses remain in separate local binding table")
T.check(actual.pawn==nil,"engine address never enters source recipe")
T.check(adapter_phases[1].context==phase_context and adapter_phases[1].stage=="adapter_capture"and adapter_phases[1].edge=="enter"
    and adapter_phases[#adapter_phases].stage=="adapter_capture"and adapter_phases[#adapter_phases].detail.ok==true,
    "actual adapter phase callback preserves original immutable entity/revision context")
local prior=pawn.GetActorScale3D
pawn.GetActorScale3D=function()current_address=12;return {X=1,Y=1,Z=1}end
T.check(adapter.capture(0)==nil,"same-world source incarnation replacement during getter refuses capture")
pawn.GetActorScale3D=prior;current_address=10
scope=false
T.check(adapter.capture(0)==nil,"actual adapter performs no source capture in dropped world")
scope=true
local broken=Adapter.new({WG=WG,resolve=function(index)return {index=index,world_key="fixture",pc_address=9,pc_name="PC",pawn_address=10,pawn_name="Fixture",pawn=pawn,
    world={GetAddress=function()scope=false;return 1 end}}end})
local guarded,reason=broken.capture(0)
T.check(guarded==nil and type(reason)=="string","world loss during initial source binding returns explicit refusal")

local Vertex=dofile("mods/HSMPMatch/Scripts/native_source_vertex.lua")
local function color_array(values)
    return {GetArrayNum=function()return #values end,ForEach=function(_,fn)for i,v in ipairs(values)do fn(i,wrapped(v))end end}
end
scope=true
local colors={{R=255,G=0,B=128,A=0},{R=255,G=0,B=128,A=0},{R=0,G=255,B=0,A=255}}
local vertex_env={guard=function()return scope end,lods=function()return 1 end,count=function()return #colors end,
    colors=function()return color_array(colors)end,array_kind="property"}
local lods=Vertex.capture(vertex_env)
T.check(lods~=nil and #lods[1].runs==2,"native color RLE preserves complete source ordering")
T.check(T.eq(lods[1].runs[1].count,2),"identical native colors compact exactly")
T.check(T.eq(lods[1].runs[1].color[3],128),"native RGBA8 channels never gamma-converted")
T.check(T.eq(lods[1].runs[1].color[4],0),"native cut alpha zero preserved")
local function color_return(values)local out={};for i,v in ipairs(values)do out[i]=wrapped(v,"LocalUnrealParam")end;return out end
vertex_env.array_kind="return";vertex_env.colors=function()return color_return(colors)end
T.check(Vertex.capture(vertex_env)~=nil,"native function return colors use plain table of typed local struct parameters")
vertex_env.colors=function()return colors end
T.check(Vertex.capture(vertex_env)~=nil,"copied direct color tables remain exact without parameter get")
vertex_env.colors=function()return {[1]=wrapped(colors[1],"LocalUnrealParam"),[3]=wrapped(colors[3],"LocalUnrealParam")}end
T.check(Vertex.capture(vertex_env)==nil,"sparse native return array refuses rather than samples existing entries")
vertex_env.array_kind="property";vertex_env.colors=function()return color_array(colors)end
colors[1].A=nil;T.check(Vertex.capture(vertex_env)==nil,"missing native alpha refuses color capture")
colors[1].A=0;vertex_env.count=function()return 4 end
T.check(Vertex.capture(vertex_env)==nil,"native vertex count mismatch refuses capture")
vertex_env.count=function()return #colors end
vertex_env.colors=function()scope=false;return color_array(colors)end
T.check(Vertex.capture(vertex_env)==nil,"world loss during color getter refuses capture")
scope=true
local requests=0;vertex_env.colors=function()requests=requests+1;return color_array(colors)end
vertex_env.reserve=function()error("native vertex expansion bound",0)end
T.check(Vertex.capture(vertex_env)==nil and requests==0,"vertex expansion bound refuses before native color array allocation")

-- Exercise the default rare render collector, not an injected recipe. These
-- are typed offline wrappers, never evidence that a real client rendered them.
local Render=dofile("mods/HSMPMatch/Scripts/native_source_render.lua")
local fname_mt={__eq=function(a,b)return a.value==b.value and a.number==b.number end}
local function fname(value,number)return setmetatable({value=value,number=number or 0,ToString=function()return value end},fname_mt)end
local next_array=1000
local function array(values)
    next_array=next_array+1;local a={header_address=next_array,data_address=next_array*64}
    a.GetArrayAddress=function()return a.header_address end;a.GetArrayDataAddress=function()return a.data_address end
    a.GetArrayNum=function()return #values end;a.GetArrayMax=function()return #values end
    a.ForEach=function(_,fn)for i,v in ipairs(values)do fn(i,wrapped(v))end end
    return a
end
local render_world={GetAddress=function()return 1 end,IsValid=function()return true end}
local runtime_objects={}
local function object(address,n,path,transient)
    local value={GetAddress=function()return address end,IsValid=function()return true end,GetFName=function()return fname(n)end,
        GetFullName=function()return "FixtureClass "..path end,HasAnyFlags=function(_,mask)return mask&0x40~=0 and transient or false end,
        GetWorld=function()return render_world end,GetClass=function()return {GetAddress=function()return 400 end,GetFullName=function()return "Class /Script/Engine.SceneComponent"end}end,
        IsA=function()return false end,ComponentHasTag=function()return false end,
        type=function()return "UObject"end,get=function()error("direct UObject is not a parameter",0)end}
    runtime_objects[path]=value
    return value
end
local saved_find,saved_fname=StaticFindObject,FName
FName=function(n)return n end
scope=true
local root=object(100,"CapsuleRoot","/Game/Test/Root.Root",false)
local host=object(10,"Pawn","/Game/Test/Pawn.Pawn",false);host.RootComponent=root
local function scene(c,actor,cls)
    c.GetOwner=function()return actor end;c.GetClass=function()return {GetFullName=function()return "Class "..cls end}end
    c.IsA=function(_,class)return class=="/Script/Engine.SplineComponent"and cls=="/Script/Engine.SplineComponent"end
    c.GetAttachParent=function()return nil end;c.GetAttachSocketName=function()return fname("None")end
    c.IsVisible=function()return true end;c.bHiddenInGame=false
    c.GetRelativeTransform=function()return {Translation={X=0.125,Y=-2.5,Z=3},Rotation={X=0,Y=0,Z=0,W=1},Scale3D={X=0.5,Y=1,Z=2}}end
    c.K2_GetComponentToWorld=function()return {Translation={X=10.125,Y=20,Z=-30},Rotation={X=0,Y=0,Z=0,W=1},Scale3D={X=0.5,Y=1,Z=2}}end
    c.GetCollisionResponseToChannel=function(_,channel)return channel%3 end;c.GetCollisionEnabled=function()return 3 end
    c.GetCollisionObjectType=function()return 3 end;c.GetCollisionProfileName=function()return fname("ExactAnchorProfile")end;c.IsSimulatingPhysics=function()return false end
    return c
end
scene(root,host,"/Script/Engine.CapsuleComponent");root.bHiddenInGame=true
local live_weapon=object(200,"LiveWeapon","/Game/Test/WeaponActor.WeaponActor",false)
local body_asset=object(300,"Body","/Game/Test/Body.Body",false)
body_asset.GetClass=function()return {GetFullName=function()return "Class /Script/Engine.SkeletalMesh"end}end
body_asset.Skeleton=object(301,"Skeleton","/Game/Test/Skeleton.Skeleton",false)
body_asset.GetOverlayMaterial=function()return nil end;body_asset.GetDefaultMeshDeformer=function()return nil end
body_asset.MeshClothingAssets=array({});body_asset.GetMorphTargetsPtrConv=function()return {wrapped(object(302,"ExactMorph","/Game/Test/Morph.Morph",false))}end
local cooked_mat=object(310,"Material","/Game/Test/Material.Material",false)
-- Actual level-owned MID shape: RF_Transient is clear independently of class.
local dynamic_mat=object(311,"MID","/Game/Maps/Arenas/Map_Arena_Yard.Map_Arena_Yard:PersistentLevel.Willie.CharacterMesh0.MID_Actual",false)
dynamic_mat.IsA=function(_,c)return c=="/Script/Engine.MaterialInstance"or c=="/Script/Engine.MaterialInstanceDynamic"end
dynamic_mat.GetClass=function()return {GetAddress=function()return 401 end,GetFullName=function()return "Class /Script/Engine.MaterialInstanceDynamic"end}end
dynamic_mat.Parent=cooked_mat
dynamic_mat.BasePropertyOverrides=setmetatable({},{__index=function()return false end})
for _,field in ipairs({"DoubleVectorParameterValues","FontParameterValues","RuntimeVirtualTextureParameterValues","SparseVolumeTextureParameterValues"})do dynamic_mat[field]=array({})end
local function parameter(n,v)return {ParameterInfo={Name=fname(n),Association=0,Index=-1},ParameterValue=v}end
dynamic_mat.ScalarParameterValues=array({parameter("ExactBlood",0.3125)})
dynamic_mat.VectorParameterValues=array({parameter("ExactTint",{R=0.2,G=0.4,B=0.6,A=0.0})})
dynamic_mat.TextureParameterValues=array({parameter("ExactTexture",object(312,"Texture","/Game/Test/Texture.Texture",false))})
local bone_names={"root"};for i=2,40 do bone_names[i]="native_render_bone_"..i end
local bone_index={};for i,n in ipairs(bone_names)do bone_index[n]=i end
local physical_asset=object(320,"Physics","/Game/Test/Physics.Physics",false)
body_asset.GetPhysicsAsset=function()return physical_asset end
local function mesh(address,n,actor,kind,asset)
    local c=object(address,n,"/Game/Test/Runtime."..n,false)
    c.GetOwner=function()return actor end;c.IsA=function(_,k)return k=="/Script/Engine."..(kind=="skeletal"and "SkeletalMeshComponent"or "StaticMeshComponent")end
    c.GetClass=function()return {GetFullName=function()return "Class /Script/Engine."..(kind=="skeletal"and "SkeletalMeshComponent"or "StaticMeshComponent")end}end
    c.GetRelativeTransform=function()return {Translation={X=0.125,Y=0.25,Z=0.5},Rotation={X=0,Y=0,Z=0,W=1},Scale3D={X=1,Y=1,Z=1}}end
    c.IsVisible=function()return true end;c.bHiddenInGame=false;c.GetAttachSocketName=function()return fname("None")end
    c.GetCollisionResponseToChannel=function(_,channel)return channel%3 end;c.GetCollisionEnabled=function()return 3 end
    c.GetCollisionObjectType=function()return 3 end;c.GetCollisionProfileName=function()return fname("ExactNativeProfile")end;c.IsSimulatingPhysics=function()return true end
    c.GetSkeletalMeshAsset=function()return asset end;c.StaticMesh=asset
    c.GetPhysicsAsset=function()error("GetPhysicsAsset is not reflected on a component",0)end
    c.bDisableClothSimulation=false;c.GetNumBones=function()return #bone_names end;c.GetBoneName=function(_,i)return fname(bone_names[i+1])end
    c.GetOverlayMaterial=function()return nil end;c.GetMeshDeformerInstance=function()return nil end;c.IsUsingSkinWeightProfile=function()return false end
    c.bHideSkin=false;c.bDisableMorphTarget=false;c.bForceWireframe=false;c.GetVertexOffsetUsage=function()return 0 end;c.IsMaterialSectionShown=function()return true end
    c.GetParentBone=function(_,n)return fname(bone_index[n]==1 and "None"or bone_names[bone_index[n]-1])end
    c.IsBoneHiddenByName=function(_,n)return n==bone_names[40]end;c.GetMorphTarget=function()return 0.6875 end
    c.GetNumMaterials=function()return 1 end;c.GetMaterial=function()return dynamic_mat end
    if kind=="skeletal"then c.GetNumLODs=function()return 1 end
    else c.GetNumLODs=function()error("GetNumLODs is not reflected on StaticMeshComponent",0)end end
    c.ComponentHasTag=function()return false end;c.native_colors={{R=255,G=128,B=0,A=255},{R=255,G=128,B=0,A=255}}
    return c
end
local body=mesh(11,"BodyMesh",host,"skeletal",body_asset);host.Mesh=body;body.GetAttachParent=function()return root end
local weapon_asset=object(330,"WeaponMesh","/Game/Test/WeaponMesh.WeaponMesh",false);weapon_asset.GetNumLODs=function()return 1 end
weapon_asset.GetClass=function()return {GetFullName=function()return "Class /Script/Engine.StaticMesh"end}end
weapon_asset.bAllowCPUAccess=true
local weapon_mesh=mesh(201,"WeaponMesh",live_weapon,"static",weapon_asset);weapon_mesh.GetAttachParent=function()return body end
weapon_mesh.native_vertex_proof={state="captured_required",lod_info_count=1,no_override=false,asset_present=true,material_count=1,material_null_mask=0,component_kind="static"}
local mesh_is_a=body.IsA;body.IsA=function(self,k)return k=="/Script/Engine.MeshComponent"or mesh_is_a(self,k)end
local weapon_is_a=weapon_mesh.IsA;weapon_mesh.IsA=function(self,k)return k=="/Script/Engine.MeshComponent"or weapon_is_a(self,k)end
live_weapon.RootComponent=weapon_mesh
local source_scenes={body,root}
local scene_census_calls,mesh_return_calls,spline_return_calls=0,0,0
local function source_mesh_return(_,class)
    if class=="/Script/Engine.MeshComponent"then mesh_return_calls=mesh_return_calls+1
    elseif class=="/Script/Engine.SplineComponent"then spline_return_calls=spline_return_calls+1
    else scene_census_calls=scene_census_calls+1;error("full helper Scene enumeration is forbidden",0)end
    local out={};for _,c in ipairs(source_scenes)do if c:IsA(class)then out[#out+1]=wrapped(c)end end;return out
end
host.K2_GetComponentsByClass=source_mesh_return;live_weapon.K2_GetComponentsByClass=function(_,class)
    if class=="/Script/Engine.SplineComponent"then spline_return_calls=spline_return_calls+1;return {}end
    mesh_return_calls=mesh_return_calls+1
    if class~="/Script/Engine.MeshComponent"then scene_census_calls=scene_census_calls+1;error("full weapon Scene enumeration is forbidden",0)end
    return {wrapped(weapon_mesh)}end
local plain_component_getter_calls=0
local function unavailable_component_alias()plain_component_getter_calls=plain_component_getter_calls+1;error("unreflected GetComponentsByClass alias",0)end
host.GetComponentsByClass=unavailable_component_alias;live_weapon.GetComponentsByClass=unavailable_component_alias
local rvp={IsValid=function()return true end,GetMeshComponentAmountOfVerticesOnLOD=function(_,c)
    -- Matched shipping StaticMesh branch returns0 at the CPU-access gate,
    -- before inspecting either component override or asset vertex buffers.
    if c==weapon_mesh and c.StaticMesh.bAllowCPUAccess==false then return 0 end
    return #c.native_colors
end,
    GetMeshComponentVertexColorsAtLOD_Wrapper=function(_,c)return color_return(c.native_colors)end}
StaticFindObject=function(p)return runtime_objects[p]or(p=="/Script/VertexPaintDetectionPlugin.Default__VertexPaintFunctionLibrary"and rvp or p)end
local render_env={read=function(fn)if not scope then error("scope",0)end;local value=fn({pawn=host,world=render_world});if not scope then error("scope",0)end;return value end,
    guard=function()if not scope then error("scope",0)end end,token_valid=function()return scope end,weapon=function()return live_weapon end}
local native_bindings={weapons={{id=1,address=200,name="LiveWeapon",field="Weapon R"}}}
-- Offline original-identity protocol mock. Native slot/flag/class semantics are
-- tested independently in Rust; this validates the production Lua call shape.
local scope_rows={}
local function snapshot(o)
    if not scope or o.GetWorld():GetAddress()~=1 or o:HasAnyFlags(0x40000000)then error("original native world/garbage changed",0)end
    return {address=o.GetAddress(),name=o.GetFName():ToString(),class=o.GetClass():GetFullName(),
        path=o.GetFullName():match("^%S+%s+(.+)$"),owner=o.GetOwner():GetAddress(),
        root=o.GetOwner().RootComponent:GetAddress(),root_name=o.GetOwner().RootComponent:GetFName():ToString(),
        root_path=o.GetOwner().RootComponent:GetFullName(),parent=o.GetAttachParent()and o.GetAttachParent():GetAddress()or 0}
end
render_env.scope={keep=function(row)
    local o=runtime_objects[row.path];if not o or o:GetAddress()~=row.address then return nil,"original runtime path changed"end
    local s=snapshot(o)
    if (s.owner~=10 and s.owner~=200)or(row.owner~=0 and row.owner~=s.owner)then return nil,"original native owner changed"end
    for i,old in ipairs(scope_rows)do if old.address==s.address then
        if not T.eq(old,s)then return nil,"original native identity/link changed"end
        return i,s.owner
    end end
    scope_rows[#scope_rows+1]=s;return #scope_rows,s.owner
end,resolve=function(handle)
    local s=scope_rows[handle];local o=s and runtime_objects[s.path]
    if not s or not o or not T.eq(snapshot(o),s)then return nil,"original native identity/link changed"end
    return s.address
end,profile=function(handle)
    local address,reason=render_env.scope.resolve(handle)
    if not address then return nil,reason end
    local s=scope_rows[handle];local o=runtime_objects[s.path]
    if o:GetClass():GetFullName()~="Class /Script/Engine.SplineComponent"then return nil,"native spline class changed"end
    return plain(o.native_spline_profile)
end,vertex_state=function(handle)
    local address,reason=render_env.scope.resolve(handle)
    if not address then return nil,reason end
    local o=runtime_objects[scope_rows[handle].path]
    local class=o:GetClass():GetFullName(); if class~="Class /Script/Engine.StaticMeshComponent"and class~="Class /Script/Engine.SkeletalMeshComponent"then return nil,"native mesh class changed"end
    return plain(o.native_vertex_proof),"fixture native override proof unavailable"
end}
local render_phases={}
render_env.phase=function(stage,edge,detail)
    if stage=="render_capture"and edge=="enter"then scope_rows={}end
    render_phases[#render_phases+1]={stage=stage,edge=edge,detail=detail}
end
mesh_return_calls=0;spline_return_calls=0
local rendered=Render.capture(render_env,native_bindings)
local render_stats=render_phases[#render_phases].detail
T.check(render_phases[1].stage=="render_capture"and render_phases[1].edge=="enter"
    and render_phases[#render_phases].stage=="render_capture"and render_stats.ok==true,"rare render scope has entry before native class lookup and exit after complete binding harvest")
T.check(render_stats.mesh_census_calls==mesh_return_calls and render_stats.mesh_census_calls==4
    and render_stats.component_reads>120 and render_stats.qualifications>render_stats.component_reads,
    "two owners use exactly begin/end full mesh censuses regardless of full bone getter count")
T.check(spline_return_calls==4,"two owners also use exactly begin/end complete native spline censuses")
local phase_scalars=true
for _,event in ipairs(render_phases)do for _,value in pairs(event.detail)do
    local t=type(value);if t~="number"and t~="boolean"and t~="string"then phase_scalars=false end
end end
T.check(phase_scalars and #render_phases<80,"capture phase details remain bounded copied scalars with no UObject or perbone spam")
T.check(T.eq(plain_component_getter_calls,0),"source census uses exact reflected K2 component getter")
T.check(#rendered.components==3 and #rendered.components[1].bones==40,"source collector retains full render dictionary and actual owner root")
T.check(T.eq(rendered.components[1].materials[1].scalars[1].value,0.3125),"native scalar material override captured exactly")
T.check(T.eq(rendered.components[1].materials[1].vectors[1].value[4],0.0),"native vector alpha zero captured exactly")
T.check(T.eq(rendered.components[1].materials[1].textures[1].value,"/Game/Test/Texture.Texture"),"native texture exact full identity preserved")
T.check(rendered.components[1].materials[1].base=="/Game/Test/Material.Material",
    "level-owned RF-nontransient native MID resolves actual cooked parent rather than admitting its runtime colon path")
local typed_mid_phase=false
for _,event in ipairs(render_phases)do if event.stage=="material_layer"and event.edge=="enter"
    and event.detail.reason:find("dynamic=true rf_transient=false",1,true)
    and event.detail.reason:find("/Script/Engine.MaterialInstanceDynamic",1,true)then typed_mid_phase=true end end
T.check(typed_mid_phase,"rare material phase distinguishes actual native class from RF_Transient and string path predicates")
;(function()
    local parent=object(313,"ParentMID","/Engine/Transient.ParentMID",true)
    parent.IsA=dynamic_mat.IsA;parent.GetClass=dynamic_mat.GetClass;parent.Parent=cooked_mat
    parent.BasePropertyOverrides=dynamic_mat.BasePropertyOverrides
    for _,field in ipairs({"DoubleVectorParameterValues","FontParameterValues","RuntimeVirtualTextureParameterValues","SparseVolumeTextureParameterValues"})do parent[field]=array({})end
    parent.ScalarParameterValues=array({parameter("ExactBlood",0.9375),parameter("ExactInherited",-0.0)})
    parent.VectorParameterValues=array({parameter("ExactTint",{R=1,G=1,B=1,A=1}),parameter("InheritedTint",{R=0.1,G=0.2,B=0.3,A=0})})
    parent.TextureParameterValues=array({parameter("InheritedTexture",nil)})
    local original_parent=dynamic_mat.Parent;dynamic_mat.Parent=parent
    local inherited=Render.capture(render_env,native_bindings).components[1].materials[1]
    local values={};for _,p in ipairs(inherited.scalars)do values[p.info.name]=p.value end
    T.check(#inherited.scalars==2 and values.ExactBlood==0.3125 and values.ExactInherited==0,
        "all native MID layers retain child-first overrides and inherited parameters without dropping occurrences")
    T.check(D.signature({value=values.ExactInherited})==D.signature({value=-0.0}),"inherited native scalar signed zero is preserved")
    T.check(#inherited.vectors==2 and #inherited.textures==2 and inherited.base=="/Game/Test/Material.Material",
        "dynamic parent chain preserves every vector/texture dictionary including explicit null texture")
    dynamic_mat.Parent=dynamic_mat
    T.check(not pcall(Render.capture,render_env,native_bindings),"native MID parent cycle refuses without flattening")
    dynamic_mat.Parent=nil
    T.check(not pcall(Render.capture,render_env,native_bindings),"native MID without an actual cooked parent refuses")
    dynamic_mat.Parent=original_parent
    local dynamic_is_a=dynamic_mat.IsA
    dynamic_mat.IsA=function(_,c)return c=="/Script/Engine.MaterialInstance"end
    T.check(not pcall(Render.capture,render_env,native_bindings),"unproved runtime non-MID base remains refused rather than accepting a colon path")
    dynamic_mat.IsA=dynamic_is_a
    local count=0;local scalar_array=dynamic_mat.ScalarParameterValues
    local scalar_count=scalar_array.GetArrayNum
    scalar_array.GetArrayNum=function(...)count=count+1;return scalar_count(...)end
    local original_class,original_get_material=dynamic_mat.GetClass,body.GetMaterial
    dynamic_mat.GetClass=function()return {GetAddress=function()return 401 end,GetFullName=function()
        body.GetMaterial=function()return parent end
        return "Class /Script/Engine.MaterialInstanceDynamic"
    end}end
    T.check(not pcall(Render.capture,render_env,native_bindings)and count==0,
        "class-name callback replacing original material slot refuses before reading old override arrays")
    dynamic_mat.GetClass=original_class;body.GetMaterial=original_get_material;scalar_array.GetArrayNum=scalar_count
    local original_each,original_name=scalar_array.ForEach,dynamic_mat.GetFName
    scalar_array.ForEach=function(self,fn)
        dynamic_mat.GetFName=function()return fname("MID",1)end
        return original_each(self,fn)
    end
    T.check(not pcall(Render.capture,render_env,native_bindings),"same-address material FName Number replacement refuses full native identity")
    scalar_array.ForEach=original_each;dynamic_mat.GetFName=original_name
    local texture=dynamic_mat.TextureParameterValues
    local original_texture=object(314,"TextureMutation","/Game/Test/TextureMutation.TextureMutation",false)
    original_texture.GetFullName=function()dynamic_mat.Parent=parent;return "Texture2D /Game/Test/TextureMutation.TextureMutation"end
    dynamic_mat.TextureParameterValues=array({parameter("MutationTexture",original_texture)})
    T.check(not pcall(Render.capture,render_env,native_bindings),"texture path callback replacing original parent refuses before subsequent parameter use")
    dynamic_mat.TextureParameterValues=texture;dynamic_mat.Parent=original_parent
    local original_scalars=dynamic_mat.ScalarParameterValues
    for _,change in ipairs({"data","address","num","max"})do
        local old_reads,poisoned=0,false
        local poisoned_array
        local callback_name={ToString=function()
            poisoned=true
            if change=="data"then poisoned_array.data_address=poisoned_array.data_address+64
            elseif change=="address"then poisoned_array.header_address=poisoned_array.header_address+1
            elseif change=="num"then poisoned_array.GetArrayNum=function()return 0 end
            else poisoned_array.GetArrayMax=function()return 2 end end
            return "ReallocatedParameter"
        end}
        local info=setmetatable({},{__index=function(_,key)
            if poisoned then old_reads=old_reads+1;error("old parameter-info storage was dereferenced",0)end
            return ({Name=callback_name,Association=2,Index=-1})[key]
        end})
        local row=setmetatable({},{__index=function(_,key)
            if poisoned then old_reads=old_reads+1;error("old parameter-row storage was dereferenced",0)end
            return ({ParameterInfo=info,ParameterValue=0.5})[key]
        end})
        poisoned_array=array({row});dynamic_mat.ScalarParameterValues=poisoned_array
        local ok,reason=pcall(Render.capture,render_env,native_bindings)
        T.check(not ok and tostring(reason):find("native material parameter array changed",1,true)and old_reads==0,
            "same-material "..change.." header replacement refuses before stale row/info reads after FName callback")
    end
    dynamic_mat.ScalarParameterValues=original_scalars
    local previous=dynamic_mat.Parent
    local chain=cooked_mat
    for i=1,17 do
        local link=object(4000+i,"BoundMID"..i,"/Game/Test/Level.Level:BoundMID"..i,false)
        link.IsA=dynamic_mat.IsA;link.GetClass=dynamic_mat.GetClass;link.BasePropertyOverrides=dynamic_mat.BasePropertyOverrides;link.Parent=chain
        for _,field in ipairs({"ScalarParameterValues","VectorParameterValues","TextureParameterValues","DoubleVectorParameterValues","FontParameterValues","RuntimeVirtualTextureParameterValues","SparseVolumeTextureParameterValues"})do link[field]=array({})end
        chain=link
    end
    dynamic_mat.Parent=chain
    T.check(not pcall(Render.capture,render_env,native_bindings),"native material chain retains strict16-layer bound without dropping parent data")
    dynamic_mat.Parent=previous
end)()
T.check(T.eq(rendered.components[1].hidden_bones[1],bone_names[40]),"native hidden render bone retained")
T.check(T.eq(rendered.components[3].parent,1),"native weapon attachment binds captured source body")
T.check(T.eq(rendered.bindings[3].owner,200),"ephemeral binding uses actual owner address rather than logical wire id")
T.check(T.eq(rendered.components[1].parent,2),"native body preserves actual capsule root rather than actor-root shortcut")
T.check(T.eq(rendered.components[2].scene.type,"hidden_capsule"),"root eligibility follows actual native hidden flag")
T.check(T.eq(rendered.topology.vertex_state,"captured"),"source vertex readiness follows actual complete getter data")
local actual_vertex_proof=render_env.scope.vertex_state
local native_present_proof=plain(weapon_mesh.native_vertex_proof)
local no_override_proof={state="native_asset",lod_info_count=0,no_override=true,asset_present=true,material_count=1,material_null_mask=0,component_kind="static"}
weapon_mesh.native_vertex_proof=plain(no_override_proof);weapon_asset.bAllowCPUAccess=false
local saved_static_count,saved_static_colors=rvp.GetMeshComponentAmountOfVerticesOnLOD,rvp.GetMeshComponentVertexColorsAtLOD_Wrapper
local static_paint_calls=0
rvp.GetMeshComponentAmountOfVerticesOnLOD=function(self,c,lod)
    if c==weapon_mesh then static_paint_calls=static_paint_calls+1;error("CPU-gated native getter must not establish absence",0)end
    return saved_static_count(self,c,lod)
end
rvp.GetMeshComponentVertexColorsAtLOD_Wrapper=function(self,c,lod)
    if c==weapon_mesh then static_paint_calls=static_paint_calls+1;error("no synthesized static override colors",0)end
    return saved_static_colors(self,c,lod)
end
local native_asset_capture=Render.capture(render_env,native_bindings)
local native_asset_component=native_asset_capture.components[3]
T.check(native_asset_component.vertex_state=="native_asset"and #native_asset_component.vertex_colors==0 and static_paint_calls==0
    and native_asset_component.asset=="/Game/Test/WeaponMesh.WeaponMesh"and #native_asset_component.materials==1,
    "guarded all-null static override proof retains actual cooked geometry/materials without RVP or fabricated colors")
rvp.GetMeshComponentAmountOfVerticesOnLOD=saved_static_count;rvp.GetMeshComponentVertexColorsAtLOD_Wrapper=saved_static_colors
weapon_mesh.native_vertex_proof=plain(native_present_proof);weapon_asset.bAllowCPUAccess=true
local present_capture=Render.capture(render_env,native_bindings)
T.check(present_capture.components[3].vertex_state=="captured"and #present_capture.components[3].vertex_colors==1,
    "present native override proof requires the existing exact full RVP color capture")
for _,bad in ipairs({
    {state="native_asset",lod_info_count=0,asset_present=true},
    {state="native_asset",lod_info_count=0,no_override=false,asset_present=true},
    {state="captured_required",lod_info_count=1,no_override=true,asset_present=true},
    {state="captured_required",lod_info_count=0,no_override=false,asset_present=true},
    {state="unknown",lod_info_count=1,no_override=true,asset_present=true},
    {state="native_asset",no_override=true,asset_present=true},
    {state="native_asset",lod_info_count=-1,no_override=true,asset_present=true},
    {state="native_asset",lod_info_count=17,no_override=true,asset_present=true},
    {state="native_asset",lod_info_count=1.5,no_override=true,asset_present=true},
    {state="native_asset",lod_info_count=1,no_override="true",asset_present=true},
    {state="native_asset",lod_info_count=1,no_override=true,asset_present=true,guessed=true},
    {state="native_asset",lod_info_count=0,no_override=true},
    {state="native_asset",lod_info_count=0,no_override=true,asset_present="true"},
    {state="native_asset",lod_info_count=0,no_override=true,asset_present=false},
    {state="native_empty",lod_info_count=0,no_override=true,asset_present=true,material_count=1,material_null_mask=0,component_kind="static"},
    {state="native_empty",lod_info_count=0,no_override=true,asset_present=false},
})do
    bad.material_count=1;bad.material_null_mask=0;bad.component_kind="static"
    weapon_mesh.native_vertex_proof=bad
    T.check(not pcall(Render.capture,render_env,native_bindings),"malformed native static override proof refuses")
end
weapon_mesh.native_vertex_proof=nil
local proof_ok,proof_reason=pcall(Render.capture,render_env,native_bindings)
T.check(not proof_ok and proof_reason:find("fixture native override proof unavailable",1,true),"unknown native override proof never falls back to asset colors")
render_env.scope.vertex_state=nil
proof_ok,proof_reason=pcall(Render.capture,render_env,native_bindings)
T.check(not proof_ok and proof_reason:find("native mesh vertex proof capability unavailable",1,true),"absent sixth native capability refuses static source capture")
render_env.scope.vertex_state=actual_vertex_proof;weapon_mesh.native_vertex_proof=plain(no_override_proof)
local static_flags=weapon_asset.HasAnyFlags;weapon_asset.HasAnyFlags=function(_,flag)return flag==0x40 end
local proof_calls=0
render_env.scope.vertex_state=function(handle)proof_calls=proof_calls+1;return actual_vertex_proof(handle)end
proof_ok,proof_reason=pcall(Render.capture,render_env,native_bindings)
T.check(not proof_ok and proof_calls==0 and proof_reason:find("native static runtime geometry unsupported",1,true),
    "runtime transient static geometry never acquires a cooked native-asset proof")
weapon_asset.HasAnyFlags=static_flags
render_env.scope.vertex_state=function(handle)local result=actual_vertex_proof(handle);weapon_mesh.StaticMesh=body_asset;return result end
T.check(not pcall(Render.capture,render_env,native_bindings),"static asset mutation during native override proof refuses source recipe")
weapon_mesh.StaticMesh=weapon_asset
render_env.scope.vertex_state=function(handle)local result=actual_vertex_proof(handle);weapon_mesh.GetAttachParent=function()return root end;return result end
T.check(not pcall(Render.capture,render_env,native_bindings),"original attachment mutation during native override proof refuses source recipe")
weapon_mesh.GetAttachParent=function()return body end
render_env.scope.vertex_state=function(handle)local result=actual_vertex_proof(handle);scope=false;return result end
T.check(not pcall(Render.capture,render_env,native_bindings),"world loss during native override proof refuses source recipe")
scope=true;render_env.scope.vertex_state=actual_vertex_proof;weapon_mesh.native_vertex_proof=plain(native_present_proof)
;(function()
    local empty_proof={state="native_empty",lod_info_count=0,no_override=true,asset_present=false,material_count=1,material_null_mask=0,component_kind="static"}
    local paint_calls=0
    local count_getter,colors_getter=rvp.GetMeshComponentAmountOfVerticesOnLOD,rvp.GetMeshComponentVertexColorsAtLOD_Wrapper
    rvp.GetMeshComponentAmountOfVerticesOnLOD=function(self,c,lod)
        if c==weapon_mesh then paint_calls=paint_calls+1;error("hard-null mesh has no invented vertex count",0)end
        return count_getter(self,c,lod)
    end
    rvp.GetMeshComponentVertexColorsAtLOD_Wrapper=function(self,c,lod)
        if c==weapon_mesh then paint_calls=paint_calls+1;error("hard-null mesh has no synthesized colors",0)end
        return colors_getter(self,c,lod)
    end
    weapon_mesh.StaticMesh=nil;weapon_mesh.native_vertex_proof=plain(empty_proof)
    local capture=Render.capture(render_env,native_bindings)
    local copied=capture.components[3]
    T.check(copied.kind=="static"and copied.component_class=="/Script/Engine.StaticMeshComponent"
        and copied.geometry=="native_empty"and copied.asset==""and copied.vertex_state=="not_applicable",
        "native hard-null proof preserves the actual static component with explicit empty geometry")
    T.check(#capture.components==3 and copied.parent==capture.components[1].id and copied.owner==1
        and T.eq(copied.relative.translation,{0.125,0.25,0.5})and copied.collision.enabled==3
        and copied.collision.profile=="ExactNativeProfile"and copied.collision.simulating==true,
        "empty native mesh keeps its complete native parent transform ownership and collision observations")
    T.check(#copied.materials==1 and copied.materials[1].base=="/Game/Test/Material.Material"
        and copied.materials[1].scalars[1].value==0.3125 and copied.materials[1].vectors[1].value[4]==0,
        "empty native static retains actual material slots and overrides rather than assuming zero")
    T.check(paint_calls==0 and #copied.vertex_colors==0 and #copied.bones==0,
        "native empty static never queries asset LODs or fabricates bone and paint data")
    weapon_mesh.StaticMesh={GetAddress=function()return 0 end,IsValid=function()return false end}
    T.check(pcall(Render.capture,render_env,native_bindings),"address-zero wrapper still requires explicit native hard-null proof")
    weapon_mesh.StaticMesh=nil;weapon_mesh.native_vertex_proof=nil
    T.check(not pcall(Render.capture,render_env,native_bindings),"nil asset wrapper never establishes empty native geometry without proof")
    for _,bad in ipairs({
        {state="native_empty",lod_info_count=0,no_override=true},
        {state="native_empty",lod_info_count=0,no_override=true,asset_present=true,material_count=1,material_null_mask=0,component_kind="static"},
        {state="native_empty",lod_info_count=1,no_override=false,asset_present=false},
        {state="native_asset",lod_info_count=0,no_override=true,asset_present=true,material_count=1,material_null_mask=0,component_kind="static"},
        {state="captured_required",lod_info_count=1,no_override=false,asset_present=true,material_count=1,material_null_mask=0,component_kind="static"},
    })do
        bad.material_count=1;bad.material_null_mask=0;bad.component_kind="static"
        weapon_mesh.native_vertex_proof=bad
        T.check(not pcall(Render.capture,render_env,native_bindings),"unavailable wrapper cannot turn malformed or present geometry proof into empty")
    end
    weapon_mesh.native_vertex_proof=plain(empty_proof)
    local nonzero_invalid={GetAddress=function()return 9876 end,IsValid=function()return false end}
    weapon_mesh.StaticMesh=nonzero_invalid
    local proof_calls=0
    render_env.scope.vertex_state=function(handle)proof_calls=proof_calls+1;return actual_vertex_proof(handle)end
    T.check(not pcall(Render.capture,render_env,native_bindings)and proof_calls==0,
        "nonnull invalid asset wrapper is refused before any empty proof request")
    weapon_mesh.StaticMesh=nil;render_env.scope.vertex_state=actual_vertex_proof
    local material_getter=weapon_mesh.GetMaterial
    weapon_mesh.GetMaterial=function()weapon_mesh.StaticMesh=weapon_asset;return material_getter()end
    T.check(not pcall(Render.capture,render_env,native_bindings),"native material callback populating an empty mesh refuses capture")
    weapon_mesh.StaticMesh=nil;weapon_mesh.GetMaterial=material_getter
    render_env.scope.vertex_state=function(handle)local result=actual_vertex_proof(handle);weapon_mesh.StaticMesh=weapon_asset;return result end
    T.check(not pcall(Render.capture,render_env,native_bindings),"native empty proof callback populating the original mesh refuses capture")
    weapon_mesh.StaticMesh=nil;render_env.scope.vertex_state=actual_vertex_proof
    local tag_getter=weapon_mesh.ComponentHasTag
    weapon_mesh.ComponentHasTag=function()weapon_mesh.StaticMesh=weapon_asset;return false end
    T.check(not pcall(Render.capture,render_env,native_bindings),"late metadata callback populating empty static refuses final evidence")
    weapon_mesh.StaticMesh=nil;weapon_mesh.ComponentHasTag=tag_getter
    local count=0
    render_env.scope.vertex_state=function(handle)
        count=count+1;local proof=actual_vertex_proof(handle);proof.lod_info_count=count==1 and 0 or 1;return proof
    end
    T.check(not pcall(Render.capture,render_env,native_bindings),"changed actual empty LOD census refuses ending proof")
    render_env.scope.vertex_state=actual_vertex_proof
    weapon_mesh.StaticMesh=weapon_asset;weapon_mesh.native_vertex_proof=plain(native_present_proof)
    rvp.GetMeshComponentAmountOfVerticesOnLOD=count_getter;rvp.GetMeshComponentVertexColorsAtLOD_Wrapper=colors_getter
end)()
;(function()
    -- Actual skeletal GetNumMaterials returns0 with both assets absent; raw
    -- OverrideMaterials can still contain null and nonnull entries.
    local holder=mesh(202,"EmptySkeletalHolder",live_weapon,"skeletal",nil)
    local mesh_predicate=holder.IsA
    holder.IsA=function(self,k)return k=="/Script/Engine.MeshComponent"or mesh_predicate(self,k)end
    holder.GetAttachParent=function()return weapon_mesh end
    holder.GetNumBones=function()return 0 end
    holder.GetBoneName=function()error("empty holder must not invent a bone dictionary",0)end
    holder.GetNumMaterials=function()return 0 end
    local slots={ [1]=dynamic_mat }
    holder.GetMaterial=function(_,slot)return slots[slot]end
    holder.native_vertex_proof={state="native_empty_skeletal",lod_info_count=0,no_override=true,asset_present=false,
        material_count=3,material_null_mask=5,component_kind="skeletal"}
    local original_census=live_weapon.K2_GetComponentsByClass
    live_weapon.K2_GetComponentsByClass=function(_,class)
        if class=="/Script/Engine.SplineComponent"then return {}end
        if class~="/Script/Engine.MeshComponent"then error("only complete mesh/spline census",0)end
        return {wrapped(weapon_mesh),wrapped(holder)}
    end
    local function captured_holder()
        local capture=Render.capture(render_env,native_bindings)
        for _,c in ipairs(capture.components)do if c.name=="EmptySkeletalHolder"then return c,capture end end
        error("complete holder missing",0)
    end
    local copied,capture=captured_holder()
    T.check(copied.kind=="skeletal"and copied.component_class=="/Script/Engine.SkeletalMeshComponent"
        and copied.geometry=="native_empty"and copied.asset==""and copied.skeleton==""and #copied.bones==0
        and copied.vertex_state=="not_applicable"and #copied.vertex_colors==0,
        "qualified empty skeletal keeps its original component without invented asset/bones/paint")
    T.check(#copied.materials==3 and copied.materials[1].slot==0 and copied.materials[1].base==""
        and #copied.materials[1].scalars==0 and copied.materials[2].slot==1
        and copied.materials[2].base=="/Game/Test/Material.Material"and copied.materials[2].scalars[1].value==0.3125
        and copied.materials[3].slot==2 and copied.materials[3].base=="",
        "raw material count preserves explicit null/material/null slots despite native GetNumMaterials0")
    local parent_id;for _,c in ipairs(capture.components)do if c.name=="WeaponMesh"then parent_id=c.id end end
    T.check(copied.parent==parent_id and copied.owner==1 and copied.collision.enabled==3
        and copied.collision.simulating==true and T.eq(copied.relative.translation,{0.125,0.25,0.5}),
        "empty skeletal retains original owner attachment transforms and collision observations")
    local valid=plain(holder.native_vertex_proof)
    for _,key in ipairs({"state","lod_info_count","no_override","asset_present","material_count","material_null_mask","component_kind"})do
        holder.native_vertex_proof=plain(valid);holder.native_vertex_proof[key]=nil
        T.check(not pcall(captured_holder),"empty skeletal missing proof field "..key.." refuses")
    end
    for _,mutation in ipairs({
        {"material_count",33},{"material_count",1.5},{"material_null_mask",8},{"material_null_mask",-1},
        {"material_null_mask",0x100000000},{"component_kind","static"},{"component_kind",0},
        {"state","native_asset"},{"asset_present",true},{"no_override",false},{"guessed",true},
    })do
        holder.native_vertex_proof=plain(valid);holder.native_vertex_proof[mutation[1]]=mutation[2]
        T.check(not pcall(captured_holder),"empty skeletal counterfeit "..mutation[1].." refuses")
    end
    holder.native_vertex_proof=plain(valid)
    slots[0]=dynamic_mat
    T.check(not pcall(captured_holder),"native-null mask cannot hide a nonnull material slot")
    slots[0]=nil;slots[1]=nil
    T.check(not pcall(captured_holder),"native-nonnull slot cannot be silently converted to null")
    slots[1]={GetAddress=function()return 909 end,IsValid=function()return false end}
    T.check(not pcall(captured_holder),"nonnull invalid material wrapper cannot become a null slot")
    slots[1]=dynamic_mat
    holder.SkinnedAsset=body_asset
    T.check(not pcall(captured_holder),"nonzero hard SkinnedAsset alias refuses empty skeletal")
    holder.SkinnedAsset=nil;holder.SkeletalMesh=body_asset
    T.check(not pcall(captured_holder),"nonzero hard legacy skeletal alias refuses empty skeletal")
    holder.SkeletalMesh=nil;holder.GetNumBones=function()return 1 end
    T.check(not pcall(captured_holder),"empty native proof still requires actual zero render-bone getter")
    holder.GetNumBones=function()return 0 end
    local original_material=holder.GetMaterial
    holder.GetMaterial=function(self,slot)holder.SkinnedAsset=body_asset;return original_material(self,slot)end
    T.check(not pcall(captured_holder),"native material callback populating empty skeletal asset refuses")
    holder.SkinnedAsset=nil;holder.GetMaterial=original_material
    local proof_calls=0
    render_env.scope.vertex_state=function(handle)
        local p=actual_vertex_proof(handle)
        if p and p.component_kind=="skeletal"then proof_calls=proof_calls+1;if proof_calls>1 then p.material_null_mask=1 end end
        return p
    end
    T.check(not pcall(captured_holder),"ending raw material census change refuses whole source harvest")
    render_env.scope.vertex_state=actual_vertex_proof
    holder.native_vertex_proof.material_count=32;holder.native_vertex_proof.material_null_mask=0xFFFFFFFF
    slots={}
    copied=captured_holder()
    T.check(#copied.materials==32 and copied.materials[32].slot==31 and copied.materials[32].base=="",
        "all32 native null material slots and unsigned high mask bit are retained exactly")
    live_weapon.K2_GetComponentsByClass=original_census
end)()
local static_lod_getter=weapon_asset.GetNumLODs
for _,case in ipairs({
    {label="zero",getter=function()return 0 end,kind="number",value="0"},
    {label="nil",getter=function()return nil end,kind="nil",value="nil"},
    {label="false",getter=function()return false end,kind="boolean",value="false"},
    {label="error",getter=function()error("exact native getter error",0)end,kind="string",value="exact native getter error"},
})do
    weapon_asset.GetNumLODs=case.getter;render_phases={}
    local captured,reason=pcall(Render.capture,render_env,native_bindings)
    local last=render_phases[#render_phases]
    T.check(not captured and last.stage=="vertex_lods"and last.edge=="exit"and last.detail.ok==false
        and last.detail.getter=="StaticMesh.GetNumLODs"and reason:find(case.kind,1,true)and reason:find(case.value,1,true)
        and reason:find('asset_address=0x14A',1,true)and reason:find('asset_class="Class /Script/Engine.StaticMesh"',1,true)
        and reason:find('asset_path="/Game/Test/WeaponMesh.WeaponMesh"',1,true),
        "static native LOD "..case.label.." preserves typed original asset diagnostic and refuses")
end
weapon_asset.GetNumLODs=static_lod_getter
local original_static_asset=weapon_mesh.StaticMesh
local replacement_static_asset=object(331,"ReplacementMesh","/Game/Test/ReplacementMesh.ReplacementMesh",false)
replacement_static_asset.GetClass=weapon_asset.GetClass
weapon_asset.GetNumLODs=function()weapon_mesh.StaticMesh=replacement_static_asset;return 1 end
local static_changed,static_reason=pcall(Render.capture,render_env,native_bindings)
T.check(not static_changed and static_reason:find("native static asset changed",1,true),
    "static native asset replacement inside GetNumLODs refuses the original asset recipe")
weapon_mesh.StaticMesh=original_static_asset;weapon_asset.GetNumLODs=static_lod_getter
local original_static_path=weapon_asset.GetFullName
weapon_asset.GetNumLODs=function()weapon_asset.GetFullName=function()return "StaticMesh /Game/Test/ChangedPath.ChangedPath"end;return 1 end
static_changed,static_reason=pcall(Render.capture,render_env,native_bindings)
T.check(not static_changed and static_reason:find("native static asset changed",1,true),
    "static asset path mutation inside GetNumLODs refuses original identity")
weapon_asset.GetFullName=original_static_path;weapon_asset.GetNumLODs=static_lod_getter
local original_static_class=weapon_asset.GetClass
weapon_asset.GetNumLODs=function()weapon_asset.GetClass=body_asset.GetClass;return 1 end
static_changed,static_reason=pcall(Render.capture,render_env,native_bindings)
T.check(not static_changed and static_reason:find("native static asset changed",1,true),
    "static asset class mutation inside GetNumLODs refuses original identity")
weapon_asset.GetClass=original_static_class;weapon_asset.GetNumLODs=static_lod_getter
local metadata_class_reads,old_static_lod_calls=0,0
weapon_asset.GetClass=function()return {GetFullName=function()
    metadata_class_reads=metadata_class_reads+1
    if metadata_class_reads==2 then weapon_mesh.StaticMesh=replacement_static_asset end
    return "Class /Script/Engine.StaticMesh"
end}end
weapon_asset.GetNumLODs=function()old_static_lod_calls=old_static_lod_calls+1;return 1 end
static_changed,static_reason=pcall(Render.capture,render_env,native_bindings)
T.check(not static_changed and static_reason:find("native static asset changed",1,true)and old_static_lod_calls==0,
    "asset metadata callback replacement refuses before old asset GetNumLODs dispatch")
weapon_mesh.StaticMesh=original_static_asset;weapon_asset.GetClass=original_static_class;weapon_asset.GetNumLODs=static_lod_getter
local original_vertex_count=rvp.GetMeshComponentAmountOfVerticesOnLOD
weapon_asset.bAllowCPUAccess=false;render_phases={}
local count_ok,count_reason=pcall(Render.capture,render_env,native_bindings)
local count_exit=render_phases[#render_phases]
T.check(not count_ok and count_exit.stage=="vertex_count"and count_exit.edge=="exit"and count_exit.detail.ok==false
    and count_exit.detail.lod==0 and count_reason:find("returned_type=number returned_value=0",1,true)
    and count_reason:find("allow_cpu_access_type=boolean allow_cpu_access=false",1,true)
    and count_reason:find('asset_path="/Game/Test/WeaponMesh.WeaponMesh"',1,true),
    "resident staticLOD with native CPU-access false keeps exact zero count diagnostic and refuses")
weapon_asset.bAllowCPUAccess=true
for _,case in ipairs({
    {label="nil",getter=function()return nil end,kind="nil",value="nil"},
    {label="false",getter=function()return false end,kind="boolean",value="false"},
    {label="error",getter=function()error("exact native count error",0)end,kind="string",value="exact native count error"},
})do
    rvp.GetMeshComponentAmountOfVerticesOnLOD=function(self,c,lod)
        if c==weapon_mesh then return case.getter()end
        return original_vertex_count(self,c,lod)
    end
    render_phases={};count_ok,count_reason=pcall(Render.capture,render_env,native_bindings)
    count_exit=render_phases[#render_phases]
    T.check(not count_ok and count_exit.stage=="vertex_count"and count_exit.edge=="exit"and count_exit.detail.ok==false
        and count_exit.detail.getter=="GetMeshComponentAmountOfVerticesOnLOD"and count_exit.detail.lod==0
        and count_reason:find(case.kind,1,true)and count_reason:find(case.value,1,true)
        and count_reason:find("allow_cpu_access_type=boolean allow_cpu_access=true",1,true),
        "native vertex count "..case.label.." preserves typed facts without accepting a fallback")
end
weapon_asset.bAllowCPUAccess=nil
rvp.GetMeshComponentAmountOfVerticesOnLOD=function(self,c,lod)if c==weapon_mesh then return 0 end;return original_vertex_count(self,c,lod)end
count_ok,count_reason=pcall(Render.capture,render_env,native_bindings)
T.check(not count_ok and count_reason:find("allow_cpu_access_type=nil allow_cpu_access=nil",1,true),
    "unknown native CPU-access flag remains explicit nil rather than false or true")
weapon_asset.bAllowCPUAccess=true;rvp.GetMeshComponentAmountOfVerticesOnLOD=original_vertex_count
local original_body_lods=body.GetNumLODs
local asset_fallback_calls=0
body_asset.GetNumLODs=function()asset_fallback_calls=asset_fallback_calls+1;return 1 end
body.GetNumLODs=function()return false end
local body_lod_ok,body_lod_reason=pcall(Render.capture,render_env,native_bindings)
T.check(not body_lod_ok and asset_fallback_calls==0 and body_lod_reason:find("SkeletalMeshComponent.GetNumLODs returned_type=boolean returned_value=false",1,true),
    "false skeletal component LOD return never falls through to the asset getter")
body.GetNumLODs=original_body_lods;body_asset.GetNumLODs=nil
-- Exercise the actual adapter -> native-scope -> production render collector
-- lifecycle, including two complete equal harvests and failure cleanup.
for k,v in pairs(pawn)do host[k]=v end
host.GetActorScale3D=pawn.GetActorScale3D
local native_scope_begins,native_scope_ends=0,0
local adapter_scope={begin=function(meta,b)
    native_scope_begins=native_scope_begins+1;scope_rows={}
    if meta~=phase_context or b.pawn~=10 or b.world~=1 or b.controller~=9 or b.index~=0 then return nil,"exact scope metadata lost"end
    return 101
end,keep=function(id,row)if id~=101 then return nil,"scope id changed"end;return render_env.scope.keep(row)end,
    resolve=function(id,row)if id~=101 then return nil,"scope id changed"end;return render_env.scope.resolve(row)end,
    profile=function(id,row)if id~=101 then return nil,"scope id changed"end;return render_env.scope.profile(row)end,
    vertex_state=function(id,row)if id~=101 then return nil,"scope id changed"end;return render_env.scope.vertex_state(row)end,
    finish=function(id)native_scope_ends=native_scope_ends+1;return id==101 end}
local real_adapter=Adapter.new({WG=WG,source_scope=adapter_scope,
    resolve=function(index)return {index=index,world_key="fixture",pc_address=9,pc_name="PC",pawn_address=10,pawn_name="Pawn",pawn=host,world=render_world}end})
local captured_real,real_reason=real_adapter.capture(0,phase_context)
T.check(captured_real~=nil and native_scope_begins==2 and native_scope_ends==2,
    "production source adapter begins/ends original native scope for both equal full harvests: "..tostring(real_reason))
local native_count=body.GetNumBones
body.GetNumBones=function()error("fixture source getter failed",0)end
T.check(real_adapter.capture(0,phase_context)==nil and native_scope_begins==3 and native_scope_ends==3,
    "failed production source getter always releases scalar-only native scope")
body.GetNumBones=native_count
local before_begins,before_ends=native_scope_begins,native_scope_ends
local adapter_static=mesh(204,"AdapterStaticMesh",host,"static",weapon_asset)
local adapter_static_is_a=adapter_static.IsA
adapter_static.IsA=function(self,k)return k=="/Script/Engine.MeshComponent"or adapter_static_is_a(self,k)end
adapter_static.GetAttachParent=function()return root end;adapter_static.native_vertex_proof=plain(no_override_proof)
source_scenes[#source_scenes+1]=adapter_static;weapon_asset.bAllowCPUAccess=false
local captured_native_asset,native_asset_reason=real_adapter.capture(0,phase_context)
local adapter_native_component
for _,c in ipairs(captured_native_asset and captured_native_asset.components or {})do if c.name=="AdapterStaticMesh"then adapter_native_component=c end end
T.check(adapter_native_component and adapter_native_component.vertex_state=="native_asset"
    and native_scope_begins==before_begins+2 and native_scope_ends==before_ends+2,
    "production adapter delegates sixth API across two complete equal original-native harvests: "..tostring(native_asset_reason))
weapon_asset.bAllowCPUAccess=true
local vertex_proof_calls=0
render_env.scope.vertex_state=function(handle)
    vertex_proof_calls=vertex_proof_calls+1
    return vertex_proof_calls==1 and plain(no_override_proof)or plain(native_present_proof)
end
T.check(real_adapter.capture(0,phase_context)==nil and vertex_proof_calls==2,
    "override-state change between two complete native harvests refuses the recipe")
render_env.scope.vertex_state=actual_vertex_proof;table.remove(source_scenes)
local adapter_vertex_state=adapter_scope.vertex_state;adapter_scope.vertex_state=nil
T.check(real_adapter.capture(0,phase_context)==nil,"production adapter requires sixth native capability instead of delegating a fallback")
adapter_scope.vertex_state=adapter_vertex_state
local absent_scope_adapter=Adapter.new({WG=WG,resolve=function(index)return {index=index,world_key="fixture",pc_address=9,pc_name="PC",pawn_address=10,pawn_name="Pawn",pawn=host,world=render_world}end})
T.check(absent_scope_adapter.capture(0,phase_context)==nil,"production source collector refuses missing native identity capability")
host.K2_GetComponentsByClass=function(self,class)local out=source_mesh_return(self,class);out.outTable=false;return out end
local malformed,malformed_reason=pcall(Render.capture,render_env,native_bindings)
T.check(not malformed and malformed_reason:find('K2_GetComponentsByClass(MeshComponent) collect owner=Pawn',1,true)
    and malformed_reason:find('key="outTable" key_type=string value_type=boolean',1,true),"actual scene getter refusal identifies unexpected native return key without filtering it")
host.K2_GetComponentsByClass=source_mesh_return
local morph_getter=body_asset.GetMorphTargetsPtrConv
body_asset.GetMorphTargetsPtrConv=function()local out=morph_getter();out.outTable=false;return out end
malformed,malformed_reason=pcall(Render.capture,render_env,native_bindings)
T.check(not malformed and malformed_reason:find('SkeletalMesh.GetMorphTargetsPtrConv component=BodyMesh',1,true),"morph return refusal names exact native producer")
body_asset.GetMorphTargetsPtrConv=morph_getter
local color_getter=rvp.GetMeshComponentVertexColorsAtLOD_Wrapper
render_phases={}
rvp.GetMeshComponentVertexColorsAtLOD_Wrapper=function()
    local last=render_phases[#render_phases]
    if not last or last.stage~="vertex_color_getter"or last.edge~="enter"or last.detail.lod~=0 then error("vertex entry missing before native getter",0)end
    error("synthetic vertex native getter refused",0)
end
malformed,malformed_reason=pcall(Render.capture,render_env,native_bindings)
T.check(not malformed and malformed_reason:find("synthetic vertex native getter refused",1,true)
    and render_phases[#render_phases].stage=="vertex_color_getter"and render_phases[#render_phases].edge=="enter",
    "blocking native color getter receives persisted entry before call and no fabricated exit")
rvp.GetMeshComponentVertexColorsAtLOD_Wrapper=color_getter
rvp.GetMeshComponentVertexColorsAtLOD_Wrapper=function(self,c)local out=color_getter(self,c);out.outTable=false;return out end
malformed,malformed_reason=pcall(Render.capture,render_env,native_bindings)
T.check(not malformed and malformed_reason:find('GetMeshComponentVertexColorsAtLOD_Wrapper component=BodyMesh lod=0',1,true),"vertex return refusal preserves exact component and LOD getter context")
rvp.GetMeshComponentVertexColorsAtLOD_Wrapper=color_getter
local anchor=scene(object(101,"ActualSceneAnchor","/Game/Test/Runtime.ActualSceneAnchor",false),host,"/Script/Engine.SceneComponent")
anchor.GetAttachParent=function()return root end;anchor.GetAttachSocketName=function()return fname("ExactAnchorSocket")end
body.GetAttachParent=function()return anchor end
local anchored,anchor_reason=pcall(Render.capture,render_env,native_bindings)
T.check(anchored and #anchor_reason.components==4,"actual hard parent outside mesh census is fully included")
local captured_anchor=anchor_reason.components[1]
T.check(captured_anchor.name=="ActualSceneAnchor" and captured_anchor.relative.translation[1]==0.125
    and captured_anchor.relative.translation[2]==-2.5 and captured_anchor.relative.scale[3]==2
    and anchor_reason.components[2].parent==captured_anchor.id,"hard parent closure preserves complete actual relative transform and native link")
local parent_ops=0;anchor.GetWorld=function()scope=false;return render_world end
anchor.GetRelativeTransform=function()parent_ops=parent_ops+1;return {}end
T.check(not pcall(Render.capture,render_env,native_bindings) and parent_ops==0,"world change during attachment evidence prevents later parent getters")
scope=true;body.GetAttachParent=function()return root end
anchor.GetWorld=function()return render_world end
scene(anchor,host,"/Script/Engine.SceneComponent")
local spline=scene(object(102,"ActualSpline","/Game/Test/Runtime.ActualSpline",false),host,"/Script/Engine.SplineComponent")
spline.bDrawDebug=false;spline.GetAttachParent=function()return root end;anchor.GetAttachParent=function()return spline end;body.GetAttachParent=function()return anchor end
source_scenes={body,root,anchor,spline}
local closed=Render.capture(render_env,native_bindings)
local closed_by_name={};for _,c in ipairs(closed.components)do closed_by_name[c.name]=c end
T.check(#closed.components==5 and closed_by_name.BodyMesh.parent==closed_by_name.ActualSceneAnchor.id
    and closed_by_name.ActualSceneAnchor.parent==closed_by_name.ActualSpline.id and closed_by_name.ActualSpline.parent==closed_by_name.CapsuleRoot.id,
    "actual scene spline capsule ancestor graph closes without dropping native parents")
T.check(closed_by_name.ActualSceneAnchor.collision==false and closed_by_name.ActualSpline.collision~=false
    and closed_by_name.ActualSceneAnchor.vertex_state=="not_applicable" and closed_by_name.ActualSpline.scene.draw_debug==false,
    "anchor collision and geometry applicability reflect exact native classes")
T.check(T.eq(closed_by_name.ActualSceneAnchor.relative.translation[1],0.125),"native anchor transform is copied without flattening")
for i=1,300 do
    local helper=object(1000+i,"NativeCollisionHelper_"..i,"/Game/Test/Runtime.Helper_"..i,false)
    helper.GetOwner=function()error("nonmesh helper was enumerated",0)end
    helper.GetAttachParent=function()error("nonancestor helper was traversed",0)end
    source_scenes[#source_scenes+1]=helper
end
local helper_closed=Render.capture(render_env,native_bindings)
T.check(#source_scenes>256 and #helper_closed.components==5 and scene_census_calls==0,
    "more than256 nonmesh helpers do not truncate complete mesh/root/hard ancestor closure")
local extra_gear=mesh(202,"NativeExtraGear",host,"static",weapon_asset)
extra_gear.native_vertex_proof=plain(native_present_proof)
local extra_is_a=extra_gear.IsA;extra_gear.IsA=function(self,k)return k=="/Script/Engine.MeshComponent"or extra_is_a(self,k)end
extra_gear.bHiddenInGame=true;extra_gear.GetAttachParent=function()return root end
source_scenes[#source_scenes+1]=extra_gear
local all_gear=Render.capture(render_env,native_bindings);local copied_gear
for _,c in ipairs(all_gear.components)do if c.name=="NativeExtraGear"then copied_gear=c end end
T.check(#all_gear.components==6 and copied_gear and copied_gear.hidden and copied_gear.parent~=0,"every native gear mesh is kept even when source hidden")
table.remove(source_scenes)
local unknown_mesh=object(203,"UnknownNativeRender","/Game/Test/Runtime.UnknownNativeRender",false)
unknown_mesh.IsA=function(_,k)return k=="/Script/Engine.MeshComponent"end;unknown_mesh.GetOwner=function()return host end
unknown_mesh.GetAttachParent=function()return root end
source_scenes[#source_scenes+1]=unknown_mesh
local unknown_ok,unknown_reason=pcall(Render.capture,render_env,native_bindings)
T.check(not unknown_ok and unknown_reason:find("native render component kind unsupported",1,true),"unknown native mesh rendering refuses instead of being filtered")
table.remove(source_scenes)
local original_relative=anchor.GetRelativeTransform
anchor.GetRelativeTransform=function()body.GetAttachParent=function()return root end;return original_relative()end
T.check(not pcall(Render.capture,render_env,native_bindings),"hard parent path mutation during getter refuses source capture")
anchor.GetRelativeTransform=original_relative;body.GetAttachParent=function()return anchor end
local original_name=anchor.GetFName
anchor.GetRelativeTransform=function()anchor.GetFName=function()return fname("ChangedAnchor")end;return original_relative()end
T.check(not pcall(Render.capture,render_env,native_bindings),"native ancestor FName mutation refuses replayed identity")
anchor.GetRelativeTransform=original_relative;anchor.GetFName=original_name
local foreign_actor=object(500,"ForeignActor","/Game/Test/Runtime.ForeignActor",false);foreign_actor.RootComponent=root
anchor.GetRelativeTransform=function()anchor.GetOwner=function()return foreign_actor end;return original_relative()end
T.check(not pcall(Render.capture,render_env,native_bindings),"native ancestor owner mutation refuses replayed binding")
anchor.GetRelativeTransform=original_relative;anchor.GetOwner=function()return host end
local original_bone_count=body.GetNumBones
body.GetNumBones=function()host.RootComponent=anchor;return original_bone_count()end
T.check(not pcall(Render.capture,render_env,native_bindings),"actual owner root mutation during getter refuses source capture")
body.GetNumBones=original_bone_count;host.RootComponent=root
body.GetNumBones=function()source_scenes[#source_scenes+1]=unknown_mesh;return original_bone_count()end
T.check(not pcall(Render.capture,render_env,native_bindings),"new native mesh during source harvest refuses incomplete source census")
body.GetNumBones=original_bone_count;table.remove(source_scenes)
local many_meshes={};for i=1,65 do many_meshes[i]=wrapped(body)end
host.K2_GetComponentsByClass=function()return many_meshes end
local bound_ok,bound_reason=pcall(Render.capture,render_env,native_bindings)
T.check(not bound_ok and bound_reason:find('max=64',1,true),"native complete mesh seed bound stays64 rather than taking a subset")
host.K2_GetComponentsByClass=source_mesh_return
local head=root
for i=1,63 do
    local parent=head;head=scene(object(2000+i,"DeepAncestor_"..i,"/Game/Test/Runtime.DeepAncestor_"..i,false),host,"/Script/Engine.SceneComponent")
    head.GetAttachParent=function()return parent end
end
body.GetAttachParent=function()return head end
bound_ok,bound_reason=pcall(Render.capture,render_env,native_bindings)
T.check(not bound_ok and bound_reason:find('native complete attachment component bound',1,true),"full actual parent closure retains final64 component bound")
body.GetAttachParent=function()return anchor end
spline.bDrawDebug=true
local exact_spline_profile={position_count=3,rotation_count=2,scale_count=1,reparam_count=21,metadata_null=true}
spline.native_spline_profile=plain(exact_spline_profile)
spline.GetSplinePointAt=function()error("converted rotator point replay is incomplete",0)end
spline.SplineCurves=setmetatable({},{__index=function()error("raw spline point harvesting belongs to native bulk capture",0)end})
local spline_ok,spline_reason=pcall(Render.capture,render_env,native_bindings)
local function copied_spline(capture,name)
    for _,c in ipairs(capture.components)do if c.name==(name or "ActualSpline")then return c end end
end
local native_spline=spline_ok and copied_spline(spline_reason)
T.check(native_spline and native_spline.kind=="spline" and native_spline.geometry=="native_spline"
    and native_spline.vertex_state=="not_applicable"and native_spline.scene.type=="not_applicable"
    and T.eq(native_spline.spline_profile,exact_spline_profile)and native_spline.visible and not native_spline.hidden,
    "native visible debug spline records exact bulk profile without claiming absent rendering")
T.check(native_spline and #native_spline.bones==0 and #native_spline.materials==0
    and #native_spline.vertex_colors==0 and native_spline.asset=="" and native_spline.collision~=false,
    "native spline replay uses raw native curves rather than inventing a mesh/material/vertex recipe")
spline.bHiddenInGame=true
spline_ok,spline_reason=pcall(Render.capture,render_env,native_bindings)
T.check(spline_ok and copied_spline(spline_reason).hidden and copied_spline(spline_reason).kind=="spline",
    "hidden native debug spline still requires actual native replay profile")
spline.bHiddenInGame=false;spline.IsVisible=function()return false end
spline_ok,spline_reason=pcall(Render.capture,render_env,native_bindings)
T.check(spline_ok and not copied_spline(spline_reason).visible and copied_spline(spline_reason).kind=="spline",
    "invisible native debug spline is retained with exact source profile and flags")
local standalone=scene(object(103,"StandaloneStepSpline","/Game/Test/Runtime.StandaloneStepSpline",false),host,"/Script/Engine.SplineComponent")
standalone.bDrawDebug=true;standalone.native_spline_profile=plain(exact_spline_profile)
standalone.GetAttachParent=function()return root end
source_scenes[#source_scenes+1]=standalone
local standalone_capture=Render.capture(render_env,native_bindings)
T.check(copied_spline(standalone_capture,"StandaloneStepSpline")~=nil and #standalone_capture.components==6,
    "all native owned splines are kept even without mesh descendants")
local profile_calls=0;local native_profile_getter=render_env.scope.profile
render_env.scope.profile=function(handle)
    local last=render_phases[#render_phases]
    if not last or last.stage~="spline_profile"or last.edge~="enter"then error("spline profile entry missing",0)end
    profile_calls=profile_calls+1;return native_profile_getter(handle)
end
local profile_capture=Render.capture(render_env,native_bindings)
T.check(profile_calls==2 and copied_spline(profile_capture,"StandaloneStepSpline").spline_profile.position_count==3,
    "rare spline profile entries persist before exactly one guarded native bulk call per rendered spline")
render_env.scope.profile=native_profile_getter
local mutations={
    {"metadata false",function(p)p.metadata_null=false end},
    {"metadata missing",function(p)p.metadata_null=nil end},
    {"position count missing",function(p)p.position_count=nil end},
    {"rotation count missing",function(p)p.rotation_count=nil end},
    {"scale count missing",function(p)p.scale_count=nil end},
    {"reparam count missing",function(p)p.reparam_count=nil end},
    {"position overflow",function(p)p.position_count=65 end},
    {"rotation overflow",function(p)p.rotation_count=65 end},
    {"scale overflow",function(p)p.scale_count=65 end},
    {"reparam overflow",function(p)p.reparam_count=1025 end},
    {"noninteger count",function(p)p.position_count=1.5 end},
    {"negative count",function(p)p.position_count=-1 end},
    {"unknown field",function(p)p.extra=0 end},
}
for _,mutation in ipairs(mutations)do
    spline.native_spline_profile=plain(exact_spline_profile);mutation[2](spline.native_spline_profile)
    T.check(not pcall(Render.capture,render_env,native_bindings),"native spline profile refuses "..mutation[1])
end
spline.native_spline_profile=plain(exact_spline_profile)
render_env.scope.profile=nil
T.check(not pcall(Render.capture,render_env,native_bindings),"native debug spline refuses missing bulk profile capability")
render_env.scope.profile=native_profile_getter
render_env.scope.profile=function(handle)
    local result=native_profile_getter(handle);spline.GetAttachParent=function()return anchor end;return result
end
T.check(not pcall(Render.capture,render_env,native_bindings),"parent mutation inside native profile call refuses original source scope")
spline.GetAttachParent=function()return root end;render_env.scope.profile=native_profile_getter
local original_spline_body_bones=body.GetNumBones
body.GetNumBones=function()table.remove(source_scenes);return original_spline_body_bones()end
T.check(not pcall(Render.capture,render_env,native_bindings),"native spline removal during metadata harvest refuses complete ending spline census")
body.GetNumBones=original_spline_body_bones
-- Previous mutation removed the standalone spline; do not retain it implicitly.
for i=#source_scenes,1,-1 do if source_scenes[i]==standalone then table.remove(source_scenes,i)end end
body.GetNumBones=function()source_scenes[#source_scenes+1]=standalone;return original_spline_body_bones()end
T.check(not pcall(Render.capture,render_env,native_bindings),"native spline addition during metadata harvest refuses complete ending spline census")
body.GetNumBones=original_spline_body_bones;table.remove(source_scenes)
local native_profile_calls=0
render_env.scope.profile=function(handle)
    native_profile_calls=native_profile_calls+1
    local result=native_profile_getter(handle);result.position_count=native_profile_calls;return result
end
local changing_recipe,changing_reason=real_adapter.capture(0,phase_context)
T.check(changing_recipe==nil and changing_reason:find("source recipe changed",1,true),
    "changed native raw spline counts require a new coherent descriptor instead of stale buffer sizing")
render_env.scope.profile=native_profile_getter
spline.IsVisible=function()return true end;spline.bDrawDebug=nil
spline_ok,spline_reason=pcall(Render.capture,render_env,native_bindings)
T.check(not spline_ok and spline_reason:find('draw_debug_type=nil draw_debug=unavailable(nil)',1,true),
    "missing native spline bool read is distinguished from actual true/false")
T.check(spline_reason:find('component_transient=false component_garbage=false',1,true)
    and spline_reason:find('owner_hidden=unavailable(nil)',1,true)
    and spline_reason:find('owner_transient=false owner_garbage=false',1,true),
    "native component/owner flags are actual booleans and missing actor hidden flag stays explicit")
local old_host_name,old_root_name,old_spline_name=host.GetFName,root.GetFName,spline.GetFName
host.GetFName=function()return fname(string.rep("NativeOwner",100))end
root.GetFName=function()return fname(string.rep("NativeRoot",100))end
spline.GetFName=function()return fname(string.rep("NativeSpline",100))end
host.bHidden=true;spline.bDrawDebug=nil
spline_ok,spline_reason=pcall(Render.capture,render_env,native_bindings)
local persisted_reason=spline_reason:sub(1,512)
T.check(not spline_ok and #spline_reason>512 and persisted_reason:find('draw_debug_type=nil draw_debug=unavailable(nil) visible=true hidden=false owner_hidden=true',1,true)
    and persisted_reason:find('component_transient=false component_garbage=false owner_transient=false owner_garbage=false',1,true),
    "all decisive native spline flags survive actual512-byte worker phase limit even with long labels")
host.GetFName,root.GetFName,spline.GetFName=old_host_name,old_root_name,old_spline_name
host.bHidden=nil
spline.bDrawDebug=false;root.bHiddenInGame=false
T.check(not pcall(Render.capture,render_env,native_bindings),"visible capsule root never becomes nonrendering anchor by class")
root.bHiddenInGame=true;spline.GetAttachParent=function()return anchor end
T.check(not pcall(Render.capture,render_env,native_bindings),"native scene ancestor cycle refuses capture")
spline.GetAttachParent=function()return root end;anchor.GetClass=function()return {GetFullName=function()return "Class /Script/Engine.UnknownSceneSubclass"end}end
T.check(not pcall(Render.capture,render_env,native_bindings),"unknown scene subclass cannot claim absent rendering")
anchor.GetClass=function()return {GetFullName=function()return "Class /Script/Engine.SceneComponent"end}end
anchor.GetWorld=function()return {IsValid=function()return true end,GetAddress=function()return 99 end}end
T.check(not pcall(Render.capture,render_env,native_bindings),"foreign world native ancestor refuses source capture")
anchor.GetWorld=function()return render_world end;anchor.GetOwner=function()return foreign_actor end
T.check(not pcall(Render.capture,render_env,native_bindings),"parent owned outside original source pawn/weapon set refuses capture")
anchor.GetOwner=function()return host end;source_scenes={body,root};body.GetAttachParent=function()return root end
T.check(T.eq(rendered.components[1].physics_asset,"/Game/Test/Physics.Physics"),"null native component override selects actual skeletal asset physics")
local override_asset=object(321,"OverridePhysics","/Game/Test/OverridePhysics.OverridePhysics",false)
body.PhysicsAssetOverride=override_asset
local override_render=Render.capture(render_env,native_bindings)
T.check(T.eq(override_render.components[1].physics_asset,"/Game/Test/OverridePhysics.OverridePhysics"),"nonnull native component override preserves its distinct exact physics asset")
body.PhysicsAssetOverride=nil
host.K2_GetComponentsByClass=function(_,class)return class=="/Script/Engine.MeshComponent"and {body}or {}end
live_weapon.K2_GetComponentsByClass=function(_,class)return class=="/Script/Engine.MeshComponent"and {weapon_mesh}or {}end
T.check(pcall(Render.capture,render_env,native_bindings),"direct returned UObject entries never receive parameter get")
host.K2_GetComponentsByClass=function()return {[2]=wrapped(body)}end
T.check(not pcall(Render.capture,render_env,native_bindings),"malformed native component return table refuses capture")
host.K2_GetComponentsByClass=source_mesh_return;live_weapon.K2_GetComponentsByClass=function(_,class)return class=="/Script/Engine.MeshComponent"and {wrapped(weapon_mesh)}or {}end
local bones_after_loss=0;body_asset.GetPhysicsAsset=function()scope=false;return physical_asset end
body.GetNumBones=function()bones_after_loss=bones_after_loss+1;return #bone_names end
T.check(not pcall(Render.capture,render_env,native_bindings) and bones_after_loss==0,"world loss inside render getter prevents next source operation")
scope=true;body_asset.GetPhysicsAsset=function()return physical_asset end
body_asset.GetPhysicsAsset=function()body.GetSkeletalMeshAsset=function()return weapon_asset end;return physical_asset end
T.check(not pcall(Render.capture,render_env,native_bindings),"source skeletal asset replacement during physics getter refuses stale capture")
body.GetSkeletalMeshAsset=function()return body_asset end;body_asset.GetPhysicsAsset=function()return physical_asset end
body_asset.GetPhysicsAsset=function()body.PhysicsAssetOverride=override_asset;return physical_asset end
T.check(not pcall(Render.capture,render_env,native_bindings),"native override replacement during default physics getter refuses capture")
body.PhysicsAssetOverride=nil;body_asset.GetPhysicsAsset=function()return physical_asset end
;(function()
    -- Exact offline native-class wrappers exercise the production collector.
    -- The socket label is returned by the source; it is never a default name.
    local arm=scene(object(8001,"ArmCameraBoom","/Game/Test/Runtime.ArmCameraBoom",false),host,"/Script/Engine.SpringArmComponent")
    local camera=scene(object(8002,"CameraFollow","/Game/Test/Runtime.CameraFollow",false),host,"/Script/Engine.CameraComponent")
    local socket="OfflineExactSocket"
    local socket_reads=0
    local function sockets()
        socket_reads=socket_reads+1
        return {wrapped(fname(socket))}
    end
    arm.bDrawDebugLagMarkers=false;arm.GetAllSocketNames=sockets
    arm.GetAttachParent=function()return root end
    camera.GetAttachParent=function()return arm end
    camera.GetAttachSocketName=function()return fname(socket)end
    body.GetAttachParent=function()return camera end
    local function no_primitive()error("native Camera/SpringArm is not a PrimitiveComponent",0)end
    for _,c in ipairs({arm,camera})do
        c.GetCollisionResponseToChannel=no_primitive;c.GetCollisionEnabled=no_primitive
        c.GetCollisionObjectType=no_primitive;c.GetCollisionProfileName=no_primitive;c.IsSimulatingPhysics=no_primitive
    end
    local capture=Render.capture(render_env,native_bindings)
    local copied={};for _,c in ipairs(capture.components)do copied[c.name]=c end
    local a,c=copied.ArmCameraBoom,copied.CameraFollow
    T.check(a and c and a.kind=="scene"and c.kind=="scene"and a.scene.type=="spring_arm"and c.scene.type=="camera",
        "exact native Camera and SpringArm ancestors have distinct required replay evidence")
    T.check(a.scene.draw_debug_lag_markers==false and a.scene.socket_name==socket and socket_reads==2,
        "native SpringArm singleton socket and debug flag are harvested freshly before and after metadata")
    T.check(a.collision==false and c.collision==false and a.geometry=="not_applicable"and c.vertex_state=="not_applicable",
        "nonprimitive native Camera and SpringArm never invoke or invent collision/geometry data")
    T.check(c.parent==a.id and c.socket==socket and copied.BodyMesh.parent==c.id and a.parent==copied.CapsuleRoot.id,
        "complete camera-arm-root hierarchy retains the actual named attachment socket")
    T.check(T.eq(a.relative.translation,{0.125,-2.5,3})and T.eq(c.relative.scale,{0.5,1,2}),
        "source Camera and SpringArm retain exact native transforms")
    arm.IsVisible=function()return false end;camera.bHiddenInGame=true
    capture=Render.capture(render_env,native_bindings)
    copied={};for _,v in ipairs(capture.components)do copied[v.name]=v end
    T.check(not copied.ArmCameraBoom.visible and copied.CameraFollow.hidden,
        "Camera and SpringArm visibility flags are copied from the source")
    arm.IsVisible=function()return true end;camera.bHiddenInGame=false
    for _,draw in ipairs({true,"false",0})do
        arm.bDrawDebugLagMarkers=draw
        T.check(not pcall(Render.capture,render_env,native_bindings),"native SpringArm refuses unsupported debug type/value "..type(draw))
    end
    arm.bDrawDebugLagMarkers=nil
    T.check(not pcall(Render.capture,render_env,native_bindings),"missing native SpringArm debug flag is unknown")
    arm.bDrawDebugLagMarkers=false
    for _,result in ipairs({{}, {wrapped(fname(socket)),wrapped(fname("OtherSocket"))}, {[2]=wrapped(fname(socket))}, {outTable=false}})do
        arm.GetAllSocketNames=function()return result end
        T.check(not pcall(Render.capture,render_env,native_bindings),"native SpringArm requires a complete strict singleton returned array")
    end
    arm.GetAllSocketNames=sockets
    for _,value in ipairs({"","None",string.rep("x",129),"x\0y"})do
        socket=value
        T.check(not pcall(Render.capture,render_env,native_bindings),"native SpringArm socket name is observed and bounded")
    end
    socket="OfflineExactSocket"
    arm.GetAllSocketNames=function()
        return {wrapped({ToString=function()arm.bDrawDebugLagMarkers=true;return socket end})}
    end
    T.check(not pcall(Render.capture,render_env,native_bindings),"socket name conversion changing native debug state refuses capture")
    arm.bDrawDebugLagMarkers=false;arm.GetAllSocketNames=sockets
    local bone_getter=body.GetNumBones
    body.GetNumBones=function()socket="ChangedSocket";return bone_getter()end
    local ok,reason=pcall(Render.capture,render_env,native_bindings)
    T.check(not ok and reason:find("native spring arm evidence changed during harvest",1,true),
        "later component metadata changing an earlier arm socket refuses ending evidence")
    socket="OfflineExactSocket";body.GetNumBones=function()arm.bDrawDebugLagMarkers=true;return bone_getter()end
    T.check(not pcall(Render.capture,render_env,native_bindings),"later component metadata changing an earlier arm debug flag refuses ending evidence")
    body.GetNumBones=bone_getter;arm.bDrawDebugLagMarkers=false
    arm.GetAllSocketNames=function()scope=false;return {wrapped(fname(socket))}end
    T.check(not pcall(Render.capture,render_env,native_bindings),"world loss in native socket getter prevents stale source replay")
    scope=true;arm.GetAllSocketNames=sockets
    camera.GetClass=function()return {GetFullName=function()return "Class /Script/Engine.UnknownCameraSubclass"end}end
    T.check(not pcall(Render.capture,render_env,native_bindings),"unproved Camera subclass cannot inherit absent-rendering evidence")
    body.GetAttachParent=function()return root end
end)()
dynamic_mat.FontParameterValues=array({true})
T.check(not pcall(Render.capture,render_env,native_bindings),"unsupported native material parameter refuses rather than drops data")
dynamic_mat.FontParameterValues=array({})
body.GetOverlayMaterial=function()return cooked_mat end
T.check(not pcall(Render.capture,render_env,native_bindings),"native overlay material refuses rather than disappears from mirror")
body.GetOverlayMaterial=function()return nil end;body.IsUsingSkinWeightProfile=function()return true end
T.check(not pcall(Render.capture,render_env,native_bindings),"native skin weight mutation refuses unverified geometry")
body.IsUsingSkinWeightProfile=function()return false end;body.IsMaterialSectionShown=function()return false end
T.check(not pcall(Render.capture,render_env,native_bindings),"native hidden section refuses incomplete render recipe")
body.IsMaterialSectionShown=function()return true end;body_asset.HasAnyFlags=function()return true end
body_asset.GetFullName=function()return "SkeletalMesh /Engine/Transient.SkeletalMesh_1"end
local transient_render=Render.capture(render_env,native_bindings)
T.check(T.eq(transient_render.components[1].geometry,"runtime_transient"),"RF_Transient does not invent an observed merge recipe")
body_asset.HasAnyFlags=function()return nil end
T.check(not pcall(Render.capture,render_env,native_bindings),"missing native transient flag refuses rather than assumes cooked geometry")
body_asset.HasAnyFlags=function()return true end;body.IsBoneHiddenByName=function()return nil end
T.check(not pcall(Render.capture,render_env,native_bindings),"missing native bone visibility refuses rather than assumes visible")
body.IsBoneHiddenByName=function()return false end;body.ComponentHasTag=function()return nil end
T.check(not pcall(Render.capture,render_env,native_bindings),"missing native component tags refuse rather than erase persistent parts")
StaticFindObject,FName=saved_find,saved_fname
