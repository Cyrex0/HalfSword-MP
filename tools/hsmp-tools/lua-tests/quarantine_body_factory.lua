local M=dofile(T.path("mods/dev/HSMPParity/Scripts/quarantine_body_factory.lua"))
local source={name="native-source",address=500,world_id="native-world-incarnation-1",match_id=81,round=2,life=1}
local function fixture()
    local world={id="native-world-incarnation-1",address=10,name="World Arena"}
    local actors,events={},{}
    local masses={};for i=1,22 do masses["bone"..i]=i*.5 end
    local baseline={body_count=22,native_joint_count=21,height=.9457,actor_scale={.9932,.9932,.9932},
        mesh_scale={.9966,.9966,.9966},physics_asset="exact-native-asset-hash",masses=masses,
        motors_ready=true,controls_ready=true,tick_owned=true,player=false,ai_active=false,
        hidden=true,collision=false,physics=true}
    local snapshot={complete=true,source=source,passport_hash="full-native-passport-hash",passport={height=.9457,
        equipment={hands={right="captured-weapon",left="None"}},body={mass_rate=1.005},character_id=91},baseline=baseline}
    local proof={full_passport_capture=true,deferred_passport_copy=true,passport_readback=true,
        deferred_nonpossessing_spawn=true,finish_normal_beginplay=true,fresh_identity_resolve=true,
        dormant_readback=true,native_readiness=true,owned_pair_control=true}
    local env={proof=proof,world=function()return world end,capture=function()return snapshot end}
    env.begin=function(_,opts)
        events[#events+1]="begin"
        T.check(opts.player==false and opts.auto_possess_player==0 and opts.auto_possess_ai==0
            and opts.spawn_ai==false and opts.hidden and opts.collision==false,
            "constructor requests no possession/AI and starts hidden with collision disabled")
        local a={name="quarantine"..(#actors+1),address=100+#actors,readback=baseline}
        actors[#actors+1]=a;return a
    end
    env.identity=function(a)return {name=a.name,address=a.address}end
    env.apply_passport=function(a,p,hash)a.passport=p;a.hash=hash;events[#events+1]="passport";return true end
    env.passport_hash=function(a)events[#events+1]="readback";return a.hash end
    env.finish=function(a)
        T.check(a.passport and a.passport.equipment.hands.right=="captured-weapon",
            "entire captured passport precedes finish, not merely height")
        events[#events+1]="finish";return true
    end
    env.resolve=function(id)
        events[#events+1]="resolve"
        for _,a in ipairs(actors)do if a.name==id.name and a.address==id.address then return a end end
    end
    env.probe=function(a)return a.readback end
    env.dormant=function(a,opts)
        events[#events+1]="dormant"
        a.readback={physics=false,collision=false,hidden=true,ai_active=false,tick_owned=false}
        return true
    end
    env.activate_pair=function(_,_,token)T.check(token==M.TOKEN,"owned pair consumes explicit quarantine ledger token");return true end
    return env,actors,events,snapshot,function(w)world=w end
end
do
    local env,actors,events=fixture();env.proof.full_passport_capture=nil
    local F=M.new(env);local id,why=F:construct(source)
    T.check(id==nil and why=="unproved:full_passport_capture"and #events==0,
        "unproved full passport API fails before any native allocation")
end
do
    local env,actors,events,snapshot,setworld=fixture()
    local F=M.new(env);local one=F:construct(source);local two=F:construct(source)
    T.check(events[1]=="begin"and events[2]=="passport"and events[3]=="readback"and events[4]=="finish",
        "constructor order is deferred begin, full passport, exact readback, normal finish")
    snapshot.passport.equipment.hands.right="later-mutated-source"
    T.check(actors[1].passport.equipment.hands.right=="captured-weapon","captured full passport is immutable")
    T.check(F:construct(source)==nil,"maximum two native Willies per world")
    actors[1].readback.native_joint_count=20
    T.check(not F:poll(one),"missing native joint blocks readiness")
    actors[1].readback.native_joint_count=21
    T.check(F:poll(one) and F:poll(two),"22 bodies/21 joints, height/scales/masses, PA and controls verified")
    local ok,result=F:with_pair(one,two,function(a,b)return {a=a.name,b=b.name}end)
    T.check(ok and result.a~=result.b and F:owned(one).state=="dormant"and F:owned(two).state=="dormant",
        "only owned probe pair activates, then both retire to dormant pool")
    T.check(F:construct(source)==nil,"dormancy never frees native world allocation cap")
    T.check(not F:retire(999),"user/unowned actor has no cleanup route")
    local n=#events;setworld({id="new-world",name="World Arena",address=10})
    T.check(not F:retire(one) and #events==n,"recycled world address never resolves or touches old body")
    T.check(F:construct(source)==nil and #events==n,"reconnect token cannot reopen a full native world pool")
end
do
    local env,actors,events=fixture();env.identity=function()error("native name unavailable")end
    local F=M.new(env);local one=F:construct(source);local two=F:construct(source)
    T.check(F:owned(one).state=="orphan"and F:owned(two).state=="orphan"and F:construct(source)==nil,
        "partially allocated unresolved bodies consume bounded pool immediately")
end
do
    local env,actors,events=fixture();env.finish=function()error("native finish failure")end
    local F=M.new(env);local id,why=F:construct(source)
    T.check(id and why and F:owned(id).state=="dormant","failed finish retires by fresh identity without destroy")
end
do
    local env,actors,events=fixture();local F=M.new(env)
    local one,two=F:construct(source),F:construct(source);F:poll(one);F:poll(two)
    local ok=F:with_pair(one,two,function()error("native A/B probe failure")end)
    T.check(not ok and F:owned(one).state=="dormant"and F:owned(two).state=="dormant",
        "probe exception still disables physics/collision and AI on fresh owned identities")
end
do
    local env,actors,events=fixture()
    env.begin=function()return {name=source.name,address=source.address}end
    local F=M.new(env);local id=F:construct(source)
    T.check(F:owned(id).state=="orphan"and #events==0,
        "constructor returning the captured user source is refused before any Passport or dormancy write")
end
do
    local env,actors,events=fixture();local F=M.new(env)
    local id=F:construct(source);F:poll(id)
    local old=env.dormant;env.dormant=function()return false end
    T.check(not F:retire(id)and F:owned(id).state=="retired","native dormancy failure remains explicit retryable ownership")
    env.dormant=old
    T.check(F:retire(id)and F:owned(id).state=="dormant","cleanup retries resolve exact current actor before recovery")
end
do
    local env,actors,events,snapshot=fixture();snapshot.source={name="wrong-pawn"}
    local F=M.new(env)
    T.check(F:construct(source)==nil and #events==0,"wrong original source Passport lineage refuses before spawn")
end

