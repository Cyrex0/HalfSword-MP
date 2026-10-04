-- HSMP-SHM Lua facade (mods/shared/hsmp_ipc.lua) against the
-- in-memory HSMPNative mock (lib/hsmp_native_mock.lua).
--
--     hsmp-tools lua-test ipc
--
-- Backend selection, ABI check, per-kind cap fallback, the legacy-path bridge
-- (session sub-maps, peer slots, blobs, outboxes, request files, inbound
-- logs), hsmp_state caching, the send retry queue, per-kind event queues,
-- hsmp_session liveness on the sidecar heartbeat,
-- HSMPSync's shm pose sender and HSMPAvatars' play_from_out.

local mode, opts = ...
if mode ~= "case" then
    for _, k in ipairs({ "select", "caps", "bridge", "send", "events", "session",
                         "two_states", "sync_shm", "bus", "world_typed", "play_from_out" }) do
        T.isolated(T.script, "case", { kind = k })
    end
    return
end

local NM = require("hsmp_native_mock")
local S = NM.S
local SHARED = T.path("mods/shared/")
local function fresh_ipc() return dofile(SHARED .. "hsmp_ipc.lua") end
local sd = T.tmpdir("hsmp_ipc_sd_")
local logs = {}
local function log(fmt, ...) logs[#logs + 1] = string.format(fmt, ...) end
local function init(o)
    local IPC = fresh_ipc()
    o = o or {}
    o.mod, o.state_dir, o.log = o.mod or "Test", sd, log
    IPC.init(o)
    return IPC
end

if opts.kind == "select" then
    _G.HSMPNative = nil
    local IPC = init({ native = false })
    T.check(IPC.backend == "none" and IPC.ui_error == IPC.MSG_NO_HELPER
        and T.contains(table.concat(logs, "\n"), "ipc_unavailable{"), "no native module: unavailable + ipc_unavailable, never files", IPC.why)
    T.check(IPC.bus_table("playback") == nil and IPC.bus_put("playback", { rows = {} }) == nil
        and IPC.rec("session") == nil and IPC.read_file == nil and IPC.append_line == nil and IPC.tail_lines == nil,
        "unavailable: records are absent / refused; no path adapter exists (no file fallback)")
    T.check(not T.exists(sd .. "/.playback.json"), "unavailable: nothing written")
    local N = NM.new()
    IPC = init({ native = N, settings = '{"ipc":"file"}' })
    T.check(IPC.backend == "shm", "the native module is always used (no ipc setting / HSMP_IPC any more)", IPC.why)
    IPC = init({ native = NM.new({ abi = { S.ABI_MAJOR, "0000000000000000" } }) })
    T.check(IPC.backend == "none" and IPC.ui_error == IPC.MSG_MISMATCH
        and T.contains(table.concat(logs, "\n"), "ipc_refused{code=abi_mismatch"), "layout hash mismatch: refused + reinstall message", IPC.why)
    IPC = init({ native = NM.new({ abi = { S.ABI_MAJOR + 1, S.LAYOUT_HASH } }) })
    T.check(IPC.backend == "none" and IPC.ui_error == IPC.MSG_MISMATCH, "ABI major mismatch: refused")
    _G.HSMPNative = NM.new()
    IPC = init({})
    T.check(IPC.backend == "shm" and IPC.native_src == "HSMPNative", "global HSMPNative is picked up")
    T.check(rawget(_G, "HSMP_IPC") == IPC, "the facade is published as the per-state global HSMP_IPC")
    T.check(_G.HSMPNative._st.calls.ipc_open == 1, "init opens the segment")
    _G.HSMPNative = nil
    -- a raising native function never escapes
    local bad = NM.new()
    bad.frame = function() error("boom") end
    IPC = init({ native = bad })
    local ok, f = pcall(IPC.frame, "w")
    T.check(ok and f == 0, "a raising native call returns 0 / nil, never raises", tostring(f))

elseif opts.kind == "caps" then
    local N = NM.new({ caps = S.CAPS.POSE })
    local IPC = init({ native = N })
    T.check(IPC.use("local_pose") and IPC.use("peer_play"), "POSE negotiated: pose kinds on shm")
    T.check(not IPC.use("session") and not IPC.use("vitals") and not IPC.use("world_claim"),
        "STATE / VITALS / QUEUES not negotiated: file path")
    T.check(not IPC.use("bus"), "BUS is a game-local cap (game caps)")
    N._st.sidecar_state = "absent"
    IPC.refresh_info(true)
    T.check(not IPC.use("local_pose"), "sidecar not attached: nothing on shm")
    N._st.sidecar_state = "ready"
    N.sc_caps(S.CAPS.POSE | S.CAPS.STATE, NM.CAPS_ALL)
    IPC.frame("w1")   -- SIDECAR_RESET -> caps refreshed now, not in 1 s
    T.check(IPC.use("session"), "caps re-read on SIDECAR_RESET")
    T.check(not IPC.use("no_such_kind"), "unknown kind: never shm")
    _G.HSMPNative = nil
    local IPCf = init({ native = false })
    T.check(not IPCf.use("local_pose") and IPCf.put_pose() == nil and IPCf.peers({}) == 0,
        "IPC unavailable: every typed call is a no-op")

elseif opts.kind == "bridge" then
    local N = NM.new()
    local IPC = init({ native = N })
    -- The session domain is typed records (slots session / link / admin): its old file
    -- names are not routed any more (a read of them is real file IO: nil here).
    N.sc_put("link", { status = 1, state = 1, my_peer_id = 3 })
    T.check(IPC.rec("link").my_peer_id == 3, "the link record, typed")
    -- peer slots
    N.sc_peer_dir({ { id = 5, slot = 0, nick = "Bob" } })
    IPC.frame("w")
    N.sc_put("peer_vitals", { seq = 4, flags = 1, v = { 5120 } }, 0)
    local v = IPC.peer_rec("peer_vitals", 5)
    T.check(v and v.seq == 4 and v.flags == 1 and v.v[1] == 5120 and v.v[19] == 0, "peer_rec(peer_vitals, id) -> the typed record", T.repr(v))
    T.check(IPC.peer_rec("peer_vitals", 5) == v, "unchanged peer slot: the cached table")
    T.check(IPC.peer_rec("peer_vitals", 9) == nil, "a peer without a slot: nil")
    -- peer_root is a record slot (the relayed root record + its owner), read typed.
    N.sc_put("peer_root", { peer_id = 5, root = { tick = 77, ts = 1000, pos = { 1, 2, 3 }, rot = { 0, 0, 0.7071068, 0.7071068 } } }, 0)
    local pr = IPC.peer_rec("peer_root", 5)
    T.check(pr and pr.peer_id == 5 and pr.root.tick == 77 and pr.root.pos[3] == 3 and math.abs(pr.root.rot[4] - 0.7071068) < 1e-6,
        "peer_root -> the typed record (root + peer id)", T.repr(pr))
    -- writes
    T.check(IPC.put("vitals", { seq = 1, flags = 0, v = { 3520 } }) == true and IPC.rec("vitals").v[1] == 3520,
        "put('vitals', record) and read back with IPC.rec (any Lua state)")
    T.check(IPC.send("leave", { reason = 0 })
        and N._rec.sends[1].kind == "leave" and N._rec.sends[1].data.reason == 0, "leave: a typed G2S record (the file is gone)")
    T.check(IPC.bus_put("spawn_status", { seq = 1, pawn = "W", verified = true })
        and N._rec.bus.spawn_status.t.verified == true and IPC.bus_table("spawn_status").pawn == "W",
        "spawn_status: a typed bus record (no path adapter)")
    IPC.bus_clear("spawn_status")
    local cl = IPC.bus_table("spawn_status")
    T.check(cl and cl.seq == 0 and cl.pawn == "", "bus_clear on a typed key: the zeroed record (seq 0 = absent)", T.repr(cl))
    -- events
    T.check(#IPC.events("death") == 0 and #IPC.events("hitfx_in") == 0, "typed kinds subscribe on first use")
    N.sc_rec_event("death", { peer_id = 5, round = 2, match_id = 7 })
    N.sc_rec_event("hitfx_in", { target_peer_id = 3, hit_id = 9 }, 4, 11)
    local d = IPC.events("death")
    T.check(#d == 1 and d[1].data.peer_id == 5 and d[1].data.round == 2 and d[1].data.match_id == 7, "typed S2G death records", T.repr(d))
    T.check(#IPC.events("death") == 0, "each event once")
    local fx = IPC.events("hitfx_in")
    T.check(#fx == 1 and fx[1].data.target_peer_id == 3 and fx[1].peer == 4 and fx[1].aux == 11,
        "IPC.pump keeps a typed record's peer / aux (the attacker of a hitfx_in)", T.repr(fx))

elseif opts.kind == "send" then
    -- The retry queue, on a typed G2S record (every G2S message is one now).
    local N = NM.new({ ring = 2 })
    local IPC = init({ native = N })
    local function dr(n) return { death_id = n, round = 1 } end
    T.check(type(IPC.send("death_report", dr(1))) == "number", "send -> req_id")
    IPC.send("death_report", dr(2))
    T.check(IPC.send("death_report", dr(3)) == true and IPC.pending() == 1, "ring full: queued for retry (true)")
    T.check(IPC.send("death_report", dr(4)) == true and IPC.pending() == 2, "later sends queue behind (order kept)")
    T.check(T.contains(table.concat(logs, "\n"), "ipc_backpressure{ring=g2s,state=full"), "backpressure logged once")
    N.sc_rec_drain()
    T.check(IPC.flush() and IPC.pending() == 0, "drained ring: the retry queue flushes")
    local q = N._rec.sends
    T.check(q[3].data.death_id == 3 and q[4].data.death_id == 4, "in order", T.repr({ q[3] and q[3].data, q[4] and q[4].data }))
    local a, e = IPC.send("death_report", "not a table")
    T.check(a == nil and e ~= nil and IPC.pending() == 0, "a bad message is refused, not queued", tostring(e))
    -- bounded queue
    N._st.ring = 0
    for i = 1, IPC.RETRY_MAX + 10 do IPC.send("death_report", dr(i)) end
    T.check(IPC.pending() == IPC.RETRY_MAX and IPC.stats.dropped >= 10, "the retry queue is bounded (oldest dropped)",
        IPC.pending())
    local Nc = NM.new({ caps = S.CAPS.POSE })
    local IPC2 = init({ native = Nc })
    local r2 = IPC2.send("death_report", dr(1))
    T.check(r2 == true and IPC2.pending() == 1, "COMBAT not negotiated (yet): queued for retry, not dropped", tostring(r2))

elseif opts.kind == "events" then
    local N = NM.new()
    local IPC = init({ native = N })
    T.check(#IPC.events("damage_in") == 0, "first call subscribes")
    for i = 1, 5 do N.sc_rec_event("damage_in", { hit_id = i, target_peer_id = 1, bone = "head" }, 2) end
    N.sc_event("notice", { text = "x" })
    local ev = IPC.events("damage_in")
    T.check(#ev == 5 and ev[5].data.hit_id == 5 and ev[5].peer == 2 and ev[5].data.bone == "head",
        "damage_in records in order, with the attacker (peer)", #ev)
    T.check(#IPC.events("notice") == 0, "a kind subscribed later sees only later events")
    -- overflow of the native log -> resync
    local Ns = NM.new({ log = 4 })
    local IPCs = init({ native = Ns })
    IPCs.events("death")
    for i = 1, 10 do Ns.sc_rec_event("death", { peer_id = i, round = 1 }) end
    local d = IPCs.events("death")
    T.check(IPCs.resync == true and #d == 4 and d[1].data.peer_id == 7, "a cursor that fell behind: resync + the oldest kept", T.repr(d))

elseif opts.kind == "session" then
    local N = NM.new()
    local IPC = init({ native = N })
    local clock = 0
    local HS = dofile(SHARED .. "hsmp_session.lua")
    local Sx = HS.new({ ipc = IPC, clock = function() return clock end, every_s = 0 })
    N.sc_put("link", { status = S.ENUMS.sidecar_status.CONNECTED, state = S.ENUMS.link_state.UP, my_peer_id = 4 })
    Sx:poll(true)
    T.check(Sx.status == "connected" and Sx.peer_id == 4, "status from the typed link record (no file)")
    T.check(Sx:live(), "live on the sidecar heartbeat (the link changes on change only)")
    clock = 30
    Sx:poll(true)
    T.check(Sx:live(), "still live 30 s later while the heartbeat is fresh")
    N._st.hb_age = 9
    IPC.refresh_info(true)
    T.check(not Sx:live(), "heartbeat stale (> FRESH_S): not live")
    T.check(not T.exists(sd .. "/.sidecar.json"), "no state file involved")

elseif opts.kind == "bus" then
    -- the Lua<->Lua contracts are typed bus records in shared memory
    local N = NM.new()
    local IPC = init({ native = N })
    T.check(IPC.bus_table("playback") == nil, "unwritten key: nil")
    T.check(IPC.bus_put("playback", { rows = { { peer = 2, body_ts = 100, arm_ts = 100, local_ms = 5 } } }), "bus_put a record")
    local t, g = IPC.bus_table("playback")
    T.check(t and t.n == 1 and t.rows[1].peer == 2 and t.rows[1].body_ts == 100 and g ~= nil, "bus_table reads it back", T.repr(t))
    T.check(IPC.bus_table("playback") == t, "same generation: the cached table")
    T.check(IPC.bus_put("dev_notes", { phase = "fight" }) == nil, "a key that is not a typed record is refused")
    T.check(IPC.bus_put("fallback_swap", { until_s = 5, keep = "W" }) and IPC.bus_table("fallback_swap").keep == "W", "fallback_swap set")
    IPC.bus_clear("fallback_swap")
    T.check(IPC.bus_table("fallback_swap").keep == "" and IPC.bus_table("fallback_swap").until_s == 0, "fallback_swap cleared: the zeroed record")
    -- plain file helpers (config / logs; no contract)
    T.check(IPC.write(sd .. "/.x_test.txt", "7\n") and IPC.read(sd .. "/.x_test.txt") == "7\n" and IPC.exists(sd .. "/.x_test.txt"),
        "IPC.write / read / exists: plain files")
    IPC.remove(sd .. "/.x_test.txt")
    T.check(not IPC.exists(sd .. "/.x_test.txt"), "IPC.remove: plain os.remove")
    -- typed bus keys (session domain records): tables in, tables out
    T.check(IPC.bus_put("return_to_lobby", { seq = 1 }) and IPC.bus_table("return_to_lobby").seq == 1, "typed key set")
    IPC.bus_clear("return_to_lobby")
    T.check(IPC.bus_table("return_to_lobby").seq == 0, "typed key cleared: the zeroed record")
    T.check(IPC.bus_put("travel_request", { seq = 3, want = "arena", arena = "Map_Arena_Pit", t = 12 })
        and IPC.bus_table("travel_request").arena == "Map_Arena_Pit", "a typed request round-trips")
    local bad = IPC.bus_put("travel_request", { seq = 4, t = 0 / 0 })
    T.check(not bad and IPC.bus_table("travel_request").seq == 3, "a non-finite float is refused (the record is unchanged)")
    -- (the game-written vitals are a record slot now: IPC.put / IPC.rec, no bus mirror)
    T.check(not T.exists(sd .. "/.playback.json") and not T.exists(sd .. "/.vitals_out.json"), "no state file anywhere")

elseif opts.kind == "world_typed" then
    -- HSMPWorld reads and writes the world as typed records (schema/world.rs): no JSON, no
    -- legacy names, no line reader.
    local N = NM.new()
    _G.HSMPNative = N
    local IPC = init({ native = N })
    local o = { id = 11, pos = { 1, 2, 3 }, rot = { 0, 0, 0 }, vel = { 0, 0, 0 }, flags = 4 | (3 << 6) }
    N.sc_put("world_remote", { level = 7, epoch = 2, rows = { { sender = 3, seq = 1, ts = 500, obj = o } } })
    local t, ver = IPC.rec("world_remote")
    T.check(type(t) == "table" and ver and t.n == 1 and t.rows[1].obj.id == 11 and t.rows[1].sender == 3,
        "world_remote as a typed record", T.repr(t))
    T.check(IPC.route == nil and IPC.state_table == nil, "no path routing, no state blobs")
    local src = T.read(T.path("mods/HSMPWorld/Scripts/main.lua"))
    T.check(not T.contains(src, "state_table(") and not T.contains(src, "write_atomic(") and not T.contains(src, "string.format('{"),
        "HSMPWorld: no blob tables, no file writer, no JSON built")
    T.check(T.contains(src, 'ipc.put("world_out"') and T.contains(src, 'ipc.put("world_manifest_out"')
        and T.contains(src, 'ipc.put("world_hash"') and T.contains(src, 'ipc.send("world_claim"')
        and T.contains(src, 'ipc.send("world_sync"'), "HSMPWorld: records put / sent")
    T.check(IPC.use("world_remote") and IPC.use("world_out") and IPC.use("world_claim"), "WORLD negotiated (all caps)")
    T.check(IPC.send("world_sync", { level = 7 }) and N._rec.sends[1].kind == "world_sync" and N._rec.sends[1].data.level == 7,
        "world_sync is a G2S record")

elseif opts.kind == "two_states" then
    -- Two Lua states (two facade instances) share one native: each has its
    -- own cursor and sees every event once.
    local N = NM.new()
    N._set_cursor("A"); local A = init({ native = N, mod = "A" })
    N._set_cursor("B"); local B = init({ native = N, mod = "B" })
    N._set_cursor("A"); A.events("death")
    N._set_cursor("B"); B.events("death")
    N.sc_event("death", { peer_id = 1 })
    N.sc_event("death", { peer_id = 2 })
    N._set_cursor("A"); local ea = A.events("death")
    N._set_cursor("B"); local eb = B.events("death")
    T.check(#ea == 2 and #eb == 2 and ea[2].data.peer_id == 2 and eb[1].data.peer_id == 1, "fan-out: both states see both deaths")

elseif opts.kind == "sync_shm" then
    -- HSMPSync's sender with the native backend: root / weapon / pose go to
    -- the native, nothing is written to the state dir.
    local M = require("umg_mock")
    local la = T.tmpdir("hsmp_ipc_la_")
    local shm = true
    local N = NM.new()
    M.install({ state_dir = sd, env = { LOCALAPPDATA = la, HSMP_INST = "7", HSMP_POSE_PROBE = false }, strict = true })
    _G.HSMPNative = N
    local V0 = { X = 0, Y = 0, Z = 0 }
    local Mt = M.Methods
    Mt.GetFullName = function(self)
        if rawget(self, "__cls") == "World" then return "World /Game/Maps/Arenas/Map_Arena_Pit/Map_Arena_Pit.Map_Arena_Pit" end
        return "Obj " .. tostring(rawget(self, "__name"))
    end
    Mt.GetAddress = function(self) return rawget(self, "__addr") or 4096 end
    Mt.GetWorld = function() return M.world end
    Mt.K2_GetActorLocation = function() return { X = 10, Y = 20, Z = 30 } end
    Mt.K2_GetActorRotation = function() return { Pitch = 0, Yaw = 90, Roll = 0 } end
    Mt.GetVelocity = function() return V0 end
    Mt.GetRealTimeSeconds = function() return M.now / 1000 end
    Mt.GetBoneIndex = function() return 1 end
    Mt.IsSimulatingPhysics = function() return true end
    Mt.GetSocketTransform = function() return { Translation = { X = 1, Y = 2, Z = 3 }, Rotation = { X = 0, Y = 0, Z = 0, W = 1 } } end
    Mt.GetTransform = function() return { Translation = { X = 1, Y = 2, Z = 3 }, Rotation = { X = 0, Y = 0, Z = 0, W = 1 } } end
    Mt.GetSocketLocation = function() return { X = 1, Y = 2, Z = 3 } end
    Mt.GetPhysicsLinearVelocityAtPoint = function() return V0 end
    Mt.GetPhysicsAngularVelocityInDegrees = function() return V0 end
    Mt.K2_GetComponentLocation = function() return { X = 1, Y = 2, Z = 50 } end
    Mt.ExecuteConsoleCommand = function() end
    Mt.GetConsoleVariableIntValue = function() return 1 end
    local sfo = _G.StaticFindObject
    _G.StaticFindObject = function(path)
        if path == "/Script/Engine.Default__KismetSystemLibrary" then return M.ksl end
        return sfo(path)
    end
    _G.RegisterHook = function() return 1, 2 end
    io.popen = function() return nil end
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
    M.pawn = pawn
    dofile(T.path("mods/HSMPSync/Scripts/main.lua"))
    local beats = 0
    for _ = 1, 12 do
        beats = beats + 1
        if shm then
            N.sc_put("link", { status = 1, state = 1, my_peer_id = 1 })
        else
            T.write(sd .. "/.sidecar.json", string.format('{"peer_id":1,"status":"connected","last_tick":%d,"peers":[]}\n', beats))
        end
        M.run(250)
    end
    local files = T.glob(sd, "*")
    if shm then
        local L = N._st.local_
        T.check(L.local_pose and #L.local_pose.b == 23 * 13, "put_pose: 23 x 13 numbers", L.local_pose and #L.local_pose.b)
        T.check(L.local_pose and #L.local_pose.w == 21 and L.local_pose.w[1] == 1, "put_pose: the held sword (21 numbers, hands 1)",
            T.repr(L.local_pose and L.local_pose.w))
        T.check(L.local_root and L.local_root[3] == 10 and L.local_root[7] == 90, "put_root: pos + rotator",
            T.repr(L.local_root))
        T.check(L.local_weapon and L.local_weapon[4] == 1, "put_weapon: held right", T.repr(L.local_weapon))
        local root = N.sc_get("local_root")
        T.check(root and root.pos[1] == 10 and math.abs(root.rot[3] - math.sqrt(0.5)) < 1e-6 and math.abs(root.rot[4] - math.sqrt(0.5)) < 1e-6,
            "local_root: the root record, rotator -> quaternion (yaw 90)", T.repr(root))
        local wr = N.sc_get("local_weapon")
        T.check(wr and wr.held == 1 and wr.weapon_id > 0, "local_weapon: the weapon record", T.repr(wr))
        T.check(N.sc_get("local_pose") and N.sc_get("local_pose").tick == N._st.local_.local_pose.tick, "local_pose: the pose record head")
        local poses, ctl = 0, 0
        for _, p in ipairs(N._st.puts) do if p[1] == "local_pose" then poses = poses + 1 end end
        T.check(poses >= 60, "pose sampled at the send rate", poses)
        T.check(N._st.calls.frame and N._st.calls.frame >= 100, "one IPC.frame pump per game frame", N._st.calls.frame)
        T.check(N._st.calls.world_ready == 1 and N._st.ready_key ~= nil, "world_ready once after the settle time",
            N._st.calls.world_ready)
        T.check(N._st.local_.local_pose.b_ref == N._st.local_.local_pose.b_ref, "reused pose buffer")
        local streamed = false
        for _, f in ipairs(files) do
            if f:find("skeletal") or f:find("weapon") or f:find("me_%d") then streamed = true end
        end
        T.check(not streamed and not T.exists(sd .. "/.sidecar.json"), "no stream / session file in the state dir", T.repr(files))
        for _, p in ipairs(N._st.puts) do if p[1] == "local_pose" and N._st.local_.local_pose.c then ctl = ctl + 1 end end
        T.check(N._st.local_.local_pose.c == nil or #N._st.local_.local_pose.c == 37, "control layer: 37 numbers")
    end
    T.check(#M.dead_touch == 0, "nothing freed touched", T.repr(M.dead_touch))

elseif opts.kind == "play_from_out" then
    -- PURE.play_from_out builds exactly what PURE.parse_play2 builds from the
    -- equivalent play line.
    local src = T.read(T.path("mods/HSMPAvatars/Scripts/avatars_pure.lua"))
    local a = src:find("-- BEGIN PURE", 1, true)
    local b = src:find("-- END PURE", 1, true)
    local PURE = assert(load(src:sub(a, b - 1) .."\nreturn PURE", "@PURE"))()
    local B, nums = {}, {}
    local m = (1 << 0) | (1 << 1) | (1 << 23)   -- pelvis, spine_01, weapon_r
    for i = 1, 3 * 13 do B[i] = i * 0.5; nums[i] = string.format("%.2f", i * 0.5) end
    local W = { 1, 7, 0.5, 1.5, 2.5, 3.5, 4.5, 5.5 }
    local C = {}
    for i = 1, 37 do C[i] = i end
    local line = string.format('{"peer_id":5,"seq":12,"pt":1000,"mode":"interp","age":30,"delay":40.0,"jit":2.5,"cut":3,'
        .. '"lead":4.00,"st":8.00,"root":[1.0,2.0,3.0,90.00],"v":2,"ptf":1000.25,"m":%d,"vm":%d,"B":[%s],"W":[%s],"C":[%s],"iv":16.7,"end":1}',
        m, m, table.concat(nums, ","), table.concat(W, ","), table.concat(C, ","))
    local want = PURE.parse_play2(line, nil)
    local got = PURE.play_from_out({ peer_id = 5, seq = 12, mode = "interp", cut = 3, pt = 1000.25, age = 30, delay = 40,
        jit = 2.5, lead = 4, st = 8, iv = 16.7, k = 1, v2 = true, root = { 1, 2, 3, 90 }, m = m, vm = m, B = B, W = W, C = C })
    T.check(want and got, "both decode", T.repr({ want ~= nil, got ~= nil }))
    for _, k in ipairs({ "seq", "pt", "mode", "age", "delay", "jit", "cut", "lead", "iv", "st", "nbones" }) do
        T.check(want[k] == got[k], "field " .. k, T.repr({ want[k], got[k] }))
    end
    local function eq(x, y)
        if type(x) ~= "table" or type(y) ~= "table" then return x == y end
        for k, v in pairs(x) do if not eq(v, y[k]) then return false end end
        for k in pairs(y) do if x[k] == nil then return false end end
        return true
    end
    T.check(eq(want.slots, got.slots) and eq(want.weapons, got.weapons) and eq(want.root, got.root)
        and eq(want.control, got.control),
        "slots, weapons, root and control identical", T.repr({ want.root, got.root, want.control and #want.control, got.control and #got.control, want.weapons, got.weapons }))
    B[39] = nil
    T.check(PURE.play_from_out({ seq = 1, m = m, B = B }) == nil, "inconsistent mask / B: nil (no evidence)")
end
