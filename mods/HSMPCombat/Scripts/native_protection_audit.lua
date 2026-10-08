-- Developer-only POST evidence for native armour construction. These values
-- describe installed proxy topology, never the layers crossed by a damage trace.
-- No UObject is retained between callbacks, and no SoftObject property is read.
local M = {}
M.HOOK = "/Game/Character/Blueprints/Willie_BP.Willie_BP_C:Set Up Proxy Collisions"
M.TRACE_HOOK = "/Script/Engine.KismetSystemLibrary:SphereTraceMultiForObjects"
local DEF = {
    {"defB", "Blunt_2_0C001DBE4C8B7C85C68641B217A18F10"},
    {"defC", "Cut_9_722ACAC246B5F7600CAC84AFF9188F9D"},
    {"defS", "Stab_6_C3AA0F1D4B183BE20371069F7FDCA346"},
    {"dens", "Density_8_B3D4247C4C7FD0F8EE8F0C8D74E1A90D"},
}
local PASS = {
    {"passport_id", "ID_54_C6BBB1A64A3828B5AB1D8E804EC7C8F7"},
    {"slot", "Slot_30_7561CB484566A4512003EA96ED44F88D"},
    {"core_removed", "CoreRemoved_12_5CFF8F6D4A05C15812594CAF6771C66B"},
    {"module1", "Module1_5_46B7198E4341C93CBF6AE989EF9898E4"},
    {"module2", "Module2_7_5B7940B84CFD673B25103D96E0AFEEB0"},
    {"module3", "Module3_9_E282C465414F6D4EF2A8039FBA847AD2"},
    {"steel", "SteelType_84_7BA6626740476C2CD69648847A1E592F"},
}
local CORE = "ArmorCore_3_F6B7C69C4BD7D9720DB91EB635EE2B43"
local function safe(f) local ok,v=pcall(f); if ok then return v end end
local function field(o,k) return safe(function() return o[k] end) end
local function valid(o) return o and safe(function() return o:IsValid() end)==true end
local function address(o) return safe(function() return o:GetAddress() end) end
local function same(a,b) local x,y=address(a),address(b);return x~=nil and x==y end
local function name(o) return valid(o) and safe(function() return o:GetFullName() end) or nil end
local function value(v)
    if type(v)=="number" then
        if v~=v or math.abs(v)==math.huge then return "unavailable" end
        return string.format("%.17g",v)
    end
    if type(v)=="boolean" then return v and "true" or "false" end
    if type(v)~="string" then return "unavailable" end
    return v:gsub("%s","_"):sub(1,256)
end
local function array_n(a) return safe(function() return a:GetArrayNum() end) end
local function each(a,f) if a then safe(function() a:ForEach(f) end) end end
local function tags_of(comp,unwrap)
    local tags,n={},0
    each(field(comp,"ComponentTags"),function(_,e)
        n=n+1
        if n<=32 then
            local t=unwrap(e)
            local s=safe(function() return t:ToString() end)
            if s then tags[#tags+1]=value(s) end
        end
    end)
    return #tags>0 and table.concat(tags,",") or "unavailable",n>32
end

function M.new(opts)
    local seq,installed=0,false
    local function capture(selfp,meshp,defp,passp)
        if not opts.enabled() then return end -- no native reads when disabled
        local w,mesh,def,pass=opts.unwrap(selfp),opts.unwrap(meshp),opts.unwrap(defp),opts.unwrap(passp)
        if not valid(w) then return end
        seq=seq+1
        local parts={"LAB_ARMOR_BUILD", "evidence=armor_construction_only"}
        local function put(k,v) parts[#parts+1]=k.."="..value(v) end
        put("seq",seq);put("pawn",name(w))
        local ctx=safe(function() return opts.context(w) end) or {}
        for _,k in ipairs({"peer","match_id","round","life"}) do put(k,ctx[k]) end
        put("mesh",name(mesh));put("core",name(field(pass,CORE)))
        for _,p in ipairs(DEF) do put(p[1],field(def,p[2])) end
        for _,p in ipairs(PASS) do put(p[1],field(pass,p[2])) end
        -- The POST call appended a fresh component (AddComponent -> AddUnique)
        -- before adding its passport map entry. Match the newest same mesh;
        -- never manufacture an identity when reflection/arrays are unavailable.
        local arr=field(w,"Armor Collision Meshes")
        put("collision_n",array_n(arr))
        local comp,index,n=nil,nil,0
        each(arr,function(_,e)
            n=n+1
            if n<=256 then
                local c=opts.unwrap(e)
                if valid(c) and valid(mesh) and same(field(c,"StaticMesh"),mesh) then comp,index=c,n end
            end
        end)
        put("topology_truncated",n>256);put("component",name(comp));put("component_index",index)
        local mapped
        each(field(w,"Armor Passports related to Armor Collision Meshes"),function(k,v)
            if comp and same(opts.unwrap(k),comp) then mapped=opts.unwrap(v) end
        end)
        put("mapped_core",name(field(mapped,CORE)))
        put("mapped_passport_id",field(mapped,PASS[1][2]));put("mapped_slot",field(mapped,PASS[2][2]))
        local tags,tags_truncated=tags_of(comp,opts.unwrap)
        put("tags",tags);put("tags_truncated",tags_truncated)
        put("collision_enabled",safe(function() return comp:GetCollisionEnabled() end))
        put("object_type",safe(function() return comp:GetCollisionObjectType() end))
        put("profile",safe(function() return comp:GetCollisionProfileName():ToString() end))
        local xf=safe(function() return comp:K2_GetComponentToWorld() end)
        for _,p in ipairs({{"pos","Translation"},{"scale","Scale3D"},{"quat","Rotation"}}) do
            local v=field(xf,p[2])
            for _,axis in ipairs(p[1]=="quat" and {"X","Y","Z","W"} or {"X","Y","Z"}) do put(p[1]..axis,field(v,axis)) end
        end
        -- Reflected hard UObject BodySetup only; never load a referenced asset.
        local bs=valid(mesh) and field(mesh,"BodySetup") or nil
        local agg=valid(bs) and field(bs,"AggGeom") or nil
        put("trace_flag",valid(bs) and field(bs,"CollisionTraceFlag") or nil)
        for _,k in ipairs({"SphereElems","BoxElems","SphylElems","ConvexElems","TaperedCapsuleElems","LevelSetElems","SkinnedLevelSetElems"}) do
            put(k,array_n(field(agg,k)))
        end
        opts.log("%s",table.concat(parts," "))
    end
    local function trace_array(a,limit,convert)
        local function count()return safe(function()return a:GetArrayNum()end)end
        local n=count()
        if type(n)~="number" or n~=n or math.abs(n)==math.huge or n%1~=0 or n<0 then
            return {available=false,observed=0,reason="count_unavailable"}
        end
        if n>limit then return {available=false,count=n,observed=0,truncated=true,reason="count_over_limit"}end
        local values,observed,why={},0,nil
        local ok=pcall(function()a:ForEach(function(i,e)
            observed=observed+1
            if observed>n or observed>limit or i~=observed then why="iteration_count_or_order_changed";return true end
            local read,v=pcall(convert,e,i)
            if not read or v==nil then why="entry_read_failed";return true end
            values[#values+1]=v
            -- Pinned LuaTArray.cpp:158-160 breaks on a true callback result.
        end)end)
        local after=count()
        if not ok then why="iteration_failed"
        elseif after~=n then why="count_changed"
        elseif observed~=n and not why then why="iteration_incomplete"end
        return {available=why==nil,count=n,post_count=after,observed=observed,truncated=observed>limit,
            reason=why,values=not why and values or nil}
    end
    local function trace(startp,endp,radiusp,objectsp,complexp,hitsp,ignorep,returnedp)
        if not opts.enabled() then return nil end
        local start,finish,radius,objects,complex,hits,ignore,returned=
            opts.unwrap(startp),opts.unwrap(endp),opts.unwrap(radiusp),opts.unwrap(objectsp),
            opts.unwrap(complexp),opts.unwrap(hitsp),opts.unwrap(ignorep),opts.unwrap(returnedp)
        local rows={}
        local function put(k,v) rows[#rows+1]=k..":"..value(v) end
        local types=trace_array(objects,32,function(e)
            local v=opts.unwrap(e)
            if type(v)=="number" and v==v and math.abs(v)<math.huge and v%1==0 then return value(v)end
        end)
        put("objects",types.available and (#types.values>0 and table.concat(types.values,",") or "[]") or nil)
        put("radius",radius);put("complex",complex);put("ignore_self",ignore);put("out",returned)
        for _,p in ipairs({{"start",start},{"end",finish}}) do
            for _,axis in ipairs({"X","Y","Z"})do put(p[1]..axis,field(p[2],axis))end
        end
        local function finite(v)return type(v)=="number" and v==v and math.abs(v)<math.huge end
        local complete=finite(radius) and type(complex)=="boolean" and type(ignore)=="boolean" and type(returned)=="boolean"
        for _,axis in ipairs({"X","Y","Z"})do
            if not finite(field(start,axis)) or not finite(field(finish,axis))then complete=false end
        end
        local layers=trace_array(hits,128,function(entry,n)
                local hit=opts.unwrap(entry)
                -- These are proven FWeakObjectProperty fields (not SoftObject):
                -- UE4SS FWeakObjectPtr.get resolves them, then validity gates
                -- every UObject read. No wrapper leaves this callback.
                local comp=opts.unwrap(field(hit,"Component"))
                local mat=opts.unwrap(field(hit,"PhysMaterial"))
                local mesh=valid(comp) and field(comp,"StaticMesh") or nil
                local tags=trace_array(valid(comp) and field(comp,"ComponentTags") or nil,32,function(e)
                    local s=opts.unwrap(e):ToString()
                    if type(s)=="string" then return value(s)end
                end)
                if not tags.available then complete=false end
                local bone=field(hit,"BoneName")
                local comp_name,surface=name(comp),valid(mat) and field(mat,"SurfaceType") or nil
                local bone_name=safe(function()return bone:ToString()end)
                local distance,blocking,overlap=field(hit,"Distance"),field(hit,"bBlockingHit"),field(hit,"bStartPenetrating")
                if not comp_name or not finite(surface) or type(bone_name)~="string" or not finite(distance)
                    or type(blocking)~="boolean" or type(overlap)~="boolean" then complete=false end
                return table.concat({"i:"..n,"component:"..value(comp_name),"mesh:"..value(name(mesh)),
                    "surface:"..value(surface),"tags:"..(tags.available and (#tags.values>0 and table.concat(tags.values,",") or "[]") or "unavailable"),
                    "tags_truncated:"..value(tags.truncated==true),"bone:"..value(bone_name),
                    "distance:"..value(distance),"blocking:"..value(blocking),
                    "initial_overlap:"..value(overlap),
                    "tags_read_complete:"..value(tags.available),"tags_count:"..value(tags.count),
                    "tags_post_count:"..value(tags.post_count),"tags_observed:"..value(tags.observed),"tags_reason:"..value(tags.reason)},"|")
        end)
        -- Keep the legacy header fields contiguous for existing diagnostics.
        table.insert(rows,6,"hits:"..value(layers.count))
        put("objects_read_complete",types.available);put("objects_post_count",types.post_count);put("objects_observed",types.observed);put("objects_reason",types.reason)
        put("hits_read_complete",layers.available);put("hits_post_count",layers.post_count);put("hits_observed",layers.observed);put("hits_reason",layers.reason)
        put("truncated",layers.truncated==true or types.truncated==true)
        put("read_complete",types.available and layers.available and complete)
        rows[#rows+1]="ordered_hits:"..(layers.available and (#layers.values>0 and table.concat(layers.values,"/") or "[]") or "unavailable")
        return table.concat(rows,",")
    end
    return {
        capture=function(...) local a=table.pack(...);return safe(function() return capture(table.unpack(a,1,a.n)) end) end,
        trace=function(...) local a=table.pack(...);return safe(function() return trace(table.unpack(a,1,a.n)) end) end,
        install=function(register)
            if installed then return true end
            installed=pcall(register,M.HOOK,function(...) local args=table.pack(...);safe(function() capture(table.unpack(args,1,args.n)) end) end)
            return installed
        end,
    }
end
return M
