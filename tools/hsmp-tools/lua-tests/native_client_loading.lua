-- Offline UMG/cursor contracts, not native on-screen or owned-camera evidence.
local Loading=dofile((...)or "mods/HSMPMatch/Scripts/native_client_loading.lua")
local Presentation=dofile("mods/HSMPAvatars/Scripts/native_presentation.lua")
local n=0
local function check(ok,why)n=n+1;T.check(ok,why);assert(ok,why)end
local function fixture(view_ready)
    local f={generation=1,available=true,objects={},calls={},stale=0,time=0,logs=0}
    local function object(kind)
        local born=f.generation
        local o={kind=kind,Font={Size=12},born=born}
        local function call(self,name,...)
            if born~=f.generation then f.stale=f.stale+1;error("old widget touched")end
            local args=table.pack(...);f.calls[#f.calls+1]={object=self,name=name,args=args}
            if f.hook then f.hook(name,self,args)end
        end
        for _,name in ipairs({"SetOwningPlayer","SetAnchors","SetOffsets","SetAlignment","SetPosition","SetSize","SetVisibility",
            "SetBrushColor","SetText","SetJustification","SetFont","SetColorAndOpacity","SetFillColorAndOpacity",
            "SetAnchorsInViewport","AddToViewport","SetIsMarquee","SetPercent","RemoveFromParent"})do
            o[name]=function(self,...)call(self,name,...)end
        end
        o.IsValid=function(self)call(self,"IsValid");return true end
        o.AddChildToCanvas=function(self,w)call(self,"AddChildToCanvas",w);return object("CanvasPanelSlot")end
        f.objects[#f.objects+1]=o;return o
    end
    f.WG={same=function(token)return f.available and token==f.generation end,token=function()return f.generation end,
        check=function()return f.available end,world=function()return{}end,pc=function()return{}end}
    f.ui=Loading.new({WG=f.WG,find=function(path)return{IsValid=function()return true end,path=path}end,
        construct=function(cls)return object(cls.path)end,name=function(value)return value end,text=function(value)return value end,
        view_ready=view_ready,now=function()return f.time end,log=function()f.logs=f.logs+1 end})
    function f:count(name)local total=0;for _,row in ipairs(self.calls)do if row.name==name then total=total+1 end end;return total end
    function f:last(name)for i=#self.calls,1,-1 do if self.calls[i].name==name then return self.calls[i]end end end
    return f
end
do
    local f=fixture(function()return true end);f.ui:tick()
    for _,stage in ipairs({"present","character","native_setup","equipment","controls","check_equipment"})do
        f.ui:set(stage);f.ui:status("wait_scene",nil,nil,"native gameplay pawn preparing");f.ui:tick()
        check(f.ui.stage==stage and f:last("SetIsMarquee").args[1]==true and f:count("SetPercent")==0,
            "exact gameplay preparation pending preserves its honest indeterminate stage: "..stage)
    end
    local original={fresh=true,receipt=123,frame_seq=40}
    f.ui:set("sync");f.ui:tick()
    check(f.ui.stage=="sync"and not f.ui.ready and f:count("RemoveFromParent")==0,
        "preparation completion alone cannot remove the synchronization overlay")
    f.ui:status("live",original,{});f.ui:tick();local mounts=f:count("AddToViewport")
    f.ui:set("sync");f.ui:tick()
    check(f.ui.ready and f.ui.host==nil and f:count("AddToViewport")==mounts,
        "ordinary warm synchronization progress cannot re-cover a proven live view")
    f.ui:status("wait_scene",original,{},"native gameplay result is stale");f.ui:tick()
    check(not f.ui.ready and f.ui.host and f.ui.stage=="sync"and f.ui.host.signature:find("Synchronizing the latest match state",1,true)
        and f:last("SetIsMarquee").args[1]==true and f:count("SetPercent")==0,
        "an exact original receipt expiry restores a precise sync wait without a completed match bar")
    local texts=f:count("SetText");f.time=.5;f.ui:tick()
    check(f:count("SetText")>texts and original.receipt==123 and original.frame_seq==40,
        "sync waiting remains animated on the real clock without changing original state provenance")
    f.ui:status("wait_scene",original,{},"unknown native refusal");f.ui:tick()
    check(f.ui.stage=="waiting","unknown pending causes do not inherit a precise gameplay synchronization claim")
end
do
    local f=fixture();f.ui:tick();local text_calls=f:count("SetText")
    f.time=.1;f.ui:tick();f.time=.4;f.ui:tick()
    check(f:count("SetText")==text_calls,"cosmetic waiting updates stay below2Hz between genuine stage changes")
    f.time=.5;f.ui:tick()
    check(f.ui.host.signature:find("Connecting to your match.",1,true)and f:last("SetText").args[1]=="Elapsed: 0s",
        "waiting dots advance on the real half-second clock while elapsed time stays truthful")
    f.time=1;f.ui:tick();text_calls=f:count("SetText")
    check(f:last("SetText").args[1]=="Elapsed: 1s"and f:count("SetPercent")==0,"elapsed seconds never become unknown server progress")
    for _=1,100 do f.time=f.time+.002;f.ui:tick()end
    check(f:count("SetText")==text_calls,"one hundred high-frequency ticks cannot flood the cosmetic widget updates")
    f.ui:set("waiting");f.ui:tick()
    check(f.ui.host.signature:find("Waiting for the match",1,true)and f:last("SetText").args[1]:find("Progress is not available",1,true),
        "actual stage transition bypasses cosmetic throttling immediately")
    f.ui:set("assets",0,0);f.ui:tick();f.ui:set("assets",1,4);f.ui:tick()
    check(f:last("SetPercent").args[1]==.25 and f:last("SetText").args[1]:find("Player assets: 1 / 4",1,true),
        "the first real asset count appears immediately even within the same half-second")
    f.ui:set("assets",2,4);f.ui:tick()
    check(f:last("SetPercent").args[1]==.5 and f:last("SetText").args[1]:find("Player assets: 2 / 4",1,true),
        "every changed actual asset count bypasses the cosmetic throttle immediately")
    text_calls=f:count("SetText");f.ui:set("assets",2,4);f.ui:tick()
    check(f:count("SetText")==text_calls,"unchanged actual counts retain the2Hz cosmetic throttle")
    local percent_calls=f:count("SetPercent");f.ui:set("assets",4,4);f.ui:tick()
    check(f:last("SetIsMarquee").args[1]==true and f:count("SetPercent")==percent_calls
        and f.ui.host.signature:find("Player assets loaded",1,true)
        and f:last("SetText").args[1]:find("Player models and your view are still loading",1,true),
        "completed assets keep their exact count but cannot show a completed match bar")
    check(not f.ui.ready and f:count("RemoveFromParent")==0,"asset completion never supplies model or owned-view readiness")
    f.ui:set("present");f.ui:tick()
    check(f.ui.host.signature:find("Creating player models",1,true)and f:last("SetIsMarquee").args[1]==true,
        "completed asset preparation switches immediately to creating models without claiming match completion")
    f.ui:status("wait_scene",nil,nil,"native scene assets loading");f.ui:tick()
    check(f.ui.stage=="present","only exact known preparation pending preserves the real model-creation stage")
    f.ui:status("wait_scene",nil,nil,"native applied scene is stale");f.ui:tick()
    check(f.ui.stage=="waiting","stale or missing source data cannot retain a misleading model-creation stage")
    f.ui:status("error",nil,nil,"Player models could not be created");f.ui:tick()
    check(f.ui.host.signature:find("Unable to load the match",1,true)and f:last("SetText").args[1]:find("Player models could not be created",1,true)
        and f:last("SetText").args[1]:find("Close and join again",1,true),"fatal cause bypasses cosmetic throttling and remains visible")
    f.ui:status("stopped",nil,nil,"stop requested");f.ui:set("present");f.ui:tick()
    check(f:last("SetText").args[1]:find("Player models could not be created",1,true)and not f:last("SetText").args[1]:find("stop requested",1,true),
        "later stop and preparation reports cannot replace the first fatal cause")
    f.generation=2;f.ui:drop();f.ui:tick()
    check(f:last("SetText").args[1]:find("Player models could not be created",1,true)and f.stale==0,
        "fatal cause survives safe world drop without touching previous widgets")
    f=fixture();f.ui:tick();f.ui:status("mirror_ready",{fresh=true},{});f.ui:tick();f.ui:set("present");f.ui:tick()
    check(f.ui.stage=="view"and not f.ui.ready and f:count("RemoveFromParent")==0,
        "repeated native presentation cannot replace the precise unverified-view waiting stage")
end
do
    local f=fixture();f.ui:tick()
    f.ui:status("error",nil,nil,string.rep("x",191).."é"..string.rep("y",400).."\ncontrol")
    f.ui:tick()
    check(#f.ui.failure_reason<=195 and utf8.len(f.ui.failure_reason)~=nil and not f.ui.failure_reason:find("\n",1,true),
        "visible copied error is bounded and cannot split a UTF-8 character or add control lines")
end
do
    local f=fixture();local attempts=0
    f.hook=function(name)if name=="IsValid"then attempts=attempts+1;error("widget construction refused",0)end end
    local ok,why=f.ui:tick()
    check(ok==nil and why:find("loading view unavailable",1,true)and f.ui.failed,"same-world widget failure is an explicit fatal loading result rather than a normal world wait")
    for _=1,100 do f.time=f.time+.008;f.ui:tick()end
    check(attempts==1 and f.logs==1,"one hundred failure-loop ticks cannot reconstruct or log before the bounded retry deadline")
    f.time=1.1;f.ui:tick();f.time=2.2;f.ui:tick();f.time=10;f.ui:tick()
    check(attempts==3 and f.logs==1,"persistent failure has a per-world attempt ceiling and logs only a changed error")
    f.generation=2;f.ui:drop();f.hook=nil
    check(f.ui:tick()and f.ui.host.token==2 and f.ui.stage=="error","new world can rebuild the retained error overlay with reset retry state")
    f.available=false;ok,why=f.ui:tick()
    check(ok==false and why==nil,"ordinary world-not-ready remains a transient wait without a fatal widget reason")
end
do
    local f=fixture()
    check(f.ui:tick()and f.ui.host~=nil,"loading view mounts as soon as a guarded world and PC exist")
    local brush=f:last("SetBrushColor").args[1]
    check(brush.A==1 and f:last("AddToViewport").args[1]==10000,"opaque backing precedes a high-priority viewport overlay")
    local fill
    for _,row in ipairs(f.calls)do if row.name=="SetAnchors"and row.args[1].Maximum.X==1 then fill=row end end
    check(fill and fill.args[1].Minimum.X==0 and fill.args[1].Minimum.Y==0 and fill.args[1].Maximum.Y==1
        and f:last("SetOffsets").args[1].Right==0,"backing stretches over the complete viewport rather than the center panel")
    f.ui:set("waiting");f.ui:tick()
    check(f:last("SetIsMarquee").args[1]==true and f:count("SetPercent")==0,"unknown server transfer is indeterminate and never gets a fabricated percentage")
    check(f:last("SetText").args[1]:find("Progress is not available",1,true)~=nil,"unknown transfer explains why there is no percentage")
    f.ui:set("assets",1,4);f.ui:tick()
    check(f:last("SetIsMarquee").args[1]==false and f:last("SetPercent").args[1]==.25,"asset percentage uses only completed over total exact assets")
    local calls=#f.calls;f.ui:tick()
    check(#f.calls==calls,"unchanged loading state does not spam widget writes")
    f.ui:status("mirror_ready",{fresh=true},{});f.ui:tick()
    check(f.ui.stage=="view"and f:count("RemoveFromParent")==0,"native mirror readiness alone cannot expose the unverified view")
    f.ui:status("live",{fresh=true},{});f.ui:tick()
    check(f:count("RemoveFromParent")==0 and f:last("SetIsMarquee").args[1]==true,"LIVE without a positive owned-view proof stays opaque and indeterminate")
    f.ui:fail();f.ui:set("assets",4,4);f.ui:status("live",{fresh=true},{});f.ui:tick()
    check(f.ui.stage=="error"and f.ui.failed and f:count("RemoveFromParent")==0,"fatal error remains visible and cannot be overwritten by progress or readiness")
    f.generation=2;f.ui:drop();check(f.stale==0 and f.ui.host==nil,"world drop forgets old widgets without touching them")
    check(f.ui:tick()and f.ui.host.token==2 and f.ui.stage=="error","error overlay rebuilds safely in the new native world")
end
do
    local proven=false;local f=fixture(function(scene,own)return proven and scene.owner==own end)
    f.ui:tick();local own={};local scene={fresh=false,owner=own}
    proven=true;f.ui:status("live",scene,own);f.ui:tick()
    check(f:count("RemoveFromParent")==0,"view proof cannot hide a stale source scene")
    scene.fresh=true;f.ui:status("live",scene,own);f.ui:tick()
    check(f:count("RemoveFromParent")==1 and f.ui.host==nil,"only fresh LIVE plus positive independent view proof removes the current overlay")
    f.generation=2;f.ui:drop();f.ui:tick()
    check(f.ui.host and not f.ui.ready and f.stale==0,"world replacement invalidates positive view readiness and restores the opaque overlay")
end
do
    local f=fixture();local triggered=false
    f.hook=function(name)if name=="SetText"and not triggered then triggered=true;f.generation=2;f.ui:drop()end end
    check(not f.ui:tick()and f.stale==0 and f.ui.host==nil,"world change in widget construction stops before any later old-widget operation")
    f.hook=nil;check(f.ui:tick()and f.ui.host.token==2,"next tick constructs only current-world widgets after callback travel")
    f.ui:set("assets",2,3);f.hook=function(name)if name=="SetText"then f.generation=3;f.ui:drop()end end
    check(not f.ui:tick()and f.stale==0,"world change during a label update stops before detail/bar access")
end
do
    local f=fixture(function()return true end);f.ui:tick();local original=f.ui.host
    f.ui:status("live",{fresh=true},{})
    f.hook=function(name)if name=="RemoveFromParent"then error("remove refused",0)end end
    local ok,why=f.ui:tick()
    check(ok==nil and why:find("remove refused",1,true)and f.ui.host==original and f.ui.failed and not f.ui.ready,
        "same-world removal failure keeps the original host and returns the same fatal loading contract")
    f.ui:tick();check(f:count("RemoveFromParent")==1 and f.logs==1,"removal failure cannot disappear into positive readiness or repeat during retry backoff")
    f.hook=nil;f.time=1.1;f.ui:tick()
    check(f.ui.host==original and f.ui.stage=="error"and not f.ui.ready,"a retained loading host shows the error instead of resuming after removal refusal")
end
do
    local Core=dofile("mods/HSMPMatch/Scripts/native_client_core.lua")
    local f=fixture(function()return true end)
    local source={epoch=77,dir_seq=4,frame_seq=10,state=2,peer_id=9,generation="live4",fresh=true,receipt=123,
        entities={{epoch=77,id=1,incarnation=3,owner_peer=9,kind=0}}}
    local directory={epoch=77,seq=4,state=2,arena="Map_Arena_Yard"}
    local world={GetAddress=function()return 1234 end}
    local p=Presentation.new({world=function()return world,f.generation end,same=f.WG.same,
        progress=function(stage,done,total)f.ui:set(stage,done,total)end,
        native={native_scene_assets=function()return{},source.generation end,native_present=function()return true,source end,native_clear_mirrors=function()end}})
    local core=Core.new({now=function()return 0 end,link=function()return{connected=true}end,directory=function()return directory end,
        world=function()return{ready=true,arena=directory.arena,key="world1"}end,isolated=function()return true end,
        scene=function()return source end,present=function()return p:apply()end,clear=function()p:clear()end,close=function()end,
        report=function(state,_,scene,own)f.ui:status(state,scene,own)end,input=function()return{0,0,0,0,0,0,0,0},0 end,send=function()return true end})
    f.ui:tick();check(core:tick()and core.state=="live","actual Core and Presentation can supply the separate positive view seam after native readiness")
    f.ui:tick();local mounts=f:count("AddToViewport")
    local valid=true;for _=1,21 do valid=core:tick()and valid;f.ui:tick()end
    check(valid,"same-generation native frames remain valid during positive view proof")
    check(f:count("AddToViewport")==mounts and f:count("RemoveFromParent")==1 and f.ui.ready,"ordinary repeated native apply cannot re-cover a positively verified view between periodic status reports")
    source.fresh=false;core:tick();f.ui:tick()
    check(f.ui.host and not f.ui.ready and f.ui.stage=="waiting","a stale native-applied scene immediately restores opacity without refreshing its receipt")
    check(source.receipt==123 and source.frame_seq==10 and source.fresh==false,"loading state cannot rewrite native scene age or frame identity")
end
-- The production preparation adapter yields actual asset work without keeping
-- UObject wrappers or changing the original native scene's freshness/receipt.
do
    local old_find,old_load=StaticFindObject,LoadAsset
    local f={token=1,generation="g1",paths={"/Game/A.A","/Game/B.B","/Game/C.C"},loads={},steps={},presents=0,clears=0}
    local source={frame_seq=9,fresh=false,receipt=123}
    StaticFindObject=function()return nil end
    LoadAsset=function(path)
        f.loads[#f.loads+1]=path
        local born=f.token
        return{IsValid=function()assert(born==f.token,"old asset touched");return true end,
            GetFullName=function()if f.on_name then f.on_name()end;return"StaticMesh "..path end}
    end
    local world={GetAddress=function()return 1234 end}
    local p=Presentation.new({world=function()return world,f.token end,same=function(token)return token==f.token end,
        progress=function(stage,done,total)f.steps[#f.steps+1]={stage=stage,done=done,total=total}end,
        native={native_scene_assets=function()return f.paths,f.generation end,native_clear_mirrors=function()f.clears=f.clears+1 end,
            native_present=function(address)assert(address==1234);f.presents=f.presents+1;return true,source end}})
    local ok,why=p:apply()
    check(ok==nil and why=="native scene assets loading"and#f.loads==1 and f.presents==0,"first tick completes one actual asset and returns the explicit nonfatal pending reason")
    check(f.steps[#f.steps].done==1 and f.steps[#f.steps].total==3,"progress reports completed work rather than a timer")
    p:apply();check(#f.loads==2 and f.presents==0,"second tick yields after the second exact asset")
    ok,why=p:apply();check(ok==nil and why=="native scene assets loading"and#f.loads==3 and f.presents==0 and f.steps[#f.steps].stage=="present",
        "last asset yields one complete tick for the model-creation label without entering native presentation")
    ok,why=p:apply();check(ok==true and why==source and#f.loads==3 and f.presents==1,"following tick rechecks the exact current generation before entering native presentation")
    check(source.frame_seq==9 and source.receipt==123 and source.fresh==false,"preparation does not renew receipt age, rewrite the frame, or invent freshness")
    p:apply();check(#f.loads==3 and f.presents==2,"same complete generation does not reload prepared assets")
    f.generation="g2";p:apply();check(#f.loads==4 and p.loading.done==1,"new recipe generation restarts preparation from its exact asset list")
    f.generation="g3";f.on_name=function()f.generation="g4"end
    ok,why=p:apply();check(ok==nil and why=="client mirror generation"and p.loading==nil and f.presents==2,"recipe change during asset metadata cannot publish a partly prepared replacement")
    f.on_name=nil;p:apply();f.token=2;p:drop();p:apply()
    check(p.loading.token==2 and p.loading.done==1,"world drop resets the cursor without reading a previous-world asset wrapper")
    f.generation="g5";f.on_name=function()f.token=3;p:drop()end
    ok,why=p:apply();check(ok==nil and why=="world changed during source asset load"and f.presents==2,"callback travel during name conversion refuses before any native present")
    f.on_name=nil;f.paths={"/Game/A.A"};f.generation="g6";p:drop();p:apply()
    f.generation="g7";ok,why=p:apply()
    check(ok==nil and why=="native scene assets loading"and p.key=="g7"and f.presents==2,
        "generation change between last-asset yield and native create prepares the latest generation instead of presenting the old one")
    f.token=4;p:drop();ok,why=p:apply()
    check(ok==nil and why=="native scene assets loading"and f.presents==2,
        "world drop during the model-stage yield restarts current-world preparation before any native create")
    ok,why=p:apply();check(ok==true and why==source and f.presents==3 and source.receipt==123,
        "only the following exact current-world/current-generation tick creates models without renewing the original receipt")
    StaticFindObject,LoadAsset=old_find,old_load
end
print(string.format("native_client_loading: %d checks passed",n))
