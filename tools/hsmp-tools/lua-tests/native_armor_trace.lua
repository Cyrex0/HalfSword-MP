local M=dofile(T.path("mods/HSMPCombat/Scripts/native_armor_trace.lua"))
local now,enabled,reads,formats,guards=1000,true,0,0,0
local logs={}
local ctx={world="100@World current",peer=2,match_id=9,round=3,life=4,pawn="Willie current",actor=20,mesh=21,side="owner"}
local function copy(t)local out={};for k,v in pairs(t)do out[k]=v end;return out end
local detail="objects:11,hits:2,ordered_hits:mesh:Plate|tags:defB5/mesh:Flesh|surface:1"
local inv={context=copy(ctx),pawn=20,attacker=1,hit_id=8,cid=6,parent_cid=0}
local types={GetArrayNum=function()return 1 end,ForEach=function(_,f)f(1,11)end}
local api=M.new{clock=function()return now end,enabled=function()guards=guards+1;return enabled end,
    unwrap=function(v)reads=reads+1;return v end,context=function()return copy(ctx)end,
    format_trace=function()formats=formats+1;return detail end,invocation=function()return inv end,
    log=function(fmt,...)logs[#logs+1]=string.format(fmt,...)end}
local function observe(t)return api.capture(true,{},nil,nil,0.1,t or types,true,{},false)end
observe()
T.check(reads==0 and formats==0 and guards==0,"inactive burst performs no enabled/context/native reads")
T.check(api.start()==true,"explicit independent read-only burst starts")
observe()
T.check(#logs==1 and logs[1]:find("replay_span=known attacker=1 hit_id=8 cid=6 parent_cid=0",1,true),"owner span requires the actual complete native context and active pawn")
T.check(logs[1]:find("dcd_caller=unavailable source_parent=unavailable authority=false",1,true)and logs[1]:find(detail,1,true),"real ordered trace text never becomes source/DCD authority")
local before=formats;observe()
T.check(formats==before,"same-scope interval skips hit formatting on the hot callback")
now=1250;ctx.side="source";observe()
T.check(logs[2]:find("replay_span=unavailable attacker=unavailable",1,true),"source trace cannot inherit owner replay IDs")
now=1500;ctx.side="owner";inv.context.life=5;observe()
T.check(logs[3]:find("replay_span=unavailable",1,true),"changed replay life is unavailable rather than relabeled")
now=1750;detail=string.rep("x",M.MAX_DETAIL_BYTES+1);observe()
T.check(logs[4]:find("trace_text_available=false detail_truncated=true detail=unavailable:detail_byte_limit",1,true),"oversized details explicitly lose availability and never claim completeness")
now=2000;before=formats;observe{GetArrayNum=function()return 1 end,ForEach=function(_,f)f(1,10)end}
T.check(formats==before,"another native object channel is not armor-trace evidence")
local visited=0
observe{GetArrayNum=function()return 1 end,ForEach=function(_,f)
    for i=1,10000 do visited=visited+1;if f(i,11)==true then break end end
end}
T.check(visited==2 and formats==before,"changing type array stops at the first extra entry before formatting")
visited=0
observe{GetArrayNum=function()return 1 end,ForEach=function(_,f)
    for i=1,10000 do visited=visited+1;if f(i,10)==true then break end end
end}
T.check(visited==1 and formats==before,"wrong channel stops native iteration immediately")
local failed=M.new{clock=function()return 2000 end,enabled=function()return true end,unwrap=function(v)return v end,
    context=function()return copy(ctx)end,format_trace=function()ctx.life=ctx.life+1;return "observed"end,
    log=function()error("changed scope must never be logged")end}
failed.start()
T.check(pcall(failed.capture,true,{},nil,nil,1,types,true,{},false),"scope changing during native read discards the row")
api.stop();before=guards;observe()
T.check(guards==before,"explicit stop restores the zero-read hot path")
local count=0
local bounded=M.new{clock=function()return now end,enabled=function()return true end,unwrap=function(v)return v end,
    context=function()return copy(ctx)end,format_trace=function()count=count+1;return "hits:0,out:false"end,log=function()end}
bounded.start()
for _=1,M.MAX_SCOPE_RECORDS+1 do bounded.capture(false,{},nil,nil,1,types,true,{},false);now=now+M.INTERVAL_MS end
T.check(count==M.MAX_SCOPE_RECORDS,"bounded scope budget caps both successful and empty native observations")
now=now+M.BURST_MS;before=count;bounded.capture(false,{},nil,nil,1,types,true,{},false)
T.check(count==before,"expired burst stops observation")
local source=assert(io.open(T.path("mods/HSMPCombat/Scripts/main.lua"),"r"))
local code=source:read("*a");source:close()
local bridge=assert(code:match("(function C3%.native_trace_post.-\nend)"))
local captured
local state={native_probe=false,armor_trace={capture=function(...)captured=table.pack(...)end}}
local env={C3=state};setmetatable(env,{__index=_G})
assert(load(bridge,"native armor bridge","t",env))()
local result,world,hits,ignored={},{},{},{}
state.native_trace_post({},result,world,{},{},1,types,true,{},0,hits,ignored,nil,nil,5)
T.check(captured and captured.n==9 and captured[1]==result and captured[2]==world
    and captured[8]==hits and captured[9]==ignored,"real native POST bridge forwards correct return/world/hit slots even with damage probe disabled")
local P=dofile(T.path("mods/HSMPCombat/Scripts/native_protection_audit.lua"))
local function array(v)return{GetArrayNum=function()return #v end,ForEach=function(_,f)
    for i,e in ipairs(v)do if f(i,e)==true then break end end
end}end
local component={IsValid=function()return true end,GetFullName=function()return "StaticMeshComponent Armor"end,
    ComponentTags=array({{ToString=function()ctx.life=ctx.life+1;return "defB5"end}})}
local material={IsValid=function()return true end,SurfaceType=1}
local native_hit={Component=component,PhysMaterial=material,BoneName={ToString=function()return "head"end},
    Distance=1,bBlockingHit=false,bStartPenetrating=false}
local formatter=P.new{enabled=function()return true end,unwrap=function(v)return v end}
local real_logs=0
local checked=M.new{clock=function()return now end,enabled=function()return true end,unwrap=function(v)return v end,
    context=function()return copy(ctx)end,format_trace=formatter.trace,log=function()real_logs=real_logs+1 end}
checked.start();local initial_life=ctx.life
checked.capture(true,{},{X=0,Y=0,Z=0},{X=0,Y=0,Z=0},1,array({11}),true,array({native_hit}),false)
T.check(ctx.life==initial_life+1 and real_logs==0,"real tag formatter scope change discards the observation instead of relabeling its body life")
