-- World guard (shared/hsmp_wg.lua) + HSMPMatch / HSMPNoCutscene smoke tests.
--
--   hsmp-tools lua-test world_guard
--
-- 1. hsmp_wg.lua semantics, identical to the per-mod blocks it replaces:
--    first sight of a world drops + skips, travel holds until the world swaps,
--    hold timeout, no-world drop, token/same, handler isolation, hooks.
-- 2. HSMPMatch/Scripts/main.lua boots under umg_mock with the shared libs
--    found in the source tree, registers its hooks, runs the Director, travels
--    only through the Director, rewrites a native Tavern travel in its
--    OpenLevel pre-hook, and touches nothing after the world is freed.
-- 3. HSMPNoCutscene: the Play hook is a synchronous POST hook (no deferred
--    capture), ignored while a level change is pending.

-- ---------------------------------------------------------------------------
T.log("== hsmp_wg.lua semantics")
do
    local HW = dofile(T.path("mods/shared/hsmp_wg.lua"))
    local clk = 0
    local world = { name = "World /Game/Maps/A.A", addr = 1, valid = true }
    local pc = { name = "PlayerController_0", valid = true }
    local function obj(t, kind)
        return {
            IsValid = function() return t.valid end,
            GetFullName = function() return t.name end,
            GetAddress = function() return t.addr end,
            GetFName = function() return { ToString = function() return t.name end } end,
            GetWorld = function() return kind == "pc" and obj(world, "world") or nil end,
        }
    end
    local UEH = { GetPlayerController = function() return obj(pc, "pc") end, GetWorld = function() return obj(world, "world") end }
    local logs, drops = {}, {}
    local hooks = {}
    _G.RegisterHook = function(path, fn) hooks[#hooks + 1] = { path = path, fn = fn } end
    local WG = HW.new({ log = function(f, ...) logs[#logs + 1] = string.format(f, ...) end,
        UEHelpers = UEH, clock = function() return clk end })
    T.check(#hooks == 2 and hooks[1].path:match("OpenLevel$") and hooks[2].path:match("OpenLevelBySoftObjectPtr$"),
        "pre-hooks on OpenLevel + OpenLevelBySoftObjectPtr installed by default")
    WG.on_drop(function(why) drops[#drops + 1] = why end, "test cache")
    WG.cache("throws", function() error("boom") end)
    T.check(WG.check() == false and drops[1] == "world changed", "first sight of a world: drop + skip this tick")
    T.check(WG.check() == true, "same world: caches valid")
    T.check(WG.key == "World /Game/Maps/A.A@1#PlayerController_0" and WG.short() == "A", "key = name@addr#pc; short name", WG.key)
    local tok = WG.token()
    T.check(WG.same(tok), "token same in the same world")
    -- level change requested (pre-hook): hold while the old world is still up
    hooks[1].fn()
    T.check(WG.pending() and #drops == 2 and drops[2] == "level change requested", "travel drops caches at once")
    T.check(not WG.same(tok), "a token from before the travel is stale")
    T.check(WG.check() == false, "old world still up: hold")
    clk = 5
    T.check(WG.check() == false, "still holding at 5 s")
    -- same arena reloaded: same name + address, new PC
    pc.name = "PlayerController_1"
    T.check(WG.check() == false and drops[3] == "world changed" and not WG.pending(), "new world (new PC name): drop + skip")
    T.check(WG.check() == true, "then valid")
    -- hold timeout: a travel that never happens
    hooks[2].fn(); clk = 20
    T.check(WG.check() == false, "after the 10 s hold the old key counts as a new world once")
    T.check(WG.check() == true, "and is then valid again")
    -- no world
    world.valid = false
    local n = #drops
    T.check(WG.check() == false and drops[n + 1] == "no valid world" and WG.key == nil, "no valid world: drop once")
    T.check(WG.check() == false and #drops == n + 1, "no repeated drops while there is no world")
    T.check(#logs >= 4 and logs[1]:match("^world guard: world changed %-> object caches dropped %(#1%)"), "same log line as the old blocks", logs[1])
    T.check(WG.drop_names[1] == "test cache" and WG.drop_names[2] == "throws", "named caches recorded")
    local WG2 = HW.new({ hooks = false, UEHelpers = UEH, log = function() end })
    T.check(#hooks == 2 and WG2 ~= WG, "hooks=false installs nothing; instances are independent")
end

T.log("== hsmp_wg.lua WG.pc(): one PlayerController lookup per game frame")
do
    local HW = dofile(T.path("mods/shared/hsmp_wg.lua"))
    local fr, n = 1, 0
    local world = { valid = true }
    local pcs = {}
    local function mkpc(name)
        local o = { name = name, valid = true }
        o.IsValid = function() return o.valid end
        o.GetFName = function() return { ToString = function() return o.name end } end
        o.GetWorld = function() return { IsValid = function() return world.valid end,
            GetFullName = function() return "World /Game/Maps/A.A" end, GetAddress = function() return 7 end } end
        return o
    end
    local cur = mkpc("PlayerController_0")
    local UEH = { GetPlayerController = function() n = n + 1; pcs[#pcs + 1] = cur; return cur end,
                  GetWorld = function() return nil end }
    local WG = HW.new({ hooks = false, UEHelpers = UEH, log = function() end, frame = function() return fr end })
    T.check(WG.check() == false, "first sight of the world: drop + skip (as before)")
    fr, n = 2, 0
    local pc = WG.pc()
    T.check(pc == cur and n == 1, "first call of a frame looks the PlayerController up")
    WG.check(); WG.check(WG.pc()); WG.pc(); WG.world_key(); WG.same(WG.token())
    T.check(n == 1, "every guard call of the same frame shares that one lookup", n)
    T.check(WG.check() == true and n == 1, "the guard accepts it (same world)", n)
    fr = 3
    WG.check(); WG.pc(); WG.check()
    T.check(n == 2, "next frame: exactly one new lookup", n)
    -- a level change requested in this frame: the cache is dropped untouched
    local old = cur
    WG.travel("test travel")
    cur = mkpc("PlayerController_1")
    old.IsValid = function() error("freed PlayerController touched") end
    T.check(WG.pc() == cur, "after a guard drop the same frame looks up afresh (never the old PC)")
    fr = 4
    local ok = pcall(WG.check)
    T.check(ok and WG.pc() == cur, "the old world's PC is never touched again")
    T.check(WG.pc_lookups == #pcs, "pc_lookups counts the real lookups", WG.pc_lookups)
end

T.log("== hsmp_wg.lua WG.pc(): keyed on the engine frame counter, not an os.clock ms")
do
    local HW = dofile(T.path("mods/shared/hsmp_wg.lua"))
    local fc, n = 100, 0
    local ksl = { GetFrameCount = function() return fc end }
    local pc = { IsValid = function() return true end }
    local UEH = { GetPlayerController = function() n = n + 1; return pc end,
                  GetKismetSystemLibrary = function() return ksl end, GetWorld = function() return nil end }
    local WG = HW.new({ hooks = false, UEHelpers = UEH, log = function() end })
    local real_clock = os.clock
    local c = 50.0
    os.clock = function() return c end
    WG.pc(); c = c + 0.004; WG.pc(); c = c + 0.007; WG.pc()
    T.check(n == 1, "three loops in different milliseconds of ONE engine frame share one lookup", n)
    fc = 101
    WG.pc()
    T.check(n == 2, "the next engine frame (same ms or not) looks up afresh", n)
    -- no frame counter (call fails): the os.clock ms fallback
    ksl.GetFrameCount = function() error("no CDO") end
    c = 60.0; WG.pc(); WG.pc()
    T.check(n == 3, "fallback: one lookup per os.clock millisecond", n)
    c = 60.002; WG.pc()
    T.check(n == 4, "fallback: a new millisecond looks up afresh", n)
    os.clock = real_clock
end

T.log("== hsmp_wg.lua WG.settled(): the shared world-settle gate")
do
    local HW = dofile(T.path("mods/shared/hsmp_wg.lua"))
    local clk, fr = 0, 0
    local pcname = "PlayerController_0"
    local w = { IsValid = function() return true end, GetFullName = function() return "World /Game/Maps/A.A" end,
                GetAddress = function() return 7 end }
    local pc = { IsValid = function() return true end, GetWorld = function() return w end,
                 GetFName = function() return { ToString = function() return pcname end } end }
    local UEH = { GetPlayerController = function() return pc end, GetWorld = function() return w end }
    local WG = HW.new({ hooks = false, UEHelpers = UEH, log = function() end,
                        clock = function() return clk end, frame = function() fr = fr + 1; return fr end })
    T.check(HW.SETTLE_S == 2.0 and not WG.settled(), "no world seen yet: not settled")
    WG.check(); clk = 0.1; WG.check()
    T.check(not WG.settled(), "0.1 s into a world: not settled")
    clk = 1.9; WG.check()
    T.check(not WG.settled(), "1.9 s: not settled (the crash window of 48e2d8e)")
    clk = 2.05; WG.check()
    T.check(WG.settled() and not WG.settled(5), "2 s: settled; a longer custom window is not")
    -- round reset: the same arena reloads (new PC name) -> unsettled again
    WG.travel("reset"); T.check(not WG.settled(), "a requested level change unsettles at once")
    pcname = "PlayerController_1"; clk = 3; WG.check(); WG.check()
    T.check(not WG.settled(), "the reloaded world starts a new settle window")
    clk = 5.1; WG.check()
    T.check(WG.settled(), "and settles 2 s after its key changed")
    -- the standalone tracker (mods with their own world guard)
    local tc = 0
    local st = HW.settle_tracker(function() return tc end)
    T.check(not st.settled(), "tracker: nothing noted -> not settled")
    st.note("k1"); tc = 1.5; st.note("k1")
    T.check(not st.settled(), "tracker: 1.5 s -> not settled")
    tc = 2.0; T.check(st.settled(), "tracker: 2 s -> settled")
    st.note("k2"); T.check(not st.settled(), "tracker: a new key restarts the window")
    st.note(nil); tc = 10; T.check(not st.settled(), "tracker: no world is never settled")
end

-- ---------------------------------------------------------------------------
-- umg_mock based smoke tests

local M = require("umg_mock")

local NM = require("hsmp_native_mock")
local S = NM.S

local function setup(sd, la)
    M.loops, M.delayed, M.logs, M.objs, M.openlevel, M.dead_touch = {}, {}, {}, {}, {}, {}
    -- the session side: the typed native mock (link / session records, heartbeat)
    M.N = NM.new{}
    M.N._st.sidecar_state, M.N._st.hb_age = "absent", nil
    _G.HSMPNative = M.N
    package.loaded["UEHelpers"] = nil
    M.install({ state_dir = sd, env = { LOCALAPPDATA = la, HSMP_INST = "7", HSMP_ALLOW_CUTSCENES = false } })
    local Methods = M.Methods
    local fullname = "World /Game/Maps/Map_Menu_Startup.Map_Menu_Startup"
    M.set_world = function(n) fullname = n end
    Methods.GetFullName = function(self)
        if rawget(self, "__cls") == "World" then return fullname end
        return "Obj " .. tostring(rawget(self, "__name"))
    end
    Methods.GetAddress = function(self) return 4096 end
    Methods.GetWorld = function(self) return M.world end
    M.hooks = {}
    _G.RegisterHook = function(path, pre, post)
        M.hooks[path] = M.hooks[path] or {}
        table.insert(M.hooks[path], { pre = pre, post = post })
        return 1, 2
    end
    M.fire = function(path, ...)
        for _, h in ipairs(M.hooks[path] or {}) do if h.pre then h.pre(...) end end
    end
    M.fire_post = function(path, ...)
        for _, h in ipairs(M.hooks[path] or {}) do if h.post then h.post(...) end end
    end
    -- OpenLevel goes through the hooks like ProcessEvent does
    Methods.OpenLevel = function(self, world, name, abs, opts)
        local cur = name
        local param = { get = function() return { ToString = function() return cur end } end,
                        set = function(_, v) cur = v end }
        M.fire("/Script/Engine.GameplayStatics:OpenLevel", {}, {}, param, {}, {})
        table.insert(M.openlevel, cur)
    end
    Methods.DoesSaveGameExist = function(self, slot) return false end
    _G.FindAllOf = function() return nil end
end

-- The sidecar is attached and beating; its link record says `status`.
local function beat(sd, n, status)
    local N = M.N
    N._st.sidecar_state, N._st.hb_age = "ready", 0.01
    status = status or "connected"
    if M.link_status ~= status then
        M.link_status = status
        N.sc_put("link", { status = S.ENUMS.sidecar_status[status:upper()], state = S.ENUMS.link_state.UP,
            my_peer_id = 1, is_admin = true })
    end
end
-- The server's session snapshot (peers 1 "Me" and 2 "B"; `dead`: peer ids).
local function snapshot(phase, round, dead)
    M.seq = (M.seq or 0) + 1
    local d = {}
    for _, id in ipairs(dead or {}) do d[id] = true end
    M.N.sc_put("session", { epoch = 1, seq = M.seq, round = round, phase = S.ENUMS.phase[phase], winner_seat = 255,
        phase_deadline_ms = 13000, server_time_ms = 10000, has_frozen = phase ~= "LOBBY",
        config = { arena = "Map_Arena_Pit", best_of = 3, countdown_s = 3 }, frozen = { arena = "Map_Arena_Pit", best_of = 3, countdown_s = 3 },
        rows = { { seat = 1, peer_id = 1, connected = true, alive = not d[1], nick = "Me" },
                 { seat = 2, peer_id = 2, connected = true, alive = not d[2], nick = "B" } } })
end

T.log("== HSMPMatch main.lua boots and travels only through the Director")
do
    local sd, la = T.tmpdir("hsmp_dir_sd_"), T.tmpdir("hsmp_dir_la_")
    setup(sd, la)
    dofile(T.path("mods/HSMPMatch/Scripts/main.lua"))
    local log = M.logtext()
    T.check(T.contains(log, "Director v1 up"), "Director constructed", log)
    T.check(T.contains(log, "OpenLevel hook registered"), "OpenLevel pre-hook registered")
    T.check(T.contains(log, "save guard: shared/hsmp_saveguard.lua installed"), "save guard installed from shared/")
    T.check(M.hooks["/Script/Engine.GameplayStatics:OpenLevel"] and #M.hooks["/Script/Engine.GameplayStatics:OpenLevel"] >= 2,
        "world guard + Director both hook OpenLevel")
    T.check(M.hooks["/Script/Engine.GameplayStatics:SaveGameToSlot"] ~= nil, "save slot redirect hooks present")
    M.run(1600)
    M.link_status = nil
    local hb = M.N.sc_get("director") or {}
    T.check(hb.state == "Menu" and hb.world == "Map_Menu_Startup", "heartbeat in the menu", T.repr(hb))
    T.check(T.contains(M.logtext(), "saveguard selftest:"), "save guard selftest ran once in the menu")
    -- a session starts and the server opens the countdown
    for i = 1, 6 do beat(sd, i); M.run(250) end
    snapshot("COUNTDOWN", 0)
    for i = 7, 10 do beat(sd, i); M.run(250) end
    T.check(M.openlevel[1] == "Map_Arena_Pit" and #M.openlevel == 1, "one OpenLevel, to the server arena", T.repr(M.openlevel))
    T.check(M.gi.__props["Free Mode Foes Amount"] == 1 and M.gi.__props["Player Just Died"] == false,
        "GI profile written on the real GameInstance object")
    T.check(T.contains(M.logtext(), "director: travel Map_Menu_Startup -> Map_Arena_Pit"), "travel log line")
    -- a native Tavern travel while the match runs is rewritten by the pre-hook
    local cur = "Map_Hub_Tavern_Frank"
    local param = { get = function() return { ToString = function() return cur end } end, set = function(_, v) cur = v end }
    M.fire("/Script/Engine.GameplayStatics:OpenLevel", {}, {}, param, {}, {})
    T.check(cur == "Map_Arena_Pit", "pre-hook rewrote the Tavern target to the server arena", cur)
    T.check(T.contains(M.logtext(), "native OpenLevel('Map_Hub_Tavern_Frank') rewritten"), "rewrite logged")
    T.check(T.read(sd .. "/.level_change.flag") == nil, "no .level_change.flag (the pre-hook reports to IPC.world_leaving)")
    -- the arena loads: the pipeline runs on the real (mock) objects
    M.set_world("World /Game/Maps/Arenas/Map_Arena_Pit/Map_Arena_Pit.Map_Arena_Pit")
    local pawn = M.new_obj("Willie_BP_C", "Willie_BP_C_3")
    for k, v in pairs({ Health = 40, ["Neck Health"] = 25, Consciousness = 100 }) do pawn.__props[k] = v end
    M.Methods.K2_GetActorLocation = function() return { X = 0, Y = 0, Z = 0 } end
    M.Methods.DisableInput = function(self) rawset(self, "input_off", true) end
    M.Methods.EnableInput = function(self) rawset(self, "input_off", false) end
    M.pc.__props.Pawn = pawn
    for i = 11, 22 do beat(sd, i); M.run(250) end
    T.check(T.contains(M.logtext(), "director: world ready Map_Arena_Pit"), "world_ready in the arena", M.logtext())
    T.check(rawget(pawn, "input_off") == true, "the pawn's input is frozen during the countdown")
    T.check(T.contains(M.logtext(), "MP session ACTIVE"), "the MP guard (hsmp_session) sees the live session", M.logtext())
    T.check(not T.contains(M.logtext(), "tick error"), "no tick errors in the arena", M.logtext())
    -- PlayerController walks (UEHelpers GetPlayerController / GetWorld)
    -- in an MP arena: at most one per game frame (the 50 ms slow-mo guard and
    -- the 250 ms tick: 24 callback frames per second)
    do
        local UEH = require("UEHelpers")
        local gp, gw, walks = UEH.GetPlayerController, UEH.GetWorld, 0
        UEH.GetPlayerController = function() walks = walks + 1; return gp() end
        UEH.GetWorld = function() walks = walks + 1; return gw() end
        for i = 1, 4 do beat(sd, 50 + i); M.run(250) end
        UEH.GetPlayerController, UEH.GetWorld = gp, gw
        T.check(walks <= 24, "HSMPMatch: <= one PlayerController walk per callback frame (WG.pc)", walks)
    end
    -- Dead -> spectate a living opponent's stand-in; alive again -> the
    -- camera goes back to the own pawn
    local standin = M.new_obj("Willie_BP_C", "Willie_BP_C_9")
    local real_fao = _G.FindAllOf
    -- the native in-game HUD, whose "Black" image fades in over a dead player's view
    local native_hud = M.new_obj("UI_HUD_C", "UI_HUD_C_0")
    rawset(native_hud, "vis", 4)
    local black = M.new_obj("Image", "Black")
    rawset(black, "vis", 4)
    native_hud.__props.Black = black
    M.Methods.IsInViewport = function() return true end
    M.Methods.GetVisibility = function(self) return rawget(self, "vis") or 0 end
    _G.FindAllOf = function(c)
        if c == "Willie_BP_C" then return { standin } end
        if c == "UI_HUD_C" then return { native_hud } end
        return nil
    end
    M.Methods.SetViewTargetWithBlend = function(self, t) M.view_target = t end
    -- our spectator camera: a CameraActor spawned in this world, moved every frame
    rawset(standin, "K2_GetActorLocation", function() return { X = 400, Y = 0, Z = 90 } end)
    local real_sfo = _G.StaticFindObject
    _G.StaticFindObject = function(p) if p == "/Script/Engine.CameraActor" then return M.new_obj("Class", "CameraActor") end return real_sfo(p) end
    M.Methods.BeginDeferredActorSpawnFromClass = function()
        local a = M.new_obj("CameraActor", "CameraActor_" .. (#M.objs))
        a.__props.CameraComponent = M.new_obj("CameraComponent", "CameraComponent")
        M.cams = M.cams or {}
        M.cams[#M.cams + 1] = a
        return a
    end
    M.Methods.FinishSpawningActor = function() end
    M.Methods.K2_SetActorLocationAndRotation = function(self, l) rawset(self, "at", l) end
    M.N.bus_put("puppets", { rows = { { peer = 2, name = "Willie_BP_C_9" } } })   -- HSMPAvatars' bus key (its own contract)
    snapshot("LIVE", 1, { 1 })
    pawn.__props.Health = 0
    for i = 1, 4 do beat(sd, 100 + i); M.run(250) end
    local cam = M.cams and M.cams[1]
    T.check(cam and M.view_target == cam and #M.cams == 1 and T.contains(M.logtext(), "spectating B"),
        "dead: our camera actor follows the opponent's stand-in (never the stand-in's own camera)", M.logtext())
    local at = cam and rawget(cam, "at")
    T.check(at and math.abs(math.sqrt((at.X - 400) ^ 2 + at.Y ^ 2) - 320) < 40 and at.Z > 90, "placed behind and above the stand-in", T.repr(at))
    T.check(rawget(black, "vis") == 1 and rawget(native_hud, "vis") == 4,
        "spectating: the native HUD's Black image is collapsed, the HUD itself (HSMPHud's host) stays", tostring(rawget(black, "vis")))
    local sp = M.N.sc_get("spectate")
    T.check(sp and sp.target == 2 and sp.nick == "B" and sp.round == 1, "bus spectate {target, nick, round}", T.repr(sp))
    snapshot("LIVE", 1)
    pawn.__props.Health = 100
    for i = 1, 4 do beat(sd, 110 + i); M.run(250) end
    T.check(M.view_target == pawn, "alive again: the camera is handed back to the own pawn", tostring(M.view_target))
    T.check(rawget(black, "vis") == 4, "alive again: the Black image is restored as it was", tostring(rawget(black, "vis")))
    T.check(M.N.sc_get("spectate").target == 0, "bus spectate cleared (target 0)")
    _G.FindAllOf = real_fao
    _G.StaticFindObject = real_sfo
    -- the old world is freed: nothing may touch it any more
    M.kill_all()
    for i = 23, 30 do beat(sd, i); M.run(250) end
    local bad = T.filter(M.dead_touch, function(x) return not x:match("%.IsValid$") end)
    T.check(#bad == 0, "no freed UObject touched after the level change", T.repr(bad))
    T.check(not T.contains(M.logtext(), "tick error"), "no tick errors", M.logtext())
end

T.log("== HSMPMatch's MP guard never treats a dead sidecar's 'connected' link as a session")
do
    local sd, la = T.tmpdir("hsmp_dir_sd2_"), T.tmpdir("hsmp_dir_la2_")
    setup(sd, la)
    M.link_status = nil
    beat(sd, 7)   -- a crashed sidecar's last link record; its heartbeat stopped long ago
    M.N._st.hb_age = 1e6
    M.set_world("World /Game/Maps/Arenas/Map_Arena_Pit/Map_Arena_Pit.Map_Arena_Pit")
    local pawn = M.new_obj("Willie_BP_C", "Willie_BP_C_3")
    pawn.__props.Health = 100
    M.Methods.K2_GetActorLocation = function() return { X = 0, Y = 0, Z = 0 } end
    M.pc.__props.Pawn = pawn
    dofile(T.path("mods/HSMPMatch/Scripts/main.lua"))
    M.run(20000)
    T.check(not T.contains(M.logtext(), "MP session ACTIVE"), "single-player with a dead sidecar: the guard stays inactive", M.logtext())
end

T.log("== a slow-mo BP hook that fails to register is retried, not marked done")
do
    local sd, la = T.tmpdir("hsmp_dir_sd3_"), T.tmpdir("hsmp_dir_la3_")
    setup(sd, la)
    local sfo = _G.StaticFindObject
    _G.StaticFindObject = function(path)
        if path:find("Willie_BP_C:", 1, true) then return M.new_obj("Function", path) end
        return sfo(path)
    end
    local fails = 2
    local reg = _G.RegisterHook
    _G.RegisterHook = function(path, pre, post)
        if path:find("Willie_BP_C:", 1, true) and fails > 0 then
            if path:find("InpActEvt_SloMo", 1, true) then fails = fails - 1 end
            error("hook not ready")
        end
        return reg(path, pre, post)
    end
    dofile(T.path("mods/HSMPMatch/Scripts/main.lua"))
    M.run(5000)
    local log = M.logtext()
    T.check(T.contains(log, "slow-mo hook FAILED (try 1, retrying)"), "a failed registration is logged as a retry", log)
    T.check(T.contains(log, "slow-mo hook registered: /Game/Character/Blueprints/Willie_BP.Willie_BP_C:InpActEvt_SloMo"),
        "and registered on a later try (was: marked done after the failure)", log)
    _G.StaticFindObject, _G.RegisterHook = sfo, reg
end

T.log("== HSMPNoCutscene: synchronous post hook, guarded")
do
    local sd, la = T.tmpdir("hsmp_nc_sd_"), T.tmpdir("hsmp_nc_la_")
    setup(sd, la)
    M.set_world("World /Game/Maps/Arenas/Map_Arena_Pit.Map_Arena_Pit")
    dofile(T.path("mods/HSMPNoCutscene/Scripts/main.lua"))
    T.check(T.contains(M.logtext(), "Play hook registered (post, synchronous)"), "post hook registered", M.logtext())
    local h = M.hooks["/Script/MovieScene.MovieSceneSequencePlayer:Play"]
    T.check(h and h[1].post ~= nil, "Play hook has a post callback")
    M.run(1100)    -- poll: guard sees the world

    local stopped = {}
    local function seq(name)
        return { IsValid = function() return true end, IsPlaying = function() return true end,
                 GetFullName = function() return name end, GoToEndAndStop = function() stopped[#stopped + 1] = name end }
    end
    M.fire_post("/Script/MovieScene.MovieSceneSequencePlayer:Play", { get = function() return seq("Intro") end })
    T.check(#stopped == 0 and T.contains(M.logtext(), "allowing level sequence (started during load grace): Intro"),
        "during the load grace the level's own sequence is allowed")
    -- after the grace, an idle cutscene is stopped synchronously in the hook
    local real_time = os.time
    os.time = function() return real_time() + 60 end
    M.fire_post("/Script/MovieScene.MovieSceneSequencePlayer:Play", { get = function() return seq("IdleCutscene") end })
    T.check(stopped[1] == "IdleCutscene", "idle cutscene stopped in the post hook (no deferral)", T.repr(stopped))
    -- a level change is pending: the hook does nothing
    M.fire("/Script/Engine.GameplayStatics:OpenLevel", {}, {}, { get = function() return { ToString = function() return "X" end } end }, {}, {})
    M.fire_post("/Script/MovieScene.MovieSceneSequencePlayer:Play", { get = function() return seq("DuringTravel") end })
    T.check(#stopped == 1, "ignored while a level change is pending")
    os.time = real_time
    T.check(#M.delayed == 0 and next(M.delayed) == nil, "nothing deferred")
end
