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
local Isolation=dofile("mods/HSMPMatch/Scripts/native_client_isolation.lua")
do
    local valid=true
    local function obj(name,path)
        return{IsValid=function()return true end,GetFName=function()return{ToString=function()return name end}end,
            GetClass=function()return{IsValid=function()return true end,GetFullName=function()return"Class "..path end}end}
    end
    local world=obj("Map_Arena_Yard","/Script/Engine.World")
    world.GetFullName=function()return"World /Game/Maps/Arenas/Map_Arena_Yard.Map_Arena_Yard"end
    world.GetAddress=function()return 1234 end
    local gm=obj("GameModeBase_1","/Script/Engine.GameModeBase");gm.OptionsString="?game=/Script/Engine.GameModeBase"
    local pc=obj("PlayerController_1","/Script/Engine.PlayerController")
    pc.K2_GetPawn=function()return nil end
    pc.SetIgnoreMoveInput=function(self)self.move=true end;pc.SetIgnoreLookInput=function(self)self.look=true end
    pc.IsMoveInputIgnored=function(self)return self.move end;pc.IsLookInputIgnored=function(self)return self.look end
    local mesh=obj("Mesh","/Script/Engine.SkeletalMeshComponent")
    mesh.IsVisible=function()return false end;mesh.IsSimulatingPhysics=function()return false end;mesh.GetCollisionEnabled=function()return 0 end
    local pawn=obj("Willie_BP_C_0","/Game/Character/Blueprints/Willie_BP.Willie_BP_C")
    pawn.Mesh=mesh;pawn.bHidden=true;pawn.GetWorld=function()return world end
    pawn.ActorHasTag=function(_,tag)return tag=="Persistent"end;pawn.GetActorEnableCollision=function()return false end
    local env={WG={token=function()return 1 end,same=function()return valid end,world=function()return world end,pc=function()return pc end},
        UEHelpers={GetGameplayStatics=function()return{IsValid=function()return true end,GetGameMode=function()return gm end}end},
        find_all=function()return{pawn}end,FName=function(s)return s end}
    local ok,why,facts=Isolation.inspect(env)
    check(ok and facts.game_mode_class.value=="/Script/Engine.GameModeBase"and facts.game_mode_options.value==gm.OptionsString,"isolation captures exact native class and options")
    check(facts.willies[1].mesh_visible.known and facts.willies[1].mesh_visible.value==false and facts.willies[1].actor_collision.value==false,"native false values remain known false in diagnostics")
    check(facts.controller_class.value=="/Script/Engine.PlayerController"and facts.move_ignored.value==true and facts.look_ignored.value==true,"controller input suppression requires actual readback")
    gm.GetClass=function()return{IsValid=function()return true end,GetFullName=function()return"Class /Game/Blueprints/Utility/BP_HalfSwordGameMode.BP_HalfSwordGameMode_C"end}end
    ok,why,facts=Isolation.inspect(env)
    check(not ok and why=="isolation_game_mode_mismatch:"..facts.game_mode_class.value,"wrong game mode refuses with exact original class")
    gm.GetClass=function()return{IsValid=function()return true end,GetFullName=function()return"Class /Script/Engine.GameModeBase"end}end
    pawn.ActorHasTag=function()return false end
    ok,why=Isolation.inspect(env);check(not ok and why=="isolation_local_fighter:Willie_BP_C_0","hidden nonpersistent fighters still refuse")
    pawn.ActorHasTag=function()return true end;mesh.IsVisible=function()return 0 end
    ok,why,facts=Isolation.inspect(env);check(not ok and why=="isolation_mesh_visible_unavailable:Willie_BP_C_0"and facts.willies[1].mesh_visible.known==false,"nonboolean native data never becomes a false proof")
    mesh.IsVisible=function()return false end;pawn.GetActorEnableCollision=function()return true end
    ok,why=Isolation.inspect(env);check(not ok and why=="isolation_actor_collision:Willie_BP_C_0","colliding persistent actor refuses")
    pawn.GetActorEnableCollision=function()return false end;mesh.IsSimulatingPhysics=function()return true end
    ok,why=Isolation.inspect(env);check(not ok and why=="isolation_mesh_simulating:Willie_BP_C_0","simulating persistent actor refuses")
    env.find_all=function()return nil end
    ok,why=Isolation.inspect(env);check(not ok and why=="isolation_census_incomplete","missing census is never guessed empty")
    env.find_all=function()local rows={};for i=1,65 do rows[i]=pawn end;return rows end
    ok,why,facts=Isolation.inspect(env);check(not ok and why=="isolation_census_incomplete"and facts.census_error=="Willie census exceeds64","diagnostic census is bounded without truncating into success")
    valid=false;ok,why=Isolation.inspect(env);check(not ok and why=="isolation_world_unavailable","world invalidation refuses before engine getters")
end
do
    local reason,key="isolation_mesh_simulating:Willie_BP_C_0","before"
    local stopped_reason
    local boot=Core.new({now=function()return 0 end,link=function()return{connected=true}end,directory=function()return directory end,
        world=function()return{ready=true,arena=directory.arena,key=key}end,isolated=function()return false,reason end,
        travel=function()return true end,report=function(_,why)stopped_reason=why end,clear=function()end,close=function()end})
    boot:tick();key="after";boot:tick()
    check(boot.stopped and stopped_reason==reason,"post-travel refusal preserves the exact native reason")
end
do
    local Director=dofile("mods/HSMPMatch/Scripts/director.lua")
    local old_name,old_string=FName,FString
    FName=function(s)return s end;FString=function(s)return s end
    local accepted,console,options=false,0,nil
    local object={IsValid=function()return true end}
    local gs={IsValid=object.IsValid,OpenLevel=function(_,_,_,_,value)options=value;if not accepted then error("OpenLevel unavailable")end end}
    local ksl={IsValid=object.IsValid,ExecuteConsoleCommand=function()console=console+1 end}
    local env=Director.make_ue_env({WG={world=function()return object end,pc=function()return object end},
        UEHelpers={GetGameplayStatics=function()return gs end,GetKismetSystemLibrary=function()return ksl end},log=function()end,state_dir="unused"})
    check(not Director.native_client_travel(env,"Map_Arena_Yard")and console==0,"mandatory game-mode option cannot fall back to an optionless travel")
    check(env.open_level("Map_Arena_Yard")==true and console==1,"legacy empty-option console fallback remains supported")
    accepted=true
    check(Director.native_client_travel(env,"Map_Arena_Yard")==true and options=="game=/Script/Engine.GameModeBase"and console==1,"native OpenLevel receives the exact mandatory option")
    FName,FString=old_name,old_string
end
print(string.format("native_client: %d checks passed",n))
