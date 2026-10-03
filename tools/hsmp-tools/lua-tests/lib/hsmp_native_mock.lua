-- hsmp_native_mock.lua -- an in-memory HSMPNative (docs/development/
-- ipc-shared-memory.md) for the offline Lua tests. It keeps the
-- contract's observable semantics: nothing raises, kinds by name, reused
-- caller-owned output tables, generations, per-cursor event fan-out, a
-- bounded G2S ring ("full"), caps gating, world epochs. The "sidecar" side
-- (sc_*) lets a test play the other process.
--
--   local NM = require("hsmp_native_mock")
--   local N = NM.new{ caps = NM.CAPS_ALL }     -- then _G.HSMPNative = N, or IPC.init{native = N}

local M = {}

-- The typed-record mixin (lib/hsmp_native_records.lua), found by require or next to this file.
local RECORDS_SRC = (rawget(_G, "debug") and ((debug.getinfo(1, "S").source or ""):gsub("^@", ""))) or ""
function M.records()
    local ok, R = pcall(require, "hsmp_native_records")
    if ok and type(R) == "table" then return R end
    local dir = rawget(_G, "HSMP_TEST_LIB") or RECORDS_SRC:match("^(.*)[/\\]") or "."
    return dofile(dir .. "/hsmp_native_records.lua")
end

-- The pose hot paths (lib/hsmp_native_pose.lua), found by require or next to this file.
function M.pose()
    local ok, P = pcall(require, "hsmp_native_pose")
    if ok and type(P) == "table" then return P end
    local dir = rawget(_G, "HSMP_TEST_LIB") or RECORDS_SRC:match("^(.*)[/\\]") or "."
    return dofile(dir .. "/hsmp_native_pose.lua")
end

local function load_schema()
    local ok, S = pcall(require, "hsmp_ipc_schema")
    if ok and type(S) == "table" then return S end
    return dofile(T.path("mods/shared/hsmp_ipc_schema.lua"))
end
local S = load_schema()
M.S = S

M.CAPS_ALL = 0
for _, v in pairs(S.CAPS) do M.CAPS_ALL = M.CAPS_ALL | v end

local function deep(v)
    if type(v) ~= "table" then return v end
    local o = {}
    for k, x in pairs(v) do o[k] = deep(x) end
    return o
end
M.deep = deep

-- o: caps (offered by both sides; default all), game_caps, abi = {major, hash}, ring = G2S capacity,
--    log = 4096 (event log size)
function M.new(o)
    o = o or {}
    local N = {}
    local st = {
        caps_game = o.game_caps or o.caps or M.CAPS_ALL,
        caps_sidecar = o.caps or M.CAPS_ALL,
        sidecar_state = o.sidecar_state or "ready",
        hb_age = o.hb_age or 0.01,
        abi_major = (o.abi and o.abi[1]) or S.ABI_MAJOR,
        hash = (o.abi and o.abi[2]) or S.LAYOUT_HASH,
        flags = 0, world_epoch = 1, world_key = nil, loading = false,
        frames = 0, opened = nil,
        local_ = {},       -- local_root / local_weapon / local_pose / pose_lead -> last args
        puts = {},         -- every put / put_* call, in order {kind, ...}
        ring = o.ring or 512,      -- G2S capacity (typed sends: lib/hsmp_native_records.lua)
        log = {}, log_base = 1, log_max = o.log or 4096,   -- event log: absolute index -> record
        cursors = {},      -- cursor key -> { next = abs index, filter = set|nil }
        dir = {},          -- slot -> { id, slot, gen, nick, play_seq, root_seq, vitals_seq, kit_seq }
        peer = {},         -- "peer_play" -> slot -> { seq, tbl }
        calls = {},        -- name -> count
    }
    N._st = st
    local function count(name) st.calls[name] = (st.calls[name] or 0) + 1 end
    local function caps_eff()
        if st.sidecar_state ~= "ready" then return 0 end
        return st.caps_game & st.caps_sidecar
    end
    local function kind_ok(kind)
        local k = S.KINDS[kind]
        if not k then return nil, "bad" end
        local caps = (k.dir == "local") and st.caps_game or caps_eff()
        if caps & k.cap == 0 then return nil, "cap" end
        return k
    end
    local cursor_key = function() return o.cursor or "default" end
    function N._set_cursor(key) cursor_key = function() return key end end
    function N._cursor_key() return cursor_key() end

    -- ---- game-side API (C.1) ------------------------------------------------
    function N.abi() count("abi"); return st.abi_major, 0, st.hash end
    function N.now_us() return math.floor(os.clock() * 1e6) end
    function N.thread_ok() return true end
    function N.ipc_open()
        count("ipc_open")
        st.opened = st.opened or ("Local\\HSMP.ipc.1.4242." .. "1d9a2b3c4d5e6f7")
        return st.opened
    end
    function N.ipc_info()
        count("ipc_info")
        return { name = st.opened, state = "ready", abi_major = st.abi_major, abi_minor = 0, layout_hash = st.hash,
                 caps_game = st.caps_game, caps_sidecar = st.caps_sidecar, caps_effective = caps_eff(),
                 sidecar_state = st.sidecar_state, sidecar_hb_age_s = st.hb_age, world_epoch = st.world_epoch,
                 session_epoch = 1, attach_count = 1, refuse_code = 0, refuse_detail = "", counters = {} }
    end
    function N.frame(world_key)
        count("frame")
        st.frames = st.frames + 1
        local f = st.flags
        st.flags = 0
        if world_key ~= st.world_key then
            if st.world_key ~= nil then f = f | S.FLAGS.WORLD_CHANGED end
            st.world_key = world_key
        end
        st.last_flags = f
        return f
    end
    function N.flags() return st.last_flags or 0 end
    function N.world_leaving()
        count("world_leaving")
        st.world_epoch = st.world_epoch + 1
        st.loading = true
        return true
    end
    function N.world_ready(key)
        count("world_ready")
        st.loading, st.ready_key = false, key
        return true
    end
    local function rec(kind, ...) st.puts[#st.puts + 1] = { kind, ... } end
    function N.put_lead(ms)
        local k, e = kind_ok("pose_lead"); if not k then return nil, e end
        st.local_.pose_lead = ms; rec("pose_lead", ms); return true
    end
    function N.peers(out)
        count("peers")
        local n = 0
        local slots = {}
        for s in pairs(st.dir) do slots[#slots + 1] = s end
        table.sort(slots)
        for _, s in ipairs(slots) do
            n = n + 1
            local e = out[n] or {}
            out[n] = e
            for k2 in pairs(e) do e[k2] = nil end
            for k2, v in pairs(st.dir[s]) do e[k2] = v end
        end
        for i = n + 1, #out do out[i] = nil end
        return n
    end
    function N.peer_play(slot, out, last)
        count("peer_play")
        local k = kind_ok("peer_play"); if not k then return nil end
        local p = st.peer.peer_play and st.peer.peer_play[slot]
        if not p or p.seq == last then return nil end
        local B, W, C = out.B or {}, out.W or {}, out.C
        for k2 in pairs(out) do out[k2] = nil end
        for k2, v in pairs(p.tbl) do if k2 ~= "B" and k2 ~= "W" and k2 ~= "C" then out[k2] = v end end
        for i = 1, #p.tbl.B do B[i] = p.tbl.B[i] end
        for i = #p.tbl.B + 1, #B do B[i] = nil end
        local pw = p.tbl.W or {}
        for i = 1, #pw do W[i] = pw[i] end
        for i = #pw + 1, #W do W[i] = nil end
        out.B, out.W = B, W
        if p.tbl.C then
            C = C or {}
            for i = 1, #p.tbl.C do C[i] = p.tbl.C[i] end
            for i = #p.tbl.C + 1, #C do C[i] = nil end
            out.C = C
        end
        st.B_ref = B
        return p.seq
    end
    function N.subscribe(kinds)
        local c = st.cursors[cursor_key()]
        if not c then c = { next = #st.log + st.log_base }; st.cursors[cursor_key()] = c end
        c.filter = {}
        for _, k in ipairs(kinds or {}) do c.filter[k] = true end
        return true
    end
    function N.poll(max, out)
        count("poll")
        local key = cursor_key()
        local c = st.cursors[key]
        local top = st.log_base + #st.log   -- next absolute index
        if not c then c = { next = top }; st.cursors[key] = c end
        local n = 0
        if c.next < st.log_base then
            c.next = st.log_base
            n = n + 1
            out[n] = { kind = "resync" }
        end
        while c.next < top and n < max do
            local r = st.log[c.next - st.log_base + 1]
            c.next = c.next + 1
            if not c.filter or c.filter[r.kind] then
                n = n + 1
                local e = out[n] or {}
                out[n] = e
                e.kind, e.req_id, e.data = r.kind, r.req_id, deep(r.data)
                e.peer, e.aux = r.peer, r.aux   -- typed records (sc_rec_event)
            end
        end
        for i = n + 1, #out do out[i] = nil end
        return n
    end
    -- ---- sidecar-side helpers (tests) ----------------------------------------
    function N.sc_peer_dir(entries)   -- { {id=, slot=, nick=}, ... }
        st.dir = {}
        for _, e in ipairs(entries) do
            st.dir[e.slot] = { id = e.id, slot = e.slot, gen = e.gen or 1, nick = e.nick or ("P" .. e.id),
                               play_seq = 0, root_seq = 0, vitals_seq = 0, kit_seq = 0, rtt_ms = e.rtt_ms or 0 }
        end
        st.flags = st.flags | S.FLAGS.PEERS_CHANGED
    end
    function N.sc_peer(kind, slot, tbl)
        st.peer[kind] = st.peer[kind] or {}
        local p = st.peer[kind][slot] or { seq = 0 }
        p.seq, p.tbl = p.seq + 2, deep(tbl)
        st.peer[kind][slot] = p
    end
    function N.sc_event(kind, data, req_id)
        st.log[#st.log + 1] = { kind = kind, data = deep(data), req_id = req_id or 0 }
        while #st.log > st.log_max do table.remove(st.log, 1); st.log_base = st.log_base + 1 end
        st.flags = st.flags | S.FLAGS.EVENTS
    end
    function N.sc_drain()   -- the sidecar consumes the G2S ring (typed sends: N.sc_rec_drain)
        return N.sc_rec_drain and N.sc_rec_drain() or {}
    end
    function N.sc_caps(game, sidecar)
        st.caps_game, st.caps_sidecar = game or st.caps_game, sidecar or st.caps_sidecar
        st.flags = st.flags | S.FLAGS.SIDECAR_RESET
    end
    -- Typed records (ABI 2): put/get/peer/send/poll/bus_* for record names.
    M.records().install(N, S)
    -- The pose hot paths build the root / weapon records into their slots (hsmp_native_pose.lua),
    -- and the suites keep the arguments (st.local_, st.puts).
    M.pose().install(N, S, function(cap) return caps_eff() & cap ~= 0 end)
    local pr, pw, pp = N.put_root, N.put_weapon, N.put_pose
    function N.put_root(...)
        local ok, e = pr(...)
        if ok then st.local_.local_root = { ... }; rec("local_root", ...) end
        return ok, e
    end
    function N.put_weapon(...)
        local ok, e = pw(...)
        if ok then st.local_.local_weapon = { ... }; rec("local_weapon", ...) end
        return ok, e
    end
    function N.put_pose(tick, ...)
        local ok, e = pp(tick, ...)
        if ok then st.local_.local_pose = N._hot.pose; rec("local_pose", tick) end
        return ok, e
    end
    return N
end

return M
