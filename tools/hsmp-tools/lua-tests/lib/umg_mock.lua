-- umg_mock: a mock UE4SS + UMG environment for offline Lua tests (hsmp-tools lua-test).
--
--   local M = require("umg_mock")
--   M.install({ vw = 1920, vh = 1080, scale = 1.0, state_dir = sd })
--   dofile(T.path("mods/HSMPMenu/Scripts/main.lua"))
--   M.on_new(M.menu); M.run(700)
--
-- What it provides (globals, as UE4SS does): StaticFindObject /
-- StaticConstructObject / FindAllOf / FText / FName / FString / Key,
-- UEHelpers (package.preload), the game-thread loop + delay API on a FAKE
-- clock (M.now, ms; M.run(ms) advances it 1 ms at a time), NotifyOnNewObject /
-- RegisterLoadMapPre/PostHook capture (M.on_new / M.premap / M.postmap),
-- print -> M.logs (the event log), os.execute -> M.execs, os.clock -> fake,
-- os.getenv overrides (HSMP_STATE_DIR = opts.state_dir, opts.env[k]).
-- Raw LoopAsync / ExecuteWithDelay raise (they must be shimmed to game thread).
--
-- Widgets are tables with an `ObjMT` metatable: methods in M.Methods, props in
-- __props, state via rawget/rawset (pressed, hovered, vis, text, x/y/w/h on slots).
-- M.click(w) holds IsPressed for 70 ms then releases for 70 ms; M.live() lists
-- visible canvas widgets with their slot rects; M.kill_all() frees every object
-- and any later touch is recorded in M.dead_touch.

local M = { now = 0, loops = {}, delayed = {}, seq = 0, logs = {}, execs = {}, dead_touch = {},
            objs = {}, canvas_children = {}, openlevel = {}, vw = 1920, vh = 1080, scale = 1.0 }

local Methods = {}
M.Methods = Methods
local ObjMT = {}
M.ObjMT = ObjMT
ObjMT.__index = function(t, k)
    if rawget(t, "__dead") and (k ~= "IsValid" or M.strict) then table.insert(M.dead_touch, tostring(rawget(t, "__name")) .. "." .. k) end
    local m = Methods[k]; if m then return m end
    local p = rawget(t, "__props")[k]
    if p ~= nil then return p end
    if k == "Font" then local f = { Size = 24 }; t.__props.Font = f; return f end
    if k == "WidgetStyle" then local s = { Font = { Size = 12 } }; t.__props.WidgetStyle = s; return s end
    return nil
end
ObjMT.__newindex = function(t, k, v)
    if rawget(t, "__dead") then table.insert(M.dead_touch, tostring(rawget(t, "__name")) .. "." .. k .. "=") end
    t.__props[k] = v
end

local function new_obj(cls, name)
    local o = setmetatable({ __cls = cls, __name = name, __props = {}, __children = {} }, ObjMT)
    table.insert(M.objs, o)
    return o
end
M.new_obj = new_obj

local function fname(s) return { ToString = function() return s end } end
-- In the real game IsValid() on a freed object reads freed memory (an
-- access violation pcall cannot catch). opts.strict makes the mock behave
-- like that: the touch is recorded (above) AND it raises, so a "guarded"
-- IsValid on a stale cache fails the test instead of passing silently.
Methods.IsValid = function(self)
    if rawget(self, "__dead") and M.strict then error("access violation: IsValid on freed " .. tostring(rawget(self, "__name"))) end
    return not rawget(self, "__dead")
end
Methods.GetFName = function(self) return fname(rawget(self, "__name")) end
Methods.GetClass = function(self) local c = rawget(self, "__cls"); return { GetFName = function() return fname(c) end } end
-- KismetSystemLibrary::GetFrameCount (hsmp_wg WG.pc frame id): one
-- mock "frame" per M.now step
Methods.GetFrameCount = function(self) return M.frame_n or M.now end
Methods.GetFullName = function(self) return "World /Game/Maps/Map_Menu_Startup" end
Methods.SetVisibility = function(self, v) rawset(self, "vis", v) end
Methods.SetText = function(self, ft) rawset(self, "text", ft and ft.s or "") end
Methods.GetText = function(self) local s = rawget(self, "text") or ""; return { ToString = function() return s end } end
Methods.SetHintText = function(self, ft) rawset(self, "hint", ft and ft.s) end
Methods.SetJustification = function(self, j) rawset(self, "just", j) end
Methods.SetFont = function(self, f) rawset(self, "fs", f.Size) end
Methods.SetColorAndOpacity = function(self, c) rawset(self, "color", c.SpecifiedColor) end
Methods.SetShadowColorAndOpacity = function() end
Methods.SetBrushColor = function(self, c) rawset(self, "brush", c) end
Methods.SetBackgroundColor = function(self, c) rawset(self, "bgc", c) end
Methods.SetPadding = function() end
Methods.SetContent = function(self, w) rawset(self, "content", w) end
Methods.IsPressed = function(self) return rawget(self, "pressed") or false end
Methods.IsHovered = function(self) return rawget(self, "hovered") or false end
Methods.SetRenderOpacity = function(self, o) rawset(self, "opacity", o) end
Methods.IsInViewport = function() return true end
Methods.RemoveFromViewport = function() end
Methods.GetChildrenCount = function(self) return #rawget(self, "__children") end
Methods.GetChildAt = function(self, i) return rawget(self, "__children")[i + 1] end
Methods.RemoveChild = function(self, w) rawset(w, "removed", true) end
Methods.AddChildToCanvas = function(self, w)
    local slot = new_obj("CanvasPanelSlot", "slot")
    rawset(w, "slotobj", slot)
    rawset(slot, "widget", w)
    table.insert(M.canvas_children, w)
    return slot
end
Methods.SetAnchors = function(self, a) rawset(self, "anchors", a) end
Methods.SetAlignment = function(self, a) rawset(self, "align", a) end
Methods.SetPosition = function(self, p) rawset(self, "x", p.X); rawset(self, "y", p.Y) end
Methods.SetSize = function(self, p) rawset(self, "w", p.X); rawset(self, "h", p.Y) end
Methods.SetZOrder = function(self, z) rawset(self, "z", z) end
Methods.GetPosition = function(self) return { X = rawget(self, "x") or 0, Y = rawget(self, "y") or 0 } end
Methods.GetSize = function(self) return { X = rawget(self, "w") or 0, Y = rawget(self, "h") or 0 } end
Methods.GetAnchors = function() return { Minimum = { X = 0.5, Y = 0.5 }, Maximum = { X = 0.5, Y = 0.5 } } end
Methods.GetAlignment = function() return { X = 0, Y = 0 } end
Methods.GetViewportScale = function() return M.scale end
Methods.GetGameViewportClient = function() return M.vp end
-- WidgetLayoutLibrary::GetViewportSize(WorldContext) -> FVector2D (a BlueprintCallable static);
-- GameViewportClient::GetViewportSize(out) is C++-only in the real game: opts.gvc_broken makes
-- it fail like there (the in-game bug: every build assumed 1920x1080 physical).
Methods.GetViewportSize = function(self, out)
    if rawget(self, "__cls") == "WLL" then
        if M.wll_size_broken then error("GetViewportSize: no such function") end
        return { X = M.vw, Y = M.vh }
    end
    if M.gvc_broken then error("GetViewportSize is not a UFunction") end
    out.X = M.vw; out.Y = M.vh
end
Methods.ConsoleCommand = function(self, c) table.insert(M.execs, "console " .. tostring(c)) end
-- KismetSystemLibrary:QuitGame(WorldContext, SpecificPlayer, QuitPreference, bIgnorePlatformRestrictions)
Methods.QuitGame = function(self, ctx, pc, pref) table.insert(M.execs, "QuitGame " .. tostring(pref)) end
Methods.OpenLevel = function(self, world, name) table.insert(M.openlevel, name) end
-- keyboard focus (text inputs) + gamepad keys
Methods.HasKeyboardFocus = function(self) return rawget(self, "kbfocus") or false end
Methods.SetKeyboardFocus = function(self)
    if M.kbfocus and M.kbfocus ~= self then rawset(M.kbfocus, "kbfocus", false) end
    M.kbfocus = self; rawset(self, "kbfocus", true)
end
Methods.SetFocusToGameViewport = function()
    if M.kbfocus then rawset(M.kbfocus, "kbfocus", false) end
    M.kbfocus = nil
    M.viewport_focus_n = (M.viewport_focus_n or 0) + 1
end
Methods.IsInputKeyDown = function(self, key)
    if M.pad_error then error("IsInputKeyDown: bad FKey") end
    return M.pad_down[key and key.KeyName or ""] or false
end
Methods.SetInputMode_GameOnly = function() end
Methods.SetShowMouseCursor = function() end
Methods.SetIgnoreLookInput = function() end
Methods.SetIgnoreMoveInput = function() end

-- Install the globals. opts: vw, vh, scale (nil = no WidgetLayoutLibrary / DPI default),
-- state_dir (-> HSMP_STATE_DIR), env (extra getenv overrides; false = unset),
-- startup_menu (default true: the native UI_Startup_Menu with 6 buttons).
function M.install(opts)
    opts = opts or {}
    -- a fresh world: nothing of an earlier install keeps running or listening
    M.now, M.loops, M.delayed, M.seq = 0, {}, {}, 0
    M.logs, M.execs, M.dead_touch, M.objs, M.canvas_children, M.openlevel = {}, {}, {}, {}, {}, {}
    M.vw, M.vh = opts.vw or 1920, opts.vh or 1080
    if opts.scale ~= nil or opts.vw ~= nil then M.scale = opts.scale end
    M.gvc_broken, M.wll_size_broken = opts.gvc_broken and true or nil, opts.wll_size_broken and true or nil
    _G.M = M

    function _G.FText(s) return { s = s } end
    function _G.FName(s) return s end
    function _G.FString(s) return s end
    _G.FNAME_Add = 1
    -- every UE4SS Key name maps to itself; ModifierKey likewise
    _G.Key = setmetatable({}, { __index = function(_, k) return k end })
    _G.ModifierKey = { SHIFT = "SHIFT", CONTROL = "CONTROL", ALT = "ALT" }
    M.keybinds, M.pad_down, M.kbfocus, M.pad_error = {}, {}, nil, nil

    M.vp = new_obj("GameViewportClient", "vp")
    local gi = new_obj("GI", "gi")
    local pc = new_obj("PC", "pc")
    local world = new_obj("World", "world")
    local gs = new_obj("GS", "gs")
    local wll = new_obj("WLL", "wll")
    M.gi, M.pc, M.world, M.gs = gi, pc, world, gs
    M.ksl = new_obj("KSL", "Default__KismetSystemLibrary")
    package.preload["UEHelpers"] = function()
        return { GetGameInstance = function() return gi end, GetPlayerController = function() return pc end,
                 GetWorld = function() return world end, GetGameplayStatics = function() return gs end,
                 GetKismetSystemLibrary = function() return M.ksl end }
    end

    local wbl = new_obj("WBL", "wbl")
    function _G.StaticFindObject(path)
        if path == "/Script/UMG.Default__WidgetLayoutLibrary" then return M.scale and wll or nil end
        if path == "/Script/UMG.Default__WidgetBlueprintLibrary" then return wbl end
        if path:match("^/Script/UMG%.") then return new_obj("Class", path:match("%.(%w+)$")) end
        return nil
    end
    function _G.StaticConstructObject(cls, outer, name)
        local c = cls:GetFName():ToString()
        return new_obj(c, name)
    end

    if opts.startup_menu ~= false then
        local function native_button(name, x, y)
            local b = new_obj("Button", name)
            local slot = new_obj("CanvasPanelSlot", "slot_" .. name)
            rawset(slot, "x", x); rawset(slot, "y", y); rawset(slot, "w", 469); rawset(slot, "h", 120)
            b.__props.Slot = slot
            local t = new_obj("TextBlock", "txt_" .. name)
            table.insert(rawget(b, "__children"), t)
            return b
        end
        M.canvas = new_obj("CanvasPanel", "CanvasPanel_5")
        for i, n in ipairs({ "Button", "Button_0", "Button_1", "Button_2", "Button_3", "Button_4" }) do
            table.insert(rawget(M.canvas, "__children"), native_button(n, -700, -360 + i * 110))
        end
        M.menu = new_obj("UI_Startup_Menu_C", "UI_Startup_Menu_C_0")
        M.menu.__props.WidgetTree = new_obj("WidgetTree", "wt")
        M.menu.__props.WidgetTree.__props.RootWidget = M.canvas
    end
    function _G.FindAllOf(c) if c == "UI_Startup_Menu_C" and M.menu then return { M.menu } end return nil end

    -- game-thread scheduling (fake clock, ms)
    function _G.LoopInGameThreadWithDelay(ms, fn) M.seq = M.seq + 1; M.loops[M.seq] = { ms = ms, fn = fn, due = M.now + ms }; return M.seq end
    function _G.ExecuteInGameThreadWithDelay(ms, fn) M.seq = M.seq + 1; M.delayed[M.seq] = { fn = fn, due = M.now + ms }; return M.seq end
    function _G.CancelDelayedAction(h) M.loops[h] = nil; M.delayed[h] = nil end
    -- strict: deferred to the next game-thread step like the real API (not
    -- run inside the caller's pcall, so an unprotected body shows its error)
    M.strict = opts.strict and true or false
    if M.strict then
        function _G.ExecuteInGameThread(fn) M.seq = M.seq + 1; M.delayed[M.seq] = { fn = fn, due = M.now + 1, raw = true }; return M.seq end
    else
        function _G.ExecuteInGameThread(fn) fn() end
    end
    function _G.LoopAsync(ms, fn) error("raw LoopAsync must be shimmed") end
    function _G.ExecuteWithDelay(ms, fn) error("raw ExecuteWithDelay must be shimmed") end
    -- key binds are captured; M.press(key, mods) fires every bind whose key
    -- matches and whose modifiers are all held (UE4SS: a plain TAB bind also
    -- fires on Shift+TAB)
    local function capture(key, a, b)
        local mods, fn = {}, a
        if b ~= nil then mods, fn = a or {}, b end
        table.insert(M.keybinds, { key = key, mods = mods, fn = fn })
    end
    _G.RegisterKeyBind = capture
    _G.RegisterKeyBindAsync = capture
    function _G.NotifyOnNewObject(cls, fn) M.on_new = fn end
    function _G.RegisterLoadMapPreHook(fn) M.premap = fn end
    function _G.RegisterLoadMapPostHook(fn) M.postmap = fn end
    function _G.RegisterHook() end

    _G.print = function(s) table.insert(M.logs, s) end
    os.execute = function(c) table.insert(M.execs, c); return true end
    os.clock = function() return M.now / 1000 end

    local env = opts.env or {}
    local sd = opts.state_dir
    local _g = os.getenv
    os.getenv = function(k)
        if k == "HSMP_STATE_DIR" and sd then return sd end
        local v = env[k]
        if v == false then return nil end
        if v ~= nil then return v end
        return _g(k)
    end
    return M
end

function M.run(ms)
    local target = M.now + ms
    while M.now < target do
        M.now = M.now + 1
        local due = {}
        for h, l in pairs(M.loops) do if l.due <= M.now then due[#due + 1] = h end end
        table.sort(due)
        for _, h in ipairs(due) do
            local l = M.loops[h]
            if l then l.due = M.now + l.ms; local ok, e = pcall(l.fn); if not ok then table.insert(M.logs, "LOOP ERR " .. tostring(e)) end end
        end
        due = {}
        for h, d in pairs(M.delayed) do if d.due <= M.now then due[#due + 1] = h end end
        table.sort(due)
        for _, h in ipairs(due) do
            local d = M.delayed[h]
            if d then M.delayed[h] = nil; local ok, e = pcall(d.fn); if not ok then table.insert(M.logs, "DELAY ERR " .. tostring(e)) end end
        end
    end
end

function M.set(w, k, v) rawset(w, k, v) end

-- Press a key (UE4SS Key name, optional held modifier names), then let the
-- 33 ms input poll run.
function M.press(key, mods, ms)
    local held = {}
    for _, m in ipairs(mods or {}) do held[m] = true end
    for _, b in ipairs(M.keybinds) do
        if b.key == key then
            local ok = true
            for _, m in ipairs(b.mods) do if not held[m] then ok = false end end
            if ok then b.fn() end
        end
    end
    M.run(ms or 80)
end

-- Hold / release a gamepad key (FKey name) for the IsInputKeyDown poll.
function M.pad(name, down) M.pad_down[name] = down and true or nil end

function M.click(w)
    if not w then error("click on nil widget") end
    rawset(w, "pressed", true); M.run(70); rawset(w, "pressed", false); M.run(70)
end

-- live (added, not removed, visible) canvas widgets with their rect
function M.live()
    local out = {}
    for _, w in ipairs(M.canvas_children) do
        local s = rawget(w, "slotobj")
        if s and not rawget(w, "removed") and rawget(w, "vis") ~= 1 and rawget(w, "vis") ~= 2 then
            table.insert(out, { cls = rawget(w, "__cls"), name = rawget(w, "__name"), text = rawget(w, "text"),
                x = rawget(s, "x"), y = rawget(s, "y"), w = rawget(s, "w"), h = rawget(s, "h"),
                fs = rawget(w, "fs"), wrap = w.__props.AutoWrapText, obj = w })
        end
    end
    return out
end

function M.kill_all()   -- the old world is freed: any touch is recorded
    -- class default objects (Default__*) are never freed by a level change
    for _, o in ipairs(M.objs) do
        if not tostring(rawget(o, "__name")):match("^Default__") then rawset(o, "__dead", true) end
    end
end

function M.logtext() return table.concat(M.logs, "\n") end

return M
