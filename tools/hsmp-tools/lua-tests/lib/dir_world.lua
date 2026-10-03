-- dir_world: an in-memory world for offline Director tests (HSMPMatch/Scripts/director.lua).
--
--   local DW = require("dir_world")
--   local D = DW.D                      -- the Director module
--   local w = DW.new({ sg = true })     -- IPC, clock, worlds, pawn, GI, census, save guard
--   w:sidecar_status("connected"); w:match("lobby", "Map_Arena_Pit", 0); w:start(); w:tick(4)
--
-- The session side is the typed native mock (lib/hsmp_native_mock.lua) behind a
-- real facade (mods/shared/hsmp_ipc.lua): the sidecar's `link` and `session`
-- records (N.sc_put), its heartbeat age (the header, ipc_info), the bus keys
-- (travel_request / travel_ack / spawn_status / spawn_request / director /
-- conn_state / return_to_lobby / ui_request) and the G2S records the Director
-- sends (command, game_status, leave). Real files stay files (w.files):
-- .gi_backup.txt, .settings.json and HSMPLoadout's kit status.

local DW = {}
DW.SD = "S"
local SD = DW.SD
DW.D = dofile(T.path("mods/HSMPMatch/Scripts/director.lua"))
local D = DW.D
local NM = require("hsmp_native_mock")
local S = NM.S
DW.S = S

local PHASE = { lobby = "LOBBY", loading = "LOADING", countdown = "COUNTDOWN", live = "LIVE", roundover = "ROUND_OVER",
                match_over = "MATCH_OVER", paused = "PAUSED", post_match = "POST_MATCH" }
local REASON = { [""] = "NONE", kill = "KILL", draw = "DRAW", forfeit = "FORFEIT", opponent_left = "OPPONENT_LEFT",
                 load_failed = "LOAD_FAILED" }
DW.NICKS = { [1] = "Me", [2] = "Bob", [3] = "Me" }

function DW.new(opts)
    opts = opts or {}
    local w = {
        clock = 100.0, wallt = 1790000000, files = {}, opens = {}, logs = {}, evs = {},
        world = { ok = true, short = "Map_Menu_Startup", key = "menu#1" }, nkey = 1,
        gi = { ["Player Just Died"] = true, ["Current Game Mode Enum"] = 3, ["Current Combat Mode"] = 2,
               ["Current Play Mode"] = 1, ["Free Mode Activated"] = true, ["Rounds To WIn"] = 5,
               ["Rounds Won"] = 2, ["Free Mode Foes Amount"] = 4, ["Free Mode Carnage"] = true,
               ["Free Mode Brawling"] = false, ["Free Mode Blossfechten"] = false },
        gi_stuck = {}, pawn = nil, npawn = 0, census_n = 0, frozen = nil, game_input = 0,
        cdo = { Health = 100, ["Head Health"] = 100, ["Neck Health"] = 100, ["Arm_R Health"] = 100,
                Consciousness = 100, Stamina = 100, Bleeding = 0 },
        open_ok = true, sidecar = nil, sg = opts.sg, beats = 0, sidecar_frozen = false,
        sg_seeds = 0, sg_forced = "unset", my_id = 1,
        -- the server's session snapshot as the harness builds it
        sess = { epoch = 0, match_id = 0, seq = 0, phase = "LOBBY", arena = "", round = 0, fighters = { 1, 2 },
                 extra = {}, plan = {} },
        reason = "", reason_code = 0, link_state = "UP", rx_age_ms = 0, attempt = 0,
        hb_at = nil, sent_seen = 0,
    }
    w.original_gi = {}
    for k, v in pairs(w.gi) do w.original_gi[k] = v end

    -- shared memory: the native mock + a real facade on the test clock
    local N = NM.new{}
    N._st.sidecar_state = "absent"
    N._st.hb_age = nil
    w.N = N
    local IPC = dofile(T.path("mods/shared/hsmp_ipc.lua"))
    IPC.init({ mod = "HSMPMatch", native = N, clock = function() return w.clock end, log = function() end, reinit = true })
    w.IPC = IPC

    local env = {}
    env.ipc = IPC
    env.now = function() return w.clock end
    env.wall = function() return w.wallt + math.floor(w.clock) end
    -- like the real reader (f:read("l")): nil for a missing file AND for an empty one
    env.read = function(p) local s = w.files[p]; if s == nil or s == "" then return nil end; return s:match("^([^\n]*)") end
    env.read_all = function(p) return w.files[p] end
    env.kit_status = function() return w.kit_status end   -- bus key kit_status (typed record)
    env.write_atomic = function(p, s) w.files[p] = s; return true end
    env.remove = function(p) w.files[p] = nil end
    env.world = function() return { ok = w.world.ok, short = w.world.ok and w.world.short or nil,
                                    key = w.world.ok and w.world.key or nil } end
    env.open_level = function(name)
        w.opens[#w.opens + 1] = name
        if not w.open_ok then return false end
        -- the native pre-hook runs inside OpenLevel (as in UE4SS)
        if w.dir then w.hook_result = w.dir:on_native_open_level(name) end
        w.pending = name
        return true
    end
    env.pawn = function() return w.pawn end
    env.pawn_id = function(p) return p.id end
    env.pawn_get = function(p, k) return p.props[k] end
    env.pawn_set = function(p, k, v) p.props[k] = v; return true end
    env.pawn_call = function(p, fn) p.calls[#p.calls + 1] = fn end
    env.pawn_loc = function(p) return p.x, p.y, p.z end
    env.cdo_vitals = function() return w.cdo end
    env.gi_get = function(k) return w.gi[k] end
    env.gi_set = function(k, v)
        if w.gi_stuck[k] then return false end
        w.gi[k] = v; return true
    end
    env.census = function(p) return w.census_n, "Willie_BP_C_" .. w.census_n end
    env.freeze = function(p, on) w.frozen = on and (p and p.id or true) or false end
    env.game_input = function() w.game_input = w.game_input + 1 end
    env.log = function(fmt, ...) w.logs[#w.logs + 1] = string.format(fmt, ...) end
    env.ev = function(n, f) w.evs[#w.evs + 1] = { n = n, f = f } end
    env.flush = function() IPC.flush() end
    if w.sg then
        w.sg_is_active = true
        env.sg_active = function() return w.sg_is_active end
        env.sg_force = function(v) w.sg_forced = v; if v ~= nil then w.sg_is_active = v end end
        env.sg_seed = function()
            w.sg_seeds = w.sg_seeds + 1
            w.seeded_gi = {}
            for k, v in pairs(w.gi) do w.seeded_gi[k] = v end
            return true, "HSMP_0_GameProgress"
        end
    end
    w.env = env

    -- ---- the sidecar: link record + heartbeat ----------------------------------------
    function w:put_link()
        if not self.sidecar then return end
        N.sc_put("link", { status = S.ENUMS.sidecar_status[self.sidecar:upper()], state = S.ENUMS.link_state[self.link_state],
            my_peer_id = self.my_id, is_admin = self.is_admin ~= false, reason = self.reason, reason_code = self.reason_code,
            rx_age_ms = self.rx_age_ms, attempt = self.attempt })
    end
    -- st: a sidecar status name, or nil = the session process is gone (detached).
    function w:sidecar_status(st)
        self.sidecar = st
        if st then
            if N._st.sidecar_state ~= "ready" then self.hb_at = self.clock end
            N._st.sidecar_state = "ready"
            N._st.hb_age = self.clock - (self.hb_at or self.clock) + 0.01
            self:put_link()
        else
            N._st.sidecar_state = "absent"
        end
        IPC.refresh_info(true)
    end
    -- the sidecar's heartbeat (header): beats every tick while it runs and is not frozen
    function w:beat()
        if self.sidecar and N._st.sidecar_state == "ready" then
            if not self.sidecar_frozen then
                self.beats = self.beats + 1
                self.hb_at = self.clock
            end
            N._st.hb_age = self.clock - (self.hb_at or self.clock) + 0.01
        else
            N._st.hb_age = nil
        end
    end
    -- the link's transport view: "up" with an rx age; above 3 s the link state is STALLED
    function w:link(state, rx_age_ms, extra)
        extra = extra or {}
        self.rx_age_ms = rx_age_ms or 0
        self.attempt = extra.attempt or 0
        self.link_state = (state == "up" and self.rx_age_ms > 3000) and "STALLED" or (state == "up" and "UP" or state:upper())
        self:put_link()
    end
    function w:kicked(reason) self.reason = reason end
    function w:rejected(reason) self.reason = reason end

    -- ---- the server's session snapshot -------------------------------------------------
    function w:put_session()
        local s = self.sess
        local rows = {}
        local dead = {}
        for _, id in ipairs(s.extra.dead or {}) do dead[id] = true end
        local waiting = {}
        for _, id in ipairs(s.extra.waiting or {}) do waiting[id] = true end
        local winner_seat = 255
        for i, id in ipairs(s.fighters) do
            local o = s.plan[id]
            rows[i] = { seat = i, peer_id = id, connected = true, alive = not dead[id], wins = 0, ready = false,
                        nick = DW.NICKS[id] or ("P" .. id), role = 0, waiting = waiting[id] or false,
                        spawn_id = o and o.spawn_id or 0, spawn_slot = o and o.slot or 0,
                        spawn_pos = o and { o.x, o.y, o.z } or { 0, 0, 0 }, spawn_yaw = o and o.yaw or 0 }
            if s.extra.winner == id then winner_seat = i end
        end
        local cd = s.extra.countdown or 3
        local deadline = (s.phase ~= "LOBBY" and s.phase ~= "LIVE") and (10000 + cd * 1000) or 0
        s.seq = s.seq + 1
        N.sc_put("session", {
            epoch = s.epoch, match_id = s.match_id, seq = s.seq, round = s.round, phase = S.ENUMS.phase[s.phase],
            phase_deadline_ms = deadline, server_time_ms = 10000, winner_seat = winner_seat,
            result_reason = S.ENUMS.result_reason[REASON[s.extra.reason or ""] or "NONE"],
            has_frozen = s.phase ~= "LOBBY",
            config = { arena = s.arena, best_of = 3, countdown_s = cd },
            frozen = { arena = s.arena, best_of = 3, countdown_s = cd },
            rows = rows,
        })
    end
    -- state: the legacy match-state vocabulary (lobby, countdown, live, ...); extra:
    -- { countdown, winner (peer id), reason ("kill", "draw" ...), dead = {ids}, waiting = {ids} }
    function w:match(state, arena, round, fighters, extra)
        local s = self.sess
        s.phase = PHASE[state] or "LOBBY"
        s.arena, s.round, s.fighters, s.extra = arena or "", round or 0, fighters or { 1, 2 }, extra or {}
        self:put_session()
    end
    -- the server instance / match id the following snapshots carry (republished now)
    function w:session(epoch, match_id)
        self.sess.epoch = tonumber(epoch) or 0
        if match_id ~= nil then self.sess.match_id = match_id end
        self:put_session()
    end
    -- the server's spawn plan for `round` (orders for peers 1 and 2)
    function w:spawns(round, arena, x, y, z)
        self.sess.plan = {
            [self.my_id] = { spawn_id = round * 256, slot = 2, x = x, y = y, z = z, yaw = 90 },
            [2] = { spawn_id = round * 256 + 1, slot = 5, x = 0, y = 0, z = 0, yaw = 0 },
        }
        if self.my_id ~= 1 then self.sess.plan[1] = nil end
        self:put_session()
    end

    -- ---- bus keys --------------------------------------------------------------------
    function w:request(seq, want, arena, reason)
        IPC.bus_put("travel_request", { want = want, arena = arena or "", reason = reason or "test", seq = seq,
            t = 1790000000, from = "HSMPMenu" })
    end
    function w:ui_request(seq, want, reason)
        IPC.bus_put("ui_request", { seq = seq, want = want, reason = reason or "", t = 1, from = "HSMPHud" })
    end
    -- HSMPSync's handshake: spawn_status for the current pawn; by default
    -- verified and the pawn really stands there.
    function w:placed(round, o)
        o = o or {}
        local arena = o.arena or "Map_Arena_Pit"
        local x, y, z = o.x or 100, o.y or 200, o.z or 98
        local pu = o.protect_until
        if pu == nil and o.verified ~= false and not o.unbounded then pu = self.clock + 3 end
        IPC.bus_put("spawn_status", { seq = o.seq or 1, round = round, arena = arena, spawn_id = round * 256, slot = 2,
            pawn = o.pawn or self.pawn.id, has_dest = true, pos = { x, y, z }, clear = true, why = o.why or "round start",
            verified = o.verified ~= false, tries = o.tries or 1, has_floor = true, floor = z - 100, protect_ms = 3000,
            has_protect_until = pu ~= nil, protect_until = pu or 0, t = self.clock, error = o.error or "", tol_cm = 100 })
        if not o.stay then self.pawn.x, self.pawn.y, self.pawn.z = x, y, z end
    end
    local function bus(key)
        local t = IPC.bus_table(key)
        return t
    end
    function w:ack() local a = bus("travel_ack"); return (a and a.seq ~= 0) and a or nil end
    -- conn_state with the unused action slots dropped (nil = never written)
    function w:conn()
        local c = bus("conn_state")
        if not c then return nil end
        local o = {}
        for k, v in pairs(c) do o[k] = v end
        o.actions = {}
        for _, a in ipairs(c.actions or {}) do if a ~= "" then o.actions[#o.actions + 1] = a end end
        return o
    end
    function w:director() return bus("director") end
    function w:spawn_request() return bus("spawn_request") end
    -- HSMPMenu's lobby hand-back counter (bus return_to_lobby seq; 0 = never)
    function w:rtl() local r = bus("return_to_lobby"); return r and r.seq or 0 end
    -- G2S records the Director sent, of `kind` (all of them, in order)
    function w:sent(kind)
        local out = {}
        for _, m in ipairs(N._rec.sends) do if m.kind == kind then out[#out + 1] = m.data end end
        return out
    end
    function w:left() return #self:sent("leave") > 0 end
    -- the game_status reports as "<round>:<dead 0|1>:<arena>:<load_error code>"
    function w:pings()
        local out = {}
        for _, g in ipairs(self:sent("game_status")) do
            out[#out + 1] = string.format("%d:%d:%s:%d", g.round, (g.flags & S.ENUMS.status_flag.DEAD) ~= 0 and 1 or 0,
                g.arena, g.load_error)
        end
        return out
    end
    function w:clear_pings() N._rec.sends = {} end

    -- ---- worlds ----------------------------------------------------------------------
    function w:load(name, opts2)
        opts2 = opts2 or {}
        name = name or self.pending
        self.pending = nil
        self.nkey = self.nkey + 1
        self.world = { ok = true, short = name, key = name .. "#" .. self.nkey }
        if name:match("^Map_Arena_") and not opts2.keep_gi then
            local src = self.seeded_gi or self.original_gi
            for k, v in pairs(src) do self.gi[k] = v end
            if not self.seeded_gi then self.gi["Player Just Died"] = true end
        end
        if name:match("^Map_Arena_") and not opts2.no_pawn then self:new_pawn() else self.pawn = nil end
    end
    -- complete whatever travel is pending (no-op when none)
    function w:settle() if self.pending then self:load() end end
    function w:new_pawn()
        self.npawn = self.npawn + 1
        self.pawn = { id = "Willie_BP_C_" .. (10 + self.npawn), calls = {}, x = 5000, y = 5000, z = 100,
            props = { Health = 50, ["Head Health"] = 100, ["Neck Health"] = 25, ["Arm_R Health"] = 25,
                      Consciousness = 100, Stamina = 80, Bleeding = 1 } }
    end
    function w:travelling() self.world.ok = false end
    -- one tick = 250 ms; `auto_load` completes every travel the next tick (the
    -- game loads the level), so oscillations show up as travel counts.
    function w:tick(n, dt)
        for _ = 1, (n or 1) do
            self.clock = self.clock + (dt or 0.25)
            if self.auto_load and self.pending then self:load() end
            self:beat()
            self.dir:tick()
        end
    end
    function w:evs_named(n)
        local out = {}
        for _, e in ipairs(self.evs) do if e.n == n then out[#out + 1] = e.f end end
        return out
    end
    function w:last_ev(n) local l = self:evs_named(n); return l[#l] end
    function w:logtext() return table.concat(self.logs, "\n") end
    function w:opens_to(name)
        local n = 0
        for _, o in ipairs(self.opens) do if o == name then n = n + 1 end end
        return n
    end
    -- The Director boots with the game, before any session: a snapshot the
    -- test wrote first is republished after construction unless `leftover`
    -- (a snapshot present at boot is an earlier session's).
    function w:start(get_session, leftover)
        self.dir = D.new(self.env, { state_dir = SD, get_session = get_session })
        if not leftover and self.sess.seq > 0 then self:put_session() end
        self:beat()
        return self.dir
    end
    return w
end

-- Drive a fresh world to Ready in Map_Arena_Pit (round 1). Returns w.
function DW.to_ready(opts)
    opts = opts or {}
    local w = DW.new(opts)
    if opts.setup then opts.setup(w) end   -- extra env functions before the Director starts
    w:sidecar_status("connected")
    w:match("lobby", "Map_Arena_Pit", 0)
    if opts.epoch then w:session(opts.epoch) end
    w:start()
    w:tick(4)
    w:match("countdown", "Map_Arena_Pit", 0)
    w:spawns(1, "Map_Arena_Pit", 100, 200, 10)
    w:tick(1)
    w:load()
    w:tick(2)
    w:placed(1)
    w.kit_status = { pawn = w.pawn.id, ok = true, armour_n = 5, r_class = "Sword", l_class = "Shield", rev = 1 }
    w.census_n = 1
    w:tick(8)
    return w
end

-- to_ready, then the round goes live.
function DW.to_live(opts)
    local w = DW.to_ready(opts)
    w:match("live", "Map_Arena_Pit", 1)
    w:tick(2)
    return w
end

return DW
