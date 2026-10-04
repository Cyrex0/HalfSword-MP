-- hsmp_ipc.lua -- the single IPC facade of the HSMP Lua mods (shared-memory
-- transport; design in docs/development/ipc-shared-memory.md).
--
-- build-and-deploy.ps1 copies shared/*.lua into every mod's Scripts/; do not
-- edit the per-mod copies. Each mod has its own Lua state, so each gets its own
-- facade instance (caps cache, event queues, retry queue).
-- Game thread only, like every IPC call in the mods. Nothing here raises.
--
--   local IPC = require("hsmp_ipc")
--   IPC.init{ mod = "HSMPAvatars", state_dir = STATE_DIR, log = Log }
--   seq = IPC.peer_play(slot, out, last)
--
-- Shared memory is the only game<->sidecar transport. The native binding is the
-- HSMPNative global (the UE4SS C++ mod), else hsmp_lua.dll loaded with
-- package.loadlib. Without
-- either, or on an ABI / layout-hash mismatch, IPC is unavailable: every call
-- returns "nothing" (nil / 0 / false), IPC.ui_error carries the player-facing
-- reason, and nothing ever falls back to state-dir files.
--
-- IPC.use(kind) is true when the kind's capability bit is negotiated
-- (caps_effective; the game's own caps for game-local kinds such as the bus).
--
-- Every contract is a typed record: IPC.put / get / rec / peer_rec / send /
-- events / bus_put / bus_table. IPC.read / write / remove / exists are plain file helpers
-- for the mods' own config and log files (no state-dir contract is a file).

local IPC = { VERSION = 2, backend = "none", mod = "?", N = nil, S = nil, why = "not initialised" }

local function noop() end
local Log = noop

-- ---- loading siblings (deploy copies shared/*.lua next to main.lua) ----------
local function load_lib(name)
    local ok, m = pcall(require, name)
    if ok and type(m) == "table" then return m end
    local src = (debug.getinfo(1, "S").source or ""):gsub("^@", "")
    local dir = src:match("^(.*)[/\\]") or "."
    for _, p in ipairs({ dir .. "/" .. name .. ".lua", dir .. "/../../shared/" .. name .. ".lua" }) do
        local ok2, m2 = pcall(dofile, p)
        if ok2 and type(m2) == "table" then return m2 end
    end
    return nil
end
IPC.load_lib = load_lib

local S = load_lib("hsmp_ipc_schema")
IPC.S = S

-- Player-facing reasons (HSMPMenu shows IPC.ui_error on the lobby note line).
IPC.MSG_MISMATCH = "Mod and helper program versions do not match - reinstall HSMP"
IPC.MSG_NO_HELPER = "Helper program did not start - reinstall HSMP"

-- ---- small helpers ------------------------------------------------------------
-- The results pass straight through (no table.pack): every IPC call of every frame
-- goes through here.
local function called(ok, ...)
    if not ok then
        IPC.errors = (IPC.errors or 0) + 1
        if (IPC.errors or 0) <= 5 then Log("ipc: native call raised: %s", tostring((...))) end
        return nil, "raised"
    end
    return ...
end
local function call(f, ...)
    if type(f) ~= "function" then return nil, "missing" end
    return called(pcall(f, ...))
end

local function band(a, b) return ((tonumber(a) or 0) // 1 | 0) & ((tonumber(b) or 0) // 1 | 0) end

-- Load the native binding: HSMPNative -> require "hsmp_lua" -> package.loadlib.
local function find_native(o)
    local N = o.native or rawget(_G, "HSMPNative")
    if type(N) == "table" then return N, "HSMPNative" end
    local ok, m = pcall(require, "hsmp_lua")
    if ok and type(m) == "table" then return m, "hsmp_lua" end
    if package and package.loadlib then
        -- hsmp_lua.dll from its deployed location (crates/hsmp-native/README.md).
        for _, p in ipairs({ "ue4ss/Mods/HSMPNative/dlls/hsmp_lua.dll", "Mods/HSMPNative/dlls/hsmp_lua.dll" }) do
            local okl, f = pcall(package.loadlib, p, "luaopen_hsmp_lua")   -- raises in a sandboxed Lua
            if okl and f then
                local ok2, m2 = pcall(f)
                if ok2 and type(m2) == "table" then return m2, "hsmp_lua" end
            end
        end
    end
    return nil
end

-- ---- init -----------------------------------------------------------------------
-- o: mod, state_dir, log, native (tests), clock (tests)
function IPC.init(o)
    o = o or {}
    if IPC.inited and not o.reinit then
        if o.log then Log = o.log end
        if o.state_dir then IPC.state_dir = o.state_dir end
        return IPC
    end
    IPC.inited = true
    rawset(_G, "HSMP_IPC", IPC)   -- per Lua state: the shared libs (hsmp_session) route through it
    IPC.mod = tostring(o.mod or IPC.mod)
    Log = o.log or Log
    IPC.state_dir = o.state_dir or IPC.state_dir or os.getenv("HSMP_STATE_DIR") or "hsmp_state"
    IPC.clock = o.clock or IPC.clock or function() return os.clock() end
    IPC.backend, IPC.N, IPC.why, IPC.ui_error = "none", nil, nil, nil
    IPC.caps_eff, IPC.caps_game, IPC.caps_at = 0, 0, -math.huge
    IPC.last_flags, IPC.info = 0, nil
    IPC.queues, IPC.want = {}, {}
    IPC.retry = {}
    IPC.peer_cache = { at = -math.huge, by_id = {}, list = {} }
    IPC.bus_cache = {}
    IPC.stats = { sent = 0, retried = 0, dropped = 0, full = 0, polled = 0 }
    if not S then
        IPC.why, IPC.ui_error = "hsmp_ipc_schema.lua missing", IPC.MSG_MISMATCH
        Log("ipc_unavailable{reason=%q,mod=%s}", IPC.why, IPC.mod)
        return IPC
    end
    local N, src = find_native(o)
    if not N then
        IPC.why, IPC.ui_error = "no native module (HSMPNative / hsmp_lua.dll)", IPC.MSG_NO_HELPER
        Log("ipc_unavailable{reason=%q,mod=%s}", IPC.why, IPC.mod)
        return IPC
    end
    local maj, _, hash = call(N.abi)
    if tonumber(maj) ~= S.ABI_MAJOR or tostring(hash or ""):lower() ~= S.LAYOUT_HASH then
        IPC.why = string.format("abi mismatch (native %s/%s, schema %d/%s)", tostring(maj), tostring(hash), S.ABI_MAJOR, S.LAYOUT_HASH)
        IPC.ui_error = IPC.MSG_MISMATCH
        Log("ipc_refused{code=abi_mismatch,detail=%q,mod=%s}", IPC.why, IPC.mod)
        return IPC
    end
    IPC.N, IPC.backend, IPC.native_src = N, "shm", src
    -- The segment must exist before any bus key or slot is used (idempotent;
    -- HSMPMenu calls it again to get the name it passes to the sidecar).
    local name, err = call(N.ipc_open)
    if not name then
        IPC.why, IPC.ui_error = "ipc_open failed: " .. tostring(err), IPC.MSG_NO_HELPER
        Log("ipc_unavailable{reason=%q,mod=%s}", IPC.why, IPC.mod)
    else
        IPC.why = "shm via " .. src
    end
    Log("ipc: shm (%s, abi %d, layout %s) for %s", src, S.ABI_MAJOR, S.LAYOUT_HASH, IPC.mod)
    return IPC
end

function IPC.is_shm() return IPC.backend == "shm" end
function IPC.available() return IPC.backend == "shm" end

-- ---- capabilities ---------------------------------------------------------------
local RESET_FLAGS = 0
if S then RESET_FLAGS = S.FLAGS.SIDECAR_RESET | S.FLAGS.REFUSED | S.FLAGS.SIDECAR_LOST end

function IPC.refresh_info(force)
    if IPC.backend ~= "shm" then return nil end
    local now = IPC.clock()
    if not force and now - IPC.caps_at < 1.0 then return IPC.info end
    IPC.caps_at = now
    local info = call(IPC.N.ipc_info)
    if type(info) == "table" then
        IPC.info = info
        IPC.caps_eff = tonumber(info.caps_effective) or 0
        IPC.caps_game = tonumber(info.caps_game) or 0
        -- A sidecar that is not attached (absent / closing / refused) negotiates nothing.
        local st = info.sidecar_state
        if st ~= nil and st ~= "ready" and st ~= 2 then IPC.caps_eff = 0 end
        local rc = tonumber(info.refuse_code) or 0
        if rc ~= 0 and S.REFUSE[rc] == "abi_mismatch" then IPC.ui_error = IPC.MSG_MISMATCH end
    end
    return IPC.info
end

-- true when `kind` (a schema kind name, record name or record slot name) is negotiated
-- right now.
function IPC.use(kind)
    if IPC.backend ~= "shm" then return false end
    local k = S.KINDS[kind] or (S.SLOTS and S.SLOTS[kind])
    if not k and S.RECORDS and S.RECORDS[kind] then
        IPC.refresh_info(false)
        return S.RECORDS[kind].cap == 0 or band(IPC.caps_eff, S.RECORDS[kind].cap) ~= 0
    end
    if not k then return false end
    IPC.refresh_info(false)
    if k.dir == "local" then return band(IPC.caps_game, k.cap) ~= 0 end
    return band(IPC.caps_eff, k.cap) ~= 0
end

function IPC.cap(name)
    if IPC.backend ~= "shm" or not S.CAPS[name] then return false end
    IPC.refresh_info(false)
    return band(IPC.caps_eff, S.CAPS[name]) ~= 0
end

-- The sidecar is attached (state ready).
function IPC.attached()
    local info = IPC.refresh_info(false)
    return type(info) == "table" and (info.sidecar_state == "ready" or info.sidecar_state == 2)
end

-- ---- per-frame pump -------------------------------------------------------------
-- HSMPSync only: one native frame() per game frame. Other mods call IPC.flags().
function IPC.frame(world_key)
    if IPC.backend ~= "shm" then return 0 end
    local f = call(IPC.N.frame, world_key)
    f = tonumber(f) or 0
    IPC.last_flags = f
    if f & RESET_FLAGS ~= 0 then IPC.refresh_info(true) end
    return f
end

function IPC.flags()
    if IPC.backend ~= "shm" then return 0 end
    local f = tonumber((call(IPC.N.flags))) or 0
    if f & RESET_FLAGS ~= 0 and f ~= IPC.last_flags then IPC.refresh_info(true) end
    IPC.last_flags = f
    return f
end

function IPC.open()
    if IPC.backend ~= "shm" then return nil, IPC.why end
    return call(IPC.N.ipc_open)
end

function IPC.world_leaving()
    if IPC.backend ~= "shm" then return false end
    IPC.bus_cache = {}
    return call(IPC.N.world_leaving) and true or false
end

function IPC.world_ready(key)
    if IPC.backend ~= "shm" then return false end
    return call(IPC.N.world_ready, key) and true or false
end

-- ---- typed writes ---------------------------------------------------------------
function IPC.put_root(...) if IPC.backend ~= "shm" then return nil, "unavailable" end return call(IPC.N.put_root, ...) end
function IPC.put_weapon(...) if IPC.backend ~= "shm" then return nil, "unavailable" end return call(IPC.N.put_weapon, ...) end
function IPC.put_pose(...) if IPC.backend ~= "shm" then return nil, "unavailable" end return call(IPC.N.put_pose, ...) end
function IPC.put_lead(ms) if IPC.backend ~= "shm" then return nil, "unavailable" end return call(IPC.N.put_lead, ms) end
-- A refused write (too_big / bad) silently loses data: log it once per kind and reason.
IPC.refused = {}
local function note_refused(op, kind, err)
    if err ~= "too_big" and err ~= "bad" then return end
    local key = op .. ":" .. tostring(kind) .. ":" .. tostring(err)
    local n = (IPC.refused[key] or 0) + 1
    IPC.refused[key] = n
    if n == 1 or n == 100 or n == 1000 then
        Log("ipc_write_refused{op=%s,kind=%s,err=%s,n=%d,mod=%s}", op, tostring(kind), tostring(err), n, tostring(IPC.mod))
    end
end
function IPC.put(kind, t)
    if IPC.backend ~= "shm" then return nil, "unavailable" end
    local ok, err = call(IPC.N.put, kind, t)
    if not ok then note_refused("put", kind, err) end
    return ok, err
end

-- ---- typed reads ----------------------------------------------------------------
function IPC.peers(out)
    if IPC.backend ~= "shm" then return 0 end
    return tonumber((call(IPC.N.peers, out))) or 0
end
function IPC.peer_play(slot, out, last)
    if IPC.backend ~= "shm" then return nil end
    return (call(IPC.N.peer_play, slot, out, last))
end
function IPC.peer(kind, slot, last, out)
    if IPC.backend ~= "shm" then return nil end
    return call(IPC.N.peer, kind, slot, last, out)
end

-- ---- typed records (ABI 2) --------------------------------------------------------
-- get(slot, last, out) -> ver, t | ver (unchanged since `last`) | nil (never written).
-- `slot` is a record slot name (S.SLOTS); `out` (optional) is a reused table filled in place.
function IPC.get(slot, last, out)
    if IPC.backend ~= "shm" then return nil end
    return call(IPC.N.get, slot, last, out)
end

-- The current table of a record slot, cached per version in this Lua state (one decode per
-- change). nil = never written (or IPC unavailable).
IPC.rec_cache = {}
function IPC.rec(slot)
    local c = IPC.rec_cache[slot]
    if not c then c = {}; IPC.rec_cache[slot] = c end
    if IPC.backend ~= "shm" then return nil end
    local ver, t = IPC.get(slot, c.ver)
    if ver == nil then
        c.ver, c.t = nil, nil
    elseif ver ~= c.ver then
        c.ver, c.t = ver, t
    end
    return c.t, c.ver
end

-- A peer's record slot (S.SLOTS form peer_slot / peer_blob) by peer id, cached per version.
IPC.peer_rec_cache = {}
function IPC.peer_rec(slot, peer_id)
    local key = slot .. ":" .. tostring(peer_id)
    local ps, e = IPC.peer_slot(peer_id)
    local c = IPC.peer_rec_cache[key]
    if ps == nil then IPC.peer_rec_cache[key] = nil; return nil end
    if c and c.slot_gen ~= (e and e.gen) then c = nil end
    local ver, t = IPC.peer(slot, ps, c and c.ver or nil)
    if ver ~= nil and type(t) == "table" then
        c = { ver = ver, t = t, slot_gen = e and e.gen }
        IPC.peer_rec_cache[key] = c
    end
    return c and c.t or nil
end
function IPC.bus_put(key, t)
    if IPC.backend ~= "shm" then return nil, "unavailable" end
    return call(IPC.N.bus_put, key, t)
end
function IPC.bus_get(key, last, out)
    if IPC.backend ~= "shm" then return nil end
    return call(IPC.N.bus_get, key, last, out)
end

-- ---- DevCtl and processes -------------------------------------------------------------
-- dev_poll(max, out) -> n; out[i] = {kind = "dev_cmd", data = t} (each command once per Lua state).
function IPC.dev_poll(max, out)
    if IPC.backend ~= "shm" then return 0 end
    return tonumber((call(IPC.N.dev_poll, max, out))) or 0
end
-- spawn(exe, args, opts{cwd, env, hide}) -> pid | nil, err (CreateProcessW; the module keeps the handle).
function IPC.spawn(exe, args, opts)
    if not IPC.N then return nil, "unavailable" end
    return call(IPC.N.spawn, exe, args, opts)
end
-- true only for a process this module spawned that is still running.
function IPC.proc_alive(pid)
    if not IPC.N then return false end
    return call(IPC.N.proc_alive, pid) == true
end
-- exit code | nil, "running" | nil, "not_ours"
function IPC.proc_exit_code(pid)
    if not IPC.N then return nil, "unavailable" end
    return call(IPC.N.proc_exit_code, pid)
end
-- Kill a process we spawned (its whole job): true | nil, "not_ours" | nil, "gone".
function IPC.proc_kill(pid)
    if not IPC.N then return nil, "unavailable" end
    return call(IPC.N.proc_kill, pid)
end
-- spawn_capture(exe, args, opts?) -> h | nil, err (stdout + stderr through a pipe).
function IPC.spawn_capture(exe, args, opts)
    if not IPC.N then return nil, "unavailable" end
    return call(IPC.N.spawn_capture, exe, args, opts)
end
-- capture_poll(h, wait_ms?) -> false (running) | true, exit_code, output (then released) | nil, "bad".
-- wait_ms > 0 blocks up to that long (native cap 5 s) for the child to exit.
function IPC.capture_poll(h, wait_ms)
    if not IPC.N then return nil, "unavailable" end
    return call(IPC.N.capture_poll, h, wait_ms)
end
function IPC.current_pid()
    if not IPC.N then return nil end
    return (call(IPC.N.current_pid))
end

-- ---- native sampling (cap NATIVE_SAMPLE) ----------------------------------------------
-- sample_config(cfg) -> true | nil, err; sample_local(a) -> mask | mask, err | nil, err;
-- sample_status() -> table. Contract: crates/hsmp-native/src/sample.rs.
function IPC.sample_config(cfg)
    if IPC.backend ~= "shm" then return nil, "unavailable" end
    return call(IPC.N.sample_config, cfg)
end
function IPC.sample_local(a)
    if IPC.backend ~= "shm" then return nil, "unavailable" end
    return call(IPC.N.sample_local, a)
end
function IPC.sample_status()
    if IPC.backend ~= "shm" then return nil end
    return (call(IPC.N.sample_status))
end
-- ---- native stand-in body servo (cap NATIVE_SERVO) ------------------------------------
-- servo_config({bones}) -> true | nil, err; servo_bodies(a, out) -> n | nil, err;
-- servo_status() -> table. Contract: crates/hsmp-native/src/servo.rs.
function IPC.servo_config(cfg)
    if IPC.backend ~= "shm" then return nil, "unavailable" end
    return call(IPC.N.servo_config, cfg)
end
function IPC.servo_bodies(a, out)
    if IPC.backend ~= "shm" then return nil, "unavailable" end
    return call(IPC.N.servo_bodies, a, out)
end
-- servo_weapon(a, out) -> true | nil, err (one held weapon of a stand-in).
function IPC.servo_weapon(a, out)
    if IPC.backend ~= "shm" then return nil, "unavailable" end
    return call(IPC.N.servo_weapon, a, out)
end
function IPC.servo_status()
    if IPC.backend ~= "shm" then return nil end
    return (call(IPC.N.servo_status))
end
-- neutralise_config(cfg) -> true | nil, err; neutralise(a) -> writes | nil, err;
-- neutralise_status() -> table. Contract: crates/hsmp-native/src/neutralise.rs.
function IPC.neutralise_config(cfg)
    if IPC.backend ~= "shm" then return nil, "unavailable" end
    return call(IPC.N.neutralise_config, cfg)
end
function IPC.neutralise(a)
    if IPC.backend ~= "shm" then return nil, "unavailable" end
    return call(IPC.N.neutralise, a)
end
function IPC.neutralise_status()
    if IPC.backend ~= "shm" then return nil end
    return (call(IPC.N.neutralise_status))
end


-- A bus key's current record table (cached per generation in this Lua state), or nil (never
-- written). Every bus key is a typed record (S.SLOTS form "bus"): its cleared
-- form (IPC.bus_clear, a world leave) is the zeroed record, which each reader treats as
-- absent (seq 0, state "", target 0, no rows, ...).
function IPC.bus_table(key)
    local c = IPC.bus_cache[key]
    if not c then c = { gen = nil, t = nil }; IPC.bus_cache[key] = c end
    if IPC.backend ~= "shm" then return nil end
    local gen, t = IPC.bus_get(key, c.gen)
    if gen ~= nil and gen ~= c.gen then
        c.gen = gen
        c.t = type(t) == "table" and t or nil
    end
    return c.t, c.gen
end

-- Clear a bus key: its zeroed record.
function IPC.bus_clear(key)
    IPC.bus_cache[key] = nil
    return IPC.bus_put(key, {})
end

-- Peer directory, cached (refreshed on PEERS_CHANGED or every 0.25 s).
-- Returns { by_id = {[id] = entry}, list = {entry...} }; entry = {id, slot, gen, nick, ...}.
function IPC.peer_dir(force)
    local c = IPC.peer_cache
    if IPC.backend ~= "shm" then return c end
    local now = IPC.clock()
    local changed = (IPC.last_flags & ((S and S.FLAGS.PEERS_CHANGED) or 0)) ~= 0
    if not force and not changed and now - c.at < 0.25 then return c end
    c.at = now
    c.out = c.out or {}
    local n = IPC.peers(c.out)
    c.by_id, c.list = {}, {}
    for i = 1, n do
        local e = c.out[i]
        if type(e) == "table" and e.id then
            c.list[#c.list + 1] = e
            c.by_id[math.tointeger(e.id) or e.id] = e
        end
    end
    return c
end

function IPC.peer_slot(id)
    local e = IPC.peer_dir().by_id[math.tointeger(tonumber(id)) or id]
    return e and e.slot, e
end

-- ---- G2S messages with a bounded retry queue ------------------------------------
IPC.RETRY_MAX = 256
local function send_now(kind, t)
    local id, err = call(IPC.N.send, kind, t)
    if not id then note_refused("send", kind, err) end
    return id, err
end

-- Retry queued sends in order; true when nothing is left.
function IPC.flush()
    local q = IPC.retry
    if #q == 0 or IPC.backend ~= "shm" then return #q == 0 end
    local i = 1
    while i <= #q do
        local id, err = send_now(q[i].kind, q[i].t)
        if id then
            IPC.stats.retried = IPC.stats.retried + 1
            i = i + 1
        elseif err == "full" or err == "raised" or err == "cap" then
            break   -- cap: the sidecar is not attached (yet); keep the order
        else
            IPC.stats.dropped = IPC.stats.dropped + 1   -- bad / too_big: never succeeds
            i = i + 1
        end
    end
    if i > 1 then
        local rest = {}
        for k = i, #q do rest[#rest + 1] = q[k] end
        IPC.retry, q = rest, rest
    end
    if #q == 0 and IPC.backpressure then
        IPC.backpressure = false
        Log("ipc_backpressure{ring=g2s,state=cleared,mod=%s}", IPC.mod)
    end
    return #q == 0
end

local function enqueue(kind, t)
    local q = IPC.retry
    if #q >= IPC.RETRY_MAX then table.remove(q, 1); IPC.stats.dropped = IPC.stats.dropped + 1 end
    q[#q + 1] = { kind = kind, t = t }
end

-- Send one G2S message. Returns req_id, or true when queued for retry (ring
-- full / sidecar not attached yet), or nil, err (bad / too_big / unavailable).
function IPC.send(kind, t)
    if IPC.backend ~= "shm" then return nil, "unavailable" end
    if not IPC.flush() then enqueue(kind, t); return true end
    local id, err = send_now(kind, t)
    if id then IPC.stats.sent = IPC.stats.sent + 1; return id end
    if err == "full" or err == "cap" then
        if err == "full" then
            IPC.stats.full = IPC.stats.full + 1
            if not IPC.backpressure then
                IPC.backpressure = true
                Log("ipc_backpressure{ring=g2s,state=full,mod=%s}", IPC.mod)
            end
        end
        enqueue(kind, t)
        return true
    end
    return nil, err
end

function IPC.pending() return #IPC.retry end

-- ---- S2G events -----------------------------------------------------------------
-- Raw passthrough (this Lua state's cursor).
function IPC.poll(max, out)
    if IPC.backend ~= "shm" then return 0 end
    return tonumber((call(IPC.N.poll, max, out))) or 0
end

IPC.QUEUE_MAX = 1024
-- Drain this state's cursor into per-kind queues for the kinds asked for with
-- IPC.events(kind). A "resync" record sets IPC.resync (the mod re-reads state).
function IPC.pump()
    if IPC.backend ~= "shm" or next(IPC.want) == nil then return end
    local out = IPC.poll_out or {}
    IPC.poll_out = out
    for _ = 1, 64 do
        local n = IPC.poll(64, out)
        for i = 1, n do
            local r = out[i]
            if type(r) == "table" then
                if r.kind == "resync" then
                    IPC.resync = true
                    Log("ipc_resync{reason=cursor_behind,mod=%s}", IPC.mod)
                elseif IPC.want[r.kind] then
                    local q = IPC.queues[r.kind]
                    if #q >= IPC.QUEUE_MAX then table.remove(q, 1) end
                    q[#q + 1] = { req_id = r.req_id, data = r.data, peer = r.peer, aux = r.aux }
                end
                IPC.stats.polled = IPC.stats.polled + 1
            end
        end
        if n < 64 then break end
    end
end

-- Drain the queued events of `kind` (array of {req_id, data, peer, aux}; peer / aux are the
-- record's ring header fields).
function IPC.events(kind)
    if not IPC.want[kind] then
        IPC.want[kind] = true
        IPC.queues[kind] = IPC.queues[kind] or {}
        local list = {}
        for k in pairs(IPC.want) do list[#list + 1] = k end
        table.sort(list)
        call(IPC.N and IPC.N.subscribe, list)
    end
    IPC.pump()
    local q = IPC.queues[kind]
    if #q == 0 then return q end
    IPC.queues[kind] = {}
    return q
end

-- ---- plain file helpers (the mods' own config and log files; no IPC contract) ----------
-- Read: content string (the first line when line_only), false = absent.
function IPC.read(path, line_only)
    local f = io.open(path, "rb")
    if not f then return false end
    local s
    if line_only then s = f:read("l") else s = f:read("a") end
    f:close()
    return s or ""
end

-- Write whole: tmp + rename (one in-place write if the rename fails). True on success.
function IPC.write(path, s)
    local tmp = path .. ".tmp"
    local f = io.open(tmp, "wb")
    if not f then return false end
    f:write(s)
    f:close()
    if os.rename(tmp, path) then return true end
    os.remove(path)
    if os.rename(tmp, path) then return true end
    local g = io.open(path, "wb")
    if g then g:write(s); g:close() end
    os.remove(tmp)
    return g ~= nil
end

function IPC.remove(path)
    return os.remove(path) and true or false
end

function IPC.exists(path)
    local f = io.open(path, "rb")
    if f then f:close(); return true end
    return false
end

return IPC
