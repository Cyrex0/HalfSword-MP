-- HSMPRvpProbe (dev only): crash instrumentation for the Runtime Vertex
-- Paint plugin (VertexPaintDetectionPlugin, "RVP"): the game's blood / wound
-- painting. It never changes game state unless HSMP_RVP_STRESS is set.
--
--   * every 100 ms (game thread): the RVP task-queue sizes read from the
--     GameInstance subsystem (UVertexPaintDetectionGISubSystem.TaskQueue); a
--     line whenever they change (rate-limited) and a 2 s summary while busy;
--   * per-second call counts of the RVP task wrappers (UFunction pre-hooks);
--   * the queue sizes at every OpenLevel / LoadMap pre-hook (= what is in
--     flight when the old world is torn down);
--   * HSMP_RVP_STRESS=<n>: single-player crash amplifier. From the menu it
--     opens Map_Arena_Alley; in the arena, once settled, it queues RVP detect
--     tasks on every Willie mesh for a few frames and then reloads the arena,
--     n times. HSMP_RVP_GATE=1 routes that reload through shared/hsmp_rvp.lua
--     (the RVP travel guard) instead of a raw OpenLevel. The iteration count is
--     appended to <state>/.rvp_stress.txt before each reload (survives a crash).
--   * dev console: <state>/.rvp_cmd.lua runs once on the game thread.
--
-- WARNING: the stress runs in SINGLE-PLAYER, where the career save guard is
-- (correctly) inactive, and it wounds the player's own Willie: the game then
-- saves that state into the career slot GameProgress.sav, and later MP
-- sessions seed from it (every own-kit check then fails).
-- Copy the SaveGames folder aside first and put it back afterwards.
--
-- Not deployed by build-and-deploy.ps1 (it lives outside mods/ because its
-- file console and stress counter are against the state-file policy, G0
-- state_files / travel). To use it: copy this folder to ue4ss/Mods/, copy
-- mods/shared/hsmp_wg.lua and hsmp_rvp.lua into its Scripts/, add
-- "HSMPRvpProbe : 1" to ue4ss/Mods/mods.txt, and run with HSMP_DEV=1
-- (experimental/crash-rr/soak.ps1 / stress.ps1 do the launching; A/B:
-- -Gate 0 vs -Gate 1). Results: docs/development/crash-rr.md (reproduction).

local UEHelpers = require("UEHelpers")

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
end

local function Log(fmt, ...) print(string.format("[HSMPRvpProbe] " .. fmt .. "\n", ...)) end
local function trim(s) return s and s:match("^%s*(.-)%s*$") or nil end
local STATE_DIR = (trim(os.getenv("HSMP_STATE_DIR")) or "hsmp_state"):gsub("\\", "/")
local DEV = (trim(os.getenv("HSMP_DEV")) or "") == "1"
if not DEV then Log("off (HSMP_DEV is not 1)"); return end

local function load_module(name)
    local ok, mod = pcall(require, name)
    if ok and type(mod) == "table" then return mod end
    local src = ((debug and debug.getinfo and debug.getinfo(1, "S").source) or ""):gsub("^@", "")
    local dir = src:match("^(.*)[/\\]") or "."
    for _, path in ipairs({ dir .. "/" .. name .. ".lua", dir .. "/../../../shared/" .. name .. ".lua" }) do
        local ok2, mod2 = pcall(dofile, path)
        if ok2 and type(mod2) == "table" then return mod2 end
    end
    return nil
end
local HW = load_module("hsmp_wg")
if not HW then Log("FATAL: hsmp_wg.lua missing"); return end
local WG = HW.new({ log = Log, UEHelpers = UEHelpers })
pcall(function() RegisterLoadMapPreHook(function() pcall(WG.travel, "LoadMap") end) end)
local HR = load_module("hsmp_rvp")
local RVP = HR and HR.new({ log = Log }) or nil
Log("guard module: %s", RVP and ("hsmp_rvp v" .. tostring(HR.VERSION)) or "MISSING")

local function counts()
    if not RVP then return nil end
    return RVP.counts()
end
local function cstr(c)
    if not c then return "n/a" end
    return string.format("paintQ=%d detectQ=%d paintComps=%d detectComps=%d", c.paint, c.detect, c.paint_comps, c.detect_comps)
end

-- --- wrapper call counters -------------------------------------------------------
local LIB = "/Script/VertexPaintDetectionPlugin.VertexPaintTasksFunctionLibrary:"
local WRAPPERS = {
    "PaintOnMeshAtLocation_Wrapper", "PaintOnMeshWithinArea_Wrapper", "PaintOnEntireMesh_Wrapper",
    "PaintColorSnippetOnMesh_Wrappers", "PaintGroupSnippetOnMesh_Wrapper", "SetMeshComponentVertexColors_Wrapper",
    "SetMeshComponentVertexColorsUsingSerializedString_Wrapper", "GetColorsWithinArea_Wrapper",
    "GetClosestVertexDataOnMesh_Wrapper", "GetAllVertexColorsOnly_Wrapper",
}
local calls, calls_total = {}, 0
local hooked = 0
for _, w in ipairs(WRAPPERS) do
    local ok = pcall(function()
        RegisterHook(LIB .. w, function()   -- count only: nothing captured, nothing touched
            calls[w] = (calls[w] or 0) + 1
            calls_total = calls_total + 1
        end)
    end)
    if ok then hooked = hooked + 1 end
end
Log("wrapper hooks: %d/%d registered", hooked, #WRAPPERS)

-- Which actors own the components with queued tasks (pre-teardown only: the
-- world is still alive in the OpenLevel pre-hook).
local function queue_owners(prop)
    local owners = {}
    pcall(function()
        local ss = FindFirstOf("VertexPaintDetectionGISubSystem")
        ss.TaskQueue[prop]:ForEach(function(k)
            local o = "?"
            pcall(function()
                local c = k:get()
                o = c:GetOwner():GetClass():GetFName():ToString() .. "." .. c:GetClass():GetFName():ToString()
            end)
            owners[o] = (owners[o] or 0) + 1
        end)
    end)
    local parts = {}
    for k, v in pairs(owners) do parts[#parts + 1] = k .. "x" .. v end
    table.sort(parts)
    return table.concat(parts, " ")
end
local function travel_note(what)
    local c = counts()
    Log("%s: RVP at teardown: %s (wrapper calls so far %d)", what, cstr(c), calls_total)
    if c and what:find("OpenLevel", 1, true) then
        if c.detect_comps > 0 then Log("  detect owners: %s", queue_owners("ComponentDetectTaskIDs")) end
        if c.paint_comps > 0 then Log("  paint owners: %s", queue_owners("ComponentPaintTaskIDs")) end
    end
end
for _, fn in ipairs({ "/Script/Engine.GameplayStatics:OpenLevel", "/Script/Engine.GameplayStatics:OpenLevelBySoftObjectPtr" }) do
    pcall(function() RegisterHook(fn, function() pcall(travel_note, "OpenLevel pre") end) end)
end
pcall(function() RegisterLoadMapPreHook(function()
    pcall(travel_note, "LoadMap pre")
    if RVP then pcall(RVP.on_loadmap) end   -- as HSMPMatch does: open again before the new BeginPlay
end) end)

-- --- queue watcher ----------------------------------------------------------------
local last, last_line, last_sum = "", -1e9, -1e9
local per_s, per_s_at = {}, os.clock()
LoopAsync(100, function()
    local now = os.clock()
    local c = counts()
    local s = cstr(c)
    if s ~= last and now - last_line > 0.25 then
        Log("queue %s", s)
        last, last_line = s, now
    end
    if c and (c.paint + c.detect) > 0 and now - last_sum > 2 then
        last_sum = now
        Log("busy: %s", s)
        if c.detect_comps > 5 and WG.check() then Log("  detect owners: %s", queue_owners("ComponentDetectTaskIDs")) end
    end
    if now - per_s_at >= 1.0 then
        local parts = {}
        for k, v in pairs(calls) do parts[#parts + 1] = k:gsub("_Wrappers?$", "") .. "=" .. v end
        if #parts > 0 then Log("wrapper calls/%.1fs: %s", now - per_s_at, table.concat(parts, " ")) end
        calls = {}
        per_s_at = now
    end
    return false
end)

-- --- stress (crash amplifier) -----------------------------------------------------
local STRESS_N = tonumber(trim(os.getenv("HSMP_RVP_STRESS")) or "") or 0
local USE_GATE = (trim(os.getenv("HSMP_RVP_GATE")) or "") == "1"
local ARENA = trim(os.getenv("HSMP_RVP_ARENA")) or "Map_Arena_Alley"
local STRESS_FILE = STATE_DIR .. "/.rvp_stress.txt"
local function stress_done()
    local n = 0
    local f = io.open(STRESS_FILE, "rb")
    if f then for _ in f:lines() do n = n + 1 end; f:close() end
    return n
end
local function stress_mark(i, detail)
    local f = io.open(STRESS_FILE, "ab")
    if f then f:write(string.format("%d %d %s\n", i, os.time(), detail)); f:close() end
end

local function open_level(name)
    local ok = false
    pcall(function()
        local gs = UEHelpers.GetGameplayStatics()
        local w = WG.world()
        if gs and gs:IsValid() and w then gs:OpenLevel(w, FName(name), true, FString("")); ok = true end
    end)
    return ok
end

-- Make the game bleed: one native "Get Damage" hit per call on every Willie
-- that is not the player's (nor the pooled Willie_BP_C_0 template), on a
-- rotating bone. The game's own Blueprint then queues its blood paints
-- (PaintOnMeshAtLocation, ~100+/s while bleeding) -- exactly what a HitFx /
-- damage replay or a death does in MP. NEVER call the RVP wrappers from Lua
-- with a hand-built settings table: UE4SS marshals the unset FString / TArray
-- members of the 0x458-byte settings struct as garbage (memcpy access
-- violation in the game).
local BONES = { "head", "spine_03", "spine_02", "upperarm_l", "upperarm_r", "thigh_l", "thigh_r", "neck_01", "pelvis", "calf_l" }
local bone_i = 0
local function flood()
    local me
    pcall(function() local pc = WG.pc(); me = pc and pc.Pawn end)
    local myn = "?"
    pcall(function() if me and me:IsValid() then myn = me:GetFName():ToString() end end)
    local n, err = 0, nil
    for _, w in pairs(FindAllOf("Willie_BP_C") or {}) do
        local nm = "?"
        pcall(function() if w:IsValid() then nm = w:GetFName():ToString() end end)
        -- every Willie but the pooled template: the player's own pawn bleeding
        -- out at travel reproduces the crash too
        if nm ~= "?" and nm ~= "Willie_BP_C_0" then
            bone_i = bone_i % #BONES + 1
            local bone = BONES[bone_i]
            local ok, e = pcall(function()
                local mesh = w.Mesh
                local l = mesh:GetSocketLocation(FName(bone))
                local function V(x, y, z) return { X = x, Y = y, Z = z } end
                pcall(function() w["Last Complex Damage Impulse"] = 0 end)
                w["Get Damage"](w, V(0, 0, -300), V(0, 0, -800), V(l.X, l.Y, l.Z), V(0, 0, 1), FName(bone),
                    800, 1.0, true, mesh, 0, false, false, nil, false, nil, 10, false, 0, {})
            end)
            if ok then n = n + 1 else err = tostring(e) end
        end
    end
    return n, err
end

if STRESS_N > 0 then
    Log("STRESS: %d reloads of %s, guard %s; %d done before this boot", STRESS_N, ARENA, USE_GATE and "ON" or "OFF", stress_done())
    local st = { phase = "wait", t = os.clock(), frames = 0, queued = 0 }
    LoopAsync(16, function()
        local now = os.clock()
        if not WG.check() then st.phase, st.t = "wait", now; return false end
        local short = WG.short()
        local done = stress_done()
        if done >= STRESS_N then
            if st.phase ~= "finished" then st.phase = "finished"; Log("STRESS finished: %d reloads", done) end
            return false
        end
        if short ~= ARENA then
            if now - st.t > 8 then st.t = now; Log("STRESS: in %s -> open %s", tostring(short), ARENA); open_level(ARENA) end
            return false
        end
        if st.phase == "wait" then
            if RVP then RVP.reopen("stress: new world") end   -- no-op unless the guard closed it
            if WG.settled(6.0) then st.phase, st.frames, st.queued = "flood", 0, 0 end
            return false
        end
        if st.phase == "flood" then
            local n, err = flood()
            st.queued = st.queued + n
            st.frames = st.frames + 1
            if err and st.frames == 1 then Log("STRESS flood error: %s", err) end
            if st.frames >= 6 then
                -- travel 0 / 0.3 / 0.6 / 0.9 s after the last hit (bleeding at its peak and decaying)
                st.phase, st.until_t = "bleed", now + ((done % 4) * 0.3)
                Log("STRESS #%d: %d hits; %s", done + 1, st.queued, cstr(counts()))
            end
            return false
        end
        if st.phase == "bleed" then
            st.queued = st.queued + (flood())      -- keep the blood coming up to the travel frame
            if now < st.until_t then return false end
            st.phase = USE_GATE and "gate" or "go"
        end
        if st.phase == "gate" then
            st.queued = st.queued + (flood())   -- the corpse keeps bleeding during the hold
            if RVP and RVP.hold("stress reload") then return false end
            st.phase = "go"
        end
        if st.phase == "go" then
            st.queued = st.queued + (flood())   -- blood requested in the travel frame itself (guard on or off)
            stress_mark(done + 1, string.format("queued=%d guard=%s at_travel={%s}", st.queued, tostring(USE_GATE), cstr(counts())))
            st.phase, st.t = "wait", now
            if RVP and USE_GATE then RVP.travel_issued() end
            open_level(ARENA)
        end
        return false
    end)
end

-- --- dev console --------------------------------------------------------------------
local CMD_FILE, OUT_FILE = STATE_DIR .. "/.rvp_cmd.lua", STATE_DIR .. "/.rvp_out.txt"
LoopAsync(250, function()
    local f = io.open(CMD_FILE, "rb")
    if not f then return false end
    local code = f:read("a"); f:close()
    os.remove(CMD_FILE)
    local of = io.open(OUT_FILE, "ab")
    local function out(fmt, ...)
        local ok, s = pcall(string.format, tostring(fmt), ...)
        s = ok and s or tostring(fmt)
        if of then of:write(s, "\n") end
        Log("cmd: %s", s)
    end
    local env = setmetatable({ out = out, UEHelpers = UEHelpers, WG = WG, RVP = RVP, counts = counts, cstr = cstr,
        flood = flood, open_level = open_level }, { __index = _G })
    local fn, err = load(code, "rvp_cmd", "t", env)
    if not fn then out("compile error: %s", tostring(err))
    else
        local ok, e = pcall(fn)
        if not ok then out("runtime error: %s", tostring(e)) end
    end
    out("-- done")
    if of then of:close() end
    return false
end)

Log("loaded (dev): RVP queue watcher, %d wrapper hooks, console %s", hooked, CMD_FILE)
