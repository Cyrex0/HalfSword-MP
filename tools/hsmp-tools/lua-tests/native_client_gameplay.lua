local Gameplay=dofile("mods/HSMPMatch/Scripts/native_client_gameplay.lua")
local Core=dofile("mods/HSMPMatch/Scripts/native_client_core.lua")
local n=0
local function check(value,label)n=n+1;T.check(value,label);assert(value,label)end
local function fixture()
    local f={world="w1",calls={},actions={},progress={},clears=0,forgot=0,equipment=0,armor=0,weapons=0,apply_count=0,confirmed=0,scalar_guards=0,wrappers=0,clock=0,timings={}}
    local cls={GetAddress=function()return 100 end}
    local function pawn(id)
        local p={GetAddress=function()return id+1000 end,GetFName=function()return id end,GetClass=function()return cls end,HasAnyFlags=function()return false end}
        for _,name in ipairs({"InpAxisEvt_Move Forward / Backward_K2Node_InputAxisEvent_14","InpAxisEvt_Move Right / Left_K2Node_InputAxisEvent_19",
            "InpActEvt_Run_K2Node_InputActionEvent_4","InpActEvt_Run_K2Node_InputActionEvent_5"})do
            p[name]=setmetatable({type=function()return"UFunction"end,IsValid=function()return true end,
                GetFullName=function()return"Function /Game/Character/Blueprints/Willie_BP.Willie_BP_C:"..name end},
                {__call=function(_,self,value)check(self==p,"accepted native action targets exact original pawn");f.actions[#f.actions+1]={id=id,name=name,value=value};f.clock=f.clock+2 end})
        end
        return p
    end
    f.pawns={pawn(1),pawn(2)}
    local function recipe(id)return{schema=1,actor_class="/Game/Character/Blueprints/Willie_BP.Willie_BP_C",team=id,
        passport={fixture_id=id},construction={},equipment={}}end
    f.scene={epoch=-7,dir_seq=8,generation="g8",peer_id=9,state=1,fresh=true,frame_seq=40,authority_tick=40,received_age_ms=4,
        entities={{epoch=-7,id=1,incarnation=2,revision=3,owner_peer=9,kind=0,recipe=recipe(1),buttons=1,axes={0.25,-0.5,0,0,0,0,0,0}},
            {epoch=-7,id=2,incarnation=4,revision=5,owner_peer=10,kind=0,recipe=recipe(2),buttons=0,axes={0,0,0,0,0,0,0,0}}}}
    local N={}
    N.native_gameplay_begin=function(scene,world,pc,id)
        check(scene==f.scene and world==88 and pc==77,"begin uses exact current scene/world/controller")
        f.calls[#f.calls+1]="begin"..id;return id,f.pawns[id]
    end
    N.native_gameplay_current=function(handle,scalar)
        if f.expired then return nil,"original native weak expired"end
        if scalar==true then
            f.scalar_guards=f.scalar_guards+1
            if f.scalar_error then return nil,f.scalar_error end
            if f.scalar_result~=nil then return f.scalar_result end
            return true
        end
        f.wrappers=f.wrappers+1;return f.pawns[handle]
    end
    N.native_gameplay_construct=function(handle)f.calls[#f.calls+1]="construct"..handle;return true,f.pawns[handle]end
    N.native_gameplay_finish=function(handle)f.calls[#f.calls+1]="possess"..handle;return true end
    N.native_gameplay_apply=function(scene)f.apply_count=f.apply_count+1;f.calls[#f.calls+1]="apply";f.clock=f.clock+20;return true,f.applied_scene or scene end
    N.native_gameplay_confirm=function(scene)
        f.confirmed=f.confirmed+1;f.calls[#f.calls+1]="confirm";f.clock=f.clock+30;if f.confirm_error then return nil,f.confirm_error end;return true,scene
    end
    N.native_gameplay_clear=function(forget)f.clears=f.clears+1;if forget then f.forgot=f.forgot+1 end;return true end
    local Passport={before_finish=function(recipe,env)
        check(env.guard()==true and env.current()==f.pawns[recipe.passport.fixture_id],"passport writes only fresh original deferred pawn")
        f.calls[#f.calls+1]="passport"..recipe.passport.fixture_id;return true
    end,restore_live_armor=function(recipe,env)
        check(env.guard()==true,"live armor restoration remains original-pawn guarded")
        f.armor=f.armor+1;f.calls[#f.calls+1]="armor"..recipe.passport.fixture_id
        if f.armor_error then return nil,f.armor_error end;return true
    end,restore_live_weapons=function(recipe,env)
        check(env.guard()==true,"live weapon restoration remains original-pawn guarded")
        f.weapons=f.weapons+1;f.calls[#f.calls+1]="weapons"..recipe.passport.fixture_id
        if f.weapon_error then return nil,f.weapon_error end;return true
    end,verify_equipment=function(recipe,env)
        f.equipment=f.equipment+1;check(env.guard()==true,"native gear checks remain guarded")
        f.clock=f.clock+100
        f.calls[#f.calls+1]="gear"..recipe.passport.fixture_id
        if f.gear_error then return nil,f.gear_error end;return true
    end}
    f.game=Gameplay.new({native=N,passport=Passport,same=function(token)return token==f.world end,
        now_us=function()if f.clock_error then error("diagnostic clock unavailable")end;return f.clock end,
        diagnostic=function(row)
            check(f.calls[#f.calls]=="confirm"or not row.ok,"timing emitted only after native proof or original refusal")
            f.timings[#f.timings+1]=row;if f.log_error then error("diagnostic sink refused")end
        end,
        world=function()return{GetAddress=function()return 88 end},f.world,{GetAddress=function()return 77 end}end,
        world_address=function()return 88 end,progress=function(stage,done,total)
            if done~=nil or total~=nil then f.progress_counts=true end
            f.progress[#f.progress+1]=stage
        end,fname=function(name)return name end})
    function f:bootstrap()
        for _=1,12 do local ok,why=self.game:apply(self.scene);check(ok==nil and why==Gameplay.PENDING,"every bootstrap stage yields pending without readiness")end
    end
    return f
end
do
    local f=fixture();f:bootstrap();check(#f.timings==0,"bootstrap and READY work do not emit per-LIVE timing")
    f.scene.state=2;f.scene.gameplay_proof=true;f.applied_scene={}
    for key,value in pairs(f.scene)do f.applied_scene[key]=value end;f.applied_scene.received_age_ms=17
    local ok,actual=f.game:apply(f.scene);local row=f.timings[1]
    check(ok==true and actual==f.applied_scene and row.epoch==-7 and row.frame_seq==40 and row.authority_tick==40,
        "timing preserves original signed epoch and integer result provenance")
    check(row.received_age_entry_ms==4 and row.applied_received_age_ms==17 and row.applied_authority_tick==40 and
        row.elapsed_apply_return_to_confirm_us==200,"timing separates exact native returned original receipt age from elapsed gear time")
    check(row.native_apply_us==20 and row.gear_us==200 and row.confirm_us==30 and row.movement_us==12 and row.total_us==262 and
        row.rows[1].id==1 and row.rows[1].incarnation==2 and row.rows[1].us==100 and row.rows[2].id==2 and row.rows[2].us==100,
        "timing attributes every full original gear row and native stage without changing proof execution")
    for _=1,11 do f.game:apply(f.scene)end
    check(#f.timings==8,"successful per-LIVE timing has a fixed first-eight emission bound")
    f.confirm_error="native gameplay result is stale";local success,reason=f.game:apply(f.scene)
    check(success==nil and reason==f.confirm_error and #f.timings==9 and f.timings[9].stage=="confirm" and not f.timings[9].ok and
        f.timings[9].applied_received_age_ms==17,"first later refusal records its failing edge while preserving original stale reason and receipt")
    f.game:apply(f.scene);check(#f.timings==9,"later refusal retries cannot exceed the timing budget")
end
do
    local f=fixture();f:bootstrap();f.scene.state=2;f.scene.gameplay_proof=true;f.gear_error="original gear changed"
    local ok,why=f.game:apply(f.scene)
    check(ok==nil and why==f.gear_error and f.timings[1].stage=="gear" and f.timings[1].rows[1].us==100 and f.confirmed==0,
        "failed gear row timing remains copied and prevents premature confirmation")
end
for _,fault in ipairs({"clock_error","log_error"})do
    local f=fixture();f:bootstrap();f.scene.state=2;f.scene.gameplay_proof=true;f[fault]=true
    local ok,actual=f.game:apply(f.scene)
    check(ok==true and actual==f.scene and f.confirmed==1,"optional diagnostic failure never changes native readiness or original result")
end
for _,state in ipairs({1,2})do
    local f=fixture();f:bootstrap();f.scene.state=state;f.scene.gameplay_proof=true
    check(f.game:apply(f.scene)==true,"original fresh gameplay result establishes view proof")
    local actions,applies,confirms,armor,weapons=#f.actions,f.apply_count,f.confirmed,f.armor,f.weapons
    local original=f.scene;original.fresh=false
    local ok,why=f.game:apply(original)
    check(ok==nil and why=="native gameplay result is stale"and not f.game:view_ready(original)and#f.actions==actions and f.apply_count==applies and f.confirmed==confirms,
        "completed stale READY/LIVE result clears view proof before movement, native application or confirmation")
    f.scene={};for key,value in pairs(original)do f.scene[key]=value end
    f.scene.fresh=true;f.scene.frame_seq=41;f.scene.authority_tick=41
    local applied,actual=f.game:apply(f.scene)
    check(applied==true and actual==f.scene and f.game:view_ready(actual)and f.armor==armor and f.weapons==weapons and original.fresh==false and original.frame_seq==40,
        "new fresh gameplay result reuses prepared pawns and all full gear checks without renewing the old result")
    f.confirm_error="native gameplay result is stale";ok,why=f.game:apply(f.scene)
    check(ok==nil and why==f.confirm_error and not f.game:view_ready(f.scene),"expiry after full gear/native apply removes the previous positive view proof")
end
do
    local f=fixture();f:bootstrap()
    check(table.concat(f.progress,",")=="present,character,present,equipment,controls,check_equipment,present,character,present,equipment,controls,check_equipment",
        "each yielded native preparation step reports its actual work stage")
    check(table.concat(f.calls,",")=="begin1,passport1,construct1,armor1,weapons1,gear1,possess1,gear1,begin2,passport2,construct2,armor2,weapons2,gear2,possess2,gear2","native construction/live armor/weapons/gear/possession order including post-possession proof")
    check(f.apply_count==0 and f.confirmed==0 and not f.game:view_ready(f.scene),"receipt and staged construction never become view readiness")
    check(f.scalar_guards>0 and f.wrappers>0,"discarded guards use explicit scalar admission while actual getters retain full wrappers")
    f.scene.gameplay_proof=true;local ok,actual=f.game:apply(f.scene)
    check(f.progress[#f.progress]=="sync","finished preparation waits for current authoritative state without claiming readiness")
    check(not f.progress_counts,"gameplay preparation and synchronization never invent an overall completion count")
    check(ok==true and actual==f.scene and f.confirmed==1 and f.game:view_ready(actual),"complete raw state proof follows whole-roster Lua gear callbacks before readiness")
    check(table.concat(f.calls,","):match("apply,gear1,gear2,confirm$"),"confirmation runs after both final gear checks")
    check(#f.actions==6 and f.actions[1].value==0.25 and f.actions[2].value==-0.5 and f.actions[3].value.KeyName=="None","exact accepted movement and source default FKey Run replay before reconciliation")
    f.game:apply(f.scene);check(#f.actions==10 and f.armor==2 and f.weapons==2,"held Run never repeats press and gear restoration runs once per constructed pawn")
    local axes,buttons=Gameplay.intent(actual,actual.entities[1]);check(axes[1]==1 and axes[2]==0 and buttons==1,"first acceptance intent is only Move+Run")
    actual.fresh=false;check(Gameplay.intent(actual,actual.entities[1])==nil,"stale result cannot drive even bounded test intent")
    f.game:drop();check(f.forgot==1 and #f.game.rows==0,"world drop is scalar forget only")
end
do
    local f=fixture();f.game:apply(f.scene);f.scalar_error="gameplay stage: original native weak expired"
    local ok,why=f.game:apply(f.scene)
    check(ok==nil and why==f.scalar_error and f.wrappers==0 and #f.calls==1 and f.confirmed==0,
        "scalar admission refusal preserves exact reason and prevents wrapper construction or stage advancement")
end
for _,value in ipairs({false,{},1})do
    local f=fixture();f.game:apply(f.scene);f.scalar_result=value
    local ok,why=f.game:apply(f.scene)
    check(ok==nil and why=="native gameplay original pawn guard unavailable" and f.wrappers==0 and #f.calls==1,
        "only exact boolean true admits scalar pawn guard")
end
do
    local f=fixture();f:bootstrap();f.scene.gameplay_proof=true;f.gear_error="original armor changed in later callback"
    local ok,why=f.game:apply(f.scene);check(ok==nil and why==f.gear_error and f.confirmed==0,"late native gear mutation refuses before input acknowledgment")
end
do
    local f=fixture();f:bootstrap();f.scene.gameplay_proof=true;f.confirm_error="native gameplay final root changed"
    local ok,why=f.game:apply(f.scene);check(ok==nil and why==f.confirm_error and not f.game:view_ready(f.scene),"last native raw census refusal never fabricates readiness")
end
do
    local f=fixture();for _=1,3 do f.game:apply(f.scene)end
    f.armor_error="original construction armor restore refused"
    local ok,why=f.game:apply(f.scene)
    check(ok==nil and why==f.armor_error and f.equipment==0 and f.confirmed==0,
        "construction armor restoration failure prevents verification, possession and readiness")
end
do
    local f=fixture();for _=1,3 do f.game:apply(f.scene)end
    f.weapon_error="native exact weapon binding unsupported"
    local ok,why=f.game:apply(f.scene)
    check(ok==nil and why==f.weapon_error and f.armor==1 and f.weapons==1 and f.equipment==0 and f.confirmed==0,
        "native weapon restoration failure prevents verification, possession and readiness")
end
do
    local f=fixture();f.game:apply(f.scene);check(f.game:owned(f.pawns[1]),"suppression exemption requires a freshly requalified owned native pawn")
    local foreign={GetAddress=function()return 2000 end,GetFName=function()return 1 end,GetClass=function()return{GetAddress=function()return 100 end}end,HasAnyFlags=function()return false end}
    check(not f.game:owned(foreign),"foreign same-name pawn cannot acquire ownership exemption")
    f.expired=true;check(not pcall(f.game.owned,f.game,f.pawns[1]),"expired original native weak refuses rather than rebinds")
end
do
    local f=fixture();f.game:apply(f.scene);f.scene.generation="g9";f.game:sync(f.scene)
    check(f.clears==1 and f.game.key==nil,"recipe generation change clears original native handles before suppression")
end
do
    local closes=0;local core=Core.new({gameplay=true,now=function()return 0 end,clear=function()error("owned cleanup refused")end,close=function()closes=closes+1 end,report=function()end})
    core:stop("original failure");core:stop("repeat");check(closes==1 and core.stopped,"cleanup failure still closes endpoint once after stop latches")
end
for _,mutation in ipairs({function(r)r.schema=6 end,function(r)r.components={}end,function(r)r.equipment=nil end})do
    local f=fixture();mutation(f.scene.entities[1].recipe)
    local ok=f.game:apply(f.scene)
    check(ok==nil and #f.calls==0,"wrong/full-render/incomplete bootstrap refuses before native spawn")
end
check(n>50,"focused staged gameplay coverage")
