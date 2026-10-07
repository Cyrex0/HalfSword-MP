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
    local function trace(startp,endp,radiusp,objectsp,complexp,hitsp,ignorep,returnedp)
        if not opts.enabled() then return nil end
        local start,finish,radius,objects,complex,hits,ignore,returned=
            opts.unwrap(startp),opts.unwrap(endp),opts.unwrap(radiusp),opts.unwrap(objectsp),
            opts.unwrap(complexp),opts.unwrap(hitsp),opts.unwrap(ignorep),opts.unwrap(returnedp)
        local rows,types,layers,n={},{},{},0
        local function put(k,v) rows[#rows+1]=k..":"..value(v) end
        each(objects,function(_,e)if #types<32 then types[#types+1]=value(opts.unwrap(e)) end end)
        put("objects",#types>0 and table.concat(types,",") or nil)
        put("radius",radius);put("complex",complex);put("ignore_self",ignore);put("out",returned)
        put("hits",array_n(hits))
        for _,p in ipairs({{"start",start},{"end",finish}}) do
            for _,axis in ipairs({"X","Y","Z"})do put(p[1]..axis,field(p[2],axis))end
        end
        each(hits,function(_,entry)
            n=n+1
            if n<=128 then
                local hit=opts.unwrap(entry)
                -- These are proven FWeakObjectProperty fields (not SoftObject):
                -- UE4SS FWeakObjectPtr.get resolves them, then validity gates
                -- every UObject read. No wrapper leaves this callback.
                local comp=opts.unwrap(field(hit,"Component"))
                local mat=opts.unwrap(field(hit,"PhysMaterial"))
                local mesh=valid(comp) and field(comp,"StaticMesh") or nil
                local tags,tags_truncated="unavailable",false
                if valid(comp) then tags,tags_truncated=tags_of(comp,opts.unwrap) end
                local bone=field(hit,"BoneName")
                layers[#layers+1]=table.concat({"i:"..n,"component:"..value(name(comp)),"mesh:"..value(name(mesh)),
                    "surface:"..value(valid(mat) and field(mat,"SurfaceType") or nil),"tags:"..value(tags),
                    "tags_truncated:"..value(tags_truncated),"bone:"..value(safe(function()return bone:ToString()end)),
                    "distance:"..value(field(hit,"Distance")),"blocking:"..value(field(hit,"bBlockingHit")),
                    "initial_overlap:"..value(field(hit,"bStartPenetrating"))},"|")
            end
        end)
        put("truncated",n>128)
        rows[#rows+1]="ordered_hits:"..(#layers>0 and table.concat(layers,"/") or "unavailable")
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
