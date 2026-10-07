-- HSMPAvatars runtime tests, offline under umg_mock.
--
--     hsmp-tools lua-test avatars
--
--   * the Willie_BP ReceiveTick post-hook fails at mod load (the BP is
--     not loaded in the menu) and is retried at 1 Hz until it registers; the
--     registration is logged.
--   * a stand-in HSMPCombat put to death for a server-declared death
--     (the `standin_dead` bus record) is never re-claimed; a stand-in that died locally
--     with no owner death is re-claimed only after a grace.
-- Every case runs in its own Lua state (T.isolated).

local mode, opts = ...
local AV = T.path("mods/HSMPAvatars/Scripts/main.lua")

if mode ~= "case" then
    T.isolated(T.script, "case", { kind = "hook" })
    T.isolated(T.script, "case", { kind = "parse" })
    T.isolated(T.script, "case", { kind = "corpse", standin_dead = true })
    T.isolated(T.script, "case", { kind = "corpse", standin_dead = false })
    T.isolated(T.script, "case", { kind = "corpse", vitals_dead = true })
    T.isolated(T.script, "case", { kind = "census" })
    T.isolated(T.script, "case", { kind = "frame" })
    T.isolated(T.script, "case", { kind = "levelchange" })
    T.isolated(T.script, "case", { kind = "weapon_destroyed" })
    T.isolated(T.script, "case", { kind = "bodies3" })
    T.isolated(T.script, "case", { kind = "spectate" })
    T.isolated(T.script, "case", { kind = "tune" })
    T.isolated(T.script, "case", { kind = "native_servo" })
    T.isolated(T.script, "case", { kind = "native_neutralise" })
    T.isolated(T.script, "case", { kind = "native_wservo" })
    T.isolated(T.script, "case", { kind = "wpn_status" })
    T.isolated(T.script, "case", { kind = "skeleton_ref" })
    T.isolated(T.script, "case", { kind = "body_scale" })
    T.isolated(T.script, "case", { kind = "fist_grip" })
    T.isolated(T.script, "case", { kind = "pose_context" })
    T.isolated(T.script, "case", { kind = "bodyheight" })
    return
end

local M = require("umg_mock")
local sd = T.tmpdir("hsmp_av_sd_")
local la = T.tmpdir("hsmp_av_la_")

-- A relayed root of peer `id` as the sidecar publishes it: the peer_root record slot (root +
-- peer id; rotation a quaternion, here a yaw of 45 degrees). Replaces me_remote<id>.json.
local function peer_root(id, tick, x, y, z)
    local h = math.rad(45) / 2
    _G.HSMPNative.sc_put("peer_root", { peer_id = id, root = { tick = tick, ts = 1000 + tick, pos = { x, y, z },
        rot = { 0, 0, math.sin(h), math.cos(h) } } }, id)
end

local function boot(register_ok)
    M.install({ state_dir = sd, env = { LOCALAPPDATA = la, HSMP_INST = "7" }, strict = true })
    package.path = T.path("mods/shared") .. "/?.lua;" .. package.path
    local arena = "World /Game/Maps/Arenas/Map_Arena_Pit/Map_Arena_Pit.Map_Arena_Pit"
    M.Methods.GetFullName = function(self)
        if rawget(self, "__cls") == "World" then return arena end
        return "Obj " .. tostring(rawget(self, "__name"))
    end
    M.Methods.GetAddress = function(self) return rawget(self, "__addr") or 4096 end
    M.Methods.GetWorld = function() return M.world end
    M.Methods.K2_GetActorLocation = function() return { X = 500, Y = 0, Z = 100 } end
    M.Methods.GetWorldDeltaSeconds = function() return 1 / 60 end
    M.Methods.GetRealTimeSeconds = function() return M.now / 1000 end
    M.hooks, M.hook_calls = {}, 0
    M.register_ok = register_ok
    _G.RegisterHook = function(path, pre, post)
        M.hook_calls = M.hook_calls + 1
        if path:find("Willie_BP_C:ReceiveTick", 1, true) and not M.register_ok then
            error("No UFunction with the specified name was found: " .. path)
        end
        M.hooks[path] = { pre = pre, post = post }
        return 1, 2
    end
    _G.HSMP_AVATARS_TEST = {}
    dofile(AV)
    return HSMP_AVATARS_TEST.api
end

-- The sidecar (typed): its `link` record + header heartbeat, and the peer directory
-- (the sidecar allocates an entry for every connected roster peer, lobby included).
-- peers: { {id, nick}, ... }
local function sidecar(peers)
    local N = _G.HSMPNative
    N.sc_put("link", { status = 1, state = 1, my_peer_id = 1 })   -- CONNECTED, UP
    N._st.hb_age = 0.05
    local e = {}
    for _, p in ipairs(peers) do e[#e + 1] = { id = p[1], nick = p[2] } end
    N.sc_peer_dir(e)
end

if opts.kind == "hook" then
    local api = boot(false)
    local log = M.logtext()
    T.check(api ~= nil, "HSMPAvatars loads under the mock with its test api")
    T.check(T.contains(log, "ReceiveTick=false(retrying)"), "load-time registration fails in the menu (BP not loaded)", log)
    T.check(api.tick_hook.ok == false and api.tick_hook.tries == 1, "one try at load")
    M.run(2000)   -- the menu: still not loaded, retried at 1 Hz
    T.check(api.tick_hook.ok == false and api.tick_hook.tries >= 2 and api.tick_hook.tries <= 4,
        "retried about once a second while unavailable", api.tick_hook.tries)
    M.register_ok = true   -- the arena loads Willie_BP
    M.run(1200)
    log = M.logtext()
    T.check(api.tick_hook.ok == true, "registered once the BP class exists")
    T.check(T.contains(log, "ReceiveTick post-hook registered (try"), "registration is logged", log)
    local h = M.hooks["/Game/Character/Blueprints/Willie_BP.Willie_BP_C:ReceiveTick"]
    T.check(h and type(h.pre) == "function" and type(h.post) == "function", "pre (no-op) + post callbacks")
    local n = M.hook_calls
    M.run(3000)
    T.check(M.hook_calls == n, "no further RegisterHook calls once registered")
    T.check(not T.contains(M.logtext(), "LOOP ERR"), "no loop errors", M.logtext())
end

-- Dev tuning knobs: dev_cmd TUNE records (hsmp-tools ipc-ctl tune), no .pose_tune.json.
if opts.kind == "tune" then
    local api = boot(true)
    local N = HSMPNative
    T.check(type(N.sc_dev) == "function", "the mock has the DevCtl ring (sc_dev)")
    local t = api.tune()
    T.check(t.servo == nil and select(1, api.caps()) == 900, "defaults before any record")
    T.write(sd .. "/.pose_tune.json", '{"servo":0,"cap_lin":5}\n')
    api.on_tick()
    T.check(api.tune().servo == nil and select(1, api.caps()) == 900, "a .pose_tune.json file is ignored (no state-dir knob)")
    N.sc_dev({ id = 1, op = 2, key = "servo", num = 0 })
    N.sc_dev({ id = 2, op = 2, key = "cap_lin", num = 1e9 })
    N.sc_dev({ id = 3, op = 2, key = "lead", num = -12.5 })
    N.sc_dev({ id = 4, op = 2, key = "tdiag", num = 1 })
    N.sc_dev({ id = 5, op = 2, key = "bench", num = 0 })
    api.on_tick()
    t = api.tune()
    T.check(t.servo == 0, "servo 0 stops driving", t.servo)
    T.check(t.cap_lin == 20000 and select(1, api.caps()) == 20000, "cap_lin clamped to 20000 and applied", t.cap_lin)
    T.check(t.lead == -12.5, "lead set", t.lead)
    T.check(t.tdiag == 1 and t.bench == 1, "tdiag flag on; bench clamped to >= 1", T.repr({ t.tdiag, t.bench }))
    T.check(T.contains(M.logtext(), "pose tune: "), "a tune change is logged")
    N.sc_dev({ id = 6, op = 2, key = "tdiag", num = 0 })
    N.sc_dev({ id = 7, op = 2, key = "servo", num = 3 })
    N.sc_dev({ id = 8, op = 2, key = "warp", num = 1 })
    N.sc_dev({ id = 9, op = 2, key = "warp", num = 2 })
    N.sc_dev({ id = 10, op = 1, key = "gain", num = 0.9 })   -- AUTOTEST: not ours
    N.sc_dev({ id = 11, op = 3, key = "tdiag", num = 1 })    -- TDIAG: HSMPSync's
    api.on_tick()
    t = api.tune()
    T.check(not t.tdiag and t.servo == 1, "tdiag off clears the flag; a nonzero flag value is stored as 1", T.repr({ t.tdiag, t.servo }))
    T.check(t.warp == nil and T.count(M.logtext(), "warp") == 1, "an unknown key is ignored and logged once", M.logtext())
    T.check(t.gain == nil, "records of other ops are ignored")
    T.check(api.PURE.tune_value("gain", 0 / 0) == nil and api.PURE.tune_value("gain", 7) == 1, "NaN refused; gain clamped to 1")
end

if opts.kind == "bodyheight" then
    local api = boot(true)
    local me = M.new_obj("Willie_BP_C", "ME"); rawset(me,"__addr",7001)
    me.__props.Health=100; M.pc.__props.Pawn=me
    sidecar({ {1,"Me"}, {2,"Peer"} }); peer_root(2,10,300,0,100); M.run(2200)
    local actor_scale, mesh_scale, sim, calls = {X=.97,Y=.97,Z=.97}, {X=.997,Y=.996,Z=.998}, true, {}
    local field="Height_21_0EB204DF4978B92AD0ED188FD32EEC7B"
    local mesh={IsValid=function() return true end,GetAddress=function() return 8002 end,
        K2_GetComponentScale=function() return mesh_scale end,IsSimulatingPhysics=function() return sim end,
        SetSimulatePhysics=function(_,v) sim=v;calls[#calls+1]=v and "on" or "off" end,
        SetWorldScale3D=function(_,v) mesh_scale=v;calls[#calls+1]="mesh" end}
    local actor={IsValid=function() return true end,GetAddress=function() return 8001 end,
        GetFName=function() return {ToString=function() return "REMOTE" end} end,Mesh=mesh,["Character Passport"]={[field]=.76},
        GetActorScale3D=function() return actor_scale end,
        SetActorScale3D=function(_,v) actor_scale=v;calls[#calls+1]="actor" end}
    actor["Set Character Height"]=function(self)
        calls[#calls+1]="native-height"
        local k=.875+.125*self["Character Passport"][field];self:SetActorScale3D({X=k,Y=k,Z=k})
    end
    FindAllOf=function(cls) if cls=="Willie_BP_C" then return {me,actor} end end
    local p={gen=api.generation(),actor=actor,driving=true,in_range=true,nick="Peer",
        body={mesh=mesh,ctl="servo",sv={},handles={},sims={}}}
    T.check(select(2,api.generation())==p.gen,"height test uses settled current-world cache",T.repr({api.generation()}))
    api.set_puppet(2,p)
    api.PX.bodyheight("2 0.946")
    T.check(p.body.height_probe and p.body.repose and calls[1]=="off" and calls[2]=="native-height",
        "remote height probe enters driver's physics-off window before native actor scaling",M.logtext())
    T.check(p.body.sv==nil and p.body.scale_remeasure,"probe discards cached COM and rest offsets")
    if not p.body.height_probe then return end
    local expiry=p.body.height_probe.until_t
    api.PX.height_tick(p,expiry-.01)
    T.check(actor["Character Passport"][field]==.946,"geometry probe remains active before deadline")
    api.PX.height_tick(p,expiry)
    T.check(p.body.height_probe==nil and actor["Character Passport"][field]==.76 and actor_scale.X==.97,
        "bounded timeout restores original native passport/actor scale without a new pose")
    p.body.repose=nil;sim=true
    api.PX.bodyheight("2 0.946")
    api.PX.height_restore(p,"release",false)
    T.check(p.body.height_probe==nil and sim and not p.body.repose and mesh_scale.Y==.996,
        "release cleanup restores original geometry and simulation immediately")
    p.body.repose=nil;sim=true
    api.PX.bodyheight("2 0.946")
    local n=#calls
    actor.IsValid=function() error("old world object touched") end
    mesh.IsValid=function() error("old world mesh touched") end
    api.drop_caches("test world teardown")
    T.check(#calls==n and next(api.puppets())==nil,
        "world teardown discards diagnostic state without touching old native objects")
    actor.IsValid=function() return true end;mesh.IsValid=function() return true end
    p.body.height_probe=nil;p.body.repose=nil
    api.set_puppet(2,p)
    p.body.repose=nil
    M.pc.__props.Pawn=actor
    n=#calls;api.PX.bodyheight("2 0.946")
    T.check(#calls==n and p.body.height_probe==nil,"probe refuses a stand-in now possessed as our pawn")
end

if opts.kind == "pose_context" then
    local api = boot(true)
    HSMPNative.sc_put("session", { seq = 1, match_id = 901, round = 2, phase = 3 })
    HSMPNative.sc_put("mode", { seq = 1, match_id = 901, round = 2,
        rows = { { peer_id = 2, seat = 1, life = 3, alive = true } } })
    local p = { gen = -1, driving = true, last = { v2 = true, has_context = true,
        match_id = 901, round = 2, life = 2 }, aim = {}, shown = {}, play = { seq = 77, seq_at = 500 } }
    api.drive_frame(2, p, 501)
    T.check(p.driving == false, "authoritative new life releases an old cached servo immediately")
    T.check(p.last == nil and p.aim == nil and p.shown == nil and p.play.seq == nil,
        "old cached pose and displayed timeline are removed before another frame can drive")
end

if opts.kind == "parse" then
    local api = boot(true)
    local P = api.PURE
    local s = { match_id = 901, round = 2, state = "live" }
    local m = { match_id = 901, round = 2, rows = { [2] = { life = 3 } } }
    local pose = { has_context = true, match_id = 901, round = 2, life = 3, B = {}, m = 0 }
    T.check(P.pose_context_ok(pose, s, m, 2), "pose belongs to current match/round/native life")
    T.check(not P.pose_context_ok(pose, s, nil, 2), "live pose waits for authoritative Mode life")
    pose.life = 2
    T.check(not P.pose_context_ok(pose, s, m, 2), "previous life is rejected even when cached")
    pose.life, pose.match_id = 3, 900
    T.check(not P.pose_context_ok(pose, s, m, 2), "previous match pose is rejected")
    pose.match_id, pose.round = 901, 1
    T.check(not P.pose_context_ok(pose, s, m, 2), "previous round pose is rejected")
    pose.round, pose.has_context = 2, false
    T.check(not P.pose_context_ok(pose, s, m, 2), "active match never accepts an unlabelled pose")
    -- A relayed peer_root as the native decoder hands it over: plain Root fields, no has_context.
    local root = { tick = 2600, ts = 65676, pos = { 815.3, 280.4, 377.0 }, rot = { 0, 0, -0.95, 0.30 },
                   vel = { 0, 0, 0 }, match_id = 901, round = 2, life = 3, send_wall_ms = 1 }
    T.check(P.pose_context_ok(P.root_context(root), s, m, 2), "stamped peer root of the current life is live (stand-in can spawn)")
    root.life = 2
    T.check(not P.pose_context_ok(P.root_context(root), s, m, 2), "peer root of a previous life is rejected")
    root.life, root.match_id = 3, 0
    T.check(not P.pose_context_ok(P.root_context(root), s, m, 2), "unstamped peer root is rejected in a match")
    s.state, s.spawn_round, pose.round, pose.life, pose.has_context = "countdown", 3, 3, 1, true
    T.check(P.pose_context_ok(pose, s, m, 2), "next round placement uses its own initial life")
    local shown=P.displayed_pose(pose,"Willie_BP_C_rendered",100,500)
    pose.life=2
    local row=P.playback_row(2,shown,501)
    T.check(row and row.life==1 and row.match_id==901 and row.round==3 and row.pawn=="Willie_BP_C_rendered",
        "displayed playback retains original pose life and actual pawn after source table changes")
    T.check(P.playback_row(2,shown,751)==nil,"stalled displayed pose cannot attribute new contacts")
    local delayed=P.playback_row(2,shown,850,true)
    T.check(delayed and delayed.local_ms==500 and delayed.life==1,
        "approved delayed trade retains original timestamp and life without refreshing contact eligibility")
    T.check(P.playback_row(2,shown,499,true)==nil,"retained generation cannot authorize a future timestamp")
    shown.pawn=""
    T.check(P.playback_row(2,shown,501)==nil,"displayed playback requires exact native actor identity")
    pose.life=1
    pose.life = 3
    T.check(not P.pose_context_ok(pose, s, m, 2), "old Mode life cannot authorize next round pose")
    pose.life = 1
    local parsed = P.play_from_out(pose)
    T.check(parsed.has_context and parsed.match_id == 901 and parsed.round == 3 and parsed.life == 1,
        "typed pose retains life context in alternating cached output")
    P.play_from_out({ B = {}, m = 0 }, parsed)
    T.check(not parsed.has_context and parsed.match_id == nil and parsed.life == nil,
        "reused pose table clears absent life context")
    -- the typed `standin_dead` bus record HSMPCombat writes ({wall, rows = {{peer, name}}})
    local set, wall = api.parse_standin_dead({ wall = 1700000000,
        rows = { { peer = 2, name = "Willie_BP_C_5" }, { peer = 11, name = "Willie_BP_C_9" } } })
    T.check(set and set[2] == "Willie_BP_C_5" and set[11] == "Willie_BP_C_9" and wall == 1700000000, "parse: complete record")
    T.check(api.parse_standin_dead(nil) == nil and api.parse_standin_dead({}) == nil, "parse: no record / no wall stamp")
    local e = api.parse_standin_dead({ wall = 5, rows = {} })
    T.check(e and next(e) == nil, "parse: empty set")
    local N = HSMPNative
    T.check(N.bus_put("standin_dead", { wall = os.time(), rows = { { peer = 2, name = "W" } } }) == true, "typed bus write")
    T.check(api.combat_declared_dead(2) and not api.combat_declared_dead(3), "declared: listed peer only")
    T.check(api.combat_declared_dead(2), "an unchanged record keeps the last set")
    T.check(api.combat_declared_dead(2, "W") and not api.combat_declared_dead(2, "Willie_BP_C_77"),
        "the listed stand-in name must match")
    N.bus_put("standin_dead", { wall = os.time() - 60, rows = { { peer = 2, name = "W" } } })
    T.check(not api.combat_declared_dead(2), "a set older than 5 s is void (crashed HSMPCombat)")
end

if opts.kind == "corpse" then
    local api = boot(true)
    -- an MP arena: our pawn + one peer (2) with a stand-in
    local me = M.new_obj("Willie_BP_C", "Willie_BP_C_ME"); rawset(me, "__addr", 7001)
    me.__props.Health = 100
    M.pc.__props.Pawn = me
    sidecar({ { 1, "Me" }, { 2, "B" } })
    peer_root(2, 10, 100.0, 0.0, 90.0)
    M.run(2200)   -- past PX.WORLD_SETTLE_S: on_tick touches no Willie before
    local corpse = M.new_obj("Willie_BP_C", "Willie_BP_C_5"); rawset(corpse, "__addr", 7005)
    corpse.__props.Health = 100
    api.set_puppet(2, { actor = corpse, nick = "B", claimed_at = 0, gen = 0, addr = 7005 })
    M.run(200)
    T.check(api.puppets()[2] and api.puppets()[2].actor == corpse, "stand-in kept while alive")
    -- HSMPCombat: server says peer 2 died -> record first, then Health 0 + Death()
    if opts.standin_dead then
        HSMPNative.bus_put("standin_dead", { wall = os.time(), rows = { { peer = 2, name = "Willie_BP_C_5" } } })
    end
    if opts.vitals_dead then   -- the owner's own `peer_vitals` record says dead (VF DEAD, Health 0)
        HSMPNative.sc_put("peer_vitals", { seq = 9, flags = 1, v = { 0 } }, 2)
    end
    corpse.__props.Health = 0
    corpse.__props.DED = true
    M.run(2000)   -- the owner's vitals never caught up in this window
    local log = M.logtext()
    if opts.standin_dead or opts.vitals_dead then
        T.check(api.puppets()[2] and api.puppets()[2].actor == corpse, "server-declared death keeps the corpse", T.repr(api.puppets()[2]))
        T.check(not T.contains(log, "re-claiming"), "no re-claim (no living stand-in 37 ms later)", log)
        T.check(api.puppets()[2].owner_dead == true, "the corpse is owner-dead (free ragdoll, not driven)")
    else
        T.check(T.contains(log, "puppet died locally (HP 0.0 DED=true); re-claiming") or T.contains(log, "puppet died locally (HP 0 DED=true); re-claiming"),
            "a local death without an owner death is still re-claimed", log)
    end
end

if opts.kind == "census" then
    -- Peer 3 is rostered but still in the lobby (no snapshot); the
    -- unreplicated extra foe must still be removed once peer 2 has a stand-in.
    local api = boot(true)
    M.Methods.SetActorHiddenInGame = function(self, h) self.__props.bHidden = h end
    M.Methods.IsActorBeingDestroyed = function() return false end
    local me = M.new_obj("Willie_BP_C", "Willie_BP_C_ME"); rawset(me, "__addr", 7001)
    me.__props.Health = 100
    M.pc.__props.Pawn = me
    local standin = M.new_obj("Willie_BP_C", "Willie_BP_C_5"); rawset(standin, "__addr", 7005)
    standin.__props.Health = 100
    local extra = M.new_obj("Willie_BP_C", "Willie_BP_C_9"); rawset(extra, "__addr", 7009)
    extra.__props.Health = 100
    _G.FindAllOf = function(c)
        if c == "Willie_BP_C" then return { me, standin, extra } end
        return nil
    end
    local tick = 0
    local function beat()
        tick = tick + 1
        sidecar({ { 1, "Me" }, { 2, "B" }, { 3, "Lobby" } })
        peer_root(2, 10 + tick, 100.0, 0.0, 90.0)
    end
    beat(); M.run(300)
    api.set_puppet(2, { actor = standin, nick = "B", claimed_at = 0, gen = 0, addr = 7005 })
    for _ = 1, 30 do beat(); M.run(250) end
    local log = M.logtext()
    T.check(T.contains(log, "census: removed Willie_BP_C_9"), "a rostered peer without a pawn does not block the census", log)
    T.check(extra.__props.bHidden == true and standin.__props.bHidden ~= true, "only the extra is taken out")
    -- Every connected roster peer has a PeerDir entry now (lobby included): an entry alone
    -- never claims a stand-in; only an advancing PeerRoot does.
    T.check(api.puppets()[3] == nil and not T.contains(log, "puppet for peer 3"),
        "a PeerDir entry without a streaming PeerRoot never claims a stand-in", T.repr(api.puppets()[3]))
end

if opts.kind == "frame" then
    -- No PlayerController lookup (a full FindAllOf in the real
    -- UEHelpers) per rendered frame; a stand-in leaving POSE_RANGE is
    -- released (gravity / collision / muscles back), not left hanging.
    local api = boot(true)
    local U = require("UEHelpers")
    local pc_calls = 0
    local orig = U.GetPlayerController
    U.GetPlayerController = function() pc_calls = pc_calls + 1; return orig() end
    M.Methods.K2_GetActorLocation = function(self) return rawget(self, "loc") or { X = 0, Y = 0, Z = 100 } end
    local me = M.new_obj("Willie_BP_C", "Willie_BP_C_ME"); rawset(me, "__addr", 7001)
    me.__props.Health = 100
    M.pc.__props.Pawn = me
    local si = M.new_obj("Willie_BP_C", "Willie_BP_C_5"); rawset(si, "__addr", 7005)
    si.__props.Health = 100
    rawset(si, "loc", { X = 300, Y = 0, Z = 100 })
    local tick = 0
    local function beat()
        tick = tick + 1
        sidecar({ { 1, "Me" }, { 2, "B" } })
        peer_root(2, 10 + tick, 300.0, 0.0, 90.0)
        -- a live v2 pose stream with no bodies (the capsule only): keeps it driven
        HSMPNative.sc_peer_play(2, { peer_id = 2, seq = tick, v = 2, pt = 0, mode = "interp", cut = 0, root = { 300.0, 0.0, 100.0, 0.0 }, m = 0, B = {} })
    end
    beat(); M.run(300)
    for _ = 1, 8 do beat(); M.run(250) end   -- past PX.WORLD_SETTLE_S
    api.set_puppet(2, { actor = si, nick = "B", claimed_at = 0, gen = 0, addr = 7005, driving = true, in_range = true })
    M.run(100)
    pc_calls = 0
    for _ = 1, 4 do beat(); M.run(250) end   -- 1 s: ~30 ticks, ~125 frames (8 ms fallback loop)
    T.check(pc_calls > 0 and pc_calls <= 45, "PlayerController lookups follow the 33 ms tick, not the frame rate (" .. pc_calls .. " in 1 s)")
    -- out of range
    rawset(si, "loc", { X = 6000, Y = 0, Z = 100 })
    api.puppets()[2].driving = true
    for _ = 1, 2 do beat(); M.run(250) end
    local log = M.logtext()
    T.check(T.contains(log, "pose peer 2: out of range"), "leaving POSE_RANGE releases the stand-in", log)
    T.check(api.puppets()[2] and api.puppets()[2].driving == false, "released: no longer driven")
    T.check(not T.contains(log, "LOOP ERR"), "no loop errors")
end

if opts.kind == "levelchange" then
    -- Strict mock: a round reset frees every object of the old world;
    -- nothing may be touched afterwards, IsValid() included (it crashes the game).
    local api = boot(true)
    local me = M.new_obj("Willie_BP_C", "Willie_BP_C_ME"); rawset(me, "__addr", 7001)
    me.__props.Health = 100
    M.pc.__props.Pawn = me
    local si = M.new_obj("Willie_BP_C", "Willie_BP_C_5"); rawset(si, "__addr", 7005)
    si.__props.Health = 100
    local tick = 0
    local function beat()
        tick = tick + 1
        sidecar({ { 1, "Me" }, { 2, "B" } })
        peer_root(2, 10 + tick, 300.0, 0.0, 90.0)
        HSMPNative.sc_peer_play(2, { peer_id = 2, seq = tick, v = 2, pt = 0, mode = "interp", cut = 0, root = { 300.0, 0.0, 100.0, 0.0 }, m = 0, B = {} })
    end
    beat(); M.run(300)
    for _ = 1, 8 do beat(); M.run(250) end   -- past PX.WORLD_SETTLE_S
    api.set_puppet(2, { actor = si, nick = "B", claimed_at = 0, gen = 0, addr = 7005, driving = true, in_range = true })
    for _ = 1, 3 do beat(); M.run(250) end
    -- round reset: LoadMap pre-hook, the old world freed, a new world + pawn
    M.premap(); M.kill_all()
    M.world = M.new_obj("World", "world2"); M.pc = M.new_obj("PC", "pc2")
    local me2 = M.new_obj("Willie_BP_C", "Willie_BP_C_ME2"); rawset(me2, "__addr", 7101)
    me2.__props.Health = 100
    M.pc.__props.Pawn = me2
    package.loaded["UEHelpers"].GetPlayerController = function() return M.pc end
    package.loaded["UEHelpers"].GetWorld = function() return M.world end
    if M.postmap then M.postmap() end
    for _ = 1, 4 do beat(); M.run(250) end
    T.check(#M.dead_touch == 0, "nothing of the old world touched after the reset, IsValid included", T.repr(M.dead_touch))
    T.check(api.puppets()[2] == nil or api.puppets()[2].actor ~= si, "the old stand-in is forgotten")
end

if opts.kind == "weapon_destroyed" then
    -- HSMPLoadout K2_DestroyActor's a stand-in's hand weapon (disarm /
    -- world item in hand); GC frees it ~60 s later. The cached root component
    -- (body.sv.wc) must never be touched again: in the strict mock IsValid on
    -- a freed object raises (as the real access violation would kill the game).
    local api = boot(true)
    local PX = api.PX
    _G.StaticFindObject = function() return { __cls = "Class" } end
    M.Methods.IsSimulatingPhysics = function() return true end
    M.Methods.SetEnableGravity = function(self, on) rawset(self, "grav", on) end
    M.Methods.SetCollisionResponseToChannel = function() end
    M.Methods.GetTransform = function() return { Translation = { X = 0, Y = 0, Z = 0 }, Rotation = { X = 0, Y = 0, Z = 0, W = 1 } } end
    M.Methods.GetCenterOfMass = function() return { X = 1, Y = 0, Z = 0 } end
    M.Methods.K2_GetComponentLocation = function() return { X = 0, Y = 0, Z = 0 } end
    M.Methods.K2_SetWorldLocation = function(self) rawset(self, "moved", (rawget(self, "moved") or 0) + 1) end
    M.Methods.SetPhysicsLinearVelocity = function() end
    M.Methods.SetPhysicsAngularVelocityInDegrees = function() end
    M.Methods.SetAllPhysicsLinearVelocity = function() end
    M.Methods.GetSocketLocation = function() return { X = 0, Y = 0, Z = 0 } end
    local comps = {}
    M.Methods.K2_GetComponentsByClass = function() return comps end
    local si = M.new_obj("Willie_BP_C", "Willie_BP_C_5"); rawset(si, "__addr", 7005)
    local function weapon(name, addr, raddr)
        local w = M.new_obj("ModularWeaponBP_Polearm_C", name); rawset(w, "__addr", addr)
        local r = M.new_obj("StaticMeshComponent", name .. ".Root"); rawset(r, "__addr", raddr)
        w.__props.RootComponent = r
        return w, r
    end
    local function destroy(o) rawset(o, "IsValid", function() return false end) end   -- pending kill
    local function free(o) rawset(o, "IsValid", nil); rawset(o, "__dead", true) end   -- GC freed it: IsValid is an AV
    local mesh = M.new_obj("SkeletalMeshComponent", "Mesh")
    local body = { mesh = mesh, sims = { mesh }, snaps = 0, gravity = false, sv = { wc = {} } }
    local p = { actor = si, body = body }
    local w1, r1 = weapon("ModularWeaponBP_Polearm_C_3", 8001, 8002)
    si.__props["Weapon R"] = w1
    local c, wa = api.servo_weapon_parts(p, body, "Weapon R")
    T.check(c and wa == w1 and body.sv.wc["Weapon R"] == c and c.root == r1 and r1.grav == false,
        "a held weapon is cached with gravity off (servoed)")
    -- 1. disarm: Loadout destroys it, the hand reads empty, GC frees it
    destroy(w1); destroy(r1)
    si.__props["Weapon R"] = nil
    T.check(api.servo_weapon_parts(p, body, "Weapon R") == nil and body.sv.wc["Weapon R"] == nil,
        "an empty hand drops the cached entry at once")
    free(w1); free(r1)
    body.gravity = nil
    local ok, err = pcall(api.set_gravity, body, false)
    T.check(ok and #M.dead_touch == 0, "set_gravity never touches the freed root", tostring(err) .. " " .. T.repr(M.dead_touch))
    -- 2. a stale entry that servo_weapon_parts did not see this frame (the
    -- left hand): every user validates against a FRESH hand read first
    local w2, r2 = weapon("ModularWeaponBP_Sword_C_4", 8011, 8012)
    si.__props["Weapon L"] = w2
    api.servo_weapon_parts(p, body, "Weapon L")
    destroy(w2); destroy(r2); si.__props["Weapon L"] = nil; free(w2); free(r2)
    body.gravity = nil
    ok, err = pcall(api.set_gravity, body, true)
    T.check(ok and #M.dead_touch == 0 and body.sv.wc["Weapon L"] == nil, "set_gravity drops a stale entry untouched", tostring(err) .. " " .. T.repr(M.dead_touch))
    -- 3. ABA: a NEW weapon at the old address (different FName / root)
    local w3, r3 = weapon("ModularWeaponBP_Polearm_C_9", 8001, 8022)
    si.__props["Weapon R"] = w3
    local w0, r0 = weapon("ModularWeaponBP_Polearm_C_8", 8031, 8032)
    si.__props["Weapon R"] = w0
    local c0 = api.servo_weapon_parts(p, body, "Weapon R")
    destroy(w0); destroy(r0); si.__props["Weapon R"] = w3   -- swapped by the BP; w3 reuses an address
    rawset(w3, "__addr", 8031)
    free(w0); free(r0)
    local c3 = api.servo_weapon_parts(p, body, "Weapon R")
    T.check(c3 and c3 ~= c0 and c3.root == r3 and #M.dead_touch == 0,
        "same address, other FName: re-resolved, the old root never touched", T.repr(M.dead_touch))
    -- 4. snap_mesh / release_standin after a destroy
    destroy(w3); destroy(r3); si.__props["Weapon R"] = nil; free(w3); free(r3)
    ok, err = pcall(api.snap_mesh, body, 10, 0, 0)
    T.check(ok and #M.dead_touch == 0, "snap_mesh never moves a freed weapon", tostring(err) .. " " .. T.repr(M.dead_touch))
    -- 5. Loadout's destroy signal: a new generation drops every cached root pointer
    local w4, r4 = weapon("ModularWeaponBP_Sword_C_12", 8041, 8042)
    si.__props["Weapon R"] = w4
    local c4 = api.servo_weapon_parts(p, body, "Weapon R")
    HSMPNative.bus_put("standin_weapons", { gen = 7 })   -- the typed bus record HSMPLoadout bumps
    PX.wc_gen_at = -1e9
    PX.wc_valid(body)
    T.check(body.sv.wc_gen == "7" and c4.root == r4, "a new standin_weapons generation re-validates every entry (live ones keep a FRESH root)")
    -- 6. grips: the BP rebuilt the hand constraints; the cached ones are freed
    local g1 = M.new_obj("PhysicsConstraintComponent", "Grip_old"); rawset(g1, "__addr", 9001)
    p.grips = { at = 0, list = { { c = g1, addr = 9001, hand = "hand_r", stiff = 1 } }, by = {} }
    free(g1)
    local g2 = M.new_obj("PhysicsConstraintComponent", "Grip_new"); rawset(g2, "__addr", 9002)
    comps = { g2 }
    ok, err = pcall(PX.grips_off, p, true)
    T.check(ok and #M.dead_touch == 0 and p.grips.list[1].c == nil, "grips_off never touches a rebuilt (freed) grip", tostring(err) .. " " .. T.repr(M.dead_touch))
end

if opts.kind == "bodies3" then
    -- A 3-player match needs 2 stand-ins on this client. In MP the
    -- arenas spawn no foes, so the SpawnCombatants fallback is the only body
    -- source: ONE call must ask for every missing body, and a peer that still
    -- has none once the round is Live (re-claim, join in progress) must still
    -- get one.
    local api = boot(true)
    M.Methods.SetActorHiddenInGame = function(self, h) self.__props.bHidden = h end
    M.Methods.IsActorBeingDestroyed = function() return false end
    M.Methods.K2_SetActorLocation = function(self, l) rawset(self, "loc", l) end
    M.Methods.K2_GetActorLocation = function(self) return rawget(self, "loc") or { X = 500, Y = 0, Z = 100 } end
    M.Methods.SetSphereRadius = function() end
    local me = M.new_obj("Willie_BP_C", "Willie_BP_C_ME"); rawset(me, "__addr", 7001)
    me.__props.Health = 100
    -- our pawn away from the peers' poses: the fallback never spawns a body
    -- within 250 uu of our own pawn (PX.spawn_spot)
    rawset(me, "loc", { X = -2000, Y = 0, Z = 100 })
    M.pc.__props.Pawn = me
    local bodies, calls, nb = { me }, {}, 0
    local lm = M.new_obj("BP_LevelManager_C", "LM")
    lm.__props["Spawn Combatants"] = function(self)
        local n = self.__props["Amount of Characters to Spawn"] or 0
        calls[#calls + 1] = n
        for _ = 1, n do
            nb = nb + 1
            local w = M.new_obj("Willie_BP_C", "Willie_BP_C_" .. (20 + nb)); rawset(w, "__addr", 7100 + nb)
            w.__props.Health = 100
            rawset(w, "loc", { X = 1000 + 200 * nb, Y = 0, Z = 100 })
            bodies[#bodies + 1] = w
        end
    end
    local sps = { M.new_obj("BP_SpawnerPoint_Willies_C", "SP1"), M.new_obj("BP_SpawnerPoint_Willies_C", "SP2") }
    for _, sp in ipairs(sps) do sp.__props.Sphere = M.new_obj("SphereComponent", "S") end
    _G.FindAllOf = function(c)
        if c == "Willie_BP_C" then return bodies end
        if c == "BP_LevelManager_C" then return { lm } end
        if c == "BP_SpawnerPoint_Willies_C" then return sps end
        return nil
    end
    local tick, peers = 0, { 2, 3 }
    local function director(state) HSMP_IPC.bus_put("director", { state = state }) end
    local function beat()
        tick = tick + 1
        local ps = { { 1, "Me" } }
        for _, id in ipairs(peers) do ps[#ps + 1] = { id, "P" .. id } end
        sidecar(ps)
        for _, id in ipairs(peers) do
            peer_root(id, 10 + tick, 100.0 * id, 50.0 * id, 90.0)
        end
    end
    director("Spawn")
    for _ = 1, 36 do beat(); M.run(250) end   -- 9 s: past the 2 s settle and the 6 s fallback grace
    local P = api.puppets()
    T.check(calls[1] == 2, "the first SpawnCombatants call asks for BOTH missing bodies", T.repr(calls))
    T.check(P[2] and P[3] and P[2].actor ~= P[3].actor, "both opponents get a stand-in", T.repr({ calls, P[2] and P[2].addr, P[3] and P[3].addr }))
    local sl1, sl2 = rawget(sps[1], "loc"), rawget(sps[2], "loc")
    T.check(sl1 and sl2 and (sl1.X ~= sl2.X or sl1.Y ~= sl2.Y), "the spawners go to distinct peer poses (no stacked bodies)", T.repr({ sl1, sl2 }))
    -- the round is Live; a fourth player joins in progress and needs a body
    director("Live")
    peers = { 2, 3, 4 }
    local n0 = #calls
    for _ = 1, 48 do beat(); M.run(250) end   -- 12 s
    P = api.puppets()
    T.check(#calls > n0 and calls[#calls] >= 1, "the fallback is allowed while the round is Live", T.repr(calls))
    T.check(P[4] ~= nil, "the join-in-progress peer gets a stand-in in Live")
    T.check(not T.contains(M.logtext(), "LOOP ERR"), "no loop errors", M.logtext())
end

if opts.kind == "spectate" then
    -- After my death only the camera moves (HSMPMatch .spectate.json):
    -- stand-in range is measured from the camera, the spectated stand-in is
    -- always driven, and there is 10% hysteresis at the edge.
    local api = boot(true)
    M.Methods.K2_GetActorLocation = function(self) return rawget(self, "loc") or { X = 0, Y = 0, Z = 100 } end
    local cam = { loc = { X = 0, Y = 0, Z = 200 } }
    local cm = M.new_obj("PlayerCameraManager", "PCM")
    rawset(cm, "GetCameraLocation", function() return cam.loc end)
    M.pc.__props.PlayerCameraManager = cm
    local me = M.new_obj("Willie_BP_C", "Willie_BP_C_ME"); rawset(me, "__addr", 7001)
    me.__props.Health = 100
    M.pc.__props.Pawn = me
    local si = M.new_obj("Willie_BP_C", "Willie_BP_C_5"); rawset(si, "__addr", 7005)
    si.__props.Health = 100
    rawset(si, "loc", { X = 6000, Y = 0, Z = 100 })
    local tick = 0
    local function beat()
        tick = tick + 1
        sidecar({ { 1, "Me" }, { 2, "B" } })
        peer_root(2, 10 + tick, 6000.0, 0.0, 90.0)
        HSMPNative.sc_peer_play(2, { peer_id = 2, seq = tick, v = 2, pt = 0, mode = "interp", cut = 0, root = { 6000.0, 0.0, 100.0, 0.0 }, m = 0, B = {} })
    end
    HSMP_IPC.bus_put("spectate", { target = 0 })
    beat(); M.run(300)
    for _ = 1, 8 do beat(); M.run(250) end
    api.set_puppet(2, { actor = si, nick = "B", claimed_at = 0, gen = 0, addr = 7005, driving = true, in_range = true })
    cam.loc = { X = 5800, Y = 0, Z = 200 }   -- I am dead at the origin; the camera spectates the far fight
    for _ = 1, 3 do beat(); M.run(250) end
    T.check(api.puppets()[2].in_range == true and not T.contains(M.logtext(), "out of range"),
        "range from the camera, not from my corpse", M.logtext())
    cam.loc = { X = 1700, Y = 0, Z = 200 }   -- 4300 uu: inside the 10% hysteresis band
    for _ = 1, 3 do beat(); M.run(250) end
    T.check(api.puppets()[2].in_range == true, "no release inside the hysteresis band")
    cam.loc = { X = 0, Y = 0, Z = 200 }
    for _ = 1, 3 do beat(); M.run(250) end
    T.check(api.puppets()[2].in_range == false, "far from the camera: released")
    HSMP_IPC.bus_put("spectate", { target = 2, nick = "B", round = 1, alive = 1 })
    for _ = 1, 3 do beat(); M.run(250) end
    T.check(api.puppets()[2].in_range == true, "the spectated stand-in is always driven")
    T.check(not T.contains(M.logtext(), "LOOP ERR"), "no loop errors")
end

if opts.kind == "native_servo" then
    -- Native servo: the A/B switch (settings "native_servo"), the native body loop while it
    -- answers, the Lua loop on a refusal, a 5 s pause after a module-level refusal.
    -- (HSMPNative.servo_bodies is the mock's test hook here; the native side has its own tests.)
    T.write(sd .. "/.settings.json", '{"avatars":true,"native_servo":true}\n')
    local api = boot(true)
    local N = rawget(_G, "HSMPNative")
    local seen = {}
    local function ok_hook(a, out)
        seen[#seen + 1] = { mesh = a.mesh, leg_from = a.leg_from, holding = a.holding, gain = a.gain, leg_gain = a.leg_gain }
        out.c, out.v, out.dl, out.gl = { 1, 2, 3, 0, 0, 0, 1 }, { 4, 5, 6, 7, 8, 9 }, { 0.5 }, { 0.25 }
        return 22
    end
    N._servo = ok_hook
    local mesh = M.new_obj("SkeletalMeshComponent", "CharacterMesh0"); rawset(mesh, "__addr", 9001)
    local sv = { com = { { 0, 0, 0 } } }
    local PX = api.PX
    local out = PX.ns_bodies(mesh, {}, sv, 1 / 60, 900, 900, 0.8, nil, true)
    T.check(out and out.v[1] == 4 and seen[1] and seen[1].mesh == 9001 and seen[1].leg_from == 18 and seen[1].holding == true,
        "settings native_servo=true: one native call drives the body loop", T.repr(seen))
    local y = { gain = 0.15 }
    PX.ns_bodies(mesh, {}, sv, 1 / 60, 150, 150, 0.15, y, false)
    T.check(seen[2] and seen[2].leg_gain == 0.15, "yielding: legs use the yield gain too (same as the Lua loop)", T.repr(seen[2]))
    N._servo = function() return nil, "skip:call" end
    T.check(PX.ns_bodies(mesh, {}, sv, 1 / 60, 900, 900, 0.8, nil, false) == nil, "a refused frame -> nil: the Lua loop drives it")
    N._servo = ok_hook
    T.check(PX.ns_bodies(mesh, {}, sv, 1 / 60, 900, 900, 0.8, nil, false) ~= nil, "a per-frame refusal does not pause")
    N._servo = function() return nil, "disabled:/Script/Engine.X: Soft* param refused" end
    PX.ns_bodies(mesh, {}, sv, 1 / 60, 900, 900, 0.8, nil, false)
    N._servo = ok_hook
    local n0 = #seen
    T.check(PX.ns_bodies(mesh, {}, sv, 1 / 60, 900, 900, 0.8, nil, false) == nil and #seen == n0, "a module-level refusal pauses native attempts")
    M.run(5200)
    T.check(PX.ns_bodies(mesh, {}, sv, 1 / 60, 900, 900, 0.8, nil, false) ~= nil, "and retries after 5 s")
    T.write(sd .. "/.settings.json", '{"avatars":true,"native_servo":false}\n')
    M.run(1500)
    n0 = #seen
    T.check(PX.ns_bodies(mesh, {}, sv, 1 / 60, 900, 900, 0.8, nil, false) == nil and #seen == n0, "settings native_servo=false: Lua loop only")
    local log = M.logtext()
    T.check(T.count(log, "native servo ON") == 1 and T.count(log, "native servo off") == 1, "both switches logged once", log:sub(1, 300))
    T.check(T.count(log, "native servo refused (disabled:") == 1, "the refusal is logged once", log:sub(1, 300))
end

if opts.kind == "native_neutralise" then
    -- Native neutralise: the A/B switch (settings "native_neutralise", separate from native_servo), the native
    -- variable writes while they answer, the tonus originals kept by Lua, the Lua path on a refusal.
    T.write(sd .. "/.settings.json", '{"avatars":true,"native_neutralise":true,"native_servo":false}\n')
    local api = boot(true)
    local N = rawget(_G, "HSMPNative")
    local seen = {}
    N._neutralise = function(a)
        seen[#seen + 1] = { actor = a.actor, tonus = a.tonus, motors_off = a.motors_off, mesh = a.mesh }
        return 30
    end
    local actor = M.new_obj("Willie_BP_C", "Willie_BP_C_5"); rawset(actor, "__addr", 7005)
    actor.__props["All Body Tonus"] = 100
    actor.__props["Muscle Power"] = 35
    local mesh = M.new_obj("SkeletalMeshComponent", "CharacterMesh0"); rawset(mesh, "__addr", 9001)
    local p, body = { actor = actor }, { mesh = mesh }
    local PX = api.PX
    T.check(PX.nn(p, body) == true and seen[1] and seen[1].actor == 7005 and seen[1].tonus == true and seen[1].motors_off == true
        and seen[1].mesh == 9001, "settings native_neutralise=true: one native call", T.repr(seen))
    T.check(p.tonus0 and p.tonus0["All Body Tonus"] == 100 and p.tonus0["Muscle Power"] == 35,
        "the tonus originals are read once by Lua (restored on release)", T.repr(p.tonus0))
    T.check(actor.__props["All Body Tonus"] == 100, "the native path does the writes (not Lua)")
    T.check(PX.ns_bodies(mesh, {}, { com = {} }, 1 / 60, 900, 900, 0.8, nil, false) == nil, "G2a stays off: its own switch")
    N._neutralise = function() return nil, "skip:actor" end
    T.check(PX.nn(p, body) == false, "a refused frame -> false: neutralise_bp writes from Lua")
    T.write(sd .. "/.settings.json", '{"avatars":true,"native_neutralise":false}\n')
    N._neutralise = function(a) seen[#seen + 1] = a; return 30 end
    M.run(1500)
    local n0 = #seen
    T.check(PX.nn(p, body) == false and #seen == n0, "settings native_neutralise=false: Lua path only")
    local log = M.logtext()
    T.check(T.count(log, "native neutralise ON") == 1 and T.count(log, "native neutralise off") == 1, "both switches logged", log:sub(1, 300))
end

if opts.kind == "native_wservo" then
    -- Native weapon servo: its own A/B switch; the validated weapon entry's addresses go to the native
    -- call (no extra reflected call); an entry without a root address stays on the Lua path.
    T.write(sd .. "/.settings.json", '{"avatars":true,"native_wservo":true,"native_servo":false}\n')
    local api = boot(true)
    local N = rawget(_G, "HSMPNative")
    local seen = {}
    N._servo = function() return 22 end
    N._servo_weapon = function(a, out)
        seen[#seen + 1] = { actor = a.actor, root = a.root, gain = a.gain, holding = a.holding, n = #a.aim }
        out.x, out.v = { 1, 2, 3, 0, 0, 0, 1 }, { 4, 5, 6 }
        return true
    end
    local PX = api.PX
    local c = { addr = 8001, root_addr = 8002, com = { 0, 0, 0 } }
    local tg = { 1, 2, 3, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0 }
    local out = PX.nw_weapon(c, tg, 1 / 60, false)
    T.check(out and out.x[1] == 1 and seen[1] and seen[1].actor == 8001 and seen[1].root == 8002 and seen[1].n == 13,
        "settings native_wservo=true: the validated entry's addresses go native", T.repr(seen))
    T.check(PX.nw_weapon({ addr = 8001, com = { 0, 0, 0 } }, tg, 1 / 60, false) == nil, "no root address: Lua path")
    T.check(PX.ns_bodies(M.new_obj("SkeletalMeshComponent", "M"), {}, { com = {} }, 1 / 60, 900, 900, 0.8, nil, false) == nil,
        "G2a stays off: its own switch")
    N._servo_weapon = function() return nil, "skip:root" end
    T.check(PX.nw_weapon(c, tg, 1 / 60, true) == nil, "a refused frame -> nil: the Lua servo")
    T.check(T.count(M.logtext(), "native weapon servo ON") == 1, "switch logged")
end

if opts.kind == "wpn_status" then
    -- The stand-in weapon status string is cached on the weapon entry (refreshed at most once a
    -- second; a new entry = a new weapon starts fresh; a freed weapon is never touched).
    local api = boot(true)
    local calls = 0
    M.Methods.GetClass = function(self) calls = calls + 1; return M.new_obj("Class", "ModularWeaponBP_ArmingSword_C") end
    M.Methods.GetAttachParent = function() return nil end
    local wa = M.new_obj("ModularWeaponBP_ArmingSword_C", "W_1")
    local c = { root = M.new_obj("StaticMeshComponent", "W_1_root") }
    local PX = api.PX
    local s1 = PX.wpn_status(c, wa, 1000)
    local s2 = PX.wpn_status(c, wa, 1500)
    T.check(s1 == s2 and calls == 1, "cached within a second: one GetClass for two frames", T.repr({ s1, s2, calls }))
    PX.wpn_status(c, wa, 2100)
    T.check(calls == 2, "refreshed after a second", calls)
    local c2 = { root = M.new_obj("StaticMeshComponent", "W_2_root") }
    rawset(wa, "__dead", true)   -- the old weapon is freed (strict mock: touching it fails the suite)
    local w2 = M.new_obj("ModularWeaponBP_ArmingSword_C", "W_2")
    PX.wpn_status(c2, w2, 2200)
    T.check(calls == 3, "a new entry (weapon change) reads the new weapon at once", calls)
    T.check(PX.wpn_status(nil, nil, 2300) == "-", "no weapon: no call")
    T.check(#M.dead_touch == 0, "the freed weapon was never touched", T.repr(M.dead_touch))
end

if opts.kind == "skeleton_ref" then
    -- Parent offsets measured on a stretched stand-in are replaced by the reference
    -- skeleton (scaled to the character); the stretch measure sees a pulled joint.
    local api = boot(true)
    local P = api.PURE
    local k = 1.06
    local loc = {}
    for i = 2, #P.V2_REF_T do local r = P.V2_REF_T[i]; loc[i] = { r[1] * k, r[2] * k, r[3] * k } end
    local out, fixed, ks = P.ref_loc(loc)
    T.check(fixed == 0 and math.abs(ks - k) < 1e-6, "an intact body keeps its own offsets", T.repr({ fixed, ks }))
    loc[17] = { 60, 0, 0 }   -- hand_r 60 uu from the forearm (reference 27.25 x k)
    loc[19] = nil            -- calf_l not measured
    out, fixed = P.ref_loc(loc)
    T.check(fixed == 2 and math.abs(P.len3(out[17]) - 27.25 * k) < 1e-3 and out[19] ~= nil,
        "a stretched and a missing offset come from the reference", T.repr({ fixed, out[17], out[19] }))
    local neck = P.V2_REF_T[7]
    loc[7] = { P.len3(neck) * k, 0, 0 }
    out, fixed = P.ref_loc(loc)
    T.check(fixed == 3 and P.d3(out[7], { neck[1] * k, neck[2] * k, neck[3] * k }) < 1e-6,
        "an equal-length neck displaced sideways is not cached as the reference joint", T.repr(out[7]))
    -- a pose built from the reference, then one joint pulled 12 uu
    local pos, len = { { 0, 0, 100 } }, {}
    for i = 2, #P.V2_REF_T do
        local par, r = pos[P.V2_PARENT[i]], P.V2_REF_T[i]
        pos[i] = { par[1] + r[1], par[2] + r[2], par[3] + r[3] }
        len[i] = P.len3(r)
    end
    local d0 = P.stretch(pos, len)
    T.check(d0 < 1e-6, "no stretch on the reference pose", d0)
    pos[13] = { pos[13][1] + 12, pos[13][2], pos[13][3] }   -- hand_l pulled off the forearm
    local d, b = P.stretch(pos, len)
    T.check(d > 10 and (b == 13 or b == 12), "a pulled joint is measured", T.repr({ d, b }))
end

if opts.kind == "body_scale" then
    local api = boot(true)
    local n = HSMPNative
    local scale, calls, asset_rebuilds = { X=1, Y=1, Z=1 }, {}, 0
    local mesh = {
        K2_GetComponentScale = function() return scale end,
        SetSimulatePhysics = function(_, on) calls[#calls+1] = on and "on" or "off" end,
        SetWorldScale3D = function(_, s) scale = s; calls[#calls+1] = "scale" end,
        SetPhysicsAsset = function() asset_rebuilds = asset_rebuilds + 1 end,
    }
    local p = { nick="peer", aim={}, shown={}, qhist={}, idlew={}, qfoot={} }
    local body = { mesh=mesh, sv={} }
    local pose = { has_context=true,match_id=10,round=1,life=1 }
    n.sc_put("peer_body2", { version=1, match_id=10,round=1,life=1,pawn="Owner",native_height=.5,actor_scale={1,1,1},
        height_rate=1, muscle_rate=0, mass_scale_bp=1,
        char_scale={ 1.1, 0.95, 1.2 }, rows={} }, 2)
    HSMP_IPC.peer_dir(true)
    local stale_pose = { has_context=true,match_id=10,round=1,life=2 }
    T.check(not api.PX.sync_body_scale(2,p,body,999,stale_pose) and #calls == 0,
        "an old body generation cannot resize the fresh native life")
    T.check(not api.PX.sync_body_scale(2,p,body,999) and #calls == 0,
        "no body geometry is applied before an authenticated pose is displayed")
    T.check(api.PX.sync_body_scale(2, p, body, 1000, pose), "owner mesh scale starts a coordinated geometry reset")
    T.check(T.eq(calls, { "off", "scale" }) and body.repose and body.scale_remeasure,
        "physics stops before resizing, then geometry is marked for measurement", T.repr(calls))
    T.check(asset_rebuilds == 0,
        "scale sync retains native bodies and their active physics-control bindings")
    T.check(body.sv == nil and p.aim == nil and p.shown == nil and p.qhist == nil,
        "old COM/joint offsets and prior pose aims are discarded after resizing")
    T.check(math.abs(scale.X-1.1)<1e-6 and math.abs(scale.Y-0.95)<1e-6 and math.abs(scale.Z-1.2)<1e-6,
        "all three owner scale axes are preserved")
    T.check(not api.PX.sync_body_scale(2, p, body, 1017, pose) and #calls == 2,
        "unchanged geometry does not reset physics every frame")
    n.sc_put("peer_body2", { version=2, match_id=10,round=1,life=1,pawn="Owner",native_height=.5,actor_scale={1,1,1},
        height_rate=1, muscle_rate=0, mass_scale_bp=1,
        char_scale={ 1, 1, 1 }, rows={} }, 2)
    body.sv, p.aim = {}, {}
    T.check(api.PX.sync_body_scale(2, p, body, 2000, pose) and body.sv == nil and p.aim == nil and #calls == 4,
        "a later owner geometry change invalidates a running servo too")
end

if opts.kind == "fist_grip" then
    local api = boot(true)
    local states = {}
    local grip = {
        IsValid=function() return true end, GetAddress=function() return 501 end,
        ConstraintInstance={ ConstraintBone2={ToString=function() return "hand_r" end},
            ProfileInstance={AngularDrive={SlerpDrive={Stiffness=10,Damping=2,MaxForce=100}},
                LinearLimit={XMotion=2,YMotion=2,ZMotion=2,Limit=1},
                ConeLimit={Swing1Motion=2,Swing1LimitDegrees=10,Swing2Motion=2,Swing2LimitDegrees=20},
                TwistLimit={TwistMotion=2,TwistLimitDegrees=30}} },
    }
    for _, k in ipairs({"LinearX","LinearY","LinearZ","AngularSwing1","AngularSwing2","AngularTwist"}) do
        grip["Set"..k.."Limit"] = function(_, mode, value) states[k]={mode,value} end
    end
    grip.SetAngularDriveParams = function() end
    local function weapon(cls)
        return {IsValid=function() return true end,
            GetClass=function() return {GetFName=function() return {ToString=function() return cls end} end} end}
    end
    local actor={ ["Weapon R"]=weapon("Weapon_Fists_C"), K2_GetComponentsByClass=function() return {grip} end }
    local p={actor=actor}
    local list=api.PX.grip_constraints(p,1000)
    T.check(list[1].freed and states.LinearX[1]==0 and states.AngularTwist[1]==0,
        "fist grip is freed even without a weapon servo target")
    actor["Weapon R"]=weapon("ModularWeaponBP_Polearm_C")
    list=api.PX.grip_constraints(p,2001)
    T.check(not list[1].freed and states.LinearX[1]==2 and states.AngularTwist[2]==30,
        "an unserved real weapon regains its original grip limits")
    actor["Weapon R"]=weapon("Weapon_Fists_C")
    api.PX.grip_constraints(p,3002)
    actor["Weapon R"]=nil
    list=api.PX.grip_constraints(p,4003)
    T.check(not list[1].freed and states.LinearX[1]==2,"an empty hand regains original grip limits")
    actor["Weapon R"]=weapon("Weapon_Fists_C")
    api.PX.grip_constraints(p,5004)
    api.PX.grips_off(p,false)
    T.check(not list[1].freed and states.LinearX[1]==2 and states.AngularTwist[1]==2,
        "ending the stand-in drive restores its original fist grip")
end
