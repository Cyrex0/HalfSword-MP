-- HSMPNoCutscene — stops the cutscene/video Half Sword plays when it thinks
-- the player is idle (an unfocused second window never gets input, so it
-- always looks idle).
--
-- Rule: anything already playing within GRACE_S of a level loading is the
-- level's own backdrop/intro and is left alone. Any level sequence or media
-- player that starts after that is skipped to its end (GoToEndAndStop fires
-- the sequence's finish event, so Blueprint waiting on it carries on) or
-- closed. Every sequence/media seen is logged by name so the trigger can be
-- identified. Set HSMP_ALLOW_CUTSCENES=1 to disable.
--
-- World guard (shared/hsmp_wg.lua): no UObject is kept beyond one callback.
-- The Play hook handles the player synchronously in its POST callback
-- (deferring a captured object, even by 1 ms, is the stale-object pattern
-- that crashes the game); the poll runs only when the guard says the world
-- is current, and every per-world table is keyed by name and reset on each
-- world change.

local UEHelpers = require("UEHelpers")

-- Thread-safety shim (see HSMPAvatars): keep all Lua on the game thread.
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
end

local function Log(fmt, ...)
    print(string.format("[HSMPNoCutscene] " .. fmt .. "\n", ...))
end

if os.getenv("HSMP_ALLOW_CUTSCENES") == "1" then
    Log("disabled by HSMP_ALLOW_CUTSCENES=1")
    return
end

-- shared/*.lua is copied into Scripts/ at deploy; fall back to the source tree.
local function load_module(name)
    local ok, mod = pcall(require, name)
    if ok and type(mod) == "table" then return mod end
    local src = (debug.getinfo(1, "S").source or ""):gsub("^@", "")
    local dir = src:match("^(.*)[/\\]") or "."
    for _, path in ipairs({ dir .. "/" .. name .. ".lua", dir .. "/../../shared/" .. name .. ".lua" }) do
        local ok2, mod2 = pcall(dofile, path)
        if ok2 and type(mod2) == "table" then return mod2 end
    end
    return nil
end

local HW = load_module("hsmp_wg")
if not HW then
    Log("FATAL: shared/hsmp_wg.lua missing - HSMPNoCutscene disabled (deploy copies shared/*.lua)")
    return
end
local WG = HW.new({ log = Log, UEHelpers = UEHelpers })

local GRACE_S = 8.0
local POLL_MS = 500

local world_loaded_at = 0
local baseline = {}   -- full name -> true: playing during the grace window
local seen = {}       -- full name -> true: already logged
local stopped = {}    -- full name -> true: already stopped

local function now() return os.time() end   -- wall clock; 1 s resolution is plenty

-- Names only (never objects): a new world restarts the grace window.
WG.cache("baseline/stopped (names)", function()
    baseline, stopped = {}, {}
    world_loaded_at = now()
end)

local function full_name(o)
    local n; pcall(function() n = o:GetFullName() end)
    return n or "?"
end

local function in_gameplay() return WG.name and WG.name:find("Map_Menu_") == nil end

local function release_cinematic_lock()
    local pc = UEHelpers.GetPlayerController()
    if not pc or not pc:IsValid() then return end
    -- bInCinematicMode, bHidePlayer, bAffectsHUD, bAffectsMovement, bAffectsTurning
    pcall(function() pc:SetCinematicMode(false, false, false, true, true) end)
end

-- The splash level's intro video/sequence gates the move to the main menu;
-- stopping it would leave the game stuck on the splash. Never touch it.
local function protected_world()
    return WG.name == nil or WG.name:find("Map_Menu_SplashScreens") ~= nil
end

local function handle_sequence(p, source)
    if not p or not p:IsValid() or protected_world() then return end
    local name = full_name(p)
    local playing = false
    pcall(function() playing = p:IsPlaying() end)
    if not seen[name] then
        seen[name] = true
        Log("sequence seen (%s, playing=%s): %s", source, tostring(playing), name)
    end
    if not playing or baseline[name] or stopped[name] then return end
    if now() - world_loaded_at < GRACE_S then
        baseline[name] = true
        Log("allowing level sequence (started during load grace): %s", name)
        return
    end
    stopped[name] = true
    pcall(function() p:GoToEndAndStop() end)
    Log("STOPPED idle/random cutscene: %s", name)
    if in_gameplay() then release_cinematic_lock() end
end

local function handle_media(m)
    if not m or not m:IsValid() or protected_world() then return end
    local name = full_name(m)
    local playing = false
    pcall(function() playing = m:IsPlaying() end)
    if not seen[name] then
        seen[name] = true
        Log("media player seen (playing=%s): %s", tostring(playing), name)
    end
    if not playing or baseline[name] or stopped[name] then return end
    if now() - world_loaded_at < GRACE_S then
        baseline[name] = true
        Log("allowing media (started during load grace): %s", name)
        return
    end
    stopped[name] = true
    pcall(function() m:Close() end)
    Log("STOPPED idle/random video: %s", name)
end

-- Immediate path: Blueprint calling Play on a sequence player. Handled in the
-- POST callback, synchronously, while the object is certainly alive (Play has
-- run, so IsPlaying() is meaningful). Nothing is deferred or kept.
pcall(function()
    RegisterHook("/Script/MovieScene.MovieSceneSequencePlayer:Play", function() end, function(self)
        if WG.pending() or WG.key == nil then return end   -- level change in flight / no current world
        local p; pcall(function() p = self:get() end)
        pcall(handle_sequence, p, "Play hook")
    end)
    Log("Play hook registered (post, synchronous)")
end)

-- Fallback path: poll for anything playing (auto-play actors bypass the hook).
LoopAsync(POLL_MS, function()
    ExecuteInGameThread(function()
        if not WG.check() then return end
        pcall(function()
            for _, p in pairs(FindAllOf("LevelSequencePlayer") or {}) do handle_sequence(p, "poll") end
        end)
        pcall(function()
            for _, m in pairs(FindAllOf("MediaPlayer") or {}) do handle_media(m) end
        end)
    end)
    return false
end)

Log("loaded (grace %.0fs after each level load)", GRACE_S)
