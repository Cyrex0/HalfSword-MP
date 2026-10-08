-- Developer-only configuration evidence. Own source and remote proxy are
-- different peers; counterpart rows must be joined across clients, not by time.
local M={MAX_ATTEMPTS=3,JOINTS={{"UserConstraint_10","clavicle_r","upperarm_r"},
    {"UserConstraint_11","upperarm_r","lowerarm_r"},{"UserConstraint_12","lowerarm_r","hand_r"}}}
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
    local ref={Owner=weak,Index=index};local p,c={},{}
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
local function joint(e,j)
    local ref,before=accessor(e,j);local r={name=j[1],binding=before,complete=true}
    for _,key in ipairs(order)do
        assert(e.current(),"scope changed")
        attempt(r,key,function()
            local s=spec[key];local args={};for i=1,#s[2]do args[i]={}end
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
    local _,after=accessor(e,j)
    assert(same(before.owner,after.owner)and before.index==after.index and before.parent==after.parent and before.child==after.child,"accessor changed")
    return r
end
local function row(c,e,all_current)
    local lost=false;local old=e.current
    e=setmetatable({current=function()
        if lost then return false end
        local ok,v=pcall(old)
        if not ok or v~=true then lost=true;return false end
        local ok2,v2=pcall(all_current)
        if not ok2 or v2~=true then lost=true;return false end
        return true
    end},{__index=e})
    assert(e.current(),"scope changed")
    local r=copy(c);r.admission_ms=r.observed_ms;r.observed_ms=number(e.now())
    r.side="right";r.phase="pre_driver_configuration_observation";r.authority=false;r.joints={}
    local before=asset(e.mesh,e);assert(e.current(),"scope changed");r.asset=before
    r.hand={}
    for _,field in ipairs({"R_GripType_Current","L_GripType_Current","R Two Handed Grip","L Hand In Offhand Attached"})do
        assert(e.current(),"scope changed")
        attempt(r.hand,field,function()
            local v=e.actor[field]
            if field:find("GripType",1,true)then number(v);assert(math.tointeger(v)and v>=0 and v<=255,"grip unavailable")
            else assert(type(v)=="boolean","hand flag unavailable")end
            return v
        end)
        assert(e.current(),"scope changed")
    end
    for _,j in ipairs(M.JOINTS)do
        assert(e.current(),"scope changed")
        local jr={name=j[1]};attempt(jr,"current",function()return joint(e,j)end);r.joints[#r.joints+1]=jr
        assert(e.current(),"scope changed")
    end
    assert(e.current(),"scope changed");local after=asset(e.mesh,e);assert(e.current(),"scope changed")
    assert(equal_asset(before,after),"asset changed")
    r.observed_end_ms=number(e.now());assert(r.observed_end_ms>=r.observed_ms,"observation clock regressed")
    return copy(r)
end
function M.new(emit)
    local s={attempts=0,used=false}
    function s:attempt()
        if self.used or self.attempts>=M.MAX_ATTEMPTS then return false end
        self.attempts=self.attempts+1;return true
    end
    function s:capture(source,proxy,se,pe)
        if self.used or self.attempts<1 then return nil end
        self.used=true -- all optional native reads, including failed reads, are once-only.
        local lost=false
        local function current()
            if lost then return false end
            local ok,a=pcall(se.current)
            if not ok or a~=true then lost=true;return false end
            local ok2,b=pcall(pe.current)
            if not ok2 or b~=true then lost=true;return false end
            return true
        end
        local ok,result=pcall(function()
            assert(source.peer~=proxy.peer,"source and proxy peer must differ")
            for _,k in ipairs({"world","generation","match_id","round"})do
                assert(source[k]~=nil and source[k]==proxy[k],"source/proxy scope disagreement")
            end
            local a=row(source,se,current);assert(current(),"scope changed")
            local b=row(proxy,pe,current);assert(current(),"scope changed")
            return {instance=source.instance,coverage="right_only",comparison="different_peers_same_process; correlate counterpart peer across clients",
                temporal_pairing=false,authority=false,source=a,proxy=b,observation_separation_ms=b.observed_ms-a.observed_ms}
        end)
        if not ok then return nil,tostring(result):sub(1,120)end
        pcall(emit,result);return result
    end
    return s
end
return M
