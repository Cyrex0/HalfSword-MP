-- HSMPWorld — replicates world physics objects and interactables (v2).
-- Design + verification checklist: docs/development/subsystems/world-replication.md.
--
-- What: every loose, level-owned simulated/interactable body in the arena:
--   * weapons/shields/tools/treasure/traps (ModularWeaponBP_C and subclasses)
--     lying in the level (rack/floor weapons, spawned at runtime or placed);
--   * physics props (StaticMeshActor whose mesh simulates: buckets, cups,
--     ladders, benches, firewood ... the *_PhysicsProps sublevels);
--   * multi-body props (destructible barrels/planks/boards, flimsy fences,
--     sandbags, chains, swinging traps, chandeliers, candle stands, chests):
--     each simulating (or movable, for destructibles) mesh is its own body;
--   * dynamic items: a weapon that leaves MY hand (drop / disarm) becomes a
--     world item every client sees fall, lie there and be picked up.
-- Never: Willies, weapons held or attached to any Willie at discovery
-- (loadout/pose own those), archetype templates, skeletal ragdolls.
--
-- Identity: network id = FNV-1a of class | component | (level-placed FName
-- or spawn cell), deterministic on every client; the server's manifest is
-- canonical and folds near-duplicates, and we bind fuzzily (same class +
-- component within 120 cm of the spawn point) when a cell straddles.
--
-- Authority: a body is FREE (every client simulates it at rest; the server
-- keeps its rest pose) or LEASED to the peer touching/holding it. Only the
-- lease holder simulates and streams it; everyone else turns its physics off
-- and follows the stream through a jitter buffer. Picking up = hold claim
-- (beats a touch lease, can't rob a holder); pushing = touch claim (and the
-- first state is an implicit claim); at rest 1.5 s = release with the final
-- pose. Leases lapse after 3 s of silence or on disconnect.
--
-- Rounds: the server bumps its world epoch at every countdown / lobby; we
-- stop replicating at once and, if the arena isn't reloaded within
-- RESTORE_DELAY_MS, restore every body to its spawn pose and retire the
-- copies we spawned; then re-sync, so each round starts from the map's
-- initial state everywhere.
--
-- Physics safety: every teleport / SetSimulatePhysics goes through
-- can_drive() (validated world, no level change pending, object alive, not
-- held or attached natively); see "physics state helpers".
--
-- Shared memory (typed records, crates/hsmp-ipc/src/schema/world.rs; the sidecar side is
-- server/src/world_client.rs). Quantised once, here: smallest-three rotations, i16 velocities.
--   out: G2S world_sync, world_claim; slots world_out (my bodies' states),
--        world_manifest_out / world_dyn_out (bodies / my dropped items to propose),
--        world_hash (consistency report); bus key world_held (for HSMPLoadout /
--        HSMPAvatars: which peer holds which world item in which hand — don't give
--        their stand-in a duplicate); .world_scan.txt (dev: discovery report, a log)
--   in:  slots world_owners, world_manifest, world_dyn, world_remote,
--        world_consistency; the session status; bus key puppets
--
-- Identical worlds (world-replication.md, "Identical worlds"): pieces of compound
-- structures (Alley barricades, traps) are bodies; state-bearing actors
-- (planks, barrels, fences, levers, lever gates, traps, chests, chains) get
-- server-ordered state records (joint broken bits + a flag); the first
-- client's settled poses become every client's initial state, forced exactly
-- before Ready; a consistency report every 5 s is diffed by the server (verdict ->
-- hsmp_log world_consistency).

local UEHelpers = require("UEHelpers")

-- Thread-safety shim. LoopAsync, ExecuteWithDelay and *Async keybind
-- callbacks run on UE4SS worker threads; running them alongside game-thread
-- Lua corrupted this mod's VM (symbolized crash dumps: lua_next / __index on
-- garbage userdata). Route every deferred, looped and key-bound callback onto
-- the game thread.
local GAME_THREAD_LOOP = false
if LoopInGameThreadWithDelay and ExecuteInGameThreadWithDelay and CancelDelayedAction then
    GAME_THREAD_LOOP = true
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
    print(string.format("[HSMPWorld] " .. fmt .. "\n", ...))
end

-- --- world guard (shared/hsmp_wg.lua) ------------------------------------------
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
if not HW then
    Log("FATAL: shared/hsmp_wg.lua missing - %s disabled (deploy copies shared/*.lua)", "HSMPWorld")
    return
end
local WG = HW.new({ log = Log, UEHelpers = UEHelpers })
-- Any travel path (console "open", ServerTravel, native BP) ends in LoadMap: drop there too.
pcall(function() RegisterLoadMapPreHook(function() pcall(WG.travel, "LoadMap") end) end)
local wg_check, wg_on_drop, wg_token, wg_same = WG.check, WG.on_drop, WG.token, WG.same

local function trim(s) return s and s:match("^%s*(.-)%s*$") or nil end

local STATE_DIR     = (trim(os.getenv("HSMP_STATE_DIR")) or "hsmp_state"):gsub("\\", "/")
local SCAN_FILE     = STATE_DIR .. "/.world_scan.txt"
-- Identical-worlds constants live in one table (the main chunk is at Lua's 200-local limit).
local K = {}
local W2 = {}   -- identical-worlds and record helpers (same reason)
-- How bodies somebody else simulates are shown (world_follow.lua, next to this file).
K.FW = load_module("world_follow")
K.NA = load_module("native_array")
if not K.NA or type(K.NA.each)~="function" then
    Log("FATAL: native_array.lua missing - %s disabled (native enumeration required)","HSMPWorld")
    return
end
if not K.FW then
    Log("FATAL: world_follow.lua missing - %s disabled (deploy copies every Scripts/*.lua)", "HSMPWorld")
    return
end

-- Structured events (shared/hsmp_log.lua; no-op when not deployed).
K.HL = load_module("hsmp_log")
if K.HL and K.HL.init then pcall(K.HL.init, { mod = "HSMPWorld", state_dir = STATE_DIR }) end
-- HSMP-SHM facade (shared/hsmp_ipc.lua), published as the per-state global
-- HSMP_IPC; this mod's state-dir helpers route through it (transition bridge).
do
    local m = load_module("hsmp_ipc")
    if m then m.init({ mod = "HSMPWorld", state_dir = STATE_DIR, log = Log }) end
end
local function ev(name, fields)
    local HL = K.HL
    if not HL then return end
    local f = HL.event or HL.emit
    if f then pcall(f, name, fields or {}) end
end

-- --- tuning ----------------------------------------------------------------------
local TICK_MS          = 16      -- apply/follow cadence (~frame rate)
local SEND_EVERY       = 3       -- send every 3rd tick (~20 Hz)
local SEND_MAX         = 24      -- bodies per send (one datagram)
local AWAKE_SPEED      = 8       -- cm/s
local AWAKE_MOVE       = 1.5     -- cm between reads
local AWAKE_ROT        = 1.0     -- deg between reads
local SLEEP_AFTER_MS   = 400     -- below thresholds this long = asleep
local RELEASE_AFTER_MS = 1500    -- asleep this long = release lease
local ASLEEP_REPEAT    = 3       -- final rest frame sent this many times
local NEAR_DIST        = 300     -- "I touched it": my pawn within this
local MINE_NEAR        = 150     -- ...or one of my moving bodies within this
local TOUCH_SPEED      = 20
local TOUCH_MOVE       = 5
local SETTLE_SPEED     = 2       -- absorb local settling into the anchor
local ANCHOR_DRIFT     = 10      -- free body this far off its anchor ...
local ANCHOR_GRACE_MS  = 1200    -- ... for this long (nobody claimed) = snap back
local CLAIM_RESEND_MS  = 300
local CLAIM_TIMEOUT_MS = 4000
local SYNC_RETRY_MS    = 1500
local STARTUP_QUIET_MS = 3000    -- no touch claims while the level settles
local SCAN_AT_MS       = { 100, 1000, 3000, 8000 }
local SCAN_SLICE       = 300     -- actors examined per tick while scanning
local MAX_BODIES_ACTOR = 12
local MAX_DYN_SPAWN_TICK = 2
local DYN_SPAWN_TRIES  = 5       -- per remote dynamic item, with backoff
local DYN_RETRY_MS     = 400     -- first retry delay (doubles)
local MY_DROP_R        = 600     -- a drop happens within this of my pawn (cm)
local DROP_CONFIRM_MS  = 100     -- loose this long before its pose is read
local DROP_GIVEUP_MS   = 1500    -- no plausible drop pose by then: not a drop
local RESTORE_DELAY_MS = 1200    -- round reset: wait for the arena reload first
-- Identical initial state, server-ordered actor states, consistency.
K.CLAIM_STATE, K.CLAIM_INIT = 4, 5   -- world.rs claim modes (not leases)
K.MODE_STATE      = 0x80     -- owner-record mode of an actor-state record (| bits)
K.ST_BREAK_MASK, K.ST_FLAG = 0x3F, 0x40
K.HS_ALIVE, K.HS_SETTLED, K.HS_STATE = 1, 2, 4
K.INIT_AFTER_SCAN  = 3       -- propose / force initial state after this scan pass (3 s: post-respawn)
K.INIT_SETTLE_MS   = 600     -- a body still this long proposes its rest pose
K.INIT_PROPOSE_DEADLINE_MS = 5000  -- ... or whatever pose it has by then
K.INIT_RESEND_MS   = 1500
K.INIT_TRIES       = 3
K.INIT_WAIT_MS     = 9000    -- Ready stops waiting for an anchor this long after init started
K.READY_TIMEOUT_MS = 25000   -- from level load: ready anyway, complete=false
K.FORCE_POS        = 0.5     -- cm: initial / healed anchors are forced exactly
K.FORCE_ROT        = 0.5     -- deg
K.PIN_EVERY_MS     = 1000    -- an untouched anchored body is pinned back at most this often
K.FREED_HOLD_MS    = 3000    -- a released body that moves on its own this soon is pinned at once
K.SHIELD_R         = 200     -- a free body this near a followed copy that moves is held on its anchor
K.PIN_FREE_DIST    = 240     -- a kinematically pinned prop gets physics back when a Willie is this close (cm):
                             -- a fighter's reach incl. a polearm, so it is physical before anyone can touch it
K.STATE_EVERY      = 15      -- ticks between actor-state passes (~250 ms)
K.STATE_APPLY_TRIES = 3
K.STATE_SETTLE_MS  = 3000    -- local != server this long: reported as comparable (a real mismatch)
K.HASH_EVERY_MS    = 5000
K.MAX_GROUPS       = 3       -- constraint groups (6 constraints each) per state actor
K.TAKE_R, K.TAKE_MARGIN, K.TAKE_SPEED = 200, 60, 150   -- touch hand-over (world.rs TAKE_*)
-- ESpawnActorCollisionHandlingMethod::AlwaysSpawn; ESpawnActorScaleMethod::
-- MultiplyWithRoot (UE 5.4 default: keeps the class's own root scale).
local SPAWN_ALWAYS, SCALE_MULTIPLY = 1, 1
local WEAPON_BASE_PATH = "/Game/Assets/Weapons/Blueprints/ModularWeaponBP.ModularWeaponBP_C"
-- "Weapon Passport" struct members (UE4SS_ObjectDump names; same list as
-- HSMPLoadout's WEAPON_FIELDS). Copied from the dropping peer's stand-in
-- weapon so the remote copy looks like the real one.
local PASSPORT_FIELDS = {
    { "WeaponClass_54_B478ECF7499977809745A3973AD678EC", "class", "class" },
    { "ID_70_C02CF656483647A1933EEA96314B78A6", "num", "id" },
    { "Name_57_3729B51148E846FE8DD336B9419BCEE1", "name", "name" },
    { "HeadSubModule1_7_ABBFD017411F42A4950B1C9F2360A30D", "class", "head_sub1" },
    { "HeadSubModule2_9_90AAA8304C7794E1BF814C9354A1A7E9", "class", "head_sub2" },
    { "HeadModule_11_62DF53134688807E1DA7F4A20E9F7139", "class", "head" },
    { "GuardModule_13_6DD2B06245505E53B529D090333012F0", "class", "guard" },
    { "PommelModule_15_561B01324BFCD4360DAE9A95299BB9D6", "class", "pommel" },
    { "GripModule_18_F4DF51EB4E742195B8C6BAB17E4C5DB4", "class", "grip" },
    { "HeadSize_21_2D425E61473B8F64FBAB51B223459D57", "vec", "head_size" },
    { "GuardSize_23_5A1AA0E04708E86FEFF61E974DDA8704", "vec", "guard_size" },
    { "GripSize_25_AC1660814C4C25C521AAA8830FE8ECCF", "vec", "grip_size" },
    { "PommelSize_27_660CC00C49C26D503E16B2BC58CE115E", "vec", "pommel_size" },
    { "CustomMassScaleHead_30_B95872A242AD944E2CE4D493F718F9D7", "num", "mass_head" },
    { "CustomMassScaleGuard_51_3A9024E74306B7BB5D186087011D1927", "num", "mass_guard" },
    { "CustomMassScaleGrip_32_0EAADEE0419C05C6DB38F0AE134A9B10", "num", "mass_grip" },
    { "CustomMassScalePommel_34_0AB28D814BDEF17D408D0DAA3A453173", "num", "mass_pommel" },
    { "MaterialMetalSteel_37_AB7A28C94B176CF81A6C8BA34AC57C36", "num", "mat_steel" },
    { "MaterialMetalColored_39_DC2EAC244758A8D82855CC940784A1D2", "num", "mat_colored" },
    { "MaterialWeood_41_E0B3C8DB48943B878AEFA3AB01E7B99A", "num", "mat_wood" },
    { "MaterialLeather_43_41D1114148FDB4FE4DACC8A2F4CA9FEB", "num", "mat_leather" },
    { "ColorWood_46_F3AE05AD4495EBCD1D354C8025D7C743", "color", "color_wood" },
    { "ColorLeather_48_DC45F07E4C0C3280278212A7158EE638", "color", "color_leather" },
    { "Price_60_83FE5A624EA188485BBE4E9C8606AEE5", "num", "price" },
    { "Tier_67_05026E6F43B7300AA8BACC9D9F9AB461", "num", "tier" },
}

local WEAPON_CLASS = "ModularWeaponBP_C"
-- Multi-body / interactable BP props seen in the arena maps (short class
-- names; FindAllOf includes subclasses).
local PROP_CLASSES = {
    "BP_Barrel_Destructable_Constraits_C", "BP_Structure_Plank_Destructible_Master_C",
    "BP_Fence_Flimsy_Big_C", "BP_Fence_Flimsy_Curved_C", "BP_Fence_Flimsy_Small_C",
    "BP_Fence_Bags_C", "Chain_BP_C", "Trap_BP_C", "Trap_Kettle_BP_C", "BP_Structure_Trap_C",
    "BP_Prop_Light_Chandelier_A_001_C", "BP_Prop_Light_Candle_Stand_001_C", "BP_Candle_C",
    "BP_Container_Master_C", "BP_Prop_Training_Dummy_001_C",
    -- EastTower lever / lever-gated block, LordsHall lidded pots.
    "ST_Lever_C", "ST_LeverActivated_Child_C", "BP_Prop_Cooking_Utencil_Pot_001_C",
}
-- Compound "structures" whose pieces are separate child actors attached to
-- them (Alley's BP_Structure_Trap_1..5 barricades: planks, boards, barrels;
-- EastTower's Trap_BP swinging blade, Cellar's kettle trap). Their pieces are
-- scene bodies like any other: they must not be skipped as "attached", or
-- the barricades ("doors") are never replicated.
K.STRUCT_PARENTS = { "^BP_Structure_", "^Trap_BP_C$", "^Trap_Kettle_BP_C$" }
local WEAPON_BODY_KEYS = { "BaseMesh", "Grip", "Head" }   -- verified: no spaces (UE4SS_ObjectDump)

local MODE_FREE, MODE_TOUCH, MODE_HOLD_R, MODE_HOLD_L = 0, 1, 2, 3
local WF_ASLEEP, WF_HELD, WF_LEFT, WF_SIM = 1, 2, 4, 8
local RF_TEMPLATE = 0x10 | 0x20  -- RF_ClassDefaultObject | RF_ArchetypeObject

-- --- pure helpers (unit-tested: `hsmp-tools lua-test hsmpworld`) ---------------------------------------------
local T = {}
-- The pure helpers (and W2's record / codec functions) live in world_pure.lua.
local P = load_module("world_pure")
if not (P and P.install) then error("HSMPWorld: world_pure.lua is missing (deploy copies every Scripts/*.lua)") end
P = P.install(T, K, W2)
local fnv1a, clock_ms, v3, dist3, vlen, F, rot_diff = P.fnv1a, P.clock_ms, P.v3, P.dist3, P.vlen, P.F, P.rot_diff
local quat_to_rot, rot_to_quat_t, stable_name = P.quat_to_rot, P.rot_to_quat_t, P.stable_name
local assign_ids, bind_manifest = P.assign_ids, P.bind_manifest

-- --- UE helpers ------------------------------------------------------------------------
local NAME_NONE
-- The reflected reads below run for every body every tick: pcall(RD.f, obj, ...) with
-- these functions made once, not a closure per call.
local RD = {}
function RD.is_valid(o) return o:IsValid() end
function RD.addr(o) return o:GetAddress() end
function RD.fname(o) return o:GetFName():ToString() end
function RD.cls(o) return o:GetClass():GetFName():ToString() end
function RD.cls_full(o) return o:GetClass():GetFullName() end
function RD.index(o, k) return o[k] end
function RD.sim(c) return c:IsSimulatingPhysics(NAME_NONE) end
function RD.loc(o) local l = o:K2_GetActorLocation(); return v3(l.X, l.Y, l.Z) end
function RD.cloc(c) local l = c:K2_GetComponentLocation(); return v3(l.X, l.Y, l.Z) end
function RD.crot(c) local q = c:K2_GetComponentRotation(); return { Pitch = q.Pitch, Yaw = q.Yaw, Roll = q.Roll } end
function RD.parent(a) return a:GetAttachParentActor() end
function RD.linvel(c) local v = c:GetPhysicsLinearVelocity(NAME_NONE); return v3(v.X, v.Y, v.Z) end
function RD.pc() return UEHelpers.GetPlayerController() end
function RD.pawn_of(pc)
    pc = pc or UEHelpers.GetPlayerController()
    if pc and pc:IsValid() and pc.Pawn and pc.Pawn:IsValid() then return pc.Pawn end
    return WG.ai_pawn()   -- the game's own AI drives our pawn (dev, HSMPParity `ai on`)
end
local function valid(o)
    if o == nil then return false end
    local ok, v = pcall(RD.is_valid, o)
    return ok and v == true
end
local function addr(o)
    local ok, a = pcall(RD.addr, o)
    if ok then return a end
    return nil
end
local function same(a, b)
    if not valid(a) or not valid(b) then return false end
    local x, y = addr(a), addr(b)
    return x ~= nil and x == y
end
local function fname_of(o)
    local ok, n = pcall(RD.fname, o)
    if ok then return n end
    return nil
end
local function cls_short(o)
    local ok, n = pcall(RD.cls, o)
    if ok then return n end
    return nil
end
local function cls_path(o)
    local ok, s = pcall(RD.cls_full, o)
    if not ok then s = nil end
    return s and s:match("^%S+%s+(.+)$") or s
end
local function field(o, k)
    local ok, v = pcall(RD.index, o, k)
    if ok then return v end
    return nil
end
local function is_sim(c)
    local ok, s = pcall(RD.sim, c)
    return ok and s == true
end
local function get_loc(o)
    local ok, r = pcall(RD.loc, o)
    if ok then return r end
    return nil
end

-- Level-owned, real (not a CDO/archetype template), not attached to anything.
local function live_actor(a)
    if not valid(a) then return false, "invalid" end
    local tmpl = false
    pcall(function() tmpl = a:HasAnyFlags(RF_TEMPLATE) end)
    if tmpl then return false, "template" end
    local outer_cls
    pcall(function() outer_cls = a:GetOuter():GetClass():GetFName():ToString() end)
    if outer_cls and outer_cls ~= "Level" then return false, "not-in-level" end
    local parent
    pcall(function() parent = a:GetAttachParentActor() end)
    if valid(parent) and not W2.attach_ok(cls_short(parent)) then return false, "attached" end
    return true
end

local function level_path(a)
    local s; pcall(function() s = a:GetOuter():GetFullName() end)
    return s and s:match("^%S+%s+(.+)$") or ""
end

-- A body pose within 1 cm of the world origin on every axis is never a real
-- world item: it is a destroyed / never-finished actor (Clean Up Map leaves
-- the placed rack weapons there; a deferred spawn that was never finished
-- sits there too).
local function at_origin(p)
    return p ~= nil and math.abs(p.X) < 1 and math.abs(p.Y) < 1 and math.abs(p.Z) < 1
end
T.at_origin = at_origin

-- Arena bounds = bounding box of the level's discovered scene bodies plus a
-- margin (set by finish_scan; nil until then). A pose outside them, at the
-- origin, or non-finite is never used to place anything.
local BOUNDS_MARGIN_XY, BOUNDS_MARGIN_DOWN, BOUNDS_MARGIN_UP = 3000, 3000, 5000
local WORLD_MAX = 1.0e6
local function pose_ok(p, bounds)
    if p == nil or at_origin(p) then return false end
    for _, k in ipairs({ "X", "Y", "Z" }) do
        local v = p[k]
        if type(v) ~= "number" or v ~= v or math.abs(v) > WORLD_MAX then return false end
    end
    if bounds then
        local lo, hi = bounds.lo, bounds.hi
        if p.X < lo.X or p.Y < lo.Y or p.Z < lo.Z or p.X > hi.X or p.Y > hi.Y or p.Z > hi.Z then return false end
    end
    return true
end
T.pose_ok = pose_ok

local function bounds_of(points)
    if #points < 3 then return nil end
    local lo = { X = math.huge, Y = math.huge, Z = math.huge }
    local hi = { X = -math.huge, Y = -math.huge, Z = -math.huge }
    for _, p in ipairs(points) do
        for _, k in ipairs({ "X", "Y", "Z" }) do
            lo[k] = math.min(lo[k], p[k]); hi[k] = math.max(hi[k], p[k])
        end
    end
    lo.X, lo.Y, lo.Z = lo.X - BOUNDS_MARGIN_XY, lo.Y - BOUNDS_MARGIN_XY, lo.Z - BOUNDS_MARGIN_DOWN
    hi.X, hi.Y, hi.Z = hi.X + BOUNDS_MARGIN_XY, hi.Y + BOUNDS_MARGIN_XY, hi.Z + BOUNDS_MARGIN_UP
    return { lo = lo, hi = hi }
end
T.bounds_of = bounds_of

-- Willies are pooled: K2_DestroyActor on one is a silent no-op that snaps it
-- to the origin. HSMPWorld never retires them.
local function is_willie_class(name)
    return name ~= nil and name:find("Willie", 1, true) ~= nil
end
T.is_willie_class = is_willie_class

-- Retire an actor this mod spawned: make it inert first (hidden, no
-- collision, no physics, no tick), then destroy it. If the destroy is
-- refused or deferred, nothing visible or physical is left behind.
local function retire_actor(a, weapon_keys)
    if not valid(a) then return false end
    if is_willie_class(cls_short(a)) then return false end
    pcall(function() a:SetActorHiddenInGame(true) end)
    pcall(function() a:SetActorEnableCollision(false) end)
    for _, k in ipairs(weapon_keys or {}) do
        local c = field(a, k)
        if valid(c) then pcall(function() c:SetSimulatePhysics(false) end) end
    end
    pcall(function()
        local root = a:K2_GetRootComponent()
        if valid(root) then root:SetSimulatePhysics(false) end
    end)
    pcall(function() a:SetActorTickEnabled(false) end)
    local ok = pcall(function() a:K2_DestroyActor() end) -- unsafe: ok Willies rejected above (is_willie_class)
    return ok
end

-- --- session -----------------------------------------------------------------------------
local sess = { connected = false, my_id = 0, next_read = 0 }
-- Dynamic ids I created (nid -> level hash; plain numbers, kept for the
-- process) and their actors (nid -> {actor, addr, cls, world}; UObject
-- refs, dropped untouched on every world-guard drop).
sess.my_nids, sess.mine = {}, {}
WG.cache("my dropped items", function() sess.mine = {} end)
-- The shared liveness rule (status connected AND a fresh heartbeat).
sess.HSM = load_module("hsmp_session")   -- typed session / link records
sess.HS = sess.HSM and sess.HSM.new({ every_s = 0 })
-- The typed spawn_status bus record (HSMPSync), or nil when absent / cleared (seq 0).
function sess.spawn_status()
    local ipc = rawget(_G, "HSMP_IPC")
    local t = ipc and ipc.bus_table and ipc.bus_table("spawn_status")
    if type(t) ~= "table" or (t.seq or 0) == 0 then return nil end
    return t
end

-- An empty / torn read is no evidence (keep the last state), and a
-- disconnect only counts once it lasted DISCONNECT_DEBOUNCE_MS: a one-read
-- blip ("reconnecting", a torn read, a rename in flight) must never drop the
-- replicated world state. sess.connected = the raw status; sess.live = the
-- debounced one the world state follows.
local DISCONNECT_DEBOUNCE_MS = 3000
sess.live, sess.down_since = false, nil
local function refresh_session(now)
    if now < sess.next_read then return end
    sess.next_read = now + 1000
    -- The sidecar's link record (slot `link`, typed).
    local st = sess.HSM and sess.HSM.link()
    if type(st) ~= "table" then
        sess.connected = false
    else
        sess.my_id = math.tointeger(st.my_peer_id) or 0
        -- ping round trip (the follower's one-way estimate for flying bodies)
        local rtt = tonumber(st.rtt_ms)
        if rtt and rtt > 0 and rtt < 2000 then sess.rtt = rtt end
        sess.connected = sess.HSM.status_name(st) == "connected" and sess.my_id > 0
        -- a "connected" link whose sidecar stopped beating is a dead sidecar
        if sess.HS then sess.HS:poll(true); sess.connected = sess.connected and sess.HS:fresh() end
        -- A new peer id is a new session (a quick reconnect / NAT
        -- rebind well inside the debounce): the server does not know the old
        -- id's leases any more, so the world state must be re-synced.
        if sess.my_id > 0 then
            if sess.last_id and sess.last_id ~= sess.my_id then
                sess.id_changed = { from = sess.last_id, to = sess.my_id }
            end
            sess.last_id = sess.my_id
        end
    end
    -- HSMPLoadout re-arms the kit during the countdown and the spawn
    -- protection (kit.lua rearm destroys the dropped kit weapon). A drop in
    -- that window must not become a world item, or every peer keeps a
    -- pickable ghost copy of a weapon that no longer exists for the owner.
    do
        local v = sess.HSM and sess.HSM.view()
        if v and type(v.state) == "string" then sess.match_state = v.state end
        local ss = sess.spawn_status()
        if ss then
            -- has_protect_until false = unbounded until Live
            sess.protected = (not ss.has_protect_until) or os.clock() < ss.protect_until
        else
            sess.protected = false
        end
    end
    if sess.connected then
        sess.live, sess.down_since = true, nil
    else
        sess.down_since = sess.down_since or now
        if now - sess.down_since >= DISCONNECT_DEBOUNCE_MS then sess.live = false end
    end
end
T.sess = sess
T.refresh_session = function(now) sess.next_read = 0; return refresh_session(now) end

-- --- per-level state ----------------------------------------------------------------------
local W = nil
local send_seq = 0   -- world_out frame seq (continues across W resets in one process)
local last_clock = clock_ms()

local function new_level(name, waddr, now)
    W2.world_lineage = (W2.world_lineage or 0) + 1
    W = {
        name = name, addr = waddr, lineage = W2.world_lineage, level = fnv1a(name), t0 = now, tick = 0,
        epoch = nil, synced = false, sync_next = 0, sync_tries = 0,
        objs = {}, by_lid = {}, by_nid = {}, by_addr = {}, taken = {}, known_actor = {},
        owners = {}, pending = {}, fclock = {}, fseq = {}, peer_rtt = {}, req = 0, seq = 0,
        scan = nil, scans_done = 0, scan_report = {}, skipped = {},
        last_owners = nil, last_manifest = nil, last_remote = nil, last_mout = nil, last_held = nil,
        mlen = 0, unmatched = 0, dyn_queue = {}, dyn_failed = {}, dyn_ctr = 1, my_items = {},
        classes = {}, unbound_rows = {}, restore_due = nil, manifest_seen = false, synced_at = 0,
        puppets = {}, puppet_next = 0, peer_passports = {},
        stats = { sent = 0, recv = 0, claims = 0, snaps = 0, t = now, follow = 0 },
        -- identical worlds
        actors = {}, known_state = {}, unbound_ids = {},
        init_t0 = nil, ready = false, ready_reason = "sync",
        hash_seq = 0, hash_next = 0, last_cons = nil, cons_log_budget = 20,
    }
    if W.level == 0 then W.level = 1 end
    Log("level %s -> level hash %d (me=%d)", name, W.level, sess.my_id)
end

-- One world_claim (G2S): `rest` = a WorldObj row (W2.qobj) or nil.
function W2.claim(id, mode, rest)
    W.req = W.req + 1
    local ipc = W2.ipc()
    if ipc then
        ipc.send("world_claim", { level = W.level, epoch = W.epoch or 0, req = W.req, id = id, mode = mode,
                                  has_rest = rest ~= nil, rest = rest })
    end
    return W.req
end

local function obj_label(o)
    return string.format("%s/%s#%d", o.cls or "?", o.comp or "?", o.nid or o.lid or 0)
end

-- --- discovery ------------------------------------------------------------------------------
local SMC_CLASS
local function static_mesh_comp_class()
    if not SMC_CLASS then pcall(function() SMC_CLASS = StaticFindObject("/Script/Engine.StaticMeshComponent") end) end
    return SMC_CLASS
end

local function weapon_body(a)
    for _, k in ipairs(WEAPON_BODY_KEYS) do
        local c = field(a, k)
        if valid(c) and is_sim(c) then return c, true end
    end
    local root; pcall(function() root = a:K2_GetRootComponent() end)
    if valid(root) then return root, is_sim(root) end
    local b = field(a, "BaseMesh")
    if valid(b) then return b, false end
    return nil, false
end

local function body_pose(c)
    local ok, p = pcall(RD.cloc, c)
    if not ok then return nil, nil end
    local ok2, r = pcall(RD.crot, c)
    if not ok2 then r = nil end
    return p, r
end

local function skip(reason, a)
    W.skipped[reason] = (W.skipped[reason] or 0) + 1
    if reason ~= "known" and #W.scan_report < 60 then
        W.scan_report[#W.scan_report + 1] = string.format("skip %-14s %s (%s)", reason, fname_of(a) or "?", cls_short(a) or "?")
    end
end

-- --- state-bearing actors -----------------------------------------------------------
K.PCC_PATH = "/Script/Engine.PhysicsConstraintComponent"
function W2.constraint_class()
    -- Per level (W is dropped on every world change).
    if not W.pcc_class then pcall(function() W.pcc_class = StaticFindObject(K.PCC_PATH) end) end
    return W.pcc_class
end

-- Constraint components of an actor sorted by FName: { {name, comp}, ... }.
-- The names come from the Blueprint (SCS / AddComponent node names), so the
-- order is the same on every client; bit k of a state group = constraint k.
function W2.actor_constraints(a)
    local out = {}
    local cc = W2.constraint_class()
    if not cc then return out end
    local arr
    pcall(function() arr = a:K2_GetComponentsByClass(cc) end)
    if arr then
        pcall(function()
            K.NA.each(arr,function(c)
                if valid(c) then out[#out + 1] = { fname_of(c) or ("pc" .. #out), c } end
            end)
        end)
    end
    table.sort(out, function(x, y) return x[1] < y[1] end)
    return out
end

-- One state candidate per group of 6 constraints (at least one per actor:
-- levers/traps carry only the flag). Ids come from assign_ids with comp
-- "A<g>", so they are as deterministic as body ids.
function W2.add_state_cands(s, a, ad, cls)
    local sk = W2.state_kind(cls)
    if not sk then return end
    W.known_state[ad] = true
    local p = get_loc(a)
    if not p or at_origin(p) then return end
    local cons = W2.actor_constraints(a)
    local groups = math.max(1, math.min(K.MAX_GROUPS, math.ceil(#cons / 6)))
    local name = fname_of(a)
    for g = 0, groups - 1 do
        local slice = {}
        for k = g * 6 + 1, math.min(#cons, g * 6 + 6) do slice[#slice + 1] = cons[k] end
        s.scands[#s.scands + 1] = {
            cls = cls, comp = "A" .. g, skind = sk, group = g, cons = slice, actor = a, addr = ad,
            name = name, levelpath = level_path(a), pos = p, stable = stable_name(name),
        }
    end
end

-- Candidate bodies of one actor: list of {comp_name, comp, sim}.
local function actor_bodies(a, kind, cls)
    if kind == "weapon" then
        if field(a, "Is Held") == true then return nil, "held" end
        if valid(field(a, "Parent Actor")) then return nil, "has-parent" end
        -- Ever held by a Willie (a loadout / NPC weapon, not scene content):
        -- player items become world items only via the dropped-item path.
        if valid(field(a, "Last Parent")) then return nil, "was-held" end
        local b, sim = weapon_body(a)
        if not b then return nil, "no-body" end
        -- BP_GameManager's weapon-pool rolls spawn a temporary base-class
        -- weapon at the origin and destroy it (docs/arena_static/README.md).
        -- Placed rack weapons that Clean Up Map destroyed also read back at
        -- the origin (seen in-game: EastTower's five polearms at (0,0,0)).
        local p = body_pose(b)
        if at_origin(p) then return nil, cls == WEAPON_CLASS and "pool-temp" or "at-origin" end
        return { { "W", b, sim } }
    elseif kind == "sma" then
        local c = field(a, "StaticMeshComponent")
        if not valid(c) or not is_sim(c) then return nil, "static" end
        return { { "SM", c, true } }
    else
        local out = {}
        local smc = static_mesh_comp_class()
        -- Movable (not yet simulating) pieces count too on destructibles and
        -- every state-bearing kind: planks start kinematic until their
        -- StartUp Delay, lever-gated blocks until the lever fires.
        local destruct = cls and ((cls:find("Destruct") or cls:find("Barrel") or cls:find("Fence")) ~= nil
                                  or W2.state_kind(cls) ~= nil)
        local arr
        pcall(function() arr = a:K2_GetComponentsByClass(smc) end)
        if arr then
            pcall(function()
                arr:ForEach(function(_, e)
                    if #out >= MAX_BODIES_ACTOR then return end
                    local c = e:get()
                    if valid(c) then
                        local sim = is_sim(c)
                        local mob = field(c, "Mobility")
                        if sim or (destruct and mob == 2) then
                            out[#out + 1] = { fname_of(c) or ("c" .. #out), c, sim }
                        end
                    end
                end)
            end)
        end
        if #out == 0 then return nil, "no-sim-body" end
        table.sort(out, function(x, y) return x[1] < y[1] end)
        return out
    end
end

local function start_scan()
    local q = {}
    local function add(list, kind)
        for _, a in pairs(list or {}) do q[#q + 1] = { a, kind } end
    end
    pcall(function() add(FindAllOf(WEAPON_CLASS), "weapon") end)
    pcall(function() add(FindAllOf("StaticMeshActor"), "sma") end)
    for _, cn in ipairs(PROP_CLASSES) do pcall(function() add(FindAllOf(cn), "prop") end) end
    W.scan = { q = q, i = 1, cands = {}, seen = {}, scands = {} }
end

local function finish_scan()
    local s = W.scan
    W.scan = nil
    W.scans_done = W.scans_done + 1
    assign_ids(s.cands, W.taken)
    for _, c in ipairs(s.cands) do
        local p, r = body_pose(c.body)
        local o = {
            lid = c.lid, chash = c.chash, cls = c.cls, comp = c.comp, kind = c.kind,
            actor = c.actor, actor_addr = c.addr, body = c.body, sim0 = c.sim, sim = c.sim, probed_at = clock_ms(),
            spawn_pos = p or c.pos, spawn_rot = r or { Pitch = 0, Yaw = 0, Roll = 0 },
            anchor_pos = p or c.pos, anchor_rot = r or { Pitch = 0, Yaw = 0, Roll = 0 },
            dyn_owner = 0, buf = {}, prio = 0, last_read = -999, last_probe = -999,
            stable = c.stable, full = c.full,
        }
        o.pos, o.rot, o.vel = o.spawn_pos, o.spawn_rot, v3(0, 0, 0)
        W.objs[#W.objs + 1] = o
        W.by_lid[o.lid] = o
        if o.kind == "weapon" and o.actor_addr then W.by_addr[o.actor_addr] = o end
    end
    assign_ids(s.scands, W.taken)
    for _, c in ipairs(s.scands) do
        local r = {
            lid = c.lid, chash = c.chash, cls = c.cls, comp = c.comp, kind = "state", skind = c.skind,
            group = c.group, cons = c.cons, actor = c.actor, actor_addr = c.addr, spawn_pos = c.pos,
            dyn_owner = 0, stable = c.stable, changed_at = 0, sent_at = -1e9, apply_tries = 0,
        }
        W.actors[#W.actors + 1] = r
        W.by_lid[r.lid] = r
    end
    local counts = { weapon = 0, sma = 0, prop = 0 }
    local pts = {}
    for _, o in ipairs(W.objs) do
        counts[o.kind] = (counts[o.kind] or 0) + 1
        if o.dyn_owner == 0 and pose_ok(o.spawn_pos) then pts[#pts + 1] = o.spawn_pos end
    end
    W.bounds = bounds_of(pts) or W.bounds
    local sk = {}
    for k, v in pairs(W.skipped) do sk[#sk + 1] = k .. "=" .. v end
    table.sort(sk)
    Log("scan %d: +%d bodies -> %d total (weapons=%d props=%d multi-body=%d), +%d actor states -> %d; skipped: %s",
        W.scans_done, #s.cands, #W.objs, counts.weapon, counts.sma, counts.prop, #s.scands, #W.actors,
        table.concat(sk, " "))
    W.skipped = {}
    W.mout_dirty = true
    W.last_manifest = nil   -- re-bind against the manifest with the new bodies
    if W.scans_done == #SCAN_AT_MS then
        local f = io.open(SCAN_FILE, "wb")
        if f then
            f:write(string.format("world %s level=%d bodies=%d\n", W.name, W.level, #W.objs))
            for _, o in ipairs(W.objs) do
                f:write(string.format("%10d %-6s %-44s %-22s sim=%s %s pos=(%.0f,%.0f,%.0f) %s\n", o.lid, o.kind,
                    o.cls, o.comp, tostring(o.sim0), o.stable and "placed " or "runtime", o.spawn_pos.X,
                    o.spawn_pos.Y, o.spawn_pos.Z, o.full or ""))
            end
            for _, r in ipairs(W.actors) do
                local names = {}
                for _, c in ipairs(r.cons) do names[#names + 1] = c[1] end
                f:write(string.format("%10d state  %-44s %-22s kind=%s %s pos=(%.0f,%.0f,%.0f) constraints=[%s]\n",
                    r.lid, r.cls, r.comp, r.skind, r.stable and "placed " or "runtime", r.spawn_pos.X, r.spawn_pos.Y,
                    r.spawn_pos.Z, table.concat(names, ",")))
            end
            for _, l in ipairs(W.scan_report) do f:write(l, "\n") end
            f:close()
            Log("discovery report written: %s", SCAN_FILE)
        end
    end
end

local function scan_step()
    local s = W.scan
    local n = 0
    while s.i <= #s.q and n < SCAN_SLICE do
        local a, kind = s.q[s.i][1], s.q[s.i][2]
        s.i = s.i + 1
        n = n + 1
        local ad = addr(a)
        if ad and not s.seen[ad] then
            s.seen[ad] = true
            local ok, why = live_actor(a)
            if ok and kind == "prop" and not W.known_state[ad] then
                W2.add_state_cands(s, a, ad, cls_short(a) or "?")
            end
            if not ok then
                skip(why, a)
            elseif W.known_actor[ad] then
                skip("known", a)
            else
                local cls = cls_short(a) or "?"
                local bodies, why2 = actor_bodies(a, kind, cls)
                if not bodies then
                    if kind ~= "sma" then skip(why2, a) else W.skipped["sma-static"] = (W.skipped["sma-static"] or 0) + 1 end
                else
                    W.known_actor[ad] = true
                    local name = fname_of(a)
                    local full; pcall(function() full = a:GetFullName() end)
                    for _, b in ipairs(bodies) do
                        local p = body_pose(b[2])
                        if p then
                            s.cands[#s.cands + 1] = {
                                cls = cls, comp = b[1], body = b[2], sim = b[3], actor = a, addr = ad,
                                kind = kind, name = name, levelpath = level_path(a), pos = p,
                                stable = stable_name(name), full = full,
                            }
                        end
                    end
                end
            end
        end
    end
    if s.i > #s.q then finish_scan() end
end

-- --- ownership ---------------------------------------------------------------------------
local function rec_of(o)
    return o.nid and W.owners[o.nid] or nil
end

local function owner_of(o)
    local r = rec_of(o)
    return r and r.owner or 0, r and r.mode or MODE_FREE
end

-- Me, optimistically, while my claim on a free/touched body is in flight.
local function effective_owner(o)
    local ow, mode = owner_of(o)
    if ow == sess.my_id then return ow, mode end
    local p = W.pending[o.nid]
    if p and p.mode ~= MODE_FREE and (ow == 0 or (p.mode >= MODE_HOLD_R and mode == MODE_TOUCH)) then
        return sess.my_id, p.mode
    end
    return ow, mode
end

-- 0.1 uu (positions) / 1 uu/s rounding of the records.
function W2.rnd(x, k) return math.floor(F(x) * k + 0.5) / k end

local function send_claim(o, mode, rest_flags)
    local rest = nil
    if mode == MODE_FREE and o.pos and o.rot and pose_ok(o.pos, W.bounds) then
        rest = W2.qobj(o.nid, o.pos, o.rot, o.vel, rest_flags or (WF_ASLEEP | (o.sim and WF_SIM or 0)))
    end
    return W2.claim(o.nid, mode, rest)
end

local claim_log_budget = 40
local function want_claim(o, mode, why)
    if not o.nid or not W.synced then return end
    local p = W.pending[o.nid]
    if p and p.mode == mode then return end
    local now = clock_ms()
    if mode ~= MODE_FREE and o.claim_cooldown and now < o.claim_cooldown then return end
    W.pending[o.nid] = { mode = mode, req = send_claim(o, mode), sent = now, start = now, why = why }
    W.stats.claims = W.stats.claims + 1
    if claim_log_budget > 0 then
        claim_log_budget = claim_log_budget - 1
        local ow = owner_of(o)
        Log("%s %s (%s; owner was %d)", mode == MODE_FREE and "release" or ("claim mode " .. mode), obj_label(o), why, ow)
    end
end

local unresolved_n, unresolved_next = 0, 0   -- log budget
local function service_pending(now)
    for nid, p in pairs(W.pending) do
        local o = W.by_nid[nid]
        local r = W.owners[nid]
        local ow, mode = r and r.owner or 0, r and r.mode or 0
        local done = (p.mode == MODE_FREE and ow ~= sess.my_id)
                  or (p.mode ~= MODE_FREE and ow == sess.my_id and mode == p.mode)
        if not o then
            W.pending[nid] = nil
        elseif done then
            if p.why and p.why:find("^contact") then W.stats.takeovers = (W.stats.takeovers or 0) + 1 end
            W.pending[nid] = nil
        elseif p.mode ~= MODE_FREE and ow ~= 0 and ow ~= sess.my_id
               and now - p.start > math.min(2 * CLAIM_RESEND_MS, math.max(250, 1.5 * (sess.rtt or 400) + 100)) and not (p.mode >= MODE_HOLD_R and mode == MODE_TOUCH) then
            -- Someone else holds it (or won the race): stop asking for a while.
            W.pending[nid] = nil
            o.claim_cooldown = now + 2000
            if claim_log_budget > 0 then
                claim_log_budget = claim_log_budget - 1
                Log("claim %s refused: owner %d mode %d", obj_label(o), ow, mode)
            end
        elseif now - p.start > CLAIM_TIMEOUT_MS then
            -- budgeted (the first few, then a count every 30 s)
            unresolved_n = unresolved_n + 1
            if unresolved_n <= 10 or now >= unresolved_next then
                unresolved_next = now + 30000
                Log("%s of %s unresolved after %d ms (owner=%d mode=%d); giving up [#%d]",
                    p.mode == MODE_FREE and "release" or "claim", obj_label(o), CLAIM_TIMEOUT_MS, ow, mode, unresolved_n)
            end
            W.pending[nid] = nil
        elseif now - p.sent > CLAIM_RESEND_MS then
            p.sent = now
            send_claim(o, p.mode)   -- idempotent on the server
        end
    end
end

-- --- physics state helpers -----------------------------------------------------------------
local ZERO = v3(0, 0, 0)

-- Physics safety rules (every teleport / SetSimulatePhysics goes through here):
--   * only inside a validated world: callers run after wg_check() in the tick,
--     and nothing is driven while a level change is pending (WG.travel_from);
--   * a destroyed actor / component is noticed while it is merely pending
--     kill (we probe far more often than GC runs) and its reference is then
--     dropped for good (o.dead / o.body = nil), never touched again;
--   * never fight a native holder: a body attached to another actor or a
--     weapon held by any Willie (my pawn or a stand-in) is left alone;
--   * no redundant toggles (each SetSimulatePhysics recreates the physics
--     state and wakes neighbours); rest poses are put to sleep.
local function alive(o)
    if o.dead then return false end
    if not valid(o.actor) then
        o.dead, o.actor, o.body = true, nil, nil
        return false
    end
    if o.body ~= nil and not valid(o.body) then
        o.body = nil
        if o.kind ~= "weapon" then o.dead = true; return false end   -- broken off / destroyed piece
    end
    o.probed_at = clock_ms()
    return true
end

-- An object that has not been probed for PROBE_STALE_MS may have been
-- destroyed AND garbage-collected since: IsValid on it reads freed memory.
-- Such objects are never touched (restore_all / release_world skip them);
-- update_body probes unbound ones on a slow cadence so they stay fresh.
local PROBE_STALE_MS = 3000
local function probed_recently(o, now)
    return o.probed_at ~= nil and (now or clock_ms()) - o.probed_at < PROBE_STALE_MS
end

local function natively_held(o)
    if o.kind == "weapon" then
        if field(o.actor, "Is Held") == true then return true end
        if valid(field(o.actor, "Parent Actor")) then return true end
    end
    local ok, parent = pcall(RD.parent, o.actor)
    if not ok then parent = nil end
    return valid(parent) and not W2.attach_ok(cls_short(parent))
end

local function can_drive(o)
    if WG.travel_from ~= nil then return false end
    if not alive(o) or not valid(o.body) then return false end
    return not natively_held(o)
end
T.can_drive = can_drive

local function set_sim(o, on)
    if not can_drive(o) then return false end
    if on == false and W2.note_freeze and not W2.note_freeze(o) then return false end
    local read_ok, current = pcall(RD.sim, o.body)
    if not read_ok then return false end
    if current == on then
        o.sim = on
        if on and W2.clear_freeze then W2.clear_freeze(o) end
        return true
    end
    local ok = pcall(function() o.body:SetSimulatePhysics(on) end)
    if not ok then return false end
    local verified, actual = pcall(RD.sim, o.body)
    if not verified then return false end
    o.sim = actual == true
    if o.sim ~= on then return false end
    o.sim = on
    if on and W2.clear_freeze then W2.clear_freeze(o) end
    return true
end
T.set_sim = set_sim

-- End an anchor pin (o.pin_kin: a prop held kinematic on its anchor). Must
-- run on a same-world restore, a session end and a peer-id change, not only
-- when a Willie comes near: a prop left pinned stays kinematic for good
-- (free_update skips it, a new anchor never applies, Ready waits for
-- READY_TIMEOUT_MS, the next scan skips it as "static"). restore = put the
-- scene's own simulation flag (sim0) back (only when we may drive the body).
function W2.unpin(o, restore)
    local was = o.pin_kin or o.pin_pending
    if was and restore and o.sim0 ~= nil and o.sim ~= o.sim0 and not set_sim(o, o.sim0) then return false end
    o.pin_kin, o.pinned_at, o.pin_pending = nil, nil, nil
    return was
end

-- rest = true: the target is a rest pose (anchor / final frame): zero the
-- velocities and put the body to sleep so it doesn't jitter or slide off.
local function teleport(o, pos, rot, vel, rest)
    if not can_drive(o) then return false end
    if not pose_ok(pos, W and W.bounds) then
        -- Never place a body at the origin / outside the arena (a destroyed
        -- actor's pose, a bad anchor): that is how items "vanish".
        W.stats.bad_pose = (W.stats.bad_pose or 0) + 1
        if not o.bad_pose_logged then
            o.bad_pose_logged = true
            Log("refused to move %s to (%.0f,%.0f,%.0f): origin / outside the arena", obj_label(o),
                pos and pos.X or 0, pos and pos.Y or 0, pos and pos.Z or 0)
        end
        return false
    end
    local ok = pcall(function()
        o.body:K2_SetWorldLocationAndRotation(pos, rot, false, {}, true)
        if o.sim then
            o.body:SetPhysicsLinearVelocity(vel or ZERO, false, NAME_NONE)
            o.body:SetPhysicsAngularVelocityInDegrees(ZERO, false, NAME_NONE)
            if rest then o.body:PutRigidBodyToSleep(NAME_NONE) end
        end
    end)
    if not ok then return false end
    o.pos, o.rot = pos, rot
    return true
end
T.teleport = teleport

local function read_body(o, now)
    if not alive(o) then return false end
    if o.kind == "weapon" and (not valid(o.body) or now - (o.body_at or 0) > 1000) and not o.kin then
        local b, sim = weapon_body(o.actor)
        if b then o.body, o.sim = b, sim end
        o.body_at = now
    end
    if not valid(o.body) then return false end
    local p, r = body_pose(o.body)
    if not p then return false end
    local sim = is_sim(o.body)
    o.sim = sim
    local vel
    if sim then
        local ok, v = pcall(RD.linvel, o.body)
        if ok then vel = v end
    end
    if not vel then
        local dt = (now - (o.read_at or now)) / 1000
        if o.pos and dt > 0.001 then
            vel = v3((p.X - o.pos.X) / dt, (p.Y - o.pos.Y) / dt, (p.Z - o.pos.Z) / dt)
        else
            vel = v3(0, 0, 0)
        end
    end
    o.prev_pos, o.prev_rot = o.pos, o.rot
    o.pos, o.rot, o.vel, o.read_at = p, r, vel, now
    return true
end

-- --- local hands ---------------------------------------------------------------------------
local hands = { R = nil, L = nil, gR = nil, gL = nil, pawn = nil, pawn_loc = nil }

-- Pass the PlayerController looked up once this tick (UEHelpers.GetPlayerController
-- is a full FindAllOf in UE4SS 3.0.1); never cached across ticks.
local function read_hands(tick_pc)
    hands.R, hands.L, hands.gR, hands.gL, hands.Ra, hands.La = nil, nil, nil, nil, nil, nil
    local okp, pawn = pcall(RD.pawn_of, tick_pc)
    if not okp then pawn = nil end
    hands.pawn = pawn
    hands.pawn_loc = pawn and get_loc(pawn) or nil
    if not pawn then return end
    -- Willie_BP_C property names verified against UE4SS_ObjectDump (spaces!).
    local r, l = field(pawn, "Weapon R"), field(pawn, "Weapon L")
    if valid(r) then hands.R = addr(r); hands.Ra = r end
    if valid(l) then hands.L = addr(l); hands.La = l end
    local gr, gl = field(pawn, "Grab Component R"), field(pawn, "Grab Component L")
    if valid(gr) and field(pawn, "Grabbed R") == true then hands.gR = addr(gr) end
    if valid(gl) and field(pawn, "Grabbed L") == true then hands.gL = addr(gl) end
end

-- MODE_HOLD_R/L if my pawn holds this body, else nil.
local function my_hold_mode(o)
    if o.kind == "weapon" then
        local a = o.actor_addr
        if a and hands.R == a then return MODE_HOLD_R end
        if a and hands.L == a then return MODE_HOLD_L end
    else
        local b = addr(o.body)
        if b and hands.gR == b then return MODE_HOLD_R end
        if b and hands.gL == b then return MODE_HOLD_L end
    end
    return nil
end

-- --- puppets (peer stand-ins) --------------------------------------------------------------
local function refresh_puppets(now)
    if now < W.puppet_next then return end
    W.puppet_next = now + 1000
    -- HSMPAvatars' stand-ins (bus key "puppets", typed)
    local ipc = rawget(_G, "HSMP_IPC")
    -- the peers' round trips (server-measured): the follower's one-way estimate
    pcall(function()
        local d = ipc and ipc.peer_dir and ipc.peer_dir()
        local r = {}
        for id, e in pairs((d and d.by_id) or {}) do
            local v = tonumber(e.rtt_ms)
            if v and v > 0 and v < 2000 then r[id] = v end
        end
        W.peer_rtt = r
    end)
    local pt = ipc and ipc.bus_table("puppets")
    local want = {}
    if type(pt) == "table" then
        for _, r in ipairs(pt.rows or {}) do
            if r.peer and r.peer ~= 0 and type(r.name) == "string" and r.name ~= "" then want[r.name] = r.peer end
        end
    end
    -- No stand-ins listed (HSMPAvatars clears the puppets rows
    -- on every world drop): never walk the Willies for nothing.
    if next(want) == nil then W.puppets = {}; return end
    -- Never in the first WG.SETTLE_S of a world (on_tick gates too).
    if not WG.settled() then W.puppet_next = now; return end
    local list = {}
    pcall(function()
        for _, w in pairs(FindAllOf("Willie_BP_C") or {}) do
            local nm = fname_of(w)
            if nm and want[nm] then list[#list + 1] = { id = want[nm], actor = w } end
        end
    end)
    W.puppets = list
end

local function nearest_puppet_dist(p)
    local best = math.huge
    for _, pp in ipairs(W.puppets) do
        if not pp.loc_tick or pp.loc_tick ~= W.tick then
            pp.loc = get_loc(pp.actor); pp.loc_tick = W.tick
        end
        if pp.loc then best = math.min(best, dist3(pp.loc, p)) end
    end
    return best
end

-- --- remote samples ------------------------------------------------------------------------
local function ingest_remote(now)
    -- The world_remote record (typed; decoded once per version by the facade).
    local ipc = W2.ipc()
    local t, gen = nil, nil
    if ipc then t, gen = ipc.rec("world_remote") end
    if not t or gen == W.last_remote then return end
    W.last_remote = gen
    local r = W2.remote_from(t)
    if not r or r.level ~= W.level or r.epoch ~= W.epoch then return end
    for _, row in ipairs(r.rows) do
        local o = W.by_nid[row.id]
        if not o and (row.id & 0x80000000) ~= 0 and row.sender ~= 0 and row.sender ~= sess.my_id then
            -- A peer's dynamic item we haven't spawned yet: remember its newest
            -- pose so the local copy appears where it is, not where it was dropped.
            local old = W.unbound_rows[row.id]
            if not old or row.ts >= old.ts then W.unbound_rows[row.id] = row end
        end
        if o and o.kind == "state" then o = nil end
        if o then
            if row.sender == 0 then
                -- Server rest-pose anchor for a free body.
                local key = string.format("%.1f,%.1f,%.1f,%.4f", row.pos.X, row.pos.Y, row.pos.Z, row.q[4])
                if key == o.anchor_old then
                    -- superseded by my own release (see owner_update)
                elseif o.anchor_key ~= key and not pose_ok(row.pos, W.bounds) then
                    o.anchor_key = key
                    if not o.bad_anchor_logged then
                        o.bad_anchor_logged = true
                        Log("ignoring anchor of %s at (%.0f,%.0f,%.0f): origin / outside the arena",
                            obj_label(o), row.pos.X, row.pos.Y, row.pos.Z)
                    end
                elseif o.anchor_key ~= key then
                    o.anchor_key = key
                    o.anchor_pos = row.pos
                    o.anchor_rot = quat_to_rot(row.q[1], row.q[2], row.q[3], row.q[4])
                    o.anchor_new = true
                    o.anchor_sim = (row.flags & WF_SIM) ~= 0
                    -- Initial state (before Ready): forced exactly, not
                    -- only beyond the 10 cm drift rule.
                    if not W.ready then o.force_exact = true end
                    W.stats.snaps = W.stats.snaps + 1
                end
            elseif row.sender ~= sess.my_id then
                local key = row.sender .. ":" .. row.seq
                if o.consumed ~= key then
                    o.consumed = key
                    local FW = K.FW
                    o.fw = o.fw or {}
                    if o.buf_sender ~= row.sender then
                        -- a new owner: its own timeline; blend from what is on screen
                        if o.buf_sender ~= nil then FW.rebase(o.fw) end
                        o.buf, o.buf_sender = {}, row.sender
                    end
                    local buf = o.buf
                    if #buf > 0 and math.abs(row.ts - buf[#buf].t) > 5000 then FW.rebase(o.fw); buf = {}; o.buf = buf end
                    local smp = { t = row.ts, pos = row.pos, q = row.q, vel = row.vel, flags = row.flags, rx = now }
                    -- A gap in this sender's stream (it slept, or this is a new lease of the
                    -- same peer): never interpolate across it. A nearby last pose is where
                    -- the body rested until just now; anything else is stale.
                    local last = buf[#buf]
                    if last and smp.t - last.t > FW.GAP_MS then
                        FW.before_change(o.fw, buf)
                        buf = {}
                        o.buf = buf
                        if dist3(last.pos, smp.pos) < FW.GAP_KEEP_CM then
                            buf[1] = { t = smp.t - FW.SEND_MS, pos = last.pos, q = last.q, vel = v3(0, 0, 0), flags = last.flags }
                        end
                    end
                    local i = #buf
                    while i >= 1 and buf[i].t > smp.t do i = i - 1 end
                    if not (buf[i] and buf[i].t == smp.t) then
                        FW.before_change(o.fw, buf)
                        table.insert(buf, i + 1, smp)
                        while #buf > 8 do table.remove(buf, 1) end
                    end
                    o.settled = false
                    -- the sender's clock: one note per packet
                    local fs = row.sender .. ":" .. row.seq
                    if W.fseq[row.sender] ~= fs then
                        W.fseq[row.sender] = fs
                        local c = W.fclock[row.sender]
                        if not c then c = FW.clock_new(); W.fclock[row.sender] = c end
                        FW.clock_note(c, row.ts, now)
                    end
                    W.stats.recv = W.stats.recv + 1
                end
            end
        end
    end
end

-- Where the owner's stand-in is on this screen (cached per tick), for the follower.
function W2.owner_loc(owner)
    for _, pp in ipairs(W.puppets) do
        if pp.id == owner then
            if not pp.loc_tick or pp.loc_tick ~= W.tick then pp.loc = get_loc(pp.actor); pp.loc_tick = W.tick end
            return pp.loc
        end
    end
    return nil
end

-- --- per-body update --------------------------------------------------------------------------
local function become_local(o, why)
    -- A pinned prop that changes hands is no longer pinned.
    if (o.pin_kin or o.pin_pending) and not o.kin then
        if not W2.unpin(o, true) then return false end
    else o.pin_kin, o.pinned_at, o.pin_pending = nil, nil, nil end
    -- Physics back on after following someone else's stream.
    if o.kin then
        local last = o.buf[#o.buf]
        local asleep = last and (last.flags & WF_ASLEEP) ~= 0
        -- Place it at the final rest frame while still kinematic, THEN turn
        -- physics on: enabling simulation first and teleporting a live body
        -- afterwards kicks it (and whatever it overlaps).
        if why == "free" and asleep then
            if not teleport(o, last.pos, quat_to_rot(last.q[1], last.q[2], last.q[3], last.q[4]), ZERO) then return false end
        end
        local want = o.prev_sim_flag
        if want == nil then want = o.sim0 end
        if not set_sim(o, want) then return false end
        o.kin = false
        if last and o.sim and can_drive(o) then
            -- Hinged doors / levers / lids (and anything spinning): keep the
            -- owner's angular velocity, derived from its last two samples
            -- (the wire carries orientation, not spin).
            local prev = o.buf[#o.buf - 1]
            local w = (not asleep and prev) and W2.ang_vel_deg(prev.q, last.q, last.t - prev.t) or ZERO
            pcall(function()
                o.body:SetPhysicsLinearVelocity(asleep and ZERO or last.vel, false, NAME_NONE)
                o.body:SetPhysicsAngularVelocityInDegrees(w, false, NAME_NONE)
                if asleep then o.body:PutRigidBodyToSleep(NAME_NONE) end
            end)
        end
    end
    -- Freed after following an owner: the anchor is the OWNER's final pose
    -- (its last streamed frame, where the body was just placed), the same on
    -- every screen. Not o.pos, the local pose read before that teleport (a
    -- lagging interpolated frame): each follower would pin the prop to its
    -- own slightly different rest pose for good (props several cm / tens of
    -- degrees apart while every client reports 0 cm off its anchor).
    if why == "free" then
        o.freed_at = clock_ms()
        local last = o.buf[#o.buf]
        if last and pose_ok(last.pos, W.bounds) then
            o.anchor_pos = last.pos
            o.anchor_rot = quat_to_rot(last.q[1], last.q[2], last.q[3], last.q[4])
        elseif o.pos then
            o.anchor_pos, o.anchor_rot = o.pos, o.rot
        end
    end
    return true
end
T.become_local = become_local

local function follower_update(o, owner, now)
    if not alive(o) then return end
    if my_hold_mode(o) then
        -- My pawn holds it but the server says another player has it. Once my own
        -- claim has been refused (not pending any more), the server's verdict wins here too.
        local r = rec_of(o)
        if r and r.mode >= MODE_HOLD_R and r.mode < K.MODE_STATE and not W.pending[o.nid] and W2.lose_hold(o, owner, now) then
            return
        end
        if not o.conflict_logged then
            o.conflict_logged = true
            Log("conflict: my pawn holds %s but peer %d owns it; not driving it", obj_label(o), owner)
        end
        if o.kin then become_local(o, "conflict") end
        return
    end
    -- My pawn pushes a body another player's touch lease holds: ask for it. The server
    -- hands a slow body over only when my root is clearly nearer (world.rs TAKE_*).
    local r = rec_of(o)
    local last = o.buf[#o.buf]
    if r and r.mode == MODE_TOUCH and hands.pawn_loc and o.applied_pos and last and vlen(last.vel) < K.TAKE_SPEED then
        local dm = dist3(hands.pawn_loc, o.applied_pos)
        if dm < K.TAKE_R then
            local ol = W2.owner_loc(owner)
            if not ol or dm + K.TAKE_MARGIN <= dist3(ol, o.applied_pos) then
                want_claim(o, MODE_TOUCH, string.format("contact %.0f cm", dm))
            end
        end
    end
    if o.settled and #o.buf > 0 then return end
    local FW = K.FW
    o.fw = o.fw or {}
    local c = W.fclock[o.buf_sender or -1]
    if #o.buf == 0 or not c then return end
    -- wait for a sample of this lease (the buffer may still hold the owner's previous one)
    if not o.kin and (o.buf[#o.buf].rx or 0) < (o.lease_at or 0) then return end
    if not o.kin then
        -- A weapon's simulated part can change (spawned copies enable their
        -- physics in BeginPlay): pick it now, before we freeze it.
        if o.kind == "weapon" then
            local b, sim = weapon_body(o.actor)
            if b then o.body, o.sim = b, sim end
        end
        if not can_drive(o) then
            if not o.native_logged then
                o.native_logged = true
                Log("not following %s: held/attached natively here", obj_label(o))
            end
            return
        end
        o.prev_sim_flag = nil
        if not set_sim(o, false) then return end
        o.kin = true
        -- start from where it is here and blend onto the owner's stream
        FW.reset(o.fw)
        local p, r = body_pose(o.body)
        if p and r then
            local qt = rot_to_quat_t(r)
            o.fw.last_p, o.fw.last_q, o.fw.rebase = p, { qt.X, qt.Y, qt.Z, qt.W }, true
        end
    end
    local ol = W2.owner_loc(owner)
    local s = o.buf[#o.buf]
    -- a stream at the origin / outside the arena is junk: teleport refuses (and logs) it
    if not pose_ok(s.pos, W.bounds) then teleport(o, s.pos, o.rot or { Pitch = 0, Yaw = 0, Roll = 0 }); return end
    local lead = 0.5 * ((sess.rtt or 0) + (W.peer_rtt[owner] or sess.rtt or 0)) + 20
    local pos, q, flags, snapped = FW.pose(o.fw, o.buf, c, {
        now = now, dt = (W.dt or 16) / 1000, lead = lead, owner_dist = ol and dist3(ol, s.pos) or nil,
        floor = math.min(o.spawn_pos and o.spawn_pos.Z or math.huge, o.anchor_pos and o.anchor_pos.Z or math.huge) - 2 })
    if not pos then return end
    if snapped then W.stats.hard_snaps = (W.stats.hard_snaps or 0) + 1 end
    o.prev_sim_flag = (flags & WF_SIM) ~= 0
    local rot = quat_to_rot(q[1], q[2], q[3], q[4])
    if o.applied_pos and dist3(o.applied_pos, pos) < 0.1 and rot_diff(o.applied_rot, rot) < 0.1 then
        if (s.flags & WF_ASLEEP) ~= 0 then o.settled = true end
        return
    end
    if teleport(o, pos, rot) then
        o.applied_pos, o.applied_rot = pos, rot
        W.stats.follow = W.stats.follow + 1
        W.kin_now = W.kin_now or {}
        W.kin_now[#W.kin_now + 1] = o
    end
end

local function i_touched(o)
    if hands.pawn_loc and dist3(hands.pawn_loc, o.pos) < NEAR_DIST
       and dist3(hands.pawn_loc, o.pos) <= nearest_puppet_dist(o.pos) then
        return true
    end
    for _, x in ipairs(W.mine_awake or {}) do
        if x ~= o and x.pos and dist3(x.pos, o.pos) < MINE_NEAR then return true end
    end
    return false
end

local function pin_anchor(o, now)
    -- Retain the transition if freezing succeeds but placement fails.
    -- The next tick retries placement rather than forgetting a frozen body.
    o.pin_pending = true
    if not set_sim(o, false) or not teleport(o, o.anchor_pos, o.anchor_rot, ZERO, true) then return false end
    o.pin_kin, o.pinned_at, o.pin_pending = true, now, nil
    return true
end
T.pin_anchor = pin_anchor

local function free_update(o, now)
    if o.kin and not become_local(o, "free") then return end
    if o.pin_pending then
        if not pin_anchor(o, now) then return end
        return
    end
    -- A pinned prop is held kinematic on its anchor (see the anchor pin below)
    -- until any Willie comes within PIN_FREE_DIST: physics is back on well
    -- before anyone can touch it (checked every tick, cheap: one distance to
    -- my pawn and the per-tick cached puppet locations).
    if o.pin_kin then
        local p = o.anchor_pos or o.pos
        local mine = hands.pawn_loc and p and dist3(hands.pawn_loc, p) or math.huge
        if o.anchor_new then
            -- A new / healed anchor arrived: un-pin and apply it below
            -- (the anchor handling restores the anchor's simulation flag).
            if not W2.unpin(o, true) then return end
        elseif not p or mine < K.PIN_FREE_DIST or nearest_puppet_dist(p) < K.PIN_FREE_DIST then
            if p and o.anchor_rot and not teleport(o, p, o.anchor_rot, ZERO, true) then return end
            if not set_sim(o, true) then return end
            o.pin_kin = nil
        else
            return
        end
    end
    -- Probe rate: near my pawn often, else rarely.
    local near = hands.pawn_loc and o.pos and dist3(hands.pawn_loc, o.pos) < 800
    local every = near and 4 or 30
    if not o.anchor_new and W.tick - o.last_probe < every then return end
    o.last_probe = W.tick
    if not read_body(o, now) then return end
    local off = dist3(o.pos, o.anchor_pos)
    local spd = vlen(o.vel)
    if o.anchor_new then
        -- Initial / healed anchors are forced exactly; ordinary
        -- anchor updates only beyond the drift rule.
        local exact = o.force_exact
        local need = off > ANCHOR_DRIFT
            or (exact and (off > K.FORCE_POS or rot_diff(o.rot, o.anchor_rot) > K.FORCE_ROT))
        if need and not (near and i_touched(o)) then
            -- Rest pose first (kinematic if it will end kinematic), then the
            -- simulation flag of the anchor.
            if o.anchor_sim == false and o.sim and not set_sim(o, false) then return end
            if not teleport(o, o.anchor_pos, o.anchor_rot, ZERO, true) then return end
            if o.anchor_sim == true and not o.sim then
                if not set_sim(o, true) or not teleport(o, o.anchor_pos, o.anchor_rot, ZERO, true) then return end
            end
            o.anchor_new, o.force_exact = false, nil
            if exact then o.forced = true; W.stats.forced = (W.stats.forced or 0) + 1 end
            o.drift_since = nil
            return
        end
        o.anchor_new, o.force_exact = false, nil
        if exact then o.forced = true end
    end
    -- Anchor pin: a server-anchored body that nobody touches must stay on the shared
    -- pose. Each client's solver settles it independently (a few cm / degrees apart
    -- between screens, below TOUCH_MOVE), so it is pinned back onto the anchor and put
    -- to sleep whenever it is off by more than the exact-force tolerance (position or
    -- rotation) while slow and untouched. A real disturbance (pushed by my pawn or one
    -- of my bodies) still hands it to a lease below.
    local adiff = (o.rot and o.anchor_rot) and rot_diff(o.rot, o.anchor_rot) or 0
    local off_anchor = off > K.FORCE_POS or adiff > K.FORCE_ROT
    local disturbed = spd > TOUCH_SPEED or off > TOUCH_MOVE
    if not disturbed then
        o.drift_since = nil
        if o.anchor_key then
            -- (proximity is not a touch: otherwise a resting prop near a spawn point stays
            -- off its anchor on that client for whole rounds because "my pawn is near")
            if off_anchor and spd < SETTLE_SPEED and not (o.nid and W.pending[o.nid])
               and now - (o.pinned_at or -1e9) >= K.PIN_EVERY_MS and pose_ok(o.anchor_pos, W.bounds) then
                if not teleport(o, o.anchor_pos, o.anchor_rot, ZERO, true) then return end
                W.stats.pinned = (W.stats.pinned or 0) + 1
                -- Pinned once already and it settled away again (some props
                -- teleported onto the shared pose re-settle a few cm / degrees off
                -- within a second, every time): the local world does not hold
                -- that pose, so hold it kinematic until a Willie nears.
                if o.pinned_at and o.anchor_sim ~= false and o.sim then
                    if not pin_anchor(o, now) then return end
                    W.stats.pin_kin = (W.stats.pin_kin or 0) + 1
                end
                o.pinned_at = now
            end
            return
        end
        -- Absorb local settling (level start) into the anchor.
        if spd < SETTLE_SPEED and off > 0.5 and off < 15 and not o.anchor_key and pose_ok(o.pos, W.bounds) then
            o.anchor_pos, o.anchor_rot = o.pos, o.rot
        end
        return
    end
    if now - W.t0 > STARTUP_QUIET_MS and i_touched(o) then
        want_claim(o, MODE_TOUCH, string.format("pushed %.0f cm/s", spd))
        o.drift_since = nil
        return
    end
    -- Knocked by a body another player owns (its copy here follows their stream): the hit is
    -- theirs to simulate, and their game claims what it really hit. Here the copy's path is
    -- not the owner's body, so whatever it shoves would end up somewhere no other screen has it.
    -- Hold it on its anchor until the owner's claim (if any) arrives.
    if o.anchor_pos and o.anchor_sim ~= false and o.sim and pose_ok(o.anchor_pos, W.bounds) then
        for _, x in ipairs(W.kin_last or {}) do
            if x ~= o and x.applied_pos and o.pos and dist3(x.applied_pos, o.pos) < K.SHIELD_R then
                if not pin_anchor(o, now) then return end
                o.drift_since = nil
                W.stats.shielded = (W.stats.shielded or 0) + 1
                return
            end
        end
    end
    -- Just released by its owner and moving here on its own: this solver does not hold the
    -- owner's rest pose (a body resting on a slope or on another body slides off). Hold it
    -- kinematic on that pose at once, until a Willie comes near (the anchor pin below).
    if o.freed_at and now - o.freed_at < K.FREED_HOLD_MS and o.anchor_pos and pose_ok(o.anchor_pos, W.bounds)
       and o.anchor_sim ~= false and o.sim then
        if not pin_anchor(o, now) then return end
        o.drift_since, o.freed_at = nil, nil
        W.stats.pin_kin = (W.stats.pin_kin or 0) + 1
        return
    end
    -- Knocked off its anchor and already at rest again, nobody of mine near and
    -- no claim pending: nothing is going to move it further, so the grace below
    -- only keeps the screens apart (a prop can lie well off its anchor on one
    -- client at the round's last consistency check). Pin it back at once
    -- (rate-limited like the pin above).
    if o.anchor_key and spd < SETTLE_SPEED and not (o.nid and W.pending[o.nid])
       and now - (o.pinned_at or -1e9) >= K.PIN_EVERY_MS and pose_ok(o.anchor_pos, W.bounds) then
        if not teleport(o, o.anchor_pos, o.anchor_rot, ZERO, true) then return end
        -- Pinned before and it came to rest off the anchor again: this world
        -- does not hold that pose (re-pinning every second does not keep it
        -- there). Hold it kinematic until a Willie comes near, exactly like the
        -- in-tolerance pin above.
        if o.pinned_at and o.anchor_sim ~= false and o.sim then
            if not pin_anchor(o, now) then return end
            W.stats.pin_kin = (W.stats.pin_kin or 0) + 1
        end
        o.pinned_at, o.drift_since = now, nil
        W.stats.snapback = (W.stats.snapback or 0) + 1
        return
    end
    o.drift_since = o.drift_since or now
    if now - o.drift_since > ANCHOR_GRACE_MS and (off > ANCHOR_DRIFT or (o.anchor_key and off_anchor)) then
        if not teleport(o, o.anchor_pos, o.anchor_rot, ZERO, true) then return end
        o.drift_since = nil
        W.stats.snapback = (W.stats.snapback or 0) + 1
    elseif not o.anchor_key and spd < SETTLE_SPEED and pose_ok(o.pos, W.bounds) then
        -- Came to rest somewhere new with nobody near and no server anchor:
        -- the whole lobby saw the same initial settle; accept it.
        o.anchor_pos, o.anchor_rot = o.pos, o.rot
        o.drift_since = nil
    end
end

local function owner_update(o, mode, now, send_tick)
    local held = my_hold_mode(o)
    if o.kin and not become_local(o, "owner") then return end
    if not send_tick then return end
    if not read_body(o, now) then return end
    local moved = o.prev_pos and dist3(o.pos, o.prev_pos) or 1e9
    local turned = o.prev_rot and rot_diff(o.rot, o.prev_rot) or 1e9
    local active = held or vlen(o.vel) > AWAKE_SPEED or moved > AWAKE_MOVE or turned > AWAKE_ROT
    if active then
        o.still_since = nil
        if not o.awake then o.awake = true end
    else
        o.still_since = o.still_since or now
        if o.awake and now - o.still_since >= SLEEP_AFTER_MS then
            o.awake = false
            o.asleep_repeat = ASLEEP_REPEAT
        end
    end
    -- Release at rest (granted leases only, never while held / in flight).
    local ow = owner_of(o)
    if not held and not o.awake and ow == sess.my_id and not W.pending[o.nid]
       and o.still_since and now - o.still_since >= RELEASE_AFTER_MS then
        want_claim(o, MODE_FREE, "at rest")
        if pose_ok(o.pos, W.bounds) then o.anchor_pos, o.anchor_rot = o.pos, o.rot end
        -- the old anchor is superseded by this rest pose: never apply it again (the
        -- sidecar may still list it until the server's new anchor arrives)
        if o.anchor_key then o.anchor_old = o.anchor_key end
        o.anchor_key, o.anchor_new = nil, false
    end
end

local function update_body(o, now, send_tick)
    if o.dead then return end
    if not o.nid then
        -- Unbound (refused / not in the manifest) bodies are probed too, on
        -- a slow cadence, so a destroyed one is marked dead before it is GC'd.
        if W.tick % 30 == 0 then alive(o) end
        return
    end
    if W.tick % 30 == 0 and not alive(o) then return end   -- free bodies may sleep between probes
    local held = my_hold_mode(o)
    local ow, mode = owner_of(o)
    if held then
        if not (ow == sess.my_id and mode == held) then want_claim(o, held, "picked up") end
    elseif ow == sess.my_id and mode >= MODE_HOLD_R then
        want_claim(o, MODE_TOUCH, "let go (drop/disarm)")
    end
    local eo, emode = effective_owner(o)
    -- The body left the free state (owned / followed): no pin.
    if (o.pin_kin or o.pin_pending) and eo ~= 0 and not W2.unpin(o, true) then return end
    if eo == sess.my_id then
        if not o.was_mine then o.was_mine = true; o.awake = true; o.still_since = nil; o.consumed = nil end
        -- held items and bodies at my pawn stream at the fast rate (every 2nd tick), the
        -- rest every SEND_EVERY (world_follow.lua FAST_SEND_MS / SEND_MS)
        local fast = W2.fast(o, held)
        owner_update(o, emode, now, (fast and W.fast_tick) or (not fast and send_tick))
        if o.awake or (o.asleep_repeat or 0) > 0 then W.mine_awake[#W.mine_awake + 1] = o end
    else
        if o.was_mine then o.was_mine = false; o.awake = false; o.asleep_repeat = 0 end
        if eo ~= 0 then
            follower_update(o, eo, now)
        else
            free_update(o, now)
        end
    end
end

-- --- sending ------------------------------------------------------------------------------------
-- Held, or moving at my pawn (in contact): streamed at the fast rate.
function W2.fast(o, held)
    return held ~= nil or (hands.pawn_loc ~= nil and o.pos ~= nil and dist3(hands.pawn_loc, o.pos) < NEAR_DIST)
end

local function send_states(now, send_tick)
    local list = {}
    for _, o in ipairs(W.mine_awake) do
        local held = my_hold_mode(o)
        local fast = W2.fast(o, held)
        local due = (fast and W.fast_tick) or (not fast and send_tick)
        if due and o.pos and o.rot and pose_ok(o.pos, W.bounds) then
            local w = held and 8 or (vlen(o.vel) > 300 and 4) or (o.awake and 2) or 1
            if (o.asleep_repeat or 0) > 0 then w = 6 end
            if hands.pawn_loc and dist3(hands.pawn_loc, o.pos) > 1000 then w = w * 0.5 end
            o.prio = (o.prio or 0) + w
            list[#list + 1] = o
        end
    end
    if #list == 0 then return end
    table.sort(list, function(a, b) return a.prio > b.prio end)
    -- The world_out record: one WorldObj row per body, quantised here (W2.qobj).
    local objs = {}
    for i = 1, math.min(#list, SEND_MAX) do
        local o = list[i]
        local held = my_hold_mode(o)
        local flags = (o.sim and WF_SIM or 0) | (held and WF_HELD or 0) | (held == MODE_HOLD_L and WF_LEFT or 0)
        if not o.awake then
            flags = flags | WF_ASLEEP
            o.asleep_repeat = math.max((o.asleep_repeat or 1) - 1, 0)
        end
        objs[#objs + 1] = W2.qobj(o.nid, o.pos, o.rot, o.vel, flags)
        o.prio = 0
    end
    send_seq = send_seq + 1   -- module scope, never restarts with a new W (receivers drop seq <= last)
    W.seq = send_seq
    local ts = math.floor(now) % 4294967296
    local ipc = W2.ipc()
    if ipc then ipc.put("world_out", { level = W.level, epoch = W.epoch or 0, seq = W.seq, ts = ts, rows = objs }) end
    W.stats.sent = W.stats.sent + #objs
end

-- --- dynamic items (my weapons that leave my hand) ---------------------------------------------
-- Is this a real drop / disarm? It happens at my hand, inside the arena, and
-- the weapon is loose. A weapon the game or HSMPLoadout destroyed or swapped
-- out (fists re-arm, kit re-apply, round reset) reads back at the origin
-- (seen in-game as "world item ... at (0,0,0)") or far away.
local function drop_verdict(p, attached, pawn_loc, bounds)
    if p == nil then return "no-pose" end
    if at_origin(p) then return "origin" end
    if attached then return "attached" end
    if not pose_ok(p, bounds) then return "out-of-arena" end
    if pawn_loc and dist3(p, pawn_loc) > MY_DROP_R then return "far-from-me" end
    return "ok"
end
T.drop_verdict = drop_verdict

-- Body parts: never world items.
local NOT_ITEMS = { Weapon_Fists_C = true, Weapon_Feet_C = true }

-- The kit re-arm window, computed exactly like HSMPLoadout's
-- kit.rearm_window (HSMPLoadout/Scripts/kit.lua): both records read fresh, the
-- spawn protection counts only for MY pawn. true, why while a drop of my kit
-- weapon is put back in hand by the kit (so it is not a world item yet).
T.drop_window = function(pawn, now_s)
    now_s = now_s or os.clock()
    local v = sess.HSM and sess.HSM.view()
    local st = v and v.state or "lobby"
    if st == "countdown" or st == "paused" then return true, st end
    local ss = sess.spawn_status()
    local pn = pawn and fname_of(pawn)
    if ss and pn and ss.pawn == pn then
        if not ss.has_protect_until then return true, "spawn protection" end
        if now_s < ss.protect_until then return true, "spawn protection" end
    end
    return false, st
end

local function track_my_items(now)
    if not hands.pawn then return end
    for _, side in ipairs({ "R", "L" }) do
        local a, ad = hands[side .. "a"], hands[side]
        if ad and not W.my_items[ad] and not W.by_addr[ad] and not NOT_ITEMS[cls_short(a) or ""] then
            W.my_items[ad] = { actor = a, side = side }
        end
    end
    for ad, it in pairs(W.my_items) do
        if ad == hands.R or ad == hands.L then
            it.loose_since = nil
        elseif not valid(it.actor) then
            W.my_items[ad] = nil
        elseif field(it.actor, "Is Held") ~= true and not valid(field(it.actor, "Parent Actor")) then
            it.loose_since = it.loose_since or now
            if now - it.loose_since > DROP_CONFIRM_MS and W.synced and W.dyn_ctr < 0xFFFF then
                -- Pose from the weapon's simulating part, read at least
                -- DROP_CONFIRM_MS after it left the hand (never in the detach
                -- frame); if it isn't a plausible drop pose yet, look again on
                -- later ticks and only give up after DROP_GIVEUP_MS.
                local body, sim = weapon_body(it.actor)
                local p, r = nil, nil
                if body then p, r = body_pose(body) end
                local attached
                pcall(function() attached = it.actor:GetAttachParentActor() end)
                local verdict = drop_verdict(p, valid(attached), hands.pawn_loc, W.bounds)
                if verdict ~= "ok" then
                    if now - it.loose_since > DROP_GIVEUP_MS then
                        W.my_items[ad] = nil
                        Log("my %s-hand item %s left my hand but is not a drop (%s at (%.0f,%.0f,%.0f)); not a world item",
                            it.side, cls_path(it.actor) or "?", verdict, p and p.X or 0, p and p.Y or 0, p and p.Z or 0)
                    end
                    p = nil
                else
                    -- The same window HSMPLoadout's kit re-arm uses
                    -- (kit.rearm_window: fresh records, own pawn). Inside it the
                    -- kit puts this actor back in hand; the entry is KEPT, so
                    -- if the re-arm did not take (a new kit weapon was given
                    -- instead) the still-loose actor becomes a world item as
                    -- soon as the window closes.
                    local inwin, why = T.drop_window(hands.pawn)
                    if inwin then
                        if not it.suppressed then
                            Log("my %s-hand item %s left my hand during the kit re-arm window (%s): not a world item (yet)",
                                it.side, cls_path(it.actor) or "?", why)
                        end
                        it.suppressed = why
                        p = nil
                    else
                        if it.suppressed then
                            Log("my %s-hand item %s is still loose after the kit re-arm window (%s): registering it",
                                it.side, cls_path(it.actor) or "?", it.suppressed)
                        end
                        W.my_items[ad] = nil
                    end
                end
                if p then
                    local cls = cls_short(it.actor) or "?"
                    local nid = 0x80000000 | ((sess.my_id & 0x7FFF) << 16) | (W.dyn_ctr & 0xFFFF)
                    W.dyn_ctr = W.dyn_ctr + 1
                    local o = {
                        lid = nid, nid = nid, chash = fnv1a(cls .. "|W"), cls = cls, comp = "W", kind = "weapon",
                        actor = it.actor, actor_addr = ad, body = body, sim0 = sim, sim = sim, probed_at = clock_ms(),
                        spawn_pos = p, spawn_rot = r, anchor_pos = p, anchor_rot = r, pos = p, rot = r,
                        vel = v3(0, 0, 0), dyn_owner = sess.my_id, class_path = cls_path(it.actor),
                        passport = W2.encode_passport(it.actor),
                        buf = {}, prio = 0, last_read = -999, last_probe = -999,
                    }
                    W.objs[#W.objs + 1] = o
                    W.by_lid[nid] = o
                    W.by_nid[nid] = o
                    W.by_addr[ad] = o
                    W.known_actor[ad] = true
                    W.mout_dirty = true
                    -- Remember the ids I created (plain numbers, per
                    -- level): after a peer-id change the server still lists
                    -- them under my OLD id, and they must not come back to me
                    -- as a peer's item.
                    sess.my_nids[nid] = W.level
                    -- Optimistic: stream now (implicit claim), confirm by claim.
                    W.pending[nid] = { mode = MODE_TOUCH, req = 0, sent = 0, start = now, why = "dropped item" }
                    Log("my %s-hand item %s left my hand -> world item %d at (%.0f,%.0f,%.0f)",
                        it.side, o.class_path or cls, nid, p.X, p.Y, p.Z)
                end
            end
        end
    end
end

-- --- remote dynamic items (a peer's dropped / disarmed weapon) ----------------------------
-- UE 5.4's GameplayStatics::FinishSpawningActor takes three parameters
-- (Actor, SpawnTransform, TransformScaleMethod) and UE4SS refuses a UFunction
-- call with a missing one ("UFunction expected 3 parameters, received 2").
-- BeginDeferredActorSpawnFromClass has already succeeded by then, so a failed
-- finish leaves a half-constructed actor at the origin. Hence: pass every
-- parameter, configure the deferred actor before its construction script runs
-- (passport, physics), and retire the half-spawned actor if finishing fails.

-- Only Blueprint weapon classes (the server checks too): never a pawn.
local function weapon_class_path_ok(path)
    return type(path) == "string" and #path <= 200 and path:match("^/Game/[%w_/%.%-]+_C$") ~= nil
        and not is_willie_class(path)
end
T.weapon_class_path_ok = weapon_class_path_ok

-- Resolve (and if needed load) the class; verify it is a ModularWeaponBP.
-- Cached per level (UClass objects are only trusted within one world).
local function resolve_weapon_class(path)
    if not weapon_class_path_ok(path) then return nil, "bad class path" end
    local c = W.classes[path]
    if c ~= nil then
        if c ~= false and valid(c) then return c end
        if c == false then return nil, "not a weapon class" end
    end
    local cls
    pcall(function() cls = StaticFindObject(path) end)
    if not valid(cls) then
        pcall(function() LoadAsset((path:gsub("%.[^%./]+$", ""))) end)
        pcall(function() cls = StaticFindObject(path) end)
    end
    if not valid(cls) then return nil, "class not found" end
    local base
    pcall(function() base = StaticFindObject(WEAPON_BASE_PATH) end)
    if valid(base) then
        local is_weapon = nil
        pcall(function() is_weapon = cls:GetCDO():IsA(base) end)
        if is_weapon == false then W.classes[path] = false; return nil, "not a weapon class" end
    end
    W.classes[path] = cls
    return cls
end

-- The dropping peer's stand-in usually still holds its copy of the weapon
-- (HSMPLoadout): take the passport (modules, sizes, materials) from it.
local function passport_source(e)
    for _, pp in ipairs(W.puppets or {}) do
        if pp.id == e.dyn and valid(pp.actor) then
            for _, k in ipairs({ "Weapon R", "Weapon L", "Weapon R_0", "Weapon L_0" }) do
                local w = field(pp.actor, k)
                if valid(w) and cls_path(w) == e.class then return w end
            end
        end
    end
    return nil
end

-- Remember plain record data while the peer still holds a weapon. The
-- loadout mod may strip its actor before this mod spawns the dropped copy;
-- relying on that actor makes module geometry depend on callback order.
function W2.cache_peer_passports()
    local ipc = W2.ipc()
    if not ipc or not ipc.peer_rec then return end
    for _, pp in ipairs(W.puppets or {}) do
        local rec = ipc.peer_rec("peer_loadout", pp.id)
        if type(rec) == "table" then
            local cache = W.peer_passports[pp.id] or {}
            W.peer_passports[pp.id] = cache
            for i, side in ipairs({ "r", "l" }) do
                local pass = rec[side]
                if ((tonumber(rec.flags) or 0) & (1 << (i - 1))) ~= 0 and type(pass) == "table" then
                    local path = W2.passport_path(pass.class)
                    if path then cache[path] = pass end
                end
            end
        end
    end
end

function W2.passport_path(path)
    if type(path) ~= "string" or path == "" then return nil end
    path = path:gsub("^@", "/Game/Assets/")
    if not path:find("%.") then path = path .. "." .. (path:match("([^/]+)$") or "") .. "_C" end
    return path
end

-- Decode the owner-authored module sizes/materials, without inventing a
-- family template. UClass references are resolved only in this world.
function W2.record_passport(pass)
    local out = {}
    for _, fd in ipairs(PASSPORT_FIELDS) do
        local name, kind, v = fd[1], fd[2], pass[fd[3]]
        if kind == "class" then
            local path = W2.passport_path(v)
            if path then
                local cls
                pcall(function() cls = StaticFindObject(path) end)
                if not valid(cls) then
                    pcall(function() LoadAsset(path:gsub("%.[^%./]+$", "")) end)
                    pcall(function() cls = StaticFindObject(path) end)
                end
                if not valid(cls) then return nil end
                out[name] = cls
            end
        elseif kind == "vec" then
            v = type(v) == "table" and v or {}
            out[name] = { X = v[1] or 0, Y = v[2] or 0, Z = v[3] or 0 }
        elseif kind == "color" then
            v = type(v) == "table" and v or {}
            out[name] = { R = v[1] or 0, G = v[2] or 0, B = v[3] or 0, A = v[4] or 1 }
        elseif kind == "name" then out[name] = FName(type(v) == "string" and v or "")
        else out[name] = tonumber(v) or 0 end
    end
    return out
end
T.record_passport = W2.record_passport
T.passport_path = W2.passport_path
T.cache_peer_passports = W2.cache_peer_passports

-- Capture the actor itself at the drop, not a same-class hand on a peer.
-- All fields are plain values: no actor or UClass survives a world reload.
function W2.encode_passport(actor)
    local sp = field(actor, "Weapon Passport")
    if not sp then return nil end
    local out = {}
    for _, fd in ipairs(PASSPORT_FIELDS) do
        local name, kind = fd[1], fd[2]
        local ok, v = pcall(function() return sp[name] end)
        if not ok then return nil end
        if kind == "class" then
            local full
            if valid(v) then pcall(function() full = v:GetFullName() end) end
            local path = full and full:match("^%S+%s+(.+)$") or ""
            local pkg, leaf = path:match("^(.+)%.([^%./]+)_C$")
            if pkg and pkg:sub(-(#leaf + 1)) == "/" .. leaf then path = pkg end
            out[fd[3]] = path:gsub("^/Game/Assets/", "@")
        elseif kind == "vec" then
            out[fd[3]] = { v.X, v.Y, v.Z }
        elseif kind == "color" then
            out[fd[3]] = { v.R, v.G, v.B, v.A }
        elseif kind == "name" then
            out[fd[3]] = v:ToString()
        else out[fd[3]] = tonumber(v) or 0 end
    end
    if out.class == "" then
        -- Level-placed/picked-up weapons can omit WeaponClass in their
        -- passport; their live actor still supplies the exact spawn class.
        out.class = (cls_path(actor) or ""):gsub("^/Game/Assets/", "@")
    end
    if out.class == "" then return nil end
    return out
end
T.encode_passport = W2.encode_passport

-- All-or-nothing copy of "Weapon Passport" into a deferred (not yet
-- constructed) weapon actor.
local function copy_passport(src, dst)
    local sp = field(src, "Weapon Passport")
    if sp == nil then return false end
    local t = {}
    for _, fd in ipairs(PASSPORT_FIELDS) do
        local name, kind = fd[1], fd[2]
        local ok = pcall(function()
            local v = sp[name]
            if kind == "vec" then t[name] = { X = v.X, Y = v.Y, Z = v.Z }
            elseif kind == "color" then t[name] = { R = v.R, G = v.G, B = v.B, A = v.A }
            elseif kind == "name" then t[name] = FName(v:ToString())
            elseif kind == "class" then if valid(v) then t[name] = v end
            else t[name] = v end
        end)
        if not ok then return false end
    end
    return (pcall(function() dst["Weapon Passport"] = t end))
end


-- Spawn transform: the newest streamed pose of the item if we have one
-- (no pop from the drop point), else the manifest's drop point.
local function spawn_transform(e)
    local row = W.unbound_rows[e.id]
    local q = e.q or { X = 0, Y = 0, Z = 0, W = 1 }
    local pos = e.pos
    if row then
        pos = row.pos
        q = { X = row.q[1], Y = row.q[2], Z = row.q[3], W = row.q[4] }
    end
    return { Translation = { X = pos.X, Y = pos.Y, Z = pos.Z }, Rotation = q, Scale3D = { X = 1, Y = 1, Z = 1 } }
end
T.spawn_transform = function(e) return spawn_transform(e) end

local function spawn_remote_item(e)
    local cls, why = resolve_weapon_class(e.class)
    if not cls then return nil, why end
    local world, gs
    pcall(function() world = UEHelpers.GetWorld() end)
    pcall(function() gs = UEHelpers.GetGameplayStatics() end)
    if not valid(world) or not valid(gs) then return nil, "no world" end
    local t = spawn_transform(e)
    local a
    local ok, err = pcall(function()
        a = gs:BeginDeferredActorSpawnFromClass(world, cls, t, SPAWN_ALWAYS, nil, SCALE_MULTIPLY)
    end)
    if not ok or not valid(a) then return nil, "BeginDeferredActorSpawnFromClass: " .. tostring(err) end
    -- Deferred: set what the construction script / BeginPlay reads.
    pcall(function() a["Simulates Physics"] = true end)
    local src = e.src or passport_source(e)
    local look = not e.passport and src and copy_passport(src, a) and "peer passport" or "class default look"
    -- A pickup-race replacement must be a copy of this exact actor. If
    -- reading its passport failed, keep the original for the next attempt
    -- rather than retiring it in favour of an unrelated class-default kit.
    if e.src and not e.passport and look ~= "peer passport" then
        retire_actor(a, WEAPON_BODY_KEYS)
        return nil, "source weapon passport not ready"
    end
    if look == "class default look" then
        local cached = e.passport or ((W.peer_passports[e.dyn] or {})[e.class])
        local decoded = cached and W2.record_passport(cached)
        if cached then
            if not decoded or not pcall(function() a["Weapon Passport"] = decoded end) then
                retire_actor(a, WEAPON_BODY_KEYS)
                return nil, "received weapon modules not ready"
            end
            look = "received peer passport"
        end
    end
    local ok2, err2 = pcall(function() gs:FinishSpawningActor(a, t, SCALE_MULTIPLY) end)
    if not ok2 then
        retire_actor(a, WEAPON_BODY_KEYS)
        return nil, "FinishSpawningActor: " .. tostring(err2)
    end
    if not valid(a) then return nil, "destroyed during construction" end
    return a, look
end
T.spawn_remote_item = function(e) return spawn_remote_item(e) end

-- Lost a pickup race: my pawn picked up a world weapon the server gave to another
-- player (both grabbed it within a round trip). The server's verdict holds on every
-- screen: this game lets go of its copy the way the kit strips a hand weapon (the actor
-- is retired) and shows the item with a fresh copy (same class, same passport) that
-- follows the winner's stream, like a peer's dropped item. Returns true when done.
function W2.lose_hold(o, owner, now)
    if o.kind ~= "weapon" or not o.actor or now < (o.lose_next or 0) then return false end
    o.lose_next = now + 2000
    local old = o.actor
    local p, r = body_pose(o.body)
    if not p or not pose_ok(p, W.bounds) then p = o.anchor_pos end
    local q = rot_to_quat_t(r or o.anchor_rot or { Pitch = 0, Yaw = 0, Roll = 0 })
    local copy, info = spawn_remote_item({ id = o.nid, class = cls_path(old), pos = p, q = q, src = old })
    if not copy then
        Log("lost the pickup race for %s to peer %d, but no copy could be made (%s); still holding it here",
            obj_label(o), owner, tostring(info))
        return false
    end
    local oad = o.actor_addr
    retire_actor(old, WEAPON_BODY_KEYS)   -- unsafe: ok a weapon actor (Willies are refused)
    if oad then W.by_addr[oad] = nil; W.my_items[oad] = nil end
    local body, sim = weapon_body(copy)
    o.actor, o.actor_addr, o.body, o.sim, o.body_at = copy, addr(copy), body, sim, now
    o.spawned_by_us, o.kin, o.settled, o.applied_pos, o.probed_at = true, false, false, nil, clock_ms()
    W2.track_copy(o)
    if o.actor_addr then W.by_addr[o.actor_addr] = o; W.known_actor[o.actor_addr] = true end
    W.stats.lost_races = (W.stats.lost_races or 0) + 1
    Log("lost the pickup race for %s to peer %d: let go here, showing their copy (%s)", obj_label(o), owner, info or "?")
    return true
end

-- Queue / retry with backoff: a failed spawn is retried on later ticks even
-- if the manifest record doesn't change again.
-- A dynamic entry I created under an earlier peer id of this
-- session (a reconnect / NAT rebind gave me a new id; the server keeps the
-- entry under the old one). It is MY weapon, still lying in my world: bind
-- that very actor to the entry (claims and leases then work as for any
-- world item), never spawn a copy of it.
T.adopt_mine = function(e)
    local rec = sess.mine[e.id]
    local wid = W.name .. "@" .. tostring(W.addr)
    -- (a record is at most 15 s old: never touch an actor not probed lately)
    local a = rec and rec.world == wid and clock_ms() - (rec.at or -1e9) < 15000 and rec.actor or nil
    if a and valid(a) and addr(a) == rec.addr and not W.by_addr[rec.addr] and (cls_short(a) or "?") == rec.cls then
        local body, sim = weapon_body(a)
        local p, r = nil, nil
        if body then p, r = body_pose(body) end
        local r0 = r or { Pitch = 0, Yaw = 0, Roll = 0 }
        local o = {
            lid = e.id, nid = e.id, chash = e.chash, cls = rec.cls, comp = "W", kind = "weapon",
            actor = a, actor_addr = rec.addr, body = body, sim0 = sim, sim = sim, probed_at = clock_ms(),
            spawn_pos = p or e.pos, spawn_rot = r0, anchor_pos = p or e.pos, anchor_rot = r0,
            pos = p or e.pos, rot = r0, vel = v3(0, 0, 0),
            dyn_owner = e.dyn, class_path = e.class, adopted = true,
            buf = {}, prio = 0, last_read = -999, last_probe = -999,
        }
        W.objs[#W.objs + 1] = o
        W.by_lid[e.id], W.by_nid[e.id], W.by_addr[rec.addr] = o, o, o
        W.known_actor[rec.addr] = true
        Log("my own dropped item %d (%s, listed under my old peer id %d): re-adopted, no copy spawned", e.id, e.class, e.dyn)
        return true
    end
    W.dyn_failed[e.id] = true   -- never spawned as a peer's copy
    Log("my own dropped item %d (%s, listed under my old peer id %d): not found here any more; no copy spawned",
        e.id, e.class, e.dyn)
    return false
end

local function bind_dyn(entries, now)
    for _, e in ipairs(entries) do
        if not W.by_nid[e.id] and e.dyn ~= sess.my_id and sess.my_nids[e.id] == W.level and not W.dyn_failed[e.id] then
            W.dyn_queue[e.id] = nil
            T.adopt_mine(e)
        elseif not W.by_nid[e.id] and e.dyn ~= sess.my_id and not W.dyn_queue[e.id] and not W.dyn_failed[e.id] then
            W.dyn_queue[e.id] = { e = e, tries = 0, next = now }
        end
    end
end

local dyn_log_budget = 20
local function service_dyn(now)
    local spawned = 0
    for id, q in pairs(W.dyn_queue) do
        if W.by_nid[id] then
            W.dyn_queue[id] = nil
        elseif spawned < MAX_DYN_SPAWN_TICK and now >= q.next then
            spawned = spawned + 1
            q.tries = q.tries + 1
            local e = q.e
            local a, info = spawn_remote_item(e)
            if a then
                W.dyn_queue[id] = nil
                local body, sim = weapon_body(a)
                local p, r = nil, nil
                if body then p, r = body_pose(body) end
                local r0 = r or { Pitch = 0, Yaw = 0, Roll = 0 }
                local o = {
                    lid = e.id, nid = e.id, chash = e.chash, cls = cls_short(a) or "?", comp = "W", kind = "weapon",
                    actor = a, actor_addr = addr(a), body = body, sim0 = sim, sim = sim, probed_at = clock_ms(), body_at = now,
                    spawn_pos = p or e.pos, spawn_rot = r0, anchor_pos = p or e.pos, anchor_rot = r0,
                    pos = p or e.pos, rot = r0, vel = v3(0, 0, 0),
                    dyn_owner = e.dyn, class_path = e.class, spawned_by_us = true,
                    buf = {}, prio = 0, last_read = -999, last_probe = -999,
                }
                W2.track_copy(o)
                W.objs[#W.objs + 1] = o
                W.by_lid[e.id] = o
                W.by_nid[e.id] = o
                if o.actor_addr then W.by_addr[o.actor_addr] = o; W.known_actor[o.actor_addr] = true end
                W.stats.dyn_spawned = (W.stats.dyn_spawned or 0) + 1
                Log("peer %d's dropped item %d (%s): spawned local copy (%s)", e.dyn, e.id, e.class, info or "?")
            else
                if q.tries >= DYN_SPAWN_TRIES then
                    W.dyn_queue[id] = nil
                    W.dyn_failed[id] = true
                end
                if dyn_log_budget > 0 then
                    dyn_log_budget = dyn_log_budget - 1
                    Log("peer %d's dropped item %d (%s): spawn failed (try %d/%d): %s", e.dyn, e.id, e.class,
                        q.tries, DYN_SPAWN_TRIES, tostring(info))
                end
                q.next = now + DYN_RETRY_MS * (1 << (q.tries - 1))
            end
        end
    end
end

-- --- manifest / owners / held records -------------------------------------------------------
-- On the 8 MP arenas the server builds the manifest from
-- docs/arena_static and arrives with the sync reply; our entries are only
-- aliases. So a scene body is proposed only if it is still unbound once the
-- server's manifest has been read (or 2 s after sync on levels without one):
-- the server folds it into a canonical id (alias) or, for bodies it can't
-- know statically (multi-body pieces, chest loot), accepts it when it lies
-- near a static object of the arena. Dropped items are always proposed.
local MOUT_WAIT_MS = 2000
local function write_manifest_out(now)
    if not W.synced or not W.mout_dirty then return end
    local static_ok = W.manifest_seen or (now - W.synced_at) > MOUT_WAIT_MS
    if not static_ok then return end
    W.mout_dirty = false
    -- The world_manifest_out record (unbound scene bodies and actor states) and the
    -- world_dyn_out record (my dropped items with their class path; the server keeps at
    -- most 32 per peer). Written on change only (mout_dirty); the sidecar proposes each
    -- entry until the server answers it.
    local rows, drows = {}, {}
    local rnd = W2.rnd
    for _, o in ipairs(W.objs) do
        if o.dyn_owner == 0 and not o.nid and not o.dead then
            rows[#rows + 1] = { id = o.lid, chash = o.chash, pos = { rnd(o.spawn_pos.X, 10), rnd(o.spawn_pos.Y, 10), rnd(o.spawn_pos.Z, 10) } }
        elseif o.dyn_owner ~= 0 and o.dyn_owner == sess.my_id and o.class_path and #drows < 32 then
            drows[#drows + 1] = { id = o.lid, chash = o.chash, dyn_owner = o.dyn_owner, class_path = o.class_path,
                                  has_passport = o.passport ~= nil, passport = o.passport,
                                  pos = { rnd(o.spawn_pos.X, 10), rnd(o.spawn_pos.Y, 10), rnd(o.spawn_pos.Z, 10) } }
        end
    end
    for _, a in ipairs(W.actors) do
        if not a.nid and not a.dead then
            rows[#rows + 1] = { id = a.lid, chash = a.chash, pos = { rnd(a.spawn_pos.X, 10), rnd(a.spawn_pos.Y, 10), rnd(a.spawn_pos.Z, 10) } }
        end
    end
    local ipc = W2.ipc()
    if ipc then
        ipc.put("world_manifest_out", { level = W.level, epoch = W.epoch or 0, rows = rows })
        ipc.put("world_dyn_out", { level = W.level, epoch = W.epoch or 0, rows = drows })
    end
end
-- Round reset without an arena reload (the reload, when it comes, drops W
-- through the world guard before this runs: see RESTORE_DELAY_MS).
--   * local copies of peers' dropped items are retired (hidden, inert, then
--     destroyed) -- unless a Willie holds it right now: destroying a held
--     weapon under the holder's BP is not safe, so it is left in the game and
--     simply forgotten (if it's in MY hand it becomes my item again);
--   * scene bodies go back to their spawn pose: kinematic first, teleport
--     only if they actually moved (constrained neighbours aren't kicked),
--     then their original simulation flag, at rest;
--   * nothing is touched that is held, attached or already destroyed.
-- Hand the world back to the game when the session ends while this
-- arena stays loaded (W is about to be dropped). Unlike restore_all nothing
-- is teleported: bodies we followed (kinematic, o.kin) get their original
-- simulation back where they are, and local copies of peers' dropped items
-- (spawned_by_us, not held) are retired so they are not proposed twice when
-- a session resumes. Only for the CURRENT world (caller checks name + addr).
-- Plain identities only: W is discarded on session loss. Resolve each retry
-- from the current world's live actor inventory; never retain old UObjects.
local release_retries = {}
W2.frozen_identities = {}
function W2.world_scope(name, key)
    if type(key) == "string" and key:find("@", 1, true) then return key:match("^(.-)#") or key end
    return tostring(name) .. "@" .. tostring(key)
end
function W2.native_scope(object)
    local ok, scope = pcall(function()
        local world = object:GetWorld()
        if valid(world) then return world:GetFullName() .. "@" .. tostring(world:GetAddress()) end
    end)
    return ok and scope or nil
end
function W2.ownership_identity(o, retire)
    if not W then return end
    local aa, ba = addr(o.actor), addr(o.body)
    local an, bn = fname_of(o.actor), fname_of(o.body)
    if not aa or not an or not o.cls or (not retire and (not ba or not bn)) then return end
    local body_path
    pcall(function() body_path = o.body:GetFullName():match("^%S+ (.+)$") end)
    local key = aa .. ":" .. (retire and "copy" or ba)
    return key, { name = an, body_name = bn, body_path = body_path, actor_addr = aa, body_addr = ba,
        cls = o.cls, kind = o.kind, sim0 = o.sim0, world_name = W.name, world_key = W.addr,
        scope = W2.world_scope(W.name, W.addr), lineage = W.lineage, retire = retire,
        next_ms = 0, tries = 0 }
end
function W2.track_copy(o)
    local key, r = W2.ownership_identity(o, true)
    if not key then return false end
    W2.frozen_identities[key] = W2.frozen_identities[key] or r
    return true
end
function W2.note_freeze(o)
    if not W then return true end -- fresh recovery after the old W was discarded
    if o.spawned_by_us then return W2.track_copy(o) end
    if o.sim0 ~= true then return true end
    local key, r = W2.ownership_identity(o, false)
    if not key then return false end
    W2.frozen_identities[key] = W2.frozen_identities[key] or r
    return true
end
function W2.clear_freeze(o)
    if o.spawned_by_us then return end
    local aa, ba = addr(o.actor), addr(o.body)
    if aa and ba then W2.frozen_identities[aa .. ":" .. ba] = nil end
end
function W2.queue_copy_cleanup(o, held)
    local key = o.actor_addr and (o.actor_addr .. ":copy")
    if not key then return end
    if held then release_retries[key], W2.frozen_identities[key] = nil, nil
    elseif W2.frozen_identities[key] then release_retries[key] = W2.frozen_identities[key] end
end
function W2.recovery_pending(wname, wkey)
    local scope = W2.world_scope(wname, wkey)
    for _, r in pairs(release_retries) do
        if r.recovering and r.scope == scope then return true end
    end
    return false
end
local function queue_release(o)
    local key, r = W2.ownership_identity(o, false)
    if key then release_retries[key] = r end
end
function W2.service_release_retries(wname, wkey, now)
    if WG.travel_from ~= nil then release_retries, W2.frozen_identities = {}, {}; return end
    local budget = 4
    for key, r in pairs(release_retries) do
        if r.world_name ~= wname or (r.recovering and r.scope ~= W2.world_scope(wname, wkey))
            or (not r.recovering and r.world_key ~= wkey) then
            release_retries[key] = nil
            W2.frozen_identities[key] = nil
        elseif budget > 0 and now >= r.next_ms and WG.settled() then
            budget = budget - 1
            r.tries = r.tries + 1
            -- Retain ownership of a failed restoration while this world lives.
            -- Back off after the initial burst instead of abandoning a frozen
            -- body. Only plain identities survive between these callbacks.
            r.next_ms = now + (r.tries >= 12 and 5000 or 250)
            local actor, body
            local ok, list = pcall(FindAllOf, r.cls)
            if ok then
                for _, a in pairs(list or {}) do
                    if valid(a) and addr(a) == r.actor_addr and fname_of(a) == r.name then actor = a; break end
                end
            end
            if actor then
                if W2.native_scope(actor) ~= W2.world_scope(wname, wkey) then actor = nil end
            elseif ok and type(list) == "table" then
                -- Successful fresh enumeration proves the original actor absent.
                release_retries[key], W2.frozen_identities[key] = nil, nil
            end
            if actor and r.retire then
                body = weapon_body(actor)
                if natively_held({ actor = actor, body = body, kind = r.kind }) then
                    release_retries[key], W2.frozen_identities[key] = nil, nil
                elseif retire_actor(actor, WEAPON_BODY_KEYS) then
                    -- Keep the plain identity until a fresh lookup confirms
                    -- retirement; a destroy request alone cannot release the
                    -- recovery barrier or change the validated world scope.
                end
            elseif actor then
                local body_absent = false
                if r.body_path then
                    local found, b = pcall(StaticFindObject, r.body_path)
                    if found then
                        if valid(b) and addr(b) == r.body_addr and fname_of(b) == r.body_name then body = b
                        else body_absent = true end
                    end
                else
                    if r.kind == "weapon" then
                        local b = weapon_body(actor)
                        if valid(b) and addr(b) == r.body_addr and fname_of(b) == r.body_name then body = b end
                    end
                    local cc; pcall(function() cc = StaticFindObject("/Script/Engine.PrimitiveComponent") end)
                    if not body and valid(cc) then
                        local enumerated = pcall(function()
                            local actors = { actor }
                            if r.kind == "weapon" then
                                local children = {}
                                actor:GetAttachedActors(children, true, true)
                                for _, child in pairs(children) do
                                    if not valid(child) then child = child:get() end
                                    if valid(child) and not is_willie_class(cls_short(child)) then actors[#actors + 1] = child end
                                end
                            end
                            for _, a in ipairs(actors) do
                                K.NA.each(a:K2_GetComponentsByClass(cc),function(b)
                                    if valid(b) and addr(b) == r.body_addr and fname_of(b) == r.body_name then body = b end
                                end)
                            end
                        end)
                        body_absent = enumerated and body == nil
                    end
                end
                if body_absent then release_retries[key], W2.frozen_identities[key] = nil, nil end
                if body and W2.native_scope(body) ~= W2.world_scope(wname, wkey) then body = nil end
                if body and addr(body) == r.body_addr and fname_of(body) == r.body_name then
                    local reclaimed = false
                    if W and W.name == wname and W.addr == wkey and W.lineage ~= r.lineage then
                        for _, current in ipairs(W.objs) do
                            if current.actor_addr == r.actor_addr and probed_recently(current)
                                and valid(current.body) and addr(current.body) == r.body_addr then
                                current.sim0 = r.sim0
                                reclaimed = true; break
                            end
                        end
                    end
                    local o = { actor = actor, body = body, kind = r.kind }
                    if reclaimed then release_retries[key] = nil
                    elseif natively_held(o) then release_retries[key], W2.frozen_identities[key] = nil, nil
                    elseif set_sim(o, r.sim0) then release_retries[key], W2.frozen_identities[key] = nil, nil end
                end
            end
        end
    end
end
T.release_retries = function() return release_retries end

local function release_world(why)
    read_hands()
    local phys, retired, kept, failed = 0, 0, 0, 0
    for _, o in ipairs(W.objs) do
        local released = true
        if o.spawned_by_us then
            if not probed_recently(o) then
                -- not probed lately: may be freed; forget it untouched
                W2.queue_copy_cleanup(o, false)
            elseif alive(o) and (my_hold_mode(o) or natively_held(o)) then
                kept = kept + 1
                W2.queue_copy_cleanup(o, true)
            else
                if alive(o) and retire_actor(o.actor, WEAPON_BODY_KEYS) then retired = retired + 1 end
                W2.queue_copy_cleanup(o, false)
            end
            o.dead, o.actor, o.body = true, nil, nil
        elseif (o.kin or o.pin_kin or o.pin_pending) and probed_recently(o) and alive(o) and valid(o.body) and my_hold_mode(o) == nil then
            -- (a pinned prop goes back to the scene's own physics too)
            if set_sim(o, o.sim0) then phys = phys + 1
            else failed = failed + 1; released = false; queue_release(o) end
        end
        if released then
            o.kin, o.pin_kin, o.pinned_at, o.pin_pending = false, nil, nil, nil
        end
    end
    Log("%s: released the replicated world (%d followed bodies back to physics, %d copies retired, %d held, %d retrying)",
        why, phys, retired, kept, failed)
    return failed == 0
end
T.release_world = function(why) return release_world(why) end

local function restore_body(o)
    o.restore_pending = true
    if o.sim and not set_sim(o, false) then return false end
    if not teleport(o, o.spawn_pos, o.spawn_rot, ZERO) then return false end
    if not set_sim(o, o.sim0) then return false end
    if o.sim0 and not teleport(o, o.spawn_pos, o.spawn_rot, ZERO, true) then return false end
    o.restore_pending = nil
    return true
end
T.restore_body = restore_body
local function restore_all(why)
    read_hands()
    local n, retired, kept, failed = 0, 0, 0, 0
    for _, o in ipairs(W.objs) do
        if o.spawned_by_us then
            if not probed_recently(o) then
                -- not probed lately: may be freed; forget it untouched
                W2.queue_copy_cleanup(o, false)
            elseif alive(o) and (my_hold_mode(o) or natively_held(o)) then
                kept = kept + 1
                W2.queue_copy_cleanup(o, true)
            else
                if alive(o) and retire_actor(o.actor, WEAPON_BODY_KEYS) then retired = retired + 1 end
                W2.queue_copy_cleanup(o, false)
            end
            o.dead, o.actor, o.body = true, nil, nil
        elseif o.dyn_owner == 0 and probed_recently(o) and alive(o) and valid(o.body) and my_hold_mode(o) == nil and can_drive(o) then
            local moved = (o.pos and dist3(o.pos, o.spawn_pos) > 0.5) or (o.rot and rot_diff(o.rot, o.spawn_rot) > 0.5)
            if moved or o.kin or o.pin_kin or o.pin_pending or o.restore_pending or o.sim ~= o.sim0 then
                if restore_body(o) then n = n + 1 else failed = failed + 1 end
            end
        end
    end
    -- No state is relinquished while a reset transition still needs retry.
    if failed > 0 then return false end
    for _, o in ipairs(W.objs) do
        o.kin, o.pin_kin, o.pinned_at = false, nil, nil   -- unpinned (sim0 restored above)
        o.pin_pending, o.restore_pending = nil, nil
        o.nid, o.buf, o.buf_sender, o.consumed, o.settled = nil, {}, nil, nil, false
        o.anchor_pos, o.anchor_rot, o.anchor_key, o.anchor_new = o.spawn_pos, o.spawn_rot, nil, false
        o.was_mine, o.awake, o.asleep_repeat, o.applied_pos = false, false, 0, nil
    end
    -- Drop dynamic records (mine stay in the game; they'll re-register if dropped again).
    local keep = {}
    for _, o in ipairs(W.objs) do if o.dyn_owner == 0 and not o.dead then keep[#keep + 1] = o end end
    W.objs = keep
    W.by_lid, W.by_nid, W.by_addr = {}, {}, {}
    for _, o in ipairs(W.objs) do
        W.by_lid[o.lid] = o
        if o.kind == "weapon" and o.actor_addr then W.by_addr[o.actor_addr] = o end
        o.forced, o.force_exact, o.init_tries, o.init_next, o.still_from, o.h_pos = nil, nil, nil, nil, nil, nil
        o.init_sample = nil
    end
    -- Actor states: broken constraints can't be mended without a reload;
    -- the records are re-bound and re-synced like bodies.
    for _, r in ipairs(W.actors) do
        W.by_lid[r.lid] = r
        r.nid, r.srv_bits, r.srv_ver, r.sent_bits, r.sent_at, r.sends = nil, nil, nil, nil, -1e9, 0
        r.apply_ver, r.apply_tries, r.flag_dirty = nil, 0, false
    end
    W.ready, W.init_t0, W.unbound_ids, W.hash_next = false, nil, {}, 0
    W.t0 = clock_ms()   -- Ready timeout and startup quiet restart with the round
    W.owners, W.pending, W.fclock, W.fseq = {}, {}, {}, {}
    W.dyn_queue, W.dyn_failed, W.unbound_rows = {}, {}, {}
    W.last_manifest, W.last_mout, W.last_remote, W.last_owners = nil, nil, nil, nil
    W.manifest_seen = false
    W.mout_dirty = true
    Log("%s: restored %d bodies to their spawn pose; retired %d spawned copies (%d held, left alone)",
        why, n, retired, kept)
    return true
end
T.restore_all = function(why) return restore_all(why) end

local function read_owners(now)
    local ipc = W2.ipc()   -- the world_owners record
    local t, gen = nil, nil
    if ipc then t, gen = ipc.rec("world_owners") end
    if not t or gen == W.last_owners then return end
    W.last_owners = gen
    local r = W2.owners_from(t)
    if not r or r.level ~= W.level then return end
    if W.synced and r.epoch ~= W.epoch then
        -- The countdown that bumped the epoch usually reloads the arena a
        -- moment later (the world guard then drops all of W untouched). Stop
        -- replicating now; restore only if the world is still up after
        -- RESTORE_DELAY_MS, so we never teleport/destroy bodies of a world
        -- that is about to be torn down.
        Log("world epoch %d -> %d (round reset); restoring in %d ms unless the arena reloads",
            W.epoch or 0, r.epoch, RESTORE_DELAY_MS)
        W.restore_due = now + RESTORE_DELAY_MS
        W.synced, W.epoch, W.sync_tries, W.sync_next = false, nil, 0, 0
        W.owners, W.pending = {}, {}
        W.ready, W.init_t0 = false, nil
        return
    end
    if not W.synced then
        if W.restore_due then return end
        if r.sync and r.epoch ~= 0 then
            W.synced, W.epoch = true, r.epoch
            W.synced_at = now
            local n = 0; for _ in pairs(r.o) do n = n + 1 end
            Log("world sync complete: epoch=%d owners=%d manifest=%d", r.epoch, n, r.mlen)
            W.mout_dirty = true
        else
            return
        end
    end
    W.mlen = r.mlen
    -- Owner records arrive newest-version-first filtered by the sidecar.
    for id, rec in pairs(r.o) do
        local old = W.owners[id]
        if not old or old.ver ~= rec.ver then
            local o = W.by_nid[id]
            -- a new owner: only its samples from now on count (older ones are another lease)
            if o and (not old or old.owner ~= rec.owner) then o.lease_at = now end
            W.owners[id] = rec
            if o then o.settled = false end
        end
    end
end

local function read_manifest(now)
    local ipc = W2.ipc()   -- the world_manifest + world_dyn records
    local t, gen, td, gd = nil, nil, nil, nil
    if ipc then t, gen = ipc.rec("world_manifest"); td, gd = ipc.rec("world_dyn") end
    if not t or (gen == W.last_manifest and gd == W.last_dyn) then return end
    local m = W2.manifest_from(t, td)
    if not m or m.level ~= W.level or m.epoch ~= W.epoch then return end
    W.last_manifest, W.last_dyn = gen, gd
    if #m.e > 0 and not W.manifest_seen then W.manifest_seen = true; W.mout_dirty = true end
    -- Bodies and actor-state records bind the same way (state ids have their
    -- own class|A<g> hash, so they never bind to a body or vice versa).
    local all = W.objs
    if #W.actors > 0 then
        all = {}
        for _, o in ipairs(W.objs) do all[#all + 1] = o end
        for _, a in ipairs(W.actors) do all[#all + 1] = a end
    end
    local c = bind_manifest(m.e, all, W.by_lid, W.by_nid)
    W.unbound_ids = c.unmatched_ids
    if c.exact + c.alias + c.mismatch > 0 then W.mout_dirty = true end   -- fewer bodies to propose
    if #c.dyn > 0 then bind_dyn(c.dyn, now) end
    local bound = 0
    for _, o in ipairs(W.objs) do if o.nid then bound = bound + 1 end end
    W.unmatched = c.unmatched
    if c.exact + c.alias + c.mismatch > 0 or c.unmatched ~= (W.last_unmatched or -1) then
        W.last_unmatched = c.unmatched
        Log("manifest %d entries: bound +%d exact +%d alias +%d class-mismatch; %d/%d local bodies bound; %d remote entries unmatched here",
            #m.e, c.exact, c.alias, c.mismatch, bound, #W.objs, c.unmatched)
    end
    if c.mismatch > 0 then
        for _, o in ipairs(W.objs) do
            if o.bind_how == "mismatch" and not o.mismatch_logged then
                o.mismatch_logged = true
                Log("WARNING class mismatch: local %s bound to remote id %d (different weapon here; transform still synced)", o.cls, o.nid)
            end
        end
    end
end

-- The world_held bus key: one row per remote peer's hand holding a world item
-- ({peer, nid, hand 0 = R / 1 = L, actor = the world actor's FName here}).
function W2.held_rows()
    local rows = {}
    for _, o in ipairs(W.objs) do
        local r = rec_of(o)
        if r and r.owner ~= 0 and r.owner ~= sess.my_id and r.mode >= MODE_HOLD_R and r.mode < K.MODE_STATE
           and not o.dead and o.actor then
            rows[#rows + 1] = { peer = r.owner, nid = o.nid, hand = r.mode == MODE_HOLD_L and 1 or 0,
                                actor = fname_of(o.actor) or "" }
        end
    end
    table.sort(rows, function(a, b) if a.peer ~= b.peer then return a.peer < b.peer end return a.hand < b.hand end)
    return rows
end
function W2.same_held(a, b)
    if not a or #a ~= #b then return false end
    for i, x in ipairs(a) do
        local y = b[i]
        if x.peer ~= y.peer or x.nid ~= y.nid or x.hand ~= y.hand or x.actor ~= y.actor then return false end
    end
    return true
end
function W2.put_held(rows)
    local ipc = W2.ipc()
    if ipc then ipc.bus_put("world_held", { rows = rows }) end
end

local function write_held()
    local rows = W2.held_rows()
    if not W2.same_held(W.last_held, rows) then
        W.last_held = rows
        W2.put_held(rows)
    end
end

-- --- actor states (server-ordered events) ------------------------------------------------
-- A state actor's reference (and its constraint components) is probed every
-- K.STATE_EVERY ticks; a destroyed one is dropped for good (never touched
-- again, like bodies in alive()).
function W2.actor_alive(r)
    if r.dead then return false end
    if not valid(r.actor) then
        r.dead, r.actor = true, nil
        for _, c in ipairs(r.cons) do c[2] = nil end
        return false
    end
    return true
end

-- The kind's flag (group 0 only): lever Loaded, trap armed, chest lid open.
function W2.read_flag(r)
    if r.group ~= 0 then return false end
    local a = r.actor
    if r.skind == "lever" then return field(a, "Loaded") == true end
    if r.skind == "trap" then
        local w = field(a, "As Modular Weapon BP")
        return valid(w) and field(w, "Simulates Physics") == true
    end
    if r.skind == "chest" then
        -- BP_Container_Chest_*: open = |roll of Lid relative to Box| > 75.
        local lid, box = field(a, "Lid"), field(a, "Box")
        if not valid(lid) or not valid(box) then return false end
        local _, lr = body_pose(lid)
        local _, br = body_pose(box)
        if not lr or not br then return false end
        local ql, qb = rot_to_quat_t(lr), rot_to_quat_t(br)
        local rel = W2.quat_mul(W2.quat_conj({ qb.X, qb.Y, qb.Z, qb.W }), { ql.X, ql.Y, ql.Z, ql.W })
        return math.abs(quat_to_rot(rel[1], rel[2], rel[3], rel[4]).Roll) > 75
    end
    return false
end

-- Local bits: constraint k broken (a destroyed constraint counts as broken
-- and its reference is dropped), plus the flag.
function W2.read_bits(r)
    local broken = {}
    for k, c in ipairs(r.cons) do
        local b = c.broken == true
        if not b then
            if c[2] ~= nil and valid(c[2]) then
                local ok, v = pcall(function() return c[2]:IsBroken() end)
                b = ok and v == true
            else
                b = true
            end
            if b then c.broken, c[2] = true, nil end
        end
        broken[k] = b
    end
    return W2.group_bits(broken, W2.read_flag(r))
end

-- Make the local actor match the server's record: break the constraints it
-- broke elsewhere (the gate's own "Activate Event" for a lever-gated block,
-- so its collision change runs too) and fire a trap the server armed.
function W2.apply_state(r, brk, arm)
    if WG.travel_from ~= nil or not W2.actor_alive(r) then return end
    local a = r.actor
    local activated = false
    for _, k in ipairs(brk) do
        local c = r.cons[k]
        if c and r.skind == "gate" and c[1] == "PhysicsConstraint1" then
            if not activated then
                activated = true
                pcall(function() a["Activate Event"](a) end)
            end
        elseif c and c[2] ~= nil and valid(c[2]) then
            pcall(function() c[2]:BreakConstraint() end)
        end
    end
    if arm and r.skind == "trap" then pcall(function() a["Event Activate Trap"](a) end) end
end

-- An actor-state report: a CLAIM_STATE claim whose rest pose carries the bits in vel[1].
function W2.send_state(r, bits)
    local rest = W2.qobj(r.nid, r.spawn_pos, { Pitch = 0, Yaw = 0, Roll = 0 }, nil, 0)
    rest.vel = { bits, 0, 0 }
    W2.claim(r.nid, K.CLAIM_STATE, rest)
end

W2.state_log_budget = 40
function W2.service_states(now)
    for _, r in ipairs(W.actors) do
        if r.nid and W2.actor_alive(r) then
            local loc = W2.read_bits(r)
            if loc ~= r.loc_bits then
                if r.loc_bits ~= nil and (loc & K.ST_FLAG) ~= (r.loc_bits & K.ST_FLAG) then r.flag_dirty = true end
                r.loc_bits, r.changed_at = loc, now
            end
            local rec = W.owners[r.nid]
            local srv = (rec and rec.mode >= K.MODE_STATE) and (rec.mode & 0x7F) or 0
            r.srv_bits, r.srv_ver = srv, rec and rec.ver or 0
            -- Server -> local: a few tries per record version.
            local brk, flag = W2.state_todo(srv, loc)
            local arm = flag and r.skind == "trap"
            if #brk > 0 or arm then
                if r.apply_ver ~= r.srv_ver then r.apply_ver, r.apply_tries = r.srv_ver, 0 end
                if r.apply_tries < K.STATE_APPLY_TRIES then
                    r.apply_tries = r.apply_tries + 1
                    W2.apply_state(r, brk, arm)
                    if W2.state_log_budget > 0 then
                        W2.state_log_budget = W2.state_log_budget - 1
                        Log("state %s/%s#%d (%s): server bits %d, local %d -> applying (try %d)", r.cls, r.comp, r.nid,
                            r.skind, srv, loc, r.apply_tries)
                    end
                end
            end
            -- Local -> server: new local breaks, and a flag that changed here
            -- (edge-triggered, so a stale flag never overwrites the record).
            if r.flag_dirty and (loc & K.ST_FLAG) == (srv & K.ST_FLAG) then r.flag_dirty = false end
            local bits = (loc & K.ST_BREAK_MASK) | (r.flag_dirty and (loc & K.ST_FLAG) or (srv & K.ST_FLAG))
            local need = (bits & ~srv & K.ST_BREAK_MASK) ~= 0 or (bits & K.ST_FLAG) ~= (srv & K.ST_FLAG)
            if not need then
                r.sends = 0
            elseif (bits ~= r.sent_bits or now - r.sent_at > 1500) and (r.sends or 0) < 6 then
                W2.send_state(r, bits)
                r.sent_bits, r.sent_at, r.sends = bits, now, (r.sends or 0) + 1
                W.stats.state_sent = (W.stats.state_sent or 0) + 1
                if W2.state_log_budget > 0 then
                    W2.state_log_budget = W2.state_log_budget - 1
                    Log("state %s/%s#%d (%s): local %d, server %d -> reporting %d", r.cls, r.comp, r.nid, r.skind, loc, srv, bits)
                end
            end
        end
    end
end
T.service_states = function(now) return W2.service_states(now) end

-- --- initial state (one client's settled world, forced everywhere) ----------------------
-- After the K.INIT_AFTER_SCAN pass every client proposes the rest pose of each
-- unanchored free body once it has been still for K.INIT_SETTLE_MS (or at the
-- deadline). The server accepts only the first proposer's (the init
-- authority) and fans those anchors out; every client, the authority too,
-- then forces them exactly (free_update, o.force_exact) before Ready.
K.INIT_SAMPLE_MS = 150
-- true = still since the last sample, false = moved, nil = no verdict yet
-- (the sample is younger than INIT_SAMPLE_MS).
function W2.init_still(o, now)
    local s = o.init_sample
    if not s then
        o.init_sample = { t = now, pos = o.pos, rot = o.rot }
        return nil
    end
    if now - s.t < K.INIT_SAMPLE_MS then return nil end
    local still = dist3(o.pos, s.pos) < 0.3 and rot_diff(o.rot, s.rot) < 0.3
        and vlen(o.vel or ZERO) < SETTLE_SPEED
    o.init_sample = { t = now, pos = o.pos, rot = o.rot }
    return still
end
T.init_still = function(o, now) return W2.init_still(o, now) end

function W2.service_init(now)
    if not W.synced or W.scans_done < K.INIT_AFTER_SCAN then return end
    if not W.init_t0 then
        W.init_t0 = now
        Log("initial state: proposing settled rest poses (the first proposer's world becomes everyone's)")
    end
    local deadline = now - W.init_t0 >= K.INIT_PROPOSE_DEADLINE_MS
    local n = 0
    for _, o in ipairs(W.objs) do
        if n >= 40 then break end
        if o.nid and o.dyn_owner == 0 and not o.anchor_key and not o.dead and (o.init_tries or 0) < K.INIT_TRIES
           and now >= (o.init_next or 0) and owner_of(o) == 0 and not W.pending[o.nid] and not o.kin then
            if read_body(o, now) and pose_ok(o.pos, W.bounds) then
                -- read_body just set prev_pos = pos in this same frame, so
                -- "pos vs prev_pos" would always read still. Compare against an
                -- own sample at least INIT_SAMPLE_MS old, and require a low speed.
                local still = W2.init_still(o, now)
                if still then o.still_from = o.still_from or now elseif still == false then o.still_from = nil end
                if deadline or (o.still_from and now - o.still_from >= K.INIT_SETTLE_MS) then
                    n = n + 1
                    W2.claim(o.nid, K.CLAIM_INIT, W2.qobj(o.nid, o.pos, o.rot, ZERO, WF_ASLEEP | (o.sim and WF_SIM or 0)))
                    o.init_tries = (o.init_tries or 0) + 1
                    o.init_next = now + K.INIT_RESEND_MS
                    W.stats.init_sent = (W.stats.init_sent or 0) + 1
                end
            end
        end
    end
end
T.service_init = function(now) return W2.service_init(now) end

-- Ready = synced, past the K.INIT_AFTER_SCAN pass, every free static body
-- anchored and forced (or waited K.INIT_WAIT_MS for), every server actor state
-- applied. K.READY_TIMEOUT_MS after the level load it is ready anyway, with
-- complete=false and the reason.
function W2.compute_ready(now)
    local reason
    local c = { bodies = 0, bound = 0, anchored = 0, forced = 0, unanchored = 0, owned = 0,
                states = 0, states_pending = 0 }
    for _, o in ipairs(W.objs) do
        if o.dyn_owner == 0 and not o.dead then
            c.bodies = c.bodies + 1
            if o.nid then
                c.bound = c.bound + 1
                if o.anchor_key then c.anchored = c.anchored + 1 end
                if o.forced then c.forced = c.forced + 1 end
            end
        end
    end
    if not W.synced then
        reason = "sync"
    elseif W.scans_done < K.INIT_AFTER_SCAN or not W.init_t0 then
        reason = "scan"
    else
        local waited = now - W.init_t0
        local pend = 0
        for _, o in ipairs(W.objs) do
            if o.nid and o.dyn_owner == 0 and not o.dead then
                if owner_of(o) ~= 0 then
                    c.owned = c.owned + 1          -- follows its owner's stream
                elseif not o.anchor_key then
                    if waited < K.INIT_WAIT_MS then pend = pend + 1 else c.unanchored = c.unanchored + 1 end
                elseif o.anchor_new then
                    pend = pend + 1                -- anchored, not forced yet
                end
            end
        end
        for _, r in ipairs(W.actors) do
            if r.nid and not r.dead then
                c.states = c.states + 1
                local rec = W.owners[r.nid]
                local srv = (rec and rec.mode >= K.MODE_STATE) and (rec.mode & 0x7F) or 0
                local brk, flag = W2.state_todo(srv, r.loc_bits or 0)
                if (#brk > 0 or (flag and r.skind == "trap")) and (r.apply_tries or 0) < K.STATE_APPLY_TRIES then
                    c.states_pending = c.states_pending + 1
                end
            end
        end
        pend = pend + c.states_pending
        if pend > 0 then reason = "init" end
    end
    local timed_out = reason ~= nil and now - W.t0 > K.READY_TIMEOUT_MS
    local ready = reason == nil or timed_out
    local complete = reason == nil and c.unanchored == 0
    if ready and not W.ready then
        W.ready = true
        Log("world ready%s: %d/%d bodies bound, %d anchored, %d forced, %d unanchored, %d owned; %d actor states (%s)",
            complete and "" or " (INCOMPLETE)", c.bound, c.bodies, c.anchored, c.forced, c.unanchored, c.owned,
            c.states, timed_out and ("timeout while " .. reason) or "ok")
    end
    W.ready_reason = timed_out and "timeout" or (reason or "ok")
    -- (No ready record is published here: the Director emits its own world_ready event.)
    return ready, complete, c
end
T.compute_ready = function(now) return W2.compute_ready(now) end

-- --- consistency report / verdict ----------------------------------------------------------
function W2.r1(v) return math.floor(v * 10 + 0.5) / 10 end

-- Every K.HASH_EVERY_MS once ready: each static body (and actor state) as
-- [id, record ver, status, x, y, z, q]. Settled = free, not followed, and
-- unmoved since the previous report. The client hash covers the settled and
-- missing rows (server/src/world.rs world_hash, bit-identical).
function W2.hash_report(now)
    if not W.ready or not W.synced or now < W.hash_next then return end
    W.hash_next = now + K.HASH_EVERY_MS
    local rows, seen = {}, {}
    for _, o in ipairs(W.objs) do
        if o.nid and o.dyn_owner == 0 and not seen[o.nid] then
            seen[o.nid] = true
            local rec = W.owners[o.nid]
            local row = { id = o.nid, ver = rec and rec.ver or 0, status = 0, pos = { X = 0, Y = 0, Z = 0 }, q = 0 }
            if not o.dead and alive(o) and read_body(o, now) then
                row.status = K.HS_ALIVE
                row.pos = { X = W2.r1(o.pos.X), Y = W2.r1(o.pos.Y), Z = W2.r1(o.pos.Z) }
                local qt = rot_to_quat_t(o.rot)
                row.q = W2.pack_quat({ qt.X, qt.Y, qt.Z, qt.W })
                local free = owner_of(o) == 0 and not W.pending[o.nid] and not o.kin
                local still = o.h_pos ~= nil and dist3(o.pos, o.h_pos) < 0.5 and rot_diff(o.rot, o.h_rot) < 0.5
                    and vlen(o.vel or ZERO) < SETTLE_SPEED
                o.h_pos, o.h_rot = o.pos, o.rot
                if free and still then row.status = K.HS_ALIVE | K.HS_SETTLED end
            end
            rows[#rows + 1] = row
        end
    end
    for _, id in ipairs(W.unbound_ids or {}) do
        if not seen[id] then
            seen[id] = true
            local rec = W.owners[id]
            rows[#rows + 1] = { id = id, ver = rec and rec.ver or 0, status = 0, pos = { X = 0, Y = 0, Z = 0 }, q = 0 }
        end
    end
    for _, r in ipairs(W.actors) do
        if r.nid and not seen[r.nid] then
            seen[r.nid] = true
            local row = { id = r.nid, ver = r.srv_ver or 0, status = K.HS_STATE, pos = { X = 0, Y = 0, Z = 0 }, q = 0 }
            if W2.actor_alive(r) and r.loc_bits ~= nil then
                row.status = K.HS_STATE | K.HS_ALIVE
                row.q = r.loc_bits
                if r.loc_bits == (r.srv_bits or 0) or now - (r.changed_at or 0) > K.STATE_SETTLE_MS then
                    row.status = row.status | K.HS_SETTLED
                end
            end
            rows[#rows + 1] = row
        end
    end
    table.sort(rows, function(a, b) return a.id < b.id end)
    -- The world_hash record: one HashRow {id, ver, pos, q, status} per body / actor state.
    local hrows, b = {}, {}
    local rnd = W2.rnd
    for _, r in ipairs(rows) do
        local st = r.status & (K.HS_ALIVE | K.HS_SETTLED)
        if st == (K.HS_ALIVE | K.HS_SETTLED) or st & K.HS_ALIVE == 0 then hrows[#hrows + 1] = r end
        b[#b + 1] = { id = r.id, ver = r.ver, status = r.status, q = r.q,
                      pos = { rnd(r.pos.X, 10), rnd(r.pos.Y, 10), rnd(r.pos.Z, 10) } }
    end
    W.hash_seq = W.hash_seq + 1
    local ipc = W2.ipc()
    if ipc then
        ipc.put("world_hash", { level = W.level, epoch = W.epoch or 0, seq = W.hash_seq, hash = W2.world_hash(hrows), rows = b })
    end
end
T.hash_report = function(now) return W2.hash_report(now) end

K.MM_NAMES = { "pose", "presence", "state" }

-- Server verdict -> world_consistency event; mismatched free bodies are
-- forced back onto their anchors, mismatched actor states re-applied.
function W2.read_consistency(now)
    local ipc = W2.ipc()   -- the world_consistency slot (the latest world_verdict)
    local t, gen = nil, nil
    if ipc then t, gen = ipc.rec("world_consistency") end
    if not t or gen == W.last_cons then return end
    W.last_cons = gen
    local v = W2.consistency_from(t)
    if not v or v.level ~= W.level or v.epoch ~= W.epoch then return end
    local ids = {}
    for _, m in ipairs(v.rows) do ids[#ids + 1] = m.id end
    ev("world_consistency", { hash_match = v.hash_match, mismatched = ids, mismatched_n = v.mismatched_n,
        compared = v.compared, hash_equal = v.hash_equal, peer = v.peer, seq = v.seq,
        level = W.level, epoch = W.epoch or 0, world = WG.short and WG.short() or W.name })
    if v.hash_match then return end
    W.stats.mismatch = (W.stats.mismatch or 0) + 1
    if W.cons_log_budget > 0 then
        W.cons_log_budget = W.cons_log_budget - 1
        local d = {}
        for i, m in ipairs(v.rows) do
            if i > 8 then break end
            local o = W.by_nid[m.id]
            d[#d + 1] = string.format("%s %s %.0fcm/%.0fdeg", o and obj_label(o) or ("#" .. m.id),
                K.MM_NAMES[m.kind] or "?", m.dpos or 0, m.dang or 0)
        end
        Log("WORLD MISMATCH vs peer %d: %d of %d compared bodies differ: %s", v.peer, v.mismatched_n, v.compared,
            table.concat(d, "; "))
        -- why the pin (free_update) did not hold it: the body's own view
        for i, m in ipairs(v.rows) do
            if i > 4 then break end
            local o = W.by_nid[m.id]
            if o and o.kind ~= "state" then
                local ow, mode = owner_of(o)
                local off = (o.pos and o.anchor_pos) and dist3(o.pos, o.anchor_pos) or -1
                local ad = (o.rot and o.anchor_rot) and rot_diff(o.rot, o.anchor_rot) or -1
                Log("  #%d %s: anchor_key=%s off=%.1fcm/%.1fdeg spd=%.1f sim=%s kin=%s owner=%s/%s can_drive=%s pinned=%s near_pawn=%s",
                    m.id, obj_label(o), tostring(o.anchor_key), off, ad, o.vel and vlen(o.vel) or -1, tostring(o.sim),
                    tostring(o.kin), tostring(ow), tostring(mode), tostring(can_drive(o)), tostring(o.pinned_at ~= nil),
                    tostring(hands.pawn_loc ~= nil and o.pos ~= nil and dist3(hands.pawn_loc, o.pos) < NEAR_DIST))
            end
        end
    end
    for _, m in ipairs(v.rows) do
        local o = W.by_nid[m.id]
        if o and o.kind == "state" then
            o.apply_ver = nil
        elseif o and m.kind == 1 and o.anchor_key and owner_of(o) == 0 then
            o.force_exact, o.anchor_new = true, true
        end
    end
end
T.read_consistency = function(now) return W2.read_consistency(now) end

-- --- sync quality (WORLD-2, docs/development/testing.md) ------------------------------------
-- world_track (harness runs only: HSMP_AUTOTEST or HSMP_WORLD_TRACK): every 100 ms, the pose
-- of each body that is owned or moved lately, on the host clock (W2.track_clock), so the gate
-- can pair two instances' screens at the same moment; one rest sample when a body stops and a final one
-- 3 s later. world_sync_quality (always, every 5 s while something was followed): what the
-- follower had to correct.
K.TRACK_ON = (os.getenv("HSMP_AUTOTEST") or "") ~= "" or (os.getenv("HSMP_WORLD_TRACK") or "") ~= ""
K.TRACK_EVERY_MS, K.TRACK_MAX, K.QUALITY_EVERY_MS = 100, 8, 5000

-- The world_track timeline: the host's performance counter (HSMPNative now_us), the same
-- in every game process on one machine, so the gate can pair two instances to the ms. The
-- harness runs every instance on one machine; without the native module, the local clock.
function W2.track_clock(now)
    local N = rawget(_G, "HSMPNative")
    local f = type(N) == "table" and N.now_us
    if f then
        local ok, us = pcall(f)
        if ok and type(us) == "number" then return us / 1000 end
    end
    return now + (rawget(_G, "SIM_TRACK_OFF") or 0)   -- (the simulator's true time)
end

function W2.track_mode(o)
    local ow = owner_of(o)
    if ow == sess.my_id then return "own", ow end
    if o.kin then return "follow", ow end
    return "free", ow
end

function W2.track(now)
    if not K.TRACK_ON or not W.ready or now < (W.track_next or 0) then return end
    W.track_next = now + K.TRACK_EVERY_MS
    local srv = W2.track_clock(now)
    local n = 0
    for _, o in ipairs(W.objs) do
        if n >= K.TRACK_MAX then break end
        if o.nid and not o.dead and o.body then
            local active = owner_of(o) ~= 0 or o.kin
            -- (moving on its own here counts too: a released body sliding off its rest pose)
            if active or (o.tr_p and o.pos and dist3(o.pos, o.tr_p) > 0.5) then o.tr_until = now + 4000 end
            if active or (o.tr_until or 0) > now then
                local p, r = body_pose(o.body)
                if p and r then
                    local moved = not o.tr_p or dist3(p, o.tr_p) > 0.5 or rot_diff(r, o.tr_r) > 0.5
                    local emit, rest = moved, false
                    if moved then
                        o.tr_still = nil
                    elseif o.tr_moving then
                        emit, rest, o.tr_still = true, true, now   -- it just stopped
                    elseif o.tr_still and now - o.tr_still > 3000 then
                        emit, rest, o.tr_still = true, true, nil   -- final rest pose
                    end
                    o.tr_moving = moved
                    if emit then
                        n = n + 1
                        o.tr_p, o.tr_r = p, r
                        local q = rot_to_quat_t(r)
                        local mode, ow = W2.track_mode(o)
                        ev("world_track", { nid = o.nid, t = math.floor(srv), x = W2.r1(p.X), y = W2.r1(p.Y), z = W2.r1(p.Z),
                            qx = W2.rnd(q.X, 10000), qy = W2.rnd(q.Y, 10000), qz = W2.rnd(q.Z, 10000), qw = W2.rnd(q.W, 10000),
                            mode = mode, owner = ow, rest = rest, level = W.level, epoch = W.epoch or 0 })
                    end
                end
            end
        end
    end
end

function W2.quality(now)
    W.q = W.q or { t = now, follow = 0, max_off = 0 }
    local q = W.q
    for _, o in ipairs(W.objs) do
        if o.kin and o.fw then
            q.follow = q.follow + 1
            q.max_off = math.max(q.max_off, o.fw.max_off or 0)
            o.fw.max_off = 0
        end
    end
    if now - q.t < K.QUALITY_EVERY_MS then return end
    local st = W.stats
    if q.follow > 0 or (st.lost_races or 0) > 0 or (st.poked or 0) > 0 then
        ev("world_sync_quality", { hard_snaps = st.hard_snaps or 0, max_off_cm = math.floor(q.max_off + 0.5),
            lost_races = st.lost_races or 0, takeovers = st.takeovers or 0, poked = st.poked or 0,
            follow_ticks = q.follow, window_s = math.floor((now - q.t) / 100 + 0.5) / 10,
            level = W.level, epoch = W.epoch or 0 })
    end
    st.hard_snaps, st.lost_races, st.takeovers, st.poked = 0, 0, 0, 0
    W.q = { t = now, follow = 0, max_off = 0 }
end

-- Harness hook (HSMP_AUTOTEST): `hsmp-tools ipc-ctl --pid <game> autotest world_poke
-- "<cm/s>[,<n>]"` throws the n free bodies nearest to my pawn up and away (a mod-side
-- impulse, as if my pawn kicked them: a touch claim first). HSMPMenu ignores the command.
function W2.autotest(now)
    if not K.TRACK_ON or not W.ready then return end
    local ipc = W2.ipc()
    if not ipc or not ipc.dev_poll then return end
    W.dev_out = W.dev_out or {}
    local m = ipc.dev_poll(8, W.dev_out)
    for i = 1, m or 0 do
        local d = W.dev_out[i] and (W.dev_out[i].data or W.dev_out[i])
        if type(d) == "table" and d.op == 1 and d.key == "world_poke" then
            local sp, cnt = tostring(d.arg or ""):match("^(%d+),?(%d*)$")
            W2.poke(tonumber(sp) or 400, tonumber(cnt) or 1, now)
        end
    end
end

function W2.poke(speed, count, now)
    if not hands.pawn_loc then Log("world_poke: no pawn"); return 0 end
    local c = {}
    for _, o in ipairs(W.objs) do
        if o.nid and not o.dead and o.dyn_owner == 0 and owner_of(o) == 0 and not W.pending[o.nid] and o.pos
           and not o.pin_kin and not o.kin and dist3(o.pos, hands.pawn_loc) < 3000 then
            c[#c + 1] = { o, dist3(o.pos, hands.pawn_loc) }
        end
    end
    table.sort(c, function(a, b) return a[2] < b[2] end)
    local n = 0
    for k = 1, math.min(count, #c) do
        local o = c[k][1]
        if can_drive(o) and is_sim(o.body) then
            local dx, dy = o.pos.X - hands.pawn_loc.X, o.pos.Y - hands.pawn_loc.Y
            local dl = math.max(1, math.sqrt(dx * dx + dy * dy))
            want_claim(o, MODE_TOUCH, string.format("autotest poke %d cm/s", speed))
            local v = v3(dx / dl * speed, dy / dl * speed, speed * 0.6)
            pcall(function() o.body:SetPhysicsLinearVelocity(v, false, NAME_NONE) end)
            n = n + 1
        end
    end
    W.stats.poked = (W.stats.poked or 0) + n
    Log("world_poke: %d body(ies) thrown at %d cm/s", n, speed)
    return n
end
T.poke = function(speed, count, now) return W2.poke(speed, count, now) end

-- --- main tick ------------------------------------------------------------------------------------
-- World changed / level change requested: the whole per-level state (every
-- body, actor, component and puppet reference lives in W) and the per-tick
-- hand refs are dropped without touching any of them.

wg_on_drop(function(why)
    if why == "world changed" or why == "no valid world" then
        -- Only plain records were captured before freezing/spawning. An
        -- ambiguous lookup loss must not read any of the discarded UObjects.
        for key, r in pairs(W2.frozen_identities) do release_retries[key] = r end
        for _, r in pairs(release_retries) do r.recovering, r.next_ms = true, 0 end
    else
        release_retries, W2.frozen_identities = {}, {}
    end
    if W then
        Log("leaving world state (%s)", tostring(why))
        W = nil
        W2.put_held({})
    end
    hands.R, hands.L, hands.gR, hands.gL, hands.Ra, hands.La = nil, nil, nil, nil, nil, nil
    hands.pawn, hands.pawn_loc = nil, nil
end)

-- Full world name + world-guard key (name + UWorld address + PC name: an
-- arena reload keeps the name and may reuse the address). Only valid after
-- wg_check() returned true this tick.
local function current_world()
    return WG.name, WG.key
end

-- The peer id changed (a new session on the same arena): hand the
-- world back like a session end (release_world: only this world's objects),
-- then drop W so the next tick builds a fresh one that re-syncs (G2S world_sync).
T.apply_session_change = function(wname, waddr)
    local ch = sess.id_changed
    if not ch then return false end
    sess.id_changed = nil
    if not W then return false end
    sess.mine = {}
    if W.name == wname and W.addr == waddr then
        local ok, err = pcall(release_world, string.format("new session (peer id %d -> %d)", ch.from, ch.to))
        if not ok then Log("release_world error: %s", tostring(err)) end
        -- My own dropped items stay in this world; keep them (only
        -- ones probed alive just now) for re-adoption against the server's
        -- entries, which still carry my old id.
        local wid = W.name .. "@" .. tostring(W.addr)
        for _, o in ipairs(W.objs) do
            if o.nid and sess.my_nids[o.nid] == W.level and not o.dead and o.actor and probed_recently(o) and alive(o) then
                sess.mine[o.nid] = { actor = o.actor, addr = o.actor_addr, cls = o.cls, world = wid, at = clock_ms() }
            end
        end
    end
    Log("session changed (peer id %d -> %d): replicated world dropped, re-syncing", ch.from, ch.to)
    W = nil
    W2.put_held({})
    return true
end

local function on_tick()
    local now = clock_ms()
    local dt = now - last_clock
    last_clock = now
    NAME_NONE = NAME_NONE or FName("None")

    refresh_session(now)
    -- One PlayerController lookup per tick, shared by the guard and read_hands
    local okpc, tick_pc = pcall(RD.pc)
    if not okpc then tick_pc = nil end
    if not wg_check(tick_pc) then return end
    local wname, waddr = current_world()
    T.apply_session_change(wname, waddr)
    W2.service_release_retries(wname, waddr, now)
    -- Recover the original native baseline before rescanning. Otherwise a
    -- frozen follower would be rediscovered with false as its original sim0.
    if W2.recovery_pending(wname, waddr) then return end
    if not sess.live or not wname or wname:find("Map_Menu_", 1, true) or not wname:find("/Game/Maps/", 1, true) then
        if W then
            -- The session ended while THIS world is still loaded: hand the
            -- bodies back to the game before forgetting them (physics on for
            -- the ones we followed kinematically; local copies of peers'
            -- dropped items retired). Another world: never touched.
            if W.name == wname and W.addr == waddr then
                local ok, err = pcall(release_world, "session ended")
                if not ok then Log("release_world error: %s", tostring(err)) end
            end
            Log("leaving replicated world"); W = nil
            W2.put_held({})
        end
        return
    end
    if not W or W.name ~= wname or W.addr ~= waddr then
        if W then Log("arena reloaded") end
        new_level(wname, waddr, now)
        W2.put_held({})
    end
    W.tick = W.tick + 1

    -- Round reset pending: nothing replicates until the restore (or the
    -- arena reload, which replaces W) has happened.
    if W.restore_due then
        if now < W.restore_due then return end
        if not restore_all("round reset") then W.restore_due = now + 250; return end
        W.restore_due = nil
    end

    -- (Re)sync until the sidecar reports the reply for this level (IPC only).
    if not W.synced and now >= W.sync_next then
        W.sync_tries = W.sync_tries + 1
        W.sync_next = now + SYNC_RETRY_MS
        local ipc = W2.ipc()
        if ipc then ipc.send("world_sync", { level = W.level }) end
        if W.sync_tries == 1 or W.sync_tries % 10 == 0 then
            Log("requesting world sync (try %d)", W.sync_tries)
        end
    end

    -- The shared world-settle gate (shared/hsmp_wg.lua). Nothing below walks
    -- FindAllOf or reads an actor's name in the first WG.SETTLE_S of a world:
    -- the old world is still being purged incrementally (walking it then
    -- crashed the game 0.5-0.6 s after a round reload).
    if not WG.settled() then return end
    W.scan_t0 = W.scan_t0 or now   -- the discovery schedule starts at the settle

    -- Discovery: a few passes while the level settles, sliced across ticks.
    if W.scan then
        scan_step()
    elseif W.scans_done < #SCAN_AT_MS and now - W.scan_t0 >= SCAN_AT_MS[W.scans_done + 1] then
        start_scan()
    end
    if W.tick % 2 == 0 then read_owners(now) end
    if not W.synced then
        if W.tick % K.STATE_EVERY == 0 then W2.compute_ready(now) end
        return
    end
    if W.tick % 8 == 0 then read_manifest(now) end
    write_manifest_out(now)
    service_pending(now)

    read_hands(tick_pc)
    refresh_puppets(now)
    W2.cache_peer_passports()
    track_my_items(now)
    ingest_remote(now)
    service_dyn(now)
    W.dt = dt

    local send_tick = (W.tick % SEND_EVERY) == 0
    W.fast_tick = (W.tick % 2) == 0
    W.mine_awake = {}
    W.kin_last, W.kin_now = W.kin_now, {}
    for _, o in ipairs(W.objs) do
        local ok, err = pcall(update_body, o, now, send_tick)
        if not ok and not o.err_logged then
            o.err_logged = true
            Log("update error on %s: %s", obj_label(o), tostring(err))
        end
    end
    if send_tick or W.fast_tick then send_states(now, send_tick) end
    if W.tick % 15 == 0 then write_held() end

    -- Identical worlds (initial state, actor states, Ready, consistency).
    if W.tick % 8 == 0 then W2.service_init(now) end
    if W.tick % K.STATE_EVERY == 0 then
        W2.service_states(now)
        W2.compute_ready(now)
    end
    if W.tick % 30 == 0 then W2.read_consistency(now) end
    W2.hash_report(now)
    W2.track(now)
    W2.quality(now)
    W2.autotest(now)

    if now - W.stats.t > 10000 then
        local owned, following, bound = 0, 0, 0
        for _, o in ipairs(W.objs) do
            if o.nid then
                bound = bound + 1
                local ow = owner_of(o)
                if ow == sess.my_id then owned = owned + 1 elseif ow ~= 0 then following = following + 1 end
            end
        end
        local st = W.stats
        Log("stats 10s: epoch=%d bodies=%d bound=%d (manifest %d) owned=%d following=%d | sent %d states, recv %d samples, %d anchors, %d follow-moves, %d claims, %d snap-backs",
            W.epoch or 0, #W.objs, bound, W.mlen, owned, following, st.sent, st.recv, st.snaps, st.follow,
            st.claims, st.snapback or 0)
        Log("stats 10s: ready=%s (%s) actors=%d | init proposals %d, forced %d, state reports %d, consistency mismatches %d",
            tostring(W.ready), W.ready_reason, #W.actors, st.init_sent or 0, st.forced or 0, st.state_sent or 0, st.mismatch or 0)
        W.stats = { sent = 0, recv = 0, claims = 0, snaps = 0, follow = 0, t = now }
        claim_log_budget = 40
    end
end

T.W = function() return W end
-- Test hooks (tools/hsmp-tools/lua-tests/hsmpworld.lua: mocked UE4SS, Rust + mlua).
T.new_level = new_level
T.sess, T.hands, T.WG = sess, hands, WG
T.bind_dyn, T.service_dyn = bind_dyn, service_dyn
T.update_body, T.track_my_items = update_body, track_my_items
T.set_name_none = function(v) NAME_NONE = v end
-- Identical-worlds test hooks (tools/hsmp-tools/lua-tests/world_state.lua).
T.ingest_remote, T.read_owners, T.read_manifest = ingest_remote, read_owners, read_manifest
T.start_scan, T.scan_step, T.read_hands = start_scan, scan_step, read_hands
T.on_tick = function() return on_tick() end
T.K, T.W2 = K, W2
if rawget(_G, "HSMP_WORLD_TEST") then return T end

LoopAsync(TICK_MS, function()
    if GAME_THREAD_LOOP then
        local ok, err = pcall(on_tick)
        if not ok then Log("tick error: %s", tostring(err)) end
    else
        ExecuteInGameThread(function()
            local ok, err = pcall(on_tick)
            if not ok then Log("tick error: %s", tostring(err)) end
        end)
    end
    return false
end)

Log("loaded (v2: manifest ids, leases, epochs); state_dir=%s", STATE_DIR)
