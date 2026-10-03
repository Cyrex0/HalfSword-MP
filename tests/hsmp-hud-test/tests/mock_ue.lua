-- Mocked UE4SS 3.0.1 + UMG for the HSMPHud offline tests (tests/hud.rs).
--
-- Objects are tables with a metatable: methods come from `Methods`, other
-- fields are properties. Once M.kill_all() has run (the old world is freed),
-- ANY access to an old object (IsValid included) is recorded in M.dead_touch:
-- the tests require that list to stay empty (world guard).

M = { now = 0, loops = {}, delayed = {}, seq = 0, logs = {}, dead_touch = {}, objs = {}, addr = 0,
      vw = 1920, vh = 1080, scale = 1.0, cw = 1920, ch = 1080, keys = {}, viewport = {}, input_calls = {},
      hud_present = true, hud_inviewport = true, pause_open = false, tab_down = false, tab_unreadable = false,
      vis_set = {}, constructed = {}, env = {} }

local Methods = {}
local ObjMT = {}
ObjMT.__index = function(t, k)
    -- IsValid() on a freed object is a touch too (it reads freed memory in the game)
    if rawget(t, "__dead") then table.insert(M.dead_touch, tostring(rawget(t, "__name")) .. "." .. k) end
    local m = Methods[k]; if m then return m end
    local p = rawget(t, "__props")[k]
    if p ~= nil then return p end
    if k == "Font" then local f = { Size = 24 }; t.__props.Font = f; return f end
    return nil
end
ObjMT.__newindex = function(t, k, v)
    if rawget(t, "__dead") then table.insert(M.dead_touch, tostring(rawget(t, "__name")) .. "." .. k .. "=") end
    t.__props[k] = v
end

function new_obj(cls, name)
    M.addr = M.addr + 1
    local o = setmetatable({ __cls = cls, __name = name, __props = {}, __addr = M.addr }, ObjMT)
    table.insert(M.objs, o)
    return o
end

local function fname(s) return { ToString = function() return s end } end
Methods.IsValid = function(self)
    if rawget(self, "__dead") then error("access violation: IsValid on freed " .. tostring(rawget(self, "__name"))) end
    return true
end
Methods.GetFName = function(self) return fname(rawget(self, "__name")) end
Methods.GetAddress = function(self) return rawget(self, "__addr") end
Methods.GetClass = function(self) local c = rawget(self, "__cls"); return { GetFName = function() return fname(c) end } end
Methods.GetFullName = function(self) return rawget(self, "fullname") or ("Obj " .. tostring(rawget(self, "__name"))) end
Methods.GetWorld = function() return M.world end
Methods.SetVisibility = function(self, v)
    rawset(self, "vis", v)
    M.vis_set[v] = (M.vis_set[v] or 0) + 1
end
Methods.SetText = function(self, ft) rawset(self, "text", ft and ft.s or "") end
Methods.SetJustification = function(self, j) rawset(self, "just", j) end
Methods.SetFont = function(self, f) rawset(self, "fs", f.Size) end
Methods.SetColorAndOpacity = function(self, c) rawset(self, "color", c.SpecifiedColor) end
Methods.SetShadowColorAndOpacity = function() end
Methods.SetShadowOffset = function() end
Methods.SetBrushColor = function(self, c) rawset(self, "brush", c) end
Methods.SetRenderOpacity = function(self, o) rawset(self, "opacity", o) end
Methods.IsInViewport = function(self)
    if rawget(self, "__cls") == "UI_HUD_C" then return M.hud_inviewport end
    return rawget(self, "inviewport") or false
end
Methods.AddToViewport = function(self, z) M.viewport[self] = z; rawset(self, "inviewport", true) end
Methods.RemoveFromParent = function(self) rawset(self, "removed", true); rawset(self, "inviewport", false); M.viewport[self] = nil end
Methods.GetOwningPlayer = function() return M.pc end
Methods.AddChildToCanvas = function(self, w)
    local slot = new_obj("CanvasPanelSlot", "slot")
    rawset(w, "slotobj", slot)
    rawset(w, "parent", self)
    return slot
end
Methods.SetAnchors = function(self, a) rawset(self, "anchors", a) end
Methods.SetAlignment = function(self, a) rawset(self, "align", a) end
Methods.SetOffsets = function(self, o) rawset(self, "offsets", o) end
Methods.SetPosition = function(self, p) rawset(self, "x", p.X); rawset(self, "y", p.Y) end
Methods.SetSize = function(self, p) rawset(self, "w", p.X); rawset(self, "h", p.Y) end
Methods.SetZOrder = function(self, z) rawset(self, "z", z) end
Methods.GetViewportScale = function() return M.scale end
Methods.GetGameViewportClient = function() return M.vp end
-- WidgetLayoutLibrary::GetViewportSize(WorldContext) -> FVector2D; the GameViewportClient
-- path is C++-only in the game (M.gvc_broken makes it fail like there).
Methods.GetViewportSize = function(self, out)
    if rawget(self, "__cls") == "WLL" then return { X = M.vw, Y = M.vh } end
    if M.gvc_broken then error("GetViewportSize is not a UFunction") end
    out.X = M.vw; out.Y = M.vh
end
Methods.IsInputKeyDown = function(_, key)
    if M.tab_unreadable then error("FKey not marshalled") end
    M.last_key = key and key.KeyName
    return M.tab_down
end
Methods.SetInputMode_GameOnly = function() table.insert(M.input_calls, "SetInputMode_GameOnly") end
Methods.SetInputMode_UIOnly = function() table.insert(M.input_calls, "SetInputMode_UIOnly") end
Methods.SetInputMode_GameAndUIEx = function() table.insert(M.input_calls, "SetInputMode_GameAndUI") end
Methods.SetShowMouseCursor = function() table.insert(M.input_calls, "SetShowMouseCursor") end
Methods.SetKeyboardFocus = function() table.insert(M.input_calls, "SetKeyboardFocus") end

function FText(s) return { s = s } end
function FName(s) return s end
function FString(s) return s end
FNAME_Add = 1
Key = { TAB = "TAB", F1 = "F1" }

-- GameInstance, viewport client and the layout library outlive a level change.
M.vp = new_obj("GameViewportClient", "vp")
local gi = new_obj("GI", "gi")
local wll = new_obj("WLL", "wll")
for _, o in ipairs({ M.vp, gi, wll }) do rawset(o, "__persist", true) end

-- A fresh world: new UWorld, PlayerController, native UI_HUD_C (CanvasPanel root) and pause menu.
function M.new_world(name)
    M.world = new_obj("World", "world")
    rawset(M.world, "fullname", "World /Game/Maps/" .. name .. "." .. name)
    M.pc = new_obj("PC", "PC_" .. tostring(M.addr))
    M.hud = new_obj("UI_HUD_C", "UI_HUD_C_" .. tostring(M.addr))
    local wt = new_obj("WidgetTree", "WidgetTree")
    M.hud_root = new_obj("CanvasPanel", "CanvasPanel_13")
    wt.__props.RootWidget = M.hud_root
    M.hud.__props.WidgetTree = wt
    M.hud.__props.TextWin = new_obj("TextBlock", "TextWin")
    M.pause = new_obj("UI_Pause_C", "UI_Pause_C_0")
    rawset(M.pause, "inviewport", true)
    M.photo = new_obj("UI_PhotoMode_C", "UI_PhotoMode_C_0")
    rawset(M.photo, "inviewport", true)
    -- every world creates its own pause / photo-mode widgets
    if M.fire_new then M.fire_new(M.pause); M.fire_new(M.photo) end
end
M.new_world("Map_Menu_Startup")

package.preload["UEHelpers"] = function()
    return { GetGameInstance = function() return gi end, GetPlayerController = function() return M.pc end,
             GetWorld = function() return M.world end, GetGameplayStatics = function() return nil end }
end

function StaticFindObject(path)
    if path == "/Script/UMG.Default__WidgetLayoutLibrary" then return M.scale and wll or nil end
    if path:match("^/Script/UMG%.") then return new_obj("Class", path:match("%.(%w+)$")) end
    return nil
end
function StaticConstructObject(cls, outer, name)
    local c = cls:GetFName():ToString()
    if outer == nil then error("nil outer") end
    if rawget(outer, "__dead") then table.insert(M.dead_touch, "construct into dead outer") end
    M.constructed[c] = (M.constructed[c] or 0) + 1
    local o = new_obj(c, name or ("anon_" .. c))
    rawset(o, "outer", outer)
    return o
end
function FindAllOf(c)
    if c == "UI_HUD_C" then return M.hud_present and { M.hud } or nil end
    if c == "UI_Pause_C" then return M.pause_open and { M.pause } or nil end
    if c == "UI_PhotoMode_C" then return M.photo_open and { M.photo } or nil end
    return nil
end

-- game-thread scheduling (fake clock, ms). Raw LoopAsync / ExecuteWithDelay
-- raise: the mod must route them through its game-thread shim.
function LoopInGameThreadWithDelay(ms, fn) M.seq = M.seq + 1; M.loops[M.seq] = { ms = ms, fn = fn, due = M.now + ms }; return M.seq end
function ExecuteInGameThreadWithDelay(ms, fn) M.seq = M.seq + 1; M.delayed[M.seq] = { fn = fn, due = M.now + ms }; return M.seq end
function CancelDelayedAction(h) M.loops[h] = nil; M.delayed[h] = nil end
function ExecuteInGameThread(fn) fn() end
function LoopAsync() error("raw LoopAsync must be shimmed") end
function ExecuteWithDelay() error("raw ExecuteWithDelay must be shimmed") end
function RegisterKeyBind(key, a, b) M.keys[key] = b or a end
function RegisterKeyBindAsync() M.keys_async = (M.keys_async or 0) + 1 end
M.notify_cbs = {}
function NotifyOnNewObject(cls, fn) table.insert(M.notify_cbs, fn) end
-- a native widget instance is created (UE4SS NotifyOnNewObject fires)
function M.fire_new(o) for _, fn in ipairs(M.notify_cbs or {}) do pcall(fn, o) end end
function RegisterLoadMapPreHook(fn) M.premap = fn end
function RegisterLoadMapPostHook() end
function RegisterHook(path, fn) if path:find("OpenLevel$") then M.openlevel_hook = fn end end

print = function(s) table.insert(M.logs, s) end
os.clock = function() return M.now / 1000 end
local _getenv = os.getenv
os.getenv = function(k) if M.env[k] ~= nil then return M.env[k] end return _getenv(k) end

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

-- Visible HUD widgets on the current host canvas with ABSOLUTE rects
-- (anchor point * canvas size + slot offset). Empty when the HUD root is hidden.
function M.live()
    local h = HSMPHUD and HSMPHUD.host()
    if not h then return {} end
    local root = (h.kind == "viewport") and h.uw or h.canvas
    if rawget(root, "vis") == 1 or rawget(root, "removed") then return {} end
    local out = {}
    for _, w in ipairs(M.objs) do
        local s = rawget(w, "slotobj")
        if s and rawget(w, "parent") == h.canvas and not rawget(w, "removed")
            and rawget(w, "vis") ~= 1 and rawget(w, "vis") ~= 2 then
            local a = rawget(s, "anchors") or { Minimum = { X = 0, Y = 0 } }
            local ax, ay = a.Minimum.X, a.Minimum.Y
            table.insert(out, { cls = rawget(w, "__cls"), name = rawget(w, "__name"), text = rawget(w, "text") or "",
                x = ax * M.cw + (rawget(s, "x") or 0), y = ay * M.ch + (rawget(s, "y") or 0),
                w = rawget(s, "w") or 0, h = rawget(s, "h") or 0, fs = rawget(w, "fs") or 24,
                anchor = string.format("%.1f,%.1f", ax, ay) })
        end
    end
    return out
end

function M.kill_all()   -- the old world is freed: any later touch is recorded
    for _, o in ipairs(M.objs) do
        if not rawget(o, "__persist") and rawget(o, "__cls") ~= "Class" then rawset(o, "__dead", true) end
    end
end

function M.logtext() return table.concat(M.logs, "\n") end
