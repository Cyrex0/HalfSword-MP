local M=dofile(T.path("mods/HSMPCombat/Scripts/replay_batch.lua"))
local R=dofile(T.path("mods/HSMPCombat/Scripts/replay_attempts.lua"))
local function event(id,peer,life,ts)
    return {peer=peer or 1,data={match_id=99,round=1,attacker_life=1,victim_life=life or 1,
        hit_id=id,attacker_ts=ts or id,bone="hand_r"}}
end
local function ids(rows)local t={};for _,e in ipairs(rows)do t[#t+1]=e.data.hit_id end;return table.concat(t,",")end
for _,sequence in ipairs({{1,3,2},{17,18,21,19,22,20},{31,32,34,35,33}})do
    local rows={};for _,id in ipairs(sequence)do rows[#rows+1]=event(id)end
    local before=ids(rows);local ordered,moved=M.order(rows)
    local sorted={table.unpack(sequence)};table.sort(sorted)
    T.check(ids(ordered)==table.concat(sorted,",")and moved>0,"observed inversion sorts only the returned batch")
    T.check(ids(rows)==before,"incoming array and records remain unchanged")
    local calls=0;local cache=R.new{cap=32,now=function()return 0 end,send=function()end}
    for _,e in ipairs(ordered)do cache.run(e.data,e.peer,function()calls=calls+1;return "changed",1 end)end
    T.check(calls==#sequence,"ordered batch executes every unique approved record once")
end
local a,b,c=event(3),event(9,2),event(2)
local rows={a,b,c};local ordered=M.order(rows)
T.check(ordered[1]==c and ordered[2]==b and ordered[3]==a,"inter-stream slots remain unchanged")
local other=event(1,1,2);ordered=M.order({a,other,c})
T.check(ordered[2]==other,"different victim life cannot borrow ordering context")
other=event(1);other.data.attacker_life=2;ordered=M.order({a,other,c})
T.check(ordered[2]==other,"different attacker life remains independent")
other=event(1);other.data.match_id=100;ordered=M.order({a,other,c})
T.check(ordered[2]==other,"different match remains independent")
other=event(1);other.data.round=2;ordered=M.order({a,other,c})
T.check(ordered[2]==other,"different round remains independent")
local first,duplicate=event(2),event(2);first.data.bone="head";duplicate.data.bone="foot"
ordered=M.order({event(3),first,duplicate})
T.check(ordered[1]==first and ordered[2]==duplicate,"equal IDs retain original first payload")
local calls=0;local cache=R.new{cap=2,now=function()return 0 end,send=function()end}
local function apply()calls=calls+1;return "changed",1 end
for _,e in ipairs(ordered)do cache.run(e.data,e.peer,apply)end
T.check(calls==2,"same-batch duplicate cannot repeat native execution")
cache.run(event(1).data,1,apply)
T.check(calls==2,"older IDs in later batches retain existing watermark refusal")
cache.clear();calls=0
for _,e in ipairs(M.order({event(1),event(3),event(2)}))do cache.run(e.data,1,apply)end
local _,expired=cache.run(event(1).data,1,apply)
T.check(calls==3 and expired.status==7,"evicted IDs remain retired after ordering")
local wrap={event(1,1,1,12),event(4294967295,1,1,11)};ordered=M.order(wrap)
T.check(ids(ordered)=="4294967295,1","unambiguous serial wrap sorts with increasing source timestamps")
cache.clear();calls=0;for _,e in ipairs(ordered)do cache.run(e.data,1,apply)end
cache.run(event(4294967294).data,1,apply)
T.check(calls==2,"prior across-wrap replay refusal remains intact")
local cases={
    {event(2147483649),event(1)}, -- half-range ambiguity
    {event(3,1,1,100),event(2,1,1,110)}, -- decreasing timestamp if sorted
    {event(3),event(2,1,1,0)},
    {event(0),event(2)},
    {event(4294967296),event(2)},
}
local invalid=event(2);invalid.data.attacker_life=nil;cases[#cases+1]={event(3),invalid}
for _,batch in ipairs(cases)do
    local out,moved=M.order(batch)
    T.check(out==batch and moved==0,"unknown, ambiguous or timestamp-regressing batch preserves arrival behavior")
end
local large={};for i=1,M.MAX_EVENTS+1 do large[i]=event(i)end
T.check(M.order(large)==large,"oversize input is untouched")
local sparse={[1]=event(3),[3]=event(2)}
T.check(M.order(sparse)==sparse,"sparse input is untouched")
