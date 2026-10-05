local M=dofile(T.path("mods/HSMPMatch/Scripts/surrender_hold.lua"))
local now=0
local ctx={match_id=81,round=2,life=1,pawn="owner",address=10,world="arena",world_key=20,spawn_id=512}
local sent={}
local fail=false
local progress={}
local S=M.new({now=function()return now end,context=function()return ctx end,
    send=function(r)if fail then return false end sent[#sent+1]=r;return true end,
    accept_actor=function(actor)return actor=="owner"end,
    publish=function(r)progress[#progress+1]=r end})
local hooks={}
S:hooks(function(path,f)hooks[path]=f end)
S:hooks(function()error("registered twice")end)
T.check(hooks[M.PRESS] and hooks[M.RELEASE],"exact native Talk pressed/released hooks register once")
hooks[M.PRESS]("other");now=3;S:tick()
T.check(#sent==0,"remote Talk input cannot surrender local pawn")
hooks[M.PRESS]("owner");now=5.19;S:tick()
T.check(#sent==0,"native opener plus meter requires full 2.2 seconds")
local meter=progress[#progress]
T.check(meter.active and meter.match_id==81 and meter.life==1 and meter.pawn=="owner"
    and meter.progress>0.99 and meter.at_ms==5190,"typed meter retains original proof and fresh process time")
hooks[M.RELEASE]("owner");now=6;S:tick()
T.check(#sent==0,"release before threshold cancels continuous hold")
T.check(progress[#progress].active==false,"release immediately clears visible hold")
hooks[M.PRESS]("owner");now=7;hooks[M.PRESS]("owner");now=8.21;S:tick()
T.check(#sent==1 and sent[1].reason==2 and sent[1].match_id==81 and sent[1].round==2 and sent[1].life==1,
    "key repeats keep original hold and completion sends scoped surrender")
T.check(progress[#progress].active==false,"submitted surrender clears hold meter without native death")
now=12;S:tick();hooks[M.RELEASE]("owner");hooks[M.PRESS]("owner");now=15;S:tick()
T.check(#sent==1,"same-life surrender cannot duplicate even with another hold")
hooks[M.RELEASE]("owner")
for _,key in ipairs({"match_id","round","life","pawn","address","world","world_key","spawn_id"})do
    local old=ctx[key]
    hooks[M.PRESS]("owner")
    ctx[key]=type(old)=="number" and old+1 or old.."new"
    now=now+3;S:tick()
    ctx[key]=old;S:tick()
    T.check(#sent==1,"context change "..key.." cancels hold without rearming")
    hooks[M.RELEASE]("owner")
end
ctx.life=2;hooks[M.PRESS]("owner");local old=ctx;ctx=nil;now=now+3;S:tick();ctx=old;S:tick()
T.check(#sent==1,"session/world/alive invalidation cannot resume original hold")
hooks[M.RELEASE]("owner");hooks[M.PRESS]("owner");fail=true;now=now+3;S:tick()
T.check(#sent==1,"failed IPC acceptance retains completion for retry")
fail=false;S:tick()
T.check(#sent==2 and sent[2].life==2,"retry preserves original full life without native Health writes")
hooks[M.RELEASE]("owner");ctx.life=3;hooks[M.PRESS]("owner");S:cancel();now=now+3;S:tick()
T.check(#sent==2,"world drop cancels immediately even when same arena/address returns between ticks")
local partial={}
local P=M.new({now=function()return now end,context=function()return ctx end,
    send=function()error("unpaired hooks must never send")end})
P:hooks(function(path,f)if path==M.RELEASE then error("release unavailable")end partial[path]=f end)
partial[M.PRESS]("owner");now=now+3;P:tick()
T.check(P.journal==nil,"missing release hook fails closed until both native input edges are available")
do
    local world={address=123,name="World Arena"}
    world.IsValid=function()return true end
    world.GetAddress=function(self)return self.address end
    world.GetFullName=function(self)return self.name end
    local pawnworld=world
    local pawn={IsValid=function()return true end,GetAddress=function()return 456 end,
        GetFName=function()return {ToString=function()return "owner"end}end,
        GetWorld=function()return pawnworld end}
    local view={status="connected",state="live",my_peer_id=1,match_id=81,round=2,
        alive={[1]=true},spawns={[1]={spawn_id=512}}}
    local mode={match_id=81,round=2,rows={[1]={alive=true,life=1}}}
    local status={verified=true,pawn="owner",match_id=81,round=2,life=1,spawn_id=512}
    local ready=true
    local capture=M.make_context({ready=function()return ready end,view=function()return view end,
        mode=function()return mode end,pawn=function()return pawn end,status=function()return status end,
        world=function()return world end,world_key=function()return "World Arena@123#PC456"end})
    T.check(capture(pawn).world_key=="World Arena@123#PC456",
        "production adapter compares native worlds while retaining composite string guard identity")
    pawnworld=setmetatable({address=999,name="World Arena"},{__index=world})
    T.check(capture(pawn)==nil,"production adapter refuses another native world's same name")
    pawnworld.address=123;pawnworld.name="Old World"
    T.check(capture(pawn)==nil,"production adapter refuses recycled world address with old fullname")
    pawnworld=world;status.life=129
    T.check(capture(pawn)==nil,"production adapter refuses aliased spawn id from original older life")
    status.life=1;view.alive[1]=false
    T.check(capture(pawn)==nil,"production adapter cancels when authoritative fighter is no longer alive")
    view.alive[1]=true;ready=false
    T.check(capture(pawn)==nil,"production adapter refuses travel or unsettled native world")
end
