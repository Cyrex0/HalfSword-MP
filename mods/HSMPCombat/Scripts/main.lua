-- HSMPCombat — owner-authoritative damage / death replication.
--
-- Half Sword's damage model (Willie_BP_C, decompiled; see
-- docs/development/subsystems/combat.md): a
-- weapon's "Collision Hit" (or a Willie's body contact) calls "Deal Complex
-- Damage" on the victim (the armour stage), which calls "Get Damage". Damage
-- lands across many fields — Health (floored per body part), per-part health,
-- Consciousness, Bleeding, Pain, blood and wound marks — not just Health.
--
-- UE4SS: a hook on a Blueprint function runs its callback AFTER the body;
-- a third ("post") callback is never called (ue4ss Docs registerhook.md).
-- Nothing here can change a BP call's inputs before it runs.
--
-- Attacker side (my weapon / body hits a peer's stand-in):
--   the stand-in is Invulnerable and its weapons damage-gated, so it takes
--   nothing; the "Deal Complex Damage" callback reads the call's inputs
--   (impulse, velocity, cutting power, stab rate, rigidity, kick, ...) and
--   that is the claim (FLAG_COMPLEX), one per contact and bone. Nothing is
--   measured on the stand-in.
--
-- Owner side (a hit on my real character arrives as a `damage_in` record):
--   replay it natively through my own pawn's "Deal Complex Damage" (my armour,
--   my height, my wounds, bleeding and death). Native cutting/constraint
--   construction is a separate path and is not reproduced by scalar replay.
--   A stand-in's blow that lands on my pawn locally (an echo) is undone in the
--   callback from my per-tick baseline and reported as a "touch" (evidence
--   for the server that a clash I also saw did not stop the blow).
--
-- Other screens: a changed native owner injury authorizes broadcast (S2CHitFx ->
-- `hitfx_in` records) and replayed on my stand-in of its victim for the native
-- blood, wounds and bruises; the stand-in's damage state is put back at once.
--
-- Vitals (docs/development/subsystems/vitals.md): ~15 Hz on change, this mod samples OUR
-- pawn's Health, every limb health, Consciousness, Stamina/Exhaustion,
-- Bleeding/Blood Rate, Pain, DED/Fallen/Downed/Headless/Pain Shock and the
-- confirmed native completed distal cuts into the `vitals` record (quantised); the server relays it (only a
-- heal faster than any game heal is clamped); each stand-in mirrors its
-- owner's frame (never lethally). Peers see the victim's own values.
--
-- Authoritative deaths: the server declares deaths (owner
-- report, owner vitals, or its own damage ledger when a lethal hit is
-- accepted) and broadcasts it -> `death` records {peer_id, round, ...}.
-- A line about US in the current live/roundover round forces our pawn dead
-- (Health 0 + native "Death"), even if our copy hasn't applied the lethal hit
-- yet; HSMPMatch freezes input from the scoreboard. Our own native
-- Death/Dying also reports a `death_report` record (reliable) at once.
--
-- MP spawn heal: career-save limb wounds are applied to the pawn on spawn
-- (the Tavern normally heals them; MP skips it). On every new local pawn and
-- at each round's live start (until the pawn takes a hit) the damage fields
-- are restored from the Willie_BP CDO. The save is never written.
--
-- Lag compensation (server/src/lagcomp.rs): every hit also carries
--   ats  my own sender clock at the hit (same clock as HSMPSync's ts),
--   vts  the victim's sender time its stand-in body was displaying,
--   vats the same for its arms/weapon,
-- from HSMPAvatars' bus key "playback". The server rewinds both players to those
-- instants. Weapon-on-weapon contact between my weapon and a stand-in's
-- (ModularWeaponBP "Collision Hit") is reported as a "clash" so the server
-- can cancel a hit the defender parried on their own screen.

local UEHelpers = require("UEHelpers")

-- Thread-safety shim. LoopAsync, ExecuteWithDelay and *Async keybind
-- callbacks run on UE4SS worker threads; running them alongside game-thread
-- Lua corrupted this mod's VM (symbolized crash dumps: lua_next / __index on
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
    print(string.format("[HSMPCombat] " .. fmt .. "\n", ...))
end

local function trim(s) return s and s:match("^%s*(.-)%s*$") or nil end
local STATE_DIR = (trim(os.getenv("HSMP_STATE_DIR")) or "hsmp_state"):gsub("\\", "/")

local GET_DAMAGE    = "/Game/Character/Blueprints/Willie_BP.Willie_BP_C:Get Damage"
-- Armour stage (Willie_BP, BPI_Complex_Collision): the weapon's Collision Hit
-- calls it with the PRE-armour impact; it traces the proxy armour layers and
-- then calls Get Damage. The attacker captures its inputs on the stand-in and
-- the victim replays it on its own pawn: the victim's armour, solo parity.
local DEAL_COMPLEX  = "/Game/Character/Blueprints/Willie_BP.Willie_BP_C:Deal Complex Damage"
local FLAG_COMPLEX  = 32       -- claim flags bit 5: impact fields carry Deal Complex Damage inputs
-- More claim flags and limits live in the BF table below (the main chunk is
-- at Lua's 200-locals limit):
--   BF.LOCAL (bit 6): offset / normal / velocity / impulse are in the hit
--     bone's frame (rotation removed, offset divided by the bone scale). Deal
--     Complex Damage maps the hit point into bone space and traces the armour
--     layers from there, so a world-space point re-added to a victim that
--     turned since the attacker saw it lands somewhere else (helmet vs face).
--   BF.WEAPON (bit 7): the striking component was a weapon, not a body part.
--     The replay passes the attacker's weapon as Collided Component; Get
--     Damage reads its 'Weapon' tag (consciousness on light blows).
--   BF.GD_GATE_MS: Get Damage's per-bone gate (Last Damage Taken) resets
--     0.2 s after the last blow that passed it. Replays of blows further apart
--     than that on the attacker's clock must not gate each other.
--   BF.MAX_PER_BONE_TICK: armour-stage calls per bone and tick that become claims.
local DEATH_FN      = "/Game/Character/Blueprints/Willie_BP.Willie_BP_C:Death"
local DYING_FN      = "/Game/Character/Blueprints/Willie_BP.Willie_BP_C:Dying"
local WEAPON_HIT    ="/Game/Assets/Weapons/Blueprints/ModularWeaponBP.ModularWeaponBP_C:Collision Hit"
local CLASH_GAP_MS  = 100      -- per-peer clash report rate limit (a long blade-on-blade
                               -- contact is reported every 100 ms: server bucket 5 / 10 per s)
-- Claims: every armour-stage call on a stand-in that passed the game's own
-- contact gate (see flush_claims), at most BF.MAX_PER_BONE_TICK per bone and tick.
-- FIELDS (1-based) that are Get Damage's own bookkeeping, not damage:
-- #15 "Sustained Damage" and #17 "Last Damage Taken" hold the hit's DRS
-- (thousands). Never sent (the server drops them too).
local BOOKKEEPING = { [16] = true, [18] = true }
-- Server answers to our claims arrive as typed `damage_verdict` S2G records
-- (kind CONFIRM / FINAL / CLASH, code = S.ENUMS.damage_reason).
local QUALITY_S = 5            -- combat_quality event period (hsmp_log)
-- A stand-in's blow on my pawn (echo) is undone in the hook's after-callback,
-- reaction included: the server-validated replay plays the
-- native reaction and the damage once. ECHO_WINDOW_S is kept for the replay's
-- reaction rule (no second flinch within it after an earlier replay).
local ECHO_WINDOW_S = 0.5
-- Hit-reaction state Get Damage writes synchronously (Willie_BP decompile).
-- Restored with the damage fields wherever a damage call
-- must leave no trace (an echo on my pawn, a stand-in's fx replay): the
-- tonus values feed "Control Physical Animation Str" on the next tick.
local REACT = {
    "Was Just Touched", "All Body Tonus", "Head Tonus", "Arm R Tonus", "Arm L Tonus",
    "Upper Body Tonus", "Leg R Tonus", "Leg L Tonus", "Consciousness Cap", "Get Up Rate",
    "Angelic Touch", "Getup Animation State", "Camera Shake", "Force Death",
    "Current Pain Threshold", "Fear", "Pain Shock Rate", "Flinch Index", "Current Pain Grab",
    "Pain Head", "Pain Neck", "Pain Arm R", "Pain Arm L", "Pain Upper Body", "Pain Lower Body",
    "Pain Leg R", "Pain Leg L", "Ball Pain", "Liver Pain",
}
local REACT_VEC = { "Pain Stumble Immediate", "PainFlinchDirection_Latest", "Pain Wound Direction" }
local TICK_MS       = 33     -- this mod's own Lua loop on the game thread, not the server tick
local PUPPET_HP_PIN = 100.0    -- a stand-in's Health floor (it is Invulnerable; the game's own regen clamps Health to 100 every tick, so a higher pin is rewritten each frame)
local EPS           = 0.01

-- --- world guard (shared/hsmp_wg.lua) -----------------------------------------
-- A level change frees every actor, component and world-outered widget of the
-- old world, and the round reset re-opens the SAME arena (OpenLevel). Touching
-- a freed UObject afterwards (a property write, a call, even IsValid(), which
-- reads the freed object) is an access violation pcall cannot catch (seen as a
-- crash in UStruct::FindProperty <- __newindex from a delayed callback).
-- Rule for every UObject kept beyond one callback (see shared/hsmp_wg.lua):
--   * wg_check() runs first in every loop/frame callback; on a world change
--     every cache is dropped WITHOUT touching the old objects (wg_on_drop
--     handlers only reset Lua references) and the callback skips this tick;
--   * delayed one-shots capture wg_token() and test wg_same(token) first.
-- The shared guard installs the OpenLevel / OpenLevelBySoftObjectPtr pre-hooks.
-- shared/*.lua is copied into Scripts/ at deploy; fall back to the source tree.
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

local HW = load_module("hsmp_wg")
local NativeModules=load_module("native_weapon_modules")
if not HW then
    Log("FATAL: shared/hsmp_wg.lua missing - %s disabled (deploy copies shared/*.lua)", "HSMPCombat")
    return
end
local WG = HW.new({ log = Log, UEHelpers = UEHelpers })
-- Structured events (shared/hsmp_log.lua; no-op when absent):
-- combat_quality every QUALITY_S.
local HL = load_module("hsmp_log")
if HL and HL.init then pcall(HL.init, { mod = "HSMPCombat", state_dir = STATE_DIR }) end
-- HSMP-SHM facade (shared/hsmp_ipc.lua), published as the per-state global
-- HSMP_IPC; this mod's state-dir helpers route through it (transition bridge).
do
    local m = load_module("hsmp_ipc")
    if m then m.init({ mod = "HSMPCombat", state_dir = STATE_DIR, log = Log }) end
end
-- Any travel path (console "open", ServerTravel, native BP) ends in LoadMap: drop there too.
pcall(function() RegisterLoadMapPreHook(function() pcall(WG.travel, "LoadMap") end) end)
local wg_check, wg_on_drop, wg_token, wg_same = WG.check, WG.on_drop, WG.token, WG.same

-- Shared field table. The INDEX (0-based) is the wire id carried in each hit's
-- deltas — server/src/combat.rs caps indices at 24 and books index 0
-- (Health) in its authoritative death ledger. Append only.
-- kind "dec": damage lowers it; "inc": damage raises it.
-- Names are the real reflected property names
-- (docs/development/halfsword/willie_props.txt): they contain spaces.
-- CamelCase forms ("HeadHealth", ...) resolve to nothing.
local FIELDS = {
    { "Health", "dec" },                  -- 0
    { "Head Health", "dec" },             -- 1
    { "Neck Health", "dec" },             -- 2
    { "Body Upper Health", "dec" },       -- 3
    { "Body Lower Health", "dec" },       -- 4
    { "Back Health", "dec" },             -- 5
    { "Arm_R Health", "dec" },            -- 6
    { "Arm_L Health", "dec" },            -- 7
    { "Leg_R Health", "dec" },            -- 8
    { "Leg_L Health", "dec" },            -- 9
    { "Consciousness", "dec" },           -- 10
    { "Head Health (Crush)", "dec" },     -- 11
    { "Bleeding", "inc" },                -- 12
    { "Pain", "inc" },                    -- 13
    { "Blood Rate", "inc" },              -- 14
    { "Sustained Damage", "inc" },        -- 15
    { "Damage Taken", "inc" },            -- 16
    { "Last Damage Taken", "inc" },       -- 17
    { "Consciousness 2 (Legs)", "dec" },  -- 18
    -- Stamina loss from being hit, if "Get Damage" applies any, is measured
    -- on the stand-in and replayed (top-up/exact) on the victim like any
    -- other field. NOT buffered (nobuf): it can't kill, and a 100000 stamina
    -- would distort whatever the hit math does with it.
    { "Stamina", "dec", nobuf = true },   -- 19
    { "Exhaustion", "inc" },              -- 20
}

-- --- vitals stream -----------------------------------------------------------
-- Owner-authoritative: each client samples ITS OWN pawn (~15 Hz, on change,
-- deadband, 1 Hz heartbeat) into the `vitals` slot; the sidecar gates and
-- sends it (C2SVitalsFrame), the server feeds its ledger and relays; each
-- receiver's sidecar writes that peer's `peer_vitals` slot, mirrored here onto that
-- peer's stand-in. Wire order = server/src/vitals.rs NAMES. Append only.
-- { property, deadband, mirror }  mirror: "limb" (floored), "set", false.
local VITALS = {
    { "Health", 0.25, false },                 -- 0  pinned >= PUPPET_HP_PIN on stand-ins
    { "Head Health", 0.25, "limb" },           -- 1
    { "Neck Health", 0.25, "limb" },           -- 2
    { "Body Upper Health", 0.25, "limb" },     -- 3
    { "Body Lower Health", 0.25, "limb" },     -- 4
    { "Back Health", 0.25, "limb" },           -- 5
    { "Arm_R Health", 0.25, "limb" },          -- 6
    { "Arm_L Health", 0.25, "limb" },          -- 7
    { "Leg_R Health", 0.25, "limb" },          -- 8
    { "Leg_L Health", 0.25, "limb" },          -- 9
    { "Head Health (Crush)", 0.25, "limb" },   -- 10
    { "Consciousness", 0.5, "limb" },          -- 11
    { "Consciousness 2 (Legs)", 0.5, "limb" }, -- 12
    { "Stamina", 0.5, "set" },                 -- 13
    { "Exhaustion", 0.5, "set" },              -- 14
    { "Bleeding", 0.05, "set" },               -- 15
    { "Blood Rate", 0.05, "set" },             -- 16
    { "Pain", 0.25, "set" },                   -- 17
    { "Fallen Rate", 0.05, false },            -- 18 posture comes from the pose stream
}
-- Flag bits (vitals.rs F_*): Willie_BP_C booleans.
local VFLAGS = {
    { "DED", 1 }, { "Fallen", 2 }, { "Downed", 4 }, { "Headless", 8 }, { "Pain Shock", 16 },
    { "Head Broken", 32 }, { "Neck Snapped", 64 }, { "Neck Dislocated", 64 },
    { "Back Broken", 128 }, { "Spine Dislocated", 128 },
    { "Arm R Broken", 256 }, { "Arm R Dislocated", 256 },
    { "Arm L Broken", 512 }, { "Arm L Dislocated", 512 },
    { "Leg R Broken", 1024 }, { "Leg R Dislocated", 1024 },
    { "Leg L Broken", 2048 }, { "Leg L Dislocated", 2048 },
}
local VITALS_TICKS    = 2      -- sample every 2nd 33 ms tick (~15 Hz)
local VITALS_BEAT_S   = 1.0    -- heartbeat (server ledger reflection)
local DISM_TICKS      = 6      -- completed native distal ledger read at ~5 Hz
local STANDIN_FLOOR   = 1.0    -- a mirrored limb/consciousness never reaches 0 locally
local MIRROR_REASSERT = 10     -- ticks: re-assert mirrored values (native regen drifts them)
local MIRROR_DISMEMBER = true  -- hide the owner's severed bones on the stand-in (visual only)
-- Damage is solo-parity: the native replay only, no exact-delta top-up.

-- Restored from the Willie_BP CDO on every MP spawn (career-save wounds are
-- applied on spawn and normally healed in the Tavern, which MP skips).
local CDO_PATH = "/Game/Character/Blueprints/Willie_BP.Default__Willie_BP_C"
local RESTORE_FIELDS = {
    "Health", "Head Health", "Neck Health", "Body Upper Health", "Body Lower Health",
    "Back Health", "Arm_R Health", "Arm_L Health", "Leg_R Health", "Leg_L Health",
    "Head Health (Crush)", "Consciousness", "Consciousness 2 (Legs)",
    "Bleeding", "Pain", "Sustained Damage", "Damage Taken", "Last Damage Taken",
    "Stamina", "Exhaustion",
}
local RESTORE_FUNCS = {
    "Reset Sustained Damage", "Reset Last Damage Taken", "Reset Blood Bleed", "Reset Latest Complex Damage",
}

-- --- helpers ----------------------------------------------------------------

local function pv(p)   -- hook param value (RemoteUnrealParam or raw value)
    if type(p) == "userdata" or type(p) == "table" then
        local ok, v = pcall(function() return p:get() end)
        if ok then return v end
    end
    return p
end

local function pset(p, v)   -- write a hook param (PRE hook only has effect)
    if type(p) ~= "userdata" and type(p) ~= "table" then return false end
    return (pcall(function() p:set(v) end))
end

local function num(x) return tonumber(x) or 0 end

local function vec(v)
    if not v then return { 0, 0, 0 } end
    local ok, t = pcall(function() return { v.X, v.Y, v.Z } end)
    return ok and t or { 0, 0, 0 }
end

-- G2S combat records (typed: damage / touch / clash / death_report)
-- go straight to the facade's IPC.send (no line ids, no JSON). Nothing is sent
-- before the session is up (`connected`, from the session status below).
local connected = false
local my_peer_id = 0
-- "connected" = the shared liveness rule (shared/hsmp_session.lua):
-- status connected AND a fresh sidecar heartbeat. A dead sidecar's leftover
-- "connected" file must not make single-player act as MP (career heals,
-- vitals, died lines).
local HSESS = load_module("hsmp_session")   -- typed session / link records (session / link slots)
local SESS = HSESS and HSESS.new({})
-- SEND: the G2S kinds this mod sends; refused = sends IPC.send refused (bad /
-- too_big / unavailable), per kind.
local SEND = { kinds = { damage = true, touch = true, clash = true, death_report = true, replay_outcome = true }, refused = {} }
local function send_rec(kind, t)
    if not connected then return false end
    local ipc = rawget(_G, "HSMP_IPC")
    local id, err
    if ipc then id, err = ipc.send(kind, t) end
    if id == nil then
        local n = (SEND.refused[kind] or 0) + 1
        SEND.refused[kind] = n
        if n <= 5 or n % 100 == 0 then Log("send %s refused: %s (#%d)", kind, tostring(err or "no ipc"), n) end
        return false
    end
    return true
end

-- This frame's one PlayerController lookup (WG.pc()), never a
-- FindAllOf per Get Damage call; the hooks bail before calling this when no
-- stand-in exists (single-player, the menu).
local function local_pawn()
    -- A dev AI takeover deliberately unpossesses our verified pawn. Native
    -- Spawn Combatants can briefly possess another Willie during stand-in
    -- allocation; that controller pawn must not replace our body/vitals owner.
    local ai = WG.ai_pawn()
    if ai then return ai end
    local pc = WG.pc()
    if not pc or not pc:IsValid() then return nil end
    local p = pc.Pawn
    if p and p:IsValid() then return p end
    return nil
end

local function body_mesh(w)
    for _, k in ipairs({ "SK_Skeleton", "Mesh" }) do
        local m; pcall(function() m = w[k] end)
        if m and m:IsValid() then return m end
    end
    return nil
end

local function bone_pos(mesh, bone)
    if not mesh or not bone or bone == "" then return nil end
    local ok, l = pcall(function() return mesh:GetSocketLocation(FName(bone)) end)
    if ok and l then return { l.X, l.Y, l.Z } end
    return nil
end

-- Bone frames: { p = world position, q = unit quaternion {x, y, z, w}, s = scale }.
local BF = { LOCAL = 64, WEAPON = 128, FIST = 262144, LEFT = 524288, RIGHT = 1048576, COMPONENT = 2097152, FEET = 33554432,
    GD_GATE_MS = 200, MAX_PER_BONE_TICK = 4 }
local CuttingBox = load_module("cutting_box")
local function bone_index(mesh, bone) return mesh:GetBoneIndex(FName(bone)) end
function BF.of(mesh, bone)
    if not mesh or not bone or bone == "" then return nil end
    -- GetSocketTransform silently answers the component's own transform for a name
    -- the skeleton lacks: never let that stand in for a bone frame.
    local okb, bi = pcall(bone_index, mesh, bone)
    if okb and type(bi) == "number" and bi < 0 then return nil end
    local f
    pcall(function()
        local t = mesh:GetSocketTransform(FName(bone), 0)
        local p, q = t.Translation, t.Rotation
        local s = 1
        pcall(function() s = tonumber(t.Scale3D.X) or 1 end)
        local x, y, z, w = tonumber(q.X), tonumber(q.Y), tonumber(q.Z), tonumber(q.W)
        local n = x and y and z and w and math.sqrt(x * x + y * y + z * z + w * w) or 0
        if n > 1e-6 and s > 1e-3 then
            f = { p = { p.X, p.Y, p.Z }, q = { x / n, y / n, z / n, w / n }, s = s }
        end
    end)
    return f
end
-- v rotated by q (FQuat::RotateVector); inv = by its conjugate.
function BF.rot(q, v, inv)
    local qx, qy, qz, qw = q[1], q[2], q[3], q[4]
    if inv then qx, qy, qz = -qx, -qy, -qz end
    local tx = 2 * (qy * v[3] - qz * v[2])
    local ty = 2 * (qz * v[1] - qx * v[3])
    local tz = 2 * (qx * v[2] - qy * v[1])
    return { v[1] + qw * tx + (qy * tz - qz * ty),
             v[2] + qw * ty + (qz * tx - qx * tz),
             v[3] + qw * tz + (qx * ty - qy * tx) }
end
function BF.to_local(f, loc)
    local d = BF.rot(f.q, { loc[1] - f.p[1], loc[2] - f.p[2], loc[3] - f.p[3] }, true)
    return { d[1] / f.s, d[2] / f.s, d[3] / f.s }
end
function BF.to_world(f, off)
    local d = BF.rot(f.q, { off[1] * f.s, off[2] * f.s, off[3] * f.s })
    return { f.p[1] + d[1], f.p[2] + d[2], f.p[3] + d[3] }
end

-- After a Deal Complex Damage call on `w`: did it pass the function's contact
-- gate (|Hit Impulse|·(Cutting Power + 1) >= Last Complex Damage Impulse, or
-- a new bone)? A pass stores this call's value and bone; a stopped call
-- leaves a stronger value on the same bone. nil = gate state unreadable.
function BF.gate_passed(w, bone, imp, cp)
    local last, lbone
    pcall(function() last = tonumber(w["Last Complex Damage Impulse"]) end)
    pcall(function() lbone = w["Last Complex Damage Bone"]:ToString() end)
    if last == nil or type(lbone) ~= "string" then return nil end
    local b = ""
    pcall(function() b = bone:ToString() end)
    if lbone:lower() ~= b:lower() then return true end
    local v = vec(imp)
    local g = math.sqrt(v[1] * v[1] + v[2] * v[2] + v[3] * v[3]) * (num(cp) + 1)
    return last <= g * (1 + 1e-6) + 1e-6
end

-- A striking component whose owner is not a Willie (a weapon actor), or one
-- tagged 'Weapon'.
function BF.is_weapon(comp)
    if not comp then return false end
    local tagged = false
    local ok, live = pcall(function() return comp:IsValid() end)
    if not ok or not live then return false end
    pcall(function() tagged = comp:ComponentHasTag(FName("Weapon")) == true end)
    if tagged then return true end
    local cls
    pcall(function()
        -- GetOwner can return a truthy UE4SS wrapper around nullptr after a
        -- weapon drop/destruction. pcall cannot catch GetClass's native AV.
        local owner = comp:GetOwner()
        if owner and owner:IsValid() then cls = owner:GetClass():GetFName():ToString() end
    end)
    return cls ~= nil and cls ~= "Willie_BP_C"
end

-- Blueprint call by its REAL name (BP names keep their spaces, e.g.
-- "Get Damage"; the CamelCase form silently resolves to nothing). Logs once
-- per name whether it resolved and how the first call went.
local bp_logged = {}
local function bp_call(obj, name, ...)
    local fn
    local okf = pcall(function() fn = obj[name] end)
    if not okf or fn == nil then
        if not bp_logged[name] then
            bp_logged[name] = true
            Log("BP function '%s' did NOT resolve", name)
        end
        return false, "unresolved"
    end
    local args = table.pack(...)
    local ok, err = pcall(function() fn(obj, table.unpack(args, 1, args.n)) end)
    if not bp_logged[name] then
        bp_logged[name] = true
        Log("BP function '%s' resolved; first call %s", name, ok and "ok" or ("failed: " .. tostring(err)))
    end
    return ok, err
end

local function wname(w)
    local nm; pcall(function() nm = w:GetFName():ToString() end)
    return nm
end

-- Damage-field snapshot / restore.
local function snapshot(w)
    local s = {}
    for i, f in ipairs(FIELDS) do
        local v; pcall(function() v = tonumber(w[f[1]]) end)
        s[i] = v   -- nil if the field isn't readable on this build
    end
    return s
end

local function restore(w, s)
    for i, f in ipairs(FIELDS) do
        if s[i] ~= nil then pcall(function() w[f[1]] = s[i] end) end
    end
end

-- Per-field change b - a, only fields that moved.
local function diff(a, b)
    local d = {}
    for i = 1, #FIELDS do
        if a[i] and b[i] then
            local v = b[i] - a[i]
            if math.abs(v) > EPS then d[i] = v end
        end
    end
    return d
end

local function fmt_fields(d, scale)
    local parts = {}
    for i = 1, #FIELDS do
        if d[i] then table.insert(parts, string.format("%s%+.1f", FIELDS[i][1], d[i] * (scale or 1))) end
    end
    return #parts > 0 and table.concat(parts, " ") or "none"
end

local _in_game, _in_game_tick, tick_num = false, -999, 0
local function in_gameplay()
    if tick_num - _in_game_tick < 30 then return _in_game end
    _in_game_tick = tick_num
    _in_game = false
    pcall(function()
        local w = WG.world()   -- this frame's PlayerController, no second walk
        if w and w:IsValid() then _in_game = w:GetFullName():find("Map_Menu_") == nil end
    end)
    return _in_game
end

-- New typed S2G records of one kind (array of {req_id, data, peer, aux}; the
-- cursor is this Lua state's).
local function events(kind)
    local ipc = rawget(_G, "HSMP_IPC")
    return (ipc and ipc.events(kind)) or {}
end

-- --- stand-in registry (bus key "puppets" from HSMPAvatars) ----------------

local puppet_peer = {}     -- actor FName -> peer_id (strings only)
local puppet_actor = {}    -- peer_id -> actor (world-scoped: dropped by the world guard)
local puppet_weapon = {}   -- held weapon actor FName -> peer_id (strings only)
local _puppets_raw = nil

-- Rebuilt from a fresh FindAllOf on every call (~2 Hz). Cached actors are
-- never re-validated with IsValid(): a stand-in destroyed by HSMPAvatars (or
-- freed by a level change) may already be gone, and IsValid reads the object.
local function refresh_puppets()
    -- HSMPAvatars' stand-ins (bus key "puppets": typed rows { peer, name = "<actor FName>" }).
    local ipc = rawget(_G, "HSMP_IPC")
    local pt = ipc and ipc.bus_table("puppets")
    if type(pt) ~= "table" then return end   -- never written: keep the last set
    _puppets_raw = pt
    puppet_peer, puppet_actor, puppet_weapon = {}, {}, {}
    for _, r in ipairs(pt.rows or {}) do
        if r.peer and r.peer ~= 0 and type(r.name) == "string" and r.name ~= "" then puppet_peer[r.name] = r.peer end
    end
    if next(puppet_peer) == nil then return end
    pcall(function()
        local ws = FindAllOf("Willie_BP_C")
        if not ws then return end
        for _, w in pairs(ws) do
            if w and w:IsValid() then
                local id = puppet_peer[wname(w) or ""]
                if id then
                    puppet_actor[id] = w
                    -- Its held weapons, so a stand-in's blade is recognised as
                    -- the stand-in's even when the weapon's "Parent Actor" /
                    -- attach parent doesn't resolve (else its local echo on
                    -- my pawn would NOT be zeroed: double damage).
                    for _, k in ipairs({ "Weapon R", "Weapon L", "Foot R Weapon", "Foot L Weapon" }) do
                        pcall(function()
                            local wp = w[k]
                            if wp and wp:IsValid() then puppet_weapon[wname(wp) or ""] = id end
                        end)
                    end
                end
            end
        end
    end)
    puppet_weapon[""] = nil
end

-- --- source attribution -----------------------------------------------------

-- UE4SS userdata wrappers aren't guaranteed to be == for the same UObject.
local function same(a, b)
    if not a or not b then return false end
    if rawequal(a, b) then return true end
    local ok, r = pcall(function() return a:GetAddress() == b:GetAddress() end)
    if ok then return r end
    local ok2, r2 = pcall(function() return a:GetFullName() == b:GetFullName() end)
    return ok2 and r2 or false
end

-- Source identity travels separately from WEAPON: fists are native weapon
-- components for replay, but their geometry is the replicated hand/body.
function BF.source(comp, pawn)
    local owner, cls
    pcall(function()
        if not (comp and comp:IsValid()) then return end
        owner = comp:GetOwner()
        if not (owner and owner:IsValid()) then owner = nil; return end
        cls = owner:GetClass():GetFName():ToString()
    end)
    if not owner then return 0 end
    local bits = cls and cls:find("Fists", 1, true) and BF.FIST or 0
    local feet = cls and cls:find("Weapon_Feet", 1, true) ~= nil
    if feet then bits = bits + BF.FEET end
    local r, l
    pcall(function()
        if feet then r, l = pawn["Foot R Weapon"], pawn["Foot L Weapon"]
        else r, l = pawn["Weapon R"], pawn["Weapon L"] end
    end)
    if same(owner, r) then bits = bits + BF.RIGHT
    elseif same(owner, l) then bits = bits + BF.LEFT end
    if feet and not same(owner, r) and not same(owner, l) then return bits, nil, "unknown_foot_owner" end
    if not feet and not same(owner, r) and not same(owner, l) then return bits, nil, "unknown_weapon_owner" end
    local ordinal, reason, arr
    local read_ok = pcall(function() arr = owner["Collision Components Array"] end)
    if not read_ok or not arr then return bits, nil, "missing_array" end
    local scanned = pcall(function()
        local n = 0
        arr:ForEach(function(_, entry)
            n = n + 1 -- Count invalid entries too: identity is the native array slot.
            if same(entry:get(), comp) then
                if n <= 15 then ordinal = n else reason = "unsupported_index" end
            end
        end)
    end)
    if not scanned then reason = "unreadable_array" end
    return bits + (ordinal or 0) * BF.COMPONENT, ordinal, reason or (not ordinal and "component_not_listed" or nil)
end

-- HitBox is a distinct optional native cutting input. Never substitute the
-- striking collider for it, or resolve a box from the other held weapon.
function BF.hit_box(box, collided, pawn)
    if not box then return 0 end
    local valid, same_owner, is_box = false, false, false
    pcall(function()
        valid = box:IsValid()
        if valid then
            same_owner = same(box:GetOwner(), collided:GetOwner())
            is_box = box:GetClass():GetFName():ToString() == "BoxComponent"
        end
    end)
    if not valid then return 0 end
    if not same_owner or not is_box then return nil end
    if not NativeModules then return nil end
    local _,ordinal=BF.source(collided,pawn)
    if not ordinal then return nil end
    return NativeModules.box(collided:GetOwner(),collided,box)
end

-- The Willie holding a weapon actor. Real property name "Parent Actor"
-- (with a space): the CamelCase `.ParentActor` resolves to nil, which would
-- attribute every sword hit to the weapon instead of its wielder.
local function weapon_parent(a)
    local parent
    pcall(function() parent = a["Parent Actor"] end)
    if parent and parent:IsValid() then return parent end
    pcall(function()
        if a:GetClass():GetFName():ToString() == "Willie_BP_C" then return end
        local pa = a:GetAttachParentActor()
        if pa and pa:IsValid() and pa:GetClass():GetFName():ToString() == "Willie_BP_C" then parent = pa end
    end)
    if parent and parent:IsValid() then return parent end
    return nil
end

-- Actor responsible for a hit component: the component's owner, or — when
-- the owner is a weapon — the Willie holding it.
local function hit_source(comp)
    if not comp then return nil end
    local ok, owner = pcall(function() return comp:GetOwner() end)
    if not ok or not owner or not owner:IsValid() then return nil end
    return weapon_parent(owner) or owner
end

local function source_standin_peer(comp)
    local src = hit_source(comp)
    if not src then return nil end
    local nm = wname(src) or ""
    return puppet_peer[nm] or puppet_weapon[nm]
end

-- --- lag-compensation timestamps -------------------------------------------

-- Same clock as HSMPSync's ts and HSMPAvatars' now_ms (MSVC clock(): wall ms).
local function now_ms() return math.floor(os.clock() * 1000) end

-- Bus key "playback" (typed rows {peer, body_ts, arm_ts, local_ms}) rewritten by
-- HSMPAvatars every tick. Read at most once per tick, only when needed.
local playback, playback_tick = {}, -1
local function playback_ts(peer)
    if playback_tick ~= tick_num then
        playback_tick = tick_num
        local ipc = rawget(_G, "HSMP_IPC")
        local pb = ipc and ipc.bus_table("playback")
        if type(pb) == "table" then
            local t = {}
            for _, r in ipairs(pb.rows or {}) do
                if r.peer and r.peer ~= 0 then t[r.peer] = { r.body_ts, r.arm_ts, r.local_ms } end
            end
            playback = t
        else
            playback = {}   -- never written / cleared at the world leave: nothing shown
        end
    end
    local e = playback[peer]
    if not e then return 0, 0 end
    -- Advance the (≤ one tick old) sample to now: playback runs at real time.
    local adv = math.max(0, now_ms() - e[3])
    if adv > 250 then return 0, 0 end   -- HSMPAvatars stalled: no claim
    return math.max(0, e[1] + adv), math.max(0, e[2] + adv)
end

-- --- hooks ------------------------------------------------------------------

-- Server round state (the typed session snapshot, from S2CMatchState).
-- Hits only count while the round is live: outside it stand-ins neither take
-- nor deal damage, nothing is sent, and received hits are acked-and-dropped
-- (the sidecar already acked them on receipt).
local match_live, match_round, match_state = false, -1, "lobby"
local match_gen = 0
local last_death_mid = nil   -- match_id of the last `death` record
local round_key   -- defined below refresh_match
local combat_window = false
local function refresh_match()
    -- The typed session snapshot (slot `session`; HS.view()): state = the legacy match state.
    local v = HSESS and HSESS.view()
    if not v then match_live, combat_window = false, false; match_state = "lobby"; return end
    local prev_state, prev_round = match_state, match_round
    match_state = v.state or match_state
    match_live = match_state == "live"
    -- Hits still count while a finished round settles on the server (trade
    -- window); the server decides with round tags + its own state.
    combat_window = match_live or match_state == "roundover"
    match_round = math.tointeger(v.round) or -1
    -- Rounds restart at 1 in every match. A new match context starts
    -- when the state falls back to the lobby after a match, or the round goes
    -- down; round bookkeeping is keyed "<match_gen>:<round>".
    if (prev_state ~= "lobby" and match_state == "lobby") or (match_round >= 0 and match_round < prev_round) then
        match_gen = match_gen + 1
    end
end
function round_key(r) return string.format("%d:%d", match_gen, r or -1) end

local replaying = false   -- true while WE call a damage function (own replay, stand-in fx)
local sent_count = 0
local zeroed_count = 0         -- echoes on my pawn undone
local echo_stamina_fixes = 0   -- (kept for the 5 s vitals log line)
local echo_reactions = 0       -- (kept for the test api; echoes are undone, not played)
local last_echo = {}           -- peer_id -> os.clock() of its last echo on my pawn

-- Reaction-state snapshot / restore (numbers, booleans; vectors by component).
local function snap_react(w)
    local s = {}
    for _, k in ipairs(REACT) do
        local v; pcall(function() v = w[k] end)
        local t = type(v)
        if t == "number" or t == "boolean" then s[k] = v end
    end
    for _, k in ipairs(REACT_VEC) do
        pcall(function()
            local v = w[k]
            if v then s[k] = { tonumber(v.X) or 0, tonumber(v.Y) or 0, tonumber(v.Z) or 0 } end
        end)
    end
    return s
end

local function put_react(w, s)
    for k, v in pairs(s) do
        if type(v) == "table" then
            local ok = pcall(function() w[k] = { X = v[1], Y = v[2], Z = v[3] } end)
            if not ok then pcall(function() local o = w[k]; o.X, o.Y, o.Z = v[1], v[2], v[3] end) end
        else
            pcall(function() w[k] = v end)
        end
    end
end

-- ===== what the UE4SS hooks really see =========================================
-- RegisterHook on a Blueprint function (any path not under /Script/) runs its
-- callback AFTER the function body executed; a third ("post") callback is
-- never called (ue4ss/Docs/lua-api/global-functions/registerhook.md). So a
-- "PRE" hook cannot rewrite params (zero an echo, buffer a stand-in's
-- health), and the Get Damage callback runs before the Deal Complex Damage
-- one. The design therefore is:
--   * a hit is the Deal Complex Damage call on a stand-in whose Collided
--     Component is mine (weapon or body). Its inputs are read after the call
--     (input params are unchanged by a BP body) and claimed as-is; nothing is
--     measured on the stand-in. The victim replays the call natively;
--   * stand-ins take no native damage: "Invulnerable" (Get Damage's first
--     gate) and their weapons' "Temp Disable Damage" (Collision Hit's gate),
--     re-asserted every tick; anything that still lands on a stand-in is put
--     back from the tick's baseline in the after-callback;
--   * a stand-in's blow on MY pawn (an echo) is undone in the after-callback
--     from the baseline of my pawn (refreshed every tick and after every
--     legitimate hit) and reported to the server as a "touch" (evidence
--     against a parry, lagcomp `parried`). The server-approved replay is the
--     only damage my pawn takes from another player;
--   * blood, wounds and bruises on stand-ins come after actual native owner
--     injury outcomes (`hitfx_in` records), replayed natively on
--     the stand-in with its damage state restored right after.
local C3 = {
    STANDIN_INVULNERABLE = true,
    TOUCH_GAP_MS = 50,          -- per-peer touch report rate limit
    -- Bookkeeping Deal Complex Damage / Get Damage write besides FIELDS: their
    -- contact gates. An echo that left them set would gate the server's
    -- replay of the same blow (0.1-0.2 s).
    GATES = { "Last Complex Damage Impulse", "Last Hit Impulse", "Last Hit Impulse (Other Body)" },
    GATE_NAMES = { "Last Complex Damage Bone", "Last Damaged Bone" },
    base = {},                  -- actor address -> baseline (my pawn, every stand-in)
    touch_last = {},            -- peer -> now_ms of the last touch report
    touches = 0, undone = 0, standin_restores = 0, fx = 0, fx_skipped = 0,
    protect_tick = -1,
    replay_stats = {}, receipt_stats = {}, replay_fields=0, replay_hp=0, receipt_fields=0, receipt_hp=0,
}

-- Combat records waiting in the facade's retry queue (ring full /
-- sidecar not attached) belong to THEIR session; a session end or a new peer
-- id drops them (other mods' queued sends are kept).
function C3.discard_outbox(why)
    local ipc = rawget(_G, "HSMP_IPC")
    local q = ipc and ipc.retry
    if type(q) ~= "table" or #q == 0 then return 0 end
    local keep, n = {}, 0
    for _, e in ipairs(q) do
        if type(e) == "table" and SEND.kinds[e.kind] then n = n + 1 else keep[#keep + 1] = e end
    end
    if n > 0 then
        ipc.retry = keep
        Log("outbox: %d queued record(s) of the old session discarded (%s)", n, why)
    end
    return n
end

-- Life / limb state an echo must not change.
C3.LIFE = { "Force Death", "DED", "Headless" }

-- Everything a damage call may change that we put back: FIELDS, the
-- reaction state, the life flags and the contact gates.
function C3.snap_all(w)
    local s = { f = snapshot(w), r = snap_react(w), life = {}, g = {}, gn = {} }
    for _, k in ipairs(C3.LIFE) do
        local v; pcall(function() v = w[k] end)
        if type(v) == "boolean" then s.life[k] = v end
    end
    for _, k in ipairs(C3.GATES) do
        local v; pcall(function() v = tonumber(w[k]) end)
        s.g[k] = v
    end
    for _, k in ipairs(C3.GATE_NAMES) do
        local v; pcall(function() v = w[k]:ToString() end)
        s.gn[k] = v
    end
    pcall(function() s.dism_n = #w["Dismembered Array"] end)
    return s
end

-- keep_contact: leave Deal Complex Damage's contact gate as the call left it
-- (a stand-in: that gate decides which of my calls are claims, as on the
-- victim in solo).
C3.CONTACT_GATE = { ["Last Complex Damage Impulse"] = true, ["Last Complex Damage Bone"] = true }
function C3.put_all(w, s, keep_contact)
    if not s then return end
    restore(w, s.f)
    put_react(w, s.r)
    for k, v in pairs(s.life) do pcall(function() w[k] = v end) end
    for k, v in pairs(s.g) do
        if not (keep_contact and C3.CONTACT_GATE[k]) then pcall(function() w[k] = v end) end
    end
    for k, v in pairs(s.gn) do
        if not (keep_contact and C3.CONTACT_GATE[k]) then pcall(function() w[k] = FName(v) end) end
    end
end

local function addr_of(w)
    local a; pcall(function() a = w:GetAddress() end)
    return a
end

-- Baseline of `w` (my pawn or a stand-in) as of now.
function C3.baseline(w)
    local a = addr_of(w)
    if a then
        C3.base[a] = C3.snap_all(w)
        if C3.probe_after then C3.probe_after[a]=nil end
    end
end

-- Stand-ins never take native damage (see the block comment above). The
-- names of what was gated are kept (strings only): a gated weapon I pick up,
-- or a pooled Willie that becomes my pawn, gets its gate lifted (C3.ungate).
C3.gated = {}
-- Team Int: a stand-in spawns with the local player's Team Int (1), and
-- ModularWeaponBP's Collision Hit sets "Friendly Fire?" for the same non-zero
-- team: Cutting Rate 0 (no cut or stab, only blunt) and Hit Velocity /
-- Impulse x0.1, so every blow would be claimed as a weak blunt touch. A
-- stand-in therefore gets its own team, the same as mine only when the
-- server says we are teammates (session roster "team").
C3.team_of = {}            -- peer -> server team (0 = none), from the session roster
C3.my_team = 0
function C3.read_teams()
    local v = HSESS and HSESS.view()   -- the typed session snapshot: roster rows
    if not v then return end
    local t = {}
    for _, r in ipairs(v.rows) do
        if r.connected and r.peer_id ~= 0 then
            t[r.peer_id] = r.team or 0
            if v.my_seat ~= nil and r.seat == v.my_seat then C3.my_team = t[r.peer_id] end
        end
    end
    C3.team_of = t
end
function C3.team_for(peer, mine)
    local pt = C3.team_of[peer] or 0
    if pt ~= 0 and pt == C3.my_team then return mine end   -- a real MP teammate: friendly fire as in solo
    local t = 100 + (peer % 100)
    if t == mine then t = t + 100 end
    return t
end
function C3.protect(w, peer)
    -- Native DCD @4356/@9789 paints armour before Get Damage's Invulnerable
    -- gate. This boolean suppresses speculative paint without changing the
    -- native damage/contact computation (including the dev probe).
    pcall(function() w["Force Disable Vertex Paint"] = true end)
    if peer then
        pcall(function()
            local me = local_pawn()
            local mine = me and tonumber(me["Team Int"]) or 0
            local want = C3.team_for(peer, mine)
            if tonumber(w["Team Int"]) ~= want then w["Team Int"] = want end
        end)
    end
    if C3.STANDIN_INVULNERABLE then
        -- dev `combat_probe`: the game's own Get Damage runs on the stand-in (and is put back,
        -- C3.standin_hit) so each blow's native result can be logged beside its replay
        local inv = not C3.native_probe
        pcall(function() if w.Invulnerable ~= inv then w.Invulnerable = inv end end)
        pcall(function() w["Force Disable Dismemberment"] = true end)
        local nm = wname(w)
        if nm then C3.gated[nm] = true end
    end
    for _, k in ipairs({ "Weapon R", "Weapon L", "Foot R Weapon", "Foot L Weapon" }) do
        pcall(function()
            local wp = w[k]
            if wp and wp:IsValid() then
                if wp["Temp Disable Damage"] ~= true then wp["Temp Disable Damage"] = true end
                local wn = wname(wp)
                if wn then C3.gated[wn] = true end
            end
        end)
    end
end

-- My pawn or a weapon in my hands that we gated as a stand-in's: lift it
-- (once; the game's own short "Temporary Disable Damage" is left alone).
function C3.ungate(me)
    local nm = wname(me)
    if nm and C3.gated[nm] then
        C3.gated[nm] = nil
        pcall(function() me.Invulnerable = false end)
        -- Avatars/cosmetic replay harden stand-ins structurally as well. A
        -- pooled stand-in becoming OUR real pawn must regain native severing.
        -- Only touch a pawn recorded as ours to gate, never normal spawn flags.
        pcall(function() me["Force Disable Dismemberment"] = false end)
        pcall(function() me["Force Disable Vertex Paint"] = false end)
        Log("my pawn %s was a stand-in: Invulnerable and dismemberment guards lifted", nm)
    end
    for _, k in ipairs({ "Weapon R", "Weapon L", "Foot R Weapon", "Foot L Weapon" }) do
        pcall(function()
            local wp = me[k]
            local wn = wp and wp:IsValid() and wname(wp)
            if wn and C3.gated[wn] then
                C3.gated[wn] = nil
                wp["Temp Disable Damage"] = false
                Log("weapon %s (a stand-in's) is in my hands now: damage gate lifted", wn)
            end
        end)
    end
end

-- "touch": the stand-in of `peer` reached my body on my screen (C2STouch).
function C3.touch(peer)
    if not (combat_window and WG.settled()) then return end
    local now = now_ms()
    if C3.touch_last[peer] and now - C3.touch_last[peer] < C3.TOUCH_GAP_MS then return end
    C3.touch_last[peer] = now
    local _, arm_ts = playback_ts(peer)
    if arm_ts <= 0 then return end
    if send_rec("touch", { other_peer_id = peer, my_ts = now, other_ts = arm_ts }) then
        C3.touches = C3.touches + 1
    end
end

-- A stand-in's blow landed on my pawn (its body / a weapon whose damage gate
-- leaked): undo it from my baseline, report the touch.
function C3.echo(me, peer)
    local b = C3.base[addr_of(me)]
    if b then C3.put_all(me, b) end
    last_echo[peer] = os.clock()
    zeroed_count = zeroed_count + 1
    if zeroed_count <= 5 or zeroed_count % 50 == 0 then
        Log("stand-in %d blow on my pawn undone%s; the server-approved hit is the damage (#%d)",
            peer, b and "" or " (NO baseline yet)", zeroed_count)
    end
    local n; pcall(function() n = #me["Dismembered Array"] end)
    if b and b.dism_n and n and n > b.dism_n then
        Log("WARNING: a stand-in echo on my pawn dismembered %d part(s) (cannot be undone)", n - b.dism_n)
    end
    C3.touch(peer)
end

-- Something landed on a stand-in outside our own replay: put it back.
function C3.standin_hit(w)
    local b = C3.base[addr_of(w)]
    if not b then return end
    local now = snapshot(w)
    -- Only DAMAGE counts (a "dec" field down, an "inc" field up by > 0.5):
    -- the game's own regen and our mirror move the other way between ticks
    -- (the Health regen clamp alone would log a false "put back" every tick).
    local hurt = false
    for i, fl in ipairs(FIELDS) do
        local a, c = b.f[i], now[i]
        if a and c and ((fl[2] == "dec" and c < a - 0.5) or (fl[2] == "inc" and c > a + 0.5)) and not BOOKKEEPING[i] then
            hurt = true; break
        end
    end
    if not hurt then return end   -- Invulnerable held: nothing to undo
    -- (runs inside the Deal Complex Damage call too, before its callback reads
    -- the contact gate)
    C3.put_all(w, b, true)
    C3.standin_restores = C3.standin_restores + 1
    if C3.standin_restores <= 5 or C3.standin_restores % 50 == 0 then
        Log("native damage on stand-in %s put back (#%d; Invulnerable did not hold)", wname(w) or "?", C3.standin_restores)
    end
end

-- POST-only native evidence: measure against the saved stand-in baseline,
-- before the backstop restores it. Missing baselines/fields are unavailable,
-- never an invented zero. Each Get Damage sample is consumed by one path.
function C3.probe_measure(w)
    if not C3.native_probe then return nil end
    local baseline=C3.base[addr_of(w)]
    if not baseline then return "dmg Health unavailable [native baseline absent]" end
    -- Tiny native changes can stay below the backstop's 0.5 threshold. Use
    -- the previous callback's actual post-restore state within this tick,
    -- so a second call cannot count the first one's measured change again.
    local previous=C3.probe_after and C3.probe_after[addr_of(w)]
    if previous and previous.tick==tick_num then baseline=previous end
    local current=snapshot(w)
    if not (baseline.f[1] and current[1]) then return "dmg Health unavailable [native Health absent]" end
    return string.format("dmg Health %+.6f [%s]",current[1]-baseline.f[1],fmt_fields(diff(baseline.f,current)))
end

-- Developer-only plain scalar evidence. The six DCD outputs are reflected
-- enum/double/bool params, never armour inventory guesses or SoftObject values.
function C3.native_scalar(v)
    if type(v)=="boolean" then return tostring(v) end
    if type(v)=="number" and v==v and math.abs(v)<math.huge then return string.format("%.17g",v) end
    return "unavailable"
end
function C3.native_zone(w)
    local value;pcall(function() value=w["Last Hit Body Part"] end)
    return C3.native_scalar(value)
end
function C3.dcd_evidence(args,hook)
    local names={"Hit Surface","Damage Out","Cutting Rate Out","Rigidity Out","Material Density Out","Lower Threshold Out"}
    local keys={"surface","damage","cp","rigidity","density","lower"}
    local rows={}
    for i,name in ipairs(names) do
        local value
        if hook then value=pv(args[i]) else
            -- UE4SS copies out params into named fields of supplied tables.
            -- Search all six slots; absent/copy-out-failed stays unavailable.
            for slot=18,23 do
                local out=args[slot]
                if type(out)=="table" and out[name]~=nil then value=out[name];break end
            end
        end
        rows[#rows+1]="dcd_"..keys[i].."="..C3.native_scalar(value)
    end
    return table.concat(rows," ")
end
function C3.gd_evidence(w,bone,raw,cut,draw,pain,inside,lower,applied)
    local b;pcall(function() b=pv(bone):ToString() end)
    return string.format("bone:%s,zone:%s,raw:%s,cp:%s,draw:%s,pain:%s,inside:%s,lower:%s,out:%s",
        tostring(b or "unavailable"),C3.native_zone(w),C3.native_scalar(pv(raw)),C3.native_scalar(pv(cut)),
        C3.native_scalar(pv(draw)),C3.native_scalar(pv(pain)),C3.native_scalar(pv(inside)),
        C3.native_scalar(pv(lower)),C3.native_scalar(pv(applied)))
end
function C3.log_native_evidence(side,d,attacker,detail,cid)
    if not C3.native_probe then return end
    Log("LAB_NATIVE side=%s attacker=%s target=%s cid=%s parent_cid=%s match=%s round=%s attacker_life=%s victim_life=%s bone=%s %s",
        side,tostring(attacker),tostring(d.target_peer_id or d.peer or my_peer_id),tostring(cid or d.hit_id or d.cid),
        tostring(d.parent_cid or 0),tostring(d.match_id),tostring(d.round),tostring(d.attacker_life),tostring(d.victim_life),
        tostring(d.bone),detail or "native_evidence=unavailable")
end

-- POST armour construction: installed proxy meshes/tags/passports, separate
-- from the layers crossed by a particular DCD trace. Disabled probes read no
-- parameters; the hook owns no UObject cache across callbacks/worlds.
C3.armor_audit = load_module("native_protection_audit")
if C3.armor_audit then
    C3.armor_audit=C3.armor_audit.new({
        enabled=function() return C3.native_probe and WG.check() and WG.settled() end,
        unwrap=pv,log=Log,
        context=function(w)
            local own_pawn=local_pawn()
            local peer=own_pawn and same(w,own_pawn) and my_peer_id or puppet_peer[wname(w) or ""]
            local ctx=peer and (peer==my_peer_id and C3.life_for(peer,true) or C3.displayed_for(peer,w)) or nil
            return {peer=peer,match_id=ctx and ctx.match_id,round=ctx and ctx.round,life=ctx and ctx.life}
        end,
    })
end

-- Separate read-only body probe: enabling it never makes stand-ins take
-- damage. Native BP hooks are POST-only; replay invocation snapshots are
-- labelled separately from the unavailable nested Get Damage PRE state.
function C3.body_audit_context(w)
    if not w or not same(w,local_pawn()) then return nil end
    local ctx=C3.vitals_context and C3.vitals_context(w)
    if not ctx then
        -- Preparation telemetry may describe the assigned owner while
        -- placement is in progress. It authorizes no damage or publication.
        local ipc=rawget(_G,"HSMP_IPC")
        local status=ipc and ipc.bus_table and ipc.bus_table("spawn_status")
        local view=HSESS and HSESS.view and HSESS.view()
        local order=view and view.spawns and view.spawns[my_peer_id]
        if not status or not view or not order or status.pawn~=wname(w)
            or status.match_id~=view.match_id or status.spawn_id~=order.spawn_id
            or (view.state~="loading" and view.state~="countdown")
            or status.round~=view.pending_round or status.round~=view.spawn_round or status.life~=1 then return nil end
        ctx={match_id=status.match_id,round=status.round,life=status.life}
    end
    local world=WG.world()
    local mesh;pcall(function()mesh=w.Mesh end)
    if not world or not world:IsValid() or not mesh or not mesh:IsValid() then return nil end
    return {world=tostring(world:GetAddress()).."@"..world:GetFullName(),peer=my_peer_id,
        match_id=ctx.match_id,round=ctx.round,life=ctx.life,pawn=wname(w),actor=addr_of(w),mesh=addr_of(mesh)}
end
-- Sever callbacks also describe source-native attempts on displayed stand-ins.
-- This diagnostic admission grants no damage, publication or readiness authority.
function C3.body_sever_context(w)
    if not WG.check() or not WG.settled() or not w or not w:IsValid() then return nil end
    local ctx,peer,side
    if same(w,local_pawn()) then
        ctx,peer,side=C3.body_audit_context(w),my_peer_id,"owner"
    else
        local name=wname(w)
        peer=name and puppet_peer[name]
        if not peer or peer==my_peer_id then return nil end
        ctx,side=C3.displayed_for(peer,w,true),"source"
    end
    if not ctx then return nil end
    local world=WG.world()
    local mesh,actor_world,mesh_world
    local ok=pcall(function()mesh=w.Mesh;actor_world=w:GetWorld();mesh_world=mesh:GetWorld()end)
    if not ok or not world or not world:IsValid() or not mesh or not mesh:IsValid()
        or not actor_world or not actor_world:IsValid() or not mesh_world or not mesh_world:IsValid()
        or not same(world,actor_world) or not same(world,mesh_world)
        or actor_world:GetFullName()~=world:GetFullName() or mesh_world:GetFullName()~=world:GetFullName() then return nil end
    return {world=tostring(world:GetAddress()).."@"..world:GetFullName(),peer=peer,
        match_id=ctx.match_id,round=ctx.round,life=ctx.life,pawn=wname(w),actor=addr_of(w),mesh=addr_of(mesh),side=side}
end
C3.body_audit=load_module("native_body_audit")
C3.topology_audit=load_module("native_topology_audit")
if C3.body_audit then
    C3.body_audit=C3.body_audit.new({
        enabled=function()return os.getenv("HSMP_DEV")=="1" and C3.body_probe
            and WG.check() and WG.settled() end,
        unwrap=pv,fname=FName,log=Log,context=C3.body_audit_context,source_context=C3.body_sever_context,
        topology_reader=C3.topology_audit,
    })
end
function C3.body_replay_meta(d,attacker)
    return {attacker=attacker,hit_id=d.hit_id,cid=d.cid,parent_cid=d.parent_cid,
        match_id=d.match_id,round=d.round,victim_life=d.victim_life,
        attacker_life=d.attacker_life,source_class=d.source_class,source_meta=d.dism_blunt,bone=d.bone,
        pre="fresh:owner_replay_invocation"}
end

-- Independent read-only armor trace bursts. This never enables stand-in
-- damage; the existing native trace callback supplies already computed hits.
C3.armor_trace_module=load_module("native_armor_trace")
if C3.armor_trace_module and C3.armor_audit then
    local function enabled()
        return os.getenv("HSMP_DEV")=="1" and WG.check() and WG.settled()
    end
    local formatter=load_module("native_protection_audit").new({enabled=enabled,unwrap=pv,log=Log})
    C3.armor_trace=C3.armor_trace_module.new({enabled=enabled,clock=now_ms,unwrap=pv,
        context=C3.body_sever_context,format_trace=formatter.trace,log=Log,
        invocation=function(w,ctx)
            local meta,trace=C3.body_replay,C3.replay_trace
            if not replaying or not meta or not trace or ctx.side~="owner"
                or trace.pawn~=ctx.actor or addr_of(w)~=trace.pawn
                or meta.match_id~=ctx.match_id or meta.round~=ctx.round
                or meta.victim_life~=ctx.life then return nil end
            local context={};for k,v in pairs(ctx)do context[k]=v end
            local out={pawn=trace.pawn,context=context}
            for k,v in pairs(meta)do out[k]=v end
            return out
        end})
end

-- Pinned UE4SS native POST hooks pass context, then ReturnValue, then
-- reflected parameters. Blueprint hooks use a different argument ordering.
function C3.native_trace_post(_,returnedp,worldp,startp,endp,radiusp,objectsp,complexp,ignoreactorsp,debugp,hitsp,ignoreselfp,colorp,hitcolorp,timep)
    if C3.armor_trace then
        pcall(C3.armor_trace.capture,returnedp,worldp,startp,endp,radiusp,objectsp,complexp,hitsp,ignoreselfp)
    end
    if not C3.native_probe or not replaying or not C3.replay_trace or not C3.armor_audit then return end
    if not WG.check() or not WG.settled() then return end
    local world_context=pv(worldp)
    local trace=C3.replay_trace
    if not world_context or not world_context:IsValid() or addr_of(world_context)~=trace.pawn then return end
    trace.proxy_trace_calls=(trace.proxy_trace_calls or 0)+1
    trace.proxy_trace_samples=trace.proxy_trace_samples or {}
    if #trace.proxy_trace_samples<8 then
        trace.proxy_trace_samples[#trace.proxy_trace_samples+1]=C3.armor_audit.trace(startp,endp,radiusp,objectsp,complexp,hitsp,ignoreselfp,returnedp)
            or "native_trace=unavailable"
    else trace.proxy_trace_truncated=true end
end

-- The owner's passport body on stand-ins (standin_body.lua): my own body into
-- the `body2` slot, each peer's `peer_body2` onto its proven stand-in life.
C3.BODY = load_module("standin_body")
function C3.body_context(me)
    local ipc = rawget(_G,"HSMP_IPC")
    local status = ipc and ipc.bus_table and ipc.bus_table("spawn_status")
    local view = HSESS and HSESS.view and HSESS.view()
    local order = view and view.spawns and view.spawns[my_peer_id]
    if not me or not view or not status or status.verified ~= true or status.pawn ~= wname(me)
        or status.match_id ~= view.match_id or not order or status.spawn_id ~= order.spawn_id then return nil end
    if view.state == "countdown" then
        if status.round ~= view.round + 1 or status.life ~= 1 then return nil end
    else
        local expected = C3.life_for(my_peer_id,true)
        if not expected or status.round ~= expected.round or status.life ~= expected.life then return nil end
    end
    -- Copy original placement facts together; Mode is solely an admission check.
    return {match_id=status.match_id,round=status.round,life=status.life,pawn=status.pawn}
end
function C3.body_publish(me, ctx)
    local B, ipc = C3.BODY, rawget(_G, "HSMP_IPC")
    if not (B and ipc and ipc.put) then return end
    local mesh = C3.hit_mesh and C3.hit_mesh(me) or body_mesh(me)
    ctx = ctx or C3.body_context(me)
    if mesh and ctx then return B.publish(me, mesh, ipc.put, Log, ctx) end
end
function C3.body_tick(me)
    local ctx = C3.body_context(me)
    if not ctx then return end
    local key = string.format("%s:%d:%d:%s",tostring(ctx.match_id),ctx.round,ctx.life,ctx.pawn)
    local now = os.clock()
    if (C3.body_generation ~= key and now >= (C3.body_retry_at or 0))
        or tick_num % C3.BODY.PUBLISH_TICKS == 0 then
        C3.body_retry_at = now + .25
        if C3.body_publish(me,ctx) then C3.body_generation = key end
    end
end
function C3.body_expected_context(peer)
    local view = HSESS and HSESS.view and HSESS.view()
    if view and view.state == "countdown" then
        if not (view.spawns and view.spawns[peer]) then return nil end
        return {match_id=view.match_id,round=view.round+1,life=1}
    end
    return C3.life_for(peer,false)
end
function C3.body_apply(peer, w)
    local B, ipc = C3.BODY, rawget(_G, "HSMP_IPC")
    if not (B and ipc and ipc.peer_rec) then return end
    local rec = ipc.peer_rec("peer_body2", peer)
    if type(rec) ~= "table" then return end
    local nm = wname(w)
    local mesh = C3.hit_mesh(w)
    if not (nm and mesh) then return end
    local expected = C3.body_expected_context(peer)
    local playback = ipc.bus_table and ipc.bus_table("playback")
    local context
    for _, row in ipairs(playback and playback.rows or {}) do
        if row.peer == peer and row.pawn == nm and B.context_matches(rec, row)
            and B.context_matches(rec, expected) then context = row; break end
    end
    if not context then return end
    local n = B.apply(nm, w, mesh, rec, context)
    if n > 0 then
        local f = B.fight_count()
        if f <= 5 or f % 50 == 0 then
            Log("body: stand-in %s of peer %d <- owner body v=%s: %d bone mass(es) set%s", nm, peer,
                tostring(rec.version), n, f > 0 and string.format(" (the game reset %d before)", f) or "")
        end
    end
end

-- Armour-stage bookkeeping: the Deal Complex Damage calls my weapon / body
-- made on stand-ins this tick, in callback order. No UObject is
-- kept in it; cleared every flush and on every world drop.
local CX = { pending = {}, orphans = 0, source_skips = {} }
local read_vitals
CX.discard_outbox = C3.discard_outbox
local complex_hook_ok = false
local complex_tries = 0

-- Relative speed at the contact, including angular motion of the striking
-- component and victim bone. The server compares this with its reconstructed
-- contact-point speed, so COM velocities would measure a different quantity.
-- 0 = stationary relative contact, or unavailable measurement.
function C3.vrel(collided, hitcomp, bone, point, source_bone)
    local function vel(c, b)
        if point then
            local ok, measured = pcall(function()
                return c:GetPhysicsLinearVelocityAtPoint(point, b or FName("None"))
            end)
            -- A successful zero is a real stationary point (e.g. a rotating
            -- body's pivot); do not replace it with its moving COM velocity.
            if ok and measured then return vec(measured) end
        end
        local v
        local ok, measured = pcall(function() return c:GetPhysicsLinearVelocity(b or FName("None")) end)
        -- A stationary physics body is still a valid measurement. Replacing
        -- zero with its skeletal component's movement invents a moving limb
        -- on a resting contact, just as replacing a stationary point would.
        if ok and measured then return vec(measured) end
        pcall(function() v = vec(c:GetComponentVelocity()) end)
        return v
    end
    local wv, bv = vel(collided, source_bone), vel(hitcomp, bone)
    if not wv or not bv then return 0 end
    local dx, dy, dz = wv[1] - bv[1], wv[2] - bv[2], wv[3] - bv[3]
    return math.sqrt(dx * dx + dy * dy + dz * dz)
end

-- Blueprint ComponentHit hooks run after their nested DCD call. Resolve the
-- original physics body then, before flush: component addresses and the exact
-- native impact point associate it with that pending call, never a nearby bone
-- or the pawn's stale last-contact fields. Only numeric metadata is retained.
function C3.body_hit(selfp, HitComponent, OtherActor, OtherComp, NormalImpulse, Hit)
    if replaying or #CX.pending == 0 then return end
    local hc, oc, hit = pv(HitComponent), pv(OtherComp), pv(Hit)
    if not hc or not oc or not hit then return end
    local ha, oa = addr_of(hc), addr_of(oc)
    if not ha or not oa then return end
    local point, my_bone, other_bone
    pcall(function() if hit.ImpactPoint then point = vec(hit.ImpactPoint) end end)
    pcall(function() my_bone = hit.MyBoneName end)
    pcall(function() other_bone = hit.BoneName end)
    if not point then return end
    -- Willie selects its self physics body with K2_GetClosestPointOnPhysicsAsset
    -- before the nested DCD (native ubergraph 234333/234420). MyBoneName can be
    -- None. Use that exact native result only while all original event fields
    -- still identify this same callback; never reuse last-contact pawn state.
    local w = pv(selfp)
    pcall(function()
        local original_name = my_bone and my_bone:ToString()
        if original_name and original_name ~= "None" and original_name ~= "" and hc:GetBoneIndex(my_bone) >= 0 then return end
        local native_point = w["Body Hit Impact Point"]
        if not native_point then return end
        local stored = vec(native_point)
        if same(w.Mesh,hc) and same(w["Hit Component"],hc) and same(w["Other Comp"],oc)
            and math.abs(stored[1]-point[1]) <= 0.01 and math.abs(stored[2]-point[2]) <= 0.01
            and math.abs(stored[3]-point[3]) <= 0.01 then
            local native_bone = w["Body Hit Bone Name Self"]
            local name = native_bone:ToString()
            if name ~= "None" and name ~= "" and hc:GetBoneIndex(native_bone) >= 0 then my_bone = native_bone end
        end
    end)
    local now = os.clock()
    for i = #CX.pending, math.max(1, #CX.pending - 63), -1 do
        local r = CX.pending[i]
        if r.body_source and not r.source_bone and now - r.at >= 0 and now - r.at <= 0.05 then
            local src, dst, sb, tb, target_name
            if r.source_address == ha and r.target_address == oa then src, dst, sb, tb = hc, oc, my_bone, other_bone
            elseif r.source_address == oa and r.target_address == ha then src, dst, sb, tb = oc, hc, other_bone, my_bone end
            pcall(function() target_name = tb:ToString() end)
            if src and target_name == r.bone and math.abs(r.loc[1]-point[1]) <= 0.01 and math.abs(r.loc[2]-point[2]) <= 0.01
                and math.abs(r.loc[3]-point[3]) <= 0.01 then
                local name, index
                pcall(function() name = sb:ToString(); index = src:GetBoneIndex(sb) end)
                if name and name ~= "None" and name ~= "" and index and index >= 0 then
                    r.vrel = C3.vrel(src, dst, FName(r.bone), {X=point[1],Y=point[2],Z=point[3]}, sb)
                    r.source_bone = name
                    CX.body_resolved = (CX.body_resolved or 0) + 1
                    if CX.body_resolved <= 5 or CX.body_resolved % 100 == 0 then
                        Log("body contact velocity: native source bone=%s victim=%s speed=%.1f (#%d)",
                            name, r.bone, r.vrel, CX.body_resolved)
                    end
                end
                return -- One original event resolves its newest matching DCD only.
            end
        end
    end
end

-- After-callback of Willie_BP "Deal Complex Damage" (the call has run).
local function on_complex(selfp, HitComponent, CollidedComponent, HitBone, Location, Normal,
                          HitVelocity, HitImpulse, CuttingPower, StabRate, Rigidity, BluntInt,
                          LowerThreshold, DamageParent, KickPower, HitBox, ExtraHigh, DrawCut,
                          HitSurfaceOut, DamageOut, CuttingOut, RigidityOut, DensityOut, LowerOut)
    if replaying then
        if C3.native_probe and C3.replay_trace then
            local w=pv(selfp)
            local trace=C3.replay_trace
            if w and w:IsValid() and addr_of(w)==trace.pawn then
                trace.dcd_calls=(trace.dcd_calls or 0)+1
                trace.dcd=C3.dcd_evidence({HitSurfaceOut,DamageOut,CuttingOut,RigidityOut,DensityOut,LowerOut},true)
            end
        end
        return
    end
    if next(puppet_peer) == nil and next(puppet_weapon) == nil then return end
    local w = pv(selfp)
    if not w or not w:IsValid() then return end
    local nm = wname(w)
    local native_probe_sample=C3.probe_last
    C3.probe_last=nil -- Consume once even if the original DCD cannot be claimed.
    local me = local_pawn()
    if me and same(w, me) then
        local sp = source_standin_peer(pv(CollidedComponent))
        if sp then C3.echo(me, sp) else C3.baseline(me) end
        return
    end
    if not (nm and puppet_peer[nm]) then return end
    -- Did this call pass the stand-in's own contact gate? The gate runs
    -- natively and exactly as in solo; a call it stopped never reached Get
    -- Damage there. (The backstop keeps that gate as the call left it.)
    local gate = BF.gate_passed(w, pv(HitBone), pv(HitImpulse), pv(CuttingPower))
    C3.standin_hit(w)   -- backstop (the nested Get Damage callback did it too)
    if not combat_window then return end
    if WG.travel_from ~= nil or WG.key == nil or not WG.settled() then return end
    local coll = pv(CollidedComponent)
    local src = hit_source(coll)
    if not (me and src and same(src, me)) then
        if src and not me then
            CX.me_stops = (CX.me_stops or 0) + 1
            if CX.me_stops <= 5 or CX.me_stops % 500 == 0 then Log("claim stopped: no local pawn resolved (#%d)", CX.me_stops) end
        end
        return
    end
    if gate == false then
        CX.gate_stops = (CX.gate_stops or 0) + 1
        if CX.gate_stops <= 5 or CX.gate_stops % 1000 == 0 then
            local last, last_bone, bone
            pcall(function() last=w["Last Complex Damage Impulse"];last_bone=w["Last Complex Damage Bone"]:ToString();bone=pv(HitBone):ToString() end)
            Log("native contact gate stopped claim: victim=%s bone=%s priorBone=%s priorImpulse=%s (#%d)",
                nm,tostring(bone),tostring(last_bone),tostring(last),CX.gate_stops)
        end
    end
    local peer = puppet_peer[nm]
    local mine, theirs = C3.life_for(my_peer_id,true), C3.displayed_for(peer,w)
    if not mine or not theirs or mine.match_id ~= theirs.match_id or mine.round ~= theirs.round then
        CX.ctx_stops = (CX.ctx_stops or 0) + 1
        if CX.ctx_stops <= 5 or CX.ctx_stops % 500 == 0 then
            Log("claim context missing: mine=%s theirs=%s peer=%s (#%d)", mine and "ok" or "nil", theirs and "ok" or "nil", tostring(peer), CX.ctx_stops)
        end
        return
    end
    local probe_text
    if C3.native_probe then
        -- this blow on one line: my native inputs and what the game did to the stand-in
        local pl = native_probe_sample
        local v = vec(pv(HitVelocity))
        local b = ""; pcall(function() b = pv(HitBone):ToString() end)
        local matched=gate~=false and pl and pl.name==nm and pl.bone==b and pl.source and pl.source==addr_of(coll)
            and now_ms()-pl.at>=0 and now_ms()-pl.at<50
        probe_text=matched and pl.text or "dmg Health unavailable [original native Get Damage sample absent]"
        if not matched then native_probe_sample=nil end
        Log("PROBE native on peer %s bone=%s vel=%.0f rig=%.2f cut=%.0f stab=%.2f: %s", tostring(peer), b,
            math.sqrt(v[1] ^ 2 + v[2] ^ 2 + v[3] ^ 2), num(pv(Rigidity)), num(pv(CuttingPower)), num(pv(StabRate)),
            probe_text)
    end
    CX.input_diag = (CX.input_diag or 0) + 1
    if CX.input_diag <= 10 or CX.input_diag % 100 == 0 then
        pcall(function()
            local hb = pv(HitBox)
            local has_box = hb and hb:IsValid()
            local box_name = has_box and hb:GetFullName() or "nil"
            local _, slot = BF.source(coll, me)
            local box_slot
            if has_box then _, box_slot = BF.source(hb, me) end
            Log("native DCD inputs #%d: collider=%s ordinal=%s Weapon=%s Flesh=%s HitBox=%s boxOrdinal=%s sameCollider=%s DamageParent=%s",
                CX.input_diag, coll:GetFullName(), tostring(slot),
                tostring(coll:ComponentHasTag(FName("Weapon"))), tostring(coll:ComponentHasTag(FName("Flesh"))),
                box_name, tostring(box_slot), tostring(has_box and same(hb, coll) or false), tostring(pv(DamageParent)))
        end)
    end
    local bname = ""
    pcall(function() bname = pv(HitBone):ToString() end)
    bname = bname:gsub("[^%w_]", "")
    local body, vitals = C3.BODY, read_vitals and read_vitals(peer)
    if body and body.severed and vitals and body.severed(bname, vitals.dism) then return end
    local loc = vec(pv(Location))
    local nrm, vel, imp = vec(pv(Normal)), vec(pv(HitVelocity)), vec(pv(HitImpulse))
    local hmesh = pv(HitComponent)
    local fr = BF.of(hmesh, bname) or BF.of(body_mesh(w), bname)
    local off, flags = { 0, 0, 0 }, FLAG_COMPLEX
    if fr then
        off = BF.to_local(fr, loc)
        nrm, vel, imp = BF.rot(fr.q, nrm, true), BF.rot(fr.q, vel, true), BF.rot(fr.q, imp, true)
        flags = flags + BF.LOCAL
    else
        local bp = bone_pos(hmesh, bname) or bone_pos(body_mesh(w), bname)
        if bp then off = { loc[1] - bp[1], loc[2] - bp[2], loc[3] - bp[3] } end
    end
    local source = 0
    if BF.is_weapon(coll) then
        flags = flags + BF.WEAPON
        local ordinal, why
        source, ordinal, why = BF.source(coll, me)
        if not ordinal then
            why = why or "unknown_source"
            local n = (CX.source_skips[why] or 0) + 1
            CX.source_skips[why] = n
            if n <= 3 or n % 100 == 0 then
                local name = "?"; pcall(function() name = coll:GetFullName() end)
                Log("contact claim skipped: source collider %s (%s, #%d)", name, why, n)
            end
            return -- Preserve actual collider identity; never fabricate a head/root fallback.
        end
    end
    local vts, vats = playback_ts(peer)
    local hit_box = BF.hit_box(pv(HitBox), coll, me)
    if hit_box == nil then
        Log("contact claim skipped: native cutting HitBox cannot be represented exactly")
        return
    end
    if hit_box ~= 0 and (flags & BF.WEAPON == 0 or source & (BF.LEFT | BF.RIGHT) == 0) then return end
    local body_source = same(coll, C3.hit_mesh(me)) or same(coll, body_mesh(me))
    local source_class = ""
    if flags & BF.WEAPON ~= 0 then
        pcall(function() source_class=coll:GetOwner():GetClass():GetFName():ToString() end)
        if source_class=="" or #source_class>=48 then
            Log("contact claim skipped: original native weapon class is not representable (%d bytes)",#source_class);return
        end
    end
    local box_frame={0,0,0,0,0,0,0,0,0,0,0,0,0}
    if hit_box~=0 then
        box_frame=CuttingBox.capture(pv(HitBox),fr,BF)
        if not box_frame then Log("contact claim skipped: original native cutting Box geometry unavailable");return end
    end
    local rec = {
        at = os.clock(), vel = vel, imp = imp,
        -- Skeletal bodies have many physics bodies: None would sample the root,
        -- silently losing a moving limb's velocity. Await original ComponentHit.
        vrel = body_source and 0 or C3.vrel(coll, hmesh, pv(HitBone), pv(Location)),
        body_source = body_source, source_address = addr_of(coll), target_address = addr_of(hmesh),
        cut = num(pv(CuttingPower)), stab = num(pv(StabRate)), rig = num(pv(Rigidity)),
        dism = math.floor(num(pv(BluntInt))), lower = pv(LowerThreshold) == true,
        kick = num(pv(KickPower)), xhv = pv(ExtraHigh) == true, draw = num(pv(DrawCut)),
        nm = nm, peer = peer, bone = bname, gate = gate, flags = flags, source = source, parent = pv(DamageParent) == true,
        match_id = mine.match_id, round = mine.round, attacker_life = mine.life, victim_life = theirs.life, hit_box = hit_box,
        source_class=source_class,hit_box_frame=box_frame,
        probe=probe_text,probe_attacker=my_peer_id,
        native_evidence=C3.native_probe and ("last_zone="..C3.native_zone(w).." "..
            C3.dcd_evidence({HitSurfaceOut,DamageOut,CuttingOut,RigidityOut,DensityOut,LowerOut},true)..
            " gd="..(native_probe_sample and native_probe_sample.trace or "unavailable")) or nil,
        ats = now_ms(), vts = vts, vats = vats, loc = loc, nrm = nrm, off = off,
    }
    CX.pending[#CX.pending + 1] = rec
end
CX.pre = function(...) pcall(on_complex, ...) end   -- (name kept: the hook wrapper)

-- Evidence only: embedded-weapon GD is not an ordinary impact claim. Resolve
-- original native constraint membership synchronously; retain no UObject.
function C3.inside_journal(w,coll,bone,mesh,box,raw,cut,draw,pain,apply)
    if not combat_window or not coll or not coll:IsValid() then return end
    local me=local_pawn()
    local peer=puppet_peer[wname(w) or ""]
    local mine,theirs=C3.life_for(my_peer_id,true),C3.displayed_for(peer,w)
    if not me or not peer or not mine or not theirs or mine.match_id~=theirs.match_id or mine.round~=theirs.round then return end
    local weapon=coll:GetOwner()
    if not weapon or not weapon:IsValid() or not same(weapon_parent(weapon),me) then return end
    local source,ordinal=BF.source(coll,me)
    local matched,n=nil,0
    local arr=weapon["Stuck Constraints Array"]
    if arr then arr:ForEach(function(_,entry)
        n=n+1;if n>128 then return end
        local c=entry:get()
        if c and c:IsValid() and same(c["My Weapon"],weapon) and same(c["Weapon Hit Module"],coll)
            and same(c["Hit Actor"],w) and same(c["Component 2 (Body)"],mesh)
            and c["Bone Name 2"]:ToString()==bone:ToString() then matched=c:GetFullName() end
    end) end
    C3.inside_count=(C3.inside_count or 0)+1
    if C3.inside_count>8 and os.clock()<(C3.inside_diag_at or 0)+1 then return end
    C3.inside_diag_at=os.clock()
    local frame=CuttingBox.capture(box,BF.of(mesh,bone:ToString()),BF)
    Log("INSIDE_JOURNAL evidence_only=true match=%s round=%s attackerLife=%s victimLife=%s peer=%s source=%s ordinal=%s class=%s constraint=%s bone=%s raw=%s cut=%s draw=%s pain=%s applyBone=%s boxFrame=%s collider=%s",
        tostring(mine.match_id),tostring(mine.round),tostring(mine.life),tostring(theirs.life),tostring(peer),
        tostring(source),tostring(ordinal),tostring(weapon:GetClass():GetFName():ToString()),tostring(matched or "UNPROVEN"),
        bone:ToString(),tostring(raw),tostring(cut),tostring(draw),tostring(pain),tostring(apply),
        frame and table.concat(frame,",") or "nil_or_unrepresentable",coll:GetFullName())
    return matched~=nil
end

function C3.constraint_begin_journal(selfp)
    if replaying or not combat_window then return end
    local c=pv(selfp)
    if not c or not c:IsValid() then return end
    local weapon,w=c["My Weapon"],c["Hit Actor"]
    local me=local_pawn()
    if not weapon or not weapon:IsValid() or not me or not same(weapon_parent(weapon),me)
        or not w or not w:IsValid() then return end
    local peer=puppet_peer[wname(w) or ""]
    local mine,theirs=C3.life_for(my_peer_id,true),C3.displayed_for(peer,w)
    if not peer or not mine or not theirs or mine.match_id~=theirs.match_id or mine.round~=theirs.round then return end
    local coll=c["Weapon Hit Module"]
    local source,ordinal=BF.source(coll,me)
    -- Match the original native call before selection, including a gate-suppressed
    -- contact. Both component addresses and callback order identify its module.
    pcall(function()
        local nm,b2=wname(w),c["Bone Name 2"]:ToString()
        for i=#CX.pending,1,-1 do
            local r=CX.pending[i]
            if r.nm==nm and r.source_address and r.target_address
                and r.source_address==addr_of(coll) and r.target_address==addr_of(c["Component 2 (Body)"])
                and r.source==source and source~=0 and os.clock()-r.at>=0 and os.clock()-r.at<=0.05
                and (r.bone==b2 or (r.bone=="pelvis" and b2=="spine_02")) then
                C3.stuck_parent=C3.stuck_parent or {}
                if next(C3.stuck_parent)~=nil and C3.stuck_parent_n and C3.stuck_parent_n>64 then C3.stuck_parent,C3.stuck_parent_n={},0 end
                C3.stuck_parent[c:GetFullName()]=r
                r.constraint_parent=true
                C3.stuck_parent_n=(C3.stuck_parent_n or 0)+1
                break
            end
        end
    end)
    C3.constraint_count=(C3.constraint_count or 0)+1
    if C3.constraint_count>8 and os.clock()<(C3.constraint_diag_at or 0)+1 then return end
    C3.constraint_diag_at=os.clock()
    Log("CONSTRAINT_JOURNAL evidence_only=true event=BeginPlay constraint=%s match=%s round=%s attackerLife=%s victimLife=%s peer=%s weapon=%s source=%s ordinal=%s bone=%s targetComponent=%s",
        c:GetFullName(),tostring(mine.match_id),tostring(mine.round),tostring(mine.life),tostring(theirs.life),tostring(peer),
        weapon:GetFullName(),tostring(source),tostring(ordinal),c["Bone Name 2"]:ToString(),c["Component 2 (Body)"]:GetFullName())
end

-- After-callback of Willie_BP "Get Damage" (the call has run).
local function on_get_damage(selfp, Impulse, Velocity, Location, Normal, bone, RawDamage,
                             CuttingPower, Inside, DamagedMesh, DismBlunt, LowerThreshold,
                             Shockwave, HitByComponent, Flesh, HitBox, PainRate, ApplyBoneChange, DrawCut, DamageApplied)
    if C3.body_audit and C3.body_probe then
        local meta={}
        for k,v in pairs(C3.body_replay or {})do meta[k]=v end
        meta.pre="unavailable:Blueprint_POST"
        local b=pv(bone);pcall(function()meta.bone=b:ToString()end)
        pcall(function()
            local m,h=pv(DamagedMesh),pv(HitByComponent)
            if m and m:IsValid()then meta.damaged_mesh=addr_of(m)end
            if h and h:IsValid()then meta.hit_by=addr_of(h)end
        end)
        meta.raw,meta.cut,meta.draw,meta.pain_rate=pv(RawDamage),pv(CuttingPower),pv(DrawCut),pv(PainRate)
        meta.inside,meta.lower,meta.damage_applied=pv(Inside),pv(LowerThreshold),pv(DamageApplied)
        pcall(C3.body_audit.capture,pv(selfp),"Get Damage POST",meta)
    end
    if replaying then
        local trace=C3.replay_trace
        local w=pv(selfp)
        if trace and w and w:IsValid() and addr_of(w)==trace.pawn then
            trace.calls=trace.calls+1
            trace.raw=tonumber(pv(RawDamage));trace.cut=tonumber(pv(CuttingPower))
            trace.draw=tonumber(pv(DrawCut));trace.applied=pv(DamageApplied)
            local b=pv(bone);trace.bone=b and b:ToString() or "?"
            if C3.native_probe then
                trace.samples=trace.samples or {}
                if #trace.samples<32 then trace.samples[#trace.samples+1]=C3.gd_evidence(w,bone,RawDamage,CuttingPower,DrawCut,PainRate,Inside,LowerThreshold,DamageApplied)
                else trace.truncated=true end
            end
        end
        return
    end
    if next(puppet_peer) == nil and next(puppet_weapon) == nil then return end
    local w = pv(selfp)
    if not w or not w:IsValid() then return end
    local native_probe=C3.native_probe and puppet_peer[wname(w) or ""] and C3.probe_measure(w) or nil
    local gd_trace=native_probe and C3.gd_evidence(w,bone,RawDamage,CuttingPower,DrawCut,PainRate,Inside,LowerThreshold,DamageApplied) or nil
    if pv(Inside)==true then
        pcall(C3.inside_journal,w,pv(HitByComponent),pv(bone),pv(DamagedMesh),pv(HitBox),
            pv(RawDamage),pv(CuttingPower),pv(DrawCut),pv(PainRate),pv(ApplyBoneChange))
        pcall(C3.inside_forward,w,pv(HitByComponent),pv(bone),pv(DamagedMesh),pv(Location),
            pv(RawDamage),pv(CuttingPower),pv(DrawCut),pv(PainRate),pv(LowerThreshold),pv(Shockwave),
            pv(Flesh),pv(ApplyBoneChange),pv(DismBlunt),pv(Normal),pv(Impulse),pv(Velocity),native_probe,gd_trace)
    elseif native_probe then
        C3.probe_last={name=wname(w),at=now_ms(),bone=pv(bone):ToString(),source=addr_of(pv(HitByComponent)),text=native_probe,trace=gd_trace}
    end
    local me = local_pawn()
    if me and same(w, me) then
        local sp = source_standin_peer(pv(HitByComponent))
        if sp then C3.echo(me, sp) else C3.baseline(me) end
        return
    end
    local nm = wname(w)
    if nm and puppet_peer[nm] then
        C3.standin_hit(w)
        if native_probe then
            local address=addr_of(w)
            if address then
                C3.probe_after=C3.probe_after or {}
                C3.probe_after[address]={f=snapshot(w),tick=tick_num}
            end
        end
    end
end

-- Weapon-on-weapon contact between my weapon and a stand-in's (either side
-- of the collision may be the one whose "Collision Hit" fires).
local clash_last = {}      -- peer_id -> now_ms of the last report
local clash_count = 0
local function weapon_owner(a)
    if not a or not a:IsValid() then return nil end
    return weapon_parent(a)
end

local function on_weapon_hit(selfp, HitComponent, OtherActor)
    if replaying or not combat_window then return end
    if next(puppet_peer) == nil then return end   -- no stand-ins, no clash to report
    if not WG.settled() then return end
    local me = local_pawn()
    if not me then return end
    local oa = pv(OtherActor)
    local mine = weapon_owner(pv(selfp))
    -- A stand-in's weapon reached MY body. Its damage gate is off
    -- ("Temp Disable Damage"), so this callback is the only trace: report the
    -- touch (the server keeps a hit this blow's clash would have cancelled).
    if mine and oa and same(oa, me) then
        local p = puppet_peer[wname(mine) or ""]
        if p then C3.touch(p) end
        return
    end
    local other = weapon_owner(oa)
    if not mine or not other then return end          -- not weapon vs weapon
    local peer
    if same(mine, me) then peer = puppet_peer[wname(other) or ""]
    elseif same(other, me) then peer = puppet_peer[wname(mine) or ""] end
    if not peer then return end
    local now = now_ms()
    if clash_last[peer] and now - clash_last[peer] < CLASH_GAP_MS then return end
    clash_last[peer] = now
    local _, arm_ts = playback_ts(peer)
    send_rec("clash", { other_peer_id = peer, my_ts = now, other_ts = arm_ts })
    clash_count = clash_count + 1
    if clash_count <= 10 or clash_count % 20 == 0 then
        Log("clash with peer %d (my_ts=%d other_ts=%d, #%d)", peer, now, arm_ts, clash_count)
    end
end

local on_native_death   -- defined in the owner section below
local hook_ok, weapon_hook_ok, weapon_hook_tries = false, false, 0
local death_hook_ok, death_hook_tries = false, 0
local death_hooked = {}   -- fn path -> true once registered
local function try_hook()
    local retry_now = tick_num <= 30 or tick_num % 30 == 0   -- 1 Hz after the first second
    if C3.body_audit and not C3.body_hook_ok and retry_now then
        C3.body_hook_ok=C3.body_audit.install(RegisterHook)
    end
    if C3.armor_audit and not C3.armor_hook_ok and retry_now then
        C3.armor_hook_ok=C3.armor_audit.install(RegisterHook)
        if C3.armor_hook_ok then Log("native armour proxy construction hook registered (developer evidence only)") end
    end
    if C3.armor_audit and not C3.trace_hook_ok and retry_now then
        C3.trace_hook_ok=pcall(function()
            RegisterHook("/Script/Engine.KismetSystemLibrary:SphereTraceMultiForObjects",function()end,
                function(...) pcall(C3.native_trace_post,...) end)
        end)
        if C3.trace_hook_ok then Log("native armour trace POST hook registered (active owner replay evidence only)") end
    end
    if not hook_ok and retry_now then
        -- A BP function: the callback runs AFTER the body (see above).
        hook_ok = pcall(function()
            RegisterHook(GET_DAMAGE, function(...) pcall(on_get_damage, ...) end)
        end)
        if hook_ok then Log("Get Damage hook registered (after-call: echo undo, stand-in backstop)") end
    end
    if hook_ok and not complex_hook_ok and retry_now then
        complex_hook_ok = pcall(function() RegisterHook(DEAL_COMPLEX, CX.pre) end)
        complex_tries = complex_tries + 1
        if complex_hook_ok or complex_tries == 1 then
            Log("Deal Complex Damage hook %s", complex_hook_ok and "registered (claims: armour-stage inputs)"
                or "NOT available yet (retrying)")
        end
    end
    if complex_hook_ok and not CX.body_hook_ok and retry_now then
        CX.body_hook_ok = pcall(function()
            RegisterHook("/Game/Character/Blueprints/Willie_BP.Willie_BP_C:BndEvt__BP_ThirdPersonCharacter_Mesh_K2Node_ComponentBoundEvent_0_ComponentHitSignature__DelegateSignature",
                function(...) pcall(C3.body_hit, ...) end)
        end)
        if CX.body_hook_ok then Log("native body-hit source bone hook registered (after nested DCD)") end
    end
    if hook_ok and not CX.constraint_hook_ok and retry_now then
        CX.constraint_hook_ok=pcall(function()
            RegisterHook("/Game/Blueprints/Utility/Constraint_Weapon_Stuck_BP.Constraint_Weapon_Stuck_BP_C:ReceiveBeginPlay",
                function(s) pcall(C3.constraint_begin_journal,s) end)
        end)
        if CX.constraint_hook_ok then Log("native stuck-constraint hook registered (exact contact parent binding)") end
    end
    -- The BP classes load with the first arena, which can be long after
    -- mod load (menu / server browser). Retry once a second, FOREVER, until
    -- registered (a failed RegisterHook is one cheap lookup), per function
    -- (a function never gets two hooks).
    -- Native death of OUR pawn -> immediate reliable death report (extra
    -- channel next to HSMPSync's Health<=0 poll). First callback = PRE.
    if not death_hook_ok and tick_num % 30 == 0 then
        death_hook_tries = death_hook_tries + 1
        local n = 0
        for _, fn in ipairs({ DEATH_FN, DYING_FN }) do
            if not death_hooked[fn] then
                death_hooked[fn] = pcall(function() RegisterHook(fn, function(s) pcall(on_native_death, s) end) end)
                if death_hooked[fn] then Log("native %s hook registered (try %d)", fn:match("[^:]+$"), death_hook_tries) end
            end
            if death_hooked[fn] then n = n + 1 end
        end
        if not death_hooked["native_defeat"] then
            death_hooked["native_defeat"]=pcall(function()
                RegisterHook("/Game/Character/Blueprints/Willie_BP.Willie_BP_C:Event Lose Match",
                    function(selfp) pcall(C3.on_native_defeat,selfp) end)
            end)
            if death_hooked["native_defeat"] then Log("native Event Lose Match hook registered (verified defeat)") end
        end
        death_hook_ok = n == 2 and death_hooked["native_defeat"]
    end
    -- The weapon class loads with the first arena; retry every ~1 s, forever.
    if not weapon_hook_ok and tick_num % 30 == 0 then
        weapon_hook_tries = weapon_hook_tries + 1
        weapon_hook_ok = pcall(function()
            RegisterHook(WEAPON_HIT, function(s, hc, oa) pcall(on_weapon_hit, s, hc, oa) end)
        end)
        if weapon_hook_ok then Log("weapon Collision Hit hook registered (clash reports)") end
    end
end

-- --- claims: one per contact ------------------------------------------------------

-- The strongest armour-stage call of a contact: the game's own contact
-- strength, |Hit Impulse| · (Cutting Power + 1) (Deal Complex Damage's gate),
-- then Rigidity · |Hit Velocity| (its Raw before armour).
local function strength(r)
    local function len(v) return math.sqrt(v[1] * v[1] + v[2] * v[2] + v[3] * v[3]) end
    return len(r.imp) * (r.cut + 1), r.rig * len(r.vel)
end
-- Rank by Raw (Rigidity · |Hit Velocity|), the damage itself, and only then by
-- the gate value. Ranking by the gate first prefers a blade STUCK on the
-- stand-in (rubber-banded / high collision threshold: Hit Velocity ×0.1, the
-- normal impulse still huge) over the clean frame that cut, so strong
-- contacts get claimed at a tenth of their speed.
local function stronger(a, b)
    if not a then return b end
    if not b then return a end
    local ga, ra = strength(a)
    local gb, rb = strength(b)
    if ra ~= rb then return ra > rb and a or b end
    return gb > ga and b or a
end

local cid_seq = 0
local claim_peer = {}      -- cid -> peer (cues)
local quality = { claims = 0, accepted = 0, confirmed = 0, clashes = 0, rejected = {}, pending = 0 }

-- One claim: the Deal Complex Damage inputs (FLAG_COMPLEX) as the game passed
-- them on the stand-in. imp / vel = Hit Impulse / Hit Velocity, cut / draw =
-- Cutting Power / Draw Cut, pain = Stab Rate, out = Rigidity, raw = relative
-- speed (uu/s, 0 unknown), dism = Blunt Destruction Int | Kick·10 << 8 |
-- Lower Threshold << 16 | Extra High Velocity << 17. No measured deltas: the
-- victim's native replay is the damage (solo parity), and the server books
-- nothing from the stand-in.
local function send_claim(peer, r)
    if r.cid then return true end
    local dism = math.max(0, math.min(255, r.dism)) + math.max(0, math.min(255, math.floor(r.kick * 10 + 0.5))) * 256
        + (r.lower and 65536 or 0) + (r.xhv and 131072 or 0) + (r.source or 0) + (r.parent and 67108864 or 0)
        + (r.hit_box or 0) * 134217728
    local cid = cid_seq + 1
    local lage = math.max(0, math.min(1000, now_ms() - r.ats))
    -- The `damage` record (crates/hsmp-ipc schema/combat.rs Damage): hit_id,
    -- round and age_ms are the sidecar's; no delta rows.
    local sent = send_rec("damage", {
        cid = cid, target_peer_id = peer, lage_ms = lage,
        match_id = r.match_id, round = r.round, attacker_life = r.attacker_life, victim_life = r.victim_life,
        source_class=r.source_class or "",hit_box_frame=r.hit_box_frame or {0,0,0,0,0,0,0,0,0,0,0,0,0},
        attacker_ts = r.ats, victim_view_ts = r.vts, victim_arm_ts = r.vats,
        dism_blunt = dism, raw_damage = r.vrel or 0, cutting_power = r.cut, pain_rate = r.stab,
        draw_cut = r.draw, damage_out = r.rig,
        offset = r.off, location = r.loc, impulse = r.imp, velocity = r.vel, normal = r.nrm,
        -- Inside|Complex without parent means the native DCD gate stopped this
        -- origin. Authenticate its geometry, acknowledge it without replaying.
        bone = r.bone, flags = (r.flags or FLAG_COMPLEX) + (r.gate==false and 1 or 0),
    })
    if not sent then
        quality.refused = (quality.refused or 0) + 1
        return false
    end
    cid_seq = cid
    r.cid = cid   -- a blade this hit leaves stuck continues under this cid (C3.inside_forward)
    if r.probe then Log("LAB_PROBE attacker=%d cid=%d parent_cid=0 bone=%s %s",r.probe_attacker or my_peer_id,cid,r.bone,r.probe) end
    if r.native_evidence then C3.log_native_evidence("probe",r,r.probe_attacker or my_peer_id,r.native_evidence,cid) end
    claim_peer[cid] = peer
    sent_count = sent_count + 1
    quality.claims = quality.claims + 1
    quality.pending = quality.pending + 1
    if sent_count <= 10 or sent_count % 20 == 0 then
        local g, raw = strength(r)
        Log("HIT peer %d stand-in bone=%s claim #%d: vel=%.0f imp=%.0f rig=%.2f cut=%.0f stab=%.2f draw=%.0f kick=%.1f (raw %.0f, gate %.0f) (#%d)",
            peer, r.bone, cid_seq, math.sqrt(r.vel[1] ^ 2 + r.vel[2] ^ 2 + r.vel[3] ^ 2),
            math.sqrt(r.imp[1] ^ 2 + r.imp[2] ^ 2 + r.imp[3] ^ 2), r.rig, r.cut, r.stab, r.draw, r.kick, raw, g, sent_count)
    end
    return true
end

-- A blade of mine stuck in a stand-in (native Constraint_Weapon_Stuck) calls its Get Damage
-- with Inside while it stays embedded: the owner gets the same call, as a FLAG_INSIDE claim
-- under the cid of the hit that stuck it (the server binds it to that accepted hit).
-- Get Damage(.., Raw, Cut, Inside, .., DismBlunt, LowerThreshold, Shockwave, HitBy, Stab?,
-- HitBox, PainRate, HitFlesh, DrawCut): the legacy replay maps flag bits 1 Inside,
-- 2 Lower Threshold, 4 Shockwave, 8 Stab, 16 Hit Flesh.
function C3.inside_send(rec)
    local parent=rec.parent
    if not parent.cid then return false end
    local probe,probe_attacker,evidence=rec.probe,rec.probe_attacker,rec.native_evidence
    rec.parent=nil;rec.probe=nil;rec.probe_attacker=nil;rec.native_evidence=nil;rec.cid=cid_seq+1;rec.parent_cid=parent.cid
    if not send_rec("damage",rec) then return false end
    cid_seq=cid_seq+1;claim_peer[cid_seq]=rec.target_peer_id
    if probe then Log("LAB_PROBE attacker=%d cid=%d parent_cid=%d bone=%s %s",probe_attacker or my_peer_id,cid_seq,parent.cid,rec.bone,probe) end
    if evidence then C3.log_native_evidence("probe",rec,probe_attacker or my_peer_id,evidence,cid_seq) end
    CX.inside_sent=(CX.inside_sent or 0)+1
    if CX.inside_sent<=5 or CX.inside_sent%200==0 then
        Log("stuck blade in peer %d bone=%s: Inside Get Damage forwarded under claim #%d (raw %.0f cut %.0f, #%d)",
            rec.target_peer_id,rec.bone,parent.cid,rec.raw_damage,rec.cutting_power,CX.inside_sent)
    end
    return true
end

function C3.inside_forward(w,coll,bone,mesh,loc,raw,cut,draw,pain,lower,shock,stab,flesh,dism,nrm,imp,vel,probe,gd_trace)
    if replaying or not combat_window or not coll or not coll:IsValid() then return end
    local me=local_pawn()
    local peer=puppet_peer[wname(w) or ""]
    local mine,theirs=C3.life_for(my_peer_id,true),C3.displayed_for(peer,w)
    if not me or not peer or not mine or not theirs or mine.match_id~=theirs.match_id or mine.round~=theirs.round then return end
    local weapon=coll:GetOwner()
    if not weapon or not weapon:IsValid() or not same(weapon_parent(weapon),me) then return end
    local bname=bone:ToString()
    local parent,constraint_evidence
    local arr=weapon["Stuck Constraints Array"]
    if arr then arr:ForEach(function(_,entry)
        local c=entry:get()
        if not parent and c and c:IsValid() and same(c["My Weapon"],weapon) and same(c["Hit Actor"],w)
            and same(c["Weapon Hit Module"],coll) and same(c["Component 2 (Body)"],mesh)
            and c["Bone Name 2"]:ToString()==bname then
            parent=C3.stuck_parent and C3.stuck_parent[c:GetFullName()]
            if C3.native_probe then
                local rows={"constraint="..c:GetFullName():gsub("%s","_")}
                for _,key in ipairs({"Material Density","Distance","Draw Cut","Edge Sharpness","Tip Sharpness","Thrust?","Stuck In Bone","Spikes (temp)"}) do
                    local value;pcall(function() value=c[key] end)
                    rows[#rows+1]=key:gsub("%W","_").."="..C3.native_scalar(value)
                end
                constraint_evidence=table.concat(rows," ")
            end
        end
    end) end
    local source,ordinal=BF.source(coll,me)
    if not (parent and ordinal and source==parent.source and parent.match_id==mine.match_id
        and parent.round==mine.round and parent.attacker_life==mine.life and parent.victim_life==theirs.life) then
        CX.inside_orphans=(CX.inside_orphans or 0)+1
        if CX.inside_orphans<=5 or CX.inside_orphans%200==0 then
            Log("stuck-blade call not forwarded: no claimed parent hit for this constraint (#%d)",CX.inside_orphans)
        end
        return
    end
    local function b(v,n) return v==true and n or 0 end
    local flags=1+b(lower,2)+b(shock,4)+b(stab,8)+b(flesh,16)+BF.WEAPON
    local bp=bone_pos(mesh,bname)
    if not bp then return end -- No invented bone origin for a native continuation.
    local at=vec(loc)
    local rec={
        parent=parent,target_peer_id=peer,lage_ms=0,
        probe=probe,probe_attacker=my_peer_id,
        native_evidence=C3.native_probe and ("gd="..(gd_trace or "unavailable").." "..(constraint_evidence or "constraint=unavailable")) or nil,
        match_id=mine.match_id,round=mine.round,attacker_life=mine.life,victim_life=theirs.life,
        source_class=parent.source_class,hit_box_frame={0,0,0,0,0,0,0,0,0,0,0,0,0},
        attacker_ts=parent.ats or 0,victim_view_ts=parent.vts or 0,victim_arm_ts=parent.vats or 0,
        dism_blunt=math.max(0,math.min(255,math.floor(num(dism))))+source,raw_damage=num(raw),cutting_power=num(cut),
        pain_rate=num(pain),draw_cut=num(draw),damage_out=0,
        offset={at[1]-bp[1],at[2]-bp[2],at[3]-bp[3]},location=at,
        impulse=imp and vec(imp) or {0,0,0},velocity=vel and vec(vel) or {0,0,0},
        normal=nrm and vec(nrm) or {0,0,0},bone=bname,flags=flags,
    }
    if parent.cid then return C3.inside_send(rec) end
    -- Native BeginPlay's initial Inside call can run before this tick's flush.
    -- Queue plain data after its exact origin instead of orphaning it.
    CX.inside_queue=CX.inside_queue or {}
    if #CX.inside_queue<128 then CX.inside_queue[#CX.inside_queue+1]=rec end
end

local episodes = {}        -- (kept for the test api; claims follow the game's own gate)

-- This tick's armour-stage calls, per stand-in and bone. Every call that
-- passed the stand-in's Deal Complex Damage gate is a claim, in order: that
-- gate runs natively on my screen exactly as it would on the victim in solo,
-- so these are the calls that reach Get Damage in solo (a graze followed by
-- a harder frame is two applications there, and two here). The victim's
-- replay then applies Get Damage's own per-bone gate as solo would. Calls
-- whose gate state could not be read fall back to the strongest per bone.
local function flush_claims()
    local work, groups = CX.pending, {}
    CX.pending = {}
    if not combat_window then CX.inside_queue={};return end
    -- Select within each body/bone's bounded budget, then emit in the original
    -- callback order. Get Damage remembers the previous damaged bone: grouping
    -- A/B/A into A/A/B changes which native calls pass that gate.
    for _, r in ipairs(work) do
        if r.body_source and not r.source_bone then
            CX.body_unresolved = (CX.body_unresolved or 0) + 1
            if CX.body_unresolved <= 5 or CX.body_unresolved % 100 == 0 then
                Log("body contact velocity unavailable: no original matching physics bone victim=%s (#%d)", r.bone, CX.body_unresolved)
            end
        end
        local by_bone = groups[r.nm]
        if not by_bone then by_bone = {}; groups[r.nm] = by_bone end
        local rs = by_bone[r.bone]
        if not rs then rs = {}; by_bone[r.bone] = rs end
        if r.gate~=false then rs[#rs + 1] = r end
    end
    local selected = {}
    for _, by_bone in pairs(groups) do
        for _, rs in pairs(by_bone) do
            local known = true
            for _, r in ipairs(rs) do if r.gate == nil then known = false end end
            if #rs==0 then
                rs={}
            elseif not known then
                local best
                for _, r in ipairs(rs) do best = stronger(best, r) end
                rs = { best }
            elseif #rs > BF.MAX_PER_BONE_TICK then
                -- keep the first call and the strongest of the rest, in order
                local idx = {}
                for i = 2, #rs do idx[#idx + 1] = i end
                table.sort(idx, function(a, b)
                    local ga, ra = strength(rs[a])
                    local gb, rb = strength(rs[b])
                    if ra ~= rb then return ra > rb end
                    if ga ~= gb then return ga > gb end
                    return a < b
                end)
                local keep = { [1] = true }
                for k = 1, BF.MAX_PER_BONE_TICK - 1 do keep[idx[k]] = true end
                local out = {}
                for i, r in ipairs(rs) do if keep[i] then out[#out + 1] = r end end
                rs = out
            end
            for _, r in ipairs(rs) do selected[r] = true end
        end
    end
    for _, r in ipairs(work) do
        if selected[r] or r.constraint_parent then send_claim(r.peer, r) end
    end
    local inside=CX.inside_queue or {};CX.inside_queue={}
    for _,rec in ipairs(inside) do C3.inside_send(rec) end
end

-- --- server feedback: cues, logs, combat_quality ------------------------------------

local reject_logs = 0

-- `damage_verdict` records: kind = S.ENUMS.verdict_kind (CONFIRM 1, FINAL 2,
-- CLASH 3), code = S.ENUMS.damage_reason (rejects are counted by its lower-case
-- name: "parried", "range", ...), reason = the text for the log.
local function read_feedback()
    local ipc = rawget(_G, "HSMP_IPC")
    local S = ipc and ipc.S
    local names = S and S.ENUM_NAMES and S.ENUM_NAMES.damage_reason or {}
    for _, e in ipairs(events("damage_verdict")) do
        local d = type(e.data) == "table" and e.data or {}
        local kind = tonumber(d.kind) or 0
        local cid = tonumber(d.cid) or 0
        if kind == 1 then
            quality.confirmed = quality.confirmed + 1
        elseif kind == 2 then
            quality.pending = math.max(0, quality.pending - 1)
            if d.ok == true then
                quality.accepted = quality.accepted + 1
            else
                local code = string.lower(tostring(names[tonumber(d.code) or -1] or "unknown"))
                quality.rejected[code] = (quality.rejected[code] or 0) + 1
                reject_logs = reject_logs + 1
                if reject_logs <= 20 or reject_logs % 20 == 0 then
                    Log("claim #%d on peer %s REJECTED by server: %s (#%d)", cid,
                        tostring(claim_peer[cid]), tostring(d.reason ~= "" and d.reason or code), reject_logs)
                end
            end
            claim_peer[cid] = nil
        elseif kind == 3 then
            quality.clashes = quality.clashes + 1
            if quality.clashes <= 10 or quality.clashes % 20 == 0 then
                Log("server validated a clash with peer %s (#%d)", tostring(d.peer), quality.clashes)
            end
        end
    end
end

-- combat_quality every QUALITY_S during a round (claim acceptance in real
-- games). Fields: claims, accepted, rejected_by_reason {code = n},
-- confirmed, clashes, pending (claims without a final answer yet).
local quality_at = 0
local function emit_quality(force)
    local now = os.clock()
    if not force and now - quality_at < QUALITY_S then return end
    quality_at = now
    -- Called only while the round is Live: a quiet window is reported too
    -- (claims=0), so the session gate can tell "nobody fought" (pending not
    -- growing, nothing to judge) from "the combat path is dead" (no sample).
    -- The acceptance ratio is only judged once >= 20 claims were decided.
    local f = {
        claims = quality.claims, accepted = quality.accepted, rejected_by_reason = quality.rejected,
        confirmed = quality.confirmed, clashes = quality.clashes, pending = quality.pending,
        round = match_round, window_s = QUALITY_S,
        unsupported_source_colliders = CX.source_skips,
        owner_replay_by_status = C3.replay_stats,
        attacker_receipts_by_status = C3.receipt_stats,
        owner_observed_fields = C3.replay_fields, owner_health_delta = C3.replay_hp,
        attacker_observed_fields = C3.receipt_fields, attacker_health_delta = C3.receipt_hp,
    }
    local parts = {}
    for k, v in pairs(quality.rejected) do parts[#parts + 1] = k .. "=" .. v end
    Log("combat_quality: claims=%d accepted=%d rejected{%s} confirmed=%d clashes=%d pending=%d",
        quality.claims, quality.accepted, table.concat(parts, ","), quality.confirmed, quality.clashes, quality.pending)
    if HL then
        local name = (HL.EVENTS and HL.EVENTS.combat_quality) and "combat_quality" or "x_combat_quality"
        pcall(HL.event, name, f)
    end
    local pending = quality.pending
    quality = { claims = 0, accepted = 0, confirmed = 0, clashes = 0, rejected = {}, pending = pending }
end

-- --- owner side: replay hits on our real pawn -------------------------------

local function is_dead(w)
    local hp; pcall(function() hp = tonumber(w.Health) end)
    return hp ~= nil and hp <= 0
end

local death_dispatching = false   -- true while WE play a stand-in's death

-- Per-possessed-pawn bookkeeping (reset whenever the local pawn changes:
-- arena (re)load = new round).
local own = { addr = nil, at = 0, hits = 0, passes = 0, live_round = -1, forced_round = -1, died_round = -1 }

-- Willie_BP defaults, read once from the class default object.
local cdo_cache = nil
local function cdo_defaults()
    if cdo_cache then return cdo_cache end
    local cdo
    pcall(function() cdo = StaticFindObject(CDO_PATH) end)
    if not cdo or not cdo:IsValid() then return nil end
    local t, n = {}, 0
    for _, name in ipairs(RESTORE_FIELDS) do
        local v; pcall(function() v = tonumber(cdo[name]) end)
        if v then t[name] = v; n = n + 1 end
    end
    if n > 0 then cdo_cache = t end
    return cdo_cache
end

-- Heal the local pawn to the Willie_BP defaults: career-save limb wounds are
-- applied on spawn (normally healed by the Tavern innkeeper, which MP never
-- visits). Never touches the save (no GI "Reset Player Character").
local function restore_vitals(me, why)
    -- A number reset cannot rebuild a severed body. Fresh arena lives are
    -- prepared before possession by the director; keep injuries on this pawn.
    local severed = false
    pcall(function() severed = me.Headless == true end)
    pcall(function()
        local arr = me["Dismembered Array"]
        if arr then arr:ForEach(function() severed = true end) end
    end)
    if severed then
        Log("vitals restore skipped (%s): pawn has severed parts", why)
        return false
    end
    local d = cdo_defaults()
    if not d then
        Log("vitals restore skipped (%s): Willie_BP CDO not readable", why)
        return false
    end
    for _, fn in ipairs(RESTORE_FUNCS) do bp_call(me, fn) end
    local n = 0
    for name, v in pairs(d) do
        if pcall(function() me[name] = v end) then n = n + 1 end
    end
    local function g(k) local v; pcall(function() v = tonumber(me[k]) end); return v and string.format("%.0f", v) or "?" end
    Log("vitals restored to defaults (%s): head=%s neck=%s torso=%s/%s back=%s arms=%s/%s legs=%s/%s health=%s consciousness=%s [%d fields]",
        why, g("Head Health"), g("Neck Health"), g("Body Upper Health"), g("Body Lower Health"), g("Back Health"),
        g("Arm_R Health"), g("Arm_L Health"), g("Leg_R Health"), g("Leg_L Health"), g("Health"), g("Consciousness"), n)
    return true
end

local RESTORE_AT = { 0.5, 1.5, 3.0 }   -- s after possession (wounds may land late)
local function own_pawn_tick(me)
    local addr; pcall(function() addr = tostring(me:GetAddress()) .. "|" .. tostring(wname(me)) end)
    if addr ~= own.addr then
        own = { addr = addr, at = os.clock(), hits = 0, passes = 0, live_round = -1,
                forced_round = -1, died_round = -1 }
        Log("local pawn possessed (state=%s round=%d)", match_state, match_round)
        C3.ungate(me) -- before any incoming approved hit on the newly possessed pawn
    end
    if own.defeat_pending and os.clock()>=(own.defeat_retry_at or 0) then
        local ctx=C3.life_for(my_peer_id,true)
        local pending=own.defeat_pending
        if ctx and pending.pawn==addr_of(me) and pending.match_id==ctx.match_id
            and pending.round==ctx.round and pending.life==ctx.life then C3.on_native_defeat(me)
        else own.defeat_pending=nil end
    end
    -- Accepted network hits are not the only source of injury: falls, props
    -- and native combat can hurt before a claim arrives. End preparation as
    -- soon as this pawn enters play, and never heal it again during this life.
    local untouched = not match_live and own.live_round == -1
        and own.hits == 0 and not is_dead(me)
    local age = os.clock() - own.at
    if own.passes < #RESTORE_AT and age >= RESTORE_AT[own.passes + 1] then
        own.passes = own.passes + 1
        if untouched then restore_vitals(me, "spawn pass " .. own.passes) end
    end
    if match_live and own.live_round ~= match_round then
        own.live_round = match_round
        own.passes = #RESTORE_AT
    end
end

-- The server declared our death (a `death` record). Final for the
-- round: kill the pawn even if our own copy still thinks it's alive (e.g. the
-- window was in the background and the lethal hit wasn't applied yet).
-- HSMPMatch freezes input from the scoreboard's alive=false.
function C3.death_context(peer, d)
    local ctx = C3.life_for(peer, peer == my_peer_id)
    if not ctx then return false, "missing verified life context" end
    if not d.match_id or d.match_id == 0 or d.match_id ~= ctx.match_id then
        return false, "different match"
    end
    if d.round ~= ctx.round then return false, "different round" end
    if not d.life or d.life <= 0 or d.life ~= ctx.life then return false, "different life" end
    return true
end
local function force_own_death(round, killer, cause, context)
    if own.forced_round == round and own.forced_match_id == context.match_id
        and own.forced_life == context.life then return end
    local me = local_pawn()
    if not me then return end
    own.defeat_pending=nil
    if cause==5 or cause==6 then
        -- Permanent native defeat eliminates gameplay without killing its living pawn.
        own.forced_round,own.forced_match_id,own.forced_life=round,context.match_id,context.life
        own.died_round,own.died_match_id,own.died_life=round,context.match_id,context.life
        own.defeated={match_id=context.match_id,round=round,life=context.life}
        own.defeat_pending=nil
        Log("SERVER confirmed native defeat (round %d, killer=%s): native Health/KO retained",round,tostring(killer))
        return
    end
    -- Suppress a redundant report during the native Death callback, but keep
    -- failed application retryable until the pawn's dead state is observed.
    own.server_death_pending = {match_id=context.match_id,round=round,life=context.life}
    local hp; pcall(function() hp = tonumber(me.Health) end)
    local was_alive = hp == nil or hp > 0
    pcall(function() me.Health = 0 end)
    local how = "already dead"
    if was_alive then
        local ok = bp_call(me, "Death")
        if not ok then
            pcall(function() me["Force Death"] = true end)
            how = "Force Death flag"
        else
            how = "Death()"
        end
    end
    local completed = is_dead(me)
    if not completed then pcall(function() completed = me.DED == true end) end
    if completed then
        own.forced_round, own.forced_match_id, own.forced_life = round, context.match_id, context.life
        own.died_round, own.died_match_id, own.died_life = round, context.match_id, context.life
        own.server_death_pending = nil
    else
        how = how .. " (native application pending; retry allowed)"
    end
    Log("SERVER declared my death (round %d, killer=%d, cause=%d): hp was %s -> %s",
        round, killer or 0, cause or -1, tostring(hp), how)
end

-- Exact native loss boundary, distinct from recoverable Fallen/Consciousness zero.
function C3.on_native_defeat(selfp)
    local mode=HSESS and HSESS.mode and HSESS.mode()
    local w=pv(selfp)
    local me=local_pawn()
    if not (w and w:IsValid() and me and same(w,me) and combat_window) or is_dead(w) then return false end
    -- The owner requested native AI yield as surrender in dev AI duels. The
    -- nonplayer native lose branch (@256919) sets Give Up only; Give Up 2
    -- is player-only (@256461). Do not infer yield from Fallen/zero cap.
    local ai_yield=false
    if mode and mode.mode==0 and (os.getenv("HSMP_DEV") or "") == "1" then
        pcall(function()
            local c=w.Controller
            ai_yield=w.Player==false and c and c:IsValid()
                and c:GetClass():GetFName():ToString()=="AI_BP_C"
        end)
    end
    if not mode or (mode.mode~=5 and not ai_yield) then own.defeat_pending=nil;return false end
    local ctx=C3.life_for(my_peer_id,true)
    if not ctx then return false end
    local ok,confirmed=pcall(function()
        local gi,gm=w["GI Settings"],w["HS Game Mode"]
        return (w.Player==true or ai_yield) and w.DED~=true and w["Give Up"]==true
            and (ai_yield or w["Give Up 2 (Temp)"]==true)
            and gi and gi:IsValid() and tonumber(gi["Current Game Mode enum"])~=nil
            and tonumber(gi["Current Game Mode enum"])~=5 and gm and gm:IsValid() and gm["Match Won"]==false
    end)
    if not ok or not confirmed then return false end
    if own.defeated and own.defeated.match_id==ctx.match_id and own.defeated.round==ctx.round
        and own.defeated.life==ctx.life then return true end
    local row=mode and mode.rows and mode.rows[my_peer_id]
    if mode and mode.match_id==ctx.match_id and mode.round==ctx.round
        and row and row.life==ctx.life and row.alive==false then
        own.defeat_pending=nil
        return true
    end
    local pending=own.defeat_pending
    if pending and (pending.pawn~=addr_of(w) or pending.match_id~=ctx.match_id
        or pending.round~=ctx.round or pending.life~=ctx.life) then
        own.defeat_pending=nil;own.defeat_retry_at=nil
    end
    if own.defeat_pending and os.clock()<(own.defeat_retry_at or 0) then return false end
    own.defeat_retry_at=os.clock()+1
    own.defeat_pending={match_id=ctx.match_id,round=ctx.round,life=ctx.life,pawn=addr_of(w)}
    if not send_rec("death_report",{match_id=ctx.match_id,round=ctx.round,life=ctx.life,reason=ai_yield and 2 or 1}) then return false end
    own.defeat_reported={match_id=ctx.match_id,round=ctx.round,life=ctx.life}
    -- IPC acceptance is not authoritative gameplay acknowledgement; retain
    -- the original event proof and retry until its current life is eliminated.
    local cap,hp;pcall(function()cap=w["Consciousness Cap"];hp=w.Health end)
    Log("native Event Lose Match VERIFIED pawn=%s match=%s round=%s life=%s Health=%s ConsciousnessCap=%s -> %s reported",
        tostring(wname(w)),tostring(ctx.match_id),tostring(ctx.round),tostring(ctx.life),tostring(hp),tostring(cap),ai_yield and "AI yield surrender" or "defeat")
    return true
end

-- Native Death/Dying fired on some Willie.
function on_native_death(selfp)   -- assigns the forward-declared local
    local w = pv(selfp)
    if not w or not w:IsValid() then return end
    -- Dying also enters the native downed/loss flow. Only observed
    -- biological death may produce a death report or replace its baseline.
    local ded=false;pcall(function() ded=w.DED==true end)
    if not ded and not is_dead(w) then return end
    local me = local_pawn()
    if me and same(w, me) then
        -- (a hook callback: Death has run) a later echo's undo must not
        -- restore the living pawn of the last baseline
        C3.baseline(me)
        if combat_window then
            local ctx = C3.life_for(my_peer_id, true)
            if not ctx then return end
            local pending = own.server_death_pending
            if pending and pending.match_id == ctx.match_id and pending.round == ctx.round
                and pending.life == ctx.life then return end
            if own.died_round == ctx.round and own.died_match_id == ctx.match_id
                and own.died_life == ctx.life then return end
            if send_rec("death_report", { match_id = ctx.match_id, round = ctx.round, life = ctx.life }) then
                own.died_round, own.died_match_id, own.died_life = ctx.round, ctx.match_id, ctx.life
                Log("my pawn's native death fired (round %d) -> reported to server", match_round)
            else
                Log("my pawn's native death fired (round %d) while not connected: not reported (scoped HSMPSync fallback retries)", match_round)
            end
        end
        return
    end
    local peer = puppet_peer[wname(w) or ""]
    if peer and not death_dispatching then
        Log("WARNING: stand-in of peer %d ran its native death locally (not server-declared)", peer)
    end
end

local replay_err_logged = false

-- The component a native hit names as "Hit Component": the Willie's Mesh
-- (CharacterMesh0, tagged ProxyCollision; Deal Complex Damage maps the hit
-- point from ITS bone space into the proxy skeleton). SK_Skeleton only as a
-- fallback.
function C3.hit_mesh(w)
    local m; pcall(function() m = w.Mesh end)
    if m and m:IsValid() then return m end
    return body_mesh(w)
end

-- A `damage` / `damage_in` / `hitfx_in` record's vector field (3 numbers).
function C3.v3(d, k)
    local t = type(d[k]) == "table" and d[k] or {}
    return { num(t[1]), num(t[2]), num(t[3]) }
end

-- Deal Complex Damage arguments of an armour-stage hit record (the `damage`
-- layout, see send_claim) on mesh `mesh` with geometry `g` (C3.hit_geo) and
-- striking component `coll` (nil when unknown): 17 inputs + the 6 out-param
-- slots (inside the table: `f(table.unpack(t), x)` would truncate the unpack).
function C3.dcd_args(d, mesh, g, coll, hit_box)
    local function V(t) return { X = t[1], Y = t[2], Z = t[3] } end
    local bone = type(d.bone) == "string" and d.bone or ""
    local dism_all = math.floor(num(d.dism_blunt))
    local kick = (math.floor(dism_all / 256) % 256) / 10   -- 0 stays 0
    return {
        mesh, coll, FName(bone ~= "" and bone or "pelvis"), V(g.at), V(g.nrm),
        V(g.vel), V(g.imp), num(d.cutting_power), num(d.pain_rate),
        num(d.damage_out), dism_all % 256, math.floor(dism_all / 65536) % 2 == 1, math.floor(dism_all / 67108864) % 2 == 1,
        kick, hit_box, math.floor(dism_all / 131072) % 2 == 1, num(d.draw_cut),
        {}, {}, {}, {}, {}, {},
    }
end

-- The blow's geometry on `w`: the hit point at the record's offset on w's own
-- bone, and normal / velocity / impulse turned with that bone (BF.LOCAL
-- records), so the blow lands where it landed on the attacker's screen
-- relative to the body, however w has moved or turned since.
function C3.hit_geo(w, mesh, d)
    local bone = type(d.bone) == "string" and d.bone or ""
    local off, loc = C3.v3(d, "offset"), C3.v3(d, "location")
    local g = { at = loc, nrm = C3.v3(d, "normal"), vel = C3.v3(d, "velocity"), imp = C3.v3(d, "impulse") }
    if math.floor(num(d.flags) / BF.LOCAL) % 2 == 1 then
        local fr = BF.of(mesh, bone) or BF.of(body_mesh(w), bone)
        if fr then
            g.at = BF.to_world(fr, off)
            g.nrm, g.vel, g.imp = BF.rot(fr.q, g.nrm), BF.rot(fr.q, g.vel), BF.rot(fr.q, g.imp)
            g.frame = true
            g.bone_frame=fr
        else
            g.local_lost = true   -- magnitudes still right, directions not
        end
        return g
    end
    local bp = bone_pos(mesh, bone) or bone_pos(body_mesh(w), bone)
    if bp then g.at = { bp[1] + off[1], bp[2] + off[2], bp[3] + off[3] } end
    return g
end
function C3.hit_point(w, mesh, d) return C3.hit_geo(w, mesh, d).at end

-- A mode assignment is a life only after its exact spawn order placed this
-- native pawn. Death/respawn countdown does not change that identity; only
-- a new generation does. Never relabel old messages from current Mode.
function C3.life_for(peer, require_placement)
    local mode = HSESS and HSESS.mode and HSESS.mode()
    local view = HSESS and HSESS.view and HSESS.view()
    peer = tonumber(peer)
    local row = mode and mode.rows and mode.rows[peer]
    if not row or not view or mode.match_id ~= view.match_id or mode.round ~= view.round
        or not row.life or row.life < 1 then return nil end
    if require_placement then
        if row.respawning then return nil end -- Preparation vitals use their separate publication context.
        local pawn = local_pawn()
        local ipc = rawget(_G,"HSMP_IPC")
        local status = ipc and ipc.bus_table("spawn_status")
        local order = view.spawns and view.spawns[peer]
        if not pawn or not status or status.verified ~= true or status.pawn ~= wname(pawn)
            or status.round ~= mode.round or status.match_id ~= mode.match_id or status.life ~= row.life
            or not order or status.spawn_id ~= order.spawn_id then return nil end
    end
    return { match_id = mode.match_id, round = mode.round, life = row.life }
end

-- Refresh wrappers before native replay/source access. Actor names survive in
-- the puppet registry, but a cached UObject wrapper may already be freed.
function C3.remote_actor(peer)
    for _,actor in ipairs(FindAllOf("Willie_BP_C") or {}) do
        local name=wname(actor)
        if name and puppet_peer[name]==peer then return actor end
    end
    return nil
end

-- Bind a remote actor to the sample actually applied to it, never to Mode's
-- next assigned generation while the old body still occupies the screen.
-- New claims need a fresh sample. Already approved delayed trades need only
-- the immutable actor-generation binding, including its frozen corpse pose.
function C3.displayed_for(peer,actor,require_fresh)
    if not actor then return nil end
    local context=C3.life_for(peer,false)
    if not context then return nil end
    local ipc=rawget(_G,"HSMP_IPC")
    local pb=ipc and ipc.bus_table("playback")
    if type(pb)~="table" then return nil end
    local name=wname(actor)
    for _,row in ipairs(pb.rows or {}) do
        if row.peer==peer and row.pawn==name and row.match_id==context.match_id
            and row.round==context.round and row.life==context.life
            and type(row.local_ms)=="number"
            and (require_fresh==false or math.abs(now_ms()-row.local_ms)<=250) then
            return {match_id=row.match_id,round=row.round,life=row.life}
        end
    end
    return nil
end

local FootReplay = load_module("replay_feet").new({ UEHelpers = UEHelpers, log = Log })
local BoxReplay = CuttingBox.new({UEHelpers=UEHelpers,log=Log,bf=BF})
C3.fist_replay=load_module("replay_fists").new({UEHelpers=UEHelpers,log=Log,
    world_key=function()return WG.key end,
    current_generation=function(peer)return C3.life_for(peer,false) end,
    current_actor=function(peer)
        local pawn=peer==my_peer_id and local_pawn() or C3.remote_actor(peer)
        local original=pawn and (peer==my_peer_id and C3.life_for(peer,true) or C3.displayed_for(peer,pawn,false))
        return original and pawn or nil,original
    end,
    context=function(pawn)
        if same(pawn,local_pawn()) then return C3.life_for(my_peer_id,true) end
        return C3.displayed_for(puppet_peer[wname(pawn)],pawn,false)
    end})

-- The component that struck, as this screen has it: the exact native weapon
-- collision-array slot for new BF.WEAPON records, else its body mesh.
-- Ordinal zero from older senders retains the original first-component fallback.
-- The attacker is my own pawn or its stand-in here. nil when not found.
function C3.hitter(attacker, d)
    local a
    if attacker and attacker == C3.me_id then a = local_pawn() else a = C3.remote_actor(attacker) end
    if not (a and a:IsValid()) then return nil,"attacker_pawn_unavailable" end
    if math.floor(num(d.flags) / BF.WEAPON) % 2 == 1 then
        local meta = math.floor(num(d.dism_blunt))
        local fist = math.floor(meta / BF.FIST) % 2 == 1
        local feet = math.floor(meta / BF.FEET) % 2 == 1
        local ordinal = math.floor(meta / BF.COMPONENT) % 16
        local left, right = math.floor(meta / BF.LEFT) % 2 == 1, math.floor(meta / BF.RIGHT) % 2 == 1
        -- Modern ordinals identify a component only within its original hand.
        -- A dropped actor cannot be represented by selecting another held
        -- weapon with the same array slot. Ordinal zero remains legacy.
        if ordinal == 0 or left == right then return nil,"invalid_source_identity" end
        local fields
        if feet then fields = left and { "Foot L Weapon" } or right and { "Foot R Weapon" } or {}
        else fields = left and { "Weapon L" } or right and { "Weapon R" } or { "Weapon R", "Weapon L" } end
        local reasons={}
        for _, k in ipairs(fields) do
            local c
            local reason="component_missing"
            local ok,err=pcall(function()
                local wp = a[k]
                if not (wp and wp:IsValid()) then
                    reason="held_actor_missing:pawn="..tostring(wname(a))
                    if not feet then
                        local alias=a[k.."_0"]
                        reason=reason..":secondary="..tostring(alias and alias:IsValid() and wname(alias) or "none")
                    end
                    if feet then c = FootReplay.component(a, left, ordinal) end
                    return
                end
                if type(d.source_class)=="string" and d.source_class~=""
                    and wp:GetClass():GetFName():ToString()~=d.source_class then reason="class_mismatch:"..wp:GetClass():GetFName():ToString();return end
                -- Preserve the source hand and native pseudo-weapon. A fist
                -- beside a sword must never replay using the sword's component.
                if fist or feet or left or right then
                    local cls = wp:GetClass():GetFName():ToString()
                    local is_fist = cls:find("Fists", 1, true) ~= nil
                    local is_feet = cls:find("Weapon_Feet", 1, true) ~= nil
                    if is_fist ~= fist or is_feet ~= feet then reason="native_striker_kind_mismatch:"..cls;return end
                end
                local arr = wp["Collision Components Array"]
                if arr then
                    local n = 0
                    arr:ForEach(function(_, e)
                        n = n + 1
                        if (ordinal == 0 and not c) or n == ordinal then c = e:get() end
                    end)
                    reason="pawn:"..tostring(wname(a))..":weapon:"..tostring(wname(wp))..":array_count:"..n..":selected:"..tostring(c and c:IsValid())
                else reason="collision_array_missing" end
                if ordinal == 0 and not (c and c:IsValid()) then c = wp:K2_GetRootComponent() end
            end)
            if c and c:IsValid() then return c end
            reasons[#reasons+1]=k..":"..(ok and reason or ("native_lookup_error:"..tostring(err)))
        end
        if fist and not feet and d.source_class=="Weapon_Fists_C" then
            -- A native fist is temporary. An approved historical punch must
            -- retain that source even after its actor was dropped/replaced.
            -- This separate collision-disabled source never replaces a hand.
            local c=C3.fist_replay.component(a,left,ordinal,
                {match_id=d.match_id,round=d.round,life=d.attacker_life,peer_id=attacker})
            if c and c:IsValid() then return c end
        end
        return nil,table.concat(reasons,";")
    end
    local m; pcall(function() m = a.Mesh end)
    if m and m:IsValid() then return m end
    return nil
end

function C3.hit_box(attacker, d, collided)
    local meta = math.floor(num(d.dism_blunt))
    local ordinal = (meta >> 27) & 15
    if ordinal == 0 then return nil end
    if not NativeModules or not collided or not collided:IsValid() then return nil end
    local owner=collided:GetOwner()
    local source=(meta >> 21)&15
    local box=NativeModules.find(owner,ordinal,source)
    local ok, exact = pcall(function()
        return box and box:IsValid() and collided and collided:IsValid()
            and same(box:GetOwner(), collided:GetOwner())
            and box:GetClass():GetFName():ToString() == "BoxComponent"
    end)
    return ok and exact and box or nil
end

-- Get Damage's gate: a blow on the bone of the last blow that passed it, while
-- Last Damage Taken is still set (RetriggerableDelay 0.2 s), needs DRS >= that
-- value × (Draw Cut + 1). C3.gd_last is the last replayed blow that passed:
-- { attacker, ats (its clock), bone, name, ldt }. Returns the gate state
-- before the call.
C3.gd_last = nil
C3.diag_counts,C3.diag_last={},{}
function C3.diag_length(v)return math.sqrt(v[1]^2+v[2]^2+v[3]^2)end
function C3.replay_diag(kind,fmt,...)
    local n=(C3.diag_counts[kind] or 0)+1;C3.diag_counts[kind]=n
    local at=now_ms()
    if n<=4 or at-(C3.diag_last[kind] or -1e9)>=1000 then
        C3.diag_last[kind]=at
        Log("REPLAY_DIAG kind=%s count=%d "..fmt,kind,n,...)
    end
end
function C3.gd_gate_open(me, attacker, d, bone)
    local ats = math.tointeger(tonumber(d.attacker_ts)) or 0
    local b = bone:lower()
    local gl = C3.gd_last
    local keep = gl ~= nil and gl.attacker == attacker and gl.bone == b and ats > 0
        and gl.pawn == addr_of(me) and gl.match_id == d.match_id and gl.round == d.round
        and gl.attacker_life == d.attacker_life and gl.victim_life == d.victim_life
        and ats >= gl.ats and ats - gl.ats < BF.GD_GATE_MS
    if not keep then
        pcall(function() me["Last Damage Taken"] = 0 end)
    elseif gl.ldt then
        -- Solo still has the gate set here, but the replays may be further
        -- apart on my clock than 0.2 s (jitter, a parry hold): the game's own
        -- reset has cleared it by now. Put the value of that blow back.
        pcall(function() me["Last Damage Taken"] = gl.ldt end)
        pcall(function() me["Last Damaged Bone"] = FName(gl.name or bone) end)
    end
    local s = {kept=keep,previous_ats=gl and gl.ats,age=gl and ats-gl.ats}
    pcall(function() s.ldt = tonumber(me["Last Damage Taken"]) end)
    pcall(function() s.ldb = me["Last Damaged Bone"]:ToString():lower() end)
    pcall(function() s.sustained = tonumber(me["Sustained Damage"]) end)
    pcall(function() s.invulnerable=me.Invulnerable;s.fallen=me.Fallen;s.consciousness=me.Consciousness end)
    pcall(function() s.damage_rate=me["Damagr Rste Var"] end)
    return s
end
function C3.gd_gate_note(me, attacker, d, bone, s0)
    local ldt, ldb, sustained
    pcall(function() ldt = tonumber(me["Last Damage Taken"]) end)
    pcall(function() ldb = me["Last Damaged Bone"]:ToString():lower() end)
    pcall(function() sustained = tonumber(me["Sustained Damage"]) end)
    local b = bone:lower()
    -- Equal-strength contacts pass the native >= gate and retrigger its delay
    -- even though Last Damage Taken does not change. Sustained Damage records
    -- that pass, including light contacts that cause no Health loss.
    local passed_equal = sustained ~= nil and s0.sustained ~= nil and sustained > s0.sustained
    if ldb == b and (ldt ~= s0.ldt or s0.ldb ~= b or passed_equal) then
        C3.gd_last = { attacker = attacker, ats = math.tointeger(tonumber(d.attacker_ts)) or 0, bone = b,
                       name = bone, ldt = ldt, pawn = addr_of(me), match_id = d.match_id, round = d.round,
                       attacker_life = d.attacker_life, victim_life = d.victim_life }
    end
end

-- A hit on MY pawn (`damage_in` record `d`; the attacker is the entry's peer).
local function apply_hit(d, _attacker)
    if type(d) ~= "table" then return "hit dropped: no record",3 end
    local ctx = C3.life_for(my_peer_id,true)
    if not ctx or d.match_id ~= ctx.match_id or d.round ~= ctx.round or d.victim_life ~= ctx.life then
        return "hit dropped: stale match or pawn life",3
    end
    local src_ctx = _attacker==my_peer_id and C3.life_for(_attacker,true)
        or C3.displayed_for(_attacker,C3.remote_actor(_attacker),false)
    if not src_ctx or d.match_id ~= src_ctx.match_id or d.round ~= src_ctx.round or d.attacker_life ~= src_ctx.life then
        return "hit dropped: stale attacker pawn life",3
    end
    local hr = math.tointeger(tonumber(d.round)) or 0
    if hr == 0 then hr = nil end   -- 0 = not stamped
    if not combat_window or (hr and hr ~= match_round) then refresh_match() end
    if not combat_window then return "hit dropped: round not live",3 end
    if hr and hr ~= match_round then
        return string.format("hit dropped: stale round %d (now %d)", hr, match_round),3
    end
    local me = local_pawn()
    if not me then return "no pawn",5 end
    if is_dead(me) then return "already dead",5 end
    local bone = type(d.bone) == "string" and d.bone or ""
    local mesh = C3.hit_mesh(me)
    local geo = C3.hit_geo(me, mesh, d)
    if geo.local_lost then return "hit dropped: victim bone frame unavailable",4 end
    local at = geo.at
    local flags = math.floor(num(d.flags))
    local function bit(n) return math.floor(flags / n) % 2 == 1 end
    local function V(t) return { X = t[1], Y = t[2], Z = t[3] } end
    local complex = bit(FLAG_COMPLEX)
    if complex and bit(1) and num(d.parent_cid)==0 then
        return "native contact origin acknowledged (DCD gate suppressed damage)",2
    end
    local dism_all = math.floor(num(d.dism_blunt))
    local before = snapshot(me)
    local coll,source_reason = C3.hitter(_attacker, d)
    if (complex or num(d.parent_cid)~=0) and math.floor(dism_all / BF.COMPONENT) % 16 ~= 0 and not coll then
        C3.replay_diag("source4","peer=%s hit=%s class=%s meta=%s reason=%s",
            tostring(_attacker),tostring(d.hit_id),tostring(d.source_class),tostring(dism_all),tostring(source_reason))
        return "hit dropped: striking component unavailable ("..tostring(source_reason)..")",4
    end
    local hit_box = C3.hit_box(_attacker, d, coll)
    if complex and ((dism_all >> 27) & 15) ~= 0 and not hit_box then
        return "hit dropped: native cutting HitBox unavailable",4
    end
    if hit_box then
        hit_box=BoxReplay.component(me,d.hit_box_frame,geo.bone_frame,tostring(_attacker)..":"..tostring(dism_all & (BF.LEFT|BF.RIGHT)))
        if not hit_box then return "hit dropped: historical cutting Box unavailable",4 end
    end

    -- Native replay: my own armour, wounds, bleeding, dismemberment, death.
    replaying = true
    local ok, err, via = false, nil, "Get Damage"
    local gate0
    C3.replay_trace={pawn=addr_of(me),calls=0}
    if complex then
        -- Deal Complex Damage's own contact gate already ran on the attacker's
        -- screen (only calls that passed it are claimed): open it here.
        pcall(function() me["Last Complex Damage Impulse"] = 0 end)
        -- Get Damage's per-bone gate as solo would see it: kept between blows
        -- of one attacker less than 0.2 s apart on ITS clock, reset otherwise
        -- (replays arrive bunched: parry holds, jitter, resends).
        gate0 = C3.gd_gate_open(me, _attacker, d, bone)
        C3.body_replay=C3.body_replay_meta(d,_attacker)
        if C3.body_audit then pcall(C3.body_audit.capture,me,"Deal Complex Damage invocation PRE",C3.body_replay) end
        ok, err = bp_call(me, "Deal Complex Damage", table.unpack(C3.dcd_args(d, mesh, geo, coll, hit_box), 1, 23))
        if C3.body_audit then pcall(C3.body_audit.capture,me,"Deal Complex Damage invocation POST",C3.body_replay) end
        C3.body_replay=nil
        C3.gd_gate_note(me, _attacker, d, bone, gate0)
        via = "Deal Complex Damage"
        -- Never a second application. A call that errored AFTER the
        -- BP ran (e.g. copying out-params) already applied the hit: keep it.
        -- One that did nothing is DROPPED (an armour-stage raw is a speed,
        -- never a Get Damage input).
        if not ok then
            if next(diff(before, snapshot(me))) ~= nil then
                ok, via = true, "Deal Complex Damage (errored after applying)"
            else
                replaying = false
                C3.replay_trace=nil
                Log("armour-stage replay failed (%s): claim dropped, never re-applied through Get Damage", tostring(err))
                return "hit dropped: armour-stage replay failed (" .. tostring(err) .. ")",6
            end
        end
    else
        -- Legacy claim (no FLAG_COMPLEX): Get Damage with the claim's raw.
        local args = {
            V(C3.v3(d, "impulse")), V(C3.v3(d, "velocity")), V(at), V(C3.v3(d, "normal")),
            FName(bone ~= "" and bone or "pelvis"),
            num(d.raw_damage), num(d.cutting_power), bit(1), mesh,
            dism_all % 256, bit(2), bit(4), num(d.parent_cid)~=0 and coll or nil, bit(8), nil,
            num(d.pain_rate), bit(16), num(d.draw_cut), {},
        }
        C3.body_replay=C3.body_replay_meta(d,_attacker)
        if C3.body_audit then pcall(C3.body_audit.capture,me,"Get Damage invocation PRE",C3.body_replay) end
        ok, err = bp_call(me, "Get Damage", table.unpack(args, 1, 19))
        if C3.body_audit then pcall(C3.body_audit.capture,me,"Get Damage invocation POST",C3.body_replay) end
        C3.body_replay=nil
        if not ok and err ~= "unresolved" then
            if next(diff(before, snapshot(me))) ~= nil then
                ok, via = true, "Get Damage (errored after applying)"
            end -- A copy-out error can occur after native effects: never call twice.
        end
    end
    replaying = false
    local trace=C3.replay_trace;C3.replay_trace=nil
    if C3.native_probe then
        C3.log_native_evidence("replay",d,_attacker,
            "last_zone="..C3.native_zone(me).." "..(trace and trace.dcd or "dcd=unavailable")..
            " dcd_calls="..tostring(trace and trace.dcd_calls or 0)..
            " gd_calls="..tostring(trace and trace.calls or 0)..
            " gd="..(trace and trace.samples and table.concat(trace.samples,";") or "unavailable")..
            " gd_truncated="..tostring(trace and trace.truncated==true)..
            " trace_hook="..(C3.trace_hook_ok and "registered" or "unavailable")..
            " trace_calls="..tostring(trace and trace.proxy_trace_calls or 0)..
            " trace="..(trace and trace.proxy_trace_samples and table.concat(trace.proxy_trace_samples,";") or "unavailable")..
            " trace_truncated="..tostring(trace and trace.proxy_trace_truncated==true))
    end
    C3.baseline(me)   -- an echo later this frame must not undo this hit
    if not ok and not replay_err_logged then
        replay_err_logged = true
        Log("native damage replay failed: %s", tostring(err))
    end
    own.hits = own.hits + 1
    local after = snapshot(me)
    local res = string.format("native %s(%s) dmg Health %s [%s]",
        via, ok and "ok" or "failed",
        (before[1] and after[1]) and string.format("%+.2f", after[1] - before[1]) or "?",
        fmt_fields(diff(before, after)))
    if after[1] and after[1] <= 0 then res = res .. " LETHAL" end
    local changes = diff(before,after)
    local mask = 0
    for i in pairs(changes) do if i<=24 then mask=mask | (1 << (i-1)) end end
    local hp = before[1] and after[1] and after[1]-before[1] or 0
    if ok and mask==0 then
        C3.replay_diag("gate2","peer=%s hit=%s bone=%s ats=%s kept=%s prior_age=%s prior_ldt=%s prior_bone=%s native_calls=%s native_raw=%s native_draw=%s native_applied=%s invulnerable=%s fallen=%s consciousness=%s damage_rate=%s source_vel=%.1f source_imp=%.1f rig=%s",
            tostring(_attacker),tostring(d.hit_id),bone,tostring(d.attacker_ts),tostring(gate0 and gate0.kept),
            tostring(gate0 and gate0.age),tostring(gate0 and gate0.ldt),tostring(gate0 and gate0.ldb),
            tostring(trace and trace.calls),tostring(trace and trace.raw),tostring(trace and trace.draw),
            tostring(trace and trace.applied),tostring(gate0 and gate0.invulnerable),tostring(gate0 and gate0.fallen),
            tostring(gate0 and gate0.consciousness),tostring(gate0 and gate0.damage_rate),C3.diag_length(C3.v3(d,"velocity")),C3.diag_length(C3.v3(d,"impulse")),tostring(d.damage_out))
    end
    -- Fresh diagnostic availability is independent of the production changed
    -- mask. A successfully read unchanged Health is an actual native zero;
    -- unread fields remain absent. This fifth value is never cached/transported.
    return res, ok and (mask~=0 and 1 or 2) or 6, ok and mask or 0, ok and hp or 0,
        before[1] and after[1] and hp or nil
end

local ReplayAttempts = load_module("replay_attempts").new({ now=os.clock,send=function(r) return send_rec("replay_outcome",r) end })

function C3.log_replay_probe(d,attacker,outcome,text,fresh,native_health)
    if not fresh or not C3.native_probe then return end
    local fields=type(text)=="string" and text:match("%[([^%]]*)%]") or "native fields unavailable"
    -- ReplayOutcome's observed_fields describes changed native fields, not
    -- snapshot readability. Only this fresh callback's native reads qualify.
    local hp=native_health
    if type(hp)=="number" and hp==hp and math.abs(hp)<math.huge then
        Log("LAB_REPLAY attacker=%d cid=%d parent_cid=%d bone=%s dmg Health %+.6f [%s]",
            attacker,num(d.hit_id),num(d.parent_cid),tostring(d.bone),hp,fields)
    else
        Log("LAB_REPLAY attacker=%d cid=%d parent_cid=%d bone=%s dmg Health unavailable [native Health not observed]",
            attacker,num(d.hit_id),num(d.parent_cid),tostring(d.bone))
    end
end


-- --- vitals + stand-in life ---------------------------------------------------

-- Owner side: sample OUR pawn, write the `vitals` record on change (per-field
-- deadband) or every VITALS_BEAT_S. The sidecar applies the network policy
-- (rate cap, keyframes; combat_client.rs VitalsGate).
local function fmt_num(x) return x and string.format("%.3f", x) or "?" end

-- The `vitals` record (schema/combat.rs Vitals), quantised ONCE here:
-- v[i] = floor(x * 64 + 0.5) clamped to the field's max (health-type 0..12:
-- 150 * 64, stamina / exhaustion 13..14: 200 * 64, the rest 65534), 65535 =
-- unknown; dism = bit S.ENUMS.dism_part[NAME] per severed bone; flags = VF bits.
local VQ = { UNKNOWN = 65535, SCALE = 64, cache = {} }
function VQ.qmax(i)   -- 1-based VITALS index
    if i <= 13 then return 9600 elseif i <= 15 then return 12800 end
    return 65534
end
function VQ.q(i, x)
    if type(x) ~= "number" or x ~= x or x == math.huge or x == -math.huge then return VQ.UNKNOWN end
    local q = math.floor(math.max(x, 0) * VQ.SCALE + 0.5)
    local m = VQ.qmax(i)
    return q > m and m or q
end
function VQ.dq(q)
    q = math.tointeger(q)
    if q == nil or q == VQ.UNKNOWN then return nil end
    return q / VQ.SCALE
end
function VQ.enums()
    local ipc = rawget(_G, "HSMP_IPC")
    local S = ipc and ipc.S
    return (S and S.ENUMS and S.ENUMS.dism_part) or {}, (S and S.ENUM_NAMES and S.ENUM_NAMES.dism_part) or {}
end
function VQ.record(seq, s)
    local v = {}
    for i = 1, #VITALS do v[i] = VQ.q(i, s.v[i]) end
    local bits = VQ.enums()
    local dism = 0
    for _, n in ipairs(s.dism or {}) do
        local b = bits[string.upper(n)]
        if b then dism = dism | (1 << b) end
    end
    return { seq = seq, dism = dism, flags = s.f & 4095, v = v }
end
-- A `vitals` / `peer_vitals` record -> {seq, hp, dead, f, v (1-based floats,
-- nil = unknown), dism (lower-case bone names)}; dead = DEAD flag or Health <= 0.
function VQ.frame(t)
    if type(t) ~= "table" then return nil end
    local f = math.tointeger(tonumber(t.flags)) or 0
    local r = { seq = math.tointeger(tonumber(t.seq)) or 0, f = f, v = {}, dism = {},
        match_id = t.match_id, round = t.round, life = t.life }
    local src = type(t.v) == "table" and t.v or {}
    for i = 1, #VITALS do r.v[i] = VQ.dq(src[i]) end
    r.hp = r.v[1]
    r.dead = f & 1 ~= 0 or (r.hp ~= nil and r.hp <= 0)
    local mask = math.tointeger(tonumber(t.dism)) or 0
    if mask ~= 0 then
        local _, names = VQ.enums()
        for i = 0, 22 do
            if mask & (1 << i) ~= 0 and names[i] then r.dism[#r.dism + 1] = string.lower(names[i]) end
        end
    end
    return r
end

local SeverProjection=load_module("native_sever_projection")
local dism_projection=SeverProjection and SeverProjection.new()
local dism_cache, dism_tick, dism_owner = {}, -999, nil
function C3.sever_projection_context(me)
    if not WG.check() or not WG.settled() or not me or not same(me,local_pawn()) then return nil end
    local c=C3.vitals_context and C3.vitals_context(me)
    if not c then return nil end
    local ok,r=pcall(function()
        local world,mesh=WG.world(),me.Mesh
        if not world or not world:IsValid() or not mesh or not mesh:IsValid()
            or not same(world,me:GetWorld()) or not same(world,mesh:GetWorld()) then return nil end
        return {world=tostring(world:GetAddress()).."@"..world:GetFullName(),peer=my_peer_id,
            match_id=c.match_id,round=c.round,life=c.life,pawn=wname(me),actor=addr_of(me),
            mesh=addr_of(mesh),mesh_name=mesh:GetFName():ToString()}
    end)
    return ok and r or nil
end
local function sever_scope_same(a,b)
    if not a or not b then return false end
    for _,k in ipairs({"world","peer","match_id","round","life","pawn","actor","mesh","mesh_name"})do
        if a[k]~=b[k] then return false end
    end
    return true
end
local function sample_own_vitals(me,publication)
    local identity=C3.sever_projection_context(me)
    if identity and not sever_scope_same(dism_owner,identity) then
        dism_cache, dism_tick, dism_owner = {}, -999, identity
        if dism_projection then dism_projection.reset() end
    end
    local s = { v = {}, f = 0, dism = nil }
    for i, d in ipairs(VITALS) do
        local x; pcall(function() x = tonumber(me[d[1]]) end)
        s.v[i] = x            -- nil = unreadable on this build
    end
    for _, fl in ipairs(VFLAGS) do
        local b = false; pcall(function() b = me[fl[1]] == true end)
        if b then s.f = s.f | fl[2] end
    end
    if s.v[1] and s.v[1] <= 0 and s.f % 2 == 0 then s.f = s.f + 1 end   -- dead
    if tick_num - dism_tick >= DISM_TICKS then
        dism_tick = tick_num
        if dism_projection and C3.topology_audit and identity then
            local function guard(expected)
                local fresh=C3.sever_projection_context(me)
                return sever_scope_same(expected,fresh) and me["Dismemberment In Process"]==false
            end
            local ok,names,available,reason=pcall(function()
                local topology
                if guard(identity) then
                    topology=C3.topology_audit.read(me,{context=C3.sever_projection_context,unwrap=pv})
                end
                return dism_projection.observe(identity,topology,{guard=guard,
                    hidden=function(bone,expected)
                        if not guard(expected) then return nil end
                        return me.Mesh:IsBoneHiddenByName(FName(bone))
                    end})
            end)
            if ok then
                dism_cache=names
                C3.sever_projection_available=available
                C3.sever_projection_reason=reason
            end
        end
    end
    -- Preserve prior evidence internally across unavailable Mesh reads, but
    -- never stamp an old body's missing limbs onto a newly assigned life.
    local outbound=publication or (C3.vitals_context and C3.vitals_context(me))
    local cached=dism_owner and outbound and dism_owner.peer==my_peer_id and dism_owner.match_id==outbound.match_id
        and dism_owner.round==outbound.round and dism_owner.life==outbound.life
    if cached and not identity then
        local ok,current=pcall(function()
            local world=WG.world()
            return WG.settled() and world and world:IsValid() and me:IsValid()
                and same(world,me:GetWorld()) and addr_of(me)==dism_owner.actor
                and wname(me)==dism_owner.pawn
                and tostring(world:GetAddress()).."@"..world:GetFullName()==dism_owner.world
        end)
        cached=ok and current==true
    end
    s.dism = cached and dism_cache or {}
    return s
end

local function vitals_changed(a, b)   -- a = new sample, b = last written
    if not b then return true end
    if a.f ~= b.f or table.concat(a.dism, ",") ~= table.concat(b.dism, ",") then return true end
    for i, d in ipairs(VITALS) do
        local x, y = a.v[i], b.v[i]
        if (x == nil) ~= (y == nil) then return true end
        if x and math.abs(x - y) >= d[2] then return true end
    end
    return false
end

local vout = { seq = 0, last = nil, at = -1e9, writes = 0, samples = 0 }
-- Preparation needs the newly placed native owner's vitals before Live has
-- installed its Mode life. This publication context does not authorize hits,
-- deaths or replays: those continue to require C3.life_for's current Mode.
function C3.vitals_context(me)
    if not me or not same(me,local_pawn()) then return nil end
    local current=C3.life_for(my_peer_id,true)
    if current then return current end
    local ipc=rawget(_G,"HSMP_IPC")
    local status=ipc and ipc.bus_table("spawn_status")
    local view=HSESS and HSESS.view and HSESS.view()
    local phase=view and view.state
    local order=view and view.spawns and view.spawns[my_peer_id]
    if not status or status.verified~=true or status.pawn~=wname(me)
        or not view or not status.match_id or status.match_id==0 or status.match_id~=view.match_id
        or not order or status.spawn_id~=order.spawn_id then return nil end
    if phase=="live" then
        local mode=HSESS and HSESS.mode and HSESS.mode()
        local row=mode and mode.rows and mode.rows[my_peer_id]
        if mode and mode.id=="deathmatch" and row and row.respawning==true
            and mode.match_id==status.match_id and mode.round==view.round and status.round==view.round
            and row.life==status.life and status.life>=1 then
            return {match_id=status.match_id,round=status.round,life=status.life}
        end
        return nil
    end
    if phase~="loading" and phase~="countdown" then return nil end
    if status.round~=view.spawn_round or status.life~=1 then return nil end
    if status.round~=view.pending_round then return nil end
    return {match_id=status.match_id,round=status.round,life=status.life}
end
local function publish_own_vitals(me)
    local ctx = C3.vitals_context(me)
    if not ctx then return false end
    local key = tostring(ctx.match_id)..":"..ctx.round..":"..ctx.life
    local s = sample_own_vitals(me,ctx)
    vout.samples = vout.samples + 1
    local now = os.clock()
    if vout.ctx == key and not vitals_changed(s, vout.last) and now - vout.at < VITALS_BEAT_S then return false end
    vout.seq = vout.seq + 1
    -- The game's own `vitals` slot: read by the sidecar (gated, sent C2S) and
    -- by any Lua state (the HUD) through IPC.rec("vitals").
    local ipc = rawget(_G, "HSMP_IPC")
    local record = VQ.record(vout.seq,s)
    record.match_id,record.round,record.life = ctx.match_id,ctx.round,ctx.life
    if ipc and ipc.put("vitals", record) then
        vout.ctx = key
        vout.last, vout.at, vout.writes = s, now, vout.writes + 1
        return true
    end
    return false
end

C3.remote_defeated={}      -- verified permanent loss, still native-alive KO
local remote_dead = {}     -- peer_id -> true once the owner is dead
local death_shown = {}     -- peer_id -> true once the stand-in played death
local server_dead = {}     -- peer_id -> round_key() of the round the SERVER declared it dead

local remote_vitals = {}   -- peer_id -> last frame (logs)
C3.server_death_context={}
C3.remote_dead_context={}
-- Initialization mirrors follow the actual actor-generation published by
-- Avatars from a PeerPlay sample with has_context=true. Pending round vitals
-- never borrow the previous live Mode; combat displayed_for stays strict.
function C3.mirror_context(peer,w)
    local view=HSESS and HSESS.view and HSESS.view()
    local phase=view and view.state
    if phase~="loading" and phase~="countdown" then return C3.displayed_for(peer,w,false) end
    local order=view.spawns and view.spawns[peer]
    if not order or not view.match_id or view.match_id==0 or not view.spawn_round
        or view.spawn_round~=view.pending_round then return nil end
    local ipc=rawget(_G,"HSMP_IPC")
    local pb=ipc and ipc.bus_table("playback")
    local name=w and wname(w)
    for _,row in ipairs(pb and pb.rows or {})do
        if name and row.peer==peer and row.pawn==name and row.match_id==view.match_id
            and row.round==view.spawn_round and row.life==1
            and type(row.body_ts)=="number" and row.body_ts>0 and type(row.local_ms)=="number"
            and now_ms()-row.local_ms>=0 and now_ms()-row.local_ms<=250 then
            return {match_id=row.match_id,round=row.round,life=row.life}
        end
    end
    return nil
end
-- A peer's `peer_vitals` record (the sidecar writes the owner's record as it
-- came), dequantised once per new record table (IPC.peer_rec caches per version).
read_vitals = function(peer,w)
    local ipc = rawget(_G, "HSMP_IPC")
    local t = ipc and ipc.peer_rec("peer_vitals", peer)
    local ctx = C3.mirror_context(peer,w or puppet_actor[peer])
    if t == nil or not ctx or t.match_id ~= ctx.match_id or t.round ~= ctx.round or t.life ~= ctx.life then return nil end
    local c = VQ.cache[peer]
    if not c or c.src ~= t then
        c = { src = t, r = VQ.frame(t) }
        VQ.cache[peer] = c
    end
    if c.r then remote_vitals[peer] = c.r end
    return c.r
end

-- Stand-in mirror: the owner's vitals on the stand-in, so its native
-- behaviour (blood, pain, exhaustion, limb state) matches. Never lethal:
-- Health stays pinned (PUPPET_HP_PIN), limb/consciousness values are
-- floored at STANDIN_FLOOR, posture flags (Fallen/Downed) are NOT written
-- (the pose stream already shows the owner's real posture; the native
-- get-up logic would fight the PhysicsHandles). Severed parts: the bone is
-- hidden (PBO_None: physics bodies stay, handles keep working).
local mirror = {}          -- peer_id -> { addr, seq, tick, hidden = {bone=true|"fail"}, n }
local function standin_meshes(w)
    local out = {}
    for _, k in ipairs({ "Mesh", "SK_Skeleton" }) do
        local m; pcall(function() m = w[k] end)
        if m and m:IsValid() then out[#out + 1] = m end
    end
    return out
end

local function set_bone_hidden(w, bone, hide)
    local ok_any = false
    for _, m in ipairs(standin_meshes(w)) do
        local idx = -1
        pcall(function() idx = m:GetBoneIndex(FName(bone)) end)
        if idx and idx >= 0 then
            local ok = pcall(function()
                if hide then m:HideBoneByName(FName(bone), 0) else m:UnHideBoneByName(FName(bone)) end
            end)
            ok_any = ok_any or ok
        end
    end
    return ok_any
end

local function mirror_vitals(peer, w, r)
    -- Mode may already assign a new life while this pooled actor still shows
    -- the preceding body. Never put the new life's injuries onto that body.
    local shown = C3.mirror_context(peer,w)
    if not r or not shown or r.match_id ~= shown.match_id
        or r.round ~= shown.round or r.life ~= shown.life then return 0 end
    local addr; pcall(function() addr = w:GetAddress() end)
    local m = mirror[peer]
    if not m or m.addr ~= addr or m.match_id ~= shown.match_id
        or m.round ~= shown.round or m.life ~= shown.life then
        -- Same native actor keeps its hidden-bone bookkeeping so the fresh
        -- life can explicitly unhide parts instead of forgetting old writes.
        local hidden = m and m.addr == addr and m.hidden or {}
        m = { addr = addr, match_id=shown.match_id,round=shown.round,life=shown.life,
            seq = -1, tick = -999, hidden = hidden, n = 0 }
        mirror[peer] = m
    end
    if not r or not r.v then return 0 end
    if r.seq == m.seq and tick_num - m.tick < MIRROR_REASSERT then return 0 end
    local fresh = r.seq ~= m.seq
    m.seq, m.tick = r.seq, tick_num
    local n = 0
    for i, d in ipairs(VITALS) do
        local x = r.v[i]
        if x and d[3] then
            if d[3] == "limb" then x = math.max(x, STANDIN_FLOOR) end
            if pcall(function() w[d[1]] = x end) then n = n + 1 end
        end
    end
    if MIRROR_DISMEMBER then
        local want = {}
        for _, b in ipairs(r.dism) do want[b] = true end
        for b in pairs(want) do
            -- The game's appearance/physics rebuild can unhide a previously
            -- mirrored bone. Reassert at the same bounded rate as vitals and
            -- retry a failed lookup rather than treating "fail" as success.
            do
                local previous = m.hidden[b]
                local ok = set_bone_hidden(w, b, true)
                m.hidden[b] = ok and true or "fail"
                if previous ~= m.hidden[b] then
                    Log("peer %d severed '%s' -> stand-in bone %s", peer, b, ok and "hidden" or "NOT found on stand-in mesh")
                end
            end
        end
        for b, st in pairs(m.hidden) do
            if not want[b] then
                if st == true then set_bone_hidden(w, b, false) end
                m.hidden[b] = nil
            end
        end
    end
    m.n = n
    if fresh and not m.logged then
        m.logged = true
        local function g(i) return r.v[i] and string.format("%.0f", r.v[i]) or "?" end
        Log("vitals mirror: peer %d stand-in %s <- seq %d: hp=%s head=%s torso=%s/%s arms=%s/%s legs=%s/%s cons=%s stamina=%s exh=%s bleed=%s pain=%s flags=%d (%d fields written)",
            peer, wname(w) or "?", r.seq, g(1), g(2), g(4), g(5), g(7), g(8), g(9), g(10), g(12), g(14), g(15), g(16), g(18), r.f, n)
    end
    return n
end

-- Death handshake with HSMPAvatars: every peer whose stand-in plays a
-- death is listed in the `standin_dead` bus record ({wall, rows = {{peer,
-- name}}}) BEFORE Health is zeroed, so Avatars keeps the corpse (no "puppet
-- died locally; re-claiming"). Rewritten on change and once a second while
-- non-empty (the wall stamp is the liveness).
local sdead = { key = nil, at = -1e9, MAX = 32 }
local function write_standin_dead(force)
    local ids = {}
    for peer in pairs(death_shown) do ids[#ids + 1] = peer end
    table.sort(ids)
    local rows, keys = {}, {}
    for i, peer in ipairs(ids) do
        if i > sdead.MAX then break end
        local name = tostring(death_shown[peer])
        rows[i] = { peer = peer, name = name }
        keys[i] = peer .. "=" .. name
    end
    local key = table.concat(keys, ",")
    local now = os.clock()
    if not force and key == sdead.key and (#ids == 0 or now - sdead.at < 1.0) then return end
    local ipc = rawget(_G, "HSMP_IPC")
    if ipc and ipc.bus_put("standin_dead", { wall = os.time(), rows = rows }) then
        sdead.key, sdead.at = key, now
    end
end

-- A vitals "dead" flag is trusted only once the owner's vitals seq has
-- advanced in this world (a flag left over from the last round / an unseen
-- death must not kill the new round's stand-in), or when the server declared
-- the death for this round.
-- (one table: the main chunk is at Lua's 200-locals limit)
local VB = { base = {} }   -- base: peer -> vitals seq first seen in this world

-- A server-declared death stands for the rest of THAT round only. During the
-- next round's countdown the match state still carries the old round number,
-- so round_key(match_round) would match the death of the round that just
-- ended and kill the NEW round's stand-in in its fresh world (HSMPAvatars
-- then re-claims a DED puppet at Live, which can steal the local possession
-- and teleport the new pawn). A countdown (or the lobby) is never part of a
-- dead round.
function VB.server_dead_now(peer,shown)
    local ctx=C3.server_death_context[peer]
    return server_dead[peer] ~= nil and server_dead[peer] == round_key(match_round)
        and match_state ~= "loading" and match_state ~= "countdown" and match_state ~= "lobby"
        and ctx and shown and ctx.match_id==shown.match_id and ctx.round==shown.round and ctx.life==shown.life
end

local function update_standins()
    for peer, w in pairs(puppet_actor) do
        if w and w:IsValid() then
            local shown=C3.mirror_context(peer,w)
            local v = read_vitals(peer,w)
            local wk = WG.key or "?"
            local old=C3.remote_dead_context[peer]
            if shown and old and (old.match_id~=shown.match_id or old.round~=shown.round or old.life~=shown.life) then
                remote_dead[peer],death_shown[peer]=nil,nil
                C3.remote_dead_context[peer]=nil
            end
            -- A death carried over from another world (round reset) is
            -- re-validated: only the server's verdict for THIS round keeps it.
            if remote_dead[peer] and remote_dead[peer] ~= wk then
                if VB.server_dead_now(peer,shown) then remote_dead[peer] = wk
                else remote_dead[peer], death_shown[peer] = nil, nil end
            end
            if v and v.dead then remote_dead[peer] = wk;C3.remote_dead_context[peer]={match_id=v.match_id,round=v.round,life=v.life} end
            if v and not v.dead and v.hp and v.hp > 0 then
                -- New round (fresh owner pawn) — but a server-declared death
                -- stands for the whole round even if the owner's own copy
                -- still reports health (background window / not applied yet).
                if remote_dead[peer] and not VB.server_dead_now(peer,shown) then
                    remote_dead[peer], death_shown[peer] = nil, nil
                end
            end
            local defeated=C3.remote_defeated[peer]
            local defeated_shown=defeated and shown
            if defeated and defeated_shown and defeated.match_id==defeated_shown.match_id
                and defeated.round==defeated_shown.round and defeated.life==defeated_shown.life then
                -- Preserve the owner's native KO pose. No corpse conversion or HP0.
                pcall(function() w["Give Up"]=true;w["Give Up 2 (Temp)"]=true end)
                mirror_vitals(peer,w,v)
                C3.protect(w,peer);C3.baseline(w)
            elseif remote_dead[peer] and shown then
                -- death_shown = the stand-in (FName) that played it: a stand-in
                -- replaced while the owner is still dead plays it again.
                local nm = wname(w) or "?"
                if death_shown[peer] ~= nm then
                    death_shown[peer] = nm
                    write_standin_dead(true)   -- Avatars must see it before Health hits 0
                    pcall(function() w.Health = 0 end)
                    death_dispatching = true
                    C3.base[addr_of(w) or 0] = nil   -- no backstop "heal" of the corpse
                    local ok = bp_call(w, "Death")
                    if not ok then bp_call(w, "Dying") end
                    death_dispatching = false
                    Log("peer %d died -> stand-in plays death", peer)
                end
            else
                pcall(function()
                    if (tonumber(w.Health) or 0) < PUPPET_HP_PIN then w.Health = PUPPET_HP_PIN end
                end)
                mirror_vitals(peer, w, v)
                -- No native damage on a stand-in, and the state any
                -- leak is put back to (the mirrored owner state, this tick).
                C3.protect(w, peer)
                C3.baseline(w)
                if C3.BODY then
                    local ipc = rawget(_G,"HSMP_IPC")
                    local rec = ipc and ipc.peer_rec and ipc.peer_rec("peer_body2",peer)
                    local applied = C3.BODY.applied[wname(w)]
                    if (tick_num + peer) % C3.BODY.APPLY_TICKS == 0
                        or (rec and (not applied or applied.ver ~= rec.version)) then C3.body_apply(peer,w) end
                end
            end
        end
    end
end

-- Deathmatch respawns (the `mode` record, shared/hsmp_session.lua HS.mode()): a peer whose
-- life count went up in this round is back in it. Its earlier death no longer holds its
-- stand-in (HSMPAvatars re-claims a body once standin_dead drops it and the owner's fresh
-- vitals say alive), and its next death in the same round plays again.
C3.lives = {}   -- peer -> { round, life }
function C3.respawns()
    local m = HSESS and HSESS.mode and HSESS.mode()
    if not m then return end
    local changed = false
    for peer, row in pairs(m.rows) do
        local prev = C3.lives[peer]
        if prev and prev.round == m.round and row.life > prev.life then
            if remote_dead[peer] or death_shown[peer] or server_dead[peer] then changed = true end
            remote_dead[peer], death_shown[peer], server_dead[peer] = nil, nil, nil
            C3.remote_dead_context[peer],C3.server_death_context[peer]=nil,nil
            C3.remote_defeated[peer]=nil
            Log("peer %d respawned (round %d, life %d): its death no longer holds", peer, m.round, row.life)
        end
        C3.lives[peer] = { round = m.round, life = row.life }
    end
    if changed then write_standin_dead(true) end
end

-- --- main tick ----------------------------------------------------------------

-- The typed S2G records this mod reads. Registering this Lua state's event
-- cursor for each kind now (and dropping whatever is already queued) means
-- records from before this load are never replayed, while records arriving
-- before the first poll are kept.
C3.EVENTS = { "damage_in", "hitfx_in", "damage_verdict", "death", "replay_outcome", "replay_outcome_ack" }
for _, kind in ipairs(C3.EVENTS) do events(kind) end
-- `hitfx_in`: an accepted hit on ANOTHER player (`d.target_peer_id`; the
-- attacker is the entry's peer), replayed natively on my stand-in of that
-- player for its blood, wounds, bruises and sounds. The stand-in's damage,
-- reaction, life and gate state is put back right after (the owner's vitals
-- stream stays the authority; whole distal missing regions come from the
-- owner's completed-cut ledger projection; native partial/detached geometry
-- still needs separate topology replication. Stand-ins disable local severing.
function C3.apply_fx(d, _attacker)
    if type(d) ~= "table" then return "fx: no record" end
    local target = math.tointeger(tonumber(d.target_peer_id)) or 0
    if target == 0 then return "fx: no target" end
    if target == my_peer_id then return "fx: own hit (applied from damage_in)" end
    -- Reliable cosmetic packets retain the original native contact generation.
    -- Undoing numeric values cannot undo wounds painted on a fresh life.
    local victim_context=C3.displayed_for(target,C3.remote_actor(target),false)
    local attacker_context=_attacker==my_peer_id and C3.life_for(_attacker,true)
        or C3.displayed_for(_attacker,C3.remote_actor(_attacker),false)
    if not victim_context or not attacker_context
        or d.match_id~=victim_context.match_id or d.round~=victim_context.round or d.victim_life~=victim_context.life
        or d.match_id~=attacker_context.match_id or d.round~=attacker_context.round or d.attacker_life~=attacker_context.life then
        return "fx: stale context"
    end
    local flags = math.floor(num(d.flags))
    if math.floor(flags / FLAG_COMPLEX) % 2 ~= 1 then return "fx: not an armour-stage hit" end
    if flags & 1 ~= 0 and num(d.parent_cid)==0 then return "fx: native origin without damage" end
    local hr = math.tointeger(tonumber(d.round)) or 0
    if hr == 0 then hr = nil end
    if not combat_window or (hr and hr ~= match_round) then return "fx: round not live" end
    local w = C3.remote_actor(target)
    if not (w and w:IsValid()) then return "fx: no stand-in for peer " .. target end
    if remote_dead[target] then return "fx: stand-in dead" end
    local coll = C3.hitter(_attacker, d)
    if math.floor(num(d.dism_blunt) / BF.COMPONENT) % 16 ~= 0 and not coll then
        return "fx: skipped (striking component unavailable)"
    end
    local hit_box = C3.hit_box(_attacker, d, coll)
    if ((math.floor(num(d.dism_blunt)) >> 27) & 15) ~= 0 and not hit_box then
        return "fx: skipped (native cutting HitBox unavailable)"
    end
    local mesh = C3.hit_mesh(w)
    local geo = C3.hit_geo(w, mesh, d)
    if geo.local_lost then return "fx: skipped (victim bone frame unavailable)" end
    if hit_box then
        hit_box=BoxReplay.component(local_pawn(),d.hit_box_frame,geo.bone_frame,tostring(_attacker)..":"..tostring(math.floor(num(d.dism_blunt)) & (BF.LEFT|BF.RIGHT)))
        if not hit_box then return "fx: skipped (historical cutting Box unavailable)" end
    end
    local s = C3.snap_all(w)
    local fx_world,fx_drops = WG.key,WG.drops
    local inv; pcall(function() inv = w.Invulnerable end)
    -- Cosmetic replay cannot safely undo structural native changes. Require
    -- the native guard immediately before every call (pooled pawns can reset
    -- the creation-time flag); otherwise skip the cosmetic call entirely.
    local protected = false
    pcall(function()
        w["Force Disable Dismemberment"] = true
        w["Force Disable Vertex Paint"] = true
        protected = w["Force Disable Dismemberment"] == true and w["Force Disable Vertex Paint"] == true
    end)
    if not protected then return "fx: skipped (native dismemberment guard unavailable)" end
    pcall(function() w.Invulnerable = false end)
    -- (gates open: this is the blow's look, its damage is the owner's replay)
    pcall(function() w["Last Complex Damage Impulse"] = 0; w["Last Damage Taken"] = 0 end)
    replaying = true
    local paint_open = false
    pcall(function() w["Force Disable Vertex Paint"] = false;paint_open = w["Force Disable Vertex Paint"] == false end)
    local ok, err = false,"native paint guard unavailable"
    if paint_open then
        local executed,native_ok,native_err=pcall(function()
            return bp_call(w, "Deal Complex Damage", table.unpack(C3.dcd_args(d, mesh, geo, coll, hit_box), 1, 23))
        end)
        ok=executed and native_ok
        err=executed and native_err or native_ok
    end
    replaying = false
    -- A travel hook drops old-world objects without touching them. Even a
    -- caught native error must close the paint guard in the same world.
    if WG.key ~= fx_world or WG.drops ~= fx_drops then return "fx: world changed during replay" end
    pcall(function() w["Force Disable Vertex Paint"] = true end)
    C3.put_all(w, s)
    pcall(function() w.Invulnerable = (inv == nil) and C3.STANDIN_INVULNERABLE or inv end)
    pcall(function() if (tonumber(w.Health) or 0) < PUPPET_HP_PIN then w.Health = PUPPET_HP_PIN end end)
    C3.fx = C3.fx + 1
    return ok and "fx: replayed on stand-in" or ("fx: replay failed (" .. tostring(err) .. ")")
end

-- Death bookkeeping of the previous session / match must not leak.
local function clear_death_state(why)
    if next(remote_dead) or next(death_shown) or next(server_dead) then
        Log("death state cleared (%s)", why)
    end
    remote_dead, death_shown, server_dead = {}, {}, {}
    C3.remote_dead_context,C3.server_death_context={},{}
    C3.remote_defeated={}
    if sdead.key and sdead.key ~= "" then write_standin_dead(true) end
end

-- The sidecar's link record: own peer id + connected. A missing / torn read
-- is no evidence: keep the last values. The death state is cleared when the
-- peer id changes (every Welcome: reconnect, server restart) and when the
-- session ends (record absent on two reads in a row, or a terminal status),
-- so session 2 never starts with session 1's dead peers.
-- A link stall (connected -> reconnecting -> connected, same id) keeps it.
local session_gone_reads = 0
local SESSION_END = { ended = true, stopped = true, exited = true, disconnected = true, closed = true,
                      kicked = true, replaced = true, server_closed = true, rejected = true }
local function refresh_session()
    -- The sidecar's link record (slot `link`: my peer id, status code).
    local st = HSESS and HSESS.link()
    if type(st) ~= "table" then
        session_gone_reads = session_gone_reads + 1
        if session_gone_reads >= 2 then
            if connected then Log("session gone (no sidecar status)") end
            connected = false
            clear_death_state("session ended")
            CX.discard_outbox("session ended")
        end
        return
    end
    session_gone_reads = 0
    local id = math.tointeger(st.my_peer_id) or 0
    local status = HSESS.status_name(st)
    if id > 0 and id ~= my_peer_id then
        if my_peer_id ~= 0 then
            clear_death_state(string.format("peer id %d -> %d", my_peer_id, id))
            CX.discard_outbox("new peer id")
        end
        my_peer_id = id
        C3.me_id = id
    end
    if SESSION_END[status] then
        clear_death_state("sidecar status " .. tostring(status))
        CX.discard_outbox("sidecar status " .. tostring(status))
    end
    if SESS then SESS:poll(true); connected = SESS:live() else connected = status == "connected" end
end
local puppets_stale = true   -- re-resolve stand-ins on the first tick of a world

-- World changed / level change requested: forget every world-scoped object
-- without touching it. Stand-in FNames (puppet_peer) are plain strings and
-- stay, so hits on stand-ins are still recognised until the refresh.
wg_on_drop(function(why)
    C3.body_replay=nil
    if C3.body_audit then C3.body_audit.clear() end
    puppet_actor = {}
    CX.pending = {}
    CX.inside_queue = {}
    C3.stuck_parent,C3.stuck_parent_n = {},0
    C3.probe_last=nil
    C3.probe_after={}
    C3.base = {}              -- baselines name actors of the old world
    C3.gated = {}             -- (actor names are reused by the next world)
    FootReplay.clear(why)
    BoxReplay.clear(why)
    C3.fist_replay.clear(why)
    -- Numeric native-attempt identities outlive object caches, including a
    -- transient PlayerController miss. Original full match/life keys bound
    -- every record; dropping the watermark could execute a lost-ACK retry twice.
    -- The per-pawn bookkeeping (spawn heals, the round-start heal) is reset
    -- only by a REAL level change (the OpenLevel / LoadMap pre-hooks) or a
    -- lost world; a "world changed" drop inside one world (a one-tick
    -- PlayerController lookup miss changes the guard key) keeps it, else the
    -- next tick re-runs restore_vitals("round start"): a full mid-round heal.
    -- own_pawn_tick still starts afresh for a new pawn (address + name).
    if why ~= "world changed" then
        C3.gd_last = nil
        own = { addr = nil, at = 0, hits = 0, passes = 0, live_round = -1, forced_round = -1, died_round = -1 }
    end
    _in_game_tick = -999
    puppets_stale = true
    mirror = {}               -- stand-in actors are gone (hidden-bone state with them)
    dism_cache, dism_tick, dism_owner = {}, -999, nil
    if dism_projection then dism_projection.reset() end
    vout.last = nil           -- first sample of the new world goes out at once
    VB.base = {}          -- a dead flag must be re-earned in the new world
    if C3.BODY then C3.BODY.drop() end
end)

local function on_tick()
    tick_num = tick_num + 1
    try_hook()
    -- no UObject: also between worlds and in the menu, so a session
    -- that ends outside an arena still clears its death state
    if tick_num % 15 == 0 or tick_num == 1 then refresh_session() end
    -- Retry records the facade queued (ring full / sidecar not attached),
    -- only into the SAME live session (a session end / new peer id
    -- discards them, see refresh_session)
    if connected then
        local ipc = rawget(_G, "HSMP_IPC")
        if ipc and ipc.pending and ipc.pending() > 0 then ipc.flush() end
    end
    if not wg_check() then return end
    if tick_num % 5 == 0 then refresh_match() end
    flush_claims()
    if not in_gameplay() then return end
    -- WG.settled (shared/hsmp_wg.lua): no FindAllOf over Willies and no
    -- name read on what it returns, and nothing on the local pawn, in the
    -- first 2 s of a world (old world still being purged, the new player
    -- Willie mid-construction: uncatchable access violations).
    -- Damage / death records wait in this state's event queue meanwhile.
    if not WG.settled() then puppets_stale = true; return end
    if puppets_stale then
        puppets_stale = false
        refresh_puppets()
        C3.read_teams()
    end
    if tick_num % 15 == 0 then
        refresh_puppets()
        C3.read_teams()
    end
    if not connected then return end

    local me = local_pawn()
    if me then own_pawn_tick(me) end
    if me and C3.BODY then C3.body_tick(me) end
    if me and next(C3.gated) ~= nil and tick_num % 5 == 0 then C3.ungate(me) end

    read_feedback()
    if match_live then emit_quality(false) end

    for _,e in ipairs(events("replay_outcome_ack")) do ReplayAttempts.ack(e.data or {}) end
    for _,e in ipairs(events("replay_outcome")) do
        local r=e.data or {}
        if r.status then
            local k=tostring(r.status);C3.receipt_stats[k]=(C3.receipt_stats[k] or 0)+1
            C3.receipt_fields=C3.receipt_fields | (math.tointeger(r.observed_fields) or 0)
            C3.receipt_hp=C3.receipt_hp+num(r.health_delta)
        end
        Log("OWNER outcome victim=%s attacker=%s #%s match=%s round=%s life=%s status=%s fields=%s hp=%s",
            tostring(e.peer),tostring(r.attacker),tostring(r.hit_id),tostring(r.match_id),tostring(r.round),
            tostring(r.victim_life),tostring(r.status),tostring(r.observed_fields),tostring(r.health_delta))
    end
    local applied = 0
    for _, e in ipairs(events("damage_in")) do
        local d = type(e.data) == "table" and e.data or {}
        local native_health
        local res, outcome, fresh = ReplayAttempts.run(d,e.peer,function()
            local text,status,fields,hp,observed_health = apply_hit(d,e.peer)
            native_health = observed_health
            return text,status,fields,hp
        end)
        C3.log_replay_probe(d,e.peer,outcome,res,fresh,native_health)
        replaying = false -- also release the guard after an unexpected Lua failure
        if fresh then applied = applied + 1 end
        local stats=C3.replay_stats
        if fresh then
            local k=tostring(outcome.status);stats[k]=(stats[k] or 0)+1
            C3.replay_fields=C3.replay_fields | (math.tointeger(outcome.observed_fields) or 0)
            C3.replay_hp=C3.replay_hp+num(outcome.health_delta)
        end
        local v = C3.v3(d, "velocity")
        Log((fresh and "HIT from peer" or "CACHED HIT from peer") .. " %s (#%s) bone=%s vel=%.0f rig=%.2f cut=%.0f stab=%.2f: %s",
            tostring(e.peer), tostring(d.hit_id), tostring(d.bone), math.sqrt(v[1] ^ 2 + v[2] ^ 2 + v[3] ^ 2),
            num(d.damage_out), num(d.cutting_power), num(d.pain_rate), tostring(res))
    end
    ReplayAttempts.tick()
    -- Blood / wounds after observed native owner injury, on their stand-ins.
    for _, e in ipairs(events("hitfx_in")) do
        local d = type(e.data) == "table" and e.data or {}
        local res = C3.apply_fx(d, e.peer)
        if C3.fx <= 10 or C3.fx % 50 == 0 or not res:find("replayed", 1, true) then
            Log("hit fx: peer %s -> peer %s (#%s) bone=%s: %s", tostring(e.peer),
                tostring(d.target_peer_id), tostring(d.hit_id), tostring(d.bone), res)
        end
    end
    if me then C3.baseline(me) end   -- the echo-undo state, after this tick's replays

    -- Authoritative deaths are scoped to a server match, round and native life.
    local deaths = events("death")
    if #deaths > 0 then refresh_match() end
    for _, e in ipairs(deaths) do
        local d = type(e.data) == "table" and e.data or {}
        local id = math.tointeger(tonumber(d.peer_id))
        local r = math.tointeger(tonumber(d.round))
        if r == 0 then r = nil end
        local killer = tonumber(d.killer)
        local cause = tonumber(d.cause)
        local current, why = C3.death_context(id, d)
        -- Only a validated current death may advance local bookkeeping.
        local mid = tonumber(d.match_id)
        if current and mid and mid ~= 0 then
            if last_death_mid and mid ~= last_death_mid then match_gen = match_gen + 1 end
            last_death_mid = mid
        end
        if not current then
            Log("ignored stale death for peer %s: %s", tostring(id), why)
        elseif id and id == my_peer_id then
            -- Round-gated: never kill a pawn respawned for the next round
            -- (round increments only at "live", so countdown is excluded).
            if r and r == match_round and (match_state == "live" or match_state == "roundover") then
                force_own_death(r, killer, cause, d)
            else
                Log("ignored stale death line for me (round %s, now %d %s)", tostring(r), match_round, match_state)
            end
        elseif id and (r == nil or r == match_round) then
            if cause==5 or cause==6 then
                C3.remote_defeated[id]={match_id=d.match_id,round=d.round,life=d.life}
                if r then server_dead[id]=round_key(r) end
                Log("server: peer %d defeated (native KO preserved)",id)
            else
            if not remote_dead[id] then
                Log("server: peer %d died (round %s, killer=%s, cause=%s)", id, tostring(r), tostring(killer), tostring(cause))
            end
            remote_dead[id] = WG.key or "?"
            C3.remote_dead_context[id]={match_id=d.match_id,round=d.round,life=d.life}
            if r then server_dead[id] = round_key(r) end
            C3.server_death_context[id]={match_id=d.match_id,round=d.round,life=d.life}
            C3.remote_defeated[id]=nil
            end
        end
    end

    -- Own vitals: ~15 Hz, and in the SAME tick as any applied hit (the
    -- server ledger counts a hit as reflected by the next vitals).
    if me and (applied > 0 or tick_num % VITALS_TICKS == 0) then publish_own_vitals(me) end
    C3.respawns()
    FootReplay.prune()
    C3.fist_replay.prune()
    update_standins()
    write_standin_dead(false)   -- heartbeat (no write unless changed / 1 s)
    if tick_num % 150 == 0 then
        local s = vout.last
        local mir = {}
        for id, m in pairs(mirror) do
            local r = remote_vitals[id]
            mir[#mir + 1] = string.format("peer %d seq=%d hp=%s st=%s fields=%d hidden=%d",
                id, m.seq, r and r.v and fmt_num(r.v[1]) or "?", r and r.v and fmt_num(r.v[14]) or "?",
                m.n or 0, (function() local k = 0; for _, st in pairs(m.hidden) do if st == true then k = k + 1 end end; return k end)())
        end
        Log("vitals: out seq=%d writes=%d/%d samples (5 s) hp=%s st=%s flags=%d | mirror %s | echo stamina fixes %d",
            vout.seq, vout.writes, vout.samples, s and fmt_num(s.v[1]) or "?", s and fmt_num(s.v[14]) or "?",
            s and s.f or -1, #mir > 0 and table.concat(mir, "; ") or "none", echo_stamina_fixes)
        vout.writes, vout.samples = 0, 0
    end
end

-- Dev measurement switch (HSMP_DEV=1 only): `hsmp-tools ipc-ctl --pid <game> autotest
-- combat_probe on|off` (the DevCtl ring; the native module fans each record out to every
-- Lua state). On: stand-ins take the game's own damage for my blows, logged as PROBE and
-- put back (C3.standin_hit), beside the owner's replay of the same claim.
C3.probe_out = {}
function C3.poll_probe()
    if (os.getenv("HSMP_DEV") or "") ~= "1" then return end
    local ipc = rawget(_G, "HSMP_IPC")
    if not (ipc and ipc.dev_poll) then return end
    for i = 1, ipc.dev_poll(8, C3.probe_out) do
        local d = C3.probe_out[i] and (C3.probe_out[i].data or C3.probe_out[i])
        if type(d) == "table" and d.op == 1 and d.key == "combat_probe" then
            C3.native_probe = tostring(d.arg or "") == "on"
            Log("combat_probe %s: stand-ins %s the game's own damage for my blows (logged as PROBE, put back)",
                C3.native_probe and "ON" or "OFF", C3.native_probe and "take" or "no longer take")
        elseif type(d)=="table" and d.op==1 and d.key=="body_probe" then
            C3.body_probe=tostring(d.arg or "")=="on"
            Log("body_probe %s: current-owner native body readback only",C3.body_probe and "ON" or "OFF")
        elseif type(d)=="table" and d.op==1 and d.key=="body_snapshot" then
            if C3.body_audit then pcall(C3.body_audit.capture,local_pawn(),"manual readback",{pre="unavailable:manual_read"}) end
        elseif type(d)=="table" and d.op==1 and d.key=="armor_probe" then
            if C3.armor_trace then
                if tostring(d.arg or "")=="on" then
                    Log("armor_probe: read-only burst armed=%s (15 seconds; native damage unchanged)",tostring(C3.armor_trace.start()))
                else
                    C3.armor_trace.stop()
                    Log("armor_probe: OFF")
                end
            else
                Log("armor_probe: unavailable")
            end
        end
    end
end

LoopAsync(TICK_MS, function()
    ExecuteInGameThread(function() pcall(C3.poll_probe); on_tick() end)
    return false
end)

Log("loaded; state_dir=%s (armour-stage claims, after-call hooks, hit fx)", STATE_DIR)

-- Offline test hook (`hsmp-tools lua-test vitals`): never set in game.
if rawget(_G, "HSMP_COMBAT_TEST") then
    HSMP_COMBAT_TEST.api = {
        FIELDS = FIELDS, VITALS = VITALS, is_weapon = BF.is_weapon, source = BF.source, native_hit_box = BF.hit_box,
        refresh_puppets = refresh_puppets, puppet_peers = function() return puppet_peer end,
        sample_own_vitals = sample_own_vitals, vitals_changed = vitals_changed, VQ = VQ,
        publish_own_vitals = publish_own_vitals, read_vitals = read_vitals,
        mirror_vitals = mirror_vitals, update_standins = update_standins, apply_hit = apply_hit,
        refresh_match = refresh_match, refresh_session = refresh_session, on_get_damage = on_get_damage, on_complex = on_complex,
        try_hook = try_hook, round_key = function(r) return round_key(r) end,
        hooks = function() return { get_damage = hook_ok, death = death_hook_ok, weapon = weapon_hook_ok,
                                    death_tries = death_hook_tries, weapon_tries = weapon_hook_tries } end,
        session = function() return { connected = connected, my_peer_id = my_peer_id, server_dead = server_dead,
                                      death_shown = death_shown } end,
        flush_claims = flush_claims, on_weapon_hit = on_weapon_hit, on_native_death = function(s) on_native_death(s) end,
        read_feedback = read_feedback, emit_quality = emit_quality, REACT = REACT, REACT_VEC = REACT_VEC,
        C3 = C3, CX = CX, SEND = SEND, WG=WG,local_pawn=local_pawn,
        discard_outbox = C3.discard_outbox, set_my_peer_id = function(id) my_peer_id = id; C3.me_id = id end,
        own = function() return own end, wg_drop = function(why) WG.drop(why) end,
        force_own_death = force_own_death,
        on_native_defeat=C3.on_native_defeat,
        update_standins=update_standins,
        own_pawn_tick = function(me) own_pawn_tick(me) end,
        quality = function() return quality end,
        set_puppets = function(peers, actors, weapons)
            puppet_peer, puppet_actor, puppet_weapon = peers, actors, weapons or {}
        end,
        set_tick = function(n) tick_num = n end,
        -- a test world is settled unless `fresh`
        set_world = function(key, fresh) WG.key = key; WG.key_at = fresh and os.clock() or -1e9 end,
        drop_world = function(key) WG.drop("test"); WG.key = key; WG.key_at = -1e9 end,
        settled = function() return WG.settled() end, on_tick = on_tick,
        write_standin_dead = function(force) write_standin_dead(force) end,
        state = function()
            return { vout = vout, mirror = mirror, remote_dead = remote_dead, sent = sent_count,
                     zeroed = zeroed_count, echo_fixes = echo_stamina_fixes, combat_window = combat_window,
                     echo_reactions = echo_reactions, cid = cid_seq, episodes = episodes }
        end,
    }
end
