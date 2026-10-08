-- The native sampler stages a DTO; publication waits for the original world
-- token to survive the complete reflected call. Invalidation is retried after
-- the native API lock is released, without another old-object access.
local M = {}
function M.sample(env, token, args)
    local sampled, reason = env.sample(args)
    if not env.same(token) then
        env.invalidate()
        return false, "world changed during canonical sample"
    end
    if sampled ~= true then return false, reason end
    if env.render then
        local rendered, why = env.render()
        if not env.same(token) then
            env.invalidate()
            return false, "world changed during native render sample"
        end
        if rendered ~= true then
            if env.refresh then env.refresh() end
            return false, "native render sample: " .. tostring(why)
        end
    elseif not env.allow_core_only then
        return false, "native render sampler unavailable"
    end
    return env.commit()
end
return M
