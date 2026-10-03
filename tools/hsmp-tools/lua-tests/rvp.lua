-- shared/hsmp_rvp.lua offline tests (the Runtime Vertex Paint quiesce
-- before a level change; docs/development/crash-rr.md).
--
--   hsmp-tools lua-test rvp
--
-- The module is driven through its injectable `ue` layer (queue sizes, the
-- settings limit, purge) and clock; then the real UE layer (real_ue) runs
-- against a mock UE4SS object model (FindFirstOf / StaticFindObject / TMap).

local M = dofile(T.path("mods/shared/hsmp_rvp.lua"))

local function fake(o)
    o = o or {}
    local f = { clock = 10.0, limit = o.limit or 10, paint = 0, detect = 0, purges = 0, purged_comps = 0,
                limit_ok = o.limit_ok ~= false, logs = {}, evs = {}, plugin = o.plugin ~= false, sets = {} }
    f.ue = {
        default_limit = 10,
        counts = function()
            if not f.plugin then return nil end
            return { paint = f.paint, detect = f.detect, paint_comps = f.paint > 0 and 1 or 0, detect_comps = f.detect > 0 and 1 or 0 }
        end,
        set_limit = function(v)
            f.sets[#f.sets + 1] = v
            if not f.limit_ok then return false, f.limit end
            local prev = f.limit; f.limit = v; return true, prev
        end,
        purge = function()
            f.purges = f.purges + 1
            local n = (f.paint > 0 and 1 or 0) + (f.detect > 0 and 1 or 0)
            f.purged_comps = f.purged_comps + n
            if f.purge_clears then f.paint, f.detect = f.running or 0, 0 end
            return n
        end,
    }
    f.R = M.new({ ue = f.ue, clock = function() return f.clock end,
        log = function(fmt, ...) f.logs[#f.logs + 1] = string.format(fmt, ...) end,
        ev = function(n, x) f.evs[#f.evs + 1] = { n = n, f = x } end })
    function f:step(dt) self.clock = self.clock + (dt or 0.05) end
    function f:logtext() return table.concat(self.logs, "\n") end
    -- hold until it lets go; returns the number of held calls and the elapsed time
    function f:run(dt, max)
        local n, t0 = 0, self.clock
        for _ = 1, (max or 1000) do
            if not self.R.hold("test") then return n, self.clock - t0 end
            n = n + 1
            if self.on_step then self.on_step(self) end
            self:step(dt)
        end
        return n, self.clock - t0
    end
    return f
end

T.log("== no plugin: nothing to wait for")
do
    local f = fake({ plugin = false })
    T.check(f.R.hold("x") == false and #f.sets == 0 and f.purges == 0, "counts nil -> no hold, no close, no purge")
    T.check(f.R.purge_now("native") == nil, "purge_now without the plugin is a no-op")
end

T.log("== idle queue: closed, held only for the quiet window")
do
    local f = fake()
    local n, el = f:run(0.05)
    T.check(n >= 1 and el >= M.QUIET_S - 1e-9 and el < M.QUIET_S + 0.06, "idle: held ~QUIET_S", T.repr({ n, el }))
    T.check(f.R.closed and f.limit == 0 and f.sets[1] == 0, "the queue is closed (limit 0) for the travel")
    T.check(f.purges == 1, "one purge at the start", tostring(f.purges))
    T.check(T.contains(f:logtext(), "RVP quiesce before travel (test): paint=0 detect=0"), "the quiesce is logged", f:logtext())
    T.check(f.R.travel_issued() == "quiet", "travel_issued reports how the hold ended")
    T.check(f.R.closed, "still closed between OpenLevel and the new world")
    T.check(f.R.reopen("new world") == true and f.limit == 10 and not f.R.closed, "reopen restores the saved limit (10)", tostring(f.limit))
    T.check(f.R.reopen("again") == false, "reopen twice is a no-op")
end

T.log("== busy queue: wait for in-flight tasks, re-purge, then go")
do
    local f = fake()
    f.paint, f.detect = 3, 1
    local busy_until = f.clock + 0.6     -- the running task finishes after 0.6 s
    f.on_step = function(s) if s.clock >= busy_until then s.paint, s.detect = 0, 0 end end
    local n, el = f:run(0.05)
    T.check(el >= 0.6 + M.QUIET_S - 1e-9 and el < 0.6 + M.QUIET_S + 0.11, "held until empty + QUIET_S", T.repr({ n, el }))
    T.check(f.purges >= 5, "re-purged while waiting (every PURGE_EVERY_S)", tostring(f.purges))
    T.check(T.contains(f:logtext(), "RVP quiet after"), "a long wait is logged")
    T.check(f.R.stats.holds == 1 and f.R.stats.timeouts == 0 and f.R.stats.max_wait_s >= 0.6, "stats", T.repr(f.R.stats))
end

T.log("== a task re-queued during the quiet window restarts it")
do
    local f = fake()
    local t0 = f.clock
    f.on_step = function(s)
        local dt = s.clock - t0
        s.paint = (dt > 0.1 and dt < 0.16) and 1 or 0   -- one blip after 0.1 s
    end
    local _, el = f:run(0.05)
    T.check(el >= 0.15 + M.QUIET_S - 1e-9, "quiet window restarted after the blip", tostring(el))
end

T.log("== never blocks: MAX_HOLD_S")
do
    local f = fake()
    f.paint = 2                          -- a corpse that never stops bleeding (close failed)
    local n, el = f:run(0.05)
    T.check(el >= M.MAX_HOLD_S - 1e-9 and el < M.MAX_HOLD_S + 0.06, "lets go after MAX_HOLD_S", T.repr({ n, el }))
    T.check(f.R.stats.timeouts == 1 and f.evs[1] and f.evs[1].n == "rvp_hold_timeout" and f.evs[1].f.paint == 2,
        "rvp_hold_timeout event", T.repr(f.evs))
    T.check(T.contains(f:logtext(), "travelling anyway"), "timeout logged")
    T.check(f.R.travel_issued() == "timeout", "travel_issued -> timeout")
end

T.log("== reopen is refused while a hold is in progress")
do
    local f = fake()
    f.paint = 1
    T.check(f.R.hold("x") == true and f.R.closed, "holding, closed")
    T.check(f.R.reopen("world key blip") == false and f.R.closed and f.limit == 0,
        "a world-key change inside the old world does not reopen the queue")
    f.paint = 0; f:step(1)
    T.check(f.R.hold("x") == true, "quiet window starts")
    f:step(1)
    T.check(f.R.hold("x") == false, "then lets go")
    f.R.travel_issued()
    T.check(f.R.reopen("new world") == true and f.limit == 10, "reopen after the travel")
end

T.log("== saved limit 0 / unreadable -> plugin default on reopen")
do
    local f = fake({ limit = 0 })
    f.R.hold("x"); f:step(1); f.R.hold("x"); f.R.travel_issued()
    T.check(f.R.reopen("w") and f.limit == 10, "a saved 0 is never restored (the queue would stay closed)", tostring(f.limit))
end

T.log("== close failure: still waits for the queue, says NOT closed")
do
    local f = fake({ limit_ok = false })
    f.paint = 1
    T.check(f.R.hold("x") == true and not f.R.closed, "held without a closed queue")
    T.check(T.contains(f:logtext(), "NOT closed"), "logged as NOT closed")
    f.paint = 0; f:step(0.3); f.R.hold("x"); f:step(0.3)
    T.check(f.R.hold("x") == false, "lets go once quiet")
    T.check(f.R.reopen("w") == false, "nothing to reopen")
end

T.log("== tick: a closed queue never outlives REOPEN_S")
do
    local f = fake()
    f.R.hold("x"); f:step(1); f.R.hold("x"); f.R.travel_issued()
    f:step(M.REOPEN_S - 1); f.R.tick()
    T.check(f.R.closed, "still closed before REOPEN_S")
    f:step(2); f.R.tick()
    T.check(not f.R.closed and f.limit == 10 and T.contains(f:logtext(), "no new world after"), "reopened by tick after REOPEN_S")
end

T.log("== a hold nobody polls any more (no travel came) is abandoned and the queue reopened")
do
    local f = fake()
    f.paint = 1
    T.check(f.R.hold("round 2 starting") == true and f.R.closed, "holding, closed")
    f:step(0.5); f.R.tick()
    T.check(f.R.h ~= nil and f.R.closed, "polled recently: kept")
    f:step(0.6); f.R.tick()
    T.check(f.R.h == nil and not f.R.closed and f.limit == 10 and T.contains(f:logtext(), "hold abandoned"),
        "not polled for ABANDON_S: dropped, queue reopened", f:logtext())
    T.check(f.R.hold("again") == true and f.R.stats.holds == 2, "a later travel starts a fresh hold")
end

T.log("== LoadMap reopens the queue before the new world's BeginPlay")
do
    local f = fake()
    f.R.hold("x"); f:step(1); f.R.hold("x"); f.R.travel_issued()
    T.check(f.R.closed, "closed after the travel was issued")
    T.check(f.R.on_loadmap() == true and not f.R.closed and f.limit == 10 and T.contains(f:logtext(), "reopened (LoadMap)"),
        "on_loadmap: empty queue -> reopened (the arena's weapons are built through RVP tasks)")
    T.check(f.R.on_loadmap() == false, "nothing to do when already open")
    -- tasks still queued at LoadMap (a hold that timed out): purge; if they stay, keep closed
    local g = fake()
    g.paint = 3
    g.R.purge_now("native OpenLevel")
    T.check(g.R.on_loadmap() == false and g.R.closed and g.limit == 0 and T.contains(g:logtext(), "kept closed through LoadMap"),
        "stuck tasks: kept closed (they would start during the teardown)", g:logtext())
    T.check(g.purges >= 2, "purged again at LoadMap")
    g.paint = 0
    T.check(g.R.reopen("new world") == true and g.limit == 10, "the new world's first tick reopens it")
    -- a hold in progress (no travel_issued, e.g. a native travel during a hold) is dropped by LoadMap
    local h = fake()
    h.R.hold("x")
    T.check(h.R.on_loadmap() == true and h.R.h == nil and not h.R.closed, "LoadMap ends a pending hold and reopens")
    -- purge at LoadMap clears the queue -> reopen
    local k = fake()
    k.paint = 2; k.purge_clears = true
    k.R.purge_now("native")
    T.check(k.R.on_loadmap() == true and not k.R.closed, "purge at LoadMap emptied the queue -> reopened")
end

T.log("== purge_now (native travel): close + purge at once")
do
    local f = fake()
    f.paint = 2
    local c = f.R.purge_now("native OpenLevel(Map_Hub_Tavern_Frank)")
    T.check(c and c.paint == 2 and f.R.closed and f.purges == 1, "closed + purged synchronously")
    T.check(T.contains(f:logtext(), "RVP purge at native OpenLevel"), "logged")
    T.check(f.R.reopen("new world") and f.limit == 10, "reopened by the new world")
end

T.log("== a hold after a finished travel starts afresh")
do
    local f = fake()
    f.R.hold("a"); f:step(1); f.R.hold("a"); f.R.travel_issued(); f.R.reopen("w")
    f.paint = 1
    T.check(f.R.hold("b") == true and f.R.stats.holds == 2 and f.R.closed, "second travel: new hold, closed again")
end

-- ---------------------------------------------------------------------------
-- real_ue against a mock UE4SS object model

T.log("== real_ue: counts / set_limit / purge through UE4SS objects")
do
    local function obj(t)
        t.IsValid = function() return t.valid ~= false end
        t.GetFullName = function() return t.full or "Obj /Engine/Transient.X" end
        return t
    end
    local function tmap(keys)
        local m = { keys = keys }
        return setmetatable(m, {
            __len = function(s) return #s.keys end,
            __index = {
                ForEach = function(s, fn) for _, k in ipairs(s.keys) do fn({ get = function() return k end }, {}) end end,
            },
        })
    end
    local compA, compB = obj({ name = "A" }), obj({ name = "B" })
    local dead = obj({ name = "dead", valid = false })
    local q = obj({
        CalculateColorsPaintQueue = tmap({ 1, 2, 3 }),
        CalculateColorsDetectionQueue = tmap({ 9 }),
        ComponentPaintTaskIDs = tmap({ compA, dead }),
        ComponentDetectTaskIDs = tmap({ compB }),
    })
    local ss = obj({ TaskQueue = q, full = "VertexPaintDetectionGISubSystem /Engine/Transient.GameInstance.VPDSub" })
    local removed = {}
    local lib = obj({
        RemoveComponentFromPaintTaskQueue = function(_, c) removed[#removed + 1] = "P:" .. c.name end,
        RemoveComponentFromDetectTaskQueue = function(_, c) removed[#removed + 1] = "D:" .. c.name end,
    })
    local settings = obj({ MaxAmountOfAllowedTasksPerMesh = 7 })
    local finds = 0
    local oldF, oldS = rawget(_G, "FindFirstOf"), rawget(_G, "StaticFindObject")
    _G.FindFirstOf = function(cls) finds = finds + 1; if cls == M.SUBSYSTEM_CLASS then return ss end end
    _G.StaticFindObject = function(p)
        if p == M.SETTINGS_CDO then return settings end
        if p == M.LIB_CDO then return lib end
    end
    local ue = M.real_ue()
    local c = ue.counts()
    T.check(c and c.paint == 3 and c.detect == 1 and c.paint_comps == 2 and c.detect_comps == 1, "queue sizes via #TMap", T.repr(c))
    ue.counts()
    T.check(finds == 1, "subsystem cached (one FindFirstOf)", tostring(finds))
    local ok, prev = ue.set_limit(0)
    T.check(ok and prev == 7 and settings.MaxAmountOfAllowedTasksPerMesh == 0, "limit written + read back, previous returned")
    T.check(ue.purge() == 2 and table.concat(removed, ",") == "P:A,D:B", "purge removes every valid component, skips nulled ones",
        table.concat(removed, ","))
    ss.valid = false
    T.check(ue.counts() == nil and finds == 2, "an invalid subsystem is re-found (and its queue dropped)", tostring(finds))
    _G.FindFirstOf = function() return nil end
    ss.valid = true
    local ue2 = M.real_ue()
    T.check(ue2.counts() == nil and ue2.purge() == 0, "no subsystem: counts nil, purge 0")
    _G.FindFirstOf = function() return obj({ full = "VertexPaintDetectionGISubSystem /Script/X.Default__VertexPaintDetectionGISubSystem" }) end
    local ue3 = M.real_ue()
    T.check(ue3.counts() == nil, "the class default object is never used as the subsystem")
    _G.FindFirstOf, _G.StaticFindObject = oldF, oldS
end
