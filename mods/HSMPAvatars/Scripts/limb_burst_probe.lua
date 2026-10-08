-- Default-OFF diagnostic: one three-drive burst, at most twelve phase rows.
-- SDK getters only. Socket frames are not independent rigid-body authority.
local M={MAX_DRIVES=3,MAX_ROWS=12,WINDOW_MS=500,BONES={"upperarm_l","lowerarm_l","hand_l"}}
local joints={{"UserConstraint_14","clavicle_l","upperarm_l"},{"UserConstraint_15","upperarm_l","lowerarm_l"},
    {"UserConstraint_16","lowerarm_l","hand_l"}}
local function number(v)assert(type(v)=="number"and v==v and math.abs(v)<math.huge,"numeric read unavailable");return v end
local function boolean(v)assert(type(v)=="boolean","boolean read unavailable");return v end
local function vec(v)return {number(v.X),number(v.Y),number(v.Z)}end
local function identity(o)
    assert(o and o:IsValid()==true,"identity unavailable")
    local a,n=number(o:GetAddress()),o:GetFName():ToString()
    assert(math.tointeger(a)and a>0 and type(n)=="string"and n~="","identity unavailable")
    return {address=a,name=n}
end
local function same(a,b)return a and b and a.address==b.address and a.name==b.name end
local function out(name,...)
    for i=1,select("#",...)do local t=select(i,...);if type(t)=="table"and t[name]~=nil then return t[name]end end
    error("out parameter unavailable: "..name,0)
end
local function attempt(row,key,fn)
    local ok,v=pcall(fn);if ok then row[key]={available=true,value=v}else row[key]={available=false,reason=tostring(v):sub(1,160)}end
end
local function transform(t)
    local q=t.Rotation
    return {p=vec(t.Translation),q={number(q.X),number(q.Y),number(q.Z),number(q.W)}}
end
local function copy(v,depth)
    if type(v)=="table"then
        assert((depth or 0)<12,"scalar copy depth")
        local out,n={},0;for k,x in pairs(v)do n=n+1;assert(n<=128,"scalar copy capacity");out[k]=copy(x,(depth or 0)+1)end;return out
    end
    assert(v==nil or type(v)=="string"or type(v)=="boolean"or type(v)=="number","native wrapper in scalar record")
    if type(v)=="number"then number(v)end
    return v
end
local function key(c)
    if type(c)~="table"then return nil end
    local a={}
    for _,k in ipairs({"world","generation","peer","match_id","round","life","source_cut"})do
        if c[k]==nil then return nil end;a[#a+1]=tostring(c[k])
    end
    for _,k in ipairs({"pawn","mesh"})do
        local v=c[k];if type(v)~="table"or not v.address or not v.name then return nil end
        a[#a+1]=v.address;a[#a+1]=v.name
    end
    return table.concat(a,"|")
end
local function asset(mesh)
    local sk=mesh.SkeletalMesh
    local override=mesh.PhysicsAssetOverride
    local oa=override:GetAddress() -- positive native null differs from unreadable.
    assert(type(oa)=="number"and math.tointeger(oa)and oa>=0,"asset override unavailable")
    return {skeletal_mesh=identity(sk),physics_asset=identity(oa==0 and sk:GetPhysicsAsset()or override),override=oa~=0}
end
local function accessor(mesh,lib,j,e)
    assert(e.current(),"scope changed")
    local a=mesh:GetConstraintByName(e.fname(j[1]),false)
    assert(e.current(),"scope changed")
    local owner=identity(a.Owner:get())
    assert(same(owner,identity(mesh)),"accessor owner mismatch")
    local index=number(a.Index);assert(math.tointeger(index)and index>=0 and index<32,"accessor index unavailable")
    local ref={Owner=a.Owner,Index=index};local p,c={},{}
    assert(e.current(),"scope changed")
    lib:GetAttachedBodyNames(ref,p,c)
    assert(e.current(),"scope changed")
    local parent,child=out("ParentBody",p,c):ToString(),out("ChildBody",p,c):ToString()
    assert(parent==j[2]and child==j[3],"constraint endpoint mismatch")
    return ref,{owner=owner,index=index,parent=parent,child=child}
end
local function joint(mesh,lib,j,e)
    assert(e.current(),"scope changed")
    local before=asset(mesh)
    local ref,id=accessor(mesh,lib,j,e)
    local r={name=j[1],owner=id.owner,index=id.index,parent=id.parent,child=id.child,asset=before}
    local function read(name,fields,types)
        assert(e.current(),"scope changed")
        local args={};for i=1,#fields do args[i]={}end
        assert(lib[name]~=nil,"getter unavailable: "..name)
        lib[name](lib,ref,table.unpack(args))
        assert(e.current(),"scope changed")
        local v={};for i,f in ipairs(fields)do local x=out(f,table.unpack(args));if types and types[i]=="bool"then v[f]=boolean(x)else v[f]=number(x)end end
        return v
    end
    attempt(r,"angular_limits",function()return read("GetAngularLimits",{"Swing1MotionType","Swing1LimitAngle","Swing2MotionType","Swing2LimitAngle","TwistMotionType","TwistLimitAngle"})end)
    attempt(r,"linear_limits",function()return read("GetLinearLimits",{"XMotion","YMotion","ZMotion","Limit"})end)
    attempt(r,"drive_params",function()return read("GetAngularDriveParams",{"OutPositionStrength","OutVelocityStrength","OutForceLimit"})end)
    attempt(r,"drive_mode",function()return read("GetAngularDriveMode",{"OutDriveMode"})end)
    attempt(r,"orientation_slerp",function()return read("GetOrientationDriveSLERP",{"bOutEnableSLERP"},{"bool"})end)
    attempt(r,"velocity_slerp",function()return read("GetAngularVelocityDriveSLERP",{"bOutEnableSLERP"},{"bool"})end)
    attempt(r,"orientation_twist_swing",function()return read("GetOrientationDriveTwistAndSwing",{"bOutEnableTwistDrive","bOutEnableSwingDrive"},{"bool","bool"})end)
    attempt(r,"velocity_twist_swing",function()return read("GetAngularVelocityDriveTwistAndSwing",{"bOutEnableTwistDrive","bOutEnableSwingDrive"},{"bool","bool"})end)
    attempt(r,"orientation_target",function()
        assert(e.current(),"scope changed")
        local t={};lib:GetAngularOrientationTarget(ref,t)
        assert(e.current(),"scope changed")
        local v=out("OutPosTarget",t);return {number(v.Pitch),number(v.Yaw),number(v.Roll)}
    end)
    attempt(r,"velocity_target",function()
        assert(e.current(),"scope changed")
        local t={};lib:GetAngularVelocityTarget(ref,t)
        assert(e.current(),"scope changed")
        return vec(out("OutVelTarget",t))
    end)
    local _,after=accessor(mesh,lib,j,e);local aa=asset(mesh)
    assert(after.index==id.index and same(after.owner,id.owner)and same(before.skeletal_mesh,aa.skeletal_mesh)
        and same(before.physics_asset,aa.physics_asset)and before.override==aa.override,"constraint identity changed")
    return r
end
local function pa_list(actor,mesh,c,e)
    local function collect()
        local arr=actor["Phys Anim Array"];local count=#arr
        assert(math.tointeger(count)and count>=0 and count<=8,"PA collection count unavailable/overflow")
        local list,seen,overflow={},{},false
        arr:ForEach(function(_,v)
            if #list>=count then overflow=true;return true end
            local pa=v:get();local id=identity(pa)
            assert(not seen[id.address..":"..id.name],"duplicate PA collection entry")
            seen[id.address..":"..id.name]=true;list[#list+1]={object=pa,id=id}
        end)
        assert(not overflow and #list==count and #arr==count,"PA collection incomplete")
        return list
    end
    local first=collect();local cached={}
    for i,id in ipairs(e.cached or {})do
        assert(i<=8,"cached PA capacity")
        assert(type(id)=="table"and type(id.address)=="number"and type(id.name)=="string","cached scalar identity unavailable")
        cached[i]={available=true,identity=copy(id)}
    end
    local primary=actor.PhysicalAnimation
    local primary_id=identity(primary)
    local list={{object=primary,id=primary_id,source="PhysicalAnimation"}}
    for _,row in ipairs(first)do row.source="Phys Anim Array";list[#list+1]=row end
    local values={}
    for _,row in ipairs(list)do
        local r={identity=row.id,source=row.source,cached=false};values[#values+1]=r
        for _,old in ipairs(cached)do if old.available and same(old.identity,row.id)then r.cached=true end end
        attempt(r,"binding",function()
            local owner=row.object:GetOwner();assert(same(identity(owner),c.pawn),"PA owner mismatch")
            local world=owner:GetWorld();local wid=world:GetAddress().."@"..world:GetFullName()
            assert(c.world:match("%|(.+)$")==wid,"PA world mismatch")
            local bound=identity(row.object.SkeletalMeshComponent)
            return {mesh=bound,current_mesh=same(bound,identity(mesh)),strength=number(row.object.StrengthMultiplyer),world=wid}
        end)
    end
    local second=collect();assert(#first==#second and same(primary_id,identity(actor.PhysicalAnimation)),"PA membership changed")
    for i,row in ipairs(first)do assert(same(row.id,second[i].id),"PA membership changed")end
    for i,row in ipairs(list)do
        local r=values[i]
        if r.binding.available then
            assert(same(identity(row.object:GetOwner()),c.pawn)and same(identity(row.object.SkeletalMeshComponent),r.binding.value.mesh),"PA binding changed")
        end
    end
    return {entries=values,cached=cached,cached_available=e.cached~=nil,array_count=#first,array_complete=true}
end
function M.new(emit)
    local s={drives=0,rows=0,attempts=0,phases={},closed=false}
    function s:want(fault,now)
        if self.started and now and (now<self.started or now-self.started>M.WINDOW_MS)then self.closed=true end
        return not self.closed and (self.key~=nil or type(fault)=="table"and type(fault.settle_rot_deg)=="number"
            and fault.settle_rot_deg>10 and tostring(fault.settle_reason):find("_l",1,true)~=nil)
    end
    function s:attempt(fault,now)
        if not self:want(fault,now)or self.attempts>=24 then self.closed=self.attempts>=24 or self.closed;return false end
        self.attempts=self.attempts+1;return true
    end
    function s:capture(c,phase,e,params,fault)
        if self.closed or self.rows>=M.MAX_ROWS or not self:want(fault)then return nil end
        -- A failed current check ends this entire burst. Independent optional
        -- getter failures cannot allow a later apparently healthy scope to
        -- resume reads through the earlier accessor or component wrappers.
        local original=e
        local lost=false
        e=setmetatable({current=function()
            if lost or self.closed then return false end
            local ok,valid=pcall(original.current)
            if not ok or valid~=true then lost=true;self.closed=true;return false end
            return true
        end},{__index=original})
        local k=key(c);if not k then return nil end
        if self.key and (self.key~=k or c.at<self.started or c.at-self.started>M.WINDOW_MS)then self.closed=true;return nil end
        local phasekey=c.frame..":"..phase
        if self.phases[phasekey]then return nil end
        if phase=="pre_driver"then
            if self.drives>=M.MAX_DRIVES or self.frame and c.frame~=self.frame+1 then self.closed=true;return nil end
            self.key,self.started=self.key or k,self.started or c.at
            self.trigger=self.trigger or copy({reason=fault and fault.settle_reason,
                max_arm_rotation_deg=fault and fault.settle_rot_deg,max_arm_position_uu=fault and fault.settle_pos_uu,
                sample_ms=fault and fault.settle_sample_ms,source_seq=fault and fault.settle_source_seq,
                source_ts=fault and fault.settle_source_ts,meaning="arm maxima and last failing bone; burst candidate, not joint angle"})
            self.drives=self.drives+1;self.frame=c.frame;self.source_seq=c.source_seq
        elseif not self.key or c.frame~=self.frame or c.source_seq~=self.source_seq then return nil end
        self.phases[phasekey]=true;self.rows=self.rows+1 -- attempted reads are bounded too.
        if not e.current()then self.closed=true;return nil end
        local r={instance=c.instance,peer=c.peer,pawn=c.pawn,mesh=c.mesh,world=c.world,generation=c.generation,
            match_id=c.match_id,round=c.round,life=c.life,cut=c.source_cut,source_seq=c.source_seq,source_mode=c.source_mode,
            source_age=c.source_age,source_pt=c.source_pt,frame=c.frame,at_ms=c.at,phase=phase,drive_number=self.drives,
            params=params,audit=c.audit,trigger=self.trigger,qualification=c.audit and c.audit.qualification or false,authority=false,read_only=true,
            socket_frame_authority=false,bones={},joints={}}
        if phase=="pre_driver"then self.params=copy(params)end
        r.driver_params=copy(self.params)
        attempt(r,"physics_asset",function()return asset(e.mesh)end)
        if r.physics_asset.available then
            local a=r.physics_asset.value;local ak=a.skeletal_mesh.address..":"..a.skeletal_mesh.name..":"..a.physics_asset.address..":"..a.physics_asset.name..":"..tostring(a.override)
            if self.asset_key and self.asset_key~=ak then self.closed=true;return nil end
            self.asset_key=ak
        else r.constraint_generation_available=false end
        local driver
        attempt(r,"driver_component",function()
            driver=e.actor.DriverSkeleton
            assert(same(identity(driver:GetOwner()),c.pawn),"DriverSkeleton owner mismatch")
            local owner=driver:GetOwner();local w=owner:GetWorld()
            assert(w:GetAddress().."@"..w:GetFullName()==c.world:match("%|(.+)$"),"DriverSkeleton world mismatch")
            return identity(driver)
        end)
        for _,bone in ipairs(M.BONES)do
            if not e.current()then self.closed=true;return nil end
            local b={bone=bone};r.bones[#r.bones+1]=b
            attempt(b,"angular_velocity_deg_s",function()return vec(e.mesh:GetPhysicsAngularVelocityInDegrees(e.fname(bone)))end)
            if not e.current()then self.closed=true;return nil end
            attempt(b,"mesh_socket_world",function()return transform(e.mesh:GetSocketTransform(e.fname(bone),0))end)
            if not e.current()then self.closed=true;return nil end
            attempt(b,"driver_socket_world",function()
                assert(r.driver_component.available,"DriverSkeleton identity unavailable")
                assert(same(identity(e.actor.DriverSkeleton),r.driver_component.value),"DriverSkeleton replaced")
                assert(same(identity(driver:GetOwner()),c.pawn),"DriverSkeleton owner mismatch")
                return {identity=identity(driver),transform=transform(driver:GetSocketTransform(e.fname(bone),0))}
            end)
            if not e.current()then self.closed=true;return nil end
            if e.pose then attempt(b,"pose",function()
                local values=e.pose(bone);local row={}
                for _,which in ipairs({"decoded","aim","prior"})do
                    local t=values[which]
                    if t then local copied={};for i=1,#t do copied[i]=number(t[i])end;row[which]=copied end
                end
                return row
            end)end
        end
        for _,j in ipairs(joints)do
            if not e.current()then self.closed=true;return nil end
            local row={name=j[1]};r.joints[#r.joints+1]=row
            attempt(row,"current",function()return joint(e.mesh,assert(e.library,"constraint library unavailable"),j,e)end)
        end
        if not e.current()then self.closed=true;return nil end
        attempt(r,"physical_animation",function()return pa_list(e.actor,e.mesh,c,e)end)
        if not e.current()then self.closed=true;return nil end
        if r.driver_component.available then
            local valid=pcall(function()
                assert(same(identity(e.actor.DriverSkeleton),r.driver_component.value)
                    and same(identity(e.actor.DriverSkeleton:GetOwner()),c.pawn),"DriverSkeleton changed")
            end)
            if not valid then
                r.driver_component={available=false,reason="DriverSkeleton changed"}
                for _,b in ipairs(r.bones)do b.driver_socket_world={available=false,reason="DriverSkeleton changed"}end
            end
        end
        if not e.current()then self.closed=true;return nil end
        r=copy(r)
        if self.rows>=M.MAX_ROWS then self.closed=true end
        emit(r);return r
    end
    return s
end
return M
