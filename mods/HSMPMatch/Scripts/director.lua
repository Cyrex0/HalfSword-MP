-- HSMPMatch / director.lua -- the Director.
--
-- The Director is the single owner of level travel in the whole HSMP mod tree
-- (the `check_travel` lint in tools/hsmp-tools fails the build on any other caller). It
-- follows the server's session state and nothing else:
--
--   Menu ─(phase ∈ countdown/live/roundover/paused with a server arena)─► Prepare
--   Prepare:   save guard ON → GI profile write + read-back (3 tries)
--   Travel:    OpenLevel(server arena)
--   WaitWorld: a NEW world key whose short name is the arena (≤ 30 s, 3 tries)
--   Spawn:     the verified spawn pipeline (each step guarded by the world key)
--                world → pawn possessed (≤ 15 s) → vitals from the CDO →
--                HSMPSync placement → HSMPLoadout kit → stand-in census →
--                (input frozen all along)
--   Ready:     report the loaded round to the server; input frozen
--   Live:      phase == live; input released (unless the server has us dead)
--   Ready/Live ─(countdown for a round this world does not serve)─► Prepare
--   any ─(phase lobby after a match / session gone / link lost 12 s)─► TravelMenu
--   TravelMenu: return-to-lobby flag → GI restore → OpenLevel(menu) → Menu
--               (save guard held ON until the menu world arrived)
--
-- Session source: get_session() (D.session_reader below): the typed `link` and
-- `session` records the sidecar copies into shared memory (the server's
-- snapshot as received) and the sidecar heartbeat in the segment header.
--
-- Travel requests from HSMPMenu (docs/development/subsystems/director-contract.md): polled from the
-- bus key travel_request every tick, acked in travel_ack in the same tick,
-- heartbeat in the bus key director every second. A request is a
-- WANT, never a command: the Director acts on the server's state only.
--
-- This file is pure logic over an `env` table (UE access, IPC, files, clock), so it
-- runs offline under mlua (`hsmp-tools lua-test director`). make_ue_env()
-- at the bottom builds the real env; only main.lua calls it.
--
-- Threading: every entry point (tick, hook callbacks) runs on the game thread.

local D = { VERSION = 1 }

-- The shared session view (shared/hsmp_session.lua, copied next to this file at
-- deploy; the source tree's copy offline): the typed `link` / `session` records
-- the sidecar copies into shared memory, and the sidecar heartbeat age.
D.HS = (function()
    local ok, m = pcall(require, "hsmp_session")
    if ok and type(m) == "table" and m.view then return m end
    local src = ((debug and debug.getinfo and debug.getinfo(1, "S").source) or ""):gsub("^@", "")
    local dir = src:match("^(.*)[/\\]") or "."
    for _, p in ipairs({ dir .. "/hsmp_session.lua", dir .. "/../../shared/hsmp_session.lua" }) do
        local ok2, m2 = pcall(dofile, p)
        if ok2 and type(m2) == "table" and m2.view then return m2 end
    end
    return nil
end)()
D.SpawnReady = (function()
    local ok, module = pcall(require, "hsmp_spawn_ready")
    if ok and type(module) == "table" then return module end
    local source = (debug.getinfo(1, "S").source or ""):gsub("^@", "")
    local directory = source:match("^(.*)[/\\]") or "."
    for _, path in ipairs({ directory .. "/hsmp_spawn_ready.lua", directory .. "/../../shared/hsmp_spawn_ready.lua" }) do
        local loaded, value = pcall(dofile, path)
        if loaded and type(value) == "table" then return value end
    end
end)()

-- The IPC facade the Director talks through: env.ipc (tests) or this Lua
-- state's shared/hsmp_ipc.lua (HSMP_IPC). nil = IPC unavailable.
local function ipc_of(env) return (env and env.ipc) or rawget(_G, "HSMP_IPC") end
D.ipc_of = ipc_of

-- A code of a generated table (S.ENUMS.<t>.<NAME>), or `def`.
local function code(env, t, name, def)
    local I = ipc_of(env)
    local S = I and I.S
    local e = S and S.ENUMS and S.ENUMS[t]
    return (e and e[name]) or def
end

D.MENU_WORLD = "Map_Menu_Startup"
D.STATES = { "Menu", "Prepare", "Travel", "WaitWorld", "Spawn", "Ready", "Live", "TravelMenu" }

-- Phases (normalised, lower case) in which the server wants us in its arena.
D.ACTIVE = { loading = true, countdown = true, live = true, roundover = true, paused = true }

-- Timeouts and limits (seconds unless noted).
D.T = {
    gi_tries         = 3,      -- Prepare: GI verify attempts before giving up
    gi_retry_s       = 5,      -- Prepare: wait after a failed Prepare
    open_tries       = 3,      -- Travel: OpenLevel attempts per Prepare
    world_s          = 30,     -- WaitWorld: max wait for the new world
    debounce_s       = 5,      -- watchdog travel debounce
    pawn_s           = 15,     -- Spawn: pawn possessed
    vitals_s         = 4,      -- Spawn: CDO readable + values verified
    place_wait_s     = 20,     -- Spawn: no order for this round/arena -> skip placement
    place_retry_s    = 8,      -- Spawn: no verified placement yet -> ask HSMPSync to re-place (once)
    place_s          = 20,     -- Spawn: an order exists but no verified placement -> spawn_timeout
    place_tol_cm     = 100,    -- the pawn must still be this close (XY) to HSMPSync's verified spot (= HSMPSync verify_tol_cm)
    kit_absent_s     = 4,      -- Spawn: no kit_status at all -> kit check unavailable
    kit_s            = 15,     -- Spawn: kit not verified -> kit_error (non-fatal)
    kit_hold_s       = 1.5,    -- Spawn: a verified kit must have held this long (native re-arm window)
    kit_world_slack_s = 0.5,   -- Spawn: a kit_status "t" older than the pipeline start - this is an old world's
    census_s         = 8,      -- Spawn: stand-in census settles (extras == 0)
    census_missing_s = 18,     -- Spawn: ... but a stand-in still missing is waited for this long
    census_live_s    = 5,      -- Live: willie_census{at="live"} cadence
    -- Connection state machine (docs/development/subsystems/director.md):
    stall_s          = 3.0,    -- link rx age above this = link down (the server sends >= 1 packet/s)
    sidecar_stale_s  = 6.0,    -- no sidecar heartbeat this long = the sidecar is hung/dead
    resume_window_s  = 35,     -- in a match, link down this long -> give up (server pause grace 30 s + margin)
    resume_settle_s  = 1.0,    -- link back: trust the session records only after this long (fresh snapshots)
    rematch_hold_s   = 15,     -- REMATCH: stay in the arena this long in the lobby waiting for the host's START
    rematch_cmd_s    = 2.0,    -- REMATCH: resend ready/start this often
    session_gone_s   = 3,      -- session process gone mid-match -> menu
    session_stale_s  = 45,     -- no sidecar heartbeat this long = no session process, whatever its
                               -- status; > sidecar_stale_s + resume_window_s (the modal comes first)
    live_s           = 15,     -- "live" needs a heartbeat within this long (= HSMPMatch MP_ALIVE_S)
    ping_s           = 1.0,    -- game_status cadence (~1 Hz)
    hb_s             = 1.0,    -- bus `director` heartbeat
    menu_restore_s   = 5,      -- in the menu with no session this long -> restore a leftover GI backup
    min_native_foes  = 1,      -- the arena's native flow needs >= 1 foe; extras are hidden stand-ins
}
D.T.lost_link_s = D.T.resume_window_s   -- old name (HUD / docs)

-- Sidecar statuses that never recover in the same sidecar process.
D.TERMINAL = { kicked = true, replaced = true, server_closed = true, rejected = true }

-- The MP GameInstance profile (MP_GI_PROFILE): one table for the
-- arena GI values, real (space-containing) property names. `restore=false`:
-- never put back on leaving MP. "Free Mode Foes Amount" is filled from the
-- roster at Prepare time.
D.MP_GI_PROFILE = {
    { name = "Player Just Died",       value = false, restore = false },
    { name = "Current Game Mode Enum", value = 0 },
    { name = "Current Combat Mode",    value = 0 },
    { name = "Current Play Mode",      value = 0 },
    { name = "Free Mode Activated",    value = false },
    { name = "Rounds To WIn",          value = 0 },
    { name = "Rounds Won",             value = 0 },
    { name = "Free Mode Foes Amount",  value = "foes" },
    { name = "Free Mode Carnage",      value = false },
    { name = "Free Mode Brawling",     value = false },
    { name = "Free Mode Blossfechten", value = false },
}

-- GI "Player Body Condition" (FStr_Character_Body_Condition, CXXHeaderDump):
-- the spawner hands it to the player's new pawn as "Start Body Condition".
-- A death in round N (any real death, or a test's debug kill) writes the
-- wounds into it and the arena's Save Game / Load Game round trip (diverted
-- to the session slot) carries them into round N+1: that fighter would start
-- every later round Downed at consciousness ~20. Healed to the CDO values
-- with the GI profile, before every travel and the slot seed.
D.BODY_COND = "Player Body Condition"
D.BODY_COND_FIELDS = {
    "HeadHealth_2_61859BB444171EF8952E0FA5DD8628EE", "NeckHealth_4_C658DC6A4BD1988C40F1A5B3C4F8F4EE",
    "ArmRHealth_9_A65DD4C14ACBF6030A2B3AAD90FD0CFD", "ArmLHealth_11_32345C31454A51B3CDE618918B9574F6",
    "BodyUpperHealth_16_F71EA0C742135DC3B4F71EA3FEF07C46", "BodyLowerHealth_18_37C008FF4FA0C0E5F5E09C9F0C174FE3",
    "LegRHealth_13_D50D4E174859A541DBEA66963D162E12", "LegLHealth_15_41C766B5460596C0804EA5B4B8F8EB36",
}

-- Vitals restored from the Willie_BP CDO (career-save wounds; same list as HSMPCombat).
D.VITALS_FIELDS = {
    "Health", "Head Health", "Neck Health", "Body Upper Health", "Body Lower Health",
    "Back Health", "Arm_R Health", "Arm_L Health", "Leg_R Health", "Leg_L Health",
    "Head Health (Crush)", "Consciousness", "Consciousness 2 (Legs)",
    "Bleeding", "Pain", "Stamina", "Exhaustion",
}
D.VITALS_RESET_FUNCS = {
    "Reset Sustained Damage", "Reset Last Damage Taken", "Reset Blood Bleed", "Reset Latest Complex Damage",
}

-- Engine cvars forced at runtime (env.apply_cvars; the IO-1 hair crash,
-- docs/development/halfsword/io-dispatcher-crash.md). Hair cards instead of
-- strands: the strands bulk data is never read. Engine.ini and the launch
-- argument set both before the first groom loads; this is the backstop.
D.RUNTIME_CVARS = { { "r.HairStrands.Streaming", "0" }, { "r.HairStrands.UseCardsInsteadOfStrands", "1" } }
D.IO1_CVARS = 2   -- the first entries of RUNTIME_CVARS that are read back
-- Test rigs only: HSMP_TEST_CVARS="r.VSync=0;t.MaxFPS=60" (set by mp_test.ps1).
-- Two game instances on one GPU with VSync on drop to exactly 30 fps on the
-- heavier arenas (VSync halves a missed 16.7 ms frame): the pose sender then
-- streams at 30 Hz and the receiver's jitter buffer grows to 85-115 ms, an
-- artefact of the rig, not of a player's PC.
-- Only plain "name=value" pairs of a fixed allow-list are taken.
D.TEST_CVAR_OK = { ["r.VSync"] = true, ["t.MaxFPS"] = true }
do
    local s = os.getenv("HSMP_TEST_CVARS")
    if s and s ~= "" then
        for name, val in s:gmatch("([%w%._]+)%s*=%s*([%w%.%-]+)") do
            if D.TEST_CVAR_OK[name] then D.RUNTIME_CVARS[#D.RUNTIME_CVARS + 1] = { name, val } end
        end
    end
end

-- Console-open fallback paths (OpenLevel by short FName is the normal route).
D.ARENA_PATH = function(arena) return "/Game/Maps/Arenas/" .. arena .. "/" .. arena end

-- ---------------------------------------------------------------------------
-- small helpers

local function jstr(s)
    if s == nil then return "null" end
    return '"' .. tostring(s):gsub('[%c"\\]', function(c)
        if c == '"' then return '\\"' elseif c == "\\" then return "\\\\" end
        return string.format("\\u%04x", c:byte())
    end) .. '"'
end
D.jstr = jstr

local function jfield_str(line, k) return line and line:match('"' .. k .. '"%s*:%s*"([^"]*)"') end
local function jfield_num(line, k) return line and tonumber(line:match('"' .. k .. '"%s*:%s*(%-?[%d%.]+)')) end
local function jfield_bool(line, k)
    local v = line and line:match('"' .. k .. '"%s*:%s*(%a+)')
    if v == "true" then return true elseif v == "false" then return false end
    return nil
end
D.jfield_str, D.jfield_num, D.jfield_bool = jfield_str, jfield_num, jfield_bool

-- A complete one-line JSON object: trimmed text starts with '{' and ends with
-- '}'. An empty, torn or partial read (a writer mid-rewrite, the non-atomic
-- fallbacks) is not complete and must be treated as "no new evidence".
function D.complete(line)
    if type(line) ~= "string" then return false end
    local t = line:match("^%s*(.-)%s*$")
    return t:sub(1, 1) == "{" and t:sub(-1) == "}"
end


local function is_menu(w) return w ~= nil and w:find("^Map_Menu_") ~= nil end
local function is_hub(w) return w ~= nil and (w:find("Tavern") ~= nil or w:find("Map_Hub") ~= nil) end
-- A server arena we are willing to travel to (never the hub or a menu).
local function valid_arena(a)
    return type(a) == "string" and a:match("^[%w_]+$") ~= nil and not is_hub(a) and not is_menu(a)
end
D.valid_arena, D.is_hub, D.is_menu = valid_arena, is_hub, is_menu

-- ---------------------------------------------------------------------------
-- The session reader: one normalised table from the typed records the sidecar
-- copies into shared memory (slot `link`: status, my peer id, admin, link
-- state, kick / reject reason; slot `session`: the server's snapshot) and the
-- sidecar's heartbeat in the segment header (shared/hsmp_session.lua):
--   source        "session"
--   exists        an MP session process (sidecar) exists for this game
--   live          connected / reconnecting with a fresh heartbeat
--   connected     the sidecar is connected to the server (and beating)
--   status        the sidecar status name ("connected", "kicked", ...; "none")
--   sidecar_age   seconds since the sidecar's last heartbeat; sidecar_hung
--   rejected / terminal / terminal_reason (link.reason)
--   rx_age, stalled (link state STALLED), link_attempt
--   epoch, match_id (nil while unknown / no session snapshot)
--   phase         lobby|countdown|live|roundover|paused|match_over|none (the
--                 server's phase; loading counts as countdown)
--   arena         the server's arena (frozen in a match) or nil
--   round; pending_round = the round a fresh load serves (round + 1)
--   my_id, my_nick, nicks{id->nick}, is_admin
--   roster        { {id, nick, role, alive, wins} } (connected seats, server order)
--   remote_fighters  fighter seats other than mine
--   order, wins{}, alive{}, waiting{}, countdown, best_of, last_winner, reason
--   spawn_order(round, arena) -> {spawn_id, slot, x, y, z, yaw} | nil
--   respawn       deathmatch: {spawn_id, life} while the server orders us back into the
--                 running round (the `mode` record says respawning, the roster carries a
--                 respawn order: spawn_id round << 8 | 0x80 | life), else nil
--   match_stale   the snapshot is a previous session's
--
-- Liveness is the header heartbeat (never "the content changed recently"):
--   exists     = a link record, the sidecar attached and beating within
--                session_stale_s, its status not "ended";
--   connected  = exists, status connected, heartbeat within sidecar_stale_s;
--   live       = status connected / reconnecting, heartbeat within live_s.
-- A detached sidecar counts as gone only after >= absent_reads reads over
-- >= absent_s (one missed look is no session end).
D.T.absent_reads, D.T.absent_s = 2, 1.0

-- Why a terminal sidecar status ended the session, in words for the player.
function D.terminal_reason(link, status)
    local r = type(link) == "table" and link.reason or nil
    if r == "" then r = nil end
    if status == "kicked" then return r or "kicked by the host"
    elseif status == "server_closed" then return r or "the server closed"
    elseif status == "rejected" then return r or "rejected by the server"
    elseif status == "replaced" then return "this player joined the server from another game" end
    return nil
end

function D.session_reader(env, state_dir)
    local HS = D.HS
    local sess = {
        source = "session", exists = false, connected = false, phase = "none", round = 0,
        countdown = 0, best_of = 3, arena = nil, my_id = 0, my_nick = "You", nicks = {},
        order = {}, wins = {}, alive = {}, waiting = {}, last_winner = 0, reason = "",
        roster = {}, remote_fighters = 0, pending_round = 1, status = "none",
    }
    local st = { present = false, absent_n = 0, absent_since = nil, link = nil, age = nil,
                 ver = nil, stale_ver = nil, view = nil, was_exists = false }
    -- The session snapshot present when this reader starts belongs to an
    -- earlier session of this game process: "stale" until the server sends a
    -- new one (a new record version).
    do
        local I = ipc_of(env)
        if I and I.rec then
            local _, v = I.rec("session")
            st.stale_ver = v
        end
    end
    -- A dead sidecar never sends a snapshot again; its last one ("live")
    -- must not send a later single-player travel into the arena. Dir:lose()
    -- calls this; clear_match_stale once the link is fresh again.
    function sess.mark_match_stale() st.stale_ver = st.ver end
    function sess.clear_match_stale() st.stale_ver = nil end
    function sess.spawn_order(round, arena)
        local v = st.view
        local sp = v and v.spawns and v.spawns[sess.my_id]
        if not sp or arena ~= sess.arena or (sp.spawn_id >> 8) ~= round then return nil end
        return { spawn_id = sp.spawn_id, slot = sp.slot, x = sp.x, y = sp.y, z = sp.z, yaw = sp.yaw }
    end

    return function()
        local now = env.now()
        local I = ipc_of(env)
        local o = { ipc = I, clock = env.now }
        local link = (I and I.rec and HS) and (I.rec("link")) or nil
        local age = (link ~= nil and HS) and HS.hb_age(o) or nil
        if link ~= nil and age ~= nil then
            st.present, st.absent_n, st.absent_since = true, 0, nil
            st.link, st.age = link, age
        else
            st.absent_n = st.absent_n + 1
            st.absent_since = st.absent_since or now
            if link == nil or not st.present
                or (st.absent_n >= D.T.absent_reads and now - st.absent_since >= D.T.absent_s) then
                st.present, st.link, st.age = false, link, nil
            end
        end
        local lk, hb = st.link, st.age
        local status = (st.present and lk and HS) and HS.status_name(lk, o) or nil
        local exists = st.present and hb ~= nil and status ~= nil and status ~= "ended" and hb <= D.T.session_stale_s
        sess.exists = exists
        sess.status = status or "none"
        sess.sidecar_age = hb or 0
        sess.live = st.present and hb ~= nil and (status == "connected" or status == "reconnecting") and hb <= D.T.live_s
        -- A sidecar that stopped beating is not connected, whatever its last status says.
        sess.sidecar_hung = exists and hb > D.T.sidecar_stale_s
        sess.connected = exists and status == "connected" and not sess.sidecar_hung
        sess.rejected = nil
        sess.terminal, sess.terminal_reason = nil, nil
        if exists and D.TERMINAL[status] then
            sess.terminal = status
            sess.terminal_reason = D.terminal_reason(lk, status)
        end
        if st.present and status == "rejected" then sess.rejected = D.terminal_reason(lk, "rejected") end
        -- Link health from the sidecar transport (the link record's state).
        local lstate = (exists and lk and HS) and HS.link_state_name(lk, o) or nil
        sess.rx_age = (exists and lk) and (lk.rx_age_ms or 0) / 1000 or nil
        sess.stalled = sess.connected and lstate == "stalled"
        sess.link_attempt = (exists and lk) and lk.attempt or nil
        if lk then
            if (lk.my_peer_id or 0) ~= 0 then sess.my_id = lk.my_peer_id end
            sess.is_admin = lk.is_admin == true
        end
        local cfg = env.read and state_dir and env.read(state_dir .. "/.settings.json")
        if cfg then sess.my_nick = cfg:match('"nick"%s*:%s*"([^"]+)"') or sess.my_nick end

        -- The session snapshot (only while a session process exists).
        if st.was_exists and not exists then st.stale_ver = st.ver end
        st.was_exists = exists
        local snap, ver = nil, nil
        if exists and I and I.rec then snap, ver = I.rec("session") end
        if snap ~= nil then st.ver = ver end
        sess.match_stale = (snap ~= nil and ver == st.stale_ver) or nil
        if sess.match_stale then snap = nil elseif snap ~= nil then st.stale_ver = nil end
        local v = (snap ~= nil and HS) and HS.view(o) or nil
        st.view = v
        if v then
            sess.epoch = (v.epoch ~= 0) and v.epoch or nil
            sess.match_id = v.match_id
            sess.phase = v.state
            sess.round = v.round or 0
            sess.countdown = v.countdown_s or 0
            sess.best_of = v.best_of or sess.best_of
            sess.last_winner = v.last_winner or 0
            sess.reason = v.reason or ""
            sess.order, sess.wins, sess.alive, sess.roster = {}, {}, {}, {}
            for _, r in ipairs(v.rows) do
                if r.connected and r.peer_id ~= 0 then
                    local id = r.peer_id
                    sess.order[#sess.order + 1] = id
                    sess.wins[id], sess.alive[id] = r.wins, r.alive
                    if r.nick ~= "" then sess.nicks[id] = r.nick end
                    sess.roster[#sess.roster + 1] = { id = id, nick = r.nick, role = (r.role == 1) and "spectator" or "fighter",
                        alive = r.alive, wins = r.wins }
                end
            end
            sess.waiting = {}
            for _, id in ipairs(v.waiting_on or {}) do sess.waiting[#sess.waiting + 1] = id end
            sess.arena = v.arena
            sess.remote_fighters = v.remote_fighters or 0
            -- Deathmatch respawn order (shared/hsmp_session.lua HS.mode()).
            sess.respawn = nil
            local m = HS and HS.mode and HS.mode(o) or nil
            local row = m and m.rows[sess.my_id]
            sess.life = row and m.match_id == sess.match_id and m.round == sess.round and row.life or nil
            local sp = v.spawns and v.spawns[sess.my_id]
            if row and row.respawning and sp and (sp.spawn_id & 0x80) ~= 0 and (sp.spawn_id >> 8) == sess.round then
                sess.respawn = { spawn_id = sp.spawn_id, life = row.life }
            end
        else
            sess.respawn = nil
            if not exists then sess.epoch, sess.match_id = nil, nil end
            sess.phase, sess.arena, sess.roster, sess.order = "none", nil, {}, {}
            sess.remote_fighters = 0
        end
        -- A load during countdown / live / roundover serves round + 1.
        sess.pending_round = sess.round + 1
        return sess
    end
end
-- Old name (HSMPMatch main.lua, tests).
D.match_json_reader = D.session_reader

-- ---------------------------------------------------------------------------
-- The Director.
--
-- env (all functions; see make_ue_env for the real one, the mlua tests for a mock):
--   now() -> s (monotonic)          wall() -> unix s
--   read(path) -> first line|nil    write_atomic(path, s)   remove(path)   (real files, kit_status)
--   ipc (optional): the IPC facade (default: this Lua state's HSMP_IPC)
--   world() -> { ok=bool, short=, key= }   ok=false: level change pending / no world
--   open_level(name) -> bool        (OpenLevel, console fallback)
--   pawn() -> pawn|nil              pawn_id(p) -> string
--   pawn_get(p, k)  pawn_set(p, k, v) -> bool   pawn_call(p, fn_name)
--   pawn_loc(p) -> x, y, z
--   cdo_vitals() -> { [field]=number } | nil
--   gi_get(k)  gi_set(k, v) -> bool
--   census(p) -> visible_other_willies, detail_string
--   freeze(p, on)                   game_input()   (input mode game-only, cursor off)
--   log(fmt, ...)                   ev(name, fields)
--   sg_active() -> bool|nil  sg_force(true|nil)  sg_seed() -> ok, detail   (optional: save guard)
-- opts: state_dir, get_session

local Dir = {}
Dir.__index = Dir

function D.new(env, opts)
    local self = setmetatable({}, Dir)
    self.env = env
    self.dir = opts.state_dir
    self.get_session = opts.get_session or D.match_json_reader(env, opts.state_dir)
    self.state = "Menu"
    self.state_since = env.now()
    self.wkey, self.wshort = nil, nil
    self.target = nil              -- arena we are travelling to / serving
    self.loaded_for = 0            -- round the current world load serves
    self.ready_round = 0           -- loaded_for once the pipeline verified it
    self.reloaded_for = nil        -- round_key() we already issued a reload for
    self.loaded_key = nil          -- round_key() of loaded_for
    self.match_gen = 0             -- match context counter (track_match)
    self.mt = { match_id = nil, epoch = nil, in_m = false }
    self.in_match = false
    self.load_error = nil          -- reported in the ping until the next world
    self.last_action = -1e9        -- now() of our last travel (debounce)
    self.self_travel = false       -- inside our own OpenLevel call
    self.counters = { travel = 0, native_rewritten = 0, requests = 0, refused = 0 }
    self.next_ping, self.next_hb = 0, 0
    self.frozen_id = nil
    self.live_release = nil       -- pure context of the life whose input was released
    self.req_seq = nil             -- last seen travel_request seq
    self.link_lost_at, self.gone_at, self.menu_since = nil, nil, nil
    -- The single connection state shown to the player (docs/development/subsystems/director.md):
    --   ok | reconnecting (in a match, link down: overlay, frozen, no travel)
    --   | lost (latched: one travel to the menu, a modal, no automatic re-entry)
    --   | notice (informational modal, e.g. "the server restarted").
    -- up_since: when the link last came up (nil while down); -1e9 = up from the start.
    self.conn = { state = "ok", seq = 0, up_since = -1e9, latched = false }
    self.epoch = nil               -- server instance (S2CSession epoch) we play on
    self.terminal_seen = nil       -- terminal sidecar status already handled
    self.cmd_n = 0
    -- Startup: never replay a request left over from an earlier run (bus keys
    -- outlive a Lua reload within one game process).
    local r = self:bus("travel_request")
    self.req_seq = (r and r.seq) or 0
    local u = self:bus("ui_request")
    self.ui_seq = (u and u.seq) or 0
    -- HSMPSync ignores spawn_request seqs <= the one it last saw (seeded
    -- at its own start); continue from the leftover.
    local sr = self:bus("spawn_request")
    self.place_req_seq = (sr and sr.seq) or 0
    -- HSMPMenu reopens the lobby once per new return_to_lobby seq.
    local rl = self:bus("return_to_lobby")
    self.rtl_seq = (rl and rl.seq) or 0
    env.log("Director v%d up (state=Menu, last request seq=%d)", D.VERSION, self.req_seq)
    return self
end

-- A typed game-local bus key (shared memory; records of schema session.rs), or nil.
function Dir:bus(key)
    local I = ipc_of(self.env)
    if not I or not I.bus_table then return nil end
    local t = I.bus_table(key)
    return t
end
function Dir:bus_put(key, t)
    local I = ipc_of(self.env)
    if not I or not I.bus_put then return false end
    local ok, err = I.bus_put(key, t)
    if not ok and not self.bus_err_logged then
        self.bus_err_logged = true
        self.env.log("director: bus_put(%s) refused: %s", key, tostring(err))
    end
    return ok
end
-- One G2S record (shared memory ring; the facade queues it when the ring is full).
function Dir:send(kind, t)
    local I = ipc_of(self.env)
    if not I or not I.send then return nil end
    return I.send(kind, t)
end

-- HSMPMenu's lobby hand-back: a new return_to_lobby seq (bus key).
function Dir:return_to_lobby()
    self.rtl_seq = (self.rtl_seq or 0) + 1
    self:bus_put("return_to_lobby", { seq = self.rtl_seq })
end

function Dir:set_state(s, why)
    if s == self.state then return end
    self.env.log("director: %s -> %s%s", self.state, s, why and (" (" .. why .. ")") or "")
    self.state, self.state_since = s, self.env.now()
end

function Dir:ev(name, fields) pcall(self.env.ev, name, fields or {}) end

-- A load error is reported in the ping until the next world. The FIRST error
-- of a world load is kept (it is the root cause); later ones are only logged.
function Dir:error(code, detail)
    self.errors_logged = self.errors_logged or {}
    if not self.errors_logged[code] then
        self.errors_logged[code] = true
        self.env.log("director ERROR %s: %s", code, tostring(detail or ""))
    end
    if not self.load_error then
        local source = self.pipe or self.ready_context
        local s = self.sess or {}
        self.load_context = { match_id = source and source.match_id or s.match_id or 0,
            round = source and source.round or s.pending_round or ((s.round or 0) + 1),
            match_gen = self.match_gen }
    end
    self.load_error = self.load_error or code
end

-- Where the server wants us right now: an arena name, D.MENU_WORLD, or nil
-- (stay). Nil without a session process: single-player is never touched.
-- `raw`: the last known server state even when it may be stale (only the
-- native-travel rewrite uses it). Otherwise the session records are trusted
-- only while the link is up and settled, and never while a lost connection
-- is latched: a stale "live" snapshot must never send us back into the arena
-- (that turns into a reconnect loop).
function Dir:want(s, raw)
    s = s or self.sess
    if not s or not s.exists then return nil end
    -- We asked the sidecar to leave (LEAVE / BACK TO MENU): its last state
    -- ("live") is not ours to follow any more while it says goodbye.
    if self.leaving then return nil end
    if not raw then
        if self.conn.latched then return nil end
        if not self:link_fresh(s) then return nil end
    end
    if D.ACTIVE[s.phase] and valid_arena(s.arena) then return s.arena end
    if s.phase == "lobby" or s.phase == "match_over" then
        if s.phase == "match_over" and self.in_match then return nil end   -- stay for the result screen
        if s.phase == "lobby" and self:rematch_holding() then return nil end
        return D.MENU_WORLD
    end
    return nil
end

-- Where a NATIVE travel (OpenLevel pre-hook, soft-travel post-hook) must go.
-- want(s, true), plus: while we stay in the arena on purpose (the result
-- screen in match_over, a REMATCH hold in the lobby) want() says nil =
-- "stay", which alone would let a native travel (the win/lose flow's Tavern)
-- through.
-- Staying means the arena we are in: the native travel is rewritten to it.
function Dir:native_want(s)
    -- Outside a match with the link down (a lost session, RECONNECT
    -- pending or rejoining) the raw server phases are no reason to hijack a
    -- single-player travel into the MP arena (and its MP GI profile).
    if s and s.exists and not self.in_match and not self:link_up(s) then return nil end
    local want = self:want(s, true)
    if want ~= nil or not s.exists or self.leaving then return want end
    local hold = (s.phase == "match_over" and self.in_match) or (s.phase == "lobby" and self:rematch_holding())
    if not hold then return nil end
    if valid_arena(self.wshort) then return self.wshort end
    if valid_arena(self.target) then return self.target end
    return nil
end

-- True while an MP match runs for this game: the session process exists
-- (connected OR riding out a reconnect) and the server is in a match phase
-- or showing its result. Requiring `connected` would lift the native
-- win-flow pinning (HSMPMatch suppress_native_end_flow) during a reconnect.
function D.match_running(s)
    return s ~= nil and s.exists == true and not s.terminal
        and (D.ACTIVE[s.phase] == true or s.phase == "match_over")
end

-- --- connection state machine -------------------------------------------------

function Dir:link_up(s)
    return s.exists and s.connected and not s.stalled
end

-- Up, and up long enough that the session records are fresh snapshots.
function Dir:link_fresh(s)
    local c = self.conn
    return self:link_up(s) and c.up_since ~= nil and self.env.now() - c.up_since >= D.T.resume_settle_s
end

D.LOST_TEXT = {
    link_lost       = { "CONNECTION LOST", "No answer from the server for %d s." },
    sidecar_stopped = { "CONNECTION LOST", "The network helper (hsmp-sidecar) stopped responding." },
    kicked          = { "YOU WERE KICKED", "%s" },
    server_closed   = { "SERVER CLOSED", "%s" },
    rejected        = { "CONNECTION REJECTED", "%s" },
    replaced        = { "DISCONNECTED", "You joined this server from another game." },
    server_restarted = { "SERVER RESTARTED", "The server restarted and the match was reset. You are back in the lobby." },
    match_ended_away = { "MATCH ENDED", "The match ended while you were reconnecting." },
}

-- Publish the connection state (bus key conn_state, read by HSMPHud and HSMPMenu).
function Dir:write_conn()
    local c, env = self.conn, self.env
    local now = env.now()
    local elapsed = c.since and (now - c.since) or 0
    local remaining = (c.state == "reconnecting") and math.max(0, D.T.resume_window_s - elapsed) or 0
    local actions = {}
    for i = 1, 4 do actions[i] = (c.actions and c.actions[i]) or "" end
    self:bus_put("conn_state", {
        wall = env.wall(), seq = c.seq, elapsed_s = math.floor(elapsed), remaining_s = math.ceil(remaining),
        window_s = D.T.resume_window_s, attempt = (self.sess and self.sess.link_attempt) or -1,
        in_match = self.in_match == true, latched = c.latched == true, state = c.state, reason = c.reason or "",
        sidecar = (self.sess and self.sess.status) or "", actions = actions, title = c.title or "", text = c.text or "",
    })
    c.written_at = now
end

function Dir:set_conn(state, fields)
    local c = self.conn
    local changed = c.state ~= state or (fields and fields.reason ~= c.reason)
    c.state = state
    for k, v in pairs(fields or {}) do c[k] = v end
    if state == "ok" then c.reason, c.title, c.text, c.actions, c.since, c.gone_seen = nil, nil, nil, nil, nil, nil end
    if changed then
        c.seq = c.seq + 1
        self.env.log("director: connection %s%s", state, c.reason and (" (" .. c.reason .. ")") or "")
        self:ev("conn_state", { state = state, reason = c.reason, seq = c.seq })
    end
    self:write_conn()
end

-- One transition out of a broken session: latch, publish the reason, travel
-- to the menu once (no lobby hand-back: the session is gone). Never retried.
function Dir:lose(reason, detail)
    local t = D.LOST_TEXT[reason] or { "CONNECTION LOST", "%s" }
    local text = string.format(t[2], (reason == "link_lost") and D.T.resume_window_s or tostring(detail or reason))
    local can_reconnect = (reason == "link_lost")
    self.env.log("director: connection LOST (%s): %s", reason, text)
    self:ev("travel_reason", { reason = reason, text = text })
    self.conn.gone_seen = nil
    self:set_conn("lost", { reason = reason, title = t[1], text = text, latched = true,
        since = self.env.now(), actions = can_reconnect and { "reconnect", "menu" } or { "menu" } })
    self.rematch = nil
    -- The dead sidecar's last session snapshot never changes again: from
    -- now on it is stale (phase none), so a later native travel is not
    -- rewritten into its arena while the snapshot still looks alive.
    -- The same for a lost link: until the link is fresh again the last
    -- snapshot ("live") is not where a single-player travel goes.
    if (reason == "sidecar_stopped" or reason == "link_lost") and self.sess and self.sess.mark_match_stale then
        pcall(self.sess.mark_match_stale)
    end
    if self.in_match or (self.wshort and not is_menu(self.wshort)) then
        self:travel_menu(string.format("connection lost: %s - %s", reason, text), { no_flag = true })
    end
    self.in_match = false
end

-- Informational modal; the session itself is fine (server restart, match over).
function Dir:notice(reason)
    local t = D.LOST_TEXT[reason]
    self.env.log("director: notice %s", reason)
    self:set_conn("notice", { reason = reason, title = t[1], text = t[2], latched = false,
        since = self.env.now(), actions = { "ok" } })
end

-- Runs every tick before the travel logic. Returns true when the travel logic
-- must not run this tick (link down in a match: freeze in place, no travel).
function Dir:step_conn(w, s)
    local env, c = self.env, self.conn
    local now = env.now()
    local up = self:link_up(s)
    -- The settle delay applies after the link was down in a session (a
    -- reconnect, a fresh connect); a session first seen already connected is
    -- fresh at once.
    if not s.exists then c.was_down = false elseif not up then c.was_down = true end
    if up then
        if c.up_since == nil then
            c.up_since = c.was_down and now or -1e9
            if c.state == "reconnecting" then
                env.log("director: link back after %.1f s; waiting %.1f s for fresh session state",
                    now - (c.since or now), D.T.resume_settle_s)
            end
        end
    else
        c.up_since = nil
    end
    local fresh = self:link_fresh(s)

    -- A new terminal status (kick, server closing, replaced, rejected) ends
    -- the session at once, in a match or in the lobby.
    if s.terminal and self.terminal_seen ~= s.terminal then
        self.terminal_seen = s.terminal
        self:lose(s.terminal, s.terminal_reason)
        return true
    elseif not s.terminal and self.terminal_seen and s.exists and s.status ~= "none" then
        -- A terminal sidecar never recovers: a non-terminal status is a NEW
        -- session (the menu joined again) and clears the latch.
        self.terminal_seen = nil
        if c.latched then self:set_conn("ok", { latched = false }) end
    end

    -- A "sidecar_stopped" modal outlives the dead sidecar (its link record
    -- ages out): it ends when the player answers it, or when a session process
    -- exists again (a new HOST / JOIN, or the hung sidecar came back).
    if s.exists and c.latched and c.reason == "sidecar_stopped" and c.gone_seen then
        c.gone_seen = nil
        env.log("director: a session process exists again; the sidecar_stopped modal is closed")
        self:set_conn("ok", { latched = false })
    end

    -- Session process gone: the latch and any modal end with it.
    if not s.exists then
        -- The terminal status was this (now gone) process's: the next
        -- session's identical rejection / kick is a new event and gets its modal.
        self.terminal_seen = nil
        if c.latched and c.reason == "sidecar_stopped" then
            c.gone_seen = true          -- the dead sidecar IS the reason: keep the modal
            self.gone_since = nil
        elseif c.latched or c.state == "notice" then
            self.gone_since = self.gone_since or now
            if now - self.gone_since > 1.0 then
                self.gone_since = nil
                self:set_conn("ok", { latched = false })
            end
        end
        if c.state == "reconnecting" then self:set_conn("ok") end
        return false
    end
    self.gone_since = nil

    -- Server instance: a new epoch while we played = the server restarted.
    if fresh and s.epoch then
        if self.epoch == nil then
            self.epoch = s.epoch
        elseif s.epoch ~= self.epoch then
            env.log("director: server epoch %s -> %s (server restarted)", self.epoch, s.epoch)
            self.epoch = s.epoch
            if self.in_match or c.state == "reconnecting" then
                self.restarted = true
            end
        end
    end

    if c.latched then return false end

    if c.state == "reconnecting" then
        if fresh then
            local away = now - (c.since or now)
            env.log("director: link restored after %.1f s (phase=%s round=%d)", away, tostring(s.phase), s.round or 0)
            -- A link_lost marked the session snapshot stale; the session's
            -- own state counts again now that it is fresh
            if self.sess and self.sess.clear_match_stale then pcall(self.sess.clear_match_stale) end
            self:ev("resume", { ok = true, away_s = away, phase = s.phase, round = s.round })
            self:set_conn("ok")
            if self.restarted then
                self.restarted = nil
                self:notice("server_restarted")
                return false   -- the regular flow travels to the menu (lobby)
            end
            if not D.ACTIVE[s.phase] and s.phase ~= "match_over" then
                self:notice("match_ended_away")
            end
            return false
        end
        if now - (c.since or now) > D.T.resume_window_s then
            self:ev("resume", { ok = false, away_s = now - (c.since or now) })
            self:lose(s.sidecar_hung and "sidecar_stopped" or "link_lost")
            return true
        end
        -- keep the countdown in the overlay current (1 Hz)
        if now - (c.written_at or 0) >= 1.0 then self:write_conn() end
        return true
    end

    -- In a match and the link went down: freeze in place, no travel.
    if self.in_match and not up then
        self:set_conn("reconnecting", { since = now, reason = s.sidecar_hung and "sidecar_stopped"
            or (s.stalled and "stalled" or s.status), title = "RECONNECTING", text = "" })
        return true
    end
    if self.restarted and fresh and not self.in_match then self.restarted = nil end
    return false
end

-- --- GI profile ------------------------------------------------------------

function Dir:gi_backup_once(values)
    local path = self.dir .. "/.gi_backup.txt"
    if self.env.read(path) ~= nil then return end
    local lines = {}
    for _, kv in ipairs(values) do
        if kv.restore ~= false then
            local cur = self.env.gi_get(kv.name)
            if type(cur) == "boolean" or type(cur) == "number" then
                lines[#lines + 1] = kv.name .. "\t" .. tostring(cur)
            end
        end
    end
    self.env.write_atomic(path, table.concat(lines, "\n") .. "\n")
    self.env.log("GI single-player values backed up (%d)", #lines)
end

function Dir:gi_restore(reason)
    local path = self.dir .. "/.gi_backup.txt"
    local body = self.env.read_all and self.env.read_all(path) or self.env.read(path)
    if body == nil then return 0 end
    local n = 0
    for line in (body .. "\n"):gmatch("([^\n]*)\n") do
        local name, val = line:match("^(.-)\t(.+)$")
        if name then
            local v
            if val == "true" then v = true elseif val == "false" then v = false else v = tonumber(val) end
            if v ~= nil and self.env.gi_set(name, v) then n = n + 1 end
        end
    end
    self.env.remove(path)
    self.env.log("GI single-player values restored (%d) - %s", n, reason)
    return n
end

function Dir:foe_count(s)
    return math.max(D.T.min_native_foes, s.remote_fighters or 0)
end

-- Read-back only. Returns ok, {bad names}.
function Dir:gi_verify(s)
    local foes = self:foe_count(s)
    local bad = {}
    for _, kv in ipairs(D.MP_GI_PROFILE) do
        local want = (kv.value == "foes") and foes or kv.value
        if self.env.gi_get(kv.name) ~= want then bad[#bad + 1] = kv.name end
    end
    return #bad == 0, bad
end

-- Write the profile, read every value back. Returns ok, {bad names}.
function Dir:apply_gi(s, backup)
    local foes = self:foe_count(s)
    local values = {}
    for _, kv in ipairs(D.MP_GI_PROFILE) do
        values[#values + 1] = { name = kv.name, value = (kv.value == "foes") and foes or kv.value, restore = kv.restore }
    end
    if backup then self:gi_backup_once(values) end
    local bad = {}
    for _, kv in ipairs(values) do
        self.env.gi_set(kv.name, kv.value)
        if self.env.gi_get(kv.name) ~= kv.value then bad[#bad + 1] = kv.name end
    end
    if self.env.gi_heal_body then
        local n, what = self.env.gi_heal_body()
        if n and n > 0 then self.env.log("director: GI body condition healed (%d part(s): %s)", n, tostring(what)) end
    end
    return #bad == 0, bad, foes
end

-- --- travel ----------------------------------------------------------------

-- Round-reset crash guard (docs/development/crash-rr.md): no level change while the Runtime
-- Vertex Paint plugin (the game's blood / wound painting) has tasks queued or
-- running. Its task queue outlives the world, and a task that finishes after
-- the old arena was freed reads the freed UWorld on the game thread (the
-- intermittent round-reset crash, exe+0x4a23c64). env.travel_hold(why) closes
-- the queue, purges it and returns true until it is quiet (bounded, see
-- shared/hsmp_rvp.lua); open() then returns "held" and the caller retries on
-- a later tick. opts.now: a travel that must go at once (the soft-travel
-- re-issue: the native travel is already pending; its pre-hook purged).
-- Returns true (issued), false (OpenLevel failed) or "held".
function Dir:open(name, why, opts)
    local env = self.env
    if not (opts and opts.now) and env.travel_hold and env.travel_hold(why) then
        if self.held_for ~= name then
            self.held_for = name
            env.log("director: travel to %s held (%s): waiting for the Runtime Vertex Paint queue", name, tostring(why))
        end
        return "held"
    end
    self.held_for = nil
    local from = self.wshort or "?"
    self.self_travel = true
    local ok = false
    pcall(function() ok = self.env.open_level(name) end)
    self.self_travel = false
    if env.travel_issued then pcall(env.travel_issued, ok) end
    self.last_action = self.env.now()
    if ok then
        self.counters.travel = self.counters.travel + 1
        self.pending_travel = name
        self.env.log("director: travel %s -> %s (%s)", from, name, why)
        self:ev("travel", { from = from, to = name, by = "director", reason = why })
    else
        self.env.log("director: OpenLevel(%s) FAILED (%s)", name, why)
    end
    return ok
end

function Dir:begin_prepare(arena, why)
    self.target = arena
    self.prep = { tries = 0, opens = 0, why = why, next_at = 0 }
    self.in_match = true
    self:set_state("Prepare", why)
    self:step_prepare(self.sess)
end

function Dir:step_prepare(s)
    local now = self.env.now()
    local p = self.prep
    if now < p.next_at then return end
    self:ensure_save_guard()
    p.tries = p.tries + 1
    local ok, bad, foes = self:apply_gi(s, true)
    if not ok then
        if p.tries < D.T.gi_tries then return end   -- retry next tick
        self:error("gi_verify", "GI writes did not stick: " .. table.concat(bad, ", "))
        p.tries, p.next_at = 0, now + D.T.gi_retry_s
        return
    end
    self.env.log("director: GI profile verified for %s (foes=%d from %d remote fighter(s))",
        tostring(self.target), foes, s.remote_fighters or 0)
    self:seed_save_slot()
    self:set_state("Travel", p.why)
    self:step_travel()
end

-- Career save guard (shared/hsmp_saveguard.lua). The guard
-- diverts every SaveGameToSlot to HSMP_<inst>_<slot> while a session exists.
-- The Director knows a session exists when it prepares a match or serves an
-- arena, so it forces the guard ON from then on, and hands it back to
-- automatic only once the MENU world has arrived: the guard's own
-- liveness turns off (and runs GI "Load Game" on the career slot) in the same
-- tick that sees kicked / server_closed, while the MP arena is still loaded
-- and its native flow can still save; the travel to the menu comes later.
function Dir:ensure_save_guard()
    local env = self.env
    if not (env.sg_active and env.sg_force) or self.sg_forced then return end
    local active = env.sg_active()
    if active == nil then return end
    env.sg_force(true)
    self.sg_forced = true
    if active then
        env.log("director: save guard held ON until the menu world arrives")
    else
        env.log("director: save guard was inactive while preparing a match -> forced ON")
    end
end

-- The menu world is loaded (no MP arena left that could save): back to automatic.
function Dir:release_save_guard(why)
    if not (self.sg_forced and self.env.sg_force) then return end
    self.env.sg_force(nil)
    self.sg_forced = false
    self.env.log("director: save guard back to automatic (%s)", why)
end

-- The arena's game manager runs GI "Load Game" on every arena load, which
-- would undo the GI profile written before travel (and pull career wounds /
-- "Player Just Died" back in). Seeding writes the verified profile into the
-- session slot (the guard diverts the write), so that load reads it back.
-- Only from the Director's own tick: never inside an OpenLevel hook.
function Dir:seed_save_slot()
    local env = self.env
    if not env.sg_seed then return end
    local ok, d = env.sg_seed()
    if ok == nil then return end
    if ok then
        env.log("director: session save slot seeded (%s)", tostring(d))
    else
        self:error("save_seed", tostring(d))
    end
end

function Dir:step_travel()
    local p = self.prep
    self.travel_from_key = self.wkey
    local r = self:open(self.target, p.why)
    if r == "held" then return end   -- RVP hold: retried next tick (state stays Travel)
    p.opens = p.opens + 1
    if r then
        self.wait_since = self.env.now()
        self:set_state("WaitWorld")
    elseif p.opens >= D.T.open_tries then
        self:error("open_level_failed", self.target)
        p.opens, p.tries, p.next_at = 0, 0, self.env.now() + D.T.gi_retry_s
        self:set_state("Prepare", "retry after OpenLevel failure")
    end
end

function Dir:travel_menu(why, opts)
    opts = opts or {}
    local s = self.sess
    self:set_state("TravelMenu", why)
    self.in_match = false
    self.target = D.MENU_WORLD
    self.ready_round, self.loaded_for, self.loaded_key, self.reloaded_for = 0, 0, nil, nil
    self.ready_context, self.applied_spawn_id, self.place_context = nil, 0, nil
    self.live_release, self.live_wait_reason = nil, nil
    self.pipe = nil
    if s and s.exists and not opts.no_flag then
        self:return_to_lobby()
    end
    self:gi_restore(why)
    -- The save guard stays forced ON until the menu world has arrived
    -- (on_world); the guard reloads the career GI when it turns off there.
    if self.wshort == D.MENU_WORLD and not self.pending_travel then
        self:set_state("Menu", "already in the menu")
        self:release_save_guard("already in the menu")
        return
    end
    self.menu_retry_at = self.env.now() + 2
    -- A held menu travel is retried every tick until issued (the
    -- TravelMenu watchdog below skips menu worlds such as the splash screen,
    -- so it cannot be what retries it).
    self.menu_held = (self:open(D.MENU_WORLD, why) == "held") and why or nil
end

-- --- native travel interception (called from the OpenLevel pre-hook) ----------

-- target: the short level name the native flow wants. Returns the rewrite
-- destination, or nil to let it through.
function Dir:on_native_open_level(target)
    if self.self_travel then return nil end
    local s = self.get_session()
    self.sess = s
    if not s.exists then return nil end
    local want = self:native_want(s)
    if self.state == "TravelMenu" or self.conn.latched then want = D.MENU_WORLD end
    if not want or target == want then
        self:ev("travel", { from = self.wshort or "?", to = tostring(target), by = "native", reason = "native OpenLevel" })
        return nil
    end
    self.counters.native_rewritten = self.counters.native_rewritten + 1
    self.env.log("director: native OpenLevel('%s') rewritten -> '%s' (phase=%s round=%d) [#%d]",
        tostring(target), want, tostring(s.phase), s.round or 0, self.counters.native_rewritten)
    self:ev("native_travel_rewritten", { from = target, to = want, phase = s.phase, n = self.counters.native_rewritten })
    -- The in-place rewrite is a travel (the native OpenLevel goes on to load `want`)
    self:ev("travel", { from = self.wshort or "?", to = want, by = "native_rewrite", reason = "native OpenLevel(" .. tostring(target) .. ") rewritten" })
    if want == D.MENU_WORLD then
        self:return_to_lobby()
    else
        self:apply_gi(s, false)
    end
    self.last_action = self.env.now()
    return want
end

-- OpenLevelBySoftObjectPtr cannot be rewritten in place; the post-hook
-- re-issues our travel (the last request wins: one load, never Tavern -> arena).
function Dir:on_soft_travel_post(target)
    local s = self.get_session()
    self.sess = s
    if not s.exists then return nil end
    local want = self:native_want(s)
    if self.state == "TravelMenu" or self.conn.latched then want = D.MENU_WORLD end
    if not want then return nil end
    self.counters.native_rewritten = self.counters.native_rewritten + 1
    self:ev("native_travel_rewritten", { from = tostring(target), to = want, phase = s.phase,
        soft = true, n = self.counters.native_rewritten })
    if want == D.MENU_WORLD then
        self:return_to_lobby()
    else
        self:apply_gi(s, false)
    end
    self:open(want, "reroute native soft-object travel " .. tostring(target), { now = true })
    return want
end

-- --- requests from HSMPMenu (docs/development/subsystems/director-contract.md) ---

function Dir:ack(seq, status, arena, reason, ack_file)
    -- ack_file false: log only (UI requests are not acked through a file).
    if ack_file ~= false then
        self:bus_put("travel_ack", { seq = seq, status = status, arena = arena or "", reason = reason or "" })
    end
    self.env.log("director: %s request #%d %s%s%s", ack_file == nil and "travel" or "ui", seq, status,
        arena and (" arena=" .. arena) or "", (reason and reason ~= "") and (" (" .. reason .. ")") or "")
end

-- Evaluate one request against the server state. Returns status, arena, reason.
--   arena | menu                    (director-contract.md)
--   reconnect | dismiss | leave | rematch | ok   (connection / end-of-match UI)
function Dir:judge(req, s)
    local c = self.conn
    if req.want == "arena" then
        if not s.exists then return "refused", nil, "no session" end
        if c.latched then return "refused", nil, "connection lost (" .. tostring(c.reason) .. "): press RECONNECT" end
        if not self:link_fresh(s) then return "refused", nil, "not connected to the server" end
        if not D.ACTIVE[s.phase] then
            return "refused", nil, string.format("no match running (server phase %s)", tostring(s.phase))
        end
        if not valid_arena(s.arena) then return "refused", nil, "the server has no arena yet" end
        local why = (req.arena and req.arena ~= s.arena)
            and string.format("server arena is %s (requested %s)", s.arena, tostring(req.arena)) or ""
        return "accepted", s.arena, why
    elseif req.want == "menu" then
        if s.exists and D.ACTIVE[s.phase] and self:link_fresh(s) and not c.latched then
            return "refused", nil, string.format("match in progress (phase %s)", s.phase)
        end
        return "accepted", D.MENU_WORLD, ""
    elseif req.want == "reconnect" then
        if not c.latched then return "accepted", nil, "not disconnected" end
        if not s.exists or s.terminal then
            return "refused", nil, string.format("the session ended (%s): join the server again",
                tostring(s.terminal or "no session"))
        end
        return "accepted", nil, "rejoining when the link is back"
    elseif req.want == "dismiss" or req.want == "ok" then
        return "accepted", D.MENU_WORLD, ""
    elseif req.want == "leave" then
        if not s.exists then return "accepted", D.MENU_WORLD, "no session" end
        return "accepted", D.MENU_WORLD, "leaving the server"
    elseif req.want == "rematch" then
        if not s.exists or not self:link_fresh(s) then return "refused", nil, "not connected to the server" end
        if s.phase ~= "match_over" and s.phase ~= "lobby" then
            return "refused", nil, string.format("no finished match (phase %s)", tostring(s.phase))
        end
        return "accepted", s.arena, s.is_admin and "starting again when everyone is ready" or "ready for a rematch"
    end
    return "refused", nil, "unknown want " .. tostring(req.want)
end

-- Ask the sidecar to leave the server and exit (a G2S `leave` record).
function Dir:request_leave(why)
    self:send("leave", { reason = code(self.env, "leave_reason", "USER", 0) })
    self.env.log("director: asked the sidecar to leave the server (%s)", why)
    -- Until that session process is gone (its link record absent for >= 1 s,
    -- or "ended"), a new one starts (status connecting) or leave_s passed,
    -- the Director does not follow its last state back into the arena.
    self.leaving = { at = self.env.now() }
end
D.T.leave_s = 15

function Dir:step_leaving(s)
    local l = self.leaving
    if not l then return end
    local st = s.status
    if not s.exists or (st ~= "connected" and st ~= "reconnecting") or self.env.now() - l.at > D.T.leave_s then
        self.leaving = nil
    end
end

-- A typed command (G2S `command` record; the sidecar resends it until its
-- cmd_result). op: a cmd_op name ("READY", "START"); flag: READY's value /
-- START's force.
function Dir:send_cmd(op, flag)
    self.cmd_n = self.cmd_n + 1
    -- unique per player across sidecar restarts (the server caches results by (key, cmd_id))
    local id = (self.env.wall() % 40000000) * 100 + (self.cmd_n % 100)
    if id == 0 then id = 1 end
    return self:send("command", { cmd_id = id, op = code(self.env, "cmd_op", op, 0), flag = flag and true or false })
end

-- REMATCH: hold in the arena through the lobby until the host's START (or
-- give up after rematch_hold_s and go to the lobby screen).
function Dir:rematch_holding()
    local r = self.rematch
    return r ~= nil and r.lobby_at ~= nil and self.env.now() - r.lobby_at < D.T.rematch_hold_s
end

function Dir:step_rematch(s)
    local r = self.rematch
    if not r then return end
    local now = self.env.now()
    if D.ACTIVE[s.phase] then self.rematch = nil; return end   -- the new match started
    if s.phase == "lobby" then
        r.lobby_at = r.lobby_at or now
        -- The next match numbers its rounds from 1 again: this world serves
        -- none of them (its round 1 needs a fresh load).
        self.loaded_for, self.loaded_key, self.reloaded_for = 0, nil, nil
        if now - r.lobby_at >= D.T.rematch_hold_s then
            self.env.log("director: no rematch START within %d s; going to the lobby", D.T.rematch_hold_s)
            self.rematch = nil
            return
        end
        if now >= (r.next_cmd or 0) then
            r.next_cmd = now + D.T.rematch_cmd_s
            self:send_cmd("READY", true)
            if s.is_admin then self:send_cmd("START", false) end
        end
    end
end

-- Two request channels (bus keys) with their own seq: HSMPMenu's
-- travel_request (acked in travel_ack) and HSMPHud's ui_request (the modal /
-- MP pause / result-screen buttons; logged, not acked).
function Dir:serve_requests(s)
    self:serve_request_file(s, "travel_request", nil, "req_seq")
    self:serve_request_file(s, "ui_request", false, "ui_seq")   -- UI requests are not acked
end

-- key: the bus key of the request ("travel_request" | "ui_request"; a cleared
-- key has seq 0).
function Dir:serve_request_file(s, key, ack_file, seq_key)
    local r = self:bus(key)
    if not r then return end
    local seq = r.seq or 0
    if seq == 0 or seq <= (self[seq_key] or 0) then return end
    self[seq_key] = seq
    self.counters.requests = self.counters.requests + 1
    local req = { want = (r.want ~= "") and r.want or nil, arena = (r.arena ~= nil and r.arena ~= "") and r.arena or nil,
                  reason = r.reason or "", seq = seq }
    local status, arena, reason = self:judge(req, s)
    self:ack(seq, status, arena, reason, ack_file)
    if status ~= "accepted" then
        self.counters.refused = self.counters.refused + 1
        return
    end
    local why = "menu request #" .. seq .. " " .. req.reason
    if req.want == "arena" then
        -- Usually the session already moved us; act now if not.
        if (self.state == "Menu" or self.state == "TravelMenu") or self.target ~= arena then
            self:begin_prepare(arena, why)
        end
    elseif req.want == "reconnect" then
        if self.conn.latched then
            self.terminal_seen = nil
            if self:link_up(s) then
                self.env.log("director: RECONNECT pressed; the link is up - following the server")
                if self.sess and self.sess.clear_match_stale then pcall(self.sess.clear_match_stale) end
                self:set_conn("ok", { latched = false })
            else
                -- The server is still unreachable. Keep a visible
                -- "rejoining" state (overlay + countdown, BACK TO MENU) until
                -- the link is fresh, or lose it again after resume_window_s,
                -- rather than closing the modal with nothing on screen.
                self.env.log("director: RECONNECT pressed; rejoining when the link is back (%d s)", D.T.resume_window_s)
                self:set_conn("reconnecting", { latched = false, since = self.env.now(), reason = "rejoining",
                    title = "REJOINING", text = "", actions = { "menu" } })
            end
        end
    elseif req.want == "dismiss" or req.want == "ok" then
        local was = self.conn.reason
        local end_session = self.conn.latched and was ~= "server_restarted" and was ~= "match_ended_away"
        self:set_conn("ok", { latched = false })
        if end_session and s.exists then self:request_leave("player left after: " .. tostring(was)) end
        if self.wshort ~= D.MENU_WORLD and self.state ~= "TravelMenu" and (end_session or not s.exists) then
            self:travel_menu(why, { no_flag = true })
        end
    elseif req.want == "leave" then
        self.rematch = nil
        if s.exists then self:request_leave("left the match") end
        self:set_conn("ok", { latched = false })
        if self.wshort ~= D.MENU_WORLD then self:travel_menu(why, { no_flag = true }) end
    elseif req.want == "rematch" then
        self.rematch = { at = self.env.now() }
        self.env.log("director: REMATCH requested (%s)", s.is_admin and "host: will START" or "ready")
    else
        if self.wshort ~= D.MENU_WORLD then
            self:travel_menu(why, { no_flag = not s.connected })
        end
    end
end

-- --- spawn pipeline ----------------------------------------------------------

D.PIPE = { "world", "gi", "pawn", "vitals", "place", "kit", "census", "ready" }
function D.step_index(name)
    for i, n in ipairs(D.PIPE) do if n == name then return i end end
end
D.T.gi_post_s = 3   -- Spawn: GI profile re-applied after the arena's own "Load Game"

function Dir:start_pipeline(w, s, why, serve_round, status_since)
    self:ensure_save_guard()   -- also when the match found us already in the arena
    -- A deathmatch respawn load serves the round being fought; any other load the next one.
    local respawn = s.phase == "live" and s.respawn ~= nil and s.respawn.spawn_id == self.respawn_for
        and s.respawn.life == self.respawn_life
    self.loaded_for = serve_round or (respawn and s.round or (s.pending_round or (s.round + 1)))
    self.loaded_key = self:round_key(self.loaded_for)
    self.ready_round = 0
    self.ready_context, self.place_context = nil, nil
    self.live_release, self.live_wait_reason = nil, nil
    self.load_error, self.errors_logged, self.protect_logged = nil, nil, nil
    self.load_context = nil
    self.pipe = { key = w.key, arena = w.short, round = self.loaded_for, match_id = s.match_id,
                  life = respawn and s.respawn.life or (self.loaded_for == s.round and s.life or 1),
                  match_gen = self.match_gen, step = 1, t0 = self.env.now(),
                  status_since = status_since,
                  step_t = self.env.now(), notes = {}, tries = 0 }
    self:set_state("Spawn", why)
    self.env.log("director: world ready %s (phase=%s round=%d) -> serving round %d",
        w.short, tostring(s.phase), s.round or 0, self.loaded_for)
    self:ev("world_ready", { arena = w.short, world_key = w.key, round = self.loaded_for, server_arena = s.arena })
end

function Dir:next_step(note)
    local p = self.pipe
    if note then p.notes[#p.notes + 1] = D.PIPE[p.step] .. "=" .. note end
    p.step = p.step + 1
    p.step_t = self.env.now()
    p.tries = 0
end

function Dir:vitals_check(p, write)
    local env = self.env
    local cdo = env.cdo_vitals()
    if not cdo then return nil, "cdo unreadable" end
    if write then
        for _, fn in ipairs(D.VITALS_RESET_FUNCS) do pcall(env.pawn_call, p, fn) end
        for _, k in ipairs(D.VITALS_FIELDS) do
            if cdo[k] ~= nil then env.pawn_set(p, k, cdo[k]) end
        end
    end
    local bad = {}
    for _, k in ipairs(D.VITALS_FIELDS) do
        local want = cdo[k]
        if want ~= nil then
            local got = tonumber(env.pawn_get(p, k))
            if got == nil or math.abs(got - want) > 0.01 then bad[#bad + 1] = k end
        end
    end
    return #bad == 0, table.concat(bad, ",")
end

-- One pipeline step per call (the next call continues). Returns true when Ready.
function Dir:step_pipeline(w, s)
    local env, p = self.env, self.pipe
    local now = env.now()
    local age = now - p.step_t
    local step = D.PIPE[p.step]

    if step == "world" then
        if w.key ~= p.key or w.short ~= s.arena then
            self:error("wrong_world", tostring(w.short) .. " vs " .. tostring(s.arena))
            return false
        end
        self:next_step(); return false

    elseif step == "gi" then
        -- The arena's game manager reloaded the GI from the (session) save on
        -- BeginPlay: write the profile again AFTER that load and read it back.
        p.tries = p.tries + 1
        local ok, bad = self:apply_gi(s, false)
        if ok then self:next_step(p.tries > 1 and ("ok(try " .. p.tries .. ")") or "ok"); return false end
        if age > D.T.gi_post_s then
            self:error("gi_verify", "after load: " .. table.concat(bad, ", "))
            self:next_step("FAILED(" .. table.concat(bad, ",") .. ")")
        end
        return false

    elseif step == "pawn" then
        local pawn = env.pawn()
        if pawn then
            p.pawn_id = env.pawn_id(pawn)
            env.game_input()
            self:next_step(); return false
        end
        if now - p.t0 > D.T.pawn_s then self:error("no_pawn", "no possessed pawn after " .. D.T.pawn_s .. " s") end
        return false

    elseif step == "vitals" then
        local pawn = env.pawn()
        if not pawn or env.pawn_id(pawn) ~= p.pawn_id then return self:restart_pawn() end
        p.tries = p.tries + 1
        local ok, why = self:vitals_check(pawn, true)
        if ok then self:next_step("ok"); return false end
        if age > D.T.vitals_s then
            self:error("vitals", why)
            self:next_step("FAILED(" .. tostring(why) .. ")")
        end
        return false

    elseif step == "place" then
        local pawn = env.pawn()
        if not pawn or env.pawn_id(pawn) ~= p.pawn_id then return self:restart_pawn() end
        local order = s.spawn_order and s.spawn_order(p.round, p.arena)
        if order then p.order = order end
        -- The only placement evidence is HSMPSync's verified status for this
        -- round, arena and pawn, cross-checked against where the pawn is now
        -- (a body that snapped back after the status was written does not count).
        local st = self:spawn_status(p)
        if st and st.verified then
            local x, y = env.pawn_loc(pawn)
            local d = (x and st.x) and math.sqrt((x - st.x) ^ 2 + (y - st.y) ^ 2) or nil
            if d and d <= D.T.place_tol_cm then
                p.placed = { st.x, st.y, st.z }
                p.verified_life = st.life
                p.place_tol = st.tol
                -- The verified spawn order authorizes this pawn's roots on the server before
                -- Ready (ping, without LOADED): peers' stand-ins can be made during the
                -- census instead of waiting on each other's Ready.
                self.place_context = (p.order and (p.order.spawn_id or 0) ~= 0) and { match_id = p.match_id or s.match_id,
                    match_gen = p.match_gen, life = st.life, round = p.round, pawn = p.pawn_id, world = p.key,
                    arena = p.arena, spawn_id = p.order.spawn_id } or nil
                self:next_step(string.format("hsmpsync(id=%s tries=%s %.0fcm%s)", tostring(st.spawn_id),
                    tostring(st.tries), d, p.retried and " after retry" or ""))
                return false
            end
            p.why_not = string.format("verified, but the pawn is %s cm from it now", d and string.format("%.0f", d) or "?")
        elseif st then
            p.why_not = st.error and ("HSMPSync: " .. st.error) or "placed, not verified yet"
        else
            p.why_not = order and "no .spawn_status.json for this round/pawn" or "no spawn order"
        end
        if order then
            -- One retry: HSMPSync re-places when asked (bus key spawn_request),
            -- early if it already reported a failure.
            local failed = st and st.error ~= nil
            if not p.retried and (age > D.T.place_retry_s or (failed and age > 1)) then
                p.retried = true
                self:request_place(p, order, p.why_not)
            end
            if age > D.T.place_s then
                self:error("spawn_timeout", string.format("not placed on order %s after %d s (%s)",
                    tostring(order.spawn_id), D.T.place_s, tostring(p.why_not)))
                self:next_step("TIMEOUT")
            end
        elseif age > D.T.place_wait_s then
            self:next_step("no_order")
        end
        return false

    elseif step == "kit" then
        local pawn = env.pawn()
        if not pawn or env.pawn_id(pawn) ~= p.pawn_id then return self:restart_pawn() end
        local st = env.kit_status and env.kit_status() or nil
        if st == nil then
            -- One missing read (the key not written yet for this pawn, or
            -- cleared) is not "no kit status"; absence counts only after >= 2
            -- reads spanning >= absent_s.
            p.kit_absent_n = (p.kit_absent_n or 0) + 1
            p.kit_absent_since = p.kit_absent_since or now
            if age > D.T.kit_absent_s and p.kit_absent_n >= D.T.absent_reads
                and now - p.kit_absent_since >= D.T.absent_s then
                self:next_step("unavailable(no kit_status)")
            end
            return false
        end
        p.kit_absent_n, p.kit_absent_since = 0, nil
        local t = tonumber(st.t)
        -- Pooled Willie FNames come back in the next world (an arena
        -- reload re-uses Willie_BP_C_11), so the name alone is no proof: the
        -- status must also be written for THIS world load ("t" = the writer's
        -- os.clock, the same process clock as env.now; a line without "t" is
        -- an older writer and counts by name).
        local fresh = (st.pawn == p.pawn_id) and (t == nil or t >= p.t0 - D.T.kit_world_slack_s)
        -- Held: the dress survived the native re-arm that strikes ~0.6-1 s
        -- after a setup (HSMPLoadout rewrites ok=false when it does), or
        -- HSMPLoadout's own stability window already confirmed it.
        local held = st.stable == true or t == nil or now - t >= D.T.kit_hold_s
        if fresh and st.ok == true and held then
            p.kit = st
            self:next_step(string.format("ok(armour=%s r=%s l=%s)", tostring(st.armour_n),
                tostring(st.r_class), tostring(st.l_class)))
            return false
        end
        if age > D.T.kit_s then
            self:error("kit_error", fresh and ((st.error or "") ~= "" and st.error or "not verified") or "no status for this pawn")
            self:next_step("FAILED")
        end
        return false

    elseif step == "census" then
        local pawn = env.pawn()
        if not pawn or env.pawn_id(pawn) ~= p.pawn_id then return self:restart_pawn() end
        local vis, detail = env.census(pawn)
        local combat_ready, combat_why = true, nil
        if env.combat_ready then combat_ready, combat_why = env.combat_ready(p, s, pawn) end
        if not combat_ready and p.combat_wait_reason ~= combat_why then
            p.combat_wait_reason = combat_why
            env.log("director: Ready waits for combat spawn proof: %s", tostring(combat_why))
        end
        if vis == nil then
            -- The world is still settling (no Willie walk yet): wait;
            -- give up only well past the census window
            if not env.combat_ready and age > D.T.census_s + 5 then self:next_step("unavailable(" .. tostring(detail) .. ")") end
            return false
        end
        local expected = s.remote_fighters or 0
        local settled = vis == expected
        -- A stand-in that is still MISSING is usually still being made: on an
        -- arena without a free native foe HSMPAvatars spawns its body ~6 s into
        -- the world and HSMPLoadout keeps it hidden until its kit is verified
        -- (~1.5-2.5 s). On a slow instance (LordsHall at ~20 fps) that can run
        -- ~1 s past the 8 s census window, and the round would start with the
        -- peer invisible for its first seconds. Wait for it up to
        -- census_missing_s (far inside the server's 45 s load barrier);
        -- extras are judged at census_s.
        local limit = (vis or 0) < expected and D.T.census_missing_s or D.T.census_s
        if (settled and combat_ready) or (not env.combat_ready and age > limit) then
            local extras = math.max(0, (vis or 0) - expected)
            self:ev("willie_census", { visible = (vis or 0) + 1, expected = expected + 1, extras = extras,
                missing = math.max(0, expected - (vis or 0)), at = "ready", round = p.round, detail = detail })
            if not settled then
                env.log("director: stand-in census DEFECT visible=%d expected=%d (%s)", vis or -1, expected, tostring(detail))
            end
            self:next_step(settled and "ok" or string.format("DEFECT(%d/%d)", vis or -1, expected))
        end
        return false

    elseif step == "ready" then
        local pawn = env.pawn()
        if not pawn or env.pawn_id(pawn) ~= p.pawn_id then return self:restart_pawn() end
        if env.combat_ready then
            local ready, why = env.combat_ready(p, s, pawn)
            if not ready then
                if p.combat_wait_reason ~= why then
                    p.combat_wait_reason = why
                    env.log("director: Ready waits for combat spawn proof: %s", tostring(why))
                end
                return false
            end
        end
        -- HSMPSync's watchdog may be re-placing the pawn right now (pushed off
        -- its spawn after the place step): Ready waits for that placement to be
        -- verified and then reports against its destination. Reporting in the
        -- middle of it can put a pawn ~150 cm off at Ready. Bounded by place_s.
        if p.placed then
            local st = self:spawn_status(p)
            local x, y = env.pawn_loc(pawn)
            local dist = st and st.x and x and math.sqrt((x - st.x) ^ 2 + (y - st.y) ^ 2)
            if not st or not st.verified or st.error or not dist or dist > D.T.place_tol_cm then
                if age > D.T.place_s then
                    self:error("spawn_timeout", "final Ready placement is not verified for this pawn")
                else
                if not p.ready_wait_logged then
                    p.ready_wait_logged = true
                    env.log("director: Ready waits for HSMPSync's re-placement of the pawn")
                end
                return false
                end
            end
            if st and st.verified and st.x and (st.x ~= p.placed[1] or st.y ~= p.placed[2]) then
                p.placed = { st.x, st.y, st.z }
            end
        end
        -- Final vitals check (wounds may land late); re-apply once if needed.
        local vok = self:vitals_check(pawn, false)
        if vok == false then vok = self:vitals_check(pawn, true) end
        if vok == false then self:error("vitals", "final Ready vitals check failed") end
        -- Final GI read-back (a late game-manager "Load Game" would undo it).
        local gok, gbad = self:gi_verify(s)
        if not gok then
            env.log("director: GI profile drifted before Ready (%s); re-applied", table.concat(gbad, ", "))
            p.notes[#p.notes + 1] = "gi_reapplied"
            gok = self:apply_gi(s, false)
        end
        if not gok then self:error("gi_verify", "final Ready GI check failed") end
        local x, y, z = env.pawn_loc(pawn)
        local o = p.order
        local dist
        if o and x and o.x then dist = math.sqrt((x - o.x) ^ 2 + (y - o.y) ^ 2) end
        local ok = self.load_error == nil and vok ~= false and gok
        -- Only this verified pawn/world may acknowledge the placement. Keep
        -- its original match identity; a newer session cannot relabel it.
        self.ready_round = ok and p.round or 0
        self.applied_spawn_id = ok and (p.placed and p.order and p.order.spawn_id) or 0
        self.ready_context = ok and { match_id = p.match_id or s.match_id, match_gen = p.match_gen,
            life = p.verified_life, round = p.round, status_since = p.status_since or p.t0,
            pawn = p.pawn_id, world = p.key, arena = p.arena } or nil
        -- Fields hsmp-gate check-events reads: dist_cm to the order, x/y/z,
        -- and snap_z = the slot's ground-snapped Z HSMPSync placed the pawn on
        -- (spawn_status z, kept in p.placed[3]).
        local snap_z = p.placed and p.placed[3] or nil
        -- dest_cm: pawn -> HSMPSync's destination (the order, or the clear
        -- spiral point next to it when the order was blocked); offset_cm:
        -- destination -> order (0 when clear; <= the outer spiral ring).
        -- tol_cm: the placer's acceptance radius (hsmp-gate requires it to
        -- equal its own placement limit, so the two cannot disagree).
        local dest_cm, offset_cm
        if p.placed and p.placed[1] and x then
            dest_cm = math.sqrt((x - p.placed[1]) ^ 2 + (y - p.placed[2]) ^ 2)
            if o and o.x then offset_cm = math.sqrt((p.placed[1] - o.x) ^ 2 + (p.placed[2] - o.y) ^ 2) end
        end
        self:ev("spawn_verified", { round = p.round, ok = ok, arena = p.arena, world_key = p.key,
            snap_z = snap_z, dest_cm = dest_cm, offset_cm = offset_cm, tol_cm = p.place_tol,
            vitals_ok = vok, gi_ok = gok, x = x, y = y, z = z, spawn_id = o and o.spawn_id, dist_cm = dist,
            steps = table.concat(p.notes, " "), load_error = self.load_error,
            ms = math.floor((now - p.t0) * 1000) })
        self:ev("ready_report", { round = p.round, arena = p.arena, load_error = self.load_error })
        env.log("director: READY round %d on %s in %.1f s [%s]%s", p.round, p.arena, now - p.t0,
            table.concat(p.notes, " "), self.load_error and (" load_error=" .. self.load_error) or "")
        self.pipe = nil
        self:set_state((s.phase == "live") and "Live" or "Ready")
        return true
    end
    return false
end

-- HSMPSync's bus key spawn_status (docs/development/subsystems/director.md) if
-- it is for this pipeline's round, arena and pawn: { verified, x, y, z,
-- spawn_id, tries, error, protect_until }.
function Dir:spawn_status(p)
    local r = self:bus("spawn_status")
    if not r or (r.seq or 0) == 0 then return nil end   -- absent / cleared: no evidence this tick
    if r.round ~= p.round or r.arena ~= p.arena then return nil end
    if p.order and r.spawn_id ~= p.order.spawn_id then return nil end
    if p.match_id and p.match_id ~= 0 and r.match_id ~= p.match_id then return nil end
    if p.life and p.life ~= 0 and r.life ~= p.life then return nil end
    if r.t and r.t < (p.status_since or p.t0) then return nil end
    if r.pawn == "" or r.pawn ~= p.pawn_id then return nil end
    local pos = r.has_dest and r.pos or {}
    return { verified = r.verified == true, life = r.life, x = pos[1], y = pos[2], z = pos[3],
             spawn_id = (r.spawn_id ~= 0) and r.spawn_id or nil, tries = r.tries,
             error = (r.error ~= "") and r.error or nil, protect_until = r.has_protect_until and r.protect_until or nil,
             tol = r.tol_cm }
end

-- Ask HSMPSync to place the pawn again (one retry per world load).
function Dir:request_place(p, order, why)
    self.place_req_seq = (self.place_req_seq or 0) + 1
    self:bus_put("spawn_request", { seq = self.place_req_seq, round = p.round, arena = p.arena or "",
        pawn = p.pawn_id or "", spawn_id = order.spawn_id or 0, why = tostring(why or "") })
    self.env.log("director: placement on order %s not confirmed after %.0f s (%s); asking HSMPSync to re-place (retry 1)",
        tostring(order.spawn_id), self.env.now() - p.step_t, tostring(why))
end

-- Spawn protection (HSMPSync, docs/development/subsystems/spawns.md): spawn_status says
-- until when (process clock, the same os.clock in every mod) the pawn is
-- protected. A pawn that "dies" inside the window is not reported dead.
function Dir:protected_now(pawn_id)
    local st = self:bus("spawn_status")
    if not st or (st.seq or 0) == 0 then self.last_protected = false; return false end
    -- A status for another pawn (an earlier world / process) protects nothing.
    if pawn_id and st.pawn ~= "" and st.pawn ~= pawn_id then self.last_protected = false; return false end
    local r
    if not st.has_protect_until then
        -- HSMPSync leaves protect_until unset from placement until Live (unbounded protection).
        r = self.pipe ~= nil or self.state ~= "Live"
    else
        r = self.env.now() < st.protect_until
    end
    self.last_protected = r
    return r
end

-- The possessed pawn changed under us (native respawn/possession swap):
-- restart from the pawn step in the same world.
function Dir:restart_pawn()
    local p = self.pipe
    self.env.log("director: possessed pawn changed during the spawn pipeline; restarting at the pawn step")
    p.step, p.step_t, p.pawn_id = D.step_index("pawn"), self.env.now(), nil
    -- Drop the notes of the steps that will run again (otherwise the READY
    -- line lists "vitals=ok vitals=ok place=..." after a possession swap).
    local keep = {}
    for _, n in ipairs(p.notes or {}) do
        local i = D.step_index(n:match("^([^=]+)=") or "")
        if i and i < p.step then keep[#keep + 1] = n end
    end
    keep[#keep + 1] = "pawn_restart"
    p.notes = keep
    return false
end

-- --- per-tick ----------------------------------------------------------------

function Dir:on_world(w, s)
    local prev = self.wshort
    self.wkey, self.wshort = w.key, w.short
    -- The RVP queue closed for the travel opens again in the new world.
    if self.env.rvp_reopen then pcall(self.env.rvp_reopen, "new world " .. tostring(w.short)) end
    self.menu_held = nil
    self.pending_travel = nil
    self.pipe = nil
    self.ready_round = 0
    self.ready_context, self.place_context = nil, nil
    self.live_release, self.live_wait_reason = nil, nil
    self.frozen_id = nil
    self.applied_spawn_id = 0
    -- The menu world is up, the MP arena is gone: the save guard may
    -- follow its own liveness again (not while a new match is being prepared).
    if is_menu(w.short) and self.state ~= "Prepare" and self.state ~= "Travel" and self.state ~= "WaitWorld" then
        self:release_save_guard("menu world " .. tostring(w.short) .. " arrived")
    end
    -- Arriving where we travelled is local work: the last known server arena
    -- counts even while the link is briefly down (never while latched).
    local want = (not self.conn.latched) and self:want(s, true) or nil
    if self.state == "TravelMenu" then
        if is_menu(w.short) then self:set_state("Menu", "arrived in " .. w.short) end
        return
    end
    if want and want ~= D.MENU_WORLD then
        if w.short == want then
            self.target = want
            self.in_match = true
            self:start_pipeline(w, s, (self.state == "WaitWorld") and "arrived" or ("world changed from " .. tostring(prev)))
            return
        end
        if self.state == "WaitWorld" then
            -- Landed somewhere else (the pre-hook should have prevented it): retry.
            self.env.log("director: travel to %s landed in %s; retrying", want, tostring(w.short))
            if self.prep and self.prep.opens < D.T.open_tries then
                self:set_state("Travel", "retry"); self:step_travel()
            else
                self:error("wrong_world", "landed in " .. tostring(w.short))
                self:begin_prepare(want, "retry after wrong world")
            end
        end
        return
    end
    if is_menu(w.short) and self.state ~= "Prepare" and self.state ~= "Travel" and self.state ~= "WaitWorld" then
        self:set_state("Menu", "in " .. w.short)
    end
end

-- Match identity: round numbers restart at 1 in every match (and on a
-- server restart), so every round bookkeeping key is "<match_gen>:<round>".
-- match_gen moves when the server's match_id changes (S2CSession), when the
-- server epoch changes, or when the phase falls back from a match to the
-- lobby / no session (an older sidecar without match_id). It never moves on a
-- nil -> known transition, so a late match_id cannot fake a new match.
-- The phase and the match_id need not change together: the lobby between two
-- matches already moved the context (phase bump); the new match_id often
-- lands later, during match 2's countdown, after the pipeline keyed its world
-- on the new context. A match_id change while such a bump is still
-- unconsumed (t.fresh) is that same new match: adopted, no second bump (a
-- second bump would re-key the round and reload the arena in the countdown). t.fresh ends with the first match_id change after it, or once a
-- round of the new context ended (roundover / match_over: any late id has
-- arrived by then).
function Dir:track_match(s)
    local t = self.mt
    if s.match_id and s.match_id ~= 0 then
        if t.match_id and t.match_id ~= s.match_id then
            if t.fresh then
                t.fresh = false
                self.env.log("director: match_id %s -> %s belongs to the match context #%d already opened (%s); no reload",
                    tostring(t.match_id), tostring(s.match_id), self.match_gen, tostring(t.fresh_why))
            else
                self:bump_match("match_id " .. t.match_id .. " -> " .. s.match_id)
                t.fresh = false   -- this bump IS the match_id change
            end
        end
        t.match_id = s.match_id
    end
    if s.phase == "roundover" or s.phase == "match_over" then t.fresh = false end
    if s.epoch then
        if t.epoch and t.epoch ~= s.epoch then self:bump_match("epoch " .. t.epoch .. " -> " .. s.epoch) end
        t.epoch = s.epoch
    end
    local ph = s.phase
    local in_m = D.ACTIVE[ph] or ph == "match_over"
    if t.in_m and (ph == "lobby" or ph == "none" or not s.exists) then
        self:bump_match("phase " .. tostring(ph))
    end
    t.in_m = in_m and true or false
end

function Dir:bump_match(why)
    self.match_gen = self.match_gen + 1
    -- Respawn spawn IDs encode round/life, so a later match can reuse the
    -- exact same ID (round 1, life 2 = 386). Deduplicate within a match only.
    -- track_match already coalesces a late match_id into an opened context;
    -- clearing here must not happen on that late allocation transition.
    self.respawn_for = nil
    self.respawn_life = nil
    self.ready_round, self.applied_spawn_id, self.ready_context, self.place_context = 0, 0, nil, nil
    self.live_release, self.live_wait_reason = nil, nil
    self.mt.fresh, self.mt.fresh_why = true, why
    self.env.log("director: new match context #%d (%s)", self.match_gen, why)
end

function Dir:round_key(round) return string.format("%d:%d", self.match_gen, round or -1) end

function Dir:tick()
    local env = self.env
    local now = env.now()
    local s = self.get_session()
    self.sess = s
    self:step_leaving(s)
    self:track_match(s)
    if env.flush then pcall(env.flush) end   -- retry queued G2S sends
    if env.rvp_tick then pcall(env.rvp_tick) end   -- RVP: closed-queue safety reopen
    local w = env.world()

    self:serve_requests(s)
    if now >= self.next_hb then self.next_hb = now + D.T.hb_s; self:heartbeat() end

    if w.ok and w.key ~= self.wkey then self:on_world(w, s) end

    local blocked = self:step_conn(w, s)
    self.link_lost_at = (self.conn.state == "reconnecting") and self.conn.since or nil
    local pawn = (w.ok and self.wshort and self.wshort == self.target) and env.pawn() or nil
    if w.ok and not blocked then
        self:step_rematch(s)
        self:step_state(w, s)
    end
    if w.ok then self:update_freeze(s, pawn) end
    if w.ok and pawn then self:live_census(s, pawn) end
    self:ping(s, pawn)
end

-- willie_census{at="live"} every census_live_s while the round is Live (the
-- Director is the only emitter; the pipeline emits at="ready").
function Dir:live_census(s, pawn)
    local now = self.env.now()
    if self.state ~= "Live" or s.phase ~= "live" then self.next_census = nil; return end
    -- The first Live sample is taken on the first Live tick (a round can be
    -- shorter than one cadence, e.g. a test that kills right after Live),
    -- then every census_live_s.
    if self.next_census == nil then self.next_census = now end
    if now < self.next_census then return end
    self.next_census = now + D.T.census_live_s
    local vis, detail = self.env.census(pawn)
    if vis == nil then self.next_census = now + 0.5; return end   -- world still settling
    local expected = s.remote_fighters or 0
    self:ev("willie_census", { visible = (vis or 0) + 1, expected = expected + 1, at = "live", round = s.round,
        extras = math.max(0, (vis or 0) - expected), missing = math.max(0, expected - (vis or 0)), detail = detail })
end

function Dir:step_state(w, s)
    local env = self.env
    local now = env.now()
    local want = self:want(s)

    -- Session process gone mid-match (the menu's CANCEL / LEAVE deleted it):
    -- the player chose it, so no modal. A lost LINK is step_conn's job.
    if self.in_match and not s.exists then
        self.gone_at = self.gone_at or now
        if now - self.gone_at > D.T.session_gone_s then
            self.gone_at = nil
            return self:travel_menu("session ended", { no_flag = true })
        end
    else
        self.gone_at = nil
    end

    -- Leftover GI backup (crashed session): restore once we sit in the menu with no session.
    if not s.exists and is_menu(w.short) and self.state == "Menu" then
        self.menu_since = self.menu_since or now
        if now - self.menu_since > D.T.menu_restore_s and env.read(self.dir .. "/.gi_backup.txt") then
            self:gi_restore("no MP session (startup / leftover backup)")
        end
    else
        self.menu_since = nil
    end

    local st = self.state
    -- The server changed its mind while we were getting there (abort, MAP).
    if (st == "Prepare" or st == "Travel" or st == "WaitWorld") and want and want ~= self.target then
        if want == D.MENU_WORLD then return self:travel_menu("match aborted while travelling") end
        return self:begin_prepare(want, "server arena changed to " .. want)
    end
    if st == "Prepare" then return self:step_prepare(s) end
    if st == "Travel" then return self:step_travel() end
    if st == "WaitWorld" then
        if now - self.wait_since > D.T.world_s then
            self:error("world_timeout", "no new world after " .. D.T.world_s .. " s")
            return self:begin_prepare(self.target, "retry after world timeout")
        end
        return
    end
    if st == "TravelMenu" then
        if self.menu_held then
            if self:open(D.MENU_WORLD, self.menu_held) ~= "held" then
                self.menu_held = nil
                self.menu_retry_at = now + 2
            end
            return
        end
        if now > (self.menu_retry_at or 0) and not is_menu(w.short) then
            self.menu_retry_at = now + 5
            if self:open(D.MENU_WORLD, "retry") == "held" then self.menu_held = "retry" end
        end
        return
    end

    -- Menu / Spawn / Ready / Live: follow the server.
    if want and want ~= D.MENU_WORLD then
        if w.short ~= want then
            if st == "Menu" or now - self.last_action > D.T.debounce_s then
                local why = (st == "Menu") and string.format("server phase %s, arena %s", s.phase, want)
                    or string.format("watchdog: in %s but server arena is %s", tostring(w.short), want)
                return self:begin_prepare(want, why)
            end
            return
        end
        -- In the right arena.
        local evidence = self.pipe or self.ready_context
        if evidence and evidence.match_gen == self.match_gen and evidence.match_id and evidence.match_id ~= 0
            and s.match_id and s.match_id ~= 0 and evidence.match_id ~= s.match_id
            and (st == "Spawn" or st == "Ready" or st == "Live") then
            -- A late match allocation belongs to this already loaded world,
            -- but the old proof cannot be relabelled. Verify the new source
            -- placement and the pawn again without another world travel.
            return self:start_pipeline(w, s, "verify newly allocated match identity", evidence.round,
                evidence.status_since or evidence.t0)
        end
        if st == "Menu" then
            self.target, self.in_match = want, true
            return self:start_pipeline(w, s, "already in the arena")
        end
        local pending = s.pending_round or (s.round + 1)
        local pkey = self:round_key(pending)
        if s.phase == "countdown" and self.loaded_key ~= pkey and self.reloaded_for ~= pkey
            and (st == "Ready" or st == "Live" or st == "Spawn") then
            self.reloaded_for = pkey
            return self:begin_prepare(want, string.format("round %d starting", pending))
        end
        -- Deathmatch: the server ordered us back into the running round. The same reload
        -- as a new round (fresh world, pawn, vitals, placement on the respawn order, kit,
        -- census), serving THIS round; the server revives us on the placement report.
        local rsp = s.respawn
        if s.phase == "live" and rsp and (self.respawn_for ~= rsp.spawn_id or self.respawn_life ~= rsp.life)
            and (st == "Ready" or st == "Live" or st == "Spawn") then
            self.respawn_for = rsp.spawn_id
            self.respawn_life = rsp.life
            self:ev("respawn", { round = s.round, spawn_id = rsp.spawn_id, life = rsp.life })
            return self:begin_prepare(want, string.format("respawn order (round %d, life %d)", s.round, rsp.life))
        end
        if st == "Spawn" and self.pipe then
            for _ = 1, 3 do   -- run a few cheap steps per tick
                local before = self.pipe and self.pipe.step
                if self:step_pipeline(w, s) or not self.pipe or self.pipe.step == before then break end
            end
            return
        end
        if st == "Ready" and s.phase == "live" then self:set_state("Live") end
        if st == "Live" and s.phase ~= "live" then self:set_state("Ready", "phase " .. s.phase) end
        return
    end
    if want == D.MENU_WORLD and self.in_match and now - self.last_action > 2 then
        return self:travel_menu(s.phase == "lobby" and "match over" or "session phase " .. tostring(s.phase))
    end
    if want == D.MENU_WORLD and s.exists and not is_menu(w.short) and now - self.last_action > D.T.debounce_s then
        return self:travel_menu("watchdog: in " .. tostring(w.short) .. " while the server has no match")
    end
end

-- Input frozen while a session runs in our arena, except in Live while the
-- server has us alive. Idempotent per pawn.
function Dir:update_freeze(s, pawn)
    local env = self.env
    local in_arena = s.exists and self.wshort ~= nil and self.wshort == self.target and self.in_match
    -- Reconnecting: the world stays as it is, the player cannot act.
    local ctx = self.ready_context
    local verified = ctx and ctx.world == self.wkey and ctx.arena == self.wshort
        and ctx.match_id == s.match_id and ctx.match_gen == self.match_gen and ctx.round == s.round
        and (s.life == nil or ctx.life == s.life)
        and pawn and ctx.pawn == env.pawn_id(pawn) and self.ready_round == s.round
        and self.load_error == nil
    local released = in_arena and verified and self.state == "Live" and s.phase == "live"
        and s.alive[s.my_id] ~= false and self.conn.state == "ok"
    if not released then
        -- Pause/reconnect must re-establish a current proof on resume. New
        -- worlds/lives also clear the latch; ordinary wounded Live does not.
        self.live_release, self.live_wait_reason = nil, nil
    else
        local prior = self.live_release
        local same = prior and prior.match_id == ctx.match_id and prior.match_gen == ctx.match_gen
            and prior.round == ctx.round and prior.life == ctx.life and prior.world == ctx.world
            and prior.pawn == ctx.pawn and prior.spawn_id == self.applied_spawn_id
        if not same then
            self.live_release = nil
            local ready, why = true, nil
            if env.combat_ready then
                ready, why = env.combat_ready({ key = ctx.world, pawn_id = ctx.pawn, arena = ctx.arena,
                    match_id = ctx.match_id, match_gen = ctx.match_gen, round = ctx.round,
                    life = ctx.life, verified_life = ctx.life }, s, pawn)
            end
            if ready then
                self.live_release = { match_id = ctx.match_id, match_gen = ctx.match_gen, round = ctx.round,
                    life = ctx.life, world = ctx.world, pawn = ctx.pawn, spawn_id = self.applied_spawn_id }
                self.live_wait_reason = nil
            else
                released = false
                if self.live_wait_reason ~= why then
                    self.live_wait_reason = why
                    env.log("director: Live input waits for combat spawn proof: %s", tostring(why))
                end
            end
        end
    end
    local frozen = in_arena and not released
    if frozen and pawn then
        local id = env.pawn_id(pawn)
        if self.frozen_id ~= id then
            env.freeze(pawn, true)
            self.frozen_id = id
            env.log("director: input frozen (state=%s phase=%s round=%d)", self.state, tostring(s.phase), s.round or 0)
        end
    elseif not frozen and self.frozen_id then
        env.freeze(pawn, false)
        self.frozen_id = nil
        env.log("director: input released (state=%s phase=%s round=%d)", self.state, tostring(s.phase), s.round or 0)
    end
end

-- The game status report (G2S `game_status`): the round whose world is ready,
-- dead, the loaded arena, the load error and the applied spawn order.
function Dir:ping(s, pawn)
    local now = self.env.now()
    if not s.connected or now < self.next_ping then return end
    self.next_ping = now + D.T.ping_s
    local dead = false
    if pawn then
        local hp = tonumber(self.env.pawn_get(pawn, "Health"))
        dead = hp ~= nil and hp <= 0
        if dead and self:protected_now(self.env.pawn_id(pawn)) then
            dead = false
            if not self.protect_logged then
                self.protect_logged = true
                self.env.log("director: pawn Health <= 0 inside the spawn protection window; not reported dead")
            end
        end
    end
    local arena = (self.wshort and self.wshort == self.target) and self.wshort or ""
    -- G2S `game_status` (C2SGameStatus): the load-barrier report, game
    -- liveness and diagnostic dead state, ~1 Hz while connected. Only the
    -- reliable generation-scoped DeathReport can declare a server death.
    local env = self.env
    local flags = 0
    local ctx = self.ready_context
    local loaded = self.ready_round > 0 and ctx and ctx.match_gen == self.match_gen
        and ctx.life and ctx.life > 0
        and ctx.world == self.wkey and ctx.arena == self.wshort and pawn
        and ctx.pawn == env.pawn_id(pawn) and self.load_error == nil
    -- A first allocation can arrive after verification; it belongs to the
    -- same context only while that exact pawn and world remain current.
    if loaded and (not ctx.match_id or ctx.match_id == 0) then ctx.match_id = s.match_id end
    if loaded then flags = flags | code(env, "status_flag", "LOADED", 1) end
    -- Placed, not yet Ready: the same context checks on the verified placement, no LOADED
    -- (the load barrier still waits for Ready).
    local pc = not loaded and self.place_context
    local placed = pc and pc.match_gen == self.match_gen and pc.life and pc.life > 0
        and pc.world == self.wkey and pc.arena == self.wshort and pawn
        and pc.pawn == env.pawn_id(pawn) and self.load_error == nil
    if placed and (not pc.match_id or pc.match_id == 0) then pc.match_id = s.match_id end
    if dead then flags = flags | code(env, "status_flag", "DEAD", 4) end
    if self.in_match == false and (self.wshort == nil or is_menu(self.wshort)) then
        flags = flags | code(env, "status_flag", "IN_MENU", 8)
    end
    local failed = self.load_error and self.load_context
    local failure_current = failed and failed.match_gen == self.match_gen
    self:send("game_status", {
        match_id = (loaded and ctx.match_id) or (placed and pc.match_id) or (failed and failed.match_id) or s.match_id or 0,
        round = loaded and self.ready_round or (placed and pc.round) or (failed and failed.round) or 0,
        world_key = D.world_hash(self.wkey), flags = flags,
        spawn_id = loaded and self.applied_spawn_id or (placed and pc.spawn_id) or 0,
        load_error = D.load_error_code(env, failure_current and self.load_error or nil), arena = arena,
        life = loaded and ctx.life or (placed and pc.life) or 0,
    })
end

-- A u32 for a world key string (C2SGameStatus.world_key; 0 = none).
function D.world_hash(key)
    if type(key) ~= "string" or key == "" then return 0 end
    local h = 2166136261
    for i = 1, #key do h = ((h ~ key:byte(i)) * 16777619) & 0xFFFFFFFF end
    return h
end

-- The Director's load error names -> S.ENUMS.load_error codes.
D.LOAD_ERROR_CODE = { travel_failed = "TRAVEL_FAILED", wrong_world = "WRONG_WORLD", no_pawn = "NO_PAWN",
    vitals = "VITALS", spawn_timeout = "SPAWN_PLACE", kit_error = "DRESS", stand_ins = "STAND_INS",
    timeout = "TIMEOUT", world_timeout = "TIMEOUT", open_level_failed = "TRAVEL_FAILED" }
function D.load_error_code(env, name)
    if name == nil or name == "" then return 0 end
    return code(env, "load_error", D.LOAD_ERROR_CODE[name] or "OTHER", 9)
end

function Dir:heartbeat()
    local s = self.sess or {}
    self:bus_put("director", {
        hb = self.env.wall(), round = s.round or 0, ready_round = self.ready_round,
        travel_n = self.counters.travel, native_rewritten = self.counters.native_rewritten,
        in_match = self.in_match == true, rematch = self.rematch ~= nil, state = self.state, phase = s.phase or "",
        conn = self.conn.state or "", world_key = self.wkey or "", world = self.wshort or "", target = self.target or "",
        load_error = self.load_error or "", conn_reason = self.conn.reason or "",
    })
end

-- ---------------------------------------------------------------------------
-- The real env (UE4SS). Only main.lua calls this.
--   ctx: { WG, UEHelpers, log, ev, state_dir, SG }   (SG = shared/hsmp_saveguard.lua, optional)
function D.make_ue_env(ctx)
    local UEH, WG, Log = ctx.UEHelpers, ctx.WG, ctx.log
    local env = {}
    env.now = os.clock
    env.wall = os.time
    env.log = Log
    env.ev = ctx.ev or function() end
    -- The Runtime Vertex Paint quiesce (shared/hsmp_rvp.lua), optional.
    local RVP = ctx.RVP
    if RVP then
        function env.travel_hold(why)
            local ok, h = pcall(RVP.hold, why)
            return ok and h == true
        end
        function env.travel_issued() pcall(RVP.travel_issued) end
        function env.rvp_reopen(why) pcall(RVP.reopen, why) end
        function env.rvp_tick() pcall(RVP.tick) end
    end
    local SG = ctx.SG
    if SG then
        function env.sg_active()
            local ok, v = pcall(SG.is_active)
            return ok and v and true or false
        end
        function env.sg_force(v) pcall(SG.set_active, v) end
        function env.sg_seed()
            local ok, r, d = pcall(SG.seed_session_slot)
            if not ok then return false, tostring(r) end
            return r and true or false, d
        end
    end

    -- The session, link, bus keys and G2S records go through the IPC facade
    -- (env.ipc unset: this Lua state's HSMP_IPC; typed records). The path
    -- helpers below remain for .gi_backup.txt and .settings.json (real files)
    -- and HSMPLoadout's kit_status (its own contract).
    local function ipc() return rawget(_G, "HSMP_IPC") end
    function env.read(path)
        local I = ipc()
        return (I and I.read(path, true)) or nil
    end
    function env.read_all(path)
        local I = ipc()
        return (I and I.read(path)) or nil
    end
    -- Bus key kit_status (typed record, schema loadout.rs KitStatus; HSMPLoadout kit.lua
    -- writes it). nil = never written or cleared (pawn "").
    function env.kit_status()
        local I = ipc()
        local t = I and I.bus_table and I.bus_table("kit_status")
        if type(t) ~= "table" or (t.pawn or "") == "" then return nil end
        return t
    end
    function env.write_atomic(path, s)
        local I = ipc()
        return I and I.write(path, s) or false
    end
    -- G2S messages: a full ring queues them in the facade's bounded retry
    -- queue; env.flush() retries them on the next tick.
    function env.flush()
        local I = ipc()
        if I then I.flush() end
    end
    function env.remove(path)
        local I = ipc()
        return I and I.remove(path)
    end

    -- The world as the guard sees it this tick (fresh lookups only).
    function env.world()
        local ok = WG.check()
        return { ok = ok, short = ok and WG.short() or nil, key = ok and WG.key or nil }
    end

    function env.open_level(name)
        local ok = false
        pcall(function()
            local gs = UEH.GetGameplayStatics()
            local world = (WG.world and WG.world() or UEH.GetWorld())
            if gs and gs:IsValid() and world and world:IsValid() then
                gs:OpenLevel(world, FName(name), true, FString(""))
                ok = true
            end
        end)
        if not ok and name:match("^Map_Arena_") then
            pcall(function()
                -- KismetSystemLibrary:ExecuteConsoleCommand (as apply_cvars does):
                -- APlayerController:ConsoleCommand is not callable from UE4SS Lua.
                local pc = (WG.pc and WG.pc() or UEH.GetPlayerController())
                local ksl = UEH.GetKismetSystemLibrary()
                if pc and pc:IsValid() and ksl and ksl:IsValid() then
                    ksl:ExecuteConsoleCommand(pc, FString("open " .. D.ARENA_PATH(name)), pc)
                    ok = true
                    Log("director: console 'open %s' (OpenLevel unavailable)", D.ARENA_PATH(name))
                end
            end)
        end
        return ok
    end

    -- Fresh lookup every call: a pawn is never cached across ticks here.
    function env.pawn()
        local p
        -- A native stand-in spawn can temporarily possess another Willie.
        -- The verified AI-held fighter remains this life's owner throughout.
        if WG.ai_pawn then p = WG.ai_pawn() end
        if p then return p end
        pcall(function()
            local pc = (WG.pc and WG.pc() or UEH.GetPlayerController())
            if pc and pc:IsValid() then
                local pw = pc.Pawn
                if pw and pw:IsValid() then p = pw end
            end
        end)
        -- the game's own AI drives our pawn (dev, HSMPParity `ai on`)
        return p
    end
    function env.pawn_id(p)
        local id = "?"
        pcall(function() id = p:GetFName():ToString() end)
        return id
    end
    function env.pawn_get(p, k) local v; pcall(function() v = p[k] end); return v end
    function env.pawn_set(p, k, v) return pcall(function() p[k] = v end) end
    function env.pawn_call(p, fn)
        pcall(function() local f = p[fn]; if f then f(p) end end)
    end
    function env.pawn_loc(p)
        local l; pcall(function() l = p:K2_GetActorLocation() end)
        if l then return l.X, l.Y, l.Z end
        return nil
    end

    local cdo_cache = nil   -- plain numbers, not a UObject
    function env.cdo_vitals()
        if cdo_cache then return cdo_cache end
        local cdo
        pcall(function() cdo = StaticFindObject("/Game/Character/Blueprints/Willie_BP.Default__Willie_BP_C") end)
        if not cdo or not cdo:IsValid() then return nil end
        local t, n = {}, 0
        for _, k in ipairs(D.VITALS_FIELDS) do
            local v; pcall(function() v = tonumber(cdo[k]) end)
            if v then t[k] = v; n = n + 1 end
        end
        if n > 0 then cdo_cache = t end
        return cdo_cache
    end

    function env.gi_get(k)
        local v
        pcall(function()
            local gi = UEH.GetGameInstance()
            if gi and gi:IsValid() then v = gi[k] end
        end)
        return v
    end
    function env.gi_set(k, v)
        local ok = false
        pcall(function()
            local gi = UEH.GetGameInstance()
            if gi and gi:IsValid() then gi[k] = v; ok = (gi[k] == v) end
        end)
        return ok
    end
    -- Heal GI "Player Body Condition" (8 part healths) to the GI CDO's values
    -- (100 when the CDO cannot be read). Returns the number of fields that
    -- were below that, and a short list for the log; nil when not possible.
    function env.gi_heal_body()
        local n, worst = nil, {}
        pcall(function()
            local gi = UEH.GetGameInstance()
            if not (gi and gi:IsValid()) then return end
            local bc = gi[D.BODY_COND]
            if not bc then return end
            local cdo_bc
            pcall(function() cdo_bc = gi:GetClass():GetCDO()[D.BODY_COND] end)
            n = 0
            for _, f in ipairs(D.BODY_COND_FIELDS) do
                local want = 100
                pcall(function() local d = cdo_bc and cdo_bc[f]; if type(d) == "number" and d > 0 then want = d end end)
                local cur = bc[f]
                if type(cur) == "number" and cur < want then
                    bc[f] = want
                    n = n + 1
                    worst[#worst + 1] = string.format("%s=%.0f", f:match("^(%a+)") or f, cur)
                end
            end
        end)
        return n, table.concat(worst, " ")
    end

    -- Visible Willies other than the own pawn: not bHidden (a neutralised
    -- extra: SetActorHiddenInGame), its Mesh visible, and not the "Persistent"
    -- pooled template at the origin. Why not the obvious checks:
    -- AActor::IsHidden is not a UFunction (nil from Lua, so nothing ever reads
    -- as hidden), and ActorHasTag(FName("Persistent")) returns false on
    -- Willie_BP_C_0 although its Tags array is exactly ["Persistent"]; that
    -- Willie's Mesh is invisible (IsVisible=false), i.e. nobody can see it.
    local function has_tag(w, tag)
        local yes = false
        pcall(function()
            w.Tags:ForEach(function(_, el)
                local s; pcall(function() s = el:get():ToString() end)
                if s == tag then yes = true end
            end)
        end)
        return yes
    end
    -- No FindAllOf over Willies (nor a name read on what it returns) in the
    -- first WG.SETTLE_S of a world: the old world is still being purged and
    -- the new player Willie is mid-construction (uncatchable crashes 0.5-0.6 s
    -- after a round reload). nil, "settling" = not yet: the census
    -- step waits, the Live sample is skipped.
    function env.census(own)
        if WG and WG.settled and not WG.settled() then return nil, "settling" end
        local vis, parts = 0, {}
        pcall(function()
            for _, w in pairs(FindAllOf("Willie_BP_C") or {}) do
                if w and w:IsValid() and w ~= own then
                    local hidden = false
                    pcall(function() hidden = w.bHidden == true end)
                    if not hidden then
                        pcall(function()
                            local m = w.Mesh
                            if m and m:IsValid() and m:IsVisible() == false then hidden = true end
                        end)
                    end
                    local persistent = has_tag(w, "Persistent")
                    if not hidden and not persistent then
                        vis = vis + 1
                        local n = "?"; pcall(function() n = w:GetFName():ToString() end)
                        parts[#parts + 1] = n
                    end
                end
            end
        end)
        return vis, table.concat(parts, ",")
    end

    local spawn_ready = D.SpawnReady and D.SpawnReady.new()
    function env.combat_ready(p, s, own)
        if not spawn_ready then return false, "spawn proof helper unavailable" end
        if WG and WG.settled and not WG.settled() then return false, "world settling" end
        local I = ipc_of(env)
        if not I or not I.rec or not I.peer_rec or not I.bus_table then return false, "spawn proof IPC unavailable" end
        local mode = D.HS and D.HS.mode and D.HS.mode({ ipc = I, clock = env.now })
        local names, playback, actors = {}, {}, {}
        for _, row in ipairs((I.bus_table("puppets") or {}).rows or {}) do names[row.peer] = row.name end
        for _, row in ipairs((I.bus_table("playback") or {}).rows or {}) do playback[row.peer] = row end
        -- Look up this world's actors once. No UObject survives this call.
        local read = pcall(function()
            for _, actor in pairs(FindAllOf("Willie_BP_C") or {}) do
                if actor and actor:IsValid() then actors[env.pawn_id(actor)] = actor end
            end
        end)
        if not read then return false, "native stand-in lookup unavailable" end
        local remotes = {}
        for _, row in ipairs(s.roster or {}) do
            local mode_row = mode and mode.match_id == p.match_id and mode.round == p.round and mode.rows[row.id]
            local participating = s.phase ~= "live" or (row.alive ~= false
                and (not mode_row or (mode_row.alive ~= false and mode_row.respawning ~= true)))
            if row.id ~= s.my_id and row.role == "fighter" and participating then
                local id, name = row.id, names[row.id]
                local source = {}
                local slot = I.peer_slot and I.peer_slot(id)
                if slot ~= nil and I.peer_play then I.peer_play(slot, source) end
                local life = mode_row and mode_row.life or (p.round ~= s.round and 1 or nil)
                local actor, native = name and actors[name], {}
                if actor and actor ~= own then
                    pcall(function() native.alive = actor.Health > 0 and actor.DED == false end)
                    local best, best_score
                    for _, field in ipairs({ "SK_Skeleton", "BoneCore", "DriverSkeleton", "Mesh" }) do
                        pcall(function()
                            local mesh = actor[field]
                            if mesh and mesh:IsValid() then
                                local sim = mesh:IsSimulatingPhysics(FName("Pelvis")) == true
                                local score = (sim and 2 or 0) + (mesh:IsVisible() == true and 1 or 0)
                                if best_score == nil or score > best_score then best, best_score = mesh, score end
                            end
                        end)
                    end
                    pcall(function()
                        local collision = best and best:GetCollisionEnabled()
                        native.collision = actor:GetActorEnableCollision() == true
                            and best:IsSimulatingPhysics(FName("Pelvis")) == true and (collision == 2 or collision == 3)
                    end)
                end
                remotes[#remotes + 1] = { peer = id, pawn = name, match_id = p.match_id, round = p.round,
                    life = life, source = source, playback = playback[id], vitals = I.peer_rec("peer_vitals", id), native = native }
            end
        end
        local sampling = I.sample_status and I.sample_status()
        return spawn_ready:check({ world = p.key, own = { match_id = p.match_id, round = p.round,
            life = p.verified_life or p.life, pawn = p.pawn_id }, root = I.rec("local_root"),
            pose = sampling and sampling.pose, vitals = I.rec("vitals"), remotes = remotes, now_ms = env.now() * 1000 })
    end

    function env.freeze(p, on)
        pcall(function()
            local pc = (WG.pc and WG.pc() or UEH.GetPlayerController())
            if not (pc and pc:IsValid()) then return end
            if on then
                if p and p:IsValid() then pcall(function() p:DisableInput(pc) end) end
                pcall(function() pc:SetIgnoreMoveInput(true) end)
            else
                local cur = pc.Pawn
                if cur and cur:IsValid() then pcall(function() cur:EnableInput(pc) end) end
                pcall(function() pc:ResetIgnoreMoveInput() end)
            end
        end)
    end

    -- Runtime engine cvars (docs/development/halfsword/io-dispatcher-crash.md:
    -- hair-strand streaming reads past the end of the hair .ubulk on the IoDispatcher thread ->
    -- PAK_ASYNC_READ_OOB at the first arena load). Engine.ini overrides get
    -- reset by the game, so they are set at runtime: at boot and on every new
    -- world. Not a travel: only the listed cvars ever go through here.
    -- Returns ok, the read-back values ("name=value ...", "?" where the engine does not expose one).
    function env.apply_cvars()
        local ok, back = false, nil
        pcall(function()
            local ksl = StaticFindObject("/Script/Engine.Default__KismetSystemLibrary")
            local world = (WG.world and WG.world() or UEH.GetWorld())
            if not (ksl and ksl:IsValid() and world and world:IsValid()) then return end
            for _, cv in ipairs(D.RUNTIME_CVARS) do
                ksl:ExecuteConsoleCommand(world, FString(cv[1] .. " " .. cv[2]), nil)
                ok = true
            end
            local parts = {}
            for i = 1, D.IO1_CVARS do
                local name, v = D.RUNTIME_CVARS[i][1], "?"
                pcall(function() v = tostring(ksl:GetConsoleVariableIntValue(FString(name))) end)
                parts[#parts + 1] = name .. "=" .. v
            end
            back = table.concat(parts, " ")
        end)
        return ok, back
    end

    function env.game_input()
        pcall(function()
            local pc = (WG.pc and WG.pc() or UEH.GetPlayerController())
            if not (pc and pc:IsValid()) then return end
            local wbl = StaticFindObject("/Script/UMG.Default__WidgetBlueprintLibrary")
            if wbl and wbl:IsValid() then pcall(function() wbl:SetInputMode_GameOnly(pc, false) end) end
            pcall(function() pc:SetShowMouseCursor(false) end)
            pcall(function() pc.bShowMouseCursor = false end)
            pcall(function() pc:SetIgnoreLookInput(false) end)
        end)
    end
    return env
end

return D
