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
    "Dismemberment In Process","Force Disable Dismemberment","Force Disable Vertex Paint"}
-- These are actual skeleton names, queried independently. They are not a
-- mapping from the native 15-part enum to the 23-bit network bone mask.
M.BONES={"pelvis","spine_01","spine_02","spine_03","spine_04","spine_05","neck_01","neck_02","head",
    "clavicle_l","upperarm_l","lowerarm_l","hand_l","clavicle_r","upperarm_r","lowerarm_r","hand_r",
    "thigh_l","calf_l","foot_l","thigh_r","calf_r","foot_r"}
M.CONSTRAINTS={"Dislocated Bone Constraint Arm R","Dislocated Bone Constraint Arm L","Dislocated Bone Constraint Leg R",
    "Dislocated Bone Constraint Leg L","Dislocated Bone Constraint Neck","Dislocated Bone Constraint Back"}
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
    local function parts_map(a)
        if not a then return {available=false} end
        local values,count={},0
        local ok=pcall(function()a:ForEach(function(k,v)
            count=count+1;if count>15 then error("bounded map exceeded") end
            k,v=number(o.unwrap(k)),boolean(o.unwrap(v))
            if not k or k%1~=0 or k<0 or k>14 or v==nil then error("unreadable typed part map") end
            values[#values+1]={part=k,value=v}
        end)end)
        if ok then table.sort(values,function(a,b)return a.part<b.part end) end
        return {available=ok,count=ok and count or nil,values=ok and values or nil,truncated=count>15}
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
    local function snapshot(w,event,meta)
        if not o.enabled() then return nil end
        local c=safe(function()return o.context(w)end)
        local key=scope_key(c);if not key then return nil end
        if address(w)~=c.actor then return nil end
        local mesh=field(w,"Mesh")
        if address(mesh)~=c.mesh then return nil end
        if active_scope~=key then starts={};active_scope=key end
        seq=seq+1
        local r={seq=seq,event=event,context=c,key=key,meta=meta or {},health={},flags={},components={},constraints={}}
        for _,k in ipairs(M.HEALTH)do r.health[k]=number(field(w,k))end
        for _,k in ipairs(M.FLAGS)do r.flags[k]=boolean(field(w,k))end
        r.dism_array=array(field(w,"Dismembered Array"),fname)
        r.dism_bones=array(field(w,"Dismembered Bones"),fname)
        r.parts=parts_map(field(w,"Dismembered Parts Map"))
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
        local after=safe(function()return o.context(w)end)
        if scope_key(after)~=key then return nil end
        return r
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
        local function collection(k,a,convert)
            put(k.."_available",a.available);put(k.."_n",a.count);put(k.."_truncated",a.truncated)
            local values={};for _,v in ipairs(a.values or {})do values[#values+1]=convert(v)end
            rows[#rows+1]=k.."="..(a.available and ("["..table.concat(values,",").."]") or "unavailable")
        end
        collection("dism_array",r.dism_array,token);collection("dism_bones",r.dism_bones,token)
        collection("parts",r.parts,function(v)return v.part..":"..token(v.value)end);put("spawn_bone",r.spawn_bone)
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
        o.log("%s",table.concat(rows," "))
    end
    local function capture(w,event,meta) local r=snapshot(w,event,meta);emit(r);return r end
    local function sever(event,selfp,masterp,partp,_,__,___,____,weaponp)
        if not o.enabled() then return end
        local w,master,part,weapon=o.unwrap(selfp),o.unwrap(masterp),number(o.unwrap(partp)),o.unwrap(weaponp)
        local meta={part=part,master=address(master),weapon=address(weapon),pre="unavailable:Blueprint_POST"}
        local r=snapshot(w,event,meta);if not r then return end
        local key=r.key..":"..token(meta.master)..":"..token(part)
        if event==M.HOOKS[1] then starts[key]=r.seq;meta.pair=r.seq
        else meta.pair=starts[key];starts[key]=nil end
        emit(r)
    end
    return {capture=capture,snapshot=snapshot,
        install=function(register)
            for _,event in ipairs(M.HOOKS)do
                if not installed[event] and not ambiguous[event] then
                    local path="/Game/Character/Blueprints/Willie_BP.Willie_BP_C:"..event
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
