-- A veto for NEW source records only. Unknown participation never changes the
-- existing claim path; queued records and approved trades keep their own scope.
local M = { MAX_ROWS = 64 }
local function integer(v, low)
    return type(v)=="number" and math.tointeger(v) and v>=low
end
local function header(info, fresh_s)
    return type(info)=="table" and (info.sidecar_state=="ready" or info.sidecar_state==2)
        and type(info.sidecar_hb_age_s)=="number" and info.sidecar_hb_age_s==info.sidecar_hb_age_s
        and info.sidecar_hb_age_s>=0 and info.sidecar_hb_age_s<=fresh_s
end
local function pair(rows, a, b)
    if type(rows)~="table" then return end
    local first,second,na,nb,n=nil,nil,0,0,0
    for k,row in pairs(rows)do
        n=n+1
        if n>M.MAX_ROWS or not integer(k,1) or k>M.MAX_ROWS or type(row)~="table"
            or not integer(row.peer_id,1) then return end
        if row.peer_id==a then first=row;na=na+1 end
        if row.peer_id==b then second=row;nb=nb+1 end
    end
    if n~=#rows or na~=1 or nb~=1 then return end
    return first,second
end

function M.known_down(ipc, expected, fresh_s)
    local ok,down,reason=pcall(function()
        local q=expected
        if type(q)~="table" or not integer(q.attacker,1) or not integer(q.target,1) or q.attacker==q.target
            or not integer(q.match_id,1) or not integer(q.round,1)
            or not integer(q.attacker_life,1) or not integer(q.victim_life,1)
            or type(fresh_s)~="number" or fresh_s~=fresh_s or fresh_s<=0 or fresh_s==math.huge
            or type(ipc)~="table" or type(ipc.N)~="table" or type(ipc.N.ipc_info)~="function"
            or type(ipc.rec)~="function" or type(ipc.get)~="function" then return end
        -- refresh_info may retain a healthy cache after a failed native read.
        if not header(ipc.N.ipc_info(),fresh_s) then return end
        local link,lv=ipc.rec("link")
        local sess,sv=ipc.rec("session")
        local mode,mv=ipc.rec("mode")
        local enums=ipc.S and ipc.S.ENUMS
        local connected=enums and enums.sidecar_status and enums.sidecar_status.CONNECTED
        if connected==nil or type(link)~="table" or link.status~=connected or link.my_peer_id~=q.attacker
            or type(sess)~="table" or not integer(sess.seq,1) or sess.match_id~=q.match_id or sess.round~=q.round
            or (sess.phase~=3 and sess.phase~=4)
            or type(mode)~="table" or not integer(mode.seq,1) or mode.match_id~=q.match_id or mode.round~=q.round then return end
        local sa,sb=pair(sess.rows,q.attacker,q.target)
        local ma,mb=pair(mode.rows,q.attacker,q.target)
        if not sa or not sb or sa.connected~=true or sb.connected~=true or not ma or not mb
            or ma.life~=q.attacker_life or mb.life~=q.victim_life
            or type(ma.alive)~="boolean" or type(mb.alive)~="boolean" then return end
        -- Mode is change-driven: freshness is the native slot version and
        -- attached heartbeat, never an invented maximum Mode record age.
        for _,v in ipairs({lv,sv,mv})do if type(v)~="number" or not math.tointeger(v)then return end end
        if lv==nil or sv==nil or mv==nil or ipc.get("link",lv)~=lv
            or ipc.get("session",sv)~=sv or ipc.get("mode",mv)~=mv
            or not header(ipc.N.ipc_info(),fresh_s) then return end
        if ma.alive==false then return true,"attacker_down" end
        if mb.alive==false then return true,"target_down" end
        return false,"participating"
    end)
    if not ok or down==nil then return nil,"unavailable" end
    return down,reason
end
return M
