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
return M
