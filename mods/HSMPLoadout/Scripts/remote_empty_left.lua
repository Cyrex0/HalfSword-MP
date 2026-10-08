-- An absent appearance hand is native None, not an invented Weapon_Fists.
-- Native Left setup tears down even an offhand constraint, so call it only
-- for an actual unwanted L actor. Debt contains scalar identities only.
local M = {}
local function integer(v) return type(v)=="number" and math.tointeger(v) end
local function identity(actor)
    local ok, value = pcall(function()
        local address=actor:GetAddress()
        if not integer(address) or address<=0 or not actor:IsValid() or actor:IsActorBeingDestroyed() then return nil end
        local name,class=actor:GetFName():ToString(),actor:GetClass():GetFName():ToString()
        if type(name)~="string" or name=="" or type(class)~="string" or class=="" then return nil end
        return {address=address,name=name,class=class}
    end)
    return ok and value or nil
end
-- Pinned UE4SS wraps nullptr as a UObject whose address is zero. Unknown
-- properties use special_invalid_ptr instead; IsValid false is not None proof.
function M.left(pawn)
    local ok, actor, address = pcall(function()
        local value=pawn["Weapon L"]
        return value,value:GetAddress()
    end)
    if not ok or not integer(address) or address<0 then return nil,"L field unavailable" end
    if address==0 then return {empty=true} end
    local id=identity(actor)
    if not id then return nil,"L actor unavailable" end
    return {actor=actor,id=id}
end
function M.scope_key(c)
    if type(c)~="table" or type(c.world)~="string" or c.world==""
        or type(c.pawn)~="string" or c.pawn=="" or not integer(c.address) or c.address<=0
        or not integer(c.mesh) or c.mesh<=0 or type(c.mesh_name)~="string" or c.mesh_name=="" or not integer(c.peer) or c.peer<=0
        or not integer(c.match_id) or c.match_id<=0 or not integer(c.round) or c.round<=0
        or not integer(c.life) or c.life<=0 or not integer(c.spawn_id) or c.spawn_id<=0 then return nil end
    return table.concat({c.world,c.pawn,c.address,c.mesh,c.mesh_name,c.peer,c.match_id,c.round,c.life,c.spawn_id},"|")
end
-- The exact native None literal used by Willie_BP's cleanup call @669253.
-- Class/module fields stay absent; materials/colours are native struct defaults,
-- not the previous weapon's passport and not guessed zero materials.
function M.none_record()
    return {id=0,name="None",head_size={0,0,0},guard_size={0,0,0},grip_size={0,0,0},pommel_size={0,0,0},
        mass_head=0,mass_guard=0,mass_grip=0,mass_pommel=0,mat_steel=1,mat_colored=4,mat_wood=14,mat_leather=10,
        color_wood={1,1,1,1},color_leather={0.186343,0.074751,0.06444,1},price=0,tier=0}
end
function M.new(api)
    local self={debts={},uncertain={}}
    function self:reset() self.debts,self.uncertain={},{} end
    local function scope(pawn,peer)
        local ok,c=pcall(api.context,pawn,peer)
        return ok and c or nil
    end
    local function current(pawn,peer,key)
        return key~=nil and M.scope_key(scope(pawn,peer))==key
    end
    local function owned(pawn,actor,c)
        local ok,yes=pcall(function()
            local parent=actor["Parent Actor"]
            return parent:GetAddress()==c.address and actor:GetWorld():GetAddress()==c.world_address
                and pawn:GetAddress()==c.address
        end)
        return ok and yes==true
    end
    function self:allow(pawn,peer)
        local uncertain=self.uncertain[peer]
        if not uncertain then return true end
        local key=M.scope_key(scope(pawn,peer))
        if not key then return false,"FAIL uncertain None context unavailable" end
        if key==uncertain.key then return false,uncertain.reason end
        self.uncertain[peer]=nil;return true
    end
    -- wanted identifies a real R weapon only. nil R keeps its existing path;
    -- explicit fists/feet cannot be safely reused through a grip0 actor call.
    function self:prepare(pawn,peer,wanted)
        local c=scope(pawn,peer)
        local key=M.scope_key(c)
        if not key then return false,"FAIL empty L context unavailable" end
        local uncertain=self.uncertain[peer]
        if uncertain and uncertain.key==key then return false,uncertain.reason end
        if uncertain then self.uncertain[peer]=nil end
        local wanted_key=wanted and api.wanted_key(wanted) or "-"
        local debt=self.debts[peer]
        if debt and (debt.key~=key or debt.wanted~=wanted_key) then self.debts[peer]=nil end
        local left,why=M.left(pawn)
        if not left then return false,"FAIL "..why end
        if left.actor and not owned(pawn,left.actor,c) then return false,"FAIL L ownership unavailable" end
        local right
        if left.actor and wanted then
            local ok,actor=pcall(function()return pawn["Weapon R"]end)
            local id=ok and identity(actor)
            if id and owned(pawn,actor,c) and api.matches(actor,wanted) then right=id end
        end
        -- No passport change before positive native field/context proof.
        if not current(pawn,peer,key) then return false,"FAIL empty L context changed" end
        if not api.clear_passport(pawn) then return false,"FAIL hand passport" end
        if not current(pawn,peer,key) then return false,"FAIL empty L context changed" end
        if left.empty then return true,"none" end
        local ok=pcall(api.cleanup,pawn,M.none_record())
        if not ok then
            self.uncertain[peer]={key=key,reason="FAIL native None cleanup uncertain"}
            return false,self.uncertain[peer].reason
        end
        if not current(pawn,peer,key) then
            self.uncertain[peer]={key=key,reason="FAIL native None context changed"}
            return false,self.uncertain[peer].reason
        end
        local after=M.left(pawn)
        if not after or not after.empty then
            self.uncertain[peer]={key=key,reason="FAIL native None readback uncertain"}
            return false,self.uncertain[peer].reason
        end
        api.replaced()
        -- Only completed native L cleanup can create a rebind obligation.
        if right then self.debts[peer]={key=key,wanted=wanted_key,right=right} end
        return true,"none(cleaned)"
    end
    function self:finish(pawn,peer,wanted)
        local allowed,reason=self:allow(pawn,peer)
        if not allowed then return false,reason end
        local debt=self.debts[peer]
        if not debt then return true end
        local c=scope(pawn,peer)
        local key=M.scope_key(c)
        if not key then return false,"FAIL retained R context unavailable" end -- keep metadata, never use it
        if key~=debt.key or not wanted or api.wanted_key(wanted)~=debt.wanted then
            self.debts[peer]=nil;return false,"FAIL retained R context changed"
        end
        local left=M.left(pawn)
        if not left or not left.empty then return false,"FAIL retained R needs empty L" end
        local ok,actor=pcall(function()return pawn["Weapon R"]end)
        local id=ok and identity(actor)
        if not id or not owned(pawn,actor,c) then return false,"FAIL retained R actor unavailable" end
        if id.address~=debt.right.address or id.name~=debt.right.name or id.class~=debt.right.class then
            -- Ordinary native application replaced R with the desired actor;
            -- the old actor must never be retrieved/rebound from metadata.
            if api.matches(actor,wanted) then self.debts[peer]=nil;return true end
            return false,"FAIL retained R actor changed"
        end
        if not api.matches(actor,wanted) then return false,"FAIL retained R passport changed" end
        local read,grip=pcall(function()return pawn["R_GripType_Current"]end)
        if not read or not integer(grip) or grip<=0 or grip>255 then return false,"FAIL retained R grip unavailable/zero" end
        if not current(pawn,peer,key) then return false,"FAIL retained R context changed" end
        local function uncertain(why)
            self.uncertain[peer]={key=key,reason=why}
            return false,why
        end
        local done,result,stage=pcall(api.reequip,pawn,actor,wanted)
        if done and not result and stage=="preflight" then return false,"FAIL retained R preflight" end
        if not done or not result or stage~="complete" then return uncertain("FAIL retained R native completion uncertain") end
        if not current(pawn,peer,key) then return uncertain("FAIL retained R native context uncertain") end
        local read,proved=pcall(function()
            local final_actor=pawn["Weapon R"]
            local final=identity(final_actor)
            return final and final.address==id.address and final.name==id.name and final.class==id.class
                and api.matches(final_actor,wanted)
        end)
        if not read or not proved then
            return uncertain("FAIL retained R native readback uncertain")
        end
        self.debts[peer]=nil
        api.replaced() -- Avatars must discover the newly created native constraint.
        return true,"ok(same actor after None)"
    end
    -- Explicit L is a different hand operation, but cannot repair unobserved
    -- partial weight accounting from an ambiguous cleanup on this same body.
    function self:forget(peer) self.debts[peer]=nil end
    return self
end
return M
