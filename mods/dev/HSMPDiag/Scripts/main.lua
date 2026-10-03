-- HSMPDiag (dev only) — on entering an arena, dumps every reflected property
-- of the GameInstance, GameMode and BP_LevelManager to
-- <state_dir>/.state_dump_<epoch>.txt so two runs can be diffed.

local UEHelpers = require("UEHelpers")

if LoopInGameThreadWithDelay and ExecuteInGameThreadWithDelay and CancelDelayedAction then
    ExecuteWithDelay = function(ms, fn) return ExecuteInGameThreadWithDelay(ms, fn) end
    LoopAsync = function(ms, fn)
        local h
        h = LoopInGameThreadWithDelay(ms, function()
            local ok, stop = pcall(fn)
            if ok and stop == true and h then CancelDelayedAction(h) end
        end)
        return h
    end
end

local function Log(fmt, ...) print(string.format("[HSMPDiag] " .. fmt .. "\n", ...)) end
local function trim(s) return s and s:match("^%s*(.-)%s*$") or nil end
local STATE_DIR = (trim(os.getenv("HSMP_STATE_DIR")) or "hsmp_state"):gsub("\\", "/")

local function valstr(v)
    local t = type(v)
    if t == "number" or t == "boolean" or t == "string" then return tostring(v) end
    if t == "userdata" then
        local s
        pcall(function() s = v:ToString() end)                    -- FName/FString/FText
        if s then return s end
        pcall(function() if v:IsValid() then s = v:GetFullName() end end)
        if s then return s end
        pcall(function() s = "#" .. tostring(v:GetArrayNum()) end) -- TArray length
        return s or "<userdata>"
    end
    return "<" .. t .. ">"
end

local SAFE_TYPES = {
    BoolProperty = true, ByteProperty = true, EnumProperty = true,
    IntProperty = true, Int64Property = true, UInt32Property = true,
    FloatProperty = true, DoubleProperty = true,
    NameProperty = true, StrProperty = true, TextProperty = true,
    ObjectProperty = true,
}

local function dump_obj(w, label, obj)
    if not obj or not obj:IsValid() then w(label .. ": <none>"); return end
    w(string.format("== %s: %s", label, obj:GetFullName()))
    local cls = obj:GetClass()
    local seen = 0
    while cls and cls:IsValid() and seen < 8 do
        pcall(function()
            cls:ForEachProperty(function(prop)
                local name = prop:GetFName():ToString()
                local pt = "?"
                pcall(function() pt = prop:GetClass():GetFName():ToString() end)
                -- Reading SoftObject/SoftClass properties crashes UE4SS 3.0.1
                -- (push_softobjectproperty memcpy); containers/structs are
                -- noisy. Only read plain scalars, names, strings and objects.
                if SAFE_TYPES[pt] then
                    local ok, v = pcall(function() return obj[name] end)
                    w(string.format("  %s = %s", name, ok and valstr(v) or "<err>"))
                else
                    w(string.format("  %s = <%s skipped>", name, pt))
                end
            end)
        end)
        local sup; pcall(function() sup = cls:GetSuperStruct() end)
        cls = sup; seen = seen + 1
        -- Stop at engine base classes (explicit compares: Lua patterns have no "|").
        local cn
        if cls then pcall(function() if cls:IsValid() then cn = cls:GetFName():ToString() end end) end
        if cn == "GameInstance" or cn == "GameModeBase" or cn == "Actor" then break end
    end
end

-- World guard (shared/hsmp_wg.lua): a delayed dump must never run
-- against a world that a level load (round reset re-opens the same map) has
-- freed in the meantime. Each delayed dump captures WG.token(), re-checks it
-- with WG.same() and fetches every object fresh; no UObject is cached.
-- shared/*.lua is copied into Scripts/ at deploy; fall back to the source tree.
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
if not HW then
    Log("FATAL: shared/hsmp_wg.lua missing - HSMPDiag disabled (deploy copies shared/*.lua)")
    return
end
local WG = HW.new({ log = Log, UEHelpers = UEHelpers })
-- Any travel path (console "open", ServerTravel, native BP) ends in LoadMap: drop there too.
pcall(function() RegisterLoadMapPreHook(function() pcall(WG.travel, "LoadMap") end) end)

-- A bounded number of dump pairs per process: every round reset is a world
-- change, so an unbounded dumper would write two files per round forever.
local MAX_DUMPS = tonumber(trim(os.getenv("HSMP_DIAG_MAX_DUMPS")) or "") or 6
local dumps = 0
local last_world
LoopAsync(500, function()
    if not WG.check() then return false end
    local key, wn = WG.key, WG.name
    if not key or key == last_world then return false end
    last_world = key
    if not wn or not wn:find("Map_Arena_") then return false end
    if dumps >= MAX_DUMPS then
        if dumps == MAX_DUMPS then dumps = dumps + 1; Log("dump budget (%d) used; no more state dumps", MAX_DUMPS) end
        return false
    end
    dumps = dumps + 1
    local tok = WG.token()
    ExecuteWithDelay(400, function()
        if not WG.same(tok) then return end
        local path = STATE_DIR .. "/.state_dump_" .. os.time() .. ".txt"
        local f = io.open(path, "wb"); if not f then return end
        local function w(s) f:write(s, "\n") end
        w("world: " .. wn)
        dump_obj(w, "GameInstance", UEHelpers.GetGameInstance())
        local gm; pcall(function() gm = UEHelpers.GetGameplayStatics():GetGameMode(UEHelpers.GetWorld()) end)
        dump_obj(w, "GameMode", gm)
        local lm; pcall(function() for _, x in pairs(FindAllOf("BP_LevelManager_C") or {}) do lm = x; break end end)
        dump_obj(w, "LevelManager", lm)
        f:close()
        Log("state dump written: %s", path)
    end)
    ExecuteWithDelay(3000, function()
        if not WG.same(tok) then return end
        local pc = UEHelpers.GetPlayerController()
        local pawn = pc and pc:IsValid() and pc.Pawn or nil
        if not pawn or not pawn:IsValid() then Log("no pawn to dump"); return end
        local path = STATE_DIR .. "/.willie_dump_" .. os.time() .. ".txt"
        local f = io.open(path, "wb"); if not f then return end
        dump_obj(function(s) f:write(s, "\n") end, "Willie", pawn)
        f:close()
        Log("willie dump written: %s", path)
    end)
    return false
end)

-- There is deliberately no file-driven dev console: the mod never executes code
-- read from a file (see docs/development/ipc-shared-memory.md).

Log("loaded (dev): dumps GI/GameMode/LevelManager and the local Willie on arena entry")
