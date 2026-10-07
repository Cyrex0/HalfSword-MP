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
package.path = T.path("mods/HSMPCombat/Scripts") .. "/?.lua;" .. T.path("mods/shared") .. "/?.lua;" .. package.path
local fn, err = load(T.read(SRC))
T.check(fn ~= nil, "HSMPCombat loads (Lua 5.4)", err)
if fn == nil then return end
fn()
local api = HSMP_COMBAT_TEST.api
api.set_world("world#1")

do
    local original_ai,original_pc=api.WG.ai_pawn,PC.Pawn
    local foreign=mk_willie("Willie_BP_C_native_fallback",{Health=0,DED=true})
    PC.Pawn=foreign
    api.WG.ai_pawn=function()return ME end
    T.check(api.local_pawn()==ME,"verified AI owner survives native fallback possession of another Willie")
    api.WG.ai_pawn=function()return nil end
    T.check(api.local_pawn()==foreign,"normal controller possession remains the local pawn without verified AI owner")
    api.WG.ai_pawn,PC.Pawn=original_ai,original_pc
end
do
    local previous_pawn=PC.Pawn
    local victim=mk_willie("Willie_BP_C_force_retry")
    local hp,force,refuse,calls=100,false,true,0
    rawset(victim,"Health",nil);rawset(victim,"Force Death",nil)
    setmetatable(victim,{
        __index=function(_,key) if key=="Health" then return hp elseif key=="Force Death" then return force end end,
        __newindex=function(obj,key,value)
            if key=="Health" or key=="Force Death" then
                if refuse then error("transient native write failure") end
                if key=="Health" then hp=value else force=value end
            else rawset(obj,key,value) end
        end,
    })
    victim.Death=function(self)
        calls=calls+1
        if refuse then error("transient native Death failure") end
        hp=0;self.DED=true
    end
    PC.Pawn=victim
    local ctx={match_id=9999,round=5,life=3}
    api.force_own_death(5,2,1,ctx)
    T.check(hp==100 and api.own().server_death_pending~=nil,
        "failed forced death remains pending instead of recording native success")
    refuse=false
    api.force_own_death(5,2,1,ctx)
    T.check(hp==0 and calls==2 and api.own().forced_life==3 and api.own().server_death_pending==nil,
        "same-context forced death retries after transient native failures")
    api.force_own_death(5,2,1,ctx)
    T.check(calls==2,"completed forced death deduplicates only after native dead readback")
    hp=100;victim.DED=false;ctx.life=4
    api.force_own_death(5,2,1,ctx)
    T.check(hp==0 and calls==3 and api.own().forced_life==4,
        "fresh life on same native pawn accepts a new server death within the same round")
    PC.Pawn=previous_pawn
end
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
-- Arithmetic/contact tests isolate life assignment; exact production binding
-- is exercised separately below against typed Session/Mode/SpawnStatus.
local real_life_for = api.C3.life_for
local real_displayed_for = api.C3.displayed_for
local mock_actors={}
local set_puppets=api.set_puppets
api.set_puppets=function(peers,actors,weapons) mock_actors=actors;set_puppets(peers,actors,weapons) end
api.C3.remote_actor=function(peer) return mock_actors[peer] end
local arithmetic_life_for = function(peer)
    if peer == 1 or peer == 2 or peer == 3 or peer == 7 or peer == 9 then
        return {match_id=4242,round=SESS.round,life=1}
    end
end
api.C3.life_for = arithmetic_life_for
local arithmetic_displayed_for=function(peer) return api.C3.life_for(peer,false) end
api.C3.displayed_for=arithmetic_displayed_for
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

local SI = mk_willie("Willie_BP_C_7", { Health = 1000, ["Last Complex Damage Impulse"] = 0 })
api.set_puppets({ ["Willie_BP_C_7"] = 2 }, { [2] = SI }, nil)
local zero = { X = 0, Y = 0, Z = 0 }
local function comp_owned_by(actor) return { IsValid = valid, GetOwner = function() return actor end } end
do
    local reads = 0
    local null_owner = { IsValid = function() return false end,
        GetClass = function() reads = reads + 1; error("native null dereference") end }
    T.check(not api.is_weapon(comp_owned_by(null_owner)), "ownerless contact is not a weapon")
    T.check(reads == 0, "null owner is never passed to GetClass (pcall cannot catch a native AV)")
    T.check(not api.is_weapon({ IsValid = function() return false end,
        GetOwner = function() reads = reads + 1; error("invalid component") end }), "invalid contact component skipped")
    T.check(reads == 0, "invalid component never dereferenced")
    local weapon = { IsValid = valid, GetClass = function() return { GetFName = function() return FName("Weapon_Sword_C") end } end }
    T.check(api.is_weapon(comp_owned_by(weapon)), "live weapon owner still classified")
    local body = { IsValid = valid, GetClass = function() return { GetFName = function() return FName("Willie_BP_C") end } end }
    T.check(not api.is_weapon(comp_owned_by(body)), "live pawn contact remains a body hit")
    local fists = { IsValid = valid, GetClass = function() return { GetFName = function() return FName("Weapon_Fists_C") end } end }
    local old_r, old_l = ME["Weapon R"], ME["Weapon L"]
    ME["Weapon R"], ME["Weapon L"] = weapon, fists
    weapon.K2_GetRootComponent = function() return weapon end
    fists.K2_GetRootComponent = function() return fists end
    local fist_bits, fist_slot, fist_reason = api.source(comp_owned_by(fists), ME)
    T.check(fist_bits == 262144 + 524288, "fist source keeps offhand identity")
    T.check(fist_slot == nil and fist_reason == "missing_array", "missing fist collision array is diagnosed explicitly")
    T.check(api.is_weapon(comp_owned_by(fists)), "native fist component retains weapon replay flag")
    local old_id = api.C3.me_id; api.C3.me_id = 1
    T.check(api.C3.hitter(1, { flags = 224, dism_blunt = fist_bits }) == nil, "protocol8 missing original fist ordinal cannot substitute a root component")
    T.check(api.C3.hitter(1, { flags = 224, dism_blunt = 1048576 }) == nil, "protocol8 missing original weapon ordinal cannot substitute a root component")
    local head, gap, grip = comp_owned_by(weapon), { IsValid = function() return false end }, comp_owned_by(weapon)
    weapon["Collision Components Array"] = { ForEach = function(_, f)
        for i, c in ipairs({head, gap, grip}) do f(i - 1, {get = function() return c end}) end
    end }
    local grip_bits, slot = api.source(grip, ME)
    T.check(slot == 3 and grip_bits == 1048576 + 3 * 2097152, "source preserves native array ordinal across invalid entries")
    T.check(api.C3.hitter(1, { flags = 224, dism_blunt = grip_bits }) == grip, "haft grip replay does not substitute weapon head")
    T.check(api.C3.hitter(1, {flags=224,dism_blunt=3*2097152}) == nil,
        "modern handless ordinal cannot substitute a held weapon sharing its array slot")
    local unheld = {IsValid=valid,GetClass=weapon.GetClass,["Collision Components Array"]=weapon["Collision Components Array"]}
    local _, unheld_slot, unheld_reason = api.source(comp_owned_by(unheld), ME)
    T.check(unheld_slot == nil and unheld_reason == "unknown_weapon_owner",
        "unheld native source cannot fabricate an original hand identity")
    head.GetClass = function() return {GetFName=function() return FName("BoxComponent") end} end
    grip.GetClass = function() return {GetFName=function() return FName("SphereComponent") end} end
    head.GetChildrenComponents=function()return {}end
    grip.GetChildrenComponents=function()return {}end
    weapon["Hit Box Collision"]=head
    T.check(api.native_hit_box(nil, grip, ME) == 0, "native nil HitBox remains absent")
    T.check(api.native_hit_box(head, grip, ME) == 1, "native cutting box identity differs from striking grip")
    T.check(api.native_hit_box(grip, head, ME) == nil, "non-box cutting component is never invented")
    T.check(api.native_hit_box(head, comp_owned_by(fists), ME) == nil, "cutting box cannot belong to another weapon")
    local box_record = {flags=224,dism_blunt=grip_bits + 134217728}
    T.check(api.C3.hit_box(1, box_record, grip) == head, "replay resolves original cutting box separately from striking collider")
    T.check(api.C3.hit_box(1, {flags=224,dism_blunt=grip_bits}, grip) == nil, "nil cutting box never falls back to head")
    T.check(api.C3.hit_box(1, {flags=224,dism_blunt=grip_bits+2*134217728}, grip) == nil, "missing cutting box never falls back")
    local args = api.C3.dcd_args(box_record, ME.Mesh, {at={0,0,0},nrm={0,0,0},vel={0,0,0},imp={0,0,0}}, grip, head)
    T.check(args[2] == grip and args[15] == head, "native replay receives distinct original collided component and HitBox")
    T.check(api.C3.hitter(1, { flags = 224, dism_blunt = 1048576 + 2 * 2097152 }) == nil, "invalid claimed component cannot fall back to weapon root")
    T.check(api.C3.hitter(1, { flags = 224, dism_blunt = 1048576 + 8 * 2097152 }) == nil, "missing claimed component cannot fall back to weapon head")
    local unknown_bits, unknown_slot, unknown_reason = api.source(comp_owned_by(weapon), ME)
    T.check(unknown_slot == nil and unknown_bits == 1048576 and unknown_reason == "component_not_listed", "unlisted native collider is diagnosed without fabricated ordinal")
    weapon["Collision Components Array"] = { ForEach = function(_, f)
        for i = 1, 16 do f(i - 1, {get = function() return i == 16 and grip or gap end}) end
    end }
    local _, unsupported_slot, unsupported_reason = api.source(grip, ME)
    T.check(unsupported_slot == nil and unsupported_reason == "unsupported_index", "native collider outside supported ordinals is diagnosed")
    local sphere = comp_owned_by(fists)
    fists["Collision Components Array"] = { ForEach = function(_, f)
        for i = 1, 10 do f(i - 1, {get = function() return i == 10 and sphere or gap end}) end
    end }
    local sphere_bits, sphere_slot = api.source(sphere, ME)
    T.check(sphere_slot == 10 and sphere_bits == 262144 + 524288 + 10 * 2097152, "native fist Sphere after nine inherited entries is claimed exactly")
    T.check(api.C3.hitter(1, { flags = 224, dism_blunt = sphere_bits }) == sphere, "native fist Sphere ordinal10 replays instead of inherited head")
    local foot = { IsValid = valid, GetClass = function() return { GetFName = function() return FName("Weapon_Feet_C") end } end }
    local box = comp_owned_by(foot)
    foot["Collision Components Array"] = { ForEach = function(_, f)
        for i = 1, 10 do f(i - 1, {get = function() return i == 10 and box or gap end}) end
    end }
    local old_foot = ME["Foot R Weapon"]; ME["Foot R Weapon"] = foot
    local foot_bits, foot_slot = api.source(box, ME)
    T.check(foot_slot == 10 and foot_bits == 33554432 + 1048576 + 10 * 2097152, "native kick keeps right foot and exact Box ordinal")
    T.check(api.C3.hitter(1, { flags = 224, dism_blunt = foot_bits }) == box, "kick replays native foot Box despite held sword and fist")
    ME["Foot R Weapon"] = weapon
    T.check(api.C3.hitter(1, { flags = 224, dism_blunt = foot_bits }) == nil, "missing foot actor never substitutes held sword")
    local _, unknown_foot, unknown_foot_reason = api.source(box, ME)
    T.check(unknown_foot == nil and unknown_foot_reason == "unknown_foot_owner", "unattributed kick actor is refused explicitly")
    ME["Foot R Weapon"] = old_foot
    ME["Weapon L"] = weapon
    T.check(api.C3.hitter(1, { flags = 224, dism_blunt = fist_bits }) == nil, "changed fist hand does not substitute a blade")
    local native_fists=api.C3.fist_replay
    local historical_calls=0
    api.C3.fist_replay={component=function(pawn,left,ordinal,original)
        historical_calls=historical_calls+1
        T.check(pawn==ME and left and ordinal==10,"historical fist keeps exact attacker/hand/Sphere identity")
        T.check(original.match_id==12345 and original.round==3 and original.life==1,"historical fist factory receives immutable claim life")
        return sphere
    end}
    local historical={flags=224,dism_blunt=sphere_bits,source_class="Weapon_Fists_C",match_id=12345,round=3,attacker_life=1}
    T.check(api.C3.hitter(1,historical)==sphere,"accepted historical punch retains native fist after the current hand holds a blade")
    T.check(ME["Weapon L"]==weapon,"historical source does not replace current held equipment")
    ME["Weapon L"]=nil
    T.check(api.C3.hitter(1,historical)==sphere,"accepted historical punch retains native fist after temporary actor disappeared")
    historical.source_class="Weapon_Sword_C"
    T.check(api.C3.hitter(1,historical)==nil and historical_calls==2,"fist metadata cannot construct a source for a different class")
    api.C3.fist_replay=native_fists
    ME["Weapon R"], ME["Weapon L"], api.C3.me_id = old_r, old_l, old_id
end
do
    local at, bone = { X = 100, Y = 0, Z = 0 }, FName("lowerarm_l")
    local calls, com_reads = {}, 0
    local function rotating(omega, expected_bone)
        return {
            GetPhysicsLinearVelocityAtPoint = function(_, p, b)
                calls[#calls + 1] = { p = p, bone = b:ToString() }
                T.check(p == at and b:ToString() == expected_bone, "point velocity samples the shared contact and correct physics body")
                -- v(point) = v(COM) + omega cross (point - COM).
                return { X = 10 - omega*p.Y, Y = 20 + omega*p.X, Z = 0 }
            end,
            GetPhysicsLinearVelocity = function() com_reads = com_reads + 1; return { X = 10, Y = 20, Z = 0 } end,
        }
    end
    T.check(api.C3.vrel(rotating(2, "None"), rotating(1, "lowerarm_l"), bone, at) == 100,
        "rotating bodies with identical COM velocities retain their contact-point relative speed")
    T.check(com_reads == 0 and #calls == 2, "available point velocities never fall back to COM")
    local pivot = {
        GetPhysicsLinearVelocityAtPoint = function() return zero end,
        GetPhysicsLinearVelocity = function() error("stationary pivot must not use moving COM") end,
    }
    T.check(api.C3.vrel(pivot, pivot, bone, at) == 0, "zero point velocity is preserved")
    local old_weapon = { GetPhysicsLinearVelocity = function() return { X = 30, Y = 40, Z = 0 } end }
    local old_victim = { GetPhysicsLinearVelocityAtPoint = function() error("unavailable") end,
        GetPhysicsLinearVelocity = function(_, b) T.check(b == bone, "COM fallback retains victim bone"); return zero end,
        GetComponentVelocity = function() return zero end }
    T.check(api.C3.vrel(old_weapon, old_victim, bone, at) == 50, "unavailable point query falls back to legacy physics velocity")
    local resting_limb = {
        GetPhysicsLinearVelocityAtPoint = function() error("unavailable") end,
        GetPhysicsLinearVelocity = function(_, b)
            T.check(b == bone, "stationary COM fallback samples the requested limb")
            return zero
        end,
        GetComponentVelocity = function() error("stationary limb must not inherit moving component") end,
    }
    T.check(api.C3.vrel(resting_limb, pivot, bone, at, bone) == 0,
        "unavailable point query preserves stationary physics limb instead of inventing component motion")
end

-- Original native ComponentHit supplies the striking skeletal physics body,
-- even after nested DCD capture. A collapsed pelvis must not mask limb speed.
do
    local point = { X=1, Y=2, Z=3 }
    local source = {
        GetAddress=function() return 9101 end,
        GetBoneIndex=function(_, b) return b:ToString()=="lowerarm_r" and 7 or -1 end,
        GetPhysicsLinearVelocityAtPoint=function(_, p, b)
            T.check(p.X==1 and p.Y==2 and p.Z==3, "body velocity samples original contact")
            return {X=b:ToString()=="lowerarm_r" and 1200 or 0,Y=0,Z=0}
        end,
    }
    local victim = { GetAddress=function() return 9102 end,
        GetPhysicsLinearVelocityAtPoint=function(_, _, b)
            T.check(b:ToString()=="thigh_r", "native body contact retains victim bone")
            return zero
        end }
    local rec = { at=CLOCK, loc={1,2,3}, bone="thigh_r", body_source=true,
        source_address=9101, target_address=9102, vrel=0 }
    local prior=api.CX.pending; api.CX.pending={rec}
    api.C3.body_hit(ME,source,SI,victim,zero,
        {ImpactPoint=point,MyBoneName=FName("lowerarm_r"),BoneName=FName("thigh_r")})
    T.check(rec.vrel==1200 and rec.source_bone=="lowerarm_r", "original body-hit source bone preserves moving limb with stationary pelvis")
    local done=rec.vrel; source.GetPhysicsLinearVelocityAtPoint=function() error("resolved callback must not be remeasured") end
    api.C3.body_hit(ME,source,SI,victim,zero,
        {ImpactPoint=point,MyBoneName=FName("lowerarm_r"),BoneName=FName("thigh_r")})
    T.check(rec.vrel==done, "same original contact resolves only once")
    source.GetPhysicsLinearVelocityAtPoint=function() return zero end
    local function pending() return {at=CLOCK,loc={1,2,3},bone="thigh_r",body_source=true,source_address=9101,target_address=9102,vrel=0} end
    rec=pending(); api.CX.pending={rec}
    api.C3.body_hit(ME,source,SI,victim,zero,{ImpactPoint=point,MyBoneName=FName("lowerarm_r"),BoneName=FName("thigh_r")})
    T.check(rec.vrel==0 and rec.source_bone=="lowerarm_r", "stationary touching limb remains zero despite authentic body identity")
    rec=pending(); api.CX.pending={rec}
    api.C3.body_hit(ME,source,SI,victim,zero,{ImpactPoint={X=10,Y=2,Z=3},MyBoneName=FName("lowerarm_r"),BoneName=FName("thigh_r")})
    T.check(rec.source_bone==nil, "other contact location cannot relabel pending body source")
    api.C3.body_hit(ME,source,SI,victim,zero,{ImpactPoint=point,MyBoneName=FName("None"),BoneName=FName("thigh_r")})
    T.check(rec.source_bone==nil, "missing native source bone never falls back to root")
    api.C3.body_hit(ME,source,SI,victim,zero,{ImpactPoint=point,MyBoneName=FName("lowerarm_r"),BoneName=FName("pelvis")})
    T.check(rec.source_bone==nil, "same component and point with different victim bone cannot relabel a claim")
    local native_event={Mesh=source,["Hit Component"]=source,["Other Comp"]=victim,
        ["Body Hit Impact Point"]=point,["Body Hit Bone Name Self"]=FName("lowerarm_r")}
    api.C3.body_hit(native_event,source,SI,victim,zero,{ImpactPoint=point,MyBoneName=FName("None"),BoneName=FName("thigh_r")})
    T.check(rec.source_bone=="lowerarm_r", "exact native closest-physics-body result resolves absent MyBoneName")
    rec=pending();api.CX.pending={rec}
    native_event["Body Hit Bone Name Self"]=FName("pelvis")
    source.GetBoneIndex=function(_,b) return b:ToString()=="lowerarm_r" and 7 or b:ToString()=="pelvis" and 0 or -1 end
    api.C3.body_hit(native_event,source,SI,victim,zero,{ImpactPoint=point,MyBoneName=FName("lowerarm_r"),BoneName=FName("thigh_r")})
    T.check(rec.source_bone=="lowerarm_r","valid original contact body overrides differing closest pelvis fallback")
    native_event["Body Hit Bone Name Self"]=FName("lowerarm_r")
    rec=pending();api.CX.pending={rec};native_event["Body Hit Impact Point"]={X=9,Y=2,Z=3}
    api.C3.body_hit(native_event,source,SI,victim,zero,{ImpactPoint=point,MyBoneName=FName("None"),BoneName=FName("thigh_r")})
    T.check(rec.source_bone==nil,"stale native closest-body field cannot supply absent callback bone")
    rec.at=CLOCK-0.1
    api.C3.body_hit(ME,source,SI,victim,zero,{ImpactPoint=point,MyBoneName=FName("lowerarm_r"),BoneName=FName("thigh_r")})
    T.check(rec.source_bone==nil, "late unrelated ComponentHit cannot revive stale body record")
    rec=pending(); api.CX.pending={rec}
    api.C3.body_hit(SI,victim,ME,source,zero,{ImpactPoint=point,MyBoneName=FName("thigh_r"),BoneName=FName("lowerarm_r")})
    T.check(rec.source_bone=="lowerarm_r", "victim native callback uses original OtherComp BoneName for source")
    api.CX.pending=prior
end

-- UE4SS calls a Blueprint hook back AFTER the body: this is the callback of a
-- Deal Complex Damage my weapon made on the stand-in. The body's contact gate
-- runs first, as in game: |Hit Impulse|·(CP+1) >= Last Complex Damage Impulse
-- or a new bone; a pass stores both and arms the 0.1 s reset (Delay).
local GATE_RESET_AT = nil
local function dcd_gate(w, bone, imp, cut)
    if GATE_RESET_AT and CLOCK >= GATE_RESET_AT then w["Last Complex Damage Impulse"], GATE_RESET_AT = 0, nil end
    local g = math.abs(imp) * ((cut or 50.0) + 1)
    local lb = w["Last Complex Damage Bone"] and w["Last Complex Damage Bone"]:ToString() or "None"
    if not (g >= w["Last Complex Damage Impulse"] or lb ~= bone) then return end
    w["Last Complex Damage Impulse"], w["Last Complex Damage Bone"] = g, FName(bone)
    GATE_RESET_AT = GATE_RESET_AT or (CLOCK + 0.1)
end
local function hit_si(bone, vel, cut)
    dcd_gate(SI, bone, vel * 0.6, cut)
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
    if t.match_id == nil then t.match_id=4242 end
    if t.attacker_life == nil then t.attacker_life=1 end
    if t.victim_life == nil then t.victim_life=1 end
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
T.check(l1.cid == 1 and l1.lage_ms == 10 and l1.target_peer_id == 2 and l1.hit_id == 0 and l1.round == 3
    and l1.match_id==4242 and l1.attacker_life==1 and l1.victim_life==1,
    "claim carries original match/round/lives, cid and age (hit_id is the sidecar's)", T.repr(l1))
T.check(l1.flags == 32 and l1.n == 0 and empty(l1.rows), "armour-stage claim, no measured deltas", T.repr(l1))
T.check(SI.Health == si0[1] and SI.Pain == si0[2] and SI["All Body Tonus"] == si0[3] and SI["Flinch Index"] == si0[4],
    "the stand-in is not touched by claiming (nothing buffered or measured)")

-- ---- later frames: every call the game's own contact gate lets through ------------
-- (on the victim in solo exactly these reach Get Damage; weaker frames on the
-- same bone inside the gate window never do)
CLOCK = 10.050; hit_si("spine_03", 1300); api.set_tick(101); api.flush_claims()
T.check(#damage_lines() == 4, "a harder frame of the same contact passes the gate: claimed", #damage_lines())
CLOCK = 10.060; hit_si("spine_03", 1000); api.set_tick(102); api.flush_claims()
T.check(#damage_lines() == 4, "a weaker frame on the same bone is stopped by the gate: no claim", #damage_lines())
CLOCK = 10.170; hit_si("spine_03", 1000); api.set_tick(103); api.flush_claims()
T.check(#damage_lines() == 5, "after the gate's 0.1 s reset a weaker frame is a blow again", #damage_lines())
CLOCK = 10.300; api.set_tick(104); api.flush_claims()
T.check(#damage_lines() == 5, "no claim without a call", #damage_lines())
CLOCK = 11.000; hit_si("spine_03", 1200); api.set_tick(105); api.flush_claims()
T.check(#damage_lines() == 6, "a new contact -> new claim", #damage_lines())
do  -- two frames passing the gate in one tick: both, in order (solo applies both)
    CLOCK = 11.500; hit_si("thigh_l", 400); CLOCK = 11.516; hit_si("thigh_l", 900)
    api.set_tick(106); api.flush_claims()
    local dl2 = damage_lines()
    T.check(#dl2 == 8 and dl2[7].bone == "thigh_l" and v3is(dl2[7].velocity, 400, 0, 0) and v3is(dl2[8].velocity, 900, 0, 0),
        "a graze then a harder frame in one tick: two claims in order", T.repr({ dl2[7], dl2[8] }))
end
do -- A blade crosses torso/head/torso: bone grouping must not reorder gates.
    local n = #damage_lines()
    CLOCK = 11.700; hit_si("spine_03", 800)
    CLOCK = 11.705; hit_si("head", 900)
    CLOCK = 11.710; hit_si("spine_03", 700)
    api.flush_claims()
    local ds = damage_lines()
    T.check(#ds == n + 3 and ds[n + 1].bone == "spine_03" and ds[n + 2].bone == "head"
        and ds[n + 3].bone == "spine_03", "interleaved bones preserve native contact order", T.repr(ds))
end
local NCLAIMS = #damage_lines()

do -- Equal strength retriggers native Get Damage's 200 ms gate.
    local w = mk_willie("equal_gate", { ["Last Damaged Bone"] = FName("head"),
        ["Last Damage Taken"] = 500, ["Sustained Damage"] = 1000 })
    api.C3.gd_last = { attacker = 2, ats = 1000, bone = "head", name = "head", ldt = 500, pawn = w:GetAddress() }
    local s = api.C3.gd_gate_open(w, 2, { attacker_ts = 1150 }, "head")
    w["Sustained Damage"] = 1500 -- accepted equal hit leaves LDT unchanged
    api.C3.gd_gate_note(w, 2, { attacker_ts = 1150 }, "head", s)
    T.check(api.C3.gd_last.ats == 1150, "equal-strength accepted contact advances native gate time")
    w["Last Damage Taken"] = 0 -- victim's local reset elapsed before delayed replay
    api.C3.gd_gate_open(w, 2, { attacker_ts = 1300 }, "head")
    T.check(w["Last Damage Taken"] == 500, "delay is measured from last accepted equal contact")
    api.C3.gd_gate_open(w, 2, { attacker_ts = 1300, victim_life = 2 }, "head")
    T.check(w["Last Damage Taken"] == 0, "new victim life cannot inherit the previous native damage gate")
    local gl = api.C3.gd_last
    local replacement = mk_willie("replacement_gate", {["Last Damage Taken"] = 99})
    api.C3.gd_gate_open(replacement, 2, {attacker_ts=1300}, "head")
    T.check(replacement["Last Damage Taken"] == 0, "new native pawn cannot inherit same-life cached bone gate")
    for _, field in ipairs({"match_id", "round", "attacker_life"}) do
        local d = {attacker_ts=1300}; d[field] = 2
        w["Last Damage Taken"] = 500; api.C3.gd_last = gl
        api.C3.gd_gate_open(w, 2, d, "head")
        T.check(w["Last Damage Taken"] == 0, "native bone gate cannot cross " .. field)
    end
    api.C3.gd_last = nil
end

-- ---- a Get Damage callback never makes a claim ------------------------------------
-- (claiming it too would make two claims per blow, one with -99900 deltas)
CLOCK = 12.000
api.on_get_damage(SI, zero, zero, { X = 1, Y = 2, Z = 3 }, zero, FName("spine_03"), 700, 0.5, false, SI.Mesh, 0,
    false, false, comp_owned_by(ME))
api.set_tick(106); api.flush_claims(); CLOCK = 13.0; api.set_tick(107); api.flush_claims()
T.check(#damage_lines() == NCLAIMS, "Get Damage on a stand-in is not a claim (Deal Complex Damage is)", #damage_lines())
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
T.check(q.claims == NCLAIMS and q.accepted == 1 and q.confirmed == 1 and q.clashes == 1, "quality counters", T.repr(q))
T.check(q.rejected.parried == 1 and q.rejected.body_miss == 1, "rejects counted by reason code", T.repr(q.rejected))
local cues = read(".combat_cue.jsonl")
T.check(cues == nil or cues == "", "no .combat_cue.jsonl (written, never read) any more", cues)
local logs = table.concat(T.map(LOGS, tostring), "")
T.check(T.contains(logs, "REJECTED by server: body_miss: contact 40 uu"), "reject logged with its reason")
CLOCK = 20.0
api.emit_quality(true)
local ev = read("hsmp_events.jsonl")
T.check((T.contains(ev, '"ev":"combat_quality"') or T.contains(ev, '"ev":"x_combat_quality"')) and T.contains(ev, '"rejected_by_reason":{')
    and T.contains(ev, string.format('"claims":%d', NCLAIMS)), "combat_quality event written (hsmp_log)", ev)

-- ---- solo parity: armour-stage capture on the stand-in, replay on the victim -------
CLOCK = 30.0
local WCOMP = comp_owned_by(ME)
api.on_complex(SI, SI.Mesh, WCOMP, FName("spine_03"), { X = 1, Y = 2, Z = 3 }, { X = 0, Y = 1, Z = 0 },
    { X = 1500, Y = 0, Z = 0 }, { X = 900, Y = 0, Z = 0 }, 120.0, 0.25, 0.85, 2, true, true, 1.0, nil, false, 0.0)
CLOCK = 30.01; api.set_tick(200); api.flush_claims()
local all = damage_lines()
local lc = all[#all] or {}
local dc = dec(lc)
T.check(dc.flags == 32 and v3is(dc.velocity, 1500, 0, 0) and v3is(dc.impulse, 900, 0, 0)
    and dc.cutting_power == 120 and math.abs((dc.pain_rate or 0) - 0.25) < 1e-6 and math.abs((dc.damage_out or 0) - 0.85) < 1e-6
    and dc.dism_blunt == (2 + 10 * 256 + 65536 + 67108864), "claim carries the armour-stage inputs (FLAG_COMPLEX)", T.repr(lc))

local VX = mk_willie("Willie_BP_C_VX")
local got
VX["Deal Complex Damage"] = function(self, hc, cc, bone, loc, nrm, vel, imp, cut, stab, rig, dism, lower, parent, kick, hb, xhv, draw)
    got = { bone = bone.s, vel = vel.X, imp = imp.X, cut = cut, stab = stab, rig = rig, dism = dism, lower = lower, kick = kick, parent = parent }
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
    and got.dism == 2 and got.lower == true and got.kick == 1.0 and got.parent == true, "victim replays Deal Complex Damage with the decoded inputs", T.repr(got))
T.check(VX.Health == 97.5 and T.contains(r2, "Deal Complex Damage(ok)"),
    "only the victim's own armour stage decided the damage", T.repr({ VX.Health, r2 }))
do
    local previous_probe,previous_dcd=api.C3.native_probe,VX["Deal Complex Damage"]
    local previous_guard=api.WG.check;api.WG.check=function()return true end
    api.C3.native_probe=true
    local native_calls,log_start=0,#LOGS
    VX["Deal Complex Damage"]=function(self,...)
        native_calls=native_calls+1
        local args=table.pack(...)
        local array=function(items)return{GetArrayNum=function()return #items end,
            ForEach=function(_,f)for i,v in ipairs(items)do f(i,{get=function()return v end})end end}end
        api.C3.native_trace_post(nil,ME,zero,zero,.1,array({11}),true,nil,0,array({}),false,nil,nil,5,false)
        api.C3.native_trace_post(nil,self,zero,zero,.1,array({11}),true,nil,0,array({}),false,nil,nil,5,false)
        self["Last Hit Body Part"]=2;self.Health=self.Health-.25
        api.on_get_damage(self,zero,zero,zero,zero,FName("spine_03"),123.45678901234567,
            .12345678901234566,false,self.Mesh,0,false,false,args[2],true,nil,1,false,0,true)
        api.on_complex(self,args[1],args[2],args[3],args[4],args[5],args[6],args[7],args[8],args[9],
            args[10],args[11],args[12],args[13],args[14],args[15],args[16],args[17],
            2,123.45678901234567,.12345678901234566,.85,7800.125,true)
    end
    local before=VX.Health
    api.apply_hit(hit_rec(inl),7)
    local evidence
    for i=log_start+1,#LOGS do if T.contains(LOGS[i],"LAB_NATIVE side=replay ")then evidence=LOGS[i]end end
    T.check(native_calls==1 and VX.Health==before-.25,"native evidence hooks never add another damage application")
    T.check(evidence and T.contains(evidence,"attacker=7") and T.contains(evidence,"cid=5 parent_cid=0")
        and T.contains(evidence,"match=4242 round=3 attacker_life=1 victim_life=1"),
        "native replay evidence retains exact original hit and life identity",evidence)
    T.check(evidence and T.contains(evidence,"dcd_surface=2") and T.contains(evidence,"dcd_density=7800.125")
        and T.contains(evidence,"dcd_cp="..api.C3.native_scalar(.12345678901234566)),
        "owner replay records actual armour-stage output doubles at full precision",evidence)
    T.check(evidence and T.contains(evidence,"dcd_calls=1 gd_calls=1") and T.contains(evidence,"bone:spine_03,zone:2")
        and T.contains(evidence,"out:true"),"owner replay pairs successful nested native Get Damage with body zone",evidence)
    T.check(evidence and T.contains(evidence,"trace_calls=1") and T.contains(evidence,"objects:11")
        and T.contains(evidence,"hits:0"),"only exact active owner context captures existing native trace output",evidence)
    api.C3.native_probe,VX["Deal Complex Damage"]=previous_probe,previous_dcd
    api.WG.check=previous_guard
end
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
    local frame_calls = 0
    V["Deal Complex Damage"] = function() frame_calls = frame_calls + 1 end
    cline.flags = 96
    local missing_frame, missing_status = api.apply_hit(cline, 7)
    T.check(frame_calls == 0 and missing_status == 4 and T.contains(missing_frame, "victim bone frame unavailable"),
        "local hit without victim bone frame cannot replay stale world location and unrotated local vectors")
    cline.flags = 32
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
    -- (c) an uncertain native failure is terminal even without numeric change:
    -- structural/async effects may already have executed before copying outputs.
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
    T.check(calls == 1 and V.Health == 93, "an uncertain native failure never invokes Get Damage a second time", T.repr({ calls, V.Health }))
    -- Kick Power 0 is replayed as 0
    local kick
    V["Deal Complex Damage"] = function(self, hc, cc, bone, loc, nrm, vel, imp, cut, stab, rig, dism, lower, parent, k)
        kick = k
    end
    api.apply_hit(cline, 7)
    T.check(kick == 0, "kick power 0 stays 0 (was replayed as 1)", tostring(kick))
    api.C3.diag_counts.gate2=0
    local original_hp=V.Health
    V["Deal Complex Damage"]=function(self)
        api.on_get_damage(self,zero,zero,zero,zero,FName("spine_03"),777,0,false,self.Mesh,0,false,false,nil,false,nil,0,false,0,false)
    end
    local result,status=api.apply_hit(cline,7)
    local diagnostic=LOGS[#LOGS] or ""
    T.check(status==2 and V.Health==original_hp and gd==0,"native gate diagnostic never changes damage outcome or reapplies hit")
    T.check(diagnostic:find("native_calls=1",1,true) and diagnostic:find("native_raw=777",1,true)
        and diagnostic:find("native_applied=false",1,true),"gate2 diagnostic captures actual nested GetDamage inputs/output rather than claim raw")
    T.check(api.C3.replay_trace==nil,"native replay diagnostic context cleared after call")
    local last=#LOGS
    for _=1,20 do api.C3.replay_diag("rate_test","example")end
    T.check(#LOGS-last==4,"diagnostics limit repeated contact logs to first four and then one per second")
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
-- Native injuries can occur before the first accepted network claim. Spawn
-- preparation must finish at Live, even if that pawn still has hits == 0.
do
    local saved_find, saved_clock = StaticFindObject, CLOCK
    StaticFindObject = function() return mk_willie("CDO") end
    local fresh = mk_willie("fresh", { ["Arm_L Health"] = 40 })
    match("countdown", 3); api.refresh_match()
    api.own_pawn_tick(fresh)
    CLOCK = CLOCK + 0.6; api.own_pawn_tick(fresh)
    T.check(fresh["Arm_L Health"] == 100, "fresh countdown pawn gets career wound preparation")
    fresh["Arm_L Health"], fresh.Bleeding = 15, 5
    match("live", 3); api.refresh_match(); api.own_pawn_tick(fresh)
    CLOCK = CLOCK + 4; api.own_pawn_tick(fresh)
    T.check(fresh["Arm_L Health"] == 15 and fresh.Bleeding == 5,
        "native injury with no network hits survives Live and delayed spawn passes")
    match("countdown", 4); api.refresh_match(); api.own_pawn_tick(fresh)
    T.check(fresh["Arm_L Health"] == 15, "same injured body is not healed by a phase change")
    local severed = mk_willie("severed", { Headless = true, ["Head Health"] = 0 })
    api.own_pawn_tick(severed); CLOCK = CLOCK + 0.6; api.own_pawn_tick(severed)
    T.check(severed["Head Health"] == 0, "preparation cannot heal a headless body with a numeric reset")
    StaticFindObject, CLOCK = saved_find, saved_clock
    match("live", 3); api.refresh_match(); api.own_pawn_tick(ME)
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
    T.check(S2["Force Disable Dismemberment"] == true, "protect: stand-in native severing guard set")
    T.check(S2["Force Disable Vertex Paint"] == true,"native stand-in paint guard suppresses speculative armour/skin marks")
    local probe_before=api.C3.native_probe
    api.C3.native_probe=true;api.C3.protect(S2)
    local paints=0
    S2["Deal Complex Damage"]=function(self)
        if not self["Force Disable Vertex Paint"] then paints=paints+1 end
        if not self.Invulnerable then self.Health=self.Health-2 end
    end
    S2["Deal Complex Damage"](S2)
    T.check(S2.Health==998 and paints==0,"probe still samples native damage while speculative paint is disabled")
    api.C3.native_probe=probe_before
    api.C3.ungate(S2)
    T.check(S2.Invulnerable == false and S2["Force Disable Dismemberment"] == false,
        "pooled stand-in possessed locally regains native injury")
    T.check(S2["Force Disable Vertex Paint"] == false,"confirmed local pooled pawn regains native paint")
    api.C3.protect(S2) -- re-establish weapon provenance for the pickup checks below
    ME["Weapon R"] = SW                      -- I picked its weapon up
    ME.Invulnerable = true                   -- (the game's own spawn protection: left alone)
    ME["Force Disable Dismemberment"] = true -- not a stand-in pawn: preserve native setting
    ME["Force Disable Vertex Paint"] = true
    api.C3.ungate(ME)
    T.check(SW["Temp Disable Damage"] == false and ME.Invulnerable == true,
        "a gated stand-in weapon in my hands deals damage again; my own Invulnerable untouched")
    T.check(ME["Force Disable Dismemberment"] == true, "normal own pawn dismemberment setting is untouched")
    T.check(ME["Force Disable Vertex Paint"] == true,"normal own pawn native paint setting is untouched")
    ME["Force Disable Vertex Paint"] = nil
    ME["Force Disable Dismemberment"] = false
    SW["Temp Disable Damage"] = true         -- the game's own 0.2 s gate later
    api.C3.ungate(ME)
    T.check(SW["Temp Disable Damage"] == true, "lifted once only (the game's own gate is left alone)")
    ME["Weapon R"], ME.Invulnerable = nil, nil
end

-- Deathmatch: a peer whose life count goes up in the round (the `mode` record) is alive
-- again; its earlier death stops holding the stand-in.
do
    local GM = ES.game_mode
    api.C3.life_for = real_life_for
    local previous_mid = SESS.match_id
    SESS.match_id = 12345; publish()
    local st = api.state()
    NAT.sc_put("mode", { match_id=12345, seq = 1, mode = GM.DEATHMATCH, round = 3, rows = { { peer_id = 2, seat = 1, life = 1 } } })
    api.C3.respawns()
    st.remote_dead[2] = "world#1"
    api.session().death_shown[2] = "Willie_BP_C_44"
    api.session().server_dead[2] = api.round_key(3)
    NAT.sc_put("mode", { match_id=12345, seq = 2, mode = GM.DEATHMATCH, round = 3, rows = { { peer_id = 2, seat = 1, life = 1, respawning = true } } })
    api.C3.respawns()
    T.check(api.state().remote_dead[2] ~= nil, "waiting for the respawn: still dead")
    T.check(api.C3.death_context(2, {match_id=12345, round=3, life=1}),
        "same-life death remains authoritative during the respawn countdown")
    NAT.sc_put("mode", { match_id=12345, seq = 3, mode = GM.DEATHMATCH, round = 3, rows = { { peer_id = 2, seat = 1, life = 2, alive = true } } })
    api.C3.respawns()
    T.check(api.state().remote_dead[2] == nil and api.session().death_shown[2] == nil and api.session().server_dead[2] == nil,
        "respawned (life 2): the death no longer holds the stand-in")
    T.check(not api.C3.death_context(2, {match_id=12345, round=3, life=1}),
        "late previous-life death is rejected after respawn finishes")
    T.check(api.C3.death_context(2, {match_id=12345, round=3, life=2}),
        "a genuine death in the current life remains authoritative")
    T.check(not api.C3.death_context(2, {match_id=54321, round=3, life=2}),
        "same-round death from another match cannot change the current context")
    T.check(not api.C3.death_context(2, {match_id=12345, round=2, life=2}),
        "death from another round is rejected")
    T.check(not api.C3.death_context(2, {match_id=12345, round=3}),
        "missing life identity is never assigned the current life")
    T.check(not api.C3.death_context(99, {match_id=12345, round=3, life=2}),
        "a death without a known peer life is rejected")
    SESS.match_id = previous_mid; publish()
    NAT._rec.slots.mode = nil
    api.C3.life_for = arithmetic_life_for
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
    api.C3.life_for = real_life_for
    api.C3.displayed_for=real_displayed_for
    local previous_mid, previous_rows = SESS.match_id, SESS.rows
    SESS.match_id = 12345
    SESS.rows = {{peer_id=9,seat=0,connected=true,alive=true,spawn_id=768},
                 {peer_id=2,seat=1,connected=true,alive=true,spawn_id=768}}
    publish()
    NAT.sc_put("mode", {match_id=12345,seq=100,mode=ES.game_mode.DEATHMATCH,round=3,
        rows={{peer_id=9,seat=0,life=1,alive=true},{peer_id=2,seat=1,life=1,alive=true}}})
    local status = {match_id=12345,life=1,round=3,spawn_id=768,pawn=ME.__name,verified=true}
    local function placed() NAT.bus_put("spawn_status", status) end
    placed()
    do
        local original_phase,original_round=SESS.phase,SESS.round
        SESS.phase,SESS.round=1,2;publish() -- Server Loading for the verified round3 order.
        T.check(real_life_for(9,true)==nil,"Loading spawn has no live combat life before Mode transition")
        local preparation=api.C3.vitals_context(ME)
        T.check(preparation and preparation.match_id==12345 and preparation.round==3 and preparation.life==1,
            "verified pending native spawn supplies separate preparation vitals context",T.repr(preparation))
        T.check(api.publish_own_vitals(ME),"native owner health publishes while new spawn is still Loading")
        local record=NAT._rec.slots.vitals and NAT._rec.slots.vitals.t
        T.check(record and record.match_id==12345 and record.round==3 and record.life==1,
            "preparation vitals carry the verified pending spawn identity",T.repr(record))
        local pending_pose={peer=2,pawn=SI.__name,match_id=12345,round=3,life=1,
            body_ts=100,arm_ts=100,local_ms=math.floor(CLOCK*1000)}
        NAT.bus_put("playback",{rows={pending_pose}})
        local pending_vitals=api.VQ.record(200,{v={[1]=100,[18]=7},f=0,dism={}})
        pending_vitals.match_id,pending_vitals.round,pending_vitals.life=12345,3,1
        NAT.sc_peer_dir({{id=2,slot=2,nick="pending owner"}})
        IPCF.peer_dir(true)
        NAT.sc_put("peer_vitals",pending_vitals,2)
        local observed=api.read_vitals(2,SI)
        T.check(observed and observed.round==3 and observed.life==1 and observed.hp==100,
            "pending receiver consumes only health for actual newly displayed native generation",
            T.repr({context=api.C3.mirror_context(2,SI),raw=IPCF.peer_rec("peer_vitals",2),observed=observed}))
        local oldhp,oldpain,olddeath=SI.Health,SI.Pain,SI.Death
        local deaths=0;SI.Death=function()deaths=deaths+1 end
        api.state().remote_dead[2]="world#1";api.session().server_dead[2]=api.round_key(2)
        api.C3.remote_dead_context[2]={match_id=12345,round=2,life=1}
        api.C3.server_death_context[2]={match_id=12345,round=2,life=1}
        api.update_standins()
        T.check(deaths==0 and SI.Health==oldhp and SI.Pain==7 and not api.state().remote_dead[2],
            "previous round death cannot kill fresh pending actor; actual new owner vitals mirror")
        pending_pose.round=2;NAT.bus_put("playback",{rows={pending_pose}})
        T.check(api.read_vitals(2,SI)==nil,"previous displayed generation cannot borrow pending owner vitals")
        pending_pose.round=3;pending_pose.pawn="PreviousStandin";NAT.bus_put("playback",{rows={pending_pose}})
        T.check(api.read_vitals(2,SI)==nil,"pending owner vitals cannot bind another native actor")
        pending_pose.pawn=SI.__name;pending_pose.local_ms=pending_pose.local_ms-251;NAT.bus_put("playback",{rows={pending_pose}})
        T.check(api.read_vitals(2,SI)==nil,"stalled pending display cannot initialize new owner health")
        SI.Health,SI.Pain,SI.Death=oldhp,oldpain,olddeath
        api.state().remote_dead[2],api.session().server_dead[2],api.session().death_shown[2]=nil,nil,nil
        api.C3.remote_dead_context[2],api.C3.server_death_context[2]=nil,nil
        status.verified=false;placed()
        T.check(api.C3.vitals_context(ME)==nil,"unverified preparation cannot publish owner vitals")
        status.verified=true;status.pawn="PreviousWillie";placed()
        T.check(api.C3.vitals_context(ME)==nil,"previous native pawn cannot supply pending owner vitals")
        status.pawn=ME.__name;status.spawn_id=769;placed()
        T.check(api.C3.vitals_context(ME)==nil,"wrong pending spawn order cannot supply owner vitals")
        status.spawn_id=768;status.life=2;placed()
        T.check(api.C3.vitals_context(ME)==nil,"preparation never borrows a respawned Mode life")
        status.life=1;status.match_id=54321;placed()
        T.check(api.C3.vitals_context(ME)==nil,"previous match cannot supply pending owner vitals")
        status.match_id=12345;status.round=2;placed()
        T.check(api.C3.vitals_context(ME)==nil,"previous round cannot supply pending owner vitals")
        status.round=3;placed()
        SESS.phase=7;publish()
        T.check(api.C3.vitals_context(ME)==nil,"Paused cannot relabel pending placement as server-supported preparation vitals")
        SESS.phase=3;publish()
        T.check(api.C3.vitals_context(ME)==nil,"Live cannot fall back to preparation instead of exact Mode life")
        SESS.phase,SESS.round=original_phase,original_round;publish()
    end
    do
        local original_mode=RL.deep(NAT._rec.slots.mode.t)
        local old_order=SESS.rows[1].spawn_id
        SESS.rows[1].spawn_id=769;publish()
        status.spawn_id,status.life=769,2;placed()
        NAT.sc_put("mode",{match_id=12345,seq=1001,mode=ES.game_mode.DEATHMATCH,round=3,
            rows={{peer_id=9,seat=0,life=2,alive=false,respawning=true},{peer_id=2,seat=1,life=1,alive=true}}})
        T.check(real_life_for(9,true)==nil,"assigned Deathmatch respawn still has no strict combat context")
        local respawn=api.C3.vitals_context(ME)
        T.check(respawn and respawn.round==3 and respawn.life==2 and api.publish_own_vitals(ME),
            "exact verified assigned Deathmatch spawn publishes preparation health before LOADED")
        status.spawn_id=768;placed()
        T.check(api.C3.vitals_context(ME)==nil,"Deathmatch preparation requires exact newly assigned spawn order")
        status.spawn_id=769;status.life=1;placed()
        T.check(api.C3.vitals_context(ME)==nil,"Deathmatch preparation cannot borrow old native life")
        status.life=2;placed()
        local not_dm=RL.deep(original_mode);not_dm.mode=ES.game_mode.DUEL;not_dm.rows[1].life=2;not_dm.rows[1].respawning=true
        NAT.sc_put("mode",not_dm)
        T.check(api.C3.vitals_context(ME)==nil,"respawn preparation is limited to the server Deathmatch contract")
        NAT.sc_put("mode",original_mode)
        SESS.rows[1].spawn_id=old_order;publish()
        status.spawn_id,status.life=768,1;placed()
    end
    T.check(real_life_for(9,true) ~= nil, "exact verified own pawn/order/match/life binds")
    status.verified=false; placed()
    T.check(real_life_for(9,true) == nil, "unverified own placement cannot send combat")
    status.verified=true; status.pawn="PreviousWillie"; placed()
    T.check(real_life_for(9,true) == nil, "previous pawn's verified status cannot bind current pawn")
    status.pawn=ME.__name; status.match_id=54321; placed()
    T.check(real_life_for(9,true) == nil, "same spawn order from previous match cannot bind")
    status.match_id=12345; status.life=2; placed()
    T.check(real_life_for(9,true) == nil, "verified status from another life cannot bind")
    status.life=1; status.spawn_id=769; placed()
    T.check(real_life_for(9,true) == nil, "different authoritative spawn order cannot bind")
    status.spawn_id=768; status.round=2; placed()
    T.check(real_life_for(9,true) == nil, "previous round's placement cannot bind")
    status.round=3; placed()
    local rendered={peer=2,pawn=SI.__name,match_id=12345,round=3,life=1,
        body_ts=100,arm_ts=100,local_ms=math.floor(CLOCK*1000)}
    local function displayed() NAT.bus_put("playback",{rows={rendered}}) end
    displayed()
    T.check(real_displayed_for(2,SI)~=nil,"exact original rendered pose binds native contact")
    rendered.life=2;displayed()
    T.check(real_displayed_for(2,SI)==nil,"same actor's other displayed life cannot be relabelled as current")
    rendered.life=1;rendered.pawn="PreviousStandin";displayed()
    T.check(real_displayed_for(2,SI)==nil,"displayed pose of previous actor cannot bind current collider")
    rendered.pawn=SI.__name;rendered.local_ms=rendered.local_ms-251;displayed()
    T.check(real_displayed_for(2,SI)==nil,"stalled displayed pose cannot create native contact")
    local trade=real_displayed_for(2,SI,false)
    T.check(trade and trade.life==1,"approved delayed trade retains exact original life despite stale pose time")
    local native_calls=0
    local original_native=ME["Deal Complex Damage"]
    ME["Deal Complex Damage"]=function(self)native_calls=native_calls+1;self.Health=self.Health-1 end
    local delayed=hit_rec{target_peer_id=9,bone="pelvis",flags=32,match_id=12345,round=3,attacker_life=1,victim_life=1}
    local hp_before=ME.Health
    local delayed_result=api.apply_hit(delayed,2)
    T.check(native_calls==1 and ME.Health==hp_before-1,
        "already approved stale-time same-life source executes native replay",delayed_result)
    ME["Deal Complex Damage"]=original_native
    api.flush_claims()
    local claims_before=#damage_lines()
    api.on_complex(SI,SI.Mesh,comp_owned_by(ME),FName("pelvis"),zero,zero,
        {X=1400,Y=0,Z=0},{X=800,Y=0,Z=0},40,0,.85,0,false,false,0,nil,false,0)
    api.flush_claims()
    T.check(#damage_lines()==claims_before,"same stale displayed sample cannot generate a new contact claim")
    rendered.life=2;displayed()
    T.check(real_displayed_for(2,SI,false)==nil,"delayed replay cannot borrow another displayed life")
    rendered.life=1
    rendered.local_ms=math.floor(CLOCK*1000);displayed()
    T.check(not api.C3.death_context(9,{match_id=12345,round=3,life=2}), "own old/future-life death cannot kill exact verified pawn")
    local hp0=ME.Health
    local stale=hit_rec({target_peer_id=9,bone="head",flags=32,match_id=12345,round=3,attacker_life=1,victim_life=2})
    T.check(T.contains(api.apply_hit(stale,2),"stale") and ME.Health==hp0,
        "late hit for another victim life is refused before native damage")
    stale.victim_life=1;stale.match_id=54321
    T.check(T.contains(api.apply_hit(stale,2),"stale") and ME.Health==hp0,
        "previous match's same-round hit is refused before native damage")
    local before_capture=#damage_lines()
    api.on_complex(SI,SI.Mesh,comp_owned_by(ME),FName("pelvis"),zero,zero,
        {X=1400,Y=0,Z=0},{X=800,Y=0,Z=0},40,0,.85,0,false,false,0,nil,false,0)
    status.life=2;placed()
    NAT.sc_put("mode", {match_id=12345,seq=101,mode=ES.game_mode.DEATHMATCH,round=3,
        rows={{peer_id=9,seat=0,life=2,alive=true},{peer_id=2,seat=1,life=1,alive=true}}})
    api.flush_claims()
    local captured=damage_lines()
    T.check(#captured==before_capture+1 and captured[#captured].match_id==12345
        and captured[#captured].round==3 and captured[#captured].attacker_life==1 and captured[#captured].victim_life==1,
        "queued native contact retains original life instead of being relabelled on flush",T.repr(captured[#captured]))
    status.life=1;placed()
    NAT.sc_put("mode", {match_id=12345,seq=102,mode=ES.game_mode.DEATHMATCH,round=3,
        rows={{peer_id=9,seat=0,life=1,alive=true},{peer_id=2,seat=1,life=1,alive=true}}})
    local fx_calls,fx_paints=0,0
    local original_dcd=SI["Deal Complex Damage"]
    SI["Deal Complex Damage"]=function(self)
        fx_calls=fx_calls+1
        if self["Force Disable Vertex Paint"]==false then fx_paints=fx_paints+1 end
    end
    local fx={target_peer_id=2,bone="head",flags=32,match_id=12345,round=3,attacker_life=2,victim_life=1}
    T.check(T.contains(api.C3.apply_fx(fx,9),"stale") and fx_calls==0,"old attacker life cannot paint current stand-in")
    fx.attacker_life=1;fx.victim_life=2
    T.check(T.contains(api.C3.apply_fx(fx,9),"stale") and fx_calls==0,"old victim life cannot paint respawned stand-in")
    fx.victim_life=1;fx.match_id=54321
    T.check(T.contains(api.C3.apply_fx(fx,9),"stale") and fx_calls==0,"previous match cannot paint same-round stand-in")
    fx.match_id=12345
    T.check(api.C3.apply_fx(fx,9)=="fx: replayed on stand-in" and fx_calls==1,"correct displayed-life cosmetic hit still invokes native replay")
    T.check(fx_paints==1 and SI["Force Disable Vertex Paint"]==true,
        "approved owner outcome paint opens only during native replay and closes afterward")
    SI["Deal Complex Damage"]=function(self)
        T.check(self["Force Disable Vertex Paint"]==false,"authorized cosmetic error occurs inside scoped paint permission")
        error("native cosmetic failure")
    end
    T.check(T.contains(api.C3.apply_fx(fx,9),"replay failed") and SI["Force Disable Vertex Paint"]==true,
        "native replay failure always restores stand-in paint guard")
    local original_args=api.C3.dcd_args
    api.C3.dcd_args=function()error("argument construction failure")end
    T.check(T.contains(api.C3.apply_fx(fx,9),"replay failed") and SI["Force Disable Vertex Paint"]==true,
        "argument failure also closes guard instead of leaving replay permission active")
    api.C3.dcd_args=original_args
    local old_mesh_valid=SI.IsValid
    local own_before_fx=RL.deep(api.own())
    SI["Deal Complex Damage"]=function()
        api.wg_drop("level change requested")
        SI.IsValid=function()error("old-world UObject touched")end
    end
    T.check(api.C3.apply_fx(fx,9)=="fx: world changed during replay",
        "world change during native cosmetics returns without touching discarded target wrappers")
    SI.IsValid=old_mesh_valid
    api.set_world("world#1")
    api.set_puppets({[SI.__name]=2},{[2]=SI},{})
    api.C3.protect(SI)
    for k in pairs(api.own())do api.own()[k]=nil end
    for k,v in pairs(own_before_fx)do api.own()[k]=v end
    SI["Deal Complex Damage"]=original_dcd
    do
        local oldweapon=ME["Weapon R"]
        local weapon={IsValid=valid,["Parent Actor"]=ME,GetClass=function()return{GetFName=function()return FName("ArmingSword_C")end}end,
            GetFullName=function()return "ArmingSword_Journal"end}
        local coll={IsValid=valid,GetOwner=function()return weapon end,GetFullName=function()return "ArmingSword_Journal.Box"end}
        local constraint={IsValid=valid,["My Weapon"]=weapon,["Weapon Hit Module"]=coll,["Hit Actor"]=SI,
            ["Component 2 (Body)"]=SI.Mesh,["Bone Name 2"]=FName("head"),GetFullName=function()return "Stuck_Journal"end}
        weapon["Collision Components Array"]={ForEach=function(_,f)f(0,{get=function()return coll end})end}
        weapon["Stuck Constraints Array"]={ForEach=function(_,f)f(0,{get=function()return constraint end})end}
        ME["Weapon R"]=weapon
        local claims=#sent("damage")
        T.check(api.C3.inside_journal(SI,coll,FName("head"),SI.Mesh,nil,1000,1000,0,1,false)==true,
            "direct Inside evidence proves exact current native constraint membership")
        T.check(#sent("damage")==claims,"direct Inside journal never forwards raw damage as an ordinary claim")
        constraint["Bone Name 2"]=FName("spine_03")
        T.check(api.C3.inside_journal(SI,coll,FName("head"),SI.Mesh,nil,1000,1000,0,1,false)==false,
            "wrong constraint target bone cannot be reported as proven")
        local old_mesh_address=SI.Mesh.GetAddress
        coll.GetAddress=function()return 7001 end
        SI.Mesh.GetAddress=function()return 7002 end
        SI.Mesh.GetFullName=function()return "JournalVictim.Mesh" end
        constraint["Bone Name 2"]=FName("head")
        local original_gate,original_gate_bone=SI["Last Complex Damage Impulse"],SI["Last Complex Damage Bone"]
        SI["Last Complex Damage Impulse"],SI["Last Complex Damage Bone"]=100000,FName("head")
        api.on_complex(SI,SI.Mesh,coll,FName("head"),zero,zero,
            {X=100,Y=0,Z=0},{X=1,Y=0,Z=0},40,0,.85,0,false,false,1,nil,false,0)
        local parent=api.CX.pending[#api.CX.pending]
        T.check(parent and parent.gate==false and not parent.cid,"suppressed native DCD retained before constraint construction")
        api.C3.constraint_begin_journal(constraint)
        T.check(parent and parent.constraint_parent,"exact original source/body callback binds suppressed penetration parent")
        local before=#sent("damage")
        local native_normal={X=0,Y=.8,Z=.6}
        api.C3.inside_forward(SI,coll,FName("head"),SI.Mesh,zero,1000,1000,0,1,false,false,false,false,0,native_normal,zero,zero)
        T.check(#sent("damage")==before and #(api.CX.inside_queue or {})==1,
            "initial native Inside before origin flush queues plain data instead of orphaning")
        api.flush_claims()
        local emitted=sent("damage")
        local origin,inside=emitted[before+1],emitted[before+2]
        T.check(#emitted==before+2 and origin and origin.flags==161 and inside and inside.parent_cid==origin.cid,
            "suppressed origin is authenticated without damage before its continuation",T.repr({origin,inside}))
        T.check(inside and inside.flags==129 and inside.source_class=="ArmingSword_C"
            and inside.dism_blunt==3145728 and math.abs(inside.normal[2]-.8)<1e-6 and math.abs(inside.normal[3]-.6)<1e-6,
            "continuation preserves exact source hand/module/class and native normal",T.repr(inside))
        local own_gd,own_dcd=ME["Get Damage"],ME["Deal Complex Damage"]
        local remote_weapon=SI["Weapon R"]
        local calls,hit_by,normal=0,nil,nil
        ME["Get Damage"]=function(self,imp,vel,loc,nrm,bone,raw,cut,inside,mesh,dism,lower,shock,hitter)
            calls=calls+1;hit_by=hitter;normal=nrm
        end
        ME["Deal Complex Damage"]=function()calls=calls+1 end
        SI["Weapon R"]=weapon
        if origin then
            origin.target_peer_id=9
            local result,status=api.apply_hit(hit_rec(origin),2)
            T.check(status==2 and calls==0 and T.contains(result,"origin acknowledged"),
                "suppressed origin owner acknowledgment invokes no native damage",result)
            origin.target_peer_id=2
            T.check(api.C3.apply_fx(origin,9)=="fx: native origin without damage",
                "suppressed origin never paints wounds on a stand-in")
        end
        if inside then
            inside.target_peer_id=9
            local result=api.apply_hit(hit_rec(inside),2)
            T.check(calls==1 and hit_by==coll and math.abs(normal.Y-.8)<1e-6 and math.abs(normal.Z-.6)<1e-6,
                "victim native Get Damage receives exact attacker module and original normal",result)
            SI["Weapon R"]=nil
            local _,status=api.apply_hit(hit_rec(inside),2)
            T.check(status==4 and calls==1,"missing continuation component drops before native damage without a substitute")
        end
        ME["Get Damage"],ME["Deal Complex Damage"],SI["Weapon R"]=own_gd,own_dcd,remote_weapon
        local prior_probe=api.C3.native_probe
        api.C3.native_probe=true;api.C3.protect(SI);api.C3.baseline(SI)
        local before_probe_health=SI.Health
        local before_probe_claims=#sent("damage")
        local before_probe_logs=#LOGS
        SI.Health=SI.Health-2 -- Native fixture ran before its POST Get Damage callback.
        api.on_get_damage(SI,zero,zero,zero,zero,FName("head"),1000,50,false,SI.Mesh,0,false,false,coll,false,nil,1,false,0,0)
        T.check(SI.Health==before_probe_health,"ordinary exact-id probe captures before backstop and retains damage restoration")
        SI["Last Complex Damage Impulse"],SI["Last Complex Damage Bone"]=41000,FName("head")
        api.on_complex(SI,SI.Mesh,coll,FName("head"),zero,zero,
            {X=1400,Y=0,Z=0},{X=1000,Y=0,Z=0},40,0,.85,0,false,false,1,nil,false,0)
        api.C3.constraint_begin_journal(constraint)
        SI.Health=SI.Health-3
        api.on_get_damage(SI,zero,zero,zero,zero,FName("head"),1000,1000,true,SI.Mesh,0,false,false,coll,false,nil,1,false,0,0)
        T.check(SI.Health==before_probe_health and #(api.CX.inside_queue or {})==1,
            "initial Inside measurement queues as plain data before restoration and cid assignment")
        api.flush_claims()
        local probe_records=sent("damage")
        local probe_parent,probe_child=probe_records[before_probe_claims+1],probe_records[before_probe_claims+2]
        local probe_lines={}
        for i=before_probe_logs+1,#LOGS do if T.contains(LOGS[i],"LAB_PROBE ")then probe_lines[#probe_lines+1]=LOGS[i]end end
        T.check(#probe_lines==2 and probe_parent and probe_child and probe_child.parent_cid==probe_parent.cid,
            "one native DCD and one Inside callback emit exactly two cid-bound probe lines")
        T.check(probe_parent and T.contains(probe_lines[1] or "",string.format("LAB_PROBE attacker=9 cid=%d parent_cid=0 bone=head dmg Health -2.000000",probe_parent.cid)),
            "ordinary successful send logs exact original attacker and assigned claim cid",T.repr(probe_lines))
        T.check(probe_child and T.contains(probe_lines[2] or "",string.format("LAB_PROBE attacker=9 cid=%d parent_cid=%d bone=head dmg Health -3.000000",probe_child.cid,probe_parent.cid)),
            "queued continuation logs its own cid and exact parent after successful send",T.repr(probe_lines))
        T.check(probe_child and probe_child.probe==nil and probe_child.probe_attacker==nil,
            "local probe strings never enter damage transport schema")
        api.C3.probe_last={name=SI.__name,at=math.floor(CLOCK*1000),bone="head",source=99999,text="wrong native sample",trace="wrong-source"}
        local pending_before=#api.CX.pending
        api.on_complex(SI,SI.Mesh,coll,FName("head"),zero,zero,
            {X=1400,Y=0,Z=0},{X=1000,Y=0,Z=0},40,0,.85,0,false,false,1,nil,false,0)
        local unpaired=api.CX.pending[pending_before+1]
        T.check(unpaired and T.contains(unpaired.native_evidence,"gd=unavailable")
            and not T.contains(unpaired.native_evidence,"wrong-source"),
            "a different native source cannot attach its Get Damage evidence to this DCD")
        api.CX.pending[pending_before+1]=nil
        local tiny_before,tiny_log=SI.Health,#LOGS
        for _=1,2 do
            SI.Health=SI.Health-.1
            api.on_get_damage(SI,zero,zero,zero,zero,FName("head"),1000,1000,true,SI.Mesh,0,false,false,coll,false,nil,1,false,0,0)
        end
        local tiny_samples=0
        for i=tiny_log+1,#LOGS do
            if T.contains(LOGS[i],"LAB_PROBE ") and T.contains(LOGS[i],"dmg Health -0.100000")then tiny_samples=tiny_samples+1 end
        end
        T.check(tiny_samples==2,"two sub-threshold Inside callbacks report each native change once without cumulative double counting")
        T.check(math.abs(SI.Health-(tiny_before-.2))<1e-6,"probe logging leaves native damage/backstop behavior unchanged")
        SI.Health=tiny_before;api.C3.baseline(SI)
        api.C3.base[SI:GetAddress()]=nil
        local unavailable_at=#LOGS
        api.on_get_damage(SI,zero,zero,zero,zero,FName("head"),1000,1000,true,SI.Mesh,0,false,false,coll,false,nil,1,false,0,0)
        local unavailable=false
        for i=unavailable_at+1,#LOGS do
            if T.contains(LOGS[i],"LAB_PROBE ") and T.contains(LOGS[i],"dmg Health unavailable [native baseline absent]")then unavailable=true end
        end
        T.check(unavailable,"missing native baseline logs unavailable instead of inventing zero damage")
        T.check(api.C3.probe_last==nil,"Inside samples cannot be reused by the next ordinary DCD")
        local replay_log=#LOGS
        api.C3.log_replay_probe({hit_id=701,parent_cid=700,bone="head"},2,
            {observed_fields=1,health_delta=-.00123456},"native Get Damage(ok) dmg Health +0.00 [Health-0.0 Pain+2.0]",true)
        T.check(#LOGS==replay_log+1 and T.contains(LOGS[#LOGS],"LAB_REPLAY attacker=2 cid=701 parent_cid=700 bone=head dmg Health -0.001235 [Health-0.0 Pain+2.0]"),
            "fresh owner replay logs original ids and native Health precision beyond legacy two decimals")
        api.C3.log_replay_probe({hit_id=701,parent_cid=700,bone="head"},2,
            {observed_fields=1,health_delta=-.00123456},"native replay cached",false)
        T.check(#LOGS==replay_log+1,"cached owner result cannot double-count a native LAB_REPLAY measurement")
        api.C3.log_replay_probe({hit_id=702,parent_cid=700,bone="head"},2,
            {observed_fields=8192,health_delta=0},"native Get Damage(ok) [Pain+2.0]",true)
        T.check(T.contains(LOGS[#LOGS],"LAB_REPLAY attacker=2 cid=702 parent_cid=700 bone=head dmg Health unavailable [native Health not observed]"),
            "owner Health absent from observed mask is unavailable instead of inferred zero")
        api.C3.native_probe=prior_probe;api.C3.protect(SI);api.C3.baseline(SI)
        SI["Last Complex Damage Impulse"],SI["Last Complex Damage Bone"]=original_gate,original_gate_bone
        SI.Mesh.GetAddress=old_mesh_address
        api.C3.stuck_parent={};api.CX.inside_queue={}
        ME["Weapon R"]=oldweapon
    end
    do
        local saved=RL.deep(api.own())
        local originalmode=RL.deep(NAT._rec.slots.mode)
        NAT.sc_put("mode",{match_id=12345,seq=104,mode=ES.game_mode.DUEL,round=3,
            rows={{peer_id=9,seat=0,life=1,alive=true},{peer_id=2,seat=1,life=1,alive=true}}})
        local oldflags={ME.Player,ME.DED,ME["Give Up"],ME["Give Up 2 (Temp)"],ME["GI Settings"],ME["HS Game Mode"]}
        local gi={IsValid=valid,["Current Game Mode enum"]=0}
        local gm={IsValid=valid,["Match Won"]=false}
        ME.Player,ME.DED,ME["GI Settings"],ME["HS Game Mode"]=true,false,gi,gm
        ME["Give Up"],ME["Give Up 2 (Temp)"]=false,false
        local reports=#sent("death_report")
        local hp=ME.Health
        T.check(not api.on_native_defeat(ME) and #sent("death_report")==reports,"recoverable KO without actual native lose flags never reports defeat")
        ME["Give Up"],ME["Give Up 2 (Temp)"]=true,true
        gi["Current Game Mode enum"]=5
        T.check(not api.on_native_defeat(ME),"native mode5 early-return cannot fabricate defeat from flags")
        gi["Current Game Mode enum"]=0
        T.check(not api.on_native_defeat(SI),"stand-in Event Lose Match never reports owner's defeat")
        T.check(not api.on_native_defeat(ME) and #sent("death_report")==reports,"automatic native KO cannot eliminate a Duel life")
        local originalenv=os.getenv
        local originalcontroller=ME.Controller
        ME.Controller={IsValid=valid,GetClass=function()return {GetFName=function()return FName("AI_BP_C")end}end}
        ME.Player=false;ME["Give Up 2 (Temp)"]=false
        T.check(not api.on_native_defeat(ME),"AI native yield cannot surrender a non-dev Duel")
        os.getenv=function(k)if k=="HSMP_DEV" then return "1" end return originalenv(k) end
        ME["Give Up"]=false
        T.check(not api.on_native_defeat(ME),"AI KO before native Give Up is not surrender")
        ME["Give Up"]=true
        T.check(api.on_native_defeat(ME),"own dev AI native yield surrenders Duel without player-only Give Up 2")
        local ai_report=sent("death_report")[#sent("death_report")]
        T.check(ai_report.reason==2 and ai_report.match_id==12345 and ai_report.life==1,"AI native yield is scoped surrender, not biological death")
        api.own().defeat_pending=nil;api.own().defeat_retry_at=nil
        os.getenv=originalenv;ME.Controller=originalcontroller;ME.Player=true;ME["Give Up 2 (Temp)"]=true
        NAT.sc_put("mode",{match_id=12345,seq=105,mode=ES.game_mode.BRAWL,round=3,
            rows={{peer_id=9,seat=0,life=1,alive=true},{peer_id=2,seat=1,life=1,alive=true}}})
        T.check(api.on_native_defeat(ME),"actual own native lose flags report verified defeat")
        local report=sent("death_report")[#sent("death_report")]
        T.check(report.reason==1 and report.match_id==12345 and report.round==3 and report.life==1,"defeat report retains original verified generation and distinct reason")
        T.check(api.own().defeat_pending~=nil,"successful IPC enqueue retains pending until authoritative elimination")
        local n=#sent("death_report")
        api.own_pawn_tick(ME)
        T.check(#sent("death_report")==n,"defeat retry does not flood within one second")
        CLOCK=CLOCK+1.01
        api.own_pawn_tick(ME)
        T.check(#sent("death_report")==n+1,"original one-shot native defeat retries when server initially ignored protected report")
        local deaths=0;local originaldeath=ME.Death
        ME.Death=function()deaths=deaths+1 end
        api.force_own_death(3,2,5,{match_id=12345,round=3,life=1})
        T.check(ME.Health==hp and deaths==0 and api.own().defeat_pending==nil,"authoritative defeat stops retry without HP0 or native Death")
        api.force_own_death(3,2,6,{match_id=12345,round=3,life=2})
        T.check(ME.Health==hp and deaths==0,"authoritative voluntary surrender preserves the living native pawn")
        CLOCK=CLOCK+1.01;api.own_pawn_tick(ME)
        T.check(#sent("death_report")==n+1,"confirmed native defeat cannot report twice")
        local sh=SI.Health;local sd=SI.Death;local dying=SI.Dying
        SI.Death=function()deaths=deaths+1 end;SI.Dying=SI.Death
        api.C3.remote_defeated[2]={match_id=12345,round=3,life=1}
        api.set_puppets({[SI:GetFName():ToString()]=2},{[2]=SI},{})
        api.update_standins()
        T.check(SI.Health==sh and deaths==0 and SI["Give Up"]==true,"remote defeated pawn keeps native-alive KO and never plays death")
        api.C3.remote_defeated[2]=nil;SI.Death,SI.Dying=sd,dying
        ME.Death=originaldeath
        ME.Player,ME.DED,ME["Give Up"],ME["Give Up 2 (Temp)"],ME["GI Settings"],ME["HS Game Mode"]=table.unpack(oldflags,1,6)
        for k in pairs(api.own())do api.own()[k]=nil end
        for k,v in pairs(saved)do api.own()[k]=v end
        NAT._rec.slots.mode=originalmode
    end
    local n0 = #sent("death_report")
    local original_health,original_ded=ME.Health,ME.DED
    ME.Health,ME.DED=100,false
    api.on_native_death(ME)
    T.check(#sent("death_report")==n0,"native Dying with living Health and DED false never reports biological death")
    ME.DED=true
    api.on_native_death(ME)
    local dr = sent("death_report")
    T.check(#dr == n0 + 1 and dr[#dr].round == 3 and dr[#dr].death_id == 0 and dr[#dr].match_id==12345 and dr[#dr].life==1,
        "native death reports original verified match/round/life (death_id is the sidecar's)", T.repr(dr))
    api.on_native_death(ME)
    T.check(#sent("death_report") == n0 + 1, "reported once per native life", #sent("death_report"))
    status.life=2;placed()
    NAT.sc_put("mode", {match_id=12345,seq=103,mode=ES.game_mode.DEATHMATCH,round=3,
        rows={{peer_id=9,seat=0,life=2,alive=true},{peer_id=2,seat=1,life=1,alive=true}}})
    api.on_native_death(ME)
    dr=sent("death_report")
    T.check(#dr==n0+2 and dr[#dr].life==2,
        "same pawn in a new verified life reports death again within the same round")
    api.on_native_death(ME)
    T.check(#sent("death_report")==n0+2,"second native life also deduplicates its own repeated death callback")
    ME.Health,ME.DED=original_health,original_ded
    SESS.match_id, SESS.rows = previous_mid, previous_rows; publish()
    NAT._rec.slots.mode=nil
    api.C3.life_for = arithmetic_life_for
    api.C3.displayed_for=arithmetic_displayed_for
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

do
    local Feet = dofile(T.path("mods/HSMPCombat/Scripts/replay_feet.lua"))
    local all, spawns, destroyed, present = {}, 0, 0, {}
    local oldfind, oldclass = FindAllOf, StaticFindObject
    _G.FindAllOf=function(class)if class=="Willie_BP_C" then return present end;return all end
    local cls={IsValid=valid}
    _G.StaticFindObject=function(path)
        T.check(path:find("Built_Weapons/Weapon_Feet.Weapon_Feet_C",1,true)~=nil,"kick source uses exact native class")
        return cls
    end
    local transform={Translation={X=1,Y=2,Z=3}}
    local pawn={IsValid=valid,GetAddress=function()return 550 end,
        Mesh={GetSocketTransform=function(_,bone,space)
            T.check(bone:ToString()=="foot_r" and space==0,"kick source uses exact right foot world socket")
            return transform
        end}}
    local gs={}
    function gs:BeginDeferredActorSpawnFromClass(context,c,t,collision,owner,scale)
        T.check(context==pawn and c==cls and t==transform and collision==1 and owner==pawn and scale==2,
            "kick source reproduces native deferred spawn arguments and owner")
        spawns=spawns+1
        local a={IsValid=valid,GetAddress=function()return 551 end,GetOwner=function()return pawn end,
            GetFName=function()return FName("ReplayFoot_"..spawns)end,
            SetActorHiddenInGame=function()end,K2_DestroyActor=function(self)self.IsValid=function()return false end;destroyed=destroyed+1 end}
        local cbox={IsValid=valid,GetAddress=function()return 552 end,
            GetClass=function()return {IsValid=valid,GetFName=function()return FName("BoxComponent")end}end}
        function cbox:SetCollisionEnabled(v)self.enabled=v end
        function cbox:GetCollisionEnabled()return self.enabled end
        a.Box=cbox
        a.BaseMesh={SetSimulatePhysics=function(self,v)self.sim=v end,SetCollisionEnabled=function(self,v)self.enabled=v end}
        function a:SetActorEnableCollision(v)self.coll=v end
        function a:GetActorEnableCollision()return self.coll end
        function a:K2_SetActorTransform(t)self.transform=t;return true end
        a["Collision Components Array"]={ForEach=function(_,fn)
            for i=1,10 do fn(i-1,{get=function()return i==10 and cbox or {IsValid=function()return false end}end})end
        end}
        all[#all+1]=a;return a
    end
    function gs:FinishSpawningActor(a,t,scale)
        T.check(a["Parent Actor"]==pawn and a["Last Parent"]==pawn and t==transform and scale==2,
            "native foot parent properties set before native construction")
    end
    local cache=Feet.new({UEHelpers={GetGameplayStatics=function()return gs end},log=function()end})
    local box=cache.component(pawn,false,10)
    T.check(box==all[1].Box and all[1]["Kick Power"]==10 and all[1]["Is Held"]==true,
        "missing transient kick replays actual natively constructed Box ordinal10")
    T.check(all[1].coll==false and all[1]["Temp Disable Damage"]==true and all[1].BaseMesh.sim==false and box.enabled==0,
        "replay foot cannot generate contacts or simulated movement")
    T.check(cache.component(pawn,false,10)==box and spawns==1,"repeated delayed kick retains bounded same-pawn native source")
    T.check(cache.component(pawn,false,1)==nil,"foot replay fails closed for unverified inherited collider ordinal")
    local poisoned_calls=0
    local poisoned={IsValid=function()poisoned_calls=poisoned_calls+1;error("freed cached wrapper")end,
        GetAddress=function()poisoned_calls=poisoned_calls+1;error("freed cached wrapper")end}
    present={{IsValid=valid,GetAddress=function()return 550 end}}
    local prune_ok=pcall(cache.prune,{poisoned})
    T.check(prune_ok and poisoned_calls==0 and destroyed==0,
        "kick prune ignores poisoned cached wrapper and retains fresh native pawn identity")
    present={}
    cache.clear("world changed");cache.clear("no valid world");cache.clear()
    T.check(cache.component(pawn,false,10)==box and spawns==1,"ambiguous world and session drops retain bounded foot source")
    cache.prune({});T.check(destroyed==1,"released avatar requests retirement of its retained kick source")
    cache.prune({})
    _G.FindAllOf=function()error("empty cache must not enumerate native actors")end
    T.check(pcall(cache.prune),"empty kick source cache performs no native actor lookup")
    _G.FindAllOf,_G.StaticFindObject=oldfind,oldclass
    local g={at={0,0,0},nrm={0,0,1},vel={1,0,0},imp={0,0,0}}
    T.check(api.C3.dcd_args({dism_blunt=67108864},ME.Mesh,g,nil)[13]==true,
        "owner replay preserves original native DamageParent true")
    T.check(api.C3.dcd_args({dism_blunt=0},ME.Mesh,g,nil)[13]==false,
        "ordinary native weapon replay retains DamageParent false")
end

-- The actual main-loop closure owns this cache. No production test export is
-- necessary to prove its WG drop handler preserves numeric native identities.
do
    local attempts
    for i=1,100 do
        local name,value=debug.getupvalue(api.on_tick,i)
        if not name then break end
        if name=="ReplayAttempts" then attempts=value;break end
    end
    T.check(attempts~=nil,"actual main tick captures native attempt cache")
    if attempts then
        CLOCK=700;api.set_world("native-once-world")
        match("live",3);api.refresh_match();sidecar("connected",1);api.refresh_session()
        api.set_my_peer_id(1)
        api.C3.life_for=arithmetic_life_for;api.C3.displayed_for=arithmetic_displayed_for
        api.set_puppets({[SI.__name]=2},{[2]=SI})
        api.session().server_dead[1]=nil
        ME.Health=100;ME.Consciousness=100;ME["Force Death"]=false
        local calls=0;local previous=ME["Deal Complex Damage"]
        ME["Deal Complex Damage"]=function(self)calls=calls+1;self.Health=self.Health-5 end
        local d=hit_rec{hit_id=1000001,round=3,target_peer_id=1,bone="pelvis",flags=32}
        local text,outcome,fresh=attempts.run(d,2,function()return api.apply_hit(d,2)end)
        T.check(fresh and calls==1 and outcome.status==1 and ME.Health==95,
            "actual native replay records one applied hit before lost outcome acknowledgment",T.repr({text,outcome,ME.Health}))
        api.wg_drop("world changed");api.set_world("native-once-world")
        api.set_puppets({[SI.__name]=2},{[2]=SI})
        local cached,result,repeated=attempts.run(d,2,function()return api.apply_hit(d,2)end)
        T.check(not repeated and calls==1 and ME.Health==95 and result.health_delta==-5,
            "transient WG lookup drop plus same-life redelivery never executes native damage twice",T.repr({cached,calls,ME.Health}))
        api.wg_drop("level change requested");api.set_world("native-once-world")
        local _,same,again=attempts.run(d,2,function()return api.apply_hit(d,2)end)
        T.check(not again and calls==1 and same.status==1,
            "object-cache reset also retains immutable recorded native attempt")
        ME["Deal Complex Damage"]=previous
    end
end
