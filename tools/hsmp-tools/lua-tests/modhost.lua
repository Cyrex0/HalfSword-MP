-- HSMPModHost's loader (mods/HSMPModHost/Scripts/modhost.lua) against a fake UE4SS API:
--
--     hsmp-tools lua-test modhost
--
--   * a server mod gets its own globals, no HSMPNative / HSMP_IPC, require only inside its
--     own Scripts folder (UEHelpers excepted), no precompiled chunks; HSMP* names refused;
--   * its registrations are tracked: unloading removes hooks / custom events, cancels
--     delayed actions, makes the rest inert; errors never escape and name the mod.

local T = T
local Host = dofile(T.path("mods/HSMPModHost/Scripts/modhost.lua"))
local cache = T.tmpdir("hsmp_modhost_")
local function hash(c) return string.rep(c, 64) end
local function put(h, rel, src)
    local p = cache .. "/" .. h .. "/Scripts/" .. rel
    T.mkdir(p:match("^(.*)/[^/]*$"))
    T.write(p, src)
end

-- the fake UE4SS
local calls = { hooks = {}, unhooked = {}, cancelled = {}, events = {}, unevents = {}, keys = {} }
local G = setmetatable({}, { __index = _G })
G.HSMPNative = { secret = true }
G.HSMP_IPC = { secret = true }
G.RegisterHook = function(path, pre, post) calls.hooks[#calls.hooks + 1] = { path, pre, post }; return 11, 12 end
G.UnregisterHook = function(path, a, b) calls.unhooked[#calls.unhooked + 1] = { path, a, b } end
G.RegisterCustomEvent = function(name, cb) calls.events[name] = cb end
G.UnregisterCustomEvent = function(name) calls.unevents[#calls.unevents + 1] = name end
G.RegisterKeyBind = function(key, cb) calls.keys[#calls.keys + 1] = cb end
local next_h = 100
G.LoopAsync = function(ms, cb) next_h = next_h + 1; calls.loop = cb; return next_h end
G.CancelDelayedAction = function(h) calls.cancelled[#calls.cancelled + 1] = h end
G.require = function(n) if n == "UEHelpers" then return { shared = true } end error("no " .. n) end

local logs = {}
local H = Host.new({ G = G, cache = cache, read = function(p) return T.read(p) end,
    log = function(fmt, ...) logs[#logs + 1] = string.format(fmt, ...) end })

put(hash("a"), "util.lua", "return { v = 42 }")
put(hash("a"), "main.lua", [[
    local u = require("util")
    local ueh = require("UEHelpers")
    mine = u.v
    seen = { native = HSMPNative, ipc = HSMP_IPC, gnative = _G.HSMPNative, shared = ueh.shared }
    escape = pcall(require, "..util") or pcall(require, "hsmp_ipc")
    RegisterHook("/Script/Engine.Actor:Tick", function() hook_ran = (hook_ran or 0) + 1 end, function() error("boom") end)
    RegisterCustomEvent("Ping", function() pinged = true end)
    RegisterKeyBind(1, function() key_ran = true end)
    LoopAsync(100, function() return false end)
    function OnUnload() unloaded = true end
]])
put(hash("b"), "main.lua", "error('broken mod')")
put(hash("c"), "main.lua", "\27LuaT bytecode")

local ok, failed, text = H.load_set("set1", {
    { name = "Arena", hash = hash("a") }, { name = "Broken", hash = hash("b") },
    { name = "Bytes", hash = hash("c") }, { name = "HSMPMenu", hash = hash("a") },
})
T.check(not ok and failed == 3, "a refused HSMP* name fails the set; 3 mods failed (" .. tostring(ok) .. " " .. tostring(failed) .. " " .. tostring(text) .. ")")
local env = H.mods[1].env
T.check(env.mine == 42 and rawget(_G, "mine") == nil and G.mine == nil, "globals stay in the mod")
T.check(env.seen.native == nil and env.seen.ipc == nil and env.seen.gnative == nil, "HSMPNative / HSMP_IPC hidden")
T.check(env.seen.shared == true and env.escape == false, "require: own folder + UEHelpers only")
T.check(H.mods[3].failed and H.mods[4].failed, "bytecode and HSMP* names refused")
-- callbacks: errors stay inside, the mod is named
calls.hooks[1][3]()
calls.hooks[1][2]()
T.check(env.hook_ran == 1, "a hook callback runs")
T.check(T.any(logs, function(l) return l:find("Arena", 1, true) and l:find("boom", 1, true) end), "an error names its mod")
-- unload
H.unload_all("test")
T.check(env.unloaded == true, "OnUnload ran")
T.check(#calls.unhooked == 1 and calls.unhooked[1][2] == 11 and calls.unhooked[1][3] == 12, "the hook is removed")
T.check(calls.unevents[1] == "Ping" and #calls.cancelled == 1, "custom event removed, loop cancelled")
calls.keys[1]()
T.check(env.key_ran == nil and calls.loop() == true, "inert key bind does nothing, the loop stops")
T.check(H.inert_left_total() == 1, "one registration left inert until a restart")
