-- Independent, bounded observation of existing native object-11 armor traces.
-- No trace/damage call, physics/property write, or retained native reference.
local M={BURST_MS=15000,MAX_RECORDS=48,MAX_SCOPE_RECORDS=16,INTERVAL_MS=250,MAX_DETAIL_BYTES=16384}
local fields={"world","peer","match_id","round","life","pawn","actor","mesh","side"}
local function safe(f)local ok,v=pcall(f);if ok then return v end end
local function number(v)return type(v)=="number" and v==v and math.abs(v)<math.huge and v or nil end
local function integer(v)return number(v) and math.tointeger(v) and v or nil end
local function context(c)
    if type(c)~="table" then return nil end
    local out={}
    for _,k in ipairs(fields)do
        local v=c[k]
        if k=="world" or k=="pawn" or k=="side" then
            if type(v)~="string" or v=="" or #v>1024 or v:find("\0",1,true)then return nil end
        elseif not integer(v) or v<=0 then return nil end
        out[k]=v
    end
    if out.side~="owner" and out.side~="source" then return nil end
    if out.peer>65535 or out.life>65535 or out.round>0xffffffff then return nil end
    return out
end
local function same(a,b)
    if not a or not b then return false end
    for _,k in ipairs(fields)do if a[k]~=b[k]then return false end end
    return true
end
local function key(c)local values={};for _,k in ipairs(fields)do values[#values+1]=tostring(c[k])end;return table.concat(values,"\0")end
local function text(v)
    if type(v)=="string" then return v:gsub("[%s%z]","_")end
    if type(v)=="boolean" then return v and "true" or "false" end
    if number(v)then return string.format("%.17g",v)end
    return "unavailable"
end
function M.new(o)
    local started,deadline,emitted,seq,scopes=nil,nil,0,0,{}
    local api={}
    function api.start()
        local now=number(safe(o.clock));if not now then return false end
        started,deadline,emitted,scopes=now,now+M.BURST_MS,0,{}
        return true
    end
    function api.stop()started,deadline,scopes=nil,nil,{}end
    function api.capture(returnedp,worldp,startp,endp,radiusp,objectsp,complexp,hitsp,ignorep)
        -- This is on a hot engine hook. An inactive/consumed burst performs no
        -- enabled/WG/context/native-parameter reads, including after timeout.
        if not deadline or emitted>=M.MAX_RECORDS then return end
        local now=number(safe(o.clock))
        if not now or now<started or now>=deadline then api.stop();return end
        if safe(o.enabled)~=true then return end
        local w=o.unwrap(worldp)
        local c=context(safe(function()return o.context(w)end));if not c then return end
        local scope=key(c);local budget=scopes[scope]
        if budget and (budget.count>=M.MAX_SCOPE_RECORDS or now-budget.last<M.INTERVAL_MS)then return end
        local radius=o.unwrap(radiusp)
        if not number(radius) or radius<0.1 or radius>15
            or o.unwrap(complexp)~=true or o.unwrap(ignorep)~=false then return end
        local types=o.unwrap(objectsp)
        if safe(function()return types:GetArrayNum()end)~=1 then return end
        local count,matching=0,true
        local ok=pcall(function()types:ForEach(function(i,v)
            count=count+1
            if count>1 or i~=1 or o.unwrap(v)~=11 then matching=false;return true end
        end)end)
        if not ok or not matching or count~=1 or safe(function()return types:GetArrayNum()end)~=1 then return end
        -- Spend bounded observation capacity even if a read fails. A failed
        -- read is evidence unavailable, never grounds for unlimited retries.
        emitted=emitted+1;seq=seq+1
        scopes[scope]={count=(budget and budget.count or 0)+1,last=now}
        local detail=safe(function()return o.format_trace(startp,endp,radiusp,objectsp,complexp,hitsp,ignorep,returnedp)end)
        local available=type(detail)=="string" and detail~=""
        local truncated=available and #detail>M.MAX_DETAIL_BYTES
        if truncated then detail="unavailable:detail_byte_limit";available=false
        elseif not available then detail="unavailable:native_trace_read"end
        if safe(o.enabled)~=true or not same(c,context(safe(function()return o.context(w)end)))then return end
        local invocation=safe(function()return o.invocation and o.invocation(w,c)end)
        local known=c.side=="owner" and type(invocation)=="table"
            and invocation.pawn==c.actor and same(c,context(invocation.context))
            and integer(invocation.attacker) and invocation.attacker>0
            and integer(invocation.hit_id) and invocation.hit_id>0
        -- Context can change while resolving the optional active replay span.
        if not same(c,context(safe(function()return o.context(w)end)))then return end
        local rows={"LAB_ARMOR_TRACE","evidence_only=true","purpose=object11_complex_proxy_trace",
            "dcd_caller=unavailable","source_parent=unavailable","authority=false","seq="..seq}
        for _,k in ipairs(fields)do rows[#rows+1]=k.."="..text(c[k])end
        rows[#rows+1]="replay_span="..(known and "known" or "unavailable")
        for _,k in ipairs({"attacker","hit_id","cid","parent_cid"})do
            local v=known and invocation[k] or nil
            rows[#rows+1]=k.."="..text(integer(v)and v>=0 and v or nil)
        end
        rows[#rows+1]="trace_text_available="..text(available)
        rows[#rows+1]="detail_truncated="..text(truncated==true)
        rows[#rows+1]="detail="..detail
        o.log("%s",table.concat(rows," "))
    end
    return api
end
return M
