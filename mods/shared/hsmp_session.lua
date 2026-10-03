-- hsmp_session.lua -- the session, match and connection state of the HSMP Lua mods,
-- read from the typed records the sidecar copies into shared memory:
--   slot `link`     the sidecar's connection view (status, my peer id, admin, link state,
--                   kick / reject reason, transport metrics)
--   slot `session`  the server's session snapshot as received (config, frozen config,
--                   roster rows with wins / alive / ready / waiting / spawn order, result)
--   slot `admin`    who is admin (+ masked bans for an admin)
-- and the single "is an MP session live?" predicate every mod uses.
--
-- build-and-deploy.ps1 copies shared/*.lua into every mod's Scripts/; do not edit the
-- per-mod copies. Pure Lua over the IPC facade (HSMP_IPC), no UObjects, game thread only.
--
-- Liveness: a session is LIVE when the link says CONNECTED and the sidecar's heartbeat in
-- the segment header is fresh (IPC.refresh_info().sidecar_hb_age_s <= FRESH_S while the
-- sidecar is attached). A crashed / killed / hung sidecar stops beating, so its last
-- "connected" never counts (no line-change heuristics: the records change on change only).
--
--   local HS = require("hsmp_session")
--   local S = HS.new{}                 -- opts: ipc (facade; default HSMP_IPC), clock, fresh_s, every_s
--   S:poll()                           -- at most every opts.every_s (default 0.25 s)
--   if S:live() then ... end
--   S.status (name: "connected", "kicked", ...), S.peer_id, S.exists, S.id_changed, S.link
--   local v = HS.view()                -- the normalised session / match view (below), or nil
--   HS.link(), HS.session(), HS.admin(), HS.is_admin(), HS.my_peer_id()

local M = { VERSION = 2, FRESH_S = 5.0 }

-- Terminal sidecar statuses (names): never live, never reconnect by themselves.
M.TERMINAL = { ended = true, stopped = true, exited = true, disconnected = true, closed = true,
               kicked = true, replaced = true, server_closed = true, rejected = true }

local function ipc_of(o)
    return (o and o.ipc) or rawget(_G, "HSMP_IPC")
end

local function schema(ipc)
    return (ipc and ipc.S) or nil
end

-- Code -> lower-case name from a generated code table (S.ENUM_NAMES.<t>), cached.
local name_cache = {}
local function code_name(ipc, tbl, code)
    local S = schema(ipc)
    local names = S and S.ENUM_NAMES and S.ENUM_NAMES[tbl]
    local n = names and names[code]
    if not n then return nil end
    local c = name_cache[n]
    if not c then c = n:lower(); name_cache[n] = c end
    return c
end
M.code_name = code_name

-- Legacy v4 match-state names by phase (the readers' "state" vocabulary).
M.STATE_OF_PHASE = { [0] = "lobby", [1] = "countdown", [2] = "countdown", [3] = "live", [4] = "roundover",
                     [5] = "match_over", [6] = "lobby", [7] = "paused" }
-- Phase names (S2CSession vocabulary; the Director's).
M.PHASE_NAME = { [0] = "lobby", [1] = "loading", [2] = "countdown", [3] = "live", [4] = "roundover",
                 [5] = "match_over", [6] = "post_match", [7] = "paused" }
-- Legacy v4 result reason strings by result_reason code.
M.REASON_OF_RESULT = { [0] = "", [1] = "", [2] = "draw", [3] = "forfeit", [4] = "opponent_left",
                       [5] = "time_limit", [6] = "", [7] = "load_failed" }

-- ---- raw records ------------------------------------------------------------------
function M.link(o)
    local ipc = ipc_of(o)
    if not ipc or not ipc.rec then return nil end
    return (ipc.rec("link"))
end
function M.session(o)
    local ipc = ipc_of(o)
    if not ipc or not ipc.rec then return nil end
    return ipc.rec("session")
end
function M.admin(o)
    local ipc = ipc_of(o)
    if not ipc or not ipc.rec then return nil end
    return (ipc.rec("admin"))
end
function M.my_peer_id(o)
    local l = M.link(o)
    return l and l.my_peer_id or 0
end
function M.is_admin(o)
    local l = M.link(o)
    return l ~= nil and l.is_admin == true
end
-- The sidecar status name of a link ("connecting", "connected", ..., "ended"); nil = none.
function M.status_name(link, o)
    if type(link) ~= "table" then return nil end
    return code_name(ipc_of(o), "sidecar_status", link.status)
end
-- The link state name ("up", "stalled", "reconnecting", "terminal", "connecting").
function M.link_state_name(link, o)
    if type(link) ~= "table" then return nil end
    return code_name(ipc_of(o), "link_state", link.state)
end

-- Seconds since the sidecar's last header heartbeat (nil = unknown / not attached).
function M.hb_age(o)
    local ipc = ipc_of(o)
    if not ipc or not ipc.refresh_info then return nil end
    local info = ipc.refresh_info(false)
    if type(info) ~= "table" then return nil end
    local st = info.sidecar_state
    if st ~= nil and st ~= "ready" and st ~= 2 then return nil end
    return tonumber(info.sidecar_hb_age_s)
end

-- ---- the normalised view (cached per (session, link) version per Lua state) ---------
-- v = {
--   epoch (integer), seq, match_id (integer; 0 in the lobby), phase (code), phase_name,
--   state (legacy match state: lobby | countdown | live | roundover | match_over | paused),
--   round, pending_round, arena (effective: frozen in a match), config, frozen (or nil),
--   best_of (effective), countdown_s, deadline_in_ms (nil = no deadline), recv_clock,
--   rows (roster rows as received), by_peer {[peer_id] = row}, by_seat {[seat] = row},
--   my_peer_id, my_seat (nil = none), me (my row), remote_fighters,
--   scoreboard {{id, wins, alive}} (connected seats, roster order), wins {[id]}, alive {[id]},
--   ready {ids}, waiting_on {ids}, last_winner (peer id; 0 = none), winner_seat (nil = none),
--   reason (legacy string), result_reason (code),
--   spawn_round, spawns {[peer_id] = {peer, spawn_id, slot, x, y, z, yaw, protect_ms}},
--   is_admin, status (sidecar status name), link (the link record) }
local vc = { sver = nil, lver = nil, v = nil }

local function build(sess, link, clock_now, o)
    local ipc = ipc_of(o)
    local v = { rows = sess.rows or {}, by_peer = {}, by_seat = {}, scoreboard = {}, wins = {}, alive = {},
                ready = {}, waiting_on = {}, spawns = {} }
    v.epoch, v.seq, v.match_id, v.round = sess.epoch, sess.seq, sess.match_id, sess.round
    v.phase = sess.phase
    v.phase_name = M.PHASE_NAME[sess.phase] or "unknown"
    v.state = M.STATE_OF_PHASE[sess.phase] or "lobby"
    v.pending_round = (sess.phase == 1 or sess.phase == 2) and sess.round + 1 or sess.round
    v.config = sess.config
    v.frozen = sess.has_frozen and sess.frozen or nil
    local eff = (v.frozen and sess.phase ~= 0) and v.frozen or v.config or {}
    v.arena = (eff.arena ~= nil and eff.arena ~= "") and eff.arena or nil
    v.best_of = eff.best_of
    v.recv_clock = clock_now
    if (sess.phase_deadline_ms or 0) ~= 0 then
        v.deadline_in_ms = math.max(0, sess.phase_deadline_ms - (sess.server_time_ms or 0))
    end
    v.result_reason = sess.result_reason
    v.reason = M.REASON_OF_RESULT[sess.result_reason] or ""
    v.my_peer_id = link and link.my_peer_id or 0
    v.is_admin = link ~= nil and link.is_admin == true
    v.status = link and M.status_name(link, o) or nil
    v.link = link
    local remote = 0
    for _, r in ipairs(v.rows) do
        v.by_seat[r.seat] = r
        if r.connected and r.peer_id ~= 0 then
            local id = r.peer_id
            v.by_peer[id] = r
            v.scoreboard[#v.scoreboard + 1] = { id, r.wins, r.alive and 1 or 0 }
            v.wins[id], v.alive[id] = r.wins, r.alive
            if r.ready then v.ready[#v.ready + 1] = id end
            if r.waiting then v.waiting_on[#v.waiting_on + 1] = id end
            if r.spawn_id ~= 0 then
                v.spawns[id] = { peer = id, spawn_id = r.spawn_id, slot = r.spawn_slot, x = r.spawn_pos[1],
                                 y = r.spawn_pos[2], z = r.spawn_pos[3], yaw = r.spawn_yaw, protect_ms = r.spawn_protect_ms }
                v.spawn_round = v.spawn_round or (r.spawn_id >> 8)
            end
            if id == v.my_peer_id and v.my_peer_id ~= 0 then v.my_seat, v.me = r.seat, r end
        end
    end
    for _, r in ipairs(v.rows) do
        if r.role == 0 and r.seat ~= v.my_seat then remote = remote + 1 end   -- FIGHTER, not me
    end
    v.remote_fighters = remote
    v.spawn_round = v.spawn_round or 0
    if sess.winner_seat ~= 255 then
        v.winner_seat = sess.winner_seat
        local w = v.by_seat[sess.winner_seat]
        v.last_winner = w and w.peer_id or 0
    else
        v.last_winner = 0
    end
    local _ = ipc
    return v
end

-- The current view (nil = no session snapshot). countdown_s / deadline_in_ms are refreshed
-- on every call from the receipt time.
function M.view(o)
    local ipc = ipc_of(o)
    if not ipc or not ipc.rec then return nil end
    local sess, sver = ipc.rec("session")
    if type(sess) ~= "table" then vc.sver, vc.v = nil, nil; return nil end
    local link, lver = ipc.rec("link")
    local clock = (o and o.clock) or os.clock
    if vc.v == nil or vc.ipc ~= ipc or sver ~= vc.sver or lver ~= vc.lver then
        local recv = (vc.ipc == ipc and sver == vc.sver and vc.v) and vc.v.recv_clock or clock()
        vc.v = build(sess, link, recv, o)
        vc.sver, vc.lver, vc.ipc = sver, lver, ipc
    end
    local v = vc.v
    local d = sess.phase_deadline_ms or 0
    if d ~= 0 then
        local left = d - (sess.server_time_ms or 0) - (clock() - v.recv_clock) * 1000
        v.deadline_in_ms = math.max(0, math.floor(left))
    else
        v.deadline_in_ms = nil
    end
    if v.phase == 1 then   -- LOADING: the frozen countdown (v4 showed it frozen)
        v.countdown_s = (v.frozen or v.config or {}).countdown_s or 0
    else
        v.countdown_s = v.deadline_in_ms and math.ceil(v.deadline_in_ms / 1000) or 0
    end
    return v
end
-- Forget the cached view (tests, world changes).
function M.reset_view() vc.sver, vc.lver, vc.v = nil, nil, nil end

-- ---- notices (S2G `notice` records) -------------------------------------------------
M.NOTICE_NAME = { [1] = "host_left", [2] = "role_changed", [3] = "load_failed", [4] = "player_joined",
                  [5] = "player_left", [6] = "admin_changed", [7] = "config_queued", [8] = "sudden_death" }
-- The player-facing text of a notice record (the sidecar's old notice_json wording).
function M.notice_text(n)
    local a = n.args or {}
    local function arg(i) return a[i] or "" end
    local c = n.code
    if c == 3 then return string.format("%s failed to load: %s", arg(1), arg(2))
    elseif c == 1 and arg(2) ~= "" then return string.format("The host %s left; %s is the host now", arg(1), arg(2))
    elseif c == 1 then return string.format("The host %s left", arg(1))
    elseif c == 4 then return string.format("%s %s", arg(1), arg(2) == "rejoined" and "rejoined" or "joined")
    elseif c == 5 then return string.format("%s %s", arg(1), arg(2) == "left" and "left" or "disconnected") end
    local parts = {}
    for i = 1, #a do if a[i] ~= "" then parts[#parts + 1] = a[i] end end
    return table.concat(parts, " ")
end
function M.notice_name(n) return M.NOTICE_NAME[n.code] or "notice" end

-- ---- the liveness tracker -----------------------------------------------------------
-- opts: ipc, clock (function -> s), fresh_s, every_s. (state_dir / path are ignored: kept so
-- old call sites keep working.)
function M.new(opts)
    opts = opts or {}
    local clock = opts.clock or function() return os.clock() end
    local S = {
        fresh_s = opts.fresh_s or M.FRESH_S, every_s = opts.every_s or 0.25,
        status = nil, peer_id = 0, exists = false, link = nil, link_ver = nil,
        changed_at = -math.huge, read_at = -math.huge, id_changed = nil, reads = 0,
    }
    function S:poll(force)
        local now = clock()
        if not force and now - self.read_at < self.every_s then return self end
        self.read_at = now
        self.reads = self.reads + 1
        local ipc = ipc_of(opts)
        local l, ver = nil, nil
        if ipc and ipc.rec then l, ver = ipc.rec("link") end
        if type(l) ~= "table" then
            self.exists, self.status, self.link, self.link_ver = false, nil, nil, nil
            return self
        end
        self.exists = true
        if ver ~= self.link_ver then self.link_ver, self.changed_at = ver, now end
        self.link = l
        self.status = M.status_name(l, opts)
        local id = l.my_peer_id or 0
        if id > 0 then
            if self.peer_id > 0 and id ~= self.peer_id then self.id_changed = { from = self.peer_id, to = id } end
            self.peer_id = id
        end
        return self
    end
    -- The sidecar is beating (header heartbeat within fresh_s, sidecar attached).
    function S:fresh()
        if not self.exists then return false end
        local age = M.hb_age(opts)
        return age ~= nil and age <= self.fresh_s
    end
    -- Seconds since the link record last changed (0 when absent).
    function S:quiet_s(now)
        if not self.exists then return 0 end
        return (now or clock()) - self.changed_at
    end
    -- THE predicate: connected AND a fresh heartbeat.
    function S:live() return self.status == "connected" and self:fresh() end
    function S:terminal() return self.exists and self.status ~= nil and M.TERMINAL[self.status] == true end
    return S
end

return M
