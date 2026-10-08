local M=dofile(T.path("mods/HSMPCombat/Scripts/native_cut_journal.lua"))
local clock,enabled,emitted,reads=100,true,{},0
local function fresh()
    emitted={};reads=0;clock=100;enabled=true
    return M.new({enabled=function()return enabled end,now=function()return clock end,
        unwrap=function(v)return v end,emit=function(r)emitted[#emitted+1]=r end})
end
local seq=100
local function fname(s)return {ToString=function()reads=reads+1;return s end}end
local function array(values)
    return {GetArrayNum=function()reads=reads+1;return #values end,
        ForEach=function(_,f)reads=reads+1;for i,v in ipairs(values)do f(i,v)end end}
end
local function object(name)
    seq=seq+1;local a=seq
    return {IsValid=function()reads=reads+1;return true end,GetAddress=function()reads=reads+1;return a end,
        GetFName=function()reads=reads+1;return fname(name)end}
end
local owner=object("Willie")
local function geometry(name)
    local o=object(name);o.ComponentTags=array({fname("HP025"),fname("Arm L|;= exact")})
    o.GetOwner=function()reads=reads+1;return owner end
    o.GetAttachParent=function()reads=reads+1;return owner end
    o.GetAttachSocketName=function()reads=reads+1;return fname("hand_r")end
    o.K2_GetComponentToWorld=function()reads=reads+1;return {Translation={X=1,Y=2,Z=3},Scale3D={X=1,Y=1,Z=1},Rotation={X=0,Y=0,Z=0,W=1}}end
    o.GetUnscaledBoxExtent=function()reads=reads+1;return {X=100,Y=500,Z=100}end
    return o
end
local function constraint()
    local o=object("Constraint_Weapon_Stuck_1")
    for _,d in ipairs(M.INIT)do
        o[d[1]]=d[2]=="ref"and geometry(d[1])or d[2]=="name"and fname("Grip")
            or d[2]=="vec"and {X=1,Y=2,Z=3}or d[2]=="bool"and false or 0
        if d[2]=="bool"then o[d[1]]=false end
    end
    o["Gore Rate"]=1;o["Is Ammo"]=false;o.Enum_DismembermentPart=4;o["Dismemberment In Progress"]=false
    o["Overlapped Markers"]=array({geometry("marker")});o["Overlapped Markers Current"]=array({})
    return o
end
local context={scope_key="world|1|match|round|full-lives",match_id=7,round=2,victim_life=3,victim_peer=2}
local function event(con,current)
    return {context=context,constraint=con,current=current or function()return true end,
        parent={association="unavailable"},damage=function()return 4 end}
end
local s=fresh();local resolves=0
s:capture("wear",function()resolves=resolves+1 end)
T.check(resolves==0 and reads==0,"default OFF consumes no parameters or native reads")
enabled=false
T.check(not s:start()and not s:install(function()error("register")end,function()end),"developer gate refuses installation and arming")
enabled=true;s:start();local con=constraint();local row=s:capture("wear",function()return event(con)end)
T.check(row and #M.INIT==25 and row.initializer.complete,"all25 actual initializer fields are copied from first observed POST")
T.check(row.phase=="Blueprint_POST"and not row.pre_available and not row.baseline_available
    and not row.history_complete and not row.relay_eligible and not row.authority,"complete reads never manufacture baseline, wear history or relay authority")
T.check(row.initializer.fields["Thrust?"].available and row.initializer.fields["Thrust?"].value==false
    and row.initializer.fields["Draw Cut"].value==0,"native false and zero remain available")
T.check(row.markers.value.values[1].tags.value[1]=="HP025"and row.markers.value.values[1].tags.value[2]=="Arm L|;= exact",
    "actual non100 marker HP and exact tag text survive scalar copying")
T.check(row.capture_elapsed_ms==0 and #emitted==1,"pure clock records total observation cost and one emitted row")
con["Draw Cut"]=99;context.match_id=8
T.check(row.initializer.fields["Draw Cut"].value==0 and row.context.match_id==7,"copied initializer/context do not alias caller tables")
context.match_id=7
row=s:capture("wear",function()return event(con)end)
T.check(row.initializer_available and row.initializer_observation=="earlier copied POST","same scoped constraint uses prior scalar evidence, not another25field scan")
local oldkey=context.scope_key;context.scope_key="world|new-life"
row=s:capture("wear",function()return event(con)end)
T.check(row.initializer and row.initializer.fields["Draw Cut"].value==99,"new full scope cannot borrow old initializer proof")
context.scope_key=oldkey
for _,case in ipairs({"stop","deadline"})do
    s=fresh();s:start();local later=0
    row=s:capture("wear",function(admitted)
        assert(admitted());if case=="stop"then s:stop("resolver getter")else clock=116 end
        assert(admitted());later=later+1;return event(constraint())
    end)
    T.check(not row and later==0 and #emitted==0,"initial resolver "..case.." uses same immutable admitted control token")
end
for _,case in ipairs({"false","throw","stop","deadline","disable","restart","recovery"})do
    s=fresh();s:start();con=constraint();local lost=false;local invoked=0
    con.IsValid=function()
        invoked=invoked+1
        if case=="stop"or case=="restart"then s:stop("during read");if case=="restart"then s:start()end
        elseif case=="deadline"then clock=116 elseif case=="disable"then enabled=false else lost=true end
        return true
    end
    con.GetAddress=function()error("old object touched after loss")end
    row=s:capture("wear",function()return event(con,function()
        if lost then if case=="throw"then error("validator")end;if case=="recovery"then lost=false end;return false end
        return true
    end)end)
    T.check(not row and invoked==1 and #emitted==0,"first getter "..case.." latches closed before any later old-object read or emission")
end
s=fresh();s:start();con=constraint();con["Thrust?"]=nil;row=s:capture("constraint_begin",function()return event(con)end)
T.check(row and not row.initializer.complete and not row.initializer.fields["Thrust?"].available,"missing native bool stays incomplete rather than false")
s=fresh();s:start();con=constraint();con["Overlapped Markers"]={GetArrayNum=function()return 33 end,ForEach=function()error("overflow iterated")end}
row=s:capture("wear",function()return event(con)end)
T.check(row and not row.markers.available,"marker overflow is unavailable before traversal")
s=fresh();s:start();con=constraint();local first,second=geometry("old"),geometry("replacement");local passes=0
con["Overlapped Markers"]={GetArrayNum=function()return 1 end,ForEach=function(_,f)passes=passes+1;f(1,passes==1 and first or second)end}
row=s:capture("wear",function()return event(con)end)
T.check(row and not row.markers.available and passes==2,"same-count marker replacement rejects membership completeness")
s=fresh();s:start();local victim=object("victim")
victim["Currently Dismembered Part"]=4;victim["Currently Dismembered Mesh"]=geometry("alternate armor master");victim["Dismemberment In Process"]=true
local args={geometry("alternate armor master"),4,geometry("attach"),array({geometry("marker")}),geometry("box1"),geometry("box2"),geometry("weapon module")}
row=s:capture("delayed",function()return {context=context,victim=victim,args=args,current=function()return true end,
    topology=function()return {available=false,reason="native topology unavailable"}end}end)
T.check(row and row.seven.box1.value.extent.value.Y==500 and row.seven.box2.value.extent.value.X==100,"native cut boxesY500 use a separate contract without changing DCD bounds")
T.check(row.seven.master.value.identity.name=="alternate armor master"and row.selected_part.value==4
    and row.process.value==true,"actual Master/current mutable Part/process are copied, never inferred from primary Mesh or input Part")
T.check(row.topology.value.available==false and not row.relay_eligible,"unavailable topology cannot become a healthy or eligible transaction")
local topology=dofile(T.path("mods/HSMPCombat/Scripts/native_topology_audit.lua"))
for _,loss in ipairs({"stop","deadline"})do
    s=fresh();s:start();local later=0;local first=0;local v=object("victim")
    v.Mesh=object("Mesh");local va,ma=v:GetAddress(),v.Mesh:GetAddress()
    setmetatable(v,{__index=function(_,key)
        if key=="Dismembered Array"then first=first+1;if loss=="stop"then s:stop("nested topology")else clock=116 end;return nil end
        if key=="Dismembered Bones"or key=="Dismembered Parts Map"or key=="Headless"then later=later+1 end
    end})
    row=s:capture("delayed",function(admitted)
        return {context=context,victim=v,args=args,current=admitted,topology=function()
            return topology.read(v,{unwrap=function(x)return x end,context=function()
                if not admitted()then return nil end
                return {world="1@World",peer=2,match_id=7,round=2,life=3,pawn="victim",actor=va,mesh=ma}
            end})
        end}
    end)
    T.check(not row and first==1 and later==0 and #emitted==0,"nested native-topology "..loss.." prevents subsequent optional fields and emission")
end
s=fresh();s:start();con=constraint();local finalized=0
row=s:capture("wear",function()local e=event(con);e.finish=function()finalized=finalized+1;return false end;return e end)
T.check(not row and finalized==1 and #emitted==0 and s.initializer_count==nil,"failed final binding neither emits nor caches initializer proof")
s=fresh();s:start();local response={fields={damage={available=true,value=8}}}
s:response(response);s:response(response);s:response(response)
T.check(s.responses==2 and s.events==0 and #emitted==2,"two scalar owner response slots cannot exhaust source capture budget")
response.fields.damage.value=9
T.check(emitted[1].response.fields.damage.value==8 and not emitted[1].authority,"owner response copies observations without authority")
con=constraint()
for i=1,16 do row=s:capture("wear",function()return event(con)end);if i==16 then T.check(row~=nil,"admitted event16 survives its own event cap")end end
resolves=0;row=s:capture("wear",function()resolves=resolves+1 end)
T.check(not row and resolves==0 and not s.armed,"event17 stops before resolver/native reads")
s=fresh();s:start();for _=1,3 do s:capture("wear",function()return nil end)end
resolves=0;s:capture("wear",function()resolves=resolves+1 end)
T.check(s.attempts==3 and resolves==0 and not s.armed,"three failed admissions close the whole process window")
s=fresh();s:start();s:stop("off")
T.check(not s:start(),"developer off cannot restart whole-run budget")
for _,hookid in ipairs({0,-3,2147483647,-2147483648})do
    s=fresh();s:start();local calls=0
    T.check(s:install(function()calls=calls+1;return hookid,hookid end,function()end)and calls==5,"equal signed int32 hook IDs prove registration")
    s:install(function()calls=calls+1 end,function()end)
    T.check(calls==5,"proven hook registration is not repeated")
end
s=fresh();s:start();local calls=0
s:install(function()calls=calls+1;return nil,nil end,function()end)
s:install(function()calls=calls+1;return 1,1 end,function()end)
T.check(calls==5,"successful ambiguous hook submission cannot duplicate callbacks on retry")
