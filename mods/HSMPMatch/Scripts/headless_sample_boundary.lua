-- The native sampler stages a DTO; publication waits for the original world
-- token to survive the complete reflected call. Invalidation is retried after
-- the native API lock is released, without another old-object access.
local M = {}
function M.sample(env, token, args)
    local context={epoch=args.epoch,dir_seq=args.dir_seq,frame_seq=args.frame_seq}
    local function phase(stage,edge,detail)
        if env.phase then env.phase(context,stage,edge,detail) end
    end
    phase("canonical_core","enter")
    local sampled, reason = env.sample(args)
    local same = env.same(token)
    phase("canonical_core","exit",{ok=sampled==true and same,reason=not same and "world changed" or tostring(reason or "")})
    if not same then
        env.invalidate()
        return false, "world changed during canonical sample"
    end
    if sampled ~= true then return false, reason end
    if env.render then
        phase("native_render","enter")
        local rendered, why = env.render()
        same = env.same(token)
        phase("native_render","exit",{ok=rendered==true and same,reason=not same and "world changed" or tostring(why or "")})
        if not same then
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
    phase("canonical_publish","enter")
    local committed, why = env.commit()
    phase("canonical_publish","exit",{ok=committed==true,reason=tostring(why or "")})
    return committed, why
end
return M
