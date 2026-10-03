-- HSMPMenu / legacy_travel.lua — COMPATIBILITY SHIM. DELETE THIS FILE once the
-- Director (HSMPMatch/Scripts/director.lua) is proven in-game, and drop its
-- allow-list entry in tools/hsmp-tools/src/bin/check_travel; nothing else
-- needs to change.
--
-- Opt-in: main.lua loads this file only with HSMP_LEGACY_TRAVEL=1 (off by
-- default) and logs "!!! LEGACY TRAVEL ..." when it is armed and every time
-- it travels.
--
-- It holds the menu's level-travel and GameInstance code so that main.lua
-- never travels or writes GI itself. travel.lua calls it only when a travel
-- request got no Director acknowledgement (log line "DIRECTOR MISSING -
-- legacy travel"). It is the ONLY HSMPMenu file allowed to call OpenLevel /
-- console "open" and to write GI values; tools/hsmp-tools/lua-tests/menu_ui.lua
-- asserts that.
--
-- Client-local decisions kept here on purpose (they belong to the Director's
-- spawn pipeline, see docs/development/subsystems/director-contract.md):
--   * GI profile writes + one-time backup/restore (.gi_backup.txt)
--   * native foe count = remote peers reported by the sidecar (not the roster)
--   * dismissing the menu widget and forcing game-only input

local L = {}

local ctx     -- { state_dir, log, state (main.lua's widget state), remote_peers(), ev(name, fields) }
local Log = function() end
local UEHelpers

local LOBBY_MAP = os.getenv("HSMP_LOBBY_MAP") or "Map_Arena_Alley"
local LOBBY_MAP_PATHS = {
    "/Game/Maps/Arenas/Map_Arena_Alley/Map_Arena_Alley",
    "/Game/Maps/Arenas/Map_Arena_Yard/Map_Arena_Yard",
    "/Game/Maps/Arenas/Map_Arena_Pit/Map_Arena_Pit",
    "/Game/Maps/Arenas/Map_Arena_Cellar/Map_Arena_Cellar",
    "/Game/Maps/Arenas/Map_Arena_Slums/Map_Arena_Slums",
    "/Game/Maps/Arenas/Map_Arena_LordsHall/Map_Arena_LordsHall",
    "/Game/Maps/Arenas/Map_Arena_EastTower/Map_Arena_EastTower",
}

local function gi_backup_file() return ctx.state_dir .. "/.gi_backup.txt" end

-- Write a GI property and confirm it stuck (a wrong name is a silent no-op).
local function gi_set(gi, name, v)
    pcall(function() gi[name] = v end)
    local rb; pcall(function() rb = gi[name] end)
    return rb == v
end

local function gi_value_str(v)
    if type(v) == "boolean" or type(v) == "number" then return tostring(v) end
    return nil
end

-- Record the single-player values we're about to override, once per MP
-- session. "Player Just Died" is never restored.
local function gi_backup_once(gi, values)
    local f = io.open(gi_backup_file(), "rb")
    if f then f:close(); return end
    local lines = {}
    for _, kv in ipairs(values) do
        if kv[1] ~= "Player Just Died" then
            local cur; pcall(function() cur = gi[kv[1]] end)
            local s = gi_value_str(cur)
            if s then table.insert(lines, kv[1] .. "\t" .. s) end
        end
    end
    local w = io.open(gi_backup_file(), "wb")
    if w then w:write(table.concat(lines, "\n"), "\n"); w:close() end
    Log("GI single-player values backed up (%d)", #lines)
end

function L.gi_restore_backup(reason)
    local f = io.open(gi_backup_file(), "rb"); if not f then return end
    local gi = UEHelpers.GetGameInstance()
    if not gi or not gi:IsValid() then f:close(); return end
    local n = 0
    for line in f:lines() do
        local name, val = line:match("^(.-)\t(.+)$")
        if name then
            local v = (val == "true") or (val ~= "false" and tonumber(val))
            if val == "false" then v = false end
            if v ~= nil and gi_set(gi, name, v) then n = n + 1 end
        end
    end
    f:close()
    os.remove(gi_backup_file())
    Log("GI single-player values restored (%d) - %s", n, reason)
end

-- Known-good arena GI values (spaces in the real property names).
local function set_gi_for_map(chosen)
    local gi = UEHelpers.GetGameInstance()
    if not gi or not gi:IsValid() then Log("set_gi_for_map: no GameInstance"); return end
    local foes = math.max(1, (ctx.remote_peers and ctx.remote_peers()) or 1)   -- one native foe per remote peer
    local mp_values = {
        { "Player Just Died",       false },
        { "Current Game Mode Enum", 0 },
        { "Current Combat Mode",    0 },
        { "Current Play Mode",      0 },
        { "Free Mode Activated",    false },
        { "Rounds To WIn",          0 },
        { "Rounds Won",             0 },
        { "Free Mode Foes Amount",  foes },
        { "Free Mode Carnage",      false },
        { "Free Mode Brawling",     false },
        { "Free Mode Blossfechten", false },
    }
    gi_backup_once(gi, mp_values)
    local bad = {}
    for _, kv in ipairs(mp_values) do
        if not gi_set(gi, kv[1], kv[2]) then table.insert(bad, kv[1]) end
    end
    Log("set_gi_for_map: arena %s foes=%d (%s)", chosen, foes,
        #bad == 0 and "all GI writes verified" or ("FAILED: " .. table.concat(bad, ", ")))
end

local function force_game_input(pc)
    if not pc or not pc:IsValid() then return end
    local wbl = StaticFindObject("/Script/UMG.Default__WidgetBlueprintLibrary")
    if wbl and wbl:IsValid() then pcall(function() wbl:SetInputMode_GameOnly(pc, false) end) end
    pcall(function() pc:SetShowMouseCursor(false) end)
    pcall(function() pc:SetIgnoreLookInput(false) end)
    pcall(function() pc:SetIgnoreMoveInput(false) end)
    Log("force_game_input: applied to PC")
end

local function dismiss_menu()
    local st = ctx.state
    if st and st.injected and st.menu and st.menu:IsValid() then
        pcall(function() st.menu:RemoveFromViewport() end)
        pcall(function() st.menu:SetVisibility(1) end)
        Log("dismiss_menu: removed from viewport")
    end
end

local function world_short()
    local wn; pcall(function() local w = UEHelpers.GetWorld(); if w and w:IsValid() then wn = w:GetFullName() end end)
    return wn and (wn:match("([^/%.%s]+)$") or wn) or "?"
end

local function open_level(name)
    local world = UEHelpers.GetWorld()
    local gs = UEHelpers.GetGameplayStatics()
    local loaded = false
    if gs and gs:IsValid() and world and world:IsValid() then
        pcall(function() gs:OpenLevel(world, FName(name), true, FString("")); loaded = true end)
    end
    return loaded
end

local function open_lobby_map(map_name)
    local chosen = (map_name and map_name ~= "") and map_name or LOBBY_MAP
    local pc = UEHelpers.GetPlayerController()
    if not pc or not pc:IsValid() then Log("open_lobby_map: no PlayerController"); return end
    set_gi_for_map(chosen)
    dismiss_menu()
    local from = world_short()
    local loaded = open_level(chosen)
    if loaded then
        Log("open_lobby_map: OpenLevel('%s') OK", chosen)
    else
        for _, path in ipairs(LOBBY_MAP_PATHS) do
            if path:match(chosen) then
                pcall(function() pc:ConsoleCommand(FString("open " .. path), false); loaded = true end)
                if loaded then Log("open_lobby_map: console 'open %s'", path); break end
            end
        end
    end
    if loaded and ctx.ev then ctx.ev("travel", { from = from, to = chosen, by = "menu_legacy" }) end
    ExecuteWithDelay(2000, function() force_game_input(UEHelpers.GetPlayerController()) end)
    ExecuteWithDelay(4000, function() force_game_input(UEHelpers.GetPlayerController()) end)
end

-- Entry point used by travel.lua's fallback (req = the travel request).
function L.travel(req)
    if req.want == "menu" then
        local from = world_short()
        if open_level("Map_Menu_Startup") then
            Log("legacy travel: OpenLevel('Map_Menu_Startup') (%s)", tostring(req.reason))
            if ctx.ev then ctx.ev("travel", { from = from, to = "Map_Menu_Startup", by = "menu_legacy" }) end
        end
        return
    end
    open_lobby_map(req.arena)
end

function L.init(c)
    ctx = c
    Log = c.log or Log
    UEHelpers = c.UEHelpers or require("UEHelpers")
end

return L
