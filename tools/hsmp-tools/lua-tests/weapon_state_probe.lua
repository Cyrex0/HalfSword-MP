local P=dofile(T.path("mods/dev/HSMPParity/Scripts/weapon_state_probe.lua"))
local next_addr=20
local function object(name,class)
    next_addr=next_addr+1
    local addr=next_addr
    local o={}
    o.IsValid=function()return true end
    o.GetAddress=function()return addr end
    o.GetFName=function()return {ToString=function()return name end}end
    o.GetClass=function()return {GetFName=function()return {ToString=function()return class end}end}end
    return o
end
local function fn(n)return {ToString=function()return n end}end
local function vector(x,y,z)return {X=x,Y=y,Z=z}end
local pawn=object("Owner","Willie_BP_C")
local weapon=object("Polearm","ModularWeaponBP_Polearm_Mid_Tier_C")
local root=object("WeaponRoot","StaticMeshComponent")
local base=object("BaseMesh","StaticMeshComponent")
local hand=object("CharacterMesh0","SkeletalMeshComponent")
local reads=0
for _,c in ipairs({root,base}) do
    c.IsSimulatingPhysics=function()reads=reads+1;return true end
    c.GetCollisionEnabled=function()return 3 end
    c.GetOwner=function()return weapon end
    c.GetAttachParent=function()return hand end
    c.GetAttachSocketName=function()return fn("hand_r")end
    c.GetMass=function()return 4.2 end
    c.GetSocketTransform=function()return {Translation=vector(1,2,3),Rotation={X=0,Y=0,Z=0,W=1}}end
end
pawn["Weapon R"],weapon.RootComponent,weapon.BaseMesh=pawn["Weapon R"] or weapon,root,base
weapon["Is Held"],weapon["Grip R Hand Default"],weapon["Grip L Hand Default"]=true,14,3
local grip=object("Grip","PhysicsConstraintComponent")
grip.GetConstrainedComponents=function(_,a,b,c,d)
    -- Named OutParms can be filled into any passed table in the pinned build.
    d.OutComponent1,d.OutBoneName1,d.OutComponent2,d.OutBoneName2=base,fn("None"),hand,fn("hand_r")
end
grip.ConstraintInstance={ConstraintBone1=fn("None"),ConstraintBone2=fn("hand_r"),
    Pos1=vector(1,0,0),PriAxis1=vector(1,0,0),SecAxis1=vector(0,1,0),Pos2=vector(0,2,0),PriAxis2=vector(0,1,0),SecAxis2=vector(1,0,0),
    ProfileInstance={AngularDrive={SlerpDrive={Stiffness=0,Damping=0,MaxForce=0}},LinearLimit={XMotion=2,YMotion=2,ZMotion=2,Limit=1},
        ConeLimit={Swing1Motion=2,Swing1LimitDegrees=10,Swing2Motion=2,Swing2LimitDegrees=20},TwistLimit={TwistMotion=2,TwistLimitDegrees=30}}}
local handle=object("Handle","PhysicsHandleComponent")
handle.GetGrabbedComponent=function()return base end
local component_queries=0
pawn.K2_GetComponentsByClass=function(_,cls)
    component_queries=component_queries+1
    local c=cls=="PhysicsConstraintComponent" and grip or handle
    return {{get=function()return c end}}
end
weapon.K2_GetComponentsByClass=function()return {}end
local context={peer=0,pawn="Owner",address=pawn:GetAddress(),match_id=61,round=1,life=1,world="one"}
local e={class=function(c)return c end,fname=function(n)return n end,current=function()return true end}
local r,why=P.capture(pawn,context,e)
T.check(r and r.read_only and reads==2,"snapshot reads actual Root and BaseMesh simulation independently",why)
T.check(r.weapons[1].root.id.address==root:GetAddress() and r.weapons[1].base.id.address==base:GetAddress(),"component identity is retained as copied addresses")
T.check(r.constraints[1].endpoints.two.address==hand:GetAddress() and r.constraints[1].endpoints.bone2=="hand_r","native constrained endpoints use named OutParms")
T.check(r.constraints[1].reference.pos2[2]==2 and r.constraints[1].profile.twist==30,"authored reference axes and limits are read without widening")
T.check(r.handles[1].grabbed.address==base:GetAddress(),"native PhysicsHandle grab is captured")
local lines={}; P.emit(r,function(f,...)lines[#lines+1]=string.format(f,...)end)
T.check(table.concat(lines,"\n"):find("actual_sim=true",1,true) and table.concat(lines,"\n"):find("WPNREFERENCE",1,true),"diagnostic emits actual simulation and constraint reference evidence")
local calls=0
e.current=function()calls=calls+1;return calls==1 end
T.check(P.capture(pawn,context,e)==nil,"a changed original generation refuses the completed snapshot")
e.current=function()return false end
local before=component_queries
T.check(P.capture(pawn,context,e)==nil and component_queries==before,"unverified context touches no component arrays")
