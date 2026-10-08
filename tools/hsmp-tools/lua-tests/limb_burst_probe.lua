local L=dofile(T.path("mods/HSMPAvatars/Scripts/limb_burst_probe.lua"))
local function env()
    local e={reads=0,ok=true,records={}}
    local function object(n,a)
        return {IsValid=function()return true end,GetAddress=function()return a end,
            GetFName=function()return {ToString=function()return n end}end}
    end
    e.actor=object("Pawn",10);e.mesh=object("Mesh",20)
    local world={GetAddress=function()return 500 end,GetFullName=function()return "World Arena"end}
    e.actor.GetWorld=function()return world end
    e.mesh.GetOwner=function()return e.actor end
    local sk,paasset=object("Body",21),object("PA_Body",22)
    sk.GetPhysicsAsset=function()return paasset end
    e.mesh.SkeletalMesh=sk;e.mesh.PhysicsAssetOverride={GetAddress=function()return 0 end}
    e.mesh.GetSocketTransform=function()
        e.reads=e.reads+1;return {Translation={X=1,Y=2,Z=3},Rotation={X=0,Y=0,Z=0,W=1}}
    end
    e.mesh.GetPhysicsAngularVelocityInDegrees=function()
        e.reads=e.reads+1;if e.flip_scope then e.ok=false end
        return {X=10,Y=20,Z=30}
    end
    e.actor.DriverSkeleton=object("Driver",23)
    e.actor.DriverSkeleton.GetOwner=function()return e.actor end
    e.actor.DriverSkeleton.GetSocketTransform=function(...)
        local t=e.mesh.GetSocketTransform(...)
        if e.replace_driver then local replacement=object("RebuiltDriver",23);replacement.GetOwner=function()return e.actor end;e.actor.DriverSkeleton=replacement end
        return t
    end
    e.current=function()return e.ok end;e.fname=function(s)return s end
    local names={UserConstraint_14={"clavicle_l","upperarm_l",5},UserConstraint_15={"upperarm_l","lowerarm_l",8},UserConstraint_16={"lowerarm_l","hand_l",11}}
    local weak={get=function()return e.mesh end}
    e.mesh.GetConstraintByName=function(_,name,terminated)
        e.reads=e.reads+1;assert(terminated==false);local j=assert(names[name]);return {Owner=weak,Index=j[3]}
    end
    e.library={GetAttachedBodyNames=function(_,a,p,c)
        for _,j in pairs(names)do if j[3]==a.Index then p.ParentBody={ToString=function()return j[1]end};c.ChildBody={ToString=function()return e.bad_endpoint and "head"or j[2]end};return end end
    end}
    local spec={GetAngularLimits={"Swing1MotionType","Swing1LimitAngle","Swing2MotionType","Swing2LimitAngle","TwistMotionType","TwistLimitAngle"},
        GetLinearLimits={"XMotion","YMotion","ZMotion","Limit"},GetAngularDriveParams={"OutPositionStrength","OutVelocityStrength","OutForceLimit"},
        GetAngularDriveMode={"OutDriveMode"},GetOrientationDriveSLERP={"bOutEnableSLERP"},GetAngularVelocityDriveSLERP={"bOutEnableSLERP"},
        GetOrientationDriveTwistAndSwing={"bOutEnableTwistDrive","bOutEnableSwingDrive"},GetAngularVelocityDriveTwistAndSwing={"bOutEnableTwistDrive","bOutEnableSwingDrive"}}
    for fn,fields in pairs(spec)do
        e.library[fn]=function(_,a,...)
            e.reads=e.reads+1;local args={...}
            for i,field in ipairs(fields)do
                if field:find("bOut",1,true)then args[#args][field]=false else args[#args][field]=0 end
            end
            if e.change_accessor and a.Index==8 then names.UserConstraint_15[3]=9 end
            if e.change_asset then paasset=object("RebuiltAsset",24)end
        end
    end
    e.library.GetAngularOrientationTarget=function(_,a,t)t.Pitch=0;t.Yaw=0;t.Roll=0 end
    e.library.GetAngularVelocityTarget=function(_,a,t)t.X=0;t.Y=0;t.Z=0 end
    local pa=object("PhysAnim",30);pa.GetOwner=function()return e.actor end
    pa.SkeletalMeshComponent=e.mesh;pa.StrengthMultiplyer=0
    e.actor.PhysicalAnimation=pa;e.cached={{address=30,name="PhysAnim"}}
    local list={pa}
    e.actor["Phys Anim Array"]=setmetatable({ForEach=function(_,f)
        for _,v in ipairs(list)do if f(nil,{get=function()return v end})then break end end
        if e.replace_array then list={object("Replacement",31)}end
        if e.rebind_pa then e.enumerations=(e.enumerations or 0)+1;if e.enumerations==2 then pa.SkeletalMeshComponent=object("OtherMesh",32)end end
    end},{__len=function()return e.array_count or #list end})
    e.context={instance="2",peer=1,pawn={address=10,name="Pawn"},mesh={address=20,name="Mesh"},world="6|500@World Arena",
        generation=6,match_id=8,round=1,life=1,source_cut=2,source_seq=12,source_mode="interp",source_age=-3,source_pt=100,
        frame=100,at=100,audit={qualification=false,mode_round=0,mode_life=0,pending=true}}
    e.fault={settle_rot_deg=175,settle_reason="lowerarm_l rotation"}
    e.params={dt_s=0.016,gain=0.8,cap_ang=900,holding=false}
    e.probe=L.new(function(r)e.records[#e.records+1]=r end)
    return e
end
for _,shape in ipairs({"direct","nested","matching"})do
    local e=env()
    e.library.GetAngularOrientationTarget=function(_,a,t)
        if shape~="nested"then t.Pitch=-12.5;t.Yaw=0;t.Roll=30 end
        if shape~="direct"then t.OutPosTarget={Pitch=-12.5,Yaw=0,Roll=30}end
    end
    e.library.GetAngularVelocityTarget=function(_,a,t)
        if shape~="nested"then t.X=0;t.Y=-2.75;t.Z=18 end
        if shape~="direct"then t.OutVelTarget={X=0,Y=-2.75,Z=18}end
    end
    local r=e.probe:capture(e.context,"pre_driver",e,e.params,e.fault)
    local j=r.joints[1].current.value
    T.check(j.orientation_target.available and j.orientation_target.value[1]==-12.5
        and j.orientation_target.value[2]==0 and j.orientation_target.value[3]==30,
        shape.." rotator struct output preserves finite signed and zero native fields")
    T.check(j.velocity_target.available and j.velocity_target.value[1]==0
        and j.velocity_target.value[2]==-2.75 and j.velocity_target.value[3]==18,
        shape.." vector struct output preserves finite signed and zero native fields")
end
for _,kind in ipairs({"missing","partial","string","nan","infinite","nested_invalid","conflicting"})do
    local e=env()
    e.library.GetAngularOrientationTarget=function(_,a,t)
        if kind=="missing"then return end
        t.Pitch=0;t.Yaw=0;t.Roll=0
        if kind=="partial"then t.Roll=nil
        elseif kind=="string"then t.Roll="0"
        elseif kind=="nan"then t.Roll=0/0
        elseif kind=="infinite"then t.Roll=math.huge
        elseif kind=="nested_invalid"then t.OutPosTarget=false
        elseif kind=="conflicting"then t.OutPosTarget={Pitch=0,Yaw=1,Roll=0}end
    end
    e.library.GetAngularVelocityTarget=function(_,a,t)
        if kind=="missing"then return end
        t.X=0;t.Y=0;t.Z=0
        if kind=="partial"then t.Z=nil
        elseif kind=="string"then t.Z="0"
        elseif kind=="nan"then t.Z=0/0
        elseif kind=="infinite"then t.Z=-math.huge
        elseif kind=="nested_invalid"then t.OutVelTarget=false
        elseif kind=="conflicting"then t.OutVelTarget={X=1,Y=0,Z=0}end
    end
    local r=e.probe:capture(e.context,"pre_driver",e,e.params,e.fault)
    local j=r.joints[1].current.value
    T.check(not j.orientation_target.available and not j.velocity_target.available,
        kind.." targets remain explicitly unavailable, without zero/default invention")
    T.check(j.angular_limits.available and j.drive_params.available,
        kind.." target failure leaves independent native constraint evidence intact")
end
do
    local e=env();e.replace_driver=true
    local r=e.probe:capture(e.context,"pre_driver",e,e.params,e.fault)
    T.check(r and not r.driver_component.available and not r.bones[1].driver_socket_world.available,
        "same-address/new-name DriverSkeleton cannot mix frames across one phase")
end
do
    local e=env();e.rebind_pa=true
    local r=e.probe:capture(e.context,"pre_driver",e,e.params,e.fault)
    T.check(r and not r.physical_animation.available,"same-membership PA rebind invalidates the captured phase")
end
do
    local e=env();T.check(not e.probe:want(nil)and e.reads==0,"no native reads without an existing left-arm fault")
    local r=e.probe:capture(e.context,"pre_driver",e,e.params,e.fault)
    T.check(r and r.phase=="pre_driver"and not r.qualification and r.audit.mode_round==0,"pending raw Mode stays observed, never fabricated Live")
    T.check(r.bones[1].angular_velocity_deg_s.value[2]==20 and not r.socket_frame_authority,"native velocity and socket frame authority stay distinct")
    T.check(r.joints[1].current.value.index==5 and r.joints[2].current.value.index==8,"uses native indices, never joint-name suffixes")
    T.check(r.joints[1].current.value.orientation_slerp.available and r.joints[1].current.value.orientation_slerp.value.bOutEnableSLERP==false,
        "named scalar outputs preserve available false even in another output table")
    T.check(r.joints[2].current.value.drive_params.value.OutPositionStrength==0,"available zero drive is native evidence")
    T.check(r.joints[1].current.value.orientation_target.available and r.joints[1].current.value.orientation_target.value[1]==0
        and r.joints[1].current.value.velocity_target.available and r.joints[1].current.value.velocity_target.value[3]==0,
        "pinned UE4SS direct struct outputs preserve available zero targets")
    T.check(r.physical_animation.available and r.physical_animation.value.entries[1].binding.value.strength==0
        and r.physical_animation.value.entries[1].cached,"fresh PA membership/binding compared to cached identity")
    e.params.gain=99;e.context.audit.mode_round=9;e.context.pawn.name="changed"
    T.check(r.params.gain==0.8 and r.driver_params.gain==0.8 and r.audit.mode_round==0 and r.pawn.name=="Pawn",
        "published params/context/audit are immutable scalar copies")
    local function plain(v)
        if type(v)~="table"then return type(v)~="userdata"end
        for _,x in pairs(v)do if not plain(x)then return false end end;return true
    end
    T.check(plain(r)and plain(e.probe),"burst state and output retain no native wrappers")
end
for _,kind in ipairs({"bad_endpoint","change_accessor","change_asset"})do
    local e=env();e[kind]=true
    local r=e.probe:capture(e.context,"pre_driver",e,e.params,e.fault)
    T.check(r and not r.joints[kind=="change_accessor"and 2 or 1].current.available,kind.." rejects current constraint proof")
end
for _,kind in ipairs({"missing_getter","missing_PA","overflow","replace_array","bad_strength"})do
    local e=env()
    if kind=="missing_getter"then e.library.GetAngularDriveMode=nil
    elseif kind=="missing_PA"then e.actor["Phys Anim Array"]=nil
    elseif kind=="overflow"then e.array_count=9
    elseif kind=="replace_array"then e.replace_array=true
    else e.actor.PhysicalAnimation.StrengthMultiplyer=0/0 end
    local r=e.probe:capture(e.context,"pre_driver",e,e.params,e.fault)
    if kind=="missing_getter"then
        T.check(r.joints[1].current.available and not r.joints[1].current.value.drive_mode.available,"missing getter remains partial without invented mode")
    elseif kind=="bad_strength"then T.check(r.physical_animation.available and not r.physical_animation.value.entries[1].binding.available,"nonfinite PA strength cannot mean neutralised")
    else T.check(r and not r.physical_animation.available,kind.." cannot prove complete PA collection")end
end
do
    local e=env();e.flip_scope=true
    T.check(not e.probe:capture(e.context,"pre_driver",e,e.params,e.fault)and #e.records==0,"mid-read scope change discards row")
    local n=e.reads;e.probe:capture(e.context,"post_driver",e,e.params,e.fault)
    T.check(e.reads==n,"scope loss closes burst before more native reads")
end
for _,kind in ipairs({"scope_false","scope_throw","scope_recovers"})do
    local e=env();local lost,checked=false,false
    local old_reads,pa_reads,after_calls=0,0,{}
    local native_current=e.current
    e.current=function()
        if not lost then return native_current()end
        if kind=="scope_throw"then error("native current scope read threw")end
        if kind=="scope_recovers"then
            if checked then return true end
            checked=true
        end
        return false
    end
    for name,fn in pairs(e.library)do
        e.library[name]=function(...)
            if lost then old_reads=old_reads+1;after_calls[#after_calls+1]=name end
            local result=fn(...)
            if name=="GetAngularLimits"then lost=true end
            return result
        end
    end
    local resolve=e.mesh.GetConstraintByName
    e.mesh.GetConstraintByName=function(...)
        if lost then old_reads=old_reads+1;after_calls[#after_calls+1]="final accessor"end
        return resolve(...)
    end
    local arr=e.actor["Phys Anim Array"];local each=arr.ForEach
    arr.ForEach=function(...)
        if lost then pa_reads=pa_reads+1 end
        return each(...)
    end
    local ok,r=pcall(e.probe.capture,e.probe,e.context,"pre_driver",e,e.params,e.fault)
    T.check(ok and not r and #e.records==0 and e.probe.closed,
        kind.." inside first constraint getter closes/discards capture rather than becoming partial healthy proof")
    T.check(old_reads==0 and pa_reads==0,
        kind.." permits zero subsequent old constraint/accessor/PA reads",T.repr(after_calls))
end
do
    local e=env()
    for i=1,3 do
        e.context.frame=99+i;e.context.source_seq=11+i;e.context.at=99+i
        for _,phase in ipairs({"pre_driver","post_driver","bp_post","policy_post"})do
            e.probe:capture(e.context,phase,e,e.params,e.fault)
            local n=e.reads;e.probe:capture(e.context,phase,e,e.params,e.fault)
            T.check(e.reads==n,"duplicate "..phase.." has no additional reads, drive"..i)
        end
    end
    T.check(#e.records==12 and e.probe.drives==3,"one whole-run three-drive burst has twelve actual phase rows")
    e.context.frame=103;local n=e.reads
    T.check(not e.probe:capture(e.context,"pre_driver",e,e.params,e.fault)and n==e.reads,"fourth drive cannot extend budget")
end
do
    local e=env();e.probe:capture(e.context,"pre_driver",e,e.params,e.fault)
    e.context.life=2;local n=e.reads
    T.check(not e.probe:capture(e.context,"post_driver",e,e.params,e.fault)and n==e.reads,"new life cannot inherit previous burst")
end
do
    local e=env();e.probe:capture(e.context,"pre_driver",e,e.params,e.fault)
    e.context.at=601;local n=e.reads
    T.check(not e.probe:capture(e.context,"post_driver",e,e.params,e.fault)and n==e.reads,"hard half-second burst expiry precedes native reads")
end
do
    local e=env();for i=1,24 do assert(e.probe:attempt(e.fault,100))end
    T.check(not e.probe:attempt(e.fault,100)and e.probe.closed,"twenty-four refused fresh-context attempts exhaust whole-run admission")
end
