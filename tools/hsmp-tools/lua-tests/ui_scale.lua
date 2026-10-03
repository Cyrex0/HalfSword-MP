-- UI-SCALE: the one UI scaling rule (shared/hsmp_ui_scale.lua) and every
-- HSMPHud rect (HUD, scoreboard, net indicator, kill feed / toasts, the
-- reconnect / connection modal, the MP pause and the match result buttons)
-- at every tested resolution x DPI scale (lib/ui_res.lua: 720p .. 4K,
-- 21:9 / 32:9 ultrawide incl. 5120x1440, 16:10, 4:3, 5:4, small windows;
-- DPI 1 .. 2 and the default curve). The menu screens are covered at the same
-- matrix by menu_ui.lua (it boots the real HSMPMenu).
--
--     hsmp-tools lua-test ui_scale
--
-- Asserted for every widget: inside the canvas' safe margin (nothing off
-- screen), the HUD's regions / the panel's parts never overlap (siblings),
-- every font >= 9 physical px (proportionally less only under the floor
-- window, 1024x576), text never taller / wider than its box, and the centred
-- boxes (banner, scoreboard, modal) never wider than 16:9 of the height.

local T = T
local UIS = dofile(T.path("mods/shared/hsmp_ui_scale.lua"))
local RES = dofile(T.path("tools/hsmp-tools/lua-tests/lib/ui_res.lua"))
local HUD = T.path("mods/HSMPHud/Scripts")
package.path = HUD .. "/?.lua;" .. T.path("mods/shared") .. "/?.lua;" .. T.path("tools/hsmp-tools/lua-tests/lib") .. "/?.lua;" .. package.path
local check = T.check

local function near(a, b, tol) return math.abs(a - b) <= (tol or 0.01) end

-- ============================================================================================
-- 1. the rule
-- ============================================================================================
T.log("== hsmp_ui_scale: canvas, scale, fonts, margins, max width")
do
    local cw, ch, d = UIS.canvas(1280, 720, 720 / 1080)
    check(near(cw, 1920) and near(ch, 1080) and near(d, 0.6667, 0.001), "1280x720 on the default curve is a 1920x1080 canvas")
    cw, ch, d = UIS.canvas(5120, 1440, nil)
    check(near(cw, 3840) and near(ch, 1080) and near(d, 1.3333, 0.001), "5120x1440 without a DPI: default curve -> 3840x1080")
    cw, ch = UIS.canvas(nil, nil, nil)
    check(cw == 1920 and ch == 1080, "unknown viewport -> the reference")
    cw, ch, d = UIS.canvas(1920, 1080, 0 / 0)
    check(near(d, 1) and cw == 1920, "a NaN DPI falls back to the curve")
    check(near(UIS.scale(1920, 1080), 1) and near(UIS.scale(3840, 1080), 1) and near(UIS.scale(2560, 1440), 1.3333, 0.001),
        "16:9 and ultrawide scale by the short side")
    check(near(UIS.scale(1440, 1080), 0.75), "4:3 is fitted to the 1920-wide design (0.75)")
    check(near(UIS.scale(1080, 1920), 0.5625), "portrait is fitted to the width")
    check(UIS.min_font(1.0, 1) == 9 and UIS.min_font(2.0, 1) == 5 and UIS.min_font(1.25, 1) == 8,
        "minimum font: 9 physical px (canvas units round up)")
    check(UIS.min_font(1.0, 1024 / 1920) == 9, "the floor window keeps 9 px")
    check(UIS.min_font(1.0, 800 / 1920) == 7, "under the floor the minimum shrinks with the window (800 wide -> 7)")
    check(UIS.font(14, 1, 1) == 14 and UIS.font(14, 0.6, 1) == 9, "font = design * s, never under the minimum")
    check(UIS.margin(1920, 1080, 1) == 24 and UIS.margin(3840, 1080, 1) == 24, "safe margin 24 design units")
    check(UIS.max_width(3840, 1080, 1) == 1920, "an ultrawide panel is capped at 16:9 of the height")
    check(UIS.max_width(1920, 1080, 1) == 1872, "16:9: the safe width")
    local pw, ph = UIS.panel(1760, 1000, 1, 3840, 1080)
    check(pw == 1760 and ph == 1000, "the menu panel at 5120x1440 keeps its design size (centred)")
    pw, ph = UIS.panel(1760, 1000, 1, 1600, 900)
    check(pw <= 1600 - 2 * UIS.margin(1600, 900, 1) and ph <= 900 - 2 * UIS.margin(1600, 900, 1), "a panel is clamped to the safe area")
    local M = UIS.metrics(2560, 1440, 1.0)
    check(M.cw == 2560 and near(M.s, 1.3333, 0.001) and M.u(24) == 32 and M.F(9) == 12, "metrics: u() and F() scale")
end

T.log("== resize watcher: seed, settle, drag, DPI")
do
    local w = UIS.watcher(2)
    check(UIS.watch(w, 1920, 1080, 1) == false, "the first sample only seeds")
    check(UIS.watch(w, 1920, 1080, 1) == false, "same size: no change")
    check(UIS.watch(w, 2560, 1440, 1) == false, "a new size is not reported at once")
    check(UIS.watch(w, 2560, 1440, 1) == true, "... but after it held for 2 polls")
    check(UIS.watch(w, 2560, 1440, 1) == false, "reported once")
    local fired = 0
    for i = 1, 10 do if UIS.watch(w, 2000 + i * 10, 1100, 1) then fired = fired + 1 end end
    check(fired == 0, "a drag (a new size every poll) never fires")
    UIS.watch(w, 1280, 720, 1)
    check(UIS.watch(w, 1280, 720, 1) == true, "it fires once the drag settles")
    UIS.watch(w, 1280, 720, 1.5)
    check(UIS.watch(w, 1280, 720, 1.5) == true, "a DPI change alone is a change")
    UIS.watch_reset(w)
    check(UIS.watch(w, 640, 480, 1) == false, "after a reset the next sample seeds again")
end

T.log("== the engine probe (mocked UE4SS)")
do
    local M = require("umg_mock")
    local UEH
    local function probe_with(o)
        M.install({ vw = o.vw, vh = o.vh, scale = o.scale, gvc_broken = o.gvc_broken, wll_size_broken = o.wll_size_broken })
        package.loaded["UEHelpers"] = nil
        UEH = require("UEHelpers")
        return UIS.measure(UEH, M.world)
    end
    -- the game: GameViewportClient:GetViewportSize is C++-only; the layout library answers
    local G = probe_with({ vw = 1280, vh = 720, scale = 720 / 1080, gvc_broken = true })
    check(G.src == "wll" and near(G.cw, 1920) and near(G.ch, 1080) and near(G.s, 1),
        "in game: the layout library's size + DPI -> 1920x1080 canvas (" .. tostring(G.src) .. ")")
    -- what HSMP did before (GVC failed silently -> 1920x1080 assumed, real DPI): a 2883x1622 canvas
    local old_cw = 1920 / (720 / 1080)
    check(math.floor(old_cw + 0.5) == 2880 and not near(G.cw, old_cw), "the old assumption gave a 2880-wide canvas at 720p (1.5x too big)")
    G = probe_with({ vw = 3440, vh = 1440, scale = 1440 / 1080, gvc_broken = true })
    check(near(G.cw, 2580) and near(G.ch, 1080) and near(G.s, 1), "3440x1440: 2580x1080 canvas, scale 1")
    G = probe_with({ vw = 2560, vh = 1440, scale = nil })
    check(G.src == "gvc+curve" and near(G.cw, 1920) and near(G.s, 1), "no layout library: the legacy path + the default curve (" .. G.src .. ")")
    G = probe_with({ vw = 2560, vh = 1440, scale = 1440 / 1080, gvc_broken = true, wll_size_broken = true })
    check(G.src == "dpi-curve" and near(G.cw, 1920) and near(G.ch, 1080), "only the DPI known: 16:9 at the curve's short side")
    G = probe_with({ vw = 2560, vh = 1440, scale = nil, gvc_broken = true })
    check(G.src == "default+curve" and G.cw == 1920, "nothing known: the reference size")
end

-- ============================================================================================
-- 2. HSMPHud: every widget of the HUD at every resolution x DPI
-- ============================================================================================

local M = require("umg_mock")
local Kit, View, Panel

local function load_hud()
    for _, n in ipairs({ "hud_kit", "hud_view", "hud_panel", "hsmp_ui_scale" }) do package.loaded[n] = nil end
    Kit = require("hud_kit")
    View = require("hud_view")
    Panel = require("hud_panel")
    Kit.init({ Log = function() end, world_ok = function() return true end,
               construct = function(path, outer, name) return M.new_obj(path:match("%.(%w+)$"), name) end })
    Panel.init({ Log = function() end, world_ok = function() return true end, UEHelpers = nil,
                 construct = function(path, outer, name) return M.new_obj(path:match("%.(%w+)$"), name) end })
end

local LONG = "Sir Reginald Fitzwilliam the Unready"
local function model(n, board)
    local cells, opps, rows, feed = {}, {}, {}, {}
    for i = 1, n do cells[i] = { text = string.format("%s %d", i == 1 and "YOU" or ("P" .. i), i % 3), me = i == 1, dead = i == 3 } end
    for i = 1, View.MAX_OPPS do opps[i] = { name = (i == 1) and LONG or ("Opponent " .. i), hp = 55, hp_max = 100, st = 70, st_max = 100, down = i == 2, dead = i == 3,
                                                                          body = 100, con = 100, bleeding = i == 4 } end
    for i = 1, View.MAX_ROWS do rows[i] = { rank = tostring(i), nick = (i == 2) and LONG or ("Player " .. i), wins = "3", ping = "116", status = (i % 2 == 0) and "DEAD" or "ALIVE", me = i == 1 } end
    for i = 1, View.MAX_FEED do feed[i] = { text = (i == 1) and (LONG .. " slew Mate") or "Mate joined", alpha = 1, mine = i == 1 } end
    return {
        top = { title = "ROUND 3  -  BEST OF 5", timer = "1:23", cells = cells },
        opps = opps,
        me = { hp = 100, hp_max = 100, st = 40, st_max = 100, body = 100, con = 100, bleeding = true },   -- widest combat texts
        feed = feed,
        net = { level = "ok", text = "NET OK  116 ms  0.4% loss" },
        centre = (not board) and { title = "OPPONENT DISCONNECTED", sub = "waiting for them to reconnect   -   27" } or nil,
        board = board and { title = "SCOREBOARD  -  ROUND 3 / BEST OF 5", foot = "hold TAB", rows = rows } or nil,
    }
end

-- visible widgets of the HUD canvas, absolute (anchor * canvas + offset)
local function live_abs(cw, ch)
    local out = {}
    for _, w in ipairs(M.canvas_children) do
        local s = rawget(w, "slotobj")
        local vis = rawget(w, "vis")
        if s and vis ~= 1 and vis ~= 2 then
            local a = rawget(s, "anchors") or { Minimum = { X = 0.5, Y = 0.5 } }
            out[#out + 1] = { cls = rawget(w, "__cls"), text = rawget(w, "text") or "", fs = rawget(w, "fs"),
                              anchor = string.format("%.1f,%.1f", a.Minimum.X, a.Minimum.Y),
                              x = a.Minimum.X * cw + (rawget(s, "x") or 0), y = a.Minimum.Y * ch + (rawget(s, "y") or 0),
                              w = rawget(s, "w") or 0, h = rawget(s, "h") or 0 }
        end
    end
    return out
end

local REGION = { ["0.0,0.0"] = "opponents (TL)", ["0.5,0.0"] = "banner (TC)", ["1.0,0.0"] = "kill feed (TR)",
                 ["0.5,0.3"] = "centre", ["0.5,0.5"] = "scoreboard", ["0.0,1.0"] = "vitals (BL)", ["1.0,1.0"] = "net (BR)" }

local stats = { hud = 0, panel = 0 }

local function text_checks(tg, texts, dpi, ps)
    local short, wide, tiny = {}, {}, {}
    local minpx = UIS.min_font_px(ps)
    for _, t in ipairs(texts) do
        local fs = t.fs or 24
        if t.h + 1 < fs * UIS.LINE_H - 0.01 then short[#short + 1] = { t.text, t.h, fs } end
        if #t.text * fs * UIS.CHAR_W > t.w + 1 then wide[#wide + 1] = { t.text, t.w, fs } end
        -- the minimum (canvas units) for this DPI, and 9 physical px whenever the window is >= the floor
        if fs < UIS.min_font(dpi, ps / dpi) - 0.01 or (ps >= UIS.FLOOR_S - 1e-6 and fs * dpi < UIS.MIN_FONT_PX - 0.01) then
            tiny[#tiny + 1] = { t.text, fs, dpi }
        end
    end
    check(#short == 0, string.format("%s: no text taller than its box %s", tg, T.repr(short[1])))
    check(#wide == 0, string.format("%s: no text wider than its box %s", tg, T.repr(wide[1])))
    check(#tiny == 0, string.format("%s: every font >= %.1f physical px %s", tg, minpx, T.repr(tiny[1])))
end

local function hud_case(vw, vh, dpi_opt, n, board)
    local cw, ch, dpi = UIS.canvas(vw, vh, dpi_opt or nil)
    local tg = string.format("HUD %s n=%d%s", RES.tag(vw, vh, dpi_opt), n, board and " +board" or "")
    M.install({ vw = vw, vh = vh, scale = dpi_opt or nil })
    load_hud()
    local canvas = M.new_obj("CanvasPanel", "hud_canvas")
    Kit.begin({ tree = M.new_obj("WidgetTree", "wt"), canvas = canvas, kind = "test" })
    local w = View.build(cw, ch, dpi)
    View.apply(w, model(n, board), 0)
    local L = w.L
    local s = L.s
    local m = UIS.margin(cw, ch, s)
    local live = live_abs(cw, ch)
    check(#live > 40, string.format("%s: widgets built (%d)", tg, #live))
    -- inside the safe area
    local safe = { x = m, y = m, w = cw - 2 * m, h = ch - 2 * m }
    local out = T.filter(live, function(r) return not UIS.inside(r, safe, 1) end)
    check(#out == 0, string.format("%s: every widget inside the %d-unit safe margin (%d out, e.g. %s)", tg, m, #out,
        T.repr(out[1] and { out[1].text, out[1].anchor, math.floor(out[1].x), math.floor(out[1].y), out[1].w, out[1].h, cw, ch } or nil)))
    -- regions never overlap
    local groups = {}
    for _, r in ipairs(live) do
        local g = REGION[r.anchor] or r.anchor
        groups[g] = groups[g] or {}
        table.insert(groups[g], r)
    end
    local names = T.sorted(T.keys(groups))
    local clash = {}
    for i = 1, #names do
        for j = i + 1, #names do
            if UIS.overlap(UIS.bbox(groups[names[i]]), UIS.bbox(groups[names[j]])) then clash[#clash + 1] = names[i] .. " / " .. names[j] end
        end
    end
    check(#clash == 0, string.format("%s: HUD regions never overlap %s", tg, T.repr(clash)))
    -- texts never overlap each other
    local texts = T.filter(live, function(r) return r.cls == "TextBlock" and r.text:match("%S") end)
    local tov = {}
    for i = 1, #texts do
        for j = i + 1, #texts do
            if UIS.overlap(texts[i], texts[j]) then tov[#tov + 1] = { texts[i].text, texts[j].text } end
        end
    end
    check(#tov == 0, string.format("%s: no two texts overlap %s", tg, T.repr(tov[1])))
    text_checks(tg, texts, dpi, UIS.scale(cw, ch) * dpi)
    -- fixed labels are never clipped
    local have = T.set(T.map(texts, function(t) return t.text end))
    local want = { "YOU", "1:23" }
    if board then for _, h in ipairs({ "#", "PLAYER", "WINS", "PING", "STATUS" }) do want[#want + 1] = h end end
    local missing = T.filter(want, function(x) return not have[x] end)
    check(#missing == 0, string.format("%s: fixed labels unclipped %s", tg, T.repr(missing)))
    -- centred boxes: never wider than 16:9 of the height
    local maxw = UIS.max_width(cw, ch, s)
    check(L.top.w <= maxw and L.board.bg.w <= maxw, string.format("%s: banner %d / scoreboard %d <= max width %d", tg, L.top.w, L.board.bg.w, maxw))
    -- the corner columns stay in their corners on an ultrawide (anchored, not stretched)
    check(L.side <= math.ceil(380 * s) + 1, tg .. ": side columns keep their design width")
    stats.hud = stats.hud + 1
end

T.log("== HUD: every resolution x DPI, 1 / 2 / 8 score cells, with and without the TAB scoreboard")
for _, r in ipairs(RES.RES) do
    for _, d in ipairs(RES.DPI) do
        hud_case(r[1], r[2], d, 2, false)
        hud_case(r[1], r[2], d, 8, true)
        hud_case(r[1], r[2], d, 1, false)
    end
end

-- ============================================================================================
-- 3. HSMPHud panels: connection / reconnect modal, MP pause, match result
-- ============================================================================================

local SPECS = {
    { kind = "conn", modal = true, title = "CONNECTION LOST", tone = "bad",
      text = "The server stopped answering (timeout after 10 s). RECONNECT tries again for 30 s; BACK TO MENU leaves the match and keeps your career save.",
      buttons = { { id = "conn:reconnect", label = "RECONNECT", style = "primary" }, { id = "conn:menu", label = "BACK TO MENU" } } },
    { kind = "conn", modal = true, title = "RECONNECTING...", text = "27 s left",
      buttons = { { id = "conn:menu", label = "BACK TO MENU", style = "primary" } } },
    { kind = "pause", modal = true, title = "PAUSED", vertical = true, text = "Leave the match? In a duel your opponent wins by forfeit.",
      buttons = { { id = "pause:resume", label = "RESUME", style = "primary" }, { id = "pause:settings", label = "SETTINGS" },
                  { id = "pause:leave", label = "CONFIRM: LEAVE MATCH", style = "danger" }, { id = "pause:quit", label = "QUIT GAME", style = "danger" } } },
    { kind = "result", modal = false,
      buttons = { { id = "result:rematch", label = "REMATCH: STARTING WHEN READY", style = "primary" }, { id = "result:menu", label = "BACK TO LOBBY" } } },
}

local function panel_case(vw, vh, dpi_opt, spec)
    local cw, ch, dpi = UIS.canvas(vw, vh, dpi_opt or nil)
    local tg = string.format("panel %s %s", spec.title or spec.kind, RES.tag(vw, vh, dpi_opt))
    load_hud()
    local L = Panel.layout(spec, cw, ch, dpi)
    local s = L.s
    local m = UIS.margin(cw, ch, s)
    local safe = { x = m, y = m, w = cw - 2 * m, h = ch - 2 * m }
    local function abs(g) return { x = cw / 2 + g.x, y = ch / 2 + g.y, w = g.w, h = g.h } end
    local parts, texts = {}, {}
    for i, b in ipairs(L.buttons) do
        local r = abs(b)
        parts[#parts + 1] = r
        local lab = { x = r.x + L.label_pad, y = r.y, w = r.w - 2 * L.label_pad, h = r.h, fs = L.fs_btn }
        lab.text = Kit.fit(spec.buttons[i].label, lab.w, lab.fs, 1)
        check(lab.text == spec.buttons[i].label, string.format("%s: button label %q unclipped (%q)", tg, spec.buttons[i].label, lab.text))
        texts[#texts + 1] = lab
    end
    local out = T.filter(parts, function(r) return not UIS.inside(r, safe, 1) end)
    check(#out == 0, string.format("%s: buttons inside the safe area %s", tg, T.repr(out[1] and { out[1].x, out[1].y, out[1].w, out[1].h } or nil)))
    local ov = 0
    for i = 1, #parts do for j = i + 1, #parts do if UIS.overlap(parts[i], parts[j]) then ov = ov + 1 end end end
    check(ov == 0, tg .. ": buttons never overlap")
    if L.box then
        local box = abs(L.box)
        check(UIS.inside(box, safe, 1), string.format("%s: the box %.0fx%.0f inside the safe area of %.0fx%.0f", tg, box.w, box.h, cw, ch))
        check(box.w <= UIS.max_width(cw, ch, s), tg .. ": the box is never wider than 16:9 of the height")
        check(T.all(parts, function(r) return UIS.inside(r, box, 1) end), tg .. ": buttons inside the box")
        local title = abs(L.title); title.fs = L.fs_title; title.text = spec.title
        texts[#texts + 1] = title
        for i, line in ipairs(L.lines) do
            local g = abs(L.text[i]); g.fs = L.fs_text; g.text = Kit.fit(line, g.w, g.fs, 1)
            check(g.text == Kit.ascii(line) or i == #L.lines, string.format("%s: text line %d unclipped", tg, i))
            texts[#texts + 1] = g
        end
        for _, t in ipairs(texts) do check(UIS.inside(t, box, 1), tg .. ": text inside the box (" .. tostring(t.text) .. ")") end
        local tb = T.filter(texts, function(t) return t.fs ~= L.fs_btn end)
        local hit = 0
        for _, t in ipairs(tb) do for _, p in ipairs(parts) do if UIS.overlap(t, p) then hit = hit + 1 end end end
        check(hit == 0, tg .. ": title / text never under a button")
    else
        -- the result row sits under the centre banner (HUD centre message, anchor 0.5 / 0.3)
        local V = View.layout(cw, ch, 2, dpi)
        local sub_bottom = 0.3 * ch + V.centre.sub.y + V.centre.sub.h
        check(T.all(parts, function(r) return r.y >= sub_bottom - 1 end),
            string.format("%s: result buttons below the centre banner (%d >= %d)", tg, math.floor(parts[1].y), math.floor(sub_bottom)))
    end
    text_checks(tg, texts, dpi, UIS.scale(cw, ch) * dpi)
    stats.panel = stats.panel + 1
end

T.log("== panels: connection / reconnect modal, MP pause, match result at every resolution x DPI")
M.install({})
for _, r in ipairs(RES.RES) do
    for _, d in ipairs(RES.DPI) do
        for _, spec in ipairs(SPECS) do panel_case(r[1], r[2], d, spec) end
    end
end

check(stats.hud == #RES.RES * #RES.DPI * 3 and stats.panel == #RES.RES * #RES.DPI * #SPECS,
    string.format("every case ran (HUD %d, panels %d)", stats.hud, stats.panel))
