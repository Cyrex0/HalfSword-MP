-- diag.lua -- this run's log folder (%LOCALAPPDATA%\HSMP\logs\<stamp>_p<pid>) and the
-- "the game crashed last time" note (docs/players/troubleshooting.md).
--
-- At boot the menu runs `hsmp-sidecar --log-session start` (captured, polled from the
-- 500 ms game-thread loop), which creates the folder and says how the previous run ended.
-- Then it starts `hsmp-sidecar --log-session watch`, a separate process that waits for the
-- game to exit and copies UE4SS.log, the crash folders and the event logs into the folder.
-- The game thread only spawns those two processes and reads one short output: no file I/O.
-- The sidecar and a listen server get `--log-dir <folder>` (D.log_args()).

local D = { dir = nil, id = nil, prev_outcome = nil, prev_id = nil, done = false, tries = 0, h = nil }

D.CRASH_NOTE = "The game crashed last time. Open the launcher > Create bug report to send us the logs."

local ctx   -- { log, ipc() -> HSMP_IPC or nil, sidecar_exe (Windows path), game_pid, state_dir (Windows path) }

function D.init(c) ctx = c end

-- "key=value" lines -> table
function D.parse(out)
    local t = {}
    for line in tostring(out or ""):gmatch("[^\r\n]+") do
        local k, v = line:match("^([%w_]+)=(.*)$")
        if k then t[k] = v end
    end
    return t
end

local function spawn_watch(ipc)
    local args = { "--log-session", "watch", "--session-dir", D.dir, "--parent-pid", tostring(ctx.game_pid),
                   "--state-dir", ctx.state_dir }
    local pid, err = ipc.spawn(ctx.sidecar_exe, args, {})
    ctx.log("logs: %s (watcher %s)", D.dir, pid and ("pid " .. tostring(pid)) or ("NOT started: " .. tostring(err)))
end

-- Apply the `start` result (also the offline tests' entry point). Returns D.
function D.on_start(code, out)
    local t = D.parse(out)
    if tonumber(code) ~= 0 or not t.dir or t.dir == "" then
        ctx.log("logs: no session folder (exit %s: %s)", tostring(code), tostring(t.error or out))
        return D
    end
    D.dir, D.id = t.dir, t.id
    D.prev_id, D.prev_outcome = t.prev_id, t.prev_outcome
    if D.prev_outcome then ctx.log("logs: the previous run %s ended %s", tostring(D.prev_id), D.prev_outcome) end
    local ipc = ctx.ipc()
    if ipc and ipc.spawn then spawn_watch(ipc) end
    return D
end

-- Called every 500 ms on the game thread until the folder is known (or given up).
function D.poll()
    if D.done or not ctx then return end
    local ipc = ctx.ipc()
    if not (ipc and ipc.spawn_capture and ctx.game_pid) then D.done = true; return end
    if not D.h then
        D.tries = D.tries + 1
        if D.tries > 3 then D.done = true; ctx.log("logs: --log-session start did not run; no session folder"); return end
        D.h = ipc.spawn_capture(ctx.sidecar_exe, { "--log-session", "start", "--parent-pid", tostring(ctx.game_pid) })
        return
    end
    local done, code, out = ipc.capture_poll(D.h)
    if done == false then return end
    D.h = nil
    if done == nil then return end   -- lost: started again on the next poll
    D.done = true
    D.on_start(code, out)
end

-- Extra arguments for hsmp-sidecar / hsmp-server: their log file in this run's folder.
function D.log_args()
    if D.dir and D.dir ~= "" then return { "--log-dir", D.dir } end
    return {}
end

function D.crashed_last_time() return D.prev_outcome == "crashed" end

return D
