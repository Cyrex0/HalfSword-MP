-- HSMPMenu / ui_kit.lua — shared flat widget kit for the HSMP menu screens
-- (lobby, loadout, server browser, settings, character).
--
-- Every screen is a native sub-screen: all widgets go onto the Startup menu's
-- own CanvasPanel (no overlay, no viewport widget, no scroll box).
--
-- Look: flat colour blocks, not the game's parchment "scroll" ribbons.
--   button = UBorder (flat colour, z)  +  transparent UButton (hit + IsPressed,
--            z+1)  +  TextBlock label(s) (HitTestInvisible, z+2)
-- Hover/selected/disabled/pending/pressed are shown by recolouring the Border.
--
-- Layout: every screen is laid out in DESIGN UNITS (a 1920x1080 canvas) and
-- multiplied by one scale s (shared/hsmp_ui_scale.lua: the canvas short side
-- / 1080, fitted to the 1920-wide design), where (cw, ch) is the canvas
-- (viewport / DPI scale, probed fresh through the layout library). The whole
-- menu is therefore proportional at every resolution, aspect and DPI; panels
-- are centred inside the safe margin and never wider than 16:9 of the height
-- (ultrawide); fonts never go below K.MIN_PX physical pixels (down to the
-- 1024x576 floor window). A resize / resolution / DPI change rebuilds the open
-- screen (main.lua MX.relayout; K.relayout is true during that build).
-- Spacing, type, button sizes: K.SP, K.TS, K.BH, K.BW (the one scale for all).
--
-- Focus + input (keyboard / gamepad parity, docs/development/subsystems/menu-ui.md "Navigation"):
--   * every button / tile / chip / input is focusable (opts.focus = false
--     opts out) and belongs to a focus group (K.group); the focused one gets
--     an amber ring (4 thin Borders, moved in place);
--   * main.lua feeds actions into K.key(action, device): arrows/D-pad move
--     (spatial nearest neighbour), Enter/A activates (the same callback as a
--     click), Esc/B goes back, Tab/Shift+Tab cycles groups, PgUp/PgDn page,
--     Q/E (LB/RB) switch tabs, F5 refreshes;
--   * text guard: while an input has keyboard focus only Enter (done), Esc
--     (cancel) and Tab (next) reach the menu; everything else is typing;
--   * hovering with the mouse moves the focus too (one visual state);
--   * the footer hint bar shows the keys (keyboard or gamepad labels, by the
--     last device used) and the focused control's help / disabled reason.
--
-- Clicks: main.lua edge-polls IsPressed every 33 ms over
-- state.screen_widgets; every clickable widget here is registered there.
--
-- World guard: every build is tagged with the menu world generation.
-- K.alive() is false after a level load (main.lua bumps the generation in its
-- LoadMap pre-hook and calls K.forget()), after screen exit, or when another
-- screen is active; nothing here touches a widget unless K.alive() holds.
-- K.forget() drops references WITHOUT touching them.
--
-- Performance: nothing is rebuilt per frame. Builds happen on screen entry;
-- every later change is an in-place setter that writes only when the value
-- changed. K.perf counts ticks, renders, widget writes and builds and logs the
-- menu's Lua cost every 10 s while a screen is open.

local K = {}

local ctx                    -- { state, construct, clone_text_look, world_gen(), Log }
local Log = function() end

K.cur = nil                  -- current build (K.begin)
K.serial = 0
K.uid = 0

-- Text width estimate used for clipping (and by the offline layout test).
K.CHAR_W = 0.60              -- average glyph advance as a fraction of the font size
K.LINE_H = 1.40              -- line height as a fraction of the font size

-- --- design tokens (design units at 1920x1080; multiply by the scale) ------------------
K.SP = { xs = 4, sm = 8, md = 12, lg = 16, xl = 24, xxl = 32 }     -- spacing scale
K.TS = { title = 34, h1 = 24, h2 = 19, body = 17, label = 15, small = 14 } -- type scale
K.BH = { action = 56, field = 44, chip = 40, row = 40, tab = 36, hint = 34 } -- control heights
K.BW = { action = 240, chip_max = 260 }
K.PANEL = { w = 1760, h = 1000, pad = 24 }
K.MIN_PX = 9                 -- smallest font size in physical pixels (fs * dpi)

-- colours (linear RGBA)
K.C = {
    panel     = { R = 0.012, G = 0.010, B = 0.008, A = 0.93 },
    section   = { R = 0.024, G = 0.019, B = 0.014, A = 0.92 },
    track     = { R = 0.006, G = 0.005, B = 0.004, A = 0.95 },
    rule      = { R = 0.30,  G = 0.20,  B = 0.08,  A = 0.85 },
    focus     = { R = 1.00,  G = 0.82,  B = 0.36,  A = 1 },

    text      = { R = 0.86, G = 0.79, B = 0.64, A = 1 },
    dim       = { R = 0.34, G = 0.31, B = 0.26, A = 1 },
    head      = { R = 1.00, G = 0.66, B = 0.26, A = 1 },
    on_text   = { R = 0.02, G = 0.015, B = 0.01, A = 1 },
    good      = { R = 0.28, G = 0.85, B = 0.30, A = 1 },
    ok        = { R = 0.95, G = 0.76, B = 0.20, A = 1 },
    warn      = { R = 1.00, G = 0.45, B = 0.10, A = 1 },
    bad       = { R = 1.00, G = 0.20, B = 0.13, A = 1 },
}

-- Border colour per style and state.
local P = {
    normal  = { n = { 0.045, 0.038, 0.030, 0.95 }, h = { 0.110, 0.085, 0.058, 0.98 },
                on = { 0.70, 0.40, 0.07, 1 }, onh = { 0.86, 0.52, 0.10, 1 }, off = { 0.020, 0.018, 0.015, 0.70 } },
    primary = { n = { 0.40, 0.18, 0.03, 1 },  h = { 0.58, 0.27, 0.05, 1 },
                on = { 0.70, 0.40, 0.07, 1 }, onh = { 0.86, 0.52, 0.10, 1 }, off = { 0.06, 0.04, 0.02, 0.75 } },
    danger  = { n = { 0.26, 0.035, 0.025, 1 }, h = { 0.42, 0.06, 0.04, 1 },
                on = { 0.55, 0.08, 0.05, 1 }, onh = { 0.65, 0.10, 0.06, 1 }, off = { 0.06, 0.02, 0.02, 0.75 } },
    row     = { n = { 0.030, 0.025, 0.020, 0.92 }, h = { 0.090, 0.070, 0.048, 0.96 },
                on = { 0.62, 0.36, 0.06, 1 }, onh = { 0.78, 0.47, 0.09, 1 }, off = { 0.018, 0.016, 0.013, 0.70 } },
    rowb    = { n = { 0.020, 0.017, 0.014, 0.92 }, h = { 0.090, 0.070, 0.048, 0.96 },
                on = { 0.62, 0.36, 0.06, 1 }, onh = { 0.78, 0.47, 0.09, 1 }, off = { 0.014, 0.012, 0.010, 0.70 } },
    tile    = { n = { 0.040, 0.033, 0.025, 0.95 }, h = { 0.115, 0.088, 0.058, 0.98 },
                on = { 0.62, 0.36, 0.06, 1 }, onh = { 0.78, 0.47, 0.09, 1 }, off = { 0.016, 0.014, 0.012, 0.80 } },
    head    = { n = { 0.090, 0.060, 0.025, 0.96 }, h = { 0.160, 0.110, 0.045, 0.98 },
                on = { 0.24, 0.16, 0.06, 1 }, onh = { 0.30, 0.20, 0.08, 1 }, off = { 0.05, 0.04, 0.03, 0.8 } },
}
local function rgba(t) return { R = t[1], G = t[2], B = t[3], A = t[4] } end

-- "Pending" (a request sent, no server answer yet): the same amber for every
-- style, dimmer than "on" so the server's selection stays the obvious one.
local PEND = { pend = { 0.30, 0.19, 0.04, 1 }, pendh = { 0.40, 0.26, 0.06, 1 } }

-- --- perf (menu frame cost) --------------------------------------------------------------

K.perf = { ticks = 0, tick_ms = 0, tick_max = 0, renders = 0, render_ms = 0, render_max = 0,
           polls = 0, poll_ms = 0, writes = 0, builds = 0, t0 = nil, report = nil }
local PERF_EVERY_S = 10

local function W() K.perf.writes = K.perf.writes + 1 end

-- kind: "tick" | "render" | "poll"; ms: wall time of one call
function K.perf_add(kind, ms)
    local p = K.perf
    if kind == "tick" then
        p.ticks = p.ticks + 1; p.tick_ms = p.tick_ms + ms; if ms > p.tick_max then p.tick_max = ms end
    elseif kind == "render" then
        p.renders = p.renders + 1; p.render_ms = p.render_ms + ms; if ms > p.render_max then p.render_max = ms end
    else
        p.polls = p.polls + 1; p.poll_ms = p.poll_ms + ms
    end
end

local function perf_reset(p, now)
    p.t0, p.ticks, p.tick_ms, p.tick_max, p.renders, p.render_ms, p.render_max = now, 0, 0, 0, 0, 0, 0
    p.polls, p.poll_ms, p.writes, p.builds = 0, 0, 0, 0
end

local function perf_report(now)
    local p = K.perf
    if not p.t0 then perf_reset(p, now); return end
    local dt = now - p.t0
    if dt < PERF_EVERY_S then return end
    local busy = p.tick_ms + p.render_ms + p.poll_ms
    p.report = { screen = K.cur and K.cur.screen or "-", secs = dt, ticks = p.ticks, renders = p.renders,
                 writes = p.writes, builds = p.builds, ms_per_s = busy / dt,
                 tick_avg = p.ticks > 0 and p.tick_ms / p.ticks or 0, tick_max = p.tick_max,
                 render_avg = p.renders > 0 and p.render_ms / p.renders or 0, render_max = p.render_max }
    local r = p.report
    Log("menu perf (%s, %.0f s): %.2f ms/s Lua | tick avg %.2f max %.1f ms (%d) | render avg %.2f max %.1f ms (%d) | %d widget writes | %d builds",
        r.screen, dt, r.ms_per_s, r.tick_avg, r.tick_max, r.ticks, r.render_avg, r.render_max, r.renders, r.writes, r.builds)
    perf_reset(p, now)
end

-- --- small helpers ---------------------------------------------------------

function K.ascii(s)
    -- FText from Lua strings is not reliably UTF-8 aware here; keep it ASCII.
    return (tostring(s or ""):gsub("[\128-\255]+", "?"):gsub("[%c]", " "))
end

-- Clip `s` so it fits `w` units at font size `fs` (on `lines` lines).
-- A string that is too long first loses its padding (runs of spaces -> one
-- space, so "TIER II  |  3 PTS" still fits a small tile), then is clipped.
function K.fit(s, w, fs, lines)
    s = K.ascii(s)
    local per = math.max(1, math.floor((w or 0) / ((fs or 16) * K.CHAR_W)))
    local n = per * (lines or 1)
    if #s > n then
        local c = s:gsub("  +", " ")
        if #c <= n then return c end
        return c:sub(1, math.max(1, n - 2)) .. ".."
    end
    return s
end

function K.clamp(v, a, b) if v < a then return a elseif v > b then return b end return v end

function K.now() return os.clock() end

local function uname(tag)
    K.uid = K.uid + 1
    local scr = (K.cur and K.cur.screen) or "x"
    return string.format("HSMPUI_%s_%s_%d_%d", scr, tag, K.serial, K.uid)
end

-- Is this build still the one on screen, in this world? Never touch a widget
-- unless this is true.
function K.alive(b)
    b = b or K.cur
    if not b or not ctx or b ~= K.cur then return false end
    local st = ctx.state
    if not st or not st.injected or not st.canvas or not st.wt then return false end
    if st.screen_active ~= b.screen then return false end
    return b.gen == ctx.world_gen()
end

-- Drop every reference of the current build (screen exit / world change).
-- Does NOT touch the widgets.
function K.forget()
    K.cur = nil
end

K.focus_mem = {}             -- screen -> fkey of the last focused control (focus restore)
K.device = "kb"              -- "kb" | "pad": the last input device (hint labels)

-- Start a build for `screen`. Returns the build table; screens keep their
-- widget refs inside it (b.w) so forgetting the build forgets them all.
function K.begin(screen)
    K.serial = K.serial + 1
    K.perf.builds = K.perf.builds + 1
    K.cur = { screen = screen, gen = ctx.world_gen(), serial = K.serial,
              hover = {}, ticks = {}, w = {}, n = 0,
              focus = {}, groups = {}, group_last = {}, group = "main", inputs = {}, spinners = {},
              keys = {}, nfocus = 0 }
    return K.cur
end

-- Scaling: shared/hsmp_ui_scale.lua (UIS) is the one rule (design 1920x1080,
-- s from the canvas short side, fonts >= MIN_PX physical px, safe margins, max
-- panel width). main.lua passes it in (ctx.uis) with ctx.measure(), which
-- probes the viewport fresh on the game thread. Without the shared lib (a
-- broken deploy) the plain proportional rule below keeps the menu usable.
local UIS

-- Canvas size in slate units (the DPI scale divides the viewport) and the DPI scale.
function K.canvas_size()
    local M = K.measure()
    return M.cw, M.ch, M.dpi
end

-- The current viewport metrics (fresh): { vw, vh, cw, ch, dpi, s, m, src }.
function K.measure()
    if ctx and ctx.measure then
        local ok, M = pcall(ctx.measure)
        if ok and type(M) == "table" and M.cw then return M end
    end
    -- fallback (no shared lib): viewport from main.lua, DPI from the layout library
    local st = ctx.state
    local vw, vh = st.vw or 1920, st.vh or 1080
    local scale
    pcall(function()
        local wll = StaticFindObject("/Script/UMG.Default__WidgetLayoutLibrary")
        if wll and wll:IsValid() then scale = wll:GetViewportScale(st.menu) end
    end)
    if type(scale) ~= "number" or scale <= 0.1 then scale = math.min(vw, vh) / 1080 end
    local cw, ch = vw / scale, vh / scale
    return { vw = vw, vh = vh, cw = cw, ch = ch, dpi = scale, s = K.ui_scale(cw, ch), m = 2, src = "kit-fallback" }
end

-- UI scale factor relative to a 1920x1080 canvas (the design space).
function K.ui_scale(cw, ch)
    if UIS then return UIS.scale(cw, ch) end
    return K.clamp(math.min(cw / 1920, ch / 1080), 0.25, 2.5)
end

-- Scale helpers for one build: u(n) design units -> canvas units, F(n) font size.
function K.metrics()
    local G = K.measure()
    local cw, ch, dpi = G.cw, G.ch, G.dpi
    local s = K.ui_scale(cw, ch)
    local M = { cw = cw, ch = ch, dpi = dpi, s = s, vw = G.vw, vh = G.vh, src = G.src }
    -- smallest font in canvas units (UIS: MIN_PX physical down to the floor window)
    K.minfs = UIS and UIS.min_font(dpi, s) or math.ceil(K.MIN_PX / math.max(0.1, dpi) - 1e-9)
    K.s = s                                                   -- the scale of the build being made
    K.last_metrics = M
    M.u = function(n) return math.floor(n * s + 0.5) end
    M.F = function(n) return math.max(math.floor(n * s), K.minfs) end
    -- a centred panel of design size (dw, dh): scaled, inside the safe area, never wider than 16:9
    M.panel = function(dw, dh)
        if UIS then return UIS.panel(dw, dh, s, cw, ch) end
        return math.min(M.u(dw), math.floor(cw) - 2), math.min(M.u(dh), math.floor(ch) - 2)
    end
    -- a text line box for font fs (at least the design height n)
    M.lh = function(fs, n, lines) return math.max(n and M.u(n) or 0, math.ceil(fs * K.LINE_H * (lines or 1))) end
    return M
end

-- --- low level ------------------------------------------------------------------

local function track(w, cb, nopoll)
    local e = { button = w, cb = cb or function() end, prev = false, nopoll = nopoll and true or nil }
    table.insert(ctx.state.screen_widgets, e)
    if K.cur then K.cur.n = K.cur.n + 1 end
    return e
end

local function place(w, x, y, wd, h, z)
    local slot
    pcall(function() slot = ctx.state.canvas:AddChildToCanvas(w) end)
    if not slot then return nil end
    pcall(function() slot:SetAnchors({ Minimum = { X = 0.5, Y = 0.5 }, Maximum = { X = 0.5, Y = 0.5 } }) end)
    pcall(function() slot:SetAlignment({ X = 0, Y = 0 }) end)
    pcall(function() slot:SetPosition({ X = x, Y = y }) end)
    pcall(function() slot:SetSize({ X = wd, Y = h }) end)
    pcall(function() slot:SetZOrder(z or 10) end)
    return slot
end

local function move(slot, x, y, wd, h)
    if not slot then return end
    W()
    pcall(function() slot:SetPosition({ X = x, Y = y }) end)
    pcall(function() slot:SetSize({ X = wd, Y = h }) end)
end

local function style_text(tb, fs, just, color, wrap)
    if ctx.state.text_src then pcall(ctx.clone_text_look, tb, ctx.state.text_src) end
    pcall(function() tb:SetJustification(just or 0) end)
    pcall(function() tb.Justification = just or 0 end)
    pcall(function() tb.AutoWrapText = wrap and true or false end)
    if fs then
        pcall(function()
            local f = tb.Font
            if f then f.Size = fs; tb:SetFont(f) end
        end)
    end
    if color then pcall(function() tb:SetColorAndOpacity({ SpecifiedColor = color, ColorUseRule = 0 }) end) end
    -- no heavy drop shadow on flat UI (the cloned ribbon font has one)
    pcall(function() tb:SetShadowColorAndOpacity({ R = 0, G = 0, B = 0, A = 0.6 }) end)
end

local function set_vis(w, v) if w then W(); pcall(function() w:SetVisibility(v) end) end end

-- --- primitives ---------------------------------------------------------------------

-- Flat colour block (panel, section, bar track/fill). Not clickable.
function K.rect(x, y, w, h, color, z)
    local b = ctx.construct("/Script/UMG.Border", ctx.state.wt, uname("r"))
    if not b then return nil end
    pcall(function() b:SetBrushColor(color or K.C.panel) end)
    pcall(function() b:SetVisibility(3) end)            -- HitTestInvisible
    local slot = place(b, x, y, w, h, z or 10)
    track(b, nil, true)
    return { w = b, slot = slot, rect = { x, y, w, h }, color = color, shown = true }
end

function K.set_rect_color(r, color)
    if not r or not r.w or r.color == color or not K.alive() then return end
    r.color = color
    W()
    pcall(function() r.w:SetBrushColor(color) end)
end

function K.resize(r, x, y, w, h)
    if not r or not K.alive() then return end
    local o = r.rect
    if o and o[1] == x and o[2] == y and o[3] == w and o[4] == h then return end
    r.rect = { x, y, w, h }
    move(r.slot, x, y, w, h)
end

function K.show_rect(r, shown)
    if not r or not r.w or not K.alive() then return end
    shown = shown and true or false
    if r.shown == shown then return end
    r.shown = shown
    set_vis(r.w, shown and 3 or 1)
end

-- Text. opts: { wrap = lines (>1 enables AutoWrapText), valign = "top"|"center", z }
-- The text box is vertically centred in (y, h) unless valign == "top".
function K.text(label, x, y, w, h, fs, just, color, opts)
    opts = opts or {}
    fs = math.floor(fs or 16)
    local lines = opts.wrap or 1
    local tb = ctx.construct("/Script/UMG.TextBlock", ctx.state.wt, uname("t"))
    if not tb then return nil end
    local s = K.fit(label, w, fs, lines)
    pcall(function() tb:SetText(FText(s)) end)
    style_text(tb, fs, just, color, lines > 1)
    local th = math.min(h, math.ceil(fs * K.LINE_H * lines))
    local ty = (opts.valign == "top") and y or (y + math.floor((h - th) / 2))
    local slot = place(tb, x, ty, w, th, opts.z or 13)
    pcall(function() tb:SetVisibility(3) end)            -- HitTestInvisible
    track(tb, nil, true)
    return { tb = tb, slot = slot, last = s, color = color, w = w, fs = fs, lines = lines,
             rect = { x, ty, w, th }, box = { x, y, w, h } }
end

function K.set_text(t, s)
    if not t or not t.tb then return end
    s = K.fit(s, t.w, t.fs, t.lines)
    if t.last == s or not K.alive() then return end
    t.last = s
    W()
    pcall(function() t.tb:SetText(FText(s)) end)
end

-- Does `s` fit the text's box unclipped?
function K.fits(t, s)
    if not t then return false end
    local per = math.max(1, math.floor((t.w or 0) / ((t.fs or 16) * K.CHAR_W)))
    return #(K.ascii(s):gsub("  +", " ")) <= per * (t.lines or 1)
end

-- Set the first of `variants` (long .. short) that fits unclipped; the last one
-- otherwise (clipped). Small windows get the short wording instead of "..".
function K.set_text_fit(t, variants)
    if not t then return end
    for _, v in ipairs(variants) do
        if v and K.fits(t, v) then K.set_text(t, v); return end
    end
    K.set_text(t, variants[#variants] or "")
end

function K.set_color(t, color)
    if not t or not t.tb or t.color == color or not color or not K.alive() then return end
    t.color = color
    W()
    pcall(function() t.tb:SetColorAndOpacity({ SpecifiedColor = color, ColorUseRule = 0 }) end)
end

function K.show_text(t, shown)
    if not t or not t.tb or not K.alive() then return end
    shown = shown and true or false
    if t.shown == shown then return end
    t.shown = shown
    set_vis(t.tb, shown and 3 or 1)
end

-- --- focus registration (used by buttons, tiles, inputs) ----------------------------------

-- The focus group new controls join (Tab / Shift+Tab cycle groups in this order).
function K.group(name)
    local b = K.cur
    if not b then return end
    b.group = name or "main"
end

local function focus_add(ref, opts)
    local b = K.cur
    if not b or (opts and opts.focus == false) then return end
    b.nfocus = b.nfocus + 1
    ref.fgroup = (opts and opts.group) or b.group or "main"
    ref.fkey = (opts and opts.fkey) or (b.screen .. "#" .. b.nfocus)
    if opts and opts.help then ref.help = opts.help end
    b.focus[#b.focus + 1] = ref
    local seen = false
    for _, g in ipairs(b.groups) do if g == ref.fgroup then seen = true; break end end
    if not seen then b.groups[#b.groups + 1] = ref.fgroup end
end

-- --- buttons --------------------------------------------------------------------------

local function paint(ref)
    if not ref or not ref.bg then return end
    local pal = P[ref.style] or P.normal
    local key
    local hot = ref.hovered or (ref.flash_until and K.now() < ref.flash_until)
    if ref.disabled then key = "off"
    elseif ref.on then key = hot and "onh" or "on"
    elseif ref.pending then key = hot and "pendh" or "pend"
    else key = hot and "h" or "n" end
    if ref.painted == key then return end
    ref.painted = key
    W()
    pcall(function() ref.bg:SetBrushColor(rgba(pal[key] or PEND[key])) end)
    -- label colour follows the state
    local tc = ref.disabled and K.C.dim or (ref.on and K.C.on_text or (ref.text_color or K.C.text))
    if ref.label then K.set_color(ref.label, tc) end
end

-- Flat button. opts: { fs, style = "normal"|"primary"|"danger"|"row"|"rowb"|"tile"|"head",
--                      just, z, label_pad, no_label, hover = true,
--                      focus = true, group, fkey, help (string | function -> string) }
-- Returns ref { bg, button, label, entry, rect, style, on, disabled }.
-- ref.reason (string | function) explains a disabled control (hint bar + click).
function K.button(label, cb, x, y, w, h, opts)
    opts = opts or {}
    local z = opts.z or 11
    local ref = { style = opts.style or "normal", rect = { x, y, w, h }, on = false, disabled = false,
                  shown = true }
    -- background
    local bg = ctx.construct("/Script/UMG.Border", ctx.state.wt, uname("bg"))
    if bg then
        pcall(function() bg:SetVisibility(3) end)
        ref.bg_slot = place(bg, x, y, w, h, z)
        track(bg, nil, true)
        ref.bg = bg
    end
    -- hit area
    local btn = ctx.construct("/Script/UMG.Button", ctx.state.wt, uname("b"))
    if not btn then return nil end
    pcall(function() btn.IsFocusable = false end)       -- Slate never focuses it: our focus manager does
    pcall(function() btn:SetBackgroundColor({ R = 0, G = 0, B = 0, A = 0 }) end)
    ref.slot = place(btn, x, y, w, h, z + 1)
    ref.button = btn
    ref.entry = track(btn, function()
        if ref.disabled then
            if ref.on_disabled then ref.on_disabled()
            elseif ref.reason then K.flash(K.reason_of(ref), K.C.warn) end
            return
        end
        if cb then cb() end
    end)
    -- label
    if not opts.no_label then
        local pad = opts.label_pad or math.max(2, math.floor(8 * (K.s or 1)))
        local fs = opts.fs or 16
        ref.label = K.text(label or "", x + pad, y, w - 2 * pad, h, fs, opts.just or 1, K.C.text, { z = z + 2 })
        -- opts.short: the wording for a button too narrow for the label (small windows)
        if opts.short and ref.label and not K.fits(ref.label, label or "") then K.set_text(ref.label, opts.short) end
    end
    if opts.hover ~= false and K.cur then table.insert(K.cur.hover, ref) end
    focus_add(ref, opts)
    paint(ref)
    return ref
end

function K.set_label(ref, s) if ref then K.set_text(ref.label, s) end end

-- on = selected / active; disabled = greyed (click calls ref.on_disabled if set)
function K.set_state(ref, on, disabled)
    if not ref or not K.alive() then return end
    ref.on = on and true or false
    ref.disabled = disabled and true or false
    paint(ref)
end

-- pending = a request for this button/chip is out and unanswered (amber).
-- "on" (the server's state) still wins over pending.
function K.set_pending(ref, pending)
    if not ref or not K.alive() then return end
    pending = pending and true or false
    if ref.pending == pending then return end
    ref.pending = pending
    paint(ref)
end

function K.set_style(ref, style)
    if not ref or ref.style == style or not K.alive() then return end
    ref.style = style; ref.painted = nil
    paint(ref)
end

-- Why a control is disabled (ref.reason may be a function).
function K.reason_of(ref)
    local r = ref and ref.reason
    if type(r) == "function" then local ok, s = pcall(r); r = ok and s or nil end
    return r
end

-- Show / hide a button (with its background and every text in ref.texts).
function K.show(ref, shown)
    if not ref or not K.alive() then return end
    shown = shown and true or false
    if ref.shown == shown then return end
    ref.shown = shown
    if ref.bg then set_vis(ref.bg, shown and 3 or 1) end
    if ref.button then set_vis(ref.button, shown and 0 or 1) end
    if ref.box then set_vis(ref.box, shown and 0 or 1) end
    if ref.frame then set_vis(ref.frame.w, shown and 3 or 1) end
    if ref.label and ref.label.tb then set_vis(ref.label.tb, shown and 3 or 1) end
    for _, t in ipairs(ref.texts or {}) do
        if t ~= ref.label then set_vis(t.tb, shown and 3 or 1) end
    end
    if not shown then ref.hovered = false; ref.painted = nil; paint(ref) end
end

-- Editable text box (polled; OnTextChanged is not Lua-bindable). Focusable:
-- Enter / A starts typing, Enter again commits (ref.on_submit(text)).
-- opts: { group, fkey, help, on_submit, focus }
function K.input(hint, initial, x, y, w, h, fs, opts)
    opts = opts or {}
    fs = math.floor(fs or 16)
    -- a flat frame behind the box so it reads as part of the kit even when
    -- the engine's default box style cannot be retinted
    local frame = K.rect(x, y, w, h, K.C.rule, 11)
    local inset = math.max(1, math.floor(h / 22))
    local box = ctx.construct("/Script/UMG.EditableTextBox", ctx.state.wt, uname("e"))
    if not box then return nil end
    pcall(function() box:SetHintText(FText(K.ascii(hint))) end)
    if initial and initial ~= "" then pcall(function() box:SetText(FText(K.ascii(initial))) end) end
    pcall(function() box.WidgetStyle.Font.Size = fs end)                 -- UE4-style field
    pcall(function() box.WidgetStyle.TextStyle.Font.Size = fs end)       -- UE5 field
    -- Colours stay the engine's (light field, dark text): a half-applied
    -- retint (dark field + dark text) would be unreadable, and it cannot be
    -- verified offline. The kit frame around the box carries the theme.
    pcall(function() box.ClearKeyboardFocusOnCommit = true end)
    local slot = place(box, x + inset, y + inset, w - 2 * inset, h - 2 * inset, 12)
    track(box, nil, true)
    local ref = { box = box, slot = slot, rect = { x, y, w, h }, kind = "input", shown = true, frame = frame,
                  on_submit = opts.on_submit }
    if K.cur then table.insert(K.cur.inputs, ref) end
    focus_add(ref, opts)
    return ref
end

function K.input_text(i)
    if not i or not i.box or not K.alive() then return nil end
    local s
    pcall(function() s = i.box:GetText():ToString() end)
    return s
end

function K.input_set(i, s)
    if not i or not i.box or not K.alive() then return end
    W()
    pcall(function() i.box:SetText(FText(K.ascii(s or ""))) end)
end

-- Frame colour of an input: validation feedback (good / bad / neutral).
function K.input_state(i, color)
    if not i or not i.frame then return end
    K.set_rect_color(i.frame, color or K.C.rule)
end

-- --- composites ---------------------------------------------------------------------------

-- Equal-width chips in a row. items = { { key, label, help? }, ... }.
-- Returns group { chips = { ref }, keys = { key }, by_key = { key -> ref } }.
-- opts: { gap, max_w, fs, style, group, help }
function K.chip_group(items, x, y, w, h, on_pick, opts)
    opts = opts or {}
    local gap = opts.gap or 8
    local n = math.max(1, #items)
    local cw = math.floor((w - (n - 1) * gap) / n)
    if opts.max_w and cw > opts.max_w then cw = opts.max_w end
    local g = { chips = {}, keys = {}, by_key = {}, rect = { x, y, w, h } }
    for i, it in ipairs(items) do
        local key = it[1]
        local ref = K.button(it[2], function() on_pick(key) end,
            x + (i - 1) * (cw + gap), y, cw, h,
            { fs = opts.fs or 15, style = opts.style or "normal", group = opts.group,
              help = it[3] or opts.help })
        g.chips[i] = ref; g.keys[i] = key; g.by_key[key] = ref
    end
    return g
end

-- Mark `active` (one key, or a set {key=true}) as on; `disabled` (bool or set).
function K.chip_group_set(g, active, disabled)
    if not g then return end
    for i, ref in ipairs(g.chips) do
        local key = g.keys[i]
        local on = (type(active) == "table") and (active[key] and true or false) or (active == key)
        local dis = (type(disabled) == "table") and (disabled[key] and true or false) or (disabled and true or false)
        K.set_state(ref, on, dis)
    end
end

-- Mark one key of a chip group as pending (nil clears every chip).
function K.chip_group_pending(g, key)
    if not g then return end
    for i, ref in ipairs(g.chips) do K.set_pending(ref, key ~= nil and g.keys[i] == key) end
end

-- One disabled reason for every chip of a group.
function K.chip_group_reason(g, reason)
    if not g then return end
    for _, ref in ipairs(g.chips) do ref.reason = reason end
end

-- Rectangles of a cols x rows grid inside (x, y, w, h).
function K.grid_rects(x, y, w, h, cols, rows, gap)
    gap = gap or 8
    local cw = math.floor((w - (cols - 1) * gap) / cols)
    local rh = math.floor((h - (rows - 1) * gap) / rows)
    local out = {}
    for r = 1, rows do
        for c = 1, cols do
            out[#out + 1] = { x + (c - 1) * (cw + gap), y + (r - 1) * (rh + gap), cw, rh }
        end
    end
    return out
end

-- Slice `list` into pages of `per`; clamps `page`. Returns slice, pages, page.
function K.paginate(list, page, per)
    local pages = math.max(1, math.ceil(#list / per))
    page = K.clamp(page or 1, 1, pages)
    local out = {}
    for i = (page - 1) * per + 1, math.min(#list, page * per) do out[#out + 1] = list[i] end
    return out, pages, page
end

-- A tile: button with a title (up to 2 lines), a meta line and a note line.
-- opts: { scale, fs, z, group, fkey, help, title_lines (default 2) }
function K.tile(cb, x, y, w, h, opts)
    opts = opts or {}
    local s = opts.scale or 1
    local pad = math.floor(8 * s)
    local ref = K.button("", cb, x, y, w, h, { style = "tile", no_label = true, z = opts.z, group = opts.group,
                                              fkey = opts.fkey, help = opts.help })
    if not ref then return nil end
    local fs1 = opts.fs or math.floor(16 * s)
    local fs2 = math.max(K.minfs or K.MIN_PX, math.floor(fs1 * 0.82))
    local m_h = math.ceil(fs2 * K.LINE_H)
    -- title lines: as many as fit above the meta + note lines (small windows: the
    -- minimum font makes the lines taller than the scaled tile)
    local tl = opts.title_lines or 2
    while tl > 1 and pad + math.ceil(fs1 * K.LINE_H * tl) > h - pad - 2 * m_h do tl = tl - 1 end
    local t_h = math.ceil(fs1 * K.LINE_H * tl)
    local z = (opts.z or 11) + 2
    ref.title = K.text("", x + pad, y + pad, w - 2 * pad, t_h, fs1, 0, K.C.text, { wrap = tl > 1 and tl or nil, valign = "top", z = z })
    ref.meta  = K.text("", x + pad, y + h - pad - 2 * m_h, w - 2 * pad, m_h, fs2, 0, K.C.dim, { z = z })
    ref.note  = K.text("", x + pad, y + h - pad - m_h, w - 2 * pad, m_h, fs2, 0, K.C.dim, { z = z })
    ref.texts = { ref.title, ref.meta, ref.note }
    ref.label = ref.title
    return ref
end

-- Fill a tile: { title, meta, note, note_color, on, disabled }
function K.tile_set(ref, d)
    if not ref then return end
    if not d then K.show(ref, false); return end
    K.show(ref, true)
    K.set_text(ref.title, d.title or "")
    K.set_text(ref.meta, d.meta or "")
    K.set_text(ref.note, d.note or "")
    K.set_state(ref, d.on, d.disabled)
    K.set_color(ref.title, d.disabled and K.C.dim or (d.on and K.C.on_text or K.C.text))
    K.set_color(ref.meta, d.on and K.C.on_text or K.C.dim)
    K.set_color(ref.note, d.disabled and (d.note_color or K.C.bad) or (d.on and K.C.on_text or (d.note_color or K.C.dim)))
end

-- PREV  page x / y  NEXT
function K.pager(x, y, w, h, on_prev, on_next, opts)
    opts = opts or {}
    local bw = math.floor(math.min(w * 0.3, opts.bw or 150))
    local fs = opts.fs or 15
    local p = {}
    p.prev = K.button("< PREV", on_prev, x, y, bw, h, { fs = fs, group = opts.group, help = "Previous page (PgUp)" })
    p.label = K.text("", x + bw, y, w - 2 * bw, h, fs, 1, K.C.text)
    p.next = K.button("NEXT >", on_next, x + w - bw, y, bw, h, { fs = fs, group = opts.group, help = "Next page (PgDn)" })
    p.prev.reason = "Already on the first page"
    p.next.reason = "Already on the last page"
    return p
end

function K.pager_set(p, page, pages)
    if not p then return end
    K.set_text(p.label, string.format("PAGE %d / %d", page, pages))
    K.set_state(p.prev, false, page <= 1)
    K.set_state(p.next, false, page >= pages)
end

-- Horizontal bar: track + fill + centred label.
function K.bar(x, y, w, h, fs)
    local b = { x = x, y = y, w = w, h = h }
    b.track = K.rect(x, y, w, h, K.C.track, 11)
    b.fill = K.rect(x, y, math.max(1, math.floor(w * 0.5)), h, K.C.ok, 12)
    b.label = K.text("", x, y, w, h, fs or 16, 1, K.C.text, { z = 14 })
    return b
end

function K.bar_set(b, frac, color, label)
    if not b then return end
    frac = K.clamp(frac or 0, 0, 1)
    local fw = math.max(1, math.floor(b.w * frac))
    if b.fw ~= fw then b.fw = fw; K.resize(b.fill, b.x, b.y, fw, b.h) end
    K.set_rect_color(b.fill, color or K.C.ok)
    K.set_text(b.label, label or "")
end

-- --- spinner (loading state; animated by the hover tick, in place) ----------------------

local SPIN = { "[=   ]", "[ =  ]", "[  = ]", "[   =]", "[  = ]", "[ =  ]" }
K.SPIN = SPIN

-- The current spinner frame (for labels that animate themselves).
function K.spin_frame() return SPIN[(math.floor(K.now() * 8) % #SPIN) + 1] end

-- A text that shows "<frame>  <label>" while active, else the plain label.
function K.spinner(x, y, w, h, fs, just, color)
    local t = K.text("", x, y, w, h, fs, just or 0, color or K.C.ok)
    if not t then return nil end
    t.spin = { active = false, label = "" }
    if K.cur then table.insert(K.cur.spinners, t) end
    return t
end

function K.spinner_set(t, active, label, color)
    if not t or not t.spin then return end
    t.spin.active = active and true or false
    t.spin.label = label or ""
    if color then K.set_color(t, color) end
    K.set_text(t, t.spin.active and (K.spin_frame() .. "  " .. t.spin.label) or t.spin.label)
end

-- --- focus manager ------------------------------------------------------------------------

local function focusable(b, ref)
    if not ref or ref.shown == false or ref.skip_focus or not ref.rect then return false end
    if b.modal and ref.modal ~= b.modal then return false end
    return true
end

local function center(r) return r[1] + r[3] / 2, r[2] + r[4] / 2 end

-- The focus ring: four thin rects around the focused control (built lazily).
local function ring_place(b, ref)
    local L = b.L
    local t = math.max(2, (L and L.u and L.u(3)) or 3)
    if not b.ring then
        b.ring = {}
        for i = 1, 4 do b.ring[i] = K.rect(0, 0, 1, 1, K.C.focus, 40) end
    end
    if not ref then
        for i = 1, 4 do K.show_rect(b.ring[i], false) end
        return
    end
    local x, y, w, h = ref.rect[1], ref.rect[2], ref.rect[3], ref.rect[4]
    K.resize(b.ring[1], x - t, y - t, w + 2 * t, t)
    K.resize(b.ring[2], x - t, y + h, w + 2 * t, t)
    K.resize(b.ring[3], x - t, y, t, h)
    K.resize(b.ring[4], x + w, y, t, h)
    for i = 1, 4 do K.show_rect(b.ring[i], true) end
end

local function help_of(ref)
    if not ref then return "" end
    if ref.disabled then
        local r = K.reason_of(ref)
        if r and r ~= "" then return "UNAVAILABLE: " .. r end
    end
    local h = ref.help
    if type(h) == "function" then local ok, s = pcall(h); h = ok and s or nil end
    if ref.kind == "input" and not h then h = "ENTER to type, ENTER again when done" end
    return h or ""
end

local refresh_hints

-- Move the focus to `ref` (how: "key" | "mouse" | "init").
function K.set_focus(ref, how)
    local b = K.cur
    if not K.alive(b) or not focusable(b, ref) then return false end
    local prev = b.focused
    b.focused = ref
    b.group_last[ref.fgroup or "main"] = ref
    K.focus_mem[b.screen] = ref.fkey
    ring_place(b, ref)
    if refresh_hints then refresh_hints(b) end
    if prev ~= ref and b.on_focus then pcall(b.on_focus, ref, how) end
    return true
end

function K.focused() return K.cur and K.cur.focused or nil end

-- First focusable in reading order (top, then left), optionally in one group.
local function first_in(b, group)
    local best
    for _, r in ipairs(b.focus) do
        if focusable(b, r) and (not group or r.fgroup == group) then
            if not best or r.rect[2] < best.rect[2] - 1
                or (math.abs(r.rect[2] - best.rect[2]) <= 1 and r.rect[1] < best.rect[1]) then
                best = r
            end
        end
    end
    return best
end

-- Focus the remembered control of this screen, else `default`, else the first.
function K.focus_default(default)
    local b = K.cur
    if not K.alive(b) then return end
    local key = K.focus_mem[b.screen]
    if key then
        for _, r in ipairs(b.focus) do
            if r.fkey == key and focusable(b, r) then K.set_focus(r, "init"); return end
        end
    end
    default = default or b.default_focus
    if default and focusable(b, default) then K.set_focus(default, "init"); return end
    local f = first_in(b)
    if f then K.set_focus(f, "init") end
end

-- Spatial navigation: the nearest focusable control in direction dir.
function K.nav(dir)
    local b = K.cur
    if not K.alive(b) then return false end
    local cur = b.focused
    if not focusable(b, cur) then K.focus_default(); return true end
    local r = cur.rect
    local cx, cy = center(r)
    local best, bs
    for _, c in ipairs(b.focus) do
        if c ~= cur and focusable(b, c) then
            local q = c.rect
            local qx, qy = center(q)
            local ok, prim, gap, off
            if dir == "right" then
                ok = qx > cx + 0.5 and q[1] > r[1] + 0.5
                prim = math.max(0, q[1] - (r[1] + r[3]))
                gap = math.max(0, q[2] - (r[2] + r[4]), r[2] - (q[2] + q[4])); off = math.abs(qy - cy)
            elseif dir == "left" then
                ok = qx < cx - 0.5 and q[1] + q[3] < r[1] + r[3] - 0.5
                prim = math.max(0, r[1] - (q[1] + q[3]))
                gap = math.max(0, q[2] - (r[2] + r[4]), r[2] - (q[2] + q[4])); off = math.abs(qy - cy)
            elseif dir == "down" then
                ok = qy > cy + 0.5 and q[2] > r[2] + 0.5
                prim = math.max(0, q[2] - (r[2] + r[4]))
                gap = math.max(0, q[1] - (r[1] + r[3]), r[1] - (q[1] + q[3])); off = math.abs(qx - cx)
            else -- up
                ok = qy < cy - 0.5 and q[2] + q[4] < r[2] + r[4] - 0.5
                prim = math.max(0, r[2] - (q[2] + q[4]))
                gap = math.max(0, q[1] - (r[1] + r[3]), r[1] - (q[1] + q[3])); off = math.abs(qx - cx)
            end
            if ok then
                local score = prim + gap * 5 + off * 0.25
                if not bs or score < bs then best, bs = c, score end
            end
        end
    end
    if best then K.set_focus(best, "key"); return true end
    return false
end

-- The next / previous focusable in form order: section by section (focus
-- group order), and inside a section top to bottom, left to right.
function K.nav_order(delta)
    local b = K.cur
    if not K.alive(b) then return false end
    local gorder = {}
    for i, g in ipairs(b.groups) do gorder[g] = i end
    local list = {}
    for _, r in ipairs(b.focus) do if focusable(b, r) then list[#list + 1] = r end end
    if #list == 0 then return false end
    table.sort(list, function(a, c)
        local ga, gc = gorder[a.fgroup] or 0, gorder[c.fgroup] or 0
        if ga ~= gc then return ga < gc end
        if math.abs(a.rect[2] - c.rect[2]) > 1 then return a.rect[2] < c.rect[2] end
        return a.rect[1] < c.rect[1]
    end)
    local i = 0
    for k, r in ipairs(list) do if r == b.focused then i = k end end
    return K.set_focus(list[((i - 1 + delta) % #list) + 1], "key")
end

-- Tab / Shift+Tab: the next / previous group (its last focused control, else its first).
function K.cycle_group(delta)
    local b = K.cur
    if not K.alive(b) or #b.groups == 0 then return false end
    local gi = 1
    local cur = b.focused
    for i, g in ipairs(b.groups) do if cur and g == cur.fgroup then gi = i end end
    for step = 1, #b.groups do
        local g = b.groups[((gi - 1 + delta * step) % #b.groups) + 1]
        local target = b.group_last[g]
        if not focusable(b, target) or target.fgroup ~= g then target = first_in(b, g) end
        if target and target ~= cur then K.set_focus(target, "key"); return true end
    end
    return false
end

-- Activate the focused control exactly like a click (inputs start typing).
function K.activate(ref)
    local b = K.cur
    if not K.alive(b) then return false end
    ref = ref or b.focused
    if not focusable(b, ref) then K.focus_default(); return true end
    if ref.kind == "input" then K.begin_typing(ref); return true end
    ref.flash_until = K.now() + 0.15
    b.flashing = b.flashing or {}
    b.flashing[ref] = true
    paint(ref)
    if ref.entry and ref.entry.cb then
        local ok, err = pcall(ref.entry.cb)
        if not ok then Log("activate %s failed: %s", tostring(ref.fkey), tostring(err)) end
    end
    return true
end

-- --- text inputs: typing mode + the text guard ----------------------------------------------

local TYPING_GRACE_S = 0.30

local function blur()
    local ok = pcall(function()
        local wbl = StaticFindObject("/Script/UMG.Default__WidgetBlueprintLibrary")
        if wbl and wbl:IsValid() then wbl:SetFocusToGameViewport() end
    end)
    return ok
end
K._blur = blur

function K.begin_typing(ref)
    local b = K.cur
    if not K.alive(b) or not ref or not ref.box then return end
    K.set_focus(ref, "key")
    pcall(function() ref.box:SetKeyboardFocus() end)
    b.typing, b.typing_until = ref, K.now() + TYPING_GRACE_S
    if refresh_hints then refresh_hints(b) end
end

-- Leave typing mode; submit = run the input's on_submit with its text.
function K.end_typing(submit)
    local b = K.cur
    if not b then return end
    local ref = b.typing
    b.typing, b.typing_until = nil, nil
    blur()
    if refresh_hints and K.alive(b) then refresh_hints(b) end
    if submit and ref and ref.on_submit and K.alive(b) then
        local ok, err = pcall(ref.on_submit, K.input_text(ref) or "")
        if not ok then Log("input submit failed: %s", tostring(err)) end
    end
end

-- The input being typed into (focus seen now or within the grace window), or nil.
function K.typing()
    local b = K.cur
    if not K.alive(b) then return nil end
    for _, r in ipairs(b.inputs) do
        local f = false
        if r.shown ~= false then pcall(function() f = r.box:HasKeyboardFocus() end) end
        if f then b.typing, b.typing_until = r, K.now() + TYPING_GRACE_S; return r end
    end
    if b.typing and b.typing_until and K.now() <= b.typing_until then return b.typing end
    return nil
end

-- --- input router ------------------------------------------------------------------------------

-- Per-build override: fn() -> true when it handled the action.
function K.on_key(action, fn) if K.cur then K.cur.keys[action] = fn end end

local NAV = { up = true, down = true, left = true, right = true }

-- action: up | down | left | right | accept | back | next_group | prev_group |
--         page_prev | page_next | tab_prev | tab_next | refresh
-- device: "kb" | "pad". Returns true when the menu consumed it.
function K.key(action, device)
    local b = K.cur
    if not K.alive(b) then return false end
    if device and device ~= K.device then K.device = device; if refresh_hints then refresh_hints(b) end end
    local typing = K.typing()
    if typing then
        if action == "accept" then K.end_typing(true)
        elseif action == "back" then K.end_typing(false)
        elseif action == "next_group" or action == "prev_group" then
            K.end_typing(true)
            if K.alive(b) then K.nav_order(action == "next_group" and 1 or -1) end
        end
        return true                       -- everything else is typing (the text box has it)
    end
    local h = b.keys[action]
    if h then
        local ok, done = pcall(h)
        if not ok then Log("key %s handler failed: %s", action, tostring(done)) end
        if ok and done then return true end
    end
    if not K.alive(b) then return true end
    if NAV[action] then
        if not b.focused then K.focus_default(); return true end
        K.nav(action); return true
    elseif action == "accept" then return K.activate()
    elseif action == "back" then
        if b.modal and b.modal.cancel then b.modal.cancel(); return true end
        if b.on_back then b.on_back(); return true end
    elseif action == "next_group" then return K.cycle_group(1)
    elseif action == "prev_group" then return K.cycle_group(-1)
    elseif action == "page_prev" or action == "page_next" then
        if b.on_page then b.on_page(action == "page_next" and 1 or -1); return true end
    elseif action == "tab_prev" or action == "tab_next" then
        if b.on_tab then b.on_tab(action == "tab_next" and 1 or -1); return true end
    elseif action == "refresh" then
        if b.on_refresh then b.on_refresh(); return true end
    end
    return false
end

-- --- hint bar (keys + context help) -------------------------------------------------------------

local HINT_BASE = {
    kb  = { "[ARROWS] MOVE", "[ENTER] SELECT", "[ESC] BACK", "[TAB] NEXT SECTION" },
    pad = { "[D-PAD] MOVE", "[A] SELECT", "[B] BACK" },
}
local HINT_TYPING = { kb = "[ENTER] DONE   [ESC] CANCEL   [TAB] NEXT FIELD", pad = "[A] DONE   [B] CANCEL" }
K.HINT_EXTRA = {
    page = { kb = "[PGUP/PGDN] PAGE", pad = "[LT/RT] PAGE" },
    tabs = { kb = "[Q/E] TAB", pad = "[LB/RB] TAB" },
    refresh = { kb = "[F5] REFRESH", pad = "[Y] REFRESH" },
}

-- cap (optional): the characters that fit the hint slot. A small window drops
-- the wide separators first, then the least important keys from the end
-- (screen extras before the basic keys), never clipping a key in half.
function K.hint_text(b, cap)
    b = b or K.cur
    local dev = (K.device == "pad") and "pad" or "kb"
    if b and b.typing then
        local t = HINT_TYPING[dev]
        if cap and #t > cap then t = t:gsub("   ", "  ") end
        return t
    end
    local parts = {}
    for _, s in ipairs(HINT_BASE[dev]) do parts[#parts + 1] = s end
    for _, e in ipairs((b and b.hint_extra) or {}) do
        local x = K.HINT_EXTRA[e] or e
        parts[#parts + 1] = (type(x) == "table") and x[dev] or tostring(x)
    end
    local s = table.concat(parts, "   ")
    if not cap or #s <= cap then return s end
    while #parts > 1 do
        s = table.concat(parts, "  ")
        if #s <= cap then return s end
        table.remove(parts)
    end
    return parts[1] or ""
end

function refresh_hints(b)
    local L = b and b.L
    if not L or not L.hint_keys or not K.alive(b) then return end
    local hk = L.hint_keys
    local cap = math.max(1, math.floor((hk.w or 0) / ((hk.fs or 16) * K.CHAR_W)))
    K.set_text(hk, K.hint_text(b, cap))
    local fl = b.flash
    if fl and K.now() < fl.until_t then
        K.set_text(L.hint_help, fl.text); K.set_color(L.hint_help, fl.color or K.C.warn)
    else
        local f = b.focused
        K.set_text(L.hint_help, help_of(f))
        K.set_color(L.hint_help, (f and f.disabled) and K.C.warn or K.C.dim)
    end
end
K.refresh_hints = refresh_hints

-- A short message in the hint bar's help slot (disabled reasons, results).
function K.flash(text, color, secs)
    local b = K.cur
    if not b then return end
    b.flash = { text = tostring(text or ""), color = color or K.C.warn, until_t = K.now() + (secs or 4) }
    refresh_hints(b)
end

-- --- screen frame: panel, title, status, message line, actions, hint bar ------------------------

-- opts: { w, h (design units), hint_extra = { "page", "tabs", "refresh" } }
-- Returns L: { s, u(), F(), cw, ch, dpi, px, py, pw, ph, pad, gap, x0, iw, top, bottom,
--              action_y, action_h, msg_y, msg_h, hint_y, hint_h, title, status, msg }
function K.frame(screen, title, opts)
    opts = opts or {}
    local b = K.begin(screen)
    local M = K.metrics()
    local u, F = M.u, M.F
    local L = { s = M.s, u = u, F = F, lh = M.lh, cw = M.cw, ch = M.ch, dpi = M.dpi, vw = M.vw, vh = M.vh, src = M.src }
    L.pw, L.ph = M.panel(opts.w or K.PANEL.w, opts.h or K.PANEL.h)
    L.px, L.py = -math.floor(L.pw / 2), -math.floor(L.ph / 2)
    L.pad = u(K.PANEL.pad); L.gap = u(K.SP.sm)
    L.x0 = L.px + L.pad; L.iw = L.pw - 2 * L.pad
    b.L = L
    b.hint_extra = opts.hint_extra or {}
    K.rect(L.px, L.py, L.pw, L.ph, K.C.panel, 10)
    local th = u(56)
    local ty = L.py + L.pad
    L.title = K.text(title, L.x0, ty, math.floor(L.iw * 0.58), th, F(K.TS.title), 0, K.C.head)
    L.status = K.spinner(L.x0 + math.floor(L.iw * 0.58), ty, L.iw - math.floor(L.iw * 0.58), th, F(K.TS.h2), 2, K.C.dim)
    local ry = ty + th
    K.rect(L.x0, ry, L.iw, math.max(1, u(2)), K.C.rule, 11)
    L.top = ry + u(K.SP.lg)
    -- bottom: hint bar, actions above it, message line above them
    L.hint_h = u(K.BH.hint)
    L.hint_y = L.py + L.ph - L.pad - L.hint_h
    L.action_h = u(K.BH.action)
    L.action_y = L.hint_y - u(K.SP.md) - L.action_h
    L.msg_h = u(30)
    L.msg_y = L.action_y - u(K.SP.sm) - L.msg_h
    L.bottom = L.msg_y - u(K.SP.sm)
    K.rect(L.x0, L.hint_y, L.iw, L.hint_h, K.C.section, 11)
    local hp = u(K.SP.md)
    local hfs = F(K.TS.small)
    L.hint_keys = K.text("", L.x0 + hp, L.hint_y, math.floor(L.iw * 0.56) - hp, L.hint_h, hfs, 0, K.C.dim)
    L.hint_help = K.text("", L.x0 + math.floor(L.iw * 0.56), L.hint_y, L.iw - math.floor(L.iw * 0.56) - hp,
        L.hint_h, hfs, 2, K.C.dim)
    L.msg = K.text("", L.x0, L.msg_y, L.iw, L.msg_h, F(K.TS.label), 0, K.C.dim)
    return L, b
end

-- The status line (top right): text + colour, with a spinner while `busy`.
function K.status(text, color, busy)
    local b = K.cur
    if not b or not b.L then return end
    K.spinner_set(b.L.status, busy, text, color)
end

-- The message line above the actions.
function K.msg(text, color)
    local b = K.cur
    if not b or not b.L then return end
    K.set_text(b.L.msg, text or "")
    K.set_color(b.L.msg, color or K.C.dim)
end

-- Action bar. left = { {label, cb, style, help, key, reason}, ... } from the left edge;
-- right = { ... } from the right edge (right[1] is the right-most: BACK / LEAVE).
-- Every action button has the same size. Returns { key -> ref }.
function K.actions(L, left, right)
    local u = L.u
    left, right = left or {}, right or {}
    local n = #left + #right
    local sp = u(K.SP.lg)
    local bw = math.min(u(K.BW.action), math.floor((L.iw - math.max(0, n - 1) * sp - u(K.SP.xxl)) / math.max(1, n)))
    local out = {}
    K.group("actions")
    local function mk(a, x)
        local ref = K.button(a.label, a.cb, x, L.action_y, bw, L.action_h,
            { fs = L.F(K.TS.h2), style = a.style, help = a.help, fkey = a.key and (K.cur.screen .. ":" .. a.key) or nil })
        if ref then ref.reason = a.reason end
        out[a.key or a.label] = ref
        return ref
    end
    for i, a in ipairs(left) do mk(a, L.x0 + (i - 1) * (bw + sp)) end
    for i, a in ipairs(right) do mk(a, L.x0 + L.iw - i * bw - (i - 1) * sp) end
    L.action_w = bw
    return out
end

-- --- inline confirm strip (on the message line; destructive actions) ---------------------------

-- Opens "<text>  [YES] [CANCEL]" on the frame's message line; keyboard focus is
-- held by the strip (CANCEL first) until it closes. Auto-cancels after opts.secs.
-- opts: { no_label = "CANCEL", danger = true, secs = 10, on_cancel }
function K.confirm(text, yes_label, on_yes, opts)
    local b = K.cur
    if not K.alive(b) or not b.L then return end
    opts = opts or {}
    local L = b.L
    if not b.cstrip then
        local u = L.u
        local bw = u(200)
        local sp = u(K.SP.md)
        local h = L.msg_h + u(K.SP.sm)
        local y = L.msg_y - u(K.SP.xs)
        local cs = { modal_tag = {} }
        cs.bg = K.rect(L.x0, y, L.iw, h, K.C.section, 11)
        cs.text = K.text("", L.x0 + u(K.SP.md), y, L.iw - 2 * bw - 2 * sp - u(K.SP.md), h, L.F(K.TS.label), 0, K.C.ok)
        K.group("confirm")
        cs.yes = K.button("", function() if cs.on_yes then local f = cs.on_yes; K.confirm_close(); f() end end,
            L.x0 + L.iw - 2 * bw - sp, y, bw, h, { fs = L.F(K.TS.label), style = "danger", fkey = b.screen .. ":confirm_yes" })
        cs.no = K.button("", function() K.confirm_close(true) end,
            L.x0 + L.iw - bw, y, bw, h, { fs = L.F(K.TS.label), fkey = b.screen .. ":confirm_no" })
        cs.yes.modal, cs.no.modal = cs.modal_tag, cs.modal_tag
        cs.modal_tag.cancel = function() K.confirm_close(true) end
        b.cstrip = cs
        K.show_rect(cs.bg, false); K.show_text(cs.text, false); K.show(cs.yes, false); K.show(cs.no, false)
    end
    local cs = b.cstrip
    cs.on_yes, cs.on_cancel = on_yes, opts.on_cancel
    cs.until_t = K.now() + (opts.secs or 10)
    cs.prev_focus = b.focused
    K.set_text(cs.text, text)
    K.set_label(cs.yes, yes_label or "CONFIRM")
    K.set_style(cs.yes, opts.danger == false and "primary" or "danger")
    K.set_label(cs.no, opts.no_label or "CANCEL")
    K.show_text(L.msg, false)
    K.show_rect(cs.bg, true); K.show_text(cs.text, true); K.show(cs.yes, true); K.show(cs.no, true)
    b.modal = cs.modal_tag
    K.set_focus(cs.no, "key")
end

function K.confirm_open() local b = K.cur; return b ~= nil and b.cstrip ~= nil and b.modal == b.cstrip.modal_tag end

function K.confirm_close(cancelled)
    local b = K.cur
    if not b or not b.cstrip or b.modal ~= b.cstrip.modal_tag then return end
    local cs = b.cstrip
    b.modal = nil
    local cb = cancelled and cs.on_cancel or nil
    cs.on_yes, cs.on_cancel = nil, nil
    if K.alive(b) then
        K.show(cs.yes, false); K.show(cs.no, false); K.show_text(cs.text, false); K.show_rect(cs.bg, false)
        K.show_text(b.L.msg, true)
        if focusable(b, cs.prev_focus) then K.set_focus(cs.prev_focus, "key") else K.focus_default() end
    end
    if cb then pcall(cb) end
end

-- --- hover + per-screen tick ---------------------------------------------------------------

-- Register `fn` to run every `every` hover ticks (50 ms each) while this build
-- is alive.
function K.on_tick(every, fn)
    if K.cur then table.insert(K.cur.ticks, { every = every, fn = fn }) end
end

local tick_n = 0
local function hover_tick()
    local b = K.cur
    if not K.alive(b) then return end
    tick_n = tick_n + 1
    local now = K.now()
    for _, ref in ipairs(b.hover) do
        if ref.shown ~= false and ref.button then
            local h = false
            pcall(function() h = ref.button:IsHovered() end)
            h = h and true or false
            if h ~= ref.hovered then
                ref.hovered = h; paint(ref)
                if h and b.focused ~= ref then K.set_focus(ref, "mouse") end
            end
        end
        if b ~= K.cur then return end
    end
    if b.flashing then
        for ref in pairs(b.flashing) do
            if not ref.flash_until or now >= ref.flash_until then b.flashing[ref] = nil; ref.flash_until = nil; paint(ref) end
        end
    end
    -- inputs: a mouse click into a box starts typing (ring + hints follow)
    if #b.inputs > 0 then
        local was = b.typing
        local t = K.typing()
        if t and t ~= b.focused then K.set_focus(t, "mouse") end
        if (t ~= nil) ~= (was ~= nil) then refresh_hints(b) end
    end
    -- the focused control was hidden: move to a visible one
    if b.focused and not focusable(b, b.focused) then
        local f = b.focused
        if not K.nav("down") and not K.nav("up") then K.focus_default() end
        if b.focused == f then b.focused = nil; ring_place(b, nil) end
    end
    -- spinners (in place, ~8 fps)
    if tick_n % 2 == 0 then
        for _, t in ipairs(b.spinners) do
            if t.spin.active then K.set_text(t, K.spin_frame() .. "  " .. t.spin.label) end
        end
    end
    -- flash expiry, confirm strip timeout
    if b.flash and now >= b.flash.until_t then b.flash = nil; refresh_hints(b) end
    if b.cstrip and b.modal == b.cstrip.modal_tag and now >= (b.cstrip.until_t or now) then K.confirm_close(true) end
    for _, t in ipairs(b.ticks) do
        if tick_n % t.every == 0 and K.alive(b) then
            local ok, err = pcall(t.fn)
            if not ok then Log("ui tick error (%s): %s", b.screen, tostring(err)) end
        end
    end
end

-- ctx: { state, construct, clone_text_look, world_gen = function() -> n, Log }
function K.init(c)
    ctx = c
    Log = c.Log or Log
    UIS = c.uis
    if not UIS then
        local ok, m = pcall(require, "hsmp_ui_scale")
        if ok and type(m) == "table" then UIS = m end
    end
    K.uis = UIS
    -- LoopAsync is remapped to LoopInGameThreadWithDelay by main.lua's shim.
    LoopAsync(50, function()
        local t0 = os.clock()
        local ok, err = pcall(hover_tick)
        if not ok then Log("ui hover tick error: %s", tostring(err)) end
        if K.cur then
            K.perf_add("tick", (os.clock() - t0) * 1000)
            perf_report(os.clock())
        else
            K.perf.t0 = nil
        end
        return false
    end)
end

K._hover_tick = hover_tick   -- offline tests
K._P = P

return K
