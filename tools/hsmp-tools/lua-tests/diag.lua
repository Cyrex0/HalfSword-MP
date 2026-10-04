-- HSMPMenu/Scripts/diag.lua offline tests: the run's log folder (`hsmp-sidecar
-- --log-session start` captured, then the watcher), `--log-dir` for the sidecar and the
-- server, and the "crashed last time" note.
--
--   hsmp-tools lua-test diag

local function load()
    return dofile(T.path("mods/HSMPMenu/Scripts/diag.lua"))
end

-- a fake HSMP_IPC: spawn_capture -> handle, capture_poll answers from `polls`
local function fake_ipc(polls)
    local f = { captured = {}, spawned = {}, polls = polls }
    function f.spawn_capture(exe, args) f.captured[#f.captured + 1] = { exe = exe, args = args }; return #f.captured end
    function f.capture_poll(h)
        local r = table.remove(f.polls, 1) or { false }
        return table.unpack(r)
    end
    function f.spawn(exe, args, opts) f.spawned[#f.spawned + 1] = { exe = exe, args = args, opts = opts }; return 4242 end
    return f
end

local function ctx(ipc, logs)
    return { log = function(fmt, ...) logs[#logs + 1] = string.format(fmt, ...) end, ipc = function() return ipc end,
             sidecar_exe = "C:\\Game\\hsmp\\hsmp-sidecar.exe", game_pid = 1234, state_dir = "hsmp_state" }
end

T.log("== parse")
do
    local D = load()
    local t = D.parse("dir=C:\\Users\\x\\AppData\\Local\\HSMP\\logs\\20261004_101500_p1234\r\nid=20261004_101500_p1234\nprev_outcome=crashed\njunk line\n")
    T.check(t.dir == "C:\\Users\\x\\AppData\\Local\\HSMP\\logs\\20261004_101500_p1234" and t.id == "20261004_101500_p1234", "dir and id")
    T.check(t.prev_outcome == "crashed" and t.junk == nil, "only key=value lines")
end

T.log("== boot: start is captured, polled, then the watcher is spawned")
do
    local D = load()
    local logs = {}
    local out = "dir=C:\\L\\20261004_101500_p1234\nid=20261004_101500_p1234\nprev_id=20261003_090000_p99\nprev_outcome=crashed\nprev_crash=UECC-Windows-1\n"
    local ipc = fake_ipc({ { false }, { true, 0, out } })
    D.init(ctx(ipc, logs))
    T.check(#D.log_args() == 0, "no --log-dir before the folder is known")
    D.poll()
    T.check(#ipc.captured == 1 and T.eq(ipc.captured[1].args, { "--log-session", "start", "--parent-pid", "1234" }), "start is captured with the game pid")
    D.poll()   -- still running
    T.check(D.dir == nil and #ipc.spawned == 0, "running: nothing yet")
    D.poll()   -- done
    T.check(D.dir == "C:\\L\\20261004_101500_p1234", "folder known")
    T.check(#ipc.spawned == 1, "the watcher is spawned once")
    local w = ipc.spawned[1]
    T.check(w.exe == "C:\\Game\\hsmp\\hsmp-sidecar.exe" and T.eq(w.args, { "--log-session", "watch", "--session-dir", "C:\\L\\20261004_101500_p1234",
        "--parent-pid", "1234", "--state-dir", "hsmp_state" }), "watch args")
    T.check(T.eq(D.log_args(), { "--log-dir", "C:\\L\\20261004_101500_p1234" }), "--log-dir for the sidecar / server")
    T.check(D.crashed_last_time(), "the previous run crashed: the note shows")
    T.check(T.contains(D.CRASH_NOTE, "Create bug report"), "the note names the launcher button")
    D.poll()
    T.check(#ipc.captured == 1 and #ipc.spawned == 1, "done: no more spawns")
end

T.log("== a clean previous run, a failed start, no native module")
do
    local D = load()
    local ipc = fake_ipc({ { true, 0, "dir=C:\\L\\a\nprev_outcome=clean\n" } })
    D.init(ctx(ipc, {}))
    D.poll(); D.poll()
    T.check(D.dir == "C:\\L\\a" and not D.crashed_last_time(), "clean: no note")

    D = load()
    local logs = {}
    ipc = fake_ipc({ { true, 1, "error=LOCALAPPDATA is not set\n" } })
    D.init(ctx(ipc, logs))
    D.poll(); D.poll()
    T.check(D.dir == nil and #ipc.spawned == 0 and #D.log_args() == 0, "failed start: no folder, no watcher, no --log-dir")
    T.check(T.any(logs, function(l) return T.contains(l, "LOCALAPPDATA") end), "the reason is logged")

    D = load()
    ipc = fake_ipc({ { nil, "bad" }, { nil, "bad" }, { nil, "bad" } })
    D.init(ctx(ipc, {}))
    for _ = 1, 10 do D.poll() end
    T.check(D.done and #ipc.captured == 3, "a lost capture is retried at most 3 times")

    D = load()
    D.init(ctx(nil, {}))
    D.poll()
    T.check(D.done and #D.log_args() == 0, "no native module: nothing to do")
end
