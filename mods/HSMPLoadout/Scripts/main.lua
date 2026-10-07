-- HSMPLoadout — makes each peer's stand-in wear the real player's gear.
--
-- Writer (every 2 s in an arena): reads the local Willie's armour from every
-- place the game keeps it (NewArmorSlots, EquippedArmor game-armour objects,
-- ExternalArmorSlots, ArmorSlots, LoadEquipment), sends the richest list
-- cross-checked against WornArmor, plus armour passports (colours) and both
-- held weapons' passports, as one typed record into the game slot "loadout"
-- (schema loadout.rs: LoadoutHead + ArmorRow rows) with a version that changes
-- only when the gear does (pickups, drops, broken armour included). The
-- sidecar sends each version once (reliable); the server keeps the newest per
-- peer and replays it to late joiners.
--
-- Applier (every 1 s): for each stand-in (bus key "puppets", HSMPAvatars), reads the
-- owner's record from the per-peer blob "peer_loadout", fills
-- its slot maps and calls the game's own BP functions by their real names
-- ("Set Up Armor", "Set Up Right/Left Hand Weapon" — they contain spaces),
-- strips weapons the real player isn't holding, retries a failed apply up to
-- 4 times, and only then marks that version applied.
--
-- Diagnostics: a [gear] dump of the local player and every stand-in runs once
-- 10 s after each arena load, and on Ctrl+F7.
--
-- Classes / gear selection (kit.lua + hsmp_catalog.lua): the server-validated
-- kit (per-peer slot "peer_kit") is put on the local pawn on every spawn / round
-- reset, and limits what stand-ins show under CLASSES/CUSTOM rules. Ctrl+F7
-- (gear dump) also re-applies the own kit. See
-- docs/development/subsystems/classes-loadout.md.

local UEHelpers = require("UEHelpers")

-- Thread-safety shim. LoopAsync, ExecuteWithDelay and *Async keybind
-- callbacks run on UE4SS worker threads; running them alongside game-thread
-- Lua corrupted this mod's VM (symbolized crash dumps: lua_next / __index on
-- garbage userdata). Route every deferred, looped and key-bound callback onto
-- the game thread.
if LoopInGameThreadWithDelay and ExecuteInGameThreadWithDelay and CancelDelayedAction then
    ExecuteWithDelay = function(ms, fn) return ExecuteInGameThreadWithDelay(ms, fn) end
    LoopAsync = function(ms, fn)
        local h
        h = LoopInGameThreadWithDelay(ms, function()
            local ok, stop = pcall(fn)
            if ok and stop == true and h then CancelDelayedAction(h) end
        end)
        return h
    end
    -- Key callbacks run on the UE4SS input thread. Native-function hooks run
    -- on the game thread on this mod's hook state without UE4SS's lock, so an
    -- ExecuteInGameThread call from a key callback pushed onto that state
    -- mid-hook (crash in push_structproperty, 2026-10-03). The input thread
    -- now only appends to a queue (plain Lua, no UE4SS call) and a
    -- game-thread loop runs what it queued.
    local kq, kq_w, kq_r, kq_loop = {}, 0, 0, nil
    local function kq_wrap(fn)
        if not kq_loop then
            kq_loop = LoopInGameThreadWithDelay(16, function()
                while kq_r < kq_w do
                    kq_r = kq_r + 1
                    local f = kq[kq_r]
                    kq[kq_r] = nil
                    if f then pcall(f) end
                end
            end)
        end
        return function() local n = kq_w + 1; kq[n] = fn; kq_w = n end
    end
    local _rkba = RegisterKeyBindAsync
    RegisterKeyBindAsync = function(key, mods, fn) return _rkba(key, mods, kq_wrap(fn)) end
    local _rkb = RegisterKeyBind
    RegisterKeyBind = function(key, a, b)
        if b then return _rkb(key, a, kq_wrap(b)) end
        return _rkb(key, kq_wrap(a))
    end
end
-- U4: the shim comes before kit.lua is loaded, so nothing kit.lua captures at load time can be the raw worker-thread LoopAsync / ExecuteWithDelay.
-- kit.lua (classes system). Never let a missing/broken kit.lua take the
-- appearance replication down with it: fall back to a no-op stub.
local Kit
do
    local ok, mod = pcall(require, "kit")
    if not ok or type(mod) ~= "table" then
        local src = (debug.getinfo(1, "S").source or ""):gsub("^@", "")
        local dir = src:match("^(.*)[/\\]") or "."
        local ok2, mod2 = pcall(dofile, dir .. "/kit.lua")
        if ok2 and type(mod2) == "table" then ok, mod = true, mod2 else mod = tostring(mod) .. " / " .. tostring(mod2) end
    end
    if ok and type(mod) == "table" then
        Kit = mod
    else
        print("[HSMPLoadout] kit.lua failed to load (" .. tostring(mod) .. ") - classes disabled\n")
        Kit = {
            init = function() end, tick_local = function() end, force_local = function() end,
            effective_remote = function(_, L) return L, "" end,
            give_weapon = function() return "nokit" end, apply_hair = function() return "-" end,
            armour_sig = function(L) return tostring(L and L.v) end, weapon_sig = function() return "-" end,
            parse_dyn = function() return {} end, forget_rules = function() end,
        }
    end
end


local function Log(fmt, ...)
    print(string.format("[HSMPLoadout] " .. fmt .. "\n", ...))
end

local function trim(s) return s and s:match("^%s*(.-)%s*$") or nil end

-- World guard (see HSMPAvatars): a round reset re-opens the same map and
-- frees every actor of the old world. Bumped by the LoadMap hooks; every
-- per-world cache below is keyed by / dropped on it, and loops re-check the
-- world identity before touching anything.
local world_gen = 0
local function world_gen_ref() return world_gen end

local STATE_DIR    = (trim(os.getenv("HSMP_STATE_DIR")) or "hsmp_state"):gsub("\\", "/")

-- --- passport field tables (mangled BP struct property names, SDK dump) -----

local ARMOR_FIELDS = {
    { "ArmorCore_3_F6B7C69C4BD7D9720DB91EB635EE2B43",          "class", "class" },
    { "ID_54_C6BBB1A64A3828B5AB1D8E804EC7C8F7",                 "int", "id" },
    { "CoreRemoved_12_5CFF8F6D4A05C15812594CAF6771C66B",        "bool", "core_removed" },
    { "Module1_5_46B7198E4341C93CBF6AE989EF9898E4",             "int", "module1" },
    { "Module2_7_5B7940B84CFD673B25103D96E0AFEEB0",             "int", "module2" },
    { "Module3_9_E282C465414F6D4EF2A8039FBA847AD2",             "int", "module3" },
    { "BackgroundColor_58_CD7AE55B4C46E5A79F8448BB9CDB3B82",    "color", "bg_color" },
    { "LeatherColor_67_A8A17E654ED0341E58247C9B39D29597",       "color", "leather_color" },
    { "FabricColor1_15_4C7C24744C4F50FFAFB62DB50DE29393",       "color", "fabric1" },
    { "FabricColor2_17_4199336A482894E5BC99E69E52B50B1C",       "color", "fabric2" },
    { "FabricColor3_89_167D399343950DE18CC2F9AC76D99042",       "color", "fabric3" },
    { "SteelType_84_7BA6626740476C2CD69648847A1E592F",          "int", "steel" },
    { "MetalPiecesType_81_203BFD454D41FA24B0B5C5838898AA60",    "int", "metal" },
    { "RustToggle_73_E4F1415F4A1E7AD75CAFD68BBA632FEF",         "bool", "rust" },
    { "DirtToggle_74_A166ACBE4A95555CCA980F98495E364A",         "bool", "dirt" },
    { "Price_27_8E3ADD54484EFC4A59FE9381485AC192",              "num", "price" },
    { "Slot_30_7561CB484566A4512003EA96ED44F88D",               "int", "pslot" },
    { "ProvidesUpperAP_34_A85C3E3B4E4EF35DA44FFA960797B6C6",    "bool", "up_ap" },
    { "ProvidesLowerAP_36_FFA5916240E32AC30239D58BCDD69D62",    "bool", "low_ap" },
    { "RequiresUpperAP_38_079BBCD74D92FB832584E8B776EC8A6E",    "bool", "req_up_ap" },
    { "RequiresLowerAP_40_BF13845C4B210380A7A569A912A6F614",    "bool", "req_low_ap" },
    { "RequiresModuleHirarchy_47_9ED58E2C48514BE5153606977BE68B6A", "bool", "req_hier" },
    { "Tier_50_E497AE434B01B84C559DEE8A863BB42E",               "int", "tier" },
}

local WEAPON_FIELDS = {
    { "WeaponClass_54_B478ECF7499977809745A3973AD678EC",        "class", "class" },
    { "ID_70_C02CF656483647A1933EEA96314B78A6",                 "int", "id" },
    { "Name_57_3729B51148E846FE8DD336B9419BCEE1",               "name", "name" },
    { "HeadSubModule1_7_ABBFD017411F42A4950B1C9F2360A30D",      "class", "head_sub1" },
    { "HeadSubModule2_9_90AAA8304C7794E1BF814C9354A1A7E9",      "class", "head_sub2" },
    { "HeadModule_11_62DF53134688807E1DA7F4A20E9F7139",         "class", "head" },
    { "GuardModule_13_6DD2B06245505E53B529D090333012F0",        "class", "guard" },
    { "PommelModule_15_561B01324BFCD4360DAE9A95299BB9D6",       "class", "pommel" },
    { "GripModule_18_F4DF51EB4E742195B8C6BAB17E4C5DB4",         "class", "grip" },
    { "HeadSize_21_2D425E61473B8F64FBAB51B223459D57",           "vec", "head_size" },
    { "GuardSize_23_5A1AA0E04708E86FEFF61E974DDA8704",          "vec", "guard_size" },
    { "GripSize_25_AC1660814C4C25C521AAA8830FE8ECCF",           "vec", "grip_size" },
    { "PommelSize_27_660CC00C49C26D503E16B2BC58CE115E",         "vec", "pommel_size" },
    { "CustomMassScaleHead_30_B95872A242AD944E2CE4D493F718F9D7",   "num", "mass_head" },
    { "CustomMassScaleGuard_51_3A9024E74306B7BB5D186087011D1927",  "num", "mass_guard" },
    { "CustomMassScaleGrip_32_0EAADEE0419C05C6DB38F0AE134A9B10",   "num", "mass_grip" },
    { "CustomMassScalePommel_34_0AB28D814BDEF17D408D0DAA3A453173", "num", "mass_pommel" },
    { "MaterialMetalSteel_37_AB7A28C94B176CF81A6C8BA34AC57C36", "int", "mat_steel" },
    { "MaterialMetalColored_39_DC2EAC244758A8D82855CC940784A1D2", "int", "mat_colored" },
    { "MaterialWeood_41_E0B3C8DB48943B878AEFA3AB01E7B99A",      "int", "mat_wood" },
    { "MaterialLeather_43_41D1114148FDB4FE4DACC8A2F4CA9FEB",    "int", "mat_leather" },
    { "ColorWood_46_F3AE05AD4495EBCD1D354C8025D7C743",          "color", "color_wood" },
    { "ColorLeather_48_DC45F07E4C0C3280278212A7158EE638",       "color", "color_leather" },
    { "Price_60_83FE5A624EA188485BBE4E9C8606AEE5",              "num", "price" },
    { "Tier_67_05026E6F43B7300AA8BACC9D9F9AB461",               "int", "tier" },
}

local CP_EQUIPMENT  = "Equipment_26_741A2FC641801842FE691295645C604F"
local EQ_ARMOR_IN   = "ArmorinSlots_5_BD7AC6CB43FBB2FDB943E7864486F358"

-- --- records (schema loadout.rs; the native module marshals these tables) -------
-- The appearance travels as one typed record ("loadout": LoadoutHead + ArmorRow
-- rows): passports are tables keyed by the record field names above (third
-- element of ARMOR_FIELDS / WEAPON_FIELDS). Code tables: S.ENUMS.loadout_row /
-- loadout_flag.
-- MAX_ROWS = schema LOADOUT_MAX_ROWS, CLASS_MAX = schema CLASS_PATH (Str<128>).
local REC = { PIECE = 1, PASSPORT = 2, HAS_R = 1, HAS_L = 2, MAX_ROWS = 48, CLASS_MAX = 128 }

-- Deterministic text of a value (sorted keys): change detection and apply keys.
local function sig(v)
    local t = type(v)
    if t == "number" then
        if v == math.floor(v) and math.abs(v) < 1e15 then return string.format("%d", v) end
        return string.format("%.4f", v)
    elseif t ~= "table" then return tostring(v) end
    local keys = {}
    for k in pairs(v) do keys[#keys + 1] = k end
    table.sort(keys, function(a, b) return tostring(a) < tostring(b) end)
    local parts = {}
    for i, k in ipairs(keys) do parts[i] = tostring(k) .. "=" .. sig(v[k]) end
    return "{" .. table.concat(parts, ",") .. "}"
end

local function ipc() return rawget(_G, "HSMP_IPC") end

-- --- class path compression + resolution -------------------------------------
-- "/Game/Assets/Armor/X/BP_A.BP_A_C" <-> "@Armor/X/BP_A"

local function class_path(cls)
    if not cls or not cls.IsValid or not cls:IsValid() then return "" end
    local full; pcall(function() full = cls:GetFullName() end)
    local path = full and full:match("^%S+%s+(.+)$")
    if not path then return "" end
    local pkg, name = path:match("^(.+)%.([^%./]+)_C$")
    if pkg and pkg:sub(-(#name + 1)) == "/" .. name then
        path = pkg
    end
    return (path:gsub("^/Game/Assets/", "@"))
end

local function expand_path(short)
    if not short or short == "" then return nil end
    local p = short:gsub("^@", "/Game/Assets/")
    if not p:find("%.") then
        local name = p:match("([^/]+)$")
        p = p .. "." .. name .. "_C"
    end
    return p
end

local class_cache = {}
local function resolve_class(short)
    local path = expand_path(short)
    if not path then return nil end
    local c = class_cache[path]
    if c and c:IsValid() then return c end
    c = StaticFindObject(path)
    if not (c and c:IsValid()) then
        -- Not loaded on this client yet (nobody here wore it): load the asset.
        pcall(function() LoadAsset((path:gsub("_C$", ""))) end)
        c = StaticFindObject(path)
    end
    if c and c:IsValid() then
        class_cache[path] = c
        return c
    end
    Log("cannot resolve class %s", path)
    return nil
end

-- --- struct <-> array --------------------------------------------------------

local function r3(x) return math.floor((tonumber(x) or 0) * 1000 + 0.5) / 1000 end

-- A passport UStruct as its record table (ArmorRow / WeaponPass fields).
local function enc_struct(s, fields, exact)
    local round = exact and function(value) return tonumber(value) or 0 end or r3
    local t = {}
    for _, fd in ipairs(fields) do
        local name, kind = fd[1], fd[2]
        local v; pcall(function() v = s[name] end)
        local e
        if kind == "class" then e = class_path(v)
        elseif kind == "int" or kind == "num" then e = tonumber(v) or 0
        elseif kind == "bool" then e = (v == true)
        elseif kind == "color" then
            e = { 0, 0, 0, 1 }
            pcall(function() e = { round(v.R), round(v.G), round(v.B), round(v.A) } end)
        elseif kind == "vec" then
            e = { 0, 0, 0 }
            pcall(function() e = { round(v.X), round(v.Y), round(v.Z) } end)
        elseif kind == "name" then
            e = ""
            pcall(function() e = v:ToString() end)
        end
        t[fd[3]] = e
    end
    return t
end

-- A record table (ArmorRow / WeaponPass) as the UStruct value the BP functions take.
local function dec_struct(rec, fields)
    local t = {}
    for _, fd in ipairs(fields) do
        local name, kind, v = fd[1], fd[2], rec[fd[3]]
        if kind == "class" then
            local c = resolve_class(v)
            if c then t[name] = c end
        elseif kind == "int" or kind == "num" then t[name] = tonumber(v) or 0
        elseif kind == "bool" then t[name] = (v == 1 or v == true)
        elseif kind == "color" and type(v) == "table" then
            t[name] = { R = v[1] or 0, G = v[2] or 0, B = v[3] or 0, A = v[4] or 1 }
        elseif kind == "vec" and type(v) == "table" then
            t[name] = { X = v[1] or 0, Y = v[2] or 0, Z = v[3] or 0 }
        elseif kind == "name" and type(v) == "string" then
            t[name] = FName(v)
        end
    end
    return t
end

-- A copy of a passport table (colour / size arrays copied too).
local function copy_pass(p)
    local c = {}
    for k, v in pairs(p) do
        if type(v) == "table" then local a = {}; for i, x in ipairs(v) do a[i] = x end; c[k] = a else c[k] = v end
    end
    return c
end

-- --- world / pawn helpers ----------------------------------------------------

local function valid(o)
    local ok = false
    pcall(function() ok = o ~= nil and o:IsValid() end)
    return ok
end

local function cls_name(o)
    local n = "?"
    pcall(function() n = o:GetClass():GetFName():ToString() end)
    return n
end

local function obj_name(o)
    local n = "?"
    pcall(function() n = o:GetFName():ToString() end)
    return n
end

-- One PlayerController walk (UEHelpers.GetPlayerController is a full
-- FindAllOf) per game frame (os.clock ms) and world generation; every call
-- of the same frame shares it. Never kept beyond the frame / a LoadMap.
local PCF = { at = nil, gen = nil, pc = nil }
function PCF.get()
    local now = os.clock()
    if PCF.at == now and PCF.gen == world_gen then return PCF.pc end
    local pc; pcall(function() pc = UEHelpers.GetPlayerController() end)
    PCF.at, PCF.gen, PCF.pc = now, world_gen, pc
    return pc
end
function PCF.world()
    local w
    pcall(function()
        local pc = PCF.get()
        if pc and pc:IsValid() then w = pc:GetWorld() end
        if not (w and w:IsValid()) then w = UEHelpers.GetWorld() end
    end)
    return w
end

local function in_arena()
    local ok = false
    pcall(function()
        local w = PCF.world()
        ok = w and w:IsValid() and w:GetFullName():find("Map_Arena_") ~= nil
    end)
    return ok
end

function PCF.fighter_context()
    local i = rawget(_G, "HSMP_IPC")
    if not (i and i.rec and i.bus_table and Kit.fighter_context) then return nil end
    local w = PCF.world()
    if not (w and w:IsValid()) then return nil end
    local world = tostring(world_gen) .. "|" .. tostring(w:GetAddress()) .. "@" .. w:GetFullName()
    return Kit.fighter_context(i.rec("session"), i.rec("link"), i.rec("mode"), i.bus_table("spawn_status"), world)
end
function PCF.describe(pawn)
    local m
    pcall(function()
        if not (pawn and pawn:IsValid()) then return end
        local w = pawn:GetWorld()
        if not (w and w:IsValid()) then return end
        m = { address = tostring(pawn:GetAddress()), name = pawn:GetFName():ToString(),
            world = tostring(world_gen) .. "|" .. tostring(w:GetAddress()) .. "@" .. w:GetFullName() }
    end)
    return m
end
function PCF.resolve(name, address)
    if not (PCF.settled and PCF.settled()) then return nil end
    for _, pawn in pairs(FindAllOf("Willie_BP_C") or {}) do
        local m = PCF.describe(pawn)
        if m and m.name == name and m.address == address then return pawn end
    end
end
local function local_pawn()
    local pc = PCF.get()
    if not pc or not pc:IsValid() then return nil end
    local owned = PCF.aip and PCF.aip.ai_pawn_lookup and PCF.aip.ai_pawn_lookup()
    local p = owned or pc.Pawn
    if not (p and p:IsValid()) then p = nil end
    if not Kit.local_fighter then return p end
    local i = rawget(_G, "HSMP_IPC")
    local swap = i and i.bus_table and i.bus_table("fallback_swap")
    local context = PCF.fighter_context()
    p, PCF.bound = Kit.local_fighter(p, context, PCF.bound, swap, os.clock(), PCF.describe, PCF.resolve)
    return p
end

local function busy(w)
    local b = false
    pcall(function() b = (w["Setup Armor in Process"] == true) or (w["Armor Swap In Process"] == true) end)
    return b
end

local function tarray_num(a)
    local n = 0
    pcall(function() n = a:GetArrayNum() end)
    return n
end

local function arr_each(a, fn)
    if not a then return end
    pcall(function() a:ForEach(function(_, e) fn(e:get()) end) end)
end

local function map_each(m, fn)
    if not m then return end
    pcall(function() m:ForEach(function(k, v) fn(k:get(), v:get()) end) end)
end

-- Two Lua handles to one UObject are not guaranteed to be ==; compare addresses.
local function same(a, b)
    if a == nil or b == nil then return false end
    local ok, r = pcall(function() return a:GetAddress() == b:GetAddress() end)
    if ok then return r end
    return a == b
end

local function field(o, name)
    local v; pcall(function() v = o[name] end)
    return v
end

-- Call a Blueprint function by its real name. Half Sword's BP functions have
-- spaces ("Set Up Armor"); `obj:SetUpArmor()` resolves to nothing and the
-- call silently does nothing.
local function bp_call(obj, fname, ...)
    local fn = obj[fname]
    if not fn then error("no BP function '" .. fname .. "'") end
    return fn(obj, ...)
end

-- --- gear sources --------------------------------------------------------------
-- The game keeps armour in several places depending on which code path dressed
-- the character. Each reader returns a list of {slot, classpath} pieces; the
-- writer sends the richest list, cross-checked against WornArmor (the meshes
-- actually on the body).

local NEW_SLOT_PIECES = "ArmorPiecesInSlot_5_437E5B6C48558F4524ABF1811C892DF8"
local LE_ARMOR        = "Armor_84_A1BA4DD44FD262BCA53B9DACF03CDF04"
local LE_WEAPONS      = "Weapons_83_06F076E247B54D0D9942B383323C1968"
local LA_IN_SLOTS     = "ArmorinSlots_31_702A9C5C40C7F4335C6B4687EC09936A"
local AE_CLASS        = "ArmorBPClass_2_0A22459840BF9E6989DFA4BA6CFED1D3"

local function add_piece(list, seen, slot, cls)
    local p = class_path(cls)
    if p == "" then return end
    local k = tostring(slot) .. "|" .. p
    if seen[k] then return end
    seen[k] = true
    list[#list + 1] = { tonumber(slot) or 0, p }
end

local SOURCES = {
    -- What "Set Up Armor" actually put on (its output map): the truth.
    { "equipped", function(w, out, seen)
        map_each(field(w, "Currently Equipped Armor"), function(slot, pass)
            add_piece(out, seen, slot, field(pass, "ArmorCore_3_F6B7C69C4BD7D9720DB91EB635EE2B43"))
        end)
    end },
    { "new-slots", function(w, out, seen)
        map_each(field(w, "NewArmorSlots"), function(slot, contents)
            arr_each(field(contents, NEW_SLOT_PIECES), function(cls) add_piece(out, seen, slot, cls) end)
        end)
    end },
    { "game-armor", function(w, out, seen)
        arr_each(field(w, "EquippedArmor"), function(ga)
            if valid(ga) then add_piece(out, seen, field(ga, "ArmorSlot"), field(ga, "SkeletalMesh")) end
        end)
    end },
    { "external-slots", function(w, out, seen)
        if field(w, "Use External Armor Slots") ~= true then return end
        map_each(field(w, "ExternalArmorSlots"), function(slot, cls) add_piece(out, seen, slot, cls) end)
    end },
    { "armor-slots", function(w, out, seen)
        map_each(field(w, "ArmorSlots"), function(slot, cls) add_piece(out, seen, slot, cls) end)
    end },
    { "load-equipment", function(w, out, seen)
        local le = field(w, "Load Equipment")
        local la = le and field(le, LE_ARMOR)
        map_each(la and field(la, LA_IN_SLOTS), function(slot, el)
            add_piece(out, seen, slot, field(el, AE_CLASS))
        end)
    end },
}

local function worn_count(w)
    return tarray_num(field(w, "Worn Armor"))
end

-- Pick the source that best explains what is actually worn: the largest list,
-- ties broken by SOURCES order. Returns pieces, source name, and all counts.
local function read_pieces(w)
    local best, best_src, counts = {}, "none", {}
    for _, src in ipairs(SOURCES) do
        local out, seen = {}, {}
        pcall(src[2], w, out, seen)
        counts[#counts + 1] = src[1] .. "=" .. #out
        -- "equipped" (Set Up Armor's own output) is authoritative when present.
        if #out > #best and best_src ~= "equipped" then best, best_src = out, src[1] end
    end
    table.sort(best, function(x, y)
        if x[1] ~= y[1] then return x[1] < y[1] end
        return x[2] < y[2]
    end)
    return best, best_src, table.concat(counts, " ")
end

-- Pieces from one named source only (kit.lua verification).
local function read_source(w, name)
    for _, src in ipairs(SOURCES) do
        if src[1] == name then
            local out, seen = {}, {}
            pcall(src[2], w, out, seen)
            return out
        end
    end
    return {}
end

-- Passports carry the colours/materials; CurrentlyEquippedArmor first, else
-- the collision-mesh passport map (one entry per worn piece).
local function read_passports(w)
    local out = {}
    map_each(field(w, "Currently Equipped Armor"), function(slot, pass)
        out[#out + 1] = { tonumber(slot) or 0, enc_struct(pass, ARMOR_FIELDS) }
    end)
    if #out > 0 then return out, "equipped" end
    local seen = {}
    map_each(field(w, "Armor Passports related to Armor Collision Meshes"), function(_, pass)
        local enc = enc_struct(pass, ARMOR_FIELDS)
        local key = tostring(enc.class) .. "#" .. tostring(enc.id)
        if not seen[key] then
            seen[key] = true
            out[#out + 1] = { enc.pslot or 0, enc }   -- the passport's own Slot field
        end
    end)
    if #out > 0 then return out, "collision-map" end
    return out, "none"
end

-- Held weapons: the hand fields and their *_0 twins. The
-- FindAllOf("ModularWeaponBP_C") fallback (any weapon whose ParentActor is
-- this Willie and that reports IsHeld) is used only by the Ctrl+F7 dump
-- (scan = true): on every 2 s write of a one-handed kit it would run at a
-- random point of a round reload's crash window.
local function held_weapons(w, scan)
    local R, L
    for _, f in ipairs({ "Weapon R", "Weapon R_0" }) do
        if not R then local x = field(w, f); if valid(x) then R = x end end
    end
    for _, f in ipairs({ "Weapon L", "Weapon L_0" }) do
        if not L then local x = field(w, f); if valid(x) then L = x end end
    end
    if (R and L) or not scan then return R, L end
    pcall(function()
        for _, cn in ipairs({ "ModularWeaponBP_C" }) do
            for _, x in pairs(FindAllOf(cn) or {}) do
                if valid(x) and not same(x, R) and not same(x, L) and field(x, "Is Held") == true
                    and same(field(x, "Parent Actor"), w) then
                    local sock = ""
                    pcall(function() sock = x:GetAttachParentSocketName():ToString():lower() end)
                    if (sock:find("_l") or sock:find("left")) and not L then L = x
                    elseif not R then R = x end
                end
            end
        end
    end)
    return R, L
end

local function weapon_entry(x)
    if not valid(x) then return nil end
    local pass = field(x, "Weapon Passport")
    local arr = pass and enc_struct(pass, WEAPON_FIELDS, true) or nil
    -- The passport's own class can be empty for level-placed weapons; fall back
    -- to the actor's class so the stand-in still gets the right weapon.
    if arr and (arr.class == nil or arr.class == "") then
        local c; pcall(function() c = x:GetClass() end)
        arr.class = class_path(c)
    end
    return arr
end

-- --- diagnostics (one-shot per arena + Ctrl+F7) -------------------------------

local function dump_gear(w, label)
    if not valid(w) then Log("[gear] %s: invalid", label); return end
    Log("[gear] ===== %s: %s (%s) HP=%s busy=%s useExternal=%s =====",
        label, obj_name(w), cls_name(w), tostring(field(w, "Health")),
        tostring(busy(w)), tostring(field(w, "Use External Armor Slots")))
    for _, f in ipairs({ "Weapon R", "Weapon L", "Weapon R_0", "Weapon L_0", "Weapon Slot R 1", "Weapon Slot R 2",
                         "Weapon Slot L 1", "Weapon Slot L 2", "Weapon Slot Back", "Carried Armor R Hand", "Carried Armor L Hand" }) do
        local x = field(w, f)
        if valid(x) then Log("[gear]   %-16s %s (%s)", f, obj_name(x), cls_name(x)) end
    end
    local _, _, counts = read_pieces(w)
    Log("[gear]   armour sources: %s | WornArmor=%d WornArmorItems=%d",
        counts, worn_count(w), tarray_num(field(w, "Worn Armor Items")))
    for _, src in ipairs(SOURCES) do
        local out, seen = {}, {}
        pcall(src[2], w, out, seen)
        for _, p in ipairs(out) do Log("[gear]     %s slot=%d %s", src[1], p[1], p[2]) end
    end
    arr_each(field(w, "Worn Armor"), function(c)
        if valid(c) then
            local asset = "?"
            -- A null asset comes back as an invalid wrapper; GetFullName on
            -- it reads address 0x18 and kills the game (pcall cannot catch it).
            pcall(function()
                local sa = c:GetSkinnedAsset()
                if sa and sa:IsValid() then asset = sa:GetFullName() end
            end)
            Log("[gear]     worn mesh %s -> %s", obj_name(c), asset)
        end
    end)
    arr_each(field(w, "Worn Armor Items"), function(n)
        local s = "?"; pcall(function() s = n:ToString() end)
        Log("[gear]     worn item %s", s)
    end)
    local pass, psrc = read_passports(w)
    Log("[gear]   passports: %d (%s)", #pass, psrc)
    for _, e in ipairs(pass) do
        local a = e[2]
        Log("[gear]     passport key=%s core=%s id=%s removed=%s mods=%s/%s/%s slot=%s steel=%s metal=%s AP=%s%s%s%s tier=%s fabric1=%s",
            tostring(e[1]), tostring(a.class), tostring(a.id), tostring(a.core_removed), tostring(a.module1), tostring(a.module2),
            tostring(a.module3), tostring(a.pslot), tostring(a.steel), tostring(a.metal), tostring(a.up_ap), tostring(a.low_ap),
            tostring(a.req_up_ap), tostring(a.req_low_ap), tostring(a.tier),
            type(a.fabric1) == "table" and table.concat(a.fabric1, ",") or "?")
    end
    local R, L = held_weapons(w, true)
    local function wdesc(x)
        if not x then return "-" end
        local e = weapon_entry(x)
        return obj_name(x) .. " " .. tostring(e and e.class or "?")
    end
    Log("[gear]   held R=%s L=%s", wdesc(R), wdesc(L))
    local n = 0
    pcall(function()
        for _, x in pairs(FindAllOf("ModularWeaponBP_C") or {}) do
            if valid(x) and same(field(x, "Parent Actor"), w) then
                n = n + 1
                Log("[gear]     weapon with ParentActor=me: %s (%s) held=%s", obj_name(x), cls_name(x),
                    tostring(field(x, "Is Held")))
            end
        end
    end)
    Log("[gear]   ModularWeaponBP_C scan matched %d", n)
end

-- --- writer ------------------------------------------------------------------

local function read_local_loadout(pawn)
    local pieces, psrc = read_pieces(pawn)
    local pass, passsrc = read_passports(pawn)
    local R, L = held_weapons(pawn)
    local wpn = {}
    wpn.R = weapon_entry(R)
    wpn.L = weapon_entry(L)
    return { p = pieces, a = pass, w = wpn, src = psrc .. "/" .. passsrc, worn = worn_count(pawn) }
end

-- The `loadout` record of L (version v): both hands in the head; one row per worn piece,
-- merged with its passport when slot and class agree, and one row per passport without a
-- piece. Over the row capacity, passport-only rows go first (they are only colours).
-- Returns the record table and the number of rows dropped.
local function loadout_record(L, v)
    local warned_long = REC.warned or {}
    REC.warned = warned_long
    local rows, by = {}, {}
    local function fits(path)
        if #tostring(path) < REC.CLASS_MAX then return true end
        if not warned_long[path] then
            warned_long[path] = true
            Log("WARNING: class path over %d bytes not sent: %s", REC.CLASS_MAX - 1, tostring(path))
        end
        return false
    end
    for _, pc in ipairs(L.p) do
        if fits(pc[2]) then
            local r = { slot = pc[1], class = pc[2], flags = REC.PIECE }
            rows[#rows + 1] = r
            by[tostring(pc[1]) .. "|" .. pc[2]] = r
        end
    end
    local alone = {}
    for _, e in ipairs(L.a) do
        local pass = e[2]
        if type(pass) == "table" and pass.class and pass.class ~= "" and fits(pass.class) then
            local r = by[tostring(e[1]) .. "|" .. pass.class]
            if r and r.flags == REC.PIECE then
                for k, x in pairs(pass) do r[k] = x end
                r.flags = REC.PIECE | REC.PASSPORT
            else
                r = copy_pass(pass)
                r.slot, r.flags = e[1], REC.PASSPORT
                alone[#alone + 1] = r
            end
        end
    end
    local dropped = 0
    for _, r in ipairs(alone) do
        if #rows < REC.MAX_ROWS then rows[#rows + 1] = r else dropped = dropped + 1 end
    end
    while #rows > REC.MAX_ROWS do rows[#rows] = nil; dropped = dropped + 1 end
    local rec = { version = v, flags = 0, rows = rows }
    if L.w.R then rec.r, rec.flags = L.w.R, rec.flags | REC.HAS_R end
    if L.w.L then rec.l, rec.flags = L.w.L, rec.flags | REC.HAS_L end
    return rec, dropped
end

local last_content, version = nil, 0
local function next_version()
    local base = (os.time() - 1700000000) * 16
    version = math.max(version + 1, base)
    return version
end

local busy_since = nil
local last_heartbeat = 0
local function write_local()
    if not in_arena() then return end
    local pawn = local_pawn()
    if not pawn then return end
    -- Don't read mid armour-swap, but never let a stuck flag block forever.
    if busy(pawn) then
        busy_since = busy_since or os.time()
        if os.time() - busy_since < 6 then return end
    else
        busy_since = nil
    end
    local L = read_local_loadout(pawn)
    local content = sig({ p = L.p, a = L.a }) .. "|" .. Kit.weapon_sig(L)
    local now = os.time()
    if content == last_content then
        if now - last_heartbeat >= 30 then
            last_heartbeat = now
            Log("local loadout unchanged v=%d pieces=%d worn=%d src=%s R=%s L=%s",
                version, #L.p, L.worn, L.src,
                L.w.R and L.w.R.class or "-", L.w.L and L.w.L.class or "-")
        end
        return
    end
    last_content = content
    last_heartbeat = now
    local v = next_version()
    local rec, dropped = loadout_record(L, v)
    local I = ipc()
    local ok, err = false, "no ipc"
    if I then ok, err = I.put("loadout", rec) end
    Log("local loadout v=%d pieces=%d worn=%d passports=%d rows=%d%s src=%s R=%s L=%s%s",
        v, #L.p, L.worn, #L.a, #rec.rows, dropped > 0 and string.format(" (dropped %d)", dropped) or "", L.src,
        L.w.R and L.w.R.class or "-", L.w.L and L.w.L.class or "-", ok and "" or (" NOT WRITTEN: " .. tostring(err)))
    if #L.p == 0 and L.worn > 0 then
        Log("WARNING: %d armour meshes worn but no class source found — press Ctrl+F7 and send the [gear] dump", L.worn)
    end
end

-- --- applier -----------------------------------------------------------------

local applied = {}   -- peer_id -> armour key "<kit/rules>|<armour sig>@puppetName" once applied OK
local tries   = {}   -- peer_id -> { key, n, next_at }
-- Stand-in state beyond the armour key (one table: the main chunk is near
-- Lua's 200-locals limit). World-scoped (reset by drop_world_caches):
--   aw[id]    weapon key applied (hands keyed apart from the armour)
--   strip[id] { v = loadout version, R = true, L = true }: hands emptied
--             because a dynamic world item from that peer appeared
--   dyn_seen  manifest entry id -> true (nil until the world's first read)
--   dyn_raw   last world_dyn record version
--   hand      own hands' address signature (publish on change)
local SI = { aw = {}, strip = {}, dyn_seen = nil, dyn_raw = nil, hand = nil }
function SI.reset() SI.aw, SI.strip, SI.dyn_seen, SI.dyn_raw, SI.hand = {}, {}, nil, nil, nil end
SI.weapon_equal = (function()
    local ok, value = pcall(require, 'weapon_passport_equal')
    if ok and type(value) == 'table' then return value end
    local source = (debug.getinfo(1, 'S').source or ''):gsub('^@', '')
    local directory = source:match('^(.*)[/\\]') or '.'
    local loaded, module = pcall(dofile, directory .. '/weapon_passport_equal.lua')
    if loaded and type(module) == 'table' then return module end
    Log('exact Weapon Passport helper unavailable: %s', tostring(module))
end)()
Kit.weapon_passport_key = function(record)
    return SI.weapon_equal and SI.weapon_equal.signature(record, WEAPON_FIELDS) or "unavailable"
end

-- HSMPAvatars' stand-ins (bus key "puppets", typed rows {peer, name}) as
-- { ["<peer id>"] = "<Willie FName>" }; nil = never written.
local function puppets()
    local I = ipc()
    local t = I and I.bus_table and I.bus_table("puppets")
    if type(t) ~= "table" then return nil end
    local m = {}
    for _, r in ipairs(t.rows or {}) do
        if r.peer and r.peer ~= 0 and type(r.name) == "string" and r.name ~= "" then m[tostring(r.peer)] = r.name end
    end
    return m
end

-- A peer's appearance (per-peer blob "peer_loadout", one `loadout` record) in the
-- applier's shape: { v, p = { {slot, class} }, a = { {slot, passport} }, w = { R, L } }.
-- The rows and hand passports are the record's own tables (the facade caches one table
-- per version); nil when none arrived.
local lo_cache = {}   -- peer id -> { rec, L }
local function remote_loadout(id)
    local I = ipc()
    local t = I and I.peer_rec and I.peer_rec("peer_loadout", tonumber(id))
    if type(t) ~= "table" or not t.version then return nil end
    local c = lo_cache[id]
    if c and c.rec == t then return c.L end
    local L = { v = t.version, p = {}, a = {}, w = {}, exact_armour = true }
    for _, r in ipairs(t.rows or {}) do
        local f = tonumber(r.flags) or 0
        if f & REC.PIECE ~= 0 then L.p[#L.p + 1] = { r.slot, r.class } end
        if f & REC.PASSPORT ~= 0 then L.a[#L.a + 1] = { r.slot, r } end
    end
    local hf = tonumber(t.flags) or 0
    if hf & REC.HAS_R ~= 0 then L.w.R = t.r end
    if hf & REC.HAS_L ~= 0 then L.w.L = t.l end
    lo_cache[id] = { rec = t, L = L }
    return L
end

local function find_willie(name)
    local found
    pcall(function()
        for _, w in pairs(FindAllOf("Willie_BP_C") or {}) do
            if w and w:IsValid() and w:GetFName():ToString() == name then found = w; return end
        end
    end)
    return found
end

local function set_class_map(map, pieces)
    map:Empty()
    local seen = {}
    for _, e in ipairs(pieces) do
        if not seen[e[1]] then
            local c = resolve_class(e[2])
            if c then map:Add(e[1], c); seen[e[1]] = true end
        end
    end
end

-- --- how the game really dresses a Willie ---------------------------------------
-- (Read from the Willie_BP bytecode of Set Up Armor.)
-- "Set Up Armor"(Clear Previous, No Check Block) spawns one armour actor per
-- entry of the pawn's own "Character Passport".Equipment.ArmorinSlots map
-- (ArmorSlots_Enum -> Str_Passport_Armor1: ArmorCore class + modules +
-- colours + slot) and writes what it accepted to "Currently Equipped Armor"
-- (an output, cleared on every call). ArmorSlots / ExternalArmorSlots /
-- NewArmorSlots are not read at all. On top of that it only spawns a piece if
--   (slot == 16 legs  or  not "Spawn in Pants")  and
--   (slot in 12/15/16 or  not "Blossfechten Gear")
-- and the arena spawns players and fresh foes with "Spawn in Pants" = true,
-- so left alone everybody stays in hosen.
-- So: write the pawn's passport map, clear those two per-pawn flags, call
-- "Set Up Armor". Nothing touches the GI or the save.
--
-- Passport contents come from the game's own loadout data assets
-- (DA_Equipment_Loadout_Tier_*: ArmorinSlots, real modules/colours/steel);
-- an item no tier loadout uses gets a synthesised passport (core only).

local TIER_DAS = { "Beggar", "Peasant", "Commoner", "Militia", "Soldier", "ManAtArms", "Veteran", "Knight" }
local pass_tmpl, pass_tmpl_n = nil, 0

local function harvest_templates()
    if pass_tmpl then return pass_tmpl end
    local t, nda = {}, 0
    for _, tier in ipairs(TIER_DAS) do
        local pkg = "/Game/Blueprints/DataAssets/Equipment/Loadout/Tiers/DA_Equipment_Loadout_Tier_" .. tier
        local full = pkg .. ".DA_Equipment_Loadout_Tier_" .. tier
        local da
        pcall(function() da = StaticFindObject(full) end)
        if not valid(da) then
            pcall(function() LoadAsset(pkg) end)
            pcall(function() da = StaticFindObject(full) end)
        end
        if valid(da) then
            nda = nda + 1
            local lo = field(da, "Loadout")
            map_each(lo and field(lo, EQ_ARMOR_IN), function(_, pass)
                local enc = enc_struct(pass, ARMOR_FIELDS)
                if enc.class and enc.class ~= "" and not t[enc.class] then
                    t[enc.class] = enc
                    pass_tmpl_n = pass_tmpl_n + 1
                end
            end)
        end
    end
    -- Only cache a successful harvest; retry later otherwise.
    if nda > 0 then pass_tmpl = t end
    Log("armour passport templates: %d classes from %d/%d tier loadouts", pass_tmpl_n, nda, #TIER_DAS)
    return t
end

local DEFAULT_PASSPORT_COLOURS = {
    { 0, 0, 0, 1 },                 -- background
    { 0.22, 0.13, 0.07, 1 },        -- leather
    { 0.42, 0.37, 0.29, 1 },        -- fabric 1
    { 0.30, 0.26, 0.20, 1 },        -- fabric 2
    { 0.20, 0.17, 0.13, 1 },        -- fabric 3
}

-- Encoded passport (ARMOR_FIELDS order) for armour class `path` in `slot`.
local function make_passport(slot, path, tint)
    local tm = harvest_templates()[path]
    local arr
    if tm then
        arr = copy_pass(tm)
    else
        local C = DEFAULT_PASSPORT_COLOURS
        local function col(i) return { C[i][1], C[i][2], C[i][3], C[i][4] } end
        arr = { class = path, id = 0, core_removed = false, module1 = 0, module2 = 0, module3 = 0,
                bg_color = col(1), leather_color = col(2), fabric1 = col(3), fabric2 = col(4), fabric3 = col(5),
                steel = 0, metal = 0, rust = false, dirt = false, price = 0, pslot = slot,
                up_ap = false, low_ap = false, req_up_ap = false, req_low_ap = false, req_hier = false, tier = 0 }
        -- Arming-point flags from the armour class defaults.
        local cls = resolve_class(path)
        local cdo; pcall(function() cdo = cls:GetCDO() end)
        if valid(cdo) then
            local function b(n) local v; pcall(function() v = cdo[n] end); return v == true end
            arr.up_ap = b("Unlocks Upper Arming Points")
            arr.low_ap = b("Unlocks Lower Arming Points")
            arr.req_up_ap = b("Requires Upper Arming Points")
            arr.req_low_ap = b("Requires Lower Arming Points")
        end
    end
    arr.class, arr.core_removed, arr.pslot = path, false, slot
    if tint then
        local col, dark = tint[1], tint[2]
        arr.leather_color, arr.fabric1, arr.fabric2, arr.fabric3 = dark, col, dark, col
    end
    return arr, tm ~= nil
end

-- Native passports a pawn had before we first dressed it (underwear / base
-- clothing), keyed by world generation + pawn address. Only entries whose
-- class is not a selectable catalogue item are kept, so a foe's own armour
-- never leaks onto a stand-in.
local base_pass = {}

local function pawn_key(w)
    local a = "?"; pcall(function() a = tostring(w:GetAddress()) end)
    return tostring(world_gen_ref()) .. "|" .. a
end

local function passport_list(m)
    local out = {}
    map_each(m, function(slot, pass)
        out[#out + 1] = { tonumber(slot) or 0, enc_struct(pass, ARMOR_FIELDS) }
    end)
    return out
end

-- What the game actually built ("Set Up Armor" output).
local function current_passports(w)
    return passport_list(field(w, "Currently Equipped Armor"))
end

-- The pawn's own character passport armour map ("Set Up Armor" input).
local function passport_armour_map(w)
    local cp = field(w, "Character Passport")
    local eq = cp and field(cp, CP_EQUIPMENT)
    return eq and field(eq, EQ_ARMOR_IN)
end

-- Pieces that unlock arming points go first: "Set Up Armor" processes the
-- passport map in insertion order and a piece that requires arming points is
-- only accepted once a provider is on.
local function unlocks_ap(path)
    local cls = resolve_class(path)
    local cdo; pcall(function() cdo = cls:GetCDO() end)
    if not valid(cdo) then return false end
    local u, l
    pcall(function() u = cdo["Unlocks Upper Arming Points"]; l = cdo["Unlocks Lower Arming Points"] end)
    return u == true or l == true
end

-- IO-1 (docs/development/halfsword/io-dispatcher-crash.md). In the first arena
-- of a process, a Willie hidden from BeginPlay and shown later makes the
-- engine read past the end of Hair_M_SideSweptFringe.ubulk (IoDispatcher
-- crash, ~60% of such loads in single-player). Letting the groom render for
-- HAIR_WARM_S before hiding it avoids that, so in the first arena world the
-- BeginPlay hide is delayed (the pawn shows undressed that long). Later worlds
-- hide at once. HSMP_HAIR_WARM_S=0 restores the old hide (A/B runs only).
local HAIR_WARM_S = tonumber(trim(os.getenv("HSMP_HAIR_WARM_S")) or "") or 0.5
local hair_warm_done = false     -- a groom rendered unhidden for HAIR_WARM_S in this process
local hair_warmed_here = false   -- ... in the current world
local hair_world = 0              -- bumped only by the LoadMap pre-hook (BeginPlay runs before the post-hook)
local function hair_warm_pending() return HAIR_WARM_S > 0 and not hair_warm_done end

-- Also: a visible Willie is re-dressed only HAIR_SETTLE_S after we first saw it
-- visible (own pawn after a safety reveal, stand-ins). A hidden one at once.
-- HSMP_HAIR_SETTLE_S=0 turns that wait off.
local HAIR_SETTLE_S = tonumber(trim(os.getenv("HSMP_HAIR_SETTLE_S")) or "") or 2.5
local seen_vis = {}          -- Willie FName -> { hidden = bool, shown_t = os.clock() }

local function mark_shown(n) seen_vis[n] = { hidden = false, shown_t = os.clock() } end

-- Seconds to wait before `w` may be dressed (0 = now).
local function dress_wait(w)
    if HAIR_SETTLE_S <= 0 then return 0 end
    local n = obj_name(w)
    local hidden = false
    pcall(function() hidden = w.bHidden == true end)
    if hidden then seen_vis[n] = { hidden = true }; return 0 end
    local s = seen_vis[n]
    if not s or s.hidden then mark_shown(n); s = seen_vis[n] end
    return math.max(0, HAIR_SETTLE_S - (os.clock() - s.shown_t))
end

local function call_setup(puppet, no_check)
    -- Every "Set Up Armor" goes through here: the backstop for a caller that
    -- did not wait (the callers wait first so they keep their retry budget).
    local wait = dress_wait(puppet)
    if wait > 0 then return false, string.format("hair settle (%.1f s left)", wait) end
    -- "Spawn in Pants" (set by the arena for the player / fresh foes) makes
    -- "Set Up Armor" skip every slot except the legs; "Blossfechten Gear"
    -- limits it to body/feet/legs. Both are per-pawn display flags.
    pcall(function() puppet["Spawn in Pants"] = false end)
    pcall(function() puppet["Blossfechten Gear"] = false end)
    pcall(function() puppet["Use External Armor Slots"] = false end)
    return pcall(bp_call, puppet, "Set Up Armor", true, no_check)
end

local function apply_armour(puppet, L)
    local pieces = type(L.p) == "table" and L.p or {}
    local res = {}
    local key = pawn_key(puppet)
    if not base_pass[key] then
        -- Native base clothing (underwear etc.) the pawn was born with: kept
        -- under the kit unless the kit fills that slot. Catalogue items are
        -- never kept, so a foe's own gear can't leak onto a stand-in.
        local keep = {}
        for _, e in ipairs(passport_list(passport_armour_map(puppet))) do
            if not (Kit.is_catalog_path and Kit.is_catalog_path(e[2].class)) then keep[#keep + 1] = e end
        end
        base_pass[key] = keep
    end
    -- Owner's real passports (replicated appearance), by slot.
    local given = {}
    for _, e in ipairs(type(L.a) == "table" and L.a or {}) do
        if type(e) == "table" and type(e[2]) == "table" then given[tonumber(e[1]) or -1] = e[2] end
    end
    local want, first, later = {}, {}, {}
    local nt, ns = 0, 0
    -- Received appearances already include the owner's real base clothing.
    -- A pooled foe's non-catalogue pieces can be helmets/plate too, not just
    -- underwear; merging them makes an opponent wear gear its owner removed.
    if not L.exact_armour then
        for _, e in ipairs(base_pass[key]) do want[e[1]] = e[2] end
    end
    for _, pc in ipairs(pieces) do
        local slot, path = pc[1], pc[2]
        local g = given[slot]
        if g and g.class == path then
            want[slot] = g
        else
            local enc, from_tmpl = make_passport(slot, path, L.tint)
            if from_tmpl then nt = nt + 1 else ns = ns + 1 end
            want[slot] = enc
        end
    end
    for slot, enc in pairs(want) do
        if unlocks_ap(enc.class) then first[#first + 1] = slot else later[#later + 1] = slot end
    end
    table.sort(first); table.sort(later)
    local wrote = 0
    local passport_ok, passport_err = pcall(function()
        local m = passport_armour_map(puppet)
        m:Empty()
        for _, list in ipairs({ first, later }) do
            for _, slot in ipairs(list) do
                m:Add(slot, dec_struct(want[slot], ARMOR_FIELDS))
                wrote = wrote + 1
            end
        end
    end)
    res[#res + 1] = "passport=" .. tostring(passport_ok)
    if not passport_ok then
        return false, #pieces, table.concat(res, " ") .. " FAIL " .. tostring(passport_err)
    end
    local ok, err = call_setup(puppet, false)
    local built = #current_passports(puppet)
    local mode = "checked"
    if ok and built < wrote then
        -- The game's layering rules refused something the server-validated
        -- kit allows (arming points / slot blocking): rebuild without checks.
        -- ("Set Up Armor" removed the refused entries from the passport map.)
        local rewrite_ok, rewrite_err = pcall(function()
            local m = passport_armour_map(puppet)
            m:Empty()
            for _, list in ipairs({ first, later }) do
                for _, slot in ipairs(list) do m:Add(slot, dec_struct(want[slot], ARMOR_FIELDS)) end
            end
        end)
        if not rewrite_ok then
            return false, #pieces, table.concat(res, " ") .. " rewrite FAIL " .. tostring(rewrite_err)
        end
        ok, err = call_setup(puppet, true)
        built = #current_passports(puppet)
        mode = "nocheck"
    end
    res[#res + 1] = string.format("built=%d/%d(tmpl=%d synth=%d base=%d %s)", built, wrote, nt, ns, #base_pass[key], mode)
    res[#res + 1] = "SetUpArmor=" .. (ok and "ok" or ("FAIL " .. tostring(err)))
    return ok and built >= wrote, #pieces, table.concat(res, " ")
end

-- The pawn's own "Character Passport".Equipment.WeaponinHands (key 0 = right,
-- 1 = left). Willie_BP's ubergraph re-arms the hands from it ~0.2 s after a
-- (re)setup whenever "Spawn in Pants" is false (bytecode: Map_Find(..., 0) ->
-- "Set Up Right Hand Weapon"), so it must match the kit, or the game swaps
-- the kit weapon for fists a moment later.
local EQ_WEAPON_HANDS = "WeaponinHands_23_B3FE643741AF91A6DFE51888205C0F05"
local WP_CLASS = "WeaponClass_54_B478ECF7499977809745A3973AD678EC"

-- A modular weapon
-- (the tiered swords, falchions, maces, hafted weapons, polearms) builds its
-- blade / guard / grip / pommel from the module classes in its "Weapon
-- Passport" at BeginPlay, and the tier classes' own default passport is empty.
-- Spawned from it, the weapon is an actor in the hand with no mesh asset on
-- any module component (probed in game: every component StaticMesh = nil but
-- the 1 cm SM_Grip_Dummy). The game arms its own fighters from full passports;
-- GI_Settings keeps lists of them ("Available Weapons 1H" / "2H" / "Available
-- Shields": base ModularWeaponBP_C entries with real modules). A passport
-- without a head module takes its modules (and sizes, materials) from the
-- first of those whose head module belongs to the same weapon family; the
-- weapon class stays the kit's.
local WEAPON_FAMILIES = {
    { "Sword", "Sword_Blade" }, { "LongSword", "Sword_Blade" }, { "GreatSword", "Sword_Blade" },
    { "Falchion", "Falchion_Blade" }, { "Messer", "Langmesser" }, { "Dagger", "Sword_Blade" },
    { "Polearm", "Polearm_Head" }, { "Polearm", "PA_Head" }, { "Mace", "Mace" }, { "Hafted", "Hafted_Head" },
}
local module_templates = nil   -- head-module path -> encoded passport (GI lists, read once per process)
local function weapon_templates()
    if module_templates then return module_templates end
    local list = {}
    pcall(function()
        local gi = UEHelpers.GetGameInstance()
        if not valid(gi) then return end
        for _, key in ipairs({ "Available Weapons 1H", "Available Weapons 2H" }) do
            local arr = gi[key]
            for i = 1, #arr do
                local e
                pcall(function() e = enc_struct(arr[i], WEAPON_FIELDS, true) end)
                if e and e.head and e.head ~= "" then list[#list + 1] = e end
            end
        end
    end)
    if #list > 0 then
        module_templates = list
        Log("weapon module templates: %d full passports from the game's GI lists", #list)
    end
    return list
end
local function module_template_for(cls)
    local cn = ""; pcall(function() cn = cls:GetFName():ToString() end)
    local list = weapon_templates()
    for _, fam in ipairs(WEAPON_FAMILIES) do
        if cn:find(fam[1], 1, true) then
            for _, e in ipairs(list) do
                if tostring(e.head):find(fam[2], 1, true) then return e end
            end
        end
    end
    return nil
end

local function weapon_passport_for(cls, from_actor)
    local src = from_actor
    if not valid(src) then pcall(function() src = cls:GetCDO() end) end
    local e
    pcall(function() e = enc_struct(src["Weapon Passport"], WEAPON_FIELDS) end)
    if not e or not e.head or e.head == "" then
        local tm = module_template_for(cls)
        if tm then
            e = copy_pass(tm)
        end
    end
    local t = {}
    if e then pcall(function() t = dec_struct(e, WEAPON_FIELDS) end) end
    t[WP_CLASS] = cls
    return t
end

local function set_hand_passport(pawn, side, cls, pass)
    local key = side == "R" and 0 or 1
    return pcall(function()
        local cp = pawn["Character Passport"]
        local m = cp[CP_EQUIPMENT][EQ_WEAPON_HANDS]
        if cls then m:Add(key, pass or weapon_passport_for(cls)) else m:Remove(key) end
    end)
end

-- HSMPAvatars caches a stand-in's hand-weapon root component; every
-- destruction or native replacement of a hand weapon bumps "standin_weapons"
-- (typed record { gen }) so Avatars drops its cached pointers at once (it also
-- re-validates each use).
local weapon_gen = nil
local function bump_weapon_generation()
    local I = ipc()
    if weapon_gen == nil then
        local t = I and I.bus_table and I.bus_table("standin_weapons")
        weapon_gen = tonumber(type(t) == "table" and t.gen) or 0
    end
    weapon_gen = weapon_gen + 1
    if I and I.bus_put then pcall(I.bus_put, "standin_weapons", { gen = weapon_gen }) end
end
local function destroy_hand_weapon(a)
    pcall(function() a:K2_DestroyActor() end)   -- unsafe: ok hand weapon actor only, never a Willie
    bump_weapon_generation()
end
SI.destroy_hand_weapon = destroy_hand_weapon

local function spawn_weapon_actor(puppet, cls, pass)
    local world = PCF.world()
    local gs = UEHelpers.GetGameplayStatics()
    local loc; pcall(function() loc = puppet:K2_GetActorLocation() end)
    local t = { Translation = loc or { X = 0, Y = 0, Z = 0 },
                Rotation = { X = 0, Y = 0, Z = 0, W = 1 }, Scale3D = { X = 1, Y = 1, Z = 1 } }
    -- UE 5.4: FinishSpawningActor takes 3 params (scale method); with 2 UE4SS
    -- rejects the call and leaves a half-built actor at the origin.
    local a = gs:BeginDeferredActorSpawnFromClass(world, cls, t, 1, nil, 1)
    if not valid(a) then return nil end
    if pass then pcall(function() a["Weapon Passport"] = pass end) end
    local ok = pcall(function() gs:FinishSpawningActor(a, t, 1) end)
    if not ok then pcall(function() a:K2_DestroyActor() end); return nil end
    return a
end

local function apply_weapon(puppet, side, entry)
    if not SI.weapon_equal then return "FAIL exact Passport helper unavailable" end
    local empty_hand = entry == nil
    local fname = side == "R" and "Set Up Right Hand Weapon" or "Set Up Left Hand Weapon"
    local cur
    for _, f in ipairs(side == "R" and { "Weapon R", "Weapon R_0" } or { "Weapon L", "Weapon L_0" }) do
        if not cur then local x = field(puppet, f); if valid(x) then cur = x end end
    end
    if not entry then
        -- Fists are omitted from the appearance stream, but the native Sphere
        -- still supplies collision tags/owner identity to damage replay. Keep
        -- or construct the game's real pseudo-weapon rather than deleting it.
        entry = { class = "@Weapons/Blueprints/Built_Weapons/Weapon_Fists" }
        -- Keep the old actor valid until native hand setup subtracts its
        -- weight and destroys it through Destroy Previous=true.
    end
    local cls = resolve_class(entry.class)
    if not cls then return "noclass" end
    if cur then
        local cp = ""; pcall(function() cp = class_path(cur:GetClass()) end)
        if cp == entry.class and (empty_hand or SI.weapon_equal.matches(field(cur, "Weapon Passport"), entry, WEAPON_FIELDS, class_path)) then
            local reused = empty_hand and weapon_passport_for(cls, cur) or dec_struct(entry, WEAPON_FIELDS)
            reused[WP_CLASS] = cls
            if not set_hand_passport(puppet, side, cls, reused) then return "FAIL hand passport" end
            return "same"
        end
    end
    local pass = empty_hand and weapon_passport_for(cls) or dec_struct(entry, WEAPON_FIELDS)
    pass[WP_CLASS] = cls
    set_hand_passport(puppet, side, cls, pass)
    local ok, err = pcall(bp_call, puppet, fname, cls, nil, false, true, pass)
    if ok then
        if cur then bump_weapon_generation() end
        return "ok"
    end
    -- nil actor rejected by the reflection layer: spawn it ourselves and hand
    -- the actor to the same BP function.
    local a
    local ok2, err2 = pcall(function()
        a = spawn_weapon_actor(puppet, cls, pass)
        if not a then error("spawn failed") end
        bp_call(puppet, fname, cls, a, false, true, pass)
    end)
    if ok2 then
        if cur then bump_weapon_generation() end
        return "ok(spawned)"
    end
    if valid(a) then pcall(function() a:K2_DestroyActor() end) end
    return "FAIL " .. tostring(err) .. " / " .. tostring(err2)
end

-- HSMPWorld contract (docs/development/subsystems/world-replication.md): a hand listed in the
-- world_held bus key (a typed record: rows {peer, nid, hand 0 = R / 1 = L, actor})
-- already shows a replicated world item, so the stand-in must not get (or keep) a
-- loadout weapon there. Never destroy the world actor. Returns {R = {nid, actor}, L = ...}.
local function world_held(id)
    local ipc = rawget(_G, "HSMP_IPC")
    local t = ipc and ipc.bus_table("world_held")
    if type(t) ~= "table" then return nil end
    local e
    for _, r in ipairs(t.rows or {}) do
        if tostring(r.peer) == tostring(id) then
            e = e or {}
            e[r.hand == 1 and "L" or "R"] = { r.nid, r.actor }
        end
    end
    return e
end

local function yield_hand(puppet, side, held)
    -- The hand shows a replicated world item, so the passport must not
    -- name a loadout weapon there: "Set Up Armor" (any re-dress) re-arms the
    -- hands from Equipment.WeaponinHands ~0.2 s later, and a stale entry puts
    -- a colliding duplicate weapon next to the world item.
    set_hand_passport(puppet, side, nil)
    local cur
    for _, f in ipairs(side == "R" and { "Weapon R", "Weapon R_0" } or { "Weapon L", "Weapon L_0" }) do
        if not cur then local x = field(puppet, f); if valid(x) then cur = x end end
    end
    if not cur then return "world" end
    local n = ""; pcall(function() n = cur:GetFName():ToString() end)
    if n == tostring(held[2]) then return "world" end
    destroy_hand_weapon(cur)   -- bumps standin_weapons
    return "world(stripped)"
end

-- The hands only (a held-item change never re-runs "Set Up Armor").
-- strip = { R = true, L = true }: that hand's weapon left the owner's hand
-- (a dynamic world item appeared) and L.w still names it: show it empty.
local function apply_weapons(puppet, L, id, strip)
    local wr, wl
    local w = L.w or {}
    local wR = not (strip and strip.R) and w.R or nil
    local wL = not (strip and strip.L) and w.L or nil
    local held = id and world_held(id)
    if held and (held.R or held.L) then
        local kw = L.kit_w
        wr = held.R and yield_hand(puppet, "R", held.R)
            or (kw and Kit.give_weapon(puppet, "R", kw.R) or apply_weapon(puppet, "R", wR))
        wl = held.L and yield_hand(puppet, "L", held.L)
            or (kw and Kit.give_weapon(puppet, "L", kw.L) or apply_weapon(puppet, "L", wL))
    elseif L.kit_w then
        -- Synthesised from the validated kit (no appearance received yet).
        wr = (strip and strip.R) and apply_weapon(puppet, "R", nil) or Kit.give_weapon(puppet, "R", L.kit_w.R)
        wl = (strip and strip.L) and apply_weapon(puppet, "L", nil) or Kit.give_weapon(puppet, "L", L.kit_w.L)
    else
        wr = apply_weapon(puppet, "R", wR)
        wl = apply_weapon(puppet, "L", wL)
    end
    return not wr:find("^FAIL") and not wl:find("^FAIL"), wr, wl
end

local function apply_loadout(puppet, L, id, strip)
    local before = worn_count(puppet)
    local armour_ok, npieces, ares = apply_armour(puppet, L)
    local after = worn_count(puppet)
    local wok, wr, wl = apply_weapons(puppet, L, id, strip)
    local hair = L.cos_kit and Kit.apply_hair(puppet, L.cos_kit) or "-"
    local summary = string.format("pieces=%d %s worn %d->%d weapons R=%s L=%s hair=%s%s",
        npieces, ares, before, after, wr, wl, hair,
        L.cos_kit and (" kit=" .. tostring(L.cos_kit.class)) or "")
    return armour_ok and wok, summary
end

-- --- invisible dressing ---------------------------------------------------------
-- In an MP arena every new Willie is hidden at BeginPlay and revealed only
-- once its kit is on and verified (own pawn: kit.lua; stand-ins: below), so
-- nobody is ever seen in their underwear. Safety: a pawn that is ours or a
-- stand-in is revealed after DRESS_SAFETY_S whatever happens.
local DRESS_SAFETY_S = 3.0
local dressing = {}          -- Willie FName -> { t0, revealed }

-- Seconds since BeginPlay hid `w` (nil if we did not hide it).
local function dress_age(w)
    local d = dressing[obj_name(w)]
    return d and (os.clock() - d.t0) or nil
end

-- A live MP session is the one shared rule (shared/hsmp_session:
-- status "connected" AND a heartbeat observed within FRESH_S; the first read
-- only seeds). State left over from an earlier session (a quit, a crash: the
-- link record, peer_kit record, puppets of last time) never changes, so it
-- never dresses a single-player pawn or a native foe. Counting
-- "reconnecting"/"connecting" as live with a 15 s allowance would still apply
-- the kit in the next single-player arena after a sidecar crash.
-- SI.sess is created (and polled) below; until then nothing is live.
local function refresh_session() end   -- kept for the loop; SI.sess:poll does the work
local function mp_live()
    local S = SI.sess
    if not S then return false end
    local ok, live = pcall(S.live, S)
    return ok and live == true
end

-- Everything a Willie carries as a separate actor. SetActorHiddenInGame on
-- the pawn does not reach attached actors, and a weapon spawned or re-armed
-- while its pawn was hidden (dressing), or a pooled weapon actor that the
-- census hid on an extra (HSMPAvatars remove_willie), can stay hidden: no
-- weapon shows although the kit verified it in hand.
local CARRIED_FIELDS = { "Weapon R", "Weapon L", "Weapon R_0", "Weapon L_0",
    "Weapon Slot R 1", "Weapon Slot R 2", "Weapon Slot L 1", "Weapon Slot L 2", "Weapon Slot Back" }

-- true / false (hidden actor, or an invisible root component) / nil (no actor).
local function actor_visible(x)
    if not valid(x) then return nil end
    local hidden = false
    pcall(function() hidden = x.bHidden == true end)
    if hidden then return false end
    local vis = true
    pcall(function()
        local rc = x.RootComponent
        if rc and rc:IsValid() then vis = rc:IsVisible() ~= false end
    end)
    return vis
end

-- A kit weapon can verify in hand with bHidden=false and still not show.
-- bHidden and the root component's IsVisible say nothing about where the
-- weapon is or whether it has a mesh: a modular weapon draws its module /
-- built-in mesh components, and a grip that broke on a spawn teleport
-- leaves "Weapon R" set while the
-- weapon lies metres away (or at the origin for a half-built spawn). The
-- probe: hidden flag, root visible, visible mesh components, origin, and the
-- distance from the hand bone. Read-only; the caller decides what to do.
local WEAPON_HAND_MAX_UU = 400   -- a long polearm's root can sit ~2.5 m up the haft from the grip
local MESH_CLASS = nil
local function weapon_probe(x, pawn, side)
    local r = { hidden = false, root_vis = true, meshes = -1, dist = -1, origin = false }
    if not valid(x) then return nil end
    pcall(function() r.hidden = x.bHidden == true end)
    local wl
    pcall(function()
        local rc = x.RootComponent
        if rc and rc:IsValid() then
            r.root_vis = rc:IsVisible() ~= false
            wl = rc:K2_GetComponentLocation()
        end
    end)
    pcall(function()
        MESH_CLASS = MESH_CLASS or StaticFindObject("/Script/Engine.MeshComponent")
        local comps = x:K2_GetComponentsByClass(MESH_CLASS)
        local n = 0
        for i = 1, #comps do
            pcall(function()
                local c = comps[i]
                local ok, y = pcall(function() return c:get() end)
                if ok and y then c = y end
                -- Only a component that has a mesh asset draws anything: an empty
                -- module component is "visible" yet renders nothing. The 1 cm
                -- SM_Grip_Dummy base does not count either.
                local asset
                pcall(function() local m = c.StaticMesh; if m and m:IsValid() then asset = m:GetFName():ToString() end end)
                if not asset then pcall(function() local m = c.SkeletalMesh; if m and m:IsValid() then asset = m:GetFName():ToString() end end) end
                if not asset then pcall(function() local m = c.SkeletalMeshAsset; if m and m:IsValid() then asset = m:GetFName():ToString() end end) end
                if asset == "SM_Grip_Dummy" then asset = nil end
                if asset and c:IsValid() and c:IsVisible() and c.bHiddenInGame ~= true then n = n + 1 end
            end)
        end
        r.meshes = n
    end)
    if wl then
        r.origin = math.abs(wl.X) < 1 and math.abs(wl.Y) < 1 and math.abs(wl.Z) < 1
        pcall(function()
            local m = pawn and pawn.Mesh
            if m and m:IsValid() then
                local h = m:GetSocketLocation(FName(side == "L" and "hand_l" or "hand_r"))
                local dx, dy, dz = wl.X - h.X, wl.Y - h.Y, wl.Z - h.Z
                r.dist = math.sqrt(dx * dx + dy * dy + dz * dz)
            end
        end)
    end
    return r
end
-- ok, why ("hidden" / "root invisible" / "no visible mesh" / "at origin" /
-- "far from hand (N uu)"); ok = nil when there is no weapon.
local function weapon_shown(x, pawn, side)
    local r = weapon_probe(x, pawn, side)
    if not r then return nil, "none" end
    if r.hidden then return false, "hidden" end
    if not r.root_vis then return false, "root invisible" end
    if r.meshes == 0 then return false, "no visible mesh" end
    if r.origin then return false, "at origin" end
    if r.dist > WEAPON_HAND_MAX_UU then return false, string.format("far from hand (%.0f uu)", r.dist) end
    return true, string.format("ok (meshes=%d hand=%.0f uu)", r.meshes, r.dist)
end
SI.weapon_shown = weapon_shown

-- Unhide the pawn's carried actors (only the actor-level flag, which is the
-- one hiding uses; components keep their own design-time visibility).
-- Returns how many were hidden.
local function unhide_carried(w)
    local n = 0
    for _, f in ipairs(CARRIED_FIELDS) do
        local x = field(w, f)
        if valid(x) then
            local hidden = false
            pcall(function() hidden = x.bHidden == true end)
            if hidden then
                pcall(function() x:SetActorHiddenInGame(false) end)
                n = n + 1
            end
        end
    end
    return n
end

-- Reveal a Willie we hid; returns ms since it was hidden (nil if we didn't).
-- The carried actors are unhidden every time, hidden by us or not.
local function reveal(w, why)
    local nh = unhide_carried(w)
    if nh > 0 then Log("[kit] %s: %d carried actor(s) were hidden; unhidden (%s)", obj_name(w), nh, tostring(why)) end
    local n = obj_name(w)
    local d = dressing[n]
    if not d or d.revealed then return nil end
    d.revealed = true
    pcall(function() w:SetActorHiddenInGame(false) end)
    mark_shown(n)
    return math.floor((os.clock() - d.t0) * 1000 + 0.5)
end

pcall(function()
    RegisterBeginPlayPostHook(function(ctx)
        if not mp_live() then return end   -- the shared liveness rule
        local a; pcall(function() a = ctx:get() end)
        if not a then return end
        local cn; pcall(function() cn = a:GetClass():GetFName():ToString() end)
        if cn ~= "Willie_BP_C" then return end
        local full = ""; pcall(function() full = a:GetFullName() end)
        if not full:find("Map_Arena_") then return end
        local n = obj_name(a)
        if n == "Willie_BP_C_0" then return end   -- the level's pooled proxy (HSMPAvatars census)
        dressing[n] = { t0 = os.clock() }
        if hair_warm_pending() then
            -- First arena of this process: let the groom render before hiding.
            local gen = hair_world
            dressing[n].warming = true
            ExecuteWithDelay(math.floor(HAIR_WARM_S * 1000), function()
                if hair_world ~= gen then return end   -- that world is gone: never touch its actors
                local d = dressing[n]
                if not d or d.revealed then return end
                pcall(function() if a:IsValid() then a:SetActorHiddenInGame(true) end end)
                d.warming = nil
                seen_vis[n] = { hidden = true }
                hair_warmed_here = true
            end)
            return
        end
        pcall(function() a:SetActorHiddenInGame(true) end)
        seen_vis[n] = { hidden = true }
    end)
end)

local puppet_names = {}      -- FName -> peer id, from the last "puppets" bus read

local function safety_reveal()
    local now = os.clock()
    local me
    for n, d in pairs(dressing) do
        if not d.revealed and now - d.t0 > DRESS_SAFETY_S and not d.orphan then
            local w = find_willie(n)
            me = me or local_pawn()
            if not w then
                dressing[n] = nil
            elseif same(w, me) or puppet_names[n] then
                local ms = reveal(w, "safety")
                Log("[kit] safety reveal of %s (%s) after %d ms: kit not verified in time",
                    n, puppet_names[n] and ("stand-in peer " .. puppet_names[n]) or "own pawn", ms or -1)
            else
                -- Not ours and not a stand-in: an extra (HSMPAvatars removes
                -- it). Stays hidden unless it is claimed and dressed later.
                d.orphan = true
            end
        elseif d.orphan and puppet_names[n] then
            d.orphan = nil   -- claimed after all: dressed + revealed by apply_remote
            d.t0 = now
        end
    end
end

local applied_at = {}        -- peer id -> { t, expect, hands } for the post-dress stability check
-- The hand actors' FNames ("R|L", "-" for empty): stability check.
function SI.hands_sig(w)
    local R, L = held_weapons(w)
    return (R and obj_name(R) or "-") .. "|" .. (L and obj_name(L) or "-")
end
local late = { ev = function() end }   -- late.ev = ev() once hsmp_log is loaded (below)

local function apply_remote()
    if not in_arena() then return end
    if not mp_live() then return end   -- never from leftover state (single-player foes)
    local pup = puppets()
    if not pup then return end
    local me = local_pawn()
    local now = os.clock()
    puppet_names = {}
    for id, name in pairs(pup) do puppet_names[tostring(name)] = id end
    for id, name in pairs(pup) do
        -- Stability window: a native re-arm right after dressing gets undone.
        local aa = applied_at[id]
        if aa and applied[id] and now - aa.t < 3 and now >= (aa.next or 0) then
            aa.next = now + 0.3
            local w = find_willie(name)
            if w and #current_passports(w) < aa.expect then
                Log("[kit] stand-in peer %s: native re-arm undid the kit %.1f s after dressing; re-dressing", id, now - aa.t)
                applied[id] = nil
            elseif w and aa.hands and SI.hands_sig(w) ~= aa.hands then
                -- The ubergraph re-armed a hand ~0.2 s after "Set Up
                -- Armor" (a stale passport entry, or a hand that shows a world
                -- item): re-run the hands only (bounded by tries[]).
                Log("[kit] stand-in peer %s: hands changed %.1f s after dressing (%s -> %s); re-applying the hands",
                    id, now - aa.t, aa.hands, SI.hands_sig(w))
                aa.hands = nil
                SI.aw[id] = nil
            end
        end
        local R = remote_loadout(id)   -- the peer_loadout record (one table per version)
        -- Validated class kit: filters (enforced rules) or stands in for the
        -- appearance until it arrives.
        local L, ksuf = R, ""
        local okk, EL, ks = pcall(Kit.effective_remote, id, R)
        if okk then L, ksuf = EL, ks or "" end
        if type(L) == "table" then
            local wh = world_held(id)
            local hsuf = wh and string.format("|h%s,%s", wh.R and tostring(wh.R[1]) or "-",
                                                         wh.L and tostring(wh.L[1]) or "-") or ""
            -- A strip holds until the owner's next loadout version.
            local st = SI.strip[id]
            if st and st.v ~= L.v then SI.strip[id], st = nil, nil end
            -- Armour and hands have separate keys.
            local key = ksuf .. "|" .. Kit.armour_sig(L) .. "@" .. tostring(name)
            local wkey = Kit.weapon_sig(L, hsuf, st) .. "@" .. tostring(name)
            if applied[id] == key and SI.aw[id] ~= wkey then
                local t = tries[id]
                local tk = "w:" .. wkey
                if not t or t.key ~= tk then t = { key = tk, n = 0, next_at = 0 }; tries[id] = t end
                if t.n < 6 and now >= t.next_at then
                    local w = find_willie(name)
                    local hp = 1
                    if w then pcall(function() hp = tonumber(w.Health) or 1 end) end
                    if w and not same(w, me) and hp > 0 then
                        t.n = t.n + 1
                        t.next_at = now + 0.5
                        local ok, wr, wl = apply_weapons(w, L, id, st)
                        if ok then SI.aw[id] = wkey end
                        Log("peer %s: hands only on %s (try %d): R=%s L=%s%s", id, name, t.n, wr, wl,
                            ok and "" or " - will retry")
                    end
                end
            elseif applied[id] ~= key then
                local t = tries[id]
                if not t or t.key ~= key then t = { key = key, n = 0, next_at = 0 }; tries[id] = t end
                if t.n < 6 and now >= t.next_at then
                    local w = find_willie(name)
                    local hp = 1
                    if w then pcall(function() hp = tonumber(w.Health) or 1 end) end
                    local wait = (w and hp > 0) and dress_wait(w) or 0
                    if wait > 0 then
                        if t.hair_note ~= key then
                            t.hair_note = key
                            Log("[kit] stand-in peer %s: %s became visible undressed; dressing in %.1f s (hair settle)",
                                id, name, wait)
                        end
                    elseif w and not same(w, me) and hp > 0 then
                        t.n = t.n + 1
                        t.next_at = now + 0.5
                        local ok, res = apply_loadout(w, L, id, st)
                        if ok then
                            applied[id], SI.aw[id] = key, wkey
                            local neq = #current_passports(w)
                            applied_at[id] = { t = now, expect = neq, hands = SI.hands_sig(w) }
                            local ms = reveal(w, "dressed")
                            local R, Lw = held_weapons(w)
                            -- Where the stand-in's weapons
                            -- really are (hidden / mesh / origin / hand distance)
                            local _, rwhy = weapon_shown(R, w, "R")
                            local _, lwhy = weapon_shown(Lw, w, "L")
                            Log("[kit] stand-in peer %s verified on %s: equipped=%d worn meshes=%d R=%s [%s] L=%s [%s]%s",
                                id, name, neq, worn_count(w),
                                R and obj_name(R) or "-", tostring(rwhy), Lw and obj_name(Lw) or "-", tostring(lwhy),
                                ms and string.format(" - dressed in %d ms", ms) or "")
                            -- the gate's peer kit evidence
                            late.ev("kit_verified", { who = "peer:" .. tostring(id), armour_n = neq,
                                r_class = R and cls_name(R) or "None", l_class = Lw and cls_name(Lw) or "None",
                                ok = true })
                        end
                        Log("peer %s: applied loadout v=%s to %s (try %d): %s%s", id, tostring(L.v),
                            name, t.n, res, ok and "" or " — will retry")
                        if not ok and t.n == 1 then dump_gear(w, "stand-in for peer " .. id .. " after failed apply") end
                    end
                end
            end
        end
    end
end

-- A peer's weapon left their hand (a drop or a disarm): the sidecar's
-- world_dyn record (typed) gets a dynamic entry with dyn_owner == that peer at once,
-- while their next peer_loadout record can be 2 s away. Strip the stand-in's copy
-- of that class now (else it keeps parrying with a colliding duplicate), and keep
-- the hand empty until the owner's next loadout version (no re-arm from the
-- stale L.w). The world's first read is a baseline (entries of earlier drops
-- are not acted on).
function SI.strip_dropped()
    local ipc = rawget(_G, "HSMP_IPC")
    local t, ver = nil, nil
    if ipc and ipc.rec then t, ver = ipc.rec("world_dyn") end
    if ver == SI.dyn_raw and SI.dyn_seen ~= nil then return end
    SI.dyn_raw = ver
    local first = SI.dyn_seen == nil   -- (never written yet: an empty baseline)
    SI.dyn_seen = SI.dyn_seen or {}
    local fresh, dyn = {}, {}
    for _, r in ipairs(type(t) == "table" and t.rows or {}) do
        local cls = r.class_path or ""
        dyn[#dyn + 1] = { id = r.id, peer = r.dyn_owner, leaf = cls:match("([^%./]+)$") or cls }
    end
    for _, e in ipairs(dyn) do
        if not SI.dyn_seen[e.id] then
            SI.dyn_seen[e.id] = true
            if not first then fresh[#fresh + 1] = e end
        end
    end
    if #fresh == 0 then return end
    local pup = puppets()
    if not pup then return end
    for _, e in ipairs(fresh) do
        local id = tostring(e.peer)
        local name = pup[id]
        local w = name and find_willie(name)
        local wh = w and world_held(id)
        if w then
            for _, side in ipairs({ "R", "L" }) do
                local cur
                -- a hand that shows a replicated world item (bus key world_held) is
                -- HSMPWorld's: never destroyed here
                if not (wh and wh[side]) then
                    for _, f in ipairs(side == "R" and { "Weapon R", "Weapon R_0" } or { "Weapon L", "Weapon L_0" }) do
                        if not cur then local x = field(w, f); if valid(x) then cur = x end end
                    end
                end
                if cur and cls_name(cur) == e.leaf then
                    local res = apply_weapon(w, side, nil)
                    local R = remote_loadout(id)
                    local v = (R and R.v) or 0
                    local st = SI.strip[id]
                    if not st or st.v ~= v then st = { v = v }; SI.strip[id] = st end
                    st[side] = true
                    Log("peer %s dropped %s (world item %d): stand-in %s hand %s %s at once", id, e.leaf, e.id,
                        tostring(name), side, res)
                end
            end
        end
    end
end

-- Publish the own loadout as soon as a hand's weapon actor changes
-- (a drop, a disarm, a pickup), not on the next 2 s writer tick. Addresses
-- only (numbers), nothing kept beyond the call.
function SI.hand_watch()
    local p = local_pawn()
    if not p then SI.hand = nil; return end
    local sig = {}
    for i, f in ipairs({ "Weapon R", "Weapon L" }) do
        local x = field(p, f)
        local a = 0
        if valid(x) then pcall(function() a = x:GetAddress() end) end
        sig[i] = tostring(a)
    end
    local s = table.concat(sig, "|")
    local was = SI.hand
    SI.hand = s
    if was ~= nil and was ~= s then pcall(write_local) end
end

-- --- diagnostics triggers ------------------------------------------------------

-- Forward declaration: the Ctrl+F7 keybind below calls it, and it is defined
-- with the loops further down (a later `local function world_ok` would leave
-- the keybind calling a nil global, swallowed by the shim's pcall).
local world_ok
local dumped_this_arena = false
local function dump_all(reason)
    Log("[gear] dump (%s)", reason)
    dump_gear(local_pawn(), "LOCAL player")
    local pup = puppets()
    if pup then
        for id, name in pairs(pup) do dump_gear(find_willie(name), "stand-in for peer " .. id) end
    end
end

pcall(function()
    RegisterKeyBind(Key.F7, { ModifierKey.CONTROL }, function()
        if world_ok() and SI.world_settled and SI.world_settled() and in_arena() then
            dump_all("Ctrl+F7")
            Kit.force_local()   -- and re-apply the own class kit
        end
    end)
end)


-- Structured events (shared/hsmp_log.lua; kit_verified{who="self",...}). No-op when not deployed.
local HL
do
    local ok, m = pcall(require, "hsmp_log")
    if not (ok and type(m) == "table") then
        local src = (debug.getinfo(1, "S").source or ""):gsub("^@", "")
        local dir = src:match("^(.*)[/\\]") or "."
        for _, p in ipairs({ dir .. "/hsmp_log.lua", dir .. "/../../shared/hsmp_log.lua" }) do
            local ok2, m2 = pcall(dofile, p)
            if ok2 and type(m2) == "table" then m = m2; ok = true; break end
        end
    end
    if ok and type(m) == "table" then
        HL = m
        if HL.init then pcall(HL.init, { mod = "HSMPLoadout", state_dir = STATE_DIR }) end
    end
end
-- HSMP-SHM facade (shared/hsmp_ipc.lua), published as the per-state global
-- HSMP_IPC; this mod's state-dir helpers route through it.
do
    local m = (function() local ok, x = pcall(require, "hsmp_ipc"); if ok and type(x) == "table" then return x end; local src = (debug.getinfo(1, "S").source or ""):gsub("^@", ""); local dir = src:match("^(.*)[/\\]") or "."; for _, p in ipairs({ dir .. "/hsmp_ipc.lua", dir .. "/../../shared/hsmp_ipc.lua" }) do local ok2, y = pcall(dofile, p); if ok2 and type(y) == "table" then return y end end end)()
    if m then m.init({ mod = "HSMPLoadout", state_dir = STATE_DIR, log = Log }) end
end
local function ev(name, fields)
    if HL and HL.event then pcall(HL.event, name, fields or {}) end
end
late.ev = ev

Kit.init({
    Log = Log, STATE_DIR = STATE_DIR, ev = ev, now = os.clock,
    actor_visible = actor_visible, unhide_carried = unhide_carried,
    valid = valid, field = field, resolve_class = resolve_class, expand_path = expand_path,
    class_path = class_path, bp_call = bp_call, spawn_weapon_actor = spawn_weapon_actor,
    enc_struct = enc_struct, dec_struct = dec_struct, WEAPON_FIELDS = WEAPON_FIELDS,
    set_hand_passport = set_hand_passport, weapon_passport_for = weapon_passport_for,
    destroy_hand_weapon = destroy_hand_weapon, weapon_shown = weapon_shown,
    reveal = reveal, dress_wait = dress_wait, dress_age = dress_age,
    apply_armour = apply_armour, read_pieces = read_pieces, read_source = read_source,
    worn_count = worn_count, in_arena = in_arena, local_pawn = local_pawn, busy = busy,
    fighter_context = function(pawn)
        local context, m = PCF.fighter_context(), PCF.describe(pawn)
        return context and m and context.pawn == m.name and context.world == m.world and context.key or nil
    end,
    mp_live = mp_live,
})

-- --- loops -----------------------------------------------------------------------

-- Forget everything tied to the old world without touching it. Pure Lua:
-- safe inside the LoadMap hook.
local function drop_world_caches()
    PCF.bound = nil
    applied, tries, dumped_this_arena = {}, {}, false
    dressing, applied_at, puppet_names, seen_vis = {}, {}, {}, {}
    if hair_warmed_here then hair_warm_done = true end
    hair_warmed_here = false
    hair_world = hair_world + 1
    for k in pairs(class_cache) do class_cache[k] = nil end
    base_pass = {}
    SI.reset()
    pcall(function() if Kit.on_world_change then Kit.on_world_change() end end)
end

pcall(function()
    RegisterLoadMapPreHook(function() world_gen = world_gen + 1; drop_world_caches() end)
end)
pcall(function()
    RegisterLoadMapPostHook(function() world_gen = world_gen + 1 end)
end)

-- The shared world-settle rule (shared/hsmp_wg.lua M.settle_tracker):
-- nothing here walks FindAllOf over Willies / weapons or touches a pawn in
-- the first SETTLE_S of a world (the round-reload crash window).
local SETTLE = (function()
    local ok, m = pcall(require, "hsmp_wg")
    if not (ok and type(m) == "table") then
        local src = (debug.getinfo(1, "S").source or ""):gsub("^@", "")
        local dir = src:match("^(.*)[/\\]") or "."
        for _, p in ipairs({ dir .. "/hsmp_wg.lua", dir .. "/../../shared/hsmp_wg.lua" }) do
            local ok2, m2 = pcall(dofile, p)
            if ok2 and type(m2) == "table" then m = m2; ok = true; break end
        end
    end
    if ok and type(m) == "table" then PCF.aip = m end   -- local_pawn's AI-drive fallback
    if ok and type(m) == "table" and m.settle_tracker then return m.settle_tracker(), m.SETTLE_S end
    -- fallback (shared lib missing): the same rule inline
    local st = { key = nil, at = nil }
    function st.note(k) if k ~= st.key then st.key, st.at = k, os.clock() end end
    function st.settled(s) return st.key ~= nil and os.clock() - st.at >= (s or 2.0) end
    return st, 2.0
end)()
local function world_settled() return SETTLE.settled() end
PCF.settled = world_settled
SI.world_settled = world_settled

local seen_world = nil
world_ok = function()
    local id
    pcall(function()
        local w = PCF.world()
        if w and w:IsValid() then id = tostring(w:GetAddress()) .. "@" .. w:GetFullName() end
    end)
    local key = id and (tostring(world_gen) .. "|" .. id) or nil
    if key ~= seen_world then
        if seen_world ~= nil then drop_world_caches() end
        seen_world = key
        SI.world_at = os.time()
    end
    SETTLE.note(key)
    return key ~= nil
end

local was_in_arena, arena_entered_at = false, 0

-- In the menu with no live session, clear the kit state an earlier session
-- left behind (once per menu visit); the sidecar re-writes everything it
-- needs after its next Welcome.
local function menu_name()
    local n
    pcall(function()
        local w = PCF.world()
        if w and w:IsValid() then n = w:GetFullName() end
    end)
    return n
end
local menu_cleaned = false
-- Shared memory: the peer kit / loadout slots and the session's kit
-- rules belong to the sidecar (stale ones carry an old session epoch); only
-- this mod's own bus keys are cleared here.
local function clear_stale_session_files()
    local I = ipc()
    local n = 0
    if I and I.bus_table then
        -- kit_status is a typed record: the zero record (pawn "") reads as "no status".
        local ks = I.bus_table("kit_status")
        if type(ks) == "table" and (ks.pawn or "") ~= "" then I.bus_put("kit_status", {}); n = n + 1 end
        if I.bus_table("world_held") ~= nil then I.bus_clear("world_held"); n = n + 1 end
    end
    if Kit.forget_rules then Kit.forget_rules() end   -- no last-good rules across sessions
    lo_cache = {}
    Log("menu without a live MP session: %d leftover kit state key(s) cleared", n)
end
-- 100 ms: dressing must happen within a few frames of a spawn. Everything in
-- here is cheap when nothing is pending (a few small record reads).
-- The menu cleanup decides on shared/hsmp_session.lua (terminal status,
-- absence >= 2 s, or no heartbeat for >= 15 s: Kit.menu_cleanup_ok), never on
-- a single look at state the sidecar may be writing right now.
SI.sess = (function()
    local ok, m = pcall(require, "hsmp_session")
    if not (ok and type(m) == "table") then
        local src = (debug.getinfo(1, "S").source or ""):gsub("^@", "")
        local dir = src:match("^(.*)[/\\]") or "."
        for _, p in ipairs({ dir .. "/hsmp_session.lua", dir .. "/../../shared/hsmp_session.lua" }) do
            local ok2, m2 = pcall(dofile, p)
            if ok2 and type(m2) == "table" then m = m2; ok = true; break end
        end
    end
    return (ok and type(m) == "table") and m.new({ every_s = 0 }) or nil
end)()
if not SI.sess then Log("WARNING: shared/hsmp_session.lua missing - leftover kit files are not cleaned in the menu") end

local loop_n = 0
LoopAsync(100, function()
    ExecuteInGameThread(function()
        loop_n = loop_n + 1
        if loop_n % 10 == 1 then
            refresh_session()
            if SI.sess then pcall(SI.sess.poll, SI.sess, true) end
        end
        if not world_ok() then was_in_arena = false; return end
        if loop_n % 10 == 1 then
            local wn = menu_name() or ""
            if wn:find("Map_Menu_") then
                if not menu_cleaned and not mp_live() and Kit.menu_cleanup_ok
                    and Kit.menu_cleanup_ok(SI.sess, os.clock()) then
                    menu_cleaned = true
                    pcall(clear_stale_session_files)
                end
            else
                menu_cleaned = false
            end
        end
        -- No Willie / weapon is touched in the first SETTLE_S of a world
        if not world_settled() then return end
        local now_in = in_arena()
        if was_in_arena and not now_in then
            applied, tries, dumped_this_arena = {}, {}, false
            SI.reset()
        end
        if now_in and mp_live() then
            pcall(SI.strip_dropped)   -- before the applier, so it sees the strip
            pcall(SI.hand_watch)
        end
        if now_in and not was_in_arena then arena_entered_at = os.time() end
        was_in_arena = now_in
        -- 10 s after the arena AND after the current world loaded: a round
        -- reload keeps us "in the arena", and a dump 0.5 s into the new world
        -- on a half-built pawn can crash the game (GetFullName on a null
        -- worn-mesh asset).
        if now_in and not dumped_this_arena and local_pawn()
           and os.time() - math.max(arena_entered_at, SI.world_at or 0) >= 10 then
            dumped_this_arena = true
            pcall(dump_all, "auto, 10 s after arena load")
        end
        pcall(apply_remote)
        local okk, kerr = pcall(Kit.tick_local)
        if not okk then Log("kit tick error: %s", tostring(kerr)) end
        pcall(safety_reveal)
    end)
    return false
end)

LoopAsync(2000, function()
    -- The own appearance is published only while an MP session is live,
    -- and never in the first SETTLE_S of a world (held_weapons reads the hands)
    ExecuteInGameThread(function() if world_ok() and world_settled() and mp_live() then pcall(write_local) end end)
    return false
end)

Log("loaded; state_dir=%s (Ctrl+F7 = gear dump + re-apply own kit)", STATE_DIR)


