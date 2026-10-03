-- HSMPMenu / local_master.lua — the hosting instance's own hsmp-master.
--
-- Without hsmp.cfg the master_url is the public list (https://master.halfswordmp.workers.dev).
-- Dev and test deploys write master_url = http://127.0.0.1:7778, and a normal launch
-- starts no hsmp-master by itself; without one the host's server cannot
-- register and the browser shows "master_unreachable".
--
-- Rule: when the PRIMARY master_url points at this machine (127.0.0.1 /
-- localhost / ::1), HOST GAME makes sure a master is answering there:
--
--   ensure("HOST")
--     -> probe: is a master answering at the URL?   (async; never blocks the
--        game thread: hsmp-query --master URL, or curl, run through the native
--        spawn_capture; tick() polls its output, no file)
--     -> answering: use it. It is OURS only when this module spawned it in this
--        game (it keeps the pid); a master the test harness or another instance
--        started is never adopted.
--     -> not answering: a master we spawned earlier that hangs is stopped
--        first, then hsmp-master.exe is started (native spawn: no window, no
--        shell; --bind 127.0.0.1:<port> --parent-pid <game>) and re-probed until
--        it answers (3 tries) -> owned = true.
--   stop(reason)            (CANCEL / CLOSE LOBBY / leave / quit)
--     -> only when owned: kill exactly the process we spawned (our process
--        handle, main.lua kill_role("master")). Never by image name. A master
--        we did not start is left running.
--
-- Pure logic + injected effects (ctx), so the offline test drives every path
-- with mocked process / probe results. tick() must run on the game thread.
--
-- ctx = {
--   log(fmt, ...), now() -> seconds,
--   url  = primary master URL,
--   state_dir, master_exe, query_exe (paths, forward slashes),
--   exists(path) -> bool,
--   spawn(exe, args) -> pid | nil, err           -- the master (main.lua records it as "master")
--   spawn_capture(exe, args) -> h | nil, err     -- the probe (stdout captured)
--   capture_poll(h) -> false | true, code, output | nil, err
--   alive(pid) -> bool                           -- a process we spawned is still running
--   kill() -> bool                               -- stop the master we spawned
--   parent_args() -> array                       -- {"--parent-pid", "<game pid>"}
--   mode = "auto" | "off"                        -- hsmp.cfg local_master (default auto)
-- }

local L = {}

local ctx
local Log = function() end

L.PROBE_TIMEOUT_S = 5       -- no probe answer after this = not answering
L.START_GRACE_S   = 1.5     -- wait after the spawn before the first re-probe
L.VERIFY_TRIES    = 3

L.state = "idle"            -- idle | remote | off | probing | starting | verifying | up | missing | failed
L.owned = false
L.pid = nil                 -- the master this module spawned (owned)
L.gen = 0
L.detail = ""

-- host, port of a master URL when it points at this machine; nil otherwise.
function L.local_target(url)
    url = tostring(url or "")
    local scheme, rest = url:match("^(https?)://(.*)$")
    if not scheme then return nil end
    local hostport = rest:match("^([^/]*)")
    local host, port
    if hostport:sub(1, 1) == "[" then
        host, port = hostport:match("^%[([^%]]+)%]:?(%d*)$")
    else
        host, port = hostport:match("^([^:]+):?(%d*)$")
    end
    if not host then return nil end
    host = host:lower()
    local is_local = (host == "127.0.0.1" or host == "localhost" or host == "::1" or host:match("^127%.%d+%.%d+%.%d+$"))
    if not is_local then return nil end
    local p = tonumber(port)
    if not p then p = (scheme == "https") and 443 or 80 end
    if p < 1 or p > 65535 then return nil end
    return host, p
end

-- Fire an async probe. hsmp-query prints "status\tok" when the master
-- answered GET /v1/servers; curl prints the body (a JSON array).
local function start_probe()
    L.probe_t0 = ctx.now()
    L.probe_gen = L.gen
    L.probe_h = nil
    local h, err
    if ctx.query_exe and ctx.exists(ctx.query_exe) then
        L.probe_kind = "query"
        h, err = ctx.spawn_capture(ctx.query_exe, { "--master", ctx.url, "--gen", "lm" .. L.gen, "--timeout-ms", "300" })
    else
        L.probe_kind = "curl"
        h, err = ctx.spawn_capture("curl", { "-s", "-f", "-m", "3", ctx.url .. "/v1/servers" })
    end
    if not h then Log("local master: probe not started (%s)", tostring(err)) end
    L.probe_h = h
end

-- The probe's answer from its output: "up" | "down", why | nil (unusable / an older probe)
local function parse_probe(s, code)
    s = s or ""
    if L.probe_kind == "query" then
        if not s:find("^HSMPQ") then return "down", "no answer (exit " .. tostring(code) .. ")" end
        local g = s:match("\ngen\t([^\r\n]*)")
        if g ~= ("lm" .. L.probe_gen) then return nil end         -- an older probe's answer
        local st = s:match("\nstatus\t([%w_]+)")
        if st == "ok" then return "up" end
        if st and st ~= "trying" then return "down", (s:match("\nstatus\t[%w_]+\t([^\r\n]*)") or st) end
        return "down", "no status"
    end
    local body = s:match("^%s*(.-)%s*$")
    if body:sub(1, 1) == "[" then return "up" end
    if body == "" then return "down", "no answer" end
    return "down", "bad answer"
end

-- "up" | "down" | nil (no answer yet)
local function read_probe()
    if L.probe_h then
        local done, code, out = ctx.capture_poll(L.probe_h)
        if done == nil then
            L.probe_h = nil
            return "down", "probe lost (" .. tostring(code) .. ")"
        end
        if done then
            L.probe_h = nil
            local r, why = parse_probe(out, code)
            if r then return r, why end
        end
    end
    if ctx.now() - L.probe_t0 >= L.PROBE_TIMEOUT_S then return "down", "no answer" end
    return nil
end

local function start_master()
    local exe = ctx.master_exe
    if not exe or not ctx.exists(exe) then
        L.state = "missing"
        L.detail = "hsmp-master.exe not found at " .. tostring(exe)
        Log("local master: %s - this game will not be listed (DIRECT CONNECT / LAN still work)", L.detail)
        return
    end
    -- A master we spawned but nothing answering: hung. Stop exactly that
    -- process (our handle) so the port is free.
    if L.pid then
        Log("local master: ours (pid %s) does not answer - stopping it first", tostring(L.pid))
        pcall(ctx.kill)
        L.pid = nil
    end
    local host, port = L.local_target(ctx.url)
    local bind = (host == "::1") and string.format("[::1]:%d", port) or string.format("127.0.0.1:%d", port)
    local args = { "--bind", bind }
    for _, a in ipairs((ctx.parent_args and ctx.parent_args()) or {}) do args[#args + 1] = a end
    Log("local master: starting %s %s", exe, table.concat(args, " "))
    local pid, err = ctx.spawn(exe, args)
    if not pid then
        L.state = "failed"
        L.detail = "hsmp-master did not start (" .. tostring(err) .. ")"
        Log("local master: %s (DIRECT CONNECT / LAN still work)", L.detail)
        return
    end
    L.pid = pid
    L.owned = true
    L.state = "starting"
    L.start_t = ctx.now()
    L.tries = 0
end

-- HOST: make sure the local master answers. Returns the state.
function L.ensure(reason)
    if not ctx then return "idle" end
    if ctx.mode == "off" then
        L.state = "off"
        Log("local master: disabled (hsmp.cfg local_master = off)")
        return L.state
    end
    if not L.local_target(ctx.url) then
        L.state = "remote"
        Log("local master: master_url %s is not on this machine - nothing to start", tostring(ctx.url))
        return L.state
    end
    if L.state == "probing" or L.state == "starting" or L.state == "verifying" then return L.state end
    -- a master we spawned that has exited is not running (and no longer ours)
    if L.pid and not ctx.alive(L.pid) then L.pid, L.owned = nil, false end
    L.gen = L.gen + 1
    L.state = "probing"
    L.detail = ""
    Log("local master: %s - checking %s", tostring(reason), ctx.url)
    start_probe()
    return L.state
end

-- Advance the state machine (game thread; main.lua's 500 ms loop).
function L.tick()
    if not ctx then return end
    local st = L.state
    if st == "probing" then
        local r, why = read_probe()
        if r == "up" then
            L.state = "up"
            L.owned = L.pid ~= nil and ctx.alive(L.pid)
            Log("local master: %s is answering (%s)", ctx.url,
                L.owned and "ours: started by this game" or "not ours - it is left running on CANCEL")
        elseif r == "down" then
            Log("local master: nothing answers at %s (%s)", ctx.url, tostring(why))
            start_master()
        end
    elseif st == "starting" then
        if ctx.now() - L.start_t >= L.START_GRACE_S then
            L.state = "verifying"
            L.tries = L.tries + 1
            start_probe()
        end
    elseif st == "verifying" then
        local r, why = read_probe()
        if r == "up" then
            L.state = "up"
            Log("local master: started and answering at %s (owned=%s)", ctx.url, tostring(L.owned))
        elseif r == "down" then
            if L.tries < L.VERIFY_TRIES then
                L.tries = L.tries + 1
                start_probe()
            else
                L.state = "failed"
                L.detail = "started hsmp-master but it does not answer (" .. tostring(why) .. ")"
                Log("local master: %s - is port %s taken? (DIRECT CONNECT / LAN still work)", L.detail,
                    tostring(select(2, L.local_target(ctx.url))))
            end
        end
    end
end

-- CANCEL / quit: stop the master only when this game started it.
function L.stop(reason)
    if not ctx then return false end
    L.gen = L.gen + 1                       -- a probe still in flight is ignored
    L.probe_h = nil
    local was = L.state
    L.state = "idle"
    if not L.owned then
        if was == "up" then Log("local master: not ours - left running (%s)", tostring(reason)) end
        return false
    end
    L.owned, L.pid = false, nil
    Log("local master: stopping ours (%s)", tostring(reason))
    local ok, killed = pcall(ctx.kill)
    return ok and killed or false
end

function L.status() return L.state, L.owned, L.detail end

-- SETTINGS changed the server lists: the primary URL is checked from the
-- next HOST on. A master we already own keeps running until CANCEL / quit.
function L.set_url(url)
    if not ctx or not url or url == ctx.url then return end
    ctx.url = url
    if L.state == "remote" or L.state == "off" or L.state == "failed" or L.state == "missing" then L.state = "idle" end
end

function L.init(c)
    ctx = c
    Log = c.log or Log
    L.state, L.owned, L.detail, L.pid = "idle", false, "", nil
    return L
end

return L
