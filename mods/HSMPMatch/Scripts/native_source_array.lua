-- Pinned UE4SS function-return arrays are dense Lua tables; property arrays
-- are TArray userdata. Known hard inners may be typed Remote/Local params.
local M={}
local function fail(reason)error(reason,0)end
local function bounded(value,max)
    local t=type(value)
    local s=t=="string" and string.format("%q",value) or t=="number" and string.format("%.17g",value)
        or t=="boolean" and tostring(value) or "<"..t..">"
    return #s<=max and s or s:sub(1,max).."[truncated]"
end
local function context(env,producer)
    return " producer="..bounded(producer,24).." getter="..bounded(env.context,192)
end
local function checked(env,fn)env.guard();local value=fn();env.guard();return value end
function M.unwrap(value,env)
    local t=type(value)
    if t=="userdata" or (t=="table" and type(rawget(value,"type"))=="function")then
        local kind=checked(env,function()return value:type()end)
        if kind=="RemoteUnrealParam" or kind=="LocalUnrealParam"then
            return checked(env,function()return value:get()end)
        end
    end
    return value
end
local function dense(value,max,env)
    local info=context(env,"return")
    if type(value)~="table" or getmetatable(value)~=nil then fail("native returned array shape unavailable"..info.." value_type="..type(value))end
    local n=0
    for key in pairs(value)do
        if type(key)~="number" or not math.tointeger(key) or key<1 or key>max then
            fail("native returned array key"..info.." key="..bounded(key,96).." key_type="..type(key)
                .." value_type="..type(rawget(value,key)).." max="..tostring(max))
        end
        n=n+1;if n>max then fail("native source array bound"..info.." max="..tostring(max))end
    end
    for i=1,n do if rawget(value,i)==nil then fail("native returned array hole"..info.." index="..tostring(i).." count="..tostring(n))end end
    return n
end
function M.collect(value,max,producer,env,convert)
    local info=context(env,producer)
    if value==nil then fail("native source array unavailable"..info)end
    local out={}
    if producer=="return"then
        local n=checked(env,function()return dense(value,max,env)end)
        local original={};for i=1,n do original[i]=rawget(value,i)end
        for i=1,n do
            out[i]=checked(env,function()return convert(M.unwrap(original[i],env))end)
            if out[i]==nil then fail("native source array conversion unavailable"..info.." index="..tostring(i))end
        end
        if checked(env,function()return dense(value,max,env)end)~=n then fail("native source array changed"..info)end
        for i=1,n do if not rawequal(original[i],rawget(value,i))then fail("native source array changed"..info.." index="..tostring(i))end end
    elseif producer=="property"then
        local n=checked(env,function()return value:GetArrayNum()end)
        if type(n)~="number" or not math.tointeger(n) or n<0 or n>max then fail("native source array bound"..info.." count="..bounded(n,32).." max="..tostring(max))end
        checked(env,function()value:ForEach(function(_,v)
            if #out>=max then fail("native source array bound"..info.." max="..tostring(max))end
            local copied=checked(env,function()return convert(M.unwrap(v,env))end)
            if copied==nil then fail("native source array conversion unavailable"..info.." index="..tostring(#out+1))end
            out[#out+1]=copied
        end)end)
        if #out~=n or checked(env,function()return value:GetArrayNum()end)~=n then fail("native source array changed"..info)end
    else fail("native source array producer unavailable"..info)end
    env.guard();return out
end
return M
