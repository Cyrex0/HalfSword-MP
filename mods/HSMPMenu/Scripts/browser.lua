-- HSMPMenu / browser.lua — the SERVER BROWSER sub-screen.
--
-- A server browser built from flat ui_kit widgets on the Startup menu canvas
-- (no overlay, no scroll boxes, no "< value >" cyclers):
--
--   SERVER BROWSER                                   [REFRESH]
--   [search box......] [HIDE FULL] [HIDE EMPTY] [HIDE LOCKED]   PING [ANY][<50][<100][<150]  [CLEAR]
--   PW | SERVER NAME | MAP | MODE | PLAYERS | PING | REGION / VER   <- click = sort
--   12 fixed rows (hover highlight; click = select; double-click = join)
--   status bar: list age / counts / errors                   selection
--   [< PREV]  PAGE x / y  [NEXT >]
--   RECENT [addr 1] [addr 2] [addr 3] [addr 4] [addr 5]     <- one-click direct connect
--   [direct ip:port box] [CONNECT]                         [BACK] [JOIN]
--
-- Data: an `hsmp-query.exe` process fetches the master list and UDP-pings
-- every server (server/src/query_tool.rs) and prints a tab-separated result
-- on stdout. The native module starts it (IPC.spawn_capture: no window, a
-- pipe, no state-dir file) and the browser tick polls for its exit
-- (IPC.capture_poll), so nothing here blocks the game thread.
-- If hsmp-query.exe is missing we fall back to curl.exe (captured the same
-- way) + a tolerant JSON scan (no pings).
--
-- Masters: hsmp.cfg master_url may list a primary and fallbacks. They are
-- tried in order (hsmp-query --master A --master B ..., or one curl per
-- master); the status bar names the one that answered. When none answers the
-- table shows "Server list unavailable (...). Use DIRECT CONNECT or LAN" -
-- never an endless spinner (every refresh has a deadline).
--
-- LAN: hsmp-query --lan broadcasts the browser query (query.rs) on the LAN
-- and to 127.0.0.1 over hsmp.cfg lan_ports (default 7777-7786); servers that
-- answer are listed in a "LAN" section above the internet list, so local play
-- works with no master at all. The shipped hsmp-query always has --lan.
--
-- main.lua owns the screen framework: it calls B.build() when the browser
-- screen is entered and destroys everything we put into
-- state.screen_widgets on exit (then calls B.forget()). Clickable entries are
-- edge-polled by main.lua's poll_set (33 ms); texts/backgrounds are `nopoll`.
-- Widget refs live only in the current ui_kit build (B.b); every touch is
-- guarded by Kit.alive(B.b), which is false after a world change.

local B = {}

local ctx            -- set by B.init
local Kit            -- ui_kit
local Log = function(...) end

local ROWS = 12
local PING_CHIPS = { { 0, "ANY" }, { 50, "<50" }, { 100, "<100" }, { 150, "<150" } }
local PING_OK = { [0] = true, [50] = true, [100] = true, [150] = true }
local QUERY_TIMEOUT_S = 12
local PER_FALLBACK_S = 4                          -- extra deadline per fallback master
local CURL_TIMEOUT_S = 8
local DIRECT_MAX = 5                              -- direct-connect history (one-click chips)
local DEFAULT_LAN_PORTS = "7777-7786"
local AUTO_REFRESH_S = 20                         -- on (re)enter, if older
local DOUBLE_CLICK_S = 0.50
local DOUBLE_CLICK_MIN_S = 0.05                   -- faster than this = switch bounce, not a double-click

-- text colours (filled from ui_kit in B.init)
local C = {}

-- columns: key, header label, width fraction, justification (0 L, 1 C, 2 R)
local COLS = {
    { key = "pwd",     label = "PW",            frac = 0.045, just = 1 },
    { key = "name",    label = "SERVER NAME",   frac = 0.335, just = 0 },
    { key = "map",     label = "MAP",           frac = 0.150, just = 0 },
    { key = "mode",    label = "MODE",          frac = 0.130, just = 0 },
    { key = "players", label = "PLAYERS",       frac = 0.090, just = 1 },
    { key = "ping",    label = "PING",          frac = 0.080, just = 1 },
    { key = "region",  label = "REGION / VER",  frac = 0.170, just = 0 },
}
-- default direction when a column is first clicked
local SORT_DEFAULT_DESC = { players = true }

-- --- model ------------------------------------------------------------------

B.servers  = {}        -- last good list (rows from hsmp-query)
B.view     = {}        -- filtered + sorted
B.page     = 1
B.sel      = nil       -- "host:port"
B.f        = { search = "", hide_full = false, hide_empty = false, hide_locked = false, compat_only = false, max_ping = 0 }
B.sort     = { key = "players", desc = true }
B.direct   = {}        -- recent direct-connect addresses, newest first (max DIRECT_MAX)
B.q        = {
    inflight = false, mode = nil, gen = nil, started = 0,
    status = "none", msg = "", listed = 0, skipped = 0, done = true,
    last_ok = nil,          -- os.time() of the last list we accepted
    have_list = false, my_proto = nil, pings_pending = false,
    urls = {},              -- masters tried this refresh, in order
    master_url = nil, master_idx = nil, master_n = nil,   -- the one that answered
    lan_on = false, lan_n = 0, limit = QUERY_TIMEOUT_S,
}
B.view_n   = 0         -- servers in B.view (B.view also holds section headers)
B.flash    = nil       -- { text, color, until_t }
B.b        = nil       -- current ui_kit build (widget refs in B.b.w); dropped on exit / world change
B.serial   = 0
B.uid      = 0
B.last_click = { key = nil, t = 0 }
B.joining_until = 0

-- --- small helpers -------------------------------------------------------------

local function now() return os.clock() end

local function trim(s) return s and (s:gsub("^%s+", ""):gsub("%s+$", "")) or "" end

local function ascii(s)
    -- UE4SS FText from Lua strings is not reliably UTF-8 aware; keep it safe.
    return (tostring(s or ""):gsub("[\128-\255]+", "?"):gsub("[%c]", " "))
end

local function clip(s, n)
    s = ascii(s)
    if #s > n then return s:sub(1, math.max(1, n - 2)) .. ".." end
    return s
end

local function split_tabs(line)
    local t = {}
    for f in (line .. "\t"):gmatch("([^\t]*)\t") do t[#t + 1] = f end
    return t
end

local function key_of(s)
    local h = tostring(s.host)
    if h:find(":", 1, true) and not h:find("^%[") then h = "[" .. h .. "]" end -- IPv6 literal
    return h .. ":" .. tostring(s.port)
end

local function flash(text, color, secs)
    B.flash = { text = text, color = color or C.warn, until_t = now() + (secs or 4) }
    Log("browser: %s", text)
end

-- true, or false + a short note ("needs v0.2.0"). With the mods' build identity
-- (ctx.build / ctx.build_id, shared/hsmp_build.lua): protocol range and content tag;
-- without it, the query helper's protocol number.
local function compatible(s)
    local Bld, mine = ctx and ctx.build, ctx and ctx.build_id and ctx.build_id()
    if Bld and mine then return Bld.server_compat(mine, s) end
    if not B.q.my_proto or not s.proto or s.proto == 0 then return true end
    if s.proto == B.q.my_proto then return true end
    return false, string.format("needs protocol v%d", s.proto)
end

-- "Server runs HalfSword-MP X, you have Y" + what to do.
local function mismatch_text(s)
    local mine = ctx and ctx.build_id and ctx.build_id()
    local theirs = (s.version ~= "" and s.version) or ("protocol v" .. tostring(s.proto))
    local have = (mine and mine.version) or ("protocol v" .. tostring(B.q.my_proto or "?"))
    return string.format("Server runs HalfSword-MP %s, you have %s. Update via the launcher, or the server is outdated.", theirs, have)
end

local function safe_url(u)
    u = trim(u)
    if u:match("^https?://[%w%.%-_:/]+$") and #u < 200 then return u end
    return nil
end

local function safe_addr(a)
    a = trim(a)
    if (a:match("^[%w%.%-]+:%d+$") or a:match("^%[[%x:%.]+%]:%d+$")) and #a < 260 then return a end
    return nil
end

-- --- persistence (per-viewer conveniences only) --------------------------------

local function prefs_file() return ctx.STATE_DIR .. "/.browser_prefs.txt" end

local function save_prefs()
    local f = io.open(prefs_file(), "wb"); if not f then return end
    f:write("search=", ascii(B.f.search):gsub("[\r\n=]", " "), "\n")
    f:write("hide_full=", tostring(B.f.hide_full), "\n")
    f:write("hide_empty=", tostring(B.f.hide_empty), "\n")
    f:write("hide_locked=", tostring(B.f.hide_locked), "\n")
    f:write("compat_only=", tostring(B.f.compat_only), "\n")
    f:write("max_ping=", tostring(B.f.max_ping), "\n")
    f:write("sort_key=", B.sort.key, "\n")
    f:write("sort_desc=", tostring(B.sort.desc), "\n")
    for i, d in ipairs(B.direct) do f:write("direct", i, "=", d, "\n") end
    f:close()
end

local function load_prefs()
    local f = io.open(prefs_file(), "rb"); if not f then return end
    for line in f:lines() do
        local k, v = line:match("^([%w_]+)=(.*)$")
        if k then
            v = v:gsub("\r$", "")
            if k == "search" then B.f.search = v
            elseif k == "hide_full" then B.f.hide_full = (v == "true")
            elseif k == "hide_empty" then B.f.hide_empty = (v == "true")
            elseif k == "hide_locked" then B.f.hide_locked = (v == "true")
            elseif k == "compat_only" then B.f.compat_only = (v == "true")
            elseif k == "max_ping" then
                local n = tonumber(v) or 0
                B.f.max_ping = PING_OK[n] and n or 0      -- old prefs may hold 250
            elseif k == "sort_key" then
                for _, c in ipairs(COLS) do if c.key == v then B.sort.key = v end end
            elseif k == "sort_desc" then B.sort.desc = (v == "true")
            elseif k:match("^direct%d$") and safe_addr(v) and #B.direct < DIRECT_MAX then
                local dup = false
                for _, d in ipairs(B.direct) do if d == v then dup = true end end
                if not dup then table.insert(B.direct, v) end
            end
        end
    end
    f:close()
end

local function remember_direct(addr)
    local out = { addr }
    for _, d in ipairs(B.direct) do
        if d ~= addr and #out < DIRECT_MAX then table.insert(out, d) end
    end
    B.direct = out
    save_prefs()
end

-- --- querying ----------------------------------------------------------------------

-- The helpers run through the native module: IPC.spawn_capture
-- reads their stdout through a pipe and IPC.capture_poll reports the exit, so
-- nothing is written to the state dir and nothing blocks the game thread.
local function ipc() return rawget(_G, "HSMP_IPC") end

local function exe_exists(p)
    local f = io.open(p, "rb"); if f then f:close(); return true end
    return false
end

-- curl ships with Windows 10+ (System32); found on PATH by CreateProcess.
local CURL_EXE = "curl.exe"

-- Every master to try, in order. ctx.MASTER_URLS is hsmp_cfg's master_url
-- list (hsmp.cfg, HSMP_MASTER_URL already applied; primary first). An
-- explicit HSMP_MASTER_URL wins; otherwise a master_url saved in
-- .settings.json goes first, with hsmp.cfg's list as its fallbacks.
local function master_urls()
    local list, seen = {}, {}
    local function add(u)
        u = u and safe_url(u)
        if u and not seen[u] then seen[u] = true; list[#list + 1] = u end
    end
    if (os.getenv("HSMP_MASTER_URL") or "") == "" then
        local f = io.open(ctx.STATE_DIR .. "/.settings.json", "rb")
        if f then
            local line = f:read("*a") or ""; f:close()
            -- SETTINGS saves a comma-separated list, primary first
            for part in ((line:match('"master_url"%s*:%s*"([^"]+)"') or "") .. ","):gmatch("([^,]*),") do add(part) end
        end
    end
    for _, u in ipairs(ctx.MASTER_URLS or {}) do add(u) end
    add(ctx.MASTER_URL)
    if #list == 0 then list[1] = "https://master.halfswordmp.workers.dev" end
    return list
end

local function host_of(url) return (tostring(url or ""):match("^https?://([^/]+)") or tostring(url or "")) end

local function lan_ports()
    local p = trim(ctx.LAN_PORTS or "")
    local a, b = p:match("^(%d+)%-(%d+)$")
    a, b = tonumber(a or p), tonumber(b or a or p)
    if not a or not b or a < 1 or b > 65535 or b < a or b - a > 63 then return DEFAULT_LAN_PORTS end
    return (a == b) and tostring(a) or (a .. "-" .. b)
end

function B.refresh()
    if B.q.inflight and (os.time() - B.q.started) < (B.q.limit or QUERY_TIMEOUT_S) then
        flash("Refresh already in progress...", C.dim, 2)
        return
    end
    local urls = master_urls()
    B.serial_q = (B.serial_q or 0) + 1
    local gen = tostring(os.time()) .. "_" .. tostring(B.serial_q)
    B.q.gen = gen
    B.q.started = os.time()
    B.q.inflight = true
    B.q.done = false
    B.q.pings_pending = false
    B.q.urls = urls
    B.q.master_url, B.q.master_idx, B.q.master_n = nil, nil, nil
    B.q.trying = nil
    -- a helper still running from an earlier (timed-out) refresh is ours: stop it
    if B.q.h and ipc() then pcall(ipc().proc_kill, B.q.h) end
    B.q.h, B.q.curl_i = nil, nil

    local exe = ctx.QUERY_EXE
    local I = ipc()
    local h, err
    if exe and exe_exists(exe) then
        B.q.mode = "tool"
        local args = {}
        for _, u in ipairs(urls) do args[#args + 1] = "--master"; args[#args + 1] = u end
        if ctx.LAN ~= false then
            args[#args + 1] = "--lan"; args[#args + 1] = "--lan-ports"; args[#args + 1] = lan_ports()
        end
        B.q.limit = QUERY_TIMEOUT_S + PER_FALLBACK_S * (#urls - 1)
        B.q.lan_on = ctx.LAN ~= false
        args[#args + 1] = "--gen"; args[#args + 1] = gen
        for _, d in ipairs(B.direct) do
            if safe_addr(d) then args[#args + 1] = "--direct"; args[#args + 1] = d end
        end
        Log("browser refresh (gen %s): %s %s", gen, exe, table.concat(args, " "))
        if I then h, err = I.spawn_capture(exe, args) else err = "shared/hsmp_ipc.lua missing" end
    else
        -- curl fallback: each master in order (the next one starts when one fails).
        B.q.mode = "curl"
        B.q.lan_on = false
        B.q.limit = CURL_TIMEOUT_S + PER_FALLBACK_S * (#urls - 1)
        B.q.curl_i = 1
        Log("browser refresh (curl fallback, hsmp-query.exe not found at %s): %s", tostring(exe), urls[1])
        if I then h, err = I.spawn_capture(CURL_EXE, { "-s", "-f", "-m", "4", urls[1] .. "/v1/servers" })
        else err = "shared/hsmp_ipc.lua missing" end
    end
    if h then
        B.q.h = h
    else
        -- the helper did not start: a finished refresh with the reason, never a spinner
        B.q.inflight, B.q.done, B.q.pings_pending = false, true, false
        B.q.status = "master_error"
        B.q.msg = string.format("%s did not start (%s)", B.q.mode == "curl" and "curl" or "hsmp-query", tostring(err))
        Log("browser refresh: %s", B.q.msg)
    end
    B.render()
end

-- hsmp-query output (query_tool.rs). Lines this file understands beyond the
-- original set (older readers ignore them):
--   status trying <url (i/n)>     a master is being asked (progress, done 0)
--   master <url> <i> <n>          the master that answered (i of n tried)
--   lan    <0|1> <count>          LAN discovery ran / servers it found
local function parse_tool_text(raw)
    if not raw or raw == "" then return nil end
    local r = { servers = {}, listed = 0, skipped = 0, msg = "" }
    local first = true
    for raw_line in raw:gmatch("[^\n]+") do
        local line = raw_line:gsub("\r$", "")
        local t = split_tabs(line)
        if first then
            if t[1] ~= "HSMPQ" then return nil end
            first = false
        elseif t[1] == "gen" then r.gen = t[2]
        elseif t[1] == "status" then r.status = t[2]; r.msg = t[3] or ""
        elseif t[1] == "counts" then r.listed = tonumber(t[2]) or 0; r.skipped = tonumber(t[3]) or 0
        elseif t[1] == "proto" then r.proto = tonumber(t[2])
        elseif t[1] == "done" then r.done = (t[2] == "1")
        elseif t[1] == "master" then
            r.master_url = safe_url(t[2] or ""); r.master_idx = tonumber(t[3]); r.master_n = tonumber(t[4])
        elseif t[1] == "lan" then r.lan_on = (t[2] == "1"); r.lan_n = tonumber(t[3]) or 0
        elseif t[1] == "S" then
            local port = tonumber(t[3])
            if #t >= 15 and port and t[2] ~= "" then
                table.insert(r.servers, {
                    host = t[2], port = port, name = t[4], map = t[5], mode = t[6],
                    players = tonumber(t[7]) or 0, max = tonumber(t[8]) or 0,
                    pwd = (t[9] == "1"), proto = tonumber(t[10]) or 0,
                    version = t[11], region = t[12], ping = tonumber(t[13]) or -2,
                    source = t[14], live = (t[15] == "1"),
                    content = t[16] or "", proto_min = tonumber(t[17]) or 0, proto_max = tonumber(t[18]) or 0,
                })
            else
                r.skipped = r.skipped + 1
            end
        end
    end
    if first or not r.status then return nil end
    return r
end

-- curl fallback: tolerant scan of a flat JSON array of flat objects (the body
-- curl printed for master `idx`, read once curl exited 0).
local function parse_curl_text(raw, idx)
    raw = trim(raw or "")
    local urls = B.q.urls or {}
    local used = { master_url = idx and urls[idx], master_idx = idx, master_n = #urls }
    if raw:sub(1, 1) ~= "[" or raw:sub(-1) ~= "]" then
        return { status = "master_bad_data", msg = "listing is not a JSON array", servers = {}, listed = 0, skipped = 0, done = true,
                 master_url = used.master_url, master_idx = idx, master_n = #urls }
    end
    local r = { status = "ok", msg = "", servers = {}, listed = 0, skipped = 0, done = true,
                master_url = used.master_url, master_idx = idx, master_n = #urls }
    for obj in raw:gmatch("{(.-)}") do
        local host = obj:match('"host"%s*:%s*"([%w%.%-:]+)"')
        local port = tonumber(obj:match('"port"%s*:%s*(%d+)'))
        if host and port and port > 0 and port < 65536 then
            local name = obj:match('"name"%s*:%s*"([^"]*)"') or ""
            table.insert(r.servers, {
                host = host, port = port,
                name = (name ~= "" and name) or (host .. ":" .. port),
                map = obj:match('"map"%s*:%s*"([^"]*)"') or "",
                mode = obj:match('"mode"%s*:%s*"([^"]*)"') or "",
                players = tonumber(obj:match('"players"%s*:%s*(%d+)')) or 0,
                max = tonumber(obj:match('"max_players"%s*:%s*(%d+)')) or 0,
                pwd = (obj:match('"pwd_protected"%s*:%s*(%a+)') == "true"),
                proto = tonumber(obj:match('"proto_ver"%s*:%s*(%d+)')) or 0,
                version = obj:match('"version"%s*:%s*"([^"]*)"') or "",
                region = obj:match('"region"%s*:%s*"([^"]*)"') or "",
                content = (obj:match('"content_hash"%s*:%s*"(%x+)"') or ""):sub(1, 16):lower(),
                proto_min = tonumber(obj:match('"proto_min"%s*:%s*(%d+)')) or 0,
                proto_max = tonumber(obj:match('"proto_max"%s*:%s*(%d+)')) or 0,
                ping = -3, source = "master", live = false,
            })
        else
            r.skipped = r.skipped + 1
        end
    end
    r.listed = #r.servers
    return r
end

local function master_problem(st)
    return st == "master_unreachable" or st == "master_error" or st == "master_bad_data" or st == "timeout"
end

local function accept_result(r)
    r.servers = r.servers or {}
    if r.status == "trying" then
        -- progress: master i of n is being asked; keep the current list
        B.q.trying = r.msg ~= "" and r.msg or nil
        B.q.pings_pending = false
        return
    end
    B.q.trying = nil
    B.q.status = r.status or "ok"
    B.q.msg = r.msg or ""
    B.q.skipped = r.skipped or 0
    if r.proto then B.q.my_proto = r.proto end
    B.q.master_url, B.q.master_idx, B.q.master_n = r.master_url, r.master_idx, r.master_n
    if r.lan_on ~= nil then B.q.lan_on = r.lan_on end
    local lan = 0
    for _, s in ipairs(r.servers) do if s.source == "lan" then lan = lan + 1 end end
    B.q.lan_n = lan
    if B.q.status == "ok" or B.q.status == "no_master" or #r.servers > 0
        or (master_problem(B.q.status) and B.q.lan_on and r.done) then
        -- With every master down, a finished LAN scan is still a fresh answer
        -- (possibly "nothing on the LAN"): show it instead of an old list.
        -- Keep the selection if the server is still listed.
        B.servers = r.servers
        B.q.listed = #r.servers
        B.q.have_list = true
        B.q.last_ok = os.time()
    end
    B.q.pings_pending = not r.done
    if r.done then
        B.q.inflight = false
        B.q.done = true
        local answered = 0
        for _, s in ipairs(r.servers) do if s.ping and s.ping >= 0 then answered = answered + 1 end end
        Log("browser result: status=%s listed=%d answered=%d skipped=%d lan=%d via=%s %s",
            B.q.status, #r.servers, answered, B.q.skipped, lan,
            r.master_url and string.format("%s (%s/%s)", r.master_url, tostring(r.master_idx), tostring(r.master_n)) or "-",
            B.q.msg)
    end
end

local function poll_query()
    if not B.q.inflight then return false end
    local r
    local I = ipc()
    if B.q.h and I then
        local fin, code, out = I.capture_poll(B.q.h)
        if fin == nil then
            B.q.h = nil                                 -- an unknown handle: the deadline below ends it
        elseif fin then
            B.q.h = nil
            if B.q.mode == "tool" then
                r = parse_tool_text(out)
                if r and r.gen ~= B.q.gen then r = nil end     -- not this refresh's answer
                if not r then
                    r = { status = "master_error", servers = {}, listed = 0, skipped = 0, done = true,
                          msg = string.format("hsmp-query exited %s without a result", tostring(code)) }
                end
            elseif code == 0 then
                r = parse_curl_text(out, B.q.curl_i)
            else
                -- this master failed: the next one, or every one failed
                local urls = B.q.urls or {}
                local i = (B.q.curl_i or 1) + 1
                if i <= #urls then
                    B.q.curl_i = i
                    local h = I.spawn_capture(CURL_EXE, { "-s", "-f", "-m", "4", urls[i] .. "/v1/servers" })
                    B.q.h = h
                end
            end
        end
    end
    if r then
        accept_result(r)
        return true
    end
    local limit = B.q.limit or ((B.q.mode == "curl") and CURL_TIMEOUT_S or QUERY_TIMEOUT_S)
    local curl_all_failed = B.q.mode == "curl" and not B.q.h
    if curl_all_failed or os.time() - B.q.started >= limit then
        if B.q.h and I then pcall(I.proc_kill, B.q.h) end
        B.q.h = nil
        B.q.inflight = false
        B.q.done = true
        B.q.pings_pending = false
        B.q.trying = nil
        B.q.status = "timeout"
        local n = #(B.q.urls or {})
        B.q.msg = (B.q.mode == "curl")
            and ((n > 1) and string.format("none of %d server lists answered", n) or "no answer from the server list")
            or "no result from hsmp-query after " .. limit .. "s"
        Log("browser query timed out (%s)", B.q.mode)
        return true
    end
    return false
end

-- --- filtering / sorting --------------------------------------------------------

local function passes(s)
    local f = B.f
    if f.hide_full and s.max > 0 and s.players >= s.max then return false end
    if f.hide_empty and s.players == 0 then return false end
    if f.hide_locked and s.pwd then return false end
    if f.compat_only and not compatible(s) then return false end
    if f.max_ping > 0 then
        -- pending pings stay visible until measured
        if s.ping == -2 then return false end
        if s.ping >= 0 and s.ping >= f.max_ping then return false end
    end
    local q = trim(f.search):lower()
    if q ~= "" then
        local hay = (tostring(s.name) .. " " .. tostring(s.map) .. " " .. tostring(s.mode)):lower()
        if not hay:find(q, 1, true) then return false end
    end
    return true
end

local function sort_value(s, k)
    if k == "pwd" then return s.pwd and 1 or 0
    elseif k == "players" then return s.players
    elseif k == "ping" then return (s.ping and s.ping >= 0) and s.ping or 1e9
    elseif k == "region" then return (tostring(s.region) .. " " .. tostring(s.version)):lower()
    else return tostring(s[k] or ""):lower() end
end

local function sort_list(v)
    local k, desc = B.sort.key, B.sort.desc
    table.sort(v, function(a, b)
        local x, y = sort_value(a, k), sort_value(b, k)
        if x ~= y then
            -- unknown pings always last regardless of direction
            if k == "ping" and (x == 1e9 or y == 1e9) then return x < y end
            if desc then return x > y else return x < y end
        end
        -- deterministic tie-breakers: more players, lower ping, name, address
        if a.players ~= b.players then return a.players > b.players end
        local pa, pb = sort_value(a, "ping"), sort_value(b, "ping")
        if pa ~= pb then return pa < pb end
        local na, nb = tostring(a.name):lower(), tostring(b.name):lower()
        if na ~= nb then return na < nb end
        return key_of(a) < key_of(b)
    end)
end

local function internet_header()
    local q = B.q
    if master_problem(q.status) then return "INTERNET  -  server list unavailable" end
    if q.master_url then return "INTERNET  -  via " .. host_of(q.master_url) end
    return "INTERNET"
end

-- LAN servers get their own section ("LAN" header row first, then an
-- "INTERNET" header before the master / direct rows). With no LAN server in
-- view the table is the plain list.
local function rebuild_view()
    local lan, net = {}, {}
    for _, s in ipairs(B.servers) do
        if passes(s) then table.insert((s.source == "lan") and lan or net, s) end
    end
    sort_list(lan); sort_list(net)
    local v = {}
    if #lan > 0 then
        v[1] = { header = true, label = string.format("LAN  -  %d server%s on your network", #lan, #lan == 1 and "" or "s") }
        for _, s in ipairs(lan) do v[#v + 1] = s end
        if #net > 0 or master_problem(B.q.status) then
            v[#v + 1] = { header = true, label = internet_header() }
        end
    end
    for _, s in ipairs(net) do v[#v + 1] = s end
    B.view = v
    B.view_n = #lan + #net
    local pages = math.max(1, math.ceil(#v / ROWS))
    if B.page > pages then B.page = pages end
    if B.page < 1 then B.page = 1 end
    return pages
end

local function find_server(key)
    if not key then return nil end
    for _, s in ipairs(B.servers) do if key_of(s) == key then return s end end
    return nil
end

-- --- joining ------------------------------------------------------------------------

local function do_join(addr, map, label)
    if now() < B.joining_until then return end
    B.joining_until = now() + 3
    Log("browser JOIN %s (%s) map=%s", addr, tostring(label), tostring(map))
    ctx.join(addr, map, (label and label ~= "direct" and label ~= "recent") and label or addr)
end

function B.join_selected()
    local s = find_server(B.sel)
    if not s then flash("Select a server first (click a row).", C.warn); return end
    local blocked = ctx.blocked and ctx.blocked()
    if blocked then flash(blocked, C.bad, 3600); return end
    if not compatible(s) then
        flash(mismatch_text(s), C.bad, 6)
        return
    end
    if s.max > 0 and s.players >= s.max then
        flash("That server is full. REFRESH or pick another.", C.bad); return
    end
    -- There is no password entry yet; joining would only be rejected.
    if s.pwd then
        flash("That server needs a password, which HSMP cannot send yet. Pick another server.", C.bad, 5); return
    end
    if s.ping == -2 then
        Log("browser: joining %s although it did not answer the ping", key_of(s))
    end
    do_join(key_of(s), s.map, s.name)
end

local function parse_direct(text)
    local s = trim(text)
    if s == "" then return nil, "Type an address in the box first (ip:port)." end
    local h, p = s:match("^([%w%.%-]+):(%d+)$")
    if not h then
        h = s:match("^([%w%.%-]+)$"); p = "7777"
    end
    if not h then return nil, "Bad address. Use ip:port, e.g. 203.0.113.5:7777" end
    local port = tonumber(p)
    if not port or port < 1 or port > 65535 then return nil, "Port must be 1-65535." end
    if #h > 253 or h:sub(1, 1) == "." or h:sub(1, 1) == "-" then return nil, "Bad host name." end
    return h .. ":" .. port
end

function B.direct_connect()
    local text = ""
    local W = B.b and B.b.w
    if W and W.direct_box and Kit.alive(B.b) then
        text = Kit.input_text(W.direct_box) or ""
    end
    local addr, err = parse_direct(text)
    if not addr then flash(err, C.bad); return end
    remember_direct(addr)
    do_join(addr, "", "direct")
end

-- One click on a RECENT chip connects to that address (and moves it to the front).
function B.connect_recent(i)
    local addr = B.direct[i]
    if not addr or not parse_direct(addr) then return end
    addr = parse_direct(addr)
    Log("browser: recent chip %d -> %s", i, addr)
    remember_direct(addr)
    B.render()
    do_join(addr, "", "recent")
end


-- --- layout + build ------------------------------------------------------------------

local function set_filter(fn)
    return function()
        if not Kit.alive(B.b) then return end
        fn(); B.page = 1; save_prefs(); B.render()
    end
end

-- Layout (design units, Kit.frame):
--   SERVER BROWSER                                              [REFRESH]
--   [search] [HIDE FULL][HIDE EMPTY][HIDE LOCKED]  PING [ANY][<50][<100][<150]  [CLEAR]
--   PW | SERVER NAME | MAP | MODE | PLAYERS | PING | REGION / VER      (click = sort)
--   12 rows                       status bar               [< PREV] PAGE x / y [NEXT >]
--   RECENT [addr] x5
--   message line
--   [direct ip:port] [CONNECT]                                  [JOIN] [BACK]
--   keys                                                        focused help
function B.build()
    Kit = ctx.kit
    local L, bld = Kit.frame("browser", "SERVER BROWSER", { hint_extra = { "page", "refresh" } })
    B.b = bld
    local W = bld.w
    W.rows, W.heads = {}, {}
    local u, F = L.u, L.F
    local s = L.s
    local x0, iw = L.x0, L.iw
    local gap = L.gap
    local filt_h, head_h = u(Kit.BH.field), u(36)
    local stat_h, pg_h, rec_h = u(32), u(42), u(Kit.BH.tab)
    local vg = u(Kit.SP.sm)

    -- title row: REFRESH (right)
    local rw = u(Kit.BW.action)
    W.refresh = Kit.button("REFRESH", function() if Kit.alive(B.b) then B.refresh() end end,
        x0 + iw - rw, L.py + L.pad + math.floor((u(56) - filt_h) / 2), rw, filt_h,
        { fs = F(Kit.TS.label), group = "top", help = "Ask the server lists and the LAN again (F5)" })

    -- filter bar: search, toggle chips, ping chip group (exactly one on), clear
    local y = L.top
    local ffs = F(Kit.TS.label)
    Kit.group("filters")
    local sw = math.floor(iw * 0.18)
    W.search_box = Kit.input("Search server / map / mode...", B.f.search, x0, y, sw, filt_h, F(Kit.TS.body),
        { fkey = "browser:search", help = "Type part of a server, map or mode name" })
    W.search_last = B.f.search
    local bx = x0 + sw + gap
    local tw = math.floor(iw * 0.095)
    W.f_full = Kit.button("HIDE FULL", set_filter(function() B.f.hide_full = not B.f.hide_full end), bx, y, tw, filt_h,
        { fs = ffs, help = "Hide servers with no free slot" })
    bx = bx + tw + gap
    W.f_empty = Kit.button("HIDE EMPTY", set_filter(function() B.f.hide_empty = not B.f.hide_empty end), bx, y, tw, filt_h,
        { fs = ffs, help = "Hide servers nobody plays on" })
    bx = bx + tw + gap
    W.f_locked = Kit.button("HIDE LOCKED", set_filter(function() B.f.hide_locked = not B.f.hide_locked end), bx, y, tw, filt_h,
        { fs = ffs, help = "Hide password-protected servers" })
    bx = bx + tw + gap
    W.f_compat = Kit.button("COMPATIBLE", set_filter(function() B.f.compat_only = not B.f.compat_only end), bx, y, tw, filt_h,
        { fs = ffs, help = "Only servers this install can join (same version and mod files)" })
    bx = bx + tw + gap * 3
    local plw = math.floor(iw * 0.045)
    Kit.text("PING", bx, y, plw, filt_h, F(Kit.TS.small), 0, Kit.C.dim)
    bx = bx + plw
    local pcw = math.floor(iw * 0.058)
    W.f_ping = Kit.chip_group(PING_CHIPS, bx, y, 4 * pcw + 3 * gap, filt_h,
        function(k) set_filter(function() B.f.max_ping = k end)() end,
        { fs = ffs, gap = gap, help = "Only servers that answer faster than this (ms)" })
    bx = bx + 4 * pcw + 3 * gap + gap * 3
    W.f_clear = Kit.button("CLEAR", set_filter(function()
        B.f.search = ""; B.f.hide_full = false; B.f.hide_empty = false
        B.f.hide_locked = false; B.f.compat_only = false; B.f.max_ping = 0
        if W.search_box then Kit.input_set(W.search_box, "") end
        W.search_last = ""
    end), bx, y, x0 + iw - bx, filt_h, { fs = ffs, help = "Reset every filter" })
    W.f_clear.reason = "No filter is set"
    y = y + filt_h + vg

    -- column headers (click = sort)
    Kit.group("columns")
    local cx = x0
    W.col_x = {}
    for i, c in ipairs(COLS) do
        local cwid = (i == #COLS) and (x0 + iw - cx) or math.floor(iw * c.frac)
        W.col_x[i] = { x = cx, w = cwid }
        local key = c.key
        local h = Kit.button(c.label, function()
            if not Kit.alive(B.b) then return end
            if B.sort.key == key then B.sort.desc = not B.sort.desc
            else B.sort.key = key; B.sort.desc = SORT_DEFAULT_DESC[key] or false end
            B.page = 1; save_prefs(); B.render()
        end, cx, y, cwid - 2, head_h, { fs = F(Kit.TS.small), style = "head", just = c.just, label_pad = u(Kit.SP.sm),
            help = "Sort by " .. c.label:lower() .. " (again = reverse)" })
        if h then h.text_color = Kit.C.head end
        W.heads[i] = h
        cx = cx + cwid
    end
    y = y + head_h + 2

    -- rows: one flat button per row (select / double-click / Enter) with the column texts on top
    local fixed = stat_h + vg + pg_h + vg + rec_h
    local rh = math.max(u(18), math.min(u(40), math.floor((L.bottom - y - fixed) / ROWS) - 2))
    local fs = F(Kit.TS.body)
    if fs * Kit.LINE_H > rh then fs = math.max(Kit.minfs or 9, math.floor(rh / Kit.LINE_H)) end
    bld.layout = { px = L.px, py = L.py, pw = L.pw, ph = L.ph, rh = rh }
    Kit.group("rows")
    local ry0 = y
    for r = 1, ROWS do
        local ry = ry0 + (r - 1) * (rh + 2)
        local idx = r
        local row = Kit.button("", function() B.click_row(idx) end, x0, ry, iw, rh,
            { style = (r % 2 == 1) and "row" or "rowb", no_label = true, fkey = "browser:row" .. r,
              help = "ENTER or double-click joins this server" })
        local cells = {}
        for i, c in ipairs(COLS) do
            local col = W.col_x[i]
            cells[i] = Kit.text("", col.x + u(Kit.SP.sm), ry, col.w - 2 * u(Kit.SP.sm), rh, fs, c.just, Kit.C.text, { z = 14 })
        end
        row.texts = cells
        W.rows[r] = { btn = row, cells = cells }
    end
    local rows_end = ry0 + ROWS * (rh + 2)
    local efs = F(Kit.TS.h2)
    W.empty = Kit.spinner(x0, ry0 + 3 * (rh + 2), iw, math.max(rh, math.ceil(efs * Kit.LINE_H)), efs, 1, Kit.C.dim)

    -- status bar
    y = rows_end + vg
    Kit.rect(x0, y, iw, stat_h, Kit.C.section, 11)
    W.status = Kit.text("", x0 + u(Kit.SP.md), y, math.floor(iw * 0.62) - u(Kit.SP.md), stat_h, F(Kit.TS.small), 0, Kit.C.dim)
    W.selinfo = Kit.text("", x0 + math.floor(iw * 0.62), y, iw - math.floor(iw * 0.62) - u(Kit.SP.md), stat_h, F(Kit.TS.small), 2, Kit.C.dim)
    y = y + stat_h + vg

    -- paging
    Kit.group("pager")
    local pgw = math.floor(math.min(iw, u(560)))
    W.pager = Kit.pager(-math.floor(pgw / 2), y, pgw, pg_h,
        function() if Kit.alive(B.b) and B.page > 1 then B.page = B.page - 1; B.render() end end,
        function() if Kit.alive(B.b) then B.page = B.page + 1; B.render() end end,   -- clamped in rebuild_view
        { fs = F(Kit.TS.label), bw = u(160) })
    y = y + pg_h + vg

    -- recent direct-connect addresses: one click connects
    Kit.group("recent")
    local rlw = math.floor(iw * 0.13)
    W.recent_label = Kit.text("RECENT", x0, y, rlw - gap, rec_h, F(Kit.TS.small), 0, Kit.C.dim)
    W.recent = {}
    local rcx = x0 + rlw
    local rcw = math.floor((x0 + iw - rcx - (DIRECT_MAX - 1) * gap) / DIRECT_MAX)
    for i = 1, DIRECT_MAX do
        local idx = i
        local chip = Kit.button("", function() if Kit.alive(B.b) then B.connect_recent(idx) end end,
            rcx + (i - 1) * (rcw + gap), y, rcw, rec_h, { fs = F(Kit.TS.small), help = "Connect to this address again" })
        W.recent[i] = chip
    end

    -- bottom bar: direct connect (left), JOIN + BACK (right; BACK right-most)
    local acts = Kit.actions(L, nil, {
        { label = "BACK", key = "back", cb = function() if Kit.alive(B.b) then ctx.exit_screen() end end,
          help = "Back to the main menu" },
        { label = "JOIN", key = "join", style = "primary", cb = function() if Kit.alive(B.b) then B.join_selected() end end,
          help = "Join the selected server" } })
    W.join, W.back = acts.join, acts.back
    W.join.reason = "Select a server first (click a row or use the arrows)"
    W.join.on_disabled = function() if Kit.alive(B.b) then flash("Select a server first (click a row).", C.warn); B.render() end end
    Kit.group("direct")
    local dw = math.floor(iw * 0.24)
    local ih = u(Kit.BH.field)
    local iy = L.action_y + math.floor((L.action_h - ih) / 2)
    local draft = (Kit.relayout and B.direct_draft) or B.direct[1] or ""
    B.direct_draft = nil
    W.direct_box = Kit.input("Direct connect  ip:port", draft, x0, iy, dw, ih, F(Kit.TS.body),
        { fkey = "browser:direct", help = "A server address like 203.0.113.5:7777 - ENTER connects",
          on_submit = function(text) if Kit.alive(B.b) and trim(text) ~= "" then B.direct_connect() end end })
    W.connect = Kit.button("CONNECT", function() if Kit.alive(B.b) then B.direct_connect() end end,
        x0 + dw + gap, iy, u(150), ih, { fs = F(Kit.TS.label), help = "Connect to the typed address" })

    -- keys: Enter on a row joins it; PgUp/PgDn page; F5 refreshes; Esc goes back
    bld.on_focus = function(ref, how)
        if how ~= "key" then return end
        for _, row in ipairs(W.rows) do
            if row.btn == ref and row.server_key then B.sel = row.server_key; B.render(); return end
        end
    end
    Kit.on_key("accept", function()
        local f = Kit.focused()
        for _, row in ipairs(W.rows) do
            if row.btn == f and row.server_key then
                B.sel = row.server_key
                B.render()
                B.join_selected()
                return true
            end
        end
        return false
    end)
    bld.on_page = function(d)
        local pages = math.max(1, math.ceil(#B.view / ROWS))
        B.page = Kit.clamp(B.page + d, 1, pages)
        B.render()
    end
    bld.on_refresh = function() B.refresh() end
    bld.on_back = function() ctx.exit_screen() end
    -- a resize rebuild keeps the typed search + direct address
    bld.before_rebuild = function()
        if not Kit.alive(bld) then return end
        local s = Kit.input_text(W.search_box)
        if s then B.f.search = s end
        B.direct_draft = Kit.input_text(W.direct_box)
    end

    Kit.on_tick(2, function() B.tick() end)          -- 100 ms: query poll, search box, status age

    Log("browser build #%d: canvas %.0fx%.0f scale %.2f panel %dx%d rows=%d rh=%d widgets=%d",
        bld.serial, L.cw, L.ch, s, L.pw, L.ph, ROWS, rh, bld.n)

    -- these mod files do not match the helper programs: say so for as long as the screen is open
    local blocked = ctx.blocked and ctx.blocked()
    if blocked then flash(blocked, C.bad, 3600) end
    B.render()
    Kit.focus_default(W.rows[1].btn)
    if not B.q.inflight and (not B.q.last_ok or os.time() - B.q.last_ok >= AUTO_REFRESH_S) then
        B.refresh()
    end
end

-- --- rendering ---------------------------------------------------------------------

local function ping_cell(s)
    if s.ping == -3 then return "n/a", C.dim end
    if s.ping == -1 or s.ping == nil then return "...", C.dim end
    if s.ping < 0 then return "--", C.bad end
    local c = (s.ping < 80 and C.good) or (s.ping < 150 and C.ok) or (s.ping < 250 and C.warn) or C.bad
    return tostring(math.floor(s.ping)), c
end

local function map_name(m)
    if not m or m == "" then return "-" end
    local ok, n = pcall(ctx.map_display_name, m)
    if ok and n and n ~= m then return n end
    return (m:gsub("^Map_Arena_", ""):gsub("_", " "))
end

-- "via master.example.net" / "via backup.example.net (fallback 2/3)"
local function via_text()
    local q = B.q
    if not q.master_url then return nil end
    local s = "via " .. host_of(q.master_url)
    if q.master_idx and q.master_idx > 1 then
        s = s .. string.format(" (fallback %d/%d)", q.master_idx, q.master_n or #(q.urls or {}))
    end
    return s
end

local function unavailable_text(n)
    return string.format("Server list unavailable (%s). Use DIRECT CONNECT or LAN", clip(B.q.msg ~= "" and B.q.msg or B.q.status, n or 60))
end

local function status_line()
    local q = B.q
    if B.flash and now() < B.flash.until_t then return B.flash.text, B.flash.color end
    local age = q.last_ok and (os.time() - q.last_ok) or nil
    local age_s = age and (age < 2 and "just now" or (age .. "s ago")) or "never"
    if q.inflight and q.trying then return "Contacting server list " .. clip(q.trying, 70) .. "...", C.dim end
    if q.inflight and not q.have_list then
        local n = #(q.urls or {})
        return (n > 1) and string.format("Contacting server list (%d to try)...", n) or "Contacting server list...", C.dim
    end
    if master_problem(q.status) then
        local lan = q.lan_on and string.format("  |  LAN: %d found", q.lan_n or 0) or ""
        if q.have_list and (q.lan_n or 0) == 0 and #B.servers > 0 and not q.lan_on then
            return string.format("%s - showing list from %s", unavailable_text(36), age_s), C.bad
        end
        return unavailable_text(36) .. lan, C.bad
    end
    local shown = B.view_n or #B.view
    local s = string.format("Updated %s  |  %d server%s, %d shown", age_s, #B.servers,
        (#B.servers == 1) and "" or "s", shown)
    local via = via_text()
    if via then s = s .. "  |  " .. via end
    if q.lan_on then s = s .. string.format("  |  LAN %d", q.lan_n or 0) end
    if q.pings_pending or q.inflight then s = s .. "  |  measuring ping..." end
    if q.skipped and q.skipped > 0 then s = s .. string.format("  |  %d bad entr%s ignored", q.skipped, q.skipped == 1 and "y" or "ies") end
    if q.mode == "curl" then s = s .. "  |  no ping (hsmp-query.exe missing)" end
    return s, C.dim
end

-- The empty-table line and whether it is a loading state (spinner).
local function empty_line()
    local q = B.q
    if #B.view > 0 then return "", false end
    if q.inflight and (not q.have_list or q.trying) then return "Loading servers...", true end
    if master_problem(q.status) and (not q.have_list or #B.servers == 0 or q.lan_on) then
        return unavailable_text(), false
    end
    if not q.have_list then
        if q.status == "none" then return "", false end
        return "No servers found. Check your connection, then REFRESH or DIRECT CONNECT.", false
    end
    if #B.servers == 0 then
        return "No servers found. REFRESH or DIRECT CONNECT.", false
    end
    return string.format("No servers match your filters (%d hidden). Press CLEAR.", #B.servers), false
end

function B.render()
    local bld = B.b
    if not Kit or not bld or not Kit.alive(bld) then return end
    local W = bld.w
    local pages = rebuild_view()
    local my_region = (ctx.my_region and ctx.my_region()) or ""

    -- filters: toggle chips + ping chip group (exactly one active)
    Kit.set_state(W.f_full, B.f.hide_full, false)
    Kit.set_state(W.f_empty, B.f.hide_empty, false)
    Kit.set_state(W.f_locked, B.f.hide_locked, false)
    Kit.set_state(W.f_compat, B.f.compat_only, false)
    Kit.chip_group_set(W.f_ping, B.f.max_ping, false)
    local any = B.f.hide_full or B.f.hide_empty or B.f.hide_locked or B.f.compat_only or B.f.max_ping > 0 or trim(B.f.search) ~= ""
    Kit.set_state(W.f_clear, false, not any)

    -- headers with sort arrow
    for i, c in ipairs(COLS) do
        local h = W.heads[i]
        if h then
            local lbl = c.label
            if B.sort.key == c.key then lbl = lbl .. (B.sort.desc and " v" or " ^") end
            Kit.set_label(h, lbl)
            Kit.set_state(h, B.sort.key == c.key, false)
        end
    end

    -- rows
    local first = (B.page - 1) * ROWS
    for r = 1, ROWS do
        local row = W.rows[r]
        local s = B.view[first + r]
        if row then
            local hdr = s and s.header
            row.server_key = (s and not hdr) and key_of(s) or nil
            row.btn.skip_focus = (s == nil) or hdr or nil
            Kit.show(row.btn, s ~= nil)
            Kit.set_style(row.btn, hdr and "head" or ((r % 2 == 1) and "row" or "rowb"))
            if hdr then
                -- section header row (LAN / INTERNET): not selectable
                Kit.set_state(row.btn, false, false)
                local cells = row.cells
                for i = 1, #cells do Kit.set_text(cells[i], "") end
                Kit.set_text(cells[2], s.label)
                Kit.set_color(cells[2], Kit.C.head)
            elseif s then
                local sel = (B.sel == key_of(s))
                local compat, why = compatible(s)
                Kit.set_state(row.btn, sel, false)
                local base = (not compat) and C.dim or (sel and Kit.C.on_text or C.text)
                local cells = row.cells
                Kit.set_text(cells[1], s.pwd and "PW" or "")
                Kit.set_color(cells[1], sel and Kit.C.on_text or C.ok)
                Kit.set_text(cells[2], ascii(s.name) .. (s.source == "direct" and "  (direct)" or ""))
                Kit.set_color(cells[2], base)
                Kit.set_text(cells[3], map_name(s.map)); Kit.set_color(cells[3], base)
                Kit.set_text(cells[4], s.mode ~= "" and s.mode or "-"); Kit.set_color(cells[4], base)
                local full = s.max > 0 and s.players >= s.max
                Kit.set_text(cells[5], string.format("%d/%s", s.players, s.max > 0 and tostring(s.max) or "?"))
                Kit.set_color(cells[5], (full and not sel) and C.bad or base)
                local pt, pc = ping_cell(s)
                Kit.set_text(cells[6], pt); Kit.set_color(cells[6], sel and Kit.C.on_text or pc)
                local reg = (s.region ~= "" and s.region) or ((s.source == "lan") and "LAN") or "--"
                local ver = (s.version ~= "" and ("v" .. s.version)) or ""
                if not compat then
                    Kit.set_text(cells[7], reg .. "  " .. clip(why or "incompatible", 22)); Kit.set_color(cells[7], C.bad)
                else
                    Kit.set_text(cells[7], reg .. "  " .. ver)
                    local mine = my_region ~= "" and tostring(s.region):upper() == my_region:upper()
                    Kit.set_color(cells[7], (mine and not sel) and Kit.C.head or base)
                end
            end
        end
    end
    local el, loading = empty_line()
    Kit.spinner_set(W.empty, loading, el, Kit.C.dim)
    Kit.show_text(W.empty, #B.view == 0)

    -- recent direct-connect chips
    if W.recent then
        Kit.set_text_fit(W.recent_label, (#B.direct > 0) and { "RECENT" } or { "RECENT: none yet", "RECENT: none", "RECENT" })
        for i, chip in ipairs(W.recent) do
            local addr = B.direct[i]
            Kit.set_label(chip, addr or "")
            Kit.show(chip, addr ~= nil)
        end
    end

    -- status / selection / paging
    local st, sc = status_line()
    Kit.set_text(W.status, st); Kit.set_color(W.status, sc)
    local s = find_server(B.sel)
    if s then
        Kit.set_text_fit(W.selinfo, {
            string.format("SELECTED: %s (%s)  -  ENTER, double-click or JOIN", ascii(s.name), key_of(s)),
            string.format("SELECTED: %s (%s)", ascii(s.name), key_of(s)),
            string.format("SELECTED: %s", ascii(s.name)) })
        Kit.set_color(W.selinfo, C.text)
    else
        Kit.set_text(W.selinfo, ((B.view_n or 0) > 0) and "Pick a server (arrows or click)" or "")
        Kit.set_color(W.selinfo, C.dim)
    end
    Kit.pager_set(W.pager, B.page, pages)
    Kit.set_state(W.join, false, s == nil)
    Kit.set_state(W.refresh, B.q.inflight, false)
    Kit.set_label(W.refresh, B.q.inflight and "REFRESHING..." or "REFRESH")
    -- message line: joining / the last problem / a hint
    if now() < B.joining_until then
        Kit.msg("Joining... the lobby opens when the server answers", Kit.C.ok)
    elseif B.flash and now() < B.flash.until_t then
        Kit.msg(B.flash.text, B.flash.color)
    else
        Kit.msg(s and "ENTER or JOIN to join the selected server." or "Arrows pick a server, ENTER joins. Or type an address below.", Kit.C.dim)
    end
end

-- Single click selects; a second click on the same row (same server, same
-- page, same build) within DOUBLE_CLICK_S joins. main.lua polls IsPressed
-- every 33 ms and fires on the rising edge only, so two clicks are always at
-- least one release apart.
function B.click_row(r)
    local bld = B.b
    if not bld or not Kit.alive(bld) then return end
    local row = bld.w.rows[r]; if not row or not row.server_key then return end
    local key = row.server_key
    local t = now()
    local lc = B.last_click
    local dt = t - (lc.t or 0)
    if lc.key == key and lc.page == B.page and lc.serial == bld.serial
        and dt <= DOUBLE_CLICK_S and dt >= DOUBLE_CLICK_MIN_S then
        B.last_click = { key = nil, t = 0 }
        B.sel = key
        B.render()
        Log("browser: double-click row %d -> join %s", r, key)
        B.join_selected()
        return
    end
    B.last_click = { key = key, t = t, page = B.page, serial = bld.serial }
    B.sel = key
    B.render()
end

-- --- game-thread tick (ui_kit per-screen tick, 100 ms) ----------------------------------

local tick_n = 0
function B.tick()
    local bld = B.b
    if not ctx or not Kit or not bld or not Kit.alive(bld) then return end
    tick_n = tick_n + 1
    local dirty = poll_query()

    -- search box (debounced 300 ms; OnTextChanged is not Lua-bindable)
    local W = bld.w
    if tick_n % 3 == 0 and W.search_box then
        local txt = Kit.input_text(W.search_box)
        if txt and txt ~= W.search_last then
            W.search_last = txt
            B.f.search = txt:sub(1, 64)
            B.page = 1
            dirty = true
            B.prefs_dirty_at = now()
        end
    end
    if B.prefs_dirty_at and now() - B.prefs_dirty_at > 1.5 then
        B.prefs_dirty_at = nil; save_prefs()
    end
    -- age / flash text refresh once a second
    if tick_n % 10 == 0 then dirty = true end
    if dirty then B.render() end
end

-- Screen exit / world change: drop the widget refs without touching them.
function B.forget()
    B.b = nil
end

-- --- init ------------------------------------------------------------------------------

-- ctx fields: state, Log, STATE_DIR, MASTER_URL, MASTER_URLS, QUERY_EXE, LAN, LAN_PORTS, kit,
-- map_display_name, my_region(), join(addr, map, label), exit_screen, build (shared/hsmp_build.lua),
-- build_id() (the mods' identity or nil), blocked() (the build-check message or nil)
function B.init(c)
    ctx = c
    Kit = c.kit
    Log = c.Log or Log
    for _, k in ipairs({ "text", "dim", "head", "good", "ok", "warn", "bad" }) do C[k] = Kit.C[k] end
    C.sel_text = Kit.C.on_text
    pcall(load_prefs)
    Log("browser module ready (query exe: %s, masters: %s, lan: %s %s)", tostring(c.QUERY_EXE),
        table.concat(c.MASTER_URLS or { tostring(c.MASTER_URL) }, ", "), tostring(c.LAN ~= false), lan_ports())
end

-- exposed for offline tests
B._parse_direct = parse_direct
B._split_tabs = split_tabs
B._passes = passes
B._rebuild_view = rebuild_view
B._accept_result = accept_result
B._parse_tool_text = parse_tool_text
B._parse_curl_text = parse_curl_text
B._poll_query = poll_query
B._master_urls = master_urls
B.ROWS = ROWS
B.DIRECT_MAX = DIRECT_MAX

return B
