-- Native execution deduplication belongs in the game, beside the BP call.
-- A lost result or IPC redelivery must never cause another native application.
local M={}
function M.new(e)
    local entries,order,streams={}, {}, {}
    local stream_count=0
    local cap=e.cap or 4096
    local function key(r) return tostring(r.match_id)..":"..r.round..":"..r.attacker..":"..r.hit_id..":"..r.victim_life end
    local function result(d,a,status)
        return {match_id=d.match_id or 0,round=d.round or 0,attacker=a,hit_id=d.hit_id or 0,
            victim_life=d.victim_life or 0,status=status,observed_fields=0,health_delta=0}
    end
    local out={}
    function out.run(d,a,callback)
        local seed=result(d,a,6)
        local k=key(seed)
        local old=entries[k]
        if old then old.sent=nil;return old.text,old.r,false end
        local stream=tostring(seed.match_id)..":"..seed.round..":"..a..":"..seed.victim_life
        local id=math.tointeger(seed.hit_id) or 0
        local last=streams[stream]
        local forward=last==nil or ((id-last)%4294967296>0 and (id-last)%4294967296<2147483648)
        if id==0 or not forward then
            seed.status=7
            e.send(seed)
            return "outcome expired: native execution not repeated",seed,false
        end
        -- Advance before the call, including uncertain failures. Evicted ids
        -- remain below the watermark and can never execute again.
        if last==nil then
            if stream_count>=4096 then seed.status=7;e.send(seed);return "native attempt cache full",seed,false end
            stream_count=stream_count+1
        end
        streams[stream]=id
        if #order>=cap then local expired=table.remove(order,1);entries[expired]=nil end
        local row={r=seed,text="native attempt uncertain",at=e.now()}
        entries[k]=row;order[#order+1]=k
        local ok,text,status,fields,hp=pcall(callback)
        if ok then
            row.text=text;seed.status=status or 6;seed.observed_fields=fields or 0;seed.health_delta=hp or 0
        else row.text="native attempt uncertain: "..tostring(text) end
        e.send(seed);row.sent=e.now()
        return row.text,seed,true
    end
    function out.ack(r)
        local row=entries[key(r)]
        if row and row.r.status==r.status and row.r.observed_fields==r.observed_fields and row.r.health_delta==r.health_delta then
            row.acked=true;return true
        end
        return false
    end
    function out.tick()
        local now=e.now();local n=0
        for _,k in ipairs(order) do
            local row=entries[k]
            if row and not row.acked and now-row.at<=10 and (not row.sent or now-row.sent>=0.1) then
                e.send(row.r);row.sent=now;n=n+1;if n>=64 then break end
            end
        end
    end
    function out.clear() entries,order,streams={}, {}, {};stream_count=0 end
    return out
end
return M
