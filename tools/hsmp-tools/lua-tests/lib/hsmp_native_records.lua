-- hsmp_native_records.lua -- the typed-record half of the HSMPNative API
-- (docs/development/ipc-shared-memory.md), in Lua, for the offline
-- suites. It is the executable spec of how a Lua table becomes a record and back:
-- crates/hsmp-native (src/marshal.rs, src/records.rs) implements the same rules on the real
-- segment, and lib/records_conformance.lua runs the same checks against both
-- (`hsmp-tools lua-test records` here, crates/hsmp-native/tests/records.rs on the native).
--
--   local R = require("hsmp_native_records")
--   R.install(N, S)          -- adds put/get/peer/send/poll/bus_put/bus_get typed paths,
--                            -- dev_poll and the process API to a mock N
--   local t, err = R.marshal(S, "root", tbl)   -- canonical table or nil, "bad:<field>"
--   local ok, err = R.check(S, "root", t)      -- the record's own check (R.CHECKS)
--
-- Table shape (both directions):
--   * a struct is a table keyed by its Rust field names; fields named "_..." are padding
--     and never appear;
--   * integers are Lua integers (a float is truncated toward zero and saturated, NaN -> 0,
--     i.e. Rust `as`; an integer wraps to the field width; true = 1, false = 0); u64 above
--     2^63-1 reads back negative (the same 64 bits);
--   * floats are numbers and MUST be finite, also after rounding to f32 (NaN / inf / a
--     value that overflows f32 -> nil, "bad:<field>"); true = 1.0;
--   * an array field is a Lua array (1-based); extra elements are ignored, missing ones default;
--   * a Str<N> field is a Lua string (a number is converted with tostring), truncated on a
--     UTF-8 boundary to N bytes; the result must be valid UTF-8 without NUL bytes, else
--     nil, "bad:<field>";
--   * a Bool field is a boolean (nil = false, a number: nonzero = true, anything else: truthiness);
--   * a missing field is 0 / "" / false / a zeroed struct;
--   * a variable record carries its rows as `t.rows` (array of row tables, or numbers for a
--     primitive row type); its count field is set from the raw length of t.rows; more than
--     max_rows -> nil, "too_big"; t.rows not a table (nil / false = no rows) -> "bad:rows";
--   * an error names the innermost field (an array element: the array's field; a row that is
--     not a table: the row type's name);
--   * after marshalling, the record's own check runs (Rust: schema::check_payload; here:
--     R.CHECKS[name], mirrored by hand) -> nil, "bad:<field>" / "bad:<reason>".
-- Reading fills `out` in place when given (keys that are not fields are removed, nested
-- tables reused, trailing array / row entries set to nil); without `out` a fresh table.

local R = {}

local INT = { u8 = 8, i8 = 8, u16 = 16, i16 = 16, u32 = 32, i32 = 32, u64 = 64, i64 = 64 }
local SIGNED = { i8 = true, i16 = true, i32 = true, i64 = true }
-- |v| >= this rounds to +-inf as an f32 (FLT_MAX + half an ulp).
local F32_INF_AT = 2 ^ 128 - 2 ^ 103

local function int_of(ty, v)
    if type(v) == "boolean" then v = v and 1 or 0 end
    if type(v) ~= "number" then return nil end
    local bits = INT[ty]
    if math.type(v) == "integer" then
        local i = v
        if bits < 64 then
            i = i & ((1 << bits) - 1)
            if SIGNED[ty] and i >= (1 << (bits - 1)) then i = i - (1 << bits) end
        end
        return i
    end
    -- float: Rust `as` (NaN -> 0, truncate toward zero, saturate)
    if v ~= v then return 0 end
    local t = v >= 0 and math.floor(v) or math.ceil(v)
    if SIGNED[ty] then
        local lim = 2.0 ^ (bits - 1)
        if t <= -lim then return bits == 64 and math.mininteger or -(1 << (bits - 1)) end
        if t >= lim then return bits == 64 and math.maxinteger or (1 << (bits - 1)) - 1 end
        return math.tointeger(t) or 0
    end
    if t <= 0 then return 0 end
    local lim = 2.0 ^ bits
    if t >= lim then return bits == 64 and -1 or (1 << bits) - 1 end
    if bits == 64 and t >= 2.0 ^ 63 then return math.mininteger + math.tointeger(t - 2.0 ^ 63) end
    return math.tointeger(t) or 0
end
R.int_of = int_of

local function trunc_utf8(s, cap)
    if #s <= cap then return s end
    local n = cap
    -- back off a continuation byte (10xxxxxx) so the cut is on a char boundary
    while n > 0 do
        local b = s:byte(n + 1)
        if not b or b < 0x80 or b >= 0xC0 then break end
        n = n - 1
    end
    return s:sub(1, n)
end

-- Default (zero) value of a type.
local function zero(S, ty)
    if type(ty) == "table" then
        if ty[1] == "str" then return "" end
        local a = {}
        for i = 1, ty[2] do a[i] = zero(S, ty[1]) end
        return a
    end
    if ty == "bool" then return false end
    if ty == "f32" or ty == "f64" then return 0.0 end
    if INT[ty] then return 0 end
    local st = S.STRUCTS[ty]
    local t = {}
    if st then
        for _, f in ipairs(st.fields) do
            if f[1]:sub(1, 1) ~= "_" then t[f[1]] = zero(S, f[2]) end
        end
    end
    return t
end
R.zero = zero

-- Canonicalise `v` as type `ty`. Returns value or nil, err.
local function canon(S, ty, v, field)
    if type(ty) == "table" then
        if ty[1] == "str" then
            if v == nil then return "" end
            if type(v) == "number" then v = tostring(v) end
            if type(v) ~= "string" then return nil, "bad:" .. field end
            local s = trunc_utf8(v, ty[2])
            if s:find("\0", 1, true) or not utf8.len(s) then return nil, "bad:" .. field end
            return s
        end
        local a = {}
        local src = (type(v) == "table") and v or {}
        if v ~= nil and type(v) ~= "table" then return nil, "bad:" .. field end
        for i = 1, ty[2] do
            local x, e = canon(S, ty[1], rawget(src, i), field)
            if e then return nil, e end
            a[i] = x
        end
        return a
    end
    if ty == "bool" then
        if v == nil then return false end
        if type(v) == "number" then return v ~= 0 end
        return v and true or false
    end
    if ty == "f32" or ty == "f64" then
        if v == nil then return 0.0 end
        if type(v) == "boolean" then v = v and 1 or 0 end
        if type(v) ~= "number" or v ~= v or v == math.huge or v == -math.huge then return nil, "bad:" .. field end
        if ty == "f32" then
            if math.abs(v) >= F32_INF_AT then return nil, "bad:" .. field end
            v = string.unpack("<f", string.pack("<f", v))
        end
        return v + 0.0
    end
    if INT[ty] then
        if v == nil then return 0 end
        local i = int_of(ty, v)
        if i == nil then return nil, "bad:" .. field end
        return i
    end
    local st = S.STRUCTS[ty]
    if not st then return nil, "bad:" .. field end
    if v ~= nil and type(v) ~= "table" then return nil, "bad:" .. field end
    local src = v or {}
    local t = {}
    for _, f in ipairs(st.fields) do
        local name = f[1]
        if name:sub(1, 1) ~= "_" then
            local x, e = canon(S, f[2], rawget(src, name), name)
            if e then return nil, e end
            t[name] = x
        end
    end
    return t
end

-- A record table (head + rows) for record name `rec`. Returns canonical table or nil, err.
function R.marshal(S, rec, tbl)
    local r = S.RECORDS[rec]
    if not r then return nil, "bad" end
    if type(tbl) ~= "table" then return nil, "bad" end
    local t, e = canon(S, r.layout, tbl, r.layout)
    if not t then return nil, e end
    if r.row then
        local rows = rawget(tbl, "rows") or {}
        if type(rows) ~= "table" then return nil, "bad:rows" end
        local n = rawlen(rows)
        if n > r.max_rows then return nil, "too_big" end
        local out = {}
        for i = 1, n do
            local x, e2 = canon(S, r.row, rawget(rows, i), r.row)
            if x == nil then return nil, e2 end
            out[i] = x
        end
        t.rows = out
        if r.count then t[r.count] = n end
    end
    return t
end

-- The records' own checks (Rust check / check_row fns), mirrored by hand: name ->
-- function(t) returning nil (ok) or the failing field / reason. Domains add theirs.
R.CHECKS = R.CHECKS or {}
local function quat_ok(q)
    local n = q[1] * q[1] + q[2] * q[2] + q[3] * q[3] + q[4] * q[4]
    return n >= 0.25 and n <= 2.25
end
local function pos_ok(p)
    for i = 1, 3 do if math.abs(p[i]) > 1.0e7 then return false end end
    return true
end
R.CHECKS.root = function(t)
    if not pos_ok(t.pos) then return "pos" end
    if not quat_ok(t.rot) then return "rot" end
end
R.CHECKS.weapon = function(t)
    if t.held > 2 then return "held" end
    if not pos_ok(t.pos) then return "pos" end
    if not quat_ok(t.rot) then return "rot" end
end
R.CHECKS.pose = function(t)
    if t.n < 24 then return "n" end
end
R.CHECKS.peer_root = function(t)
    if t.peer_id == 0 then return "peer_id" end
    return R.CHECKS.root(t.root)
end
R.CHECKS.dev_cmd = function(t)
    if t.op < 1 or t.op > 3 then return "op" end
end
R.CHECKS.body = function(t)   -- schema/loadout.rs check_body / check_body_bone
    local function rate(x) return x >= 0 and x <= 4 end
    local function scale(x) return x > 0 and x <= 16 end
    if not rate(t.height_rate) or not rate(t.muscle_rate) then return "rate" end
    if not scale(t.mass_scale_bp) then return "scale" end
    for i = 1, 3 do if not scale(t.char_scale[i]) then return "scale" end end
    for _, r in ipairs(t.rows or {}) do
        if r.bone == "" then return "bone" end
        if not (r.mass > 0 and r.mass <= 500) or not (r.mass_scale > 0 and r.mass_scale <= 16) then return "mass" end
    end
end

function R.check(_, rec, t)
    local f = R.CHECKS[rec]
    local bad = f and f(t)
    if bad then return nil, "bad:" .. bad end
    return true
end

-- marshal + check (what every game write goes through).
local function marshal_checked(S, rec, tbl)
    local t, e = R.marshal(S, rec, tbl)
    if not t then return nil, e end
    local ok, e2 = R.check(S, rec, t)
    if not ok then return nil, e2 end
    return t
end
R.marshal_checked = marshal_checked

local function deep(v)
    if type(v) ~= "table" then return v end
    local o = {}
    for k, x in pairs(v) do o[k] = deep(x) end
    return o
end
R.deep = deep

-- Copy `t` into the reused nested table `o`: fields / elements are overwritten (nested tables
-- reused), an array's entries past its length are set to nil; other keys are left alone.
local function fill_nested(o, t)
    for k, v in pairs(t) do
        if type(v) == "table" then
            local s = rawget(o, k)
            if type(s) ~= "table" then s = {}; o[k] = s end
            fill_nested(s, v)
        else
            o[k] = v
        end
    end
    local n = rawlen(t)
    if n > 0 or next(t) == nil then
        for i = rawlen(o), n + 1, -1 do o[i] = nil end
    end
end

-- Fill `out` (reused) with `t`; returns out. Keys of `out` that are not fields are removed.
local function fill(out, t)
    for k in pairs(out) do if t[k] == nil then out[k] = nil end end
    fill_nested(out, t)
    return out
end

local MAX_PEER_SLOTS = 32

-- An integer argument as the native module reads it (a float with an integer value converts).
local function int_arg(v) return type(v) == "number" and math.tointeger(v) or nil end

-- Command line checks the native module applies (proc.rs command_line).
local function proc_args_ok(exe, args, opts)
    if type(exe) ~= "string" or exe == "" or exe:find('["%z\r\n]') then return nil end
    local a = {}
    if args ~= nil then
        if type(args) ~= "table" then return nil end
        for i = 1, rawlen(args) do
            local v = rawget(args, i)
            if type(v) == "number" then v = tostring(v) end
            if type(v) ~= "string" or v:find("%z") then return nil end
            a[i] = v
        end
    end
    local lx = exe:lower()
    if lx:sub(-4) == ".cmd" or lx:sub(-4) == ".bat" then
        for _, v in ipairs(a) do if v:find('["%%!\r\n]') then return nil end end
    end
    if opts ~= nil then
        if type(opts) ~= "table" then return nil end
        if opts.cwd ~= nil and type(opts.cwd) ~= "string" then return nil end
        if opts.hide ~= nil and type(opts.hide) ~= "boolean" then return nil end
        if opts.env ~= nil then
            if type(opts.env) ~= "table" then return nil end
            for k, v in pairs(opts.env) do
                if type(k) ~= "string" or k == "" or k:find("[=%z]") then return nil end
                if not (v == false or type(v) == "string" or type(v) == "number") then return nil end
                if type(v) == "string" and v:find("%z") then return nil end
            end
        end
    end
    return a
end

-- Install the typed paths on mock N: put / get / peer / send / bus_* for record names (any
-- other name is "bad", as in the native module), plus poll / world_leaving on top of the base
-- mock's.
function R.install(N, S)
    local st = {
        slots = {},     -- slot name -> { ver, t }        (single + game-written + sidecar-written)
        peers = {},     -- slot name -> peer slot -> { ver, t }
        bus = {},       -- key -> { ver, t }
        sends = {},     -- every typed send { kind, req_id, data }
        g2s = {},       -- undrained typed sends
        puts = {},      -- every typed put { slot, t }
        req = 0,
        refused = {},   -- "op:name:err" -> n
        dev = {}, dev_base = 1, dev_max = 256, dev_cursors = {},   -- DevCtl fan-out log
    }
    N._rec = st
    local base = { poll = N.poll, world_leaving = N.world_leaving }

    local function refuse(op, name, err)
        local k = op .. ":" .. tostring(name) .. ":" .. tostring(err)
        st.refused[k] = (st.refused[k] or 0) + 1
        return nil, err
    end

    local function slot_rec(name)
        if S.KINDS and S.KINDS[name] then return nil end
        local s = S.SLOTS and S.SLOTS[name]
        if not s then return nil end
        return s, s.record
    end

    local function caps_eff()
        local b = N._st
        if not b then return 0 end
        if b.sidecar_state ~= "ready" then return 0 end
        return (b.caps_game or 0) & (b.caps_sidecar or 0)
    end

    function N.put(name, tbl)
        local s, rec = slot_rec(name)
        if not s then return refuse("put", name, "bad") end
        if (s.dir ~= "g2s" and s.dir ~= "local") or (s.form ~= "slot" and s.form ~= "blob") then return refuse("put", name, "bad") end
        if s.dir == "g2s" then
            local eff = caps_eff()
            if s.cap ~= 0 and eff ~= 0 and eff & s.cap == 0 then return refuse("put", name, "cap") end
        end
        local t, e = marshal_checked(S, rec, tbl)
        if not t then return refuse("put", name, e) end
        local c = st.slots[name] or { ver = 0 }
        c.ver, c.t = c.ver + 2, t
        st.slots[name] = c
        st.puts[#st.puts + 1] = { slot = name, t = deep(t) }
        return true
    end

    -- get(slot, last, out) -> ver, t | ver (unchanged) | nil (no value)
    function N.get(name, last, out)
        local s = slot_rec(name)
        if not s or (s.form ~= "slot" and s.form ~= "blob") then return nil, "bad" end
        if last ~= nil then last = int_arg(last); if last == nil then return nil, "bad" end end
        local c = st.slots[name]
        if not c or not c.t then return nil end
        if c.ver == last then return c.ver end
        return c.ver, type(out) == "table" and fill(out, c.t) or deep(c.t)
    end

    function N.peer(name, slot, last, out)
        local s = slot_rec(name)
        if not s then return nil, "bad" end
        if s.form ~= "peer_slot" and s.form ~= "peer_blob" then return nil, "bad" end
        slot = int_arg(slot)
        if slot == nil or slot < 0 or slot >= MAX_PEER_SLOTS then return nil, "bad" end
        if last ~= nil then last = int_arg(last); if last == nil then return nil, "bad" end end
        local p = st.peers[name] and st.peers[name][slot]
        if not p or p.ver == last then return nil end
        return p.ver, type(out) == "table" and fill(out, p.t) or deep(p.t)
    end

    function N.send(kind, tbl)
        local r = (not (S.KINDS and S.KINDS[kind])) and S.RECORDS and S.RECORDS[kind]
        if not r then return refuse("send", kind, "bad") end
        if not r.flow:find("g2s", 1, true) then return refuse("send", kind, "bad") end
        if r.cap ~= 0 and caps_eff() & r.cap == 0 then return refuse("send", kind, "cap") end
        local t, e = marshal_checked(S, kind, tbl)
        if not t then return refuse("send", kind, e) end
        if N._st and N._st.ring and #st.g2s >= N._st.ring then return refuse("send", kind, "full") end
        st.req = st.req + 1
        local m = { kind = kind, req_id = st.req, data = t }
        st.sends[#st.sends + 1] = m
        st.g2s[#st.g2s + 1] = m
        return st.req
    end

    function N.bus_put(key, tbl)
        local s, rec = slot_rec(key)
        if not s or s.form ~= "bus" then return refuse("bus_put", key, "bad") end
        local t, e = marshal_checked(S, rec, tbl)
        if not t then return refuse("bus_put", key, e) end
        local b = st.bus[key] or { ver = 0 }
        b.ver, b.t = b.ver + 2, t
        st.bus[key] = b
        return true
    end

    function N.bus_get(key, last, out)
        local s = slot_rec(key)
        if not s or s.form ~= "bus" then return nil, "bad" end
        if last ~= nil then last = int_arg(last); if last == nil then return nil, "bad" end end
        local b = st.bus[key]
        if not b or not b.t then return 0 end
        if b.ver == last then return b.ver end
        return b.ver, type(out) == "table" and fill(out, b.t) or deep(b.t)
    end

    -- world_leaving: world-scoped game-written record slots and typed bus keys lose their value.
    function N.world_leaving(...)
        for name, s in pairs(S.SLOTS or {}) do
            if s.world_scoped and s.dir ~= "s2g" then
                if s.form == "bus" then
                    local b = st.bus[name]
                    if b and b.t then b.ver, b.t = b.ver + 2, nil end
                elseif s.form == "slot" or s.form == "blob" then
                    local c = st.slots[name]
                    if c and c.t then c.ver, c.t = c.ver + 2, nil end
                end
            end
        end
        if base.world_leaving then return base.world_leaving(...) end
        return true
    end

    -- ---- DevCtl (E.7) ----------------------------------------------------------------------
    local function dev_key() return (N._cursor_key and N._cursor_key()) or "default" end
    st.dev_cursors[dev_key()] = st.dev_base   -- the state's cursor starts at "now"
    -- A new cursor key = a new Lua state getting the API: its DevCtl cursor starts at "now".
    local base_set_cursor = N._set_cursor
    if base_set_cursor then
        function N._set_cursor(key)
            base_set_cursor(key)
            if st.dev_cursors[key] == nil then st.dev_cursors[key] = st.dev_base + #st.dev end
        end
    end

    -- dev_poll(max, out) -> n; out[i] = {kind = "dev_cmd", data = t} (entries reused, data fresh)
    function N.dev_poll(max, out)
        if type(out) ~= "table" then return nil, "bad" end
        max = max == nil and 16 or int_arg(max)
        if max == nil then return nil, "bad" end
        local key = dev_key()
        local top = st.dev_base + #st.dev
        local cur = st.dev_cursors[key] or top
        if cur < st.dev_base then cur = st.dev_base end
        local n = 0
        while n < max and cur < top do
            local t = st.dev[cur - st.dev_base + 1]
            cur = cur + 1
            n = n + 1
            local e = type(out[n]) == "table" and out[n] or {}
            out[n] = e
            e.kind, e.data = "dev_cmd", deep(t)
        end
        st.dev_cursors[key] = cur
        for i = n + 1, #out do out[i] = nil end
        return n
    end

    -- A dev_cmd from `hsmp-tools ipc-ctl` (marshalled by the schema; an invalid one is dropped).
    function N.sc_dev(tbl)
        local t, e = R.marshal(S, "dev_cmd", tbl)
        assert(t, "sc_dev: " .. tostring(e))
        if not R.check(S, "dev_cmd", t) then return nil end
        st.dev[#st.dev + 1] = t
        while #st.dev > st.dev_max do table.remove(st.dev, 1); st.dev_base = st.dev_base + 1 end
        return t
    end

    -- ---- processes (E.7) ---------------------------------------------------------------------
    -- N._proc: spawned = {{exe, args, opts, pid, capture}}, alive = {[pid] = bool},
    -- killed = {pid...}, captures = {[h] = {exe, args, done, code, out}}, exit = {[pid] = code}.
    -- auto = true simulates `cmd.exe /c exit N` and `cmd.exe /c echo ...` (conformance).
    -- Test helpers: fail = "<err>" (the next spawn fails), responder = function(exe, args)
    -- -> code, out | nil (completes a capture at its first poll), game_pid (current_pid).
    local P = { spawned = {}, alive = {}, killed = {}, captures = {}, exit = {}, next_pid = 5000, auto = false }
    N._proc = P

    local function simulate(pid, args)
        if not P.auto then return end
        if args[1] == "/c" and args[2] == "exit" then
            N.sc_proc_exit(pid, math.tointeger(tonumber(args[3])) or 0)
        elseif args[1] == "/c" and args[2] == "echo" then
            local c = P.captures[pid]
            if c then c.out = c.out .. table.concat(args, " ", 3) .. "\r\n" end
            N.sc_proc_exit(pid, 0)
        end
    end

    local function spawn(exe, args, opts, capture)
        local a = proc_args_ok(exe, args, opts)
        if not a then return nil, "bad" end
        if P.fail then   -- test helper: the next spawn fails with this error
            local e = P.fail
            P.fail = nil
            return nil, e
        end
        P.next_pid = P.next_pid + 4
        local pid = P.next_pid
        P.spawned[#P.spawned + 1] = { exe = exe, args = a, opts = opts, pid = pid, capture = capture }
        P.alive[pid] = true
        if capture then P.captures[pid] = { exe = exe, args = a, done = false, out = "" } end
        simulate(pid, a)
        return pid
    end
    function N.spawn(exe, args, opts) return spawn(exe, args, opts, false) end
    function N.spawn_capture(exe, args, opts) return spawn(exe, args, opts, true) end
    function N.proc_alive(pid) return P.alive[pid] == true end
    function N.proc_exit_code(pid)
        if P.alive[pid] == nil then return nil, "not_ours" end
        if P.alive[pid] then return nil, "running" end
        return P.exit[pid] or 0
    end
    function N.proc_kill(pid)
        if P.alive[pid] == nil then return nil, "not_ours" end
        if not P.alive[pid] then return nil, "gone" end
        P.killed[#P.killed + 1] = pid
        N.sc_proc_exit(pid, 1)
        return true
    end
    function N.capture_poll(h)
        local c = P.captures[h]
        if not c then return nil, "bad" end
        if not c.done and P.responder then   -- test helper: function(exe, args) -> code, out | nil
            local code, out = P.responder(c.exe, c.args)
            if code ~= nil then N.sc_capture_done(h, code, out) end
        end
        if not c.done then return false end
        P.captures[h] = nil
        return true, c.code, c.out
    end
    function N.current_pid() return P.game_pid or 4242 end
    -- The process exits (code default 0).
    function N.sc_proc_exit(pid, code)
        if P.alive[pid] == nil then return end
        P.alive[pid] = false
        P.exit[pid] = code or 0
        local c = P.captures[pid]
        if c then c.done, c.code = true, code or 0 end
    end
    -- A captured process finishes with `code` and output `out` (appended).
    function N.sc_capture_done(h, code, out)
        local c = P.captures[h]
        if not c then return end
        c.out = c.out .. (out or "")
        N.sc_proc_exit(h, code)
    end

    -- ---- native sampling (sample.rs). The mock has no engine: sample_local is
    -- "unavailable" unless a test installs N._sample = function(a) -> mask[, err].
    function N.sample_config(cfg)
        if type(cfg) ~= "table" then return nil, "bad" end
        st.sample_cfg = cfg
        return true
    end
    function N.sample_local(a)
        if type(a) ~= "table" then return nil, "bad" end
        if not N._sample then return nil, "unavailable" end
        if not st.sample_cfg then return nil, "not configured" end
        st.sample_calls = (st.sample_calls or 0) + 1
        return N._sample(a)
    end
    function N.sample_status()
        return { available = N._sample ~= nil, configured = st.sample_cfg ~= nil, verified = N._sample ~= nil,
                 world_ok = true, samples = st.sample_calls or 0, fallbacks = 0, pe_calls = 0, avg_us = 0, last_us = 0 }
    end
    -- Native servo (servo.rs): "unavailable" unless a test installs N._servo = function(a, out) -> n[, err].
    function N.servo_config(cfg)
        if type(cfg) ~= "table" or type(cfg.bones) ~= "table" then return nil, "bad" end
        st.servo_cfg = cfg
        return true
    end
    function N.servo_bodies(a, out)
        if type(a) ~= "table" or type(out) ~= "table" then return nil, "bad" end
        if not N._servo then return nil, "unavailable" end
        if not st.servo_cfg then return nil, "not configured" end
        st.servo_calls = (st.servo_calls or 0) + 1
        return N._servo(a, out)
    end
    function N.servo_weapon(a, out)
        if type(a) ~= "table" or type(out) ~= "table" then return nil, "bad" end
        if not N._servo_weapon then return nil, "unavailable" end
        if not st.servo_cfg then return nil, "not configured" end
        return N._servo_weapon(a, out)
    end
    function N.servo_status()
        return { available = N._servo ~= nil, configured = st.servo_cfg ~= nil, verified = N._servo ~= nil,
                 frames = st.servo_calls or 0, bodies = 0, avg_us = 0 }
    end
    -- Native neutralise (neutralise.rs): "unavailable" unless a test installs N._neutralise = function(a) -> n[, err].
    function N.neutralise_config(cfg)
        if type(cfg) ~= "table" then return nil, "bad" end
        st.neut_cfg = cfg
        return true
    end
    function N.neutralise(a)
        if type(a) ~= "table" then return nil, "bad" end
        if not N._neutralise then return nil, "unavailable" end
        if not st.neut_cfg then return nil, "not configured" end
        return N._neutralise(a)
    end
    function N.neutralise_status()
        return { available = N._neutralise ~= nil, configured = st.neut_cfg ~= nil, verified = N._neutralise ~= nil,
                 calls = 0, writes = 0, skipped = 0, avg_us = 0 }
    end

    -- ---- the sidecar's side (tests) ---------------------------------------------------------
    -- A sidecar-written slot (single or per peer). Marshalled like a game write, so a test
    -- cannot hand the mod a shape the native module would never produce.
    function N.sc_put(name, tbl, peer_slot)
        local s, rec = slot_rec(name)
        assert(s, "sc_put: unknown slot " .. tostring(name))
        local t, e = marshal_checked(S, rec, tbl)
        assert(t, "sc_put " .. name .. ": " .. tostring(e))
        if peer_slot ~= nil then
            st.peers[name] = st.peers[name] or {}
            local p = st.peers[name][peer_slot] or { ver = 0 }
            p.ver, p.t = p.ver + 2, t
            st.peers[name][peer_slot] = p
        else
            local c = st.slots[name] or { ver = 0 }
            c.ver, c.t = c.ver + 2, t
            st.slots[name] = c
        end
        if N._st and S.FLAGS then
            if name == "session" or name == "link" or name == "admin" then N._st.flags = (N._st.flags or 0) | S.FLAGS.SESSION_CHANGED end
        end
        return t
    end

    -- An S2G ring record (typed). Both mocks append it to their event log (sc_event); a mock
    -- without one queues it here and N.poll hands it out after the log.
    st.events = {}
    function N.sc_rec_event(kind, tbl, peer, aux, req_id)
        local t, e = marshal_checked(S, kind, tbl)
        assert(t, "sc_rec_event " .. tostring(kind) .. ": " .. tostring(e))
        if N.sc_event then
            N.sc_event(kind, t, req_id)
            local log = N._st and N._st.log
            if log and log[#log] then log[#log].peer, log[#log].aux = peer or 0, aux or 0 end
        else
            st.events[#st.events + 1] = { kind = kind, data = t, peer = peer or 0, aux = aux or 0, req_id = req_id or 0 }
        end
        return t
    end

    -- Typed G2S sends since the last drain.
    function N.sc_rec_drain()
        local g = st.g2s
        st.g2s = {}
        return g
    end

    -- The canonical table of a game-written slot / bus key (nil if never written).
    function N.sc_get(name)
        local c = st.slots[name] or st.bus[name]
        return c and deep(c.t) or nil
    end

    -- poll: typed records carry peer / aux (the base mock copies them from the log).
    if base.poll then
        function N.poll(max, out)
            local n = base.poll(max, out)
            if not N.sc_event and #st.events > 0 then
                -- file mock: typed records queued by sc_rec_event (a kind this state did not
                -- subscribe to is skipped, like the native cursor's filter)
                local f = N._st and N._st.filter
                local rest = {}
                for _, r in ipairs(st.events) do
                    if n >= max then
                        rest[#rest + 1] = r
                    elseif not f or f[r.kind] then
                        n = n + 1
                        local e = out[n] or {}
                        out[n] = e
                        e.kind, e.req_id, e.data, e.peer, e.aux = r.kind, r.req_id, deep(r.data), r.peer, r.aux
                    end
                end
                st.events = rest
                for i = n + 1, #out do out[i] = nil end
            end
            for i = 1, n do
                local e = out[i]
                if e and e.kind ~= "resync" then
                    e.peer = e.peer or 0
                    e.aux = e.aux or 0
                end
            end
            return n
        end
    end
    return N
end

return R
