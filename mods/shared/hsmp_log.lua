-- hsmp_log.lua -- HSMP structured event log (JSONL), shared by every HSMP mod.
--
-- build-and-deploy.ps1 copies this file into every mod's Scripts/ directory;
-- do not edit the per-mod copies.
--
-- Every event is one JSON object per line in <state_dir>/hsmp_events.jsonl:
--   {"v":1,"ev":"world_ready","inst":"2","mod":"HSMPMatch","seq":7,
--    "t_ms":81234,"wall_ms":1759340000123, ...event fields...}
-- The JSONL file is the only default sink. With HSMP_LOG_ECHO=1 (or
-- init{echo=true}) every line is also echoed to UE4SS.log as
--   [hsmp_ev] {...same json...}
-- (the harness sets it; "inst" attributes a line when two game instances
-- share one UE4SS.log). The echo is off by default: a normal session would
-- print hundreds of [hsmp_ev] lines into the player's UE4SS.log.
--
-- The event vocabulary (M.EVENTS) is frozen: new names are additive only, and
-- experiments use an "x_" prefix.
--
-- Threading: call only from the game thread (RegisterHook callbacks,
-- LoopInGameThreadWithDelay / ExecuteInGameThreadWithDelay bodies, keybinds
-- marshalled with ExecuteInGameThread). Each call opens the file in append
-- mode, writes one complete line and closes it, so several mods (separate Lua
-- states) can share the file and rotation can rename it safely. Never call it
-- from LoopAsync / ExecuteWithDelay without the HSMP thread-safety shim.
--
-- Usage (in a mod's main.lua):
--   local HL = require("hsmp_log")            -- or dofile next to main.lua
--   HL.init("HSMPMatch")                      -- or HL.init{mod="HSMPMatch", state_dir=STATE_DIR}
--   HL.event("world_ready", { arena = "Map_Arena_Alley", world_key = wk })
--   HL.travel(from, to, "director")           -- convenience wrappers below

local M = {}

M.VERSION = 1
M.FILE_NAME = "hsmp_events.jsonl"

-- Frozen vocabulary: name -> list of REQUIRED fields. Optional fields are
-- documented in docs/development/testing.md. A missing required field does not drop the
-- event; it is written with "_missing":[...] so the assert step reports it.
M.EVENTS = {
    lobby_ready             = {},
    cmd_sent                = { "cmd", "cmd_id" },
    cmd_result              = { "cmd", "cmd_id", "ok" },
    travel                  = { "from", "to", "by" },
    world_ready             = { "arena", "world_key" },
    spawn_verified          = { "round", "ok" },
    kit_verified            = { "who", "armour_n", "r_class", "l_class" },
    willie_census           = { "visible", "expected" },
    save_redirected         = { "fn", "slot", "to_slot" },
    native_travel_rewritten = { "from", "to" },
    rvp_hold_timeout        = { "why", "waited_s" },   -- shared/hsmp_rvp.lua
    hitch                   = { "ms" },          -- + world_key, travel (M.frame)
    frame_hb                = { "n", "max_ms" }, -- M.frame heartbeat, every 10 s
    -- the Director's connection state machine (director.lua set_conn / lose /
    -- step_conn)
    conn_state              = { "state" },
    travel_reason           = { "reason" },
    resume                  = { "ok" },
    -- the Director reloads for a deathmatch respawn order (director.lua)
    respawn                 = { "round", "spawn_id" },
    -- every 5 s during a round (HSMPCombat emit_quality)
    combat_quality          = {},
    ready_report            = { "round" },
    -- `phase{from,to,match_id,round}` is a SERVER event (hsmp-server --events);
    -- it is listed so a Lua mod mirroring it does not trip the vocabulary check.
    phase                   = { "from", "to" },
    -- every 5 s per remote peer during Live
    -- (also carries jitter_ratio, foot_slide_p95, idle_rms)
    pose_quality            = { "peer", "arm_p95_uu", "tip_p95_uu", "latency_ms" },
    -- SMOOTH-1: every 5 s per remote peer during Live (HSMPAvatars), and every
    -- move of the local pawn (HSMPSync spawn_place, HSMPAvatars launch clamp)
    netfeel                 = { "peer", "snaps_per_min", "window_s" },
    pawn_correction         = { "why", "live" },
    -- SPAWN-1: joint stretch after a stand-in starts / is re-posed, and after our spawn
    spawn_stretch           = { "who", "max_uu" },
    -- emitted by hsmp-server / hsmp-sidecar --events; listed so a mod
    -- mirroring them passes the vocabulary check
    load_failed             = { "round", "error" },
    round_void              = { "round" },
    death_ignored           = { "why" },
    notice                  = {},
    cmd_timeout             = { "cmd", "cmd_id" },
    -- the server's verdict on this client's world consistency report (HSMPWorld,
    -- every ~5 s while in an arena; also emitted by hsmp-server --events with
    -- peer/other). See docs/development/subsystems/world-replication.md.
    world_consistency       = { "hash_match", "mismatched" },
    world_track             = { "nid", "t", "x", "y", "z", "mode", "rest" },
    world_sync_quality      = { "hard_snaps", "max_off_cm" },
    -- the local pawn's state at placement / Ready / every 5 s while
    -- spawn-protected / Live / protection end (HSMPSync spawn_place.lua,
    -- at=placed|ready|protect|live|protect_end) and on a kit weapon drop
    -- (HSMPLoadout kit.lua, at=rearm|weapon_drop).
    -- Also carries at, round, pawn, fallen, health, protected, live, dist_cm, reason.
    -- Expected while protected=true: no Downed=true, consciousness 100, a
    -- weapon_r (docs/development/subsystems/spawns.md).
    pawn_state              = { "consciousness", "downed", "weapon_r", "weapon_l" },
}

-- Meta events written by this library itself.
M.META = { _open = true, _bad_event = true, _rotated = true }

local cfg = {
    mod = "?",
    inst = "0",
    state_dir = "hsmp_state",
    path = nil,
    echo = false,                  -- UE4SS.log echo: HSMP_LOG_ECHO=1 or init{echo=true}
    max_bytes = 8 * 1024 * 1024,   -- rotate above this size
    keep = 4,                      -- hsmp_events.1.jsonl .. .4.jsonl
    hitch_ms = 2000,               -- M.frame(): report gaps above this (> 2 s)
    hb_s = 10,                     -- M.frame(): frame_hb heartbeat period
}
local seq = 0
local wall_base, clk_base = nil, nil
local last_travel_clk = -1e9
local stats = { written = 0, failed = 0, rotated = 0 }
M.stats = stats

-- --- clocks -------------------------------------------------------------
-- os.clock() on Windows (MSVCRT clock()) is wall time since process start
-- with ms resolution; os.time() is absolute but 1 s resolution. wall_ms is
-- anchored on both and re-synced whenever os.time() proves it drifted, so it
-- converges to within the spacing of events (good enough to merge instances).

local function now_clk() return os.clock() end

local function wall_ms()
    local c = now_clk()
    if not wall_base then
        wall_base, clk_base = os.time() * 1000, c
    end
    local w = wall_base + (c - clk_base) * 1000
    local t = os.time() * 1000
    if w < t then
        wall_base, clk_base = t, c; w = t
    elseif w >= t + 1000 then
        wall_base, clk_base = t + 999, c; w = t + 999
    end
    return math.floor(w)
end
M.wall_ms = wall_ms

-- --- JSON encoder (compact, deterministic key order) --------------------

local esc_map = { ['"'] = '\\"', ['\\'] = '\\\\', ['\b'] = '\\b', ['\f'] = '\\f',
                  ['\n'] = '\\n', ['\r'] = '\\r', ['\t'] = '\\t' }
local function esc_str(s)
    return '"' .. s:gsub('[%c"\\]', function(ch)
        return esc_map[ch] or string.format("\\u%04x", ch:byte())
    end) .. '"'
end

local encode
local function is_array(t)
    local n = 0
    for k in pairs(t) do
        if type(k) ~= "number" or k < 1 or k % 1 ~= 0 then return false end
        n = n + 1
    end
    for i = 1, n do if t[i] == nil then return false end end
    return true, n
end

encode = function(v, depth)
    depth = depth or 0
    local tv = type(v)
    if v == nil then return "null"
    elseif tv == "boolean" then return v and "true" or "false"
    elseif tv == "number" then
        if v ~= v or v == math.huge or v == -math.huge then return "null" end
        if v % 1 == 0 and v > -2^53 and v < 2^53 then return string.format("%d", v) end
        -- shortest of %.15g / %.17g that round-trips (positions, clocks > 1e6)
        local s = string.format("%.15g", v)
        if tonumber(s) ~= v then s = string.format("%.17g", v) end
        return s
    elseif tv == "string" then return esc_str(v)
    elseif tv == "table" then
        if depth > 8 then return '"<depth>"' end
        local arr, n = is_array(v)
        if arr and n > 0 then
            local parts = {}
            for i = 1, n do parts[i] = encode(v[i], depth + 1) end
            return "[" .. table.concat(parts, ",") .. "]"
        end
        local keys = {}
        for k in pairs(v) do keys[#keys + 1] = tostring(k) end
        table.sort(keys)
        local parts = {}
        for _, k in ipairs(keys) do
            local val = v[k]
            if val == nil then val = v[tonumber(k)] end
            parts[#parts + 1] = esc_str(k) .. ":" .. encode(val, depth + 1)
        end
        return "{" .. table.concat(parts, ",") .. "}"
    else
        return esc_str(tostring(v))   -- userdata / functions: never touch them
    end
end
M.encode = encode

-- --- file I/O --------------------------------------------------------------

local function trim(s) return s and s:match("^%s*(.-)%s*$") or nil end

local function rotated_name(i)
    return (cfg.path:gsub("%.jsonl$", "")) .. "." .. i .. ".jsonl"
end

-- os.rename() returns nil on failure (it does not raise). If the chain were
-- shifted first, a locked live log (another reader / AV scanner holding it)
-- would shift it on every event and lose all history within `keep` events.
-- So the live file is moved aside first; only when that worked is the chain
-- shifted. Returns true when it rotated.
local rotate_backoff_until = -math.huge
local function rotate()
    local hold = cfg.path .. ".rotating"
    os.remove(hold)
    if not os.rename(cfg.path, hold) then return false end
    os.remove(rotated_name(cfg.keep))
    for i = cfg.keep - 1, 1, -1 do os.rename(rotated_name(i), rotated_name(i + 1)) end
    if not os.rename(hold, rotated_name(1)) then
        os.rename(hold, cfg.path)   -- put it back; the next attempt retries
        return false
    end
    stats.rotated = stats.rotated + 1
    return true
end

local function append_line(line)
    local f = io.open(cfg.path, "ab")
    if not f then stats.failed = stats.failed + 1; return false end
    local size = f:seek("end") or 0
    if size + #line > cfg.max_bytes and size > 0 and now_clk() >= rotate_backoff_until then
        f:close()
        local pok, rotated = pcall(rotate)
        local ok = pok and rotated == true
        if not ok then rotate_backoff_until = now_clk() + 30 end   -- back off, keep appending
        f = io.open(cfg.path, "ab")
        if not f then stats.failed = stats.failed + 1; return false end
        if ok then
            -- first line of the fresh file records the rotation
            seq = seq + 1
            f:write(encode({ v = M.VERSION, ev = "_rotated", inst = cfg.inst, mod = cfg.mod,
                             seq = seq, t_ms = math.floor(now_clk() * 1000), wall_ms = wall_ms() }), "\n")
        end
    end
    f:write(line, "\n")
    f:close()
    stats.written = stats.written + 1
    return true
end

-- --- public API ------------------------------------------------------------

-- HSMP_LOG_ECHO value -> echo on? Only an explicit "1"/"true"/"yes"/"on" turns
-- the UE4SS.log echo on; unset, "0" or anything else keeps it off.
function M.echo_from_env(v)
    v = trim(v)
    if not v then return false end
    v = v:lower()
    return v == "1" or v == "true" or v == "yes" or v == "on"
end
function M.echo() return cfg.echo end

-- init(mod_name [, opts])  or  init{mod=..., state_dir=..., ...}
-- opts: state_dir, inst, echo, max_bytes, keep, hitch_ms, path.
-- Defaults: HSMP_STATE_DIR (or "hsmp_state"), HSMP_INST (or "0").
function M.init(mod_name, opts)
    if type(mod_name) == "table" then
        opts, mod_name = mod_name, mod_name.mod
    end
    opts = opts or {}
    cfg.mod = tostring(mod_name or "?")
    cfg.inst = tostring(opts.inst or trim(os.getenv("HSMP_INST")) or "0")
    if cfg.inst == "" then cfg.inst = "0" end
    cfg.state_dir = ((opts.state_dir or trim(os.getenv("HSMP_STATE_DIR")) or "hsmp_state"):gsub("\\", "/"))
    if cfg.state_dir == "" then cfg.state_dir = "hsmp_state" end
    cfg.path = opts.path or (cfg.state_dir .. "/" .. M.FILE_NAME)
    if opts.echo ~= nil then cfg.echo = opts.echo and true or false
    else cfg.echo = M.echo_from_env(os.getenv("HSMP_LOG_ECHO")) end
    cfg.max_bytes = tonumber(opts.max_bytes) or cfg.max_bytes
    cfg.keep = math.max(1, tonumber(opts.keep) or cfg.keep)
    cfg.hitch_ms = tonumber(opts.hitch_ms) or cfg.hitch_ms
    cfg.hb_s = tonumber(opts.hb_s) or cfg.hb_s
    M.emit("_open", { lib_v = M.VERSION, state_dir = cfg.state_dir })
    return M
end

function M.path() return cfg.path end
function M.inst() return cfg.inst end

local ENVELOPE = { v = true, ev = true, inst = true, mod = true, seq = true, t_ms = true, wall_ms = true }

-- Low-level writer; no vocabulary check. Returns the encoded line (or nil).
-- Caller fields named like an envelope key (v, ev, inst, mod, seq, t_ms,
-- wall_ms) are written as f_<key> (e.g. world_consistency.seq -> f_seq).
function M.emit(ev, fields)
    if not cfg.path then M.init(cfg.mod) end
    seq = seq + 1
    local rec = {}
    if type(fields) == "table" then
        for k, v in pairs(fields) do
            -- a caller field that collides with the envelope is kept as f_<key>
            if ENVELOPE[k] then rec["f_" .. k] = v else rec[k] = v end
        end
    end
    rec.v, rec.ev, rec.inst, rec.mod, rec.seq = M.VERSION, ev, cfg.inst, cfg.mod, seq
    rec.t_ms, rec.wall_ms = math.floor(now_clk() * 1000), wall_ms()
    local ok, line = pcall(encode, rec)
    if not ok then
        line = encode({ v = M.VERSION, ev = "_bad_event", bad_ev = tostring(ev), err = tostring(line),
                        inst = cfg.inst, mod = cfg.mod, seq = seq, t_ms = rec.t_ms, wall_ms = rec.wall_ms })
    end
    pcall(append_line, line)
    if cfg.echo then pcall(print, "[hsmp_ev] " .. line .. "\n") end
    return line
end

-- Checked writer: the name must be in the frozen vocabulary (or "x_*").
function M.event(ev, fields)
    ev = tostring(ev)
    local req = M.EVENTS[ev]
    if not req and not ev:match("^x_") then
        return M.emit("_bad_event", { bad_ev = ev, reason = "not in vocabulary" })
    end
    if req and #req > 0 then
        local missing
        for _, k in ipairs(req) do
            if type(fields) ~= "table" or fields[k] == nil then
                missing = missing or {}
                missing[#missing + 1] = k
            end
        end
        if missing then
            local f2 = {}
            if type(fields) == "table" then for k, v in pairs(fields) do f2[k] = v end end
            f2._missing = missing
            fields = f2
        end
    end
    if ev == "travel" then last_travel_clk = now_clk() end
    return M.emit(ev, fields)
end

-- Convenience wrappers (argument order = the required fields in M.EVENTS).
function M.lobby_ready(f)                    return M.event("lobby_ready", f or {}) end
function M.cmd_sent(cmd, cmd_id, f)          f = f or {}; f.cmd, f.cmd_id = cmd, cmd_id; return M.event("cmd_sent", f) end
function M.cmd_result(cmd, cmd_id, ok, reason, f)
    f = f or {}; f.cmd, f.cmd_id, f.ok, f.reason = cmd, cmd_id, ok and true or false, reason
    return M.event("cmd_result", f)
end
function M.travel(from, to, by, f)           f = f or {}; f.from, f.to, f.by = from, to, by; return M.event("travel", f) end
function M.world_ready(arena, world_key, f)  f = f or {}; f.arena, f.world_key = arena, world_key; return M.event("world_ready", f) end
function M.spawn_verified(f)                 return M.event("spawn_verified", f or {}) end
function M.kit_verified(who, armour_n, r_class, l_class, f)
    f = f or {}; f.who, f.armour_n, f.r_class, f.l_class = who, armour_n, r_class, l_class
    return M.event("kit_verified", f)
end
function M.willie_census(visible, expected, f)
    f = f or {}; f.visible, f.expected = visible, expected
    return M.event("willie_census", f)
end
function M.save_redirected(fn, slot, to_slot, f)
    f = f or {}; f.fn, f.slot, f.to_slot = fn, slot, to_slot
    return M.event("save_redirected", f)
end
function M.native_travel_rewritten(from, to, f)
    f = f or {}; f.from, f.to = from, to
    return M.event("native_travel_rewritten", f)
end
function M.hitch(ms, f)                      f = f or {}; f.ms = ms; return M.event("hitch", f) end
function M.ready_report(round, f)            f = f or {}; f.round = round; return M.event("ready_report", f) end
function M.pose_quality(peer, arm_p95_uu, tip_p95_uu, latency_ms, f)
    f = f or {}; f.peer, f.arm_p95_uu, f.tip_p95_uu, f.latency_ms = peer, arm_p95_uu, tip_p95_uu, latency_ms
    return M.event("pose_quality", f)
end

-- Hitch detector + frame heartbeat (docs/development/testing.md):
--   hitch{ms, world_key, travel:bool}   a gap between calls of more than
--                                        expected_ms + hitch_ms (default 2000)
--   frame_hb{n, max_ms}                 every hb_s (10 s): frames seen since the
--                                        last heartbeat and the largest gap
-- Call M.frame() from one game-thread loop per process. HSMPMatch owns it (it
-- is in the RELEASE profile; HSMPDiag is dev-only). `travel` is true when the
-- caller says so, or when this Lua state logged a travel event in the last
-- 30 s (level loads legitimately block the game thread).
-- Forms: M.frame(expected_ms [, in_travel [, world_key]])
--        M.frame{ expected_ms=, travel=, world_key= }
local last_frame_clk = nil
local hb = { n = 0, max_ms = 0, at = nil }
function M.frame(expected_ms, in_travel, world_key)
    if type(expected_ms) == "table" then
        local o = expected_ms
        expected_ms, in_travel, world_key = o.expected_ms, o.travel, o.world_key
    end
    local c = now_clk()
    hb.at = hb.at or c
    if last_frame_clk then
        local raw = (c - last_frame_clk) * 1000
        local gap = raw - (expected_ms or 0)
        hb.n = hb.n + 1
        if raw > hb.max_ms then hb.max_ms = raw end
        if gap > cfg.hitch_ms then
            if in_travel == nil then in_travel = (c - last_travel_clk) < 30 end
            M.hitch(math.floor(gap), { travel = in_travel and true or false,
                                       world_key = world_key and tostring(world_key) or "none" })
        end
    end
    last_frame_clk = c
    if c - hb.at >= cfg.hb_s then
        M.event("frame_hb", { n = hb.n, max_ms = math.floor(hb.max_ms) })
        hb.n, hb.max_ms, hb.at = 0, 0, c
    end
end

-- Test hook: reset module state (offline tests only).
function M._reset()
    seq, wall_base, clk_base, last_frame_clk, last_travel_clk = 0, nil, nil, nil, -1e9
    hb.n, hb.max_ms, hb.at = 0, 0, nil
    stats.written, stats.failed, stats.rotated = 0, 0, 0
    cfg.path = nil
    cfg.echo = false
end

return M
