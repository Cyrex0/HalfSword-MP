local D=dofile("mods/HSMPMatch/Scripts/native_source_descriptor.lua")
local Adapter=dofile("mods/HSMPMatch/Scripts/native_source_adapter.lua")
local function plain(v)if type(v)~="table"then return v end;local out={};for k,x in pairs(v)do out[k]=plain(x)end;return out end
local function fixture(path)local f=assert(io.open(path,"rb"));local s=f:read("*a");f:close();return plain(T.json_decode(s))end
local recipe=fixture("tools/hsmp-tools/lua-tests/fixtures/native_source_recipe.json")
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
local function fname(value)return {ToString=function()return value end}end
local function array(values)return {GetArrayNum=function()return #values end,ForEach=function(_,fn)for i,v in ipairs(values)do fn(i,wrapped(v))end end}end
local render_world={GetAddress=function()return 1 end,IsValid=function()return true end}
local runtime_objects={}
local function object(address,n,path,transient)
    local value={GetAddress=function()return address end,IsValid=function()return true end,GetFName=function()return fname(n)end,
        GetFullName=function()return "FixtureClass "..path end,HasAnyFlags=function()return transient end,
        GetWorld=function()return render_world end,GetClass=function()return {GetFullName=function()return "Class /Script/Engine.SceneComponent"end}end,
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
body_asset.Skeleton=object(301,"Skeleton","/Game/Test/Skeleton.Skeleton",false)
body_asset.GetOverlayMaterial=function()return nil end;body_asset.GetDefaultMeshDeformer=function()return nil end
body_asset.MeshClothingAssets=array({});body_asset.GetMorphTargetsPtrConv=function()return {wrapped(object(302,"ExactMorph","/Game/Test/Morph.Morph",false))}end
local cooked_mat=object(310,"Material","/Game/Test/Material.Material",false)
local dynamic_mat=object(311,"MID","/Engine/Transient.MaterialInstanceDynamic_1",true)
dynamic_mat.IsA=function(_,c)return c=="/Script/Engine.MaterialInstance"end
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
    c.GetNumMaterials=function()return 1 end;c.GetMaterial=function()return dynamic_mat end;c.GetNumLODs=function()return 1 end
    c.ComponentHasTag=function()return false end;c.native_colors={{R=255,G=128,B=0,A=255},{R=255,G=128,B=0,A=255}}
    return c
end
local body=mesh(11,"BodyMesh",host,"skeletal",body_asset);host.Mesh=body;body.GetAttachParent=function()return root end
local weapon_asset=object(330,"WeaponMesh","/Game/Test/WeaponMesh.WeaponMesh",false);weapon_asset.GetNumLODs=function()return 1 end
local weapon_mesh=mesh(201,"WeaponMesh",live_weapon,"static",weapon_asset);weapon_mesh.GetAttachParent=function()return body end
local mesh_is_a=body.IsA;body.IsA=function(self,k)return k=="/Script/Engine.MeshComponent"or mesh_is_a(self,k)end
local weapon_is_a=weapon_mesh.IsA;weapon_mesh.IsA=function(self,k)return k=="/Script/Engine.MeshComponent"or weapon_is_a(self,k)end
live_weapon.RootComponent=weapon_mesh
local source_scenes={body,root}
local scene_census_calls,mesh_return_calls=0,0
local function source_mesh_return(_,class)
    mesh_return_calls=mesh_return_calls+1
    if class~="/Script/Engine.MeshComponent"then scene_census_calls=scene_census_calls+1;error("full helper Scene enumeration is forbidden",0)end
    local out={};for _,c in ipairs(source_scenes)do if c:IsA(class)then out[#out+1]=wrapped(c)end end;return out
end
host.K2_GetComponentsByClass=source_mesh_return;live_weapon.K2_GetComponentsByClass=function(_,class)
    mesh_return_calls=mesh_return_calls+1
    if class~="/Script/Engine.MeshComponent"then scene_census_calls=scene_census_calls+1;error("full weapon Scene enumeration is forbidden",0)end
    return {wrapped(weapon_mesh)}end
local plain_component_getter_calls=0
local function unavailable_component_alias()plain_component_getter_calls=plain_component_getter_calls+1;error("unreflected GetComponentsByClass alias",0)end
host.GetComponentsByClass=unavailable_component_alias;live_weapon.GetComponentsByClass=unavailable_component_alias
local rvp={IsValid=function()return true end,GetMeshComponentAmountOfVerticesOnLOD=function(_,c)return #c.native_colors end,
    GetMeshComponentVertexColorsAtLOD_Wrapper=function(_,c)return color_return(c.native_colors)end}
StaticFindObject=function(p)return runtime_objects[p]or(p=="/Script/VertexPaintDetectionPlugin.Default__VertexPaintFunctionLibrary"and rvp or p)end
local render_env={read=function(fn)if not scope then error("scope",0)end;local value=fn({pawn=host,world=render_world});if not scope then error("scope",0)end;return value end,
    guard=function()if not scope then error("scope",0)end end,token_valid=function()return scope end,weapon=function()return live_weapon end}
local native_bindings={weapons={{id=1,address=200,name="LiveWeapon",field="Weapon R"}}}
-- Offline original-identity protocol mock. Native slot/flag/class semantics are
-- tested independently in Rust; this validates the production Lua call shape.
local scope_rows={}
local function snapshot(o)
    if not scope or o.GetWorld():GetAddress()~=1 or o.HasAnyFlags(0x40000000)then error("original native world/garbage changed",0)end
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
end}
local render_phases={}
render_env.phase=function(stage,edge,detail)
    if stage=="render_capture"and edge=="enter"then scope_rows={}end
    render_phases[#render_phases+1]={stage=stage,edge=edge,detail=detail}
end
mesh_return_calls=0
local rendered=Render.capture(render_env,native_bindings)
local render_stats=render_phases[#render_phases].detail
T.check(render_phases[1].stage=="render_capture"and render_phases[1].edge=="enter"
    and render_phases[#render_phases].stage=="render_capture"and render_stats.ok==true,"rare render scope has entry before native class lookup and exit after complete binding harvest")
T.check(render_stats.mesh_census_calls==mesh_return_calls and render_stats.mesh_census_calls==4
    and render_stats.component_reads>120 and render_stats.qualifications>render_stats.component_reads,
    "two owners use exactly begin/end full mesh censuses regardless of full bone getter count")
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
T.check(T.eq(rendered.components[1].hidden_bones[1],bone_names[40]),"native hidden render bone retained")
T.check(T.eq(rendered.components[3].parent,1),"native weapon attachment binds captured source body")
T.check(T.eq(rendered.bindings[3].owner,200),"ephemeral binding uses actual owner address rather than logical wire id")
T.check(T.eq(rendered.components[1].parent,2),"native body preserves actual capsule root rather than actor-root shortcut")
T.check(T.eq(rendered.components[2].scene.type,"hidden_capsule"),"root eligibility follows actual native hidden flag")
T.check(T.eq(rendered.topology.vertex_state,"captured"),"source vertex readiness follows actual complete getter data")
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
local spline_ok,spline_reason=pcall(Render.capture,render_env,native_bindings)
T.check(not spline_ok and spline_reason:find('draw_debug_type=boolean draw_debug=true',1,true)
    and spline_reason:find('visible=true hidden=false',1,true) and spline_reason:find('class="/Script/Engine.SplineComponent"',1,true)
    and spline_reason:find('owner_address=0xA owner_name="Pawn" root="CapsuleRoot"',1,true),
    "native debug drawn spline refuses with exact source class/owner/root/visibility facts")
T.check(render_phases[#render_phases].stage=="scene_eligibility"and render_phases[#render_phases].detail.ok==false
    and render_phases[#render_phases].detail.reason==spline_reason,"rare spline flag refusal is emitted as bounded scalar phase evidence")
spline.bHiddenInGame=true
spline_ok,spline_reason=pcall(Render.capture,render_env,native_bindings)
T.check(not spline_ok and spline_reason:find('draw_debug=true visible=true hidden=true',1,true),
    "hidden native debug spline remains refused pending actual profile/rendering proof")
spline.bHiddenInGame=false;spline.IsVisible=function()return false end
spline_ok,spline_reason=pcall(Render.capture,render_env,native_bindings)
T.check(not spline_ok and spline_reason:find('draw_debug=true visible=false hidden=false',1,true),
    "invisible native debug spline records source visibility without silently changing eligibility")
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
host.bHidden=true;spline.bDrawDebug=true
spline_ok,spline_reason=pcall(Render.capture,render_env,native_bindings)
local persisted_reason=spline_reason:sub(1,512)
T.check(not spline_ok and #spline_reason>512 and persisted_reason:find('draw_debug_type=boolean draw_debug=true visible=true hidden=false owner_hidden=true',1,true)
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
host.K2_GetComponentsByClass=function()return {body}end;live_weapon.K2_GetComponentsByClass=function()return {weapon_mesh}end
T.check(pcall(Render.capture,render_env,native_bindings),"direct returned UObject entries never receive parameter get")
host.K2_GetComponentsByClass=function()return {[2]=wrapped(body)}end
T.check(not pcall(Render.capture,render_env,native_bindings),"malformed native component return table refuses capture")
host.K2_GetComponentsByClass=source_mesh_return;live_weapon.K2_GetComponentsByClass=function()return {wrapped(weapon_mesh)}end
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
