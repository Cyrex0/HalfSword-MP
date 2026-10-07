-- HSMPSync: the local player's outbound state.
--
-- Two loops, both on the game thread:
--   * fast_send (every game frame): pumps the shared-memory IPC once per frame
--     and, at send_hz, samples root + held weapon + full pose in one frame
--     with one sender timestamp and hands them to HSMPNative (local_root /
--     local_weapon / local_pose records). The sidecar puts them on the wire.
--   * on_tick (~30 Hz): world guard, gameplay-world gate, spawn placement
--     (spawn_place.lua), dev commands, MP session gate, settings reload and
--     death detection (death_report).
-- Nothing is streamed unless an MP session is live (shared/hsmp_session.lua).
-- Keys: F4 toggles verbose logging, F7 writes a state dump to the state dir.

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

local verbose = false
local function Log(fmt, ...)
    print(string.format("[HSMPSync] " .. fmt .. "\n", ...))
end
local function verbose_fail(msg)
    if verbose then Log("fail: %s", msg) end
end

-- --- world guard (shared/hsmp_wg.lua) ------------------------------------------
-- A level change frees every actor, component and world-outered widget of the
-- old world, and the round reset re-opens the SAME arena (OpenLevel). Touching
-- a freed UObject afterwards (a property write, a call, even IsValid(), which
-- reads the freed object) is an access violation pcall cannot catch (it shows
-- up as UStruct::FindProperty <- __newindex from a delayed callback).
-- Rule for every UObject kept beyond one callback (see shared/hsmp_wg.lua):
--   * wg_check() runs first in every loop/frame callback: if the world key
--     changed, a level change is pending, or no world is valid, every cache is
--     dropped WITHOUT touching the old objects (wg_on_drop handlers only reset
--     Lua references) and the callback skips this tick;
--   * delayed one-shots capture wg_token() and test wg_same(token) first.
-- The shared guard installs the OpenLevel / OpenLevelBySoftObjectPtr
-- pre-hooks. WG.pc() is the one PlayerController lookup per game frame
-- (UEHelpers.GetPlayerController / GetWorld walk the whole object array);
-- every per-tick / per-frame path here takes its PlayerController from it.
-- shared/*.lua is copied into Scripts/ at deploy; fall back to the source tree.
local function load_shared(name)
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

local HW = load_shared("hsmp_wg")
local WEAPON_BOUNDS = load_shared("weapon_bounds")
local BODY_STRIKERS = load_shared("body_strikers")
-- Read-only: the pose writers copy it, nothing ever adds to it.
local NO_STRIKERS = {}
local POSE_CONTEXT = load_shared("pose_context")
if not HW then
    Log("FATAL: shared/hsmp_wg.lua missing - HSMPSync disabled (deploy copies shared/*.lua)")
    return
end
local WG = HW.new({ log = Log, UEHelpers = UEHelpers })
local wg_check, wg_on_drop = WG.check, WG.on_drop

-- --- paths -----------------------------------------------------------------

-- Instance id for logs. Seeded from os.time + os.clock plus a random
-- component; two instances would have to start in the same millisecond to
-- collide.
math.randomseed(os.time() * 1000 + math.floor((os.clock() or 0) * 1e6))
local PID = (os.time() % 100000) * 10000 + math.random(0, 9999)

-- State dir: HSMP_STATE_DIR env var if set (used by the launcher so each game
-- instance can have its own dir on the same PC), otherwise a shared default.
local function trim(s) return s and s:match("^%s*(.-)%s*$") or nil end
local env_dir = trim(os.getenv("HSMP_STATE_DIR"))
local STATE_DIR = (env_dir and env_dir ~= "" and env_dir) or "hsmp_state"
local SETTINGS_FILE = STATE_DIR .. "/.settings.json"

os.execute("mkdir \"" .. STATE_DIR:gsub("/", "\\") .. "\" 2>nul")

-- Shared-memory IPC facade (shared/hsmp_ipc.lua, docs/development/ipc-shared-memory.md).
-- HSMPSync owns the one per-frame pump (IPC.frame) and writes root, weapon
-- and pose through it.
local IPC = load_shared("hsmp_ipc")
if IPC then IPC.init({ mod = "HSMPSync", state_dir = STATE_DIR, log = Log }) end

-- --- settings (live-reloaded) ----------------------------------------------
--
-- The menu writes .settings.json atomically. It is re-read every ~1 s
-- during a live session:
--   * send_hz        snapshot rate (root + weapon + pose); 10..120 Hz
--   * hud            HUD on / off
--   * native_sample  native pose sampling on / off
--
-- A missing or malformed file keeps the current values.

-- Root/weapon send rate. 60 Hz default; 120 max (LAN/strong links). Higher is
-- pointless: the game only samples once per rendered frame.
local send_hz = 60
local hud_on  = true
-- Native sampling A/B: "native_sample": true / false (nil = not in the file; NSAMPLE applies it)
local native_sample_setting = nil

local function refresh_settings()
    local f = io.open(SETTINGS_FILE, "rb")
    if not f then return end
    local line = f:read("l"); f:close()
    if not line then return end
    local hz = tonumber(line:match('"send_hz"%s*:%s*(%-?%d+)'))
    if hz and hz >= 10 and hz <= 120 then send_hz = hz end
    -- the menu writes "hud" (docs/development/subsystems/menu-ui.md); "hud_on" is the old key
    local hud = line:match('"hud"%s*:%s*(%w+)') or line:match('"hud_on"%s*:%s*(%w+)')
    if hud == "true" then hud_on = true
    elseif hud == "false" then hud_on = false
    end
    local ns = line:match('"native_sample"%s*:%s*(%w+)')
    if ns == "true" then native_sample_setting = true elseif ns == "false" then native_sample_setting = false end
end

refresh_settings()

-- --- state readers ---------------------------------------------------------

-- Reflected reads for the per-frame paths, closure-free: pcall(f, obj, ...) with
-- these module-level functions instead of a new closure per read.
local function r_index(o, k) return o[k] end
local function r_addr(o) return o:GetAddress() end
local function r_fname(o) return o:GetFName():ToString() end
local function r_clsname(o) return o:GetClass():GetFName():ToString() end
local function pget(o, k)   -- o[k], or nil when the read raises
    local ok, v = pcall(r_index, o, k)
    if ok then return v end
    return nil
end
local function pval(f, ...)   -- f(...)'s first result, or nil when it raises
    local ok, v = pcall(f, ...)
    if ok then return v end
    return nil
end
local WEAPON_SIDES = { "R", "L" }
local WEAPON_FIELD = { R = "Weapon R", L = "Weapon L" }

-- Pawn cache: identity-checked. Reflection on a pawn pointer that just got
-- re-possessed (round reset / respawn) can crash; fewer property reads per
-- tick also narrow the GC-race window.
local _pawn_cache = nil
local _mesh_cache = nil

local function get_local_pawn(pc)
    pc = pc or WG.pc()
    if not pc or not pc:IsValid() then return nil end
    local pawn = pc.Pawn
    if not pawn or not pawn:IsValid() then
        pawn = WG.ai_pawn()   -- the game's own AI drives our pawn (dev, HSMPParity `ai on`)
        if not pawn then return nil end
    end
    if pawn ~= _pawn_cache then
        _pawn_cache = pawn
        _mesh_cache = nil   -- re-resolve on identity change
    end
    return pawn
end

-- Prefer SK_Skeleton (physics ragdoll driver on Willie_BP). Fallbacks are
-- BoneCore then the inherited ACharacter Mesh slot.
local function get_pawn_mesh(pawn)
    if _mesh_cache and _mesh_cache:IsValid() then return _mesh_cache end
    if not pawn or not pawn:IsValid() then return nil end
    local m
    pcall(function() m = pawn.SK_Skeleton end)
    if m and m:IsValid() then _mesh_cache = m; return m end
    pcall(function() m = pawn.BoneCore end)
    if m and m:IsValid() then _mesh_cache = m; return m end
    pcall(function() m = pawn.Mesh end)
    if m and m:IsValid() then _mesh_cache = m; return m end
    return nil
end

-- --- weapon detection ------------------------------------------------------
--
-- Willie_BP_C exposes WeaponR and WeaponL as direct AModularWeaponBP_C
-- pointers, read directly: no GetAttachedActors (out-param marshaling bug in
-- UE4SS 3.0.1), no FindAllOf("Actor") (a 30 Hz full-world enum). Leaf class
-- names all begin with "ModularWeaponBP_"; a plain "Sword" substring match
-- would also hit BP_HalfSwordGameMode_C.
local WEAPON_IDS = {
    ArmingSword = 1, Longsword = 1,
    Mace        = 2, Hammer    = 6,
    Dagger      = 3,
    Axe         = 4, Hafted    = 4,
    Spear       = 5,
}

local function classify_weapon(cls_name)
    if not cls_name then return nil end
    local leaf = cls_name:match("^ModularWeaponBP_(.-)_C$")
    if not leaf then return nil end
    return WEAPON_IDS[leaf] or 99
end

-- Returns weapon actor, id, class name, hand ("R"/"L").
local function find_held_weapon(pawn)
    if not pawn or not pawn:IsValid() then return nil end
    -- Real reflected names have spaces ("Weapon R"); .WeaponR is nil.
    local w, hand = nil, "R"
    w = pget(pawn, "Weapon R")
    if not w or not w:IsValid() then
        w, hand = nil, "L"
        w = pget(pawn, "Weapon L")
    end
    if not w or not w:IsValid() then return nil end
    local cls_ok, cls = pcall(r_clsname, w)
    if not cls_ok or not cls then return nil end
    local id = classify_weapon(cls)
    if not id then return nil end
    return w, id, cls, hand
end

local logged_weapon_cls = nil

-- --- death detection -------------------------------------------------------
--
-- The local pawn is polled each tick for Health <= 0 or native DED. When that first flips
-- true a `death_report` record is sent (tagged with the round it happened in).
-- The latch resets when the pawn is alive again, so the next round registers
-- a fresh death.

local already_dead = false
local _protect_death_logged = false

local function read_ded(pawn) return pawn.DED == true end
local function read_health(pawn) return tonumber(pawn.Health) end
local function detect_dead(pawn)
    if not pawn or not pawn:IsValid() then return false end
    -- Native Death is the sole producer of Willie.DED=true. Structural
    -- death can retain positive Health; Dying/Downed/Con0 also cover
    -- recoverable outcomes and must not be interpreted as biological death.
    local okd, ded = pcall(read_ded, pawn)
    if okd and ded then return true end
    local okh, hp = pcall(read_health, pawn)
    return okh and hp ~= nil and hp <= 0
end

-- The death report can be lost on the way to the server, which ignores
-- repeats (already-dead peers), so it is re-sent while the pawn stays dead.
-- GameStatus DEAD is diagnostic only; these original-context reports retry.
local _died_resend_at = 0
local DIED_RESEND_S = 1.0

-- Server-authoritative match facts (the session record, shared/hsmp_session.lua
-- HS.view()). A death only counts inside a LIVE round, and it is
-- round-tagged so a late/duplicate report can never kill us in a later round.
-- SP.match_reader (spawn_place.lua, set below) keeps the last complete value
-- on a torn / empty read; this fallback is used only if it is missing.
-- shared/hsmp_session.lua: the session record (typed), the link, the liveness rule.
local HS = load_shared("hsmp_session")
local match_get = nil
local function current_match()
    if match_get then return match_get() end
    local v = HS and HS.view()   -- the session record (typed)
    if type(v) ~= "table" then return nil, 0 end
    return v.state, math.tointeger(tonumber(v.round)) or 0
end

local my_peer_status, refresh_my_peer_id   -- forward: defined in "peer id" below
-- The round a death was detected in. Resends report THAT round only:
-- a pawn that stays dead into a same-world round change must not report
-- died:<new round>. Reset on a world drop (wg_on_drop below).
local _died_round = nil
local _died_context, _died_pawn = nil, nil
local pose_context -- forward: source placement helper, defined after the peer reader
local function send_died(why, latched)
    local st, round = current_match()
    if st ~= "live" or round <= 0 then return false end
    if latched ~= nil and round ~= latched then return false end
    local pawn=get_local_pawn()
    local original=_died_context
    local current=pawn and pose_context and pose_context(pawn)
    if not original or not current or pval(r_fname,pawn)~=_died_pawn
        or current.match_id~=original.match_id or current.round~=original.round or current.life~=original.life then return false end
    -- Nothing is sent before the session is up.
    if refresh_my_peer_id then pcall(refresh_my_peer_id) end
    if my_peer_status ~= nil and my_peer_status ~= "connected" then return false end
    -- The typed G2S `death_report` record (schema/combat.rs DeathReport; the
    -- sidecar fills death_id and resends it until the server's death_ack).
    if not (IPC and IPC.send("death_report", { match_id=original.match_id, round=original.round, life=original.life })) then return false end
    if why then Log("%s; emitted death_report round %d", why, round) end
    return true
end

-- Spawn protection (spawn_place.lua, set below): a pawn that "dies" between
-- its spawn and placement + protect_ms never reports a death.
local is_spawn_protected = function() return false end

local function check_death(pc)
    if IPC then IPC.flush() end   -- retry typed sends (death_report) a full ring queued
    local pawn = get_local_pawn(pc)
    -- No pawn (level transition / unpossessed corpse): neither dead nor alive.
    if not pawn then return end
    local dead = detect_dead(pawn)
    if not dead then _protect_death_logged = false end
    if dead and not already_dead and is_spawn_protected() then
        if not _protect_death_logged then
            _protect_death_logged = true
            Log("death detected during spawn protection; not reported (the spawn placer restores the vitals)")
        end
        return
    end
    if dead and not already_dead then
        already_dead = true
        _died_resend_at = os.clock() + DIED_RESEND_S
        local st, round = current_match()
        _died_context = st=="live" and pose_context and pose_context(pawn) or nil
        _died_pawn = pval(r_fname,pawn)
        _died_round = _died_context and _died_context.round or false   -- never acquire a later life
        if not send_died("death detected", _died_round or -1) then
            Log("death detected outside a live round; not reported (server decides rounds)")
        end
    elseif dead and already_dead and _died_round and os.clock() >= _died_resend_at then
        _died_resend_at = os.clock() + DIED_RESEND_S
        send_died(nil, _died_round)   -- idempotent resend while dead in THAT live round
    elseif not dead and already_dead then
        already_dead = false
        Log("re-alive; resetting death latch")
    end
end

-- Exposed global so another mod or a game event hook can trigger a death
-- notification without duplicating the latching logic.
function hs_mp_player_died()
    if already_dead then return end
    already_dead = true
    local st, round = current_match()
    local pawn=get_local_pawn()
    _died_context = st=="live" and pawn and pose_context and pose_context(pawn) or nil
    _died_pawn = pawn and pval(r_fname,pawn) or nil
    _died_round = _died_context and _died_context.round or false
    send_died("hs_mp_player_died() invoked externally", _died_round or -1)
end

-- --- peer id ------------------------------------------------------------------
--
-- Our own peer_id comes from the sidecar's `link` record (the sidecar is the
-- source of truth; it rotates on every re-join). Spawn placement is
-- server-commanded through the session record's spawn orders (see "Spawn
-- placement" below).

local my_peer_id = 0
my_peer_status = nil
local my_session_live = false   -- SP.peer_reader's liveness predicate

-- The server never reuses peer ids and the sidecar writes the new one on
-- every Welcome (reconnect, server restart), so the id is re-read on every
-- use (0.5 s throttle), never latched. A link record without a positive peer id
-- (connecting, not welcomed yet) means "no id" (0), so a stale id is never
-- used to look up this session's spawn orders. Defined once spawn_place.lua
-- is loaded (SP.peer_reader, below); until then the id is unknown.
refresh_my_peer_id = function() end
local function get_my_peer_id()
    pcall(refresh_my_peer_id)
    return my_peer_id
end


-- --- pose replication (sender side), codec v2 ----------------------------------
--
-- Willie is an active ragdoll: the visible pose is the simulated bodies of
-- `Mesh` (CharacterMesh0, SK_Body_Man). Head, gore mesh and every armour
-- piece copy their pose from it (ABP_CopyPose), so the 22 bodies ARE the
-- visible character. Once per sample (send_hz, default 60, max 120) we hand
-- the FULL physical state to HSMPNative `put_pose`
-- (docs/development/subsystems/replication.md):
--   tick, ts (sender ms, sub-ms), dt (physics step),
--   b = 23 bones (POSE_BONES order) x px,py,pz,qx,qy,qz,qw,vx,vy,vz,wx,wy,wz,
--   w = per weapon: hands,id,px,py,pz,qx,qy,qz,qw,vx,vy,vz,wx,wy,wz,base xyz,tip xyz,
--   c = the 37 control numbers (every 2nd sample).
-- World-space bone transforms; linear velocity AT the bone origin (uu/s),
-- angular velocity (deg/s); weapon blade base/tip as world points. The native
-- module encodes it ONCE into the codec v2 `pose` record (hsmp_pose::sample).
-- Root, weapon and pose of one sample share one sender time. `ts` is the
-- frame's world real time anchored to the os.clock base (os.clock alone is
-- 1 ms-quantised: 1.5 uu of error on a 1500 uu/s hand).
-- Names must match BONES / CONTROL_* in crates/hsmp-pose/src/posecodec_v2.rs.
local POSE_BONES = {
    "pelvis", "spine_01", "spine_02", "spine_03", "spine_04", "spine_05",
    "neck_01", "neck_02", "head",
    "clavicle_l", "upperarm_l", "lowerarm_l", "hand_l",
    "clavicle_r", "upperarm_r", "lowerarm_r", "hand_r",
    "thigh_l", "calf_l", "foot_l",
    "thigh_r", "calf_r", "foot_r",
}
local POSE_NOBODY = { spine_01 = true }
local CONTROL_FLAGS = {
    "R_Guarding", "L_Guarding", "Any_Guarding", "Parrying R", "Parrying L",
    "R_Thrusting", "L_Thrusting", "Alt Thrusting", "Kicking  R", "Kicking  L",
    "Fallen", "Downed", "R Kneel", "L Kneel", "Is Crouched", "Dodging",
    "R Two Handed Grip", "R Alt Grip", "Mordhau Grip", "Master Stroke Grip",
    "R Hand Reverse Grip", "L Hand Reverse Grip", "Pain Shock", "Being Grabbed",
    "Grabbed R", "Grabbed L", "Threatening Stance", "R Down", "L Down",
    "Classic Half Swording Toggle", "L Hand In Offhand Attached", "R Kneel Falling",
}
local CONTROL_SCALARS = {
    "All Body Tonus", "Upper Body Tonus", "Arm R Tonus", "Arm L Tonus", "Leg R Tonus", "Leg L Tonus",
    "Head Tonus", "Muscle Power", "Constraint Rate", "R Thrust Alpha", "Fallen Rate", "Get Up Rate",
    "Root Linear Constraint Power", "Root Angular Constraint Power", "Consciousness", "Stamina",
}
local CONTROL_IK = { "R Out End Pos", "L Out End Pos", "R Out Joint Pos", "L Out Joint Pos" }

-- Mesh fields kept for the legacy pick rule (diagnostics only).
local MESH_FIELDS = { "SK_Skeleton", "BoneCore", "DriverSkeleton", "Mesh" }
local _skel = { pawn = nil, mesh = nil, bones = nil }   -- dropped by the world guard
local _skel_seq = 0
local _pose_fn = {}
for i, bn in ipairs(POSE_BONES) do _pose_fn[i] = FName(bn) end
local FN_NONE = FName("None")

-- The simulated, visible body mesh of `pawn` (pawn.Mesh = CharacterMesh0).
-- A pawn without the codec-v2 bones (not a Willie) is re-probed at most
-- every BAD_MESH_RETRY_S and logged once per pawn, instead of 23 bone
-- lookups and one log line per sample.
local BAD_MESH_RETRY_S = 2.0
local function pose_mesh_for(pawn)
    local key = pval(r_fname, pawn)
    if key and _skel.pawn == key and _skel.mesh and _skel.mesh:IsValid() then return _skel.mesh end
    if key and _skel.bad == key and os.clock() - (_skel.bad_at or 0) < BAD_MESH_RETRY_S then return nil end
    _skel.pawn, _skel.mesh, _skel.bones = key, nil, nil
    local m; pcall(function() m = pawn.Mesh end)
    if not (m and m:IsValid()) then return nil end
    local sim, nb = false, 0
    pcall(function() sim = m:IsSimulatingPhysics(_pose_fn[1]) end)
    for i = 1, #POSE_BONES do
        local idx = -1
        pcall(function() idx = m:GetBoneIndex(_pose_fn[i]) end)
        if idx and idx >= 0 then nb = nb + 1 end
    end
    if nb < #POSE_BONES then
        if _skel.bad_logged ~= key then
            _skel.bad_logged = key
            Log("pose sender: Mesh of %s has only %d/%d codec-v2 bones; not sampling (re-checked every %.0f s, logged once)",
                tostring(key), nb, #POSE_BONES, BAD_MESH_RETRY_S)
        end
        _skel.bad, _skel.bad_at = key, os.clock()
        return nil
    end
    _skel.bad, _skel.bad_logged = nil, nil
    _skel.mesh = m
    -- Keep the sampled pose live when this window is minimized / not rendered
    -- (bones refresh regardless of visibility). The background tick
    -- (t.IdleWhenNotForeground) is set by mp_background() in MP only.
    pcall(function() m.VisibilityBasedAnimTickOption = 0 end)
    Log("pose sender: sampling Mesh (CharacterMesh0) codec v2, %d bones (%d simulated bodies), pelvis sim=%s",
        nb, nb - 1, tostring(sim))
    return m
end

-- Sender clock: os.clock()'s ms base, sub-ms from the world's real time
-- (one value per game frame = the instant this frame's physics stands for).
local _clk_off = nil
local function r_realtime(ctx) return UEHelpers.GetGameplayStatics():GetRealTimeSeconds(ctx) end
local function r_delta(ctx) return UEHelpers.GetGameplayStatics():GetWorldDeltaSeconds(ctx) end
local function precise_ms(ctx)
    local now = os.clock() * 1000
    local rt = pval(r_realtime, ctx)
    if type(rt) ~= "number" then return now end
    local v = rt * 1000
    local d = now - v
    -- Frame start is at most one frame before `now`: keep the smallest
    -- offset; re-anchor when the world's clock restarts (level change) or
    -- runs away (> 100 ms).
    if _clk_off == nil or d < _clk_off or d - _clk_off > 100 then _clk_off = d end
    return v + _clk_off
end
_G.hsmp_pose_clock_ms = precise_ms   -- same-state helper (probe)

local function finite(x) return type(x) == "number" and x == x and x > -1e9 and x < 1e9 end

-- One bone: world transform, origin velocity, angular velocity, into a table reused
-- for every bone (the caller copies it out at once).
local _sb = {}
local function sample_bone(mesh, i)
    local fn = _pose_fn[i]
    local t = mesh:GetSocketTransform(fn, 0)
    local p, q = t.Translation, t.Rotation
    local r = _sb
    r[1], r[2], r[3], r[4], r[5], r[6], r[7] = p.X, p.Y, p.Z, q.X, q.Y, q.Z, q.W
    r[8], r[9], r[10], r[11], r[12], r[13] = 0, 0, 0, 0, 0, 0
    if not POSE_NOBODY[POSE_BONES[i]] then
        local v = mesh:GetPhysicsLinearVelocityAtPoint(p, fn)
        local w = mesh:GetPhysicsAngularVelocityInDegrees(fn)
        r[8], r[9], r[10], r[11], r[12], r[13] = v.X, v.Y, v.Z, w.X, w.Y, w.Z
    end
    return r
end

-- The weapon's components, read afresh from the weapon actor of THIS sample.
-- Never cache them by actor address: a kit re-arm, a disarm or an HSMPWorld
-- pickup destroys the weapon mid-world and a new one can land at the reused
-- address, so a cache would run IsValid / K2_GetComponentLocation on freed
-- components. Three property reads per sample; nothing kept.
local function weapon_parts(w)
    local c = {}
    pcall(function() c.root = w.RootComponent end)
    -- Blade line: base = the grip origin (actor/root), tip = "TippyTipScene"
    -- (measured in-game; the "Trace Length Top/Bottom" scenes are +/-1000 uu
    -- trace ends, not blade points).
    pcall(function() c.base = w["Root Scene"] end)
    pcall(function() c.tip = w.TippyTipScene end)
    return c
end

-- One-byte class tag (1..255) both ends compute from the weapon class name:
-- a stand-in only drives its own weapon when it holds the same class.
local function class_tag(name)
    local h = 5381
    for i = 1, #(name or "") do h = (h * 33 + name:byte(i)) % 4294967296 end
    return h % 255 + 1
end

-- One held weapon as 21 numbers written to out[at+1 .. at+21]: hands, class
-- id, the 13 transform/velocity floats, blade base xyz, blade tip xyz (the
-- put_pose `w` layout). Returns true when the weapon is in hand.
local ZERO3 = { X = 0, Y = 0, Z = 0 }   -- read only
local function r_simulating(c) return c:IsSimulatingPhysics(FN_NONE) end
local function r_linvel_at(c, p) return c:GetPhysicsLinearVelocityAtPoint(p, FN_NONE) end
local function r_angvel(c) return c:GetPhysicsAngularVelocityInDegrees(FN_NONE) end
local function r_comp_loc(c, dflt)   -- a valid component's location, else dflt
    if c and c:IsValid() then return c:K2_GetComponentLocation() end
    return dflt
end
local function sample_weapon_vals(pawn, w, hands, mesh, out, at)
    -- Bare hands ("Weapon_Fists_C") are not a weapon: the hands themselves are replicated.
    local cls0 = pval(r_clsname, w)
    if cls0 and cls0:find("Fists", 1, true) then return false end
    local root, base, tip = pget(w, "RootComponent"), pget(w, "Root Scene"), pget(w, "TippyTipScene")
    if not (root and root:IsValid()) then return false end
    -- Only a weapon really in the hand: simulating and next to the hand bone
    -- (an idle "Weapon_Fists_C" actor sits far away, not simulating).
    local oks, sim = pcall(r_simulating, root)
    if not (oks and sim) then return false end
    local t = w:GetTransform()
    local p, q = t.Translation, t.Rotation
    local hb = mesh:GetSocketLocation(FName((hands == 2) and "hand_l" or "hand_r"))
    if math.sqrt((hb.X - p.X) ^ 2 + (hb.Y - p.Y) ^ 2 + (hb.Z - p.Z) ^ 2) > 150 then return false end
    local v, av = ZERO3, ZERO3
    local ok, x = pcall(r_linvel_at, root, p)
    if ok then v = x end
    ok, x = pcall(r_angvel, root)
    if ok then av = x end
    local b, tp = p, p
    ok, x = pcall(r_comp_loc, base, p)
    if ok then b = x end
    ok, x = pcall(r_comp_loc, tip, p)
    if ok then tp = x end
    local cls = pval(r_clsname, w)
    out[at + 1], out[at + 2] = hands, class_tag(cls)
    out[at + 3], out[at + 4], out[at + 5] = p.X, p.Y, p.Z
    out[at + 6], out[at + 7], out[at + 8], out[at + 9] = q.X, q.Y, q.Z, q.W
    out[at + 10], out[at + 11], out[at + 12] = v.X, v.Y, v.Z
    out[at + 13], out[at + 14], out[at + 15] = av.X, av.Y, av.Z
    out[at + 16], out[at + 17], out[at + 18] = b.X, b.Y, b.Z
    out[at + 19], out[at + 20], out[at + 21] = tp.X, tp.Y, tp.Z
    return true
end

local _wv = {}

local function vec3(v) if v and finite(v.X) then return v.X, v.Y, v.Z end return 0, 0, 0 end

-- The control layer as 37 numbers in c[1..37] (put_pose `c`):
-- flags, grip_r, grip_l, 16 scalars, aim xyz, control pitch, yaw, 4 x ik xyz,
-- ik-world bits (0: the sidecar classifies them).
local function sample_control_vals(pawn, c)
    local flags, bit = 0, 1
    for _, k in ipairs(CONTROL_FLAGS) do
        if pawn[k] == true then flags = flags + bit end
        bit = bit * 2
    end
    c[1], c[2], c[3] = flags, tonumber(pawn["R_GripType_Current"]) or 0, tonumber(pawn["L_GripType_Current"]) or 0
    for i, k in ipairs(CONTROL_SCALARS) do
        local v = tonumber(pawn[k]) or 0
        c[3 + i] = finite(v) and v or 0
    end
    c[20], c[21], c[22] = vec3(pawn["Aim Vector"])
    local cp, cy = 0, 0
    pcall(function() local r = pawn["Current Control Rotation"]; cp, cy = r.Pitch, r.Yaw end)
    c[23], c[24] = finite(cp) and cp or 0, finite(cy) and cy or 0
    for i, k in ipairs(CONTROL_IK) do
        local b = 22 + 3 * i
        c[b], c[b + 1], c[b + 2] = vec3(pawn[k])
    end
    c[37] = 0
    return c
end


-- --- native sampling (cap NATIVE_SAMPLE) ---------------------------------------
-- HSMPNative.sample_local reads the bones / weapons / control / root / weapon actor
-- through ProcessEvent and writes the same records as the Lua path below (one
-- native call instead of ~70 reflected calls). This file keeps the policy: when
-- to sample, which pawn / mesh / weapons. Any refusal falls back to the Lua path.
-- A/B (default ON): settings "native_sample" (true / false), env HSMP_NATIVE_SAMPLE=1/0
-- (overrides), dev tune key "native_sample" (MP.native_sample_tune(v)).
-- Measured in game: about 85 % less pose sender cost than the Lua path, same quality.
local NSAMPLE = { want = true, env = os.getenv("HSMP_NATIVE_SAMPLE"), configured = false, retry_at = 0,
                  used = 0, fell_back = 0, nw = 0, logged = nil,
                  a = {} }
if NSAMPLE.env == "0" then NSAMPLE.want = false end
function NSAMPLE.set(on, why)
    on = on and true or false
    if NSAMPLE.env == "0" or NSAMPLE.env == "1" then on = (NSAMPLE.env == "1") end
    if on ~= NSAMPLE.want then
        NSAMPLE.want = on
        NSAMPLE.retry_at = 0
        Log("native sampling %s (%s)", on and "ON" or "off", tostring(why))
    end
end
function NSAMPLE.on()
    if native_sample_setting ~= nil and native_sample_setting ~= NSAMPLE.setting_seen then
        NSAMPLE.setting_seen = native_sample_setting
        NSAMPLE.set(native_sample_setting, "settings")
    end
    if not NSAMPLE.want or not IPC or not IPC.is_shm() or not IPC.sample_local then return false end
    if os.clock() < NSAMPLE.retry_at then return false end
    IPC.refresh_info(false)
    local cap = (IPC.S and IPC.S.CAPS and IPC.S.CAPS.NATIVE_SAMPLE) or 0
    if ((IPC.caps_game or 0) & cap) == 0 then return false end
    if not NSAMPLE.configured then
        local ok, err = IPC.sample_config({
            bones = POSE_BONES, nobody = POSE_NOBODY, flags = CONTROL_FLAGS, scalars = CONTROL_SCALARS, ik = CONTROL_IK,
            grip_r = "R_GripType_Current", grip_l = "L_GripType_Current",
            aim = "Aim Vector", ctrl_rot = "Current Control Rotation",
            weapon_base = "Root Scene", weapon_tip = "TippyTipScene",
        })
        if not ok then
            NSAMPLE.refused("config:" .. tostring(err))
            return false
        end
        NSAMPLE.configured = true
    end
    return true
end
-- A refusal: the Lua path takes this sample; a module-level one (no engine
-- access / verification refused) pauses native attempts for 5 s, logged once per reason.
function NSAMPLE.refused(err)
    NSAMPLE.fell_back = NSAMPLE.fell_back + 1
    err = tostring(err)
    if err == "unavailable" or err == "missing" or err == "not configured" or err:find("^disabled:") or err:find("^config:") then
        NSAMPLE.retry_at = os.clock() + 5
        if err == "not configured" then NSAMPLE.configured = false end
    end
    if NSAMPLE.logged ~= err then
        NSAMPLE.logged = err
        Log("native sampling refused (%s): Lua path", err)
    end
end
-- The bus record and both session views are cached tables, rebuilt only when their record
-- changes, so the same inputs give the same context: reuse it instead of building one per
-- sample (it runs at the sample rate; the pose writers copy it).
local ctx_memo = { ok = false }
pose_context = function(pawn)
    if not POSE_CONTEXT or not HS then return nil end
    local st, view, mode, peer, name = IPC.bus_table("spawn_status"), HS.view(), HS.mode(), get_my_peer_id(), pval(r_fname, pawn)
    local m = ctx_memo
    if m.ok and m.st == st and m.view == view and m.mode == mode and m.peer == peer and m.name == name then return m.ctx end
    m.st, m.view, m.mode, m.peer, m.name = st, view, mode, peer, name
    m.ctx, m.ok = POSE_CONTEXT.of(st, view, mode, peer, name), true
    return m.ctx
end
local function addr_of(o) return pval(r_addr, o) end
-- The pose part of a native sample, into `a`: the same weapon policy as
-- put_skeletal_state (two-handed grip, one actor per address, no "Fists").
-- Returns the number of weapons, or nil when the mesh or pawn has no address.
function NSAMPLE.pose_fields(a, pawn, mesh, tick, ts, dstep)
    a.context=pose_context(pawn)
    if not a.context then a.mesh=nil; return nil end
    a.mesh, a.pawn = addr_of(mesh), addr_of(pawn)
    if not (a.mesh and a.pawn) then a.mesh = nil; return nil end
    a.w1, a.h1, a.t1, a.w2, a.h2, a.t2 = 0, 0, 0, 0, 0, 0
    a.s1, a.s2 = nil, nil
    a.strikers = NO_STRIKERS
    if BODY_STRIKERS then local ok,s=pcall(BODY_STRIKERS.of,pawn,mesh,Log); if ok then a.strikers=s end end
    local two = pget(pawn, "R Two Handed Grip") == true
    local seen, nw = nil, 0
    for _, side in ipairs(WEAPON_SIDES) do
        local w = pget(pawn, WEAPON_FIELD[side])
        if w and w:IsValid() then
            local addr = addr_of(w)
            if addr ~= nil and addr ~= seen then
                seen = seen or addr
                local cls = pval(r_clsname, w)
                if not (cls and cls:find("Fists", 1, true)) then
                    nw = nw + 1
                    local hands = (side == "R") and (two and 3 or 1) or 2
                    if nw == 1 then a.w1, a.h1, a.t1 = addr, hands, class_tag(cls)
                    else a.w2, a.h2, a.t2 = addr, hands, class_tag(cls) end
                    if WEAPON_BOUNDS then
                        local ok, boxes = pcall(WEAPON_BOUNDS.of, w, WG.key, os.clock(), Log)
                        if ok then if nw==1 then a.s1=boxes else a.s2=boxes end end
                    end
                end
            end
        end
    end
    a.pose_tick, a.pose_ts, a.dt, a.k, a.control = tick, ts, dstep or 0, 0, tick % 2 == 0
    return nw
end
local _pose_stats = { n = 0, bones = 0, wpn = 0, ctl = 0, cost_ms = 0, at = 0 }

-- The Lua path: the same sample as numbers into reused flat arrays, one
-- IPC.put_pose call: the native module builds the codec v2 `pose` record once.
local _pb, _pw, _pc, _ps = {}, {}, {}, {}
local function put_skeletal_state(pawn, mesh, ts, dstep)
    local context=pose_context(pawn)
    if not context then return end
    for i = 1, #POSE_BONES do
        local ok, r = pcall(sample_bone, mesh, i)
        if not ok or not r then return end
        local b = (i - 1) * 13
        for k = 1, 13 do
            local v = r[k]
            if not finite(v) then return end
            _pb[b + k] = v
        end
    end
    for k = #_pw, 1, -1 do _pw[k] = nil end
    _ps[1], _ps[2] = nil, nil
    local seen, two = nil, false
    pcall(function() two = pawn["R Two Handed Grip"] == true end)
    local nw = 0
    for _, side in ipairs({ "R", "L" }) do
        local w; pcall(function() w = pawn["Weapon " .. side] end)
        if w and w:IsValid() then
            local addr; pcall(function() addr = w:GetAddress() end)
            if addr == nil or addr ~= seen then
                seen = seen or addr
                local hands = (side == "R") and (two and 3 or 1) or 2
                local ok, got = pcall(sample_weapon_vals, pawn, w, hands, mesh, _pw, nw * 21)
                if ok and got then
                    nw = nw + 1
                    if WEAPON_BOUNDS then local bok, boxes = pcall(WEAPON_BOUNDS.of, w, WG.key, os.clock(), Log); if bok then _ps[nw]=boxes end end
                else for k = nw * 21 + 1, nw * 21 + 21 do _pw[k] = nil end end
            end
        end
    end
    _skel_seq = _skel_seq + 1
    local ctl = nil
    if _skel_seq % 2 == 0 then
        local ok = pcall(sample_control_vals, pawn, _pc)
        if ok then ctl = _pc; _pose_stats.ctl = _pose_stats.ctl + 1 end
    end
    local strikers=NO_STRIKERS
    if BODY_STRIKERS then local ok,s=pcall(BODY_STRIKERS.of,pawn,mesh,Log); if ok then strikers=s end end
    if IPC.put_pose(_skel_seq, ts, dstep or 0, 0, _pb, _pw, ctl, _ps, strikers, context) then
        _pose_stats.n = _pose_stats.n + 1
        _pose_stats.bones = #POSE_BONES
        if nw > 0 then _pose_stats.wpn = _pose_stats.wpn + 1 end
    end
end

local function write_skeletal_state(pawn, ts, dstep)
    local mesh = pose_mesh_for(pawn)
    if not mesh then return end
    if IPC then return put_skeletal_state(pawn, mesh, ts, dstep) end   -- the local_pose record slot
end

-- Ground-truth probe (HSMP_POSE_PROBE=1; docs/development/subsystems/replication.md
-- "Measuring"): every game frame, the sender's own bodies + blade, stamped
-- with the same clock as the pose records, appended to <state>/.probe_send.txt (flushed every ~0.5 s).
-- Probe wall clock: os.clock() is wall ms since process start (MSVC);
-- hsmp-pose-truth adds each process's start time (--send-start/--recv-start).
local function utc_ms() return os.clock() * 1000 end
local PROBE = (os.getenv("HSMP_POSE_PROBE") or "") == "1"
local _probe_buf, _probe_at = {}, 0
local function probe_frame(pawn, ts)
    local mesh = pose_mesh_for(pawn)
    if not mesh then return end
    local out = { string.format("S %.3f", ts) }
    for i = 1, #POSE_BONES do
        local t = mesh:GetSocketTransform(_pose_fn[i], 0)
        out[#out + 1] = string.format("%.3f %.3f %.3f %.6f %.6f %.6f %.6f",
            t.Translation.X, t.Translation.Y, t.Translation.Z, t.Rotation.X, t.Rotation.Y, t.Rotation.Z, t.Rotation.W)
    end
    local w; pcall(function() w = pawn["Weapon R"] end)
    local inhand = false
    if w and w:IsValid() then local ok, got = pcall(sample_weapon_vals, pawn, w, 1, mesh, _wv, 0); inhand = ok and got == true end
    if inhand then
        local c = weapon_parts(w)
        local b, tp
        pcall(function() b = c.base:K2_GetComponentLocation() end)
        pcall(function() tp = c.tip:K2_GetComponentLocation() end)
        if b and tp then out[#out + 1] = string.format("W %.3f %.3f %.3f %.3f %.3f %.3f", b.X, b.Y, b.Z, tp.X, tp.Y, tp.Z) end
    end
    out[#out + 1] = string.format("U %.0f", utc_ms())
    local vis = true
    pcall(function() vis = mesh:WasRecentlyRendered(0.25) end)
    out[#out + 1] = vis and "V 1" or "V 0"
    _probe_buf[#_probe_buf + 1] = table.concat(out, " ")
    if ts - _probe_at > 500 then
        _probe_at = ts
        local f = io.open(STATE_DIR .. "/.probe_send.txt", "ab")
        if f then f:write(table.concat(_probe_buf, "\n"), "\n"); f:close() end
        _probe_buf = {}
    end
end

local _weapon_seq = 0

local function write_weapon_state(ts, weapon, wid, cls_name, hand, native_done)
    if not weapon or not wid then return end
    if logged_weapon_cls ~= cls_name then
        logged_weapon_cls = cls_name
        Log("holding weapon: %s (id=%d, hand %s)", cls_name or "?", wid, hand or "?")
    end
    if native_done then return end   -- written by the native sample (NSAMPLE.all)
    local pos, rot, vel
    local ok1 = pcall(function() pos = weapon:K2_GetActorLocation() end)
    local ok2 = pcall(function() rot = weapon:K2_GetActorRotation() end)
    local ok3 = pcall(function() vel = weapon:GetVelocity() end)
    if not (ok1 and ok2 and ok3) or not pos or not rot or not vel then return end
    -- held hand: 0 none / 1 right / 2 left.
    local held = (hand == "L") and 2 or 1
    -- Per-write counter (sidecar dedups on tick) + the sample's shared ts.
    _weapon_seq = _weapon_seq + 1
    if IPC then   -- the local_weapon record slot
        IPC.put_weapon(_weapon_seq, ts, wid, held, pos.X, pos.Y, pos.Z, rot.Pitch, rot.Yaw, rot.Roll, vel.X, vel.Y, vel.Z)
    end
end

local _root_seq = 0   -- root write counter; the sidecar dedups on `tick`

local function read_my_transform(ts)
    local pawn = get_local_pawn()
    if not pawn then return nil end
    local context=pose_context(pawn)
    if not context then return nil end
    -- Wrap each reflection call in pcall. The Pawn pointer can be in a
    -- transitional state (level-reload, respawn) where IsValid returns true
    -- but the underlying UObject fields haven't been populated yet.
    local ok1, loc = pcall(function() return pawn:K2_GetActorLocation() end)
    local ok2, rot = pcall(function() return pawn:K2_GetActorRotation() end)
    local ok3, vel = pcall(function() return pawn:GetVelocity() end)
    if not (ok1 and ok2 and ok3) or not loc or not rot or not vel then
        return nil
    end
    -- `tick` must change on every write (sidecar dedups on it); use a write
    -- counter, not tick_counter, since writes come from the per-frame sender,
    -- not on_tick.
    _root_seq = _root_seq + 1
    return {
        pid  = PID,
        context = context,
        nick = "Willie",
        tick = _root_seq,
        -- Sender game clock (MSVC clock(): wall ms since process start).
        -- Receivers interpolate on it, so IPC/network jitter is removed.
        ts   = ts or math.floor(os.clock() * 1000),
        pos  = { loc.X, loc.Y, loc.Z },
        rot  = { rot.Pitch, rot.Yaw, rot.Roll },
        vel  = { vel.X, vel.Y, vel.Z },
    }
end

-- The local_root record slot (one native call: rotator -> quaternion in the native module).
local function write_my_state(ts)
    local st = read_my_transform(ts)
    if not st or not IPC then return false end
    local p, r, v = st.pos, st.rot, st.vel
    return IPC.put_root(st.tick, st.ts, p[1], p[2], p[3], r[1], r[2], r[3], v[1], v[2], v[3],st.context) and true or false
end

-- --- MP session gate --------------------------------------------------------------
-- Root / weapon / pose sending (fast_send) and the tick's settings reads run
-- only while an MP session is LIVE: shared/hsmp_session.lua, the one rule every
-- mod uses (the `link` record says connected AND the sidecar's header heartbeat
-- is fresh). Single-player never pays for sampling (23 bones x 3 UFunction
-- calls at 60 Hz), and a "connected" link left behind by a dead sidecar never
-- counts (no heartbeat).
-- The background tick (t.IdleWhenNotForeground 0: the local player's body must
-- keep simulating and sending while alt-tabbed) is set only while live and the
-- previous value is restored when the session is not live any more.
if not HS then Log("WARNING: shared/hsmp_session.lua missing - no MP streaming (deploy copies shared/*.lua)") end
local MP = { S = HS and HS.new({}), live = false, bg_on = false, bg_prev = nil }
-- Native sampling A/B from a dev command (hsmp-tools ipc-ctl tune native_sample 0|1).
MP.native_sample_tune = function(v) NSAMPLE.set(tonumber(v) ~= 0 and v ~= false, "dev tune") end
MP.native_sample = NSAMPLE
function MP.poll()
    local live = false
    if MP.S then pcall(function() MP.S:poll(); live = MP.S:live() end) end
    if live ~= MP.live then
        Log("MP session %s (sidecar status=%s)", live and "LIVE: pose/root/weapon streaming on" or "not live: streaming off",
            tostring(MP.S and MP.S.status))
    end
    MP.live = live
    return live
end
local BG_CVAR = "t.IdleWhenNotForeground"
function MP.cvar(value)
    local ok = false
    pcall(function()
        local ksl = StaticFindObject("/Script/Engine.Default__KismetSystemLibrary")
        local world = WG.world()
        if ksl and ksl:IsValid() and world then
            ksl:ExecuteConsoleCommand(world, FString(BG_CVAR .. " " .. tostring(value)), nil) -- unsafe: ok fixed CVar t.IdleWhenNotForeground (background tick), never travel/quit/open
            ok = true
        end
    end)
    return ok
end
function MP.background(on)
    on = on and true or false
    if on == MP.bg_on then return end
    if on then
        local prev
        pcall(function()
            local ksl = StaticFindObject("/Script/Engine.Default__KismetSystemLibrary")
            prev = ksl:GetConsoleVariableIntValue(FString(BG_CVAR))
        end)
        MP.bg_prev = tonumber(prev)
        if not MP.cvar(0) then return end
        Log("background tick on (%s 0, was %s)", BG_CVAR, tostring(MP.bg_prev))
    else
        if MP.bg_prev ~= nil then
            if not MP.cvar(MP.bg_prev) then return end
            Log("background tick restored (%s %d)", BG_CVAR, MP.bg_prev)
        else
            Log("background tick: the previous %s value was unreadable; left as is", BG_CVAR)
        end
    end
    MP.bg_on = on
end

-- --- main loop -------------------------------------------------------------

tick_counter = 0

local settings_poll = 0
local send_divider  = 1   -- derived from send_hz below; 1 = send every tick
local tick_in_cycle = 0

local function recompute_send_divider()
    -- This mod's loop is 30 Hz (game side; the server tick is separate). If send_hz is less, skip ticks between sends.
    if send_hz >= 30 then
        send_divider = 1
    else
        send_divider = math.max(1, math.floor(30 / math.max(1, send_hz)))
    end
end
recompute_send_divider()

-- Startup-menu gate. HSMPSync's reflection paths crash the game on
-- Map_Menu_Startup because the preview-pawn hierarchy isn't the real
-- character. Short-circuit the whole tick until a gameplay world is loaded.
-- 1 s probe cache so GetFullName() itself doesn't run at 30 Hz.
local _in_game_cache = false
local _in_game_checked_tick = -999
local _last_in_game = false
local function is_in_gameplay_world()
    if tick_counter - _in_game_checked_tick < 30 then return _in_game_cache end
    _in_game_checked_tick = tick_counter
    _in_game_cache = false
    pcall(function()
        local w = WG.world()
        if w and w:IsValid() then
            local path = w:GetFullName()
            _in_game_cache = not (path:find("Map_Menu_") ~= nil)
        end
    end)
    return _in_game_cache
end

-- ===== Spawn placement (spawn_place.lua) =======================================
-- WHERE the local pawn spawns is decided by the server only: one absolute
-- order per player per round in the session record
-- (docs/development/subsystems/spawns.md).
-- spawn_place.lua places the pawn on it, verifies that the move stuck,
-- tells the Director (bus `spawn_status`), keeps the pawn under spawn
-- protection until placement + protect_ms, and runs the fall watchdog. This
-- block only wires it to this mod's world guard and state files.
local function load_sibling(name)
    local ok, m = pcall(require, name)
    if ok and type(m) == "table" then return m end
    local src = (debug.getinfo(1, "S").source or ""):gsub("^@", "")
    local dir = src:match("^(.*)[/\\]") or "."
    local ok2, m2 = pcall(dofile, dir .. "/" .. name .. ".lua")
    if ok2 and type(m2) == "table" then return m2 end
    return nil, tostring(m) .. " / " .. tostring(m2)
end
local SPL, spl_err = load_sibling("spawn_place")
if not SPL then Log("ERROR: spawn_place.lua failed to load (%s) - no spawn placement", tostring(spl_err)) end
if SPL and SPL.peer_reader then
    -- The sidecar's `link` record + its header heartbeat (shared/hsmp_session.lua).
    local PEER_S = HS and HS.new({ every_s = 0 })
    local read_peer = SPL.peer_reader(function()
        if not PEER_S then return nil, 0, false end
        PEER_S:poll(true)
        if not PEER_S.exists then return nil, 0, false end
        return PEER_S.status, PEER_S.link and PEER_S.link.my_peer_id or 0, PEER_S:fresh()
    end, os.clock, 0.5, Log)
    refresh_my_peer_id = function(force) my_peer_id, my_peer_status, my_session_live = read_peer(force) end
end
if SPL and SPL.match_reader then
    match_get = SPL.match_reader(function() return HS and HS.view() end)
end
local function session_live()
    pcall(refresh_my_peer_id)
    return my_session_live
end

local function current_world_name()
    local n
    pcall(function()
        local w = WG.world()
        if w and w:IsValid() then n = w:GetFullName() end
    end)
    return n
end

local function in_arena_world()
    local n = current_world_name()
    return n ~= nil and n:find("Map_Arena_") ~= nil
end

local spawn_env = SPL and SPL.make_ue_env({
    UEHelpers = UEHelpers, pc = WG.pc, ai_pawn = WG.ai_pawn, log = Log, state_dir = STATE_DIR,
    my_peer_id = get_my_peer_id,   -- re-read every call (0.5 s throttle), never latched
    match = function() return current_match() end,
    world = function()
        return { key = WG.key, short = (current_world_name() or ""):match("([%w_]+)$") }
    end,
    view = function() return HS and HS.view() end,
    mode = function() return HS and HS.mode() end,   -- the session record (spawn plan)
    session_live = session_live,         -- no order without a live MP session
    connected = function() return session_live() and my_peer_status == "connected" end,
})
local spawn = SPL and SPL.new(spawn_env, { state_dir = STATE_DIR })

-- true while the local pawn is under spawn protection (no death reports).
local function spawn_protected()
    return spawn ~= nil and spawn:protected()
end
is_spawn_protected = spawn_protected

local function spawn_order_text()
    return spawn and spawn:order_text() or "spawn_place.lua not loaded"
end

-- Runs every tick while the world guard passed and an arena is loaded.
local function spawn_rescue(pc)
    if not spawn or not pc or not pc:IsValid() or not in_arena_world() then return end
    local ok, err = pcall(spawn.tick, spawn)
    if not ok then Log("spawn tick error: %s", tostring(err)) end
end

-- World changed / level change requested: forget every world-scoped object
-- without touching it (pawn + mesh caches, the pose mesh, the spawn placer's
-- world state, which re-arms placement for the new world: an arena reload
-- keeps the world name, so the name alone cannot re-arm it).
-- fast_send stays off until on_tick has re-probed the new world.
local _world_ok = false
wg_on_drop(function()
    _pawn_cache, _mesh_cache = nil, nil
    _skel.pawn, _skel.mesh, _skel.bones = nil, nil, nil
    if spawn then spawn:reset("world dropped") end
    if spawn_env and spawn_env.drop_native then spawn_env.drop_native() end
    _in_game_checked_tick = -999
    _world_ok = false
    _died_round = nil
    _died_context, _died_pawn = nil, nil
end)

local function on_tick()
    tick_counter = tick_counter + 1
    local pc0 = WG.pc()   -- this frame's one PlayerController lookup
    if not wg_check(pc0) then return end
    -- Don't touch pawns/actors on the Startup menu: the preview hierarchy
    -- isn't the player and reflection on it crashes the game.
    local in_game = is_in_gameplay_world()
    _world_ok = true
    if not in_game then
        MP.background(false)
        -- Leaving gameplay (back to the menu): reset the spawn placer and
        -- the caches so the next arena load starts fresh.
        if _last_in_game then
            _last_in_game = false
            if spawn then spawn:reset("left gameplay") end
            _logged_in_game_classes = false
            _pawn_cache = nil
            _mesh_cache = nil
        end
        return
    end
    _last_in_game = true
    tick_in_cycle = tick_in_cycle + 1

    -- Logged once per in-game session: the world, pawn class and position
    -- Half Sword starts us with (the pawn may still be nil; that is logged
    -- too).
    local pc_now = pc0
    local pawn_now
    if pc_now and pc_now:IsValid() then
        local p = pc_now.Pawn
        if p and p:IsValid() then pawn_now = p end
    end

    if not _logged_in_game_classes then
        _logged_in_game_classes = true
        pcall(function()
            local gi = UEHelpers.GetGameInstance()
            local world = WG.world()
            local pawn_cls = pawn_now and pawn_now:GetClass():GetFName():ToString() or "nil"
            local gi_cls   = gi    and gi:IsValid()    and gi:GetClass():GetFName():ToString()   or "nil"
            local wname    = world and world:GetFullName() or "nil"
            local lx, ly, lz = 0, 0, 0
            if pawn_now then
                local l; pcall(function() l = pawn_now:K2_GetActorLocation() end)
                if l then lx, ly, lz = l.X, l.Y, l.Z end
            end
            Log("world=%s pawn_cls=%s pos=(%.0f,%.0f,%.0f) gi_cls=%s",
                wname, pawn_cls, lx, ly, lz, gi_cls)
        end)
    end

    -- Spawn placement; spawn_place.lua gates itself on the session.
    if pc_now and pc_now:IsValid() then
        spawn_rescue(pc_now)
    end

    -- The settings reload and death detection below (and fast_send) run only
    -- during a live MP session.
    -- Dev knobs from the DevCtl ring, polled every tick (also outside a
    -- session, so nothing queues up): TDIAG = timing diagnostics on / off
    -- (`hsmp-tools ipc-ctl --pid <game> tdiag on|off`); TUNE native_sample = native pose
    -- sampling on / off (`ipc-ctl ... tune native_sample 1|0`, MP.native_sample_tune).
    if IPC and IPC.dev_poll then
        MP.dev_out = MP.dev_out or {}
        for i = 1, IPC.dev_poll(16, MP.dev_out) do
            local c = MP.dev_out[i] and (MP.dev_out[i].data or MP.dev_out[i])
            if type(c) == "table" and c.op == 3 then   -- S.ENUMS.dev_op.TDIAG
                local on = (tonumber(c.num) or 0) ~= 0
                if on ~= (MP.tdiag == true) then Log("tdiag %s (dev_cmd #%s)", on and "on" or "off", tostring(c.id)) end
                MP.tdiag = on
            elseif type(c) == "table" and c.op == 2 and c.key == "native_sample" and MP.native_sample_tune then   -- TUNE
                MP.native_sample_tune(tonumber(c.num) or 0)
            end
        end
    end

    local live = MP.poll()
    MP.background(live)
    if not live then return end

    settings_poll = settings_poll + 1
    if settings_poll >= 30 then   -- ~1 s @ 30 Hz
        settings_poll = 0
        local prev = send_hz
        refresh_settings()
        if send_hz ~= prev then
            recompute_send_divider()
            Log("settings reload: send_hz=%d (divider=%d), hud=%s",
                send_hz, send_divider, tostring(hud_on))
        end
    end

    -- Rate-limit the slower state according to send_hz.
    if tick_in_cycle < send_divider then return end
    tick_in_cycle = 0

    -- Root, weapon and pose are written by the per-frame sender (fast_send);
    -- on_tick only gates the slower state on having a pawn.
    if get_local_pawn(pc0) == nil then return end

    -- Death / respawn (native DED or nonpositive Health, never KO hints).
    pcall(check_death, pc0)
end

local function toggle_verbose()
    verbose = not verbose
    Log("verbose %s", verbose and "ON" or "OFF")
end

RegisterKeyBindAsync(Key.F4, {}, toggle_verbose)

-- F7: dump a snapshot of the current gameplay state (game instance flags,
-- controllers, Willies, level manager, spawn slots) for comparing the
-- game's own Play flow with the HSMP host flow. Writes
-- <state_dir>/.diag_<timestamp>.txt.
local function dump_diagnostics()
    local path = STATE_DIR .. "/.diag_" .. os.time() .. ".txt"
    local f = io.open(path, "wb")
    if not f then Log("dump: cannot open %s", path); return end
    local function w(s) f:write(s); f:write("\n") end

    w("=== HSMP state dump @ " .. os.date() .. " ===")

    pcall(function()
        local world = UEHelpers.GetWorld()
        w("world: " .. (world and world:GetFullName() or "nil"))
    end)
    pcall(function()
        local gi = UEHelpers.GetGameInstance()
        w("gi_class: " .. (gi and gi:GetClass():GetFName():ToString() or "nil"))
        if gi and gi:IsValid() then
            -- Real (space-containing) GI property names.
            for _, k in ipairs({"Player Just Died","Current Play Mode","Current Game Mode Enum",
                                "Current Combat Mode","Free Mode Activated","FreeMode Multiplayer",
                                "Current FreeMode Map","Free Mode Foes Amount","Combatants Amount",
                                "Free Mode Tier","Free Mode Brawling","Free Mode Carnage",
                                "Free Mode Blossfechten","Rounds To WIn","Rounds Won","Startup",
                                "Fresh Start Map (Temp)"}) do
                local v; pcall(function() v = gi[k] end)
                w(string.format("  gi[%q] = %s", k, tostring(v)))
            end
            pcall(function() w("  gi[\"Last Open Level Path\"] = " .. gi["Last Open Level Path"]:ToString()) end)
        end
    end)
    pcall(function()
        local pcs = FindAllOf("PlayerController")
        local n = 0; for _, _p in pairs(pcs or {}) do n = n + 1 end
        w("PlayerControllers: " .. n)
        for _, p in pairs(pcs or {}) do
            if p and p:IsValid() then
                local pn = p.Pawn
                local pawn_cls = (pn and pn:IsValid()) and pn:GetClass():GetFName():ToString() or "nil"
                local loc; if pn and pn:IsValid() then pcall(function() loc = pn:K2_GetActorLocation() end) end
                local pos = loc and string.format("(%.0f,%.0f,%.0f)", loc.X, loc.Y, loc.Z) or "nil"
                w(string.format("  PC %s -> Pawn %s @ %s", p:GetFName():ToString(), pawn_cls, pos))
            end
        end
    end)
    pcall(function()
        local ws = FindAllOf("Willie_BP_C")
        local n = 0; for _, _w in pairs(ws or {}) do n = n + 1 end
        w("Willie_BP_C instances: " .. n)
        for _, ww in pairs(ws or {}) do
            if ww and ww:IsValid() then
                local loc; pcall(function() loc = ww:K2_GetActorLocation() end)
                local pos = loc and string.format("(%.0f,%.0f,%.0f)", loc.X, loc.Y, loc.Z) or "nil"
                local hp; pcall(function() hp = ww.Health end)
                local wu; pcall(function() wu = ww["Initial Wake Up State"] end)
                local sg; pcall(function() sg = ww["Spawn On Ground"] end)
                w(string.format("  Willie %s @ %s HP=%s wake=%s onground=%s",
                    ww:GetFName():ToString(), pos, tostring(hp), tostring(wu), tostring(sg)))
            end
        end
    end)
    pcall(function()
        local lms = FindAllOf("BP_LevelManager_C")
        local n = 0; for _, _l in pairs(lms or {}) do n = n + 1 end
        w("BP_LevelManager instances: " .. n)
        for _, lm in pairs(lms or {}) do
            if lm and lm:IsValid() then
                for _, k in ipairs({"Player Spawned","P2 Spawned","Amount of Characters to Spawn",
                                    "Purge Map","Level Load Delay"}) do
                    local v; pcall(function() v = lm[k] end)
                    w(string.format("  lm[%q] = %s", k, tostring(v)))
                end
            end
        end
    end)
    pcall(function()
        local gm = UEHelpers.GetGameplayStatics():GetGameMode(UEHelpers.GetWorld())
        if gm and gm:IsValid() then
            w("GameMode: " .. gm:GetClass():GetFName():ToString())
        end
    end)
    pcall(function()
        local starts = FindAllOf("PlayerStart")
        local n = 0; for _, _s in pairs(starts or {}) do n = n + 1 end
        w("PlayerStart instances: " .. n)
        for _, s in pairs(starts or {}) do
            if s and s:IsValid() then
                local loc; pcall(function() loc = s:K2_GetActorLocation() end)
                local pos = loc and string.format("(%.0f,%.0f,%.0f)", loc.X, loc.Y, loc.Z) or "nil"
                w("  start @ " .. pos)
            end
        end
    end)
    pcall(function()
        local pts = spawn_env and spawn_env.native_points() or {}
        w("Spawn slots (BP_SpawnerPoint_Willies): " .. #pts .. "  server order: " .. spawn_order_text()
            .. "  protected: " .. tostring(spawn_protected()))
        local pawn = UEHelpers.GetPlayerController().Pawn
        for i, p in ipairs(pts) do
            local g = pawn and pawn:IsValid() and spawn_env.ground(pawn, p.X, p.Y, p.Z) or nil
            w(string.format("  slot %d (%.0f,%.0f,%.0f) yaw=%.0f player=%s team=%d ground=%s",
                i - 1, p.X, p.Y, p.Z, p.Yaw, tostring(p.player), p.team,
                g and string.format("%.0f", g) or "NONE"))
        end
    end)
    w("=== end ===")
    f:close()
    Log("DUMP WROTE: %s", path)
end
RegisterKeyBindAsync(Key.F7, {}, function()
    pcall(function() ExecuteInGameThread(dump_diagnostics) end)
end)

-- Per-frame sender: root + weapon + pose at send_hz (default 60, max 120),
-- on a drift-free schedule driven by the game clock. Runs every frame
-- on the game thread, so a sample is taken within one frame of being due
-- (a 33 ms loop plus an ExecuteInGameThread hop would add up to ~50 ms and
-- ±16 ms of jitter before the sidecar sees it).
-- One sample = root + held weapon + full pose, all read in the same game
-- frame and stamped with the same sender ts.
local _root_next_ms = 0
-- The one IPC pump per game frame (heartbeat, world key, doorbell,
-- sidecar attach / death, S2G drain). world_ready once the world key has been
-- stable for the hsmp_wg settle time (2 s); the Director's OpenLevel
-- pre-hooks report the leave (HSMPMatch).
MP.ipc_settle = HW.settle_tracker and HW.settle_tracker() or nil
local function ipc_pump()
    local key = WG.key
    IPC.frame(key)
    local st = MP.ipc_settle
    if st and key ~= nil and st.note(key).settled() and MP.ipc_ready_key ~= key then
        MP.ipc_ready_key = key
        IPC.world_ready(key)
    end
end
-- Native sampling of one sample: root, held weapon and pose in ONE sample_local call
-- (mask bit 1 root, 2 weapon, 4 pose written). Returns the mask; fast_send runs the Lua
-- path for the parts it did not write. A refusal is counted and logged once per call.
function NSAMPLE.all(pawn, ts, tsf, dstep, weapon, wid, hand)
    local a = NSAMPLE.a
    a.context=pose_context(pawn)
    if not a.context then a.root_pawn=nil;a.mesh=nil;return 0 end
    a.pawn=addr_of(pawn)
    a.root_pawn, a.root_tick, a.root_ts = addr_of(pawn), _root_seq + 1, ts
    if weapon and wid then
        a.weapon_actor, a.weapon_tick, a.weapon_ts, a.weapon_id = addr_of(weapon), _weapon_seq + 1, ts, wid
        a.weapon_held = (hand == "L") and 2 or 1
    else
        a.weapon_actor = nil
    end
    local mesh = pose_mesh_for(pawn)
    local nw = mesh and NSAMPLE.pose_fields(a, pawn, mesh, _skel_seq + 1, tsf, dstep)
    if not nw then a.mesh = nil end
    local want = (a.root_pawn and 1 or 0) | (a.weapon_actor and 2 or 0) | (a.mesh and 4 or 0)
    if want == 0 then return 0 end
    local mask, err = IPC.sample_local(a)
    mask = math.tointeger(mask) or 0
    if mask & want ~= want then NSAMPLE.refused(err) end
    if mask & 1 ~= 0 then _root_seq = _root_seq + 1 end
    if mask & 2 ~= 0 then _weapon_seq = _weapon_seq + 1 end
    if mask & 4 ~= 0 then
        _skel_seq = _skel_seq + 1
        _pose_stats.n = _pose_stats.n + 1
        _pose_stats.bones = #POSE_BONES
        if nw > 0 then _pose_stats.wpn = _pose_stats.wpn + 1 end
        if _skel_seq % 2 == 0 then _pose_stats.ctl = _pose_stats.ctl + 1 end
        NSAMPLE.used = NSAMPLE.used + 1
        NSAMPLE.nw = nw
    end
    return mask & want
end
local function fast_send()
    if IPC and IPC.backend == "shm" then pcall(ipc_pump) end
    if not (_last_in_game and _world_ok and MP.live) then return end   -- on_tick maintains the world + MP gates
    local now = os.clock() * 1000
    -- 8 ms floor allows up to ~120 Hz; actual rate is also bounded by frame rate.
    local interval = math.max(8, 1000 / math.max(1, math.min(send_hz or 60, 120)))
    local due = now >= _root_next_ms
    if not due and not PROBE then return end
    -- Per-frame callback: re-check the world before touching the cached pose
    -- mesh (a level change can land between two on_tick runs).
    local pc = WG.pc()   -- shared with every other callback of this frame
    if not wg_check(pc) then return end
    local pawn = get_local_pawn(pc)
    if not pawn then return end
    local tsf = precise_ms(pawn)
    if PROBE then pcall(probe_frame, pawn, tsf) end
    -- Timing diagnostics (dev: `hsmp-tools ipc-ctl --pid <game> tdiag on`): per frame the
    -- clocks, the world delta and the pelvis state, to relate sample stamps
    -- to the physics step they stand for.
    if MP.tdiag then
        pcall(function()
            local gs = UEHelpers.GetGameplayStatics()
            local d = gs:GetWorldDeltaSeconds(pawn) * 1000
            local rt = gs:GetRealTimeSeconds(pawn) * 1000
            local mesh = pawn.Mesh
            local fn = FName("pelvis")
            local t = mesh:GetSocketTransform(fn, 0)
            local v = mesh:GetPhysicsLinearVelocityAtPoint(t.Translation, fn)
            MP.tbuf = MP.tbuf or {}
            MP.tbuf[#MP.tbuf + 1] = string.format("S %.3f %.3f %.4f %.3f %.3f %.3f %.3f %.2f %.2f %.2f %d",
                now, rt, d, tsf, t.Translation.X, t.Translation.Y, t.Translation.Z, v.X, v.Y, v.Z, due and 1 or 0)
            if #MP.tbuf >= 60 then
                local f = io.open(STATE_DIR .. "/.tdiag_send.txt", "ab")
                if f then f:write(table.concat(MP.tbuf, "\n"), "\n"); f:close() end
                MP.tbuf = {}
            end
        end)
    end
    if not due then return end
    _root_next_ms = math.max(_root_next_ms + interval, now - interval)
    local ts = math.floor(tsf)   -- root / weapon streams carry integer ms
    local okw, weapon, wid, cls, hand = pcall(find_held_weapon, pawn)
    if not okw then weapon, wid, cls, hand = nil, nil, nil, nil end
    -- The bodies read here stand for the END of this frame's physics step
    -- (this callback runs after it; tsf is the frame START). The step goes
    -- along as "dt" (codec v2 optional step byte): the receiver places the
    -- pose at ts + dt but keeps its clock offset / jitter estimate on ts,
    -- whose arrival does not swing with the frame length (measured with
    -- tdiag: p95 42-48 ms vs 72-86 ms at 20-35 fps). Without it a 25-55 ms
    -- frame puts each pose up to a frame early on the timeline: jagged
    -- targets at low frame rates (LordsHall). Lag comp keeps the plain ts.
    local dstep = 0
    local d = pval(r_delta, pawn)
    if type(d) == "number" then
        d = d * 1000
        if d == d and d > 0 and d <= 80 then dstep = d end
    end
    -- Native sampling takes root + weapon + pose in one call; the Lua path writes
    -- whatever it did not.
    local t0 = os.clock() * 1000
    local nat = 0
    if NSAMPLE.on() then
        local okn, m = pcall(NSAMPLE.all, pawn, ts, tsf, dstep, weapon, wid, hand)
        if okn then nat = m end
    end
    if nat & 1 == 0 then pcall(write_my_state, ts) end
    pcall(write_weapon_state, ts, weapon, wid, cls, hand, nat & 2 ~= 0)
    if nat & 4 == 0 then pcall(write_skeletal_state, pawn, tsf, dstep) end
    -- Sender cost / rate report (os.clock is ms-resolution: average it).
    _pose_stats.cost_ms = _pose_stats.cost_ms + (os.clock() * 1000 - t0)
    if now - _pose_stats.at >= 5000 then
        if _pose_stats.at > 0 then
            local secs = (now - _pose_stats.at) / 1000
            Log("pose sender: %.1f frames/s (target %d Hz) codec v2, %d bones + weapon in %d%%, control in %d%%, pose sample+write avg %.2f ms, sampler %s (native %d, fallbacks %d)",
                _pose_stats.n / secs, math.min(send_hz or 60, 120), _pose_stats.bones,
                _pose_stats.n > 0 and math.floor(100 * _pose_stats.wpn / _pose_stats.n) or 0,
                _pose_stats.n > 0 and math.floor(100 * _pose_stats.ctl / _pose_stats.n) or 0,
                _pose_stats.n > 0 and _pose_stats.cost_ms / _pose_stats.n or 0,
                NSAMPLE.want and "native" or "lua", NSAMPLE.used, NSAMPLE.fell_back)
            NSAMPLE.used, NSAMPLE.fell_back = 0, 0
        end
        _pose_stats.n, _pose_stats.wpn, _pose_stats.ctl, _pose_stats.cost_ms, _pose_stats.at = 0, 0, 0, 0, now
    end
end

if LoopInGameThreadAfterFrames and EngineTickAvailable ~= false then
    LoopInGameThreadAfterFrames(1, fast_send)
else
    LoopAsync(8, function() ExecuteInGameThread(fast_send); return false end)
end

-- Run on_tick every 33 ms (~30 Hz) for the slower state (world gate, spawn
-- placement, death/respawn, peer id reads). The shim keeps this on the game thread.
LoopAsync(33, function()
    local ok, err = pcall(function()
        ExecuteInGameThread(on_tick)
    end)
    if not ok then verbose_fail("tick error: " .. tostring(err)) end
    return false -- keep looping
end)

Log("loaded. PID=%d (shared-memory IPC). F4=verbose, F5=list peers now.", PID)

