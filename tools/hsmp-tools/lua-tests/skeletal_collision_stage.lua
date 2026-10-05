local M=dofile(T.path("mods/dev/HSMPParity/Scripts/skeletal_collision_stage.lua"))
local Journal=dofile(T.path("mods/dev/HSMPParity/Scripts/skeletal_hit_context_stage.lua"))
local function near(a,b)return math.abs(a-b)<1e-7 end
local id={actor="quarantine-weapon-1",life=2,round=3,match_id="123",component=4,asset="native-strap-B",bone="Joint2",element=1,kind="capsule"}
local localshape={};for k,v in pairs(id)do localshape[k]=v end
localshape.center={0,0,0};localshape.rotation={0,0,0,1};localshape.radius=1;localshape.length=8
local frame={p={10,20,30},q={0,math.sqrt(0.5),0,math.sqrt(0.5)},scale={1,1,1}}
local shape=M.to_world(localshape,frame)
T.check(near(M.distance(shape,{15,20,30}),0),"capsule cylinder half-length plus radius is exact under rotation")
T.check(near(M.distance(shape,{15.25,20,30}),0.25),"rotated native capsule distance uses rounded end")
T.check(M.distance(shape,{15,21,30})>0.3,"capsule corner outside despite render box containment")
local velocity=M.velocity_at_point({0,0,0},{0,0,0},{0,0,2},{10,0,0})
T.check(near(velocity[2],20),"angular body velocity at impact survives zero center velocity")
local b={};for k,v in pairs(shape)do b[k]=v end;b.p={30,20,30}
local sweep=M.swept_distance(shape,b,{20,20,30},0.001)
T.check(sweep.status=="hit","source-specific capsule sweep covers between-snapshot contact")
T.check(M.swept_distance(shape,b,{20,30,30},0.001).status=="miss","capsule sweep excludes remote point")
local changed={};for k,v in pairs(b)do changed[k]=v end;changed.bone="Joint3"
T.check(not pcall(M.swept_distance,shape,changed,{20,20,30},0.1),"no sweep across different skeletal bodies")
changed.bone=b.bone;changed.actor="new-native-actor"
T.check(not pcall(M.interpolate,shape,changed,0.5),"no manufactured velocity across source actor replacement")
T.check(not pcall(M.to_world,localshape,{p=frame.p,q=frame.q,scale={1,2,1}}),"nonuniform scaled capsule never approximated by bounding box")
local native=dofile(T.path("test-results/dev-feature-checks/weapon-catalogue-audit/native-strap-fixture.lua"))
local function valid(x)return type(x)=="table" and x.valid==true end
local current_bone,point_calls={},{}
local component={valid=true}
function component:GetPhysicsLinearVelocityAtPoint(point,bone)point_calls[#point_calls+1]=bone;return {X=1,Y=2,Z=3}end
function component:GetPhysicsAngularVelocityInRadians(bone)return {X=0,Y=0,Z=2}end
local env={valid=valid,name=function(x)return x end,fullname=function(x)return x.name end,
    each=function(a,f)for _,v in ipairs(a or {})do f(v)end end,
    physics_body_exists=function(c,bone)return bone:match("^Joint[1-4]$")~=nil end,
    physics_transform=function(c,bone)current_bone[#current_bone+1]=bone;return {Translation={X=100,Y=200,Z=300},Rotation={X=0,Y=0,Z=0,W=1},Scale3D={X=1,Y=1,Z=1}}end}
for _,asset in ipairs(native)do
    component.PhysicsAssetOverride=asset
    local shapes,err=M.read_component(component,id,env)
    T.check(shapes and #shapes==4,"all four original native capsule bodies retained: "..asset.name.." "..tostring(err))
    for _,s in ipairs(shapes or {})do
        T.check(s.kind=="capsule" and s.radius>0 and s.bone:match("^Joint[1-4]$")~=nil,"native body bone/dimensions captured")
        T.check(s.native_frame_verified==false,"fixture success does not claim native transform proof")
        T.check(near(M.distance(s,s.p),0),"native capsule center lies within exact shape")
    end
end
T.check(#point_calls==12 and #current_bone==12,"physical transform and point velocity queried by every original body name")
local exists=env.physics_body_exists;env.physics_body_exists=nil
local missing,missing_error=M.read_component(component,id,env)
T.check(not missing and missing_error:find("existence unproved"),"asset body entry alone cannot prove live native physics body exists")
env.physics_body_exists=exists
component.PhysicsAssetOverride={valid=true,name="unsupported",SkeletalBodySetups={{valid=true,BoneName="Joint1",AggGeom={ConvexElems={{}}}}}}
local shapes,err=M.read_component(component,id,env)
T.check(not shapes and err:find("ConvexElems"),"unsupported convex geometry fails whole component explicitly")
component.PhysicsAssetOverride={valid=true,name="spherebox",SkeletalBodySetups={{valid=true,BoneName="Joint1",AggGeom={
    SphereElems={{Center={X=0,Y=0,Z=0},Radius=2,CollisionEnabled=3}},
    BoxElems={{Center={X=4,Y=0,Z=0},Rotation={Pitch=0,Yaw=0,Roll=0},X=4,Y=6,Z=8,CollisionEnabled=3}}
}}}}
shapes,err=M.read_component(component,id,env)
T.check(shapes and #shapes==2 and shapes[2].half[3]==4,"native FKBox dimensions are full lengths, exact half conversion")
T.check(near(M.distance(shapes[1],{103,200,300}),1),"exact native sphere distance")
local oldmax=M.MAX_SHAPES;M.MAX_SHAPES=1
shapes,err=M.read_component(component,id,env);M.MAX_SHAPES=oldmax
T.check(not shapes and err:find("capacity"),"capacity overflow refuses complete component, never truncates")
local j=Journal.new()
local contact={match_id="123",round=3,life=2,weapon_actor=10,source_component=20,victim_component=30,victim_bone="Head",point={1,2,3}}
local hit={};for k,v in pairs(contact)do hit[k]=v end;hit.my_bone="Joint3";hit.life=1
T.check(j.record(contact)==nil,"post callback without native entry scope cannot authorize source bone")
local token=j.begin_collision(hit)
T.check(j.record(contact)==nil,"old-life CollisionHit cannot bind source bone")
j.end_collision(token);hit.life=2;hit.source_component=21;token=j.begin_collision(hit)
T.check(j.record(contact)==nil,"different source module cannot donate MyBoneName")
j.end_collision(token);hit.source_component=20;token=j.begin_collision(hit)
local resolved=j.record(contact)
T.check(resolved and resolved.token==token and resolved.source_bone=="Joint3","original CollisionHit MyBoneName binds exact nested DCD token")
hit.my_bone="Joint4";local nested=j.begin_collision(hit)
T.check(j.record(contact).source_bone=="Joint4","identical nested contact keeps original inner source bone")
T.check(not pcall(j.end_collision,token),"out-of-order native return fails closed")
j.end_collision(nested)
T.check(j.record(contact).source_bone=="Joint3","nested return restores original outer source bone")
j.end_collision(token)
T.check(j.record(contact)==nil,"after native return no stale source-bone context survives")
