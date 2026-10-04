-- Per-frame Lua heap churn of the hot loops, offline.
--
--     hsmp-tools lua-test framecost       (HSMP_FRAMECOST_PRINT=1 prints the numbers)
--
-- Every UE4SS call and the native module are mocked with allocation-free stand-ins
-- (constant return values, no-op writes), so the bytes counted are the mods' own
-- per-frame garbage: temporary tables, closures, formatted strings. The GC is stopped
-- while a frame runs and collectgarbage("count") is read around it.
--
--   * ipc:      a facade call (IPC.flags / IPC.peer_play / IPC.bus_table) allocates nothing.
--   * sync:     HSMPSync's per-frame sender (fast_send), native and Lua sampling.
--   * avatars:  HSMPAvatars' per-frame stand-in driver (on_frame) on a full 22-body v2
--               stand-in, native servo and Lua servo.
--   * pure_equal: the allocation-free target math matches the beta.4 math bit for bit.
-- A global FC_PROF(on) function, when a caller defines one, brackets each measured frame
-- (a line-level allocation profiler hook for local investigation).
-- Every case runs in its own Lua state (T.isolated).

local mode, opts = ...
local PRINT = false
for _, a in ipairs({ ... }) do if a == "print" then PRINT = true end end

if mode ~= "case" then
    for _, k in ipairs({ "ipc", "sync_native", "sync_lua", "avatars_native", "avatars_lua", "pure_equal", "world" }) do
        T.isolated(T.script, "case", { kind = k, print = PRINT or os.getenv("HSMP_FRAMECOST_PRINT") == "1" })
    end
    return
end

-- The harness prelude wraps pcall / xpcall (to report swallowed nil-global calls) with a
-- table.pack per call; measure against the raw functions, which is what UE4SS runs.
for _, fname in ipairs({ "pcall", "xpcall" }) do
    local f = _G[fname]
    for i = 1, 8 do
        local n, v = debug.getupvalue(f, i)
        if n == nil then break end
        if n == "raw_" .. fname then _G[fname] = v; break end
    end
end

local function report(name, bytes, limit)
    local line = string.format("framecost %-16s %9.1f B/frame (limit %d)", name, bytes, limit)
    if opts.print then print(line) end
    T.log(line)
    T.check(bytes <= limit, name .. ": per-frame Lua garbage within the budget", line)
end

-- Bytes allocated per call of f (GC stopped around each call; `between` runs unmeasured).
local function churn(n, f, between)
    collectgarbage("collect")
    local sum = 0
    for i = 1, n do
        if between then between(i) end
        collectgarbage("stop")
        local a = collectgarbage("count")
        local prof = rawget(_G, "FC_PROF")
        if prof then prof(true) end
        f(i)
        if prof then prof(false) end
        sum = sum + (collectgarbage("count") - a)
        collectgarbage("restart")
    end
    return sum * 1024 / n
end

if opts.kind == "ipc" then
    package.path = T.path("mods/shared") .. "/?.lua;" .. T.path("tools/hsmp-tools/lua-tests/lib") .. "/?.lua;" .. package.path
    local NM = require("hsmp_native_mock")
    local N = NM.new({})
    local IPC = dofile(T.path("mods/shared/hsmp_ipc.lua"))
    IPC.init({ mod = "framecost", native = N, log = function() end })
    T.check(IPC.backend == "shm", "the facade binds the mock")
    -- allocation-free native stand-ins: the facade's own cost only
    local bus_t = { state = "Live" }
    N.flags = function() return 3 end
    N.peer_play = function(slot, out, last) return nil end
    N.bus_get = function(key, last) if last == 7 then return 7 end return 7, bus_t end
    IPC.bus_table("director")
    report("ipc.flags", churn(2000, function() IPC.flags() end), 8)
    local out = {}
    report("ipc.peer_play", churn(2000, function() IPC.peer_play(1, out, 5) end), 8)
    report("ipc.bus_table", churn(2000, function() IPC.bus_table("director") end), 8)
    return
end

local M = require("umg_mock")
local sd = T.tmpdir("hsmp_fc_sd_")
local la = T.tmpdir("hsmp_fc_la_")

-- Allocation-free UE4SS returns: one constant table per kind of value.
local XF = { Translation = { X = 1, Y = 2, Z = 3 }, Rotation = { X = 0, Y = 0, Z = 0, W = 1 } }
local V0 = { X = 0, Y = 0, Z = 0 }
local LOC = { X = 10, Y = 20, Z = 30 }
local ROT = { Pitch = 0, Yaw = 90, Roll = 0 }
local function lean_methods(o)
    local Mt = M.Methods
    local arena = "World /Game/Maps/Arenas/Map_Arena_Pit/Map_Arena_Pit.Map_Arena_Pit"
    local fn_cache, cls_cache = {}, {}
    Mt.GetFullName = function(self)
        if rawget(self, "__cls") == "World" then return arena end
        return "Obj"
    end
    Mt.GetFName = function(self)
        local n = rawget(self, "__name")
        local f = fn_cache[n]
        if not f then f = { ToString = function() return n end }; fn_cache[n] = f end
        return f
    end
    Mt.GetClass = function(self)
        local c = rawget(self, "__cls")
        local k = cls_cache[c]
        if not k then
            local f = { ToString = function() return c end }
            k = { GetFName = function() return f end }
            cls_cache[c] = k
        end
        return k
    end
    Mt.GetAddress = function(self) return rawget(self, "__addr") or 4096 end
    Mt.GetWorld = function() return M.world end
    Mt.K2_GetActorLocation = function(self) return rawget(self, "loc") or LOC end
    Mt.K2_GetActorRotation = function() return ROT end
    Mt.GetVelocity = function() return V0 end
    Mt.GetRealTimeSeconds = function() return M.now / 1000 end
    Mt.GetWorldDeltaSeconds = function() return 1 / 60 end
    Mt.GetBoneIndex = function() return 1 end
    Mt.IsSimulatingPhysics = function() return true end
    Mt.GetSocketTransform = function() return XF end
    Mt.GetTransform = function() return XF end
    Mt.GetSocketLocation = function() return LOC end
    Mt.GetCenterOfMass = function() return LOC end
    Mt.GetPhysicsLinearVelocity = function() return V0 end
    Mt.GetPhysicsLinearVelocityAtPoint = function() return V0 end
    Mt.GetPhysicsAngularVelocityInDegrees = function() return V0 end
    Mt.K2_GetComponentLocation = function() return LOC end
    Mt.GetBoneMass = function() return 5 end
    Mt.SetPhysicsLinearVelocity = function() end
    Mt.SetPhysicsAngularVelocityInDegrees = function() end
    Mt.SetAllPhysicsLinearVelocity = function() end
    Mt.SetSimulatePhysics = function() end
    Mt.SetEnableGravity = function() end
    Mt.SetCollisionEnabled = function() end
    Mt.SetCollisionResponseToChannel = function() end
    Mt.GetCollisionResponseToChannel = function() return 2 end
    Mt.SetAllMotorsAngularDriveParams = function() end
    Mt.SetActorEnableCollision = function() end
    Mt.WasRecentlyRendered = function() return true end
    Mt.GetFrameCount = function() return M.now end
    Mt.ExecuteConsoleCommand = function() end
    Mt.GetConsoleVariableIntValue = function() return 1 end
end

-- ---------------------------------------------------------------- HSMPSync
if opts.kind == "sync_native" or opts.kind == "sync_lua" then
    local native = opts.kind == "sync_native"
    T.write(sd .. "/.settings.json", native and '{"send_hz":60}\n' or '{"send_hz":60,"native_sample":false}\n')
    M.install({ state_dir = sd, env = { LOCALAPPDATA = la, HSMP_INST = "7", HSMP_POSE_PROBE = false }, strict = true })
    lean_methods()
    local sfo = _G.StaticFindObject
    _G.StaticFindObject = function(path)
        if path == "/Script/Engine.Default__KismetSystemLibrary" then return M.ksl end
        return sfo(path)
    end
    _G.RegisterHook = function() return 1, 2 end
    local frame_fn
    _G.LoopInGameThreadAfterFrames = function(n, fn) frame_fn = fn; return 1 end
    local pawn = M.new_obj("Willie_BP_C", "Willie_BP_C_3")
    pawn.__props.Health = 100
    pawn.__props.Mesh = M.new_obj("SkeletalMeshComponent", "CharacterMesh0")
    local w = M.new_obj("ModularWeaponBP_ArmingSword_C", "W_1")
    rawset(w, "__addr", 5001)
    w.__props.RootComponent = M.new_obj("StaticMeshComponent", "W_1_root")
    w.__props["Root Scene"] = M.new_obj("SceneComponent", "W_1_base")
    w.__props.TippyTipScene = M.new_obj("SceneComponent", "W_1_tip")
    pawn.__props["Weapon R"] = w
    M.pc.__props.Pawn = pawn
    dofile(T.path("mods/HSMPSync/Scripts/main.lua"))
    T.check(type(frame_fn) == "function", "the per-frame sender is registered")
    local N = rawget(_G, "HSMPNative")
    local ncalls = 0
    N._sample = function(a)   -- one call carries every part (mask 1 root, 2 weapon, 4 pose)
        ncalls = ncalls + 1
        return (a.root_pawn and 1 or 0) | (a.weapon_actor and 2 or 0) | (a.mesh and 4 or 0)
    end
    local function beat()
        N.sc_put("link", { status = 1, state = 1, my_peer_id = 1 })
        N._st.hb_age = 0.05
    end
    beat()
    -- live, settled and sending (the 30 Hz tick runs from the mock's loops)
    for _ = 1, 16 do beat(); M.run(250) end
    local hot = N._hot.n
    local pose0 = hot.local_pose
    -- allocation-free writes for the measured frames (the Lua path's put_* are the mock's)
    local puts = 0
    N.put_root = function() puts = puts + 1; return true end
    N.put_weapon = function() puts = puts + 1; return true end
    N.put_pose = function() puts = puts + 1; return true end
    N.frame = function() return 0 end
    local c0 = ncalls
    local b = churn(600, function() frame_fn() end, function()
        M.now = M.now + 17   -- one 60 Hz sample due per measured frame
    end)
    if native then
        T.check(hot.local_pose == pose0 and puts == 0, "native sampling took every sample", puts)
        T.check(ncalls - c0 == 600, "one native call per sample (root + weapon + pose)", ncalls - c0)
    else
        T.check(puts >= 1200, "the Lua sampler wrote root / weapon / pose every frame", puts)
    end
    report(opts.kind, b, native and 400 or 2500)
    T.check(#M.dead_touch == 0, "nothing freed touched", T.repr(M.dead_touch))
    return
end

-- ---------------------------------------------------------------- HSMPAvatars
if opts.kind == "avatars_native" or opts.kind == "avatars_lua" then
    local native = opts.kind == "avatars_native"
    T.write(sd .. "/.settings.json", native and '{"avatars":true}\n'
        or '{"avatars":true,"native_servo":false,"native_neutralise":false,"native_wservo":false}\n')
    M.install({ state_dir = sd, env = { LOCALAPPDATA = la, HSMP_INST = "7" }, strict = true })
    package.path = T.path("mods/shared") .. "/?.lua;" .. package.path
    lean_methods()
    _G.RegisterHook = function() return 1, 2 end
    local frame_fn
    _G.LoopInGameThreadAfterFrames = function(n, fn) frame_fn = fn; return 1 end
    _G.HSMP_AVATARS_TEST = {}
    dofile(T.path("mods/HSMPAvatars/Scripts/main.lua"))
    local api = HSMP_AVATARS_TEST.api
    T.check(type(frame_fn) == "function" and api, "the per-frame driver is registered")
    local N = rawget(_G, "HSMPNative")
    local me = M.new_obj("Willie_BP_C", "Willie_BP_C_ME"); rawset(me, "__addr", 7001)
    me.__props.Health = 100
    rawset(me, "loc", { X = -2000, Y = 0, Z = 100 })
    M.pc.__props.Pawn = me
    local si = M.new_obj("Willie_BP_C", "Willie_BP_C_5"); rawset(si, "__addr", 7005)
    si.__props.Health = 100
    rawset(si, "loc", { X = 300, Y = 0, Z = 100 })
    local mesh = M.new_obj("SkeletalMeshComponent", "CharacterMesh0"); rawset(mesh, "__addr", 9001)
    si.__props.Mesh = mesh
    -- a full v2 sample: 23 bones + both weapon slots, control block
    local B, W, C = {}, {}, {}
    for s = 1, 25 do
        local b = (s - 1) * 13
        B[b + 1], B[b + 2], B[b + 3] = 300 + s, s, 100 + s
        B[b + 4], B[b + 5], B[b + 6], B[b + 7] = 0, 0, 0.0998, 0.995
        B[b + 8], B[b + 9], B[b + 10] = 120, 30, -5
        B[b + 11], B[b + 12], B[b + 13] = 10, 20, 30
    end
    for i = 1, 16 do W[i] = i end
    for i = 1, 60 do C[i] = i / 10 end
    local seq = 0
    local function play()
        seq = seq + 1
        N.sc_peer_play(2, { peer_id = 2, seq = seq, v = 2, pt = M.now - 50, mode = "interp", cut = 0, delay = 30,
            root = { 300.0, 0.0, 100.0, 0.0 }, m = (1 << 25) - 1, B = B, W = W, C = C })
    end
    local function beat()
        N.sc_put("link", { status = 1, state = 1, my_peer_id = 1 })
        N._st.hb_age = 0.05
        N.sc_peer_dir({ { id = 1, nick = "Me" }, { id = 2, nick = "B" } })
    end
    if native then
        local o_c, o_v, o_dl, o_gl = {}, {}, {}, {}
        for i = 1, 23 * 7 do o_c[i] = (i % 7 == 0) and 1 or 0.5 end
        for i = 1, 23 * 3 do o_v[i] = 1 end
        for i = 1, 23 do o_dl[i], o_gl[i] = 1, 1 end
        N._servo = function(a, out) out.c, out.v, out.dl, out.gl = o_c, o_v, o_dl, o_gl; return 22 end
        N._neutralise = function() return 30 end
    end
    beat(); play(); M.run(300)
    for _ = 1, 10 do beat(); play(); M.run(250) end   -- past the world settle
    local body = { mesh = mesh, field = "Mesh", sims = { mesh }, bones = {}, motors = {}, handles = {}, cache = {},
                   ctl = "servo", snaps = 0, lens = {}, made_at = 0 }
    api.set_puppet(2, { actor = si, nick = "B", claimed_at = 0, gen = api.PX.world_gen_for_test and 0 or 0,
                        addr = 7005, driving = true, in_range = true, body = body })
    for _ = 1, 4 do beat(); play(); M.run(250) end
    for _ = 1, 60 do M.now = M.now + 16; play(); frame_fn() end
    local p = api.puppets()[2]
    T.check(p and p.body and p.body.sv and p.body.sv.n == 22, "the stand-in is servoed on 22 bodies",
        T.repr(p and p.body and p.body.sv and p.body.sv.n))
    local drove0 = p.body.sv.err.n
    local b = churn(400, function() frame_fn() end, function()
        M.now = M.now + 16
        play()
    end)
    T.check(p.body.sv.err.n > drove0 + 300 * 20, "every measured frame drove the bodies", p.body.sv.err.n - drove0)
    report(opts.kind, b, native and 8000 or 9000)
    T.check(not T.contains(M.logtext(), "frame driver error"), "no driver errors", M.logtext():sub(-2000))
    T.check(#M.dead_touch == 0, "nothing freed touched", T.repr(M.dead_touch))
    return
end

-- ---------------------------------------------------------------- PURE, bit for bit
-- The allocation-free target math (scalar advance / aim, in-place retarget, reused
-- play tables) against the beta.4 implementations (lib/avatars_pure_ref.lua): every
-- number identical, on random samples.
if opts.kind == "pure_equal" then
    M.install({ state_dir = sd, env = { LOCALAPPDATA = la, HSMP_INST = "7" }, strict = true })
    package.path = T.path("mods/shared") .. "/?.lua;" .. package.path
    _G.RegisterHook = function() return 1, 2 end
    _G.HSMP_AVATARS_TEST = {}
    dofile(T.path("mods/HSMPAvatars/Scripts/main.lua"))
    local P = HSMP_AVATARS_TEST.api.PURE
    local R = dofile(T.path("tools/hsmp-tools/lua-tests/lib/avatars_pure_ref.lua"))(P)
    math.randomseed(20261004)
    local function rnd(a) return (math.random() * 2 - 1) * a end
    local function sample()
        local q = { rnd(1), rnd(1), rnd(1), rnd(1) }
        local l = math.sqrt(q[1] ^ 2 + q[2] ^ 2 + q[3] ^ 2 + q[4] ^ 2)
        return { rnd(3000), rnd(3000), rnd(500), q[1] / l, q[2] / l, q[3] / l, q[4] / l,
                 rnd(1500), rnd(1500), rnd(800), rnd(900), rnd(900), rnd(900) }
    end
    local function same(a, b)
        if type(a) ~= "table" or type(b) ~= "table" then return a == b end
        for k, v in pairs(a) do if not same(v, b[k]) then return false end end
        for k in pairs(b) do if a[k] == nil then return false end end
        return true
    end
    local function copy(t) local o = {}; for k, v in pairs(t) do o[k] = type(v) == "table" and copy(v) or v end; return o end
    local bad = { adv = 0, adv_acc = 0, aim = 0, aim_dst = 0, fk = 0, fk_in = 0, qang = 0, acc = 0, play = 0, play_in = 0 }
    for n = 1, 3000 do
        local tg, ms, ma = sample(), rnd(100), rnd(100)
        if n % 50 == 0 then tg[11], tg[12], tg[13] = 0, 0, 0 end   -- no rotation: the a <= 1e-9 branch
        local acc = { rnd(60000), rnd(60000), rnd(60000) }
        if not same(P.advance(tg, ms), R.advance(tg, ms)) then bad.adv = bad.adv + 1 end
        if not same(P.advance(tg, ms, acc, {}), R.advance(tg, ms, acc)) then bad.adv_acc = bad.adv_acc + 1 end
        if not same(P.aim(tg, ms, ma, acc), R.aim(tg, ms, ma, acc)) then bad.aim = bad.aim + 1 end
        local d = sample()
        if not same(P.aim(tg, ms, ma, nil, d), R.aim(tg, ms, ma, nil)) then bad.aim_dst = bad.aim_dst + 1 end
        local a8 = sample()
        if P.qangle8(tg[4], tg[5], tg[6], tg[7], a8[4], a8[5], a8[6], a8[7]) ~= R.qangle({ tg[4], tg[5], tg[6], tg[7] }, { a8[4], a8[5], a8[6], a8[7] }) then
            bad.qang = bad.qang + 1
        end
        if not same(P.clamp_acc3({}, acc[1], acc[2], acc[3]), R.clamp_acc({ acc[1], acc[2], acc[3] })) then bad.acc = bad.acc + 1 end
        if n % 10 == 0 then
            local targets, loc = {}, {}
            for s = 1, 25 do if s == 1 or math.random() < 0.9 then targets[s] = sample() end end
            for i = 2, P.V2_NB do if math.random() < 0.9 then loc[i] = { rnd(30), rnd(30), rnd(30) } end end
            local want = R.fk_retarget(targets, loc)
            if not same(P.fk_retarget(targets, loc), want) then bad.fk = bad.fk + 1 end
            if not same(P.fk_retarget_in(copy(targets), loc), want) then bad.fk_in = bad.fk_in + 1 end
        end
    end
    -- play_from_out: fresh, and refilled in place across samples whose masks and weapons change
    local into = { slots = {}, weapons = {} }
    for n = 1, 300 do
        local m, B, W, C = 0, {}, {}, {}
        for s = 1, 25 do
            if s == 1 or math.random() < 0.8 then
                m = m | (1 << (s - 1))
                for k = 1, 13 do B[#B + 1] = rnd(1000) end
            end
        end
        for k = 1, (n % 3) * 8 do W[k] = rnd(10) end
        for k = 1, (n % 2) * 60 do C[k] = rnd(5) end
        local o = { seq = n, pt = rnd(1e5), mode = (n % 4 == 0) and "hold" or "interp", age = n, delay = 30, jit = 4,
                    cut = n % 5, lead = 12, iv = 16, st = 3, rate = 1.05, m = m, B = B, W = W, C = (n % 2 == 1) and C or false,
                    root = (n % 7 ~= 0) and { x = rnd(1000), y = rnd(1000), z = rnd(100), yaw = rnd(180) } or false }
        local want = R.play_from_out(o)
        if not same(P.play_from_out(o), want) then bad.play = bad.play + 1 end
        local got = P.play_from_out(o, into)
        local view = {}
        for k, v in pairs(got) do if k ~= "spare" then view[k] = v end end
        if not same(view, want) then bad.play_in = bad.play_in + 1 end
    end
    for k, v in pairs(bad) do T.check(v == 0, "PURE " .. k .. " identical to the beta.4 math", v) end
    return
end

-- ---------------------------------------------------------------- HSMPWorld
-- The per-body reads of the 16 ms tick: can_drive (alive / valid / natively held) on a
-- scene prop, and the tick's hand read, through lean UObject stand-ins.
if opts.kind == "world" then
    package.preload["UEHelpers"] = function() return {} end
    package.path = T.path("mods/shared") .. "/?.lua;" .. T.path("mods/HSMPWorld/Scripts") .. "/?.lua;" .. package.path
    local real_getenv = os.getenv
    os.getenv = function(k) if k == "HSMP_STATE_DIR" then return sd end return real_getenv(k) end
    _G.HSMP_WORLD_TEST = true
    _G.FName = function(s) return s end
    _G.print = function() end
    local H = assert(load(T.read(T.path("mods/HSMPWorld/Scripts/main.lua")), "@HSMPWorld/Scripts/main.lua"))()
    H.set_name_none("None")
    local LOC = { X = 1, Y = 2, Z = 3 }
    local Mt = { IsValid = function() return true end, GetAddress = function(self) return rawget(self, "_a") end,
                 GetAttachParentActor = function() return nil end, IsSimulatingPhysics = function() return true end,
                 K2_GetActorLocation = function() return LOC end }
    local MT = { __index = Mt }
    local function obj(a, props)
        local o = setmetatable(props or {}, MT)
        rawset(o, "_a", a)
        return o
    end
    local actor, body = obj(0x100), obj(0x110)
    local o = { actor = actor, body = body, kind = "prop" }
    T.check(H.can_drive(o) == true, "the prop is drivable")
    report("world.can_drive", churn(2000, function() H.can_drive(o) end), opts.limit or 40)
    local pawn = obj(0x200, { ["Weapon R"] = obj(0x210), ["Grab Component R"] = obj(0x220), ["Grabbed R"] = true })
    local pc = obj(0x300, { Pawn = pawn })
    H.read_hands(pc)
    T.check(H.hands.R == 0x210 and H.hands.gR == 0x220, "the hand read sees the held weapon and grab", T.repr(H.hands))
    report("world.read_hands", churn(2000, function() H.read_hands(pc) end), opts.limit or 200)
    return
end
