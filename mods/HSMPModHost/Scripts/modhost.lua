-- HSMPModHost / modhost.lua -- loads the server mods the player accepted, each in its own
-- environment, tracks what they register with UE4SS and undoes it when the session ends
-- (docs/development/lua-mods.md "Server mods", docs/hosting/server-mods.md).
--
-- NOT A SECURITY BOUNDARY. A server mod runs in this Lua state with the full power of any
-- UE4SS Lua mod; the player agreed to that. The environment only keeps mods from trampling
-- each other's globals and HSMP's: each mod gets
--   * its own globals table (reads fall through to _G; writes stay in the mod),
--   * `HSMPNative` and `HSMP_IPC` hidden (nil), `_G` pointing at the mod's own table,
--   * `require` that resolves only inside the mod's own Scripts folder (plus UE4SS's
--     shared `UEHelpers`), text chunks only (no precompiled bytecode), no `package.loadlib`,
--   * wrapped registration APIs: every callback runs under xpcall (an error is logged with the
--     mod's name and never reaches HSMP) and is recorded so `unload` can undo it.
--
-- What unload can undo, and what it cannot (UE4SS has no API to remove them):
--   removed   RegisterHook (UnregisterHook), RegisterCustomEvent (UnregisterCustomEvent),
--             LoopInGameThreadWithDelay / ExecuteInGameThreadWithDelay / LoopAsync /
--             ExecuteWithDelay (CancelDelayedAction where a handle exists; the loop stops)
--   inert     NotifyOnNewObject, RegisterKeyBind(Async), RegisterConsoleCommand(Global)Handler,
--             RegisterLoadMap/InitGameState/BeginPlay/ProcessConsoleExec/
--             CallFunctionByNameWithArguments/ULocalPlayerExec Pre/Post hooks:
--             they stay registered with UE4SS, but their callbacks do nothing any more.
--   never     changes the mod made to game objects, spawned actors, property writes,
--             files it wrote: a game restart is the only full reset (the menu says so).
-- A mod may define a global `OnUnload()`; it is called first.

local M = {}

-- UE4SS shared libraries a server mod may require (from ue4ss/Mods/shared).
M.SHARED = { UEHelpers = true }
-- Globals a server mod never sees.
M.HIDDEN = { HSMPNative = true, HSMP_IPC = true }
-- Error lines per mod before they are only counted.
M.MAX_ERROR_LOGS = 20

-- Registration APIs. kind: hook | event | delay (handle-cancelled, may loop) | inert.
-- cb = position of the callback argument; dead = what a dead callback returns.
M.APIS = {
    RegisterHook = { kind = "hook" },
    RegisterCustomEvent = { kind = "event", cb = 2 },
    NotifyOnNewObject = { kind = "inert", cb = 2 },
    RegisterKeyBind = { kind = "inert", cb = -1 },
    RegisterKeyBindAsync = { kind = "inert", cb = -1 },
    RegisterConsoleCommandHandler = { kind = "inert", cb = 2, dead = false },
    RegisterConsoleCommandGlobalHandler = { kind = "inert", cb = 2, dead = false },
    RegisterLoadMapPreHook = { kind = "inert", cb = 1 },
    RegisterLoadMapPostHook = { kind = "inert", cb = 1 },
    RegisterInitGameStatePreHook = { kind = "inert", cb = 1 },
    RegisterInitGameStatePostHook = { kind = "inert", cb = 1 },
    RegisterBeginPlayPreHook = { kind = "inert", cb = 1 },
    RegisterBeginPlayPostHook = { kind = "inert", cb = 1 },
    RegisterProcessConsoleExecPreHook = { kind = "inert", cb = 1 },
    RegisterProcessConsoleExecPostHook = { kind = "inert", cb = 1 },
    RegisterCallFunctionByNameWithArgumentsPreHook = { kind = "inert", cb = 1 },
    RegisterCallFunctionByNameWithArgumentsPostHook = { kind = "inert", cb = 1 },
    RegisterULocalPlayerExecPreHook = { kind = "inert", cb = 1 },
    RegisterULocalPlayerExecPostHook = { kind = "inert", cb = 1 },
    LoopAsync = { kind = "delay", cb = 2, dead = true },
    LoopInGameThreadWithDelay = { kind = "delay", cb = 2, dead = true },
    ExecuteWithDelay = { kind = "delay", cb = 2 },
    ExecuteInGameThreadWithDelay = { kind = "delay", cb = 2 },
    ExecuteInGameThread = { kind = "delay", cb = 1 },
}

local function valid_name(n)
    return type(n) == "string" and #n >= 1 and #n <= 32 and n:match("^[%w_%-]+$") ~= nil and n:lower():sub(1, 4) ~= "hsmp"
end

-- api: { G = real globals, cache = cache dir, read = fn(path) -> string|nil, log = fn(fmt, ...),
--        event = fn(name, fields) | nil }
function M.new(api)
    local H = { mods = {}, set = nil, api = api }
    local G = api.G
    local log = api.log or function() end

    local function report(m, what, err)
        m.errors = m.errors + 1
        if m.errors <= M.MAX_ERROR_LOGS then
            log("server mod %s: error in %s: %s", m.name, what, tostring(err))
        elseif m.errors == M.MAX_ERROR_LOGS + 1 then
            log("server mod %s: more errors; only counted from now on", m.name)
        end
        if api.event and m.errors <= 3 then pcall(api.event, "x_server_mod_error", { mod = m.name, what = what, error = tostring(err):sub(1, 200) }) end
    end

    -- A callback the mod hands to UE4SS: inert once the mod is unloaded, never raises.
    local function guard(m, fn, what, dead)
        if type(fn) ~= "function" then return fn end
        return function(...)
            if not m.alive then return dead end
            local r = table.pack(xpcall(fn, debug.traceback, ...))
            if not r[1] then report(m, what, r[2]); return nil end
            return table.unpack(r, 2, r.n)
        end
    end

    local function load_text(m, path, chunkname, env)
        local src = api.read(path)
        if not src then return nil, "not found" end
        if src:sub(1, 1) == "\27" then return nil, "precompiled chunks are not allowed" end
        return load(src, chunkname, "t", env)
    end

    local function make_env(m)
        local env = {}
        setmetatable(env, { __index = function(_, k)
            if M.HIDDEN[k] then return nil end
            return G[k]
        end })
        env._G = env
        env.package = { loaded = m.loaded, path = m.scripts .. "/?.lua", cpath = "" }
        env.require = function(name)
            if type(name) ~= "string" then error("require: a module name", 2) end
            if m.loaded[name] ~= nil then return m.loaded[name] end
            if name:match("^[%w_%-%.]+$") and not name:find("..", 1, true) and name:sub(1, 1) ~= "." then
                local rel = name:gsub("%.", "/")
                for _, p in ipairs({ rel .. ".lua", rel .. "/init.lua" }) do
                    local chunk, err = load_text(m, m.scripts .. "/" .. p, "@" .. m.name .. "/Scripts/" .. p, env)
                    if chunk then
                        local v = chunk(name)
                        if v == nil then v = true end
                        m.loaded[name] = v
                        return v
                    elseif err ~= "not found" then
                        error(string.format("require %s: %s", name, tostring(err)), 2)
                    end
                end
            end
            if M.SHARED[name] then return G.require(name) end
            error(string.format("module '%s' not found in %s's Scripts folder", name, m.name), 2)
        end
        for api_name, spec in pairs(M.APIS) do
            local real = G[api_name]
            if type(real) == "function" then
                env[api_name] = function(...)
                    local a = table.pack(...)
                    if spec.kind == "hook" then
                        a[2] = guard(m, a[2], api_name .. " " .. tostring(a[1]))
                        a[3] = guard(m, a[3], api_name .. " " .. tostring(a[1]))
                        local pre, post = real(table.unpack(a, 1, a.n))
                        m.hooks[#m.hooks + 1] = { path = a[1], pre = pre, post = post }
                        return pre, post
                    end
                    local i = spec.cb == -1 and a.n or spec.cb
                    a[i] = guard(m, a[i], api_name, spec.dead)
                    local r = table.pack(real(table.unpack(a, 1, a.n)))
                    if spec.kind == "event" then m.events[#m.events + 1] = a[1]
                    elseif spec.kind == "delay" then if r[1] ~= nil then m.handles[#m.handles + 1] = r[1] end
                    else m.inert = m.inert + 1 end
                    return table.unpack(r, 1, r.n)
                end
            end
        end
        -- A worker-thread callback would run inside this shared Lua state: server mods get the
        -- game-thread variant instead (docs/development/lua-mods.md 5.1).
        env.ExecuteAsync = env.ExecuteInGameThread -- unsafe: ok rerouted to the game thread
        return env
    end

    -- Undo what can be undone (see the file header). Returns a summary table.
    function H.unload_mod(m)
        if not m.alive then return nil end
        if m.env and type(rawget(m.env, "OnUnload")) == "function" then
            local ok, err = xpcall(m.env.OnUnload, debug.traceback)
            if not ok then report(m, "OnUnload", err) end
        end
        m.alive = false
        H.inert_left = (H.inert_left or 0) + m.inert
        local s = { hooks = 0, events = 0, cancelled = 0, inert = m.inert }
        for _, h in ipairs(m.hooks) do
            if type(G.UnregisterHook) == "function" and pcall(G.UnregisterHook, h.path, h.pre, h.post) then s.hooks = s.hooks + 1 end
        end
        for _, e in ipairs(m.events) do
            if type(G.UnregisterCustomEvent) == "function" and pcall(G.UnregisterCustomEvent, e) then s.events = s.events + 1 end
        end
        for _, h in ipairs(m.handles) do
            if type(G.CancelDelayedAction) == "function" and pcall(G.CancelDelayedAction, h) then s.cancelled = s.cancelled + 1 end
        end
        m.hooks, m.events, m.handles = {}, {}, {}
        for k in pairs(m.loaded) do m.loaded[k] = nil end
        log("server mod %s unloaded: %d hook(s) removed, %d custom event(s) removed, %d delayed action(s) cancelled, "
            .. "%d registration(s) inert until a game restart", m.name, s.hooks, s.events, s.cancelled, s.inert)
        return s
    end

    function H.load_mod(e)
        local m = { name = e.name, hash = e.hash, alive = true, errors = 0, hooks = {}, events = {}, handles = {}, inert = 0, loaded = {} }
        if not valid_name(e.name) or type(e.hash) ~= "string" or not e.hash:match("^%x+$") or #e.hash ~= 64 then
            log("server mod %s refused: bad name or hash", tostring(e.name))
            m.alive, m.failed, m.missing = false, true, true
            return m
        end
        m.scripts = api.cache .. "/" .. e.hash:lower() .. "/Scripts"
        m.env = make_env(m)
        local chunk, err = load_text(m, m.scripts .. "/main.lua", "@" .. m.name .. "/Scripts/main.lua", m.env)
        if not chunk then
            log("server mod %s: cannot load Scripts/main.lua: %s", m.name, tostring(err))
            m.alive, m.failed, m.missing = false, true, err == "not found"
            return m
        end
        local ok, rerr = xpcall(chunk, debug.traceback)
        if not ok then
            report(m, "Scripts/main.lua", rerr)
            H.unload_mod(m)
            m.failed = true
        else
            log("server mod %s loaded (%s)", m.name, e.hash:sub(1, 12))
        end
        return m
    end

    -- Load a set (entries in set order: { name, hash }). Returns ok, failed count, text.
    -- ok = false only when a mod's files are missing from the cache.
    function H.load_set(set, entries)
        if H.set == set then return true, H.failed or 0, "already loaded" end
        H.unload_all("a new mod set")
        local failed, missing, names = 0, 0, {}
        for _, e in ipairs(entries) do
            local m = H.load_mod(e)
            H.mods[#H.mods + 1] = m
            if m.failed then failed = failed + 1; names[#names + 1] = tostring(e.name) end
            if m.missing then missing = missing + 1 end
        end
        H.set, H.failed = set, failed
        if missing > 0 then return false, failed, "missing from the cache: " .. table.concat(names, ", ") end
        if failed > 0 then return true, failed, "failed to start: " .. table.concat(names, ", ") end
        return true, 0, ""
    end

    function H.unload_all(why)
        if #H.mods == 0 and not H.set then return end
        log("unloading %d server mod(s) (%s)", #H.mods, tostring(why))
        for i = #H.mods, 1, -1 do H.unload_mod(H.mods[i]) end
        H.mods, H.set, H.failed = {}, nil, nil
    end

    -- Registrations of unloaded mods that stay with UE4SS (inert) until a game restart.
    function H.inert_left_total() return H.inert_left or 0 end

    return H
end

return M
