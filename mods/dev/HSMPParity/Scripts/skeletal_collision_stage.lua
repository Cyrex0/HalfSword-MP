-- DEV-ONLY prototype. Not imported by production or Parity. No native writes.
-- Exact primitive math; native frame/scale correctness still needs an isolated
-- read-only probe before this can authorize gameplay contacts.
local M={MAX_BODIES=32,MAX_SHAPES=128}
local function finite(x)return type(x)=="number" and x==x and math.abs(x)<math.huge end
local function v(a)return {a.X,a.Y,a.Z}end
local function add(a,b)return {a[1]+b[1],a[2]+b[2],a[3]+b[3]}end
local function sub(a,b)return {a[1]-b[1],a[2]-b[2],a[3]-b[3]}end
local function mul(a,k)return {a[1]*k,a[2]*k,a[3]*k}end
local function dot(a,b)return a[1]*b[1]+a[2]*b[2]+a[3]*b[3]end
local function len(a)return math.sqrt(dot(a,a))end
local function cross(a,b)return {a[2]*b[3]-a[3]*b[2],a[3]*b[1]-a[1]*b[3],a[1]*b[2]-a[2]*b[1]}end
local function conj(q)return {-q[1],-q[2],-q[3],q[4]}end
local function qm(a,b)return {a[4]*b[1]+a[1]*b[4]+a[2]*b[3]-a[3]*b[2],a[4]*b[2]-a[1]*b[3]+a[2]*b[4]+a[3]*b[1],a[4]*b[3]+a[1]*b[2]-a[2]*b[1]+a[3]*b[4],a[4]*b[4]-a[1]*b[1]-a[2]*b[2]-a[3]*b[3]}end
local function rot(q,p)local a=qm(qm(q,{p[1],p[2],p[3],0}),conj(q));return {a[1],a[2],a[3]}end
local function normq(q)local n=0;for i=1,4 do assert(finite(q[i]),"invalid quaternion");n=n+q[i]*q[i] end;assert(n>0.99 and n<1.01,"nonunit quaternion");n=math.sqrt(n);return {q[1]/n,q[2]/n,q[3]/n,q[4]/n}end
local function vec(a)for i=1,3 do assert(finite(a[i]),"nonfinite vector")end;return a end
local function pos(x)assert(finite(x) and x>0 and x<=1000,"invalid primitive dimension");return x end
local function euler(r)
    local h=math.pi/360;local p,y,r=r.Pitch*h,r.Yaw*h,r.Roll*h
    local sp,cp,sy,cy,sr,cr=math.sin(p),math.cos(p),math.sin(y),math.cos(y),math.sin(r),math.cos(r)
    return normq({cr*sp*sy-sr*cp*cy,-cr*sp*cy-sr*cp*sy,cr*cp*sy-sr*sp*cy,cr*cp*cy+sr*sp*sy})
end
local function same(a,b)return a.actor==b.actor and a.life==b.life and a.match_id==b.match_id and a.round==b.round and a.component==b.component and a.bone==b.bone and a.kind==b.kind and a.element==b.element and a.asset==b.asset end
M.same_identity=same
function M.to_world(shape,frame)
    local s=frame.scale;vec(s)
    assert(s[1]>0 and math.abs(s[1]-s[2])<1e-6 and math.abs(s[1]-s[3])<1e-6,"nonuniform primitive scale requires native Chaos geometry proof")
    local q=normq(frame.q);local out={}
    for k,x in pairs(shape)do out[k]=x end
    out.p=add(vec(frame.p),rot(q,mul(vec(shape.center),s[1])))
    out.q=qm(q,normq(shape.rotation or {0,0,0,1}))
    if shape.kind=="sphere" then out.radius=pos(shape.radius)*s[1]
    elseif shape.kind=="capsule" then
        out.radius=pos(shape.radius)*s[1];assert(finite(shape.length) and shape.length>=0,"invalid capsule cylinder length")
        out.length=shape.length*s[1]
    elseif shape.kind=="box" then out.half=mul(vec(shape.half),s[1]);for i=1,3 do pos(out.half[i])end
    else error("unsupported primitive kind")end
    return out
end
function M.distance(shape,point)
    local p=rot(conj(normq(shape.q)),sub(vec(point),vec(shape.p)))
    if shape.kind=="sphere" then return math.max(0,len(p)-shape.radius)
    elseif shape.kind=="capsule" then
        p[3]=p[3]-math.max(-shape.length/2,math.min(shape.length/2,p[3]))
        return math.max(0,len(p)-shape.radius)
    elseif shape.kind=="box" then
        for i=1,3 do p[i]=math.max(0,math.abs(p[i])-shape.half[i])end
        return len(p)
    end
    error("unsupported distance shape")
end
function M.velocity_at_point(center,linear,angular,point)
    return add(vec(linear),cross(vec(angular),sub(vec(point),vec(center))))
end
local function slerp(a,b,t)
    local d=0;for i=1,4 do d=d+a[i]*b[i]end
    if d<0 then b={-b[1],-b[2],-b[3],-b[4]};d=-d end
    d=math.min(1,d);local x,y=1-t,t
    if d<0.9995 then local theta=math.acos(d);local sn=math.sin(theta);x,y=math.sin((1-t)*theta)/sn,math.sin(t*theta)/sn end
    return normq({a[1]*x+b[1]*y,a[2]*x+b[2]*y,a[3]*x+b[3]*y,a[4]*x+b[4]*y})
end
function M.interpolate(a,b,t)
    assert(same(a,b),"source identity discontinuity")
    assert(t>=0 and t<=1,"interpolation fraction")
    assert(a.radius==b.radius and a.length==b.length,"primitive size changed")
    if a.kind=="box" then for i=1,3 do assert(a.half[i]==b.half[i],"box size changed")end end
    local o={};for k,x in pairs(a)do o[k]=x end
    o.p=add(a.p,mul(sub(b.p,a.p),t));o.q=slerp(normq(a.q),normq(b.q),t)
    return o
end
-- Continuous sweep of the stated interpolated rigid-body history. Exact shape
-- distance at samples; Lipschitz lower bounds exclude intervals. Exhausted
-- work returns indeterminate, NEVER an AABB/conservative-bound acceptance.
function M.swept_distance(a,b,point,tolerance,budget)
    assert(same(a,b),"source identity discontinuity");assert(finite(tolerance) and tolerance>=0,"tolerance")
    budget=budget or 128;assert(budget>=3 and budget<=4096,"sweep budget")
    local d=0;for i=1,4 do d=d+a.q[i]*b.q[i]end
    local angle=2*math.acos(math.min(1,math.abs(d)))
    local radius=a.kind=="box" and len(a.half) or a.kind=="capsule" and a.length/2+a.radius or a.radius
    local speed=len(sub(b.p,a.p))+angle*radius
    local best=math.huge;local best_t=0;local work={{0,1}};local used=0
    while #work>0 do
        if used>=budget then return {status="indeterminate",distance=best,time=best_t,samples=used} end
        local it=table.remove(work);local t=(it[1]+it[2])/2
        local dist=M.distance(M.interpolate(a,b,t),point);used=used+1
        if dist<best then best,best_t=dist,t end
        if dist<=tolerance then return {status="hit",distance=dist,time=t,samples=used} end
        if dist-speed*(it[2]-it[1])/2<=tolerance then
            work[#work+1]={it[1],t};work[#work+1]={t,it[2]}
        end
    end
    return {status="miss",distance=best,time=best_t,samples=used}
end
-- e.each follows the pinned UE4SS out/table/TArray unwrap contract. e.name
-- converts native FName; e.valid checks UObjects, never structs. No UObject is
-- retained in the returned rows. Unknown physics elements fail the entire set.
function M.read_component(component,identity,e)
    local ok,result=pcall(function()
        assert(e.valid(component),"invalid skeletal component")
        local asset=component.PhysicsAssetOverride
        if not e.valid(asset) then local mesh=component:GetSkeletalMeshAsset();assert(e.valid(mesh),"missing skeletal mesh");asset=mesh:GetPhysicsAsset()end
        assert(e.valid(asset),"missing PhysicsAsset")
        local asset_name=e.fullname(asset);assert(type(asset_name)=="string" and asset_name~="","missing PhysicsAsset identity")
        local out={};local bodies=0;local seen={}
        e.each(asset.SkeletalBodySetups,function(body)
            assert(e.valid(body),"invalid skeletal body setup");bodies=bodies+1;assert(bodies<=M.MAX_BODIES,"body capacity exceeded")
            local bone=e.name(body.BoneName);assert(type(bone)=="string" and bone~="" and bone~="None" and not seen[bone],"missing/duplicate body bone");seen[bone]=true
            assert(e.physics_body_exists and e.physics_body_exists(component,body.BoneName)==true,"native named physics body existence unproved")
            local geom=body.AggGeom
            for _,field in ipairs({"ConvexElems","TaperedCapsuleElems","LevelSetElems","SkinnedLevelSetElems"})do
                e.each(geom[field],function()error("unsupported native shape "..field.." on "..bone)end)
            end
            local native=e.physics_transform(component,body.BoneName)
            local frame={p=v(native.Translation),q={native.Rotation.X,native.Rotation.Y,native.Rotation.Z,native.Rotation.W},scale=v(native.Scale3D)}
            local function take(kind,field)
                local ordinal=0
                e.each(geom[field],function(el)
                    ordinal=ordinal+1
                    local collision=e.name(el.CollisionEnabled)
                    local numeric=tonumber(el.CollisionEnabled)
                    if numeric then collision=({[0]="NoCollision",[1]="QueryOnly",[2]="PhysicsOnly",[3]="QueryAndPhysics"})[numeric] or "unknown" end
                    if collision=="NoCollision" or collision=="ECollisionEnabled::NoCollision" or collision=="QueryOnly" or collision=="ECollisionEnabled::QueryOnly" then return end
                    assert(collision=="QueryAndPhysics" or collision=="PhysicsOnly" or collision=="ECollisionEnabled::QueryAndPhysics" or collision=="ECollisionEnabled::PhysicsOnly","unknown native shape collision state")
                    local shape={};for k,x in pairs(identity)do assert(type(x)~="table" and type(x)~="userdata","identity must be plain");shape[k]=x end
                    shape.asset=asset_name;shape.bone=bone;shape.kind=kind;shape.element=ordinal;shape.center=v(el.Center)
                    shape.rotation=kind=="sphere" and {0,0,0,1} or euler(el.Rotation)
                    if kind=="box" then shape.half={el.X/2,el.Y/2,el.Z/2} else shape.radius=el.Radius;shape.length=kind=="capsule" and el.Length or nil end
                    local world=M.to_world(shape,frame)
                    local point={X=world.p[1],Y=world.p[2],Z=world.p[3]}
                    world.linear=v(component:GetPhysicsLinearVelocityAtPoint(point,body.BoneName))
                    world.angular=v(component:GetPhysicsAngularVelocityInRadians(body.BoneName));vec(world.linear);vec(world.angular)
                    world.native_frame_verified=false -- staged probe must establish frame/scale semantics
                    out[#out+1]=world;assert(#out<=M.MAX_SHAPES,"primitive capacity exceeded")
                end)
            end
            take("sphere","SphereElems");take("box","BoxElems");take("capsule","SphylElems")
        end)
        assert(#out>0,"no active native collision primitives")
        return out
    end)
    return ok and result or nil,ok and nil or tostring(result)
end
return M
