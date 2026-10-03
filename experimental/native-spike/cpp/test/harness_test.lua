-- Offline checks run by harness.exe (host Lua copy + mock UE4SS.dll).
local function check(cond, msg)
  if not cond then error("CHECK FAILED: " .. msg, 2) end
  print("  ok  " .. msg)
end

local function loopback(api, label, n)
  local port, err = api.net_start("127.0.0.1:0", "self")
  check(port and port > 0, label .. " net_start -> port " .. tostring(port or err))
  local again, aerr = api.net_start("127.0.0.1:0", "self")
  check(again == nil and aerr == "already running", label .. " second net_start refused (" .. tostring(aerr) .. ")")
  local got, worst, sum = 0, 0, 0
  for i = 1, n do
    local t0 = api.now_us()
    local msg = string.format("ping|%d|%d|%s", i, t0, string.rep("x", i % 200))
    local sent, serr = api.net_send(msg)
    if not sent then error(label .. " net_send failed: " .. tostring(serr)) end
    local deadline = t0 + 2000000
    while true do
      local batch = api.net_poll(16)
      if #batch > 0 then
        for _, m in ipairs(batch) do
          if m ~= msg then error(label .. " payload mismatch: " .. m:sub(1, 40)) end
          got = got + 1
        end
        break
      end
      if api.now_us() > deadline then error(label .. " timeout waiting for echo " .. i) end
    end
    local rtt = api.now_us() - t0
    sum = sum + rtt
    if rtt > worst then worst = rtt end
  end
  check(got == n, string.format("%s %d/%d loopback echoes, avg %d us, worst %d us", label, got, n, sum // n, worst))
  local s = api.net_stats()
  check(s.sent >= n and s.recv >= n and s.running == 1, string.format("%s stats sent=%d recv=%d loops=%d", label, s.sent, s.recv, s.loops))
  local big_ok, big_err = api.net_send(string.rep("y", api.max_payload + 1))
  check(big_ok == nil and big_err == "message too big", label .. " oversize rejected without raising")
  local bad_ok, bad_err = api.net_send({})
  check(bad_ok == nil and type(bad_err) == "string", label .. " bad arg returns nil,err (" .. bad_err .. ")")
  check(api.net_stop() == true, label .. " net_stop")
  local ns, nerr = api.net_send("after stop")
  check(ns == nil and nerr == "not running", label .. " send after stop -> not running")
end

print("[harness_test] option F: HSMPNative global (C++ mod)")
local F = HSMPNative
check(type(F) == "table", "HSMPNative table present")
local pong = F.ping()
print("  " .. pong)
check(pong:find("pong from HSMPNative", 1, true), "HSMPNative.ping()")
check(F.lock_mode():find("private", 1, true), "lock_mode falls back to private with mock UE4SS.dll: " .. F.lock_mode())
check(F.process_event_add(2, 40) == 42, "process_event_add(2,40) == 42 via UObject::ProcessEvent")
check(F.process_event_add(-7, 7) == 0, "process_event_add(-7,7) == 0")
local fc1 = F.frame_count()
check(fc1 >= 123456 and F.frame_count() > fc1, "frame_count() via ProcessEvent (mock counter advances)")
local r, e = F.process_event_add("x")
check(r == nil and type(e) == "string", "process_event_add bad arg -> nil,err")
loopback(F, "F", 300)

print("[harness_test] option E: require 'hsmp_lua' (Rust Lua C module)")
local E = require("hsmp_lua")
check(type(E) == "table", "require('hsmp_lua') returned a table")
check(E.pinned == true, "module pinned itself (survives FreeLibrary on state close)")
print("  " .. E.ping())
check(E.ping():find("option E", 1, true), "hsmp_lua.ping()")
loopback(E, "E", 300)

print("[harness_test] F and E side by side (separate Rust cores, separate ports)")
local pf = F.net_start("127.0.0.1:0", "self")
local pe = E.net_start("127.0.0.1:0", "self")
check(pf and pe and pf ~= pe, "both cores running on ports " .. tostring(pf) .. " / " .. tostring(pe))
F.net_send("from F"); E.net_send("from E")
local t0 = F.now_us()
local fF, fE
repeat
  for _, m in ipairs(F.net_poll()) do fF = m end
  for _, m in ipairs(E.net_poll()) do fE = m end
until (fF and fE) or F.now_us() - t0 > 2000000
check(fF == "from F" and fE == "from E", "each core only sees its own traffic")
F.net_stop(); E.net_stop()

print("[harness_test] dual Lua copy GC stress (values created by native copy, collected by host copy)")
local keep = {}
for round = 1, 50 do
  E.net_start("127.0.0.1:0", "self")
  for i = 1, 200 do E.net_send(string.rep(string.char(65 + i % 26), i)) end
  local deadline = E.now_us() + 2000000
  local n = 0
  while n < 200 and E.now_us() < deadline do
    for _, m in ipairs(E.net_poll(64)) do n = n + 1; keep[#keep + 1] = m end
  end
  E.net_stop()
  if round % 10 == 0 then keep = {} end
  collectgarbage("collect")
  local st = F.net_stats(); st = nil
end
collectgarbage("collect")
check(true, "50 rounds x 200 msgs with full GC between rounds")

print("[harness_test] in-game probe (lua/HSMPNativeProbe/Scripts/main.lua) under a fake game-thread scheduler")
do
  local here = debug.getinfo(1, "S").source:sub(2):gsub("\\", "/"):match("^(.*)/")
  local probe_path = here .. "/../../lua/HSMPNativeProbe/Scripts/main.lua"
  local chunk, perr = loadfile(probe_path)
  check(chunk ~= nil, "probe compiles: " .. tostring(perr or "ok"))

  -- Minimal stand-ins for the UE4SS game-thread scheduling API the probe uses.
  local tasks, next_id = {}, 0
  local function add(ms, fn, loop)
    next_id = next_id + 1
    tasks[next_id] = { due = F.now_us() + ms * 1000, fn = fn, loop = loop and ms or nil }
    return next_id
  end
  local saved = { ExecuteInGameThreadWithDelay, LoopInGameThreadWithDelay, CancelDelayedAction, print }
  ExecuteInGameThreadWithDelay = function(ms, fn) return add(ms, fn, false) end
  LoopInGameThreadWithDelay = function(ms, fn) return add(ms, fn, true) end
  CancelDelayedAction = function(id) tasks[id] = nil end
  local verdict
  print = function(s)
    saved[4]("    probe> " .. tostring(s):gsub("\n$", ""))
    if tostring(s):find("M1 VERDICT", 1, true) then verdict = s end
  end

  chunk()
  local deadline = F.now_us() + 30 * 1000000
  while not verdict and next(tasks) and F.now_us() < deadline do
    local now, due = F.now_us(), {}
    for id, t in pairs(tasks) do
      if t.due <= now then due[#due + 1] = id end
    end
    table.sort(due)
    for _, id in ipairs(due) do
      local t = tasks[id]
      if t then -- may have been cancelled by an earlier callback
        if t.loop then t.due = now + t.loop * 1000 else tasks[id] = nil end
        t.fn()
      end
    end
  end
  ExecuteInGameThreadWithDelay, LoopInGameThreadWithDelay, CancelDelayedAction, print = table.unpack(saved)
  check(verdict ~= nil, "probe printed a verdict")
  -- In the harness the lock is 'private' (mock UE4SS.dll) but everything else must pass.
  check(verdict:find("M1 VERDICT F=PASS E=PASS", 1, true) ~= nil, "probe verdict F=PASS E=PASS")
end

print("[harness_test] all checks passed")
