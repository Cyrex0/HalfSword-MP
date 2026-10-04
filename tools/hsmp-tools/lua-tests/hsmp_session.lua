-- shared/hsmp_session.lua: the typed session / link view and the one MP-liveness predicate,
-- on the typed native mock (records `link`, `session`).
--
--     hsmp-tools lua-test hsmp_session

local NM = require("hsmp_native_mock")
local S = NM.S
local N = NM.new{}
local IPC = dofile(T.path("mods/shared/hsmp_ipc.lua"))
local clk = 0
IPC.init{ mod = "Test", native = N, clock = function() return clk end, log = function() end }
local HS = dofile(T.path("mods/shared/hsmp_session.lua"))
local ST, LS = S.ENUMS.sidecar_status, S.ENUMS.link_state

local Sx = HS.new({ ipc = IPC, clock = function() return clk end, every_s = 0 })
local function at(t) clk = t; Sx:poll() end
local function link(status, id, extra)
    local t = { status = ST[status:upper()], state = LS.UP, my_peer_id = id or 1 }
    for k, v in pairs(extra or {}) do t[k] = v end
    N.sc_put("link", t)
end

at(0)
T.check(not Sx:live() and not Sx.exists, "no link record: not live")
link("connected", 1)
at(1)
T.check(Sx:live() and Sx.peer_id == 1 and Sx.status == "connected", "connected + fresh heartbeat -> live")
N._st.hb_age = 4.9
IPC.refresh_info(true)
T.check(Sx:live(), "heartbeat 4.9 s old: still live")
N._st.hb_age = 5.2
IPC.refresh_info(true)
T.check(not Sx:live() and Sx.status == "connected", "a 'connected' link whose sidecar stopped beating is not live")
N._st.hb_age = 0.01
N._st.sidecar_state = "absent"
IPC.refresh_info(true)
T.check(not Sx:live(), "a detached sidecar is never live")
N._st.sidecar_state = "ready"
IPC.refresh_info(true)
link("reconnecting", 1)
at(2)
T.check(not Sx:live() and Sx:fresh(), "reconnecting is not live")
link("kicked", 1, { reason = "afk", state = LS.TERMINAL })
at(3)
T.check(not Sx:live() and Sx:terminal() and Sx.link.reason == "afk", "kicked: terminal, reason carried")
link("connected", 4)
at(4)
T.check(Sx:live() and Sx.id_changed and Sx.id_changed.from == 1 and Sx.id_changed.to == 4, "a new peer id is flagged")
Sx.id_changed = nil
at(4.5)
T.check(Sx.id_changed == nil and Sx:quiet_s(5) == 1, "unchanged link: no flag, quiet since its change")

-- rate limit: every_s
local S2 = HS.new({ ipc = IPC, clock = function() return clk end })
S2:poll(); local r = S2.reads; S2:poll(); S2:poll()
T.check(S2.reads == r, "polls are rate-limited (every_s 0.25 s)")
clk = clk + 0.3; S2:poll()
T.check(S2.reads == r + 1, "next read after every_s")

-- The normalised view: phases, roster, spawn orders, winner, countdown.
local P = S.ENUMS.phase
local function row(seat, peer, t)
    local r = { seat = seat, peer_id = peer, connected = peer ~= 0, alive = true, wins = seat, nick = "P" .. seat,
                ready = true, spawn_id = 0, spawn_pos = { 0, 0, 0 } }
    for k, v in pairs(t or {}) do r[k] = v end
    return r
end
N.sc_put("session", {
    epoch = 77, seq = 9, match_id = 1234, round = 1, phase = P.LOADING, winner_seat = 255,
    phase_deadline_ms = 50000, server_time_ms = 20000, has_frozen = true,
    config = { arena = "Map_Arena_Yard", best_of = 3, countdown_s = 3 },
    frozen = { arena = "Map_Arena_Pit", best_of = 5, countdown_s = 3 },
    rows = {
        row(1, 10, { spawn_id = (2 << 8) | 1, spawn_slot = 2, spawn_pos = { 1, 2, 3 }, spawn_yaw = 90, spawn_protect_ms = 3000 }),
        row(2, 4, { waiting = true, admin_role = 2 }),
        row(3, 0, { alive = false }),
        row(4, 12, { role = 1 }),
    },
})
local v = HS.view({ ipc = IPC, clock = function() return clk end })
T.check(v and v.state == "countdown" and v.phase_name == "loading" and v.pending_round == 2, "loading = legacy countdown, pending round", T.repr(v and v.state))
T.check(v.arena == "Map_Arena_Pit" and v.best_of == 5 and v.countdown_s == 3, "frozen config in a match; LOADING shows the frozen countdown")
T.check(v.my_peer_id == 4 and v.my_seat == 2 and v.me.waiting == true, "my seat from the link's peer id")
T.check(v.remote_fighters == 2, "seats 1 and 3 (disconnected) are remote fighters", v.remote_fighters)
T.check(#v.waiting_on == 1 and v.waiting_on[1] == 4 and #v.scoreboard == 3 and v.by_peer[12] and not v.by_peer[0], "scoreboard / waiting by peer id")
local sp = v.spawns[10]
T.check(sp and sp.spawn_id == 513 and sp.slot == 2 and sp.x == 1 and sp.z == 3 and sp.yaw == 90 and v.spawn_round == 2, "spawn orders by peer", T.repr(sp))
T.check(v.last_winner == 0 and v.reason == "", "no winner")
N.sc_put("session", { epoch = 77, seq = 10, round = 1, phase = P.ROUND_OVER, winner_seat = 1, result_reason = 3,
    phase_deadline_ms = 25000, server_time_ms = 20000, config = { arena = "Map_Arena_Yard" }, rows = { row(1, 10) } })
v = HS.view({ ipc = IPC, clock = function() return clk end })
T.check(v.state == "roundover" and v.last_winner == 10 and v.reason == "forfeit" and v.countdown_s == 5, "winner seat -> peer, reason, deadline")
clk = clk + 2
v = HS.view({ ipc = IPC, clock = function() return clk end })
T.check(v.countdown_s == 3 and v.deadline_in_ms == 3000, "the countdown runs from the receipt time", tostring(v.deadline_in_ms))

-- notice texts (the sidecar's old wording)
T.check(HS.notice_text({ code = 3, args = { "Mate", "spawn_timeout", "2", "2" } }) == "Mate failed to load: spawn_timeout", "load_failed text")
T.check(HS.notice_text({ code = 1, args = { "Host", "Mate" } }) == "The host Host left; Mate is the host now", "host_left text")
T.check(HS.notice_text({ code = 4, args = { "Mate", "rejoined" } }) == "Mate rejoined" and HS.notice_text({ code = 5, args = { "Mate", "x" } }) == "Mate disconnected", "join / leave texts")
T.check(HS.notice_name({ code = 5 }) == "player_left", "notice names")

-- the game-mode view (slots `mode` / `zone`)
T.check(HS.mode({ ipc = IPC }) == nil, "no mode record: nil")
local GM = S.ENUMS.game_mode
N.sc_put("mode", { seq = 0, mode = GM.DUEL })
T.check(HS.mode({ ipc = IPC }) == nil, "seq 0 (a server without game modes): nil")
N.sc_put("mode", { seq = 3, match_id = 9, mode = GM.KING_OF_HILL, round = 2, teams = 2, team_rule = 1, target_s = 60,
    server_time_ms = 50000, round_end_ms = 80000, team_wins = { 1, 0, 0, 0 }, team_score = { 23000, 5000, 0, 0 },
    team_alive = { 2, 1, 0, 0 }, kit_label = "", friendly_fire = false,
    rows = { { peer_id = 10, seat = 1, team = 1, kills = 2, deaths = 1, score = 23000, life = 1, alive = true, in_zone = true },
             { peer_id = 12, seat = 2, team = 2, deaths = 2, life = 1, respawning = true, respawn_at_ms = 52000 } } })
N.sc_put("zone", { match_id = 9, center = { 10, 20, 30 }, radius_cm = 400, half_height_cm = 300, holder_seat = 255,
    holder_team = 1, inside = 1 })
local m = HS.mode({ ipc = IPC, clock = function() return clk end })
T.check(m and m.id == "koth" and m.label == "King of the hill" and m.teams == 2 and m.target_s == 60, "mode, teams, target", T.repr(m and m.id))
T.check(m.round_left_ms == 30000 and m.team_wins[1] == 1 and m.team_score[1] == 23000 and m.team_alive[2] == 1, "clock and team numbers")
T.check(m.rows[10].kills == 2 and m.rows[10].in_zone and m.by_seat[2].respawn_in_ms == 2000, "rows by peer and seat", T.repr(m.by_seat[2]))
T.check(m.zone and m.zone.r == 400 and m.zone.x == 10 and m.zone.holder_team == 1 and m.zone.holder_seat == nil, "the hill", T.repr(m.zone))
clk = clk + 5
m = HS.mode({ ipc = IPC, clock = function() return clk end })
T.check(m.round_left_ms == 25000 and m.by_seat[2].respawn_in_ms == 0, "the clocks run from the receipt", tostring(m.round_left_ms))
N.sc_put("zone", { match_id = 8, center = { 0, 0, 0 }, radius_cm = 400, half_height_cm = 300 })
m = HS.mode({ ipc = IPC, clock = function() return clk end })
T.check(m.zone == nil, "a zone of another match is not shown")

T.check(HS.VERSION == 2 and HS.TERMINAL.server_closed, "module surface")
