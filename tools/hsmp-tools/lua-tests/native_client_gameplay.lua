local Gameplay=dofile("mods/HSMPMatch/Scripts/native_client_gameplay.lua")
local Core=dofile("mods/HSMPMatch/Scripts/native_client_core.lua")
local n=0
local function check(value,label)n=n+1;T.check(value,label);assert(value,label)end
local function fixture()
    local f={world="w1",calls={},progress={},clears=0,forgot=0,equipment=0,armor=0,weapons=0,puppets=0,clock=0,diagnostics={}}
    local cls={GetAddress=function()return 100 end,GetFName=function()return 101 end}
    local function pawn(id)
        return{GetAddress=function()return id+1000 end,GetFName=function()return id end,GetClass=function()return cls end,HasAnyFlags=function()return false end}
    end
    f.pawns={pawn(1),pawn(2)}
    local function recipe(id)return{schema=1,actor_class="/Game/Character/Blueprints/Willie_BP.Willie_BP_C",team=id,
        passport={fixture_id=id},construction={},equipment={}}end
    f.scene={epoch=-7,dir_seq=8,generation="g8",peer_id=9,state=1,fresh=true,frame_seq=40,authority_tick=40,received_age_ms=4,
        entities={{epoch=-7,id=1,incarnation=2,revision=3,owner_peer=9,kind=0,recipe=recipe(1)},
            {epoch=-7,id=2,incarnation=4,revision=5,owner_peer=10,kind=0,recipe=recipe(2)}}}
    local N={}
    N.native_gameplay_begin=function(scene,world,pc,id)
        check(scene==f.scene and world==88 and pc==77,"begin uses exact current scene/world/controller")
        f.calls[#f.calls+1]="begin"..id;return id,f.pawns[id]
    end
    N.native_gameplay_current=function(handle,scalar)
        if f.expired then return nil,"original native weak expired"end
        if scalar==true then return true end
        return f.pawns[handle]
    end
    N.native_gameplay_construct=function(handle)f.calls[#f.calls+1]="construct"..handle;return true,f.pawns[handle]end
    N.native_gameplay_initialized=function(handle)
        f.calls[#f.calls+1]="initialized"..handle
        if f.setup_error then return nil,f.setup_error end
        if f.setup_hook then f.setup_hook(handle)end
        if f.setup_count~=nil then return f.setup_count end
        return 0
    end
    N.native_gameplay_finish=function(handle)
        f.calls[#f.calls+1]="possess"..handle
        if f.finish_result~=nil then return f.finish_result,f.finish_reason end
        return true
    end
    N.native_gameplay_puppet=function(scene)
        f.puppets=f.puppets+1;f.calls[#f.calls+1]="puppet";f.clock=f.clock+250
        if f.puppet_error then return nil,f.puppet_error end
        return true,{generation=scene.generation,gameplay_proof=true,fresh=scene.fresh,received_age_ms=6,authority_tick=scene.authority_tick,entities=scene.entities,puppet_frames=f.shown or 5}
    end
    N.native_gameplay_clear=function(forget)f.clears=f.clears+1;if forget then f.forgot=f.forgot+1 end;return true end
    f.native=N
    local Passport={before_finish=function(recipe,env)
        check(env.guard()==true and env.current()==f.pawns[recipe.passport.fixture_id],"passport writes only the fresh original deferred pawn")
        f.calls[#f.calls+1]="passport"..recipe.passport.fixture_id;return true
    end,restore_live_armor=function(recipe)
        f.armor=f.armor+1;f.calls[#f.calls+1]="armor"..recipe.passport.fixture_id
        if f.armor_error then return nil,f.armor_error end;return true
    end,restore_live_weapons=function(recipe)
        f.weapons=f.weapons+1;f.calls[#f.calls+1]="weapons"..recipe.passport.fixture_id
        if f.weapon_error then return nil,f.weapon_error end;return true
    end,verify_equipment=function(recipe,env)
        f.equipment=f.equipment+1;check(env.guard()==true,"gear check runs against the guarded original pawn")
        f.calls[#f.calls+1]="gear"..recipe.passport.fixture_id
        if f.gear_error then return nil,f.gear_error end;return true
    end}
    f.game=Gameplay.new({native=N,passport=Passport,same=function(token)return token==f.world end,
        now_us=function()return f.clock end,
        diagnostic=function(row)f.diagnostics[#f.diagnostics+1]=row end,
        world=function()return{GetAddress=function()return 88 end},f.world,{GetAddress=function()return 77 end}end,
        progress=function(stage)f.progress[#f.progress+1]=stage end,fname=function(name)return name end})
    function f:bootstrap()
        for _=1,16 do local ok,why=self.game:apply(self.scene);check(ok==nil and why==Gameplay.PENDING,"every bootstrap stage yields pending without readiness")end
        check(self.game:bootstrapped(),"sixteen yielded stages build both original pawns")
    end
    return f
end
do
    local f=fixture();f:bootstrap()
    check(table.concat(f.calls,",")=="begin1,passport1,construct1,initialized1,armor1,weapons1,initialized1,gear1,possess1,gear1,"..
        "begin2,passport2,construct2,initialized2,armor2,weapons2,initialized2,gear2,possess2,gear2",
        "bootstrap order: construction, settled setup, gear restoration, settled setup + gear check, possession, gear check")
    check(f.puppets==0 and not f.game:view_ready(f.scene),"bootstrap never shows a puppet or claims view readiness")
    check(table.concat(f.progress,","):find("present,character,present,native_setup,equipment,native_setup,controls,check_equipment",1,true)==1,
        "loading progress names each actual bootstrap stage")
    local ok,actual,sampled=f.game:apply(f.scene)
    check(ok==true and actual.gameplay_proof==true and f.puppets==1 and f.game:view_ready(actual),"after bootstrap one puppet call per frame establishes view readiness")
    check(type(sampled)=="number"and f.progress[#f.progress]=="sync","frame returns a display sample time and reports sync")
    for _=1,10 do f.game:apply(f.scene)end
    check(f.puppets==11 and f.equipment==4 and f.armor==2 and f.weapons==2,"frames repeat neither gear restoration nor gear checks")
    local frames=0;for _,row in ipairs(f.diagnostics)do if row.kind=="frame"then frames=frames+1 end end
    check(frames==8 and f.diagnostics[1].us==250,"frame timing diagnostics are bounded to the first eight frames")
    f.scene.fresh=false;ok=f.game:apply(f.scene)
    check(ok==true and f.puppets==12,"a stale authority frame still plays back the buffered pose")
end
do
    local f=fixture()
    for _=1,3 do f.game:apply(f.scene)end
    f.setup_count=2
    for _=1,3 do f.game:apply(f.scene);f.clock=f.clock+1000000 end
    check(f.game.rows[1].stage=="native_setup"and f.armor==0,"pending native setup waits while under the settle bound")
    f.clock=f.clock+1000000;f.game:apply(f.scene)
    check(f.game.rows[1].stage=="equipment","pending native latent work past the settle bound advances the stage")
    f.setup_count=0;f.game:apply(f.scene)
    check(f.armor==1 and f.weapons==1 and f.game.rows[1].stage=="post_setup","gear restoration runs once after setup")
end
do
    local f=fixture();for _=1,6 do f.game:apply(f.scene)end
    check(f.game.rows[1].stage=="possess","settled post setup checks gear then reaches possession")
    f.finish_result=false;f.finish_reason="native gameplay initialization remains pending"
    local ok,why=f.game:apply(f.scene)
    check(ok==nil and why==Gameplay.PENDING and f.game.rows[1].stage=="post_setup"and f.armor==1,"typed finish pending returns to post setup without restoring gear")
    f.finish_result,f.finish_reason=false,"other native finish refusal";f.game:apply(f.scene)
    ok,why=f.game:apply(f.scene)
    check(ok==nil and why=="other native finish refusal"and not f.game.ready,"any other finish refusal is fatal")
end
do
    local f=fixture();f.gear_error="native live weapon passport differs";f:bootstrap()
    check(#f.game.gear_warnings==4 and f.game.gear_warnings[1].reason==f.gear_error and f.diagnostics[1].kind=="gear",
        "gear mismatch on a display-only puppet is reported, never fatal")
    check(f.game:apply(f.scene)==true,"reported gear mismatch still reaches the puppet frame path")
end
do
    local f=fixture();f:bootstrap();f.game:apply(f.scene)
    f.puppet_error="native gameplay puppet lost: puppet mesh lost"
    local ok,why=f.game:apply(f.scene)
    check(ok==nil and why==Gameplay.PENDING and f.clears==1 and not f.game:bootstrapped()and not f.game.ready,"a lost puppet clears the originals for a rebuild")
    check(f.diagnostics[#f.diagnostics].kind=="rebuild"and f.game.rebuilds==1,"rebuilds are counted and reported")
    for _=1,2 do f.puppet_error=nil;f:bootstrap();f.puppet_error="native gameplay puppet lost: pawn";f.game:apply(f.scene)end
    f.puppet_error=nil;f:bootstrap();f.puppet_error="native gameplay puppet lost: pawn"
    ok,why=f.game:apply(f.scene)
    check(ok==nil and why=="native gameplay puppet lost: pawn"and f.game.rebuilds==4,"repeated puppet loss past the rebuild bound is fatal")
end
do
    local f=fixture();f:bootstrap();f.puppet_error="puppet pose shipping profile unsupported"
    local ok,why=f.game:apply(f.scene)
    check(ok==nil and why==f.puppet_error,"non-loss puppet refusal is fatal with its reason")
    check(not f.game.ready and f.clears==0,"fatal puppet refusal keeps the originals for the controller to stop")
end
do
    local f=fixture();f:bootstrap();f.game:apply(f.scene)
    local next={generation="g9",epoch=-7,dir_seq=9,entities={}}
    local ok,why=f.game:apply(next)
    check(ok==nil and why==Gameplay.PENDING and f.clears==1 and f.game.key==nil,"a new generation after bootstrap clears and rebuilds from the next full scene")
end
do
    local f=fixture();for _=1,4 do f.game:apply(f.scene)end
    f.armor_error="original construction armor restore refused"
    local ok,why=f.game:apply(f.scene)
    check(ok==nil and why==f.armor_error and f.equipment==0 and f.puppets==0,"gear restoration failure is fatal before possession")
end
do
    local f=fixture();for _=1,3 do f.game:apply(f.scene)end
    f.setup_hook=function()f.world="w2"end
    local ok,why=f.game:apply(f.scene)
    check(ok==nil and why=="native gameplay world changed"and f.game.rows[1].stage=="native_setup","a world change inside a native call refuses")
    f.game:drop();check(f.forgot==1 and #f.game.rows==0 and not f.game.ready,"world drop forgets native handles")
end
for _,mutation in ipairs({function(r)r.schema=6 end,function(r)r.components={}end,function(r)r.equipment=nil end})do
    local f=fixture();mutation(f.scene.entities[1].recipe)
    local ok=f.game:apply(f.scene)
    check(ok==nil and #f.calls==0,"wrong or incomplete bootstrap recipe refuses before native spawn")
end
do
    local f=fixture();f.game:apply(f.scene);check(f.game:owned(f.pawns[1]),"suppression exemption recognises an owned original pawn")
    local foreign={GetAddress=function()return 2000 end,GetFName=function()return 1 end,GetClass=function()return{GetAddress=function()return 100 end}end,HasAnyFlags=function()return false end}
    check(not f.game:owned(foreign),"a foreign pawn gets no ownership exemption")
end
do
    local f=fixture();f.game:apply(f.scene);f.scene.generation="g9";f.game:sync(f.scene)
    check(f.clears==1 and f.game.key==nil,"recipe generation change clears original native handles")
end
do
    local closes=0;local core=Core.new({gameplay=true,now=function()return 0 end,clear=function()error("owned cleanup refused")end,close=function()closes=closes+1 end,report=function()end})
    core:stop("original failure");core:stop("repeat");check(closes==1 and core.stopped,"cleanup failure still closes endpoint once after stop latches")
end
do
    local f=fixture();f:bootstrap();f.puppet_error="native gameplay puppet lost: pawn";f.game:apply(f.scene)
    f.puppet_error=nil;f:bootstrap()
    for _=1,600 do f.game:apply(f.scene)end
    check(f.game.rebuilds==0,"a long run of good frames forgives earlier puppet losses")
end
do
    local f=fixture();f.shown=0;f:bootstrap()
    for _=1,120 do f.game:apply(f.scene)end
    local stalled=false;for _,row in ipairs(f.diagnostics)do if row.kind=="stall"then stalled=true end end
    check(stalled,"frames that never publish an authority pose are reported as a stall")
    f.shown=nil;local g=fixture();g:bootstrap();for _=1,120 do g.game:apply(g.scene)end
    for _,row in ipairs(g.diagnostics)do check(row.kind~="stall","published poses never report a stall")end
end
check(n>30,"focused staged gameplay coverage")
