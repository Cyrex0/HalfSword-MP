local M=dofile(T.path("mods/dev/HSMPParity/Scripts/body_frame_probe.lua"))
local t={Translation={X=1,Y=2,Z=3},Rotation={X=0,Y=0,Z=0,W=1},Scale3D={X=1,Y=1,Z=1}}
local mesh={GetAddress=function()return 10 end,GetSocketTransform=function()return t end,
    GetCenterOfMass=function()return t.Translation end,GetBoneMass=function()return 2 end,
    GetPhysicsAngularVelocityInDegrees=function()return {X=0,Y=90,Z=0}end}
function mesh:GetCurrentJointAngles(name,s1,tw,s2)
    T.check(name=="hand_r","joint query uses exact original bone")
    s1.Swing1Angle=12;tw.TwistAngle=34;s2.Swing2Angle=56
end
local env={current=function()return true end,allowed_bone=function(b)return b=="hand_r"end,
    fname=function(b)return b end,now=function()return 7 end,
    physics={GetPhysicsObjectWorldTransform=function()return t end},
    target=function()return {1,2,3,0,0,0,1}end}
local id={match_id=1,round=2,life=3,pawn="native",pawn_address=4,display_time=5}
local r=M.capture(mesh,id,{"hand_r"},env)
T.check(r and r.rows[1].joint_angles_deg.twist==34,"scalar outputs use actual reflected parameter names")
T.check(r.physics_frame_verified==false,"raw physics observation never claims basis verified")
t.Translation.X=999
T.check(r.rows[1].socket.p[1]==1 and r.rows[1].physics.p[1]==1,"no native transform wrapper retained")
mesh.GetCurrentJointAngles=function()error("native unavailable")end
r=M.capture(mesh,id,{"hand_r"},env)
T.check(r.rows[1].errors.joint_angles_deg:find("native unavailable",1,true)~=nil,"missing native joint query is explicit")
T.check(not M.capture(mesh,id,{"unknown"},env),"unknown bone cannot alias a body")
local n=0;env.current=function()n=n+1;return n==1 end
T.check(not M.capture(mesh,id,{"hand_r"},env),"same-life context must survive the entire synchronous capture")
env.current=function()return true end
local weak={native_weak_handle=true}
mesh.GetConstraintByName=function(_,name,terminated)
    T.check(name=="UserConstraint_12" and terminated==false,"constraint resolved by exact native dictionary name")
    return {Owner=weak,Index=13}
end
env.joints={{name="UserConstraint_12",parent="lowerarm_r",child="hand_r"}}
env.constraint_library={
    GetAttachedBodyNames=function(_,a,p,c)
        T.check(a.Owner==weak and a.Index==13,"opaque native weak owner and index copied unchanged for by-ref struct")
        p.ParentBody={ToString=function()return "lowerarm_r"end};c.ChildBody={ToString=function()return "hand_r"end}
    end,
    GetAngularLimits=function(_,a,s1,l1,s2,l2,tw,tl)
        s1.Swing1MotionType=1;l1.Swing1LimitAngle=45;s2.Swing2MotionType=1;l2.Swing2LimitAngle=45;tw.TwistMotionType=1;tl.TwistLimitAngle=85
    end,
    GetAngularDriveParams=function(_,a,p,v,f)p.OutPositionStrength=0;v.OutVelocityStrength=0;f.OutForceLimit=0 end,
    GetLinearLimits=function(_,a,x,y,z,l)x.XMotion=2;y.YMotion=2;z.ZMotion=2;l.Limit=0 end,
}
r=M.capture(mesh,id,{"hand_r"},env)
T.check(r.joints[1].current.twist_limit==85 and r.joints[1].current.linear_x==2,"actual profile numbers copied independently from asset defaults")
env.joints[1].parent="head"
r=M.capture(mesh,id,{"hand_r"},env)
T.check(r.joints[1].current==nil and r.joints[1].errors.current:find("endpoint mismatch",1,true)~=nil,"wrong constraint endpoints are reported rather than guessed")
