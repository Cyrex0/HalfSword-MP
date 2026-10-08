-- Reorder only one already-drained batch. No waiting, native writes or replay privilege.
local M={MAX_EVENTS=1024}
local UINT32,HALF=4294967296,2147483648
local function integer(v,low,high)
    return type(v)=="number"and math.tointeger(v)and v>=low and (not high or v<=high)
end
function M.order(rows)
    if type(rows)~="table"or #rows>M.MAX_EVENTS then return rows,0 end
    local count=0
    for k in pairs(rows)do
        count=count+1;if not integer(k,1,#rows)then return rows,0 end
    end
    if count~=#rows then return rows,0 end
    local groups={}
    for i,e in ipairs(rows)do
        local d=type(e)=="table"and e.data
        if type(d)~="table"or not integer(e.peer,1,65535)or not integer(d.match_id,1)
            or not integer(d.round,1,UINT32-1)or not integer(d.attacker_life,1,UINT32-1)
            or not integer(d.victim_life,1,UINT32-1)or not integer(d.hit_id,1,UINT32-1)
            or not integer(d.attacker_ts,0,UINT32-1)then return rows,0 end
        local key=table.concat({d.match_id,d.round,e.peer,d.attacker_life,d.victim_life},":")
        local g=groups[key]
        if not g then g={base=d.hit_id,slots={},items={},min=0,max=0};groups[key]=g end
        local delta=(d.hit_id-g.base)%UINT32
        if delta>=HALF then delta=delta-UINT32 end
        g.min=math.min(g.min,delta);g.max=math.max(g.max,delta)
        g.slots[#g.slots+1]=i;g.items[#g.items+1]={event=e,delta=delta,arrival=i}
    end
    local out,changed={},0
    for i,e in ipairs(rows)do out[i]=e end
    for _,g in pairs(groups)do
        if #g.items>1 and g.max-g.min<HALF then
            table.sort(g.items,function(a,b)return a.delta<b.delta or a.delta==b.delta and a.arrival<b.arrival end)
            local monotonic=true
            for i,item in ipairs(g.items)do
                local ts=item.event.data.attacker_ts
                if ts==0 or i>1 and ts<g.items[i-1].event.data.attacker_ts then monotonic=false;break end
            end
            if monotonic then
                for i,item in ipairs(g.items)do
                    local slot=g.slots[i]
                    if rows[slot]~=item.event then changed=changed+1 end
                    out[slot]=item.event
                end
            end
        end
    end
    return changed>0 and out or rows,changed
end
return M
