-- hsmp_wg.lua -- the HSMP world guard (stale-UObject protection), shared by
-- every HSMP mod that keeps a UObject beyond one callback.
--
-- build-and-deploy.ps1 copies shared/*.lua into each mod's Scripts/
-- directory; never edit a per-mod copy. Each mod is its own Lua state, so
-- each gets its own guard.
--
-- Why: a level change frees every actor, component and world-outered widget
-- of the old world, and the round reset re-opens the SAME arena (OpenLevel).
-- Touching a freed UObject afterwards (a property write, a call, even
-- IsValid(), which reads the freed object) is an access violation pcall cannot
-- catch (seen as a crash in UStruct::FindProperty <- __newindex from a
-- delayed callback).
--
-- Rule for every UObject kept beyond one callback:
--   * it belongs to the world it was cached in (WG.key);
--   * WG.check() runs first in every loop/frame callback: if the world key
--     changed, a level change is pending, or no world is valid, every cache is
--     dropped WITHOUT touching the old objects (WG.on_drop handlers only reset
--     Lua references) and the callback skips this tick;
--   * delayed one-shots capture WG.token() and test WG.same(token) first;
--   * a hook callback that defers work never captures the hooked object: it
--     either does the work synchronously (post-hook) or re-finds the object.
-- Key = world full name + UWorld address + PlayerController FName: reloading
-- the same arena keeps the name and may reuse the address, but the new PC is a
-- fresh runtime spawn with a new name.
-- OpenLevel / OpenLevelBySoftObjectPtr pre-hooks drop the caches BEFORE the old
-- world is torn down and hold every tick until the world has actually swapped
-- (the travel runs on a later engine tick).
--
-- Usage:
--   local WG = require("hsmp_wg").new{ log = Log }   -- installs the hooks
--   local wg_check, wg_on_drop, wg_token, wg_same = WG.check, WG.on_drop, WG.token, WG.same
--   wg_on_drop(function(why) my_cache = nil end)      -- reset Lua refs only
--   LoopAsync(100, function() if not wg_check() then return false end ... end)
--   WG.key / WG.name / WG.travel_from / WG.drops are readable fields.
--   local pc = WG.pc()        -- the PlayerController, ONE lookup per game frame
--   if not wg_check(pc) then return end                -- (see WG.pc)
--
-- Lint: `check_wg` (tools/hsmp-tools/src/bin/check_wg) flags module-level
-- UObject caches that no on_drop handler resets.

local M = { VERSION = 2 }

M.TRAVEL_HOLD_S = 10

-- The world-settle rule. Nothing may walk FindAllOf over actors (Willies,
-- weapons, props) or read their names in the first SETTLE_S of a world: the
-- previous world is still being purged incrementally and the new player
-- Willie is mid-construction. GetFName / ToString on a dead object right after
-- FindAllOf("Willie_BP_C"), 0.5-0.6 s after a round reload, is an access
-- violation pcall cannot catch. Every mod that touches Willies or weapons gates on
-- WG.settled() (or a M.settle_tracker() for mods with their own world guard).
M.SETTLE_S = 2.0

-- A standalone settle tracker for mods that keep their own world identity
-- (HSMPAvatars, HSMPLoadout): note(key) every tick with the current world
-- key (nil = no world); settled(s) is true only s seconds after the key last
-- changed to a non-nil value.
function M.settle_tracker(clock)
    clock = clock or function() return os.clock() end
    local st = { key = nil, at = nil }
    function st.note(key)
        if key ~= st.key then st.key, st.at = key, clock() end
        return st
    end
    function st.reset() st.key, st.at = nil, nil end
    function st.age() return st.at and (clock() - st.at) or 0 end
    function st.settled(s)
        return st.key ~= nil and st.at ~= nil and clock() - st.at >= (s or M.SETTLE_S)
    end
    return st
end
M.TRAVEL_FUNCS = {
    "/Script/Engine.GameplayStatics:OpenLevel",
    "/Script/Engine.GameplayStatics:OpenLevelBySoftObjectPtr",
}

-- opts:
--   log        function(fmt, ...)   the mod's logger (default: print)
--   UEHelpers  module               default: require("UEHelpers")
--   hooks      boolean              install the OpenLevel pre-hooks (default true)
--   hold_s     number               travel hold (default M.TRAVEL_HOLD_S)
--   clock      function() -> s      default os.clock (tests)
--   frame      function() -> id     the game-frame id for WG.pc() (default:
--                                   the engine frame counter, KismetSystemLibrary
--                                   GetFrameCount; fallback the os.clock() ms)
function M.new(opts)
    opts = opts or {}
    local Log = opts.log or function(fmt, ...) print(string.format(fmt, ...) .. "\n") end
    local UEH = opts.UEHelpers
    if not UEH then
        local ok, m = pcall(require, "UEHelpers")
        UEH = ok and m or nil
    end
    local clock = opts.clock or os.clock
    -- The game-frame id is the engine's frame counter (GFrameCounter
    -- through KismetSystemLibrary::GetFrameCount on the class default object,
    -- which is never freed): every callback of one engine frame shares one
    -- PlayerController lookup, whichever millisecond it runs in, and two
    -- frames never share one. Fallback (no CDO / call failed): the os.clock()
    -- millisecond, in its own key space.
    local frame = opts.frame or function()
        local id
        pcall(function()
            local ksl = UEH and UEH.GetKismetSystemLibrary and UEH.GetKismetSystemLibrary()
            local n = ksl and ksl:GetFrameCount()
            if type(n) == "number" then id = n end
        end)
        if id ~= nil then return id end
        return "c" .. tostring(os.clock())   -- looked up per call (tests patch os.clock)
    end
    local hold_s = opts.hold_s or M.TRAVEL_HOLD_S

    local WG = { key = nil, name = nil, travel_from = nil, travel_at = 0, drops = 0, pc_lookups = 0 }
    local drop_fns = {}
    WG.drop_names = {}   -- names given to on_drop/cache (diagnostics)

    -- UEHelpers.GetPlayerController() walks the whole object array
    -- (FindAllOf), and mods need it several times per tick. WG.pc()
    -- does one lookup per game frame and hands the same object to every
    -- caller of that frame. The cache never outlives the frame, and it is
    -- also dropped (untouched) on every guard drop (world change, level change
    -- requested), so a PlayerController of an old world is never handed out.
    local pcc = { pc = nil, frame = nil, drops = -1 }
    function WG.pc()
        local f = frame()
        if pcc.frame ~= nil and pcc.frame == f and pcc.drops == WG.drops then return pcc.pc end
        local pc
        pcall(function() pc = UEH.GetPlayerController() end)
        WG.pc_lookups = WG.pc_lookups + 1
        pcc.pc, pcc.frame, pcc.drops = pc, f, WG.drops
        return pc
    end

    -- The current UWorld through this frame's WG.pc() (UEHelpers.GetWorld()
    -- is another full PlayerController walk); the UEHelpers fallback only
    -- when there is no PlayerController. nil when there is no valid world.
    function WG.world()
        local w
        pcall(function()
            local pc = WG.pc()
            if pc and pc:IsValid() then w = pc:GetWorld() end
            if not (w and w:IsValid()) then w = UEH.GetWorld() end
            if not (w and w:IsValid()) then w = nil end
        end)
        return w
    end

    -- Register a handler run on every drop. It must only reset Lua
    -- references (never touch the old objects).
    function WG.on_drop(fn, name)
        drop_fns[#drop_fns + 1] = fn
        WG.drop_names[#WG.drop_names + 1] = name or "?"
    end
    -- Named variant, preferred for new code: WG.cache("banner", function() banner = {} end)
    function WG.cache(name, reset_fn) WG.on_drop(reset_fn, name) end

    -- Fresh lookups only (this frame's WG.pc() / GetWorld); never touches an
    -- object cached beyond the current frame.
    function WG.world_key(pc)
        local key, name
        pcall(function()
            if not (pc and pc:IsValid()) then pc = WG.pc() end
            local w
            if pc and pc:IsValid() then w = pc:GetWorld() else pc = nil end
            if not (w and w:IsValid()) then pc = nil; w = UEH.GetWorld() end
            if w and w:IsValid() then
                name = w:GetFullName()
                key = name .. "@" .. tostring(w:GetAddress())
                if pc then key = key .. "#" .. pc:GetFName():ToString() end
            end
        end)
        return key, name
    end

    function WG.drop(why)
        WG.drops = WG.drops + 1
        pcc.pc, pcc.frame = nil, nil   -- never touched; WG.pc() looks up afresh
        Log("world guard: %s -> object caches dropped (#%d)", why, WG.drops)
        for _, fn in ipairs(drop_fns) do pcall(fn, why) end
    end

    -- true: the caches belong to the current world. false: skip this tick.
    function WG.check(pc)
        local key, name = WG.world_key(pc)
        if WG.travel_from then
            if key ~= nil and key == WG.travel_from and clock() - WG.travel_at < hold_s then
                return false   -- level change requested, old world still up: hold
            end
            WG.travel_from = nil
        end
        if key == nil then
            if WG.key ~= nil then WG.key, WG.name, WG.key_at = nil, nil, nil; WG.drop("no valid world") end
            return false
        end
        if key ~= WG.key then
            WG.key, WG.name = key, name
            WG.key_at = clock()
            WG.drop("world changed")
            return false
        end
        return true
    end

    -- True only when the current world key has been stable for s
    -- seconds (default M.SETTLE_S) and no level change is pending. Gate every
    -- FindAllOf over actors (and every name read on what it returns) on it.
    -- Only meaningful after WG.check() returned true this tick.
    function WG.settled(s)
        return WG.key ~= nil and WG.key_at ~= nil and WG.travel_from == nil
            and clock() - WG.key_at >= (s or M.SETTLE_S)
    end
    function WG.settle_age() return WG.key_at and (clock() - WG.key_at) or 0 end

    -- Called from the OpenLevel pre-hooks: drop now, hold until the world swaps.
    function WG.travel(what)
        WG.travel_from = WG.world_key() or WG.key
        WG.travel_at = clock()
        WG.key, WG.name, WG.key_at = nil, nil, nil
        WG.drop(what)
    end

    function WG.token() return { key = WG.key, drops = WG.drops } end
    function WG.same(t)
        return t ~= nil and t.key ~= nil and WG.travel_from == nil and WG.drops == t.drops
            and WG.world_key() == t.key
    end

    -- "World /Game/Maps/Arenas/Map_Arena_Alley.Map_Arena_Alley" -> "Map_Arena_Alley".
    -- Only meaningful after WG.check() returned true this tick.
    function WG.short()
        return WG.name and WG.name:match("([%w_]+)$") or nil
    end

    function WG.pending() return WG.travel_from ~= nil end

    function WG.install_hooks()
        if WG._hooked then return end
        WG._hooked = true
        for _, fn in ipairs(M.TRAVEL_FUNCS) do
            pcall(function()
                RegisterHook(fn, function() pcall(WG.travel, "level change requested") end)
            end)
        end
    end
    if opts.hooks ~= false then WG.install_hooks() end
    return WG
end

return M
