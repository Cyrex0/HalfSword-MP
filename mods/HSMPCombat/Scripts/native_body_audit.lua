-- Opt-in native body readback. No damage/physics writes, enum-to-bone guesses,
-- retained UObjects, or cached values substituted for a failed native read.
local M={}
M.HOOKS={"Dismember Function Initiate","Dismember Function Delayed"}
M.HEALTH={"Health","Head Health","Neck Health","Body Upper Health","Body Lower Health","Back Health",
    "Arm_R Health","Arm_L Health","Leg_R Health","Leg_L Health","Head Health (Crush)","Bleeding","Pain"}
M.FLAGS={"Invulnerable","Block Spine Breaking","Head Broken","Neck Snapped","Neck Dislocated","Back Broken",
    "Spine Dislocated","Arm R Broken","Arm R Dislocated","Arm L Broken","Arm L Dislocated",
    "Leg R Broken","Leg R Dislocated","Leg L Broken","Leg L Dislocated","Headless",
    "Hand R Torn Off","Hand L Torn Off","Leg R Torn Off","Leg L Torn Off","Upper Body Spawned",
    "Dismemberment In Process","Force Disable Dismemberment","Force Disable Vertex Paint","Setup Armor in Process"}
-- These are actual skeleton names, queried independently. They are not a
-- mapping from the native 15-part enum to the 23-bit network bone mask.
M.BONES={"pelvis","spine_01","spine_02","spine_03","spine_04","spine_05","neck_01","neck_02","head",
    "clavicle_l","upperarm_l","lowerarm_l","hand_l","clavicle_r","upperarm_r","lowerarm_r","hand_r",
    "thigh_l","calf_l","foot_l","thigh_r","calf_r","foot_r"}
M.CONSTRAINTS={"Dislocated Bone Constraint Arm R","Dislocated Bone Constraint Arm L","Dislocated Bone Constraint Leg R",
    "Dislocated Bone Constraint Leg L","Dislocated Bone Constraint Neck","Dislocated Bone Constraint Back"}
-- Cooked Willie offsets 623197/623312 and 624058/624173 pass 3 to
-- SceneComponent:GetSocketTransform. The local SDK's ERelativeTransformSpace
-- declares RTS_ParentBoneSpace=3 (RTS_Component=2); these are not world poses.
M.DISLOCATION_SPACE=3
M.DISLOCATION_BONES={{bone="upperarm_r",part=3,flag="Arm R Dislocated"},
    {bone="upperarm_l",part=6,flag="Arm L Dislocated"}}
-- Native topology version1 availability fields; these never form a bone mask.
M.TOPOLOGY_FLAGS={"Headless","Hand R Torn Off","Hand L Torn Off","Leg R Torn Off","Leg L Torn Off",
    "Upper Body Spawned","Dismemberment In Process"}
M.SEVER_MARKER_LIMIT=64
M.SEVER_TAG_BYTES=1024
M.SEVER_TYPES={master="/Script/Engine.SkeletalMeshComponent",attach_marker="/Script/Engine.PrimitiveComponent",
    box1="/Script/Engine.BoxComponent",box2="/Script/Engine.BoxComponent",weapon="/Script/Engine.PrimitiveComponent"}
local function safe(f) local ok,v=pcall(f);if ok then return v end end
local function field(o,k) return safe(function()return o[k]end) end
local function number(v) return type(v)=="number" and v==v and math.abs(v)<math.huge and v or nil end
local function boolean(v) if type(v)=="boolean" then return v end end
local function token(v)
    if type(v)=="boolean" then return v and "true" or "false" end
    if number(v) then return string.format("%.17g",v) end
    if type(v)=="string" then return v:gsub("[%s|;,=]","_"):sub(1,256) end
    return "unavailable"
end
-- Exact JSON string, including separators used by the surrounding log row.
-- Native UTF-8 bytes remain intact; whitespace/control bytes never split rows.
local escaped_tag_bytes={[34]=true,[92]=true,[124]=true,[59]=true,[44]=true,[61]=true,
    [91]=true,[93]=true,[58]=true,[47]=true}
local function quote_tag(v)
    return '"'..v:gsub(".",function(ch)
        local b=ch:byte()
        return (b<=32 or escaped_tag_bytes[b]) and string.format("\\u%04x",b) or ch
    end)..'"'
end
local function valid(o) return o and safe(function()return o:IsValid()end)==true end
local function address(o) return valid(o) and safe(function()return o:GetAddress()end) or nil end
local function name(o) return valid(o) and safe(function()return o:GetFullName()end) or nil end
local function fname(v) return safe(function()return v:ToString()end) end
local function identity(o) return {address=address(o),name=name(o)} end
local function scope_key(c)
    if type(c)~="table" or type(c.world)~="string" or c.world=="" or type(c.pawn)~="string"
        or not number(c.peer) or c.peer<=0 or not number(c.match_id) or c.match_id<=0
        or not number(c.round) or c.round<=0 or not number(c.life) or c.life<=0
        or c.actor==nil or c.mesh==nil then return nil end
    return table.concat({c.world,c.peer,c.match_id,c.round,c.life,c.pawn,tostring(c.actor),tostring(c.mesh)},":")
end
function M.new(o)
    local seq,installed,ambiguous,hook_errors,starts,active_scope=0,{},{},{},{},nil
    local function hook_id(v)return number(v) and v%1==0 and v>=-2147483648 and v<=2147483647 end
    local function array(a,convert,limit)
        if not a then return {available=false} end
        local n=number(safe(function()return a:GetArrayNum()end))
        local values,count={},0
        local ok=pcall(function()a:ForEach(function(_,v)
            count=count+1
            if count>(limit or 64) then error("bounded read exceeded") end
            local value=convert(o.unwrap(v));if value==nil then error("unreadable entry") end
            values[#values+1]=value
        end)end)
        local complete=ok and n~=nil and n==count
        return {available=complete,count=n,values=complete and values or nil,truncated=count>(limit or 64)}
    end
    local function topology(w,key,current_context)
        if type(o.topology_reader)~="table" or type(o.topology_reader.read)~="function" then
            return {available=false,reason="reader unavailable",flags={}}
        end
        local topology_env={unwrap=o.unwrap,map_count=o.map_count,context=function(actor)
            -- Recheck the admission that originally selected owner/source.
            if not o.enabled() then return nil end
            return current_context(actor)
        end}
        local ok,t,why=pcall(o.topology_reader.read,w,topology_env)
        if not ok or type(t)~="table" then
            return {available=false,reason=ok and (why or "reader unavailable") or "reader failed",flags={}}
        end
        if t.version~=1 or scope_key(t.context)~=key then
            return {available=false,reason="reader version or scope mismatch",flags={}}
        end
        t.available=true
        return t
    end
    local function component(c,bones)
        if not valid(c) then return {available=false} end
        local asset=safe(function()return c:GetSkeletalMeshAsset()end)
        local override=field(c,"PhysicsAssetOverride")
        local physics=valid(override) and override or (valid(asset) and field(asset,"PhysicsAsset") or nil)
        local r={available=true,identity=identity(c),collision=number(safe(function()return c:GetCollisionEnabled()end)),
            physics_asset=identity(physics),physics_override=identity(override),asset=identity(asset),bones={}}
        local xf=safe(function()return c:K2_GetComponentToWorld()end)
        r.transform={}
        for _,k in ipairs({"Translation","Rotation","Scale3D"})do
            local v=field(xf,k);r.transform[k]={}
            for _,a in ipairs(k=="Rotation" and {"X","Y","Z","W"} or {"X","Y","Z"})do r.transform[k][a]=number(field(v,a))end
        end
        local box=field(c,"BoxExtent")
        r.box={number(field(box,"X")),number(field(box,"Y")),number(field(box,"Z"))}
        for _,b in ipairs(bones or {}) do
            local f=o.fname(b)
            local idx=number(safe(function()return c:GetBoneIndex(f)end))
            local row={bone=b,index=idx}
            if idx and idx>=0 then
                row.hidden=boolean(safe(function()return c:IsBoneHiddenByName(f)end))
                row.sim=boolean(safe(function()return c:IsSimulatingPhysics(f)end))
                row.mass=number(safe(function()return c:GetBoneMass(f,true)end))
                -- COM only describes a proven simulated native body.
                if row.sim==true then
                    local p=safe(function()return c:GetCenterOfMass(f)end)
                    row.com={number(field(p,"X")),number(field(p,"Y")),number(field(p,"Z"))}
                end
            end
            r.bones[#r.bones+1]=row
        end
        return r
    end
    local function constraint(c)
        if not valid(c)then return {available=false}end
        return {available=true,identity=identity(c),broken=boolean(safe(function()return c:IsBroken()end)),
            swing1=number(safe(function()return c:GetCurrentSwing1()end)),
            swing2=number(safe(function()return c:GetCurrentSwing2()end)),twist=number(safe(function()return c:GetCurrentTwist()end))}
    end
    local function dislocation_inputs(w,mesh,r,current_context)
        local invalid
        local function guard()
            if invalid then return false end
            if not o.enabled() or scope_key(safe(function()return current_context(w)end))~=r.key then
                invalid="scope changed";return false
            end
            return true
        end
        local function read(f)if guard() then return safe(f)end end
        local function unknown(why)return {available=false,reason=why or "unavailable"}end
        local function ref(c)
            if read(function()return valid(c)end)~=true then return unknown("identity unavailable")end
            local id=number(read(function()return address(c)end))
            local nm=read(function()return name(c)end)
            local fn=read(function()return fname(c:GetFName())end)
            local ok=id and id>0 and id%1==0 and type(nm)=="string" and nm~="" and type(fn)=="string" and fn~=""
            return {available=ok==true,address=id,name=type(nm)=="string" and nm or nil,fname=type(fn)=="string" and fn or nil}
        end
        local function world(c)
            local v=ref(read(function()return c:GetWorld()end))
            return v.available and tostring(v.address).."@"..v.name or nil
        end
        local pawn=ref(w);pawn.world=world(w)
        local s={version=1,space=M.DISLOCATION_SPACE,space_name="RTS_ParentBoneSpace",pawn=pawn,
            eligibility="unavailable:sample_only",available=false,read_complete=false,bones={}}
        local function scalar(k,convert,native_type)
            local raw=read(function()return field(w,k)end)
            local v=convert(raw)
            return {available=v~=nil,value=v,read_type=type(raw),native_type=native_type}
        end
        -- Willie SDK BoneSnapping is double; BlockSpineBreaking is bool.
        s.bone_snapping=scalar("Bone Snapping",number,"double")
        s.block_spine_breaking=scalar("Block Spine Breaking",boolean,"bool")
        local function owned(c)
            local id=ref(c);local owner=ref(read(function()return c:GetOwner()end))
            local cw,ow=world(c),world(read(function()return c:GetOwner()end))
            local typed=read(function()return boolean(c:IsA("/Script/Engine.SkeletalMeshComponent"))end)
            local g={available=false,identity=id,owner=owner,world=cw,class_match=typed}
            owner.world=ow
            if not id.available or not owner.available then g.reason="component or owner identity unavailable"
            elseif cw~=r.context.world or ow~=r.context.world then g.reason="component or owner world unavailable or mismatched"
            elseif owner.address~=pawn.address or owner.name~=pawn.name or owner.fname~=pawn.fname then g.reason="component owner mismatched"
            elseif typed~=true then g.reason="skeletal component type unavailable or mismatched"
            else g.available=true end
            return g
        end
        local driver=read(function()return field(w,"DriverSkeleton")end)
        s.mesh,s.driver=owned(mesh),owned(driver)
        if not guard() then return nil end
        if not pawn.available or pawn.address~=r.context.actor or pawn.fname~=r.context.pawn or pawn.world~=r.context.world then
            s.reason="pawn identity or world unavailable or mismatched";return s
        end
        if not s.mesh.available or not s.driver.available or s.mesh.identity.address~=r.context.mesh
            or s.mesh.identity.address==s.driver.identity.address then
            s.reason="distinct owned Mesh and DriverSkeleton unavailable";return s
        end
        local function position(c,bone)
            local f=read(function()return o.fname(bone)end)
            local idx=number(read(function()return c:GetBoneIndex(f)end))
            if not idx or idx<0 or idx%1~=0 then return unknown("native bone index unavailable")end
            local xf=read(function()return c:GetSocketTransform(f,M.DISLOCATION_SPACE)end)
            local p=read(function()return field(xf,"Translation")end)
            local v={available=true,index=idx}
            for _,k in ipairs({"X","Y","Z"})do
                v[k]=number(read(function()return field(p,k)end));if v[k]==nil then v.available=false end
            end
            if not v.available then v.reason="native translation unavailable or nonfinite"end
            return v
        end
        s.available=true;s.read_complete=s.bone_snapping.available and s.block_spine_breaking.available
        for _,spec in ipairs(M.DISLOCATION_BONES)do
            local b={bone=spec.bone,native_part=spec.part,part=unknown("native map unavailable or key absent"),
                dislocated=scalar(spec.flag,boolean,"bool"),mesh=position(mesh,spec.bone),driver=position(driver,spec.bone)}
            -- Exact native map keys 3/6 are proved by the cooked arm branches,
            -- independent of network bone bits. Absent keys remain unknown.
            if r.parts.available then
                b.part.present=false
                for _,v in ipairs(r.parts.values or {})do
                    if v.part==spec.part then b.part={available=true,present=true,value=v.value};break end
                end
            end
            if b.mesh.available and b.driver.available then
                local dx,dy,dz=b.mesh.X-b.driver.X,b.mesh.Y-b.driver.Y,b.mesh.Z-b.driver.Z
                b.distance=number(math.sqrt(dx*dx+dy*dy+dz*dz))
            end
            b.read_complete=b.distance~=nil and b.part.available and b.dislocated.available
                and s.bone_snapping.available and s.block_spine_breaking.available
            if b.read_complete then
                b.sampled_predicates_match=b.distance>15 and b.dislocated.value==false and b.part.value==false
                    and s.block_spine_breaking.value==false and s.bone_snapping.value>0.1
            end
            if not b.read_complete then s.read_complete=false end
            s.bones[#s.bones+1]=b
        end
        if not guard() then return nil end
        local ma,da=owned(read(function()return field(w,"Mesh")end)),owned(read(function()return field(w,"DriverSkeleton")end))
        local pa=ref(w)
        if not guard() then return nil end
        if not ma.available or not da.available or ma.identity.address~=s.mesh.identity.address or ma.identity.name~=s.mesh.identity.name
            or da.identity.address~=s.driver.identity.address or da.identity.name~=s.driver.identity.name
            or world(w)~=pawn.world or pa.address~=pawn.address or pa.name~=pawn.name or pa.fname~=pawn.fname then
            return {version=1,space=M.DISLOCATION_SPACE,space_name="RTS_ParentBoneSpace",available=false,
                read_complete=false,eligibility="unavailable:sample_only",reason="native identity changed",bones={}}
        end
        if not guard() then return nil end
        return s
    end
    local function snapshot(w,event,meta,current_context)
        current_context=current_context or o.context
        if not o.enabled() then return nil end
        local c=safe(function()return current_context(w)end)
        local key=scope_key(c);if not key then return nil end
        if address(w)~=c.actor then return nil end
        local mesh=field(w,"Mesh")
        if address(mesh)~=c.mesh then return nil end
        if active_scope~=key then starts={};active_scope=key end
        seq=seq+1
        local r={seq=seq,event=event,context=c,key=key,meta=meta or {},health={},flags={},components={},constraints={}}
        for _,k in ipairs(M.HEALTH)do r.health[k]=number(field(w,k))end
        for _,k in ipairs(M.FLAGS)do r.flags[k]=boolean(field(w,k))end
        r.topology=topology(w,key,current_context)
        if not o.enabled() or scope_key(safe(function()return current_context(w)end))~=key then return nil end
        local function missing()return {available=false,reason=r.topology.reason}end
        r.dism_array=r.topology.available and r.topology.dism_array or missing()
        r.dism_bones=r.topology.available and r.topology.dism_bones or missing()
        r.parts=r.topology.available and r.topology.parts or missing()
        r.dislocation_inputs=dislocation_inputs(w,mesh,r,current_context)
        if not r.dislocation_inputs then return nil end
        for _,k in ipairs(M.TOPOLOGY_FLAGS)do
            local f=r.topology.flags[k]
            if f and f.available then r.flags[k]=f.value else r.flags[k]=nil end
        end
        r.spawn_bone=fname(field(w,"Spawn Bone"))
        r.components.Mesh=component(mesh,M.BONES)
        r.components.SK_Skeleton=component(field(w,"SK_Skeleton"),M.BONES)
        -- Native severing can add components on the SAME Willie. Read fresh
        -- arrays/references, never turn their native enums into guessed bones.
        for _,k in ipairs({"Upper Body Mesh","Dismembered Limb"})do
            r.components[k]=component(field(w,k),M.BONES)
        end
        for _,k in ipairs({"Dism VP Collision Box 1","Dism VP Collision Box 2"})do r.components[k]=component(field(w,k))end
        for _,k in ipairs({"Dismember Child Parts","Dismember Parent Parts","Worn Armor"})do
            r.components[k]=array(field(w,k),function(v)return valid(v) and component(v,M.BONES) or nil end,16)
        end
        for _,k in ipairs(M.CONSTRAINTS)do r.constraints[k]=constraint(field(w,k))end
        local cmap=field(w,"Dismembered Body Constraints")
        local cv,cn={},0
        local cok=cmap and pcall(function()cmap:ForEach(function(k,v)
            cn=cn+1;if cn>32 then error("bounded constraints exceeded")end
            local bone=fname(o.unwrap(k));local component=constraint(o.unwrap(v))
            if not bone or not component.available then error("unreadable native constraint")end
            cv[#cv+1]={bone=bone,component=component}
        end)end)
        r.constraints.dismembered={available=cok==true,count=cok and cn or nil,values=cok and cv or nil,truncated=cn>32}
        -- Native construction callbacks are synchronous here, but a stale
        -- identity halfway through a read must never become a qualified row.
        local after=safe(function()return current_context(w)end)
        if scope_key(after)~=key then return nil end
        return r
    end
    -- Local SDK: seven reflected inputs at 0/8/10/18/28/30/38 (hex), no
    -- ReturnValue. Pinned script_hook passes the reference array's live out
    -- address. These are POST observations, never eligible-cut/PRE evidence.
    local function sever_inputs(w,args,r,current_context)
        local invalid
        local function guard()
            if invalid then return false end
            if not o.enabled() or scope_key(safe(function()return current_context(w)end))~=r.key then
                invalid="scope changed";return false
            end
            return true
        end
        local function read(f)if guard() then return safe(f)end end
        local function unknown(reason)return {available=false,reason=reason or invalid or "unavailable"}end
        local function integer(v,max)
            return number(v) and v%1==0 and v>=0 and (not max or v<=max) and v or nil
        end
        local function text_value(v)
            return type(v)=="string" and v~="" and #v<=1024 and not v:find("\0",1,true) and v or nil
        end
        local function world_key(c)
            if not valid(c) then return nil end
            local id,nm=address(c),text_value(name(c))
            return id and nm and tostring(id).."@"..nm or nil
        end
        local function ref(c)
            if read(function()return valid(c)end)~=true then return unknown()end
            local id=integer(read(function()return address(c)end))
            if id==0 then id=nil end
            local nm=read(function()return text_value(name(c))end)
            local fn=read(function()return text_value(fname(c:GetFName()))end)
            return {available=id~=nil and nm~=nil and fn~=nil,address=id,name=nm,fname=fn}
        end
        local function names(a)
            local n=integer(read(function()return a:GetArrayNum()end))
            if n==nil then return unknown("count unavailable")end
            if n>M.SEVER_MARKER_LIMIT then return {available=false,reason="tag count limit exceeded",count=n,truncated=true}end
            local values,count={},0
            local limited=false
            local ok=pcall(function()a:ForEach(function(i,v)
                if not guard() then error(invalid)end
                count=count+1
                if count>n or i~=count then error("array order/count mismatch")end
                local raw=fname(o.unwrap(v))
                if type(raw)=="string" and #raw>M.SEVER_TAG_BYTES then limited=true;error("tag byte limit exceeded")end
                local value=text_value(raw)
                if not value then error("name unavailable")end
                values[count]=value
            end)end)
            local after=integer(read(function()return a:GetArrayNum()end),M.SEVER_MARKER_LIMIT)
            if not ok or after~=n or count~=n then
                return {available=false,reason=limited and "tag byte limit exceeded" or "iteration incomplete or count changed",
                    count=n,truncated=limited}
            end
            return {available=true,count=n,values=values,truncated=false}
        end
        local function vector(v,axes)
            local t={available=true}
            for _,k in ipairs(axes)do
                t[k]=read(function()return number(field(v,k))end)
                if t[k]==nil then t.available=false end
            end
            return t
        end
        local function geometry(c,expected)
            if not guard() then return unknown()end
            local id=ref(c)
            if not id.available then return unknown("native component identity unavailable")end
            local cw=read(function()return world_key(c:GetWorld())end)
            local class=read(function()return c:GetClass()end)
            local class_ref=ref(class)
            local typed=read(function()return boolean(c:IsA(expected))end)
            local g={available=false,identity=id,class=class_ref,expected=expected,class_match=typed,world=cw}
            if cw~=r.context.world then g.reason="native component world unavailable or changed";return g end
            if typed~=true then g.reason="native component type unavailable or mismatched";return g end
            g.available=true
            local owner=read(function()return c:GetOwner()end)
            g.owner=ref(owner)
            if g.owner.available then
                g.owner.world=read(function()return world_key(owner:GetWorld())end)
                if g.owner.world~=r.context.world then g.owner=unknown("owner world unavailable or changed")end
            end
            local parent=read(function()return c:GetAttachParent()end)
            g.parent=ref(parent)
            if g.parent.available then
                g.parent.world=read(function()return world_key(parent:GetWorld())end)
                if g.parent.world~=r.context.world then g.parent=unknown("attachment world unavailable or changed")end
            end
            g.socket=text_value(read(function()return fname(c:GetAttachSocketName())end))
            g.socket_available=g.socket~=nil
            g.tags=names(read(function()return field(c,"ComponentTags")end))
            local xf=read(function()return c:K2_GetComponentToWorld()end)
            g.transform={translation=vector(read(function()return field(xf,"Translation")end),{"X","Y","Z"}),
                rotation=vector(read(function()return field(xf,"Rotation")end),{"X","Y","Z","W"}),
                scale=vector(read(function()return field(xf,"Scale3D")end),{"X","Y","Z"})}
            g.transform.available=g.transform.translation.available and g.transform.rotation.available and g.transform.scale.available
            g.collision=read(function()return number(c:GetCollisionEnabled())end)
            g.collision_available=g.collision~=nil
            if expected==M.SEVER_TYPES.box1 then
                g.extent=vector(read(function()return c:GetUnscaledBoxExtent()end),{"X","Y","Z"})
            elseif expected==M.SEVER_TYPES.master then
                local asset=read(function()return c:GetSkeletalMeshAsset()end)
                local override=read(function()return field(c,"PhysicsAssetOverride")end)
                g.asset=ref(asset)
                g.physics_asset=ref(read(function()return valid(override) and override or field(asset,"PhysicsAsset")end))
            end
            local after=ref(c)
            if after.address~=id.address or after.fname~=id.fname or after.name~=id.name
                or read(function()return world_key(c:GetWorld())end)~=cw then return unknown("native component identity changed")end
            g.read_complete=class_ref.available and g.owner.available and g.parent.available and g.socket_available
                and g.tags.available and g.transform.available and g.collision_available
                and (not g.extent or g.extent.available)
                and (not g.asset or (g.asset.available and g.physics_asset.available))
            return g
        end
        local inputs={version=1,observation="Blueprint_POST",eligibility="unavailable",read_complete=true}
        local selected=integer(read(function()return field(w,"Currently Dismembered Part")end),14)
        inputs.current_part={available=selected~=nil,value=selected}
        inputs.current_master=ref(read(function()return field(w,"Currently Dismembered Mesh")end))
        local part=integer(read(function()return o.unwrap(args[2])end),14)
        inputs.part={available=part~=nil,value=part}
        for _,entry in ipairs({{"master",1},{"attach_marker",3},{"box1",5},{"box2",6},{"weapon",7}})do
            inputs[entry[1]]=geometry(read(function()return o.unwrap(args[entry[2]])end),M.SEVER_TYPES[entry[1]])
        end
        local a=read(function()return o.unwrap(args[4])end)
        local n=integer(read(function()return a:GetArrayNum()end))
        local markers=unknown("count unavailable")
        if n and n>M.SEVER_MARKER_LIMIT then markers={available=false,reason="limit exceeded",count=n}
        elseif n~=nil then
            local values,count,complete={},0,true
            local ok=pcall(function()a:ForEach(function(i,v)
                if not guard() then error(invalid)end
                count=count+1
                if count>n or i~=count then error("array order/count mismatch")end
                values[count]=geometry(o.unwrap(v),M.SEVER_TYPES.attach_marker)
                values[count].index=i -- pinned TArray ForEach is one-based
                if not values[count].read_complete then complete=false end
            end)end)
            local after=integer(read(function()return a:GetArrayNum()end),M.SEVER_MARKER_LIMIT)
            if ok and count==n and after==n then markers={available=true,count=n,values=values,read_complete=complete}
            else markers=unknown("iteration incomplete or count changed")end
        end
        inputs.overlapped_markers=markers
        for _,key in ipairs({"part","master","attach_marker","overlapped_markers","box1","box2","weapon"})do
            local v=inputs[key]
            if not v.available or (key~="part" and not v.read_complete)then inputs.read_complete=false end
        end
        if not guard() then return nil end
        return inputs
    end
    local function emit(r)
        if not r then return end
        local rows={"LAB_BODY","seq="..r.seq,"event="..token(r.event),"world="..token(r.context.world),
            "peer="..token(r.context.peer),"match="..token(r.context.match_id),"round="..token(r.context.round),
            "life="..token(r.context.life),"pawn="..token(r.context.pawn),"actor="..token(r.context.actor),"mesh="..token(r.context.mesh)}
        local function put(k,v)rows[#rows+1]=k.."="..token(v)end
        for _,k in ipairs({"attacker","hit_id","cid","parent_cid","attacker_life","source_class","source_meta","bone","part","master","weapon","pre","pair",
            "damaged_mesh","hit_by","raw","cut","draw","pain_rate","inside","lower","damage_applied"})do put(k,r.meta[k])end
        for _,k in ipairs(M.HEALTH)do put("hp_"..k:gsub("%W","_"),r.health[k])end
        for _,k in ipairs(M.FLAGS)do put("flag_"..k:gsub("%W","_"),r.flags[k])end
        put("topology_available",r.topology.available);put("topology_version",r.topology.version)
        put("topology_read_complete",r.topology.read_complete);put("topology_reason",r.topology.reason)
        put("native_part_enum",r.topology.part_enum)
        put("native_part_present_mask",r.parts.present_mask);put("native_part_true_mask",r.parts.true_mask)
        put("native_part_count_check",r.parts.count_check)
        for _,k in ipairs(M.TOPOLOGY_FLAGS)do
            local f=r.topology.flags[k]
            put("topology_flag_"..k:gsub("%W","_").."_available",f and f.available)
        end
        local function collection(k,a,convert)
            put(k.."_available",a.available);put(k.."_n",a.count);put(k.."_truncated",a.truncated);put(k.."_reason",a.reason)
            local values={};for _,v in ipairs(a.values or {})do values[#values+1]=convert(v)end
            rows[#rows+1]=k.."="..(a.available and ("["..table.concat(values,",").."]") or "unavailable")
        end
        collection("dism_array",r.dism_array,token);collection("dism_bones",r.dism_bones,token)
        collection("parts",r.parts,function(v)return v.part..":"..token(v.value)end);put("spawn_bone",r.spawn_bone)
        local d=r.dislocation_inputs
        put("dislocation_inputs_version",d.version);put("dislocation_space",d.space);put("dislocation_space_name",d.space_name)
        put("dislocation_inputs_available",d.available);put("dislocation_inputs_read_complete",d.read_complete)
        put("dislocation_inputs_reason",d.reason);put("dislocation_eligibility",d.eligibility)
        for _,k in ipairs({"bone_snapping","block_spine_breaking"})do
            local v=d[k] or {};put("dislocation_"..k.."_available",v.available);put("dislocation_"..k,v.value)
            put("dislocation_"..k.."_read_type",v.read_type)
        end
        for _,k in ipairs({"pawn","mesh","driver"})do
            local g=d[k] or {};local id=g.identity or g;local owner=g.owner or {}
            rows[#rows+1]="dislocation_"..k.."="..table.concat({token(g.available),token(id.address),token(id.name),token(id.fname),
                "world:"..token(g.world),"owner:"..token(owner.available).."/"..token(owner.address).."/"..token(owner.name)
                .."/"..token(owner.fname).."/"..token(owner.world),"reason:"..token(g.reason)},"|")
        end
        for _,b in ipairs(d.bones or {})do
            local function pos(v)return table.concat({token(v.available),token(v.index),token(v.X),token(v.Y),token(v.Z)},"/")end
            rows[#rows+1]="dislocation_"..b.bone.."="..table.concat({"part:"..b.native_part,"part_available:"..token(b.part.available),
                "part_present:"..token(b.part.present),"part_value:"..token(b.part.value),"dislocated:"..token(b.dislocated.available)
                .."/"..token(b.dislocated.value),"mesh:"..pos(b.mesh),"driver:"..pos(b.driver),"distance:"..token(b.distance),
                "complete:"..token(b.read_complete),"sampled_predicates_match:"..token(b.sampled_predicates_match)},"|")
        end
        local function comp(c)
            if not c.available then return "unavailable"end
            local b={};for _,v in ipairs(c.bones)do
                local p=v.com and table.concat({token(v.com[1]),token(v.com[2]),token(v.com[3])},"/") or "unavailable"
                b[#b+1]=table.concat({v.bone,token(v.index),token(v.hidden),token(v.sim),token(v.mass),p},":")
            end
            return table.concat({token(c.identity.address),token(c.identity.name),"collision:"..token(c.collision),
                "asset:"..token(c.asset.name),"physics_asset:"..token(c.physics_asset.name),"physics_override:"..token(c.physics_override.name),
                "position:"..token(c.transform.Translation.X).."/"..token(c.transform.Translation.Y).."/"..token(c.transform.Translation.Z),
                "quat:"..token(c.transform.Rotation.X).."/"..token(c.transform.Rotation.Y).."/"..token(c.transform.Rotation.Z).."/"..token(c.transform.Rotation.W),
                "scale:"..token(c.transform.Scale3D.X).."/"..token(c.transform.Scale3D.Y).."/"..token(c.transform.Scale3D.Z),
                "box:"..token(c.box[1]).."/"..token(c.box[2]).."/"..token(c.box[3]),"bones["..table.concat(b,",").."]"},"|")
        end
        for _,k in ipairs({"Mesh","SK_Skeleton","Upper Body Mesh","Dismembered Limb","Dism VP Collision Box 1","Dism VP Collision Box 2"})do rows[#rows+1]=k:gsub("%W","_").."="..comp(r.components[k])end
        for _,k in ipairs({"Dismember Child Parts","Dismember Parent Parts","Worn Armor"})do collection(k:gsub("%W","_"),r.components[k],comp)end
        local function con(c)
            return c.available and table.concat({token(c.identity.address),token(c.identity.name),"broken:"..token(c.broken),
                "swing1:"..token(c.swing1),"swing2:"..token(c.swing2),"twist:"..token(c.twist)},"|") or "unavailable"
        end
        for _,k in ipairs(M.CONSTRAINTS)do rows[#rows+1]=k:gsub("%W","_").."="..con(r.constraints[k])end
        collection("dism_constraints",r.constraints.dismembered,function(v)return token(v.bone)..":"..con(v.component)end)
        if r.sever_inputs then
            local s=r.sever_inputs
            put("sever_inputs_version",s.version);put("sever_observation",s.observation)
            put("sever_eligibility",s.eligibility);put("sever_inputs_read_complete",s.read_complete)
            put("sever_side",r.context.side or "owner")
            put("sever_part_available",s.part.available);put("sever_part",s.part.value)
            put("sever_current_part_available",s.current_part.available);put("sever_current_part",s.current_part.value)
            put("sever_current_master",s.current_master.address)
            local function reference(v)
                return table.concat({token(v.available),token(v.address),token(v.name),token(v.fname)},"/")
            end
            local function vec(v,axes)
                local out={token(v.available)}
                for _,k in ipairs(axes)do out[#out+1]=token(v[k])end
                return table.concat(out,"/")
            end
            local function geom(g)
                local parts={"available:"..token(g.available),"reason:"..token(g.reason),
                    "expected:"..token(g.expected),"class_match:"..token(g.class_match),"identity:"..reference(g.identity or {}),
                    "class:"..reference(g.class or {}),"world:"..token(g.world),"complete:"..token(g.read_complete)}
                if g.available then
                    parts[#parts+1]="owner:"..reference(g.owner).."/"..token(g.owner.world)
                    parts[#parts+1]="parent:"..reference(g.parent).."/"..token(g.parent.world)
                    parts[#parts+1]="socket:"..token(g.socket_available).."/"..token(g.socket)
                    local tags={};for _,v in ipairs(g.tags.values or {})do tags[#tags+1]=quote_tag(v)end
                    parts[#parts+1]="tags:"..token(g.tags.available).."/"..token(g.tags.count).."/["..table.concat(tags,",").."]"
                    parts[#parts+1]="tags_encoding:json|tags_byte_limit:"..M.SEVER_TAG_BYTES
                        .."|tags_truncated:"..token(g.tags.truncated).."|tags_reason:"..token(g.tags.reason)
                    parts[#parts+1]="position:"..vec(g.transform.translation,{"X","Y","Z"})
                    parts[#parts+1]="quat:"..vec(g.transform.rotation,{"X","Y","Z","W"})
                    parts[#parts+1]="scale:"..vec(g.transform.scale,{"X","Y","Z"})
                    parts[#parts+1]="collision:"..token(g.collision_available).."/"..token(g.collision)
                    if g.extent then parts[#parts+1]="unscaled_extent:"..vec(g.extent,{"X","Y","Z"})end
                    if g.asset then
                        parts[#parts+1]="asset:"..reference(g.asset)
                        parts[#parts+1]="physics_asset:"..reference(g.physics_asset)
                    end
                end
                return table.concat(parts,"|")
            end
            for _,key in ipairs({"master","attach_marker","box1","box2","weapon"})do
                rows[#rows+1]="sever_"..key.."="..geom(s[key])
            end
            put("sever_markers_read_complete",s.overlapped_markers.read_complete)
            collection("sever_markers",s.overlapped_markers,function(v)return v.index..":"..geom(v)end)
        end
        o.log("%s",table.concat(rows," "))
    end
    local function capture(w,event,meta) local r=snapshot(w,event,meta);emit(r);return r end
    local function sever(event,selfp,masterp,partp,attachp,markersp,box1p,box2p,weaponp)
        if not o.enabled() then return end
        local w=o.unwrap(selfp)
        local current_context=o.source_context or o.context
        local meta={pre="unavailable:Blueprint_POST"}
        local r=snapshot(w,event,meta,current_context);if not r then return end
        r.sever_inputs=sever_inputs(w,{masterp,partp,attachp,markersp,box1p,box2p,weaponp},r,current_context)
        if not r.sever_inputs then return end
        local s=r.sever_inputs
        meta.part,meta.master,meta.weapon=s.part.value,s.master.identity and s.master.identity.address,s.weapon.identity and s.weapon.identity.address
        -- This remains an observed POST-pair hint, not a causal native
        -- operation ID or proof that Initiate passed its native gates.
        if s.part.available and s.master.available then
            local key=r.key..":"..token(meta.master)..":"..token(meta.part)
            if event==M.HOOKS[1] then starts[key]=r.seq;meta.pair=r.seq
            else meta.pair=starts[key];starts[key]=nil end
        end
        emit(r)
        return r
    end
    return {capture=capture,snapshot=snapshot,sever_snapshot=sever,
        install=function(register)
            for _,event in ipairs(M.HOOKS)do
                if not installed[event] and not ambiguous[event] then
                    -- LuaMod.cpp strips the first "Function " anywhere in the
                    -- argument. An explicit leading prefix preserves names that
                    -- themselves contain it, such as Dismember Function Initiate.
                    local path="Function /Game/Character/Blueprints/Willie_BP.Willie_BP_C:"..event
                    local ok,pre_id,post_id=pcall(register,path,
                        function(...)local args=table.pack(...);safe(function()sever(event,table.unpack(args,1,args.n))end)end)
                    -- Pinned LuaMod.cpp returns the same int32 ID twice for a
                    -- Blueprint POST hook. A successful call without those IDs
                    -- might already have installed a hook: do not retry blindly.
                    local proven=ok and hook_id(pre_id) and hook_id(post_id) and pre_id==post_id
                    installed[event]=proven==true
                    ambiguous[event]=ok and not installed[event]
                    local err=not ok and token(tostring(pre_id))
                        or (installed[event] and "none" or "unavailable:Blueprint_hook_ids")
                    -- Retry failed lookups, but retain the actual engine exception
                    -- once per distinct failure instead of flooding every tick.
                    if ok or hook_errors[event]~=err then
                        o.log("LAB_BODY_HOOK event=%s invoked=%s registered=%s ambiguous=%s path=%q pre_id=%s post_id=%s error=%s convention=Blueprint_POST pre=unavailable",
                            token(event),token(ok),ambiguous[event] and "unavailable" or token(installed[event]),token(ambiguous[event]),path,ok and token(pre_id) or "unavailable",
                            ok and token(post_id) or "unavailable",err)
                    end
                    hook_errors[event]=err
                end
            end
            return installed[M.HOOKS[1]] and installed[M.HOOKS[2]]
        end,
        clear=function()starts={};active_scope=nil end}
end
return M
