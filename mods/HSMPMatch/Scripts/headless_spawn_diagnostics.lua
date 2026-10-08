-- Startup evidence only. Readers return scalars; a world loss stops the next
-- reader before it can reuse a controller, pawn or component from the old world.
local M = {}
function M.property(value)
    if type(value) == "boolean" or type(value) == "number" then return value end
    return "unavailable"
end
-- SDK-proven TMap<TEnumAsByte<...>, bool>; absent Map_Find keys have a
-- default false bool out-value in the native Spawn Combatants predicate.
function M.map_flag(map, wanted, valid)
    if not valid() then error("native spawner world changed",0) end
    local count, found, seen = #map, false, {}
    if count < 0 or count > 64 then error("native spawner map bound",0) end
    local visited = 0
    map:ForEach(function(key,value)
        if not valid() then error("native spawner world changed",0) end
        local k = type(key)=="number" and key or key:get()
        if not valid() then error("native spawner world changed",0) end
        local v
        if type(value)=="boolean" then v=value else v=value:get() end
        if not valid() then error("native spawner world changed",0) end
        if type(k)~="number" or k%1~=0 or k<0 or k>255 or seen[k] or type(v)~="boolean" then error("native spawner map type",0) end
        seen[k]=true;visited=visited+1
        if visited>64 then error("native spawner map bound",0) end
        if k==wanted then found=v end
        if not valid() then error("native spawner world changed",0) end
    end)
    if not valid() or visited~=count or #map~=count then error("native spawner map changed",0) end
    return found
end
function M.compatible(game,combat,play,combatants,required)
    if type(game)~="boolean" or type(combat)~="boolean" or type(play)~="boolean"
        or type(combatants)~="number" or combatants%1~=0 or type(required)~="number" or required%1~=0 then return nil end
    return game and combat and play and combatants>=required
end
function M.capture(env, token, reason)
    local result = { reason = reason or "", world = env.world_key() }
    local names = { "profile", "controllers", "mode", "level", "willies" }
    if env.spawners then names[#names+1]="spawners" end
    for _, name in ipairs(names) do
        if not env.same(token) then return nil, "world changed before " .. name end
        local ok, value, why = pcall(env[name], token)
        if not env.same(token) then return nil, "world changed during " .. name end
        if not ok then return nil, name .. " reader refused: " .. tostring(value) end
        if value == nil then return nil, why or (name .. " unavailable") end
        result[name] = value
    end
    return result
end
function M.format(value)
    local parts = {}
    local function walk(v, key)
        if type(v) == "table" then
            local keys = {}
            for k in pairs(v) do keys[#keys + 1] = k end
            table.sort(keys, function(a, b) return tostring(a) < tostring(b) end)
            for _, k in ipairs(keys) do walk(v[k], key .. "[" .. tostring(k) .. "]") end
        elseif type(v) == "string" or type(v) == "number" or type(v) == "boolean" then
            parts[#parts + 1] = key .. "=" .. tostring(v)
        else error("non-scalar native spawn diagnostic") end
    end
    walk(value, "native")
    return table.concat(parts, "; ")
end
return M
