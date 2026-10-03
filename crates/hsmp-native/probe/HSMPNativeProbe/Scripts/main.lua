-- HSMPNativeProbe (dev only): checks the HSMP-SHM native binding in the real
-- game and prints one verdict line to UE4SS.log:
--   [HSMPNativeProbe] IPC VERDICT attach=PASS abi=PASS slots=PASS ring=PASS bus=PASS frame_us=<p50>/<p99> impl=F lock=host...
--
-- attach: ipc_open() works and, if an HSMP sidecar is running for this game, it attached
--         (otherwise "attach=NOSIDECAR", which is not a failure of the binding).
-- abi:    HSMPNative.abi() matches mods/shared/hsmp_ipc_schema.lua when present.
-- slots:  put_root / put_pose / put("vitals") succeed; peers() / peer_play() / state() never raise.
-- ring:   send() returns a req_id (sidecar attached) or nil,"cap" (no sidecar); poll() never raises.
-- bus:    bus_put / bus_get round trip.
-- frame_us: frame() cost over 600 game frames.
--
-- Runs on the game thread only (LoopInGameThreadWithDelay), touches no gameplay state and
-- never sends OS input.

local TAG = "[HSMPNativeProbe] "
local function Log(fmt, ...) print(string.format(TAG .. fmt .. "\n", ...)) end

-- Opt-in only: the probe pumps frame() with its own world key and writes test samples, which
-- would disturb HSMPSync's pump in a real session. A -Dev deploy enables the mod; the probe
-- runs only with HSMP_NATIVE_PROBE=1 (native checks only; never during a gate run).
if (os.getenv("HSMP_NATIVE_PROBE") or "") ~= "1" then
  Log("idle (set HSMP_NATIVE_PROBE=1 to run)")
  return
end

local N = rawget(_G, "HSMPNative")
local impl = "F"
if type(N) ~= "table" then
  local dll = (debug.getinfo(1, "S").source:match("^@(.*[\\/])Mods[\\/]") or "") .. "Mods/HSMPNative/dlls/hsmp_lua.dll"
  local f = package.loadlib(dll, "luaopen_hsmp_lua")
  if f then
    local ok, t = pcall(f)
    if ok and type(t) == "table" then N, impl = t, "E" end
  end
end
if type(N) ~= "table" then
  Log("IPC VERDICT attach=FAIL(no HSMPNative global and hsmp_lua.dll not loadable) abi=FAIL slots=FAIL ring=FAIL bus=FAIL frame_us=0/0")
  return
end

local res = { attach = "FAIL", abi = "FAIL", slots = "FAIL", ring = "FAIL", bus = "FAIL" }

local okS, S = pcall(require, "hsmp_ipc_schema")
local ma, mi, hash = N.abi()
if okS and type(S) == "table" then
  res.abi = (S.ABI_MAJOR == ma and S.LAYOUT_HASH == hash) and "PASS" or string.format("FAIL(dll %d %s, lua %s %s)", ma, hash, tostring(S.ABI_MAJOR), tostring(S.LAYOUT_HASH))
else
  res.abi = string.format("PASS(dll %d.%d %s, no schema file)", ma, mi, hash)
end

local name, err = N.ipc_open()
if not name then
  res.attach = "FAIL(" .. tostring(err) .. ")"
end

local samples, frames = {}, 0
local b, w, c = {}, {}, {}
for i = 1, 23 * 13 do b[i] = 0 end
local peers, play, ev = {}, {}, {}

local function finish()
  table.sort(samples)
  local p50 = samples[math.max(1, #samples // 2)] or 0
  local p99 = samples[math.max(1, math.floor(#samples * 0.99))] or 0
  local info = N.ipc_info()
  if name then
    if info.sidecar_epoch ~= "0000000000000000" then
      res.attach = (info.state ~= "disabled") and "PASS" or "FAIL(disabled)"
    else
      res.attach = "NOSIDECAR"
    end
  end
  -- slots
  local ok1 = N.put_root(1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0)
  local ok2 = N.put_pose(1, 0, 0, 0, b, w, nil)
  local ok3, e3 = N.put("vitals", { seq = 1, v = { 6400 } })
  local okp = pcall(function()
    local n = N.peers(peers)
    for i = 1, n do N.peer_play(peers[i].slot, play, -1) end
    N.get("session", -1)
  end)
  res.slots = (ok1 and ok2 and (ok3 or e3 == "cap") and okp) and "PASS" or string.format("FAIL(%s %s %s %s)", tostring(ok1), tostring(ok2), tostring(e3), tostring(okp))
  -- ring
  local id, se = N.send("death_report", { death_id = 1, round = 0 })
  local okpoll = pcall(N.poll, 16, ev)
  if (id or se == "cap") and okpoll then res.ring = id and "PASS" or "PASS(no sidecar caps)" else res.ring = "FAIL(" .. tostring(se) .. ")" end
  -- bus
  local okb = N.bus_put("conn_state", { state = "probe", reason = "Grüße" })
  local g, t = N.bus_get("conn_state", -1)
  res.bus = (okb and type(t) == "table" and t.reason == "Grüße") and "PASS" or "FAIL"
  Log("IPC VERDICT attach=%s abi=%s slots=%s ring=%s bus=%s frame_us=%.1f/%.1f impl=%s lock=%s counters.wrong_thread=%d",
    res.attach, res.abi, res.slots, res.ring, res.bus, p50, p99, impl, tostring(N.lock_mode), info.counters and info.counters.wrong_thread or -1)
end

local done = false
local function tick()
  -- UE4SS 4.0-rc1 kept looping after the callback returned true (seen in game):
  -- do nothing once the verdict is out.
  if done then return true end
  frames = frames + 1
  local t0 = N.now_us()
  N.frame("probe")
  samples[#samples + 1] = N.now_us() - t0
  if frames >= 600 then
    done = true
    finish()
    return true
  end
  return false
end

if LoopInGameThreadWithDelay then
  LoopInGameThreadWithDelay(1, function() return tick() end)
else
  -- Offline / older UE4SS: run synchronously (still the calling thread).
  while not tick() do end
end
Log("probe started (impl %s, ipc %s)", impl, tostring(name or err))
