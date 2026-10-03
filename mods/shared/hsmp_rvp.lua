-- hsmp_rvp.lua -- quiesce the Runtime Vertex Paint plugin before a level
-- change (the round-reset crash). Shared: build-and-deploy copies it into each mod's
-- Scripts/; each mod's Lua state gets its own instance (new()).
--
-- Root cause (docs/development/crash-rr.md):
--   The game's blood / wound painting runs on the VertexPaintDetection plugin
--   ("RVP"). Its task queue lives in a GameInstance subsystem
--   (UVertexPaintDetectionGISubSystem.TaskQueue), so it SURVIVES a level
--   change, and every queued or running task holds raw pointers into the
--   world it was started in (FRVPDPTaskFundamentalSettings.TaskWorld,
--   MeshComponent). A task still queued or running when OpenLevel tears the
--   arena down finishes on an RVP worker thread and posts its "task finished"
--   lambda to the game thread, which computes
--   TaskDuration = TaskWorld->TimeSeconds - start on the FREED old UWorld:
--   EXCEPTION_ACCESS_VIOLATION in the exe at +0x4a23c64, ~1 s after the round
--   reload, no Lua frame. The worker reading a freed mesh's vertex data
--   (+0x1e28f4f on "Runtime Vertex Paint and Detection Plugin Thread") is the
--   same bug on the other side. Every MP round reset reloads the arena seconds after a death,
--   while the corpse still bleeds blood tasks into the queue.
--
-- The fix: never change the level while RVP has work.
--   hold(why) -> true while the travel must wait. On the first call it
--     CLOSES the queue (the settings CDO's MaxAmountOfAllowedTasksPerMesh = 0:
--     the plugin refuses every new task) and PURGES the queued ones
--     (RemoveComponentFromPaintTaskQueue / ...DetectTaskQueue per component);
--     then it waits until both queues have been empty for QUIET_S (the task
--     that was running finishes and its game-thread callback runs against the
--     still-live world). Bounded by MAX_HOLD_S: a travel is never blocked.
--   travel_issued() -> call right after OpenLevel; the queue stays closed
--     until reopen() (the new world arrived) or REOPEN_S, whichever is first.
--   purge_now(why) -> for a travel that cannot wait (a native OpenLevel
--     pre-hook): close + purge synchronously.
--   counts() -> { paint, detect, paint_comps, detect_comps } or nil when the
--     plugin is not there.
--
-- UE4SS rules: game thread only (callers are game-thread loops / hooks);
-- the subsystem and the settings CDO are process-lifetime objects (the
-- GameInstance outlives every world); components are only touched while the
-- world they live in is still up (before the OpenLevel), and only those the
-- plugin's own UPROPERTY maps hold (GC nulls destroyed ones). No Soft* :get().

local M = { VERSION = 1 }

M.SUBSYSTEM_CLASS = "VertexPaintDetectionGISubSystem"
M.SETTINGS_CDO = "/Script/VertexPaintDetectionPlugin.Default__VertexPaintDetectionSettings"
M.LIB_CDO = "/Script/VertexPaintDetectionPlugin.Default__VertexPaintFunctionLibrary"
M.LIMIT_PROP = "MaxAmountOfAllowedTasksPerMesh"
M.QUEUES = {   -- TaskQueue property -> counts() field
    { "CalculateColorsPaintQueue", "paint" },
    { "CalculateColorsDetectionQueue", "detect" },
    { "ComponentPaintTaskIDs", "paint_comps" },
    { "ComponentDetectTaskIDs", "detect_comps" },
}
M.QUIET_S = 0.2       -- both queues empty this long before the travel may go
M.MAX_HOLD_S = 2.5    -- never hold a travel longer than this
M.PURGE_EVERY_S = 0.1 -- re-purge while waiting (a task queued before the close)
M.REOPEN_S = 15       -- closed this long without reopen(): reopen anyway
M.ABANDON_S = 1.0     -- a hold nobody polled for this long was abandoned (no travel came)

-- opts:
--   log    function(fmt, ...)
--   ev     function(name, fields)           optional structured event
--   clock  function() -> s                  default os.clock
--   ue     table                            the UE access layer (tests); default: the real one
function M.new(opts)
    opts = opts or {}
    local Log = opts.log or function(fmt, ...) print(string.format(fmt, ...) .. "\n") end
    local ev = opts.ev or function() end
    local clock = opts.clock or os.clock
    local ue = opts.ue or M.real_ue()

    local R = { closed = false, closed_at = nil, saved_limit = nil, h = nil,
                stats = { holds = 0, timeouts = 0, purged = 0, max_wait_s = 0 } }

    function R.counts()
        local c
        pcall(function() c = ue.counts() end)
        return c
    end
    function R.busy()
        local c = R.counts()
        return c ~= nil and (c.paint + c.detect) > 0, c
    end

    function R.close()
        if R.closed then return true end
        local ok, prev = false, nil
        pcall(function() ok, prev = ue.set_limit(0) end)
        if ok then
            R.closed, R.closed_at = true, clock()
            if R.saved_limit == nil then R.saved_limit = prev end
        end
        return ok
    end

    -- Not while a hold is in progress: a world-key change inside the OLD world
    -- (a one-tick PlayerController lookup miss) must not reopen the queue
    -- before the travel was issued. After travel_issued() the caller's world
    -- guard holds until the world has really swapped.
    function R.reopen(why)
        if not R.closed or R.h ~= nil then return false end
        local back = R.saved_limit
        if back == nil or back == 0 then back = ue.default_limit end
        local ok = false
        pcall(function() ok = ue.set_limit(back) end)
        R.closed, R.closed_at = false, nil
        R.saved_limit = nil
        Log("RVP queue reopened (%s): %s = %s%s", tostring(why), M.LIMIT_PROP, tostring(back), ok and "" or " [write FAILED]")
        return ok
    end

    function R.purge()
        local n = 0
        pcall(function() n = ue.purge() or 0 end)
        R.stats.purged = R.stats.purged + n
        return n
    end

    local function cstr(c)
        if not c then return "n/a" end
        return string.format("paint=%d detect=%d (comps %d/%d)", c.paint, c.detect, c.paint_comps or 0, c.detect_comps or 0)
    end
    R.cstr = cstr

    -- true: hold the travel this tick.
    function R.hold(why)
        local now = clock()
        local h = R.h
        if not h then
            local c = R.counts()
            if not c then return false end           -- no RVP plugin: nothing to wait for
            local closed = R.close()
            local purged = R.purge()
            h = { since = now, why = why, quiet_since = nil, last_purge = now, first = c, polled = now }
            R.h = h
            R.stats.holds = R.stats.holds + 1
            Log("RVP quiesce before travel (%s): %s; queue %s, %d component(s) purged",
                tostring(why), cstr(c), closed and "closed" or "NOT closed", purged)
        end
        h.polled = now
        local busy, c = R.busy()
        if c == nil then R.h = nil; return false end
        if busy then
            h.quiet_since = nil
            if now - h.last_purge >= M.PURGE_EVERY_S then h.last_purge = now; R.purge() end
        else
            h.quiet_since = h.quiet_since or now
        end
        local waited = now - h.since
        if not busy and now - h.quiet_since >= M.QUIET_S then
            if waited > R.stats.max_wait_s then R.stats.max_wait_s = waited end
            if waited >= 0.5 then Log("RVP quiet after %.2f s (%s)", waited, tostring(h.why)) end
            h.done = "quiet"
            return false
        end
        if waited >= M.MAX_HOLD_S then
            R.stats.timeouts = R.stats.timeouts + 1
            Log("RVP still busy after %.1f s (%s): %s - travelling anyway", waited, tostring(h.why), cstr(c))
            pcall(ev, "rvp_hold_timeout", { why = h.why, waited_s = waited, paint = c.paint, detect = c.detect })
            h.done = "timeout"
            return false
        end
        return true
    end

    function R.travel_issued()
        local h = R.h
        R.h = nil
        if R.closed then R.closed_at = clock() end   -- REOPEN_S counts from the travel
        return h and h.done or nil
    end

    -- LoadMap pre-hook: the old world never ticks again from here on, and the
    -- new world's BeginPlay has not run. The queue MUST be open again before
    -- that BeginPlay: the arena builds its weapons through RVP tasks, and a
    -- closed queue refuses them (Yard loaded with the limit at 0 has 0
    -- weapon actors instead of 84, and every kit then fails its check).
    -- Exception: tasks still in the queue here (a hold that timed out, a
    -- native travel) would START during the teardown once the queue opens,
    -- which is the crash itself; purge, and if anything is left keep the
    -- queue closed until the new world's first tick (reopen from the caller)
    -- or REOPEN_S: a broken weapon build in that rare case, never the crash.
    function R.on_loadmap()
        R.h = nil
        if not R.closed then return false end
        R.purge()
        local busy, c = R.busy()
        if busy then
            Log("RVP queue kept closed through LoadMap: %s still queued", cstr(c))
            R.closed_at = clock()
            return false
        end
        return R.reopen("LoadMap")
    end

    -- A travel that cannot wait (native OpenLevel pre-hook): close + purge now.
    function R.purge_now(why)
        local c = R.counts()
        if not c then return nil end
        local closed = R.close()
        local n = R.purge()
        if (c.paint + c.detect) > 0 or n > 0 then
            Log("RVP purge at %s: %s; queue %s, %d component(s) purged", tostring(why), cstr(c),
                closed and "closed" or "NOT closed", n)
        end
        return c
    end

    -- Every tick (any world state): a hold that stopped being polled without a
    -- travel (the session ended, the server changed its mind) is dropped and
    -- the queue reopened; a closed queue never outlives REOPEN_S.
    function R.tick()
        local now = clock()
        if R.h and now - (R.h.polled or now) >= M.ABANDON_S then
            Log("RVP hold abandoned (%s): no travel followed", tostring(R.h.why))
            R.h = nil
            R.reopen("hold abandoned")
            return
        end
        if R.closed and R.h == nil and R.closed_at and now - R.closed_at >= M.REOPEN_S then
            R.reopen("no new world after " .. M.REOPEN_S .. " s")
        end
    end

    return R
end

-- --- the real UE access layer ------------------------------------------------------
-- Process-lifetime objects only are cached (subsystem, its TaskQueue, the CDOs);
-- each is re-found when it reads as invalid.
function M.real_ue()
    local ue = { default_limit = 10 }
    local cache = {}

    local function valid(o)
        local ok, v = pcall(function() return o ~= nil and o:IsValid() end)
        return ok and v == true
    end
    local function subsystem()
        if valid(cache.ss) then return cache.ss end
        cache.ss, cache.q = nil, nil
        local ss
        pcall(function() ss = FindFirstOf(M.SUBSYSTEM_CLASS) end)
        if valid(ss) then
            local full = ""
            pcall(function() full = ss:GetFullName() end)
            if not full:find("Default__", 1, true) then cache.ss = ss end
        end
        return cache.ss
    end
    local function task_queue()
        local ss = subsystem()
        if not ss then cache.q = nil; return nil end
        if cache.q_of == ss and valid(cache.q) then return cache.q end
        local q
        pcall(function() q = ss.TaskQueue end)
        cache.q = valid(q) and q or nil
        cache.q_of = cache.q and ss or nil
        return cache.q
    end
    local function cdo(path, key)
        if valid(cache[key]) then return cache[key] end
        local o
        pcall(function() o = StaticFindObject(path) end)
        cache[key] = valid(o) and o or nil
        return cache[key]
    end
    local function map_len(m)
        local n
        pcall(function() n = #m end)
        if type(n) == "number" then return n end
        n = 0
        pcall(function() m:ForEach(function() n = n + 1 end) end)
        return n
    end

    function ue.counts()
        local q = task_queue()
        if not q then return nil end
        local c = {}
        for _, e in ipairs(M.QUEUES) do
            local m
            pcall(function() m = q[e[1]] end)
            c[e[2]] = m and map_len(m) or 0
        end
        return c
    end

    function ue.set_limit(v)
        local s = cdo(M.SETTINGS_CDO, "settings")
        if not s then return false end
        local prev
        pcall(function() prev = s[M.LIMIT_PROP] end)
        local ok = pcall(function() s[M.LIMIT_PROP] = v end)
        local back
        pcall(function() back = s[M.LIMIT_PROP] end)
        return ok and back == v, prev
    end

    -- Remove every component's queued tasks (the plugin's own API). The keys
    -- come from the plugin's UPROPERTY maps: GC nulls destroyed components.
    function ue.purge()
        local q = task_queue()
        local lib = cdo(M.LIB_CDO, "lib")
        if not (q and lib) then return 0 end
        local n = 0
        for _, spec in ipairs({ { "ComponentPaintTaskIDs", "RemoveComponentFromPaintTaskQueue" },
                                { "ComponentDetectTaskIDs", "RemoveComponentFromDetectTaskQueue" } }) do
            local comps = {}
            pcall(function()
                q[spec[1]]:ForEach(function(k)
                    local c; pcall(function() c = k:get() end)
                    if valid(c) then comps[#comps + 1] = c end
                end)
            end)
            for _, c in ipairs(comps) do
                if pcall(function() lib[spec[2]](lib, c) end) then n = n + 1 end
            end
        end
        return n
    end
    return ue
end

return M
