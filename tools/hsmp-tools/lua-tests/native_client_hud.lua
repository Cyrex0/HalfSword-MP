-- Offline display/lifetime checks, not native pixel or Slate parity evidence.
local Hud=dofile("mods/HSMPMatch/Scripts/native_client_hud.lua")
local function check(ok,why)T.check(ok,why);assert(ok,why)end
local function scene(frame,age)
    local other={epoch=8,id=2,incarnation=4,kind=0,owner_peer=2,health={kind="f32",value=73.125},stamina={kind="f64",value=-0.0}}
    local own={epoch=8,id=1,incarnation=5,kind=0,owner_peer=1,health={kind="f64",value=1.0523740317784964},stamina={kind="f32",value=42.5}}
    return {state=2,fresh=true,gameplay_proof=true,generation="8:7:original",epoch=8,peer_id=1,frame_seq=frame or 9,
        received_age_ms=age or 20,entities={other,own}},own
end
local function fixture()
    local f={generation=1,time=0,available=true,calls={},objects={},stale=0,logs=0}
    local function object(path)
        local born=f.generation;local o={path=path,Font={Size=12},born=born}
        local function call(self,name,...)
            if born~=f.generation then f.stale=f.stale+1;error("old widget touched",0)end
            local row={name=name,object=self,args=table.pack(...)};f.calls[#f.calls+1]=row
            if f.hook then f.hook(row)end
        end
        for _,name in ipairs({"SetOwningPlayer","SetJustification","SetFont","SetColorAndOpacity","SetShadowOffset","SetShadowColorAndOpacity",
            "SetVisibility","SetAnchors","SetOffsets","SetAnchorsInViewport","AddToViewport","SetText","RemoveFromParent"})do
            o[name]=function(self,...)call(self,name,...)end
        end
        o.IsValid=function(self)call(self,"IsValid");return true end
        o.AddChildToCanvas=function(self,w)call(self,"AddChildToCanvas",w);return object("slot")end
        f.objects[#f.objects+1]=o;return o
    end
    f.WG={check=function()return f.available end,same=function(token)return f.available and token==f.generation end,
        token=function()return f.generation end,world=function()return{}end,pc=function()return{}end}
    f.hud=Hud.new({WG=f.WG,now=function()return f.time end,name=function(s)return s end,text=function(s)return s end,
        find=function(path)return{path=path,IsValid=function()return true end}end,construct=function(cls)return object(cls.path)end,
        log=function()f.logs=f.logs+1 end})
    function f:last(name)for i=#self.calls,1,-1 do if self.calls[i].name==name then return self.calls[i]end end end
    function f:count(name)local count=0;for _,row in ipairs(self.calls)do if row.name==name then count=count+1 end end;return count end
    return f
end
do
    local f=fixture();local s,own=scene()
    local function status(state,data,player,ready)return f.hud:status(state,data,player,ready,f.time)end
    check(f.hud:tick()==false and #f.objects==0,"waiting does not create or reveal stats")
    check(not status("mirror_ready",s,own,true)and not status("live",s,own,false),"model readiness or missing view readiness cannot reveal stats")
    check(status("live",s,own,true)and f.hud:tick()==true,"confirmed fresh LIVE with readiness creates passive HUD")
    local text=f:last("SetText").args[1]
    check(text:find("You  |  Health: 1.0523740317784964  |  Stamina: 42.5",1,true)==1,"own exact roundtrip numeric values are first")
    check(text:find("Player 2  |  Health: 73.125  |  Stamina: -0",1,true)~=nil,"other confirmed vitals and signed zero are retained")
    check(f:last("AddToViewport").args[1]==100 and f:last("SetVisibility").args[1]==3,"stats use lower viewport priority than loading and no hit testing")
    local calls=#f.calls;f.hud:tick();check(#f.calls==calls,"unchanged text and visibility cause no repeated widget writes")
    s.entities[2].health.value=999;s.entities[1].stamina.value=100;f.hud:tick()
    check(f:last("SetText").args[1]==text,"rendering retains only copied confirmed values, not mutable scene references")
    check(s.received_age_ms==20 and s.frame_seq==9 and own.health.value==999,"display never changes source timestamps or frame identity")
    f.hud:status("wait_scene",s,own,false);check(f.hud:tick()==false and f:last("SetVisibility").args[1]==1,"waiting immediately hides previously ready stats")
    s,own=scene(10);check(status("live",s,own,true)and f.hud:tick(),"new confirmed result restores existing HUD")
    check(f:count("AddToViewport")==1,"recovery does not create duplicate HUD hosts")
    f.hud:status("error",nil,nil,false);f.hud:tick();check(f:last("SetVisibility").args[1]==1,"fatal/loading state hides stats without copying unknown values")
end
do
    local f=fixture();local s,own=scene(9,100)
    f.hud:status("live",s,own,true,0);f.hud:tick();f.time=.1
    check(f.hud:status("live",s,own,true,.1)and f.hud.receipt.expires==.15,"repeated same frame cannot renew its original remaining receipt")
    f.time=.15;check(f.hud:tick()==false and f:last("SetVisibility").args[1]==1,"stats hide at the original250ms deadline")
    f.hud:status("wait_scene",s,own,false);f.time=.2;f.hud:status("live",s,own,true,.2)
    check(f.hud:tick()==false,"waiting and refeeding the same frame cannot manufacture freshness")
    local old,old_own=scene(8,0);check(not f.hud:status("live",old,old_own,true,.2),"older same-generation result does not replace last confirmed identity")
    s,own=scene(10,0);f.hud:status("live",s,own,true,.2);f.time=.199
    check(f.hud:tick()==false,"backward display clock cannot reveal confirmed stats")
end
do
    local f=fixture();local s,own=scene()
    for _,change in ipairs({function()s.gameplay_proof=false end,function()s.fresh=false end,function()s.received_age_ms=251 end,
        function()s.entities[1].health=nil end,function()s.entities[1].stamina.value=0/0 end,function()s.entities[1].health.kind="unknown"end,
        function()s.entities[3]=s.entities[1]end,function()own.owner_peer=2 end})do
        s,own=scene();change();check(not f.hud:status("live",s,own,true,0),"unconfirmed/stale/incomplete/ambiguous vitals do not invent a value")
    end
    check(#f.objects==0,"invalid copied scenes do not cause widget construction")
end
do
    local f=fixture();local s,own=scene();f.hud:status("live",s,own,true,0);f.hud:tick()
    f.generation=2;f.hud:drop();check(f.stale==0 and f.hud.host==nil and f.hud.snapshot==nil,"world drop forgets all original widgets and readiness without old access")
    f.hud:tick();check(f.stale==0 and f:count("AddToViewport")==1,"new world cannot expose old confirmed stats")
    s,own=scene(11);s.generation="next-world";f.hud:status("live",s,own,true,0);f.hud:tick()
    check(f.hud.host.token==2 and f.stale==0,"next-world HUD requires its own confirmed scene and widgets")
    check(f.hud:clear()and f:count("RemoveFromParent")==1 and f.hud.host==nil,"same-world cleanup removes only the HUD's own host")
end
do
    local f=fixture();local s,own=scene();f.hud:status("live",s,own,true,0)
    f.hook=function(row)if row.name=="SetText"then f.time=.3 end end
    check(f.hud:tick()==false and f:last("SetVisibility").args[1]==1,"slow construction/text update cannot reveal a receipt that expired during widget work")
    f=fixture();s,own=scene();f.hud:status("live",s,own,true,0)
    f.hook=function(row)if row.name=="AddChildToCanvas"then f.generation=2 end end
    check(f.hud:tick()==false and f.stale==0 and f.hud.host==nil,"world change inside a widget callback refuses before any old-widget continuation")
    f=fixture();s,own=scene();f.hud:status("live",s,own,true,0)
    f.hook=function(row)if row.name=="SetText"then error("stats write failed",0)end end
    local ok,why=f.hud:tick();check(ok==nil and why:find("stats view unavailable",1,true)and f.logs==1,"same-world UI failure is explicit and hides labels")
    local calls=#f.calls;for _=1,100 do f.hud:tick()end
    check(#f.calls==calls and f.logs==1,"fatal widget failure cannot rebuild or flood logs over repeated ticks")
end
do
    local f=fixture();local s,own=scene(9,100)
    check(not f.hud:status("live",s,own,true)and not f.hud:status("live",s,own,true,math.huge)
        and not f.hud:status("live",s,own,true,.001),"missing/nonfinite/future confirm clock cannot anchor receipt freshness")
    f.time=.16
    check(f.hud:status("live",s,own,true,0)and f.hud.receipt.expires==.15 and f.hud:tick()==false and #f.objects==0,
        "delayed first status delivery includes controller callback time and cannot extend original receipt")
    s,own=scene(10,20);f.time=.2;f.hud:status("live",s,own,true,.1)
    check(f.hud:tick()==true and f.hud.receipt.expires==.33,"confirmed sampling anchor preserves only original remaining duration")
end
