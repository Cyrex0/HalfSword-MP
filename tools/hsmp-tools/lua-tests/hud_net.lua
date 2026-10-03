-- HSMPHud net chip grade: sensible thresholds for internet play.
--
--     hsmp-tools lua-test hud_net
--
-- A clean 116 ms link must not read "NET BAD". RTT good < 80, ok < 150,
-- poor < 250, bad above; the 10 s loss window caps the grade; RTT
-- variation (rttvar) drops a grade over 50 ms, two over 120 ms.

local HS = dofile(T.path("mods/HSMPHud/Scripts/hud_state.lua"))
local HM = dofile(T.path("mods/HSMPHud/Scripts/hud_model.lua"))
local g = HM.net_grade

T.log("== grade table")
local cases = {
    -- rtt, loss, jitter, rx_late, expected
    { 20, 0, 3, false, "good" },
    { 79, nil, nil, false, "good" },
    { 80, 0, 0, false, "ok" },
    { 116, 0, 5, false, "ok" },      -- the user's link
    { 116, 0.4, 12, false, "ok" },
    { 149, 0, 0, false, "ok" },
    { 150, 0, 0, false, "poor" },
    { 249, 0, 0, false, "poor" },
    { 250, 0, 0, false, "bad" },
    { 400, 0, 0, false, "bad" },
    -- loss caps
    { 40, 0.9, 0, false, "good" },
    { 40, 1.0, 0, false, "ok" },
    { 40, 2.9, 0, false, "ok" },
    { 40, 3.0, 0, false, "poor" },
    { 40, 8.0, 0, false, "poor" },
    { 40, 8.1, 0, false, "bad" },
    { 200, 2.0, 0, false, "poor" },   -- loss never improves a grade
    -- jitter (rttvar)
    { 40, 0, 50, false, "good" },
    { 40, 0, 51, false, "ok" },
    { 116, 0, 60, false, "poor" },
    { 40, 0, 121, false, "poor" },
    { 200, 0, 121, false, "bad" },    -- capped at bad
    -- no datagram
    { 30, 0, 0, true, "bad" },
}
for _, c in ipairs(cases) do
    local got = g(c[1], c[2], c[3], c[4])
    T.check(got == c[5], string.format("rtt %s loss %s jitter %s rx_late %s -> %s", tostring(c[1]), tostring(c[2]),
        tostring(c[3]), tostring(c[4]), c[5]), got)
end

T.log("== the chip end to end (link record -> hud_state metrics_from -> hud_model net)")
local function chip(link, now)
    local T0 = HM.new()
    T0.connected = true
    T0.metrics.changed_at = 0
    link.metrics_wall_ms = link.metrics_wall_ms or 1
    if link.loss_pct_10s == nil then link.loss_pct_10s = -1 end   -- no window yet
    local snap = { sc = { status = "connected" }, metrics = HS.metrics_from(link) }
    return HM.net(T0, snap, now or 1)
end
local n = chip({ rtt_ms = 116, clock_offset_ms = 1, loss_pct = 0, jitter_ms = 8, rx_age_ms = 12 })
T.check(n.level == "ok" and n.text == "NET OK  116 ms  0.0%  +-8", "the user's 116 ms clean link: NET OK (not BAD)", n.text)
n = chip({ rtt_ms = 42 })
T.check(n.level == "good" and n.text == "NET GOOD  42 ms", "42 ms: NET GOOD", n.text)
-- The 10 s window wins over the lifetime figure
n = chip({ rtt_ms = 60, loss_pct = 12, loss_pct_10s = 0, jitter_ms = 4 })
T.check(n.level == "good" and n.text:find("0.0%%") ~= nil, "a bad early minute (lifetime 12 %) no longer pins NET BAD", n.text)
n = chip({ rtt_ms = 60, loss_pct = 0.1, loss_pct_10s = 9.5 })
T.check(n.level == "bad", "current loss (10 s window 9.5 %) shows", n.text)
n = chip({ rtt_ms = 60, loss_pct = 4 })
T.check(n.level == "poor", "no window yet: the lifetime loss is the fallback", n.text)
n = chip({ rtt_ms = 40, loss_pct = 0, rx_age_ms = 1200 })
T.check(n.level == "good", "a 1.2 s RX gap is not yet bad (1.5 s)", n.text)
n = chip({ rtt_ms = 40, loss_pct = 0, rx_age_ms = 1600 })
T.check(n.level == "bad", "no datagram for 1.6 s: bad", n.text)
n = chip({ rtt_ms = 40 }, 6)
T.check(n.level == "bad" and n.text == "NET BAD  NO DATA", "metrics unchanged 5 s: NO DATA", n.text)

T.log("== typed records end to end (mock: link / session / peer dir / bus / notice events)")
local NM = require("hsmp_native_mock")
local S = NM.S
local N = NM.new{}
local clk = 0
local IPC = dofile(T.path("mods/shared/hsmp_ipc.lua"))
IPC.init{ mod = "HSMPHud", native = N, clock = function() return clk end, log = function() end }
local ST, LS, P = S.ENUMS.sidecar_status, S.ENUMS.link_state, S.ENUMS.phase
N.sc_put("link", { status = ST.CONNECTED, state = LS.UP, my_peer_id = 3, is_admin = true, rtt_ms = 116,
                   loss_pct = 0, loss_pct_10s = -1, jitter_ms = 8, rx_age_ms = 12, metrics_wall_ms = 1000 })
N.sc_peer_dir({ { id = 1, slot = 0, nick = "A \"q\"", rtt_ms = 44 }, { id = 3, slot = 1, nick = "Me", rtt_ms = 0 } })
N.sc_put("session", { epoch = 5, seq = 7, round = 2, phase = P.LIVE, winner_seat = 255,
    config = { arena = "Map_Arena_Pit", best_of = 5 },
    rows = { { seat = 1, peer_id = 1, connected = true, alive = true, wins = 1, ready = true, spawn_pos = { 0, 0, 0 } },
             { seat = 2, peer_id = 3, connected = true, alive = false, wins = 0, spawn_pos = { 0, 0, 0 } } } })
IPC.bus_put("conn_state", { seq = 4, state = "reconnecting", reason = "link_lost", remaining_s = 27, window_s = 35,
                            in_match = true, actions = { "reconnect", "menu" } })
IPC.bus_put("spectate", { target = 1, nick = "A", alive = 1 })
local snap = HS.read_all("unused_state_dir")
T.check(snap.sc and snap.sc.status == "connected" and snap.sc.my_id == 3 and snap.sc.nicks[1] == "A \"q\""
    and snap.sc.ping[1] == 44 and snap.sc.ping[3] == nil and snap.is_admin, "sidecar view from link + peer dir", T.repr(snap.sc))
local m = snap.match
T.check(m and m.state == "live" and m.round == 2 and m.best_of == 5 and m.arena == "Map_Arena_Pit" and m.seq == 7
    and m.order[1] == 1 and m.wins[1] == 1 and m.alive[3] == false and m.ready[1] and not m.ready[3], "match view from the session record", T.repr(m))
T.check(snap.metrics and snap.metrics.rtt == 116 and snap.metrics.loss == 0 and snap.metrics.last_rx == 12
    and snap.metrics_raw == 1000, "metrics from the link (no 10 s window: lifetime loss)", T.repr(snap.metrics))
T.check(snap.conn and snap.conn.state == "reconnecting" and snap.conn.remaining == 27 and #snap.conn.actions == 2
    and snap.conn.in_match, "conn_state from the bus", T.repr(snap.conn))
T.check(snap.spectate and snap.spectate.target == 1 and snap.spectate.nick == "A", "spectate from the bus")
IPC.bus_clear("spectate")
IPC.bus_clear("conn_state")
local snap2 = HS.read_all("x")
T.check(snap2.spectate == nil and snap2.conn == nil, "cleared bus keys read as absent")
N.sc_put("link", { status = ST.REJECTED, state = LS.TERMINAL, reason = "server full (2 peers)", metrics_wall_ms = 0 })
local snap3 = HS.read_all("x")
T.check(snap3.rejected == "server full (2 peers)" and snap3.metrics == nil, "rejected: the link's reason; no metrics yet")

-- notices: typed S2G events, deduped by event_id, worded by hsmp_session
local tail = HS.new_tail("notice")
T.check(#HS.poll_notices(tail) == 0, "no notices yet")
N.sc_rec_event("notice", { event_id = 9, code = S.ENUMS.notice.PLAYER_JOINED, args = { "Mate", "joined" } })
N.sc_rec_event("notice", { event_id = 9, code = S.ENUMS.notice.PLAYER_JOINED, args = { "Mate", "joined" } })
N.sc_rec_event("notice", { event_id = 10, code = S.ENUMS.notice.LOAD_FAILED, args = { "Mate", "spawn_timeout", "2", "2" } })
local got = HS.poll_notices(tail)
T.check(#got == 2 and got[1].text == "Mate joined" and got[1].name == "player_joined"
    and got[2].text == "Mate failed to load: spawn_timeout", "notices once each, worded", T.repr(got))

-- Liveness is the header heartbeat, not "the status changed recently"
local T1 = HM.new()
local s4 = { sc = { status = "connected" }, sidecar_raw = 1, hb_age = 0.2 }
HM.observe(T1, s4, 100, { in_arena = false })
T.check(T1.connected and T1.mp.active, "connected + beating sidecar: connected, though the link never changed")
HM.observe(T1, { sc = { status = "connected" }, sidecar_raw = 1, hb_age = 30 }, 101, { in_arena = false })
T.check(not T1.connected and not T1.mp.active and T1.mp.dead, "a 'connected' link whose sidecar stopped beating is no session")
local T2 = HM.new()
for i = 1, 5 do HM.observe(T2, { sc = { status = "connected" }, sidecar_raw = i, hb_age = nil }, i, { in_arena = false }) end
T.check(not T2.connected, "a changing link without a heartbeat is not live (no line-change heuristic)")
