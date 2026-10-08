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
    return env.commit()
end
return M
