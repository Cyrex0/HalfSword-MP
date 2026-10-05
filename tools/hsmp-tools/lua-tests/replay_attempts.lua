local M=dofile(T.path("mods/HSMPCombat/Scripts/replay_attempts.lua"))
local now,calls,sent=0,0,{}
local cache=M.new{cap=2,now=function()return now end,send=function(r)sent[#sent+1]=r end}
local function d(id,life)return{match_id=123,round=3,hit_id=id,victim_life=life or 1}end
local function apply()calls=calls+1;return "changed",1,3,-7 end
local _,r,fresh=cache.run(d(1),8,apply)
T.check(fresh and calls==1 and r.health_delta==-7,"first native execution records observed result")
cache.run(d(1),8,apply)
T.check(calls==1,"lost result duplicate never applies native damage twice")
now=.2;cache.tick()
T.check(#sent==2,"cached result retries after lost acknowledgment")
local wrong={match_id=123,round=3,attacker=8,hit_id=1,victim_life=1,status=2,observed_fields=0,health_delta=0}
T.check(not cache.ack(wrong),"conflicting receipt acknowledgment refused")
T.check(cache.ack(r),"exact acknowledgment settles receipt")
now=.4;cache.tick();T.check(#sent==2,"settled receipt stops retrying")
cache.run(d(2),8,function()calls=calls+1;error("after native invocation")end)
local _,failed=cache.run(d(2),8,apply)
T.check(calls==2 and failed.status==6,"uncertain native error is terminal and idempotent")
cache.run(d(3),8,apply)
local _,expired=cache.run(d(1),8,apply)
T.check(calls==3 and expired.status==7,"evicted old attempt stays below stream watermark")
cache.run(d(1,2),8,apply);T.check(calls==4,"fresh victim generation has independent attempt stream")
cache.clear();cache.run(d(4294967295),8,apply);cache.run(d(1),8,apply)
T.check(calls==6,"wrapped increasing hit ids preserve callback order")
cache.run(d(4294967294),8,apply);T.check(calls==6,"old id across wrap cannot execute")
