-- HSMPAvatars: makes each connected peer visible in your world.
--
-- A peer is shown by PUPPETING one of the arena's natively spawned foe
-- Willies (fully booted by the game: armour, weapon, physical animation)
-- instead of raw-spawning a Willie_BP_C: raw spawns skip the native init
-- chain and come out naked and T-posed. The foe's AI_BP_C controller is
-- detached and the stand-in is driven from the peer's replicated state.
--
-- Data path (docs/development/subsystems/replication.md):
--   the sender's HSMPSync samples root + codec-v2 bones + held weapons in one
--   game frame (60 Hz default) -> its sidecar -> server relay -> our sidecar,
--   whose jitter buffer (crates/hsmp-pose/src/poseplay.rs) interpolates on
--   its own clock and publishes each peer's pose_play record in shared memory
--   (HSMPNative.peer_play, read through shared/hsmp_ipc.lua).
--   This mod reads it EVERY GAME FRAME and drives the stand-in with a
--   velocity servo (drive_v2): every body gets the velocity that lands it on
--   the replicated pose at the end of the next physics step. The older
--   PhysicsHandle driver (one handle per hero bone, stiffer on hands) is the
--   fallback for codec-v1 data.
--   The peer_root record slot (relayed root snapshots) is read for peer liveness.
--
-- While driven, the stand-in's own muscle logic is blocked ("Block * Muscles",
-- PhysicalAnimation strength 0) so nothing pulls it toward an idle pose. A
-- "cut" (teleport / respawn / round reset) or a sustained large pelvis error
-- snaps the whole mesh rigidly onto the target.
--
-- Claim priority for a peer without a puppet:
--   1. an AI-controlled native foe (detach its AI)
--   2. an idle, uncontrolled, living Willie (a foe we already neutralised, or
--      the puppet of a peer that left)
--   3. last resort: ask BP_LevelManager "Spawn Combatants" for more natively
--      booted foes (rate-limited; our own pawn is re-possessed if the call
--      steals it)
-- Dead (DED) or dismembered Willies are never claimed. Once every live peer
-- has a puppet, any remaining AI foe is neutralised (AI detached, hidden).
--
-- Half Sword BP property names contain spaces ("Weapon R", "Block Arm
-- Muscles"); CamelCase reads silently return nil. See
-- docs/development/halfsword/willie_props.txt.

local UEHelpers = require("UEHelpers")

-- Thread-safety shim. LoopAsync, ExecuteWithDelay and *Async keybind
-- callbacks run on UE4SS worker threads; running them alongside game-thread
-- Lua corrupts the mod's VM (crashes in lua_next / __index on garbage
-- userdata). Route every deferred, looped and key-bound callback onto
-- the game thread.
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
    -- Key callbacks run on the UE4SS input thread. Native-function hooks run
    -- on the game thread on this mod's hook state without UE4SS's lock, so an
    -- ExecuteInGameThread call from a key callback pushed onto that state
    -- mid-hook (crash in push_structproperty, 2026-10-03). The input thread
    -- now only appends to a queue (plain Lua, no UE4SS call) and a
    -- game-thread loop runs what it queued.
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

local function Log(fmt, ...)
    print(string.format("[HSMPAvatars] " .. fmt .. "\n", ...))
end

local function trim(s) return s and s:match("^%s*(.-)%s*$") or nil end

local STATE_DIR     = (trim(os.getenv("HSMP_STATE_DIR")) or "hsmp_state"):gsub("\\", "/")
local SETTINGS_FILE = STATE_DIR .. "/.settings.json"

-- Structured events (shared/hsmp_log.lua; no-op when it is not deployed).
local function load_module(name)
    local ok, mod = pcall(require, name)
    if ok and type(mod) == "table" then return mod end
    local src = (debug.getinfo(1, "S").source or ""):gsub("^@", "")
    local dir = src:match("^(.*)[/\\]") or "."
    local paths = { dir .. "/" .. name .. ".lua", dir .. "/../../shared/" .. name .. ".lua" }
    if name == "standin_body" then paths[#paths+1] = dir .. "/../../HSMPCombat/Scripts/standin_body.lua" end
    for _, path in ipairs(paths) do
        local ok2, mod2 = pcall(dofile, path)
        if ok2 and type(mod2) == "table" then return mod2 end
    end
    return nil
end
local HL = load_module("hsmp_log")
local function ev(name, fields)
    if not HL then return end
    local f = HL.event or HL.emit
    if f then pcall(f, name, fields or {}) end
end
if HL and HL.init then pcall(HL.init, { mod = "HSMPAvatars", state_dir = STATE_DIR }) end
-- Shared-memory IPC facade (shared/hsmp_ipc.lua). No new local (the main chunk is at
-- the 200-locals limit): IPC.init publishes it as the per-state global HSMP_IPC.
do
    local m = load_module("hsmp_ipc")
    if m then m.init({ mod = "HSMPAvatars", state_dir = STATE_DIR, log = Log }) end
end

local TICK_MS        = 33    -- this mod's own Lua loop on the game thread, not the server tick
local MAX_PEER_ID    = 8     -- probe peer ids 1..8 (server max_peers default)
local PEERS_EVERY    = 15    -- re-read the roster + probe ids every ~0.5 s
local CLAIM_RETRY    = 30    -- retry claiming a puppet every ~1 s
local STALE_TICKS    = 150   -- a peer is gone after ~5 s with no new snapshot
local NEUTRAL_EVERY  = 30    -- scan for loose AI foes every ~1 s
local SPAWN_COOLDOWN = 120   -- SpawnCombatants fallback at most every ~4 s (> the 3 s possession guard)
local SPAWN_COOLDOWN_LIVE = 240   -- ~8 s while the round is Live (each call swaps possession for a moment)
local SPAWN_MAX      = 3     -- ... and at most this many times per level, plus one per live peer
local TELEPORT_DIST  = 150   -- larger capsule jumps reset physics velocity

-- ---- pose tunables (see docs/development/subsystems/replication.md) --------
local PLAY_STALE_MS   = 500    -- no new pose_play seq for this long -> release
local POSE_RANGE      = 4000   -- only pose-drive stand-ins this close (cost bound)
local SNAP_BODY_ERR   = 150    -- pelvis error (uu) that triggers a rigid mesh snap
local SNAP_BODY_MS    = 150    -- ... sustained for this long
local BLOCK_MUSCLES   = true   -- block the stand-in's own muscles while driven
local NOSIM_FIX_MS    = 1500   -- stand-in with no simulated bodies this long -> enable physics

-- PhysicsHandle tuning (soft constraints, UE PhysicsHandle units). Hands and
-- the weapon are stiffest so swings land where the attacker swung; the
-- torso/legs are softer so contact and gravity still look physical.
-- `.settings.json` "pose_stiffness" (0.25..4, default 1) scales all of them live.
local PH_BODY = { lin_k = 12000, lin_d = 400, ang_k = 12000, ang_d = 400 }
local PH_ARM  = { lin_k = 25000, lin_d = 600, ang_k = 25000, ang_d = 600 }
local PH_HAND = { lin_k = 40000, lin_d = 900, ang_k = 40000, ang_d = 900 }
local PH_WPN  = { lin_k = 40000, lin_d = 900, ang_k = 50000, ang_d = 1000 }

-- Velocity-steering fallback (only for bones a PhysicsHandle can't grab).
local POSE_K_POS        = 18.0   -- 1/s
local POSE_K_ROT        = 14.0   -- 1/s
local POSE_MAX_LIN      = 4000   -- uu/s clamp
local POSE_MAX_ANG      = 1440   -- deg/s clamp

local POSE_DIAG_MS      = 5000

-- Codec-v2 velocity servo (replication.md "Driver"): physics stays
-- on, every body gets the velocity that lands it on the replicated pose at
-- the end of the next physics step. The part of that velocity beyond the
-- sender's own body motion is capped so a stand-in held back by contact
-- catches up gently instead of hitting like a hammer.
local SERVO_CAP_LIN   = 900     -- uu/s beyond the replicated velocity
local SERVO_CAP_ANG   = 900     -- deg/s beyond the replicated angular velocity
-- Impact yield: a body the solver left IMPACT_DV uu/s off its commanded velocity was struck
-- (blade, body, prop), not servo-tracked: ease the stand-in for IMPACT_MS so the native contact
-- response shows, then servo back onto the owner's pose (which by then carries the owner's own
-- reaction). Without it a stand-in shrugs off every blow within one physics step. First
-- guesses, not measured: tune impact_dv / impact_ms (impact_dv 0 = off).
local IMPACT_DV       = 300
local IMPACT_MS       = 200
local IMPACT_GAIN     = 0.08
local IMPACT_CAP      = 200
local SERVO_GAIN      = 0.3     -- share of the remaining error corrected per step (1 = deadbeat; measured A/B under motion: 1.0 -> jitter 2-9, 0.7 -> 1.4-2.0, 0.4 -> 1.1-1.3, 0.3 -> 1.0-1.2 at the same 0.2-0.6 uu arm error; 0.2 lets arm error reach 3-9 uu)
local SERVO_EXTRAP_MS = 120     -- advance a target by its velocity at most this long (v2 aim at 20 fps: ~2 x 50 ms past the sample)
local HOLD_RELEASE_MS = 5000    -- stale data: hold the last pose this long, then release
local POSE_PROBE      = (os.getenv("HSMP_POSE_PROBE") or "") == "1"
local TUNE            = { key = "" }   -- dev knobs (dev_cmd TUNE), see PX.poll_dev

-- Codec-v1 hero bones; must match WIRE_BONES in crates/hsmp-pose/src/posecodec.rs.
local HERO_BONES = {
    "Pelvis", "Spine_02", "Spine_04", "Head",
    "Upperarm_L", "Lowerarm_L", "Hand_L",
    "Upperarm_R", "Lowerarm_R", "Hand_R",
    "Thigh_L", "Calf_L", "Foot_L",
    "Thigh_R", "Calf_R", "Foot_R",
}
local ARM_BONES = {
    Upperarm_L = true, Lowerarm_L = true, Hand_L = true,
    Upperarm_R = true, Lowerarm_R = true, Hand_R = true,
}
local HAND_BONES = { Hand_L = true, Hand_R = true }
-- Real (space-containing) Willie_BP_C property names, willie_props.txt.
local MUSCLE_FLAGS = {
    "Block Body Muscles", "Block Upper Leg Muscles", "Block Lower Leg Muscles",
    "Block Shoulder Muscles", "Block Arm Muscles",
}
-- Stand-in BP reaction state zeroed every driven frame (Willie_BP_C names).
-- Not "Pain" itself: HSMPCombat measures it (FIELDS #13) around a local hit.
local PAIN_ZERO = { "Pain Shock Rate", "Pain Upper Body", "Pain Lower Body", "Pain Neck", "Pain Head",
    "Pain Arm R", "Pain Arm L", "Pain Leg R", "Pain Leg L", "Pain Grab Rate", "Pain L Arm Alpha", "Pain R Arm Alpha" }
local PAIN_FALSE = { "Pain Shock", "Bend Over Extreme" }
-- Bodies the in-game smoothness metrics look at (v2 slot -> kind).
local QBODY = { [1] = "pelvis", [9] = "head", [13] = "hand", [17] = "hand", [20] = "foot", [23] = "foot" }
-- pose_quality windows: planted-foot slide over >= FOOT_WIN_S of a
-- planted run; idle = owner pelvis + hands slower than IDLE_SPD uu/s, judged
-- over IDLE_MIN..IDLE_WIN frames.
-- (one table: the main chunk is at Lua's 200-locals limit)
local PQ = { FOOT_WIN_S = 0.1, IDLE_SPD = 10, IDLE_MIN = 30, IDLE_WIN = 120, JIT_SPD = 20, JIT_MIN_N = 10,
    FEET = { 20, 23 }, PLANT_SPD = 5, PLANT_CREEP = 3, PLANT_MAX_ERR = 15 }   -- foot planting (drive_v2)
-- Is the round Live? (bus key "director", state "Live", written by the
-- Director every second and on every state change; re-read at most every
-- 0.25 s). gen counts Live windows, so each one starts a clean sample.
PQ.live_st = { live = false, at = -1e9, gen = 0 }
function PQ.live()
    local Q_LIVE = PQ.live_st
    local t = os.clock()
    if t - Q_LIVE.at < 0.25 then return Q_LIVE.live end
    Q_LIVE.at = t
    -- The Director's heartbeat (bus key "director").
    local d = HSMP_IPC and HSMP_IPC.bus_table("director")
    if type(d) ~= "table" or d.state == "" then return Q_LIVE.live end   -- absent / cleared: keep
    local live = d.state == "Live"
    if live and not Q_LIVE.live then Q_LIVE.gen = Q_LIVE.gen + 1 end
    Q_LIVE.live = live
    return live
end
-- RMS about the mean of the accumulated tracking errors (worst of the slots).
function PQ.idle_window_rms(iw)
    if not iw or iw.n < PQ.IDLE_MIN then return nil end
    local v
    for _, s in ipairs({ 1, 13, 17 }) do
        local a = iw[s]
        if a then
            local m2 = (a[1] / iw.n) ^ 2 + (a[2] / iw.n) ^ 2 + (a[3] / iw.n) ^ 2
            v = math.max(v or 0, math.sqrt(math.max(0, a[4] / iw.n - m2)))
        end
    end
    return v
end
local _fnames = {}
local function fname(s)
    local f = _fnames[s]
    if not f then f = FName(s); _fnames[s] = f end
    return f
end
local function now_ms() return os.clock() * 1000 end   -- MSVC clock(): wall ms

-- The pure math (no UE calls) lives in avatars_pure.lua.
local PURE = load_module("avatars_pure")
local INJURY = load_module("body_injury")
local BODY_HEIGHT = load_module("standin_body") -- optional dev-only native height probe helper
local SEVERED_PHYSICS = os.getenv("HSMP_SEVERED_PHYSICS") ~= "0" -- explicit diagnostic opt-out
if not PURE then error("HSMPAvatars: avatars_pure.lua is missing (deploy copies every Scripts/*.lua)") end
local quat_to_rot, clamp = PURE.quat_to_rot, PURE.clamp

-- --- settings ----------------------------------------------------------------

local show_avatars = true
local stiff_mult = 1.0
local function refresh_settings()
    local f = io.open(SETTINGS_FILE, "rb"); if not f then return end
    local line = f:read("l"); f:close(); if not line then return end
    -- HSMPMenu writes "avatars"; older HSMPSettings wrote "show_avatars".
    local v = line:match('"avatars"%s*:%s*(%a+)') or line:match('"show_avatars"%s*:%s*(%a+)')
    if v == "true" then show_avatars = true elseif v == "false" then show_avatars = false end
    local nsv = { native_servo = line:match('"native_servo"%s*:%s*(%a+)'),   -- native servo A/B (true / false)
                  native_neutralise = line:match('"native_neutralise"%s*:%s*(%a+)'),
                  native_wservo = line:match('"native_wservo"%s*:%s*(%a+)') }
    local s = tonumber(line:match('"pose_stiffness"%s*:%s*([%d%.]+)'))
    if s then
        s = clamp(s, 0.25, 4.0)
        if s ~= stiff_mult then Log("pose_stiffness -> %.2f", s); stiff_mult = s end
    end
    for k, v in pairs(nsv) do
        if v == "true" then TUNE["setting_" .. k] = true elseif v == "false" then TUNE["setting_" .. k] = false end
    end
end
refresh_settings()


-- --- world guard ---------------------------------------------------------------
-- Every cached UObject (puppets, bodies, handles, weapons, meshes) belongs to
-- the world it was cached in. A round reset re-opens the SAME map: the world
-- keeps its name, but every actor of the old world is freed. Touching one of
-- them afterwards (even IsValid, and above all a property write) is an access
-- violation that pcall cannot catch (UObject __newindex ->
-- UStruct::FindProperty on a freed stand-in).
--   * world_gen is bumped by the LoadMap pre/post hooks, which run on the game
--     thread before the old world is torn down; the pre-hook also drops every
--     cache right there (pure Lua tables, no UE calls).
--   * every tick re-derives the world identity (generation + UWorld address +
--     name) from a fresh PlayerController; any change drops the caches
--     WITHOUT touching the old objects.
--   * the per-frame driver does nothing unless the tick has validated the
--     current generation.
local world_gen     = 0
local cache_gen     = -1      -- world_gen the caches were (re)validated for
local cache_world   = nil     -- world identity string the caches belong to
local drop_caches               -- forward (defined with the puppet tables)

local function world_identity(pc)
    local w
    if pc then pcall(function() w = pc:GetWorld() end) end
    if not (w and w:IsValid()) then pcall(function() w = UEHelpers.GetWorld() end) end
    if not (w and w:IsValid()) then return nil, nil end
    local name, addr
    pcall(function() name = w:GetFullName() end)
    pcall(function() addr = w:GetAddress() end)
    if not name then return nil, nil end
    return name, string.format("%s@%s", tostring(addr), name)
end

local tick_num = 0
local tick_hook = { ok = false, tries = 0, try = nil }   -- ReceiveTick post-hook state
local _world_name = nil
local function current_world() return _world_name end

local function is_gameplay(world_name)
    return world_name ~= nil and world_name:find("Map_Menu_") == nil
end

local function local_pc()
    local pc = UEHelpers.GetPlayerController()
    if pc and pc:IsValid() then return pc end
    return nil
end

local function local_pawn(pc)
    pc = pc or local_pc()
    if not pc then return nil end
    -- the game's own AI drives our pawn (dev, HSMPParity `ai on`; shared/hsmp_wg.lua)
    if TUNE.aip == nil then TUNE.aip = load_module("hsmp_wg") or false end
    local ai = TUNE.aip and TUNE.aip.ai_pawn_lookup() or nil
    if ai then return ai end
    local p = pc.Pawn
    if p and p:IsValid() then return p end
    return nil
end

-- UObject identity by address: two userdata wrappers of the same object are
-- not guaranteed to compare equal.
local function same(a, b)
    if a == nil or b == nil then return false end
    local ok, r = pcall(function() return a:GetAddress() == b:GetAddress() end)
    if ok then return r end
    return a == b
end

local function willie_health(w)
    local hp
    pcall(function() hp = tonumber(w.Health) end)
    return hp
end

local function willie_loc(w)
    local l
    pcall(function() l = w:K2_GetActorLocation() end)
    return l
end

local function at_origin(l)
    return not l or (math.abs(l.X) + math.abs(l.Y) + math.abs(l.Z) < 50)
end

local function array_num(arr)
    if not arr then return 0 end
    local n = 0
    if not pcall(function() n = arr:GetArrayNum() end) then
        pcall(function() n = #arr end)
    end
    return tonumber(n) or 0
end

local function willie_dead(w)
    local d = false
    pcall(function() d = (w.DED == true) end)
    return d
end

local function willie_dismembered(w)
    local arr; pcall(function() arr = w["Dismembered Array"] end)
    return array_num(arr) > 0
end

-- Controller class name of a Willie, "" when uncontrolled.
local function controller_class(w)
    local ctrl, cls = nil, ""
    pcall(function() ctrl = w.Controller end)
    if ctrl and ctrl:IsValid() then
        pcall(function() cls = ctrl:GetClass():GetFName():ToString() end)
    end
    return cls, ctrl
end

local function held_weapon_actor(actor, prefer_left)
    local order = prefer_left and { "Weapon L", "Weapon R" } or { "Weapon R", "Weapon L" }
    for _, k in ipairs(order) do
        local w; pcall(function() w = actor[k] end)
        if w and w:IsValid() then return w, k end
    end
    return nil
end

-- --- peer discovery ----------------------------------------------------------
-- Live peers = union of the sidecar roster and peer_root record slots whose
-- snapshot tick has advanced recently.

local my_peer_id   = 0
local roster       = {}   -- id -> nick (the peer directory: every connected roster peer)
local snap         = {}   -- id -> { tick, changed_at, st }
local _scan_tick   = -999

-- The one shared liveness rule (shared/hsmp_session.lua:
-- the sidecar's `link` record says "connected" AND its header heartbeat is fresh).
local HSM = load_module("hsmp_session")
local SESS = HSM and HSM.new({ every_s = 0 })
if not SESS then Log("WARNING: shared/hsmp_session.lua missing - falling back to the private liveness rule") end
local function mp_session_active()
    if SESS then
        local ok, live = pcall(SESS.live, SESS)
        return ok and live == true
    end
    return false   -- no shared/hsmp_session.lua: never an MP session
end

local function read_roster()
    if SESS then pcall(SESS.poll, SESS, true) end   -- every roster read (0.5 s) is a liveness poll
    local IPC = HSMP_IPC
    if IPC and IPC.use("peer_dir") then
        if not (SESS and SESS.exists) then return end   -- no link record yet: keep the last roster
        -- Own id from the sidecar's `link` record, the roster from the peer
        -- directory (typed). Every connected roster peer has an entry (lobby included):
        -- a stand-in is still claimed only for a peer whose PeerRoot is advancing.
        my_peer_id = math.tointeger(tonumber(SESS.link and SESS.link.my_peer_id)) or my_peer_id
        local out = {}
        for _, e in ipairs(IPC.peer_dir(true).list) do   -- roster reads are ~2 Hz: always fresh
            local id = math.tointeger(tonumber(e.id))
            if id and id ~= my_peer_id then out[id] = e.nick or ("P" .. id) end
        end
        roster = out
        return
    end
    roster = {}   -- no sidecar attached (or IPC unavailable): no session, no peers
end

local function read_snapshot(id)
    local IPC = HSMP_IPC
    if IPC and IPC.use("peer_root") then
        -- The peer_root record slot = the relayed root record + its owner (cached per
        -- version); liveness = an advancing tick. Only the yaw is derived here.
        local t = IPC.peer_rec("peer_root", id)
        local r = type(t) == "table" and t.root or nil
        if type(r) ~= "table" or type(r.pos) ~= "table" or not r.tick then return nil end
        if not PURE.pose_context_ok(PURE.root_context(r),HSM and HSM.view(),HSM and HSM.mode(),id) then return nil end
        -- yaw (degrees) of the quaternion {x, y, z, w}, as FQuat::Rotator
        local q = type(r.rot) == "table" and r.rot or {}
        local x, y, z, w = tonumber(q[1]) or 0, tonumber(q[2]) or 0, tonumber(q[3]) or 0, tonumber(q[4]) or 1
        local yaw = math.deg(math.atan(2 * (w * z + x * y), 1 - 2 * (y * y + z * z)))
        return { pos = { X = r.pos[1], Y = r.pos[2], Z = r.pos[3] }, yaw = yaw, tick = r.tick }
    end
    return nil
end

-- Refresh snapshot freshness for every candidate id (every tick for known
-- peers; full 1..MAX probe only every PEERS_EVERY ticks).
local function update_snapshots()
    local full_scan = tick_num - _scan_tick >= PEERS_EVERY
    if full_scan then
        _scan_tick = tick_num
        read_roster()
    end
    -- Candidates: every roster id and every id already seen (unbounded: ids
    -- keep growing across reconnects / matches on a dedicated server), plus
    -- a 1..MAX_PEER_ID probe on full scans (roster-less peers).
    if full_scan then
        -- An id that left the roster and has not moved for 10 s (and so is no
        -- longer live) is forgotten, so it is not probed at 30 Hz for the rest
        -- of the process.
        for id, s in pairs(snap) do
            if not roster[id] and tick_num - s.changed_at > 2 * STALE_TICKS then snap[id] = nil end
        end
    end
    local ids = {}
    for id in pairs(roster) do ids[id] = true end
    for id in pairs(snap) do ids[id] = true end
    if full_scan then for id = 1, MAX_PEER_ID do ids[id] = true end end
    for id in pairs(ids) do
        if id ~= my_peer_id then
            local st = read_snapshot(id)
            if st then
                local s = snap[id]
                if not s then
                    -- First sight is not proof of life (could be a leftover
                    -- record from an old session); only an advancing tick is.
                    snap[id] = { tick = st.tick, changed_at = -math.huge, st = st }
                elseif st.tick ~= s.tick then
                    s.tick, s.changed_at, s.st = st.tick, tick_num, st
                end
            end
        end
    end
end

-- Death handshake: HSMPCombat lists in the `standin_dead` bus record
-- every peer whose stand-in it put to death for a server-declared death,
-- BEFORE it zeroes Health and calls Death():
--   {wall = <os.time()>, rows = {{peer = <id>, name = "<stand-in FName>"}, ...}}
-- rewritten each second while non-empty. Never written keeps the last set (no
-- evidence); a set whose wall stamp is older than 5 s is void (a crashed
-- HSMPCombat must not pin corpses forever).
local DH = { set = {}, wall = 0, GRACE_S = 0.5 }
-- The typed record -> set {[peer] = name}, wall (nil when not a record).
function DH.parse(t)
    if type(t) ~= "table" then return nil end
    local wall = math.tointeger(t.wall)
    if not wall then return nil end
    local set = {}
    for _, r in ipairs(type(t.rows) == "table" and t.rows or {}) do
        local id = type(r) == "table" and math.tointeger(r.peer)
        if id then set[id] = type(r.name) == "string" and r.name or "" end
    end
    return set, wall
end
-- name (optional): the stand-in's actor FName. HSMPCombat writes the
-- name of the stand-in it put to death; an entry carried over from the last
-- world (another Willie) must not mark the NEW stand-in owner-dead.
function DH.declared(id, name)
    local t = HSMP_IPC and HSMP_IPC.bus_table("standin_dead")
    if t then
        local set, wall = DH.parse(t)
        if set then DH.set, DH.wall = set, wall end
    end
    if os.time() - DH.wall > 5 then return false end
    local v = DH.set[id]
    if v == nil then return false end
    if name and v ~= "" and v ~= name then return false end
    return true
end

-- A peer is live if the roster lists it, or its snapshots are still moving.
local function live_peers()
    local live = {}
    for id, nick in pairs(roster) do live[id] = nick end
    for id, s in pairs(snap) do
        if tick_num - s.changed_at <= STALE_TICKS and not live[id] then
            live[id] = "P" .. id
        end
    end
    return live
end

-- --- stand-in setup / diagnostics ------------------------------------------------

local PH_CLASS
local function ph_class()
    if PH_CLASS and PH_CLASS:IsValid() then return PH_CLASS end
    PH_CLASS = StaticFindObject("/Script/Engine.PhysicsHandleComponent")
    return PH_CLASS
end

local IDENTITY_XFORM = {
    Translation = { X = 0, Y = 0, Z = 0 },
    Rotation    = { X = 0, Y = 0, Z = 0, W = 1 },
    Scale3D     = { X = 1, Y = 1, Z = 1 },
}

local function tune_handle(h, tune, scale)
    local m = stiff_mult * (scale or 1)
    pcall(function() h:SetLinearStiffness(tune.lin_k * m) end)
    pcall(function() h:SetLinearDamping(tune.lin_d * math.sqrt(m)) end)
    pcall(function() h:SetAngularStiffness(tune.ang_k * m) end)
    pcall(function() h:SetAngularDamping(tune.ang_d * math.sqrt(m)) end)
end

local function make_handle(actor, tune)
    local cls = ph_class()
    if not cls then return nil end
    local h
    pcall(function() h = actor:AddComponentByClass(cls, true, IDENTITY_XFORM, false) end)
    if not h or not h:IsValid() then return nil end
    pcall(function() h.bSoftLinearConstraint = true end)
    pcall(function() h.bSoftAngularConstraint = true end)
    pcall(function() h.bInterpolateTarget = false end)   -- targets are already interpolated
    tune_handle(h, tune)
    return h
end

local function grabbed(h)
    local g; pcall(function() g = h.GrabbedComponent end)
    return g and g:IsValid()
end

local function tune_for(bn)
    if HAND_BONES[bn] then return PH_HAND end
    if ARM_BONES[bn] then return PH_ARM end
    return PH_BODY
end

-- Mesh pick rule (codec-v1 driver, local pawn): +2 if the Pelvis body
-- simulates, +1 if visible. Real component names: SK_Skeleton,
-- BoneCore, DriverSkeleton, Mesh (UE4SS_ObjectDump.txt).
local MESH_FIELDS = { "SK_Skeleton", "BoneCore", "DriverSkeleton", "Mesh" }
local function pick_pose_mesh(actor)
    local best, best_field, best_score, sims, report = nil, nil, -1, {}, {}
    for _, field in ipairs(MESH_FIELDS) do
        local m; pcall(function() m = actor[field] end)
        if m and m:IsValid() then
            local sim, vis = false, false
            pcall(function() sim = m:IsSimulatingPhysics(fname("Pelvis")) end)
            pcall(function() vis = m:IsVisible() end)
            if sim then sims[#sims + 1] = m end
            local score = (sim and 2 or 0) + (vis and 1 or 0)
            report[#report + 1] = string.format("%s(sim=%d vis=%d)", field, sim and 1 or 0, vis and 1 or 0)
            if score > best_score then best, best_field, best_score = m, field, score end
        end
    end
    return best, best_field, sims, table.concat(report, " ")
end

-- Stand-in's own bone lengths (bone -> distance to PURE.PARENT), measured on
-- the driven mesh. Ragdoll joints keep these fixed.
local function measure_lengths(mesh)
    local pos = {}
    for _, bn in ipairs(HERO_BONES) do
        local l; pcall(function() l = mesh:GetSocketLocation(fname(bn)) end)
        if l then pos[bn] = { l.X, l.Y, l.Z } end
    end
    local lens, n = {}, 0
    for bn, par in pairs(PURE.PARENT) do
        if pos[bn] and pos[par] then
            local L = PURE.d3(pos[bn], pos[par])
            if L > 0.5 then lens[bn] = L; n = n + 1 end
        end
    end
    return lens, n
end

local handle_cache = {}   -- actor address -> { bones = {bone -> rec}, wpn = rec }
local function puppet_body(p)
    if p.body and p.body.mesh and p.body.mesh:IsValid() then return p.body end
    local actor = p.actor
    local pick, field_name, sims, report = pick_pose_mesh(actor)
    if not pick then return nil end
    local bones = {}
    for _, bn in ipairs(HERO_BONES) do
        local idx = -1
        pcall(function() idx = pick:GetBoneIndex(fname(bn)) end)
        if idx and idx >= 0 then bones[#bones + 1] = bn end
    end
    local motors = {}
    pcall(function() local pa = actor.PhysicalAnimation; if pa and pa:IsValid() then motors[#motors + 1] = pa end end)
    pcall(function()
        local arr = actor["Phys Anim Array"]
        if arr then arr:ForEach(function(_, e) local pa = e:get(); if pa and pa:IsValid() then motors[#motors + 1] = pa end end) end
    end)
    -- Reuse handle components across re-claims of the same actor (they are
    -- real components on it; creating new ones each time would leak).
    local key; pcall(function() key = actor:GetAddress() end)
    local hs = (key and handle_cache[key]) or {}
    if key then handle_cache[key] = hs end
    hs.bones = hs.bones or {}
    local lens, nlens = measure_lengths(pick)
    local mesh_addr,mesh_fname
    pcall(function()mesh_addr=pick:GetAddress();mesh_fname=pick:GetFName():ToString()end)
    p.body = { mesh = pick, field = field_name, sims = sims, bones = bones, motors = motors,
               mesh_addr=mesh_addr,mesh_fname=mesh_fname,
               handles = hs.bones, wpn = hs.wpn, cache = hs, ctl = nil, snaps = 0,
               lens = lens, err_since = nil, made_at = now_ms(), stiff = stiff_mult }
    Log("pose: stand-in %s drives %s (%d simulated meshes), %d/%d bones, %d bone lengths, %d physical-animation comps; candidates: %s",
        actor:GetFName():ToString(), tostring(field_name), #sims, #bones, #HERO_BONES, nlens, #motors, report)
    return p.body
end

-- One-shot dump of everything that decides whether a stand-in can follow.
local function dump_standin(p)
    local a = p.actor
    local function b(v) return v and "1" or "0" end
    local function rd(k) local v; pcall(function() v = a[k] end); return v end
    local lines = {}
    local function add(fmt, ...) lines[#lines + 1] = string.format(fmt, ...) end
    local dism = {}
    pcall(function()
        local arr = a["Dismembered Array"]
        if arr then arr:ForEach(function(_, e) local n = e:get(); dism[#dism + 1] = n and n:ToString() or "?" end) end
    end)
    local muscles = {}
    for _, k in ipairs(MUSCLE_FLAGS) do
        muscles[#muscles + 1] = k:gsub("^Block ", ""):gsub(" Muscles$", ""):gsub(" ", "") .. "=" .. b(rd(k) == true)
    end
    add("dump %s: HP=%s DED=%s Fallen=%s Downed=%s Consciousness=%s ForceDisableDismemberment=%s dismembered=[%s] block{%s} ctrl=%s",
        a:GetFName():ToString(), tostring(willie_health(a)), b(rd("DED") == true), b(rd("Fallen") == true),
        b(rd("Downed") == true), tostring(rd("Consciousness")), b(rd("Force Disable Dismemberment") == true),
        table.concat(dism, ","), table.concat(muscles, " "), controller_class(a) ~= "" and controller_class(a) or "none")
    local root; pcall(function() root = a.RootComponent end)
    for _, field in ipairs({ "RootComponent", "SK_Skeleton", "BoneCore", "DriverSkeleton", "Mesh" }) do
        local m = (field == "RootComponent") and root or rd(field)
        if m and m:IsValid() then
            local cls, l, sim_any, simb, hid, vis = "?", nil, false, 0, 0, false
            pcall(function() cls = m:GetClass():GetFName():ToString() end)
            pcall(function() l = m:K2_GetComponentLocation() end)
            pcall(function() sim_any = m:IsSimulatingPhysics(fname("None")) end)
            pcall(function() vis = m:IsVisible() end)
            for _, bn in ipairs(HERO_BONES) do
                local s = false
                pcall(function() s = m:IsSimulatingPhysics(fname(bn)) end)
                if s then simb = simb + 1 end
                local h = false
                pcall(function() h = m:IsBoneHiddenByName(fname(bn)) end)
                if h then hid = hid + 1 end
            end
            local pel
            pcall(function() pel = m:GetSocketLocation(fname("Pelvis")) end)
            add("  %-15s %-28s at (%s) vis=%s sim=%s simBones=%d/16 hiddenBones=%d pelvis=(%s)",
                field, cls,
                l and string.format("%.0f,%.0f,%.0f", l.X, l.Y, l.Z) or "?",
                b(vis), b(sim_any), simb, hid,
                pel and string.format("%.0f,%.0f,%.0f", pel.X, pel.Y, pel.Z) or "?")
        end
    end
    for _, k in ipairs({ "Weapon R", "Weapon L" }) do
        local w = rd(k)
        if w and w:IsValid() then
            local wr, sim, par = nil, false, "none"
            pcall(function() wr = w.RootComponent end)
            pcall(function() sim = wr:IsSimulatingPhysics(fname("None")) end)
            pcall(function() local pa = w:GetAttachParentActor(); if pa and pa:IsValid() then par = pa:GetFName():ToString() end end)
            add("  %s = %s root=%s sim=%s attachParent=%s", k, w:GetFName():ToString(),
                (wr and wr:IsValid()) and wr:GetClass():GetFName():ToString() or "nil", b(sim), par)
        else
            add("  %s = none", k)
        end
    end
    add("  PhysicsHandleComponent class %s", ph_class() and "found" or "MISSING")
    for _, l in ipairs(lines) do Log("%s", l) end
end

local function set_muscles_blocked(p, blocked)
    if not BLOCK_MUSCLES then blocked = false end
    if p.muscles_blocked == blocked then return end
    local n = 0
    for _, k in ipairs(MUSCLE_FLAGS) do
        if pcall(function() p.actor[k] = blocked end) then n = n + 1 end
    end
    p.muscles_blocked = blocked
    if not p.muscles_logged then
        p.muscles_logged = true
        local v; pcall(function() v = p.actor["Block Arm Muscles"] end)
        Log("pose: %s muscles %s (%d/%d flags set, readback \"Block Arm Muscles\"=%s)",
            p.actor:GetFName():ToString(), blocked and "blocked" or "released", n, #MUSCLE_FLAGS, tostring(v))
    end
end

-- Stand-ins must never lose limbs or die from purely local physics: the
-- owner is authoritative (HSMPCombat replicates real wounds/death).
local function harden_standin(p)
    pcall(function() p.actor["Force Disable Dismemberment"] = true end)
    pcall(function() p.actor["Force Disable Vertex Paint"] = true end)
end

local function set_motor_strength(body, s)
    for _, pa in ipairs(body.motors) do
        if pa and pa:IsValid() then pcall(function() pa:SetStrengthMultiplyer(s) end) end
    end
    body.motor_str = s
end

-- Create (once) and grab a handle per replicated bone. Caches the grab state
-- in rec.ok for the per-frame driver. Returns the number of bones held.
local function ensure_handles(p, body)
    if body.ctl == "velocity" then return 0 end
    if body.stiff ~= stiff_mult then   -- live re-tune from the settings file
        body.stiff = stiff_mult
        for bn, rec in pairs(body.handles) do
            if rec.h and rec.h:IsValid() then tune_handle(rec.h, tune_for(bn)); rec.soft = false end
        end
        if body.wpn and body.wpn.h and body.wpn.h:IsValid() then tune_handle(body.wpn.h, PH_WPN) end
    end
    local held = 0
    for _, bn in ipairs(body.bones) do
        local rec = body.handles[bn]
        if not rec then
            local h = make_handle(p.actor, tune_for(bn))
            if not h then
                body.ctl = "velocity"
                Log("pose: PhysicsHandle unavailable on %s; falling back to velocity steering",
                    p.actor:GetFName():ToString())
                return 0
            end
            rec = { h = h, nobody = false, ok = false }
            body.handles[bn] = rec
        end
        rec.ok = (not rec.nobody) and grabbed(rec.h)
        if not rec.nobody and not rec.ok then
            local fn = fname(bn)
            local loc, q
            pcall(function() loc = body.mesh:GetSocketLocation(fn) end)
            pcall(function() q = body.mesh:GetSocketQuaternion(fn) end)
            if loc and q then
                pcall(function()
                    rec.h:GrabComponentAtLocationWithRotation(body.mesh, fn, loc, quat_to_rot(q.X, q.Y, q.Z, q.W))
                end)
                rec.ok = grabbed(rec.h)
                if not rec.ok then
                    rec.nobody = true   -- this bone has no physics body
                end
            end
        end
        if rec.ok then held = held + 1 end
    end
    body.ctl = "handles"
    body.held = held
    return held
end

local function release_handles(body)
    for _, rec in pairs(body.handles) do
        if rec.h and rec.h:IsValid() and grabbed(rec.h) then pcall(function() rec.h:ReleaseComponent() end) end
        rec.ok = false
    end
    if body.wpn and body.wpn.h and body.wpn.h:IsValid() and grabbed(body.wpn.h) then
        pcall(function() body.wpn.h:ReleaseComponent() end)
    end
    if body.wpn then body.wpn.ok = false end
end

-- The stand-in's grip constraints (BP-added PhysicsConstraintComponents
-- between a held item's BaseMesh and hand_r / hand_l, angular SLERP drive).
-- After a weapon is dropped the grip is re-made to the Weapon_Fists pseudo
-- weapon, which sits kinematic at the world origin: its angular drive then
-- twists the driven hand/forearm/upper arm towards the fists' orientation
-- (measured in-game: 50-90 deg of pure TWIST error on one arm, held for tens
-- of seconds, positions within a few uu). While driven, both hands and the
-- weapons are servoed to the owner's own (consistent) poses, so the grip
-- drives are switched off; the BP's values come back on release.
-- The grip's LIMITS (locked hand-to-weapon frame) are freed as well: the
-- stand-in's grip frame is whatever its kit grab produced, not the owner's
-- grip (measured: both hands 105-120 deg of twist off target for a whole
-- session while the servoed weapons tracked to 5 uu: the weapons won). With
-- the grip free, hand and weapon each go exactly where the owner has them.

local PX = {}   -- pose extras (one local: the main chunk is at the 200-locals limit)
PX.SETTLE = load_module("spawn_settle")
function PX.settle_reset(p,reason)
    if PX.SETTLE then
        PX.SETTLE.invalidate(p.settle_state,reason,now_ms())
        PX.SETTLE.copy(p.shown,p.settle_state)
    end
end

-- Dev tuning knobs for the v2 servo: dev_cmd TUNE records from the DevCtl ring
-- (`hsmp-tools ipc-ctl --pid <game> tune servo 0`; PURE.tune_value): "servo" 0 stops
-- driving (experiments), "wpn" 0 skips the weapon servo, "cap_lin"/"cap_ang" override the
-- correction caps, ... Values live for this game session (a restart = defaults). Polled
-- every tick (one native call; the native module fans each record out to every Lua state).
function PX.poll_dev()
    local ipc = HSMP_IPC
    if not ipc or not ipc.dev_poll then return end
    TUNE.dev_out = TUNE.dev_out or {}
    local out = TUNE.dev_out
    local n = ipc.dev_poll(16, out)
    for i = 1, n do
        local c = out[i] and (out[i].data or out[i])
        if type(c) == "table" and c.op == 1 and c.key == "bodyphysics" and PX.bodyphysics then
            PX.bodyphysics_request = tostring(c.arg or "") -- act only after the normal world guard
        end
        if type(c) == "table" and c.op == 1 and c.key == "bodyheight" then
            PX.bodyheight_request = tostring(c.arg or "")
        end
        if type(c)=="table" and c.op==1 and c.key=="parity" and os.getenv("HSMP_DEV")=="1" then
            local exp,rest=tostring(c.arg or ""):match("^(%S+)%s*(.*)$")
            if exp=="weaponstate" then PX.weaponstate_request=rest end
        end
        if type(c) == "table" and c.op == 2 then   -- S.ENUMS.dev_op.TUNE
            local k = tostring(c.key or "")
            local v, err = PURE.tune_value(k, tonumber(c.num))
            if v == nil then
                TUNE.unknown = TUNE.unknown or {}
                if not TUNE.unknown[k] then TUNE.unknown[k] = true; Log("pose tune: %s %s ignored (%s)", k, tostring(c.num), tostring(err)) end
            else
                TUNE[k] = v or nil
                SERVO_CAP_LIN = TUNE.cap_lin or 900
                SERVO_CAP_ANG = TUNE.cap_ang or 900
                local key = PURE.tune_key({ servo = TUNE.servo, wpn = TUNE.wpn, cap_lin = TUNE.cap_lin, cap_ang = TUNE.cap_ang,
                    lead = TUNE.lead, gain = TUNE.gain, world = TUNE.world, ghost = TUNE.ghost, clock = TUNE.clock, motors = TUNE.motors,
                    bench = TUNE.bench, grips = TUNE.grips, limits = TUNE.limits, leg_gain = TUNE.leg_gain, retarget = TUNE.retarget,
                    tonus = TUNE.tonus, tdiag = TUNE.tdiag, stamp = TUNE.stamp, noacc = TUNE.noacc, lat = TUNE.lat, plant = TUNE.plant,
                    v1aim = TUNE.v1aim })
                if key ~= TUNE.key then Log("pose tune: %s", key) end
                TUNE.key = key
            end
        end
    end
end
-- Native servo (cap NATIVE_SERVO, docs/development/ipc-shared-memory.md), in three parts;
-- each has its own A/B switch, ON by default (measured in game: stand-in frame cost about
-- 31 % lower, same quality): settings "<key>" (true / false), dev tune "<key>" (1 / 0,
-- TUNE[key]), env HSMP_<KEY>=1|0 (overrides). Any refusal: the Lua path does that frame.
--   "native_servo":      drive_v2's body loop (22 x GetSocketTransform +
--        SetPhysicsLinearVelocity + SetPhysicsAngularVelocityInDegrees) as ONE native call
--        (HSMPNative.servo_bodies: the same PURE.servo math bit for bit); this file keeps the
--        targets, the gains / yield policy and every metric.
--   "native_neutralise": neutralise_bp's ~30 BP variable writes + the joint-motor call
--        (HSMPNative.neutralise); the tonus originals (restored on release), the
--        PhysicalAnimation strength and the grips stay here.
--   "native_wservo":     a held weapon's servo (GetTransform + the two velocity sets on its
--        root) as one native call (HSMPNative.servo_weapon); the weapon parts' validation
--        and the tip metrics stay here.
function PX.nat_state(key, label)
    return { key = key, label = label, want = false, env = os.getenv("HSMP_" .. key:upper()), a = {}, out = {},
             configured = false, retry_at = 0, used = 0, fell = 0, logged = nil }
end
PX.NSV = PX.nat_state("native_servo", "native servo")
PX.NNV = PX.nat_state("native_neutralise", "native neutralise")
PX.NWV = PX.nat_state("native_wservo", "native weapon servo")
-- Is stage `s` switched on and available right now?
function PX.nat_on(s)
    local want = TUNE["setting_" .. s.key] ~= false   -- on unless the settings say false
    if TUNE[s.key] ~= nil then want = TUNE[s.key] ~= 0 end
    if s.env == "1" then want = true elseif s.env == "0" then want = false end
    if want ~= s.want then
        s.want = want
        s.retry_at = 0
        Log("%s %s", s.label, want and "ON" or "off")
    end
    if not want then return false end
    local ipc = HSMP_IPC
    if not (ipc and ipc.is_shm and ipc.is_shm()) or os.clock() < s.retry_at then return false end
    ipc.refresh_info(false)
    local cap = (ipc.S and ipc.S.CAPS and ipc.S.CAPS.NATIVE_SERVO) or 0
    return ((ipc.caps_game or 0) & cap) ~= 0
end
function PX.nat_refused(s, err)
    s.fell = s.fell + 1
    err = tostring(err)
    if err == "unavailable" or err == "missing" or err == "not configured" or err:find("^disabled:") or err:find("^config:") then
        s.retry_at = os.clock() + 5
        if err == "not configured" then s.configured = false; PX.servo_configured = nil end
    end
    if s.logged ~= err then s.logged = err; Log("%s refused (%s): Lua path", s.label, err) end
end
-- Closure-free reflected reads for the per-frame paths: pcall(PX.r_*, obj, ...).
function PX.r_index(o, k) return o[k] end
PX.vlin, PX.vang = { X = 0, Y = 0, Z = 0 }, { X = 0, Y = 0, Z = 0 }   -- velocity arguments, refilled per call
function PX.r_addr(o) return o:GetAddress() end
function PX.nat_addr(o)
    local ok, a = pcall(PX.r_addr, o)
    if ok then return a end
    return nil
end
-- servo_config once (shared by native_servo and native_wservo); false = refused (logged against `s`).
function PX.ns_config(s)
    if PX.servo_configured then return true end
    local bones = {}
    for i = 1, PURE.V2_NB do bones[i] = PURE.V2_SLOTS[i] end
    local ok, err = HSMP_IPC.servo_config({ bones = bones })
    if not ok then PX.nat_refused(s, "config:" .. tostring(err)); return false end
    PX.servo_configured = true
    return true
end
function PX.ns_on()
    local s = PX.NSV
    if not PX.nat_on(s) or not HSMP_IPC.servo_bodies then return false end
    return PX.ns_config(s)
end
-- The weapon part of the stand-in status line ("Sword_Arming,att=CharacterMesh0/hand_r"):
-- ~5 reflected calls per hand, so cached on the weapon entry `c` (a new entry for every
-- weapon change; dropped with the world) and refreshed at most once a second, always from
-- the `wa` / `c.root` that servo_weapon_parts validated THIS frame.
PX.WPN_STATUS_MS = 1000
function PX.wpn_status(c, wa, now)
    if not (c and wa) then return "-" end
    if c.status and now - (c.status_at or -1e9) < PX.WPN_STATUS_MS then return c.status end
    local cls = "-"
    pcall(function() cls = wa:GetClass():GetFName():ToString():gsub("ModularWeaponBP_", ""):gsub("_C$", "") end)
    pcall(function()
        local par = c.root:GetAttachParent()
        if par and par:IsValid() then cls = cls .. ",att=" .. par:GetFName():ToString() .. "/" .. c.root:GetAttachSocketName():ToString() end
    end)
    c.status, c.status_at = cls, now
    return cls
end
-- native_wservo: one held weapon natively; returns the reused out table {x, v} or nil (Lua path).
function PX.nw_weapon(c, tg, dt, holding)
    local s = PX.NWV
    if not PX.nat_on(s) or not HSMP_IPC.servo_weapon or not PX.ns_config(s) then return nil end
    if not (c.addr and c.root_addr and c.com) then return nil end
    local a = s.a
    a.actor, a.root, a.aim, a.com, a.dt = c.addr, c.root_addr, tg, c.com, dt
    a.cap_lin, a.cap_ang, a.gain, a.holding = SERVO_CAP_LIN, SERVO_CAP_ANG, TUNE.gain or SERVO_GAIN, holding and true or false
    local ok, err = HSMP_IPC.servo_weapon(a, s.out)
    if ok then s.used = s.used + 1; return s.out end
    PX.nat_refused(s, err)
    return nil
end
-- native_servo: the body loop natively: returns the reused out table {c, v, dl, gl} or nil (Lua loop).
function PX.ns_bodies(mesh, aim, sv, dt, capl, capa, gain, yl, holding)
    if not PX.ns_on() then return nil end
    local s, a = PX.NSV, PX.NSV.a
    local addr = PX.nat_addr(mesh)
    if not addr then return nil end
    a.mesh, a.dt, a.cap_lin, a.cap_ang, a.gain = addr, dt, capl, capa, gain
    a.leg_gain = (not yl) and (TUNE.leg_gain or PX.LEG_GAIN) or gain
    a.leg_from, a.holding, a.aim, a.com = 18, holding and true or false, aim, sv.com
    local n, err = HSMP_IPC.servo_bodies(a, s.out)
    if n then s.used = s.used + 1; return s.out end
    PX.nat_refused(s, err)
    return nil
end
-- native_neutralise: neutralise_bp's variable writes + the motor call natively; true = done.
function PX.nn(p, body)
    local s = PX.NNV
    if not PX.nat_on(s) or not HSMP_IPC.neutralise then return false end
    if not s.configured then
        local zero = { "Head Tonus", "Root Angular Constraint Power", "Root Linear Constraint Power" }
        for _, k in ipairs(PAIN_ZERO) do zero[#zero + 1] = k end
        local ok, err = HSMP_IPC.neutralise_config({ zero = zero, set_true = BLOCK_MUSCLES and MUSCLE_FLAGS or {},
                                                     set_false = PAIN_FALSE, tonus = PX.TONUS })
        if not ok then PX.nat_refused(s, "config:" .. tostring(err)); return false end
        s.configured = true
    end
    local tonus = TUNE.tonus ~= 0
    if tonus and not p.tonus0 then   -- the originals, once (restored on release)
        local act = p.actor
        p.tonus0 = {}
        for _, k in ipairs(PX.TONUS) do pcall(function() p.tonus0[k] = act[k] end) end
    end
    local a = s.a
    a.actor, a.tonus, a.motors_off = PX.nat_addr(p.actor), tonus, TUNE.motors ~= 0
    a.mesh = a.motors_off and PX.nat_addr(body.mesh) or nil
    if not a.actor then return false end
    local n, err = HSMP_IPC.neutralise(a)
    if n then s.used = s.used + 1; return true end
    PX.nat_refused(s, err)
    return false
end
-- The owner's `peer_vitals` record says dead (VF DEAD flag, or Health quantised
-- to 0; 65535 = unknown). nil = no record for that peer (no evidence).
function PX.owner_vitals_dead(id)
    local t = HSMP_IPC and HSMP_IPC.peer_rec("peer_vitals", id)
    if type(t) ~= "table" then return nil end
    local f = math.tointeger(t.flags) or 0
    local hp = type(t.v) == "table" and math.tointeger(t.v[1]) or nil
    return f & 1 ~= 0 or hp == 0
end
-- The owner's `peer_vitals` record says fallen or downed (VF FALLEN 2 / DOWNED 4). nil = no record.
function PX.owner_vitals_down(id)
    local t = HSMP_IPC and HSMP_IPC.peer_rec("peer_vitals", id)
    if type(t) ~= "table" then return nil end
    return (math.tointeger(t.flags) or 0) & 6 ~= 0
end
PX.BUDGET_MS = 4   -- Lua frame budget for the stand-in driver (ms per frame)
-- Legs keep a stiffer servo than the 0.3 body gain: the driven mesh ignores world
-- geometry (no floor friction), so only the servo holds a planted foot against the
-- reaction of arm swings (A/B under the mover: foot_slide_p95 8-9 uu/s at 0.3,
-- up to 4.9 at 0.6, 1.1-3.5 at 0.9 with jitter_ratio ~1.0). With the time-exact v2 aim the legs no
-- longer lag a frame; 0.9 then passes every target revision on to the feet (foot jitter_ratio 1.5-1.9 at
-- n = 25), while 0.5 keeps foot_slide_p95 <= 1.5 uu/s and the feet at 1.2-1.3 on Pit.
PX.LAT_TARGET_MS = 75   -- v2 aim: display latency (sample age + buffer + step part) the state offset aims for (gate limit 90)
PX.STALL_FRAMES = 20   -- a limb capped > 45 deg off this many frames in a row -> re-pose
PX.LEG_GAIN = 0.5
PX.FALLBACK_GRACE_S = 6   -- no SpawnCombatants fallback this soon after a world load (after the spawn placement)
PX.WORLD_SETTLE_S = 2.0   -- on_tick touches no Willie this soon after a world load (see on_tick)
-- Our own pawn's spawn placement is verified in this world (HSMPSync's typed bus
-- key `spawn_status`: verified, t = os.clock() at the write, the process clock all
-- UE4SS mods share; census.arena_at = this world's start). seq 0 = cleared.
function PX.own_placed()
    local s = HSMP_IPC and HSMP_IPC.bus_table("spawn_status")
    if type(s) ~= "table" or (s.seq or 0) == 0 or not s.verified then return false end
    local t = tonumber(s.t)
    return t ~= nil and PX.arena_at_fn ~= nil and t > PX.arena_at_fn()
end
-- Where a peer's fighter stands this round: the server's spawn order for it (the
-- session record's roster row, shared/hsmp_session.lua HS.view()), as a capsule-centre point.
function PX.peer_spawn(id)
    local v = HSM and HSM.view()
    local e = v and v.spawns[id]
    if not e or not (e.x and e.y and e.z) then return nil end
    return { X = e.x, Y = e.y, Z = e.z + 90 }
end
-- Where the fallback spawns peer `id`'s body: its server spawn order (where it
-- will stand), else its last pose; never within 250 uu of our own pawn (nil:
-- wait). Before the peer's own placement its pose is the native player
-- spawner, which can be OUR slot (LordsHall slot 0): a body spawned into our
-- placed pawn shoves it ~2 m off its spawn before Ready. `s` = snap[id]
-- (passed: snap is declared later).
function PX.spawn_spot(id, s)
    local at = PX.peer_spawn(id) or (s and s.st and s.st.pos and s.st.pos.X and s.st.pos) or nil
    if not at then return nil end
    local me = local_pawn()
    local ml = me and willie_loc(me)
    if ml and PURE.d3({ at.X, at.Y, at.Z }, { ml.X, ml.Y, ml.Z }) < 250 then
        if not PX.spawn_near_logged then
            PX.spawn_near_logged = true
            Log("fallback: peer %d's spot (%.0f,%.0f) is within 250 uu of our pawn; not spawning there", id, at.X, at.Y)
        end
        return nil
    end
    return at
end
PX.TONUS = { "All Body Tonus", "Upper Body Tonus", "Arm R Tonus", "Arm L Tonus", "Leg R Tonus", "Leg L Tonus", "Muscle Power" }
function PX.grip_want(p,g,now)
    if not g.lim or TUNE.grips==0 then return false end
    local field=g.hand=="hand_r" and "Weapon R" or "Weapon L"
    local wa=p.actor[field] -- current field, never a retained weapon UObject
    if not (wa and wa:IsValid()) then return false end
    if wa:GetClass():GetFName():ToString():match("^Weapon_Fists") then return true end
    local ws=p.wservo and p.wservo[field]
    return ws and now>=ws and now-ws<500 and p.wservo_actor and p.wservo_actor[field]==wa:GetAddress()
end
function PX.grip_identity(g,c)
    return c and c:IsValid() and c:GetAddress()==g.addr and c:GetFName():ToString()==g.fname
        and c.ConstraintInstance.ConstraintBone2:ToString()==g.hand
end
function PX.grip_limits(p,g,c,now)
    if PX.grip_want(p,g,now) then
        c:SetLinearXLimit(0,0);c:SetLinearYLimit(0,0);c:SetLinearZLimit(0,0)
        c:SetAngularSwing1Limit(0,0);c:SetAngularSwing2Limit(0,0);c:SetAngularTwistLimit(0,0)
        g.freed=true
    elseif g.freed and g.lim then
        local l=g.lim
        c:SetLinearXLimit(l[1],l[4]);c:SetLinearYLimit(l[2],l[4]);c:SetLinearZLimit(l[3],l[4])
        c:SetAngularSwing1Limit(l[5],l[6]);c:SetAngularSwing2Limit(l[7],l[8]);c:SetAngularTwistLimit(l[9],l[10])
        g.freed=nil
    end
end
function PX.grip_drive_current(p)
    if not p.driving or p.gen~=world_gen or cache_gen~=world_gen or not p.peer or type(p.last)~="table" then return false end
    return PURE.pose_context_ok(p.last,HSM and HSM.view(),HSM and HSM.mode(),p.peer)
end
function PX.grip_constraints(p, now)
    if p.grips and now - p.grips.at < 1000 then return p.grips.list end
    local list = {}
    PX.CONSTRAINT_CLASS = PX.CONSTRAINT_CLASS or StaticFindObject("/Script/Engine.PhysicsConstraintComponent")
    local comps
    pcall(function() comps = p.actor:K2_GetComponentsByClass(PX.CONSTRAINT_CLASS) end)
    local n = 0
    pcall(function() n = #comps end)
    for i = 1, n do
        pcall(function()
            local c = comps[i]
            local ok, x = pcall(function() return c:get() end)
            if ok and x then c = x end
            if not c:IsValid() then return end
            local b2 = c.ConstraintInstance.ConstraintBone2:ToString()
            if b2 == "hand_r" or b2 == "hand_l" then
                local addr = c:GetAddress()
                local name=c:GetFName():ToString()
                -- keep the BP values seen first (before we zeroed them) for the restore
                local o = p.grips and p.grips.by[addr]
                if o and o.fname==name and o.hand==b2 then o.c = c; list[#list + 1] = o else   -- the fresh component, never last scan's pointer
                    local pi = c.ConstraintInstance.ProfileInstance
                    local d = pi.AngularDrive.SlerpDrive
                    local g = { c = c, addr = addr, fname=name, hand = b2, stiff = d.Stiffness, damp = d.Damping, maxf = d.MaxForce }
                    pcall(function()
                        local L, C, T = pi.LinearLimit, pi.ConeLimit, pi.TwistLimit
                        g.lim = { L.XMotion, L.YMotion, L.ZMotion, L.Limit, C.Swing1Motion, C.Swing1LimitDegrees,
                                  C.Swing2Motion, C.Swing2LimitDegrees, T.TwistMotion, T.TwistLimitDegrees }
                    end)
                    list[#list + 1] = g
                    g.fresh = true
                end
            end
        end)
    end
    local by = {}
    local changed = false
    for _, g in ipairs(list) do by[g.addr] = g; if not (p.grips and p.grips.by[g.addr]==g) then changed = true end end
    if changed then
        Log("pose: stand-in grip constraints driven off: %d (BP slerp drive %s)", #list, tostring(list[1] and list[1].stiff))
    end
    p.grips = { at = now, list = list, by = by }
    -- A real weapon needs a fresh servo target before its grip is freed.
    -- Fists have no weapon pose target at all: a locked grip to that pseudo
    -- actor fights the hand-bone servo, so driven fists must be free too.
    -- Re-applied each refresh: the BP may re-lock.
    for _, g in ipairs(list) do
        pcall(function()
            PX.grip_limits(p,g,g.c,now)
        end)
        g.fresh = nil
    end
    return list
end
function PX.grips_desc(p)
    local t = {}
    for _, g in ipairs(p.grips and p.grips.list or {}) do
        t[#t + 1] = string.format("%s%s", g.hand == "hand_r" and "R" or "L", g.freed and ":free" or ":locked")
    end
    return #t > 0 and table.concat(t, ",") or "none"
end
-- The grip constraints of a stand-in, as FRESH components keyed by
-- address. The BP destroys and rebuilds them on a pick-up / drop / disarm;
-- a pointer kept from the last 1 s scan can be freed by GC by then, and
-- IsValid() on freed memory is an access violation pcall cannot catch.
function PX.fresh_grips(p)
    local fresh = {}
    PX.CONSTRAINT_CLASS = PX.CONSTRAINT_CLASS or StaticFindObject("/Script/Engine.PhysicsConstraintComponent")
    pcall(function()
        local comps = p.actor:K2_GetComponentsByClass(PX.CONSTRAINT_CLASS)
        for i = 1, #comps do
            pcall(function()
                local c = comps[i]
                local ok, x = pcall(function() return c:get() end)
                if ok and x then c = x end
                if c:IsValid() then fresh[c:GetAddress()] = c end
            end)
        end
    end)
    return fresh
end
local function constraint_bone2(c) return c.ConstraintInstance.ConstraintBone2:ToString() end
function PX.grips_off(p, off)
    local list = p.grips and p.grips.list
    if not list or #list == 0 then return end
    if (p.gen~=nil and p.gen~=world_gen) or (off and not PX.grip_drive_current(p)) then
        for _,g in ipairs(list) do g.c=nil end -- forget wrappers without touching stale native objects
        return
    end
    local fresh = PX.fresh_grips(p)
    if off then
        -- A grip the BP rebuilt (pick-up, drop, grip change) has a new address: until the
        -- next scan it would keep its locked limits and drive against the hand servo
        -- (seen 60-90 deg hand error). Force that scan on the next drive tick.
        local g = p.grips
        for addr, c in pairs(fresh) do
            if not g.by[addr] and not (g.other and g.other[addr]) then
                local ok, b2 = pcall(constraint_bone2, c)
                if ok and (b2 == "hand_r" or b2 == "hand_l") then g.at = -math.huge
                else g.other = g.other or {}; g.other[addr] = true end
            end
        end
    end
    for _, g in ipairs(list) do
        g.c = fresh[g.addr]   -- nil when the BP rebuilt it: never touch the old one
        pcall(function()
            if not PX.grip_identity(g,g.c) then
                if off then p.grips.at=-math.huge end
                g.c=nil;return
            end
            if off then
                g.c:SetAngularDriveParams(0, 0, 0)
                -- The native sliding-grip timeline writes Z limits every
                -- frame. Reassert this same policy on the fresh component,
                -- including when the retained `freed` flag is already true.
                PX.grip_limits(p,g,g.c,now_ms())
            else
                if g.stiff then g.c:SetAngularDriveParams(g.stiff, g.damp or 0, g.maxf or 0) end
                if g.freed and g.lim then
                    local l = g.lim
                    g.c:SetLinearXLimit(l[1], l[4]); g.c:SetLinearYLimit(l[2], l[4]); g.c:SetLinearZLimit(l[3], l[4])
                    g.c:SetAngularSwing1Limit(l[5], l[6]); g.c:SetAngularSwing2Limit(l[7], l[8]); g.c:SetAngularTwistLimit(l[9], l[10])
                    g.freed = nil
                end
            end
        end)
    end
end

-- Hand the stand-in back to its own muscles (no fresh pose).
local function release_standin(p, handoff)
    PX.settle_reset(p,"drive released")
    p.wservo,p.wservo_actor=nil,nil
    if PX.height_restore and p.body and p.body.height_probe and p.gen == world_gen and cache_gen == world_gen then
        PX.height_restore(p, "release", false)
    end
    local injury_ok=true
    if handoff and PX.injury_release then injury_ok=PX.injury_release(p) end
    if p.driving == false then return injury_ok end
    if p.gen == world_gen and cache_gen == world_gen and p.actor and p.actor:IsValid() then
        PX.grips_off(p, false); PX.close_limits(p, p.body)
        for k, v in pairs(p.tonus0 or {}) do pcall(function() p.actor[k] = v end) end
    end
    p.tonus0 = nil
    p.grips = nil
    -- Only ever write to a stand-in of the CURRENT world that is still alive.
    if p.gen ~= world_gen or cache_gen ~= world_gen or not (p.actor and p.actor:IsValid()) then
        p.driving = false
        return injury_ok
    end
    if p.body then
        release_handles(p.body); set_motor_strength(p.body, 1.0)
        -- Servo stand-in: hand gravity back (body + held weapons).
        if p.body.gravity == false then
            pcall(function() p.body.mesh:SetEnableGravity(true) end)
            p.body.wc_owner = p
            for _, w in pairs(PX.wc_valid(p.body)) do   -- validated, fresh roots only
                if w.root and w.root:IsValid() then pcall(function() w.root:SetEnableGravity(true) end) end
            end
            p.body.gravity = true
            if p.body.world_resp then
                pcall(function()
                    p.body.mesh:SetCollisionResponseToChannel(0, p.body.world_resp[1])
                    p.body.mesh:SetCollisionResponseToChannel(1, p.body.world_resp[2])
                end)
                p.body.world_resp = nil
            end
        end
        p.aim = nil
        -- A hand weapon whose collision the driver turned off gets it
        -- back (fresh lookup, matched by address: never a cached actor).
        for field, c in pairs(p.body.sv and p.body.sv.wc or {}) do
            if c.nocoll then
                local wa; pcall(function() wa = p.actor[field] end)
                local wad; pcall(function() wad = wa and wa:GetAddress() end)
                if wad and wad == c.addr then pcall(function() wa:SetActorEnableCollision(true) end) end
                c.nocoll = nil
            end
        end
        if p.body.ghost_until then   -- collision back on (see ghost())
            pcall(function() p.body.mesh:SetCollisionEnabled(p.body.coll0 or 3) end)
            p.body.ghost_until = nil
        end
    end
    set_muscles_blocked(p, false)
    p.driving = false
    if not handoff and PX.injury_targets and p.body then PX.injury_targets(p.peer or 0,p,nil,nil) end
    return injury_ok
end

-- Rigidly move every simulated mesh so its pelvis lands on the target.
local function snap_mesh(body, tx, ty, tz)
    local cur
    pcall(function() cur = body.mesh:GetSocketLocation(fname("Pelvis")) end)
    if not cur then return end
    local ox, oy, oz = tx - cur.X, ty - cur.Y, tz - cur.Z
    local meshes = (#body.sims > 0) and body.sims or { body.mesh }
    for _, m in ipairs(meshes) do
        pcall(function()
            local l = m:K2_GetComponentLocation()
            m:K2_SetWorldLocation({ X = l.X + ox, Y = l.Y + oy, Z = l.Z + oz }, false, {}, true)
            m:SetAllPhysicsLinearVelocity({ X = 0, Y = 0, Z = 0 }, false)
        end)
    end
    -- The servoed held weapons (their own simulated actors) move with the body.
    -- Left behind, the servo drags a polearm back through the snapped arm at
    -- its capped speed: the forearm ends up on the wrong side of the shaft,
    -- twisted ~173 deg with the hand ~24 uu off for the whole round.
    for _, c in pairs(PX.wc_valid(body)) do   -- validated, fresh roots only
        if c.sim and c.root then
            pcall(function()
                if not c.root:IsValid() then return end
                local l = c.root:K2_GetComponentLocation()
                c.root:K2_SetWorldLocation({ X = l.X + ox, Y = l.Y + oy, Z = l.Z + oz }, false, {}, true)
                c.root:SetPhysicsLinearVelocity({ X = 0, Y = 0, Z = 0 }, false, fname("None"))
                c.root:SetPhysicsAngularVelocityInDegrees({ X = 0, Y = 0, Z = 0 }, false, fname("None"))
            end)
        end
    end
    body.snaps = body.snaps + 1
end

-- Velocity-steering fallback (no PhysicsHandle): per bone, per frame.
local function clamp_vec(x, y, z, maxlen)
    local l = math.sqrt(x * x + y * y + z * z)
    if l > maxlen and l > 0 then local k = maxlen / l; return x * k, y * k, z * k end
    return x, y, z
end
local function steer_velocity(mesh, fn, tg)
    local cur, cq
    pcall(function() cur = mesh:GetSocketLocation(fn) end)
    pcall(function() cq = mesh:GetSocketQuaternion(fn) end)
    if not (cur and cq) then return end
    local vx, vy, vz = clamp_vec((tg[1] - cur.X) * POSE_K_POS, (tg[2] - cur.Y) * POSE_K_POS,
        (tg[3] - cur.Z) * POSE_K_POS, POSE_MAX_LIN)
    pcall(function() mesh:SetPhysicsLinearVelocity({ X = vx, Y = vy, Z = vz }, false, fn) end)
    local tx, ty, tz, tw = tg[4], tg[5], tg[6], tg[7]
    local cx, cy, cz, cw = -cq.X, -cq.Y, -cq.Z, cq.W
    local qw = tw * cw - tx * cx - ty * cy - tz * cz
    local qx = tw * cx + tx * cw + ty * cz - tz * cy
    local qy = tw * cy - tx * cz + ty * cw + tz * cx
    local qz = tw * cz + tx * cy - ty * cx + tz * cw
    if qw < 0 then qx, qy, qz, qw = -qx, -qy, -qz, -qw end
    local s = math.sqrt(math.max(0, 1 - qw * qw))
    if s > 1e-4 then
        local ang = math.deg(2 * math.acos(math.min(1, qw))) * POSE_K_ROT
        local ax, ay, az = clamp_vec(qx / s * ang, qy / s * ang, qz / s * ang, POSE_MAX_ANG)
        pcall(function() mesh:SetPhysicsAngularVelocityInDegrees({ X = ax, Y = ay, Z = az }, false, fn) end)
    end
end

-- Push the (retargeted) targets into the handles. Per frame; no reads.
-- Contact safety (otherwise characters fly across the map on the slightest
-- touch). A PhysicsHandle pulls its bone with a force that grows
-- with the distance to its target; when the stand-in's limb is blocked by the
-- local pawn the remote target keeps moving through it, the error grows and
-- the handle pushes the local body away with an ever larger force.
--   * lead clamp: a handle target is never further than LEAD_* uu from the
--     bone's current position, which bounds the force (stiffness x lead);
--   * blocked backoff: a bone whose error stays above BLOCK_ERR for BLOCK_MS
--     drops to BACKOFF x stiffness until it is back within UNBLOCK_ERR;
--   * no rigid snap of the stand-in into an overlapping local pawn;
--   * a last-resort clamp on the local pawn's velocity near a stand-in.
local LEAD_BODY, LEAD_LIMB = 30, 45
local BLOCK_ERR, UNBLOCK_ERR, BLOCK_MS, BACKOFF = 60, 25, 120, 0.2
local SNAP_CLEAR      = 90      -- uu between target pelvis and our pelvis
local LAUNCH_NEAR     = 160     -- uu: "touching" a stand-in
local LAUNCH_SPEED    = 1500    -- uu/s: a launch, not walking/fighting
local LAUNCH_KEEP     = 600     -- uu/s left after a clamp
local contact = { peak = 0, near_peak = 0, clamps = 0, backoffs = 0, snaps_held = 0, mind = math.huge,
                  last_clamp = -1e9, last_log = 0 }
local local_body = nil   -- { mesh, gen, addr } of OUR pawn's simulated mesh (this world only)

local function local_pelvis()
    if not local_body or local_body.gen ~= world_gen then return nil end
    local m = local_body.mesh
    if not (m and m:IsValid()) then return nil end
    local l; pcall(function() l = m:GetSocketLocation(fname("Pelvis")) end)
    return l, m
end

-- Never rigidly teleport a stand-in into our own body: the depenetration
-- impulse is what flings characters. The snap is retried next frame.
local function snap_clear(pel)
    local lp = local_pelvis()
    if not lp then return true end
    if PURE.d3(pel, { lp.X, lp.Y, lp.Z }) < SNAP_CLEAR then
        contact.snaps_held = contact.snaps_held + 1
        return false
    end
    return true
end

local function finite(x) return type(x) == "number" and x == x and x > -1e6 and x < 1e6 end

local function lead_target(cur, tg, lead)
    if not (finite(cur.X) and finite(cur.Y) and finite(cur.Z)) then return nil end
    local dx, dy, dz = tg[1] - cur.X, tg[2] - cur.Y, tg[3] - cur.Z
    local d = math.sqrt(dx * dx + dy * dy + dz * dz)
    if d <= lead then return tg[1], tg[2], tg[3], d end
    local k = lead / d
    return cur.X + dx * k, cur.Y + dy * k, cur.Z + dz * k, d
end

local function backoff(rec, tune, err, now)
    if err > BLOCK_ERR then
        rec.err_since = rec.err_since or now
        if not rec.soft and now - rec.err_since >= BLOCK_MS then
            tune_handle(rec.h, tune, BACKOFF)
            rec.soft = true
            contact.backoffs = contact.backoffs + 1
        end
    elseif err < UNBLOCK_ERR then
        rec.err_since = nil
        if rec.soft then tune_handle(rec.h, tune, 1); rec.soft = false end
    end
end

local function apply_pose(body, targets, now)
    local mesh = body.mesh
    now = now or now_ms()
    for _, bn in ipairs(body.bones) do
        local tg = targets[bn]
        if tg then
            local rec = body.handles[bn]
            if body.ctl == "handles" and rec and rec.ok then
                pcall(function()
                    local fn = fname(bn)
                    -- Never hand Chaos a NaN/inf/huge target.
                    for i = 1, 7 do if not finite(tg[i]) then return end end
                    local cur = mesh:GetSocketLocation(fn)
                    if not cur then return end
                    local x, y, z, err = lead_target(cur, tg, ARM_BONES[bn] and LEAD_LIMB or LEAD_BODY)
                    if not x then return end
                    backoff(rec, tune_for(bn), err, now)
                    rec.h:SetTargetLocationAndRotation({ X = x, Y = y, Z = z },
                        quat_to_rot(tg[4], tg[5], tg[6], tg[7]))
                end)
            elseif body.ctl == "velocity" or (rec and rec.nobody) then
                local okv = true
                for i = 1, 7 do if not finite(tg[i]) then okv = false end end
                if okv then steer_velocity(mesh, fname(bn), tg) end
            end
        end
    end
end

-- Pose error vs. targets (diagnostics only; costs one read per bone).
local function pose_error(body, targets)
    local bsum, bmax, bn_, asum, amax, an_, hmax = 0, 0, 0, 0, 0, 0, 0
    for _, bn in ipairs(body.bones) do
        local tg = targets[bn]
        local cur; pcall(function() cur = body.mesh:GetSocketLocation(fname(bn)) end)
        if tg and cur then
            local err = PURE.d3(tg, { cur.X, cur.Y, cur.Z })
            if ARM_BONES[bn] then
                asum, an_ = asum + err, an_ + 1; if err > amax then amax = err end
                if HAND_BONES[bn] and err > hmax then hmax = err end
            else
                bsum, bn_ = bsum + err, bn_ + 1; if err > bmax then bmax = err end
            end
        end
    end
    return (bn_ > 0) and bsum / bn_ or 0, bmax, (an_ > 0) and asum / an_ or 0, amax, hmax
end

-- Bus key "world_held" (HSMPWorld ~4 Hz; a typed record): rows {peer, nid, hand 0 = R / 1 = L, actor}.
-- Re-read at most every 250 ms; this runs every frame.
local _wh_data, _wh_at = nil, -1
local function world_held_for(id)
    local t = os.clock()
    if t - _wh_at >= 0.25 then
        _wh_at = t
        _wh_data = nil
        local t = HSMP_IPC and HSMP_IPC.bus_table("world_held")
        if type(t) == "table" then
            local d = {}
            for _, r in ipairs(t.rows or {}) do
                local e = d[tostring(r.peer)] or {}
                e[r.hand == 1 and "L" or "R"] = { r.nid }
                d[tostring(r.peer)] = e
            end
            _wh_data = d
        end
    end
    local e = _wh_data and _wh_data[tostring(id)]
    return type(e) == "table" and e or nil
end

-- Drive the stand-in's held weapon onto the remote weapon transform (already
-- re-anchored to the stand-in's hand target by PURE.retarget).
-- The held weapon is a separate actor whose lifetime other code owns
-- (HSMPLoadout swaps it, the game's "Set Up Right Hand Weapon" detaches and
-- replaces it, HSMPWorld moves world items). A PhysicsHandle still grabbing
-- a weapon body that gets destroyed between two frames leaves Chaos with a
-- dangling constraint (worker-thread access violation). So no handle on the
-- weapon: it follows the stiffly driven hand through the game's own hand
-- constraint. Set WEAPON_HANDLE = true only for experiments.
local WEAPON_HANDLE = false

local function apply_weapon(p, body, wt, hand)
    local function let_go()
        if body.wpn and body.wpn.h and body.wpn.h:IsValid() and body.wpn.ok then
            pcall(function() body.wpn.h:ReleaseComponent() end)
        end
        if body.wpn then body.wpn.ok = false end
    end
    if not WEAPON_HANDLE then let_go(); return "hand" end
    local wa = held_weapon_actor(p.actor, hand == "Hand_L")
    if not wa or not wt then let_go(); return "none" end
    body.wpn = body.wpn or {}
    if body.cache then body.cache.wpn = body.wpn end
    local w = body.wpn
    if not same(w.actor, wa) then
        let_go()
        w.actor, w.root, w.sim = wa, nil, nil
    end
    if not (w.root and w.root:IsValid()) then
        pcall(function() w.root = wa.RootComponent end)
        if not (w.root and w.root:IsValid()) then return "noroot" end
        pcall(function() w.sim = w.root:IsSimulatingPhysics(fname("None")) end)
    end
    if not w.sim then return "kinematic" end   -- attached kinematically: it follows the hand
    if not (w.h and w.h:IsValid()) then
        w.h = make_handle(p.actor, PH_WPN)
        if not w.h then return "nohandle" end
    end
    if not w.ok then
        local l, r
        pcall(function() l = wa:K2_GetActorLocation() end)
        pcall(function() r = wa:K2_GetActorRotation() end)
        if l and r then pcall(function() w.h:GrabComponentAtLocationWithRotation(w.root, fname("None"), l, r) end) end
        w.ok = grabbed(w.h)
        if not w.ok then return "nograb" end
    end
    pcall(function()
        local x, y, z = wt[1], wt[2], wt[3]
        local cur = wa:K2_GetActorLocation()
        if cur then
            local err
            x, y, z, err = lead_target(cur, wt, LEAD_LIMB)
            backoff(w, PH_WPN, err, now_ms())
        end
        w.h:SetTargetLocationAndRotation({ X = x, Y = y, Z = z }, quat_to_rot(wt[4], wt[5], wt[6], wt[7]))
    end)
    return "held"
end

-- Capsule / logical actor position. The visible ragdoll is not rigidly
-- attached to the capsule (moving the capsule leaves the bodies behind), so
-- this only places the actor for gameplay queries; the body follows the pose.
local function drive_root(actor, pos, yaw, force_teleport)
    local cur = willie_loc(actor)
    local teleport = force_teleport or not cur
    if not teleport then
        teleport = PURE.d3({ pos.X, pos.Y, pos.Z }, { cur.X, cur.Y, cur.Z }) > TELEPORT_DIST
    end
    pcall(function() actor:K2_SetActorLocation(pos, false, {}, teleport) end)
    pcall(function() actor:K2_SetActorRotation({ Pitch = 0, Yaw = yaw, Roll = 0 }, teleport) end)
end

-- --- puppets -----------------------------------------------------------------

local puppets     = {}   -- id -> { actor, nick, body, play, ... }
function PX.injury_diag(id,body,bone,stage)
    pcall(function()
        local f = fname(bone)
        local c = body.mesh:GetCenterOfMass(f)
        Log("bodyphysics evidence peer=%d mesh=%s bone=%s stage=%s com=(%.3f,%.3f,%.3f) mass=%.4f sim=%s disabled=%s",
            id,tostring(body.mesh:GetAddress()),bone,stage,c.X,c.Y,c.Z,
            body.mesh:GetBoneMass(f,false),tostring(body.mesh:IsSimulatingPhysics(f)),
            tostring(body.injury_disabled and body.injury_disabled[bone] == true))
    end)
end
function PX.bodyphysics(arg)
    local id, bone, action = arg:match("^(%d+)%s+([%w_]+)%s+(%a+)$")
    id = tonumber(id)
    local p = id and puppets[id]
    bone = bone and bone:lower()
    -- Diagnostic limbs only: never disable the pelvis or whole character.
    local allowed = {lowerarm_l=true,lowerarm_r=true,hand_l=true,hand_r=true,
        calf_l=true,calf_r=true,foot_l=true,foot_r=true}
    if not (p and p.gen == world_gen and p.actor and p.actor:IsValid() and p.body and p.body.mesh:IsValid()
        and allowed[bone] and (action == "off" or action == "on")) then
        Log("bodyphysics refused: expected current stand-in peer, distal limb, off|on")
        return
    end
    local body = p.body
    PX.injury_diag(id,body,bone,"before_" .. action)
    if action == "off" then
        body.injury_probe = {bone=bone,until_t=os.clock()+2}
    else
        body.injury_probe = nil
    end
    local want = action == "off" and {[bone]=true} or {}
    local err
    body.injury_disabled, err = INJURY.apply(body.mesh,body.injury_disabled,want,fname)
    Log("bodyphysics peer=%d mesh=%s bone=%s action=%s applied=%s error=%s auto_restore_s=2",
        id,tostring(body.mesh:GetAddress()),bone,action,tostring(body.injury_disabled[bone] == true),tostring(err))
    PX.injury_diag(id,body,bone,"after_" .. action)
end

-- Explicit dev command only: resize exactly one existing remote body for
-- two seconds, retaining its native physics asset/controls throughout.
function PX.height_resolve(p, s)
    if not (p and p.gen == world_gen and cache_gen == world_gen and p.body and s) then return nil end
    local actor, mesh
    pcall(function()
        local me = local_pawn()
        if not (me and me:IsValid()) then return end
        for _, w in pairs(FindAllOf("Willie_BP_C") or {}) do
            if w and w:IsValid() and not same(w, me) and w:GetAddress() == s.addr
                and w:GetFName():ToString() == s.name then
                local m = w.Mesh
                if m and m:IsValid() and m:GetAddress() == s.mesh_addr then actor, mesh = w, m end
                break
            end
        end
    end)
    return actor, mesh
end
function PX.height_restore(p, why, repose)
    local body = p.body
    local s = body and body.height_probe
    if not s then return true end
    if s.retry_at and os.clock() < s.retry_at then return false end
    local actor, mesh = PX.height_resolve(p, s)
    if not actor then
        -- Never write a previous world's object or a body now possessed by us.
        body.height_probe = nil
        Log("bodyheight restoration abandoned: target identity/ownership changed (%s)", tostring(why))
        return false
    end
    if repose then PX.start_repose(p, body, now_ms(), "height diagnostic restore") end
    local ok = BODY_HEIGHT.height_restore(actor, mesh, s)
    body.sv, body.scale_remeasure = nil, true
    p.aim, p.shown, p.qhist, p.idlew, p.qfoot = nil, nil, nil, nil, nil
    if not repose then
        body.repose = nil
        pcall(function() mesh:SetSimulatePhysics(s.sim) end)
    end
    if ok then
        body.height_probe = nil
    else
        s.retry_at = os.clock()+1 -- retain snapshot without resetting/logging every frame
    end
    Log("bodyheight peer=%d actor=%s mesh=%s restore=%s reason=%s original_height=%.9f",
        s.peer, s.name, tostring(s.mesh_addr), tostring(ok), tostring(why), s.height)
    return ok
end
function PX.height_tick(p, clock)
    local s = p.body and p.body.height_probe
    if p.gen == world_gen and s and (clock or os.clock()) >= s.until_t then
        PX.height_restore(p, "automatic 2-second timeout", p.driving == true)
    end
end
function PX.bodyheight(arg)
    local id, height = arg:match("^(%d+)%s+([%d%.]+)$")
    id, height = tonumber(id), tonumber(height)
    local p = id and puppets[id]
    if not (BODY_HEIGHT and p and p.gen == world_gen and p.driving and p.in_range and p.body
        and p.body.ctl == "servo" and not p.owner_dead and not p.body.height_probe and not p.body.repose
        and height and height >= 0 and height <= 1) then
        Log("bodyheight refused: expected active remote servo peer and native passport height 0..1")
        return
    end
    local s, err
    pcall(function()
        s = BODY_HEIGHT.height_snapshot(p.actor, p.body.mesh)
        if not s then return end
        s.addr, s.name, s.mesh_addr = p.actor:GetAddress(), p.actor:GetFName():ToString(), p.body.mesh:GetAddress()
        s.sim = p.body.mesh:IsSimulatingPhysics(fname("pelvis"))
    end)
    local actor, mesh = PX.height_resolve(p, s)
    if not actor or s.sim ~= true then Log("bodyheight refused: fresh identity or native snapshot unavailable"); return end
    s.peer, s.until_t = id, os.clock()+2
    p.body.height_probe = s -- retain restoration data before the first write
    PX.start_repose(p, p.body, now_ms(), "height diagnostic apply")
    if not p.body.repose then PX.height_restore(p, "physics-off refused", false); return end
    local ok
    ok, err = BODY_HEIGHT.height_probe(actor, mesh, height, s)
    p.body.sv, p.body.scale_remeasure = nil, true
    p.aim, p.shown, p.qhist, p.idlew, p.qfoot = nil, nil, nil, nil, nil
    Log("bodyheight peer=%d actor=%s mesh=%s height=%.9f original=%.9f applied=%s error=%s auto_restore_s=2",
        id, s.name, tostring(s.mesh_addr), height, s.height, tostring(ok), tostring(err))
    if not ok then PX.height_restore(p, "native write failed", false) end
end
-- Resolve the current component from its actor field before any injury write.
-- A world/actor/mesh replacement drops its bookkeeping without touching the old
-- UObject; only an exact current body may restore a previously owned exclusion.
function PX.injury_mesh(p)
    if not (p and p.gen==world_gen and cache_gen==world_gen and p.body) then return nil,nil,"world changed" end
    local mesh,key,why
    local ok=pcall(function()
        local a=p.actor
        if not p.addr then why="body unavailable";return end
        if type(p.body.mesh_addr)~="number" or p.body.mesh_addr<=0 or not math.tointeger(p.body.mesh_addr)
            or type(p.body.mesh_fname)~="string" or p.body.mesh_fname==""then why="body unavailable";return end
        if not (a and a:IsValid()) or a:GetAddress()~=p.addr then why="actor changed";return end
        local w=a:GetWorld()
        if not (w and w:IsValid()) then why="body unavailable";return end
        local wid=tostring(w:GetAddress()).."@"..w:GetFullName()
        if cache_world~=tostring(world_gen).."|"..wid then why="world changed";return end
        local m=a[p.body.field or "Mesh"]
        if not (m and m:IsValid()) then why="body unavailable";return end
        -- Compare only the fresh field against the captured scalar identity.
        -- The retained wrapper may already be freed after a native rebuild.
        if m:GetAddress()~=p.body.mesh_addr or m:GetFName():ToString()~=p.body.mesh_fname then
            why="mesh changed";return
        end
        mesh=m
        key=a:GetFName():ToString().."@"..tostring(a:GetAddress())
            ..":"..m:GetFName():ToString().."@"..tostring(m:GetAddress())
    end)
    return mesh,key,(not ok and "body unavailable" or why)
end
function PX.injury_release(p)
    local body=p.body
    if not body then return true end
    local mesh,key,why=PX.injury_mesh(p)
    if not mesh or (body.injury_key and body.injury_key~=key) then
        if why=="body unavailable" and next(body.injury_disabled or {}) then
            body.injury_retiring,body.injury_error=true,why
            return false -- unknown is not a lease handoff: retain exact restoration ownership
        end
        body.injury_disabled,body.injury_wanted,body.injury_journal={},{},nil
        body.injury_probe,body.injury_retiring=nil,nil
        return true -- identity lost: no old UObject access
    end
    body.injury_disabled=INJURY.restore(mesh,body.injury_disabled,fname)
    body.injury_probe=nil
    body.injury_retiring=next(body.injury_disabled)~=nil
    if body.injury_retiring then
        body.injury_error="restore failed"
        if body.injury_diag_key~="restore failed" then
            body.injury_diag_key="restore failed"
            Log("sever physics peer=%d body=%s restore failed; lease retained",p.peer or 0,key)
        end
        return false
    end
    body.injury_error=nil
    body.injury_wanted,body.injury_journal,body.injury_diag_key={},nil,nil
    return true
end
function PX.injury_targets(id,p,targets,aim)
    local body = p.body
    if not body then return end
    if not SEVERED_PHYSICS and not body.injury_probe and not next(body.injury_disabled or {}) then return end
    local mesh,key,why=PX.injury_mesh(p)
    if not mesh then
        body.injury_error=why
        INJURY.omit(aim,body.injury_wanted,PURE.V2_SLOTS,PURE.V2_PARENT)
        INJURY.omit(targets,body.injury_wanted,PURE.V2_SLOTS,PURE.V2_PARENT)
        INJURY.omit(aim,body.injury_disabled,PURE.V2_SLOTS,PURE.V2_PARENT)
        INJURY.omit(targets,body.injury_disabled,PURE.V2_SLOTS,PURE.V2_PARENT)
        if next(body.injury_wanted or {}) and body.injury_diag_key~=why then
            body.injury_diag_key=why;Log("sever physics peer=%d unavailable=%s (no native write)",id,tostring(why))
        end
        return
    end
    if body.injury_key and body.injury_key~=key then
        body.injury_disabled,body.injury_wanted,body.injury_journal={},{},nil
    end
    body.injury_key=key
    if body.injury_retiring then return end
    local want = body.injury_wanted or {}
    if SEVERED_PHYSICS then
        local r = HSMP_IPC and HSMP_IPC.peer_rec("peer_vitals",id)
        local shown=p.shown or p.applied_context
        local view=HSM and HSM.view()
        local allowed=shown and shown.has_context==true and (shown.match_id or 0)>0
            and (shown.round or 0)>0 and (shown.life or 0)>0
            and shown.pawn==p.actor:GetFName():ToString()
            and view and (view.match_id or 0)>0
            and PURE.pose_context_ok(shown,view,HSM and HSM.mode(),id)
        local mask
        body.injury_journal,mask=INJURY.select_mask(body.injury_journal,shown,allowed,r,key)
        local enums = HSMP_IPC and HSMP_IPC.S and HSMP_IPC.S.ENUMS and HSMP_IPC.S.ENUMS.dism_part
        -- Zero has no source availability bit today. It cannot prove that a
        -- previously missing part regrew; a true lease handoff restores it.
        if mask and mask>0 and enums then
            local retained={};for bone in pairs(want)do retained[bone]=true end
            want=retained
            for name,bit in pairs(enums) do
                if mask & (1 << bit) ~= 0 then want[name:lower()] = true end
            end
            body.injury_wanted=want
        end
    end
    local probe = body.injury_probe
    local restored_bone
    if probe and os.clock() < probe.until_t then
        local copy={};for bone in pairs(want)do copy[bone]=true end
        want=copy;want[probe.bone] = true
    elseif probe then
        restored_bone = probe.bone
        body.injury_probe = nil; Log("bodyphysics peer=%d automatic restore",id)
    end
    local err
    -- Reassert even with unchanged bookkeeping: collision/simulation toggles
    -- and native appearance setup may rebuild physics on this same component.
    body.injury_disabled,err = INJURY.apply(mesh,body.injury_disabled,want,fname,true)
    body.injury_error=err
    local signature=tostring(body.injury_journal and body.injury_journal.mask).."|"..tostring(err)
    if next(want) and (signature~=body.injury_diag_key or now_ms()>=(body.injury_diag_at or 0)) then
        body.injury_diag_key,body.injury_diag_at=signature,now_ms()+1000
        local samples=INJURY.simulation(mesh,want,PURE.V2_SLOTS,PURE.V2_PARENT,fname)
        local parts={};for bone,value in pairs(samples)do parts[#parts+1]=bone.."="..value end;table.sort(parts)
        Log("sever physics peer=%d body=%s match=%s round=%s life=%s mask=%s simulation=%s collision=unverified error=%s",
            id,key,tostring(body.injury_journal and body.injury_journal.match_id),tostring(body.injury_journal and body.injury_journal.round),
            tostring(body.injury_journal and body.injury_journal.life),tostring(body.injury_journal and body.injury_journal.mask),table.concat(parts,","),tostring(err))
    end
    if restored_bone then PX.injury_diag(id,body,restored_bone,"automatic_restore") end
    -- Owner-confirmed absence controls servo omission even if native exclusion
    -- throws; failed restoration also keeps the still-disabled roots omitted.
    INJURY.omit(aim,want,PURE.V2_SLOTS,PURE.V2_PARENT)
    INJURY.omit(targets,want,PURE.V2_SLOTS,PURE.V2_PARENT)
    INJURY.omit(aim,body.injury_disabled,PURE.V2_SLOTS,PURE.V2_PARENT)
    INJURY.omit(targets,body.injury_disabled,PURE.V2_SLOTS,PURE.V2_PARENT)
end
function PX.injury_tick(id,p)
    if p.body and p.body.injury_retiring then return PX.injury_release(p) end
    PX.injury_targets(id,p,nil,nil)
end
local _driven     = {}   -- actor address -> driven stand-in (ReceiveTick post-hook)
local next_claim  = {}   -- id -> tick of next claim attempt
local warned_none = {}   -- id -> true once "no combatant" was logged
local spawn_calls, last_spawn_call = 0, -9999
local last_world  = nil
local _last_puppets_line = nil
local census = { removed = 0, logged_key = nil, armed_at = nil, seen = {}, gone = {}, arena_at = os.clock() }
PX.arena_at_fn = function() return census.arena_at or 0 end

local function is_claimed(actor)
    for _, p in pairs(puppets) do
        if same(p.actor, actor) then return true end
    end
    return false
end

-- Living, intact, non-local, unclaimed, not the origin placeholder.
local function candidate_willies()
    local me = local_pawn()
    local ai, idle = {}, {}
    pcall(function()
        local ws = FindAllOf("Willie_BP_C")
        if not ws then return end
        for _, w in pairs(ws) do
            if w and w:IsValid() and not same(w, me) and not is_claimed(w) then
                local hp = willie_health(w)
                local l = willie_loc(w)
                if (hp == nil or hp > 0) and not at_origin(l)
                   and not willie_dead(w) and not willie_dismembered(w) then
                    local cls, ctrl = controller_class(w)
                    if cls == "" then
                        table.insert(idle, w)
                    elseif not cls:find("PlayerController") then
                        table.insert(ai, { w = w, ctrl = ctrl, cls = cls })
                    end
                end
            end
        end
    end)
    return ai, idle
end

-- A native stand-in spawn may possess its new body even while the original
-- fighter is AI-driven. Release that foreign possession without stealing the
-- original fighter back from its verified AI controller. Human control keeps
-- the normal restoration path.
function PX.restore_spawn_possession(pawn, pc)
    if not (pc and pc:IsValid() and pawn and pawn:IsValid() and (willie_health(pawn) or 1) > 0) then return nil end
    if TUNE.aip == nil then TUNE.aip = load_module("hsmp_wg") or false end
    local ai = TUNE.aip and TUNE.aip.ai_pawn_lookup() or nil
    if ai and same(ai, pawn) then
        pc:UnPossess()
        return "released the temporary pawn; original fighter keeps its AI"
    end
    pc:Possess(pawn)
    return "re-possessed the original pawn"
end

-- Last resort: have the arena spawn more natively booted foes.
-- count: how many bodies are missing in total (every fresh peer without a
-- stand-in, not the running count of the claim loop); ats: the poses of the
-- peers that need one (a list of {X,Y,Z}; a single {X,Y,Z} is accepted too).
local function request_native_foes(count, ats, peers)
    local live = PQ.live()
    if spawn_calls >= SPAWN_MAX + (peers or 0)
        or tick_num - last_spawn_call < (live and SPAWN_COOLDOWN_LIVE or SPAWN_COOLDOWN) then
        return
    end
    -- The native spawn swaps the local possession to its new player pawn (that
    -- is how the old pawn becomes a free combatant to puppet). In Live that
    -- swaps the player mid-round. Blocking the call in Live entirely is no
    -- answer either: a 3+ player match would then keep an invisible,
    -- unhittable opponent for the whole round when the bodies were asked for
    -- one at a time. So every missing body is requested at once, a pooled
    -- idle body (a released / census-removed Willie, claim_puppet) is always
    -- preferred, and in Live the call is allowed but rarer, and only while our
    -- own pawn is alive and possessed: PX.keep_possession hands the original
    -- pawn back within the 3 s guard, and HSMPSync does not place pawns in Live.
    if live then
        local me = local_pawn()
        if not (me and me:IsValid() and (willie_health(me) or 1) > 0) then return end
    end
    if ats and ats.X then ats = { ats } end
    ats = ats or {}
    local at = ats[1]
    -- In MP the arenas never spawn a foe (Free Mode is off): this fallback is
    -- the only source of a stand-in body. Its native spawn puts a new 'player'
    -- pawn on the native player spawner and possesses it. Called right at the
    -- world load, that spawner still holds our own pawn: the new one spawns
    -- into it and lies knocked down, and the freed one often dies (no
    -- stand-in), on every arena but Alley. So wait until the spawn placement
    -- has moved our pawn to its slot; PX.keep_possession then hands the player
    -- back their placed pawn and the new body becomes the stand-in.
    -- HSMPSync's verified placement of THIS world ends the wait early: a fixed
    -- grace alone makes the stand-in late on a slow instance (LordsHall at
    -- ~20 fps: body spawned ~8 s into the world, still hidden for its kit when
    -- the Director's census runs).
    if os.clock() - (census.arena_at or 0) < PX.FALLBACK_GRACE_S and not PX.own_placed() then return end
    local lm
    pcall(function()
        local lms = FindAllOf("BP_LevelManager_C")
        if lms then for _, x in pairs(lms) do if x and x:IsValid() then lm = x; break end end end
    end)
    if not lm then return end
    spawn_calls, last_spawn_call = spawn_calls + 1, tick_num
    local pc, before = local_pc(), local_pawn()
    -- Tell HSMPSync's placer that the possession swap the native spawn is about
    -- to make is transient (spawn_place.lua P:transient_swap): it keeps the
    -- placement and protection of the pawn we hand back.
    pcall(function()
        local nm = before and before:IsValid() and before:GetFName():ToString()
        if not nm then return end
        if HSMP_IPC then HSMP_IPC.bus_put("fallback_swap", { until_s = os.clock() + 3.0, keep = nm }) end
    end)
    -- Spawn the body where the stand-in must stand anyway (the peer's pose), not
    -- on the native spawner: the server can put a fighter's slot right there
    -- (Pit slot 0 = the spawner's only spawn point), and a body spawned into a
    -- placed pawn / its polearm comes in dead (HP 0) or knocks the pawn down.
    -- How BP_SpawnerPoint_Willies picks the spot (from its bytecode): "Spawn
    -- Willies" waits one frame (Delay 0), re-traces a
    -- 100 cm grid inside its Sphere for ground points ("Spawn Locations Pre
    -- Purge", which also keeps every earlier hit), then moves random Pre Purge
    -- entries into "Spawn Locations" and spawns at one of them (+ height);
    -- with no Pre Purge entry at all it uses the actor location + 100. So
    -- writing "Spawn Locations" or only moving the actor changes nothing (the
    -- bodies still land on the native spots). Steering that holds: move every
    -- live spawner (the arena destroys unused ones; FindAllOf
    -- still lists those at (0,0,0)) to the peer's pose, shrink its Sphere to
    -- 1 cm so no grid trace hits, and clear both lists. Local and client-only;
    -- the spawners are not used again in this world (MP never spawns foes) and
    -- a world reload restores them.
    local steered = 0
    if at and at.X then
        pcall(function()
            for _, sp in pairs(FindAllOf("BP_SpawnerPoint_Willies_C") or {}) do
                if sp and sp:IsValid() then
                    local l = willie_loc(sp)
                    if l and not at_origin(l) then
                        -- Spawner k goes to the k-th missing peer's pose
                        -- (several bodies must not spawn stacked on one spot);
                        -- more spawners than peers: staggered 150 uu apart.
                        local k = steered
                        local base = ats[(k % #ats) + 1]
                        local lap = math.floor(k / #ats)
                        local at = { X = base.X + 150 * lap, Y = base.Y, Z = base.Z }
                        local ok = pcall(function()
                            -- spawn Z = actor Z + 100 (+ height <= 10); at.Z is
                            -- the peer's capsule centre, ~90 above its ground
                            sp:K2_SetActorLocation({ X = at.X, Y = at.Y, Z = at.Z - 70 }, false, {}, true)
                            sp.Sphere:SetSphereRadius(1.0, false)
                            sp["Spawn Locations Pre Purge"] = {}
                            sp["Spawn Locations"] = {}
                        end)
                        if ok then steered = steered + 1 end
                    end
                end
            end
        end)
        Log("fallback: %d native spawner(s) steered to %d peer pose(s), first (%.0f,%.0f,%.0f)%s",
            steered, #ats, at.X, at.Y, at.Z, live and " [round Live]" or "")
    end
    -- Real (space-containing) property names; CamelCase is a silent no-op.
    pcall(function() lm["Player Spawned"] = true end)     -- don't spawn another us
    pcall(function() lm["P2 Spawned"] = true end)
    pcall(function() lm["Amount of Characters to Spawn"] = count end)
    -- BP function names contain spaces; the CamelCase form silently no-ops.
    local ok = pcall(function() lm["Spawn Combatants"](lm) end)
    Log("fallback: BP_LevelManager:SpawnCombatants(%d) call %d/%d ok=%s",
        count, spawn_calls, SPAWN_MAX, tostring(ok))
    local after; pcall(function() after = pc and pc.Pawn end)
    if pc and before and before:IsValid() and after and after:IsValid() and not same(after, before) then
        local ok, how = pcall(PX.restore_spawn_possession, before, pc)
        Log("fallback: SpawnCombatants changed our possession; %s", ok and how or "original pawn unavailable")
    end
    -- The native spawn possesses its new pawn a few frames LATER (36 ms after
    -- this call, past the check above). Guard the original (already placed)
    -- possession for 3 s; the new, unpossessed body is then claimed as the
    -- stand-in by the regular idle-combatant path.
    if before and before:IsValid() then
        PX.keep_pawn = { pawn = before, gen = world_gen, until_tick = tick_num + 90 }
    end
end

-- Called every tick (on_tick, after the world guard): undo a possession swap
-- caused by our SpawnCombatants fallback while its guard window is open.
function PX.keep_possession()
    local k = PX.keep_pawn
    if not k then return end
    if k.gen ~= world_gen or tick_num > k.until_tick then PX.keep_pawn = nil; return end
    local pc, cur = local_pc(), nil
    pcall(function() cur = pc and pc.Pawn end)
    if cur and cur:IsValid() and not same(cur, k.pawn) then
        local ok, how = pcall(PX.restore_spawn_possession, k.pawn, pc)
        Log("fallback: native spawn swapped our possession; %s", ok and how or "original pawn gone - kept the new one")
        PX.keep_pawn = nil
    end
end

-- needed / ats / peers: the whole tick's demand, see request_native_foes.
local function claim_puppet(id, needed, ats, peers)
    local ai, idle = candidate_willies()
    local pick, how
    if ai[1] then
        pick = ai[1].w
        pcall(function() ai[1].ctrl:UnPossess() end)
        how = "AI " .. ai[1].cls .. " detached"
    elseif idle[1] then
        pick = idle[1]
        how = "idle combatant"
    end
    if pick then
        harden_standin({actor=pick}) -- Before collision/visibility and the first combat tick.
        Log("peer %d -> puppet %s (%s)", id, pick:GetFName():ToString(), how)
        return pick
    end
    -- Where to spawn the body: the peer's spawn order for this round (where
    -- it will stand), not its last pose: before the peer's own placement that
    -- pose is the native player spawner, which can be OUR slot (LordsHall slot
    -- 0): a body spawned into our placed pawn shoves it ~2 m off its spawn
    -- right before Ready.
    local s = snap[id]
    if not (ats and #ats > 0) then
        local at = PX.spawn_spot(id, s)
        if not at then return nil end
        ats = { at }
    end
    request_native_foes(math.max(needed or 1, 1), ats, peers)
    return nil
end

-- Once every live peer has a puppet, stop unclaimed AI foes from fighting:
-- they exist only in this client's world and are not replicated.
local function neutralise_loose_ai()
    local ai = candidate_willies()
    for _, a in ipairs(ai) do
        pcall(function() a.ctrl:UnPossess() end)
        -- Hide it: unreplicated NPCs must not be visible or hittable in PvP.
        -- Not destroyed: that can trip the arena's "all enemies dead" flow.
        pcall(function() a.w:SetActorHiddenInGame(true) end)
        pcall(function() a.w:SetActorEnableCollision(false) end)
        Log("neutralised unreplicated AI foe %s (%s detached, hidden, no collision)",
            a.w:GetFName():ToString(), a.cls)
    end
end

-- --- census + ghost removal (MP sessions) ----------------------------------------
-- In an MP session the only Willies that may exist in this world are our own
-- pawn and one claimed stand-in per live peer. Anything else (an extra native
-- foe, a stand-in released by a peer that left, the corpse of a stand-in that
-- was re-claimed, a late arena/LevelManager spawn) is unreplicated: other
-- players can't see it, yet here it stands around, blocks and gets hit. It is
-- destroyed together with its AI controller and the weapons attached to it.
-- Runs only once every live peer has a stand-in (never steals a claim
-- candidate) and a few seconds after the world loaded (native spawns settle).

local CENSUS_EVERY   = 30     -- ticks (~1 s)
local CENSUS_GRACE_S = 4.0    -- after world load before anything is removed
local CARRIED_FIELDS = { "Weapon R", "Weapon L", "Weapon R_0", "Weapon L_0",
    "Weapon Slot R 1", "Weapon Slot R 2", "Weapon Slot L 1", "Weapon Slot L 2", "Weapon Slot Back" }

local function obj_name(o)
    local n = "?"; pcall(function() n = o:GetFName():ToString() end); return n
end

local function describe_willie(w, role)
    local cls = controller_class(w)
    local l = willie_loc(w)
    local hidden = false; pcall(function() hidden = w.bHidden == true end)
    return string.format("%s role=%s ctrl=%s loc=(%s) HP=%s dead=%s hidden=%s",
        obj_name(w), role, cls ~= "" and cls or "none",
        l and string.format("%.0f,%.0f,%.0f", l.X, l.Y, l.Z) or "?",
        tostring(willie_health(w)), tostring(willie_dead(w)), tostring(hidden))
end

-- Half Sword pools its Willies: K2_DestroyActor on one is a silent no-op that
-- only snaps it back to the world origin (verified in game; the
-- level-placed pool entry Willie_BP_C_0, tag "Persistent", sits there from the
-- start). So an extra is taken out of the game instead: AI controller
-- detached and destroyed, carried weapons destroyed, body frozen (no physics,
-- no tick), hidden and without collision. It can't be seen, hit or collide,
-- and the arena's own bookkeeping still finds a (living) Willie object.
local function remove_willie(w)
    -- Release any handle of ours on this body BEFORE freezing/hiding it.
    local key; pcall(function() key = w:GetAddress() end)
    local hs = key and handle_cache[key]
    if hs then
        for _, rec in pairs(hs.bones or {}) do
            if rec.h and rec.h:IsValid() and grabbed(rec.h) then pcall(function() rec.h:ReleaseComponent() end) end
            rec.ok = false
        end
        if hs.wpn and hs.wpn.h and hs.wpn.h:IsValid() and grabbed(hs.wpn.h) then pcall(function() hs.wpn.h:ReleaseComponent() end) end
    end
    local cls, ctrl = controller_class(w)
    local nw = 0
    for _, f in ipairs(CARRIED_FIELDS) do
        local x; pcall(function() x = w[f] end)
        if x and x:IsValid() then
            local par; pcall(function() par = x:GetAttachParentActor() end)
            if par and same(par, w) then
                pcall(function() x:SetActorHiddenInGame(true) end)
                pcall(function() x:SetActorEnableCollision(false) end)
                if pcall(function() x:K2_DestroyActor() end) then nw = nw + 1 end -- unsafe: ok x is a weapon actor from CARRIED_FIELDS attached to w, never a Willie
            end
        end
    end
    if ctrl and ctrl:IsValid() and not cls:find("PlayerController") then
        pcall(function() ctrl:UnPossess() end)
        pcall(function() ctrl:K2_DestroyActor() end) -- unsafe: ok an AI controller (PlayerControllers excluded above), never a Willie
    end
    for _, field in ipairs(MESH_FIELDS) do
        local m; pcall(function() m = w[field] end)
        if m and m:IsValid() then
            pcall(function() m:SetSimulatePhysics(false) end)
            pcall(function() m:SetAllPhysicsLinearVelocity({ X = 0, Y = 0, Z = 0 }, false) end)
        end
    end
    local ok = pcall(function() w:SetActorHiddenInGame(true) end)
    pcall(function() w:SetActorEnableCollision(false) end)
    pcall(function() w:SetActorTickEnabled(false) end)
    return ok, nw
end

-- A Willie taken by a stand-in must be visible and solid again (a pooled or
-- previously removed one can be reused by the game or by a re-claim).
-- Only un-hides a body the census hid: a newborn Willie hidden by HSMPLoadout
-- stays hidden until its kit is verified there.
local function restore_willie(w, census_hidden)
    if census_hidden then pcall(function() w:SetActorHiddenInGame(false) end) end
    pcall(function() w:SetActorEnableCollision(true) end)
    pcall(function() w:SetActorTickEnabled(true) end)
end

local function is_hidden(w)
    local h = false; pcall(function() h = w.bHidden == true end); return h
end

local function census_tick(me, live_count, missing)
    local active = mp_session_active()
    local all
    pcall(function() all = FindAllOf("Willie_BP_C") end)
    local nlocal, npup, extras = 0, 0, {}
    for _, w in pairs(all or {}) do
        if w and w:IsValid() then
            local pending = false
            pcall(function() pending = w:IsActorBeingDestroyed() end)
            if same(w, me) then nlocal = nlocal + 1
            elseif is_claimed(w) then npup = npup + 1
            elseif not pending then extras[#extras + 1] = w end
        end
    end
    local may_remove = active and missing == 0
        and os.clock() - (census.arena_at or 0) >= CENSUS_GRACE_S
    local removed_now = 0
    local left = 0
    for _, w in ipairs(extras) do
        local nm = obj_name(w)
        if not census.seen[nm] then
            census.seen[nm] = true
            Log("census: extra Willie %s", describe_willie(w, "other"))
        end
        if census.gone[nm] and is_hidden(w) then
            -- already taken out
        elseif may_remove then
            local ok, nw = remove_willie(w)
            if ok and is_hidden(w) then
                if not census.gone[nm] then census.removed = census.removed + 1 end
                census.gone[nm] = true
                removed_now = removed_now + 1
                Log("census: removed %s (hidden, frozen, no collision, AI + %d carried weapon(s) destroyed)", nm, nw)
            else
                left = left + 1
            end
        else
            left = left + 1
        end
    end
    local ck = string.format("%d|%d|%d|%d|%d", nlocal, npup, left, census.removed, live_count)
    if ck ~= census.logged_key then
        census.logged_key = ck
        Log("census: local=%d puppets=%d peers=%d extras=%d removed=%d%s",
            nlocal, npup, live_count, left, census.removed,
            active and "" or " (no MP session: nothing removed)")
    end
end

-- Bus key "puppets" (typed: { rows = { { peer = <id>, name = "<actor FName>" }, ... } }), read
-- by HSMPCombat, HSMPLoadout, HSMPInteract, HSMPWorld and HSMPMatch. `key` is the rows'
-- signature; remembered only once written (a failed write is retried next time).
local function write_puppets(key, rows)
    local ok = HSMP_IPC and HSMP_IPC.bus_put("puppets", { rows = rows })
    if ok then _last_puppets_line = key end
    return ok and true or false
end

-- Forget every cached UObject WITHOUT touching it (world gone / changing).
-- Pure Lua + bus writes: safe inside the LoadMap hook.
drop_caches = function(reason)
    for _, p in pairs(puppets) do
        if p.body and p.body.height_probe then Log("bodyheight probe discarded at world teardown (no old UObject access)") end
    end
    PX.bodyheight_request = nil
    PX.weaponstate_request = nil
    if next(puppets) ~= nil then Log("dropping all puppet caches (no UE access): %s", reason) end
    puppets, next_claim, warned_none = {}, {}, {}
    _driven = {}
    spawn_calls, last_spawn_call = 0, -9999
    PX.spawn_near_logged = nil
    for k in pairs(handle_cache) do handle_cache[k] = nil end
    PH_CLASS = nil
    PX.gs = nil           -- re-found on the next frame (a CDO, but never kept across worlds)
    local_body = nil
    census.removed, census.logged_key, census.armed_at, census.seen, census.gone = 0, nil, nil, {}, {}
    census.arena_at = os.clock()
    -- Other mods (HSMPLoadout, HSMPCombat) must not look up old stand-ins.
    if _last_puppets_line ~= "" then write_puppets("", {}) end
    if HSMP_IPC then HSMP_IPC.bus_put("playback", {}) end
end

-- Same world, deliberate stop (left gameplay, avatars switched off): the
-- stand-ins are alive, so hand them back before forgetting them.
local function reset_all(reason)
    if next(puppets) ~= nil then Log("releasing all puppets: %s", reason) end
    if cache_gen == world_gen then
        local restored=true
        for _, p in pairs(puppets) do
            local ok,done=pcall(release_standin,p,true)
            if not ok or done==false then restored=false end
        end
        if not restored then return false end -- retry while these bodies still belong to us
    end
    drop_caches(reason)
end

-- --- per-frame pose driver -----------------------------------------------------

-- Per peer, the slot / gen / slot-seq last read from PeerPlay.
PX.play = { out = {}, key = {}, seq = {}, bufs = {}, last = {} }
local function read_play(id, last_seq)
    local IPC = HSMP_IPC
    if IPC and IPC.use("peer_play") then
        local slot, e = IPC.peer_slot(id)
        if not slot then return nil end
        local P = PX.play
        local key = tostring(slot) .. ":" .. tostring(e.gen)
        if P.key[id] ~= key then P.key[id], P.seq[id] = key, nil end
        local seq = IPC.peer_play(slot, P.out, P.seq[id])
        -- PeerPlay is evaluated by a wall-clock sidecar thread. A rendered
        -- frame can already be late when this slot is read; its physical
        -- frame timestamp is therefore not the slot's receipt timestamp.
        local read_at=now_ms()
        if seq == nil then
            if P.seq[id] ~= nil and last_seq ~= nil then return "same" end   -- unchanged
            return nil                                                       -- empty / stale epoch
        end
        P.seq[id] = seq
        if not PURE.pose_context_ok(P.out, HSM and HSM.view(), HSM and HSM.mode(), id) then
            P.last[id], P.seq[id] = nil, nil
            return "context"
        end
        if last_seq ~= nil and P.out.seq == last_seq then return "same" end
        -- two tables per peer, alternating: the caller keeps the last one (p.last)
        -- and may still hold the one before (last frame's aim) this frame
        local bufs = P.bufs[id]
        if not bufs then bufs = { {}, {} }; P.bufs[id] = bufs end
        local into = (P.last[id] == bufs[1]) and bufs[2] or bufs[1]
        if not into.slots then into.slots, into.weapons = {}, {} end
        local t = PURE.play_from_out(P.out, into)
        if t then t.read_at=read_at; P.last[id] = t end
        return t
    end
    return nil
end

-- --- codec-v2 velocity servo --------------------------------------------------------
local _sv_fn = {}
for i, bn in ipairs(PURE.V2_SLOTS) do _sv_fn[i] = fname(bn) end

-- Joint limits of a driven stand-in are opened (TUNE "limits", deg, default
-- 150; 0 = off). Measured on the probe: an owner gripping weapons has elbows
-- at 105-107 deg and wrists at 55-59 deg - past the physics asset's limits
-- (the grip constraints pull him there) - while his stand-in stopped at
-- 95-97 / 46-48 deg: both hands ~50 deg off target, capped every frame. The
-- servo drives every body, so the limits are not needed to hold the pose;
-- on release the physics asset's own profile comes back (native ragdoll).

-- Clean (re)start of a stand-in: physics off for one frame so every body takes
-- the animated pose, then (drive_frame, re-pose second half) physics on and the
-- mesh snapped onto the target pelvis. `why` (drive start, discontinuity)
-- also starts the soft servo ramp and the spawn stretch watch.
function PX.start_repose(p, body, now, why)
    PX.settle_reset(p,"repose")
    p.wservo,p.wservo_actor=nil,nil
    body.repose_at = now
    body.reposes = (body.reposes or 0) + 1
    if why then
        body.repose_start = why
        Log("pose: stand-in of %s: %s: clean start (physics reset, snap onto the target pelvis)", tostring(p.nick), why)
    end
    if pcall(function() body.mesh:SetSimulatePhysics(false) end) then body.repose = true end
end

-- Geometry changes belong to the avatar driver: resizing a live mesh while
-- retaining old COM/parent-offset caches would pull every joint apart.
function PX.sync_body_scale(id, p, body, now, pose)
    local r = HSMP_IPC and HSMP_IPC.peer_rec("peer_body2", id)
    if not pose or pose.has_context ~= true or type(r) ~= "table" or r.match_id == 0
        or r.match_id ~= pose.match_id or r.round ~= pose.round or r.life ~= pose.life
        or not r.life or r.life < 1 or type(r.pawn) ~= "string" or r.pawn == "" then return false end
    local s = type(r) == "table" and r.char_scale
    if type(s) ~= "table" then return false end
    for i = 1, 3 do
        if type(s[i]) ~= "number" or s[i] ~= s[i] or s[i] <= 0 or s[i] > 16 then return false end
    end
    local c
    pcall(function() c = body.mesh:K2_GetComponentScale() end)
    if not c then return false end
    if math.abs(c.X-s[1]) <= 0.001 and math.abs(c.Y-s[2]) <= 0.001 and math.abs(c.Z-s[3]) <= 0.001 then return false end
    PX.start_repose(p, body, now, "owner body scale changed")
    if not body.repose then return false end
    local ok = pcall(function() body.mesh:SetWorldScale3D({ X=s[1], Y=s[2], Z=s[3] }) end)
    if not ok then return true end -- complete the physics reset even after a refused write
    -- Keep existing bodies: SetPhysicsAsset(force=true) destroys the native
    -- character's active physics-control bindings (verified live: both
    -- stand-ins reached 20–130 cm / 90–155 degree tracking errors).
    Log("pose: owner body scale %.3f/%.3f/%.3f applied; existing physics bodies retained", s[1], s[2], s[3])
    body.sv, body.scale_remeasure = nil, true
    p.aim, p.shown, p.qhist, p.idlew, p.qfoot = nil, nil, nil, nil, nil
    return true
end

-- How long after a clean start the servo is soft, and the stretch watch.
PX.RAMP_MS = 300
PX.STRETCH_WATCH_MS = 3000
PX.PAWN_WATCH_MS = 8000

-- One frame of the spawn stretch watch of stand-in `p` (positions of its bodies
-- this frame); emits `spawn_stretch` once the window is over.
function PX.watch_stretch(id, p, sv, cpos, now)
    local w = p.spawn_watch
    if not w then return end
    local d, b = PURE.stretch(cpos, sv.ref_len or {})
    if d > w.max then w.max, w.bone = d, PURE.V2_SLOTS[b] or "-" end
    w.n = (w.n or 0) + 1
    if now - w.t0 >= PX.STRETCH_WATCH_MS then
        p.spawn_watch = nil
        ev("spawn_stretch", { who = "standin", peer = id, max_uu = w.max, bone = w.bone, why = w.why, frames = w.n })
        Log("pose peer %d: spawn stretch over %d ms after %s: max %.1f uu (%s), %d frames", id, PX.STRETCH_WATCH_MS,
            w.why, w.max, w.bone, w.n)
    end
end

-- Rubber-band measure of one driven body (netfeel): how much the body's
-- per-frame motion changes beyond the change in its targets' (second
-- differences, uu per frame). A jump of the shown body shows as a spike.
function PX.nf_note(st, c, h, a)
    if not (h.c1 and h.c2 and h.a1 and h.a2) then return end
    local dr, da = 0, 0
    for k = 1, 3 do
        dr = dr + (c[k] - 2 * h.c1[k] + h.c2[k]) ^ 2
        da = da + (a[k] - 2 * h.a1[k] + h.a2[k]) ^ 2
    end
    local x = math.max(0, math.sqrt(dr) - math.sqrt(da))
    local nf = st.nf or { n = 0, snaps = 0, max = 0 }
    st.nf = nf
    nf.n = nf.n + 1
    if x > PX.NF_SNAP_UU then nf.snaps = nf.snaps + 1 end
    if x > nf.max then nf.max = x end
end
PX.NF_SNAP_UU = 3

-- Spawn stretch watch of OUR pawn: for PX.PAWN_WATCH_MS after its mesh is
-- (re)acquired (a new world / pawn: covers the settle, the placement teleport
-- and its verification), the largest joint stretch against the reference
-- skeleton scaled to this character. Emits `spawn_stretch` (who = pawn) once.
function PX.watch_pawn(mesh, now)
    local w = PX.local_watch
    if not w then return end
    local pos = {}
    for i = 1, PURE.V2_NB do
        pcall(function() local l = mesh:GetSocketLocation(_sv_fn[i]); pos[i] = { l.X, l.Y, l.Z } end)
    end
    local ratios, ref_len = {}, {}
    for i = 2, PURE.V2_NB do
        local a, b, rl = pos[i], pos[PURE.V2_PARENT[i]], PURE.len3(PURE.V2_REF_T[i])
        if a and b and rl > 5 then ratios[#ratios + 1] = PURE.len3({ a[1] - b[1], a[2] - b[2], a[3] - b[3] }) / rl end
    end
    if #ratios >= 10 then
        table.sort(ratios)
        local k = ratios[math.floor((#ratios + 1) / 2)]
        for i = 2, PURE.V2_NB do ref_len[i] = PURE.len3(PURE.V2_REF_T[i]) * k end
        local d, b = PURE.stretch(pos, ref_len)
        if d > w.max then w.max, w.bone = d, PURE.V2_SLOTS[b] or "-" end
        w.n = w.n + 1
    end
    if now - w.t0 >= PX.PAWN_WATCH_MS then
        PX.local_watch = nil
        ev("spawn_stretch", { who = "pawn", peer = -1, max_uu = w.max, bone = w.bone, why = "pawn", frames = w.n })
        Log("contact: our pawn's spawn stretch over %d ms: max %.1f uu (%s), %d frames", PX.PAWN_WATCH_MS, w.max, w.bone, w.n)
    end
end

function PX.open_limits(p, body, now)
    local deg = TUNE.limits or 0
    if deg <= 0 then return PX.close_limits(p, body) end
    if body.limits_at and now - body.limits_at < 5000 and body.limits_deg == deg then return end
    body.limits_at, body.limits_deg = now, deg
    for i = 2, PURE.V2_NB do
        if not PURE.V2_NOBODY[i] then
            pcall(function() body.mesh:SetAngularLimits(_sv_fn[i], deg, deg, deg) end)
        end
    end
end
function PX.close_limits(p, body)
    if not body or not body.limits_at then return end
    body.limits_at, body.limits_deg = nil, nil
    pcall(function() body.mesh:SetConstraintProfileForAll(fname("None"), true) end)
end
local FN_NONE = fname("None")
local _frame_dt = 1 / 60          -- predicted physics step of the coming frame (s)
local _clk_off = nil              -- os.clock base - world real time (ms)

-- Bone-frame centre of mass of every stand-in body, measured once.
local function servo_setup(body, clean_geometry)
    if body.sv then return body.sv end
    local sv = { com = {}, n = 0, err = { n = 0, e = 0, emax = 0, a = 0, amax = 0, hmax = 0, capped = 0 }, wc = {} }
    for i = 1, PURE.V2_NB do
        local idx = -1
        pcall(function() idx = body.mesh:GetBoneIndex(_sv_fn[i]) end)
        if idx and idx >= 0 and not PURE.V2_NOBODY[i] then
            local ok = pcall(function()
                local t = body.mesh:GetSocketTransform(_sv_fn[i], 0)
                local c = body.mesh:GetCenterOfMass(_sv_fn[i])
                local q = { t.Rotation.X, t.Rotation.Y, t.Rotation.Z, t.Rotation.W }
                sv.com[i] = PURE.qrot(PURE.qconj(q), { c.X - t.Translation.X, c.Y - t.Translation.Y, c.Z - t.Translation.Z })
            end)
            if ok and sv.com[i] then sv.n = sv.n + 1 end
        end
    end
    -- The stand-in's own parent offsets (parent frame), for PURE.fk_retarget.
    sv.loc = {}
    for i = 2, PURE.V2_NB do
        pcall(function()
            local t = body.mesh:GetSocketTransform(_sv_fn[i], 0)
            local pt = body.mesh:GetSocketTransform(_sv_fn[PURE.V2_PARENT[i]], 0)
            local q = { pt.Rotation.X, pt.Rotation.Y, pt.Rotation.Z, pt.Rotation.W }
            sv.loc[i] = PURE.qrot(PURE.qconj(q), { t.Translation.X - pt.Translation.X, t.Translation.Y - pt.Translation.Y, t.Translation.Z - pt.Translation.Z })
        end)
    end
    local fixed, k
    if clean_geometry then
        -- These offsets were read immediately after the animated reset and
        -- physics rebuild, so they include the owner's anisotropic scale.
        -- Comparing them against one uniform reference scale would undo it.
        fixed, k = 0, PURE.ref_scale(sv.loc)
    else
        sv.loc, fixed, k = PURE.ref_loc(sv.loc)
    end
    sv.ref_len = {}
    for i = 2, PURE.V2_NB do sv.ref_len[i] = PURE.len3(sv.loc[i]) end
    if fixed > 0 then
        Log("pose: %d of %d parent offsets measured off the reference skeleton (stretched body at setup?): reference used (scale %.3f)",
            fixed, PURE.V2_NB - 1, k)
    end
    body.sv = sv
    Log("pose: servo set up on %s: %d/%d bodies (codec v2)", tostring(body.field), sv.n, PURE.V2_NB - 1)
    return sv
end

-- Stand-in vs. world geometry (WorldStatic=0, WorldDynamic=1): ignored while
-- driven; the original responses are restored on release (native ragdoll).
local function world_collision(body, on)
    if on or TUNE.world == 0 then
        if body.world_resp then
            pcall(function() body.mesh:SetCollisionResponseToChannel(0, body.world_resp[1]); body.mesh:SetCollisionResponseToChannel(1, body.world_resp[2]) end)
            body.world_resp = nil
        end
        return
    end
    if not body.world_resp then
        local a, b = 2, 2
        pcall(function() a = body.mesh:GetCollisionResponseToChannel(0); b = body.mesh:GetCollisionResponseToChannel(1) end)
        body.world_resp = { a, b }
    end
    pcall(function() body.mesh:SetCollisionResponseToChannel(0, 0); body.mesh:SetCollisionResponseToChannel(1, 0) end)
end

local function set_gravity(body, on)
    if body.gravity == on then return end
    pcall(function() body.mesh:SetEnableGravity(on) end)
    if body.sv then
        for _, w in pairs(PX.wc_valid(body)) do   -- validated, fresh roots only
            if w.root and w.root:IsValid() then pcall(function() w.root:SetEnableGravity(on) end) end
        end
    end
    body.gravity = on
end

-- "Ghosting": the stand-in's bodies stop colliding (QueryOnly) for a moment so
-- the servo can carry them through geometry. Used after a teleport snap (the
-- rigid snap can land limbs inside a wall or step) and when a stand-in stays
-- wedged in geometry (most bodies capped for ~1 s, local pawn not near).
local GHOST_MS = 300
local function ghost(body, now, why)
    -- Opt-in only ("ghost":1): measured in-game, switching the driven mesh to
    -- QueryOnly made EVERY body worse (head 57 deg, all capped) - it rebuilds
    -- the physics state and drops the stand-in's driver settings.
    if TUNE.ghost ~= 1 then return end
    if body.coll0 == nil then
        local c; pcall(function() c = body.mesh:GetCollisionEnabled() end)
        body.coll0 = c or 3
    end
    pcall(function() body.mesh:SetCollisionEnabled(1) end)
    body.ghost_until = now + GHOST_MS
    body.ghosts = (body.ghosts or 0) + 1
    if why then Log("pose: stand-in ghosted %d ms (%s)", GHOST_MS, why) end
end
local function unghost_if_due(body, now, force)
    if body.ghost_until and (force or now >= body.ghost_until) then
        pcall(function() body.mesh:SetCollisionEnabled(body.coll0 or 3) end)
        body.ghost_until = nil
    end
end

-- A cached hand-weapon entry (body.sv.wc[field]) is trusted only after a
-- FRESH read of p.actor[field] matches its actor address AND FName and the
-- actor's RootComponent matches the cached root address; the root pointer is
-- then re-taken from that fresh read. HSMPLoadout K2_DestroyActor's stand-in
-- weapons (disarm, world item in hand, kit re-arm): a root kept across that
-- is freed by GC ~60 s later, and IsValid() on it is an access violation.
-- HSMPLoadout also bumps the bus key "standin_weapons" (typed record { gen }) on
-- every such destroy; a new generation drops every cached root pointer at once
-- (PX.wc_gen_poll).
PX.wc_gen, PX.wc_gen_at = nil, -1e9
function PX.wc_gen_poll()
    local now = os.clock()
    if now - PX.wc_gen_at < 0.1 then return PX.wc_gen end
    PX.wc_gen_at = now
    local t = HSMP_IPC and HSMP_IPC.bus_table and HSMP_IPC.bus_table("standin_weapons")
    if type(t) == "table" and t.gen then PX.wc_gen = tostring(t.gen) end
    return PX.wc_gen
end
function PX.wc_check(p, field, c)
    if not (p and p.actor and c and c.addr) then return nil end
    local wa, root
    pcall(function()
        local x = p.actor[field]
        if not (x and x:IsValid()) then return end
        if x:GetAddress() ~= c.addr then return end
        if c.fname and x:GetFName():ToString() ~= c.fname then return end
        local r = x.RootComponent
        if not (r and r:IsValid()) then return end
        if c.root_addr and r:GetAddress() ~= c.root_addr then return end
        wa, root = x, r
    end)
    return wa, root
end
-- Synchronous dev snapshot of cache versus fresh native readbacks. It shares
-- Parity's broadcast command, but reads only after this mod's world guard.
function PX.wc_vec(v)
    if type(v)~="table" then return "unavailable" end
    return string.format("(%.6f,%.6f,%.6f)",v[1],v[2],v[3])
end
function PX.weaponstate(arg)
    if os.getenv("HSMP_DEV")~="1" then return end
    local session=HSM and HSM.new({every_s=0})
    if session then session:poll(true) end
    if not session or not session:live() then return end
    local requested=tonumber(arg)
    if arg~="" and (not requested or requested<0 or requested%1~=0) then return end
    for peer,p in pairs(puppets) do
        if (requested==nil or requested==peer) and p.gen==world_gen and p.actor and p.actor:IsValid() then
            local shown=p.shown or p.applied_context
            if shown and shown.has_context==true and shown.pawn==p.actor:GetFName():ToString() and PURE.pose_context_ok(shown,HSM and HSM.view(),HSM and HSM.mode(),peer) then
                for _,field in ipairs({"Weapon R","Weapon L"}) do
                    local c=p.body and p.body.sv and p.body.sv.wc and p.body.sv.wc[field]
                    local wa,root=PX.wc_check(p,field,c)
                    local actual,base,base_sim
                    local actual_com,com_delta,mass,actor_scale,root_scale,com_error
                    if wa then
                        pcall(function()actual=root:IsSimulatingPhysics(fname("None"))end)
                        pcall(function()base=wa.BaseMesh;if base and base:IsValid() then base_sim=base:IsSimulatingPhysics(fname("None")) end end)
                        local ok,why=pcall(function()
                            local t=wa:GetTransform()
                            local m=root:GetCenterOfMass(fname("None"))
                            local q={t.Rotation.X,t.Rotation.Y,t.Rotation.Z,t.Rotation.W}
                            -- Same orientation-frame/world-uu COM used when
                            -- creating the servo entry; do not divide by scale.
                            actual_com=PURE.qrot(PURE.qconj(q),{m.X-t.Translation.X,m.Y-t.Translation.Y,m.Z-t.Translation.Z})
                            if c.com then com_delta=PURE.d3(actual_com,c.com) end
                            actor_scale={t.Scale3D.X,t.Scale3D.Y,t.Scale3D.Z}
                        end)
                        if not ok then com_error=tostring(why) end
                        pcall(function()mass=root:GetMass()end)
                        pcall(function()local s=root:K2_GetComponentScale();root_scale={s.X,s.Y,s.Z}end)
                    end
                    Log("WPNCACHE peer=%s pawn=%s match=%s round=%s life=%s world=%s generation=%s field=%s actor_address=%s root_address=%s cached_sim=%s actual_root_sim=%s actual_base_sim=%s servo_at=%s grips=%s cached_com=%s actual_com=%s com_delta=%s root_mass=%s actor_scale=%s root_scale=%s read_only=true",
                        tostring(peer),shown.pawn,tostring(shown.match_id),tostring(shown.round),tostring(shown.life),tostring(PX.settle_world),tostring(p.gen),field,tostring(c and c.addr),tostring(c and c.root_addr),tostring(c and c.sim),tostring(actual),tostring(base_sim),tostring(p.wservo and p.wservo[field]),PX.grips_desc(p),
                        PX.wc_vec(c and c.com),PX.wc_vec(actual_com),com_delta and string.format("%.6f",com_delta) or "unavailable",tostring(mass),PX.wc_vec(actor_scale),PX.wc_vec(root_scale))
                    if com_error then Log("WPNCACHE_ERROR peer=%s pawn=%s field=%s property=COM reason=%s",tostring(peer),shown.pawn,field,com_error) end
                end
            end
        end
    end
end
-- The validated weapon entries of a body: stale ones are dropped WITHOUT
-- being touched, live ones get their root re-taken from the fresh read.
-- Call before every use of body.sv.wc.
function PX.wc_valid(body)
    local wc = body and body.sv and body.sv.wc
    if not wc then return {} end
    local p = body.wc_owner
    local gen = PX.wc_gen_poll()
    if body.sv.wc_gen ~= gen then
        for _, c in pairs(wc) do c.root = nil end
        body.sv.wc_gen = gen
    end
    for field, c in pairs(wc) do
        local wa, root = PX.wc_check(p, field, c)
        if wa then c.root = root else wc[field] = nil end
    end
    return wc
end

-- Stand-in weapon parts for a hand ("Weapon R"/"Weapon L"), re-resolved when
-- the actor changes (HSMPLoadout / the game swap weapons at will).
-- Contact impulse of one body (see CONTACT_BODIES): its velocity now against what the
-- servo commanded last frame.
function PX.contact_body(mesh, sv, i, cv, near)
    local fn = _sv_fn[i]
    local mass = tonumber(mesh:GetBoneMass(fn, false))
    if not mass or mass ~= mass or mass <= 0 or mass == math.huge then return end
    sv.mass[i] = mass -- diagnostics follow successful current native mass changes
    local v = mesh:GetPhysicsLinearVelocity(fn)
    local dv = math.sqrt((v.X - cv[1]) ^ 2 + (v.Y - cv[2]) ^ 2 + (v.Z - cv[3]) ^ 2)
    local imp = dv * sv.mass[i]   -- kg*uu/s
    local ci = sv.err.ci or { near = {}, far = {}, maxn = 0, maxb = "-" }
    sv.err.ci = ci
    local t = near and ci.near or ci.far
    t[#t + 1] = imp
    if near and imp > ci.maxn then ci.maxn = imp; ci.maxb = PURE.V2_SLOTS[i] end
    return dv
end

-- Simulation is mutable on a live weapon root (native pickup / setup can
-- enable it after this cache was created). Only call with the fresh root
-- returned by wc_check, or while creating the newly resolved entry.
function PX.wc_refresh_sim(p, body, field, c, root)
    local previous, sim = c.sim, nil
    local ok = pcall(function() sim = root:IsSimulatingPhysics(FN_NONE) end)
    local unavailable = not ok or type(sim) ~= "boolean"
    if unavailable then sim = nil end
    c.sim = sim
    local changed = previous ~= sim or (unavailable and not c.sim_unavailable)
    c.sim_unavailable = unavailable or nil
    if changed or unavailable then
        if p.wservo then p.wservo[field] = nil end
        if p.wservo_actor then p.wservo_actor[field] = nil end
        PX.settle_reset(p, unavailable and "weapon physics unavailable" or "weapon physics changed")
    end
    if changed then
        Log("WPNSIM peer=%s field=%s actor_address=%s root_address=%s previous=%s actual=%s unavailable=%s",
            tostring(p.peer), field, tostring(c.addr), tostring(c.root_addr), tostring(previous), tostring(sim), tostring(unavailable))
    end
    if sim == true and previous ~= true and body.gravity == false then
        -- The same policy used when a simulated entry is first constructed.
        pcall(function() root:SetEnableGravity(false) end)
        pcall(function() root:SetCollisionResponseToChannel(0, 0); root:SetCollisionResponseToChannel(1, 0) end)
    end
end

local function servo_weapon_parts(p, body, field)
    -- Production callers carry both guards. Reject a stale generation / life
    -- before reading any native actor or root, including a reused entry.
    if (p.gen ~= nil and (p.gen ~= world_gen or cache_gen ~= world_gen))
        or (p.peer ~= nil and not PURE.pose_context_ok(p.last, HSM and HSM.view(), HSM and HSM.mode(), p.peer)) then
        return nil
    end
    body.wc_owner = p
    local okw, wa = pcall(PX.r_index, p.actor, field)
    if not okw then wa = nil end
    -- An empty hand drops the entry at once (never kept for later use)
    if not (wa and wa:IsValid()) then body.sv.wc[field] = nil; return nil end
    local addr = PX.nat_addr(wa)
    local c = body.sv.wc[field]
    if c and c.addr == addr and body.sv.wc_gen == PX.wc_gen_poll() then
        local wa2, root = PX.wc_check(p, field, c)
        if wa2 then
            c.root = root
            PX.wc_refresh_sim(p, body, field, c, root)
            return c, wa2
        end
    end
    body.sv.wc[field] = nil   -- a different / destroyed weapon: forget the old entry untouched
    body.sv.wc_gen = PX.wc_gen_poll()
    c = { addr = addr }
    pcall(function() c.fname = wa:GetFName():ToString() end)
    pcall(function() c.root = wa.RootComponent end)
    if not (c.root and c.root:IsValid()) then return nil end
    pcall(function() c.root_addr = c.root:GetAddress() end)
    pcall(function()
        local t = wa:GetTransform()
        local m = c.root:GetCenterOfMass(FN_NONE)
        local q = { t.Rotation.X, t.Rotation.Y, t.Rotation.Z, t.Rotation.W }
        c.com = PURE.qrot(PURE.qconj(q), { m.X - t.Translation.X, m.Y - t.Translation.Y, m.Z - t.Translation.Z })
    end)
    pcall(function() c.base = wa["Root Scene"]; c.tip = wa.TippyTipScene end)
    pcall(function() c.tag = PURE.class_tag(wa:GetClass():GetFName():ToString()) end)
    PX.wc_refresh_sim(p, body, field, c, c.root)
    body.sv.wc[field] = c
    return c, wa
end

local function xf7(t, dst)
    if not dst then
        return { t.Translation.X, t.Translation.Y, t.Translation.Z, t.Rotation.X, t.Rotation.Y, t.Rotation.Z, t.Rotation.W }
    end
    local tr, r = t.Translation, t.Rotation
    dst[1], dst[2], dst[3], dst[4], dst[5], dst[6], dst[7] = tr.X, tr.Y, tr.Z, r.X, r.Y, r.Z, r.W
    return dst
end

-- Receiver half of the ground-truth probe: what the stand-in shows, labelled
-- with the sender time it was driven to (appended to .probe_recv<id>.txt).
-- Probe wall clock: os.clock() is wall ms since process start (MSVC);
-- hsmp-pose-truth adds each process's start time (--send-start/--recv-start).
local function utc_ms() return os.clock() * 1000 end
local _probe_buf, _probe_at = {}, 0
local function probe_flush(id, force)
    local now = now_ms()
    if #_probe_buf == 0 or (not force and now - _probe_at < 500) then return end
    _probe_at = now
    local f = io.open(STATE_DIR .. "/.probe_recv" .. id .. ".txt", "ab")
    if f then f:write(table.concat(_probe_buf, "\n"), "\n"); f:close() end
    _probe_buf = {}
end

-- Everything of the stand-in's own Willie BP that pulls its bodies, measured
-- in-game per body (tracking error with the servo on): the "Block * Muscles"
-- flags, "Head Tonus" (~40 deg head error), the pelvis-to-capsule root
-- constraint (~3 deg), the physics-asset joint motors (97 -> 9 deg head
-- error when zeroed), PhysicalAnimation (the BP sets StrengthMultiplyer back
-- to 2.0, pulling the stand-in to its own idle animation = the "broken back")
-- and pain / flinch reactions to local hits. The BP recomputes these in its
-- tick, so they are zeroed right AFTER that tick (ReceiveTick post-hook,
-- before physics) and again from the frame driver. On release the BP
-- restores its own values.
local function neutralise_bp(p, body)
    -- The variable writes + the motor call natively when native_neutralise is on (PX.nn).
    local native = PX.nn(p, body)
    if not native then pcall(function()
        local a = p.actor
        a["Head Tonus"] = 0
        a["Root Angular Constraint Power"] = 0
        a["Root Linear Constraint Power"] = 0
        if BLOCK_MUSCLES then for _, k in ipairs(MUSCLE_FLAGS) do a[k] = true end end
        for _, k in ipairs(PAIN_ZERO) do a[k] = 0 end
        for _, k in ipairs(PAIN_FALSE) do a[k] = false end
    end) end
    -- The BP's muscle tone (beyond the "Block * Muscles" flags): with these at
    -- their defaults (100 / 1 / 1 / 1 / 35) the stand-in's own arm control
    -- holds an owner in a sword+dagger guard 30-50 deg off at the forearms and
    -- hands, capped every frame, whatever the joints/grips/collision; zeroed:
    -- 0.04 uu / 0.2 deg (measured live, same pose). Originals kept for release.
    if not native and TUNE.tonus ~= 0 then
        local a = p.actor
        if not p.tonus0 then
            p.tonus0 = {}
            for _, k in ipairs(PX.TONUS) do pcall(function() p.tonus0[k] = a[k] end) end
        end
        for _, k in ipairs(PX.TONUS) do pcall(function() a[k] = 0 end) end
    end
    if not native and TUNE.motors ~= 0 then pcall(function() body.mesh:SetAllMotorsAngularDriveParams(0, 0, 0, false) end) end
    for _, pa in ipairs(body.motors or {}) do
        pcall(function() if pa:IsValid() then pa:SetStrengthMultiplyer(0) end end)
    end
    if TUNE.grips ~= 0 then PX.grips_off(p, true) end
end

-- Interaction compliance hook (INTERACT channel). While a local grab/shove on
-- a stand-in is being forwarded to its owner, the stand-in must YIELD to our
-- pawn instead of servoing rigidly back onto the owner's pose. HSMPInteract writes
-- the typed bus record pose_yield = { rows = {{peer, until_ms, gain, cap_lin, cap_ang}} }.
-- until_ms is on the process clock os.clock()*1000 (shared by all UE4SS mods in
-- the game); gain (0..1) / caps (uu/s, deg/s) replace the servo's for that peer
-- until then. Read at 30 Hz.
PX.YIELD = {}
PX.yield_read_at = -1e9
function PX.refresh_yield(now)
    if now - PX.yield_read_at < 33 then return end
    PX.yield_read_at = now
    -- An absent or torn record keeps the last entries: each one expires by
    -- its own until-time, so nothing is held longer than the writer asked.
    -- Typed bus record "pose_yield" (schema/interact.rs): rows {peer, until_ms, gain, cap_lin, cap_ang}.
    local pt = HSMP_IPC and HSMP_IPC.bus_table("pose_yield")
    if type(pt) ~= "table" or type(pt.rows) ~= "table" then return end
    local y = {}
    for _, r in ipairs(pt.rows) do
        y[r.peer] = { untl = r.until_ms, gain = r.gain, cap_lin = r.cap_lin, cap_ang = r.cap_ang }
    end
    PX.YIELD = y
end
function PX.yield_for(id, now)
    local y = PX.YIELD[id]
    if y and now < y.untl then return y end
    return nil
end

-- Contact impulse measurement: the velocity the solver actually left on a
-- body vs. what the servo commanded last frame, times the body mass. Joint
-- impulses show up here too (baseline "far"); "near" (our pelvis within
-- CONTACT_NEAR_UU of the stand-in's) adds contact with our pawn.
PX.CONTACT_NEAR_UU = 150
PX.CONTACT_BODIES = { 1, 4, 9, 13, 17 }   -- pelvis, spine_03, head, hand_l, hand_r

-- Keep the reset decision and its evidence together: emit the pre-correction
-- clock and playback state exactly once per discontinuity, never per frame.
function PX.playback_clock(id, p, cur, clk, now, cut_reset, fresh)
    local rate = PURE.clamp(tonumber(cur.rate) or 1, 0.5, 1.5)
    if rate ~= rate or (tonumber(cur.rate) or 0) <= 0 then rate = 1 end
    -- `now` is the physical frame time. Project the actually received
    -- sidecar clock back to it when the callback/slot read happened late.
    local expect = cur.pt - (cur.lead or 0) + (now - (cur.read_at or now)) * rate
    local projected = clk and (clk.pt + (now - clk.at) * (clk.r or 1))
    local epoch = cur.has_context==true and type(cur.match_id)=="number" and cur.match_id>0
        and cur.match_id<math.huge and cur.match_id==math.floor(cur.match_id)
        and type(cur.round)=="number" and cur.round>0 and cur.round<math.huge and cur.round==math.floor(cur.round)
        and type(cur.life)=="number" and cur.life>0 and cur.life<math.huge and cur.life==math.floor(cur.life)
        and type(cur.cut)=="number" and cur.cut>=0 and cur.cut<math.huge and cur.cut==math.floor(cur.cut)
    -- cut_seen belongs to the body lifecycle: its cut/repose frames return
    -- before this clock integrates. Bind the clock's own generation on the
    -- first actual drive instead of reusing its previous cut's phase.
    local new_epoch = epoch and not (clk and clk.has_context==true and clk.match_id==cur.match_id
        and clk.round==cur.round and clk.life==cur.life and clk.cut==cur.cut)
    local reset = clk ~= nil and not cut_reset and not new_epoch and math.abs(expect - projected) > 50
    if reset then
        ev("x_pose_clock_reset", {
            peer=id, threshold_ms=50, expect=expect, projected_clk=projected, delta_ms=expect-projected,
            clk_pt=clk.pt, clk_at=clk.at, clk_rate=clk.r or 1, local_ms=now,
            source_pt=cur.pt, read_at=cur.read_at or now, rate=rate, lead=cur.lead or 0,
            mode=cur.mode, age=cur.age, delay=cur.delay, jitter=cur.jit, iv=cur.iv,
            quiet=now-((p.play and p.play.seq_at) or now), cut=cur.cut, step=cur.st,
            frame=PX.frame_no or 0, source_seq=cur.seq, fresh=fresh==true,
            match_id=cur.match_id, round=cur.round, life=cur.life, has_context=cur.has_context==true,
        })
    end
    local result
    if not clk or cut_reset or new_epoch or reset then result={pt=expect,at=now,r=rate}
    else
        local r = clk.r or 1
        result={pt=projected+0.1*(expect-projected),at=now,r=r+0.3*(rate-r)}
    end
    if epoch then
        result.has_context,result.match_id,result.round,result.life,result.cut=true,cur.match_id,cur.round,cur.life,cur.cut
    end
    return result, reset
end

-- One frame of the v2 driver. `fresh`: a new pose_play sample arrived this frame.
local function drive_v2(id, p, body, cur, fresh, now, holding, cut_reset)
    local sv = servo_setup(body)
    -- What is on screen now = what we aimed at last frame (contract for
    -- bus key "playback", i.e. HSMPCombat's view time of this peer).
    if p.aim then
        p.shown = PURE.displayed_pose(p.aim, p.aim.pawn, p.aim.label, now)
        p.applied_context = p.shown
    end
    if sv.n == 0 then return end
    -- Time accounting (measured in game with tdiag): this callback runs after
    -- the frame's physics, so the bodies we read stand for the end of the
    -- step that just ran (playback clock + this frame's world delta), and the
    -- velocities we set act over the NEXT step, whose length equals the next
    -- frame's world delta (substepping: up to 80 ms, not the 1/30 s
    -- MaxPhysicsDeltaTime). Aiming at the pose of the frame start and
    -- correcting as if every step were the smoothed frame time (the "v1aim"
    -- path) injects ff * (h_k - h_k-1) of error on each frame-time change
    -- (t.MaxFPS pacing, hitches): stand-ins rougher than their targets
    -- (jitter_ratio 1.3-1.6 on hands/feet), and lag at 20 fps (arm error
    -- 5-15 uu on LordsHall).
    local v1aim = TUNE.v1aim == 1
    local m = 1
    if not v1aim then
        local gap = p.last_drive_no and (PX.frame_no or 0) - p.last_drive_no or 1
        if gap < 1 or gap > 8 then gap = 1 end
        p.dgap = p.dgap and (p.dgap + 0.2 * (gap - p.dgap)) or gap
        m = math.max(1, math.floor(p.dgap + 0.5))   -- frames until this stand-in is driven again
    end
    p.last_drive_no = PX.frame_no
    local hstep = v1aim and 0 or (PX.h_pred or 16.7) * m   -- ms the velocities set now will act for
    local dt = v1aim and (PX.dt_s or _frame_dt) or hstep / 1000
    -- A discontinuity (teleport / respawn / round reset) snaps the stand-in:
    -- quality histories must not span it (a second difference across a snap
    -- reads as a 100+ uu "jitter"; seen as jitter_ratio 118 in a gate run).
    if cut_reset then p.qhist, p.idlew, p.qfoot = nil, nil, nil end
    -- Pose quality is judged on Live only: it is accumulated only while the
    -- Director says Live and the owner is alive (a countdown placement teleport,
    -- a death ragdoll or a round reset is not tracking quality). A new Live
    -- window starts a clean sample.
    local st0 = body.sv and body.sv.err
    local qon = PQ.live() and not p.owner_dead
    if qon and p.q_gen ~= PQ.live_st.gen then
        p.q_gen = PQ.live_st.gen
        p.qhist, p.idlew, p.qfoot, p.q_last = nil, nil, nil, nil
        if st0 then st0.arm, st0.tip, st0.q, st0.qn = nil, nil, nil, 0 end
    end
    -- v2 aim: the bodies on screen stand for playback time + xoff, where
    -- xoff <= this frame's step d: the full step when the latency budget is
    -- tight, less when it is not (less prediction past the newest frame:
    -- at 20-30 fps a d + h look-ahead extrapolated ~45 ms and the targets
    -- jumped by 2-3 uu every frame as new frames came in).
    local xoff = 0
    if not v1aim and TUNE.stamp ~= 0 then
        -- The smoothed frame time, not this frame's: with a long buffer xoff sits at
        -- its cap, and a cap that follows every frame-time change makes the shown
        -- time jump forward on a long frame and back on the next (netfeel).
        local dn = PX.dt_s and PX.dt_s * 1000 or PX.d_now or 0
        xoff = PURE.clamp(8 + (cur.delay or 0) + dn - (TUNE.lat or PX.LAT_TARGET_MS), 0, dn)
    end
    p.xoff = xoff
    PX.xoff_last = xoff
    if qon and st0 then
        st0.qn = (st0.qn or 0) + 1
        -- display latency of THIS frame: sample age + jitter buffer + the part
        -- of this frame's step the shown pose stands behind (v1aim: one servo
        -- frame - lead); the sample reports the window mean.
        local fr = v1aim and ((_frame_dt or 0) * 1000 - (TUNE.lead or 0)) or ((PX.d_now or 0) - xoff)
        st0.lat_sum = (st0.lat_sum or 0) + math.max(0, 8 + (cur.delay or 0) + fr)
    end
    -- Target = the played-back sample moved to a SMOOTH local playback time.
    -- The sidecar writes a sample every 2 ms, so the pt of the sample read in
    -- a frame advances unevenly (frame dt +/- 2 ms): driving to it as-is makes
    -- the stand-in's speed jitter (probe: high-frequency energy many times the
    -- owner's). The Lua clock advances with real time, slews 10 %/frame toward
    -- the sample's pt + its age, and the sample is moved to it with the sender
    -- velocities (|shift| <= a few ms). "lead" (tune knob, ms) adds a
    -- velocity lead; off by default (replay: doubles a swing's hand error).
    -- (cur.lead: the sidecar evaluated this sample that far past its playback clock, see PoseLead)
    -- The sidecar's clock runs at cur.rate (slower while its buffer starves,
    -- faster while it catches up): this clock follows that rate, so a stretch
    -- is not read as an error (which would reset the clock again and again).
    local clk, clkr = PX.playback_clock(id, p, cur, (TUNE.clock ~= 0) and p.clk or nil, now, cut_reset, fresh)
    if clkr and body.sv and body.sv.err then body.sv.err.clkr = (body.sv.err.clkr or 0) + 1 end
    p.clk = clk
    -- A hold sample already stands still (zero velocities): only stale data
    -- freezes the shown time, so leaving a hold does not step it back.
    local frozen = cur.mode == "stale" or holding
    -- Per-slot acceleration of the replicated motion (from consecutive fresh
    -- samples): the aim over a 17-50 ms step follows the curve, not a tangent.
    if fresh and not v1aim then
        p.acc = p.acc or {}
        local pv = p.vprev
        if pv and cur.pt - pv.pt >= 2 and cur.pt - pv.pt <= 80 then
            local k = 1000 / (cur.pt - pv.pt)
            for s, tg in pairs(cur.slots) do
                local o = pv.v[s]
                if o and tg[8] then
                    p.acc[s] = PURE.clamp_acc3(p.acc[s] or {}, (tg[8] - o[1]) * k, (tg[9] - o[2]) * k, (tg[10] - o[3]) * k)
                end
            end
        elseif not pv or cur.pt - pv.pt > 80 then
            p.acc = {}
        end
        -- this sample's velocities, for the next fresh sample (tables reused in place)
        pv = pv or { v = {} }
        local vv = pv.v
        for s in pairs(vv) do
            local tg = cur.slots[s]
            if not (tg and tg[8]) then vv[s] = nil end
        end
        for s, tg in pairs(cur.slots) do
            if tg[8] then
                local v = vv[s] or {}
                v[1], v[2], v[3] = tg[8], tg[9], tg[10]
                vv[s] = v
            end
        end
        pv.pt = cur.pt
        p.vprev = pv
    end
    local targets, label, aim, aim_label
    if v1aim then
        targets = cur.slots
        local ms
        label, aim_label, ms = PURE.display_times(cur.pt, clk.pt + (TUNE.lead or 0), 0, SERVO_EXTRAP_MS, frozen)
        if ms ~= 0 then
            targets = {}
            for s, tg in pairs(cur.slots) do targets[s] = PURE.advance(tg, ms) end
        end
        aim = targets
    else
        -- label: the time the bodies stand for NOW; aim: the pose at the end
        -- of the coming step, approached with the chord velocity of that step.
        local ms, ma
        label, aim_label, ms, ma = PURE.display_times(cur.pt, clk.pt + xoff, hstep, SERVO_EXTRAP_MS, frozen)
        -- Target and aim tables come from a per-stand-in ring of 6 sets (2 per drive):
        -- a set is refilled 3 drives later. The targets are read until 2 drives later
        -- (quality history a1 / a2), the aim until the next one (p.aim).
        local ring = p.vring
        if not ring then
            ring = { n = 0 }
            for k = 1, 6 do ring[k] = { c = {}, t = {} } end
            p.vring = ring
        end
        ring.n = ring.n % 6 + 1
        local tr = ring[ring.n]
        ring.n = ring.n % 6 + 1
        local ar = ring[ring.n]
        targets, aim = tr.c, ar.c
        for s in pairs(targets) do targets[s] = nil end
        for s in pairs(aim) do aim[s] = nil end
        local acc = (not frozen and TUNE.noacc ~= 1) and p.acc or nil
        local tt, at = tr.t, ar.t
        for s, tg in pairs(cur.slots) do
            local a = acc and acc[s]
            local t = PURE.advance(tg, ms, a, tt[s] or {})
            tt[s], targets[s] = t, t
            local e = PURE.aim(tg, ms, ma, a, at[s] or {})
            at[s], aim[s] = e, e
        end
    end
    holding = holding or (cur.mode == "stale")
    if TUNE.retarget ~= 0 and sv.loc then
        if v1aim then
            targets = PURE.fk_retarget(targets, sv.loc)
            if aim ~= targets then aim = PURE.fk_retarget(aim, sv.loc) end
        else   -- both built above for this frame only: rebuilt in place
            PURE.fk_retarget_in(targets, sv.loc)
            PURE.fk_retarget_in(aim, sv.loc)
        end
    end
    -- What is on screen now (contract for bus key "playback" = HSMPCombat's view
    -- time of this peer): v2 aim knows it exactly; v1aim shows last frame's
    -- aim.
    -- Publish the physical sample time used to construct these targets. The
    -- selected sender step cannot be subtracted: around hitches that mapping
    -- has several different poses for the same frame-start label.
    if not v1aim then
        p.shown = PURE.displayed_pose(cur, p.actor:GetFName():ToString(), label, now)
        p.applied_context = p.shown
    end
    -- Foot planting: while the owner's foot is planted (its replicated speed
    -- under PQ.PLANT_SPD), the stand-in's foot is locked where it was when the
    -- plant began and only creeps toward the owner's foot at PQ.PLANT_CREEP
    -- uu/s; at lift-off it follows the replicated pose again. Without it the
    -- planted foot chases every revision of the predicted pose and the
    -- residual error it had at touch-down: 6-60 uu/s of slide at 20-35 fps
    -- (LordsHall).
    if not v1aim and not frozen and TUNE.plant ~= 0 then
        p.plant = p.plant or {}
        for _, i in ipairs(PQ.FEET) do
            local t, a = targets[i], aim[i]
            local spd = t and math.sqrt((t[8] or 0) ^ 2 + (t[9] or 0) ^ 2 + (t[10] or 0) ^ 2) or 99
            if t and a and spd < PQ.PLANT_SPD then
                local pl = p.plant[i]
                if not pl then
                    local c
                    pcall(function() local x = body.mesh:GetSocketLocation(_sv_fn[i]); c = { x.X, x.Y, x.Z } end)
                    if c and PURE.d3(c, t) < PQ.PLANT_MAX_ERR then pl = { c[1], c[2], c[3] }; p.plant[i] = pl end
                end
                if pl then
                    local dx, dy, dz = t[1] - pl[1], t[2] - pl[2], t[3] - pl[3]
                    local l = math.sqrt(dx * dx + dy * dy + dz * dz)
                    local step = PQ.PLANT_CREEP * dt
                    if l > step then local k = step / l; dx, dy, dz = dx * k, dy * k, dz * k end
                    pl[1], pl[2], pl[3] = pl[1] + dx, pl[2] + dy, pl[3] + dz
                    local n = {}
                    for k = 1, #a do n[k] = a[k] end
                    n[1], n[2], n[3], n[8], n[9], n[10] = pl[1], pl[2], pl[3], 0, 0, 0
                    aim[i] = n
                end
            else
                p.plant[i] = nil
            end
        end
    elseif p.plant then
        p.plant = nil
    end
    neutralise_bp(p, body)
    local mesh = body.mesh
    local prev = v1aim and p.aim or { slots = targets, label = label }
    PX.refresh_yield(now)
    local yl = PX.yield_for(id, now)
    local s_gain = yl and yl.gain or (TUNE.gain or SERVO_GAIN)
    local s_capl = yl and yl.cap_lin or SERVO_CAP_LIN
    local s_capa = yl and yl.cap_ang or SERVO_CAP_ANG
    if p.impact_until and now < p.impact_until then
        s_gain, s_capl, s_capa = math.min(s_gain, IMPACT_GAIN), math.min(s_capl, IMPACT_CAP), math.min(s_capa, IMPACT_CAP)
    end
    if body.ramp_at then
        local k = (now - body.ramp_at) / PX.RAMP_MS
        if k >= 1 or k < 0 then body.ramp_at = nil else s_capl, s_capa = s_capl * (0.3 + 0.7 * k), s_capa * (0.3 + 0.7 * k) end
    end
    if yl and not p.yielding then Log("pose peer %d: yielding to an interaction (gain %.2f caps %.0f/%.0f)", id, s_gain, s_capl, s_capa) end
    p.yielding = yl ~= nil
    -- contact impulses (see CONTACT_BODIES)
    local near = false
    do
        local lp = local_pelvis()
        local tp = targets[1]
        if lp and tp then near = PURE.d3(tp, { lp.X, lp.Y, lp.Z }) < PX.CONTACT_NEAR_UU end
        p.near_us = lp and tp and PURE.d3(tp, { lp.X, lp.Y, lp.Z }) < 250 or false
    end
    sv.cmd = sv.cmd or {}
    sv.mass = sv.mass or {}
    sv.dvp = sv.dvp or {}
    if sv.cmd[1] then
        local idv = TUNE.impact_dv or IMPACT_DV
        for _, i in ipairs(PX.CONTACT_BODIES) do
            local cv = sv.cmd[i]
            if cv then
                local ok, dv = pcall(PX.contact_body, mesh, sv, i, cv, near)
                -- A blow is a STEP in how far the solver left the body off its command. A limb
                -- held against a joint or a grip sits a steady ~900 uu/s off the capped servo
                -- (measured live: an idle stand-in's hand_r 920 uu/s every frame, 438 false
                -- "struck" in a 70-round gate without one blow), which is no reason to yield.
                local prev = sv.dvp[i]
                if ok and dv then sv.dvp[i] = dv end
                if ok and dv and prev and idv > 0 and dv > idv and dv - prev > idv and not body.ramp_at then
                    if not (p.impact_logged and now - p.impact_logged < 2000) then
                        p.impact_logged = now
                        Log("pose peer %d: struck (%s %.0f uu/s off the servo, +%.0f in one frame): yielding %d ms", id,
                            PURE.V2_SLOTS[i] or "?", dv, dv - prev, TUNE.impact_ms or IMPACT_MS)
                    end
                    p.impact_until = now + (TUNE.impact_ms or IMPACT_MS)
                end
            end
        end
    end
    local probe = POSE_PROBE and prev and {} or nil
    local cpos = p.spawn_watch and {} or nil
    local st = sv.err
    local pel_err
    local ncap = 0
    if TUNE.tdiag then PX.td_acc = { 0, 0, 0, 0, 0, 0, 0, 0, 0, 0 } end
    if v1aim and (SEVERED_PHYSICS or body.injury_probe or next(body.injury_disabled or {})) then
        local copy = {}
        for s,t in pairs(targets) do copy[s] = t end
        targets = copy -- never remove slots from the retained received pose
    end
    PX.injury_targets(id,p,targets,aim)
    local nat = PX.ns_bodies(mesh, aim, sv, dt, s_capl, s_capa, s_gain, yl, holding)   -- native servo
    if PX.SETTLE then
        p.settle_state=PX.SETTLE.begin(p.settle_state,PX.settle_world,p.shown,p.aim,cur,now)
    end
    local sv_c = sv.c7 or {}
    sv.c7 = sv_c
    for i = 1, PURE.V2_NB do
        local com = sv.com[i]
        local tg = aim[i]
        if com and tg then
            local fn = _sv_fn[i]
            -- this body's transform: a fresh table when something keeps it past this
            -- frame (probe, stretch watch, quality history), else a reused one
            local c
            if probe or cpos or (qon and QBODY[i]) then
                c = {}
            else
                c = sv_c[i]
                if not c then c = {}; sv_c[i] = c end
            end
            if nat then
                local b = (i - 1) * 7
                local oc = nat.c
                c[1], c[2], c[3], c[4], c[5], c[6], c[7] = oc[b + 1], oc[b + 2], oc[b + 3], oc[b + 4], oc[b + 5], oc[b + 6], oc[b + 7]
            else
                c = xf7(mesh:GetSocketTransform(fn, 0), c)
            end
            if PX.SETTLE then PX.SETTLE.measure(p.settle_state,i,c,p.aim and p.aim.slots and p.aim.slots[i]) end
            -- Tracking error vs. what we aimed at last frame.
            if prev and prev.slots[i] then
                local a = prev.slots[i]
                local e = PURE.d3(c, a)
                local ang = PURE.qangle8(c[4], c[5], c[6], c[7], a[4], a[5], a[6], a[7])
                st.n, st.e, st.a = st.n + 1, st.e + e, st.a + ang
                st.pb = st.pb or {}
                local pb = st.pb[i] or { 0, 0, 0, 0, 0 }
                pb[1], pb[2], pb[3] = pb[1] + e, pb[2] + ang, pb[3] + 1
                -- Twist part of the error (about the bone's own X axis).
                local nx, ny, nz, cw = -c[4], -c[5], -c[6], c[7]   -- conj(c) * a, x and w only
                local le1 = cw * a[4] + nx * a[7] + ny * a[6] - nz * a[5]
                local le4 = cw * a[7] - nx * a[4] - ny * a[5] - nz * a[6]
                pb[5] = (pb[5] or 0) + math.deg(2 * math.atan(math.abs(le1), math.abs(le4)))
                st.pb[i] = pb
                if e > st.emax then st.emax = e; st.wi = i end
                if ang > st.amax then st.amax = ang; st.ai = i end
                if (i == 13 or i == 17) and e > st.hmax then st.hmax = e end
                if qon and i >= 10 and i <= 17 then st.arm = st.arm or {}; st.arm[#st.arm + 1] = e end
            end
            if probe then probe[i] = c end
            if cpos then cpos[i] = c end
            -- Smoothness / planted-foot / idle metrics (pose_quality): pelvis,
            -- head, hands, feet. Stand-in second differences vs. those of the
            -- poses it was driven to (the owner's motion at the same labels).
            local qi = QBODY[i]
            if qon and qi and prev and prev.slots[i] then
                st.q = st.q or { hr = {}, ha = {}, foot = {}, idle = {} }
                local h = p.qhist and p.qhist[i]
                local a = prev.slots[i]
                -- Smoothness is judged on MOVING bodies only (target >= PQ.JIT_SPD uu/s),
                -- as documented: on a still body the ratio compares sub-mm solver noise
                -- (0.1 uu, 1/3 px) with nothing and reads ~2; stillness is idle_rms's job.
                local tspd = math.sqrt((a[8] or 0) ^ 2 + (a[9] or 0) ^ 2 + (a[10] or 0) ^ 2)
                if h and h.c2 and tspd >= PQ.JIT_SPD then
                    local mr, ma = 0, 0
                    for k = 1, 3 do
                        local dr = c[k] - 2 * h.c1[k] + h.c2[k]
                        local da = a[k] - 2 * h.a1[k] + h.a2[k]
                        mr, ma = mr + dr * dr, ma + da * da
                    end
                    st.q.hr[i] = (st.q.hr[i] or 0) + mr
                    st.q.hn = st.q.hn or {}
                    st.q.hn[i] = (st.q.hn[i] or 0) + 1
                    st.q.ha[i] = (st.q.ha[i] or 0) + ma
                end
                if qi == "foot" then
                    -- Planted-foot slide: the stand-in foot's DISPLACEMENT over a
                    -- planted run (owner's foot target < 5 uu/s) of >= FOOT_WIN_S,
                    -- divided by its duration. A per-frame finite difference
                    -- would turn 0.05-0.08 uu of solver noise per frame into
                    -- 3-5 uu/s at 60 Hz, i.e. the limit (5) would measure noise;
                    -- a real slide persists over the window, noise does not.
                    local tv = math.sqrt((a[8] or 0) ^ 2 + (a[9] or 0) ^ 2 + (a[10] or 0) ^ 2)
                    p.qfoot = p.qfoot or {}
                    local run = p.qfoot[i]
                    if tv < 5 then
                        if not run then
                            p.qfoot[i] = { c0 = c, t = 0 }
                        else
                            run.t = run.t + dt
                            if run.t >= PQ.FOOT_WIN_S then
                                st.q.foot[#st.q.foot + 1] = PURE.d3(c, run.c0) / run.t
                                p.qfoot[i] = { c0 = c, t = 0 }
                            end
                        end
                    else
                        p.qfoot[i] = nil
                    end
                end
                if i == 1 and h then PX.nf_note(st, c, h, a) end
                p.qhist = p.qhist or {}
                p.qhist[i] = { c1 = c, c2 = h and h.c1, a1 = a, a2 = h and h.a1 }
            end
            if i == 1 then pel_err = PURE.d3(c, targets[1] or tg) end
            if TUNE.tdiag and (i == 1 or i == 17) and targets[i] then
                -- lag (ms) of the body behind its same-label target along the motion
                local t = targets[i]
                local vv = (t[8] or 0) ^ 2 + (t[9] or 0) ^ 2 + (t[10] or 0) ^ 2
                local lag = vv > 400 and -((c[1] - t[1]) * t[8] + (c[2] - t[2]) * t[9] + (c[3] - t[3]) * t[10]) / vv * 1000 or 0
                PX["td_lag" .. i] = lag
                PX["td_spd" .. i] = math.sqrt(vv)
                if i == 1 then
                    local a1 = aim[1] or t
                    PX.td_pel = string.format("%.2f %.2f %.2f %.2f %.2f %.2f %.2f %.2f %.2f", c[1], c[2], c[3], t[1], t[2], t[3], a1[1], a1[2], a1[3])
                    local hk = p.hook
                    PX.td_hk = hk and vv > 400 and string.format("%.2f %.3f %.3f",
                        ((c[1] - hk[1]) * t[8] + (c[2] - hk[2]) * t[9] + (c[3] - hk[3]) * t[10]) / vv * 1000,
                        (PX.rt_raw or 0) - hk[4], os.clock() * 1000 - hk[5]) or "x x x"
                end
            end
            local vx, vy, vz, wx, wy, wz, dl, gl
            if nat then
                local b, ov = (i - 1) * 3, nat.v   -- linear only (the angular velocity is set natively)
                vx, vy, vz = ov[b + 1], ov[b + 2], ov[b + 3]
                dl, gl = nat.dl[i], nat.gl[i]
            else
                local g = holding and { tg[1], tg[2], tg[3], tg[4], tg[5], tg[6], tg[7], 0, 0, 0, 0, 0, 0 } or tg
                local gi = (i >= 18 and not yl and (TUNE.leg_gain or PX.LEG_GAIN)) or s_gain   -- legs: dev knob "leg_gain"
                vx, vy, vz, wx, wy, wz, dl, gl = PURE.servo(c, g, com, dt, s_capl, s_capa, gi)
            end
            local cmd = sv.cmd[i] or {}
            cmd[1], cmd[2], cmd[3] = vx, vy, vz
            sv.cmd[i] = cmd
            if TUNE.tdiag and i == 20 and targets[i] then
                local t = targets[i]
                PX.td_foot = string.format("%.2f %.2f %.2f %.2f %.2f %.2f %.1f %d %.2f %.2f %.2f", c[1], c[2], c[3], t[1], t[2], t[3],
                    math.sqrt((t[8] or 0) ^ 2 + (t[9] or 0) ^ 2 + (t[10] or 0) ^ 2), (p.plant and p.plant[20]) and 1 or 0,
                    vx, vy, vz)
            end
            if TUNE.tdiag then
                -- Timing diagnostics (dev knob "tdiag"): mass-weighted COM of the
                -- driven bodies and the commanded COM momentum, per frame.
                local td = PX.td_acc
                sv.mass2 = sv.mass2 or {}
                sv.mass2[i] = nil
                pcall(function() sv.mass2[i] = mesh:GetBoneMass(fn, false) or 0 end)
                local m = sv.mass2[i] or 0
                local wc = PURE.qrot({ c[4], c[5], c[6], c[7] }, com)
                td[1], td[2], td[3], td[4] = td[1] + m, td[2] + m * (c[1] + wc[1]), td[3] + m * (c[2] + wc[2]), td[4] + m * (c[3] + wc[3])
                td[5], td[6], td[7] = td[5] + m * vx, td[6] + m * vy, td[7] + m * vz
                td[8] = td[8] + m * (tg[8] or 0); td[9] = td[9] + m * (tg[9] or 0); td[10] = td[10] + m * (tg[10] or 0)
            end
            if dl > s_capl or gl > s_capa then
                ncap = ncap + 1
                st.capped = st.capped + 1
                if st.pb and st.pb[i] then st.pb[i][4] = st.pb[i][4] + 1 end
            end
            -- Stall watch: a limb body held far off its target, capped, frame after
            -- frame (twisted through a joint limit: the shortest way back is
            -- blocked, so the servo pushes against the joint forever; seen as
            -- arm 60-87 uu / legs 50-110 deg "BENT" for whole rounds).
            if i >= 10 and targets[i] then
                local t = targets[i]
                local bad = (dl > s_capl or gl > s_capa)
                    and PURE.qangle8(c[4], c[5], c[6], c[7], t[4], t[5], t[6], t[7]) > 45
                body.stall = body.stall or {}
                body.stall[i] = bad and ((body.stall[i] or 0) + 1) or 0
                if body.stall[i] > (PX.STALL_FRAMES or 20) then PX.stalled = i end
            end
            if not nat then   -- the native call already set them
                local lv, av = PX.vlin, PX.vang   -- reused: the call copies them into FVectors
                lv.X, lv.Y, lv.Z, av.X, av.Y, av.Z = vx, vy, vz, wx, wy, wz
                mesh:SetPhysicsLinearVelocity(lv, false, fn)
                mesh:SetPhysicsAngularVelocityInDegrees(av, false, fn)
            end
        end
    end
    if cpos then PX.watch_stretch(id, p, sv, cpos, now) end
    if TUNE.tdiag and PX.td_acc and PX.td_acc[1] > 0 then
        local td, M = PX.td_acc, PX.td_acc[1]
        PX.tbuf = PX.tbuf or {}
        PX.tbuf[#PX.tbuf + 1] = string.format("F %d %.3f %.3f %.4f %.3f %.3f %.3f %s %.3f %.3f %.3f %.2f %.2f %.2f %.2f %.2f %.2f %.3f %d %.2f %.1f %.2f %.1f %.2f %s %s %.2f %.2f %.2f %s",
            id, os.clock() * 1000, PX.clk_raw or -1, (PX.d_raw or -1) * 1000, PX.rt_raw or -1, label, cur.pt, cur.mode,
            td[2] / M, td[3] / M, td[4] / M, td[5] / M, td[6] / M, td[7] / M, td[8] / M, td[9] / M, td[10] / M, dt * 1000, fresh and 1 or 0, PX.td_lag1 or 0, PX.td_spd1 or 0, PX.td_lag17 or 0, PX.td_spd17 or 0, PX.d_now or -1, PX.td_hk or "x x x", PX.td_pel or "x x x x x x x x x", sv.cmd[1] and sv.cmd[1][1] or 0, sv.cmd[1] and sv.cmd[1][2] or 0, sv.cmd[1] and sv.cmd[1][3] or 0, PX.td_foot or "x x x x x x x x x x x")
        if #PX.tbuf >= 60 then
            local f = io.open(STATE_DIR .. "/.tdiag_recv.txt", "ab")
            if f then f:write(table.concat(PX.tbuf, "\n"), "\n"); f:close() end
            PX.tbuf = {}
        end
    end
    -- Idle stillness: while the owner is idle (replicated pelvis and hand
    -- speeds < PQ.IDLE_SPD uu/s: standing, not moving), the stand-in must not
    -- add motion of its own: RMS about the mean of its tracking error
    -- (stand-in minus target, pelvis and hands) <= 0.5 uu. A stricter idle
    -- test (< 2 uu/s on pelvis AND both hands for 60 consecutive frames) is
    -- never met by a standing active ragdoll (idle sway), and absolute
    -- positions would measure the owner's own sway, hence the tracking error.
    -- Windows close at IDLE_WIN frames or when the owner moves.
    if qon and prev and p.qhist and p.qhist[1] and p.qhist[13] and p.qhist[17] then
        local function spd(s) local t = prev.slots[s]; return (t and t[8]) and math.sqrt(t[8] ^ 2 + (t[9] or 0) ^ 2 + (t[10] or 0) ^ 2) or 99 end
        local still = spd(1) < PQ.IDLE_SPD and spd(13) < PQ.IDLE_SPD and spd(17) < PQ.IDLE_SPD
        p.idlew = p.idlew or { n = 0 }
        local iw = p.idlew
        if still then
            iw.n = iw.n + 1
            for _, s in ipairs({ 1, 13, 17 }) do
                local c, tg = p.qhist[s].c1, p.qhist[s].a1
                if tg then
                    local ex, ey, ez = c[1] - tg[1], c[2] - tg[2], c[3] - tg[3]
                    iw[s] = iw[s] or { 0, 0, 0, 0 }
                    local a = iw[s]
                    a[1], a[2], a[3], a[4] = a[1] + ex, a[2] + ey, a[3] + ez, a[4] + ex * ex + ey * ey + ez * ez
                end
            end
        end
        if (not still and iw.n >= PQ.IDLE_MIN) or iw.n >= PQ.IDLE_WIN then
            local v = PQ.idle_window_rms(iw)
            if v then
                st.q = st.q or { hr = {}, ha = {}, foot = {}, idle = {} }
                st.q.idle[#st.q.idle + 1] = v
            end
            p.idlew = { n = 0 }
        elseif not still then
            p.idlew = { n = 0 }
        end
    end
    -- Held weapons (their own simulated actors, constrained to the hand).
    local wstate = "none"
    local wst = {}
    for s = PURE.V2_WPN_R, PURE.V2_WPN_L do
        local tg = (TUNE.wpn ~= 0) and aim[s] or nil
        local field = (s == PURE.V2_WPN_R) and "Weapon R" or "Weapon L"
        local wprev = wstate
        wstate = "none"
        -- Resolve the stand-in's weapon in that hand every frame even when the
        -- sender holds nothing there: servo_weapon_parts turns its gravity off,
        -- so an unmatched weapon does not hang off the driven hand.
        local c, wa = servo_weapon_parts(p, body, field)
        -- The owner holds nothing in this hand (disarmed / dropped)
        -- but the stand-in still has a weapon there until HSMPLoadout strips
        -- it (up to ~2 s): it must not parry or hit meanwhile -> no collision
        -- after 150 ms; back on as soon as the owner holds a weapon again.
        if c and wa and TUNE.wpn ~= 0 then
            if not tg then
                c.empty_since = c.empty_since or now
                if not c.nocoll and now - c.empty_since > 150 then
                    pcall(function() wa:SetActorEnableCollision(false) end)
                    c.nocoll = true
                    Log("pose peer %d: owner holds nothing in %s; stand-in's weapon collision off until re-armed", id, field)
                end
            else
                c.empty_since = nil
                if c.nocoll then pcall(function() wa:SetActorEnableCollision(true) end); c.nocoll = nil end
            end
        end
        if tg then
            if not c then
                wstate = "missing"
            elseif cur.weapons[s] and c.tag ~= cur.weapons[s][2] then
                wstate = "other-class"   -- not the sender's weapon: it just follows the hand
            elseif c.sim == nil then
                wstate = "physics-unavailable"
            elseif not c.sim then
                wstate = "kinematic"
            elseif c.com then
                local wout = PX.nw_weapon(c, tg, dt, holding)   -- native weapon servo
                local x
                if wout then
                    local ox = wout.x
                    x = { ox[1], ox[2], ox[3], ox[4], ox[5], ox[6], ox[7] }
                else
                    x = xf7(wa:GetTransform())
                end
                if probe and s == PURE.V2_WPN_R then
                    local b, tp
                    pcall(function() b = c.base:K2_GetComponentLocation(); tp = c.tip:K2_GetComponentLocation() end)
                    if b and tp then probe.w = { b.X, b.Y, b.Z, tp.X, tp.Y, tp.Z } end
                end
                -- Blade tip tracking error vs. last frame's aim (sender's tip, weapon space).
                local wm = cur.weapons[s]
                if qon and prev and prev.slots[s] and wm then
                    local tl, a = { wm[6], wm[7], wm[8] }, prev.slots[s]
                    local tc = PURE.qrot({ x[4], x[5], x[6], x[7] }, tl)
                    local ta = PURE.qrot({ a[4], a[5], a[6], a[7] }, tl)
                    st.tip = st.tip or {}
                    st.tip[#st.tip + 1] = PURE.d3({ x[1] + tc[1], x[2] + tc[2], x[3] + tc[3] }, { a[1] + ta[1], a[2] + ta[2], a[3] + ta[3] })
                end
                if not wout then   -- the native call already servoed it
                    local g = holding and { tg[1], tg[2], tg[3], tg[4], tg[5], tg[6], tg[7], 0, 0, 0, 0, 0, 0 } or tg
                    local vx, vy, vz, wx, wy, wz = PURE.servo(x, g, c.com, dt, SERVO_CAP_LIN, SERVO_CAP_ANG, TUNE.gain or SERVO_GAIN)
                    c.root:SetPhysicsLinearVelocity({ X = vx, Y = vy, Z = vz }, false, FN_NONE)
                    c.root:SetPhysicsAngularVelocityInDegrees({ X = wx, Y = wy, Z = wz }, false, FN_NONE)
                end
                wstate = "servo"
                p.wservo = p.wservo or {}
                p.wservo[field] = now
                p.wservo_actor=p.wservo_actor or {}
                p.wservo_actor[field]=c.addr
            end
        end
        local cls = PX.wpn_status(c, wa, now)
        wst[#wst + 1] = string.format("%s:%s(%s%s)", field == "Weapon R" and "R" or "L",
            tg and (wstate == "none" and "?" or wstate) or "no-target", cls,
            (c and c.sim) and ",sim" or "")
        if wstate == "none" then wstate = wprev end
    end
    p.wpn_state = wstate
    p.wpn_states = wst
    -- Wedged in geometry: most bodies held off target for ~1 s while our own
    -- pawn is not touching it -> ghost through (contact with US is legit).
    unghost_if_due(body, now, false)
    if ncap >= 5 then body.stuck_frames = (body.stuck_frames or 0) + 1 else body.stuck_frames = 0 end
    if body.stuck_frames > 45 and not body.ghost_until then
        local lp = local_pelvis()
        local sp = targets[1]
        if not lp or not sp or PURE.d3(sp, { lp.X, lp.Y, lp.Z }) > 150 then
            ghost(body, now, string.format("wedged: %d bodies held off target for %d frames", ncap, body.stuck_frames))
            body.stuck_frames = 0
        end
    end
    if probe then
        local out = { string.format("R %.3f", prev.label) }
        for i = 1, PURE.V2_NB do
            local c = probe[i]
            if not c then
                pcall(function() c = xf7(mesh:GetSocketTransform(_sv_fn[i], 0)) end)
            end
            c = c or { 0, 0, 0, 0, 0, 0, 1 }
            out[#out + 1] = string.format("%.3f %.3f %.3f %.6f %.6f %.6f %.6f", c[1], c[2], c[3], c[4], c[5], c[6], c[7])
        end
        if probe.w then out[#out + 1] = string.format("W %.3f %.3f %.3f %.3f %.3f %.3f", table.unpack(probe.w)) end
    out[#out + 1] = string.format("U %.0f", utc_ms())
    local vis = true
    pcall(function() vis = mesh:WasRecentlyRendered(0.25) end)
    out[#out + 1] = vis and "V 1" or "V 0"
    _probe_buf[#_probe_buf + 1] = table.concat(out, " ")
        probe_flush(id, false)
    end
    if PX.SETTLE then
        PX.SETTLE.finish(p.settle_state,now)
        PX.SETTLE.copy(p.shown,p.settle_state)
        if p.settle_state.settle_ready and not p.settle_state.logged then
            p.settle_state.logged=true
            Log("spawn settle peer=%d pawn=%s match=%s round=%s life=%s ready=%s reason=%s limbs=%d pos=%.2f rot=%.2f stable_ms=%.0f source_seq=%s source_ts=%s",
                id,tostring(p.shown and p.shown.pawn),tostring(cur.match_id),tostring(cur.round),tostring(cur.life),
                tostring(p.settle_state.settle_ready),p.settle_state.settle_reason,p.settle_state.settle_count,
                p.settle_state.settle_pos_uu,p.settle_state.settle_rot_deg,p.settle_state.settle_stable_ms,
                tostring(p.settle_state.settle_source_seq),tostring(p.settle_state.settle_source_ts))
        end
    end
    p.aim = PURE.displayed_pose(cur, p.actor:GetFName():ToString(), aim_label, now)
    p.aim.world,p.aim.cut,p.aim.seq=PX.settle_world,cur.cut,cur.seq
    p.aim.slots = aim
    return pel_err
end

-- Runs every game frame for every claimed puppet.
local function drive_frame(id, p, now)
    p.peer=id
    local pl = p.play or {}
    p.play = pl
    local r = read_play(id, pl.seq)
    pl.reads = (pl.reads or 0) + 1
    if type(r) == "table" then
        if pl.seq then pl.fresh = (pl.fresh or 0) + 1 end
        p.last, pl.seq, pl.seq_at = r, r.seq, now
    elseif r == nil then
        pl.bad = (pl.bad or 0) + 1   -- missing or torn record
    end
    local cur = p.last
    if r == "context" or (cur and not PURE.pose_context_ok(cur, HSM and HSM.view(), HSM and HSM.mode(), id)) then
        if p.driving then release_standin(p) end
        p.last, p.aim, p.shown, p.cut_seen = nil, nil, nil, nil
        pl.seq, pl.seq_at = nil, nil
        return
    end
    if p.body and p.body.injury_retiring then return end
    if cur and cur.v2 then
        -- Codec v2: velocity servo every frame (replication.md "Driver").
        local quiet = now - (pl.seq_at or -1e9)
        local function let_go(why)
            if p.driving then
                Log("pose peer %d: %s; stand-in released (servo off, gravity on)", id, why)
                release_standin(p)
            end
        end
        if p.owner_dead then let_go("owner dead (server-declared): native ragdoll"); return end
        if quiet > HOLD_RELEASE_MS then let_go("no pose for " .. math.floor(quiet) .. " ms"); return end
        local fresh = type(r) == "table"
        local cut_changed = fresh and p.cut_seen ~= nil and cur.cut ~= p.cut_seen
        if fresh and cur.root then
            drive_root(p.actor, { X = cur.root[1], Y = cur.root[2], Z = cur.root[3] }, cur.root[4], cut_changed)
        end
        local body = p.in_range and p.body or nil
        if not body or cur.nbones == 0 then p.cut_seen = cur.cut; return end
        if body.ctl ~= "servo" then
            release_handles(body)
            p.driving = false
            -- The servo drives the visible ragdoll itself (CharacterMesh0);
            -- head, gore mesh and armour copy their pose from it.
            if body.field ~= "Mesh" then
                PX.settle_reset(p,"mesh handoff")
                local m; pcall(function() m = p.actor.Mesh end)
                if not (m and m:IsValid()) then return end
                local ma,mn
                pcall(function()ma=m:GetAddress();mn=m:GetFName():ToString()end)
                if type(ma)~="number" or ma<=0 or not math.tointeger(ma)or type(mn)~="string"or mn==""then return end
                if PX.injury_release(p)==false then return end
                body.mesh, body.field, body.sims, body.sv = m, "Mesh", { m }, nil
                body.mesh_addr,body.mesh_fname=ma,mn
                pcall(function() m:SetSimulatePhysics(true) end)
            end
            body.ctl = "servo" -- commit only after restoration and the required mesh switch
        end
        if PX.sync_body_scale(id, p, body, now, cur) then return end
        if not p.driving then
            -- Start only on live data read after the claim: a stale sample can be
            -- the owner's previous round or place, and pulling a fresh Willie there
            -- body by body is what tore limbs apart on spawn.
            if not fresh or cur.mode == "stale" or cur.mode == "hold" or (cur.read_at or 0) < (p.claimed_at or 0) then
                p.cut_seen = cur.cut
                return
            end
            set_muscles_blocked(p, true)
            set_motor_strength(body, 0.0)
            servo_setup(body)
            set_gravity(body, false)
            -- Bones must refresh even when this window is not rendering
            -- (minimized / hidden): the servo reads them every frame.
            pcall(function() body.mesh.VisibilityBasedAnimTickOption = 0 end)
            pcall(function() local bc = p.actor.BoneCore; if bc and bc:IsValid() then bc:SetCollisionEnabled(0) end end)
            -- Collision matrix: the owner's body already resolved
            -- the world; a stand-in that also collides with world geometry gets
            -- wedged in fences/walls/steps and fights its target (measured: head
            -- 120 deg off, feet sliding). It keeps colliding with pawns, bodies and
            -- weapons (contact with US stays physical).
            world_collision(body, false)
            p.driving = true
            -- Root first, then a clean start: physics off for one frame (the
            -- bodies take the animated pose, any broken constraint state of a
            -- reused Willie is gone), snapped onto the target pelvis next frame
            -- (re-pose below), and a soft servo for the first moments.
            if cur.root then
                drive_root(p.actor, { X = cur.root[1], Y = cur.root[2], Z = cur.root[3] }, cur.root[4], true)
            end
            PX.start_repose(p, body, now, "drive start")
            p.cut_seen = cur.cut
            return
        end
        local pel = cur.slots[1]
        p.targets = pel and { Pelvis = pel } or nil
        if pel and cut_changed then
            -- Teleport / respawn: the same clean start as a new drive.
            PX.start_repose(p, body, now, string.format("discontinuity #%d (teleport/respawn)", cur.cut))
            ghost(body, now, nil)
            body.err_since = nil
            p.cut_seen = cur.cut
            return
        end
        p.cut_seen = cur.cut
        local holding = cur.mode == "stale" or quiet > PLAY_STALE_MS
        if TUNE.servo == 0 then return end
        -- Re-pose (second half): physics back on, the bodies now start from the
        -- mesh's animated pose (no twist), snapped onto the target pelvis.
        if body.repose then
            body.repose = nil
            pcall(function() body.mesh:SetSimulatePhysics(true) end)
            if body.scale_remeasure then
                body.sv, body.scale_remeasure = nil, nil
                servo_setup(body, true)
            end
            body.gravity = nil
            set_gravity(body, false)
            set_motor_strength(body, 0.0)
            world_collision(body, false)
            body.stall = nil
            if pel and snap_clear(pel) then snap_mesh(body, pel[1], pel[2], pel[3]) end
            PX.injury_targets(id,p,nil,nil) -- physics reset may have re-enabled missing bodies
            p.qhist, p.idlew, p.qfoot = nil, nil, nil
            p.aim = nil
            -- soft servo for the first moments, and the spawn stretch watch
            if body.repose_start then
                body.ramp_at = now
                p.spawn_watch = { t0 = now, max = 0, bone = "-", why = body.repose_start }
                body.repose_start = nil
            end
            return
        end
        PX.stalled = nil
        local pel_err = drive_v2(id, p, body, cur, fresh, now, holding, cut_changed)
        -- Re-pose (first half): a limb stalled against a joint limit for
        -- > STALL_FRAMES frames, not because of us (no yield, our pawn not
        -- near): physics off for one frame, so every body takes the mesh's
        -- animated (untwisted) pose, then the servo starts over from there.
        if PX.stalled and not p.yielding and not p.near_us and now - (body.repose_at or -1e9) > 2000 then
            Log("pose peer %d: %s stalled off target (twisted against a joint) for %d frames: re-posing the stand-in",
                id, PURE.V2_SLOTS[PX.stalled] or "?", body.stall and body.stall[PX.stalled] or 0)
            PX.start_repose(p, body, now, nil)
        end
        if pel and pel_err and pel_err > SNAP_BODY_ERR then
            body.err_since = body.err_since or now
            if now - body.err_since >= SNAP_BODY_MS and snap_clear(pel) then
                snap_mesh(body, pel[1], pel[2], pel[3])
                if body.sv and body.sv.err then body.sv.err.rigid = (body.sv.err.rigid or 0) + 1 end
                ghost(body, now, "after a rigid snap")
                body.err_since = nil
                p.qhist, p.idlew, p.qfoot = nil, nil, nil   -- no quality history across a snap
            end
        else
            body.err_since = nil
        end
        return
    end
    if not cur or now - (pl.seq_at or -1e9) > PLAY_STALE_MS or cur.mode == "stale" then
        if p.driving then
            Log("pose peer %d: no live pose (%s); handing stand-in back to its muscles",
                id, cur and (cur.mode == "stale" and "sender stale" or "play file not updating") or "no play file")
            release_standin(p)
        end
        if not cur and not pl.warned_nofile and now - (p.claimed_at or now) > 3000 then
            pl.warned_nofile = true
            Log("pose peer %d: no PeerPlay sample yet (is hsmp-sidecar attached and receiving pose?)", id)
        end
        return
    end
    if type(r) ~= "table" then return end   -- nothing new since last frame

    -- Root (capsule) on the same playback clock.
    local cut_changed = (p.cut_seen ~= nil and cur.cut ~= p.cut_seen)
    if cur.root then
        drive_root(p.actor, { X = cur.root[1], Y = cur.root[2], Z = cur.root[3] }, cur.root[4], cut_changed)
    end
    local body = p.in_range and p.body or nil
    if not body or #body.bones == 0 or cur.nbones == 0 then p.cut_seen = cur.cut; return end
    if body.ctl ~= "handles" and body.ctl ~= "velocity" then return end   -- tick sets up control

    if not p.driving then
        set_muscles_blocked(p, true)
        set_motor_strength(body, 0.0)
        p.driving = true
    end
    local targets = PURE.retarget(cur.bones, body.lens)
    p.targets = targets

    -- Teleport / respawn / round reset: snap now, don't drag the body there.
    local pel = targets.Pelvis
    if pel then
        if cut_changed and snap_clear(pel) then
            snap_mesh(body, pel[1], pel[2], pel[3])
            Log("pose peer %d: discontinuity #%d (teleport/respawn) -> snapped stand-in", id, cur.cut)
            body.err_since = nil
        else
            local c; pcall(function() c = body.mesh:GetSocketLocation(fname("Pelvis")) end)
            if c and PURE.d3(pel, { c.X, c.Y, c.Z }) > SNAP_BODY_ERR then
                body.err_since = body.err_since or now
                if now - body.err_since >= SNAP_BODY_MS and snap_clear(pel) then
                    snap_mesh(body, pel[1], pel[2], pel[3])
                    body.err_since = nil
                end
            else
                body.err_since = nil
            end
        end
    end
    p.cut_seen = cur.cut
    apply_pose(body, targets, now)
    -- A world item in that hand is moved by HSMPWorld (world-replication.md): don't grab it too.
    local wh = world_held_for(id)
    local side = targets._wpn_hand == "Hand_L" and "L" or "R"
    if wh and wh[side] then
        if body.wpn and body.wpn.ok and body.wpn.h and body.wpn.h:IsValid() then
            pcall(function() body.wpn.h:ReleaseComponent() end)
            body.wpn.ok = false
        end
        p.wpn_state = "world"
    else
        p.wpn_state = apply_weapon(p, body, targets.Weapon, targets._wpn_hand)
    end
end

local _frame_cost, _frame_n = 0, 0
local function on_frame()
    if not show_avatars or next(puppets) == nil then return end
    -- Never touch a stand-in until the tick has validated this world.
    if cache_gen ~= world_gen then return end
    local t0 = now_ms()
    local c0 = os.clock()
    -- The coming physics step is predicted to be as long as this frame's
    -- (GetWorldDeltaSeconds read here = the step that just ran).
    -- Frame time: os.clock() is 1 ms-quantised, too coarse for a smooth
    -- playback clock (1 ms = 1 uu on a 1000 uu/s hand). Use the world's real
    -- time (one value per frame) anchored to the os.clock base, like HSMPSync.
    -- No PlayerController lookup (a full FindAllOf) per frame: the
    -- GameplayStatics CDO (never freed) and a stand-in of this validated world
    -- as the world context.
    pcall(function()
        local gs = PX.gs
        if not (gs and gs:IsValid()) then gs = StaticFindObject("/Script/Engine.Default__GameplayStatics"); PX.gs = gs end
        local me
        for _, p in pairs(puppets) do
            if p.gen == world_gen and p.actor and p.actor:IsValid() then me = p.actor; break end
        end
        if not (gs and me) then return end
        local d = gs:GetWorldDeltaSeconds(me)
        PX.d_raw = d
        -- Physics never steps more than MaxPhysicsDeltaTime (UE default 1/30 s).
        if type(d) == "number" and d == d then
            _frame_dt = clamp(d, 1 / 240, 1 / 30)
            -- The servo's step estimate: an EMA of the frame time. With the raw last
            -- frame dt (frames of 7-18 ms) every correction over- or undershoots by
            -- the next frame's dt / the last one, which makes the stand-in visibly
            -- rougher than its targets under motion (jitter_ratio 1.4-2.0).
            PX.dt_s = PX.dt_s and (PX.dt_s + 0.15 * (_frame_dt - PX.dt_s)) or _frame_dt
        end
        local rt = gs:GetRealTimeSeconds(me) * 1000
        PX.rt_raw, PX.clk_raw = rt, t0
        -- Step bookkeeping for the v2 aim (see drive_v2): d_now = the step that
        -- just ran (the bodies stand for its end); h_pred = the coming step.
        -- With t.MaxFPS pacing consecutive world deltas are mostly identical
        -- (median |d(k+1) - d(k)| 0.007 ms), so the last delta predicts the
        -- next one; when this callback came late (a long frame in progress,
        -- os.clock gap differs by > 2.5 ms) the gap predicts better (tdiag:
        -- p95 error 6.0 vs 10.8 ms). Substepping (16 x 5 ms) caps a step at 80 ms.
        if type(d) == "number" and d == d then
            local dms = d * 1000
            local g0 = PX.clk_prev and (t0 - PX.clk_prev) or nil
            local hp = (g0 and g0 > 0 and math.abs(g0 - dms) > 2.5) and g0 or dms
            PX.d_now = clamp(dms, 0, 80)
            PX.h_pred = clamp(hp, 4, 80)
            -- Ask the sidecar to evaluate the poses where the bodies will stand
            -- after the coming step (interpolated from its buffer where it can),
            -- so the aim is not a 2-frame extrapolation at low frame rates.
            local lead = (TUNE.v1aim == 1) and 0 or math.floor((PX.xoff_last or PX.d_now) + PX.h_pred + 0.5)
            if math.abs(lead - (PX.lead_w or -99)) >= 3 or t0 - (PX.lead_at or -1e9) > 1000 then
                PX.lead_w, PX.lead_at = lead, t0
                if HSMP_IPC then HSMP_IPC.put_lead(lead) end   -- PoseLead slot
            end
        end
        PX.clk_prev = t0
        local off = t0 - rt
        if _clk_off == nil or off < _clk_off or off - _clk_off > 100 then _clk_off = off end
        t0 = rt + _clk_off
    end)
    -- Frame budget: stand-ins are driven round-robin from a rotating
    -- start; once this frame's Lua cost passes PX.BUDGET_MS the rest wait for
    -- the next frame (their servo catches up), and a stand-in out of
    -- POSE_RANGE (capsule only, no servo) is driven every 4th frame.
    PX.frame_no = (PX.frame_no or 0) + 1
    local ids = {}
    for id in pairs(puppets) do ids[#ids + 1] = id end
    table.sort(ids)
    local nids = #ids
    PX.rr = ((PX.rr or 0) % math.max(nids, 1)) + 1
    local spent = 0
    for k = 0, nids - 1 do
        local id = ids[(PX.rr + k - 1) % nids + 1]
        local p = puppets[id]
        local due = p.in_range ~= false or (PX.frame_no + id) % 4 == 0
        if spent > PX.BUDGET_MS and k > 0 then
            PX.skipped = (PX.skipped or 0) + 1
        elseif due and p.gen == world_gen and p.actor and p.actor:IsValid() then
            local s0 = os.clock()
            local ok, err = pcall(drive_frame, id, p, t0)
            spent = spent + (os.clock() - s0) * 1000
            PX.cost = (PX.cost or 0) + (os.clock() - s0) * 1000
            PX.driven = (PX.driven or 0) + 1
            if not ok and not p.frame_err then
                p.frame_err = true
                Log("pose peer %d: frame driver error: %s", id, tostring(err))
            end
            -- Cost benchmark (tune knob "bench" = N): run the complete
            -- per-stand-in frame work (pose_play read + unpack, servo, writes) N
            -- times per stand-in, as if N stand-ins were driven.
            for _ = 2, (TUNE.bench or 1) do
                if p.play then p.play.seq = nil end
                pcall(drive_frame, id, p, t0)
            end
        end
    end
    pcall(function()
        local lp, lm = local_pelvis()
        if not lp then return end
        if PX.local_watch then PX.watch_pawn(lm, t0) end
        local near = math.huge
        for _, p in pairs(puppets) do
            local tp = p.targets and p.targets.Pelvis
            if tp then
                local d = PURE.d3(tp, { lp.X, lp.Y, lp.Z })
                if d < near then near = d end
            end
        end
        if near < contact.mind then contact.mind = near end
        -- Our pelvis speed from its frame-to-frame motion (the body velocity
        -- getters read 0 on this mesh).
        local pv = contact.prev
        contact.prev = { lp.X, lp.Y, lp.Z, t0, world_gen }
        if not pv or pv[5] ~= world_gen then return end
        local dt = (t0 - pv[4]) / 1000
        if dt < 0.004 or dt > 0.25 then return end
        local v = { X = (lp.X - pv[1]) / dt, Y = (lp.Y - pv[2]) / dt, Z = (lp.Z - pv[3]) / dt }
        local sp = math.sqrt(v.X * v.X + v.Y * v.Y + v.Z * v.Z)
        -- A teleport (spawn placement, its hold corrections), not physics: seen as
        -- 4000-8000 uu/s over one frame right after a new-round placement.
        if sp > 20000 or sp * dt > 150 then return end
        if sp > contact.peak then contact.peak = sp end
        if near < LAUNCH_NEAR * 2 and sp > contact.near_peak then contact.near_peak = sp end
        -- Only in the fight: before Live our pawn is placed and held (protected, no
        -- collision with stand-ins), and those moves read as launches.
        if near < LAUNCH_NEAR and sp > LAUNCH_SPEED and t0 - contact.last_clamp > 200 and PQ.live() then
            contact.last_clamp = t0
            contact.clamps = contact.clamps + 1
            local k = LAUNCH_KEEP / sp
            lm:SetAllPhysicsLinearVelocity({ X = v.X * k, Y = v.Y * k, Z = v.Z * k }, false)
            Log("contact: launch clamp on our pawn (%.0f uu/s at %.0f uu from a stand-in -> %d uu/s)", sp, near, LAUNCH_KEEP)
            ev("pawn_correction", { why = "launch_clamp", live = PQ.live() and true or false, dist_cm = 0, speed = sp })
        end
    end)
    _frame_cost = _frame_cost + (os.clock() - c0) * 1000   -- Lua cost of this frame (ms)
    _frame_n = _frame_n + 1
end

-- --- per-tick bookkeeping for a claimed puppet ------------------------------------

-- The stand-in actor's FName (cached per actor address; nil if unknown).
function PX.actor_name(p)
    if not (p and p.actor) then return nil end
    if p.name_of_addr and p.name_of_addr == p.addr and p.actor_fname then return p.actor_fname end
    local n; pcall(function() n = p.actor:GetFName():ToString() end)
    p.actor_fname, p.name_of_addr = n, p.addr
    return n
end
-- Who the camera follows (HSMPMatch, bus key "spectate", set after my death;
-- target 0 = nobody). Re-read at most 4x a second; a torn / absent read
-- keeps the last value.
PX.spec = { at = -1e9, target = nil }
function PX.spectate_target()
    local now = os.clock()
    if now - PX.spec.at < 0.25 then return PX.spec.target end
    PX.spec.at = now
    local t = HSMP_IPC and HSMP_IPC.bus_table("spectate")   -- HSMPMatch, bus key "spectate"
    if type(t) ~= "table" then return PX.spec.target end
    local tg = math.tointeger(tonumber(t.target))
    PX.spec.target = (tg and tg ~= 0) and tg or nil   -- 0 = nobody
    return PX.spec.target
end
-- The view point for the range test: the player camera's location (fresh
-- this tick through the current PlayerController), else nil (= our pawn).
function PX.camera_loc(pc)
    local l
    pcall(function()
        local cm = pc.PlayerCameraManager
        if cm and cm:IsValid() then
            local v = cm:GetCameraLocation()
            if v and v.X then l = { X = v.X, Y = v.Y, Z = v.Z } end
        end
    end)
    return l
end

local function tick_puppet(id, p, me_loc)
    local now = now_ms()
    local was_in_range = p.in_range
    p.in_range = true
    -- Range is measured from the VIEW (the camera manager; after my death
    -- HSMPMatch moves only the camera to the spectated fighter), with 10%
    -- hysteresis (no release / re-drive flapping at the edge), and the
    -- stand-in the camera follows (bus key "spectate") is always in range.
    local view = PX.view_loc or me_loc
    if view and PX.spectate_target() ~= id then
        local l = willie_loc(p.actor)
        if l then
            local d = PURE.d3({ l.X, l.Y, l.Z }, { view.X, view.Y, view.Z })
            p.in_range = d <= ((was_in_range ~= false) and POSE_RANGE * 1.1 or POSE_RANGE)
        end
    end
    -- Leaving POSE_RANGE: the body is no longer servo-driven, so hand
    -- it back (gravity, world collision, muscles) instead of leaving a limp,
    -- weightless ragdoll hanging in the air. Back in range, the drive is set
    -- up again and the snap path puts it on its target.
    if was_in_range and not p.in_range and p.driving then
        Log("pose peer %d: out of range (> %d uu): stand-in released until it is back", id, POSE_RANGE)
        release_standin(p)
    end
    local body = p.in_range and puppet_body(p) or nil
    if body and not p.dumped then p.dumped = true; pcall(dump_standin, p); harden_standin(p) end

    if body and p.driving then
        set_motor_strength(body, 0.0)   -- re-asserted: BP may reset it
        if body.ctl ~= "servo" then ensure_handles(p, body)
        else
            -- BoneCore (invisible helper skeleton) of a claimed foe can be left
            -- kinematic with collision on, copying the pose a frame late and
            -- shoving the driven bodies (seen: hand 77 deg held off target).
            -- On a local pawn it simulates far below the map; off here.
            pcall(function() local bc = p.actor.BoneCore; if bc and bc:IsValid() and bc:GetCollisionEnabled() ~= 0 then bc:SetCollisionEnabled(0) end end)
            -- Off while the owner stands (the servo holds the feet; see drive start). A fallen or
            -- downed owner lies on the floor: the stand-in collides with it again instead of
            -- floating above or sinking through it (servo and gravity unchanged; tune downed_world 0).
            local down = TUNE.downed_world ~= 0 and PX.owner_vitals_down(id) == true
            if down ~= (p.down_world == true) then
                Log("pose peer %d: owner %s: stand-in world collision %s", id, down and "down" or "up", down and "on" or "off")
                p.down_world = down
            end
            world_collision(body, down)   -- re-asserted (see drive start)
            -- Gravity off is re-asserted too: anything that recreates the
            -- mesh's physics state (collision toggles, the BP's own resets)
            -- turns it back on, and a cached "off" would hide that for good.
            body.gravity = nil
            set_gravity(body, false)
            PX.grip_constraints(p, now)   -- re-scanned each 1 s (grips are re-made on pick-up/drop)
            PX.open_limits(p, body, now)
            -- The BP resets the "Block * Muscles" flags (seen in-game): re-assert.
            p.muscles_blocked = nil
            set_muscles_blocked(p, true)
        end
        -- A stand-in without simulated bodies can't be driven at all (it
        -- would sit in its reference pose = "T-pose"): turn physics on.
        if #body.sims == 0 and now - body.made_at > NOSIM_FIX_MS and not body.sim_fixed then
            body.sim_fixed = true
            pcall(function() body.mesh:SetSimulatePhysics(true) end)
            local s = false
            pcall(function() s = body.mesh:IsSimulatingPhysics(fname("Pelvis")) end)
            Log("pose: stand-in %s had NO simulated bodies on %s; SetSimulatePhysics(true) -> pelvis sim=%s",
                p.actor:GetFName():ToString(), tostring(body.field), tostring(s))
            if s then body.sims = { body.mesh }; for _, rec in pairs(body.handles) do rec.nobody = false end end
        end
    elseif body and body.ctl == nil and not (p.last and p.last.v2) then
        ensure_handles(p, body)   -- prepare control before the first (v1) pose
    end
    if p.body then PX.injury_targets(id,p,nil,nil) end -- includes released/out-of-range bodies
    -- Server-declared death of the owner (peer_vitals record,
    -- docs/development/subsystems/vitals.md): the stand-in becomes a free
    -- native ragdoll (drive_frame lets go).
    do
        -- A torn / absent read is no evidence: keep the last complete value
        -- (counting it as alive would servo-drive a corpse for a frame or two).
        local dead = p.vdead == true
        local vd = PX.owner_vitals_dead(id)
        if vd ~= nil then dead = vd end
        -- A "dead" flag that outlives a respawn (seen: the owner's vitals kept
        -- dead=1 with hp 100 after the round restart while his pose showed
        -- him alive and moving) must not freeze the stand-in for the rest of
        -- the round: once the pose has a discontinuity (cut =
        -- respawn/teleport) after the death, the flag is ignored until it
        -- clears and is raised again.
        local cut = p.last and p.last.cut or 0
        if dead and not p.vdead then p.dead_cut = cut end
        if not dead then p.dead_cut = nil end
        p.vdead = dead
        local stale_flag = dead and p.dead_cut ~= nil and cut ~= p.dead_cut
        if stale_flag and not p.dead_ignored then
            Log("pose peer %d: owner's vitals still say dead after a respawn (cut %d -> %d): driving again", id, p.dead_cut, cut)
        end
        p.dead_ignored = stale_flag
        -- A server-declared death (HSMPCombat played it on this
        -- stand-in) is owner-dead too, even before the owner's vitals say so.
        p.owner_dead = (dead and not stale_flag) or DH.declared(id, PX.actor_name(p))
    end

    local pl, cur = p.play or {}, p.last
    p.diag_at = p.diag_at or now
    if now - p.diag_at >= POSE_DIAG_MS then
        local secs = (now - p.diag_at) / 1000
        p.diag_at = now
        local bavg, bmax, aavg, amax, hmax = 0, 0, 0, 0, 0
        if body and p.driving and p.targets then bavg, bmax, aavg, amax, hmax = pose_error(body, p.targets) end
        Log("pose peer %d: play mode=%s new=%.0f/s reads=%.0f/s torn/missing=%d delay=%.0fms jitter(p90)=%.0fms iv=%.0fms lead=%.0fms age=%.0fms bones=%d | ctl=%s held=%d/%d driving=%s weapon=%s | err body avg=%.0f max=%.0f arms avg=%.0f max=%.0f hands max=%.0f uu | snaps=%d cut=%d | frame cost avg %.3f ms",
            id, cur and cur.mode or "none", (pl.fresh or 0) / secs, (pl.reads or 0) / secs, pl.bad or 0,
            cur and cur.delay or 0, cur and cur.jit or 0, cur and cur.iv or -1, cur and cur.lead or 0, cur and cur.age or -1, cur and cur.nbones or 0,
            body and body.ctl or "-", body and body.held or 0, body and #body.bones or 0, tostring(p.driving),
            tostring(p.wpn_state or "-"), bavg, bmax, aavg, amax, hmax,
            body and body.snaps or 0, cur and cur.cut or 0,
            _frame_n > 0 and _frame_cost / _frame_n or 0)
        pl.fresh, pl.reads, pl.bad = 0, 0, 0
        if body and body.sv and body.ctl == "servo" then
            local e = body.sv.err
            Log("pose peer %d: servo v2 %d bodies | tracking err (vs last frame's aim) avg %.2f max %.2f uu, rot avg %.2f max %.2f deg, hands max %.2f uu | worst pos %s rot %s | capped %d | dt %.1f ms | owner_dead=%s | weapons %s | grips %s",
                id, body.sv.n, e.n > 0 and e.e / e.n or 0, e.emax, e.n > 0 and e.a / e.n or 0, e.amax, e.hmax,
                PURE.V2_SLOTS[e.wi or 0] or "-", PURE.V2_SLOTS[e.ai or 0] or "-",
                e.capped, _frame_dt * 1000, tostring(p.owner_dead), table.concat(p.wpn_states or {}, " "), PX.grips_desc(p))
            if e.ci then
                local function pq(t, q)
                    if #t == 0 then return -1 end
                    table.sort(t); return t[math.max(1, math.ceil(#t * q))]
                end
                local ci = e.ci
                if #ci.near > 0 then
                    Log("pose peer %d: contact impulse near our pawn p50 %.0f p95 %.0f max %.0f kg*uu/s (%s), baseline far p95 %.0f, n %d/%d%s",
                        id, pq(ci.near, 0.5), pq(ci.near, 0.95), ci.maxn, ci.maxb, pq(ci.far, 0.95), #ci.near, #ci.far,
                        p.yielding and " [yielding]" or "")
                    ev("x_pose_contact", { peer = id, near_p50 = pq(ci.near, 0.5), near_p95 = pq(ci.near, 0.95), near_max = ci.maxn,
                        body = ci.maxb, far_p95 = pq(ci.far, 0.95), n = #ci.near, yielding = p.yielding or false })
                end
            end
            if e.pb then
                local parts = {}
                for i = 1, PURE.V2_NB do
                    local b = e.pb[i]
                    if b and b[3] > 0 then
                        parts[#parts + 1] = string.format("%s %.1f/%.1f/t%.1f%s", PURE.V2_SLOTS[i], b[1] / b[3], b[2] / b[3], (b[5] or 0) / b[3],
                            b[4] > 0 and string.format("(c%d)", b[4]) or "")
                    end
                end
                Log("pose peer %d: per-body err uu/deg: %s", id, table.concat(parts, " "))
                -- "Broken back" watch: a spine/neck that stays far off its target
                -- (capped every frame) is held by something; dump the stand-in's
                -- BP state once per 30 s so the cause shows up in the log.
                local sp = e.pb[6] or e.pb[7]
                local bad = (sp and sp[3] > 0 and sp[2] / sp[3] > 20) or (e.n > 0 and e.a / e.n > 8)
                if bad and now - (p.bent_dump_at or -1e9) > 20000 then
                    p.bent_dump_at = now
                    local kv = {}
                    for _, k in ipairs({ "Fallen", "Downed", "DED", "Consciousness", "Health", "Pain", "Pain Shock",
                        "Bend Over Extreme", "Head Tonus", "All Body Tonus", "Upper Body Tonus", "Muscle Power",
                        "Constraint Rate", "Activate Constraint", "Being Grabbed", "Grabbed R", "Grabbed L",
                        "Neck Dislocated", "Spine Dislocated", "Back Broken", "Neck Snapped", "Force Fall",
                        "Get Up Rate", "Fallen Rate", "Hit Rigidity", "Block Body Muscles", "Block Arm Muscles", "Block Upper Leg Muscles", "Block Lower Leg Muscles", "Block Shoulder Muscles", "Invulnerable", "Stamina", "Pos Constraint", "Rot Constraint", "Constraint Rate Proxy", "Root Linear Constraint Power", "Root Angular Constraint Power" }) do
                        local v; pcall(function() v = p.actor[k] end)
                        kv[#kv + 1] = k .. "=" .. tostring(v)
                    end
                    Log("pose peer %d: BENT/OFF stand-in (spine/neck or whole body held off target): %s", id, table.concat(kv, " | "))
                end
            end
            -- Gate metric: arm-chain and blade-tip tracking p95 + display latency.
            local function p95(t)
                if not t or #t == 0 then return -1 end
                table.sort(t)
                return t[math.max(1, math.ceil(#t * 0.95))]
            end
            local lead = TUNE.lead or 0
            -- Smoothness (stand-in second-difference energy / that of its
            -- targets, moving bodies only), planted-foot speed, idle RMS.
            -- jitter_ratio: worst body's sqrt((stand-in + floor) / (targets +
            -- floor)) second-difference energy; the floor (0.05 uu per axis
            -- per frame, below visibility) keeps a still owner from turning
            -- sub-0.1 uu solver noise into a huge ratio (hsmp-pose-truth uses
            -- the same floor). foot_slide_p95 / idle_rms: this window's value,
            -- else the last one measured (a fighter is rarely planted/idle for
            -- a whole window); never measured -> field omitted.
            local jr, fs, ir = nil, nil, nil
            -- A body judged on fewer than PQ.JIT_MIN_N moving frames (1/6 s)
            -- has no smoothness to judge: its "ratio" is one or two second
            -- differences (seen: pelvis r=7.28 from n=1, foot r=5.68 from
            -- n=7, while the same windows' bodies with n >= 25 read 0.9-1.3);
            -- it is left out like a still body.
            if e.q then
                for i, _ in pairs(QBODY) do
                    local hr, ha, hn = e.q.hr[i], e.q.ha[i], e.q.hn and e.q.hn[i]
                    if hr and ha and hn and hn >= PQ.JIT_MIN_N then
                        local fl = 0.0075 * hn
                        local r = math.sqrt((hr + fl) / (ha + fl))
                        if r > (jr or 0) then
                            -- evidence: per-frame RMS second difference (uu) of the
                            -- stand-in and of its targets for the worst body
                            p.q_jdbg = string.format("%s r=%.2f standin %.3f target %.3f uu/frame n=%d", QBODY[i], r,
                                math.sqrt(hr / hn), math.sqrt(ha / hn), hn)
                        end
                        jr = math.max(jr or 0, r)
                    end
                end
                if #e.q.foot > 0 then fs = p95(e.q.foot) end
                for _, v in ipairs(e.q.idle) do ir = math.max(ir or 0, v) end
            end
            -- No idle window closed yet in this sample: an open one long enough
            -- counts (the first sample of a world must carry idle_rms too).
            if ir == nil then ir = PQ.idle_window_rms(p.idlew) end
            p.q_last = p.q_last or {}
            -- jitter_ratio is NOT carried forward: a window with no moving body
            -- has no smoothness to judge (-1), and re-reporting the previous
            -- window's value would count one rough window twice.
            if fs then p.q_last.fs = fs else fs = p.q_last.fs end
            if ir then p.q_last.ir = ir else ir = p.q_last.ir end
            -- Always present (the gate reads all three): -1 = not applicable in this Live
            -- window (nothing moved / no planted foot / the owner was never idle).
            jr, fs, ir = jr or -1, fs or -1, ir or -1
            local vis = true
            pcall(function() vis = body.mesh:WasRecentlyRendered(0.5) end)
            -- latency_ms: sample age (half a send interval) + jitter buffer +
            -- one servo frame - lead; network transit is NOT included (the
            -- probe's ground truth measures it end to end).
            local q = { peer = id, arm_p95_uu = p95(e.arm), tip_p95_uu = p95(e.tip),
                        latency_ms = ((e.qn or 0) > 0 and e.lat_sum) and (e.lat_sum / e.qn) or (cur and math.max(0, 8 + (cur.delay or 0) + _frame_dt * 1000 - lead) or -1),
                        buffer_ms = cur and cur.delay or -1, lead_ms = lead, n = e.n,
                        jitter_ratio = jr, foot_slide_p95 = fs, idle_rms = ir, rendered = vis and 1 or 0 }
            jr, fs, ir = jr or -1, fs or -1, ir or -1
            -- Not rendering (minimized): bones are not refreshed, metrics invalid.
            -- Exactly the gate contract shape (docs/development/testing.md); the
            -- diagnostics (buffer, lead, n, rendered) stay in the log line.
            -- Only a window with >= 0.5 s of Live (owner alive) is a gate sample.
            local qframes = e.qn or 0
            if vis and qframes >= 30 then ev("pose_quality", { peer = q.peer, arm_p95_uu = q.arm_p95_uu, tip_p95_uu = q.tip_p95_uu, latency_ms = q.latency_ms,
                jitter_ratio = q.jitter_ratio, foot_slide_p95 = q.foot_slide_p95, idle_rms = q.idle_rms }) end
            -- SMOOTH-1: rubber banding of this stand-in over the same window.
            local nf = e.nf or { n = 0, snaps = 0, max = 0 }
            local win = math.max(0.5, (now - (p.nf_t0 or (now - 5000))) / 1000)
            p.nf_t0 = now
            if vis and qframes >= 30 then ev("netfeel", { peer = id, frames = nf.n, snaps_per_min = nf.snaps * 60 / win,
                jump_max_uu = nf.max, rigid_snaps = e.rigid or 0, clock_resets = e.clkr or 0,
                buffer_ms = cur and cur.delay or -1, jitter_ms = cur and cur.jit or -1, window_s = win }) end
            Log("pose peer %d: pose_quality%s arm_p95=%.2f uu tip_p95=%.2f uu jitter_ratio=%.2f foot_slide_p95=%.1f uu/s idle_rms=%.2f uu latency~%.0f ms (sample age 8 + buffer %.0f + 1 servo frame - lead %.0f; excludes network transit) | jitter worst: %s",
                id, (not vis and " (NOT RENDERED: not reported)") or (qframes < 30 and string.format(" (%d Live frames: not reported)", qframes)) or "", q.arm_p95_uu, q.tip_p95_uu, jr, fs, ir, q.latency_ms, q.buffer_ms, lead, tostring(p.q_jdbg))
            p.q_jdbg = nil
            body.sv.err = { n = 0, e = 0, emax = 0, a = 0, amax = 0, hmax = 0, capped = 0 }
        end
        if POSE_PROBE then probe_flush(id, true) end
        _frame_cost, _frame_n = 0, 0
    end
end

-- --- main tick ---------------------------------------------------------------

local function on_tick()
    tick_num = tick_num + 1
    if tick_num % 30 == 0 then refresh_settings() end
    PX.poll_dev()                                       -- dev_cmd TUNE records (DevCtl ring)
    if not tick_hook.ok then tick_hook.try(false) end   -- 1 Hz until registered

    -- World guard FIRST: nothing cached may be touched before this.
    local pc = local_pc()
    local wname, wid = world_identity(pc)
    local key = wid and (tostring(world_gen) .. "|" .. wid) or nil
    -- A tick without a world identity (no PC for a moment) only skips;
    -- it does not drop live stand-ins unreleased (a re-claim would then
    -- capture tonus0 = 0). A real world change always shows up as a new key:
    -- world_gen (bumped by the LoadMap hooks) is part of it.
    if key and key ~= cache_world then
        if cache_world ~= nil then drop_caches("world changed -> " .. tostring(wname)) end
        cache_world = key
        census.arena_at = os.clock()   -- a round reset reloads the SAME map: the name alone never changes
    end
    -- ... unless the LoadMap pre-hook is missing: then a level change can only
    -- be seen as the world going away, and that must drop (stale-object rule).
    if not key and cache_world ~= nil and not tick_hook.loadmap_ok then
        drop_caches("no valid world"); cache_world = nil
    end
    if not key then cache_gen = -1; _world_name = nil; return end
    cache_gen = world_gen
    PX.settle_world=wid
    _world_name = wname
    PX.keep_possession()
    local world = wname
    if world ~= last_world then
        last_world = world
        census.arena_at = os.clock()
    end
    if not show_avatars or not is_gameplay(world) then
        if next(puppets) ~= nil then reset_all("not in gameplay / avatars off") end
        return
    end
    -- Let HSMPSync finish possessing our own Willie first, so neither mod can
    -- mistake the other's Willie during spawn.
    local me = local_pawn(pc)
    if not me then return end
    -- Nothing touches Willies in the first PX.WORLD_SETTLE_S of a world: the
    -- previous world is still being purged incrementally and the new world's
    -- player Willie is mid-construction. Claiming then crashes the game
    -- 0.5-0.6 s after a round reload (GetFName / FName:ToString on a dead
    -- object, right after the first claim pass).
    if os.clock() - (census.arena_at or 0) < PX.WORLD_SETTLE_S then return end
    if PX.bodyphysics_request then
        local request = PX.bodyphysics_request
        PX.bodyphysics_request = nil
        PX.bodyphysics(request)
    end
    if PX.bodyheight_request then
        local request = PX.bodyheight_request
        PX.bodyheight_request = nil
        PX.bodyheight(request)
    end
    if PX.weaponstate_request then
        local request=PX.weaponstate_request
        PX.weaponstate_request=nil
        PX.weaponstate(request)
    end
    -- A diagnostic must restore even when no fresh pose reaches drive_v2.
    for id,p in pairs(puppets) do
        PX.height_tick(p)
        if p.gen == world_gen and p.actor and p.actor:IsValid() and p.body and p.body.mesh:IsValid()
            and (p.body.injury_probe or next(p.body.injury_disabled or {})) then
            PX.injury_tick(id,p)
        end
    end
    local me_loc = willie_loc(me)
    PX.view_loc = PX.camera_loc(pc) or me_loc   -- stand-in range from the camera
    do
        local addr; pcall(function() addr = me:GetAddress() end)
        if not local_body or local_body.gen ~= world_gen or local_body.addr ~= addr then
            local m = pick_pose_mesh(me)
            local_body = m and { mesh = m, gen = world_gen, addr = addr } or nil
            PX.local_watch = local_body and { t0 = now_ms(), max = 0, bone = "-", n = 0 } or nil
        end
    end
    if next(puppets) ~= nil and os.clock() - contact.last_log >= 5 then
        contact.last_log = os.clock()
        Log("contact: our peak speed %.0f uu/s (%.0f while within %d uu of a stand-in), closest %.0f uu, handle backoffs %d, snaps held %d, launch clamps %d",
            contact.peak, contact.near_peak, LAUNCH_NEAR * 2, contact.mind == math.huge and -1 or contact.mind,
            contact.backoffs, contact.snaps_held, contact.clamps)
        contact.peak, contact.near_peak, contact.mind = 0, 0, math.huge
        contact.backoffs, contact.snaps_held, contact.clamps = 0, 0, 0
        -- Per-stand-in driver cost (target <= 1 ms), budget skips.
        if (PX.driven or 0) > 0 then
            -- Native servo A/B evidence: native calls used / refused (cumulative) per part.
            local function nat(s) return s and string.format("%d/%d", s.used or 0, s.fell or 0) or "-" end
            Log("frame cost: %.3f ms per stand-in frame (%d drives, %d deferred by the %d ms budget) | ReceiveTick hook calls %d, driven %d | native used/fell servo %s neutralise %s wservo %s",
                (PX.cost or 0) / PX.driven, PX.driven, PX.skipped or 0, PX.BUDGET_MS, PX.hook_calls or 0, PX.hook_hits or 0,
                nat(PX.NSV), nat(PX.NNV), nat(PX.NWV))
        end
        PX.cost, PX.driven, PX.skipped, PX.hook_calls, PX.hook_hits = 0, 0, 0, 0, 0
    end

    update_snapshots()
    local live = live_peers()
    local live_count, missing = 0, 0
    for _ in pairs(live) do live_count = live_count + 1 end
    -- The whole demand BEFORE the claim loop: every streaming peer without a
    -- stand-in, and where each one stands (one native spawn call asks for all
    -- of them; the loop's running count would ask for 1).
    local demand, demand_at = 0, {}
    for id in pairs(live) do
        local s = snap[id]
        if not puppets[id] and s and (tick_num - s.changed_at) <= STALE_TICKS then
            demand = demand + 1
            local at = PX.spawn_spot(id, s)
            if at then demand_at[#demand_at + 1] = at end
        end
    end

    for id, nick in pairs(live) do
        local p = puppets[id]
        -- Re-claim when the puppet vanished or died / was dismembered locally.
        if p then
            local why
            if not (p.actor and p.actor:IsValid()) then
                why = "puppet invalid"
            else
                local hp = willie_health(p.actor)
                local dead = (hp and hp <= 0) or willie_dead(p.actor)
                if dead then
                    -- HSMPCombat zeroes a stand-in's Health to play the OWNER's
                    -- real death (server-declared, the `standin_dead` bus record) and the
                    -- owner's own vitals say dead for a native death: keep that
                    -- corpse ragdolled until the round reset / respawn.
                    local owner_dead = DH.declared(id, PX.actor_name(p))
                    if not owner_dead then
                        owner_dead = PX.owner_vitals_dead(id) or false
                    end
                    if owner_dead then
                        p.local_dead_at = nil
                    else
                        -- The owner's vitals lag S2CDeath by more than a tick:
                        -- a local death must persist before it re-claims.
                        p.local_dead_at = p.local_dead_at or os.clock()
                        if os.clock() - p.local_dead_at >= DH.GRACE_S then
                            why = string.format("puppet died locally (HP %s DED=%s)",
                                tostring(hp), tostring(willie_dead(p.actor)))
                        end
                    end
                else
                    p.local_dead_at = nil
                end
            end
            if why then
                Log("peer %d (%s): %s; re-claiming", id, nick, why)
                local ok,restored=pcall(release_standin,p,true)
                if ok and restored~=false then
                    puppets[id] = nil; p = nil
                    demand = demand + 1   -- a re-claim needs a body too
                    next_claim[id] = tick_num
                end
            end
        end

        local s = snap[id]
        if not p then
            local fresh = s and (tick_num - s.changed_at) <= STALE_TICKS
            -- Only a peer whose pawn is streaming (fresh snapshot)
            -- still needs a stand-in; a rostered peer in the lobby / still
            -- loading must not block the census for the whole round.
            if fresh then missing = missing + 1 end
            if fresh and tick_num >= (next_claim[id] or 0) then
                next_claim[id] = tick_num + CLAIM_RETRY
                local actor = claim_puppet(id, math.max(demand, 1), demand_at, live_count)
                if actor then
                    local addr; pcall(function() addr = actor:GetAddress() end)
                    restore_willie(actor, census.gone[obj_name(actor)] == true)
                    census.gone[obj_name(actor)] = nil
                    puppets[id] = { actor = actor, nick = nick, claimed_at = now_ms(), driving = nil,
                                    gen = world_gen, addr = addr }
                    warned_none[id] = nil
                    demand = math.max(0, demand - 1)   -- this one is served
                    drive_root(actor, s.st.pos, s.st.yaw, true)
                elseif not warned_none[id] then
                    warned_none[id] = true
                    Log("no free native combatant to puppet for peer %d (%s) yet", id, nick)
                end
            end
        else
            local ok, err = pcall(tick_puppet, id, p, me_loc)
            if not ok and not p.tick_err then
                p.tick_err = true
                Log("pose peer %d: tick error: %s", id, tostring(err))
            end
        end
    end

    -- Release puppets only for peers that are actually gone (not in the
    -- roster and no new snapshot for STALE_TICKS), never on a skipped frame.
    for id, p in pairs(puppets) do
        if not live[id] then
            Log("peer %d (%s) gone; releasing puppet (left standing)", id, p.nick or "?")
            local ok,restored=pcall(release_standin,p,true)
            if ok and restored~=false then puppets[id]=nil;snap[id]=nil end
        end
    end

    if tick_num % CENSUS_EVERY == 0 then
        local ok, err = pcall(census_tick, me, live_count, missing)
        if not ok then Log("census error: %s", tostring(err)) end
    end
    if live_count > 0 and missing == 0 and tick_num % NEUTRAL_EVERY == 0 and not mp_session_active() then
        neutralise_loose_ai()
    end

    -- Stand-ins the ReceiveTick post-hook neutralises (address -> puppet).
    _driven = {}
    for _, p in pairs(puppets) do
        if p.driving and p.addr and p.gen == world_gen and p.body and p.body.ctl == "servo" then _driven[p.addr] = p end
    end

    -- Shared contract for other mods (HSMPLoadout, HSMPCombat): which in-world
    -- Willie stands in for which peer (bus key "puppets", typed rows {peer, name}).
    -- Look up with FindAllOf("Willie_BP_C") + GetFName():ToString().
    if tick_num % 15 == 0 then
        local rows, ids = {}, {}
        for id in pairs(puppets) do ids[#ids + 1] = id end
        table.sort(ids)
        local parts = {}
        for _, id in ipairs(ids) do
            local p = puppets[id]
            if p.actor and p.actor:IsValid() then
                local nm; pcall(function() nm = p.actor:GetFName():ToString() end)
                if nm then
                    rows[#rows + 1] = { peer = id, name = nm }
                    parts[#parts + 1] = id .. "=" .. nm
                end
            end
        end
        local key = table.concat(parts, ",")
        if key ~= _last_puppets_line then write_puppets(key, rows) end
    end

    -- Lag-compensation contract (HSMPCombat -> server rewind): the SENDER-clock
    -- time each peer's stand-in is displaying (bus key "playback", typed rows
    -- {peer, body_ts, arm_ts, local_ms}). Body, arms and weapon share one
    -- timeline, so body_ts == arm_ts; local_ms is when that sample was read, so
    -- the reader can advance it.
    local pb = {}
    for id, p in pairs(puppets) do
        -- Keep the original timestamp and pawn generation after driving stops.
        -- New contacts still enforce freshness; approved death trades need this
        -- immutable binding until the actor or its native life is replaced.
        local row = PURE.playback_row(id, p.shown or p.applied_context, now_ms(), true)
        if row then pb[#pb + 1] = row end
    end
    if HSMP_IPC then HSMP_IPC.bus_put("playback", { rows = pb }) end
end

-- World teardown: LoadMap runs on the game thread; the pre-hook fires before
-- the old world's actors are destroyed. Bump the generation and forget every
-- cached UObject right here (pure Lua, no UE access), so no later callback can
-- reach a stand-in of the old world.
local hook_note = {}
local ok_pre = pcall(function()
    RegisterLoadMapPreHook(function()
        world_gen = world_gen + 1
        cache_gen, cache_world = -1, nil
        drop_caches("LoadMap (world teardown)")
    end)
end)
local ok_post = pcall(function()
    RegisterLoadMapPostHook(function()
        world_gen = world_gen + 1
        cache_gen, cache_world = -1, nil
    end)
end)
hook_note[#hook_note + 1] = "LoadMap pre=" .. tostring(ok_pre) .. " post=" .. tostring(ok_post)
tick_hook.loadmap_ok = ok_pre   -- on_tick may keep caches over a key-less tick only with it

-- A stand-in destroyed mid-world (by the game or another mod): forget it
-- before its memory can be reused. Only Lua tables are touched here.
local ok_end = pcall(function()
    RegisterEndPlayPreHook(function(ctx)
        if next(puppets) == nil then return end
        local a; pcall(function() a = ctx:get() end)
        if not a then return end
        local addr; pcall(function() addr = a:GetAddress() end)
        if not addr then return end
        for id, p in pairs(puppets) do
            if p.addr == addr then
                puppets[id] = nil
                _driven[addr] = nil
                handle_cache[addr] = nil
                next_claim[id] = tick_num
                Log("peer %d: stand-in ended play (destroyed); forgotten, will re-claim", id)
            end
        end
    end)
end)
hook_note[#hook_note + 1] = "EndPlay=" .. tostring(ok_end)

-- Stand-in BP neutralising right after its own tick (before physics): the
-- Willie BP re-sets motors / PhysicalAnimation / tonus every tick, so a
-- per-frame write from the frame driver (which runs after physics) is
-- overwritten before every physics step. Pure-Lua lookup by address; only
-- stand-ins of the current, validated world are touched.
-- Willie_BP is not loaded in the main menu, so a registration at mod load
-- fails ("ReceiveTick=false" in the log) and the post-BP neutralising would
-- never run. Retried once a second from the tick, forever, until it succeeds
-- (one cheap failed lookup; same pattern as HSMPCombat).
-- Pinned UE4SS script hooks execute argument #2 AFTER the Blueprint; the
-- native-only third callback slot is ignored for this non-native function.
tick_hook.FN = "/Game/Character/Blueprints/Willie_BP.Willie_BP_C:ReceiveTick"
function tick_hook.post(ctx)
    if next(_driven) == nil or cache_gen ~= world_gen then return end
    pcall(function()
        local _,wid=world_identity(local_pc())
        if not wid or cache_world~=tostring(world_gen).."|"..wid then return end
        local a=ctx:get();PX.hook_calls=(PX.hook_calls or 0)+1
        if not (a and a:IsValid())then return end
        local addr,name=a:GetAddress(),a:GetFName():ToString()
        local p=_driven[addr]
        if not p or not p.body or p.body.ctl~="servo" or not PX.grip_drive_current(p)then return end
        local shown=p.shown or p.applied_context
        if not shown or shown.pawn~=name or not PURE.pose_context_ok(shown,HSM and HSM.view(),HSM and HSM.mode(),p.peer)then return end
        local mesh,key=PX.injury_mesh(p)
        if not mesh or p.actor:GetFName():ToString()~=name then return end
        local owner=mesh:GetOwner()
        if not (owner and owner:IsValid()) or owner:GetAddress()~=addr or owner:GetFName():ToString()~=name then return end
        local body=p.body
        local fresh,fresh_key=PX.injury_mesh(p)
        if fresh~=mesh and not same(fresh,mesh) or fresh_key~=key or not PX.grip_drive_current(p)then return end
        PX.hook_hits=(PX.hook_hits or 0)+1
        neutralise_bp(p, {mesh=mesh,motors=body.motors}) -- current wrapper, no retained body-mesh access
        if TUNE.tdiag then
            pcall(function()
                local gs = PX.gs
                local l = mesh:GetSocketLocation(_sv_fn[1])
                p.hook = { l.X, l.Y, l.Z, gs and gs:GetRealTimeSeconds(a) * 1000 or -1, os.clock() * 1000 }
            end)
        end
    end)
end
tick_hook.try = function(force)
    if tick_hook.ok then return true end
    if not force and tick_num % 30 ~= 0 then return false end
    tick_hook.tries = tick_hook.tries + 1
    local ok, err = pcall(function() RegisterHook(tick_hook.FN, tick_hook.post) end)
    tick_hook.ok = ok
    if ok then
        Log("ReceiveTick post-hook registered (try %d): BP neutralising active", tick_hook.tries)
    elseif tick_hook.tries == 1 or tick_hook.tries % 60 == 0 then
        Log("ReceiveTick post-hook not available yet (try %d: %s); retrying every 1 s", tick_hook.tries, tostring(err))
    end
    return ok
end
tick_hook.try(true)
hook_note[#hook_note + 1] = "ReceiveTick=" .. tostring(tick_hook.ok) .. (tick_hook.ok and "" or "(retrying)")

-- Diagnostics for ghost hunting: when (relative to world load) each Willie is
-- constructed. Only the name is read; the object is still being built.
-- Thread rule (game thread only): UE4SS runs a NotifyOnNewObject callback on
-- whatever thread constructs the object, and that cannot be marshalled (the
-- callback itself already IS Lua). A Willie built on the async loading
-- thread (its CDO / archetype on a level load) would run this Lua off the
-- game thread, which corrupts the Lua VM. So it is dev-only: registered
-- only with HSMP_AVATARS_NEWOBJ_DIAG=1; the census logs every extra Willie
-- with its timing anyway.
if os.getenv("HSMP_AVATARS_NEWOBJ_DIAG") == "1" then pcall(function()
    NotifyOnNewObject("/Game/Character/Blueprints/Willie_BP.Willie_BP_C", function(obj)
        local nm = "?"; pcall(function() nm = obj:GetFName():ToString() end)
        if nm:find("^Default__") then return end
        Log("census: Willie constructed %s (%.1f s after world load, gen %d)",
            nm, os.clock() - (census.arena_at or 0), world_gen)
    end)
end) end
Log("world guard hooks: %s", table.concat(hook_note, " "))

-- ExecuteInGameThread keeps both loops on the game thread even if the shim
-- above could not install (it is a cheap re-queue when already there).
LoopAsync(TICK_MS, function()
    ExecuteInGameThread(on_tick)
    return false
end)

-- Pose application runs every rendered frame (the sidecar refreshes the
-- pose_play records every few ms), not on the 33 ms bookkeeping tick.
if LoopInGameThreadAfterFrames and EngineTickAvailable ~= false then
    LoopInGameThreadAfterFrames(1, function() pcall(on_frame) end)
    Log("pose driver: every game frame")
else
    LoopAsync(8, function() ExecuteInGameThread(function() pcall(on_frame) end); return false end)
    Log("pose driver: 8 ms game-thread loop (LoopInGameThreadAfterFrames unavailable)")
end

Log("loaded; state_dir=%s (puppet mode, per-frame PhysicsHandle pose control from the PeerPlay slots)", STATE_DIR)

-- Offline test hook (`hsmp-tools lua-test avatars`): never set in game.
if rawget(_G, "HSMP_AVATARS_TEST") then
    HSMP_AVATARS_TEST.api = {
        tick_hook = tick_hook, on_tick = on_tick, local_pawn = local_pawn,
        parse_standin_dead = DH.parse, combat_declared_dead = DH.declared,
        puppets = function() return puppets end,
        set_puppet = function(id, p) puppets[id] = p end,
        set_driven = function(p) _driven[p.addr]=p end,
        PX = PX, drive_frame = drive_frame, drive_v2 = drive_v2, servo_weapon_parts = servo_weapon_parts, set_gravity = set_gravity,
        generation = function() return world_gen, cache_gen end,
        drop_caches = drop_caches,
        snap_mesh = snap_mesh, release_standin = release_standin,
        read_roster = read_roster, roster = function() return roster end,
        tune = function() return TUNE end, caps = function() return SERVO_CAP_LIN, SERVO_CAP_ANG end, PURE = PURE,
    }
end
