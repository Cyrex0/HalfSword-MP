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

-- HSMPMenu — native main-menu integration for Half Sword Multiplayer.
--
-- Lifecycle on the Startup menu:
--   1. Inject 5 UButton "ribbons" as a 2nd column (HOST GAME / SERVER BROWSER /
--      SETTINGS / CHARACTER / QUIT MP), cloning Button_1's scroll style.
--   2. HOST GAME       -> spawn hsmp-server + hsmp-sidecar, then the LOBBY screen
--   3. SERVER BROWSER  -> the browser screen (browser.lua)
--   4. SETTINGS        -> the settings screen (settings.lua)
--   5. CHARACTER       -> the character screen
--   6. QUIT MP         -> tear the MP session down, then console "quit"
--
-- Every sub-screen is built with ui_kit.lua (required) on the Startup menu's
-- own CanvasPanel: no overlays, no viewport widgets. A screen hides the
-- native Button + Button_0..4 and the 5 ribbons; BACK restores them.
--
-- Input: keyboard and gamepad reach every action (docs/development/subsystems/menu-ui.md
-- "Navigation"). Keys are registered ONCE at load and routed by a queue that
-- the 33 ms game-thread poll drains: into the active screen (ui_kit focus
-- manager), or into the ribbon column on the top level, or dropped.

local UEHelpers = require("UEHelpers")

-- Thread-safety shim. LoopAsync, ExecuteWithDelay and *Async keybind
-- callbacks run on UE4SS worker threads; running them alongside game-thread
-- Lua corrupts this mod's VM (crash dumps show lua_next / __index on garbage
-- userdata). Route every deferred, looped and key-bound callback onto the
-- game thread.
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

local function Log(fmt, ...) print(string.format("[HSMPMenu] " .. fmt .. "\n", ...)) end

-- --- paths + persisted settings -------------------------------------------

local function trim(s) return s and s:match("^%s*(.-)%s*$") or nil end
local function env_get(k) local v = trim(os.getenv(k)); if v == "" then return nil end; return v end
-- the public server list; dev and test deploys write a local master into hsmp.cfg
local PUBLIC_MASTER = "https://master.halfswordmp.workers.dev"

-- Sibling modules: require() first, then dofile next to this file, then the
-- repo's shared/ dir (dev tree; build-and-deploy copies shared libs next to us).
local SCRIPT_DIR = ((debug.getinfo(1, "S").source or ""):gsub("^@", "")):match("^(.*)[/\\]") or "."
local function load_module(name, quiet)
    local ok, mod = pcall(require, name)
    if ok and type(mod) == "table" then return mod end
    local errs = { tostring(mod) }
    for _, p in ipairs({ SCRIPT_DIR .. "/" .. name .. ".lua", SCRIPT_DIR .. "/../../shared/" .. name .. ".lua" }) do
        local f = io.open(p, "rb")
        if f then
            f:close()
            local ok2, mod2 = pcall(dofile, p)
            if ok2 and type(mod2) == "table" then return mod2 end
            errs[#errs + 1] = tostring(mod2)
        end
    end
    if not quiet then Log("%s.lua failed to load (%s)", name, table.concat(errs, " / ")) end
    return nil
end
-- UMG widget helpers (menu_umg.lua).
local U = load_module("menu_umg")
if not U then error("HSMPMenu: menu_umg.lua is missing (deploy copies every Scripts/*.lua)") end

-- shared/hsmp_cfg.lua (docs/development/testing.md): every path and
-- URL comes from it (hsmp.cfg next to the game exe + the HSMP_* env overrides).
-- When the library is not deployed, a built-in fallback with the SAME defaults
-- and env overrides is used (bin_dir "hsmp" next to the game, the public list);
-- there is no hard-coded developer path anywhere in this mod.
local function fallback_cfg()
    local F = { _fallback = true }
    F.bin_dir = ((env_get("HSMP_BIN_DIR") or "hsmp"):gsub("\\", "/"):gsub("/+$", ""))
    function F.safe_url(u)
        u = trim(u or "")
        if u and u:match("^https?://[%w%.%-]+[:%d]*[%w%./%-_]*$") then return (u:gsub("/+$", "")) end
        return nil
    end
    -- master_url may be a comma-separated list (primary first), like hsmp_cfg
    local urls = {}
    for part in ((env_get("HSMP_MASTER_URL") or "") .. ","):gmatch("([^,]*),") do
        local u = F.safe_url(part)
        if u then urls[#urls + 1] = u end
    end
    if #urls == 0 then urls = { PUBLIC_MASTER } end
    F.master_url = urls[1]
    function F.master_urls() local o = {}; for i, u in ipairs(urls) do o[i] = u end; return o end
    function F.get(_, default) return default end
    local function exe(envk, name) return ((env_get(envk) or (F.bin_dir .. "/" .. name .. ".exe")):gsub("\\", "/")) end
    function F.server_exe()  return exe("HSMP_SERVER_EXE", "hsmp-server") end
    function F.sidecar_exe() return exe("HSMP_SIDECAR_EXE", "hsmp-sidecar") end
    function F.query_exe()   return exe("HSMP_QUERY_EXE", "hsmp-query") end
    function F.state_dir()   return ((env_get("HSMP_STATE_DIR") or "hsmp_state"):gsub("\\", "/")) end
    function F.dev()         return os.getenv("HSMP_DEV") == "1" end
    return F
end
local CfgLib = load_module("hsmp_cfg", true)
local CfgFB = fallback_cfg()
-- cfg(name): the library's function/field when it has one, else the fallback's.
local function cfg(name)
    local src = (type(CfgLib) == "table" and CfgLib[name] ~= nil) and CfgLib or CfgFB
    local v = src[name]
    if type(v) == "function" then
        local ok, r = pcall(v)
        if ok and type(r) == "string" and r ~= "" then return r end
        v = CfgFB[name]; if type(v) == "function" then return v() end
    end
    return v
end
local function cfg_safe_url(u)
    local f = (type(CfgLib) == "table" and type(CfgLib.safe_url) == "function") and CfgLib.safe_url or CfgFB.safe_url
    local ok, r = pcall(f, u)
    return ok and r or nil
end
local BIN_DIR     = tostring(cfg("bin_dir"))
local SIDECAR_EXE = cfg("sidecar_exe")
local SERVER_EXE  = cfg("server_exe")
local QUERY_EXE   = cfg("query_exe")
local MASTER_URL  = cfg_safe_url(cfg("master_url")) or PUBLIC_MASTER
-- Every master to try, primary (MASTER_URL, the one a hosted server registers
-- with) first. An hsmp_cfg without list support gives just { MASTER_URL }.
local MASTER_URLS = { MASTER_URL }
do
    local lib = (type(CfgLib) == "table" and type(CfgLib.master_urls) == "function") and CfgLib or CfgFB
    local ok, list = pcall(lib.master_urls)
    if ok and type(list) == "table" and #list > 0 then
        local out, seen = {}, {}
        for _, u in ipairs(list) do
            local s = cfg_safe_url(u)
            if s and not seen[s] then seen[s] = true; out[#out + 1] = s end
        end
        if #out > 0 and out[1] == MASTER_URL then MASTER_URLS = out end
    end
end
-- Any other hsmp.cfg key (string), e.g. lan_ports / lan_discovery / local_master.
local function cfg_get(key, default)
    if type(CfgLib) == "table" and type(CfgLib.get) == "function" then
        local ok, v = pcall(CfgLib.get, key, default)
        if ok and v ~= nil then return tostring(v) end
    end
    return default
end
local DEV = os.getenv("HSMP_DEV") == "1"      -- dev-only keys (hsmp_cfg.dev() reads the same variable)

local STATE_DIR = cfg("state_dir")
os.execute("mkdir \"" .. STATE_DIR:gsub("/", "\\") .. "\" 2>nul")
local SETTINGS_FILE  = STATE_DIR .. "/.settings.json"
local CHARACTER_FILE = STATE_DIR .. "/.my_character.json"
-- Commands are typed `command` records (IPC.send), sent only once the session is
-- connected: records sent earlier are held here (newest 32) and flushed by the lobby poll.
local CTL = { hold = {} }
local send_command   -- defined with the lobby helpers

-- shared/hsmp_log.lua: JSONL events for the gate. A no-op
-- stub with the same API when the library is not deployed.
local HLog = load_module("hsmp_log", true)
if HLog and type(HLog.init) == "function" then
    pcall(HLog.init, { mod = "HSMPMenu", state_dir = STATE_DIR })
end
-- Shared-memory IPC facade (shared/hsmp_ipc.lua): published as the per-state global
-- HSMP_IPC (no new local: the main chunk is near the 200-locals limit).
do
    local m = load_module("hsmp_ipc", true)
    if m then m.init({ mod = "HSMPMenu", state_dir = STATE_DIR, log = Log }) end
end
local function ev(name, fields)
    if not HLog then return end
    pcall(function()
        local f = HLog.event or HLog.emit or HLog.log
        if type(f) == "function" then f(name, fields or {}) end
    end)
end
Log("paths: bin_dir=%s (%s) master=%s state=%s events=%s", BIN_DIR, CfgLib and "hsmp_cfg" or "fallback",
    MASTER_URL, STATE_DIR, HLog and "hsmp_log" or "stub")
if #MASTER_URLS > 1 then Log("master fallbacks: %s", table.concat(MASTER_URLS, ", ", 2)) end

local J = load_module("jsonlite")

-- The 7 picker arenas the host can REQUEST in the lobby (pick_arena). The
-- server validates the pick against its registry and owns the result.
local MAP_PRESETS  = {
    { name = "Alley",         path = "Map_Arena_Alley" },
    { name = "Pit",           path = "Map_Arena_Pit" },
    { name = "Yard",          path = "Map_Arena_Yard" },
    { name = "Slums",         path = "Map_Arena_Slums" },
    { name = "Cellar",        path = "Map_Arena_Cellar" },
    { name = "Lords Hall",    path = "Map_Arena_LordsHall" },
    { name = "East Tower",    path = "Map_Arena_EastTower" },
}
-- Spawn counts from the generated arena catalogue (shared/hsmp_arenas.lua), for the tiles.
do
    local A = load_module("hsmp_arenas", true)
    for _, m in ipairs(MAP_PRESETS) do
        local a = type(A) == "table" and A[m.path] or nil
        if type(a) == "table" and type(a.spawns) == "table" then
            local n = 0
            for _, sp in ipairs(a.spawns) do if sp.valid ~= false then n = n + 1 end end
            m.spawns = n
        end
    end
end
local BEST_OF = { 1, 3, 5, 7 }      -- rounds chips (server: set_best_of, 1..31)

local function map_display_name(path)
    for _, m in ipairs(MAP_PRESETS) do if m.path == path then return m.name end end
    return path or "?"
end

-- Forward declaration so functions declared BEFORE the full state table
-- (spawn_* helpers) can still see it via upvalue capture.
local state = {
    menu = nil, canvas = nil, wt = nil,
    style_src = nil, text_src = nil,
    nmin_x = 0, nmin_y = 0, nmax_x = 0, nmax_y = 0,
    vw = 1920, vh = 1080,
    rib_w = 509, rib_h = 129, v_gap = -23,
    injected = false,
    ribbons = {},
    native_names = { "Button", "Button_0", "Button_1", "Button_2", "Button_3", "Button_4" },
    native_hidden = {},
    screen_active = nil,
    screen_widgets = {},
    screen_build  = nil,
    top_focus = nil,         -- index of the keyboard-focused ribbon (top level), or nil
    top_ring = nil,          -- 4 Borders around it
}

-- World guard. Every widget cached in `state` lives in (and is freed with) the
-- world it was built in; a level load (menu -> arena, round reset, back to the
-- menu) frees them. Touching one afterwards is an access violation pcall
-- can't catch. The LoadMap pre-hook (game thread, before the old world is torn
-- down) bumps menu_world_gen and forgets every widget WITHOUT touching it;
-- loops compare the generation before polling anything. The menu re-injects
-- itself on the next UI_Startup_Menu (NotifyOnNewObject below).
local menu_world_gen = 0
-- Screen modules (ui_kit, browser, classes, settings, lobby/character builds)
-- register a function here that drops their cached widget refs WITHOUT
-- touching them. Run on screen exit (destroy_screen_widgets) and on world change.
local forget_hooks = {}
local function drop_screen_refs()
    for _, f in ipairs(forget_hooks) do pcall(f) end
end
local function forget_widgets()
    state.injected = false
    state.ribbons = {}
    state.screen_widgets = {}
    state.screen_active = nil
    state.screen_build = nil
    state.screen_render = nil
    state.native_hidden = {}
    state.top_focus, state.top_ring = nil, nil
    state.menu, state.canvas, state.wt = nil, nil, nil
    state.style_src, state.text_src = nil, nil
    state.crash_note = nil
    drop_screen_refs()
end
pcall(function()
    RegisterLoadMapPreHook(function()
        menu_world_gen = menu_world_gen + 1
        forget_widgets()
    end)
end)
pcall(function()
    RegisterLoadMapPostHook(function() menu_world_gen = menu_world_gen + 1 end)
end)

-- Settings (settings.lua): one .settings.json line, merged on write so other
-- mods' keys survive, validated at load (nick and server go into command
-- lines).
-- shared/hsmp_ui_scale.lua: the one UI scaling rule (viewport probe, design
-- 1920x1080 -> canvas, fonts, safe margins). ui_kit gets it through Kit.init.
local UIS = load_module("hsmp_ui_scale", true)
if UIS then package.loaded["hsmp_ui_scale"] = UIS end
local Kit = load_module("ui_kit")
local Settings = load_module("settings")
local settings
if Settings and J then
    settings = Settings.init({ kit = Kit, json = J, log = Log, path = SETTINGS_FILE, state_dir = STATE_DIR,
        exit_screen = function() exit_screen() end,
        default_urls = function() return MASTER_URLS end,
        on_saved = function(changed) on_settings_saved(changed) end,
        forget_server_mods = function() return MX.Mods and MX.Mods.forget_all() end,
        server_mods_count = function() return MX.Mods and MX.Mods.count() or 0 end })
else
    Log("settings.lua / jsonlite.lua missing - using built-in defaults (nothing is saved)")
    settings = { nick = "Willie", server = "127.0.0.1:7777", send_hz = 60, hud = true, avatars = true,
                 lobby_map = MAP_PRESETS[1].path, lobby_mode = "Best of 3", region = "", master_url = "" }
end
local function save_settings() if Settings then Settings.save() end end

-- The server lists to use: HSMP_MASTER_URL (env) wins, then the list saved in
-- SETTINGS, then hsmp.cfg's. The first is the one a hosted server registers with.
local function effective_master_urls()
    if env_get("HSMP_MASTER_URL") then return MASTER_URLS end
    if Settings then
        local list = Settings.master_urls()
        if #list > 0 then return list end
    end
    return MASTER_URLS
end
local function primary_master() return effective_master_urls()[1] or MASTER_URL end

local character = { str = 5, agi = 5, int = 5, sta = 5 }

-- Lobby runtime state (not persisted — reset each session)
local lobby = {
    active        = false,   -- true while on the LOBBY sub-screen
    is_host       = false,   -- true if we spawned the server
    is_ready      = false,   -- local "ready" state (toggled by button)
    launched      = false,   -- true once a travel request went to the Director this round
    chosen_map    = MAP_PRESETS[1].path,  -- display hint only (advertised / host's last pick); never travelled to
    serial        = 0,       -- bumped per session (HOST / JOIN / CANCEL)
}
-- Misc helpers (one table: main.lua is at Lua's 200-local limit).
local MX = { boot_wall = os.time(), HS = load_module("hsmp_session", true) }   -- shared/hsmp_session.lua: the typed session view

local function load_character()
    local f = io.open(CHARACTER_FILE, "rb"); if not f then return end
    local line = f:read("l"); f:close(); if not line then return end
    for _, k in ipairs({ "strength","agility","intelligence","stamina" }) do
        local n = tonumber(line:match('"' .. k .. '"%s*:%s*(%-?%d+)'))
        if n then character[({ strength="str", agility="agi", intelligence="int", stamina="sta" })[k]] = math.max(0, math.min(10, n)) end
    end
end

local function save_character()
    local f = io.open(CHARACTER_FILE, "wb"); if not f then return false end
    f:write(string.format('{"strength":%d,"agility":%d,"intelligence":%d,"stamina":%d}\n',
        character.str, character.agi, character.int, character.sta))
    f:close()
    return true
end

load_character()

-- --- server-launch helpers ------------------------------------------------

-- The arena the HOST asks the server to boot with (--map). It is a request:
-- the server's arena (the session snapshot) is the only truth afterwards. Level travel
-- and GameInstance writes are NOT done here: they belong to the Director
-- (travel.lua requests; legacy_travel.lua is the removable shim).
local LOBBY_MAP = os.getenv("HSMP_LOBBY_MAP") or "Map_Arena_Alley"

local Cmd, Travel, Legacy     -- commands.lua, travel.lua, legacy_travel.lua (loaded below)

-- A new MP session (HOST / JOIN / CANCEL): forget the previous session's
-- commands, travel latch and lobby_ready event.
local function reset_lobby_session(is_host, chosen)
    lobby.serial     = lobby.serial + 1
    lobby.is_host    = is_host
    lobby.is_ready   = false
    lobby.launched   = false
    lobby.chosen_map = chosen
    lobby.want_map, lobby.map_cmd = nil, nil
    lobby.start_cmd, lobby.ready_cmd, lobby.mode_cmd, lobby.note_cmd = nil, nil, nil, nil
    lobby.rules_cmd, lobby.kick_cmd, lobby.promote_cmd, lobby.sel_peer = nil, nil, nil, nil
    lobby.travel_note, lobby.travel_t, lobby.no_arena_logged = nil, nil, nil
    lobby.ready_evented, lobby.ready_epoch, lobby.ready_rearm = false, nil, nil
    lobby.session_wall = os.time()
    lobby.entered_t = nil
    lobby.over = nil
    MX.net_status = nil        -- the new server reports its own reachability
    lobby.saw_live = false     -- this session's sidecar has written a live status
    lobby.saw_terminal, lobby.term = false, nil   -- a terminal status seen / pending
    lobby.prefs_sent = false   -- the host's saved ROUNDS / KIT RULES not re-sent yet
    lobby.lost_seen, lobby.lost_since, lobby.hold_logged = false, nil, nil
    CTL.hold = {}              -- lines held for an earlier session that never connected
    -- The previous session's host rules request must not be re-sent to
    -- another server when this player becomes admin there.
    if HSMP_IPC then HSMP_IPC.put("kit_rules_req", { seq = 0 }) end   -- the kit_rules_req slot: seq 0 = no request
    if Cmd then Cmd.reset() end
    if MX.Mods then MX.Mods.reset() end   -- server mods: the last server's offer is gone
end

-- --- process tracking (docs/development/testing.md) -------------------------
-- The native module spawns hsmp-server / hsmp-sidecar / hsmp-master
-- (IPC.spawn: CreateProcessW, no window, no shell, args as an array) and keeps
-- their process handles. The game remembers the PIDs it spawned (MX.procs):
-- CANCEL asks the sidecar to leave, waits for it, then kills exactly the
-- processes THIS game spawned (IPC.proc_kill, by our handle, the child's tree).
-- Never by image name, never by a pid file: with nothing spawned nothing is
-- killed. The shipped binaries match the module (the shared-memory ABI check
-- is the capability check), so --parent-pid / --owner-key-file are always
-- passed: no --help probes.
MX.procs = {}
local function win(p) return (p:gsub("/", "\\")) end

-- The game's own PID, for --parent-pid: the sidecar and the listen
-- server exit when the game does. The native module knows it (current_pid).
local GAME_PID = rawget(_G, "HSMP_IPC") and HSMP_IPC.current_pid() or nil
if GAME_PID then Log("game pid %d: passed as --parent-pid", GAME_PID)
else Log("game pid unknown (no native module): the sidecar cannot be started") end

-- Career-guard crash recovery at boot (`hsmp-sidecar --career-recover`, the launcher's
-- step too): a session a crash left open is restored before the game writes its first save,
-- also when the game was started from Steam. It touches nothing when no session is open, a
-- live sidecar's session, or a career file written after that session ended. Waits up to
-- 3 s; a slower run finishes in the background (MX.career_poll).
MX.career = { started = os.clock() }
function MX.career_done(code, out)
    MX.career.h = nil
    for line in tostring(out or ""):gmatch("[^\r\n]+") do Log("career recover: %s", line) end
    Log("career recover: exit %s after %.0f ms", tostring(code), (os.clock() - MX.career.started) * 1000)
    ev("x_career_recover", { code = tonumber(code), ms = math.floor((os.clock() - MX.career.started) * 1000) })
end
function MX.career_poll(wait_ms)
    local ipc, h = rawget(_G, "HSMP_IPC"), MX.career.h
    if not (ipc and h) then return end
    local done, code, out = ipc.capture_poll(h, wait_ms)
    if done == true then MX.career_done(code, out)
    elseif done == nil then MX.career.h = nil; Log("career recover: lost (%s)", tostring(code)) end
end
do
    local ipc = rawget(_G, "HSMP_IPC")
    if ipc and ipc.N then
        local h, err = ipc.spawn_capture(win(tostring(SIDECAR_EXE)), { "--career-recover" })
        if h then
            MX.career.h = h
            MX.career_poll(3000)
            if MX.career.h then Log("career recover: still running after 3 s, finishing in the background") end
        else
            Log("career recover: could not start %s (%s)", tostring(SIDECAR_EXE), tostring(err))
        end
    else
        Log("career recover: skipped (no native module); the launcher runs it before Play")
    end
end

local kill_role

-- The role's extra args (an array) or nil, reason (sidecar only).
local function proc_args(role)
    if role == "sidecar" then
        if MX.build_block then return nil, "build mismatch: " .. MX.build_block end
        -- Shared memory is the sidecar's ONLY link to the
        -- game. The game creates the segment; the sidecar attaches to it and
        -- verifies the segment's game pid against --parent-pid, so both are
        -- required: without them the sidecar is not started (nil, reason).
        local ipc = rawget(_G, "HSMP_IPC")
        if not ipc then return nil, "shared/hsmp_ipc.lua missing" end
        if not GAME_PID then return nil, "game pid unknown" end
        local name, err = ipc.open()
        if type(name) ~= "string" or not name:match("^[%w%.\\_%-]+$") then
            return nil, tostring(err or ipc.why or name)
        end
        Log("sidecar: shared-memory IPC %s", name)
        return { "--parent-pid", tostring(GAME_PID), "--ipc", "shm:" .. name }
    end
    if GAME_PID then return { "--parent-pid", tostring(GAME_PID) } end
    return {}
end

-- The sidecar could not be started: shared-memory IPC is unavailable. The
-- lobby note shows the reason (a broken install: reinstall HSMP).
function MX.ipc_fail(why)
    local ipc = rawget(_G, "HSMP_IPC")
    MX.ipc_error = MX.build_block or (ipc and ipc.ui_error) or "Helper program did not start - reinstall HSMP"
    Log("sidecar NOT started: shared-memory IPC unavailable (%s)", tostring(why))
end

-- Build identity check (shared/hsmp_build.lua): these mod files against
-- `hsmp-sidecar --build-info`, once at startup. A mismatch sets MX.build_block,
-- which refuses HOST / JOIN with the launcher hint.
MX.Build = load_module("hsmp_build", true)
MX.build_id = MX.Build and MX.Build.load(load_module) or nil
function MX.build_poll()
    local B = MX.Build
    if not B or MX.build_done then return end
    local ipc = rawget(_G, "HSMP_IPC")
    if not MX.build_id then
        MX.build_done, MX.build_block = true, B.MSG_MISSING
        Log("build check: %s (hsmp_build_id.lua missing): multiplayer is off", B.MSG_MISSING)
        return
    end
    if not ipc or not ipc.spawn_capture then return end
    if not MX.build_h then
        MX.build_tries = (MX.build_tries or 0) + 1
        if MX.build_tries > 3 then MX.build_done = true; Log("build check: hsmp-sidecar --build-info did not start; skipped"); return end
        MX.build_h = ipc.spawn_capture(win(tostring(SIDECAR_EXE)), { "--build-info" })
        return
    end
    local done, code, out = ipc.capture_poll(MX.build_h)
    if done == false then return end
    MX.build_h = nil
    if done == nil then return end   -- lost: started again on the next poll
    MX.build_done = true
    local bad = B.check(MX.build_id, code == 0 and B.parse_info(out) or nil)
    if bad then
        MX.build_block = bad.msg
        Log("build check FAILED: %s (%s): multiplayer is off", bad.msg, bad.detail)
    else
        Log("build check: mods and sidecar are HalfSword-MP %s (content %s)", MX.build_id.version, B.content_tag(MX.build_id))
    end
end

-- This run's log folder (diag.lua): `hsmp-sidecar --log-session start` at boot, then the
-- watcher that collects the logs when the game exits. The sidecar and a listen server get
-- `--log-dir` (MX.log_args) once the folder is known.
MX.Diag = load_module("diag", true)
if MX.Diag then
    MX.Diag.init({ log = Log, ipc = function() return rawget(_G, "HSMP_IPC") end, sidecar_exe = win(tostring(SIDECAR_EXE)),
                   game_pid = GAME_PID, state_dir = win(STATE_DIR) })
end
function MX.log_args() return MX.Diag and MX.Diag.log_args() or {} end

-- Start `exe args` as `role` (a previous child of that role is stopped first).
-- opts: {env = {K = v}}. Returns the pid or nil, err.
function MX.spawn_role(role, exe, args, opts)
    local ipc = rawget(_G, "HSMP_IPC")
    if not ipc then Log("%s NOT started: shared/hsmp_ipc.lua missing", role); return nil, "no ipc" end
    if MX.procs[role] then kill_role(role) end
    exe = win(tostring(exe))
    local pid, err = ipc.spawn(exe, args, opts or {})
    Log("%s: %s %s -> %s", role, tostring(exe), table.concat(args, " "), pid and ("pid " .. tostring(pid)) or ("FAILED " .. tostring(err)))
    if not pid then return nil, err end
    MX.procs[role] = pid
    return pid
end

-- Stop one role this game spawned (by our process handle). Returns true when a kill was issued.
function kill_role(role)
    local pid = MX.procs[role]
    if not pid then Log("stop %s: not started by this game - nothing killed", role); return false end
    MX.procs[role] = nil
    local ipc = rawget(_G, "HSMP_IPC")
    local ok, err = false, "no ipc"
    if ipc then ok, err = ipc.proc_kill(pid) end
    Log("stop %s: pid %d %s", role, pid, ok and "killed" or ("(" .. tostring(err) .. ")"))
    return ok and true or false
end

-- The hosting instance's own hsmp-master (local_master.lua): HOST starts one
-- when the primary master_url is on this machine and nothing answers there;
-- CANCEL / quit stop it only when this game started it (our process handle).
local LocalMaster = load_module("local_master")
if LocalMaster then
    local ipc = rawget(_G, "HSMP_IPC")
    LocalMaster.init({
        log = Log, now = os.clock, url = primary_master(), state_dir = STATE_DIR,
        master_exe = ((BIN_DIR .. "/hsmp-master.exe"):gsub("\\", "/")), query_exe = QUERY_EXE,
        mode = (cfg_get("local_master", "auto"):lower() == "off") and "off" or "auto",
        exists = function(p) local f = io.open(p, "rb"); if f then f:close(); return true end; return false end,
        spawn = function(exe, args) return MX.spawn_role("master", exe, args) end,
        spawn_capture = function(exe, args) if not ipc then return nil, "no ipc" end; return ipc.spawn_capture(exe, args) end,
        capture_poll = function(h) if not ipc then return nil, "no ipc" end; return ipc.capture_poll(h) end,
        alive = function(pid) return ipc ~= nil and ipc.proc_alive(pid) end,
        kill = function() return kill_role("master") end,
        -- the local master exits with the game, however it ends
        parent_args = function() return proc_args("master") end,
    })
end

-- SETTINGS saved: a new server list moves the local-master check with it.
function on_settings_saved(changed)
    if changed.master_url and LocalMaster and LocalMaster.set_url then
        LocalMaster.set_url(primary_master())
        Log("server lists now: %s", table.concat(effective_master_urls(), ", "))
    end
end

-- The HOST PORT setting; anything outside 1024-65535 (a hand-edited settings file) is 7777,
-- never port 0 (an OS-picked port nobody could reach) or a port that cannot bind.
local function host_port()
    local p = tonumber((settings.server or ""):match(":(%d+)$"))
    if not p or p < 1024 or p > 65535 or p ~= math.floor(p) then return 7777 end
    return p
end

-- The server-mods cache the sidecar writes and HSMPModHost loads (hsmp_cfg; outside ue4ss/Mods).
function MX.mods_cache()
    local d = (type(CfgLib) == "table" and type(CfgLib.mods_cache_dir) == "function") and CfgLib.mods_cache_dir() or "hsmp_mods"
    return win(d)
end

-- HOST GAME: boots hsmp-server + hsmp-sidecar and sends the user to the
-- LOBBY sub-screen. No level travel yet — the player waits in-lobby until
-- the server starts the match; every client then loads the server's arena.
local function spawn_server_and_sidecar()
    local chosen_map = settings.lobby_map or LOBBY_MAP
    local port = tostring(host_port())
    local master = primary_master()
    -- Server-browser listing name/mode/region (cmd-safe characters only).
    local adv_name = ((settings.nick or "Host") .. "'s game"):gsub("[^%w %-_%.']", ""):sub(1, 40)
    local adv_mode = tostring(settings.lobby_mode or "Best of 3"):gsub("[^%w %-_%.]", ""):sub(1, 30)
    local region = tostring(settings.region or ""):gsub("[^%w%-]", ""):sub(1, 16)
    -- Local/LAN play: a master on this machine must be up for the browser to
    -- list us (async; the server's master_client retries registration).
    if LocalMaster then
        if LocalMaster.set_url then LocalMaster.set_url(master) end
        pcall(LocalMaster.ensure, "HOST")
    end
    -- HSMP_LISTEN_HOST=1: when this (listen) host leaves, the server closes and
    -- tells every joiner "Host closed the server" (docs/development/subsystems/director.md).
    -- This player is the server's admin by its player key, which
    -- our sidecar writes to <state>/.player_key before it connects (the server
    -- reads it at each join until we have joined). Never "first to join".
    -- The sidecar's shared-memory link first: without it nothing is started.
    local sc_args, sc_err = proc_args("sidecar")
    if not sc_args then
        MX.ipc_fail(sc_err)
        ExecuteWithDelay(100, function() enter_screen("lobby") end)
        return
    end
    MX.ipc_error = nil
    sc_args[#sc_args + 1] = "--mods-cache"; sc_args[#sc_args + 1] = MX.mods_cache()
    -- No shell: the args are an array and the listing values travel as the
    -- server's own environment (IPC.spawn opts.env).
    local server_args = { "--bind", "0.0.0.0:" .. port, "--max-peers", "8", "--map", chosen_map,
                          "--owner-key-file", win(STATE_DIR .. "/.player_key") }
    for _, a in ipairs(proc_args("server")) do server_args[#server_args + 1] = a end
    for _, a in ipairs(MX.log_args()) do server_args[#server_args + 1] = a end
    -- SETTINGS > AUTO PORT FORWARD off: no UPnP / PCP / NAT-PMP mapping on the router.
    if settings.upnp == false then
        server_args[#server_args + 1] = "--port-map"; server_args[#server_args + 1] = "off"
    end
    local server_env = { HSMP_LOBBY_MAP = chosen_map, HSMP_MASTER_URL = master, HSMP_SERVER_NAME = adv_name,
                         HSMP_SERVER_MODE = adv_mode, HSMP_LISTEN_HOST = "1", HSMP_REGION = (region ~= "") and region or nil }
    local sidecar_args = { "--server", "127.0.0.1:" .. port, "--state-dir", win(STATE_DIR), "--nick", tostring(settings.nick) }
    for _, a in ipairs(sc_args) do sidecar_args[#sidecar_args + 1] = a end
    for _, a in ipairs(MX.log_args()) do sidecar_args[#sidecar_args + 1] = a end
    reset_lobby_session(true, chosen_map)
    lobby.starting = os.clock()   -- latched until the lobby screen is up
    lobby.server_label = adv_name
    lobby.want_map = chosen_map
    Log("HOST: server env name=%s mode=%s region=%s master=%s", adv_name, adv_mode, region, master)
    MX.spawn_role("server", SERVER_EXE, server_args, { env = server_env })
    ExecuteWithDelay(500, function() MX.spawn_role("sidecar", SIDECAR_EXE, sidecar_args) end)

    -- Enter the lobby sub-screen while we wait for peers + START. The boot
    -- arena is (re)requested as a pick_arena command by the lobby poll until
    -- the server answers (it also covers the sidecar connecting late).
    ExecuteWithDelay(1000, function() enter_screen("lobby") end)
end

-- JOIN: boots the sidecar and sends the user to the LOBBY sub-screen.
local function spawn_sidecar_only(server, nick, map_name, label)
    -- Test hook: route this client through hsmp-tools netsim (latency/loss
    -- proxy) when the game was launched with HSMP_NETSIM_ADDR=host:port.
    local netsim = os.getenv("HSMP_NETSIM_ADDR")
    if netsim and netsim:match("^[%w%.%-]+:%d+$") then
        Log("JOIN via netsim %s (instead of %s)", netsim, server)
        server = netsim
    end
    if not (tostring(server):match("^[%w%.%-]+:%d+$") or tostring(server):match("^%[[%x:%.]+%]:%d+$")) then Log("JOIN refused: bad address %s", tostring(server)); return end
    local sc_args, sc_err = proc_args("sidecar")
    if not sc_args then
        MX.ipc_fail(sc_err)
        ExecuteWithDelay(100, function() enter_screen("lobby") end)
        return
    end
    MX.ipc_error = nil
    sc_args[#sc_args + 1] = "--mods-cache"; sc_args[#sc_args + 1] = MX.mods_cache()
    local sidecar_args = { "--server", server, "--state-dir", win(STATE_DIR), "--nick", tostring(nick or settings.nick) }
    for _, a in ipairs(sc_args) do sidecar_args[#sidecar_args + 1] = a end
    for _, a in ipairs(MX.log_args()) do sidecar_args[#sidecar_args + 1] = a end
    -- The server lists that may relay a NAT-traversal punch to this host when it does not
    -- answer directly (the sidecar tries direct first, then the punch).
    for _, u in ipairs(effective_master_urls()) do
        if type(u) == "string" and u:match("^https?://[%w%.%-%[%]:]+[%w%./%-_]*$") then
            sidecar_args[#sidecar_args + 1] = "--master"; sidecar_args[#sidecar_args + 1] = u
        end
    end
    -- The advertised map is a display hint until the session snapshot reports the
    -- server's arena; nothing ever travels to it.
    reset_lobby_session(false, (map_name and map_name ~= "") and map_name or settings.lobby_map)
    lobby.starting = os.clock()   -- latched until the lobby screen is up
    lobby.server_label = label or server
    lobby.server_addr = server
    Log("JOIN: %s", server)
    MX.spawn_role("sidecar", SIDECAR_EXE, sidecar_args)
    ExecuteWithDelay(1000, function() enter_screen("lobby") end)
end

-- --- lobby helpers --------------------------------------------------------

-- The session state comes from the typed records the sidecar copies into shared
-- memory (shared/hsmp_session.lua): `link` (status, my peer id, admin, metrics) and
-- `session` (the server's snapshot). No files, no JSON.
function MX.link() return MX.HS and MX.HS.link() or nil end
function MX.view() return MX.HS and MX.HS.view() or nil end
-- The Director's return_to_lobby bus key: true once per new seq. The seq present at load
-- is a leftover (seeded below), never replayed.
function MX.return_to_lobby_new()
    local ipc = rawget(_G, "HSMP_IPC")
    local t = ipc and ipc.bus_table and ipc.bus_table("return_to_lobby") or nil
    local seq = type(t) == "table" and tonumber(t.seq) or 0
    if MX.rtl_seq == nil then MX.rtl_seq = seq; return false end
    if seq ~= 0 and seq ~= MX.rtl_seq then MX.rtl_seq = seq; return true end
    return false
end
MX.return_to_lobby_new()

-- The lobby players: the session roster's connected seats (me included), with the
-- server-measured RTT from the peer directory; before the first snapshot, the
-- peer directory itself.
local function read_sidecar_peers()
    local ipc = rawget(_G, "HSMP_IPC")
    local dir = (ipc and ipc.peer_dir) and ipc.peer_dir() or { by_id = {}, list = {} }
    local function rtt(id)
        local e = dir.by_id[id]
        local r = e and tonumber(e.rtt_ms)
        return (r and r > 0) and r or nil
    end
    local out = {}
    local v = MX.view()
    if v then
        for _, r in ipairs(v.rows) do
            if r.connected and r.peer_id ~= 0 then
                out[#out + 1] = { id = r.peer_id, nick = tostring(r.nick ~= "" and r.nick or ("P" .. r.peer_id)), ping = rtt(r.peer_id) }
            end
        end
        return out
    end
    for _, e in ipairs(dir.list) do
        if tonumber(e.id) then out[#out + 1] = { id = tonumber(e.id), nick = tostring(e.nick or "?"), ping = rtt(tonumber(e.id)) } end
    end
    return out
end

local function read_my_peer_id()
    local l = MX.link()
    return l and l.my_peer_id or 0
end

-- The sidecar status name ("connecting", "connected", ..., "ended"); nil = no link
-- record (no session).
local function read_sidecar_status()
    local l = MX.link()
    return l and MX.HS.status_name(l) or nil
end

-- The Menu's commands (see CTL above): one typed `command` record each.
local function ctl_write(rec)   -- a G2S command record
    local ipc = rawget(_G, "HSMP_IPC")
    return ipc ~= nil and ipc.send("command", rec) ~= nil
end
-- Flush held records once connected. Cheap: one status read only while something is held.
function CTL.flush()
    if #CTL.hold == 0 or read_sidecar_status() ~= "connected" then return end
    local held = CTL.hold
    CTL.hold = {}
    for _, rec in ipairs(held) do ctl_write(rec) end
    Log("commands: %d held command(s) sent after connect", #held)
end
send_command = function(rec)
    if read_sidecar_status() ~= "connected" then
        if #CTL.hold >= 32 then table.remove(CTL.hold, 1) end
        CTL.hold[#CTL.hold + 1] = rec
        return
    end
    CTL.flush()
    ctl_write(rec)
end

-- nil = no link record yet
local function read_is_admin()
    local l = MX.link()
    if not l then return nil end
    return l.is_admin == true
end

-- The legacy match-state view (state, arena, countdown, best_of, ready peer ids)
-- from the session snapshot; nil before the first snapshot.
local function read_match_state()
    local v = MX.view()
    if not v then return nil end
    return { state = v.state, arena = v.arena, countdown = v.countdown_s or 0, best_of = v.best_of, ready = v.ready,
             mode = v.config and v.config.mode }
end

-- The server's arena from the session snapshot, as a MAP_PRESETS path when it is one
-- ("Map_Arena_Pit", "/Game/Maps/Arenas/Map_Arena_Pit/Map_Arena_Pit" or
-- "...Map_Arena_Pit.Map_Arena_Pit" all give "Map_Arena_Pit"); otherwise the
-- raw value. nil when the server has not reported an arena ("" / "default").
local function server_arena(match_st)
    local a = match_st and match_st.arena
    if not a or a == "" or a == "default" then return nil end
    local short = a:match("([^/%.]+)$") or a
    for _, m in ipairs(MAP_PRESETS) do
        if m.path == short or m.path == a then return m.path end
    end
    return a
end

local function is_peer_ready(pid, match_st)
    if not match_st or not match_st.ready then return false end
    for _, r in ipairs(match_st.ready) do if r == pid then return true end end
    return false
end

-- The listen host's reachability: the server's NET_STATUS notice (code 9) to its owner,
-- args { state, port, public address, NAT kind } (server/src/nat). The newest one wins.
MX.net_status = nil
local function net_status_pump()
    local ipc = rawget(_G, "HSMP_IPC")
    for _, e in ipairs((ipc and ipc.events and ipc.events("notice")) or {}) do
        local n = e.data or e
        if type(n) == "table" and n.code == 9 then
            local a = n.args or {}
            MX.net_status = { state = a[1] or "", port = tonumber(a[2]), public = a[3] or "", kind = a[4] or "" }
            Log("NET_STATUS: port map %s, port %s, public %s, nat %s", tostring(a[1]), tostring(a[2]), tostring(a[3]), tostring(a[4]))
        end
    end
end

-- The host lobby's note: longest first (set_text_fit), and its colour.
local function host_net_note(bind_port)
    local n = MX.net_status
    if not n then
        return { "Checking whether players outside your network can reach you...", "Checking your router..." }, Kit.C.dim
    end
    local p = n.port or bind_port
    local names = { upnp = "UPnP", pcp = "PCP", natpmp = "NAT-PMP" }
    if names[n.state] then
        return { string.format("Router port opened automatically (%s): players outside your network can join on UDP %d", names[n.state], p),
                 string.format("Router port opened automatically (%s)", names[n.state]) }, Kit.C.good
    elseif n.state == "open" then
        return { string.format("This PC has a public address: players can reach UDP %d if your firewall allows it", p),
                 "Public address: reachable if your firewall allows it" }, Kit.C.good
    elseif n.state == "double" then
        return { "Router port opened, but your router is behind another NAT (your provider's?): outside players may not reach you",
                 "Router port opened, but another NAT is in front of it" }, Kit.C.warn
    elseif n.state == "trying" then
        return { "Opening your router port...", "Opening router port..." }, Kit.C.dim
    end
    local punch = n.kind == "cone+punch"
    local tail = punch and " (joiners will try NAT traversal)" or ""
    if n.state == "off" then
        return { string.format("Automatic port forwarding is off: forward UDP %d to this PC for players outside your network%s", p, tail),
                 string.format("Forward UDP %d to this PC for internet players", p) }, Kit.C.warn
    end
    return { string.format("Couldn't open your router port automatically: players outside your network may not reach you. Forward UDP %d to this PC.%s", p, tail),
             string.format("Couldn't open your router port automatically. Forward UDP %d to this PC.", p),
             string.format("Forward UDP %d to this PC for internet players", p) }, Kit.C.warn
end

-- SERVER chat replies (from_peer 0): commands.lua infers refusals from them.
-- The S2G `chat_in` records, accumulated here (bounded) so the callers keep their
-- "position" semantics (a count of messages seen).
CTL.chat = { lines = {}, base = 0 }   -- base = messages dropped from the front
local function chat_pump()
    local ipc = rawget(_G, "HSMP_IPC")
    local c = CTL.chat
    for _, e in ipairs((ipc and ipc.events and ipc.events("chat_in")) or {}) do
        if type(e.data) == "table" then c.lines[#c.lines + 1] = e.data end
    end
    while #c.lines > 256 do table.remove(c.lines, 1); c.base = c.base + 1 end
end
local function chat_log_size()
    chat_pump()
    return CTL.chat.base + #CTL.chat.lines
end
local function server_chat_lines(pos)
    chat_pump()
    local c = CTL.chat
    local from = math.max(0, (pos or 0) - c.base)
    local out = {}
    for i = from + 1, #c.lines do
        local m = c.lines[i]
        if m.from_peer == 0 and m.text ~= "" then out[#out + 1] = m.text end
    end
    return out
end

local function remote_peer_count()
    local my_pid, n = read_my_peer_id(), 0
    for _, p in ipairs(read_sidecar_peers()) do if p.id ~= my_pid then n = n + 1 end end
    return n
end

-- The session snapshot view (shared/hsmp_session.lua): roster admin roles + the server's config.
local function read_session()
    return MX.view()
end

-- The server's kit rules: the kit_rules slot (sidecar), else the session snapshot's config.
local Classes
local function server_kit_rules(sess)
    local r = Classes and Classes.server_rules and Classes.server_rules() or nil
    if r then return r end
    sess = sess or read_session()
    local c = type(sess) == "table" and type(sess.config) == "table" and sess.config or nil
    if c and tonumber(c.kit_mode) then return { mode = c.kit_mode, budget = tonumber(c.kit_budget) or 0 } end
    return nil
end

-- Is `pid` the server's admin? (roster admin_role), nil = unknown.
local function admin_of(pid, sess)
    sess = sess or read_session()
    if type(sess) ~= "table" or type(sess.rows) ~= "table" then return nil end
    for _, r in ipairs(sess.rows) do
        if r.peer_id == pid and r.connected then
            return r.admin_role >= 2   -- ADMIN / OWNER
        end
    end
    return nil
end

-- A server has an admin only when its owner (listen host) or a configured
-- admin is connected. false = no admin here (players start the match by
-- READY), nil = unknown (no session snapshot yet).
local function server_has_admin(sess)
    if type(sess) ~= "table" or type(sess.rows) ~= "table" then return nil end
    for _, r in ipairs(sess.rows) do
        if r.connected and r.admin_role >= 2 then   -- ADMIN / OWNER
            return true
        end
    end
    return false
end

-- The no-admin auto start is armed (lobby with a deadline): seconds left, else nil.
local function auto_start_in(sess)
    if type(sess) ~= "table" or sess.phase_name ~= "lobby" then return nil end
    local ms = tonumber(sess.deadline_in_ms)
    if not ms then return nil end
    return math.max(0, math.ceil(ms / 1000))
end

-- --- commands + travel requests -------------------------------------------------------

local function now_ms() return os.clock() * 1000 end

Cmd = load_module("commands")
if Cmd then
    Cmd.init({
        state_dir = STATE_DIR, log = Log, ev = ev, net = load_module("hsmp_net", true),
        now = function() return os.clock() end,
        send_cmd = send_command,
        chat_size = chat_log_size, chat_lines = server_chat_lines,
        my_peer_id = read_my_peer_id,
        server_arena = function() return server_arena(read_match_state()) end,
        peer_ids = function() local o = {}; for _, p in ipairs(read_sidecar_peers()) do o[#o + 1] = p.id end; return o end,
        admin_of = function(pid) return admin_of(pid) end,
        kit_rules = function() return server_kit_rules() end,
        on_resend = function(c)
            if c.kind == "start" then Log("START: no state change after %ds - resending once", Cmd.KINDS.start.resend_s) end
        end,
        on_result = function(c)
            if c.kind == "start" and c.state == "refused" then
                Log("START: server did not start the match (reply: %s)", tostring(c.reason))
            end
            if state.screen_active == "lobby" then pcall(refresh_screen) end
        end,
    })
end

-- cmd_send / cmd_status: the one entry point every host/player action uses
-- (kinds: pick_arena{arena}, best_of{n}, start, abort, ready{value},
-- kit_rules{mode,budget}, kick{peer}, promote{peer}).
-- Returns the command id (nil only if commands.lua is missing; the line is
-- then still sent fire-and-forget so the menu keeps working).
function cmd_send(kind, args, via)
    if Cmd then return Cmd.send(kind, args, via) end
    Log("commands.lua missing: %s not sent", tostring(kind))
    return nil
end
function cmd_status(id) return (Cmd and id) and Cmd.status(id) or nil end

-- COMPATIBILITY SHIM (docs/development/subsystems/director-contract.md). The
-- Director (HSMPMatch) owns every level change, so the shim is OPT-IN: it is
-- loaded only with HSMP_LEGACY_TRAVEL=1 and logs loudly whenever it is armed
-- or used. To remove it, delete legacy_travel.lua and its check_travel
-- allow-list entry; nothing else changes (an unacked request is then refused,
-- logged "DIRECTOR MISSING and no legacy shim - NOT travelling").
local LEGACY_TRAVEL_ON = os.getenv("HSMP_LEGACY_TRAVEL") == "1"
if LEGACY_TRAVEL_ON then
    Legacy = load_module("legacy_travel", true)
    if Legacy then
        Log("!!! LEGACY TRAVEL SHIM ENABLED (HSMP_LEGACY_TRAVEL=1): the menu travels ITSELF when the Director does not ack - debug only !!!")
    else
        Log("!!! HSMP_LEGACY_TRAVEL=1 but legacy_travel.lua is gone - the Director is the only travel path")
    end
end
if Legacy then
    Legacy.init({ state_dir = STATE_DIR, log = Log, state = state, remote_peers = remote_peer_count,
                  ev = ev, UEHelpers = UEHelpers })
end

-- Runs only when the Director did not acknowledge a travel request (travel.lua).
local function legacy_fallback(req)
    Log("!!! LEGACY TRAVEL USED (HSMP_LEGACY_TRAVEL=1): #%d want=%s arena=%s - the Director did not ack; this is NOT the product path !!!",
        req.seq or 0, tostring(req.want), tostring(req.arena))
    if req.want == "arena" then
        if state.injected and state.screen_active then exit_screen() end
        ExecuteWithDelay(100, function() Legacy.travel(req) end)
    else
        Legacy.travel(req)
    end
end

Travel = load_module("travel")
if Travel then
    Travel.init({ state_dir = STATE_DIR, log = Log, now_ms = now_ms,
                  world_gen = function() return menu_world_gen end,
                  delay = function(ms, fn) ExecuteWithDelay(ms, fn) end,
                  fallback = Legacy and legacy_fallback or nil })
end

-- GI single-player values: restored by the Director on TravelMenu. Only the
-- legacy shim restores them, and only while no Director is running.
local function legacy_gi_restore(reason)
    if Legacy and not (Travel and Travel.director_alive()) then Legacy.gi_restore_backup(reason) end
end

-- The server left the lobby (countdown / live): ask the Director to travel to
-- the SERVER's arena. The menu never chooses the arena: with no arena reported
-- nothing is requested, and a refused request is retried after 3 s at most.
local TRAVEL_RETRY_S = 3
local function begin_combat_load(match_st)
    if lobby.launched then return end
    if lobby.travel_retry_t and os.clock() - lobby.travel_retry_t < TRAVEL_RETRY_S then return end
    local srv = server_arena(match_st)
    if not srv then
        if not lobby.no_arena_logged then
            lobby.no_arena_logged = true
            Log("match %s but the server reported no arena - not travelling", tostring(match_st and match_st.state))
        end
        lobby.travel_note = "THE SERVER REPORTED NO ARENA - WAITING"
        return
    end
    if not Travel then Log("travel.lua missing - cannot request travel"); return end
    lobby.launched = true
    lobby.travel_retry_t = nil
    lobby.travel_note = "LOADING " .. map_display_name(srv):upper() .. "..."
    Log("loading server arena %s (local pick %s) - travel requested from the Director",
        srv, tostring(lobby.want_map or lobby.chosen_map))
    Travel.request("arena", srv, "match_" .. tostring(match_st.state), function(res)
        if res.status == "accepted" then
            if state.injected and state.screen_active == "lobby" then exit_screen() end
        elseif res.status == "refused" then
            lobby.launched = false
            lobby.travel_retry_t = os.clock()
            lobby.travel_note = "TRAVEL REFUSED: " .. tostring(res.reason or "?")
            if state.screen_active == "lobby" then pcall(refresh_screen) end
        end
        -- "legacy": legacy_fallback already left the screen; "world_changed": the world is gone
    end)
end

-- --- UMG widget helpers ---------------------------------------------------


-- --- fade-in --------------------------------------------------------------

local function fade_in_all(widgets, ms)
    for _, w in ipairs(widgets) do
        pcall(function() w.button:SetRenderOpacity(0.0) end)
    end
    local t0 = os.clock() * 1000
    local gen = menu_world_gen
    LoopAsync(16, function()
        if gen ~= menu_world_gen then return true end   -- widgets freed with their world
        local t = (os.clock() * 1000 - t0) / ms
        if t >= 1 then
            for _, w in ipairs(widgets) do pcall(function() w.button:SetRenderOpacity(1.0) end) end
            return true
        end
        for _, w in ipairs(widgets) do pcall(function() w.button:SetRenderOpacity(t) end) end
        return false
    end)
end

-- --- native + injection discovery ----------------------------------------

local function find_menu()
    local list = FindAllOf("UI_Startup_Menu_C"); if not list then return nil end
    for _, w in pairs(list) do
        if w and w:IsValid() then
            local in_vp = false; pcall(function() in_vp = w:IsInViewport() end)
            if in_vp then return w end
        end
    end
    return nil
end

local function discover_native(menu)
    local wt = menu.WidgetTree
    local canvas = wt and wt.RootWidget
    if not canvas or not canvas:IsValid() then return false end

    state.menu = menu; state.wt = wt; state.canvas = canvas

    local anchors = {}
    for _, n in ipairs({ "Button_0","Button_1","Button_2","Button_3","Button_4" }) do
        local b = U.find_child(canvas, n); if b then table.insert(anchors, { name = n, btn = b }) end
    end
    if #anchors == 0 then return false end

    local best_r, best_i = 0, 1
    local nmin_x, nmin_y, nmax_x, nmax_y = math.huge, math.huge, -math.huge, -math.huge
    for i, a in ipairs(anchors) do
        local x, y, w, h = U.slot_rect(a.btn)
        if w and h and h > 0 then
            if (w/h) > best_r then best_r = w/h; best_i = i end
            nmin_x = math.min(nmin_x, x); nmin_y = math.min(nmin_y, y)
            nmax_x = math.max(nmax_x, x + w); nmax_y = math.max(nmax_y, y + h)
        end
    end
    state.style_src = anchors[best_i].btn
    state.text_src  = U.find_text_child(state.style_src)
    state.nmin_x = nmin_x; state.nmin_y = nmin_y
    state.nmax_x = nmax_x; state.nmax_y = nmax_y

    -- the native column's anchor (the ribbons copy it): x offsets are relative to it
    local ax, ay = 0.5, 0.5
    pcall(function()
        local a = state.style_src.Slot:GetAnchors()
        local x, y = a and a.Minimum and a.Minimum.X, a and a.Minimum and a.Minimum.Y
        if type(x) == "number" and x >= 0 and x <= 1 then ax = x end
        if type(y) == "number" and y >= 0 and y <= 1 then ay = y end
    end)
    state.anchor_x, state.anchor_y = ax, ay
    MX.measure_viewport()
    MX.ribbon_geometry()
    return true
end

-- Viewport + canvas metrics, fresh (shared/hsmp_ui_scale.lua; game thread,
-- this world's menu as the world context). Sets state.vw/vh/cw/ch/dpi.
function MX.measure_viewport()
    local M
    if UIS and state.menu then
        local ok, r = pcall(UIS.measure, UEHelpers, state.menu)
        if ok and type(r) == "table" then M = r end
    end
    if not M then
        -- no shared lib: the reference size, the layout library's DPI
        local dpi
        pcall(function()
            local wll = StaticFindObject("/Script/UMG.Default__WidgetLayoutLibrary")
            if wll and wll:IsValid() and state.menu then dpi = wll:GetViewportScale(state.menu) end
        end)
        if type(dpi) ~= "number" or dpi <= 0.05 then dpi = 1 end
        M = { vw = 1920 * dpi, vh = 1080 * dpi, cw = 1920, ch = 1080, dpi = dpi, s = 1, m = 24, src = "no-lib" }
    end
    state.vw, state.vh, state.cw, state.ch, state.dpi = M.vw, M.vh, M.cw, M.ch, M.dpi
    state.metrics = M
    return M
end

-- Top-ribbon size from the native buttons, shrunk so the column ends inside
-- the canvas's right safe margin and its 5 ribbons fit the safe height
-- (canvas units; the column shares the native anchor state.anchor_x/y).
function MX.ribbon_geometry()
    local sw, sh = select(3, U.slot_rect(state.style_src))
    local rw = math.floor((sw or 469) * 1.10)
    local rh = math.floor((sh or 120) * 1.10)
    local M = state.metrics or {}
    local cw, ch = M.cw or 1920, M.ch or 1080
    local margin = UIS and UIS.margin(cw, ch, M.s or 1) or 15
    state.rib_margin = margin
    local right = cw * (1 - (state.anchor_x or 0.5))              -- canvas right edge, slot space
    local max_w = right - (state.nmax_x + 15) - margin
    local max_h = (ch - 2 * margin) / (5 - 4 * 0.18)               -- 5 ribbons, overlapped 18 %
    local shrink = 1
    if rw > max_w and max_w > 0 then shrink = max_w / rw end
    if rh * shrink > max_h and max_h > 0 then shrink = max_h / rh end
    if shrink < 1 then rw = math.floor(rw * shrink); rh = math.floor(rh * shrink) end
    state.rib_w = rw; state.rib_h = rh
    state.v_gap = math.floor(-rh * 0.18)
end

-- One native-styled ribbon (UButton + TextBlock). The widget names carry the
-- menu world generation, so a re-injection never reuses a live name.
local function build_native_button(label, cb, name_suffix)
    local tag = name_suffix .. "_g" .. menu_world_gen
    local btn = U.construct("/Script/UMG.Button", state.wt, "HSMPBtn_" .. tag)
    if not btn then return nil end
    U.clone_button_look(btn, state.style_src)
    pcall(function() btn.ContentPadding = { Left=0, Top=0, Right=0, Bottom=0 } end)
    pcall(function() btn.HorizontalAlignment = 2 end)
    pcall(function() btn.VerticalAlignment   = 2 end)
    local tb = U.construct("/Script/UMG.TextBlock", btn, "HSMPLbl_" .. tag)
    if tb then
        pcall(function() tb:SetText(FText(label)) end)
        if state.text_src then U.clone_text_look(tb, state.text_src) end
        pcall(function() tb:SetJustification(1) end)
        pcall(function() tb.Justification = 1 end)
        pcall(function() tb.AutoWrapText = false end)
        pcall(function() btn:SetContent(tb) end)
    end
    return { button = btn, text = tb, cb = cb, prev = false }
end

local function add_to_canvas(entry, x, y, w, h)
    local slot
    pcall(function() slot = state.canvas:AddChildToCanvas(entry.button) end)
    if slot and slot:IsValid() then
        U.copy_slot_anchoring(state.style_src, slot)
        pcall(function()
            slot:SetPosition({ X = x, Y = y })
            slot:SetSize({ X = w, Y = h })
        end)
        entry.slot = slot
    end
end

-- The main menu's "the game crashed last time" line (diag.lua), under the ribbon column.
-- Built once the menu is injected; shown only while no sub-screen is open. The ref is
-- forgotten with the other widgets on a world change (forget_widgets).
function MX.crash_note_sync()
    local Dg = MX.Diag
    if not (Dg and Dg.crashed_last_time() and state.injected and state.canvas and state.wt) then return end
    local n = state.crash_note
    if not n then
        local tb = U.construct("/Script/UMG.TextBlock", state.wt, "HSMP_CrashNote_" .. tostring(menu_world_gen))
        if not tb then return end
        if state.text_src then U.clone_text_look(tb, state.text_src) end
        pcall(function() tb:SetText(FText(Dg.CRASH_NOTE)) end)
        pcall(function() tb.AutoWrapText = true end)
        pcall(function() local f = tb.Font; if f then f.Size = 18; tb:SetFont(f) end end)
        pcall(function() tb:SetColorAndOpacity({ SpecifiedColor = { R = 0.95, G = 0.75, B = 0.3, A = 1 }, ColorUseRule = 0 }) end)
        local r = MX.top_ribbon_rects()[5]
        n = { button = tb, shown = nil }
        add_to_canvas(n, r[1] + math.floor(r[3] * 0.08), r[2] + r[4] + 4, math.floor(r[3] * 0.9), 64)
        state.crash_note = n
        Log("crash note shown (previous run %s crashed)", tostring(Dg.prev_id))
    end
    local want = state.screen_active == nil
    if n.shown ~= want then
        n.shown = want
        U.set_vis(n.button, want and 3 or 1)
    end
end

-- --- top-level ribbon injection -------------------------------------------

local TOP_LABELS = { "HOST GAME", "SERVER BROWSER", "SETTINGS", "CHARACTER", "QUIT MP" }
local SCREEN_RIBBON = { lobby = 1, classes = 1, browser = 2, settings = 3, character = 4 }
local Browser   -- browser.lua module, loaded below

local quit_desktop

-- HOST GAME / JOIN are not re-entrant. The ribbons stay live for the 1 s
-- before the lobby screen opens, and a double click would start two sidecars
-- on the same state. lobby.starting latches until the lobby is up (or
-- START_LATCH_S passed: the delayed enter_screen can be skipped in a level
-- change). A session that is still held (RECONNECT pending, or the lobby
-- left for the main menu) is never replaced by HOST: it goes back to it.
MX.START_LATCH_S = 5
function MX.start_latched()
    return lobby.starting ~= nil and os.clock() - lobby.starting < MX.START_LATCH_S
end
function MX.host_click()
    if lobby.active then
        Log("HOST GAME: an MP session is still held - back to its lobby (no second session)")
        enter_screen("lobby")
        return
    end
    if MX.build_block then   -- the browser shows the launcher hint
        Log("HOST GAME refused: %s", MX.build_block)
        enter_screen("browser")
        return
    end
    if MX.start_latched() then Log("HOST GAME: a session is already starting - ignored"); return end
    spawn_server_and_sidecar()
end
-- MX.join_click(addr, map, label): defined after session_teardown below

-- While a session is held the HOST GAME ribbon says so (and goes back
-- to it): RECONNECTING... while the link is down, BACK TO LOBBY once it is up.
-- Written only on a change (the ribbon entry remembers its label).
function MX.update_host_label(sstat)
    if not state.injected then return end
    local e = state.ribbons and state.ribbons[1]
    if not e or not e.text then return end
    local want = TOP_LABELS[1]
    if lobby.active then want = (sstat == "connected") and "BACK TO LOBBY" or "RECONNECTING..." end
    if MX.build_block and not lobby.active then want = "UPDATE HSMP" end
    if e.label_cur == want or (e.label_cur == nil and want == TOP_LABELS[1]) then return end
    e.label_cur = want
    pcall(function() e.text:SetText(FText(want)) end)
end

local function top_cb(idx)
    if idx == 1 then
        return function() MX.host_click() end
    elseif idx == 2 then
        return function() enter_screen("browser") end
    elseif idx == 3 then
        return function() enter_screen("settings") end
    elseif idx == 4 then
        return function() enter_screen("character") end
    elseif idx == 5 then
        return function() quit_desktop() end
    end
end

-- Keyboard focus ring on the ribbon column (4 thin Borders, same anchoring as the ribbons).
local function build_top_ring()
    state.top_ring = {}
    for i = 1, 4 do
        local b = U.construct("/Script/UMG.Border", state.wt, string.format("HSMPTopRing_%d_g%d", i, menu_world_gen))
        if b then
            pcall(function() b:SetBrushColor((Kit and Kit.C.focus) or { R = 1, G = 0.82, B = 0.36, A = 1 }) end)
            pcall(function() b:SetVisibility(1) end)
            local e = { button = b }
            add_to_canvas(e, 0, 0, 1, 1)
            pcall(function() e.slot:SetZOrder(60) end)
            state.top_ring[i] = e
        end
    end
end

-- The ribbon column's rects (slot space of the native anchor): right of the
-- native column, vertically centred on it.
function MX.top_ribbon_rects()
    local total_h = 5 * state.rib_h + 4 * state.v_gap
    local col_x = state.nmax_x + 15
    local y0 = (state.nmin_y + state.nmax_y) / 2 - total_h / 2
    -- kept inside the canvas' safe height (slot space of the native anchor)
    local M = state.metrics
    if M and M.ch then
        local m = state.rib_margin or 0
        local top = -M.ch * (state.anchor_y or 0.5) + m
        local bottom = M.ch * (1 - (state.anchor_y or 0.5)) - m
        if y0 + total_h > bottom then y0 = bottom - total_h end
        if y0 < top then y0 = top end
    end
    local out = {}
    for i = 1, 5 do out[i] = { col_x, y0 + (i - 1) * (state.rib_h + state.v_gap), state.rib_w, state.rib_h } end
    return out
end

local function inject_top_ribbons()
    local rects = MX.top_ribbon_rects()
    state.ribbons = {}
    for i = 1, 5 do
        local r = rects[i]
        local e = build_native_button(TOP_LABELS[i], top_cb(i), "top" .. i)
        if e then
            add_to_canvas(e, r[1], r[2], r[3], r[4])
            e.rect = r
            state.ribbons[i] = e
            Log("top ribbon %d: %s @ (%.0f, %.0f) %dx%d", i, TOP_LABELS[i], r[1], r[2], r[3], r[4])
        end
    end
    build_top_ring()
end

-- Show the ring on ribbon i (nil hides it). The ribbon art has a soft margin,
-- so the ring sits a little inside the slot.
local function top_focus(i)
    state.top_focus = i
    local ring = state.top_ring
    if not ring or not state.injected then return end
    local e = i and state.ribbons[i]
    if not e or not e.rect then
        for _, r in ipairs(ring) do U.set_vis(r.button, 1) end
        return
    end
    local x, y, w, h = e.rect[1], e.rect[2], e.rect[3], e.rect[4]
    local ix, iy = math.floor(w * 0.06), math.floor(h * 0.20)
    x, y, w, h = x + ix, y + iy, w - 2 * ix, h - 2 * iy
    local t = 4
    local rects = { { x, y - t, w, t }, { x, y + h, w, t }, { x - t, y - t, t, h + 2 * t }, { x + w, y - t, t, h + 2 * t } }
    for k, r in ipairs(ring) do
        pcall(function()
            r.slot:SetPosition({ X = rects[k][1], Y = rects[k][2] })
            r.slot:SetSize({ X = rects[k][3], Y = rects[k][4] })
        end)
        U.set_vis(r.button, 3)
    end
end

-- --- sub-screen framework ------------------------------------------------
-- Hide the native Button + Button_0..4 and the 5 top ribbons; build fresh
-- widgets on the same canvas with a fade-in; BACK removes them + un-hides.

local function hide_top_and_natives()
    state.native_hidden = {}
    for _, n in ipairs(state.native_names) do
        local w = U.find_child(state.canvas, n)
        if w then U.set_vis(w, 1); table.insert(state.native_hidden, w) end
    end
    for _, r in ipairs(state.ribbons) do U.set_vis(r.button, 1) end
    for _, r in ipairs(state.top_ring or {}) do U.set_vis(r.button, 1) end
end

local function show_top_and_natives()
    for _, w in ipairs(state.native_hidden) do U.set_vis(w, 0) end
    state.native_hidden = {}
    for _, r in ipairs(state.ribbons) do U.set_vis(r.button, 0) end
end

-- Remove sub-screen widgets. UWidget doesn't expose RemoveFromParent on
-- CanvasPanelSlot children directly in UE4SS, so we collapse + null the ref.
local function destroy_screen_widgets()
    for _, w in ipairs(state.screen_widgets) do
        U.set_vis(w.button, 1)       -- collapse
        -- Also try RemoveChild if the canvas exposes it.
        pcall(function() state.canvas:RemoveChild(w.button) end)
    end
    state.screen_widgets = {}
    drop_screen_refs()
end

-- --- screen modules -----------------------------------------------------------------

-- ui_kit (required): every screen is built with it.
if Kit then
    Kit.init({
        state = state, Log = Log, construct = U.construct, clone_text_look = U.clone_text_look,
        world_gen = function() return menu_world_gen end,
        uis = UIS,
        -- fresh viewport metrics for a build (only ever called on this world's injected menu)
        measure = function() if UIS and state.menu then return MX.measure_viewport() end return nil end,
    })
    table.insert(forget_hooks, function() Kit.forget() end)
else
    Log("ERROR: ui_kit.lua missing - the HSMP menu screens are unavailable (reinstall HSMPMenu)")
end
if Settings then table.insert(forget_hooks, function() Settings.forget() end) end

-- Server browser lives in browser.lua (table, sort, paging, filter chips, ping).
do
    local mod = Kit and load_module("browser")
    if mod then
        Browser = mod
        Browser.init({
            state = state, Log = Log, STATE_DIR = STATE_DIR, MASTER_URL = MASTER_URL,
            MASTER_URLS = MASTER_URLS, QUERY_EXE = QUERY_EXE,
            LAN = cfg_get("lan_discovery", "1") ~= "0",
            LAN_PORTS = cfg_get("lan_ports", "7777-7786"),
            local_master = LocalMaster and function() return LocalMaster.status() end or nil,
            kit = Kit, map_display_name = map_display_name,
            my_region = function() return settings.region or "" end,
            join = function(addr, map, label) MX.join_click(addr, map, label) end,
            build = MX.Build, build_id = function() return MX.build_id end,
            blocked = function() return MX.build_block end,
            exit_screen = function() exit_screen() end,
        })
        table.insert(forget_hooks, function() Browser.forget() end)
    else
        Log("ERROR: browser.lua not available - no SERVER BROWSER screen")
    end
end

-- Classes / gear selection sub-screen lives in classes.lua (HSMP classes system).
do
    local mod = load_module("classes")
    if mod then
        Classes = mod
        Classes.attach({
            log = Log, state_dir = STATE_DIR, kit = Kit, json = J,
            viewport = function() return state.vw, state.vh end,
            refresh = function() refresh_screen() end,
            back = function() enter_screen("lobby") end,
            is_host = function() return lobby.is_host end,
            my_peer_id = read_my_peer_id,
            send_rules = function(mode, budget)
                lobby.rules_cmd = cmd_send("kit_rules", { mode = mode, budget = budget })
                lobby.note_cmd = lobby.rules_cmd
            end,
        })
        table.insert(forget_hooks, function() Classes.forget() end)
    else
        Log("classes.lua failed to load - no LOADOUT / CLASS screen")
    end
end

-- The GAME MODE screen (lobby -> MODE) lives in modes_ui.lua.
MX.ModeUI = load_module("modes_ui", true)
if MX.ModeUI then
    MX.ModeUI.attach({
        kit = Kit,
        cmd = function(kind, args) local id = cmd_send(kind, args); lobby.note_cmd = id; return id end,
        cmd_status = function(id) return cmd_status(id) end,
        config = function() local v = read_session(); return v and v.config or nil end,
        mode = function() return MX.HS and MX.HS.mode and MX.HS.mode() or nil end,
        roster = function() local v = read_session(); return v and v.rows or {} end,
        my_peer_id = function() return read_my_peer_id() end,
        is_admin = function() return lobby.admin_now and true or false end,
        in_lobby = function() local m = read_match_state(); return m == nil or m.state == "lobby" end,
        alive = function() return state.screen_active == "mode" end,
        back = function() enter_screen("lobby") end,
    })
    table.insert(forget_hooks, function() MX.ModeUI.forget() end)
end

-- Server mods: the consent, the warning screen and the progress (server_mods.lua).
do
    local mod = load_module("server_mods")
    if mod then
        MX.Mods = mod
        mod.attach({
            kit = Kit, log = Log, json = J, state_dir = STATE_DIR, ev = ev,
            ipc = function() return rawget(_G, "HSMP_IPC") end,
            policy = function() return settings.server_mods or "ask" end,
            server_label = function() return lobby.server_label or lobby.server_addr or "" end,
            decline = function(why) MX.mods_leave(why) end,
        })
        table.insert(forget_hooks, function() mod.forget() end)
    else
        Log("server_mods.lua failed to load - servers with mods cannot be joined")
    end
end

-- --- main.lua kit screens: character, lobby ------------------------------------------
-- Built once on enter; clicks and ticks re-render in place (state.screen_render).
-- The build table (ui) is dropped on screen exit / world change via forget_hooks.

local ui                    -- current kit build of a main.lua screen (character/lobby)
table.insert(forget_hooks, function() ui = nil end)

local function ui_alive() return Kit and ui and Kit.alive(ui) end

-- CHARACTER ----------------------------------------------------------------------

local STAT_ROWS = {
    { "str", "STRENGTH", "Raw power behind your swings" },
    { "agi", "AGILITY", "How fast you move and recover" },
    { "int", "INTELLIGENCE", "Learning and technique" },
    { "sta", "STAMINA", "How long you can keep fighting" },
}
local char_edit

local function char_dirty()
    if not char_edit then return false end
    for _, r in ipairs(STAT_ROWS) do if char_edit[r[1]] ~= character[r[1]] then return true end end
    return false
end

local function render_character()
    if not ui_alive() or not char_edit then return end
    for _, r in ipairs(STAT_ROWS) do Kit.chip_group_set(ui.w[r[1]], char_edit[r[1]], false) end
    local total = char_edit.str + char_edit.agi + char_edit.int + char_edit.sta
    Kit.set_text(ui.w.total, string.format("TOTAL  %d / 40", total))
    local d = char_dirty()
    if ui.msg_t and Kit.now() < ui.msg_t then
        -- the last result stays a moment
    elseif d then Kit.msg("Unsaved changes - SAVE & PUBLISH sends them to the server", Kit.C.warn)
    else Kit.msg("These values are sent to the server with your profile.", Kit.C.dim) end
    Kit.status(d and "UNSAVED CHANGES" or "SAVED", d and Kit.C.warn or Kit.C.dim, false)
end

local function character_back()
    if not ui_alive() then return end
    if char_dirty() then
        Kit.confirm("Discard your unsaved changes?", "DISCARD", function() char_edit = nil; exit_screen() end,
            { no_label = "KEEP EDITING" })
        return
    end
    char_edit = nil
    exit_screen()
end

local function build_character_kit()
    local L
    L, ui = Kit.frame("character", "CHARACTER", { w = 1500, h = 560 })
    -- a resize rebuild keeps the unsaved edits
    if not (Kit.relayout and char_edit) then
        char_edit = { str = character.str, agi = character.agi, int = character.int, sta = character.sta }
    end
    local u, F = L.u, L.F
    local rh = u(Kit.BH.chip)
    local lw = u(260)
    local y = L.top + u(Kit.SP.sm)
    local vals = {}
    for v = 0, 10 do vals[#vals + 1] = { v, tostring(v) } end
    for _, r in ipairs(STAT_ROWS) do
        local k = r[1]
        Kit.group(k)
        Kit.text(r[2], L.x0, y, lw - L.gap, rh, F(Kit.TS.body), 0, Kit.C.text)
        ui.w[k] = Kit.chip_group(vals, L.x0 + lw, y, L.iw - lw, rh, function(v)
            if not ui_alive() then return end
            char_edit[k] = v; ui.msg_t = nil; render_character()
        end, { fs = F(Kit.TS.label), gap = u(6), help = r[3] .. " (0-10)" })
        y = y + rh + u(Kit.SP.lg)
    end
    ui.w.total = Kit.text("", L.x0 + lw, y, L.iw - lw, u(30), F(Kit.TS.label), 0, Kit.C.dim)
    local acts = Kit.actions(L, nil, {
        { label = "BACK", key = "back", cb = character_back, help = "Leave without saving" },
        { label = "SAVE & PUBLISH", key = "save", style = "primary", help = "Save and send to the server", cb = function()
            if not ui_alive() then return end
            for _, r in ipairs(STAT_ROWS) do character[r[1]] = char_edit[r[1]] end
            if save_character() then
                ui.msg_t = Kit.now() + 4
                Kit.msg("Saved and published.", Kit.C.good)
            else
                Kit.msg("Could not write " .. CHARACTER_FILE, Kit.C.bad)
            end
            render_character()
        end },
    })
    ui.w.save, ui.w.back = acts.save, acts.back
    ui.on_back = character_back
    Kit.focus_default(ui.w.str.by_key[character.str])
    render_character()
end

-- LOBBY ---------------------------------------------------------------------------
-- The menu decides nothing here:
--  * every host/player action is a command (cmd_send) shown as pending, then
--    accepted / refused with the server's reason;
--  * the SERVER's arena (the session snapshot) is the selected tile for everyone; the
--    host's pick shows as pending until the server answers;
--  * the server's best-of and kit rules drive their chips;
--  * START is enabled only when the server would accept it, else the reason shows;
--  * level travel is requested from the Director (begin_combat_load), only
--    while the sidecar is connected.

local MAX_LOBBY_ROWS = 8
local NOTE_ACCEPTED_S = 4         -- an accepted command stays in the note line this long
local CONNECT_TIMEOUT_S = 15      -- "cannot reach the server" after this long without a connection
local CMD_WHAT = { pick_arena = "ARENA", best_of = "ROUNDS", start = "START", abort = "ABORT", ready = "READY",
                   kit_rules = "KIT RULES", kick = "KICK", promote = "MAKE HOST", game_mode = "MODE", teams = "TEAMS",
                   round_time = "ROUND CLOCK", set_team = "TEAM", set_option = "MODE OPTION" }
local KIT_MODE_NAMES = { [0] = "FREE", [1] = "CLASSES ONLY", [2] = "CUSTOM" }

local function host_pick_map(path, via)
    lobby.want_map = path
    settings.lobby_map = path             -- remembered as the next HOST's boot request only
    save_settings()
    lobby.map_cmd = cmd_send("pick_arena", { arena = path }, via) or true
    lobby.note_cmd = lobby.map_cmd
end

local function host_pick_best_of(n)
    settings.lobby_mode = "Best of " .. n
    save_settings()
    lobby.mode_cmd = cmd_send("best_of", { n = n })
    lobby.note_cmd = lobby.mode_cmd
    Log("rounds: best_of %d requested", n)
end

local function host_pick_rules(mode, budget)
    -- remembered for the next HOST (re-sent on connect, see MX.send_host_prefs)
    settings.host_kit_mode, settings.host_kit_budget = tonumber(mode) or -1, tonumber(budget) or 0
    save_settings()
    if Classes and Classes.request_rules then Classes.request_rules(mode, budget, true) end
    lobby.rules_cmd = cmd_send("kit_rules", { mode = mode, budget = budget })
    lobby.note_cmd = lobby.rules_cmd
end

-- The host's saved ROUNDS and KIT RULES picks: the boot passes only
-- HSMP_SERVER_MODE (a listing label), so the server starts at best_of 3 / its
-- default kit rules. Once per session, when our own server first answers
-- (connected + the session snapshot), the saved picks are sent as commands
-- next to the boot arena request, only where the server differs.
function MX.send_host_prefs(sstat, match_st)
    if lobby.prefs_sent or not lobby.is_host or sstat ~= "connected" or not match_st then return end
    if match_st.state ~= "lobby" then return end
    lobby.prefs_sent = true
    local n = tonumber(tostring(settings.lobby_mode or ""):match("(%d+)%s*$"))
    if n and match_st.best_of and match_st.best_of ~= n then
        Log("rounds: the saved BEST OF %d re-sent (server has %d)", n, match_st.best_of)
        lobby.mode_cmd = cmd_send("best_of", { n = n }) or lobby.mode_cmd
    end
    local km, kb = tonumber(settings.host_kit_mode), tonumber(settings.host_kit_budget) or 0
    if km and km >= 0 then
        local r = server_kit_rules()
        if not r or r.mode ~= km or (km == 2 and (r.budget or 0) ~= kb) then
            Log("kit rules: the saved mode %d budget %d re-sent", km, kb)
            if Classes and Classes.request_rules then Classes.request_rules(km, kb, true) end
            lobby.rules_cmd = cmd_send("kit_rules", { mode = km, budget = kb }) or lobby.rules_cmd
        end
    end
end

-- READY toggles relative to the server's view, or to a still-pending request.
local function set_ready(want, via)
    lobby.ready_cmd = cmd_send("ready", { value = want and true or false }, via)
    lobby.note_cmd = lobby.ready_cmd
end
local function toggle_ready()
    local want = not lobby.is_ready
    local st = cmd_status(lobby.ready_cmd)
    if st and st.state == "pending" then want = not st.args.value end
    set_ready(want)
end

local function cmd_pending(id)
    local st = cmd_status(id)
    return st ~= nil and st.state == "pending", st
end

local function peer_nick(id)
    for _, p in ipairs(read_sidecar_peers()) do if p.id == id then return p.nick end end
    return "#" .. tostring(id)
end

-- Note line text for a command: pending / accepted (for a few s) / refused + reason.
local function cmd_note(id)
    local st = cmd_status(id)
    if not st then return nil end
    local what = CMD_WHAT[st.kind] or st.kind:upper()
    local subject = (st.kind == "pick_arena" and map_display_name(st.args.arena):upper())
        or (st.kind == "best_of" and ("BEST OF " .. tostring(st.args.n)))
        or (st.kind == "ready" and (st.args.value and "ON" or "OFF"))
        or ((st.kind == "kick" or st.kind == "promote") and tostring(st.args.nick or peer_nick(st.args.peer)):upper())
        or (st.kind == "kit_rules" and ((KIT_MODE_NAMES[st.args.mode] or "?") .. (st.args.mode == 2 and (" " .. tostring(st.args.budget)) or "")))
        or nil
    local head = subject and (what .. " " .. subject) or what
    if st.state == "pending" then
        if st.kind == "start" then
            return (st.tries > 1 and "WAITING FOR SERVER... (START resent)" or "WAITING FOR SERVER..."), Kit.C.ok
        end
        return head .. ": WAITING FOR SERVER...", Kit.C.ok
    elseif st.state == "refused" then
        return what .. " REFUSED - " .. tostring(st.reason or "?"), Kit.C.bad
    elseif st.state == "accepted" and (st.done_age_s or 99) < NOTE_ACCEPTED_S then
        return head .. ": ACCEPTED", Kit.C.good
    elseif st.state == "superseded" and st.source ~= "local" and (st.done_age_s or 99) < NOTE_ACCEPTED_S then
        return head .. " DROPPED - " .. tostring(st.reason or "?"), Kit.C.warn
    end
    return nil
end

-- One read of every lobby input (500 ms poll).
local function lobby_snapshot()
    local S = {}
    S.peers    = read_sidecar_peers()
    S.my_pid   = read_my_peer_id()
    S.status   = read_sidecar_status()
    S.match    = read_match_state() or { state = "lobby", ready = {} }
    S.session  = read_session()
    local sa = read_is_admin()
    S.admin = (sa == true) or (sa == nil and (lobby.is_host or lobby.at_admin)) or false
    local lk = MX.link()   -- the sidecar's transport metrics (link record)
    S.rtt = lk and lk.metrics_wall_ms ~= 0 and ((lk.srtt_ms > 0 and lk.srtt_ms) or (lk.rtt_ms > 0 and lk.rtt_ms)) or nil
    S.rules = server_kit_rules(S.session)
    local cfgm = type(S.session) == "table" and type(S.session.config) == "table" and S.session.config.mode or nil
    S.mode = cfgm and MX.HS.code_name(rawget(_G, "HSMP_IPC"), "game_mode", cfgm) or nil
    S.nready = 0
    for _, p in ipairs(S.peers) do if is_peer_ready(p.id, S.match) then S.nready = S.nready + 1 end end
    -- No admin on this server (a dedicated server without configured
    -- admins, or the host dropped): READY players start the match themselves.
    S.no_admin = (not S.admin) and server_has_admin(S.session) == false
    S.auto_in = S.no_admin and auto_start_in(S.session) or nil
    return S
end

-- Can the host START now? ok, reason
local function start_check(S)
    if not S.admin then
        if S.no_admin then
            return false, string.format("No host here: the match starts when everyone is READY (%d/%d)", S.nready, #S.peers)
        end
        return false, string.format("The host starts the match (%d/%d ready)", S.nready, #S.peers)
    end
    if S.status ~= "connected" then return false, "Not connected to the server yet" end
    if S.match.state ~= "lobby" then return false, "A match is already running" end
    if #S.peers == 0 then return false, "Waiting for the server to list the players" end
    if #S.peers == 1 then return true end
    local missing, me_missing = {}, false
    for _, p in ipairs(S.peers) do
        if not is_peer_ready(p.id, S.match) then
            if p.id == S.my_pid then me_missing = true else missing[#missing + 1] = p.nick end
        end
    end
    if me_missing and #missing == 0 then return false, "Mark yourself READY first" end
    if #missing > 0 then
        local names = table.concat(missing, ", ", 1, math.min(3, #missing)) .. ((#missing > 3) and ", ..." or "")
        return false, string.format("Waiting for %s to be READY%s", names, me_missing and " (and you)" or "")
    end
    return true
end

-- The link record's reason line while still connecting, or nil.
local function lobby_link_reason()
    local l = MX.link()
    local r = l and l.reason
    if type(r) ~= "string" or r == "" then return nil end
    return r
end

local function lobby_status_line(S)
    local st = S.status
    if st == "connected" then
        return "CONNECTED" .. (S.rtt and string.format("   %d MS", math.floor(S.rtt + 0.5)) or ""), Kit.C.good, false
    elseif st == "reconnecting" then return "RECONNECTING...", Kit.C.warn, true
    elseif st == nil or st == "connecting" or st == "handshake" then
        -- the sidecar's NAT traversal line ("Connecting through your router...", or what the
        -- host must do when nothing gets through)
        local why = lobby_link_reason()
        if why then
            local blocked = why:find("blocks incoming", 1, true) ~= nil
            return why:upper(), blocked and Kit.C.bad or Kit.C.ok, not blocked
        end
        return "CONNECTING...", Kit.C.ok, true
    end
    return tostring(st):upper(), Kit.C.bad, false
end

local function render_lobby()
    if not ui_alive() then return end
    local w = ui.w
    if lobby.is_host and w.port_note then
        net_status_pump()
        local note, col = host_net_note(host_port())
        Kit.set_text_fit(w.port_note, note)
        Kit.set_color(w.port_note, col)
    end
    local S = lobby_snapshot()
    local admin = S.admin
    lobby.admin_now = admin

    local stl, stc, busy = lobby_status_line(S)
    Kit.status(stl, stc, busy)

    -- roster
    Kit.set_text(w.roster_head, string.format("PLAYERS  %d / %d", #S.peers, MAX_LOBBY_ROWS))
    Kit.set_text(w.tools_head, admin and "HOST TOOLS" or "")
    Kit.set_text(w.roster_sub, string.format("%d READY", S.nready))
    Kit.set_color(w.roster_sub, (#S.peers > 0 and S.nready == #S.peers) and Kit.C.good or Kit.C.dim)
    local sel_ok = false
    for i = 1, MAX_LOBBY_ROWS do
        local r = w.rows[i]
        local p = S.peers[i]
        r.peer = p and p.id or nil
        if p then
            local me = p.id == S.my_pid
            local rdy = is_peer_ready(p.id, S.match)
            local is_adm = admin_of(p.id, S.session)
            if is_adm == nil then is_adm = (me and admin) or false end
            local sel = lobby.sel_peer == p.id
            if sel then sel_ok = true end
            r.btn.skip_focus = false
            Kit.set_state(r.btn, sel, false)
            local base = sel and Kit.C.on_text or Kit.C.text
            Kit.set_text(r.name, p.nick .. (me and "  (YOU)" or "") .. (is_adm and "  HOST" or ""))
            Kit.set_color(r.name, sel and Kit.C.on_text or (me and Kit.C.head or Kit.C.text))
            Kit.set_text(r.ready, rdy and "READY" or "NOT READY")
            Kit.set_color(r.ready, sel and Kit.C.on_text or (rdy and Kit.C.good or Kit.C.dim))
            Kit.set_text(r.class, Classes and Classes.peer_class and Classes.peer_class(p.id) or "-")
            Kit.set_color(r.class, base)
            local ping = me and S.rtt or p.ping
            Kit.set_text(r.ping, ping and string.format("%d MS", math.floor(ping + 0.5)) or "-")
            Kit.set_color(r.ping, sel and Kit.C.on_text or ((ping and ping > 150) and Kit.C.warn or Kit.C.dim))
            local acts = admin and sel and not me
            Kit.show(r.kick, acts); Kit.show(r.promote, acts)
            if acts then
                Kit.set_pending(r.kick, cmd_pending(lobby.kick_cmd) and cmd_status(lobby.kick_cmd).args.peer == p.id)
                Kit.set_pending(r.promote, cmd_pending(lobby.promote_cmd) and cmd_status(lobby.promote_cmd).args.peer == p.id)
            end
        else
            r.btn.skip_focus = true
            Kit.set_state(r.btn, false, true)
            Kit.set_text(r.name, (i == #S.peers + 1) and "(free slot)" or "")
            Kit.set_color(r.name, Kit.C.dim)
            Kit.set_text(r.ready, ""); Kit.set_text(r.class, ""); Kit.set_text(r.ping, "")
            Kit.show(r.kick, false); Kit.show(r.promote, false)
        end
    end
    if lobby.sel_peer and not sel_ok then lobby.sel_peer = nil end
    Kit.show_text(w.no_peers, #S.peers == 0)
    Kit.set_text(w.no_peers, (S.status == "connected") and "Waiting for the server to list the players..."
        or "Waiting for the connection to the server...")

    -- arena tiles: selected = the server's arena; the host's unanswered pick = pending
    local srv = server_arena(S.match)
    local map_pend, map_st = cmd_pending(lobby.map_cmd)
    local pend_path = (admin and map_pend and map_st.args.arena ~= srv) and map_st.args.arena or nil
    for i, ref in ipairs(w.maps.chips) do
        local m = MAP_PRESETS[i]
        local on = (m.path == srv)
        Kit.tile_set(ref, { title = m.name:upper(), meta = m.spawns and (m.spawns .. " SPAWNS") or "",
            note = on and "SERVER ARENA" or ((pend_path == m.path) and "REQUESTED..." or ""),
            note_color = on and nil or Kit.C.ok, on = on, disabled = not admin })
        Kit.set_pending(ref, pend_path == m.path)
    end
    local srv_name = srv and map_display_name(srv):upper() or "NOT REPORTED YET"
    Kit.set_text(w.map_hint, admin and ("SERVER: " .. srv_name)
        or ((S.no_admin and "SET BY THE SERVER  -  " or "HOST PICKS  -  SERVER: ") .. srv_name))

    -- rounds: the server's best-of
    Kit.chip_group_set(w.modes, S.match.best_of, not admin)
    local mode_pend, mode_st = cmd_pending(lobby.mode_cmd)
    Kit.chip_group_pending(w.modes, (admin and mode_pend and mode_st.args.n ~= S.match.best_of) and mode_st.args.n or nil)
    Kit.set_text(w.mode_hint, S.match.best_of and ("SERVER: BEST OF " .. S.match.best_of) or "SERVER: -")

    -- kit rules: the server's rules; budget only for CUSTOM
    local rules = S.rules
    local r_pend, r_st = cmd_pending(lobby.rules_cmd)
    local want_mode = (r_pend and r_st.args.mode) or (rules and rules.mode)
    Kit.chip_group_set(w.kitmode, rules and rules.mode, not admin)
    Kit.chip_group_pending(w.kitmode, (admin and r_pend and (not rules or r_st.args.mode ~= rules.mode)) and r_st.args.mode or nil)
    Kit.chip_group_set(w.budget, (rules and rules.mode == 2) and rules.budget or nil,
        (not admin) or want_mode ~= 2)
    Kit.chip_group_pending(w.budget, (admin and r_pend and r_st.args.mode == 2 and (not rules or r_st.args.budget ~= rules.budget))
        and r_st.args.budget or nil)
    Kit.chip_group_reason(w.budget, (not admin) and "Only the host changes the match settings"
        or "The point budget only applies to CUSTOM kits - pick CUSTOM first")
    Kit.set_text(w.rules_hint, rules and ("SERVER: " .. (KIT_MODE_NAMES[rules.mode] or "?") .. ((rules.mode == 2) and (" " .. rules.budget) or ""))
        or "SERVER: -")
    if MX.ModeUI then
        local sess = S.session
        Kit.set_text(w.mode_line, MX.ModeUI.summary(MX.ModeUI.state(sess and sess.config, MX.HS and MX.HS.mode and MX.HS.mode(),
            sess and sess.rows, S.my_pid or 0)))
    else
        Kit.set_text(w.mode_line, "MODE  " .. (S.mode and S.mode:upper():gsub("_", " ") or "DUEL") .. "   (set by the server)")
    end

    -- your loadout
    if w.loadout_sum then Kit.set_text(w.loadout_sum, "YOUR KIT: " .. (Classes and Classes.my_summary and Classes.my_summary() or "-")) end

    -- actions
    local my_ready = is_peer_ready(S.my_pid, S.match)
    lobby.is_ready = my_ready
    local rdy_pend, rdy_st = cmd_pending(lobby.ready_cmd)
    if rdy_pend then
        Kit.set_label(w.ready, rdy_st.args.value and "SENDING READY..." or "SENDING UNREADY...")
    else
        Kit.set_label(w.ready, my_ready and "READY  [x]" or "MARK READY")
    end
    Kit.set_state(w.ready, my_ready, S.status ~= "connected")
    Kit.set_pending(w.ready, rdy_pend)
    w.ready.help = my_ready and "You are READY - press again to cancel" or "Tell everyone you are ready to fight"
    local can_start, why = start_check(S)
    local start_pend = cmd_pending(lobby.start_cmd)
    w.start.reason = why
    if admin then
        Kit.set_label(w.start, start_pend and "STARTING..." or ((#S.peers <= 1) and "START (SOLO)" or "START MATCH"))
        Kit.set_state(w.start, false, (not can_start) and not start_pend)
        Kit.set_pending(w.start, start_pend)
    elseif S.no_admin then
        -- No admin on this server: READY is the vote, the server starts it.
        Kit.set_label(w.start, S.auto_in and string.format("STARTING IN %d", S.auto_in)
            or string.format("AUTO START (%d/%d)", S.nready, #S.peers))
        Kit.set_state(w.start, false, true)
        Kit.set_pending(w.start, S.auto_in ~= nil)
    else
        Kit.set_label(w.start, string.format("HOST STARTS (%d/%d)", S.nready, #S.peers))
        Kit.set_state(w.start, false, true)
        Kit.set_pending(w.start, false)
    end
    Kit.set_label(w.cancel, lobby.is_host and "CLOSE LOBBY" or "LEAVE")

    -- note line: travel > connection problem > the newest command > START's reason
    local note, ncol
    if MX.ipc_error then
        note, ncol = MX.ipc_error, Kit.C.bad
    elseif lobby.travel_note then
        note, ncol = lobby.travel_note, (lobby.travel_note:find("REFUSED") and Kit.C.bad or Kit.C.ok)
    elseif S.status ~= "connected" and lobby.entered_t and os.clock() - lobby.entered_t > 5
        and HSMP_IPC and HSMP_IPC.available() and not HSMP_IPC.attached() then
        -- the sidecar never attached to the shared-memory segment within 5 s
        note, ncol = (HSMP_IPC.ui_error or "Helper program did not start - reinstall HSMP"), Kit.C.bad
    elseif S.status ~= "connected" and lobby.entered_t and os.clock() - lobby.entered_t > CONNECT_TIMEOUT_S then
        note = string.format("Cannot reach the server%s. Check the address and that its UDP port is open, then LEAVE and try again.",
            lobby.server_addr and (" at " .. lobby.server_addr) or "")
        ncol = Kit.C.bad
    else
        note, ncol = cmd_note(lobby.note_cmd)
    end
    if not note then
        if admin and not can_start then note, ncol = "START: " .. why, Kit.C.dim
        elseif admin then note, ncol = (#S.peers <= 1) and "Everyone is ready. START when you are." or "Everyone is READY - START the match.", Kit.C.good
        elseif S.auto_in then note, ncol = string.format("Everyone is READY - the match starts in %d s.", S.auto_in), Kit.C.good
        elseif S.no_admin then
            note, ncol = (#S.peers < 2) and "No host on this server: the match starts when 2 or more players are all READY."
                or "No host on this server: MARK READY - the match starts when everyone is ready.", Kit.C.dim
        else note, ncol = "Pick your LOADOUT and MARK READY. The host starts the match.", Kit.C.dim end
    end
    Kit.msg(note, ncol)
end

-- --- leaving a session ------------------------------------------------------------------
-- CANCEL / LEAVE / QUIT / session over: ask the sidecar to leave (C2SLeave,
-- career saves restored, status "ended"), wait up to LEAVE_WAIT_S for it,
-- then the kills of our own children. No widget is touched
-- here (safe from the arena / after a level change).
local LEAVE_WAIT_S = 1.5   -- > 4 sidecar leave-request polls (250 ms) + its C2SLeave and career restore

local function write_leave_request(reason)
    local ipc = rawget(_G, "HSMP_IPC")   -- a G2S `leave` record: the sidecar leaves the server and exits
    if not ipc then return end
    Log("leave request (%s)", tostring(reason))
    ipc.send("leave", { reason = ipc.S and ipc.S.ENUMS.leave_reason.USER or 0 })
end

local function teardown_finish(reason, serial, server_grace, on_done)
    if serial and serial ~= lobby.serial then
        Log("leave (%s): a new session started meanwhile - its processes are left alone", tostring(reason))
        if on_done then pcall(on_done) end
        return
    end
    -- Stop the hsmp-sidecar / hsmp-server THIS game spawned, by our process
    -- handles (IPC.proc_kill). Never by image name: another
    -- instance's processes on the same machine are left alone, and with
    -- nothing spawned nothing is killed.
    pcall(kill_role, "sidecar")
    -- A listen server closes by itself when its host leaves and first
    -- tells every joiner ("Host closed the server", ~125 ms): after a graceful
    -- leave it gets 400 ms before the kill (a no-op if it is gone).
    -- on_done (QUIT MP) runs after the server's grace, so the close broadcast
    -- and the kill both happen before the game is asked to quit.
    if server_grace then
        local s = lobby.serial
        ExecuteWithDelay(400, function()
            if lobby.serial == s then pcall(kill_role, "server") end
            if on_done then pcall(on_done) end
        end)
    else
        pcall(kill_role, "server")
    end
    -- Our own local hsmp-master (only if this instance started it).
    if LocalMaster then pcall(LocalMaster.stop, reason) end
    -- Shared memory: nothing to clear. The dead sidecar's session blob reads
    -- as absent once its death is seen (process handle, ~1 s), the next
    -- sidecar publishes under a new epoch, and a leave message nobody
    -- consumed is dropped when the next sidecar attaches.
    legacy_gi_restore("left multiplayer (" .. tostring(reason) .. ")")
    if on_done and not server_grace then pcall(on_done) end
end

-- opts.no_leave: the sidecar already ended (session over).
-- opts.on_done: called once the teardown finished (QUIT MP quits from it).
-- Never a busy-wait on the game thread: the wait is always the 100 ms
-- delayed poll, bounded by LEAVE_WAIT_S, so the sidecar's 250 ms
-- leave-request poll gets to send C2SLeave and restore the career saves
-- before anything is killed. An absent status ends the wait only after
-- LEAVE_ABSENT_READS polls in a row (a momentary absence is not an exit).
MX.LEAVE_ABSENT_READS = 3
local function session_teardown(reason, opts)
    opts = opts or {}
    Log("closing the MP session (%s)", tostring(reason))
    local live = read_sidecar_status()
    if live == "connected" then
        if Cmd then Cmd.fire("ready", { value = false }) end    -- best effort, untracked: the session is being closed
    end
    lobby.active = false
    reset_lobby_session(false, lobby.chosen_map)
    local serial = lobby.serial
    local wait = live ~= nil and live ~= "ended" and not opts.no_leave
    if wait then write_leave_request(reason) end
    if not wait then teardown_finish(reason, nil, false, opts.on_done); return end
    local t0 = os.clock()
    local absent = 0
    local function poll()
        local st = read_sidecar_status()
        absent = (st == nil) and (absent + 1) or 0
        local gone = absent >= MX.LEAVE_ABSENT_READS
        if st == "ended" or gone or os.clock() - t0 >= LEAVE_WAIT_S then
            Log("leave (%s): sidecar %s after %.1f s", tostring(reason),
                (st == "ended") and "ended" or (gone and "gone" or "did not end - stopping it"), os.clock() - t0)
            teardown_finish(reason, serial, st == "ended" or gone, opts.on_done)
        else
            ExecuteWithDelay(100, poll)
        end
    end
    ExecuteWithDelay(100, poll)
end

local function lobby_cancel()
    session_teardown("CANCEL")
    exit_screen()
end

-- Server mods declined or failed: leave the server, back to the server browser.
function MX.mods_leave(why)
    session_teardown(why or "server mods declined")
    exit_screen()
    enter_screen("browser")
end

-- JOIN from the browser. A held session (RECONNECT pending, lobby left for
-- the main menu) is closed gracefully first (leave request, the 100 ms poll,
-- the kills of our children) after a confirm, so the reconnecting sidecar
-- still sends its leave and restores the career saves.
MX.join_click = function(addr, map, label)
    -- (the browser debounces its own JOIN for 3 s; a HOST still starting is
    -- what must not get a second sidecar on the same state files)
    if MX.start_latched() and lobby.is_host then Log("JOIN %s: a hosted session is still starting - ignored", tostring(addr)); return end
    local function go() spawn_sidecar_only(addr, settings.nick, map, label) end
    if lobby.active then
        local function leave_then_join()
            lobby.starting = os.clock()
            exit_screen()
            session_teardown("JOIN another server", { on_done = go })
        end
        if Kit and Kit.cur then
            Kit.confirm(string.format("Leave the current session and join %s?", tostring(label or addr)), "JOIN", leave_then_join)
        else
            leave_then_join()
        end
        return
    end
    go()
    exit_screen()
end

-- Host with other players connected: CLOSE LOBBY disconnects them, so confirm.
local function lobby_cancel_click()
    if not ui_alive() then return end
    local others = remote_peer_count()
    if lobby.is_host and others > 0 then
        Kit.confirm(string.format("Close the lobby? %d other player%s will be disconnected.", others, others == 1 and "" or "s"),
            "CLOSE LOBBY", function() lobby_cancel() end)
        return
    end
    lobby_cancel()
end

-- The session ended under us (kicked, host closed the server, rejected, lost):
-- HSMPHud's modal explains it; the menu only leaves the lobby and cleans up.
-- graceful: the sidecar may still be running (terminal status, lost latch
-- dismissed): ask it to leave (a `leave` record) and wait, as CANCEL does.
-- Not graceful: it already ended. Never a forced kill of a sidecar that
-- could still reconnect.
local function lobby_session_over(why, graceful)
    if lobby.over then return end
    lobby.over = why
    Log("session over (%s): leaving the lobby screen (%s)", tostring(why), graceful and "leave request" or "sidecar ended")
    -- LOADOUT (classes) is opened from the lobby: it closes too
    local on = state.injected and (state.screen_active == "lobby" or state.screen_active == "classes"
        or state.screen_active == "mode" or state.screen_active == "mods")
    session_teardown("session over: " .. tostring(why), { no_leave = not graceful })
    if on then exit_screen() end
end

-- The Director's conn_state bus key latched "lost" for THIS session (written after it began).
-- Returns the reason and the set of actions the HUD offers (reconnect / menu).
-- An absent / cleared conn_state counts only after >= 2 reads spanning
-- >= 1 s; until then the last complete content stands (a momentarily
-- missing record read as "not latched" would look like the player dismissed
-- the modal).
MX.CONN_RD = { last = nil, absent_n = 0, absent_since = nil }
function MX.read_conn_state()
    local ipc = rawget(_G, "HSMP_IPC")
    local t = ipc and ipc.bus_table and ipc.bus_table("conn_state") or nil
    if type(t) == "table" and t.state ~= "" then   -- a zeroed record = cleared
        MX.CONN_RD.last, MX.CONN_RD.absent_n, MX.CONN_RD.absent_since = t, 0, nil
        return t
    end
    local now = os.clock()
    MX.CONN_RD.absent_n = MX.CONN_RD.absent_n + 1
    MX.CONN_RD.absent_since = MX.CONN_RD.absent_since or now
    if MX.CONN_RD.absent_n >= 2 and now - MX.CONN_RD.absent_since >= 1.0 then MX.CONN_RD.last = nil end
    return MX.CONN_RD.last
end
local function conn_lost_latched()
    local t = MX.read_conn_state()
    if type(t) ~= "table" or t.latched ~= true or t.state ~= "lost" then return nil end
    local wall = tonumber(t.wall) or 0
    if wall > 1e11 then wall = wall / 1000 end                  -- ms timestamps tolerated
    if wall + 2 < (lobby.session_wall or 0) then return nil end -- from an earlier session
    local actions = {}
    if type(t.actions) == "table" then for _, a in ipairs(t.actions) do if a ~= "" then actions[tostring(a)] = true end end end
    return tostring(t.reason or "lost"), actions
end

-- When is this session over FOR THE MENU? Only the Director and the
-- player's own clicks end a session:
--   * the sidecar ended (status "ended" after this session was seen live);
--   * a terminal status (kicked, server closed, replaced, rejected) once the
--     player dismissed the HUD modal (no latch any more) or after LOST_HOLD_S
--     (the reason stays readable meanwhile);
--   * a "lost" latch that offers RECONNECT is never acted on: the sidecar
--     keeps trying, RECONNECT / BACK TO MENU in the HUD decide (the Director
--     then writes the leave request and the sidecar ends).
local LOST_HOLD_S = 20
local SESSION_TERMINAL = { kicked = true, server_closed = true, replaced = true, rejected = true,
                           closed = true, disconnected = true, stopped = true, exited = true }
local function lobby_over_reason(sstat)
    -- A terminal status (rejected, kicked, ...) is first only noted. The menu
    -- polls at 2 Hz and the Director at 4 Hz: acting on the status at once
    -- would tear the session down (and its status) before the Director has
    -- latched the reason, so a rejected JOIN or a kick would show its modal
    -- for about 1 s, or never.
    local term = lobby.term
    if sstat and SESSION_TERMINAL[sstat] then
        if not term or term.status ~= sstat then
            term = { status = sstat, at = os.clock(), latched = false }
            lobby.term = term
            Log("session status %s: waiting for the Director's modal before leaving", sstat)
        end
    else
        term, lobby.term = nil, nil
    end
    if sstat == "ended" and (lobby.saw_live or lobby.saw_terminal) then return "sidecar ended", false end
    local lost, actions = conn_lost_latched()
    if term and lost then term.latched = true end
    if lost then
        lobby.lost_seen = true
        if actions.reconnect then
            if lobby.hold_logged ~= lost then
                lobby.hold_logged = lost
                Log("connection lost (%s): RECONNECT offered - the session is kept (no teardown)", lost)
            end
            lobby.lost_since = nil
            return nil
        end
        lobby.lost_since = lobby.lost_since or os.clock()
        if os.clock() - lobby.lost_since < LOST_HOLD_S then return nil end
        return lost, true
    end
    lobby.lost_since, lobby.hold_logged = nil, nil
    -- the Director showed it and the player dismissed it (latch gone), or it
    -- was never latched (an old Director, no HUD) for LOST_HOLD_S
    if term and (term.latched or os.clock() - term.at >= LOST_HOLD_S) then return sstat, true end
    return nil
end

-- START is a command: the server starts the match (or refuses with a reason);
-- the combat map is then requested from the Director by the lobby poll when
-- the session snapshot says countdown/live, on the server's arena. Nothing loads
-- locally on a click. commands.lua resends once after 3 s and reports a
-- timeout after 9 s.
local function lobby_start(via)
    -- host = we spawned the server; at_admin = autotest seat 1 on an external (dedicated) server
    if not lobby.is_host and not lobby.at_admin and not lobby.admin_now then return end
    lobby.travel_note = nil
    lobby.start_cmd = cmd_send("start", nil, via)
    lobby.note_cmd = lobby.start_cmd
    Log("START requested; server arena=%s local pick=%s mode=%s",
        tostring(server_arena(read_match_state() or {})), tostring(lobby.want_map or lobby.chosen_map), settings.lobby_mode)
end

-- Dev/CI autotest driver (host only, HSMP_AUTOTEST=1). Everything goes through
-- cmd_send - a pick is a pick_arena command and START is a start command;
-- travel is still requested by the lobby poll from the server's arena.
--   HSMP_AUTOTEST_WAIT_MS   delay before the first START (a joiner connects)
--   HSMP_AUTOTEST_MAPS      "Map_Arena_Yard,Map_Arena_Pit": consecutive matches
--                           cycle through the list (pick, then START, again
--                           after each return to the lobby). Without it: one
--                           START on the server's arena.
-- Stop-gap for the gate until the RCON MAP / START verbs exist.
local AUTOTEST = { on = false }
local AUTOTEST_RETRY_S = 3
local AUTOTEST_NEXT_S = 2

local function autotest_arm(wait_ms)
    local maps = {}
    for m in (os.getenv("HSMP_AUTOTEST_MAPS") or ""):gmatch("[^,%s]+") do
        if m:match("^[%w_]+$") then maps[#maps + 1] = m else Log("AUTOTEST: ignoring bad arena name '%s'", m) end
    end
    AUTOTEST = { on = true, maps = maps, idx = 0, phase = "wait", t = os.clock() + wait_ms / 1000,
                 matches = 0, cycle = #maps > 0 }
    Log("AUTOTEST: armed (%s), first START in %d ms", #maps > 0 and table.concat(maps, ",") or "server arena", wait_ms)
end

local function autotest_tick(match_st, srv)
    local A = AUTOTEST
    if not A.on or not (lobby.is_host or lobby.at_admin) or not lobby.active then return end
    local now = os.clock()
    if match_st and match_st.state ~= "lobby" then
        if A.phase ~= "in_match" then
            A.phase, A.pick = "in_match", nil
            A.matches = A.matches + 1
            Log("AUTOTEST: match %d running on %s", A.matches, tostring(srv))
        end
        return
    end
    if A.phase == "in_match" then
        if not A.cycle then A.on = false; return end          -- single match
        if lobby.launched then return end                       -- not back in the menu lobby yet
        A.phase, A.t = "wait", now + AUTOTEST_NEXT_S
    end
    if A.phase == "wait" then
        if now < A.t then return end
        if #A.maps > 0 then
            if not A.pick then A.idx = A.idx % #A.maps + 1; A.pick = A.maps[A.idx] end
            if srv == A.pick then
                A.phase = "start_now"
            else
                Log("AUTOTEST: match %d - pick_arena %s (server has %s)", A.matches + 1, A.pick, tostring(srv))
                lobby.want_map = A.pick
                A.cmd = cmd_send("pick_arena", { arena = A.pick }, "autotest")
                lobby.map_cmd, lobby.note_cmd = A.cmd or true, A.cmd
                A.phase = "pick"
            end
        else
            A.phase = "start_now"
        end
    end
    if A.phase == "pick" then
        local st = cmd_status(A.cmd)
        if not st or st.state == "accepted" then
            A.phase = "start_now"
        elseif st.state ~= "pending" then
            Log("AUTOTEST: pick %s %s (%s) - retry in %ds", A.pick, st.state, tostring(st.reason), AUTOTEST_RETRY_S)
            if st.source ~= "timeout" then A.pick = nil end     -- the server refused this arena: next one
            A.phase, A.t = "wait", now + AUTOTEST_RETRY_S
        end
    end
    if A.phase == "start_now" then
        Log("AUTOTEST: requesting match on %s", tostring(A.pick or srv))
        lobby_start("autotest")
        A.cmd, A.phase = lobby.start_cmd, "start"
    elseif A.phase == "start" then
        local st = cmd_status(A.cmd)
        if st and st.state ~= "pending" and st.state ~= "accepted" then
            Log("AUTOTEST: START %s (%s) - retry in %ds", st.state, tostring(st.reason), AUTOTEST_RETRY_S)
            A.phase, A.t = "wait", now + AUTOTEST_RETRY_S
        end
    end
end

-- Called by the 500 ms lobby poll: lobby_ready{epoch, peer_id, notice} once
-- per lobby arrival: when the session first reaches the lobby, again every
-- time a finished/aborted match brings us back to the lobby screen
-- (lobby.ready_rearm, set by the return_to_lobby handler; the harness
-- waits for it after ABORT), and again whenever the server's epoch changes
-- (a restarted server, with a notice). Epochs are u64: compared as digit
-- strings, never through tonumber.
local function read_sidecar_epoch()
    local v = MX.view()   -- the server instance of the last accepted snapshot
    if not (v and v.epoch and v.epoch ~= 0) then return nil end
    local e = v.epoch   -- a u64: its unsigned digit string
    if e >= 0 then return string.format("%d", e) end
    local q = (e >> 1) // 5
    return string.format("%d%d", q, e - q * 10)
end
local function lobby_ready_watch(match_st, srv)
    local my_pid = read_my_peer_id()
    if my_pid == 0 or read_sidecar_status() ~= "connected" then return end
    local epoch = read_sidecar_epoch()
    local new_epoch = lobby.ready_evented and epoch ~= nil and epoch ~= lobby.ready_epoch
    if lobby.ready_evented and not lobby.ready_rearm and not new_epoch then return end
    local notice
    if new_epoch then
        notice = "the server restarted - back in the lobby"
    end
    lobby.ready_evented, lobby.ready_rearm = true, nil
    if epoch ~= nil then lobby.ready_epoch = epoch end
    local role = lobby.is_host and "host" or "join"
    -- the event carries the number when it is exact, else the digit string
    local ev_epoch = epoch and ((#epoch <= 15) and tonumber(epoch) or epoch) or nil
    ev("lobby_ready", { epoch = ev_epoch, peer_id = my_pid, notice = notice, role = role, arena = srv or "" })
    Log("lobby_ready: %s peer %d, epoch %s, server arena %s", role, my_pid, tostring(epoch), tostring(srv))
end

-- --- harness hooks (docs/development/testing.md) -----------------------------------------
--   HSMP_AUTOTEST=host      seat 1: host a listen server (or, with HSMP_AUTOTEST_EXTERNAL=1,
--                           join HSMP_AUTOTEST_ADDR as the admin seat). No auto-START: the
--                           harness drives picks/START through the command channel. With
--                           HSMP_AUTOTEST_MAPS the map-cycle driver picks + STARTs itself.
--   HSMP_AUTOTEST=join      join HSMP_AUTOTEST_ADDR.
--   HSMP_AUTOTEST=1         legacy stopgap: host + START after HSMP_AUTOTEST_WAIT_MS.
--   HSMP_AUTOTEST_READY     1|0: auto-ready in the lobby (default 1 for host/join, 0 for legacy 1).
-- Command channel: `hsmp-tools ipc-ctl --pid <game> autotest <cmd> [arg]`
-- pushes a dev_cmd record (op AUTOTEST, key = cmd: pick_arena | start | ready | unready |
-- leave | quit | move, arg) into the game's DevCtl ring; IPC.dev_poll hands it to this Lua
-- state once (the native cursor starts at load, so an earlier run's commands never arrive).
-- Each is dispatched through the SAME functions as the buttons, so pick_arena / start /
-- ready produce the usual cmd_sent / cmd_result events. TUNE / TDIAG records are other
-- mods' business and are ignored here.
local AT_MODE = os.getenv("HSMP_AUTOTEST")
if AT_MODE ~= "1" and AT_MODE ~= "host" and AT_MODE ~= "join" then AT_MODE = nil end
local AT_READY_RETRY_S = 3
local at = {
    dev_out = {},
    ready_want = (function()
        local r = os.getenv("HSMP_AUTOTEST_READY")
        if r == "1" then return true elseif r == "0" then return false end
        return AT_MODE == "host" or AT_MODE == "join"
    end)(),
    ready_t = -1e9,
    handled = 0,
}

-- "Yard" | "map_arena_yard" | "Lords Hall" | "Map_Arena_LordsHall" -> "Map_Arena_LordsHall".
-- An unknown but well-formed name is passed through (the server refuses it: cmd_result ok=false).
local function resolve_arena(a)
    if type(a) ~= "string" then return nil end
    local k = a:gsub("%s", ""):lower()
    if k == "" then return nil end
    for _, m in ipairs(MAP_PRESETS) do
        local p = m.path:lower()
        if p == k or p == "map_arena_" .. k or m.name:gsub("%s", ""):lower() == k then return m.path end
    end
    if a:match("^Map_[%w_]+$") then return a end
    if a:match("^[%w_]+$") then return "Map_Arena_" .. a end
    return nil
end

-- Leave the session from wherever we are (lobby screen, arena, after a level change).
local function autotest_leave(why)
    local on_menu = state.injected and state.canvas ~= nil
    if on_menu and state.screen_active then
        session_teardown("CANCEL")
        exit_screen()
    else
        session_teardown(why)
        if Travel and not on_menu then Travel.request("menu", nil, "autotest_" .. why) end
    end
end

local function autotest_dispatch(c)
    local name = c.cmd
    at.handled = at.handled + 1
    ev("x_autotest_cmd", { id = c.id, cmd = name, arg = c.arg })
    Log("AUTOTEST cmd #%s: %s %s", tostring(c.id), tostring(name), tostring(c.arg or ""))
    if name == "pick_arena" then
        local path = resolve_arena(c.arg)
        if not path then Log("AUTOTEST cmd #%s: bad arena %s - ignored", tostring(c.id), tostring(c.arg)); return end
        host_pick_map(path, "autotest")
    elseif name == "start" then
        if not lobby.is_host and not lobby.at_admin then
            Log("AUTOTEST cmd #%s: start ignored - this instance is not seat 1", tostring(c.id)); return
        end
        lobby_start("autotest")
    elseif name == "ready" or name == "unready" then
        at.ready_want = (name == "ready")
        set_ready(at.ready_want, "autotest")
        at.ready_t = os.clock()
    elseif name == "leave" then
        autotest_leave("leave")
    elseif name == "quit" then
        quit_desktop()
    elseif name == "move" then
        -- harness-only scripted mover (autotest_mover.lua): the pose check under motion
        if not at.mover then
            local Mv = load_module("autotest_mover", true)
            at.mover = Mv and Mv.new({ log = Log, UEH = UEHelpers, loop = LoopAsync }) or false
        end
        if at.mover then at.mover.start(c.arg or "10")
        else Log("AUTOTEST cmd #%s: move - autotest_mover.lua not found", tostring(c.id)) end
    elseif name == "world_poke" then
        -- HSMPWorld reads it from its own DevCtl cursor (world sync test)
    elseif name == "team" then
        -- the GAME MODE screen's YOUR TEAM chip (modes_ui.lua send "set_team")
        local t = math.tointeger(tonumber(c.arg))
        if not t then Log("AUTOTEST cmd #%s: team needs a number", tostring(c.id)); return end
        lobby.note_cmd = cmd_send("set_team", { team = t })
    elseif name == "kit" then
        -- the LOADOUT screen's class card + SAVE (classes.lua): worn from the next spawn
        local ok, why = false, "no classes module"
        if Classes and Classes.autotest_kit then ok, why = Classes.autotest_kit(tostring(c.arg or "")) end   -- "<class> [r=..] [l=..] [armor=a,b]"
        Log("AUTOTEST cmd #%s: kit %s -> %s", tostring(c.id), tostring(c.arg), ok and "saved" or tostring(why))
    elseif name == "mods_accept" or name == "mods_decline" then
        -- the SERVER MODS screen's ACCEPT & JOIN / DECLINE buttons
        if not MX.Mods then Log("AUTOTEST cmd #%s: %s - no server_mods module", tostring(c.id), name); return end
        if name == "mods_accept" then MX.Mods.accept() else MX.Mods.decline() end
    else
        Log("AUTOTEST cmd #%s: unknown cmd %s - ignored", tostring(c.id), tostring(name))
    end
end

-- Poll the command channel (game thread). Returns the number of commands run.
local function autotest_channel_tick()
    if not AT_MODE then return 0 end
    local ipc = rawget(_G, "HSMP_IPC")
    if not ipc then return 0 end
    local out = at.dev_out
    local m = ipc.dev_poll(16, out)
    local n = 0
    for i = 1, m do
        local d = out[i] and (out[i].data or out[i])
        if type(d) == "table" and d.op == 1 then             -- S.ENUMS.dev_op.AUTOTEST
            local c = { id = d.id, cmd = d.key, arg = (d.arg ~= nil and d.arg ~= "") and d.arg or nil }
            if type(c.cmd) ~= "string" or not c.cmd:match("^[%w_]+$") then
                Log("AUTOTEST: unreadable command %s", tostring(c.cmd))
            else
                local ok, err = pcall(autotest_dispatch, c)
                if not ok then Log("AUTOTEST cmd #%s %s failed: %s", tostring(c.id), c.cmd, tostring(err)) end
                n = n + 1
            end
        end
    end
    return n
end

-- Auto-ready (HSMP_AUTOTEST_READY): in the lobby, connected and not ready -> a ready
-- command (the button's path); retried while the server does not list us.
local function autotest_ready_tick(match_st)
    if not AT_MODE or not at.ready_want or not lobby.active then return end
    if not match_st or match_st.state ~= "lobby" then return end
    local me = read_my_peer_id()
    if me == 0 or read_sidecar_status() ~= "connected" or is_peer_ready(me, match_st) then return end
    if cmd_pending(lobby.ready_cmd) or os.clock() - at.ready_t < AT_READY_RETRY_S then return end
    at.ready_t = os.clock()
    Log("AUTOTEST: auto-ready")
    set_ready(true, "autotest")
end

-- Session in progress? (a sidecar status or a child of ours still running)
local function session_exists()
    if lobby.active then return true end
    local st = read_sidecar_status()
    if st ~= nil and st ~= "ended" then return true end   -- a sidecar of ours is (or was just) running
    local ipc = rawget(_G, "HSMP_IPC")
    for _, role in ipairs({ "sidecar", "server" }) do
        if MX.procs[role] and ipc and ipc.proc_alive(MX.procs[role]) then return true end
    end
    return false
end

-- QUIT MP / harness quit: the session is torn down first (leave request,
-- the 100 ms delayed poll for up to LEAVE_WAIT_S, our children killed), so nothing
-- outlives the game; the game quits from the teardown's done-callback (no
-- busy-wait, no forced kill of a sidecar that is leaving).
MX.quitting = nil
function MX.quit_now()
    if LocalMaster then pcall(LocalMaster.stop, "quit") end
    -- KismetSystemLibrary:QuitGame (the BP "Quit Game" node); the process
    -- exits in ~2 s. APlayerController:ConsoleCommand is not callable from
    -- UE4SS Lua ("attempt to call a TrivialObject value"), so a console "quit"
    -- silently does nothing (and WM_CLOSE does not close Half Sword either).
    local pc = UEHelpers.GetPlayerController()
    local quit_ok = false
    pcall(function()
        local ksl = UEHelpers.GetKismetSystemLibrary()
        if ksl and ksl:IsValid() and pc and pc:IsValid() then
            ksl:QuitGame(pc, pc, 0, false)     -- EQuitPreference::Quit, platform restrictions kept
            quit_ok = true
        end
    end)
    -- (no console fallback: ConsoleCommand is not callable from Lua, see above)
    Log("quit: %s", quit_ok and "QuitGame issued" or "QuitGame unavailable (no PlayerController / KismetSystemLibrary)")
    if not quit_ok then MX.quitting = nil end   -- let the player try again
end
function quit_desktop()
    if MX.quitting then Log("quit: already leaving the session - ignored"); return end
    MX.quitting = os.clock()
    if session_exists() then
        Log("quit: leaving the MP session first")
        session_teardown("quit", { on_done = MX.quit_now })
    else
        MX.quit_now()
    end
end

-- LOBBY layout (design units; see docs/development/subsystems/menu-ui.md for the mock):
--   PLAYERS (left 56%): 8 roster rows [x] NAME (YOU) HOST | CLASS | PING  [MAKE HOST][KICK]
--   MATCH (right): ARENA tiles 4+3, ROUNDS chips, KIT RULES chips, BUDGET chips, MODE line
--   message line (command results / START's reason / confirm strip)
--   [MARK READY] [LOADOUT]                       [START MATCH] [CLOSE LOBBY | LEAVE]
local function build_lobby_kit()
    local L
    local title = lobby.is_host and "HOST LOBBY" or "LOBBY"
    if lobby.server_label then title = title .. "  -  " .. lobby.server_label end
    L, ui = Kit.frame("lobby", title, { h = 840 })
    lobby.entered_t = lobby.entered_t or os.clock()
    local w = ui.w
    local u, F = L.u, L.F
    local gap = L.gap
    local colgap = u(Kit.SP.xxl)
    local lw = math.floor((L.iw - colgap) * 0.56)
    local rx = L.x0 + lw + colgap
    local rw = L.iw - lw - colgap
    local sh = u(30)
    local top = L.top

    -- roster (left)
    Kit.group("roster")
    w.roster_head = Kit.text("", L.x0, top, math.floor(lw * 0.6), sh, F(Kit.TS.h2), 0, Kit.C.head)
    w.roster_sub = Kit.text("", L.x0 + math.floor(lw * 0.6), top, lw - math.floor(lw * 0.6), sh, F(Kit.TS.label), 2, Kit.C.dim)
    local y = top + sh + u(Kit.SP.sm)
    local hh = u(28)
    local aw = u(118)                               -- one inline action button
    local acts_w = 2 * aw + gap + u(Kit.SP.sm)
    local rowsw = lw - acts_w
    local cols = { { 0.00, 0.21, "STATUS" }, { 0.21, 0.40, "NAME" }, { 0.61, 0.24, "CLASS" }, { 0.85, 0.15, "PING" } }
    Kit.rect(L.x0, y, lw, hh, Kit.C.section, 11)
    local cpad = u(Kit.SP.md)
    for _, c in ipairs(cols) do
        Kit.text(c[3], L.x0 + math.floor(rowsw * c[1]) + cpad, y, math.floor(rowsw * c[2]) - 2 * cpad + (c[3] == "PING" and cpad or 0), hh,
            F(Kit.TS.small), 0, Kit.C.dim)
    end
    w.tools_head = Kit.text("", L.x0 + rowsw + u(Kit.SP.sm), y, acts_w - u(Kit.SP.sm), hh, F(Kit.TS.small), 1, Kit.C.dim)
    y = y + hh + u(Kit.SP.xs)
    local avail = L.bottom - u(Kit.SP.xl) - u(30) - y
    local prh = math.min(u(46), math.floor(avail / MAX_LOBBY_ROWS) - u(Kit.SP.xs))
    local pfs = math.min(F(Kit.TS.body), math.floor(prh / Kit.LINE_H))
    w.rows = {}
    for i = 1, MAX_LOBBY_ROWS do
        local ry = y + (i - 1) * (prh + u(Kit.SP.xs))
        local r = {}
        local idx = i
        r.btn = Kit.button("", function()
            if not ui_alive() then return end
            local row = ui.w.rows[idx]
            if row.peer then
                lobby.sel_peer = (lobby.sel_peer == row.peer) and nil or row.peer
                render_lobby()
            end
        end, L.x0, ry, rowsw, prh, { style = (i % 2 == 1) and "row" or "rowb", no_label = true, fkey = "lobby:row" .. i,
            help = function()
                if not lobby.admin_now then return "A player in this lobby" end
                return "Select a player for the host tools (MAKE HOST / KICK)"
            end })
        Kit.rect(L.x0 + rowsw, ry, lw - rowsw, prh, (i % 2 == 1) and Kit.C.section or Kit.C.track, 10)
        local function cell(ci, just)
            local c = cols[ci]
            return Kit.text("", L.x0 + math.floor(rowsw * c[1]) + cpad, ry, math.floor(rowsw * c[2]) - 2 * cpad + (ci == 4 and cpad or 0),
                prh, pfs, just or 0, Kit.C.text, { z = 14 })
        end
        r.ready = cell(1)
        r.name = cell(2)
        r.class = cell(3)
        r.ping = cell(4)
        r.btn.texts = { r.ready, r.name, r.class, r.ping }
        local bx = L.x0 + rowsw + u(Kit.SP.sm)
        r.promote = Kit.button("MAKE HOST", function()
            if not ui_alive() then return end
            local pid = ui.w.rows[idx].peer
            if not pid then return end
            Kit.confirm(string.format("Make %s the host? You lose the host tools.", peer_nick(pid)), "MAKE HOST", function()
                lobby.promote_cmd = cmd_send("promote", { peer = pid, nick = peer_nick(pid) })
                lobby.note_cmd = lobby.promote_cmd
                render_lobby()
            end, { danger = false })
        end, bx, ry, aw, prh, { fs = F(Kit.TS.small), short = "HOST", help = "Give the host role (map, rules, START) to this player" })
        r.kick = Kit.button("KICK", function()
            if not ui_alive() then return end
            local pid = ui.w.rows[idx].peer
            if not pid then return end
            Kit.confirm(string.format("Kick %s from the server?", peer_nick(pid)), "KICK", function()
                lobby.kick_cmd = cmd_send("kick", { peer = pid, nick = peer_nick(pid) })
                lobby.note_cmd = lobby.kick_cmd
                lobby.sel_peer = nil
                render_lobby()
            end)
        end, bx + aw + gap, ry, aw, prh, { fs = F(Kit.TS.small), style = "danger", help = "Remove this player from the server" })
        Kit.show(r.promote, false); Kit.show(r.kick, false)
        w.rows[i] = r
    end
    w.no_peers = Kit.text("", L.x0, y + prh + u(Kit.SP.xs), rowsw, prh, pfs, 1, Kit.C.dim, { z = 14 })
    local ky = y + MAX_LOBBY_ROWS * (prh + u(Kit.SP.xs)) + u(Kit.SP.md)
    w.loadout_sum = Classes and Kit.text("", L.x0, ky, lw, u(30), F(Kit.TS.label), 0, Kit.C.text) or nil

    -- match settings (right)
    local my = top
    Kit.text("MATCH", rx, my, rw, sh, F(Kit.TS.h2), 0, Kit.C.head)
    my = my + sh + u(Kit.SP.sm)
    local lh = u(26)
    local function label_row(t)
        local lbl = Kit.text(t, rx, my, math.floor(rw * 0.4), lh, F(Kit.TS.label), 0, Kit.C.text)
        local hint = Kit.text("", rx + math.floor(rw * 0.4), my, rw - math.floor(rw * 0.4), lh, F(Kit.TS.small), 2, Kit.C.dim)
        my = my + lh + u(Kit.SP.xs)
        return lbl, hint
    end
    local host_only = function() return lobby.admin_now and nil or "Only the host changes the match settings" end
    Kit.group("arena")
    _, w.map_hint = label_row("ARENA")
    -- tall enough for a title + meta + note line at the DPI's minimum font
    local tfs = F(Kit.TS.label)
    local tfs2 = math.max(Kit.minfs or 9, math.floor(tfs * 0.82))
    local tile_h = math.max(u(76), 2 * math.floor(8 * L.s) + math.ceil(tfs * Kit.LINE_H) + 2 * math.ceil(tfs2 * Kit.LINE_H) + 2)
    local rects = Kit.grid_rects(rx, my, rw, 2 * tile_h + gap, 4, 2, gap)
    w.maps = { chips = {}, keys = {}, by_key = {} }
    for i, m in ipairs(MAP_PRESETS) do
        local path = m.path
        local rc = rects[i]
        local ref = Kit.tile(function()
            if not ui_alive() or not lobby.admin_now then return end
            host_pick_map(path); render_lobby()
        end, rc[1], rc[2], rc[3], rc[4], { scale = L.s, fs = F(Kit.TS.label), title_lines = 1, fkey = "lobby:map:" .. path,
            help = "Ask the server for " .. m.name .. " (everyone loads the server's arena)" })
        ref.reason = host_only
        ref.on_disabled = function() Kit.flash("Only the host picks the arena", Kit.C.warn) end
        w.maps.chips[i] = ref; w.maps.keys[i] = path; w.maps.by_key[path] = ref
    end
    my = my + 2 * tile_h + gap + u(Kit.SP.md)
    Kit.group("rounds")
    _, w.mode_hint = label_row("ROUNDS")
    local chh = u(Kit.BH.chip)
    local bo = {}
    for _, n in ipairs(BEST_OF) do bo[#bo + 1] = { n, "BEST OF " .. n } end
    w.modes = Kit.chip_group(bo, rx, my, rw, chh, function(n)
        if not ui_alive() or not lobby.admin_now then return end
        host_pick_best_of(n); render_lobby()
    end, { fs = F(Kit.TS.label), gap = gap, help = "How many rounds decide the match" })
    Kit.chip_group_reason(w.modes, "Only the host changes the match settings")
    my = my + chh + u(Kit.SP.md)
    Kit.group("rules")
    _, w.rules_hint = label_row("KIT RULES")
    w.kitmode = Kit.chip_group({ { 0, "FREE", "Any kit, no points limit" }, { 1, "CLASSES ONLY", "Class presets only, no custom kits" },
                                 { 2, "CUSTOM", "Custom kits within a point budget" } },
        rx, my, rw, chh, function(mode)
            if not ui_alive() or not lobby.admin_now then return end
            local r = server_kit_rules() or {}
            host_pick_rules(mode, (mode == 2) and (r.budget and r.budget > 0 and r.budget or 30) or (r.budget or 30))
            render_lobby()
        end, { fs = F(Kit.TS.label), gap = gap })
    Kit.chip_group_reason(w.kitmode, "Only the host changes the match settings")
    my = my + chh + u(Kit.SP.sm)
    local budgets = {}
    for _, v in ipairs((Classes and Classes.budgets and Classes.budgets()) or { 12, 20, 30, 45, 60 }) do
        budgets[#budgets + 1] = { v, v .. " PTS" }
    end
    Kit.text("BUDGET", rx, my, u(110), chh, F(Kit.TS.small), 0, Kit.C.dim)
    w.budget = Kit.chip_group(budgets, rx + u(110), my, rw - u(110), chh, function(v)
        if not ui_alive() or not lobby.admin_now then return end
        host_pick_rules(2, v); render_lobby()
    end, { fs = F(Kit.TS.label), gap = gap, help = "Points every CUSTOM kit must fit in" })
    my = my + chh + u(Kit.SP.lg)
    w.mode_line = Kit.text("", rx, my, rw, lh, F(Kit.TS.small), 0, Kit.C.dim)
    if lobby.is_host then
        -- whether internet players can reach us: the server's NET_STATUS (router port opened
        -- automatically, or what to forward by hand); refreshed by render_lobby
        local p = host_port()
        local fs = F(Kit.TS.small)
        local ny = my + lh
        local lines = math.min(2, math.floor((L.bottom - ny) / math.ceil(fs * Kit.LINE_H)))
        if lines >= 1 then
            w.port_note = Kit.text("", rx, ny, rw, math.ceil(fs * Kit.LINE_H) * lines, fs, 0, Kit.C.dim,
                { wrap = lines > 1 and lines or nil, valign = "top" })
            local note, col = host_net_note(p)
            Kit.set_text_fit(w.port_note, note)
            Kit.set_color(w.port_note, col)
        end
    end

    -- actions: READY | LOADOUT                START | CLOSE LOBBY / LEAVE
    local acts = Kit.actions(L,
        { { label = "MARK READY", key = "ready", cb = function()
              if not ui_alive() then return end
              toggle_ready(); render_lobby()
          end, reason = "Not connected to the server yet" },
          { label = "LOADOUT", key = "loadout", help = "Pick your class and gear", cb = function()
              if ui_alive() then enter_screen("classes") end
          end },
          { label = "MODE", key = "mode", help = "Game mode, teams and your team", cb = function()
              if ui_alive() and MX.ModeUI then enter_screen("mode") end
          end } },
        { { label = lobby.is_host and "CLOSE LOBBY" or "LEAVE", key = "cancel", style = "danger", cb = lobby_cancel_click,
            help = lobby.is_host and "Close the server and return to the menu" or "Leave the server and return to the menu" },
          { label = "START MATCH", key = "start", style = "primary", help = "Start the match on the server's arena", cb = function()
              if ui_alive() then lobby_start(); render_lobby() end
          end } })
    w.ready, w.loadout, w.cancel, w.start = acts.ready, acts.loadout, acts.cancel, acts.start
    w.start.on_disabled = function() if ui_alive() then Kit.flash(Kit.reason_of(w.start) or "START is not available", Kit.C.warn) end end
    w.note = L.msg
    ui.on_back = lobby_cancel_click
    Kit.focus_default(w.ready)
    render_lobby()
    Log("lobby build: canvas %.0fx%.0f scale %.2f panel %dx%d rows=%d widgets=%d", L.cw, L.ch, L.s, L.pw, L.ph, MAX_LOBBY_ROWS, ui.n)
end

-- --- enter / exit / refresh ----------------------------------------------

local BUILDERS = {
    browser   = function() if Browser then Browser.build(); return Browser.render end end,
    settings  = function() if Settings then Settings.build(); return Settings.render end end,
    character = function() build_character_kit(); return render_character end,
    lobby     = function() build_lobby_kit(); return render_lobby end,
    classes   = function() if Classes then Classes.build(); return Kit and Classes.render or nil end end,
    mode      = function() if MX.ModeUI then MX.ModeUI.build(); return MX.ModeUI.render end end,
    mods      = function() if MX.Mods then MX.Mods.build(); return MX.Mods.render end end,
}

function enter_screen(kind)
    if state.screen_active == kind then return end
    -- A delayed caller may arrive after a level load freed the menu: only
    -- build on a live, injected menu of the current world.
    if not state.injected or not state.wt or not state.canvas then
        Log("enter screen %s skipped: menu not injected in this world", tostring(kind))
        return
    end
    if not Kit or not BUILDERS[kind] then
        Log("enter screen %s refused: %s", tostring(kind), Kit and "unknown screen" or "ui_kit.lua is missing")
        return
    end
    -- No zombie lobby stuck on CONNECTING...: the lobby (and LOADOUT,
    -- opened from it) needs a session that is held or starting.
    if (kind == "lobby" or kind == "classes" or kind == "mode" or kind == "mods") and not lobby.active and not lobby.starting then
        Log("enter screen %s refused: no MP session (it ended)", kind)
        if state.screen_active then exit_screen() end
        return
    end
    destroy_screen_widgets()
    hide_top_and_natives()
    state.top_focus = nil
    pcall(Kit._blur)                     -- no native widget keeps keyboard focus under our screen
    state.screen_active = kind
    state.screen_render = nil            -- ui_kit screens re-render in place; set by the builder
    if kind == "lobby" then lobby.active, lobby.starting = true, nil end
    state.screen_build = BUILDERS[kind]
    local ok, render = pcall(state.screen_build)
    if not ok then Log("build %s failed: %s", kind, tostring(render)) end
    state.screen_render = ok and render or nil
    fade_in_all(state.screen_widgets, 150)
    Log("enter screen: %s (%d widgets)", kind, #state.screen_widgets)
end

function exit_screen()
    local was = state.screen_active
    destroy_screen_widgets()
    show_top_and_natives()
    state.screen_active = nil
    state.screen_build = nil
    state.screen_render = nil
    Log("exit screen → top (from %s)", tostring(was))
end

-- ui_kit screens update their widgets in place (no rebuild, no flicker).
function refresh_screen()
    if not state.screen_active or not state.screen_render then return end
    local t0 = os.clock()
    local ok, err = pcall(state.screen_render)
    if not ok then Log("render %s failed: %s", tostring(state.screen_active), tostring(err)) end
    if Kit then Kit.perf_add("render", (os.clock() - t0) * 1000) end
end

-- --- resize: every layout follows the window / resolution / DPI --------------------
-- Polled every 250 ms on the game thread while this world's menu is injected
-- (fresh probe; the menu widget of THIS world is the context). A new canvas
-- size must hold for 2 polls (a window drag reports many), then the ribbon
-- column moves in place and the open screen is rebuilt at the new scale. Its
-- module state (browser page / filters / selection, loadout picks, unsaved
-- settings and character edits) survives: Kit.relayout tells the builders.
MX.resize_watch = UIS and UIS.watcher(2) or nil
MX.relayouts = 0

function MX.place_top_ribbons()
    local rects = MX.top_ribbon_rects()
    for i, e in ipairs(state.ribbons) do
        local r = rects[i]
        if e.slot and r then
            pcall(function()
                e.slot:SetPosition({ X = r[1], Y = r[2] })
                e.slot:SetSize({ X = r[3], Y = r[4] })
            end)
            e.rect = r
        end
    end
    if state.top_focus then top_focus(state.top_focus) end
end

function MX.relayout(why)
    if not state.injected or not state.canvas or not state.style_src then return end
    MX.relayouts = MX.relayouts + 1
    MX.ribbon_geometry()
    MX.place_top_ribbons()
    local M = state.metrics or {}
    Log("relayout #%d (%s): viewport %.0fx%.0f dpi %.3f -> canvas %.0fx%.0f scale %.2f [%s]", MX.relayouts,
        tostring(why), M.vw or 0, M.vh or 0, M.dpi or 0, M.cw or 0, M.ch or 0, M.s or 0, tostring(M.src))
    local kind = state.screen_active
    if kind and Kit then
        -- let the screen keep what is typed / staged (its own widgets, still alive)
        local cur = Kit.cur
        if cur and cur.before_rebuild and Kit.alive(cur) then pcall(cur.before_rebuild) end
        Kit.relayout = true
        state.screen_active = nil            -- enter_screen ignores the screen that is already open
        local ok, err = pcall(enter_screen, kind)
        Kit.relayout = false
        if not ok then Log("relayout: rebuilding %s failed: %s", kind, tostring(err)) end
    end
end

-- Seed the watcher with the size the menu was just built at.
function MX.resize_seed()
    if not MX.resize_watch then return end
    UIS.watch_reset(MX.resize_watch)
    local M = state.metrics
    if M then UIS.watch(MX.resize_watch, M.cw, M.ch, M.dpi) end
end

function MX.resize_poll()
    if not MX.resize_watch or not state.injected or not state.menu or not state.canvas then return end
    local M = MX.measure_viewport()
    if UIS.watch(MX.resize_watch, M.cw, M.ch, M.dpi) then MX.relayout("viewport changed") end
end

-- --- input router: keyboard + gamepad -> the screen's focus manager -------------------
-- Keys are registered once (UE4SS cannot unregister); each press is queued
-- and handled by the 33 ms game-thread poll. TAB waits one poll so a
-- Shift+TAB (both binds fire) counts once.

local input_q = {}
local function key_push(action, device) input_q[#input_q + 1] = { a = action, d = device or "kb" } end

local KEYMAP = {
    { "UP_ARROW", "up" }, { "DOWN_ARROW", "down" }, { "LEFT_ARROW", "left" }, { "RIGHT_ARROW", "right" },
    { "RETURN", "accept" }, { "SPACE", "accept" }, { "ESCAPE", "back" }, { "TAB", "next_group" },
    { "PAGE_UP", "page_prev" }, { "PAGE_DOWN", "page_next" }, { "Q", "tab_prev" }, { "E", "tab_next" },
    { "F5", "refresh" },
}
local function register_keys()
    if not Key or not RegisterKeyBind then Log("input: no RegisterKeyBind - mouse only"); return end
    local n = 0
    for _, km in ipairs(KEYMAP) do
        local k = Key[km[1]]
        if k ~= nil then
            local action = km[2]
            local ok = pcall(RegisterKeyBind, k, function() key_push(action, "kb") end)
            if ok then n = n + 1 end
        end
    end
    if Key.TAB ~= nil and ModifierKey and ModifierKey.SHIFT ~= nil then
        if pcall(RegisterKeyBind, Key.TAB, { ModifierKey.SHIFT }, function() key_push("prev_group", "kb") end) then n = n + 1 end
    end
    Log("input: %d key binds (arrows, Enter/Space, Esc, Tab/Shift+Tab, PgUp/PgDn, Q/E, F5)", n)
end
register_keys()

-- Gamepad: PlayerController:IsInputKeyDown(FKey) per pad key,
-- rising edges + hold-repeat for the D-pad. In the menu's UI-only input mode
-- the controller may never see pad keys; then this simply never fires. A
-- failing call disables the poll after 3 errors. hsmp.cfg menu_gamepad = 0 turns it off.
local PADMAP = {
    { "Gamepad_DPad_Up", "up", true }, { "Gamepad_DPad_Down", "down", true },
    { "Gamepad_DPad_Left", "left", true }, { "Gamepad_DPad_Right", "right", true },
    { "Gamepad_LeftStick_Up", "up", true }, { "Gamepad_LeftStick_Down", "down", true },
    { "Gamepad_LeftStick_Left", "left", true }, { "Gamepad_LeftStick_Right", "right", true },
    { "Gamepad_FaceButton_Bottom", "accept" }, { "Gamepad_FaceButton_Right", "back" },
    { "Gamepad_FaceButton_Top", "refresh" },
    { "Gamepad_LeftShoulder", "tab_prev" }, { "Gamepad_RightShoulder", "tab_next" },
    { "Gamepad_LeftTrigger", "page_prev" }, { "Gamepad_RightTrigger", "page_next" },
}
local PAD_REPEAT_DELAY_S, PAD_REPEAT_EVERY_S = 0.40, 0.12
local pad = { on = cfg_get("menu_gamepad", "1") ~= "0", errors = 0, keys = nil, seen = false }
local function pad_poll()
    if not pad.on or not state.injected then return end
    if not pad.keys then
        pad.keys = {}
        for _, p in ipairs(PADMAP) do
            pad.keys[#pad.keys + 1] = { key = { KeyName = FName(p[1], FNAME_Add) }, action = p[2], rep = p[3], down = false, t = 0, next_t = 0 }
        end
    end
    local pc = UEHelpers.GetPlayerController()
    if not pc or not pc:IsValid() then return end
    local now = os.clock()
    for _, k in ipairs(pad.keys) do
        local ok, down = pcall(function() return pc:IsInputKeyDown(k.key) end)
        if not ok or type(down) ~= "boolean" then
            pad.errors = pad.errors + 1
            if pad.errors >= 3 then pad.on = false; Log("input: gamepad poll disabled (IsInputKeyDown failed: %s)", tostring(down)) end
            return
        end
        if down and not k.down then
            k.t, k.next_t = now, now + PAD_REPEAT_DELAY_S
            if not pad.seen then pad.seen = true; Log("input: gamepad seen (%s)", k.action) end
            key_push(k.action, "pad")
        elseif down and k.rep and now >= k.next_t then
            k.next_t = now + PAD_REPEAT_EVERY_S
            key_push(k.action, "pad")
        end
        k.down = down
    end
end

-- Top level (no screen): the ribbon column joins the keyboard focus on TAB
-- only (arrows / Enter stay with the native menu and its own panels, e.g. its
-- Settings sliders); UP/DOWN move, ENTER/A opens, LEFT/ESC/B hand the focus
-- back. Touching a native button with the mouse drops the column focus too.
local function top_key(action)
    if not state.injected or #state.ribbons == 0 then return end
    local i = state.top_focus
    if not i then
        if action == "next_group" then
            pcall(Kit and Kit._blur or function() end)
            top_focus(1)
        end
        return
    end
    if action == "up" then top_focus((i - 2) % #state.ribbons + 1)
    elseif action == "down" or action == "next_group" then top_focus(i % #state.ribbons + 1)
    elseif action == "prev_group" then top_focus((i - 2) % #state.ribbons + 1)
    elseif action == "accept" then
        local e = state.ribbons[i]
        if e and e.cb then local ok, err = pcall(e.cb); if not ok then Log("ribbon %d failed: %s", i, tostring(err)) end end
    elseif action == "left" or action == "back" then top_focus(nil)
    end
end

local function dispatch(action, device)
    if not state.injected then return end
    -- While HSMPHud's connection modal is latched, keys never reach the menu
    -- behind it (Enter on a focused ribbon would fire HOST GAME).
    do
        local cs = MX.read_conn_state()
        local wall = type(cs) == "table" and tonumber(cs.wall) or 0
        if wall > 1e11 then wall = wall / 1000 end
        -- (only a latch written during this game run: a crash can leave one behind)
        if type(cs) == "table" and cs.latched == true and wall + 2 >= MX.boot_wall then
            if action == "accept" then Log("input %s ignored: the connection modal is open", action) end
            return
        end
    end
    if state.screen_active then
        local scr = state.screen_active
        if Kit then Kit.key(action, device) end
        -- left a screen with the keyboard: the ribbon that opened it gets the focus
        if state.screen_active == nil and state.injected and SCREEN_RIBBON[scr] then top_focus(SCREEN_RIBBON[scr]) end
    else
        top_key(action)
    end
end

local function drain_input()
    if #input_q == 0 then return end
    local batch = input_q
    input_q = {}
    local has_prev = false
    for _, e in ipairs(batch) do if e.a == "prev_group" then has_prev = true end end
    for _, e in ipairs(batch) do
        if e.a == "next_group" and has_prev then
            -- Shift+TAB fired both binds: the shifted one wins
        elseif e.a == "next_group" and not e.held then
            e.held = true
            input_q[#input_q + 1] = e
        else
            local ok, err = pcall(dispatch, e.a, e.d)
            if not ok then Log("input %s failed: %s", tostring(e.a), tostring(err)) end
        end
    end
end

-- --- click polling (top + screen) ----------------------------------------

local function poll_set(entries)
    for _, r in ipairs(entries) do
        if not r.nopoll and not r.hidden and r.button and r.button:IsValid() then   -- nopoll: texts/inputs; hidden: collapsed by ui_kit
            local pressed = false
            pcall(function() pressed = r.button:IsPressed() end)
            if pressed and not r.prev then
                r.prev = true           -- latch BEFORE firing so re-poll
                pcall(r.cb)              -- during the same hold doesn't retrigger
                return true
            end
            r.prev = pressed
        end
    end
    return false
end

-- --- top-level injection --------------------------------------------------

local function inject()
    if state.injected then return end
    local menu = find_menu(); if not menu then return end
    if not discover_native(menu) then return end
    inject_top_ribbons()
    state.injected = true
    MX.resize_seed()
    do
        local M = state.metrics or {}
        Log("viewport %.0fx%.0f dpi %.3f -> canvas %.0fx%.0f scale %.2f [%s]", M.vw or 0, M.vh or 0, M.dpi or 0,
            M.cw or 0, M.ch or 0, M.s or 0, tostring(M.src))
    end
    Log("top-ribbon injection complete")

    -- Back on the main menu with no MP session (left MP, or the game crashed
    -- mid-session): the single-player GI values go back. That is the
    -- Director's job (TravelMenu); the legacy shim does it while there is none.
    local sc = read_sidecar_status()
    if sc == nil or sc == "ended" then legacy_gi_restore("main menu, no MP session") end

    -- Dev/CI hooks (harness contract above autotest_dispatch; see also the
    -- splash skip at the bottom of this file). Once per process.
    if AT_MODE and not at.fired then
        at.fired = true
        local addr = env_get("HSMP_AUTOTEST_ADDR") or "127.0.0.1:7777"
        if not addr:match("^[%w%.%-]+:%d+$") then Log("AUTOTEST: bad HSMP_AUTOTEST_ADDR %s", addr); addr = "127.0.0.1:7777" end
        local wait_ms = tonumber(os.getenv("HSMP_AUTOTEST_WAIT_MS") or "") or 4000
        if AT_MODE == "join" then
            Log("AUTOTEST: joining %s", addr)
            spawn_sidecar_only(addr, settings.nick, lobby.chosen_map)
        elseif AT_MODE == "host" and os.getenv("HSMP_AUTOTEST_EXTERNAL") == "1" then
            Log("AUTOTEST: seat 1 on the external server %s", addr)
            spawn_sidecar_only(addr, settings.nick, lobby.chosen_map)
            lobby.at_admin = true
            if (os.getenv("HSMP_AUTOTEST_MAPS") or "") ~= "" then autotest_arm(wait_ms) end
        elseif AT_MODE == "host" then
            Log("AUTOTEST: hosting (listen; START comes from the command channel)")
            spawn_server_and_sidecar()
            if (os.getenv("HSMP_AUTOTEST_MAPS") or "") ~= "" then autotest_arm(wait_ms) end
        else   -- legacy "1" (stopgap): host + START
            Log("AUTOTEST: hosting")
            spawn_server_and_sidecar()
            autotest_arm(wait_ms)
        end
    end

    -- HSMPMatch bumps the return_to_lobby bus key's seq when a finished match sends us
    -- back here: reopen the lobby sub-screen (session still connected) and re-arm START,
    -- once per new seq (MX.rtl_seq is seeded at load: a leftover is never replayed).
    if MX.return_to_lobby_new() then
        lobby.launched = false
        lobby.is_ready = false
        lobby.travel_note, lobby.travel_retry_t, lobby.no_arena_logged = nil, nil, nil
        lobby.ready_rearm = true     -- a new lobby arrival: lobby_ready again (harness sync after ABORT)
        enter_screen("lobby")
        Log("returned from match -> lobby sub-screen")
    end
end

-- --- lifecycle ------------------------------------------------------------

NotifyOnNewObject("/Script/UMG.UserWidget", function(obj)
    if not obj or not obj:IsValid() then return end
    local cls = obj:GetClass():GetFName():ToString()
    if cls == "UI_Startup_Menu_C" then
        forget_widgets()          -- new menu instance: drop every cached widget ref untouched
        ExecuteWithDelay(500, inject)
    end
end)

-- Dev only (HSMP_DEV=1): F11 re-injects the menu. Widget names carry the
-- world generation, so a re-injection never reuses a live name.
if DEV and Key and Key.F11 ~= nil then
    RegisterKeyBindAsync(Key.F11, {}, function()
        menu_world_gen = menu_world_gen + 1
        forget_widgets()
        inject()
    end)
end

LoopAsync(33, function()
    pcall(function()
        ExecuteInGameThread(function()
            if not state.injected or not state.canvas then input_q = {}; return end
            local t0 = os.clock()
            drain_input()
            if state.injected then
                if state.screen_active then
                    poll_set(state.screen_widgets)
                else
                    poll_set(state.ribbons)
                    -- the mouse went to the native menu: the ribbon column lets go
                    if state.top_focus then
                        for _, n in ipairs(state.native_names) do
                            local nb = U.find_child(state.canvas, n)
                            local h = false
                            if nb then pcall(function() h = nb:IsHovered() end) end
                            if h then top_focus(nil); break end
                        end
                    end
                end
            end
            if Kit then Kit.perf_add("poll", (os.clock() - t0) * 1000) end
        end)
    end)
    return false
end)

LoopAsync(50, function()
    pcall(function() ExecuteInGameThread(pad_poll) end)
    return false
end)

-- Resize watcher (window / resolution / DPI), 4 Hz.
LoopAsync(250, function()
    pcall(function()
        ExecuteInGameThread(function()
            local ok, err = pcall(MX.resize_poll)
            if not ok then Log("resize poll failed: %s", tostring(err)) end
        end)
    end)
    return false
end)

-- Lobby auto-refresh + command results + match-state watchdog, every 500 ms
-- while the LOBBY sub-screen is active: advance pending commands, refresh
-- peers / ready / arena in place, and when the server's match leaves the
-- lobby (countdown / live) REQUEST travel to the server's arena from the
-- Director - only while the sidecar is connected.
local lobby_tick = 0
LoopAsync(500, function()
    pcall(function()
        ExecuteInGameThread(function()
            if LocalMaster then pcall(LocalMaster.tick) end      -- touches no widget
            pcall(MX.build_poll)                                 -- startup build check (no widget)
            pcall(MX.career_poll)                                -- boot career recovery, if slow
            if MX.Diag then pcall(MX.Diag.poll); pcall(MX.crash_note_sync) end   -- log folder + crash note
            CTL.flush()                                          -- command records held until connected
            if not lobby.active then MX.update_host_label(nil); return end
            lobby_tick = lobby_tick + 1

            -- The session ended under us: HSMPHud's modal explains it.
            local sstat = read_sidecar_status()
            MX.update_host_label(sstat)
            -- ("ended" counts only after THIS session's sidecar was seen live: a
            -- leftover status from a crashed session must not close a new lobby)
            -- A terminal status is not "live" (lobby_over_reason waits
            -- for the Director's modal before acting on it).
            if sstat and sstat ~= "ended" then
                if SESSION_TERMINAL[sstat] then lobby.saw_terminal = true else lobby.saw_live = true end
            end
            -- Only while this world's menu is up (the lobby or a screen
            -- opened from it). In the arena the Director + HSMPHud own a broken
            -- session; the menu never kills a sidecar that can still reconnect.
            if state.injected then
                local over, graceful = lobby_over_reason(sstat)
                if over then lobby_session_over(over, graceful); return end
                -- The server's mods: the warning, the download, the load (the lobby waits).
                if MX.Mods then
                    local ok, hold = pcall(MX.Mods.tick, state.screen_active, enter_screen)
                    if not ok then Log("server mods tick failed: %s", tostring(hold)) elseif hold then return end
                end
                -- RECONNECT worked after a lost travel left us on the main menu:
                -- back to the lobby screen.
                if lobby.lost_seen and sstat == "connected" and not state.screen_active and not conn_lost_latched() then
                    lobby.lost_seen = false
                    -- Nothing else resets the travel latch after a lost-link
                    -- travel: without this a stale "LOADING <ARENA>..." note
                    -- stays and the next countdown is never followed.
                    lobby.launched, lobby.is_ready = false, false
                    lobby.travel_note, lobby.travel_retry_t, lobby.no_arena_logged = nil, nil, nil
                    lobby.ready_rearm = true
                    Log("session back after a lost link -> lobby sub-screen")
                    enter_screen("lobby")
                end
            end

            -- The server's arena (the session snapshot) is the only source of truth
            -- for the map; chosen_map is just the display hint.
            local match_st = read_match_state()
            local srv = server_arena(match_st)
            for _, m in ipairs(MAP_PRESETS) do
                if m.path == srv then lobby.chosen_map = srv; break end
            end
            if Cmd then Cmd.tick(match_st, srv) end
            autotest_tick(match_st, srv)

            if match_st and (match_st.state == "countdown" or match_st.state == "live") then
                if sstat == "connected" then begin_combat_load(match_st) end
            else
                -- Host: request the boot arena once as a command (commands.lua
                -- resends it until the server answers; this also covers the
                -- lobby opening before the sidecar connected).
                if lobby.is_host and lobby.want_map and not lobby.map_cmd and srv ~= lobby.want_map then
                    Log("arena: server has %s, host picked %s - sending pick_arena", tostring(srv), lobby.want_map)
                    lobby.map_cmd = cmd_send("pick_arena", { arena = lobby.want_map }) or true
                end
                MX.send_host_prefs(sstat, match_st)
                lobby_ready_watch(match_st, srv)
                autotest_ready_tick(match_st)
            end

            -- Re-render the lobby in place (setters write only what changed).
            if state.screen_active == "lobby" then refresh_screen() end
        end)
    end)
    return false
end)

-- Harness command channel (HSMP_AUTOTEST only): polled every 250 ms on the game
-- thread, in the menu and in the arena alike (leave / quit work mid-match).
if AT_MODE then
    LoopAsync(250, function()
        pcall(function()
            ExecuteInGameThread(function() autotest_channel_tick() end)
        end)
        return false
    end)
end

-- Dev/CI hook: under HSMP_AUTOTEST an unfocused, script-launched window can
-- sit on the splash screens forever waiting for a key press. Skip to the
-- main menu after 6 s so unattended tests always reach it.
if AT_MODE then
    local splash_since
    LoopAsync(1000, function()
        local wn
        pcall(function() local w = UEHelpers.GetWorld(); if w and w:IsValid() then wn = w:GetFullName() end end)
        if wn and wn:find("Map_Menu_SplashScreens") then
            splash_since = splash_since or os.time()
            if os.time() - splash_since >= 6 then
                Log("AUTOTEST: skipping splash -> requesting Map_Menu_Startup")
                if Travel then Travel.request("menu", nil, "autotest_splash_skip") end
                return true
            end
        elseif wn then
            splash_since = nil
            if wn:find("Map_Menu_Startup") or wn:find("Map_Arena") then return true end
        end
        return false
    end)
end

-- offline tests
HSMP_MENU_TEST = { dispatch = dispatch, key_push = key_push, drain = drain_input, top_key = top_key,
                   state = state, lobby = lobby, settings = settings, start_check = start_check,
                   snapshot = lobby_snapshot, pad = pad, game_pid = function() return GAME_PID end }

Log("loaded — native sub-screen multiplayer menu")
