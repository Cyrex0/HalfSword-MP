-- HSMPMenu / server_mods.lua -- the player's side of server mods (docs/hosting/server-mods.md):
-- the consent store, the warning screen with the mod list, the download progress, errors.
--
-- Flow: the sidecar pushes `mod_offer` + one `mod_entry` per mod + `mod_progress` OFFER
-- (names, sizes, hashes, text: no paths, no code). Then, in this order:
--   * SETTINGS > SERVER MODS = NEVER: declined at once, the screen says why;
--   * this server key already got an explicit ACCEPT with "remember" for exactly this set
--     hash: accepted at once (the progress still shows);
--   * otherwise the SERVER MODS screen asks: ACCEPT & JOIN or DECLINE (= leave, back to
--     the server browser). Nothing is ever accepted by default.
-- Any change of the set (another file, another version) is a new set hash: asked again.
--
-- Consent file: <state>/.server_mods.json, one JSON line
--   {"v":1,"servers":{"<server key hex>":{"<set hash hex>":<unix time>}}}
-- FORGET REMEMBERED SERVERS (settings) empties it; deleting the file does the same.

local M = {}

M.FILE = ".server_mods.json"
M.MAX_SERVERS = 64
M.MAX_SETS = 8

local ctx           -- attach(): { kit, log, json, state_dir, ipc(), policy() -> "ask"|"never", ev(name, f),
                    --             decline(why), joined(), server_label() }
local Kit, J
local Log = function() end
local b             -- the screen build
M.cur = nil         -- the offer: { set, raw, key, total, cached, n, mods = {...}, state, done, bytes, code, text,
                    --              decided, remember, auto }

-- --- helpers ----------------------------------------------------------------------------

function M.hex(t)
    if type(t) ~= "table" then return "" end
    local o = {}
    for i = 1, 32 do o[i] = string.format("%02x", (tonumber(t[i]) or 0) & 0xff) end
    return table.concat(o)
end

function M.size(n)
    n = tonumber(n) or 0
    if n >= 1048576 then return string.format("%.1f MB", n / 1048576) end
    if n >= 1024 then return string.format("%.0f KB", n / 1024) end
    return string.format("%d B", n)
end

local function ipc() return ctx and ctx.ipc and ctx.ipc() or nil end
local function path() return ctx.state_dir .. "/" .. M.FILE end

-- --- consent store ------------------------------------------------------------------------

function M.load_store()
    local t
    local f = io.open(path(), "rb")
    if f then
        local s = f:read("a") or ""; f:close()
        t = J and J.decode((s:match("[^\r\n]+")) or "") or nil
    end
    if type(t) ~= "table" or type(t.servers) ~= "table" then t = { v = 1, servers = {} } end
    return t
end

-- {"v":1,"servers":{key:{set:time}}}; keys are 64 hex chars (checked), values integers.
local function encode(t)
    local srv = {}
    for k, sets in pairs(t.servers) do
        if type(k) == "string" and k:match("^%x+$") and type(sets) == "table" then
            local parts = {}
            for sk, v in pairs(sets) do
                if type(sk) == "string" and sk:match("^%x+$") then parts[#parts + 1] = string.format('"%s":%d', sk, math.floor(tonumber(v) or 0)) end
            end
            table.sort(parts)
            srv[#srv + 1] = string.format('"%s":{%s}', k, table.concat(parts, ","))
        end
    end
    table.sort(srv)
    return '{"v":1,"servers":{' .. table.concat(srv, ",") .. '}}\n'
end

local function save_store(t)
    local I = ipc()
    local ok
    if I and I.write then ok = I.write(path(), encode(t))   -- tmp + rename
    else
        local f = io.open(path(), "wb")
        ok = f ~= nil
        if f then f:write(encode(t)); f:close() end
    end
    if not ok then Log("server mods: cannot write %s", path()) end
    return ok
end

function M.remembered(key, set)
    if type(key) ~= "string" or #key ~= 64 or type(set) ~= "string" or #set ~= 64 then return false end
    local s = M.load_store().servers[key]
    return type(s) == "table" and s[set] ~= nil
end

function M.remember(key, set)
    if type(key) ~= "string" or #key ~= 64 or key == string.rep("0", 64) then return false end
    local t = M.load_store()
    local s = t.servers[key]
    if type(s) ~= "table" then
        local n = 0
        for _ in pairs(t.servers) do n = n + 1 end
        if n >= M.MAX_SERVERS then return false end
        s = {}; t.servers[key] = s
    end
    s[set] = os.time()
    local sets = {}
    for k, v in pairs(s) do sets[#sets + 1] = { k, tonumber(v) or 0 } end
    table.sort(sets, function(a, c) return a[2] > c[2] end)
    for i = M.MAX_SETS + 1, #sets do s[sets[i][1]] = nil end
    return save_store(t)
end

-- Remembered servers (settings shows the count).
function M.count()
    local n = 0
    for _ in pairs(M.load_store().servers) do n = n + 1 end
    return n
end

function M.forget_all()
    local ok = save_store({ v = 1, servers = {} })
    Log("server mods: every remembered server forgotten")
    return ok
end

-- --- the offer --------------------------------------------------------------------------

local function S() local I = ipc(); return I and I.S or nil end

local function send_decision(op)
    local I, sch = ipc(), S()
    if not (I and sch and M.cur) then return false end
    return I.send("mod_decision", { set_hash = M.cur.raw, op = sch.ENUMS.mod_op[op] }) ~= nil
end

local STATE_NAMES = { "offer", "downloading", "verifying", "ready", "joined", "failed", "clear" }

-- A new session: forget the last offer.
function M.reset() M.cur = nil end

-- Drain the sidecar's records (subscribes on the first call).
function M.pump()
    local I = ipc()
    if not I then return end
    for _, e in ipairs(I.events("mod_offer")) do
        local d = e.data or {}
        local set,key = M.hex(d.set_hash),M.hex(d.server_key)
        if not M.cur or M.cur.set ~= set or M.cur.key ~= key then
            M.cur = { set = set, raw = d.set_hash, mods = {}, remember = true }
        end
        local c = M.cur
        c.key, c.n, c.total, c.cached = key, tonumber(d.n) or 0, tonumber(d.total_bytes) or 0, tonumber(d.cached_bytes) or 0
    end
    for _, e in ipairs(I.events("mod_entry")) do
        local d = e.data or {}
        if M.cur and M.hex(d.set_hash) == M.cur.set then
            M.cur.mods[(tonumber(d.index) or 0) + 1] = {
                name = tostring(d.name or "?"), version = tostring(d.version or ""), author = tostring(d.author or ""),
                description = tostring(d.description or ""), bytes = tonumber(d.bytes) or 0, cached = d.cached == true,
            }
        end
    end
    for _, e in ipairs(I.events("mod_progress")) do
        local d = e.data or {}
        if M.cur and M.hex(d.set_hash) == M.cur.set then
            local c = M.cur
            c.state = STATE_NAMES[tonumber(d.state) or 0] or c.state
            c.done, c.bytes = tonumber(d.done_bytes) or 0, tonumber(d.total_bytes) or c.total
            if c.state == "failed" then
                c.code = tonumber(d.code) or 0
                c.text = tostring(d.text or "")
            end
        end
    end
end

local function complete(c)
    if not c or not c.n then return false end
    for i = 1, c.n do if not c.mods[i] then return false end end
    return true
end

-- Every 500 ms while a session runs. `screen` = the open screen; `open(kind)` opens one.
-- Returns true while the server's mods hold the session (the lobby waits).
function M.tick(screen, open)
    M.pump()
    local c = M.cur
    if not c then return false end
    if c.state == "offer" and not c.decided and complete(c) then
        local policy = ctx.policy and ctx.policy() or "ask"
        if policy == "never" then
            c.decided, c.auto = "decline", "never"
            send_decision("DECLINE")
            c.state, c.text = "failed", "You set SERVER MODS to NEVER in the settings, so this server's mods were declined."
            Log("server mods: declined automatically (setting NEVER)")
            if ctx.ev then ctx.ev("x_server_mods_offer", { mods = c.n, bytes = c.total, decision = "never" }) end
        elseif M.remembered(c.key, c.set) then
            c.decided, c.auto = "accept", "remembered"
            send_decision("ACCEPT")
            Log("server mods: accepted (remembered for this server and this exact mod set)")
            if ctx.ev then ctx.ev("x_server_mods_offer", { mods = c.n, bytes = c.total, decision = "remembered" }) end
        end
        if screen ~= "mods" then open("mods") end
    elseif c.state == "joined" then
        if screen == "mods" then open("lobby") end
        return false
    elseif (c.state and c.state ~= "clear") and screen ~= "mods" and screen ~= nil then
        open("mods")
    end
    if screen == "mods" and M.render then M.render() end
    return c.state ~= nil and c.state ~= "joined" and c.state ~= "clear"
end

function M.accept()
    local c = M.cur
    if not c or c.decided or c.state ~= "offer" then return end
    c.decided = "accept"
    if c.remember then M.remember(c.key, c.set) end
    send_decision("ACCEPT")
    Log("server mods: ACCEPTED by the player (%d mods, %s, remember=%s)", c.n, M.size(c.total), tostring(c.remember))
    if ctx.ev then ctx.ev("x_server_mods_offer", { mods = c.n, bytes = c.total, decision = "accept" }) end
    M.render()
end

function M.decline()
    local c = M.cur
    if c and not c.decided and c.state == "offer" then
        c.decided = "decline"
        send_decision("DECLINE")
        Log("server mods: DECLINED by the player")
        if ctx.ev then ctx.ev("x_server_mods_offer", { mods = c.n, bytes = c.total, decision = "decline" }) end
    end
    M.cur = nil
    ctx.decline((c and c.state == "failed") and "server mods failed" or "server mods declined")
end

-- --- the SERVER MODS screen ------------------------------------------------------------

local ROWS = 8

function M.build()
    Kit = ctx.kit
    local L
    L, b = Kit.frame("mods", "SERVER MODS", { w = 1500, h = 820 })
    local w = b.w
    local u, F = L.u, L.F
    local y = L.top
    local lh = u(30)
    w.warn = Kit.text("", L.x0, y, L.iw, u(64), F(Kit.TS.h2), 0, Kit.C.warn, { wrap = 2 })
    y = y + u(64) + u(Kit.SP.sm)
    w.server = Kit.text("", L.x0, y, L.iw, lh, F(Kit.TS.small), 0, Kit.C.dim)
    y = y + lh + u(Kit.SP.sm)
    Kit.rect(L.x0, y, L.iw, math.max(1, u(2)), Kit.C.rule, 11)
    y = y + u(Kit.SP.sm)
    w.rows = {}
    local rh = u(30)
    for i = 1, ROWS do
        local r = {}
        r.name = Kit.text("", L.x0, y, math.floor(L.iw * 0.62), rh, F(Kit.TS.label), 0, Kit.C.text)
        r.size = Kit.text("", L.x0 + math.floor(L.iw * 0.62), y, L.iw - math.floor(L.iw * 0.62), rh, F(Kit.TS.label), 2, Kit.C.dim)
        r.desc = Kit.text("", L.x0 + u(Kit.SP.lg), y + rh, L.iw - u(Kit.SP.lg), u(24), F(Kit.TS.small), 0, Kit.C.dim)
        w.rows[i] = r
        y = y + rh + u(24) + u(Kit.SP.xs)
    end
    w.more = Kit.text("", L.x0, y, L.iw, u(24), F(Kit.TS.small), 0, Kit.C.dim)
    y = y + u(24) + u(Kit.SP.sm)
    w.total = Kit.text("", L.x0, y, math.floor(L.iw * 0.5), lh, F(Kit.TS.label), 0, Kit.C.text)
    Kit.group("remember")
    local cw = math.floor(L.iw * 0.45)
    w.remember = Kit.chip_group({ { true, "REMEMBER FOR THIS SERVER" }, { false, "ASK EVERY TIME" } },
        L.x0 + L.iw - cw, y, cw, lh, function(v) if M.cur then M.cur.remember = v; M.render() end end,
        { fs = F(Kit.TS.small), gap = L.gap, help = "Remember this answer for this server and exactly these mods (any change asks again)" })
    y = y + lh + u(Kit.SP.md)
    w.bar = Kit.bar(L.x0, y, L.iw, u(28), F(Kit.TS.small))
    local acts = Kit.actions(L, {},
        { { label = "DECLINE", key = "decline", style = "danger", cb = function() M.decline() end,
            help = "Do not install them: leave this server and go back to the server browser" },
          { label = "ACCEPT & JOIN", key = "accept", style = "primary", cb = function() M.accept() end,
            help = "Download these mods, run them and join. Only for servers you trust" } })
    w.accept, w.decline = acts.accept, acts.decline
    b.on_back = function() M.decline() end
    Kit.on_tick(5, function() M.render() end)
    Kit.focus_default(w.decline)
    M.render()
end

function M.render()
    if not b or not Kit or not Kit.alive(b) then return end
    local w, c = b.w, M.cur
    if not c then
        Kit.set_text(w.warn, "Waiting for the server's mod list...")
        Kit.status("CONNECTING", Kit.C.dim, true)
        Kit.show(w.accept, false)
        return
    end
    Kit.set_text(w.warn, string.format("This server wants to install %d mod%s that run with FULL ACCESS to your PC "
        .. "(files, network, everything a game mod can do). Only accept for servers you trust.", c.n or 0, (c.n == 1) and "" or "s"))
    local label = ctx.server_label and ctx.server_label() or ""
    Kit.set_text(w.server, string.format("SERVER %s   KEY %s   MOD SET %s", label ~= "" and label or "?",
        (c.key or ""):sub(1, 16), (c.set or ""):sub(1, 12)))
    for i = 1, ROWS do
        local r, m = w.rows[i], c.mods[i]
        if m then
            Kit.set_text(r.name, m.name .. (m.version ~= "" and ("  " .. m.version) or "") .. (m.author ~= "" and ("  by " .. m.author) or ""))
            Kit.set_text(r.size, M.size(m.bytes) .. (m.cached and "  (DOWNLOADED BEFORE)" or ""))
            Kit.set_text(r.desc, m.description)
        else
            Kit.set_text(r.name, ""); Kit.set_text(r.size, ""); Kit.set_text(r.desc, "")
        end
    end
    Kit.set_text(w.more, (c.n or 0) > ROWS and string.format("... and %d more", c.n - ROWS) or "")
    Kit.set_text(w.total, string.format("TOTAL %s%s", M.size(c.total), (c.cached or 0) > 0 and ("  (" .. M.size(c.cached) .. " downloaded before)") or ""))
    Kit.chip_group_set(w.remember, c.remember, c.decided ~= nil)
    local st = c.state or "offer"
    local undecided = st == "offer" and not c.decided
    Kit.show(w.accept, undecided)
    Kit.set_label(w.decline, undecided and "DECLINE" or (st == "failed" and "BACK TO BROWSER" or "CANCEL & LEAVE"))
    local frac = (c.bytes or 0) > 0 and (c.done or 0) / c.bytes or 0
    if st == "failed" then
        Kit.bar_set(w.bar, 1, Kit.C.bad, "FAILED")
        Kit.status("FAILED", Kit.C.bad, false)
        Kit.msg(c.text ~= "" and c.text or "The server's mods could not be installed.", Kit.C.bad)
    elseif undecided then
        Kit.bar_set(w.bar, 0, Kit.C.dim, "")
        Kit.status("WAITING FOR YOU", Kit.C.warn, false)
        Kit.msg("Accept only if you trust this server. Declining leaves the server.", Kit.C.warn)
    elseif st == "ready" or st == "joined" then
        Kit.bar_set(w.bar, 1, Kit.C.good, "LOADING THE MODS...")
        Kit.status("LOADING", Kit.C.ok, true)
        Kit.msg(c.auto == "remembered" and "Accepted earlier for this server and these mods." or "Verified. Starting the mods...", Kit.C.ok)
    else
        Kit.bar_set(w.bar, frac, Kit.C.ok, string.format("DOWNLOADING %s / %s", M.size(c.done), M.size(c.bytes)))
        Kit.status("DOWNLOADING", Kit.C.ok, true)
        Kit.msg(c.auto == "remembered" and "Accepted earlier for this server and these mods. Every file is checked against the server's hash."
            or "Every file is checked against the server's hash before it is saved.", Kit.C.dim)
    end
end

function M.forget() b = nil end

-- c: see `ctx` above.
function M.attach(c)
    ctx = c
    Kit, J = c.kit, c.json
    Log = c.log or Log
    pcall(M.pump)   -- subscribe before the sidecar can push anything
end

return M
