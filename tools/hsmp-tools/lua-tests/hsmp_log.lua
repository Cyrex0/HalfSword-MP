-- Suite: shared/hsmp_log.lua + shared/hsmp_cfg.lua.
--   hsmp-tools lua-test hsmp_log
-- Each dofile() of a library is a separate module instance, standing in for a
-- separate mod (UE4SS gives every mod its own Lua state). os.getenv / os.clock /
-- os.time / print are mocked; files go to a temp state dir.

local ENV, CLK, EPOCH, PRINTED = {}, { 1.0 }, 1759340000, {}
os.getenv = function(k) return ENV[k] end
os.clock = function() return CLK[1] end
os.time = function() return math.floor(EPOCH + CLK[1]) end
print = function(...) PRINTED[#PRINTED + 1] = table.concat({ ... }, " ") end

local LOG = T.path("mods/shared/hsmp_log.lua")
local CFG = T.path("mods/shared/hsmp_cfg.lua")

local function lines(p)
    local out, torn = {}, 0
    local s = T.read(p) or ""
    for l in s:gmatch("[^\n]+") do
        local ok, v = pcall(T.json_decode, l)
        if ok then out[#out + 1] = v else torn = torn + 1 end
    end
    return out, torn
end
local function where(list, f) local r = {} for _, e in ipairs(list) do if f(e) then r[#r + 1] = e end end return r end

-- --- vocabulary, envelope, encoding -----------------------------------------------
local sd = T.tmpdir("hsmp_log_")
ENV.HSMP_STATE_DIR, ENV.HSMP_INST = sd, "2"
local H1 = dofile(LOG)
local H2 = dofile(LOG)
H1.init("HSMPMatch")
H2.init { mod = "HSMPLoadout", state_dir = sd, echo = false } -- table form

H1.lobby_ready { epoch = 3 }
H1.cmd_sent("start", "c1")
H1.cmd_result("start", "c1", true, nil)
H1.travel("Map_Menu_Startup", "Map_Arena_Alley", "director", { round = 1 })
CLK[1] = CLK[1] + 0.25
H1.world_ready("Map_Arena_Alley", "W#5", { round = 1 })
H2.kit_verified("self", 7, "BP_Sword_C", "None", { round = 1, exp_armour_n = 7 })
H2.kit_verified("peer:1", 7, "BP_Sword_C", "None", { round = 1 })
H1.willie_census(2, 2, { round = 1 })
H1.spawn_verified { round = 1, ok = true, dist_m = 0.4, min_peer_m = 4.2, health_ok = true, z_ok = true }
H1.save_redirected("SaveGameToSlot", "GameProgress", "HSMP_2_GameProgress")
H1.native_travel_rewritten("Hub_Tavern", "Map_Arena_Alley")
H1.hitch(2500, { travel = true })
H1.ready_report(1)
H1.pose_quality(1, 2.5, 4.0, 61, { round = 1, jitter_ratio = 1.1, foot_slide_p95 = 3.0, idle_rms = 0.2 })
for _, n in ipairs { "load_failed", "round_void", "death_ignored", "notice", "cmd_timeout" } do
    H1.event(n, { round = 1, error = "x", why = "x", cmd = "start", cmd_id = "c2" })   -- session event names are vocabulary
end
H1.event("travel", { from = "a" })               -- missing to/by
H1.event("totally_new_event", {})                -- not in the vocabulary
H1.event("x_probe", { s = 'q"uo\\te\n\t\1 \195\156n\195\175', nan = 0 / 0, inf = math.huge,
                      nested = { a = { 1, 2, 3 } }, fn = print })

local path = sd .. "/hsmp_events.jsonl"
local evs, torn = lines(path)
T.check(torn == 0, "every line is valid JSON", torn)
T.check(#where(evs, function(e) return e.ev == "_open" end) == 2, "two _open events")
local env_ok = true
for _, e in ipairs(evs) do
    for _, k in ipairs { "v", "ev", "inst", "mod", "seq", "t_ms", "wall_ms" } do
        if e[k] == nil then env_ok = false end
    end
    if e.inst ~= "2" then env_ok = false end
end
T.check(env_ok, "envelope v/ev/inst/mod/seq/t_ms/wall_ms with inst=2 on every event")
for _, name in ipairs { "lobby_ready", "cmd_sent", "cmd_result", "travel", "world_ready", "spawn_verified",
                        "kit_verified", "willie_census", "save_redirected", "native_travel_rewritten",
                        "hitch", "ready_report", "pose_quality", "load_failed", "round_void",
                        "death_ignored", "notice", "cmd_timeout" } do
    T.check(#where(evs, function(e) return e.ev == name end) > 0, "vocabulary event " .. name .. " written")
end
local tr = where(evs, function(e) return e.ev == "travel" end)
T.check(#tr == 2 and tr[1].by == "director" and tr[1].to == "Map_Arena_Alley" and tr[1]._missing == nil, "travel fields")
T.check(tr[2] and tr[2]._missing and tr[2]._missing[1] == "to" and tr[2]._missing[2] == "by", "_missing flags to/by", T.repr(tr[2]))
local bad = where(evs, function(e) return e.ev == "_bad_event" end)
T.check(#bad == 1 and bad[1].bad_ev == "totally_new_event", "unknown name -> _bad_event")
local xp = where(evs, function(e) return e.ev == "x_probe" end)[1]
T.check(xp and xp.s == 'q"uo\\te\n\t\1 \195\156n\195\175', "string escapes round-trip", xp and xp.s)
T.check(xp and xp.nan == nil and xp.inf == nil, "nan/inf -> null")
T.check(xp and xp.nested and xp.nested.a and xp.nested.a[3] == 3, "nested tables")
T.check(xp and type(xp.fn) == "string", "functions encoded as strings")
T.check(#where(evs, function(e) return e.ev == "cmd_result" and e.ok == true end) == 1, "cmd_result ok is a bool")
T.check(#where(evs, function(e) return e.ev == "kit_verified" and e.mod == "HSMPLoadout" and e.armour_n == 7 end) == 2,
        "second module writes the same file")
for _, md in ipairs { "HSMPMatch", "HSMPLoadout" } do
    local last, ok = -1, true
    for _, e in ipairs(evs) do if e.mod == md then if e.seq <= last then ok = false end last = e.seq end end
    T.check(ok, "seq strictly increasing for " .. md)
end
local lastw, wok, firstw = -1, true, nil
for _, e in ipairs(where(evs, function(e) return e.mod == "HSMPMatch" end)) do
    firstw = firstw or e.wall_ms
    if e.wall_ms < lastw then wok = false end
    lastw = e.wall_ms
end
T.check(wok, "wall_ms non-decreasing")
T.check(firstw and math.abs(firstw - (EPOCH + 1) * 1000) <= 1000, "wall_ms anchored on os.time", firstw)
local function echoed_lines(mod)
    local n = 0
    for _, p in ipairs(PRINTED) do
        if p:sub(1, 11) == "[hsmp_ev] {" and (not mod or p:find('"mod":"' .. mod .. '"', 1, true)) then n = n + 1 end
    end
    return n
end
T.check(echoed_lines() == 0, "default: NOTHING echoed to UE4SS.log (file only)", #PRINTED)
T.check(H1.echo() == false and H2.echo() == false, "echo() reports off by default")

-- --- the HSMP_LOG_ECHO flag -------------------------------------------------------------
do
    local ed = T.tmpdir("hsmp_log_echo_")
    ENV.HSMP_STATE_DIR = ed
    for _, case in ipairs {
        { env = "1", want = true }, { env = "true", want = true }, { env = " ON ", want = true },
        { env = "0", want = false }, { env = "", want = false }, { env = "nope", want = false }, { env = nil, want = false },
    } do
        ENV.HSMP_LOG_ECHO = case.env
        local E = dofile(LOG)
        local before = #PRINTED
        E.init("EchoMod" .. tostring(case.env))
        E.ready_report(1)
        local got = #PRINTED > before
        T.check(got == case.want and E.echo() == case.want,
            "HSMP_LOG_ECHO=" .. tostring(case.env) .. " -> echo " .. tostring(case.want), #PRINTED - before)
    end
    ENV.HSMP_LOG_ECHO = "1"
    local E = dofile(LOG)
    E.init("EchoOff", { echo = false })
    local before = #PRINTED
    E.ready_report(2)
    T.check(#PRINTED == before, "init{echo=false} wins over HSMP_LOG_ECHO=1")
    ENV.HSMP_LOG_ECHO = nil
    local F = dofile(LOG)
    F.init("EchoOn", { echo = true })
    before = #PRINTED
    F.ready_report(3)
    T.check(#PRINTED == before + 1 and PRINTED[#PRINTED]:sub(1, 11) == "[hsmp_ev] {", "init{echo=true} echoes [hsmp_ev] lines")
    local fl = lines(ed .. "/hsmp_events.jsonl")
    T.check(#where(fl, function(e) return e.ev == "ready_report" end) == 9, "the JSONL file gets every event whatever the echo", #fl)
    T.check(F.echo_from_env("1") and not F.echo_from_env("0") and not F.echo_from_env(nil), "echo_from_env")
    ENV.HSMP_STATE_DIR = sd
end

-- --- hitch detector --------------------------------------------------------------------
local before = #lines(path)
H1.frame(100)
CLK[1] = CLK[1] + 0.1; H1.frame(100)    -- on time
CLK[1] = CLK[1] + 3.1; H1.frame(100)    -- 3000 ms late
local all = lines(path)
local hs = {}
for i = before + 1, #all do if all[i].ev == "hitch" then hs[#hs + 1] = all[i] end end
T.check(#hs == 1 and hs[1].ms >= 2900 and hs[1].ms <= 3100, "hitch detector fires once on a 3 s gap", T.repr(hs))
T.check(hs[1] and hs[1].travel == true, "hitch within 30 s of a travel is marked travel")
CLK[1] = CLK[1] + 40; H1.frame(100)
all = lines(path)
local lasth
for i = #all, 1, -1 do if all[i].ev == "hitch" then lasth = all[i]; break end end
T.check(lasth and lasth.travel == false and lasth.ms > 39000, "hitch long after travel is not marked travel", T.repr(lasth))
T.check(lasth and lasth.world_key == "none", "hitch always carries world_key (none when unknown)", T.repr(lasth))

-- --- hitch contract: > 2 s only, world_key, travel flag, frame_hb every 10 s ------------
do
    local hd = T.tmpdir("hsmp_log_hb_")
    local HB = dofile(LOG)
    HB.init { mod = "HSMPMatch", state_dir = hd, echo = false }
    local p = hd .. "/hsmp_events.jsonl"
    HB.frame { expected_ms = 250, world_key = "W#1" }
    for _ = 1, 39 do CLK[1] = CLK[1] + 0.25; HB.frame { expected_ms = 250, world_key = "W#1" } end   -- 10 s of 250 ms ticks
    CLK[1] = CLK[1] + 1.9; HB.frame { expected_ms = 250, world_key = "W#1" }                       -- 1.65 s late: no hitch
    CLK[1] = CLK[1] + 2.6; HB.frame { expected_ms = 250, world_key = "W#1", travel = false }       -- 2.35 s late
    CLK[1] = CLK[1] + 5.0; HB.frame(250, true, "W#2")
    for _ = 1, 40 do CLK[1] = CLK[1] + 0.25; HB.frame(250, nil, "W#2") end
    local ev = lines(p)
    local hits = where(ev, function(e) return e.ev == "hitch" end)
    local hbs = where(ev, function(e) return e.ev == "frame_hb" end)
    T.check(#hits == 2, "only gaps > 2 s are hitches", T.repr(hits))
    T.check(hits[1] and hits[1].world_key == "W#1" and hits[1].travel == false and hits[1].ms >= 2300 and hits[1].ms <= 2400,
        "hitch{ms, world_key, travel=false}", T.repr(hits[1]))
    T.check(hits[2] and hits[2].travel == true and hits[2].world_key == "W#2", "caller-supplied travel=true", T.repr(hits[2]))
    T.check(#hbs >= 2 and hbs[1].n >= 39 and type(hbs[1].max_ms) == "number", "frame_hb{n, max_ms} every 10 s", T.repr(hbs))
    local maxhb = 0
    for _, h in ipairs(hbs) do if h.max_ms > maxhb then maxhb = h.max_ms end end
    T.check(maxhb >= 5000, "frame_hb.max_ms carries the worst gap", maxhb)
    T.check(#where(ev, function(e) return e.ev == "_bad_event" end) == 0, "hitch/frame_hb are vocabulary")
end

-- --- number precision and envelope collisions -------------------------------------------------
do
    local ld = T.tmpdir("hsmp_log_l18_")
    local L = dofile(LOG)
    L.init { mod = "HSMPWorld", state_dir = ld, echo = false }
    L.event("x_num", { x = 1234567.89, c = 81234.125, small = 0.1, seq = 42, mod = "other" })
    local e = lines(ld .. "/hsmp_events.jsonl")
    e = e[#e]
    T.check(e.x == 1234567.89 and e.c == 81234.125 and e.small == 0.1, "non-integers round-trip (no %.6g truncation)", T.repr(e))
    T.check(e.f_seq == 42 and e.f_mod == "other" and e.mod == "HSMPWorld" and type(e.seq) == "number" and e.seq ~= 42,
        "caller seq/mod kept as f_seq/f_mod; envelope intact", T.repr(e))
end

-- --- rotation ------------------------------------------------------------------------------
local rd = T.tmpdir("hsmp_log_rot_")
ENV.HSMP_STATE_DIR, ENV.HSMP_INST = rd, "1"
local A, B = dofile(LOG), dofile(LOG)
A.init("ModA", { max_bytes = 4000, keep = 3, echo = false })
B.init("ModB", { max_bytes = 4000, keep = 3, echo = false })
local N = 400
for i = 0, N - 1 do (i % 2 == 0 and A or B).event("x_fill", { i = i }) end
T.check(not T.exists(rd .. "/hsmp_events.4.jsonl"), "no file beyond keep=3")
local got, rotated, torn_all = {}, false, 0
for _, f in ipairs { "hsmp_events.3.jsonl", "hsmp_events.2.jsonl", "hsmp_events.1.jsonl", "hsmp_events.jsonl" } do
    local p = rd .. "/" .. f
    if T.exists(p) then
        T.check(#(T.read(p)) <= 4400, f .. " size bounded")
        local l, t = lines(p)
        torn_all = torn_all + t
        for _, e in ipairs(l) do
            if e.ev == "x_fill" then got[#got + 1] = e.i end
            if e.ev == "_rotated" then rotated = true end
        end
    end
end
local inorder = true
for i = 2, #got do if got[i] <= got[i - 1] then inorder = false end end
T.check(torn_all == 0, "no torn lines across rotation")
T.check(inorder and got[#got] == N - 1, "rotation keeps the newest lines in order")
T.check(rotated, "_rotated marker written")

-- --- hsmp_cfg -------------------------------------------------------------------------------
local cd = T.tmpdir("hsmp_cfg_")
local cfgp = cd .. "/hsmp.cfg"
T.write(cfgp, '# comment\n; other\n\nbin_dir = "C:\\Games\\HSMP\\bin\\"   # trailing\n' ..
              'master_url=https://master.example.org:8443/  \nfuture_key = 42\n')
for k in pairs(ENV) do ENV[k] = nil end
ENV.HSMP_CFG = cfgp
local C = dofile(CFG)
T.check(C.source() == cfgp, "cfg source", C.source())
T.check(C.bin_dir == "C:/Games/HSMP/bin", "bin_dir plain field", C.bin_dir)
T.check(C.get("bin_dir") == C.bin_dir and C.load().bin_dir == C.bin_dir, "bin_dir: get / load()[key] / field agree")
T.check(C.server_exe() == "C:/Games/HSMP/bin/hsmp-server.exe", "server_exe", C.server_exe())
T.check(C.sidecar_exe() == "C:/Games/HSMP/bin/hsmp-sidecar.exe", "sidecar_exe")
T.check(C.master_url == "https://master.example.org:8443", "master_url plain field", C.master_url)
T.check(C.get("master_url") == C.master_url and C.load().master_url == C.master_url, "master_url: all forms agree")
T.check(C.get("future_key") == "42", "unknown keys kept")
T.check(C.state_dir() == "hsmp_state" and C.inst() == "0" and C.dev() == false, "state_dir/inst/dev defaults")
ENV.HSMP_MASTER_URL, ENV.HSMP_SERVER_EXE, ENV.HSMP_DEV = "http://10.0.0.5:7778", "D:\\x\\srv.exe", "1"
C._reset()
T.check(C.master_url == "http://10.0.0.5:7778", "HSMP_MASTER_URL override")
T.check(C.server_exe() == "D:/x/srv.exe", "legacy HSMP_SERVER_EXE override")
T.check(C.dev() == true, "HSMP_DEV=1")
ENV.HSMP_MASTER_URL = "http://evil&calc.exe"
C._reset()
T.check(C.master_url == "http://127.0.0.1:7778" and C.get("master_url") == C.master_url, "unsafe URL rejected -> default")
ENV.HSMP_CFG = cd .. "/missing.cfg"
ENV.HSMP_MASTER_URL, ENV.HSMP_SERVER_EXE, ENV.HSMP_DEV = nil, nil, nil
if not T.exists("hsmp.cfg") and not T.exists("ue4ss/hsmp.cfg") then
    local D = dofile(CFG)
    T.check(D.source() == nil, "no cfg file -> source nil")
    T.check(D.server_exe() == "hsmp/hsmp-server.exe", "default server_exe", D.server_exe())
end
local pq = where(evs, function(e) return e.ev == "pose_quality" end)[1]
T.check(pq and pq.jitter_ratio == 1.1 and pq.foot_slide_p95 == 3.0 and pq.idle_rms == 0.2 and pq._missing == nil,
        "pose_quality carries jitter_ratio / foot_slide_p95 / idle_rms", T.repr(pq))

-- --- a locked live log must not shift (and lose) the rotation chain -------------------------
do
    local ld2 = T.tmpdir("hsmp_log_lock_")
    ENV.HSMP_STATE_DIR, ENV.HSMP_INST = ld2, "1"
    local L2 = dofile(LOG)
    L2.init("ModL", { max_bytes = 2000, keep = 3, echo = false })
    for i = 0, 199 do L2.event("x_fill", { i = i }) end   -- several rotations: .1 .. .3 exist
    local before = {}
    for k = 1, 3 do before[k] = T.read(ld2 .. "/hsmp_events." .. k .. ".jsonl") end
    T.check(before[1] and before[2] and before[3], " setupa full rotation chain")
    local live = ld2 .. "/hsmp_events.jsonl"
    local real_rename, real_clock = os.rename, os.clock
    local t0 = real_clock()
    os.clock = function() return t0 end
    os.rename = function(a, b)
        if a:gsub("\\", "/") == live:gsub("\\", "/") then return nil, "locked" end   -- the live file is held open elsewhere
        return real_rename(a, b)
    end
    for i = 200, 260 do L2.event("x_fill", { i = i }) end
    local same = true
    for k = 1, 3 do if T.read(ld2 .. "/hsmp_events." .. k .. ".jsonl") ~= before[k] then same = false end end
    T.check(same, "the chain is untouched while the live log cannot be moved (no history lost)")
    local l = T.read(live) or ""
    T.check(l:find('"i":260', 1, true) ~= nil, "events keep being appended to the live log")
    os.rename = real_rename
    os.clock = function() return t0 + 31 end   -- past the 30 s backoff
    L2.event("x_fill", { i = 261 })
    T.check(T.read(ld2 .. "/hsmp_events.1.jsonl") ~= before[1], "rotation resumes once the file can be moved")
    os.clock = real_clock
end
