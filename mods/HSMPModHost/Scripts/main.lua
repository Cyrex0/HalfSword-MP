-- HSMPModHost -- runs the server mods the player accepted (docs/hosting/server-mods.md).
--
-- The sidecar downloads a server's mods into the verified cache (hsmp_cfg mods_cache_dir,
-- `hsmp_mods` next to the game, outside ue4ss/Mods) only after the player accepted them in
-- HSMPMenu. When it reports the set READY (`mod_progress`), this mod loads every mod of the
-- set from `<cache>/<mod hash>/Scripts/main.lua` (modhost.lua: own environment, tracked
-- registrations) and answers `mod_loaded`; the sidecar then lets the player into the session.
-- The set is unloaded when the session ends (the sidecar is gone or ended, or the player was
-- kicked / rejected / replaced / the server closed), when another server offers another set,
-- and on `mod_progress` CLEAR / FAILED.
--
-- This mod's name starts with HSMP, so its Lua state has the native API; the server mods
-- share that state but `HSMPNative` and `HSMP_IPC` are hidden from them. That is tidiness,
-- not a security boundary: the player accepted code with full access (SECURITY.md).

-- Thread-safety shim (same as every HSMP mod; docs/development/lua-mods.md 5.1). Server mods
-- get the rerouted LoopAsync / ExecuteWithDelay / key binds through their environment.
if LoopInGameThreadWithDelay and ExecuteInGameThreadWithDelay and CancelDelayedAction then
    ExecuteWithDelay = function(ms, fn) return ExecuteInGameThreadWithDelay(ms, fn) end
    LoopAsync = function(ms, fn)
        local h
        h = LoopInGameThreadWithDelay(ms, function()
            local ok, stop = pcall(fn)
            if ok and stop == true and h then CancelDelayedAction(h) end
        end)
        return h
    end
    local kq, kq_w, kq_r, kq_loop = {}, 0, 0, nil
    local function kq_wrap(fn)
        if not kq_loop then
            kq_loop = LoopInGameThreadWithDelay(16, function()
                while kq_r < kq_w do
                    kq_r = kq_r + 1
                    local f = kq[kq_r]
                    kq[kq_r] = nil
                    if f then pcall(f) end
                end
            end)
        end
        return function() local n = kq_w + 1; kq[n] = fn; kq_w = n end
    end
    local _rkba = RegisterKeyBindAsync
    RegisterKeyBindAsync = function(key, mods, fn) return _rkba(key, mods, kq_wrap(fn)) end
    local _rkb = RegisterKeyBind
    RegisterKeyBind = function(key, a, b)
        if b then return _rkb(key, a, kq_wrap(b)) end
        return _rkb(key, kq_wrap(a))
    end
end

local function Log(fmt, ...) print(string.format("[HSMPModHost] " .. fmt .. "\n", ...)) end

local SCRIPT_DIR = ((debug.getinfo(1, "S").source or ""):gsub("^@", "")):match("^(.*)[/\\]") or "."
local function load_module(name)
    local ok, m = pcall(require, name)
    if ok and type(m) == "table" then return m end
    for _, p in ipairs({ SCRIPT_DIR .. "/" .. name .. ".lua", SCRIPT_DIR .. "/../../shared/" .. name .. ".lua" }) do
        local ok2, m2 = pcall(dofile, p)
        if ok2 and type(m2) == "table" then return m2 end
    end
    return nil
end

local Cfg = load_module("hsmp_cfg")
local STATE_DIR = Cfg and Cfg.state_dir() or "hsmp_state"
local CACHE = (Cfg and Cfg.mods_cache_dir and Cfg.mods_cache_dir()) or "hsmp_mods"
-- The world guard: this mod keeps no UObject; server mods bring their own.
local WG = load_module("hsmp_wg")
if WG and WG.new then pcall(WG.new, { log = Log }) end
local HL = load_module("hsmp_log")
if HL and HL.init then pcall(HL.init, { mod = "HSMPModHost", state_dir = STATE_DIR }) end
local function ev(name, fields)
    if HL and HL.event then pcall(HL.event, name, fields or {}) end
end
do
    local m = load_module("hsmp_ipc")
    if m then m.init({ mod = "HSMPModHost", state_dir = STATE_DIR, log = Log }) end
end
local IPC = rawget(_G, "HSMP_IPC")
local HS = load_module("hsmp_session")
local Host = load_module("modhost")
if not (IPC and Host) then
    Log("disabled: %s", IPC and "modhost.lua missing" or "shared/hsmp_ipc.lua missing")
    return
end

local H = Host.new({
    G = _G,
    cache = CACHE,
    read = function(p)
        local f = io.open(p, "rb")
        if not f then return nil end
        local s = f:read("a"); f:close()
        return s
    end,
    log = Log,
    event = function(name, f) ev(name, f) end,
})

local function hex(t)
    if type(t) ~= "table" then return "" end
    local o = {}
    for i = 1, 32 do o[i] = string.format("%02x", (tonumber(t[i]) or 0) & 0xff) end
    return table.concat(o)
end

local S = IPC.S
local ST = S and S.ENUMS.mod_state or {}
local TERMINAL = { ended = true, kicked = true, rejected = true, replaced = true, server_closed = true }

-- set hex -> { raw = set_hash table, n, entries = { [index + 1] = { name, hash } } }
local sets = {}
local ready = nil          -- set hex waiting for its entries
local resend_at = 0
local attached_seen, detached_since = false, nil

local function forget_old_sets(keep)
    for k in pairs(sets) do if k ~= keep and k ~= H.set then sets[k] = nil end end
end

local function try_load(set)
    local s = sets[set]
    if not s or not s.n then return false end
    local list = {}
    for i = 1, s.n do
        if not s.entries[i] then return false end
        list[i] = s.entries[i]
    end
    local ok, failed, text = H.load_set(set, list)
    Log("set %s: %d mod(s), ok=%s failed=%d %s", set:sub(1, 12), #list, tostring(ok), failed, text)
    ev("x_server_mods_loaded", { set = set:sub(1, 16), mods = #list, ok = ok, failed = failed })
    IPC.send("mod_loaded", { set_hash = s.raw, ok = ok, failed = math.min(failed, 255), text = text })
    return true
end

local function unload(why)
    if H.set or #H.mods > 0 then
        H.unload_all(why)
        ev("x_server_mods_unloaded", { why = why, inert = H.inert_left_total() })
    end
    ready = nil
end

local function pump()
    for _, e in ipairs(IPC.events("mod_offer")) do
        local d = e.data or {}
        local set = hex(d.set_hash)
        sets[set] = sets[set] or { raw = d.set_hash, entries = {} }
        sets[set].n = tonumber(d.n) or 0
        forget_old_sets(set)
    end
    for _, e in ipairs(IPC.events("mod_entry")) do
        local d = e.data or {}
        local set = hex(d.set_hash)
        local s = sets[set] or { raw = d.set_hash, entries = {} }
        sets[set] = s
        s.n = s.n or tonumber(d.n)
        s.entries[(tonumber(d.index) or 0) + 1] = { name = tostring(d.name or ""), hash = hex(d.mod_hash) }
    end
    for _, e in ipairs(IPC.events("mod_progress")) do
        local d = e.data or {}
        local set, st = hex(d.set_hash), tonumber(d.state)
        if st == ST.READY then
            if H.set ~= set then ready = set; resend_at = 0 end
            if H.set == set then IPC.send("mod_loaded", { set_hash = d.set_hash, ok = true, failed = H.failed or 0, text = "" }) end
        elseif st == ST.CLEAR or st == ST.FAILED then
            unload(st == ST.CLEAR and "the server has no mods" or "the mods failed")
        elseif st == ST.OFFER and H.set and H.set ~= set then
            unload("another server's mods")
        end
    end
    if ready then
        if try_load(ready) then
            ready = nil
        elseif os.clock() >= resend_at then
            -- the entries were missed (a cursor fell behind): ask the sidecar for the offer again
            resend_at = os.clock() + 1
            local s = sets[ready]
            if s and s.raw then IPC.send("mod_decision", { set_hash = s.raw, op = S.ENUMS.mod_op.RESEND }) end
        end
    end
end

-- The session ended: the mods go.
local function watch_session()
    if not H.set then return end
    local att = IPC.attached()
    if att then attached_seen, detached_since = true, nil
    elseif attached_seen then
        detached_since = detached_since or os.clock()
        if os.clock() - detached_since > 3 then unload("the helper program stopped"); attached_seen = false end
    end
    local lk = HS and HS.link and HS.link() or nil
    local st = lk and HS.status_name(lk) or nil
    if st and TERMINAL[st] then unload("session " .. st) end
end

-- Subscribe now, before the sidecar can push anything.
IPC.events("mod_offer"); IPC.events("mod_entry"); IPC.events("mod_progress")

LoopAsync(100, function()
    local ok, err = pcall(function() pump(); watch_session() end)
    if not ok then Log("tick failed: %s", tostring(err)) end
    return false
end)

HSMP_MODHOST_TEST = { host = H, pump = pump, watch = watch_session, sets = sets }
Log("loaded (cache %s)", CACHE)
