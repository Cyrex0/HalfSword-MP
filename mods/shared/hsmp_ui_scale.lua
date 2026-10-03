-- hsmp_ui_scale.lua -- the single UI scaling rule for every HSMP widget (HSMPMenu's
-- ui_kit screens + top ribbons, HSMPHud's HUD and panels).
--
-- build-and-deploy.ps1 copies shared/*.lua into every mod's Scripts/; do not
-- edit the per-mod copies. Everything but U.probe / U.measure is pure Lua
-- (no UObject), so the offline tests compute every rect with the same code.
--
-- Units
--   physical px  the game window (GetViewportSize).
--   canvas units Slate units of the canvas we place widgets on: physical px
--                divided by the viewport's DPI scale (GetViewportScale, the
--                game's own DPI curve times the OS scale when UE applies it).
--   design units a 1920x1080 reference canvas. Every layout is written in
--                design units and multiplied by one scale s.
--
-- The rule (UE's "ShortSide" DPI curve, applied to our design space):
--   s = short side / 1080, then fitted so the 1920-wide design never exceeds
--   the canvas width (4:3, 5:4, portrait). On 16:9 and wider s = ch / 1080, so
--   an ultrawide (3440x1440, 5120x1440) gets exactly the 2560x1440 size,
--   centred, never stretched. Fonts use the same s and never drop under
--   U.MIN_FONT_PX physical pixels.
--
-- Why the viewport probe asks WidgetLayoutLibrary (BlueprintCallable statics)
-- first: GameInstance:GetGameViewportClient():GetViewportSize() is C++-only
-- (not a UFUNCTION), so through UE4SS the pcall fails silently. Assuming
-- 1920x1080 physical while GetViewportScale is real gives a 1280x720 window a
-- 2883x1622 "canvas": menus 1.5x too large and off screen.
--
-- Thread / world rules: U.probe is called on the game
-- thread only, looks everything up fresh (nothing is cached across worlds) and
-- reads no SoftObject property.

local U = { VERSION = 1 }

U.REF_W, U.REF_H = 1920, 1080      -- design canvas
U.MIN_FONT_PX = 9                  -- smallest font, physical px
U.SAFE_DESIGN = 24                 -- safe margin at the reference size (design units)
U.SAFE_FRAC = 0.02                 -- ... and at least 2% of the short side
U.CHAR_W = 0.60                    -- average glyph advance / font size (text fit estimate)
U.LINE_H = 1.40                    -- line height / font size
U.S_MIN, U.S_MAX = 0.1, 4.0        -- sanity bounds of s

local floor, ceil, min, max = math.floor, math.ceil, math.min, math.max

local function clamp(v, a, b) if v < a then return a elseif v > b then return b end return v end
U.clamp = clamp
local function num(v) return type(v) == "number" and v == v and v > 0 and v < 1e6 end

-- UE's default DPI curve (ShortSide: 1080 -> 1.0). Used only when the game's
-- own DPI scale cannot be read.
function U.default_dpi(vw, vh)
    return max(1, min(vw, vh)) / U.REF_H
end

-- Canvas size (canvas units) from the viewport (physical px) and DPI scale.
-- Unknown / nonsense inputs fall back to the reference and the default curve.
-- Returns cw, ch, dpi.
function U.canvas(vw, vh, dpi)
    if not (num(vw) and num(vh)) then vw, vh = U.REF_W, U.REF_H end
    if not num(dpi) or dpi < 0.05 or dpi > 20 then dpi = U.default_dpi(vw, vh) end
    return vw / dpi, vh / dpi, dpi
end

-- The one scale factor (design units -> canvas units). lo / hi: optional bounds.
function U.scale(cw, ch, lo, hi)
    local s = min(cw, ch) / U.REF_H            -- short side, like UE's DPI curve
    local fit = cw / U.REF_W                   -- a 16:9 design must fit the width too
    if fit < s then s = fit end
    return clamp(s, lo or U.S_MIN, hi or U.S_MAX)
end

-- The smallest window the minimum font is sized for. Above it, text is never
-- under U.MIN_FONT_PX physical px; below it (a tiny window), the minimum
-- shrinks with the window like the rest of the UI, so a layout that fits the
-- floor window still fits (smaller, never overlapping or clipped).
U.FLOOR_W, U.FLOOR_H = 1024, 576
U.FLOOR_S = U.FLOOR_W / U.REF_W                -- physical scale of the floor window

-- Minimum font, physical px, for a physical scale ps = s * dpi.
function U.min_font_px(ps)
    if not num(ps) then return U.MIN_FONT_PX end
    return U.MIN_FONT_PX * min(1, ps / U.FLOOR_S)
end

-- Smallest font in canvas units for a DPI scale (and the layout scale s).
-- (Rounded up while the window is at least the floor, so text is never under
-- MIN_FONT_PX; rounded to nearest below it, so it shrinks in proportion.)
function U.min_font(dpi, s)
    dpi = max(0.05, dpi or 1)
    if s and s * dpi < U.FLOOR_S - 1e-9 then
        return max(1, floor(U.min_font_px(s * dpi) / dpi + 0.5))
    end
    return max(1, ceil(U.MIN_FONT_PX / dpi - 1e-9))
end

-- A design font size in canvas units (never under the minimum).
function U.font(n, s, dpi)
    return max(floor(n * s), U.min_font(dpi, s))
end

-- A design length in canvas units (rounded).
function U.len(n, s) return floor(n * s + 0.5) end

-- Line box height for a font (canvas units).
function U.line_h(fs, lines) return ceil(fs * U.LINE_H * (lines or 1)) end

-- Safe margin (canvas units): nothing of ours is placed closer to an edge.
function U.margin(cw, ch, s)
    return max(floor(U.SAFE_DESIGN * s + 0.5), ceil(min(cw, ch) * U.SAFE_FRAC))
end

-- The widest a centred panel may be: the safe width, and never more than a
-- 16:9 area of the canvas height (no stretching across an ultrawide).
function U.max_width(cw, ch, s)
    local m = U.margin(cw, ch, s)
    return floor(min(cw - 2 * m, ch * U.REF_W / U.REF_H))
end

function U.max_height(cw, ch, s)
    return floor(ch - 2 * U.margin(cw, ch, s))
end

-- A centred panel of design size (dw, dh): scaled, then clamped to the safe
-- area and the max width. Returns pw, ph (canvas units).
function U.panel(dw, dh, s, cw, ch)
    return min(U.len(dw, s), U.max_width(cw, ch, s)), min(U.len(dh, s), U.max_height(cw, ch, s))
end

-- Everything one layout needs: { vw, vh, cw, ch, dpi, s, m, minfs, src,
-- u(n), F(n) }. lo / hi: optional bounds of s.
function U.metrics(vw, vh, dpi, lo, hi)
    local cw, ch, d = U.canvas(vw, vh, dpi)
    local s = U.scale(cw, ch, lo, hi)
    local M = { vw = vw, vh = vh, cw = cw, ch = ch, dpi = d, s = s, m = U.margin(cw, ch, s), minfs = U.min_font(d, s) }
    M.u = function(n) return floor(n * s + 0.5) end
    M.F = function(n) return max(floor(n * s), M.minfs) end
    return M
end

-- --- geometry helpers (tests + layouts) ---------------------------------------------------

-- A rect given relative to an anchor point (ax, ay in 0..1) of a cw x ch
-- canvas -> absolute { x, y, w, h } (origin top-left).
function U.abs(anchor, r, cw, ch)
    return { x = anchor[1] * cw + r.x, y = anchor[2] * ch + r.y, w = r.w, h = r.h }
end

function U.overlap(a, b, tol)
    tol = tol or 0.5
    return a.x < b.x + b.w - tol and b.x < a.x + a.w - tol and a.y < b.y + b.h - tol and b.y < a.y + a.h - tol
end

function U.inside(a, b, tol)
    tol = tol or 0.5
    return a.x >= b.x - tol and a.y >= b.y - tol and a.x + a.w <= b.x + b.w + tol and a.y + a.h <= b.y + b.h + tol
end

-- Bounding box of a list of rects.
function U.bbox(list)
    local x0, y0, x1, y1 = math.huge, math.huge, -math.huge, -math.huge
    for _, r in ipairs(list) do
        x0 = min(x0, r.x); y0 = min(y0, r.y); x1 = max(x1, r.x + r.w); y1 = max(y1, r.y + r.h)
    end
    if x0 == math.huge then return nil end
    return { x = x0, y = y0, w = x1 - x0, h = y1 - y0 }
end

-- Does `s` fit a w x h box at font fs (estimate: CHAR_W / LINE_H)?
function U.text_fits(s, w, h, fs, lines)
    lines = lines or 1
    return #(s or "") * fs * U.CHAR_W <= w * lines + 1 and fs * U.LINE_H * lines <= h + 1
end

-- --- resize watcher -----------------------------------------------------------------------

-- Feed it the measured canvas on every poll; it answers true ONCE when a new
-- size has held for `settle` polls (a window drag reports many sizes: rebuild
-- once, at the end). The first sample only seeds it.
function U.watcher(settle)
    return { settle = settle or 2, cur = nil, cand = nil, n = 0 }
end

function U.key(cw, ch, dpi)
    return string.format("%dx%d@%.3f", floor(cw + 0.5), floor(ch + 0.5), dpi or 1)
end

function U.watch(w, cw, ch, dpi)
    local k = U.key(cw, ch, dpi)
    if w.cur == nil then w.cur = k; return false end
    if k == w.cur then w.cand, w.n = nil, 0; return false end
    if k ~= w.cand then w.cand, w.n = k, 1 else w.n = w.n + 1 end
    if w.n >= w.settle then w.cur, w.cand, w.n = k, nil, 0; return true end
    return false
end

-- Forget the baseline (a new world / a new host): the next sample seeds it.
function U.watch_reset(w) w.cur, w.cand, w.n = nil, nil, 0 end

-- --- the engine probe (game thread only; fresh lookups; nothing cached) -----------------------

local function vec2(v)
    if v == nil then return nil end
    local x, y
    pcall(function() x, y = v.X, v.Y end)
    if num(x) and num(y) then return x, y end
    return nil
end

-- Physical viewport size + DPI scale. ctx_obj: a world context (a widget of
-- this world, or the world). UEH: UE4SS's UEHelpers. Returns vw, vh, dpi, src
-- (vw / vh / dpi are nil when unknown; src names the size source).
function U.probe(UEH, ctx_obj)
    local vw, vh, dpi, src
    local wll
    pcall(function()
        wll = StaticFindObject("/Script/UMG.Default__WidgetLayoutLibrary")
        if wll and not wll:IsValid() then wll = nil end
    end)
    if wll and ctx_obj ~= nil then
        -- UWidgetLayoutLibrary::GetViewportSize(WorldContextObject) -> FVector2D (physical px)
        pcall(function()
            local x, y = vec2(wll:GetViewportSize(ctx_obj))
            if x then vw, vh, src = x, y, "wll" end
        end)
        pcall(function()
            local d = wll:GetViewportScale(ctx_obj)
            if num(d) then dpi = d end
        end)
    end
    if not vw and UEH then
        -- legacy path (C++-only functions in UE 5.4: kept in case a UE4SS build exposes them)
        pcall(function()
            local gi = UEH.GetGameInstance()
            local vp = gi and gi:GetGameViewportClient() or nil
            if vp and vp:IsValid() then
                local out = { X = 0, Y = 0 }
                vp:GetViewportSize(out)
                if num(out.X) and num(out.Y) then vw, vh, src = out.X, out.Y, "gvc" end
            end
        end)
    end
    if not vw then
        -- the applied resolution from GameUserSettings (BlueprintCallable)
        pcall(function()
            local cdo = StaticFindObject("/Script/Engine.Default__GameUserSettings")
            local gus = cdo and cdo:IsValid() and cdo:GetGameUserSettings() or nil
            if gus and gus:IsValid() then
                local x, y = vec2(gus:GetScreenResolution())
                if x then vw, vh, src = x, y, "gus" end
            end
        end)
    end
    return vw, vh, dpi, src
end

-- U.metrics from the engine. With no size but a DPI scale, the default curve
-- gives the short side (16:9 assumed); with neither, the reference size.
function U.measure(UEH, ctx_obj, lo, hi)
    local vw, vh, dpi, src = U.probe(UEH, ctx_obj)
    if not vw then
        if dpi then
            vh = dpi * U.REF_H; vw = vh * U.REF_W / U.REF_H; src = "dpi-curve"
        else
            vw, vh, src = U.REF_W, U.REF_H, "default"
        end
    end
    local M = U.metrics(vw, vh, dpi, lo, hi)
    M.src = src .. (dpi and "" or "+curve")
    return M
end

return U
