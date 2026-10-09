-- Rare, read-only source harvest. Native wrappers never leave this call. The
-- adapter qualifies the original world/pawn before and after every access;
-- frequent full render pose/material capture belongs to the native batch API.
local source=(debug.getinfo(1,"S").source or ""):gsub("^@","")
local directory=source:match("^(.*)[/\\]") or "."
local Fields=dofile(directory.."/native_source_fields.lua")
-- The lossless encoded recipe allows 512KiB; each copied value/table requires
-- at least one codec token byte. Keys retain their existing separate semantics.
local M={SCHEMA=6,FIELDS=Fields,MAX_RECIPE_NODES=512*1024}
local equipment_fields={armor="ArmorinSlots_5_BD7AC6CB43FBB2FDB943E7864486F358",
    sheaths="WeaponsinSlots_11_B42349384F5EF74DE78A7F870D89656A",
    hands="WeaponinHands_23_B3FE643741AF91A6DFE51888205C0F05"}
local function fail(reason)error(reason,0)end
local function finite(v)return type(v)=="number" and v==v and math.abs(v)~=math.huge end
local function text(v,max,empty)
    return type(v)=="string" and (empty or #v>0) and #v<=max and not v:find("\0",1,true)
end
local function guard(env)
    if type(env.guard)~="function" or env.guard()~=true then fail("source scope changed")end
end
local function access(env,fn)
    guard(env);local value=fn();guard(env);return value
end
local function unwrap(env,v)
    -- Only callers with known hard native inner types use this adapter.
    return access(env,function()if env.unwrap then return env.unwrap(v)end;return v end)
end
local read_struct
local function map(env,getter,max,convert)
    local native=access(env,getter)
    if native==nil then fail("source map unavailable")end
    local count=access(env,function()return env.map_count and env.map_count(native) or #native end)
    if not math.tointeger(count) or count<0 or count>max then fail("source map bound")end
    local rows,seen,visited={},{},0
    access(env,function()native:ForEach(function(k,v)
        guard(env);visited=visited+1
        if visited>max then fail("source map bound")end
        local key=unwrap(env,k)
        if type(key)~="number" or not math.tointeger(key) or key<0 or key>=max or seen[key] then fail("source map key")end
        seen[key]=true
        local value=unwrap(env,v)
        rows[#rows+1]=convert(key,value)
        guard(env)
    end)end)
    local after=access(env,function()return env.map_count and env.map_count(native) or #native end)
    if after~=count or visited~=count then fail("source map changed or incomplete")end
    table.sort(rows,function(a,b)return a.slot<b.slot end)
    return rows
end
local function vector(env,getter,components)
    local out={}
    for i,name in ipairs(components)do
        local n=access(env,function()local v=getter();if not v then fail("source vector unavailable")end;return v[name]end)
        if not finite(n)then fail("source vector value")end
        out[i]=n
    end
    return out
end
local function equipment(env,getter)
    local out={}
    for _,kind in ipairs({"armor","sheaths","hands"})do
        local maximum=kind=="armor" and 17 or kind=="sheaths" and 6 or 2
        out[kind]=map(env,function()local e=getter();if not e then fail("source equipment unavailable")end;return e[equipment_fields[kind]]end,maximum,
            function(slot,value)return {slot=slot,passport=read_struct(kind=="armor" and "armor" or "weapon",function()return value end,env)}end)
    end
    return out
end
read_struct=function(kind,getter,env)
    local fields=Fields[kind];if not fields then fail("source passport type")end
    local out={}
    for _,fd in ipairs(fields)do
        local function value()local s=getter();if not s then fail("source passport unavailable")end;return s[fd[1]]end
        local t,v=fd[2]
        if t=="vec" or t=="color" then v=vector(env,value,t=="vec" and {"X","Y","Z"} or {"R","G","B","A"})
        elseif t=="equipment" then v=equipment(env,value)
        elseif t=="flags" then v=map(env,value,17,function(slot,b)
            if type(b)~="boolean"then fail("source blocked-slot value")end
            return {slot=slot,value=b}
        end)
        elseif t=="class"then
            if type(env.class_path)~="function"then fail("source class adapter unavailable")end
            v=access(env,function()return env.class_path(value())end)
            if not text(v,512,true)then fail("source class unavailable")end
        elseif t=="name"then
            v=access(env,function()local n=value();return env.name and env.name(n) or n:ToString()end)
            if not text(v,128,true)then fail("source name unavailable")end
        else
            v=access(env,value)
            if t=="bool"then if type(v)~="boolean"then fail("source boolean unavailable")end
            elseif t=="int" or t=="byte"then
                if not finite(v) or not math.tointeger(v) or (t=="int" and (v< -0x80000000 or v>0x7fffffff))
                    or (t=="byte" and (v<0 or v>255))then fail("source integer unavailable")end
            elseif t=="num"then if not finite(v)then fail("source number unavailable")end
            else fail("source field type")end
        end
        out[fd[3]]=v
    end
    return out
end
function M.read_passport(kind,getter,env)
    local ok,value=pcall(read_struct,kind,getter,env)
    if not ok then return nil,value end
    return value
end
function M.read_armor_map(getter,env)
    local ok,value=pcall(function()return map(env,getter,17,function(slot,p)
        return {slot=slot,passport=read_struct("armor",function()return p end,env)}end)end)
    if not ok then return nil,value end
    return value
end
function M.read_flags(getter,maximum,env)
    local ok,value=pcall(function()return map(env,getter,maximum,function(slot,b)
        if type(b)~="boolean"then fail("source flag value")end;return {slot=slot,value=b}end)end)
    if not ok then return nil,value end;return value
end
function M.read_names(getter,maximum,env)
    local ok,value=pcall(function()
        local native=access(env,getter);if native==nil then fail("source names unavailable")end
        local count=access(env,function()return native:GetArrayNum()end)
        if type(count)~="number" or not math.tointeger(count) or count<0 or count>maximum then fail("source names bound")end
        local out={};access(env,function()native:ForEach(function(_,v)
            if #out>=maximum then fail("source names bound")end
            local n=unwrap(env,v);n=access(env,function()return env.name and env.name(n) or n:ToString()end)
            if not text(n,128,false)then fail("source names value")end;out[#out+1]=n
        end)end)
        if #out~=count or access(env,function()return native:GetArrayNum()end)~=count then fail("source names changed")end
        return out
    end)
    if not ok then return nil,value end;return value
end
-- Clone only plain scalars/tables. No userdata, closures or engine pointers
-- reach descriptor publication. A bounded path rejects cycles and oversized data.
local function plain(v,depth,budget)
    budget.n=budget.n+1;if budget.n>M.MAX_RECIPE_NODES or depth>24 then fail("source table bound")end
    local t=type(v)
    if t=="boolean"then return v end
    if t=="number"then if not finite(v)then fail("source scalar unavailable")end;return v end
    if t=="string"then if not text(v,512,true)then fail("source text bound")end;return v end
    if t~="table" or getmetatable(v)~=nil then fail("source copied value unavailable")end
    local out={}
    for k,x in pairs(v)do
        if type(k)~="string" and (type(k)~="number" or not math.tointeger(k) or k<1)then fail("source table key")end
        out[k]=plain(x,depth+1,budget)
    end
    return out
end
function M.signature(value)
    local function pack(v)
        local t=type(v)
        if t=="number"then return "n"..string.pack("<d",v)end
        if t=="string"then return "s"..string.pack("<I4",#v)..v end
        if t=="boolean"then return v and "t" or "f"end
        if t~="table"then fail("source signature type")end
        local keys={};for k in pairs(v)do keys[#keys+1]=k end
        table.sort(keys,function(a,b)if type(a)==type(b)then return a<b end;return type(a)<type(b)end)
        local out={"["};for _,k in ipairs(keys)do out[#out+1]=pack(k);out[#out+1]=pack(v[k])end
        out[#out+1]="]";return table.concat(out)
    end
    return pack(plain(value,0,{n=0}))
end
local function phase(env,stage,edge,detail)
    if env.phase then detail=detail or {};detail.pass=env.pass;env.phase(stage,edge,detail)end
end
local function harvest(env)
    phase(env,"harvest","enter")
    phase(env,"passport","enter",{getter="Willie.Character Passport"})
    local passport=read_struct("character",env.character,env)
    local actor_class=access(env,env.actor_class)
    if not text(actor_class,512,false)then fail("source actor class unavailable")end
    local team=access(env,env.team)
    if not finite(team) or not math.tointeger(team) or team< -0x80000000 or team>0x7fffffff then fail("native team unavailable")end
    phase(env,"passport","exit",{ok=true})
    phase(env,"equipment","enter",{getter="Willie.Currently Equipped Armor/Weapon Passport"})
    local armor,why=M.read_armor_map(env.current_armor,env);if not armor then fail(why)end
    local construction=plain(access(env,env.construction),0,{n=0})
    local gear=access(env,env.live_weapons)
    if not gear then fail("source live weapons unavailable")end
    gear=plain(gear,0,{n=0});gear.armor=armor
    phase(env,"equipment","exit",{ok=true,count=#gear.weapons})
    phase(env,"render","enter",{getter="SourceRender.capture"})
    local render=plain(access(env,env.render),0,{n=0})
    if type(render.components)~="table" or type(render.topology)~="table"then fail("source render capture incomplete")end
    phase(env,"render","exit",{ok=true,count=#render.components})
    for _,w in ipairs(gear.weapons)do
        w.components={}
        for _,c in ipairs(render.components)do if c.owner==w.id then w.components[#w.components+1]=c.id end end
        if #w.components==0 then fail("source weapon render components incomplete")end
    end
    guard(env)
    phase(env,"harvest","exit",{ok=true,count=#render.components})
    return {schema=M.SCHEMA,actor_class=actor_class,team=team,passport=passport,construction=construction,
        equipment=gear,components=render.components,topology=render.topology}
end
function M.capture(env)
    if type(env)~="table"then return nil,"source adapter unavailable"end
    env.pass=1
    local ok,first=pcall(harvest,env);if not ok then phase(env,"harvest","exit",{ok=false,reason=tostring(first)});return nil,first end
    env.pass=2
    local again,second=pcall(harvest,env);if not again then phase(env,"harvest","exit",{ok=false,reason=tostring(second)});return nil,second end
    phase(env,"signature","enter",{getter="SourceDescriptor.signature"})
    local compared,a,b=pcall(function()return M.signature(first),M.signature(second)end)
    phase(env,"signature","exit",{ok=compared and a==b})
    if not compared or a~=b then return nil,"source recipe changed during capture"end
    local same=pcall(guard,env);if not same then return nil,"source scope changed"end
    return first
end
return M
