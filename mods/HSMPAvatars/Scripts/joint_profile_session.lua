-- Diagnostic-only Session/Mode snapshot. Heartbeat versions remain metadata;
-- every other raw field, including future fields, is part of the comparison.
local M={MAX_DEPTH=8,MAX_KEYS=128,MAX_ENTRIES=8192,MAX_TEXT=32768,MAX_STRING=4096}
local OMIT={seq=true,server_time_ms=true}
local function finite(v)return type(v)=="number"and v==v and math.abs(v)<math.huge end
local function scalar(v,budget)
    local t=type(v)
    assert(t=="nil"or t=="number"or t=="boolean"or t=="string","nonplain value")
    if t=="number"then assert(finite(v),"nonfinite number")end
    if t=="string"then
        assert(#v<=M.MAX_STRING,"string capacity")
        budget.text=budget.text+#v;assert(budget.text<=(budget.text_limit or M.MAX_TEXT),"text capacity")
    end
    return v
end
local function key(k,budget)
    if type(k)=="string"then assert(#k<=128,"key capacity");return scalar(k,budget)end
    assert(type(k)=="number"and math.tointeger(k)and k>=1 and k<=M.MAX_KEYS,"nonplain key")
    return k
end
local function copy(v,budget,depth,root)
    if type(v)~="table"then return scalar(v,budget)end
    assert(depth<=M.MAX_DEPTH and getmetatable(v)==nil,"table depth or metatable")
    assert(not budget.seen[v],"duplicate or cyclic table");budget.seen[v]=true
    local out,n={},0
    for k,x in next,v do
        n=n+1;budget.entries=budget.entries+1
        assert(n<=M.MAX_KEYS and budget.entries<=M.MAX_ENTRIES,"entry capacity")
        key(k,budget)
        if not(root and OMIT[k])then out[k]=copy(x,budget,depth+1,false)end
    end
    return out
end
local function metadata(v)
    if v==nil then return {available=false}end
    local out={available=true}
    for _,k in ipairs({"seq","server_time_ms"})do
        local x=rawget(v,k)
        assert(x==nil or finite(x)and math.tointeger(x)and x>=0,"heartbeat metadata unavailable")
        out[k.."_available"]=x~=nil;out[k]=x
    end
    return out
end
function M.capture(session,mode)
    local ok,value,meta=pcall(function()
        assert(type(session)=="table"and (mode==nil or type(mode)=="table"),"raw records unavailable")
        local budget={entries=0,text=0,seen={}}
        local out={session=copy(session,budget,1,true),mode_present=mode~=nil}
        if mode~=nil then out.mode=copy(mode,budget,1,true)end
        return out,{session=metadata(session),mode=metadata(mode)}
    end)
    if not ok then return nil,type(value)=="string"and value:sub(1,120)or "snapshot unavailable"end
    return value,meta
end
local function path(parent,k)return (parent.."."..tostring(k)):sub(1,160)end
function M.same(a,b)
    if type(a)~="table"or type(b)~="table"then return false,"raw.snapshot_unavailable"end
    local budget,seen_a,seen_b=0,{},{}
    local text_budget={text=0,text_limit=M.MAX_TEXT*2}
    local function equal(x,y,where,depth)
        if type(x)~=type(y)then return false,where,x,y end
        if type(x)~="table"then
            assert(x==nil or type(x)=="boolean"or type(x)=="string"or finite(x),"nonplain comparison")
            scalar(x,text_budget);scalar(y,text_budget)
            if x~=y then return false,where,x,y end
            return true
        end
        assert(depth<=M.MAX_DEPTH+1 and getmetatable(x)==nil and getmetatable(y)==nil,"comparison depth or metatable")
        assert(not seen_a[x]and not seen_b[y],"duplicate or cyclic comparison")
        seen_a[x],seen_b[y]=true,true
        local n=0
        for k,v in next,x do
            n=n+1;budget=budget+1;assert(n<=M.MAX_KEYS and budget<=M.MAX_ENTRIES,"comparison capacity")
            key(k,text_budget)
            local ok,field,expected,observed=equal(v,rawget(y,k),path(where,k),depth+1)
            if not ok then return false,field,expected,observed end
        end
        n=0
        for k,v in next,y do
            n=n+1;assert(n<=M.MAX_KEYS,"comparison capacity")
            key(k,text_budget)
            if rawget(x,k)==nil then return false,path(where,k),nil,v end
        end
        return true
    end
    local ok,same,field,expected,observed=pcall(equal,a,b,"raw",1)
    if not ok then return false,"raw.snapshot_unavailable"end
    return same,field,expected,observed
end
return M
