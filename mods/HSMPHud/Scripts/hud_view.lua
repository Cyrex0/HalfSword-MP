-- HSMPHud / hud_view.lua — the HUD layout (built once per host) and the
-- in-place update from a hud_model model. Widgets are created once per host
-- build and only re-texted / recoloured / shown / hidden afterwards.
--
-- Layout (canvas units; s = K.ui_scale; every element anchored to its corner):
--
--   top-left  (0,0)   opponents: name + CON / BODY % / BLEED, BODY bar, ST bar (max 7)
--   top-mid   (.5,0)  round banner: ROUND n / BEST OF k ......... timer
--                     score strip: one cell per player (max 8)
--   top-right (1,0)   kill feed (5 lines, newest first, fades)
--   centre    (.5,.3) phase message: title + sub line
--   centre    (.5,.5) scoreboard (hold TAB)
--   bot-left  (0,1)   YOU: BODY bar + CON / hp / BLEED, ST bar
--   bot-right (1,1)   net indicator: dot + "NET OK  42 ms  0.0%"

local V = {}
local K = require("hud_kit")

V.MAX_CELLS, V.MAX_OPPS, V.MAX_FEED, V.MAX_ROWS = 8, 7, 5, 8

local A_TL, A_TC, A_TR = { 0, 0 }, { 0.5, 0 }, { 1, 0 }
local A_C3, A_C5 = { 0.5, 0.3 }, { 0.5, 0.5 }
local A_BL, A_BR = { 0, 1 }, { 1, 1 }
V.ANCHORS = { TL = A_TL, TC = A_TC, TR = A_TR, C3 = A_C3, C5 = A_C5, BL = A_BL, BR = A_BR }

-- The HUD's smallest design font (the scoreboard footer). Its scale never goes
-- under the one where this font meets the minimum font, so on a small window
-- every text box grows with its font instead of the font outgrowing its box.
V.MIN_DESIGN_FS = 12

-- Pure geometry. Returns every rect (relative to its anchor) for n score cells.
-- dpi: the canvas' DPI scale (fonts never under UIS.MIN_FONT_PX physical px).
function V.layout(cw, ch, n, dpi)
    if dpi then K.set_dpi(dpi) end
    local s = K.layout_scale(cw, ch, V.MIN_DESIGN_FS)
    local L = { s = s, cw = cw, ch = ch, dpi = K.dpi }
    -- safe margin from every edge (shared rule; 20 design units without it)
    local m = (K.uis and K.uis.margin(cw, ch, s)) or math.floor(20 * s)
    local side = math.floor(380 * s)
    L.m, L.side = m, side
    -- free width between the two side columns
    local mid_w = cw - 2 * (side + 2 * m)
    L.mid_w = mid_w

    -- top-centre banner
    n = math.max(1, math.min(V.MAX_CELLS, n or 2))
    local W = math.floor(math.min(K.clamp(n * 150, 460, 1100) * s, mid_w))
    local pad = math.floor(6 * s)
    local th, sh = math.floor(34 * s), math.floor(30 * s)
    L.top = { x = -math.floor(W / 2), y = m, w = W, h = pad + th + pad + sh + pad }
    L.title = { x = L.top.x + 2 * pad, y = m + pad, w = math.floor(W * 0.62) - 2 * pad, h = th, fs = K.fs(20, s) }
    local tw = math.floor(W * 0.36) - 2 * pad
    L.timer = { x = L.top.x + W - 2 * pad - tw, y = m + pad, w = tw, h = th, fs = K.fs(20, s) }
    L.cells = {}
    local cy = m + pad + th + pad
    local cwid = math.floor((W - 2 * pad) / n)
    local gap = math.max(2, math.floor(4 * s))
    for i = 1, n do
        local x = L.top.x + pad + (i - 1) * cwid
        L.cells[i] = { x = x, y = cy, w = cwid - gap, h = sh, fs = K.fs(15, s) }
    end

    -- opponents (top-left)
    L.opps = {}
    local rh = math.floor(58 * s)
    for i = 1, V.MAX_OPPS do
        local y = m + (i - 1) * rh
        local nh = math.floor(22 * s)
        local bw = side - math.floor(8 * s)
        L.opps[i] = {
            bg   = { x = m, y = y, w = side, h = rh - math.floor(4 * s) },
            name = { x = m + math.floor(6 * s), y = y + math.floor(2 * s), w = math.floor(side * 0.56) - math.floor(6 * s), h = nh, fs = K.fs(15, s) },
            vals = { x = m + math.floor(side * 0.56), y = y + math.floor(2 * s), w = math.floor(side * 0.44) - math.floor(6 * s), h = nh, fs = K.fs(13, s) },
            hp   = { x = m + math.floor(4 * s), y = y + nh + math.floor(4 * s), w = bw, h = math.floor(12 * s) },
            st   = { x = m + math.floor(4 * s), y = y + nh + math.floor(20 * s), w = bw, h = math.max(3, math.floor(6 * s)) },
        }
    end

    -- local vitals (bottom-left), offsets are negative from the bottom edge
    local vw = math.floor(360 * s)
    -- (the backdrop sits on the safe margin; its content is padded inside it)
    local st_h, hp_h, lab_h = math.max(4, math.floor(10 * s)), math.floor(20 * s), math.floor(22 * s)
    local p6 = math.floor(6 * s)
    local st_y = -m - p6 - st_h
    local hp_y = st_y - math.floor(4 * s) - hp_h
    local lab_y = hp_y - math.floor(4 * s) - lab_h
    local bg_y = lab_y - math.floor(4 * s)
    local ix = m + p6
    L.me = {
        bg   = { x = m, y = bg_y, w = vw + 2 * p6, h = -m - bg_y },
        name = { x = ix, y = lab_y, w = math.floor(vw * 0.5), h = lab_h, fs = K.fs(15, s) },
        vals = { x = ix + math.floor(vw * 0.5), y = lab_y, w = vw - math.floor(vw * 0.5), h = lab_h, fs = K.fs(13, s) },
        hp   = { x = ix, y = hp_y, w = vw, h = hp_h, fs = K.fs(13, s) },
        st   = { x = ix, y = st_y, w = vw, h = st_h },
    }

    -- kill feed (top-right)
    L.feed = {}
    local fh, fg = math.floor(26 * s), math.floor(4 * s)
    for i = 1, V.MAX_FEED do
        local y = m + (i - 1) * (fh + fg)
        L.feed[i] = { bg = { x = -m - side, y = y, w = side, h = fh },
                      tx = { x = -m - side + math.floor(8 * s), y = y, w = side - math.floor(16 * s), h = fh, fs = K.fs(15, s) } }
    end

    -- net indicator (bottom-right)
    local nw, nh2 = math.floor(300 * s), math.floor(30 * s)
    local dot = math.max(6, math.floor(12 * s))
    L.net = {
        bg  = { x = -m - nw, y = -m - nh2, w = nw, h = nh2 },
        dot = { x = -m - nw + math.floor(10 * s), y = -m - nh2 + math.floor((nh2 - dot) / 2), w = dot, h = dot },
        tx  = { x = -m - nw + math.floor(10 * s) + dot + math.floor(8 * s), y = -m - nh2,
                w = nw - dot - math.floor(28 * s), h = nh2, fs = K.fs(14, s) },
    }

    -- centre phase message (anchor 0.5, 0.3)
    local cwid2 = math.floor(math.min(1400 * s, mid_w))
    L.centre = {
        title = { x = -math.floor(cwid2 / 2), y = -math.floor(50 * s), w = cwid2, h = math.floor(80 * s), fs = K.fs(56, s) },
        sub   = { x = -math.floor(cwid2 / 2), y = math.floor(34 * s), w = cwid2, h = math.floor(40 * s), fs = K.fs(24, s) },
    }

    -- scoreboard (anchor 0.5, 0.5)
    local bw2 = math.floor(math.min(820 * s, mid_w))
    local hh, ch2, rr, fo, bp = math.floor(44 * s), math.floor(28 * s), math.floor(32 * s), math.floor(26 * s), math.floor(8 * s)
    local bh = bp + hh + ch2 + V.MAX_ROWS * rr + fo + bp
    local bx, by = -math.floor(bw2 / 2), -math.floor(bh / 2)
    local cols = { { "#", 0.07, 1 }, { "PLAYER", 0.43, 0 }, { "WINS", 0.14, 1 }, { "PING", 0.17, 1 }, { "STATUS", 0.19, 1 } }
    L.board = { bg = { x = bx, y = by, w = bw2, h = bh },
                title = { x = bx + 2 * bp, y = by + bp, w = bw2 - 4 * bp, h = hh, fs = K.fs(20, s) },
                foot = { x = bx + 2 * bp, y = by + bp + hh + ch2 + V.MAX_ROWS * rr, w = bw2 - 4 * bp, h = fo, fs = K.fs(12, s) },
                cols = {}, rows = {} }
    local inner = bw2 - 2 * bp
    local cx = bx + bp
    for ci, c in ipairs(cols) do
        local w = math.floor(inner * c[2])
        L.board.cols[ci] = { name = c[1], just = c[3], x = cx, w = w }
        cx = cx + w
    end
    L.board.head = { y = by + bp + hh, h = ch2, fs = K.fs(13, s) }
    for r = 1, V.MAX_ROWS do
        L.board.rows[r] = { y = by + bp + hh + ch2 + (r - 1) * rr, h = rr, fs = K.fs(16, s) }
    end
    return L
end

-- --- build ------------------------------------------------------------------------------

local function R(anchor, g, color, z) return K.rect(anchor, g.x, g.y, g.w, g.h, color, z) end
local function T(anchor, g, just, color, z, fs, s)
    return K.text(anchor, "", g.x, g.y, g.w, g.h, fs or g.fs, just, color, { z = z, scale = s })
end

local function move_text(t, g)
    if not t then return end
    if t.box and t.box[1] == g.x and t.box[2] == g.y and t.box[3] == g.w and t.box[4] == g.h then return end
    t.box = { g.x, g.y, g.w, g.h }
    local th = math.min(g.h, math.ceil(t.fs * K.LINE_H))
    t.w = g.w
    K.resize(t, g.x, g.y + math.floor((g.h - th) / 2), g.w, th)
    local s = t.last; t.last = nil; K.set_text(t, s)   -- re-fit at the new width
end

-- Build every widget into the current K.host. cw, ch = canvas size.
function V.build(cw, ch, dpi)
    local L = V.layout(cw, ch, 2, dpi)
    local s = L.s
    local w = { L = L, n = 2 }
    w.top_bg = R(A_TC, L.top, K.C.panel, 10)
    w.title = T(A_TC, L.title, 0, K.C.head, 13, nil, s)
    w.timer = T(A_TC, L.timer, 2, K.C.white, 13, nil, s)
    w.cells = {}
    for i = 1, V.MAX_CELLS do
        local g = L.cells[i] or L.cells[#L.cells]
        w.cells[i] = { bg = R(A_TC, g, K.C.strip, 11), tx = T(A_TC, g, 1, K.C.text, 13, g.fs, s) }
    end
    for i = 1, 2 do w.cells[i].tx.box = { L.cells[i].x, L.cells[i].y, L.cells[i].w, L.cells[i].h } end

    w.opps = {}
    for i = 1, V.MAX_OPPS do
        local g = L.opps[i]
        w.opps[i] = { bg = R(A_TL, g.bg, K.C.feed, 10),
                      name = T(A_TL, g.name, 0, K.C.text, 13, nil, s),
                      vals = T(A_TL, g.vals, 2, K.C.text, 13, nil, s),
                      hp = K.bar(A_TL, g.hp.x, g.hp.y, g.hp.w, g.hp.h, K.C.good, nil, 11),
                      st = K.bar(A_TL, g.st.x, g.st.y, g.st.w, g.st.h, K.C.stam, nil, 11) }
    end

    local me = L.me
    w.me = { bg = R(A_BL, me.bg, K.C.feed, 10),
             name = T(A_BL, me.name, 0, K.C.head, 13, nil, s),
             vals = T(A_BL, me.vals, 2, K.C.text, 13, nil, s),
             hp = K.bar(A_BL, me.hp.x, me.hp.y, me.hp.w, me.hp.h, K.C.good, me.hp.fs, 11),
             st = K.bar(A_BL, me.st.x, me.st.y, me.st.w, me.st.h, K.C.stam, nil, 11) }

    w.feed = {}
    for i = 1, V.MAX_FEED do
        local g = L.feed[i]
        w.feed[i] = { bg = R(A_TR, g.bg, K.C.feed, 10), tx = T(A_TR, g.tx, 2, K.C.white, 13, nil, s) }
    end

    w.net = { bg = R(A_BR, L.net.bg, K.C.feed, 10), dot = R(A_BR, L.net.dot, K.C.off, 12),
              tx = T(A_BR, L.net.tx, 0, K.C.text, 13, nil, s) }

    w.c_title = T(A_C3, L.centre.title, 1, K.C.title, 20, nil, s)
    w.c_sub = T(A_C3, L.centre.sub, 1, K.C.white, 20, nil, s)

    local B = L.board
    w.board = { bg = R(A_C5, B.bg, K.C.panel, 30),
                title = T(A_C5, B.title, 0, K.C.head, 33, nil, s),
                foot = T(A_C5, B.foot, 2, K.C.dim, 33, nil, s),
                head = {}, rows = {} }
    for ci, c in ipairs(B.cols) do
        w.board.head[ci] = T(A_C5, { x = c.x + math.floor(4 * s), y = B.head.y, w = c.w - math.floor(8 * s), h = B.head.h },
                             c.just, K.C.dim, 33, B.head.fs, s)
        K.set_text(w.board.head[ci], c.name)
    end
    for r = 1, V.MAX_ROWS do
        local rg = B.rows[r]
        local row = { bg = R(A_C5, { x = B.bg.x + math.floor(8 * s), y = rg.y, w = B.bg.w - math.floor(16 * s), h = rg.h - math.floor(2 * s) },
                              (r % 2 == 1) and K.C.row or K.C.rowb, 31), cells = {} }
        for ci, c in ipairs(B.cols) do
            row.cells[ci] = T(A_C5, { x = c.x + math.floor(4 * s), y = rg.y, w = c.w - math.floor(8 * s), h = rg.h }, c.just, K.C.text, 33, rg.fs, s)
        end
        w.board.rows[r] = row
    end
    K.set_text(w.me.name, "YOU")
    return w
end

-- --- update -----------------------------------------------------------------------------

local function hp_color(f)
    if f > 0.6 then return K.C.good elseif f > 0.3 then return K.C.ok end
    return K.C.bad
end

local function show_all(list, on) for _, e in ipairs(list) do K.show(e, on) end end

-- Vitals (hud_state.vitals_metrics): CON, BODY %, BLEED; raw Health only as
-- a small "hp". Without the body/consciousness fields: plain HP / ST.
local function r0(x) return math.floor(x + 0.5) end
V.has_body = function(o) return o.body ~= nil or o.con ~= nil end
function V.opp_text(o)
    if not V.has_body(o) then
        return (o.hp and string.format("HP %d", r0(o.hp)) or "HP --") .. (o.st and string.format("  ST %d", r0(o.st)) or "")
    end
    local t = string.format("CON %s BODY %s", o.con and tostring(r0(o.con)) or "--", o.body and (r0(o.body) .. "%") or "--")
    return t   -- bleeding: the row turns red (V.apply; no room for a word at 1280x720)
end
function V.bar_frac(o)
    if o.body then return o.body / 100 end
    return (o.hp or 0) / math.max(1, o.hp_max or 100)
end
function V.me_label(me)
    if me.dead then return "DEAD" end
    if me.body then return string.format("BODY %d%%", r0(me.body)) end
    return string.format("HP %d", r0(me.hp))
end
function V.me_vals(me)
    if me.dead then return "DEAD" end
    if not V.has_body(me) then return me.st and string.format("ST %d", r0(me.st)) or "ST --" end
    local t = string.format("CON %s  hp %d", me.con and tostring(r0(me.con)) or "--", r0(me.hp or 0))
    if me.bleeding then t = t .. "  BLEED" end
    return t
end

local NET_COLOR = { good = K.C.good, ok = K.C.ok, poor = K.C.warn, bad = K.C.bad, none = K.C.off }

local function set_board_visible(w, on)
    K.show(w.board.bg, on); K.show(w.board.title, on); K.show(w.board.foot, on)
    show_all(w.board.head, on)
    for _, row in ipairs(w.board.rows) do K.show(row.bg, on); show_all(row.cells, on) end
end

function V.apply(w, model, now)
    if not w or not K.alive() then return end
    local L = w.L

    -- top banner + score strip
    local top = model.top
    local n = top and math.max(1, math.min(V.MAX_CELLS, #top.cells)) or 2
    if n ~= w.n then
        w.n = n
        local L2 = V.layout(L.cw, L.ch, n, L.dpi)
        w.L = L2; L = L2
        K.resize(w.top_bg, L.top.x, L.top.y, L.top.w, L.top.h)
        move_text(w.title, L.title); move_text(w.timer, L.timer)
        for i = 1, n do
            K.resize(w.cells[i].bg, L.cells[i].x, L.cells[i].y, L.cells[i].w, L.cells[i].h)
            move_text(w.cells[i].tx, L.cells[i])
        end
    end
    K.show(w.top_bg, top ~= nil); K.show(w.title, top ~= nil); K.show(w.timer, top ~= nil)
    if top then
        K.set_text(w.title, top.title)
        K.set_text(w.timer, top.timer or "")
    end
    for i = 1, V.MAX_CELLS do
        local c = top and top.cells[i]
        local on = c ~= nil and i <= n
        K.show(w.cells[i].bg, on); K.show(w.cells[i].tx, on)
        if on then
            K.set_text(w.cells[i].tx, c.text)
            K.set_rect_color(w.cells[i].bg, c.me and K.C.me_cell or K.C.strip)
            K.set_color(w.cells[i].tx, c.dead and K.C.dim or (c.me and K.C.title or K.C.text))
        end
    end

    -- opponents
    for i = 1, V.MAX_OPPS do
        local o = model.opps and model.opps[i]
        local W = w.opps[i]
        local on = o ~= nil
        K.show(W.bg, on); K.show(W.name, on); K.show(W.vals, on)
        K.bar_show(W.hp, on); K.bar_show(W.st, on)
        if on then
            K.set_text(W.name, o.name)
            local v
            if o.dead then v = "DEAD"
            else
                v = V.opp_text(o)
                if o.down then v = "DOWN  " .. v end
            end
            K.set_text(W.vals, v)
            K.set_color(W.name, o.dead and K.C.dim or K.C.text)
            K.set_color(W.vals, (o.dead or o.bleeding) and K.C.bad or (o.down and K.C.warn or K.C.text))
            local hf = o.dead and 0 or V.bar_frac(o)
            K.bar_set(W.hp, hf, hp_color(hf))
            K.bar_set(W.st, o.dead and 0 or ((o.st or 0) / math.max(1, o.st_max or 100)), K.C.stam)
        end
    end

    -- local vitals
    local me = model.me
    local mw = w.me
    local on = me ~= nil
    K.show(mw.bg, on); K.show(mw.name, on); K.show(mw.vals, on)
    K.bar_show(mw.hp, on); K.bar_show(mw.st, on)
    if on then
        local hf = me.dead and 0 or V.bar_frac(me)
        K.bar_set(mw.hp, hf, hp_color(hf), V.me_label(me))
        K.bar_set(mw.st, me.dead and 0 or ((me.st or 0) / math.max(1, me.st_max or 100)), K.C.stam)
        K.set_text(mw.vals, V.me_vals(me))
        K.set_color(mw.vals, (me.dead or me.bleeding) and K.C.bad or K.C.text)
    end

    -- kill feed
    for i = 1, V.MAX_FEED do
        local f = model.feed and model.feed[i]
        local F = w.feed[i]
        K.show(F.bg, f ~= nil); K.show(F.tx, f ~= nil)
        if f then
            K.set_text(F.tx, f.text)
            K.set_color(F.tx, (f.host and K.C.ok) or (f.notice and K.C.head) or (f.mine and K.C.title) or K.C.white)
            K.opacity(F.bg, f.alpha); K.opacity(F.tx, f.alpha)
        end
    end

    -- net
    local net = model.net
    K.show(w.net.bg, net ~= nil); K.show(w.net.dot, net ~= nil); K.show(w.net.tx, net ~= nil)
    if net then
        K.set_rect_color(w.net.dot, NET_COLOR[net.level] or K.C.off)
        K.set_text(w.net.tx, net.text)
        K.set_color(w.net.tx, net.level == "bad" and K.C.bad or (net.level == "poor" and K.C.warn or K.C.text))
    end

    -- centre message
    local c = model.centre
    K.show(w.c_title, c ~= nil); K.show(w.c_sub, c ~= nil and (c.sub or "") ~= "")
    if c then
        K.set_text(w.c_title, c.title)
        K.set_text(w.c_sub, c.sub or "")
        K.set_color(w.c_title, c.tone == "bad" and K.C.bad or (c.tone == "good" and K.C.good or K.C.title))
    end

    -- scoreboard
    local b = model.board
    set_board_visible(w, b ~= nil)
    if b then
        K.set_text(w.board.title, b.title)
        K.set_text(w.board.foot, b.foot or "")
        for r = 1, V.MAX_ROWS do
            local row, d = w.board.rows[r], b.rows[r]
            K.show(row.bg, d ~= nil)
            for ci, t in ipairs(row.cells) do
                K.show(t, d ~= nil)
                if d then
                    local val = ({ d.rank, d.nick, d.wins, d.ping, d.status })[ci]
                    K.set_text(t, val)
                    local col = d.me and K.C.title or K.C.text
                    if ci == 5 then col = (d.status == "DEAD") and K.C.bad or ((d.status == "LOADING") and K.C.ok or K.C.good) end
                    K.set_color(t, col)
                end
            end
        end
    end
end

return V
