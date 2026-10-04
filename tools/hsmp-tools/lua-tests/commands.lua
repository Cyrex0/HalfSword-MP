-- HSMPMenu commands.lua: one result per command, and that result is the server's
-- answer whenever one can arrive (gate DoD-10).
--
--   hsmp-tools lua-test commands
--
-- commands.lua runs against a fake clock, a fake HSMP_IPC whose cmd_result event
-- queue the test fills, and a ctx that records every sent record and event.
--
-- Covered: the session snapshot showing the outcome before the server's cmd_result
-- record (a lost result packet on a far link) still resolves as source="server";
-- an answer that never comes resolves as inferred after the grace; an inference that
-- goes away during the grace is dropped; without the record transport the inference
-- is immediate; the per-kind timeout and resends are unchanged.

local CMDS = T.path("mods/HSMPMenu/Scripts/commands.lua")

local function new_env(with_ipc)
    local w = { clock = 100.0, sent = {}, evs = {}, queue = {} }
    if with_ipc then
        _G.HSMP_IPC = { events = function(kind)
            if kind ~= "cmd_result" then return {} end
            local q = w.queue
            w.queue = {}
            return q
        end }
    else
        _G.HSMP_IPC = nil
    end
    local C = dofile(CMDS)
    C.init({
        now = function() return w.clock end,
        send_cmd = function(r) w.sent[#w.sent + 1] = r end,
        ev = function(n, f) w.evs[#w.evs + 1] = { n = n, f = f } end,
        log = function() end,
    })
    w.C = C
    -- the lobby poll: every 500 ms
    function w:run(s, match_st)
        local steps = math.floor(s / 0.5 + 0.5)
        for _ = 1, steps do
            self.clock = self.clock + 0.5
            C.tick(type(match_st) == "function" and match_st() or match_st, nil)
        end
    end
    function w:answer(id, ok) self.queue[#self.queue + 1] = { data = { cmd_id = id, ok = ok } } end
    function w:results(id)
        return T.filter(self.evs, function(e) return e.n == "cmd_result" and e.f.cmd_id == id end)
    end
    return w
end

local LOBBY5 = { state = "lobby", best_of = 5, ready = {} }

T.log("== game-mode commands are typed records")
do
    _G.HSMP_IPC = { S = { ENUMS = { cmd_op = { SET_CONFIG = 5, SET_TEAM = 11, SET_OPTION = 14 },
        cfg = { MODE = 2, ROUND_TIME = 8, TEAM_RULE = 16, TEAMS = 32 } } } }
    local C = dofile(CMDS)
    local r = C.record({ id = 7, kind = "game_mode", args = { mode = 6 } })
    T.check(r.op == 5 and r.patch.mask == 2 and r.patch.mode == 6, "mode -> SET_CONFIG MODE", T.repr(r))
    r = C.record({ id = 8, kind = "teams", args = { rule = 2, n = 3 } })
    T.check(r.patch.mask == 48 and r.patch.team_rule == 2 and r.patch.teams == 3, "teams -> TEAM_RULE | TEAMS", T.repr(r))
    r = C.record({ id = 9, kind = "set_team", args = { team = 2 } })
    T.check(r.op == 11 and r.role == 2 and r.peer_id == 0, "set_team: my own pick", T.repr(r))
    r = C.record({ id = 10, kind = "set_option", args = { opt = 1, value = 90 } })
    T.check(r.op == 14 and r.choice == 1 and r.ballot == 90, "set_option", T.repr(r))
    r = C.record({ id = 11, kind = "round_time", args = { s = 300 } })
    T.check(r.patch.mask == 8 and r.patch.round_time_limit_s == 300, "round_time", T.repr(r))
    _G.HSMP_IPC = nil
end

T.log("== the snapshot arrives before the server's answer (lost result packet)")
do
    local w = new_env(true)
    local id = w.C.send("best_of", { n = 5 })
    w:run(0.5, LOBBY5)            -- snapshot already shows best_of 5
    w:run(0.5, LOBBY5)
    w:answer(id, true)            -- the retransmitted cmd_result, ~1 s late
    w:run(0.5, LOBBY5)
    local st = w.C.status(id)
    local rs = w:results(id)
    T.check(st.state == "accepted" and st.source == "server", "resolved by the server's answer", T.repr(st))
    T.check(#rs == 1 and rs[1].f.source == "server", "exactly one cmd_result, source=server", T.repr(rs))
end

T.log("== the answer never arrives: inferred after the grace, before the timeout")
do
    local w = new_env(true)
    local id = w.C.send("best_of", { n = 5 })
    w:run(1.5, LOBBY5)
    T.check(w.C.status(id).state == "pending", "still pending inside the grace", T.repr(w.C.status(id)))
    w:run(1.0, LOBBY5)
    local st = w.C.status(id)
    T.check(st.state == "accepted" and st.source == "inferred", "inferred once the grace is over", T.repr(st))
    local rs = w:results(id)
    T.check(#rs == 1 and rs[1].f.ms >= w.C.INFER_GRACE_S * 1000 and rs[1].f.ms < w.C.KINDS.best_of.timeout_s * 1000,
        "one result between the grace and the timeout", T.repr(rs))
    w:answer(id, true)
    w:run(1.0, LOBBY5)
    T.check(#w:results(id) == 1, "a late answer adds no second result")
end

T.log("== an inference that goes away during the grace starts over")
do
    local w = new_env(true)
    local id = w.C.send("best_of", { n = 5 })
    w:run(1.0, LOBBY5)
    w:run(1.0, { state = "lobby", best_of = 3, ready = {} })
    w:run(1.5, LOBBY5)
    T.check(w.C.status(id).state == "pending", "the grace restarts when the snapshot changes back", T.repr(w.C.status(id)))
    w:run(1.0, LOBBY5)
    T.check(w.C.status(id).source == "inferred", "then inferred", T.repr(w.C.status(id)))
end

T.log("== a refusal read from the snapshot waits for the server's reason too")
do
    local w = new_env(true)
    local id = w.C.send("best_of", { n = 5 })
    w:run(1.0, { state = "live", best_of = 3, ready = {} })
    w.queue[#w.queue + 1] = { data = { cmd_id = id, ok = false, reason_text = "match config is frozen" } }
    w:run(0.5, { state = "live", best_of = 3, ready = {} })
    local st = w.C.status(id)
    T.check(st.state == "refused" and st.source == "server" and st.reason == "match config is frozen",
        "the server's refusal and reason win", T.repr(st))
end

T.log("== without the record transport the inference is immediate")
do
    local w = new_env(false)
    local id = w.C.send("best_of", { n = 5 })
    w:run(0.5, LOBBY5)
    local st = w.C.status(id)
    T.check(st.state == "accepted" and st.source == "inferred", "inferred on the first poll", T.repr(st))
end

T.log("== no answer and no inference: resent, then refused on the kind's timeout")
do
    local w = new_env(true)
    local id = w.C.send("best_of", { n = 5 })
    local lobby3 = { state = "lobby", best_of = 3, ready = {} }
    w:run(5.5, lobby3)
    T.check(w.C.status(id).state == "pending", "pending until the timeout")
    w:run(0.5, lobby3)
    local st = w.C.status(id)
    T.check(st.state == "refused" and st.source == "timeout" and #w.sent == 2, "refused after one resend",
        T.repr(st) .. " sent=" .. #w.sent)
end

_G.HSMP_IPC = nil
