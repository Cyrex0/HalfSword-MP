-- Developer-only configuration evidence. Own source and remote proxy are
-- different peers; counterpart rows must be joined across clients, not by time.
local M={MAX_ATTEMPTS=3,RETRY_MS=5000,JOINTS={{"UserConstraint_10","clavicle_r","upperarm_r"},
    {"UserConstraint_11","upperarm_r","lowerarm_r"},{"UserConstraint_12","lowerarm_r","hand_r"}}}
-- Cooked primary asset and native limb capture agree on this name/endpoints.
-- The runtime accessor still proves its owner, index and both actual bodies.
M.LEFT_UPPERARM={{"UserConstraint_14","clavicle_l","upperarm_l"}}
M.RIGHT_HAND={{"UserConstraint_12","lowerarm_r","hand_r"}}
-- This is a copied trigger from an already completed physical measurement.
-- Native scope and current Mesh identity are independently rechecked at capture.
function M.right_fault(s,cur,shown,aim,body,world,generation,now,threshold)
    local function finite(v)return type(v)=="number"and v==v and math.abs(v)<math.huge end
    if type(s)~="table"or type(cur)~="table"or type(shown)~="table"or type(aim)~="table"or type(body)~="table"
        or s.enabled~=true or s.settle_count~=6 or s.settle_ready~=false
        or (s.settle_reason~="hand_r rotation"and s.settle_reason~="hand_r position")then return nil end
    local frames=body.stall and body.stall[17]
    if not finite(frames)or not finite(threshold)or frames<=threshold or not finite(now)
        or not finite(s.settle_sample_ms)or now-s.settle_sample_ms<0 or now-s.settle_sample_ms>250
        or not finite(cur.age)or math.abs(cur.age)>250 or (cur.mode~="interp"and cur.mode~="extrap")then return nil end
    if cur.has_context~=true or shown.has_context~=true or aim.has_context~=true or s.world~=world
        or aim.world~=world or s.pawn~=shown.pawn or s.pawn~=aim.pawn
        or s.probe_generation~=generation or s.probe_mesh_addr~=body.mesh_addr
        or s.probe_mesh_fname~=body.mesh_fname or type(body.mesh_addr)~="number"or body.mesh_addr<=0
        or type(body.mesh_fname)~="string"or body.mesh_fname==""or aim.at~=s.settle_sample_ms then return nil end
    for _,k in ipairs({"match_id","round","life","cut"})do
        if not finite(s[k])or s[k]~=cur[k]or s[k]~=shown[k]or s[k]~=aim[k]then return nil end
    end
    if not finite(s.settle_pos_uu)or not finite(s.settle_rot_deg)then return nil end
    return {kind="persistent_hand_r",sample_ms=s.settle_sample_ms,observed_ms=now,
        reason=s.settle_reason,frames=frames,threshold=threshold,six_limb_max_position_uu=s.settle_pos_uu,
        six_limb_max_rotation_deg=s.settle_rot_deg,source_seq=s.settle_source_seq,source_ts=s.settle_source_ts,
        measurement_mesh={address=s.probe_mesh_addr,name=s.probe_mesh_fname},generation=generation,
        pairing="fault-side observation; counterpart availability unproved"}
end
local function number(v)
    assert(type(v)=="number"and v==v and math.abs(v)<math.huge,"number unavailable");return v
end
local function guarded(e,fn)
    assert(e.current(),"scope changed");local v=fn();assert(e.current(),"scope changed");return v
end
local function id(o,e)
    assert(o and guarded(e,function()return o:IsValid()end)==true,"identity unavailable")
    local a=number(guarded(e,function()return o:GetAddress()end))
    local fn=guarded(e,function()return o:GetFName()end)
    local n=guarded(e,function()return fn:ToString()end)
    assert(math.tointeger(a)and a>0 and type(n)=="string"and #n>0 and #n<=256,"identity unavailable")
    return {address=a,name=n}
end
local function same(a,b)return a and b and a.address==b.address and a.name==b.name end
local function copy(v,depth)
    if type(v)=="table"then
        assert((depth or 0)<10,"copy depth");local t,n={},0
        for k,x in pairs(v)do n=n+1;assert(n<=64,"copy capacity");t[k]=copy(x,(depth or 0)+1)end
        return t
    end
    assert(v==nil or type(v)=="number"or type(v)=="boolean"or type(v)=="string","native wrapper in output")
    if type(v)=="number"then number(v)elseif type(v)=="string"then assert(#v<=2048,"text capacity")end
    return v
end
function M.reason(v)
    local ok,text=pcall(tostring,v)
    if not ok then return "diagnostic reason unavailable"end
    return text:gsub("[%c]"," "):sub(1,120)
end
M.AUDIT_FIELDS={"session_seq","session_match","session_round","session_phase","effective_round","pending",
    "spawn_id","spawn_peer","mode_present","mode_seq","mode_match","mode_round","mode_row_available",
    "mode_peer","mode_life","mode_available","qualification","qualification_reason"}
function M.failure(validator,predicate,field,expected,observed)
    local function scalar(v)
        if type(v)=="boolean"then return true,v,false end
        if type(v)=="string"then return true,M.reason(v),#v>120 end
        if type(v)=="number"and v==v and math.abs(v)<math.huge then return true,v,false end
        return false,nil,false
    end
    local ea,ev,et=scalar(expected);local oa,ov,ot=scalar(observed)
    return {validator=validator=="source"and "source"or validator=="proxy"and "proxy"or "unavailable",
        predicate=type(predicate)=="string"and M.reason(predicate)or "predicate unavailable",
        field=type(field)=="string"and M.reason(field)or "unavailable",
        expected_available=ea,expected=ev,expected_truncated=et,observed_available=oa,observed=ov,observed_truncated=ot}
end
local function attempt(r,key,fn)
    local ok,v=pcall(fn);r[key]=ok and {available=true,value=v}or {available=false,reason=tostring(v):sub(1,120)}
end
local function asset(mesh,e)
    local sk=guarded(e,function()return mesh.SkeletalMesh end)
    local override=guarded(e,function()return mesh.PhysicsAssetOverride end)
    local a=number(guarded(e,function()return override:GetAddress()end));assert(math.tointeger(a)and a>=0,"override unavailable")
    local pa=override
    if a==0 then pa=guarded(e,function()return sk:GetPhysicsAsset()end)end
    return {skeletal_mesh=id(sk,e),physics_asset=id(pa,e),override=a~=0}
end
local function equal_asset(a,b)return a.override==b.override and same(a.skeletal_mesh,b.skeletal_mesh)and same(a.physics_asset,b.physics_asset)end
local function output(t,key)
    local v=t[key];assert(v~=nil,"output unavailable: "..key);return v
end
local function accessor(e,j)
    assert(e.current(),"scope changed")
    local a=e.mesh:GetConstraintByName(e.fname(j[1]),false)
    assert(e.current(),"scope changed")
    local weak=guarded(e,function()return a.Owner end)
    local owner=id(guarded(e,function()return weak:get()end),e)
    assert(same(owner,id(e.mesh,e)),"accessor owner mismatch")
    local index=number(guarded(e,function()return a.Index end));assert(math.tointeger(index)and index>=0 and index<32,"index unavailable")
    -- Pinned LuaUObject.cpp doesn't consume scalar-out placeholders.
    -- Both exact FName output keys therefore share one copied container.
    local ref={Owner=weak,Index=index};local p={};local c=p
    assert(e.current(),"scope changed");e.library:GetAttachedBodyNames(ref,p,c)
    assert(e.current(),"scope changed")
    local parent=guarded(e,function()return output(p,"ParentBody"):ToString()end)
    local child=guarded(e,function()return output(c,"ChildBody"):ToString()end)
    assert(parent==j[2]and child==j[3],"endpoint mismatch")
    return ref,{owner=owner,index=index,parent=parent,child=child}
end
local spec={
    angular_limits={"GetAngularLimits",{"Swing1MotionType","Swing1LimitAngle","Swing2MotionType","Swing2LimitAngle","TwistMotionType","TwistLimitAngle"}},
    angular_drive={"GetAngularDriveParams",{"OutPositionStrength","OutVelocityStrength","OutForceLimit"}},
    soft_swing={"GetAngularSoftSwingLimitParams",{"bSoftSwingLimit","SwingLimitStiffness","SwingLimitDamping","SwingLimitRestitution","SwingLimitContactDistance"}},
    soft_twist={"GetAngularSoftTwistLimitParams",{"bSoftTwistLimit","TwistLimitStiffness","TwistLimitDamping","TwistLimitRestitution","TwistLimitContactDistance"}},
    projection={"GetProjectionParams",{"bEnableProjection","ProjectionLinearAlpha","ProjectionAngularAlpha"}},
}
local order={"angular_limits","angular_drive","soft_swing","soft_twist","projection"}
local function joint(e,j,label)
    e.stage(label..":accessor_before")
    local ref,before=accessor(e,j);local r={name=j[1],binding=before,complete=true}
    for _,key in ipairs(order)do
        e.stage(label..":"..key)
        assert(e.current(),"scope changed")
        attempt(r,key,function()
            local s=spec[key];local args,scalar_outputs={},{};for i=1,#s[2]do args[i]=scalar_outputs end
            local fn=guarded(e,function()return e.library[s[1]]end);assert(fn,"getter unavailable")
            assert(e.current(),"scope changed");fn(e.library,ref,table.unpack(args))
            assert(e.current(),"scope changed")
            local out={};for i,k in ipairs(s[2])do
                local v=output(args[i],k)
                if k:sub(1,1)=="b"then assert(type(v)=="boolean","boolean unavailable")else number(v)end
                out[k]=v
            end;return out
        end)
        assert(e.current(),"scope changed");r.complete=r.complete and r[key].available
    end
    if e.joint_angles==true then
        e.stage(label..":joint_angles")
        attempt(r,"joint_angles",function()
            -- Scalar outputs share one copied table in the pinned bridge. The
            -- returned angles are observations, not proof of target eligibility.
            local fn=guarded(e,function()return e.mesh.GetCurrentJointAngles end)
            assert(fn,"joint angle getter unavailable")
            local out={}
            assert(e.current(),"scope changed")
            fn(e.mesh,e.fname(j[1]),out,out,out)
            assert(e.current(),"scope changed")
            return {lookup_name=j[1],lookup_meaning="verified constraint name; native lookup behavior unproved",
                swing1=number(output(out,"Swing1Angle")),twist=number(output(out,"TwistAngle")),
                swing2=number(output(out,"Swing2Angle")),authority=false,physics_verified=false,
                units="native API output; no conversion"}
        end)
        assert(e.current(),"scope changed")
    end
    e.stage(label..":accessor_after")
    local _,after=accessor(e,j)
    assert(same(before.owner,after.owner)and before.index==after.index and before.parent==after.parent and before.child==after.child,"accessor changed")
    return r
end
local function row(c,e,all_current,stage,label,on_loss,selection,side)
    local lost=false;local old=e.current
    e=setmetatable({stage=stage,current=function()
        if lost then return false end
        local ok,v,why=pcall(old)
        if not ok or v~=true then lost=true;on_loss(label,ok,ok and why or v);return false end
        local ok2,v2=pcall(all_current)
        if not ok2 or v2~=true then lost=true;return false end
        return true
    end},{__index=e})
    stage(label..":scope_before")
    assert(e.current(),"scope changed")
    local r=copy(c);r.admission_ms=r.observed_ms;r.observed_ms=number(e.now())
    r.side=side;r.phase="pre_driver_configuration_observation";r.authority=false;r.joints={}
    stage(label..":asset_before")
    local before=asset(e.mesh,e);assert(e.current(),"scope changed");r.asset=before
    r.hand={}
    for _,field in ipairs({"R_GripType_Current","L_GripType_Current","R Two Handed Grip","L Hand In Offhand Attached"})do
        stage(label..":hand:"..field)
        assert(e.current(),"scope changed")
        attempt(r.hand,field,function()
            local v=e.actor[field]
            if field:find("GripType",1,true)then number(v);assert(math.tointeger(v)and v>=0 and v<=255,"grip unavailable")
            else assert(type(v)=="boolean","hand flag unavailable")end
            return v
        end)
        assert(e.current(),"scope changed")
    end
    if e.grip_flags then
        stage(label..":current_grip_flags")
        assert(e.current(),"scope changed")
        attempt(r,"grip_flags",function()return copy(guarded(e,e.grip_flags))end)
        assert(e.current(),"scope changed")
    end
    for _,j in ipairs(selection)do
        stage(label..":"..j[1])
        assert(e.current(),"scope changed")
        local jr={name=j[1]};attempt(jr,"current",function()return joint(e,j,label..":"..j[1])end);r.joints[#r.joints+1]=jr
        assert(e.current(),"scope changed")
    end
    stage(label..":asset_after")
    assert(e.current(),"scope changed");local after=asset(e.mesh,e);assert(e.current(),"scope changed")
    assert(equal_asset(before,after),"asset changed")
    r.observed_end_ms=number(e.now());assert(r.observed_end_ms>=r.observed_ms,"observation clock regressed")
    return copy(r)
end
function M.new(emit,focus,trigger)
    if trigger~=nil and trigger~=""and trigger~="warm"and trigger~="fault"then return nil,"unsupported joint trigger"end
    local selection,side,coverage
    if focus==nil or focus==""or focus=="right"then
        selection,side,coverage=copy(M.JOINTS),"right","right_only"
    elseif focus=="upperarm_l"then
        selection,side,coverage=copy(M.LEFT_UPPERARM),"left","left_upperarm_only"
    elseif focus=="hand_r"then
        selection,side,coverage=copy(M.RIGHT_HAND),"right","right_hand_only"
    else return nil,"unsupported joint focus"end
    local s={attempts=0,used=false,trigger=trigger=="fault"and "fault"or "warm"}
    local function finite(v)return type(v)=="number"and v==v and math.abs(v)<math.huge end
    local function retryable(f)
        if type(f)~="table"then return false end
        if f.validator=="source"and f.predicate=="source_sample_age"and f.field=="sample_age_ms"then
            return f.observed_available==true and finite(f.observed)and f.observed>250
        end
        local field=f.field
        return (f.validator=="source"or f.validator=="proxy")and f.predicate=="raw_scope_changed"
            and type(field)=="string"and (field:match("^raw%.session%.")~=nil
                or field:match("^raw%.mode%.")~=nil or field=="raw.mode"or field=="raw.mode_present")
    end
    function s:attempt(now)
        if self.used or self.attempts>=M.MAX_ATTEMPTS then return false end
        if self.trigger=="fault"and self.in_attempt then return false end
        if self.retry_pending and(not finite(now)or not finite(self.last_attempt_ms)
            or now-self.last_attempt_ms<M.RETRY_MS)then return false end
        self.retry_attempt=self.retry_pending==true;self.retry_pending=false
        self.last_attempt_ms=finite(now)and now or nil;self.in_attempt=true
        self.attempts=self.attempts+1;return true
    end
    function s:abandon()
        self.used=true;self.in_attempt=false;self.retry_pending=false;self.retry_attempt=false
    end
    function s:capture(source,proxy,se,pe)
        if self.used or self.attempts<1 or self.trigger=="fault"and not self.in_attempt then return nil end
        local retry=self.retry_attempt==true
        self.used=true;self.in_attempt=false;self.retry_attempt=false
        local stage="pair:scope"
        local function clock()
            local ok,v=pcall(se.now)
            return ok and type(v)=="number"and v==v and math.abs(v)<math.huge and v or nil
        end
        local started=clock()
        local lost,first_failure=false,nil
        local function on_loss(validator,ok,why)
            if first_failure then return end
            if not ok then
                first_failure=M.failure(validator,"validator_exception","exception",nil,type(why)=="string"and why or nil)
            elseif type(why)=="table"then
                local expected,observed
                if rawget(why,"expected_available")==true then expected=rawget(why,"expected")end
                if rawget(why,"observed_available")==true then observed=rawget(why,"observed")end
                first_failure=M.failure(validator,rawget(why,"predicate"),rawget(why,"field"),
                    expected,observed)
                first_failure.expected_truncated=first_failure.expected_truncated or rawget(why,"expected_truncated")==true
                first_failure.observed_truncated=first_failure.observed_truncated or rawget(why,"observed_truncated")==true
            else first_failure=M.failure(validator,"predicate unavailable","unavailable",nil,nil)end
        end
        local function current()
            if lost then return false end
            local ok,a,why=pcall(se.current)
            if not ok or a~=true then lost=true;on_loss("source",ok,ok and why or a);return false end
            local ok2,b,why2=pcall(pe.current)
            if not ok2 or b~=true then lost=true;on_loss("proxy",ok2,ok2 and why2 or b);return false end
            return true
        end
        local ok,result=pcall(function()
            assert(not retry or source.pending==true and proxy.pending==true,"retry requires current pending peers")
            assert(source.peer~=proxy.peer,"source and proxy peer must differ")
            for _,k in ipairs({"world","generation","match_id","round"})do
                assert(source[k]~=nil and source[k]==proxy[k],"source/proxy scope disagreement")
            end
            local function set_stage(value)stage=value end
            local a=row(source,se,current,set_stage,"source",on_loss,selection,side);assert(current(),"scope changed")
            local b=row(proxy,pe,current,set_stage,"proxy",on_loss,selection,side);assert(current(),"scope changed")
            stage="pair:complete"
            return {instance=source.instance,coverage=coverage,comparison="different_peers_same_process; correlate counterpart peer across clients",
                temporal_pairing=false,authority=false,source=a,proxy=b,observation_separation_ms=b.observed_ms-a.observed_ms}
        end)
        if not ok then
            local finished=clock();local available=started~=nil and finished~=nil and finished>=started
            local allowed=self.trigger=="fault"and source.pending==true and proxy.pending==true
                and self.attempts<M.MAX_ATTEMPTS and finite(self.last_attempt_ms)and retryable(first_failure)
            if allowed then self.used=false;self.retry_pending=true end
            -- Total capture-entry-to-refusal time, not a per-stage duration.
            return nil,M.reason(result),{stage=stage,elapsed_available=available,capture_elapsed_ms=available and finished-started or nil,
                first_failure=first_failure,retry_eligible=allowed}
        end
        pcall(emit,result);return result
    end
    return s
end
return M
