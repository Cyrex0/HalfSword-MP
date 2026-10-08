local M=dofile(T.path("mods/HSMPCombat/Scripts/native_damage_response.lua"))
local c={world="Yard",drops=2,match_id=99,round=1,attacker=2,attacker_life=3,victim=1,victim_life=4,hit_id=8}
local args={};for _,f in ipairs(M.FIELDS)do args[f[1]]={[f[2]]=f[3]=="boolean"and false or 1}end
args[23]["Lower Threshold Out"]=false
local row=M.capture(args,true,c)
T.check(row.complete and row.context_available and row.authority==false,"all exact scalar outputs retain availability without authority")
T.check(row.fields["Lower Threshold Out"].value==false,"native false is retained")
args[19]["Damage Out"]=20;c.hit_id=9
T.check(row.fields["Damage Out"].value==1 and row.context.hit_id==8,"response owns copied scalar values")
for _,bad in ipairs({0/0,math.huge,"20",{}})do
    args[19]["Damage Out"]=bad;row=M.capture(args,true,c)
    T.check(not row.complete and not row.fields["Damage Out"].available,"non-native numeric output refuses completeness")
end
args[19]["Damage Out"]=20;args[18]["Hit Surface"]=1.5
T.check(not M.capture(args,true,c).complete,"byte output requires an actual byte")
args[18]["Hit Surface"]=255
row=M.capture(args,false,c)
T.check(not row.complete and not row.call_ok and row.fields["Damage Out"].value==20,"call failure retains copied observations without claiming complete output")
args[23]={};row=M.capture(args,true,c)
T.check(not row.complete and not row.fields["Lower Threshold Out"].available,"missing boolean never invents false")
args[23]=nil
T.check(not M.capture(args,true,c).fields["Lower Threshold Out"].available,"missing output slot cannot invent false")
args[23]=false
T.check(not M.capture(args,true,c).fields["Lower Threshold Out"].available,"non-table output slot cannot invent false")
args[23]={lower_threshold=false}
T.check(not M.capture(args,true,c).complete,"wrong output spelling never supplies a native field")
local touches=0;local poisoned=setmetatable({},{__index=function()touches=touches+1;error("native wrapper")end})
args[23]=poisoned;c.victim_life=poisoned;row=M.capture(args,true,c)
T.check(not row.complete and not row.context_available and touches==0,"unknown wrapper values are neither invoked nor copied")
row=M.capture(nil,true,nil)
T.check(not row.complete and not row.context_available and not row.fields["Lower Threshold Out"].available,
    "missing invocation evidence is explicit")
