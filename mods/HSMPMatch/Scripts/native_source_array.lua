-- Pinned UE4SS function-return arrays are dense Lua tables; property arrays
-- are TArray userdata. Known hard inners may be typed Remote/Local params.
local M={}
local function fail(reason)error(reason,0)end
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
local function dense(value,max)
    if type(value)~="table" or getmetatable(value)~=nil then fail("native returned array shape unavailable")end
    local n=0
    for key in pairs(value)do
        if type(key)~="number" or not math.tointeger(key) or key<1 or key>max then fail("native returned array key")end
        n=n+1;if n>max then fail("native source array bound")end
    end
    for i=1,n do if rawget(value,i)==nil then fail("native returned array hole")end end
    return n
end
function M.collect(value,max,producer,env,convert)
    if value==nil then fail("native source array unavailable")end
    local out={}
    if producer=="return"then
        local n=checked(env,function()return dense(value,max)end)
        local original={};for i=1,n do original[i]=rawget(value,i)end
        for i=1,n do
            out[i]=checked(env,function()return convert(M.unwrap(original[i],env))end)
            if out[i]==nil then fail("native source array conversion unavailable")end
        end
        if checked(env,function()return dense(value,max)end)~=n then fail("native source array changed")end
        for i=1,n do if not rawequal(original[i],rawget(value,i))then fail("native source array changed")end end
    elseif producer=="property"then
        local n=checked(env,function()return value:GetArrayNum()end)
        if type(n)~="number" or not math.tointeger(n) or n<0 or n>max then fail("native source array bound")end
        checked(env,function()value:ForEach(function(_,v)
            if #out>=max then fail("native source array bound")end
            local copied=checked(env,function()return convert(M.unwrap(v,env))end)
            if copied==nil then fail("native source array conversion unavailable")end
            out[#out+1]=copied
        end)end)
        if #out~=n or checked(env,function()return value:GetArrayNum()end)~=n then fail("native source array changed")end
    else fail("native source array producer unavailable")end
    env.guard();return out
end
return M
