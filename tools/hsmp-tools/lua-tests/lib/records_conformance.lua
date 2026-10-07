-- records_conformance.lua -- ONE set of checks for the typed-record API,
-- run against BOTH implementations:
--   * the Lua mock (lib/hsmp_native_mock.lua + lib/hsmp_native_records.lua, the spec):
--       hsmp-tools lua-test records
--   * the real native module (crates/hsmp-native/tests/records.rs, mlua + the real segment).
--
--   local C = require("records_conformance")
--   C.install_test_schema(S, R)            -- mock only: test records / slots / checks
--   C.run(N, H, check)                     -- check(ok, name, detail)
--
-- H plays the sidecar / tool side: H.sc_put(slot, t, peer_slot?), H.sc_rec_event(kind, t, peer,
-- aux, req_id), H.sc_rec_drain() -> {{kind, req_id, data}...}, H.sc_dev(t), H.sleep(ms),
-- H.comspec (cmd.exe path). For the mock, H is the mock itself (plus sleep / comspec).
--
-- The test-only records below exist in Rust only inside crates/hsmp-native/tests/records.rs
-- (registered with hsmp_native::api::register_test_schema); that test checks this table
-- against the Rust layouts field by field (C.signature).

local C = {}

-- ---- test schema (mirrors tests/records.rs) ------------------------------------------------
C.STRUCTS = {
    TInner = { size = 8, fields = {
        { "a", "u16" }, { "b", "bool" }, { "_r", "u8" }, { "c", "f32" },
    } },
    TFixed = { size = 152, fields = {
        { "u8v", "u8" }, { "i8v", "i8" }, { "u16v", "u16" }, { "u32v", "u32" },
        { "i16v", "i16" }, { "_r", "u16" }, { "i32v", "i32" },
        { "u64v", "u64" }, { "i64v", "i64" },
        { "f32v", "f32" }, { "flag", "bool" }, { "_r2", { "u8", 3 } },
        { "f64v", "f64" },
        { "name", { "str", 8 } },
        { "vec", { "f32", 3 } }, { "_r3", "u32" },
        { "inner", "TInner" },
        { "inners", { "TInner", 2 } },
        { "tags", { { "str", 4 }, 2 } },
        { "grid", { { "u8", 2 }, 2 } }, { "_r4", { "u8", 4 } },
        { "note", { "str", 40 } },
    } },
    TVar = { size = 24, fields = {
        { "id", "u32" }, { "n", "u16" }, { "flag", "bool" }, { "_r", "u8" }, { "name", { "str", 16 } },
    } },
    TRow = { size = 16, fields = {
        { "k", "u32" }, { "v", "f32" }, { "tag", { "str", 8 } },
    } },
    TFloats = { size = 8, fields = {
        { "n", "u16" }, { "_r", { "u16", 3 } },
    } },
}
C.RECORDS = {
    t_fixed = { id = 0x7f10, layout = "TFixed", row = nil, count = nil, max_rows = 0, cap = 0, flow = "g2s,s2g,local", chan = "none" },
    t_var = { id = 0x7f11, layout = "TVar", row = "TRow", count = "n", max_rows = 24, cap = 0, flow = "g2s,s2g", chan = "none" },
    t_floats = { id = 0x7f12, layout = "TFloats", row = "f32", count = "n", max_rows = 8, cap = 0, flow = "g2s", chan = "none" },
}
C.SLOTS = {
    t_fixed_out = { record = "t_fixed", form = "slot", dir = "g2s", cap = 0, world_scoped = true },
    t_var_out = { record = "t_var", form = "blob", dir = "g2s", cap = 0, world_scoped = false },
    t_local = { record = "t_fixed", form = "slot", dir = "local", cap = 0, world_scoped = false },
    t_fixed_in = { record = "t_fixed", form = "slot", dir = "s2g", cap = 0, world_scoped = false },
    t_var_in = { record = "t_var", form = "blob", dir = "s2g", cap = 0, world_scoped = false },
    t_peer = { record = "t_fixed", form = "peer_slot", dir = "s2g", cap = 0, world_scoped = false },
    t_peer_blob = { record = "t_var", form = "peer_blob", dir = "s2g", cap = 0, world_scoped = false },
    t_bus = { record = "t_fixed", form = "bus", dir = "local", cap = 0, world_scoped = true },
    t_bus_keep = { record = "t_var", form = "bus", dir = "local", cap = 0, world_scoped = false },
}
-- The records' own checks (Rust: check_fixed / check_var / check_row in tests/records.rs).
C.CHECKS = {
    t_fixed = function(t) if t.u8v == 77 then return "u8v" end end,
    t_var = function(t)
        if t.id == 999 then return "id" end
        for _, r in ipairs(t.rows or {}) do if r.k == 0xFFFFFFFF then return "k" end end
    end,
}

function C.install_test_schema(S, R)
    for k, v in pairs(C.STRUCTS) do S.STRUCTS[k] = v end
    for k, v in pairs(C.RECORDS) do S.RECORDS[k] = v; S.RECORD_BY_ID[v.id] = k end
    S.SLOTS = S.SLOTS or {}
    for k, v in pairs(C.SLOTS) do S.SLOTS[k] = v end
    for k, f in pairs(C.CHECKS) do R.CHECKS[k] = f end
end

-- A canonical one-line signature of a struct / record / slot (compared with the Rust side).
local function tsig(ty)
    if type(ty) == "table" then
        if ty[1] == "str" then return "str" .. ty[2] end
        return "[" .. tsig(ty[1]) .. ";" .. ty[2] .. "]"
    end
    return ty
end
function C.signature()
    local out = {}
    local names = {}
    for k in pairs(C.STRUCTS) do names[#names + 1] = k end
    table.sort(names)
    for _, k in ipairs(names) do
        local f = {}
        for _, x in ipairs(C.STRUCTS[k].fields) do f[#f + 1] = x[1] .. ":" .. tsig(x[2]) end
        out[#out + 1] = k .. "(" .. C.STRUCTS[k].size .. "){" .. table.concat(f, ",") .. "}"
    end
    names = {}
    for k in pairs(C.RECORDS) do names[#names + 1] = k end
    table.sort(names)
    for _, k in ipairs(names) do
        local r = C.RECORDS[k]
        out[#out + 1] = string.format("%s=%04x:%s:%s:%s:%d:%s", k, r.id, r.layout, tostring(r.row), tostring(r.count), r.max_rows, r.flow)
    end
    names = {}
    for k in pairs(C.SLOTS) do names[#names + 1] = k end
    table.sort(names)
    for _, k in ipairs(names) do
        local s = C.SLOTS[k]
        out[#out + 1] = string.format("%s=%s:%s:%s:%s", k, s.record, s.form, s.dir, tostring(s.world_scoped))
    end
    return table.concat(out, "\n")
end

-- ---- the checks ------------------------------------------------------------------------------
local function f32(v) return string.unpack("<f", string.pack("<f", v)) end

local function repr(v, depth)
    depth = depth or 0
    if type(v) ~= "table" then return type(v) == "string" and string.format("%q", v) or tostring(v) end
    if depth > 3 then return "{...}" end
    local keys = {}
    for k in pairs(v) do keys[#keys + 1] = k end
    table.sort(keys, function(a, b) return tostring(a) < tostring(b) end)
    local p = {}
    for _, k in ipairs(keys) do p[#p + 1] = tostring(k) .. "=" .. repr(v[k], depth + 1) end
    return "{" .. table.concat(p, ",") .. "}"
end
C.repr = repr

function C.run(N, H, check)
    local function eq(a, b, name)
        check(a == b and math.type(a) == math.type(b), name, "got " .. repr(a) .. " (" .. tostring(math.type(a)) .. "), want " .. repr(b))
    end
    local function err(name, want, r, e)
        check(r == nil and e == want, name, "got " .. repr(r) .. ", " .. repr(e) .. "; want nil, " .. repr(want))
    end
    -- put t into t_fixed_out and read it back
    local function rt(t)
        local ok, e = N.put("t_fixed_out", t)
        if not ok then return nil, e end
        local ver, g = N.get("t_fixed_out", -1)
        return g, ver
    end

    -- integers: wrap (integers), truncate + saturate (floats, NaN -> 0), booleans
    local g = rt({ u8v = 300, i8v = 128, u16v = 70000, u32v = -1, i16v = -1.5, i32v = 1e10, u64v = 1e30, i64v = -1e30 })
    check(g ~= nil, "int case 1 accepted")
    if g then
        eq(g.u8v, 44, "u8 wraps"); eq(g.i8v, -128, "i8 wraps"); eq(g.u16v, 4464, "u16 wraps")
        eq(g.u32v, 4294967295, "u32 wraps -1"); eq(g.i16v, -1, "i16 float truncates toward zero")
        eq(g.i32v, 2147483647, "i32 float saturates"); eq(g.u64v, -1, "u64 float saturates (reads -1)")
        eq(g.i64v, math.mininteger, "i64 float saturates low")
    end
    g = rt({ u8v = -1, i8v = 0 / 0, u16v = true, u32v = 2.9, i16v = -1e9, i32v = -2.5, u64v = 2.0 ^ 63, i64v = 1e30 })
    if g then
        eq(g.u8v, 255, "u8 wraps -1"); eq(g.i8v, 0, "NaN -> 0"); eq(g.u16v, 1, "true = 1"); eq(g.u32v, 2, "u32 truncates")
        eq(g.i16v, -32768, "i16 saturates low"); eq(g.i32v, -2, "i32 truncates toward zero")
        eq(g.u64v, math.mininteger, "u64 2^63 reads negative"); eq(g.i64v, math.maxinteger, "i64 saturates high")
    else check(false, "int case 2 accepted") end
    g = rt({ u64v = 1.5e19, i64v = math.huge, u32v = -math.huge, u8v = false })
    if g then
        eq(g.u64v, -3446744073709551616, "u64 large float"); eq(g.i64v, math.maxinteger, "i64 +inf saturates")
        eq(g.u32v, 0, "u32 -inf saturates to 0"); eq(g.u8v, 0, "false = 0")
    else check(false, "int case 3 accepted") end
    err("string in an int field", "bad:u16v", N.put("t_fixed_out", { u16v = "7" }))
    err("table in an int field", "bad:i64v", N.put("t_fixed_out", { i64v = {} }))

    -- floats: finite only (also after f32 rounding), f32 values, true = 1.0
    g = rt({ f32v = 0.1, f64v = 0.1 })
    if g then
        eq(g.f32v, f32(0.1), "f32 is rounded"); eq(g.f64v, 0.1, "f64 exact")
        eq(g.vec[1], 0.0, "missing array elements default to 0.0")
    end
    g = rt({ f32v = true, f64v = 1e300, vec = { 1, 2, 3, 4, 5 } })
    if g then
        eq(g.f32v, 1.0, "true = 1.0"); eq(g.f64v, 1e300, "f64 large ok")
        check(#g.vec == 3 and g.vec[3] == 3.0, "extra array elements ignored", repr(g.vec))
    end
    g = rt({ f32v = 3.4028234663852886e38, vec = { 7 } })
    if g then
        eq(g.f32v, 3.4028234663852886e38, "FLT_MAX ok")
        check(g.vec[1] == 7.0 and g.vec[2] == 0.0 and g.vec[3] == 0.0, "short array: rest defaults", repr(g.vec))
    end
    err("f32 overflow", "bad:f32v", N.put("t_fixed_out", { f32v = 3.5e38 }))
    err("f32 NaN", "bad:f32v", N.put("t_fixed_out", { f32v = 0 / 0 }))
    err("f64 -inf", "bad:f64v", N.put("t_fixed_out", { f64v = -math.huge }))
    err("NaN in an array", "bad:vec", N.put("t_fixed_out", { vec = { 1, 0 / 0 } }))
    err("array not a table", "bad:vec", N.put("t_fixed_out", { vec = "x" }))
    err("string in a float field", "bad:f64v", N.put("t_fixed_out", { f64v = "1" }))

    -- booleans
    g = rt({ flag = true, inner = { b = 2 } })
    if g then eq(g.flag, true, "bool true"); eq(g.inner.b, true, "bool from nonzero number") end
    g = rt({ flag = 0 })
    if g then eq(g.flag, false, "bool from 0"); eq(g.inner.b, false, "missing bool = false") end
    g = rt({ flag = "x", inner = { b = 0 / 0 } })
    if g then eq(g.flag, true, "bool from a string (truthy)"); eq(g.inner.b, true, "bool from NaN (~= 0)") end

    -- strings
    g = rt({ name = "abc", note = 12, tags = { "abcde", 5 } })
    if g then
        eq(g.name, "abc", "str"); eq(g.note, "12", "number -> tostring")
        check(g.tags[1] == "abcd" and g.tags[2] == "5", "str array truncates / converts", repr(g.tags))
    end
    g = rt({ name = "ééééé", note = 1.5 })
    if g then eq(g.name, "éééé", "UTF-8 truncation on a char boundary"); eq(g.note, "1.5", "float -> tostring") end
    g = rt({ name = "abcdefgé" })
    if g then eq(g.name, "abcdefg", "a 2-byte char straddling the cap is dropped") end
    g = rt({ name = string.rep("x", 20) .. "\xff" })
    if g then eq(g.name, "xxxxxxxx", "invalid bytes past the cap are cut away") end
    g = rt({})
    if g then eq(g.name, "", "missing str = \"\"") end
    err("NUL in a str", "bad:name", N.put("t_fixed_out", { name = "a\0b" }))
    err("invalid UTF-8", "bad:name", N.put("t_fixed_out", { name = "\xff" }))
    err("table in a str", "bad:name", N.put("t_fixed_out", { name = {} }))
    err("bad str in an array", "bad:tags", N.put("t_fixed_out", { tags = { "ok", {} } }))

    -- nested structs and arrays
    g = rt({ inner = { a = 70000, b = 1, c = 2.5 }, inners = { { a = 1 }, { a = 2 }, { a = 3 } }, grid = { { 1, 2 }, { 3 } }, foo = 1 })
    if g then
        check(g.inner.a == 4464 and g.inner.b == true and g.inner.c == 2.5, "nested struct", repr(g.inner))
        check(#g.inners == 2 and g.inners[2].a == 2, "struct array", repr(g.inners))
        check(g.grid[1][2] == 2 and g.grid[2][1] == 3 and g.grid[2][2] == 0, "nested arrays", repr(g.grid))
        check(g.foo == nil, "unknown input fields ignored")
        check(g._r == nil and g._r2 == nil and g._r3 == nil and g._r4 == nil and g.inner._r == nil, "padding never appears", repr(g))
    end
    g = rt({ inners = { [2] = { a = 2 } } })
    if g then check(g.inners[1].a == 0 and g.inners[1].b == false and g.inners[1].c == 0.0 and g.inners[2].a == 2, "missing struct = zeroed", repr(g.inners)) end
    err("struct not a table", "bad:inner", N.put("t_fixed_out", { inner = 5 }))
    err("error names the inner field", "bad:c", N.put("t_fixed_out", { inner = { c = "x" } }))
    err("struct array element not a table", "bad:inners", N.put("t_fixed_out", { inners = { 5 } }))
    err("nested array element not a table", "bad:grid", N.put("t_fixed_out", { grid = { { 1, 2 }, "x" } }))
    err("top level not a table", "bad", N.put("t_fixed_out", 5))
    err("record check", "bad:u8v", N.put("t_fixed_out", { u8v = 77 }))

    -- variable records: rows, count, capacity, checks
    local ok, e = N.put("t_var_out", { id = 1, n = 99, name = "v", rows = { { k = 1, v = 0.5, tag = "a" }, { k = 2 } } })
    check(ok == true, "variable record put", tostring(e))
    local ver, t = N.get("t_var_out", -1)
    if t then
        eq(t.n, 2, "count from #rows"); eq(#t.rows, 2, "rows")
        check(t.rows[1].v == 0.5 and t.rows[1].tag == "a" and t.rows[2].tag == "" and t.rows[2].v == 0.0, "row fields", repr(t.rows))
        check(t.flag == false and t.name == "v", "head fields", repr(t))
    else check(false, "variable record get", repr(ver)) end
    local rows25 = {}
    for i = 1, 25 do rows25[i] = { k = i } end
    err("rows over max_rows", "too_big", N.put("t_var_out", { rows = rows25 }))
    err("rows not a table", "bad:rows", N.put("t_var_out", { rows = "x" }))
    err("row not a table", "bad:TRow", N.put("t_var_out", { rows = { 5 } }))
    err("row field error", "bad:v", N.put("t_var_out", { rows = { { v = 0 / 0 } } }))
    err("head errors come first", "bad:id", N.put("t_var_out", { id = "x", rows = "y" }))
    err("count field type is still checked", "bad:n", N.put("t_var_out", { n = "x" }))
    err("head check", "bad:id", N.put("t_var_out", { id = 999 }))
    err("row check", "bad:k", N.put("t_var_out", { rows = { { k = 0xFFFFFFFF } } }))
    err("row check (wrapped -1)", "bad:k", N.put("t_var_out", { rows = { { k = -1 } } }))
    ok = N.put("t_var_out", { id = 2, rows = false })
    ver, t = N.get("t_var_out", -1)
    check(ok == true and t and t.n == 0 and type(t.rows) == "table" and #t.rows == 0, "rows = false: no rows", repr(t))

    -- reading into `out`: reuse, foreign keys removed, trailing rows nil, unchanged / never
    N.put("t_var_out", { id = 3, rows = { { k = 1 }, { k = 2 } } })
    local o = { junk = 1, rows = { { k = 9 }, { k = 8 }, { k = 7 }, extra = 1 } }
    local rows_t = o.rows
    local v1, o2 = N.get("t_var_out", -1, o)
    check(o2 == o, "get fills out in place")
    check(o.junk == nil and #o.rows == 2 and o.rows[3] == nil and o.rows[2].k == 2 and o.id == 3, "out: foreign keys removed, trailing rows nil", repr(o))
    check(o.rows == rows_t, "nested tables reused")
    local r1, r2 = N.get("t_var_out", v1, o)
    check(r1 == v1 and r2 == nil, "unchanged -> ver only", repr(r1) .. " " .. repr(r2))
    check(N.get("t_fixed_in", -1) == nil, "never written -> nil")
    err("get unknown slot", "bad", N.get("nope", -1))
    err("get a peer slot", "bad", N.get("t_peer", -1))

    -- slots the game may not put
    err("put a sidecar-written slot", "bad", N.put("t_fixed_in", {}))
    err("put a peer slot", "bad", N.put("t_peer", {}))
    err("put a bus slot", "bad", N.put("t_bus", {}))
    ok = N.put("t_local", { u8v = 1 })
    ver, t = N.get("t_local")
    check(ok == true and t and t.u8v == 1, "game-local slot", repr(t))

    -- sidecar-written slots / blobs and per-peer slots
    H.sc_put("t_fixed_in", { u8v = 5, name = "sc" })
    ver, t = N.get("t_fixed_in", -1)
    check(ver and t and t.u8v == 5 and t.name == "sc", "sidecar slot", repr(t))
    H.sc_put("t_fixed_in", { u8v = 6 })
    local ver2, t2 = N.get("t_fixed_in", ver)
    check(ver2 ~= ver and t2 and t2.u8v == 6, "sidecar slot new version", repr(t2))
    H.sc_put("t_var_in", { id = 4, rows = { { k = 4 } } })
    ver, t = N.get("t_var_in", -1)
    check(t and t.id == 4 and t.rows[1].k == 4, "sidecar blob", repr(t))
    check(N.get("t_var_in", ver) == ver, "sidecar blob unchanged")
    ver2, t2 = N.get("t_var_in", -1)
    check(ver2 == ver and t2 and t2.id == 4, "sidecar blob read again (cached per version)", repr(t2))
    H.sc_put("t_peer", { u8v = 5 }, 3)
    ver, t = N.peer("t_peer", 3, -1)
    check(ver and t and t.u8v == 5, "peer slot", repr(t))
    check(N.peer("t_peer", 3, ver) == nil, "peer unchanged -> nil")
    check(N.peer("t_peer", 4, -1) == nil, "peer never written -> nil")
    err("peer slot out of range", "bad", N.peer("t_peer", 32, -1))
    err("peer slot negative", "bad", N.peer("t_peer", -1, -1))
    err("peer on a single slot", "bad", N.peer("t_fixed_in", 0, -1))
    H.sc_put("t_peer_blob", { id = 7, rows = { { k = 1 }, { k = 2 }, { k = 3 } } }, 0)
    local po = { rows = {} }
    ver, t = N.peer("t_peer_blob", 0, -1, po)
    check(t == po and po.id == 7 and #po.rows == 3, "peer blob into out", repr(po))

    -- G2S sends
    H.sc_rec_drain()
    local id = N.send("t_fixed", { u8v = 9, name = "s" })
    check(math.type(id) == "integer", "send -> req_id", repr(id))
    N.send("t_var", { id = 5, rows = { { k = 1, tag = "x" } } })
    N.send("t_floats", { rows = { 1, 2.5 } })
    local d = H.sc_rec_drain()
    check(#d == 3 and d[1].kind == "t_fixed" and d[1].data.u8v == 9 and d[1].data.name == "s" and d[1].req_id == id, "sidecar sees the typed send", repr(d[1]))
    check(d[2] and d[2].kind == "t_var" and d[2].data.n == 1 and d[2].data.rows[1].tag == "x", "variable send", repr(d[2]))
    check(d[3] and d[3].data.n == 2 and d[3].data.rows[1] == 1.0 and d[3].data.rows[2] == 2.5, "primitive rows", repr(d[3]))
    err("send: record check", "bad:u8v", N.send("t_fixed", { u8v = 77 }))
    err("send: primitive row error names the type", "bad:f32", N.send("t_floats", { rows = { 1.5, 0 / 0 } }))
    err("send: rows over capacity", "too_big", N.send("t_var", { rows = rows25 }))
    err("send: not a g2s record", "bad", N.send("dev_cmd", { op = 1 }))
    err("send: not a table", "bad", N.send("t_fixed", "x"))
    local sent, full = 0, nil
    for _ = 1, 600 do
        local r, e2 = N.send("t_floats", {})
        if not r then full = e2 break end
        sent = sent + 1
    end
    check(full == "full" and sent == 512, "G2S ring full after 512", tostring(sent) .. " " .. tostring(full))
    H.sc_rec_drain()

    -- S2G events
    local ev = {}
    N.poll(64, ev)
    H.sc_rec_event("t_fixed", { u8v = 9, name = "ev" }, 7, 33, 55)
    H.sc_rec_event("t_var", { id = 8, rows = { { k = 3 } } }, 2, 0, 56)
    local n = N.poll(8, ev)
    check(n == 2 and ev[1].kind == "t_fixed" and ev[1].peer == 7 and ev[1].aux == 33 and ev[1].req_id == 55
        and ev[1].data.u8v == 9 and ev[1].data.name == "ev", "poll: typed event with peer / aux / req_id", repr(ev[1]))
    check(ev[2] and ev[2].kind == "t_var" and ev[2].data.rows[1].k == 3 and ev[3] == nil, "poll: variable event", repr(ev[2]))
    local keep = ev[1].data
    n = N.poll(8, ev)
    check(n == 0 and ev[1] == nil and keep.u8v == 9, "poll: drained; data tables are not reused")

    -- typed bus keys
    ok, e = N.bus_put("t_bus", { u8v = 3, name = "b" })
    check(ok == true, "bus_put typed", tostring(e))
    ver, t = N.bus_get("t_bus", -1)
    check(ver and ver ~= 0 and t and t.u8v == 3 and t.name == "b", "bus_get typed", repr(t))
    check(N.bus_get("t_bus", ver) == ver, "bus_get unchanged -> ver")
    check(N.bus_get("t_bus_keep", -1) == 0, "bus_get never written -> 0")
    err("bus_put: record check", "bad:u8v", N.bus_put("t_bus", { u8v = 77 }))
    N.bus_put("t_bus_keep", { id = 6, rows = { { k = 1 }, { k = 2 } } })
    local bo = {}
    ver, t = N.bus_get("t_bus_keep", -1, bo)
    check(t == bo and bo.id == 6 and #bo.rows == 2, "bus_get into out", repr(bo))

    -- The production local playback proof survives the real native marshaler,
    -- not just Lua table copies. All 32 peers must fit the bus intact.
    local proof_rows = {}
    for i = 1, 32 do
        proof_rows[i] = { peer=i, pawn="Willie_BP_C_"..i, match_id=9007199254740993, round=3, life=130,
            body_ts=1250.5, arm_ts=1250.5, local_ms=1600.0,
            settle_world="123456@World /Game/Maps/Map_Arena.Map_Arena", settle_reason="settled",
            settle_sample_ms=1600.0, settle_stable_ms=151.0, settle_source_ts=1234.5,
            settle_source_seq=9007199254740993, settle_cut=7, settle_pos_uu=4.5, settle_rot_deg=9.5,
            settle_ready=true, settle_count=6 }
    end
    ok, e = N.bus_put("playback", {rows=proof_rows})
    check(ok == true, "32-peer physical playback fits typed bus", tostring(e))
    local proof_out = {}
    ver, t = N.bus_get("playback", -1, proof_out)
    check(t == proof_out and t.n == 32 and #t.rows == 32, "32-peer playback is not truncated", repr(t and t.n))
    if t and t.rows and t.rows[32] then
        local p = t.rows[32]
        eq(p.match_id, 9007199254740993, "playback full match integer")
        eq(p.life, 130, "playback full life")
        eq(p.settle_source_seq, 9007199254740993, "physical source sequence full integer")
        eq(p.settle_source_ts, 1234.5, "physical integrated sender timestamp")
        eq(p.settle_sample_ms, 1600.0, "physical actual sample timestamp")
        eq(p.settle_stable_ms, 151.0, "physical continuous stable duration")
        check(p.pawn == "Willie_BP_C_32" and p.settle_world == proof_rows[32].settle_world
            and p.settle_reason == "settled" and p.settle_ready == true and p.settle_count == 6
            and p.settle_cut == 7 and p.settle_pos_uu == 4.5 and p.settle_rot_deg == 9.5,
            "physical world, pawn, cut and measured limits survive native transport", repr(p))
    end

    -- world leave: world-scoped game slots and bus keys lose their value
    N.put("t_fixed_out", { u8v = 1 })
    N.world_leaving()
    check(N.get("t_fixed_out", -1) == nil, "world-scoped slot invalid after world_leaving")
    ver, t = N.get("t_var_out", -1)
    check(t and t.id == 3, "non-world-scoped blob kept", repr(t))
    check(N.bus_get("t_bus", -1) == 0, "world-scoped bus key cleared")
    check(N.bus_get("playback", -1) == 0, "physical playback cleared on world leave")
    ver, t = N.bus_get("t_bus_keep", -1)
    check(t and t.id == 6, "non-world-scoped bus key kept", repr(t))
    if N.world_ready then N.world_ready("conformance") end
    N.put("t_fixed_out", { u8v = 2 })
    ver, t = N.get("t_fixed_out", -1)
    check(t and t.u8v == 2, "slot writable again after the leave", repr(t))

    -- DevCtl
    local dv = {}
    N.dev_poll(64, dv)
    H.sc_dev({ id = 1, op = 2, num = 0.5, key = "pose_stiffness" })
    n = N.dev_poll(8, dv)
    check(n == 1 and dv[1].kind == "dev_cmd" and dv[1].data.key == "pose_stiffness" and dv[1].data.num == 0.5
        and dv[1].data.op == 2 and dv[1].data.id == 1 and dv[1].data.arg == "", "dev_poll", repr(dv[1]))
    n = N.dev_poll(8, dv)
    check(n == 0 and dv[1] == nil, "dev_poll: once per state")
    H.sc_dev({ id = 2, op = 9 })
    H.sc_dev({ id = 3, op = 1, key = "join", arg = "127.0.0.1:7777" })
    n = N.dev_poll(8, dv)
    check(n == 1 and dv[1].data.id == 3 and dv[1].data.arg == "127.0.0.1:7777", "dev_poll drops invalid records", repr(dv))
    err("dev_poll without out", "bad", N.dev_poll(8))

    -- processes
    local cmd = H.comspec
    local function wait(f)
        for _ = 1, 1000 do
            if f() then return true end
            H.sleep(10)
        end
        return false
    end
    local me = N.current_pid()
    check(math.type(me) == "integer" and me > 0, "current_pid", repr(me))
    check(N.proc_alive(me) == false, "proc_alive: not ours -> false")
    err("proc_kill: not ours", "not_ours", N.proc_kill(me))
    err("proc_exit_code: not ours", "not_ours", N.proc_exit_code(me))
    err("capture_poll: unknown", "bad", N.capture_poll(123456))
    local pid = N.spawn(cmd, { "/c", "exit", "3" })
    check(math.type(pid) == "integer" and pid > 0, "spawn -> pid", repr(pid))
    if pid then
        check(wait(function() return not N.proc_alive(pid) end), "spawned process exits")
        eq(N.proc_exit_code(pid), 3, "exit code")
        err("proc_kill: gone", "gone", N.proc_kill(pid))
    end
    local h = N.spawn_capture(cmd, { "/c", "echo", "hello", "world" })
    check(math.type(h) == "integer", "spawn_capture -> h", repr(h))
    if h then
        local done, code, out
        wait(function()
            done, code, out = N.capture_poll(h)
            return done ~= false
        end)
        check(done == true and code == 0 and out == "hello world\r\n", "capture_poll: output and exit code", repr(done) .. " " .. repr(code) .. " " .. repr(out))
        err("capture_poll: released after done", "bad", N.capture_poll(h))
    end
    pid = N.spawn(cmd, { "/c", "ping", "-n", "30", "127.0.0.1" }, { hide = true, env = { HSMP_CONFORMANCE = "1", HSMP_UNSET_ME = false }, cwd = nil })
    if pid then
        check(N.proc_alive(pid) == true, "long process alive")
        err("proc_exit_code: running", "running", N.proc_exit_code(pid))
        check(N.proc_kill(pid) == true, "proc_kill: ours")
        check(wait(function() return not N.proc_alive(pid) end), "killed process exits")
        eq(N.proc_exit_code(pid), 1, "killed exit code")
    else check(false, "long spawn") end
    err("spawn: no exe", "bad", N.spawn(nil))
    err("spawn: args not a table", "bad", N.spawn(cmd, "x"))
    err("spawn: arg not a string", "bad", N.spawn(cmd, { {} }))
    err("spawn: bad env", "bad", N.spawn(cmd, {}, { env = { A = {} } }))
    err("spawn: bad cwd", "bad", N.spawn(cmd, {}, { cwd = 5 }))
    err("spawn: quote in exe", "bad", N.spawn('a"b', {}))
    err("spawn: % in a batch argument", "bad", N.spawn("x.cmd", { "%PATH%" }))
end

return C
