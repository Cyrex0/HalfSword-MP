-- Offline tests for HSMPCombat's claim path:
--
--     hsmp-tools lua-test combat
--
-- UE4SS calls a Blueprint hook back AFTER the body ran; the tests call the
-- callbacks the same way.
--   * one claim per real contact: a tick's Deal Complex Damage calls my
--     weapon made on one stand-in are ONE claim per bone (the strongest
--     call's armour-stage inputs); later calls of the same contact become at
--     most one continuation per CONT_MS, flushed when it ends;
--   * claims carry "cid" and "lage" and no measured deltas; a Get Damage
--     callback is never a claim (that would make two claims per blow);
--   * the stand-in is never touched by claiming; leaked native damage on it
--     is put back from its baseline;
--   * a stand-in's blow on my pawn (echo) is undone after the call (damage,
--     reaction, life flags) and reported as a touch; the replicated hit then
--     applies the damage through my own Deal Complex Damage;
--   * server feedback (`damage_verdict` records) -> combat_quality;
--   * everything the game sends is a typed record (`damage`, `touch`, `clash`,
--     `death_report`), read back with HSMPNative.sc_rec_drain().

local T = T
local STATE = T.tmpdir("hsmp_combat_test_")
local SRC = T.path("mods/HSMPCombat/Scripts/main.lua")

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

local addr = 2000
local function valid() return true end
local function mk_mesh()
    local m = { hidden = {} }
    m.IsValid = valid
    m.GetBoneIndex = function() return -1 end
    m.GetSocketLocation = function() return { X = 10, Y = 20, Z = 30 } end
    return m
end
local function mk_willie(name, over)
    addr = addr + 1
    local w = {
        __name = name, __addr = addr, calls = {},
        Health = 100, ["Head Health"] = 100, ["Neck Health"] = 100, ["Body Upper Health"] = 100,
        ["Body Lower Health"] = 100, ["Back Health"] = 100, ["Arm_R Health"] = 100, ["Arm_L Health"] = 100,
        ["Leg_R Health"] = 100, ["Leg_L Health"] = 100, ["Head Health (Crush)"] = 100,
        Consciousness = 100, ["Consciousness 2 (Legs)"] = 100, Stamina = 100, Exhaustion = 0,
        Bleeding = 0, ["Blood Rate"] = 1.5, Pain = 0, ["Fallen Rate"] = 0,
        ["Sustained Damage"] = 0, ["Damage Taken"] = 0, ["Last Damage Taken"] = 0,
        ["All Body Tonus"] = 56, ["Upper Body Tonus"] = 0.5, ["Flinch Index"] = 6, ["Force Death"] = false,
        ["Pain Upper Body"] = 0, ["Pain Stumble Immediate"] = { X = 0, Y = 0, Z = 0 },
    }
    for k, v in pairs(over or {}) do w[k] = v end
    w.IsValid = valid
    w.GetAddress = function(self) return self.__addr end
    w.GetFName = function(self) return FName(self.__name) end
    w.GetClass = function() return { GetFName = function() return FName("Willie_BP_C") end } end
    w.Mesh = mk_mesh()
    return w
end
-- The game's Get Damage (enough of it): Health / part health, the DRS
-- bookkeeping fields, pain, and the reaction state.
local function get_damage(self, bone, raw)
    local drs = 2 * raw
    self.Health = self.Health - drs * 5 * 0.00025
    self["Body Upper Health"] = self["Body Upper Health"] - drs * 0.0025
    self["Last Damage Taken"] = drs
    self["Sustained Damage"] = drs
    self.Pain = self.Pain + drs * 0.002
    self["All Body Tonus"] = 0           -- goes limp
    self["Upper Body Tonus"] = 0
    self["Flinch Index"] = 2
    self["Pain Upper Body"] = 40
    self["Pain Stumble Immediate"] = { X = 5, Y = 0, Z = 0 }
end

ME = mk_willie("Willie_BP_C_ME")
PC = { Pawn = ME, IsValid = valid }
package.preload["UEHelpers"] = function()
    return { GetPlayerController = function() return PC end, GetWorld = function() return nil end }
end
package.path = T.path("mods/shared") .. "/?.lua;" .. package.path
local fn, err = load(T.read(SRC))
T.check(fn ~= nil, "HSMPCombat loads (Lua 5.4)", err)
if fn == nil then return end
fn()
local api = HSMP_COMBAT_TEST.api
api.set_world("world#1")
local function write(name, text) T.write(STATE .. "/" .. name, text) end
local function read(name) return T.read(STATE .. "/" .. name) or "" end
local function append(name, text) T.write(STATE .. "/" .. name, read(name) .. text) end
-- Typed session state: the sidecar's `link` record + the server's `session`
-- record in the native mock, and the header heartbeat that makes a sidecar live.
local NAT = rawget(_G, "HSMPNative")
local IPCF = rawget(_G, "HSMP_IPC")
local ES = IPCF.S.ENUMS
local function hb(age) NAT._st.hb_age = age; IPCF.refresh_info(true) end
hb(0.01)
local function sidecar(status, id) NAT.sc_put("link", { status = ES.sidecar_status[status:upper()], state = ES.link_state.UP, my_peer_id = id }) end
local function no_link() NAT._rec.slots.link = nil end
local PHASE = { lobby = 0, countdown = 2, live = 3, roundover = 4, match_over = 5, paused = 7 }
local SESS = { phase = 0, round = 0, winner_seat = 255, rows = {} }
local function publish() NAT.sc_put("session", SESS) end
local function match(state, round) SESS.phase, SESS.round = PHASE[state], round; publish() end
local function roster(rows) SESS.rows = rows; publish() end
match("live", 3)
api.refresh_match()
-- the session is up (outbox protocol: nothing is written before "connected")
sidecar("connected", 1)
api.refresh_session()   -- connected + a fresh header heartbeat: live

-- Reaction property names are real Willie_BP_C properties.
local props = T.read(T.path("docs/development/halfsword/willie_props.txt"))
local real = T.set(T.re_findall("(?m)^  (.+?) = ", props))
local missing = {}
for _, n in ipairs(api.REACT) do if not real[n] then missing[#missing + 1] = n end end
for _, n in ipairs(api.REACT_VEC) do if not real[n] then missing[#missing + 1] = n end end
T.check(#missing == 0, "every REACT name is a real Willie_BP_C property", T.repr(missing))

local SI = mk_willie("Willie_BP_C_7", { Health = 1000 })
api.set_puppets({ ["Willie_BP_C_7"] = 2 }, { [2] = SI }, nil)
local zero = { X = 0, Y = 0, Z = 0 }
local function comp_owned_by(actor) return { GetOwner = function() return actor end } end
-- UE4SS calls a Blueprint hook back AFTER the body: this is the callback of a
-- Deal Complex Damage my weapon made on the stand-in.
local function hit_si(bone, vel, cut)
    api.on_complex(SI, SI.Mesh, comp_owned_by(ME), FName(bone), { X = 1, Y = 2, Z = 3 }, { X = 0, Y = 1, Z = 0 },
        { X = vel, Y = 0, Z = 0 }, { X = vel * 0.6, Y = 0, Z = 0 }, cut or 50.0, 0.0, 0.85, 0, false, false, 1.0, nil, false, 0.0)
end
-- Typed G2S records: every send the game made, in order
-- ({kind, req_id, data}); `sent(kind)` = the data tables of one kind.
local N = HSMPNative
local SENT = {}
local function sent(kind)
    for _, m in ipairs(N.sc_rec_drain()) do SENT[#SENT + 1] = m end
    local out = {}
    for _, m in ipairs(SENT) do if kind == nil or m.kind == kind then out[#out + 1] = m.data end end
    return out
end
local function v3is(v, x, y, z) return type(v) == "table" and v[1] == x and v[2] == y and v[3] == z end
local function empty(t) return type(t) == "table" and next(t) == nil end
local function damage_lines() return sent("damage") end
local function dec(d) return type(d) == "table" and d or {} end
-- A `damage_in` record as the native module hands it over (marshalled by the
-- executable spec, lib/hsmp_native_records.lua).
local RL = dofile(T.path("tools/hsmp-tools/lua-tests/lib/hsmp_native_records.lua"))
local function hit_rec(t)
    local r, e = RL.marshal(rawget(_G, "HSMP_IPC").S, "damage_in", t)
    assert(r, "hit_rec: " .. tostring(e))
    return r
end

-- ---- one contact, two bodies + a hand in one tick: ONE claim per bone -----------
CLOCK = 10.000
local si0 = { SI.Health, SI.Pain, SI["All Body Tonus"], SI["Flinch Index"] }
hit_si("spine_03", 1200)
hit_si("head", 900)
hit_si("head", 600)       -- a weaker frame of the same contact on the same bone
hit_si("hand_r", 300)
CLOCK = 10.010
api.set_tick(100)
api.flush_claims()
local dl = damage_lines()
T.check(#dl == 3, "three bones in one tick -> one claim each", T.repr(dl))
local l1 = dl[1] or {}
local function line_of(b) for _, l in ipairs(dl) do if l.bone == b then return l end end end
T.check(line_of("spine_03") and line_of("head") and line_of("hand_r"), "every bone the blade touched is claimed", T.repr(dl))
T.check(v3is(dec(line_of("head")).velocity, 900, 0, 0), "the strongest call of the bone is claimed", T.repr(line_of("head")))
T.check(l1.cid == 1 and l1.lage_ms == 10 and l1.target_peer_id == 2 and l1.hit_id == 0 and l1.round == 0,
    "claim carries cid and lage (hit_id / round are the sidecar's)", T.repr(l1))
T.check(l1.flags == 32 and l1.n == 0 and empty(l1.rows), "armour-stage claim, no measured deltas", T.repr(l1))
T.check(SI.Health == si0[1] and SI.Pain == si0[2] and SI["All Body Tonus"] == si0[3] and SI["Flinch Index"] == si0[4],
    "the stand-in is not touched by claiming (nothing buffered or measured)")

-- ---- same contact continues: continuation, not one claim per frame ---------------
CLOCK = 10.050; hit_si("spine_03", 1300); api.set_tick(101); api.flush_claims()
CLOCK = 10.083; hit_si("spine_03", 1300); api.set_tick(102); api.flush_claims()
T.check(#damage_lines() == 3, "calls within the episode are accumulated", #damage_lines())
CLOCK = 10.170; hit_si("spine_03", 1400); api.set_tick(103); api.flush_claims()
T.check(#damage_lines() == 4, "one continuation after CONT_MS", #damage_lines())
CLOCK = 10.300; api.set_tick(104); api.flush_claims()
T.check(#damage_lines() == 4, "episodes close without an empty claim", #damage_lines())
T.check(next(api.state().episodes) == nil, "all bone episodes closed")
CLOCK = 11.000; hit_si("spine_03", 1200); api.set_tick(105); api.flush_claims()
T.check(#damage_lines() == 5, "a new contact after the gap -> new claim", #damage_lines())

-- ---- a Get Damage callback never makes a claim ------------------------------------
-- (claiming it too would make two claims per blow, one with -99900 deltas)
CLOCK = 12.000
api.on_get_damage(SI, zero, zero, { X = 1, Y = 2, Z = 3 }, zero, FName("spine_03"), 700, 0.5, false, SI.Mesh, 0,
    false, false, comp_owned_by(ME))
api.set_tick(106); api.flush_claims(); CLOCK = 13.0; api.set_tick(107); api.flush_claims()
T.check(#damage_lines() == 5, "Get Damage on a stand-in is not a claim (Deal Complex Damage is)", #damage_lines())
-- ...and if native damage leaked onto the stand-in, it is put back from its baseline
api.C3.baseline(SI)
SI.Health, SI["Head Health"] = 990, 70
api.on_get_damage(SI, zero, zero, { X = 1, Y = 2, Z = 3 }, zero, FName("head"), 700, 0.5, false, SI.Mesh, 0,
    false, false, comp_owned_by(ME))
T.check(SI.Health == 1000 and SI["Head Health"] == 100, "native damage that got past Invulnerable is put back on the stand-in")

-- ---- a stand-in's blow on my pawn: undone after the call, the server's hit is the damage --
api.set_my_peer_id(1)
api.C3.baseline(ME)
HSMPNative.bus_put("playback", { rows = { { peer = 2, body_ts = 7000, arm_ts = 7100, local_ms = math.floor(CLOCK * 1000) } } })
api.set_tick(120)
ME.Health, ME["Body Upper Health"], ME.Pain, ME["Last Damage Taken"] = 96.25, 92.5, 3, 3000   -- the native echo ran
ME["All Body Tonus"], ME["Flinch Index"] = 0, 2
api.on_get_damage(ME, zero, zero, { X = 1, Y = 2, Z = 3 }, zero, FName("spine_03"), 1500, 0.5, false, ME.Mesh, 0,
    false, false, comp_owned_by(SI))
T.check(ME.Health == 100 and ME["Body Upper Health"] == 100 and ME.Pain == 0 and ME["Last Damage Taken"] == 0,
    "echo: every damage field back (the server's hit carries the damage)", T.repr({ ME.Health, ME["Body Upper Health"], ME.Pain }))
T.check(ME["All Body Tonus"] == 56 and ME["Flinch Index"] == 6, "echo: the reaction is undone too (the replay plays it)")
local touches = sent("touch")
T.check(#touches == 1 and touches[1].other_peer_id == 2 and touches[1].other_ts == 7100,
    "echo reported as a touch with the attacker time shown", T.repr(touches))
-- my own non-stand-in hit (a wall, my own blade): kept, and the new baseline
local WALL = mk_willie("StaticMeshActor_9")
ME.Health = 97
api.on_get_damage(ME, zero, zero, { X = 1, Y = 2, Z = 3 }, zero, FName("spine_03"), 1500, 0.5, false, ME.Mesh, 0,
    false, false, comp_owned_by(WALL))
T.check(ME.Health == 97, "a hit from anything but a stand-in keeps its native damage")
ME.Health = 90   -- a stand-in echo after it: undone to 97, not to 100
api.on_get_damage(ME, zero, zero, { X = 1, Y = 2, Z = 3 }, zero, FName("spine_03"), 1500, 0.5, false, ME.Mesh, 0,
    false, false, comp_owned_by(SI))
T.check(ME.Health == 97, "an echo after a legitimate hit undoes only itself", ME.Health)
ME.Health = 100
api.C3.baseline(ME)
-- The replicated hit arrives: the victim's own Deal Complex Damage applies it.
ME["Deal Complex Damage"] = function(self, hc, cc, bone, loc, nrm, vel) self.Health = self.Health - vel.X * 0.0025 end
CLOCK = 13.2
local res = api.apply_hit(hit_rec({ hit_id = 9, round = 3, target_peer_id = 1, bone = "spine_03",
    impulse = { 900, 0, 0 }, velocity = { 1500, 0, 0 }, cutting_power = 0.5, damage_out = 0.85, dism_blunt = 2560,
    flags = 32 }), 2)
T.check(ME.Health == 96.25, "replay applies the accepted damage", T.repr({ ME.Health, res }))

-- ---- server feedback -> cues, logs, combat_quality ----------------------------------
local RS = rawget(_G, "HSMP_IPC").S
local VK, DR = RS.ENUMS.verdict_kind, RS.ENUMS.damage_reason
N.sc_rec_event("damage_verdict", { kind = VK.CONFIRM, cid = 1, code = DR.CONFIRM })
N.sc_rec_event("damage_verdict", { kind = VK.FINAL, cid = 1, ok = true, code = DR.OK })
N.sc_rec_event("damage_verdict", { kind = VK.FINAL, cid = 2, ok = false, code = DR.PARRIED, reason = "parried" })
N.sc_rec_event("damage_verdict", { kind = VK.FINAL, cid = 3, ok = false, code = DR.BODY_MISS,
    reason = "body_miss: contact 40 uu from rewound victim body" })
N.sc_rec_event("damage_verdict", { kind = VK.CLASH, peer = 2, code = DR.CLASH })
api.read_feedback()
local q = api.quality()
T.check(q.claims == 5 and q.accepted == 1 and q.confirmed == 1 and q.clashes == 1, "quality counters", T.repr(q))
T.check(q.rejected.parried == 1 and q.rejected.body_miss == 1, "rejects counted by reason code", T.repr(q.rejected))
local cues = read(".combat_cue.jsonl")
T.check(cues == nil or cues == "", "no .combat_cue.jsonl (written, never read) any more", cues)
local logs = table.concat(T.map(LOGS, tostring), "")
T.check(T.contains(logs, "REJECTED by server: body_miss: contact 40 uu"), "reject logged with its reason")
CLOCK = 20.0
api.emit_quality(true)
local ev = read("hsmp_events.jsonl")
T.check((T.contains(ev, '"ev":"combat_quality"') or T.contains(ev, '"ev":"x_combat_quality"')) and T.contains(ev, '"rejected_by_reason":{')
    and T.contains(ev, '"claims":5'), "combat_quality event written (hsmp_log)", ev)

-- ---- solo parity: armour-stage capture on the stand-in, replay on the victim -------
CLOCK = 30.0
local WCOMP = comp_owned_by(ME)
api.on_complex(SI, SI.Mesh, WCOMP, FName("spine_03"), { X = 1, Y = 2, Z = 3 }, { X = 0, Y = 1, Z = 0 },
    { X = 1500, Y = 0, Z = 0 }, { X = 900, Y = 0, Z = 0 }, 120.0, 0.25, 0.85, 2, true, false, 1.0, nil, false, 0.0)
CLOCK = 30.01; api.set_tick(200); api.flush_claims()
local all = damage_lines()
local lc = all[#all] or {}
local dc = dec(lc)
T.check(dc.flags == 32 and v3is(dc.velocity, 1500, 0, 0) and v3is(dc.impulse, 900, 0, 0)
    and dc.cutting_power == 120 and math.abs((dc.pain_rate or 0) - 0.25) < 1e-6 and math.abs((dc.damage_out or 0) - 0.85) < 1e-6
    and dc.dism_blunt == (2 + 10 * 256 + 65536), "claim carries the armour-stage inputs (FLAG_COMPLEX)", T.repr(lc))

local VX = mk_willie("Willie_BP_C_VX")
local got
VX["Deal Complex Damage"] = function(self, hc, cc, bone, loc, nrm, vel, imp, cut, stab, rig, dism, lower, parent, kick, hb, xhv, draw)
    got = { bone = bone.s, vel = vel.X, imp = imp.X, cut = cut, stab = stab, rig = rig, dism = dism, lower = lower, kick = kick }
    self.Health = self.Health - 2.5   -- its own armour decided
end
VX["Get Damage"] = function() error("Get Damage must not be called directly on the armour path") end
PC.Pawn = VX
-- the server forwards the claim's bytes as `damage_in` (the sidecars stamped hit_id / round)
local inl = RL.deep(lc)
inl.hit_id, inl.round = 5, 3
local r2 = api.apply_hit(hit_rec(inl), 7)
T.check(got ~= nil and got.vel == 1500 and got.imp == 900 and got.cut == 120 and got.stab == 0.25
    and math.abs(got.rig - 0.85) < 1e-6   -- f32 on the wire
    and got.dism == 2 and got.lower == true and got.kick == 1.0, "victim replays Deal Complex Damage with the decoded inputs", T.repr(got))
T.check(VX.Health == 97.5 and T.contains(r2, "Deal Complex Damage(ok)") and T.contains(r2, "solo-equivalent"),
    "only the victim's own armour stage decided the damage", T.repr({ VX.Health, r2 }))
PC.Pawn = ME

-- ---- typed records, session and death-state guards ------------------------------------

-- Typed records: every combat send is a record of a known kind with
-- its own req_id (no line ids); nothing before "connected"
do
    sent()
    local ok, n, ids = true, #SENT, {}
    for _, m in ipairs(SENT) do
        if not (m.kind == "damage" or m.kind == "touch" or m.kind == "clash") or ids[m.req_id] or m.data.id ~= nil then ok = false end
        ids[m.req_id] = true
    end
    T.check(n > 0 and ok, "every combat send is a typed record with a unique req_id", T.repr(T.map(SENT, function(m) return m.kind end)))
    sidecar("reconnecting", 1)
    api.refresh_session()
    local before = #sent()
    T.check(api.session().connected == false, "reconnecting -> not connected")
    sidecar("reconnecting", 1)
    api.refresh_session()
    T.check(api.session().connected == false and api.session().my_peer_id == 1, "an unchanged re-publish keeps the last values")
    sidecar("connected", 1)
    api.refresh_session()
    T.check(api.session().connected == true and #sent() == before, "connected again; nothing was sent meanwhile")
end

-- Death state does not survive a new peer id, a session end, or a new match
do
    local st = api.state()
    st.remote_dead[2] = true
    api.session().death_shown[2] = true
    api.session().server_dead[2] = api.round_key(1)
    sidecar("reconnecting", 1); api.refresh_session()
    sidecar("connected", 1); api.refresh_session()
    T.check(api.state().remote_dead[2] == true, "a link stall (same peer id) keeps the death state")
    sidecar("connected", 4); api.refresh_session()
    T.check(next(api.state().remote_dead) == nil and next(api.session().death_shown) == nil
        and next(api.session().server_dead) == nil, "a new peer id (reconnect / server restart) clears it")
    api.state().remote_dead[3] = true
    no_link()
    api.refresh_session()
    T.check(api.state().remote_dead[3] == true, "one failed open is not a session end")
    api.refresh_session()
    T.check(next(api.state().remote_dead) == nil and api.session().connected == false, "session end clears it")
    sidecar("connected", 4); api.refresh_session()
    -- round keys carry the match: match 2 round 1 is not match 1 round 1
    match("live", 1); api.refresh_match()
    local k1 = api.round_key(1)
    match("match_over", 2); api.refresh_match()
    match("lobby", 0); api.refresh_match()
    match("live", 1); api.refresh_match()
    T.check(api.round_key(1) ~= k1, "round 1 of match 2 has a new key", api.round_key(1) .. " vs " .. k1)
    local k2 = api.round_key(1)
    publish(); api.refresh_match()
    T.check(api.round_key(1) == k2, "the same snapshot re-published: no new match context")
    match("live", 3); api.refresh_match()
end

-- Hooks are retried at 1 Hz indefinitely (the arena may load > 10 min after mod load)
do
    local saved = RegisterHook
    local fail = true
    local registered = {}
    RegisterHook = function(fn) if fail then error("function not found: " .. fn) end registered[#registered + 1] = fn end
    local h0 = api.hooks()
    T.check(h0.death == false and h0.weapon == false, "no BP hooks before the arena (RegisterHook was a no-op mock)", T.repr(h0))
    for t = 31, 30 * 1300, 1 do api.set_tick(t); api.try_hook() end     -- ~21 minutes at 30 Hz
    local h1 = api.hooks()
    T.check(h1.death == false and h1.death_tries >= 1250 and h1.weapon_tries >= 1250,
        "still retrying after 20 minutes (the cap of 600 tries is gone)", T.repr(h1))
    fail = false
    api.set_tick(30 * 1301); api.try_hook()
    local h2 = api.hooks()
    T.check(h2.death == true and h2.weapon == true, "hooks register as soon as the class exists", T.repr(h2))
    local n = #registered
    for t = 30 * 1302, 30 * 1310 do api.set_tick(t); api.try_hook() end
    T.check(#registered == n, "no second registration once hooked", #registered)
    local deaths = 0
    for _, fn in ipairs(registered) do if fn:find(":Death$") or fn:find(":Dying$") then deaths = deaths + 1 end end
    T.check(deaths == 2, "Death and Dying hooked exactly once each", T.repr(registered))
    RegisterHook = saved
end

-- The shared liveness rule. A "connected" link whose sidecar stops
-- changing (a crashed / killed sidecar) is not a session: no claims, no vitals.
do
    sidecar("connected", 1)
    api.refresh_session()
    T.check(api.session().connected == true, "fresh connected heartbeat: connected")
    hb(6)
    api.refresh_session()
    T.check(api.session().connected == false, "a 'connected' link whose sidecar stopped beating 6 s ago is a dead sidecar (single-player)")
    hb(0.01)
    api.refresh_session()
    T.check(api.session().connected == true, "the heartbeat moves again: connected")
end

-- A claim the outbox refuses (session not up) inflates nothing.
do
    sidecar("reconnecting", 1)
    api.refresh_session()
    T.check(api.session().connected == false, "reconnecting: not connected")
    local cid0, pend0, n0 = api.state().cid, api.quality().pending, #damage_lines()
    CLOCK = CLOCK + 3; hit_si("spine_03", 1200); api.set_tick(9000); api.flush_claims()
    CLOCK = CLOCK + 1; api.set_tick(9001); api.flush_claims()
    T.check(#damage_lines() == n0 and api.state().cid == cid0 and api.quality().pending == pend0,
        "refused claim: no line, cid and pending unchanged",
        T.repr({ api.state().cid, cid0, api.quality().pending, pend0 }))
    T.check(SI.Health == 1000, "the stand-in is still restored after a refused claim")
end

-- ==== world settle, echo undo, single application, session isolation =====================
sidecar("connected", 1)
api.refresh_session()
T.check(api.session().connected == true, "fw3: session live again")
local function P(v) return { v = v, get = function(self) return self.v end, set = function(self, x) self.v = x end } end
local function claims_since(n) local l = damage_lines(); local out = {}; for i = n + 1, #l do out[#out + 1] = l[i] end; return out end

-- No stand-in in this world -> a Get Damage costs no PlayerController walk
do
    local UEH = require("UEHelpers")
    local real = UEH.GetPlayerController
    local n = 0
    UEH.GetPlayerController = function() n = n + 1; return PC end
    api.set_puppets({}, {}, {})
    for i = 1, 20 do
        CLOCK = 40 + i * 0.01
        api.on_get_damage(ME, zero, zero, zero, zero, FName("head"), 900, 0.5, false, ME.Mesh, 2,
            false, false, comp_owned_by(SI))
    end
    T.check(n == 0, "20 Get Damage calls without stand-ins: no PlayerController lookup", n)
    api.set_puppets({ ["Willie_BP_C_7"] = 2 }, { [2] = SI }, nil)
    CLOCK = 41
    api.on_get_damage(SI, zero, zero, zero, zero, FName("head"), 10, 0.5, false, SI.Mesh, 0,
        false, false, comp_owned_by(ME))
    T.check(n == 1, "with a stand-in: one lookup (WG.pc, once per frame)", n)
    CLOCK = 41.5; api.set_tick(9100); api.flush_claims()
    UEH.GetPlayerController = real
end

-- An echo can never kill or maim locally: Force Death /
-- DED / Headless and every damage field are put back after the call.
do
    CLOCK = 50
    ME["Force Death"], ME.DED, ME.Headless = false, false, false
    api.C3.baseline(ME)
    local hp0 = ME.Health
    get_damage(ME, FName("neck_01"), 1500)     -- the native echo ran (lethal)
    ME["Force Death"], ME.DED, ME.Headless = true, true, true
    api.on_get_damage(ME, zero, zero, { X = 1, Y = 2, Z = 3 }, zero, FName("neck_01"), 1500, 0.9, false, ME.Mesh, 3,
        false, false, comp_owned_by(SI))
    T.check(ME["Force Death"] == false and ME.DED == false and ME.Headless == false,
        "Force Death / DED / Headless put back (no local kill from an unconfirmed hit)",
        T.repr({ ME["Force Death"], ME.DED, ME.Headless }))
    T.check(ME.Health == hp0 and ME["All Body Tonus"] == 56, "health and reaction back to their pre-echo values", ME.Health)
end

-- A hit exists when my weapon's armour stage runs on the
-- stand-in, whatever the stand-in's own armour does with it.
local WCOMP2 = comp_owned_by(ME)
local function complex(bone, vel)
    api.on_complex(SI, SI.Mesh, WCOMP2, FName(bone), { X = 1, Y = 2, Z = 3 }, { X = 0, Y = 1, Z = 0 },
        { X = vel, Y = 0, Z = 0 }, { X = 900, Y = 0, Z = 0 }, 120.0, 0.25, 0.85, 2, true, false, 1.0, nil, false, 0.0)
end
do
    -- (a) full plate on the stand-in: the armour stage runs, no Get Damage follows
    CLOCK = 60; local n0 = #damage_lines()
    complex("spine_03", 1500)
    CLOCK = 60.01; api.set_tick(9200); api.flush_claims()
    local c = claims_since(n0)
    T.check(#c == 1 and dec(c[1]).flags == 32 and dec(c[1]).bone == "spine_03"
        and empty(dec(c[1]).rows) and (dec(c[1]).velocity or {})[1] == 1500,
        "a blow the stand-in's plate absorbed is still claimed (armour-stage inputs, no measured deltas)", T.repr(c))
    -- (b) the real UE4SS order: the nested Get Damage calls back BEFORE its
    -- Deal Complex Damage. Still ONE claim, with the armour-stage inputs.
    CLOCK = 62; n0 = #damage_lines()
    api.on_get_damage(SI, zero, zero, zero, zero, FName("spine_01"), 800, 0.5, false, SI.Mesh, 0, false, false, WCOMP2)
    api.CX.pre(SI, SI.Mesh, WCOMP2, FName("spine_02"), { X = 1, Y = 2, Z = 3 }, { X = 0, Y = 1, Z = 0 },
        { X = 1300, Y = 0, Z = 0 }, { X = 900, Y = 0, Z = 0 }, 120.0, 0.25, 0.85, 2, true, false, 1.0, nil, false, 0.0)
    CLOCK = 62.01; api.set_tick(9202); api.flush_claims()
    c = claims_since(n0)
    T.check(#c == 1 and dec(c[1]).flags == 32 and (dec(c[1]).velocity or {})[1] == 1300,
        "playtest 2026-10-02: Get Damage then Deal Complex Damage callbacks = ONE claim (was two)", T.repr(c))
    -- (c) nothing carries over to the next tick
    T.check(next(api.CX.pending) == nil, "pending inputs are cleared every tick")
end

-- Never a second application; an armour-stage raw never reaches Get Damage.
do
    local V = mk_willie("Willie_BP_C_V17")
    PC.Pawn = V
    local gd = 0
    V["Get Damage"] = function() gd = gd + 1; V.Health = V.Health - 50 end
    local cline = hit_rec({ hit_id = 21, round = 3, target_peer_id = 1, bone = "spine_03",
        impulse = { 900, 0, 0 }, velocity = { 1500, 0, 0 }, normal = { 0, 1, 0 }, raw_damage = 1480, cutting_power = 120,
        pain_rate = 0.25, damage_out = 0.85, dism_blunt = 2, flags = 32 })
    -- (a) the armour stage errors before doing anything: the claim is dropped
    V["Deal Complex Damage"] = function() error("bad out-param") end
    local r = api.apply_hit(cline, 7)
    T.check(gd == 0 and V.Health == 100 and T.contains(r, "hit dropped: armour-stage replay failed"),
        "a failed armour-stage replay is dropped, its raw (relative speed) never fed to Get Damage", T.repr({ gd, V.Health, r }))
    -- (b) it errors AFTER the BP applied the hit: kept, no second application
    V["Deal Complex Damage"] = function(self) self.Health = self.Health - 4; error("copy out-params") end
    r = api.apply_hit(cline, 7)
    T.check(gd == 0 and V.Health == 96 and T.contains(r, "errored after applying"),
        "applied-then-errored armour stage counts once", T.repr({ gd, V.Health, r }))
    -- (c) plain Get Damage: the 18-arg retry only when the first call changed nothing
    local calls = 0
    V["Get Damage"] = function(self) calls = calls + 1; self.Health = self.Health - 3; error("out-param") end
    local pline = RL.deep(cline)
    pline.flags = 0
    r = api.apply_hit(pline, 7)
    T.check(calls == 1 and V.Health == 93, "Get Damage that applied then errored is not called again", T.repr({ calls, V.Health, r }))
    calls = 0
    V["Get Damage"] = function(self, ...)
        calls = calls + 1
        if select("#", ...) == 19 then error("arity") end
        self.Health = self.Health - 3
    end
    r = api.apply_hit(pline, 7)
    T.check(calls == 2 and V.Health == 90, "a 19-arg call that did nothing falls back to 18 args once", T.repr({ calls, V.Health }))
    -- Kick Power 0 is replayed as 0
    local kick
    V["Deal Complex Damage"] = function(self, hc, cc, bone, loc, nrm, vel, imp, cut, stab, rig, dism, lower, parent, k)
        kick = k
    end
    api.apply_hit(cline, 7)
    T.check(kick == 0, "kick power 0 stays 0 (was replayed as 1)", tostring(kick))
    PC.Pawn = ME
end

-- A "world changed" guard drop inside one world keeps the per-pawn
-- bookkeeping (no second round-start heal); a real level change resets it.
do
    api.own_pawn_tick(ME)
    local o = api.own()
    o.hits = 3
    api.wg_drop("world changed")
    T.check(api.own().hits == 3, "a PC-lookup blip keeps own (no mid-round heal)")
    api.own_pawn_tick(ME)
    T.check(api.own().hits == 3, "the same pawn keeps its bookkeeping")
    api.wg_drop("level change requested")
    T.check(api.own().addr == nil and api.own().hits == 0, "a real level change resets it")
    api.set_world("world#1")
end

-- Lines queued in session 1 are never flushed into session 2.
do
    T.check(api.discard_outbox ~= nil, "discard_outbox() exported")
    -- Records waiting on a full ring live in the facade's retry queue.
    local I = rawget(_G, "HSMP_IPC")
    I.retry[#I.retry + 1] = { kind = "damage", t = { cid = 999, target_peer_id = 2, bone = "stale" } }
    I.retry[#I.retry + 1] = { kind = "touch", t = { other_peer_id = 2 } }
    I.retry[#I.retry + 1] = { kind = "world_claim", t = { lid = "HSMPWorld:1:1", id = 5, mode = 1 } }   -- another mod's kind
    local n0 = #sent()
    sidecar("connected", 9)
    api.refresh_session()
    local stale = T.filter(sent("damage"), function(d) return d.bone == "stale" end)
    T.check(I.pending() == 1 and I.retry[1].kind == "world_claim" and #stale == 0 and #sent() == n0,
        "a new peer id discards the old session's queued combat records (others kept)", T.repr(I.retry))
    I.retry = {}
end

-- No Willie walk, no stand-in measurement / armour-stage record and no
-- echo reaction in the first 2 s of a world (WG.settled, shared/hsmp_wg.lua).
do
    sidecar("connected", 9)
    api.refresh_session()
    api.set_puppets({ ["Willie_BP_C_7"] = 2 }, { [2] = SI }, nil)
    -- hooks in an unsettled world
    api.set_world("world#fresh", true)
    T.check(not api.settled(), "a fresh world is not settled")
    local n0 = #damage_lines()
    CLOCK = CLOCK + 0.5
    complex("spine_03", 1500)
    api.on_get_damage(SI, zero, zero, zero, zero, FName("spine_03"), 1200, 0.5, false, SI.Mesh, 0,
        false, false, comp_owned_by(ME))
    T.check(next(api.CX.pending) == nil, "unsettled: nothing recorded for a stand-in hit")
    local t0 = #sent("touch")
    api.on_get_damage(ME, zero, zero, zero, zero, FName("spine_03"), 1500, 0.5, false, ME.Mesh, 0,
        false, false, comp_owned_by(SI))
    local t1 = #sent("touch")
    T.check(t1 == t0, "unsettled: an echo is undone but not reported")
    CLOCK = CLOCK + 0.1; api.set_tick(9300); api.flush_claims()
    T.check(#damage_lines() == n0, "unsettled: no claim")

    -- the tick: a real (mock) world through wg_check; FindAllOf only after 2 s
    local finds = 0
    local real_find = FindAllOf
    FindAllOf = function(c) if c == "Willie_BP_C" then finds = finds + 1; return { SI } end return nil end
    local world = { IsValid = valid, GetFullName = function() return "World /Game/Maps/Arenas/A.A" end,
                    GetAddress = function() return 77 end }
    PC.GetWorld = function() return world end
    PC.GetFName = function() return FName("PlayerController_3") end
    HSMPNative.bus_put("puppets", { rows = { { peer = 2, name = "Willie_BP_C_7" } } })
    local t0 = CLOCK
    for i = 1, 30 do CLOCK = t0 + i * 0.033; api.on_tick() end   -- ~1 s
    T.check(finds == 0, "no FindAllOf(Willie_BP_C) in the first second of a world", finds)
    for i = 31, 70 do CLOCK = t0 + i * 0.033; api.on_tick() end   -- past 2 s
    T.check(finds >= 1 and api.settled(), "the stand-in walk resumes once the world settled", finds)
    FindAllOf = real_find
    PC.GetWorld, PC.GetFName = nil, nil
    api.set_world("world#1")
end

-- The bus key "puppets" (typed rows) keeps the last stand-in set until HSMPAvatars
-- writes a new one; an empty set clears it.
do
    HSMPNative.bus_put("puppets", { rows = { { peer = 2, name = "Willie_BP_C_7" } } })
    api.refresh_puppets()
    T.check(api.puppet_peers()["Willie_BP_C_7"] == 2, " setupstand-in registered")
    api.refresh_puppets()
    T.check(api.puppet_peers()["Willie_BP_C_7"] == 2, "an unchanged key keeps the last set")
    HSMPNative.bus_put("puppets", { rows = {} })
    api.refresh_puppets()
    T.check(api.puppet_peers()["Willie_BP_C_7"] == nil, "an empty set clears it")
end

-- What C3.protect gated on a stand-in is lifted once it is mine.
do
    local SW = { IsValid = valid, GetFName = function() return FName("ModularWeaponBP_C_44") end, ["Temp Disable Damage"] = false }
    local S2 = mk_willie("Willie_BP_C_44", { Health = 1000 })
    S2["Weapon R"] = SW
    api.C3.protect(S2)
    T.check(S2.Invulnerable == true and SW["Temp Disable Damage"] == true, "protect: stand-in Invulnerable, its weapon gated")
    ME["Weapon R"] = SW                      -- I picked its weapon up
    ME.Invulnerable = true                   -- (the game's own spawn protection: left alone)
    api.C3.ungate(ME)
    T.check(SW["Temp Disable Damage"] == false and ME.Invulnerable == true,
        "a gated stand-in weapon in my hands deals damage again; my own Invulnerable untouched")
    SW["Temp Disable Damage"] = true         -- the game's own 0.2 s gate later
    api.C3.ungate(ME)
    T.check(SW["Temp Disable Damage"] == true, "lifted once only (the game's own gate is left alone)")
    ME["Weapon R"], ME.Invulnerable = nil, nil
end

-- Seen in game: a stand-in on my Team Int makes every blow of my
-- weapon "Friendly Fire?" (cutting 0, velocity x0.1). Its own team, unless
-- the server roster makes us teammates.
do
    ME["Team Int"] = 1
    local S3 = mk_willie("Willie_BP_C_45", { Health = 1000, ["Team Int"] = 1 })
    roster({ { seat = 1, peer_id = 2, team = 0, connected = true }, { seat = 2, peer_id = 9, team = 0, connected = true } })
    api.C3.read_teams()
    api.C3.protect(S3, 2)
    T.check(S3["Team Int"] ~= 1 and S3["Team Int"] ~= 0, "a duel opponent's stand-in is never on my team (no friendly fire)", S3["Team Int"])
    roster({ { seat = 1, peer_id = 2, team = 3, connected = true }, { seat = 2, peer_id = 9, team = 3, connected = true } })
    api.C3.read_teams()
    api.C3.protect(S3, 2)
    T.check(S3["Team Int"] == 1, "an MP teammate's stand-in shares my team (friendly fire as in solo)", S3["Team Int"])
    roster({ { seat = 1, peer_id = 2, team = 3, connected = true }, { seat = 2, peer_id = 9, team = 4, connected = true } })
    api.C3.read_teams()
    api.C3.protect(S3, 2)
    T.check(S3["Team Int"] ~= 1, "an MP opponent in another team is not on mine", S3["Team Int"])
end

-- Seen in game: the game's regen moves a stand-in's values UP between
-- ticks; the backstop must only undo damage (300 false "put back" lines).
do
    local S4 = mk_willie("Willie_BP_C_46", { Health = 90, Consciousness = 60 })
    api.set_puppets({ ["Willie_BP_C_46"] = 2 }, { [2] = S4 }, nil)
    api.C3.baseline(S4)
    local n0 = api.C3.standin_restores
    S4.Health, S4.Consciousness = 91, 62          -- regen
    api.on_get_damage(S4, zero, zero, zero, zero, FName("head"), 10, 0, false, S4.Mesh, 0, false, false, comp_owned_by(ME))
    T.check(S4.Health == 91 and S4.Consciousness == 62 and api.C3.standin_restores == n0, "regen on a stand-in is not 'put back'")
    S4["Head Health"] = 80                        -- real damage got through
    api.on_get_damage(S4, zero, zero, zero, zero, FName("head"), 10, 0, false, S4.Mesh, 0, false, false, comp_owned_by(ME))
    T.check(S4["Head Health"] == 100 and api.C3.standin_restores == n0 + 1, "damage on a stand-in still is")
end

-- ---- Raw-first claim selection ----------------------------------------------------
-- Seen in game: a blade stuck on the stand-in (rubber-banded: Hit Velocity
-- ×0.1, normal impulse huge) must not win over the clean frame that cut.
do
    local cleanSI = mk_willie("Willie_BP_C_8", { Health = 1000 })
    api.set_puppets({ ["Willie_BP_C_7"] = 2, ["Willie_BP_C_8"] = 3 }, { [2] = SI, [3] = cleanSI }, nil)
    local function call(vel, imp)
        api.on_complex(cleanSI, cleanSI.Mesh, comp_owned_by(ME), FName("thigh_l"), { X = 1, Y = 2, Z = 3 }, { X = 0, Y = 1, Z = 0 },
            { X = vel, Y = 0, Z = 0 }, { X = imp, Y = 0, Z = 0 }, 40.0, 0.0, 0.85, 0, false, false, 1.0, nil, false, 0.0)
    end
    CLOCK = 500.0; api.set_tick(99000)
    local n0 = #damage_lines()
    call(1900, 700)     -- the clean cut
    call(380, 3800)     -- the same contact a frame later, stuck: ×0.1
    api.flush_claims()
    local dl2 = damage_lines()
    local nl = dl2[n0 + 1] or {}
    T.check(#dl2 == n0 + 1 and v3is(dec(nl).velocity, 1900, 0, 0),
        "the clean frame is claimed, not the rubber-banded one (Raw first)", T.repr(nl))
    api.set_puppets({ ["Willie_BP_C_7"] = 2 }, { [2] = SI }, nil)
end

-- ---- death_report (typed G2S record, replaces the control outbox {"kind":"died"}) --------
do
    sidecar("connected", 9)
    api.refresh_session()
    match("live", 3); api.refresh_match()
    local n0 = #sent("death_report")
    api.on_native_death(ME)
    local dr = sent("death_report")
    T.check(#dr == n0 + 1 and dr[#dr].round == 3 and dr[#dr].death_id == 0,
        "my pawn's native death -> one death_report record for round 3 (death_id is the sidecar's)", T.repr(dr))
    api.on_native_death(ME)
    T.check(#sent("death_report") == n0 + 1, "reported once per round", #sent("death_report"))
end

-- ---- a claim the record layer refuses (non-finite input) is counted, not sent -----------
do
    CLOCK = 600.0; api.set_tick(99100)
    local n0, r0 = #damage_lines(), api.SEND.refused.damage or 0
    api.on_complex(SI, SI.Mesh, comp_owned_by(ME), FName("thigh_r"), { X = 1, Y = 2, Z = 3 }, { X = 0, Y = 1, Z = 0 },
        { X = 0 / 0, Y = 0, Z = 0 }, { X = 900, Y = 0, Z = 0 }, 40.0, 0.0, 0.85, 0, false, false, 1.0, nil, false, 0.0)
    api.flush_claims()
    T.check(#damage_lines() == n0 and (api.SEND.refused.damage or 0) == r0 + 1,
        "a NaN velocity is refused by the record layer (bad:velocity) and counted", T.repr(api.SEND.refused))
end
