local M=dofile(T.path("mods/HSMPCombat/Scripts/native_topology_audit.lua"))
local function param(v)return {get=function()return v end}end
local function unwrap(v)if type(v)=="table"and v.get then return v:get()end;return v end
local function fn(v)return {ToString=function()return v end}end
local function array(values,count)
    return {GetArrayNum=function()return count or #values end,
        ForEach=function(_,f)for i,v in ipairs(values)do f(i-1,param(v))end end}
end
local function map(values)
    return {ForEach=function(_,f)for _,v in ipairs(values)do f(param(v[1]),param(v[2]))end end}
end
local function copy(v)local t={};for k,x in pairs(v)do t[k]=x end;return t end
local ctx={world="world-generation-7:10@World arena",peer=2,match_id=71,round=3,life=130,pawn="Willie_1",actor=11,mesh=12}
local reads,writes=0,0
local mesh={IsValid=function()return true end,GetAddress=function()return 12 end}
local values={Mesh=mesh,["Dismembered Array"]=array({}),["Dismembered Bones"]=array({}),["Dismembered Parts Map"]=map({})}
for _,k in ipairs(M.FLAGS)do values[k]=false end
local pawn=setmetatable({}, {__index=function(_,k)
    reads=reads+1
    if k=="IsValid"then return function()return true end end
    if k=="GetAddress"then return function()return 11 end end
    if k=="GetFName"then return function()return fn("Willie_1")end end
    return values[k]
end,__newindex=function()writes=writes+1;error("reader wrote native field")end})
local env={context=function()return ctx end,unwrap=unwrap}
local function read()return M.read(pawn,env)end
local r=read()
T.check(r.version==1 and r.read_complete,"versioned completed snapshot is separate from scalar Vitals")
T.check(r.context~=ctx and r.context.world==ctx.world and r.context.life==130 and r.context.mesh==12,
    "original full world/native-component/life tuple is copied as plain identity")
T.check(r.dism_array.available and r.dism_array.count==0 and #r.dism_array.values==0,
    "complete empty native legacy array is distinct from unavailable")
T.check(r.dism_bones.available and r.parts.available and r.parts.count==0 and r.parts.present_mask==0 and r.parts.true_mask==0,
    "independent complete empty native bones and map are explicit")
T.check(r.flags.Headless.available and r.flags.Headless.value==false,"observed false structural flag remains available")
T.check(r.healthy==nil and r.dism==nil and r.sever_mask==nil,"reader never invents healthy state or converts native parts into bone mask")

values["Dismembered Parts Map"]=map({{14,false},{3,true},{1,false}})
values["Dismembered Bones"]=array({fn("LowerArm_R"),fn("None")})
r=read()
T.check(r.parts.values[1].part==1 and r.parts.values[2].part==3 and r.parts.values[3].part==14,
    "actual numeric native enum values are sorted, never interpreted as enumerator suffixes")
T.check(r.parts.present_mask==((1<<1)|(1<<3)|(1<<14))and r.parts.true_mask==1<<3,
    "false-valued present native map entry is distinct from absent or true")
T.check(r.dism_bones.values[1]=="LowerArm_R"and r.dism_bones.values[2]=="None",
    "exact native FNames are preserved without sanitizing or turning None into a guessed root")
T.check(r.dism_array.available and #r.dism_array.values==0 and r.parts.true_mask~=0 and r.dism==nil,
    "empty legacy array with positive completion ledger never becomes a healthy bone mask")
values.Headless=true;values["Dismemberment In Process"]=true;r=read()
T.check(r.read_complete and r.flags.Headless.value and r.flags["Dismemberment In Process"].value,
    "valid positive sever and in-process state are readable evidence without stopping health publication")
values.Headless=nil;r=read()
T.check(not r.read_complete and not r.flags.Headless.available and r.flags.Headless.value==nil and r.parts.available,
    "unavailable structural flag does not become false or erase independent map availability")
values.Headless=false

local original=values["Dismembered Array"]
values["Dismembered Array"]=nil;r=read()
T.check(not r.dism_array.available and r.dism_array.count==nil and r.dism_array.values==nil and r.dism_bones.available,
    "initial missing collection stays unknown while independently valid collections survive")
values["Dismembered Array"]=array({fn("hand_r")},2);r=read()
T.check(not r.dism_array.available and r.dism_array.values==nil,"array native count mismatch cannot publish partial topology")
local n=1
values["Dismembered Array"]={GetArrayNum=function()local old=n;n=n+1;return old end,
    ForEach=function(_,f)f(0,param(fn("hand_r")))end}
r=read();T.check(not r.dism_array.available,"count changing across native iteration remains unavailable")
values["Dismembered Array"]={GetArrayNum=function()return 2 end,ForEach=function(_,f)
    f(0,param(fn("hand_r")));error("native iteration stopped")end}
r=read();T.check(not r.dism_array.available and r.dism_array.values==nil,"mid-iteration failure never publishes cached or partial names")
for _,v in ipairs({false,12,"",string.rep("a",129)})do
    values["Dismembered Array"]=array({fn(v)});r=read()
    T.check(not r.dism_array.available,"invalid native FName remains unavailable")
end
values["Dismembered Array"]=array({},65);r=read()
T.check(not r.dism_array.available and r.dism_array.count==65,"native array exceeding bounded reader capacity is explicit unknown")
values["Dismembered Array"]={GetArrayNum=function()return 1.5 end,ForEach=function()error("must not iterate")end}
T.check(not read().dism_array.available,"unreadable noninteger native count never starts collection read")
values["Dismembered Array"]=original

for _,entries in ipairs({{{15,true}},{{23,true}},{{-1,true}},{{1.5,true}},{{"3",true}},{{3,0}},{{3,"false"}},{{3,true},{3,false}}})do
    values["Dismembered Parts Map"]=map(entries);r=read()
    T.check(not r.parts.available and r.parts.present_mask==nil and r.parts.true_mask==nil and r.parts.values==nil,
        "invalid/duplicate native typed map entry never becomes a partial or guessed sever state")
end
local all={};for part=14,0,-1 do all[#all+1]={part,part%2==0}end
values["Dismembered Parts Map"]=map(all);r=read()
T.check(r.parts.available and r.parts.count==15 and r.parts.present_mask==0x7fff and r.parts.values[1].part==0,
    "all fifteen actual native keys fit bounded snapshot and retain numeric zero")
all[#all+1]={0,true};r=read()
T.check(not r.parts.available and r.parts.values==nil,"overflowed map does not silently truncate to an intact or partial ledger")
values["Dismembered Parts Map"]={ForEach=function(_,f)f(param(3),param(true));error("map changed")end}
T.check(not read().parts.available,"native map iteration failure is independent unknown")
values["Dismembered Parts Map"]=map({{3,true}})
env.map_count=function()return 2 end;r=read()
T.check(not r.parts.available and r.parts.values==nil,"supplied proven native map count must match completed iteration")
env.map_count=function()return 1 end;r=read()
T.check(r.parts.available and r.parts.count_check=="native","available native map count check is recorded explicitly")
env.map_count=function()error("native count unavailable")end
T.check(not read().parts.available,"supplied native map count failure cannot fall back to unverified empty")
env.map_count=nil
T.check(read().parts.count_check=="iteration","reader records bounded ForEach contract without inventing a native count getter")

local base=copy(ctx)
for _,k in ipairs({"world","peer","match_id","round","life","pawn","actor","mesh"})do
    local calls=0
    env.context=function()
        calls=calls+1;local c=copy(base)
        if calls>2 then c[k]=type(c[k])=="string"and c[k].."changed"or c[k]+1 end
        return c
    end
    local mixed,why=read()
    T.check(mixed==nil and why=="scope changed","changed original "..k.." scope drops mixed-lifetime topology")
end
env.context=function()return ctx end
local old=values["Dismembered Array"]
values["Dismembered Array"]={GetArrayNum=function()return 1 end,ForEach=function(_,f)
    f(0,param(fn("hand_r")));ctx=copy(base);ctx.world=base.world.."travel"end}
local previous_reads=reads
local travel,why=read()
T.check(travel==nil and why=="scope changed"and reads-previous_reads==5,
    "world changes during array read abort before later actor properties or final identity access")
values["Dismembered Array"]=old;ctx=copy(base)
env.context=function()return ctx end
for _,bad in ipairs({{life=0},{life=1.5},{life=65536},{match_id=0},{peer=65536},{world=""},{pawn=""},{actor=mesh}})do
    local c=copy(base);for k,v in pairs(bad)do c[k]=v end
    env.context=function()return c end;previous_reads=reads
    T.check(read()==nil and reads==previous_reads,"invalid original scope fails before native object access")
end
env.context=function()return base end
values.Mesh={IsValid=function()return true end,GetAddress=function()return 13 end}
T.check(read()==nil,"exact current native Mesh must match the supplied source component scope")
values.Mesh=mesh
r=read()
local function plain(v)
    if type(v)=="table"then
        if getmetatable(v)~=nil then return false end
        for k,x in pairs(v)do if not plain(k)or not plain(x)then return false end end
        return true
    end
    return type(v)=="string"or type(v)=="number"or type(v)=="boolean"or v==nil
end
T.check(plain(r),"snapshot contains only plain scalar references/data, no UObject, wrapper, function or metatable")
values["Dismembered Parts Map"]=map({});ctx=copy(base);ctx.life=131;env.context=function()return ctx end
local nextlife=read()
T.check(nextlife.context.life==131 and nextlife.parts.count==0 and r.context.life==130 and r.parts.true_mask==1<<3,
    "same native actor new full life has a fresh immutable snapshot without retained topology or relabeling")
T.check(writes==0,"all availability/capture paths leave native damage, physics and scalar Vitals untouched")
