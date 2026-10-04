-- Director v1 offline tests (HSMPMatch/Scripts/director.lua).
--
--   hsmp-tools lua-test director
--
-- The Director is pure logic over an `env` table, so this suite drives it with
-- an in-memory world: files, clock, worlds/levels, pawn, GameInstance, census,
-- save guard. Nothing here touches the disk except loading the module.
--
-- Covered: startup (no stale request replay), single-player untouched, the
-- request/ack contract (accepted / refused / server arena wins / want=menu),
-- the full Menu -> Prepare -> Travel -> WaitWorld -> Spawn -> Ready -> Live
-- flow with every pipeline step, the round reload, the Tavern redirect (native
-- OpenLevel + soft-object travel), a world change mid-pipeline, a wrong arena
-- watchdog, GI write failure + retry, the game manager's GI reload after the
-- level load, no_pawn, kit/placement/census fallbacks and defects, lost link,
-- session end -> menu with GI restore, the save guard wiring, and the
-- typed session reader (the spawn order, heartbeat liveness).

-- The in-memory world (lib/dir_world.lua): the typed native mock behind a real
-- facade for the sidecar (link / session records, heartbeat), the bus keys and
-- the G2S records; real files for .gi_backup.txt, .settings.json and the kit status.
local DW = require("dir_world")
local D, SD, S = DW.D, DW.SD, DW.S
local new_world, to_ready = DW.new, DW.to_ready

-- ---------------------------------------------------------------------------
T.log("== startup, heartbeat, stale request")
do
    local w = new_world()
    w:request(5, "arena", "Map_Arena_Pit", "old run")
    w:start()
    w:tick(5)
    T.check(w:ack() == nil, "a request left over from an earlier run is never acked/replayed")
    local hb = w:director() or {}
    T.check(hb.state == "Menu" and type(hb.hb) == "number" and hb.hb > 0, "heartbeat bus director {state, hb}", T.repr(hb))
    T.check(#w.opens == 0, "no travel without a session")
    T.check(w.dir.state == "Menu", "state Menu")
end

T.log("== single player is never touched")
do
    local w = new_world()
    w:start()
    w:tick(3)
    w.world = { ok = true, short = "Map_Arena_Pit", key = "sp#1" }
    w:new_pawn()
    w:tick(3)
    T.check(w.dir:on_native_open_level("Map_Hub_Tavern_Frank") == nil, "no session: the Tavern travel is NOT rewritten")
    T.check(w.frozen == nil, "no input freeze in single player")
    T.check(#w.opens == 0, "no travel in single player")
    T.check(#w:pings() == 0, "no ping without a connected sidecar")
    w:request(1, "arena", "Map_Arena_Pit")
    w:tick(1)
    local a = w:ack()
    T.check(a and a.seq == 1 and a.status == "refused" and a.reason == "no session", "want=arena with no session is refused", T.repr(a))
end

T.log("== a 'connected' link whose sidecar stopped beating long ago does not count as a session")
do
    local w = new_world()
    w:sidecar_status("connected")
    w.sidecar_frozen, w.hb_at = true, w.clock - 60          -- no heartbeat for 60 s
    w:beat(); w.IPC.refresh_info(true)
    w:match("live", "Map_Arena_Pit", 1)
    w:start()
    w:tick(8)
    T.check(#w.opens == 0 and w.dir.state == "Menu" and not w.dir.sess.exists, "a dead sidecar's link never starts a travel")
end

T.log("== refusal while the server is in the lobby")
do
    local w = new_world()
    w:sidecar_status("connected")
    w:match("lobby", "Map_Arena_Pit", 0)
    w:start()
    w:tick(4)
    w:request(6, "arena", "Map_Arena_Pit", "match_countdown")
    w:tick(1)
    local a = w:ack()
    T.check(a and a.seq == 6 and a.status == "refused" and T.contains(a.reason, "no match running"),
        "want=arena refused when the server says no travel (lobby)", T.repr(a))
    T.check(#w.opens == 0, "no OpenLevel on a refused request")
    w:request(6, "arena", "Map_Arena_Pit")
    w.IPC.bus_clear("travel_ack")
    w:tick(2)
    T.check(w:ack() == nil, "the same seq is acked only once")
end

T.log("== full flow: Menu -> Prepare -> Travel -> WaitWorld -> Spawn -> Ready -> Live")
do
    local w = new_world({ sg = true })
    w:sidecar_status("connected")
    w:match("lobby", "Map_Arena_Pit", 0)
    w:start()
    w:tick(4)
    w:match("countdown", "Map_Arena_Pit", 0)
    w:spawns(1, "Map_Arena_Pit", 100, 200, 10)
    w:request(7, "arena", "Map_Arena_Alley", "match_countdown")   -- the menu asked for a different arena
    w:tick(1)
    local a = w:ack()
    T.check(a and a.seq == 7 and a.status == "accepted" and a.arena == "Map_Arena_Pit",
        "request accepted, ack names the SERVER's arena", T.repr(a))
    T.check(T.contains(a.reason, "server arena is Map_Arena_Pit"), "ack says why the arena differs", a.reason)
    T.check(#w.opens == 1 and w.opens[1] == "Map_Arena_Pit", "exactly one OpenLevel, to the server arena", T.repr(w.opens))
    T.check(w.dir.state == "WaitWorld", "state WaitWorld after OpenLevel", w.dir.state)
    T.check(w.gi["Player Just Died"] == false and w.gi["Free Mode Foes Amount"] == 1 and w.gi["Current Play Mode"] == 0
        and w.gi["Free Mode Carnage"] == false, "GI profile written (foes = 1 remote fighter from the roster)", T.repr(w.gi))
    T.check(w.files[SD .. "/.gi_backup.txt"] and T.contains(w.files[SD .. "/.gi_backup.txt"], "Rounds Won\t2")
        and not T.contains(w.files[SD .. "/.gi_backup.txt"], "Player Just Died"),
        "single-player GI backed up once (Player Just Died never restored)", w.files[SD .. "/.gi_backup.txt"])
    T.check(w.sg_seeds == 1 and w.seeded_gi and w.seeded_gi["Player Just Died"] == false,
        "session save slot seeded AFTER the verified GI apply, before OpenLevel")
    local tr = w:last_ev("travel")
    T.check(tr and tr.by == "director" and tr.to == "Map_Arena_Pit" and tr.from == "Map_Menu_Startup", "travel{from,to,by=director}", T.repr(tr))
    T.check(w.hook_result == nil, "the Director's own OpenLevel is not rewritten/counted by its pre-hook")

    -- the world swaps; the game manager reloads the GI from the (seeded) save
    w:travelling(); w:tick(2)
    w:load()
    w.gi["Free Mode Foes Amount"] = 4   -- a late game-manager write the Director must undo
    w:tick(1)
    local wr = w:last_ev("world_ready")
    T.check(wr and wr.arena == "Map_Arena_Pit" and wr.world_key == w.world.key and wr.round == 1,
        "world_ready{arena, world_key} for the new world, serving round 1", T.repr(wr))
    T.check(w.dir.state == "Spawn", "state Spawn", w.dir.state)
    T.check(w.gi["Free Mode Foes Amount"] == 1, "GI profile re-applied AFTER the arena's own load (read back)")
    T.check(w.frozen == w.pawn.id, "input frozen as soon as the pawn exists (countdown)")
    w:tick(1)
    T.check(w.game_input >= 1, "input mode game-only applied once the pawn is possessed")
    T.check(w.pawn.props["Neck Health"] == 100 and w.pawn.props.Health == 100 and w.pawn.props.Bleeding == 0,
        "career wounds healed: vitals copied from the Willie CDO", T.repr(w.pawn.props))
    T.check(T.contains(table.concat(w.pawn.calls, ","), "Reset Sustained Damage"), "BP reset functions called (real names with spaces)")
    -- placement by HSMPSync, then kit by HSMPLoadout
    w:tick(4)
    T.check(w.dir.state == "Spawn", "waits for HSMPSync placement (no .spawn_status.json yet)", w.dir.state)
    w.pawn.x, w.pawn.y, w.pawn.z = 100, 200, 98            -- standing on the order is not evidence
    w:tick(4)
    T.check(w.dir.state == "Spawn" and w.dir.pipe and D.PIPE[w.dir.pipe.step] == "place",
        "no 3 m inference: the pawn near its order without a status does not pass", w.dir.state)
    w:placed(1)
    w:tick(2)
    T.check(w.dir.state == "Spawn", "waits for the kit verified by HSMPLoadout")
    w.kit_status = { pawn = w.pawn.id, ok = false, armour_n = 3, l_class = "Shield", rev = 1, error = "verifying" }
    w:tick(2)
    T.check(w.dir.state == "Spawn", "a not-yet-verified kit holds the pipeline")
    w.kit_status = { pawn = w.pawn.id, ok = true, armour_n = 5, r_class = "Sword", l_class = "Shield", rev = 1, stable = false, t = w.clock }
    w:tick(4)                                        -- 1.0 s: inside the native re-arm window
    T.check(w.dir.state == "Spawn" and D.PIPE[w.dir.pipe.step] == "kit", "a kit verified < 1.5 s ago is not trusted yet")
    w:tick(3)
    T.check(D.PIPE[w.dir.pipe.step] ~= "kit", "the kit held for 1.5 s: kit step done")
    T.check(T.contains(table.concat(w.dir.pipe.notes, " "), "kit=ok(armour=5 r=Sword l=Shield)"), "kit=ok in the READY notes",
        table.concat(w.dir.pipe.notes, " "))
    w.census_n = 2                                   -- one ghost visible
    w:tick(4)
    T.check(w.dir.state == "Spawn", "an extra visible Willie holds the census step")
    w.census_n = 1                                   -- Avatars hid the extra
    w:tick(3)
    T.check(w.dir.state == "Ready", "state Ready", w.dir.state)
    local wc = w:last_ev("willie_census")
    T.check(wc and wc.visible == 2 and wc.expected == 2 and wc.extras == 0, "willie_census{visible=1+1, expected=1+1, extras=0}", T.repr(wc))
    local sv = w:last_ev("spawn_verified")
    T.check(sv and sv.ok == true and sv.round == 1 and sv.world_key == w.world.key and sv.spawn_id == 256
        and sv.dist_cm and sv.dist_cm < 1, "spawn_verified ok on the server order", T.repr(sv))
    local rr = w:last_ev("ready_report")
    T.check(rr and rr.round == 1, "ready_report{round=1}", T.repr(rr))
    w:tick(5)
    local pings = w:pings()
    T.check(T.any(pings, function(p) return p == "1:0:Map_Arena_Pit:0" end), "game_status reports the loaded round + arena", T.repr(pings))
    local gs = w:sent("game_status")
    gs = gs[#gs]
    T.check(gs and (gs.flags & S.ENUMS.status_flag.LOADED) ~= 0 and gs.spawn_id == 256 and gs.match_id == 0 and gs.world_key ~= 0,
        "game_status{flags LOADED, spawn_id of the applied order, world_key}", T.repr(gs))
    T.check(w.frozen == w.pawn.id, "still frozen in countdown")
    -- live
    w:match("live", "Map_Arena_Pit", 1)
    w:tick(1)
    T.check(w.dir.state == "Live" and w.frozen == false, "Live: input released", tostring(w.frozen))
    T.check(#w.opens == 1, "no further travel during the round")
    -- the server declares us dead: frozen again
    w:match("live", "Map_Arena_Pit", 1, nil, { dead = { 1 } })
    w:tick(1)
    T.check(w.frozen == w.pawn.id, "dead per server: input frozen while the round plays out")
end

T.log("== round reload, Tavern redirect, soft-object travel")
do
    local w = to_ready()
    T.check(w.dir.state == "Ready" and w.dir.ready_round == 1, "helper reached Ready round 1", w.dir.state)
    w:match("live", "Map_Arena_Pit", 1)
    w:tick(2)
    -- native death flow tries to go to the Tavern mid-round
    local dest = w.dir:on_native_open_level("Map_Hub_Tavern_Frank")
    T.check(dest == "Map_Arena_Pit", "native OpenLevel(Tavern) during live -> rewritten to the server arena", tostring(dest))
    local nr = w:last_ev("native_travel_rewritten")
    T.check(nr and nr.from == "Map_Hub_Tavern_Frank" and nr.to == "Map_Arena_Pit" and nr.n == 1, "native_travel_rewritten{from,to}", T.repr(nr))
    T.check(T.contains(w:logtext(), "native OpenLevel('Map_Hub_Tavern_Frank') rewritten -> 'Map_Arena_Pit'"), "and a log line for it")
    T.check(w.dir:on_native_open_level("Map_Arena_Pit") == nil, "travel to the right arena passes")
    local n0 = #w.opens
    T.check(w.dir:on_soft_travel_post("Map_Hub_Tavern_Frank") == "Map_Arena_Pit" and #w.opens == n0 + 1
        and w.opens[#w.opens] == "Map_Arena_Pit", "OpenLevelBySoftObjectPtr -> Director re-issues OpenLevel(arena)")
    w:load("Map_Arena_Pit")   -- that reroute reloads the arena mid-round
    w:tick(1)
    T.check(w.dir.state == "Spawn" and w.dir.loaded_for == 2, "a mid-round reload sits out and serves the next round", w.dir.loaded_for)
    w.kit_status = { pawn = w.pawn.id, ok = true, armour_n = 5 }
    w:spawns(2, "Map_Arena_Pit", 100, 200, 10)
    w:placed(2, { x = 101, y = 199 })
    w:tick(30)
    T.check(w.dir.state == "Live" or w.dir.state == "Ready", "back to Ready/Live", w.dir.state)
    -- roundover -> countdown for round 2: this world already serves round 2, no reload
    w:match("roundover", "Map_Arena_Pit", 1); w:tick(2)
    w:match("countdown", "Map_Arena_Pit", 1); w:tick(2)
    T.check(#w.opens == n0 + 1, "no reload for a round this world already serves", T.repr(w.opens))
    -- round 2 live, then countdown for round 3 -> exactly one reload
    w:match("live", "Map_Arena_Pit", 2); w:tick(2)
    w:match("roundover", "Map_Arena_Pit", 2); w:tick(2)
    w:match("countdown", "Map_Arena_Pit", 2); w:tick(1)
    T.check(#w.opens == n0 + 2 and w.opens[#w.opens] == "Map_Arena_Pit" and w.dir.state == "WaitWorld",
        "countdown for round 3 -> one fresh reload of the same arena", T.repr(w.opens))
    w:tick(10)
    T.check(#w.opens == n0 + 2, "the reload is issued once per round")
    w:load()
    w:tick(1)
    T.check(w.dir.loaded_for == 3 and w.dir.state == "Spawn", "new world serves round 3", w.dir.loaded_for)
end

T.log("== deathmatch respawn: the order reloads the arena and serves the running round")
do
    local w = DW.to_live()
    local n0 = #w.opens
    local GM = S.ENUMS.game_mode
    -- killed: the server has us dead; no reload without an order
    w:match("live", "Map_Arena_Pit", 1, nil, { dead = { 1 } })
    w.N.sc_put("mode", { seq = 2, mode = GM.DEATHMATCH, round = 1,
        rows = { { peer_id = 1, seat = 1, life = 1, respawning = true }, { peer_id = 2, seat = 2, life = 1, alive = true } } })
    w:tick(4)
    T.check(#w.opens == n0 and w.dir.state == "Live", "dead, respawn pending: no reload before the order", w.dir.state)
    -- the respawn order: the roster's spawn order becomes round 1 | 0x80 | life
    w.sess.plan[1] = { spawn_id = 256 + 0x82, slot = 4, x = 300, y = 400, z = 10, yaw = 0 }
    w:put_session()
    w.N.sc_put("mode", { seq = 3, mode = GM.DEATHMATCH, round = 1,
        rows = { { peer_id = 1, seat = 1, life = 2, respawning = true }, { peer_id = 2, seat = 2, life = 1, alive = true } } })
    w:tick(1)
    T.check(#w.opens == n0 + 1 and w.opens[#w.opens] == "Map_Arena_Pit", "one reload of the same arena", T.repr(w.opens))
    local ev = w:last_ev("respawn")
    T.check(ev and ev.round == 1 and ev.spawn_id == 256 + 0x82 and ev.life == 2, "respawn{round, spawn_id, life}", T.repr(ev))
    w:tick(4)
    T.check(#w.opens == n0 + 1, "the order is served once")
    w:load(); w:tick(1)
    T.check(w.dir.state == "Spawn" and w.dir.loaded_for == 1, "the new world serves the running round", w.dir.loaded_for)
    w:tick(2)
    w:placed(1, { x = 300, y = 400 })
    w.kit_status = { pawn = w.pawn.id, ok = true, armour_n = 5, r_class = "Sword", l_class = "Shield", rev = 1 }
    w:clear_pings()
    w:tick(30)
    T.check(w.dir.state == "Live" and w.dir.ready_round == 1, "Ready on the running round", w.dir.state)
    local gs = w:sent("game_status")
    local last = gs[#gs]
    T.check(last and last.round == 1 and last.spawn_id == 256 + 0x82 and (last.flags & S.ENUMS.status_flag.LOADED) ~= 0,
        "game_status reports the respawn order applied (the server revives on it)", T.repr(last))
    T.check(w.frozen == w.pawn.id, "input stays frozen until the server has us alive")
    -- revived: alive in the roster, no longer respawning
    w:match("live", "Map_Arena_Pit", 1)
    w.N.sc_put("mode", { seq = 4, mode = GM.DEATHMATCH, round = 1,
        rows = { { peer_id = 1, seat = 1, life = 2, alive = true }, { peer_id = 2, seat = 2, life = 1, alive = true } } })
    w:tick(2)
    T.check(w.frozen == false and #w.opens == n0 + 1, "input released; no further reload")
    -- the next round's countdown still reloads for round 2
    w:match("roundover", "Map_Arena_Pit", 1); w:tick(2)
    w:match("countdown", "Map_Arena_Pit", 1); w:tick(1)
    T.check(#w.opens == n0 + 2, "the next round reloads as usual", T.repr(w.opens))
    w:load(); w:tick(1)
    T.check(w.dir.loaded_for == 2, "and serves round 2", w.dir.loaded_for)
end

T.log("== world change mid-pipeline")
do
    local w = new_world()
    w:sidecar_status("connected")
    w:match("countdown", "Map_Arena_Pit", 0)
    w:start()
    w:tick(2)
    w:load()
    w:tick(3)
    local key1 = w.world.key
    T.check(w.dir.state == "Spawn" and w.dir.pipe and w.dir.pipe.key == key1, "pipeline running on world 1")
    -- the arena reloads under us (native restart) before Ready
    w:load("Map_Arena_Pit")
    w:tick(1)
    T.check(w.dir.pipe and w.dir.pipe.key == w.world.key and w.dir.pipe.key ~= key1, "pipeline restarted on the new world key")
    T.check(#w:evs_named("world_ready") == 2, "world_ready emitted for each new world")
    w.kit_status = { pawn = w.pawn.id, ok = true, armour_n = 5 }
    w.census_n = 1
    w:tick(120)
    local sv = w:last_ev("spawn_verified")
    T.check(w.dir.state == "Ready" and sv and sv.world_key == w.world.key, "Ready only for the CURRENT world", T.repr(sv))
    T.check(#w:evs_named("spawn_verified") == 1, "no Ready was reported for the dropped world")
    T.check(T.contains(sv.steps, "place=no_order"), "no spawn order: placement skipped after the wait", sv.steps)
    -- possession swap during the pipeline
    w:load("Map_Arena_Pit"); w:tick(3)
    local p_before = w.pawn.id
    w:new_pawn(); w:tick(1)
    T.check(T.contains(w:logtext(), "possessed pawn changed during the spawn pipeline"), "a pawn swap restarts at the pawn step")
    T.check(w.dir.pipe == nil or w.dir.pipe.pawn_id ~= p_before, "the old pawn is never used again")
    -- a wrong arena while the match runs -> watchdog travel (debounced)
    local n = #w.opens
    w.world = { ok = true, short = "Map_Arena_Alley", key = "alley#99" }
    w.clock = w.clock + 10
    w:tick(1)
    T.check(#w.opens == n + 1 and w.opens[#w.opens] == "Map_Arena_Pit" and T.contains(w:logtext(), "watchdog: in Map_Arena_Alley"),
        "in a foreign arena mid-match -> travel to the server arena", T.repr(w.opens))
    -- landed in the Tavern instead of the arena -> retry
    w:load("Map_Hub_Tavern_Frank")
    w:tick(1)
    T.check(w.opens[#w.opens] == "Map_Arena_Pit" and #w.opens == n + 2, "landed in the hub after our travel -> retried", T.repr(w.opens))
end

T.log("== GI verify failure, no_pawn, fallbacks and defects")
do
    local w = new_world()
    w:sidecar_status("connected")
    w:match("countdown", "Map_Arena_Pit", 0)
    w.gi_stuck["Current Play Mode"] = true
    w:start()
    w:tick(4)
    w:tick(3)
    T.check(#w.opens == 0 and w.dir.load_error == "gi_verify", "GI writes that do not stick: no travel, load_error=gi_verify", tostring(w.dir.load_error))
    T.check(T.any(w:pings(), function(p) return p == "0:0::" .. S.ENUMS.load_error.OTHER end), "the game status carries the load error", T.repr(w:pings()))
    w.gi_stuck = {}
    w:tick(24)
    T.check(#w.opens == 1, "retried after gi_retry_s and travelled", T.repr(w.opens))
    w:load(nil, { no_pawn = true })
    w:tick(66)
    T.check(w.dir.load_error == "no_pawn" and w.dir.state == "Spawn", "no pawn after 15 s -> load_error=no_pawn", tostring(w.dir.load_error))
    w:new_pawn()
    w:spawns(1, "Map_Arena_Pit", 100, 200, 10)
    w:placed(1, { x = 150, y = 260, tries = 2 })        -- HSMPSync placed + verified us
    w.census_n = 3                                       -- two ghosts that never go away
    w:tick(80)
    local sv = w:last_ev("spawn_verified")
    T.check(w.dir.state == "Ready" and sv, "pipeline completes despite the defects", w.dir.state)
    T.check(T.contains(sv.steps, "place=hsmpsync(id=256 tries=2"), "placement from HSMPSync's verified status", sv.steps)
    T.check(T.contains(sv.steps, "kit=unavailable"), "no kit_status: kit check reported unavailable", sv.steps)
    local wc = w:last_ev("willie_census")
    T.check(wc and wc.extras == 2 and T.contains(sv.steps, "census=DEFECT"), "census defect reported (extras=2)", T.repr(wc))
    T.check(sv.ok == false and sv.load_error == "no_pawn", "spawn_verified ok=false keeps the first load error", T.repr(sv))
end

T.log("== spawn order not reached -> spawn_timeout; kit never verified -> kit_error")
do
    local w = new_world()
    w:sidecar_status("connected")
    w:match("countdown", "Map_Arena_Pit", 0)
    w:spawns(1, "Map_Arena_Pit", 100, 200, 10)
    w:start(); w:tick(2); w:load()
    w.kit_status = { pawn = "someone_else", ok = true }
    w.census_n = 1
    w:tick(200)
    local sv = w:last_ev("spawn_verified")
    T.check(sv and T.contains(sv.steps, "place=TIMEOUT") and T.contains(sv.steps, "kit=FAILED"),
        "timeouts are reported per step", sv and sv.steps)
    T.check(sv and sv.load_error == "spawn_timeout", "first error wins: spawn_timeout", sv and sv.load_error)
    T.check(w.dir.state == "Ready", "and the round can still start", w.dir.state)
    T.check(T.contains(w:logtext(), "asking HSMPSync to re-place (retry 1)") and w:spawn_request(),
        "the Director retried placement once before reporting the error")
end

T.log("== placement handshake: verified status only, snap-back guard, one retry, early retry on failure")
do
    local function to_place()
        local w = new_world()
        w:sidecar_status("connected")
        w:match("countdown", "Map_Arena_Pit", 0)
        w:spawns(1, "Map_Arena_Pit", 100, 200, 10)
        w:start(); w:tick(2); w:load()
        w.kit_status = { pawn = "Willie_BP_C_11", ok = true, armour_n = 5 }
        w.census_n = 1
        w:tick(3)
        T.check(w.dir.pipe and D.PIPE[w.dir.pipe.step] == "place", "pipeline waits at the place step")
        return w
    end
    -- placed but not verified yet
    local w = to_place()
    w:placed(1, { verified = false })
    w:tick(8)
    T.check(D.PIPE[w.dir.pipe.step] == "place", "verified=false holds the pipeline")
    -- status for another pawn / another round never counts
    w:placed(1, { pawn = "Willie_BP_C_99" })
    w:tick(4)
    T.check(D.PIPE[w.dir.pipe.step] == "place", "a status for another pawn is ignored")
    w:placed(2)
    w:tick(4)
    T.check(D.PIPE[w.dir.pipe.step] == "place", "a status for another round is ignored")
    -- verified, but the body snapped back afterwards (the Slums bug)
    w:placed(1, { stay = true })
    w.pawn.x, w.pawn.y = 257, 415
    w:tick(4)
    T.check(D.PIPE[w.dir.pipe.step] == "place", "verified status but the pawn is 3 m+ away now: not placed")
    -- the retry after place_retry_s
    w:tick(16)
    local rq = w:spawn_request()
    T.check(rq and rq.seq == 1 and rq.round == 1 and rq.arena == "Map_Arena_Pit" and rq.pawn == w.pawn.id and rq.spawn_id == 256,
        "bus spawn_request {seq, round, arena, pawn, spawn_id}", T.repr(rq))
    T.check(T.contains(rq and rq.why or "", "verified, but the pawn is"), "the request says why (truncated to the field)", rq and rq.why)
    w:tick(8)
    T.check(w:spawn_request().seq == 1, "exactly one retry per world load")
    -- HSMPSync re-places after the request: the pipeline continues
    w:placed(1, { why = "director retry", seq = 3 })
    w:tick(4)
    local sv = w:last_ev("spawn_verified")
    T.check(w.dir.state == "Ready" and sv and sv.ok and T.contains(sv.steps, "after retry"),
        "a successful retry clears the step without an error", sv and sv.steps)

    -- HSMPSync already reported a failure -> the Director retries at once
    local w2 = to_place()
    w2:placed(1, { verified = false, error = "did not stick after 3 tries", stay = true })
    w2:tick(6)
    T.check(w2:spawn_request() ~= nil, "a reported failure triggers the retry early (not after 8 s)")
    T.check(T.contains(w2:logtext(), "HSMPSync: did not stick"), "with HSMPSync's error as the reason")
    w2:tick(90)
    local sv2 = w2:last_ev("spawn_verified")
    T.check(sv2 and sv2.load_error == "spawn_timeout" and T.contains(w2:logtext(), "after 20 s (HSMPSync: did not stick"),
        "still failing -> spawn_timeout with the reason", sv2 and sv2.load_error)
end

T.log("== spawn protection: no dead ping inside the window, input frozen until the pipeline is done")
do
    local w = to_ready()
    T.check(w.dir.state == "Ready", "Ready")
    w:match("live", "Map_Arena_Pit", 1)
    w:tick(1)
    w:placed(1, { protect_until = w.clock + 5 })
    w.pawn.props.Health = 0
    w:clear_pings()
    w:tick(8)
    T.check(T.all(w:pings(), function(p) return p:match("^1:0:") ~= nil end) and #w:pings() > 0,
        "Health 0 inside the protection window -> ping dead=0", T.repr(w:pings()))
    T.check(T.contains(w:logtext(), "inside the spawn protection window"), "logged once")
    w:tick(16)
    T.check(T.any(w:pings(), function(p) return p:match("^1:1:") ~= nil end), "after the window: dead=1 again", T.repr(w:pings()))

    -- Live arrives while the pipeline still waits for placement: input stays frozen
    local w2 = new_world()
    w2:sidecar_status("connected")
    w2:match("countdown", "Map_Arena_Pit", 0)
    w2:spawns(1, "Map_Arena_Pit", 100, 200, 10)
    w2:start(); w2:tick(2); w2:load(); w2:tick(3)
    w2:match("live", "Map_Arena_Pit", 1)
    w2:tick(4)
    T.check(w2.dir.state == "Spawn" and w2.frozen == w2.pawn.id, "phase live but not placed yet: still frozen", tostring(w2.frozen))
    w2:placed(1)
    w2.kit_status = { pawn = w2.pawn.id, ok = true, armour_n = 5 }
    w2.census_n = 1
    w2:tick(4)
    T.check(w2.dir.state == "Live" and w2.frozen == false, "placed -> Live, input released", w2.dir.state)
end

T.log("== match over -> lobby -> menu; GI restore; return flag")
do
    local w = to_ready({ sg = true })
    w.sg_is_active = false          -- pretend the guard's own liveness lapsed
    w:match("match_over", "Map_Arena_Pit", 3); w:tick(4)
    T.check(w.dir.state ~= "TravelMenu" and #w.opens == 1, "match_over: stay for the result screen")
    w:match("lobby", "Map_Arena_Pit", 0); w:tick(12)
    T.check(w.opens[#w.opens] == "Map_Menu_Startup" and w.dir.state == "TravelMenu", "lobby after a match -> travel to the menu", T.repr(w.opens))
    T.check(w:rtl() == 1, "return_to_lobby seq bumped for HSMPMenu", w:rtl())
    T.check(w.gi["Rounds Won"] == 2 and w.gi["Free Mode Foes Amount"] == 4 and w.files[SD .. "/.gi_backup.txt"] == nil,
        "single-player GI restored from the backup", T.repr(w.gi))
    w:load("Map_Menu_Startup"); w:tick(2)
    T.check(w.dir.state == "Menu", "state Menu in the menu world", w.dir.state)
    T.check(w.frozen == false or w.frozen == nil, "nothing frozen in the menu")
end

T.log("== save guard wiring")
do
    local w = new_world({ sg = true })
    w.sg_is_active = false
    w:sidecar_status("connected")
    w:match("countdown", "Map_Arena_Pit", 0)
    w:start(); w:tick(3)
    T.check(w.sg_forced == true and T.contains(w:logtext(), "forced ON"), "guard inactive at Prepare -> forced on by the Director")
    w:load(); w:tick(1)
    w:match("lobby", "Map_Arena_Pit", 0); w:tick(12)
    T.check(w.dir.state == "TravelMenu" and w.sg_forced == true, "still forced at TravelMenu while the arena is loaded")
    w:load("Map_Menu_Startup"); w:tick(1)
    T.check(w.sg_forced == nil, "handed back to automatic once the menu world arrived")
    w:tick(4)
end

T.log("== kicked in the arena -> the save guard stays ON until the menu world arrives")
do
    local w = to_ready({ sg = true })           -- the guard's own liveness says active
    T.check(w.sg_forced == true, "an active guard is held ON by the Director from Prepare on", tostring(w.sg_forced))
    w:match("live", "Map_Arena_Pit", 1); w:tick(2)
    w:kicked("afk"); w:sidecar_status("kicked"); w:tick(1)
    T.check(w.opens[#w.opens] == "Map_Menu_Startup", "kicked -> travel to the menu", T.repr(w.opens))
    T.check(w.sg_forced == true, "the arena is still loaded: the guard is still forced ON", tostring(w.sg_forced))
    w:tick(4)
    T.check(w.sg_forced == true, "still ON while the menu load has not arrived", tostring(w.sg_forced))
    w:load("Map_Menu_Startup"); w:tick(1)
    T.check(w.sg_forced == nil and T.contains(w:logtext(), "save guard back to automatic"),
        "menu world arrived -> back to automatic", tostring(w.sg_forced))
end

T.log("== lost link / session gone mid-match")
do
    local w = to_ready()
    w:match("live", "Map_Arena_Pit", 1); w:tick(2)
    w:sidecar_status("reconnecting")
    w:tick(40)                       -- 10 s: ride it out
    T.check(w.dir.state == "Live" or w.dir.state == "Ready", "short outage ridden out", w.dir.state)
    T.check(#w.opens == 1, "no travel while reconnecting", T.repr(w.opens))
    w:tick(108)                      -- > 35 s (the resume window; flow.lua covers every path)
    T.check(w.opens[#w.opens] == "Map_Menu_Startup" and T.contains(w:logtext(), "connection LOST"),
        "link lost > resume window -> menu, once", T.repr(w.opens))
    T.check(w.dir:on_native_open_level("Map_Arena_Pit") == "Map_Menu_Startup", "while going to the menu, a native arena load is rewritten to the menu")

    local w2 = to_ready()
    w2:match("live", "Map_Arena_Pit", 1); w2:tick(2)
    w2:sidecar_status(nil)           -- the menu's CANCEL stopped the sidecar (detached)
    w2:tick(20)                      -- 1 s absence debounce + session_gone_s
    T.check(w2.opens[#w2.opens] == "Map_Menu_Startup" and T.contains(w2:logtext(), "session ended"),
        "session process gone -> menu", T.repr(w2.opens))
    T.check(w2:rtl() == 0, "no lobby hand-back when the session is gone")
end

T.log("== want=menu requests")
do
    local w = to_ready()
    w:request(20, "menu", nil, "autotest_splash_skip"); w:tick(1)
    local a = w:ack()
    T.check(a and a.seq == 20 and a.status == "refused" and T.contains(a.reason, "match in progress"), "want=menu refused mid-match", T.repr(a))
    local w2 = new_world()
    w2.world = { ok = true, short = "Map_Menu_SplashScreens", key = "splash#1" }
    w2:start(); w2:tick(1)
    w2:request(1, "menu", nil, "autotest_splash_skip"); w2:tick(1)
    a = w2:ack()
    T.check(a and a.status == "accepted" and a.arena == "Map_Menu_Startup" and w2.opens[1] == "Map_Menu_Startup",
        "want=menu from the splash (no session) -> accepted, OpenLevel(menu)", T.repr(a))
    T.check(w2:rtl() == 0, "no return flag without a session")
end

T.log("== leftover GI backup restored in the menu with no session")
do
    local w = new_world()
    w.files[SD .. "/.gi_backup.txt"] = "Rounds Won\t7\nFree Mode Carnage\ttrue\n"
    w:start(); w:tick(30)
    T.check(w.gi["Rounds Won"] == 7 and w.files[SD .. "/.gi_backup.txt"] == nil, "crash-left backup restored after 5 s in the menu")
end

T.log("== typed session reader: phases, frozen arena, roster roles, spawn order (bug 1)")
do
    local w = new_world()
    w:sidecar_status("connected")
    w:start()
    w:tick(2)
    -- the server's snapshot: LOADING (the barrier) in a match on the frozen arena
    w.N.sc_put("session", { epoch = 77, seq = 1, match_id = 42, round = 0, phase = S.ENUMS.phase.LOADING, winner_seat = 255,
        has_frozen = true, config = { arena = "Map_Arena_Yard", best_of = 3, countdown_s = 3 },
        frozen = { arena = "Map_Arena_Cellar", best_of = 5, countdown_s = 3 },
        rows = { { seat = 1, peer_id = 1, connected = true, alive = true, nick = "Me", role = 0, spawn_id = 256 + 1,
                   spawn_slot = 4, spawn_pos = { 1, 2, 3 }, spawn_yaw = 45 },
                 { seat = 2, peer_id = 2, connected = true, alive = true, nick = "B", role = 0, waiting = true },
                 { seat = 3, peer_id = 3, connected = true, alive = true, nick = "C", role = 1 } } })
    w:tick(1)
    local s = w.dir.sess
    T.check(s.source == "session" and s.phase == "countdown" and s.arena == "Map_Arena_Cellar" and s.match_id == 42
        and s.epoch == 77 and s.best_of == 5, "LOADING counts as countdown; arena = the frozen one in a match",
        T.repr({ s.phase, s.arena, s.match_id, s.epoch }))
    T.check(s.remote_fighters == 1 and #s.roster == 3 and s.roster[3].role == "spectator",
        "foe count = remote FIGHTERS only (spectators excluded)", s.remote_fighters)
    T.check(s.nicks[2] == "B" and s.waiting[1] == 2 and s.is_admin == true and s.my_id == 1, "nicks, waiting, admin, my id")
    -- the spawn order comes from the session record
    local o = s.spawn_order(1, "Map_Arena_Cellar")
    T.check(o and o.spawn_id == 257 and o.slot == 4 and o.x == 1 and o.z == 3 and o.yaw == 45,
        "bug 1: spawn order from roster[me]", T.repr(o))
    T.check(s.spawn_order(2, "Map_Arena_Cellar") == nil and s.spawn_order(1, "Map_Arena_Pit") == nil,
        "an order for another round / arena is not ours")
    T.check(w.opens[1] == "Map_Arena_Cellar", "travel follows the frozen arena", T.repr(w.opens))
    w.N.sc_put("session", { epoch = 77, seq = 2, match_id = 0, phase = S.ENUMS.phase.POST_MATCH, winner_seat = 255,
        config = { arena = "Map_Arena_Yard" }, rows = {} })
    w:tick(1)
    T.check(w.dir.sess.phase == "lobby" and w.dir.sess.match_id == 0, "POST_MATCH maps to lobby")
    -- the Director's records: a command is typed, never a JSON line
    w.dir:send_cmd("READY", true)
    local c = w:sent("command")
    c = c[#c]
    T.check(c and c.op == S.ENUMS.cmd_op.READY and c.flag == true and c.cmd_id ~= 0, "command{op READY, flag, cmd_id}", T.repr(c))
end
T.log("== pure helpers")
do
    T.check(D.valid_arena("Map_Arena_Pit") and not D.valid_arena("Map_Hub_Tavern_Frank")
        and not D.valid_arena("Map_Menu_Startup") and not D.valid_arena('x"y'), "valid_arena")
    T.check(D.jstr('a"b\\c') == '"a\\"b\\\\c"' and D.jstr(nil) == "null", "jstr escapes")
    T.check(D.step_index("pawn") == 3 and D.PIPE[#D.PIPE] == "ready", "pipeline order world,gi,pawn,...,ready")
end

T.log("== the server changes its mind while we travel")
do
    local w = new_world()
    w:sidecar_status("connected")
    w:match("countdown", "Map_Arena_Pit", 0)
    w:start(); w:tick(3)
    T.check(w.dir.state == "WaitWorld" and w.opens[1] == "Map_Arena_Pit", "travelling to Pit")
    w:match("lobby", "Map_Arena_Pit", 0)          -- host ABORT before the load finished
    w:tick(1)
    T.check(w.opens[#w.opens] == "Map_Menu_Startup" and T.contains(w:logtext(), "match aborted while travelling"),
        "abort while travelling -> menu (last travel request wins)", T.repr(w.opens))
    local w2 = new_world()
    w2:sidecar_status("connected")
    w2:match("countdown", "Map_Arena_Pit", 0)
    w2.gi_stuck["Rounds Won"] = true
    w2:start(); w2:tick(3)
    T.check(#w2.opens == 0 and w2.dir.state == "Prepare", "stuck in Prepare (GI)")
    w2.gi_stuck = {}
    w2:match("countdown", "Map_Arena_Yard", 0)    -- RCON MAP + restart
    w2:tick(1)
    T.check(w2.opens[1] == "Map_Arena_Yard" and w2.dir.target == "Map_Arena_Yard", "follows the new server arena", T.repr(w2.opens))
end

-- ---------------------------------------------------------------------------
-- Round travel, typed reports, session-loss handling

-- Drive w (in Ready on round `r` in Pit) through live/roundover and the
-- countdown for round r+1; returns the number of OpenLevel calls it caused.
local function next_round(w, r)
    local n0 = #w.opens
    w:match("live", "Map_Arena_Pit", r); w:tick(2)
    w:match("roundover", "Map_Arena_Pit", r); w:tick(2)
    w:match("countdown", "Map_Arena_Pit", r); w:tick(1)
    local n = #w.opens - n0
    if n > 0 then
        w:load(); w:tick(1)
        w:spawns(r + 1, "Map_Arena_Pit", 100, 200, 10)
        w:placed(r + 1)
        w.kit_status = { pawn = w.pawn.id, ok = true, armour_n = 5 }
        w:tick(30)
    end
    return n
end

T.log("== H2: match 2 round 2 still reloads (round keys carry the match)")
do
    local w = to_ready()
    T.check(next_round(w, 1) == 1 and w.dir.loaded_for == 2, "match 1: round 2 reload", w.dir.loaded_for)
    w:match("match_over", "Map_Arena_Pit", 2); w:tick(4)
    w:match("lobby", "Map_Arena_Pit", 0); w:tick(12)
    w:load(); w:tick(2)
    T.check(w.dir.state == "Menu", "back in the menu after match 1", w.dir.state)
    local gen1 = w.dir.match_gen
    T.check(gen1 >= 1, "the lobby after a match moves the match context", gen1)
    -- match 2: rounds restart at 1
    w:match("countdown", "Map_Arena_Pit", 0); w:tick(2)
    w:load(); w:tick(1)
    w:spawns(1, "Map_Arena_Pit", 100, 200, 10); w:placed(1)
    w.kit_status = { pawn = w.pawn.id, ok = true, armour_n = 5 }
    w:tick(30)
    T.check(w.dir.loaded_for == 1 and (w.dir.state == "Ready" or w.dir.state == "Live"), "match 2 round 1 ready", w.dir.state)
    T.check(next_round(w, 1) == 1, "match 2: the countdown for round 2 reloads (was skipped: reloaded_for == 2 from match 1)")
    T.check(w.dir.loaded_for == 2, "match 2 round 2 world", w.dir.loaded_for)
end

T.log("== H2: a new match_id / epoch moves the match context; nil -> known does not")
do
    local w = to_ready()
    local g0 = w.dir.match_gen
    w:session(77, 5)
    w:tick(2)
    T.check(w.dir.sess.match_id == 5 and w.dir.match_gen == g0, "first match_id seen: no new context", w.dir.match_gen)
    w:session(77, 6)
    w:tick(2)
    T.check(w.dir.match_gen == g0 + 1, "match_id 5 -> 6: new context", w.dir.match_gen)
    w:session(78, 6)
    w:tick(2)
    T.check(w.dir.match_gen == g0 + 2 and w.dir.sess.epoch == 78, "epoch 77 -> 78: new context", w.dir.match_gen)
    w:tick(2)
    T.check(w.dir.match_gen == g0 + 2, "an unchanged snapshot: no new context", w.dir.match_gen)
end

T.log("== H7: a session snapshot left over from an earlier session never sends us to the arena")
do
    local w = new_world()
    w:match("live", "Map_Arena_Pit", 1)          -- crash mid-match last time
    w:start(nil, true)                            -- game boots with the leftover
    w:tick(4)
    w:sidecar_status("connected")
    w:tick(12)
    T.check(#w.opens == 0 and w.dir.state == "Menu", "leftover live state: no travel", T.repr(w.opens))
    T.check(w.dir.sess.match_stale == true, "reader marks it stale")
    w:match("countdown", "Map_Arena_Pit", 0)     -- the server's first real state
    w:tick(4)
    T.check(w.opens[1] == "Map_Arena_Pit", "the first fresh state is followed", T.repr(w.opens))
    -- session ends, the next session starts with the old file still there
    local w2 = new_world()
    w2:sidecar_status("connected"); w2:match("lobby", "Map_Arena_Pit", 0); w2:start(); w2:tick(4)
    w2:match("live", "Map_Arena_Yard", 2); w2:tick(1)
    w2:sidecar_status(nil); w2:tick(8)            -- session gone (sidecar detached)
    local n0 = #w2.opens
    w2:sidecar_status("connected"); w2:tick(12)   -- HOST/JOIN again; the slot still holds the old live snapshot
    local yard = 0
    for i = n0 + 1, #w2.opens do if w2.opens[i] == "Map_Arena_Yard" then yard = yard + 1 end end
    T.check(yard == 0, "previous session's live state is not replayed", T.repr(w2.opens))
end

T.log("== C2: one look at a detached sidecar is no evidence")
do
    local w = to_ready()
    w:match("live", "Map_Arena_Pit", 1); w:tick(4)
    T.check(w.dir.state == "Live" and w.dir.sess.connected, "live")
    local evs0 = #w:evs_named("conn_state")
    w.N._st.sidecar_state = "absent"; w.IPC.refresh_info(true)
    w.clock = w.clock + 0.25; w.dir:tick()
    T.check(w.dir.sess.connected and w.dir.sess.exists, "still connected after one detached look")
    w.N._st.sidecar_state = "ready"; w.IPC.refresh_info(true)
    w:tick(2)
    T.check(#w:evs_named("conn_state") == evs0 and w.dir.conn.state == "ok", "no reconnecting blip", w.dir.conn.state)
    T.check(D.complete('{"a":1}\n') and not D.complete('{"a":1') and not D.complete("") and not D.complete(nil), "D.complete")
end

T.log("== M2 / M19 / DoD-7 / H15")
do
    local w = new_world()
    w.IPC.bus_put("spawn_request", { seq = 41, round = 3, arena = "Map_Arena_Pit", pawn = "x", spawn_id = 1, why = "old" })
    w.IPC.bus_put("return_to_lobby", { seq = 9 })
    w:start()
    T.check(w.dir.place_req_seq == 41, "place_req_seq continues from the leftover spawn_request", w.dir.place_req_seq)
    T.check(w.dir.rtl_seq == 9, "return_to_lobby seq continues from the bus key", w.dir.rtl_seq)
    local w2 = to_ready()
    local sv = w2:last_ev("spawn_verified")
    T.check(sv and type(sv.dist_cm) == "number" and sv.dist_cm < 10 and sv.z == 98, "spawn_verified.dist_cm / z", T.repr(sv))
    T.check(sv and sv.snap_z == 98, "spawn_verified.snap_z = HSMPSync's placed Z (p.placed[3])", T.repr(sv))
    T.check(sv and type(sv.dest_cm) == "number" and sv.dest_cm < 10 and sv.tol_cm == 100 and type(sv.offset_cm) == "number",
        "spawn_verified.dest_cm / offset_cm / tol_cm (DoD-7 judged against the destination)", T.repr(sv))
    for _, k in ipairs({ "dist_m", "z_ok", "min_peer_m", "health_ok" }) do
        T.check(sv and sv[k] == nil, "no field outside the gate contract: " .. k)
    end
    -- DoD-6: willie_census{at="live"} every 5 s during Live
    w2.census_n = 1
    w2:match("live", "Map_Arena_Pit", 1); w2:tick(4)
    local before = #w2:evs_named("willie_census")
    w2:tick(4 * 11)   -- 11 s
    local live = T.filter(w2:evs_named("willie_census"), function(f) return f.at == "live" end)
    T.check(#live == 3 and live[1].visible == 2 and live[1].expected == 2 and live[1].round == 1,
        "willie_census{at=live} on the first Live tick, then every 5 s", T.repr(live))
    T.check(#w2:evs_named("willie_census") == before + 2, "nothing else added (the first Live sample came before)")
    w2:match("live", "Map_Arena_Pit", 1); w2:tick(2)
    w2.dir:on_native_open_level("Map_Hub_Tavern_Frank")
    local tr = w2:last_ev("travel")
    T.check(tr and tr.by == "native_rewrite" and tr.to == "Map_Arena_Pit", "travel{by=native_rewrite} for the in-place rewrite", T.repr(tr))
    -- Typed: every report is a game_status record; commands carry their own cmd_id
    local gs = w2:sent("game_status")
    T.check(#gs > 0 and gs[#gs].round == 1 and gs[#gs].arena == "Map_Arena_Pit" and T.all(gs, function(g) return g.match_id == 0 end),
        "the game reports are typed game_status records", T.repr(gs[#gs]))
    w2.dir:send_cmd("READY", true)
    w2.dir:send_cmd("START", false)
    local cmds = w2:sent("command")
    T.check(#cmds == 2 and cmds[1].cmd_id ~= cmds[2].cmd_id and cmds[2].op == S.ENUMS.cmd_op.START,
        "every command has its own cmd_id", T.repr(cmds))
end

T.log("== spawn protection: null = protected until Live; another pawn's status protects nothing")
do
    local w = to_ready()
    w:placed(1, { verified = true, unbounded = true })
    T.check(w.dir.state ~= "Live" and w.dir:protected_now(w.pawn.id) == true, "verified + no protect_until before Live -> protected")
    T.check(w.dir:protected_now("Willie_BP_C_999") == false, "status for another pawn -> not protected")
    w.IPC.bus_clear("spawn_status")
    T.check(w.dir:protected_now(w.pawn.id) == false, "a cleared status (seq 0) protects nothing")
end

-- ---------------------------------------------------------------------------
-- Sidecar liveness, late match ids, kit status, world settle, RVP-gated travel

T.log("== one look at a detached sidecar in Ready (countdown) is no session end")
do
    -- Seen in a gate run: Ready for round 5 in round 4's countdown;
    -- one empty status read -> exists=false -> phase none -> "new match
    -- context #1 (phase none)" -> the round key moved -> "Ready -> Prepare
    -- (round 5 starting)": a second arena load 0.4 s before Live.
    local w = to_ready()
    T.check(w.dir.state == "Ready" and w.dir.sess.phase == "countdown", "Ready in the countdown", w.dir.state)
    local n0, g0, key0 = #w.opens, w.dir.match_gen, w.dir.loaded_key
    for _ = 1, 2 do
        w.N._st.sidecar_state = "absent"; w.IPC.refresh_info(true)
        w.clock = w.clock + 0.25; w.dir:tick()             -- one look without the sidecar
        T.check(w.dir.sess.exists and w.dir.sess.phase == "countdown", "one detached look: the session still exists, phase kept",
            T.repr({ w.dir.sess.exists, w.dir.sess.phase }))
        w.N._st.sidecar_state = "ready"; w.IPC.refresh_info(true)
        w:tick(4)                                          -- the sidecar is back, still the countdown
    end
    T.check(#w.opens == n0 and w.dir.state == "Ready", "no second arena load in the countdown", T.repr({ w.opens, w.dir.state }))
    T.check(w.dir.match_gen == g0 and w.dir.loaded_key == key0 and not T.contains(w:logtext(), "new match context"),
        "no new match context", w.dir.match_gen)
    -- a real absence (>= 2 reads over >= 1 s) still ends the session
    w:sidecar_status(nil)
    w:tick(3)
    T.check(w.dir.sess.exists, "absent for 0.75 s: not confirmed yet")
    w:tick(2)
    T.check(not w.dir.sess.exists, "absent for >= 1 s: the session process is gone")
end

T.log("== bug 4: liveness is the sidecar's heartbeat, not 'the content changed recently'")
do
    local w = new_world()
    w:sidecar_status("connected"); w:match("lobby", "Map_Arena_Pit", 0); w:start(); w:tick(4)
    T.check(w.dir.sess.live and w.dir.sess.connected, "a beating sidecar: live + connected (the link record never changed)")
    w:tick(4 * 20)                                         -- 20 s, no record change at all
    T.check(w.dir.sess.live and w.dir.sess.connected and w.dir.sess.exists, "an unchanged link with a fresh heartbeat stays live")
    w:sidecar_status("reconnecting"); w:tick(1)
    T.check(w.dir.sess.live and not w.dir.sess.connected, "reconnecting counts as live, not connected")
    w:sidecar_status("connecting"); w:tick(1)
    T.check(not w.dir.sess.live and w.dir.sess.exists, "connecting is not live")
    w:sidecar_status("connected"); w:tick(1)
    w.sidecar_frozen = true                                -- the sidecar hangs: the heartbeat stops
    w:tick(4 * 7)
    T.check(w.dir.sess.sidecar_hung and not w.dir.sess.connected and w.dir.sess.exists, "no heartbeat > 6 s: hung, not connected")
    w:tick(4 * 9)
    T.check(not w.dir.sess.live, "no heartbeat > 15 s: not live, whatever the status")
    w:tick(4 * 31)
    T.check(not w.dir.sess.exists, "no heartbeat > 45 s: no session process")
end

T.log("== a match_id that arrives late in match 2's countdown does not reload the arena")
do
    local w = to_ready()
    w:session(77, 5)
    w:tick(2)
    T.check(next_round(w, 1) == 1, "match 1 round 2")
    w:match("match_over", "Map_Arena_Pit", 2); w:tick(4)
    w:match("lobby", "Map_Arena_Pit", 0); w:tick(12)
    w:load(); w:tick(2)
    T.check(w.dir.state == "Menu", "back in the menu", w.dir.state)
    local g1 = w.dir.match_gen
    -- match 2 starts; the snapshots still name match 5
    w:match("countdown", "Map_Arena_Pit", 0); w:tick(2)
    w:load(); w:tick(1)
    w:spawns(1, "Map_Arena_Pit", 100, 200, 10); w:placed(1)
    w.kit_status = { pawn = w.pawn.id, ok = true, armour_n = 5 }
    w:tick(30)
    T.check(w.dir.state == "Ready" and w.dir.loaded_for == 1, "match 2 round 1 Ready in the countdown", w.dir.state)
    local n0 = #w.opens
    w:session(77, 6)   -- lands now
    w:tick(4)
    T.check(#w.opens == n0 and w.dir.state == "Ready", "the late match_id does not re-prepare the arena",
        T.repr({ w.opens, w.dir.state }))
    T.check(w.dir.match_gen == g1, "no second match context for the same match", w.dir.match_gen)
    T.check(next_round(w, 1) == 1 and w.dir.loaded_for == 2, "match 2 round 2 still reloads once (H2)", w.dir.loaded_for)
    -- a match_id change with no lobby in between is still a new match
    w:session(77, 7)
    w:tick(2)
    T.check(w.dir.match_gen == g1 + 1, "match_id 6 -> 7 after a played round: new context", w.dir.match_gen)
end

T.log("== an old world's kit_status for a re-used pooled FName is not this world's kit")
do
    local w = new_world()
    w:sidecar_status("connected"); w:match("lobby", "Map_Arena_Pit", 0); w:start(); w:tick(4)
    w:match("countdown", "Map_Arena_Pit", 0); w:spawns(1, "Map_Arena_Pit", 100, 200, 10); w:tick(1)
    w:load(); w:tick(2); w:placed(1)
    -- written in the previous world for the same pooled name (the writer dedups unchanged content)
    w.kit_status = { pawn = w.pawn.id, ok = true, armour_n = 5, stable = true, t = w.clock - 12 }
    w.census_n = 1
    w:tick(8)
    T.check(w.dir.state == "Spawn" and w.dir.pipe and D.PIPE[w.dir.pipe.step] == "kit",
        "an old world's status (t before this pipeline) is no kit evidence", T.repr({ w.dir.state, w.dir.pipe and w.dir.pipe.step }))
    w.kit_status = { pawn = w.pawn.id, ok = true, armour_n = 5, stable = true, t = w.clock }
    w:tick(8)
    T.check(w.dir.state == "Ready", "this world's status for the pawn -> kit ok -> Ready", w.dir.state)
end

T.log("== match_running holds through a reconnect")
do
    T.check(D.match_running({ exists = true, connected = false, phase = "live" }) == true,
        "reconnecting mid-Live (session process up, not connected): the match still runs")
    T.check(D.match_running({ exists = true, connected = true, phase = "match_over" }) == true, "match_over counts")
    T.check(not D.match_running({ exists = true, connected = true, phase = "lobby" }), "lobby: no match")
    T.check(not D.match_running({ exists = false, phase = "live" }), "no session process: no match")
    T.check(not D.match_running({ exists = true, phase = "live", terminal = "kicked" }), "a terminal status: no match")
end

T.log("== a briefly absent kit_status is not 'no kit status'")
do
    local w = new_world()
    w:sidecar_status("connected")
    w:match("countdown", "Map_Arena_Pit", 0)
    w:spawns(1, "Map_Arena_Pit", 100, 200, 10)
    w:start(); w:tick(2); w:load()
    w.census_n = 1
    local line = { pawn = "someone_else", ok = true }
    for i = 1, 200 do
        -- a status that is briefly absent (never written yet / cleared) on every third read
        if i % 3 == 0 then w.kit_status = nil else w.kit_status = line end
        w:tick(1)
    end
    local sv = w:last_ev("spawn_verified")
    T.check(sv and T.contains(sv.steps, "kit=FAILED") and not T.contains(sv.steps, "kit=unavailable"),
        "single missing reads never make the kit check 'unavailable'", sv and sv.steps)
end

T.log("== env.census walks no Willie before the world settled; the census step waits")
do
    local walks = 0
    local old = _G.FindAllOf
    _G.FindAllOf = function() walks = walks + 1; return {} end
    local settled = false
    local env = D.make_ue_env({ WG = { settled = function() return settled end, pc = function() return nil end },
        UEHelpers = {}, log = function() end, ev = function() end, state_dir = "x" })
    local v, d = env.census(nil)
    T.check(v == nil and d == "settling" and walks == 0, "unsettled world: census returns 'settling', no FindAllOf", T.repr({ v, d, walks }))
    settled = true
    v = env.census(nil)
    T.check(v == 0 and walks == 1, "settled: the census walks the Willies", T.repr({ v, walks }))
    _G.FindAllOf = old

    -- the pipeline's census step waits for a settling world instead of a DEFECT
    local w = new_world()
    local n_settling = 6
    w.env.census = function(p)
        if n_settling > 0 then n_settling = n_settling - 1; return nil, "settling" end
        return w.census_n, "Willie_BP_C_" .. w.census_n
    end
    w:sidecar_status("connected")
    w:match("lobby", "Map_Arena_Pit", 0)
    w:start(); w:tick(4)
    w:match("countdown", "Map_Arena_Pit", 0)
    w:spawns(1, "Map_Arena_Pit", 100, 200, 10)
    w:tick(1); w:load(); w:tick(2); w:placed(1)
    w.kit_status = { pawn = w.pawn.id, ok = true, armour_n = 5, r_class = "Sword", l_class = "Shield", rev = 1 }
    w.census_n = 1
    w:tick(40)
    local sv = w:last_ev("spawn_verified")
    T.check(sv and T.contains(sv.steps, "census=ok") and n_settling == 0,
        "the census step waited out the settle window, then verified", sv and sv.steps)
end

T.log("== RVP guard: travel waits for the Runtime Vertex Paint queue")
do
    -- env.travel_hold stands in for shared/hsmp_rvp.lua: busy = held this tick.
    local rv = { busy = 0, holds = 0, issued = 0, reopened = {}, ticks = 0 }
    local function setup(w)
        w.env.travel_hold = function(why) rv.holds = rv.holds + 1; rv.why = why
            if rv.busy > 0 then rv.busy = rv.busy - 1; return true end
            return false
        end
        w.env.travel_issued = function() rv.issued = rv.issued + 1 end
        w.env.rvp_reopen = function(why) rv.reopened[#rv.reopened + 1] = why end
        w.env.rvp_tick = function() rv.ticks = rv.ticks + 1 end
    end
    local w = to_ready({ setup = setup })
    T.check(w.dir.state == "Ready" and #w.opens == 1 and rv.issued == 1, "first travel issued through the hold (queue idle)",
        T.repr({ w.dir.state, w.opens, rv.issued }))
    T.check(rv.ticks > 0, "rvp_tick runs every Director tick")
    T.check(#rv.reopened >= 1 and T.contains(rv.reopened[#rv.reopened], "new world Map_Arena_Pit"),
        "the queue reopens when the new world arrives", T.repr(rv.reopened))
    w:match("live", "Map_Arena_Pit", 1); w:tick(2)
    w:match("roundover", "Map_Arena_Pit", 1); w:tick(2)
    -- a corpse still bleeds: the queue is busy for 3 Director ticks
    rv.busy = 3
    local n0, h0 = #w.opens, rv.holds
    w:match("countdown", "Map_Arena_Pit", 1); w:tick(1)
    T.check(#w.opens == n0 and w.dir.state == "Travel", "round reload HELD while RVP tasks are in flight (no OpenLevel)",
        T.repr({ w.opens, w.dir.state }))
    T.check(T.contains(w:logtext(), "travel to Map_Arena_Pit held (round 2 starting)"), "the hold is logged once")
    w:tick(1)
    T.check(#w.opens == n0 and w.dir.state == "Travel", "still held on the next tick")
    w:tick(2)
    T.check(#w.opens == n0 + 1 and w.opens[#w.opens] == "Map_Arena_Pit" and w.dir.state == "WaitWorld",
        "quiet -> exactly one OpenLevel, state WaitWorld", T.repr({ w.opens, w.dir.state }))
    T.check(w.dir.prep.opens == 1, "held ticks do not count as OpenLevel attempts", tostring(w.dir.prep.opens))
    T.check(rv.issued == 2 and rv.holds - h0 == 4, "travel_issued after the OpenLevel; 3 held + 1 pass", T.repr(rv))
    local nlog = select(2, w:logtext():gsub("held %(round 2 starting%)", ""))
    T.check(nlog == 1, "one hold log line per travel", tostring(nlog))
    w:tick(4)
    T.check(#w.opens == n0 + 1, "no second OpenLevel while waiting for the world")
    local nre = #rv.reopened
    w:load(); w:tick(1)
    T.check(#rv.reopened == nre + 1 and w.dir.state == "Spawn" and w.dir.loaded_for == 2, "new world: reopened, round 2 pipeline",
        T.repr({ rv.reopened, w.dir.state, w.dir.loaded_for }))

    -- the native soft-object reroute never waits (the native travel is already pending)
    rv.busy = 5
    local n1 = #w.opens
    T.check(w.dir:on_soft_travel_post("Map_Hub_Tavern_Frank") == "Map_Arena_Pit" and #w.opens == n1 + 1,
        "soft-travel re-issue bypasses the hold", T.repr(w.opens))
    rv.busy = 0
end

T.log("== RVP guard: the menu travel waits for the RVP queue too")
do
    local rv = { busy = 0 }
    local w = to_ready({ setup = function(w0)
        w0.env.travel_hold = function() if rv.busy > 0 then rv.busy = rv.busy - 1; return true end; return false end
        w0.env.travel_issued = function() end
    end })
    w:match("match_over", "Map_Arena_Pit", 3); w:tick(4)
    rv.busy = 2
    local n0 = #w.opens
    w:match("lobby", "Map_Arena_Pit", 0); w:tick(1)
    T.check(w.dir.state == "TravelMenu" and #w.opens == n0, "lobby -> TravelMenu, menu travel held", T.repr({ w.dir.state, w.opens }))
    w:tick(3)
    T.check(#w.opens == n0 + 1 and w.opens[#w.opens] == "Map_Menu_Startup", "held menu travel retried on the next ticks, not after 5 s",
        T.repr(w.opens))
    w:tick(5)
    T.check(#w.opens == n0 + 1, "menu travel issued once (the 2 s arrival watchdog has not fired yet)")
end

T.log("== RVP guard: make_ue_env wires shared/hsmp_rvp.lua")
do
    local calls = {}
    local R = {
        hold = function(why) calls[#calls + 1] = "hold:" .. tostring(why); return true end,
        travel_issued = function() calls[#calls + 1] = "issued" end,
        reopen = function(why) calls[#calls + 1] = "reopen:" .. tostring(why) end,
        tick = function() calls[#calls + 1] = "tick" end,
    }
    local env = D.make_ue_env({ WG = {}, UEHelpers = {}, log = function() end, ev = function() end, state_dir = "x", RVP = R })
    T.check(env.travel_hold("r") == true and env.travel_hold ~= nil, "env.travel_hold -> RVP.hold")
    env.travel_issued(); env.rvp_reopen("w"); env.rvp_tick()
    T.check(table.concat(calls, ",") == "hold:r,issued,reopen:w,tick", "every RVP entry point wired", table.concat(calls, ","))
    R.hold = function() error("boom") end
    T.check(env.travel_hold("x") == false, "a failing RVP.hold never blocks the travel")
    local env2 = D.make_ue_env({ WG = {}, UEHelpers = {}, log = function() end, ev = function() end, state_dir = "x" })
    T.check(env2.travel_hold == nil and env2.rvp_tick == nil, "no RVP module: no hold (old behaviour)")
end
T.log("== RVP guard: a held menu travel from the splash screen is retried")
do
    local w = new_world()
    local busy, holds = 0, 0
    w.env.travel_hold = function() holds = holds + 1; if busy > 0 then busy = busy - 1; return true end; return false end
    w.env.travel_issued = function() end
    w.world = { ok = true, short = "Map_Menu_SplashScreens", key = "splash#1" }
    w:start(); w:tick(1)
    busy = 2                                   -- the request tick runs open() twice (serve + step)
    w:request(1, "menu", nil, "autotest_splash_skip"); w:tick(1)
    T.check(#w.opens == 0 and w.dir.state == "TravelMenu", "held on the request tick", T.repr(w.opens))
    w:tick(1)
    T.check(#w.opens == 1 and w.opens[1] == "Map_Menu_Startup",
        "retried on the next tick although the splash is a menu world", T.repr({ w.opens, holds }))
    w:tick(10)
    T.check(#w.opens == 1, "issued once")
    w:load("Map_Menu_Startup"); w:tick(1)
    T.check(w.dir.state == "Menu" and w.dir.menu_held == nil, "arrived: Menu, nothing held", w.dir.state)
end