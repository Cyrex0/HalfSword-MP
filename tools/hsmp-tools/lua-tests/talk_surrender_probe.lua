local M=dofile(T.path("mods/dev/HSMPParity/Scripts/talk_surrender_probe.lua"))
local now=10
local ctx={match_id=81,round=2,life=130,pawn="owner",address=9,world_key="nativeworld#pc"}
local meter,outcomes=nil,{}
local notes={}
local P=M.new({now=function()return now end,context=function(actor)if actor and actor~="owner"then return nil end return ctx end,
    actor_id=function(actor)return {pawn=actor,address=9}end,my_peer_id=function()return 2 end,
    meter=function()return meter end,outcomes=function()local r=outcomes;outcomes={};return r end,
    note=function(kind,data)notes[#notes+1]={kind=kind,data=data}end})
T.check(not P:arm(),"observer refuses uninstalled native input routes")
local hooks={};P:install(function(path,f)hooks[path]=f end)
T.check(P:arm(),"observer arms only an original verified current life")
hooks[M.PRESS]("other",error)
T.check(P.state=="armed","remote actor cannot start own surrender probe")
hooks[M.PRESS]("owner",error) -- borrowed key intentionally poisonous if touched
T.check(P.state=="holding"and P.journal.life==130,"actual native press captures original full life without touching FKey")
now=11;meter={active=true,match_id=81,round=2,life=130,pawn="owner",at_ms=11000};P:tick()
T.check(P.journal.saw_meter,"fresh original typed meter corroborates native input route")
hooks[M.RELEASE]("other");T.check(P.state=="holding","other actor's release cannot cancel own observation")
hooks[M.RELEASE]("owner");meter.active=false;P:tick()
T.check(P.state=="verified_cancel","real release before native threshold verifies cancellation")
now=13;outcomes={{at=13,data={cause=6,match_id=81,round=2,life=130,peer_id=2}}};P:tick()
T.check(P.state=="unexpected_cancelled_outcome","late outcome after already verified early release remains a cancellation failure")
now=20;P:arm();hooks[M.PRESS]("owner");now=21
meter={active=true,match_id=81,round=2,life=130,pawn="owner",at_ms=21000};P:tick()
now=22.3;hooks[M.RELEASE]("owner")
outcomes={{at=22.3,data={cause=6,match_id=81,round=2,life=2,peer_id=2}}};P:tick()
T.check(P.state=="released_complete","aliased spawn old full life cannot verify surrender")
outcomes={{at=22.3,data={cause=6,match_id=81,round=2,life=130,peer_id=2}}};P:tick()
T.check(P.state=="verified_surrender","actual native hold plus scoped meter and cause6 verifies original route")
now=30;P:arm();hooks[M.PRESS]("owner");ctx.life=131;P:tick()
T.check(P.state=="cancelled"and P.journal==nil,"generation/world invalidation cancels immutable observation")
ctx.life=130;now=40;P:arm();hooks[M.PRESS]("owner");now=43;meter=nil
outcomes={{at=43,data={cause=6,match_id=81,round=2,life=130,peer_id=2}}};P:tick()
T.check(P.state=="outcome_without_verified_meter","a bare surrender event cannot masquerade as full native-route proof")
