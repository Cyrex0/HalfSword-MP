local P=dofile(T.path("mods/HSMPLoadout/Scripts/native_weapon_presets.lua"))
local C=dofile(T.path("mods/HSMPLoadout/Scripts/hsmp_catalog.lua"))
local ids={"native_pollaxe","native_longsword","native_bastard_sword","native_arming_sword",
    "native_mace","native_baron_sword","native_baron_mace"}
local private=true
for _,id in ipairs(ids)do private=private and C.items[id]==nil and P.descriptor(id).selectable==false end
T.check(private,"mixed base-class recipes stay private until exact server source admission exists")
local pole=P.descriptor("native_pollaxe")
T.check(pole.asset=="/Game/Blueprints/DataAssets/Equipment/FreeMode/DA_FreeMode_Inventory_Tier_Knight.DA_FreeMode_Inventory_Tier_Knight"
    and pole.index==1 and pole.passport.name=="Pollaxe" and pole.passport.mat_steel==0
    and pole.passport.head=="@Weapons/Poleaxes/Modules/Head/Tiers/High_Tier/Modular_PA_Head_Center_High_Tier_002"
    and pole.passport.grip=="@Weapons/Poleaxes/Modules/Haft/Tiers/High_Tier/Modular_Weapon_Polearm_Haft_High_Tier_Avg",
    "Pollaxe proof retains exact asset entry, head/grip and enum Names value rather than numeric suffix")
local baron=P.descriptor("native_baron_mace")
T.check(baron.map_key==3 and baron.key_name=="SheathSlots_Enum::NewEnumerator2"
    and baron.passport.head=="@Weapons/Casted/Modules/Heads/Mace/Weapon_Module_Casted_Head_Mace_B",
    "Baron Mace uses native enum key3 and the exact authored head")
local generation,source,reads,matches=0,{record=pole.passport},0,true
local resolver=P.new({generation=function()return generation end,
    source=function(d)reads=reads+1;return source end,
    read_record=function(s)return s.record end,
    matches=function()return matches end})
local r=resolver.resolve("native_pollaxe")
T.check(r and P.matches_record("native_pollaxe",r),"complete exact native source yields the authored full passport")
r.head_size[1]=9
T.check(resolver.resolve("native_pollaxe").head_size[1]==.8,"resolved plain record cannot mutate the stored proof")
for _,change in ipairs({{"head","wrong-head"},{"mass_head",nil},{"mat_steel",1},{"color_wood",{1,1,1}},{"head_sub1",nil}})do
    local record=P.descriptor("native_pollaxe").passport;record[change[1]]=change[2];source={record=record}
    T.check(resolver.resolve("native_pollaxe")==nil,"changed or unavailable "..change[1].." cannot borrow an authored recipe")
end
source={record=P.descriptor("native_pollaxe").passport};matches=false
T.check(resolver.resolve("native_pollaxe")==nil,"encoded completeness cannot replace matching the original native source")
matches=true;source=nil
T.check(resolver.resolve("native_pollaxe")==nil,"unavailable exact asset does not choose a merchant or another native recipe")
local before=reads
T.check(resolver.resolve("missing_recipe")==nil and reads==before,"unknown recipe cannot perform source reads")
local changed=P.new({generation=function()return generation end,
    source=function()generation=generation+1;return {record=pole.passport}end,
    read_record=function()error("must not read old native source")end,matches=function()return true end})
T.check(changed.resolve("native_pollaxe")==nil,"context change during source acquisition refuses before struct reads")
local after=P.new({generation=function()return generation end,source=function()return {record=pole.passport}end,
    read_record=function(s)return s.record end,matches=function()generation=generation+1;return true end})
T.check(after.resolve("native_pollaxe")==nil,"context change during verification cannot publish mixed proof")
local fractional=P.descriptor("native_pollaxe").passport;fractional.tier=fractional.tier+1e-9
T.check(not P.matches_record("native_pollaxe",fractional),"integer recipe identity is exact even within one float wire unit")
