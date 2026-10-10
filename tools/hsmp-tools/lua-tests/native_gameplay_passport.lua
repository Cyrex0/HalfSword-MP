local P=dofile("mods/HSMPMatch/Scripts/native_gameplay_passport.lua")
local D=dofile("mods/HSMPMatch/Scripts/native_source_descriptor.lua")
local function clone(v)if type(v)~="table"then return v end;local out={};for k,x in pairs(v)do out[k]=clone(x)end;return out end
local function fixture(path)local f=assert(io.open(path,"rb"));local v=T.json_decode(f:read("*a"));f:close();return v end
local scene_source=fixture("tools/hsmp-tools/lua-tests/fixtures/native_source_recipe.json")
local source={schema=P.SCHEMA,actor_class=scene_source.actor_class,team=scene_source.team,
    passport=scene_source.passport,construction=scene_source.construction,equipment=scene_source.equipment}
local armor=fixture("tools/hsmp-tools/lua-tests/fixtures/native_armor_passport.json")
local function wrapped(value)return {get=function()return value end}end
local native
local map_calls={empty=0,add=0,find=0,nested=0}
local function map(values,kind)
    local function keys()local result={};for k in pairs(values)do result[#result+1]=k end;table.sort(result);return result end
    return setmetatable({type=function()return "TMap"end,
        Empty=function()map_calls.empty=map_calls.empty+1;values={}end,
        Add=function(_,key,value)
            map_calls.add=map_calls.add+1
            if kind=="armor"then for _,field in ipairs(D.FIELDS.armor)do
                if field[2]=="flags"and value[field[1]]~=nil then
                    map_calls.nested=map_calls.nested+1;error("native nested map reads stack1 instead of flags",0)
                end
            end end
            values[key]=kind and native(kind,value)or value
        end,
        Find=function(_,key)map_calls.find=map_calls.find+1;assert(values[key]~=nil,"Map key not found");return wrapped(values[key])end,
        ForEach=function(_,fn)for _,k in ipairs(keys())do fn(wrapped(k),wrapped(values[k]))end end},
        {__len=function()return #keys()end})
end
local function class(path)
    return {type=function()return "UClass"end,GetAddress=function()return path=="" and 0 or 81 end,
        IsValid=function()return path~=""end,GetFullName=function()return "BlueprintGeneratedClass "..path end}
end
local function name(value)return {ToString=function()return value end}end
local equipment_fields={armor="ArmorinSlots_5_BD7AC6CB43FBB2FDB943E7864486F358",
    sheaths="WeaponsinSlots_11_B42349384F5EF74DE78A7F870D89656A",hands="WeaponinHands_23_B3FE643741AF91A6DFE51888205C0F05"}
native=function(kind,value)
    local out={}
    for _,field in ipairs(D.FIELDS[kind])do
        local v=value[field[1]]
        if field[2]=="flags"then v=map(v or {})
        elseif field[2]=="equipment"then
            local e={};for key,fieldname in pairs(equipment_fields)do local values={}
                for slot,p in pairs(v[fieldname])do values[slot]=native(key=="armor" and "armor" or "weapon",p)end
                e[fieldname]=map(values,key=="armor"and"armor"or"weapon")
            end;v=e
        end
        out[field[1]]=v
    end
    return out
end
local function environment(recipe)
    local alive,writes=true,0;local values={};local calls={};local scale
    local methods={GetAddress=function()return 99 end,GetClass=function()return class(recipe.actor_class)end,
        SetActorScale3D=function(_,v)scale=v end,GetActorScale3D=function()return scale end}
    local character={};local equipment={}
    for key,fieldname in pairs(equipment_fields)do equipment[fieldname]=map({},key=="armor"and"armor"or"weapon")end
    character[D.FIELDS.character[7][1]]=equipment
    character[D.FIELDS.character[1][1]]=class(recipe.actor_class)
    values["Character Passport"]=setmetatable({}, {__index=character,__newindex=function(_,key,value)writes=writes+1;character[key]=value end})
    local pawn=setmetatable({}, {__index=function(_,key)return methods[key] or values[key]end,
        __newindex=function(_,key,value)
            writes=writes+1
            if key=="Character Passport"then map_calls.nested=map_calls.nested+1;error("[push_structproperty] StoredAtIndex1 Weight=0.5",0)end
            values[key]=value
        end})
    local env={guard=function()return alive end,current=function()return pawn end,
        resolve_class=function(path)return class(path)end,null_class=function()return class("")end,fname=name,
        weapon_guard=function()return alive end}
    return env,pawn,values,calls,function(v)alive=v end,function()return writes end,methods
end
local recipe=clone(source)
recipe.passport.height=-0.0;recipe.construction.scale_mutation_inhibitor=-0.0
recipe.passport.equipment.armor={{slot=7,passport=clone(armor)}}
recipe.passport.equipment.armor[1].passport.class="" -- explicit observed-null synthetic construction row
recipe.passport.equipment.armor[1].passport.pslot=2
local env,pawn,values,calls,set_alive,write_count,methods=environment(recipe)
local ok,why=P.before_finish(recipe,env)
T.check(ok==true,"complete deferred native passport/construction assigned: "..tostring(why))
T.check(map_calls.empty==4 and map_calls.add==3 and map_calls.find==3 and map_calls.nested==0,
    "all3 actual Equipment maps and nested blocked map use native Empty/Add/Find, never nested plain-map Set")
T.check(string.pack("<d",values["Character Passport"][D.FIELDS.character[4][1]])==string.pack("<d",-0.0),"source signed-zero height bit preserved")
T.check(values["Character Passport"][D.FIELDS.character[7][1]][equipment_fields.armor]~=nil,"construction equipment map remains present")
T.check(values["Scale Mutation Inhibitor"]==-0.0 and values["Spawn in Pants"]==recipe.construction.spawn_in_pants,"captured construction numbers/false booleans assigned exactly")
local read_env={guard=env.guard,unwrap=function(v)return v:get()end,class_path=function(v)return v:GetAddress()==0 and "" or v:GetFullName():match("^%S+%s+(.+)$")end}
local copied=D.read_passport("character",function()return values["Character Passport"]end,read_env)
T.check(D.signature(copied)==D.signature(recipe.passport),"all11 character/all24 armor fields plus original independent map key/null class roundtrip")
local blocked=copied.equipment.armor[1].passport.slots_blocked
T.check(#blocked==2 and blocked[1].slot==2 and blocked[1].value==false and blocked[2].slot==5 and blocked[2].value==true,
    "staged blocked map preserves false/true and every original enum key")
local old_route,old_reason=pcall(function()pawn["Character Passport"]=assert(P.marshal("character",recipe.passport,env))end)
T.check(not old_route and old_reason:find("StoredAtIndex1 Weight=0.5",1,true),"fixture reproduces actual nested plain-map setter stack failure")
local null,reason=P.find_null_class(env)
T.check(null==nil and reason:find("not observed",1,true),"nonnull deferred class never becomes a manufactured null")
local original_actor_class=values["Character Passport"][D.FIELDS.character[1][1]]
values["Character Passport"][D.FIELDS.character[1][1]]=class("")
null,reason=P.find_null_class(env)
T.check(null~=nil and null:type()=="UClass" and null:GetAddress()==0,"actual typed hard-null class getter supplies explicit null wrapper")
values["Character Passport"][D.FIELDS.character[1][1]]=original_actor_class
local wrong=clone(recipe);wrong.passport.eye_color=nil
local e,_,_,_,_,count=environment(wrong);local refused,reason=P.before_finish(wrong,e)
T.check(refused==nil and reason:find("missing field eye_color",1,true) and count()==0,"omitted native field refuses before writes")
wrong=clone(recipe);wrong.passport.fake=0;e,_,_,_,_,count=environment(wrong);refused,reason=P.before_finish(wrong,e)
T.check(refused==nil and reason:find("unknown field fake",1,true) and count()==0,"unknown passport field refuses before writes")
e,_,_,_,_,count=environment(source);refused,reason=P.before_finish(scene_source,e)
T.check(refused==nil and reason:find("recipe schema",1,true) and count()==0,"old schema6 full-scene recipe is not a compact gameplay bootstrap")
wrong=clone(recipe);wrong.components={};e,_,_,_,_,count=environment(wrong);refused,reason=P.before_finish(wrong,e)
T.check(refused==nil and reason:find("unknown field components",1,true) and count()==0,"gameplay recipe rejects ignored scene topology fields")
wrong=clone(recipe);wrong.passport.equipment.armor[1].passport.pslot=256;e,_,_,_,_,count=environment(wrong);refused,reason=P.before_finish(wrong,e)
T.check(refused==nil and count()==0,"invalid raw enum byte is never coerced")
e,_,_,_,_,count=environment(recipe);e.current=function()return {GetAddress=function()return 99 end,GetClass=function()return class("/Game/Wrong.Wrong_C")end}end
refused,reason=P.before_finish(recipe,e)
T.check(refused==nil and reason:find("pawn class mismatch",1,true) and count()==0,"wrong original deferred actor class refuses")
e,_,_,_,_,count=environment(recipe);e.null_class=nil;refused,reason=P.before_finish(recipe,e)
T.check(refused==nil and reason:find("explicit native null UClass unavailable",1,true) and count()==0,"null class never disappears into nil/skipped default")
e,_,_,_,_,count=environment(recipe);e.resolve_class=function()return class("/Game/Wrong.Wrong_C")end;refused,reason=P.before_finish(recipe,e)
T.check(refused==nil and reason:find("passport class mismatch",1,true) and count()==0,"resolved class path mismatch refuses")
e,_,_,_,set_alive,count=environment(recipe);e.fname=function(v)set_alive(false);return name(v)end;refused,reason=P.before_finish(recipe,e)
T.check(refused==nil and reason:find("scope changed",1,true) and count()==0,"native name callback changing original scope refuses before pawn writes")
local staged_env,staged_pawn,staged_values,_,staged_alive=environment(recipe)
local actual_equipment=staged_values["Character Passport"][D.FIELDS.character[7][1]]
actual_equipment[equipment_fields.armor]=map({[16]={}},"armor")
ok,why=P.before_finish(recipe,staged_env)
local staged_copy=D.read_passport("character",function()return staged_values["Character Passport"]end,read_env)
T.check(ok==true and D.signature(staged_copy)==D.signature(recipe.passport),"native Empty removes original old map rows instead of merging/defaulting them")
staged_env,staged_pawn,staged_values,_,staged_alive=environment(recipe)
local target=staged_values["Character Passport"][D.FIELDS.character[7][1]][equipment_fields.armor]
local original_add=target.Add;local finds_before=map_calls.find
target.Add=function(self,key,value)original_add(self,key,value);staged_alive(false)end
refused,reason=P.before_finish(recipe,staged_env)
T.check(refused==nil and reason:find("scope changed",1,true) and map_calls.find==finds_before,
    "native Add scope change refuses before any borrowed armor Find/get or later field write")
staged_env,staged_pawn,staged_values=environment(recipe)
staged_values["Character Passport"][D.FIELDS.character[7][1]][equipment_fields.armor]={type=function()return "table"end}
refused,reason=P.before_finish(recipe,staged_env)
T.check(refused==nil and reason:find("typed map unavailable",1,true),"unsupported typed-map representation explicitly refuses without plain-map fallback")
local weapon={}
for _,f in ipairs(D.FIELDS.weapon)do
    local kind=f[2]
    weapon[f[3]]=kind=="class" and "" or kind=="name" and "Copied Axe" or kind=="vec" and {1.125,-2.25,3.5}
        or kind=="color" and {0.25,0.5,0.75,1} or kind=="bool" and false or 2
end
weapon.class="/Game/Weapons/Actual.Actual_C";weapon.head="/Game/Modules/ActualHead.ActualHead_C"
local marshaled,error_reason=P.marshal("weapon",weapon,env)
T.check(marshaled~=nil,"complete25-field native weapon marshals: "..tostring(error_reason))
local read_weapon=D.read_passport("weapon",function()return marshaled end,read_env)
T.check(D.signature(read_weapon)==D.signature(weapon),"all25 source weapon fields including nullable classes/color/vector raw values roundtrip")
recipe.equipment.armor={{slot=12,passport=clone(armor)}};recipe.equipment.armor[1].passport.pslot=2
recipe.equipment.weapons={{id=1,actor_class="/Game/Weapons/SpawnedAxe.SpawnedAxe_C",passport=weapon}}
recipe.equipment.hands={{slot=0,item=1}};recipe.equipment.sheaths={{field="Weapon Slot Back",item=1}}
wrong=clone(recipe);wrong.equipment.weapons[1].components={1};e,_,_,_,_,count=environment(wrong);refused,reason=P.before_finish(wrong,e)
T.check(refused==nil and reason:find("unknown field components",1,true) and count()==0,"compact native gear rejects weapon component lists rather than silently ignoring them")
env,pawn,values,calls,set_alive,write_count,methods=environment(recipe)
ok,why=P.before_finish(recipe,env);T.check(ok==true,"source live and construction equipment remain separate inputs")
values["Currently Equipped Armor"]=map({[12]=native("armor",assert(P.marshal("armor",recipe.equipment.armor[1].passport,env)))})
-- Actual hard actor fields expose invalid address0 wrappers for observed nulls.
local absent={GetAddress=function()return 0 end}
for _,key in ipairs({"Weapon R","Weapon L","Weapon Slot R 1","Weapon Slot R 2","Weapon Slot Back","Weapon Slot L 1","Weapon Slot L 2"})do values[key]=absent end
local live={GetAddress=function()return 123 end,IsValid=function()return true end,
    GetClass=function()return class(recipe.equipment.weapons[1].actor_class)end,["Weapon Passport"]=assert(P.marshal("weapon",weapon,env))}
values["Weapon R"]=live;values["Weapon Slot Back"]=live
ok,why=P.verify_equipment(recipe,env)
T.check(ok==true,"live independent armor12/passport2 and aliased held/sheath native weapon verify: "..tostring(why))
ok,why=P.after_finish(recipe,env)
T.check(ok==true,"finish verification retains full source character passport alongside live equipment")
local old_passport=values["Character Passport"]
local old_face=old_passport[D.FIELDS.character[8][1]]
old_passport[D.FIELDS.character[8][1]]=99
refused,reason=P.after_finish(recipe,env)
T.check(refused==nil and reason:find("character passport differs",1,true),"native finish callback changing passport field is not silently accepted")
old_passport[D.FIELDS.character[8][1]]=old_face
T.check(D.signature(copied)==D.signature(recipe.passport),"live gear verification never overwrites construction passport")
values["Weapon L"]=live;refused,reason=P.verify_equipment(recipe,env)
T.check(refused==nil and reason:find("unexpected weapon",1,true),"extra held actor not dropped")
values["Weapon L"]=absent;local old_guard=env.weapon_guard;env.weapon_guard=function()set_alive(false);return true end
refused,reason=P.verify_equipment(recipe,env)
T.check(refused==nil and reason:find("scope changed",1,true),"native held actor callback travel refuses verification")
set_alive(true);env.weapon_guard=old_guard;values["Weapon R"]=nil;refused,reason=P.verify_equipment(recipe,env)
T.check(refused==nil and reason:find("field unavailable",1,true),"unavailable wrapper is never inferred null")
values["Weapon R"]=live;methods["Set Up Armor"]=function(_,clear,no_check)calls[#calls+1]={clear,no_check}end
refused,reason=P.setup_armor(recipe,env,{clear_previous=true})
T.check(refused==nil and #calls==0,"missing native setup control refuses without invented boolean")
ok,why=P.setup_armor(recipe,env,{clear_previous=false,no_check_block=true})
T.check(ok==true and #calls==1 and calls[1][1]==false and calls[1][2]==true,"explicit native method controls passed once then complete live gear verified")
local called
methods["Set Up Right Hand Weapon"]=function(_,cls,actor,dropped,destroy,pass)called={cls,actor,dropped,destroy,pass}end
ok,why=P.setup_hand(recipe.equipment.weapons[1],0,env,{actor=live,dropped_with_no_damage=false,destroy_previous=true})
T.check(ok==true and called[2]==live and called[3]==false and called[4]==true,"exact native five-argument hand method invoked without CDO substitute")
T.check(called[1]:GetFullName():match("^%S+%s+(.+)$")==weapon.class and called[1]:GetFullName():match("^%S+%s+(.+)$")~=recipe.equipment.weapons[1].actor_class,
    "native passport class and actual spawned actor class preserve distinct observed values")
methods["Set Up Right Hand Weapon"]=function()live["Weapon Passport"][D.FIELDS.weapon[2][1]]=77 end
refused,reason=P.setup_hand(recipe.equipment.weapons[1],0,env,{actor=live,dropped_with_no_damage=false,destroy_previous=true})
T.check(refused==nil and reason:find("equipped hand passport differs",1,true),"native setup callback must produce the exact actual source weapon passport")
