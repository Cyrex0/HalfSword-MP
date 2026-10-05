-- Original native released-actor capture. No networking/actor creation here.
-- env snapshots contain ONLY plain data; callback wrappers never survive a call.
-- UE4SS Blueprint hooks are POST-only. Observe original held/loaded actors on
-- a verified tick or earlier setup/load POST hook; never install a fake BP pre.
local M={}
M.HOOKS={
    drop='/Game/Assets/Weapons/Blueprints/ModularWeaponBP.ModularWeaponBP_C:Set Weapon Damping Drop',
    shoot='/Game/Assets/Weapons/Blueprints/Built_Weapons/Ranged/Weapon/BP_Weapon_Ranged_Weapon_Crossbow_Light.BP_Weapon_Ranged_Weapon_Crossbow_Light_C:Event Shoot',
    string_release='/Game/Assets/Weapons/Blueprints/Built_Weapons/Ranged/Weapon/BP_Weapon_Ranged_Weapon_Crossbow_Light.BP_Weapon_Ranged_Weapon_Crossbow_Light_C:Event String Released',
}
local function plain(v,seen)
    local t=type(v); if t=='number' then return v==v and math.abs(v)<math.huge end
    if t=='string' or t=='boolean' then return true end
    if t~='table' then return false end
    seen=seen or {}; if seen[v] then return false end; seen[v]=true
    for k,x in pairs(v) do if not plain(k,seen) or not plain(x,seen) then return false end end
    seen[v]=nil; return true
end
local function copy(v)
    if type(v)~='table' then return v end
    local r={}; for k,x in pairs(v) do r[k]=copy(x) end; return r
end
local function identity(a,b)
    return a and b and a.name==b.name and a.address==b.address and a.world==b.world
end
local function ctx(a,b)
    return a and b and a.peer==b.peer and a.match_id==b.match_id and a.round==b.round
        and a.life==b.life and identity(a.pawn,b.pawn) and a.world_key==b.world_key
end
function M.new(env,limit)
    return {env=env,limit=limit or 64,rows={},count=0,sequence=0}
end
function M.observe(s,route,actor)
    local c=s.env.context(); local a=s.env.snapshot(actor,route)
    if not c or not a or not plain(c) or not plain(a) or not c.verified
        or not c.alive or not c.life or c.life<1 or not c.match_id or not c.round
        or not identity(a.parent,c.pawn) or not identity(a.world_identity,c.world_identity)
        or not a.held or not identity(a.holder,c.pawn) then return nil,'context' end
    local source=a
    if route=='shot' then
        if not a.string_loaded or not a.bolt_loaded or not a.ammo or not a.ammo.is_ammo
            or a.ammo.being_fired~=false or not identity(a.ammo.parent,c.pawn)
            or not identity(a.ammo.world_identity,c.world_identity) then return nil,'loaded_ammo' end
        source=a.ammo
    elseif route~='throw' then return nil,'route' end
    if not source.identity or not source.class or not source.passport or not source.components
        or not source.module_generation then return nil,'configuration' end
    local at=s.env.now_ms()
    if type(at)~='number' or at~=at then return nil,'clock' end
    return {context=copy(c),source=copy(source),launcher=copy(a.identity),route=route,observed_ms=at}
end
function M.native_post(s,token)
    if not token or not plain(token) or not ctx(token.context,s.env.context()) then return nil,'context_changed' end
    local age=s.env.now_ms()-token.observed_ms
    if age<0 or age>250 then return nil,'stale_observation' end
    local now=s.env.resolve_snapshot(token.source.identity)
    if not now or not plain(now) or not identity(now.identity,token.source.identity)
        or not identity(now.world_identity,token.context.world_identity)
        or now.class~=token.source.class or now.module_generation~=token.source.module_generation
        or not s.env.equal_passport(now.passport,token.source.passport) then return nil,'source_changed' end
    if token.route=='shot' then
        if now.is_ammo~=true or now.being_fired~=true or not identity(now.parent,token.context.pawn) then return nil,'not_fired' end
    elseif now.held~=false or now.parent~=false or now.holder~=false then return nil,'not_released' end
    local key=now.identity.world..'|'..now.identity.name..'|'..tostring(now.identity.address)
    local old=s.rows[key]
    if old then
        if old.state=='active' and ctx(old.context,token.context) and old.route==token.route
            and old.source.module_generation==token.source.module_generation
            and s.env.equal_passport(old.source.passport,token.source.passport) then return copy(old),'duplicate' end
        return nil,'retained_incarnation'
    end
    if s.count>=s.limit then return nil,'capacity' end
    s.sequence=s.sequence+1; s.count=s.count+1
    local row={incarnation=s.sequence,context=copy(token.context),source=copy(token.source),
        route=token.route,state='active',world_id=nil}
    s.rows[key]=row; return copy(row)
end
function M.bind_world(s,identity_value,incarnation,id,epoch)
    for _,r in pairs(s.rows) do
        if identity(r.source.identity,identity_value) and r.incarnation==incarnation and r.state=='active'
            and epoch==r.context.world_epoch and type(id)=='number' and id>0 then
            if r.world_id and r.world_id~=id then return false end
            r.world_id=id; return true
        end
    end
    return false
end
function M.terminal(s,identity_value,why)
    for _,r in pairs(s.rows) do
        if identity(r.source.identity,identity_value) then r.state=why or 'terminal'; return true end
    end
    return false
end
function M.drop_world(s,world_key)
    for k,r in pairs(s.rows) do
        if r.context.world_key==world_key then s.rows[k]=nil; s.count=s.count-1 end
    end
end
return M
