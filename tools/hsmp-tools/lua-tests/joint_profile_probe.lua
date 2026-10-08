local P=dofile(T.path("mods/HSMPAvatars/Scripts/joint_profile_probe.lua"))
local function fixture()
    local x={records={},reads=0,current=true,time=100}
    local function obj(name,addr)
        return {IsValid=function()x.reads=x.reads+1;return true end,GetAddress=function()x.reads=x.reads+1;return addr end,
            GetFName=function()x.reads=x.reads+1;return {ToString=function()return name end}end}
    end
    local function env(peer)
        local e={fname=function(v)return v end,now=function()x.time=x.time+1;return x.time end,current=function()
            if x.throw_current then error("scope unavailable")end;return x.current end}
        e.actor=obj("Pawn"..peer,peer*10);e.mesh=obj("Mesh"..peer,peer*10+1)
        local sk,pa=obj("Body",99),obj("Asset",98)
        sk.GetPhysicsAsset=function()
            x.reads=x.reads+1;if x.asset_flip then x.current=false end;return pa
        end
        e.mesh.SkeletalMesh=sk;e.mesh.PhysicsAssetOverride={GetAddress=function()return 0 end}
        local binding={};for i,j in ipairs(P.JOINTS)do binding[j[1]]={index=i+7,parent=j[2],child=j[3]}end
        e.mesh.GetConstraintByName=function(_,name)
            x.reads=x.reads+1;local b=assert(binding[name]);return {Owner={get=function()return e.mesh end},Index=x.change_index and b.index+1 or b.index}
        end
        e.actor.R_GripType_Current=peer==1 and 14 or 3;e.actor.L_GripType_Current=0
        e.actor["R Two Handed Grip"]=peer==1;e.actor["L Hand In Offhand Attached"]=false
        e.library={GetAttachedBodyNames=function(_,a,p,c)
            x.reads=x.reads+1;for _,b in pairs(binding)do if a.Index==b.index then
                p.ParentBody={ToString=function()return b.parent end};c.ChildBody={ToString=function()return x.bad_endpoint and "head"or b.child end};return end end
        end}
        local methods={GetAngularLimits={"Swing1MotionType","Swing1LimitAngle","Swing2MotionType","Swing2LimitAngle","TwistMotionType","TwistLimitAngle"},
            GetAngularDriveParams={"OutPositionStrength","OutVelocityStrength","OutForceLimit"},
            GetAngularSoftSwingLimitParams={"bSoftSwingLimit","SwingLimitStiffness","SwingLimitDamping","SwingLimitRestitution","SwingLimitContactDistance"},
            GetAngularSoftTwistLimitParams={"bSoftTwistLimit","TwistLimitStiffness","TwistLimitDamping","TwistLimitRestitution","TwistLimitContactDistance"},
            GetProjectionParams={"bEnableProjection","ProjectionLinearAlpha","ProjectionAngularAlpha"}}
        for name,fields in pairs(methods)do e.library[name]=function(_,ref,...)
            x.reads=x.reads+1;x.calls=x.calls or {};x.calls[#x.calls+1]=name
            local args={...};for i,k in ipairs(fields)do args[i][k]=k:sub(1,1)=="b"and false or 0 end
            -- Avoid the Lua false-or-zero trap in the fixture too.
            for i,k in ipairs(fields)do if k:sub(1,1)=="b"then args[i][k]=false end end
            if x.nonfinite and name=="GetProjectionParams"then args[2].ProjectionLinearAlpha=0/0 end
            if x.getter_flip and name=="GetAngularLimits"then x.current=false end
            if x.getter_throw and name=="GetAngularLimits"then x.throw_current=true end
        end end
        return e
    end
    x.source=env(1);x.proxy=env(2)
    local function context(peer)
        return {instance="1",world="gen|World",generation=1,match_id=9,round=1,life=peer,peer=peer,
            pawn={address=peer*10,name="Pawn"..peer},mesh={address=peer*10+1,name="Mesh"..peer},observed_ms=90,
            sample_ms=80,qualification=peer==1,role=peer==1 and "local_source"or "remote_proxy"}
    end
    x.sc,x.pc=context(1),context(2);x.probe=P.new(function(row)x.records[#x.records+1]=row end)
    function x:run()self.probe:attempt();return self.probe:capture(self.sc,self.pc,self.source,self.proxy)end
    return x
end
local x=fixture();local r=x:run()
T.check(r and #r.source.joints==3 and #r.proxy.joints==3,"one pair captures the three exact right joints for each independent peer")
T.check(r and r.source.peer==1 and r.proxy.peer==2 and r.source.life==1 and r.proxy.life==2 and not r.temporal_pairing,
    "different peers and legitimate different lives are explicit, not a same-owner or time pairing")
T.check(r and r.observation_separation_ms>0 and r.source.observed_ms~=r.source.admission_ms,
    "actual row start times differ from preparation and from the successful pose timestamp")
T.check(r.source.joints[1].current.value.binding.index==8,"native accessor index comes from returned accessor, not suffix10")
T.check(r.source.joints[1].current.value.soft_swing.value.bSoftSwingLimit==false
    and r.source.joints[1].current.value.angular_drive.value.OutPositionStrength==0,"typed false and zero remain available")
T.check(r.source.hand.R_GripType_Current.value==14 and r.proxy.hand.R_GripType_Current.value==3,"actual native hand scalars are independently preserved")
local reads=x.reads;x:run();T.check(x.reads==reads and #x.records==1,"one whole-run pair cannot be retriggered")
x.source.actor.R_GripType_Current=9;T.check(r.source.hand.R_GripType_Current.value==14,"output owns scalar values")
for _,field in ipairs({"world","generation","match_id","round"})do
    local e=fixture();e.pc[field]=field=="world"and "Other"or 99
    T.check(e:run()==nil and e.reads==0,"different "..field.." refuses before native reads")
end
local e=fixture();e.pc.peer=1;T.check(e:run()==nil and e.reads==0,"same peer cannot masquerade as source/proxy comparison")
e=fixture();e.asset_flip=true;T.check(e:run()==nil and #e.records==0,"asset lookup scope loss refuses row")
T.check(e.reads==1,"asset lookup loss prevents subsequent asset identity/native reads")
for _,flag in ipairs({"getter_flip","getter_throw"})do
    e=fixture();e[flag]=true;T.check(e:run()==nil and #e.records==0,"first getter "..flag.." refuses pair")
    T.check(#e.calls==1 and e.calls[1]=="GetAngularLimits","scope loss prevents every later getter")
    local before=e.reads;e.current=true;e.throw_current=false;e:run();T.check(e.reads==before,"synthetic recovered scope cannot reopen used capture")
end
e=fixture();e.proxy.library.GetAngularSoftSwingLimitParams=nil;r=e:run()
T.check(r and not r.proxy.joints[1].current.value.soft_swing.available and not r.proxy.joints[1].current.value.complete,
    "missing getter remains explicit partial evidence")
e=fixture();e.nonfinite=true;r=e:run();T.check(r and not r.source.joints[1].current.value.projection.available,"nonfinite output is unavailable")
e=fixture();e.bad_endpoint=true;r=e:run();T.check(r and not r.source.joints[1].current.available,"wrong endpoint never produces a qualified accessor")
e=fixture();e.source.actor["R Two Handed Grip"]=nil;r=e:run();T.check(r and not r.source.hand["R Two Handed Grip"].available,"nil native flag is not invented false")
e=fixture();for i=1,3 do T.check(e.probe:attempt(),"bounded admission "..i)end
T.check(not e.probe:attempt()and e.reads==0,"refused context attempts stop before optional reads at three")
e=fixture();local proxy_checks=0
e.source.current=function()return false end;e.proxy.current=function()proxy_checks=proxy_checks+1;return true end
T.check(e:run()==nil and proxy_checks==0 and e.reads==0,"first lost source guard short-circuits every proxy validator/native read")
e=fixture();local first=e.source.library.GetAngularLimits;e.source.library.GetAngularLimits=nil
setmetatable(e.source.library,{__index=function(_,name)if name=="GetAngularLimits"then e.current=false;return first end end})
T.check(e:run()==nil and not e.calls,"getter lookup scope loss stops before invoking returned getter")
e=fixture();e.asset_flip=true
local value,reason,detail=e:run()
T.check(value==nil and reason:find("scope changed",1,true)and detail.stage=="source:asset_before",
    "actual helper refusal carries its bounded reason and static failed asset stage")
T.check(detail.elapsed_available and detail.capture_elapsed_ms>=0,"total capture failure interval uses the same observation clock")
T.check(P.reason("native\nread\tfailed\0"..string.rep("x",200)):find("[%c]")==nil
    and #P.reason(string.rep("x",200))==120,"persisted reason sanitizes control characters and bounds text")
e=fixture();e.sc.match_id=999;e.source.now=function()error("clock unavailable")end
value,reason,detail=e:run()
T.check(value==nil and detail.stage=="pair:scope"and detail.elapsed_available==false and detail.capture_elapsed_ms==nil,
    "unavailable diagnostic clock does not invent an elapsed interval or suppress actual refusal")
