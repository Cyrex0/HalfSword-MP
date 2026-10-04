-- Connection-flow scenarios (docs/development/subsystems/director.md): the Director
-- driven through every sidecar status sequence
--   connected -> reconnecting -> connected | server_closed | kicked | replaced
-- plus link stalls (the link record's state), a hung sidecar (its heartbeat
-- stops), server restarts (the session epoch), the menu return flag, the
-- player's modal buttons (RECONNECT / BACK TO MENU / LEAVE / REMATCH as bus
-- travel_request wants) and the HUD model reading the same records.
--
--   hsmp-tools lua-test flow
--
-- Regression ("dropped connection mid round and now it's looping"):
-- the stale last state ("live") sent the Director from the menu back into the
-- arena, where the lost link sent it to the menu again, every ~15 s.

local DW = require("dir_world")
local D, SD = DW.D, DW.SD

local function count(list, v) local n = 0; for _, x in ipairs(list) do if x == v then n = n + 1 end end; return n end
local function nlog(w, s) return T.count(w:logtext(), s) end
local function arena_entries(w) return count(w.opens, "Map_Arena_Pit") end
local function menu_entries(w) return count(w.opens, "Map_Menu_Startup") end

-- ---------------------------------------------------------------------------
T.log("== REGRESSION: link lost mid-round, stale .match.json says live -> ONE menu transition, no loop")
do
    local w = DW.to_live({ epoch = "123456789012345678" })
    w.auto_load = true
    local a0 = arena_entries(w)
    w:sidecar_status("reconnecting")       -- .match.json keeps its last line: "live"
    w:tick(8)                              -- 2 s
    local c = w:conn()
    T.check(c and c.state == "reconnecting" and c.in_match == true, "link down in a match -> state reconnecting", T.repr(c))
    T.check(w.frozen == w.pawn.id, "world frozen for the player while reconnecting")
    T.check(c.remaining_s <= D.T.resume_window_s and c.remaining_s >= D.T.resume_window_s - 3,
        "the overlay counts the resume window down", c.remaining_s)
    w:tick(4 * 30)                         -- 32 s total: still inside the window
    T.check(#w.opens == a0, "no travel at all inside the resume window", T.repr(w.opens))
    w:tick(4 * 6)                          -- past 35 s
    c = w:conn()
    T.check(menu_entries(w) == 1 and w.dir.state ~= "Live", "window over -> exactly one travel to the menu", T.repr(w.opens))
    T.check(c.state == "lost" and c.reason == "link_lost" and c.latched == true, "conn_state lost/link_lost, latched", T.repr(c))
    T.check(T.eq(c.actions, { "reconnect", "menu" }), "modal buttons RECONNECT / BACK TO MENU", T.repr(c.actions))
    T.check(T.contains(c.title, "CONNECTION LOST") and T.contains(c.text, "35"), "clear modal text", c.title .. " / " .. c.text)
    T.check(w:rtl() == 0, "no lobby hand-back (the session is broken)")
    -- the menu world: stale "live" must never pull us back in
    w:tick(4 * 120)                        -- 2 more minutes
    T.check(arena_entries(w) == a0 and menu_entries(w) == 1, "no arena re-entry, no second menu travel (no loop)", T.repr(w.opens))
    T.check(nlog(w, "connection LOST") == 1, "the loss is reported exactly once")
    -- the menu's lobby poll still asks for the arena: refused while latched
    w:request(50, "arena", "Map_Arena_Pit", "match_countdown"); w:tick(1)
    local a = w:ack()
    T.check(a and a.status == "refused" and T.contains(a.reason, "RECONNECT"), "menu request for the arena refused while latched", T.repr(a))
    T.check(w.dir:on_native_open_level("Map_Arena_Pit") == "Map_Menu_Startup", "a native arena load while latched is rewritten to the menu")
    -- the sidecar finally reconnects in the background: still no automatic travel
    w:sidecar_status("connected"); w:tick(40)
    T.check(arena_entries(w) == a0, "a background reconnect does not auto-travel (the player decides)")
    T.check(w:conn().state == "lost", "the modal stays until the player answers")
    -- RECONNECT: follow the server again (it is still in the match)
    w:request(51, "reconnect", nil, "modal"); w:tick(1)
    a = w:ack()
    T.check(a and a.status == "accepted", "RECONNECT accepted", T.repr(a))
    T.check(w:conn().state == "ok" and not w:conn().latched, "latch cleared by RECONNECT")
    w:match("countdown", "Map_Arena_Pit", 1); w:tick(12)
    T.check(arena_entries(w) == a0 + 1, "after RECONNECT the Director follows the server back into the arena, once", T.repr(w.opens))
end

-- ---------------------------------------------------------------------------
T.log("== a short stall (< 3 s) is invisible; an 8 s blackout resumes in place / with the server's replay")
do
    local w = DW.to_live()
    w.auto_load = true
    local n0 = #w.opens
    w:link("up", 2000); w:tick(8)
    T.check(w:conn() == nil or w:conn().state == "ok", "2 s without a packet: no overlay")
    T.check(w.frozen == false, "and the player keeps control")
    -- DoD-11: 8 s blackout; the transport survives (idle timeout 10 s), status stays connected
    for i = 1, 32 do w:link("up", 3000 + i * 250); w:tick(1) end
    local c = w:conn()
    T.check(c.state == "reconnecting" and c.reason == "stalled", "stalled link -> reconnecting overlay", T.repr(c))
    T.check(w.frozen == w.pawn.id, "frozen during the blackout")
    T.check(#w.opens == n0, "no travel during the blackout")
    -- the server paused the duel and replays the round once we are back
    w:match("countdown", "Map_Arena_Pit", 1)
    w:link("up", 100); w:tick(2)
    T.check(w:conn().state == "reconnecting", "link back: waits resume_settle_s for fresh state")
    w:tick(4)
    T.check(w:conn().state == "ok", "resumed", T.repr(w:conn()))
    T.check(T.contains(w:logtext(), "link restored after"), "logged")
    local rs = w:last_ev("resume")
    T.check(rs and rs.ok == true and rs.away_s > 7, "resume{ok=true, away_s}", T.repr(rs))
    T.check(#w.opens == n0 + 1 and w.opens[#w.opens] == "Map_Arena_Pit", "the replayed round reloads the SAME arena once", T.repr(w.opens))
    T.check(menu_entries(w) == 0, "never via the menu")

    -- resume while the server kept the round live: no reload at all
    local w2 = DW.to_live()
    local m0 = #w2.opens
    w2:sidecar_status("reconnecting"); w2:tick(20)          -- 5 s, new handshake pending
    T.check(w2:conn().state == "reconnecting", "reconnecting (new handshake)")
    w2.my_id = 3                                           -- a new peer id after the re-handshake
    w2:sidecar_status("connected"); w2:tick(8)
    T.check(w2:conn().state == "ok" and #w2.opens == m0 and w2.dir.state == "Live", "same round still live: continue, no reload", w2.dir.state)
    T.check(w2.frozen == false, "input released again")
end

-- ---------------------------------------------------------------------------
T.log("== resume fails: server_closed / kicked / replaced -> one transition with the reason")
do
    local cases = {
        { st = "server_closed", file = function(w) w:rejected("Host closed the server") end, title = "SERVER CLOSED", text = "Host closed the server" },
        { st = "kicked", file = function(w) w:kicked("spamming") end, title = "YOU WERE KICKED", text = "spamming" },
        { st = "replaced", file = function() end, title = "DISCONNECTED", text = "another game" },
        { st = "rejected", file = function(w) w:rejected("banned") end, title = "CONNECTION REJECTED", text = "banned" },
    }
    for _, k in ipairs(cases) do
        local w = DW.to_live()
        w.auto_load = true
        local a0 = arena_entries(w)
        w:sidecar_status("reconnecting"); w:tick(12)
        k.file(w); w:sidecar_status(k.st); w:tick(2)
        local c = w:conn()
        T.check(c.state == "lost" and c.reason == k.st and c.title == k.title and T.contains(c.text, k.text),
            k.st .. ": modal with the reason", T.repr(c))
        T.check(T.eq(c.actions, { "menu" }), k.st .. ": only BACK TO MENU (reconnecting cannot help)", T.repr(c.actions))
        T.check(menu_entries(w) == 1, k.st .. ": one travel to the menu, at once (no window wait)", T.repr(w.opens))
        local tr = w:last_ev("travel")
        T.check(tr and T.contains(tr.reason, k.text), k.st .. ": travel reason names it (gate DoD-11)", tr and tr.reason)
        w:tick(240)
        T.check(menu_entries(w) == 1 and arena_entries(w) == a0, k.st .. ": no loop afterwards", T.repr(w.opens))
        w:request(9, "reconnect", nil, "modal"); w:tick(1)
        T.check(w:ack().status == "refused", k.st .. ": RECONNECT refused (the session ended)", T.repr(w:ack()))
        w:request(10, "dismiss", nil, "modal"); w:tick(1)
        T.check(w:ack().status == "accepted" and w:conn().state == "ok" and w:left(),
            k.st .. ": BACK TO MENU clears the modal and ends the session (a leave record)")
    end
    -- host left mid-Live with the link up: status server_closed directly
    local w = DW.to_live()
    w:rejected("Host closed the server"); w:sidecar_status("server_closed"); w:tick(1)
    T.check(menu_entries(w) == 1 and w:conn().reason == "server_closed", "host CANCEL mid-Live -> menu within one tick")
end

-- ---------------------------------------------------------------------------
T.log("== kicked in the lobby: modal, no travel")
do
    local w = DW.new()
    w:sidecar_status("connected"); w:match("lobby", "Map_Arena_Pit", 0); w:start(); w:tick(4)
    w:kicked("afk"); w:sidecar_status("kicked"); w:tick(2)
    T.check(w:conn().state == "lost" and w:conn().text == "afk" and #w.opens == 0, "lobby kick: modal, no travel", T.repr(w:conn()))
    -- the menu joins another server: a new (non-terminal) sidecar status clears the latch
    w:sidecar_status("connecting"); w:tick(2)
    T.check(w:conn().state == "ok", "a new session clears the old modal")
end

-- ---------------------------------------------------------------------------
T.log("== server restart (new epoch) -> lobby with a message; match ended while away")
do
    local w = DW.to_live({ epoch = "1111" })
    w.auto_load = true
    w:session(1111, 7); w:tick(4)
    w:sidecar_status("reconnecting"); w:tick(40)
    w:match("lobby", "Map_Arena_Pit", 0); w:session(2222, 0)
    w:sidecar_status("connected"); w:tick(8)
    local c = w:conn()
    T.check(c.state == "notice" and c.reason == "server_restarted" and not c.latched, "new epoch -> notice server_restarted", T.repr(c))
    T.check(T.eq(c.actions, { "ok" }), "notice has one OK button")
    T.check(menu_entries(w) == 1 and w:rtl() > 0, "back to the menu LOBBY (return flag, session alive)", T.repr(w.opens))
    w:request(3, "ok", nil, "modal"); w:tick(1)
    T.check(w:conn().state == "ok" and not w:left(), "OK closes the notice; the session stays")
    -- same epoch, but the server is back in the lobby (forfeit while we were away)
    local w2 = DW.to_live({ epoch = "77" })
    w2:sidecar_status("reconnecting"); w2:tick(40)
    w2:match("lobby", "Map_Arena_Pit", 0); w2:sidecar_status("connected"); w2:tick(12)
    T.check(w2:conn().reason == "match_ended_away" and menu_entries(w2) == 1, "match over while away -> notice + lobby", T.repr(w2:conn()))
    -- an epoch seen for the first time is not a restart
    local w3 = DW.to_live()
    w3:session(999, 7); w3:tick(8)
    T.check(w3:conn() == nil or w3:conn().state == "ok", "first epoch: no notice")
end

-- ---------------------------------------------------------------------------
T.log("== hung sidecar, leave, lobby requests while down")
do
    local w = DW.to_live()
    w.auto_load = true
    w.sidecar_frozen = true                -- the sidecar's heartbeat stops (crash / hang)
    w:tick(4 * 8)
    T.check(w:conn().state == "reconnecting" and w:conn().reason == "sidecar_stopped", "status file frozen > 6 s -> reconnecting", T.repr(w:conn()))
    w:tick(4 * 36)
    T.check(w:conn().state == "lost" and w:conn().reason == "sidecar_stopped" and menu_entries(w) == 1,
        "never came back -> lost: sidecar_stopped", T.repr(w:conn()))

    -- LEAVE MATCH (MP pause): ask the sidecar to leave, travel once, no modal
    local w2 = DW.to_live()
    w2.auto_load = true
    w2:request(4, "leave", nil, "mp_pause"); w2:tick(1)
    T.check(w2:ack().status == "accepted" and w2:left() and menu_entries(w2) == 1,
        "LEAVE: a leave record + one travel to the menu", T.repr(w2.opens))
    w2:sidecar_status(nil); w2:tick(20)
    T.check((w2:conn() or {}).state ~= "lost" and menu_entries(w2) == 1, "the session ending afterwards is not a 'lost' modal",
        T.repr({ w2:conn(), w2.opens }))

    -- not in a match, link down: the menu's arena request is refused (stale state)
    local w3 = DW.new()
    w3:sidecar_status("connected"); w3:match("lobby", "Map_Arena_Pit", 0); w3:start(); w3:tick(4)
    w3:sidecar_status("reconnecting"); w3:match("live", "Map_Arena_Pit", 1); w3:tick(4)
    w3:request(8, "arena", "Map_Arena_Pit", "match_countdown"); w3:tick(1)
    T.check(w3:ack().status == "refused" and #w3.opens == 0, "stale live while down: no travel from the menu", T.repr(w3:ack()))
    T.check((w3:conn() or {}).state ~= "reconnecting", "no in-match overlay in the menu")
    w3:sidecar_status("connected"); w3:tick(2)
    T.check(#w3.opens == 0, "link back: still waits for fresh state (settle)")
    w3:tick(4)
    T.check(#w3.opens == 1, "then follows the server into its match", T.repr(w3.opens))
end

-- ---------------------------------------------------------------------------
T.log("== match result: REMATCH / BACK TO LOBBY")
do
    local w = DW.to_live({ epoch = "5" })
    w.auto_load = true
    local n0 = #w.opens
    w:match("match_over", "Map_Arena_Pit", 2, nil, { winner = 1 }); w:tick(4)
    w:request(30, "rematch", nil, "result screen"); w:tick(1)
    T.check(w:ack().status == "accepted", "REMATCH accepted in match_over", T.repr(w:ack()))
    w:match("lobby", "Map_Arena_Pit", 0); w:tick(12)
    T.check(#w.opens == n0, "REMATCH: stays in the arena through the lobby", T.repr(w.opens))
    local cmds = w:sent("command")
    local OP = DW.S.ENUMS.cmd_op
    T.check(T.any(cmds, function(c) return c.op == OP.READY and c.flag == true end)
        and T.any(cmds, function(c) return c.op == OP.START end), "ready + (host) start sent", T.repr(cmds))
    local ids = T.map(cmds, function(c) return c.cmd_id end)
    T.check(#ids >= 2 and #T.keys(T.set(ids)) == #ids, "every command has its own cmd_id")
    w:match("countdown", "Map_Arena_Pit", 0); w:tick(4)
    T.check(#w.opens == n0 + 1 and w.opens[#w.opens] == "Map_Arena_Pit", "host START -> the same arena reloads for round 1", T.repr(w.opens))

    local w2 = DW.to_live()
    w2:match("match_over", "Map_Arena_Pit", 2); w2:tick(2)
    w2:request(31, "rematch"); w2:tick(1)
    w2:match("lobby", "Map_Arena_Pit", 0)
    w2:tick(4 * 16)
    T.check(menu_entries(w2) == 1 and w2:rtl() > 0, "no START within 15 s -> lobby screen (timeout, never stuck)", T.repr(w2.opens))

    -- Staying in the arena on purpose (result screen, REMATCH hold)
    -- must not let a native travel (the win/lose flow's Tavern) through.
    local w4 = DW.to_live()
    w4:match("match_over", "Map_Arena_Pit", 2, nil, { winner = 1 }); w4:tick(4)
    T.check(w4.dir:on_native_open_level("Map_Hub_Tavern_Frank") == "Map_Arena_Pit",
        "result screen (match_over): a native Tavern travel is rewritten to the arena we stay in")
    w4:request(33, "rematch", nil, "result screen"); w4:tick(1)
    w4:match("lobby", "Map_Arena_Pit", 0); w4:tick(2)
    T.check(w4.dir:rematch_holding() and w4.dir:on_native_open_level("Map_Hub_Tavern_Frank") == "Map_Arena_Pit",
        "REMATCH hold in the lobby: a native Tavern travel is rewritten to the arena")
    T.check(w4.dir:on_soft_travel_post("soft-object-ptr") == "Map_Arena_Pit" and w4.opens[#w4.opens] == "Map_Arena_Pit",
        "REMATCH hold: a native soft-object travel is re-routed to the arena", T.repr(w4.opens))
    T.check(w4.dir:on_native_open_level("Map_Arena_Pit") == nil, "a travel to the arena itself passes through")

    local w3 = DW.to_live()
    w3:match("match_over", "Map_Arena_Pit", 2); w3:tick(2)
    w3:request(32, "menu", nil, "back to lobby"); w3:tick(1)
    T.check(w3:ack().status == "accepted" and menu_entries(w3) == 1 and w3:rtl() > 0,
        "BACK TO LOBBY in match_over -> menu lobby now", T.repr(w3:ack()))
end

-- ---------------------------------------------------------------------------
T.log("== de-duplication: every state change is published once")
do
    local w = DW.to_live()
    w:sidecar_status("reconnecting"); w:tick(4 * 40)
    local seqs = {}
    for _, e in ipairs(w:evs_named("conn_state")) do seqs[#seqs + 1] = e.state .. ":" .. tostring(e.reason) end
    T.check(T.eq(seqs, { "reconnecting:reconnecting", "lost:link_lost" }), "conn_state events: reconnecting, lost (no repeats)", T.repr(seqs))
    local c = w:conn()
    T.check(c.seq == 2, "conn_state.seq bumps only on a change", c.seq)
end

-- ---------------------------------------------------------------------------
-- What the PLAYER sees: HSMPHud's model (pure) reading the same files, driven
-- tick by tick next to the Director. The banner is HSMPHud's alone now.
local HS = dofile(T.path("mods/HSMPHud/Scripts/hud_state.lua"))
local HM = dofile(T.path("mods/HSMPHud/Scripts/hud_model.lua"))

local HSS = dofile(T.path("mods/shared/hsmp_session.lua"))
-- What HSMPHud reads (hud_state.read_all), from the same world's records.
local function hud_snap(w)
    local I = w.IPC
    local o = { ipc = I, clock = function() return w.clock end }
    local link, lver = I.rec("link")
    local snap = { sidecar_raw = lver, vhud = { peers = {} }, vremote = {}, link = link }
    snap.sc = HS.sidecar_from(link, { { id = 1, nick = "Me" }, { id = 2, nick = "Bob" } })
    snap.match = HS.match_from(HSS.view(o))
    snap.conn = HS.conn_from(I.bus_table("conn_state"))
    snap.spectate = HS.spectate_from(I.bus_table("spectate"))
    snap.hb_age = HSS.hb_age(o)
    snap.is_admin = link ~= nil and link.is_admin == true
    return snap
end
-- Run n Director ticks; after each, what the HUD shows (centre title + panel).
local function watch(w, n, H)
    H = H or { T = HM.new(), seen = {} }
    for _ = 1, n do
        w:tick(1)
        local snap = hud_snap(w)
        local in_arena = w.world.short and w.world.short:find("^Map_Arena_") ~= nil
        HM.observe(H.T, snap, w.clock, { in_arena = in_arena })
        local spec = HM.panel(H.T, snap, w.clock, { in_arena = in_arena })
        local m = HM.build(H.T, snap, w.clock, { in_arena = in_arena, centre_enabled = true, modal = spec and spec.modal })
        local shown = (spec and spec.modal) and ("panel:" .. spec.kind .. ":" .. tostring(spec.title))
            or (m.centre and ("centre:" .. m.centre.title)) or "-"
        if shown ~= H.seen[#H.seen] then H.seen[#H.seen + 1] = shown end
        H.spec, H.model = spec, m
    end
    return H
end
local function seen_has(H, s) for _, x in ipairs(H.seen) do if T.contains(x, s) then return true end end return false end
local function count_of(H, s) local n = 0; for _, x in ipairs(H.seen) do if T.contains(x, s) then n = n + 1 end end return n end

T.log("== HUD: the reconnect-loop regression as the player sees it")
do
    local w = DW.to_live()
    w.auto_load = true
    w:sidecar_status("reconnecting")
    local H = watch(w, 8)
    T.check(H.seen[#H.seen] == "centre:RECONNECTING...", "overlay RECONNECTING... in the arena", T.repr(H.seen))
    T.check(T.contains(H.model.centre.sub, "s left"), "with the seconds left", H.model.centre.sub)
    T.check(H.model.net and T.contains(H.model.net.text, "NO LINK"), "net indicator: no link")
    watch(w, 4 * 40, H)
    T.check(seen_has(H, "panel:conn:CONNECTION LOST"), "then the modal: CONNECTION LOST", T.repr(H.seen))
    watch(w, 4 * 120, H)
    T.check(count_of(H, "RECONNECTING") == 1 and count_of(H, "panel:conn") == 1,
        "RECONNECTING once, the modal once: no loop between them", T.repr(H.seen))
    T.check(H.spec and H.spec.kind == "conn" and H.spec.buttons[1].label == "RECONNECT"
        and H.spec.buttons[2].label == "BACK TO MENU", "buttons RECONNECT / BACK TO MENU", T.repr(H.spec and H.spec.buttons))
    -- the player clicks BACK TO MENU in the HUD modal -> bus ui_request -> Director
    local want = HM.click(H.T, "conn:menu", w.clock)
    T.check(want == "dismiss", "BACK TO MENU = want dismiss")
    w:ui_request(1, "dismiss", "conn:menu")
    watch(w, 2, H)
    T.check(w.dir.ui_seq == 1, "the Director serves the HUD request (no ack, A0)", T.repr(w.dir.ui_seq))
    T.check(w:left() and H.spec == nil and w:conn().state == "ok",
        "session ended for the sidecar, modal gone")
    T.check(w:rtl() == 0, "no lobby hand-back: the menu shows its main screen")
end

T.log("== HUD: phase UI through a match (waiting, countdown, FIGHT, spectate, results, rematch)")
do
    local w = DW.new()
    w:sidecar_status("connected"); w:match("lobby", "Map_Arena_Pit", 0); w:start(); w:tick(4)
    w.auto_load = true
    w:match("countdown", "Map_Arena_Pit", 0, nil, { waiting = { 2 } })
    local H = watch(w, 6)
    T.check(H.model.centre and H.model.centre.title == "WAITING FOR PLAYERS" and T.contains(H.model.centre.sub, "Bob"),
        "waiting for players, by name", T.repr(H.model.centre))
    w:match("countdown", "Map_Arena_Pit", 0); watch(w, 2, H)
    T.check(H.model.centre.title == "ROUND 1" and T.contains(H.model.centre.sub, "fight in 3"), "countdown", T.repr(H.model.centre))
    w:match("live", "Map_Arena_Pit", 1); watch(w, 1, H)
    T.check(H.model.centre and H.model.centre.title == "FIGHT!", "FIGHT!")
    -- I die; HSMPMatch spectates Bob
    w:match("live", "Map_Arena_Pit", 1, nil, { countdown = 0, dead = { 1 } })
    w.IPC.bus_put("spectate", { target = 2, nick = "Bob", round = 1, alive = 1 })
    watch(w, 20, H)
    T.check(H.model.centre.title == "SPECTATING BOB", "Spectating <name>", T.repr(H.model.centre))
    -- nobody left to watch: HSMPMatch shows the arena overview
    w.IPC.bus_put("spectate", { target = 0xFFFFFFFF, nick = "", round = 1, alive = 0 })
    watch(w, 2, H)
    T.check(H.model.centre.title == "ARENA VIEW", "arena view label", T.repr(H.model.centre))
    w.IPC.bus_put("spectate", { target = 2, nick = "Bob", round = 1, alive = 2 })
    watch(w, 2, H)
    T.check(H.model.centre.title == "SPECTATING BOB" and T.contains(H.model.centre.sub, "Q / E switch"), "back on Bob, Q/E hint", T.repr(H.model.centre))
    H.T.tab = true; watch(w, 1, H)
    T.check(H.model.board ~= nil and H.model.centre == nil, "TAB scoreboard while spectating (centre hidden)")
    H.T.tab = false
    w:match("roundover", "Map_Arena_Pit", 1, nil, { winner = 2, reason = "kill" }); watch(w, 2, H)
    T.check(H.model.centre.title == "ROUND OVER" and T.contains(H.model.centre.sub, "Bob wins the round")
        and T.contains(H.model.centre.sub, "next round in"), "round result with the winner and score", T.repr(H.model.centre))
    w:match("match_over", "Map_Arena_Pit", 2, nil, { winner = 1, reason = "kill" }); watch(w, 2, H)
    T.check(H.model.centre.title == "VICTORY", "match result", T.repr(H.model.centre))
    T.check(H.spec and H.spec.kind == "result" and H.spec.buttons[1].label == "REMATCH"
        and H.spec.buttons[2].label == "BACK TO LOBBY" and not H.spec.modal, "REMATCH / BACK TO LOBBY under the result")
    local want = HM.click(H.T, "result:rematch", w.clock)
    w:ui_request(7, "rematch", "result")
    watch(w, 2, H)
    T.check(want == "rematch" and w.dir.ui_seq == 7, "REMATCH reaches the Director")
    w:match("lobby", "Map_Arena_Pit", 0); watch(w, 8, H)
    T.check(H.model.visible and H.model.centre and H.model.centre.title == "REMATCH" and #w.opens == 1,
        "lobby: still in the arena, 'REMATCH - waiting for everyone' shown", T.repr({ H.model.centre, w.opens }))
    watch(w, 4 * 16, H)
    T.check(w.opens[#w.opens] == "Map_Menu_Startup" and not (H.model.centre and H.model.centre.title == "REMATCH"),
        "the hold times out into the lobby screen (never stuck)", T.repr(w.opens))
end

T.log("== HUD: notices become toasts, once each")
do
    local Tt = HM.new()
    HM.add_notices(Tt, { { event_id = 1, name = "player_joined", text = "Mate joined" },
                         { event_id = 2, name = "host_left", text = "The host Al left; Mate is the host now" },
                         { event_id = 3, name = "kill_feed", text = "x" } }, 10)
    HM.add_notices(Tt, { { event_id = 4, name = "player_joined", text = "Mate joined" } }, 11)   -- resend within 5 s
    T.check(#Tt.feed == 2 and Tt.feed[1].text == "Mate joined" and Tt.feed[2].host, "join + host-left toasts, deduped", T.repr(Tt.feed))
    -- S2G notice events (the facade's event cursor, no file):
    -- history skipped, each event_id once
    local prev_ipc = rawget(_G, "HSMP_IPC")
    local N = require("hsmp_native_mock").new()
    local I = dofile(T.path("mods/shared/hsmp_ipc.lua"))
    I.init({ mod = "HSMPHud", native = N, reinit = true })
    N.sc_rec_event("notice", { event_id = 1, code = 4, args = { "Old", "joined" } })
    local tail = HS.new_tail("notices")
    T.check(#HS.poll_notices(tail) == 0, "notices from before this run are not news")
    N.sc_rec_event("notice", { event_id = 2, code = 5, args = { "Mate", "left" } })
    N.sc_rec_event("notice", { event_id = 2, code = 5, args = { "Mate", "left" } })
    local got = HS.poll_notices(tail)
    T.check(#got == 1 and got[1].text == "Mate left" and got[1].name == "player_left", "new notice once", T.repr(got))
    rawset(_G, "HSMP_IPC", prev_ipc)
end

T.log("== HUD: MP pause buttons (confirm with timeout) and every modal has a way out")
do
    local Tt = HM.new()
    Tt.pause_open = true
    local snap = { conn = nil }
    local spec = HM.panel(Tt, snap, 100, { in_arena = true })
    T.check(spec.kind == "pause" and T.eq(T.map(spec.buttons, function(b) return b.label end),
        { "RESUME", "SETTINGS", "LEAVE MATCH", "QUIT GAME" }), "MP pause buttons", T.repr(spec.buttons))
    T.check(HM.click(Tt, "pause:leave", 100) == nil, "LEAVE first asks to confirm")
    spec = HM.panel(Tt, snap, 101, { in_arena = true })
    T.check(spec.buttons[3].label == "CONFIRM: LEAVE MATCH", "confirm label")
    spec = HM.panel(Tt, snap, 105, { in_arena = true })
    T.check(spec.buttons[3].label == "LEAVE MATCH", "the confirm reverts after 4 s")
    HM.click(Tt, "pause:quit", 106)
    local want, action = HM.click(Tt, "pause:quit", 107)
    T.check(want == "leave" and action == "quit" and not Tt.pause_open, "QUIT confirmed: leave the server, then quit")
    -- every modal the Director can publish has at least one button
    for reason, t in pairs(D.LOST_TEXT) do
        local s = { conn = { seq = 1, state = (reason == "server_restarted" or reason == "match_ended_away") and "notice" or "lost",
                             title = t[1], text = t[2], actions = {} } }
        local sp = HM.panel(HM.new(), s, 1, { in_arena = false })
        T.check(sp and #sp.buttons >= 1, "modal '" .. reason .. "' always has a way out", T.repr(sp and sp.buttons))
    end
end

T.log("== menu return flag contract")
do
    -- after a normal match end: the flag (lobby screen); after a loss: none
    local w = DW.to_live()
    w:match("match_over", "Map_Arena_Pit", 2); w:tick(2)
    w:match("lobby", "Map_Arena_Pit", 0); w:tick(12)
    T.check(w:rtl() > 0, "match over -> return_to_lobby (lobby screen)")
    local w2 = DW.to_live()
    w2:kicked("bye"); w2:sidecar_status("kicked"); w2:tick(2)
    T.check(w2:rtl() == 0, "kicked -> no flag (main menu + modal)")
end

-- ---------------------------------------------------------------------------
T.log("== a dead sidecar whose last status is 'connected' ages out")
do
    -- in a match: the sidecar panics / is killed; .sidecar.json keeps "connected" for good
    local w = DW.to_live()
    w.auto_load = true
    w.sidecar_frozen = true
    w:tick(4 * 44)
    T.check(w:conn().state == "lost" and w:conn().reason == "sidecar_stopped" and menu_entries(w) == 1,
        "dead sidecar mid-match -> lost: sidecar_stopped, one travel to the menu", T.repr(w:conn()))
    w:tick(1)
    T.check(w.dir.sess.match_stale == true and w.dir.sess.phase == "none",
        "lose(sidecar_stopped) marks the dead sidecar's .match.json stale", T.repr({ w.dir.sess.match_stale, w.dir.sess.phase }))
    T.check(w.dir:on_native_open_level("Map_Arena_Yard") == "Map_Menu_Startup", "still latched: a native travel goes to the menu")
    w:tick(4 * 6)                                        -- > session_stale_s since the last change
    T.check(not w.dir.sess.exists, "unchanged for session_stale_s: no session process, whatever its status")
    T.check(w:conn().state == "lost" and w:conn().latched, "the modal stays: the dead sidecar is its reason", T.repr(w:conn()))
    w:request(60, "dismiss", nil, "modal"); w:tick(1)
    T.check(w:conn().state == "ok", "BACK TO MENU closes it")
    -- the player starts single-player: nothing is rewritten, no MP GI profile
    local gi_before = w.gi["Free Mode Activated"]
    T.check(w.dir:on_native_open_level("Map_Hub_Tavern_Frank") == nil and w.dir:on_native_open_level("Map_Arena_Pit") == nil,
        "single-player travel is not rewritten into the dead session's arena")
    T.check(w.gi["Free Mode Activated"] == gi_before, "no MP GI profile applied")
    local n = #w.opens
    w:tick(4 * 10)
    T.check(#w.opens == n, "no travel at all afterwards", T.repr(w.opens))

    -- in the lobby: every single-player travel was rewritten to the menu (a dead end)
    local w2 = DW.new()
    w2:sidecar_status("connected"); w2:match("lobby", "Map_Arena_Pit", 0); w2:start(); w2:tick(4)
    w2.sidecar_frozen = true
    w2:tick(4 * 46)
    T.check(not w2.dir.sess.exists and w2.dir:on_native_open_level("Map_Hub_Tavern_Frank") == nil,
        "dead sidecar in the lobby: gone after session_stale_s, single-player travel passes")

    -- a new session after the dead one closes the modal and is followed
    local w3 = DW.to_live()
    w3.auto_load = true
    w3.sidecar_frozen = true
    w3:tick(4 * 50)
    T.check(w3:conn().state == "lost" and not w3.dir.sess.exists, "lost, session gone")
    w3.sidecar_frozen = false                            -- JOIN again: a fresh sidecar
    w3:match("countdown", "Map_Arena_Pit", 1)
    w3:tick(8)
    T.check(w3:conn().state == "ok", "a session process again: the sidecar_stopped modal closes", T.repr(w3:conn()))
    T.check(w3.opens[#w3.opens] == "Map_Arena_Pit", "and the new session's match is followed", T.repr(w3.opens))
end

-- ---------------------------------------------------------------------------
T.log("== RECONNECT while the server is still unreachable: a visible rejoin, no SP hijack")
do
    local w = DW.to_live({ epoch = "4242" })
    w.auto_load = true
    local a0 = arena_entries(w)
    w:sidecar_status("reconnecting"); w:tick(4 * 37)
    T.check(w:conn().state == "lost" and w:conn().reason == "link_lost" and menu_entries(w) == 1, "lost after the resume window", T.repr(w:conn()))
    -- the latch is up: a native travel goes to the menu (as before)
    T.check(w.dir:on_native_open_level("Map_Hub_Tavern_Frank") == "Map_Menu_Startup", "latched: a native travel goes to the menu")
    -- RECONNECT with the link still down
    w:request(60, "reconnect", nil, "modal"); w:tick(1)
    local c = w:conn()
    T.check(w:ack().status == "accepted" and c.state == "reconnecting" and c.reason == "rejoining" and c.latched == false
        and c.title == "REJOINING" and T.eq(c.actions, { "menu" }) and c.remaining_s > 0,
        "RECONNECT while unreachable shows a rejoining overlay with a countdown and BACK TO MENU", T.repr(c))
    -- single player meanwhile: the raw 'live' .match.json must not hijack the travel
    local gi0 = w.gi["Current Game Mode Enum"]
    T.check(w.dir:on_native_open_level("Map_Hub_Tavern_Frank") == nil, "a single-player travel while rejoining is not rewritten into the MP arena")
    T.check(w.gi["Current Game Mode Enum"] == gi0, "and no MP GI profile is applied")
    T.check(arena_entries(w) == a0, "no arena entry while rejoining", T.repr(w.opens))
    -- the rejoin window runs out: lost again (with RECONNECT)
    w:tick(4 * 37)
    c = w:conn()
    T.check(c.state == "lost" and c.latched == true and T.eq(c.actions, { "reconnect", "menu" }), "the rejoin times out into the lost modal again", T.repr(c))
    -- RECONNECT again; this time the link comes back: the server's state counts again
    w:request(61, "reconnect", nil, "modal"); w:tick(1)
    w:sidecar_status("connected"); w:tick(12)
    T.check(w:conn().state ~= "lost" and arena_entries(w) == a0 + 1,
        "link back + fresh -> follows the server into its match (the stale mark is cleared)", T.repr({ w:conn(), w.opens }))
end

-- ---------------------------------------------------------------------------
T.log("== the same rejection twice in a row shows its modal twice")
do
    local w = DW.new()
    w:sidecar_status("connecting"); w:start(); w:tick(4)
    w:rejected("server full"); w:sidecar_status("rejected"); w:tick(2)
    T.check(w:conn().state == "lost" and w:conn().reason == "rejected" and w:conn().latched, "first rejection: modal", T.repr(w:conn()))
    w:request(1, "dismiss", nil, "modal"); w:tick(1)
    T.check(w:conn().state == "ok", "dismissed")
    w:sidecar_status(nil); w:tick(8)                 -- the menu tore the session down
    -- JOIN again; the server refuses at once (the Director never sees a non-terminal status)
    w:sidecar_status("rejected"); w:tick(4)
    local c = w:conn()
    T.check(c.state == "lost" and c.reason == "rejected" and c.latched == true, "the repeated rejection gets its modal again", T.repr(c))
    T.check(nlog(w, "connection LOST (rejected)") == 2, "reported twice")
end

-- ---------------------------------------------------------------------------
T.log("== a dead sidecar in the lobby phase does not rewrite single-player travels for 45 s")
do
    local w = DW.new()
    w:sidecar_status("connected"); w:match("lobby", "Map_Arena_Pit", 0); w:start(); w:tick(8)
    w.sidecar_frozen = true                          -- the sidecar dies; its file says "connected" forever
    w:tick(4 * 8)                                    -- > sidecar_stale_s (6 s), << session_stale_s
    T.check(w.dir:on_native_open_level("Map_Hub_Tavern_Frank") == nil,
        "a hung sidecar outside a match: the single-player travel is left alone")
end
