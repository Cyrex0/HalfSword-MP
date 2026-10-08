-- Read-only diagnostic admission, never spawn readiness or combat authority.
-- Native identities supplied by the caller are fresh, copied scalar values.
local M={}
local function positive(v)
    return type(v)=="number" and v>0 and v<math.huge and v%1==0
end
local function named(v)return type(v)=="string" and v~="" end
function M.resolve(e)
    local v,s=e.view,e.status
    local peer=e.peer==0 and v and v.my_peer_id or e.peer
    if not v or not s or not positive(peer) or not positive(v.match_id)
        or type(v.round)~="number" or v.round<0 or v.round>=math.huge or v.round%1~=0
        or not named(e.world) or not named(e.world_key) or not named(e.pawn)
        or not positive(e.address) or not positive(e.mesh_address) or not named(e.mesh)
        or s.pawn~=e.pawn or s.match_id~=v.match_id then return nil end
    local pending=v.state=="loading" or v.state=="countdown"
    local round,life,order
    if pending then
        round,life=v.pending_round,1
        order=v.spawns and v.spawns[peer]
        if not positive(round) or round~=v.spawn_round or round~=v.round+1
            or not order or not positive(order.spawn_id) or (order.spawn_id >> 8)~=round then return nil end
    else
        local mode=e.mode
        local row=mode and mode.rows and mode.rows[peer]
        if not mode or mode.match_id~=v.match_id or mode.round~=v.round
            or not row or not positive(row.life) then return nil end
        round,life=mode.round,row.life
        order=v.spawns and v.spawns[peer]
    end
    if s.round~=round or s.life~=life then return nil end
    if e.peer==0 then
        if not order or s.spawn_id~=order.spawn_id or (not pending and s.verified~=true) then return nil end
    elseif s.peer~=peer then return nil end
    return {peer=e.peer,owner_peer=peer,match_id=s.match_id,round=s.round,life=s.life,
        pawn=s.pawn,address=e.address,pawn_address=e.address,mesh=e.mesh,mesh_address=e.mesh_address,
        world=e.world,world_key=e.world_key,spawn_id=order and order.spawn_id,
        placement_verified=e.peer==0 and (type(s.verified)=="boolean" and tostring(s.verified) or "unknown") or "unknown",
        display_time=s.body_ts or s.local_ms}
end
function M.same(a,b)
    if not a or not b then return false end
    for _,key in ipairs({"peer","owner_peer","match_id","round","life","pawn","address",
        "mesh","mesh_address","world","world_key","spawn_id"})do
        if a[key]~=b[key] then return false end
    end
    return true
end
return M
