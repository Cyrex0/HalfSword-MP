-- Offline tests for HSMPCombat's vitals + damage paths with mocked pawns.
--
--     hsmp-tools lua-test vitals
--
-- Loads mods/HSMPCombat/Scripts/main.lua (Lua 5.4) with the UE4SS
-- API mocked (Willie actors are plain Lua tables keyed by the REAL spaced
-- property names) and exercises, without the game:
--   * owner sampling -> the quantised `vitals` record (deadband, heartbeat,
--     dead flag, dismembered-part bitmask; quantisation round trip);
--   * stand-in mirror from the `peer_vitals` record (limb floors, Health pin,
--     stamina/bleeding/pain written, posture flags NOT written, severed bone
--     hidden, owner death -> stand-in Death);
--   * attacker claim (the Deal Complex Damage inputs, after-call hook) ->
--     owner replay: the NATIVE result only (solo parity, no top-up / trim);
--   * a stand-in's weapon echo on my pawn undone after the call + touch report.

local T = T
local STATE = T.tmpdir("hsmp_vitals_test_")
local SRC = T.path("mods/HSMPCombat/Scripts/main.lua")

-- ---- mock ---------------------------------------------------------------------
HSMP_COMBAT_TEST = {}
CLOCK = 100.0
os.clock = function() return CLOCK end
os.getenv = function(k) if k == "HSMP_STATE_DIR" then return STATE end return nil end
LOGS = {}
print = function(s) LOGS[#LOGS + 1] = s end
RegisterHook = function() end
LoopAsync = function() end
ExecuteInGameThread = function(f) f() end
StaticFindObject = function() return nil end
FindAllOf = function() return nil end
function FName(s) return { s = s, ToString = function(self) return self.s end } end

local addr = 1000
local function valid() return true end
function mk_mesh(bones)
    local m = { bones = bones or {}, hidden = {} }
    m.IsValid = valid
    m.GetBoneIndex = function(self, n) return self.bones[n.s] or -1 end
    m.HideBoneByName = function(self, n, opt) self.hidden[n.s] = opt end
    m.UnHideBoneByName = function(self, n) self.hidden[n.s] = nil end
    m.GetSocketLocation = function(self, n) return { X = 10, Y = 20, Z = 30 } end
    return m
end
function mk_arr(items)
    return { items = items, ForEach = function(self, fn)
        for i, n in ipairs(self.items) do fn(i, { get = function() return FName(n) end }) end
    end }
end
function mk_willie(name, vals)
    addr = addr + 1
    local w = { __name = name, __addr = addr, calls = {} }
    for k, v in pairs(vals) do w[k] = v end
    w.IsValid = valid
    w.GetAddress = function(self) return self.__addr end
    w.GetFName = function(self) return FName(self.__name) end
    w.GetClass = function(self) return { GetFName = function() return FName("Willie_BP_C") end } end
    w.Mesh = mk_mesh({ head = 5, lowerarm_l = 9, hand_l = 10 })
    w["Dismembered Array"] = mk_arr({})
    w["Death"] = function(self) self.calls.death = (self.calls.death or 0) + 1; self.DED = true end
    return w
end
function full_vitals(over)
    local t = {
        Health = 100, ["Head Health"] = 100, ["Neck Health"] = 100, ["Body Upper Health"] = 100,
        ["Body Lower Health"] = 100, ["Back Health"] = 100, ["Arm_R Health"] = 100, ["Arm_L Health"] = 100,
        ["Leg_R Health"] = 100, ["Leg_L Health"] = 100, ["Head Health (Crush)"] = 100,
        Consciousness = 100, ["Consciousness 2 (Legs)"] = 100, Stamina = 100, Exhaustion = 0,
        Bleeding = 0, ["Blood Rate"] = 1.5, Pain = 0, ["Fallen Rate"] = 0,
        ["Sustained Damage"] = 0, ["Damage Taken"] = 0, ["Last Damage Taken"] = 0,
        DED = false, Fallen = false, Downed = false, Headless = false, ["Pain Shock"] = false,
    }
    for k, v in pairs(over or {}) do t[k] = v end
    return t
end
-- A "Get Damage" that hurts by `scale` x raw: Health, the hit region, stamina.
function damage_fn(scale)
    return function(self, imp, vel, loc, nrm, bone, raw, cut, inside, mesh, dism, lower, shock, hbc,
                    stab, hb, pain, flesh, draw, out)
        self.calls.gd = (self.calls.gd or 0) + 1
        local r = raw * scale
        self.Health = self.Health - r * 0.5
        if bone and bone.s == "head" then self["Head Health"] = self["Head Health"] - r end
        self.Stamina = self.Stamina - 4 * scale
        self.Pain = self.Pain + 10 * scale
    end
end

ME = mk_willie("Willie_BP_C_ME", full_vitals())
PC = { Pawn = ME, IsValid = valid }
package.preload["UEHelpers"] = function()
    return { GetPlayerController = function() return PC end, GetWorld = function() return nil end }
end

-- ---- load -----------------------------------------------------------------------
-- shared/hsmp_wg.lua (HSMPCombat's world guard; deploy copies it into Scripts/)
package.path = T.path("mods/shared") .. "/?.lua;" .. package.path
local fn, err = load(T.read(SRC))
T.check(fn ~= nil, "HSMPCombat loads (Lua 5.4)", err)
if fn == nil then return end
fn()
local api = HSMP_COMBAT_TEST.api
T.check(api ~= nil, "test api exported")
api.set_world("world#1")

local function write(name, text) T.write(STATE .. "/" .. name, text) end
-- Typed session state: the sidecar's `link` record + the server's `session`
-- record in the native mock, and the header heartbeat that makes a sidecar live.
local NAT = rawget(_G, "HSMPNative")
local IPCF = rawget(_G, "HSMP_IPC")
local ES = IPCF.S.ENUMS
local function hb(age) NAT._st.hb_age = age; IPCF.refresh_info(true) end
hb(0.01)
local function sidecar(status, id) NAT.sc_put("link", { status = ES.sidecar_status[status:upper()], state = ES.link_state.UP, my_peer_id = id }) end
local PHASE = { lobby = 0, countdown = 2, live = 3, roundover = 4, match_over = 5, paused = 7 }
local SESS = { phase = 0, round = 0, winner_seat = 255, rows = {} }
local function publish() NAT.sc_put("session", SESS) end
local function match(state, round) SESS.phase, SESS.round = PHASE[state], round; publish() end
local function read(name) return T.read(STATE .. "/" .. name) end

match("live", 1)
api.refresh_match()
-- the session is up (outbox protocol: nothing is written before "connected")
sidecar("connected", 1)
api.refresh_session()   -- the first read only seeds the liveness baseline
sidecar("connected", 1)
api.refresh_session()   -- one observed heartbeat: live
T.check(api.state().combat_window == true, "match live")

-- ---- property-name sanity ------------------------------------------------------
local props = T.read(T.path("docs/development/halfsword/willie_props.txt"))
local real = T.set(T.re_findall("(?m)^  (.+?) = ", props))
local vit, fld = {}, {}
for i = 1, #api.VITALS do vit[i] = api.VITALS[i][1] end
for i = 1, #api.FIELDS do fld[i] = api.FIELDS[i][1] end
local missing = {}
local names = {}
for _, n in ipairs(vit) do names[#names + 1] = n end
for _, n in ipairs(fld) do names[#names + 1] = n end
for _, n in ipairs({ "DED", "Fallen", "Downed", "Headless", "Pain Shock", "Dismembered Array" }) do names[#names + 1] = n end
for _, n in ipairs(names) do if not real[n] then missing[#missing + 1] = n end end
T.check(#missing == 0, "every vitals/field name is a real Willie_BP_C property", T.repr(missing))
T.check(#vit == 19, "19 vitals scalars (= schema/combat.rs VITALS_N)", #vit)
-- The ONE definition of the order: crates/hsmp-ipc/src/schema/combat.rs VITALS_NAMES.
local rs = T.read(T.path("crates/hsmp-ipc/src/schema/combat.rs")) or ""
local rs_names = {}
local block = rs:match("VITALS_NAMES: %[&str; VITALS_N%] = %[(.-)%];") or ""
for n in block:gmatch('"([^"]+)"') do rs_names[#rs_names + 1] = n end
T.check(T.eq(rs_names, vit), "Lua VITALS order == Rust VITALS_NAMES order", T.repr(rs_names) .. " vs " .. T.repr(vit))

-- ---- typed records: the native mock's sidecar side ----------------
local N = HSMPNative
local S = rawget(_G, "HSMP_IPC").S
local DP = S.ENUMS.dism_part
local RL = dofile(T.path("tools/hsmp-tools/lua-tests/lib/hsmp_native_records.lua"))
local SENT = {}
local function sent(kind)
    for _, m in ipairs(N.sc_rec_drain()) do SENT[#SENT + 1] = m end
    local out = {}
    for _, m in ipairs(SENT) do if m.kind == kind then out[#out + 1] = m.data end end
    return out
end
local function q(x) return x == nil and 65535 or math.floor(x * 64 + 0.5) end

-- ---- owner sampling / publish: the quantised `vitals` record ------------------
api.set_tick(2)
local ok = api.publish_own_vitals(ME)
local rec = N.sc_get("vitals")
T.check(T.truthy(ok) and rec ~= nil, "first sample written")
do
    rec = rec or { v = {} }
    local all100 = true
    for i = 1, 13 do if rec.v[i] ~= 6400 then all100 = false end end
    T.check(rec.seq == 1 and rec.flags == 0 and rec.dism == 0 and type(rec.v) == "table" and #rec.v == 19
        and all100 and rec.v[14] == 6400 and rec.v[15] == 0 and rec.v[17] == 96,
        "vitals record: seq, flags, dism mask, v = round(x * 64)", T.repr(rec))
end
ME["Stamina"] = 99.7
CLOCK = 100.2
T.check(api.publish_own_vitals(ME) == false, "stamina -0.3: inside deadband, no write")
ME["Stamina"] = 99.4
T.check(api.publish_own_vitals(ME) == true, "stamina -0.6: written")
T.check(N.sc_get("vitals").v[14] == q(99.4), "stamina quantised (99.4 * 64 rounded)", N.sc_get("vitals").v[14])
CLOCK = 101.5
T.check(api.publish_own_vitals(ME) == true, "heartbeat after 1 s without change")
ME["Fallen"] = true
ME["Dismembered Array"]["items"][1] = "lowerarm_l"
api.set_tick(20)
api.publish_own_vitals(ME)
rec = N.sc_get("vitals")
T.check(rec.flags == 2 and rec.dism == (1 << DP.LOWERARM_L), "Fallen flag + severed part (bit LOWERARM_L) published", T.repr(rec))
ME["Health"] = 0
ME["DED"] = true
api.publish_own_vitals(ME)
rec = N.sc_get("vitals")
T.check(rec.flags == 3 and rec.v[1] == 0, "dead: DED + Health 0 -> flag bit 1, v[1] 0", T.repr(rec))
ME["Health"], ME["DED"], ME["Fallen"] = 100, false, false
ME["Dismembered Array"]["items"] = {}
api.set_tick(40)
api.publish_own_vitals(ME)

-- ---- quantisation + dism bitmask round trip (VQ = the game's one encoder) ----------
do
    local VQ = api.VQ
    local s = { v = {}, f = 4 + 16, dism = { "head", "LowerArm_L", "nosuchbone", "foot_r" } }
    for i = 1, 19 do s.v[i] = 50 end
    s.v[1] = 62.51            -- 4000.64 -> 4001
    s.v[2] = 151              -- above the health cap -> 9600
    s.v[3] = 0 / 0            -- unreadable -> 65535
    s.v[4] = -3               -- negative -> 0
    s.v[14] = 250             -- stamina cap 200 -> 12800
    s.v[16] = 1e9             -- bleeding: 65534 max
    s.v[17] = nil             -- unreadable -> 65535
    local r = VQ.record(7, s)
    T.check(r.v[1] == 4001 and r.v[2] == 9600 and r.v[3] == 65535 and r.v[4] == 0 and r.v[14] == 12800
        and r.v[16] == 65534 and r.v[17] == 65535 and r.v[5] == 3200 and r.flags == 20,
        "quantise: round(x * 64), per-field caps, 65535 unknown", T.repr(r.v))
    T.check(r.dism == (1 << DP.HEAD) | (1 << DP.LOWERARM_L) | (1 << DP.FOOT_R), "dism names -> bitmask (any case; unknown dropped)", r.dism)
    local m, e = RL.marshal(S, "vitals", r)
    T.check(m ~= nil, "the record marshals as the schema's Vitals", e)
    local f = VQ.frame(m or r)
    T.check(f.seq == 7 and f.v[1] == 4001 / 64 and f.v[3] == nil and f.v[17] == nil and f.v[14] == 200 and f.hp == 4001 / 64
        and f.f == 20 and f.dead == false, "dequantise: q / 64, 65535 -> nil", T.repr(f.v))
    T.check(T.eq(f.dism, { "head", "lowerarm_l", "foot_r" }), "bitmask -> lower-case bone names (bit order)", T.repr(f.dism))
    T.check(VQ.frame({ seq = 1, flags = 0, v = { 0 } }).dead == true, "dead rule: Health 0 without the DEAD flag is dead")
    T.check(VQ.frame(nil) == nil, "no record -> nil")
end

-- ---- HUD: my own vitals are readable by any Lua state ------------------------------
do
    local HS = dofile(T.path("mods/HSMPHud/Scripts/hud_state.lua"))
    local snap = HS.read_all(STATE)
    T.check(snap.vown ~= nil and snap.vown.hp == 100 and snap.vown.dead == false and snap.vown.con == 100,
        "HUD own vitals come from IPC.rec(\"vitals\") (were always empty)", T.repr(snap.vown))
end

-- ---- remote record + stand-in mirror ---------------------------------------------
local SI = mk_willie("Willie_BP_C_7", full_vitals({ Health = 1000 }))
api.set_puppets({ ["Willie_BP_C_7"] = 2 }, { [2] = SI }, nil)
local vals = { 62.5, 0.0, 80, 70, 70, 100, 30.25, 100, 15, 100, 100, 40, 40, 23.5, 12, 1.75, 1.5, 33, 0.5 }
local function peer_rec(seq, flags, dism, hp0)
    local v = {}
    for i, x in ipairs(vals) do v[i] = (i - 1 == 5) and 65535 or q(x) end
    if hp0 then v[1] = 0 end
    return { seq = seq, flags = flags, dism = dism, v = v }
end
N.sc_put("peer_vitals", peer_rec(10, 2, (1 << DP.LOWERARM_L) | (1 << DP.THIGH_R)), 2)
IPCF.peer_dir(true)   -- the HUD read above cached the directory before peer 2 had a slot
local r = api.read_vitals(2)
T.check(r ~= nil and r.seq == 10 and r.hp == 62.5 and r.f == 2 and r.dead == false, "peer_vitals: seq/hp/flags", T.repr(r))
T.check(r and r.v[1] == 62.5 and r.v[6] == nil and r.v[19] == 0.5, "peer_vitals: v dequantised, 65535 -> unknown")
T.check(r and T.eq(r.dism, { "lowerarm_l", "thigh_r" }), "peer_vitals: dism names", T.repr(r and r.dism))

api.set_tick(41)
api.update_standins()
T.check(SI["Health"] >= 100, "mirror: Health stays pinned (>= 100, the game regen cap)", SI["Health"])
T.check(SI["Head Health"] == 1.0, "mirror: Head Health 0 floored to 1 (never lethal)", SI["Head Health"])
T.check(SI["Arm_R Health"] == 30.25 and SI["Leg_R Health"] == 15, "mirror: limb values copied")
T.check(SI["Stamina"] == 23.5 and SI["Exhaustion"] == 12
    and SI["Bleeding"] == 1.75 and SI["Pain"] == 33, "mirror: stamina/exhaustion/bleeding/pain")
T.check(SI["Back Health"] == 100, "mirror: unknown (65535) field untouched")
T.check(SI["Fallen"] == false and SI["Fallen Rate"] == 0, "mirror: posture flags not written")
T.check(SI["Mesh"].hidden["lowerarm_l"] == 0, "mirror: severed bone hidden (PBO_None)")
T.check(SI["Mesh"].hidden["thigh_r"] == nil, "mirror: a part the stand-in mesh lacks is not hidden")
SI["Stamina"] = 90   -- native regen drifts the stand-in
api.set_tick(45)
api.update_standins()
T.check(SI["Stamina"] == 90, "mirror: same seq not re-applied before reassert")
api.set_tick(52)
api.update_standins()
T.check(SI["Stamina"] == 23.5, "mirror: re-asserted after MIRROR_REASSERT ticks")
N.sc_put("peer_vitals", peer_rec(11, 2, 0), 2)
api.set_tick(53)
api.update_standins()
T.check(SI["Mesh"].hidden["lowerarm_l"] == nil, "mirror: bone un-hidden when the owner's list clears")

-- ---- attacker claim (after-call hook) -> owner replay (native only) -------------
-- UE4SS runs a Blueprint hook's callback AFTER the body: the claim is the
-- Deal Complex Damage inputs my weapon passed on the stand-in, nothing is
-- measured, and the stand-in itself took no damage (Invulnerable).
local function comp_owned_by(actor) return { GetOwner = function() return actor end } end
local zero = { X = 0, Y = 0, Z = 0 }
local loc = { X = 11, Y = 22, Z = 33 }
local si0 = { SI.Health, SI["Head Health"], SI.Stamina, SI.Pain }
api.on_complex(SI, SI.Mesh, comp_owned_by(ME), FName("head"), loc, { X = 0, Y = 0, Z = 1 },
    { X = 900, Y = 0, Z = 0 }, { X = 600, Y = 0, Z = 0 }, 0.5, 0.0, 1.2, 1, false, false, 1.0, nil, false, 0.0)
api.set_tick(54)
api.flush_claims()
T.check(SI.Health == si0[1] and SI["Head Health"] == si0[2] and SI.Stamina == si0[3] and SI.Pain == si0[4],
    "the stand-in is untouched by its own claim (nothing buffered, nothing measured)")
local dl = sent("damage")
T.check(#dl == 1, "one damage record sent", T.repr(dl))
local dt = dl[#dl] or {}
T.check(dt.flags == 32 and dt.n == 0 and type(dt.velocity) == "table" and dt.velocity[1] == 900
    and math.abs((dt.damage_out or 0) - 1.2) < 1e-6 and dt.dism_blunt == 2561 and dt.target_peer_id == 2 and dt.bone == "head",
    "claim = armour-stage inputs (FLAG_COMPLEX), no measured deltas", T.repr(dt))

-- Owner side: its own Deal Complex Damage decides (half or double what a
-- "standard" Willie would take: its armour / height).
local function dcd_fn(scale)
    return function(self, hitc, coll, bone, l, n, vel, imp, cut, stab, rig, ...)
        self.calls.dcd = (self.calls.dcd or 0) + 1
        local raw = rig * math.sqrt(vel.X ^ 2 + vel.Y ^ 2 + vel.Z ^ 2) * scale
        self.Health = self.Health - raw * 0.01
        if bone.s == "head" then self["Head Health"] = self["Head Health"] - raw * 0.02 end
        self.Stamina = self.Stamina - 4 * scale
        self.Pain = self.Pain + 10 * scale
    end
end
local VI = mk_willie("Willie_BP_C_V", full_vitals())
VI["Deal Complex Damage"] = dcd_fn(0.5)
PC.Pawn = VI
-- the server forwards the claim's bytes as `damage_in` (hit_id / round stamped)
local inbound = RL.deep(dt)
inbound.hit_id, inbound.round = 1, 1
inbound = assert(RL.marshal(S, "damage_in", inbound))
local res = api.apply_hit(inbound, 1)
T.check(VI.calls.dcd == 1, "replay ran native Deal Complex Damage once", res)
-- rigidity 1.2 travels as f32: 1e-5 instead of 1e-6
T.check(math.abs(VI.Health - 94.6) < 1e-5 and math.abs(VI["Head Health"] - 89.2) < 1e-5 and VI.Stamina == 98 and VI.Pain == 5,
    "SOLO PARITY: the owner's own native replay is the damage", T.repr({ VI.Health, VI["Head Health"], VI.Stamina, VI.Pain, res }))
local VI3 = mk_willie("Willie_BP_C_V3", full_vitals({ Health = 10 }))
VI3["Deal Complex Damage"] = dcd_fn(2.0)
PC.Pawn = VI3
res = api.apply_hit(inbound, 1)
T.check(VI3.Health <= 0 and T.contains(res, "LETHAL"), "native lethal replay not resurrected", T.repr({ VI3.Health, res }))

-- ---- a stand-in's weapon echo on my pawn: undone after the call, touch sent --------
PC.Pawn = ME
api.set_my_peer_id(1)
local WPN = mk_willie("ModularWeaponBP_C_3", {})   -- its weapon (no Parent Actor resolvable)
WPN["GetClass"] = function() return { GetFName = function() return FName("ModularWeaponBP_C") end } end
api.set_puppets({ ["Willie_BP_C_7"] = 2 }, { [2] = SI }, { ["ModularWeaponBP_C_3"] = 2 })
api.C3.baseline(ME)
local me0 = { ME.Health, ME.Stamina, ME.Pain }
local z0 = api.state().zeroed
-- the native echo already ran (that is when UE4SS calls back)
ME.Health, ME.Stamina, ME.Pain = ME.Health - 7, ME.Stamina - 3, ME.Pain + 4
HSMPNative.bus_put("playback", { rows = { { peer = 2, body_ts = 5000, arm_ts = 5100, local_ms = math.floor(CLOCK * 1000) } } })
api.set_tick(55)
api.on_get_damage(ME, zero, zero, loc, zero, FName("spine_02"), 50, 0.5, false, ME.Mesh, 0, false, false, comp_owned_by(WPN))
T.check(ME.Health == me0[1] and ME.Stamina == me0[2] and ME.Pain == me0[3] and api.state().zeroed == z0 + 1,
    "stand-in WEAPON echo on my pawn undone (via weapon map), stamina included", T.repr({ ME.Health, ME.Stamina, ME.Pain }))
local touch = sent("touch")
T.check(#touch == 1 and touch[1].other_peer_id == 2 and touch[1].other_ts == 5100,
    "the echo is reported as a touch (evidence against a parry)", T.repr(touch))

-- ---- owner death -> stand-in plays death -----------------------------------------
N.sc_put("peer_vitals", peer_rec(12, 5, 0, true), 2)
-- Death handshake: HSMPAvatars must see the death BEFORE Health hits 0.
local sdead_at_death
SI["Death"] = function(self)
    self.calls.death = (self.calls.death or 0) + 1; self.DED = true
    sdead_at_death = { rec = N.sc_get("standin_dead"), hp = self.Health }
end
api.set_tick(60)
api.update_standins()
T.check(SI["calls"]["death"] == 1, "owner dead -> stand-in Death() once")
local sdt = sdead_at_death and sdead_at_death.rec or {}
T.check(type(sdt.rows) == "table" and #sdt.rows == 1 and sdt.rows[1].peer == 2 and sdt.rows[1].name == "Willie_BP_C_7"
    and math.type(sdt.wall) == "integer" and sdt.wall > 0,
    "the standin_dead record lists the peer before Death() (Avatars keeps the corpse)", T.repr(sdead_at_death))
api.set_tick(61)
api.update_standins()
T.check(SI["calls"]["death"] == 1, "death not replayed twice")

-- Round reset (new world). The owner's vitals still say dead (seq 12,
-- unchanged) and the server declared no death this round: the new stand-in is
-- alive, pinned, not killed; the handshake record lists nobody.
api.drop_world("world#2")
match("live", 2)
api.refresh_match()
local SI2 = mk_willie("Willie_BP_C_8", full_vitals({ Health = 1000 }))
api.set_puppets({ ["Willie_BP_C_8"] = 2 }, { [2] = SI2 }, nil)
api.set_tick(70)
api.update_standins()
api.write_standin_dead(false)
T.check(SI2["calls"]["death"] == nil and SI2["Health"] >= 1000, "a stale vitals dead flag does not kill the new round's stand-in",
    T.repr({ SI2["calls"]["death"], SI2["Health"] }))
local sd = N.sc_get("standin_dead") or {}
T.check(sd.n == 0 and #(sd.rows or {}) == 0, "handshake cleared for the new round", T.repr(sd))
N.sc_put("peer_vitals", peer_rec(13, 5, 0, true), 2)
api.set_tick(71)
api.update_standins()
T.check(SI2["calls"]["death"] == 1, "a fresh dead flag (seq advanced in this world) plays the death")

local logs = table.concat(T.map(LOGS, tostring), "")
T.check(T.contains(logs, "vitals mirror: peer 2 stand-in Willie_BP_C_7 <- seq 10"), "mirror log line")
