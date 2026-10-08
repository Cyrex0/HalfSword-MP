-- Lifecycle/input regressions. Synthetic source frames are not native display evidence.
local Core=dofile("mods/HSMPMatch/Scripts/native_client_core.lua")
local Input=dofile("mods/HSMPMatch/Scripts/native_client_input.lua")
local n=0
local function check(ok,why)n=n+1;T.check(ok,why);assert(ok,why)end
local time,state,closed,cleared,applied,sent=0,"",0,0,0,{}
local directory={epoch=77,seq=4,state=1,arena="Map_Arena_Yard"}
local scene={epoch=77,dir_seq=4,frame_seq=10,state=1,peer_id=9001,entities={{epoch=77,id=1,incarnation=3,owner_peer=9001,kind=0,position={0,0,0}},{epoch=77,id=2,incarnation=2,owner_peer=9002,kind=0,position={100,0,0}}}}
local connected,available,isolated,render_ok=true,true,true,true
local core=Core.new({now=function()return time end,link=function()return{connected=connected,error=""}end,directory=function()return directory end,
    world=function()return{ready=true,arena="Map_Arena_Yard",key="world1"}end,isolated=function()return isolated end,scene=function()return available and scene or nil end,
    present=function()applied=applied+1;return render_ok,not render_ok and"provider refused"or nil end,
    clear=function()cleared=cleared+1 end,close=function()closed=closed+1 end,travel=function()error("unexpected travel")end,
    report=function(s)state=s end,input=function()return{1,0,0,0,0,0,0,0},0 end,send=function(f)sent[#sent+1]=f;return true end})
check(core:tick()and state=="mirror_ready","source scene must apply before readiness")
check(#sent==0,"READY cannot send human inputs")
scene.state=2;directory.state=2;check(core:tick()and state=="live"and#sent==1,"LIVE sends ordinary raw input")
check(sent[1].epoch==77 and sent[1].id==1 and sent[1].incarnation==3,"original exact owned reference")
scene.dir_seq=3;check(not core:tick()and#sent==1,"old directory frame cannot drive input")
scene.dir_seq=4;time=1.2;core:tick();check(sent[2].seq==2,"input sequence advances")
connected=false;check(not core:tick()and cleared>=2,"disconnect drops visual scope")
connected=true;scene.entities[1].incarnation=4;directory.seq=5;scene.dir_seq=5;core:tick();check(sent[3].incarnation==4 and sent[3].seq==3,"reconnect uses newref without replaying sequence")
available=false;check(not core:tick()and state=="wait_scene","missing complete scene cannot remain live")
available=true;render_ok=false;check(not core:tick()and closed==1 and core.stopped,"provider failure closes and never fabricates readiness")
local bad=Core.new({now=function()return 0 end,link=function()return{connected=true}end,directory=function()return directory end,world=function()return{ready=true,arena=directory.arena,key="world"}end,
    isolated=function()return false end,report=function()end,clear=function()end,close=function()closed=closed+1 end,scene=function()error("must refuse before scene")end})
check(not bad:tick()and bad.stopped,"local combat world refuses before mirror creation")
local map={keys={"W","MouseX","LeftShift"},axes={{{key=1,scale=1}},{},{{key=2,scale=-1}},{},{},{},{},{}},actions={{{key=3,mods={}}},{},{},{},{},{},{}}}
local a,b=Input.frame(map,{{1,1},{4,0},{1,1}});check(a[1]==1 and a[3]==-4 and b==1,"engine mapping scales and held actions")
a,b=Input.frame(map,{{0,0},{0,0},{0,0}});check(a[1]==0 and b==0,"actual release clears held values")
check(Input.frame(map,{})==nil,"incomplete key state refuses")
local observed,reported,frame
local applied_scene={epoch=77,dir_seq=5,frame_seq=12,state=2,peer_id=9001,entities=scene.entities}
local race=Core.new({now=function()return 0 end,link=function()return{connected=true}end,directory=function()return directory end,world=function()return{ready=true,arena=directory.arena,key="world"}end,
    isolated=function()return true end,scene=function()return scene end,present=function()return true,applied_scene end,
    report=function(_,_,s)reported=s end,input=function(s)observed=s;return{0,0,0,0,0,0,0,0},0 end,
    send=function(f)frame=f;return true end,clear=function()end,close=function()error("must not close")end})
check(race:tick()and observed==applied_scene and reported==applied_scene and frame.incarnation==4,"input and metrics use the actual applied frame during network advancement")
local refused=Core.new({now=function()return 0 end,link=function()return{connected=true}end,directory=function()return directory end,world=function()return{ready=true,arena=directory.arena,key="world"}end,
    isolated=function()return true end,scene=function()return scene end,present=function()return nil,"client mirror generation"end,
    report=function(s)state=s end,clear=function()end,close=function()error("generation race must wait")end})
check(not refused:tick()and not refused.stopped and state=="wait_scene","a directory race clears incomplete mirrors and waits")
local key,clean,travels="old",false,0
local travel=Core.new({now=function()return 0 end,link=function()return{connected=true}end,directory=function()return directory end,world=function()return{ready=true,arena=directory.arena,key=key}end,
    isolated=function()return clean end,scene=function()return scene end,present=function()return true end,
    travel=function()travels=travels+1;return true end,report=function()end,clear=function()end,close=function()error("isolated travel should converge")end,
    input=function()return{0,0,0,0,0,0,0,0},0 end,send=function()return true end})
check(not travel:tick()and travels==1,"same arena still travels to the isolated game mode")
check(not travel:tick()and travels==1,"pending travel cannot repeat against the old world")
key="new";clean=true;check(travel:tick()and travels==1,"new isolated world can create mirrors")
print(string.format("native_client: %d checks passed",n))
