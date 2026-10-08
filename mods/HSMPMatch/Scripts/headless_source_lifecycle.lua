-- Static source recipes are captured outside the bone sampler and remain tied
-- to their original native binding, entity reference and directory generation.
local M = {}
local fields = { "index", "world_key", "pc_address", "pc_name", "pawn_address", "pawn_name" }
local function same_binding(a, b)
    if not a or not b then return false end
    for _, field in ipairs(fields) do if a[field] == nil or a[field] ~= b[field] then return false end end
    return true
end
function M.new(env)
    local cached, revisions, retry_at = {}, {}, {}
    local api = {}
    local function key(row) return tostring(row.epoch) .. ":" .. tostring(row.id) .. ":" .. tostring(row.incarnation) end
    local function valid(token, original, index)
        if not env.same(token) then return false end
        local fresh = env.resolve(index)
        return env.same(token) and same_binding(original, fresh)
    end
    function api.ensure(directory, token, frame_seq)
        for _, row in ipairs(directory.entities or {}) do
            if not env.same(token) then env.invalidate(); return false, "world changed before source descriptor" end
            local reference, index = key(row), env.index(row)
            local binding = env.resolve(index)
            if not binding or not env.same(token) then return false, "source descriptor binding unavailable" end
            local old = cached[reference]
            if old and not same_binding(old.binding, binding) then return false, "canonical incarnation changed" end
            if not old or old.dir_seq ~= directory.seq then
                if env.now_ms() < (retry_at[reference] or -math.huge) then return false, "source descriptor retry pending" end
                retry_at[reference] = env.now_ms() + 1000
                local recipe, bindings = env.capture(index)
                if not env.same(token) then env.invalidate(); return false, "world changed during source descriptor" end
                if not valid(token, binding, index) then return false, "canonical incarnation changed" end
                if not recipe then return false, "source descriptor " .. reference .. ": " .. tostring(bindings) end
                local revision = (revisions[reference] or 0) + 1
                local accepted, reason = env.describe({epoch=row.epoch,id=row.id,incarnation=row.incarnation,
                    slot=row.slot,dir_seq=directory.seq,revision=revision,frame_seq=frame_seq},recipe,bindings)
                if not env.same(token) then env.invalidate(); return false, "world changed while registering source descriptor" end
                if not valid(token, binding, index) then return false, "canonical incarnation changed" end
                if accepted ~= true then return false, "source descriptor registration: " .. tostring(reason) end
                local identity = {}
                for _, field in ipairs(fields) do identity[field] = binding[field] end
                cached[reference], revisions[reference], retry_at[reference] = {binding=identity,dir_seq=directory.seq}, revision, nil
            end
        end
        return true
    end
    function api.refresh()
        -- Keep the original scalar binding and revision. A replacement actor
        -- cannot turn a recipe refresh into reuse of the old entity reference.
        for reference, value in pairs(cached) do value.dir_seq = nil; retry_at[reference] = env.now_ms() + 1000 end
    end
    function api.drop() cached, revisions, retry_at = {}, {}, {} end
    return api
end
return M
