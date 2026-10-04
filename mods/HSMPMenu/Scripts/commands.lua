-- HSMPMenu / commands.lua — every host/player action is a COMMAND with a
-- visible result: pending -> accepted | refused (+ reason).
--
--   local id = Cmd.send(kind, args)      -- kind: pick_arena{arena} | best_of{n} | kit_rules{mode,budget}
--                                        --       start | abort | ready{value=bool} | kick{peer} | promote{peer}
--                                        --       game_mode{mode} | teams{rule, n} | round_time{s}
--                                        --       set_team{team, peer (nil = me)} | set_option{opt, value}
--   local st = Cmd.status(id)            -- { id, kind, args, state, reason, source,
--                                        --   tries, age_s }   state: "pending" |
--                                        --   "accepted" | "refused" | "superseded"
--
-- The menu NEVER acts on its own guess of the outcome; it only displays it.
--
-- Two backends, chosen per command at send time:
--
--  * net: an hsmp_net module exposing
--      net.cmd_send(kind, args, cmd_id)   and   net.cmd_result(cmd_id) -> {ok, reason_code, reason_text}|nil
--
--  * record (normal): a typed `command` record (schema session.rs: cmd_id, op,
--    flag, peer_id, text, patch) sent with IPC.send through ctx.send_cmd. The
--    sidecar resends it until the server's `cmd_result` record arrives
--    (IPC.events("cmd_result"): cmd_id, ok, reason_code, reason_text), which maps
--    1:1 onto this command's state. Meanwhile the result is also INFERRED from
--    the server's own state (the session snapshot) and its SERVER chat replies,
--    with local resends (same cmd_id: the sidecar dedups) and a timeout per kind.
--
-- Events (shared/hsmp_log vocabulary, docs/development/testing.md):
--   cmd_sent{cmd, cmd_id, arg, backend, via}       once per command (first send)
--   cmd_result{cmd, cmd_id, ok, reason, state, source, ms, tries}
-- Exactly one cmd_result per cmd_id (superseded = ok:false, reason "replaced by #n").

local C = {}

local ctx   -- see C.init
local Log = function() end

C.KINDS = {
    --              resend every   sends at most   then refused as timeout
    pick_arena = { resend_s = 3, max_tries = 6, timeout_s = 18 },
    best_of    = { resend_s = 2, max_tries = 2, timeout_s = 6 },
    start      = { resend_s = 3, max_tries = 2, timeout_s = 9 },
    abort      = { resend_s = 2, max_tries = 2, timeout_s = 6 },
    ready      = { resend_s = 2, max_tries = 2, timeout_s = 6 },
    kit_rules  = { resend_s = 2, max_tries = 2, timeout_s = 6 },
    kick       = { resend_s = 2, max_tries = 2, timeout_s = 6 },
    promote    = { resend_s = 2, max_tries = 2, timeout_s = 6 },
    ban        = { resend_s = 2, max_tries = 2, timeout_s = 6 },
    unban      = { resend_s = 2, max_tries = 2, timeout_s = 6 },
    reset_match = { resend_s = 2, max_tries = 2, timeout_s = 6 },
    game_mode  = { resend_s = 2, max_tries = 2, timeout_s = 6 },
    teams      = { resend_s = 2, max_tries = 2, timeout_s = 6 },
    round_time = { resend_s = 2, max_tries = 2, timeout_s = 6 },
    set_team   = { resend_s = 2, max_tries = 2, timeout_s = 6 },
    set_option = { resend_s = 2, max_tries = 2, timeout_s = 6 },
}

-- A pick is a ONE-SHOT request, never a desired state the menu enforces: it
-- is resent only while it is unanswered, and dropped as soon as the server
-- answers, refuses, or its arena changes for any other reason.
--
-- Commands where only the newest one matters: a newer command of the same
-- kind supersedes a pending older one (record backend stops resending it, so
-- an old pick can never overwrite a newer one on the server).
local SUPERSEDE = { pick_arena = true, best_of = true, ready = true, kit_rules = true, game_mode = true, teams = true,
                    round_time = true, set_team = true }

-- SERVER chat replies (server.rs handle_match_verb / admin verbs) -> refusal.
local HOST_ONLY = { pick_arena = true, start = true, abort = true, best_of = true, kit_rules = true, kick = true, promote = true,
                    ban = true, unban = true, reset_match = true, game_mode = true, teams = true, round_time = true,
                    set_option = true }
local REFUSAL_PATTERNS = {
    start      = { "^start blocked" },
    pick_arena = { "^map can only be changed", "^unknown arena", "^set_arena" },
    best_of    = { "^set_best_of" },
    abort      = {},
    ready      = {},
    kit_rules  = { "^kit mode must" },
    kick       = { "^kick: no such peer", "^no such peer" },
    promote    = { "^promote: no such peer", "^no such peer", "^only promotion" },
}

C.by_id = {}
C.order = {}            -- ids, oldest first
C.last_by_kind = {}     -- kind -> newest id
C.n = 0


local function now_s() return ctx.now() end

local function args_str(kind, a)
    if kind == "pick_arena" then return tostring(a.arena)
    elseif kind == "best_of" then return tostring(a.n)
    elseif kind == "ready" then return tostring(a.value)
    elseif kind == "kick" or kind == "promote" or kind == "ban" then return tostring(a.peer)
    elseif kind == "unban" then return tostring(a.entry)
    elseif kind == "kit_rules" then return tostring(a.mode) .. "/" .. tostring(a.budget)
    elseif kind == "game_mode" then return tostring(a.mode)
    elseif kind == "teams" then return tostring(a.rule) .. "/" .. tostring(a.n)
    elseif kind == "round_time" then return tostring(a.s)
    elseif kind == "set_team" then return tostring(a.team) .. (a.peer and ("@" .. tostring(a.peer)) or "")
    elseif kind == "set_option" then return tostring(a.opt) .. "=" .. tostring(a.value) end
    return ""
end

-- The typed `command` record of a command (schema session.rs Command).
local function schema()
    local ipc = rawget(_G, "HSMP_IPC")
    return ipc and ipc.S or nil
end
function C.record(cmd)
    local S = schema()
    local OP = (S and S.ENUMS.cmd_op) or {}
    local CFG = (S and S.ENUMS.cfg) or {}
    local a, k = cmd.args, cmd.kind
    local r = { cmd_id = cmd.id }
    if k == "pick_arena" then r.op, r.text = OP.PICK_ARENA, tostring(a.arena or "")
    elseif k == "best_of" then r.op, r.patch = OP.SET_CONFIG, { mask = CFG.BEST_OF, best_of = tonumber(a.n) or 3 }
    elseif k == "start" then r.op, r.flag = OP.START, a.force and true or false
    elseif k == "abort" then r.op = OP.ABORT
    elseif k == "reset_match" then r.op = OP.RESET_MATCH
    elseif k == "ready" then r.op, r.flag = OP.READY, a.value and true or false
    elseif k == "kick" then r.op, r.peer_id, r.text = OP.KICK, tonumber(a.peer) or 0, tostring(a.reason or "")
    elseif k == "ban" then r.op, r.peer_id, r.text = OP.BAN, tonumber(a.peer) or 0, tostring(a.reason or "")
    elseif k == "unban" then r.op, r.text = OP.UNBAN, tostring(a.entry or "")
    elseif k == "promote" then
        r.op, r.peer_id, r.role = OP.PROMOTE, tonumber(a.peer) or 0, (S and S.ENUMS.admin_role.ADMIN) or 2
    elseif k == "kit_rules" then
        r.op, r.patch = OP.SET_CONFIG, { mask = CFG.KIT_RULES, kit_mode = tonumber(a.mode) or 0, kit_budget = tonumber(a.budget) or 0 }
    elseif k == "game_mode" then
        r.op, r.patch = OP.SET_CONFIG, { mask = CFG.MODE, mode = tonumber(a.mode) or 0 }
    elseif k == "teams" then
        r.op, r.patch = OP.SET_CONFIG, { mask = (CFG.TEAM_RULE or 0) | (a.n and (CFG.TEAMS or 0) or 0),
                                         team_rule = tonumber(a.rule) or 0, teams = tonumber(a.n) or 0 }
    elseif k == "round_time" then
        r.op, r.patch = OP.SET_CONFIG, { mask = CFG.ROUND_TIME, round_time_limit_s = tonumber(a.s) or 0 }
    elseif k == "set_team" then
        r.op, r.role, r.peer_id = OP.SET_TEAM, tonumber(a.team) or 0, tonumber(a.peer) or 0
    elseif k == "set_option" then
        r.op, r.choice, r.ballot = OP.SET_OPTION, tonumber(a.opt) or 0, tonumber(a.value) or 0
    end
    return r
end

local function net_backend()
    local n = ctx.net
    if type(n) == "table" and type(n.cmd_send) == "function" and type(n.cmd_result) == "function" then return n end
    return nil
end

local function transmit(cmd)
    cmd.tries = cmd.tries + 1
    cmd.last_sent = now_s()
    if cmd.backend == "net" then
        pcall(ctx.net.cmd_send, cmd.kind, cmd.args, cmd.id)
    else
        ctx.send_cmd(C.record(cmd))
    end
    -- One cmd_sent per command (the first transmission); resends are logged only.
    if cmd.tries == 1 then
        local arg = args_str(cmd.kind, cmd.args)
        ctx.ev("cmd_sent", { cmd = cmd.kind, cmd_id = cmd.id, arg = (arg ~= "" and arg or nil),
                             backend = cmd.backend, via = cmd.via })
    end
    Log("cmd #%d %s(%s) sent (try %d, %s)", cmd.id, cmd.kind, args_str(cmd.kind, cmd.args), cmd.tries, cmd.backend)
end

local function resolve(cmd, state, reason, source)
    if cmd.state ~= "pending" then return end
    cmd.state, cmd.reason, cmd.source = state, reason, source
    cmd.resolved_t = now_s()
    local ms = math.floor((cmd.resolved_t - cmd.t0) * 1000 + 0.5)
    ctx.ev("cmd_result", { cmd = cmd.kind, cmd_id = cmd.id, ok = (state == "accepted"),
                           reason = reason, state = state, source = source, ms = ms, tries = cmd.tries })
    Log("cmd #%d %s(%s) %s%s [%s, %d ms]", cmd.id, cmd.kind, args_str(cmd.kind, cmd.args), state:upper(),
        reason and (": " .. reason) or "", source, ms)
    if ctx.on_result then pcall(ctx.on_result, cmd) end
end

-- ---------------------------------------------------------------------------------------
-- public API

function C.send(kind, args, via)
    local k = C.KINDS[kind]
    if not k then error("unknown command kind " .. tostring(kind)) end
    args = args or {}
    C.n = C.n + 1
    local id = C.base + C.n
    local cmd = { id = id, kind = kind, args = args, state = "pending", tries = 0,
                  t0 = now_s(), backend = net_backend() and "net" or "record", via = via,
                  chat_pos = ctx.chat_size and ctx.chat_size() or 0,
                  srv0 = ctx.server_arena and ctx.server_arena() or nil }   -- server arena when sent
    if SUPERSEDE[kind] then
        local old = C.by_id[C.last_by_kind[kind]]
        if old and old.state == "pending" then resolve(old, "superseded", "replaced by #" .. id, "local") end
    end
    C.by_id[id] = cmd
    C.order[#C.order + 1] = id
    C.last_by_kind[kind] = id
    transmit(cmd)
    return id
end

-- Fire-and-forget: one transmission, not tracked, no cmd_sent / cmd_result events (the
-- teardown's best-effort unready: the session is closing, so no answer is awaited).
function C.fire(kind, args)
    if not C.KINDS[kind] then return nil end
    C.n = C.n + 1
    local cmd = { id = C.base + C.n, kind = kind, args = args or {}, state = "fired", tries = 1,
                  t0 = now_s(), backend = net_backend() and "net" or "record" }
    if cmd.backend == "net" then
        pcall(ctx.net.cmd_send, cmd.kind, cmd.args, cmd.id)
    else
        ctx.send_cmd(C.record(cmd))
    end
    Log("cmd #%d %s(%s) fired (untracked)", cmd.id, cmd.kind, args_str(cmd.kind, cmd.args))
    return cmd.id
end

-- Snapshot of a command (a copy; nil for an unknown id).
function C.status(id)
    local c = C.by_id[id]
    if not c then return nil end
    return { id = c.id, kind = c.kind, args = c.args, state = c.state, reason = c.reason,
             source = c.source, tries = c.tries, backend = c.backend,
             age_s = now_s() - c.t0, done_age_s = c.resolved_t and (now_s() - c.resolved_t) or nil }
end

function C.last(kind) return C.last_by_kind[kind] end

-- The newest command of any of `kinds` (for the lobby's note line).
function C.latest(kinds)
    for i = #C.order, 1, -1 do
        local c = C.by_id[C.order[i]]
        if c and (not kinds or kinds[c.kind]) and c.state ~= "superseded" then return c.id end
    end
    return nil
end

function C.pending(kind)
    local c = C.by_id[C.last_by_kind[kind]]
    return c ~= nil and c.state == "pending"
end

-- Forget every command (new session: HOST / JOIN / CANCEL).
function C.reset()
    for _, id in ipairs(C.order) do
        local c = C.by_id[id]
        if c and c.state == "pending" then resolve(c, "refused", "session closed", "local") end
    end
    C.by_id, C.order, C.last_by_kind = {}, {}, {}
end

-- ---------------------------------------------------------------------------------------
-- result sources

-- The server's `cmd_result` records (S2G events, shared/hsmp_ipc.lua).
local function scan_results()
    local ipc = rawget(_G, "HSMP_IPC")
    if not (ipc and ipc.events) then return end
    for _, e in ipairs(ipc.events("cmd_result") or {}) do C.consume_result(e.data) end
end

-- One cmd_result record {cmd_id, ok, reason_code, reason_text, op, config_rev}.
function C.consume_result(r)
    if type(r) ~= "table" then return end
    local cmd = C.by_id[tonumber(r.cmd_id) or -1]
    if not cmd or cmd.state ~= "pending" then return end
    local ok = r.ok == true
    local reason = r.reason_text
    if reason == nil or reason == "" then
        local S = schema()
        reason = S and S.ENUM_NAMES.cmd_reason[r.reason_code] or nil
    end
    resolve(cmd, ok and "accepted" or "refused", (not ok) and (reason or "refused by the server") or nil, "server")
end

local function in_list(list, v)
    for _, x in ipairs(list or {}) do if x == v then return true end end
    return false
end

-- Fallback: infer from the server's state + its chat replies.
local function infer(cmd, match_st, srv_arena)
    local a = cmd.args
    if cmd.kind == "pick_arena" then
        if srv_arena and srv_arena == a.arena then return "accepted" end
        -- The server's arena changed to something else after we sent (another
        -- admin, RCON MAP, a newer pick from another client): our pick is old
        -- news. Drop it and never re-send it over the newer server value.
        if cmd.srv0 and srv_arena and srv_arena ~= cmd.srv0 then
            return "superseded", "server arena changed to " .. srv_arena
        end
        if match_st and match_st.state ~= "lobby" then return "refused", "map can only be changed in the lobby" end
    elseif cmd.kind == "best_of" then
        if match_st and match_st.best_of == a.n then return "accepted" end
        if match_st and match_st.state ~= "lobby" then return "refused", "match config is frozen until the lobby" end
    elseif cmd.kind == "start" then
        if match_st and match_st.state ~= "lobby" then return "accepted" end
    elseif cmd.kind == "abort" then
        if match_st and match_st.state == "lobby" then return "accepted" end
    elseif cmd.kind == "ready" then
        local me = ctx.my_peer_id and ctx.my_peer_id() or 0
        if me ~= 0 and match_st then
            if in_list(match_st.ready, me) == (a.value and true or false) then return "accepted" end
        end
    elseif cmd.kind == "kick" then
        -- the peer left the roster after the kick was sent
        local ids = ctx.peer_ids and ctx.peer_ids()
        if ids and not in_list(ids, a.peer) then return "accepted" end
    elseif cmd.kind == "promote" then
        -- the server moved the admin role to the peer (we lose it)
        if ctx.admin_of and ctx.admin_of(a.peer) == true then return "accepted" end
    elseif cmd.kind == "game_mode" then
        if match_st and match_st.mode == a.mode then return "accepted" end
        if match_st and match_st.state ~= "lobby" then return "refused", "match config is frozen until the lobby" end
    elseif cmd.kind == "kit_rules" then
        local r = ctx.kit_rules and ctx.kit_rules()
        if r and r.mode == a.mode and (a.mode ~= 2 or r.budget == a.budget) then return "accepted" end
        if match_st and match_st.state ~= "lobby" then return "refused", "match config is frozen until the lobby" end
    end
    -- a SERVER chat reply written after this command was sent
    if ctx.chat_lines then
        for _, text in ipairs(ctx.chat_lines(cmd.chat_pos) or {}) do
            if HOST_ONLY[cmd.kind] and (text:find("^only the host") or text:find("^admin verb refused")) then return "refused", text end
            for _, p in ipairs(REFUSAL_PATTERNS[cmd.kind] or {}) do
                if text:find(p) then return "refused", text end
            end
        end
    end
    return nil
end

-- How long an inferred outcome waits for the server's cmd_result record before it is
-- shown: two retransmits at ~300 ms RTT (initial RTO 300 ms, doubled on a second loss).
C.INFER_GRACE_S = 2.0

-- A server answer can still arrive only over the record transport.
local function answers_possible()
    local ipc = rawget(_G, "HSMP_IPC")
    return ipc ~= nil and ipc.events ~= nil
end

local function timeout_reason(cmd)
    if cmd.kind == "start" then return "the server did not start the match - check that everyone is ready" end
    return "no answer from the server"
end

-- Advance every pending command. Call it from a game-thread loop (the lobby
-- poll does, every 500 ms). match_st: main.lua's read_match_state(); srv_arena:
-- the server's arena (MAP_PRESETS path) or nil.
function C.tick(match_st, srv_arena)
    scan_results()
    local net = ctx.net
    for _, id in ipairs(C.order) do
        local cmd = C.by_id[id]
        if cmd and cmd.state == "pending" then
            local k = C.KINDS[cmd.kind]
            local age = now_s() - cmd.t0
            if cmd.backend == "net" then
                local r; pcall(function() r = net.cmd_result(cmd.id) end)
                if type(r) == "table" then
                    resolve(cmd, r.ok and "accepted" or "refused",
                        (not r.ok) and (r.reason_text or r.reason_code or "refused by the server") or nil, "server")
                elseif age >= k.timeout_s then
                    resolve(cmd, "refused", timeout_reason(cmd), "timeout")
                end
            else
                local st, why = infer(cmd, match_st, srv_arena)
                -- The session snapshot and the cmd_result record travel on different
                -- channels: when the result packet is lost the snapshot can arrive a
                -- retransmit (or two) earlier. Wait that long for the server's own answer.
                local hold = st and answers_possible()
                if st then cmd.inferred_t = cmd.inferred_t or now_s() else cmd.inferred_t = nil end
                if hold and now_s() - cmd.inferred_t < C.INFER_GRACE_S and age < k.timeout_s then
                    -- wait; the sidecar keeps resending until the answer arrives
                elseif st then
                    resolve(cmd, st, why, "inferred")
                elseif age >= k.timeout_s then
                    resolve(cmd, "refused", timeout_reason(cmd), "timeout")
                elseif cmd.tries < k.max_tries and now_s() - cmd.last_sent >= k.resend_s then
                    if ctx.on_resend then pcall(ctx.on_resend, cmd) end
                    transmit(cmd)
                end
            end
        end
    end
end

-- ctx: {
--   state_dir, now() -> seconds, send_cmd(record) (a typed `command`), ev(name, fields), log,
--   server_arena() -> the server's current arena or nil (snapshot at send),
--   chat_size() -> bytes, chat_lines(pos) -> { SERVER texts after pos },
--   my_peer_id() -> n, net = hsmp_net module or nil,
--   peer_ids() -> { id }, admin_of(pid) -> bool|nil, kit_rules() -> {mode, budget}|nil
--     (legacy inference for kick / promote / kit_rules),
--   on_result(cmd), on_resend(cmd)   (optional)
-- }
function C.init(c)
    ctx = c
    Log = c.log or Log
    ctx.ev = ctx.ev or function() end
    -- ids are u32 and unique across game restarts within one sidecar session
    C.base = (os.time() % 4000000) * 1000
    -- results already queued belong to an earlier run: subscribe now and drop them
    local ipc = rawget(_G, "HSMP_IPC")
    if ipc and ipc.events then ipc.events("cmd_result") end
end

return C
