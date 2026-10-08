-- Select the process role before registering client hooks or mutating actors.
do
    local ok, role = pcall(require, "hsmp_runtime_role") -- unsafe: ok audited pure role module: startup env and scalar predicates only
    if not ok then
        local source = (debug.getinfo(1, "S").source or ""):gsub("^@", "")
        local directory = source:match("^(.*)[/\\]") or "."
        ok, role = pcall(dofile, directory .. "/../../shared/hsmp_runtime_role.lua") -- unsafe: ok same audited pure role module
    end
    if not ok or type(role) ~= "table" or not role.client() then return end
end

-- HSMPInteract — the interaction channel (docs/development/subsystems/interact.md,
-- capability INTERACT).
--
-- Problem: each player simulates only his own Willie; the opponent on my
-- screen is a stand-in servoed to the owner's replicated pose. My grab or
-- shove on the stand-in never reaches the owner's real body, so without this
-- channel the opponent feels like an immovable, infinite mass.
--
-- Fix: initiator-detected, server-validated, owner-applied.
--
-- DETECTION (my client, I am the grabber / shover):
--   * Grab: every frame, my pawn's per-hand grab state ("Grabbed R",
--     "Grab Component R", "Grab Bone R"; L mirrors). When the grabbed
--     component belongs to a stand-in (bus key puppets), I send grab_start
--     (target peer, hero bone, grip point in that bone's frame = my hand,
--     my hand world position, my clock), grab_update every 100 ms while it
--     holds (the server lease), grab_end when it lets go. While I hold a
--     stand-in it yields (HSMPAvatars compliance via the pose_yield bus key) so my
--     hand is not welded to a stiff servo: it follows its owner's stream.
--   * Impulse: Willie_BP's mesh hit event (BndEvt ... ComponentHitSignature)
--     on a stand-in whose other actor is my pawn (body contact; weapons are
--     separate actors and go through HSMPCombat). Contacts are summed per
--     peer (first one sent at the next frame, then 50 ms windows), sent when
--     the sum reaches the threshold, oriented away from my body. A contact
--     in a frame where my body also dealt that stand-in a real damage hit is
--     dropped while a round is live (HSMPCombat replicates that hit).
--   Typed `interact` records go out with IPC.send; the sidecar (interact_client.rs)
--   mints the wire grab id and frames them; the server (interact/) validates reach
--   against lag-comp history, rate limits, caps impulses and forwards the record.
--
-- APPLICATION (my client, my body is the target), IPC.events("interact"):
--   * impulse: AddImpulseAtLocation on my pawn's simulated mesh at the bone
--     (point back from the bone frame), gain + magnitude cap + a velocity
--     change cap for the bone's mass (no launches);
--   * grab: a soft, force-limited PhysicsHandle on my grabbed bone, pulled
--     toward the grabber's stand-in hand on my screen (the grabber's hand as
--     I see it: physically consistent), falling back to the streamed hand
--     position. The target never leads the grip by more than `grab_lead`
--     (force <= stiffness x lead), so I can struggle; past `grab_break` for
--     `grab_break_ms` I break free locally. Released on grab_end, on a lease
--     timeout (no update for `grab_lease_ms`) and on any world change.
--
-- Safety: game-thread-only shim (no worker-thread Lua), shared world guard
-- (hsmp_wg: every UObject cache dropped untouched on a level change), BP
-- names with spaces, no soft-object reads, nothing runs outside an MP
-- session or in a menu world.

local UEHelpers = require("UEHelpers")

-- Thread-safety shim. LoopAsync, ExecuteWithDelay and *Async keybind
-- callbacks run on UE4SS worker threads; running them alongside game-thread
-- Lua corrupted the Lua VM (symbolized crash dumps: lua_next / __index on
-- garbage userdata). Route every deferred, looped and key-bound callback onto
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
    print(string.format("[HSMPInteract] " .. fmt .. "\n", ...))
end

local function trim(s) return s and s:match("^%s*(.-)%s*$") or nil end
local STATE_DIR = (trim(os.getenv("HSMP_STATE_DIR")) or "hsmp_state"):gsub("\\", "/")
local SETTINGS  = STATE_DIR .. "/.settings.json"

local WILLIE      = "/Game/Character/Blueprints/Willie_BP.Willie_BP_C"
local BODY_HIT_FN = WILLIE .. ":BndEvt__BP_ThirdPersonCharacter_Mesh_K2Node_ComponentBoundEvent_0_ComponentHitSignature__DelegateSignature"
local GET_DAMAGE  = WILLIE .. ":Get Damage"

-- shared/*.lua and our own modules: deploy copies shared/ into Scripts/;
-- fall back to the source tree.
local function load_module(name)
    local ok, mod = pcall(require, name)
    if ok and type(mod) == "table" then return mod end
    local src = ((debug and debug.getinfo and debug.getinfo(1, "S").source) or ""):gsub("^@", "")
    local dir = src:match("^(.*)[/\\]") or "."
    for _, path in ipairs({ dir .. "/" .. name .. ".lua", dir .. "/../../shared/" .. name .. ".lua" }) do
        local ok2, mod2 = pcall(dofile, path)
        if ok2 and type(mod2) == "table" then return mod2 end
    end
    return nil
end
-- HSMP-SHM facade (shared/hsmp_ipc.lua), published as the per-state global
-- HSMP_IPC; this mod's state-dir helpers route through it.
do
    local m = load_module("hsmp_ipc")
    if m then m.init({ mod = "HSMPInteract", state_dir = STATE_DIR, log = Log }) end
end

local C = load_module("interact_core")
local HW = load_module("hsmp_wg")
if not (C and HW) then
    Log("FATAL: %s missing - HSMPInteract disabled", C and "shared/hsmp_wg.lua" or "interact_core.lua")
    return
end
local WG = HW.new({ log = Log, UEHelpers = UEHelpers })
pcall(function() RegisterLoadMapPreHook(function() pcall(WG.travel, "LoadMap") end) end)
local wg_check, wg_on_drop = WG.check, WG.on_drop

-- --- tunables (.settings.json overrides a few) ------------------------------------

local cfg = {
    enabled          = true,
    -- detection
    impulse_min      = 1500,   -- kg*cm/s summed per contact window (size above pose_contact far_p95)
    impulse_window   = 50,     -- ms
    grab_update_ms   = 100,    -- grab lease refresh
    grip_max         = 55,     -- uu: grip point clamp (server MAX_POINT 60)
    damage_dedupe    = true,   -- drop a contact that was also a damage hit (live round)
    damage_raw_min   = 5,      -- "Raw Damage" that counts as a real hit
    closing_min      = 150,    -- uu/s: bodies must close on each other along the contact normal
    contact_pelvis_max = 200,  -- uu: pelvises farther apart than this never shove
    shove_gap_ms     = 150,    -- per-pair rate cap: at most one shove sent per this long
    -- application
    impulse_gain     = 1.0,
    apply_max_imp    = 15000,  -- per event (server clamps the same)
    apply_max_dv     = 800,    -- cm/s velocity change of the struck bone per event
    grab_k           = 1500,   -- handle stiffness (soft: the victim can struggle)
    grab_d           = 150,
    grab_ang_k       = 0,      -- position-only grab
    grab_lead        = 40,     -- uu: force <= grab_k * grab_lead
    grab_break       = 130,    -- uu between grip and anchor ...
    grab_break_ms    = 400,    -- ... for this long: broke free
    grab_lease_ms    = 1500,   -- no grab_update for this long: released
    -- stand-in compliance (bus key pose_yield)
    yield_grab       = { gain = 0.15, lin = 150, ang = 150, hold = 300 },
    yield_shove      = { gain = 0.35, lin = 300, ang = 300, hold = 150 },
}

local function now_ms() return math.floor(os.clock() * 1000) end

local function refresh_settings()
    local f = io.open(SETTINGS, "rb"); if not f then return end
    local s = f:read("a") or ""; f:close()
    local v = s:match('"interact"%s*:%s*(%a+)')
    if v == "false" then cfg.enabled = false elseif v == "true" then cfg.enabled = true end
    local n = tonumber(s:match('"interact_impulse_min"%s*:%s*([%d%.]+)'))
    if n and n >= 100 and n <= 100000 then cfg.impulse_min = n end
    n = tonumber(s:match('"interact_impulse_gain"%s*:%s*([%d%.]+)'))
    if n and n >= 0 and n <= 4 then cfg.impulse_gain = n end
    n = tonumber(s:match('"interact_grab_stiffness"%s*:%s*([%d%.]+)'))
    if n and n >= 100 and n <= 20000 then cfg.grab_k = n end
end

-- --- small UE helpers ------------------------------------------------------------

local function pv(p)   -- hook param value (RemoteUnrealParam or raw value)
    if type(p) == "userdata" or type(p) == "table" then
        local ok, v = pcall(function() return p:get() end)
        if ok and v ~= nil then return v end
    end
    return p
end

local function addr(o)
    if not o then return nil end
    local a; pcall(function() a = o:GetAddress() end)
    return a
end

local function wname(o)
    local n; pcall(function() n = o:GetFName():ToString() end)
    return n
end

local function vec3(v)
    if not v then return nil end
    local t; pcall(function() t = { v.X, v.Y, v.Z } end)
    if t and C.finite3(t) then return t end
    return nil
end

local function socket(mesh, bone)
    local p, q
    pcall(function() p = mesh:GetSocketLocation(FName(bone)) end)
    pcall(function() q = mesh:GetSocketQuaternion(FName(bone)) end)
    local pp = vec3(p)
    if not pp then return nil end
    local qq
    if q then pcall(function() qq = { q.X, q.Y, q.Z, q.W } end) end
    if not (qq and C.finite(qq[1]) and C.finite(qq[4])) then qq = { 0, 0, 0, 1 } end
    return pp, qq
end

local function V(t) return { X = t[1], Y = t[2], Z = t[3] } end

-- The simulated (pose) mesh of a Willie: same rule as HSMPSync/HSMPAvatars
-- (+2 Pelvis body simulates, +1 visible).
local MESH_FIELDS = { "SK_Skeleton", "BoneCore", "DriverSkeleton", "Mesh" }
local function pose_mesh(actor)
    local best, score = nil, -1
    for _, k in ipairs(MESH_FIELDS) do
        local m; pcall(function() m = actor[k] end)
        if m and m:IsValid() then
            local sim, vis = false, false
            pcall(function() sim = m:IsSimulatingPhysics(FName("Pelvis")) end)
            pcall(function() vis = m:IsVisible() end)
            local s = (sim and 2 or 0) + (vis and 1 or 0)
            if s > score then best, score = m, s end
        end
    end
    return best
end

-- Nearest hero bone of `mesh` to world point p (grab bones the name map
-- doesn't know).
local function nearest_hero(mesh, p)
    local best, bd = nil, math.huge
    for _, b in ipairs(C.HERO) do
        local bp = socket(mesh, b)
        if bp then
            local d = C.dist(bp, p)
            if d < bd then best, bd = b, d end
        end
    end
    return best
end

-- --- session / match / roster -------------------------------------------------------

local session = { connected = false, me = 0 }
local HSESS = load_module("hsmp_session")   -- typed session / link records
local SESS = HSESS and HSESS.new({ every_s = 0 })
if not SESS then Log("WARNING: shared/hsmp_session.lua missing - private liveness rule") end
local function refresh_session()
    -- The sidecar's link record (slot `link`, typed).
    local st = HSESS and HSESS.link()
    if type(st) ~= "table" then
        -- no status: the shared tracker decides (it confirms an absence first)
        if SESS then SESS:poll(true); session.connected = SESS:live() else session.connected = false end
        return
    end
    local id = math.tointeger(st.my_peer_id) or session.me
    if id ~= 0 and session.me ~= 0 and id ~= session.me then session.new_id = true end
    session.me = id
    if SESS then
        -- The shared liveness rule (shared/hsmp_session.lua):
        -- "connected" AND a fresh sidecar heartbeat (segment header). A crashed
        -- sidecar's leftover link is never live.
        SESS:poll(true)
        session.connected = SESS:live()
        return
    end
    session.connected = false
end
local function mp_active()
    return session.connected
end

-- One typed `interact` record to the sidecar (G2S ring; the facade keeps a bounded
-- retry queue while the ring is full). Nothing before the session is up.
local function send_out(rec)
    if not session.connected then return false end
    local ipc = rawget(_G, "HSMP_IPC")
    return ipc ~= nil and ipc.send("interact", rec) ~= nil
end

local combat_live = false
local function refresh_match()
    local m = HSESS and HSESS.view()   -- the typed session snapshot
    if type(m) ~= "table" then combat_live = false; return end
    local st = type(m.state) == "string" and m.state or nil
    if st then combat_live = (st == "live" or st == "roundover") end
end

-- --- world-scoped caches (dropped untouched on a world change) -------------------------

local world_ok = false
local tick_num, frame_num = 0, 0
local puppet_peer = {}     -- stand-in FName -> peer id (strings only)
local puppet_actor = {}    -- peer id -> stand-in actor (this world)
local puppet_mesh = {}     -- peer id -> its pose mesh (this world)
local me = { pawn = nil, addr = nil, mesh = nil, pelvis = nil }
local handles = { pawn_addr = nil, free = {}, all = 0 }   -- PhysicsHandles on MY pawn
local held = {}            -- owner side: (from*2+hand) -> grab record (has handle h)
local closed = {}          -- (from*2+hand) -> grab id I broke free from / that timed out (Lua only)
local PH_CLASS = nil

local function drop_world_caches(why)
    world_ok = false
    puppet_actor, puppet_mesh = {}, {}
    me = { pawn = nil, addr = nil, mesh = nil, pelvis = nil }
    handles = { pawn_addr = nil, free = {}, all = 0 }
    PH_CLASS = nil
    if next(held) ~= nil then Log("grabs on my body dropped (%s)", tostring(why)) end
    held = {}
end
wg_on_drop(drop_world_caches, "interact caches")

local puppets_pending = false   -- a refresh skipped before the world settled
local function refresh_puppets()
    -- HSMPAvatars' stand-ins (bus key "puppets"); never written: keep the last set.
    local ipc = rawget(_G, "HSMP_IPC")
    local pt = ipc and ipc.bus_table("puppets")
    if type(pt) ~= "table" then return end
    puppet_peer, puppet_actor, puppet_mesh = {}, {}, {}
    for _, r in ipairs(pt.rows or {}) do
        if r.peer and r.peer ~= 0 and type(r.name) == "string" and r.name ~= "" then puppet_peer[r.name] = r.peer end
    end
    puppets_pending = false
    if next(puppet_peer) == nil then return end
    -- No FindAllOf over Willies (nor a name read on what it returns)
    -- in the first WG.settled() seconds of a world: the old world is still
    -- being purged and the new player Willie is mid-construction (doing so
    -- crashes the game 0.5-0.6 s after a round reload). Retried every tick
    -- until settled.
    if not WG.settled() then puppets_pending = true; return end
    pcall(function()
        local ws = FindAllOf("Willie_BP_C")
        if not ws then return end
        for _, w in pairs(ws) do
            if w and w:IsValid() then
                local id = puppet_peer[wname(w) or ""]
                if id then puppet_actor[id] = w end
            end
        end
    end)
end

local function standin_mesh(peer)
    local m = puppet_mesh[peer]
    if m then return m end
    local a = puppet_actor[peer]
    if not a then return nil end
    m = pose_mesh(a)
    puppet_mesh[peer] = m
    return m
end

local function refresh_me(tick_pc)
    local pawn
    pcall(function()
        local pc = tick_pc or UEHelpers.GetPlayerController()
        if pc and pc:IsValid() then
            local p = pc.Pawn
            if p and p:IsValid() then pawn = p end
        end
    end)
    local a = addr(pawn)
    if a ~= me.addr then
        -- New pawn in the SAME world (respawn / possession change; the
        -- world guard has validated this tick): the old pawn's grab handles
        -- let go, the grabs on the old body are over.
        if next(held) ~= nil then
            for key, r in pairs(held) do
                -- Late lease refreshes describe the old pawn's bone. Closing
                -- that id prevents them from attaching to the new life.
                closed[key] = r.id
                pcall(function() if r.h and r.h:IsValid() then r.h:ReleaseComponent() end end)
            end
            held = {}
            Log("my pawn changed: grabs on my body released")
        end
        me = { pawn = pawn, addr = a, mesh = pawn and pose_mesh(pawn) or nil, pelvis = nil }
    elseif me.mesh == nil and pawn then
        me.mesh = pose_mesh(pawn)
    end
    -- Identity sets for contact filtering (addresses only, rebuilt every
    -- tick): my pawn's skeletal meshes and the weapons it holds.
    me.mesh_addrs, me.weapon_addrs = {}, {}
    if pawn then
        for _, k in ipairs(MESH_FIELDS) do
            local m; pcall(function() m = pawn[k] end)
            local ma = m and addr(m)
            if ma then me.mesh_addrs[ma] = true end
        end
        for _, k in ipairs({ "Weapon R", "Weapon L" }) do
            local wp; pcall(function() wp = pawn[k] end)
            local wa = wp and addr(wp)
            if wa then me.weapon_addrs[wa] = true end
        end
    end
end

-- Spawn protection of my pawn (HSMPSync's spawn_status bus record,
-- docs/development/subsystems/spawns.md): protect_until on the shared os.clock
-- (seconds); unset = unbounded until the placement is verified. No shoves are
-- sent or applied inside it. HSMPSync leaves protect_until unset from
-- placement until Live (also once verified), so unset is protected whatever
-- "verified" says (as the Director and kit.lua read it). A status for another
-- pawn (an earlier world / process) protects nothing.
local spawn_protected = false
local function refresh_spawn_protection()
    local ipc = rawget(_G, "HSMP_IPC")
    local s = ipc and ipc.bus_table and ipc.bus_table("spawn_status")   -- typed bus record
    if type(s) ~= "table" or (s.seq or 0) == 0 then spawn_protected = false; return end
    local pw = s.pawn
    if pw ~= "" and me.pawn and pw ~= wname(me.pawn) then spawn_protected = false; return end
    if not s.has_protect_until then
        spawn_protected = true
        return
    end
    spawn_protected = os.clock() < s.protect_until
end

-- --- yield requests (bus key pose_yield, HSMPAvatars compliance hook) --------------------

local yield = {}
local yield_bus = { present = false, rec = nil, at = -1e9 }
local function write_yield(now)
    -- Typed bus record `pose_yield` (schema/interact.rs): one row per yielding stand-in.
    local ipc = rawget(_G, "HSMP_IPC")
    local any = C.yield_prune(yield, now)
    if not any then
        if yield_bus.present then
            if ipc then ipc.bus_put("pose_yield", { rows = {} }) end
            yield_bus.present, yield_bus.rec = false, nil
        end
        return
    end
    local rec = C.yield_rec(yield)
    -- Rewrite when it changed, else refresh at most every 100 ms.
    if C.yield_same(rec, yield_bus.rec) and now - yield_bus.at < 100 then return end
    if not (ipc and ipc.bus_put("pose_yield", rec)) then return end
    yield_bus.present, yield_bus.rec, yield_bus.at = true, rec, now
end

-- --- detection: my grabs -------------------------------------------------------------

local GRAB = {
    [0] = { grabbed = "Grabbed R", comp = "Grab Component R", bone = "Grab Bone R" },
    [1] = { grabbed = "Grabbed L", comp = "Grab Component L", bone = "Grab Bone L" },
}
local my_grabs = {}        -- hand -> { peer, gid, bone, pt, last_sent, denied }
local next_gid = 0
local stats = { grabs = 0, grab_denied = 0, impulses = 0, suppressed = 0, rate_capped = 0, protected = 0, applied = 0, capped = 0,
                held = 0, broke = 0, lease = 0 }

local function end_my_grab(hand, why, now)
    local g = my_grabs[hand]
    if not g then return end
    my_grabs[hand] = nil
    if not g.denied then send_out(C.grab_end_rec(g.peer, g.gid, hand, now)) end
    local y = yield[g.peer]
    if y then y["until"] = math.min(y["until"], now + 50) end   -- stop yielding soon
    Log("grab %s (hand %s) on peer %d ended: %s", tostring(g.gid), C.HANDS[hand], g.peer, why)
end

local function detect_grabs(now)
    local pawn = me.pawn
    if not pawn then return end
    for hand = 0, 1 do
        local n = GRAB[hand]
        local comp, peer
        pcall(function()
            if pawn[n.grabbed] == true then
                local c = pawn[n.comp]
                if c and c:IsValid() then comp = c end
            end
        end)
        if comp then
            local owner; pcall(function() owner = comp:GetOwner() end)
            peer = owner and puppet_peer[wname(owner) or ""] or nil
        end
        local g = my_grabs[hand]
        if g and g.peer ~= peer then end_my_grab(hand, peer and "switched target" or "released", now); g = nil end
        -- A spawn-protected player starts no grab (as with shoves)
        -- and lets go of one that was still held when protection began.
        if g and spawn_protected then end_my_grab(hand, "spawn protection", now); g = nil end
        if peer and not g and spawn_protected then
            stats.protected = stats.protected + 1
        elseif peer and not g then
            local hand_pos = me.mesh and socket(me.mesh, C.HAND_BONE[hand])
            if hand_pos then
                local raw; pcall(function() raw = pawn[n.bone]:ToString() end)
                local bone = C.map_bone(raw) or nearest_hero(comp, hand_pos)
                local bp, bq = socket(comp, bone or "")
                if bone and bp then
                    local pt = C.clamp_len(C.to_local(bp, bq, hand_pos), cfg.grip_max)
                    next_gid = next_gid + 1
                    g = { peer = peer, gid = next_gid, bone = bone, pt = pt, last_sent = now, denied = false }
                    my_grabs[hand] = g
                    send_out(C.grab_rec("grab_start", peer, g.gid, hand, bone, pt, hand_pos, now))
                    stats.grabs = stats.grabs + 1
                    Log("grab %d (hand %s) on peer %d: bone %s (raw %s), grip [%.0f,%.0f,%.0f]",
                        g.gid, C.HANDS[hand], peer, bone, tostring(raw), pt[1], pt[2], pt[3])
                end
            end
        elseif g and not g.denied and now - g.last_sent >= cfg.grab_update_ms then
            local hand_pos = me.mesh and socket(me.mesh, C.HAND_BONE[hand])
            if hand_pos then
                send_out(C.grab_rec("grab_update", g.peer, g.gid, hand, g.bone, g.pt, hand_pos, now))
                g.last_sent = now
            end
        end
        if g and not g.denied then
            local y = cfg.yield_grab
            C.yield_merge(yield, g.peer, now + y.hold, y.gain, y.lin, y.ang)
        end
    end
end

-- --- detection: body contacts (impulses) ----------------------------------------------------

local acc = C.new_acc({ min = cfg.impulse_min, window = cfg.impulse_window })
local damage_frame = {}    -- peer -> last frame my body dealt it a real damage hit

local contact_stats = { seen = 0, not_mine = 0, apart = 0, slow = 0, protected = 0, accepted = 0 }

local function lin_vel(comp, bone)
    local v
    pcall(function() v = vec3(comp:GetPhysicsLinearVelocity(FName(bone or "None"))) end)
    return v
end

-- A contact counts as a shove only when all of these hold (otherwise stand-in
-- feet on the floor count as shoves while the pawns are apart):
--   1. self is a stand-in and HitComponent is one of its components;
--   2. the other component is one of my pawn's skeletal meshes, or a
--      component of a weapon my pawn holds (identity by address: the
--      component itself, and its owner) - never the floor / world / props;
--   3. my pawn is not spawn-protected and the two pelvises are within
--      `contact_pelvis_max`;
--   4. the bodies close on each other along the contact normal faster than
--      `closing_min` (a resting / sliding touch is not a shove).
-- The per-pair rate cap is applied when sending (flush_impulses).
local function on_body_hit(selfp, HitComponent, OtherActor, OtherComp, NormalImpulse, Hit)
    if not (world_ok and cfg.enabled and me.addr and me.mesh_addrs) or WG.travel_from ~= nil then return end
    -- 2. identity of the other side (cheapest rejection first)
    local ocomp = pv(OtherComp)
    local oca = ocomp and addr(ocomp)
    if not oca then return end
    local oowner; pcall(function() oowner = ocomp:GetOwner() end)
    local ooa = addr(oowner)
    local mine_body = me.mesh_addrs[oca] == true and ooa == me.addr
    local mine_weapon = ooa ~= nil and me.weapon_addrs[ooa] == true
    if not (mine_body or mine_weapon) then return end
    contact_stats.seen = contact_stats.seen + 1
    -- 1. self is a stand-in, HitComponent is its own
    local w = pv(selfp)
    local peer = w and puppet_peer[wname(w) or ""]
    if not peer then contact_stats.not_mine = contact_stats.not_mine + 1; return end
    local hc = pv(HitComponent)
    local hco; pcall(function() hco = hc:GetOwner() end)
    if not hco or addr(hco) ~= addr(w) then contact_stats.not_mine = contact_stats.not_mine + 1; return end
    -- 3. spawn protection, distance
    if spawn_protected then contact_stats.protected = contact_stats.protected + 1; return end
    local sm = standin_mesh(peer)
    local spel = sm and socket(sm, "Pelvis")
    if not (me.pelvis and spel) or C.dist(me.pelvis, spel) > cfg.contact_pelvis_max then
        contact_stats.apart = contact_stats.apart + 1
        return
    end
    local imp = vec3(pv(NormalImpulse))
    if not imp or C.len(imp) < 1 then return end
    local hit = pv(Hit)
    local raw_my, raw_other, point
    pcall(function() raw_my = hit.MyBoneName:ToString() end)
    pcall(function() raw_other = hit.BoneName:ToString() end)
    pcall(function() point = vec3(hit.ImpactPoint) end)
    local bone = C.map_bone(raw_my)
    if not bone then pcall(function() bone = C.map_bone(w["Body Hit Bone Name Self"]:ToString()) end) end
    if not point then pcall(function() point = vec3(w["Body Hit Impact Point"]) end) end
    if not point then point = socket(hc, bone or "Spine_02") end
    if not point then return end
    bone = bone or "Spine_02"
    imp = C.orient_away(imp, me.pelvis, point)
    -- 4. closing velocity along the (oriented) contact normal
    local v_me = lin_vel(ocomp, mine_body and raw_other or "None")
    local v_si = lin_vel(hc, raw_my or bone)
    local n = C.scale(imp, 1 / C.len(imp))
    local closing = (v_me and v_si) and C.dot(C.sub(v_me, v_si), n) or nil
    if not closing or closing < cfg.closing_min then
        contact_stats.slow = contact_stats.slow + 1
        return
    end
    contact_stats.accepted = contact_stats.accepted + 1
    if contact_stats.accepted <= 5 then
        Log("contact with peer %d: my %s (%s) -> its %s, closing %.0f uu/s, |%.0f|", peer,
            mine_body and "body" or "weapon", tostring(raw_other), bone, closing, C.len(imp))
    end
    C.acc_add(acc, peer, imp, point, bone, now_ms(), frame_num)
end

-- A real damage hit dealt by my body OR a weapon I hold to a stand-in
-- (HSMPCombat claims it as a hit and replicates it). For a sword the
-- component's owner is the ModularWeaponBP actor, not my pawn: accept the
-- weapons my pawn holds (by address) and a weapon whose "Parent Actor" is my
-- pawn (the same resolution HSMPCombat uses), so a sword hit near the body is
-- never also sent as a shove.
local function hit_by_me(owner)
    local oa = addr(owner)
    if not oa then return false end
    if oa == me.addr then return true end
    if me.weapon_addrs and me.weapon_addrs[oa] then return true end
    local parent; pcall(function() parent = owner["Parent Actor"] end)
    return parent ~= nil and addr(parent) == me.addr
end
local function on_damage(selfp, Impulse, Velocity, Location, Normal, bone, RawDamage,
                         CuttingPower, Inside, DamagedMesh, DismBlunt, LowerThreshold,
                         Shockwave, HitByComponent)
    if not (world_ok and me.addr) then return end
    local w = pv(selfp)
    local peer = w and puppet_peer[wname(w) or ""]
    if not peer then return end
    local raw = tonumber(pv(RawDamage)) or 0
    if raw < cfg.damage_raw_min then return end
    local comp = pv(HitByComponent)
    local owner; pcall(function() owner = comp:GetOwner() end)
    if hit_by_me(owner) then damage_frame[peer] = frame_num end
end

local function damaged(peer, f0, f1)
    if not (cfg.damage_dedupe and combat_live) then return false end
    local f = damage_frame[peer]
    return f ~= nil and f >= f0 and f <= f1
end

local last_shove = {}       -- peer -> now_ms of the last shove sent (per-pair rate cap)
local function flush_impulses(now)
    acc.min, acc.window = cfg.impulse_min, cfg.impulse_window
    for _, e in ipairs(C.acc_flush(acc, now, frame_num, damaged)) do
        if e.suppressed then
            stats.suppressed = stats.suppressed + 1
        elseif now - (last_shove[e.peer] or -1e9) < cfg.shove_gap_ms then
            stats.rate_capped = stats.rate_capped + 1
        else
            local mesh = standin_mesh(e.peer)
            local bp, bq
            if mesh then bp, bq = socket(mesh, e.bone) end
            local pt = bp and C.clamp_len(C.to_local(bp, bq, e.point), cfg.grip_max) or { 0, 0, 0 }
            send_out(C.impulse_rec(e.peer, e.bone, pt, e.v, e.ts))
            last_shove[e.peer] = now
            stats.impulses = stats.impulses + 1
            local y = cfg.yield_shove
            C.yield_merge(yield, e.peer, now + y.hold, y.gain, y.lin, y.ang)
            if stats.impulses <= 10 or stats.impulses % 50 == 0 then
                Log("shove on peer %d: %s |%.0f| (#%d)", e.peer, e.bone, C.len(e.v), stats.impulses)
            end
        end
    end
end

-- --- application on my body -------------------------------------------------------------

local function ph_class()
    if PH_CLASS then return PH_CLASS end
    pcall(function() PH_CLASS = StaticFindObject("/Script/Engine.PhysicsHandleComponent") end)
    if PH_CLASS and not PH_CLASS:IsValid() then PH_CLASS = nil end
    return PH_CLASS
end

local IDENTITY_XFORM = {
    Translation = { X = 0, Y = 0, Z = 0 },
    Rotation    = { X = 0, Y = 0, Z = 0, W = 1 },
    Scale3D     = { X = 1, Y = 1, Z = 1 },
}

-- A PhysicsHandle on my pawn (pooled per pawn: components are never leaked).
local function take_handle()
    if handles.pawn_addr ~= me.addr then handles = { pawn_addr = me.addr, free = {}, all = 0 } end
    local h = table.remove(handles.free)
    if h and h:IsValid() then return h end
    local cls = ph_class()
    if not (cls and me.pawn) then return nil end
    pcall(function() h = me.pawn:AddComponentByClass(cls, true, IDENTITY_XFORM, false) end)
    if not (h and h:IsValid()) then return nil end
    pcall(function() h.bSoftLinearConstraint = true end)
    pcall(function() h.bSoftAngularConstraint = true end)
    pcall(function() h.bInterpolateTarget = false end)
    handles.all = handles.all + 1
    return h
end

local function tune(h)
    pcall(function() h:SetLinearStiffness(cfg.grab_k) end)
    pcall(function() h:SetLinearDamping(cfg.grab_d) end)
    pcall(function() h:SetAngularStiffness(cfg.grab_ang_k) end)
    pcall(function() h:SetAngularDamping(0) end)
end

local function release(key, why, close)
    local r = held[key]
    if not r then return end
    held[key] = nil
    if close then closed[key] = r.id end
    if r.h and handles.pawn_addr == me.addr and r.h:IsValid() then
        pcall(function() r.h:ReleaseComponent() end)
        table.insert(handles.free, r.h)
    end
    Log("grab by peer %d (hand %s) on my %s released: %s", r.from, C.HANDS[r.hand], r.bone, why)
end

local function apply_impulse(e)
    if spawn_protected then stats.protected = stats.protected + 1; return end
    local mesh = me.mesh
    if not mesh then return end
    local bp, bq = socket(mesh, e.bone)
    if not bp then return end
    local at = C.to_world(bp, bq, e.pt)
    local mass; pcall(function() mass = mesh:GetBoneMass(FName(e.bone), true) end)
    local v, l, capped = C.apply_impulse(e.v, cfg.impulse_gain, cfg.apply_max_imp, tonumber(mass), cfg.apply_max_dv)
    local ok = pcall(function() mesh:AddImpulseAtLocation(V(v), V(at), FName(e.bone)) end)
    if ok then stats.applied = stats.applied + 1 end
    if capped then stats.capped = stats.capped + 1 end
    if stats.applied <= 10 or stats.applied % 50 == 0 then
        Log("shoved by peer %d: %s |%.0f|%s (#%d)", e.from, e.bone, l, capped and " (capped)" or "", stats.applied)
    end
end

local function grab_on_me(e, now)
    local key = e.from * 2 + e.hand
    -- A grab I broke free from (or that timed out) stays broken: its later
    -- updates must not re-attach it. The grabber's next grab has a new id.
    if closed[key] == e.id then return end
    local r = held[key]
    -- A spawn-protected player cannot be grabbed and dragged (as
    -- shoves, apply_impulse). The grab is closed for good: protection ends
    -- at Live, the grabber's next grab has a new id.
    if spawn_protected then
        if r then release(key, "spawn protection", true) else closed[key] = e.id end
        stats.protected = stats.protected + 1
        return
    end
    if r and r.id ~= e.id then release(key, "replaced by a new grab"); r = nil end
    if r then
        r.last, r.v = now, e.v
        if r.bone == e.bone then return end
        release(key, "bone changed"); r = nil
    end
    local mesh = me.mesh
    if not mesh then return end
    local bp, bq = socket(mesh, e.bone)
    if not bp then return end
    local h = take_handle()
    if not h then
        Log("PhysicsHandle unavailable: grab by peer %d not applied", e.from)
        return
    end
    tune(h)
    local grip = C.to_world(bp, bq, e.pt)
    local ok = pcall(function() h:GrabComponentAtLocation(mesh, FName(e.bone), V(grip)) end)
    local g; pcall(function() g = h.GrabbedComponent end)
    if not ok or not (g and g:IsValid()) then
        table.insert(handles.free, h)
        Log("grab by peer %d on my %s: could not attach", e.from, e.bone)
        return
    end
    held[key] = { from = e.from, hand = e.hand, id = e.id, bone = e.bone, pt = e.pt, v = e.v,
                  last = now, h = h, far_since = nil, at = now }
    stats.held = stats.held + 1
    Log("grabbed by peer %d (hand %s) on my %s", e.from, C.HANDS[e.hand], e.bone)
end

local function about_my_grab(e, now)
    for hand = 0, 1 do
        local g = my_grabs[hand]
        if g and e.gid and g.gid == e.gid then
            g.denied = true
            local y = yield[g.peer]
            if y then y["until"] = now end
            if e.kind == "grab_denied" then stats.grab_denied = stats.grab_denied + 1 end
            Log("my grab %d on peer %d: %s by the server", g.gid, g.peer,
                e.kind == "grab_denied" and "denied" or "ended")
        end
    end
end

-- S2G `interact` records (peer = the initiator, 0 = the server about my grab).
-- Subscribing at load starts the cursor at the current end: a mod reload
-- mid-session never replays old events.
do
    local ipc = rawget(_G, "HSMP_IPC")
    if ipc then ipc.events("interact") end
end
local function read_inbox()
    local ipc = rawget(_G, "HSMP_IPC")
    local out = {}
    for _, r in ipairs((ipc and ipc.events("interact")) or {}) do
        local e = C.from_rec(r.peer, r.data)
        if e then out[#out + 1] = e end
    end
    return out
end

local function apply_inbox(now)
    for _, e in ipairs(read_inbox()) do
        if e.from == 0 then
            about_my_grab(e, now)
        elseif e.kind == "impulse" then
            apply_impulse(e)
        elseif e.kind == "grab_start" or e.kind == "grab_update" then
            grab_on_me(e, now)
        elseif e.kind == "grab_end" then
            local key = e.from * 2 + e.hand
            if held[key] and held[key].id == e.id then release(key, "grabber let go") end
        end
    end
end

-- Pull each grabbed bone of mine toward the grabber's hand as I see it.
local function drive_held(now)
    for key, r in pairs(held) do
        if now - r.last > cfg.grab_lease_ms then
            stats.lease = stats.lease + 1
            release(key, "lease expired", true)
        elseif not (r.h and r.h:IsValid()) then
            held[key] = nil
        else
            local anchor
            local sm = standin_mesh(r.from)
            if sm then anchor = socket(sm, C.HAND_BONE[r.hand]) end
            anchor = anchor or r.v
            local bp, bq = socket(me.mesh, r.bone)
            if bp and C.finite3(anchor) then
                local grip = C.to_world(bp, bq, r.pt)
                local target, d = C.grab_target(grip, anchor, cfg.grab_lead)
                if d > cfg.grab_break then
                    r.far_since = r.far_since or now
                    if now - r.far_since >= cfg.grab_break_ms then
                        stats.broke = stats.broke + 1
                        release(key, string.format("broke free (%.0f uu)", d), true)
                    end
                else
                    r.far_since = nil
                end
                if held[key] then pcall(function() r.h:SetTargetLocation(V(target)) end) end
            end
        end
    end
end

-- --- loops ---------------------------------------------------------------------------

local hooks = { body = false, damage = false, tries = 0, next_tick = 0, errors = 0 }
local function hook_error(what, err)
    hooks.errors = hooks.errors + 1
    if hooks.errors <= 5 or hooks.errors % 500 == 0 then
        Log("%s hook error (#%d): %s", what, hooks.errors, tostring(err))
    end
end
local function try_hooks()
    if hooks.body and hooks.damage then return end
    -- The Willie class loads with the first arena: retry about once a second.
    if hooks.tries >= 600 or tick_num < hooks.next_tick then return end
    hooks.tries, hooks.next_tick = hooks.tries + 1, tick_num + 30
    if not hooks.body then
        hooks.body = pcall(function()
            RegisterHook(BODY_HIT_FN, function(s, hc, oa, oc, ni, h)
                local ok, err = pcall(on_body_hit, s, hc, oa, oc, ni, h)
                if not ok then hook_error("body hit", err) end
            end)
        end)
        if hooks.body then Log("body hit hook registered (shove detection)") end
    end
    if not hooks.damage then
        hooks.damage = pcall(function()
            RegisterHook(GET_DAMAGE, function(...)
                local ok, err = pcall(on_damage, ...)
                if not ok then hook_error("Get Damage", err) end
            end)
        end)
        if hooks.damage then Log("Get Damage hook registered (shove/hit dedupe)") end
    end
end

local function in_gameplay()
    return WG.name ~= nil and WG.name:find("Map_Menu_") == nil
end

local function end_all_my_grabs(why, now)
    for hand = 0, 1 do end_my_grab(hand, why, now) end
end

local function on_tick()
    tick_num = tick_num + 1
    do local ipc = rawget(_G, "HSMP_IPC"); if ipc then ipc.flush() end end   -- G2S retry queue (ring full)
    if tick_num % 30 == 1 then refresh_settings(); refresh_session() end
    -- A new session (new peer id: reconnect, server restart) reuses
    -- grab ids: the broken-free / timed-out list of the old one must go.
    if session.new_id then session.new_id = nil; closed = {} end
    if tick_num % 15 == 1 then refresh_match() end
    if tick_num % 3 == 1 then refresh_spawn_protection() end
    local now = now_ms()
    if not (cfg.enabled and mp_active()) then
        if world_ok or next(my_grabs) ~= nil then end_all_my_grabs("no MP session", now) end
        world_ok = false
        -- A grab on my body must not keep pulling my bone through a
        -- reconnect. The guard runs first: on a world change its drop handler
        -- forgets the handles untouched; in the same world they let go.
        if next(held) ~= nil then
            if wg_check() then
                for k in pairs(held) do release(k, "MP session down", true) end
            end
            held = {}
        end
        read_inbox()
        write_yield(now)
        return
    end
    -- One PlayerController lookup per tick, shared by the guard and refresh_me
    local tick_pc
    pcall(function() tick_pc = UEHelpers.GetPlayerController() end)
    local wg_ok = wg_check(tick_pc)
    if not wg_ok or not in_gameplay() then
        world_ok = false
        if next(my_grabs) ~= nil then end_all_my_grabs("world change", now) end
        -- A valid world that is not gameplay (menu): let go too.
        if wg_ok and next(held) ~= nil then
            for k in pairs(held) do release(k, "left gameplay", true) end
        end
        read_inbox()   -- events for a body that is not in play are stale: skip them
        write_yield(now)
        return
    end
    try_hooks()
    if not world_ok or puppets_pending or tick_num % 15 == 0 then refresh_puppets() end
    refresh_me(tick_pc)
    world_ok = me.pawn ~= nil
    write_yield(now)
    if tick_num % 300 == 0 and (stats.grabs + stats.impulses + stats.applied + stats.held) > 0 then
        Log("stats: my grabs %d (denied %d), shoves sent %d (dropped as hits %d, rate-capped %d), contacts mine %d (apart %d, slow %d, protected %d, foreign self %d), shoves applied %d (capped %d), grabs on me %d (broke free %d, lease %d)",
            stats.grabs, stats.grab_denied, stats.impulses, stats.suppressed, stats.rate_capped,
            contact_stats.seen, contact_stats.apart, contact_stats.slow, contact_stats.protected, contact_stats.not_mine,
            stats.applied, stats.capped,
            stats.held, stats.broke, stats.lease)
    end
end

local function on_frame()
    if not world_ok or WG.travel_from ~= nil then return end
    frame_num = frame_num + 1
    local now = now_ms()
    if me.mesh then
        local p = socket(me.mesh, "Pelvis")
        me.pelvis = p
    end
    detect_grabs(now)
    flush_impulses(now)
    apply_inbox(now)
    drive_held(now)
end

local loop_errors = 0
local function guarded(what, fn)
    return function()
        local ok, err = pcall(fn)
        if not ok then
            loop_errors = loop_errors + 1
            if loop_errors <= 5 or loop_errors % 500 == 0 then
                Log("%s error (#%d): %s", what, loop_errors, tostring(err))
            end
        end
        return false
    end
end

refresh_settings()
LoopAsync(33, guarded("tick", on_tick))
local frame_fn = guarded("frame", on_frame)
if LoopInGameThreadAfterFrames and EngineTickAvailable ~= false then
    LoopInGameThreadAfterFrames(1, function() frame_fn() end)
    Log("loaded: per-frame detection/application (state dir %s)", STATE_DIR)
else
    LoopAsync(8, frame_fn)
    Log("loaded: 8 ms game-thread loop (LoopInGameThreadAfterFrames unavailable; state dir %s)", STATE_DIR)
end
