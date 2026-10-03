-- HSMPHud / hud_kit.lua — display-only widget primitives for the in-match HUD.
--
-- Same look as HSMPMenu's ui_kit.lua (palette, CHAR_W / LINE_H text metrics,
-- K.fit clipping), but:
--   * no buttons, no inputs, no hover loop: the HUD never takes input. Every
--     widget is HitTestInvisible (3) or Collapsed (1), never Visible (0);
--   * anchor-aware placement: each widget is positioned relative to an anchor
--     point of the host canvas ({0,0} top-left .. {1,1} bottom-right), so a
--     corner element stays in its corner even if the canvas size estimate is
--     off (DPI curve, ultrawide);
--   * world guard: K.alive() is false once the host is dropped (world change,
--     native HUD gone). Nothing here touches a widget unless K.alive() holds.
--     K.forget() drops every reference WITHOUT touching it.

local K = {}

local ctx                 -- { construct(path, outer, name), Log, world_ok() }
local Log = function() end

K.host = nil              -- { tree, canvas, serial, kind } — the current build target
K.serial = 0
K.uid = 0
K.font_src = nil          -- optional TextBlock whose Font is cloned (native HUD text)

K.CHAR_W = 0.60           -- average glyph advance as a fraction of the font size
K.LINE_H = 1.40           -- line height as a fraction of the font size
K.MIN_FS = 9

-- colours (linear RGBA) — ui_kit.lua palette plus HUD-only entries
K.C = {
    panel   = { R = 0.012, G = 0.010, B = 0.008, A = 0.72 },
    strip   = { R = 0.020, G = 0.016, B = 0.012, A = 0.60 },
    me_cell = { R = 0.24,  G = 0.15,  B = 0.04,  A = 0.75 },
    track   = { R = 0.006, G = 0.005, B = 0.004, A = 0.80 },
    rule    = { R = 0.30,  G = 0.20,  B = 0.08,  A = 0.85 },
    row     = { R = 0.030, G = 0.025, B = 0.020, A = 0.80 },
    rowb    = { R = 0.020, G = 0.017, B = 0.014, A = 0.80 },
    feed    = { R = 0.010, G = 0.008, B = 0.006, A = 0.55 },

    text    = { R = 0.86, G = 0.79, B = 0.64, A = 1 },
    white   = { R = 0.95, G = 0.95, B = 0.95, A = 1 },
    dim     = { R = 0.34, G = 0.31, B = 0.26, A = 1 },
    head    = { R = 1.00, G = 0.66, B = 0.26, A = 1 },
    title   = { R = 1.00, G = 0.86, B = 0.45, A = 1 },
    good    = { R = 0.28, G = 0.85, B = 0.30, A = 1 },
    ok      = { R = 0.95, G = 0.76, B = 0.20, A = 1 },
    warn    = { R = 1.00, G = 0.45, B = 0.10, A = 1 },
    bad     = { R = 1.00, G = 0.20, B = 0.13, A = 1 },
    stam    = { R = 0.36, G = 0.62, B = 0.95, A = 1 },
    off     = { R = 0.25, G = 0.25, B = 0.25, A = 1 },
}

local VIS_HIT_INVISIBLE = 3
local VIS_COLLAPSED = 1

-- --- helpers -------------------------------------------------------------------------

function K.ascii(s)
    -- FText from Lua strings is not reliably UTF-8 aware here; keep it ASCII.
    return (tostring(s or ""):gsub("[\128-\255]+", "?"):gsub("[%c]", " "))
end

-- Clip `s` so it fits `w` units at font size `fs` (on `lines` lines).
function K.fit(s, w, fs, lines)
    s = K.ascii(s)
    local per = math.max(1, math.floor((w or 0) / ((fs or 16) * K.CHAR_W)))
    local n = per * (lines or 1)
    if #s > n then
        local c = s:gsub("  +", " ")          -- padding goes first ("NET OK  42 ms" on a small window)
        if #c <= n then return c end
        return c:sub(1, math.max(1, n - 2)) .. ".."
    end
    return s
end

function K.clamp(v, a, b) if v < a then return a elseif v > b then return b end return v end

-- Scaling: shared/hsmp_ui_scale.lua (UIS), the same rule as the menu: design
-- 1920x1080, s from the canvas short side (fitted to the width), fonts never
-- under UIS.MIN_FONT_PX physical px (K.set_dpi). main.lua loads it before
-- this file; without it (a broken deploy) the old proportional rule is used.
local UIS
do
    local ok, m = pcall(require, "hsmp_ui_scale")
    if ok and type(m) == "table" then UIS = m end
end
K.uis = UIS
K.dpi = 1                 -- DPI scale of the canvas being built (K.set_dpi)

-- main.lua hands the shared lib over when require() could not find it (dev tree).
function K.set_uis(m)
    if type(m) == "table" and m.scale then UIS = m; K.uis = m end
end

function K.ui_scale(cw, ch)
    if UIS then return UIS.scale(cw, ch) end
    return K.clamp(math.min(cw / 1920, ch / 1080), 0.55, 2.2)
end

-- The DPI scale of the canvas the next layout is for (min font in canvas units).
function K.set_dpi(dpi)
    K.dpi = (type(dpi) == "number" and dpi > 0.05) and dpi or 1
end

-- Smallest font (canvas units) at layout scale s on the current DPI.
function K.min_fs(s)
    if UIS then return UIS.min_font(K.dpi, s) end
    return K.MIN_FS
end

-- A layout fixes its minimum font from the canvas' own scale before it
-- raises its scale for that font (hud_view / hud_panel): K.fs uses it.
K.minfs = nil
function K.layout_scale(cw, ch, min_design_fs)
    local s0 = K.ui_scale(cw, ch)
    K.minfs = K.min_fs(s0)
    return math.max(s0, K.minfs / min_design_fs)
end

function K.fs(px, s) return math.max(K.minfs or K.min_fs(s), math.floor(px * s)) end

local function uname(tag)
    K.uid = K.uid + 1
    return string.format("HSMPHUD_%s_%d_%d", tag, K.serial, K.uid)
end

-- Is the current host still the one on screen, in this world?
function K.alive(h)
    h = h or K.host
    if not h or h ~= K.host or not ctx then return false end
    if h.dropped then return false end
    if ctx.world_ok and not ctx.world_ok() then return false end
    return true
end

-- Start building into `host` ({ tree, canvas, kind }).
function K.begin(host)
    K.serial = K.serial + 1
    host.serial = K.serial
    K.host = host
    return host
end

-- Drop every reference (world change, host lost). Does NOT touch the widgets.
function K.forget()
    if K.host then K.host.dropped = true end
    K.host = nil
    K.font_src = nil
end

-- --- low level -----------------------------------------------------------------------

-- anchor = { ax, ay } (0..1). x, y are offsets from that anchor point.
local function place(w, anchor, x, y, wd, h, z)
    local slot
    pcall(function() slot = K.host.canvas:AddChildToCanvas(w) end)
    if not slot then return nil end
    local ax, ay = anchor[1], anchor[2]
    pcall(function() slot:SetAnchors({ Minimum = { X = ax, Y = ay }, Maximum = { X = ax, Y = ay } }) end)
    pcall(function() slot:SetAlignment({ X = 0, Y = 0 }) end)
    pcall(function() slot:SetPosition({ X = x, Y = y }) end)
    pcall(function() slot:SetSize({ X = wd, Y = h }) end)
    pcall(function() slot:SetZOrder(z or 10) end)
    return slot
end

local function style_text(tb, fs, just, color, s)
    if K.font_src then pcall(function() tb.Font = K.font_src.Font end) end
    pcall(function() tb:SetJustification(just or 0) end)
    pcall(function() tb.AutoWrapText = false end)
    pcall(function()
        local f = tb.Font
        if f then f.Size = fs; tb:SetFont(f) end
    end)
    if color then pcall(function() tb:SetColorAndOpacity({ SpecifiedColor = color, ColorUseRule = 0 }) end) end
    -- drop shadow: the HUD sits on the 3D scene, not on a panel
    local so = math.max(1, math.floor(2 * (s or 1) + 0.5))
    pcall(function() tb:SetShadowOffset({ X = so, Y = so }) end)
    pcall(function() tb:SetShadowColorAndOpacity({ R = 0, G = 0, B = 0, A = 0.85 }) end)
end

-- --- primitives ----------------------------------------------------------------------

-- Flat colour block. Never hit-testable.
function K.rect(anchor, x, y, w, h, color, z)
    if not K.alive() then return nil end
    local b = ctx.construct("/Script/UMG.Border", K.host.tree, uname("r"))
    if not b then return nil end
    pcall(function() b:SetBrushColor(color or K.C.panel) end)
    pcall(function() b:SetVisibility(VIS_HIT_INVISIBLE) end)
    local slot = place(b, anchor, x, y, w, h, z or 10)
    return { w = b, slot = slot, anchor = anchor, rect = { x, y, w, h }, color = color, shown = true, op = 1 }
end

function K.set_rect_color(r, color)
    if not r or not r.w or r.color == color or not K.alive() then return end
    r.color = color
    pcall(function() r.w:SetBrushColor(color) end)
end

function K.resize(r, x, y, w, h)
    if not r or not r.slot or not K.alive() then return end
    local o = r.rect
    if o and o[1] == x and o[2] == y and o[3] == w and o[4] == h then return end
    r.rect = { x, y, w, h }
    pcall(function() r.slot:SetPosition({ X = x, Y = y }) end)
    pcall(function() r.slot:SetSize({ X = w, Y = h }) end)
end

-- Text, vertically centred in (y, h). Single line, clipped with K.fit.
function K.text(anchor, label, x, y, w, h, fs, just, color, opts)
    if not K.alive() then return nil end
    opts = opts or {}
    fs = math.max(1, math.floor(fs or 16))       -- callers size fonts with K.fs (minimum applied)
    local tb = ctx.construct("/Script/UMG.TextBlock", K.host.tree, uname("t"))
    if not tb then return nil end
    local s = K.fit(label, w, fs, 1)
    pcall(function() tb:SetText(FText(s)) end)
    style_text(tb, fs, just, color, opts.scale)
    local th = math.min(h, math.ceil(fs * K.LINE_H))
    local ty = y + math.floor((h - th) / 2)
    local slot = place(tb, anchor, x, ty, w, th, opts.z or 13)
    pcall(function() tb:SetVisibility(VIS_HIT_INVISIBLE) end)
    return { tb = tb, slot = slot, anchor = anchor, last = s, color = color, w = w, fs = fs,
             rect = { x, ty, w, th }, shown = true, op = 1 }
end

function K.set_text(t, s)
    if not t or not t.tb then return end
    s = K.fit(s, t.w, t.fs, 1)
    if t.last == s or not K.alive() then return end
    t.last = s
    pcall(function() t.tb:SetText(FText(s)) end)
end

function K.set_color(t, color)
    if not t or not t.tb or not color or t.color == color or not K.alive() then return end
    t.color = color
    pcall(function() t.tb:SetColorAndOpacity({ SpecifiedColor = color, ColorUseRule = 0 }) end)
end

-- Show / hide any element ({ w = border } or { tb = text }).
function K.show(e, shown)
    if not e or not K.alive() then return end
    shown = shown and true or false
    if e.shown == shown then return end
    e.shown = shown
    local w = e.tb or e.w
    pcall(function() w:SetVisibility(shown and VIS_HIT_INVISIBLE or VIS_COLLAPSED) end)
end

-- Render opacity 0..1 (kill-feed fade). Quantised to avoid per-tick churn.
function K.opacity(e, a)
    if not e or not K.alive() then return end
    a = math.floor(K.clamp(a or 1, 0, 1) * 20 + 0.5) / 20
    if e.op == a then return end
    e.op = a
    local w = e.tb or e.w
    pcall(function() w:SetRenderOpacity(a) end)
end

-- Horizontal bar: track + fill (+ optional centred label).
function K.bar(anchor, x, y, w, h, fill_color, label_fs, z)
    local b = { x = x, y = y, w = w, h = h, anchor = anchor }
    z = z or 11
    b.track = K.rect(anchor, x, y, w, h, K.C.track, z)
    b.fill = K.rect(anchor, x, y, math.max(1, math.floor(w * 0.5)), h, fill_color or K.C.good, z + 1)
    if label_fs then b.label = K.text(anchor, "", x, y, w, h, label_fs, 1, K.C.white, { z = z + 2 }) end
    return b
end

function K.bar_set(b, frac, color, label)
    if not b then return end
    frac = K.clamp(frac or 0, 0, 1)
    local fw = math.max(1, math.floor(b.w * frac))
    K.resize(b.fill, b.x, b.y, fw, b.h)
    K.show(b.fill, frac > 0)
    if color then K.set_rect_color(b.fill, color) end
    if b.label then K.set_text(b.label, label or "") end
end

function K.bar_show(b, shown)
    if not b then return end
    K.show(b.track, shown)
    K.show(b.fill, shown and (b.fill.rect[3] or 0) > 1)
    if b.label then K.show(b.label, shown) end
end

-- ctx: { construct(path, outer, name), Log, world_ok() }
function K.init(c)
    ctx = c
    Log = c.Log or Log
end

return K
