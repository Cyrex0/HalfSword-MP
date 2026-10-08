local D=dofile("mods/HSMPMatch/Scripts/native_source_descriptor.lua")
local Adapter=dofile("mods/HSMPMatch/Scripts/native_source_adapter.lua")
local function plain(v)if type(v)~="table"then return v end;local out={};for k,x in pairs(v)do out[k]=plain(x)end;return out end
local function fixture(path)local f=assert(io.open(path,"rb"));local s=f:read("*a");f:close();return plain(T.json_decode(s))end
local recipe=fixture("tools/hsmp-tools/lua-tests/fixtures/native_source_recipe.json")
local armor=fixture("tools/hsmp-tools/lua-tests/fixtures/native_armor_passport.json")
-- Native ForEach passes GetParam wrappers for the actual hard enum/bool/struct
-- inner properties. These fixtures reproduce that documented wrapper contract.
local function wrapped(v)return {get=function()return v end}end
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
T.eq(#D.FIELDS.character,11,"all eleven native character fields captured")
T.eq(#D.FIELDS.armor,24,"native armor includes sparse SlotsBlocked")
T.eq(#D.FIELDS.weapon,25,"all native weapon fields captured")
local source,why=D.capture(env)
T.check(source~=nil,"complete synthetic source captures: "..tostring(why))
T.eq(source.passport.height,recipe.passport.height,"native character double never rounded")
T.eq(source.equipment.armor[1].passport.price,armor.price,"native armor price double preserved")
T.eq(source.equipment.armor[1].passport.slots_blocked[1].value,false,"explicit false blocked slot preserved")
T.eq(source.equipment.armor[1].passport.slots_blocked[2].slot,5,"sparse blocked-slot keys preserved")
T.eq(#source.passport.equipment.armor,0,"consumed construction armor does not replace live armor")
native_armor[D.FIELDS.armor[2][1]]=123
T.eq(source.equipment.armor[1].passport.id,armor.id,"captured passport is detached from native storage")
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
local adapter=Adapter.new({WG=WG,resolve=function(index)return {index=index,world_key="fixture",pc_address=9,pc_name="PC",pawn_address=current_address,pawn_name="Fixture",pawn=pawn,world={GetAddress=function()return 1 end}}end,
    capture_render=function(_,bindings)return {components=recipe.components,bindings={{id=1,address=11,owner=0}},
        topology={detached={},gore={},vertex_state="unavailable"}}end})
local actual,bindings=adapter.capture(0)
T.check(actual~=nil,"actual source adapter entry captures typed native-style fields")
T.eq(actual.team,2,"actual source Team Int copied without assumed team")
T.eq(actual.topology.vertex_state,"unavailable","unverified vertex state remains incomplete")
T.eq(bindings.pawn,10,"engine addresses remain in separate local binding table")
T.check(actual.pawn==nil,"engine address never enters source recipe")
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
    colors=function()return color_array(colors)end,unwrap=function(v)return v:get()end}
local lods=Vertex.capture(vertex_env)
T.check(lods~=nil and #lods[1].runs==2,"native color RLE preserves complete source ordering")
T.eq(lods[1].runs[1].count,2,"identical native colors compact exactly")
T.eq(lods[1].runs[1].color[3],128,"native RGBA8 channels never gamma-converted")
T.eq(lods[1].runs[1].color[4],0,"native cut alpha zero preserved")
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
local function object(address,n,path,transient)
    return {GetAddress=function()return address end,IsValid=function()return true end,GetFName=function()return fname(n)end,
        GetFullName=function()return "FixtureClass "..path end,HasAnyFlags=function()return transient end}
end
local saved_find,saved_fname=StaticFindObject,FName
FName=function(n)return n end
scope=true
local root=object(100,"CapsuleRoot","/Game/Test/Root.Root",false)
local host=object(10,"Pawn","/Game/Test/Pawn.Pawn",false);host.RootComponent=root
local live_weapon=object(200,"LiveWeapon","/Game/Test/WeaponActor.WeaponActor",false)
local body_asset=object(300,"Body","/Game/Test/Body.Body",false)
body_asset.Skeleton=object(301,"Skeleton","/Game/Test/Skeleton.Skeleton",false)
body_asset.GetOverlayMaterial=function()return nil end;body_asset.GetDefaultMeshDeformer=function()return nil end
body_asset.MeshClothingAssets=array({});body_asset.GetMorphTargetsPtrConv=function()return array({object(302,"ExactMorph","/Game/Test/Morph.Morph",false)})end
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
local function mesh(address,n,actor,kind,asset)
    local c=object(address,n,"/Game/Test/Runtime."..n,false)
    c.GetOwner=function()return actor end;c.IsA=function(_,k)return k=="/Script/Engine."..(kind=="skeletal"and "SkeletalMeshComponent"or "StaticMeshComponent")end
    c.GetRelativeTransform=function()return {Translation={X=0.125,Y=0.25,Z=0.5},Rotation={X=0,Y=0,Z=0,W=1},Scale3D={X=1,Y=1,Z=1}}end
    c.IsVisible=function()return true end;c.bHiddenInGame=false;c.GetAttachSocketName=function()return fname("None")end
    c.GetCollisionResponseToChannel=function(_,channel)return channel%3 end;c.GetCollisionEnabled=function()return 3 end
    c.GetCollisionObjectType=function()return 3 end;c.GetCollisionProfileName=function()return fname("ExactNativeProfile")end;c.IsSimulatingPhysics=function()return true end
    c.GetSkeletalMeshAsset=function()return asset end;c.StaticMesh=asset;c.GetPhysicsAsset=function()return physical_asset end
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
host.K2_GetComponentsByClass=function()return array({body})end;live_weapon.K2_GetComponentsByClass=function()return array({weapon_mesh})end
local plain_component_getter_calls=0
local function unavailable_component_alias()plain_component_getter_calls=plain_component_getter_calls+1;error("unreflected GetComponentsByClass alias",0)end
host.GetComponentsByClass=unavailable_component_alias;live_weapon.GetComponentsByClass=unavailable_component_alias
local rvp={IsValid=function()return true end,GetMeshComponentAmountOfVerticesOnLOD=function(_,c)return #c.native_colors end,
    GetMeshComponentVertexColorsAtLOD_Wrapper=function(_,c)return color_array(c.native_colors)end}
StaticFindObject=function(p)return p=="/Script/VertexPaintDetectionPlugin.Default__VertexPaintFunctionLibrary"and rvp or p end
local render_env={read=function(fn)if not scope then error("scope",0)end;local value=fn({pawn=host});if not scope then error("scope",0)end;return value end,
    guard=function()if not scope then error("scope",0)end end,token_valid=function()return scope end,weapon=function()return live_weapon end}
local native_bindings={weapons={{id=1,address=200,name="LiveWeapon",field="Weapon R"}}}
local rendered=Render.capture(render_env,native_bindings)
T.eq(plain_component_getter_calls,0,"source census uses exact reflected K2 component getter")
T.check(#rendered.components==2 and #rendered.components[1].bones==40,"default source collector keeps complete render dictionary beyond physical bones")
T.eq(rendered.components[1].materials[1].scalars[1].value,0.3125,"native scalar material override captured exactly")
T.eq(rendered.components[1].materials[1].vectors[1].value[4],0.0,"native vector alpha zero captured exactly")
T.eq(rendered.components[1].materials[1].textures[1].value,"/Game/Test/Texture.Texture","native texture exact full identity preserved")
T.eq(rendered.components[1].hidden_bones[1],bone_names[40],"native hidden render bone retained")
T.eq(rendered.components[2].parent,1,"native weapon attachment binds captured source body")
T.eq(rendered.bindings[2].owner,200,"ephemeral binding uses actual owner address rather than logical wire id")
T.eq(rendered.topology.vertex_state,"captured","source vertex readiness follows actual complete getter data")
local bones_after_loss=0;body.GetPhysicsAsset=function()scope=false;return physical_asset end
body.GetNumBones=function()bones_after_loss=bones_after_loss+1;return #bone_names end
T.check(not pcall(Render.capture,render_env,native_bindings) and bones_after_loss==0,"world loss inside render getter prevents next source operation")
scope=true;body.GetPhysicsAsset=function()return physical_asset end
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
T.eq(transient_render.components[1].geometry,"runtime_transient","RF_Transient does not invent an observed merge recipe")
body_asset.HasAnyFlags=function()return nil end
T.check(not pcall(Render.capture,render_env,native_bindings),"missing native transient flag refuses rather than assumes cooked geometry")
body_asset.HasAnyFlags=function()return true end;body.IsBoneHiddenByName=function()return nil end
T.check(not pcall(Render.capture,render_env,native_bindings),"missing native bone visibility refuses rather than assumes visible")
body.IsBoneHiddenByName=function()return false end;body.ComponentHasTag=function()return nil end
T.check(not pcall(Render.capture,render_env,native_bindings),"missing native component tags refuse rather than erase persistent parts")
StaticFindObject,FName=saved_find,saved_fname
