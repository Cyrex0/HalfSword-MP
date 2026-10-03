-- HSMPLoadout runtime tests, offline under umg_mock (strict).
--
--     hsmp-tools lua-test loadout
--
--   * a held-item change on a stand-in runs only the hands, never
--     "Set Up Armor" (which respawns every armour actor mid-fight).
--   * a dynamic world item from a peer (they dropped / were disarmed)
--     strips the stand-in's copy at once; the own hands are published on an
--     address change (not on the next 2 s writer tick).
--   * the menu never deletes kit files while a sidecar is beating; it
--     does once the link record has been absent for >= 2 s.
-- Every case runs in its own Lua state (T.isolated).

local mode, opts = ...
local LO = T.path("mods/HSMPLoadout/Scripts/main.lua")

if mode ~= "case" then
    T.isolated(T.script, "case", { kind = "m15" })
    T.isolated(T.script, "case", { kind = "strip" })
    T.isolated(T.script, "case", { kind = "strip_world_held" })
    T.isolated(T.script, "case", { kind = "publish" })
    T.isolated(T.script, "case", { kind = "menu_clean" })
    T.isolated(T.script, "case", { kind = "settle" })
    T.isolated(T.script, "case", { kind = "yield_passport" })
    T.isolated(T.script, "case", { kind = "stale_session" })
    T.isolated(T.script, "case", { kind = "weapon_gen" })
    T.isolated(T.script, "case", { kind = "record" })
    return
end

local M = require("umg_mock")
local sd = T.tmpdir("hsmp_lo_sd_")
local la = T.tmpdir("hsmp_lo_la_")

local setup_armor, destroyed = 0, {}
local function boot(world)
    M.install({ state_dir = sd, env = { LOCALAPPDATA = la, HSMP_INST = "7" }, strict = true })
    local wname = world or "World /Game/Maps/Arenas/Map_Arena_Pit/Map_Arena_Pit.Map_Arena_Pit"
    local Mt = M.Methods
    Mt.GetFullName = function(self)
        if rawget(self, "__cls") == "World" then return wname end
        return "Obj " .. tostring(rawget(self, "__name"))
    end
    Mt.GetAddress = function(self) return rawget(self, "__addr") or 4096 end
    Mt.GetWorld = function() return M.world end
    Mt.GetClass = function(self)
        local c = rawget(self, "__cls")
        local path = rawget(self, "__clspath") or ("/Game/X/" .. tostring(c):gsub("_C$", "") .. "." .. tostring(c))
        return { GetFName = function() return { ToString = function() return c end } end,
                 IsValid = function() return true end,
                 GetFullName = function() return "BlueprintGeneratedClass " .. path end }
    end
    Mt["Set Up Armor"] = function() setup_armor = setup_armor + 1 end
    -- A destroyed actor is pending kill (IsValid false; UPROPERTY references
    -- are nulled by the next GC), not freed memory.
    Mt.K2_DestroyActor = function(self) destroyed[#destroyed + 1] = rawget(self, "__name"); rawset(self, "__pk", true) end
    local isvalid = Mt.IsValid
    Mt.IsValid = function(self)
        if rawget(self, "__pk") and not rawget(self, "__dead") then return false end
        return isvalid(self)
    end
    Mt.K2_GetActorLocation = function() return { X = 0, Y = 0, Z = 0 } end
    Mt.SetActorHiddenInGame = function() end
    _G.RegisterHook = function() return 1, 2 end
    M.own = M.new_obj("Willie_BP_C", "Willie_BP_C_3"); rawset(M.own, "__addr", 1003)
    M.standin = M.new_obj("Willie_BP_C", "Willie_BP_C_9"); rawset(M.standin, "__addr", 1009)
    M.pc.__props.Pawn = M.own
    _G.FindAllOf = function(c)
        if c == "Willie_BP_C" then return { M.own, M.standin } end
        return nil
    end
    dofile(LO)
end

local function weapon(name, addr, clspath)
    local leaf = clspath:match("([^%./]+)$")
    local w = M.new_obj(leaf, name)
    rawset(w, "__addr", addr); rawset(w, "__clspath", clspath)
    w.__props["Weapon Passport"] = {}
    return w
end

-- The sidecar's typed link record in the native mock, and its header heartbeat.
local NAT = rawget(_G, "HSMPNative")
local SCH = dofile(T.path("mods/shared/hsmp_ipc_schema.lua"))
local function link_connected()
    NAT.sc_put("link", { status = SCH.ENUMS.sidecar_status.CONNECTED, state = SCH.ENUMS.link_state.UP, my_peer_id = 1 })
end
local function beat()
    link_connected()
    NAT._st.hb_age = 0.01
    -- The sidecar's peer directory (IPC.peer_dir): peer 2 has a peer slot.
    NAT.sc_peer_dir({ { id = 2, nick = "B" } })
end
local function run(ms, live)
    for _ = 1, math.floor(ms / 100) do
        if live then beat() end
        M.run(100)
    end
end
-- The sidecar publishes peer 2's `loadout` record into its per-peer blob (slot 2 in the mock).
local function remote(v, R)
    HSMPNative.sc_put("peer_loadout", { version = v, flags = R and 1 or 0, r = R and { class = R } or nil, rows = {} }, 2)
end
-- The own `loadout` record the writer put (nil = never written).
local function own_loadout() return HSMPNative.sc_get("loadout") end

if opts.kind == "m15" then
    boot()
    HSMPNative.bus_put("puppets", { rows = { { peer = 2, name = "Willie_BP_C_9" } } })
    remote(1, "@Weapons/Sword")
    run(3000, true)
    local n = setup_armor
    T.check(n >= 1 and T.contains(M.logtext(), "applied loadout v=1"), "the stand-in is dressed once", M.logtext())
    remote(2, nil)                       -- the owner dropped the sword: a new loadout version, same armour
    run(1500, true)
    T.check(setup_armor == n, "a held-item change never re-runs Set Up Armor", setup_armor - n)
    T.check(T.contains(M.logtext(), "hands only on Willie_BP_C_9"), "the hands are updated alone", M.logtext())
    T.check(#M.dead_touch == 0, "nothing freed touched", T.repr(M.dead_touch))

elseif opts.kind == "strip" then
    boot()
    local sw = weapon("Sword_9", 9001, "/Game/Weapons/ModularWeaponBP_Sword.ModularWeaponBP_Sword_C")
    M.standin.__props["Weapon R"] = sw
    HSMPNative.bus_put("puppets", { rows = { { peer = 2, name = "Willie_BP_C_9" } } })
    remote(1, "@Weapons/ModularWeaponBP_Sword")
    run(3000, true)
    T.check(#destroyed == 0, "dressed; the sword stays in the stand-in's hand", T.repr(destroyed))
    -- peer 2's sword leaves their hand: HSMPWorld's manifest gets a dynamic entry
    HSMP_IPC.N.sc_put("world_dyn", { level = 3, epoch = 2, rows = { { id = 2147549185, chash = 77, pos = { -10.5, 20.0, 5.0 },
        dyn_owner = 2, class_path = "/Game/Weapons/ModularWeaponBP_Sword.ModularWeaponBP_Sword_C" } } })
    run(200, true)
    T.check(destroyed[1] == "Sword_9", "the stand-in's copy is stripped at once", T.repr(destroyed))
    T.check(T.contains(M.logtext(), "peer 2 dropped ModularWeaponBP_Sword_C"), "logged", M.logtext())
    T.check(#M.dead_touch == 0, "the stripped actor is never touched again", T.repr(M.dead_touch))

elseif opts.kind == "strip_world_held" then
    boot()
    local sw = weapon("Sword_9", 9001, "/Game/Weapons/ModularWeaponBP_Sword.ModularWeaponBP_Sword_C")
    M.standin.__props["Weapon R"] = sw
    HSMPNative.bus_put("puppets", { rows = { { peer = 2, name = "Willie_BP_C_9" } } })
    HSMP_IPC.bus_put("world_held", { rows = { { peer = 2, nid = 123, hand = 0, actor = "Sword_9" } } })   -- HSMPWorld shows a world item there
    remote(1, nil)
    run(3000, true)
    local n0 = #destroyed
    HSMP_IPC.N.sc_put("world_dyn", { level = 3, epoch = 2, rows = { { id = 2147549185, chash = 77, pos = { -10.5, 20.0, 5.0 },
        dyn_owner = 2, class_path = "/Game/Weapons/ModularWeaponBP_Sword.ModularWeaponBP_Sword_C" } } })
    run(300, true)
    T.check(#destroyed == n0 and not T.contains(M.logtext(), "peer 2 dropped"),
        "a hand that shows a world item (world_held) is never stripped", T.repr(destroyed))

elseif opts.kind == "publish" then
    boot()
    M.own.__props["Weapon R"] = weapon("A", 7001, "/Game/W/A.A_C")
    run(4300, true)   -- nothing before the 2 s world settle; liveness needs a heartbeat
    local first = own_loadout()
    T.check(first and first.flags == 1 and first.r.class:find("/Game/W/A", 1, true) ~= nil and first.version > 0,
        "the 2 s writer published the own loadout record (right hand passport)", T.repr(first))
    M.own.__props["Weapon R"] = weapon("B", 7002, "/Game/W/B.B_C")   -- a pickup / disarm swap
    run(300, true)
    local now = own_loadout()
    T.check(now and now.r.class:find("/Game/W/B", 1, true) ~= nil and now.version > first.version,
        "a hand change is published within ~100 ms, as a new version", T.repr(now))

elseif opts.kind == "menu_clean" then
    -- a session is being joined from the menu; the kit status (bus key
    -- "kit_status", a typed record) is live state
    HSMPNative.bus_put("kit_status", { pawn = "Willie_BP_C_3", ok = true })
    local function status_kept() local s = HSMPNative.sc_get("kit_status"); return s ~= nil and s.pawn ~= "" end
    beat()
    boot("World /Game/Maps/Map_Menu_Startup.Map_Menu_Startup")
    run(3000, true)
    T.check(status_kept(), "first look at a live session: nothing is cleared")
    NAT._rec.slots.link = nil; NAT._st.hb_age = 1e9   -- the session ended (no link, no heartbeat)
    run(1000, false)
    T.check(status_kept(), "absent for < 2 s: still kept")
    run(3000, false)
    T.check(not status_kept(), "absent for >= 2 s: the leftover kit status is cleared (the zero record)",
        (M.logtext():match("[^\n]*leftover[^\n]*") or "no cleanup log") .. " | " .. T.repr(HSMPNative.sc_get("kit_status")))
end

if opts.kind == "settle" then
    -- No FindAllOf over Willies / weapons and no hand read in the first
    -- 2 s of a world (the round-reload crash window).
    boot()
    local scans = 0
    local orig = _G.FindAllOf
    _G.FindAllOf = function(c) if c == "Willie_BP_C" or c == "ModularWeaponBP_C" then scans = scans + 1 end; return orig(c) end
    local reads = 0
    M.own.__props["Weapon R"] = weapon("A", 7001, "/Game/W/A.A_C")
    local mt = getmetatable(M.own)
    local idx = mt.__index
    mt.__index = function(t, k) if k == "Weapon R" or k == "Weapon L" then reads = reads + 1 end; return idx(t, k) end
    HSMPNative.bus_put("puppets", { rows = { { peer = 2, name = "Willie_BP_C_9" } } })
    remote(1, "@Weapons/Sword")
    run(1800, true)
    T.check(scans == 0 and reads == 0, "nothing walks Willies / weapons or reads a hand in the first 2 s", T.repr({ scans, reads }))
    T.check(own_loadout() == nil, "no 2 s loadout write inside the settle window")
    run(3000, true)
    mt.__index = idx
    T.check(scans > 0 and own_loadout() ~= nil, "after the settle the mod works as before")
end

if opts.kind == "yield_passport" then
    -- A stand-in hand that shows a replicated world item must not keep
    -- a passport entry for the loadout weapon, or the ubergraph re-arms a
    -- colliding duplicate ~0.2 s after the next "Set Up Armor".
    boot()
    local removed, added = {}, {}
    local hands = { Add = function(_, k, v) added[#added + 1] = k end, Remove = function(_, k) removed[#removed + 1] = k end }
    M.standin.__props["Character Passport"] = { ["Equipment_26_741A2FC641801842FE691295645C604F"] =
        { ["WeaponinHands_23_B3FE643741AF91A6DFE51888205C0F05"] = hands } }
    local sw = weapon("Sword_9", 9001, "/Game/Weapons/ModularWeaponBP_Sword.ModularWeaponBP_Sword_C")
    M.standin.__props["Weapon R"] = sw
    HSMPNative.bus_put("puppets", { rows = { { peer = 2, name = "Willie_BP_C_9" } } })
    HSMP_IPC.bus_put("world_held", { rows = { { peer = 2, nid = 123, hand = 0, actor = "WorldSword_5" } } })   -- a world item shows in that hand
    remote(1, "@Weapons/ModularWeaponBP_Sword")
    run(4500, true)
    T.check(T.any(destroyed, function(x) return x == "Sword_9" end), "the loadout copy in a world-item hand is stripped", T.repr(destroyed))
    T.check(T.any(removed, function(x) return x == 0 end), "and the R hand passport entry is removed (no re-arm from it)", T.repr({ removed, added }))
end

if opts.kind == "stale_session" then
    -- A "connected" link a crashed sidecar left behind never dresses
    -- anything (the shared hsmp_session rule: no heartbeat, not live).
    boot()
    link_connected()
    HSMPNative.bus_put("puppets", { rows = { { peer = 2, name = "Willie_BP_C_9" } } })
    remote(1, "@Weapons/Sword")
    run(6000, false)
    T.check(setup_armor == 0 and not T.contains(M.logtext(), "applied loadout"), "a stale 'connected' link dresses nothing", M.logtext())
    T.check(own_loadout() == nil, "and publishes nothing")
    run(3000, true)
    T.check(T.contains(M.logtext(), "applied loadout"), "a live heartbeat: dressing works")
end

if opts.kind == "weapon_gen" then
    -- Every hand-weapon destroy bumps the bus key standin_weapons so
    -- HSMPAvatars drops its cached weapon components at once.
    boot()
    local sw = weapon("Sword_9", 9001, "/Game/Weapons/ModularWeaponBP_Sword.ModularWeaponBP_Sword_C")
    M.standin.__props["Weapon R"] = sw
    HSMPNative.bus_put("puppets", { rows = { { peer = 2, name = "Willie_BP_C_9" } } })
    remote(1, "@Weapons/ModularWeaponBP_Sword")
    run(4500, true)
    T.check(HSMPNative.sc_get("standin_weapons") == nil, "no destroy yet: no generation")
    remote(2, nil)   -- the owner's hand is empty now: the stand-in's sword is stripped
    run(1500, true)
    T.check(T.any(destroyed, function(x) return x == "Sword_9" end), "the stand-in's sword is destroyed", T.repr(destroyed))
    local g = HSMPNative.sc_get("standin_weapons")
    T.check(g and g.gen == 1, "standin_weapons bumped to gen 1", T.repr(g))
end

if opts.kind == "record" then
    -- The appearance is ONE typed record end to end: the writer turns the own pawn's
    -- worn piece + its passport into one merged row and the right hand's passport into
    -- the head; the applier dresses a stand-in from the same record (passport fields by
    -- name, no JSON anywhere).
    boot()
    local function cls(path) return { IsValid = function() return true end,
        GetFullName = function() return "BlueprintGeneratedClass " .. path end } end
    local function tmap(entries)
        return { ForEach = function(_, fn)
            for k, v in pairs(entries) do fn({ get = function() return k end }, { get = function() return v end }) end
        end }
    end
    local AF = { core = "ArmorCore_3_F6B7C69C4BD7D9720DB91EB635EE2B43", fab1 = "FabricColor1_15_4C7C24744C4F50FFAFB62DB50DE29393",
                 rust = "RustToggle_73_E4F1415F4A1E7AD75CAFD68BBA632FEF", price = "Price_27_8E3ADD54484EFC4A59FE9381485AC192",
                 slot = "Slot_30_7561CB484566A4512003EA96ED44F88D" }
    local pass = { [AF.core] = cls("/Game/Assets/Armor/X/BP_Torso.BP_Torso_C"), [AF.fab1] = { R = 0.5, G = 0.25, B = 0.125, A = 1 },
                   [AF.rust] = true, [AF.price] = 5, [AF.slot] = 3 }
    M.own.__props["Currently Equipped Armor"] = tmap({ [3] = pass })
    local sw = weapon("A", 7001, "/Game/W/A.A_C")
    sw.__props["Weapon Passport"] = { ["HeadSize_21_2D425E61473B8F64FBAB51B223459D57"] = { X = 1, Y = 2, Z = 3 } }
    M.own.__props["Weapon R"] = sw
    run(4300, true)
    local rec = own_loadout()
    local row = rec and rec.rows and rec.rows[1]
    T.check(rec and #rec.rows == 1 and rec.n == 1 and row.flags == 3 and row.slot == 3 and row.class == "@Armor/X/BP_Torso",
        "piece + passport of one slot: ONE merged row (PIECE|PASSPORT)", T.repr(rec and rec.rows))
    T.check(row and row.fabric1[1] == 0.5 and row.fabric1[3] == 0.125 and row.rust == true and row.price == 5 and row.pslot == 3,
        "passport fields by name (colour, bool, num, Slot)", T.repr(row))
    T.check(rec and rec.flags == 1 and rec.r.class:find("/Game/W/A", 1, true) and rec.r.head_size[3] == 3 and rec.l.class == "",
        "the right hand's passport in the head, the left hand empty", T.repr(rec and rec.r))
    -- The same record arrives as peer 2's appearance: the stand-in gets that passport.
    local added = {}
    M.standin.__props["Character Passport"] = { ["Equipment_26_741A2FC641801842FE691295645C604F"] = {
        ["ArmorinSlots_5_BD7AC6CB43FBB2FDB943E7864486F358"] = {
            Empty = function() added = {} end, Add = function(_, k, v) added[k] = v end,
            ForEach = function() end },
        ["WeaponinHands_23_B3FE643741AF91A6DFE51888205C0F05"] = { Add = function() end, Remove = function() end } } }
    HSMPNative.bus_put("puppets", { rows = { { peer = 2, name = "Willie_BP_C_9" } } })
    HSMPNative.sc_put("peer_loadout", rec, 2)
    run(2000, true)
    local p = added[3]
    T.check(p and p[AF.fab1] and p[AF.fab1].R == 0.5 and p[AF.fab1].B == 0.125 and p[AF.rust] == true and p[AF.price] == 5,
        "the stand-in's passport map gets the owner's passport (by field name)", T.repr(p))
    T.check(T.contains(M.logtext(), "applied loadout v=" .. tostring(rec and rec.version)), "and the version is applied", M.logtext())
end
