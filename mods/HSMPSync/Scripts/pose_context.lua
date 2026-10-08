-- Source context is evidence from the original verified placement, never the
-- generation current when a queued pose happens to be read.
local M={}
function M.of(status, view, mode, peer, pawn)
    if type(status)~="table" or not view or status.verified~=true or status.pawn~=pawn
        or status.match_id~=view.match_id or not status.match_id or status.match_id==0 then return nil end
    local phase=view.state or view.phase_name
    if phase=="loading" then phase="countdown" end
    local round=(phase=="countdown" or phase=="paused") and view.spawn_round or view.round
    local order=view.spawns and view.spawns[peer]
    if not order or status.spawn_id~=order.spawn_id or status.round~=round or (status.life or 0)<1 then return nil end
    if phase=="live" or phase=="roundover" then
        local row=mode and mode.rows and mode.rows[peer]
        if not row or mode.match_id~=status.match_id or mode.round~=status.round or row.life~=status.life
            or row.respawning then return nil end
    elseif status.life~=1 then return nil end
    return {match_id=status.match_id,round=status.round,life=status.life}
end
-- Publication during an explicitly ordered deathmatch reload supplies the
-- evidence Ready needs before LOADED. It grants no active life/death authority.
function M.for_publication(status, view, mode, peer, pawn)
    local active=M.of(status,view,mode,peer,pawn)
    if active then return active end
    if type(status)~="table" or not view or status.verified~=true or status.pawn~=pawn
        or not status.match_id or status.match_id==0 or status.match_id~=view.match_id
        or (view.state or view.phase_name)~="live" or status.round~=view.round then return nil end
    local row=mode and mode.rows and mode.rows[peer]
    local order=view.spawns and view.spawns[peer]
    local sid=math.tointeger(status.spawn_id)
    local life=math.tointeger(status.life)
    if not mode or mode.id~="deathmatch" or mode.match_id~=status.match_id or mode.round~=status.round
        or not row or row.respawning~=true or not life or life<1 or row.life~=life
        or not order or order.spawn_id~=sid or not sid or sid<1
        or (sid >> 8)~=status.round or (sid & 0x80)==0 or (sid & 0x7f)~=(life & 0x7f) then return nil end
    return {match_id=status.match_id,round=status.round,life=life}
end
return M
