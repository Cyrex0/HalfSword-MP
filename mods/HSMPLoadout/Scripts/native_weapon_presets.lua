-- Authored native recipe proofs, intentionally absent from the public catalogue.
-- Harvest: inventory-harvest-20261008 native assets + exact native-enums Names.
-- Enum numbers below come from serialized Names, never enumerator suffixes.
-- Base ModularWeaponBP needs separate exact recipe/source combat admission.
local P={}
local recipes={
    ["native_pollaxe"]={asset="/Game/Blueprints/DataAssets/Equipment/FreeMode/DA_FreeMode_Inventory_Tier_Knight.DA_FreeMode_Inventory_Tier_Knight", container="Inventory", field="WeaponPssports_6_0FE43BA744FE0B9934051BA2CD2DD971", index=1, family="Polearm", selectable=false, passport={
        class="@Weapons/Blueprints/ModularWeaponBP",
        id=0,
        name="Pollaxe",
        head_sub1="@Weapons/Poleaxes/Modules/Head/Sub/Modular_PA_Head_SubModule_Axe",
        head_sub2="@Weapons/Poleaxes/Modules/Head/Sub/Modular_PA_Head_SubModule_Hammer_Back",
        head="@Weapons/Poleaxes/Modules/Head/Tiers/High_Tier/Modular_PA_Head_Center_High_Tier_002",
        guard="@Weapons/Poleaxes/Modules/Guard/Modular_PA_Guard_A",
        pommel="@Weapons/Poleaxes/Modules/Pommel/Modular_PA_Pommel_C",
        grip="@Weapons/Poleaxes/Modules/Haft/Tiers/High_Tier/Modular_Weapon_Polearm_Haft_High_Tier_Avg",
        head_size={0.80000000000000004,0.80000000000000004,0.80000000000000004},
        guard_size={0.80000000000000004,0.80000000000000004,0.80000000000000004},
        grip_size={0.80000000000000004,0.80000000000000004,0.80000000000000004},
        pommel_size={0.75,0.75,0.75},
        mass_head=2.25,
        mass_guard=1,
        mass_grip=0.69999999999999996,
        mass_pommel=2,
        mat_steel=0,
        mat_colored=3,
        mat_wood=14,
        mat_leather=10,
        color_wood={1,1,1,1},
        color_leather={0.18634300000000001,0.074750999999999998,0.064439999999999997,1},
        price=0,
        tier=7,
    }},
    ["native_longsword"]={asset="/Game/Blueprints/DataAssets/Equipment/FreeMode/DA_FreeMode_Inventory_Tier_Knight.DA_FreeMode_Inventory_Tier_Knight", container="Inventory", field="WeaponPssports_6_0FE43BA744FE0B9934051BA2CD2DD971", index=2, family="Sword", selectable=false, passport={
        class="@Weapons/Blueprints/ModularWeaponBP",
        id=0,
        name="Longsword",
        head_sub1="",
        head_sub2="",
        head="@Weapons/Swords/Modules/Blade/Modular_Sword_Blade_D_100",
        guard="@Weapons/Swords/Modules/Guard/Weapon_Sword_Guard_AB",
        pommel="@Weapons/Swords/Modules/Pommels/Weapon_Sword_Pommel_V_LS",
        grip="@Weapons/Swords/Modules/Grips/Weapon_Sword_Grip_20_T4",
        head_size={1,0.5,1},
        guard_size={1,1,1},
        grip_size={0.75,0.5,1},
        pommel_size={1,1,1},
        mass_head=1,
        mass_guard=0.75,
        mass_grip=1,
        mass_pommel=0.75,
        mat_steel=0,
        mat_colored=5,
        mat_wood=14,
        mat_leather=10,
        color_wood={0.03125,0.03125,0.03125,1},
        color_leather={0.03125,0.03125,0.03125,1},
        price=0,
        tier=3,
    }},
    ["native_bastard_sword"]={asset="/Game/Blueprints/DataAssets/Equipment/FreeMode/DA_FreeMode_Inventory_Tier_Knight.DA_FreeMode_Inventory_Tier_Knight", container="Inventory", field="WeaponPssports_6_0FE43BA744FE0B9934051BA2CD2DD971", index=3, family="Sword", selectable=false, passport={
        class="@Weapons/Blueprints/ModularWeaponBP",
        id=0,
        name="Bastard Sword",
        head_sub1="",
        head_sub2="",
        head="@Weapons/Swords/Modules/Blade/Modular_Sword_Blade_A_85",
        guard="@Weapons/Swords/Modules/Guard/Weapon_Sword_Guard_CA",
        pommel="@Weapons/Swords/Modules/Pommels/Weapon_Sword_Pommel_E",
        grip="@Weapons/Swords/Modules/Grips/Weapon_Sword_Grip_15_T3",
        head_size={1,0.5,1},
        guard_size={1,1,1},
        grip_size={0.75,0.5,1},
        pommel_size={1,1,1},
        mass_head=1,
        mass_guard=0.75,
        mass_grip=1,
        mass_pommel=0.75,
        mat_steel=1,
        mat_colored=3,
        mat_wood=14,
        mat_leather=10,
        color_wood={0.03125,0.03125,0.03125,1},
        color_leather={0.03125,0.03125,0.03125,1},
        price=0,
        tier=3,
    }},
    ["native_arming_sword"]={asset="/Game/Blueprints/DataAssets/Equipment/FreeMode/DA_FreeMode_Inventory_Tier_Knight.DA_FreeMode_Inventory_Tier_Knight", container="Inventory", field="WeaponPssports_6_0FE43BA744FE0B9934051BA2CD2DD971", index=4, family="Sword", selectable=false, passport={
        class="@Weapons/Blueprints/ModularWeaponBP",
        id=0,
        name="Arming Sword",
        head_sub1="",
        head_sub2="",
        head="@Weapons/Swords/Modules/Blade/Modular_Sword_Blade_G_60",
        guard="@Weapons/Swords/Modules/Guard/Weapon_Sword_Guard_GA",
        pommel="@Weapons/Swords/Modules/Pommels/Weapon_Sword_Pommel_A",
        grip="@Weapons/Swords/Modules/Grips/Weapon_Sword_Grip_12_T2",
        head_size={1,0.5,1},
        guard_size={1,1,1},
        grip_size={0.75,0.5,1},
        pommel_size={1,1,1},
        mass_head=1,
        mass_guard=0.75,
        mass_grip=1,
        mass_pommel=0.75,
        mat_steel=1,
        mat_colored=3,
        mat_wood=14,
        mat_leather=10,
        color_wood={0.03125,0.03125,0.03125,1},
        color_leather={0.03125,0.03125,0.03125,1},
        price=0,
        tier=3,
    }},
    ["native_mace"]={asset="/Game/Blueprints/DataAssets/Equipment/FreeMode/DA_FreeMode_Inventory_Tier_Knight.DA_FreeMode_Inventory_Tier_Knight", container="Inventory", field="WeaponPssports_6_0FE43BA744FE0B9934051BA2CD2DD971", index=5, family="Blunt", selectable=false, passport={
        class="@Weapons/Blueprints/ModularWeaponBP",
        id=0,
        name="Mace",
        head_sub1="",
        head_sub2="",
        head="@Weapons/Maces/Modules/MaceHeads/Weapon_Part_MaceHead_I",
        guard="",
        pommel="",
        grip="@Weapons/Maces/Modules/Shafts/Weapon_Part_Mace_Grip_I_S",
        head_size={1,1,1},
        guard_size={1,1,1},
        grip_size={1,1,1},
        pommel_size={1,1,1},
        mass_head=1,
        mass_guard=1,
        mass_grip=0.75,
        mass_pommel=1,
        mat_steel=7,
        mat_colored=7,
        mat_wood=14,
        mat_leather=10,
        color_wood={1,1,1,1},
        color_leather={0.18634300000000001,0.074750999999999998,0.064439999999999997,1},
        price=0,
        tier=0,
    }},
    ["native_baron_sword"]={asset="/Game/Blueprints/DataAssets/Equipment/Loadout/DA_Equipment_Loadout_Baron.DA_Equipment_Loadout_Baron", container="Loadout", field="WeaponsinSlots_11_B42349384F5EF74DE78A7F870D89656A", map_key=0, key_name="SheathSlots_Enum::NewEnumerator0", family="Sword", selectable=false, passport={
        class="@Weapons/Blueprints/ModularWeaponBP",
        id=4444,
        name="Baron Sword",
        head_sub1="",
        head_sub2="",
        head="@Weapons/Swords/Falchion/Modules/Blades/Modular_Falchion_Blade_B_80",
        guard="@Weapons/Swords/Modules/Guard/Weapon_Sword_Guard_JA",
        pommel="@Weapons/Swords/Modules/Pommels/Weapon_Sword_Pommel_C_LS",
        grip="@Weapons/Swords/Modules/Grips/Weapon_Sword_Grip_20_T4",
        head_size={1.5,0.34999999999999998,1.75},
        guard_size={1.25,1.25,1.25},
        grip_size={1.25,1.25,1.5},
        pommel_size={1.125,1.125,1.125},
        mass_head=1,
        mass_guard=0.75,
        mass_grip=1,
        mass_pommel=0.75,
        mat_steel=0,
        mat_colored=5,
        mat_wood=14,
        mat_leather=10,
        color_wood={0.03125,0.03125,0.03125,1},
        color_leather={0.03125,0.03125,0.03125,1},
        price=750,
        tier=8,
    }},
    ["native_baron_mace"]={asset="/Game/Blueprints/DataAssets/Equipment/Loadout/DA_Equipment_Loadout_Baron.DA_Equipment_Loadout_Baron", container="Loadout", field="WeaponsinSlots_11_B42349384F5EF74DE78A7F870D89656A", map_key=3, key_name="SheathSlots_Enum::NewEnumerator2", family="Blunt", selectable=false, passport={
        class="@Weapons/Blueprints/ModularWeaponBP",
        id=3333,
        name="Baron Mace",
        head_sub1="",
        head_sub2="",
        head="@Weapons/Casted/Modules/Heads/Mace/Weapon_Module_Casted_Head_Mace_B",
        guard="",
        pommel="",
        grip="@Weapons/Casted/Modules/Grips/Weapon_Module_Casted_Grip_BA",
        head_size={1.25,1.25,1.25},
        guard_size={0,0,0},
        grip_size={1.25,1.25,1.25},
        pommel_size={0,0,0},
        mass_head=1,
        mass_guard=1,
        mass_grip=1,
        mass_pommel=1,
        mat_steel=5,
        mat_colored=4,
        mat_wood=14,
        mat_leather=10,
        color_wood={1,1,1,1},
        color_leather={0.18634300000000001,0.074750999999999998,0.064439999999999997,1},
        price=300,
        tier=8,
    }},
}
local function copy(v)
    if type(v)~="table"then return v end
    local out={};for k,x in pairs(v)do out[k]=copy(x)end;return out
end
local function number_equal(a,b)
    if type(a)~="number"or a~=a or math.abs(a)==math.huge then return false end
    return string.pack("<f",a)==string.pack("<f",b)
end
function P.descriptor(id)return recipes[id]and copy(recipes[id])or nil end
function P.class_for(id)return recipes[id]and recipes[id].passport.class or nil end
local integers={id=true,mat_steel=true,mat_colored=true,mat_wood=true,mat_leather=true,tier=true}
function P.matches_record(id,record)
    local d=recipes[id]
    if not d or type(record)~="table"then return false end
    for k,v in pairs(d.passport)do
        local got=record[k]
        if type(v)=="table"then
            if type(got)~="table"then return false end
            for i,n in ipairs(v)do if not number_equal(got[i],n)then return false end end
        elseif type(v)=="number"then
            if integers[k] then
                if type(got)~="number"or math.tointeger(got)~=got or got~=v then return false end
            elseif not number_equal(got,v)then return false end
        elseif type(got)~="string"or got~=v then return false end
    end
    return true
end
function P.new(o)
    local out={}
    function out.resolve(id)
        local d=recipes[id]
        if not d then return nil,"unknown native recipe"end
        local ok,gen=pcall(o.generation)
        if not ok or gen==nil then return nil,"native recipe context unavailable"end
        local found,source=pcall(o.source,copy(d))
        if not found or not source then return nil,"native recipe source unavailable"end
        local current,g=pcall(o.generation)
        if not current or g~=gen then return nil,"native recipe context changed"end
        local read,record=pcall(o.read_record,source)
        if not read or not P.matches_record(id,record)then return nil,"native recipe passport mismatch"end
        local matched,exact=pcall(o.matches,source,d.passport)
        if not matched or exact~=true then return nil,"native recipe source incomplete"end
        current,g=pcall(o.generation)
        if not current or g~=gen then return nil,"native recipe context changed"end
        -- Every client receives the same authored scalars after the fresh
        -- native source proves the complete recipe; no per-client defaults.
        return copy(d.passport)
    end
    return out
end
return P

