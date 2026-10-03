-- hsmp_catalog — the HSMP class / gear catalogue (pure data, no UE calls).
--
-- Every item is a real Half Sword Blueprint class, taken from the game's own
-- equipment data assets in pakchunk0 (read from their import tables):
--   armour : /Game/Blueprints/DataAssets/Equipment/Armor/Slots/DA_Equipment_Armor_Slot_*
--            (one data asset per armour layer; a UI row below = one of those)
--   weapons: /Game/Blueprints/DataAssets/Equipment/Weapons/PreMade/{Tiers,Types}/*
--            plus the tiered sword/mace/hafted/polearm BPs in Built_Weapons/
--   kits   : modelled on DA_Equipment_Loadout_Tier_{Peasant,..,Knight}
--
-- Paths are short: "@X" = "/Game/Assets/X", and "Dir/BP_Name" expands to
-- "/Game/Assets/Dir/BP_Name.BP_Name_C" (HSMPLoadout resolve_class).
--
-- The server keeps a copy of ids/groups/costs/classes in server/src/loadout.rs
-- (`catalog` module). `cargo test` parses this file and fails if they differ,
-- so edit both together (see docs/development/subsystems/classes-loadout.md).
--
-- Line format is parsed by the Rust test, keep one entry per line:
--   A("id", "group", cost, "ClassName", "Label")              armour
--   W("id", "hand", cost, "Sub/Dir/ClassName", "Label")        weapon
--   K("id", "LABEL", "right", "left", { "armour ids" }, "blurb") class kit

local C = {
    version = 1,
    armor_root  = "@Armor/Blueprints/Modular_Armor/",
    weapon_root = "@Weapons/Blueprints/Built_Weapons/",
    items = {},        -- id -> item
    order = {},        -- group -> { ids in UI order }
    classes = {},      -- ordered list of kits
    class_by_id = {},
}

-- Armour rows, in UI order. `layer` = the game's slot data asset.
C.groups = {
    { id = "head",      label = "HEAD",       layer = "Head_2" },
    { id = "bevor",     label = "BEVOR",      layer = "Neck_2" },
    { id = "collar",    label = "MAIL COLLAR",layer = "Neck_1" },
    { id = "shoulders", label = "SHOULDERS",  layer = "Shoulder" },
    { id = "arms",      label = "ARMS",       layer = "Arms" },
    { id = "hands",     label = "HANDS",      layer = "Hand" },
    { id = "body",      label = "BODY",       layer = "Body_1" },
    { id = "mail",      label = "MAIL SHIRT", layer = "Body_2" },
    { id = "chest",     label = "CHEST PLATE",layer = "Body_3" },
    { id = "tabard",    label = "TABARD",     layer = "Body_4" },
    { id = "waist",     label = "WAIST",      layer = "Waist" },
    { id = "legs",      label = "LEGS",       layer = "Leg_1" },
    { id = "thighs",    label = "THIGHS",     layer = "Leg_2" },
    { id = "shins",     label = "KNEES/SHINS",layer = "Leg_3" },
    { id = "feet",      label = "FEET",       layer = "Foot" },
}

-- Weapon hand classes: "1h" either hand, "dagger" either hand,
-- "2h" right hand only and the left hand must be empty, "shield" left only.
C.hands = { "1h", "2h", "dagger", "shield" }

-- Rules modes (server-enforced). Budget only matters in CUSTOM.
C.MODE_FREE, C.MODE_CLASSES, C.MODE_CUSTOM = 0, 1, 2
C.mode_names = { [0] = "FREE", [1] = "CLASSES ONLY", [2] = "CUSTOM (BUDGET)" }
C.budgets = { 12, 20, 30, 45, 60 }
C.default_budget = 30
C.default_class = "man_at_arms"

-- Cosmetics (index 0 = keep the game's own look).
C.hair = {
    { "DEFAULT" },
    { "BLACK",  1.00, 0.05 }, { "DARK BROWN", 0.75, 0.25 }, { "BROWN", 0.55, 0.30 },
    { "AUBURN", 0.45, 0.80 }, { "RED", 0.30, 1.00 }, { "BLOND", 0.15, 0.15 },
    { "GREY",   0.05, 0.00 },
}
C.tints = {
    { "DEFAULT" },
    { "CRIMSON", 0.45, 0.04, 0.04 }, { "AZURE", 0.05, 0.15, 0.45 }, { "FOREST", 0.06, 0.25, 0.08 },
    { "OCHRE", 0.55, 0.38, 0.08 }, { "SABLE", 0.03, 0.03, 0.03 }, { "ARGENT", 0.75, 0.75, 0.72 },
    { "PURPURE", 0.30, 0.06, 0.35 },
}
C.faces = { "DEFAULT", "FACE 1", "FACE 2", "FACE 3", "FACE 4", "FACE 5", "FACE 6", "FACE 7" }

local function add(kind, id, group, cost, path, label)
    local root = kind == "armor" and C.armor_root or C.weapon_root
    local it = { id = id, kind = kind, group = group, cost = cost,
                 path = root .. path, label = label }
    C.items[id] = it
    C.order[group] = C.order[group] or {}
    table.insert(C.order[group], id)
end
local function A(id, group, cost, path, label) add("armor", id, group, cost, path, label) end
local function W(id, hand, cost, path, label) add("weapon", id, hand, cost, path, label) end
local function K(id, label, r, l, armor, blurb)
    local k = { id = id, label = label, r = r, l = l, armor = armor, blurb = blurb }
    table.insert(C.classes, k)
    C.class_by_id[id] = k
end

-- --- armour ---------------------------------------------------------------
A("h_hat1", "head", 0, "BP_Armor_Modular_Core_Head_Hat_1", "Hat I")
A("h_hat2", "head", 0, "BP_Armor_Modular_Core_Head_Hat_2", "Hat II")
A("h_hat3", "head", 0, "BP_Armor_Modular_Core_Head_Hat_3", "Hat III")
A("h_hat4", "head", 0, "BP_Armor_Modular_Core_Head_Hat_4", "Hat IV")
A("h_cap1", "head", 1, "BP_Armor_Modular_Core_Head_Cap_1", "Cap I")
A("h_cap2", "head", 1, "BP_Armor_Modular_Core_Head_Cap_2", "Cap II")
A("h_cap3", "head", 1, "BP_Armor_Modular_Core_Head_Cap_3", "Cap III")
A("h_cap4", "head", 1, "BP_Armor_Modular_Core_Head_Cap_4", "Cap IV")
A("h_kettle1", "head", 3, "BP_Armor_Modular_Core_Head_KettleHelm_1", "Kettle Hat I")
A("h_kettle2", "head", 3, "BP_Armor_Modular_Core_Head_KettleHelm_2", "Kettle Hat II")
A("h_kettle3", "head", 3, "BP_Armor_Modular_Core_Head_KettleHelm_3", "Kettle Hat III")
A("h_kettle4", "head", 3, "BP_Armor_Modular_Core_Head_KettleHelm_4", "Kettle Hat IV")
A("h_kettle5", "head", 3, "BP_Armor_Modular_Core_Head_KettleHelm_5", "Kettle Hat V")
A("h_kettle_c", "head", 3, "BP_Armor_Modular_Core_Head_KettleHelm_C_001", "Kettle Hat C")
A("h_kettle_f", "head", 3, "BP_Armor_Modular_Core_Head_KettleHelm_F_004", "Kettle Hat F")
A("h_kettle_g", "head", 3, "BP_Armor_Modular_Core_Head_KettleHelm_G_004", "Kettle Hat G")
A("h_eisen_aa", "head", 3, "BP_Armor_Modular_Core_Head_Eisenhut_AA_001", "Eisenhut AA")
A("h_eisen_ab", "head", 3, "BP_Armor_Modular_Core_Head_Eisenhut_AB_001", "Eisenhut AB")
A("h_eisen_b", "head", 3, "BP_Armor_Modular_Core_Head_Eisenhut_B_002", "Eisenhut B")
A("h_eisen_g", "head", 3, "BP_Armor_Modular_Core_Head_Eisenhut_G_002", "Eisenhut G")
A("h_sallet1", "head", 4, "BP_Armor_Modular_Core_Head_Sallet_1", "Sallet I")
A("h_sallet2", "head", 4, "BP_Armor_Modular_Core_Head_Sallet_2", "Sallet II")
A("h_sallet3", "head", 4, "BP_Armor_Modular_Core_Head_Sallet_3", "Sallet III")
A("h_sallet4", "head", 4, "BP_Armor_Modular_Core_Head_Sallet_4", "Sallet IV")
A("h_sallet_oa1", "head", 4, "BP_Armor_Modular_Core_Head_Sallet_Open_A_001", "Open Sallet A1")
A("h_sallet_oa2", "head", 4, "BP_Armor_Modular_Core_Head_Sallet_Open_A_002", "Open Sallet A2")
A("h_sallet_oa3", "head", 4, "BP_Armor_Modular_Core_Head_Sallet_Open_A_003", "Open Sallet A3")
A("h_sallet_sa1", "head", 5, "BP_Armor_Modular_Core_Head_Sallet_Solid_A_001", "Sallet A1")
A("h_sallet_sa3", "head", 5, "BP_Armor_Modular_Core_Head_Sallet_Solid_A_003", "Sallet A3")
A("h_sallet_sa4", "head", 5, "BP_Armor_Modular_Core_Head_Sallet_Solid_A_004", "Sallet A4")
A("h_sallet_sb3", "head", 5, "BP_Armor_Modular_Core_Head_Sallet_Solid_B_003", "Sallet B3")
A("h_sallet_sb4", "head", 5, "BP_Armor_Modular_Core_Head_Sallet_Solid_B_004", "Sallet B4")
A("h_sallet_sc1", "head", 5, "BP_Armor_Modular_Core_Head_Sallet_Solid_C_001", "Sallet C1")
A("h_sallet_sc2", "head", 5, "BP_Armor_Modular_Core_Head_Sallet_Solid_C_002", "Sallet C2")
A("h_sallet_va1", "head", 5, "BP_Armor_Modular_Core_Head_Sallet_Visor_A_001", "Visored Sallet A1")
A("h_sallet_va2", "head", 5, "BP_Armor_Modular_Core_Head_Sallet_Visor_A_002", "Visored Sallet A2")
A("h_sallet_vc2", "head", 5, "BP_Armor_Modular_Core_Head_Sallet_Visor_C_002", "Visored Sallet C2")
A("h_sallet_vd2", "head", 5, "BP_Armor_Modular_Core_Head_Sallet_Visor_D_002", "Visored Sallet D2")
A("h_barbute1", "head", 5, "BP_Armor_Modular_Core_Head_Barbute_1", "Barbute")
A("h_barbute_b", "head", 5, "BP_Armor_Modular_Core_Head_Barbute_B_001", "Barbute B")
A("h_armet", "head", 6, "BP_Armor_Modular_Core_Head_Armet", "Armet")
A("bv_1", "bevor", 3, "BP_Armor_Modular_Core_Neck_Bevor_1", "Bevor I")
A("bv_2", "bevor", 3, "BP_Armor_Modular_Core_Neck_Bevor_2", "Bevor II")
A("bv_15", "bevor", 3, "BP_Armor_Modular_Core_Neck_Bevor_15", "Bevor 15")
A("bv_16", "bevor", 3, "BP_Armor_Modular_Core_Neck_Bevor_16", "Bevor 16")
A("bv_17", "bevor", 3, "BP_Armor_Modular_Core_Neck_Bevor_17", "Bevor 17")
A("bv_18", "bevor", 3, "BP_Armor_Modular_Core_Neck_Bevor_18", "Bevor 18")
A("bv_19", "bevor", 3, "BP_Armor_Modular_Core_Neck_Bevor_19", "Bevor 19")
A("bv_20", "bevor", 3, "BP_Armor_Modular_Core_Neck_Bevor_20", "Bevor 20")
A("bv_21", "bevor", 3, "BP_Armor_Modular_Core_Neck_Bevor_21", "Bevor 21")
A("bv_22", "bevor", 3, "BP_Armor_Modular_Core_Neck_Bevor_22", "Bevor 22")
A("n_standart", "collar", 2, "BP_Armor_Modular_Core_Neck_Standart_1", "Mail Standart")
A("s_pauldron", "shoulders", 4, "BP_Armor_Modular_Core_Shoulders_Pauldron_1", "Pauldrons")
A("s_spaulder2", "shoulders", 3, "BP_Armor_Modular_Core_Shoulders_Spaulder_2", "Spaulders II")
A("s_spaulder3", "shoulders", 3, "BP_Armor_Modular_Core_Shoulders_Spaulder_3", "Spaulders III")
A("s_spaulder4", "shoulders", 3, "BP_Armor_Modular_Core_Shoulders_Spaulder_4", "Spaulders IV")
A("s_spaulder5", "shoulders", 3, "BP_Armor_Modular_Core_Shoulders_Spaulder_5", "Spaulders V")
A("s_spaulder6", "shoulders", 3, "BP_Armor_Modular_Core_Shoulders_Spaulder_6", "Spaulders VI")
A("s_spaulder7", "shoulders", 3, "BP_Armor_Modular_Core_Shoulders_Spaulder_7", "Spaulders VII")
A("s_spaulder8", "shoulders", 3, "BP_Armor_Modular_Core_Shoulders_Spaulder_8", "Spaulders VIII")
A("s_spaulder_a", "shoulders", 3, "BP_Armor_Modular_Core_Shoulders_Spaulder_A", "Spaulders A")
A("ar_harness1", "arms", 1, "BP_Armor_Modular_Core_Arms_Harness_001", "Arm Harness I")
A("ar_harness2", "arms", 1, "BP_Armor_Modular_Core_Arms_Harness_002", "Arm Harness II")
A("ar_harness3", "arms", 1, "BP_Armor_Modular_Core_Arms_Harness_003", "Arm Harness III")
A("ar_vambrace1", "arms", 3, "BP_Armor_Modular_Core_Arms_Vambrace_1", "Vambraces I")
A("ar_vambrace2", "arms", 3, "BP_Armor_Modular_Core_Arms_Vambrace_2", "Vambraces II")
A("ar_vambrace3", "arms", 3, "BP_Armor_Modular_Core_Arms_Vambrace_3", "Vambraces III")
A("ar_vambrace4", "arms", 3, "BP_Armor_Modular_Core_Arms_Vambrace_4", "Vambraces IV")
A("ar_vambrace5", "arms", 3, "BP_Armor_Modular_Core_Arms_Vambrace_5", "Vambraces V")
A("ar_vambrace6", "arms", 3, "BP_Armor_Modular_Core_Arms_Vambrace_6", "Vambraces VI")
A("ar_vambrace7", "arms", 3, "BP_Armor_Modular_Core_Arms_Vambrace_7", "Vambraces VII")
A("g_gauntlet1", "hands", 3, "BP_Armor_Modular_Core_Hands_Gauntlets_1", "Gauntlets I")
A("g_gauntlet4", "hands", 3, "BP_Armor_Modular_Core_Hands_Gauntlets_4", "Gauntlets IV")
A("g_gauntlet10", "hands", 3, "BP_Armor_Modular_Core_Hands_Gauntlets_10", "Gauntlets X")
A("g_half1", "hands", 2, "BP_Armor_Modular_Core_Hands_HalfGauntlets_1", "Half Gauntlets I")
A("g_half2", "hands", 2, "BP_Armor_Modular_Core_Hands_HalfGauntlets_2", "Half Gauntlets II")
A("b_shirt", "body", 0, "BP_Armor_Modular_Core_Body_Shirt_1", "Shirt")
A("b_tunic", "body", 0, "BP_Armor_Modular_Core_Body_Tunic_1", "Tunic")
A("b_doublet1", "body", 1, "BP_Armor_Modular_Core_Body_Doublet_1", "Doublet I")
A("b_doublet2", "body", 1, "BP_Armor_Modular_Core_Body_Doublet_2", "Doublet II")
A("b_doublet3", "body", 1, "BP_Armor_Modular_Core_Body_Doublet_3", "Doublet III")
A("b_arming", "body", 1, "BP_Armor_Modular_Core_Body_Doublet_Arming", "Arming Doublet")
A("b_arming2", "body", 1, "BP_Armor_Modular_Core_Body_Doublet_Arming_2", "Arming Doublet II")
A("b_gambeson", "body", 2, "BP_Armor_Modular_Core_Chest_Gambeson_1", "Gambeson")
A("b_jack", "body", 2, "BP_Armor_Modular_Core_Chest_Jack_1", "Jack")
A("m_hauberk", "mail", 4, "BP_Armor_Modular_Core_Body_Hauberk_1", "Mail Hauberk")
A("c_breast1", "chest", 5, "BP_Armor_Modular_Core_Chest_Breastplate_1", "Breastplate I")
A("c_breast2", "chest", 5, "BP_Armor_Modular_Core_Chest_Breastplate_2", "Breastplate II")
A("c_breast3", "chest", 5, "BP_Armor_Modular_Core_Chest_Breastplate_3", "Breastplate III")
A("c_breast4", "chest", 5, "BP_Armor_Modular_Core_Chest_Breastplate_4", "Breastplate IV")
A("c_breast5", "chest", 5, "BP_Armor_Modular_Core_Chest_Breastplate_5", "Breastplate V")
A("c_breast6", "chest", 5, "BP_Armor_Modular_Core_Chest_Breastplate_6", "Breastplate VI")
A("c_breast10", "chest", 5, "BP_Armor_Modular_Core_Chest_Breastplate_10", "Breastplate X")
A("c_breast18", "chest", 5, "BP_Armor_Modular_Core_Chest_Breastplate_18", "Breastplate XVIII")
A("c_brust1", "chest", 5, "BP_Armor_Modular_Core_Chest_Cuirass_Brust_1", "Brust I")
A("c_brust2", "chest", 5, "BP_Armor_Modular_Core_Chest_Cuirass_Brust_2", "Brust II")
A("c_cuirass1", "chest", 6, "BP_Armor_Modular_Core_Chest_Cuirass_1", "Cuirass I")
A("c_cuirass2", "chest", 6, "BP_Armor_Modular_Core_Chest_Cuirass_2", "Cuirass II")
A("c_gothic5", "chest", 7, "BP_Armor_Modular_Core_Chest_Cuirass_Gothic_5", "Gothic Cuirass V")
A("c_gothic6", "chest", 7, "BP_Armor_Modular_Core_Chest_Cuirass_Gothic_6", "Gothic Cuirass VI")
A("t_tabard", "tabard", 0, "BP_Armor_Modular_Core_Body_Tabard_1", "Tabard")
A("wa_faulds", "waist", 2, "BP_Armor_Modular_Core_Waist_MailFoulds_1", "Mail Faulds")
A("l_hosen1", "legs", 0, "BP_Armor_Mocular_Core_Legs_Hosen_1", "Hosen I")
A("l_hosen2", "legs", 0, "BP_Armor_Mocular_Core_Legs_Hosen_2", "Hosen II")
A("l_hosen3", "legs", 0, "BP_Armor_Mocular_Core_Legs_Hosen_3", "Hosen III")
A("l_trousers1", "legs", 0, "BP_Armor_Mocular_Core_Legs_Trousers_1", "Trousers I")
A("l_trousers2", "legs", 0, "BP_Armor_Mocular_Core_Legs_Trousers_2", "Trousers II")
A("th_cuisse", "thighs", 3, "BP_Armor_Modular_Core_Legs_Cuisse", "Cuisses I")
A("th_cuisse2", "thighs", 3, "BP_Armor_Modular_Core_Legs_Cuisse_2", "Cuisses II")
A("th_cuisse3", "thighs", 3, "BP_Armor_Modular_Core_Legs_Cuisse_3", "Cuisses III")
A("th_cuisse4", "thighs", 3, "BP_Armor_Modular_Core_Legs_Cuisse_4", "Cuisses IV")
A("sh_greaves", "shins", 3, "BP_Armor_Modular_Core_Legs_Greaves", "Greaves")
A("sh_poleyn", "shins", 2, "BP_Armor_Modular_Core_Legs_Poleyn_1", "Poleyns")
A("f_shoes1", "feet", 0, "BP_Armor_Modular_Core_Feet_Shoes_1", "Shoes I")
A("f_shoes2", "feet", 0, "BP_Armor_Modular_Core_Feet_Shoes_2", "Shoes II")
A("f_shoes3", "feet", 0, "BP_Armor_Modular_Core_Feet_Shoes_3", "Shoes III")

-- --- weapons --------------------------------------------------------------
W("w_arming1", "1h", 3, "ModularWeaponBP_ArmingSword_T1", "Arming Sword I")
W("w_arming2", "1h", 4, "ModularWeaponBP_ArmingSword_T2", "Arming Sword II")
W("w_arming3", "1h", 5, "ModularWeaponBP_ArmingSword_T3", "Arming Sword III")
W("w_falchion_s1", "1h", 3, "ModularWeaponBP_Falchion_Short_T1", "Short Falchion I")
W("w_falchion_s2", "1h", 4, "ModularWeaponBP_Falchion_Short_T2", "Short Falchion II")
W("w_falchion_s3", "1h", 5, "ModularWeaponBP_Falchion_Short_T3", "Short Falchion III")
W("w_falchion_l1", "1h", 4, "ModularWeaponBP_Falchion_Long_T1", "Long Falchion I")
W("w_falchion_l2", "1h", 5, "ModularWeaponBP_Falchion_Long_T2", "Long Falchion II")
W("w_falchion_l3", "1h", 6, "ModularWeaponBP_Falchion_Long_T3", "Long Falchion III")
W("w_bastard1", "1h", 5, "ModularWeaponBP_BastardSword_T1", "Bastard Sword I")
W("w_bastard2", "1h", 6, "ModularWeaponBP_BastardSword_T2", "Bastard Sword II")
W("w_bastard3", "1h", 7, "ModularWeaponBP_BastardSword_T3", "Bastard Sword III")
W("w_longsword1", "2h", 6, "ModularWeaponBP_LongSword_T1", "Longsword I")
W("w_longsword2", "2h", 7, "ModularWeaponBP_LongSword_T2", "Longsword II")
W("w_longsword3", "2h", 8, "ModularWeaponBP_LongSword_T3", "Longsword III")
W("w_greatsword", "2h", 9, "ModularWeaponBP_GreatSword", "Greatsword")
W("w_dagger1", "dagger", 1, "ModularWeaponBP_Dagger_T1", "Dagger I")
W("w_dagger2", "dagger", 2, "ModularWeaponBP_Dagger_T2", "Dagger II")
W("w_dagger3", "dagger", 2, "ModularWeaponBP_Dagger_T3", "Dagger III")
W("w_rondel", "dagger", 2, "Reforged/ModularWeaponBP_DaggerRondel", "Rondel Dagger")
W("w_axe", "1h", 3, "Reforged/ModularWeaponBP_Axe", "Axe")
W("w_axe2h", "2h", 6, "Reforged/ModularWeaponBP_Axe2H", "Great Axe")
W("w_waraxe1", "2h", 4, "Reforged/ModularWeaponBP_Axe_1", "War Axe I")
W("w_waraxe2", "2h", 4, "Reforged/ModularWeaponBP_Axe_2", "War Axe II")
W("w_waraxe3", "2h", 4, "Reforged/ModularWeaponBP_Axe_3", "War Axe III")
W("w_waraxe4", "2h", 4, "Reforged/ModularWeaponBP_Axe_4", "War Axe IV")
W("w_waraxe5", "2h", 4, "Reforged/ModularWeaponBP_Axe_5", "War Axe V")
W("w_waraxe6", "2h", 4, "Reforged/ModularWeaponBP_Axe_6", "War Axe VI")
W("w_hatchet_a1", "1h", 2, "Reforged/ModularWeaponBP_Axe_Hulbat_A_001", "Hatchet A1")
W("w_hatchet_a2", "1h", 2, "Reforged/ModularWeaponBP_Axe_Hulbat_A_002", "Hatchet A2")
W("w_hatchet_b1", "1h", 2, "Reforged/ModularWeaponBP_Axe_Hulbat_B_001", "Hatchet B1")
W("w_hatchet_b2", "1h", 2, "Reforged/ModularWeaponBP_Axe_Hulbat_B_002", "Hatchet B2")
W("w_hatchet_c1", "1h", 2, "Reforged/ModularWeaponBP_Axe_Hulbat_C_001", "Hatchet C1")
W("w_hatchet_c2", "1h", 2, "Reforged/ModularWeaponBP_Axe_Hulbat_C_002", "Hatchet C2")
W("w_hatchet_d1", "1h", 2, "Reforged/ModularWeaponBP_Axe_Hulbat_D_001", "Hatchet D1")
W("w_halberd_a", "2h", 6, "Reforged/ModularWeaponBP_Halberd_A", "Halberd A")
W("w_halberd_b", "2h", 6, "Reforged/ModularWeaponBP_Halberd_B", "Halberd B")
W("w_halberd_c", "2h", 6, "Reforged/ModularWeaponBP_Halberd_C", "Halberd C")
W("w_halberd_d", "2h", 6, "Reforged/ModularWeaponBP_Halberd_D", "Halberd D")
W("w_billhook_a", "2h", 5, "Reforged/ModularWeaponBP_Billhook_A", "Billhook A")
W("w_billhook_b", "2h", 5, "Reforged/ModularWeaponBP_Billhook_B", "Billhook B")
W("w_billhook_c", "2h", 5, "Reforged/ModularWeaponBP_Billhook_C", "Billhook C")
W("w_billhook_d", "2h", 5, "Reforged/ModularWeaponBP_Billhook_D", "Billhook D")
W("w_spear_a", "2h", 4, "Reforged/ModularWeaponBP_Spear_A", "Spear A")
W("w_spear_b", "2h", 4, "Reforged/ModularWeaponBP_Spear_B", "Spear B")
W("w_spear_c", "2h", 4, "Reforged/ModularWeaponBP_Spear_C", "Spear C")
W("w_spear_d", "2h", 4, "Reforged/ModularWeaponBP_Spear_D", "Spear D")
W("w_staff", "2h", 1, "Reforged/ModularWeaponBP_Staff", "Quarterstaff")
W("w_warstaff_a", "2h", 3, "Reforged/ModularWeaponBP_WarStaff_A", "War Staff A")
W("w_warstaff_b", "2h", 3, "Reforged/ModularWeaponBP_WarStaff_B", "War Staff B")
W("w_flail_a", "2h", 5, "Reforged/ModularWeaponBP_Flail_A", "Flail A")
W("w_flail_b", "2h", 5, "Reforged/ModularWeaponBP_Flail_B", "Flail B")
W("w_flail_c", "2h", 5, "Reforged/ModularWeaponBP_Flail_C", "Flail C")
W("w_flail_d", "2h", 5, "Reforged/ModularWeaponBP_Flail_D", "Flail D")
W("w_messer_a", "1h", 2, "Reforged/BP_Weapon_Reforged_Baurnwehr_A", "Bauernwehr A")
W("w_messer_a2", "1h", 2, "Reforged/BP_Weapon_Reforged_Baurnwehr_A_002", "Bauernwehr A2")
W("w_messer_b", "1h", 2, "Reforged/BP_Weapon_Reforged_Baurnwehr_B", "Bauernwehr B")
W("w_messer_c", "1h", 2, "Reforged/BP_Weapon_Reforged_Baurnwehr_C", "Bauernwehr C")
W("w_messer_d", "1h", 2, "Reforged/BP_Weapon_Reforged_Baurnwehr_D", "Bauernwehr D")
W("w_messer_e", "1h", 2, "Reforged/BP_Weapon_Reforged_Baurnwehr_E", "Bauernwehr E")
W("w_messer_f", "1h", 2, "Reforged/BP_Weapon_Reforged_Baurnwehr_F", "Bauernwehr F")
W("w_mace_ls", "1h", 3, "Tiers/ModularWeaponBP_Mace_Low_Tier_Short", "Mace (low)")
W("w_mace_ms", "1h", 4, "Tiers/ModularWeaponBP_Mace_Mid_Tier_Short", "Mace (mid)")
W("w_mace_hs", "1h", 5, "Tiers/ModularWeaponBP_Mace_High_Tier_Short", "Mace (high)")
W("w_mace_ma", "1h", 4, "Tiers/ModularWeaponBP_Mace_Mid_Tier_Avg", "Long Mace (mid)")
W("w_mace_ha", "1h", 5, "Tiers/ModularWeaponBP_Mace_High_Tier_Avg", "Long Mace (high)")
W("w_mace_ll", "2h", 4, "Tiers/ModularWeaponBP_Mace_Low_Tier_Long", "War Mace (low)")
W("w_mace_ml", "2h", 5, "Tiers/ModularWeaponBP_Mace_Mid_Tier_Long", "War Mace (mid)")
W("w_mace_hl", "2h", 6, "Tiers/ModularWeaponBP_Mace_High_Tier_Long", "War Mace (high)")
W("w_hammer_ls", "1h", 3, "Tiers/ModularWeaponBP_Hafted_Low_Tier_Short", "War Hammer (low)")
W("w_hammer_ms", "1h", 4, "Tiers/ModularWeaponBP_Hafted_Mid_Tier_Short", "War Hammer (mid)")
W("w_hammer_hs", "1h", 5, "Tiers/ModularWeaponBP_Hafted_High_Tier_Short", "War Hammer (high)")
W("w_hammer_ma", "1h", 4, "Tiers/ModularWeaponBP_Hafted_Mid_Tier_Avg", "Horseman Hammer (mid)")
W("w_hammer_ha", "1h", 5, "Tiers/ModularWeaponBP_Hafted_High_Tier_Avg", "Horseman Hammer (high)")
W("w_hammer_ll", "2h", 4, "Tiers/ModularWeaponBP_Hafted_Low_Tier_Long", "Long Hammer (low)")
W("w_hammer_ml", "2h", 5, "Tiers/ModularWeaponBP_Hafted_Mid_Tier_Long", "Long Hammer (mid)")
W("w_hammer_hl", "2h", 6, "Tiers/ModularWeaponBP_Hafted_High_Tier_Long", "Long Hammer (high)")
W("w_poleaxe_l", "2h", 5, "Tiers/ModularWeaponBP_Polearm_Low_Tier", "Poleaxe (low)")
W("w_poleaxe_m", "2h", 6, "Tiers/ModularWeaponBP_Polearm_Mid_Tier", "Poleaxe (mid)")
W("w_poleaxe_h", "2h", 7, "Tiers/ModularWeaponBP_Polearm_High_Tier", "Poleaxe (high)")
W("t_axe_a", "2h", 1, "Tools/BP_Weapon_Tool_Axe_A", "Wood Axe A")
W("t_axe_b", "2h", 1, "Tools/BP_Weapon_Tool_Axe_B", "Wood Axe B")
W("t_axe_c", "1h", 1, "Tools/BP_Weapon_Tool_Axe_C", "Hand Axe C")
W("t_axe_d", "1h", 1, "Tools/BP_Weapon_Tool_Axe_D", "Hand Axe D")
W("t_hammer_a", "1h", 1, "Tools/BP_Weapon_Tool_Hammer_A", "Hammer A")
W("t_hammer_b", "1h", 1, "Tools/BP_Weapon_Tool_Hammer_B", "Hammer B")
W("t_hammer_c", "1h", 1, "Tools/BP_Weapon_Tool_Hammer_C", "Hammer C")
W("t_mallet_a", "1h", 1, "Tools/BP_Weapon_Tool_Mallet_A", "Mallet A")
W("t_mallet_b", "1h", 1, "Tools/BP_Weapon_Tool_Mallet_B", "Mallet B")
W("t_mallet_c", "1h", 1, "Tools/BP_Weapon_Tool_Mallet_C", "Mallet C")
W("t_knife_a", "dagger", 0, "Tools/BP_Weapon_Tool_Knife_A", "Knife A")
W("t_knife_b", "dagger", 0, "Tools/BP_Weapon_Tool_Knife_B", "Knife B")
W("t_knife_c", "dagger", 0, "Tools/BP_Weapon_Tool_Knife_C", "Knife C")
W("t_chisel_b", "dagger", 0, "Tools/BP_Weapon_Tool_Chisel_B", "Chisel B")
W("t_chisel_c", "dagger", 0, "Tools/BP_Weapon_Tool_Chisel_C", "Chisel C")
W("t_scissors", "dagger", 0, "Tools/BP_Weapon_Tool_Scissors", "Shears")
W("t_tongs", "1h", 0, "Tools/BP_Weapon_Tool_Tongs", "Tongs")
W("t_sickle_a", "1h", 1, "Tools/BP_Weapon_Tool_Sickle_A", "Sickle A")
W("t_sickle_b", "1h", 1, "Tools/BP_Weapon_Tool_Sickle_B", "Sickle B")
W("t_sickle_c", "1h", 1, "Tools/BP_Weapon_Tool_Sickle_C", "Sickle C")
W("t_sickle_d", "1h", 1, "Tools/BP_Weapon_Tool_Sickle_D", "Sickle D")
W("t_sickle_e", "2h", 1, "Tools/BP_Weapon_Tool_Sickle_E", "Long Sickle")
W("t_hoe_a", "2h", 1, "Tools/BP_Weapon_Tool_Hoe_A", "Hoe A")
W("t_hoe_b", "1h", 1, "Tools/BP_Weapon_Tool_Hoe_B", "Hoe B")
W("t_pitchfork_a", "2h", 1, "Tools/BP_Weapon_Tool_Pitchfork_A", "Pitchfork A")
W("t_pitchfork_b", "2h", 1, "Tools/BP_Weapon_Tool_Pitchfork_B", "Pitchfork B")
W("t_scythe", "2h", 2, "Tools/BP_Weapon_Tool_Scythe_A", "Scythe")
W("t_shovel_a", "2h", 1, "Tools/BP_Weapon_Tool_Shovel_A", "Shovel A")
W("t_shovel_b", "2h", 1, "Tools/BP_Weapon_Tool_Shovel_B", "Shovel B")
W("t_pickaxe_a", "2h", 1, "Tools/BP_Weapon_Tool_Pickaxe_A", "Pickaxe A")
W("t_pickaxe_b", "2h", 1, "Tools/BP_Weapon_Tool_Pickaxe_B", "Pickaxe B")
W("t_maul", "2h", 2, "Tools/BP_Weapon_Tool_Maul", "Maul")
W("t_rake", "2h", 0, "Tools/BP_Weapon_Tool_Rake", "Rake")
W("t_flail", "2h", 1, "Tools/ModularWeaponBP_Tool_Flail", "Threshing Flail")
W("t_lid", "shield", 0, "Tools/BP_Weapon_Tool_Lid_A", "Pot Lid")
W("i_lid", "shield", 0, "Improvized/BP_Weapon_Improv_Barrel_Lid_Small", "Barrel Lid")
W("i_stool", "2h", 0, "Improvized/BP_Weapon_Improv_Stool", "Stool")
W("i_candle_big", "2h", 0, "Improvized/BP_Weapon_Improv_CandleStick_Big", "Big Candlestick")
W("i_candle_small", "1h", 0, "Improvized/BP_Weapon_Improv_CandleStick_Small", "Candlestick")
W("i_lantern", "1h", 0, "Improvized/BP_Weapon_Improv_Lantern", "Lantern")
W("s_buckler", "shield", 2, "Shield_Buckler", "Buckler")
W("s_buckler2", "shield", 2, "Shield_Buckler_2", "Buckler II")
W("s_buckler3", "shield", 3, "Shield_Buckler_3", "Buckler III")
W("s_bossgrip", "shield", 3, "Shield_BossGrip", "Boss-grip Shield")
W("s_targe", "shield", 4, "Shield_Tagre", "Targe")
W("s_pavise_l", "shield", 4, "Shield_Pavise_Light", "Light Pavise")
W("s_pavise_h", "shield", 5, "Shield_Pavise_Heavy", "Heavy Pavise")
W("s_pavise_t", "shield", 6, "Shield_Pavise_Tower", "Tower Pavise")

-- --- classes (preset kits) ------------------------------------------------
K("knight", "KNIGHT", "w_longsword3", "", { "h_armet", "bv_1", "s_pauldron", "ar_vambrace1", "g_gauntlet1", "b_arming2", "c_cuirass1", "wa_faulds", "l_hosen2", "th_cuisse", "sh_greaves", "f_shoes2" }, "Full plate and a longsword. Slow, nearly cut-proof.")
K("man_at_arms", "MAN-AT-ARMS", "w_poleaxe_m", "", { "h_sallet_oa1", "n_standart", "b_gambeson", "m_hauberk", "t_tabard", "g_half1", "l_hosen2", "f_shoes2" }, "Mail over padding and a poleaxe.")
K("duelist", "DUELIST", "w_arming3", "s_buckler3", { "h_hat2", "b_arming", "g_half2", "l_hosen1", "f_shoes1" }, "Light and quick: arming sword and buckler.")
K("brute", "BRUTE", "w_axe2h", "", { "h_cap1", "b_gambeson", "g_half1", "l_trousers1", "f_shoes3" }, "Gambeson and a great axe.")
K("peasant", "PEASANT", "t_pitchfork_b", "", { "b_tunic", "l_hosen3" }, "Rags and a pitchfork. Nothing to lose.")

-- --- helpers (shared by the UI and the applier) ---------------------------

function C.item_cost(id)
    local it = id and C.items[id]
    return it and it.cost or 0
end

-- Total cost of a selection { r=, l=, armor = { ids } }.
function C.kit_cost(sel)
    local n = C.item_cost(sel.r) + C.item_cost(sel.l)
    for _, id in ipairs(sel.armor or {}) do n = n + C.item_cost(id) end
    return n
end

-- A fresh selection copied from a class kit.
function C.class_selection(class_id)
    local k = C.class_by_id[class_id]
    if not k then return nil end
    local armor = {}
    for i, id in ipairs(k.armor) do armor[i] = id end
    return { class = k.id, r = k.r, l = k.l, armor = armor }
end

-- Mirrors the server's validation (loadout.rs validate_kit) so the UI can
-- show problems before the server does. Returns ok, reason.
function C.check(sel, mode, budget)
    if sel.class == "none" then
        if mode == C.MODE_FREE then return true, "" end
        return false, "game default gear is only allowed in FREE mode"
    end
    if not C.class_by_id[sel.class] then return false, "unknown class" end
    local seen = {}
    for _, id in ipairs(sel.armor or {}) do
        local it = C.items[id]
        if not it or it.kind ~= "armor" then return false, "unknown armour " .. tostring(id) end
        if seen[it.group] then return false, "two items in " .. it.group end
        seen[it.group] = true
    end
    local r, l = sel.r ~= "" and C.items[sel.r] or nil, sel.l ~= "" and C.items[sel.l] or nil
    if sel.r ~= "" and (not r or r.kind ~= "weapon") then return false, "unknown right-hand weapon" end
    if sel.l ~= "" and (not l or l.kind ~= "weapon") then return false, "unknown left-hand weapon" end
    if r and r.group == "shield" then return false, "shields go in the left hand" end
    if l and l.group == "2h" then return false, "two-handed weapons go in the right hand" end
    if r and r.group == "2h" and l then return false, "left hand must be empty with a two-handed weapon" end
    if mode == C.MODE_CLASSES then
        local k = C.class_selection(sel.class)
        local a, b = {}, {}
        for _, id in ipairs(k.armor) do a[id] = true end
        for _, id in ipairs(sel.armor) do b[id] = true end
        for id in pairs(a) do if not b[id] then return false, "classes only: kit was edited" end end
        for id in pairs(b) do if not a[id] then return false, "classes only: kit was edited" end end
        if k.r ~= sel.r or k.l ~= sel.l then return false, "classes only: weapons were edited" end
    elseif mode == C.MODE_CUSTOM then
        local cost = C.kit_cost(sel)
        if cost > budget then return false, string.format("over budget (%d > %d)", cost, budget) end
    end
    return true, ""
end

-- Items that fit a hand: right = 1h/2h/dagger, left = 1h/dagger/shield.
function C.hand_list(side)
    local out = {}
    local groups = side == "R" and { "1h", "2h", "dagger" } or { "1h", "dagger", "shield" }
    for _, g in ipairs(groups) do
        for _, id in ipairs(C.order[g] or {}) do out[#out + 1] = id end
    end
    return out
end

return C
