-- DEV ONLY: synchronous read-only frame/joint comparison. Not installed in a
-- tick/hook. Caller supplies current, independently verified pawn/mesh context.
-- All returned values are copied scalars; native wrappers never escape.
local M={}
local function num(v)
    assert(type(v)=="number" and v==v and math.abs(v)<math.huge,"nonfinite native scalar")
    return v
end
local function vec(v)return {num(v.X),num(v.Y),num(v.Z)}end
local function frame(t)
    local q=t.Rotation
    return {p=vec(t.Translation),q={num(q.X),num(q.Y),num(q.Z),num(q.W)},scale=vec(t.Scale3D)}
end
local function attempt(row,key,f)
    local ok,v=pcall(f)
    if ok then row[key]=v else row.errors[key]=tostring(v) end
end
function M.capture(mesh,id,bones,e)
    if not e.current(id,mesh) then return nil,"original_context_unverified" end
    if type(bones)~="table" or #bones<1 or #bones>23 then return nil,"bone_count" end
    local out={rows={},read_only=true,physics_frame_verified=false,
        match_id=id.match_id,round=id.round,life=id.life,pawn=id.pawn,pawn_address=id.pawn_address,
        mesh_address=mesh:GetAddress(),at=e.now(),display_time=id.display_time}
    local seen={}
    for _,bone in ipairs(bones) do
        if type(bone)~="string" or seen[bone] or not e.allowed_bone(bone) then return nil,"bone_dictionary" end
        seen[bone]=true
        local fn=e.fname(bone)
        local row={bone=bone,errors={}}
        attempt(row,"socket",function()return frame(mesh:GetSocketTransform(fn,0))end)
        attempt(row,"physics",function()return frame(e.physics:GetPhysicsObjectWorldTransform(mesh,fn))end)
        attempt(row,"com",function()return vec(mesh:GetCenterOfMass(fn))end)
        attempt(row,"mass",function()return num(mesh:GetBoneMass(fn,false))end)
        attempt(row,"angular_deg",function()return vec(mesh:GetPhysicsAngularVelocityInDegrees(fn))end)
        attempt(row,"joint_angles_deg",function()
            -- Pinned UE4SS scalar OutParm contract populates parameter-name
            -- keys, unlike its array/struct out-table contract.
            local s1,tw,s2={},{},{}
            mesh:GetCurrentJointAngles(fn,s1,tw,s2)
            return {swing1=num(s1.Swing1Angle),twist=num(tw.TwistAngle),swing2=num(s2.Swing2Angle)}
        end)
        if e.target then attempt(row,"target",function()
            local t=e.target(bone)
            if not t then return nil end
            local r={};for i=1,13 do if t[i]~=nil then r[i]=num(t[i])end end
            return r
        end)end
        out.rows[#out.rows+1]=row
    end
    if e.constraint_library and e.joints then
        if #e.joints>32 then return nil,"joint_dictionary_capacity" end
        out.joints={}
        for _,joint in ipairs(e.joints) do
            local row={name=joint.name,errors={}}
            attempt(row,"current",function()
                local a=mesh:GetConstraintByName(e.fname(joint.name),false)
                -- Accessor is a reflected struct with a weak Owner and Index.
                -- The native by-ref parameter requires a Lua table; copy the
                -- actual weak handle rather than inventing pointer offsets.
                local accessor={Owner=a.Owner,Index=a.Index}
                assert(type(accessor.Index)=="number" and accessor.Index>=0 and accessor.Index<32,"invalid constraint accessor")
                local lib=e.constraint_library
                local parent,child={},{}
                lib:GetAttachedBodyNames(accessor,parent,child)
                local pn,cn=parent.ParentBody:ToString(),child.ChildBody:ToString()
                assert(pn==joint.parent and cn==joint.child,"constraint endpoint mismatch")
                local s1,sl1,s2,sl2,tw,tl={},{},{},{},{},{}
                lib:GetAngularLimits(accessor,s1,sl1,s2,sl2,tw,tl)
                local ps,vs,fl={},{},{}
                lib:GetAngularDriveParams(accessor,ps,vs,fl)
                local x,y,z,limit={},{},{},{}
                lib:GetLinearLimits(accessor,x,y,z,limit)
                return {parent=pn,child=cn,index=accessor.Index,
                    swing1_motion=num(s1.Swing1MotionType),swing1_limit=num(sl1.Swing1LimitAngle),
                    swing2_motion=num(s2.Swing2MotionType),swing2_limit=num(sl2.Swing2LimitAngle),
                    twist_motion=num(tw.TwistMotionType),twist_limit=num(tl.TwistLimitAngle),
                    angular_strength=num(ps.OutPositionStrength),angular_damping=num(vs.OutVelocityStrength),angular_force_limit=num(fl.OutForceLimit),
                    linear_x=num(x.XMotion),linear_y=num(y.YMotion),linear_z=num(z.ZMotion),linear_limit=num(limit.Limit)}
            end)
            out.joints[#out.joints+1]=row
        end
    end
    if not e.current(id,mesh) then return nil,"original_context_changed" end
    return out
end
return M
