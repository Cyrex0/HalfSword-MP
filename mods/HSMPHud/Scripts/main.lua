-- HSMPHud — the in-match HUD (display only). Contract and mocks:
-- docs/development/subsystems/hud.md.
--
-- Shows: round banner + timer (top centre), score strip, own HP/ST bars
-- (bottom left), every opponent's HP/ST (top left), kill feed (top right,
-- from the `death` records), scoreboard (hold TAB), net indicator (bottom right) and
-- the centre phase messages (WAITING FOR PLAYERS, ROUND n, FIGHT!, ROUND
-- OVER, YOU DIED, SPECTATING <NAME>, VICTORY, RECONNECTING, ...). HSMPHud is
-- the only centre-banner owner; HSMP_HUD_BANNER=0 turns the centre messages
-- off. Server notices (joined / left / host left / failed to load) are toasts
-- in the feed.
--
-- The HUD itself only reads state (hud_state.lua) and never decides
-- anything; every HUD widget is HitTestInvisible. The interactive panels
-- (hud_panel.lua) are separate viewport widgets that exist only while open:
-- the connection modal (conn_state bus record: RECONNECT / BACK TO MENU / OK),
-- the MP pause (Esc in an MP arena: the native pause is suppressed and the
-- world never pauses) and the match result (REMATCH / BACK TO LOBBY). Their
-- buttons write the `ui_request` bus key for the Director. Keys: TAB only.
--
-- Host: preferred inside the native UI_HUD_C (its WidgetTree root canvas, so
-- it lives and dies with the game's own HUD); fallback: an own UserWidget in
-- the viewport at z 990. Hidden outside arenas, in the native pause / photo
-- mode / settings menus, and when no MP match runs.

local UEHelpers = require("UEHelpers")

-- Thread-safety shim (same as every HSMP mod). LoopAsync, ExecuteWithDelay and
-- *Async keybind callbacks run on UE4SS worker threads in this build; running
-- them alongside game-thread Lua corrupted the Lua VM. Route every deferred,
-- looped and key-bound callback onto the game thread.
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
    print(string.format("[HSMPHud] " .. fmt .. "\n", ...))
end

local Kit   = require("hud_kit")
local State = require("hud_state")
local Model = require("hud_model")
local View  = require("hud_view")
local Panel = require("hud_panel")

local function trim(s) return s and s:match("^%s*(.-)%s*$") or nil end
local STATE_DIR = (trim(os.getenv("HSMP_STATE_DIR")) or "hsmp_state"):gsub("\\", "/")
-- HSMPHud is the only centre-banner owner; HSMP_HUD_BANNER=0 turns the
-- centre messages off (debugging only).
local CENTRE_BANNER = trim(os.getenv("HSMP_HUD_BANNER")) ~= "0"

local TICK_MS          = 100     -- render / fade / Tab poll
local READ_EVERY       = 2       -- state reads every 2 ticks (~5 Hz)
local MENU_EVERY       = 1       -- one native menu class per tick (round-robin, ~1.1 Hz each)
local HOST_CHECK_EVERY = 10      -- native host liveness + canvas size (~1 Hz)
local NATIVE_WAIT_S    = 3.0     -- wait this long for UI_HUD_C before the viewport fallback
local VIEWPORT_Z       = 990
local NATIVE_Z         = 500
local TAB_TOGGLE_MAX_S = 8.0     -- toggle fallback (key state not readable): auto-close

-- Native menus that hide the HUD while they are in the viewport.
local MENU_WIDGETS = { "UI_Pause_C", "UI_Pause_Eng_C", "UI_PhotoMode_C", "UI_Gallery_C", "UI_KeyBinds_C",
                       "UI_GameSettings_C", "UI_DisplaySettings2_C", "UI_AudioSettings_C", "UI_Controls_C" }

-- --- world guard (shared/hsmp_wg.lua) -------------------------------------------
-- A level change frees every actor, component and world-outered widget of the
-- old world, and the round reset re-opens the same arena (OpenLevel). Touching
-- a freed UObject afterwards (a property write, a call, even IsValid(), which
-- reads the freed object) is an access violation pcall cannot catch: it shows
-- up as a crash in UStruct::FindProperty <- __newindex from a delayed callback.
-- Rule for every UObject kept beyond one callback (see shared/hsmp_wg.lua):
--   * wg_check() runs first in every loop/frame callback; on a world change
--     every cache is dropped without touching the old objects (wg_on_drop
--     handlers only reset Lua references) and the callback skips this tick;
--   * delayed one-shots capture wg_token() and test wg_same(token) first.
-- The shared guard installs the OpenLevel / OpenLevelBySoftObjectPtr pre-hooks.
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
-- HSMP-SHM facade (shared/hsmp_ipc.lua), published as the per-state global
-- HSMP_IPC; this mod's state-dir helpers route through it.
do
    local m = load_module("hsmp_ipc")
    if m then m.init({ mod = "HSMPHud", state_dir = STATE_DIR, log = Log }) end
end

-- shared/hsmp_ui_scale.lua: the one UI scaling rule (also used by HSMPMenu).
local UIS = load_module("hsmp_ui_scale")
if UIS then Kit.set_uis(UIS) else Log("WARNING: shared/hsmp_ui_scale.lua missing - HUD uses the fallback scale") end

local HW = load_module("hsmp_wg")
if not HW then
    Log("FATAL: shared/hsmp_wg.lua missing - %s disabled (deploy copies shared/*.lua)", "HSMPHud")
    return
end
local WG = HW.new({ log = Log, UEHelpers = UEHelpers })
-- The shared liveness rule (shared/hsmp_session.lua): "connected" plus
-- an observed heartbeat. Its verdict rides in every snapshot (snap.sess).
local SESS = (function() local H = load_module("hsmp_session"); return H and H.new({ every_s = 0 }) end)()
if not SESS then Log("WARNING: shared/hsmp_session.lua missing - HUD liveness falls back to its own tracker") end
local wg_check, wg_on_drop, wg_token, wg_same = WG.check, WG.on_drop, WG.token, WG.same
-- The HUD's widgets are outered to the old world: also drop on LoadMap.
pcall(function() RegisterLoadMapPreHook(function() pcall(WG.travel, "LoadMap") end) end)

-- true only inside a tick that passed wg_check this frame (the kit's guard)
local in_good_tick = false
local function world_ok() return in_good_tick and WG.key ~= nil and WG.travel_from == nil end

-- --- construction helpers -----------------------------------------------------------

local function construct(class_path, outer, name)
    if outer == nil then return nil end   -- never construct into a nil / freed outer
    local cls = StaticFindObject(class_path)
    if not cls or not cls:IsValid() then return nil end
    local ok, obj
    if name then
        ok, obj = pcall(StaticConstructObject, cls, outer, FName(name, FNAME_Add))
    else
        ok, obj = pcall(StaticConstructObject, cls, outer)
    end
    if ok and obj and obj:IsValid() then return obj end
    return nil
end

Kit.init({ construct = construct, Log = Log, world_ok = world_ok })
-- Panels are polled from their own 33 ms loop, outside the HUD tick: their
-- guard is the world key they were built in.
local panel_world = nil
Panel.init({ construct = construct, Log = Log, UEHelpers = UEHelpers,
             world_ok = function() return WG.key ~= nil and WG.key == panel_world and WG.travel_from == nil end })

local function short_name(s) return s and s:match("([%w_]+)$") or nil end

-- Canvas size in slate units (the DPI scale divides the viewport) + the DPI
-- scale. Fresh lookups (shared/hsmp_ui_scale.lua probe: the layout library's
-- GetViewportSize / GetViewportScale; GameViewportClient is C++-only).
-- Game thread only.
local last_src = nil
local function canvas_size()
    local vw, vh, dpi, src
    if UIS then
        local ok, M = pcall(UIS.measure, UEHelpers, WG.world())
        if ok and type(M) == "table" then vw, vh, dpi, src = M.vw, M.vh, M.dpi, M.src end
    end
    if not vw then
        vw, vh, src = 1920, 1080, "no-lib"
        pcall(function()
            local wll = StaticFindObject("/Script/UMG.Default__WidgetLayoutLibrary")
            if wll and wll:IsValid() then dpi = wll:GetViewportScale(WG.world()) end
        end)
        if type(dpi) ~= "number" or dpi <= 0.05 then dpi = 1 end
        vw, vh = vw * dpi, vh * dpi
    end
    if src ~= last_src then
        last_src = src
        Log("viewport %.0fx%.0f dpi %.3f [%s]", vw, vh, dpi, tostring(src))
    end
    return math.floor(vw / dpi + 0.5), math.floor(vh / dpi + 0.5), math.floor(dpi * 1000 + 0.5) / 1000
end

-- --- host ----------------------------------------------------------------------------------
-- host = { kind = "native"|"viewport", tree, canvas, root = {w, shown}, hud_addr, cw, ch }
-- World-scoped: dropped (untouched) by the world guard.

local host, view = nil, nil
local world_seen_at = nil
local native_failed = false
local stats = { native = 0, viewport = 0, lost = 0, rebuilds = 0 }
-- The UI input mode (cursor) the panels applied, and the world key it
-- was applied in. Input state belongs to a world's PlayerController: a world
-- change resets it here without any call (the old PC is gone, the next world
-- starts from its own mode), so the menu never gets a GameOnly / no-cursor.
local panel_input, panel_input_world = false, nil

wg_on_drop(function()
    Kit.forget()
    Panel.forget()
    host, view = nil, nil
    world_seen_at = nil
    native_failed = false
    panel_input, panel_input_world = false, nil
end)

local function addr_of(o)
    local a; pcall(function() a = o:GetAddress() end)
    return a
end

-- A fresh UI_HUD_C that is on screen and owned by the current controller.
local function find_native_hud()
    local found
    pcall(function()
        local pc = WG.pc()
        local pca = pc and pc:IsValid() and addr_of(pc) or nil
        for _, h in pairs(FindAllOf("UI_HUD_C") or {}) do
            if h and h:IsValid() then
                local inv = false
                pcall(function() inv = h:IsInViewport() end)
                if inv then
                    local owner_ok = true
                    if pca then
                        pcall(function()
                            local op = h:GetOwningPlayer()
                            if op and op:IsValid() then owner_ok = (addr_of(op) == pca) end
                        end)
                    end
                    if owner_ok then found = h end
                end
            end
        end
    end)
    return found
end

local function build_view(kind, cw, ch, dpi)
    view = View.build(cw, ch, dpi)
    host.cw, host.ch, host.dpi = cw, ch, dpi
    stats[kind] = stats[kind] + 1
    Log("HUD host: %s (canvas %dx%d dpi %.3f scale %.2f)", kind == "native" and "inside native UI_HUD_C" or
        "viewport fallback z " .. VIEWPORT_Z, cw, ch, dpi or 1, view and view.L and view.L.s or Kit.ui_scale(cw, ch))
end

local function build_native(hud)
    local tree, root
    pcall(function() tree = hud.WidgetTree end)
    pcall(function() root = tree and tree.RootWidget or nil end)
    local cls
    pcall(function() cls = root:GetClass():GetFName():ToString() end)
    if not (tree and root and cls == "CanvasPanel") then
        Log("UI_HUD_C root is %s, not a CanvasPanel -> viewport fallback", tostring(cls))
        return false
    end
    Kit.serial = Kit.serial + 1
    local canvas = construct("/Script/UMG.CanvasPanel", tree, "HSMPHUD_root_" .. Kit.serial)
    if not canvas then return false end
    local slot
    pcall(function() slot = root:AddChildToCanvas(canvas) end)
    if not slot then return false end
    pcall(function() slot:SetAnchors({ Minimum = { X = 0, Y = 0 }, Maximum = { X = 1, Y = 1 } }) end)
    pcall(function() slot:SetOffsets({ Left = 0, Top = 0, Right = 0, Bottom = 0 }) end)
    pcall(function() slot:SetZOrder(NATIVE_Z) end)
    pcall(function() canvas:SetVisibility(3) end)            -- HitTestInvisible: never takes input
    host = { kind = "native", tree = tree, canvas = canvas, hud_addr = addr_of(hud) }
    host.root = { w = canvas, shown = true }
    Kit.begin(host)
    pcall(function() Kit.font_src = hud.TextWin end)
    local cw, ch, dpi = canvas_size()
    build_view("native", cw, ch, dpi)
    return true
end

local function build_viewport()
    local world = WG.world()
    if not world or not world:IsValid() then return false end
    Kit.serial = Kit.serial + 1
    local uw = construct("/Script/UMG.UserWidget", world, "HSMPHUD_uw_" .. Kit.serial)
    if not uw then return false end
    local tree = construct("/Script/UMG.WidgetTree", uw, "HSMPHUD_wt_" .. Kit.serial)
    if not tree then return false end
    uw.WidgetTree = tree
    local canvas = construct("/Script/UMG.CanvasPanel", tree, "HSMPHUD_root_" .. Kit.serial)
    if not canvas then return false end
    tree.RootWidget = canvas
    host = { kind = "viewport", tree = tree, canvas = canvas, uw = uw }
    host.root = { w = uw, shown = true }
    Kit.begin(host)
    local cw, ch, dpi = canvas_size()
    build_view("viewport", cw, ch, dpi)           -- populate BEFORE AddToViewport (blank screen otherwise)
    pcall(function() uw:AddToViewport(VIEWPORT_Z) end)
    pcall(function() uw:SetVisibility(3) end)                -- HitTestInvisible
    return true
end

-- Remove our widgets (same world only: resolution change / native HUD swap).
local function teardown(why)
    if host and Kit.alive() then
        if host.kind == "native" then pcall(function() host.canvas:RemoveFromParent() end)
        else pcall(function() host.uw:RemoveFromParent() end) end
    end
    Log("HUD rebuild: %s", why)
    stats.rebuilds = stats.rebuilds + 1
    Kit.forget()
    host, view = nil, nil
end

local function ensure_host(now)
    if host then return true end
    if not native_failed then
        local hud = find_native_hud()
        if hud then
            if build_native(hud) then return true end
            Kit.forget(); host, view = nil, nil
            native_failed = true
        elseif now - (world_seen_at or now) < NATIVE_WAIT_S then
            return false                          -- the game's HUD may still be on its way
        else
            native_failed = true
            Log("no UI_HUD_C in the viewport after %.1f s -> viewport fallback", NATIVE_WAIT_S)
        end
    end
    if build_viewport() then return true end
    Kit.forget(); host, view = nil, nil
    return false
end

-- Native host still on screen? Fresh FindAllOf: our cached HUD is only ever
-- compared by address, never touched unless it is in that fresh list.
local function check_host()
    if not host then return end
    if host.kind == "native" then
        local alive = false
        pcall(function()
            for _, h in pairs(FindAllOf("UI_HUD_C") or {}) do
                if h and h:IsValid() and addr_of(h) == host.hud_addr then
                    local inv = false
                    pcall(function() inv = h:IsInViewport() end)
                    alive = inv
                end
            end
        end)
        if not alive then
            stats.lost = stats.lost + 1
            Log("native UI_HUD_C left the viewport -> refs dropped, viewport fallback for this world")
            Kit.forget(); host, view = nil, nil
            native_failed = true
            return
        end
    end
    local cw, ch, dpi = canvas_size()
    if cw ~= host.cw or ch ~= host.ch or dpi ~= host.dpi then
        teardown(string.format("canvas %dx%d@%.3f -> %dx%d@%.3f", host.cw or 0, host.ch or 0, host.dpi or 0, cw, ch, dpi))
    end
end

-- No native menu widget was ever created in this process -> nothing can be
-- on screen; skip the 9 FindAllOf scans (set by the UserWidget creation hook).
local menu_ever, pause_ever = false, false
local MENU_SET = {}
for _, c in ipairs(MENU_WIDGETS) do MENU_SET[c] = true end
-- One class per call, round-robin: walking all 9 classes at once causes a
-- periodic hitch. A class whose widget was just created (the
-- creation hook sets menu_hint) is scanned first. menu_state keeps each
-- class's last verdict (plain booleans, no UObject).
local menu_state, menu_hint, menu_rr = {}, {}, 0
local function scan_menu_class(cls)
    local open = false
    pcall(function()
        for _, w in pairs(FindAllOf(cls) or {}) do
            if w and w:IsValid() then
                local inv = false
                pcall(function() inv = w:IsInViewport() end)
                if inv then open = true end
            end
        end
    end)
    menu_state[cls] = open
end
local menu_key = nil
local function menu_open()
    if not menu_ever then return false end
    -- a new world: the old world's verdicts mean nothing (rescan from scratch)
    if WG.key ~= menu_key then
        menu_key = WG.key
        for k in pairs(menu_state) do menu_state[k] = nil end
    end
    local cls = next(menu_hint)
    if cls then
        menu_hint[cls] = nil
    else
        menu_rr = menu_rr % #MENU_WIDGETS + 1
        cls = MENU_WIDGETS[menu_rr]
    end
    scan_menu_class(cls)
    for _, c in ipairs(MENU_WIDGETS) do if menu_state[c] then return true end end
    return false
end

-- --- TAB scoreboard -------------------------------------------------------------------------
-- RegisterKeyBind fires on press (game thread via the shim). Release is read
-- with PlayerController:IsInputKeyDown(Tab); if that cannot be read, TAB
-- toggles instead (auto-close after TAB_TOGGLE_MAX_S).

local T = Model.new()
local tab = { mode = nil, opened_at = 0 }     -- mode: "hold" | "toggle"
local TAB_KEY = { KeyName = FName("Tab", FNAME_Add) }

local function tab_down()
    local ok, down = pcall(function()
        local pc = WG.pc()
        if not (pc and pc:IsValid()) then return nil end
        return pc:IsInputKeyDown(TAB_KEY)
    end)
    if ok and type(down) == "boolean" then return down end
    return nil
end

local function on_tab()
    if T.tab and tab.mode == "toggle" then T.tab = false; return end
    T.tab = true
    tab.opened_at = os.clock()
    local d = tab_down()
    tab.mode = (d == nil) and "toggle" or "hold"
end

local function poll_tab(now)
    if not T.tab then return end
    if tab.mode == "hold" then
        local d = tab_down()
        if d == false then T.tab = false
        elseif d == nil then tab.mode = "toggle" end
    elseif now - tab.opened_at > TAB_TOGGLE_MAX_S then
        T.tab = false
    end
end

pcall(function()
    if Key and Key.TAB then RegisterKeyBind(Key.TAB, on_tab) end
end)

-- --- panels: connection modal, MP pause, match result (hud_panel.lua) ------------------------

-- HSMPHud -> Director requests (bus key `ui_request`, a typed record; the Director logs its
-- verdict; there is no ack). The seq continues from the bus value, so a request from an earlier
-- Lua state is never new.
local ui_seq = nil
local function ui_request(want, why)
    local ipc = rawget(_G, "HSMP_IPC")
    if ui_seq == nil then
        local cur = ipc and ipc.bus_table and ipc.bus_table("ui_request")
        ui_seq = (type(cur) == "table" and cur.seq) or 0
    end
    ui_seq = ui_seq + 1
    if ipc then
        ipc.bus_put("ui_request", { seq = ui_seq, want = tostring(want), reason = tostring(why or ""),
                                    t = os.time(), from = "HSMPHud" })
    end
    Log("ui request #%d: %s (%s)", ui_seq, want, tostring(why))
end

-- QUIT GAME from the MP pause: leave the server first (the Director asks the
-- sidecar), then quit the game a moment later.
-- The Director handles the leave within one 250 ms tick with an
-- OpenLevel to the menu, so the world has always changed by the time the quit
-- runs: no world-token gate. Nothing is captured; every object is fetched
-- fresh in the callback, and a quit that finds no world yet (mid-load) is
-- retried every 0.5 s.
local QUIT_TRIES = 20
local function quit_game()
    local tries = 0
    local function attempt()
        tries = tries + 1
        local how
        pcall(function()
            local world = WG.world()
            if not (world and world:IsValid()) then return end
            local pc = WG.pc()
            if pc and not pc:IsValid() then pc = nil end
            local ksl = StaticFindObject("/Script/Engine.Default__KismetSystemLibrary")
            if ksl and ksl:IsValid() then ksl:QuitGame(world, pc, 0, false); how = "QuitGame"; return end
        end)
        if how then
            Log("QUIT GAME: %s (try %d)", how, tries)
        elseif tries < QUIT_TRIES then
            ExecuteWithDelay(500, attempt)
        else
            Log("QUIT GAME: no world to quit from after %d tries", tries)
        end
    end
    ExecuteWithDelay(900, attempt)
end

-- SETTINGS from the MP pause: the game's own settings screen, opened directly
-- (no native pause, so nothing pauses or travels). If it cannot be found, the
-- next Esc shows the game's pause menu for its settings (the world is kept
-- running by HSMPMatch) and a toast says so.
local SETTINGS_CLASSES = { "/Game/UI/UI_GameSettings.UI_GameSettings_C", "/Game/UI/Menu/UI_GameSettings.UI_GameSettings_C",
                           "/Game/UI/Settings/UI_GameSettings.UI_GameSettings_C" }
local native_pause_ok_until = 0
-- The settings screen we opened keeps UI input (cursor) until it closes; the
-- native-menu scan may take most of a second to see it, hence the grace.
local settings_by_us, settings_grace_until = false, 0
local function open_settings(now)
    local opened = false
    pcall(function()
        local wbl = StaticFindObject("/Script/UMG.Default__WidgetBlueprintLibrary")
        local world, pc = WG.world(), WG.pc()
        if not (wbl and wbl:IsValid() and world and world:IsValid()) then return end
        for _, path in ipairs(SETTINGS_CLASSES) do
            local cls = StaticFindObject(path)
            if cls and cls:IsValid() then
                local w = wbl:Create(world, cls, pc)
                if w and w:IsValid() then
                    w:AddToViewport(Panel.Z + 100)
                    opened = true
                    settings_by_us, settings_grace_until = true, os.clock() + 1.0
                    Log("MP pause: settings opened (%s)", path)
                    return
                end
            end
        end
    end)
    if not opened then
        native_pause_ok_until = now + 15
        Model.add_notices(T, { { name = "notice", text = "Press Esc for the game menu settings (the match keeps running)" } }, now)
        Log("MP pause: settings widget not found; the next Esc shows the native menu")
    end
    return opened
end

local function on_click(id)
    local now = os.clock()
    local want, action = Model.click(T, id, now)
    Log("panel click %s -> %s%s", id, tostring(want), action and (" + " .. action) or "")
    if want then ui_request(want, id) end
    if action == "quit" then quit_game() end
    if action == "settings" then open_settings(now) end
end

-- Native pause (Esc) in an MP arena: the widget goes, the world keeps running,
-- the MP pause opens (Esc again closes it). Fresh lookups only.
local PAUSE_WIDGETS = { "UI_Pause_C", "UI_Pause_Eng_C" }
local native_pause_n = 0
local pause_seen = false
local esc_until = -1   -- os.clock until which an Esc press keeps the pause scan on
local function intercept_native_pause(now)
    if now < native_pause_ok_until then return end
    local found = false
    for _, cls in ipairs(PAUSE_WIDGETS) do
        pcall(function()
            for _, w in pairs(FindAllOf(cls) or {}) do
                if w and w:IsValid() then
                    local inv = false
                    pcall(function() inv = w:IsInViewport() end)
                    if inv then
                        found = true
                        pcall(function() w:SetVisibility(1) end)
                        pcall(function() w:RemoveFromParent() end)
                    end
                end
            end
        end)
    end
    if not found then return end
    pcall(function()
        local gs = UEHelpers.GetGameplayStatics()
        local world = WG.world()
        if gs and world and world:IsValid() then gs:SetGamePaused(world, false) end
    end)
    native_pause_n = native_pause_n + 1
    T.pause_open = not T.pause_open
    T.confirm = nil
    Log("MP pause %s (native pause menu suppressed, world kept running) [#%d]", T.pause_open and "opened" or "closed",
        native_pause_n)
end
local notify_ok = pcall(function()
    NotifyOnNewObject("/Script/UMG.UserWidget", function(obj)
        local cls
        pcall(function() cls = obj:GetClass():GetFName():ToString() end)
        if cls == "UI_Pause_C" or cls == "UI_Pause_Eng_C" then pause_seen, pause_ever = true, true end
        if cls and MENU_SET[cls] then menu_ever = true; menu_hint[cls] = true end
    end)
end)
-- No creation hook (or the widgets predate a mod reload): always scan.
if not notify_ok then menu_ever, pause_ever = true, true end
-- An Esc press opens a 0.6 s window of pause scans (the keybind runs on
-- the game thread through the shim; it only stamps a number).
local function on_esc() esc_until = os.clock() + 0.6 end
pcall(function()
    if Key and Key.ESCAPE then RegisterKeyBind(Key.ESCAPE, on_esc) end
end)

local panel_size_t = 0
local function sync_panel(spec, in_arena, now, native_menu)
    local cur = Panel.cur
    if spec == nil then
        if cur then Panel.close(); Log("panel closed") end
    else
        -- an open panel follows a resize / resolution / DPI change (checked twice a second)
        local resized = false
        if cur and Panel.alive() and now >= (panel_size_t or 0) then
            panel_size_t = now + 0.5
            local cw, ch, dpi = canvas_size()
            if cw ~= cur.cw or ch ~= cur.ch or dpi ~= cur.dpi then
                resized = true
                Log("panel %s: canvas %dx%d@%.3f -> %dx%d@%.3f, rebuilt", spec.kind, cur.cw or 0, cur.ch or 0,
                    cur.dpi or 0, cw, ch, dpi)
            end
        end
        if resized or not cur or cur.key ~= spec.key or not Panel.alive() then
            if cur then Panel.close() end
            panel_world = WG.key
            local cw, ch, dpi = canvas_size()
            if Panel.build(spec, cw, ch, dpi) then
                Log("panel %s: %s [%s]", spec.kind, tostring(spec.title or ""), spec.key)
            end
        end
    end
    -- cursor / UI input only while a panel is open in an arena
    if settings_by_us and not native_menu and now >= settings_grace_until then settings_by_us = false end
    -- The input mode is only ever switched in an arena, and only for
    -- the world it was applied in; outside an arena the native menus own it.
    if panel_input_world ~= WG.key then panel_input, panel_input_world = false, WG.key end
    if not in_arena then panel_input = false; return end
    local want_input = Panel.cur ~= nil or settings_by_us
    if want_input ~= panel_input then
        panel_input = want_input
        Panel.set_input(want_input)
    end
end

LoopAsync(33, function()
    if Panel.cur then
        local ok, err = pcall(Panel.poll, on_click)
        if not ok then Log("panel poll error: %s", tostring(err)) end
    end
    return false
end)

-- --- tick -----------------------------------------------------------------------------------

local tail = State.new_tail("death")
local ntail = State.new_tail("notice")   -- typed S2G notice events (dedup by event_id)
local snap = { vremote = {} }
local tick_n = 0
local menu_is_open = false
local last_model = nil
local last_vis_why = nil
local last_panel = nil
local last_in_arena = false   -- the last world's verdict (idle ticks observe with it)

local function tick()
    tick_n = tick_n + 1
    in_good_tick = false
    local now = os.clock()
    -- state is read even between worlds: none of it is a UObject
    if tick_n % READ_EVERY == 1 or tick_n == 1 then
        snap = State.read_all(STATE_DIR)
        if SESS then
            pcall(function()
                SESS:poll(true)
                snap.sess = { live = SESS:live(), fresh = SESS:fresh(), status = SESS.status,
                              changed_age = os.clock() - SESS.changed_at }
                -- Decided at read time too: a snapshot whose observe()
                -- is skipped (world-guard drop tick) must not carry a banner
                -- for a leftover link into build().
                if snap.rejected and not snap.sess.fresh then snap.rejected = nil end
            end)
        end
        Model.add_deaths(T, snap, State.poll_tail(tail, snap.match), now)
        Model.add_notices(T, State.poll_notices(ntail), now)
    end
    -- Nothing MP is going on (no session, no HUD, no panel, no modal):
    -- the world check (a PlayerController lookup) runs every 3rd tick only.
    local idle = not T.mp.active and not host and not Panel.cur and not snap.rejected
        and not (snap.conn and snap.conn.state ~= "ok")
    if idle and tick_n % 3 ~= 1 then
        if tick_n % READ_EVERY == 1 then Model.observe(T, snap, now, { in_arena = last_in_arena }) end
        return
    end
    if not wg_check() then return end
    in_good_tick = true
    world_seen_at = world_seen_at or now
    local wname = short_name(WG.name) or ""
    local in_arena = wname:find("^Map_Arena_") ~= nil
    last_in_arena = in_arena
    if tick_n % READ_EVERY == 1 or tick_n == 1 then
        Model.observe(T, snap, now, { in_arena = in_arena })
    end
    -- MP pause instead of the native pause (never pauses the world)
    -- The 2 pause-class walks run on the widget creation hook, for 0.6 s
    -- after an Esc press (a reused widget fires no creation hook), and as a
    -- 1 Hz backstop (gamepad pause, a missed key).
    if in_arena and T.mp.active and (pause_seen or now < esc_until or (pause_ever and tick_n % 10 == 0)) then
        pause_seen = false
        intercept_native_pause(now)
    end
    -- The native-menu scans only matter while the HUD can show (MP)
    if tick_n % MENU_EVERY == 0 then menu_is_open = in_arena and T.mp.active and menu_open() or false end
    poll_tab(now)

    local spec = Model.panel(T, snap, now, { in_arena = in_arena })
    last_panel = spec
    sync_panel(spec, in_arena, now, menu_is_open)

    local model = Model.build(T, snap, now, { in_arena = in_arena, menu_open = menu_is_open,
                                              centre_enabled = CENTRE_BANNER, modal = spec and spec.modal })
    last_model = model
    local why = model.visible and "visible" or model.why
    if why ~= last_vis_why then
        last_vis_why = why
        Log("HUD %s (world=%s state=%s)", model.visible and "shown" or ("hidden: " .. why), wname,
            snap.match and snap.match.state or "-")
    end

    if host and tick_n % HOST_CHECK_EVERY == 0 then check_host() end
    if model.visible then
        if not host then ensure_host(now) end
        if host and view then
            Kit.show(host.root, true)
            View.apply(view, model, now)
        end
    elseif host then
        Kit.show(host.root, false)
    end
    in_good_tick = false
end

-- Tick errors are logged with a budget (the first 5, then one line per
-- 10 s with the count), never every 100 ms.
local tick_err = { n = 0, next_at = 0, last = nil }
LoopAsync(TICK_MS, function()
    local ok, err = pcall(tick)
    in_good_tick = false
    if not ok then
        tick_err.n = tick_err.n + 1
        local now = os.clock()
        local msg = tostring(err)
        if tick_err.n <= 5 or now >= tick_err.next_at then
            tick_err.next_at, tick_err.last = now + 10, msg
            Log("tick error (#%d): %s", tick_err.n, msg)
        end
    end
    return false
end)

-- Test / diagnostics handle (this mod's own Lua state only).
HSMPHUD = {
    T = T, tick = tick, on_tab = on_tab, on_click = on_click,
    host = function() return host end, view = function() return view end,
    model = function() return last_model end, panel = function() return last_panel end,
    panel_ui = function() return Panel.cur end, stats = stats,
    set_centre = function(on) CENTRE_BANNER = on and true or false end,
    on_esc = on_esc, menu_state = menu_state,
}

Log("loaded; state_dir=%s centre_banner=%s", STATE_DIR, tostring(CENTRE_BANNER))
