-- HSMPHud / hud_state.lua — read-only views of the state the HUD displays. No UObject is
-- touched here.
--
--   link record          status, my peer id, admin flag, kick / reject reason, transport
--                        metrics (shared/hsmp_session.lua HS.link())
--   peer directory       roster nicks + server-measured RTT (IPC.peer_dir())
--   session record       the server's session snapshot -> the match view (HS.view())
--   bus conn_state       HSMPMatch Director: the one connection state
--   bus spectate         HSMPMatch: who the camera follows after my death
--   notice events        server notices (typed S2G records)
--   .settings.json       my nick, HUD switches (a real config file)
--   `vitals` record      my own vitals (HSMPCombat, IPC.rec("vitals"))
--   `peer_vitals` slots  each opponent's vitals record (IPC.peer_rec)
--   `death` records      authoritative deaths (kill feed; S2G events)
--
-- Every view tolerates missing records and missing fields.

local S = {}

-- One line of a real file (.settings.json) through the facade's plain file helper.
local function read_line(path)
    local ipc = rawget(_G, "HSMP_IPC")
    return (ipc and ipc.read(path, true)) or nil
end
S.read_line = read_line

-- A one-line JSON contract; nil when absent.
local function read_json_line(path)
    local line = read_line(path)
    if not line then return nil end
    local t = line:match("^%s*(.-)%s*$")
    if t:sub(1, 1) == "{" and t:sub(-1) == "}" then return line end
    return nil
end
S.read_json_line = read_json_line

local function num(s, key)
    local v = s:match('"' .. key .. '"%s*:%s*(%-?[%d%.]+)')
    return v and tonumber(v) or nil
end

-- shared/hsmp_session.lua (deploy copies shared/*.lua next to the mod's scripts).
local HSS = nil
local function hss()
    if HSS then return HSS end
    local ok, m = pcall(require, "hsmp_session")
    if ok and type(m) == "table" and m.view then HSS = m; return m end
    local ipc = rawget(_G, "HSMP_IPC")
    m = ipc and ipc.load_lib and ipc.load_lib("hsmp_session")
    if type(m) == "table" and m.view then HSS = m end
    return HSS
end
S.hss = hss

-- --- the match (session record) ----------------------------------------------------------

-- The HUD's match table from the normalised session view (HS.view()).
function S.match_from(v)
    if type(v) ~= "table" then return nil end
    local m = {
        state = v.state or "lobby", round = v.round or 0, countdown = v.countdown_s or 0,
        best_of = v.best_of or 0, last_winner = v.last_winner or 0, reason = v.reason or "",
        arena = v.arena or "", seq = v.seq or 0,
        order = {}, wins = {}, alive = {}, waiting = {}, ready = {},
    }
    -- A round that ended but has no result yet (the server's settle window: no winner, no
    -- result reason) is "pending", not a draw.
    if m.state == "roundover" and (v.result_reason or 0) == 0 and m.last_winner == 0 then m.reason = "pending" end
    -- Live with a deadline: a round time limit (seconds left).
    if m.state == "live" and v.deadline_in_ms then m.round_left = math.ceil(v.deadline_in_ms / 1000) end
    for _, e in ipairs(v.scoreboard or {}) do
        local id = e[1]
        m.order[#m.order + 1] = id
        m.wins[id] = e[2]
        m.alive[id] = (e[3] == 1)
    end
    for _, id in ipairs(v.waiting_on or {}) do m.waiting[#m.waiting + 1] = id end
    for _, id in ipairs(v.ready or {}) do m.ready[id] = true end
    return m
end

-- --- the sidecar (link record + peer directory) -----------------------------------------

-- {status, my_id, nicks{id}, ping{id} (server-measured RTT, nil = unknown), ids}
function S.sidecar_from(link, peers)
    if type(link) ~= "table" then return nil end
    local H = hss()
    local sc = { status = (H and H.status_name(link)) or "?", my_id = link.my_peer_id or 0,
                 nicks = {}, ping = {}, ids = {} }
    for _, e in ipairs(peers or {}) do
        local id = e.id
        if id then
            sc.ids[#sc.ids + 1] = id
            sc.nicks[id] = (e.nick and e.nick ~= "") and e.nick or ("P" .. id)
            local r = tonumber(e.rtt_ms)
            sc.ping[id] = (r and r > 0) and r or nil
        end
    end
    return sc
end

function S.parse_settings_nick(line)
    return line and line:match('"nick"%s*:%s*"([^"]+)"') or nil
end

-- HUD switches from .settings.json (HSMPMenu SETTINGS,
-- docs/development/subsystems/menu-ui.md):
--   hud           bool   master switch (default true)
--   hud_killfeed  bool   kill feed (default true)
--   hud_net       "always" | "bad" (alias "when_bad") | "off"   net indicator (default always)
-- Unknown / missing values fall back to the defaults.
S.HUD_PREFS_DEFAULT = { hud = true, killfeed = true, net = "always" }
function S.parse_hud_prefs(line)
    local p = { hud = true, killfeed = true, net = "always" }
    if not line then return p end
    local function bool(k)
        local v = line:match('"' .. k .. '"%s*:%s*(%a+)')
        if v == "true" then return true elseif v == "false" then return false end
        return nil
    end
    local h, kf = bool("hud"), bool("hud_killfeed")
    if h ~= nil then p.hud = h end
    if kf ~= nil then p.killfeed = kf end
    local n = line:match('"hud_net"%s*:%s*"(%a[%w_]*)"')
    if n == "always" or n == "off" then p.net = n
    elseif n == "bad" or n == "when_bad" then p.net = "bad" end
    return p
end

-- --- transport metrics (link record) -----------------------------------------------------

-- nil until the sidecar has a metrics sample (metrics_wall_ms 0). The 10 s loss
-- window wins (negative = no window yet: the lifetime figure is the fallback).
function S.metrics_from(link)
    if type(link) ~= "table" or (link.metrics_wall_ms or 0) == 0 then return nil end
    local w = link.loss_pct_10s
    return {
        rtt = link.rtt_ms,
        loss = (w ~= nil and w >= 0) and w or link.loss_pct,
        jitter = link.jitter_ms,
        last_rx = link.rx_age_ms,
        ts = link.metrics_wall_ms,
    }
end

-- --- vitals ---------------------------------------------------------------------------------

-- The `vitals` record (schema/combat.rs Vitals; my own: IPC.rec("vitals"),
-- an opponent's: IPC.peer_rec("peer_vitals", id)): {seq, dism, flags, v[19]}
-- with v quantised once by the owner (u16 = round(x * 64), 65535 unknown).
-- Flags: 1 DEAD, 2 FALLEN, 4 DOWNED. v[14] (Lua, wire #13) is Stamina.
-- What decides a Half Sword fight is not raw Health (it only drops on hard
-- torso / head / neck hits, never below a per-part floor, and regenerates
-- about (Health - 10) % per second): it is consciousness (KO, the
-- consciousness-cap loss), the body parts (Snap Neck and head crush kill,
-- broken limbs disable) and blood loss. The HUD shows:
--   CON   Consciousness (wire #11), 0..100.
--   BODY  weighted mean of the part healths, %: head 3, neck 3, upper torso 2,
--         lower torso 1.5, back 0.5, each arm and leg 0.75 (sum 13). The fatal
--         parts dominate; a limb at 0 costs ~6 %. Unknown parts are skipped.
--   BLEED Bleeding (wire #15) above BLEED_MIN.
--   hp    raw Health, secondary.
-- The same function reads my own record and every opponent's: both screens
-- show identical numbers.
S.BODY_W = { [2] = 3, [3] = 3, [4] = 2, [5] = 1.5, [6] = 0.5, [7] = 0.75, [8] = 0.75, [9] = 0.75, [10] = 0.75 }
S.BLEED_MIN = 0.25
S.VITALS_N = 19
S.VITALS_UNKNOWN = 65535
local function q64(x) return x and math.floor(x * 64 + 0.5) / 64 or nil end
S.q64 = q64
function S.vitals_metrics(v)
    if type(v) ~= "table" then return nil end
    local sum, wsum = 0, 0
    for i, w in pairs(S.BODY_W) do
        local x = q64(v[i])
        if x then sum = sum + w * math.max(0, math.min(100, x)); wsum = wsum + w end
    end
    local con, bleed = q64(v[12]), q64(v[16])
    return {
        body = wsum > 0 and sum / wsum or nil,
        con = con and math.max(0, math.min(100, con)) or nil,
        bleed = bleed,
        bleeding = bleed ~= nil and bleed > S.BLEED_MIN,
    }
end

-- A `vitals` record table -> {seq, hp, st, dead, down, body, con, bleed,
-- bleeding}; nil without a record. dead = DEAD flag or Health <= 0.
function S.vitals_from_record(t)
    if type(t) ~= "table" then return nil end
    local src = type(t.v) == "table" and t.v or {}
    local v = {}
    for i = 1, S.VITALS_N do
        local q = math.tointeger(src[i])
        v[i] = (q and q ~= S.VITALS_UNKNOWN) and q / 64 or nil
    end
    local f = math.tointeger(t.flags) or 0
    local r = { seq = math.tointeger(t.seq) or 0, hp = v[1], st = v[14] }
    r.dead = f & 1 ~= 0 or (r.hp ~= nil and r.hp <= 0)
    r.down = f & 6 ~= 0 and not r.dead
    local m = S.vitals_metrics(v)
    if m then r.body, r.con, r.bleed, r.bleeding = m.body, m.con, m.bleed, m.bleeding end
    return r
end

-- --- deaths (typed `death` records, S2G) ------------------------------------------------------

-- Reader state: { seen = {}, primed = bool }. The event cursor of this Lua
-- state starts at the first poll (old deaths from earlier matches are not news).
function S.new_tail(path)
    return { path = path, off = 0, seen = {}, primed = false }
end

-- A `death` record {peer_id, round, killer, cause, match_id, wall_ms} ->
-- {victim, round, killer, cause, t, match_id (nil = none)}.
function S.death_of(d)
    if type(d) ~= "table" then return nil end
    local victim = math.tointeger(d.peer_id)
    if not victim or victim == 0 then return nil end
    local mid = math.tointeger(d.match_id)
    local round = math.tointeger(d.round)
    return { victim = victim, round = round, killer = math.tointeger(d.killer) or 0,
             cause = math.tointeger(d.cause), t = math.tointeger(d.wall_ms), match_id = (mid ~= 0) and mid or nil }
end

-- Round numbers restart in every match, so the dedup key carries a match
-- context: the record's match_id when the sidecar sets one, else a local
-- generation that moves when the match state falls back to the lobby after a
-- match, and when a death's round goes down (match 2 started without the HUD
-- seeing the lobby). A missing match view is no evidence.
function S.observe_match(tail, match)
    if type(match) ~= "table" then return end
    local st = match.state or "none"
    local in_m = st ~= "lobby" and st ~= "none"
    if tail.in_match and not in_m then tail.gen = (tail.gen or 0) + 1; tail.max_round = 0 end
    tail.in_match = in_m
end

function S.death_key(tail, d)
    if not d.round then return d.victim .. "@" .. tostring(d.t) end
    if d.match_id then return "m" .. d.match_id .. ":" .. d.victim .. ":" .. d.round end
    if d.round < (tail.max_round or 0) then tail.gen = (tail.gen or 0) + 1; tail.max_round = 0 end
    if d.round > (tail.max_round or 0) then tail.max_round = d.round end
    return "g" .. (tail.gen or 0) .. ":" .. d.victim .. ":" .. d.round
end

-- Returns a list of new deaths (deduped by match+victim+round). `match`:
-- the current parsed match sub-map (optional) for the match context.
function S.poll_tail(tail, match)
    S.observe_match(tail, match)
    -- S2G `death` records (shared/hsmp_ipc.lua; this Lua state's cursor).
    local ipc = rawget(_G, "HSMP_IPC")
    local evs = (ipc and ipc.events("death")) or {}
    tail.primed = true
    local out = {}
    for _, e in ipairs(evs) do
        local d = S.death_of(e.data)
        if d and d.victim then
            local key = S.death_key(tail, d)
            if not tail.seen[key] then tail.seen[key] = true; out[#out + 1] = d end
        end
    end
    return out
end

-- --- conn_state (bus; HSMPMatch Director: the one connection state) -----------------------

-- The typed bus record -> {seq, state, reason, title, text, remaining, elapsed, window,
-- in_match, latched, actions}; nil when absent (never written, or cleared: state "").
function S.conn_from(t)
    if type(t) ~= "table" or (t.state or "") == "" then return nil end
    local c = { seq = t.seq or 0, state = t.state, reason = (t.reason ~= "") and t.reason or nil,
                title = (t.title ~= "") and t.title or nil, text = (t.text ~= "") and t.text or nil,
                remaining = t.remaining_s or 0, elapsed = t.elapsed_s or 0, window = t.window_s or 0,
                in_match = t.in_match == true, latched = t.latched == true, actions = {} }
    for _, a in ipairs(t.actions or {}) do if a ~= "" then c.actions[#c.actions + 1] = a end end
    return c
end

-- --- spectate (bus; HSMPMatch: who the camera follows after my death) ----------------------

function S.spectate_from(t)
    if type(t) ~= "table" or (t.target or 0) == 0 then return nil end
    return { target = t.target, nick = t.nick or "", alive = t.alive or 0 }
end

-- --- server notices (typed S2G `notice` events) --------------------------------------------

-- Returns the new notices {event_id, name, text} once each (dedup by event_id). `evs`:
-- the events to read (tests); default IPC.events("notice") of this Lua state's cursor.
function S.poll_notices(tail, evs)
    local out = {}
    tail.ids = tail.ids or {}
    tail.primed = true
    if evs == nil then
        local ipc = rawget(_G, "HSMP_IPC")
        evs = (ipc and ipc.events and ipc.events("notice")) or {}
    end
    local H = hss()
    for _, e in ipairs(evs) do
        local n = e.data or e
        local id = n.event_id
        if id and not tail.ids[id] then
            local text = H and H.notice_text(n) or ""
            if text ~= "" then
                tail.ids[id] = true
                out[#out + 1] = { event_id = id, name = H and H.notice_name(n) or "notice", text = text }
            end
        end
    end
    return out
end

-- --- one snapshot of everything ---------------------------------------------------------------

function S.read_all(dir, peer_ids_hint)
    local snap = {}
    local ipc = rawget(_G, "HSMP_IPC")
    local H = hss()
    local link, lver = nil, nil
    if ipc and ipc.rec then link, lver = ipc.rec("link") end
    if type(link) ~= "table" then link, lver = nil, nil end
    snap.link = link
    -- change keys: the link record's version, the metrics sample's wall clock
    snap.sidecar_raw = lver
    local peers = (ipc and ipc.peer_dir and ipc.peer_dir().list) or {}
    snap.sc = S.sidecar_from(link, peers)
    snap.match = S.match_from(H and H.view())
    snap.metrics = S.metrics_from(link)
    snap.metrics_raw = snap.metrics and snap.metrics.ts or nil
    -- liveness is the sidecar's header heartbeat, never "the record changed"
    snap.hb_age = H and H.hb_age() or nil
    local settings = read_json_line(dir .. "/.settings.json")
    snap.my_nick = S.parse_settings_nick(settings)
    snap.prefs = S.parse_hud_prefs(settings)
    -- my own `vitals` record (HSMPCombat writes it; the sidecar sends it): read
    -- with the opponents' reader, so both screens compute the same numbers
    snap.vown = S.vitals_from_record(ipc and ipc.rec("vitals"))
    snap.conn = S.conn_from(ipc and ipc.bus_table and ipc.bus_table("conn_state"))
    snap.spectate = S.spectate_from(ipc and ipc.bus_table and ipc.bus_table("spectate"))
    snap.is_admin = link ~= nil and link.is_admin == true
    snap.rejected = nil
    local st = snap.sc and snap.sc.status
    if st == "rejected" then
        -- (kicked / server closed: the Director's connection modal shows those)
        snap.rejected = (link.reason ~= "" and link.reason) or "rejected by the server"
    end
    -- remote vitals only for peers in the roster (old sessions' peers are ignored)
    snap.vremote = {}
    local my = snap.sc and snap.sc.my_id or 0
    local ids = {}
    for _, id in ipairs(peer_ids_hint or {}) do ids[id] = true end
    if snap.match then for _, id in ipairs(snap.match.order) do ids[id] = true end end
    if snap.sc then for _, id in ipairs(snap.sc.ids) do ids[id] = true end end
    for id in pairs(ids) do
        if id ~= my then
            local r = S.vitals_from_record(ipc and ipc.peer_rec("peer_vitals", id))
            if r then snap.vremote[id] = r end
        end
    end
    return snap
end

return S
