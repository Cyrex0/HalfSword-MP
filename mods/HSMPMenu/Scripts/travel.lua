-- HSMPMenu / travel.lua — the menu only REQUESTS level travel.
-- The Director (HSMPMatch/Scripts/director.lua) owns every level change.
-- Contract: docs/development/subsystems/director-contract.md.
--
--   Travel.request(want, arena, reason, on_done) -> seq
--     want   = "arena" | "menu"
--     arena  = server arena short name ("Map_Arena_Pit") for want == "arena"
--     on_done(result) with result.status = "accepted" | "refused" | "legacy"
--                                          | "world_changed" | "superseded"
--
-- Channels to the Director (both are tried, the bus key is normative):
--   1. bus key `travel_request` (typed record {want, arena, reason, seq, t, from};
--      seq strictly increasing).
--   2. the global HSMP_DIRECTOR.request_travel(req) if it exists in this Lua
--      state (UE4SS mods normally have separate states, so usually it does not).
-- Acknowledgement: HSMP_DIRECTOR.request_travel returning a truthy value, or
--   bus key `travel_ack` {seq, status = "accepted"|"refused", reason, arena}
-- Director liveness: bus key `director` {hb = <unix seconds>, ...} refreshed >= 1 Hz.
--
-- COMPATIBILITY SHIM (opt-in, HSMP_LEGACY_TRAVEL=1): no ack within ACK_TIMEOUT_MS
-- and no live Director -> log "DIRECTOR MISSING - legacy travel" and call
-- ctx.fallback(req) (main.lua -> legacy_travel.lua). To remove the shim,
-- delete legacy_travel.lua; with no fallback the request is reported refused
-- and nothing travels locally.

local T = {}

local ctx
local Log = function() end

T.ACK_TIMEOUT_MS = 1000        -- no ack by then and no live Director -> legacy shim
T.ALIVE_WAIT_MS = 5000         -- a live Director gets this long to ack before the shim
T.HEARTBEAT_MAX_AGE_S = 5      -- bus `director` hb older than this = no Director

T.seq = 0
T.pending = nil

-- travel_request / travel_ack / director are typed bus keys (schema session.rs).
local function bus(key)
    local ipc = rawget(_G, "HSMP_IPC")
    return (ipc and ipc.bus_table and ipc.bus_table(key)) or nil
end

-- The travel_request record of `req`.
function T.encode(req)
    return { want = req.want, arena = req.arena or "", reason = req.reason or "", seq = req.seq, t = req.t, from = "HSMPMenu" }
end

local function put_request(req)
    local ipc = rawget(_G, "HSMP_IPC")
    return ipc and ipc.bus_put and ipc.bus_put("travel_request", T.encode(req)) and true or false
end

local function director_global()
    local d = rawget(_G, "HSMP_DIRECTOR")
    if type(d) == "table" and type(d.request_travel) == "function" then return d end
    return nil
end

-- Is a Director running (global in this state, or a fresh bus heartbeat)?
function T.director_alive()
    if director_global() then return true end
    local d = bus("director")
    local hb = d and d.state ~= "" and tonumber(d.hb) or nil
    return hb ~= nil and math.abs(os.time() - hb) <= T.HEARTBEAT_MAX_AGE_S
end

function T.read_ack(seq)
    local a = bus("travel_ack")
    if not a or a.seq == 0 or a.seq ~= seq then return nil end
    return { seq = a.seq, status = (a.status ~= "" and a.status) or "accepted",
             reason = a.reason, arena = a.arena }
end

local function finish(p, result)
    if T.pending ~= p then return end
    T.pending = nil
    result.seq = p.req.seq
    Log("travel #%d %s -> %s%s", p.req.seq, p.req.want, result.status,
        result.reason and (" (" .. result.reason .. ")") or "")
    if p.on_done then
        local ok, err = pcall(p.on_done, result, p.req)
        if not ok then Log("travel on_done error: %s", tostring(err)) end
    end
end

local function check(p)
    if T.pending ~= p then return end                       -- superseded / finished
    if ctx.world_gen() ~= p.gen then                         -- the world already changed: someone travelled
        finish(p, { status = "world_changed" }); return
    end
    local ack = T.read_ack(p.req.seq)
    if ack then
        finish(p, { status = ack.status, reason = ack.reason, arena = ack.arena, by = "director" }); return
    end
    local waited = ctx.now_ms() - p.t0
    if T.director_alive() and waited < T.ALIVE_WAIT_MS then
        if not p.warned then
            p.warned = true
            Log("travel #%d: Director alive but no ack after %d ms - waiting", p.req.seq, waited)
        end
        ctx.delay(T.ACK_TIMEOUT_MS, function() check(p) end)
        return
    end
    -- COMPATIBILITY SHIM ---------------------------------------------------------------
    if ctx.fallback then
        Log("DIRECTOR MISSING - legacy travel (#%d want=%s arena=%s reason=%s)",
            p.req.seq, p.req.want, tostring(p.req.arena), tostring(p.req.reason))
        finish(p, { status = "legacy", by = "menu_legacy" })
        ctx.fallback(p.req)
    else
        Log("DIRECTOR MISSING and no legacy shim - NOT travelling (#%d; the shim is opt-in: HSMP_LEGACY_TRAVEL=1)", p.req.seq)
        finish(p, { status = "refused", reason = "no director" })
    end
end

function T.request(want, arena, reason, on_done)
    T.seq = T.seq + 1
    local req = { want = want, arena = arena, reason = reason or "", seq = T.seq, t = os.time() }
    if T.pending then finish(T.pending, { status = "superseded", reason = "newer request #" .. req.seq }) end
    local p = { req = req, t0 = ctx.now_ms(), gen = ctx.world_gen(), on_done = on_done }
    T.pending = p
    if not put_request(req) then
        Log("travel #%d: cannot publish the travel_request bus key", req.seq)
    end
    Log("travel request #%d: want=%s arena=%s reason=%s", req.seq, want, tostring(arena), tostring(reason))
    local d = director_global()
    if d then
        local ok, r = pcall(d.request_travel, req)
        if ok and r then
            local st = (type(r) == "table" and r.status) or "accepted"
            finish(p, { status = st, reason = type(r) == "table" and r.reason or nil,
                        arena = type(r) == "table" and r.arena or nil, by = "director" })
            return req.seq
        end
    end
    ctx.delay(T.ACK_TIMEOUT_MS, function() check(p) end)
    return req.seq
end

-- ctx: { state_dir, log, now_ms(), world_gen(), delay(ms, fn) (game thread),
--        fallback(req) or nil }
function T.init(c)
    ctx = c
    Log = c.log or Log
    -- continue after the seq of a request left over from an earlier load of this mod:
    -- the Director treats the key present at its start as already consumed.
    local r = bus("travel_request")
    local old = r and tonumber(r.seq)
    if old and old > T.seq then T.seq = old end
end

return T
