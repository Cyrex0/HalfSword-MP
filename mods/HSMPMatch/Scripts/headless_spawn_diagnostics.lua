-- Startup evidence only. Readers return scalars; a world loss stops the next
-- reader before it can reuse a controller, pawn or component from the old world.
local M = {}
function M.capture(env, token, reason)
    local result = { reason = reason or "", world = env.world_key() }
    for _, name in ipairs({ "profile", "controllers", "mode", "level", "willies" }) do
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
