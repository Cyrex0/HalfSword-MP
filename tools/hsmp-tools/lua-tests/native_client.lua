-- Lifecycle/input regressions. Synthetic source frames are not native display evidence.
local Core=dofile("mods/HSMPMatch/Scripts/native_client_core.lua")
local Input=dofile("mods/HSMPMatch/Scripts/native_client_input.lua")
local Arrays=dofile("mods/HSMPMatch/Scripts/native_client_array.lua")
local n=0
local function check(ok,why)n=n+1;T.check(ok,why);assert(ok,why)end
local time,state,closed,cleared,applied,sent=0,"",0,0,0,{}
local directory={epoch=77,seq=4,state=1,arena="Map_Arena_Yard"}
local scene={epoch=77,dir_seq=4,frame_seq=10,state=1,peer_id=9001,generation="generation4",fresh=true,entities={{epoch=77,id=1,incarnation=3,owner_peer=9001,kind=0,position={0,0,0}},{epoch=77,id=2,incarnation=2,owner_peer=9002,kind=0,position={100,0,0}}}}
local connected,available,isolated,render_ok=true,true,true,true
local core=Core.new({now=function()return time end,link=function()return{connected=connected,error=""}end,directory=function()return directory end,
    world=function()return{ready=true,arena="Map_Arena_Yard",key="world1"}end,isolated=function()return isolated end,scene=function()return available and scene or nil end,
    present=function()applied=applied+1;return render_ok,render_ok and scene or"provider refused"end,
    clear=function()cleared=cleared+1 end,close=function()closed=closed+1 end,travel=function()error("unexpected travel")end,
    report=function(s)state=s end,input=function()return{1,0,0,0,0,0,0,0},0 end,send=function(f)sent[#sent+1]=f;return true end})
check(core:tick()and state=="mirror_ready","source scene must apply before readiness")
check(#sent==0,"READY cannot send human inputs")
scene.state=2;directory.state=2;check(core:tick()and state=="live"and#sent==1,"LIVE sends ordinary raw input")
check(sent[1].epoch==77 and sent[1].id==1 and sent[1].incarnation==3,"original exact owned reference")
scene.dir_seq=3;check(not core:tick()and#sent==1,"old directory frame cannot drive input")
scene.dir_seq=4;time=1.2;core:tick();check(sent[2].seq==2,"input sequence advances")
connected=false;check(not core:tick()and cleared>=2,"disconnect drops visual scope")
connected=true;scene.entities[1].incarnation=4;directory.seq=5;scene.dir_seq=5;scene.generation="generation5";core:tick();check(sent[3].incarnation==4 and sent[3].seq==3,"reconnect uses newref without replaying sequence")
available=false;check(not core:tick()and state=="wait_scene","missing complete scene cannot remain live")
available=true;render_ok=false;check(not core:tick()and closed==1 and core.stopped,"provider failure closes and never fabricates readiness")
local bad=Core.new({now=function()return 0 end,link=function()return{connected=true}end,directory=function()return directory end,world=function()return{ready=true,arena=directory.arena,key="world"}end,
    isolated=function()return false end,report=function()end,clear=function()end,close=function()closed=closed+1 end,scene=function()error("must refuse before scene")end})
check(not bad:tick()and bad.stopped,"local combat world refuses before mirror creation")
local map={keys={"W","MouseX","LeftShift"},axes={{{key=1,scale=1}},{},{{key=2,scale=-1}},{},{},{},{},{}},actions={{{key=3,mods={}}},{},{},{},{},{},{}}}
local a,b=Input.frame(map,{{1,1},{4,0},{1,1}});check(a[1]==1 and a[3]==-4 and b==1,"engine mapping scales and held actions")
a,b=Input.frame(map,{{0,0},{0,0},{0,0}});check(a[1]==0 and b==0,"actual release clears held values")
check(Input.frame(map,{})==nil,"incomplete key state refuses")
do
    local function parameter(kind,value)return{type=function()return kind end,get=function()return value end}end
    local object={type=function()return"UObject"end,get=function()error("a direct object must not be treated as a parameter")end}
    local out={};Arrays.each({parameter("RemoteUnrealParam",object),parameter("LocalUnrealParam",false)},function(v)out[#out+1]=v end)
    check(out[1]==object and out[2]==false,"function-return plain arrays unwrap typed hard parameters and preserve false")
    out={};Arrays.each({ForEach=function(_,fn)fn(1,parameter("LocalUnrealParam",7));fn(2,object)end},function(v)out[#out+1]=v end)
    check(out[1]==7 and out[2]==object,"property TArray ForEach remains supported without dereferencing direct objects")
    check(not pcall(Arrays.each,{[2]=object},function()end),"sparse return arrays refuse")
    check(not pcall(Arrays.each,{field=object},function()end),"nonnumeric return arrays refuse")
    check(not pcall(Arrays.each,setmetatable({object},{}),function()end),"return-array metamethods refuse")
    check(not pcall(Arrays.each,{object,object},function()end,1),"return-array bounds refuse without truncated success")
    check(not pcall(Arrays.each,nil,function()end),"missing array is not guessed empty")
    local names={"Move Forward / Backward","Move Right / Left","Turn Right / Left Mouse","Look Up / Down Mouse","Right Guard Axis","Left Guard Axis","Right Arm Axis","Left Arm Axis"}
    local actions={"Run","Crouch Hold","Thrust","Jump","Grab Right","Grab Left","Talk"}
    local axis_rows,action_rows={},{}
    for i,name in ipairs(names)do axis_rows[i]=parameter("LocalUnrealParam",{AxisName=name,Scale=1,Key={KeyName="Axis"..i}})end
    for i,name in ipairs(actions)do action_rows[i]=parameter("LocalUnrealParam",{ActionName=name,Key={KeyName="Button"..i},bShift=false,bCtrl=false,bAlt=false,bCmd=false})end
    local settings={AxisMappings=axis_rows,ActionMappings=action_rows}
    local copied=Input.mapping(settings,Arrays.each)
    check(copied and#copied.keys==15 and#copied.actions[1][1].mods==0,"input mapping supports copied function-return arrays with exact false modifiers")
    settings.AxisMappings={ForEach=function(_,fn)for i,v in ipairs(axis_rows)do fn(i,v)end end}
    settings.ActionMappings={ForEach=function(_,fn)for i,v in ipairs(action_rows)do fn(i,v)end end}
    local property=Input.mapping(settings,Arrays.each)
    check(property and table.concat(property.keys,",")==table.concat(copied.keys,","),"actual property mapping arrays preserve the same controls")
end
local observed,reported,frame
local applied_scene={epoch=77,dir_seq=5,frame_seq=12,state=2,peer_id=9001,generation="generation5",fresh=true,entities=scene.entities}
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
    isolated=function()return clean end,scene=function()return scene end,present=function()return true,scene end,
    travel=function()travels=travels+1;return true end,report=function()end,clear=function()end,close=function()error("isolated travel should converge")end,
    input=function()return{0,0,0,0,0,0,0,0},0 end,send=function()return true end})
check(not travel:tick()and travels==1,"same arena still travels to the isolated game mode")
check(not travel:tick()and travels==1,"pending travel cannot repeat against the old world")
key="new";clean=true;check(travel:tick()and travels==1,"new isolated world can create mirrors")
do
    local function fixture()
        local v={time=0,world="world1",epoch=77,dir_seq=4,dir_state=1,clears=0,closes=0,presents=0,inputs=0,sends=0,reports={}}
        v.scene={epoch=77,dir_seq=4,frame_seq=10,state=1,peer_id=9001,generation="recipe4",fresh=true,
            entities={{epoch=77,id=1,incarnation=3,owner_peer=9001,kind=0},{epoch=77,id=2,incarnation=2,owner_peer=9002,kind=0}}}
        v.applied=v.scene
        v.core=Core.new({now=function()return v.time end,link=function()return{connected=true}end,
            directory=function()return{epoch=v.epoch,seq=v.dir_seq,state=v.dir_state,arena="Map_Arena_Yard"}end,
            world=function()return{ready=true,arena="Map_Arena_Yard",key=v.world}end,isolated=function()return true end,
            scene=function()return v.scene end,present=function()
                v.presents=v.presents+1;if v.on_present then return v.on_present()end;return true,v.applied
            end,clear=function()v.clears=v.clears+1 end,close=function()v.closes=v.closes+1 end,
            report=function(s,why,current,own)v.reports[#v.reports+1]={state=s,reason=why,scene=current,own=own}end,
            input=function(current)v.inputs=v.inputs+1;v.input_scene=current;if v.input_refusal then return nil,v.input_refusal end;return{1,0,0,0,0,0,0,0},0 end,
            send=function(f)v.sends=v.sends+1;v.frame=f;if v.send_refusal then return nil,v.send_refusal end;return true end})
        return v
    end
    local v=fixture();v.scene.fresh=false
    check(v.core:tick()and v.core.state=="mirror_ready"and v.presents==1 and v.inputs==0,"generation-valid stale READY scene may complete native readback without live input")
    local clear_count=v.clears;v.scene=nil
    check(not v.core:tick()and v.core.state=="wait_scene"and v.clears==clear_count and v.core.scope~=nil,"transient missing complete scene retains inert mirrors without claiming live")
    v.scene=v.applied;v.scene.state=2;v.dir_state=2
    check(not v.core:tick()and v.inputs==0 and v.sends==0 and v.closes==0 and v.clears==clear_count,"stale LIVE scene neither samples nor sends input and keeps current inert mirrors")
    check(v.presents==1,"stale LIVE peek withholds provider work while stale READY readback remains eligible")
    v.scene.fresh=true
    check(v.core:tick()and v.core.state=="live"and v.inputs==1 and v.sends==1 and v.clears==clear_count,"fresh same-generation recovery reuses native mirrors and sends ordinary controls")
    for _,change in ipairs({"world","epoch","directory"})do
        v=fixture();v.core:tick();clear_count=v.clears;v.scene=nil
        if change=="world"then v.world="world2"elseif change=="epoch"then v.epoch=78 else v.dir_seq=5 end
        check(not v.core:tick()and v.clears==clear_count+1 and v.core.scope==nil,"known "..change.." change drops old mirror generation even during a complete-scene gap")
        v.core:tick();check(v.clears==clear_count+1,"missing new generation does not repeatedly clear already-retired mirrors")
    end
    for _,change in ipairs({"recipe","reference"})do
        v=fixture();v.core:tick();clear_count=v.clears
        if change=="recipe"then v.scene.generation="recipe5"else v.scene.entities[1].incarnation=4 end
        check(v.core:tick()and v.clears==clear_count+1 and v.presents==2,"complete "..change.." generation replacement clears old mirrors before native reapplication")
    end
    v=fixture();v.scene=nil
    check(not v.core:tick()and v.presents==0 and v.core.state=="wait_scene"and v.inputs==0,"received partials or assembly ACK alone cannot produce native MirrorReady")
    v=fixture();v.on_present=function()return true end
    check(not v.core:tick()and v.core.stopped and v.inputs==0,"successful boolean without actual applied scene cannot fabricate readiness")
    for _,field in ipairs({"generation","fresh"})do
        v=fixture();v.scene[field]=nil
        check(not v.core:tick()and v.core.stopped and v.presents==0,"missing complete native "..field.." provenance is refused explicitly")
    end
    v=fixture();v.scene.state=2;v.dir_state=2;local original=v.scene
    v.on_present=function()
        v.time=.300
        v.scene={epoch=77,dir_seq=4,frame_seq=11,state=2,peer_id=9001,generation="recipe4",fresh=true,entities=original.entities}
        original.fresh=false;return true,original
    end
    check(not v.core:tick()and not v.core.stopped and v.inputs==0 and v.sends==0,"newer complete receipt never refreshes age of the older native-applied frame")
    check(v.reports[#v.reports].scene==original and v.reports[#v.reports].reason=="native applied scene is stale","stale metrics identify actual applied frame rather than newly received frame")
    v=fixture();v.scene.state=2;v.dir_state=2;original=v.scene
    v.on_present=function()
        v.scene={epoch=77,dir_seq=4,frame_seq=11,state=2,peer_id=9001,generation="recipe4",fresh=true,entities=original.entities}
        return true,original
    end
    check(v.core:tick()and v.sends==1 and v.input_scene==original,"still-fresh applied frame remains usable when newer same-generation frame arrived")
    v=fixture();v.scene.state=2;v.dir_state=2;v.send_refusal="native applied scene is stale"
    check(not v.core:tick()and not v.core.stopped and v.core.state=="wait_scene"and v.sends==1 and v.closes==0,"age expiry during legal input sampling is a freshness wait rather than fatal exit")
    clear_count=v.clears;v.send_refusal=nil
    check(v.core:tick()and v.frame.seq==2 and v.clears==clear_count,"fresh recovery never resends the expired input sequence or recreates current mirrors")
    v=fixture();v.scene.state=2;v.dir_state=2;v.input_refusal="native applied scene is stale"
    check(not v.core:tick()and not v.core.stopped and v.core.state=="wait_scene"and v.inputs==1 and v.sends==0 and v.closes==0,"freshness expiring at the input boundary keeps inert mirrors and withholds controls without fatal exit")
    v=fixture();v.core:tick();clear_count=v.clears;v.scene.state=2;v.dir_state=2
    v.on_present=function()return nil,"native applied scene is stale"end
    check(not v.core:tick()and not v.core.stopped and v.clears==clear_count and v.inputs==0 and v.sends==0 and v.closes==0,"production present freshness refusal after preflight retains inert mirrors and withholds live input")
    v=fixture();v.on_present=function()v.world="world2";return true,v.applied end
    check(not v.core:tick()and not v.core.stopped and v.inputs==0 and v.core.scope==nil,"world replacement during apply cannot drive input through the old mirror generation")
    v=fixture();v.on_present=function()v.dir_seq=5;return true,v.applied end
    check(not v.core:tick()and not v.core.stopped and v.inputs==0 and v.core.scope==nil,"directory replacement during native callbacks drops old mirrors before any input")
    v=fixture();v.scene.state=2;v.dir_state=2
    v.on_present=function()v.dir_state=1;return true,v.applied end
    check(not v.core:tick()and v.inputs==0 and not v.core.stopped,"source no longer LIVE after application cannot receive old live controls")
    v=fixture();v.core:tick();clear_count=v.clears;v.on_present=function()return nil,"no coherent native scene"end
    check(not v.core:tick()and v.clears==clear_count and not v.core.stopped,"transient incomplete scene during asset preflight retains existing inert mirrors")
    v=fixture();v.core:tick();local report_count=#v.reports;v.time=.5;v.core:tick()
    check(#v.reports==report_count,"unchanged actual native readiness metrics are bounded to one update per second")
    v.time=1.1;v.core:tick();check(#v.reports==report_count+1 and v.reports[#v.reports].scene==v.applied,"periodic metrics use actual applied source frame")
end
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
