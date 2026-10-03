-- Offline test of the HSMPMenu screens (ui_kit / classes / browser / lobby)
-- with mocked UE4SS + UMG (lib/umg_mock.lua), under Lua 5.4. No game needed.
--
--     hsmp-tools lua-test menu_ui
--
-- What it does:
--   * loads the real HSMPMenu/Scripts/main.lua (which loads ui_kit.lua,
--     browser.lua, classes.lua and HSMPLoadout's hsmp_catalog.lua) against a
--     mock UE: StaticConstructObject / CanvasPanel slots / IsPressed / IsHovered,
--     the game-thread loop + delayed-action API, a fake clock and a temp
--     state dir. os.execute is stubbed (nothing is launched).
--   * drives the menu like a player: clicks are made by holding a widget's
--     IsPressed for 70 ms so main.lua's 33 ms edge poll fires its callback.
--   * checks layout at canvas 1920x1080 and 1280x720 (and 1280x720 with the
--     default DPI scale): every visible widget inside its panel, the panel inside
--     the canvas, no two clickable widgets overlap, no text overlaps another text
--     or partially covers a button, no text is taller than its box or wider than
--     its slot (estimated with ui_kit's CHAR_W / LINE_H).
--   * checks behaviour: class cards, slot -> picker, picker paging covers every
--     item exactly once, illegal / over-budget tiles greyed and not equippable,
--     budget bar, CLASSES ONLY lock, host rules chips, SAVE & BACK / BACK,
--     browser chips (toggles + exclusive ping group), sort, empty state,
--     single-click select / double-click join, lobby arena tiles driven by the
--     server's .match.json, START never loading locally, and the world guard
--     (no widget touched after a level change).
--   * travel and commands: the menu only REQUESTS travel (.travel_request.json /
--     HSMP_DIRECTOR, 1 s ack, legacy shim, shim removable), no OpenLevel / GI
--     writes outside legacy_travel.lua, cmd_send/cmd_status pending -> accepted
--     / refused (inferred, .cmd_results.jsonl, hsmp_net), a stale pick never
--     fights the server, HSMP_AUTOTEST_MAPS, hsmp_cfg paths, hsmp_log events.
--   * libraries and processes: legacy shim opt-in (HSMP_LEGACY_TRAVEL=1), the REAL shared
--     hsmp_cfg / hsmp_log (and an isolated copy without them), native spawn and
--     kill of our own children only (no shell, no image-name kill), HSMP_AUTOTEST=host|join, the
--     DevCtl autotest channel (dev_cmd records), auto-ready, lobby_ready{epoch}.
-- Every runtime scenario runs in its own Lua state (T.isolated).

local T = T
local mode, opts = ...
local SCRIPTS = T.path("mods/HSMPMenu/Scripts")
local UIS = dofile(T.path("mods/shared/hsmp_ui_scale.lua"))
local RES = dofile(T.path("tools/hsmp-tools/lua-tests/lib/ui_res.lua"))
local LOADOUT = T.path("mods/HSMPLoadout/Scripts")
local truthy, contains, startswith = T.truthy, T.contains, T.startswith

-- The code-only view used by the static lint: cut each line at the first "--".
local function strip_comments(src)
    return table.concat(T.map(T.lines(src), function(l)
        local i = l:find("--", 1, true)
        return i and l:sub(1, i - 1) or l
    end), "\n")
end
local function lstrip(s) return (s:gsub("^%s+", "")) end

-- Lint: only the removable shim may travel or write GI.
local function test_static_no_local_travel()
    local tag = "static"
    local files = {}
    for _, n in ipairs({ "main.lua", "ui_kit.lua", "classes.lua", "browser.lua", "commands.lua", "travel.lua", "local_master.lua" }) do
        files[#files + 1] = { n = n, src = T.read(SCRIPTS .. "/" .. n) }
    end
    for _, fe in ipairs(files) do
        local n, code = fe.n, strip_comments(fe.src)
        T.check(not contains(code, "OpenLevel"), tag .. ": " .. n .. " never calls OpenLevel")
        T.check(T.re_search([=[["']open ]=], code) == nil, tag .. ": " .. n .. " never issues a console 'open'")
        T.check(T.re_search([[gi\s*\[]], code) == nil and not contains(code, "gi_set"), tag .. ": " .. n .. " writes no GameInstance values")
        T.check(not contains(code, "Free Mode Foes Amount") and not contains(code, "set_gi_for_map"),
            tag .. ": " .. n .. " has no GI profile / foe-count writes")
    end
    local leg = T.read(SCRIPTS .. "/legacy_travel.lua")
    T.check(contains(leg, "OpenLevel") and contains(leg, "DELETE THIS FILE"), tag .. ": legacy_travel.lua is the one marked, removable shim")
    local main = files[1].src
    local function dlines(src)
        return T.filter(T.lines(src), function(l) return contains(l, "D:/") and not startswith(lstrip(l), "--") end)
    end
    local dpaths = dlines(main)
    T.check(#dpaths == 0, tag .. ": main.lua has no D:/ path (hsmp_cfg owns paths), got " .. T.repr(dpaths))
    T.check(contains(main, '"pick_arena"') and contains(main, 'cmd_send("start"') and contains(main, 'cmd_send("ready"')
        and contains(main, 'cmd_send("best_of"'), tag .. ": every host action goes through cmd_send")
    -- hsmp_cfg call sites (docs/development/testing.md): no direct env reads for paths / URLs
    local code = strip_comments(main)
    for _, k in ipairs({ "HSMP_SERVER_EXE", "HSMP_SIDECAR_EXE", "HSMP_QUERY_EXE" }) do
        T.check(T.count(code, '"' .. k .. '"') <= 1, tag .. ": main.lua reads " .. k .. " only in the no-library fallback")
    end
    T.check(contains(code, 'cfg("server_exe")') and contains(code, 'cfg("sidecar_exe")') and contains(code, 'cfg("query_exe")')
        and contains(code, 'cfg("master_url")') and contains(code, 'cfg("state_dir")'), tag .. ": main.lua takes every path from hsmp_cfg")
    -- DoD-12: no kill by image name in any HSMP menu file; PID + image-guarded kill only
    for _, fe in ipairs(files) do
        T.check(T.re_search([[(?i)task.?kill[^\n]*/IM\b]], strip_comments(fe.src)) == nil, tag .. ": " .. fe.n .. " never kills by image name")
    end
    -- Our binaries are spawned and killed through the native module only
    for _, fe in ipairs(files) do
        local c = strip_comments(fe.src)
        if fe.n ~= "browser.lua" and fe.n ~= "settings.lua" then
            T.check(not contains(c:lower(), "taskkill") and not contains(c, 'start ""') and not contains(c, ".pid.")
                and not contains(c, ".caps.") and not contains(c, "--help"),
                tag .. ": " .. fe.n .. " has no shell launch / taskkill / pid file / --help probe")
        end
    end
    T.check(contains(code, "ipc.proc_kill(pid)") and contains(code, "ipc.spawn(exe, args"), tag .. ": main.lua spawns and kills through IPC (our process handles)")
    T.check(not contains(code, "autotest.cmd") and contains(code, "ipc.dev_poll("), tag .. ": the autotest channel is the DevCtl ring (dev_poll), not a file")
    -- the legacy shim is opt-in
    T.check(T.re_search([[if LEGACY_TRAVEL_ON then\s+Legacy = load_module\("legacy_travel"]], code) ~= nil
        and contains(code, 'os.getenv("HSMP_LEGACY_TRAVEL") == "1"'), tag .. ": legacy_travel.lua is loaded only with HSMP_LEGACY_TRAVEL=1")
end

if mode == "dump" then T.isolated(T.script, "case", { kind = "dump" }); return end
if mode ~= "case" then
    -- ---- 0. every HSMPMenu file parses ------------------------------------------
    for _, n in ipairs({ "main.lua", "ui_kit.lua", "classes.lua", "browser.lua", "commands.lua", "travel.lua", "legacy_travel.lua",
                         "local_master.lua" }) do
        local f, err = load(T.read(SCRIPTS .. "/" .. n), "@" .. n)
        T.check(f ~= nil, "parse " .. n .. ": " .. tostring(err))
    end
    T.isolated(T.script, "case", { kind = "res", vw = 1920, vh = 1080, scale = 1.0, tag = "1920x1080" })
    T.isolated(T.script, "case", { kind = "res", vw = 1280, vh = 720, scale = 1.0, tag = "1280x720 canvas" })
    T.isolated(T.script, "case", { kind = "res", vw = 1280, vh = 720, tag = "1280x720 dpi-default" })   -- scale nil
    test_static_no_local_travel()
    for n = 1, 9 do T.isolated(T.script, "case", { kind = "director", n = n }) end
    for n = 1, 2 do T.isolated(T.script, "case", { kind = "cmd", n = n }) end
    T.isolated(T.script, "case", { kind = "authority" })
    T.isolated(T.script, "case", { kind = "autotest" })
    -- the cfg scenarios use the REAL shared/hsmp_cfg.lua + hsmp_log.lua (an hsmp.cfg in libdir);
    -- cfg 3/4 hide them with an isolated copy of HSMPMenu/Scripts
    local libdir = T.tmpdir("hsmp_shared_")
    for n = 1, 4 do T.isolated(T.script, "case", { kind = "cfg", n = n, libdir = libdir }) end
    for n = 1, 3 do T.isolated(T.script, "case", { kind = "pid", n = n }) end
    for n = 1, 7 do T.isolated(T.script, "case", { kind = "at", n = n }) end
    for n = 1, 14 do T.isolated(T.script, "case", { kind = "net", n = n }) end
    -- polish + ui-scale: every screen at every tested resolution (lib/ui_res.lua: 16:9, ultrawide,
    -- 32:9, 16:10, 4:3, 5:4, small windows) x DPI 1 / 1.25 / 1.5 / 2 / the default curve
    for _, r in ipairs(RES.RES) do
        for _, d in ipairs(RES.DPI) do
            T.isolated(T.script, "case", { kind = "matrix", vw = r[1], vh = r[2], dpi = d or nil })
        end
    end
    -- ui-scale: the in-game probe (GameViewportClient is C++-only), live resize, ribbons
    for n = 1, 6 do T.isolated(T.script, "case", { kind = "resize", n = n }) end
    for n = 1, 5 do T.isolated(T.script, "case", { kind = "nav", n = n }) end
    for n = 1, 2 do T.isolated(T.script, "case", { kind = "settings", n = n }) end
    for n = 1, 12 do T.isolated(T.script, "case", { kind = "lobby", n = n }) end
    for n = 1, 2 do T.isolated(T.script, "case", { kind = "proc", n = n }) end
    T.isolated(T.script, "case", { kind = "perf" })
    return
end

-- ================================ one runtime ================================
local check = T.check
local M = require("umg_mock")
local sd
local function raw_write(name, text) T.write(sd .. "/" .. name, text) end
local function raw_read(name) return T.read(sd .. "/" .. name) end

-- ---- the sidecar's side as typed records (schema session.rs) -------------------------------
-- The scenarios below still describe what the sidecar / Director say in their old file
-- vocabulary (.sidecar.json, .match.json, .chat.log, .conn_state.json, ...): `write`,
-- `append`, `read`, `remove_` and `exists_` translate those names into the typed records the
-- menu now reads (slots `link` / `session`, S2G `chat_in` / `cmd_result` events, typed bus
-- keys) and the typed `command` / `leave` records it sends back into readable text.
local SCH = dofile(T.path("mods/shared/hsmp_ipc_schema.lua"))
local REC = require("hsmp_native_records")
local TS = nil
local function NATIVE() return rawget(_G, "HSMPNative") end
-- S2G typed events for the file mock (it has no event log of its own for record kinds):
-- queued here and handed out by a wrapped HSMPNative.poll after the mock's own events.
local function hook_events(N)
    if not N or N._menu_events then return end
    N._menu_events = {}
    local base = N.poll
    N.poll = function(max, out)
        local n = base(max, out)
        local q = N._menu_events
        while n < max and #q > 0 do
            local e = table.remove(q, 1)
            n = n + 1
            out[n] = { kind = e.kind, req_id = 0, peer = 0, aux = 0, data = e.data }
        end
        for i = n + 1, #out do out[i] = nil end
        return n
    end
end
local function typed_reset(wipe)
    local N = NATIVE()
    hook_events(N)
    -- a new boot: the previous boot's sidecar records are gone (versions keep counting)
    if wipe ~= false and N and N._rec then
        for name, c in pairs(N._rec.slots) do N._rec.slots[name] = { ver = c.ver + 100, t = nil } end
        for key, b in pairs(N._rec.bus) do N._rec.bus[key] = { ver = b.ver + 100, t = nil } end
    end
    TS = { sd = sd, sc = nil, match = nil, sess = nil, metrics = nil, rtl = 0,
           sent0 = (N and N._rec) and #N._rec.sends or 0 }
    TS.boot_sent0 = TS.sent0
    return TS
end
local function ts() return TS or typed_reset(false) end
local function u64(s)   -- an exact u64 digit string -> Lua integer (wraps like the record)
    local v = 0
    for d in tostring(s):gmatch("%d") do v = v * 10 + (d:byte() - 48) end
    return v
end
local ADMIN_CODE = { none = 0, moderator = 1, admin = 2, owner = 3 }
local PHASE_OF = { lobby = 0, loading = 1, countdown = 2, live = 3, roundover = 4, match_over = 5, paused = 7 }
local function publish()
    local s, N = ts(), NATIVE()
    local sc, m, x = s.sc, s.match, s.sess
    if sc then
        local mt = s.metrics or {}
        N.sc_put("link", {
            status = SCH.ENUMS.sidecar_status[tostring(sc.status or "connecting"):upper()] or 0, state = 1,
            my_peer_id = tonumber(sc.peer_id) or 0, is_admin = sc.is_admin == true,
            metrics_wall_ms = s.metrics and 1 or 0, rtt_ms = tonumber(mt.rtt_ms) or 0, srtt_ms = tonumber(mt.srtt_ms) or 0,
        })
        -- the sidecar gives every connected roster peer a PeerDir entry
        local dir = {}
        for _, p in ipairs(sc.peers or {}) do
            if tonumber(p.id) then dir[#dir + 1] = { id = tonumber(p.id), nick = tostring(p.nick or ""), rtt_ms = tonumber(p.rtt_ms) or 0 } end
        end
        if N.sc_peer_dir then N.sc_peer_dir(dir) end
    end
    if not (sc or m or x or s.kr) then return end
    local me = sc and tonumber(sc.peer_id) or 0
    local ready, admin = {}, {}
    for _, id in ipairs((m and m.ready) or {}) do ready[tonumber(id)] = true end
    for _, r in ipairs((x and x.roster) or {}) do
        if type(r) == "table" and tonumber(r.peer_id) then admin[tonumber(r.peer_id)] = ADMIN_CODE[tostring(r.admin_role)] or 0 end
    end
    local host
    for _, p in ipairs((sc and sc.peers) or {}) do
        local id = tonumber(p.id)
        if id and id ~= me and (host == nil or id < host) then host = id end
    end
    local rows = {}
    for i, p in ipairs((sc and sc.peers) or {}) do
        local id = tonumber(p.id)
        -- without an explicit roster (.session.json) the real server still names its admin:
        -- me when the link says so, else the listen host (the lowest other peer id)
        local role = admin[id]
        if role == nil then
            if sc.is_admin == true then role = (id == me) and 2 or 0 else role = (id == host) and 3 or 0 end
        end
        rows[#rows + 1] = { seat = i, peer_id = id, nick = tostring(p.nick or ""), connected = true, alive = true,
                            ready = ready[id] == true, admin_role = role }
    end
    local state = (m and m.state) or (x and x.phase) or "lobby"
    local phase = PHASE_OF[state] or 0
    local arena = (m and m.arena) or ""
    local cfg = { arena = phase == 0 and arena or "", best_of = (m and tonumber(m.best_of)) or 3, countdown_s = 3,
                  kit_mode = 0, kit_budget = 0, mode = 0 }
    if s.kr then cfg.kit_mode, cfg.kit_budget = tonumber(s.kr.mode) or 0, tonumber(s.kr.budget) or 0 end
    local xc = x and x.config
    if type(xc) == "table" then
        local kr = type(xc.kit_rules) == "table" and xc.kit_rules or {}
        cfg.kit_mode, cfg.kit_budget = tonumber(kr.mode_code) or 0, tonumber(kr.budget) or 0
        cfg.mode = SCH.ENUMS.game_mode[tostring(xc.mode or "duel"):upper()] or 0
    end
    local frozen = {}
    for k, v in pairs(cfg) do frozen[k] = v end
    frozen.arena = arena
    local cd = (m and tonumber(m.countdown_s)) or 0
    local dl = (x and tonumber(x.deadline_in_ms)) or (cd > 0 and cd * 1000) or 0
    s.seq = (s.seq or 0) + 1
    N.sc_put("session", {
        epoch = sc and sc.epoch and u64(sc.epoch) or 77, seq = s.seq, round = (m and tonumber(m.round)) or 0,
        phase = phase, winner_seat = 255, phase_deadline_ms = dl > 0 and (100000 + dl) or 0, server_time_ms = 100000,
        has_frozen = phase ~= 0, config = cfg, frozen = frozen, rows = rows,
    })
end
-- Flat JSON of a record table (bus keys read back as text).
local function jtext(t)
    local keys = {}
    for k in pairs(t) do keys[#keys + 1] = k end
    table.sort(keys)
    local parts = {}
    for _, k in ipairs(keys) do
        local v = t[k]
        if type(v) == "string" then parts[#parts + 1] = string.format("%q:%q", k, v)
        elseif type(v) == "number" or type(v) == "boolean" then parts[#parts + 1] = string.format("%q:%s", k, tostring(v)) end
    end
    return "{" .. table.concat(parts, ",") .. "}\n"
end
-- The typed `command` records sent since the last clear, in the old line vocabulary.
local function cmd_text(r)
    local O, C = SCH.ENUMS.cmd_op, SCH.ENUMS.cfg
    if r.op == O.PICK_ARENA then return string.format('{"kind":"set_map","map":"%s","cmd_id":%d}', r.text, r.cmd_id)
    elseif r.op == O.SET_CONFIG and r.patch.mask == C.BEST_OF then
        return string.format('{"kind":"admin","verb":"set_best_of:%d","cmd_id":%d}', r.patch.best_of, r.cmd_id)
    elseif r.op == O.SET_CONFIG and r.patch.mask == C.KIT_RULES then
        return string.format('{"cmd":"kit_rules","mode":%d,"budget":%d,"cmd_id":%d}', r.patch.kit_mode, r.patch.kit_budget, r.cmd_id)
    elseif r.op == O.START then return string.format('{"kind":"match","verb":"start","cmd_id":%d}', r.cmd_id)
    elseif r.op == O.ABORT then return string.format('{"kind":"match","verb":"abort","cmd_id":%d}', r.cmd_id)
    elseif r.op == O.READY then return string.format('{"kind":"match","verb":"%s","cmd_id":%d}', r.flag and "ready" or "unready", r.cmd_id)
    elseif r.op == O.KICK then return string.format('{"kind":"admin","verb":"kick:%d","cmd_id":%d}', r.peer_id, r.cmd_id)
    elseif r.op == O.PROMOTE then return string.format('{"kind":"admin","verb":"promote:%d","cmd_id":%d}', r.peer_id, r.cmd_id) end
    return string.format('{"op":%d,"cmd_id":%d}', r.op, r.cmd_id)
end
local function sent_since(i0, kind)
    local N, out = NATIVE(), {}
    for i = i0 + 1, #N._rec.sends do
        local e = N._rec.sends[i]
        if e.kind == kind then out[#out + 1] = e.data end
    end
    return out
end
local function commands_text()
    local lines = {}
    for _, r in ipairs(sent_since(ts().sent0, "command")) do lines[#lines + 1] = cmd_text(r) end
    return #lines > 0 and (table.concat(lines, "\n") .. "\n") or nil
end
local function leave_sent() return #sent_since(ts().boot_sent0, "leave") > 0 end
local BUS_KEY = { [".travel_request.json"] = "travel_request", [".travel_ack.json"] = "travel_ack",
                  [".director.json"] = "director", [".conn_state.json"] = "conn_state", [".return_to_lobby.flag"] = "return_to_lobby" }

local function write(name, text)
    local s, N = ts(), NATIVE()
    if name == ".sidecar.json" then s.sc = T.json_decode(text) or s.sc; publish(); return end
    if name == ".match.json" then s.match = T.json_decode(text) or s.match; publish(); return end
    if name == ".session.json" then s.sess = T.json_decode(text) or s.sess; publish(); return end
    if name == ".kit_rules.json" then   -- the server's kit rules: the session config carries them
        s.kr = T.json_decode(text); publish(); raw_write(name, text); return
    end
    if name == ".metrics.json" then s.metrics = T.json_decode(text); publish(); return end
    if name == ".chat.log" then return end
    if name == ".return_to_lobby.flag" then s.rtl = s.rtl + 1; N.bus_put("return_to_lobby", { seq = s.rtl }); return end
    if name == ".conn_state.json" then
        local t = T.json_decode(text) or {}
        local a = {}
        for i, v in ipairs(type(t.actions) == "table" and t.actions or {}) do a[i] = v end
        N.bus_put("conn_state", { state = t.state or "", reason = t.reason or "", latched = t.latched == true,
                                  wall = tonumber(t.wall) or 0, actions = a, seq = tonumber(t.seq) or 0 })
        return
    end
    if BUS_KEY[name] then N.bus_put(BUS_KEY[name], T.json_decode(text) or {}); return end
    raw_write(name, text)
end
local function read(name)
    if name == ".control.outbox.jsonl" then return commands_text() end
    if name == ".leave_request.json" then
        local l = sent_since(ts().boot_sent0, "leave")
        return l[#l] and jtext(l[#l]) or nil
    end
    if BUS_KEY[name] then
        local N = NATIVE()
        local _, t = N.bus_get(BUS_KEY[name], nil)
        if type(t) ~= "table" or (t.seq == 0 and t.state == nil) or t.state == "" then return nil end
        return jtext(t)
    end
    return raw_read(name)
end
local function append(name, text)
    local N = NATIVE()
    if name == ".chat.log" or name == ".cmd_results.jsonl" then
        for line in tostring(text):gmatch("[^\n]+") do
            local t = T.json_decode(line)
            if type(t) == "table" then
                local kind, rec
                if name == ".chat.log" then
                    kind, rec = "chat_in", { from_peer = tonumber(t.peer_id) or 0, nick = t.nick or "", text = t.text or "" }
                else
                    kind, rec = "cmd_result", { cmd_id = tonumber(t.cmd_id) or 0, ok = t.ok == true,
                        reason_text = t.reason_text or "", reason_code = (t.ok == true) and 0 or 3 }
                end
                rec = assert(REC.marshal(SCH, kind, rec))
                hook_events(N)
                table.insert(N._menu_events, { kind = kind, data = rec })
            end
        end
        return
    end
    T.append(sd .. "/" .. name, text)
end
-- remove_ / exists_: the translated names (other names are files).
local function remove_(name)
    if name == ".control.outbox.jsonl" then ts().sent0 = #NATIVE()._rec.sends; return end
    if name == ".conn_state.json" then NATIVE().bus_put("conn_state", {}); return end   -- cleared (zeroed record)
    T.remove(sd .. "/" .. name)
end
local function exists_(name)
    if name == ".leave_request.json" then return leave_sent() end
    if name == ".sidecar.json" then return ts().sc ~= nil end   -- the link record is published
    return T.exists(sd .. "/" .. name)
end

-- make_runtime: mock + env (false = unset) + optional pre-Lua / extra package dir, then
-- load main.lua and inject into the startup menu.
-- scripts_dir: load HSMPMenu from a copy (isolated: no shared/ dir next to it).
-- boot_extra: mock knobs (gvc_broken = the real game's C++-only GameViewportClient path).
local boot_extra = {}
-- The menu starts its binaries through the native module (HSMPNative = the
-- per-state mock; lib/hsmp_native_records.lua). Every spawn / capture / kill is also
-- recorded in M.execs as one readable line, so the checks see processes in order:
--   spawn "<exe>" <args...> [env K=V ...] -> <pid>
--   capture "<exe>" <args...> -> <h>
--   proc_kill <pid> -> true | nil <err>
local function hook_native()
    local N = rawget(_G, "HSMPNative")
    if type(N) ~= "table" or N._menu_hooked or not N.spawn then return end
    N._menu_hooked = true
    local sp, cap, kill = N.spawn, N.spawn_capture, N.proc_kill
    local function render(kind, exe, args, o)
        local s = kind .. ' "' .. tostring(exe) .. '"'
        for _, a in ipairs(args or {}) do s = s .. " " .. tostring(a) end
        if type(o) == "table" and type(o.env) == "table" then
            local ks = T.keys(o.env)
            table.sort(ks)
            if #ks > 0 then
                s = s .. " [env"
                for _, k in ipairs(ks) do s = s .. " " .. k .. "=" .. tostring(o.env[k]) end
                s = s .. "]"
            end
        end
        return s
    end
    N.spawn = function(exe, args, o)
        local pid, e = sp(exe, args, o)
        table.insert(M.execs, render("spawn", exe, args, o) .. " -> " .. tostring(pid or e))
        return pid, e
    end
    N.spawn_capture = function(exe, args, o)
        local h, e = cap(exe, args, o)
        table.insert(M.execs, render("capture", exe, args, o) .. " -> " .. tostring(h or e))
        return h, e
    end
    N.proc_kill = function(pid)
        local ok, e = kill(pid)
        table.insert(M.execs, "proc_kill " .. tostring(pid) .. " -> " .. (ok and "true" or ("nil " .. tostring(e))))
        return ok, e
    end
end
local function boot(vw, vh, scale, env, pre_lua, extra_path, scripts_dir)
    sd = T.tmpdir(opts.kind == "res" and "hsmp_ui_" or "hsmp_p0_")
    typed_reset()
    -- every HSMP_* knob the menu reads is pinned (false = unset) so the host env never leaks in
    local e = { HSMP_AUTOTEST = false, HSMP_AUTOTEST_MAPS = false, HSMP_MASTER_URL = false, HSMP_SERVER_EXE = false,
                HSMP_SIDECAR_EXE = false, HSMP_QUERY_EXE = false, HSMP_NETSIM_ADDR = false, HSMP_LOBBY_MAP = false,
                HSMP_LEGACY_TRAVEL = false, HSMP_BIN_DIR = false, HSMP_CFG = false, HSMP_AUTOTEST_ADDR = false,
                HSMP_AUTOTEST_EXTERNAL = false, HSMP_AUTOTEST_READY = false, HSMP_AUTOTEST_WAIT_MS = false,
                HSMP_INST = false, HSMP_DEV = false, HSMP_LOG_ECHO = false }
    for k, v in pairs(env or {}) do e[k] = v end
    hook_native()
    -- a second boot in one case starts from scratch (no module state carried over)
    for _, n in ipairs({ "ui_kit", "browser", "classes", "settings", "commands", "travel", "legacy_travel", "local_master",
                         "jsonlite", "hsmp_cfg", "hsmp_log", "hsmp_net", "hsmp_arenas", "hsmp_catalog" }) do
        package.loaded[n] = nil
    end
    HSMP_MENU_TEST = nil
    M.install({ vw = vw, vh = vh, scale = scale, state_dir = sd, env = e, strict = true,
                gvc_broken = boot_extra.gvc_broken, wll_size_broken = boot_extra.wll_size_broken })
    local dir = scripts_dir or SCRIPTS
    local ep = extra_path and (extra_path .. "/?.lua;") or ""
    package.path = ep .. dir .. "/?.lua;" .. LOADOUT .. "/?.lua;" .. package.path
    if pre_lua then assert(load(pre_lua, "=pre_lua"))() end
    local f, err = load(T.read(dir .. "/main.lua"), "@" .. dir .. "/main.lua")
    if not f then error(err) end
    f()
    -- the startup menu appears -> inject after 500 ms
    M.on_new(M.menu)
    M.run(700)
end

-- The loadout domain's typed slots (schema loadout.rs), played for the sidecar / read back.
local KIT_REV = 0
local function kit_remote(pid, class, r, l, armor)   -- per-peer "peer_kit" (a kit_verdict record)
    KIT_REV = KIT_REV + 1
    local rows = {}
    for i, a in ipairs(armor or {}) do rows[i] = { id = a } end
    HSMPNative.sc_put("peer_kit", { class = class, r = r or "", l = l or "", rev = KIT_REV, rows = rows }, pid)
end
local function kit_rules(mode, budget)                -- slot "kit_rules" (the server's rules)
    KIT_REV = KIT_REV + 1
    HSMPNative.sc_put("kit_rules", { mode = mode, budget = budget, rev = KIT_REV })
end
local function req_text()                             -- slot "kit_rules_req" as text ("" = no request)
    local t = HSMPNative.sc_get("kit_rules_req")
    if not t or t.seq == 0 then return "" end
    return string.format('{"seq":%d,"mode":%d,"budget":%d}', t.seq, t.mode, t.budget)
end
local function own_kit()                              -- slot "kit" in the old .kit.json shape
    local t = HSMPNative.sc_get("kit")
    if not t then return {} end
    local armor = {}
    for i, r in ipairs(t.rows or {}) do armor[i] = r.id end
    return { seq = t.seq, class = t.class, r = t.r, l = t.l, armor = armor, cos = t.cos }
end
local function logtext() return table.concat(T.map(M.logs, tostring), "\n") end
local function strip(s) return (s:gsub("^%s+", ""):gsub("%s+$", "")) end
local function ulen(s) return utf8.len(s) or #s end

local function rects_overlap(a, b)
    return a.x < b.x + b.w - 0.5 and b.x < a.x + a.w - 0.5 and
        a.y < b.y + b.h - 0.5 and b.y < a.y + a.h - 0.5
end

local function inside(a, b, tol)
    tol = tol or 0.5
    return a.x >= b.x - tol and a.y >= b.y - tol and
        a.x + a.w <= b.x + b.w + tol and a.y + a.h <= b.y + b.h + tol
end

-- dpi: the viewport scale (physical px = canvas units * dpi); strict: no visible
-- label may be clipped (end in "..") - every static text must fit its slot.
local function layout_checks(tg, cw, ch, dpi, strict)
    dpi = dpi or 1
    local live = {}
    for _, w in ipairs(M.live()) do
        if truthy(w.name) and startswith(tostring(w.name), "HSMPUI_") then live[#live + 1] = w end
    end
    check(#live > 20, string.format("%s: screen has widgets (%d)", tg, #live))
    if #live == 0 then return end
    local panel
    for _, w in ipairs(live) do
        if w.cls == "Border" and (panel == nil or w.w * w.h > panel.w * panel.h) then panel = w end
    end
    check(panel.x >= -cw / 2 and panel.y >= -ch / 2 and panel.x + panel.w <= cw / 2
        and panel.y + panel.h <= ch / 2, string.format("%s: panel %sx%s inside canvas %sx%s", tg, panel.w, panel.h, cw, ch))
    -- ui-scale: the panel keeps the safe margin and never stretches past 16:9 of the height (ultrawide)
    local s = UIS.scale(cw, ch)
    local m = UIS.margin(cw, ch, s)
    check(panel.x >= -cw / 2 + m - 1 and panel.y >= -ch / 2 + m - 1 and panel.x + panel.w <= cw / 2 - m + 1
        and panel.y + panel.h <= ch / 2 - m + 1, string.format("%s: panel inside the %d-unit safe margin", tg, m))
    check(panel.w <= UIS.max_width(cw, ch, s) + 1 and panel.w <= ch * 16 / 9 + 1,
        string.format("%s: panel width %s <= max %d (16:9 of the height)", tg, panel.w, UIS.max_width(cw, ch, s)))
    local bad = T.filter(live, function(w) return not inside(w, panel) end)
    check(#bad == 0, string.format("%s: %d widget(s) outside the panel, e.g. %s", tg, #bad,
        T.repr(T.map({ bad[1], bad[2] }, function(w) return w.name end))))
    local buttons = T.filter(live, function(w) return w.cls == "Button" end)
    local texts = T.filter(live, function(w) return w.cls == "TextBlock" and strip(w.text or "") ~= "" end)
    local inputs = T.filter(live, function(w) return w.cls == "EditableTextBox" end)
    local clickables = {}
    for _, w in ipairs(buttons) do clickables[#clickables + 1] = w end
    for _, w in ipairs(inputs) do clickables[#clickables + 1] = w end
    local ov = {}
    for i, a in ipairs(clickables) do
        for j = i + 1, #clickables do
            local b = clickables[j]
            if rects_overlap(a, b) then ov[#ov + 1] = { a.name, b.name } end
        end
    end
    check(#ov == 0, string.format("%s: %d overlapping clickable pair(s), e.g. %s", tg, #ov, T.repr({ ov[1], ov[2] })))
    local tov = {}
    for i, a in ipairs(texts) do
        for j = i + 1, #texts do
            local b = texts[j]
            if rects_overlap(a, b) then tov[#tov + 1] = { a.text, b.text } end
        end
    end
    check(#tov == 0, string.format("%s: %d overlapping text pair(s), e.g. %s", tg, #tov, T.repr({ tov[1], tov[2], tov[3] })))
    local partial = {}
    for _, t in ipairs(texts) do
        for _, b_ in ipairs(clickables) do
            if rects_overlap(t, b_) and not inside(t, b_) then partial[#partial + 1] = { t.text, b_.name } end
        end
    end
    check(#partial == 0, string.format("%s: %d text(s) partially over a button, e.g. %s", tg, #partial,
        T.repr({ partial[1], partial[2], partial[3] })))
    local short, wide, tiny = {}, {}, {}
    for _, t in ipairs(texts) do
        local fs = truthy(t.fs) and t.fs or 24
        local lines = truthy(t.wrap) and 2 or 1
        if t.h + 1.0 < fs * 1.4 * lines - 0.01 and not (lines == 2 and t.h + 1.0 >= fs * 1.4) then
            short[#short + 1] = { t.text, t.h, fs }
        end
        if ulen(t.text) * fs * 0.6 > t.w * lines + 1 then
            wide[#wide + 1] = { t.text, t.w, fs }
        end
        if fs < UIS.min_font(dpi, s) - 0.01 or (s * dpi >= UIS.FLOOR_S - 1e-6 and fs * dpi < UIS.MIN_FONT_PX - 0.01) then
            tiny[#tiny + 1] = { t.text, fs, dpi }
        end
    end
    if strict then
        local clipped = T.filter(texts, function(t) return t.text:match("%.%.$") ~= nil and not t.text:match("%.%.%.$") end)
        check(#clipped == 0, string.format("%s: %d clipped label(s), e.g. %s", tg, #clipped,
            T.repr(T.map({ clipped[1], clipped[2], clipped[3] }, function(t) return t.text end))))
    end
    check(#short == 0, string.format("%s: %d text(s) taller than their box, e.g. %s", tg, #short, T.repr({ short[1], short[2], short[3] })))
    check(#wide == 0, string.format("%s: %d text(s) wider than their slot, e.g. %s", tg, #wide, T.repr({ wide[1], wide[2], wide[3] })))
    check(#tiny == 0, string.format("%s: %d text(s) under 9 physical px, e.g. %s", tg, #tiny, T.repr({ tiny[1], tiny[2], tiny[3] })))
end

local function test_resolution()
local vw, vh, scale, tag = opts.vw, opts.vh, opts.scale, opts.tag
boot(vw, vh, scale)
local cw, ch
if truthy(scale) then cw, ch = vw / scale, vh / scale else cw, ch = vw / (vh / 1080), vh / (vh / 1080) end
local logs = logtext()
check(contains(logs, "top-ribbon injection complete"), tag .. ": menu injected")
local Classes = package.loaded["classes"]
local Browser = package.loaded["browser"]
local Kit = package.loaded["ui_kit"]
check(Classes ~= nil and Browser ~= nil and Kit ~= nil, tag .. ": modules loaded")
check(contains(logs, "catalogue: 244 items, 5 classes"), tag .. ": catalogue loaded (244 items)")

-- ---- lobby (host) -------------------------------------------------------------
write(".sidecar.json", '{"status":"connected","peer_id":1,"is_admin":true,"peers":[{"id":1,"nick":"Willie"},{"id":2,"nick":"Mate"}]}\n')
write(".match.json", '{"state":"lobby","countdown_s":0,"arena":"Map_Arena_Yard","round":0,"best_of":3,"ready":[2]}\n')
kit_remote(2, "knight", "w_longsword3", "")
-- host flags live in main.lua's local `lobby`; drive HOST GAME through its ribbon
M.execs = {}
-- spawn_server_and_sidecar -> lobby after 1 s
local rib = T.filter(M.live(), function(w) return startswith(tostring(w.name), "HSMPBtn_top1") end)
check(#rib == 1, tag .. ": HOST GAME ribbon present")
M.click(rib[1].obj)
M.run(1600)
logs = logtext()
check(contains(logs, "enter screen: lobby"), tag .. ": HOST GAME opens the lobby")
layout_checks(tag .. " lobby", cw, ch)
local ui = Kit.cur
check(ui ~= nil and ui.screen == "lobby", tag .. ": lobby is a ui_kit build")
local w = ui.w
-- arena tile shows the server's arena (Yard), not the local pick
local mk = w.maps["keys"]
local on = {}
for i = 1, #mk do if truthy(w.maps.chips[i].on) then on[#on + 1] = mk[i] end end
check(T.eq(on, { "Map_Arena_Yard" }), tag .. ": lobby highlights the server arena (got " .. T.repr(on) .. ")")
local outbox = read(".control.outbox.jsonl") or ""
check(contains(outbox, '"kind":"set_map","map":"Map_Arena_Alley"'),
    tag .. ": host sends its pick when the lobby opens / server differs")
-- host clicks PIT: set_map immediately + optimistic highlight <= 1.5 s
remove_(".control.outbox.jsonl")
local pit = w.maps.by_key["Map_Arena_Pit"]
M.click(pit.button)
outbox = read(".control.outbox.jsonl") or ""
check(contains(outbox, '"kind":"set_map","map":"Map_Arena_Pit"'), tag .. ": map tile sends set_map immediately")
check(truthy(pit.pending) and not truthy(pit.on) and truthy(w.maps.by_key["Map_Arena_Yard"].on),
    tag .. ": the click shows the pick as PENDING; the server's arena stays selected")
check(pit.painted == "pend", tag .. ": pending tile painted amber (" .. tostring(pit.painted) .. ")")
M.run(2200)
check(not truthy(pit.on) and truthy(w.maps.by_key["Map_Arena_Yard"].on),
    tag .. ": without a server echo the highlight returns to the server arena")
write(".match.json", '{"state":"lobby","countdown_s":0,"arena":"Map_Arena_Pit","round":0,"best_of":3,"ready":[2]}\n')
M.run(1100)
check(pit.on, tag .. ": server echo of Pit highlights Pit")
-- player rows: class column from peer 2's peer_kit record
local r2 = w.rows[2]
check(r2["class"].last == "KNIGHT+" and r2.ready.last == "READY",
    string.format("%s: player row 2 shows READY / KNIGHT+ (edited kit) (%s / %s)", tag, tostring(r2.ready.last), tostring(r2["class"].last)))
-- START is greyed until everyone is READY; the reason names who is missing
check(truthy(w.start.disabled) and contains(Kit.reason_of(w.start) or "", "Mark yourself READY first"),
    tag .. ": START greyed while the host is not ready (" .. tostring(Kit.reason_of(w.start)) .. ")")
remove_(".control.outbox.jsonl")
M.click(w.start.button)
check(not contains(read(".control.outbox.jsonl") or "", '"verb":"start"'), tag .. ": a greyed START sends nothing")
write(".match.json", '{"state":"lobby","countdown_s":0,"arena":"Map_Arena_Pit","round":0,"best_of":3,"ready":[1]}\n')
M.run(600)
check(truthy(w.start.disabled) and contains(Kit.reason_of(w.start) or "", "Waiting for Mate to be READY"),
    tag .. ": START reason names the unready player (" .. tostring(Kit.reason_of(w.start)) .. ")")
write(".match.json", '{"state":"lobby","countdown_s":0,"arena":"Map_Arena_Pit","round":0,"best_of":3,"ready":[1,2]}\n')
M.run(600)
check(not truthy(w.start.disabled) and w.start.label.last == "START MATCH", tag .. ": everyone READY enables START")
-- START: never loads locally; resends once after 3 s
remove_(".control.outbox.jsonl")
local n_open = #M.openlevel
M.click(w.start.button)
M.run(10000)
outbox = read(".control.outbox.jsonl") or ""
check(T.count(outbox, '"verb":"start"') == 2, string.format("%s: START sent, then resent once (got %d)", tag, T.count(outbox, "start")))
check(#M.openlevel == n_open, tag .. ": START with no server state change loads NOTHING locally")
logs = logtext()
check(contains(logs, "server did not start the match"), tag .. ": START failure is reported")
-- server refusal is shown
append(".chat.log", "")
M.click(w.start.button)
append(".chat.log", '{"peer_id":0,"nick":"SERVER","text":"start blocked: 1 of 2 peers ready","time_utc_ms":1}\n')
M.run(1100)
check(contains(w.note.last or "", "start blocked: 1 of 2 peers ready"), tag .. ": server refusal shown (" .. tostring(w.note.last) .. ")")
-- LOADOUT button -> classes screen
M.click(w.loadout.button)
check(Kit.cur.screen == "classes", tag .. ": LOADOUT / CLASS opens the loadout screen")

-- ---- loadout --------------------------------------------------------------------
layout_checks(tag .. " loadout", cw, ch)
local b = Classes._build_ref()
local CW = b.w
local sel = Classes._sel()
check(sel["class"] == "man_at_arms", tag .. ": default class man-at-arms")
-- class cards
local function by_id(list, key)
    local m = {}
    for _, c in ipairs(T.list(list)) do m[c[key]] = c end
    return m
end
local card = by_id(CW.cards, "id")
M.click(card["duelist"].ref.button)
sel = Classes._sel()
check(sel["class"] == "duelist" and sel.l == "s_buckler3", tag .. ": DUELIST card selects the whole kit")
check(truthy(card["duelist"].ref.on) and not truthy(card["custom"].ref.on), tag .. ": DUELIST card highlighted")
check(card["none"].ref.shown, tag .. ": GAME GEAR card shown under FREE rules")
-- slot list -> picker
local slots = by_id(CW.slots, "slot")
check(T.len(slots) == 18, string.format("%s: 18 slot rows (15 layers in 4 regions + 2 hands + look) got %d", tag, T.len(slots)))
M.click(slots["R"].ref.button)
check(Classes._ui.slot == "R", tag .. ": clicking RIGHT HAND opens its picker")
-- paging: every candidate exactly once
local cands = T.list(Classes._candidates())
local seen = {}
local pages = 0
while true do
    pages = pages + 1
    for _, t in ipairs(T.list(CW.tiles)) do
        if truthy(t.shown) and t.value ~= nil then seen[#seen + 1] = t.value end
    end
    if truthy(CW.pager.next.disabled) then break end
    M.click(CW.pager.next.button)
    if pages > 40 then break end
end
check(T.eq(T.sorted(seen), T.sorted(cands)) and #seen == T.len(T.set(seen)),
    string.format("%s: picker pages cover all %d items exactly once (%d seen, %d pages)", tag, #cands, #seen, pages))
check(cands[1] == "", tag .. ": NONE is the first tile")
check(startswith(CW.pager.label.last, string.format("PAGE %d /", pages)), tag .. ": pager label (" .. tostring(CW.pager.label.last) .. ")")
M.click(CW.pager.prev.button)
check(Classes._ui.page == pages - 1, tag .. ": PREV goes back a page")
-- type + tier filters
local function shown_tiles(pred)
    return T.filter(T.list(CW.tiles), function(t) return truthy(t.shown) and pred(t) end)
end
M.click(CW.wtype.by_key["shield"].button)
local tiles = shown_tiles(function(t) return t.value ~= nil end)
check(#tiles > 0 and T.all(tiles, function(t) return t.value == "" or truthy(t.disabled) end) and
    T.all(tiles, function(t) return t.value == "" or contains(t.note.last or "", "LEFT HAND ONLY") end),
    tag .. ": shields greyed in the RIGHT hand")
M.click(CW.wtype.by_key["2h"].button)
M.click(CW.tier.by_key[4].button)
tiles = shown_tiles(function(t) return t.value ~= nil and t.value ~= "" end)
check(#tiles > 0 and T.all(tiles, function(t) return startswith(t.meta.last, "TIER IV") end), tag .. ": tier filter IV")
-- equip a two-hander -> left hand emptied
local gs
for _, t in ipairs(tiles) do if t.value == "w_greatsword" then gs = t; break end end
M.click(gs.button)
sel = Classes._sel()
check(sel.r == "w_greatsword" and sel.l == "", tag .. ": two-hander equipped, left hand emptied")
check(card["custom"].ref.on, tag .. ": edited kit lights the CUSTOM card")
-- left hand now: everything but NONE greyed
M.click(slots["L"].ref.button)
tiles = shown_tiles(function(t) return t.value ~= nil end)
local none_t = T.filter(tiles, function(t) return t.value == "" end)
check(#none_t > 0 and not truthy(none_t[1].disabled) and T.all(tiles, function(t) return t.value == "" or truthy(t.disabled) end),
    tag .. ": left hand greyed while a two-hander is in the right")
local dag
for _, t in ipairs(tiles) do if t.value ~= "" then dag = t; break end end
M.click(dag.button)
check(Classes._sel().l == "", tag .. ": clicking a greyed tile does not equip it")
check(contains(CW.status.last or "", "2H IN RIGHT HAND") or contains(CW.status.last or "", "2H"),
    tag .. ": greyed tile click explains why (" .. tostring(CW.status.last) .. ")")
-- CUSTOM budget: rules from the server
kit_rules(2, 30)
M.run(1100)
check(CW.rules.last == "RULES: CUSTOM 30", tag .. ": rules label (" .. tostring(CW.rules.last) .. ")")
check(card["knight"].ref.disabled, tag .. ": KNIGHT (42) greyed under CUSTOM 30")
check(not truthy(card["none"].ref.shown), tag .. ": GAME GEAR hidden outside FREE")
M.click(card["duelist"].ref.button)
check(CW.bar.label.last == "11 / 30 POINTS", tag .. ": budget bar (" .. tostring(CW.bar.label.last) .. ")")
M.click(slots["head"].ref.button)
-- tier IV filter reset on slot change; find an over-budget head? budget headroom is 19: none over.
kit_rules(2, 12)
M.run(1100)
M.click(CW.tier.by_key[4].button)    -- armet (6): 11 - 0 + 6 = 17 > 12
tiles = shown_tiles(function(t) return t.value == "h_armet" end)
check(#tiles > 0 and truthy(tiles[1].disabled) and contains(tiles[1].note.last, "OVER BUDGET"),
    tag .. ": over-budget item greyed (" .. tostring(#tiles > 0 and tiles[1].note.last or nil) .. ")")
check(CW.bar.label.last == "11 / 12 POINTS", tag .. ": bar follows the budget")
-- CLASSES ONLY: everything locked except cosmetics
kit_rules(1, 30)
M.run(1100)
tiles = shown_tiles(function(t) return t.value ~= nil end)
check(#tiles > 0 and T.all(tiles, function(t) return truthy(t.disabled) end), tag .. ": CLASSES ONLY greys every item")
check(card["custom"].ref.disabled, tag .. ": CLASSES ONLY greys the CUSTOM card")
M.click(slots["face"].ref.button)
tiles = shown_tiles(function(t) return t.value ~= nil end)
check(#tiles == 8 and not T.any(tiles, function(t) return truthy(t.disabled) end), tag .. ": cosmetics stay editable (8 faces)")
M.click(tiles[4].button)
check(Classes._sel().cos[1] == 3, tag .. ": face 3 picked by tile")
-- host rules view
check(CW.host_btn.shown, tag .. ": HOST RULES button for the host")
M.click(CW.host_btn.button)
check(Classes._ui.view == "rules" and not truthy(CW.tiles[1].shown), tag .. ": HOST RULES swaps the picker for rules")
M.click(CW.mode.by_key[2].button)
local rr = req_text()
check(contains(rr, '"mode":2'), tag .. ": CUSTOM mode chip writes the rules request (" .. strip(rr) .. ")")
M.click(CW.budget.by_key[45].button)
rr = req_text()
check(contains(rr, '"budget":45'), tag .. ": budget chip writes the rules request")
layout_checks(tag .. " loadout-rules", cw, ch)
M.click(CW.host_btn.button)
-- SAVE & BACK under FREE
kit_rules(0, 30)
M.run(1100)
M.click(card["brute"].ref.button)
M.click(CW.save.button)
local kit = own_kit()
check(kit["class"] == "brute" and kit.r == "w_axe2h" and (kit.cos or { 0 })[1] == 3,
    tag .. ": SAVE & BACK writes the kit slot (" .. T.repr(kit) .. ")")
check(Kit.cur.screen == "lobby", tag .. ": SAVE & BACK returns to the lobby")
check(contains(Kit.cur.w.loadout_sum.last or "", "BRUTE"), tag .. ": lobby shows the saved kit (" .. tostring(Kit.cur.w.loadout_sum.last) .. ")")
-- BACK drops edits
M.click(Kit.cur.w.loadout.button)
CW = Classes._build_ref().w
card = by_id(CW.cards, "id")
M.click(card["peasant"].ref.button)
M.click(CW.back.button)
check(Kit.cur.screen == "classes" and Kit.confirm_open(), tag .. ": BACK with unsaved edits asks first")
M.click(Kit.cur.cstrip.no.button)
check(Kit.cur.screen == "classes" and not Kit.confirm_open() and Classes._sel()["class"] == "peasant",
    tag .. ": KEEP EDITING keeps the edits")
M.click(CW.back.button)
M.click(Kit.cur.cstrip.yes.button)
kit = own_kit()
check(kit["class"] == "brute" and Kit.cur.screen == "lobby", tag .. ": DISCARD drops unsaved edits")

-- ---- joiner lobby view -----------------------------------------------------------
-- countdown from the server -> loads the SERVER's arena
write(".match.json", '{"state":"countdown","countdown_s":3,"arena":"Map_Arena_Cellar","round":0,"best_of":3,"ready":[1,2]}\n')
M.run(700)
check(#M.openlevel == 0, tag .. ": countdown does not travel before the Director had 1 s to ack")
local treq = T.json_decode(read(".travel_request.json") or "{}")
check(treq.want == "arena" and treq.arena == "Map_Arena_Cellar" and (treq.seq or 0) >= 1,
    tag .. ": countdown writes a travel request for the SERVER arena (" .. T.repr(treq) .. ")")
M.run(800)
local ol = T.list(M.openlevel)
check(#ol == 0, tag .. ": no Director ack -> nothing travels locally (the legacy shim is opt-in) (got " .. tostring(ol[#ol]) .. ")")
logs = logtext()
check(contains(logs, "loading server arena Map_Arena_Cellar (local pick"), tag .. ": load logs server arena + local pick")
check(contains(logs, "DIRECTOR MISSING and no legacy shim - NOT travelling"), tag .. ": the missing Director is logged")

-- ---- browser ----------------------------------------------------------------------
-- (the lobby above is still held: HOST / JOIN ask before replacing it, see lobby_t[8];
-- these checks are about the browser itself, so that session is forgotten)
HSMP_MENU_TEST.lobby.active = false
enter_screen("browser")
local bb = Browser.b
check(bb ~= nil and Kit.cur.screen == "browser", tag .. ": browser is a ui_kit build")
Browser.render()
check(bb.w.empty.last == "" or contains(bb.w.empty.last, "Loading"), tag .. ": browser loading state")
local function srv(i, name, players, mx, ping, pwd)
    return { host = "10.0.0." .. i, port = 7777, name = name, map = "Map_Arena_Pit",
             mode = "duel", players = players, max = mx, pwd = pwd or false, proto = 0,
             version = "1", region = "EU", ping = ping, source = "master", live = true }
end
local lst = { srv(1, "Alpha", 2, 2, 30), srv(2, "Bravo", 0, 8, 80), srv(3, "Charlie", 3, 8, 120, true),
              srv(4, "Delta", 1, 8, 40) }
for i = 0, 13 do lst[#lst + 1] = srv(10 + i, string.format("Filler %02d", i), 1, 8, 200) end
Browser._accept_result({ status = "ok", servers = lst, done = true })
Browser.render()
layout_checks(tag .. " browser", cw, ch)
local W = bb.w
local rows_shown = #T.filter(T.list(W.rows), function(r) return truthy(r.btn.shown) end)
check(rows_shown == 12, string.format("%s: 12 rows on page 1 (%d)", tag, rows_shown))
check(W.pager.label.last == "PAGE 1 / 2", tag .. ": browser pager (" .. tostring(W.pager.label.last) .. ")")
M.click(W.f_full.button)
check(truthy(W.f_full.on) and T.all(T.list(Browser.view), function(s) return s["name"] ~= "Alpha" end),
    tag .. ": HIDE FULL chip on and Alpha hidden")
local function ping_on()
    local o = {}
    for _, k in ipairs({ 0, 50, 100, 150 }) do if truthy(W.f_ping.by_key[k].on) then o[#o + 1] = k end end
    return o
end
M.click(W.f_ping.by_key[50].button)
on = ping_on()
check(T.eq(on, { 50 }), tag .. ": ping chip group has exactly one active (" .. T.repr(on) .. ")")
local names = T.sorted(T.map(T.list(Browser.view), function(s) return s["name"] end))
check(T.eq(names, { "Delta" }), tag .. ": HIDE FULL + <50 leaves Delta (" .. T.repr(names) .. ")")
M.click(W.f_ping.by_key[150].button)
on = ping_on()
check(T.eq(on, { 150 }), tag .. ": switching ping chip leaves one active (" .. T.repr(on) .. ")")
M.click(W.f_clear.button)
check(not truthy(W.f_full.on) and truthy(W.f_ping.by_key[0].on) and #T.list(Browser.view) == 18, tag .. ": CLEAR resets chips")
-- sort by name ascending
M.click(W.heads[2].button)
local first = T.list(Browser.view)[1]["name"]
check(first == "Alpha" and truthy(W.heads[2].on), tag .. ": sort by SERVER NAME (" .. tostring(first) .. ")")
-- single click selects; slow second click does not join; fast double click joins
local n_exec = #M.execs
local function joined_since()
    local o = {}
    for i = n_exec + 1, #M.execs do
        local e = M.execs[i]
        if contains(e, "hsmp-sidecar") then o[#o + 1] = e end
    end
    return o
end
local r1 = W.rows[1]
M.click(r1.btn.button)
check(Browser.sel == "10.0.0.1:7777" and truthy(r1.btn.on), tag .. ": single click selects + highlights the row")
M.run(600)
M.click(r1.btn.button)
local joined = joined_since()
check(#joined == 0, tag .. ": two clicks 0.7 s apart do not join")
-- hover highlight
local r3 = W.rows[3].btn
M.set(r3.button, "hovered", true)
M.run(120)
check(r3.painted == "h", tag .. ": row hover highlight (" .. tostring(r3.painted) .. ")")
M.set(r3.button, "hovered", false)
M.run(120)
-- double click
local r2b = W.rows[2].btn
M.set(r2b.button, "pressed", true); M.run(70); M.set(r2b.button, "pressed", false); M.run(100)
M.set(r2b.button, "pressed", true); M.run(70); M.set(r2b.button, "pressed", false); M.run(70)
joined = joined_since()
check(#joined == 1 and contains(joined[1], "10.0.0.2:7777"), tag .. ": double-click joins the clicked server")
M.run(1200)
-- empty state
enter_screen("browser")
Browser._accept_result({ status = "ok", servers = {}, done = true })
Browser.render()
check(Browser.b.w.empty.last == "No servers found. REFRESH or DIRECT CONNECT.",
    tag .. ": empty state text (" .. tostring(Browser.b.w.empty.last) .. ")")
exit_screen()

-- ---- settings + character ------------------------------------------------------------
enter_screen("settings")
layout_checks(tag .. " settings", cw, ch)
local sw = Kit.cur.w
M.click(sw.hz.by_key[90].button)
check(not contains(read(".settings.json") or "", '"send_hz":90') and truthy(sw.hz.by_key[90].on),
    tag .. ": SEND RATE chip is staged, not saved yet")
M.click(sw.save.button)
check(contains(read(".settings.json") or "", '"send_hz":90') and Kit.cur == nil, tag .. ": SAVE & BACK saves SEND RATE and leaves")
enter_screen("character")
layout_checks(tag .. " character", cw, ch)
M.click(Kit.cur.w.str.by_key[7].button)
check(not contains(read(".my_character.json") or "", '"strength":7'), tag .. ": stat chip is staged")
M.click(Kit.cur.w.save.button)
check(contains(read(".my_character.json") or "", '"strength":7'), tag .. ": SAVE & PUBLISH saves the stats")
exit_screen()

-- ---- world guard ------------------------------------------------------------------
enter_screen("lobby")
check(Kit.cur ~= nil, tag .. ": lobby build before the world change")
enter_screen("classes")
M.premap()
M.kill_all()
check(Kit.cur == nil and Classes._build_ref() == nil and Browser.b == nil,
    tag .. ": world change drops every widget ref")
M.run(3000)
local dead = T.list(M.dead_touch)
check(#dead == 0, tag .. ": no freed widget touched after the world change (" .. T.repr({ dead[1], dead[2], dead[3], dead[4], dead[5] }) .. ")")
local errs = T.filter(T.map(M.logs, tostring), function(l) return contains(l, "ERR") or contains(l:lower(), "error") end)
check(#errs == 0, tag .. ": no loop/tick errors (" .. T.repr({ errs[1], errs[2], errs[3] }) .. ")")
end

-- ============================================================================================
-- Travel and commands: the menu only REQUESTS travel, commands show pending -> result,
-- hsmp_cfg paths, hsmp_log events, autotest map cycle, no fighting the server.
-- ============================================================================================

local SIDECAR_2 = '{"status":"connected","peer_id":1,"is_admin":true,"peers":[{"id":1,"nick":"Willie"},{"id":2,"nick":"Mate"}]}\n'

-- json.dumps(..., separators=(",", ":")) of the match file
local function match_json(state, arena, best_of, ready)
    ready = ready or { 2 }
    local r = {}
    for i, v in ipairs(ready) do r[i] = tostring(v) end
    return string.format('{"state":"%s","countdown_s":0,"arena":"%s","round":0,"best_of":%d,"ready":[%s]}\n',
        state or "lobby", arena or "Map_Arena_Yard", best_of or 3, table.concat(r, ","))
end

-- Python json.dumps() default separators (", " / ": ") for flat objects; keys in order.
local function pyjson(kv)
    local parts = {}
    for i = 1, #kv, 2 do
        local k, v = kv[i], kv[i + 1]
        local s
        if type(v) == "string" then s = string.format("%q", v)
        elseif v == nil then s = "null"
        else s = tostring(v) end
        parts[#parts + 1] = string.format('"%s": %s', k, s)
    end
    return "{" .. table.concat(parts, ", ") .. "}"
end

local function host_lobby(arena)
    write(".sidecar.json", SIDECAR_2)
    write(".match.json", match_json(nil, arena))
    local rib = T.filter(M.live(), function(w) return startswith(tostring(w.name), "HSMPBtn_top1") end)
    M.click(rib[1].obj)
    M.run(1600)
end

-- CLOSE LOBBY / LEAVE: confirm when asked (host with other players), then give
-- the leave request its 1 s before the pidfile kills.
local function cancel_lobby()
    local K = package.loaded["ui_kit"]
    M.click(K.cur.w.cancel.button)
    if K.cur and K.confirm_open() then M.click(K.cur.cstrip.yes.button) end
    M.run(2000)   -- LEAVE_WAIT_S 1.5 s
end

local function outbox() return read(".control.outbox.jsonl") or "" end
-- G2S control messages are tables (the mock writes sorted-key JSON).
-- true when one message has every field of `want` (true = "present").
local function ob_has(want)
    for line in outbox():gmatch("[^\n]+") do
        local t = T.json_decode(line)
        if type(t) == "table" then
            local ok = true
            for k, v in pairs(want) do
                if v == true then ok = ok and t[k] ~= nil else ok = ok and t[k] == v end
            end
            if ok then return true end
        end
    end
    return false
end
local function clear_outbox() remove_(".control.outbox.jsonl") end

local function run_until(pred, max_ms, step)
    step = step or 10
    local t = 0
    while t < max_ms do
        if pred() then return true end
        M.run(step)
        t = t + step
    end
    return pred()
end

local function kit() return package.loaded["ui_kit"] end
local function req_written() return read(".travel_request.json") ~= nil end

local director = {}
-- 1. ack file within 1 s: no legacy travel, lobby left
director[1] = function(tag)
    boot(1920, 1080, 1.0)
    host_lobby()
    write(".match.json", match_json("countdown", "Map_Arena_Pit", nil, { 1, 2 }))
    local ok = run_until(req_written, 700)
    local req = T.json_decode(read(".travel_request.json") or "{}")
    check(ok and req.want == "arena" and req.arena == "Map_Arena_Pit" and req.reason == "match_countdown"
        and req.from == "HSMPMenu", tag .. ": request file has want/arena/reason/seq (" .. T.repr(req) .. ")")
    write(".travel_ack.json", pyjson({ "seq", req.seq, "status", "accepted", "arena", "Map_Arena_Pit" }))
    M.run(3000)
    check(#M.openlevel == 0, tag .. ": acked request -> the menu never travels itself")
    check(not contains(logtext(), "DIRECTOR MISSING"), tag .. ": acked request -> no legacy log")
    check(kit().cur == nil, tag .. ": accepted travel leaves the lobby screen")
    -- only ONE request for one countdown (the poll keeps running)
    local req2 = T.json_decode(read(".travel_request.json") or "{}")
    check(req2.seq == req.seq, string.format("%s: one request per match start (%s vs %s)", tag, tostring(req2.seq), tostring(req.seq)))
end
-- 2. live Director (heartbeat) without an ack: waits, then the shim after 5 s
director[2] = function(tag)
    boot(1920, 1080, 1.0, { HSMP_LEGACY_TRAVEL = "1" })
    host_lobby()
    write(".travel_ack.json", pyjson({ "seq", 999, "status", "accepted" }))     -- stale, other seq
    write(".director.json", pyjson({ "v", 1, "state", "Menu", "hb", os.time() }))
    write(".match.json", match_json("live", "Map_Arena_Slums", nil, { 1, 2 }))
    M.run(2500)
    check(#M.openlevel == 0, tag .. ": a live Director gets more than 1 s (stale ack ignored)")
    check(contains(logtext(), "Director alive but no ack"), tag .. ": waiting on a live Director is logged")
    M.run(4500)
    local ol = T.list(M.openlevel)
    check(T.eq(ol, { "Map_Arena_Slums" }), tag .. ": no ack within 5 s even from a live Director -> shim (" .. T.repr(ol) .. ")")
end
-- 3. global HSMP_DIRECTOR in the same Lua state
director[3] = function(tag)
    boot(1920, 1080, 1.0, nil,
        'HSMP_DIRECTOR = { reqs = {}, request_travel = function(r) table.insert(HSMP_DIRECTOR.reqs, r); return true end }')
    host_lobby()
    write(".match.json", match_json("countdown", "Map_Arena_LordsHall", nil, { 1, 2 }))
    M.run(3000)
    local reqs = T.list(HSMP_DIRECTOR.reqs)
    check(#reqs == 1 and reqs[1].arena == "Map_Arena_LordsHall" and reqs[1].want == "arena",
        string.format("%s: HSMP_DIRECTOR.request_travel gets the request (%d)", tag, #reqs))
    check(#M.openlevel == 0, tag .. ": global Director ack -> no local travel")
    check(read(".travel_request.json") ~= nil, tag .. ": the request file is written even with the global")
end
-- 4. refused ack: note shows it, nothing travels, retried after 3 s
director[4] = function(tag)
    boot(1920, 1080, 1.0)
    host_lobby()
    write(".match.json", match_json("countdown", "Map_Arena_Pit", nil, { 1, 2 }))
    run_until(req_written, 700)
    local req = T.json_decode(read(".travel_request.json"))
    write(".travel_ack.json", pyjson({ "seq", req.seq, "status", "refused", "reason", "not in a match" }))
    M.run(1300)
    local note = kit().cur and kit().cur.w.note.last or ""
    check(contains(note, "TRAVEL REFUSED") and contains(note, "not in a match"), tag .. ": refused travel shown (" .. tostring(note) .. ")")
    check(#M.openlevel == 0, tag .. ": refused travel -> no local travel")
    write(".travel_ack.json", pyjson({ "seq", req.seq + 1, "status", "accepted" }))
    M.run(3500)
    local req2 = T.json_decode(read(".travel_request.json"))
    check(req2.seq == req.seq + 1, string.format("%s: a refused request is retried once after 3 s (seq %s)", tag, tostring(req2.seq)))
    check(#M.openlevel == 0, tag .. ": retry acked -> still no local travel")
end
-- 5. shim removed (legacy_travel.lua deleted): nothing ever travels locally
director[5] = function(tag)
    boot(1920, 1080, 1.0, { HSMP_LEGACY_TRAVEL = "1" },
        'local _df = dofile; dofile = function(p) if tostring(p):find("legacy_travel") then error("removed") end return _df(p) end '
        .. 'package.preload["legacy_travel"] = function() error("removed") end')
    host_lobby()
    write(".match.json", match_json("countdown", "Map_Arena_Pit", nil, { 1, 2 }))
    M.run(6000)
    check(#M.openlevel == 0, tag .. ": without the shim the menu never travels")
    check(contains(logtext(), "DIRECTOR MISSING and no legacy shim - NOT travelling"), tag .. ": removed shim is logged")
    local errs = T.filter(T.map(M.logs, tostring), function(l) return contains(l, "ERR") end)
    check(#errs == 0, tag .. ": no errors with the shim removed (" .. T.repr({ errs[1], errs[2] }) .. ")")
end
-- 6. no arena from the server: never falls back to the local pick
director[6] = function(tag)
    boot(1920, 1080, 1.0)
    host_lobby()
    write(".match.json", '{"state":"countdown","countdown_s":3,"arena":"","round":0,"best_of":3,"ready":[1,2]}\n')
    M.run(3000)
    check(#M.openlevel == 0 and read(".travel_request.json") == nil, tag .. ": no server arena -> no request, no local fallback")
    check(contains(logtext(), "server reported no arena - not travelling"), tag .. ": missing arena logged")
end
-- 7. world change before the ack deadline: the shim does not fire (and touches nothing freed)
director[7] = function(tag)
    boot(1920, 1080, 1.0, { HSMP_LEGACY_TRAVEL = "1" })
    host_lobby()
    write(".match.json", match_json("countdown", "Map_Arena_Pit", nil, { 1, 2 }))
    run_until(req_written, 700)
    M.premap(); M.kill_all()
    M.run(3000)
    check(#M.openlevel == 0, tag .. ": world changed before the deadline -> no legacy travel")
    check(#M.dead_touch == 0, tag .. ": no freed object touched (" .. T.repr({ M.dead_touch[1], M.dead_touch[2], M.dead_touch[3] }) .. ")")
end
-- 8. DEFAULT (no HSMP_LEGACY_TRAVEL): the shim is not even loaded; an unacked request is refused + retried
director[8] = function(tag)
    boot(1920, 1080, 1.0)
    check(package.loaded["legacy_travel"] == nil, tag .. ": legacy_travel.lua is not loaded by default")
    check(not contains(logtext(), "LEGACY TRAVEL SHIM ENABLED"), tag .. ": no shim banner by default")
    host_lobby()
    write(".match.json", match_json("countdown", "Map_Arena_Pit", nil, { 1, 2 }))
    M.run(1800)
    check(#M.openlevel == 0, tag .. ": no ack, no shim -> nothing travels locally")
    local note = kit().cur and kit().cur.w.note.last or ""
    check(contains(note, "TRAVEL REFUSED: no director"), tag .. ": the lobby shows the refusal (" .. tostring(note) .. ")")
    local seq1 = T.json_decode(read(".travel_request.json")).seq
    write(".travel_ack.json", pyjson({ "seq", seq1 + 1, "status", "accepted", "arena", "Map_Arena_Pit" }))
    M.run(3500)
    check(T.json_decode(read(".travel_request.json")).seq == seq1 + 1, tag .. ": the request is retried for a late Director")
    check(not contains(logtext(), "LEGACY TRAVEL USED") and #M.openlevel == 0, tag .. ": still no local travel")
end
-- 9. HSMP_LEGACY_TRAVEL=1: loud banner at load and loud line on every use
director[9] = function(tag)
    boot(1920, 1080, 1.0, { HSMP_LEGACY_TRAVEL = "1" })
    check(package.loaded["legacy_travel"] ~= nil, tag .. ": HSMP_LEGACY_TRAVEL=1 loads the shim")
    check(contains(logtext(), "!!! LEGACY TRAVEL SHIM ENABLED (HSMP_LEGACY_TRAVEL=1)"), tag .. ": the armed shim is announced loudly")
    host_lobby()
    write(".match.json", match_json("countdown", "Map_Arena_Pit", nil, { 1, 2 }))
    M.run(1800)
    check(T.eq(T.list(M.openlevel), { "Map_Arena_Pit" }), tag .. ": opt-in shim travels when the Director does not ack")
    check(contains(logtext(), "!!! LEGACY TRAVEL USED (HSMP_LEGACY_TRAVEL=1): #1 want=arena arena=Map_Arena_Pit"),
        tag .. ": every shim use is logged loudly")
end

local cmd = {}
cmd[1] = function(tag)
    boot(1920, 1080, 1.0)
    host_lobby()
    local Kit = kit()
    local Cmd = package.loaded["commands"]
    local w = Kit.cur.w
    -- API shape
    local cid = cmd_send("abort", nil)
    local st = cmd_status(cid)
    check(st ~= nil and st.state == "pending" and st.kind == "abort", tag .. ": cmd_send -> id, cmd_status pending")
    check(ob_has({ verb = "abort", cmd_id = cid }), tag .. ": legacy line carries the cmd_id")
    M.run(600)
    check(cmd_status(cid).state == "accepted", tag .. ": abort accepted when the server is in the lobby")
    check(cmd_status(123) == nil, tag .. ": unknown id -> nil")

    -- best_of: pending chip -> accepted from .match.json best_of
    clear_outbox()
    M.click(w.modes.by_key[5].button)
    check(contains(outbox(), '"verb":"set_best_of:5"'), tag .. ": ROUNDS chip sends best_of 5")
    M.run(500)
    local mid = Cmd.last("best_of")
    check(cmd_status(mid).state == "pending" and w.modes.by_key[5].pending, tag .. ": best_of pending + chip pending")
    check(contains(w.note.last or "", "ROUNDS BEST OF 5: WAITING FOR SERVER"), tag .. ": pending note (" .. tostring(w.note.last) .. ")")
    write(".match.json", match_json(nil, nil, 5))
    M.run(1100)
    check(cmd_status(mid).state == "accepted" and truthy(w.modes.by_key[5].on)
        and not truthy(w.modes.by_key[5].pending), tag .. ": best_of accepted -> chip selected")
    check(contains(w.note.last or "", "ACCEPTED"), tag .. ": accepted note (" .. tostring(w.note.last) .. ")")
    -- best_of with no answer -> refused on timeout, after one resend
    clear_outbox()
    M.click(w.modes.by_key[3].button)
    M.run(7000)
    local bid = Cmd.last("best_of")
    st = cmd_status(bid)
    check(st.state == "refused" and contains(st.reason, "no answer") and T.count(outbox(), "set_best_of:3") == 2,
        string.format("%s: unanswered best_of -> resent once, then refused (timeout) (%s, %d)", tag, tostring(st.state), T.count(outbox(), "set_best_of:3")))
    check(contains(w.note.last or "", "ROUNDS REFUSED - no answer from the server"), tag .. ": timeout note (" .. tostring(w.note.last) .. ")")

    -- ready: pending label, accepted when the server lists us
    clear_outbox()
    M.click(w.ready.button)
    check(contains(outbox(), '"verb":"ready"'), tag .. ": READY sends ready")
    M.run(500)
    check(w.ready.label.last == "SENDING READY..." and w.ready.pending, tag .. ": READY pending (" .. tostring(w.ready.label.last) .. ")")
    write(".match.json", match_json(nil, nil, 5, { 1, 2 }))
    M.run(1100)
    local rid = Cmd.last("ready")
    check(cmd_status(rid).state == "accepted" and truthy(w.ready.on) and startswith(w.ready.label.last, "READY"),
        tag .. ": READY accepted (" .. tostring(w.ready.label.last) .. ")")
    M.click(w.ready.button)
    check(contains(outbox(), '"verb":"unready"'), tag .. ": second click sends unready")

    -- pick_arena refused by a SERVER chat reply
    write(".chat.log", "")
    M.click(w.maps.by_key["Map_Arena_Slums"].button)
    append(".chat.log", '{"peer_id":0,"nick":"SERVER","text":"unknown arena: Map_Arena_Slums","time_utc_ms":1}\n')
    M.run(1100)
    local pid_ = Cmd.last("pick_arena")
    st = cmd_status(pid_)
    check(st.state == "refused" and contains(st.reason, "unknown arena"), tag .. ": refusal from the SERVER reply (" .. tostring(st.state) .. ")")
    check(contains(w.note.last or "", "ARENA REFUSED - unknown arena: Map_Arena_Slums"), tag .. ": refusal note (" .. tostring(w.note.last) .. ")")
    check(not truthy(w.maps.by_key["Map_Arena_Slums"].pending), tag .. ": refused pick no longer pending")
    -- v5 results file maps 1:1 (and wins over inference)
    local sid = cmd_send("start", nil)
    append(".cmd_results.jsonl", pyjson({ "cmd_id", sid, "ok", false, "reason_code", "not_ready", "reason_text", "1 of 2 ready" }) .. "\n")
    M.run(600)
    st = cmd_status(sid)
    check(st.state == "refused" and st.reason == "1 of 2 ready" and st.source == "server",
        string.format("%s: .cmd_results.jsonl result maps 1:1 (%s, %s, %s)", tag, tostring(st.state), tostring(st.reason), tostring(st.source)))
    local sid2 = cmd_send("best_of", { n = 7 })
    append(".cmd_results.jsonl", pyjson({ "cmd_id", sid2, "ok", true }) .. "\n")
    M.run(600)
    check(cmd_status(sid2).state == "accepted" and cmd_status(sid2).source == "server", tag .. ": ok:true result -> accepted")
end
-- hsmp_net backend (v5): no outbox line, result from net.cmd_result
cmd[2] = function(tag)
    boot(1920, 1080, 1.0, nil,
        'NETSENT = {}; NETRES = {}; package.preload["hsmp_net"] = function() return { '
        .. 'cmd_send = function(k, a, id) table.insert(NETSENT, { k = k, id = id }) end, '
        .. 'cmd_result = function(id) return NETRES[id] end } end')
    host_lobby()
    clear_outbox()
    local cid = cmd_send("start", nil)
    local sent = T.list(NETSENT)
    check(#sent > 0 and sent[#sent].k == "start" and sent[#sent].id == cid and not contains(outbox(), "start"),
        tag .. ": v5 backend sends through hsmp_net only")
    check(cmd_status(cid).backend == "net", tag .. ": backend recorded as net")
    M.run(4000)
    check(#T.filter(T.list(NETSENT), function(s) return s.id == cid end) == 1, tag .. ": v5 backend never resends (the sidecar does)")
    NETRES[cid] = { ok = false, reason_code = "not_ready", reason_text = "waiting on Mate" }
    M.run(600)
    local st = cmd_status(cid)
    check(st.state == "refused" and st.reason == "waiting on Mate", tag .. ": net result maps 1:1 (" .. tostring(st.state) .. ")")
end

-- Lead bug: a stale local pick must never be re-sent over a newer server arena.
local function test_no_fighting_the_server(tag)
    boot(1920, 1080, 1.0)
    host_lobby("Map_Arena_Alley")          -- boot pick == server arena: nothing to send
    check(not contains(outbox(), '"set_map"'), tag .. ": no pick sent when the server already has the boot arena")
    local Kit = kit()
    local Cmd = package.loaded["commands"]
    local w = Kit.cur.w
    -- A: pending pick, then the server's arena changes to a THIRD arena (RCON MAP / another admin)
    M.click(w.maps.by_key["Map_Arena_Yard"].button)
    local yid = Cmd.last("pick_arena")
    M.run(500)
    write(".match.json", match_json(nil, "Map_Arena_Pit"))
    M.run(600)
    local st = cmd_status(yid)
    check(st.state == "superseded" and contains(st.reason or "", "Map_Arena_Pit"),
        string.format("%s: external arena change drops the pending pick (%s: %s)", tag, tostring(st.state), tostring(st.reason)))
    check(contains(w.note.last or "", "ARENA YARD DROPPED - server arena changed to Map_Arena_Pit"),
        tag .. ": the dropped pick is explained (" .. tostring(w.note.last) .. ")")
    clear_outbox()
    M.run(20000)
    check(not contains(outbox(), '"set_map"'), tag .. ": the stale pick is never re-sent (" .. outbox():sub(1, 80) .. ")")
    check(truthy(w.maps.by_key["Map_Arena_Pit"].on) and not truthy(w.maps.by_key["Map_Arena_Yard"].pending),
        tag .. ": the menu adopts the server's arena")
    -- B: accepted pick, then an external change: no re-send at all
    M.click(w.maps.by_key["Map_Arena_Cellar"].button)
    write(".match.json", match_json(nil, "Map_Arena_Cellar"))
    M.run(600)
    check(cmd_status(Cmd.last("pick_arena")).state == "accepted", tag .. ": pick accepted on echo")
    clear_outbox()
    write(".match.json", match_json(nil, "Map_Arena_EastTower"))
    M.run(20000)
    check(outbox() == "", tag .. ": after an accepted pick, an external change is never fought (" .. outbox():sub(1, 80) .. ")")
    check(w.maps.by_key["Map_Arena_EastTower"].on, tag .. ": server arena shown after the external change")
    -- C: three quick picks: only the last one is pending/resent (DoD-4 map_change)
    clear_outbox()
    for _, a in ipairs({ "Map_Arena_Alley", "Map_Arena_Pit", "Map_Arena_Yard" }) do
        M.click(w.maps.by_key[a].button)
    end
    M.run(3500)
    local lines = T.filter(T.lines(outbox()), function(l) return contains(l, "set_map") end)
    local resent = {}
    for i = 4, #lines do resent[#resent + 1] = lines[i] end
    check(#lines >= 4 and T.all(resent, function(l) return contains(l, "Map_Arena_Yard") end),
        tag .. ": after 3 quick picks only the newest is resent (" .. T.repr(lines) .. ")")
    local order = T.list(Cmd.order)
    local states = {}
    for i = math.max(1, #order - 2), #order do states[#states + 1] = cmd_status(order[i]).state end
    check(T.eq(states, { "superseded", "superseded", "pending" }), tag .. ": older picks superseded locally")
end

local function test_autotest_maps(tag)
    boot(1920, 1080, 1.0, { HSMP_LEGACY_TRAVEL = "1", HSMP_AUTOTEST = "1", HSMP_AUTOTEST_MAPS = "Map_Arena_Yard,Map_Arena_Pit", HSMP_AUTOTEST_WAIT_MS = "1000" })
    -- injection happened in boot before the sidecar/server files exist: write them now
    write(".sidecar.json", '{"status":"connected","peer_id":1,"is_admin":true,"peers":[{"id":1,"nick":"Willie"}]}\n')
    write(".match.json", match_json(nil, "Map_Arena_Alley", nil, {}))
    M.run(2500)
    local ob = outbox()
    check(contains(ob, '"kind":"set_map","map":"Map_Arena_Yard"') and not contains(ob, '"verb":"start"'),
        tag .. ": match 1 picks Yard through cmd_send, START waits for the answer")
    write(".match.json", match_json(nil, "Map_Arena_Yard", nil, {}))
    M.run(1100)
    check(ob_has({ verb = "start", cmd_id = true }), tag .. ": START sent as a command after the pick was accepted")
    check(#M.openlevel == 0, tag .. ": nothing travels on START")
    write(".match.json", match_json("countdown", "Map_Arena_Yard", nil, { 1 }))
    M.run(2000)
    check(T.eq(T.list(M.openlevel), { "Map_Arena_Yard" }), tag .. ": match 1 travels to the server arena via the shim")
    -- match over: back to the menu (HSMPMatch drops the return flag), server back in the lobby
    write(".match.json", match_json(nil, "Map_Arena_Yard", nil, {}))
    write(".return_to_lobby.flag", "1")
    M.premap(); M.kill_all()
    M.menu = nil
    do   -- a new Startup menu in the new world
        local new_obj = M.new_obj
        M.objs = {}
        M.canvas = new_obj("CanvasPanel", "CanvasPanel_5")
        for i, n in ipairs({ "Button", "Button_0", "Button_1", "Button_2", "Button_3", "Button_4" }) do
            local b = new_obj("Button", n)
            local slot = new_obj("CanvasPanelSlot", "slot_" .. n)
            rawset(slot, "x", -700); rawset(slot, "y", -360 + i * 110); rawset(slot, "w", 469); rawset(slot, "h", 120)
            b.__props.Slot = slot
            table.insert(rawget(b, "__children"), new_obj("TextBlock", "txt_" .. n))
            table.insert(rawget(M.canvas, "__children"), b)
        end
        M.menu = new_obj("UI_Startup_Menu_C", "UI_Startup_Menu_C_1")
        M.menu.__props.WidgetTree = new_obj("WidgetTree", "wt")
        M.menu.__props.WidgetTree.__props.RootWidget = M.canvas
        FindAllOf = function(c) if c == "UI_Startup_Menu_C" then return { M.menu } end return nil end
    end
    clear_outbox()
    M.on_new(M.menu)
    M.run(3500)
    check(contains(outbox(), '"kind":"set_map","map":"Map_Arena_Pit"'), tag .. ": match 2 cycles to Pit (" .. outbox():sub(1, 90) .. ")")
    write(".match.json", match_json(nil, "Map_Arena_Pit", nil, {}))
    M.run(1100)
    check(contains(outbox(), '"verb":"start"'), tag .. ": match 2 START after the Pit pick was accepted")
    check(contains(logtext(), "AUTOTEST: armed (Map_Arena_Yard,Map_Arena_Pit)"), tag .. ": armed log")
end

-- ---- hsmp_cfg / hsmp_log (the REAL shared libs; docs/development/testing.md) ---------------
-- Events written by shared/hsmp_log.lua to <state>/hsmp_events.jsonl.
local function events()
    local out = {}
    for _, l in ipairs(T.lines(read("hsmp_events.jsonl") or "")) do
        if strip(l) ~= "" then out[#out + 1] = T.json_decode(l) end
    end
    return out
end
local function evs(name) return T.filter(events(), function(e) return e.ev == name end) end
local function execs() return table.concat(T.map(M.execs, tostring), "\n") end

-- A copy of HSMPMenu/Scripts with NO shared/ directory anywhere near it: what a
-- deploy without the shared libs looks like (require and the ../../shared
-- lookup both find nothing).
local function isolated_scripts()
    local base = T.tmpdir("hsmp_iso_")
    local dir = base .. "/mods/HSMPMenu/Scripts"
    os.execute('mkdir "' .. dir:gsub("/", "\\") .. '" >nul 2>nul')   -- the real os.execute (mock not installed yet)
    for _, p in ipairs(T.glob(SCRIPTS, "*.lua")) do
        T.write(dir .. "/" .. p:match("([^/\\]+)$"), T.read(p))
    end
    -- The IPC facade is mandatory (the deploy copies it like every
    -- shared lib); the isolated copy hides hsmp_cfg / hsmp_log only.
    for _, n in ipairs({ "hsmp_ipc", "hsmp_ipc_schema" }) do
        T.write(dir .. "/" .. n .. ".lua", T.read(T.path("mods/shared/" .. n .. ".lua")))
    end
    return dir, base
end

local cfg = {}
cfg[1] = function(tag)
    T.write(opts.libdir .. "/hsmp.cfg", '# test cfg\nbin_dir = "C:\\HSMP\\bin\\"\nmaster_url = http://master.example:9000   ; comment\n')
    boot(1920, 1080, 1.0, { HSMP_CFG = opts.libdir .. "/hsmp.cfg", HSMP_LEGACY_TRAVEL = "1" })
    check(contains(logtext(), "bin_dir=C:/HSMP/bin (hsmp_cfg)"), tag .. ": the real shared/hsmp_cfg.lua is used")
    M.execs = {}
    host_lobby()
    local ex = execs()
    check(contains(ex, [[C:\HSMP\bin\hsmp-server.exe]]) and contains(ex, [[C:\HSMP\bin\hsmp-sidecar.exe]]),
        tag .. ": HOST launches the binaries from hsmp_cfg bin_dir")
    check(contains(ex, "HSMP_MASTER_URL=http://master.example:9000"), tag .. ": master_url from hsmp_cfg")
    check(not contains(ex, [[D:\dev\HSMP]]), tag .. ": no D:/ path used when hsmp_cfg is present")
    local open = evs("_open")
    check(#open == 1 and open[1].mod == "HSMPMenu" and open[1].state_dir == sd, tag .. ": hsmp_log.init{mod=HSMPMenu, state_dir} (" .. T.repr(open[1]) .. ")")
    local names = T.map(events(), function(e) return e.ev end)
    check(T.set(names)["lobby_ready"], tag .. ": lobby_ready emitted (" .. T.repr(names) .. ")")
    local lr = evs("lobby_ready")[1] or {}
    check(lr.role == "host" and lr.peer_id == 1 and lr.arena == "Map_Arena_Yard" and lr.inst == "0", tag .. ": lobby_ready fields")
    check(#evs("lobby_ready") == 1, tag .. ": lobby_ready once per session")
    local sent = evs("cmd_sent")
    check(#sent > 0 and sent[1].cmd == "pick_arena" and sent[1].arg == "Map_Arena_Alley" and sent[1].cmd_id,
        tag .. ": cmd_sent{cmd, cmd_id, arg} for the boot pick")
    write(".match.json", match_json(nil, "Map_Arena_Yard", nil, { 1, 2 }))
    M.run(600)
    M.click(kit().cur.w.start.button)
    write(".match.json", match_json("countdown", "Map_Arena_Yard", nil, { 1, 2 }))
    M.run(2000)
    local res = evs("cmd_result")
    local start_res = T.filter(res, function(f) return f.cmd == "start" end)
    check(#start_res > 0 and start_res[1].ok == true and start_res[1].source == "inferred" and start_res[1].ms >= 0,
        tag .. ": cmd_result{cmd, cmd_id, ok} for START (+source, ms)")
    local ids = T.map(res, function(f) return tostring(f.cmd_id) end)
    check(#ids == T.len(T.set(ids)), tag .. ": exactly one cmd_result per cmd_id")
    local sent_ids = T.set(T.map(evs("cmd_sent"), function(f) return tostring(f.cmd_id) end))
    check(T.all(ids, function(i) return sent_ids[i] end), tag .. ": every cmd_result has its cmd_sent (gate DoD-10)")
    check(#evs("cmd_sent") == T.len(sent_ids), tag .. ": one cmd_sent per cmd_id (resends are not events)")
    local bad = T.filter(events(), function(e) return e._missing ~= nil or e.ev == "_bad_event" end)
    check(#bad == 0, tag .. ": every event is in the frozen vocabulary with its required fields (" .. T.repr(bad[1]) .. ")")
    local tr = evs("travel")
    check(#tr > 0 and tr[1].to == "Map_Arena_Yard" and tr[1].by == "menu_legacy", tag .. ": legacy travel emits travel{from,to,by}")
end
-- env still overrides hsmp_cfg (per key; applied by hsmp_cfg itself)
cfg[2] = function(tag)
    T.write(opts.libdir .. "/hsmp.cfg", 'bin_dir = C:/HSMP/bin\nmaster_url = http://master.example:9000\n')
    boot(1920, 1080, 1.0, { HSMP_CFG = opts.libdir .. "/hsmp.cfg", HSMP_SERVER_EXE = "E:/x/srv.exe", HSMP_MASTER_URL = "http://env:1" })
    M.execs = {}
    host_lobby()
    local ex = execs()
    check(contains(ex, [[E:\x\srv.exe]]) and contains(ex, "http://env:1") and contains(ex, [[C:\HSMP\bin\hsmp-sidecar.exe]]),
        tag .. ": env overrides hsmp_cfg per key")
end
-- no shared libs (isolated copy): built-in fallback with hsmp_cfg's defaults + stub logger
cfg[3] = function(tag)
    local dir, base = isolated_scripts()
    check(not T.exists(base .. "/mods/shared") and not T.exists(dir .. "/hsmp_cfg.lua") and not T.exists(dir .. "/hsmp_log.lua"),
        tag .. ": isolated copy has no shared libs next to it")
    boot(1920, 1080, 1.0, nil, nil, nil, dir)
    local logs = logtext()
    check(package.loaded["hsmp_cfg"] == nil and package.loaded["hsmp_log"] == nil, tag .. ": the shared libs are really hidden")
    check(contains(logs, "bin_dir=hsmp (fallback)") and contains(logs, "events=stub"),
        tag .. ": without hsmp_cfg/hsmp_log: hsmp_cfg defaults + stub")
    M.execs = {}
    host_lobby()
    local ex = execs()
    check(contains(ex, [[hsmp\hsmp-server.exe]]) and contains(ex, [[hsmp\hsmp-sidecar.exe]])
        and contains(ex, "HSMP_MASTER_URL=http://127.0.0.1:7778"), tag .. ": fallback = bin_dir hsmp, local master")
    check(not contains(ex, [[D:\]]), tag .. ": no developer path in the fallback")
    check(not T.exists(sd .. "/hsmp_events.jsonl"), tag .. ": the stub logger writes nothing")
    check(contains(logtext(), "enter screen: lobby"), tag .. ": the menu works without the shared libs")
end
-- fallback honours the same env overrides and validates the master URL
cfg[4] = function(tag)
    local dir = isolated_scripts()
    boot(1920, 1080, 1.0, { HSMP_BIN_DIR = [[F:\bin\]], HSMP_MASTER_URL = "http://bad url&calc", HSMP_SIDECAR_EXE = "G:/s/side.exe" },
        nil, nil, dir)
    M.execs = {}
    host_lobby()
    local ex = execs()
    check(contains(ex, [[F:\bin\hsmp-server.exe]]) and contains(ex, [[G:\s\side.exe]]), tag .. ": fallback honours HSMP_BIN_DIR / HSMP_SIDECAR_EXE")
    check(contains(ex, "HSMP_MASTER_URL=http://127.0.0.1:7778") and not contains(ex, "calc"), tag .. ": an unsafe master URL is rejected")
end

-- ---- process tracking (docs/development/testing.md) -----------------------------------------
-- The native module spawns our binaries (no shell) and kills only what this game spawned, by
-- its process handle. kills(): the proc_kill calls that killed something.
local function NM() return rawget(_G, "HSMPNative") end
local function kills() return T.filter(T.map(M.execs, tostring), function(e) return startswith(e, "proc_kill ") and contains(e, "-> true") end) end
local function kill_calls() return T.filter(T.map(M.execs, tostring), function(e) return startswith(e, "proc_kill ") end) end
local function spawned(pat)
    return T.filter(NM()._proc.spawned, function(s) return not s.capture and contains(s.exe, pat) end)
end
local function argstr(s) return table.concat(s.args, " ") end
-- was the newest process whose exe matches `pat` killed (by our handle)?
local function killed(pat)
    local s = spawned(pat)
    local p = s[#s] and s[#s].pid
    return p ~= nil and T.any(kills(), function(e) return contains(e, "proc_kill " .. p .. " ") end)
end

local pid = {}
-- no --help probes at load; HOST spawns server + sidecar with parent pid / owner key / shm;
-- CANCEL kills exactly those two (our handles), never by name; nothing spawned -> nothing killed
pid[1] = function(tag)
    boot(1920, 1080, 1.0)
    check(#T.filter(T.map(M.execs, tostring), function(e) return contains(e, "--help") end) == 0, tag .. ": no --help capability probes at load")
    check(contains(logtext(), "game pid 4242: passed as --parent-pid"), tag .. ": the game pid comes from the native module")
    M.execs = {}
    host_lobby()
    local sv, sc = spawned("hsmp-server.exe"), spawned("hsmp-sidecar.exe")
    check(#sv == 1 and #sc == 1, tag .. ": HOST spawns one server and one sidecar (" .. execs() .. ")")
    local winsd = sd:gsub("/", "\\")
    if #sv == 1 and #sc == 1 then
        local a = argstr(sv[1])
        check(contains(a, "--bind 0.0.0.0:7777") and contains(a, "--owner-key-file " .. winsd .. "\\.player_key")
            and contains(a, "--parent-pid 4242") and contains(a, "--map Map_Arena_Alley"),
            tag .. ": the listen server gets bind / map / owner key / parent pid (" .. a .. ")")
        local env = sv[1].opts.env or {}
        check(env.HSMP_LISTEN_HOST == "1" and env.HSMP_LOBBY_MAP == "Map_Arena_Alley" and env.HSMP_MASTER_URL == "http://127.0.0.1:7778"
            and env.HSMP_SERVER_NAME == "Willie's game" and env.HSMP_SERVER_MODE ~= nil and env.HSMP_REGION == nil,
            tag .. ": the listing values travel as the server's environment (" .. T.repr(env) .. ")")
        local b = argstr(sc[1])
        check(contains(b, "--server 127.0.0.1:7777") and contains(b, "--parent-pid 4242") and T.re_search([[--ipc shm:\S+]], b) ~= nil
            and contains(b, "--state-dir " .. winsd) and not contains(b, "--pid-file"),
            tag .. ": the sidecar gets server / state dir / parent pid / shm name, no pid file (" .. b .. ")")
    end
    check(not T.any(M.execs, function(e) return contains(tostring(e), 'start ""') or contains(tostring(e):lower(), "taskkill") end),
        tag .. ": no shell launch, no taskkill")
    M.execs = {}
    cancel_lobby()
    local k = kills()
    check(#k == 2 and T.any(k, function(e) return contains(e, "proc_kill " .. sv[1].pid .. " ") end)
        and T.any(k, function(e) return contains(e, "proc_kill " .. sc[1].pid .. " ") end),
        tag .. ": CANCEL kills exactly the server and sidecar this game spawned (" .. T.repr(k) .. ")")
    check(kit().cur == nil, tag .. ": CANCEL leaves the lobby")
    -- nothing of ours left: a second teardown kills nothing
    M.execs = {}
    host_lobby()
    NM()._proc.alive = {}       -- (forget the children: as if another game had spawned them)
    M.execs = {}
    cancel_lobby()
    check(#kills() == 0 and #kill_calls() == 2, tag .. ": processes that are not ours are never killed (" .. T.repr(kill_calls()) .. ")")
end
-- a role spawned again stops its previous child first; a child that already exited is "gone"
pid[2] = function(tag)
    boot(1920, 1080, 1.0)
    host_lobby()
    local old = spawned("hsmp-server.exe")[1]
    cancel_lobby()
    M.execs = {}
    host_lobby()
    local sv = spawned("hsmp-server.exe")
    check(#sv == 2 and sv[2].pid ~= old.pid, tag .. ": a new HOST spawns a new server")
    -- the sidecar exits by itself, the server does not: CANCEL reports the first, kills the second
    local sc = spawned("hsmp-sidecar.exe")
    NM().sc_proc_exit(sc[#sc].pid, 0)
    M.execs = {}
    cancel_lobby()
    check(#kills() == 1 and contains(kills()[1], "proc_kill " .. sv[2].pid .. " "), tag .. ": only the live server is killed (" .. T.repr(kill_calls()) .. ")")
    check(contains(logtext(), "stop sidecar: pid " .. sc[#sc].pid .. " (gone)"), tag .. ": an exited child is logged as gone")
end
-- no game pid (no native current_pid) -> the sidecar is not started; a hostile exe never reaches a shell
pid[3] = function(tag)
    boot(1920, 1080, 1.0, nil, "HSMPNative.current_pid = function() return nil end")
    check(contains(logtext(), "game pid unknown"), tag .. ": the missing game pid is logged")
    M.execs = {}
    host_lobby()
    check(#spawned("hsmp-sidecar.exe") == 0 and #spawned("hsmp-server.exe") == 0
        and contains(logtext(), "sidecar NOT started: shared-memory IPC unavailable (game pid unknown)"),
        tag .. ": without the game pid nothing is started")
    -- shell metacharacters stay inside the one exe path (no shell ever sees them)
    local evil = 'E:/x/srv & calc.exe'
    boot(1920, 1080, 1.0, { HSMP_SERVER_EXE = evil }, "HSMPNative.current_pid = function() return 4242 end")
    M.execs = {}
    host_lobby()
    local sv = T.filter(NM()._proc.spawned, function(s) return contains(s.exe, "calc") end)
    check(#sv == 1 and sv[1].exe == 'E:\\x\\srv & calc.exe' and not contains(argstr(sv[1]), "calc"),
        tag .. ": a hostile exe string is one argument to CreateProcess, never a shell line (" .. T.repr(sv[1] and sv[1].exe) .. ")")
    -- a quote in the exe cannot be quoted for CreateProcess: the module refuses it, nothing runs
    boot(1920, 1080, 1.0, { HSMP_SERVER_EXE = 'E:/x/srv" & calc2.exe' }, "HSMPNative.current_pid = function() return 4242 end")
    M.execs = {}
    host_lobby()
    check(#T.filter(NM()._proc.spawned, function(s) return contains(s.exe, "calc2") end) == 0
        and contains(logtext(), "FAILED bad"), tag .. ": an exe with a quote is refused (spawn -> bad), nothing started")
    check(not T.any(M.execs, function(e) return not startswith(tostring(e), "spawn ") and not startswith(tostring(e), "capture ")
        and not startswith(tostring(e), "proc_kill ") and contains(tostring(e), "calc") end), tag .. ": nothing else ran it")
end

-- ---- harness hooks: HSMP_AUTOTEST host|join, command channel, auto-ready, events ------------
-- the harness's `hsmp-tools ipc-ctl --pid <game> autotest <cmd> [arg]`: a dev_cmd AUTOTEST
-- record in the DevCtl ring (dev_op AUTOTEST = 1, TUNE = 2, TDIAG = 3)
local function send_cmd(id, c, arg) NM().sc_dev({ op = 1, id = id, key = c, arg = arg }) end
local function sent_of(c) return T.filter(evs("cmd_sent"), function(e) return e.cmd == c end) end

local at = {}
-- host (listen): hosts, no auto-START, auto-ready, picks/start/ready/leave from the channel
at[1] = function(tag)
    boot(1920, 1080, 1.0, { HSMP_AUTOTEST = "host" })
    check(contains(execs(), "hsmp-server.exe") and contains(logtext(), "AUTOTEST: hosting (listen"), tag .. ": host mode hosts a listen server")
    write(".sidecar.json", SIDECAR_2)
    write(".match.json", match_json(nil, "Map_Arena_Alley"))
    M.run(2000)
    check(kit().cur ~= nil and kit().cur.screen == "lobby", tag .. ": host mode opens the lobby")
    check(ob_has({ verb = "ready", cmd_id = true }), tag .. ": auto-ready sends a ready command")
    local rs = sent_of("ready")
    check(#rs == 1 and rs[1].arg == "true" and rs[1].via == "autotest", tag .. ": auto-ready is a cmd_sent{cmd=ready, via=autotest}")
    write(".match.json", match_json(nil, "Map_Arena_Alley", nil, { 1, 2 }))
    M.run(10000)
    check(not contains(outbox(), '"verb":"start"'), tag .. ": host mode never STARTs by itself")
    check(#sent_of("ready") == 1, tag .. ": no further auto-ready once the server lists us")
    -- pick_arena from the channel: same path as the tile, short name resolved
    send_cmd(1, "pick_arena", "Yard")
    M.run(300)
    check(ob_has({ kind = "set_map", map = "Map_Arena_Yard", cmd_id = true }), tag .. ": channel pick_arena Yard -> set_map Map_Arena_Yard")
    local ps = sent_of("pick_arena")
    check(#ps == 1 and ps[1].arg == "Map_Arena_Yard" and ps[1].via == "autotest", tag .. ": cmd_sent for the channel pick")
    M.run(800)
    check(truthy(kit().cur.w.maps.by_key["Map_Arena_Yard"].pending), tag .. ": the lobby shows the channel pick as pending")
    write(".match.json", match_json(nil, "Map_Arena_Yard", nil, { 1, 2 }))
    M.run(1100)
    local pr = T.filter(evs("cmd_result"), function(e) return e.cmd == "pick_arena" end)
    check(#pr == 1 and pr[1].ok == true and pr[1].cmd_id == ps[1].cmd_id, tag .. ": cmd_result ok for the channel pick")
    -- TUNE / TDIAG records are other mods' business: the menu ignores them
    NM().sc_dev({ op = 2, id = 2, key = "servo", num = 0.5 })
    NM().sc_dev({ op = 3, id = 3, key = "tdiag", num = 1 })
    M.run(600)
    check(not contains(outbox(), '"verb":"start"') and #evs("x_autotest_cmd") == 1, tag .. ": TUNE / TDIAG records are not autotest commands")
    send_cmd(3, "start")
    M.run(300)
    M.run(600)
    check(T.count(outbox(), '"verb":"start"') == 1, tag .. ": a channel START runs once")
    local ss = sent_of("start")
    check(#ss == 1 and ss[1].via == "autotest", tag .. ": cmd_sent for the channel START")
    write(".match.json", match_json("countdown", "Map_Arena_Yard", nil, { 1, 2 }))
    M.run(600)
    local sr = T.filter(evs("cmd_result"), function(e) return e.cmd == "start" end)
    check(#sr == 1 and sr[1].ok == true, tag .. ": cmd_result ok for START")
    check(#evs("x_autotest_cmd") == 2, tag .. ": every executed channel command is logged as x_autotest_cmd (" .. #evs("x_autotest_cmd") .. ")")
    -- leave: same path as CLOSE LOBBY (our server killed by its handle); no command file
    write(".match.json", match_json(nil, "Map_Arena_Yard", nil, { 1, 2 }))
    M.run(600)
    local srv = spawned("hsmp-server.exe")
    M.execs = {}
    send_cmd(4, "leave")
    M.run(2000)
    check(#srv == 1 and T.any(kills(), function(e) return contains(e, "proc_kill " .. srv[1].pid .. " ") end), tag .. ": leave stops our server (" .. T.repr(kill_calls()) .. ")")
    check(kit().cur == nil and contains(logtext(), "closing the MP session (CANCEL)"), tag .. ": leave closes the lobby like the button")
    check(not T.exists(sd .. "/.autotest.cmd.jsonl"), tag .. ": no command file in the state dir")
    local bad = T.filter(events(), function(e) return e._missing ~= nil or e.ev == "_bad_event" end)
    check(#bad == 0, tag .. ": no event misses a required field (" .. T.repr(bad[1]) .. ")")
end
-- join: joins the address, READY=0 means no auto-ready, START is not ours, quit quits
at[2] = function(tag)
    boot(1920, 1080, 1.0, { HSMP_AUTOTEST = "join", HSMP_AUTOTEST_ADDR = "10.1.2.3:7777", HSMP_AUTOTEST_READY = "0" })
    local ex = execs()
    check(contains(ex, "hsmp-sidecar.exe\" --server 10.1.2.3:7777") and not contains(ex, "--bind 0.0.0.0"),
        tag .. ": join mode joins HSMP_AUTOTEST_ADDR and hosts nothing")
    write(".sidecar.json", '{"status":"connected","peer_id":2,"is_admin":false,"peers":[{"id":1,"nick":"Willie"},{"id":2,"nick":"Mate"}]}\n')
    write(".match.json", match_json(nil, "Map_Arena_Pit", nil, {}))
    M.run(5000)
    check(not contains(outbox(), '"verb":"ready"'), tag .. ": HSMP_AUTOTEST_READY=0 -> no auto-ready")
    local lr = evs("lobby_ready")
    check(#lr == 1 and lr[1].role == "join" and lr[1].peer_id == 2, tag .. ": the joiner emits lobby_ready")
    send_cmd(1, "start")
    send_cmd(2, "ready")
    M.run(300)
    check(not contains(outbox(), '"verb":"start"') and contains(logtext(), "start ignored - this instance is not seat 1"),
        tag .. ": START from a joiner's channel is ignored")
    check(ob_has({ verb = "ready", cmd_id = true }) and #sent_of("ready") == 1, tag .. ": channel ready -> ready command")
    M.execs = {}
    send_cmd(3, "quit")
    M.run(2000)   -- the quit waits for the leave (async)
    check(T.any(M.execs, function(e) return tostring(e) == "QuitGame 0" end) and not T.any(M.execs, function(e) return tostring(e) == "console quit" end),
        tag .. ": quit quits the game through KismetSystemLibrary:QuitGame (PlayerController:ConsoleCommand is not callable in UE4SS)")
end
-- host + EXTERNAL: seat 1 on a dedicated server (admin), START allowed, names resolved
at[3] = function(tag)
    boot(1920, 1080, 1.0, { HSMP_AUTOTEST = "host", HSMP_AUTOTEST_EXTERNAL = "1", HSMP_AUTOTEST_ADDR = "10.9.9.9:7777" })
    local ex = execs()
    check(contains(ex, "--server 10.9.9.9:7777") and not contains(ex, "--bind 0.0.0.0"), tag .. ": external host joins the dedicated server")
    write(".sidecar.json", SIDECAR_2)
    write(".match.json", match_json(nil, "Map_Arena_Alley", nil, { 1, 2 }))
    M.run(2000)
    send_cmd(1, "pick_arena", "Lords Hall")
    M.run(300)
    send_cmd(2, "pick_arena", "map_arena_cellar")
    M.run(300)
    send_cmd(3, "pick_arena", "Nowhere")
    M.run(300)
    local ob = outbox()
    check(contains(ob, '"map":"Map_Arena_LordsHall"') and contains(ob, '"map":"Map_Arena_Cellar"') and contains(ob, '"map":"Map_Arena_Nowhere"'),
        tag .. ": arena names resolved (display, lower case, unknown passed to the server)")
    send_cmd(4, "start")
    M.run(300)
    check(ob_has({ verb = "start", cmd_id = true }), tag .. ": the admin seat STARTs from the channel")
    check(#T.filter(evs("cmd_result"), function(e) return e.cmd == "pick_arena" end) == 2, tag .. ": superseded picks get their one cmd_result")
end
-- lobby_ready{epoch}: once per epoch, again (with a notice) after a server restart
at[4] = function(tag)
    boot(1920, 1080, 1.0, { HSMP_AUTOTEST = "host" })
    write(".sidecar.json", '{"status":"connected","peer_id":1,"epoch":5,"peers":[{"id":1,"nick":"Willie"}]}\n')
    write(".match.json", match_json(nil, "Map_Arena_Alley", nil, { 1 }))
    M.run(3000)
    local lr = evs("lobby_ready")
    check(#lr == 1 and lr[1].epoch == 5 and lr[1].notice == nil, tag .. ": lobby_ready carries the server epoch")
    write(".sidecar.json", '{"status":"connected","peer_id":1,"epoch":6,"peers":[{"id":1,"nick":"Willie"}]}\n')
    M.run(1100)
    lr = evs("lobby_ready")
    check(#lr == 2 and lr[2].epoch == 6 and contains(lr[2].notice or "", "restarted"), tag .. ": a new epoch re-emits lobby_ready with a notice")
    M.run(3000)
    check(#evs("lobby_ready") == 2, tag .. ": no repeat while the epoch is unchanged")
end
-- leave mid-match (after a level change): no freed widget touched, the Director is asked for the menu
at[5] = function(tag)
    boot(1920, 1080, 1.0, { HSMP_AUTOTEST = "host" })
    write(".sidecar.json", SIDECAR_2)
    write(".match.json", match_json(nil, "Map_Arena_Alley", nil, { 1, 2 }))
    M.run(2000)
    send_cmd(1, "start")
    M.run(300)
    write(".match.json", match_json("countdown", "Map_Arena_Alley", nil, { 1, 2 }))
    run_until(req_written, 1000)
    write(".travel_ack.json", pyjson({ "seq", T.json_decode(read(".travel_request.json")).seq, "status", "accepted" }))
    M.run(600)
    M.premap(); M.kill_all()
    M.run(500)
    send_cmd(2, "leave")
    M.run(1500)
    local req = T.json_decode(read(".travel_request.json") or "{}")
    check(req.want == "menu" and req.reason == "autotest_leave", tag .. ": leave in the arena asks the Director for the menu (" .. T.repr(req) .. ")")
    check(contains(logtext(), "closing the MP session (leave)"), tag .. ": the session is closed")
    check(#M.dead_touch == 0, tag .. ": no freed widget touched (" .. T.repr({ M.dead_touch[1], M.dead_touch[2] }) .. ")")
    check(#M.openlevel == 0, tag .. ": the menu never travels itself")
end
-- without HSMP_AUTOTEST the channel does not exist
at[6] = function(tag)
    boot(1920, 1080, 1.0)
    host_lobby("Map_Arena_Alley")
    send_cmd(1, "start")
    send_cmd(2, "leave")
    M.run(2000)
    check(not contains(outbox(), '"verb":"start"') and kit().cur ~= nil and #evs("x_autotest_cmd") == 0,
        tag .. ": the command channel is ignored without HSMP_AUTOTEST")
    check(not contains(outbox(), '"verb":"ready"'), tag .. ": no auto-ready without HSMP_AUTOTEST")
end
-- lobby_ready per lobby ARRIVAL: an aborted/finished match brings the menu back to the lobby
-- (HSMPMatch's .return_to_lobby.flag) and the harness waits for lobby_ready again (latched
-- once per session, a multi-arena autotest would stall after the first arena)
at[7] = function(tag)
    boot(1920, 1080, 1.0, { HSMP_AUTOTEST = "join", HSMP_AUTOTEST_ADDR = "127.0.0.1:7777" })
    write(".sidecar.json", '{"status":"connected","peer_id":2,"epoch":5,"peers":[{"id":1,"nick":"Willie"},{"id":2,"nick":"Mate"}]}\n')
    write(".match.json", match_json(nil, "Map_Arena_Alley", nil, { 1, 2 }))
    M.run(2000)
    check(#evs("lobby_ready") == 1, tag .. ": first lobby arrival -> lobby_ready")
    write(".match.json", match_json("countdown", "Map_Arena_Alley", nil, { 1, 2 }))
    M.run(1100)
    -- ABORT: back to the menu world with the flag; the server is in the lobby again
    write(".match.json", match_json(nil, "Map_Arena_Alley", nil, { 1, 2 }))
    write(".return_to_lobby.flag", "1")
    M.premap(); M.kill_all()
    M.menu = nil
    do
        local new_obj = M.new_obj
        M.objs = {}
        M.canvas = new_obj("CanvasPanel", "CanvasPanel_5")
        for i, n in ipairs({ "Button", "Button_0", "Button_1", "Button_2", "Button_3", "Button_4" }) do
            local b = new_obj("Button", n)
            local slot = new_obj("CanvasPanelSlot", "slot_" .. n)
            rawset(slot, "x", -700); rawset(slot, "y", -360 + i * 110); rawset(slot, "w", 469); rawset(slot, "h", 120)
            b.__props.Slot = slot
            table.insert(rawget(b, "__children"), new_obj("TextBlock", "txt_" .. n))
            table.insert(rawget(M.canvas, "__children"), b)
        end
        M.menu = new_obj("UI_Startup_Menu_C", "UI_Startup_Menu_C_1")
        M.menu.__props.WidgetTree = new_obj("WidgetTree", "wt")
        M.menu.__props.WidgetTree.__props.RootWidget = M.canvas
        FindAllOf = function(c) if c == "UI_Startup_Menu_C" then return { M.menu } end return nil end
    end
    M.on_new(M.menu)
    M.run(3000)
    local lr = evs("lobby_ready")
    check(#lr == 2 and lr[2].epoch == 5 and lr[2].notice == nil and lr[2].peer_id == 2,
        tag .. ": the return to the lobby re-emits lobby_ready (same epoch, no notice) (" .. T.repr(lr) .. ")")
    M.run(3000)
    check(#evs("lobby_ready") == 2, tag .. ": no repeat while staying in the lobby")
end

-- ---- local master, master fallbacks, LAN section, recent chips -------------------------------
-- Every process / probe result is mocked (lib/hsmp_native_records.lua): spawns only record,
-- a probe "answers" when the test completes its capture (curl body or hsmp-query stdout).
local function bindir_with(names)
    local d = T.tmpdir("hsmp_bin_")
    for _, n in ipairs(names) do T.write(d .. "/" .. n, "stub") end
    return d
end
local function lm() return package.loaded["local_master"] end
local function host_click()      -- HOST GAME ribbon, nothing else run
    write(".sidecar.json", SIDECAR_2)
    write(".match.json", match_json(nil, "Map_Arena_Alley"))
    local rib = T.filter(M.live(), function(w) return startswith(tostring(w.name), "HSMPBtn_top1") end)
    rawset(rib[1].obj, "pressed", true); M.run(40); rawset(rib[1].obj, "pressed", false)
end
local function exec_index(pred)
    for i, e in ipairs(M.execs) do if pred(tostring(e)) then return i end end
    return nil
end
local function master_starts() return T.filter(T.map(M.execs, tostring), function(e) return contains(e, "hsmp-master.exe\" --bind") end) end
-- the local master's probe in flight (hsmp-query --gen lm<N> or curl -m 3 .../v1/servers) and its answer
local function master_probe()
    local best
    for h, c in pairs(NM()._proc.captures) do
        local a = table.concat(c.args, " ")
        if not c.done and (contains(a, "--gen lm") or (contains(c.exe, "curl") and contains(a, "-m 3 "))) and (not best or h > best) then best = h end
    end
    return best
end
local function probe_answer(body, code)
    local h = master_probe()
    if h then NM().sc_capture_done(h, code or 0, body) end
    return h
end

local net = {}
-- 1. local master_url, nothing answers: start it (no window, parent pid), verify, CANCEL stops exactly it
net[1] = function(tag)
    local bin = bindir_with({ "hsmp-master.exe" })
    boot(1920, 1080, 1.0, { HSMP_BIN_DIR = bin })
    check(not T.any(T.map(M.execs, tostring), function(e) return contains(e, "--help") end), tag .. ": no hsmp-master --help probe at load")
    M.execs = {}
    host_click()
    local L = lm()
    check(L ~= nil and L.state == "probing", tag .. ": HOST with a local master_url probes it (" .. tostring(L and L.state) .. ")")
    local pi = exec_index(function(e) return startswith(e, 'capture "curl"') and contains(e, "http://127.0.0.1:7778/v1/servers") end)
    check(pi ~= nil and master_probe() ~= nil, tag .. ": the probe is an async curl of /v1/servers, output captured (no file)")
    check(contains(execs(), "hsmp-server.exe"), tag .. ": the server is not held back by the probe")
    M.run(3000)
    check(#master_starts() == 0 and L.state == "probing", tag .. ": no start before the probe deadline")
    M.run(2600)
    local st = master_starts()
    check(#st == 1 and startswith(st[1], 'spawn "') and contains(st[1], "--bind 127.0.0.1:7778") and contains(st[1], "--parent-pid 4242")
        and not contains(st[1], "--pid-file"),
        tag .. ": no answer -> hsmp-master spawned on 127.0.0.1:7778 with --parent-pid (" .. T.repr(st) .. ")")
    check(L.state == "starting" and L.owned == true, tag .. ": we started it -> owned")
    M.run(2100)     -- grace, then the verify probe is out
    check(L.state == "verifying", tag .. ": re-probe after the start (" .. L.state .. ")")
    probe_answer("[]")
    M.run(600)
    check(L.state == "up" and L.owned, tag .. ": started master answers -> up, owned")
    check(contains(logtext(), "local master: started and answering at http://127.0.0.1:7778"), tag .. ": logged")
    M.run(1600)     -- the lobby opens
    local mpid = T.filter(NM()._proc.spawned, function(s) return contains(s.exe, "hsmp-master.exe") end)[1].pid
    M.execs = {}
    cancel_lobby()
    local k = kills()
    check(T.any(k, function(e) return contains(e, "proc_kill " .. mpid .. " ") end),
        tag .. ": CANCEL stops our master by its process handle (" .. T.repr(k) .. ")")
    check(L.state == "idle" and not L.owned, tag .. ": state reset")
end
-- 14. the local master always gets --parent-pid (exits with the game)
net[14] = function(tag)
    local bin = bindir_with({ "hsmp-master.exe" })
    boot(1920, 1080, 1.0, { HSMP_BIN_DIR = bin })
    M.execs = {}
    host_click()
    M.run(5700)
    local st = master_starts()
    check(#st == 1 and contains(st[1], "--parent-pid 4242"), tag .. ": the local master is started with --parent-pid (" .. T.repr(st) .. ")")
end
-- 2. a master already answers (test harness / another instance): used, never started, never stopped
net[2] = function(tag)
    local bin = bindir_with({ "hsmp-master.exe" })
    boot(1920, 1080, 1.0, { HSMP_BIN_DIR = bin })
    M.execs = {}
    host_click()
    probe_answer('[{"name":"x","host":"127.0.0.1","port":7777}]')
    M.run(1600)
    local L = lm()
    check(L.state == "up" and not L.owned, tag .. ": an answering master we did not spawn is used but not owned")
    check(#master_starts() == 0, tag .. ": nothing started")
    M.execs = {}
    cancel_lobby()
    check(#T.filter(kill_calls(), function(e) return not T.any(NM()._proc.spawned, function(s)
        return not contains(s.exe, "hsmp-master") and contains(e, "proc_kill " .. s.pid .. " ") end) end) == 0,
        tag .. ": CANCEL leaves a master we did not start running (" .. T.repr(kill_calls()) .. ")")
    check(contains(logtext(), "local master: not ours - left running"), tag .. ": logged")
end
-- 3. we started it in an earlier HOST of this game: still ours, stopped on quit
net[3] = function(tag)
    local bin = bindir_with({ "hsmp-master.exe" })
    boot(1920, 1080, 1.0, { HSMP_BIN_DIR = bin })
    host_click()
    M.run(5700)                 -- no answer -> started
    M.run(2100)
    probe_answer("[]")
    M.run(600)
    local L = lm()
    check(L.state == "up" and L.owned, tag .. ": started by this game -> owned")
    local mpid = T.filter(NM()._proc.spawned, function(s) return contains(s.exe, "hsmp-master.exe") end)[1].pid
    M.execs = {}
    -- QUIT MP ribbon (top5) -> quit_desktop
    exit_screen()
    local rib = T.filter(M.live(), function(w) return startswith(tostring(w.name), "HSMPBtn_top5") end)
    check(#rib == 1, tag .. ": QUIT MP ribbon present")
    if #rib == 1 then M.click(rib[1].obj) end
    M.run(2000)   -- the session teardown is asynchronous
    check(T.any(kills(), function(e) return contains(e, "proc_kill " .. mpid .. " ") end),
        tag .. ": quit stops the master we own (" .. T.repr(kill_calls()) .. ")")
    check(T.any(M.execs, function(e) return tostring(e) == "QuitGame 0" end), tag .. ": and still quits")
end
-- 4. our master hangs (no answer at the next HOST): that process is stopped BEFORE the new one starts;
--    a master of ours that exited is not ours any more
net[4] = function(tag)
    local bin = bindir_with({ "hsmp-master.exe" })
    boot(1920, 1080, 1.0, { HSMP_BIN_DIR = bin })
    host_click()
    M.run(5700)
    M.run(2100 + 3 * 5600)      -- verify probes never answered -> failed, still running
    local L = lm()
    check(L.state == "failed" and L.owned, tag .. ": an unanswering master of ours ends failed (" .. L.state .. ")")
    local old = T.filter(NM()._proc.spawned, function(s) return contains(s.exe, "hsmp-master.exe") end)[1].pid
    M.execs = {}
    L.ensure("HOST")                -- the next HOST: still nothing answers
    M.run(5700)
    local ki = exec_index(function(e) return contains(e, "proc_kill " .. old .. " ") end)
    local si = exec_index(function(e) return contains(e, "hsmp-master.exe\" --bind") end)
    check(ki ~= nil and si ~= nil and ki < si, tag .. ": our hung master stopped before the new start (" .. execs() .. ")")
end
-- 5. hsmp-query present: the probe is hsmp-query (stdout captured); a definite "unreachable" starts at once
net[5] = function(tag)
    local bin = bindir_with({ "hsmp-master.exe", "hsmp-query.exe" })
    boot(1920, 1080, 1.0, { HSMP_BIN_DIR = bin })
    M.execs = {}
    host_click()
    local L = lm()
    local ex = execs()
    check(contains(ex, 'hsmp-query.exe" --master http://127.0.0.1:7778 --gen lm' .. L.gen) and not contains(ex, "--out"),
        tag .. ": probe via hsmp-query --master <url>, no output file (" .. ex .. ")")
    probe_answer("HSMPQ\t1\ngen\tlm" .. L.gen .. "\nstatus\tmaster_unreachable\thttp://127.0.0.1:7778: connection refused\ndone\t1\n", 0)
    M.run(600)
    local st = master_starts()
    check(#st == 1 and L.owned, tag .. ": definite no-answer starts at once; spawned by us -> owned")
    check(contains(logtext(), "connection refused"), tag .. ": the reason is logged")
    -- an older probe's answer (other gen) is ignored
    M.run(1100)
    probe_answer("HSMPQ\t1\ngen\tlm999\nstatus\tok\t\ndone\t1\n", 0)
    M.run(600)
    check(L.state == "verifying", tag .. ": an answer of another generation is ignored (" .. L.state .. ")")
    M.run(5600)     -- that probe timed out; the next verify probe is out
    probe_answer("HSMPQ\t1\ngen\tlm" .. L.gen .. "\nstatus\tok\t\ndone\t1\n", 0)
    M.run(600)
    check(L.state == "up", tag .. ": hsmp-query status ok -> up (" .. L.state .. ")")
end
-- 6. remote master_url: nothing probed or started; missing exe / failing start are reported, not retried forever
net[6] = function(tag)
    local bin = bindir_with({ "hsmp-master.exe" })
    boot(1920, 1080, 1.0, { HSMP_BIN_DIR = bin, HSMP_MASTER_URL = "https://master.example.net" })
    M.execs = {}
    host_click()
    M.run(8000)
    check(lm().state == "remote" and #master_starts() == 0 and master_probe() == nil,
        tag .. ": a remote master_url starts nothing")
    check(contains(execs(), "HSMP_MASTER_URL=https://master.example.net"), tag .. ": the server registers with the remote master")
    check(lm().local_target("http://localhost:9000") == "localhost" and select(2, lm().local_target("http://[::1]:7778")) == 7778
        and lm().local_target("http://10.0.0.5:7778") == nil and lm().local_target("http://127.0.0.1") == "127.0.0.1",
        tag .. ": local_target() recognises 127.x / localhost / ::1 only")
end
net[7] = function(tag)
    boot(1920, 1080, 1.0, { HSMP_BIN_DIR = T.tmpdir("hsmp_nobin_") })
    host_click()
    M.run(5600)
    check(lm().state == "missing" and #master_starts() == 0 and contains(logtext(), "hsmp-master.exe not found"),
        tag .. ": no hsmp-master.exe -> reported, nothing started")
end
-- a start that never answers -> failed after 3 verify probes (no endless loop)
net[12] = function(tag)
    local bin = bindir_with({ "hsmp-master.exe" })
    boot(1920, 1080, 1.0, { HSMP_BIN_DIR = bin })
    host_click()
    M.run(5600 + 3 * 5600 + 2000)
    check(lm().state == "failed" and #master_starts() == 1 and contains(logtext(), "does not answer"),
        tag .. ": a master that never answers ends as failed after 3 probes (" .. lm().state .. ")")
end

-- browser: master list + fallbacks, empty state, LAN section, recent chips
local function query_bin() return bindir_with({ "hsmp-query.exe" }) end
local function enter_browser()
    enter_screen("browser")
    return package.loaded["browser"]
end
local function result_text(B, lines)
    return "HSMPQ\t1\ngen\t" .. B.q.gen .. "\n" .. lines
end
local function S(host, port, name, source, ping)
    return table.concat({ "S", host, tostring(port), name, "Map_Arena_Pit", "duel", "1", "8", "0", "0", "1", "",
        tostring(ping or 20), source, "1" }, "\t") .. "\n"
end
-- the helpers run through the native module (IPC.spawn_capture): the mock's
-- process table (lib/hsmp_native_records.lua) records them; a test completes a capture
-- (one local table: this chunk is near Lua's 200-locals limit)
local CAP = {}
function CAP.NP() return rawget(_G, "HSMP_IPC").N end
function CAP.captures(pat)
    return T.filter(CAP.NP()._proc.spawned, function(e) return e.capture and contains(e.exe, pat) end)
end
function CAP.argline(e) return table.concat(e.args, " ") end
function CAP.last(pat) local c = CAP.captures(pat); return c[#c] end
function CAP.no_state_outputs(tag)
    for _, n in ipairs({ ".browser_result.tsv", ".servers.json", ".servers.master", ".caps.query.txt" }) do
        check(not T.exists(sd .. "/" .. n), tag .. ": no " .. n .. " in the state dir")
    end
end
net[8] = function(tag)
    local bin = query_bin()
    boot(1920, 1080, 1.0, { HSMP_BIN_DIR = bin, HSMP_MASTER_URL = "http://m1.example:7778, http://m2.example:7778" })
    check(contains(logtext(), "master fallbacks: http://m2.example:7778"), tag .. ": hsmp.cfg master_url list (primary + fallback) loaded")
    check(T.eq(lm() and { lm().state } or {}, { "idle" }), tag .. ": local master idle at load")
    check(#T.filter(CAP.NP()._proc.spawned, function(e) return contains(CAP.argline(e), "--help") end) == 0,
        tag .. ": no --help capability probe (the shipped hsmp-query matches the module)")
    local B = enter_browser()
    local q = CAP.last("hsmp-query.exe")
    local ex = q and CAP.argline(q) or ""
    check(q ~= nil and contains(ex, "--master http://m1.example:7778 --master http://m2.example:7778 --lan --lan-ports 7777-7786")
        and contains(ex, "--gen " .. B.q.gen) and not contains(ex, "--out"),
        tag .. ": hsmp-query captured: every master in order, plus LAN discovery, no output file (" .. ex .. ")")
    check(B.q.limit == 16, tag .. ": deadline grows per fallback (" .. tostring(B.q.limit) .. ")")
    -- still running: loading state, nothing parsed
    M.run(250)
    local W = B.b.w
    check(B.q.inflight and contains(W.empty.last, "Loading"), tag .. ": loading state while hsmp-query runs")
    -- master 2 answered
    CAP.NP().sc_capture_done(q.pid, 0, result_text(B, "status\tok\t\ncounts\t1\t0\nproto\t5\nmaster\thttp://m2.example:7778\t2\t2\nlan\t1\t0\ndone\t1\n"
        .. S("5.6.7.8", 7777, "Remote One", "master")))
    M.run(250)
    check(contains(W.status.last, "via m2.example:7778 (fallback 2/2)"), tag .. ": status shows which master answered (" .. W.status.last .. ")")
    check(B.q.inflight == false and W.refresh.label.last == "REFRESH", tag .. ": refresh finished (no spinner)")
    CAP.no_state_outputs(tag)
    -- every master down, no LAN server: clear empty state, no spinner
    B.q.inflight = false
    B.refresh()
    q = CAP.last("hsmp-query.exe")
    CAP.NP().sc_capture_done(q.pid, 0, result_text(B, "status\tmaster_unreachable\t2 lists tried: http://m1.example:7778: timed out; http://m2.example:7778: connection refused\n"
        .. "counts\t0\t0\nproto\t5\nlan\t1\t0\ndone\t1\n"))
    M.run(250)
    check(startswith(W.empty.last, "Server list unavailable (2 lists tried") and contains(W.empty.last, "Use DIRECT CONNECT or LAN"),
        tag .. ": all masters down -> 'Server list unavailable (...). Use DIRECT CONNECT or LAN' (" .. tostring(W.empty.last) .. ")")
    check(#B.view == 0 and not B.q.inflight and W.refresh.label.last == "REFRESH", tag .. ": no stale list, no spinner")
    check(contains(W.status.last, "LAN: 0 found"), tag .. ": status says the LAN scan found nothing (" .. W.status.last .. ")")
    layout_checks(tag .. " browser unavailable", 1920, 1080)
    -- a result that never comes: the deadline ends the refresh with the same empty state
    -- and stops OUR hsmp-query (by its process handle)
    B.q.inflight = false
    B.refresh()
    q = CAP.last("hsmp-query.exe")
    B.q.started = os.time() - 100
    M.run(250)
    check(not B.q.inflight and B.q.status == "timeout" and startswith(W.empty.last, "Server list unavailable ("),
        tag .. ": no result before the deadline -> unavailable, not an endless spinner (" .. tostring(W.empty.last) .. ")")
    check(T.any(CAP.NP()._proc.killed, function(p) return p == q.pid end), tag .. ": the late hsmp-query is killed by handle")
    CAP.no_state_outputs(tag)
end
-- an answer that is not this refresh's (old gen) or no answer at all: a finished error, never parsed as a list;
-- a helper that does not start: the reason at once, no spinner
net[9] = function(tag)
    local bin = query_bin()
    boot(1920, 1080, 1.0, { HSMP_BIN_DIR = bin, HSMP_MASTER_URL = "http://m1.example:7778,http://m2.example:7778" })
    local B = enter_browser()
    local q = CAP.last("hsmp-query.exe")
    CAP.NP().sc_capture_done(q.pid, 0, "HSMPQ\t1\ngen\told_gen\nstatus\tok\t\ncounts\t1\t0\ndone\t1\n" .. S("6.6.6.6", 7777, "Stale", "master"))
    M.run(250)
    check(not B.q.inflight and B.q.status == "master_error" and #B.servers == 0 and contains(B.q.msg, "without a result"),
        tag .. ": an answer for another refresh is not shown (" .. tostring(B.q.msg) .. ")")
    B.q.inflight = false
    B.refresh()
    q = CAP.last("hsmp-query.exe")
    CAP.NP().sc_capture_done(q.pid, 3, "& calc.exe\n")
    M.run(250)
    check(not B.q.inflight and B.q.status == "master_error" and contains(B.q.msg, "exited 3"),
        tag .. ": hostile / garbage output is only text: an error, nothing run (" .. tostring(B.q.msg) .. ")")
    local n = #CAP.NP()._proc.spawned
    B.q.inflight = false
    CAP.NP()._proc.fail = "spawn: access denied"
    B.refresh()
    check(not B.q.inflight and B.q.status == "master_error" and contains(B.q.msg, "did not start (spawn: access denied)")
        and #CAP.NP()._proc.spawned == n, tag .. ": a helper that does not start -> the reason, no spinner (" .. tostring(B.q.msg) .. ")")
end
-- curl fallback (no hsmp-query): the masters are asked in order, one curl.exe each
net[13] = function(tag)
    boot(1920, 1080, 1.0, { HSMP_BIN_DIR = T.tmpdir("hsmp_nobin_"), HSMP_MASTER_URL = "http://m1.example:7778,http://m2.example:7778" })
    local B = enter_browser()
    local c1 = CAP.last("curl")
    check(c1 ~= nil and CAP.argline(c1) == "-s -f -m 4 http://m1.example:7778/v1/servers" and #CAP.captures("curl") == 1,
        tag .. ": curl fallback asks the primary first (" .. (c1 and CAP.argline(c1) or "none") .. ")")
    M.run(250)
    check(B.q.inflight and #CAP.captures("curl") == 1, tag .. ": nothing read while curl runs")
    CAP.NP().sc_capture_done(c1.pid, 22, "")
    M.run(250)
    local c2 = CAP.last("curl")
    check(#CAP.captures("curl") == 2 and CAP.argline(c2) == "-s -f -m 4 http://m2.example:7778/v1/servers",
        tag .. ": a failed master -> the next one")
    CAP.NP().sc_capture_done(c2.pid, 0, '[{"name":"Via Curl","host":"9.9.9.9","port":7777}]')
    M.run(250)
    check(#B.servers == 1 and contains(B.b.w.status.last, "via m2.example:7778 (fallback 2/2)"),
        tag .. ": curl result names the fallback that answered (" .. B.b.w.status.last .. ")")
    -- every master fails: finished at once, no spinner
    B.q.inflight = false
    B.refresh()
    CAP.NP().sc_capture_done(CAP.last("curl").pid, 7, "")
    M.run(250)
    CAP.NP().sc_capture_done(CAP.last("curl").pid, 7, "")
    M.run(250)
    check(not B.q.inflight and B.q.status == "timeout" and contains(B.q.msg, "none of 2 server lists answered"),
        tag .. ": every master failed -> unavailable (" .. tostring(B.q.msg) .. ")")
    CAP.no_state_outputs(tag)
end
-- LAN section: header rows, LAN first, headers not selectable; master down + LAN servers
net[10] = function(tag)
    boot(1920, 1080, 1.0)
    local B = enter_browser()
    local r = B._parse_tool_text("HSMPQ\t1\ngen\tg\nstatus\tok\t\ncounts\t1\t0\nproto\t5\nmaster\thttp://127.0.0.1:7778\t1\t1\nlan\t1\t1\ndone\t1\n"
        .. S("192.168.1.20", 7777, "Lan Pit", "lan", 3) .. S("5.6.7.8", 7777, "Remote One", "master", 80))
    check(r ~= nil and r.master_idx == 1 and r.lan_on and r.lan_n == 1 and #r.servers == 2, tag .. ": parser reads master / lan lines")
    B._accept_result(r)
    B.render()
    local W = B.b.w
    local v = B.view
    check(#v == 4 and v[1].header and contains(v[1].label, "LAN") and v[2].name == "Lan Pit" and v[3].header
        and contains(v[3].label, "INTERNET") and v[4].name == "Remote One",
        tag .. ": LAN section (header + rows) above the INTERNET section (" .. T.repr(T.map(v, function(s) return s.label or s.name end)) .. ")")
    check(contains(W.rows[1].cells[2].last, "LAN  -  1 server on your network") and W.rows[1].server_key == nil,
        tag .. ": header row drawn, not a server")
    check(W.rows[2].cells[7].last == "LAN  v1" or startswith(W.rows[2].cells[7].last, "LAN"), tag .. ": LAN rows tagged LAN (" .. tostring(W.rows[2].cells[7].last) .. ")")
    M.click(W.rows[1].btn.button)
    check(B.sel == nil, tag .. ": clicking a header selects nothing")
    M.click(W.rows[2].btn.button)
    check(B.sel == "192.168.1.20:7777", tag .. ": LAN server selectable")
    check(contains(W.status.last, "2 servers, 2 shown") and contains(W.status.last, "LAN 1"), tag .. ": counts exclude headers (" .. W.status.last .. ")")
    layout_checks(tag .. " browser LAN", 1920, 1080)
    -- every master down, LAN still lists local play
    B._accept_result({ status = "master_unreachable", msg = "http://127.0.0.1:7778: connection refused", lan_on = true, done = true,
        servers = { { host = "192.168.1.20", port = 7777, name = "Lan Pit", map = "", mode = "", players = 0, max = 8, pwd = false,
                      proto = 0, version = "", region = "", ping = 3, source = "lan", live = true } } })
    B.render()
    v = B.view
    check(#v == 3 and v[1].header and v[2].name == "Lan Pit" and contains(v[3].label, "server list unavailable"),
        tag .. ": master down -> LAN server still listed, INTERNET marked unavailable")
    check(startswith(W.status.last, "Server list unavailable (") and contains(W.status.last, "LAN: 1 found"),
        tag .. ": status explains it (" .. W.status.last .. ")")
end
-- direct-connect history: last 5 as one-click chips
net[11] = function(tag)
    boot(1920, 1080, 1.0)
    local B = enter_browser()
    local W = B.b.w
    check(#W.recent == 5 and not truthy(W.recent[1].shown) and W.recent_label.last == "RECENT: none yet", tag .. ": no history -> no chips")
    local addrs = { "10.0.0.1:7777", "10.0.0.2:7777", "10.0.0.3:7777", "10.0.0.4:7777", "10.0.0.5:7777", "10.0.0.6:7777" }
    -- six direct connects through the box (newest first, capped at 5)
    for _, a in ipairs(addrs) do
        if not B.b then B = enter_browser() end
        B.joining_until = 0
        W = B.b.w
        W.direct_box.box:SetText({ s = a })
        M.click(W.connect.button)
        M.run(100)
    end
    check(T.eq(B.direct, { "10.0.0.6:7777", "10.0.0.5:7777", "10.0.0.4:7777", "10.0.0.3:7777", "10.0.0.2:7777" }),
        tag .. ": history keeps the last 5, newest first (" .. T.repr(B.direct) .. ")")
    B = enter_browser()
    W = B.b.w
    local labels = T.map(W.recent, function(c) return c.label.last end)
    check(T.eq(labels, B.direct) and T.all(W.recent, function(c) return truthy(c.shown) end), tag .. ": 5 chips with the addresses (" .. T.repr(labels) .. ")")
    layout_checks(tag .. " browser chips", 1920, 1080)
    B.joining_until = 0
    HSMP_MENU_TEST.lobby.active = false   -- the six joins above opened a lobby (held: a new HOST / JOIN would ask first)
    local n0 = #M.execs
    M.click(W.recent[3].button)
    local joined = {}
    for i = n0 + 1, #M.execs do if contains(M.execs[i], "hsmp-sidecar") then joined[#joined + 1] = M.execs[i] end end
    check(#joined == 1 and contains(joined[1], "--server 10.0.0.4:7777"), tag .. ": one click on a chip connects to it")
    check(B.direct[1] == "10.0.0.4:7777" and #B.direct == 5, tag .. ": the used address moves to the front")
    local prefs = read(".browser_prefs.txt") or ""
    check(contains(prefs, "direct1=10.0.0.4:7777") and contains(prefs, "direct5=") and not contains(prefs, "direct6="),
        tag .. ": history persisted (5 entries)")
end

-- ============================================================================================
-- Polish: layout matrix (4 resolutions x DPI), keyboard / gamepad navigation and
-- focus traversal, the text guard, SETTINGS v2, LOBBY v2 states, leave / quit lifecycle, perf.
-- ============================================================================================

-- Every visible focusable control is reachable with the arrow keys from the
-- entry focus, TAB visits every section, and the entry focus exists.
local function nav_checks(tg)
    local Kit = kit()
    local b = Kit.cur
    local foc = {}
    for _, r in ipairs(b.focus) do
        if r.shown ~= false and not r.skip_focus and (not b.modal or r.modal == b.modal) then foc[#foc + 1] = r end
    end
    check(#foc > 0, tg .. ": has focusable controls")
    check(b.focused ~= nil, tg .. ": a control has the focus on entry")
    if not b.focused then return end
    local start = b.focused
    local seen, queue = { [start] = true }, { start }
    while #queue > 0 do
        local f = table.remove(queue, 1)
        for _, d in ipairs({ "up", "down", "left", "right" }) do
            Kit.set_focus(f, "key")
            Kit.nav(d)
            local g = Kit.focused()
            if g and not seen[g] then seen[g] = true; queue[#queue + 1] = g end
        end
    end
    local missing = {}
    for _, r in ipairs(foc) do if not seen[r] then missing[#missing + 1] = r.fkey end end
    check(#missing == 0, string.format("%s: every control reachable with the arrows (%d/%d, missing %s)", tg,
        #foc - #missing, #foc, T.repr({ missing[1], missing[2], missing[3] })))
    local groups, ng = {}, 0
    for _, r in ipairs(foc) do if not groups[r.fgroup] then groups[r.fgroup] = true; ng = ng + 1 end end
    Kit.set_focus(start, "key")
    local visited, nv = { [start.fgroup] = true }, 1
    for _ = 1, ng + 1 do
        Kit.cycle_group(1)
        local f = Kit.focused()
        if f and not visited[f.fgroup] then visited[f.fgroup] = true; nv = nv + 1 end
    end
    check(nv == ng, string.format("%s: TAB visits every section (%d/%d)", tg, nv, ng))
    Kit.set_focus(start, "key")
end

local function some_servers(n)
    local out = {}
    for i = 1, n do
        out[#out + 1] = { host = "10.0.0." .. i, port = 7777, name = string.format("Server %02d", i), map = "Map_Arena_Pit",
                          mode = "duel", players = i % 4, max = 8, pwd = (i % 5 == 0), proto = 0, version = "1",
                          region = (i % 2 == 0) and "EU" or "NA", ping = 20 + i * 7, source = "master", live = true }
    end
    return out
end

-- Every screen and state at one (resolution, DPI): layout bounds, no overlap,
-- no clipped label, fonts >= 9 physical px, and full keyboard reachability.
local function test_matrix()
    local vw, vh, dpi = opts.vw, opts.vh, opts.dpi
    local eff = dpi or (vh / 1080)
    local tag = string.format("%dx%d dpi %s", vw, vh, dpi and tostring(dpi) or "auto")
    boot(vw, vh, dpi)
    local cw, ch = vw / eff, vh / eff
    kit_remote(2, "knight", "w_longsword3", "")
    write(".metrics.json", '{"rtt_ms":42,"srtt_ms":41.5}\n')
    host_lobby()
    local Kit = kit()
    local w = Kit.cur.w
    layout_checks(tag .. " lobby", cw, ch, eff, true)
    nav_checks(tag .. " lobby")
    -- host tools on a selected row, then the inline confirm strip
    Kit.set_focus(w.rows[2].btn, "key"); Kit.key("accept", "kb")
    check(truthy(w.rows[2].kick.shown) and truthy(w.rows[2].promote.shown), tag .. ": selecting a row shows its host tools")
    layout_checks(tag .. " lobby host tools", cw, ch, eff, true)
    M.click(w.rows[2].kick.button)
    check(Kit.confirm_open(), tag .. ": KICK asks to confirm")
    layout_checks(tag .. " lobby confirm", cw, ch, eff, true)
    nav_checks(tag .. " lobby confirm")
    Kit.key("back", "kb")
    check(not Kit.confirm_open(), tag .. ": ESC closes the confirm strip")
    -- loadout + host rules
    M.click(w.loadout.button)
    layout_checks(tag .. " loadout", cw, ch, eff, true)
    nav_checks(tag .. " loadout")
    M.click(Kit.cur.w.host_btn.button)
    layout_checks(tag .. " loadout rules", cw, ch, eff, true)
    nav_checks(tag .. " loadout rules")
    -- browser with a full page, then empty
    exit_screen()
    enter_screen("browser")
    local B = package.loaded["browser"]
    B._accept_result({ status = "ok", servers = some_servers(18), done = true })
    B.render()
    layout_checks(tag .. " browser", cw, ch, eff, true)
    nav_checks(tag .. " browser")
    B._accept_result({ status = "ok", servers = {}, done = true })
    B.render()
    layout_checks(tag .. " browser empty", cw, ch, eff, true)
    exit_screen()
    -- settings (with validation errors showing) + character
    enter_screen("settings")
    layout_checks(tag .. " settings", cw, ch, eff, true)
    nav_checks(tag .. " settings")
    M.set(Kit.cur.w.nick.box, "text", "x")
    M.set(Kit.cur.w.port.box, "text", "80")
    M.set(Kit.cur.w.master.box, "text", "not a url")
    M.run(300)
    layout_checks(tag .. " settings errors", cw, ch, eff, true)
    exit_screen()
    enter_screen("character")
    layout_checks(tag .. " character", cw, ch, eff, true)
    nav_checks(tag .. " character")
    exit_screen()
    local errs = T.filter(T.map(M.logs, tostring), function(l) return contains(l, "ERR") or contains(l:lower(), "error") end)
    check(#errs == 0, tag .. ": no loop/tick errors (" .. T.repr({ errs[1], errs[2] }) .. ")")
end

local nav = {}
-- top-level ribbons, SETTINGS by keyboard only: text guard, live validation, confirm, save
nav[1] = function(tag)
    boot(1920, 1080, 1.0)
    write(".settings.json", '{"nick":"Willie","server":"127.0.0.1:7777","send_hz":60,"pose_stiffness":1.5,"interact_x":3}\n')
    local S = package.loaded["settings"]
    S.load()
    local st = HSMP_MENU_TEST.state
    M.press("DOWN_ARROW"); M.press("RIGHT_ARROW"); M.press("RETURN")
    check(st.top_focus == nil and kit().cur == nil, tag .. ": arrows / Enter stay with the native menu until TAB")
    M.press("TAB")
    check(st.top_focus == 1 and rawget(st.top_ring[1].button, "vis") == 3, tag .. ": TAB focuses HOST GAME (ring shown)")
    local nb
    for _, c in ipairs(rawget(M.canvas, "__children")) do if rawget(c, "__name") == "Button_1" then nb = c end end
    M.set(nb, "hovered", true); M.run(80); M.set(nb, "hovered", false)
    check(st.top_focus == nil and rawget(st.top_ring[1].button, "vis") == 1, tag .. ": the mouse on a native button drops the column focus")
    M.press("TAB")
    M.press("UP_ARROW")
    check(st.top_focus == 5, tag .. ": UP wraps to QUIT MP")
    M.press("DOWN_ARROW"); M.press("DOWN_ARROW"); M.press("DOWN_ARROW")
    check(st.top_focus == 3, tag .. ": DOWN walks the column (" .. tostring(st.top_focus) .. ")")
    M.press("RETURN")
    local Kit = kit()
    check(Kit.cur and Kit.cur.screen == "settings", tag .. ": ENTER opens SETTINGS")
    local w = Kit.cur.w
    check(Kit.focused() == w.nick, tag .. ": the nickname field has the focus on entry")
    check(contains(Kit.cur.L.hint_keys.last or "", "[ENTER] SELECT") and contains(Kit.cur.L.hint_keys.last or "", "[ESC] BACK"),
        tag .. ": the footer shows the keys (" .. tostring(Kit.cur.L.hint_keys.last) .. ")")
    check(contains(Kit.cur.L.hint_help.last or "", "kill feed"), tag .. ": the footer shows the focused field's help")
    -- typing: Enter starts, the text guard swallows Q/E/arrows, Enter commits
    M.press("RETURN")
    check(truthy(rawget(w.nick.box, "kbfocus")) and Kit.typing() == w.nick, tag .. ": ENTER starts typing into the field")
    check(contains(Kit.cur.L.hint_keys.last or "", "[ENTER] DONE"), tag .. ": typing hints shown")
    M.press("E"); M.press("Q"); M.press("DOWN_ARROW"); M.press("F5")
    check(Kit.focused() == w.nick and Kit.cur.screen == "settings", tag .. ": while typing, keys go to the text box only")
    M.set(w.nick.box, "text", "x")
    M.run(300)
    check(w.nick_note.last:find("Too short") ~= nil and rawget(w.nick.frame.w, "brush") == Kit.C.bad,
        tag .. ": live validation: too short, red frame (" .. tostring(w.nick_note.last) .. ")")
    M.press("RETURN")
    check(not truthy(rawget(w.nick.box, "kbfocus")) and Kit.typing() == nil, tag .. ": ENTER ends typing")
    check(truthy(w.save.disabled) and contains(Kit.reason_of(w.save) or "", "NICKNAME"), tag .. ": SAVE greyed with the reason")
    -- TAB while typing: done + the next field of the form
    Kit.set_focus(w.nick, "key"); M.press("RETURN")
    M.press("TAB")
    check(Kit.typing() == nil and Kit.focused() == w.region.chips[1], tag .. ": TAB while typing goes to the next field (REGION)")
    M.set(w.nick.box, "text", "Sir Bors_2")
    M.set(w.master.box, "text", "https://a.example:7778/, http://b.example")
    M.run(300)
    check(not truthy(w.save.disabled), tag .. ": valid fields enable SAVE")
    -- ESC with changes: inline confirm, focus on KEEP EDITING; ESC again closes it
    M.press("ESCAPE")
    check(Kit.confirm_open() and Kit.focused() == Kit.cur.cstrip.no, tag .. ": ESC with changes asks (focus on KEEP EDITING)")
    M.press("ESCAPE")
    check(not Kit.confirm_open() and Kit.cur.screen == "settings", tag .. ": ESC closes the confirm")
    -- region chip by keyboard, then SAVE via the actions
    Kit.set_focus(w.region.by_key["EU"], "key"); M.press("RETURN")
    check(truthy(w.region.by_key["EU"].on), tag .. ": ENTER picks a chip")
    Kit.set_focus(w.save, "key"); M.press("RETURN")
    check(Kit.cur == nil and st.top_focus == 3, tag .. ": SAVE & BACK returns to the menu with SETTINGS focused")
    local line = strip(read(".settings.json") or "")
    local t = T.json_decode(line) or {}
    check(t.nick == "Sir Bors_2" and t.region == "EU" and t.master_url == "https://a.example:7778, http://b.example",
        tag .. ": saved nick / region / normalised server lists (" .. line .. ")")
    check(t.pose_stiffness == 1.5 and t.interact_x == 3, tag .. ": other mods' keys survive a save (M18)")
    check(not contains(line, "\n") and T.count(read(".settings.json"), "\n") == 1, tag .. ": one line")
    -- the browser now asks the saved lists first
    local B = package.loaded["browser"]
    local urls = B._master_urls()
    check(urls[1] == "https://a.example:7778" and urls[2] == "http://b.example", tag .. ": the browser uses the saved lists (" .. T.repr(urls) .. ")")
end
-- browser by keyboard: rows select on focus, ENTER joins, PgUp/PgDn, F5, Shift+TAB, ESC -> ribbon
nav[2] = function(tag)
    boot(1920, 1080, 1.0)
    enter_screen("browser")
    local Kit, B = kit(), package.loaded["browser"]
    B._accept_result({ status = "ok", servers = some_servers(18), done = true })
    B.q.inflight = false
    B.render()
    local W = B.b.w
    Kit.set_focus(W.rows[1].btn, "key")
    M.press("DOWN_ARROW")
    check(Kit.focused() == W.rows[2].btn and B.sel == W.rows[2].server_key, tag .. ": DOWN moves to row 2 and selects it")
    M.press("PAGE_DOWN")
    check(B.page == 2 and W.pager.label.last == "PAGE 2 / 2", tag .. ": PgDn pages")
    M.press("PAGE_UP")
    check(B.page == 1, tag .. ": PgUp pages back")
    local n0 = #M.execs
    M.press("F5")
    check(B.q.inflight and #M.execs > n0, tag .. ": F5 refreshes")
    local g0 = Kit.focused().fgroup
    M.press("TAB", { "SHIFT" })
    local g1 = Kit.focused().fgroup
    M.press("TAB")
    check(g1 ~= g0 and Kit.focused().fgroup == g0, tag .. ": Shift+TAB goes back one section, TAB forward (counted once)")
    Kit.set_focus(W.rows[3].btn, "key")
    B.joining_until = 0
    n0 = #M.execs
    M.press("RETURN")
    local joined = T.filter({ table.unpack(M.execs, n0 + 1) }, function(e) return contains(tostring(e), "hsmp-sidecar") end)
    check(#joined == 1 and contains(joined[1], "--server " .. W.rows[3].server_key), tag .. ": ENTER on a row joins it")
    M.run(1500)
    check(kit().cur and kit().cur.screen == "lobby", tag .. ": the lobby opens")
    exit_screen()
    enter_screen("browser")
    M.press("ESCAPE")
    check(kit().cur == nil and HSMP_MENU_TEST.state.top_focus == 2, tag .. ": ESC leaves with SERVER BROWSER focused")
    -- the search box: typing never moves the focus or pages
    enter_screen("browser")
    W = B.b.w
    Kit = kit()
    Kit.set_focus(W.search_box, "key"); M.press("RETURN")
    M.set(W.search_box.box, "text", "Server 1")
    M.press("PAGE_DOWN"); M.press("DOWN_ARROW")
    M.run(400)
    check(Kit.focused() == W.search_box and B.page == 1 and B.f.search == "Server 1", tag .. ": search typing is guarded and filters")
    M.press("ESCAPE")
    check(Kit.cur and Kit.cur.screen == "browser" and Kit.typing() == nil, tag .. ": ESC while typing only leaves the box")
end
-- lobby by keyboard: host tools on a row, KICK with confirm, MAKE HOST, accepted results
nav[3] = function(tag)
    boot(1920, 1080, 1.0)
    host_lobby()
    local Kit = kit()
    local w = Kit.cur.w
    check(Kit.focused() == w.ready, tag .. ": MARK READY has the focus on entry")
    M.press("UP_ARROW")
    check(Kit.focused() == w.rows[2].btn, tag .. ": UP from READY reaches the last player row")
    M.press("RETURN")
    check(HSMP_MENU_TEST.lobby.sel_peer == 2 and truthy(w.rows[2].kick.shown), tag .. ": ENTER selects the player (host tools shown)")
    M.press("RIGHT_ARROW")
    check(Kit.focused() == w.rows[2].promote, tag .. ": RIGHT reaches MAKE HOST")
    M.press("RIGHT_ARROW")
    check(Kit.focused() == w.rows[2].kick, tag .. ": RIGHT reaches KICK")
    clear_outbox()
    M.press("RETURN")
    check(Kit.confirm_open() and Kit.focused() == Kit.cur.cstrip.no and contains(Kit.cur.cstrip.text.last, "Kick Mate"),
        tag .. ": KICK asks first, CANCEL focused")
    M.press("DOWN_ARROW"); M.press("UP_ARROW")
    check(Kit.focused() == Kit.cur.cstrip.no or Kit.focused() == Kit.cur.cstrip.yes, tag .. ": focus stays in the confirm strip")
    M.press("LEFT_ARROW")
    check(Kit.focused() == Kit.cur.cstrip.yes, tag .. ": LEFT reaches the KICK confirm")
    M.press("RETURN")
    check(ob_has({ verb = "kick:2", cmd_id = true }) and not Kit.confirm_open(), tag .. ": confirmed KICK is a kick command")
    check(contains(w.note.last, "KICK MATE: WAITING FOR SERVER"), tag .. ": pending note (" .. tostring(w.note.last) .. ")")
    write(".sidecar.json", '{"status":"connected","peer_id":1,"is_admin":true,"peers":[{"id":1,"nick":"Willie"}]}\n')
    M.run(1100)
    check(contains(w.note.last, "KICK MATE: ACCEPTED"), tag .. ": the player left -> accepted (" .. tostring(w.note.last) .. ")")
    check(w.start.label.last == "START (SOLO)" and not truthy(w.start.disabled), tag .. ": alone -> START (SOLO) enabled")
    -- MAKE HOST: confirm, command, accepted from the roster; we lose the host tools
    write(".sidecar.json", SIDECAR_2)
    M.run(600)
    Kit.set_focus(w.rows[2].btn, "key"); M.press("RETURN")
    M.click(w.rows[2].promote.button)
    M.click(Kit.cur.cstrip.yes.button)
    check(ob_has({ verb = "promote:2", cmd_id = true }), tag .. ": MAKE HOST is a promote command")
    write(".session.json", '{"v":1,"roster":[{"peer_id":1,"admin_role":"none","connected":true},{"peer_id":2,"admin_role":"admin","connected":true}],'
        .. '"config":{"mode":"duel","kit_rules":{"mode_code":0,"budget":30}}}\n')
    write(".sidecar.json", '{"status":"connected","peer_id":1,"is_admin":false,"peers":[{"id":1,"nick":"Willie"},{"id":2,"nick":"Mate"}]}\n')
    M.run(1100)
    check(contains(w.note.last, "MAKE HOST MATE: ACCEPTED"), tag .. ": promote accepted (" .. tostring(w.note.last) .. ")")
    check(truthy(w.maps.by_key["Map_Arena_Pit"].disabled) and truthy(w.start.disabled) and contains(w.rows[2].name.last, "HOST"),
        tag .. ": after MAKE HOST the controls are greyed and Mate is HOST")
    M.click(w.maps.by_key["Map_Arena_Pit"].button)
    check(contains(Kit.cur.L.hint_help.last or "", "Only the host"), tag .. ": a greyed tile says why (" .. tostring(Kit.cur.L.hint_help.last) .. ")")
end
-- gamepad: D-pad moves (with hold-repeat), A activates, B backs out, pad hint labels; a failing poll disables itself
nav[4] = function(tag)
    boot(1920, 1080, 1.0)
    enter_screen("character")
    local Kit = kit()
    local w = Kit.cur.w
    local f0 = Kit.focused()
    M.pad("Gamepad_DPad_Right", true); M.run(80); M.pad("Gamepad_DPad_Right", false); M.run(80)
    check(Kit.focused() ~= f0 and Kit.device == "pad", tag .. ": D-pad RIGHT moves the focus")
    check(contains(Kit.cur.L.hint_keys.last or "", "[A] SELECT") and contains(Kit.cur.L.hint_keys.last or "", "[D-PAD] MOVE"),
        tag .. ": hints switch to the pad (" .. tostring(Kit.cur.L.hint_keys.last) .. ")")
    Kit.set_focus(w.str.by_key[0], "key")
    M.pad("Gamepad_DPad_Right", true); M.run(800); M.pad("Gamepad_DPad_Right", false); M.run(80)
    local v = Kit.focused() and Kit.focused().label and tonumber(Kit.focused().label.last)
    check(v and v >= 3, tag .. ": holding the D-pad repeats (reached " .. tostring(v) .. ")")
    M.pad("Gamepad_FaceButton_Bottom", true); M.run(80); M.pad("Gamepad_FaceButton_Bottom", false); M.run(80)
    check(truthy(w.str.by_key[v].on), tag .. ": A activates the focused chip")
    M.pad("Gamepad_FaceButton_Right", true); M.run(80); M.pad("Gamepad_FaceButton_Right", false); M.run(80)
    check(Kit.confirm_open(), tag .. ": B = back (asks: unsaved change)")
    M.pad("Gamepad_FaceButton_Right", true); M.run(80); M.pad("Gamepad_FaceButton_Right", false); M.run(80)
    check(not Kit.confirm_open(), tag .. ": B closes the confirm")
    M.pad_error = true
    M.run(400)
    check(HSMP_MENU_TEST.pad.on == false and contains(logtext(), "gamepad poll disabled"), tag .. ": a failing FKey call disables the pad poll")
    M.pad_error = nil
end
-- mouse: hover moves the focus (one visual state); a press flashes; a greyed click explains itself
nav[5] = function(tag)
    boot(1920, 1080, 1.0)
    host_lobby()
    local Kit = kit()
    local w = Kit.cur.w
    M.set(w.loadout.button, "hovered", true); M.run(120)
    check(Kit.focused() == w.loadout and w.loadout.painted == "h", tag .. ": hover moves the focus + highlight")
    M.set(w.loadout.button, "hovered", false); M.run(120)
    Kit.set_focus(w.modes.by_key[5], "key")
    Kit.key("accept", "kb")
    check(w.modes.by_key[5].painted == "pendh" or w.modes.by_key[5].painted == "onh", tag .. ": ENTER flashes the pressed control (" .. tostring(w.modes.by_key[5].painted) .. ")")
    M.run(300)
    check(w.modes.by_key[5].painted == "pend", tag .. ": the flash ends; the chip shows pending")
    M.click(w.start.button)
    check(contains(Kit.cur.L.hint_help.last or "", "READY"), tag .. ": a greyed START click explains why in the footer")
end

local settings_t = {}
settings_t[1] = function(tag)
    boot(1920, 1080, 1.0)
    local S = package.loaded["settings"]
    local cases = {
        { "Willie", true }, { "W", false }, { "  ", false }, { "Sir Lancelot", true }, { "a  b", false },
        { "_lead", false }, { "abcdefghijklmnopqrstu", false }, { "Mate.2-x", true }, { 'Bad"Quote', false },
        { "Bob&calc", false }, { "Jose\195\169", false },
    }
    for _, c in ipairs(cases) do
        local ok, why = S.nick_check(c[1])
        check(ok == c[2], string.format("%s: nick %q -> %s (%s)", tag, c[1], tostring(c[2]), tostring(why)))
    end
    for _, c in ipairs({ { "7777", true }, { "80", false }, { "70000", false }, { "77a", false }, { "", false } }) do
        check((S.port_check(c[1])) == c[2], string.format("%s: port %q -> %s", tag, c[1], tostring(c[2])))
    end
    local ok, _, list = S.urls_check(" https://a.example/ , http://b.example:7778,https://a.example ")
    check(ok and #list == 2 and list[1] == "https://a.example" and list[2] == "http://b.example:7778", tag .. ": urls normalised + deduplicated")
    check(not (S.urls_check("https://a.example, calc.exe")), tag .. ": a non-URL entry is refused")
    check(not (S.urls_check("https://a & calc")), tag .. ": an unsafe URL is refused")
    check((S.urls_check("")), tag .. ": empty = default list")
    -- A hand-edited file cannot put a command into os.execute
    write(".settings.json", '{"nick":"x\\" & calc & \\"","server":"1.2.3.4:7777 & calc","send_hz":77,"region":"MARS","hud_net":"maybe"}\n')
    S.load()
    local d = S.data
    check(d.nick == "Willie" and d.server == "127.0.0.1:7777" and d.send_hz == 60 and d.region == "" and d.hud_net == "always",
        tag .. ": unsafe / unknown values fall back to defaults (" .. T.repr({ d.nick, d.server, d.send_hz, d.region, d.hud_net }) .. ")")
    -- legacy (pattern-written) file still loads
    write(".settings.json", '{"nick":"Old","server":"127.0.0.1:7790","send_hz":90,"hud":false,"avatars":true,"lobby_map":"Map_Arena_Pit","lobby_mode":"Best of 5"}\n')
    S.load()
    check(d.nick == "Old" and d.send_hz == 90 and d.hud == false and d.lobby_map == "Map_Arena_Pit", tag .. ": an old file loads")
    -- A key hand-edited in game after load survives the next save
    local raw = read(".settings.json"):gsub("}%s*$", ',"pose_stiffness":1.5}\n')
    write(".settings.json", raw)
    d.nick = "Saved"
    S.save()
    local after = T.json_decode(read(".settings.json") or "{}")
    check(after.nick == "Saved" and after.pose_stiffness == 1.5, tag .. ": save keeps keys added since load (" .. tostring(read(".settings.json")) .. ")")
end
settings_t[2] = function(tag)
    -- region + port reach the hosted server; the TEST button reports per URL
    boot(1920, 1080, 1.0)
    write(".settings.json", '{"nick":"Host","server":"127.0.0.1:7790","region":"EU","master_url":"https://m1.example, https://m2.example"}\n')
    package.loaded["settings"].load()
    M.execs = {}
    host_lobby()
    local ex = execs()
    check(contains(ex, " HSMP_REGION=EU") and contains(ex, "--bind 0.0.0.0:7790") and contains(ex, "--server 127.0.0.1:7790"),
        tag .. ": region + port used when hosting")
    check(contains(ex, "HSMP_MASTER_URL=https://m1.example") and contains(ex, " HSMP_LISTEN_HOST=1"),
        tag .. ": the saved primary server list registers the game; listen host flag set")
    exit_screen()
    enter_screen("settings")
    local Kit = kit()
    local w = Kit.cur.w
    -- TEST: one captured curl.exe per list (IPC.spawn_capture; no state-dir body / flag files)
    local n0 = #CAP.captures("curl")
    M.click(w.test.button)
    local curls = T.filter(CAP.captures("curl"), function(e) return contains(CAP.argline(e), "/v1/servers") end)
    check(#curls - n0 == 2 and CAP.argline(curls[#curls - 1]) == "-s -f -m 5 https://m1.example/v1/servers",
        tag .. ": TEST asks both server lists (" .. #curls - n0 .. ")")
    check(w.master_note.spin.active and contains(w.master_note.last, "Testing 2 server lists"), tag .. ": spinner while testing")
    CAP.NP().sc_capture_done(curls[#curls - 1].pid, 0, '[{"host":"1.2.3.4","port":7777},{"host":"5.6.7.8","port":7777}]')
    CAP.NP().sc_capture_done(curls[#curls].pid, 22, "")
    M.run(400)
    check(not w.master_note.spin.active and contains(w.master_note.last, "m1.example OK (2 servers)")
        and contains(w.master_note.last, "m2.example NO ANSWER"), tag .. ": per-URL result (" .. tostring(w.master_note.last) .. ")")
    check(not T.exists(sd .. "/.settings_test1.body") and not T.exists(sd .. "/.settings_test1.flag"), tag .. ": no test files in the state dir")
    -- a list that never answers times out (no endless spinner) and its curl is stopped by handle
    M.click(w.test.button)
    local pending = CAP.last("curl")
    M.run(8500)
    check(not w.master_note.spin.active and contains(w.master_note.last, "No answer from"), tag .. ": TEST times out with advice")
    check(T.any(CAP.NP()._proc.killed, function(p) return p == pending.pid end), tag .. ": the late curl is killed by handle")
end

local lobby_t = {}
-- joiner: everything host-only is greyed with a reason; LEAVE needs no confirm
lobby_t[1] = function(tag)
    boot(1920, 1080, 1.0)
    enter_screen("browser")
    local B = package.loaded["browser"]
    B._accept_result({ status = "ok", servers = some_servers(3), done = true })
    B.render()
    B.joining_until = 0
    B.sel = "10.0.0.2:7777"
    B.render()
    M.click(B.b.w.join.button)
    M.run(1500)
    write(".sidecar.json", '{"status":"connected","peer_id":2,"is_admin":false,"peers":[{"id":1,"nick":"Host"},{"id":2,"nick":"Me"}]}\n')
    write(".match.json", match_json(nil, "Map_Arena_Pit", 3, { 1 }))
    M.run(1100)
    local Kit = kit()
    local w = Kit.cur.w
    check(contains(Kit.cur.L.title.last, "Server 02"), tag .. ": the title names the server (" .. tostring(Kit.cur.L.title.last) .. ")")
    check(T.all(w.maps.chips, function(c) return truthy(c.disabled) end) and truthy(w.modes.by_key[3].disabled)
        and truthy(w.kitmode.by_key[0].disabled), tag .. ": every match setting is greyed for a joiner")
    check(truthy(w.maps.by_key["Map_Arena_Pit"].on), tag .. ": the server's arena is shown")
    check(w.start.label.last == "HOST STARTS (1/2)" and truthy(w.start.disabled), tag .. ": START shows the host waits (" .. tostring(w.start.label.last) .. ")")
    check(w.cancel.label.last == "LEAVE" and contains(w.note.last, "MARK READY"), tag .. ": LEAVE + the joiner's hint")
    M.click(w.cancel.button)
    check(not Kit.confirm_open() and kit().cur == nil, tag .. ": LEAVE leaves at once (no confirm for a joiner)")
end
-- connecting: spinner, then a timeout that says what to do; reconnecting shown
lobby_t[2] = function(tag)
    boot(1920, 1080, 1.0)
    local rib = T.filter(M.live(), function(w) return startswith(tostring(w.name), "HSMPBtn_top1") end)
    M.click(rib[1].obj)
    M.run(1600)
    local Kit = kit()
    local L = Kit.cur.L
    check(L.status.spin.active and contains(L.status.last, "CONNECTING"), tag .. ": CONNECTING with a spinner")
    check(truthy(Kit.cur.w.ready.disabled), tag .. ": READY greyed until connected")
    M.run(15500)
    check(contains(L.msg.last, "Cannot reach the server") and contains(L.msg.last, "LEAVE"), tag .. ": the timeout says what to do (" .. tostring(L.msg.last) .. ")")
    write(".sidecar.json", '{"status":"reconnecting","peer_id":1,"is_admin":true,"peers":[{"id":1,"nick":"Willie"}]}\n')
    M.run(600)
    check(L.status.spin.active and contains(L.status.last, "RECONNECTING"), tag .. ": RECONNECTING shown")
    write(".sidecar.json", '{"status":"connected","peer_id":1,"is_admin":true,"peers":[{"id":1,"nick":"Willie"}]}\n')
    write(".metrics.json", '{"rtt_ms":38,"srtt_ms":37.6}\n')
    M.run(600)
    check(not L.status.spin.active and L.status.last == "CONNECTED   38 MS", tag .. ": CONNECTED with the ping (" .. tostring(L.status.last) .. ")")
end
-- session over: sidecar ended / a latched conn_state of THIS session leaves the lobby; a stale one does not
lobby_t[3] = function(tag)
    boot(1920, 1080, 1.0)
    host_lobby()
    write(".conn_state.json", string.format('{"v":1,"state":"lost","reason":"kicked","latched":true,"wall":%d}\n', os.time() - 600))
    M.run(1100)
    check(kit().cur ~= nil and kit().cur.screen == "lobby", tag .. ": a latched conn_state from an earlier session is ignored")
    -- A terminal loss (server closed): the lobby stays while the HUD
    -- modal is latched (the reason stays readable); nothing is killed.
    M.execs = {}
    write(".sidecar.json", '{"status":"server_closed","peer_id":1,"peers":[]}\n')
    write(".conn_state.json", string.format('{"v":1,"state":"lost","reason":"server_closed","latched":true,"actions":["menu"],"wall":%d}\n', os.time()))
    M.run(3000)
    check(kit().cur ~= nil and kit().cur.screen == "lobby" and #kills() == 0, tag .. ": a latched terminal loss keeps the lobby (modal readable), kills nothing")
    -- the player dismisses the modal (the Director clears the latch): graceful leave
    local had_lr = false
    local _rm = os.remove
    os.remove = function(p) if contains(tostring(p), ".leave_request.json") and T.exists(p) then had_lr = true end; return _rm(p) end
    write(".conn_state.json", string.format('{"v":1,"state":"ok","latched":false,"actions":[],"wall":%d}\n', os.time()))
    M.run(600)
    check(kit().cur == nil and contains(logtext(), "session over (server_closed)"), tag .. ": the dismissed terminal loss leaves the lobby")
    check(exists_(".leave_request.json") and #kills() == 0, tag .. ": graceful: a leave request first, no kill yet")
    M.run(1500)
    os.remove = _rm
    -- (in shared memory an unconsumed leave message is dropped when the
    -- next sidecar attaches; there is no file to sweep)
    check(killed("hsmp-sidecar.exe"), tag .. ": stopped after the leave wait (" .. T.repr(kill_calls()) .. ")")
    -- a leftover "ended" file from a crashed session does not close a new lobby
    boot(1920, 1080, 1.0)
    write(".sidecar.json", '{"status":"ended","peer_id":1,"peers":[]}\n')
    local rib = T.filter(M.live(), function(w) return startswith(tostring(w.name), "HSMPBtn_top1") end)
    M.click(rib[1].obj)
    M.run(3000)
    check(kit().cur ~= nil and kit().cur.screen == "lobby", tag .. ": a stale 'ended' status is ignored until the new sidecar is live")
    write(".sidecar.json", SIDECAR_2)
    M.run(600)
    write(".sidecar.json", '{"status":"ended","peer_id":1,"peers":[]}\n')
    M.run(1100)
    check(kit().cur == nil and contains(logtext(), "session over (sidecar ended)"), tag .. ": sidecar status ended leaves the lobby")
end
-- A lost link that offers RECONNECT is never torn down (in the lobby or
-- on the main menu after the lost travel), and the poll does nothing in the arena.
lobby_t[5] = function(tag)
    boot(1920, 1080, 1.0)
    host_lobby()
    M.execs = {}
    write(".sidecar.json", '{"status":"reconnecting","peer_id":1,"is_admin":true,"peers":[{"id":1,"nick":"Willie"}]}\n')
    write(".conn_state.json", string.format('{"v":1,"state":"lost","reason":"link_lost","latched":true,"actions":["reconnect","menu"],"wall":%d}\n', os.time()))
    M.run(30000)
    check(kit().cur ~= nil and kit().cur.screen == "lobby", tag .. ": lost + RECONNECT offered: the lobby stays")
    check(#kills() == 0 and exists_(".sidecar.json") and not contains(logtext(), "session over"),
        tag .. ": the sidecar that would reconnect is never killed, .sidecar.json kept")
    check(contains(logtext(), "RECONNECT offered - the session is kept"), tag .. ": the hold is logged")
    -- RECONNECT: the Director clears the latch, the link comes back
    write(".conn_state.json", string.format('{"v":1,"state":"ok","latched":false,"actions":[],"wall":%d}\n', os.time()))
    write(".sidecar.json", SIDECAR_2)
    M.run(1100)
    check(kit().cur ~= nil and kit().cur.screen == "lobby" and #kills() == 0, tag .. ": reconnected: still in the lobby, nothing killed")
    -- lost travel to the main menu (screen gone), then RECONNECT -> back to the lobby screen
    exit_screen()
    write(".conn_state.json", string.format('{"v":1,"state":"lost","reason":"link_lost","latched":true,"actions":["reconnect","menu"],"wall":%d}\n', os.time()))
    M.run(1100)
    write(".conn_state.json", string.format('{"v":1,"state":"ok","latched":false,"actions":[],"wall":%d}\n', os.time()))
    M.run(1100)
    check(kit().cur ~= nil and kit().cur.screen == "lobby" and contains(logtext(), "session back after a lost link"),
        tag .. ": RECONNECT from the main menu reopens the lobby screen")
    -- in the arena (the menu UI is freed): a terminal loss is the Director's / HUD's
    M.premap()
    M.execs = {}
    write(".sidecar.json", '{"status":"kicked","peer_id":1,"peers":[]}\n')
    write(".conn_state.json", string.format('{"v":1,"state":"lost","reason":"kicked","latched":true,"actions":["menu"],"wall":%d}\n', os.time()))
    M.run(30000)
    check(#kills() == 0 and not contains(logtext(), "session over (kicked)") and exists_(".sidecar.json"),
        tag .. ": in the arena the lobby poll never tears the session down")
end
-- Control lines carry a line id and are held until connected
-- (the sidecar sweeps its outboxes at session start, before "connected").
lobby_t[6] = function(tag)
    boot(1920, 1080, 1.0)
    clear_outbox()
    HSMPNative.put("kit_rules_req", { seq = 1, mode = 2, budget = 12 })   -- an earlier session's host request
    local rib = T.filter(M.live(), function(w) return startswith(tostring(w.name), "HSMPBtn_top1") end)
    M.click(rib[1].obj)
    M.run(2600)
    check(not contains(req_text(), '"mode"'),
        tag .. ": a new session clears the old host rules request (kit_rules_req republished with seq 0)")
    check(outbox() == "", tag .. ": nothing written to the control outbox before the session is connected (" .. outbox() .. ")")
    write(".sidecar.json", SIDECAR_2)
    M.run(1100)
    local lines = T.lines(outbox())
    check(#lines >= 1 and T.all(lines, function(l)
            local t = T.json_decode(l)
            return type(t) == "table" and (tonumber(t.cmd_id) or 0) > 0
        end),
        tag .. ": held commands flushed after connect, each a typed command with a cmd_id (" .. outbox() .. ")")
    check(T.any(lines, function(l) return contains(l, '"set_map"') or contains(l, "pick_arena") end),
        tag .. ": the host's boot-arena pick is among them")
end
-- A password server is not joined (no password entry yet): a reason, no sidecar
lobby_t[7] = function(tag)
    boot(1920, 1080, 1.0)
    enter_screen("browser")
    local B = package.loaded["browser"]
    B._accept_result({ status = "ok", servers = some_servers(5), done = true })
    B.render()
    B.joining_until = 0
    B.sel = "10.0.0.5:7777"
    B.render()
    M.execs = {}
    M.click(B.b.w.join.button)
    M.run(1500)
    check(not T.any(T.map(M.execs, tostring), function(e) return contains(e, "hsmp-sidecar") end)
        and contains(logtext(), "") and kit().cur and kit().cur.screen == "browser",
        tag .. ": no join to a password server (" .. T.repr(M.execs) .. ")")
end
-- travel only while connected; u64 epochs compared as strings; kit rules as a command
lobby_t[4] = function(tag)
    boot(1920, 1080, 1.0)
    host_lobby()
    write(".sidecar.json", '{"status":"reconnecting","peer_id":1,"is_admin":true,"peers":[{"id":1,"nick":"Willie"},{"id":2,"nick":"Mate"}]}\n')
    write(".match.json", match_json("countdown", "Map_Arena_Pit", nil, { 1, 2 }))
    M.run(1500)
    check(read(".travel_request.json") == nil, tag .. ": no travel request while the sidecar is reconnecting")
    write(".sidecar.json", SIDECAR_2)
    M.run(800)
    check(read(".travel_request.json") ~= nil, tag .. ": travel requested once connected")
    boot(1920, 1080, 1.0, { HSMP_AUTOTEST = "host" })
    write(".sidecar.json", '{"status":"connected","peer_id":1,"epoch":"18446744073709551615","peers":[{"id":1,"nick":"Willie"}]}\n')
    write(".match.json", match_json(nil, "Map_Arena_Alley", nil, { 1 }))
    M.run(3000)
    -- A u64 epoch above i64::MAX reaches Lua as a digit string (codec
    -- from_json), exactly; the Menu compares it as text.
    write(".sidecar.json", '{"status":"connected","peer_id":1,"epoch":"18446744073709551614","peers":[{"id":1,"nick":"Willie"}]}\n')
    M.run(1100)
    local lr = evs("lobby_ready")
    check(#lr == 2 and contains(lr[2].notice or "", "restarted"), tag .. ": u64 epochs differing in the last digit are a restart (" .. #lr .. ")")
    -- kit rules chips -> kit_rules command + the file for older sidecars; budget only for CUSTOM
    boot(1920, 1080, 1.0)
    host_lobby()
    local Kit = kit()
    local w = Kit.cur.w
    check(T.all(w.budget.chips, function(c) return truthy(c.disabled) end), tag .. ": budget greyed outside CUSTOM")
    clear_outbox()
    M.click(w.kitmode.by_key[2].button)
    check(ob_has({ cmd = "kit_rules", mode = 2, budget = 30, cmd_id = true }) and contains(req_text(), '"mode":2'),
        tag .. ": CUSTOM chip sends kit_rules (+ the kit_rules_req slot)")
    check(truthy(w.kitmode.by_key[2].pending) and not truthy(w.budget.by_key[45].disabled), tag .. ": pending; budget enabled for the requested CUSTOM")
    kit_rules(2, 30)
    M.run(1100)
    check(truthy(w.kitmode.by_key[2].on) and truthy(w.budget.by_key[30].on) and contains(w.note.last, "KIT RULES CUSTOM 30: ACCEPTED"),
        tag .. ": server echo -> accepted (" .. tostring(w.note.last) .. ")")
    clear_outbox()
    M.click(w.budget.by_key[45].button)
    check(ob_has({ mode = 2, budget = 45 }), tag .. ": budget chip sends CUSTOM 45")
end

-- HOST GAME / JOIN are not re-entrant, a held session is never replaced
-- by a new one (no /F of a reconnecting sidecar), and the top level says so.
lobby_t[8] = function(tag)
    boot(1920, 1080, 1.0)
    M.execs = {}
    local rib = T.filter(M.live(), function(w) return startswith(tostring(w.name), "HSMPBtn_top1") end)
    M.click(rib[1].obj)
    M.run(300)
    M.click(rib[1].obj)            -- double click while the lobby is still opening
    M.run(1600)
    local hosts = T.filter(T.map(M.execs, tostring), function(e) return contains(e, "--bind 0.0.0.0") end)
    check(#hosts == 1 and contains(logtext(), "HOST GAME: a session is already starting - ignored"),
        tag .. ": a double click on HOST GAME starts ONE server + sidecar (" .. #hosts .. ")")
    check(kit().cur and kit().cur.screen == "lobby", tag .. ": the lobby is up")
    -- the link is lost; the lost travel left the player on the main menu (session held for RECONNECT)
    local old_sc = spawned("hsmp-sidecar.exe")[1]
    write(".sidecar.json", '{"status":"reconnecting","peer_id":1,"is_admin":true,"peers":[{"id":1,"nick":"Willie"}]}\n')
    write(".conn_state.json", string.format('{"v":1,"state":"lost","reason":"link_lost","latched":true,"actions":["reconnect","menu"],"wall":%d}\n', os.time()))
    exit_screen()
    M.run(1100)
    local function host_label() local e = HSMP_MENU_TEST.state.ribbons[1]; return e and e.text and rawget(e.text, "text") end
    check(host_label() == "RECONNECTING...",
        tag .. ": the top level shows RECONNECTING... while the session is held (" .. tostring(host_label()) .. ")")
    M.execs = {}
    M.click(rib[1].obj)
    M.run(1200)
    check(#kills() == 0 and not contains(execs(), "--bind 0.0.0.0") and kit().cur and kit().cur.screen == "lobby"
        and contains(logtext(), "still held - back to its lobby"),
        tag .. ": HOST GAME with a held session goes back to it (no new session, no /F of the reconnecting sidecar)")
    -- JOIN from the browser while held: confirm, then a graceful leave first
    exit_screen()
    local B = enter_browser()
    M.execs = {}
    B.joining_until = 0
    B.b.w.direct_box.box:SetText({ s = "10.9.9.9:7777" })
    M.click(B.b.w.connect.button)
    check(kit().confirm_open() and #kills() == 0 and not contains(execs(), "10.9.9.9"),
        tag .. ": JOIN with a held session asks first (nothing killed, nothing started)")
    M.click(kit().cur.cstrip.yes.button)
    check(exists_(".leave_request.json") and #kills() == 0, tag .. ": confirmed: a leave request first")
    write(".sidecar.json", '{"status":"ended","peer_id":1,"peers":[]}\n')
    M.run(900)
    local ex = T.map(M.execs, tostring)
    local ki, ji
    for i, e in ipairs(ex) do
        if old_sc and contains(e, "proc_kill " .. old_sc.pid .. " ") then ki = ki or i end
        if contains(e, "--server 10.9.9.9:7777") then ji = ji or i end
    end
    check(ki and ji and ki < ji, tag .. ": the old sidecar ended and was cleaned up before the new JOIN started")
    M.run(1200)
    check(kit().cur and kit().cur.screen == "lobby", tag .. ": the new session's lobby opens")
end
-- A terminal status is not acted on before the Director's
-- modal latched it (the menu polls at 2 Hz, the Director at 4 Hz), and a
-- .conn_state.json caught mid-rename is not "the player dismissed it".
lobby_t[9] = function(tag)
    boot(1920, 1080, 1.0)
    local rib = T.filter(M.live(), function(w) return startswith(tostring(w.name), "HSMPBtn_top1") end)
    M.click(rib[1].obj)
    M.run(1600)
    -- a fast refusal: the sidecar never got to "connected"
    write(".sidecar.json", '{"status":"rejected","peer_id":0,"peers":[]}\n')
    M.execs = {}
    M.run(1100)   -- the menu sees "rejected" first: the Director has not latched yet
    check(kit().cur and kit().cur.screen == "lobby" and exists_(".sidecar.json") and #kills() == 0
        and not contains(logtext(), "session over"),
        tag .. ": a terminal status before the Director's latch tears nothing down (the modal can show)")
    write(".conn_state.json", string.format('{"v":1,"state":"lost","reason":"rejected","latched":true,"actions":["menu"],"wall":%d}\n', os.time()))
    M.run(1100)
    check(kit().cur and kit().cur.screen == "lobby" and #kills() == 0, tag .. ": latched: still held while the modal is up")
    -- The Director rewrites the file (remove-then-rename): one absent read is not a dismissal
    remove_(".conn_state.json")
    M.run(600)
    check(kit().cur and kit().cur.screen == "lobby" and not contains(logtext(), "session over"),
        tag .. ": a .conn_state.json missing for one read is not 'dismissed'")
    write(".conn_state.json", string.format('{"v":1,"state":"lost","reason":"rejected","latched":true,"actions":["menu"],"wall":%d}\n', os.time()))
    M.run(600)
    -- the player dismisses the modal
    write(".conn_state.json", string.format('{"v":1,"state":"ok","latched":false,"actions":[],"wall":%d}\n', os.time()))
    M.run(600)
    check(kit().cur == nil and contains(logtext(), "session over (rejected)"), tag .. ": dismissed -> the lobby closes")
    -- never latched (an old Director): the terminal status ends the lobby after LOST_HOLD_S
    boot(1920, 1080, 1.0)
    rib = T.filter(M.live(), function(w) return startswith(tostring(w.name), "HSMPBtn_top1") end)
    M.click(rib[1].obj)
    M.run(1600)
    write(".sidecar.json", '{"status":"kicked","peer_id":1,"peers":[]}\n')
    M.run(15000)
    check(kit().cur and kit().cur.screen == "lobby", tag .. ": unlatched terminal status: held for LOST_HOLD_S")
    M.run(6000)
    check(kit().cur == nil and contains(logtext(), "session over (kicked)"), tag .. ": ... then the lobby closes")
end

-- The host's saved ROUNDS / KIT RULES picks are re-sent once on connect
-- (only where the server differs), and a KIT RULES pick is saved.
lobby_t[10] = function(tag)
    boot(1920, 1080, 1.0)
    local st = HSMP_MENU_TEST.settings
    st.lobby_mode, st.host_kit_mode, st.host_kit_budget = "Best of 5", 2, 45
    clear_outbox()
    host_lobby()                 -- server: best_of 3, no kit rules known
    M.run(1100)
    local ob = outbox()
    check(contains(ob, "set_best_of:5") and ob_has({ cmd = "kit_rules", mode = 2, budget = 45 }),
        tag .. ": the saved BEST OF 5 and KIT RULES CUSTOM 45 are sent to our own server (" .. ob .. ")")
    -- the server applies them (its echo completes the commands)
    write(".match.json", match_json(nil, nil, 5))
    kit_rules(2, 45)
    M.run(600)
    clear_outbox()
    M.run(4000)
    check(not contains(outbox(), "set_best_of") and not contains(outbox(), '"cmd":"kit_rules"'), tag .. ": once per session (" .. outbox() .. ")")
    -- the server already has them: nothing is sent
    boot(1920, 1080, 1.0)
    st = HSMP_MENU_TEST.settings
    st.lobby_mode, st.host_kit_mode = "Best of 3", -1
    clear_outbox()
    host_lobby()
    M.run(1100)
    check(not contains(outbox(), "set_best_of") and not contains(outbox(), '"cmd":"kit_rules"'),
        tag .. ": nothing re-sent when the server matches / nothing saved (" .. outbox() .. ")")
    -- a KIT RULES chip pick is remembered
    local w = kit().cur.w
    M.click(w.kitmode.by_key[1].button)
    check(st.host_kit_mode == 1 and contains(read(".settings.json") or "", '"host_kit_mode":1'),
        tag .. ": the KIT RULES pick is saved for the next HOST (" .. tostring(read(".settings.json")) .. ")")
    -- a joiner never pushes its own picks
    boot(1920, 1080, 1.0, { HSMP_AUTOTEST = "join", HSMP_AUTOTEST_ADDR = "10.1.2.3:7777", HSMP_AUTOTEST_READY = "0" })
    HSMP_MENU_TEST.settings.lobby_mode = "Best of 7"
    clear_outbox()
    write(".sidecar.json", '{"status":"connected","peer_id":2,"is_admin":false,"peers":[{"id":1,"nick":"Willie"},{"id":2,"nick":"Mate"}]}\n')
    write(".match.json", match_json(nil, "Map_Arena_Pit", nil, {}))
    M.run(3000)
    check(not contains(outbox(), "set_best_of"), tag .. ": a joiner sends no BEST OF")
end

-- RECONNECT back into the lobby resets the travel latch; a
-- session that ends while LOADOUT is open closes it, and BACK opens no zombie lobby.
lobby_t[11] = function(tag)
    boot(1920, 1080, 1.0)
    host_lobby()
    local LB = HSMP_MENU_TEST.lobby
    LB.launched, LB.travel_note = true, "LOADING PIT..."   -- the countdown travel that the lost link interrupted
    write(".sidecar.json", '{"status":"reconnecting","peer_id":1,"is_admin":true,"peers":[{"id":1,"nick":"Willie"}]}\n')
    write(".conn_state.json", string.format('{"v":1,"state":"lost","reason":"link_lost","latched":true,"actions":["reconnect","menu"],"wall":%d}\n', os.time()))
    exit_screen()
    M.run(1100)
    write(".conn_state.json", string.format('{"v":1,"state":"ok","latched":false,"actions":[],"wall":%d}\n', os.time()))
    write(".sidecar.json", SIDECAR_2)
    M.run(1100)
    check(kit().cur and kit().cur.screen == "lobby" and LB.launched == false and LB.travel_note == nil and LB.ready_rearm ~= false,
        tag .. ": back in the lobby after RECONNECT with the travel latch and note reset (" .. T.repr({ LB.launched, LB.travel_note }) .. ")")
    -- LOADOUT open, the session ends
    enter_screen("classes")
    check(kit().cur and kit().cur.screen == "classes", tag .. ": LOADOUT open")
    write(".sidecar.json", '{"status":"ended","peer_id":1,"peers":[]}\n')
    M.run(1100)
    check(HSMP_MENU_TEST.state.screen_active == nil and contains(logtext(), "session over (sidecar ended)"),
        tag .. ": the session over closes LOADOUT too (" .. tostring(HSMP_MENU_TEST.state.screen_active) .. ")")
    enter_screen("lobby")
    check(HSMP_MENU_TEST.state.screen_active == nil and contains(logtext(), "enter screen lobby refused: no MP session"),
        tag .. ": no zombie lobby without a session")
    -- Enter on the focused HOST ribbon behind a latched modal does nothing
    write(".conn_state.json", string.format('{"v":1,"state":"lost","reason":"kicked","latched":true,"actions":["menu"],"wall":%d}\n', os.time()))
    M.run(600)
    M.execs = {}
    HSMP_MENU_TEST.state.top_focus = 1
    HSMP_MENU_TEST.dispatch("accept", "kb")
    check(not contains(execs(), "--bind 0.0.0.0") and contains(logtext(), "input accept ignored: the connection modal is open"),
        tag .. ": no HOST GAME behind the connection modal")
    -- a latch left by an earlier game run does not lock the keyboard
    write(".conn_state.json", string.format('{"v":1,"state":"lost","reason":"kicked","latched":true,"actions":["menu"],"wall":%d}\n', os.time() - 3600))
    M.run(600)
    HSMP_MENU_TEST.state.top_focus = 1
    HSMP_MENU_TEST.dispatch("accept", "kb")
    check(contains(execs(), "--bind 0.0.0.0"), tag .. ": an old run's latch is ignored (HOST works)")
end

-- Security: a joiner on a server with NO admin (dedicated, no configured admins)
-- sees read-only settings and the READY vote: AUTO START (n/m), then STARTING IN n
lobby_t[12] = function(tag)
    boot(1920, 1080, 1.0)
    enter_screen("browser")
    local B = package.loaded["browser"]
    B._accept_result({ status = "ok", servers = some_servers(3), done = true })
    B.render()
    B.joining_until = 0
    B.sel = "10.0.0.2:7777"
    B.render()
    M.click(B.b.w.join.button)
    M.run(1500)
    write(".sidecar.json", '{"status":"connected","peer_id":2,"is_admin":false,"peers":[{"id":1,"nick":"Other"},{"id":2,"nick":"Me"}]}\n')
    write(".match.json", match_json(nil, "Map_Arena_Pit", 3, { 1 }))
    write(".session.json", '{"v":1,"phase":"lobby","roster":[{"peer_id":1,"admin_role":"none","connected":true},'
        .. '{"peer_id":2,"admin_role":"none","connected":true}]}\n')
    M.run(1100)
    local Kit = kit()
    local w = Kit.cur.w
    check(T.all(w.maps.chips, function(c) return truthy(c.disabled) end) and truthy(w.modes.by_key[3].disabled),
        tag .. ": settings stay read-only without an admin")
    check(w.start.label.last == "AUTO START (1/2)" and truthy(w.start.disabled),
        tag .. ": START shows the READY vote (" .. tostring(w.start.label.last) .. ")")
    check(contains(w.note.last, "No host on this server") and contains(w.note.last, "READY"),
        tag .. ": the note explains how the match starts (" .. tostring(w.note.last) .. ")")
    check(contains(w.map_hint.last, "SET BY THE SERVER"), tag .. ": no 'HOST PICKS' without a host (" .. tostring(w.map_hint.last) .. ")")
    -- everyone ready: the server armed the auto start (lobby deadline)
    write(".match.json", match_json(nil, "Map_Arena_Pit", 3, { 1, 2 }))
    -- the record carries the deadline; the menu counts it down from the receipt (4.3 s, read ~1 s later)
    write(".session.json", '{"v":1,"phase":"lobby","deadline_in_ms":4300,"roster":[{"peer_id":1,"admin_role":"none","connected":true},'
        .. '{"peer_id":2,"admin_role":"none","connected":true}]}\n')
    M.run(1100)
    check(w.start.label.last == "STARTING IN 4" and contains(w.note.last, "starts in 4 s"),
        tag .. ": the countdown is shown (" .. tostring(w.start.label.last) .. " | " .. tostring(w.note.last) .. ")")
    -- an admin is connected: the old 'host starts' wording
    write(".session.json", '{"v":1,"phase":"lobby","roster":[{"peer_id":1,"admin_role":"admin","connected":true},'
        .. '{"peer_id":2,"admin_role":"none","connected":true}]}\n')
    M.run(1100)
    check(contains(w.start.label.last, "HOST STARTS"), tag .. ": with an admin the host starts (" .. tostring(w.start.label.last) .. ")")
end

local proc = {}
-- --parent-pid always (the game pid from the native module); the listen host's owner key + env
proc[1] = function(tag)
    boot(1920, 1080, 1.0)
    check(not T.any(M.execs, function(e) return contains(tostring(e), "powershell") end) and contains(logtext(), "game pid 4242"),
        tag .. ": the game pid comes from the native module (no PowerShell walk)")
    M.execs = {}
    host_lobby()
    local ex = T.filter(T.map(M.execs, tostring), function(e) return startswith(e, "spawn ") and contains(e, "--parent-pid 4242") end)
    check(#ex == 2, tag .. ": server and sidecar get --parent-pid (" .. #ex .. ")")
    -- Security: the listen host is admin by its player key (the sidecar's .player_key)
    local own = T.filter(T.map(M.execs, tostring), function(e) return contains(e, "--owner-key-file") end)
    check(#own == 1 and contains(own[1], "hsmp-server") and contains(own[1], ".player_key ")
        and contains(own[1], "HSMP_LISTEN_HOST=1"), tag .. ": HOST passes --owner-key-file <state>\\.player_key (" .. T.repr(own) .. ")")
end-- CANCEL: leave request first, the kill only after the sidecar ended (or 1 s)
proc[2] = function(tag)
    boot(1920, 1080, 1.0)
    host_lobby()
    M.execs = {}
    local Kit = kit()
    M.click(Kit.cur.w.cancel.button)
    check(Kit.confirm_open() and contains(Kit.cur.cstrip.text.last, "1 other player"), tag .. ": CLOSE LOBBY with a player connected asks first")
    M.click(Kit.cur.cstrip.yes.button)
    check(exists_(".leave_request.json") and contains(logtext(), "leave request (CANCEL)"), tag .. ": a typed leave record is sent (reason CANCEL logged)")
    check(#kills() == 0 and kit().cur == nil, tag .. ": the menu is back at once; nothing killed yet")
    write(".sidecar.json", '{"status":"ended","peer_id":1,"peers":[]}\n')
    M.run(300)
    check(#kills() == 1 and killed("hsmp-sidecar.exe") and contains(logtext(), "sidecar ended after"), tag .. ": the sidecar ended -> its kill (" .. #kills() .. ")")
    M.run(500)
    check(#kills() == 2 and killed("hsmp-server.exe"), tag .. ": the listen server gets 400 ms for its close broadcast, then its kill (" .. #kills() .. ")")
    -- a sidecar that never ends is stopped after 1 s
    boot(1920, 1080, 1.0)
    host_lobby()
    M.execs = {}
    cancel_lobby()
    check(killed("hsmp-sidecar.exe") and contains(logtext(), "did not end - stopping it"), tag .. ": no answer -> stopped after 1 s")
    -- QUIT MP tears the session down before quitting
    boot(1920, 1080, 1.0)
    host_lobby()
    local qsc = spawned("hsmp-sidecar.exe")
    qsc = qsc[#qsc]
    exit_screen()
    M.execs = {}
    local rib = T.filter(M.live(), function(w) return startswith(tostring(w.name), "HSMPBtn_top5") end)
    local had_lr = false
    local _rm = os.remove
    os.remove = function(p) if contains(tostring(p), ".leave_request.json") and T.exists(p) then had_lr = true end; return _rm(p) end
    M.click(rib[1].obj)
    -- No busy-wait: the click returns with the leave request written,
    -- nothing killed and the game still running
    check(exists_(".leave_request.json") and #kills() == 0
        and not T.any(M.execs, function(e) return tostring(e) == "QuitGame 0" end),
        tag .. ": QUIT MP asks the sidecar to leave and returns at once (no busy-wait, no kill yet)")
    M.click(rib[1].obj)
    check(contains(logtext(), "quit: already leaving the session - ignored"), tag .. ": a second QUIT MP click is ignored")
    M.run(2000)
    os.remove = _rm
    local ex = T.map(M.execs, tostring)
    local ki, qi
    for i, e in ipairs(ex) do
        if qsc and contains(e, "proc_kill " .. qsc.pid .. " ") then ki = ki or i end
        if e == "QuitGame 0" then qi = qi or i end
    end
    -- (the leave request is a G2S message: the mock keeps its last copy as the
    -- old file; the ring's consumer is reset at the next attach)
    check(ki and qi and ki < qi and exists_(".leave_request.json"), tag .. ": QUIT MP: leave request + kill before quit")
    -- A sidecar that answers the leave (status "ended") within the
    -- wait: the quit follows its exit, well inside LEAVE_WAIT_S
    boot(1920, 1080, 1.0)
    host_lobby()
    exit_screen()
    M.execs = {}
    rib = T.filter(M.live(), function(w) return startswith(tostring(w.name), "HSMPBtn_top5") end)
    M.click(rib[1].obj)
    M.run(400)   -- the sidecar's 250 ms leave poll sees the request, sends C2SLeave, restores the career save
    check(not T.any(M.execs, function(e) return tostring(e) == "QuitGame 0" end), tag .. ": still waiting for the sidecar at 400 ms")
    write(".sidecar.json", '{"status":"ended","peer_id":1,"peers":[]}\n')
    M.run(300)
    check(contains(logtext(), "leave (quit): sidecar ended after"), tag .. ": the sidecar ended gracefully (no 'did not end')")
    M.run(500)
    check(T.any(M.execs, function(e) return tostring(e) == "QuitGame 0" end) and not contains(logtext(), "leave (quit): sidecar did not end"),
        tag .. ": the game quits after the graceful leave")
end

-- perf: an idle lobby rebuilds nothing and writes no widget; the cost is logged every 10 s
local function test_perf(tag)
    boot(1920, 1080, 1.0)
    host_lobby("Map_Arena_Alley")     -- the server already has the boot arena: nothing pending
    M.run(11000)
    check(contains(logtext(), "menu perf (lobby"), tag .. ": the menu cost is logged")
    M.run(10500)
    local r = kit().perf.report
    check(r and r.screen == "lobby" and r.builds == 0 and r.writes == 0 and r.ticks >= 150,
        tag .. ": idle lobby: 0 rebuilds, 0 widget writes (" .. T.repr(r and { r.builds, r.writes, r.ticks, r.renders }) .. ")")
    enter_screen("browser")
    local B = package.loaded["browser"]
    B._accept_result({ status = "ok", servers = some_servers(18), done = true })
    B.q.inflight = false
    M.run(21000)
    r = kit().perf.report
    check(r and r.screen == "browser" and r.builds == 0 and r.writes <= 30, tag .. ": idle browser: in-place age text only (" .. T.repr(r and { r.builds, r.writes }) .. ")")
end

-- ASCII render of the live canvas (opt-in: `hsmp-tools lua-test menu_ui -- dump`).
-- Buttons are [ ... ] boxes, texts are written at their slot; one char = cw/cols units.
local function ascii_screen(title, cw, ch, cols, rows)
    local grid = {}
    for r = 1, rows do grid[r] = {}; for c = 1, cols do grid[r][c] = " " end end
    local function cx(x) return math.floor((x + cw / 2) / cw * cols) + 1 end
    local function cy(y) return math.floor((y + ch / 2) / ch * rows) + 1 end
    local function put(r, c, s)
        if r < 1 or r > rows then return end
        for i = 1, #s do local cc = c + i - 1; if cc >= 1 and cc <= cols then grid[r][cc] = s:sub(i, i) end end
    end
    local live = T.filter(M.live(), function(w) return startswith(tostring(w.name), "HSMPUI_") end)
    local panel
    for _, w in ipairs(live) do if w.cls == "Border" and (not panel or w.w * w.h > panel.w * panel.h) then panel = w end end
    if panel then
        local c1, c2, r1, r2 = cx(panel.x), cx(panel.x + panel.w) - 1, cy(panel.y), cy(panel.y + panel.h) - 1
        for c = c1, c2 do put(r1, c, "-"); put(r2, c, "-") end
        for r = r1, r2 do put(r, c1, "|"); put(r, c2, "|") end
    end
    for _, w in ipairs(live) do
        if w.cls == "Button" then
            local c1, c2, r = cx(w.x), cx(w.x + w.w) - 1, cy(w.y + w.h / 2)
            if c2 > c1 then put(r, c1, "["); put(r, c2, "]") end
        end
    end
    for _, w in ipairs(live) do
        if w.cls == "EditableTextBox" then
            local c1, c2, r = cx(w.x), cx(w.x + w.w) - 1, cy(w.y + w.h / 2)
            put(r, c1, "["); for c = c1 + 1, c2 - 1 do put(r, c, "_") end; put(r, c2, "]")
            local t = rawget(w.obj, "text") or rawget(w.obj, "hint") or ""
            put(r, c1 + 1, t:sub(1, math.max(0, c2 - c1 - 2)))
        end
    end
    for _, w in ipairs(live) do
        if w.cls == "TextBlock" and strip(w.text or "") ~= "" then
            local c1 = cx(w.x)
            local width = math.max(1, cx(w.x + w.w) - c1)
            local s = w.text
            local just = rawget(w.obj, "just") or 0
            if #s < width then
                if just == 1 then c1 = c1 + math.floor((width - #s) / 2) elseif just == 2 then c1 = c1 + width - #s end
            else s = s:sub(1, width) end
            put(cy(w.y + w.h / 2), c1, s)
        end
    end
    local out = { "== " .. title }
    for r = 1, rows do
        local line = table.concat(grid[r]):gsub("%s+$", "")
        if line ~= "" then out[#out + 1] = line end
    end
    io.stdout:write(table.concat(out, "\n"), "\n")
end

local function test_dump()
    boot(1920, 1080, 1.0)
    local cw, ch = 1920, 1080
    kit_remote(2, "knight", "w_longsword3", "")
    kit_remote(3, "duelist", "", "")
    write(".metrics.json", '{"rtt_ms":42,"srtt_ms":41.6}\n')
    write(".sidecar.json", '{"status":"connected","peer_id":1,"is_admin":true,"peers":[{"id":1,"nick":"Willie"},{"id":2,"nick":"Mate"},{"id":3,"nick":"Peasant"}]}\n')
    write(".match.json", match_json(nil, "Map_Arena_Pit", 3, { 2 }))
    kit_rules(2, 30)
    local rib = T.filter(M.live(), function(w) return startswith(tostring(w.name), "HSMPBtn_top1") end)
    M.click(rib[1].obj)
    M.run(2600)
    local Kit = kit()
    local w = Kit.cur.w
    Kit.set_focus(w.rows[2].btn, "key"); Kit.key("accept", "kb"); M.run(100)
    ascii_screen("LOBBY (host, Mate selected)", cw, ch, 190, 54)
    M.click(w.loadout.button)
    ascii_screen("LOADOUT", cw, ch, 190, 54)
    exit_screen()
    enter_screen("browser")
    local B = package.loaded["browser"]
    B._accept_result({ status = "ok", servers = some_servers(18), done = true, master_url = "https://master.example", master_idx = 1, master_n = 1 })
    B.q.inflight = false
    B.sel = "10.0.0.3:7777"
    B.render(); M.run(100)
    ascii_screen("SERVER BROWSER", cw, ch, 190, 54)
    exit_screen()
    enter_screen("settings")
    M.set(Kit.cur.w.nick.box, "text", "Willie")
    M.run(300)
    ascii_screen("SETTINGS", cw, ch, 190, 54)
    exit_screen()
    enter_screen("character")
    ascii_screen("CHARACTER", cw, ch, 190, 54)
end

-- ============================================================================================
-- UI-SCALE: the viewport probe as it behaves in the game, live resize / resolution / DPI
-- changes, the ribbon column and the screens' state across a rebuild.
-- ============================================================================================

local function ribbons_abs(cw, ch)
    local out = {}
    for _, w in ipairs(M.live()) do
        if startswith(tostring(w.name), "HSMPBtn_top") then
            out[#out + 1] = { name = w.name, x = cw / 2 + w.x, y = ch / 2 + w.y, w = w.w, h = w.h }
        end
    end
    table.sort(out, function(a, b) return a.y < b.y end)
    return out
end

local function ribbon_checks(tg, cw, ch)
    local rb = ribbons_abs(cw, ch)
    check(#rb == 5, string.format("%s: 5 ribbons on screen (%d)", tg, #rb))
    local s = UIS.scale(cw, ch)
    local safe = { x = 0, y = 0, w = cw - UIS.margin(cw, ch, s) + 1, h = ch }
    local bad = T.filter(rb, function(r) return not UIS.inside(r, safe) end)
    check(#bad == 0, string.format("%s: ribbon column inside the canvas' right safe margin (%s)", tg,
        T.repr(bad[1] and { bad[1].x, bad[1].w, cw } or nil)))
end

local function relayouts() return T.count(logtext(), "relayout #") end

local resize = {}
-- 1. the in-game probe: GameViewportClient:GetViewportSize is C++-only (the pcall fails), the
-- layout library answers. 1280x720 on the default curve (0.667) is a 1920x1080 canvas - the old
-- code assumed 1920x1080 physical and built for a 2883x1622 canvas (1.5x too big, off screen).
resize[1] = function(tag)
    boot_extra = { gvc_broken = true }
    boot(1280, 720, 720 / 1080)
    local logs = logtext()
    check(contains(logs, "viewport 1280x720 dpi 0.667 -> canvas 1920x1080 scale 1.00 [wll]"),
        tag .. ": the layout library gives the real viewport (" .. tostring(logs:match("viewport [^\n]*")) .. ")")
    check(not contains(logs, "2883"), tag .. ": no 2883x1622 canvas (the old 1920x1080 assumption)")
    ribbon_checks(tag .. " top", 1920, 1080)
    host_lobby()
    check(kit().cur and kit().cur.L.cw == 1920 and kit().cur.L.s == 1, tag .. ": lobby built for the 1920x1080 canvas")
    layout_checks(tag .. " lobby", 1920, 1080, 720 / 1080, true)
end
-- 2. no size source at all but the DPI scale: the default curve gives a 16:9 canvas of short side 1080
resize[2] = function(tag)
    boot_extra = { gvc_broken = true, wll_size_broken = true }
    boot(2560, 1440, 1440 / 1080)
    check(contains(logtext(), "-> canvas 1920x1080 scale 1.00 [dpi-curve]"),
        tag .. ": DPI-only fallback (" .. tostring(logtext():match("viewport [^\n]*")) .. ")")
    host_lobby()
    layout_checks(tag .. " lobby", 1920, 1080, 1440 / 1080, true)
end
-- 3. a live resolution change with the browser open: one rebuild at the new scale, page / filter
-- / selection kept; then a small window; then a level change (nothing touched afterwards)
resize[3] = function(tag)
    boot_extra = { gvc_broken = true }
    boot(1920, 1080, 1.0)
    enter_screen("browser")
    local B = package.loaded["browser"]
    B._accept_result({ status = "ok", servers = some_servers(30), done = true })
    B.render()
    M.click(B.b.w.f_full.button)
    M.click(B.b.w.pager.next.button)
    local page, rows1 = B.page, #B.view
    M.click(B.b.w.rows[2].btn.button)
    local sel = B.sel
    check(page == 2 and sel ~= nil, tag .. ": browser on page 2 with a selection")
    M.vw, M.vh, M.scale = 5120, 1440, 1440 / 1080
    M.run(1000)
    check(relayouts() == 1, string.format("%s: one relayout after the change (%d)", tag, relayouts()))
    local Kit = kit()
    check(Kit.cur and Kit.cur.screen == "browser" and math.floor(Kit.cur.L.cw + 0.5) == 3840 and Kit.cur.L.ch == 1080,
        tag .. ": browser rebuilt for the 3840x1080 canvas (" .. tostring(Kit.cur and Kit.cur.L.cw) .. ")")
    check(B.page == page and B.sel == sel and B.f.hide_full and #B.view == rows1,
        tag .. ": page, selection and filters survive the rebuild")
    layout_checks(tag .. " browser 5120x1440", 3840, 1080, 1440 / 1080, true)
    nav_checks(tag .. " browser 5120x1440")
    M.vw, M.vh, M.scale = 1024, 576, 1.0
    M.run(1000)
    check(relayouts() == 2 and Kit.cur and Kit.cur.L.cw == 1024, tag .. ": second relayout for a 1024x576 window")
    layout_checks(tag .. " browser 1024x576", 1024, 576, 1.0, true)
    M.run(2000)
    check(relayouts() == 2, tag .. ": a steady size never rebuilds again")
    exit_screen()
    ribbon_checks(tag .. " top 1024x576", 1024, 576)
    M.premap()
    M.kill_all()
    M.vw, M.vh = 1920, 1080
    M.run(2000)
    local dead = T.list(M.dead_touch)
    check(#dead == 0, tag .. ": the resize poll touches nothing after a level change (" .. T.repr({ dead[1], dead[2] }) .. ")")
end
-- 4. a window drag (a new size every 100 ms) rebuilds once, after it settles
resize[4] = function(tag)
    boot(1920, 1080, 1.0)
    enter_screen("settings")
    for i = 1, 10 do M.vw, M.vh = 1920 - i * 40, 1080 - i * 20; M.run(100) end
    check(relayouts() == 0, string.format("%s: no rebuild while the size keeps changing (%d)", tag, relayouts()))
    M.run(800)
    check(relayouts() == 1, string.format("%s: exactly one rebuild once it settled (%d)", tag, relayouts()))
    local cw, ch = 1520, 880
    layout_checks(tag .. " settings", cw, ch, 1.0, true)
end
-- 5. unsaved settings + typed text survive a DPI change
resize[5] = function(tag)
    boot(1920, 1080, 1.0)
    enter_screen("settings")
    local Kit = kit()
    M.set(Kit.cur.w.nick.box, "text", "Bobby")
    M.click(Kit.cur.w.hz.by_key[90].button)
    M.scale = 1.5
    M.run(1000)
    check(relayouts() == 1, tag .. ": a DPI change rebuilds")
    local S = package.loaded["settings"]
    check(Kit.cur and Kit.cur.screen == "settings" and Kit.input_text(Kit.cur.w.nick) == "Bobby",
        tag .. ": the typed nickname survives (" .. tostring(Kit.cur and Kit.input_text(Kit.cur.w.nick)) .. ")")
    check(S._edit().send_hz == 90 and truthy(Kit.cur.w.hz.by_key[90].on), tag .. ": the staged SEND RATE survives")
    layout_checks(tag .. " settings dpi 1.5", 1280, 720, 1.5, true)
end
-- 6. the ribbon column at every tested resolution (top level, no screen)
resize[6] = function(tag)
    boot(1920, 1080, 1.0)
    for _, r in ipairs(RES.RES) do
        M.vw, M.vh, M.scale = r[1], r[2], 1.0
        M.run(800)
        ribbon_checks(string.format("%s %dx%d", tag, r[1], r[2]), r[1], r[2])
    end
    check(relayouts() >= #RES.RES - 1, tag .. ": every change relayouts the ribbons")
end

-- ---- dispatch -----------------------------------------------------------------------------
local k = opts.kind
if k == "res" then test_resolution()
elseif k == "director" then director[opts.n]("director")
elseif k == "cmd" then cmd[opts.n]("cmd")
elseif k == "authority" then test_no_fighting_the_server("authority")
elseif k == "autotest" then test_autotest_maps("autotest")
elseif k == "cfg" then cfg[opts.n]("cfg+log")
elseif k == "pid" then pid[opts.n]("pid")
elseif k == "at" then at[opts.n]("harness")
elseif k == "net" then net[opts.n]("net." .. opts.n)
elseif k == "matrix" then test_matrix()
elseif k == "resize" then resize[opts.n]("resize." .. opts.n)
elseif k == "nav" then nav[opts.n]("nav." .. opts.n)
elseif k == "settings" then settings_t[opts.n]("settings." .. opts.n)
elseif k == "lobby" then lobby_t[opts.n]("lobby." .. opts.n)
elseif k == "proc" then proc[opts.n]("proc." .. opts.n)
elseif k == "perf" then test_perf("perf")
elseif k == "dump" then test_dump()
else error("unknown case " .. tostring(k)) end
