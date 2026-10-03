-- HSMPNativeProbe (transport spike, dev only). Exercises both native transport options
-- in the real game and prints one verdict line:
--   [HSMPNativeProbe] M1 VERDICT F=PASS E=PASS ...
--
-- F: global `HSMPNative`, injected by the C++ mod Mods/HSMPNative/dlls/main.dll
--    before this file runs (CppUserModBase::on_lua_start).
-- E: require("hsmp_lua") -> Mods/HSMPNativeProbe/Scripts/hsmp_lua.dll (Rust).
--
-- Everything after the top-level checks runs on the game thread only
-- (ExecuteInGameThreadWithDelay / LoopInGameThreadWithDelay), per the HSMP
-- threading rule. Nothing here touches gameplay state.

local TAG = "[HSMPNativeProbe] "
local function Log(fmt, ...) print(string.format(TAG .. fmt .. "\n", ...)) end

local result = { F_pe = "FAIL(not run)", F_net = "FAIL(not run)", E_net = "FAIL(not run)" }
local notes = {}

-------------------------------------------------------------------------- top level
local F = rawget(_G, "HSMPNative")
local top_tid
if type(F) == "table" then
  local ok, pong = pcall(F.ping)
  Log("F top-level: %s", ok and pong or ("ping raised: " .. tostring(pong)))
  top_tid = F.thread_id()
else
  Log("F top-level: HSMPNative global MISSING (C++ mod not loaded? check mods.txt + UE4SS.log 'Starting C++ mod')")
  result.F_pe = "FAIL(no global)"
end

local okE, E = pcall(require, "hsmp_lua")
if okE and type(E) == "table" then
  Log("E top-level: %s (pinned=%s)", E.ping(), tostring(E.pinned))
else
  Log("E top-level: require('hsmp_lua') failed: %s", tostring(E))
  result.E_net = "FAIL(require: " .. tostring(E) .. ")"
  E = nil
end

-------------------------------------------------------------------- game-thread tests
local function micro_bench(api)
  -- cost of the Lua->native hop on the game thread (queues only, no syscalls)
  local n = 2000
  local t0 = api.now_us()
  for i = 1, n do api.net_poll(1) end
  local poll_us = (api.now_us() - t0) / n
  return poll_us
end

local function loopback(api, label, count, done)
  local port, err = api.net_start("127.0.0.1:0", "self")
  if not port then
    result[label .. "_net"] = "FAIL(net_start: " .. tostring(err) .. ")"
    return done()
  end
  Log("%s net_start ok, UDP port %d, lock=%s", label, port, api.lock_mode())
  local seq, got, sum, worst = 0, 0, 0, 0
  local inflight = {}
  local started = api.now_us()
  local h
  h = LoopInGameThreadWithDelay(16, function()
    local ok, e = pcall(function()
      -- one ping per tick
      if seq < count then
        seq = seq + 1
        local t = api.now_us()
        inflight[seq] = t
        local s, serr = api.net_send(string.format("probe|%s|%d|%d", label, seq, t))
        if not s then error("net_send: " .. tostring(serr)) end
      end
      for _, m in ipairs(api.net_poll(64)) do
        local l, s = m:match("^probe|(%a)|(%d+)|")
        s = tonumber(s)
        if l == label and s and inflight[s] then
          local rtt = api.now_us() - inflight[s]
          inflight[s] = nil
          got = got + 1
          sum = sum + rtt
          if rtt > worst then worst = rtt end
        end
      end
    end)
    local finished = (got >= count) or (api.now_us() - started > 15000000) or not ok
    if finished then
      CancelDelayedAction(h)
      local st = api.net_stats()
      local bench = micro_bench(api)
      api.net_stop()
      if ok and got >= count then
        result[label .. "_net"] = "PASS"
      else
        result[label .. "_net"] = string.format("FAIL(%s, %d/%d echoes)", ok and "timeout" or tostring(e), got, count)
      end
      notes[#notes + 1] = string.format("%s: %d/%d echoes avg %.0fus worst %dus, sent=%d recv=%d drop_in=%d drop_out=%d, net_poll(1) %.2fus/call",
        label, got, count, got > 0 and sum / got or -1, worst, st.sent, st.recv, st.drop_in_full, st.drop_out_full, bench)
      Log("%s", notes[#notes])
      done()
    end
  end)
end

local function verdict()
  local F_ok = result.F_pe == "PASS" and result.F_net == "PASS"
  local E_ok = result.E_net == "PASS"
  Log("M1 VERDICT F=%s E=%s | F.process_event=%s F.net=%s E.net=%s | %s",
    F_ok and "PASS" or "FAIL", E_ok and "PASS" or "FAIL",
    result.F_pe, result.F_net, result.E_net, table.concat(notes, " | "))
end

ExecuteInGameThreadWithDelay(5000, function()
  -- F: ProcessEvent from native code, on the game thread
  if F then
    local gt = F.thread_id()
    local sum, perr = F.process_event_add(2, 40)
    local f1 = F.frame_count()
    Log("F game thread tid=%d (top-level tid=%s); KismetMathLibrary:Add_IntInt(2,40) via ProcessEvent = %s %s; frame=%s",
      gt, tostring(top_tid), tostring(sum), perr or "", tostring(f1))
    ExecuteInGameThreadWithDelay(500, function()
      local f2 = F.frame_count()
      Log("F frame_count advanced %s -> %s (proves a live engine call)", tostring(f1), tostring(f2))
      if sum ~= 42 then
        result.F_pe = "FAIL(Add_IntInt returned " .. tostring(sum) .. " " .. tostring(perr) .. ")"
      elseif not (f1 and f2 and f2 > f1) then
        result.F_pe = "FAIL(frame_count did not advance)"
      else
        result.F_pe = "PASS"
      end
      loopback(F, "F", 100, function()
        if E then
          loopback(E, "E", 100, verdict)
        else
          verdict()
        end
      end)
    end)
  elseif E then
    loopback(E, "E", 100, verdict)
  else
    verdict()
  end
end)

Log("loaded; game-thread tests start in 5 s")
