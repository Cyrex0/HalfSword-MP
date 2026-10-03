-- The peers list: since ABI 2 it is the sidecar's peer directory (typed; an entry for every connected
-- roster peer), never a decoded or pattern-matched status line.
--
--     hsmp-tools lua-test sidecar_peers
--
-- A peer id above 8 (every reconnect on a dedicated server bumps ids) must keep its
-- roster slot and nick (HSMPAvatars' stand-in).

local mode, opts = ...
local BOB = 'Bob "the {Brace}" [x]'

-- peers 1..20 (13 has a nick with quotes, braces and brackets)
local function dir_1_20()
    local e = {}
    for id = 1, 20 do e[#e + 1] = { id = id, nick = id == 13 and BOB or ("Willie (" .. id .. ")"), rtt_ms = 10 + id } end
    return e
end

if mode ~= "case" then
    -- no reader keeps the old closed-object pattern over a JSON status line
    local bad = {}
    for _, f in ipairs(T.glob(T.path("mods"), "**/*.lua")) do
        local src = T.read(f) or ""
        if src:find([['{"id":(%d+),"nick":"([^"]*)"}']], 1, true) or src:find([['{"id":(%d+),"nick":"([^"]+)"}']], 1, true)
            or src:find(".sidecar_peers(", 1, true) then
            bad[#bad + 1] = f
        end
    end
    T.check(#bad == 0, "no mod parses a peers list out of a status line", table.concat(bad, " "))
    T.isolated(T.script, "case", { kind = "avatars" })
    return
end

if opts.kind == "avatars" then
    local M = require("umg_mock")
    local sd = T.tmpdir("hsmp_scp_sd_")
    M.install({ state_dir = sd, env = { LOCALAPPDATA = T.tmpdir("hsmp_scp_la_"), HSMP_INST = "7" }, strict = true })
    package.path = T.path("mods/shared") .. "/?.lua;" .. package.path
    M.Methods.GetFullName = function(self) return "Obj " .. tostring(rawget(self, "__name")) end
    M.Methods.GetWorld = function() return M.world end
    _G.RegisterHook = function() return 1, 2 end
    _G.HSMP_AVATARS_TEST = {}
    dofile(T.path("mods/HSMPAvatars/Scripts/main.lua"))
    local api = HSMP_AVATARS_TEST.api
    local N = _G.HSMPNative
    N.sc_put("link", { status = 1, state = 1, my_peer_id = 3 })
    N._st.hb_age = 0.05
    N.sc_peer_dir(dir_1_20())
    api.read_roster()
    local r = api.roster()
    local n, ok = 0, true
    for id = 1, 20 do
        if id ~= 3 then ok = ok and r[id] ~= nil end
    end
    for _ in pairs(r) do n = n + 1 end
    T.check(n == 19 and ok and r[3] == nil, "HSMPAvatars: every peer but me (19 of ids 1..20) is in the roster", n)
    T.check(r[20] == "Willie (20)" and r[9] == "Willie (9)" and r[13] == BOB, "peers above 8 keep their nick (stand-in)")
    N.sc_peer_dir({ { id = 17, nick = "Solo", rtt_ms = 40 } })
    api.read_roster()
    r = api.roster()
    T.check(r[17] == "Solo" and r[4] == nil, "a newer peer directory replaces the old roster")
end
