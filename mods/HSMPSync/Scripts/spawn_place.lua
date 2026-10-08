-- HSMPSync / spawn_place.lua -- server-ordered spawn placement.
--
-- One module owns everything that moves the LOCAL pawn on a spawn:
--   * placement on the server's order (the session record's roster,
--     docs/development/subsystems/spawns.md):
--     ground snap, capsule clearance, keep-away from other Willies, a small
--     spiral of offsets when the point is blocked;
--   * a teleport that moves the WHOLE body (actor + every physics mesh), then
--     VERIFIES that it stuck (re-tried up to T.max_tries);
--   * the handshake with the Director (docs/development/subsystems/director.md 3.2):
--       bus `spawn_status`           written after every placement attempt and
--                                    every verification (round start, fall
--                                    rescue, Director retry);
--       bus `spawn_request`          read: the Director asks for a re-place;
--   * spawn protection: from the moment the pawn exists until placement +
--     protect_ms the pawn is "Invulnerable" (Willie_BP "Get Damage" returns
--     at once when it is set: bytecode, spawns.md 1.6), its vitals are
--     topped up from the CDO and no death is reported. Before Live only,
--     "Block Spine Breaking" guards native dislocation checks during
--     placement; its original value is restored on exit;
--   * the fall watchdog: a LIVING pawn below the floor is put back on its
--     order WITHOUT dying (before Live: re-placed, healed, protection
--     re-armed; during Live: re-placed only). A dead pawn is never moved.
--
-- Why the teleport is this involved: the visible ragdoll (Mesh, 22 simulated
-- bodies) is not rigidly attached to the capsule (HSMPAvatars drive_root:
-- "moving the capsule leaves the bodies behind"). Moving only the actor
-- leaves the body on the native spawner and the capsule follows it back
-- (3-13 m from the order, spawn_timeout). Toggling physics simulation off and
-- on around the move is worse: forcing physics ON on a component that did not
-- simulate before drops a copy skeleton through the floor. So the whole body
-- is moved and the result is verified.
--
-- This file is pure logic over an `env` table so it runs offline
-- (`hsmp-tools lua-test spawn_place`); make_ue_env() builds the real one.
-- Threading: every entry point runs on the game thread (HSMPSync on_tick).

local SP = { VERSION = 2 }

SP.T = {
    settle_s         = 1.5,    -- world key + possessed pawn stable before any trace/teleport
    plan_wait_s      = 20,     -- log once when no order arrives for this world
    verify_after_s   = 0.4,    -- first "did it stick" check after a teleport
    verify_hold_s    = 0.4,    -- ... and a second one this much later
    verify_tol_cm    = 100,    -- XY distance from the destination that counts as placed
                               -- (= the Director's place_tol_cm = the gate's placement limit;
                               -- spawn_verified.tol_cm carries it to the gate)
    verify_dz_cm     = 250,    -- and |dZ|
    verify_body_cm   = 250,    -- and the visible body (Mesh pelvis) this close in XY
    max_tries        = 3,      -- teleports per placement before reporting a failure
    hold_s           = 0.9,    -- after a teleport: pin the body on the destination this long
    hold_tol_cm      = 10,     -- ... moving it back whenever it drifted more than this (XY)
    flat_r           = 35,     -- level-ground probe radius around a destination (foot spread)
    flat_dz          = 12,     -- ... max floor height spread there (a step / ramp is not a spawn spot)
    min_pawn_dist    = 150,    -- cm to any other Willie / other player's planned spawn
    max_dz           = 150,    -- an offset point may not change the floor height more
    rings            = { 75, 150, 225 },
    pawn_half_height = 100,    -- actor origin above the floor
    protect_min_ms   = 3000,   -- client floor of the protection window after placement
    protect_max_ms   = 15000,  -- clamp for a server value
    fall_margin_cm   = 1500,   -- void: this far below the lowest known floor
    fall_protect_cm  = 400,    -- while protected: this far below the placed floor = fell
    drift_cm         = 60,     -- while protected and not live: pushed this far = re-place (under the 100 cm placement limit; a pawn can drift 2 m before Ready)
    watch_s          = 1.0,    -- fall watchdog cadence (0.25 s while protected)
    watch_protect_s  = 0.25,
    fall_cooldown_s  = 2.0,
    vitals_every_s   = 0.5,
    request_every_s  = 0.25,   -- bus `spawn_request` poll
    reports          = 3,      -- spawned:<...> to the server, ~1 s apart
    -- No collision between the local pawn and any other Willie (stand-ins)
    -- while protected (idle pawns knock each other down before the round
    -- starts, and Half Sword drops held weapons on a knockdown).
    nocollide_every_s = 0.5,   -- pair scan cadence while protected
    overlap_cm       = 150,    -- re-enable waits while a stand-in is this close (XY) ...
    overlap_grace_s  = 5.0,    -- ... for at most this long after protection ended
    state_every_s    = 5.0,    -- pawn_state event cadence while protected
    director_every_s = 0.5,    -- bus `director` poll (pawn_state at Ready)
}

-- Vitals restored from the Willie_BP CDO while protected (same list as the
-- Director's career-wound reset).
SP.VITALS_FIELDS = {
    "Health", "Head Health", "Neck Health", "Body Upper Health", "Body Lower Health",
    "Back Health", "Arm_R Health", "Arm_L Health", "Leg_R Health", "Leg_L Health",
    "Head Health (Crush)", "Consciousness", "Consciousness 2 (Legs)",
    "Bleeding", "Pain",
}

local function dist_xy(ax, ay, bx, by) return math.sqrt((ax - bx) ^ 2 + (ay - by) ^ 2) end

-- --- the server's spawn plan (the session record's roster rows) ---------------------------

-- The plan from the normalised session view (shared/hsmp_session.lua HS.view()):
-- { round, arena, seq, by_peer = {[peer] = {peer, spawn_id, slot, x, y, z, yaw, protect_ms}}, list }.
-- round = the round the orders are for (spawn_id >> 8); nil without any order.
function SP.plan_from_view(v, mode)
    if type(v) ~= "table" or type(v.spawns) ~= "table" then return nil end
    local p = { match_id = v.match_id or 0, round = tonumber(v.spawn_round) or 0, arena = v.arena or "", seq = tonumber(v.seq) or 0,
                by_peer = {}, list = {} }
    local ids = {}
    for id in pairs(v.spawns) do ids[#ids + 1] = id end
    table.sort(ids)
    for _, id in ipairs(ids) do
        local s = v.spawns[id]
        local e = { peer = id, spawn_id = s.spawn_id, slot = s.slot or 0, x = s.x, y = s.y, z = s.z,
                    yaw = s.yaw or 0, protect_ms = s.protect_ms or 0 }
        -- Snapshot the generation belonging to THIS order. Initial round orders
        -- precede Mode.on_live (which initializes life1); respawns require the
        -- matching full authoritative generation, not only the wrapped low bits.
        local sid = math.tointeger(e.spawn_id) or 0
        e.life = 1
        if sid & 0x80 ~= 0 then
            local row = mode and mode.rows and mode.rows[id]
            local life = row and math.tointeger(row.life) or 0
            e.life = mode and mode.match_id == p.match_id and mode.round == p.round
                and life > 0 and (life & 0x7f) == (sid & 0x7f) and life or 0
        end
        p.by_peer[id] = e
        p.list[#p.list + 1] = e
    end
    if #p.list == 0 then return nil end
    return p
end

-- The round a plan must carry for the current match phase: countdown/paused
-- announce round+1; live/roundover play round.
function SP.wanted_round(state, round)
    round = tonumber(round) or 0
    if state == "countdown" or state == "paused" then return round + 1 end
    return round
end

-- Own peer id from the sidecar's `link` record, re-read on every use
-- (throttled), never latched: the server never reuses peer ids and the sidecar
-- publishes the new one on every Welcome (reconnect, server restart). No link
-- record, or a terminal status, means status "none" / the terminal name and NO id.
-- Third result: the session liveness predicate (the sidecar's header
-- heartbeat, never "the record changed recently"): status connected or
-- reconnecting AND the sidecar beating (shared/hsmp_session.lua S:fresh()).
-- poll() -> status name | nil, peer id, fresh (bool); now() -> s.
-- Returns get(force) -> id, status, live.
SP.TERMINAL = { kicked = true, replaced = true, server_closed = true, rejected = true, ended = true }
function SP.peer_reader(poll, now, every_s, log)
    local st = { id = 0, status = nil, at = -1e9, fresh = false }
    every_s = every_s or 0.5
    local function set_id(p, s)
        if p ~= st.id and log then log("own peer id %d -> %d (sidecar status %s)", st.id, p, tostring(s)) end
        st.id, st.status = p, s
    end
    local function refresh(force)
        local t = now()
        if not force and t - st.at < every_s then return end
        st.at = t
        local s, p, fresh = poll()
        st.fresh = fresh and true or false
        if s == nil then set_id(0, "none"); return end
        p = tonumber(p) or 0
        if p < 0 or SP.TERMINAL[s] then p = 0 end
        set_id(p, s)
    end
    return function(force)
        pcall(refresh, force)
        local live = st.fresh and (st.status == "connected" or st.status == "reconnecting")
        return st.id, st.status, live and true or false
    end
end

-- The match state (legacy name: lobby / countdown / live / roundover / match_over /
-- paused) and round from the session view. view() -> HS.view() | nil.
-- Returns get() -> state|nil, round.
function SP.match_reader(view)
    return function()
        local ok, v = pcall(view)
        if not ok or type(v) ~= "table" then return nil, 0 end
        return v.state, tonumber(v.round) or 0
    end
end

-- Protection window (ms) for an order: the server's protect_ms, never below
-- the client floor (a fresh pawn needs a moment to settle), clamped.

function SP.protect_ms(order)
    local v = tonumber(order and order.protect_ms) or 0
    return math.max(SP.T.protect_min_ms, math.min(v, SP.T.protect_max_ms))
end

-- --- the placer --------------------------------------------------------------------------
--
-- env (all functions; make_ue_env below, the mock in lua-tests/spawn_place.lua):
--   now() -> s (os.clock: one clock per process, shared with the Director)
--   log(fmt, ...)
--   read(path) -> first line | nil     (only the fallback swap bus key, HSMPAvatars')
--   plan() -> SP.plan_from_view(HS.view()) | nil     request() -> bus `spawn_request` | nil
--   put_status(t) / clear_status()     bus `spawn_status` (typed record)
--   director_state() -> bus `director` state | nil   send_spawned(t) -> id | nil (G2S `spawned`)
--   world() -> { key=, short= }        (called only while the world guard passed)
--   my_peer_id() -> n                  match() -> state, round
--   pawn() -> p | nil   pawn_id(p) -> FName string   pawn_begun(p) -> bool
--   pawn_loc(p) -> x, y, z             body_loc(p) -> x, y, z | nil,nil,nil,error (Mesh pelvis; optional)
--   ground(p, x, y, z) -> floor_z | nil          (downward trace)
--   clear(p, x, y, floor_z) -> bool              (capsule sweep)
--   others(p) -> { {x,y,z}, ... }                (other Willies)
--   teleport(p, {X,Y,Z}, yaw) -> detail string   (moves actor + physics meshes)
--   prop_get(p, k)  prop_set(p, k, v)
--   cdo_vitals() -> { field = number } | nil
--   native_floor() -> z | nil   native_point() -> x, y, z, yaw | nil
--   (optional) willies(p) -> { {key=, actor=}, ... }   every other Willie (stand-ins, foes)
--   (optional) set_collision(p, other, enabled) -> ok, detail
--                         physics collision between the two pawns' bodies AND
--                         their held weapons (CollisionDisabler plugin), plus
--                         the capsules' move-ignore, in both directions
--   (optional) pawn_state(p) -> { consciousness=, downed=, fallen=, health=, weapon_r=, weapon_l= }
--   (optional) ev(name, fields)   structured event (shared/hsmp_log.lua)
--   (optional) session_live() -> bool    the MP session liveness predicate (no order without it)
--   (optional) connected() -> bool       gate for the `spawned` report
-- opts: state_dir

local P = {}
P.__index = P

function SP.new(env, opts)
    local self = setmetatable({}, P)
    self.env = env
    self.dir = (opts and opts.state_dir) or "hsmp_state"
    self.status_seq = 0
    self.drift_attempts = 0 -- whole process budget; deliberately survives reset/travel
    -- Last handled spawn request seq (bus `spawn_request`); a request left over
    -- from an earlier run is never acted on.
    local r = env.request and env.request()
    self.req_seq = (r and tonumber(r.seq)) or 0
    -- A spawn_status left by an earlier process carries protect_until on
    -- THAT process's os.clock (far future here) and an old pawn name: the
    -- Director, HSMPLoadout and HSMPInteract would read it as protection.
    -- This process has placed nothing yet, so there is no valid status.
    if env.clear_status then pcall(env.clear_status) end   -- the bus key, cleared (seq 0)
    self.counters = { placed = 0, verified = 0, failed = 0, retries = 0, falls = 0,
                      invuln_set = 0, vitals = 0, requests = 0, nocollide = 0, recollide = 0, states = 0,
                      corrections_live = 0 }
    self:reset("init")
    return self
end

-- World changed / dropped: forget the pawn and every world-scoped value
-- (nothing here holds a UObject beyond one tick except `pawn`, which is only
-- compared by identity and re-fetched every tick).
function P:reset(why)
    self.wkey, self.wkey_since = nil, 0
    self.pawn, self.pawn_id, self.pawn_since = nil, nil, 0
    self.cur = nil                   -- current placement: { e, plan, dest, floor, tries, ... }
    self.placed_pawn = nil           -- pawn id the current world's placement belongs to
    self.protect = nil               -- { until_t (nil = until placed), set_by_us, ms }
    self.dislocation_guard = nil     -- original native bool + exact pawn/world/life; forget on drop
    self.no_order_logged = false
    self.world_t0 = nil
    self.next_watch, self.next_vitals, self.next_req = 0, 0, 0
    self.last_fall = -1e9
    self.report = nil
    -- The round went Live in this world (protection then ends at until_t;
    -- before that it holds whatever until_t says).
    self.live_seen, self.live_at = false, nil
    self.respawn_spawn = false
    -- Pairs whose collision we disabled: key -> { actor, t } (UObjects of
    -- this world only; dropped with it, never touched after a world change).
    self.nc = { pairs = {}, next_t = 0, ended_t = nil, ok = nil }
    self.next_state, self.next_dir, self.ready_logged = 0, 0, false
end


-- This client's order for the loaded world, or nil, why.
function P:order()
    local env = self.env
    -- Only for a live MP session (the liveness predicate, see
    -- SP.peer_reader): stale MP state never places a single-player pawn.
    if env.session_live and not env.session_live() then return nil, nil, "no live MP session" end
    local me = tonumber(env.my_peer_id()) or 0
    if me == 0 then return nil, nil, "no peer id yet" end
    local plan = env.plan()
    if not plan or plan.round == 0 then return nil, nil, "no spawn plan" end
    local st, round = env.match()
    local want = SP.wanted_round(st, round)
    if plan.round ~= want then
        return nil, nil, string.format("plan is for round %d, match %s round %d", plan.round, tostring(st), tonumber(round) or 0)
    end
    local short = self.wshort or ""
    if short ~= plan.arena then return nil, nil, "plan is for " .. plan.arena .. ", loaded " .. short end
    local e = plan.by_peer[me]
    if not e then return nil, nil, "no seat for peer " .. me end
    if (plan.match_id or 0) ~= 0 and (e.life or 0) == 0 then return nil, nil, "waiting for spawn life" end
    return e, plan
end

function P:order_text()
    local e, plan, why = self:order()
    if not e then return "none (" .. tostring(why) .. ")" end
    return string.format("round %d slot %d (%.0f, %.0f, %.0f) yaw %.0f id %d", plan.round, e.slot, e.x, e.y, e.z, e.yaw,
        e.spawn_id or -1)
end

-- A server deathmatch life order is distinct from an incidental possession
-- swap during combat. Only the Director's actual Spawn phase authorizes it.
function P:respawn_order(e)
    local id = e and math.tointeger(e.spawn_id) or 0
    return id ~= nil and id & 0x80 ~= 0 and self.env.director_state
        and self.env.director_state() == "Spawn" or false
end

-- Is the local pawn under spawn protection right now? From the first tick an
-- MP pawn exists until the round goes Live (never into Live; the client
-- floor protect_ms only bounds a window before Live, e.g. a failed
-- placement or no order). Once the round is Live nobody is invulnerable.
function P:protected(now)
    local pr = self.protect
    if not pr then return false end
    if self.live_seen and not pr.respawn then return false end
    now = now or self.env.now()
    if pr.until_t == nil or now < pr.until_t then return true end
    return not pr.respawn and pr.until_live == true
end

-- Absolute end of protection (process clock), nil while unbounded (not
-- verified yet, or held until the round goes Live). From Live on: the Live
-- transition (or the earlier bounded end).
function P:protect_until()
    local pr = self.protect
    if not pr then return nil end
    if pr.respawn then return pr.until_t end
    if self.live_seen then
        local la = self.live_at or self.env.now()
        return (pr.until_t and pr.until_t < la) and pr.until_t or la
    end
    if pr.until_t == nil or pr.until_live then return nil end
    return pr.until_t
end

-- --- status (bus `spawn_status`: the Director's evidence) ------------------------------------

function P:write_status(c, verified, err)
    local env = self.env
    self.status_seq = self.status_seq + 1
    local d = c.dest
    local pu = self:protect_until()
    env.put_status({
        seq = self.status_seq, match_id = c.plan.match_id or 0, life = c.e.life or 0, round = c.plan.round, arena = c.plan.arena or "",
        spawn_id = tonumber(c.e.spawn_id) or 0, slot = tonumber(c.e.slot) or -1, pawn = self.pawn_id or "",
        has_dest = d ~= nil, pos = d and { d.X, d.Y, d.Z } or nil, clear = c.clear and true or false,
        why = c.why or "", verified = verified and true or false, tries = c.tries or 0,
        has_floor = tonumber(c.floor) ~= nil, floor = tonumber(c.floor) or 0, protect_ms = SP.protect_ms(c.e),
        has_protect_until = pu ~= nil, protect_until = pu or 0, t = env.now(), error = err or "",
        tol_cm = SP.T.verify_tol_cm,
    })
end

-- --- choosing the destination --------------------------------------------------------------

-- Pick the destination for order e: the server point if it is clear, else the
-- first clear offset of the spiral. Returns dest{X,Y,Z}, floor_z, clear, offset|nil
-- or nil (no ground at all under the server point).
function P:choose(pawn, e, plan)
    local env, T = self.env, SP.T
    local me = tonumber(env.my_peer_id()) or 0
    local keep = {}
    for _, o in ipairs(env.others(pawn) or {}) do keep[#keep + 1] = o end
    for _, o in ipairs(plan.list or {}) do
        if o.peer ~= me then keep[#keep + 1] = { o.x, o.y, o.z + T.pawn_half_height } end
    end
    local function far(x, y, z)
        for _, p in ipairs(keep) do
            if dist_xy(p[1], p[2], x, y) < T.min_pawn_dist and math.abs(p[3] - z) < 200 then return false end
        end
        return true
    end
    local base_floor = env.ground(pawn, e.x, e.y, e.z)
    if not base_floor then return nil end
    -- Level ground under the feet: the floor heights at the point and at
    -- flat_r around it may differ by at most flat_dz. Alley sp1 (815,280) sits on
    -- a ramp/step: a pawn verified there walks/slides 1-2 m off it before Live,
    -- while pawns on flat slots stay within 10 cm.
    local flat_logged = false
    local function level(x, y, fz)
        local lo, hi = fz, fz
        for k = 0, 3 do
            local a = k * math.pi / 2 + math.pi / 4
            local z2 = env.ground(pawn, x + math.cos(a) * T.flat_r, y + math.sin(a) * T.flat_r, e.z)
            if not z2 then return false, math.huge end
            if z2 < lo then lo = z2 end
            if z2 > hi then hi = z2 end
        end
        return hi - lo <= T.flat_dz, hi - lo
    end
    local function ok_at(x, y, is_offset)
        local fz = env.ground(pawn, x, y, e.z)
        if not fz then return nil end
        if is_offset and math.abs(fz - base_floor) > T.max_dz then return nil end
        local flat, spread = level(x, y, fz)
        if not flat then
            if not flat_logged then
                flat_logged = true
                env.log("spawn: (%.0f,%.0f) is not level ground (floor spread %.0f cm within %d cm); trying around it", x, y, spread, T.flat_r)
            end
            return nil
        end
        if not env.clear(pawn, x, y, fz) then return nil end
        local dz = fz + T.pawn_half_height
        if not far(x, y, dz) then return nil end
        return { X = x, Y = y, Z = dz }, fz
    end
    local dest, fz = ok_at(e.x, e.y, false)
    if dest then return dest, fz, true, nil end
    for _, r in ipairs(T.rings) do
        for k = 0, 7 do
            local a = k * math.pi / 4
            local dx, dy = math.cos(a) * r, math.sin(a) * r
            dest, fz = ok_at(e.x + dx, e.y + dy, true)
            if dest then return dest, fz, true, { dx, dy } end
        end
    end
    return { X = e.x, Y = e.y, Z = base_floor + T.pawn_half_height }, base_floor, false, nil
end

-- --- placement ------------------------------------------------------------------------------

-- Start (or restart) a placement of `pawn` on order e. why: "round start",
-- "fell", "director retry", "drift", "new pawn". no_protect: a Live re-place:
-- the move only, no protection (arm_protection is a no-op for it).
-- Every move of the local pawn by this module is a correction the player sees
-- (SMOOTH-1): event `pawn_correction` {why, live, dist_cm}. During Live only a
-- fall below the floor may move an honest player.
function P:note_correction(why, dest)
    local env = self.env
    local x, y, z = env.pawn_loc(self.pawn)
    local d = (x and dest) and math.sqrt((x - dest.X) ^ 2 + (y - dest.Y) ^ 2 + (z - dest.Z) ^ 2) or -1
    local live = (self.live_seen or env.match() == "live") and true or false
    if live then self.counters.corrections_live = self.counters.corrections_live + 1 end
    if env.ev then pcall(env.ev, "pawn_correction", { why = why, live = live, dist_cm = d }) end
end

function P:place(pawn, e, plan, why, no_protect)
    local env = self.env
    local now = env.now()
    local context = self:dislocation_context(e, plan)
    local g = self.dislocation_guard
    if g and not self:same_dislocation_context(g.context, context) then
        -- The guard owns its original value from the first protected tick,
        -- including the initial settle before cur exists. Transfer that
        -- ownership before replacing cur or publishing a new assignment.
        if g.ended or self:restore_dislocation_guard(g.context, "new placement") then
            self.dislocation_guard = nil
        else
            if not g.transfer_blocked then
                env.log("spawn: placement held: native dislocation guard restoration unavailable")
                g.transfer_blocked = true
            end
            return false
        end
    end
    local dest, floor, clear, off = self:choose(pawn, e, plan)
    if not dest then
        env.log("spawn: round %d slot %d -> (%.0f, %.0f, %.0f) NO GROUND under the server point; not moving (%s)",
            plan.round, e.slot, e.x, e.y, e.z, why)
        self.cur = { e = e, plan = plan, why = why, tries = 0, failed = true, t0 = now,
                     err = "no ground under the server point", no_protect = no_protect }
        self.placed_pawn = self.pawn_id   -- do not retry every tick; the Director may ask again
        self.counters.failed = self.counters.failed + 1
        self:arm_protection(now + SP.protect_ms(e) / 1000)
        self:dislocation_step(now, e, plan)
        self:write_status(self.cur, false, "no ground under the server point")
        return false
    end
    self:note_correction(why, dest)
    self.cur = { e = e, plan = plan, why = why, dest = dest, floor = floor, clear = clear, off = off,
                 tries = 0, t0 = now, no_protect = no_protect }
    self.placed_pawn = self.pawn_id
    -- Protection holds until this placement is verified (+ protect_ms then).
    self:arm_protection(nil)
    self:teleport_try()
    return true
end

function P:teleport_try()
    local env, c = self.env, self.cur
    c.tries = c.tries + 1
    self:dislocation_step(env.now(), c.e, c.plan)
    local detail = env.teleport(self.pawn, c.dest, c.e.yaw, c.tries) or ""
    local now = env.now()
    -- Hold the body on the destination for hold_s before the first check
    -- (see P:hold_step), then verify as before.
    c.hold_until = env.hold and (now + SP.T.hold_s) or nil
    c.holds = 0
    c.checks, c.next_check = 0, now + (c.hold_until and SP.T.hold_s or 0) + SP.T.verify_after_s
    self.counters.placed = self.counters.placed + 1
    local how = (c.off and string.format("offset=(%.0f,%.0f)", c.off[1], c.off[2]))
        or (c.clear and "clear=yes") or "clear=NO (blocked everywhere; placed on the server point)"
    env.log("spawn: round %d slot %d -> (%.0f,%.0f,%.0f) %s yaw=%.0f id=%d [%s] try %d/%d%s",
        c.plan.round, c.e.slot, c.dest.X, c.dest.Y, c.dest.Z, how, c.e.yaw, c.e.spawn_id or -1, c.why,
        c.tries, SP.T.max_tries, detail ~= "" and (" " .. detail) or "")
    self:write_status(c, false, nil)
end

-- After a teleport the whole pawn (capsule + pelvis, CharacterMovement
-- velocity 0) travels on ~1.3 m further IN THE DIRECTION OF THE TELEPORT at
-- ~200-250 uu/s for ~0.7 s, then stops: Willie_BP's balance/step controller
-- keeps state from before the jump and walks the body to "catch up". Each
-- re-teleport re-arms it the other way, so retries oscillate around the spot
-- and end ~1.3 m off (spawn_timeout). hold_step pins the body on the
-- destination for hold_s after each teleport (velocities zeroed, a residual
-- drift moved back rigidly) while that transient decays; only then is the
-- placement verified.
function P:hold_step(now)
    local env, c = self.env, self.cur
    if not c or not c.hold_until or c.done or c.failed then return end
    if now >= c.hold_until then
        c.hold_until = nil
        env.log("spawn: hold released after %d correction(s)", c.holds or 0)
        return
    end
    local moved = env.hold(self.pawn, c.dest, SP.T.hold_tol_cm)
    if moved then c.holds = (c.holds or 0) + 1 end
end

-- Called every tick while a placement is unverified.
function P:verify_step(now)
    local env, c, T = self.env, self.cur, SP.T
    if c and c.hold_until then self:hold_step(now) end
    if not c or c.done or c.failed or not c.next_check or now < c.next_check then return end
    local x, y, z = env.pawn_loc(self.pawn)
    local d = x and dist_xy(x, y, c.dest.X, c.dest.Y) or math.huge
    local dz = z and math.abs(z - c.dest.Z) or math.huge
    -- The body too (physical Mesh pelvis when simulated): a capsule on the spot with the
    -- ragdoll left behind is not placed.
    local bx, by, bz, body_err
    if env.body_loc then bx, by, bz, body_err = env.body_loc(self.pawn) end
    local bd = bx and dist_xy(bx, by, c.dest.X, c.dest.Y) or (body_err and math.huge or 0)
    if d <= T.verify_tol_cm and dz <= T.verify_dz_cm and bd <= T.verify_body_cm then
        c.checks = c.checks + 1
        if c.checks >= 2 then
            c.done, c.verified_at = true, now
            if env.drift_probe and self.drift_attempts < 3 then
                -- These coordinates were already read for placement, not by
                -- the probe. Their original Mesh incarnation was not sampled.
                c.drift_baseline = { tick_ms=now*1000, copied_ms=env.now()*1000,
                    capsule={x,y,z}, body=bx and {bx,by,bz} or nil,
                    world=self.wkey, pawn=self.pawn_id, match_id=c.plan.match_id, round=c.plan.round,
                    life=c.e.life, spawn_id=c.e.spawn_id, same_mesh_available=false,
                    reason="historical Mesh identity unavailable" }
            end
            self.counters.verified = self.counters.verified + 1
            self:arm_protection(now + SP.protect_ms(c.e) / 1000)
            env.log("spawn: placement verified round %d slot %d id=%d at (%.0f,%.0f,%.0f), %.0f cm from the destination, "
                .. "try %d, %.1f s [%s]; protected %d ms%s", c.plan.round, c.e.slot, c.e.spawn_id or -1, x, y, z, d, c.tries,
                now - c.t0, c.why, SP.protect_ms(c.e), self.live_seen and "" or " and until the round goes Live")
            self:write_status(c, true, nil)
            self:queue_report(c)
            self:emit_state("placed")
            self.next_state = now + SP.T.state_every_s
            return
        end
        c.next_check = now + T.verify_hold_s
        return
    end
    -- It did not stick (snapped back to the old body, pushed, fell).
    local where = x and string.format("(%.0f,%.0f,%.0f), %.0f cm from the destination%s", x, y, z, d,
        bx and string.format("; body (Mesh pelvis) %.0f cm", bd)
            or (body_err and ("; body unavailable: " .. body_err) or "")) or "unknown"
    if c.tries < T.max_tries then
        self.counters.retries = self.counters.retries + 1
        env.log("spawn: placement did NOT stick: pawn at %s; re-teleporting", where)
        self:teleport_try()
        return
    end
    c.failed = true
    self.counters.failed = self.counters.failed + 1
    local err = string.format("did not stick after %d tries (pawn at %s)", c.tries, where)
    c.err = err
    env.log("spawn: placement FAILED round %d slot %d id=%d: %s", c.plan.round, c.e.slot, c.e.spawn_id or -1, err)
    -- Bounded protection even on failure: never an invulnerable player for the whole round.
    self:arm_protection(now + SP.protect_ms(c.e) / 1000)
    self:write_status(c, false, err)
    self:queue_report(c)
end

-- --- spawn protection -------------------------------------------------------------------------

-- Native Event Check Bone Dislocation Status tests !"Block Spine Breaking"
-- for upperarm_l/r, calf_l/r, neck_02 and spine_04. Invulnerable is not a
-- predicate there. Use the reflected bool (Willie_BP offset 0x461E), without
-- changing joint limits or clearing any existing native injury.
function P:dislocation_context(e, plan)
    if not e or not plan then return nil end
    return { world = self.wkey, pawn = self.pawn, pawn_id = self.pawn_id,
        peer = e.peer or self.env.my_peer_id(), match_id = plan.match_id or 0,
        round = plan.round, spawn_id = e.spawn_id or 0, life = e.life or 0 }
end

function P:same_dislocation_context(a, b)
    return a and b and a.world == b.world and a.pawn == b.pawn and a.pawn_id == b.pawn_id
        and a.peer == b.peer and a.match_id == b.match_id and a.round == b.round
        and a.spawn_id == b.spawn_id and a.life == b.life or false
end

function P:restore_dislocation_guard(context, why)
    local env, g = self.env, self.dislocation_guard
    if not g or g.ended then return false end
    -- Compare only cached identities. Read properties only through the pawn
    -- found NOW, and only in the exact original world and life assignment.
    local w = env.world() or {}
    local fresh = w.key == g.context.world and env.pawn() or nil
    if not self:same_dislocation_context(g.context, context) or fresh ~= g.context.pawn
        or not fresh or env.pawn_id(fresh) ~= g.context.pawn_id then
        if not g.context_unavailable then
            env.log("spawn: native dislocation guard unavailable (original pawn/world/life no longer current; %s)", why)
            g.context_unavailable = true
        end
        return false
    end
    if type(g.original) ~= "boolean" then g.ended = true; return true end -- never guessed or wrote the flag
    local ok, set = pcall(env.prop_set, fresh, "Block Spine Breaking", g.original)
    local read_ok, value = pcall(env.prop_get, fresh, "Block Spine Breaking")
    g.ended = ok and set ~= false and read_ok and value == g.original
    if g.ended or not g.restore_failed then
        env.log("spawn: native dislocation guard %s (Block Spine Breaking=%s; %s)",
            g.ended and "restored" or "unavailable: restore failed", tostring(g.original), why)
    end
    g.restore_failed = not g.ended
    return g.ended
end

function P:dislocation_step(now, e, plan)
    local env = self.env
    local context = self:dislocation_context(e, plan)
    local g = self.dislocation_guard
    if g and (g.context.world ~= self.wkey or g.context.pawn ~= self.pawn) then
        self.dislocation_guard, g = nil, nil -- no old UObject touch on world/pawn replacement
    end
    local active = self:protected(now) and not self.live_seen and env.match() ~= "live" and context ~= nil
    if not active then
        if g and not g.ended then self:restore_dislocation_guard(context, "protection over") end
        return
    end
    if not g then
        local ok, original = pcall(env.prop_get, self.pawn, "Block Spine Breaking")
        g = { context = context }
        if ok and type(original) == "boolean" then g.original = original end
        self.dislocation_guard = g
        if type(g.original) ~= "boolean" then
            env.log("spawn: native dislocation guard unavailable (Block Spine Breaking original bool unreadable)")
            return
        end
    end
    if g.ended or g.restore_failed or type(g.original) ~= "boolean"
        or not self:same_dislocation_context(g.context, context) then return end
    local ok, value = pcall(env.prop_get, self.pawn, "Block Spine Breaking")
    if ok and type(value) == "boolean" and not value then
        local wrote, set = pcall(env.prop_set, self.pawn, "Block Spine Breaking", true)
        if not (wrote and set ~= false) and not g.write_unavailable then
            env.log("spawn: native dislocation guard unavailable (Block Spine Breaking write failed)")
            g.write_unavailable = true
        end
    end
end

function P:arm_protection(until_t)
    -- A Live re-place (a living player fell) never re-arms protection,
    -- also not when its placement is verified or fails.
    if self.cur and self.cur.no_protect then return end
    local pr = self.protect
    if not pr then
        pr = { set_by_us = false, invuln = 0, vitals = 0, since = self.env.now() }
        self.protect = pr
    end
    pr.respawn = self.respawn_spawn == true
    pr.until_t = until_t or (pr.respawn and (self.env.now() + SP.T.protect_max_ms / 1000)) or nil
    pr.ended = false
end

-- While protected: keep "Invulnerable" set and top the vitals up from the
-- CDO. At the end: clear "Invulnerable" once (unless the pawn is DED).
function P:protect_step(now)
    local env, pr = self.env, self.protect
    if not pr or pr.ended or not self.pawn then return end
    if self:protected(now) then
        if env.prop_get(self.pawn, "Invulnerable") ~= true then
            env.prop_set(self.pawn, "Invulnerable", true)
            pr.set_by_us = true
            pr.invuln = pr.invuln + 1
            self.counters.invuln_set = self.counters.invuln_set + 1
        end
        if now >= self.next_vitals then
            self.next_vitals = now + SP.T.vitals_every_s
            self:top_up_vitals(false)
        end
        return
    end
    pr.ended = true
    local ded = env.prop_get(self.pawn, "DED") == true
    if pr.set_by_us and not ded then env.prop_set(self.pawn, "Invulnerable", false) end
    env.log("spawn protection ended after %.1f s (invulnerable re-asserted %d x, vitals restored %d x)%s",
        now - (pr.since or now), pr.invuln, pr.vitals, ded and " - pawn is DED, flag left as is" or "")
    self:emit_state("protect_end")
end

-- --- no collision with other Willies while protected ---------------------------------------------
--
-- Idle pawns ~1.5 m apart get knocked about before the round starts (the
-- local pawn can hit the opponent's stand-in at ~950 uu/s right after spawn
-- and end up Downed), and Half Sword drops held weapons on a knockdown. While
-- protected, the local pawn's bodies and weapons do not collide with any other
-- Willie's bodies and weapons (both directions, CollisionDisabler plugin: the
-- per-pair physics filter Willie_BP itself uses), and their capsules ignore
-- each other when moving. Re-enabled when protection ends, or as soon as no
-- other Willie overlaps us (at most overlap_grace_s later).
function P:nocollide_step(now)
    local env, T, nc = self.env, SP.T, self.nc
    if not env.set_collision or not env.willies or not self.pawn then return end
    if now < nc.next_t then return end
    nc.next_t = now + T.nocollide_every_s
    if self:protected(now) then
        nc.ended_t, nc.defer_logged = nil, nil
        for _, o in ipairs(env.willies(self.pawn) or {}) do
            local rec = nc.pairs[o.key]
            if not rec or rec.sig ~= o.sig then
                local ok, detail = env.set_collision(self.pawn, o.actor, false)
                nc.pairs[o.key] = { actor = o.actor, sig = o.sig, ok = ok }
                self.counters.nocollide = self.counters.nocollide + 1
                if not rec then
                    env.log("spawn protection: no collision with %s (%s%s)", tostring(o.name or o.key),
                        ok and "ok" or "FAILED", detail and (": " .. tostring(detail)) or "")
                end
            end
        end
        return
    end
    if next(nc.pairs) == nil then return end
    nc.ended_t = nc.ended_t or now
    if now - nc.ended_t < T.overlap_grace_s and env.actor_loc then
        local x, y = env.pawn_loc(self.pawn)
        local live = self:live_willies()
        for key in pairs(nc.pairs) do
            local a = live[key]
            local ox, oy
            if a then ox, oy = env.actor_loc(a) end
            if x and ox and dist_xy(x, y, ox, oy) < T.overlap_cm then
                if not nc.defer_logged then
                    nc.defer_logged = true
                    env.log("spawn protection over, but another Willie is %.0f cm away; collision stays off until it moves "
                        .. "(at most %.0f s)", dist_xy(x, y, ox, oy), T.overlap_grace_s)
                end
                return
            end
        end
    end
    self:restore_collision("protection over")
end

-- The other Willies that exist NOW, by key (a fresh FindAllOf). The actors
-- kept in nc.pairs are never touched again: a pooled Willie can be freed
-- while we are protected (HSMPAvatars census "extras 1 -> 0"), and IsValid /
-- the collision calls on its cached object are then an uncatchable access
-- violation at the end of spawn protection (as the round goes Live).
function P:live_willies()
    local live = {}
    for _, o in ipairs((self.env.willies and self.env.willies(self.pawn)) or {}) do live[o.key] = o.actor end
    return live
end

-- Re-enable every pair we disabled whose Willie still exists (no-op without
-- pairs); a vanished one has no collision pair left to restore.
function P:restore_collision(why)
    local env, nc = self.env, self.nc
    local n, failed = 0, 0
    local live = (next(nc.pairs) ~= nil) and self:live_willies() or {}
    local pawn_ok = (not env.valid) or env.valid(self.pawn)
    for key in pairs(nc.pairs) do
        local a = live[key]
        if a and pawn_ok then
            local ok = pcall(env.set_collision, self.pawn, a, true)
            if ok then n = n + 1 else failed = failed + 1 end
        end
    end
    if n + failed > 0 then
        self.counters.recollide = self.counters.recollide + n
        env.log("spawn protection: collision with %d other Willie(s) restored (%s)%s", n, why,
            failed > 0 and string.format(", %d FAILED", failed) or "")
    end
    nc.pairs, nc.ended_t, nc.defer_logged = {}, nil, nil
end

-- --- pawn_state events (gate: "knocked down at spawn") ----------------------------------------------
--
-- pawn_state{at, round, pawn, consciousness, downed, fallen, health, weapon_r,
-- weapon_l, protected, live, dist_cm} at placement, at Ready, every 5 s while
-- protected, at Live and when protection ends.
function P:emit_state(at)
    local env = self.env
    if not env.pawn_state or not self.pawn then return end
    local s = env.pawn_state(self.pawn) or {}
    local x, y = env.pawn_loc(self.pawn)
    local c = self.cur
    local d = (c and c.dest and x) and dist_xy(x, y, c.dest.X, c.dest.Y) or nil
    self.counters.states = self.counters.states + 1
    local f = {
        at = at, round = c and c.plan.round or 0, pawn = self.pawn_id,
        consciousness = s.consciousness, downed = s.downed == true, fallen = s.fallen == true, health = s.health,
        weapon_r = s.weapon_r or "None", weapon_l = s.weapon_l or "None",
        protected = self:protected() and true or false, live = self.live_seen and true or false,
        dist_cm = d and math.floor(d + 0.5) or nil,
    }
    if env.ev then pcall(env.ev, "pawn_state", f) end
    env.log("pawn_state[%s] round %d: consciousness=%s downed=%s fallen=%s health=%s R=%s L=%s protected=%s block_spine_breaking=%s%s",
        at, f.round, tostring(f.consciousness), tostring(f.downed), tostring(f.fallen), tostring(f.health),
        f.weapon_r, f.weapon_l, tostring(f.protected), tostring(s.block_spine_breaking),
        d and string.format(" %.0f cm from the spawn", d) or "")
    return f
end

-- The Director reached Ready (or Live) for this world: one pawn_state.
function P:director_step(now)
    if self.ready_logged or now < self.next_dir or not (self.cur and self.cur.done) then return end
    self.next_dir = now + SP.T.director_every_s
    local st = self.env.director_state and self.env.director_state()
    if st == "Ready" or st == "Live" then
        self.ready_logged = true
        self:emit_state("ready")
    end
end

-- Restore the vitals from the CDO when anything dropped (force: always).
function P:top_up_vitals(force)
    local env = self.env
    local cdo = env.cdo_vitals()
    if not cdo or not self.pawn then return false end
    local hurt = force
    if not hurt then
        local hp, want = tonumber(env.prop_get(self.pawn, "Health")), cdo.Health
        if want and hp and hp < want - 0.5 then hurt = true end
    end
    if not hurt then return false end
    for _, k in ipairs(SP.VITALS_FIELDS) do
        if cdo[k] ~= nil then env.prop_set(self.pawn, k, cdo[k]) end
    end
    if self.protect then self.protect.vitals = self.protect.vitals + 1 end
    self.counters.vitals = self.counters.vitals + 1
    return true
end

-- --- the fall watchdog -------------------------------------------------------------------------

function P:fall_z(e)
    local T, c = SP.T, self.cur
    if self:protected() and c and c.floor then return c.floor - T.fall_protect_cm end
    local lo = self.env.native_floor()
    if e and e.z then lo = lo and math.min(lo, e.z) or e.z end
    if c and c.floor then lo = lo and math.min(lo, c.floor) or c.floor end
    return lo and (lo - T.fall_margin_cm) or -10000
end

-- Diagnostic work is admitted only at an existing correction boundary. A
-- failed measurement never changes the drift limit or makes placement proof.
function P:observe_drift(c, e, plan, x, y, z, now, capsule_read_ms)
    local env = self.env
    if not env.drift_probe or self.drift_attempts >= 3 then return true end
    self.drift_attempts = self.drift_attempts + 1
    local expected = { world=self.wkey, pawn=self.pawn_id, peer=e and e.peer,
        match_id=plan and plan.match_id, round=plan and plan.round, life=e and e.life,
        spawn_id=e and e.spawn_id, attempt=self.drift_attempts, watch_tick_ms=now*1000,capsule_read_ms=capsule_read_ms,
        capsule={x,y,z}, target={c.dest.X,c.dest.Y,c.dest.Z}, target_yaw=c.e.yaw, floor=c.floor, slot=c.e.slot,
        verified_baseline=c.drift_baseline }
    local function current()
        if self.cur ~= c or self.wkey ~= expected.world or self.pawn_id ~= expected.pawn then return false end
        local own, active = self:order()
        return own and active and active.match_id==expected.match_id and active.round==expected.round
            and own.peer==expected.peer and own.life==expected.life and own.spawn_id==expected.spawn_id
            and env.match()~="live" and self:protected() or false
    end
    local ok, row, recheck = pcall(env.drift_snapshot, self.pawn, expected, current)
    if ok and type(row)=="table" and env.drift_log then pcall(env.drift_log,row) end
    -- Logging or an unavailable diagnostic must not bypass the independent
    -- native world/actor/Mesh guard returned by the admitted snapshot.
    if not ok or type(recheck)~="function" then return false end
    local safe, value = pcall(recheck)
    return safe and value==true
end

function P:watch_step(now)
    local env, T = self.env, SP.T
    local prot = self:protected(now)
    if now < self.next_watch then return end
    self.next_watch = now + (prot and T.watch_protect_s or T.watch_s)
    if now - self.last_fall < T.fall_cooldown_s then return end
    local x, y, z = env.pawn_loc(self.pawn)
    local capsule_read_ms=env.drift_probe and self.drift_attempts<3 and env.now()*1000 or nil
    if not z then return end
    local e, plan = self:order()
    local c = self.cur
    local fell = z < self:fall_z(e)
    local drift = false
    if not fell and prot and c and c.done and c.dest then
        local st = env.match()
        drift = st ~= "live" and dist_xy(x, y, c.dest.X, c.dest.Y) > T.drift_cm
    end
    if not fell and not drift then return end
    -- Never revive the dead. A corpse that falls (or clips through
    -- the floor) stays where it is: the server already counted the death.
    local hp = tonumber(env.prop_get(self.pawn, "Health"))
    if env.prop_get(self.pawn, "DED") == true or (hp ~= nil and hp <= 0) then
        if not self.dead_fall_logged then
            self.dead_fall_logged = true
            env.log("spawn watchdog: pawn %s at (%.0f,%.0f,%.0f) but it is dead (DED=%s Health=%s); left alone",
                fell and "FELL" or "pushed off its spawn", x, y, z, tostring(env.prop_get(self.pawn, "DED")), tostring(hp))
        end
        return
    end
    self.dead_fall_logged = nil
    if drift and not self:observe_drift(c,e,plan,x,y,z,now,capsule_read_ms) then return end
    self.last_fall = now
    self.counters.falls = self.counters.falls + 1
    -- During Live a fall is part of the fight: the living player is put back
    -- on the spawn WITHOUT a heal and without protection (no free reset).
    local live = self.live_seen or env.match() == "live"
    env.log("spawn watchdog: pawn %s at (%.0f,%.0f,%.0f)%s; returning to spawn without a death%s",
        fell and "FELL" or "pushed off its spawn", x, y, z, prot and " during spawn protection" or "",
        live and " (Live: no heal, no protection)" or "")
    if not live then
        self:arm_protection(nil)          -- protected until the re-place is verified
        self:top_up_vitals(true)
    end
    if e then
        self:place(self.pawn, e, plan, fell and "fell" or "drift", live)
    elseif live then
        local nx, ny, nz, nyaw = env.native_point()
        local fz = nx and env.ground(self.pawn, nx, ny, nz)
        if fz then
            self:note_correction(fell and "fell" or "drift", { X = nx, Y = ny, Z = fz + T.pawn_half_height })
            env.teleport(self.pawn, { X = nx, Y = ny, Z = fz + T.pawn_half_height }, nyaw)
            env.log("spawn watchdog: no server order; put back on native spawner 0 (%.0f,%.0f,%.0f)", nx, ny, fz)
        end
    else
        local nx, ny, nz, nyaw = env.native_point()
        if nx then
            local fz = env.ground(self.pawn, nx, ny, nz)
            if fz then
                self:note_correction(fell and "fell" or "drift", { X = nx, Y = ny, Z = fz + T.pawn_half_height })
            env.teleport(self.pawn, { X = nx, Y = ny, Z = fz + T.pawn_half_height }, nyaw)
                env.log("spawn watchdog: no server order; put back on native spawner 0 (%.0f,%.0f,%.0f)", nx, ny, fz)
            end
        end
        self:arm_protection(now + T.protect_min_ms / 1000)
    end
end

-- --- Director retry requests ------------------------------------------------------------------

-- bus `spawn_request` {seq, round, arena, pawn, spawn_id, why}
-- Handled only once the pawn has settled (left pending until then).
function P:request_step(now, settled)
    if now < self.next_req then return end
    self.next_req = now + SP.T.request_every_s
    local q = self.env.request and self.env.request()
    local seq = q and tonumber(q.seq)
    if not seq or seq <= self.req_seq then return end
    if not settled then return end
    local e, plan, order_why = self:order()
    if not e and order_why == "waiting for spawn life" then return end
    self.req_seq = seq
    local r, a = tonumber(q.round), q.arena
    local pw = (q.pawn ~= nil and q.pawn ~= "") and q.pawn or nil
    if not e or r ~= plan.round or a ~= plan.arena or (pw and pw ~= self.pawn_id) then
        self.env.log("spawn: Director request #%d ignored (round %s arena %s pawn %s; mine: %s)", seq, tostring(r),
            tostring(a), tostring(pw), self:order_text())
        return
    end
    self.counters.requests = self.counters.requests + 1
    self.env.log("spawn: Director asked for a re-place (#%d: %s)", seq, (q.why ~= nil and q.why ~= "") and q.why or "?")
    self:place(self.pawn, e, plan, "director retry")
end

-- --- server report (the G2S `spawned` record) ------------------------------------------------

function P:queue_report(c)
    local d = c.dest
    self.report = {
        rec = { round = c.plan.round, slot = tonumber(c.e.slot) or 0, pos = { d.X, d.Y, d.Z },
                clear = (c.clear and c.done) and true or false },
        verb = string.format("spawned round %d slot %s (%.0f, %.0f, %.0f)", c.plan.round, tostring(c.e.slot), d.X, d.Y, d.Z),
        left = SP.T.reports, next_at = 0,
    }
end

-- Sent typed (IPC.send("spawned", ...)), and nothing before the session is up
-- (env.connected() is the gate). A held or refused report is retried, not lost.
function P:flush_report(now)
    local r = self.report
    if not r or now < r.next_at then return end
    local env = self.env
    if env.connected and not env.connected() then
        r.next_at = now + 1.0
        if not r.held_logged then
            r.held_logged = true
            env.log("spawn: %s held until the sidecar is connected", r.verb)
        end
        return
    end
    if not env.send_spawned(r.rec) then r.next_at = now + 1.0; return end
    r.left, r.next_at = r.left - 1, now + 1.0
    if r.left <= 0 then self.report = nil end
end

-- --- per tick ------------------------------------------------------------------------------------

-- Bus key "fallback_swap" = {until_s = <os.clock() s>, keep = "<pawn name>"} (HSMPAvatars,
-- written right before its SpawnCombatants fallback): until then a possession of any pawn
-- other than `keep`, while `keep` is the pawn we hold, is the native spawn's transient swap.
function P:transient_swap(pid, now)
    local fs = self.env.fallback_swap()
    if type(fs) ~= "table" then return false end
    local untl, keep = tonumber(fs.until_s), fs.keep
    if not untl or untl <= 0 or now >= untl or keep ~= self.pawn_id or pid == keep then return false end
    if self.swap_logged ~= pid then
        self.swap_logged = pid
        self.env.log("spawn: %s possessed during the stand-in fallback spawn; keeping %s (transient swap)", tostring(pid), tostring(keep))
    end
    return true
end

-- Call every HSMPSync tick while the world guard passed and an arena world is
-- loaded. Fresh lookups only (the pawn is re-fetched every call).
function P:tick()
    local env, T = self.env, SP.T
    local now = env.now()
    local w = env.world() or {}
    if w.key ~= self.wkey then
        self:reset("world")
        self.wkey, self.wkey_since, self.wshort = w.key, now, w.short
        self.world_t0 = now
    end
    self.wshort = w.short
    self:flush_report(now)

    local pawn = env.pawn()
    if not pawn then return end   -- keep the last pawn: a brief unpossess is not a new pawn
    local pid = env.pawn_id(pawn)
    -- HSMPAvatars' SpawnCombatants fallback (the stand-in body) makes the native
    -- spawn possess its new body for a frame or two before HSMPAvatars hands the
    -- player back their placed pawn. Treating that as a new pawn would drop the
    -- placement and protection of the real one and re-place it a few seconds
    -- later, after the Director has already reported Ready (a pawn ~2 m off
    -- its slot at Ready). While HSMPAvatars' swap window is open, a foreign
    -- pawn is a brief unpossess.
    if pid ~= self.pawn_id and self.pawn_id and self:transient_swap(pid, now) then return end
    if pid ~= self.pawn_id then
        -- A new pawn (first spawn, native re-possession, respawn): it needs
        -- its own placement and its own protection.
        self.pawn_id, self.pawn_since = pid, now
        if self.placed_pawn and self.placed_pawn ~= pid then
            env.log("spawn: possessed pawn changed (%s -> %s); placing the new pawn", tostring(self.placed_pawn), pid)
        end
        -- The previous pawn (same world: a possession swap) must not stay
        -- invulnerable if we made it so: it may become a stand-in or a foe.
        local old, pr = self.pawn, self.protect
        if old and old ~= pawn and pr and pr.set_by_us and not pr.ended and (not env.valid or env.valid(old))
            and env.prop_get(old, "DED") ~= true then
            env.prop_set(old, "Invulnerable", false)
        end
        if old and old ~= pawn and next(self.nc.pairs) ~= nil then self:restore_collision("possessed pawn changed") end
        self.cur, self.placed_pawn, self.protect = nil, nil, nil
        self.dislocation_guard = nil -- the previous actor is not freshly owned; never restore through its cache
        self.nc.pairs = {}
        -- A possession change during Live (a native SpawnCombatants swap, a
        -- re-possession) is never placed: teleporting the player to the spawn
        -- mid-fight would be a free reset (HSMPCombat would see an 11 m jump).
        -- The watchdog (falls) still runs.
        local incoming = self:order()
        self.respawn_spawn = self:respawn_order(incoming)
        if self.live_seen and not self.respawn_spawn then
            self.placed_pawn = pid
            env.log("spawn: new pawn %s during Live: not placed (watchdog only)", pid)
        end
    end
    self.pawn = pawn

    local e, plan, why = self:order()
    if not self.protect and self:respawn_order(e) then self.respawn_spawn = true end
    local mstate = env.match()
    if mstate == "countdown" then self.live_seen, self.live_at = false, nil end   -- a new round in the same world
    if mstate == "live" and not self.live_seen then
        self.live_seen, self.live_at = true, now
        local c = self.cur
        if c and self.protect and (c.done or c.failed) then
            -- The Director reads the bounded window from here on.
            self:write_status(c, c.done == true, c.err)
            local pu = self:protect_until()
            env.log("spawn: round is Live; spawn protection %s", pu and pu > now
                and string.format("ends in %.1f s", pu - now) or "ends now")
        end
        self:dislocation_step(now, e, plan) -- restore before the normal Live pawn diagnostics
        if self.protect then self:emit_state("live") end
    end
    -- Protection starts as soon as an MP pawn exists, before placement, and
    -- holds until the round goes Live.
    if e and not self.protect then self:arm_protection(nil) end
    if e and self.protect then self.protect.until_live = not self.protect.respawn end
    self:protect_step(now)
    self:dislocation_step(now, e, plan)
    self:nocollide_step(now)
    if self.protect and not self.protect.ended and self:protected(now) and self.cur and self.cur.done
        and now >= self.next_state then
        self.next_state = now + SP.T.state_every_s
        self:emit_state("protect")
    end
    self:director_step(now)

    local settled = now - self.wkey_since >= T.settle_s and now - self.pawn_since >= T.settle_s
        and env.pawn_begun(pawn)
    -- A Director retry may arrive before or after our own placement.
    self:request_step(now, settled)
    if self.placed_pawn ~= pid then
        if not settled then return end
        if not e then
            if not self.no_order_logged and now - (self.world_t0 or now) > T.plan_wait_s then
                self.no_order_logged = true
                env.log("spawn: no server spawn order for this world (%s); keeping the native placement", tostring(why))
            end
            return
        end
        self:place(pawn, e, plan, "round start")
        return
    end
    -- An order for a NEW round in the same world (live -> countdown without a reload).
    local c = self.cur
    if e and c and c.e and ((plan.match_id or 0) ~= (c.plan.match_id or 0) or (c.e.spawn_id ~= e.spawn_id and plan.round ~= c.plan.round)) then
        self:place(pawn, e, plan, "new round")
        return
    end
    self:verify_step(now)
    self:watch_step(now)
end

-- ---------------------------------------------------------------------------------------------
-- The real env (UE4SS). ctx: { UEHelpers, log, state_dir, my_peer_id(), match(),
-- world() -> {key, short}, view() (HS.view), session_live(), connected() }. Fresh lookups everywhere: no UObject is kept
-- beyond one call except through the placer's per-tick `pawn`.
function SP.make_ue_env(ctx)
    local UEH, Log = ctx.UEHelpers, ctx.log
    local env = {}
    env.now = os.clock
    env.log = Log
    env.my_peer_id = ctx.my_peer_id
    env.match = ctx.match
    env.world = ctx.world
    env.drift_probe = ctx.drift_probe == true
    env.drift_log = ctx.drift_log

    -- Shared-memory IPC: typed records. The plan comes from the session record
    -- (ctx.view = shared/hsmp_session.lua HS.view), the request / status / director
    -- state and HSMPAvatars' fallback swap are typed bus keys, the report is the G2S `spawned`
    -- record.
    local function ipc() return rawget(_G, "HSMP_IPC") end
    function env.fallback_swap()
        local i = ipc()
        return i and i.bus_table("fallback_swap") or nil
    end
    function env.plan() return SP.plan_from_view(ctx.view and ctx.view(), ctx.mode and ctx.mode()) end
    function env.request()
        local i = ipc()
        local t = i and i.bus_table("spawn_request")
        if t and (t.seq or 0) == 0 then return nil end   -- cleared form
        return t
    end
    function env.put_status(t)
        local i = ipc()
        return i and i.bus_put("spawn_status", t) or false
    end
    function env.clear_status()
        local i = ipc()
        return i and i.bus_clear("spawn_status") or false
    end
    function env.director_state()
        local i = ipc()
        local t = i and i.bus_table("director")
        return t and t.state ~= "" and t.state or nil
    end
    function env.send_spawned(t)
        local i = ipc()
        local id = i and i.send("spawned", t)
        return id
    end
    env.session_live = ctx.session_live      -- SP.peer_reader's third result
    env.connected = ctx.connected            -- gate for the report

    -- ctx.pc = the mod's WG.pc (one PlayerController lookup per frame).
    function env.pawn()
        local p = ctx.ai_pawn and ctx.ai_pawn() or nil
        if p then return p end -- verified AI ownership survives a native stand-in possession swap
        pcall(function()
            local pc = ctx.pc and ctx.pc() or UEH.GetPlayerController()
            if pc and pc:IsValid() then
                local pw = pc.Pawn
                if pw and pw:IsValid() then p = pw end
            end
        end)
        return p
    end
    function env.pawn_id(p) local id = "?"; pcall(function() id = p:GetFName():ToString() end); return id end
    function env.valid(p) local ok = false; pcall(function() ok = p:IsValid() end); return ok end
    function env.pawn_begun(p)
        local b = true
        pcall(function() b = p:HasActorBegunPlay() end)
        return b ~= false
    end
    function env.pawn_loc(p)
        local l; pcall(function() l = p:K2_GetActorLocation() end)
        if l then return l.X, l.Y, l.Z end
        return nil
    end
    local PELVIS = nil
    local function finite(v)
        return type(v) == "number" and v == v and v ~= math.huge and v ~= -math.huge
    end
    -- A simulated body's animated socket may follow the capsule while the
    -- rigid body is still at the native spawner. COM is a fresh physics read;
    -- its fixed bone offset cancels when measuring a translation residual.
    local function pelvis(m, guard, physical_only)
        local function read(f)
            if guard and guard()~=true then error("scope changed",0) end
            local ok, value=pcall(f)
            if guard and guard()~=true then error("scope changed",0) end
            if not ok then error("native body read failed",0) end
            return value
        end
        local sim
        local ok = pcall(function()
            PELVIS = PELVIS or FName("pelvis")
            sim = read(function() return m:IsSimulatingPhysics(PELVIS) end)
        end)
        if not ok or type(sim) ~= "boolean" then return nil, "pelvis simulation state unavailable" end
        if physical_only and not sim then return nil,"physical simulation unavailable" end
        local l
        ok = pcall(function()
            if sim then l = read(function() return m:GetCenterOfMass(PELVIS) end)
            else l = read(function() return m:GetSocketLocation(PELVIS) end) end
            -- Copy plain scalars now; no native FVector wrapper is retained.
            if not (l and finite(l.X) and finite(l.Y) and finite(l.Z)) then l = nil
            else l = { X = l.X, Y = l.Y, Z = l.Z } end
        end)
        if not ok or not l then
            return nil, sim and "physical pelvis unavailable" or "visual pelvis unavailable"
        end
        return l, nil, sim
    end
    -- Unknown simulation / physics reads explicitly fail placement proof.
    function env.body_loc(p, guard, physical_only)
        local m
        pcall(function()
            if guard and guard()~=true then return end
            local candidate=p.Mesh
            if guard and guard()~=true then return end
            if candidate and candidate:IsValid() then m=candidate end
            if guard and guard()~=true then m=nil end
        end)
        if not m then return nil, nil, nil, "Mesh unavailable" end
        local l, err = pelvis(m, guard, physical_only)
        if l then return l.X, l.Y, l.Z end
        return nil, nil, nil, err
    end

    -- Called at most three times, synchronously before an already warranted
    -- protected drift correction. No native wrapper escapes this call.
    function env.drift_snapshot(p, expected, placement_current)
        local closed, pawn_id, mesh_id, world_id=false,nil,nil,nil
        local drops=ctx.drift_drops and ctx.drift_drops()
        local function base()
            if closed then return false end
            local ok, value=pcall(function()
                return ctx.drift_world_current and ctx.drift_world_current(expected.world,drops)==true
                    and placement_current()==true
            end)
            if not ok or value~=true then closed=true;return false end
            return true
        end
        local function read(f)
            assert(base(),"scope changed")
            local ok,value=pcall(f)
            assert(base(),"scope changed")
            if not ok then error("native identity read unavailable",0) end
            return value
        end
        local function identity(o)
            assert(o and read(function() return o:IsValid() end)==true,"identity unavailable")
            local address=read(function() return o:GetAddress() end)
            local fn=read(function() return o:GetFName() end)
            local name=read(function() return fn:ToString() end)
            assert(finite(address) and math.tointeger(address) and address>0
                and type(name)=="string" and name~="" and #name<=512 and not name:find("\0",1,true),"identity unavailable")
            return {address=address,name=name}
        end
        local function same(a,b)return a and b and a.address==b.address and a.name==b.name end
        local function current()
            if not base() then return false end
            local ok,value=pcall(function()
                if not pawn_id or not mesh_id or not world_id then return false end
                local own=read(function() return env.pawn() end)
                if not same(identity(own),pawn_id) then return false end
                if not same(identity(read(function() return own:GetWorld() end)),world_id) then return false end
                local mesh=read(function() return own.Mesh end)
                return same(identity(mesh),mesh_id)
                    and same(identity(read(function() return mesh:GetOwner() end)),pawn_id)
            end)
            if not ok or value~=true then closed=true;return false end
            return true
        end
        local function xyz(a)
            if type(a)=="table" and finite(a[1]) and finite(a[2]) and finite(a[3]) then return {a[1],a[2],a[3]} end
        end
        local function number(v)return finite(v) and v or nil end
        local row={inst=(os.getenv("HSMP_INST") or ""):sub(1,16),phase="protected_drift_before_correction",
            authority=false,available=false,attempt=expected.attempt,world=expected.world,pawn=expected.pawn,
            peer=number(expected.peer),match_id=number(expected.match_id),round=number(expected.round),life=number(expected.life),
            spawn_id=number(expected.spawn_id),slot=number(expected.slot),watch_tick_ms=number(expected.watch_tick_ms),
            capsule_read_ms=number(expected.capsule_read_ms),
            capsule=xyz(expected.capsule),target=xyz(expected.target),target_yaw=number(expected.target_yaw),
            floor=finite(expected.floor) and expected.floor or nil,
            body_basis="physical pelvis center of mass"}
        local started=env.now()
        row.observed_ms=number(started) and started*1000 or nil
        local ok,why=pcall(function()
            assert(row.capsule and row.target,"position unavailable")
            for _,k in ipairs({"peer","match_id","round","life","spawn_id"}) do
                assert(finite(row[k]) and math.tointeger(row[k]) and row[k]>0,"assignment unavailable")
            end
            local pc=read(function() return ctx.pc and ctx.pc() or UEH.GetPlayerController() end)
            world_id=identity(read(function() return pc:GetWorld() end))
            pawn_id=identity(p)
            assert(pawn_id.name==expected.pawn,"pawn changed")
            assert(same(identity(read(function() return env.pawn() end)),pawn_id),"pawn changed")
            assert(same(identity(read(function() return p:GetWorld() end)),world_id),"pawn world changed")
            local mesh=read(function() return p.Mesh end)
            mesh_id=identity(mesh)
            assert(same(identity(read(function() return mesh:GetOwner() end)),pawn_id),"Mesh owner changed")
            row.native_world,row.actor,row.mesh=world_id,pawn_id,mesh_id
            assert(current(),"scope changed")
            row.body_read_start_ms=number(env.now()*1000)
            local x,y,z,err=env.body_loc(p,current,true)
            row.body_read_end_ms=number(env.now()*1000)
            assert(current(),"scope changed")
            assert(finite(x) and finite(y) and finite(z),err or "physical body unavailable")
            -- body_loc's normal visual fallback is useful for placement, but
            -- cannot constitute this physical-only diagnostic observation.
            row.body={x,y,z};row.available=true
            row.capsule_target_xy_cm=dist_xy(row.capsule[1],row.capsule[2],row.target[1],row.target[2])
            row.body_target_xy_cm=dist_xy(x,y,row.target[1],row.target[2])
            row.body_capsule_delta={x-row.capsule[1],y-row.capsule[2],z-row.capsule[3]}
            assert(finite(row.capsule_target_xy_cm) and finite(row.body_target_xy_cm) and xyz(row.body_capsule_delta),
                "derived position unavailable")
            -- These are evaluated socket/component rotations and native handle
            -- targets, not an independent rigid-body orientation read. The COM
            -- above remains the only physical-body observation in this row.
            local function guarded(f, guard)
                assert(current() and (not guard or guard()),"rotation scope changed")
                local good,value=pcall(f)
                assert(current() and (not guard or guard()),"rotation scope changed")
                if not good then error("rotation read unavailable",0) end
                return value
            end
            local function vector(v)
                assert(v and finite(v.X) and finite(v.Y) and finite(v.Z),"vector unavailable")
                return {v.X,v.Y,v.Z}
            end
            local function rotator(v)
                assert(v and finite(v.Pitch) and finite(v.Yaw) and finite(v.Roll),"rotator unavailable")
                return {pitch=v.Pitch,yaw=v.Yaw,roll=v.Roll}
            end
            local function optional(f)
                local good,value=pcall(f)
                assert(current(),"scope changed")
                if good then return {available=true,value=value} end
                return {available=false,reason=tostring(value):gsub("[%c]"," "):sub(1,192)}
            end
            local function component(field)
                local o=guarded(function()return p[field]end)
                local id=guarded(function()return identity(o)end)
                local lost=false
                local function bound()
                    if lost or not current() then lost=true;return false end
                    local good,value=pcall(function()
                        local candidate=read(function()return p[field]end)
                        if not same(identity(candidate),id) then return false end
                        return same(identity(read(function()return candidate:GetOwner()end)),pawn_id)
                    end)
                    if not good or value~=true then lost=true;return false end
                    return true
                end
                assert(bound(),"component binding unavailable")
                return o,id,bound
            end
            local rotation={physical_orientation_available=false,
                body_rotation_basis="evaluated Mesh pelvis socket; not independent rigid-body orientation",
                component_rotation_basis="current scene component world rotation",
                observed_start_ms=number(env.now()*1000)}
            rotation.actor=optional(function()return rotator(guarded(function()return p:K2_GetActorRotation()end))end)
            rotation.mesh_component=optional(function()return rotator(guarded(function()return mesh:K2_GetComponentRotation()end))end)
            rotation.mesh_pelvis_socket=optional(function()return rotator(guarded(function()return mesh:GetSocketRotation(PELVIS)end))end)
            rotation.driver=optional(function()
                local driver,id,bound=component("DriverSkeleton")
                local component_rot=guarded(function()return rotator(driver:K2_GetComponentRotation())end,bound)
                local pelvis_rot=guarded(function()return rotator(driver:GetSocketRotation(PELVIS))end,bound)
                assert(bound(),"DriverSkeleton changed")
                return {identity=id,component_rotation=component_rot,pelvis_socket_rotation=pelvis_rot,
                    basis="evaluated DriverSkeleton socket; not physical body"}
            end)
            rotation.current_control=optional(function()return rotator(guarded(function()return p["Current Control Rotation"]end))end)
            rotation.on_ground_yaw=optional(function()
                local v=guarded(function()return p["On Ground Z Rotation"]end)
                assert(finite(v),"ground yaw unavailable");return v
            end)
            rotation.movement_input=optional(function()return vector(guarded(function()return p["Movement Input Vector"]end))end)
            rotation.foot_points={}
            for _,field in ipairs({"R Foot On Ground Loc","L Foot On Ground Loc"}) do
                rotation.foot_points[field]=optional(function()return vector(guarded(function()return p[field]end))end)
            end
            rotation.handles={}
            for _,field in ipairs({"PhysicsHandle LowerBody","PhysicsHandle UpperBody"}) do
                rotation.handles[field]=optional(function()
                    local handle,id,bound=component(field)
                    local grabbed=guarded(function()return identity(handle.GrabbedComponent)end,bound)
                    assert(same(grabbed,mesh_id),"handle does not grab current Mesh")
                    local released=false
                    local function grabbing()
                        if released or not bound() then released=true;return false end
                        local good,value=pcall(function()
                            return same(identity(read(function()return handle.GrabbedComponent end)),grabbed)
                        end)
                        if not good or value~=true then released=true;return false end
                        return true
                    end
                    local target_location,target_rotation={},{}
                    guarded(function()handle:GetTargetLocationAndRotation(target_location,target_rotation)end,grabbing)
                    -- Struct OutParms consume separate tables in the pinned
                    -- bridge; support direct FStruct reuse and named wrappers.
                    local loc=guarded(function()return vector(target_location.TargetLocation or target_location)end,grabbing)
                    local rot=guarded(function()return rotator(target_rotation.TargetRotation or target_rotation)end,grabbing)
                    return {identity=id,grabbed_mesh=grabbed,target_location=loc,target_rotation=rot,
                        basis="native PhysicsHandle target; grabbed bone unavailable"}
                end)
            end
            rotation.observed_end_ms=number(env.now()*1000)
            row.rotation=rotation
            local prior=expected.verified_baseline
            if type(prior)=="table" then
                row.verified_baseline={tick_ms=number(prior.tick_ms),copied_ms=number(prior.copied_ms),
                    capsule=xyz(prior.capsule),body=xyz(prior.body),world=prior.world,pawn=prior.pawn,
                    match_id=number(prior.match_id),round=number(prior.round),life=number(prior.life),spawn_id=number(prior.spawn_id),
                    same_mesh_available=false,reason="historical Mesh identity unavailable"}
            end
        end)
        if not ok then
            row.available=false;row.body=nil;row.capsule_target_xy_cm,row.body_target_xy_cm,row.body_capsule_delta=nil,nil,nil
            row.reason=tostring(why):gsub("[%c]"," "):sub(1,192)
        end
        local finished=env.now()
        if finite(started) and finite(finished) and finished>=started then row.capture_elapsed_ms=(finished-started)*1000 end
        row.scope_current=current()
        return row,current
    end
    function env.prop_get(p, k) local v; pcall(function() v = p[k] end); return v end
    function env.prop_set(p, k, v) return pcall(function() p[k] = v end) end

    local cdo_cache = nil   -- plain numbers, not a UObject
    function env.cdo_vitals()
        if cdo_cache then return cdo_cache end
        local cdo
        pcall(function() cdo = StaticFindObject("/Game/Character/Blueprints/Willie_BP.Default__Willie_BP_C") end)
        if not cdo or not cdo:IsValid() then return nil end
        local t, n = {}, 0
        for _, k in ipairs(SP.VITALS_FIELDS) do
            local v; pcall(function() v = tonumber(cdo[k]) end)
            if v then t[k] = v; n = n + 1 end
        end
        if n > 0 then cdo_cache = t end
        return cdo_cache
    end

    -- Downward trace: world geometry first (WorldStatic = object type 0, so a
    -- Willie standing on the point is never the "floor"), then Visibility.
    function env.ground(ctxobj, x, y, z)
        local ksl = UEH.GetKismetSystemLibrary()
        if not ksl or not ksl:IsValid() then return nil end
        local s = { X = x, Y = y, Z = z + 150 }
        local e = { X = x, Y = y, Z = z - 5000 }
        local clr = { R = 0, G = 0, B = 0, A = 0 }
        local function hit_z(hit)
            local hz
            pcall(function() hz = hit.ImpactPoint.Z end)
            if not hz then pcall(function() hz = hit.Location.Z end) end
            return hz
        end
        local hit, was = {}, false
        pcall(function()
            was = ksl:LineTraceSingleForObjects(ctxobj, s, e, { 0 }, false, {}, 0, hit, true, clr, clr, 0.0)
        end)
        local hz = was and hit_z(hit) or nil
        if not hz then
            hit, was = {}, false
            pcall(function()
                was = ksl:LineTraceSingle(ctxobj, s, e, 0, false, {}, 0, hit, true, clr, clr, 0.0)
            end)
            hz = was and hit_z(hit) or nil
        end
        return hz
    end

    local function capsule(p)
        local r, hh = 34, 88
        pcall(function()
            local c = p.CapsuleComponent
            if c and c:IsValid() then
                r = tonumber(c:GetScaledCapsuleRadius()) or r
                hh = tonumber(c:GetScaledCapsuleHalfHeight()) or hh
            end
        end)
        return math.max(20, math.min(r, 60)), math.max(50, math.min(hh, 110))
    end
    -- A pawn-sized capsule standing on floor_z at (x, y) hits nothing (world
    -- static/dynamic, pawns, physics bodies). 5 cm sweep, 10 cm above the floor.
    function env.clear(p, x, y, floor_z)
        local ksl = UEH.GetKismetSystemLibrary()
        if not ksl or not ksl:IsValid() then return true end
        local r, hh = capsule(p)
        local cz = floor_z + hh + 10
        local clr = { R = 0, G = 0, B = 0, A = 0 }
        local hit, blocked = {}, false
        -- Our own held weapons are not obstacles (bIgnoreSelf covers only the pawn
        -- actor): otherwise a slot on the spot where we stand (Pit: the server's slot 0
        -- IS the native player spawner) reads "blocked" by our own polearm and moves 1.5 m.
        local ignore = {}
        for _, f in ipairs({ "Weapon R", "Weapon L" }) do
            pcall(function() local w = p[f]; if w and w:IsValid() then ignore[#ignore + 1] = w end end)
        end
        -- ... nor are the arena's Willie spawners: their spawn-area Sphere answers
        -- this trace (the hit component is BP_SpawnerPoint_Willies_C_0.Sphere),
        -- so every slot on a native spawn point (the server's slot 0 on
        -- Pit/Yard/LordsHall/EastTower...) would read "blocked" and the pawn
        -- would be hopped 150 cm aside and knocked down by the hop.
        pcall(function()
            for _, sp in pairs(FindAllOf("BP_SpawnerPoint_Willies_C") or {}) do
                if sp and sp:IsValid() then ignore[#ignore + 1] = sp end
            end
        end)
        local ok = pcall(function()
            blocked = ksl:CapsuleTraceSingleForObjects(p, { X = x, Y = y, Z = cz + 5 }, { X = x, Y = y, Z = cz },
                r - 2, hh - 5, { 0, 1, 2, 3 }, false, ignore, 0, hit, true, clr, clr, 0.0)
        end)
        if not ok then return true end
        return not blocked
    end

    function env.others(p)
        local pts = {}
        pcall(function()
            for _, w in pairs(FindAllOf("Willie_BP_C") or {}) do
                if w and w:IsValid() and w ~= p then
                    local l; pcall(function() l = w:K2_GetActorLocation() end)
                    if l then pts[#pts + 1] = { l.X, l.Y, l.Z } end
                end
            end
        end)
        return pts
    end

    -- Move the WHOLE pawn: the actor (capsule + attached components) with a
    -- physics teleport, then every mesh component that did not follow (a
    -- simulating mesh is detached from the capsule) is moved rigidly by the
    -- same offset, and all body velocities are zeroed. Physics simulation is
    -- never switched on or off here: switching it off detaches a body for
    -- good, switching it on turns a kinematic copy into a falling ragdoll.
    local MESH_FIELDS = { "Mesh", "SK_Skeleton", "BoneCore", "DriverSkeleton" }
    -- Willie_BP's own PhysicsHandles hold the pelvis / spine to targets it
    -- keeps in WORLD space and re-derives each tick from the previous target
    -- (bytecode: LowerBody target = VInterpTo(previous target in Mesh space,
    -- capsule point)). A teleport must carry those targets along, or the
    -- handles drag the body back toward where it was.
    local HANDLE_FIELDS = { "PhysicsHandle LowerBody", "PhysicsHandle UpperBody", "PhysicsHandle GetUp" }
    local function shift_handles(p, ox, oy, oz)
        local n = 0
        for _, f in ipairs(HANDLE_FIELDS) do
            pcall(function()
                local h = p[f]
                if not (h and h:IsValid()) then return end
                local grabbed = false
                pcall(function() local gc = h.GrabbedComponent; grabbed = gc ~= nil and gc:IsValid() end)
                if not grabbed then return end
                local loc, rot = {}, {}
                h:GetTargetLocationAndRotation(loc, rot)
                if loc.X then
                    h:SetTargetLocation({ X = loc.X + ox, Y = loc.Y + oy, Z = loc.Z + oz })
                    n = n + 1
                end
            end)
        end
        return n
    end
    -- Willie_BP world-space vectors its step/balance controller carries from
    -- frame to frame (bytecode: foot plant points, step spline ends, the last
    -- grounded position). Left at the old place they make the body walk on
    -- toward / past the spot after the jump; they move with the teleport.
    local WORLD_VECS = { "R Foot On Ground Loc", "L Foot On Ground Loc", "R Step Start Position", "R Step End Position",
                         "L Step Start Position", "L Step End Position", "Last On Ground Position", "LastPosition" }
    local function shift_vectors(p, ox, oy, oz)
        local n = 0
        for _, f in ipairs(WORLD_VECS) do
            pcall(function()
                local v = p[f]
                if v and v.X then
                    p[f] = { X = v.X + ox, Y = v.Y + oy, Z = v.Z + oz }
                    n = n + 1
                end
            end)
        end
        return n
    end
    -- Every OTHER scene component (foot/knee IK targets, step splines, IK
    -- scenes, ...) whose world location before the actor move is given in
    -- `before` and that did not follow the actor (absolute or detached), moved
    -- by the same offset. Returns the count.
    local SCENE_CLS = nil
    local function scene_components(p)
        local out = {}
        pcall(function()
            SCENE_CLS = SCENE_CLS or StaticFindObject("/Script/Engine.SceneComponent")
            if not (SCENE_CLS and SCENE_CLS:IsValid()) then return end
            local arr = p:K2_GetComponentsByClass(SCENE_CLS)
            arr:ForEach(function(_, el)
                local c = el:get()
                if c and c:IsValid() then out[#out + 1] = c end
            end)
        end)
        return out
    end
    local function locs_of(list)
        local t = {}
        for i, c in ipairs(list) do
            local l; pcall(function() l = c:K2_GetComponentLocation() end)
            t[i] = l and { X = l.X, Y = l.Y, Z = l.Z } or nil
        end
        return t
    end
    local function carry_left_behind(list, before, ox, oy, oz, skip)
        local n = 0
        for i, c in ipairs(list) do
            local b = before[i]
            if b and not skip[c:GetAddress()] then
                local l; pcall(function() l = c:K2_GetComponentLocation() end)
                if l and math.abs(l.X - (b.X + ox)) + math.abs(l.Y - (b.Y + oy)) + math.abs(l.Z - (b.Z + oz)) > 30 then
                    pcall(function() c:K2_SetWorldLocation({ X = b.X + ox, Y = b.Y + oy, Z = b.Z + oz }, false, {}, true) end)
                    n = n + 1
                end
            end
        end
        return n
    end
    -- "Weapon R" / "Weapon L" are separate simulating actors held by
    -- grip constraints. A 3-13 m placement (or a Live fall re-place) that
    -- moves only the pawn lets the constraint drag the weapon across the
    -- arena or break (the player starts disarmed or with a flung polearm).
    -- Their locations are taken BEFORE the pawn moves; afterwards each one
    -- that did not follow is moved by the same offset, and its bodies stop.
    -- Bare hands (Weapon_Fists / Weapon_Feet) are part of the pawn: skipped.
    local WEAPON_FIELDS = { "Weapon R", "Weapon L" }
    local ZERO = { X = 0, Y = 0, Z = 0 }
    local function held_weapons(p)
        local out = {}
        for _, k in ipairs(WEAPON_FIELDS) do
            pcall(function()
                local w = p[k]
                if not (w and w:IsValid()) then return end
                local cn = ""
                pcall(function() cn = w:GetClass():GetFName():ToString() end)
                if cn:find("Weapon_Fists", 1, true) or cn:find("Weapon_Feet", 1, true) then return end
                local l = w:K2_GetActorLocation()
                if l and l.X then out[#out + 1] = { k = k, w = w, X = l.X, Y = l.Y, Z = l.Z } end
            end)
        end
        return out
    end
    local function stop_actor(a)
        pcall(function()
            local r = a:K2_GetRootComponent()
            if r and r:IsValid() then
                r:SetAllPhysicsLinearVelocity(ZERO, false)
                r:SetAllPhysicsAngularVelocityInDegrees(ZERO, false)
            end
        end)
    end
    local function carry_weapons(list, ox, oy, oz, tol)
        local n = 0
        for _, e in ipairs(list) do
            pcall(function()
                local tx, ty, tz = e.X + ox, e.Y + oy, e.Z + oz
                local l = e.w:K2_GetActorLocation()
                if not (l and l.X) or math.abs(l.X - tx) + math.abs(l.Y - ty) + math.abs(l.Z - tz) > (tol or 30) then
                    e.w:K2_SetActorLocation({ X = tx, Y = ty, Z = tz }, false, {}, true)
                    n = n + 1
                end
            end)
            stop_actor(e.w)
        end
        return n
    end
    env.held_weapons, env.carry_weapons = held_weapons, carry_weapons   -- (tests)

    -- On every attempt, including the first, check the physical pelvis after
    -- the component moved or followed. Apply the existing rigid residual
    -- correction once when the simulated body did not accompany that move.
    function env.teleport(p, dest, yaw, try)
        local a0; pcall(function() a0 = p:K2_GetActorLocation() end)
        if not a0 then return "no actor location" end
        local ox, oy, oz = dest.X - a0.X, dest.Y - a0.Y, dest.Z - a0.Z
        local comps = {}
        for _, f in ipairs(MESH_FIELDS) do
            local m; pcall(function() m = p[f] end)
            if m and m:IsValid() then
                local l; pcall(function() l = m:K2_GetComponentLocation() end)
                if l then
                    local pl, err, sim = pelvis(m)
                    comps[#comps + 1] = { f = f, m = m, x = l.X, y = l.Y, z = l.Z,
                        pel = pl, pel_err = err, sim = sim }
                end
            end
        end
        local skip = {}
        for _, c in ipairs(comps) do pcall(function() skip[c.m:GetAddress()] = true end) end
        local scomps = scene_components(p)
        local sbefore = locs_of(scomps)
        local weapons = held_weapons(p)
        pcall(function() p:K2_SetActorLocation(dest, false, {}, true) end)
        if yaw then
            local rot = { Pitch = 0, Yaw = yaw, Roll = 0 }
            pcall(function() p:K2_SetActorRotation(rot, true) end)
            pcall(function()
                local pc = UEH.GetPlayerController()
                if pc and pc:IsValid() and pc.Pawn == p then pc:SetControlRotation(rot) end
            end)
        end
        local parts = {}
        for _, c in ipairs(comps) do
            local l; pcall(function() l = c.m:K2_GetComponentLocation() end)
            local tx, ty, tz = c.x + ox, c.y + oy, c.z + oz
            local lag = l and math.sqrt((l.X - tx) ^ 2 + (l.Y - ty) ^ 2 + (l.Z - tz) ^ 2) or math.huge
            local note
            if lag > 30 then
                pcall(function() c.m:K2_SetWorldLocation({ X = tx, Y = ty, Z = tz }, false, {}, true) end)
                note = string.format("%s moved(%.0fcm)", c.f, lag == math.huge and -1 or lag)
            else
                note = c.f .. " followed"
            end
            local pl, err, sim = pelvis(c.m)
            if c.pel_err or err then
                note = note .. "; body unavailable: " .. (c.pel_err or err)
            elseif c.sim ~= sim then
                note = note .. "; body unavailable: pelvis simulation state changed"
            elseif c.pel and pl and sim then
                local rx, ry, rz = c.pel.X + ox - pl.X, c.pel.Y + oy - pl.Y, c.pel.Z + oz - pl.Z
                local res = math.sqrt(rx * rx + ry * ry + rz * rz)
                if res > 100 then
                    -- Read the component again: the moved branch may have
                    -- changed it already. Never apply the actor offset twice.
                    local current; pcall(function() current = c.m:K2_GetComponentLocation() end)
                    if current then
                        local moved = pcall(function()
                            c.m:K2_SetWorldLocation({ X = current.X + rx, Y = current.Y + ry, Z = current.Z + rz }, false, {}, true)
                        end)
                        local after, after_err, after_sim = pelvis(c.m)
                        if moved and after and after_sim then
                            local remaining = math.sqrt((c.pel.X + ox - after.X) ^ 2
                                + (c.pel.Y + oy - after.Y) ^ 2 + (c.pel.Z + oz - after.Z) ^ 2)
                            note = note .. string.format("; %s(%.0fcm), physical residual(%.0fcm)",
                                remaining <= 100 and "bodies snapped" or "body correction unresolved", res, remaining)
                        else
                            note = note .. "; body correction unavailable" .. (after_err and (": " .. after_err) or "")
                        end
                    else
                        note = note .. "; body correction unavailable: component location"
                    end
                else
                    note = note .. string.format("; physical residual(%.0fcm)", res)
                end
            end
            parts[#parts + 1] = note
            pcall(function() c.m:SetAllPhysicsLinearVelocity({ X = 0, Y = 0, Z = 0 }, false) end)
            pcall(function() c.m:SetAllPhysicsAngularVelocityInDegrees({ X = 0, Y = 0, Z = 0 }, false) end)
        end
        local nh = shift_handles(p, ox, oy, oz)
        if nh > 0 then parts[#parts + 1] = string.format("%d handle target(s) moved", nh) end
        local nc = carry_left_behind(scomps, sbefore, ox, oy, oz, skip)
        if nc > 0 then parts[#parts + 1] = string.format("%d left-behind component(s) carried", nc) end
        local nv = shift_vectors(p, ox, oy, oz)
        if nv > 0 then parts[#parts + 1] = string.format("%d step vector(s) shifted", nv) end
        if #weapons > 0 then
            local nw = carry_weapons(weapons, ox, oy, oz)
            parts[#parts + 1] = string.format("%d held weapon(s) carried, %d followed", nw, #weapons - nw)
        end
        return "[" .. table.concat(parts, ", ") .. "]"
    end

    -- Hold the pawn on dest (P:hold_step, every tick for hold_s after a
    -- teleport): every body's velocity zeroed; when the actor drifted more
    -- than tol_cm (XY) from dest, the actor and every mesh are moved back
    -- rigidly by that residual (XY only: the body settles on the floor).
    -- Returns true when it moved the pawn.
    function env.hold(p, dest, tol_cm)
        local a; pcall(function() a = p:K2_GetActorLocation() end)
        if not a then return false end
        local rx, ry = dest.X - a.X, dest.Y - a.Y
        local moved = false
        local meshes = {}
        for _, f in ipairs(MESH_FIELDS) do
            local m; pcall(function() m = p[f] end)
            if m and m:IsValid() then meshes[#meshes + 1] = m end
        end
        local weapons = held_weapons(p)
        if math.sqrt(rx * rx + ry * ry) > (tol_cm or 10) then
            local before = {}
            for i, m in ipairs(meshes) do
                local l; pcall(function() l = m:K2_GetComponentLocation() end)
                before[i] = l and { X = l.X, Y = l.Y, Z = l.Z } or nil
            end
            pcall(function() p:K2_SetActorLocation({ X = a.X + rx, Y = a.Y + ry, Z = a.Z }, false, {}, true) end)
            carry_weapons(weapons, rx, ry, 0, 5)   -- held weapons too (the mesh tolerance below)
            for i, m in ipairs(meshes) do
                local b = before[i]
                local l; pcall(function() l = m:K2_GetComponentLocation() end)
                if b and l and math.abs(l.X - (b.X + rx)) + math.abs(l.Y - (b.Y + ry)) > 5 then
                    pcall(function() m:K2_SetWorldLocation({ X = b.X + rx, Y = b.Y + ry, Z = b.Z }, false, {}, true) end)
                end
            end
            shift_handles(p, rx, ry, 0)
            shift_vectors(p, rx, ry, 0)
            moved = true
        end
        for _, m in ipairs(meshes) do
            pcall(function() m:SetAllPhysicsLinearVelocity({ X = 0, Y = 0, Z = 0 }, false) end)
            pcall(function() m:SetAllPhysicsAngularVelocityInDegrees({ X = 0, Y = 0, Z = 0 }, false) end)
        end
        for _, e in ipairs(weapons) do stop_actor(e.w) end   -- held still with the pawn
        return moved
    end

    -- Native BP_SpawnerPoint_Willies (diagnostics + the no-order fallback).
    local np_world, np_pts = nil, nil
    function env.native_points()
        local wk = (ctx.world() or {}).key
        if np_pts and np_world == wk then return np_pts end
        local pts = {}
        pcall(function()
            for _, sp in pairs(FindAllOf("BP_SpawnerPoint_Willies_C") or {}) do
                if sp and sp:IsValid() then
                    local l, r, is_player, team
                    pcall(function() l = sp:K2_GetActorLocation() end)
                    pcall(function() r = sp:K2_GetActorRotation() end)
                    pcall(function() is_player = (sp["Spawn Player"] == true) end)
                    pcall(function() team = sp["Spawn Team Int"] end)
                    -- unplaced templates sit at the world origin
                    if l and (math.abs(l.X) + math.abs(l.Y) + math.abs(l.Z)) > 1 then
                        pts[#pts + 1] = { X = l.X, Y = l.Y, Z = l.Z, Yaw = (r and r.Yaw) or 0,
                                          player = is_player and true or false, team = tonumber(team) or 0 }
                    end
                end
            end
        end)
        table.sort(pts, function(a, b)
            if a.player ~= b.player then return a.player end
            if a.X ~= b.X then return a.X < b.X end
            return a.Y < b.Y
        end)
        if #pts > 0 then np_world, np_pts = wk, pts end   -- empty lists are not cached
        return pts
    end
    function env.native_floor()
        local lo
        for _, p in ipairs(env.native_points()) do if not lo or p.Z < lo then lo = p.Z end end
        return lo
    end
    function env.native_point()
        local p = env.native_points()[1]
        if p then return p.X, p.Y, p.Z, p.Yaw end
        return nil
    end
    function env.drop_native() np_world, np_pts = nil, nil end

    -- --- spawn protection: no collision with other Willies ------------------------------
    -- CollisionDisabler (project plugin, /Script/CollisionDisabler) is the
    -- per-pair physics filter Willie_BP itself uses (bytecode:
    -- DisableCollision_SkeletalVsSkeletal(self.SK_Skeleton, self.Rigid Bones,
    -- self.Mesh, self.Rigid Bones, -1.0) and _SingleBodyVsSingleBody on weapon
    -- BaseMesh/Scabbard pairs). TimeToExpiration -1 = until re-enabled.
    -- Channel responses would also drop collision with every other pawn and
    -- weapon, and SetActorEnableCollision(false) would drop the floor (the
    -- ragdoll falls through), so neither is used.
    local CD_PATH = "/Script/CollisionDisabler.Default__CollisionDisablerFunctionLibrary"
    local function cd_lib()
        local c; pcall(function() c = StaticFindObject(CD_PATH) end)
        if c and c:IsValid() then return c end
        return nil
    end
    local function addr(o) local a = "?"; pcall(function() a = tostring(o:GetAddress()) end); return a end
    local function obj_ok(o) local ok = false; pcall(function() ok = o ~= nil and o:IsValid() end); return ok end
    local function skeletals(w)
        local out = {}
        for _, f in ipairs({ "Mesh", "SK_Skeleton" }) do
            local m; pcall(function() m = w[f] end)
            if obj_ok(m) then out[#out + 1] = m end
        end
        return out
    end
    local function wclass(x)
        local c; pcall(function() c = x:GetClass():GetFName():ToString() end)
        return c
    end
    -- Held weapon bodies (BaseMesh, Scabbard); fists/feet have none we care about.
    local function weapon_bodies(w)
        local out = {}
        for _, f in ipairs({ "Weapon R", "Weapon L" }) do
            local x; pcall(function() x = w[f] end)
            if obj_ok(x) then
                local c = wclass(x) or ""
                if not c:find("Weapon_Fists") and not c:find("Weapon_Feet") then
                    for _, b in ipairs({ "BaseMesh", "Scabbard" }) do
                        local m; pcall(function() m = x[b] end)
                        if obj_ok(m) then out[#out + 1] = m end
                    end
                end
            end
        end
        return out
    end
    -- Bone lists: the pawn's own "Rigid Bones" TArray<FName> (what Willie_BP
    -- passes). If the reflection layer refuses a TArray as a parameter, a Lua
    -- table of FNames is used from then on.
    local bones_as_table = false
    local function rigid_bones(w)
        local b; pcall(function() b = w["Rigid Bones"] end)
        local n = 0; pcall(function() n = b:GetArrayNum() end)
        if bones_as_table and b then
            local t = {}
            pcall(function() b:ForEach(function(_, e) t[#t + 1] = e:get() end) end)
            return t, n
        end
        return b, n
    end

    function env.actor_loc(a)
        local l; pcall(function() l = a:K2_GetActorLocation() end)
        if l then return l.X, l.Y, l.Z end
        return nil
    end

    -- Every other Willie of the world, with a signature of the bodies that a
    -- disable has to cover (its meshes and weapons, and ours): a re-armed
    -- weapon changes the signature and gets covered too.
    function env.willies(p)
        local out = {}
        local mine = {}
        for _, m in ipairs(skeletals(p)) do mine[#mine + 1] = addr(m) end
        for _, m in ipairs(weapon_bodies(p)) do mine[#mine + 1] = addr(m) end
        local my_sig = table.concat(mine, ",")
        pcall(function()
            for _, w in pairs(FindAllOf("Willie_BP_C") or {}) do
                if obj_ok(w) and w ~= p and addr(w) ~= addr(p) then
                    local s = {}
                    for _, m in ipairs(skeletals(w)) do s[#s + 1] = addr(m) end
                    for _, m in ipairs(weapon_bodies(w)) do s[#s + 1] = addr(m) end
                    local name = "?"; pcall(function() name = w:GetFName():ToString() end)
                    out[#out + 1] = { key = addr(w), actor = w, name = name, sig = table.concat(s, ",") .. "|" .. my_sig }
                end
            end
        end)
        return out
    end

    function env.set_collision(p, o, enabled)
        local lib = cd_lib()
        local n, fails, errs = 0, 0, {}
        local function call(fn, ...)
            local args = { ... }
            local ok, err = pcall(function() lib[fn](lib, table.unpack(args)) end)
            if ok then n = n + 1 else fails = fails + 1; if #errs < 2 then errs[#errs + 1] = fn .. ": " .. tostring(err) end end
        end
        -- Capsules / character movement sweeps (both ways).
        local caps = 0
        for _, pair in ipairs({ { p, o }, { o, p } }) do
            pcall(function()
                local c = pair[1].CapsuleComponent
                if c and c:IsValid() then c:IgnoreActorWhenMoving(pair[2], not enabled); caps = caps + 1 end
            end)
        end
        if not lib then return caps > 0, string.format("CollisionDisabler not found; capsules=%d", caps) end
        local bp, nbp = rigid_bones(p)
        local bo, nbo = rigid_bones(o)
        if not bones_as_table and bp and bo then
            -- Probe the TArray form once on the first skeletal pair.
            local a, b = skeletals(p)[1], skeletals(o)[1]
            if a and b then
                local fn = enabled and "EnableCollision_SkeletalVsSkeletal" or "DisableCollision_SkeletalVsSkeletal"
                local ok = pcall(function()
                    if enabled then lib[fn](lib, a, bp, b, bo) else lib[fn](lib, a, bp, b, bo, -1.0) end
                end)
                if not ok then
                    bones_as_table = true
                    bp, nbp = rigid_bones(p)
                    bo, nbo = rigid_bones(o)
                end
            end
        end
        local sp, so = skeletals(p), skeletals(o)
        local wp, wo = weapon_bodies(p), weapon_bodies(o)
        local TTL = -1.0
        for _, a in ipairs(sp) do
            for _, b in ipairs(so) do
                if enabled then call("EnableCollision_SkeletalVsSkeletal", a, bp, b, bo)
                else call("DisableCollision_SkeletalVsSkeletal", a, bp, b, bo, TTL) end
            end
            for _, wb in ipairs(wo) do
                if enabled then call("EnableCollision_SkeletalVsSingleBody", a, bp, wb)
                else call("DisableCollision_SkeletalVsSingleBody", a, bp, wb, TTL) end
            end
        end
        for _, b in ipairs(so) do
            for _, wa in ipairs(wp) do
                if enabled then call("EnableCollision_SkeletalVsSingleBody", b, bo, wa)
                else call("DisableCollision_SkeletalVsSingleBody", b, bo, wa, TTL) end
            end
        end
        for _, wa in ipairs(wp) do
            for _, wb in ipairs(wo) do
                if enabled then call("EnableCollision_SingleBodyVsSingleBody", wa, wb)
                else call("DisableCollision_SingleBodyVsSingleBody", wa, wb, TTL) end
            end
        end
        -- Evidence: does the plugin report the pelvis pair as disabled now?
        local check = "?"
        if sp[1] and so[1] then
            pcall(function()
                local pel = FName("pelvis")
                check = tostring(lib:IsCollisionDisabled(sp[1], so[1], pel, pel))
            end)
        end
        local detail = string.format("calls=%d failed=%d bones=%d/%d skel=%d/%d weapons=%d/%d capsules=%d pelvis_disabled=%s%s",
            n, fails, nbp, nbo, #sp, #so, #wp, #wo, caps, check,
            #errs > 0 and (" [" .. table.concat(errs, "; ") .. "]") or "")
        return n > 0 and fails == 0, detail
    end

    function env.pawn_state(p)
        local s = {}
        pcall(function() s.consciousness = tonumber(p.Consciousness) end)
        pcall(function() s.downed = p.Downed == true end)
        pcall(function() s.fallen = p.Fallen == true end)
        pcall(function() s.health = tonumber(p.Health) end)
        pcall(function()
            local value = p["Block Spine Breaking"]
            if type(value) == "boolean" then s.block_spine_breaking = value end
        end)
        for side, f in pairs({ weapon_r = "Weapon R", weapon_l = "Weapon L" }) do
            local x; pcall(function() x = p[f] end)
            s[side] = obj_ok(x) and (wclass(x) or "?") or "None"
        end
        if s.consciousness then s.consciousness = math.floor(s.consciousness * 10 + 0.5) / 10 end
        if s.health then s.health = math.floor(s.health * 10 + 0.5) / 10 end
        return s
    end

    -- Structured events (shared/hsmp_log.lua); a no-op when it is not deployed.
    local HL
    do
        local ok, m = pcall(require, "hsmp_log")
        if not (ok and type(m) == "table") then
            local src = (debug.getinfo(1, "S").source or ""):gsub("^@", "")
            local dir = src:match("^(.*)[/\\]") or "."
            for _, pth in ipairs({ dir .. "/hsmp_log.lua", dir .. "/../../shared/hsmp_log.lua" }) do
                local ok2, m2 = pcall(dofile, pth)
                if ok2 and type(m2) == "table" then m = m2; ok = true; break end
            end
        end
        if ok and type(m) == "table" then
            HL = m
            if HL.init then pcall(HL.init, { mod = "HSMPSync", state_dir = ctx.state_dir }) end
        end
    end
    function env.ev(name, fields)
        if HL and HL.event then pcall(HL.event, name, fields or {}) end
    end
    return env
end

return SP
