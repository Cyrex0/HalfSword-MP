-- HSMPNative offline IPC test, run by harness.exe in mode F (main.dll) and E (hsmp_lua.dll)
-- against the hsmp-fake-sidecar process. Globals from the harness: MODE, HSMP_LUA_DLL,
-- FAKE_SIDECAR, GAME_PID, run_in(i, code), spawn(cmd), kill(pid), sleep_ms(ms).
local N = HSMPNative

local function check(cond, msg)
  if not cond then error("CHECK FAILED: " .. msg, 2) end
  print("  ok  " .. msg)
end

local function wait(cond, ms, label)
  local t0 = N.now_us()
  while true do
    local ok = cond()
    if ok then return ok end
    if N.now_us() - t0 > ms * 1000 then error("timeout waiting for " .. label, 2) end
    sleep_ms(1)
  end
end

print(string.format("[harness_ipc] mode %s, impl %s, lock %s", MODE, tostring(N._impl), tostring(N.lock_mode)))
check(N._impl == MODE, "implementation tag " .. tostring(N._impl))
local ma, mi, hash = N.abi()
check(ma == 2 and mi == 0 and #hash == 16, "abi " .. ma .. "." .. mi .. " " .. hash)

-- Calls before ipc_open never raise.
local r, e = N.send("chat", {text = "x"})
check(r == nil and e == "not open", "send before open -> nil, not open")
check(N.frame("arena1") == 0, "frame before open -> 0")

local name = N.ipc_open()
check(type(name) == "string" and name:find("HSMP.ipc.2.", 1, true), "ipc_open -> " .. tostring(name))
check(N.ipc_open() == name, "ipc_open idempotent")

if MODE == "F" then
  local E = assert(package.loadlib(HSMP_LUA_DLL, "luaopen_hsmp_lua"))()
  check(E == N, "E in a state with F returns the F table")
  local res = run_in(0, [[
    local E = assert(package.loadlib(HSMP_LUA_DLL, "luaopen_hsmp_lua"))()
    local r, e = E.ipc_open()
    return tostring(E._impl) .. "|" .. tostring(r) .. "|" .. tostring(e)
  ]])
  check(res:find("^E|nil|ipc owned by another native copy"), "a second native copy refuses ipc_open: " .. res)
end

-- Attach the fake sidecar.
local function start_sidecar()
  return spawn(string.format('"%s" --ipc shm:%s --parent-pid %d --peers 7', FAKE_SIDECAR, name, GAME_PID))
end
local pid = start_sidecar()
local flags = wait(function() local f = N.frame("arena1"); return (f & 0x8 ~= 0) and f end, 5000, "SIDECAR_RESET")
check(true, "SIDECAR_RESET after attach (flags " .. flags .. ")")
local info = N.ipc_info()
check(info.sidecar_pid == pid and info.caps_effective ~= 0 and info.state == "ready", "ipc_info: sidecar pid " .. info.sidecar_pid .. ", caps " .. info.caps_effective)

local peers = {}
wait(function() N.frame("arena1"); return N.peers(peers) == 7 end, 2000, "7 peers")
check(peers[1].id == 100 and peers[7].slot == 6 and peers[1].nick == "peer0", "peers(): 7 entries")
local plays = {}
for i = 1, 7 do
  plays[i] = {}
  local s = wait(function() return N.peer_play(i - 1, plays[i], -1) end, 1000, "play " .. i)
end
check(#plays[1].B == 325 and plays[3].root[1] == 2 and plays[1].v2 == true and plays[1].mode == "interp", "peer_play() x7")

local link
wait(function()
  local v, t = N.get("link", -1)
  link = t
  return t and t.my_peer_id == 1 and t.status ~= 0
end, 2000, "link record")
check(link.my_peer_id == 1 and link.attempt >= 0, "get('link'): the sidecar's link record decoded")

-- Command round trip through both rings.
local id = N.send("chat", {text = "hello:Grüße"})
check(math.type(id) == "integer", "send -> req_id " .. tostring(id))
local ev = {}
local got
wait(function()
  N.frame("arena1")
  local n = N.poll(256, ev)
  for i = 1, n do
    if ev[i].kind == "cmd_result" and ev[i].req_id == id then got = ev[i].data end
  end
  return got
end, 2000, "cmd_result")
check(got.ok == true and got.reason_text == "chat:hello:Grüße", "cmd_result echoes the command (UTF-8 intact): " .. tostring(got.ok) .. " " .. tostring(got.reason_text))

-- Fan-out: the other two Lua states see the notices independently.
sleep_ms(250)
N.frame("arena1")
local count_notices = [[
  local ev, c = {}, 0
  local n = HSMPNative.poll(4096, ev)
  for i = 1, n do if ev[i].kind == "notice" then c = c + 1 end end
  return c
]]
local c2, c3 = tonumber(run_in(2, count_notices)), tonumber(run_in(3, count_notices))
check(c2 and c3 and c2 > 0 and c3 > 0, "fan-out: state 2 got " .. tostring(c2) .. " notices, state 3 got " .. tostring(c3))

-- Allocation-free hot path with a live writer (1 kHz).
local b, w, c = {}, {}, {}
for i = 1, 23 * 13 do b[i] = i * 0.25 end
for i = 1, 21 do w[i] = i end
for i = 1, 37 do c[i] = i end
local seqs = {}
local function hot()
  N.frame("arena1")
  N.put_root(1, 2, 1, 2, 3, 0, 0, 0, 0, 0, 0)
  N.put_weapon(1, 2, 5, 1, 1, 2, 3, 0, 0, 0, 0, 0, 0)
  N.put_pose(1, 2, 3, 1, b, w, c)
  N.peers(peers)
  for i = 1, 7 do
    local s = N.peer_play(i - 1, plays[i], seqs[i] or -1)
    if s then seqs[i] = s end
  end
end
for i = 1, 200 do hot() end
collectgarbage("collect"); collectgarbage("stop")
local k0 = collectgarbage("count")
for i = 1, 10000 do hot() end
local grew = collectgarbage("count") - k0
collectgarbage("restart")
check(grew < 1, string.format("10k frames with a live sidecar allocated %.3f KiB", grew))

-- Timings (live sidecar writing every peer at 1 kHz).
local function bench(label, n, fn)
  local t0 = N.now_us()
  for i = 1, n do fn() end
  print(string.format("TIMING %-34s %8.3f us", label, (N.now_us() - t0) / n))
end
local lv = N.get("link", -1)
bench("frame", 20000, function() N.frame("arena1") end)
bench("put_root", 20000, function() N.put_root(1, 2, 1, 2, 3, 0, 0, 0, 0, 0, 0) end)
bench("put_pose", 20000, function() N.put_pose(1, 2, 3, 1, b, w, c) end)
bench("peers (7)", 20000, function() N.peers(peers) end)
bench("peer_play x7 (forced read)", 5000, function() for i = 1, 7 do N.peer_play(i - 1, plays[i], -1) end end)
bench("poll (empty)", 20000, function() N.poll(16, ev) end)
bench("get link (unchanged)", 20000, function() N.get("link", lv) end)
bench("hot frame (Sync+Avatars, 7 peers)", 5000, hot)

-- World epochs.
local we = N.ipc_info().world_epoch
check(N.world_leaving() and N.ipc_info().world_epoch == we + 1 and N.ipc_info().state == "loading", "world_leaving bumps world_epoch")
check(N.world_ready("arena2") and N.ipc_info().state == "ready", "world_ready")
check(N.frame("arena2") & 0x10 == 0 and N.frame("arena3") & 0x10 ~= 0, "world key change -> WORLD_CHANGED")

-- Bus.
check(N.bus_put("conn_state", {state = "fight", reason = "Grüße", wall = 2.5}), "bus_put (a typed bus record)")
local bus = run_in(3, [[local g, t = HSMPNative.bus_get("conn_state", -1); return t.state .. "|" .. t.reason .. "|" .. t.wall ]])
check(bus == "fight|Grüße|2.5", "bus_get from another Lua state: " .. bus)

-- Sidecar death and restart.
check(kill(pid), "killed fake sidecar by PID " .. pid)
wait(function() return N.frame("arena3") & 0x80 ~= 0 end, 3000, "SIDECAR_LOST")
check(true, "SIDECAR_LOST after the kill")
pid = start_sidecar()
wait(function() return N.frame("arena3") & 0x8 ~= 0 end, 5000, "SIDECAR_RESET after restart")
check(N.ipc_info().attach_count == 2, "restarted sidecar re-attached (attach_count 2)")
wait(function() N.frame("arena3"); return N.peers(peers) == 7 end, 2000, "peers after restart")
check(true, "peers back after restart")
kill(pid)
print("[harness_ipc] PASS")
