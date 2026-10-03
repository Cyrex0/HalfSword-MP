-- hsmp_native_filemock.lua -- the default HSMPNative of the offline Lua suites.
--
-- Every game<->sidecar and Lua<->Lua contract is a typed record, so this mock
-- is the in-memory typed-record mixin (lib/hsmp_native_records.lua: put / get / peer / send /
-- poll / bus_* by record name, N.sc_put / N.sc_rec_event for "the sidecar") and the pose hot
-- paths (lib/hsmp_native_pose.lua), on top of the base API: open / info / frame, the peer
-- directory (N.sc_peer_dir + every peer with a record), the evaluated peer play
-- (N.sc_peer_play) and the event log (typed records, in order). No file is involved (the
-- name is historical: the suites install it as their default HSMPNative).
-- It is TEST code only (installed by luatest_prelude.lua in every Lua state).
-- For in-memory checks of the facade itself use hsmp_native_mock.lua.

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

local CAPS_ALL = 0
for _, v in pairs(S.CAPS) do CAPS_ALL = CAPS_ALL | v end
M.CAPS_ALL = CAPS_ALL

-- o: caps, game_caps, sidecar_state, hb_age, dir (function -> state dir)
function M.new(o)
    o = o or {}
    local N = { _filemock = true }
    local st = {
        caps_game = o.game_caps or o.caps or CAPS_ALL,
        caps_sidecar = o.caps or CAPS_ALL,
        sidecar_state = o.sidecar_state or "ready",
        -- No header heartbeat: hsmp_session judges freshness from the link record the way the
        -- suites expect.
        hb_age = o.hb_age or 1e9,
        play = {},                  -- peer slot (= peer id here) -> { seq, t } (N.sc_peer_play)
        filter = nil,
        opened = nil, world_epoch = 1, last_flags = 0,
        log = {}, tnext = 1,        -- typed S2G records (N.sc_rec_event), in order
        calls = {},
    }
    N._st = st
    local function count(n) st.calls[n] = (st.calls[n] or 0) + 1 end
    local function dir()
        if o.dir then return o.dir() end
        local I = rawget(_G, "HSMP_IPC")
        local d = (I and I.state_dir) or os.getenv("HSMP_STATE_DIR") or "hsmp_state"
        return (tostring(d):gsub("\\", "/"))
    end
    N._dir = dir
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

    function N.abi() return S.ABI_MAJOR, S.ABI_MINOR, S.LAYOUT_HASH end
    function N.now_us() return math.floor(os.clock() * 1e6) end
    function N.thread_ok() return true end
    function N.ipc_open()
        count("ipc_open")
        st.opened = st.opened or "Local\\HSMP.ipc.1.4242.1d9a2b3c4d5e6f7"
        return st.opened
    end
    function N.ipc_info()
        return { name = st.opened, state = "ready", abi_major = S.ABI_MAJOR, abi_minor = S.ABI_MINOR,
                 layout_hash = S.LAYOUT_HASH, caps_game = st.caps_game, caps_sidecar = st.caps_sidecar,
                 caps_effective = caps_eff(), sidecar_state = st.sidecar_state, sidecar_hb_age_s = st.hb_age,
                 world_epoch = st.world_epoch, session_epoch = 1, attach_count = 1, refuse_code = 0,
                 refuse_detail = "", counters = {} }
    end
    function N.frame(world_key)
        count("frame")
        local f = 0
        if world_key ~= st.world_key then
            if st.world_key ~= nil then f = f | S.FLAGS.WORLD_CHANGED end
            st.world_key = world_key
        end
        st.last_flags = f
        return f
    end
    function N.flags() return st.last_flags or 0 end
    function N.world_leaving() st.world_epoch = st.world_epoch + 1; return true end
    function N.world_ready() return true end

    -- ---- game -> sidecar ----------------------------------------------------------------
    function N.put_lead(ms)
        local k, e = kind_ok("pose_lead"); if not k then return nil, e end
        st.pose_lead = tonumber(ms) or 0
        return true
    end

    -- The peer directory: the entries a suite set with N.sc_peer_dir (the sidecar allocates
    -- one for every connected roster peer), plus every peer with a play line or a per-peer
    -- record. Our own id is the `link` record's my_peer_id (N.sc_put("link", ...)).
    local function peer_ids()
        local ids, seen = {}, {}
        local lk = N._rec and N._rec.slots.link and N._rec.slots.link.t
        local me = lk and tonumber(lk.my_peer_id) or 0
        local nick, rtt = {}, {}
        local function add(id)
            id = math.tointeger(tonumber(id))
            if id and id ~= me and not seen[id] then seen[id] = true; ids[#ids + 1] = id end
        end
        for _, p in ipairs(st.peer_list or {}) do
            add(p.id)
            local id = math.tointeger(tonumber(p.id))
            if id then nick[id], rtt[id] = p.nick, p.rtt_ms end
        end
        for id in pairs(st.play) do add(id) end
        -- typed per-peer record slots (sc_put(name, t, peer_slot)): slot = id here
        local R = N._rec
        if R and R.peers then
            for _, slots in pairs(R.peers) do
                for id in pairs(slots) do add(id) end
            end
        end
        table.sort(ids)
        return ids, nick, rtt
    end
    -- entries: { {id=, nick=, rtt_ms=}, ... } (the sidecar's PeerDir)
    function N.sc_peer_dir(entries)
        st.peer_list = {}
        for _, e in ipairs(entries or {}) do st.peer_list[#st.peer_list + 1] = { id = e.id, nick = e.nick, rtt_ms = e.rtt_ms } end
    end
    function N.peers(out)
        count("peers")
        local ids, nick, rtt = peer_ids()
        for i, id in ipairs(ids) do
            local e = out[i] or {}
            out[i] = e
            for k2 in pairs(e) do e[k2] = nil end
            e.id, e.slot, e.gen, e.nick = id, id, 1, nick[id] or ("P" .. id)
            e.play_seq, e.root_seq, e.vitals_seq, e.kit_seq, e.rtt_ms = 0, 0, 0, 0, rtt[id] or 0
        end
        for i = #ids + 1, #out do out[i] = nil end
        return #ids
    end
    local function fill(arr, src)
        arr = arr or {}
        local n = 0
        if type(src) == "table" then for i = 1, #src do arr[i] = src[i]; n = i end end
        for i = n + 1, #arr do arr[i] = nil end
        return arr
    end
    -- The sidecar's evaluated play line for peer `id` (the slot is the id here), in the
    -- playback thread's field names: { peer_id, seq, v = 2, pt, mode, cut,
    -- root = {x, y, z, yaw}, m, vm, B, W, C, ... }. The seq comes from t.seq (else one is minted).
    function N.sc_peer_play(id, t)
        local p = st.play[id] or { seq = 0 }
        p.seq = math.tointeger(tonumber(t.seq)) or (p.seq + 1)
        p.t = t
        st.play[id] = p
    end
    function N.peer_play(slot, out, last)
        count("peer_play")
        local k = kind_ok("peer_play"); if not k then return nil end
        local p = st.play[slot]
        if not p then return nil end
        local t, seq = p.t, p.seq
        if seq == last then return nil end
        local B, W, C = out.B, out.W, out.C
        for k2 in pairs(out) do out[k2] = nil end
        out.peer_id, out.seq, out.mode, out.cut = tonumber(t.peer_id) or slot, seq, t.mode or "interp", tonumber(t.cut) or 0
        out.pt = tonumber(t.ptf or t.pt) or 0
        out.age, out.delay, out.jit = tonumber(t.age) or 0, tonumber(t.delay) or 0, tonumber(t.jit) or 0
        out.lead, out.st, out.iv, out.k = tonumber(t.lead) or 0, tonumber(t.st) or 0, tonumber(t.iv) or 0, tonumber(t.k) or 0
        out.v2 = (tonumber(t.v) == 2)
        local r = t.root
        out.root = (type(r) == "table" and #r >= 4) and { x = r[1], y = r[2], z = r[3], yaw = r[4] } or false
        out.m, out.vm = tonumber(t.m) or 0, tonumber(t.vm) or 0
        out.B = fill(B, t.B)
        out.W = fill(W, t.W)
        out.C = (type(t.C) == "table") and fill(C, t.C) or false
        return seq
    end

    -- Events: this Lua state's cursor over the typed record log. A newly subscribed state
    -- starts at the log head, like the native cursor: earlier events are not replayed.
    function N.subscribe(kinds)
        if st.filter == nil then st.tnext = #st.log + 1 end
        st.filter = {}
        for _, k in ipairs(kinds or {}) do st.filter[k] = true end
        return true
    end
    function N.poll(max, out)
        count("poll")
        local n = 0
        while n < max and st.tnext <= #st.log do
            local r = st.log[st.tnext]
            st.tnext = st.tnext + 1
            if not st.filter or st.filter[r.kind] then
                n = n + 1
                local e = out[n] or {}
                out[n] = e
                e.kind, e.req_id, e.data, e.peer, e.aux = r.kind, r.req_id or 0, r.data, r.peer or 0, r.aux or 0
            end
        end
        for i = n + 1, #out do out[i] = nil end
        return n
    end
    -- An S2G record pushed by "the sidecar" (typed: use N.sc_rec_event, which marshals it).
    function N.sc_event(kind, data, req_id)
        st.log[#st.log + 1] = { kind = kind, data = data, req_id = req_id or 0 }
    end

    -- Typed records (ABI 2): put/get/peer/send/poll/bus_* for record names.
    M.records().install(N, S)
    -- The pose hot paths build the root / weapon records into their slots; put_pose keeps its
    -- arguments in N._hot.pose (hsmp_native_pose.lua).
    M.pose().install(N, S, function(cap) return caps_eff() & cap ~= 0 end)
    return N
end

return M
