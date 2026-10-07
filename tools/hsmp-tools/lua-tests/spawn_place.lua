-- HSMPSync spawn placement offline tests (HSMPSync/Scripts/spawn_place.lua).
--
--   hsmp-tools lua-test spawn_place
--
-- Drives the placer with an in-memory world: a clock, files, a pawn whose
-- teleport can "stick" or snap back to the native spawner (as seen on
-- Slums), floors, blockers and other Willies.
--
-- Covered: the placement -> verification -> .spawn_status.json handshake
-- (verified=false on every attempt, verified=true only once the pawn stayed
-- there), snap-back re-tries and the final failure, the Director's
-- .spawn_request.json retry (and stale/mismatched requests), spawn protection
-- (Invulnerable from the first tick, vitals top-up, bounded, released once,
-- not over a DED pawn), the fall watchdog (re-place without a death), a new
-- pawn, the spiral offset, no ground, no order (single player untouched) and
-- the server report verb.

local SP = dofile(T.path("mods/HSMPSync/Scripts/spawn_place.lua"))
local R = require("hsmp_native_records")
local NS = require("hsmp_native_mock").S
local SD = "S"

do
    local owner = { IsValid=function() return true end }
    local temporary = { IsValid=function() return true end }
    local pc = { IsValid=function() return true end, Pawn=temporary }
    local env = SP.make_ue_env({UEHelpers={},pc=function() return pc end,ai_pawn=function() return owner end})
    T.check(env.pawn()==owner,"real spawn environment preserves verified AI ownership during native stand-in possession")
    local human = SP.make_ue_env({UEHelpers={},pc=function() return pc end,ai_pawn=function() return nil end})
    T.check(human.pawn()==temporary,"real spawn environment keeps ordinary human possession when no verified AI owns the life")
end

local function new_world(opts)
    opts = opts or {}
    local w = {
        clock = 50.0, files = {}, logs = {}, peer = 1, mstate = "countdown", mround = 0,
        wkey = "Map_Arena_Slums#1", wshort = "Map_Arena_Slums", floor = 853, native = { 257, 415, 853, -125 },
        others = {}, blocked = {}, no_ground = false, sticky = opts.sticky ~= false, snap_tries = opts.snap_tries or 0,
        teleports = 0, cdo = { Health = 100, ["Neck Health"] = 100, Consciousness = 100, Bleeding = 0, Pain = 0 },
    }
    local env = {}
    env.now = function() return w.clock end
    env.log = function(fmt, ...) w.logs[#w.logs + 1] = string.format(fmt, ...) end
    -- HSMPAvatars' typed bus key fallback_swap (nil = never written)
    env.fallback_swap = function() return w.fswap and R.deep(w.fswap) or nil end
    -- typed records, marshalled by the native module's table rules (lib/hsmp_native_records.lua)
    env.plan = function() return w.plan end
    env.request = function() return (w.req and w.req.seq ~= 0) and R.deep(w.req) or nil end
    env.put_status = function(t)
        local rec, e = R.marshal(NS, "spawn_status", t)
        assert(rec, "spawn_status record: " .. tostring(e))
        w.status_rec = rec
        return true
    end
    env.clear_status = function() w.status_rec = R.marshal(NS, "spawn_status", {}) end
    env.director_state = function() return w.director end
    env.send_spawned = function(t)
        local rec, e = R.marshal(NS, "spawned", t)
        assert(rec, "spawned record: " .. tostring(e))
        w.sent[#w.sent + 1] = rec
        return #w.sent
    end
    w.sent = {}
    env.world = function() return { key = w.wkey, short = w.wshort } end
    env.my_peer_id = function() return w.peer end
    env.match = function() return w.mstate, w.mround end
    env.pawn = function() return w.pawn end
    env.pawn_id = function(p) return p.id end
    env.pawn_begun = function(p) return p.begun ~= false end
    env.pawn_loc = function(p) return p.x, p.y, p.z end
    env.body_loc = function(p) return p.bx or p.x, p.by or p.y, p.z end
    env.ground = function(_, x, y, z)
        if w.no_ground then return nil end
        if w.floor_at then return w.floor_at(x, y) end
        return w.floor
    end
    env.clear = function(_, x, y)
        for _, b in ipairs(w.blocked) do
            if math.abs(b[1] - x) < 1 and math.abs(b[2] - y) < 1 then return false end
        end
        return not w.all_blocked
    end
    env.others = function() return w.others end
    -- The bug: the capsule moves but the ragdoll does not, so the actor snaps
    -- back to where the body is (the native spawner) on the next frames.
    env.teleport = function(p, dest, yaw, try)
        w.teleports = w.teleports + 1
        w.tries_seen = w.tries_seen or {}
        w.tries_seen[#w.tries_seen + 1] = try
        -- the catch-up walk: the body travels on in the direction of the jump
        if w.drift then
            local dx, dy = dest.X - p.x, dest.Y - p.y
            local d = math.max(1, math.sqrt(dx * dx + dy * dy))
            p.drift = { vx = 250 * dx / d, vy = 250 * dy / d, t = 0.7 }
        end
        p.x, p.y, p.z, p.yaw = dest.X, dest.Y, dest.Z, yaw
        if not w.sticky or w.teleports <= w.snap_tries then p.snap_back = true else p.snap_back = false end
        return "[Mesh moved(1300cm)]"
    end
    env.prop_get = function(p, k) return p.props[k] end
    w.prop_writes = {}
    env.prop_set = function(p, k, v)
        w.prop_writes[#w.prop_writes + 1] = { pawn = p, key = k, value = v }
        p.props[k] = v; return true
    end
    env.cdo_vitals = function() return w.cdo end
    env.native_floor = function() return w.native[3] end
    env.native_point = function() return table.unpack(w.native) end
    w.env = env

    -- the server's orders as the session view carries them (HS.view().spawns)
    function w:spawns(round, arena, x, y, z, protect_ms)
        self.plan = SP.plan_from_view({ spawn_round = round, arena = arena, seq = 3, spawns = {
            [1] = { spawn_id = round * 256, slot = 4, x = x, y = y, z = z, yaw = -144.6, protect_ms = protect_ms or 0 },
            [2] = { spawn_id = round * 256 + 1, slot = 7, x = -561.3, y = -516.1, z = 886.1, yaw = 35.4, protect_ms = protect_ms or 0 },
        } })
    end
    -- a Director request (bus `spawn_request`) given as the old JSON line
    function w:request_json(s) self.req = R.marshal(NS, "spawn_request", T.json_decode(s)) end
    -- the `spawned` reports in the old verb form spawned:<round>:<slot>:<x>:<y>:<z>:<clear>
    function w:verbs()
        local o = {}
        for _, r in ipairs(self.sent) do
            o[#o + 1] = string.format("spawned:%d:%d:%.0f:%.0f:%.0f:%d", r.round, r.slot, r.pos[1], r.pos[2], r.pos[3], r.clear and 1 or 0)
        end
        return o
    end
    w.npawn = 0
    function w:new_pawn()
        self.npawn = self.npawn + 1
        self.pawn = { id = "Willie_BP_C_" .. (100 + self.npawn), x = 257, y = 415, z = 953, home = { 257, 415, 953 },
                      props = { Health = 100, ["Neck Health"] = 100, Invulnerable = false, DED = false,
                                ["Block Spine Breaking"] = false } }
        return self.pawn
    end
    function w:tick(n, dt)
        for _ = 1, (n or 1) do
            self.clock = self.clock + (dt or 1 / 30)
            -- a snapped-back pawn returns to its body a few frames after the teleport
            local p = self.pawn
            if p and p.snap_back then p.x, p.y, p.z = p.home[1], p.home[2], p.home[3]; p.snap_back = false end
            if p and p.drift and p.drift.t > 0 then
                local dt0 = dt or 1 / 30
                p.x, p.y = p.x + p.drift.vx * dt0, p.y + p.drift.vy * dt0
                p.drift.t = p.drift.t - dt0
            end
            self.sp:tick()
        end
    end
    function w:secs(s) self:tick(math.floor(s * 30 + 0.5)) end
    -- countdown round 0 -> live round 1 (the plan for round 1 stays valid)
    function w:go_live() self.mstate, self.mround = "live", 1 end
    -- the status record seen through the old JSON shape (absent values nil)
    function w:status()
        local r = self.status_rec
        if not r or r.seq == 0 then return nil end
        local s = R.deep(r)
        if r.has_dest then s.x, s.y, s.z = r.pos[1], r.pos[2], r.pos[3] end
        s.floor = r.has_floor and r.floor or nil
        s.protect_until = r.has_protect_until and r.protect_until or nil
        s.error = r.error ~= "" and r.error or nil
        s.spawn_id = r.spawn_id ~= 0 and r.spawn_id or nil
        s.slot = r.slot >= 0 and r.slot or nil
        return s
    end
    function w:logtext() return table.concat(self.logs, "\n") end
    function w:start() self.sp = SP.new(self.env, { state_dir = SD }); return self.sp end
    return w
end

local function placed_world(opts)
    local w = new_world(opts)
    w:spawns(1, "Map_Arena_Slums", 517.0, 250.4, 855.3, opts and opts.protect_ms)
    w:new_pawn()
    w:start()
    return w
end

-- ---------------------------------------------------------------------------
T.log("== pure helpers")
do
    -- The plan from a real session record, through the facade and shared/hsmp_session.lua.
    local NM = require("hsmp_native_mock")
    local N = NM.new{}
    local IPC = dofile(T.path("mods/shared/hsmp_ipc.lua"))
    IPC.init{ mod = "Test", native = N, log = function() end }
    local HS = dofile(T.path("mods/shared/hsmp_session.lua"))
    N.sc_put("link", { status = 1, state = 1, my_peer_id = 1 })
    local function row(seat, peer, sid, slot, pos, yaw, pms)
        return { seat = seat, peer_id = peer, connected = true, alive = true, spawn_id = sid, spawn_slot = slot,
                 spawn_pos = pos, spawn_yaw = yaw, spawn_protect_ms = pms }
    end
    N.sc_put("session", { epoch = 5, seq = 9, round = 1, phase = NS.ENUMS.phase.COUNTDOWN, winner_seat = 255, has_frozen = true,
        config = { arena = "Map_Arena_Yard" }, frozen = { arena = "Map_Arena_Pit" },
        rows = { row(1, 1, 512, 4, { 517.0, -250.4, 855.3 }, -144.6, 2500), row(2, 2, 513, 7, { 1, 2, 3 }, 0, 0) } })
    local p = SP.plan_from_view(HS.view({ ipc = IPC }))
    T.check(p and p.round == 2 and p.arena == "Map_Arena_Pit" and #p.list == 2 and p.seq == 9,
        "plan_from_view: round (spawn_id >> 8), the frozen arena, entries", T.repr(p))
    local e = p.by_peer[1]
    T.check(e.spawn_id == 512 and e.slot == 4 and math.abs(e.y + 250.4) < 1e-3 and math.abs(e.yaw + 144.6) < 1e-3
        and e.protect_ms == 2500, "plan entry incl. slot / protect_ms (f32 values)", T.repr(e))
    T.check(SP.plan_from_view(nil) == nil and SP.plan_from_view({ spawns = {} }) == nil, "no view / no orders -> no plan")
    T.check(SP.wanted_round("countdown", 1) == 2 and SP.wanted_round("paused", 1) == 2
        and SP.wanted_round("live", 1) == 1 and SP.wanted_round("roundover", 1) == 1, "wanted_round")
    T.check(SP.protect_ms({ protect_ms = 0 }) == SP.T.protect_min_ms, "protect_ms 0 -> the client floor")
    T.check(SP.protect_ms({ protect_ms = 5000 }) == 5000, "protect_ms from the server")
    T.check(SP.protect_ms({ protect_ms = 99999 }) == SP.T.protect_max_ms, "protect_ms clamped")
end

T.log("== round start: protect at once, place after the settle, verify, then report to the Director")
do
    local w = placed_world()
    w:tick(1)
    T.check(w.sp:protected(), "protected from the first tick the pawn exists (before placement)")
    T.check(w.pawn.props.Invulnerable == true, "Invulnerable set on the pawn while protected")
    T.check(w:status() == nil and w.teleports == 0, "nothing moves before the 2 s settle")
    w:secs(2.1)
    local st = w:status()
    T.check(w.teleports == 1 and st and st.verified == false and st.round == 1 and st.arena == "Map_Arena_Slums"
        and st.spawn_id == 256 and st.slot == 4 and st.pawn == w.pawn.id and st.why == "round start" and st.tries == 1,
        ".spawn_status.json written right after the teleport (verified=false)", T.repr(st))
    T.check(math.abs(st.x - 517) < 0.1 and math.abs(st.z - (853 + 100)) < 0.1 and st.floor == 853,
        "status carries the destination (ground-snapped) and the floor", T.repr(st))
    T.check(w.sp:protected() and w.sp:protect_until() == nil, "unbounded protection until the placement is verified")
    w:secs(1.2)
    st = w:status()
    T.check(st.verified == true and st.error == nil and st.seq >= 2, "verified=true once the pawn stayed there", T.repr(st))
    T.check(st.protect_ms == SP.T.protect_min_ms and st.protect_until == nil,
        "countdown: protect_until null (protected until the round goes Live)", T.repr(st))
    -- damage during protection (before Live) is undone
    w.pawn.props.Health = 40
    w:secs(0.6)
    T.check(w.pawn.props.Health == 100, "vitals topped up from the CDO while protected (countdown)")
    local live_t = w.clock
    w:go_live()
    w:tick(1)
    st = w:status()
    T.check(type(st.protect_until) == "number" and st.protect_until <= w.clock + 0.001 and st.protect_until >= live_t
        and st.verified == true, "Live: status rewritten, protect_until = the Live transition (process clock)", T.repr(st))
    T.check(T.contains(w:logtext(), "placement verified round 1 slot 4 id=256"), "log line for the verified placement")
    -- protection ends when the round goes Live (the placement + 3 s floor never reaches into Live)
    T.check(not w.sp:protected() and w.pawn.props.Invulnerable == false, "Live ends the protection at once")
    T.check(T.contains(w:logtext(), "spawn protection ended"), "log line for the end of protection")
    -- the server report
    w:secs(2.5)
    local verbs = w:verbs()
    T.check(#verbs == 3 and verbs[1] == "spawned:1:4:517:250:953:1", "spawned:<round>:<slot>:<x>:<y>:<z>:<clear> sent 3x", T.repr(verbs))
    w:secs(1)
    w.pawn.props.Health = 40
    w:secs(1)
    T.check(w.pawn.props.Health == 40, "no top-up during Live (damage counts)")
    T.check(w.teleports == 1, "exactly one teleport for a clean placement")
end

T.log("== the order's protect_ms is reported; the round going Live still ends the protection")
do
    local w = placed_world({ protect_ms = 8000 })
    w:secs(3.5)
    local st = w:status()
    T.check(st.verified and st.protect_ms == 8000, "protect_ms from the order", T.repr(st))
    w:secs(1)
    T.check(w.sp:protected(), "protected in the countdown")
    w:go_live()
    w:tick(1)
    T.check(not w.sp:protected(), "Live: released at once, not placement + 8 s")
end

T.log("== a DED pawn keeps its native Invulnerable flag at the end of protection")
do
    local w = placed_world()
    w:secs(3.5)
    w:go_live()
    w.pawn.props.DED = true
    w:secs(4)
    T.check(not w.sp:protected() and w.pawn.props.Invulnerable == true, "flag left as is on a dead pawn")
    T.check(T.contains(w:logtext(), "pawn is DED"), "and that is logged")
end

T.log("== snap-back: the move does not stick -> re-teleport, then success on try 2")
do
    local w = placed_world({ snap_tries = 1 })
    w:secs(2.1)
    w:secs(0.6)
    T.check(T.contains(w:logtext(), "placement did NOT stick"), "the snap-back is detected and logged")
    T.check(w.teleports == 2 and w:status().verified == false and w:status().tries == 2, "re-teleported (try 2)", T.repr(w:status()))
    w:secs(1.2)
    T.check(w:status().verified == true and w:status().tries == 2, "verified on try 2", T.repr(w:status()))
end

T.log("== snap-back every time -> FAILED after max_tries, bounded protection")
do
    local w = placed_world({ sticky = false })
    w:secs(2.1)
    w:secs(4)
    local st = w:status()
    T.check(w.teleports == SP.T.max_tries, "exactly max_tries teleports", w.teleports)
    T.check(st.verified == false and T.contains(st.error, "did not stick after 3 tries"), "status: verified=false + error", T.repr(st))
    T.check(T.contains(w:logtext(), "placement FAILED round 1 slot 4"), "failure logged")
    w:go_live()
    w:tick(1)
    st = w:status()
    T.check(type(st.protect_until) == "number" and T.contains(st.error or "", "did not stick"),
        "protection is bounded even when placement fails (status keeps the error)", T.repr(st))
    w:secs(5)
    T.check(not w.sp:protected(), "no endless invulnerability after a failed placement")
    T.check(w.teleports == SP.T.max_tries, "no teleport storm afterwards")
end

T.log("== catch-up walk after a teleport: held on the spot, then verified within the 1 m tolerance")
do
    -- without the hold the body walks ~1.75 m past the spot: never verified
    local w = placed_world()
    w.drift = true
    w:secs(2.1)
    w:secs(4)
    T.check(w:status().verified == false and w.teleports == SP.T.max_tries,
        "no hold: the catch-up walk defeats every try (the gate-run failure)", T.repr(w:status()))
    -- with env.hold (UE: velocities zeroed, a > 10 cm drift moved back rigidly)
    local w2 = placed_world()
    w2.drift = true
    w2.holds = 0
    w2.env.hold = function(p, dest, tol)
        w2.holds = w2.holds + 1
        local dx, dy = dest.X - p.x, dest.Y - p.y
        if math.sqrt(dx * dx + dy * dy) > tol then p.x, p.y = dest.X, dest.Y; return true end
        return false
    end
    w2:secs(2.1)
    local t0 = w2.clock
    w2:secs(SP.T.hold_s - 0.1)
    T.check(w2:status().verified == false and w2.holds > 10, "held every tick for hold_s, no verdict yet", w2.holds)
    w2:secs(1.4)
    local st = w2:status()
    T.check(st.verified == true and st.tries == 1 and w2.teleports == 1, "verified on try 1 after the hold", T.repr(st))
    local d = math.sqrt((w2.pawn.x - st.x) ^ 2 + (w2.pawn.y - st.y) ^ 2)
    T.check(d <= SP.T.verify_tol_cm and SP.T.verify_tol_cm == 100 and st.tol_cm == 100,
        "on the spot (<= 100 cm, tol_cm carried for the gate)", d)
    T.check(T.contains(w2:logtext(), "hold released after"), "hold release logged")
end

T.log("== capsule on the spot but the visible body left behind is NOT placed")
do
    local w = placed_world()
    w.pawn.bx, w.pawn.by = 257, 415                    -- the ragdoll stays on the native spawner
    w:secs(1.6)
    w:secs(3)
    local st = w:status()
    T.check(st.verified == false and T.contains(st.error or "", "body (Mesh pelvis)"),
        "a body far from the destination fails verification, and the error says so", T.repr(st))
    T.check(T.eq(w.tries_seen, { 1, 2, 3 }), "the teleport is told which bounded attempt it is",
        T.repr(w.tries_seen))
end

T.log("== unreadable physical body cannot qualify a capsule-only placement")
do
    local w = placed_world()
    w.env.body_loc = function() return nil, nil, nil, "physical pelvis unavailable" end
    w:secs(5)
    local st = w:status()
    T.check(st and st.verified == false and w.sp.counters.verified == 0,
        "an explicitly unavailable body read never verifies placement", T.repr(st))
    T.check(st.tries == SP.T.max_tries and T.contains(st.error or "", "body unavailable: physical pelvis unavailable"),
        "unavailable physics proof fails after the existing bounded attempts with its reason", T.repr(st))
end

T.log("== Director retry (.spawn_request.json)")
do
    local w = new_world({ sticky = false })
    w:request_json('{"seq":7,"round":1,"arena":"Map_Arena_Slums","pawn":"x","why":"old"}')
    w:spawns(1, "Map_Arena_Slums", 517.0, 250.4, 855.3)
    w:new_pawn()
    w:start()
    w:secs(6.5)
    T.check(w.teleports == SP.T.max_tries and not T.contains(w:logtext(), "Director asked"),
        "a request left over from an earlier run is never acted on")
    w.sticky = true
    w:request_json(string.format(
        '{"seq":8,"round":2,"arena":"Map_Arena_Slums","pawn":"%s","why":"wrong round"}', w.pawn.id))
    w:secs(0.5)
    T.check(T.contains(w:logtext(), "Director request #8 ignored"), "a request for another round is ignored")
    w:request_json(string.format(
        '{"seq":9,"round":1,"arena":"Map_Arena_Slums","pawn":"%s","why":"not confirmed after 8 s"}', w.pawn.id))
    w:secs(0.5)
    T.check(T.contains(w:logtext(), "Director asked for a re-place (#9"), "a matching request triggers a re-place")
    T.check(w:status().why == "director retry" and w:status().tries == 1, "status why=director retry", T.repr(w:status()))
    w:secs(1.2)
    T.check(w:status().verified == true, "the retry is verified", T.repr(w:status()))
    w:secs(2)
    local n = w.teleports
    w:secs(2)
    T.check(w.teleports == n, "the same request is handled once")
end

T.log("== fall watchdog during Live: re-placed WITHOUT a heal or protection")
do
    local w = placed_world()
    w:secs(3.5)
    w:go_live()
    w:secs(4)                                    -- protection over
    T.check(not w.sp:protected(), "protection over before the fall")
    w.pawn.props.Health = 30
    w.pawn.x, w.pawn.y, w.pawn.z = 600, 300, -994   -- fell through the floor
    w.pawn.home = { 517, 250.4, 953 }
    w:secs(1.1)
    T.check(T.contains(w:logtext(), "spawn watchdog: pawn FELL at (600,300,-994)"), "the fall is detected", w:logtext())
    T.check(T.contains(w:logtext(), "(Live: no heal, no protection)"), "the log says how")
    T.check(not w.sp:protected() and w.pawn.props.Invulnerable == false, "no protection re-armed during Live")
    T.check(w.pawn.props.Health == 30, "no heal during Live (a fall is no free reset)")
    T.check(w:status().why == "fell" and w.teleports == 2, "re-placed on the order (why=fell)", T.repr(w:status()))
    w:secs(1.2)
    T.check(w:status().verified == true, "the rescue placement is verified")
    T.check(not w.sp:protected() and w.pawn.props.Invulnerable == false and w.pawn.props.Health == 30,
        "still unprotected and unhealed after the verify", T.repr(w.pawn.props))
end

T.log("== fall watchdog never revives the dead")
do
    for _, case in ipairs({ { "DED", function(p) p.props.DED = true; p.props.Health = 12 end },
                            { "Health 0", function(p) p.props.Health = 0 end } }) do
        local w = placed_world()
        w:secs(3.5)
        w:go_live()
        w:secs(4)
        case[2](w.pawn)
        local hp = w.pawn.props.Health
        w.pawn.x, w.pawn.y, w.pawn.z = 600, 300, -994   -- the corpse falls through the floor
        w.pawn.home = { 600, 300, -994 }
        w:secs(3)
        T.check(w.teleports == 1, case[1] .. ": the corpse is not moved", w.teleports)
        T.check(w.pawn.props.Health == hp and w.pawn.props.Invulnerable == false and not w.sp:protected(),
            case[1] .. ": no heal, no Invulnerable", T.repr(w.pawn.props))
        T.check(T.count(w:logtext(), "but it is dead") == 1, case[1] .. ": logged once")
    end
    -- before Live a living pawn still gets the full rescue
    local w = placed_world()
    w:secs(3.5)
    w.pawn.props.Health = 30
    w.pawn.x, w.pawn.y, w.pawn.z = 600, 300, -994
    w.pawn.home = { 517, 250.4, 953 }
    w:secs(0.4)
    T.check(w.pawn.props.Health == 100 and w.sp:protected() and w:status().why == "fell",
        "countdown: re-placed, healed, protected", T.repr(w:status()))
end

T.log("== a fall during protection is caught early (well above the void)")
do
    local w = placed_world()
    w:secs(3.5)
    T.check(w.sp:protected(), "inside the protection window")
    w.pawn.x, w.pawn.y, w.pawn.z = 517, 250, 853 - 450   -- 4.5 m below the placed floor, far above kill_z
    w.pawn.home = { 517, 250.4, 953 }
    w:secs(0.4)
    T.check(T.contains(w:logtext(), "during spawn protection"), "fall below floor-400 caught while protected")
end

T.log("== pushed off the spawn before Live (protected) -> re-placed; never during Live")
do
    local w = placed_world()
    w:secs(3.5)
    w.pawn.x = w.pawn.x + 500
    w.pawn.home = { 517, 250.4, 953 }
    w:secs(0.4)
    T.check(T.contains(w:logtext(), "pushed off its spawn"), "drift during countdown -> re-place")
    local w2 = placed_world()
    w2:secs(3.5)
    w2.mstate, w2.mround = "live", 1
    w2:spawns(1, "Map_Arena_Slums", 517.0, 250.4, 855.3)
    w2.pawn.x = w2.pawn.x + 500
    w2:secs(0.5)
    T.check(not T.contains(w2:logtext(), "pushed off"), "players walking away during Live are never pulled back")
end

T.log("== the stand-in fallback's transient possession swap keeps the placed pawn")
do
    local w = placed_world()
    w:secs(3.5)
    local placed = w.pawn
    local st0 = w:status()
    T.check(st0 and st0.verified == true, "placed and verified before the fallback", T.repr(st0))
    -- HSMPAvatars opens the swap window, the native spawn possesses its body for a tick
    w.fswap = assert(R.marshal(NS, "fallback_swap", { until_s = w.clock + 3, keep = placed.id }))
    w:new_pawn()
    w:tick(2)
    w.pawn = placed                     -- HSMPAvatars re-possessed the original
    w:secs(1.0)
    T.check(T.contains(w:logtext(), "transient swap"), "the swap is recognised", w:logtext())
    T.check(not T.contains(w:logtext(), "possessed pawn changed"), "no re-placement of the original pawn")
    T.check(w.sp:protected() and w.pawn.props.Invulnerable == true, "the placed pawn keeps its protection")
    -- outside the window a new pawn is a new pawn again
    local w2 = placed_world()
    w2:secs(3.5)
    w2.fswap = assert(R.marshal(NS, "fallback_swap", { until_s = w2.clock - 1, keep = w2.pawn.id }))
    w2:new_pawn()
    w2:secs(0.2)
    T.check(not T.contains(w2:logtext(), "transient swap"), "an expired window is ignored")
end

T.log("== a new pawn gets its own placement and protection")
do
    local w = placed_world()
    w:go_live()
    w:secs(4)
    w.plan.by_peer[1].spawn_id = 386 -- round1, deathmatch life2
    w.director = "Spawn"
    w:new_pawn()
    w:tick(1)
    T.check(w.sp:protected() and w.pawn.props.Invulnerable,
        "server-authorized fresh deathmatch life is protected even though match is already Live")
    T.check(w.sp:protect_until() > w.clock and w.sp:protect_until() <= w.clock+15,
        "live respawn protection is bounded before placement")
    w:secs(3)
    T.check(w.teleports == 2 and w:status().verified and w:status().pawn == w.pawn.id,
        "actual fresh deathmatch pawn receives its order and verifies during Live")
    w.director = "Live"
    w:secs(4)
    T.check(not w.sp:protected() and not w.pawn.props.Invulnerable,
        "verified respawn protection expires without waiting for another Live transition")
    w:new_pawn()
    w:secs(3)
    T.check(w.teleports == 2 and not w.sp:protected(),
        "completed respawn order does not authorize arbitrary later possession resets")
end
do
    local w = placed_world()
    w:secs(3.5)
    w:go_live()
    w:secs(4)
    T.check(not w.sp:protected(), "first pawn's window over")
    w:new_pawn()
    w:tick(1)
    T.check(not w.sp:protected() and w.pawn.props.Invulnerable == false, "a new pawn during Live is not protected")
    w:secs(3.5)
    local st = w:status()
    -- A possession change mid-Live is never teleported to a spawn
    -- (that would be a free mid-fight reset); the watchdog still guards falls.
    T.check(w.teleports == 1 and st.pawn ~= w.pawn.id, "a new pawn during Live is NOT placed", T.repr({ w.teleports, st }))
    T.check(T.contains(w:logtext(), "during Live: not placed (watchdog only)"), "logged")
    T.check(T.contains(w:logtext(), "possessed pawn changed"), "pawn change logged")
    -- the next round's countdown: a new pawn is protected at once
    w.mstate, w.mround = "countdown", 1
    w:spawns(2, "Map_Arena_Slums", 517.0, 250.4, 855.3)
    w:new_pawn()
    w:tick(1)
    T.check(w.sp:protected() and w.pawn.props.Invulnerable == true, "new pawn in the countdown protected at once")
    -- a possession swap DURING protection: the old body loses the flag we set
    local old = w.pawn
    w:new_pawn()
    w:tick(1)
    T.check(old.props.Invulnerable == false and w.pawn.props.Invulnerable == true,
        "swap during protection: old pawn's Invulnerable cleared, new pawn protected")
end

T.log("== blocked point -> spiral offset; blocked everywhere -> server point, clear=false")
do
    local w = placed_world()
    w.blocked = { { 517.0, 250.4 } }
    w:secs(2.1)
    local st = w:status()
    T.check(st.clear == true and math.abs(st.x - (517 + 75)) < 0.5, "first clear ring point (+75 cm)", T.repr(st))
    local w2 = placed_world()
    w2.all_blocked = true
    w2:secs(2.1)
    st = w2:status()
    T.check(st.clear == false and math.abs(st.x - 517) < 0.1, "blocked everywhere: the server point, clear=false", T.repr(st))
    local w3 = placed_world()
    w3.others = { { 517, 250, 953 } }                                     -- someone stands on it
    w3:secs(2.1)
    T.check(math.abs(w3:status().x - 517) > 50, "keeps 1.5 m from other Willies", T.repr(w3:status()))
    local w4 = placed_world()
    w4.blocked = { { 517.0, 250.4 } }
    w4.floor_at = function(x, y) if math.abs(x - 517) < 1 and math.abs(y - 250.4) < 1 then return 853 end return 1300 end
    w4:secs(2.1)
    T.check(w4:status().clear == false, "offsets on a roof (> max_dz) are never used", T.repr(w4:status()))
end

T.log("== no ground under the server point")
do
    local w = placed_world()
    w.no_ground = true
    w:secs(3)
    local st = w:status()
    T.check(w.teleports == 0 and st and st.verified == false and T.contains(st.error, "no ground"), "status error, no teleport", T.repr(st))
    T.check(T.count(w:logtext(), "NO GROUND") == 1, "logged once, not retried every tick")
end

T.log("== single player / no order: nothing moves, nothing is protected")
do
    local w = new_world()
    w:new_pawn()
    w:start()
    w:secs(25)
    T.check(w.teleports == 0 and w:status() == nil, "no order: native placement kept, no status")
    T.check(not w.sp:protected() and w.pawn.props.Invulnerable == false, "no protection without an MP order")
    T.check(T.contains(w:logtext(), "no server spawn order for this world"), "logged once after the wait")
    local w2 = new_world()
    w2:spawns(1, "Map_Arena_Pit", 1, 2, 3)                              -- plan for another arena
    w2:new_pawn(); w2:start(); w2:secs(3)
    T.check(w2.teleports == 0, "an order for another arena is never used")
    local w3 = new_world()
    w3.mstate, w3.mround = "live", 3
    w3:spawns(1, "Map_Arena_Slums", 1, 2, 3)                            -- stale round
    w3:new_pawn(); w3:start(); w3:secs(3)
    T.check(w3.teleports == 0, "an order for another round is never used")
end

T.log("== world change resets everything")
do
    local w = placed_world()
    w:secs(3.5)
    w.wkey = "Map_Arena_Slums#2"
    w:new_pawn()
    w:spawns(2, "Map_Arena_Slums", 517.0, 250.4, 855.3)
    w.mround = 1
    w:tick(1)
    T.check(w.sp:protected() and w.teleports == 1, "new world: protected, waits for the settle again")
    w:secs(3.5)
    T.check(w:status().round == 2 and w:status().verified == true and w:status().spawn_id == 512, "round 2 placed", T.repr(w:status()))
end

-- ---------------------------------------------------------------------------
-- "Still no weapons": idle pawns could knock each other down before
-- the round started (Half Sword drops held weapons on a knockdown).

-- Mock of the collision / pawn-state / event env functions.
local function collide_world(opts)
    local w = placed_world(opts)
    w.calls, w.evs = {}, {}
    w.wl = {
        { key = "A1", actor = { name = "Willie_BP_C_7", x = 900, y = 250 }, name = "Willie_BP_C_7", sig = "m7|me" },
        { key = "A2", actor = { name = "Willie_BP_C_8", x = -300, y = 0 }, name = "Willie_BP_C_8", sig = "m8|me" },
    }
    w.env.willies = function() return w.wl end
    w.env.set_collision = function(_, other, enabled)
        w.calls[#w.calls + 1] = { other = other.name, enabled = enabled }
        return true, "calls=4"
    end
    w.env.actor_loc = function(a) return a.x, a.y, 953 end
    w.env.pawn_state = function(p)
        return { consciousness = p.props.Consciousness or 100, downed = p.props.Downed == true, fallen = false,
                 health = p.props.Health, weapon_r = p.props.wr or "ModularWeaponBP_ArmingSword_C", weapon_l = "None" }
    end
    w.env.ev = function(n, f) w.evs[#w.evs + 1] = { n = n, f = f } end
    function w:count(other, enabled)
        local n = 0
        for _, c in ipairs(self.calls) do if c.other == other and c.enabled == enabled then n = n + 1 end end
        return n
    end
    function w:states(at)
        return T.filter(self.evs, function(e) return e.n == "pawn_state" and (at == nil or e.f.at == at) end)
    end
    return w
end

T.log("== protection holds until the round goes Live (a long countdown), then ends at placement + protect_ms")
do
    local w = placed_world()
    w:secs(12)
    T.check(w.sp:protected() and w.pawn.props.Invulnerable == true and w:status().protect_until == nil,
        "12 s into the countdown: still protected (placement + 3 s is long past)", T.repr(w:status()))
    w.pawn.props.Health = 10
    w:secs(0.6)
    T.check(w.pawn.props.Health == 100, "vitals still topped up during the countdown")
    w:go_live()
    w:tick(2)
    T.check(not w.sp:protected() and w.pawn.props.Invulnerable == false, "Live after the window: protection ends at once")
    T.check(T.contains(w:logtext(), "round is Live; spawn protection ends now"), "and the log says so")
    local w2 = placed_world()
    w2:secs(2.2)                                       -- placed + verifying
    w2:go_live()
    w2:secs(1.2)
    T.check(not w2.sp:protected() and w2.pawn.props.Invulnerable == false and type(w2:status().protect_until) == "number",
        "Live before placement + 3 s ends the protection at Live (the floor never reaches into Live)", T.repr(w2:status()))
end

T.log("== no collision with any other Willie while protected; restored after (both directions in one call)")
do
    local w = collide_world()
    w:tick(1)
    T.check(w:count("Willie_BP_C_7", false) == 1 and w:count("Willie_BP_C_8", false) == 1,
        "collision off with every other Willie from the first protected tick", T.repr(w.calls))
    T.check(T.contains(w:logtext(), "no collision with Willie_BP_C_7 (ok: calls=4)"), "logged per Willie with the detail")
    w:secs(3)
    T.check(#w.calls == 2, "not re-issued every scan while nothing changed", #w.calls)
    w.wl[1].sig = "m7,newsword|me"                     -- a weapon re-armed: new bodies to cover
    w:secs(0.6)
    T.check(w:count("Willie_BP_C_7", false) == 2, "re-issued when the pair's bodies change (a re-armed weapon)")
    w.wl[3] = { key = "A3", actor = { name = "Willie_BP_C_9", x = 2000, y = 0 }, name = "Willie_BP_C_9", sig = "m9|me" }
    w:secs(0.6)
    T.check(w:count("Willie_BP_C_9", false) == 1, "a stand-in claimed later is covered too")
    w:secs(10)
    T.check(w:count("Willie_BP_C_7", true) == 0, "still off during a long countdown")
    w:go_live()
    w:secs(1)
    T.check(w:count("Willie_BP_C_7", true) == 1 and w:count("Willie_BP_C_8", true) == 1 and w:count("Willie_BP_C_9", true) == 1,
        "protection over: collision restored with every Willie, once", T.repr(w.calls))
    T.check(T.contains(w:logtext(), "collision with 3 other Willie(s) restored (protection over)"), "restore logged")
    local n = #w.calls
    w:secs(3)
    T.check(#w.calls == n, "nothing more after the restore")
end

T.log("== a Willie still overlapping us when protection ends: collision stays off until it moves (bounded)")
do
    local w = collide_world()
    w:secs(3.5)
    w.wl[1].actor.x, w.wl[1].actor.y = w.pawn.x + 40, w.pawn.y   -- standing in us
    w:go_live()
    w:secs(2.5)                                         -- placement + 3 s is over
    T.check(not w.sp:protected() and w:count("Willie_BP_C_7", true) == 0, "not restored while it overlaps")
    T.check(T.contains(w:logtext(), "another Willie is 40 cm away"), "deferral logged")
    w.wl[1].actor.x = w.pawn.x + 600                    -- it walked away
    w:secs(0.6)
    T.check(w:count("Willie_BP_C_7", true) == 1, "restored once it moved away")
    local w2 = collide_world()
    w2:secs(3.5)
    w2.wl[1].actor.x, w2.wl[1].actor.y = w2.pawn.x, w2.pawn.y
    w2:go_live()
    w2:secs(2.5)
    T.check(w2:count("Willie_BP_C_7", true) == 0, "still off right after the window")
    w2:secs(SP.T.overlap_grace_s + 0.6)
    T.check(w2:count("Willie_BP_C_7", true) == 1, "restored after overlap_grace_s even if it never moves")
end

T.log("== a possession swap restores the old pawn's collision pairs")
do
    local w = collide_world()
    w:secs(1)
    w:new_pawn()
    w:tick(1)
    T.check(w:count("Willie_BP_C_7", true) == 1 and T.contains(w:logtext(), "restored (possessed pawn changed)"),
        "old pawn's pairs re-enabled", T.repr(w.calls))
    w:secs(0.6)
    T.check(w:count("Willie_BP_C_7", false) == 2, "the new pawn gets its own pairs")
end

T.log("== pawn_state events: placed, every 5 s while protected, ready, live, protection end")
do
    local w = collide_world()
    w:secs(3.5)
    local p = w:states("placed")
    T.check(#p == 1 and p[1].f.consciousness == 100 and p[1].f.downed == false and p[1].f.weapon_r == "ModularWeaponBP_ArmingSword_C"
        and p[1].f.weapon_l == "None" and p[1].f.round == 1 and p[1].f.protected == true and p[1].f.pawn == w.pawn.id,
        "pawn_state{at=placed} with consciousness/downed/weapon_r/weapon_l", T.repr(p))
    T.check(p[1].f.dist_cm ~= nil and p[1].f.dist_cm < 5, "dist_cm from the spawn", T.repr(p[1].f))
    w.director = "Ready"
    w:secs(0.6)
    T.check(#w:states("ready") == 1, "pawn_state{at=ready} once the Director is Ready")
    w.pawn.props.Consciousness, w.pawn.props.Downed, w.pawn.props.wr = 74, true, "Weapon_Fists_C"
    w:secs(5.1)
    local pr = w:states("protect")
    T.check(#pr >= 1 and pr[#pr].f.consciousness == 74 and pr[#pr].f.downed == true and pr[#pr].f.weapon_r == "Weapon_Fists_C",
        "every 5 s while protected: a knockdown at spawn is visible to the gate", T.repr(pr))
    T.check(T.contains(w:logtext(), "pawn_state[protect] round 1: consciousness=74 downed=true"), "and in the log")
    local n = #pr
    w:secs(5.1)
    T.check(#w:states("protect") == n + 1, "cadence 5 s")
    T.check(#w:states("ready") == 1, "ready only once")
    w:go_live()
    w:secs(0.2)
    T.check(#w:states("live") == 1 and #w:states("protect_end") == 1, "live + protect_end", T.repr(w:states()))
    local m = #w:states()
    w:secs(11)
    T.check(#w:states() == m, "no pawn_state after the window")
end

-- ---------------------------------------------------------------------------
-- Peer id, stale status, Director requests, session view, freed Willies

T.log("== H3: the own peer id is re-read, never latched (reconnect -> new id -> placement continues)")
do
    -- poll() = the sidecar's `link` record (status name, my_peer_id) + its header heartbeat
    local link, fresh, clock, logs = nil, true, 10.0, {}
    local poll = function() if not link then return nil, 0, fresh end; return link[1], link[2], fresh end
    local get = SP.peer_reader(poll, function() return clock end, 0.5, function(fmt, ...) logs[#logs + 1] = string.format(fmt, ...) end)
    T.check(get() == 0 and select(2, get(true)) == "none", "no link record: id 0, status none")
    link = { "connecting", 0 }
    T.check(get(true) == 0, "connecting without an id: 0")
    link = { "connected", 3 }
    T.check(get(true) == 3, "Welcome: id 3")
    link = { "reconnecting", 0 }
    T.check(get(true) == 0, "reconnecting, not welcomed again: no id (a stale id never picks a seat)")
    link = { "connected", 7 }
    clock = clock + 0.1
    T.check(get() == 0, "throttled: no re-read inside 0.5 s")
    clock = clock + 0.5
    T.check(get() == 7, "re-read after the throttle: the new id 7 (was latched forever)")
    T.check(T.contains(table.concat(logs, "\n"), "own peer id 3 -> 0"), "id changes are logged")

    -- the placer follows the new id on its next order() call
    local w = new_world()
    w:spawns(1, "Map_Arena_Slums", 517.0, 250.4, 855.3)
    w.peer = 7                                     -- not in the plan (old ids 1, 2)
    w:new_pawn(); w:start()
    w:secs(3)
    T.check(w.teleports == 0, "no seat for an unknown id: nothing placed")
    w.peer = 2                                     -- the sidecar's id after the reconnect
    w:secs(3)
    local st = w:status()
    T.check(w.teleports >= 1 and st and st.spawn_id == 257, "the new id finds its seat and is placed", T.repr(st))
end

T.log("== L12: a spawn status from an earlier process is cleared at start")
do
    local w = new_world()
    w.status_rec = R.marshal(NS, "spawn_status", { seq = 9, round = 1, arena = "Map_Arena_Slums", pawn = "Willie_BP_C_101",
        verified = true, has_protect_until = true, protect_until = 999999.0 })
    w:start()
    T.check(w:status() == nil and w.status_rec.seq == 0, "stale status cleared by SP.new (the zeroed record)")
end

T.log("== a Director request is handled once, after the pawn settled")
do
    local w = placed_world()
    w:secs(3)
    T.check(w:status() and w:status().verified, "placed and verified")
    local good = '{"seq":5,"round":1,"arena":"Map_Arena_Slums","pawn":"' .. w.pawn.id .. '","spawn_id":256,"why":"retry"}'
    w.req = R.marshal(NS, "spawn_request", {})   -- the cleared form (seq 0) is no request
    w:secs(2)
    T.check(w.sp.req_seq == 0, "a cleared request is not consumed", w.sp.req_seq)
    w:request_json(good)
    w:secs(2)
    T.check(w.sp.req_seq == 5, "the request is handled", w.sp.req_seq)
end

T.log("== the `spawned` report waits for 'connected' and is retried when the send fails")
do
    local w = placed_world()
    local connected, refuse = false, 0
    w.env.connected = function() return connected end
    local send = w.env.send_spawned
    w.env.send_spawned = function(t) if refuse > 0 then refuse = refuse - 1; return nil end; return send(t) end
    w:secs(3.5)                                      -- placed + verified: the report is queued
    w:secs(3)
    T.check(#w.sent == 0, "nothing sent while the sidecar is not connected")
    T.check(T.contains(w:logtext(), "held until the sidecar is connected"), "held, logged once")
    connected, refuse = true, 1
    w:secs(4.5)
    local v = w:verbs()
    T.check(#v == 3 and v[1] == "spawned:1:4:517:250:953:1" and v[3] == v[1], "then sent 3x (a refused send is retried)", T.repr(v))
    local r = w.sent[1]
    T.check(r.round == 1 and r.slot == 4 and r.clear == true and math.abs(r.pos[3] - 953) < 0.01, "a typed `spawned` record", T.repr(r))
end

T.log("== the match state comes from the session view")
do
    local v = nil
    local get = SP.match_reader(function() return v end)
    T.check(get() == nil, "no session: no match")
    v = { state = "live", round = 2 }
    local st, r = get()
    T.check(st == "live" and r == 2, "state and round", T.repr({ st, r }))
    v = nil
    T.check(get() == nil, "gone again: no match")
end

T.log("== peer reader clears the id on a missing link / terminal status; liveness is the heartbeat (bug 4)")
do
    local link, fresh, clock = nil, true, 10.0
    local poll = function() if not link then return nil, 0, fresh end; return link[1], link[2], fresh end
    local get = SP.peer_reader(poll, function() return clock end, 0.5)
    link = { "connected", 3 }
    local id, st, live = get(true)
    T.check(id == 3 and live, "connected + a beating sidecar: id read, live")
    fresh = false
    id, st, live = get(true)
    T.check(id == 3 and st == "connected" and not live, "the heartbeat stopped: not live, whatever the status (dead sidecar)")
    fresh = true
    link = { "reconnecting", 3 }
    T.check(select(3, get(true)) == true, "reconnecting with a beating sidecar counts as live (the seat is kept)")
    link = nil
    id, st, live = get(true)
    T.check(id == 0 and st == "none" and not live, "no link record: no id", T.repr({ id, st }))
    link = { "connected", 5 }
    id, st, live = get(true)
    T.check(id == 5 and live, "a new sidecar: new id, live")
    link = { "kicked", 5 }
    id, st, live = get(true)
    T.check(id == 0 and st == "kicked" and not live, "terminal status: no id, not live")

    -- the placer: stale MP files in single player (Free Mode in the same arena after leaving mid-Live)
    local w = new_world()
    local live_now = false
    w.env.session_live = function() return live_now end
    w:spawns(1, "Map_Arena_Slums", 517.0, 250.4, 855.3)
    w:new_pawn(); w:start()
    w:secs(5)
    T.check(w.teleports == 0 and w:status() == nil and not w.sp:protected() and w.pawn.props.Invulnerable == false,
        "no live session: no placement, no protection, no status", T.repr({ w.teleports, w.files[STATUS] }))
    T.check(T.contains(w.sp:order_text(), "no live MP session"), "order_text says why", w.sp:order_text())
    live_now = true
    w:secs(3)
    T.check(w.teleports == 1 and w:status() and w:status().verified, "a live session: placed as before", T.repr(w:status()))
end

T.log("== the real env reads fresh physical pelvis and corrects first-teleport body residual")
do
    local saved_sfo, saved_fn = rawget(_G, "StaticFindObject"), rawget(_G, "FName")
    _G.StaticFindObject = function() return nil end
    _G.FName = function(n) return n end
    local env = SP.make_ue_env({ UEHelpers = { GetPlayerController = function() return nil end },
        log = function() end, state_dir = T.tmpdir("sp_physical_"), my_peer_id = function() return 1 end,
        match = function() return "countdown" end, world = function() return { key = "k", short = "A" } end })
    -- Component / animated socket and simulated COM are independent. An actor
    -- teleport can move the former while its physics body stays at the fence.
    local function fixture(o, p)
        o, p = o or {}, p or { x = 100, y = 0, z = 100 }
        local m = { x = p.x, y = p.y, z = p.z, bx = p.x + 5, by = p.y + 10, bz = p.z + 50,
            moves = 0, com_reads = 0, socket_reads = 0, sim = o.sim ~= false }
        p.Mesh = m
        m.IsValid = function() return true end
        m.GetAddress = function() return 121 end
        m.IsSimulatingPhysics = function(_, bone)
            assert(bone == "pelvis")
            if o.sim_error then error("physics state unreadable") end
            if o.sim_unknown then return nil end
            return m.sim
        end
        m.GetCenterOfMass = function(_, bone)
            assert(bone == "pelvis")
            m.com_reads = m.com_reads + 1
            if o.com_error then error("physics COM unreadable") end
            if o.com_nil then return nil end
            return { X = o.com_nan and (0/0) or m.bx, Y = m.by, Z = o.com_inf and math.huge or m.bz }
        end
        m.GetSocketLocation = function()
            m.socket_reads = m.socket_reads + 1
            return { X = m.x, Y = m.y, Z = m.z + 50 }
        end
        m.K2_GetComponentLocation = function() return { X = m.x, Y = m.y, Z = m.z } end
        m.K2_SetWorldLocation = function(_, l, sweep, hit, teleport)
            assert(sweep == false and teleport == true)
            local dx, dy, dz = l.X - m.x, l.Y - m.y, l.Z - m.z
            m.x, m.y, m.z, m.moves = l.X, l.Y, l.Z, m.moves + 1
            -- Some detached components change location before their physics
            -- body accompanies the move; the fresh read must catch this too.
            if not (o.correction_body_lag or (o.first_mesh_body_lag and m.moves == 1)) then
                m.bx, m.by, m.bz = m.bx + dx, m.by + dy, m.bz + dz
            end
        end
        m.SetAllPhysicsLinearVelocity = function() end
        m.SetAllPhysicsAngularVelocityInDegrees = function() end
        p.K2_GetActorLocation = function() return { X = p.x, Y = p.y, Z = p.z } end
        p.K2_SetActorLocation = function(_, l, sweep, hit, teleport)
            assert(sweep == false and teleport == true)
            local dx, dy, dz = l.X - p.x, l.Y - p.y, l.Z - p.z
            p.x, p.y, p.z = l.X, l.Y, l.Z
            if not o.detached then m.x, m.y, m.z = m.x + dx, m.y + dy, m.z + dz end
            if o.body_follows then m.bx, m.by, m.bz = m.bx + dx, m.by + dy, m.bz + dz end
        end
        return p, m
    end
    local dest = { X = 1100, Y = 500, Z = 100 }
    local p, m = fixture()
    local x, y, z = env.body_loc(p)
    T.check(x == 105 and y == 10 and z == 150 and m.com_reads == 1 and m.socket_reads == 0,
        "simulated pelvis is read from native COM, independent of its animated socket")
    local detail = env.teleport(p, dest, nil, 1)
    T.check(m.bx == 1105 and m.by == 510 and m.bz == 150 and m.moves == 1,
        "first teleport corrects physics lag when the component followed", T.repr({ m.bx, m.by, m.bz, m.moves, detail }))
    T.check(T.contains(detail, "Mesh followed; bodies snapped("), "followed branch reports the immediate physical correction", detail)
    x, y, z = env.body_loc(p)
    T.check(x == 1105 and y == 510 and z == 150 and m.socket_reads == 0,
        "placement reads the corrected physical body, even when the animated component differs")

    p, m = fixture({ detached = true, first_mesh_body_lag = true })
    detail = env.teleport(p, dest, nil, 1)
    T.check(m.bx == 1105 and m.by == 510 and m.bz == 150 and m.moves == 2,
        "first teleport also checks physical lag after moving a detached component", T.repr({ m.bx, m.by, m.bz, m.moves, detail }))
    T.check(m.x == 2100 and m.y == 1000 and T.contains(detail, "Mesh moved(") and T.contains(detail, "; bodies snapped("),
        "residual correction uses the fresh component location after its initial move", detail)

    p, m = fixture({ body_follows = true })
    detail = env.teleport(p, dest, nil, 1)
    T.check(m.moves == 0 and m.bx == 1105 and m.by == 510 and not T.contains(detail, "bodies snapped"),
        "a component and physics body that already followed are not moved twice", detail)
    p, m = fixture({ detached = true })
    detail = env.teleport(p, dest, nil, 1)
    T.check(m.moves == 1 and m.bx == 1105 and m.by == 510 and not T.contains(detail, "bodies snapped"),
        "a detached body carried by its initial component move receives no residual move", detail)
    p, m = fixture({ correction_body_lag = true })
    detail = env.teleport(p, dest, nil, 1)
    T.check(m.moves == 1 and m.bx == 105 and m.by == 10 and T.contains(detail, "body correction unresolved(")
            and not T.contains(detail, "bodies snapped"),
        "a successful movement call with unchanged physical COM is reported as unresolved", detail)

    p, m = fixture({ sim = false })
    x, y, z = env.body_loc(p)
    T.check(x == 100 and y == 0 and z == 150 and m.com_reads == 0 and m.socket_reads == 1,
        "explicitly non-simulated mesh retains the visual socket fallback")
    for _, opts in ipairs({ { sim_error = true }, { sim_unknown = true }, { com_error = true },
            { com_nil = true }, { com_nan = true }, { com_inf = true } }) do
        p, m = fixture(opts)
        local err
        x, y, z, err = env.body_loc(p)
        T.check(x == nil and y == nil and z == nil and type(err) == "string" and m.socket_reads == 0,
            "unreadable simulation / COM never falls back to animated proof: " .. T.repr(opts), err)
    end
    p, m = fixture({ com_error = true })
    detail = env.teleport(p, dest, nil, 1)
    T.check(m.moves == 0 and T.contains(detail, "body unavailable: physical pelvis unavailable"),
        "unknown physical residual is logged and never guessed", detail)

    local w = placed_world()
    p, m = fixture({}, w.pawn)
    w.env.body_loc = env.body_loc
    w.env.teleport = function(pawn, target, yaw, attempt)
        w.teleports = w.teleports + 1
        return env.teleport(pawn, target, nil, attempt)
    end
    w:secs(4)
    local st = w:status()
    T.check(st and st.verified and st.tries == 1 and m.moves == 1 and w.teleports == 1,
        "first physical correction precedes the placement-complete handshake", T.repr(st))
    T.check(m.socket_reads == 0 and T.contains(w:logtext(), "bodies snapped("),
        "the complete placement uses fresh physical proof throughout", w:logtext())
    _G.StaticFindObject, _G.FName = saved_sfo, saved_fn
end

T.log("== the real env's teleport / hold carry the held weapon actors")
do
    local saved_sfo = rawget(_G, "StaticFindObject")
    _G.StaticFindObject = function() return nil end
    local function mkactor(x, y, z, cls, follow)
        local a = { x = x, y = y, z = z, cls = cls or "ModularWeaponBP_Sword_C", lin = nil, ang = nil, moves = 0 }
        a.IsValid = function() return true end
        a.GetClass = function() return { GetFName = function() return { ToString = function() return a.cls end } end } end
        a.K2_GetActorLocation = function() return { X = a.x, Y = a.y, Z = a.z } end
        a.K2_SetActorLocation = function(_, l) a.x, a.y, a.z = l.X, l.Y, l.Z; a.moves = a.moves + 1; return true end
        a.K2_GetRootComponent = function()
            return { IsValid = function() return true end,
                     SetAllPhysicsLinearVelocity = function(_, v) a.lin = v end,
                     SetAllPhysicsAngularVelocityInDegrees = function(_, v) a.ang = v end }
        end
        return a
    end
    local sword = mkactor(110, 20, 150)
    local fists = mkactor(90, -20, 150, "Weapon_Fists_C")
    local pawn = mkactor(100, 0, 100, "Willie_BP_C")
    pawn["Weapon R"], pawn["Weapon L"] = sword, fists
    -- the attached fists follow the pawn; the simulating sword does not
    local set = pawn.K2_SetActorLocation
    pawn.K2_SetActorLocation = function(self, l, ...)
        local dx, dy, dz = l.X - pawn.x, l.Y - pawn.y, l.Z - pawn.z
        fists.x, fists.y, fists.z = fists.x + dx, fists.y + dy, fists.z + dz
        return set(self, l, ...)
    end
    local env = SP.make_ue_env({ UEHelpers = { GetPlayerController = function() return nil end }, log = function() end,
        state_dir = T.tmpdir("sp_m19_"), my_peer_id = function() return 1 end, match = function() return "countdown" end,
        world = function() return { key = "k", short = "A" } end })
    local res = env.teleport(pawn, { X = 1100, Y = 500, Z = 100 }, nil, 1)
    T.check(sword.x == 1110 and sword.y == 520 and sword.z == 150 and sword.moves == 1,
        "the held sword moves with the pawn's offset (not dragged by the grip)", T.repr({ sword.x, sword.y, sword.z, res }))
    T.check(sword.lin and sword.lin.X == 0 and sword.ang and sword.ang.Z == 0, "and stops (zero velocity)")
    T.check(fists.moves == 0 and fists.x == 1090, "bare hands are part of the pawn: not moved twice")
    T.check(T.contains(res, "1 held weapon(s) carried"), "logged in the teleport detail", res)
    -- hold: the drift correction carries it the same way (XY only)
    pawn.x, pawn.y = 1130, 500; sword.x = 1140
    env.hold(pawn, { X = 1100, Y = 500, Z = 100 }, 10)
    T.check(sword.x == 1110 and sword.moves == 2, "hold carries the sword back with the pawn", T.repr({ sword.x, sword.moves }))
    _G.StaticFindObject = saved_sfo
end

T.log("== a Willie freed while we were protected: its cached actor is never touched again")
do
    -- Seen on a listen host: a pooled extra Willie vanished
    -- 2 s before Live; restoring collision touched its freed object
    -- (IsValid / set_collision) and crashed the game (STALE_UOBJECT).
    local w = collide_world()
    w:tick(1)
    T.check(w:count("Willie_BP_C_8", false) == 1, "setup: collision off with both")
    local gone = w.wl[2].actor
    table.remove(w.wl, 2)                              -- freed: FindAllOf no longer lists it
    setmetatable(gone, { __index = function() error("touched a freed Willie") end })
    w.env.valid = function(a) if a == gone then error("IsValid on a freed Willie") end; return true end
    w.env.actor_loc = function(a) if a == gone then error("location of a freed Willie") end; return a.x, a.y, 953 end
    w:secs(3)
    w:go_live()
    local ok, err = pcall(function() w:secs(2) end)
    T.check(ok, "protection ends without touching the freed Willie", tostring(err))
    T.check(w:count("Willie_BP_C_7", true) == 1 and w:count("Willie_BP_C_8", true) == 0,
        "collision restored with the Willie that still exists only", T.repr(w.calls))
    T.check(T.contains(w:logtext(), "collision with 1 other Willie(s) restored (protection over)"), "restore logged for 1")
end

T.log("== original placement context survives report refresh and changes only after new placement")
do
    local w=placed_world()
    w.plan.match_id=51
    w:secs(4)
    local old=w:status()
    T.check(old.verified and old.match_id==51 and old.life==1,"verified status stores original order match and life")
    w:go_live();w:tick()
    T.check(w:status().match_id==51 and w:status().life==1,"Live report refresh retains original placement context")
    w.mstate,w.mround="countdown",0
    w:spawns(1,"Map_Arena_Slums",517,250,853)
    w.plan.match_id=52
    local before=w.teleports
    w:tick()
    T.check(w.teleports==before+1 and not w:status().verified and w:status().match_id==52,
        "same pawn and spawn256 in a new match requires a fresh physical placement")
    T.check(old.match_id==51,"new session never relabels saved old placement")
    w:secs(1.2)
    T.check(w:status().verified and w:status().match_id==52,"new match verifies its own placement")
end

T.log("== native dislocation guard starts before placement and restores its original bool once")
do
    for _, original in ipairs({ false, true }) do
        local w = placed_world()
        w.pawn.props["Block Spine Breaking"] = original
        local teleport = w.env.teleport
        w.env.teleport = function(p, ...)
            T.check(p.props["Block Spine Breaking"] == true,
                "native dislocation guard is active before every first placement teleport")
            return teleport(p, ...)
        end
        w:tick()
        T.check(w.teleports == 0 and w.pawn.props["Block Spine Breaking"] == true,
            "native dislocation guard starts on the first protected tick, before settling")
        w.pawn.props["Block Spine Breaking"] = false -- native animation code can overwrite it
        w:tick()
        T.check(w.pawn.props["Block Spine Breaking"] == true,
            "a native false write is reasserted while protected")
        w:secs(3.5)
        local writes = 0
        for _, row in ipairs(w.prop_writes) do
            if row.key == "Block Spine Breaking" then writes = writes + 1 end
        end
        w:go_live(); w:tick()
        T.check(w.pawn.props["Block Spine Breaking"] == original,
            "Live restores the exact original native bool: " .. tostring(original))
        local after = 0
        for _, row in ipairs(w.prop_writes) do
            if row.key == "Block Spine Breaking" then after = after + 1 end
        end
        T.check(after == writes + 1, "the original bool is restored with one write at Live")
        w.pawn.props["Block Spine Breaking"] = not original
        w:secs(2)
        local later = 0
        for _, row in ipairs(w.prop_writes) do
            if row.key == "Block Spine Breaking" then later = later + 1 end
        end
        T.check(later == after and w.pawn.props["Block Spine Breaking"] == not original,
            "completed guard never restores again or fights later native writes")
    end
end

T.log("== an unreadable native dislocation bool is explicit and never guessed")
do
    for _, unavailable in ipairs({ "missing", "number", "error" }) do
        local w = placed_world()
        local get = w.env.prop_get
        w.env.prop_get = function(p, key)
            if key == "Block Spine Breaking" then
                if unavailable == "error" then error("property unavailable") end
                return unavailable == "number" and 0 or nil
            end
            return get(p, key)
        end
        w:secs(3.5); w:go_live(); w:secs(1)
        local writes = 0
        for _, row in ipairs(w.prop_writes) do
            if row.key == "Block Spine Breaking" then writes = writes + 1 end
        end
        T.check(writes == 0 and w.pawn.props["Block Spine Breaking"] == false,
            "no guessed write for an unreadable original bool: " .. unavailable)
        T.check(T.contains(w:logtext(), "native dislocation guard unavailable"),
            "unreadable native guard is logged explicitly: " .. unavailable)
    end
end

T.log("== native dislocation and dismemberment state stays under the game's control during Live")
do
    local w = placed_world()
    w:secs(3.5); w:go_live(); w:tick()
    w.pawn.props.Health, w.pawn.props["Arm L Health"] = 27, 4
    w.pawn.props["Arm L Dislocated"], w.pawn.props["Neck Dislocated"] = true, true
    w.pawn.props["Dismembered Parts Map"] = { [6] = true }
    local limbs = w.pawn.props["Dismembered Parts Map"]
    w.pawn.props["Block Spine Breaking"] = false
    w:secs(2)
    T.check(w.pawn.props.Health == 27 and w.pawn.props["Arm L Health"] == 4
        and w.pawn.props["Arm L Dislocated"] and w.pawn.props["Neck Dislocated"]
        and w.pawn.props["Dismembered Parts Map"] == limbs and limbs[6],
        "ordinary Live neither heals nor clears native injury or dismemberment")
    T.check(w.pawn.props["Block Spine Breaking"] == false,
        "ordinary Live leaves the native joint-breaking switch alone")

    w.plan.by_peer[1].spawn_id = 386
    w.director = "Spawn"
    w:new_pawn(); w:tick()
    T.check(w.sp:protected() and w.pawn.props["Block Spine Breaking"] == false,
        "existing Live deathmatch spawn protection never arms the dislocation guard")
    w.pawn.props["Arm L Dislocated"] = true
    w.pawn.props["Dismembered Parts Map"] = limbs
    w:secs(3.5)
    T.check(w.pawn.props["Block Spine Breaking"] == false and w.pawn.props["Arm L Dislocated"]
        and w.pawn.props["Dismembered Parts Map"] == limbs,
        "Live deathmatch placement never clears or freezes native joint injury")
end

T.log("== native dislocation guard drops world caches without touching an old pawn")
do
    local w = placed_world()
    w:tick()
    local old = w.pawn
    old.props = nil
    setmetatable(old, { __index = function() error("touched an old-world pawn") end })
    w.wkey = "Map_Arena_Slums#2"
    w:new_pawn()
    w.pawn.props["Block Spine Breaking"] = true
    local ok, err = pcall(function() w:secs(3.5); w:go_live(); w:tick() end)
    T.check(ok, "world drop never reads or restores the old native guard", tostring(err))
    T.check(w.pawn.props["Block Spine Breaking"] == true,
        "the next world's original true value is preserved independently")
end

T.log("== native dislocation guard transfers its original value before publishing a new assignment")
do
    local w = placed_world()
    w.plan.match_id = 61
    w:secs(3.5)
    local before = w:status()
    T.check(before.match_id == 61 and w.pawn.props["Block Spine Breaking"] == true,
        "old verified assignment owns the guarded flag")
    w:spawns(1, "Map_Arena_Slums", 517, 250, 853)
    w.plan.match_id = 62
    local teleport = w.env.teleport
    w.env.teleport = function(p, ...)
        local last = {}
        for _, row in ipairs(w.prop_writes) do
            if row.key == "Block Spine Breaking" then last[#last + 1] = row.value end
        end
        T.check(#last >= 3 and last[#last - 1] == false and last[#last] == true,
            "old false is restored before the new context captures and reasserts it")
        T.check(w:status().match_id == 61 and p.props["Block Spine Breaking"] == true,
            "new native guard precedes the new teleport and new status publication")
        return teleport(p, ...)
    end
    w:tick()
    T.check(w:status().match_id == 62 and not w:status().verified,
        "new assignment is published only after restoration ownership transfers")
    w:secs(1.2); w:go_live(); w:tick()
    T.check(w.pawn.props["Block Spine Breaking"] == false,
        "new assignment restores false, rather than inheriting the previous temporary true")
end

T.log("== a changed life cannot restore the preceding life's original native flag")
do
    local w = placed_world()
    w.pawn.props["Block Spine Breaking"] = true
    w.plan.match_id = 63
    w:secs(3.5)
    w.plan.by_peer[1].life = 2 -- a later assignment without the placer's ownership transfer
    w.pawn.props["Block Spine Breaking"] = false
    local before = #w.prop_writes
    w:go_live(); w:tick()
    local wrote = false
    for i = before + 1, #w.prop_writes do
        if w.prop_writes[i].key == "Block Spine Breaking" then wrote = true end
    end
    T.check(not wrote and w.pawn.props["Block Spine Breaking"] == false,
        "same actor in a different full life never receives an old restoration write")
    T.check(T.contains(w:logtext(), "original pawn/world/life no longer current"),
        "unproven ownership transfer is explicit, not silently inherited")
end

T.log("== a new match during initial settling transfers the guard's actual original value")
do
    local w = placed_world()
    w.plan.match_id = 66
    w:tick()
    T.check(w.sp.cur == nil and w.sp.dislocation_guard.original == false
        and w.pawn.props["Block Spine Breaking"] == true,
        "initial protected tick owns original false before any placement exists")
    w:spawns(1, "Map_Arena_Slums", 517, 250, 853)
    w.plan.match_id = 67
    w:secs(3.5)
    T.check(w:status() and w:status().verified and w:status().match_id == 67
        and w.sp.dislocation_guard.context.match_id == 67 and w.sp.dislocation_guard.original == false,
        "settling-period match replacement transfers original false, never temporary true")
    w:go_live(); w:tick()
    T.check(w.pawn.props["Block Spine Breaking"] == false and w.sp.dislocation_guard.ended,
        "Live after the replacement restores the pawn's actual original false")
end

T.log("== replacement with the same pawn name never touches a cached native guard actor")
do
    local w = placed_world()
    w:tick()
    local old, name = w.pawn, w.pawn.id
    old.props = nil
    setmetatable(old, { __index = function() error("touched a replaced native guard actor") end })
    w:new_pawn()
    w.pawn.id = name
    w.pawn.props["Block Spine Breaking"] = true
    local ok, err = pcall(function() w:secs(3.5); w:go_live(); w:tick() end)
    T.check(ok, "same-name actor replacement uses only the fresh current pawn", tostring(err))
    T.check(w.pawn.props["Block Spine Breaking"] == true,
        "replacement's original true never inherits the previous actor's false")
end

T.log("== a transient native guard restoration failure retries in the exact current Live life")
do
    local w = placed_world()
    w.plan.match_id = 68
    w:secs(3.5)
    local set, attempts = w.env.prop_set, 0
    w.env.prop_set = function(p, key, value)
        if key == "Block Spine Breaking" and value == false then
            attempts = attempts + 1
            if attempts <= 2 then return false end
        end
        return set(p, key, value)
    end
    w:go_live(); w:tick()
    T.check(attempts == 2 and w.pawn.props["Block Spine Breaking"] == true
        and w.sp.dislocation_guard.original == false and not w.sp.dislocation_guard.ended,
        "failed Live restoration retains the exact original false and stays explicitly incomplete")
    w:tick()
    T.check(attempts == 3 and w.pawn.props["Block Spine Breaking"] == false and w.sp.dislocation_guard.ended,
        "fresh same world, actor, and full life retry restores original false after transient failure")
    w:secs(1)
    T.check(attempts == 3, "successful restoration ends retries without further native writes")
end

T.log("== a permanently failed native guard restoration blocks transfer without recapturing true")
do
    local w = placed_world()
    w.plan.match_id = 64
    w:secs(3.5)
    local set, attempts = w.env.prop_set, 0
    w.env.prop_set = function(p, key, value)
        if key == "Block Spine Breaking" and value == false then attempts = attempts + 1; return false end
        return set(p, key, value)
    end
    w:go_live(); w:secs(1)
    T.check(attempts > 1 and T.contains(w:logtext(), "unavailable: restore failed")
        and not w.sp.dislocation_guard.ended,
        "permanent Live failure retries while eligible and remains explicitly unavailable")
    local old_status, old_teleports = w:status(), w.teleports
    w.mstate, w.mround = "countdown", 0
    w:spawns(1, "Map_Arena_Slums", 517, 250, 853)
    w.plan.match_id = 65
    w:secs(3.5)
    local g = w.sp.dislocation_guard
    T.check(g and not g.ended and g.original == false and g.context.match_id == 64,
        "failed restoration keeps original ownership unavailable instead of recapturing temporary true")
    T.check(w.teleports == old_teleports and w:status().match_id == old_status.match_id
        and w.sp.cur.plan.match_id == 64,
        "new assignment cannot teleport or publish new placement evidence before guard transfer succeeds")
    T.check(T.contains(w:logtext(), "placement held: native dislocation guard restoration unavailable"),
        "held startup reports its native restoration blocker explicitly")
end

T.log("== native guard restoration requires live readback, not only a successful property call")
do
    local w = placed_world()
    w:secs(3.5)
    local set, ignore = w.env.prop_set, true
    w.env.prop_set = function(p, key, value)
        if ignore and key == "Block Spine Breaking" and value == false then return true end
        return set(p, key, value)
    end
    w:go_live(); w:tick()
    T.check(not w.sp.dislocation_guard.ended and w.pawn.props["Block Spine Breaking"] == true
        and T.contains(w:logtext(), "unavailable: restore failed"),
        "a successful setter with unchanged live flag never claims successful restoration")
    ignore = false
    w:tick()
    T.check(w.sp.dislocation_guard.ended and w.pawn.props["Block Spine Breaking"] == false,
        "retry completes only after the native live flag equals the saved original bool")
end

T.log("== real pawn diagnostics expose the live native dislocation flag without guessing")
do
    local env = SP.make_ue_env({ UEHelpers = {}, log = function() end })
    local p = { ["Block Spine Breaking"] = false }
    T.check(env.pawn_state(p).block_spine_breaking == false,
        "real pawn diagnostics preserve an explicit native false")
    p["Block Spine Breaking"] = true
    T.check(env.pawn_state(p).block_spine_breaking == true,
        "real pawn diagnostics preserve an explicit native true")
    p["Block Spine Breaking"] = nil
    T.check(env.pawn_state(p).block_spine_breaking == nil,
        "missing native flag remains unavailable in pawn diagnostics")
end
