-- HSMPMenu / settings.lua — the player's MP settings: storage, validation and
-- the SETTINGS screen.
--
-- Storage: <state>/.settings.json, ONE line (HSMPSync / HSMPAvatars / HSMPHud
-- read its first line), written atomically. Keys other mods own (for example
-- "pose_stiffness") are preserved on every save.
--
--   key            type    used by
--   nick           string  sidecar --nick, lobby, HSMPHud ("my name")
--   server         string  "127.0.0.1:<port>": the port a hosted server binds
--   send_hz        number  HSMPSync pose/root send rate (30/60/90/120)
--   region         string  "" (auto) | EU | NA | SA | ASIA | OCE: HSMP_REGION of a hosted server
--   master_url     string  "" (hsmp.cfg default) | "https://a, https://b": server lists, in order
--   hud            bool    HSMPHud master switch
--   hud_killfeed   bool    HSMPHud kill feed
--   hud_net        string  HSMPHud net indicator: always | bad | off
--   avatars        bool    HSMPAvatars: show peer avatars
--   upnp           bool    a hosted server opens its port on the router (UPnP / PCP / NAT-PMP);
--                          false = hsmp-server --port-map off
--   server_mods    string  "ask" (default) | "never": servers that serve mods (server_mods.lua)
--   lobby_map, lobby_mode  the next HOST's boot arena / rounds ("Best of N", re-sent on connect)
--   host_kit_mode, host_kit_budget  the HOST's last KIT RULES pick (-1 = none), re-sent
--                  on connect as host
--
-- Screen: every control is staged; SAVE & BACK validates and writes, BACK
-- discards (with an inline confirm when something changed). Typed fields are
-- validated live (frame colour + a line under the field). TEST asks every
-- listed server list (async curl, 8 s deadline) and reports per URL.

local S = {}

local ctx            -- { kit, log, path, state_dir, exit_screen(), default_urls() -> {..}, on_saved(changed) }
local Kit, J
local Log = function() end

S.HZ = { 30, 60, 90, 120 }
S.REGIONS = { { "", "AUTO" }, { "EU", "EU" }, { "NA", "NA" }, { "SA", "SA" }, { "ASIA", "ASIA" }, { "OCE", "OCE" } }
S.NET = { { "always", "ALWAYS" }, { "bad", "WHEN BAD" }, { "off", "OFF" } }
S.NICK_MIN, S.NICK_MAX = 2, 20
S.MAX_URLS = 4

S.DEFAULTS = {
    nick = "Willie", server = "127.0.0.1:7777", send_hz = 60,
    region = "", master_url = "",
    hud = true, hud_killfeed = true, hud_net = "always", avatars = true, upnp = true,
    lobby_map = "Map_Arena_Alley", lobby_mode = "Best of 3",
    host_kit_mode = -1, host_kit_budget = 0, server_mods = "ask",
}
local ORDER = { "nick", "server", "send_hz", "hud", "avatars", "lobby_map", "lobby_mode",
                "region", "master_url", "hud_killfeed", "hud_net", "host_kit_mode", "host_kit_budget", "upnp", "server_mods" }

S.data = {}          -- the live settings (main.lua keeps a reference: never replace the table)
S.extra = {}         -- keys owned by other mods, written back unchanged

local function trim(s) return (tostring(s or ""):gsub("^%s+", ""):gsub("%s+$", "")) end

-- --- validation ---------------------------------------------------------------------

-- Nickname: 2-20 of letters, digits, space . _ - ; starts with a letter or digit.
function S.nick_check(s)
    s = trim(s)
    if #s < S.NICK_MIN then return false, string.format("Too short - use at least %d characters", S.NICK_MIN) end
    if #s > S.NICK_MAX then return false, string.format("Too long - at most %d characters (%d now)", S.NICK_MAX, #s) end
    if not s:match("^[%w _%-%.]+$") then return false, "Only letters, digits, space and . _ - are allowed" end
    if not s:match("^[%w]") then return false, "Start with a letter or a digit" end
    if s:find("  ", 1, true) then return false, "No double spaces" end
    return true, "OK", s
end

function S.safe_url(u)
    u = trim(u)
    if #u < 200 and u:match("^https?://[%w%.%-]+[:%d]*[%w%./%-_]*$") then return (u:gsub("/+$", "")) end
    return nil
end

-- "a, b" -> ok, msg, { a, b }   ("" = use the hsmp.cfg default list)
function S.urls_check(s)
    s = trim(s)
    if s == "" then return true, "Empty - the default list from hsmp.cfg is used", {} end
    local out, seen, i = {}, {}, 0
    for part in (s .. ","):gmatch("([^,]*),") do
        local p = trim(part)
        if p ~= "" then
            i = i + 1
            local u = S.safe_url(p)
            if not u then
                return false, string.format("Entry %d is not a URL like https://host:port", i), nil
            end
            if not seen[u] then seen[u] = true; out[#out + 1] = u end
        end
    end
    if #out == 0 then return false, "Type at least one URL, or leave the field empty", nil end
    if #out > S.MAX_URLS then return false, string.format("At most %d server lists", S.MAX_URLS), nil end
    return true, string.format("%d server list%s", #out, #out == 1 and "" or "s"), out
end

function S.port_check(s)
    s = trim(s)
    local n = tonumber(s)
    if not s:match("^%d+$") or not n then return false, "Numbers only, e.g. 7777", nil end
    if n < 1024 or n > 65535 then return false, "Use a port from 1024 to 65535", nil end
    return true, "OK", math.floor(n)
end

local function port_of(addr) return tonumber(tostring(addr or ""):match(":(%d+)$")) end

-- --- storage --------------------------------------------------------------------------

local function sanitize(d)
    if type(d.nick) ~= "string" or #d.nick < 1 or #d.nick > 24 or not d.nick:match("^[%w _%-%.]+$") then
        d.nick = S.DEFAULTS.nick
    end
    local p = port_of(d.server)
    if type(d.server) ~= "string" or not d.server:match("^[%w%.%-]+:%d+$") or not p then d.server = S.DEFAULTS.server end
    local hz_ok = false
    for _, v in ipairs(S.HZ) do if d.send_hz == v then hz_ok = true end end
    if not hz_ok then d.send_hz = S.DEFAULTS.send_hz end
    local reg_ok = false
    for _, r in ipairs(S.REGIONS) do if d.region == r[1] then reg_ok = true end end
    if not reg_ok then d.region = "" end
    if type(d.master_url) ~= "string" or not S.urls_check(d.master_url) then d.master_url = "" end
    local net_ok = false
    for _, r in ipairs(S.NET) do if d.hud_net == r[1] then net_ok = true end end
    if not net_ok then d.hud_net = "always" end
    local km = tonumber(d.host_kit_mode)
    if not km or km ~= math.floor(km) or km < -1 or km > 2 then d.host_kit_mode = -1 end
    local kb = tonumber(d.host_kit_budget)
    if not kb or kb < 0 or kb > 1000 then d.host_kit_budget = 0 end
    if d.server_mods ~= "never" then d.server_mods = "ask" end
end

-- Old files were written by pattern; read them the same tolerant way.
local function legacy_parse(line)
    local t = {}
    t.nick = line:match('"nick"%s*:%s*"([^"]*)"')
    t.server = line:match('"server"%s*:%s*"([^"]*)"')
    t.send_hz = tonumber(line:match('"send_hz"%s*:%s*(%-?%d+)'))
    for _, k in ipairs({ "hud", "avatars", "hud_killfeed" }) do
        local v = line:match('"' .. k .. '"%s*:%s*(%a+)')
        if v then t[k] = (v == "true") end
    end
    t.lobby_map = line:match('"lobby_map"%s*:%s*"([^"]+)"')
    t.lobby_mode = line:match('"lobby_mode"%s*:%s*"([^"]+)"')
    return t
end

function S.load()
    local d = S.data
    for k in pairs(d) do d[k] = nil end
    for k, v in pairs(S.DEFAULTS) do d[k] = v end
    S.extra = {}
    local f = io.open(ctx.path, "rb")
    if f then
        local raw = f:read("*a") or ""; f:close()
        local line = raw:match("[^\r\n]+") or ""
        local t = J.decode(line)
        if type(t) ~= "table" then t = legacy_parse(line) end
        for k, v in pairs(t) do
            if S.DEFAULTS[k] ~= nil then
                if type(v) == type(S.DEFAULTS[k]) then d[k] = v end
            elseif type(k) == "string" and (type(v) == "string" or type(v) == "number" or type(v) == "boolean") then
                S.extra[k] = v
            end
        end
    end
    sanitize(d)
    return d
end

function S.save()
    local t = {}
    -- Keys other mods / the player added since load (pose_stiffness,
    -- interact_*) are re-read and kept, not reverted to the load-time copy.
    local f = io.open(ctx.path, "rb")
    if f then
        local cur = J.decode(((f:read("*a") or ""):match("[^\r\n]+")) or "")
        f:close()
        if type(cur) == "table" then
            for k, v in pairs(cur) do
                if S.DEFAULTS[k] == nil and type(k) == "string"
                    and (type(v) == "string" or type(v) == "number" or type(v) == "boolean") then
                    S.extra[k] = v
                end
            end
        end
    end
    for k, v in pairs(S.extra) do t[k] = v end
    for k, v in pairs(S.data) do t[k] = v end
    local ok = J.write_atomic(ctx.path, J.flat(t, ORDER))
    if not ok then Log("settings: cannot write %s", tostring(ctx.path)) end
    return ok
end

-- The server lists to use: the saved ones, else hsmp.cfg's.
function S.master_urls()
    local ok, _, list = S.urls_check(S.data.master_url or "")
    if ok and list and #list > 0 then return list end
    return (ctx and ctx.default_urls and ctx.default_urls()) or {}
end

-- --- the SETTINGS screen -------------------------------------------------------------------

local b                -- current build (dropped on exit / world change)
local edit             -- staged copy of S.data
local test = nil       -- master URL TEST: { gen, urls, started, done, results = { {url, state, n} } }
local TEST_DEADLINE_S = 8

local FIELDS = { "nick", "server", "send_hz", "region", "master_url", "hud", "hud_killfeed", "hud_net", "avatars", "upnp", "server_mods" }

local function dirty()
    if not edit then return false end
    for _, k in ipairs(FIELDS) do
        local a, c = edit[k], S.data[k]
        if type(a) == "string" then a = trim(a) end
        if type(c) == "string" then c = trim(c) end
        if a ~= c then return true end
    end
    return false
end

-- Read the typed fields into `edit`; returns the three verdicts.
local function read_inputs()
    local w = b.w
    local n = Kit.input_text(w.nick); if n then edit.nick = n end
    local u = Kit.input_text(w.master); if u then edit.master_url = u end
    local p = Kit.input_text(w.port)
    if p then edit.port_text = p end
    local nok, nmsg = S.nick_check(edit.nick)
    local uok, umsg = S.urls_check(edit.master_url)
    local pok, pmsg, port = S.port_check(edit.port_text or "")
    if pok then edit.server = "127.0.0.1:" .. port end
    return { nick = { nok, nmsg }, urls = { uok, umsg }, port = { pok, pmsg } }
end

local function first_invalid(v)
    if not v.nick[1] then return "NICKNAME: " .. v.nick[2] end
    if not v.urls[1] then return "SERVER LISTS: " .. v.urls[2] end
    if not v.port[1] then return "HOST PORT: " .. v.port[2] end
    return nil
end

-- One curl.exe per server list, through the native module:
-- IPC.spawn_capture reads the body through a pipe, IPC.capture_poll reports the
-- exit (0 = answered). No state-dir file, never blocks the game thread.
local function ipc() return rawget(_G, "HSMP_IPC") end

local function stop_test()
    if not test or not ipc() then return end
    for _, r in ipairs(test.results) do
        if r.state == "pending" and r.h then pcall(ipc().proc_kill, r.h) end
    end
end

local function run_test()
    local ok, msg, list = S.urls_check(edit.master_url)
    if not ok then Kit.flash("Fix the server list first: " .. msg, Kit.C.bad); return end
    if #list == 0 then list = (ctx.default_urls and ctx.default_urls()) or {} end
    if #list == 0 then Kit.flash("No server list to test", Kit.C.bad); return end
    stop_test()
    test = { urls = list, started = Kit.now(), done = false, results = {} }
    local I = ipc()
    for i, u in ipairs(list) do
        local r = { url = u, state = "pending", n = 0 }
        test.results[i] = r
        local h, err
        if I then h, err = I.spawn_capture("curl.exe", { "-s", "-f", "-m", "5", u .. "/v1/servers" })
        else err = "shared/hsmp_ipc.lua missing" end
        if h then r.h = h else r.state = "fail"; Log("settings: TEST %s: curl did not start (%s)", u, tostring(err)) end
    end
    Log("settings: TEST %d server list(s): %s", #list, table.concat(list, ", "))
end

local function host_of(u) return (tostring(u):match("^https?://([^/]+)") or tostring(u)) end

local function poll_test()
    if not test or test.done then return end
    local pending = 0
    local I = ipc()
    for _, r in ipairs(test.results) do
        if r.state == "pending" then
            local fin, code, s
            if I and r.h then fin, code, s = I.capture_poll(r.h) end
            if fin and code == 0 then
                local n = 0
                for _ in tostring(s or ""):gmatch('"host"%s*:') do n = n + 1 end
                r.state, r.n, r.h = "ok", n, nil
            elseif fin or fin == nil then
                r.state, r.h = "fail", nil         -- curl failed (or the handle is unknown)
            else
                pending = pending + 1
            end
        end
    end
    if pending > 0 and Kit.now() - test.started >= TEST_DEADLINE_S then
        stop_test()
        for _, r in ipairs(test.results) do if r.state == "pending" then r.state = "timeout"; r.h = nil end end
        pending = 0
    end
    if pending == 0 then
        test.done = true
        local parts = {}
        for _, r in ipairs(test.results) do parts[#parts + 1] = host_of(r.url) .. "=" .. r.state end
        Log("settings: TEST result %s", table.concat(parts, " "))
    end
end

local function test_line()
    if not test then return "TEST asks each server list now (5 s timeout)", Kit.C.dim, false end
    if not test.done then
        return string.format("Testing %d server list%s...", #test.urls, #test.urls == 1 and "" or "s"), Kit.C.ok, true
    end
    local good, parts, bad = 0, {}, nil
    for _, r in ipairs(test.results) do
        if r.state == "ok" then
            good = good + 1
            parts[#parts + 1] = string.format("%s OK (%d server%s)", host_of(r.url), r.n, r.n == 1 and "" or "s")
        else
            bad = bad or host_of(r.url)
            parts[#parts + 1] = host_of(r.url) .. " NO ANSWER"
        end
    end
    if good == #test.results then return table.concat(parts, "  |  "), Kit.C.good, false end
    if good == 0 then
        return string.format("No answer from %s - check the URL and your connection", bad), Kit.C.bad, false
    end
    return table.concat(parts, "  |  "), Kit.C.warn, false
end

function S.render()
    if not b or not Kit.alive(b) or not edit then return end
    local w = b.w
    local v = read_inputs()
    -- typed fields: frame colour + the line under each
    Kit.input_state(w.nick, v.nick[1] and Kit.C.good or Kit.C.bad)
    Kit.set_text(w.nick_note, v.nick[1] and string.format("%d / %d characters", #trim(edit.nick), S.NICK_MAX) or v.nick[2])
    Kit.set_color(w.nick_note, v.nick[1] and Kit.C.dim or Kit.C.bad)
    Kit.input_state(w.master, v.urls[1] and Kit.C.rule or Kit.C.bad)
    local tl, tc, busy = test_line()
    if not v.urls[1] then tl, tc, busy = v.urls[2], Kit.C.bad, false
    elseif not test and trim(edit.master_url) == "" then
        local d = (ctx.default_urls and ctx.default_urls()) or {}
        tl = "Using the default: " .. ((#d > 0) and table.concat(d, ", ") or "none")
        tc = Kit.C.dim
    end
    Kit.spinner_set(w.master_note, busy, tl, tc)
    Kit.set_state(w.test, busy, not v.urls[1])
    Kit.set_label(w.test, busy and "TESTING..." or "TEST")
    Kit.input_state(w.port, v.port[1] and Kit.C.rule or Kit.C.bad)
    Kit.set_text(w.port_note, v.port[1] and "UDP port your hosted game listens on" or v.port[2])
    Kit.set_color(w.port_note, v.port[1] and Kit.C.dim or Kit.C.bad)
    -- chips
    Kit.chip_group_set(w.region, edit.region, false)
    Kit.chip_group_set(w.hz, edit.send_hz, false)
    Kit.chip_group_set(w.hud, edit.hud, false)
    Kit.chip_group_set(w.feed, edit.hud_killfeed, not edit.hud)
    Kit.chip_group_set(w.net, edit.hud_net, not edit.hud)
    Kit.chip_group_reason(w.feed, "The HUD is OFF - turn HUD ON first")
    Kit.chip_group_reason(w.net, "The HUD is OFF - turn HUD ON first")
    Kit.chip_group_set(w.av, edit.avatars, false)
    Kit.chip_group_set(w.upnp, edit.upnp, false)
    Kit.chip_group_set(w.smods, edit.server_mods, false)
    -- save state
    local bad = first_invalid(v)
    local d = dirty()
    Kit.set_state(w.save, false, bad ~= nil)
    w.save.reason = bad
    if b.msg_t and Kit.now() < b.msg_t then
        -- a recent message (reset / test) stays
    elseif bad then Kit.msg(bad, Kit.C.bad)
    elseif d then Kit.msg("Unsaved changes - SAVE & BACK keeps them, BACK discards them", Kit.C.warn)
    else Kit.msg("Everything saved.", Kit.C.dim) end
    Kit.status(d and "UNSAVED CHANGES" or "SAVED", d and Kit.C.warn or Kit.C.dim, false)
end

local function set_edit(k, v)
    if not b or not Kit.alive(b) then return end
    edit[k] = v
    S.render()
end

local function save_and_back()
    if not b or not Kit.alive(b) then return end
    local v = read_inputs()
    local bad = first_invalid(v)
    if bad then Kit.flash(bad, Kit.C.bad); S.render(); return end
    local changed = {}
    for _, k in ipairs(FIELDS) do
        local nv = edit[k]
        if type(nv) == "string" then nv = trim(nv) end
        if nv ~= S.data[k] then changed[k] = true; S.data[k] = nv end
    end
    -- normalise the list (dedup, no trailing slashes)
    local _, _, list = S.urls_check(S.data.master_url)
    S.data.master_url = table.concat(list or {}, ", ")
    if not S.save() then Kit.flash("Could not write the settings file", Kit.C.bad); return end
    Log("settings saved: nick=%s port=%s hz=%d region=%s masters=%s hud=%s/%s/%s avatars=%s",
        S.data.nick, tostring(port_of(S.data.server)), S.data.send_hz, S.data.region ~= "" and S.data.region or "auto",
        S.data.master_url ~= "" and S.data.master_url or "default", tostring(S.data.hud), tostring(S.data.hud_killfeed),
        S.data.hud_net, tostring(S.data.avatars))
    if ctx.on_saved then pcall(ctx.on_saved, changed) end
    edit = nil
    ctx.exit_screen()
end

local function back()
    if not b or not Kit.alive(b) then return end
    read_inputs()
    if dirty() then
        Kit.confirm("Discard your unsaved changes?", "DISCARD", function() edit = nil; ctx.exit_screen() end,
            { no_label = "KEEP EDITING" })
        return
    end
    edit = nil
    ctx.exit_screen()
end

local function reset_defaults()
    if not b or not Kit.alive(b) then return end
    for _, k in ipairs(FIELDS) do if k ~= "nick" then edit[k] = S.DEFAULTS[k] end end
    edit.port_text = tostring(port_of(S.DEFAULTS.server))
    Kit.input_set(b.w.master, "")
    Kit.input_set(b.w.port, edit.port_text)
    b.msg_t = Kit.now() + 4
    Kit.msg("Defaults restored (your nickname is kept) - SAVE & BACK keeps them", Kit.C.ok)
    S.render()
end

local function onoff(L, x, y, w, h, key, group, help)
    return Kit.chip_group({ { true, "ON" }, { false, "OFF" } }, x, y, w, h,
        function(v) set_edit(key, v) end, { fs = L.F(Kit.TS.label), gap = L.gap, group = group, help = help })
end

function S.build()
    Kit = ctx.kit
    -- a resize rebuild (Kit.relayout) keeps the unsaved edits and the typed text
    local keep = Kit.relayout and edit ~= nil
    if not keep then
        edit = {}
        for _, k in ipairs(FIELDS) do edit[k] = S.data[k] end
        edit.port_text = tostring(port_of(S.data.server) or 7777)
        test = nil
    end
    local L
    L, b = Kit.frame("settings", "SETTINGS", { w = 1500, h = 860 })
    local w = b.w
    local u, F = L.u, L.F
    local gap = L.gap
    local colgap = u(Kit.SP.xxl) + u(Kit.SP.sm)
    local lw = math.floor((L.iw - colgap) * 0.60)
    local rw = L.iw - colgap - lw
    local lx, rx = L.x0, L.x0 + lw + colgap
    local labw = u(210)
    local fh, ch_, sh = u(Kit.BH.field), u(Kit.BH.chip), L.lh(F(Kit.TS.h2), 30)
    local nh = L.lh(F(Kit.TS.small), 24)         -- note lines: never shorter than their font
    local row_gap = u(Kit.SP.md)
    local function label(t, x, y, h) Kit.text(t, x, y, labw - gap, h, F(Kit.TS.label), 0, Kit.C.text) end
    local function section(t, x, y, w_) Kit.text(t, x, y, w_, sh, F(Kit.TS.h2), 0, Kit.C.head); return y + sh + u(Kit.SP.sm) end

    -- left column: PROFILE, NETWORK
    local y = L.top
    local cx, cw = lx + labw, lw - labw
    Kit.group("profile")
    y = section("PROFILE", lx, y, lw)
    label("NICKNAME", lx, y, fh)
    w.nick = Kit.input("Your name (2-20 letters, digits, space . _ -)", keep and edit.nick or S.data.nick, cx, y, math.min(cw, u(460)), fh, F(Kit.TS.body),
        { fkey = "settings:nick", help = "Your name in lobbies, the scoreboard and the kill feed" })
    y = y + fh
    w.nick_note = Kit.text("", cx, y, cw, nh, F(Kit.TS.small), 0, Kit.C.dim)
    y = y + nh + row_gap
    label("REGION", lx, y, ch_)
    w.region = Kit.chip_group(S.REGIONS, cx, y, cw, ch_, function(v) set_edit("region", v) end,
        { fs = F(Kit.TS.label), gap = gap, help = "Shown in the server browser for games you host (AUTO = not set)" })
    y = y + ch_ + row_gap + u(Kit.SP.lg)
    Kit.group("network")
    y = section("NETWORK", lx, y, lw)
    label("SERVER LISTS", lx, y, fh)
    local tw = u(150)
    w.master = Kit.input("https://master.example:7778, ... (empty = default)", keep and edit.master_url or S.data.master_url,
        cx, y, cw - tw - gap, fh,
        F(Kit.TS.label), { fkey = "settings:master",
          help = "Server lists (master servers) to ask, in order, comma separated. Empty = the hsmp.cfg default" })
    w.test = Kit.button("TEST", function() if b and Kit.alive(b) then run_test(); S.render() end end,
        cx + cw - tw, y, tw, fh, { fs = F(Kit.TS.label), help = "Ask every listed server list now and report which answer" })
    w.test.reason = "Fix the server list first"
    y = y + fh
    w.master_note = Kit.spinner(cx, y, cw, nh, F(Kit.TS.small), 0, Kit.C.dim)
    y = y + nh + row_gap
    label("HOST PORT", lx, y, fh)
    w.port = Kit.input("7777", keep and edit.port_text or tostring(port_of(S.data.server) or 7777), cx, y, u(170), fh, F(Kit.TS.body),
        { fkey = "settings:port", help = "UDP port the game you host listens on (1024-65535). Joiners need it open" })
    y = y + fh
    w.port_note = Kit.text("", cx, y, cw, nh, F(Kit.TS.small), 0, Kit.C.dim)
    y = y + nh + row_gap
    label("SEND RATE", lx, y, ch_)
    local hz = {}
    for _, v in ipairs(S.HZ) do hz[#hz + 1] = { v, v .. " HZ" } end
    w.hz = Kit.chip_group(hz, cx, y, cw, ch_, function(v) set_edit("send_hz", v) end,
        { fs = F(Kit.TS.label), gap = gap, help = "Movement updates you send per second. Higher is smoother but uses more upload" })

    -- right column: HUD (in a match)
    Kit.group("hud")
    local ry = L.top
    local rlab = u(190)
    local rcx, rcw = rx + rlab, rw - rlab
    ry = section("HUD (IN A MATCH)", rx, ry, rw)
    local function rlabel(t) Kit.text(t, rx, ry, rlab - gap, ch_, F(Kit.TS.label), 0, Kit.C.text) end
    rlabel("HUD")
    w.hud = onoff(L, rcx, ry, rcw, ch_, "hud", "hud", "Show the multiplayer HUD (scores, timer, kill feed, net)")
    ry = ry + ch_ + row_gap
    rlabel("KILL FEED")
    w.feed = onoff(L, rcx, ry, rcw, ch_, "hud_killfeed", "hud", "Who slew whom, top right")
    ry = ry + ch_ + row_gap
    rlabel("NET INDICATOR")
    w.net = Kit.chip_group(S.NET, rcx, ry, rcw, ch_, function(v) set_edit("hud_net", v) end,
        { fs = F(Kit.TS.label), gap = gap, help = "Ping and packet loss, bottom right (WHEN BAD = only when the link is poor)" })
    ry = ry + ch_ + row_gap
    rlabel("PEER AVATARS")
    w.av = onoff(L, rcx, ry, rcw, ch_, "avatars", "hud", "Show the other players' bodies (turn off only to debug)")
    ry = ry + ch_ + u(Kit.SP.xl)
    Kit.text("HUD settings apply from your next match.", rx, ry, rw, u(26), F(Kit.TS.small), 0, Kit.C.dim)
    ry = ry + u(26) + u(Kit.SP.lg)

    -- right column: hosting
    Kit.group("hosting")
    ry = section("HOSTING", rx, ry, rw)
    rlabel("ROUTER PORT")
    w.upnp = onoff(L, rcx, ry, rcw, ch_, "upnp", "hosting",
        "Open the HOST PORT on your router automatically while you host (UPnP / NAT-PMP / PCP). OFF = forward it by hand")
    ry = ry + ch_ + u(Kit.SP.lg)

    -- right column: server mods (docs/hosting/server-mods.md)
    Kit.group("server_mods")
    ry = section("SERVER MODS", rx, ry, rw)
    rlabel("ALLOW")
    w.smods = Kit.chip_group({ { "ask", "ASK ME" }, { "never", "NEVER" } }, rcx, ry, rcw, ch_,
        function(v) set_edit("server_mods", v) end, { fs = F(Kit.TS.label), gap = gap,
          help = "Servers can offer mods that run with full access to your PC. ASK ME shows a warning first; NEVER declines them (you cannot join those servers)" })
    ry = ry + ch_ + row_gap
    w.smods_forget = Kit.button("FORGET REMEMBERED SERVERS", function()
        if not b or not Kit.alive(b) then return end
        local n = ctx.server_mods_count and ctx.server_mods_count() or 0
        if ctx.forget_server_mods then ctx.forget_server_mods() end
        b.msg_t = Kit.now() + 4
        Kit.msg(string.format("Forgot %d server%s: their mods ask again", n, n == 1 and "" or "s"), Kit.C.ok)
    end, rx, ry, rw, ch_, { fs = F(Kit.TS.label), help = "Every server you accepted mods for with REMEMBER asks again" })

    -- actions + keys
    local acts = Kit.actions(L,
        { { label = "RESET DEFAULTS", key = "reset", cb = reset_defaults, help = "Every setting except your nickname back to its default" } },
        { { label = "BACK", key = "back", cb = back, help = "Leave without saving" },
          { label = "SAVE & BACK", key = "save", style = "primary", cb = save_and_back, help = "Check and save every setting" } })
    w.save, w.back, w.reset = acts.save, acts.back, acts.reset
    w.save.on_disabled = function() if b and Kit.alive(b) then Kit.flash(w.save.reason or "Fix the fields in red first", Kit.C.bad) end end
    b.on_back = back
    b.before_rebuild = function() if b and Kit.alive(b) then read_inputs() end end
    for _, inp in ipairs({ w.nick, w.master, w.port }) do inp.on_submit = function() S.render() end end
    Kit.on_tick(5, function() poll_test(); S.render() end)      -- 250 ms: typed fields + TEST
    Kit.focus_default(w.nick)
    S.render()
    Log("settings build: canvas %.0fx%.0f scale %.2f panel %dx%d widgets=%d", L.cw, L.ch, L.s, L.pw, L.ph, b.n)
end

function S.forget() b = nil end

S._edit = function() return edit end
S._build = function() return b end

-- c: { kit, log, path, state_dir, exit_screen, default_urls, on_saved }
function S.init(c)
    ctx = c
    Kit = c.kit
    J = c.json
    Log = c.log or Log
    S.load()
    return S.data
end

return S
