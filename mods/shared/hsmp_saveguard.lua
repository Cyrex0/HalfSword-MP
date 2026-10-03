-- hsmp_saveguard.lua -- career save isolation, layer 1.
--
-- build-and-deploy.ps1 copies shared/*.lua into every mod's Scripts/
-- directory; only HSMPMatch (the Director) installs it. Do not edit per-mod
-- copies.
--
-- What it does:
--   Half Sword saves ONLY through the native GameplayStatics UFunctions
--   (SaveGameToSlot / LoadGameFromSlot / DoesSaveGameExist / DeleteGameInSlot,
--   UserIndex always 0; no native save subsystem, no async saves). The career
--   lives in slot "GameProgress" and is rewritten by GI_Settings "Save Game",
--   which runs on every arena load (BP_LevelManager BeginPlay), on the local
--   pawn's down/death (Willie_BP -> "Override Player Character Equipment"),
--   on nemesis bookkeeping when any Willie dies, and in the Construct of the
--   native end-of-fight widgets (UI_DeathDoor / UI_Lose / UI_FadeToLife).
--   An MP session therefore writes the career unless these calls are diverted.
--
--   While an MP session is live this module pre-hooks those UFunctions and
--   rewrites the SlotName parameter (the same param:set technique HSMPMatch
--   uses for OpenLevel):
--     writes  SaveGameToSlot / DeleteGameInSlot / AsyncSaveGameToSlot
--             "<slot>" -> "HSMP_<inst>_<slot>"         (mode "redirect")
--             or the write is neutralised              (mode "block")
--     reads   LoadGameFromSlot / DoesSaveGameExist / AsyncLoadGameFromSlot
--             copy-on-write: redirected only once the HSMP_ slot exists (or
--             was written / deleted this session), so the first arena load
--             of a session still sees the career state, read-only.
--   Every rewrite emits `save_redirected{fn,slot,to_slot,...}` through
--   shared/hsmp_log.lua (frozen vocabulary). Every save call, active or
--   not, can also be logged as `x_save_call` (opts.spy, default on); that
--   costs nothing (saves are rare).
--
--   If a write cannot be rewritten (param:set failed and the read-back still
--   shows the career slot), SaveGameToSlot is neutralised by nulling its
--   SaveGameObject (the engine returns false and writes nothing) and the
--   event carries ok=false. Layer 2 (server/src/career_guard.rs, sidecar)
--   backs the files up and restores them regardless.
--
-- Session liveness ("an MP session exists"):
--   1. M.set_active(true|false|nil) from the Director overrides everything
--      (nil = back to automatic).
--   2. Automatic: the sidecar's typed `link` record (ABI 2, shared memory) exists,
--      its status is not a terminal one, AND the sidecar is provably alive: its
--      header heartbeat (IPC.refresh_info().sidecar_hb_age_s, the sidecar
--      attached) is at most LIVE_S old. A link left behind by a crashed sidecar
--      never activates the guard (that would divert the player's career saves into
--      HSMP_ slots): a dead sidecar does not beat.
--      Once active, it stays active for STICKY_S without fresh evidence
--      (reconnect windows), unless the link disappears or turns terminal.
--
-- Threading: install() / tick() / hooks run on the game thread only (hooks
-- are game-thread by construction; call tick() from a LoopInGameThread body).
-- Never call from LoopAsync / ExecuteWithDelay without the HSMP shim.
--
-- Usage (HSMPMatch main.lua):
--   local SG = require("hsmp_saveguard")
--   SG.install({ mod = "HSMPMatch", log = HL })   -- HL = require("hsmp_log"), optional
--   ... in the 250 ms game-thread loop:  SG.tick()
--   ... optionally, when the Director knows better:  SG.set_active(true/false/nil)
--   ... after apply_gi_arena(), before OpenLevel:    SG.seed_session_slot()

local M = {}

M.VERSION = 1
M.PREFIX = "HSMP_"

-- name -> { path, kind, slot param index (1-based, after the context arg),
--           object param index for writes that can be neutralised }
M.FUNCTIONS = {
    { name = "SaveGameToSlot",        kind = "write",  slot = 2, obj = 1,
      path = "/Script/Engine.GameplayStatics:SaveGameToSlot" },
    { name = "DeleteGameInSlot",      kind = "delete", slot = 1,
      path = "/Script/Engine.GameplayStatics:DeleteGameInSlot" },
    { name = "LoadGameFromSlot",      kind = "read",   slot = 1,
      path = "/Script/Engine.GameplayStatics:LoadGameFromSlot" },
    { name = "DoesSaveGameExist",     kind = "read",   slot = 1,
      path = "/Script/Engine.GameplayStatics:DoesSaveGameExist" },
    -- Not used by the current Half Sword build; hooked so a patch that starts
    -- using them is still covered.
    { name = "AsyncSaveGameToSlot",   kind = "write",  slot = 3, obj = 2,
      path = "/Script/Engine.AsyncActionHandleSaveGame:AsyncSaveGameToSlot" },
    { name = "AsyncLoadGameFromSlot", kind = "read",   slot = 2,
      path = "/Script/Engine.AsyncActionHandleSaveGame:AsyncLoadGameFromSlot" },
}

-- Every slot name the shipped BPs use. Used to wipe this instance's
-- HSMP_ slots when a session starts, so each session copies the career fresh.
M.KNOWN_SLOTS = { "GameProgress", "Settings", "Save_Crash", "SG Gauntlet Progress",
                  "SG Player Equipment", "PhotosData", "SG Forge Test" }

-- kicked / replaced / server_closed are written by the sidecar
-- (sidecar/handlers.rs, net.rs), which then keeps running.
local TERMINAL = { rejected = true, stopped = true, exited = true, disconnected = true,
                   closed = true, ended = true, kicked = true, replaced = true,
                   server_closed = true }
M.TERMINAL = TERMINAL

local function trim(s) return s and s:match("^%s*(.-)%s*$") or nil end

local cfg = {}
local st = {}

local function defaults()
    cfg = {
        mod = "?",
        inst = "0",
        state_dir = "hsmp_state",
        save_dir = nil,          -- SaveGames dir (for COW existence checks / purge)
        mode = "redirect",       -- "redirect" | "block"
        spy = true,              -- emit x_save_call for every call
        live_s = 15,
        sticky_s = 45,
        poll_s = 0.5,            -- min interval between link reads
        absent_s = 1.0,          -- the link must stay absent this long (>= 2 reads) to count as gone
        link = nil,              -- function() -> status name | nil, heartbeat age s | nil (default: HSMP_IPC)
        fresh_per_session = true,-- delete this instance's HSMP_ slots on activation
        reload_gi_on_exit = true,-- GI "Load Game" (career) on active -> inactive, via tick()
        passthrough = {},        -- { Settings = true } to let a slot through untouched
        log = nil,               -- hsmp_log-like module (event/emit)
        print = print,
        now = os.time,           -- wall seconds
        clock = os.clock,        -- monotonic-ish seconds
        register_hook = nil,     -- default: global RegisterHook
        get_gi = nil,            -- default: UEHelpers.GetGameInstance
    }
    st = {
        installed = false,
        hooks = {},              -- name -> true | error string
        forced = nil,            -- Director override
        active = false,
        reason = "init",
        last_read = -1e9,
        last_live = -1e9,        -- clock() of the last proof of life
        written = {},            -- mapped slot -> true (redirected write this session)
        deleted = {},            -- mapped slot -> true (redirected delete this session)
        pending_gi_reload = false,
        sessions = 0,
        stats = { calls = 0, redirected = 0, blocked = 0, failed = 0, passthrough = 0 },
        probe = nil,
    }
    M.stats = st.stats
end
defaults()

-- --- logging -------------------------------------------------------------------

local function plog(fmt, ...)
    local ok, line = pcall(string.format, "[hsmp_saveguard] " .. fmt, ...)
    if ok then pcall(cfg.print, line .. "\n") end
end

local function ev(name, fields)
    local L = cfg.log
    if L and type(L.event) == "function" then
        pcall(L.event, name, fields)
    else
        local parts = {}
        for k, v in pairs(fields or {}) do parts[#parts + 1] = tostring(k) .. "=" .. tostring(v) end
        table.sort(parts)
        plog("%s %s", name, table.concat(parts, " "))
    end
end

-- --- slot mapping (pure) -------------------------------------------------------

function M.mapped(slot)
    return M.PREFIX .. cfg.inst .. "_" .. slot
end

function M.is_hsmp_slot(slot)
    return type(slot) == "string" and slot:sub(1, #M.PREFIX) == M.PREFIX
end

local function slot_file(slot)
    if not cfg.save_dir then return nil end
    return cfg.save_dir .. "/" .. slot .. ".sav"
end

local function file_exists(path)
    if not path then return false end
    local f = io.open(path, "rb")
    if f then f:close(); return true end
    return false
end

-- Copy-on-write read target: the HSMP_ slot once it exists / was touched.
local function read_target(slot)
    local m = M.mapped(slot)
    if st.written[m] or st.deleted[m] or file_exists(slot_file(m)) then return m end
    return nil
end

-- --- session liveness ------------------------------------------------------------

-- The sidecar's link (shared memory, typed): its status name and the header
-- heartbeat age (nil while no sidecar is attached / beating).
local function ipc_link()
    local I = rawget(_G, "HSMP_IPC")
    if not I or type(I.rec) ~= "function" then return nil end
    local l = I.rec("link")
    if type(l) ~= "table" then return nil end
    local names = I.S and I.S.ENUM_NAMES and I.S.ENUM_NAMES.sidecar_status
    local status = names and names[l.status]
    status = status and status:lower() or "unknown"
    local info = I.refresh_info and I.refresh_info(false)
    local age
    if type(info) == "table" and (info.sidecar_state == nil or info.sidecar_state == "ready" or info.sidecar_state == 2) then
        age = tonumber(info.sidecar_hb_age_s)
    end
    return status, age
end

local function set_state(active, why)
    if active == st.active then st.reason = why; return end
    st.active, st.reason = active, why
    if active then
        st.sessions = st.sessions + 1
        st.written, st.deleted = {}, {}
        if cfg.fresh_per_session and cfg.save_dir then
            for _, s in ipairs(M.KNOWN_SLOTS) do pcall(os.remove, slot_file(M.mapped(s))) end
        end
        plog("ACTIVE (%s): career slots now redirected to %s%s_<slot> [mode=%s]",
            why, M.PREFIX, cfg.inst, cfg.mode)
    else
        plog("inactive (%s): save calls pass through to the career slots", why)
        if cfg.reload_gi_on_exit then st.pending_gi_reload = true end
    end
    ev("x_save_guard", { active = active, why = why, session = st.sessions })
    st.state_emitted = true
end

-- Re-evaluate liveness (rate-limited unless force). Cheap: one record read (cached per version).
function M.refresh(force)
    local c = cfg.clock()
    if not force and c - st.last_read < cfg.poll_s then return st.active end
    st.last_read = c
    if st.forced ~= nil then
        -- A forced-live guard keeps its "last seen live" fresh, so one
        -- torn read right after the Director releases it (a match longer than
        -- sticky_s) is not taken for a stale sidecar.
        if st.forced == true then st.last_live = c end
        set_state(st.forced, "director " .. tostring(st.forced))
        return st.active
    end
    local ok, status, age = pcall(cfg.link or ipc_link)
    if not ok then status, age = nil, nil end
    if status == nil then
        -- No link record (no sidecar, or IPC unavailable). Require it to stay
        -- absent for absent_s (>= 2 reads) before turning the guard off.
        if not st.active then set_state(false, "no sidecar link"); return st.active end
        st.absent_n = (st.absent_n or 0) + 1
        st.absent_since = st.absent_since or c
        if st.absent_n >= 2 and (c - st.absent_since) >= cfg.absent_s then
            set_state(false, "no sidecar link")
        else
            st.reason = "sidecar link absent (waiting)"
        end
        return st.active
    end
    st.absent_n, st.absent_since = 0, nil
    if TERMINAL[status] then
        set_state(false, "sidecar status " .. status)
        return st.active
    end
    -- proof of life: the sidecar's header heartbeat
    local live = age ~= nil and age <= cfg.live_s
    if live then
        st.last_live = c
        set_state(true, "sidecar live (" .. status .. ")")
    elseif st.active and (c - st.last_live) <= cfg.sticky_s then
        st.reason = "sticky (no fresh sidecar evidence)"
    else
        set_state(false, st.active and "sidecar went stale" or "sidecar not live")
    end
    return st.active
end

function M.is_active() return M.refresh(false) end
function M.reason() return st.reason end
function M.set_active(v)
    if v ~= nil then v = v and true or false end
    st.forced = v
    M.refresh(true)
end

-- --- param access (UE4SS RemoteUnrealParam) ---------------------------------------

local function param_string(p)
    local ok, s = pcall(function()
        local v = p:get()
        if type(v) == "string" then return v end
        return v:ToString()
    end)
    if ok and type(s) == "string" then return s end
    return nil
end

-- Rewrite an FString param; returns true when the read-back shows `new`.
local function set_string(p, new)
    pcall(function() p:set(new) end)
    if param_string(p) == new then return true end
    pcall(function()           -- fallback: mutate the FString in place
        local v = p:get(); v:Clear(); v:Append(new)
    end)
    return param_string(p) == new
end

local function null_object(p)
    local ok = pcall(function() p:set(nil) end)
    if not ok then return false end
    local v; pcall(function() v = p:get() end)
    if v == nil then return true end
    local valid = false
    pcall(function() valid = v:IsValid() end)
    return not valid
end

-- --- the hook ------------------------------------------------------------------

local function on_call(spec, args)
    st.stats.calls = st.stats.calls + 1
    local p = args[spec.slot]
    local slot = p and param_string(p)
    if not slot then
        st.stats.failed = st.stats.failed + 1
        ev("x_save_call", { fn = spec.name, slot = "?", active = st.active, err = "slot unreadable" })
        return
    end
    -- self-test probe (M.selftest): always rewritten, never touches a real slot
    if slot == "__hsmp_probe__" then
        local to = M.mapped("probe")
        st.probe = { fn = spec.name, ok = set_string(p, to) }
        return
    end
    local active = M.refresh(false)
    -- Writes/deletes are always reported (rare; they let a test attribute a
    -- career-file mtime change to a pre-session native write, e.g.
    -- the main menu's SaveGameToSlot("Settings") on load). Reads only with spy.
    if cfg.spy or spec.kind ~= "read" then ev("x_save_call", { fn = spec.name, slot = slot, active = active }) end
    if not active or M.is_hsmp_slot(slot) or cfg.passthrough[slot] then
        st.stats.passthrough = st.stats.passthrough + 1
        return
    end

    if spec.kind == "read" then
        local to = read_target(slot)
        if not to then return end   -- COW: career read, harmless
        local ok = set_string(p, to)
        if ok then st.stats.redirected = st.stats.redirected + 1 else st.stats.failed = st.stats.failed + 1 end
        ev("save_redirected", { fn = spec.name, slot = slot, to_slot = to, op = "read", ok = ok })
        return
    end

    -- write / delete
    local to = M.mapped(slot)
    local ok, how = false, nil
    if cfg.mode == "block" and spec.obj and args[spec.obj] then
        ok = null_object(args[spec.obj]); how = "blocked"
        if ok then st.stats.blocked = st.stats.blocked + 1 end
        to = "<blocked>"
    else
        ok = set_string(p, to); how = "redirected"
        if ok then
            st.stats.redirected = st.stats.redirected + 1
            if spec.kind == "delete" then st.deleted[to] = true; st.written[to] = nil
            else st.written[to] = true; st.deleted[to] = nil end
        elseif spec.obj and args[spec.obj] then
            -- could not rewrite: neutralise the write instead
            if null_object(args[spec.obj]) then
                ok, how, to = true, "blocked (rewrite failed)", "<blocked>"
                st.stats.blocked = st.stats.blocked + 1
            end
        end
    end
    if not ok then
        st.stats.failed = st.stats.failed + 1
        plog("WARNING: %s('%s') could NOT be diverted; career file guard must restore it", spec.name, slot)
    end
    ev("save_redirected", { fn = spec.name, slot = slot, to_slot = to, op = spec.kind, ok = ok, how = how })
end

-- --- public API -------------------------------------------------------------------

-- opts: mod, inst, state_dir, save_dir, mode, spy, live_s, sticky_s, poll_s,
--       fresh_per_session, reload_gi_on_exit, passthrough, log, print, now, clock,
--       register_hook, get_gi, link (tests: function -> status name, heartbeat age).
function M.install(opts)
    if st.installed then return M end
    opts = opts or {}
    for k, v in pairs(opts) do if cfg[k] ~= nil or k == "log" or k == "save_dir"
        or k == "register_hook" or k == "get_gi" or k == "link" then cfg[k] = v end end
    cfg.inst = tostring(opts.inst or trim(os.getenv("HSMP_INST")) or "0")
    if cfg.inst == "" then cfg.inst = "0" end
    cfg.state_dir = ((opts.state_dir or trim(os.getenv("HSMP_STATE_DIR")) or "hsmp_state"):gsub("\\", "/"))
    if not opts.save_dir then
        local la = trim(os.getenv("LOCALAPPDATA"))
        if la and la ~= "" then cfg.save_dir = (la:gsub("\\", "/")) .. "/HalfSwordUE5/Saved/SaveGames" end
    else
        cfg.save_dir = (opts.save_dir:gsub("\\", "/"))
    end
    if not cfg.log then
        local ok, L = pcall(require, "hsmp_log")
        if ok and type(L) == "table" then cfg.log = L end
    end
    local reg = cfg.register_hook or RegisterHook
    for _, spec in ipairs(M.FUNCTIONS) do
        local ok, err = pcall(reg, spec.path, function(_ctx, ...)
            local args = { ... }
            local ok2, e2 = pcall(on_call, spec, args)
            if not ok2 then plog("hook error in %s: %s", spec.name, tostring(e2)) end
        end)
        st.hooks[spec.name] = ok and true or tostring(err)
        if not ok then plog("could not hook %s: %s", spec.path, tostring(err)) end
    end
    st.installed = true
    local n = 0; for _, v in pairs(st.hooks) do if v == true then n = n + 1 end end
    plog("installed v%d (inst=%s mode=%s, %d/%d hooks, save_dir=%s)", M.VERSION, cfg.inst, cfg.mode,
        n, #M.FUNCTIONS, tostring(cfg.save_dir))
    M.refresh(true)
    -- Every instance states its guard state once at start (later
    -- changes emit their own x_save_guard in set_state).
    if not st.state_emitted then
        ev("x_save_guard", { active = st.active, why = "install: " .. tostring(st.reason), session = st.sessions })
        st.state_emitted = true
    end
    return M
end

function M.hooks() return st.hooks end
function M.state() return st end
function M.config() return cfg end

-- Game-thread periodic call: re-evaluates liveness and performs deferred work.
function M.tick()
    M.refresh(false)
    if st.pending_gi_reload and not st.active then
        st.pending_gi_reload = false
        local ok, err = pcall(function()
            local gi
            if cfg.get_gi then gi = cfg.get_gi()
            else gi = require("UEHelpers").GetGameInstance() end
            if gi and gi:IsValid() then
                gi["Load Game"](gi)   -- real BP name (spaces); career slot, guard inactive
            else
                error("no valid GameInstance")
            end
        end)
        plog("career GI reload after session: %s", ok and "ok" or ("FAILED: " .. tostring(err)))
        ev("x_save_guard", { active = false, why = "gi_reload", ok = ok })
    end
end

-- Seed this session's HSMP_<inst>_GameProgress from the CURRENT GameInstance
-- state by calling GI "Save Game" while the guard is active (the hook diverts
-- it). Why: BP_GameManager's BeginPlay runs GI "Load Game" on every arena load,
-- which overwrites GI fields the Director set before OpenLevel with
-- the slot's contents. Until the session slot exists, copy-on-write serves the
-- career slot, so the first arena of a session would get career values (e.g.
-- "Player Just Died"). Call this after applying the MP GI values and before
-- OpenLevel. Returns ok, detail. Game thread only.
function M.seed_session_slot(gi)
    if not M.refresh(true) then return false, "guard inactive (would write the career slot)" end
    local before = st.stats.redirected
    local ok, err = pcall(function()
        if not gi then
            if cfg.get_gi then gi = cfg.get_gi() else gi = require("UEHelpers").GetGameInstance() end
        end
        if not (gi and gi:IsValid()) then error("no valid GameInstance") end
        gi["Save Game"](gi)
    end)
    if not ok then return false, tostring(err) end
    if st.stats.redirected <= before then return false, "Save Game ran but no write was diverted" end
    return true, M.mapped("GameProgress")
end

-- Calls DoesSaveGameExist("__hsmp_probe__") through ProcessEvent and checks
-- that the hook fired and the SlotName rewrite read back. Read-only, harmless.
-- Returns ok, detail. Only proves the ProcessEvent path; the BP-VM path is
-- proven by real save_redirected events.
function M.selftest(gs)
    st.probe = nil
    local ok, err = pcall(function()
        gs = gs or require("UEHelpers").GetGameplayStatics()
        gs:DoesSaveGameExist("__hsmp_probe__", 0)
    end)
    if not ok then return false, "call failed: " .. tostring(err) end
    if not st.probe then return false, "hook did not fire" end
    return st.probe.ok, st.probe.ok and "rewrite ok" or "rewrite did not read back"
end

-- tests only
function M._reset() defaults() end
M._on_call = on_call

return M
