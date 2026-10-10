local Gameplay=dofile("mods/HSMPMatch/Scripts/native_client_gameplay.lua")
local Core=dofile("mods/HSMPMatch/Scripts/native_client_core.lua")
local n=0
local function check(value,label)n=n+1;T.check(value,label);assert(value,label)end
local function fixture()
    local f={world="w1",calls={},actions={},clears=0,forgot=0,equipment=0,apply_count=0,confirmed=0}
    local cls={GetAddress=function()return 100 end}
    local function pawn(id)
        local p={GetAddress=function()return id+1000 end,GetFName=function()return id end,GetClass=function()return cls end,HasAnyFlags=function()return false end}
        for _,name in ipairs({"InpAxisEvt_Move Forward / Backward_K2Node_InputAxisEvent_14","InpAxisEvt_Move Right / Left_K2Node_InputAxisEvent_19",
            "InpActEvt_Run_K2Node_InputActionEvent_4","InpActEvt_Run_K2Node_InputActionEvent_5"})do
            p[name]=setmetatable({type=function()return"UFunction"end,IsValid=function()return true end,
                GetFullName=function()return"Function /Game/Character/Blueprints/Willie_BP.Willie_BP_C:"..name end},
                {__call=function(_,self,value)check(self==p,"accepted native action targets exact original pawn");f.actions[#f.actions+1]={id=id,name=name,value=value}end})
        end
        return p
    end
    f.pawns={pawn(1),pawn(2)}
    f.scene={epoch=-7,dir_seq=8,generation="g8",peer_id=9,state=1,fresh=true,frame_seq=40,
        entities={{epoch=-7,id=1,incarnation=2,revision=3,owner_peer=9,kind=0,recipe={id=1},buttons=1,axes={0.25,-0.5,0,0,0,0,0,0}},
            {epoch=-7,id=2,incarnation=4,revision=5,owner_peer=10,kind=0,recipe={id=2},buttons=0,axes={0,0,0,0,0,0,0,0}}}}
    local N={}
    N.native_gameplay_begin=function(scene,world,pc,id)
        check(scene==f.scene and world==88 and pc==77,"begin uses exact current scene/world/controller")
        f.calls[#f.calls+1]="begin"..id;return id,f.pawns[id]
    end
    N.native_gameplay_current=function(handle)if f.expired then return nil,"original native weak expired"end;return f.pawns[handle]end
    N.native_gameplay_construct=function(handle)f.calls[#f.calls+1]="construct"..handle;return true,f.pawns[handle]end
    N.native_gameplay_finish=function(handle)f.calls[#f.calls+1]="possess"..handle;return true end
    N.native_gameplay_apply=function(scene)f.apply_count=f.apply_count+1;f.calls[#f.calls+1]="apply";return true,scene end
    N.native_gameplay_confirm=function(scene)
        f.confirmed=f.confirmed+1;f.calls[#f.calls+1]="confirm";if f.confirm_error then return nil,f.confirm_error end;return true,scene
    end
    N.native_gameplay_clear=function(forget)f.clears=f.clears+1;if forget then f.forgot=f.forgot+1 end;return true end
    local Passport={before_finish=function(recipe,env)
        check(env.guard()==true and env.current()==f.pawns[recipe.id],"passport writes only fresh original deferred pawn")
        f.calls[#f.calls+1]="passport"..recipe.id;return true
    end,verify_equipment=function(recipe,env)
        f.equipment=f.equipment+1;check(env.guard()==true,"native gear checks remain guarded")
        f.calls[#f.calls+1]="gear"..recipe.id
        if f.gear_error then return nil,f.gear_error end;return true
    end}
    f.game=Gameplay.new({native=N,passport=Passport,same=function(token)return token==f.world end,
        world=function()return{GetAddress=function()return 88 end},f.world,{GetAddress=function()return 77 end}end,
        world_address=function()return 88 end,progress=function()end,fname=function(name)return name end})
    function f:bootstrap()
        for _=1,12 do local ok,why=self.game:apply(self.scene);check(ok==nil and why==Gameplay.PENDING,"every bootstrap stage yields pending without readiness")end
    end
    return f
end
do
    local f=fixture();f:bootstrap()
    check(table.concat(f.calls,",")=="begin1,passport1,construct1,gear1,possess1,gear1,begin2,passport2,construct2,gear2,possess2,gear2","native construction/passport/gear/possession order including post-possession proof")
    check(f.apply_count==0 and f.confirmed==0 and not f.game:view_ready(f.scene),"receipt and staged construction never become view readiness")
    f.scene.gameplay_proof=true;local ok,actual=f.game:apply(f.scene)
    check(ok==true and actual==f.scene and f.confirmed==1 and f.game:view_ready(actual),"complete raw state proof follows whole-roster Lua gear callbacks before readiness")
    check(table.concat(f.calls,","):match("apply,gear1,gear2,confirm$"),"confirmation runs after both final gear checks")
    check(#f.actions==6 and f.actions[1].value==0.25 and f.actions[2].value==-0.5 and f.actions[3].value.KeyName=="None","exact accepted movement and source default FKey Run replay before reconciliation")
    f.game:apply(f.scene);check(#f.actions==10,"held Run does not replay press edge on every native state application")
    local axes,buttons=Gameplay.intent(actual,actual.entities[1]);check(axes[1]==1 and axes[2]==0 and buttons==1,"first acceptance intent is only Move+Run")
    actual.fresh=false;check(Gameplay.intent(actual,actual.entities[1])==nil,"stale result cannot drive even bounded test intent")
    f.game:drop();check(f.forgot==1 and #f.game.rows==0,"world drop is scalar forget only")
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
check(n>50,"focused staged gameplay coverage")
