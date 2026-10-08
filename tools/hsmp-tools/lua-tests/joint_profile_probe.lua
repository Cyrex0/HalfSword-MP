local P=dofile(T.path("mods/HSMPAvatars/Scripts/joint_profile_probe.lua"))
local function fixture(focus)
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
        for i,j in ipairs(P.LEFT_UPPERARM)do binding[j[1]]={index=i+14,parent=j[2],child=j[3]}end
        e.mesh.GetConstraintByName=function(_,name)
            x.reads=x.reads+1;local b=assert(binding[name]);return {Owner={get=function()return e.mesh end},Index=x.change_index and b.index+1 or b.index}
        end
        e.actor.R_GripType_Current=peer==1 and 14 or 3;e.actor.L_GripType_Current=0
        e.actor["R Two Handed Grip"]=peer==1;e.actor["L Hand In Offhand Attached"]=false
        e.library={GetAttachedBodyNames=function(_,a,p,c)
            x.reads=x.reads+1;for _,b in pairs(binding)do if a.Index==b.index then
                p.ParentBody={ToString=function()return b.parent end}
                local child=x.first_scalar_only and p or c
                child.ChildBody={ToString=function()return x.bad_endpoint and "head"or b.child end};return end end
        end}
        local methods={GetAngularLimits={"Swing1MotionType","Swing1LimitAngle","Swing2MotionType","Swing2LimitAngle","TwistMotionType","TwistLimitAngle"},
            GetAngularDriveParams={"OutPositionStrength","OutVelocityStrength","OutForceLimit"},
            GetAngularSoftSwingLimitParams={"bSoftSwingLimit","SwingLimitStiffness","SwingLimitDamping","SwingLimitRestitution","SwingLimitContactDistance"},
            GetAngularSoftTwistLimitParams={"bSoftTwistLimit","TwistLimitStiffness","TwistLimitDamping","TwistLimitRestitution","TwistLimitContactDistance"},
            GetProjectionParams={"bEnableProjection","ProjectionLinearAlpha","ProjectionAngularAlpha"}}
        for name,fields in pairs(methods)do e.library[name]=function(_,ref,...)
            x.reads=x.reads+1;x.calls=x.calls or {};x.calls[#x.calls+1]=name
            local args={...};for i,k in ipairs(fields)do args[i][k]=k:sub(1,1)=="b"and false or 0 end
            if x.first_scalar_only then
                for i,k in ipairs(fields)do args[i][k]=nil end
                for _,k in ipairs(fields)do args[1][k]=k:sub(1,1)=="b"and false or 0 end
                for _,k in ipairs(fields)do if k:sub(1,1)=="b"then args[1][k]=false end end
            end
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
    x.sc,x.pc=context(1),context(2);x.probe=P.new(function(row)x.records[#x.records+1]=row end,focus)
    function x:run()self.probe:attempt();return self.probe:capture(self.sc,self.pc,self.source,self.proxy)end
    return x
end
local x=fixture();local r=x:run()
local fan_in=fixture();fan_in.first_scalar_only=true;local fused=fan_in:run()
local all_fused=fused~=nil
if fused then
    for _,side in ipairs({fused.source,fused.proxy})do
        for _,j in ipairs(side.joints)do all_fused=all_fused and j.current.available and j.current.value.complete end
    end
end
T.check(all_fused,"pinned first-table scalar copy-out supplies both endpoints and every joint group")
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
e=fixture();local source_lost=false
local failure=P.failure("source","source_sample_age","sample_age_ms","0..250",251.25)
e.source.current=function()if source_lost then return false,failure end;return true end
local prior=e.proxy.library.GetAngularLimits
e.proxy.library.GetAngularLimits=function(...)prior(...);source_lost=true end
value,reason,detail=e:run()
T.check(value==nil and detail.stage=="proxy:UserConstraint_10:angular_limits"
    and detail.first_failure.validator=="source"and detail.first_failure.predicate=="source_sample_age",
    "source-first loss during a proxy getter reports the source validator rather than inferring from stage")
T.check(detail.first_failure.observed_available and detail.first_failure.observed==251.25
    and detail.first_failure.expected=="0..250"and #e.calls==16 and #e.records==0,
    "first failed predicate uses copied existing sample age and stops every later getter")
failure.observed=0;T.check(detail.first_failure.observed==251.25,"failure output owns its bounded scalar data")
e=fixture();e.proxy.current=function()return false,P.failure("proxy","audit_changed","mode_seq",2,3)end
value,reason,detail=e:run()
T.check(value==nil and e.reads==0 and detail.first_failure.validator=="proxy"
    and detail.first_failure.field=="mode_seq"and detail.first_failure.expected==2 and detail.first_failure.observed==3,
    "first changed audit scalar is retained without any diagnostic native read")
e=fixture();e.source.current=function()error("current\nvalidator\tfailed")end
value,reason,detail=e:run()
T.check(value==nil and detail.first_failure.validator=="source"and detail.first_failure.predicate=="validator_exception"
    and detail.first_failure.observed:find("[%c]")==nil and e.reads==0,
    "thrown validator remains explicit and sanitized instead of inventing a predicate")
local f=P.failure("source","context_changed","qualification",false,true)
e=fixture();e.source.current=function()return false,f end
value,reason,detail=e:run()
T.check(detail.first_failure.expected_available and detail.first_failure.expected==false
    and detail.first_failure.observed_available and detail.first_failure.observed==true,
    "available false qualification survives first-failure forwarding")
local touches=0;local wrapper=setmetatable({},{__tostring=function()touches=touches+1;error("native wrapper")end})
f=P.failure("source",string.rep("x",200),"field",wrapper,0/0)
T.check(#f.predicate==120 and not f.expected_available and not f.observed_available and touches==0,
    "failure fields bound text and reject wrappers/nonfinite numbers without touching native objects")
e=fixture();f=P.failure("source","scope_exception","exception",nil,string.rep("q",200))
e.source.current=function()return false,f end
value,reason,detail=e:run()
T.check(#detail.first_failure.observed==120 and detail.first_failure.observed_truncated,
    "bounded scalar text explicitly retains its truncation flag through helper forwarding")

e=fixture("upperarm_l");r=e:run()
T.check(r and r.coverage=="left_upperarm_only"and r.source.side=="left"and r.proxy.side=="left"
    and #r.source.joints==1 and #r.proxy.joints==1,"fixed left focus observes one joint in both independent roles")
T.check(r.source.joints[1].name=="UserConstraint_14"and r.source.joints[1].current.value.binding.index==15
    and r.source.joints[1].current.value.binding.parent=="clavicle_l"
    and r.source.joints[1].current.value.binding.child=="upperarm_l","left focus validates actual returned accessor and endpoints")
T.check(#e.calls==10 and #P.JOINTS==3 and P.JOINTS[1][1]=="UserConstraint_10",
    "focused capture reduces getters without altering the default selection")
e=fixture("upperarm_l");e.bad_endpoint=true;r=e:run()
T.check(r and not r.source.joints[1].current.available,"left focus cannot substitute an incorrect native endpoint")
e=fixture("upperarm_l");e.getter_flip=true;r=e:run()
T.check(r==nil and #e.calls==1 and #e.records==0,"left focus retains the first scope-loss latch and stops later reads")
local invalid,invalid_reason=P.new(function()error("must not emit")end,"UserConstraint_14")
T.check(invalid==nil and invalid_reason=="unsupported joint focus","arbitrary constraint names are refused before capture")
e=fixture("upperarm_l");local saved_name=P.LEFT_UPPERARM[1][1]
P.LEFT_UPPERARM[1][1]="WrongAfterConstruction";r=e:run();P.LEFT_UPPERARM[1][1]=saved_name
T.check(r and r.source.joints[1].name==saved_name,"probe owns a copied selection unaffected by later table edits")

e=fixture("hand_r");r=e:run()
T.check(r and r.coverage=="right_hand_only"and #r.source.joints==1 and #e.calls==10
    and r.proxy.joints[1].current.value.binding.parent=="lowerarm_r"
    and r.proxy.joints[1].current.value.binding.child=="hand_r","fixed wrist focus keeps actual endpoint checks")
local function fault_case()
    local cur={has_context=true,match_id=9,round=2,life=3,cut=4,mode="interp",age=10}
    local shown={has_context=true,match_id=9,round=2,life=3,cut=4,pawn="Pawn"}
    local aim={has_context=true,match_id=9,round=2,life=3,cut=4,pawn="Pawn",world="World",at=100}
    local body={mesh_addr=40,mesh_fname="Mesh",stall={[17]=21}}
    local state={enabled=true,settle_count=6,settle_ready=false,settle_reason="hand_r rotation",
        settle_sample_ms=100,settle_pos_uu=2,settle_rot_deg=150,settle_source_seq=8,settle_source_ts=90,
        match_id=9,round=2,life=3,cut=4,pawn="Pawn",world="World",
        probe_mesh_addr=40,probe_mesh_fname="Mesh",probe_generation=6}
    return state,cur,shown,aim,body
end
local state,cur,shown,aim,body=fault_case()
local trigger=P.right_fault(state,cur,shown,aim,body,"World",6,116,20)
T.check(trigger and trigger.frames==21 and trigger.six_limb_max_rotation_deg==150 and trigger.source_seq==8,
    "fault trigger copies the prior integrated sample without requiring current source seq")
state.settle_rot_deg=0;T.check(trigger.six_limb_max_rotation_deg==150,"fault trigger owns copied scalar evidence")
for _,change in ipairs({function(s)s.settle_count=5 end,function(s)s.settle_ready=true end,
    function(s)s.settle_reason="upperarm_l rotation"end,function(s)s.life=4 end,
    function(s)s.probe_mesh_addr=41 end,function(s)s.probe_generation=7 end,
    function(s)s.settle_sample_ms=-200 end})do
    state,cur,shown,aim,body=fault_case();change(state)
    T.check(P.right_fault(state,cur,shown,aim,body,"World",6,116,20)==nil,
        "incomplete, ready, wrong limb/life/Mesh/generation or old fault refuses")
end
state,cur,shown,aim,body=fault_case();body.stall[17]=20
T.check(P.right_fault(state,cur,shown,aim,body,"World",6,116,20)==nil,"uncapped-threshold sample does not arm a fault capture")
local bad,bad_reason=P.new(function()end,"hand_r","anything")
T.check(bad==nil and bad_reason=="unsupported joint trigger","unknown trigger never arms optional reads")
e=fixture("hand_r");local observed={observed_ms=101,flags={false,false,false,false,false,false}}
e.proxy.grip_flags=function()return observed end;r=e:run()
T.check(r and r.proxy.grip_flags.available and r.proxy.grip_flags.value.flags[1]==false,
    "optional current proxy grip flags remain typed observation with their own time")
observed.flags[1]=true;T.check(r.proxy.grip_flags.value.flags[1]==false,"grip observation owns its scalar copy")
e=fixture("hand_r");e.proxy.grip_flags=function()error("binding unavailable")end;r=e:run()
T.check(r and not r.proxy.grip_flags.available,"unavailable current grip observation does not invent off flags")
e=fixture("hand_r");e.proxy.grip_flags=function()e.current=false;return observed end;r=e:run()
T.check(r==nil and #e.records==0,"current grip observation world loss stops the remaining joint getters")

e=fixture("hand_r");e.proxy.joint_angles=true
local angle_calls=0
e.proxy.mesh.GetCurrentJointAngles=function(_,name,s1,tw,s2)
    angle_calls=angle_calls+1
    assert(name=="UserConstraint_12" and s1==tw and tw==s2,"verified name and shared scalar outputs required")
    s1.Swing1Angle,s1.TwistAngle,s1.Swing2Angle=0,-37.25,12.5
end
r=e:run()
local angles=r and r.proxy.joints[1].current.value.joint_angles
T.check(angles and angles.available and angles.value.swing1==0 and angles.value.twist==-37.25
    and angles.value.swing2==12.5 and angles.value.authority==false and angle_calls==1,
    "optional current angles retain zero and signed native scalar outputs without target authority")
T.check(r.source.joints[1].current.value.joint_angles==nil,
    "disabled current-angle observation adds no native lookup or fabricated values")
e=fixture("hand_r");e.proxy.joint_angles=true;r=e:run()
T.check(r and not r.proxy.joints[1].current.value.joint_angles.available
    and r.proxy.joints[1].current.value.complete,"missing optional angle getter preserves independently complete configuration")
e=fixture("hand_r");e.proxy.joint_angles=true
e.proxy.mesh.GetCurrentJointAngles=function(_,_,s1) s1.Swing1Angle=0/0;s1.TwistAngle=0;s1.Swing2Angle=0 end
r=e:run();T.check(r and not r.proxy.joints[1].current.value.joint_angles.available,
    "nonfinite current angle remains unavailable")
e=fixture("hand_r");e.source.joint_angles=true;local proxy_angle_reads=0
e.source.mesh.GetCurrentJointAngles=function()e.current=false end
e.proxy.joint_angles=true;e.proxy.mesh.GetCurrentJointAngles=function()proxy_angle_reads=proxy_angle_reads+1 end
r=e:run();T.check(r==nil and #e.records==0 and proxy_angle_reads==0,
    "scope loss in optional angle read stops every later proxy read and emission")
