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
    T.isolated(T.script, "case", { kind = "hair_settle", standin_visible = true })
    T.isolated(T.script, "case", { kind = "hair_settle_off", standin_visible = true, settle = "0" })
    T.isolated(T.script, "case", { kind = "hair_late_update" })
    T.isolated(T.script, "case", { kind = "hair_round_reset" })
    T.isolated(T.script, "case", { kind = "hair_kit_change" })
    T.isolated(T.script, "case", { kind = "hair_warm", standin_visible = true })
    T.isolated(T.script, "case", { kind = "hair_warm_world_gone", standin_visible = true })
    T.isolated(T.script, "case", { kind = "hair_warm_off", standin_visible = true, warm = "0" })
    T.isolated(T.script, "case", { kind = "strip" })
    T.isolated(T.script, "case", { kind = "strip_world_held" })
    T.isolated(T.script, "case", { kind = "publish" })
    T.isolated(T.script, "case", { kind = "menu_clean" })
    T.isolated(T.script, "case", { kind = "settle" })
    T.isolated(T.script, "case", { kind = "yield_passport" })
    T.isolated(T.script, "case", { kind = "stale_session" })
    T.isolated(T.script, "case", { kind = "weapon_gen" })
    T.isolated(T.script, "case", { kind = "record" })
    T.isolated(T.script, "case", { kind = "native_empty_fists" })
    T.isolated(T.script, "case", { kind = "passport_write_retry" })
    T.isolated(T.script, "case", { kind = "passport_variant" })
    return
end

if opts.kind=="passport_variant"then
    package.path=T.path("mods/HSMPLoadout/Scripts/?.lua")..";"..T.path("mods/shared/?.lua")..";"..package.path
end

local M = require("umg_mock")
local sd = T.tmpdir("hsmp_lo_sd_")
local la = T.tmpdir("hsmp_lo_la_")

local setup_armor, destroyed = 0, {}
local function boot(world)
    M.install({ state_dir = sd, env = { LOCALAPPDATA = la, HSMP_INST = "7", HSMP_HAIR_SETTLE_S = opts.settle, HSMP_HAIR_WARM_S = opts.warm }, strict = true })
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
    Mt.SetActorHiddenInGame = function(self, h) self.__props.bHidden = (h == true) end
    _G.RegisterHook = function() return 1, 2 end
    M.own = M.new_obj("Willie_BP_C", "Willie_BP_C_3"); rawset(M.own, "__addr", 1003)
    M.standin = M.new_obj("Willie_BP_C", "Willie_BP_C_9"); rawset(M.standin, "__addr", 1009)
    -- Every native Willie has the equipment passport map. A missing map
    -- must fail dressing rather than letting an empty write look successful.
    M.standin.__props["Character Passport"] = { ["Equipment_26_741A2FC641801842FE691295645C604F"] = {
        ["ArmorinSlots_5_BD7AC6CB43FBB2FDB943E7864486F358"] = {
            Empty = function() end, Add = function() end, ForEach = function() end },
        ["WeaponinHands_23_B3FE643741AF91A6DFE51888205C0F05"] = {
            Add = function() end, Remove = function() end } } }
    M.standin.__props.bHidden = (opts.standin_visible ~= true)   -- in game the BeginPlay hook hides it
    M.pc.__props.Pawn = M.own
    _G.FindAllOf = function(c)
        if c == "Willie_BP_C" then return { M.own, M.standin } end
        return nil
    end
    _G.RegisterBeginPlayPostHook = function(fn) M.bp_hook = fn end
    if opts.kind == "strip" or opts.kind == "weapon_gen" then
        local path = "/Game/Assets/Weapons/Blueprints/Built_Weapons/Weapon_Fists.Weapon_Fists_C"
        local fc = { IsValid=function()return true end, GetFullName=function()return "BlueprintGeneratedClass "..path end,
            GetCDO=function()return {IsValid=function()return true end,["Weapon Passport"]={}} end }
        local find = StaticFindObject
        _G.StaticFindObject=function(p)return p==path and fc or find(p) end
        for _,side in ipairs({"R","L"}) do
            Mt["Set Up "..(side=="R" and "Right" or "Left").." Hand Weapon"]=function(pawn)
                local old=pawn.__props["Weapon "..side]
                if old and old:IsValid() then old:K2_DestroyActor() end
                local a=M.new_obj("Weapon_Fists_C","NativeFist_"..side)
                rawset(a,"__clspath",path); a.__props["Weapon Passport"]={}
                pawn.__props["Weapon "..side]=a
            end
        end
    end
    dofile(LO)
end

-- A Willie of the arena world goes through BeginPlay (the hook HSMPLoadout registers).
local function begin_play(w)
    local Mt = M.Methods
    local full = Mt.GetFullName
    Mt.GetFullName = function(self)
        if rawget(self, "__cls") == "Willie_BP_C" then
            return "Willie_BP_C /Game/Maps/Arenas/Map_Arena_Pit.Map_Arena_Pit:PersistentLevel." .. tostring(rawget(self, "__name"))
        end
        return full(self)
    end
    M.bp_hook({ get = function() return w end })
    Mt.GetFullName = full
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

if opts.kind=="passport_variant"then
    boot()
    local fields_text=T.read(LO):match("local WEAPON_FIELDS = (%b{})")
    local fields=assert(load("return "..fields_text))()
    local classpath="/Game/Assets/Weapons/ModularWeaponBP_Sword.ModularWeaponBP_Sword_C"
    local classes={}
    local function expand(short)
        local path=short:gsub("^@Weapons/","/Game/Assets/Weapons/")
        local leaf=path:match("([^/]+)$")
        return path.."."..leaf.."_C"
    end
    local function class(path)
        if not classes[path]then classes[path]={IsValid=function()return true end,
            GetFullName=function()return "BlueprintGeneratedClass "..path end,
            GetFName=function()return FName(path:match("([^%.]+)$"))end,
            GetCDO=function()return {IsValid=function()return true end,["Weapon Passport"]={}}end}end
        return classes[path]
    end
    StaticFindObject=function(path)return class(path)end
    local wanted={class="@Weapons/ModularWeaponBP_Sword",head="@Weapons/Modules/HeadA",grip="@Weapons/Modules/GripA",
        name="variant",id=7,head_size={1,1.0001,1},mass_head=1.1,mat_steel=4,
        color_wood={0,0,0,1},color_leather={0,0,0,1}}
    local native={}
    for _,fd in ipairs(fields)do
        local kind,v=fd[2],wanted[fd[3]]
        if kind=="class"then native[fd[1]]=v and class(expand(v))or nil
        elseif kind=="name"then native[fd[1]]={ToString=function()return v or ""end}
        elseif kind=="num"or kind=="int"then native[fd[1]]=v or 0
        elseif kind=="vec"then native[fd[1]]={X=v and v[1]or 0,Y=v and v[2]or 0,Z=v and v[3]or 0}
        elseif kind=="color"then native[fd[1]]={R=0,G=0,B=0,A=1}end
    end
    local current=weapon("same-class-before",9001,classpath)
    current.__props["Weapon Passport"]=native
    M.standin.__props["Weapon R"]=current
    local setups,stored=0,{}
    M.standin.__props["Character Passport"]["Equipment_26_741A2FC641801842FE691295645C604F"]
        ["WeaponinHands_23_B3FE643741AF91A6DFE51888205C0F05"].Add=function(_,hand,pass)stored[hand]=pass end
    M.Methods["Set Up Right Hand Weapon"]=function(w,cls,actor,reversed,destroy_previous,pass)
        setups=setups+1
        T.check(destroy_previous==true,"variant replacement uses native previous-weapon destruction path")
        if w.__props["Weapon R"]then w.__props["Weapon R"]:K2_DestroyActor()end
        local fresh=weapon("same-class-after"..setups,9100+setups,classpath)
        for _,fd in ipairs(fields)do
            if fd[2]=="name"then
                local text=pass[fd[1]]
                pass[fd[1]]={ToString=function()return text end}
            end
        end
        fresh.__props["Weapon Passport"]=pass;w.__props["Weapon R"]=fresh
    end
    M.Methods["Set Up Left Hand Weapon"]=function()end
    HSMPNative.bus_put("puppets",{rows={{peer=2,name="Willie_BP_C_9"}}})
    local function publish_variant(version)
        HSMPNative.sc_put("peer_loadout",{version=version,flags=1,r=wanted,rows={}},2)
    end
    publish_variant(1);run(4500,true)
    T.check(setups==0 and M.standin.__props["Weapon R"]==current,"exact same-class full Passport reuses existing native actor")
    T.check(stored[0]and string.pack("<f",stored[0]["CustomMassScaleHead_30_B95872A242AD944E2CE4D493F718F9D7"])
        ==string.pack("<f",wanted.mass_head),
        "same-variant reuse refreshes character hand passport for delayed native rearm",M.logtext())
    wanted.mass_head=2.2;publish_variant(2);run(1500,true)
    T.check(setups==1 and M.standin.__props["Weapon R"]~=current,"same-class custom mass update reaches hand applier and replaces actor")
    local generation=HSMPNative.sc_get("standin_weapons")
    T.check(generation and generation.gen==1,"same-class variant replacement invalidates cached weapon components")
    wanted.head_size[2]=1.0002;publish_variant(3);run(1500,true)
    T.check(setups==2,"submillimeter same-class shape update survives exact application key")
    publish_variant(4);run(1500,true)
    T.check(setups==2,"identical variant version refresh does not rebuild a stable native weapon")
    local own_weapon=weapon("source-same-class",9500,classpath)
    local own_pass={}
    for key,value in pairs(M.standin.__props["Weapon R"].__props["Weapon Passport"])do own_pass[key]=value end
    local size_field="HeadSize_21_2D425E61473B8F64FBAB51B223459D57"
    own_pass[size_field]={X=1,Y=1.0002,Z=1}
    own_weapon.__props["Weapon Passport"]=own_pass;M.own.__props["Weapon R"]=own_weapon
    run(2500,true)
    local before=HSMPNative.sc_get("loadout")
    T.check(before and string.pack("<f",before.r.head_size[2])==string.pack("<f",1.0002),
        "owner publisher preserves native weapon shape without .001 rounding")
    own_pass[size_field].Y=1.00021;run(2500,true)
    local after=HSMPNative.sc_get("loadout")
    T.check(before and after and before.version~=after.version
        and string.pack("<f",after.r.head_size[2])==string.pack("<f",1.00021),
        "owner same-class tiny variant change publishes instead of disappearing in rounded content signature")
    return
end
-- The own `loadout` record the writer put (nil = never written).
local function own_loadout() return HSMPNative.sc_get("loadout") end
-- Peer 2's loadout with armour pieces ({slot, class path} rows).
local function remote_armour(v, pieces)
    local rows = {}
    for i, p in ipairs(pieces) do rows[i] = { flags = 1, slot = p[1], class = p[2] } end
    HSMPNative.sc_put("peer_loadout", { version = v, flags = 0, rows = rows }, 2)
end

if opts.kind == "passport_write_retry" then
    boot()
    local pass = M.standin.__props["Character Passport"]
    M.standin.__props["Character Passport"] = nil
    HSMPNative.bus_put("puppets", { rows = { { peer = 2, name = "Willie_BP_C_9" } } })
    remote(1, nil)
    run(4300, true)
    T.check(setup_armor == 0 and T.contains(M.logtext(), "passport=false"),
        "missing passport blocks native dressing and is reported for retry", M.logtext())
    M.standin.__props["Character Passport"] = pass
    run(1500, true)
    T.check(setup_armor > 0, "same loadout retries when the native passport becomes ready")
end

if opts.kind == "native_empty_fists" then
    boot()
    local fists_path = "/Game/Assets/Weapons/Blueprints/Built_Weapons/Weapon_Fists.Weapon_Fists_C"
    local sword_path = "/Game/Assets/Weapons/ModularWeaponBP_Sword.ModularWeaponBP_Sword_C"
    local classes = {}
    for _, path in ipairs({fists_path, sword_path}) do
        classes[path] = { IsValid = function() return true end,
            GetFullName = function() return "BlueprintGeneratedClass " .. path end,
            GetFName = function() return FName(path:match("([^%.]+)$")) end,
            GetCDO = function() return { IsValid = function() return true end, ["Weapon Passport"] = {} } end }
    end
    local find = StaticFindObject
    _G.StaticFindObject = function(path) return classes[path] or find(path) end
    local setups = 0
    for _, side in ipairs({"R", "L"}) do
        M.Methods["Set Up " .. (side == "R" and "Right" or "Left") .. " Hand Weapon"] = function(pawn, cls, actor, dropped, destroy, pass)
            T.check(dropped == false and destroy == true, "empty hand uses original native setup arguments")
            setups = setups + 1
            local path = cls:GetFullName():gsub("^BlueprintGeneratedClass ", "")
            local old = pawn.__props["Weapon " .. side]
            if old and old:IsValid() then
                pawn.__props["All Weapons Weights"] = pawn.__props["All Weapons Weights"] - old.__props.mass
                pawn.__props["Armor Weight Body"] = pawn.__props["Armor Weight Body"] - old.__props.mass
                old:K2_DestroyActor()
            end
            local mass = path == fists_path and 0.5 or 5
            pawn.__props["All Weapons Weights"] = pawn.__props["All Weapons Weights"] + mass
            pawn.__props["Armor Weight Body"] = pawn.__props["Armor Weight Body"] + mass
            local a = weapon("Native_" .. side .. "_" .. setups, 9000 + setups, path)
            a.__props.mass = mass
            a.__props["Weapon Passport"], a.__props["Parent Actor"] = pass, pawn
            pawn.__props["Weapon " .. side] = a
        end
    end
    M.standin.__props["Weapon R"] = weapon("OriginalSword", 8001, sword_path)
    M.standin.__props["Weapon R"].__props.mass = 5
    M.standin.__props["Weapon R"].__props["Weapon Passport"] = { HeadSize_21_2D425E61473B8F64FBAB51B223459D57 = {X=9,Y=9,Z=9} }
    M.standin.__props["All Weapons Weights"], M.standin.__props["Armor Weight Body"] = 5, 25
    HSMPNative.bus_put("puppets", { rows = { {peer = 2, name = "Willie_BP_C_9"} } })
    remote(1, nil)
    run(4500, true)
    local r, l = M.standin.__props["Weapon R"], M.standin.__props["Weapon L"]
    T.check(r and l and rawget(r, "__clspath") == fists_path and rawget(l, "__clspath") == fists_path,
        "empty appearance creates native fists symmetrically")
    T.check(T.any(destroyed, function(n) return n == "OriginalSword" end), "empty desired hand strips real arena weapon")
    T.check(M.standin.__props["All Weapons Weights"] == 1 and M.standin.__props["Armor Weight Body"] == 21,
        "native replacement subtracts old sword weight before adding fists")
    T.check(not r.__props["Weapon Passport"].HeadSize_21_2D425E61473B8F64FBAB51B223459D57 or r.__props["Weapon Passport"].HeadSize_21_2D425E61473B8F64FBAB51B223459D57.X ~= 9,
        "native fists use their own template without the previous sword head size")
    local gen = HSMPNative.sc_get("standin_weapons")
    T.check(gen and gen.gen == 1, "native replacement invalidates cached hand components")
    local n = setups
    remote(2, nil); run(1600, true)
    T.check(setups == n and r:IsValid() and l:IsValid(), "repeated empty snapshot retains fists without actor churn")
    T.check(r.__props["Parent Actor"] == M.standin and l.__props["Parent Actor"] == M.standin,
        "native setup retains the correct parent identity")
    remote(3, "@Weapons/ModularWeaponBP_Sword"); run(1600, true)
    T.check(rawget(M.standin.__props["Weapon R"], "__clspath") == sword_path and not r:IsValid(),
        "later real weapon replaces the fist through native setup")
    T.check(l:IsValid(), "other empty hand keeps its fist")
    T.check(M.standin.__props["All Weapons Weights"] == 5.5 and M.standin.__props["Armor Weight Body"] == 25.5,
        "native sword replacement subtracts fist weight exactly once")
    return
end

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

elseif opts.kind == "hair_settle" then
    -- IO-1: a stand-in that is already visible (census unhide, safety reveal)
    -- is dressed only once its hair has been visible for the settle time.
    boot()
    HSMPNative.bus_put("puppets", { rows = { { peer = 2, name = "Willie_BP_C_9" } } })
    remote(1, "@Weapons/Sword")
    run(3000, true)
    T.check(setup_armor == 0, "no Set Up Armor while the hair settles (the control dresses by 2.5 s)", setup_armor)
    run(3000, true)
    T.check(T.contains(M.logtext(), "became visible undressed"), "the wait is logged", M.logtext())
    T.check(setup_armor >= 1 and T.contains(M.logtext(), "applied loadout v=1"), "dressed after the settle", M.logtext())

elseif opts.kind == "hair_settle_off" then
    -- Control for the case above: with the wait off the same stand-in is
    -- dressed by 2.5 s.
    boot()
    HSMPNative.bus_put("puppets", { rows = { { peer = 2, name = "Willie_BP_C_9" } } })
    remote(1, "@Weapons/Sword")
    run(2500, true)
    T.check(setup_armor >= 1, "HSMP_HAIR_SETTLE_S=0: dressed as soon as the world settle allows", setup_armor)

elseif opts.kind == "hair_late_update" then
    -- IO-1: the owner's armour changes right after the stand-in was dressed
    -- and revealed. The re-dress waits until its hair has settled.
    boot()
    HSMPNative.bus_put("puppets", { rows = { { peer = 2, name = "Willie_BP_C_9" } } })
    remote(1, "@Weapons/Sword")
    run(3000, true)
    T.check(setup_armor >= 1, "dressed while hidden", setup_armor)
    M.standin:SetActorHiddenInGame(false)   -- revealed (the mock has no BeginPlay hook to track it)
    run(100, true)
    local n = setup_armor
    remote_armour(2, { { 3, "@Armor/Blueprints/Built_Armor/BP_Armor_Head_Helmet_A" } })
    run(1500, true)
    T.check(setup_armor == n, "no re-dress inside the settle window after the reveal", setup_armor - n)
    run(3000, true)
    T.check(setup_armor > n and T.contains(M.logtext(), "applied loadout v=2"), "re-dressed once the hair settled", M.logtext())

elseif opts.kind == "hair_warm" then
    -- IO-1: in the first arena world the BeginPlay hide waits HAIR_WARM_S (the
    -- groom renders first); in later worlds it is immediate.
    boot()
    run(500, true)
    begin_play(M.standin)
    T.check(M.standin.__props.bHidden == false, "first arena: not hidden at BeginPlay")
    run(300, true)
    T.check(M.standin.__props.bHidden == false, "still visible 0.3 s in")
    run(400, true)
    T.check(M.standin.__props.bHidden == true, "hidden once the groom had 0.5 s")
    M.premap()
    M.standin.__props.bHidden = false
    begin_play(M.standin)
    T.check(M.standin.__props.bHidden == true, "next world: hidden at BeginPlay")
    T.check(#M.dead_touch == 0, "nothing freed touched", T.repr(M.dead_touch))

elseif opts.kind == "hair_warm_world_gone" then
    -- The world changes before the delayed hide: the old actor is never
    -- touched, and the next world still warms up first.
    boot()
    run(500, true)
    begin_play(M.standin)
    run(100, true)
    M.premap()
    run(600, true)
    T.check(M.standin.__props.bHidden == false, "the old world's actor was not touched")
    begin_play(M.standin)
    T.check(M.standin.__props.bHidden == false, "the next world warms up too (the first never did)")
    run(700, true)
    T.check(M.standin.__props.bHidden == true, "hidden after its own warm-up")

elseif opts.kind == "hair_warm_off" then
    boot()
    run(500, true)
    begin_play(M.standin)
    T.check(M.standin.__props.bHidden == true, "HSMP_HAIR_WARM_S=0: hidden at BeginPlay (the old hide)")

elseif opts.kind == "hair_kit_change" then
    -- IO-1: peer 2's validated kit arrives / changes after their stand-in
    -- was revealed: the re-dress for the new kit waits for the hair.
    boot()
    HSMPNative.bus_put("puppets", { rows = { { peer = 2, name = "Willie_BP_C_9" } } })
    remote(1, "@Weapons/Sword")
    run(3000, true)
    T.check(setup_armor >= 1, "dressed while hidden", setup_armor)
    M.standin:SetActorHiddenInGame(false)
    run(100, true)
    local n = setup_armor
    HSMPNative.sc_put("peer_kit", { rev = 5, seq = 1, class = "man_at_arms", r = "", l = "", rows = {} }, 2)
    run(1500, true)
    T.check(setup_armor == n, "no re-dress for the new kit inside the window", setup_armor - n)
    run(3000, true)
    T.check(setup_armor > n, "re-dressed for the new kit after the window", setup_armor - n)

elseif opts.kind == "hair_round_reset" then
    -- IO-1: a round reset with a pooled stand-in the BeginPlay hook did not
    -- hide again: visible on the first look, so it waits like a revealed one.
    boot()
    HSMPNative.bus_put("puppets", { rows = { { peer = 2, name = "Willie_BP_C_9" } } })
    remote(1, "@Weapons/Sword")
    run(3000, true)
    T.check(setup_armor >= 1, "dressed in the first world", setup_armor)
    M.premap()
    M.standin.__props.bHidden = false
    local n = setup_armor
    run(3000, true)
    T.check(setup_armor == n, "new world, visible stand-in: no Set Up Armor in the first 3 s", setup_armor - n)
    run(4000, true)
    T.check(setup_armor > n, "dressed after the settle", setup_armor - n)

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
            -- Native pooled-foe gear outside the catalogue must not leak
            -- into an owner's complete appearance, including an empty slot.
            ForEach = function(_, fn) fn({ get = function() return 0 end }, { get = function()
                return { [AF.core] = cls("/Game/Assets/Armor/X/BP_FoeHelm.BP_FoeHelm_C"), [AF.slot] = 0 }
            end }) end },
        ["WeaponinHands_23_B3FE643741AF91A6DFE51888205C0F05"] = { Add = function() end, Remove = function() end } } }
    HSMPNative.bus_put("puppets", { rows = { { peer = 2, name = "Willie_BP_C_9" } } })
    HSMPNative.sc_put("peer_loadout", rec, 2)
    run(2000, true)
    local p = added[3]
    T.check(added[0] == nil, "pooled foe helmet is not merged into the owner's replicated armour", T.repr(added))
    T.check(p and p[AF.fab1] and p[AF.fab1].R == 0.5 and p[AF.fab1].B == 0.125 and p[AF.rust] == true and p[AF.price] == 5,
        "the stand-in's passport map gets the owner's passport (by field name)", T.repr(p))
    T.check(T.contains(M.logtext(), "applied loadout v=" .. tostring(rec and rec.version)), "and the version is applied", M.logtext())
end
