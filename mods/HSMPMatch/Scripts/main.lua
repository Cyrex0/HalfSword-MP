-- HSMPMatch — client side of the MP round loop. The server owns the match
-- (state machine + load barrier, server.rs `match_step`); this mod makes the
-- local game follow it and tells the server what this game is doing.
--
--   * director.lua (the Director): the only code in the HSMP mod tree that
--     changes the level. It follows the server's session state
--     (the typed session snapshot in shared memory), runs the verified
--     spawn pipeline, reports the loaded round (game_status), freezes input
--     outside Live, serves HSMPMenu's travel requests (bus travel_request ->
--     travel_ack, heartbeat bus director;
--     docs/development/subsystems/director-contract.md)
--     and rewrites every native OpenLevel while a session process exists.
--   * Keeps the native arena flow from ending the match: pins the GameMode's
--     enemy count so puppet deaths never trigger its win flow, collapses the
--     native death/win/lose widgets, stretches the lose-flow Delay.
--   * The connection state machine (director.lua step_conn): reconnecting
--     in place, one transition to the menu when the session is lost, the
--     reason in the bus key conn_state (HSMPHud draws the overlay / modal and
--     the centre banner).
--   * Spectates a living opponent's puppet after you die (auto-advance,
--     Q / E cycle), published in the bus key spectate.
--   * Slow motion fully disabled and the world never paused in MP sessions.
--   * r.HairStrands.Streaming 0 at boot and on every world change (pak read
--     crash workaround).
--
-- Session facts: the sidecar's typed records in shared memory (link: status,
-- my peer id, admin; session: phase, arena, round, roster with wins / alive /
-- spawn orders; shared/hsmp_session.lua), and from HSMPAvatars:
--   bus key puppets   typed rows {peer, name = "<Willie FName>"}: stand-ins for peers

local UEHelpers = require("UEHelpers")

-- Thread-safety shim. LoopAsync, ExecuteWithDelay and *Async keybind
-- callbacks run on UE4SS worker threads; running them alongside game-thread
-- Lua corrupts the mod's VM (crash dumps show lua_next / __index on garbage
-- userdata). Route every deferred, looped and key-bound callback onto
-- the game thread.
if LoopInGameThreadWithDelay and ExecuteInGameThreadWithDelay and CancelDelayedAction then
    local _gt = ExecuteInGameThread
    ExecuteWithDelay = function(ms, fn) return ExecuteInGameThreadWithDelay(ms, fn) end
    LoopAsync = function(ms, fn)
        local h
        h = LoopInGameThreadWithDelay(ms, function()
            local ok, stop = pcall(fn)
            if ok and stop == true and h then CancelDelayedAction(h) end
        end)
        return h
    end
    local _rkba = RegisterKeyBindAsync
    RegisterKeyBindAsync = function(key, mods, fn)
        return _rkba(key, mods, function() _gt(function() pcall(fn) end) end)
    end
    local _rkb = RegisterKeyBind
    RegisterKeyBind = function(key, a, b)
        if b then return _rkb(key, a, function() _gt(function() pcall(b) end) end) end
        return _rkb(key, function() _gt(function() pcall(a) end) end)
    end
end

local function Log(fmt, ...)
    print(string.format("[HSMPMatch] " .. fmt .. "\n", ...))
end

-- An error line repeats at the loop rate (up to 20/s) while
-- its cause lasts: at most one line per key every ERR_EVERY_S, with the count
-- of the lines held back.
local ERR_EVERY_S = 5
local _err = {}
local function log_err(key, msg)
    local now = os.clock()
    local e = _err[key]
    if not e then e = { at = -1e9, n = 0 }; _err[key] = e end
    if now - e.at < ERR_EVERY_S then e.n = e.n + 1; return end
    if e.n > 0 then Log("%s: %s (+%d more in %.0f s)", key, tostring(msg), e.n, now - e.at)
    else Log("%s: %s", key, tostring(msg)) end
    e.at, e.n = now, 0
end

-- Sibling / shared modules: require() first (build-and-deploy copies
-- shared/*.lua into Scripts/), then dofile next to this file, then the repo's
-- mods/shared/ (running straight from the source tree).
local function load_module(name)
    local ok, mod = pcall(require, name)
    if ok and type(mod) == "table" then return mod end
    local src = (debug.getinfo(1, "S").source or ""):gsub("^@", "")
    local dir = src:match("^(.*)[/\\]") or "."
    local errs = { tostring(mod) }
    for _, path in ipairs({ dir .. "/" .. name .. ".lua", dir .. "/../../shared/" .. name .. ".lua" }) do
        local ok2, mod2 = pcall(dofile, path)
        if ok2 and type(mod2) == "table" then return mod2 end
        errs[#errs + 1] = tostring(mod2)
    end
    return nil, table.concat(errs, " / ")
end

-- --- world guard (shared/hsmp_wg.lua) ------------------------------------------
-- Every UObject kept beyond one callback belongs to the world it was cached in;
-- wg_check() first in every loop callback; caches reset in wg_on_drop handlers
-- without touching the old objects. See the library header.
local HW, hw_err = load_module("hsmp_wg")
if not HW then
    Log("FATAL: shared/hsmp_wg.lua missing (%s) - HSMPMatch disabled (deploy copies shared/*.lua)", tostring(hw_err))
    return
end
local WG = HW.new({ log = Log, UEHelpers = UEHelpers })
local wg_check, wg_on_drop = WG.check, WG.on_drop

-- --- structured events (shared/hsmp_log.lua; no-op when absent) ----------------
local function trim(s) return s and s:match("^%s*(.-)%s*$") or nil end
local STATE_DIR = (trim(os.getenv("HSMP_STATE_DIR")) or "hsmp_state"):gsub("\\", "/")

local HL = load_module("hsmp_log")
local function ev(name, fields)
    if not HL then return end
    local f = HL.event or HL.emit
    if f then pcall(f, name, fields or {}) end
end
if HL and HL.init then pcall(HL.init, { mod = "HSMPMatch", state_dir = STATE_DIR }) end
Log("event log: %s", HL and "shared/hsmp_log.lua" or "not deployed (no-op stub)")

-- --- career save guard (shared/hsmp_saveguard.lua) -----------------------------
-- Installed only here (one set of save hooks per game). While a session exists
-- every SaveGameToSlot is diverted to HSMP_<inst>_<slot>; the Director seeds
-- the session slot between its GI apply and OpenLevel (never inside a hook).
-- SG.tick() runs at the top of the 250 ms loop; selftest once in the lobby.
local SG = load_module("hsmp_saveguard")
if SG and SG.install then
    local ok, err = pcall(SG.install, { mod = "HSMPMatch", state_dir = STATE_DIR, log = HL })
    if not ok then Log("save guard install FAILED: %s", tostring(err)); SG = nil end
end
Log("save guard: %s", SG and "shared/hsmp_saveguard.lua installed"
    or "NOT DEPLOYED - career saves are not diverted in MP")
local sg_selftest_done = false

-- Shared-memory IPC facade (shared/hsmp_ipc.lua). The Director is the only
-- travel owner, so its OpenLevel pre-hooks tell the IPC layer that the world
-- is being left (world_epoch + 1, game samples invalidated, world-scoped bus
-- keys cleared).
local IPC = load_module("hsmp_ipc")
if IPC then IPC.init({ mod = "HSMPMatch", state_dir = STATE_DIR, log = Log }) end

local TICK_MS          = 250

-- Native end-of-fight widgets. UI_WIN_C / UI_WinScreen_C belong here too:
-- otherwise a client killing its local stand-in gets the native win screen
-- while the other screen still fights. Only the server's result is shown.
local NATIVE_END_WIDGETS = {
    UI_DED_C = true, UI_Lose_C = true, UI_NextFIght_C = true,
    UI_GiveUp_C = true, UI_DeathDoor_C = true,
    UI_WIN_C = true, UI_WinScreen_C = true,
    UI_Tier_C = true, UI_Unlock_Item_C = true, UI_Unlock_Lock_C = true,
    UI_Cards_C = true, UI_Card_Boss_C = true,
}

-- --- the Director ------------------------------------------------------------------
local Director, d_err = load_module("director")
if not Director then
    Log("FATAL: director.lua failed to load (%s) - HSMPMatch disabled", tostring(d_err))
    return
end
-- Round-reset crash guard (docs/development/crash-rr.md): the Runtime Vertex Paint quiesce.
-- Every Director travel waits until the plugin's blood / wound tasks are done
-- (bounded); a native OpenLevel that cannot wait gets a synchronous purge.
-- HSMP_RVP_GUARD=0 turns it off (A/B and field diagnosis only).
local RVPM = load_module("hsmp_rvp")
local RVP_OFF = (trim(os.getenv("HSMP_RVP_GUARD")) or "") == "0"
local RVP = (RVPM and not RVP_OFF) and RVPM.new({ log = Log, ev = ev }) or nil
Log("RVP travel guard: %s", RVP and ("shared/hsmp_rvp.lua v" .. tostring(RVPM.VERSION))
    or (RVP_OFF and "OFF (HSMP_RVP_GUARD=0)" or "NOT DEPLOYED - level changes do not wait for vertex-paint tasks"))
-- The queue closed for a travel opens again at the start of LoadMap: before
-- the new arena's BeginPlay, which builds its weapons through RVP tasks.
if RVP then
    local ok_lm = pcall(function() RegisterLoadMapPreHook(function() pcall(RVP.on_loadmap) end) end)
    Log("RVP LoadMap reopen hook %s", ok_lm and "registered" or "NOT registered (reopen falls back to the new world's first tick)")
end
local denv = Director.make_ue_env({ WG = WG, UEHelpers = UEHelpers, log = Log, ev = ev, state_dir = STATE_DIR,
    SG = SG, RVP = RVP })
-- The session: the typed link / session records in shared memory (director.lua D.session_reader).
local get_session = Director.session_reader(denv, STATE_DIR)
Log("session source: shared-memory records (link, session)")
-- env.world(): the result of this tick's single wg_check() (tick() below).
local tick_wok = false
denv.world = function()
    return { ok = tick_wok, short = tick_wok and WG.short() or nil, key = tick_wok and WG.key or nil }
end
local director = Director.new(denv, { state_dir = STATE_DIR, get_session = get_session })

-- The session table the banner / spectate / suppression read (refreshed by the
-- Director every tick; display only, never a travel decision).
local session = director.get_session()

local function local_pawn()
    local pc = WG.pc()
    if not pc or not pc:IsValid() then return nil, nil end
    local p = pc.Pawn
    if p and p:IsValid() then return p, pc end
    return nil, pc
end

local function pawn_health(p)
    local hp
    pcall(function() hp = tonumber(p.Health) end)
    return hp
end

local function nick_of(id)
    if id == session.my_id then return session.my_nick end
    return session.nicks[id] or ("P" .. tostring(id))
end

local function score_line()
    local parts = {}
    for _, id in ipairs(session.order) do
        table.insert(parts, string.format("%s %d", nick_of(id), session.wins[id] or 0))
    end
    return table.concat(parts, "  –  ")
end

-- "World /Game/Maps/Arenas/Map_Arena_Alley.Map_Arena_Alley" -> "Map_Arena_Alley"
local function short_name(s)
    if not s then return nil end
    return s:match("([%w_]+)$")
end

-- --- native OpenLevel interception (the Director decides) ---------------------------
-- Pre-hook: any native travel to a level that is not where the server wants
-- us is rewritten (the arena while a match runs, the menu otherwise), for as
-- long as an MP session process exists (not only while connected).

-- The pre-hooks announce the world leave to the shared-memory IPC (despite
-- the function's name, no flag file is written).
local function write_level_change_flag(_target)
    if IPC then IPC.world_leaving() end
end

local ok_hook = pcall(function()
    RegisterHook("/Script/Engine.GameplayStatics:OpenLevel",
        function(_ctx, _world, LevelName, _absolute, _options)
            local ok, raw = pcall(function() return LevelName:get():ToString() end)
            if not ok or not raw then pcall(write_level_change_flag, "?"); return end
            local target = short_name(raw)
            if not target then pcall(write_level_change_flag, raw); return end
            -- A native travel cannot wait for the vertex-paint queue
            -- (the Director's own travels already waited in Dir:open). MP
            -- sessions only: single-player is never touched.
            if RVP and not director.self_travel and director.sess and director.sess.exists then
                pcall(RVP.purge_now, "native OpenLevel(" .. target .. ")")
            end
            local okd, dest = pcall(director.on_native_open_level, director, target)
            if not okd then Log("director pre-hook error: %s", tostring(dest)); dest = nil end
            if dest and dest ~= target then
                local set_ok = pcall(function() LevelName:set(FName(dest)) end)
                if not set_ok then Log("native OpenLevel('%s') -> '%s' [param rewrite FAILED]", raw, dest) end
            end
            pcall(write_level_change_flag, dest or target)
        end)
end)
Log("OpenLevel hook %s", ok_hook and "registered" or "FAILED to register")


-- MP session = a sidecar is running for this game: its link record says
-- connected/reconnecting AND its heartbeat in the segment header is at most
-- MP_ALIVE_S old (a dead or hung sidecar stops beating), so single-player is
-- never treated as MP. Gates the slow-mo kill switch and the
-- lose-flow block below.
local MP_ALIVE_S = 15
-- The shared liveness tracker (shared/hsmp_session.lua) with this
-- guard's own, longer freshness window: connected OR reconnecting (a lost
-- link must keep the native lose flow blocked) AND a sidecar heartbeat within
-- MP_ALIVE_S.
local _mp = { active = false }
_mp.S = (function()
    local H = load_module("hsmp_session")
    return H and H.new({ fresh_s = MP_ALIVE_S, every_s = 0.5 })
end)()
if not _mp.S then Log("WARNING: shared/hsmp_session.lua missing - the MP guard treats every game as single-player") end
local function mp_session_active()
    local S = _mp.S
    if not S then return false end
    S:poll()
    local st = S.status
    local active = S.exists and (st == "connected" or st == "reconnecting") and S:fresh()
    if active ~= _mp.active then
        Log("MP session %s (sidecar status=%s)", active and "ACTIVE: slow-mo off, native lose flow blocked"
            or "inactive: single-player behaviour untouched", tostring(st))
    end
    _mp.active = active
    return active
end

-- --- native lose / give-up travel: blocked at the source (MP only) -----------------
-- Evidence (Kismet bytecode decompiled with tools/mapdump --probe,
-- docs/development/subsystems/spawns.md):
--   Willie_BP (Player, give-up/downed path, offset 256220): if GameMode
--   "Match Won" is false and no UI_NextFIght exists -> Create(UI_Lose_C).
--   UI_Lose_C Construct: GI "Time Dilation" = 0.25 + SetGlobalTimeDilation,
--   Delay(0.25) -> Delay(1.0) -> Health = 100, SetGamePaused(true), GI
--   "Save Game"() (the CAREER slot), then OpenLevelBySoftObjectPtr(
--   Map_Hub_Tavern_Frank | Map_Menu_Startup by "Current Play Mode").
--   UI_DED_C / UI_DeathDoor_C / UI_GiveUp_C / UI_NextFIght_C reach their
--   OpenLevelBySoftObjectPtr the same way: every one of them runs behind a
--   KismetSystemLibrary:Delay latent action on the widget itself.
-- UE4SS 3.0.1 hooks on /Game/ (Blueprint) functions are post-only, so the BP
-- cannot be stopped from Lua, but Delay is native (/Script/): its pre-hook
-- rewrites Duration to ~4 months when the latent owner is one of these
-- widgets, so the continuation (pause, career save, travel) never runs before
-- the server's round reset reloads the world. The dead player stays in the
-- arena and spectates. Second layer: if a native OpenLevelBySoftObjectPtr still
-- happens, its post-hook immediately re-issues OpenLevel to the right place
-- (last travel request wins), so there is one load, never Tavern -> arena.
local LOSE_FLOW_WIDGETS = {
    UI_Lose_C = true, UI_DED_C = true, UI_DeathDoor_C = true, UI_GiveUp_C = true,
    UI_NextFIght_C = true, UI_WIN_C = true, UI_WinScreen_C = true,
}
local LOSE_FLOW_DELAY_S = 1.0e7
local lose_blocks = {}

local function note_lose_block(what)
    lose_blocks[what] = (lose_blocks[what] or 0) + 1
    local n = lose_blocks[what]
    if n == 1 or n % 20 == 0 then
        Log("blocked native lose-flow travel (%s) x%d", what, n)
    end
end

pcall(function()
    RegisterHook("/Script/Engine.KismetSystemLibrary:Delay", function(_ctx, WorldContextObject, Duration)
        if not mp_session_active() then return end
        local cls
        pcall(function() cls = WorldContextObject:get():GetClass():GetFName():ToString() end)
        if not cls or not LOSE_FLOW_WIDGETS[cls] then return end
        local ok = pcall(function() Duration:set(LOSE_FLOW_DELAY_S) end)
        note_lose_block(cls .. ":Delay" .. (ok and "" or " [Duration rewrite FAILED]"))
    end)
end)

local _soft_travel = nil
pcall(function()
    RegisterHook("/Script/Engine.GameplayStatics:OpenLevelBySoftObjectPtr", function(_ctx, _world, Level)
        pcall(write_level_change_flag, "soft-object-ptr")
        if RVP and not director.self_travel and director.sess and director.sess.exists then   -- RVP purge (MP only)
            pcall(RVP.purge_now, "native OpenLevelBySoftObjectPtr")
        end
        -- Never read `Level`: it is a TSoftObjectPtr, and UE4SS 3.0.1's
        -- push_softobjectproperty memcpy crashes the game (access violation,
        -- not catchable by pcall).
        _soft_travel = "soft-object-ptr"
    end, function()
        -- Post: the native travel is pending now; the last request wins, so the
        -- Director re-issues its own (the server's arena mid-match, the menu
        -- after it) whenever a session process exists.
        local target = _soft_travel
        _soft_travel = nil
        if target == nil then return end
        local ok, dest = pcall(director.on_soft_travel_post, director, target)
        if not ok then Log("director soft-travel hook error: %s", tostring(dest)); return end
        if dest then note_lose_block(string.format("OpenLevelBySoftObjectPtr %s -> rerouted to %s", target, dest)) end
    end)
end)

-- --- native end-of-fight suppression ------------------------------------------------
-- The arena's own flow treats a death (ours, or a puppet foe's) as the end of the
-- fight: win/lose widgets, then a level change. In MP the server decides rounds.

-- World-scoped: dropped (untouched) by the world guard on every level change.
-- A widget announced during the load itself would be dropped with them, so
-- the first tick of a new world re-collects live instances with FindAllOf.
local native_widgets = {}
local native_widgets_rescan = false

-- The session process exists (connected OR riding out a reconnect), not
-- only `connected`, so a reconnect does not lift the win-flow pinning.
local function mp_match_running()
    return Director.match_running(session)
end

-- Widgets are tracked only while an MP match runs (and suppressed on the
-- next tick). Outside MP the native career widgets (UI_Tier_C, UI_Unlock_*,
-- UI_Cards_C, ...) are created and GC'd within one world; keeping them for
-- the whole world and calling IsValid on them later reads freed memory.
pcall(function()
    NotifyOnNewObject("/Script/UMG.UserWidget", function(obj)
        if not mp_match_running() then return end
        pcall(function()
            local cls = obj:GetClass():GetFName():ToString()
            if NATIVE_END_WIDGETS[cls] then table.insert(native_widgets, obj) end
        end)
    end)
end)

local function rescan_native_widgets()
    native_widgets = {}
    if not mp_match_running() then return end
    for cls in pairs(NATIVE_END_WIDGETS) do
        pcall(function()
            for _, w in pairs(FindAllOf(cls) or {}) do table.insert(native_widgets, w) end
        end)
    end
end

local function restore_game_input()
    local _, pc = local_pawn()
    if not pc then return end
    local wbl = StaticFindObject("/Script/UMG.Default__WidgetBlueprintLibrary")
    if wbl and wbl:IsValid() then pcall(function() wbl:SetInputMode_GameOnly(pc, false) end) end
    pcall(function() pc:SetShowMouseCursor(false) end)
    pcall(function() pc.bShowMouseCursor = false end)
end

local function suppress_native_end_flow(in_arena)
    -- Never hold widgets outside a running MP match (see NotifyOnNewObject).
    if #native_widgets > 0 and not mp_match_running() then native_widgets = {} end
    if #native_widgets > 0 then
        local keep, hid = {}, false
        for _, w in ipairs(native_widgets) do
            if w and w:IsValid() then
                if mp_match_running() then
                    local name = "?"
                    pcall(function() name = w:GetClass():GetFName():ToString() end)
                    pcall(function() w:SetVisibility(1) end)   -- Collapsed
                    pcall(function() w:RemoveFromParent() end)
                    Log("suppressed native %s during MP match", name)
                    hid = true
                else
                    table.insert(keep, w)
                end
            end
        end
        native_widgets = keep
        if hid then restore_game_input() end
    end
    -- Puppet foes dying must not count down to the arena's "all enemies
    -- dead" win: keep the GameMode's enemy count pinned while a match runs.
    if in_arena and mp_match_running() then
        pcall(function()
            local gs = UEHelpers.GetGameplayStatics()
            local gm = gs:GetGameMode(WG.world())
            if gm and gm:IsValid() then
                -- Real (space-containing) property names; CamelCase silently no-ops.
                if (tonumber(gm["Enemy Count"]) or 0) < 50 then gm["Enemy Count"] = 99 end
                if gm["All Enemies Dead"] == true then gm["All Enemies Dead"] = false end
                if gm["Match Won"] == true then gm["Match Won"] = false end
            end
        end)
    end
end

-- --- centre banner: owned by HSMPHud -------------------------------------------------
-- The phase banner (round, waiting for players, FIGHT!, results, connection
-- states, spectating) is drawn by HSMPHud alone (one banner owner).
-- HSMPMatch only logs the server's results below.

-- --- spectate after death ------------------------------------------------------------
-- Dead while the round plays out: the camera follows a living opponent's
-- stand-in (bus key puppets: peer id -> Willie FName). The target advances by
-- itself when it dies; Q / E cycle while spectating. The choice is published
-- in the bus key spectate for HSMPHud's "Spectating <name>" label:
--   {target = <peer id>, nick, round, alive}   or   {target = 0} (not spectating)

local spec = { on = false, target = nil, written = nil }

-- Bus key `spectate` (typed record; target 0 = not spectating). Written on change only.
local function write_spectate(target, nick, alive_n)
    local t = { target = target or 0, nick = target and tostring(nick or "") or "",
                round = target and (session.round or 0) or 0, alive = target and (alive_n or 0) or 0 }
    local key = string.format("%d|%s|%d|%d", t.target, t.nick, t.round, t.alive)
    if key == spec.written then return end
    spec.written = key
    local ipc = rawget(_G, "HSMP_IPC")
    if ipc and ipc.bus_put then ipc.bus_put("spectate", t) end
end

-- Hand the camera back to the own pawn (fresh lookup; the view
-- target was a stand-in of THIS world, the world guard clears spec.on on a
-- level change, so a new world never gets here with spec.on set).
local function view_own()
    local p, pc = local_pawn()
    if not (p and pc) then return false end
    local ok = pcall(function() pc:SetViewTargetWithBlend(p, 0.4, 0, 0, false) end)
    return ok
end

local function stop_spectating(why)
    if spec.on then
        Log("spectating off (%s)%s", why, view_own() and "; camera back on the own pawn" or "")
    end
    spec.on, spec.target = false, nil
    write_spectate(nil)
end

-- Living opponents with a stand-in, in roster order.
local function spectate_candidates()
    local pt = IPC and IPC.bus_table("puppets")
    local names = {}
    for _, r in ipairs(type(pt) == "table" and pt.rows or {}) do
        if r.peer and r.peer ~= 0 and type(r.name) == "string" and r.name ~= "" then names[r.peer] = r.name end
    end
    local out = {}
    for _, id in ipairs(session.order) do
        if id ~= session.my_id and session.alive[id] and names[id] then
            out[#out + 1] = { id = id, fname = names[id] }
        end
    end
    return out
end

-- Fresh lookup of the stand-in by FName; nothing is cached.
local function view_target(fname)
    local _, pc = local_pawn()
    if not pc then return false end
    -- No FindAllOf over Willies in the first seconds of a world
    if not WG.settled() then return false end
    local ok = false
    pcall(function()
        for _, w in pairs(FindAllOf("Willie_BP_C") or {}) do
            if w and w:IsValid() and w:GetFName():ToString() == fname then
                pc:SetViewTargetWithBlend(w, 0.6, 0, 0, false)
                ok = true
                return
            end
        end
    end)
    return ok
end

-- step 0: keep the current target while it lives (else take the next one);
-- step +1 / -1: cycle.
local function spectate_step(step)
    local c = spectate_candidates()
    if #c == 0 then
        if spec.on then write_spectate(spec.target, nick_of(spec.target), 0) end
        return
    end
    local idx
    for i, e in ipairs(c) do if e.id == spec.target then idx = i end end
    if idx and step == 0 then
        write_spectate(spec.target, nick_of(spec.target), #c)   -- keeps "alive" current (deduped)
        return
    end
    idx = idx and (((idx - 1 + step) % #c) + 1) or 1
    local e = c[idx]
    if view_target(e.fname) then
        local was = spec.target
        spec.on, spec.target = true, e.id
        Log("spectating %s (peer %d)%s", nick_of(e.id), e.id,
            (step ~= 0 and " [cycled]") or (was and " [previous target died]") or "")
        write_spectate(e.id, nick_of(e.id), #c)
    end
end

local function on_cycle(step)
    if spec.on then pcall(spectate_step, step) end
end
pcall(function()
    if Key and Key.Q then RegisterKeyBind(Key.Q, function() on_cycle(-1) end) end
    if Key and Key.E then RegisterKeyBind(Key.E, function() on_cycle(1) end) end
end)

-- --- MP never pauses ------------------------------------------------------------------
-- The world must keep running for everyone: HSMPHud replaces the native pause
-- menu with the MP pause; this guard undoes any pause that still happens.
local unpause_n = 0
local function keep_world_running()
    pcall(function()
        local gs = UEHelpers.GetGameplayStatics()
        local world = WG.world()
        if gs and world and world:IsValid() and gs:IsGamePaused(world) then
            gs:SetGamePaused(world, false)
            unpause_n = unpause_n + 1
            if unpause_n == 1 or unpause_n % 20 == 0 then
                Log("MP: the world was paused (native pause / lose flow) -> unpaused [x%d]", unpause_n)
            end
        end
    end)
end

-- --- hair-strands streaming workaround ------------------------------------------------
-- r.HairStrands.Streaming 0 at runtime (PAK_ASYNC_READ_OOB at the first arena
-- load; docs/development/halfsword/io-dispatcher-crash.md). Engine.ini is
-- reset by the game; applied at boot and on
-- every world change (Director env.apply_cvars, the only console use here).
local io1 = { logged = false, fails = 0 }
local function apply_io1(why)
    local okc, ok, back = pcall(denv.apply_cvars)
    if okc and ok then
        if not io1.logged then
            io1.logged = true
            Log("hair-strand streaming workaround: r.HairStrands.Streaming=0 (runtime)%s",
                back ~= nil and string.format(" [read back %s, %s]", tostring(back), why) or (" [" .. why .. "]"))
        end
    else
        io1.fails = io1.fails + 1
        if io1.fails <= 3 and why ~= "boot" then Log("hair-strand streaming workaround: cvar not applied yet (%s)", why) end
    end
end
apply_io1("boot")

-- --- MP: slow motion fully disabled ---------------------------------------------------
-- Every slow-motion path found in UE4SS_ObjectDump.txt + the decompiled
-- Willie_BP / UI_* bytecode (docs/development/subsystems/spawns.md, "Slow motion"):
--   1. Willie_BP_C "InpActEvt_SloMo_K2Node_InputActionEvent_8" (the SloMo input
--      action): if Consciousness > 50 and Stamina > 0.5 it toggles
--      "Slomo Active" and plays/reverses "Slomo Timeline".
--   2. "Slomo Timeline__UpdateFunc": GI "Time Dilation" = clamp(timeline value,
--      lerp(0.75, 0.25, "Body Skill (Temp)"), 1); "Arrow Time" = GI "Time
--      Dilation"; SetGlobalTimeDilation(Arrow Time) + vignette/fringe PP.
--   3. "End Arrow Time Event" re-applies Arrow Time to the global dilation.
--   4. UI_Lose_C / UI_DED_C / UI_DeathDoor_C (death): GI "Time Dilation" = 0.25 +
--      SetGlobalTimeDilation; UI_GiveUp_C, UI_WinScreen_C, UI_WeaponSelection_C,
--      BPC_PhotoMode_C (photo mode) also call SetGlobalTimeDilation.
--   5. CheatManager:Slomo (console "slomo").
--   6. "Skill Unlock Body Slomo" (Willie bool): set off as well, although the
--      SloMo input handler never reads it (no reference in the bytecode).
-- In an MP session only:
--   * pre-hook GameplayStatics:SetGlobalTimeDilation and CheatManager:Slomo
--     rewrite any value != 1 to 1.0 (covers 2, 3, 4, 5 at the source);
--   * post-hooks on 1, 2, 3 (Blueprint hooks are post-only in UE4SS 3.0.1) stop
--     the timeline, clear Slomo Active / Arrow Time and restore dilation in
--     the same frame;
--   * a periodic guard (SLOMO_GUARD_MS) resets global dilation, GI "Time Dilation", and every
--     Willie's CustomTimeDilation / Arrow Time / Slomo Active / unlock flag,
--     logging (rate-limited, with a count) whenever it had to correct one.
-- Nothing here runs or is cached outside an MP session; every object is a
-- fresh lookup (no UObject survives a callback).
local WILLIE_FN = "/Game/Character/Blueprints/Willie_BP.Willie_BP_C:"
local SLOMO_BP_HOOKS = {
    { fn = WILLIE_FN .. "InpActEvt_SloMo_K2Node_InputActionEvent_8", why = "SloMo input" },
    { fn = WILLIE_FN .. "Slomo Timeline__UpdateFunc",                why = "Slomo Timeline update" },
    { fn = WILLIE_FN .. "End Arrow Time Event",                      why = "End Arrow Time Event" },
}
-- 250 ms: each pass costs a PlayerController walk. The native setters are
-- rewritten by pre-hooks below; this guard is the backstop.
local SLOMO_GUARD_MS = 250
local slomo = { hooked = {}, hook_tries = 0, logged = {}, counts = {}, n = 0 }

local function slomo_note(kind, detail)
    slomo.counts[kind] = (slomo.counts[kind] or 0) + 1
    local now = os.clock()
    if not slomo.logged[kind] or now - slomo.logged[kind] > 10 then
        slomo.logged[kind] = now
        Log("slow-mo guard: corrected %s%s (x%d this session)", kind,
            detail and (": " .. detail) or "", slomo.counts[kind])
    end
end

local function near1(v) return v == nil or math.abs(v - 1.0) < 1e-3 end

local function neutralise_willie(w, why)
    if not (w and w:IsValid()) then return end
    local fixed = {}
    pcall(function()
        if w["Slomo Active"] == true then w["Slomo Active"] = false; table.insert(fixed, "Slomo Active") end
    end)
    pcall(function()
        local t = w["Slomo Timeline"]
        if t and t:IsValid() and t:IsPlaying() then
            t:Stop(); t:SetNewTime(0.0); table.insert(fixed, "Slomo Timeline")
        end
    end)
    pcall(function()
        if not near1(tonumber(w["Arrow Time"])) then w["Arrow Time"] = 1.0; table.insert(fixed, "Arrow Time") end
    end)
    pcall(function()
        if not near1(tonumber(w.CustomTimeDilation)) then
            w.CustomTimeDilation = 1.0; table.insert(fixed, "CustomTimeDilation")
        end
    end)
    pcall(function()
        if w["Skill Unlock Body Slomo"] == true then
            w["Skill Unlock Body Slomo"] = false; table.insert(fixed, "Skill Unlock Body Slomo")
        end
    end)
    if #fixed > 0 then slomo_note("Willie " .. why, table.concat(fixed, ", ")) end
end

local function enforce_time_dilation(why)
    local world = WG.world()
    if not world or not world:IsValid() then return end
    pcall(function()
        local gs = UEHelpers.GetGameplayStatics()
        local td = tonumber(gs:GetGlobalTimeDilation(world))
        if not near1(td) then
            gs:SetGlobalTimeDilation(world, 1.0)
            slomo_note("global time dilation", string.format("%.2f -> 1.0 (%s)", td, why))
        end
    end)
    pcall(function()
        local gi = UEHelpers.GetGameInstance()
        local v = tonumber(gi["Time Dilation"])
        if not near1(v) then
            gi["Time Dilation"] = 1.0
            slomo_note("GI Time Dilation", string.format("%.2f -> 1.0 (%s)", v, why))
        end
    end)
end

-- Native setters: rewrite the value before it is applied.
pcall(function()
    RegisterHook("/Script/Engine.GameplayStatics:SetGlobalTimeDilation", function(_ctx, _wc, TimeDilation)
        if not mp_session_active() then return end
        local v; pcall(function() v = TimeDilation:get() end)
        if v and not near1(v) then
            local ok = pcall(function() TimeDilation:set(1.0) end)
            slomo_note("SetGlobalTimeDilation call", string.format("%.2f -> 1.0%s", v, ok and "" or " [rewrite FAILED]"))
        end
    end)
end)
pcall(function()
    RegisterHook("/Script/Engine.CheatManager:Slomo", function(_ctx, NewTimeDilation)
        if not mp_session_active() then return end
        local v; pcall(function() v = NewTimeDilation:get() end)
        if v and not near1(v) then
            pcall(function() NewTimeDilation:set(1.0) end)
            slomo_note("CheatManager:Slomo", string.format("%.2f -> 1.0", v))
        end
    end)
end)

-- Blueprint post-hooks: registered once Willie_BP is loaded (RegisterHook needs
-- the UFunction in memory).
-- Retried forever at a low rate (1 Hz) until every hook is registered;
-- Willie_BP may load only with the first arena, long after mod load.
local function try_hook_slomo_bp()
    local now = os.clock()
    if slomo.all_hooked or now < (slomo.next_try or 0) then return end
    slomo.next_try = now + 1.0
    local missing = 0
    for _, h in ipairs(SLOMO_BP_HOOKS) do
        if not slomo.hooked[h.fn] then
            missing = missing + 1
            slomo.hook_tries = slomo.hook_tries + 1
            local f; pcall(function() f = StaticFindObject(h.fn) end)
            if f and f:IsValid() then
                local ok = pcall(function()
                    RegisterHook(h.fn, function(self)
                        if not mp_session_active() then return end
                        local w; pcall(function() w = self:get() end)
                        neutralise_willie(w, h.why)
                        enforce_time_dilation(h.why)
                    end)
                end)
                -- Only a hook that really registered is done; a failed RegisterHook is retried at 1 Hz (logged on
                -- the first 3 tries, then every 60th).
                if ok then
                    slomo.hooked[h.fn] = true
                    missing = missing - 1
                    Log("slow-mo hook registered: %s", h.fn)
                else
                    slomo.failed = slomo.failed or {}
                    local n = (slomo.failed[h.fn] or 0) + 1
                    slomo.failed[h.fn] = n
                    if n <= 3 or n % 60 == 0 then Log("slow-mo hook FAILED (try %d, retrying): %s", n, h.fn) end
                end
            end
        end
    end
    if missing == 0 then slomo.all_hooked = true end
end

local function slomo_guard()
    try_hook_slomo_bp()
    if not mp_session_active() then return end
    -- Fresh lookups only, and never while a level change is pending.
    if not wg_check() then return end
    enforce_time_dilation("guard")
    slomo.n = slomo.n + 1
    -- Willies every 500 ms; never in the first WG.settled() seconds of
    -- a world (FindAllOf over Willies in the round-reload crash window).
    if slomo.n % 2 == 0 and WG.settled() then
        pcall(function()
            for _, w in pairs(FindAllOf("Willie_BP_C") or {}) do neutralise_willie(w, "guard") end
        end)
    end
end

-- --- per-tick: Director first, then display / suppression -------------------------

local tick_n = 0
local hitch_prev_key = nil
local last_result_key = nil
local last_world_key  = nil

-- World changed / level change requested: forget every world-scoped object
-- without touching it (tracked native widgets; the spectate target is by name).
wg_on_drop(function()
    native_widgets = {}
    native_widgets_rescan = true
    spec.on, spec.target = false, nil
end, "native_widgets+spectate")

local function tick()
    tick_n = tick_n + 1
    -- Save guard first: it touches only the GI, which survives level changes.
    if SG then pcall(SG.tick) end
    -- One world-guard check per tick, shared with the Director (env.world).
    tick_wok = wg_check()
    -- Hitch probe (release profile): this 250 ms game-thread loop is the
    -- process's only M.frame() caller. A gap is a level load (travel=true) when
    -- the world is not ready / changed since the last tick / a travel is
    -- pending; otherwise hsmp_log falls back to "a travel event in the last 30 s".
    if HL and HL.frame then
        local wk = tick_wok and WG.key or nil
        local travel = (not tick_wok) or WG.pending() or director.pending_travel ~= nil
            or (hitch_prev_key ~= nil and wk ~= hitch_prev_key)
        local fwk = wk or hitch_prev_key
        pcall(function() HL.frame(TICK_MS, travel or nil, fwk) end)
        hitch_prev_key = wk or hitch_prev_key
    end
    -- The Director runs every tick, also while a level change is pending
    -- (requests, heartbeat and ping need no UObject); it touches the world
    -- only when tick_wok is true.
    local okd, derr = pcall(director.tick, director)
    if not okd then log_err("director tick error", derr) end
    session = director.sess or session

    if not tick_wok or WG.pending() then return end   -- a travel was issued this tick
    if SG and not sg_selftest_done and short_name(WG.name) == "Map_Menu_Startup" then
        sg_selftest_done = true
        local ok, r, d = pcall(SG.selftest)
        Log("saveguard selftest: %s (%s)", (ok and r) and "ok" or "FAILED", tostring(ok and d or r))
    end
    if native_widgets_rescan then
        native_widgets_rescan = false
        rescan_native_widgets()
    end
    local w, addr = short_name(WG.name), WG.key
    if addr ~= last_world_key then
        last_world_key = addr
        apply_io1("world " .. tostring(w))
        stop_spectating("new world")
    end
    local in_arena = w ~= nil and w == session.arena

    -- Log each FINAL result once ("pending" = server still settling trades).
    if (session.phase == "roundover" or session.phase == "match_over") and session.reason ~= "pending" then
        local key = string.format("%s:%d:%d:%s", session.phase, session.round, session.last_winner, session.reason)
        if key ~= last_result_key then
            last_result_key = key
            -- The only outcome this client acts on is the server's.
            local who = (session.last_winner == 0) and "draw"
                or string.format("%s (peer %d)", nick_of(session.last_winner), session.last_winner)
            Log("RESULT from server: %s round %d winner=%s reason=%s score: %s",
                session.phase, session.round, who,
                session.reason ~= "" and session.reason or "-", score_line())
        end
    end

    suppress_native_end_flow(in_arena)
    if in_arena and mp_session_active() then keep_world_running() end

    -- Dead (per the server, or our pawn's Health) while the round plays out:
    -- watch a living opponent; off again when the round is over.
    local round_on = session.phase == "live" or session.phase == "roundover"
    if in_arena and session.connected and round_on then
        local p = local_pawn()
        local hp = p and pawn_health(p)
        local dead = session.alive[session.my_id] == false or (hp ~= nil and hp <= 0)
        if dead then pcall(spectate_step, 0) elseif spec.on then stop_spectating("alive") end
    elseif spec.on and not round_on then
        stop_spectating("phase " .. tostring(session.phase))
    end
end

LoopAsync(SLOMO_GUARD_MS, function()
    ExecuteInGameThread(function()
        local ok, err = pcall(slomo_guard)
        if not ok then log_err("slow-mo guard error", err) end
    end)
    return false
end)

LoopAsync(TICK_MS, function()
    ExecuteInGameThread(function()
        local ok, err = pcall(tick)
        if not ok then log_err("tick error", err) end
    end)
    return false
end)

Log("loaded; state_dir=%s (Director v%d, world guard v%d)", STATE_DIR, Director.VERSION, HW.VERSION)
