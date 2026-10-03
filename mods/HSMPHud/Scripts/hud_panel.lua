-- HSMPHud / hud_panel.lua — the interactive panels: the connection modal
-- ("Connection lost: <reason>" with RECONNECT / BACK TO MENU), the MP pause
-- (RESUME / SETTINGS / LEAVE MATCH / QUIT GAME; the world never pauses) and the
-- match-result buttons (REMATCH / BACK TO LOBBY). What to show comes from
-- hud_model.panel (pure); this file only builds and polls widgets.
--
-- Unlike the HUD (hud_kit: display only, HitTestInvisible), a panel takes
-- clicks, so it is its own UserWidget in the viewport (z 2000), built when a
-- panel opens and removed when it closes. Buttons are a flat Border + a
-- transparent UButton + a label (HSMPMenu ui_kit's pattern); clicks are
-- edge-polled with IsPressed every 33 ms (OnClicked cannot be bound from
-- Lua). In an arena the cursor and UI input are enabled only while a panel is
-- open; in the menu world the input mode is left alone.
--
-- World guard: the panel's widgets are outered to their world. P.forget()
-- drops every reference WITHOUT touching it (world change); nothing is
-- touched unless P.alive().

local P = {}
local K = require("hud_kit")      -- palette, ascii/fit, ui_scale only

local ctx        -- { construct(path, outer, name), Log, world_ok(), UEHelpers }
local Log = function() end

P.Z = 2000
P.cur = nil      -- { key, kind, uw, tree, canvas, btns = { {id, button, bg, label, style, disabled, prev, hovered} }, input }
P.serial = 0

local VIS_VISIBLE, VIS_COLLAPSED, VIS_HIT_INVISIBLE = 0, 1, 3

-- button colours (normal / hovered)
P.STYLE = {
    primary = { n = { R = 0.30, G = 0.17, B = 0.03, A = 0.95 }, h = { R = 0.45, G = 0.27, B = 0.06, A = 1 } },
    normal  = { n = { R = 0.06, G = 0.05, B = 0.04, A = 0.92 }, h = { R = 0.14, G = 0.11, B = 0.07, A = 1 } },
    danger  = { n = { R = 0.22, G = 0.04, B = 0.03, A = 0.92 }, h = { R = 0.36, G = 0.07, B = 0.05, A = 1 } },
    off     = { n = { R = 0.05, G = 0.05, B = 0.05, A = 0.80 }, h = { R = 0.05, G = 0.05, B = 0.05, A = 0.80 } },
}

function P.init(c)
    ctx = c
    Log = c.Log or Log
end

function P.alive()
    return P.cur ~= nil and not P.cur.dropped and ctx ~= nil and ctx.world_ok()
end

-- World change: drop every reference untouched.
function P.forget()
    if P.cur then P.cur.dropped = true end
    P.cur = nil
end

local function uname(tag)
    P.serial = P.serial + 1
    return string.format("HSMPPANEL_%s_%d", tag, P.serial)
end

-- centre-anchored placement (x, y relative to the canvas centre)
local function place(canvas, w, x, y, wd, h, z, full)
    local slot
    pcall(function() slot = canvas:AddChildToCanvas(w) end)
    if not slot then return nil end
    if full then
        pcall(function() slot:SetAnchors({ Minimum = { X = 0, Y = 0 }, Maximum = { X = 1, Y = 1 } }) end)
        pcall(function() slot:SetOffsets({ Left = 0, Top = 0, Right = 0, Bottom = 0 }) end)
    else
        pcall(function() slot:SetAnchors({ Minimum = { X = 0.5, Y = 0.5 }, Maximum = { X = 0.5, Y = 0.5 } }) end)
        pcall(function() slot:SetAlignment({ X = 0, Y = 0 }) end)
        pcall(function() slot:SetPosition({ X = x, Y = y }) end)
        pcall(function() slot:SetSize({ X = wd, Y = h }) end)
    end
    pcall(function() slot:SetZOrder(z) end)
    return slot
end

local function rect(c, x, y, w, h, color, z, full)
    local b = ctx.construct("/Script/UMG.Border", c.tree, uname("r"))
    if not b then return nil end
    pcall(function() b:SetBrushColor(color) end)
    pcall(function() b:SetVisibility(VIS_HIT_INVISIBLE) end)
    place(c.canvas, b, x, y, w, h, z, full)
    return b
end

local function text(c, s, x, y, w, h, fs, just, color, z)
    local tb = ctx.construct("/Script/UMG.TextBlock", c.tree, uname("t"))
    if not tb then return nil end
    pcall(function() tb:SetText(FText(K.fit(s, w, fs, 1))) end)
    pcall(function() tb:SetJustification(just or 1) end)
    pcall(function() local f = tb.Font; if f then f.Size = fs; tb:SetFont(f) end end)
    pcall(function() tb:SetColorAndOpacity({ SpecifiedColor = color or K.C.text, ColorUseRule = 0 }) end)
    pcall(function() tb:SetShadowOffset({ X = 1, Y = 1 }) end)
    pcall(function() tb:SetShadowColorAndOpacity({ R = 0, G = 0, B = 0, A = 0.85 }) end)
    pcall(function() tb:SetVisibility(VIS_HIT_INVISIBLE) end)
    local th = math.min(h, math.ceil(fs * K.LINE_H))
    place(c.canvas, tb, x, y + math.floor((h - th) / 2), w, th, z)
    return tb
end

-- Split a sentence into at most `n` lines of `per` characters.
local function wrap(s, per, n)
    s = K.ascii(s or "")
    local lines, cur = {}, ""
    for word in s:gmatch("%S+") do
        if #cur == 0 then cur = word
        elseif #cur + 1 + #word <= per then cur = cur .. " " .. word
        else lines[#lines + 1] = cur; cur = word end
    end
    if #cur > 0 then lines[#lines + 1] = cur end
    while #lines > n do
        lines[n] = lines[n] .. " " .. table.remove(lines, n + 1)
    end
    return lines
end
P.wrap = wrap

local function paint(b)
    local st = P.STYLE[b.disabled and "off" or (b.style or "normal")] or P.STYLE.normal
    local key = (b.hovered and not b.disabled) and "h" or "n"
    if b.painted == key then return end
    b.painted = key
    pcall(function() b.bg:SetBrushColor(st[key]) end)
end

-- The panels' smallest design font (body text): their scale never goes under
-- the one where it meets the minimum font (small windows), like the HUD.
P.MIN_DESIGN_FS = 19

-- Pure layout (canvas units, relative to the canvas centre). Exposed for tests.
-- dpi: the canvas' DPI scale (shared/hsmp_ui_scale.lua minimum font).
function P.layout(spec, cw, ch, dpi)
    if dpi then K.set_dpi(dpi) end
    local s = K.layout_scale(cw, ch, P.MIN_DESIGN_FS)
    local n = #spec.buttons
    local L = { s = s, cw = cw, ch = ch, dpi = K.dpi }
    local pad = math.floor(24 * s)
    local bh = math.floor(52 * s)
    local gap = math.floor(12 * s)
    L.label_pad = math.max(2, math.floor(8 * s))
    -- the safe width: the shared margin, never wider than 16:9 of the height
    local maxw = K.uis and K.uis.max_width(cw, ch, s) or (cw - 2 * pad)
    if spec.kind == "result" then
        -- a button row under the centre banner (no box, no backdrop)
        local bw = math.floor(380 * s)              -- fits "REMATCH: STARTING WHEN READY"
        if n * bw + (n - 1) * gap > maxw then bw = math.floor((maxw - (n - 1) * gap) / math.max(1, n)) end
        local W = n * bw + (n - 1) * gap
        L.box = nil
        L.buttons = {}
        local y0 = math.floor(ch * 0.05)            -- just under the banner (anchor 0.5, 0.3)
        for i = 1, n do L.buttons[i] = { x = -math.floor(W / 2) + (i - 1) * (bw + gap), y = y0, w = bw, h = bh } end
        L.fs_btn = K.fs(20, s)
        return L
    end
    local W = math.floor(math.min(760 * s, maxw))
    local th, lh = math.floor(52 * s), math.floor(30 * s)
    L.fs_title, L.fs_text, L.fs_btn = K.fs(34, s), K.fs(19, s), K.fs(20, s)
    local per = math.max(10, math.floor((W - 2 * pad) / (L.fs_text * K.CHAR_W)))
    L.lines = wrap(spec.text, per, 3)
    local body = th + #L.lines * lh + gap
    local rows = spec.vertical and n or 1
    local H = pad + body + rows * bh + (rows - 1) * gap + pad
    L.box = { x = -math.floor(W / 2), y = -math.floor(H / 2), w = W, h = H }
    L.title = { x = L.box.x + pad, y = L.box.y + pad, w = W - 2 * pad, h = th }
    L.text = {}
    for i = 1, #L.lines do
        L.text[i] = { x = L.box.x + pad, y = L.box.y + pad + th + (i - 1) * lh, w = W - 2 * pad, h = lh }
    end
    L.buttons = {}
    local by = L.box.y + pad + body
    if spec.vertical then
        local bw = W - 2 * pad
        for i = 1, n do L.buttons[i] = { x = L.box.x + pad, y = by + (i - 1) * (bh + gap), w = bw, h = bh } end
    else
        local bw = math.floor((W - 2 * pad - (n - 1) * gap) / math.max(1, n))
        for i = 1, n do L.buttons[i] = { x = L.box.x + pad + (i - 1) * (bw + gap), y = by, w = bw, h = bh } end
    end
    return L
end

-- Build `spec` into a new UserWidget in the viewport. Returns true on success.
function P.build(spec, cw, ch, dpi)
    local UEH = ctx.UEHelpers
    local world = UEH.GetWorld()
    if not world or not world:IsValid() then return false end
    local uw = ctx.construct("/Script/UMG.UserWidget", world, uname("uw"))
    if not uw then return false end
    local tree = ctx.construct("/Script/UMG.WidgetTree", uw, uname("wt"))
    if not tree then return false end
    uw.WidgetTree = tree
    local canvas = ctx.construct("/Script/UMG.CanvasPanel", tree, uname("cv"))
    if not canvas then return false end
    tree.RootWidget = canvas
    local c = { key = spec.key, kind = spec.kind, modal = spec.modal, uw = uw, tree = tree, canvas = canvas, btns = {},
                cw = cw, ch = ch, dpi = dpi }
    local L = P.layout(spec, cw, ch, dpi)
    if spec.modal then
        -- dim the scene; Visible so a click never falls through to the menu / game behind
        local back = ctx.construct("/Script/UMG.Border", tree, uname("back"))
        if back then
            pcall(function() back:SetBrushColor({ R = 0, G = 0, B = 0, A = 0.55 }) end)
            pcall(function() back:SetVisibility(VIS_VISIBLE) end)
            place(canvas, back, 0, 0, 0, 0, 1, true)
        end
    end
    if L.box then
        rect(c, L.box.x, L.box.y, L.box.w, L.box.h, K.C.panel, 2)
        rect(c, L.box.x, L.box.y, L.box.w, math.max(2, math.floor(3 * L.s)), K.C.rule, 3)
        text(c, spec.title or "", L.title.x, L.title.y, L.title.w, L.title.h, L.fs_title, 1,
            (spec.tone == "bad") and K.C.bad or K.C.title, 4)
        for i, line in ipairs(L.lines) do
            local g = L.text[i]
            text(c, line, g.x, g.y, g.w, g.h, L.fs_text, 1, K.C.white, 4)
        end
    end
    for i, b in ipairs(spec.buttons) do
        local g = L.buttons[i]
        local e = { id = b.id, style = b.style or "normal", disabled = b.disabled and true or false }
        e.bg = rect(c, g.x, g.y, g.w, g.h, (P.STYLE[e.style] or P.STYLE.normal).n, 5)
        local btn = ctx.construct("/Script/UMG.Button", tree, uname("b"))
        if btn then
            pcall(function() btn.IsFocusable = false end)
            pcall(function() btn:SetBackgroundColor({ R = 0, G = 0, B = 0, A = 0 }) end)
            pcall(function() btn:SetVisibility(VIS_VISIBLE) end)
            place(canvas, btn, g.x, g.y, g.w, g.h, 6)
            e.button = btn
        end
        e.label = text(c, b.label, g.x + L.label_pad, g.y, g.w - 2 * L.label_pad, g.h, L.fs_btn, 1,
            e.disabled and K.C.dim or K.C.text, 7)
        paint(e)
        c.btns[#c.btns + 1] = e
    end
    pcall(function() uw:AddToViewport(P.Z) end)
    P.cur = c
    return true
end

-- Remove the panel (same world only).
function P.close()
    if P.alive() then pcall(function() P.cur.uw:RemoveFromParent() end) end
    P.cur = nil
end

-- Edge-poll the buttons (33 ms loop). Calls on_click(id) on a press.
function P.poll(on_click)
    if not P.alive() then return end
    local c = P.cur
    for _, b in ipairs(c.btns) do
        if b.button then
            local pressed, hovered = false, false
            pcall(function() pressed = b.button:IsPressed() end)
            pcall(function() hovered = b.button:IsHovered() end)
            hovered = hovered and true or false
            if hovered ~= b.hovered then b.hovered = hovered; paint(b) end
            if pressed and not b.prev then
                b.prev = true                       -- latch before firing (no repeat while held)
                if not b.disabled then
                    pcall(on_click, b.id)
                    return                          -- the panel may be rebuilt now
                end
            end
            b.prev = pressed and true or false
        end
        if P.cur ~= c then return end
    end
end

-- Cursor + UI input while a panel is open in an arena; game-only afterwards.
function P.set_input(on)
    local UEH = ctx.UEHelpers
    pcall(function()
        local pc = UEH.GetPlayerController()
        if not (pc and pc:IsValid()) then return end
        local wbl = StaticFindObject("/Script/UMG.Default__WidgetBlueprintLibrary")
        if on then
            if wbl and wbl:IsValid() then pcall(function() wbl:SetInputMode_GameAndUIEx(pc, nil, 0, false) end) end
            pcall(function() pc:SetShowMouseCursor(true) end)
            pcall(function() pc.bShowMouseCursor = true end)
        else
            if wbl and wbl:IsValid() then pcall(function() wbl:SetInputMode_GameOnly(pc, false) end) end
            pcall(function() pc:SetShowMouseCursor(false) end)
            pcall(function() pc.bShowMouseCursor = false end)
        end
    end)
end

return P
