-- Explicit dev construction smoke; never runs on import. Uses bounded replay
-- source factory, never assigns pawn hand fields or creates an active collider.
local M={}
local function identity(a)
    if not a or not a:IsValid()then return false end
    return a:GetFullName()..'@'..tostring(a:GetAddress())
end
local function same(a,b)return a and b and a.match_id==b.match_id and a.round==b.round and a.life==b.life end
function M.run(e,pawn,left,original)
    if e.authorized~=true then return nil,'dev_spawn_not_authorized' end
    local ok,result=pcall(function()
        assert(same(original,e.context(pawn)),'original generation mismatch')
        local p=identity(pawn);assert(p,'invalid pawn')
        local l,r=identity(pawn['Weapon L']),identity(pawn['Weapon R'])
        local c=e.factory.component(pawn,left,10,original)
        assert(c and c:IsValid(),'native fist component unavailable')
        assert(c:GetClass():GetFName():ToString()=='SphereComponent','wrong native component class')
        local a=c:GetOwner();assert(a and a:IsValid(),'missing fist owner')
        assert(a:GetClass():GetFName():ToString()=='Weapon_Fists_C','wrong native actor class')
        assert(identity(a:GetOwner())==p and identity(a['Parent Actor'])==p
            and identity(a['Last Parent'])==p,'native lineage mismatch')
        assert(identity(pawn)==p and same(original,e.context(pawn)),'generation changed')
        assert(identity(pawn['Weapon L'])==l and identity(pawn['Weapon R'])==r,'held weapon mutated')
        local scale=c:GetSocketTransform(FName('None'),0).Scale3D
        assert(scale.X==1 and scale.Y==1 and scale.Z==1,'native fist Sphere scale is not unit')
        local radius=c:GetUnscaledSphereRadius();local scaled=c:GetScaledSphereRadius()
        assert(radius==13 and scaled==13,'native Sphere radius differs from verified asset')
        assert(c:GetCollisionEnabled()==0 and a:GetActorEnableCollision()==false
            and a['Temp Disable Damage']==true,'native source collision/damage guard')
        return {pawn=p,actor=identity(a),component=identity(c),left=left,
            match_id=original.match_id,round=original.round,life=original.life,
            radius=radius,scaled_radius=scaled,scale={scale.X,scale.Y,scale.Z},collision=0,held_fields_unchanged=true}
    end)
    return ok and result or nil,ok and nil or tostring(result)
end
return M
