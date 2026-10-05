-- DEV ONLY, synchronous read-only reflected APIs. Opaque Chaos result is passed
-- directly to its native extractor; no offsets, fake structs or retained wrapper.
local M={}
local function finite(x)return type(x)=='number' and x==x and math.abs(x)<math.huge end
local function vector(v)
    assert(v and finite(v.X) and finite(v.Y) and finite(v.Z),'invalid native vector')
    return {X=v.X,Y=v.Y,Z=v.Z}
end
local function transform(t)
    local q=t.Rotation; assert(q,'missing rotation')
    local r={Translation=vector(t.Translation),Scale3D=vector(t.Scale3D),Rotation={}}
    local norm=0;for _,k in ipairs({'X','Y','Z','W'})do assert(finite(q[k]),'invalid quaternion');r.Rotation[k]=q[k];norm=norm+q[k]^2 end
    assert(norm>0.99 and norm<1.01,'nonunit native quaternion')
    return r
end
function M.make(d)
    assert(d.library and d.name and d.out_name and d.valid,'missing native dependencies')
    local e={}
    function e.physics_transform(component,bone)
        assert(d.valid(component),'invalid current component')
        return transform(d.library:GetPhysicsObjectWorldTransform(component,bone))
    end
    function e.inspect_body(component,bone)
        local ok,row=pcall(function()
            local name=d.name(bone);assert(name and name~='None','invalid body name')
            local frame=e.physics_transform(component,bone)
            local com=vector(component:GetCenterOfMass(bone))
            -- Exact name returned is positive existence proof. A mismatched
            -- nearest body is indeterminate, never a fallback to that body.
            local observations={};local found=false
            for _,p in ipairs({com,frame.Translation})do
                local result=d.library:GetClosestPhysicsObjectFromWorldLocation(component,p)
                local out={}
                local success=d.library:ExtractClosestPhysicsObjectResults(result,out)
                local got=success==true and d.out_name(out) or nil
                observations[#observations+1]={success=success==true,name=got or 'UNKNOWN',point=p}
                if got==name then found=true end
            end
            local mass=component:GetBoneMass(bone,false)
            assert(finite(mass) and mass>0,'unproved physical mass')
            local angular=vector(component:GetPhysicsAngularVelocityInRadians(bone))
            local linear=vector(component:GetPhysicsLinearVelocityAtPoint(com,bone))
            return {bone=name,exists=found,frame=frame,com=com,mass=mass,
                angular=angular,linear_at_com=linear,queries=observations,
                native_frame_verified=false,scale_semantics_verified=false}
        end)
        return ok and row or nil,ok and nil or tostring(row)
    end
    function e.physics_body_exists(component,bone)
        local row=e.inspect_body(component,bone);return row and row.exists==true or false
    end
    return e
end
return M
