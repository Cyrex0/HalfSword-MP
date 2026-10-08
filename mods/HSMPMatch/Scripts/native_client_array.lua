-- Pinned LuaUObject.cpp: GetNonTrivialLocal arrays are dense Lua tables;
-- property Get arrays are TArray userdata with ForEach. Both yield SDK-known
-- GetParam entries. Unwrap only the named hard parameter wrapper types.
local M={}
local function unwrap(value)
    local t=type(value)
    if t~="table"and t~="userdata"then return value end
    local kind=t=="table"and rawget(value,"type")or value.type
    if type(kind)=="function"then
        local name=kind(value)
        if name=="RemoteUnrealParam"or name=="LocalUnrealParam"then return value:get()end
    end
    return value
end
function M.each(array,fn,maximum)
    maximum=maximum or 512
    if array==nil then error("native array unavailable",0)end
    if type(array)=="table"and rawget(array,"ForEach")==nil then
        if getmetatable(array)~=nil then error("native return array metatable",0)end
        local count=0
        for k in pairs(array)do
            if type(k)~="number"or not math.tointeger(k)or k<1 then error("native return array key",0)end
            count=count+1;if count>maximum then error("native return array bound",0)end
        end
        for i=1,count do local value=rawget(array,i);if value==nil then error("native return array sparse",0)end;fn(unwrap(value))end
        return
    end
    local each=array.ForEach
    if type(each)~="function"then error("native property array iterator unavailable",0)end
    local count=0
    each(array,function(_,value)
        count=count+1;if count>maximum then error("native property array bound",0)end
        fn(unwrap(value))
    end)
end
return M
