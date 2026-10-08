local E=dofile(T.path("mods/HSMPLoadout/Scripts/remote_empty_left.lua"))
local function env()
    local w={writes=0,cleanups=0,rebinds=0,bumps=0,weight=7,fail_rebind=true}
    local null={GetAddress=function()return 0 end,IsValid=function()return false end}
    local world={GetAddress=function()return 500 end}
    local pawn={GetAddress=function()return 10 end}
    local function actor(address,name)
        return {GetAddress=function()return address end,GetFName=function()return {ToString=function()return name end}end,
            GetClass=function()return {GetFName=function()return {ToString=function()return "Polearm_C"end}end}end,
            GetWorld=function()return world end,IsValid=function(self)return not self.destroyed end,
            IsActorBeingDestroyed=function(self)return self.destroyed==true end,["Parent Actor"]=pawn,passport="full"}
    end
    w.null,w.pawn,w.world=null,pawn,world
    w.right,w.left=actor(11,"Right"),actor(12,"Left")
    pawn["Weapon R"],pawn["Weapon L"],pawn.R_GripType_Current=w.right,w.left,3
    w.context={world="5|500@arena",world_address=500,pawn="Pawn",address=10,mesh=20,mesh_name="CharacterMesh0",peer=2,match_id=9007199254740993,round=2,life=130,spawn_id=513}
    w.wanted={class="polearm",passport="full"}
    w.helper=E.new({context=function()return w.context end,wanted_key=function(want)return want.class..want.passport end,
        matches=function(a,want)
            if w.readback_throw then error("native passport unavailable")end
            return a.passport==want.passport
        end,
        clear_passport=function()w.writes=w.writes+1;return true end,
        cleanup=function(p,pass)
            w.cleanups=w.cleanups+1;w.none=pass
            w.weight=w.weight-2;p["Weapon L"].destroyed=true;p["Weapon L"]=null
            if w.fail_cleanup then error("after native teardown")end
        end,
        reequip=function(p,a)
            if w.fail_rebind then return nil,"preflight" end
            w.rebinds=w.rebinds+1
            w.weight=w.weight-5
            if w.attempted_failure then return nil,"attempted" end
            T.check(a==p["Weapon R"] and p.R_GripType_Current>0,"rebind retains the current real actor with nonzero grip")
            p.R_GripType_Current=14 -- the mocked native setup's own default read
            w.weight=w.weight+5
            if w.fail_post_read then w.readback_throw=true end
            if w.fail_post_property then
                p["Weapon R"]=nil;setmetatable(p,{__index=function(_,key)if key=="Weapon R"then error("native field unavailable")end end})
            end
            return "native same actor","complete"
        end,replaced=function()w.bumps=w.bumps+1 end})
    return w
end
do
    local w=env();w.pawn["Weapon L"]=w.null
    local ok,res=w.helper:prepare(w.pawn,2,w.wanted)
    T.check(ok and res=="none" and w.writes==1 and w.cleanups==0,"positive native nullptr removes the passport without Left setup")
    T.check(w.helper:finish(w.pawn,2,w.wanted) and w.rebinds==0,"already-empty L creates no R rebind debt")
end
for _,kind in ipairs({"nil","missing","throw","invalid","foreign_parent","foreign_world"})do
    local w=env()
    if kind=="nil"then w.pawn["Weapon L"]=nil
    elseif kind=="missing"then w.pawn["Weapon L"]={GetAddress=function()return -1 end}
    elseif kind=="throw"then w.pawn["Weapon L"]=nil;setmetatable(w.pawn,{__index=function()error("unreadable native field")end})
    elseif kind=="invalid"then w.left.destroyed=true
    elseif kind=="foreign_parent"then w.left["Parent Actor"]={GetAddress=function()return 99 end}
    else w.left.GetWorld=function()return {GetAddress=function()return 501 end}end end
    T.check(not w.helper:prepare(w.pawn,2,w.wanted) and w.writes==0 and w.cleanups==0,kind.." cannot authorize a None passport/native write")
end
do
    local w=env()
    T.check(w.helper:prepare(w.pawn,2,w.wanted) and w.pawn["Weapon L"]==w.null and w.weight==5,"actual unwanted L uses native teardown exactly once")
    T.check(w.none.class==nil and w.none.name=="None" and w.none.mat_steel==1 and w.none.mat_colored==4
        and w.none.mat_wood==14 and w.none.mat_leather==10 and w.none.color_leather[1]==0.186343,"None cleanup uses the exported native literal defaults")
    local debt=w.helper.debts[2]
    T.check(debt and debt.right.address==11 and debt.right.name=="Right" and debt.right.GetAddress==nil,"debt retains scalar identity, no UObject")
    T.check(not w.helper:finish(w.pawn,2,w.wanted) and w.helper.debts[2]==debt,"failed native R rebind retains exact debt")
    T.check(w.helper:prepare(w.pawn,2,w.wanted) and w.cleanups==1 and w.helper.debts[2]==debt,"retry with now-null L preserves debt and does not subtract weight twice")
    w.fail_rebind=false
    T.check(w.helper:finish(w.pawn,2,w.wanted) and w.pawn["Weapon R"]==w.right and not w.right.destroyed,"native R recovery keeps the original actor")
    local n=w.rebinds
    w.helper:prepare(w.pawn,2,w.wanted);w.helper:finish(w.pawn,2,w.wanted)
    T.check(w.rebinds==n and w.cleanups==1 and w.weight==5 and not w.helper.debts[2],"completed snapshots cannot repeatedly rebind or create another actor")
end
for _,grip in ipairs({0,-1,256,"3",false})do
    local w=env();w.helper:prepare(w.pawn,2,w.wanted);w.pawn.R_GripType_Current=grip
    local writes=w.writes
    T.check(not w.helper:finish(w.pawn,2,w.wanted) and w.rebinds==0 and w.writes==writes and w.helper.debts[2],"unsafe grip "..tostring(grip).." fails before native retained-actor/CP writes")
end
for _,field in ipairs({"world","address","mesh","mesh_name","pawn","peer","match_id","round","life","spawn_id"})do
    local w=env();w.helper:prepare(w.pawn,2,w.wanted)
    w.context[field]=type(w.context[field])=="string" and w.context[field].."new" or w.context[field]+1
    T.check(not w.helper:finish(w.pawn,2,w.wanted) and w.rebinds==0 and w.helper.debts[2]==nil,"changed "..field.." cannot reuse old-life debt")
end
do
    local w=env();w.fail_rebind=false;w.attempted_failure=true
    w.helper:prepare(w.pawn,2,w.wanted)
    T.check(not w.helper:finish(w.pawn,2,w.wanted) and w.rebinds==1 and w.weight==0 and w.helper.uncertain[2],
        "native R weight subtraction then failure records attempted completion uncertainty")
    w.attempted_failure=false
    T.check(not w.helper:prepare(w.pawn,2,w.wanted) and not w.helper:finish(w.pawn,2,w.wanted)
        and w.rebinds==1 and w.weight==0,"uncertain R completion cannot repeat native weight accounting")
end
for _,kind in ipairs({"fail_post_read","fail_post_property"})do
    local w=env();w.fail_rebind=false;w[kind]=true
    w.helper:prepare(w.pawn,2,w.wanted)
    T.check(not w.helper:finish(w.pawn,2,w.wanted) and w.helper.uncertain[2] and w.rebinds==1,
        kind.." after completed native call is unproved and latches uncertainty")
    T.check(not w.helper:finish(w.pawn,2,w.wanted) and w.rebinds==1,
        kind.." cannot escape into another native call")
end
do
    local w=env();w.helper:prepare(w.pawn,2,w.wanted)
    local prior=w.context;local debt=w.helper.debts[2];w.context=nil
    T.check(not w.helper:finish(w.pawn,2,w.wanted) and w.helper.debts[2]==debt,"unknown scope retains metadata without using it")
    w.context=prior;w.fail_rebind=false
    T.check(w.helper:finish(w.pawn,2,w.wanted),"restored exact context can finish the pending retained-actor rebind")
end
do
    local w=env();w.fail_cleanup=true
    T.check(not w.helper:prepare(w.pawn,2,w.wanted) and not w.helper.debts[2],"throw after partial native L cleanup is not a successful rebind authorization")
    local writes=w.writes
    T.check(not w.helper:prepare(w.pawn,2,w.wanted) and w.writes==writes and w.cleanups==1,"uncertain cleanup cannot become healthy just because its field is now null")
    w.helper:forget(2)
    T.check(not w.helper:allow(w.pawn,2),"explicit L/appearance change cannot conceal ambiguous same-body native accounting")
    w.context.life=w.context.life+1
    T.check(w.helper:allow(w.pawn,2) and not w.helper.uncertain[2],"a new complete life scope clears uncertainty without touching old actors")
    w.helper:reset()
    T.check(not next(w.helper.debts) and not next(w.helper.uncertain),"world teardown clears metadata without touching the old actors")
end
