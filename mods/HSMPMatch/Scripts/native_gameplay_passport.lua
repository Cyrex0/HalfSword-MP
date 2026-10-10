-- Exact copied source passports for a caller-owned, deferred native Willie.
-- The caller's original pawn/world scope surrounds every engine operation.
-- No defaults, class-family inference, CDO reads or loadout substitutions.
local source=(debug.getinfo(1,"S").source or ""):gsub("^@","")
local directory=source:match("^(.*)[/\\]") or "."
local D=dofile(directory.."/native_source_descriptor.lua")
local F=D.FIELDS
local M={}
local equipment_fields={armor="ArmorinSlots_5_BD7AC6CB43FBB2FDB943E7864486F358",
    sheaths="WeaponsinSlots_11_B42349384F5EF74DE78A7F870D89656A",hands="WeaponinHands_23_B3FE643741AF91A6DFE51888205C0F05"}
local condition_fields={"HeadHealth_2_61859BB444171EF8952E0FA5DD8628EE","NeckHealth_4_C658DC6A4BD1988C40F1A5B3C4F8F4EE",
    "ArmRHealth_9_A65DD4C14ACBF6030A2B3AAD90FD0CFD","ArmLHealth_11_32345C31454A51B3CDE618918B9574F6",
    "BodyUpperHealth_16_F71EA0C742135DC3B4F71EA3FEF07C46","BodyLowerHealth_18_37C008FF4FA0C0E5F5E09C9F0C174FE3",
    "LegRHealth_13_D50D4E174859A541DBEA66963D162E12","LegLHealth_15_41C766B5460596C0804EA5B4B8F8EB36"}
local construction_fields={{"is_zombie","Is Zombie?","bool"},{"scale_mutation_inhibitor","Scale Mutation Inhibitor","num"},
    {"spawn_in_pants","Spawn in Pants","bool"},{"bolts_in_quiver","Bolts in Quiver","int"},
    {"blossfechten_gear","Blossfechten Gear","bool"},{"character_scale","Character Scale (Set in BP)","vec"},
    {"height_rate","Height Rate","num"},{"muscle_rate","Muscle Rate","num"},{"mass_scale","Mass Scale (Set in BP)","num"}}
local sheath_fields={"Weapon Slot R 1","Weapon Slot R 2","Weapon Slot Back","Weapon Slot L 1","Weapon Slot L 2"}
local function fail(reason)error(reason,0)end
local function finite(v)return type(v)=="number" and v==v and math.abs(v)~=math.huge end
local function text(v,max,empty)return type(v)=="string" and (empty or #v>0) and #v<=max and not v:find("\0",1,true)end
local function integer(v,low,high)return finite(v) and math.tointeger(v)~=nil and v>=low and v<=high end
local function record(v,keys,label)
    if type(v)~="table" or getmetatable(v)~=nil then fail(label.." table")end
    for key in pairs(v)do if not keys[key]then fail(label.." unknown field "..tostring(key))end end
    for key in pairs(keys)do if rawget(v,key)==nil then fail(label.." missing field "..key)end end
end
local function array(v,max,label)
    if type(v)~="table" or getmetatable(v)~=nil then fail(label.." array")end
    local n=#v;if n>max then fail(label.." bound")end
    for k in pairs(v)do if not integer(k,1,n)then fail(label.." sparse array")end end
    for i=1,n do if rawget(v,i)==nil then fail(label.." sparse array")end end
    return n
end
local function vector(v,n,label)
    if array(v,n,label)~=n then fail(label.." width")end
    for i=1,n do if not finite(v[i])then fail(label.." number")end end
end
local function asset(v,empty)
    if not text(v,512,empty) or (v~="" and (v:sub(1,1)~="/" or not v:find(".",1,true)
        or v:find(":",1,true) or v:find("Transient",1,true) or v:find(" ",1,true)))then fail("passport class path")end
end
local validate_passport
local function rows(v,max,kind,label)
    array(v,max,label);local previous=-1
    for _,row in ipairs(v)do
        record(row,{slot=true,[kind=="flags" and "value" or "passport"]=true},label)
        if not integer(row.slot,0,max-1) or row.slot<=previous then fail(label.." slot")end;previous=row.slot
        if kind=="flags"then if type(row.value)~="boolean"then fail(label.." boolean")end
        else validate_passport(kind,row.passport)end
    end
end
validate_passport=function(kind,value)
    local fields=F[kind];if not fields then fail("passport kind")end
    local keys={};for _,field in ipairs(fields)do keys[field[3]]=true end
    record(value,keys,kind.." passport")
    for _,field in ipairs(fields)do
        local t,v=field[2],value[field[3]]
        if t=="class"then asset(v,true)
        elseif t=="name"then if not text(v,128,true)then fail("passport name")end
        elseif t=="vec" or t=="color"then vector(v,t=="vec" and 3 or 4,"passport "..field[3])
        elseif t=="bool"then if type(v)~="boolean"then fail("passport boolean "..field[3])end
        elseif t=="int" or t=="byte"then if not integer(v,t=="int" and -0x80000000 or 0,t=="int" and 0x7fffffff or 255)then fail("passport integer "..field[3])end
        elseif t=="num"then if not finite(v)then fail("passport number "..field[3])end
        elseif t=="flags"then rows(v,17,"flags","blocked slots")
        elseif t=="equipment"then
            record(v,{armor=true,sheaths=true,hands=true},"construction equipment")
            rows(v.armor,17,"armor","construction armor");rows(v.sheaths,6,"weapon","construction sheaths");rows(v.hands,2,"weapon","construction hands")
        else fail("passport field type")end
    end
end
local function validate(recipe)
    if type(recipe)~="table" or recipe.schema~=D.SCHEMA then fail("gameplay source recipe schema")end
    asset(recipe.actor_class,false);if not integer(recipe.team,-0x80000000,0x7fffffff)then fail("gameplay source team")end
    validate_passport("character",recipe.passport)
    local c=recipe.construction;local keys={start_body_condition=true,actor_scale=true}
    for _,field in ipairs(construction_fields)do keys[field[1]]=true end;record(c,keys,"construction")
    vector(c.start_body_condition,8,"body condition");vector(c.actor_scale,3,"actor scale")
    for _,field in ipairs(construction_fields)do local v=c[field[1]]
        if field[3]=="bool"then if type(v)~="boolean"then fail("construction boolean")end
        elseif field[3]=="int"then if not integer(v,-0x80000000,0x7fffffff)then fail("construction integer")end
        elseif field[3]=="vec"then vector(v,3,"character scale")
        elseif not finite(v)then fail("construction number")end
    end
    local e=recipe.equipment;record(e,{armor=true,weapons=true,hands=true,sheaths=true},"live equipment")
    rows(e.armor,17,"armor","live armor")
    for _,row in ipairs(e.armor)do if row.passport.class==""then fail("live armor class")end end
    array(e.weapons,32,"live weapons");local ids={}
    for _,w in ipairs(e.weapons)do
        record(w,{id=true,actor_class=true,passport=true,components=true},"live weapon")
        if not integer(w.id,1,32) or ids[w.id]then fail("live weapon id")end
        ids[w.id]=w;asset(w.actor_class,false);validate_passport("weapon",w.passport)
        array(w.components,64,"weapon components")
        local prior=0;for _,id in ipairs(w.components)do if not integer(id,1,64) or id<=prior then fail("weapon component id")end;prior=id end
    end
    array(e.hands,2,"live hands");local prior=-1
    for _,r in ipairs(e.hands)do record(r,{slot=true,item=true},"live hand")
        if not integer(r.slot,0,1) or r.slot<=prior or not ids[r.item]then fail("live hand binding")end;prior=r.slot end
    array(e.sheaths,5,"live sheaths");local seen={}
    for _,r in ipairs(e.sheaths)do record(r,{field=true,item=true},"live sheath")
        local allowed=false;for _,key in ipairs(sheath_fields)do if key==r.field then allowed=true end end
        if not allowed or seen[r.field] or not ids[r.item]then fail("live sheath binding")end;seen[r.field]=true
    end
    return {actor_class=recipe.actor_class,team=recipe.team,passport=recipe.passport,construction=c,equipment=e}
end
local function guard(env)
    if type(env)~="table" or type(env.guard)~="function" or env.guard()~=true then fail("gameplay passport scope changed")end
end
local function checked(env,fn)guard(env);local value=fn();guard(env);return value end
local function current(env)
    return checked(env,function()
        if type(env.current)~="function"then fail("gameplay original pawn unavailable")end
        local p=env.current();if not p or p:GetAddress()==0 then fail("gameplay original pawn unavailable")end;return p
    end)
end
local function class_path(env,o)
    return checked(env,function()
        if not o then fail("native class unavailable")end
        if o:GetAddress()==0 then return ""end
        if type(env.class_path)=="function"then return env.class_path(o)end
        return o:GetFullName():match("^%S+%s+(.+)$")
    end)
end
local function class(env,path)
    local o=checked(env,function()
        if path==""then
            if type(env.null_class)~="function"then fail("explicit native null UClass unavailable")end
            return env.null_class()
        end
        if type(env.resolve_class)~="function"then fail("native class resolver unavailable")end
        return env.resolve_class(path)
    end)
    if not o or checked(env,function()return o:type()end)~="UClass" or class_path(env,o)~=path then fail("native passport class mismatch")end
    if path~="" and checked(env,function()return o:IsValid()end)~=true then fail("native passport class invalid")end
    return o
end
local function native_vector(value,color)
    if color then return {R=value[1],G=value[2],B=value[3],A=value[4]}end
    return {X=value[1],Y=value[2],Z=value[3]}
end
local marshal
marshal=function(kind,value,env)
    local out={}
    for _,field in ipairs(F[kind])do
        local t,v=field[2],value[field[3]]
        if t=="class"then v=class(env,v)
        elseif t=="name"then v=checked(env,function()
            if type(env.fname)~="function"then fail("native FName factory unavailable")end;return env.fname(v)
        end);if v==nil then fail("native FName unavailable")end
        elseif t=="vec" or t=="color"then v=native_vector(v,t=="color")
        elseif t=="flags"then local copied={};for _,r in ipairs(v)do copied[r.slot]=r.value end;v=copied
        elseif t=="equipment"then local copied={}
            for _,key in ipairs({"armor","sheaths","hands"})do local map={}
                for _,r in ipairs(v[key])do map[r.slot]=marshal(key=="armor" and "armor" or "weapon",r.passport,env)end
                copied[equipment_fields[key]]=map
            end;v=copied
        end
        out[field[1]]=v
    end
    return out
end
local function reader(env)
    return {guard=function()guard(env);return true end,class_path=function(o)return class_path(env,o)end,
        name=function(o)return checked(env,function()return o:ToString()end)end,
        unwrap=function(v)if type(v)=="number" or type(v)=="boolean"then return v end;return v:get()end}
end
local function equal(actual,expected,label)
    if D.signature(actual)~=D.signature(expected)then fail(label.." differs from source")end
end
local function write(env,key,value)
    checked(env,function()current(env)[key]=value end)
end
function M.marshal(kind,record_value,env)
    local ok,value=pcall(function()validate_passport(kind,record_value);return marshal(kind,record_value,env)end)
    if not ok then return nil,value end;return value
end
-- A null ClassProperty getter constructs real UClass(nullptr) userdata in the
-- pinned UE4SS pusher. No CDO/null assumption: observe this actual hard leaf.
-- TMap offers no Lua header witness, so do not walk borrowed entries here.
function M.find_null_class(env)
    local ok,value=pcall(function()
        local o=checked(env,function()return current(env)["Character Passport"][F.character[1][1]]end)
        if not o or o:type()~="UClass" or o:GetAddress()~=0 then
            fail("explicit native null UClass not observed in deferred Character Passport; protected map search unavailable")
        end
        guard(env);return o
    end)
    if not ok then return nil,value end;return value
end
function M.before_finish(recipe,env)
    local ok,why=pcall(function()
        local p=validate(recipe);local signature=D.signature(p)
        local actual=checked(env,function()return current(env):GetClass()end)
        if class_path(env,actual)~=p.actor_class then fail("native deferred pawn class mismatch")end
        local native=marshal("character",p.passport,env)
        if D.signature(validate(recipe))~=signature then fail("gameplay source recipe changed")end
        write(env,"Character Passport",native);write(env,"Team Int",p.team)
        local condition={};for i,key in ipairs(condition_fields)do condition[key]=p.construction.start_body_condition[i]end
        write(env,"Start Body Condition",condition)
        for _,field in ipairs(construction_fields)do local v=p.construction[field[1]]
            if field[3]=="vec"then v=native_vector(v,false)end;write(env,field[2],v)
        end
        checked(env,function()current(env):SetActorScale3D(native_vector(p.construction.actor_scale,false))end)
        local actual_passport,reason=D.read_passport("character",function()return current(env)["Character Passport"]end,reader(env))
        if not actual_passport then fail(reason)end;equal(actual_passport,p.passport,"native character passport")
        if checked(env,function()return current(env)["Team Int"]end)~=p.team then fail("native team write mismatch")end
        for i,key in ipairs(condition_fields)do if checked(env,function()return current(env)["Start Body Condition"][key]end)~=p.construction.start_body_condition[i]then fail("native body condition write mismatch")end end
        for _,field in ipairs(construction_fields)do
            if field[3]=="vec"then for i,key in ipairs({"X","Y","Z"})do
                if checked(env,function()return current(env)[field[2]][key]end)~=p.construction[field[1]][i]then fail("native construction vector write mismatch")end
            end
            elseif checked(env,function()return current(env)[field[2]]end)~=p.construction[field[1]]then fail("native construction write mismatch")end
        end
        for i,key in ipairs({"X","Y","Z"})do if checked(env,function()return current(env):GetActorScale3D()[key]end)~=p.construction.actor_scale[i]then fail("native actor scale write mismatch")end end
        if D.signature(validate(recipe))~=signature then fail("gameplay source recipe changed")end;guard(env)
    end)
    if not ok then return nil,why end;return true
end
-- FinishSpawn/Willie's own construction may already initialize gear. Verify first;
-- never invoke a second setup with guessed ClearPrevious/NoCheckBlock controls.
function M.verify_equipment(recipe,env)
    local ok,why=pcall(function()
        local p=validate(recipe);local signature=D.signature(p);local r=reader(env)
        local armor,reason=D.read_armor_map(function()return current(env)["Currently Equipped Armor"]end,r)
        if not armor then fail(reason)end;equal(armor,p.equipment.armor,"native live armor")
        local weapons,fields,addresses={},{},{}
        for _,w in ipairs(p.equipment.weapons)do weapons[w.id]=w end
        for _,h in ipairs(p.equipment.hands)do fields[h.slot==0 and "Weapon R" or "Weapon L"]=h.item end
        for _,s in ipairs(p.equipment.sheaths)do fields[s.field]=s.item end
        for _,key in ipairs({"Weapon R","Weapon L",table.unpack(sheath_fields)})do
            local function actor()return checked(env,function()return current(env)[key]end)end
            local o=actor();if not o then fail("native weapon field unavailable: "..key)end
            local address=checked(env,function()return o:GetAddress()end);local id=fields[key]
            if not id then if address~=0 then fail("native unexpected weapon: "..key)end
            else
                if address==0 then fail("native missing weapon: "..key)end
                if addresses[id] and addresses[id]~=address then fail("native weapon alias changed")end
                for other,value in pairs(addresses)do if other~=id and value==address then fail("native weapon id collision")end end;addresses[id]=address
                local expected=weapons[id]
                local function fresh()
                    local a=actor();if not a or a:GetAddress()~=address or a:IsValid()~=true then fail("native held weapon changed")end
                    if type(env.weapon_guard)~="function" or env.weapon_guard(a,key,id)~=true then fail("native weapon original scope unavailable")end
                    guard(env);return a
                end
                if class_path(env,checked(env,function()return fresh():GetClass()end))~=expected.actor_class then fail("native weapon actor class mismatch")end
                local copied,error_reason=D.read_passport("weapon",function()return fresh()["Weapon Passport"]end,r)
                if not copied then fail(error_reason)end;equal(copied,expected.passport,"native live weapon passport")
                fresh()
            end
        end
        for id in pairs(weapons)do if not addresses[id]then fail("native live weapon unbound")end end
        if D.signature(validate(recipe))~=signature then fail("gameplay source recipe changed")end;guard(env)
    end)
    if not ok then return nil,why end;return true
end
function M.after_finish(recipe,env)
    local ok,why=pcall(function()
        local p=validate(recipe)
        local function passport()
            local copied,reason=D.read_passport("character",function()return current(env)["Character Passport"]end,reader(env))
            if not copied then fail(reason)end;equal(copied,p.passport,"native character passport")
        end
        passport()
        local matched,reason=M.verify_equipment(recipe,env);if matched~=true then fail(reason)end
        passport();guard(env)
    end)
    if not ok then return nil,why end;return true
end
-- These controls are operation arguments, not captured passport fields. The
-- caller must supply a proved native invocation; this helper supplies no defaults.
function M.setup_armor(recipe,env,controls)
    local ok,why=pcall(function()
        validate(recipe);record(controls,{clear_previous=true,no_check_block=true},"native armor controls")
        if type(controls.clear_previous)~="boolean" or type(controls.no_check_block)~="boolean"then fail("native armor controls unsupported")end
        checked(env,function()local pawn=current(env);local fn=pawn["Set Up Armor"]
            if type(fn)~="function"then fail("native Set Up Armor unavailable")end
            fn(pawn,controls.clear_previous,controls.no_check_block)
        end)
    end)
    if not ok then return nil,why end;return M.verify_equipment(recipe,env)
end
function M.setup_hand(record_value,side,env,controls)
    local ok,why=pcall(function()
        record(record_value,{id=true,actor_class=true,passport=true,components=true},"live weapon")
        asset(record_value.actor_class,false);validate_passport("weapon",record_value.passport)
        if side~=0 and side~=1 then fail("native hand slot")end
        record(controls,{dropped_with_no_damage=true,destroy_previous=true,actor=true},"native hand controls")
        if type(controls.dropped_with_no_damage)~="boolean" or type(controls.destroy_previous)~="boolean"then fail("native hand controls unsupported")end
        local pass=marshal("weapon",record_value.passport,env);local cls=class(env,record_value.passport.class)
        checked(env,function()
            if type(env.weapon_guard)~="function" or env.weapon_guard(controls.actor,side==0 and "Weapon R" or "Weapon L",record_value.id)~=true then fail("native weapon original scope unavailable")end
            local pawn=current(env);local fn=pawn[side==0 and "Set Up Right Hand Weapon" or "Set Up Left Hand Weapon"]
            if type(fn)~="function"then fail("native hand setup unavailable")end
            fn(pawn,cls,controls.actor,controls.dropped_with_no_damage,controls.destroy_previous,pass)
        end)
        local field=side==0 and "Weapon R" or "Weapon L"
        local function fresh()
            return checked(env,function()
                local actor=current(env)[field]
                if not actor or env.weapon_guard(actor,field,record_value.id)~=true then fail("native equipped hand scope changed")end
                return actor
            end)
        end
        if class_path(env,checked(env,function()return fresh():GetClass()end))~=record_value.actor_class then fail("native equipped hand class mismatch")end
        local copied,reason=D.read_passport("weapon",function()return fresh()["Weapon Passport"]end,reader(env))
        if not copied then fail(reason)end;equal(copied,record_value.passport,"native equipped hand passport");fresh()
    end)
    if not ok then return nil,why end;return true
end
return M
