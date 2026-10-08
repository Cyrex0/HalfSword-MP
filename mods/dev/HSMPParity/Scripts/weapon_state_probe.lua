-- Dev-only synchronous native weapon/constraint readback. No setters, cached
-- UObjects or guessed offsets; every returned object is copied identity data.
local M = {}
local function valid(o) return o and o:IsValid() == true end
local function identity(o)
    if not valid(o) then return nil end
    return {name=o:GetFName():ToString(),address=o:GetAddress(),class=o:GetClass():GetFName():ToString()}
end
local function vec(v) return {v.X,v.Y,v.Z} end
local function attempt(row,key,fn)
    local ok,v=pcall(fn)
    if ok then row[key]=v else row.errors[key]=tostring(v) end
end
local function outp(name,...)
    for i=1,select("#",...) do local t=select(i,...); if type(t)=="table" and t[name]~=nil then return t[name] end end
    error("native out-param "..name.." not written")
end
local function component(c,e)
    local row={id=identity(c),errors={}}
    if not row.id then return row end
    attempt(row,"simulating",function() return c:IsSimulatingPhysics(e.fname("None")) end)
    attempt(row,"collision",function() return c:GetCollisionEnabled() end)
    attempt(row,"owner",function() return identity(c:GetOwner()) end)
    attempt(row,"attach",function() return identity(c:GetAttachParent()) end)
    attempt(row,"socket",function() return c:GetAttachSocketName():ToString() end)
    attempt(row,"mass",function() return c:GetMass() end)
    attempt(row,"frame",function()
        local t=c:GetSocketTransform(e.fname("None"),0); local q=t.Rotation
        return {p=vec(t.Translation),q={q.X,q.Y,q.Z,q.W}}
    end)
    return row
end
local function components(actor,class,e)
    local rows=actor:K2_GetComponentsByClass(e.class(class))
    assert(type(rows)=="table" and #rows<=64,"native component array unavailable/over capacity")
    local out={}
    for _,wrapped in ipairs(rows) do
        local ok,c=pcall(function() return wrapped:get() end)
        if not ok then c=wrapped end
        if valid(c) then out[#out+1]=c end
    end
    return out
end
local hand_fields={"R_GripType_Current","L_GripType_Current","R Two Handed Grip","L Hand In Offhand Attached"}
local hand_constraints={"PhysicsConstraint R Hand","PhysicsConstraint L Hand"}
local function hand_state(actor,context,e)
    local function unavailable(reason)
        return {available=false,scope_available=false,reason=reason,fields={},constraints={},instance=e.instance or "unavailable"}
    end
    if type(e.hand_current)~="function" then return unavailable("guard_unavailable") end
    local function current(stage)
        local ok,yes,why=pcall(e.hand_current,context,actor)
        return ok and yes==true,ok and (why or stage) or stage.."_exception"
    end
    local ok,why=current("scope_start")
    if not ok then return unavailable(why) end
    local out={available=true,scope_available=true,reason="none",fields={},constraints={},instance=e.instance or "unavailable"}
    for i,field in ipairs(hand_fields)do
        local read,value=pcall(function()return actor[field]end)
        local known=read and (i<=2 and type(value)=="number" and value>=0 and value<=255 and value%1==0
            or i>2 and type(value)=="boolean")
        out.fields[field]={available=known==true,reason=known and "none" or read and "type_unavailable" or "read_failed"}
        -- A real false is evidence, not an unavailable/default value.
        if known then out.fields[field].value=value else out.available=false end
    end
    for _,field in ipairs(hand_constraints)do
        local read,value=pcall(function()
            local v=identity(actor[field])
            if not v or type(v.address)~="number" or v.address<=0 or v.address>=math.huge or v.address%1~=0
                or type(v.name)~="string" or v.name=="" or type(v.class)~="string" or v.class=="" then return nil end
            return v
        end)
        out.constraints[field]={available=read and value~=nil,id=read and value or nil,
            reason=read and value and "none" or read and "identity_unavailable" or "read_failed"}
        if not read or not value then out.available=false end
    end
    ok,why=current("scope_end")
    if not ok then return unavailable(why) end -- Discard partial observations after scope/header loss.
    if not out.available then out.reason="partial_unavailable" end
    return out
end
function M.capture(actor,context,e)
    if not e.current(context,actor) then return nil,"original_context_unverified" end
    local result={pawn=identity(actor),context=context,weapons={},constraints={},handles={},errors={},read_only=true}
    local actors={actor}
    for _,field in ipairs({"Weapon R","Weapon L"}) do
        local row={field=field,errors={}}
        attempt(row,"actor",function()
            local weapon=actor[field]
            local id=identity(weapon)
            if not id then return nil end
            actors[#actors+1]=weapon
            row.root=component(weapon.RootComponent,e)
            row.base=component(weapon.BaseMesh,e)
            attempt(row,"held",function() return weapon["Is Held"] end)
            attempt(row,"grip_r",function() return weapon["Grip R Hand Default"] end)
            attempt(row,"grip_l",function() return weapon["Grip L Hand Default"] end)
            return id
        end)
        result.weapons[#result.weapons+1]=row
    end
    for _,owner in ipairs(actors) do
        local ok,err=pcall(function()
            for _,c in ipairs(components(owner,"PhysicsConstraintComponent",e)) do
                local row={id=identity(c),owner=identity(owner),errors={}}
                attempt(row,"endpoints",function()
                    local a,b,x,y={},{},{},{}
                    c:GetConstrainedComponents(a,b,x,y)
                    return {one=identity(outp("OutComponent1",a,b,x,y)),bone1=outp("OutBoneName1",a,b,x,y):ToString(),
                        two=identity(outp("OutComponent2",a,b,x,y)),bone2=outp("OutBoneName2",a,b,x,y):ToString()}
                end)
                attempt(row,"reference",function()
                    local ci=c.ConstraintInstance
                    return {bone1=ci.ConstraintBone1:ToString(),bone2=ci.ConstraintBone2:ToString(),
                        pos1=vec(ci.Pos1),pri1=vec(ci.PriAxis1),sec1=vec(ci.SecAxis1),pos2=vec(ci.Pos2),pri2=vec(ci.PriAxis2),sec2=vec(ci.SecAxis2)}
                end)
                attempt(row,"profile",function()
                    local pi=c.ConstraintInstance.ProfileInstance
                    local d,L,C,T=pi.AngularDrive.SlerpDrive,pi.LinearLimit,pi.ConeLimit,pi.TwistLimit
                    return {strength=d.Stiffness,damping=d.Damping,max_force=d.MaxForce,
                        x=L.XMotion,y=L.YMotion,z=L.ZMotion,linear=L.Limit,
                        swing1_motion=C.Swing1Motion,swing1=C.Swing1LimitDegrees,swing2_motion=C.Swing2Motion,swing2=C.Swing2LimitDegrees,
                        twist_motion=T.TwistMotion,twist=T.TwistLimitDegrees}
                end)
                result.constraints[#result.constraints+1]=row
            end
            for _,handle in ipairs(components(owner,"PhysicsHandleComponent",e)) do
                local row={id=identity(handle),owner=identity(owner),errors={}}
                attempt(row,"grabbed",function() return identity(handle:GetGrabbedComponent()) end)
                result.handles[#result.handles+1]=row
            end
        end)
        if not ok then result.errors[#result.errors+1]=tostring(err) end
    end
    result.hand_state=hand_state(actor,context,e)
    if not e.current(context,actor) then return nil,"original_context_changed" end
    return result
end
local function id(o) return o and (o.name.."@"..tostring(o.address)..":"..tostring(o.class)) or "unavailable" end
local function values(v)
    if not v then return "unavailable" end
    local out={};for i,x in ipairs(v) do out[i]=tostring(x) end
    return "("..table.concat(out,",")..")"
end
function M.emit(r,log)
    local c=r.context
    log("WEAPONSTATE peer=%s pawn=%s match=%s round=%s life=%s world=%s read_only=true",tostring(c.peer),id(r.pawn),tostring(c.match_id),tostring(c.round),tostring(c.life),tostring(c.world))
    local h=r.hand_state
    if h then
        log("WPNHAND_CONTEXT inst=%s peer=%s pawn=%s mesh=%s mesh_address=%s match=%s round=%s life=%s world=%s scope_available=%s complete=%s reason=%s authority=false read_only=true",
            h.instance,tostring(c.peer),id(r.pawn),tostring(c.mesh),tostring(c.mesh_address),tostring(c.match_id),tostring(c.round),tostring(c.life),tostring(c.world),
            tostring(h.scope_available),tostring(h.available),h.reason)
        for _,field in ipairs(hand_fields)do
            local f=h.fields[field]
            log("WPNHAND_FLAG inst=%s peer=%s field=%s available=%s value=%s reason=%s",h.instance,tostring(c.peer),field,tostring(f and f.available or false),
                f and f.available and tostring(f.value) or "unavailable",f and f.reason or h.reason)
        end
        for _,field in ipairs(hand_constraints)do
            local f=h.constraints[field]
            log("WPNHAND_CURRENT inst=%s peer=%s field=%s available=%s component=%s reason=%s",h.instance,tostring(c.peer),field,tostring(f and f.available or false),
                id(f and f.id),f and f.reason or h.reason)
        end
    end
    for _,w in ipairs(r.weapons) do
        log("WPNSTATE field=%s actor=%s held=%s grip_r=%s grip_l=%s",w.field,id(w.actor),tostring(w.held),tostring(w.grip_r),tostring(w.grip_l))
        for _,part in ipairs({"root","base"}) do
            local p=w[part]
            if p then
                log("WPNCOMP field=%s part=%s component=%s actual_sim=%s collision=%s mass=%s owner=%s attach=%s socket=%s p=%s q=%s",
                    w.field,part,id(p.id),tostring(p.simulating),tostring(p.collision),tostring(p.mass),id(p.owner),id(p.attach),tostring(p.socket),values(p.frame and p.frame.p),values(p.frame and p.frame.q))
                for k,why in pairs(p.errors) do log("WPNSTATE_ERROR field=%s part=%s property=%s reason=%s",w.field,part,k,why) end
            end
        end
        for k,why in pairs(w.errors) do log("WPNSTATE_ERROR field=%s property=%s reason=%s",w.field,k,why) end
    end
    for _,g in ipairs(r.constraints) do
        local ep,p,ref=g.endpoints,g.profile,g.reference
        log("WPNCONSTRAINT component=%s owner=%s one=%s bone1=%s two=%s bone2=%s strength=%s damping=%s force=%s linear=%s/%s/%s/%s swing=%s/%s/%s/%s twist=%s/%s",
            id(g.id),id(g.owner),id(ep and ep.one),tostring(ep and ep.bone1),id(ep and ep.two),tostring(ep and ep.bone2),
            tostring(p and p.strength),tostring(p and p.damping),tostring(p and p.max_force),tostring(p and p.x),tostring(p and p.y),tostring(p and p.z),tostring(p and p.linear),
            tostring(p and p.swing1_motion),tostring(p and p.swing1),tostring(p and p.swing2_motion),tostring(p and p.swing2),tostring(p and p.twist_motion),tostring(p and p.twist))
        if ref then log("WPNREFERENCE component=%s bone1=%s pos1=%s pri1=%s sec1=%s bone2=%s pos2=%s pri2=%s sec2=%s",
            id(g.id),ref.bone1,values(ref.pos1),values(ref.pri1),values(ref.sec1),ref.bone2,values(ref.pos2),values(ref.pri2),values(ref.sec2)) end
        for k,why in pairs(g.errors) do log("WPNSTATE_ERROR component=%s property=%s reason=%s",id(g.id),k,why) end
    end
    for _,h in ipairs(r.handles) do
        log("WPNHANDLE component=%s owner=%s grabbed=%s",id(h.id),id(h.owner),id(h.grabbed))
        for k,why in pairs(h.errors) do log("WPNSTATE_ERROR component=%s property=%s reason=%s",id(h.id),k,why) end
    end
    for _,why in ipairs(r.errors) do log("WPNSTATE_ERROR reason=%s",why) end
end
return M
