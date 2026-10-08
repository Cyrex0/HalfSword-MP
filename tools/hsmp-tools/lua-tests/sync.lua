-- HSMPSync runtime tests, offline under umg_mock (strict: an
-- IsValid on a freed object is a crash).
--
--     hsmp-tools lua-test sync
--
--   * no root / weapon / pose streaming and no tick file I/O in
--     single-player; streaming only while the MP session is live
--     (shared/hsmp_session.lua); t.IdleWhenNotForeground set only while live
--     and restored after; no `dir /b` (io.popen) peer listing.
--   * one PlayerController walk per game frame (shared WG.pc()).
--   * a weapon destroyed and replaced at the same address is never
--     touched again (no component cache keyed by the address).
--   * a pawn without the codec-v2 bones is logged once, not per sample.
-- Every case runs in its own Lua state (T.isolated).

local mode, opts = ...
local SY = T.path("mods/HSMPSync/Scripts/main.lua")

if mode ~= "case" then
    T.isolated(T.script, "case", { kind = "sp" })
    T.isolated(T.script, "case", { kind = "mp" })
    T.isolated(T.script, "case", { kind = "walks" })
    T.isolated(T.script, "case", { kind = "weapon_swap" })
    T.isolated(T.script, "case", { kind = "bad_mesh" })
    T.isolated(T.script, "case", { kind = "died_round" })
    T.isolated(T.script, "case", { kind = "native_dead" })
    T.isolated(T.script, "case", { kind = "tdiag" })
    T.isolated(T.script, "case", { kind = "native_sample" })
    T.isolated(T.script, "case", { kind = "prepare_stream" })
    return
end

local M = require("umg_mock")
local sd = T.tmpdir("hsmp_sync_sd_")
local la = T.tmpdir("hsmp_sync_la_")
local popen_n = 0

local V0 = { X = 0, Y = 0, Z = 0 }
local function boot(o)
    o = o or {}
    M.install({ state_dir = sd, env = { LOCALAPPDATA = la, HSMP_INST = "7", HSMP_POSE_PROBE = false }, strict = true })
    local arena = "World /Game/Maps/Arenas/Map_Arena_Pit/Map_Arena_Pit.Map_Arena_Pit"
    local Mt = M.Methods
    Mt.GetFullName = function(self)
        if rawget(self, "__cls") == "World" then return arena end
        return "Obj " .. tostring(rawget(self, "__name"))
    end
    Mt.GetAddress = function(self) return rawget(self, "__addr") or 4096 end
    Mt.GetWorld = function() return M.world end
    Mt.K2_GetActorLocation = function() return { X = 10, Y = 20, Z = 30 } end
    Mt.K2_GetActorRotation = function() return { Pitch = 0, Yaw = 90, Roll = 0 } end
    Mt.GetVelocity = function() return V0 end
    Mt.GetRealTimeSeconds = function() return M.now / 1000 end
    Mt.GetBoneIndex = function() return o.bad_mesh and -1 or 1 end
    Mt.IsSimulatingPhysics = function() return true end
    Mt.GetSocketTransform = function() return { Translation = { X = 1, Y = 2, Z = 3 }, Rotation = { X = 0, Y = 0, Z = 0, W = 1 } } end
    Mt.GetTransform = function() return { Translation = { X = 1, Y = 2, Z = 3 }, Rotation = { X = 0, Y = 0, Z = 0, W = 1 } } end
    Mt.GetSocketLocation = function() return { X = 1, Y = 2, Z = 3 } end
    Mt.GetPhysicsLinearVelocityAtPoint = function() return V0 end
    Mt.GetPhysicsAngularVelocityInDegrees = function() return V0 end
    Mt.K2_GetComponentLocation = function() return { X = 1, Y = 2, Z = 50 } end
    Mt.ExecuteConsoleCommand = function(self, ctx, cmd) table.insert(M.execs, "cvar " .. tostring(cmd)) end
    Mt.GetConsoleVariableIntValue = function() return 1 end
    local sfo = _G.StaticFindObject
    _G.StaticFindObject = function(path)
        if path == "/Script/Engine.Default__KismetSystemLibrary" then return M.ksl end
        return sfo(path)
    end
    _G.RegisterHook = function() return 1, 2 end
    io.popen = function() popen_n = popen_n + 1; return nil end
    -- the local pawn with its body mesh and a sword in the right hand
    local pawn = M.new_obj("Willie_BP_C", "Willie_BP_C_3")
    pawn.__props.Health = 100
    pawn.__props.Mesh = M.new_obj("SkeletalMeshComponent", "CharacterMesh0")
    M.pc.__props.Pawn = pawn
    M.pawn = pawn
    dofile(SY)
    return pawn
end

local function weapon(name, addr)
    local w = M.new_obj("ModularWeaponBP_ArmingSword_C", name)
    rawset(w, "__addr", addr)
    w.__props.RootComponent = M.new_obj("StaticMeshComponent", name .. "_root")
    w.__props["Root Scene"] = M.new_obj("SceneComponent", name .. "_base")
    w.__props.TippyTipScene = M.new_obj("SceneComponent", name .. "_tip")
    return w
end
local function kill(o) rawset(o, "__dead", true) end

-- The sidecar: its `link` record (typed) and its header heartbeat (hb_age).
local beats = 0
local function beat()
    beats = beats + 1
    local N = _G.HSMPNative
    if beats == 1 then N.sc_put("link", { status = 1, state = 1, my_peer_id = 1 }) end   -- CONNECTED, UP
    if opts.kind ~= "died_round" and opts.kind ~= "native_dead" and opts.kind ~= "prepare_stream" then
        if beats == 1 then
            N.sc_put("session",{epoch=1,seq=1,match_id=71,phase=3,round=1,winner_seat=255,
                config={arena="Map_Arena_Pit"},rows={{peer_id=1,seat=1,connected=true,alive=true,spawn_id=256,spawn_pos={10,20,30}}}})
            N.sc_put("mode",{seq=1,match_id=71,round=1,rows={{peer_id=1,seat=1,life=1,alive=true}}})
        end
        -- The sampling harness supplies placement evidence; placement itself is
        -- covered separately by spawn_place's physical verification tests.
        N.bus_put("spawn_status",{match_id=71,round=1,life=1,spawn_id=256,pawn="Willie_BP_C_3",verified=true})
    end
    N._st.hb_age = 0.05
end
local function stop_beating() _G.HSMPNative._st.hb_age = 1e9 end
-- run ms with the sidecar beating every 250 ms (or not)
local function run(ms, live)
    local n = math.floor(ms / 250)
    for _ = 1, n do
        if live then beat() end
        M.run(250)
    end
end
-- The pose hot paths of the native mock (lib/hsmp_native_pose.lua): record puts per slot and
-- the last put_pose arguments.
local function hot() local N = rawget(_G, "HSMPNative"); return N and N._hot end
local function puts(slot) local h = hot(); return h and h.n[slot] or 0 end
local function streamed()
    return puts("local_root") + puts("local_weapon") + puts("local_pose") > 0
end
local function last_w1() local h = hot(); return h and h.pose and type(h.pose.w) == "table" and h.pose.w[1] or nil end
local function cvar_cmds()
    return T.filter(M.execs, function(e) return e:find("IdleWhenNotForeground", 1, true) ~= nil end)
end

if opts.kind == "sp" then
    local pawn = boot()
    pawn.__props["Weapon R"] = weapon("W_1", 5001)
    run(3000, false)
    T.check(not streamed(), "single-player arena: no root / weapon / pose record written",
        T.repr(hot() and hot().n))
    T.check(#cvar_cmds() == 0, "single-player: t.IdleWhenNotForeground never touched", T.repr(M.execs))
    T.check(popen_n == 0, "no io.popen (dir /b) peer listing", popen_n)
    T.check(#M.dead_touch == 0, "nothing freed touched", T.repr(M.dead_touch))

elseif opts.kind == "mp" then
    local pawn = boot()
    pawn.__props["Weapon R"] = weapon("W_1", 5001)
    run(1000, true) -- initial world drop has no verified original placement yet
    local initial_weapon_puts=puts("local_weapon")
    run(2500, true)
    T.check(puts("local_pose") > 0 and puts("local_root") > 0, "live MP session: root + pose streamed", T.repr(hot() and hot().n))
    T.check(last_w1() == 1, "the held sword is sampled", T.repr(hot() and hot().pose and hot().pose.w))
    local c = cvar_cmds()
    T.check(#c == 1 and c[1]:find("IdleWhenNotForeground 0", 1, true), "background tick set once while live", T.repr(c))
    T.check(popen_n == 0, "no io.popen (dir /b) peer listing", popen_n)
    -- the sidecar dies: its link still says "connected" but the heartbeat stops -> not live
    stop_beating()
    run(7000, false)
    c = cvar_cmds()
    T.check(#c == 2 and c[2]:find("IdleWhenNotForeground 1", 1, true), "session not live any more: the previous value (1) restored",
        T.repr(c))
    local n0 = puts("local_pose")
    run(1000, false)
    T.check(puts("local_pose") == n0, "and streaming stops", puts("local_pose") - n0)
    T.check(#M.dead_touch == 0, "nothing freed touched", T.repr(M.dead_touch))

elseif opts.kind == "walks" then
    boot()
    run(2000, true)
    local UEH = require("UEHelpers")
    local gp, gw, walks = UEH.GetPlayerController, UEH.GetWorld, 0
    UEH.GetPlayerController = function() walks = walks + 1; return gp() end
    UEH.GetWorld = function() walks = walks + 1; return gw() end
    run(1000, true)
    -- callback frames per second: fast_send samples at 60 Hz, on_tick at 30 Hz
    T.check(walks <= 95, "<= one PlayerController walk per callback frame", walks)

elseif opts.kind == "weapon_swap" then
    local pawn = boot()
    local w1 = weapon("W_1", 5001)
    pawn.__props["Weapon R"] = w1
    run(2000, true)
    -- a re-arm / disarm destroys the sword; a new one lands at the SAME address
    kill(w1); kill(w1.__props.RootComponent); kill(w1.__props["Root Scene"]); kill(w1.__props.TippyTipScene)
    local w2 = weapon("W_2", 5001)
    pawn.__props["Weapon R"] = w2
    local n0 = puts("local_pose")
    run(1000, true)
    T.check(#M.dead_touch == 0, "the destroyed weapon's components are never touched again", T.repr(M.dead_touch))
    T.check(puts("local_pose") > n0 and last_w1() == 1, "the new sword is sampled", T.repr(hot() and hot().pose and hot().pose.w))

elseif opts.kind == "bad_mesh" then
    boot({ bad_mesh = true })
    run(2000, true)
    local n = T.count(M.logtext(), "codec-v2 bones")
    T.check(n == 1, "a pawn without the codec-v2 bones is logged once, not per sample", n)

elseif opts.kind == "died_round" or opts.kind == "native_dead" then
    -- A death is reported for the round it happened in; a pawn that
    -- stays dead into a same-world round change never reports the new round.
    local pawn = boot()
    local function life(match_id,round,generation,seq)
        HSMPNative.sc_put("session",{epoch=1,seq=seq,match_id=match_id,phase=3,round=round,winner_seat=255,
            rows={{peer_id=1,seat=1,connected=true,alive=true,spawn_id=round*256,spawn_pos={10,20,30}}}})
        HSMPNative.sc_put("mode",{seq=seq,match_id=match_id,round=round,rows={{peer_id=1,seat=1,life=generation,alive=true}}})
        HSMPNative.bus_put("spawn_status",{verified=true,pawn="Willie_BP_C_3",match_id=match_id,round=round,life=generation,spawn_id=round*256})
    end
    run(500,true) -- initial world transition clears old placement evidence
    life(81,2,1,1)
    run(2500,true)
    if opts.kind == "native_dead" then
        HSMPNative.sc_rec_drain()
        pawn.__props.Health=100
        pawn.__props.Consciousness=0
        pawn.__props.Downed=true
        pawn.__props.Fallen=true
        pawn.__props["Force Death"]=true
        run(1500,true)
        T.check(not T.any(HSMPNative.sc_rec_drain(),function(m)return m.kind=="death_report" end),
            "KO and pending ForceDeath hints never create biological death")
        pawn.__props.DED=true
    else pawn.__props.Health = 0 end
    run(2500, true)
    -- the typed G2S `death_report` record
    local SENT = {}
    local function reports()
        for _, m in ipairs(HSMPNative.sc_rec_drain()) do SENT[#SENT + 1] = m end
        local rounds = {}
        for _, m in ipairs(SENT) do if m.kind == "death_report" then rounds[#rounds + 1] = m.data.round end end
        return rounds
    end
    local r = reports()
    T.check(#r >= 2 and T.all(r, function(x) return x == 2 end), "death_report round 2 sent (and resent) while dead in round 2", T.repr(r))
    if opts.kind=="native_dead" then
        T.check(pawn.__props.Health==100,"native DED fallback reports without fabricating HP damage")
    end
    local n=#r
    T.check(T.all(SENT,function(m)return m.kind~="death_report" or (m.data.match_id==81 and m.data.life==1) end),
        "Sync fallback carries the original native pawn's full death context")
    life(81,2,2,2);run(2000,true)
    T.check(#reports()==n,"same-round new life cannot relabel a latched old death")
    life(82,2,1,3);run(2000,true)
    T.check(#reports()==n,"new match reusing round/life cannot relabel old death")
    local ob = T.read(sd .. "/.control.outbox.jsonl") or ""
    T.check(ob:find("died:", 1, true) == nil, "no control-outbox died verb any more", ob)
    _G.HSMPNative.sc_put("session", { epoch = 1, seq = 2, phase = 3, round = 3, winner_seat = 255 })   -- LIVE round 3
    run(4000, true)
    r = reports()
    T.check(not T.any(r, function(x) return x == 3 end), "still dead in round 3 of the same world: no death_report for round 3", T.repr(r))

elseif opts.kind == "tdiag" then
    -- Timing diagnostics are toggled by dev_cmd TDIAG records (hsmp-tools ipc-ctl tdiag on|off),
    -- not by a .tdiag_on file.
    boot()
    local N = HSMPNative
    T.write(sd .. "/.tdiag_on", "")
    run(2000, true)
    T.check(not T.contains(M.logtext(), "tdiag on"), "a .tdiag_on file does nothing", M.logtext())
    N.sc_dev({ id = 1, op = 2, key = "tdiag", num = 1 })   -- TUNE (HSMPAvatars'): ignored here
    N.sc_dev({ id = 2, op = 1, key = "tdiag", num = 1 })   -- AUTOTEST: ignored here
    run(500, true)
    T.check(not T.contains(M.logtext(), "tdiag on"), "records of other ops are ignored", M.logtext())
    N.sc_dev({ id = 3, op = 3, key = "tdiag", num = 1 })
    run(500, true)
    T.check(T.contains(M.logtext(), "tdiag on (dev_cmd #3)"), "TDIAG on switches the diagnostics on", M.logtext())
    N.sc_dev({ id = 4, op = 3, key = "tdiag", num = 1 })
    N.sc_dev({ id = 5, op = 3, key = "tdiag", num = 0 })
    run(500, true)
    T.check(T.count(M.logtext(), "tdiag on") == 1 and T.contains(M.logtext(), "tdiag off (dev_cmd #5)"),
        "a repeated on is not re-logged; TDIAG off switches them off", M.logtext())
    T.check(#M.dead_touch == 0, "nothing freed touched", T.repr(M.dead_touch))
elseif opts.kind == "prepare_stream" then
    local pawn=boot()
    local root
    local put_root=HSMPNative.put_root
    HSMPNative.put_root=function(...)
        root=select(12,...)
        return put_root(...)
    end
    run(500,true)
    local function assignment(generation,placed,mode_id,name,sid)
        HSMPNative.sc_put("session",{epoch=1,seq=generation,match_id=91,phase=3,round=1,winner_seat=255,
            rows={{peer_id=1,seat=1,connected=true,alive=false,spawn_id=sid,spawn_pos={10,20,30}}}})
        HSMPNative.sc_put("mode",{seq=generation,mode=mode_id,match_id=91,round=1,
            rows={{peer_id=1,seat=1,life=generation,alive=false,respawning=true}}})
        HSMPNative.bus_put("spawn_status",{verified=placed,pawn=name,match_id=91,round=1,life=generation,spawn_id=sid})
    end
    assignment(2,true,6,"Willie_BP_C_3",386)
    run(1500,true)
    T.check(puts("local_root")>0 and puts("local_pose")>0,
        "actual Sync sends verified life2 root and pose before LOADED while server still respawning")
    T.check(root and root.match_id==91 and root.round==1 and root.life==2,
        "actual root carries full original respawn context",T.repr(root))
    local n=puts("local_root")
    pawn.__props.DED=true;HSMPNative.sc_rec_drain();run(1500,true)
    T.check(not T.any(HSMPNative.sc_rec_drain(),function(m)return m.kind=="death_report" end),
        "publication memo cannot leak preparation authority into the active death reporter")
    pawn.__props.DED=false
    assignment(130,true,6,"Willie_BP_C_3",386);run(1000,true)
    T.check(root and root.life==130 and puts("local_root")>n,
        "actual root preserves full life130 despite wrapped order bits")
    n=puts("local_root")
    assignment(130,true,6,"Old_Pawn",386);run(1000,true)
    T.check(puts("local_root")==n,"another pawn's placement does not resume root publication")
    assignment(130,true,0,"Willie_BP_C_3",386);run(1000,true)
    T.check(puts("local_root")==n,"non-deathmatch respawning cannot resume root publication")
    assignment(130,false,6,"Willie_BP_C_3",386);run(1000,true)
    T.check(puts("local_root")==n,"unverified respawn cannot resume root publication")
elseif opts.kind == "native_sample" then
    -- Native sampling: the settings A/B switch, the native sampler used while it answers, the
    -- Lua path on a refusal and while switched off (HSMPNative.sample_local is a test hook).
    -- Default ON: no "native_sample" key at all.
    T.write(sd .. "/.settings.json", '{"send_hz":60}\n')
    local N = rawget(_G, "HSMPNative")
    local calls, mode = { pose = 0, root = 0, weapon = 0, n = 0 }, "ok"
    N._sample = function(a)   -- one call carries every part (sample.rs: mask 1 root, 2 weapon, 4 pose)
        assert(a.context and a.context.match_id==71 and a.context.round==1 and a.context.life==1
            and a.root_pawn==a.pawn,"native root uses the same verified original pose pawn/context")
        calls.n = calls.n + 1
        local m, err = 0, nil
        if a.root_pawn then calls.root = calls.root + 1; m = m | 1 end
        if a.weapon_actor then calls.weapon = calls.weapon + 1; m = m | 2 end
        if a.mesh then
            calls.pose = calls.pose + 1
            if mode == "skip" then err = "skip:bone" else m = m | 4 end
        end
        return m, err
    end
    local pawn = boot()
    pawn.__props["Weapon R"] = weapon("W_1", 5001)
    run(3000, true)
    T.check(calls.pose > 100 and calls.root > 100, "default (no native_sample key): the native sampler takes root + pose", T.repr(calls))
    T.check(puts("local_pose") == 0 and puts("local_root") == 0 and puts("local_weapon") <= 10, "verified root and pose stay native; pre-placement weapon fallback remains bounded",
        T.repr(hot() and hot().n))
    local settled_n, settled_p = calls.n, calls.pose
    run(1000,true)
    T.check(calls.weapon > 100 and calls.n-settled_n == calls.pose-settled_p, "one native call per verified sample: root, held weapon and context-bound pose together", T.repr(calls))
    mode = "skip"
    local p0, r0 = puts("local_pose"), puts("local_root")
    run(1000, true)
    T.check(puts("local_pose") > p0 + 30, "a refused native sample falls back to the Lua path (same frame)", puts("local_pose") - p0)
    T.check(puts("local_root") == r0, "only the refused part: root stays native", puts("local_root") - r0)
    mode = "ok"
    T.write(sd .. "/.settings.json", '{"send_hz":60,"native_sample":false}\n')
    run(2000, true)
    local c0, l0 = calls.pose, puts("local_pose")
    run(1000, true)
    T.check(calls.pose == c0 and puts("local_pose") > l0 + 30, "settings native_sample=false: Lua path only", T.repr(calls))
    local log = M.logtext()
    T.check(T.count(log, "native sampling ON") == 0 and T.count(log, "native sampling off") == 1, "on by default (no ON line), the switch-off logged once", log:sub(1, 400))
    T.check(#M.dead_touch == 0, "nothing freed touched", T.repr(M.dead_touch))
end
