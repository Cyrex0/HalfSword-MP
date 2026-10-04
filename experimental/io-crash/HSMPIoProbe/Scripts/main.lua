-- HSMPIoProbe (dev only): single-player driver for the IO-1 crash
-- (PAK_ASYNC_READ_OOB, docs/development/halfsword/io-dispatcher-crash.md).
-- It reloads one arena HSMP_IO_STRESS times and appends one line per reload
-- to <state>/.io_stress.txt before travelling (survives a crash).
--
-- HSMP_IO_MODE picks what happens to the arena's Willies on each load:
--   plain    nothing: the game's own load (the vanilla rate)
--   reveal   hidden at BeginPlay, shown after 3 s (HSMPLoadout's invisible
--            dressing, with no dressing)
--   armor    as reveal, then HSMP_IO_DELAY_MS later the player's "Set Up Armor"
--            runs on the now visible pawn (the late-kit path)
--   destroy  as reveal, then "Destroy Hair" on the player's pawn: the groom
--            component goes away while its first strands read is in flight
--
-- Not deployed by build-and-deploy.ps1. experimental/io-crash/soak.ps1 copies it
-- in, sets mods.txt and restores both afterwards.
--
-- WARNING: single-player. The career save guard is inactive and the game saves
-- the player's state into GameProgress.sav. Back up SaveGames first.

local UEHelpers = require("UEHelpers")

local function Log(fmt, ...) print(string.format("[HSMPIoProbe] " .. fmt .. "\n", ...)) end
local function trim(s) return s and s:match("^%s*(.-)%s*$") or nil end
if (trim(os.getenv("HSMP_DEV")) or "") ~= "1" then Log("off (HSMP_DEV is not 1)"); return end

-- Game-thread loops only (UE4SS async loops corrupt the Lua VM).
local function loop(ms, fn)
    local done = false
    LoopInGameThreadWithDelay(ms, function()
        if done then return true end
        local ok, stop = pcall(fn)
        if not ok then Log("loop error: %s", tostring(stop)) end
        if stop == true then done = true end
        return done
    end)
end

local function load_module(name)
    local ok, mod = pcall(require, name)
    if ok and type(mod) == "table" then return mod end
    local src = ((debug and debug.getinfo and debug.getinfo(1, "S").source) or ""):gsub("^@", "")
    local dir = src:match("^(.*)[/\\]") or "."
    for _, path in ipairs({ dir .. "/" .. name .. ".lua", dir .. "/../../shared/" .. name .. ".lua" }) do
        local ok2, mod2 = pcall(dofile, path)
        if ok2 and type(mod2) == "table" then return mod2 end
    end
    return nil
end
local HW = load_module("hsmp_wg")
if not HW then Log("FATAL: hsmp_wg.lua missing"); return end
local WG = HW.new({ log = Log, UEHelpers = UEHelpers })
pcall(function() RegisterLoadMapPreHook(function() pcall(WG.travel, "LoadMap") end) end)

local STATE_DIR = (trim(os.getenv("HSMP_STATE_DIR")) or "hsmp_state"):gsub("\\", "/")
local N = tonumber(trim(os.getenv("HSMP_IO_STRESS")) or "") or 0
local MODE = trim(os.getenv("HSMP_IO_MODE")) or "plain"
local ARENA = trim(os.getenv("HSMP_IO_ARENA")) or "Map_Arena_Pit"
local DELAY_S = (tonumber(trim(os.getenv("HSMP_IO_DELAY_MS")) or "") or 100) / 1000
local REVEAL_S = (tonumber(trim(os.getenv("HSMP_IO_REVEAL_MS")) or "") or 3000) / 1000
local HOLD_S = 4.0
-- HSMP_IO_VIA_MENU=1: go back to the main menu between loads, so every load is
-- a menu -> arena load (the only kind that has crashed).
local VIA_MENU = (trim(os.getenv("HSMP_IO_VIA_MENU")) or "") == "1"
local MENU = "Map_Menu_Startup"
-- "name value;name value": console variables set in every new world (a fix candidate under test).
local CVARS = trim(os.getenv("HSMP_IO_CVARS")) or ""
-- HSMP_IO_HIDE_DELAY_MS: hide this long after BeginPlay instead of in the hook
-- (the groom renders for a few frames first; a fix candidate).
local HIDE_DELAY_S = (tonumber(trim(os.getenv("HSMP_IO_HIDE_DELAY_MS")) or "") or 0) / 1000
local FILE = STATE_DIR .. "/.io_stress.txt"

local function done_n()
    local n = 0
    local f = io.open(FILE, "rb")
    if f then for _ in f:lines() do n = n + 1 end; f:close() end
    return n
end
local function mark(i, detail)
    local f = io.open(FILE, "ab")
    if f then f:write(string.format("%d %d %s %s\n", i, os.time(), MODE, detail)); f:close() end
end

local function open_level(name)
    local ok = false
    pcall(function()
        local gs = UEHelpers.GetGameplayStatics()
        local w = WG.world()
        if gs and gs:IsValid() and w then gs:OpenLevel(w, FName(name), true, FString("")); ok = true end
    end)
    return ok
end

-- Willie FName -> clock when BeginPlay hid it; dropped on every world change.
local hidden, to_hide = {}, {}
pcall(function() RegisterLoadMapPreHook(function() hidden, to_hide = {}, {} end) end)
if MODE ~= "plain" then
    RegisterBeginPlayPostHook(function(ctx)
        local a; pcall(function() a = ctx:get() end)
        if not a then return end
        local cn; pcall(function() cn = a:GetClass():GetFName():ToString() end)
        if cn ~= "Willie_BP_C" then return end
        local full = ""; pcall(function() full = a:GetFullName() end)
        if not full:find("Map_Arena_") then return end
        local n = a:GetFName():ToString()
        if n == "Willie_BP_C_0" then return end
        if HIDE_DELAY_S > 0 then to_hide[n] = os.clock() + HIDE_DELAY_S; return end
        pcall(function() a:SetActorHiddenInGame(true) end)
        hidden[n] = os.clock()
    end)
end

local function apply_cvars()
    if CVARS == "" then return "" end
    local out = {}
    pcall(function()
        local ksl = UEHelpers.GetKismetSystemLibrary()
        local w = WG.world()
        for cv in CVARS:gmatch("[^;]+") do
            local name = cv:match("^%s*(%S+)")
            ksl:ExecuteConsoleCommand(w, FString(trim(cv)), nil)
            local v = "?"
            pcall(function() v = tostring(ksl:GetConsoleVariableIntValue(FString(name))) end)
            out[#out + 1] = name .. "=" .. v
        end
    end)
    return table.concat(out, ",")
end

local function player()
    local p
    pcall(function() local pc = WG.pc(); p = pc and pc.Pawn end)
    if p and p:IsValid() then return p end
    return nil
end

local function reveal_all()
    local n = 0
    for _, w in pairs(FindAllOf("Willie_BP_C") or {}) do
        local nm = "?"
        pcall(function() nm = w:GetFName():ToString() end)
        if hidden[nm] then
            pcall(function() w:SetActorHiddenInGame(false) end)
            hidden[nm] = nil
            n = n + 1
        end
    end
    return n
end

local function hair_op()
    local p = player()
    if not p then return "no pawn" end
    if MODE == "destroy" then
        local ok, e = pcall(function() p["Destroy Hair"](p) end)
        return ok and "Destroy Hair" or ("Destroy Hair FAIL " .. tostring(e))
    elseif MODE == "armor" then
        pcall(function() p["Spawn in Pants"] = false end)
        pcall(function() p["Blossfechten Gear"] = false end)
        local ok, e = pcall(function() p["Set Up Armor"](p, true, true) end)
        return ok and "Set Up Armor" or ("Set Up Armor FAIL " .. tostring(e))
    end
    return "none"
end

if N <= 0 then Log("idle (HSMP_IO_STRESS not set)"); return end
Log("STRESS: %d loads of %s, mode %s%s, hide delay %.0f ms, reveal after %.0f ms, op %.0f ms after it, cvars [%s]; %d done before this boot",
    N, ARENA, MODE, VIA_MENU and " via menu" or "", HIDE_DELAY_S * 1000, REVEAL_S * 1000, DELAY_S * 1000, CVARS, done_n())

local function hide_due(now)
    if next(to_hide) == nil then return end
    for _, w in pairs(FindAllOf("Willie_BP_C") or {}) do
        local nm = "?"
        pcall(function() nm = w:GetFName():ToString() end)
        local due = to_hide[nm]
        if due and now >= due then
            pcall(function() w:SetActorHiddenInGame(true) end)
            to_hide[nm] = nil
            hidden[nm] = now
        end
    end
end

local st = { phase = "wait", t = os.clock() }
loop(16, function()
    local now = os.clock()
    if WG.check() then pcall(hide_due, now) end
    if not WG.check() then st.phase, st.t = "wait", now; return false end
    local done = done_n()
    if done >= N then
        if st.phase ~= "finished" then st.phase = "finished"; Log("STRESS finished: %d loads", done) end
        return false
    end
    if WG.short() ~= ARENA then
        if now - st.t > 6 then st.t = now; Log("STRESS: in %s -> open %s", tostring(WG.short()), ARENA); open_level(ARENA) end
        return false
    end
    if st.phase == "wait" then
        st.phase, st.t, st.note = (MODE == "plain") and "hold" or "hidden", now, ""
        local cv = apply_cvars()
        if cv ~= "" then st.note = "cvars " .. cv .. " " end
        return false
    end
    if st.phase == "hidden" then
        if now - st.t < REVEAL_S then return false end
        st.note = string.format("revealed=%d", reveal_all())
        st.phase, st.t = "op", now
        return false
    end
    if st.phase == "op" then
        if now - st.t < DELAY_S then return false end
        st.note = st.note .. " op=" .. hair_op()
        st.phase, st.t = "hold", now
        return false
    end
    if st.phase == "hold" then
        if now - st.t < HOLD_S then return false end
        mark(done + 1, st.note)
        Log("STRESS #%d done (%s); reloading", done + 1, st.note)
        st.phase, st.t = "wait", now
        open_level(VIA_MENU and MENU or ARENA)
    end
    return false
end)
