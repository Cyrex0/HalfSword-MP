-- Damage parity: the same blow does the same damage in MP as in solo.
--
--     hsmp-tools lua-test damage_parity
--
-- The native path is a model of the decompiled Willie_BP
-- (docs/development/subsystems/combat.md): "Deal Complex Damage" (contact gate,
-- armour layer: Raw from Rigidity, Hit Velocity, Cutting Power, Stab Rate)
-- then "Get Damage" (Invulnerable gate, DRS = 2·hm·Raw, the per-bone contact
-- gate, the impulse / DRS gate, Health with the per-part floor, bleeding,
-- consciousness, pain, part health, blood). UE4SS calls a Blueprint hook's
-- callback AFTER the function body (ue4ss Docs registerhook.md): the model
-- does exactly that, so this suite would have caught the old PRE/POST design
-- (it measured -99900 per field and made two claims per blow).
--
-- SOLO: the attacker's weapon / fist hits the victim; the native path runs.
-- MP:   it hits the victim's STAND-IN on the attacker's machine (its own
--       armour, height and Invulnerable), HSMPCombat claims the armour-stage
--       inputs, the server forwards them (honest blow: unclamped), and the
--       victim's HSMPCombat replays them natively on its own pawn.
-- Proven (REAL HSMPCombat main.lua), for blunt, cut, stab and fist blows on
-- unarmoured, padded, mail and plate victims:
--   1. the victim's Health, Consciousness, Bleeding, Pain, part Health and
--      blood marks change exactly as in solo (the numeric table is printed);
--   2. one blow = one claim, and the stand-in takes nothing;
--   3. the stand-in's own echo of the blow on the victim's screen (its body /
--      weapon reaching the victim first) is undone and does not gate the
--      replay: echo + replay == solo;
--   4. a rejected blow (echo only) leaves the victim exactly as it was;
--   5. third screens (and the attacker) see the blow's blood on the stand-in
--      (S2CHitFx replay) while its vitals stay the owner's.

local T = T
local STATE = T.tmpdir("hsmp_parity_")
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

-- ---- the native model (decompiled Willie_BP) ------------------------------------------
local function len(v) return math.sqrt(v.X * v.X + v.Y * v.Y + v.Z * v.Z) end
local function scale(v, k) return { X = v.X * k, Y = v.Y * k, Z = v.Z * k } end
local function mrc(x, a0, a1, b0, b1)   -- MapRangeClamped
    local t = (a1 == a0) and 0 or (x - a0) / (a1 - a0)
    t = math.max(0, math.min(1, t))
    return b0 + (b1 - b0) * t
end
-- body part: 0 Head 1 Neck 2 ArmR 3 ArmL 4 Upper 5 Lower 6 LegR 7 LegL
local function part_of(bone)
    local b = bone:lower()
    if b:find("^head") then return 0 end
    if b:find("neck") then return 1 end
    if b:find("arm") or b:find("^hand") then return b:find("_r$") and 2 or 3 end
    if b:find("^thigh") or b:find("^calf") or b:find("^foot") then return b:find("_r$") and 6 or 7 end
    if b:find("^pelvis") or b:find("^spine_01") or b:find("^spine_02") then return 5 end
    return 4
end
local KH  = { [0] = 15, 7.5, 0.1, 0.1, 5, 2.5, 0.1, 0.1 }
local THR = { [0] = 1, 2.5, 15, 15, 5, 10, 15, 15 }
local KB  = { [0] = 5, 20, 0.1, 0.1, 5, 3, 0.1, 0.1 }
local KC  = { [0] = 10, 1, 0, 0, 1, 0.5, 0, 0 }
local KL  = { [0] = 3, 1, 1, 1, 1, 0.5, 1, 1 }
local BL  = { [0] = 1, 0.1, 0.1, 0.1, 0.25, 0.1, 0.1, 0.1 }
local PH  = { [0] = "Head Health", "Neck Health", "Arm_R Health", "Arm_L Health", "Body Upper Health",
              "Body Lower Health", "Leg_R Health", "Leg_L Health" }
-- armour layers (Protection Blunt / Cut / Stab, armor_protection.txt)
local PROT = { none = { 0, 0, 0 }, padded = { 5, 20, 2 }, mail = { 1, 150, 15 }, plate = { 25, 300, 150 } }

local api       -- HSMPCombat test api (set below)
local function cb(name, ...) if api then api[name](...) end end   -- the UE4SS after-callback

local function get_damage(self, Impulse, Velocity, Location, Normal, bone, raw, cp, inside, mesh, dism,
                          lower, shock, hitby, stab, hitbox, painrate, hitflesh, draw)
    (function()
        self["Was Just Touched"] = true
        if self.Invulnerable then return end
        local b = bone:ToString()
        local drs = raw * 2 * self.hm
        local lb = self["Last Damaged Bone"] and self["Last Damaged Bone"]:ToString() or "None"
        if not (drs >= self["Last Damage Taken"] * (draw + 1) or b ~= lb) then return end
        self["Last Damage Taken"], self["Last Damaged Bone"] = drs, FName(b)
        self["Sustained Damage"] = self["Sustained Damage"] + (inside and 0 or drs)
        if not (len(Impulse) > (lower and 0 or 1000) or inside or drs > 333 or len(Velocity) * (draw + 1) > 1000) then return end
        local p = part_of(b)
        local hpd = drs > 1000 or inside
        local hp = hpd and drs * KH[p] * 0.00025 or 0
        local h0 = self.Health
        local nh = h0 - hp
        self.Health = math.max(0, (nh >= THR[p]) and nh or ((h0 <= THR[p]) and h0 or THR[p]))
        self.Bleeding = self.Bleeding + drs * KB[p] * 0.0001 * math.max(cp, 1)
        local F = (4 * drs + len(Impulse)) / 5
        self.Consciousness = self.Consciousness - F * KC[p] * 0.005
        local add = drs * 0.001 * mrc(cp, 0, 1000, 1, 10) * painrate
        if add > self["Current Pain Threshold"] then
            self["Current Pain Threshold"], self["Flinch Index"] = add, ({ [0] = 4, 6, 3, 2, 6, 5, 1, 0 })[p]
        end
        self.Pain = self.Pain + add
        local L = hpd and drs * KL[p] * 0.0025 or 0
        if p == 1 then L = L * mrc(cp, 0, 1000, 1, 25) end
        self[PH[p]] = math.max(0, math.min(100, self[PH[p]] - L))
        if drs >= 10000 then self["All Body Tonus"] = math.max(0, self["All Body Tonus"] - F * 0.005) end
        local blood = drs * mrc(cp, 0, 100, BL[p], 25) * 0.001 * math.max(0.25, math.min(2, self["Blood Rate"]))
            * mrc(cp, 0, 10, hitflesh and 0.1 or 0, 1)
        if blood >= 1 then self.gore[#self.gore + 1] = string.format("%s:%.0f", b, blood) end
    end)()
    cb("on_get_damage", self, Impulse, Velocity, Location, Normal, bone, raw, cp, inside, mesh, dism,
        lower, shock, hitby, stab, hitbox, painrate, hitflesh, draw)
end

local function deal_complex(self, hitc, coll, bone, loc, nrm, vel, imp, cp, stab, rig, blunt, lower,
                            dparent, kick, hitbox, xhv, draw)
    (function()
        local b = bone:ToString()
        local lb = self["Last Complex Damage Bone"] and self["Last Complex Damage Bone"]:ToString() or "None"
        local g = len(imp) * (cp + 1)
        if not (g >= self["Last Complex Damage Impulse"] or b ~= lb) then return end
        self["Last Complex Damage Impulse"], self["Last Complex Damage Bone"] = g, FName(b)
        local a = PROT[self.armour[part_of(b)] or "none"]
        local vh = len(vel)
        local e = cp * vh * rig
        local resist = (a[2] * (1 - stab) + a[3] * stab) * 1000
        local frac = e > 0 and math.max(0, (e - resist) / e) or 0
        local dmg = math.max(0, rig * vh * (1 - frac) - a[1] * 100) + rig * vh * frac
        get_damage(self, scale(imp, rig * kick), scale(vel, rig * kick), loc, nrm, bone, dmg, frac * cp, false,
            hitc, blunt, lower, dparent, coll, false, hitbox, 1, true, draw)
    end)()
    cb("on_complex", self, hitc, coll, bone, loc, nrm, vel, imp, cp, stab, rig, blunt, lower,
        dparent, kick, hitbox, xhv, draw)
end

local addr = 5000
local function valid() return true end
local function mk_mesh()
    return { IsValid = valid, GetSocketLocation = function() return { X = 0, Y = 0, Z = 100 } end }
end
local function mk_willie(name, armour, hm)
    addr = addr + 1
    local w = {
        __name = name, __addr = addr, armour = armour or {}, hm = hm or 1.0, gore = {},
        Health = 100, ["Head Health"] = 100, ["Neck Health"] = 100, ["Body Upper Health"] = 100,
        ["Body Lower Health"] = 100, ["Back Health"] = 100, ["Arm_R Health"] = 100, ["Arm_L Health"] = 100,
        ["Leg_R Health"] = 100, ["Leg_L Health"] = 100, ["Head Health (Crush)"] = 100,
        Consciousness = 100, ["Consciousness 2 (Legs)"] = 100, Stamina = 100, Exhaustion = 0,
        Bleeding = 0, ["Blood Rate"] = 1.5, Pain = 0, ["Sustained Damage"] = 0, ["Damage Taken"] = 0,
        ["Last Damage Taken"] = 0, ["Last Complex Damage Impulse"] = 0, ["Force Death"] = false, DED = false,
        Headless = false, Invulnerable = false, ["All Body Tonus"] = 56, ["Flinch Index"] = 6,
        ["Current Pain Threshold"] = 0, ["Dismembered Array"] = {},
    }
    w.IsValid = valid
    w.GetAddress = function(self) return self.__addr end
    w.GetFName = function(self) return FName(self.__name) end
    w.GetClass = function() return { GetFName = function() return FName("Willie_BP_C") end } end
    w.Mesh = mk_mesh()
    w["Deal Complex Damage"] = function(self, hitc, coll, bone, loc, nrm, vel, imp, cp, stab, rig, blunt, lower, dparent, kick, hb, xhv, draw)
        deal_complex(self, hitc, coll, bone, loc, nrm, vel, imp, cp, stab, rig, blunt, lower, dparent, kick, hb, xhv, draw)
    end
    w["Get Damage"] = function() error("an armour-stage claim must never be replayed through Get Damage") end
    return w
end
local function comp_of(actor) return { GetOwner = function() return actor end } end

-- One blow by `attacker`'s weapon (or fist) on `target`, as the game makes it.
local function strike(target, attacker, b)
    local v = { X = b.vel, Y = 0, Z = 0 }
    local imp = { X = b.imp or b.vel * 0.6, Y = 0, Z = 0 }
    deal_complex(target, target.Mesh, comp_of(attacker), FName(b.bone), { X = 0, Y = 0, Z = 100 },
        { X = -1, Y = 0, Z = 0 }, v, imp, b.cut, b.stab, b.rig, b.dism or 0, false, false, b.kick or 1, nil, false, b.draw or 0)
end

-- ---- boot HSMPCombat --------------------------------------------------------------
local ATT = mk_willie("Willie_BP_C_ATT")
PC = { Pawn = ATT, IsValid = valid }
package.preload["UEHelpers"] = function()
    return { GetPlayerController = function() return PC end, GetWorld = function() return nil end }
end
package.path = T.path("mods/shared") .. "/?.lua;" .. package.path
local fn, err = load(T.read(SRC))
T.check(fn ~= nil, "HSMPCombat loads", err)
if not fn then return end
fn()
api = HSMP_COMBAT_TEST.api
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
local function read(name) return T.read(STATE .. "/" .. name) or "" end
match("live", 3)
api.refresh_match()
sidecar("connected", 1); api.refresh_session()
sidecar("connected", 1); api.refresh_session()
T.check(api.session().connected, "session live")
api.set_my_peer_id(1)

local WATCH = { "Health", "Consciousness", "Bleeding", "Pain", "Head Health", "Neck Health", "Body Upper Health",
                "Body Lower Health", "Arm_R Health", "Arm_L Health", "Leg_R Health", "Leg_L Health",
                "Force Death", "DED", "Headless", "Flinch Index" }
local function state_of(w)
    local s = {}
    for _, k in ipairs(WATCH) do s[k] = w[k] end
    s.gore = table.concat(w.gore, ",")
    return s
end
local function delta(a, b)
    local d = {}
    for k, v in pairs(b) do
        if type(v) == "number" then
            local x = v - a[k]
            if math.abs(x) > 1e-9 then d[k] = x end
        elseif v ~= a[k] then d[k] = tostring(a[k]) .. "->" .. tostring(v) end
    end
    return d
end
local function same(a, b)
    for k, v in pairs(a) do
        local w = b[k]
        if type(v) == "number" then
            -- relative 1e-6: the `damage` record carries f32 inputs, solo runs
            -- the Lua doubles
            if type(w) ~= "number" or math.abs(v - w) > 1e-6 * math.max(1, math.abs(v)) then return false, k end
        elseif v ~= w then return false, k end
    end
    for k in pairs(b) do if a[k] == nil then return false, k end end
    return true
end

local hit_id = 0
-- The server: an accepted honest blow, forwarded as is (the claim's bytes; the
-- attacker's sidecar stamped hit_id and round). The attacker is the entry's peer.
local RL = dofile(T.path("tools/hsmp-tools/lua-tests/lib/hsmp_native_records.lua"))
local function serve(claim, target)
    hit_id = hit_id + 1
    local t = RL.deep(claim)
    t.hit_id, t.round, t.target_peer_id = hit_id, 3, target or 2
    local r, e = RL.marshal(rawget(_G, "HSMP_IPC").S, "damage_in", t)
    assert(r, "serve: " .. tostring(e))
    return r
end
-- Every typed `damage` record the game sent (HSMPNative.sc_rec_drain, accumulated).
local SENT = {}
local function drain() for _, m in ipairs(HSMPNative.sc_rec_drain()) do SENT[#SENT + 1] = m end end
local function claims()
    drain()
    local out = {}
    for _, m in ipairs(SENT) do if m.kind == "damage" then out[#out + 1] = m.data end end
    return out
end
local function tick(dt)
    CLOCK = CLOCK + dt
    api.set_tick(math.floor(CLOCK * 30))
    api.flush_claims()
end

-- One blow, solo vs MP. opts.echo: the stand-in's copy reached the victim
-- first on the victim's screen. opts.reject: the server rejects it.
local function run(b, opts)
    opts = opts or {}
    CLOCK = CLOCK + 10
    -- SOLO
    api.set_puppets({}, {}, {})
    local VS = mk_willie("Willie_BP_C_SOLO", b.victim_armour, b.victim_hm)
    local s0 = state_of(VS)
    strike(VS, ATT, b)
    local solo = delta(s0, state_of(VS))

    -- MP, attacker machine: the stand-in has its OWN armour and height
    -- (deliberately not the victim's) and is Invulnerable (C3.protect).
    local SI = mk_willie("Willie_BP_C_SI", { [0] = "plate", [4] = "plate" }, 1.1)
    SI.Health = 1000
    api.set_puppets({ ["Willie_BP_C_SI"] = 2 }, { [2] = SI }, nil)
    api.C3.protect(SI)
    PC.Pawn = ATT
    local si0 = state_of(SI)
    local n0 = #claims()
    strike(SI, ATT, b)
    tick(0.034); tick(1.0)   -- flush, then the episode closes
    local mine = {}
    for i = n0 + 1, #claims() do mine[#mine + 1] = claims()[i] end
    local si_touched = next(delta(si0, state_of(SI))) ~= nil

    -- MP, victim machine
    local VM = mk_willie("Willie_BP_C_MP", b.victim_armour, b.victim_hm)
    local SA = mk_willie("Willie_BP_C_SA")   -- the attacker's stand-in there
    PC.Pawn = VM
    api.set_puppets({ ["Willie_BP_C_SA"] = 1 }, { [1] = SA }, nil)
    api.C3.baseline(VM)                      -- (the per-tick baseline)
    local m0 = state_of(VM)
    if opts.echo then                        -- the stand-in's copy lands first
        HSMPNative.bus_put("playback", { rows = { { peer = 1, body_ts = 5000, arm_ts = 5100, local_ms = math.floor(CLOCK * 1000) } } })
        api.set_tick(math.floor(CLOCK * 30) + 7)
        strike(VM, SA, b)
    end
    local after_echo = delta(m0, state_of(VM))
    local res = {}
    if not opts.reject then
        for _, c in ipairs(mine) do res[#res + 1] = api.apply_hit(serve(c, 2), 1) end
    end
    local mp = delta(m0, state_of(VM))
    PC.Pawn = ATT
    return solo, mp, mine, si_touched, after_echo, res, SI
end

-- ---- the blows ----------------------------------------------------------------------
-- rig / velocities in the range the game produces (see subsystems/combat.md):
--   blunt: a mace head (rig 1.33 · quality 1.1 · blunt alignment 2.33 ≈ 3.4)
--   cut:   a sword edge (rig 0.85, cutting power 100)
--   stab:  a rondel point (rig 1.0, tip alignment 1, cutting power 90)
--   fist:  a punch (Willie body contact: Hit Velocity = Hit Impulse = the
--          normal impulse, rigidity 1 standing, cutting power 0)
local BLOWS = {
    { name = "blunt head",  bone = "head",     vel = 1400, cut = 0,   stab = 0, rig = 3.4, dism = 1 },
    { name = "blunt torso", bone = "spine_03", vel = 1300, cut = 0,   stab = 0, rig = 3.4 },
    { name = "cut neck",    bone = "neck_01",  vel = 1500, cut = 100, stab = 0, rig = 0.85, draw = 0 },
    { name = "cut arm",     bone = "lowerarm_r", vel = 1600, cut = 100, stab = 0, rig = 0.85 },
    { name = "stab torso",  bone = "spine_03", vel = 1200, cut = 90,  stab = 1, rig = 1.0 },
    { name = "stab belly",  bone = "spine_02", vel = 900,  cut = 90,  stab = 1, rig = 1.0 },
    { name = "fist head",   bone = "head",     vel = 900,  imp = 900, cut = 0, stab = 0, rig = 1.0, kick = 1 },
    { name = "fist torso",  bone = "spine_04", vel = 700,  imp = 700, cut = 0, stab = 0, rig = 1.0, kick = 1 },
}
local ARMOUR = {
    { name = "none",   a = {} },
    { name = "padded", a = { [4] = "padded", [5] = "padded", [2] = "padded", [3] = "padded" } },
    { name = "mail",   a = { [1] = "mail", [4] = "mail", [5] = "mail" } },
    { name = "plate",  a = { [0] = "plate", [4] = "plate" } },
}

local function f(d, k) local v = d[k]; return type(v) == "number" and string.format("%+.2f", v) or "0" end
local rows = { "| blow | victim armour | path | Health | Consciousness | Bleeding | Pain | part Health | blood |",
               "|---|---|---|---|---|---|---|---|---|" }
local all_equal, one_claim, standin_clean, n = true, true, true, 0
for _, ar in ipairs(ARMOUR) do
    for _, b0 in ipairs(BLOWS) do
        local b = {}
        for k, v in pairs(b0) do b[k] = v end
        b.victim_armour, b.victim_hm = ar.a, 0.95
        local solo, mp, mine, si_touched = run(b)
        n = n + 1
        local ok, k = same(solo, mp)
        if not ok then
            all_equal = false
            T.log(string.format("MISMATCH %s / %s at %s: solo %s mp %s", b.name, ar.name, tostring(k), T.repr(solo), T.repr(mp)))
        end
        if #mine ~= 1 then one_claim = false; T.log(b.name .. "/" .. ar.name .. ": claims " .. #mine) end
        if si_touched then standin_clean = false end
        local pk = PH[part_of(b.bone)]
        for _, row in ipairs({ { "solo", solo }, { "MP", mp } }) do
            local d = row[2]
            rows[#rows + 1] = string.format("| %s | %s | %s | %s | %s | %s | %s | %s %s | %s |", b.name, ar.name, row[1],
                f(d, "Health"), f(d, "Consciousness"), f(d, "Bleeding"), f(d, "Pain"), pk, f(d, pk),
                d.gore and (select(2, d.gore:gsub(":", ":")) .. " mark(s)") or "0")
        end
    end
end
T.log("\nDamage parity, solo vs MP (victim height factor 0.95; stand-in in plate, height 1.1):\n" .. table.concat(rows, "\n") .. "\n")
T.check(all_equal, string.format("1. every blow x armour (%d cases): the victim's change in MP == solo, field by field", n))
T.check(one_claim, "2. one blow = exactly one claim (the old PRE/POST design made two)")
T.check(standin_clean, "2. the Invulnerable stand-in takes nothing from the blow it claims")

-- At least one case per damage type really changed Health / bleeding / pain,
-- so "equal" is not "both zero".
do
    local b = { bone = "head", vel = 1400, cut = 0, stab = 0, rig = 3.4, victim_armour = {}, victim_hm = 1 }
    local solo = run(b)
    T.check((solo.Health or 0) < -1 and (solo.Consciousness or 0) < -10, "blunt head: real Health and consciousness loss", T.repr(solo))
    b = { bone = "neck_01", vel = 1500, cut = 100, stab = 0, rig = 0.85, victim_armour = {}, victim_hm = 1 }
    solo = run(b)
    T.check((solo.Bleeding or 0) > 1 and (solo["Neck Health"] or 0) < -10 and solo.gore, "cut neck: bleeding, neck health and blood", T.repr(solo))
    b = { bone = "spine_03", vel = 1200, cut = 90, stab = 1, rig = 1.0, victim_armour = { [4] = "mail" }, victim_hm = 1 }
    solo = run(b)
    T.check((solo.Bleeding or 0) > 0 and (solo.Pain or 0) > 0, "stab through mail: bleeding and pain", T.repr(solo))
    b = { bone = "head", vel = 900, imp = 900, cut = 0, stab = 0, rig = 1.0, kick = 1, victim_armour = {}, victim_hm = 1 }
    solo = run(b)
    T.check((solo.Consciousness or 0) < 0 and (solo.Health or 0) > -10, "fist head: consciousness, little Health", T.repr(solo))
end

-- 3. the stand-in's echo lands first and is undone; replay == solo
do
    local b = { bone = "spine_03", vel = 1300, cut = 0, stab = 0, rig = 3.4, victim_armour = { [4] = "padded" }, victim_hm = 1 }
    local solo, mp, _, _, after_echo = run(b, { echo = true })
    T.check(next(after_echo) == nil, "3. the echo on the victim is undone (nothing left of it)", T.repr(after_echo))
    T.check(same(solo, mp), "3. echo + replay == solo (the echo's contact gates do not swallow the replay)",
        T.repr({ solo = solo, mp = mp }))
    drain()
    local touch = T.filter(SENT, function(m) return m.kind == "touch" end)
    T.check(#touch >= 1 and touch[#touch].data.other_peer_id == 1, "3. the echo is reported as a touch (evidence against a parry)")
    local b2 = { bone = "head", vel = 900, imp = 900, cut = 0, stab = 0, rig = 1.0, kick = 1, victim_armour = {}, victim_hm = 1 }
    solo, mp = run(b2, { echo = true })
    T.check(same(solo, mp), "3. fist: echo + replay == solo", T.repr({ solo = solo, mp = mp }))
end

-- 4. rejected (parried) blow: the echo only -> nothing.
--    A body contact (fist, shoulder) echo is undone completely. A WEAPON echo
--    never reaches Deal Complex Damage in game: C3.protect sets the stand-in
--    weapons' "Temp Disable Damage" (Collision Hit's gate). Its numbers would
--    still be undone, but blood it painted could not be, hence the gate.
do
    local b = { bone = "head", vel = 900, imp = 900, cut = 0, stab = 0, rig = 1.0, kick = 1, victim_armour = {}, victim_hm = 1 }
    local _, mp = run(b, { echo = true, reject = true })
    T.check(next(mp) == nil, "4. a rejected blow (fist echo) leaves the victim exactly as it was", T.repr(mp))
    b = { bone = "neck_01", vel = 1500, cut = 100, stab = 0, rig = 0.85, victim_armour = {}, victim_hm = 1 }
    _, mp = run(b, { echo = true, reject = true })
    mp.gore = nil
    T.check(next(mp) == nil, "4. a leaked weapon echo's numbers are undone too (blood: see the gate)", T.repr(mp))
    local SA = mk_willie("Willie_BP_C_SA2")
    SA["Weapon R"] = { IsValid = valid, ["Temp Disable Damage"] = false }
    SA["Weapon L"] = { IsValid = valid, ["Temp Disable Damage"] = false }
    api.C3.protect(SA)
    T.check(SA["Weapon R"]["Temp Disable Damage"] == true and SA["Weapon L"]["Temp Disable Damage"] == true
        and SA.Invulnerable == true, "4. stand-in weapons get the game's damage gate, the stand-in Invulnerable")
end

-- 5. the blow's blood on the stand-in for the other screens (S2CHitFx)
do
    local b = { bone = "neck_01", vel = 1500, cut = 100, stab = 0, rig = 0.85, victim_armour = {}, victim_hm = 1 }
    local _, _, mine, _, _, _, SI = run(b)
    local line = serve(mine[1], 2)
    SI.gore = {}
    local s0 = state_of(SI)
    s0.gore = nil
    api.set_puppets({ ["Willie_BP_C_SI"] = 2 }, { [2] = SI }, nil)
    local r = api.C3.apply_fx(line, 1)
    local s1 = state_of(SI)
    s1.gore = nil
    T.check(T.contains(r, "replayed") and #SI.gore >= 1, "5. the accepted blow's blood appears on the stand-in", T.repr({ r, SI.gore }))
    T.check(same(s0, s1) and SI.Invulnerable == true and SI.Health >= 1000,
        "5. ...its vitals stay the owner's (restored) and it is Invulnerable again", T.repr(delta(s0, s1)))
    T.check(T.contains(api.C3.apply_fx(serve(mine[1], 1), 2), "own hit"), "5. a hit on me is never replayed as fx")
end
