-- Rare complete render-component metadata harvest. Full per-frame poses and
-- changing material/morph values are sampled in the native bulk API, not here.
-- Native RVP AtLOD getters copy exact hard FColor arrays; unsupported kinds
-- retain an explicit incomplete vertex state.
local src=(debug.getinfo(1,"S").source or ""):gsub("^@","")
local Vertex=dofile((src:match("^(.*)[/\\]") or ".").."/native_source_vertex.lua")
local Array=dofile((src:match("^(.*)[/\\]") or ".").."/native_source_array.lua")
local M={}
local kinds={skeletal="/Script/Engine.SkeletalMeshComponent",static="/Script/Engine.StaticMeshComponent",
    groom="/Script/HairStrandsCore.GroomComponent",procedural="/Script/ProceduralMeshComponent.ProceduralMeshComponent"}
local material_flags={"bOverride_OpacityMaskClipValue","bOverride_BlendMode","bOverride_ShadingModel","bOverride_DitheredLODTransition",
    "bOverride_CastDynamicShadowAsMasked","bOverride_TwoSided","bOverride_bIsThinSurface","bOverride_OutputTranslucentVelocity",
    "bOverride_bHasPixelAnimation","bOverride_bEnableTessellation","bOverride_DisplacementScaling","bOverride_MaxWorldPositionOffsetDisplacement"}
local group_fields={
    {"HairLength","hair_length"},{"HairWidth","hair_width"},{"HairWidth_Override","hair_width_override"},
    {"HairRootScale","root_scale"},{"HairRootScale_Override","root_scale_override"},{"HairTipScale","tip_scale"},{"HairTipScale_Override","tip_scale_override"},
    {"HairShadowDensity","shadow_density"},{"HairShadowDensity_Override","shadow_density_override"},
    {"HairRaytracingRadiusScale","raytracing_radius_scale"},{"HairRaytracingRadiusScale_Override","raytracing_radius_scale_override"},
    {"bUseHairRaytracingGeometry","use_raytracing_geometry"},{"bUseHairRaytracingGeometry_Override","use_raytracing_geometry_override"},
    {"LODBias","lod_bias"},{"bUseStableRasterization","stable_rasterization"},{"bUseStableRasterization_Override","stable_rasterization_override"},
    {"bScatterSceneLighting","scatter_scene_lighting"},{"bScatterSceneLighting_Override","scatter_scene_lighting_override"},
    {"bSupportVoxelization","support_voxelization"},{"bSupportVoxelization_Override","support_voxelization_override"},
    {"HairLengthScale","length_scale"},{"HairLengthScale_Override","length_scale_override"}}
local function fail(s)error(s,0)end
function M.capture(env,bindings)
    local read,guard=env.read,env.guard
    if not env.scope or not env.scope.keep or not env.scope.resolve then fail("native source identity scope unavailable")end
    local stats={mesh_census_calls=0,component_reads=0,parent_hops=0,qualifications=0}
    local function phase(stage,edge,detail,row)
        if not env.phase then return end
        detail=detail or {}
        if row then detail.address=row.address;detail.name=row.name;detail.owner_id=row.owner;detail.component_id=row.id end
        env.phase(stage,edge,detail)
    end
    phase("render_capture","enter",{getter="StaticFindObject(render classes)"})
    local function checked(fn)guard();local v=fn();guard();return v end
    local function array(a,max,convert,producer,context)
        return Array.collect(a,max,producer or "property",{guard=guard,context=context},convert)
    end
    local function path(o)
        if o==nil then return "",false end
        if checked(function()return o:GetAddress()end)==0 then return "",false end
        if checked(function()return o:IsValid()end)~=true then fail("native render asset invalid")end
        local full=checked(function()return o:GetFullName()end)
        local p=type(full)=="string" and full:match("^%S+%s+(.+)$")
        if not p or #p>512 then fail("native render asset path unavailable")end
        local transient=checked(function()return o:HasAnyFlags(0x40)end) -- native RF_Transient
        if type(transient)~="boolean"then fail("native render asset transient flag unavailable")end
        return p,transient
    end
    local function name(v)return type(v)=="string" and v or checked(function()return v:ToString()end)end
    local function object_id(o)
        if o==nil then return nil end
        local address=checked(function()return o:GetAddress()end);if address==0 then return nil end
        if checked(function()return o:IsValid()end)~=true then fail("native render object invalid")end
        return {address=address,name=checked(function()return o:GetFName():ToString()end)}
    end
    local classes={}
    for kind,p in pairs(kinds)do classes[kind]=checked(function()return StaticFindObject(p)end)end
    local mesh_class=checked(function()return StaticFindObject("/Script/Engine.MeshComponent")end)
    local mi_class=checked(function()return StaticFindObject("/Script/Engine.MaterialInstance")end)
    local rvp=checked(function()return StaticFindObject("/Script/VertexPaintDetectionPlugin.Default__VertexPaintFunctionLibrary")end)
    if not mesh_class or not mi_class then fail("native render classes unavailable")end
    local world_address=read(function(b)return b.world:GetAddress()end)
    local root_ids={}
    local function owner(row)
        local actor
        if row.owner==0 then actor=read(function(b)return b.pawn end)
        else for _,w in ipairs(bindings.weapons)do if w.id==row.owner then actor=env.weapon(w.field,w);break end end end
        if not actor then fail("native render owner unavailable")end
        local root=root_ids[row.owner]
        if root then
            local actual=object_id(checked(function()return actor.RootComponent end))
            if not actual or actual.address~=root.address or actual.name~=root.name then fail("native source owner root changed")end
        end
        return actor
    end
    local rows,by_address,mesh_sets,owner_ids={}, {}, {}, {}
    local owners={{owner=0}};for _,w in ipairs(bindings.weapons)do owners[#owners+1]={owner=w.id}end
    local function runtime_path(c)
        -- GetFullName is direct metadata. Preserve the complete runtime outer
        -- path, including the level/actor ':' separator, for StaticFindObject.
        local full=c:GetFullName() -- direct metadata, no ProcessEvent/callback
        local p=type(full)=="string" and full:match("^%S+%s+(.+)$")
        if not p or #p>512 or p:find("\0",1,true)then fail("native source runtime path unavailable")end
        return p
    end
    local function retain(c,row)
        local p=runtime_path(c)
        local expected_owner=row.owner~=nil and checked(function()return owner(row):GetAddress()end)or 0
        local handle,actual_owner=env.scope.keep({address=row.address,owner=expected_owner,path=p})
        if not handle then fail(actual_owner or "native source identity keep refused")end
        if row.owner==nil then
            for _,candidate in ipairs(owners)do
                if checked(function()return owner(candidate):GetAddress()end)==actual_owner then row.owner=candidate.owner;break end
            end
            if row.owner==nil then fail("native source original ancestor owner unavailable")end
        elseif actual_owner~=expected_owner then fail("native source original component owner changed")end
        row.handle,row.path=handle,p
        return row
    end
    local function qualify(c,row)
        stats.qualifications=stats.qualifications+1
        if not row.handle then retain(c,row)end
        local address,reason=env.scope.resolve(row.handle)
        if address~=row.address then fail(reason or "native source original component changed")end
        -- Acquire a new wrapper only after original native identity admission.
        -- No wrapper from an earlier ProcessEvent is used for a field getter.
        c=StaticFindObject(row.path)
        if not c or c:GetAddress()~=row.address or c:IsValid()~=true or c:GetFName():ToString()~=row.name then fail("native render component identity changed")end
        if runtime_path(c)~=row.path then fail("native source original component runtime path changed")end
        return c
    end
    local function include(row)
        local previous=by_address[row.address]
        if previous then
            if previous.name~=row.name or previous.owner~=row.owner then fail("native source component binding changed")end
            return previous
        end
        by_address[row.address]=row;rows[#rows+1]=row
        if #rows>64 then fail("native complete attachment component bound")end
        return row
    end
    local function collect(actor,owner_id)
        local actor_id=object_id(actor)
        if not actor_id then fail("native mesh owner unavailable")end
        owner_ids[owner_id]={address=actor_id.address,name=actor_id.name}
        local set={};mesh_sets[owner_id]=set
        phase("mesh_census","enter",{getter="Actor.K2_GetComponentsByClass(MeshComponent)",owner_id=owner_id,name=actor_id.name,address=actor_id.address})
        stats.mesh_census_calls=stats.mesh_census_calls+1
        local a=checked(function()return actor:K2_GetComponentsByClass(mesh_class)end)
        local copied=array(a,64,function(c)
            local id=object_id(c)
            if not id or set[id.address]or by_address[id.address]then fail("native mesh component identity duplicate or unavailable")end
            id.owner=owner_id;id.mesh=true
            c=qualify(c,id)
            if c:IsA(mesh_class)~=true then fail("native mesh return class changed")end
            set[id.address]=id;include(id)
            return true
        end,"return","Actor.K2_GetComponentsByClass(MeshComponent) collect owner="..actor_id.name)
        phase("mesh_census","exit",{ok=true,count=#copied,owner_id=owner_id,name=actor_id.name,address=actor_id.address})
    end
    collect(read(function(b)return b.pawn end),0)
    for _,w in ipairs(bindings.weapons)do collect(env.weapon(w.field,w),w.id)end
    local function component(row)
        return qualify(nil,row)
    end
    local function get(row,fn)
        stats.component_reads=stats.component_reads+1
        guard();local c=component(row);local value=fn(c);c=nil;guard();component(row);return value
    end
    local function vec(v,keys)
        local out={};for i,k in ipairs(keys)do out[i]=checked(function()return v[k]end);if type(out[i])~="number"then fail("native render vector unavailable")end end
        return out
    end
    local function attachment_refusal(row,parent,root)
        -- Evidence only: an unseen native parent never becomes a guessed root.
        -- Resolve the original child and its parent again before each getter.
        local function fresh_parent()
            local p=get(row,function(o)return o:GetAttachParent()end)
            local identity=object_id(p)
            if not identity or identity.address~=parent.address or identity.name~=parent.name then fail("native attachment parent changed during evidence")end
            return p
        end
        local function parent_read(fn)
            guard();local value=fn(fresh_parent());guard();fresh_parent();return value
        end
        local function bounded(value,max)
            local s=tostring(value);return #s<=max and s or s:sub(1,max).."[truncated]"
        end
        local function identity_text(identity)
            if not identity then return "null"end
            return string.format("0x%X",identity.address)..":"..string.format("%q",bounded(identity.name,96))
        end
        local function class_name(o)return o:GetClass():GetFullName()end
        local world_address=read(function(b)return b.world:GetAddress()end)
        local parent_world=parent_read(function(o)return o:GetWorld()end)
        if not parent_world or checked(function()return parent_world:IsValid()end)~=true
            or checked(function()return parent_world:GetAddress()end)~=world_address then fail("native attachment parent world changed during evidence")end
        local child_owner=object_id(get(row,function(o)return o:GetOwner()end))
        local parent_owner=object_id(parent_read(function(o)return o:GetOwner()end))
        local child_class=get(row,class_name)
        local parent_class=parent_read(class_name)
        local parent_root=object_id(parent_read(function(o)return o:GetOwner().RootComponent end))
        local ancestor=object_id(parent_read(function(o)return o:GetAttachParent()end))
        local socket=name(parent_read(function(o)return o:GetAttachSocketName()end))
        local function transform_text(v)
            local values={}
            for _,part in ipairs({{"Translation",{"X","Y","Z"}},{"Rotation",{"X","Y","Z","W"}},{"Scale3D",{"X","Y","Z"}}})do
                for _,key in ipairs(part[2])do
                    local n=checked(function()return v[part[1]][key]end)
                    if type(n)~="number" or n~=n or math.abs(n)==math.huge then fail("native attachment transform unavailable")end
                    values[#values+1]=string.format("%.17g",n)
                end
            end
            return table.concat(values,",")
        end
        local relative=transform_text(parent_read(function(o)return o:GetRelativeTransform()end))
        local world=transform_text(parent_read(function(o)return o:K2_GetComponentToWorld()end))
        fresh_parent()
        return "native render attachment parent not captured"
            .." child="..identity_text(row).." parent="..identity_text(parent)
            .." owner="..identity_text(child_owner).." root="..identity_text(root)
            .." parent_owner="..identity_text(parent_owner).." parent_root="..identity_text(parent_root)
            .." ancestor="..identity_text(ancestor).." socket="..string.format("%q",bounded(socket,96))
            .." world="..string.format("0x%X",world_address)
            .." child_class="..bounded(child_class,160).." parent_class="..bounded(parent_class,160)
            .." parent_relative_p3q4s3="..relative.." parent_world_p3q4s3="..world
    end
    -- Only mesh seeds are enumerated. Every real owner root and every actual
    -- parent is included through hard links and its original native identity.
    -- Complete mesh censuses occur at harvest boundaries, not every getter.
    local function include_root(actor,owner_id)
        local identity=object_id(checked(function()return actor.RootComponent end))
        if not identity then fail("native actor root unavailable")end
        identity.owner=owner_id;qualify(checked(function()return actor.RootComponent end),identity)
        root_ids[owner_id]={address=identity.address,name=identity.name}
        identity.mesh=checked(function()return qualify(actor.RootComponent,identity):IsA(mesh_class)end)
        if type(identity.mesh)~="boolean"then fail("native source root kind unavailable")end
        if identity.mesh and not by_address[identity.address]then fail("native complete mesh census changed")end
        include(identity)
    end
    phase("parent_closure","enter",{getter="Actor.RootComponent/SceneComponent.GetAttachParent"})
    include_root(read(function(b)return b.pawn end),0)
    for _,weapon in ipairs(bindings.weapons)do include_root(env.weapon(weapon.field,weapon),weapon.id)end
    local cursor=1
    while cursor<=#rows do
        local row=rows[cursor]
        local parent_object=get(row,function(o)stats.parent_hops=stats.parent_hops+1;return o:GetAttachParent()end)
        local parent=object_id(parent_object)
        row.parent_address=parent and parent.address or 0
        if parent then
            retain(parent_object,parent) -- unknown owner is discovered in native scope
            parent.mesh=checked(function()return qualify(nil,parent):IsA(mesh_class)end)
            if type(parent.mesh)~="boolean"then fail("native source ancestor kind unavailable")end
            if parent.mesh and not by_address[parent.address]then fail("native complete mesh census changed")end
            include(parent)
        end
        cursor=cursor+1
    end
    for _,row in ipairs(rows)do
        local seen={};local address=row.address
        while address~=0 do
            if seen[address]then fail("native attachment cycle")end;seen[address]=true
            local ancestor=by_address[address]
            if not ancestor then fail("native attachment closure changed")end
            address=ancestor.parent_address
        end
    end
    table.sort(rows,function(a,b)if a.owner~=b.owner then return a.owner<b.owner end;return a.name<b.name end)
    for i,row in ipairs(rows)do row.id=i end
    phase("parent_closure","exit",{ok=true,count=#rows})
    local function material(row,slot)
        local scalar,vector,texture={},{},{}
        local current=get(row,function(c)return c:GetMaterial(slot)end)
        local base,transient=path(current);local depth=0
        while transient do
            depth=depth+1;if depth>16 or checked(function()return current:IsA(mi_class)end)~=true then fail("native dynamic material parent incomplete")end
            for _,flag in ipairs(material_flags)do
                local value=checked(function()return current.BasePropertyOverrides[flag]end)
                if type(value)~="boolean"then fail("native material base override unavailable")end
                if value then fail("native material base override unsupported")end
            end
            for _,field in ipairs({"DoubleVectorParameterValues","FontParameterValues","RuntimeVirtualTextureParameterValues","SparseVolumeTextureParameterValues"})do
                local values=checked(function()return current[field]end)
                if not values or checked(function()return values:GetArrayNum()end)~=0 then fail("native material parameter type unsupported")end
            end
            local function parameters(field,target,kind)
                array(checked(function()return current[field]end),128,function(v)
                    local p=checked(function()return v.ParameterInfo end)
                    local info={name=name(checked(function()return p.Name end)),association=checked(function()return p.Association end),index=checked(function()return p.Index end)}
                    local key=info.name..":"..tostring(info.association)..":"..tostring(info.index)
                    local value=checked(function()return v.ParameterValue end)
                    if target[key]==nil then
                        if kind=="color"then value=vec(value,{"R","G","B","A"})
                        elseif kind=="texture"then value=path(value)end
                        target[key]={info=info,value=value}
                    end
                    return true
                end,"property","MaterialInstance."..field.." component="..row.name.." slot="..tostring(slot))
            end
            parameters("ScalarParameterValues",scalar,"num");parameters("VectorParameterValues",vector,"color");parameters("TextureParameterValues",texture,"texture")
            current=checked(function()return current.Parent end)
            base,transient=path(current)
        end
        if base==""then fail("native material base missing")end
        local function ordered(t)local out={};for _,v in pairs(t)do out[#out+1]=v end;table.sort(out,function(a,b)
            if a.info.name~=b.info.name then return a.info.name<b.info.name end
            if a.info.association~=b.info.association then return a.info.association<b.info.association end
            return a.info.index<b.info.index end);return out end
        return {slot=slot,base=base,scalars=ordered(scalar),vectors=ordered(vector),textures=ordered(texture)}
    end
    local components,detached,gore={}, {}, {}
    local vertex_total=0
    local main_address=read(function(b)return b.pawn.Mesh:GetAddress()end)
    for _,row in ipairs(rows)do
        phase("component_static","enter",{getter="SourceRender.component metadata"},row)
        local kind;if not row.mesh then kind="scene"end
        for _,k in ipairs({"skeletal","static","groom","procedural"})do
            if classes[k] and get(row,function(c)return c:IsA(classes[k])end)==true then kind=k;break end
        end
        local class_full=get(row,function(o)return o:GetClass():GetFullName()end)
        local component_class=type(class_full)=="string" and class_full:match("^%S+%s+(.+)$")
        if not component_class then fail("native component class unavailable")end
        if not kind then fail("native render component kind unsupported class="..component_class:sub(1,192).." component="..row.name:sub(1,96))end
        local relative=get(row,function(c)return c:GetRelativeTransform()end)
        local c={id=row.id,owner=row.owner,name=row.name,component_class=component_class,kind=kind,scene={type="not_applicable"},
            role=kind=="scene" and "anchor" or row.address==main_address and "body" or row.owner~=0 and "weapon" or "attachment",
            relative={translation=vec(relative.Translation,{"X","Y","Z"}),rotation=vec(relative.Rotation,{"X","Y","Z","W"}),scale=vec(relative.Scale3D,{"X","Y","Z"})},
            visible=get(row,function(o)return o:IsVisible()end),hidden=get(row,function(o)return o.bHiddenInGame end),
            socket=name(get(row,function(o)return o:GetAttachSocketName()end)),bones={},materials={},morphs={},hidden_bones={},groom={},
            vertex_state="unavailable",vertex_colors={},deformer="",cloth=false}
        local parent=object_id(get(row,function(o)return o:GetAttachParent()end));c.parent=0
        if (parent and parent.address or 0)~=row.parent_address then fail("native attachment parent changed after closure")end
        if parent then
            for _,p in ipairs(rows)do if p.address==parent.address and p.name==parent.name then c.parent=p.id;break end end
            if c.parent==0 then fail("native complete attachment parent missing")end
        end
        if type(c.visible)~="boolean" or type(c.hidden)~="boolean"then fail("native component visibility unavailable")end
        if kind=="scene"then
            if component_class=="/Script/Engine.SceneComponent"then c.scene={type="scene"};c.collision=false
            elseif component_class=="/Script/Engine.SplineComponent"then
                phase("scene_eligibility","enter",{getter="SplineComponent.bDrawDebug",class=component_class},row)
                local draw_debug=get(row,function(o)return o.bDrawDebug end)
                if type(draw_debug)~="boolean" or draw_debug then
                    -- Record actual source flags, never infer editor-only
                    -- absence or a hidden state from the NullRHI process.
                    -- HasAnyFlags and bHidden are direct native metadata;
                    -- get/checked still qualify the original source around it.
                    local component_transient=get(row,function(o)return o:HasAnyFlags(0x40)end)
                    local component_garbage=get(row,function(o)return o:HasAnyFlags(0x40000000)end)
                    local owner_transient=checked(function()return owner(row):HasAnyFlags(0x40)end)
                    local owner_garbage=checked(function()return owner(row):HasAnyFlags(0x40000000)end)
                    local owner_hidden=checked(function()return owner(row).bHidden end)
                    local function scalar(value)
                        return type(value)=="boolean"and tostring(value)or "unavailable("..type(value)..")"
                    end
                    local function label(value)
                        local s=tostring(value or "");if #s>80 then s=s:sub(1,80).."[truncated]"end;return string.format("%q",s)
                    end
                    local actual_owner=owner_ids[row.owner];local actual_root=root_ids[row.owner]
                    if not actual_owner or not actual_root then fail("native spline original owner/root evidence unavailable")end
                    -- The worker persists only the first512 reason bytes.
                    -- Decision-critical actual flags precede all long labels.
                    local reason="native spline anchor rendering not proved absent: draw_debug_type="..type(draw_debug).." draw_debug="..scalar(draw_debug)
                        .." visible="..scalar(c.visible).." hidden="..scalar(c.hidden)
                        .." owner_hidden="..scalar(owner_hidden)
                        .." component_transient="..scalar(component_transient).." component_garbage="..scalar(component_garbage)
                        .." owner_transient="..scalar(owner_transient).." owner_garbage="..scalar(owner_garbage)
                        .." name="..label(row.name).." class="..label(component_class).." address="..string.format("0x%X",row.address)
                        .." owner_id="..tostring(row.owner).." owner_address="..string.format("0x%X",actual_owner.address)
                        .." owner_name="..label(actual_owner.name).." root="..label(actual_root.name)
                    phase("scene_eligibility","exit",{getter="SplineComponent.bDrawDebug",class=component_class,ok=false,reason=reason},row)
                    fail(reason)
                end
                phase("scene_eligibility","exit",{getter="SplineComponent.bDrawDebug",class=component_class,ok=true,
                    reason="draw_debug_type=boolean draw_debug=false visible="..tostring(c.visible).." hidden="..tostring(c.hidden)},row)
                c.scene={type="spline",draw_debug=draw_debug}
            elseif component_class=="/Script/Engine.CapsuleComponent"then
                if c.visible and not c.hidden then fail("native capsule anchor rendering not proved absent: "..row.name)end
                c.scene={type="hidden_capsule"}
            else fail("native scene anchor class unsupported: "..component_class)end
            c.asset="";c.skeleton="";c.physics_asset="";c.geometry="not_applicable";c.vertex_state="not_applicable"
        end
        if c.collision~=false then
        local responses={};for channel=0,31 do responses[channel+1]=get(row,function(o)return o:GetCollisionResponseToChannel(channel)end)end
        c.collision={enabled=get(row,function(o)return o:GetCollisionEnabled()end),object_type=get(row,function(o)return o:GetCollisionObjectType()end),
            profile=name(get(row,function(o)return o:GetCollisionProfileName()end)),responses=responses,simulating=get(row,function(o)return o:IsSimulatingPhysics(FName("None"))end)}
        end
        if kind~="scene"then
        local asset_obj
        if kind=="skeletal"then asset_obj=get(row,function(o)return o:GetSkeletalMeshAsset()end)
        elseif kind=="static"then asset_obj=get(row,function(o)return o.StaticMesh end)
        elseif kind=="groom"then asset_obj=get(row,function(o)return o.GroomAsset end)end
        local transient;c.asset,transient=path(asset_obj)
        -- RF_Transient is evidence of a runtime asset, not a specific merge.
        c.geometry=kind=="procedural" and "procedural" or transient and "runtime_transient" or "cooked"
        c.skeleton="";c.physics_asset=""
        if kind=="skeletal"then
            local asset_identity=object_id(asset_obj)
            if not asset_identity then fail("native skeletal asset unavailable")end
            local function current_asset()
                local actual=get(row,function(o)return o:GetSkeletalMeshAsset()end)
                local identity=object_id(actual)
                if not identity or identity.address~=asset_identity.address or identity.name~=asset_identity.name then fail("native skeletal asset changed")end
                return actual
            end
            local function asset_read(fn)
                guard();local value=fn(current_asset());guard();current_asset();return value
            end
            if path(asset_read(function(o)return o:GetOverlayMaterial()end))~="" then fail("native asset overlay material unsupported")end
            if path(asset_read(function(o)return o:GetDefaultMeshDeformer()end))~="" then fail("native asset mesh deformer unsupported")end
            c.skeleton=path(asset_read(function(o)return o.Skeleton end))
            local override=get(row,function(o)return o.PhysicsAssetOverride end)
            local override_identity=object_id(override)
            c.physics_asset=path(override_identity and override or asset_read(function(o)return o:GetPhysicsAsset()end))
            local after_override=object_id(get(row,function(o)return o.PhysicsAssetOverride end))
            if (override_identity==nil)~=(after_override==nil) or (override_identity and
                (override_identity.address~=after_override.address or override_identity.name~=after_override.name))then fail("native physics asset override changed")end
            current_asset()
            c.deformer=path(get(row,function(o)return o.MeshDeformer end))
            if object_id(get(row,function(o)return o:GetMeshDeformerInstance()end)) then fail("native mesh deformer instance unsupported")end
            local weight_profile=get(row,function(o)return o:IsUsingSkinWeightProfile()end)
            if type(weight_profile)~="boolean"then fail("native skin weight profile unavailable")end
            if weight_profile then fail("native skin weight profile unsupported")end
            for _,field in ipairs({"bHideSkin","bDisableMorphTarget","bForceWireframe"})do
                local flag=get(row,function(o)return o[field]end)
                if type(flag)~="boolean"then fail("native render flag unavailable: "..field)end
                if flag then fail("native render flag unsupported: "..field)end
            end
            local disable_cloth=get(row,function(o)return o.bDisableClothSimulation end)
            local clothing=asset_read(function(o)return o.MeshClothingAssets end)
            if type(disable_cloth)~="boolean" or not clothing then fail("native cloth state unavailable")end
            c.cloth=not disable_cloth and checked(function()return clothing:GetArrayNum()end)>0
            phase("bone_dictionary","enter",{getter="GetNumBones/GetBoneName/GetParentBone/IsBoneHiddenByName",class=component_class},row)
            local count=get(row,function(o)return o:GetNumBones()end)
            if type(count)~="number" or not math.tointeger(count) or count<1 or count>512 then fail("native complete bone count unavailable")end
            local names={};for i=0,count-1 do names[i+1]=name(get(row,function(o)return o:GetBoneName(i)end))end
            local index={};for i,n in ipairs(names)do index[n]=i-1 end
            for _,n in ipairs(names)do
                local parent_name=name(get(row,function(o)return o:GetParentBone(FName(n))end))
                local parent_index=parent_name=="None" and -1 or index[parent_name]
                if parent_index==nil then fail("native render bone parent incomplete")end
                c.bones[#c.bones+1]={name=n,parent=parent_index}
                local hidden=get(row,function(o)return o:IsBoneHiddenByName(FName(n))end)
                if type(hidden)~="boolean"then fail("native bone visibility unavailable")end
                if hidden then c.hidden_bones[#c.hidden_bones+1]=n end
            end
            phase("bone_dictionary","exit",{ok=true,count=count,class=component_class},row)
            phase("morph_dictionary","enter",{getter="SkeletalMesh.GetMorphTargetsPtrConv/GetMorphTarget",class=component_class},row)
            array(asset_read(function(o)return o:GetMorphTargetsPtrConv()end),128,function(m)
                current_asset();local n=name(checked(function()return m:GetFName()end));c.morphs[#c.morphs+1]={name=n,value=get(row,function(o)return o:GetMorphTarget(FName(n))end)};current_asset();return true end,"return","SkeletalMesh.GetMorphTargetsPtrConv component="..row.name)
            phase("morph_dictionary","exit",{ok=true,count=#c.morphs,class=component_class},row)
        elseif kind=="groom"then
            local groups=array(get(row,function(o)return o.GroomGroupsDesc end),32,function(v)
                local group={};for _,fd in ipairs(group_fields)do group[fd[2]]=checked(function()return v[fd[1]]end);if group[fd[2]]==nil then fail("native groom group incomplete")end end;return group end,"property","GroomComponent.GroomGroupsDesc component="..row.name)
            c.groom={{binding_asset=path(get(row,function(o)return o.BindingAsset end)),source_mesh=path(get(row,function(o)return o.SourceSkeletalMesh end)),
                cache=path(get(row,function(o)return o.GroomCache end)),physics_asset=path(get(row,function(o)return o.PhysicsAsset end)),
                attachment_name=get(row,function(o)return o.AttachmentName:ToString()end),use_cards=get(row,function(o)return o.bUseCards end),
                simulation=get(row,function(o)return o.SimulationSettings.SolverSettings.bEnableSimulation end),groups=groups}}
        end
        if path(get(row,function(o)return o:GetOverlayMaterial()end))~="" then fail("native component overlay material unsupported")end
        local count=get(row,function(o)return o:GetNumMaterials()end)
        if type(count)~="number" or not math.tointeger(count) or count<0 or count>32 then fail("native material slots incomplete")end
        phase("material","enter",{getter="MeshComponent.GetMaterial/MaterialInstance parameters",count=count,class=component_class},row)
        for slot=0,count-1 do c.materials[#c.materials+1]=material(row,slot)end
        phase("material","exit",{ok=true,count=count,class=component_class},row)
        if kind=="skeletal" or kind=="static"then
            if not rvp or checked(function()return rvp:IsValid()end)~=true then fail("native vertex getter unavailable")end
            local colors,why=Vertex.capture({
                guard=env.token_valid or function()guard();return true end,
                phase=function(stage,edge,detail)detail=detail or {};detail.class=component_class;phase(stage,edge,detail,row)end,
                lods=function()return get(row,function(o)return kind=="skeletal" and o:GetNumLODs() or asset_obj:GetNumLODs()end)end,
                count=function(lod)
                    if kind=="skeletal"then
                        local usage=get(row,function(o)return o:GetVertexOffsetUsage(lod)end)
                        if type(usage)~="number" or not math.tointeger(usage)then fail("native vertex offset usage unavailable")end
                        if usage~=0 then fail("native vertex offsets unsupported")end
                        for slot=0,count-1 do
                            local shown=get(row,function(o)return o:IsMaterialSectionShown(slot,lod)end)
                            if type(shown)~="boolean"then fail("native section visibility unavailable")end
                            if not shown then fail("native hidden material section unsupported")end
                        end
                    end
                    return get(row,function(o)return rvp:GetMeshComponentAmountOfVerticesOnLOD(o,lod)end)
                end,
                reserve=function(count)
                    vertex_total=vertex_total+count
                    if vertex_total>1000000 then fail("native vertex expansion bound")end
                end,
                colors=function(lod)return get(row,function(o)return rvp:GetMeshComponentVertexColorsAtLOD_Wrapper(o,lod)end)end,
                array_kind="return", -- known synchronous native TArray<FColor> return
                array_context="VertexPaintFunctionLibrary.GetMeshComponentVertexColorsAtLOD_Wrapper component="..row.name,
            })
            if not colors then fail(why)end
            c.vertex_colors,c.vertex_state=colors,"captured"
        elseif env.vertex_state then c.vertex_state=env.vertex_state(row)end
        end
        local detached_tag=get(row,function(o)return o:ComponentHasTag(FName("Dismembered"))end)
        local gore_tag=get(row,function(o)return o:ComponentHasTag(FName("Gore"))end)
        if type(detached_tag)~="boolean" or type(gore_tag)~="boolean"then fail("native persistent component tags unavailable")end
        if detached_tag then detached[#detached+1]=row.id end
        if gore_tag then gore[#gore+1]=row.id end
        components[#components+1]=c
        phase("component_static","exit",{ok=true,class=component_class},row)
    end
    -- Preserve completeness even when a getter adds/removes a native gear mesh.
    -- This final exact set comparison and the descriptor's second equal harvest
    -- replace allocation-heavy whole-owner enumeration on each field read.
    for _,candidate in ipairs(owners)do
        local actor=owner(candidate);local set=mesh_sets[candidate.owner];local seen={}
        phase("mesh_census","enter",{getter="Actor.K2_GetComponentsByClass(MeshComponent) end",owner_id=candidate.owner})
        stats.mesh_census_calls=stats.mesh_census_calls+1
        local copied=array(checked(function()return actor:K2_GetComponentsByClass(mesh_class)end),64,function(c)
            local identity=object_id(c);local row=identity and set[identity.address]
            if not row or row.name~=identity.name or seen[identity.address]then fail("native complete mesh census changed")end
            qualify(c,row);seen[identity.address]=true;return true
        end,"return","Actor.K2_GetComponentsByClass(MeshComponent) end owner="..tostring(candidate.owner))
        for address in pairs(set)do if not seen[address]then fail("native complete mesh census changed")end end
        phase("mesh_census","exit",{ok=true,count=#copied,getter="Actor.K2_GetComponentsByClass(MeshComponent) end",owner_id=candidate.owner})
    end
    for _,row in ipairs(rows)do component(row)end
    local bound={};for _,r in ipairs(rows)do bound[#bound+1]={id=r.id,address=r.address,
        owner=checked(function()return owner(r):GetAddress()end),name=r.name}end
    local complete=true;for _,c in ipairs(components)do if c.vertex_state~="captured" and c.vertex_state~="native_asset" and c.vertex_state~="not_applicable"then complete=false end end
    stats.count=#components;stats.ok=true
    phase("render_capture","exit",stats)
    return {components=components,bindings=bound,topology={detached=detached,gore=gore,vertex_state=complete and "captured" or "unavailable"}}
end
return M
