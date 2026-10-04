-- HSMPWorld world state ("both screens show the same world"), offline:
--
--     hsmp-tools lua-test world_state
--
-- 1. Consistency codec: the same golden vectors as server/src/world.rs
--    (pack_quat, body_hash, world_hash), helpers (state kinds, bits, angular
--    velocity, verdict parser).
-- 2. Initial-state forcing: proposals after the settle window, Ready waits
--    for the authority's anchors, anchors are forced EXACTLY (not only past
--    the 10 cm drift rule) before Ready (compute_ready's result).
-- 3. Door / hinge lease: a hinged body owned by a peer is followed
--    kinematically (physics off, interpolated hinge angle); when the lease
--    ends it gets its physics back with the owner's angular velocity.
-- 4. Actor states: server bits are applied (BreakConstraint, the gate's
--    "Activate Event", the trap's "Event Activate Trap"), local breaks and
--    flag edges are reported (CLAIM_STATE), destroyed actors are dropped.
-- 5. World-guard safety: nothing is driven while a level change is pending;
--    a world drop forgets every object without touching it and clears the
--    world_held bus key; no call ever reaches an invalid (freed) object.
-- 6. Consistency report record + verdict -> world_consistency event + heal.
-- Everything crosses the typed record mock (lib/hsmp_native_records.lua): sends and
-- puts are read back from it, the sidecar's slots are written with sc_put.

local T = T
local MODS = T.path("mods")
local WORLD = MODS .. "/HSMPWorld/Scripts/main.lua"
local STATE = T.tmpdir("hsmpworld_state")
local real_getenv = os.getenv
os.getenv = function(k) if k == "HSMP_STATE_DIR" then return STATE end return real_getenv(k) end

package.preload["UEHelpers"] = function() return {} end
package.path = MODS .. "/shared/?.lua;" .. MODS .. "/HSMPWorld/Scripts/?.lua;" .. package.path
HSMP_WORLD_TEST = true
function FName(s) return s end
local H = assert(load(T.read(WORLD), "@HSMPWorld/Scripts/main.lua"))()
T.check(H ~= nil and H.W2 ~= nil and H.K ~= nil, "HSMPWorld exposes its WORLD-2 test hooks")
local K = H.K
H.set_name_none("None")
local events = {}
K.HL = { event = function(name, f) events[#events + 1] = { name = name, f = f } end }
-- The typed record mock behind the facade (sends, puts, the sidecar's slots).
local N = HSMP_IPC and HSMP_IPC.N
T.check(N and N.sc_put and N._rec, "the typed record mock is installed")
local function claim_lines()
    local out = {}
    for _, m in ipairs(N._rec.sends) do if m.kind == "world_claim" then out[#out + 1] = m.data end end
    return out
end
-- compute_ready's result (the old .world_ready.json fields).
local last_rf
do
    local real_cr = H.compute_ready
    H.compute_ready = function(now)
        local ready, complete, c = real_cr(now)
        local Wn = H.W()
        last_rf = { ready = ready, complete = complete, reason = Wn and Wn.ready_reason, epoch = Wn and Wn.epoch }
        for k, v in pairs(c or {}) do last_rf[k] = v end
        return ready, complete, c
    end
end

-- ---------------------------------------------------------------------------
T.log("== 1. codec (golden vectors shared with server/src/world.rs)")
local r2 = math.sqrt(0.5)
T.check(H.pack_quat({ 0, 0, 0, 1 }) == (3 << 30 | 512 << 20 | 512 << 10 | 512), "identity packs like Rust")
T.check(H.pack_quat({ 0, 0, r2, r2 }) == (2 << 30 | 512 << 20 | 512 << 10 | 1023), "yaw 90: tie keeps the first largest")
T.check(H.pack_quat({ 0 / 0, 0, 0, 0 }) == H.pack_quat({ 0, 0, 0, 1 }), "garbage -> identity")
T.check(H.pack_quat({ 0, 0, -r2, -r2 }) == H.pack_quat({ 0, 0, r2, r2 }), "double cover")
T.check(H.body_hash(10, { X = 1.5, Y = -2.4, Z = 3.0 }, 7, 3) == 1760769165, "body_hash golden (world.rs GOLD_BODY_HASH)",
    tostring(H.body_hash(10, { X = 1.5, Y = -2.4, Z = 3.0 }, 7, 3)))
local gold = { { id = 10, pos = { X = 1.5, Y = -2.4, Z = 3.0 }, q = 7, status = 3 },
               { id = 11, pos = { X = 0, Y = 0, Z = 0 }, q = 0, status = 0 } }
T.check(H.world_hash(gold) == 3136834322, "world_hash golden (world.rs GOLD_WORLD_HASH)", tostring(H.world_hash(gold)))
T.check(H.world_hash({}) == 2166136261, "empty world hash = FNV offset")

-- UE FRotator::Quaternion (as in hsmpworld.lua)
local function rq(p, y, r)
    local h = math.pi / 360
    local sp, cp = math.sin(p * h), math.cos(p * h)
    local sy, cy = math.sin(y * h), math.cos(y * h)
    local sr, cr = math.sin(r * h), math.cos(r * h)
    return { cr * sp * sy - sr * cp * cy, -cr * sp * cy - sr * cp * sy, cr * cp * sy - sr * sp * cy, cr * cp * cy + sr * sp * sy }
end
local w = H.ang_vel_deg(rq(0, 0, 0), rq(0, 10, 0), 100)
T.check(math.abs(w.Z - 100) < 0.01 and math.abs(w.X) < 1e-6 and math.abs(w.Y) < 1e-6,
    "door swinging 10 deg in 100 ms about Z = 100 deg/s", T.repr(w))
local w2 = H.ang_vel_deg(rq(0, 170, 0), rq(0, -170, 0), 200)
T.check(math.abs(w2.Z - 100) < 0.01, "short way across +-180 (20 deg / 0.2 s)", T.repr(w2))
T.check(H.ang_vel_deg(rq(0, 5, 0), rq(0, 5, 0), 50).Z == 0, "no rotation, no spin")

local kinds = {
    ST_Lever_C = "lever", ST_LeverActivated_Child_C = "gate", Trap_BP_C = "trap",
    BP_Container_Chest_003_C = "chest", BP_Fence_Flimsy_Small_C = "fence",
    BP_Structure_Plank_Destructible_3M_C = "destructible", BP_Structure_Board_Destructible_1X6M_C = "destructible",
    BP_Barrel_Destructable_Constraits_C = "destructible", Chain_BP_C = "constrained", Trap_Kettle_BP_C = "constrained",
    BP_Prop_Cooking_Utencil_Pot_001_C = "constrained", StaticMeshActor = false, BP_Candle_C = false,
    BP_Env_Arena_LordHall_Door_A_001_C = false,   -- static meshes, no physics: not state-bearing
}
for cls, k in pairs(kinds) do
    T.check((H.state_kind(cls) or false) == k, "state_kind " .. cls, tostring(H.state_kind(cls)))
end
T.check(H.attach_ok("BP_Structure_Trap_3_C") and H.attach_ok("Trap_BP_C") and H.attach_ok("BP_Structure_Test_C"),
    "barricade / trap pieces are scene bodies")
T.check(not H.attach_ok("Willie_BP_C") and not H.attach_ok("BP_Prop_Light_Sconce_001_C") and not H.attach_ok(nil),
    "held weapons and sconce candles stay excluded")
T.check(H.group_bits({ true, false, true }, true) == (1 | 4 | 0x40), "group bits: constraints 1 and 3 broken + flag")
local brk, flag = H.state_todo(0x40 | 0x05, 0x01)
T.check(T.eq(brk, { 3 }) and flag == true, "todo: break constraint 3, raise flag", T.repr(brk))
local c = H.W2.consistency_from({ level = 5, epoch = 4, seq = 9, other = 2, hash_match = false, hash_equal = false, compared = 10,
    mismatched_n = 2, rows = { { id = 77, kind = 1, dpos = 12.5, dang = 4.0 }, { id = 78, kind = 2, dpos = 0, dang = 0 } } })
T.check(c.level == 5 and c.seq == 9 and c.peer == 2 and not c.hash_match and c.compared == 10 and #c.rows == 2
    and c.rows[1].id == 77 and c.rows[1].kind == 1 and c.rows[1].dpos == 12.5 and c.rows[2].kind == 2,
    "verdict record (world_verdict) -> the reader's shape", T.repr(c))

-- ---------------------------------------------------------------------------
-- UE mocks: objects that record calls and flag any use after invalidation.
local violations, calls = {}, {}
local addr = 0x1000
local function record(...) calls[#calls + 1] = table.concat({ ... }, " ") end
local function mk(kind, name, props, methods)
    addr = addr + 0x10
    local o = { _valid = true, _name = name, _addr = addr, _props = props or {} }
    local m = methods or {}
    m.IsValid = m.IsValid or function(self) return rawget(self, "_valid") end
    m.GetAddress = m.GetAddress or function(self) return rawget(self, "_addr") end
    m.GetFName = m.GetFName or function(self) local n = rawget(self, "_name"); return { ToString = function() return n end } end
    m.GetFullName = m.GetFullName or function(self) return kind .. " " .. rawget(self, "_name") end
    return setmetatable(o, {
        __index = function(self, k)
            local f = m[k]
            if f then
                return function(me, ...)
                    -- _valid=false is "pending kill" (IsValid is the only safe
                    -- call); _freed is "freed" (a level change): even IsValid crashes.
                    if rawget(me, "_freed") or (k ~= "IsValid" and not rawget(me, "_valid")) then
                        violations[#violations + 1] = "call " .. k .. " on freed " .. name
                    end
                    return f(me, ...)
                end
            end
            if not rawget(self, "_valid") then violations[#violations + 1] = "read " .. tostring(k) .. " on freed " .. name end
            return rawget(self, "_props")[k]
        end,
        __newindex = function(self, k, v)
            if not rawget(self, "_valid") then violations[#violations + 1] = "write " .. tostring(k) .. " on freed " .. name end
            rawget(self, "_props")[k] = v
        end,
    })
end
local function v3(x, y, z) return { X = x, Y = y, Z = z } end
local function body(name, pos, yaw, sim)
    local b
    b = mk("StaticMeshComponent", name, {}, {
        IsSimulatingPhysics = function(self) return rawget(self, "_sim") end,
        SetSimulatePhysics = function(self, on) rawset(self, "_sim", on); record("sim", name, tostring(on)) end,
        K2_GetComponentLocation = function(self) local p = rawget(self, "_pos"); return v3(p.X, p.Y, p.Z) end,
        K2_GetComponentRotation = function(self) local r = rawget(self, "_rot"); return { Pitch = r.Pitch, Yaw = r.Yaw, Roll = r.Roll } end,
        K2_SetWorldLocationAndRotation = function(self, p, r, sweep, hit, tp)
            if sweep ~= false or tp ~= true then violations[#violations + 1] = "teleport with sweep / without bTeleport" end
            rawset(self, "_pos", v3(p.X, p.Y, p.Z)); rawset(self, "_rot", { Pitch = r.Pitch, Yaw = r.Yaw, Roll = r.Roll })
            record("tp", name, string.format("%.2f,%.2f,%.2f,%.2f", p.X, p.Y, p.Z, r.Yaw))
        end,
        SetPhysicsLinearVelocity = function(self, v) rawset(self, "_vel", v3(v.X, v.Y, v.Z)) end,
        SetPhysicsAngularVelocityInDegrees = function(self, v) rawset(self, "_angvel", v3(v.X, v.Y, v.Z)); record("angvel", name) end,
        PutRigidBodyToSleep = function() record("sleep", name) end,
        GetPhysicsLinearVelocity = function(self) local v = rawget(self, "_vel") or v3(0, 0, 0); return v3(v.X, v.Y, v.Z) end,
    })
    rawset(b, "_pos", v3(pos.X, pos.Y, pos.Z)); rawset(b, "_rot", { Pitch = 0, Yaw = yaw or 0, Roll = 0 }); rawset(b, "_sim", sim ~= false)
    return b
end
local function actor(name, props, methods)
    local m = methods or {}
    m.GetAttachParentActor = m.GetAttachParentActor or function() return nil end
    m.K2_GetActorLocation = m.K2_GetActorLocation or function() return v3(0, 0, 0) end
    return mk("Actor", name, props, m)
end
local function constraint(name)
    return mk("PhysicsConstraintComponent", name, {}, {
        IsBroken = function(self) return rawget(self, "_broken") == true end,
        BreakConstraint = function(self) rawset(self, "_broken", true); record("break", name) end,
    })
end
local function has_call(s) for _, x in ipairs(calls) do if x == s then return true end end return false end
local function count_calls(prefix) local n = 0; for _, x in ipairs(calls) do if T.startswith(x, prefix) then n = n + 1 end end; return n end

-- A level as finish_scan + manifest binding leave it.
H.sess.connected, H.sess.my_id = true, 1
H.new_level("World /Game/Maps/Arenas/Map_Arena_Alley.Map_Arena_Alley", 0x1234, 0)
local L = H.W()
L.synced, L.epoch, L.scans_done, L.mine_awake = true, 3, 4, {}
local function add_body(nid, name, pos, yaw, sim)
    local a = actor(name .. "_A")
    local b = body(name, pos, yaw, sim)
    local o = { lid = nid, nid = nid, chash = 1, cls = "StaticMeshActor", comp = "SM", kind = "sma", actor = a,
                actor_addr = a:GetAddress(), body = b, sim0 = sim ~= false, sim = sim ~= false,
                spawn_pos = v3(pos.X, pos.Y, pos.Z), spawn_rot = { Pitch = 0, Yaw = yaw or 0, Roll = 0 },
                anchor_pos = v3(pos.X, pos.Y, pos.Z), anchor_rot = { Pitch = 0, Yaw = yaw or 0, Roll = 0 },
                dyn_owner = 0, buf = {}, prio = 0, last_read = -999, last_probe = -999 }
    o.pos, o.rot, o.vel = o.spawn_pos, o.spawn_rot, v3(0, 0, 0)
    L.objs[#L.objs + 1] = o
    L.by_lid[nid], L.by_nid[nid] = o, o
    return o, b, a
end
local function remote(rows)
    N.sc_put("world_remote", { level = L.level, epoch = 3, rows = rows })
end
local function anchor_row(id, x, y, z, yaw, flags)
    return { sender = 0, seq = 0, ts = 0, obj = H.W2.qobj(id, v3(x, y, z), { Pitch = 0, Yaw = yaw, Roll = 0 }, nil, flags or 9) }
end
local function hash_rec() return N.sc_get("world_hash") end

-- ---------------------------------------------------------------------------
T.log("== 2. initial state: propose, wait, force exactly, Ready")
local cup, cupb = add_body(101, "Cup", v3(100, 0, 50), 0)
local ready, complete = H.compute_ready(1000)
local rf = last_rf
T.check(not ready and rf and rf.ready == false and rf.reason == "scan", "not ready before the init window", T.repr(rf))
H.service_init(1000)
T.check(L.init_t0 == 1000 and #claim_lines() == 0, "init started; nothing proposed before the body is known still")
H.service_init(1200)
H.service_init(1500)
T.check(#claim_lines() == 0, "still for 300 ms: not yet")
H.service_init(1900)
local cl = claim_lines()
local prop = cl[1]
T.check(#cl == 1 and prop.mode == 5 and prop.id == 101 and prop.epoch == 3 and prop.has_rest == true
    and prop.rest.pos[1] == 100 and prop.rest.pos[3] == 50 and (prop.rest.flags & 1) == 1,
    "settled 600 ms: CLAIM_INIT with the rest pose (a WorldObj, asleep flag)", T.repr(prop))
H.service_init(2000)
T.check(#claim_lines() == 1, "proposed once per resend window")
ready = H.compute_ready(2000)
rf = last_rf
T.check(not ready and rf.reason == "init" and rf.epoch == 3 and rf.bound == 1 and rf.anchored == 0,
    "Ready waits for the authority's anchor", T.repr(rf))
-- The authority (another client) settled the cup 2 cm away, turned 4 deg.
remote({ anchor_row(101, 102, 0, 50, 4) })
H.ingest_remote(2050)
T.check(cup.anchor_new and cup.force_exact, "initial anchor marked for exact forcing")
T.check(not H.compute_ready(2060), "anchored but not applied yet: still not ready")
H.update_body(cup, 2066, false)
local p = cupb:K2_GetComponentLocation()
local r = cupb:K2_GetComponentRotation()
T.check(math.abs(p.X - 102) < 1e-3 and math.abs(r.Yaw - 4) < 0.01 and has_call("sleep Cup"),
    "2 cm / 4 deg off: forced exactly onto the anchor and put to sleep (not left to the 10 cm drift rule)",
    T.repr(p) .. " yaw " .. tostring(r.Yaw))
ready, complete = H.compute_ready(2100)
rf = last_rf
T.check(ready and complete and rf.ready == true and rf.complete == true and rf.reason == "ok" and rf.forced == 1,
    "compute_ready: ready, complete", T.repr(rf))
T.check(L.ready, "W.ready latched")
local tps = count_calls("tp Cup")
remote({ anchor_row(101, 104, 0, 50, 4) })
H.ingest_remote(2200)
H.update_body(cup, 2216, false)
p = cupb:K2_GetComponentLocation()
T.check(count_calls("tp Cup") == tps + 1 and math.abs(p.X - 104) < 1e-3,
    "after Ready an untouched, server-anchored body 2 cm off its anchor is pinned onto it (WORLD-1: settled props stay identical)", T.repr(p))
H.update_body(cup, 2232, false)
T.check(count_calls("tp Cup") == tps + 1, "on the anchor: no further pin")
-- A body the authority never anchors: Ready gives up waiting after INIT_WAIT_MS.
local lone = add_body(102, "Lone", v3(300, 0, 50), 0)
L.ready = false
T.check(not H.compute_ready(3000), "unanchored body inside the wait window keeps Ready back")
ready, complete = H.compute_ready(1000 + K.INIT_WAIT_MS + 1)
T.check(ready and not complete and last_rf.unanchored == 1, "after INIT_WAIT_MS: ready, complete=false, counted")
lone.dead = true
L.ready = true

-- ---------------------------------------------------------------------------
T.log("== 3. hinge lease: follow kinematically, hand back with the owner's spin")
local door, doorb = add_body(201, "LeverHandle", v3(500, 0, 100), 0)
L.owners[201] = { owner = 2, ver = 5, mode = 1 }
local q0, q1 = rq(0, 0, 0), rq(0, 10, 0)
door.buf = { { t = 1000, pos = v3(500, 0, 100), q = q0, vel = v3(0, 0, 0), flags = 8 },
             { t = 1100, pos = v3(500, 0, 100), q = q1, vel = v3(0, 0, 0), flags = 8 } }
door.buf_sender = 2
-- the sender's clock puts the delayed timeline at 1050 for now = 5000; the follower starts
-- from the local pose and blends onto the stream (frames at a standing clock)
L.fclock[2] = { off = 5000 - 1050 - 80, delay = 80, delay_fast = 80, hist = {} }
L.dt = 16
for _ = 1, 40 do H.update_body(door, 5000, false) end
r = doorb:K2_GetComponentRotation()
T.check(door.kin and not rawget(doorb, "_sim") and math.abs(r.Yaw - 5) < 0.05,
    "peer-owned hinge: physics off, hinge angle interpolated (5 deg at mid-sample, blended in)", "yaw " .. tostring(r.Yaw))
L.owners[201] = { owner = 0, ver = 6, mode = 0 }
door.settled = false
H.update_body(door, 5100, false)
local av = rawget(doorb, "_angvel")
T.check(not door.kin and rawget(doorb, "_sim") == true and av and math.abs(av.Z - 100) < 0.5,
    "lease over mid-swing: physics back on with the owner's 100 deg/s", T.repr(av))
door.anchor_key = "released"   -- its release pose became its anchor

-- ---------------------------------------------------------------------------
T.log("== 4. actor states: server-ordered breaks, gates, traps, flags")
local function add_state(nid, cls, skind, cons, props, methods)
    local a = actor(cls .. "_0", props, methods)
    local r = { lid = nid, nid = nid, chash = 2, cls = cls, comp = "A0", kind = "state", skind = skind, group = 0,
                cons = cons, actor = a, actor_addr = a:GetAddress(), spawn_pos = v3(10, 20, 30), dyn_owner = 0,
                changed_at = 0, sent_at = -1e9, apply_tries = 0 }
    L.actors[#L.actors + 1] = r
    L.by_lid[nid], L.by_nid[nid] = r, r
    return r, a
end
-- Plank: constraint 2 ("PhysicsConstraint", its own breaking joint) broke on another screen.
local pc0, pc1 = constraint("NODE_AddPhysicsConstraintComponent-0"), constraint("PhysicsConstraint")
local plank = add_state(301, "BP_Structure_Plank_Destructible_3M_C", "destructible",
    { { "NODE_AddPhysicsConstraintComponent-0", pc0 }, { "PhysicsConstraint", pc1 } })
L.owners[301] = { owner = 2, ver = 9, mode = K.MODE_STATE | 2 }
local before = #claim_lines()
H.service_states(6000)
T.check(has_call("break PhysicsConstraint") and not has_call("break NODE_AddPhysicsConstraintComponent-0"),
    "the server's break is applied to exactly that constraint")
H.service_states(6250)
T.check(plank.loc_bits == 2 and #claim_lines() == before and count_calls("break PhysicsConstraint") == 1,
    "applied once; local now equals the record, nothing reported")
-- A break that happens here is reported, sticky bits merged.
rawset(pc0, "_broken", true)
H.service_states(6500)
cl = claim_lines()
local st = cl[#cl]
T.check(st.mode == 4 and st.id == 301 and st.rest.vel[1] == 3, "local break -> CLAIM_STATE bits 3 (rest.vel[1])", T.repr(st))
L.owners[301] = { owner = 1, ver = 13, mode = K.MODE_STATE | 3 }   -- the server merged it
-- Lever-gated block: the gate's own "Activate Event", not a bare BreakConstraint.
local g0, g1 = constraint("PhysicsConstraint"), constraint("PhysicsConstraint1")
local activated = 0
local gate = add_state(302, "ST_LeverActivated_Child_C", "gate", { { "PhysicsConstraint", g0 }, { "PhysicsConstraint1", g1 } },
    {}, { ["Activate Event"] = function() activated = activated + 1; rawset(g1, "_broken", true) end })
L.owners[302] = { owner = 2, ver = 10, mode = K.MODE_STATE | 2 }
H.service_states(7000)
T.check(activated == 1 and not has_call("break PhysicsConstraint1"), "gate opened through its BP Activate Event")
-- Trap armed elsewhere (lever pulled on the other screen).
local weapon = mk("Actor", "BP_Weapon_Trap_C_0", { ["Simulates Physics"] = false })
local armed = 0
local trap, trapa = add_state(303, "Trap_BP_C", "trap", {}, { ["As Modular Weapon BP"] = weapon },
    { ["Event Activate Trap"] = function() armed = armed + 1; weapon["Simulates Physics"] = true end })
L.owners[303] = { owner = 2, ver = 11, mode = K.MODE_STATE | K.ST_FLAG }
H.service_states(7250)
T.check(armed == 1, "trap armed through its BP Event Activate Trap")
H.service_states(7500)
T.check(armed == 1 and trap.loc_bits == K.ST_FLAG, "armed once")
-- Lever: the flag is reported on a local edge only.
local lever, levera = add_state(304, "ST_Lever_C", "lever", {}, { Loaded = false })
H.service_states(8000)
local n0 = #claim_lines()
levera.Loaded = true
H.service_states(8250)
cl = claim_lines()
st = cl[#cl]
T.check(#cl == n0 + 1 and st.id == 304 and st.rest.vel[1] == K.ST_FLAG, "lever pulled here -> flag reported", T.repr(st))
L.owners[304] = { owner = 1, ver = 12, mode = K.MODE_STATE | K.ST_FLAG }
H.service_states(8500)
T.check(#claim_lines() == n0 + 1 and not lever.flag_dirty, "server agrees: no more reports")
-- Ready also waits for unapplied actor states.
local f2 = constraint("PhysicsConstraint")
local fence = add_state(306, "BP_Fence_Flimsy_Big_C", "fence", { { "PhysicsConstraint", f2 } })
fence.loc_bits = 0
L.owners[306] = { owner = 2, ver = 14, mode = K.MODE_STATE | 1 }
L.ready = false
H.compute_ready(8600)
T.check(not L.ready and last_rf.states_pending == 1, "a server break not applied yet keeps Ready back")
H.service_states(8700)
H.service_states(8950)
H.compute_ready(8960)
T.check(L.ready and fence.loc_bits == 1, "applied -> ready")
-- A destroyed actor is dropped without another touch.
rawset(trapa, "_valid", false)
local nv = #violations
H.service_states(9000)
T.check(trap.dead and trap.actor == nil and #violations == nv, "destroyed trap dropped, never touched again", T.repr(violations))

-- ---------------------------------------------------------------------------
T.log("== 6. consistency report + verdict")
L.hash_next = 0
H.hash_report(10000)
local hr = hash_rec()
T.check(hr and hr.level == L.level and hr.epoch == 3 and hr.seq == 1, "report head", T.repr(hr))
local rows = {}
for _, row in ipairs(hr.rows) do rows[row.id] = row end
T.check(rows[101] and rows[101].status == 1, "first report: body alive but not yet known still")
H.hash_report(14000)
T.check(hash_rec().seq == 1, "rate: one report per HASH_EVERY_MS")
H.hash_report(15001)
hr = hash_rec()
rows = {}
for _, row in ipairs(hr.rows) do rows[row.id] = row end
T.check(rows[101].status == 3 and rows[101].pos[1] == 104 and rows[101].q == H.pack_quat(rq(0, 4, 0)),
    "settled free body: {id, ver, status alive|settled, pos, q packed quat}", T.repr(rows[101]))
T.check(rows[301] and rows[301].status == (4 | 1 | 2) and rows[301].q == 3, "actor state row: HS_STATE|alive|settled, bits", T.repr(rows[301]))
T.check(rows[303] and rows[303].status == 4, "destroyed actor: state row, not alive")
-- The client hash covers exactly the settled + missing rows, like world.rs.
local hrows = {}
for _, row in ipairs(hr.rows) do
    local s = row.status & 3
    if s == 3 or (s & 1) == 0 then
        hrows[#hrows + 1] = { id = row.id, pos = v3(row.pos[1], row.pos[2], row.pos[3]), q = row.q, status = row.status }
    end
end
table.sort(hrows, function(a, b) return a.id < b.id end)
T.check(hr.hash == H.world_hash(hrows), "client hash = world_hash(settled + missing rows)")
-- Verdict: the cup differs by 12 cm on the other screen -> event + forced back.
rawset(cupb, "_pos", v3(110, 0, 50))
cup.pos = v3(110, 0, 50)
N.sc_put("world_consistency", { level = L.level, epoch = 3, seq = 2, other = 2, hash_match = false, hash_equal = false,
    compared = 7, mismatched_n = 1, rows = { { id = 101, kind = 1, dpos = 12.0, dang = 0.0 } } })
H.read_consistency(16000)
local e = events[#events]
T.check(e and e.name == "world_consistency" and e.f.hash_match == false and T.eq(e.f.mismatched, { 101 })
    and e.f.compared == 7 and e.f.peer == 2, "world_consistency{hash_match=false, mismatched={101}}", T.repr(e and e.f))
T.check(cup.force_exact and cup.anchor_new, "mismatched free body queued for exact re-forcing")
H.update_body(cup, 16016, false)
T.check(math.abs(cupb:K2_GetComponentLocation().X - 104) < 1e-3, "forced back onto its anchor")
N.sc_put("world_consistency", { level = L.level, epoch = 3, seq = 3, other = 2, hash_match = true, hash_equal = true,
    compared = 7, mismatched_n = 0, rows = {} })
H.read_consistency(21000)
e = events[#events]
T.check(e.f.hash_match == true and T.eq(e.f.mismatched, {}) and e.f.hash_equal == true, "matching verdict event")
H.read_consistency(21500)
T.check(events[#events] == e, "one event per verdict")

-- ---------------------------------------------------------------------------
T.log("== 5. world-guard safety")
H.WG.travel_from = "World /Game/Maps/Arenas/Map_Arena_Alley.Map_Arena_Alley@1#PC"
local g2 = constraint("FenceJoint")
add_state(305, "BP_Fence_Flimsy_Small_C", "fence", { { "FenceJoint", g2 } })
L.owners[305] = { owner = 2, ver = 20, mode = K.MODE_STATE | 1 }
H.service_states(22000)
T.check(not has_call("break FenceJoint"), "no BreakConstraint while a level change is pending")
local ntp = count_calls("tp ")
cup.anchor_new, cup.force_exact = true, true
rawset(cupb, "_pos", v3(150, 0, 50))
H.update_body(cup, 22016, false)
T.check(count_calls("tp ") == ntp, "no teleport while a level change is pending")
H.WG.travel_from = nil
-- Level change: every object of the old world becomes invalid at once; the
-- guard drops HSMPWorld's state without touching any of them.
local function free(x) rawset(x, "_valid", false); rawset(x, "_freed", true) end
for _, o in ipairs(L.objs) do free(o.actor); free(o.body) end
for _, a in ipairs(L.actors) do
    if a.actor then free(a.actor) end
    for _, cc in ipairs(a.cons) do if cc[2] then free(cc[2]) end end
end
nv = #violations
H.WG.travel("level change requested")
local held = N.sc_get("world_held")
T.check(H.W() == nil and held and #held.rows == 0, "world drop: state forgotten, world_held cleared", T.repr(held))
T.check(#violations == nv, "nothing of the freed world was touched", T.repr(violations))
T.check(#violations == 0, "no call/read/write on a freed object in the whole suite", T.repr(violations))

-- ---------------------------------------------------------------------------
T.log("== 7. discovery: barricade pieces (attached to BP_Structure_Trap_*) + actor states")
local classes = {}
local function cls(name)
    classes[name] = classes[name] or mk("Class", name, {}, {})
    return classes[name]
end
local LEVELP = "/Game/Maps/Arenas/Map_Arena_Alley.Map_Arena_Alley:PersistentLevel"
local level = mk("Level", "PersistentLevel", {}, {
    GetClass = function() return cls("Level") end,
    GetFullName = function() return "Level " .. LEVELP end,
})
function StaticFindObject(path) return cls(path) end
local function piece(cname, name, parent_cls, parts, cons)
    local parent = parent_cls and mk("Actor", parent_cls .. "_0", {}, { GetClass = function() return cls(parent_cls) end })
    local comps = {}
    for i, pn in ipairs(parts) do
        local b = body(name .. "." .. pn, v3(1000 + 10 * i, 0, 100), 0, false)
        rawget(b, "_props").Mobility = 2
        comps[i] = b
    end
    return mk("Actor", name, {}, {
        GetClass = function() return cls(cname) end,
        GetOuter = function() return level end,
        HasAnyFlags = function() return false end,
        GetAttachParentActor = function() return parent end,
        K2_GetActorLocation = function() return v3(1000, 0, 100) end,
        K2_GetComponentsByClass = function(_, c)
            local list = c == cls("/Script/Engine.PhysicsConstraintComponent") and cons or comps
            return { ForEach = function(_, fn) for i, x in ipairs(list) do fn(i, { get = function() return x end }) end end }
        end,
    })
end
local held_by_willie = piece("BP_Structure_Plank_Destructible_3M_C", "Beam_Held", "Willie_BP_C", { "Plank Part 1" }, {})
local beam = piece("BP_Structure_Plank_Destructible_3M_C", "Beam 3_GEN_VARIABLE_BP_Structure_Plank_Destructible_3M_C_CAT_1312",
    "BP_Structure_Trap_3_C", { "Plank Part 1", "Plank Part 2" },
    { constraint("PhysicsConstraint"), constraint("NODE_AddPhysicsConstraintComponent-1"), constraint("NODE_AddPhysicsConstraintComponent-0") })
function FindAllOf(c)
    if c == "BP_Structure_Plank_Destructible_Master_C" then return { beam, held_by_willie } end
    return nil
end
H.new_level("World /Game/Maps/Arenas/Map_Arena_Alley.Map_Arena_Alley", 0x5678, 0)
L = H.W()
H.start_scan()
for _ = 1, 10 do if L.scan then H.scan_step() end end
T.check(L.scan == nil and #L.objs == 2, "both planks of the barricade piece are bodies (v1 skipped them as attached)",
    tostring(#L.objs))
T.check(#L.actors == 1 and L.actors[1].skind == "destructible" and L.actors[1].comp == "A0", "one actor-state record")
local a1 = L.actors[1]
T.check(T.eq(T.map(a1.cons, function(x) return x[1] end),
    { "NODE_AddPhysicsConstraintComponent-0", "NODE_AddPhysicsConstraintComponent-1", "PhysicsConstraint" }),
    "constraints in FName order (bit k = constraint k on every client)")
local want = H.fnv1a("BP_Structure_Plank_Destructible_3M_C|A0|" .. LEVELP .. ":" .. "Beam 3_GEN_VARIABLE_BP_Structure_Plank_Destructible_3M_C_CAT_1312") & 0x7FFFFFFF
T.check(a1.lid == want and a1.chash == H.fnv1a("BP_Structure_Plank_Destructible_3M_C|A0"),
    "state id = the body id rule with comp A<g> (deterministic, level-placed name)")
T.check(not T.any(L.objs, function(o) return o.actor == held_by_willie end), "a piece attached to a Willie stays excluded")
-- Proposed with the manifest like bodies.
L.synced, L.epoch, L.manifest_seen, L.mout_dirty = true, 3, true, true
H.W2.compute_ready(0)
T.check(#violations == 0, "no freed-object access", T.repr(violations))
