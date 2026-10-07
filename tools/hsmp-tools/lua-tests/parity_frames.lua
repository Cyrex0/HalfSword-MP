-- Real read-only command paths using the Native17 blocked Loading tuple.
local clock,logs=60,{}
local state_dir=T.tmpdir("parity_frames_")
os.getenv=function(k)if k=="HSMP_STATE_DIR" then return state_dir elseif k=="HSMP_DEV" then return "1" end end
os.clock=function()return clock end
print=function(s)logs[#logs+1]=s end
local function object(name,address)
    return {IsValid=function()return true end,GetAddress=function()return address end,
        GetFName=function()return {ToString=function()return name end}end,
        GetClass=function()return {GetFName=function()return {ToString=function()return "Willie_BP_C" end}end}end}
end
FName=function(n)return n end
local read_action,reads=nil,0
local frame={Translation={X=1,Y=2,Z=3},Rotation={X=0,Y=0,Z=0,W=1},Scale3D={X=1,Y=1,Z=1}}
local function mesh(name,address)
    local m=object(name,address)
    m.GetSocketTransform=function()reads=reads+1;if read_action then local f=read_action;read_action=nil;f()end;return frame end
    m.GetCenterOfMass=function()return frame.Translation end
    m.GetBoneMass=function()return 2 end
    m.GetPhysicsAngularVelocityInDegrees=function()return {X=0,Y=0,Z=0}end
    m.GetCurrentJointAngles=function(_,_,s1,tw,s2)s1.Swing1Angle=0;tw.TwistAngle=0;s2.Swing2Angle=0 end
    return m
end
local owner=object("Willie_BP_C_2147481219",2974618905280)
local remote=object("Willie_BP_C_2147480987",2975372784992)
local foreign=object("TemporaryPossessedProxy",999)
owner.Mesh,remote.Mesh,foreign.Mesh=mesh("CharacterMesh0",110),mesh("CharacterMesh0",120),mesh("CharacterMesh0",130)
local owner_mesh,remote_mesh=owner.Mesh,remote.Mesh
local pc={Pawn=foreign,IsValid=function()return true end}
local world=object("Map_Arena_Yard",2974695297728)
world.GetFullName=function()return "World /Game/Maps/Arenas/Map_Arena_Yard.Map_Arena_Yard" end
local key="World /Game/Maps/Arenas/Map_Arena_Yard.Map_Arena_Yard@2974695297728#PlayerController_2147481818"
local settled,check=true,true
local guard={key=key,world=function()return world end,pc=function()return pc end,
    ai_pawn=function()return nil end,on_drop=function()end,check=function()return check end,settled=function()return settled end}
local view={state="loading",match_id=4081818838511089,round=0,pending_round=1,spawn_round=1,my_peer_id=1,
    spawns={[1]={peer=1,spawn_id=256},[2]={peer=2,spawn_id=257}}}
-- A prior Mode is present; pending diagnostics must never borrow its life.
local mode={match_id=4081818838511089,round=0,rows={[1]={life=7},[2]={life=9}}}
local status={pawn="Willie_BP_C_2147481219",match_id=view.match_id,round=1,life=1,spawn_id=256,verified=false}
local shown={peer=2,pawn="Willie_BP_C_2147480987",match_id=view.match_id,round=1,life=1,body_ts=58678,local_ms=60000}
local bus={spawn_status=status,playback={rows={shown}},puppets={rows={{peer=2,name=shown.pawn}}}}
HSMP_IPC={bus_table=function(k)return bus[k]end}
package.preload.UEHelpers=function()return {}end
package.preload.hsmp_wg=function()return {new=function()return guard end}end
package.preload.hsmp_ipc=function()return {init=function()end}end
package.preload.hsmp_session=function()return {new=function()return {}end,view=function()return view end,mode=function()return mode end}end
package.preload.body_joint_dictionary=function()return {{name="UserConstraint_12",parent="lowerarm_r",child="hand_r"}}end
FindAllOf=function()return {owner,remote,foreign}end
StaticFindObject=function(path)
    if path=="/Script/Engine.PhysicsObjectBlueprintLibrary" then
        return {IsValid=function()return true end,GetCDO=function()return {GetPhysicsObjectWorldTransform=function()return frame end}end}
    end
end
LoopAsync=function()end
RegisterHook=function()end
HSMP_PARITY_TEST={}
dofile(T.path("mods/dev/HSMPParity/Scripts/main.lua"))
local api=HSMP_PARITY_TEST
local function contains(s)for _,l in ipairs(logs)do if l:find(s,1,true)then return true end end;return false end
local function capture(peer)logs={};api.frames(tostring(peer).." hand_r");return contains("BODYFRAME_CONTEXT ") end
T.check(capture(0) and contains("pawn=Willie_BP_C_2147481219") and not contains("TemporaryPossessedProxy"),
    "Native17 Loading frames resolve the exact assigned owner despite temporary native possession")
T.check(contains("placement_verified=false") and contains("match=4081818838511089 round=1 life=1"),
    "pending diagnostic keeps full original tuple and explicitly reports unverified placement")
T.check(capture(2) and contains("pawn=Willie_BP_C_2147480987") and contains("display_time=58678"),
    "Native17 remote Loading frames retain the displayed proxy and physical label")
view.state="countdown"
status.verified=nil
T.check(capture(0) and contains("placement_verified=unknown"),"Countdown allows exact assigned preparation telemetry without inventing placement proof")
status.verified=true
T.check(capture(0) and contains("placement_verified=true"),"actual placement verification is copied separately")
view.state="loading"
local function refused(change,restore,message)
    local before=reads;change();local ok=capture(0);restore()
    T.check(not ok and reads==before,message)
end
refused(function()status.life=7 end,function()status.life=1 end,"pending capture refuses the old Mode's higher life before native reads")
refused(function()status.round=0 end,function()status.round=1 end,"pending capture refuses the old round before native reads")
refused(function()status.spawn_id=257 end,function()status.spawn_id=256 end,"own pending capture requires its exact current spawn order")
refused(function()view.pending_round=2 end,function()view.pending_round=1 end,"pending round must match the assigned spawn round")
refused(function()status.match_id=1 end,function()status.match_id=view.match_id end,"an old match cannot authorize a body snapshot")
refused(function()status.pawn="missing actor" end,function()status.pawn="Willie_BP_C_2147481219" end,"a missing assigned actor never falls back to the possessed pawn")
refused(function()settled=false end,function()settled=true end,"world settle remains mandatory before walking actors")
refused(function()check=false end,function()check=true end,"failed world guard prevents native reads")
local function changed(action,restore,message)
    read_action=action
    local ok=capture(2);restore()
    T.check(not ok and contains("original_context_changed"),message)
end
changed(function()shown.life=2 end,function()shown.life=1 end,"life changes during capture suppress all emitted frame/joint results")
changed(function()remote.Mesh=mesh("ReplacementMesh",121)end,function()remote.Mesh=remote_mesh end,
    "the same pawn with a replacement native mesh fails the final fresh mesh check")
changed(function()remote.Mesh=mesh("DifferentFNameAtReusedAddress",120)end,function()remote.Mesh=remote_mesh end,
    "a reused mesh address with a different native FName is not the captured mesh")
changed(function()guard.key="new generation"end,function()guard.key=key end,"world generation changes invalidate the capture")
local original_world=world
changed(function()world=object("Map_Arena_Yard",2974695297729);world.GetFullName=original_world.GetFullName end,
    function()world=original_world end,"a changed native world fails even when the guard key has not changed")
local original_remote=remote
changed(function()remote=object(shown.pawn,9999);remote.Mesh=remote_mesh end,function()remote=original_remote end,
    "freshly resolved actor address must remain exact through the capture")
view.state,view.round="live",1
mode.round=1;mode.rows[1].life,mode.rows[2].life=3,4
status.life,shown.life=3,4
T.check(capture(0) and capture(2),"Live captures use the exact current Mode's complete lives")
mode.round=0
T.check(not capture(0) and not capture(2),"outside pending phases a missing current Mode cannot default to life one")
mode.round=1;view.state="paused"
T.check(capture(2) and contains("round=1 life=4"),"Paused diagnostics retain the current round and life")
status.verified=false
T.check(not capture(0),"outside preparation the owner still requires its verified placement")
status.verified=true
-- Weaponstate shares the same verifier and is also refused after mesh churn.
owner.K2_GetComponentsByClass=function()return {}end
remote.K2_GetComponentsByClass=function()return {}end
view.state,view.round,mode.round="loading",0,0
status.life,shown.life,status.verified=1,1,false
logs={};api.weaponstate("0")
T.check(contains("WEAPONSTATE peer=0") and contains("placement_verified=false"),
    "weaponstate shares the exact read-only pending assignment verifier")
remote.K2_GetComponentsByClass=function()remote.Mesh=mesh("ReplacementMesh",121);return {}end
logs={};api.weaponstate("2");remote.Mesh=remote_mesh
T.check(not contains("WEAPONSTATE peer=2") and contains("original_context_changed"),
    "weaponstate also revalidates the fresh native mesh after its synchronous readback")
T.check(owner.Mesh==owner_mesh and pc.Pawn==foreign and status.verified==false,
    "diagnostics do not mutate placement, possession or the owner mesh")
