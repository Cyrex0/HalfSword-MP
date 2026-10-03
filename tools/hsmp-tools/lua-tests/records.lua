-- Suite: the typed-record table rules as implemented by the Lua mocks
-- (lib/hsmp_native_records.lua), the executable spec the native module follows.
--   hsmp-tools lua-test records

local NM = require("hsmp_native_mock")
local R = require("hsmp_native_records")
local S = NM.S

T.check(S.ABI_MAJOR == 2, "ABI 2 schema", tostring(S.ABI_MAJOR))
T.check(S.RECORDS.root and S.RECORDS.root.layout == "Root" and S.RECORDS.pose.row == "u8"
    and S.RECORDS.pose.max_rows > 0 and S.RECORDS.pose.count == "n", "records are in the generated schema", T.repr(S.RECORDS.pose))
T.check(S.ENUMS.dev_op and S.ENUMS.dev_op.TUNE == 2 and S.ENUM_NAMES.dev_op[2] == "TUNE", "code tables both ways")

-- Fixed record: missing fields default, padding never appears, floats are f32-rounded.
local t, e = R.marshal(S, "root", { tick = 7, pos = { 1, 2, 3 }, rot = { 0, 0, 0, 1 } })
T.check(t and t.tick == 7 and t.ts == 0 and t.send_wall_ms == 0 and t.pos[3] == 3.0 and #t.vel == 3
    and t.vel[1] == 0.0, "fixed record: defaults", T.repr(t) .. tostring(e))
local w = R.marshal(S, "weapon", { weapon_id = 3, held = 1, rot = { 0, 0, 0, 1 } })
T.check(w and w._r == nil and w._r2 == nil and w.held == 1, "padding fields never appear", T.repr(w))
local f32 = R.marshal(S, "root", { pos = { 0.1, 0, 0 } })
T.check(f32.pos[1] ~= 0.1 and math.abs(f32.pos[1] - 0.1) < 1e-7, "floats are f32 values", tostring(f32.pos[1]))

-- Refusals: NaN / inf floats, wrong types.
local _, e1 = R.marshal(S, "root", { pos = { 0 / 0, 0, 0 } })
local _, e2 = R.marshal(S, "root", { vel = { math.huge, 0, 0 } })
local _, e3 = R.marshal(S, "root", { tick = "x" })
T.check(e1 == "bad:pos" and e2 == "bad:vel" and e3 == "bad:tick", "non-finite floats and wrong types are refused",
    tostring(e1) .. " " .. tostring(e2) .. " " .. tostring(e3))

-- Integers: floats truncate toward zero and saturate, integers wrap, booleans are 0 / 1.
local i = R.marshal(S, "weapon", { weapon_id = 70000, held = -1.5, tick = true })
T.check(i.weapon_id == 70000 - 65536 and i.held == 0 and i.tick == 1, "int conversions", T.repr(i))
local sat = R.marshal(S, "weapon", { weapon_id = 1e9 })
T.check(sat.weapon_id == 65535, "float saturates", tostring(sat.weapon_id))

-- Variable record: rows, count from #rows, capacity.
local frame = {}
for k = 1, 30 do frame[k] = k end
local p = R.marshal(S, "pose", { tick = 5, rows = frame })
T.check(p and p.n == 30 and #p.rows == 30 and p.rows[30] == 30, "rows set the count", T.repr(p))
local big = {}
for k = 1, S.RECORDS.pose.max_rows + 1 do big[k] = 0 end
local _, eb = R.marshal(S, "pose", { rows = big })
T.check(eb == "too_big", "rows over capacity -> too_big", tostring(eb))

-- Strings: truncated on a UTF-8 boundary; numbers become strings; bools.
local d = R.marshal(S, "dev_cmd", { op = 2, key = string.rep("é", 20), arg = 12 })
T.check(d and #d.key <= 32 and d.key:sub(-2) == "é" and d.arg == "12", "Str truncation on a char boundary", T.repr(d))

-- The mock's typed paths: put / get / send / poll.
local N2 = NM.new{}
local ok, err = N2.send("root", { tick = 1, rot = { 0, 0, 0, 1 } })
T.check(ok == 1 and err == nil, "send: a G2S record kind", tostring(err))
local g = N2.sc_rec_drain()
T.check(#g == 1 and g[1].kind == "root" and g[1].data.tick == 1, "sc_rec_drain sees the typed send", T.repr(g))
local r0, e0 = N2.send("dev_cmd", { op = 1 })
T.check(r0 == nil and e0 == "bad", "send: a kind without the g2s flow is refused", tostring(e0))

-- ---- the shared conformance checks (lib/records_conformance.lua), against the mock --------
-- crates/hsmp-native/tests/records.rs runs the same script against the real native module.
local C = require("records_conformance")
C.install_test_schema(S, R)
local NC = NM.new{}
NC._proc.auto = true   -- simulate `cmd /c exit N` / `cmd /c echo ...`
local H = setmetatable({ sleep = function() end, comspec = "cmd.exe" }, { __index = NC })
C.run(NC, H, function(ok, name, detail) T.check(ok, "conformance: " .. name, detail or "") end)

-- DevCtl fan-out: every Lua state (cursor) sees every command once, from "now".
local ND = NM.new{}
ND.sc_dev({ id = 1, op = 3, num = 1 })
ND._set_cursor("B")             -- a state that registers after the first command
ND.sc_dev({ id = 2, op = 3, num = 0 })
local outB = {}
T.check(ND.dev_poll(8, outB) == 1 and outB[1].data.id == 2, "dev fan-out: a new state starts at now", C.repr(outB))
ND._set_cursor("default")
local outA = {}
T.check(ND.dev_poll(8, outA) == 2 and outA[1].data.id == 1 and outA[2].data.id == 2, "dev fan-out: the first state sees both", C.repr(outA))

-- Process mock helpers (for the mod suites).
local NP = NM.new{}
local p = NP.spawn("hsmp-sidecar.exe", { "--ipc", "shm:x", 7 }, { cwd = "C:/x", env = { HSMP_X = "1" } })
T.check(NP._proc.spawned[1].args[3] == "7" and NP.proc_alive(p) and NP._proc.spawned[1].opts.env.HSMP_X == "1", "spawn recorded", C.repr(NP._proc))
local h = NP.spawn_capture("hsmp-server.exe", { "--help" })
T.check(NP.capture_poll(h) == false, "capture running")
NP.sc_capture_done(h, 0, "usage")
local d1, c1, o1 = NP.capture_poll(h)
T.check(d1 == true and c1 == 0 and o1 == "usage", "capture done", C.repr({ d1, c1, o1 }))
T.check(NP.proc_kill(p) == true and NP._proc.killed[1] == p and not NP.proc_alive(p), "proc_kill")

-- Combat: typed S2G events carry peer / aux, peer slots and bus records work in BOTH mocks
-- (the in-memory one and the file-backed default of every suite).
local FM = require("hsmp_native_filemock")
for _, mk in ipairs({ { "mock", function() return NM.new{} end }, { "filemock", function() return FM.new{ dir = function() return T.tmpdir("hsmp_rec_fm_") end } end } }) do
    local name, N3 = mk[1], mk[2]()
    N3.subscribe({ "damage_in", "death" })
    N3.sc_rec_event("damage_in", { hit_id = 4, target_peer_id = 1, bone = "head" }, 2, 9)
    N3.sc_rec_event("death", { peer_id = 3, round = 1 })
    local out = {}
    local n = N3.poll(16, out)
    T.check(n == 2 and out[1].kind == "damage_in" and out[1].peer == 2 and out[1].aux == 9 and out[1].data.hit_id == 4
        and out[2].kind == "death" and out[2].peer == 0 and out[2].data.peer_id == 3,
        name .. ": typed S2G records keep peer / aux through poll", T.repr(out))
    N3.sc_put("peer_vitals", { seq = 2, flags = 1, v = { 64 } }, 5)
    local ver, pv = N3.peer("peer_vitals", 5, nil)
    T.check(ver ~= nil and pv.seq == 2 and pv.v[1] == 64 and N3.peer("peer_vitals", 5, ver) == nil,
        name .. ": typed peer slot (version-gated)", T.repr(pv))
    T.check(N3.bus_put("standin_dead", { wall = 7, rows = { { peer = 2, name = "Willie_BP_C_5" } } }) == true, name .. ": typed bus put")
    local bv, bt = N3.bus_get("standin_dead", nil)
    T.check(bv ~= nil and bt.wall == 7 and bt.n == 1 and bt.rows[1].name == "Willie_BP_C_5" and N3.bus_get("standin_dead", bv) == bv,
        name .. ": typed bus record read back", T.repr(bt))
    T.check(N3.put("vitals", { seq = 1, v = { 6400 } }) == true and N3.sc_get("vitals").v[1] == 6400, name .. ": game vitals slot")
    T.check(N3.send("death_report", { round = 2 }) and N3.sc_rec_drain()[1].data.round == 2, name .. ": death_report is a typed G2S send")
end
-- Native sampling in the mock: no engine -> "unavailable"; a test hook stands in.
local NS = NM.new{}
local s1, s2 = NS.sample_local({})
T.check(s1 == nil and s2 == "unavailable", "sample_local without an engine is unavailable", tostring(s2))
NS._sample = function(a) return (a.mesh and 4 or 0) | (a.root_pawn and 1 or 0) end
local n1, n2 = NS.sample_local({ mesh = 1 })
T.check(n1 == nil and n2 == "not configured", "sample_local before sample_config", tostring(n2))
T.check(NS.sample_config({ bones = {} }) == true and NS.sample_local({ mesh = 1, root_pawn = 2 }) == 5, "sample_local through the test hook")
T.check(NS.sample_status().samples == 1 and NS.sample_status().available == true, "sample_status", C.repr(NS.sample_status()))
