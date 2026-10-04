-- HSMPLoadout kit.lua: the Director's kit evidence (bus key kit_status, a typed
-- record, docs/development/subsystems/director.md) and the kit_verified event.
--
--   hsmp-tools lua-test kit_status
--
-- kit.lua runs against a mock api: a pawn whose hands and "equipped" armour
-- follow what the game's equip calls did, a clock, in-memory files, and a fake
-- HSMP_IPC whose typed slots (peer_kit, kit_rules, bus kit_status) hold tables
-- marshalled by the native module's rules (lib/hsmp_native_records.lua).
--
-- Covered: status written on every verify (dressing -> ok, verifying ->
-- ok, given up), the pawn FName / rev / armour_n / exp_armour_n / hand
-- classes / tries / error fields, the native re-arm (ok=false then
-- re-dressed), the stable flag at the end of the window, no kit (ok, nothing
-- to dress), the kit_verified event on verdicts only, no rewrite storm, and
-- the "no re-dress when the round goes Live" regression.

local SD = "S"
local R = require("hsmp_native_records")
local S = dofile(T.path("mods/shared/hsmp_ipc_schema.lua"))
local KIT = T.path("mods/HSMPLoadout/Scripts/kit.lua")

local function new_env()
    local w = { clock = 10.0, files = {}, logs = {}, evs = {}, writes = 0, applies = 0, equip_ok = true, rearm_once = false }
    local Kit = dofile(KIT)
    local Cat = dofile(T.path("mods/HSMPLoadout/Scripts/hsmp_catalog.lua"))
    w.Cat = Cat
    local function class_obj(path)
        return { path = path, GetCDO = function() return { ["Armor Slot"] = 3 } end,
                 GetFName = function() return { ToString = function() return path:match("([^/]+)$") .. "_C" end } end }
    end
    local function weapon(path)
        local cls = class_obj(path)
        return { cls = cls, bHidden = w.spawn_hidden == true, GetClass = function(self) return self.cls end,
                 props = { ["Is Held"] = true },
                 IsValid = function() return true end,
                 K2_DestroyActor = function(self) self.destroyed = true; w.destroyed = (w.destroyed or 0) + 1 end,
                 GetFName = function() return { ToString = function() return "W_" .. path:match("([^/]+)$") end } end }
    end
    w.weapon = weapon
    function w:new_pawn(n)
        self.pawn = { n = n or 1, props = {}, equipped = {}, worn = 0,
            GetFName = function(p) return { ToString = function() return "Willie_BP_C_" .. p.n end } end,
            GetAddress = function(p) return 1000 + p.n end }
        return self.pawn
    end
    local api = {
        Log = function(fmt, ...) w.logs[#w.logs + 1] = string.format(fmt, ...) end,
        STATE_DIR = SD,
        ev = function(n, f) w.evs[#w.evs + 1] = { n = n, f = f } end,
        now = function() return w.clock end,
        valid = function(o) return o ~= nil and o ~= false end,
        field = function(o, k) return o and o.props[k] end,
        resolve_class = function(path) return class_obj(path) end,
        expand_path = function(p) return "/Game/" .. p end,
        class_path = function(cls) return cls and cls.path or "" end,
        -- "Set Up Right/Left Hand Weapon"(Class, Actor, ...): the actor form
        -- puts that very actor in hand (the re-arm re-equips the dropped one).
        bp_call = function(p, fname, cls, actor)
            if not w.equip_ok then return end
            local side = fname:find("Right") and "Weapon R" or "Weapon L"
            if actor and w.actor_equip_ok == false then return end
            if actor then actor.props["Is Held"] = true end
            p.props[side] = actor or weapon(cls.path)
        end,
        spawn_weapon_actor = function() return nil end,
        apply_armour = function(p, L)
            w.applies = w.applies + 1
            p.equipped = {}
            for _, pc in ipairs(L.p) do p.equipped[#p.equipped + 1] = { pc[1], pc[2] } end
            p.worn = #L.p
            return true, #L.p, "SetUpArmor=ok"
        end,
        read_source = function(p, name) return name == "equipped" and p.equipped or {} end,
        worn_count = function(p) return p.worn end,
        in_arena = function() return true end,
        local_pawn = function() return w.pawn end,
        busy = function() return false end,
        reveal = function() return 300 end,
        -- same contract as HSMPLoadout main.lua
        actor_visible = function(x) if not x then return nil end return x.bHidden ~= true end,
        unhide_carried = function(p)
            local n = 0
            for _, f in ipairs({ "Weapon R", "Weapon L" }) do
                local x = p.props[f]
                if x and x.bHidden then x.bHidden = false; n = n + 1; w.unhid = (w.unhid or 0) + 1 end
            end
            return n
        end,
    }
    -- Typed session / link view (hsmp_session) and the game-local bus, in memory.
    w.peer_id, w.view, w.bus = 1, { state = "countdown", round = 0 }, {}
    api.hsmp_session = { my_peer_id = function() return w.peer_id end, view = function() return w.view end }
    api.bus_table = function(k) return w.bus[k] end
    -- The typed slots this suite plays the sidecar for.
    w.peer_kit, w.bus, w.kit_rules = {}, {}, nil
    _G.HSMP_IPC = {
        peer_rec = function(slot, pid) if slot == "peer_kit" then return w.peer_kit[pid] end end,
        rec = function(slot) if slot == "kit_rules" then return w.kit_rules end end,
        bus_put = function(key, t)
            local m = assert(R.marshal(S, key, t))
            w.bus[key] = m
            if key == "kit_status" then w.writes = w.writes + 1 end
            return true
        end,
        bus_table = function(key) return w.bus[key] end,
    }
    Kit.init(api)
    w.Kit, w.api = Kit, api
    -- The sidecar publishes our validated kit (a kit_verdict record) into peer_kit[1].
    function w:kit(class, rev)
        local k = Cat.class_by_id[class]
        local rows = {}
        for i, a in ipairs(k and k.armor or {}) do rows[i] = { id = a } end
        self.peer_kit[1] = assert(R.marshal(S, "kit_verdict", { rev = rev or 7, seq = 1, class = class,
            r = k and k.r or "", l = k and k.l or "", rows = rows }))
    end
    function w:rules(mode, rev) self.kit_rules = assert(R.marshal(S, "kit_rules", { mode = mode, budget = 0, rev = rev })) end
    function w:match(state, round)
        self.view = { state = state, round = round }
    end
    -- HSMPSync's spawn_status bus record (protect_until nil = unbounded until Live).
    function w:spawn_status(pawn, verified, protect_until)
        self.bus.spawn_status = { seq = 1, pawn = pawn, verified = verified, has_protect_until = protect_until ~= nil,
                                  protect_until = protect_until or 0 }
    end
    function w:tick(n, dt)
        for _ = 1, (n or 1) do
            self.clock = self.clock + (dt or 0.1)
            self.Kit.tick_local()
        end
    end
    -- The kit_status record, with the old reading conventions (rev as text, "" = nil,
    -- hand_vis 1 / 2 / 0 = true / false / nil) so the assertions read as before.
    function w:status()
        local s = self.bus.kit_status
        if not s then return nil end
        local v = {}
        for k, x in pairs(s) do v[k] = x end
        v.rev = tostring(s.rev)
        for _, k in ipairs({ "error", "r_class", "l_class" }) do if v[k] == "" then v[k] = nil end end
        local vis = { [1] = true, [2] = false }
        v.r_visible, v.l_visible = vis[s.r_visible], vis[s.l_visible]
        return v
    end
    function w:evs_named(n) return T.filter(self.evs, function(e) return e.n == n end) end
    function w:logtext() return table.concat(self.logs, "\n") end
    w:match("countdown", 0)
    return w
end

T.log("== status_record (the kit_status record)")
do
    local w = new_env()
    local rec = w.Kit.status_record({ pawn = 'W"1', rev = 4, ok = true, armour_n = 5, exp_armour_n = 5, r_class = "Sword_C",
        l_class = nil, r_visible = true, l_visible = nil, tries = 1, error = nil, kit = "duelist", stable = false, round = 1, t = 12.5 })
    local s, err = R.marshal(S, "kit_status", rec)
    T.check(s and s.pawn == 'W"1' and s.rev == 4 and s.ok == true and s.armour_n == 5 and s.r_class == "Sword_C"
        and s.l_class == "" and s.tries == 1 and s.error == "" and s.kit == "duelist" and s.stable == false and s.t == 12.5
        and s.r_visible == S.ENUMS.hand_vis.SHOWN and s.l_visible == S.ENUMS.hand_vis.UNKNOWN,
        "a valid kit_status record with every contract field", T.repr(s) .. tostring(err))
    local h = w.Kit.status_record({ r_visible = false })
    T.check(h.r_visible == S.ENUMS.hand_vis.HIDDEN and h.pawn == "" and h.kit == "none", "hidden hand / no pawn / no kit", T.repr(h))
end

T.log("== dress -> verify -> ok (duelist: 5 pieces, arming sword + buckler)")
do
    local w = new_env()
    w:new_pawn(1)
    w:kit("duelist", 7)
    w:tick(1)
    local s = w:status()
    T.check(s and s.pawn == "Willie_BP_C_1" and s.ok == false and s.error == "dressing" and s.rev == "7",
        "status written as soon as dressing starts (ok=false, error=dressing)", T.repr(s))
    w:tick(5)
    s = w:status()
    T.check(s.ok == true and s.error == nil and s.armour_n == 5 and s.exp_armour_n == 5 and s.tries == 1 and s.stable == false,
        "verified: ok=true, armour_n == exp_armour_n, tries=1", T.repr(s))
    T.check(T.contains(s.r_class, "Arming") and T.contains(s.l_class, "Buckler"), "both hand classes reported", T.repr(s))
    T.check(s.r_visible == true and s.l_visible == true, "r_visible / l_visible reported", T.repr(s))
    local kv = w:evs_named("kit_verified")
    T.check(#kv == 1 and kv[1].f.who == "self" and kv[1].f.ok == true and kv[1].f.armour_n == 5
        and T.contains(kv[1].f.r_class, "Arming"), "kit_verified{who=self, ok, armour_n, r_class, l_class} once", T.repr(kv))
    local n = w.writes
    w:tick(10)
    T.check(w.writes == n, "no rewrite while nothing changes (not every 100 ms)")
    w:tick(25)
    s = w:status()
    T.check(s.ok == true and s.stable == true, "stable=true once the 3 s window held", T.repr(s))
    T.check(#w:evs_named("kit_verified") == 1, "the stable rewrite is not a second verdict event")
end

T.log("== the native re-arm undoes the right hand -> ok=false, re-dressed, ok again")
do
    local w = new_env()
    w:new_pawn(1)
    w:kit("man_at_arms", 3)
    w:tick(6)
    T.check(w:status().ok == true, "verified first")
    w.pawn.props["Weapon R"] = nil                    -- the game re-armed from the passport: fists
    w:tick(4)
    local s = w:status()
    T.check(s.ok == false and T.contains(s.error or "", "re-dressing: native re-arm undid R hand"),
        "the undo is visible to the Director at once", T.repr(s))
    w:tick(6)
    s = w:status()
    T.check(s.ok == true and T.contains(s.r_class or "", "Pole"), "re-dressed and ok again", T.repr(s))
end

T.log("== a weapon in hand but HIDDEN is not a verified kit: unhidden, then ok + visible")
do
    local w = new_env()
    w.spawn_hidden = true                             -- equipped while the pawn was hidden for dressing
    w:new_pawn(1)
    w:kit("duelist", 5)
    w:tick(5)
    local s = w:status()
    T.check(s.ok == false and T.contains(s.error or "", "R hand HIDDEN") and T.contains(s.error or "", "L hand HIDDEN"),
        "verify fails on hidden weapons (existence is not enough)", T.repr(s))
    T.check((w.unhid or 0) == 2 and T.contains(w:logtext(), "was hidden; unhid 2"), "both carried weapons unhidden", w.unhid)
    w.spawn_hidden = false
    w:tick(6)
    s = w:status()
    T.check(s.ok == true and s.r_visible == true and s.l_visible == true, "next verify: ok with both hands visible", T.repr(s))
    -- hidden again later (pooled actor hidden by someone else) inside the window
    w.pawn.props["Weapon R"].bHidden = true
    w:tick(4)
    T.check(w.pawn.props["Weapon R"].bHidden == false, "the stability window unhides it again")
end

T.log("== equip never sticks -> given up with an error")
do
    local w = new_env()
    w.equip_ok = false
    w:new_pawn(1)
    w:kit("duelist", 1)
    w:tick(60)
    local s = w:status()
    T.check(s.ok == false and T.contains(s.error, "kit incomplete after 6 tries") and s.tries == 6,
        "status: ok=false, error names the missing items, tries", T.repr(s))
    local kv = w:evs_named("kit_verified")
    T.check(#kv == 1 and kv[1].f.ok == false and kv[1].f.error, "kit_verified{ok=false, error} on giving up", T.repr(kv))
    T.check(T.contains(w:logtext(), "still missing after 6 tries"), "and the log line")
end

T.log("== verifying (a retry) is reported, not only the verdicts")
do
    local w = new_env()
    w.equip_ok = false
    w:new_pawn(1)
    w:kit("duelist", 1)
    w:tick(5)
    local s = w:status()
    T.check(s.ok == false and T.contains(s.error, "verifying: missing"), "a failed verify writes the status", T.repr(s))
end

T.log("== no kit selected (the server's kit is class none): ok, nothing to dress")
do
    local w = new_env()
    w:new_pawn(1)
    w.peer_kit[1] = assert(R.marshal(S, "kit_verdict", { rev = 3, seq = 1, class = "none" }))
    w:tick(2)
    local s = w:status()
    T.check(s and s.ok == true and s.kit == "none" and s.pawn == "Willie_BP_C_1" and s.stable == true,
        "class none -> ok=true (the Director's kit step does not wait)", T.repr(s))
end

T.log("== L13: connected but no kit yet: NOT done")
do
    local w = new_env()
    w:new_pawn(1)
    w:tick(2)
    local s = w:status()
    T.check(s and s.ok == false and s.error == "no kit yet" and s.stable == false,
        "no peer_kit record for me yet -> ok=false, error 'no kit yet' (was ok=true)", T.repr(s))
    w:kit("duelist", 2)
    w:tick(3)
    T.check(w.applies >= 1, "the kit is dressed once it arrives", w.applies)
end

T.log("== H6: no live MP session -> a leftover kit never dresses the pawn")
do
    local w = new_env()
    local live = false
    w.api.mp_live = function() return live end
    w:new_pawn(1)
    w:kit("duelist", 2)                       -- leftover peer_kit record + a link record
    w:tick(30)
    T.check(w.applies == 0 and w.bus.kit_status == nil and #w:evs_named("kit_verified") == 0,
        "single player: no dress, no status, no event", T.repr({ w.applies, w.bus.kit_status }))
    live = true
    w:tick(30)
    T.check(w.applies >= 1, "a live session dresses as before", w.applies)
end

T.log("== a missing link record / no peer id keeps the last peer id")
do
    local w = new_env()
    T.check(w.Kit.my_peer_id() == 1, "peer id 1")
    w.peer_id = 0
    T.check(w.Kit.my_peer_id() == 1, "no link record (sidecar being replaced): still 1")
    T.check(w.Kit.my_peer_id() == 1, "a link without a peer id yet: still 1")
    w.peer_id = 3
    T.check(w.Kit.my_peer_id() == 3, "a new id (reconnect) is taken")
end

T.log("== an error status in between is replaced by the real state (gate 20261002-082601 kit_error)")
do
    local w = new_env()
    w:new_pawn(1)
    w:kit("duelist", 7)
    w:tick(40)
    T.check(w:status().ok == true, "dressed and verified first")
    -- one tick without our kit (e.g. the peer slot freed and reassigned)
    local saved = w.peer_kit[1]
    w.peer_kit[1] = nil
    w:tick(1)
    T.check(w:status().ok == false and w:status().error == "no kit yet", "the error is reported", T.repr(w:status()))
    w.peer_kit[1] = saved
    local applies = w.applies
    w:tick(1)
    local s = w:status()
    T.check(s.ok == true and s.error == nil, "next healthy tick: ok again without waiting for a change", T.repr(s))
    T.check(w.applies == applies, "and without re-dressing the pawn", { applies, w.applies })
end

T.log("== a new pawn gets a fresh status; the round going Live does NOT re-dress")
do
    local w = new_env()
    w:new_pawn(1)
    w:kit("duelist", 2)
    w:tick(40)
    local applies = w.applies
    w:match("live", 1)                                -- countdown round 0 -> live round 1
    w:tick(30)
    T.check(w.applies == applies, "no re-dress when the session's round changes at Live", w.applies - applies)
    w:new_pawn(2)
    w:tick(6)
    local s = w:status()
    T.check(s.pawn == "Willie_BP_C_2" and s.ok == true and w.applies == applies + 1, "a new pawn is dressed and reported", T.repr(s))
    w:kit("duelist", 3)                               -- kit change mid-round (live): deferred
    w:tick(5)
    T.check(w.applies == applies + 1 and T.contains(w:logtext(), "arrived during a live round"), "kit change during Live waits")
    w.Kit.on_world_change()
    w:new_pawn(3)
    w:match("countdown", 1)
    w:tick(6)
    T.check(w:status().pawn == "Willie_BP_C_3" and w:status().rev == "3" and w:status().ok, "next world: the new kit", T.repr(w:status()))
end

-- ---------------------------------------------------------------------------
-- "Still no weapons": a knockdown makes Half Sword drop the held
-- weapon; "Weapon R/L" then holds Weapon_Fists_C. Fists are never a kit weapon.

local FISTS = "/Game/Character/Blueprints/Weapon_Fists"
local function drop(w, side)
    local hand = side == "L" and "Weapon L" or "Weapon R"
    local old = w.pawn.props[hand]
    if old then old.props["Is Held"] = false end
    w.pawn.props[hand] = w.weapon(FISTS)
    return old
end
local function verified_env(class)
    local w = new_env()
    w:new_pawn(1)
    w:kit(class or "duelist", 7)
    w:tick(40)                                        -- dressed, verified, stable window over
    return w
end

T.log("== fists are never a held kit weapon (status, verdict, stable write)")
do
    local w = new_env()
    w:new_pawn(1)
    w:kit("duelist", 7)
    w:tick(6)
    T.check(w:status().ok == true, "verified with the kit weapons")
    w:tick(26)                                        -- inside the 3 s stability window ...
    drop(w, "R")                                      -- ... a drop right at its end
    w:tick(1)
    local s = w:status()
    T.check(s.ok == false and s.r_class == "Weapon_Fists_C", "never ok=true with fists in a kit hand; r_class is the real actor class",
        T.repr(s))
    T.check(w.Kit.is_bare("Weapon_Fists_C") and w.Kit.is_bare("/Game/X/Weapon_Feet") and w.Kit.is_bare(nil)
        and not w.Kit.is_bare("ModularWeaponBP_ArmingSword_C"), "is_bare")
    drop(w, "L")
    local ok, bad = w.Kit.hands_check(w.pawn, { r = "x_unknown", l = nil })
    T.check(ok == true and #bad == 0, "a kit without weapons accepts bare hands (fists)", T.repr(bad))
    ok, bad = w.Kit.hands_check(w.pawn, { r = "duelist_sword_not_in_catalogue", l = w.Cat.class_by_id.duelist.l })
    T.check(ok == false and bad[1] == "L (Weapon_Fists)", "a kit hand with fists is reported by side and class", T.repr(bad))
end

T.log("== dropped during the countdown -> re-equipped at once, status ok again")
do
    local w = verified_env()
    local applies = w.applies
    local old = drop(w, "R")
    w:tick(12)
    local s = w:status()
    T.check(T.contains(s.r_class or "", "Arming") and s.ok == true, "kit weapon back in hand, ok=true", T.repr(s))
    T.check(T.contains(w:logtext(), "kit weapon dropped before the round went Live (R (Weapon_Fists); window: countdown)"),
        "the drop and the window are logged", w:logtext())
    T.check(old.destroyed ~= true and w.pawn.props["Weapon R"] == old and old.props["Is Held"] == true,
        "the dropped sword itself is back in hand, never destroyed (HSMPWorld may have registered it)")
    T.check(T.contains(w:logtext(), "R=ok(same actor)"), "the log says the same actor was re-equipped", w:logtext())
    T.check(w.applies == applies, "weapons only: armour is not re-dressed")
    local ps = w:evs_named("pawn_state")
    T.check(#ps == 1 and ps[1].f.at == "rearm" and ps[1].f.weapon_r == "Weapon_Fists_C", "pawn_state{at=rearm}", T.repr(ps))
    T.check(T.contains(w:logtext(), "kit weapons in hand again"), "recovery logged")
end

T.log("== dropped during Live but inside the spawn protection -> re-equipped")
do
    local w = verified_env()
    w:match("live", 1)
    w:spawn_status("Willie_BP_C_1", true, w.clock + 2.0)
    drop(w, "L")
    w:tick(12)
    T.check(T.contains(w:status().l_class or "", "Buckler") and w:status().ok == true, "re-armed (window: spawn protection)",
        T.repr(w:status()))
    T.check(T.contains(w:logtext(), "window: spawn protection"), "window named in the log")
    w:spawn_status("Willie_BP_C_1", false, nil)
    drop(w, "L")
    w:tick(12)
    T.check(w:status().ok == true, "protect_until null (held until Live / placing) also re-arms")
    w:spawn_status("Willie_BP_C_9", false, nil)
    drop(w, "L")
    w:tick(12)
    T.check(w:status().ok == false, "another pawn's protection does not count")
end

T.log("== dropped during Live (no protection): no auto re-arm, ok=false with the real classes, logged once, event")
do
    local w = verified_env()
    w:match("live", 1)
    w:spawn_status("Willie_BP_C_1", true, w.clock - 5)
    drop(w, "R")
    w.pawn.props.Consciousness, w.pawn.props.Downed = 74, true
    w:tick(20)
    local s = w:status()
    T.check(s.ok == false and s.r_class == "Weapon_Fists_C" and T.contains(s.error or "", "dropped during Live: R (Weapon_Fists)"),
        "status ok=false, r_class=Weapon_Fists_C, error says why", T.repr(s))
    T.check(T.contains(w.pawn.props["Weapon R"].cls.path, "Weapon_Fists"), "nothing put back in hand (a drop is gameplay)")
    T.check(T.count(w:logtext(), "dropped during the live round") == 1, "logged once, not every tick")
    local ps = w:evs_named("pawn_state")
    T.check(#ps == 1 and ps[1].f.at == "weapon_drop" and ps[1].f.consciousness == 74 and ps[1].f.downed == true
        and ps[1].f.weapon_r == "Weapon_Fists_C" and T.contains(ps[1].f.weapon_l, "Buckler"),
        "pawn_state{at=weapon_drop, consciousness, downed, weapon_r, weapon_l}", T.repr(ps))
    -- picked a kit sword back up by hand: ok again
    w.pawn.props["Weapon R"] = w.weapon(w.Cat.items[w.Cat.class_by_id.duelist.r].path)
    w:tick(12)
    T.check(w:status().ok == true, "a kit weapon back in hand reads ok again", T.repr(w:status()))
end

T.log("== re-arm is bounded")
do
    local w = verified_env()
    w.equip_ok = false
    drop(w, "R")
    w:tick(80)
    local s = w:status()
    T.check(s.ok == false and T.contains(s.error or "", "re-arm gave up"), "gives up after MAX_REARMS", T.repr(s))
    T.check(T.count(w:logtext(), "kit weapon dropped before the round went Live") == w.Kit.MAX_REARMS, "exactly MAX_REARMS tries")
end

T.log("== a re-arm never destroys the dropped kit weapon")
do
    -- re-equipping the dropped actor does not take: a new kit weapon, the dropped one stays a world item
    local w = verified_env()
    w.actor_equip_ok = false
    local old = drop(w, "R")
    w:tick(12)
    local s = w:status()
    T.check(s.ok == true and T.contains(s.r_class or "", "Arming") and w.pawn.props["Weapon R"] ~= old,
        "fallback: a new kit weapon in hand", T.repr(s))
    T.check(old.destroyed ~= true and (w.destroyed or 0) == 0, "the dropped actor is left alone (a real, replicated item)")
    T.check(T.contains(w:logtext(), "dropped actor left as a world item"), "and the log says so")
    -- a second drop re-equips the NEW weapon (own.held follows the hand)
    w.actor_equip_ok = nil
    local new = drop(w, "R")
    w:tick(12)
    T.check(w.pawn.props["Weapon R"] == new and new.destroyed ~= true, "the next drop re-equips the weapon now held")

    -- somebody else picked the dropped sword up: a new one for us, theirs untouched
    local w2 = verified_env()
    local old2 = drop(w2, "R")
    old2.props["Is Held"] = true
    w2:tick(12)
    T.check(w2:status().ok == true and w2.pawn.props["Weapon R"] ~= old2 and old2.destroyed ~= true,
        "held by someone else: a new kit weapon, never theirs destroyed", T.repr(w2:status()))
end

T.log("== the first status of a new world is always written (pooled FName, same content)")
do
    local w = verified_env()
    local kit = w.Kit
    local before = w:status()
    local n = w.writes
    kit._write_status(w.pawn, nil, true, 1, nil, true, false)
    local n1 = w.writes
    kit._write_status(w.pawn, nil, true, 1, nil, true, false)
    T.check(w.writes == n1, "same world, same content: no rewrite (dedup)", w.writes - n1)
    kit.on_world_change()
    w.clock = w.clock + 5
    kit._write_status(w.pawn, nil, true, 1, nil, true, false)
    T.check(w.writes == n1 + 1 and w:status().t > (before.t or 0), "after a world change the same content is written again with a new t",
        T.repr({ n = n, n1 = n1, writes = w.writes }))
end

T.log("== rules missing from the slot keep the last good rules; mode + rev are in the stand-in key")
do
    local w = new_env()
    w:kit("duelist", 4)
    w:rules(1, 3)
    local r = w.Kit.rules()
    T.check(r.mode == 1 and r.rev == 3, "rules read", T.repr(r))
    local appearance = { v = 9, p = { { 3, "@Armor/X/BP_Torso" } }, a = {}, w = {} }
    local _, s1 = w.Kit.effective_remote(1, appearance)
    w.kit_rules = nil                                  -- slot not written (a new session, rules not in yet)
    T.check(w.Kit.rules().mode == 1, "missing: the last good rules (not FREE)", T.repr(w.Kit.rules()))
    w:rules(0, 0)                                      -- a zero record (rev 0) is no rules either
    T.check(w.Kit.rules().mode == 1, "rev 0: the last good rules")
    w:rules(1, 3)
    local _, s2 = w.Kit.effective_remote(1, appearance)
    T.check(s1 == s2, "unchanged rules: same stand-in key", T.repr({ s1, s2 }))
    w:rules(0, 4)                                      -- host switches to FREE
    local _, s3 = w.Kit.effective_remote(1, appearance)
    T.check(s3 ~= s2 and s3:find("m0") and s3:find("r4"), "a host rule change changes the stand-in key (re-dress)", T.repr({ s2, s3 }))
    w.Kit.forget_rules(); w.kit_rules = nil
    T.check(w.Kit.rules().mode == 0, "after forget_rules (menu cleanup) a missing slot is FREE again")
end

T.log("== a held-item change changes only the weapon key, never the armour key")
do
    local w = new_env()
    local K = w.Kit
    local L1 = { v = 5, p = { { 3, "@Armor/X/BP_Torso" }, { 7, "@Armor/X/BP_Helm" } },
                 a = { { 3, { class = "@Armor/X/BP_Torso", bg_color = { 1, 0, 0, 1 }, price = 0.5 } } },
                 w = { R = { class = "@Weapons/Sword" }, L = nil } }
    local L2 = { v = 6, p = L1.p, a = L1.a, w = { R = nil, L = nil } }            -- dropped the sword: new version
    local L3 = { v = 7, p = L1.p, a = L1.a, w = { R = { class = "@Weapons/Axe" } } }   -- picked up an axe
    T.check(K.armour_sig(L1) == K.armour_sig(L2) and K.armour_sig(L2) == K.armour_sig(L3),
        "same armour, new loadout versions: the armour key does not change")
    T.check(K.weapon_sig(L1, "") ~= K.weapon_sig(L2, "") and K.weapon_sig(L2, "") ~= K.weapon_sig(L3, ""),
        "the weapon key follows the hands")
    T.check(K.weapon_sig(L1, "") ~= K.weapon_sig(L1, "|h7,-"), "a world item in hand changes the weapon key only")
    T.check(K.weapon_sig(L1, "", { R = true }) ~= K.weapon_sig(L1, ""), "a stripped hand is part of the weapon key")
    local L4 = { v = 8, p = { { 3, "@Armor/X/BP_Torso" } }, a = L1.a, w = L3.w }
    T.check(K.armour_sig(L4) ~= K.armour_sig(L3), "an armour change changes the armour key")
    local L5 = { v = 8, p = L1.p, a = { { 3, { class = "@Armor/X/BP_Torso", bg_color = { 0, 1, 0, 1 }, price = 0.5 } } }, w = L3.w }
    T.check(K.armour_sig(L5) ~= K.armour_sig(L3), "a passport (colour) change changes the armour key")
end

T.log("== dynamic world items (dyn = the peer whose weapon left the hand)")
do
    local w = new_env()
    local raw = '{"level":3,"epoch":2,"e":[[11,123,1.0,2.0,3.0,0,"/Game/W/Rack.Rack_C"],'
        .. '[2147549185,77,-10.5,20.0,5.0,2,"/Game/Weapons/ModularWeaponBP_Sword.ModularWeaponBP_Sword_C"]]}'
    local d = w.Kit.parse_dyn(raw)
    T.check(#d == 1 and d[1].peer == 2 and d[1].id == 2147549185 and d[1].leaf == "ModularWeaponBP_Sword_C",
        "only dyn entries, with peer and class leaf", T.repr(d))
    T.check(#w.Kit.parse_dyn(nil) == 0 and #w.Kit.parse_dyn("") == 0, "no file: nothing")
end

T.log("== the menu cleans leftover kit files only when the old session is provably over")
do
    local HS = dofile(T.path("mods/shared/hsmp_session.lua"))
    local SCH = dofile(T.path("mods/shared/hsmp_ipc_schema.lua"))
    local w = new_env()
    local clk = 50.0
    local link, ver, age = { status = SCH.ENUMS.sidecar_status.CONNECTED, my_peer_id = 1 }, 2, 0.01
    local fake = { S = SCH,
        rec = function(slot) if slot == "link" and link then return link, ver end return nil end,
        refresh_info = function() return { sidecar_state = "ready", sidecar_hb_age_s = age } end }
    local S = HS.new({ ipc = fake, clock = function() return clk end, every_s = 0 })
    S:poll(true)
    T.check(not w.Kit.menu_cleanup_ok(S, clk), "first look at a connected, beating sidecar: never clean (a Welcome may be in flight)")
    for _ = 1, 4 do clk = clk + 1; S:poll(true) end
    T.check(not w.Kit.menu_cleanup_ok(S, clk), "a live (beating) session: never clean")
    age = 1e9
    clk = clk + 1; S:poll(true)
    T.check(not w.Kit.menu_cleanup_ok(S, clk), "the heartbeat just stopped: not yet")
    clk = clk + 16; S:poll(true)
    T.check(w.Kit.menu_cleanup_ok(S, clk), "no heartbeat for >= 15 s: the old session is dead -> clean")
    age = 0.01
    link, ver = { status = SCH.ENUMS.sidecar_status.KICKED, my_peer_id = 1 }, 4
    clk = clk + 1; S:poll(true)
    T.check(w.Kit.menu_cleanup_ok(S, clk), "terminal status -> clean")
    -- no link record at all: only after >= 2 s
    link = nil
    local S2 = HS.new({ ipc = fake, clock = function() return clk end, every_s = 0 })
    S2:poll(true)
    T.check(not w.Kit.menu_cleanup_ok(S2, clk), "absent at the first look: not yet")
    clk = clk + 1; S2:poll(true)
    T.check(not w.Kit.menu_cleanup_ok(S2, clk), "absent for 1 s: not yet")
    clk = clk + 1.1; S2:poll(true)
    T.check(w.Kit.menu_cleanup_ok(S2, clk), "absent for >= 2 s -> clean")
end

T.log("== IO-1: a pawn already visible (safety reveal) is dressed only after the hair settles")
do
    local w = new_env()
    w:new_pawn(1)
    local settle_until = w.clock + 2.5
    w.api.dress_wait = function() return math.max(0, settle_until - w.clock) end
    w:kit("man_at_arms", 3)
    w:tick(10)
    T.check(w.applies == 0, "no Set Up Armor while the hair is settling", tostring(w.applies))
    T.check(w:status().ok == false and w:status().error == "dressing", "status says dressing meanwhile", T.repr(w:status()))
    T.check(#T.filter(w.logs, function(l) return T.contains(l, "hair settle") end) == 1, "one log line, not one per tick", w:logtext())
    w:tick(20)
    T.check(w.applies >= 1 and w:status().ok == true, "dressed and verified once the window passed", T.repr(w:status()))
end

T.log("== IO-1: a hidden pawn is dressed at once (the normal path is not slowed down)")
do
    local w = new_env()
    w:new_pawn(1)
    w.api.dress_wait = function() return 0 end
    w:kit("man_at_arms", 3)
    w:tick(1)
    T.check(w.applies == 1, "applied on the first tick", tostring(w.applies))
end

T.log("== dev kit delay: the kit is held back until the pawn has been hidden that long")
do
    local w = new_env()
    w:new_pawn(1)
    local born = w.clock
    w.api.dress_age = function() return w.clock - born end
    w.Kit.DEV_KIT_DELAY_S = 3.2
    w:kit("man_at_arms", 3)
    w:tick(30)
    T.check(w.applies == 0 and w:status().error == "no kit yet", "no dress before the delay", T.repr(w:status()))
    w:tick(5)
    T.check(w.applies == 1, "dressed after the delay", tostring(w.applies))
    w.Kit.DEV_KIT_DELAY_S = 0
end
-- The own-pawn re-dress paths, each with the pawn already visible
-- (dress_wait > 0) when the re-dress is due.
local function settle_env()
    local w = new_env()
    w:new_pawn(1)
    w.settle_until = 0
    w.api.dress_wait = function() return math.max(0, w.settle_until - w.clock) end
    return w
end

T.log("== IO-1: a late kit update (new rev during the countdown) waits for the hair")
do
    local w = settle_env()
    w:kit("man_at_arms", 3)
    w:tick(6)
    local n = w.applies
    T.check(n >= 1 and w:status().ok == true, "first kit on while hidden", T.repr(w:status()))
    w.settle_until = w.clock + 2.5           -- revealed just now
    w:kit("duelist", 4)
    w:tick(10)
    T.check(w.applies == n, "no Set Up Armor for the new rev inside the window", tostring(w.applies - n))
    w:tick(25)
    T.check(w.applies > n and w:status().rev == "4" and w:status().ok == true, "re-dressed after the window", T.repr(w:status()))
end

T.log("== IO-1: round reset with the new pawn visible on the first look")
do
    local w = settle_env()
    w:kit("man_at_arms", 3)
    w:tick(6)
    local n = w.applies
    w.Kit.on_world_change()
    w:new_pawn(2)
    w.settle_until = w.clock + 2.5
    w:tick(10)
    T.check(w.applies == n, "new world: no Set Up Armor while the new pawn's hair settles", tostring(w.applies - n))
    w:tick(25)
    T.check(w.applies > n and w:status().pawn == "Willie_BP_C_2" and w:status().ok == true, "dressed after the window",
        T.repr(w:status()))
end

T.log("== IO-1: the re-dress after a native re-arm (stability window) waits too")
do
    local w = settle_env()
    w:kit("man_at_arms", 3)
    w:tick(6)
    local n = w.applies
    w.settle_until = w.clock + 2.5
    w.pawn.props["Weapon R"] = nil           -- the game re-armed from the passport: fists
    w:tick(10)
    T.check(w.applies == n and T.contains(w:logtext(), "re-dressing"), "re-dress decided but not run inside the window",
        tostring(w.applies - n))
    w:tick(25)
    T.check(w.applies > n, "re-dressed after the window", tostring(w.applies - n))
end