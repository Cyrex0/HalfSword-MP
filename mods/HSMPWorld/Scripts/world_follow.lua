-- HSMPWorld / world_follow.lua: how a body somebody else simulates is shown here.
-- Pure logic (no UE4SS calls), unit-tested offline (tests/hsmpworld-sim and the
-- hsmpworld lua-test suite). main.lua feeds it the owner's samples and teleports the
-- kinematic copy to what `pose` returns.
--
-- Timelines. Every sample carries the owner's game clock (ts, ms). Per sender we keep
--   off   = min(rx - ts) over the last WIN_MS (clock difference + the fastest path),
--   delay = the playout delay: one send interval + the p90 lateness of recent samples
--           (grows at once, shrinks slowly), clamped to [D_MIN, D_MAX].
-- Two ways to show a body:
--   * delayed: interpolate at ts = now - off - delay. Held items and bodies being pushed
--     by their owner play in the owner's own timeline, like its stand-in does, so a sword
--     stays in the stand-in's hand and a crate does not move before it is touched.
--   * present: dead-reckon the newest sample to the owner's present
--     (now - off + lead, lead = the one-way path estimate) with gravity while airborne
--     and floor friction while sliding. A thrown or knocked-away body is shown where it
--     is now, so both screens agree on where it flies and lands.
-- A body switches delayed -> present when it flies free of its owner and back only when
-- it is held again (time never runs backwards on screen).
--
-- Blending. A new sample (or a mode switch) moves the target; the jump is kept as a
-- visual offset that decays over BLEND_MS, so corrections never snap. A correction
-- above SNAP_CM is applied at once and counted (a snap).

local F = {}

F.SEND_MS     = 50     -- the owner's send interval (main.lua SEND_EVERY ticks)
F.FAST_SEND_MS = 33    -- held items and bodies at their owner (contact): every 2nd tick, the server's flush rate
F.D_MIN_FAST  = 25
F.CONTACT_CM  = 300    -- a body this near its owner is in contact (main.lua NEAR_DIST)
F.D_MIN       = 40
F.D_MAX       = 350
F.WIN_MS      = 3000
F.SHRINK      = 0.03   -- delay ms given back per ms (slow shrink)
F.BLEND_MS    = 110
F.SNAP_CM     = 400    -- only a correction this large is applied at once (a snap)
F.FIX_SPEED   = 400    -- corrections close at most this fast (cm/s): a big one slides, never pops
F.SNAP_DEG    = 179    -- rotation corrections are always blended
F.EXTRAP_MS   = 350    -- dead reckoning beyond the newest sample (present mode)
F.EXTRAP_ROT_MS = 200
F.HOLD_EXTRAP_MS = 100 -- delayed mode past the newest sample
F.G           = 980    -- cm/s^2
F.SLIDE_DECEL = 400    -- floor friction while sliding, cm/s^2
F.AIR_VZ      = 40     -- |vz| above this: airborne
F.FREE_SPEED  = 80     -- present mode needs this speed ...
F.FREE_DIST   = 160    -- ... and this distance from the owner's stand-in (cm)
F.LEAD_MAX    = 250
F.GAP_MS      = 400    -- a longer pause in a sender's stream: never interpolate across it
F.GAP_KEEP_CM = 40     -- ... except from a last pose this close to the new one (where it rested)
F.WF_ASLEEP, F.WF_HELD = 1, 2

local function v3(x, y, z) return { X = x, Y = y, Z = z } end
local function lerp(a, b, t) return a + (b - a) * t end
local function vlen(v) return math.sqrt(v.X * v.X + v.Y * v.Y + v.Z * v.Z) end

local function qmul(a, b)
    return { a[4] * b[1] + a[1] * b[4] + a[2] * b[3] - a[3] * b[2],
             a[4] * b[2] - a[1] * b[3] + a[2] * b[4] + a[3] * b[1],
             a[4] * b[3] + a[1] * b[2] - a[2] * b[1] + a[3] * b[4],
             a[4] * b[4] - a[1] * b[1] - a[2] * b[2] - a[3] * b[3] }
end
local function qconj(a) return { -a[1], -a[2], -a[3], a[4] } end
local function qnorm(q)
    local n = math.sqrt(q[1] * q[1] + q[2] * q[2] + q[3] * q[3] + q[4] * q[4])
    if not (n > 1e-9 and n < math.huge) then return { 0, 0, 0, 1 } end
    return { q[1] / n, q[2] / n, q[3] / n, q[4] / n }
end
local function nlerp(a, b, t)
    local d = a[1] * b[1] + a[2] * b[2] + a[3] * b[3] + a[4] * b[4]
    local s = d < 0 and -1 or 1
    return qnorm({ lerp(a[1], s * b[1], t), lerp(a[2], s * b[2], t), lerp(a[3], s * b[3], t), lerp(a[4], s * b[4], t) })
end
-- Angle of a rotation quaternion, degrees.
local function qangle(q)
    local w = math.min(1, math.abs(q[4]))
    return math.deg(2 * math.acos(w))
end
-- Rotation by angular velocity w (rad/s, world) over dt s.
local function qexp(w, dt)
    local ax, ay, az = w.X * dt, w.Y * dt, w.Z * dt
    local a = math.sqrt(ax * ax + ay * ay + az * az)
    if a < 1e-9 then return { 0, 0, 0, 1 } end
    local s = math.sin(a / 2) / a
    return { ax * s, ay * s, az * s, math.cos(a / 2) }
end
-- World angular velocity (rad/s) turning qa into qb over dt_ms.
local function ang_vel(qa, qb, dt_ms)
    if dt_ms <= 0 then return v3(0, 0, 0) end
    local d = qmul(qb, qconj(qa))
    if d[4] < 0 then d = { -d[1], -d[2], -d[3], -d[4] } end
    local w = math.max(-1, math.min(1, d[4]))
    local ang = 2 * math.acos(w)
    local s = math.sqrt(math.max(0, 1 - w * w))
    if s < 1e-6 or ang < 1e-6 then return v3(0, 0, 0) end
    local k = ang / (dt_ms / 1000) / s
    return v3(d[1] * k, d[2] * k, d[3] * k)
end
F.qmul, F.qangle, F.nlerp, F.ang_vel = qmul, qangle, nlerp, ang_vel

-- ---- per-sender clock ------------------------------------------------------------------
function F.clock_new() return { hist = {}, off = nil, delay = F.SEND_MS + 30, delay_fast = F.FAST_SEND_MS + 30, last_rx = nil } end

-- One new packet of this sender (its ts) seen at local ms rx.
function F.clock_note(c, ts, rx)
    local h = c.hist
    h[#h + 1] = { rx, rx - ts }
    while #h > 0 and (rx - h[1][1] > F.WIN_MS or #h > 80) do table.remove(h, 1) end
    local off = math.huge
    for _, e in ipairs(h) do if e[2] < off then off = e[2] end end
    -- a clock that jumped (a reload, a new session): start over from this sample
    if c.off and math.abs(off - c.off) > 2000 then
        c.hist = { { rx, rx - ts } }
        off = rx - ts
    end
    c.off = off
    local late = {}
    for _, e in ipairs(c.hist) do late[#late + 1] = e[2] - off end
    table.sort(late)
    local p90 = late[math.max(1, math.ceil(#late * 0.9))] or 0
    local want = math.max(F.D_MIN, math.min(F.D_MAX, F.SEND_MS + p90 + 10))
    if want > c.delay then
        c.delay = want
    elseif c.last_rx then
        c.delay = math.max(want, c.delay - (rx - c.last_rx) * F.SHRINK)
    end
    -- held items and bodies at their owner come at the fast rate and play like the owner's
    -- stand-in (poseplay: interval + p90 + 6): a sword stays in its hand, a pushed crate at
    -- its body
    local wh = math.max(F.D_MIN_FAST, math.min(F.D_MAX, F.FAST_SEND_MS + p90 + 6))
    if wh > c.delay_fast then
        c.delay_fast = wh
    elseif c.last_rx then
        c.delay_fast = math.max(wh, c.delay_fast - (rx - c.last_rx) * F.SHRINK)
    end
    c.last_rx = rx
end

-- Owner-clock time to show delayed bodies at, and the owner's present.
function F.render_ts(c, now) return now - c.off - c.delay end
function F.present_ts(c, now, lead) return now - c.off + math.max(0, math.min(F.LEAD_MAX, lead or 0)) end

-- ---- sampling ------------------------------------------------------------------------
-- buf: samples { t, pos, q, vel, flags } sorted by t. The newest sample's angular velocity
-- comes from the last two samples.
local function spin(buf)
    local n = #buf
    if n < 2 then return v3(0, 0, 0) end
    local a, b = buf[n - 1], buf[n]
    if b.t - a.t > 250 or (b.flags & F.WF_ASLEEP) ~= 0 then return v3(0, 0, 0) end
    return ang_vel(a.q, b.q, b.t - a.t)
end

-- Dead reckoning of sample s by dt_ms. floor: the lowest height the body may reach (a
-- landing is clamped there instead of predicting it through the ground).
function F.extrapolate(s, dt_ms, w, rot_ms, floor)
    if (s.flags & F.WF_ASLEEP) ~= 0 or dt_ms <= 0 then return s.pos, s.q end
    local dt = dt_ms / 1000
    local v = s.vel
    local p
    if math.abs(v.Z) > F.AIR_VZ then
        -- airborne: ballistic
        p = v3(s.pos.X + v.X * dt, s.pos.Y + v.Y * dt, s.pos.Z + v.Z * dt - 0.5 * F.G * dt * dt)
        if floor and p.Z < floor then p.Z = math.min(floor, s.pos.Z) end
    else
        -- sliding: friction brings it to a stop
        local sp = math.sqrt(v.X * v.X + v.Y * v.Y)
        if sp > 1e-3 then
            local ts = math.min(dt, sp / F.SLIDE_DECEL)
            local d = sp * ts - 0.5 * F.SLIDE_DECEL * ts * ts
            p = v3(s.pos.X + v.X / sp * d, s.pos.Y + v.Y / sp * d, s.pos.Z)
        else
            p = s.pos
        end
    end
    local q = s.q
    if w then q = qnorm(qmul(qexp(w, math.min(dt_ms, rot_ms or F.EXTRAP_ROT_MS) / 1000), s.q)) end
    return p, q
end

-- The pose at owner time t: interpolated between samples, or dead-reckoned past the
-- newest one (at most max_ms).
function F.at(buf, t, max_ms, floor)
    local n = #buf
    if n == 0 then return nil end
    if t <= buf[1].t then return buf[1].pos, buf[1].q, buf[1].flags end
    for i = 1, n - 1 do
        local a, b = buf[i], buf[i + 1]
        if t <= b.t then
            local k = (t - a.t) / math.max(b.t - a.t, 1)
            return v3(lerp(a.pos.X, b.pos.X, k), lerp(a.pos.Y, b.pos.Y, k), lerp(a.pos.Z, b.pos.Z, k)),
                   nlerp(a.q, b.q, k), b.flags
        end
    end
    local s = buf[n]
    local lo = floor
    for i = 1, n do if lo == nil or buf[i].pos.Z < lo then lo = buf[i].pos.Z end end
    local p, q = F.extrapolate(s, math.min(t - s.t, max_ms), spin(buf), math.min(max_ms, F.EXTRAP_ROT_MS), lo)
    return p, q, s.flags
end

-- Should this body be shown in the present? (it flies free of its owner)
function F.free_flight(s, owner_dist)
    if (s.flags & (F.WF_HELD | F.WF_ASLEEP)) ~= 0 then return false end
    return vlen(s.vel) > F.FREE_SPEED and (owner_dist == nil or owner_dist > F.FREE_DIST)
end

-- ---- the displayed pose ------------------------------------------------------------------
-- st: per-body display state (o.fw). Call F.before_change(st, buf) before a sample goes
-- into buf. c: the sender's clock; ctx = { now, dt (s), lead, owner_dist, floor }.
-- Returns pos, q, flags, snapped (a correction too large to blend, applied at once).

F.CATCHUP_MS = 250   -- delayed -> present: play faster (at least this long) instead of jumping

function F.before_change(st, buf)
    if st.old_buf then return end
    local copy = {}
    for i, s in ipairs(buf) do copy[i] = s end
    st.old_buf = copy
end

-- u = 0: the delayed timeline, 1: the present; in between while catching up.
local function eval(buf, u, off, delay, ctx)
    local td = ctx.now - off - delay
    local tp = ctx.now - off + math.max(0, math.min(F.LEAD_MAX, ctx.lead or 0))
    return F.at(buf, td + (tp - td) * u, u > 0 and F.EXTRAP_MS or F.HOLD_EXTRAP_MS, ctx.floor)
end

function F.pose(st, buf, c, ctx)
    local n = #buf
    if n == 0 or not c or not c.off then return nil end
    local s = buf[n]
    local was_u = st.u or 0
    local u = was_u
    if (s.flags & F.WF_HELD) ~= 0 then
        u = 0
    elseif u > 0 or F.free_flight(s, ctx.owner_dist) then
        -- at most 1.5 times the real speed while catching up (the gap is delay + lead)
        local gap = c.delay + math.max(0, math.min(F.LEAD_MAX, ctx.lead or 0))
        u = math.min(1, u + math.max(ctx.dt or 0, 0.001) * 1000 / math.max(F.CATCHUP_MS, gap * 2))
    end
    st.u = u
    local contact = (s.flags & F.WF_HELD) ~= 0 or (ctx.owner_dist ~= nil and ctx.owner_dist < F.CONTACT_CM)
    local delay = contact and (c.delay_fast or c.delay) or c.delay
    local tp, tq, flags = eval(buf, u, c.off, delay, ctx)
    local snapped = false
    local o, qo = st.off or v3(0, 0, 0), st.qoff or { 0, 0, 0, 1 }
    if st.rebase and st.last_p then
        -- another sender / timeline: continue from what is on screen
        o = v3(st.last_p.X - tp.X, st.last_p.Y - tp.Y, st.last_p.Z - tp.Z)
        qo = qnorm(qmul(st.last_q, qconj(tq)))
        if vlen(o) > F.SNAP_CM then o, snapped = v3(0, 0, 0), true end
    elseif st.shown then
        -- Decay the old offset, then add what changed the target this frame (a new sample,
        -- a jump back to the delayed timeline, a clock update): the old inputs' pose now
        -- minus the new one. The catch-up itself is continuous (no offset).
        local k = math.exp(-(ctx.dt or 0) * 1000 / F.BLEND_MS)
        local ol = vlen(o)
        if ol > 1e-6 then
            local nl = math.max(ol * k, ol - F.FIX_SPEED * (ctx.dt or 0))
            o = v3(o.X * nl / ol, o.Y * nl / ol, o.Z * nl / ol)
        end
        qo = nlerp({ 0, 0, 0, 1 }, qo, k)
        local back = u < was_u
        local changed = st.old_buf ~= nil or back or st.c_off ~= c.off or st.c_delay ~= delay
        if changed then
            local op, oq = eval(st.old_buf or buf, back and was_u or u, st.c_off or c.off, st.c_delay or delay, ctx)
            if op then
                o = v3(o.X + op.X - tp.X, o.Y + op.Y - tp.Y, o.Z + op.Z - tp.Z)
                qo = qnorm(qmul(qo, qmul(oq, qconj(tq))))
            end
        end
        if vlen(o) > F.SNAP_CM then o, snapped = v3(0, 0, 0), true end
        if qangle(qo) > F.SNAP_DEG then qo, snapped = { 0, 0, 0, 1 }, true end
    end
    st.old_buf, st.c_off, st.c_delay, st.shown, st.rebase = nil, c.off, delay, true, nil
    st.off, st.qoff = o, qo
    st.max_off = math.max(st.max_off or 0, vlen(o))
    local p, q = v3(tp.X + o.X, tp.Y + o.Y, tp.Z + o.Z), qnorm(qmul(qo, tq))
    st.last_p, st.last_q = p, q
    return p, q, flags, snapped
end

-- The samples now come from another sender (a new owner): blend from what is shown.
function F.rebase(st) st.old_buf, st.rebase = nil, true end

-- A body that starts (or stops) following starts from its target, no offset.
function F.reset(st)
    st.old_buf, st.c_off, st.c_delay, st.shown, st.off, st.qoff, st.u = nil, nil, nil, false, nil, nil, 0
end

return F
