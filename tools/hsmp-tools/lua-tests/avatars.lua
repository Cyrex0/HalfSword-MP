-- HSMPAvatars runtime tests, offline under umg_mock.
--
--     hsmp-tools lua-test avatars
--
--   * the Willie_BP ReceiveTick post-hook fails at mod load (the BP is
--     not loaded in the menu) and is retried at 1 Hz until it registers; the
--     registration is logged.
--   * a stand-in HSMPCombat put to death for a server-declared death
--     (the `standin_dead` bus record) is never re-claimed; a stand-in that died locally
--     with no owner death is re-claimed only after a grace.
-- Every case runs in its own Lua state (T.isolated).

local mode, opts = ...
local AV = T.path("mods/HSMPAvatars/Scripts/main.lua")

if mode ~= "case" then
    T.isolated(T.script, "case", { kind = "hook" })
    T.isolated(T.script, "case", { kind = "hook_post" })
    T.isolated(T.script, "case", { kind = "parse" })
    T.isolated(T.script, "case", { kind = "corpse", standin_dead = true })
    T.isolated(T.script, "case", { kind = "corpse", standin_dead = false })
    T.isolated(T.script, "case", { kind = "corpse", vitals_dead = true })
    T.isolated(T.script, "case", { kind = "census" })
    T.isolated(T.script, "case", { kind = "frame" })
    T.isolated(T.script, "case", { kind = "levelchange" })
    T.isolated(T.script, "case", { kind = "weapon_destroyed" })
    T.isolated(T.script, "case", { kind = "bodies3" })
    T.isolated(T.script, "case", { kind = "spectate" })
    T.isolated(T.script, "case", { kind = "tune" })
    T.isolated(T.script, "case", { kind = "native_servo" })
    T.isolated(T.script, "case", { kind = "native_neutralise" })
    T.isolated(T.script, "case", { kind = "native_wservo" })
    T.isolated(T.script, "case", { kind = "wpn_status" })
    T.isolated(T.script, "case", { kind = "skeleton_ref" })
    T.isolated(T.script, "case", { kind = "body_scale" })
    T.isolated(T.script, "case", { kind = "fist_grip" })
    T.isolated(T.script, "case", { kind = "grip_reassert" })
    T.isolated(T.script, "case", { kind = "grip_linear" })
    T.isolated(T.script, "case", { kind = "grip_probe" })
    T.isolated(T.script, "case", { kind = "grip_probe", gate = "off" })
    T.isolated(T.script, "case", { kind = "grip_probe", gate = "no_dev" })
    T.isolated(T.script, "case", { kind = "hand_pipeline" })
    T.isolated(T.script, "case", { kind = "hand_pipeline", gate = "off" })
    T.isolated(T.script, "case", { kind = "hand_pipeline", gate = "no_dev" })
    T.isolated(T.script, "case", { kind = "limb_burst" })
    T.isolated(T.script, "case", { kind = "limb_burst", gate = "off" })
    T.isolated(T.script, "case", { kind = "limb_burst", gate = "no_dev" })
    T.isolated(T.script, "case", { kind = "limb_burst", change = "mesh" })
    T.isolated(T.script, "case", { kind = "limb_burst", change = "world" })
    T.isolated(T.script, "case", { kind = "limb_burst", change = "logging" })
    T.isolated(T.script, "case", { kind = "limb_burst", change = "admission_mesh" })
    T.isolated(T.script, "case", { kind = "limb_burst", change = "admission_world" })
    T.isolated(T.script, "case", { kind = "limb_burst", change = "mode_unavailable" })
    T.isolated(T.script, "case", { kind = "limb_burst", change = "pc_world" })
    T.isolated(T.script, "case", { kind = "joint_profile" })
    T.isolated(T.script, "case", { kind = "joint_profile", change = "fault_wait" })
    T.isolated(T.script, "case", { kind = "joint_profile", gate = "off" })
    T.isolated(T.script, "case", { kind = "joint_profile", gate = "no_dev" })
    T.isolated(T.script, "case", { kind = "joint_profile", change = "pc_world" })
    T.isolated(T.script, "case", { kind = "joint_profile", change = "lookup_pc_world" })
    T.isolated(T.script, "case", { kind = "joint_profile", change = "refusal" })
    T.isolated(T.script, "case", { kind = "joint_profile", change = "predicate" })
    T.isolated(T.script, "case", { kind = "joint_profile", change = "cached_reader" })
    T.isolated(T.script, "case", { kind = "pose_context" })
    T.isolated(T.script, "case", { kind = "clock_probe" })
    T.isolated(T.script, "case", { kind = "clock_epoch" })
    T.isolated(T.script, "case", { kind = "bodyheight" })
    T.isolated(T.script, "case", { kind = "ai_owner" })
    T.isolated(T.script, "case", { kind = "weaponstate" })
    T.isolated(T.script, "case", { kind = "weapon_sim_refresh" })
    T.isolated(T.script, "case", { kind = "spawn_settle" })
    T.isolated(T.script, "case", { kind = "injury" })
    T.isolated(T.script, "case", { kind = "injury", severed_physics = "0" })
    T.isolated(T.script, "case", { kind = "injury_mesh_swap" })
    return
end

local M = require("umg_mock")
local sd = T.tmpdir("hsmp_av_sd_")
local la = T.tmpdir("hsmp_av_la_")

-- A relayed root of peer `id` as the sidecar publishes it: the peer_root record slot (root +
-- peer id; rotation a quaternion, here a yaw of 45 degrees). Replaces me_remote<id>.json.
local function peer_root(id, tick, x, y, z)
    local h = math.rad(45) / 2
    _G.HSMPNative.sc_put("peer_root", { peer_id = id, root = { tick = tick, ts = 1000 + tick, pos = { x, y, z },
        rot = { 0, 0, math.sin(h), math.cos(h) } } }, id)
end

local function boot(register_ok)
    M.install({ state_dir = sd, env = { LOCALAPPDATA = la, HSMP_INST = "7", HSMP_SEVERED_PHYSICS = opts.severed_physics,
        HSMP_DEV = (opts.kind == "ai_owner" or opts.kind == "weaponstate" or (opts.kind=="grip_probe" or opts.kind=="hand_pipeline" or opts.kind=="limb_burst"or opts.kind=="joint_profile") and opts.gate~="no_dev") and "1" or "0",
        HSMP_GRIP_PROBE = opts.kind=="grip_probe" and opts.gate~="off" and "1" or "0",
        HSMP_HAND_PIPELINE_PROBE = opts.kind=="hand_pipeline" and opts.gate~="off" and "1" or "0",
        HSMP_LIMB_BURST_PROBE = opts.kind=="limb_burst" and opts.gate~="off" and "1" or "0",
        HSMP_JOINT_PROFILE_PROBE = opts.kind=="joint_profile" and opts.gate~="off" and "1"or "0" }, strict = true })
    package.path = T.path("mods/shared") .. "/?.lua;" .. package.path
    local arena = "World /Game/Maps/Arenas/Map_Arena_Pit/Map_Arena_Pit.Map_Arena_Pit"
    M.Methods.GetFullName = function(self)
        if rawget(self, "__cls") == "World" then return arena end
        return "Obj " .. tostring(rawget(self, "__name"))
    end
    M.Methods.GetAddress = function(self) return rawget(self, "__addr") or 4096 end
    M.Methods.GetWorld = function() return M.world end
    M.Methods.K2_GetActorLocation = function() return { X = 500, Y = 0, Z = 100 } end
    M.Methods.GetWorldDeltaSeconds = function() return 1 / 60 end
    M.Methods.GetRealTimeSeconds = function() return M.now / 1000 end
    M.hooks, M.hook_calls = {}, 0
    M.register_ok = register_ok
    _G.RegisterHook = function(path, pre, post)
        M.hook_calls = M.hook_calls + 1
        if path:find("Willie_BP_C:ReceiveTick", 1, true) and not M.register_ok then
            error("No UFunction with the specified name was found: " .. path)
        end
        M.hooks[path] = { callback = pre, ignored_post = post }
        return 1, 2
    end
    M.bp_tick=function(actor,native)
        if native then native(actor)end
        local h=M.hooks["/Game/Character/Blueprints/Willie_BP.Willie_BP_C:ReceiveTick"]
        if h and h.callback then
            h.callback({get=function()M.tick_unwraps=(M.tick_unwraps or 0)+1;return actor end})
        end -- script functions run argument #2 POST; argument #3 is ignored
    end
    _G.HSMP_AVATARS_TEST = {}
    dofile(AV)
    return HSMP_AVATARS_TEST.api
end

-- The sidecar (typed): its link + header heartbeat and peer directory.
local function sidecar(peers)
    local N = _G.HSMPNative
    N.sc_put("link", { status = 1, state = 1, my_peer_id = 1 })
    N._st.hb_age = 0.05
    local e = {}
    for _, p in ipairs(peers) do e[#e + 1] = { id = p[1], nick = p[2] } end
    N.sc_peer_dir(e)
end

if opts.kind == "ai_owner" then
    local api = boot(true)
    local me = M.new_obj("Willie_BP_C", "AI_OWNER"); rawset(me, "__addr", 7701)
    local foreign = M.new_obj("Willie_BP_C", "TEMPORARY"); rawset(foreign, "__addr", 7702)
    local ai = M.new_obj("AI_BP_C", "AI_CONTROLLER")
    me.__props.Health, me.__props.Player, me.__props.Controller = 100, false, ai
    foreign.__props.Controller, M.pc.__props.Pawn = M.pc, foreign
    FindAllOf = function(c) if c == "Willie_BP_C" then return {me, foreign} end end
    sidecar({{1,"Own"},{2,"Peer"}})
    HSMPNative.sc_put("session", {seq=1,match_id=93,round=1,phase=3,rows={{peer_id=1,spawn_id=256}}})
    HSMPNative.sc_put("mode", {seq=1,match_id=93,round=1,rows={{peer_id=1,life=1}}})
    HSMP_IPC.bus_put("spawn_status", {verified=true,pawn="AI_OWNER",match_id=93,round=1,life=1,spawn_id=256})
    local releases, possessions = 0, 0
    M.Methods.UnPossess = function(self)
        releases = releases + 1
        if self.__props.Pawn then self.__props.Pawn.__props.Controller = nil end
        self.__props.Pawn = nil
    end
    M.Methods.Possess = function(self,pawn) possessions=possessions+1;self.__props.Pawn=pawn;pawn.__props.Controller=self end
    T.check(api.local_pawn() == me, "verified AI fighter takes priority over a temporary native PC pawn")
    api.PX.keep_pawn = {pawn=me,gen=api.generation(),until_tick=9999}
    api.PX.keep_possession()
    T.check(releases==1 and possessions==0 and M.pc.__props.Pawn==nil and me.__props.Controller==ai,
        "delayed stand-in swap releases only the foreign PC possession and preserves the fighter AI")
    M.pc.__props.Pawn = foreign; foreign.__props.Controller = M.pc
    me.__props.Player, me.__props.Controller = true, nil
    api.PX.restore_spawn_possession(me,M.pc)
    T.check(possessions==1 and M.pc.__props.Pawn==me,
        "normal human stand-in spawning restores the original possessed fighter")
end

if opts.kind == "injury" then
    local api=boot(true)
    sidecar({{1,"Own"},{2,"Peer"}})
    HSMPNative.sc_put("session",{seq=1,match_id=307,round=1,phase=3})
    HSMPNative.sc_put("mode",{seq=1,match_id=307,round=1,rows={{peer_id=2,life=1}}})
    api.on_tick()
    local actor=M.new_obj("Willie_BP_C","SEVER_PROXY");rawset(actor,"__addr",9301)
    local mesh=M.new_obj("SkeletalMeshComponent","SEVER_MESH");rawset(mesh,"__addr",9302)
    actor.__props.Mesh=mesh
    local P=api.PURE
    local disabled,calls={},{}
    local fail_disable,fail_restore=false,false
    M.Methods.SetAllBodiesBelowPhysicsDisabled=function(self,bone,value,include)
        T.check(self==mesh and include==true,"production native subtree call targets exact mesh and includes root")
        if (value and fail_disable)or(not value and fail_restore)then error("native exclusion unavailable")end
        bone=type(bone)=="string"and bone or bone:ToString();calls[#calls+1]={bone,value}
        for i,name in ipairs(P.V2_SLOTS)do
            local n=i
            while n and n>0 do
                if P.V2_SLOTS[n]==bone then disabled[name]=value;break end
                local parent=P.V2_PARENT[n];if parent==n then break end;n=parent
            end
        end
    end
    M.Methods.IsSimulatingPhysics=function(_,bone)
        bone=type(bone)=="string"and bone or bone:ToString();return disabled[bone]~=true
    end
    M.Methods.SetEnableGravity=function()end
    M.Methods.SetCollisionEnabled=function()disabled={}end -- a real physics rebuild defeats cached success
    local p={actor=actor,addr=9301,gen=api.generation(),peer=2,driving=false,
        shown={has_context=true,match_id=307,round=1,life=1,pawn="SEVER_PROXY"},
        body={mesh=mesh,mesh_addr=9302,mesh_fname="SEVER_MESH",field="Mesh",handles={},sims={mesh},motors={},sv={wc={}}}}
    local function vitals(seq,life,mask)
        HSMPNative.sc_put("peer_vitals",{seq=seq,match_id=307,round=1,life=life,dism=mask},2)
    end
    local function aims()
        local t={};for i=1,P.V2_NB do t[i]={i,0,0,0,0,0,1}end;return t
    end
    vitals(1,1,1<<11)
    local aim=aims();api.PX.injury_targets(2,p,aim,aim)
    if opts.severed_physics=="0"then
        T.check(#calls==0 and aim[12]and aim[13],"explicit diagnostic opt-out retains prior baseline without exclusion")
        return
    end
    T.check(disabled.lowerarm_l and disabled.hand_l and not disabled.upperarm_l,
        "default production consumes exact owner-confirmed missing subtree")
    T.check(aim[12]==nil and aim[13]==nil and aim[11]~=nil,"native and Lua servo inputs omit only confirmed severed subtree")
    vitals(2,2,1<<16);api.PX.injury_targets(2,p,nil,nil)
    T.check(not disabled.hand_r,"wrong-life severing cannot disable the current body")
    vitals(3,1,0);api.PX.injury_targets(2,p,nil,nil)
    T.check(disabled.hand_l and p.body.injury_journal.mask==1<<11,"zero wire mask cannot restore native severed parts")
    local read=HSMP_IPC.peer_rec;HSMP_IPC.peer_rec=function()return nil end
    api.PX.injury_targets(2,p,nil,nil);HSMP_IPC.peer_rec=read
    T.check(disabled.hand_l,"missing or torn Vitals retains last confirmed same-life exclusion")
    disabled={};api.PX.injury_targets(2,p,nil,nil)
    T.check(disabled.lowerarm_l and disabled.hand_l,"unchanged confirmed mask reasserts after same-component physics rebuild")
    fail_disable=true;disabled={};aim=aims();api.PX.injury_targets(2,p,aim,aim)
    T.check(aim[12]==nil and aim[13]==nil and p.body.injury_error=="disable failed lowerarm_l",
        "failed native exclusion reports failure and still omits owner-confirmed absent servo bodies")
    fail_disable=false;api.PX.injury_targets(2,p,nil,nil)
    p.driving=true;p.body.gravity=false;p.body.ghost_until=1;p.body.coll0=3
    local before=#calls;api.release_standin(p)
    local restored=false;for i=before+1,#calls do if calls[i][2]==false then restored=true end end
    T.check(not restored and disabled.hand_l and p.body.injury_journal.mask==1<<11,
        "temporary source/range release keeps exclusion even when collision restoration rebuilds physics")
    p.owner_dead=true;p.driving=true;p.last={v2=true,has_context=true,match_id=307,round=1,life=1}
    api.drive_frame(2,p,100)
    T.check(not p.driving and disabled.hand_l,"server-declared owner death releases servo without restoring missing collider")
    p.owner_dead=false
    HSMPNative.sc_put("mode",{seq=2,match_id=307,round=1,rows={{peer_id=2,life=2}}})
    p.shown.life=2;vitals(1,2,0);api.PX.injury_targets(2,p,nil,nil)
    T.check(disabled.hand_l and p.body.injury_wanted.lowerarm_l,
        "new full life on reused native body cannot restore exclusion from an unproven zero topology")
    fail_restore=true
    T.check(api.release_standin(p,true)==false and p.body.injury_retiring and p.body.injury_disabled.lowerarm_l,
        "true lease handoff retains failed restoration ownership for retry")
    fail_restore=false
    T.check(api.PX.injury_tick(2,p)==true and not disabled.hand_l and not next(p.body.injury_disabled)
        and p.body.injury_journal==nil,"bookkeeping retries true current-body lease handoff and restores only owned roots")
    vitals(2,2,1<<11);api.PX.injury_targets(2,p,nil,nil)
    local field=actor.__props.Mesh;actor.__props.Mesh=nil;before=#calls
    T.check(api.release_standin(p,true)==false and #calls==before and p.body.injury_disabled.lowerarm_l,
        "temporarily unavailable current mesh retains restoration ownership without unsafe writes")
    actor.__props.Mesh=field
    T.check(api.release_standin(p,true)==true and not disabled.hand_l,
        "exact current mesh returning permits the retained lease restoration")
    vitals(3,2,1<<11);api.PX.injury_targets(2,p,nil,nil)
    local newer=M.new_obj("SkeletalMeshComponent","REPLACEMENT_MESH");rawset(newer,"__addr",9303)
    actor.__props.Mesh=newer;before=#calls
    api.PX.injury_targets(2,p,nil,nil);api.release_standin(p,true)
    T.check(#calls==before,"component replacement discards old ownership without touching old or foreign mesh")
    actor.__props.Mesh=mesh;p.body.mesh=mesh;vitals(4,2,1<<11);api.PX.injury_targets(2,p,nil,nil)
    local getworld=M.Methods.GetWorld;M.Methods.GetWorld=function(self)if self==actor then return nil end;return M.world end
    before=#calls
    T.check(api.release_standin(p,true)==false and #calls==before and p.body.injury_disabled.lowerarm_l,
        "unavailable actor world cannot borrow global current world to authorize native restore")
    M.Methods.GetWorld=getworld
    T.check(api.release_standin(p,true)==true,"fresh exact actor world returning permits restore retry")
    p.body.mesh=newer;p.body.injury_disabled={lowerarm_l=true};p.body.injury_wanted={lowerarm_l=true}
    p.gen=p.gen+1;before=#calls
    api.release_standin(p,true)
    T.check(#calls==before,"world-generation mismatch cannot restore through cached mesh")
    api.set_puppet(2,p);api.drop_caches("injury teardown")
    T.check(#calls==before and next(api.puppets())==nil,"world teardown forgets injury state without native object access")
end

if opts.kind=="injury_mesh_swap"then
    T.write(sd.."/.settings.json",'{"avatars":true,"native_servo":false}\n')
    local api=boot(true)
    sidecar({{1,"Own"},{2,"Peer"}})
    HSMPNative.sc_put("session",{seq=1,match_id=308,round=1,phase=3})
    HSMPNative.sc_put("mode",{seq=1,match_id=308,round=1,rows={{peer_id=2,life=1}}})
    api.on_tick()
    -- The legacy file mock omits the native PeerPlay context fields. Supply
    -- this fixture's original immutable source tuple at the native boundary.
    local peer_play=HSMPNative.peer_play
    HSMPNative.peer_play=function(slot,out,last)
        local seq=peer_play(slot,out,last)
        if seq then out.has_context=true;out.match_id=308;out.round=1;out.life=1;out.rate=1 end
        return seq
    end
    local actor=M.new_obj("Willie_BP_C","SWAP_PROXY");rawset(actor,"__addr",9401)
    local old=M.new_obj("SkeletalMeshComponent","ALTERNATE_MESH");rawset(old,"__addr",9402)
    local visible=M.new_obj("SkeletalMeshComponent","VISIBLE_MESH");rawset(visible,"__addr",9403)
    actor.__props.SK_Skeleton,actor.__props.Mesh=old,visible
    local P=api.PURE
    local p={actor=actor,addr=9401,gen=api.generation(),peer=2,in_range=true,driving=true,claimed_at=0,
        shown={has_context=true,match_id=308,round=1,life=1,pawn="SWAP_PROXY"},
        settle_state={settle_ready=true,settle_stable_ms=200,settle_count=6},
        body={mesh=old,mesh_addr=9402,mesh_fname="ALTERNATE_MESH",field="SK_Skeleton",ctl="handles",handles={},sims={old},motors={},sv={wc={}},snaps=0,
            injury_disabled={lowerarm_l=true},injury_wanted={lowerarm_l=true}}}
    local _,key=api.PX.injury_mesh(p);p.body.injury_key=key
    api.PX.SETTLE.copy(p.shown,p.settle_state)
    local failed,restores=true,0
    M.Methods.SetAllBodiesBelowPhysicsDisabled=function(self,_,value,include)
        T.check(self==old and value==false and include==true,"pending mesh swap restores only exact alternate body's owned subtree")
        if failed then error("restore temporarily refused")end
        restores=restores+1
    end
    local names={};for i,bone in ipairs(P.V2_SLOTS)do names[bone]=i end
    local function current(bone)
        local t=p.aim and p.aim.slots and p.aim.slots[names[tostring(bone)]]
        return t or {0,0,0,0,0,0,1}
    end
    M.Methods.GetBoneIndex=function(_,bone)return (names[tostring(bone)]or 1)-1 end
    M.Methods.GetSocketTransform=function(_,bone)
        local t=current(bone);return {Translation={X=t[1],Y=t[2],Z=t[3]},Rotation={X=t[4],Y=t[5],Z=t[6],W=t[7]}}
    end
    M.Methods.GetCenterOfMass=function(_,bone)local t=current(bone);return {X=t[1],Y=t[2],Z=t[3]}end
    M.Methods.GetSocketLocation=M.Methods.GetCenterOfMass
    M.Methods.K2_GetComponentLocation=function()return {X=0,Y=0,Z=0}end
    M.Methods.K2_SetWorldLocation=function()end
    M.Methods.SetAllPhysicsLinearVelocity=function()end
    M.Methods.SetSimulatePhysics=function()end
    M.Methods.SetEnableGravity=function()end
    M.Methods.GetCollisionResponseToChannel=function()return 2 end
    M.Methods.SetCollisionResponseToChannel=function()end
    M.Methods.GetPhysicsLinearVelocity=function()return {X=0,Y=0,Z=0}end
    local driven={}
    M.Methods.SetPhysicsLinearVelocity=function(self)driven[#driven+1]=self end
    M.Methods.SetPhysicsAngularVelocityInDegrees=function()end
    local seq=0
    local function frame(now)
        M.now=now;seq=seq+1
        local b={};for _=1,23 do for _,v in ipairs({0,0,0,0,0,0,1,0,0,0,0,0,0})do b[#b+1]=v end end
        HSMPNative.sc_peer_play(2,{peer_id=2,seq=seq,v=2,pt=now,rate=1,mode="interp",age=0,cut=1,
            has_context=true,match_id=308,round=1,life=1,m=(1<<23)-1,B=b})
        api.drive_frame(2,p,now)
    end
    frame(2000)
    T.check(p.body.mesh==old and p.body.field=="SK_Skeleton"and p.body.ctl~="servo"and not p.driving,
        "production failed restoration leaves visible-mesh switch uncommitted and retryable")
    T.check(p.body.injury_retiring and not p.shown.settle_ready and not p.settle_state.settle_ready,
        "pending native mesh handoff invalidates previous physical qualification")
    failed=false
    T.check(api.PX.injury_tick(2,p)==true and restores==1,"normal bookkeeping restores the pending alternate component")
    frame(2016)
    T.check(p.body.mesh==visible and p.body.field=="Mesh"and p.body.ctl=="servo"and p.driving and p.body.repose,
        "next production frame commits visible Mesh and starts a fresh physical repose after recovery")
    local current_mesh=api.PX.injury_mesh(p)
    T.check(current_mesh==visible and p.body.mesh_addr==9403 and p.body.mesh_fname=="VISIBLE_MESH",
        "legitimate alternate-to-visible handoff refreshes the immutable native component identity")
    T.check(not(p.shown and p.shown.settle_ready),"successful component replacement retains unqualified state until fresh measured convergence")
    for now=2032,2304,16 do frame(now)end
    local only_visible=#driven>0;for _,mesh in ipairs(driven)do if mesh~=visible then only_visible=false end end
    T.check(only_visible and p.shown.settle_ready and p.shown.settle_count==6 and p.shown.settle_stable_ms>=150,
        "recovered production path drives only visible Mesh and qualifies its new six-limb physical samples",T.repr(p.shown))
end

if opts.kind == "weapon_sim_refresh" then
    local api = boot(true)
    sidecar({{1,"Own"},{2,"Peer"}})
    HSMPNative.sc_put("session",{seq=1,match_id=107,round=1,phase=3})
    HSMPNative.sc_put("mode",{seq=1,match_id=107,round=1,rows={{peer_id=2,life=1}}})
    api.on_tick() -- establish the same current-world guard used by production
    local P,PX = api.PURE,api.PX
    local actor=M.new_obj("Willie_BP_C","MutablePhysicsPuppet");rawset(actor,"__addr",8701)
    local weapon=M.new_obj("ModularWeaponBP_Polearm_C","MutablePolearm");rawset(weapon,"__addr",8702)
    local root=M.new_obj("StaticMeshComponent","MutableRoot");rawset(root,"__addr",8703)
    local mesh=M.new_obj("SkeletalMeshComponent","CharacterMesh0")
    actor.__props["Weapon R"],weapon.__props.RootComponent=weapon,root
    local simulated,unreadable,malformed=false,false,false
    local sim_reads,com_reads,velocity_sets,policy_sets,authored_sim=0,0,0,0,0
    M.Methods.IsSimulatingPhysics=function(self)
        if self~=root then return true end
        sim_reads=sim_reads+1
        if unreadable then error("native physics temporarily unavailable") end
        if malformed then return "true" end
        return simulated
    end
    M.Methods.GetTransform=function()return {Translation={X=0,Y=0,Z=0},Rotation={X=0,Y=0,Z=0,W=1}}end
    M.Methods.GetCenterOfMass=function()com_reads=com_reads+1;return {X=1,Y=0,Z=0}end
    M.Methods.GetSocketTransform=M.Methods.GetTransform
    M.Methods.GetPhysicsLinearVelocity=function()return {X=0,Y=0,Z=0}end
    M.Methods.SetPhysicsLinearVelocity=function(self)if self==root then velocity_sets=velocity_sets+1 end end
    M.Methods.SetPhysicsAngularVelocityInDegrees=function()end
    M.Methods.SetEnableGravity=function(self,on)if self==root then policy_sets=policy_sets+1;rawset(root,"grav",on)end end
    local channels={}
    M.Methods.SetCollisionResponseToChannel=function(self,channel,response)if self==root then channels[channel]=response end end
    M.Methods.SetSimulatePhysics=function()authored_sim=authored_sim+1;error("refresh must never author simulation")end
    local fn=function(n)return {ToString=function()return n end}end
    local grip={IsValid=function()return true end,GetAddress=function()return 8710 end,GetFName=function()return fn("MutableGrip")end}
    local pi={AngularDrive={SlerpDrive={Stiffness=10,Damping=2,MaxForce=100}},
        LinearLimit={XMotion=2,YMotion=2,ZMotion=2,Limit=31},
        ConeLimit={Swing1Motion=2,Swing1LimitDegrees=10,Swing2Motion=2,Swing2LimitDegrees=20},
        TwistLimit={TwistMotion=2,TwistLimitDegrees=30}}
    grip.ConstraintInstance={ConstraintBone2=fn("hand_r"),ProfileInstance=pi}
    for _,axis in ipairs({"X","Y","Z"})do grip["SetLinear"..axis.."Limit"]=function(_,mode)pi.LinearLimit[axis.."Motion"]=mode end end
    grip.SetAngularSwing1Limit=function(_,mode)pi.ConeLimit.Swing1Motion=mode end
    grip.SetAngularSwing2Limit=function(_,mode)pi.ConeLimit.Swing2Motion=mode end
    grip.SetAngularTwistLimit=function(_,mode)pi.TwistLimit.TwistMotion=mode end
    grip.SetAngularDriveParams=function()end
    M.Methods.K2_GetComponentsByClass=function()return {grip}end
    local sv={n=6,com={},cmd={},wc={},err={n=0,e=0,emax=0,a=0,amax=0,hmax=0,capped=0}}
    local slots={}
    for i in pairs(PX.SETTLE.bones)do sv.com[i]={0,0,0};slots[i]={0,0,0,0,0,0,1,0,0,0,0,0,0}end
    slots[P.V2_WPN_R]={10,0,0,0,0,0,1,0,0,0,0,0,0}
    local body={mesh=mesh,sv=sv,field="Mesh",gravity=false,ctl="servo"}
    local cur={v2=true,has_context=true,match_id=107,round=1,life=1,seq=1,pt=1000,read_at=1000,
        cut=0,age=0,delay=0,rate=1,st=16,mode="interp",slots=slots,weapons={},nbones=6}
    local p={actor=actor,body=body,gen=api.generation(),peer=2,driving=true,last=cur}
    local function drive(now)
        M.now=now;cur.seq=cur.seq+1;cur.pt,cur.read_at=now,now
        api.drive_v2(2,p,body,cur,true,now,false,false)
    end
    drive(1000)
    local c=sv.wc["Weapon R"];local original_com=c and c.com
    local grips=PX.grip_constraints(p,1000)
    T.check(c and c.sim==false and p.wservo==nil and velocity_sets==0 and not grips[1].freed,
        "production driver initially respects a native kinematic root with a locked grip")
    simulated=true;drive(1016);PX.grips_off(p,true)
    T.check(sv.wc["Weapon R"]==c and c.root==root and c.sim==true and c.com==original_com and com_reads==1,
        "same actor/root cache refreshes false to true without rebuilding COM")
    T.check(velocity_sets==1 and p.wservo["Weapon R"]==1016 and p.wservo_actor["Weapon R"]==8702,
        "fresh simulation resumes the actual production weapon servo in that frame")
    T.check(root.grav==false and policy_sets==1 and channels[0]==0 and channels[1]==0,
        "late simulation activation receives the existing gravity and world-response policy")
    T.check(grips[1].freed and pi.LinearLimit.ZMotion==0,"fresh servo evidence qualifies the existing free-grip policy")
    drive(1032)
    T.check(policy_sets==1 and com_reads==1 and velocity_sets==2,"stable true state keeps immutable cache and does not repeat setup writes")
    simulated=false;drive(1048);PX.grips_off(p,true)
    T.check(c.sim==false and p.wservo["Weapon R"]==nil and p.wservo_actor["Weapon R"]==nil and velocity_sets==2,
        "true to false stops physics servo and removes both servo timestamps immediately")
    T.check(not grips[1].freed and pi.LinearLimit.ZMotion==2 and not p.shown.settle_ready,
        "native kinematic transition clears qualification and restores the existing grip limits")
    simulated=true;drive(1064);PX.grips_off(p,true)
    T.check(c.sim==true and velocity_sets==3 and grips[1].freed and policy_sets==2,
        "later native simulation activation resumes servo and its existing policy")
    unreadable=true;drive(1080);PX.grips_off(p,true)
    T.check(c.sim==nil and c.sim_unavailable and p.wservo["Weapon R"]==nil and velocity_sets==3 and not grips[1].freed,
        "unreadable fresh state fails closed instead of retaining true or coercing false")
    T.check(not p.shown.settle_ready and T.contains(table.concat(p.wpn_states," "),"physics-unavailable"),
        "unreadable state removes physical qualification and is reported separately from kinematic")
    unreadable=false;malformed=true;drive(1096)
    T.check(c.sim==nil and velocity_sets==3,"a non-boolean native read is also unavailable")
    malformed=false;drive(1112)
    T.check(c.sim==true and not c.sim_unavailable and velocity_sets==4 and c.com==original_com,
        "unavailable state is retried on the next reuse and recovers without COM recomputation")
    local reads=sim_reads
    p.gen=999
    T.check(api.servo_weapon_parts(p,body,"Weapon R")==nil and sim_reads==reads,"stale world rejects reuse before reading native physics")
    p.gen=api.generation();cur.life=2
    T.check(api.servo_weapon_parts(p,body,"Weapon R")==nil and sim_reads==reads,"stale owner life rejects reuse before reading native physics")
    cur.life=1
    local old=root;root=M.new_obj("StaticMeshComponent","RebuiltRoot");rawset(root,"__addr",8704)
    weapon.__props.RootComponent=root;rawset(old,"__dead",true)
    local fresh=api.servo_weapon_parts(p,body,"Weapon R")
    T.check(fresh and fresh~=c and fresh.root_addr==8704 and #M.dead_touch==0,
        "changed root rebuilds from fresh identity and never reads the old freed root",T.repr(M.dead_touch))
    T.check(authored_sim==0,"the refresh never enables or disables native simulation")
end

if opts.kind=="weaponstate" then
    local api=boot(true)
    sidecar({{1,"Own"},{2,"Peer"}})
    HSMPNative.sc_put("session",{seq=1,match_id=94,round=1,phase=3})
    HSMPNative.sc_put("mode",{seq=1,match_id=94,round=1,rows={{peer_id=2,life=1}}})
    local actor=M.new_obj("Willie_BP_C","Puppet");rawset(actor,"__addr",8901)
    local weapon=M.new_obj("ModularWeaponBP_Polearm_Mid_Tier_C","Polearm");rawset(weapon,"__addr",8902)
    local root=M.new_obj("StaticMeshComponent","WeaponRoot");rawset(root,"__addr",8903)
    local base=M.new_obj("StaticMeshComponent","BaseMesh");rawset(base,"__addr",8904)
    actor.__props["Weapon R"],weapon.__props.RootComponent,weapon.__props.BaseMesh=weapon,root,base
    M.Methods.IsSimulatingPhysics=function()return true end
    M.Methods.GetTransform=function()return {Translation={X=10,Y=20,Z=30},Rotation={X=0,Y=0,Z=math.sqrt(.5),W=math.sqrt(.5)},Scale3D={X=1.2,Y=1.3,Z=1.4}}end
    M.Methods.GetCenterOfMass=function()return {X=10,Y=35,Z=30}end
    M.Methods.GetMass=function()return 4.25 end
    M.Methods.K2_GetComponentScale=function()return {X=1.2,Y=1.3,Z=1.4}end
    local c={addr=8902,root_addr=8903,fname="Polearm",sim=false,com={5,0,0},nocoll=true,empty_since=91}
    local before=T.repr(c)
    api.set_puppet(2,{actor=actor,gen=api.generation(),shown={has_context=true,pawn="Puppet",match_id=94,round=1,life=1},body={sv={wc={["Weapon R"]=c}}}})
    api.PX.weaponstate("2")
    local log=M.logtext()
    T.check(T.contains(log,"cached_sim=false actual_root_sim=true actual_base_sim=true"),"dev diagnostic compares cached simulation with two fresh native readbacks",log)
    T.check(c.sim==false and root:IsSimulatingPhysics("None")==true,"dev simulation mismatch snapshot mutates neither cache nor native physics")
    local cx,cy,cz=log:match("actual_com=%(([^,]+),([^,]+),([^%)]+)%)")
    T.check(T.contains(log,"cached_com=(5.000000,0.000000,0.000000)") and tonumber(cx)==15 and tonumber(cy)==0 and tonumber(cz)==0
        and T.contains(log,"com_delta=10.000000"),
        "read-only COM probe measures the fresh native center in the same rotated servo frame",log)
    T.check(T.contains(log,"root_mass=4.25 actor_scale=(1.200000,1.300000,1.400000) root_scale=(1.200000,1.300000,1.400000)"),
        "read-only COM probe captures current root mass and transform scales",log)
    T.check(T.repr(c)==before,"COM diagnostic preserves cached COM and unmatched-weapon collision bookkeeping")
end

if opts.kind=="spawn_settle" then
    T.write(sd.."/.settings.json",'{"avatars":true,"native_servo":false}\n')
    local api=boot(true)
    local S=api.PX.SETTLE
    local world="4096@World Arena"
    local shown={has_context=true,match_id=100,round=2,life=1,pawn="Puppet",label=100,at=1000}
    local cur={has_context=true,match_id=100,round=2,life=1,cut=3,mode="interp",age=0}
    local previous={has_context=true,match_id=100,round=2,life=1,pawn="Puppet",world=world,cut=3,label=100,at=984,seq=31}
    local actual={0,0,0,0,0,0,1}
    local s
    local function sample(now,rotation,missing)
        s=S.begin(s,world,shown,previous,cur,now)
        for i in pairs(S.bones) do
            if i~=missing then
                local c=(i==13 and rotation) and {0,0,0,math.sin(math.rad(rotation)/2),0,0,math.cos(math.rad(rotation)/2)} or actual
                S.measure(s,i,c,actual)
            end
        end
        S.finish(s,now)
        previous.at,previous.label,previous.seq=now,previous.label+16,previous.seq+1
    end
    sample(1000,174)
    T.check(not s.settle_ready and s.settle_rot_deg>173 and s.settle_reason=="hand_l rotation",
        "actual 174 degree hand twist refuses spawn alignment without joint getters")
    for now=1016,1160,16 do sample(now) end
    T.check(not s.settle_ready and s.settle_stable_ms==144,"a near-pose sample does not bypass continuous physical stabilization")
    sample(1176)
    T.check(s.settle_ready and s.settle_count==6 and s.settle_stable_ms==160,"all six measured arms and hands converge for at least 150 ms")
    S.copy(shown,s)
    local row=api.PURE.playback_row(2,shown,1000,true)
    T.check(row and row.settle_ready and row.settle_world==world and row.settle_source_seq==s.settle_source_seq
        and row.settle_source_ts==s.settle_source_ts,"playback retains the actual integrated aim marker and physical proof")
    sample(1192,nil,13)
    T.check(not s.settle_ready and s.settle_count==5 and s.settle_reason=="missing hand_l","an unavailable current hand invalidates settled proof")
    cur.mode="stale";sample(1208)
    T.check(not s.settle_ready and s.settle_reason=="source held/stale","freshly evaluated stale source cannot build physical stability")
    cur.mode="interp";previous.life=2;sample(1224)
    T.check(not s.settle_ready and s.settle_reason=="integrated aim context unavailable","old displayed life cannot relabel another integrated native life")
    previous.life=1;previous.world="other";sample(1240)
    T.check(not s.settle_ready and s.settle_reason=="integrated aim context unavailable","another world's aim cannot authorize the current body")
    previous.world=world;sample(1256)
    previous.label=s.last_label;sample(1272)
    T.check(not s.settle_ready and s.settle_stable_ms==0,"an unchanged source label never accrues continuous physical stability")
    local p={shown=shown,settle_state=s}
    api.PX.settle_reset(p,"repose")
    T.check(shown.settle_ready==false and shown.settle_reason=="repose","repose clears the published alignment proof immediately")

    -- The requested pose is close, but the previously integrated aim is far.
    local actor=M.new_obj("Willie_BP_C","Puppet")
    local mesh=M.new_obj("SkeletalMeshComponent","CharacterMesh0")
    local calls,twisted=0,false
    M.Methods.GetSocketTransform=function(_,bone)
        local q=(twisted and tostring(bone)=="hand_l") and math.rad(174)/2 or 0
        return {Translation={X=0,Y=0,Z=0},Rotation={X=math.sin(q),Y=0,Z=0,W=math.cos(q)}}
    end
    M.Methods.SetPhysicsLinearVelocity=function()calls=calls+1 end
    M.Methods.SetPhysicsAngularVelocityInDegrees=function()end
    M.Methods.GetPhysicsLinearVelocity=function()return {X=0,Y=0,Z=0}end
    local sv={n=6,com={},cmd={},wc={},err={n=0,e=0,emax=0,a=0,amax=0,hmax=0,capped=0}}
    local slots,old={},{}
    for i in pairs(S.bones) do sv.com[i]={0,0,0};slots[i]={0,0,0,0,0,0,1,0,0,0,0,0,0};old[i]={100,0,0,0,0,0,1} end
    local body={mesh=mesh,sv=sv,field="Mesh"}
    p={actor=actor,driving=true,aim={has_context=true,match_id=100,round=2,life=1,pawn="Puppet",world=world,cut=3,seq=41,label=1900,at=1984,slots=old}}
    api.PX.settle_world=world
    cur.seq,cur.pt,cur.read_at,cur.slots,cur.st=42,2000,2000,slots,16
    api.drive_v2(2,p,body,cur,true,2000,false,false)
    T.check(p.shown.settle_pos_uu==100 and not p.shown.settle_ready,
        "driver measures native transforms against previous integrated aim, not the close incoming target")
    for now=2016,2176,16 do
        cur.seq,cur.pt,cur.read_at=cur.seq+1,now,now
        api.drive_v2(2,p,body,cur,true,now,false,false)
    end
    T.check(p.shown.settle_ready and p.shown.settle_count==6,"production driver publishes converged six-limb evidence")
    twisted=true
    cur.seq,cur.pt,cur.read_at=cur.seq+1,2192,2192
    api.drive_v2(2,p,body,cur,true,2192,false,false)
    T.check(not p.shown.settle_ready and calls>0 and p.driving==true,
        "a later wounded/live pose clears evidence while ordinary body physics continues")
end

if opts.kind == "hook" then
    local api = boot(false)
    local log = M.logtext()
    T.check(api ~= nil, "HSMPAvatars loads under the mock with its test api")
    T.check(T.contains(log, "ReceiveTick=false(retrying)"), "load-time registration fails in the menu (BP not loaded)", log)
    T.check(api.tick_hook.ok == false and api.tick_hook.tries == 1, "one try at load")
    M.run(2000)   -- the menu: still not loaded, retried at 1 Hz
    T.check(api.tick_hook.ok == false and api.tick_hook.tries >= 2 and api.tick_hook.tries <= 4,
        "retried about once a second while unavailable", api.tick_hook.tries)
    M.register_ok = true   -- the arena loads Willie_BP
    M.run(1200)
    log = M.logtext()
    T.check(api.tick_hook.ok == true, "registered once the BP class exists")
    T.check(T.contains(log, "ReceiveTick post-hook registered (try"), "registration is logged", log)
    local h = M.hooks["/Game/Character/Blueprints/Willie_BP.Willie_BP_C:ReceiveTick"]
    T.check(h and h.callback==api.tick_hook.post and h.ignored_post==nil,
        "Blueprint post handler occupies the actual second argument slot")
    local n = M.hook_calls
    M.run(3000)
    T.check(M.hook_calls == n, "no further RegisterHook calls once registered")
    T.check(not T.contains(M.logtext(), "LOOP ERR"), "no loop errors", M.logtext())
end

if opts.kind=="hook_post"then
    T.write(sd.."/.settings.json",'{"avatars":true,"native_neutralise":false,"native_servo":false}\n')
    local api=boot(true)
    sidecar({{1,"Own"},{2,"Peer"}})
    HSMPNative.sc_put("session",{seq=1,match_id=309,round=1,phase=3})
    HSMPNative.sc_put("mode",{seq=1,match_id=309,round=1,rows={{peer_id=2,life=3}}})
    api.on_tick()
    local actor=M.new_obj("Willie_BP_C","TICK_PROXY");rawset(actor,"__addr",9501)
    local mesh=M.new_obj("SkeletalMeshComponent","TICK_MESH");rawset(mesh,"__addr",9502)
    local owner=actor;actor.__props.Mesh=mesh
    local motors,native_first=0,false
    M.Methods.GetOwner=function(self)if self==mesh then return owner end end
    M.Methods.SetAllMotorsAngularDriveParams=function(self,stiff,damp,force,skip)
        motors=motors+1
        T.check(self==mesh and stiff==0 and damp==0 and force==0 and skip==false and native_first
            and mesh.__props.motor_stiffness==800 and mesh.__props.motor_damping==100 and mesh.__props.motor_force==25,
            "actual registered post callback resets the qualified mesh motor after native BP writes")
        mesh.__props.motor_stiffness,mesh.__props.motor_damping,mesh.__props.motor_force=stiff,damp,force
    end
    local p={actor=actor,addr=9501,gen=api.generation(),peer=2,driving=true,
        last={has_context=true,match_id=309,round=1,life=3},
        shown={has_context=true,match_id=309,round=1,life=3,pawn="TICK_PROXY"},
        body={mesh=mesh,mesh_addr=9502,mesh_fname="TICK_MESH",field="Mesh",ctl="servo",motors={}}}
    api.set_puppet(2,p);api.set_driven(p)
    local function native(a)
        a.__props["Head Tonus"],a.__props["All Body Tonus"],a.__props["Muscle Power"]=42,100,35
        mesh.__props.motor_stiffness,mesh.__props.motor_damping,mesh.__props.motor_force=800,100,25
        native_first=true
    end
    M.bp_tick(actor,native)
    T.check(motors==1 and actor.__props["Head Tonus"]==0 and actor.__props["All Body Tonus"]==0
        and actor.__props["Muscle Power"]==0 and mesh.__props.motor_stiffness==0 and mesh.__props.motor_damping==0
        and mesh.__props.motor_force==0 and api.PX.hook_calls==1 and api.PX.hook_hits==1,
        "real second-slot Blueprint post callback neutralises native tone and records one qualified hit")
    local before=motors
    p.last.life=2;M.bp_tick(actor,native)
    T.check(motors==before and actor.__props["Head Tonus"]==42,"previous source life fails before actor or physical neutralisation")
    p.last.life=3;p.shown.life=2;M.bp_tick(actor,native)
    T.check(motors==before and actor.__props["Head Tonus"]==42,"current source cannot relabel an older displayed native body life")
    p.shown.life=3
    local replacement=M.new_obj("Willie_BP_C","TICK_REPLACEMENT");rawset(replacement,"__addr",9501)
    rawset(actor,"__dead",true);M.bp_tick(replacement,native)
    T.check(motors==before and #M.dead_touch==0 and replacement.__props["Head Tonus"]==42,
        "callback address reused by another native name fails before touching the old actor or body")
    rawset(actor,"__dead",false)
    local wrong_actor=M.new_obj("Willie_BP_C","TICK_WRONG_STORED_ACTOR");rawset(wrong_actor,"__addr",9551)
    p.actor=wrong_actor;M.bp_tick(actor,native)
    T.check(motors==before and actor.__props["Head Tonus"]==42 and wrong_actor.__props["Head Tonus"]==nil,
        "stored actor no longer matching the driven address fails before native variable or physical writes")
    p.actor=actor
    local alternate=M.new_obj("SkeletalMeshComponent","TICK_REBUILT_MESH");rawset(alternate,"__addr",9503)
    rawset(mesh,"__dead",true)
    actor.__props.Mesh=alternate;M.bp_tick(actor,native)
    T.check(motors==before and actor.__props["Head Tonus"]==42 and #M.dead_touch==0,
        "native replacement of a freed mesh fails before touching the retained wrapper or writing physics")
    rawset(alternate,"__addr",9502);M.bp_tick(actor,native)
    T.check(motors==before and #M.dead_touch==0,"component address reused by another native name cannot inherit body identity")
    rawset(mesh,"__dead",false)
    actor.__props.Mesh=mesh;owner=replacement;M.bp_tick(actor,native)
    T.check(motors==before and actor.__props["Head Tonus"]==42,"mesh owner with reused address and different native name fails before writes")
    owner=nil;M.bp_tick(actor,native)
    T.check(motors==before and actor.__props["Head Tonus"]==42,"unavailable mesh owner cannot authorize physical writes")
    owner=actor
    HSMPNative.sc_put("mode",{seq=2,match_id=309,round=1,rows={{peer_id=2,life=4}}});M.bp_tick(actor,native)
    T.check(motors==before and actor.__props["Head Tonus"]==42,"authoritative owner life change rejects the retained callback target")
    HSMPNative.sc_put("mode",{seq=3,match_id=309,round=1,rows={{peer_id=2,life=3}}})
    local original_world=M.world
    M.world=M.new_obj("World","TICK_WORLD_REPLACEMENT");rawset(M.world,"__addr",9601)
    local unwraps=M.tick_unwraps;M.bp_tick(actor)
    T.check(motors==before and M.tick_unwraps==unwraps,"fresh world identity change refuses the callback before unwrapping old native context")
    M.world=original_world;M.bp_tick(actor,native)
    T.check(motors==before+1,"unchanged exact actor/body/life continues after unavailable cases")
    M.premap();M.kill_all();unwraps=M.tick_unwraps
    M.bp_tick(actor)
    T.check(motors==before+1 and M.tick_unwraps==unwraps and #M.dead_touch==0,
        "world teardown leaves discarded callback and body wrappers untouched")
end

if opts.kind == "clock_probe" then
    local api=boot(true)
    local PX=api.PX
    PX.frame_no=81
    local p={play={seq_at=950}}
    local cur={pt=300,read_at=990,rate=1.2,lead=20,mode="extrap",age=22,delay=33,jit=14,iv=17,
        cut=4,st=23,seq=55,match_id=901,round=3,life=2,has_context=true}
    local prior={pt=100,at=900,r=0.8,has_context=true,match_id=901,round=3,life=2,cut=4}
    local next_clk,reset=PX.playback_clock(2,p,cur,prior,1000,false,true)
    T.check(reset and next_clk.pt==292 and next_clk.at==1000 and next_clk.r==1.2,
        "a jump above50ms resets to the existing expected-time formula")
    T.check(prior.pt==100 and prior.at==900 and prior.r==0.8,"pre-correction clock is not mutated")
    local events={}
    local function collect()
        events={}
        for line in (T.read(sd.."/hsmp_events.jsonl") or ""):gmatch("[^\n]+") do
            local e=T.json_decode(line)
            if e.ev=="x_pose_clock_reset" then events[#events+1]=e end
        end
    end
    collect()
    local e=events[1]
    T.check(#events==1 and e.expect==292 and e.projected_clk==180 and e.delta_ms==112 and e.threshold_ms==50,
        "one reset event captures signed discrepancy before clock correction",T.repr(events))
    T.check(e.clk_pt==100 and e.clk_at==900 and e.clk_rate==0.8 and e.local_ms==1000 and e.source_pt==300
        and e.read_at==990 and e.rate==1.2 and e.lead==20 and e.mode=="extrap" and e.age==22
        and e.delay==33 and e.jitter==14 and e.iv==17 and e.quiet==50 and e.cut==4 and e.step==23,
        "telemetry preserves source sample and buffer state before reset")
    T.check(e.peer==2 and e.frame==81 and e.source_seq==55 and e.fresh and e.match_id==901
        and e.round==3 and e.life==2 and e.has_context,"telemetry retains source frame and native life")
    cur.pt,cur.read_at,cur.lead,cur.rate=230,1000,0,1
    next_clk,reset=PX.playback_clock(2,p,cur,prior,1000,false,false)
    T.check(not reset and next_clk.pt==185 and next_clk.r==0.86,"exactly50ms keeps the original slew")
    PX.playback_clock(2,p,cur,nil,1000,false,true)
    PX.playback_clock(2,p,cur,prior,1000,true,true)
    collect()
    T.check(#events==1,"initialisation and explicit cuts are not clock-discontinuity events")
    cur.pt=129
    PX.playback_clock(2,p,cur,prior,1000,false,false)
    collect()
    T.check(#events==2 and events[2].delta_ms==-51 and events[2].fresh==false,
        "backward jumps preserve direction and cached-read attribution")

    -- A sidecar slot is evaluated at wall time 2000, while the late render
    -- callback still represents physical frame 1933. The next callback is
    -- quick. Backdating receipt to 1933 produces +67/-67 false phase resets.
    sidecar({{1,"Own"},{2,"Peer"}})
    HSMP_IPC.peer_dir(true)
    local puppet={}
    local function source(seq,pt)
        HSMPNative.sc_peer_play(2,{peer_id=2,seq=seq,v=2,pt=pt,lead=17,rate=1,mode="interp",cut=4,
            age=0,delay=20,jit=5,iv=17,m=0,B={},has_context=true,match_id=901,round=3,life=2})
    end
    M.now=2000;source(61,7017)
    api.drive_frame(2,puppet,1933)
    T.check(puppet.last and puppet.last.read_at==2000,
        "successful production slot read keeps actual host receipt time instead of late physical frame time",T.repr(puppet.last))
    local clock={pt=6916,at=1916,r=1,has_context=true,match_id=901,round=3,life=2,cut=4}
    local late,late_reset=PX.playback_clock(2,puppet,puppet.last,clock,1933,false,true)
    T.check(not late_reset and late.pt==6933,"a 67ms late source read projects back to the physical frame without a false forward reset")
    M.now=2016;source(62,7033)
    api.drive_frame(2,puppet,2016)
    local quick,quick_reset=PX.playback_clock(2,puppet,puppet.last,late,2016,false,true)
    T.check(not quick_reset and quick.pt==7016,"the next quick frame preserves phase instead of reversing by67ms")
    collect()
    T.check(#events==2,"late-read and quick-frame pair emits no hidden or threshold-relaxed reset")
    -- A genuine 80ms buffer-clock shift remains visible to the same detector.
    M.now=2032;source(63,6969)
    api.drive_frame(2,puppet,2032)
    puppet.last.delay=100
    PX.playback_clock(2,puppet,puppet.last,quick,2032,false,true)
    collect()
    T.check(#events==3 and events[3].delta_ms==-80 and events[3].threshold_ms==50 and events[3].delay==100,
        "real adaptive-delay phase shifts still reset and retain raw signed evidence")
end

if opts.kind=="clock_epoch" then
    T.write(sd.."/.settings.json",'{"avatars":true,"native_servo":false}\n')
    local api=boot(true)
    sidecar({{1,"Own"},{2,"Peer"}})
    HSMPNative.sc_put("session",{seq=1,match_id=308,round=1,phase=3})
    HSMPNative.sc_put("mode",{seq=1,match_id=308,round=1,rows={{peer_id=2,life=1}}})
    api.on_tick()
    local tuple={match_id=308,round=1,life=1}
    local peer_play=HSMPNative.peer_play
    HSMPNative.peer_play=function(slot,out,last)
        local seq=peer_play(slot,out,last)
        if seq then out.has_context=true;out.match_id=tuple.match_id;out.round=tuple.round;out.life=tuple.life;out.rate=1 end
        return seq
    end
    local actor=M.new_obj("Willie_BP_C","CLOCK_PROXY");rawset(actor,"__addr",9501)
    local mesh=M.new_obj("SkeletalMeshComponent","CLOCK_MESH");rawset(mesh,"__addr",9502)
    actor.__props.Mesh=mesh
    local sv={n=23,com={},err={n=0,e=0,emax=0,a=0,amax=0,hmax=0,capped=0},wc={}}
    for i=1,23 do sv.com[i]={0,0,0}end
    local old={pt=1100,at=2000,r=1,has_context=true,match_id=308,round=1,life=1,cut=1}
    local p={actor=actor,addr=9501,gen=api.generation(),peer=2,in_range=true,driving=true,claimed_at=0,cut_seen=1,clk=old,
        body={mesh=mesh,field="Mesh",ctl="servo",handles={},sims={mesh},motors={},sv=sv,snaps=0}}
    M.Methods.GetSocketTransform=function()return {Translation={X=0,Y=0,Z=0},Rotation={X=0,Y=0,Z=0,W=1}}end
    M.Methods.GetCenterOfMass=function()return {X=0,Y=0,Z=0}end
    M.Methods.GetSocketLocation=M.Methods.GetCenterOfMass
    M.Methods.K2_GetComponentLocation=M.Methods.GetCenterOfMass
    M.Methods.K2_SetWorldLocation=function()end
    M.Methods.SetAllPhysicsLinearVelocity=function()end
    M.Methods.SetSimulatePhysics=function()end
    M.Methods.SetEnableGravity=function()end
    M.Methods.GetCollisionResponseToChannel=function()return 2 end
    M.Methods.SetCollisionResponseToChannel=function()end
    M.Methods.GetPhysicsLinearVelocity=M.Methods.GetCenterOfMass
    local drives=0
    M.Methods.SetPhysicsLinearVelocity=function()drives=drives+1 end
    M.Methods.SetPhysicsAngularVelocityInDegrees=function()end
    local seq=0
    local function frame(now,cut,pt)
        M.now=now;seq=seq+1
        local b={};for _=1,23 do for _,v in ipairs({0,0,0,0,0,0,1,0,0,0,0,0,0})do b[#b+1]=v end end
        HSMPNative.sc_peer_play(2,{peer_id=2,seq=seq,v=2,pt=pt,rate=1,mode="interp",age=0,cut=cut,
            has_context=true,match_id=tuple.match_id,round=tuple.round,life=tuple.life,m=(1<<23)-1,B=b})
        api.drive_frame(2,p,now)
    end
    local function resets()
        local out={};for line in (T.read(sd.."/hsmp_events.jsonl")or""):gmatch("[^\n]+")do
            local e=T.json_decode(line);if e.ev=="x_pose_clock_reset"then out[#out+1]=e end
        end;return out
    end
    -- Native22 OFF and Native23 pre-probe cuts took these two return paths
    -- before the first servo tick, consuming cut_seen but retaining old clk.
    frame(2000,2,1140)
    T.check(p.cut_seen==2 and p.body.repose and p.clk==old and drives==0,
        "production cut frame advances body cut_seen but does not integrate the playback clock")
    frame(2016,2,1156)
    T.check(not p.body.repose and p.clk==old and drives==0,
        "production physics-on repose frame still leaves the clock integration pending")
    frame(2032,2,1172)
    T.check(drives>0 and p.clk~=old and p.clk.pt==1172 and p.clk.at==2032 and p.clk.cut==2
        and p.clk.match_id==308 and p.clk.round==1 and p.clk.life==1,
        "first actual servo tick binds new cut and resets even a sub50ms old-epoch phase error",T.repr(p.clk))
    T.check(#resets()==0,"known lifecycle clock initialization is not misreported as a same-epoch discontinuity")
    frame(2048,2,1268)
    local e=resets()
    T.check(#e==1 and e[1].delta_ms==80 and e[1].threshold_ms==50 and e[1].cut==2 and p.clk.pt==1268,
        "same-epoch80ms shift still emits raw evidence and resets through actual production drive",T.repr(e))
    local before=drives;tuple.life=2
    frame(2064,2,1284)
    T.check(drives==before and not p.driving and p.last==nil and p.clk.life==1,
        "production admission rejects unapproved life without relabeling its playback clock")
    local PX=api.PX
    local cur={pt=1400,read_at=2100,rate=1,cut=2,has_context=true,match_id=308,round=1,life=1}
    for _,key in ipairs({"match_id","round","life","cut"})do
        local prior={pt=1300,at=2100,r=1,has_context=true,match_id=308,round=1,life=1,cut=2}
        prior[key]=prior[key]+1
        local c,reset=PX.playback_clock(2,p,cur,prior,2100,false,true)
        T.check(not reset and c.pt==1400 and c[key]==cur[key],"clock scope independently resets changed "..key)
    end
    cur.has_context=false
    local unscoped=PX.playback_clock(2,p,cur,nil,2100,false,true)
    T.check(unscoped.has_context==nil and unscoped.match_id==nil,
        "unavailable context never becomes a bound clock epoch")
    cur.has_context=true;cur.life=0/0
    local invalid=PX.playback_clock(2,p,cur,nil,2100,false,true)
    T.check(invalid.has_context==nil and invalid.life==nil,"invalid life cannot authorize clock scope")
end

-- Dev tuning knobs: dev_cmd TUNE records (hsmp-tools ipc-ctl tune), no .pose_tune.json.
if opts.kind == "tune" then
    local api = boot(true)
    local N = HSMPNative
    T.check(type(N.sc_dev) == "function", "the mock has the DevCtl ring (sc_dev)")
    local t = api.tune()
    T.check(t.servo == nil and select(1, api.caps()) == 900, "defaults before any record")
    T.write(sd .. "/.pose_tune.json", '{"servo":0,"cap_lin":5}\n')
    api.on_tick()
    T.check(api.tune().servo == nil and select(1, api.caps()) == 900, "a .pose_tune.json file is ignored (no state-dir knob)")
    N.sc_dev({ id = 1, op = 2, key = "servo", num = 0 })
    N.sc_dev({ id = 2, op = 2, key = "cap_lin", num = 1e9 })
    N.sc_dev({ id = 3, op = 2, key = "lead", num = -12.5 })
    N.sc_dev({ id = 4, op = 2, key = "tdiag", num = 1 })
    N.sc_dev({ id = 5, op = 2, key = "bench", num = 0 })
    api.on_tick()
    t = api.tune()
    T.check(t.servo == 0, "servo 0 stops driving", t.servo)
    T.check(t.cap_lin == 20000 and select(1, api.caps()) == 20000, "cap_lin clamped to 20000 and applied", t.cap_lin)
    T.check(t.lead == -12.5, "lead set", t.lead)
    T.check(t.tdiag == 1 and t.bench == 1, "tdiag flag on; bench clamped to >= 1", T.repr({ t.tdiag, t.bench }))
    T.check(T.contains(M.logtext(), "pose tune: "), "a tune change is logged")
    N.sc_dev({ id = 6, op = 2, key = "tdiag", num = 0 })
    N.sc_dev({ id = 7, op = 2, key = "servo", num = 3 })
    N.sc_dev({ id = 8, op = 2, key = "warp", num = 1 })
    N.sc_dev({ id = 9, op = 2, key = "warp", num = 2 })
    N.sc_dev({ id = 10, op = 1, key = "gain", num = 0.9 })   -- AUTOTEST: not ours
    N.sc_dev({ id = 11, op = 3, key = "tdiag", num = 1 })    -- TDIAG: HSMPSync's
    api.on_tick()
    t = api.tune()
    T.check(not t.tdiag and t.servo == 1, "tdiag off clears the flag; a nonzero flag value is stored as 1", T.repr({ t.tdiag, t.servo }))
    T.check(t.warp == nil and T.count(M.logtext(), "warp") == 1, "an unknown key is ignored and logged once", M.logtext())
    T.check(t.gain == nil, "records of other ops are ignored")
    T.check(api.PURE.tune_value("gain", 0 / 0) == nil and api.PURE.tune_value("gain", 7) == 1, "NaN refused; gain clamped to 1")
end

if opts.kind == "bodyheight" then
    local api = boot(true)
    local me = M.new_obj("Willie_BP_C", "ME"); rawset(me,"__addr",7001)
    me.__props.Health=100; M.pc.__props.Pawn=me
    sidecar({ {1,"Me"}, {2,"Peer"} }); peer_root(2,10,300,0,100); M.run(2200)
    local actor_scale, mesh_scale, sim, calls = {X=.97,Y=.97,Z=.97}, {X=.997,Y=.996,Z=.998}, true, {}
    local field="Height_21_0EB204DF4978B92AD0ED188FD32EEC7B"
    local mesh={IsValid=function() return true end,GetAddress=function() return 8002 end,
        K2_GetComponentScale=function() return mesh_scale end,IsSimulatingPhysics=function() return sim end,
        SetSimulatePhysics=function(_,v) sim=v;calls[#calls+1]=v and "on" or "off" end,
        SetWorldScale3D=function(_,v) mesh_scale=v;calls[#calls+1]="mesh" end}
    local actor={IsValid=function() return true end,GetAddress=function() return 8001 end,
        GetFName=function() return {ToString=function() return "REMOTE" end} end,Mesh=mesh,["Character Passport"]={[field]=.76},
        GetActorScale3D=function() return actor_scale end,
        SetActorScale3D=function(_,v) actor_scale=v;calls[#calls+1]="actor" end}
    actor["Set Character Height"]=function(self)
        calls[#calls+1]="native-height"
        local k=.875+.125*self["Character Passport"][field];self:SetActorScale3D({X=k,Y=k,Z=k})
    end
    FindAllOf=function(cls) if cls=="Willie_BP_C" then return {me,actor} end end
    local p={gen=api.generation(),actor=actor,driving=true,in_range=true,nick="Peer",
        body={mesh=mesh,ctl="servo",sv={},handles={},sims={}}}
    T.check(select(2,api.generation())==p.gen,"height test uses settled current-world cache",T.repr({api.generation()}))
    api.set_puppet(2,p)
    api.PX.bodyheight("2 0.946")
    T.check(p.body.height_probe and p.body.repose and calls[1]=="off" and calls[2]=="native-height",
        "remote height probe enters driver's physics-off window before native actor scaling",M.logtext())
    T.check(p.body.sv==nil and p.body.scale_remeasure,"probe discards cached COM and rest offsets")
    if not p.body.height_probe then return end
    local expiry=p.body.height_probe.until_t
    api.PX.height_tick(p,expiry-.01)
    T.check(actor["Character Passport"][field]==.946,"geometry probe remains active before deadline")
    api.PX.height_tick(p,expiry)
    T.check(p.body.height_probe==nil and actor["Character Passport"][field]==.76 and actor_scale.X==.97,
        "bounded timeout restores original native passport/actor scale without a new pose")
    p.body.repose=nil;sim=true
    api.PX.bodyheight("2 0.946")
    api.PX.height_restore(p,"release",false)
    T.check(p.body.height_probe==nil and sim and not p.body.repose and mesh_scale.Y==.996,
        "release cleanup restores original geometry and simulation immediately")
    p.body.repose=nil;sim=true
    api.PX.bodyheight("2 0.946")
    local n=#calls
    actor.IsValid=function() error("old world object touched") end
    mesh.IsValid=function() error("old world mesh touched") end
    api.drop_caches("test world teardown")
    T.check(#calls==n and next(api.puppets())==nil,
        "world teardown discards diagnostic state without touching old native objects")
    actor.IsValid=function() return true end;mesh.IsValid=function() return true end
    p.body.height_probe=nil;p.body.repose=nil
    api.set_puppet(2,p)
    p.body.repose=nil
    M.pc.__props.Pawn=actor
    n=#calls;api.PX.bodyheight("2 0.946")
    T.check(#calls==n and p.body.height_probe==nil,"probe refuses a stand-in now possessed as our pawn")
end

if opts.kind == "pose_context" then
    local api = boot(true)
    HSMPNative.sc_put("session", { seq = 1, match_id = 901, round = 2, phase = 3 })
    HSMPNative.sc_put("mode", { seq = 1, match_id = 901, round = 2,
        rows = { { peer_id = 2, seat = 1, life = 3, alive = true } } })
    local p = { gen = -1, driving = true, last = { v2 = true, has_context = true,
        match_id = 901, round = 2, life = 2 }, aim = {}, shown = {}, play = { seq = 77, seq_at = 500 } }
    api.drive_frame(2, p, 501)
    T.check(p.driving == false, "authoritative new life releases an old cached servo immediately")
    T.check(p.last == nil and p.aim == nil and p.shown == nil and p.play.seq == nil,
        "old cached pose and displayed timeline are removed before another frame can drive")
end

if opts.kind == "parse" then
    local api = boot(true)
    local P = api.PURE
    local s = { match_id = 901, round = 2, state = "live" }
    local m = { match_id = 901, round = 2, rows = { [2] = { life = 3 } } }
    local pose = { has_context = true, match_id = 901, round = 2, life = 3, B = {}, m = 0 }
    T.check(P.pose_context_ok(pose, s, m, 2), "pose belongs to current match/round/native life")
    T.check(not P.pose_context_ok(pose, s, nil, 2), "live pose waits for authoritative Mode life")
    pose.life = 2
    T.check(not P.pose_context_ok(pose, s, m, 2), "previous life is rejected even when cached")
    pose.life, pose.match_id = 3, 900
    T.check(not P.pose_context_ok(pose, s, m, 2), "previous match pose is rejected")
    pose.match_id, pose.round = 901, 1
    T.check(not P.pose_context_ok(pose, s, m, 2), "previous round pose is rejected")
    pose.round, pose.has_context = 2, false
    T.check(not P.pose_context_ok(pose, s, m, 2), "active match never accepts an unlabelled pose")
    -- A relayed peer_root as the native decoder hands it over: plain Root fields, no has_context.
    local root = { tick = 2600, ts = 65676, pos = { 815.3, 280.4, 377.0 }, rot = { 0, 0, -0.95, 0.30 },
                   vel = { 0, 0, 0 }, match_id = 901, round = 2, life = 3, send_wall_ms = 1 }
    T.check(P.pose_context_ok(P.root_context(root), s, m, 2), "stamped peer root of the current life is live (stand-in can spawn)")
    root.life = 2
    T.check(not P.pose_context_ok(P.root_context(root), s, m, 2), "peer root of a previous life is rejected")
    root.life, root.match_id = 3, 0
    T.check(not P.pose_context_ok(P.root_context(root), s, m, 2), "unstamped peer root is rejected in a match")
    s.state, s.spawn_round, pose.round, pose.life, pose.has_context = "countdown", 3, 3, 1, true
    T.check(P.pose_context_ok(pose, s, m, 2), "next round placement uses its own initial life")
    local shown=P.displayed_pose(pose,"Willie_BP_C_rendered",100,500)
    pose.life=2
    local row=P.playback_row(2,shown,501)
    T.check(row and row.life==1 and row.match_id==901 and row.round==3 and row.pawn=="Willie_BP_C_rendered",
        "displayed playback retains original pose life and actual pawn after source table changes")
    T.check(P.playback_row(2,shown,751)==nil,"stalled displayed pose cannot attribute new contacts")
    local delayed=P.playback_row(2,shown,850,true)
    T.check(delayed and delayed.local_ms==500 and delayed.life==1,
        "approved delayed trade retains original timestamp and life without refreshing contact eligibility")
    T.check(P.playback_row(2,shown,499,true)==nil,"retained generation cannot authorize a future timestamp")
    shown.label=100.75
    T.check(P.playback_row(2,shown,501)==nil,"a fractional target cannot be silently rounded during publication")
    local label, aim, ms, ma=P.display_times(40260.75,40269.9,17.7,120,false)
    T.check(label==40269 and aim==40286 and 40260.75+ms==label and 40260.75+ma==aim,
        "shown and next-step aim times are quantized before target construction")
    local tg={0,0,0,0,0,0,1,1000,0,0,0,0,0}
    T.check(P.advance(tg,ms)[1]==8.25 and P.aim(tg,ms,ma)[1]==25.25,
        "servo targets use exactly the published physical times, independent of sender step")
    label,aim,ms,ma=P.display_times(1000.75,1300,80,120,false)
    T.check(label==1120 and aim==1120 and ms==119.25 and ma==119.25,
        "a capped prediction publishes the actual capped aim rather than label plus horizon")
    label,aim,ms,ma=P.display_times(1000.75,1300,80,120,true)
    T.check(label==1000 and aim==1000 and ms==-0.75 and ma==-0.75,
        "a frozen target keeps one quantized physical time")
    shown.label=100
    shown.pawn=""
    T.check(P.playback_row(2,shown,501)==nil,"displayed playback requires exact native actor identity")
    pose.life=1
    pose.life = 3
    T.check(not P.pose_context_ok(pose, s, m, 2), "old Mode life cannot authorize next round pose")
    pose.life = 1
    local parsed = P.play_from_out(pose)
    T.check(parsed.has_context and parsed.match_id == 901 and parsed.round == 3 and parsed.life == 1,
        "typed pose retains life context in alternating cached output")
    P.play_from_out({ B = {}, m = 0 }, parsed)
    T.check(not parsed.has_context and parsed.match_id == nil and parsed.life == nil,
        "reused pose table clears absent life context")
    -- the typed `standin_dead` bus record HSMPCombat writes ({wall, rows = {{peer, name}}})
    local set, wall = api.parse_standin_dead({ wall = 1700000000,
        rows = { { peer = 2, name = "Willie_BP_C_5" }, { peer = 11, name = "Willie_BP_C_9" } } })
    T.check(set and set[2] == "Willie_BP_C_5" and set[11] == "Willie_BP_C_9" and wall == 1700000000, "parse: complete record")
    T.check(api.parse_standin_dead(nil) == nil and api.parse_standin_dead({}) == nil, "parse: no record / no wall stamp")
    local e = api.parse_standin_dead({ wall = 5, rows = {} })
    T.check(e and next(e) == nil, "parse: empty set")
    local N = HSMPNative
    T.check(N.bus_put("standin_dead", { wall = os.time(), rows = { { peer = 2, name = "W" } } }) == true, "typed bus write")
    T.check(api.combat_declared_dead(2) and not api.combat_declared_dead(3), "declared: listed peer only")
    T.check(api.combat_declared_dead(2), "an unchanged record keeps the last set")
    T.check(api.combat_declared_dead(2, "W") and not api.combat_declared_dead(2, "Willie_BP_C_77"),
        "the listed stand-in name must match")
    N.bus_put("standin_dead", { wall = os.time() - 60, rows = { { peer = 2, name = "W" } } })
    T.check(not api.combat_declared_dead(2), "a set older than 5 s is void (crashed HSMPCombat)")
end

if opts.kind == "corpse" then
    local api = boot(true)
    -- an MP arena: our pawn + one peer (2) with a stand-in
    local me = M.new_obj("Willie_BP_C", "Willie_BP_C_ME"); rawset(me, "__addr", 7001)
    me.__props.Health = 100
    M.pc.__props.Pawn = me
    sidecar({ { 1, "Me" }, { 2, "B" } })
    peer_root(2, 10, 100.0, 0.0, 90.0)
    M.run(2200)   -- past PX.WORLD_SETTLE_S: on_tick touches no Willie before
    local corpse = M.new_obj("Willie_BP_C", "Willie_BP_C_5"); rawset(corpse, "__addr", 7005)
    corpse.__props.Health = 100
    api.set_puppet(2, { actor = corpse, nick = "B", claimed_at = 0, gen = 0, addr = 7005 })
    M.run(200)
    T.check(api.puppets()[2] and api.puppets()[2].actor == corpse, "stand-in kept while alive")
    -- HSMPCombat: server says peer 2 died -> record first, then Health 0 + Death()
    if opts.standin_dead then
        HSMPNative.bus_put("standin_dead", { wall = os.time(), rows = { { peer = 2, name = "Willie_BP_C_5" } } })
    end
    if opts.vitals_dead then   -- the owner's own `peer_vitals` record says dead (VF DEAD, Health 0)
        HSMPNative.sc_put("peer_vitals", { seq = 9, flags = 1, v = { 0 } }, 2)
    end
    corpse.__props.Health = 0
    corpse.__props.DED = true
    M.run(2000)   -- the owner's vitals never caught up in this window
    local log = M.logtext()
    if opts.standin_dead or opts.vitals_dead then
        T.check(api.puppets()[2] and api.puppets()[2].actor == corpse, "server-declared death keeps the corpse", T.repr(api.puppets()[2]))
        T.check(not T.contains(log, "re-claiming"), "no re-claim (no living stand-in 37 ms later)", log)
        T.check(api.puppets()[2].owner_dead == true, "the corpse is owner-dead (free ragdoll, not driven)")
    else
        T.check(T.contains(log, "puppet died locally (HP 0.0 DED=true); re-claiming") or T.contains(log, "puppet died locally (HP 0 DED=true); re-claiming"),
            "a local death without an owner death is still re-claimed", log)
    end
end

if opts.kind == "census" then
    -- Peer 3 is rostered but still in the lobby (no snapshot); the
    -- unreplicated extra foe must still be removed once peer 2 has a stand-in.
    local api = boot(true)
    M.Methods.SetActorHiddenInGame = function(self, h) self.__props.bHidden = h end
    M.Methods.IsActorBeingDestroyed = function() return false end
    local me = M.new_obj("Willie_BP_C", "Willie_BP_C_ME"); rawset(me, "__addr", 7001)
    me.__props.Health = 100
    M.pc.__props.Pawn = me
    local standin = M.new_obj("Willie_BP_C", "Willie_BP_C_5"); rawset(standin, "__addr", 7005)
    standin.__props.Health = 100
    local extra = M.new_obj("Willie_BP_C", "Willie_BP_C_9"); rawset(extra, "__addr", 7009)
    extra.__props.Health = 100
    _G.FindAllOf = function(c)
        if c == "Willie_BP_C" then return { me, standin, extra } end
        return nil
    end
    local tick = 0
    local function beat()
        tick = tick + 1
        sidecar({ { 1, "Me" }, { 2, "B" }, { 3, "Lobby" } })
        peer_root(2, 10 + tick, 100.0, 0.0, 90.0)
    end
    beat(); M.run(300)
    api.set_puppet(2, { actor = standin, nick = "B", claimed_at = 0, gen = 0, addr = 7005 })
    for _ = 1, 30 do beat(); M.run(250) end
    local log = M.logtext()
    T.check(T.contains(log, "census: removed Willie_BP_C_9"), "a rostered peer without a pawn does not block the census", log)
    T.check(extra.__props.bHidden == true and standin.__props.bHidden ~= true, "only the extra is taken out")
    -- Every connected roster peer has a PeerDir entry now (lobby included): an entry alone
    -- never claims a stand-in; only an advancing PeerRoot does.
    T.check(api.puppets()[3] == nil and not T.contains(log, "puppet for peer 3"),
        "a PeerDir entry without a streaming PeerRoot never claims a stand-in", T.repr(api.puppets()[3]))
end

if opts.kind == "frame" then
    -- No PlayerController lookup (a full FindAllOf in the real
    -- UEHelpers) per rendered frame; a stand-in leaving POSE_RANGE is
    -- released (gravity / collision / muscles back), not left hanging.
    local api = boot(true)
    local U = require("UEHelpers")
    local pc_calls = 0
    local orig = U.GetPlayerController
    U.GetPlayerController = function() pc_calls = pc_calls + 1; return orig() end
    M.Methods.K2_GetActorLocation = function(self) return rawget(self, "loc") or { X = 0, Y = 0, Z = 100 } end
    local me = M.new_obj("Willie_BP_C", "Willie_BP_C_ME"); rawset(me, "__addr", 7001)
    me.__props.Health = 100
    M.pc.__props.Pawn = me
    local si = M.new_obj("Willie_BP_C", "Willie_BP_C_5"); rawset(si, "__addr", 7005)
    si.__props.Health = 100
    rawset(si, "loc", { X = 300, Y = 0, Z = 100 })
    local tick = 0
    local function beat()
        tick = tick + 1
        sidecar({ { 1, "Me" }, { 2, "B" } })
        peer_root(2, 10 + tick, 300.0, 0.0, 90.0)
        -- a live v2 pose stream with no bodies (the capsule only): keeps it driven
        HSMPNative.sc_peer_play(2, { peer_id = 2, seq = tick, v = 2, pt = 0, mode = "interp", cut = 0, root = { 300.0, 0.0, 100.0, 0.0 }, m = 0, B = {} })
    end
    beat(); M.run(300)
    for _ = 1, 8 do beat(); M.run(250) end   -- past PX.WORLD_SETTLE_S
    api.set_puppet(2, { actor = si, nick = "B", claimed_at = 0, gen = 0, addr = 7005, driving = true, in_range = true })
    M.run(100)
    pc_calls = 0
    for _ = 1, 4 do beat(); M.run(250) end   -- 1 s: ~30 ticks, ~125 frames (8 ms fallback loop)
    T.check(pc_calls > 0 and pc_calls <= 45, "PlayerController lookups follow the 33 ms tick, not the frame rate (" .. pc_calls .. " in 1 s)")
    -- out of range
    rawset(si, "loc", { X = 6000, Y = 0, Z = 100 })
    api.puppets()[2].driving = true
    for _ = 1, 2 do beat(); M.run(250) end
    local log = M.logtext()
    T.check(T.contains(log, "pose peer 2: out of range"), "leaving POSE_RANGE releases the stand-in", log)
    T.check(api.puppets()[2] and api.puppets()[2].driving == false, "released: no longer driven")
    T.check(not T.contains(log, "LOOP ERR"), "no loop errors")
end

if opts.kind == "levelchange" then
    -- Strict mock: a round reset frees every object of the old world;
    -- nothing may be touched afterwards, IsValid() included (it crashes the game).
    local api = boot(true)
    local me = M.new_obj("Willie_BP_C", "Willie_BP_C_ME"); rawset(me, "__addr", 7001)
    me.__props.Health = 100
    M.pc.__props.Pawn = me
    local si = M.new_obj("Willie_BP_C", "Willie_BP_C_5"); rawset(si, "__addr", 7005)
    si.__props.Health = 100
    local tick = 0
    local function beat()
        tick = tick + 1
        sidecar({ { 1, "Me" }, { 2, "B" } })
        peer_root(2, 10 + tick, 300.0, 0.0, 90.0)
        HSMPNative.sc_peer_play(2, { peer_id = 2, seq = tick, v = 2, pt = 0, mode = "interp", cut = 0, root = { 300.0, 0.0, 100.0, 0.0 }, m = 0, B = {} })
    end
    beat(); M.run(300)
    for _ = 1, 8 do beat(); M.run(250) end   -- past PX.WORLD_SETTLE_S
    api.set_puppet(2, { actor = si, nick = "B", claimed_at = 0, gen = 0, addr = 7005, driving = true, in_range = true })
    for _ = 1, 3 do beat(); M.run(250) end
    -- round reset: LoadMap pre-hook, the old world freed, a new world + pawn
    M.premap(); M.kill_all()
    M.world = M.new_obj("World", "world2"); M.pc = M.new_obj("PC", "pc2")
    local me2 = M.new_obj("Willie_BP_C", "Willie_BP_C_ME2"); rawset(me2, "__addr", 7101)
    me2.__props.Health = 100
    M.pc.__props.Pawn = me2
    package.loaded["UEHelpers"].GetPlayerController = function() return M.pc end
    package.loaded["UEHelpers"].GetWorld = function() return M.world end
    if M.postmap then M.postmap() end
    for _ = 1, 4 do beat(); M.run(250) end
    T.check(#M.dead_touch == 0, "nothing of the old world touched after the reset, IsValid included", T.repr(M.dead_touch))
    T.check(api.puppets()[2] == nil or api.puppets()[2].actor ~= si, "the old stand-in is forgotten")
end

if opts.kind == "weapon_destroyed" then
    -- HSMPLoadout K2_DestroyActor's a stand-in's hand weapon (disarm /
    -- world item in hand); GC frees it ~60 s later. The cached root component
    -- (body.sv.wc) must never be touched again: in the strict mock IsValid on
    -- a freed object raises (as the real access violation would kill the game).
    local api = boot(true)
    local PX = api.PX
    _G.StaticFindObject = function() return { __cls = "Class" } end
    M.Methods.IsSimulatingPhysics = function() return true end
    M.Methods.SetEnableGravity = function(self, on) rawset(self, "grav", on) end
    M.Methods.SetCollisionResponseToChannel = function() end
    M.Methods.GetTransform = function() return { Translation = { X = 0, Y = 0, Z = 0 }, Rotation = { X = 0, Y = 0, Z = 0, W = 1 } } end
    M.Methods.GetCenterOfMass = function() return { X = 1, Y = 0, Z = 0 } end
    M.Methods.K2_GetComponentLocation = function() return { X = 0, Y = 0, Z = 0 } end
    M.Methods.K2_SetWorldLocation = function(self) rawset(self, "moved", (rawget(self, "moved") or 0) + 1) end
    M.Methods.SetPhysicsLinearVelocity = function() end
    M.Methods.SetPhysicsAngularVelocityInDegrees = function() end
    M.Methods.SetAllPhysicsLinearVelocity = function() end
    M.Methods.GetSocketLocation = function() return { X = 0, Y = 0, Z = 0 } end
    local comps = {}
    M.Methods.K2_GetComponentsByClass = function() return comps end
    local si = M.new_obj("Willie_BP_C", "Willie_BP_C_5"); rawset(si, "__addr", 7005)
    local function weapon(name, addr, raddr)
        local w = M.new_obj("ModularWeaponBP_Polearm_C", name); rawset(w, "__addr", addr)
        local r = M.new_obj("StaticMeshComponent", name .. ".Root"); rawset(r, "__addr", raddr)
        w.__props.RootComponent = r
        return w, r
    end
    local function destroy(o) rawset(o, "IsValid", function() return false end) end   -- pending kill
    local function free(o) rawset(o, "IsValid", nil); rawset(o, "__dead", true) end   -- GC freed it: IsValid is an AV
    local mesh = M.new_obj("SkeletalMeshComponent", "Mesh")
    local body = { mesh = mesh, sims = { mesh }, snaps = 0, gravity = false, sv = { wc = {} } }
    local p = { actor = si, body = body }
    local w1, r1 = weapon("ModularWeaponBP_Polearm_C_3", 8001, 8002)
    si.__props["Weapon R"] = w1
    local c, wa = api.servo_weapon_parts(p, body, "Weapon R")
    T.check(c and wa == w1 and body.sv.wc["Weapon R"] == c and c.root == r1 and r1.grav == false,
        "a held weapon is cached with gravity off (servoed)")
    -- 1. disarm: Loadout destroys it, the hand reads empty, GC frees it
    destroy(w1); destroy(r1)
    si.__props["Weapon R"] = nil
    T.check(api.servo_weapon_parts(p, body, "Weapon R") == nil and body.sv.wc["Weapon R"] == nil,
        "an empty hand drops the cached entry at once")
    free(w1); free(r1)
    body.gravity = nil
    local ok, err = pcall(api.set_gravity, body, false)
    T.check(ok and #M.dead_touch == 0, "set_gravity never touches the freed root", tostring(err) .. " " .. T.repr(M.dead_touch))
    -- 2. a stale entry that servo_weapon_parts did not see this frame (the
    -- left hand): every user validates against a FRESH hand read first
    local w2, r2 = weapon("ModularWeaponBP_Sword_C_4", 8011, 8012)
    si.__props["Weapon L"] = w2
    api.servo_weapon_parts(p, body, "Weapon L")
    destroy(w2); destroy(r2); si.__props["Weapon L"] = nil; free(w2); free(r2)
    body.gravity = nil
    ok, err = pcall(api.set_gravity, body, true)
    T.check(ok and #M.dead_touch == 0 and body.sv.wc["Weapon L"] == nil, "set_gravity drops a stale entry untouched", tostring(err) .. " " .. T.repr(M.dead_touch))
    -- 3. ABA: a NEW weapon at the old address (different FName / root)
    local w3, r3 = weapon("ModularWeaponBP_Polearm_C_9", 8001, 8022)
    si.__props["Weapon R"] = w3
    local w0, r0 = weapon("ModularWeaponBP_Polearm_C_8", 8031, 8032)
    si.__props["Weapon R"] = w0
    local c0 = api.servo_weapon_parts(p, body, "Weapon R")
    destroy(w0); destroy(r0); si.__props["Weapon R"] = w3   -- swapped by the BP; w3 reuses an address
    rawset(w3, "__addr", 8031)
    free(w0); free(r0)
    local c3 = api.servo_weapon_parts(p, body, "Weapon R")
    T.check(c3 and c3 ~= c0 and c3.root == r3 and #M.dead_touch == 0,
        "same address, other FName: re-resolved, the old root never touched", T.repr(M.dead_touch))
    -- 4. snap_mesh / release_standin after a destroy
    destroy(w3); destroy(r3); si.__props["Weapon R"] = nil; free(w3); free(r3)
    ok, err = pcall(api.snap_mesh, body, 10, 0, 0)
    T.check(ok and #M.dead_touch == 0, "snap_mesh never moves a freed weapon", tostring(err) .. " " .. T.repr(M.dead_touch))
    -- 5. Loadout's destroy signal: a new generation drops every cached root pointer
    local w4, r4 = weapon("ModularWeaponBP_Sword_C_12", 8041, 8042)
    si.__props["Weapon R"] = w4
    local c4 = api.servo_weapon_parts(p, body, "Weapon R")
    HSMPNative.bus_put("standin_weapons", { gen = 7 })   -- the typed bus record HSMPLoadout bumps
    PX.wc_gen_at = -1e9
    PX.wc_valid(body)
    T.check(body.sv.wc_gen == "7" and c4.root == r4, "a new standin_weapons generation re-validates every entry (live ones keep a FRESH root)")
    -- 6. grips: the BP rebuilt the hand constraints; the cached ones are freed
    local g1 = M.new_obj("PhysicsConstraintComponent", "Grip_old"); rawset(g1, "__addr", 9001)
    p.grips = { at = 0, list = { { c = g1, addr = 9001, hand = "hand_r", stiff = 1 } }, by = {} }
    free(g1)
    local g2 = M.new_obj("PhysicsConstraintComponent", "Grip_new"); rawset(g2, "__addr", 9002)
    comps = { g2 }
    ok, err = pcall(PX.grips_off, p, true)
    T.check(ok and #M.dead_touch == 0 and p.grips.list[1].c == nil, "grips_off never touches a rebuilt (freed) grip", tostring(err) .. " " .. T.repr(M.dead_touch))
end

if opts.kind == "bodies3" then
    -- A 3-player match needs 2 stand-ins on this client. In MP the
    -- arenas spawn no foes, so the SpawnCombatants fallback is the only body
    -- source: ONE call must ask for every missing body, and a peer that still
    -- has none once the round is Live (re-claim, join in progress) must still
    -- get one.
    local api = boot(true)
    M.Methods.SetActorHiddenInGame = function(self, h) self.__props.bHidden = h end
    M.Methods.IsActorBeingDestroyed = function() return false end
    M.Methods.K2_SetActorLocation = function(self, l) rawset(self, "loc", l) end
    M.Methods.K2_GetActorLocation = function(self) return rawget(self, "loc") or { X = 500, Y = 0, Z = 100 } end
    M.Methods.SetSphereRadius = function() end
    local me = M.new_obj("Willie_BP_C", "Willie_BP_C_ME"); rawset(me, "__addr", 7001)
    me.__props.Health = 100
    -- our pawn away from the peers' poses: the fallback never spawns a body
    -- within 250 uu of our own pawn (PX.spawn_spot)
    rawset(me, "loc", { X = -2000, Y = 0, Z = 100 })
    M.pc.__props.Pawn = me
    local bodies, calls, nb = { me }, {}, 0
    local lm = M.new_obj("BP_LevelManager_C", "LM")
    lm.__props["Spawn Combatants"] = function(self)
        local n = self.__props["Amount of Characters to Spawn"] or 0
        calls[#calls + 1] = n
        for _ = 1, n do
            nb = nb + 1
            local w = M.new_obj("Willie_BP_C", "Willie_BP_C_" .. (20 + nb)); rawset(w, "__addr", 7100 + nb)
            w.__props.Health = 100
            rawset(w, "loc", { X = 1000 + 200 * nb, Y = 0, Z = 100 })
            bodies[#bodies + 1] = w
        end
    end
    local sps = { M.new_obj("BP_SpawnerPoint_Willies_C", "SP1"), M.new_obj("BP_SpawnerPoint_Willies_C", "SP2") }
    for _, sp in ipairs(sps) do sp.__props.Sphere = M.new_obj("SphereComponent", "S") end
    _G.FindAllOf = function(c)
        if c == "Willie_BP_C" then return bodies end
        if c == "BP_LevelManager_C" then return { lm } end
        if c == "BP_SpawnerPoint_Willies_C" then return sps end
        return nil
    end
    local tick, peers = 0, { 2, 3 }
    local function director(state) HSMP_IPC.bus_put("director", { state = state }) end
    local function beat()
        tick = tick + 1
        local ps = { { 1, "Me" } }
        for _, id in ipairs(peers) do ps[#ps + 1] = { id, "P" .. id } end
        sidecar(ps)
        for _, id in ipairs(peers) do
            peer_root(id, 10 + tick, 100.0 * id, 50.0 * id, 90.0)
        end
    end
    director("Spawn")
    for _ = 1, 36 do beat(); M.run(250) end   -- 9 s: past the 2 s settle and the 6 s fallback grace
    local P = api.puppets()
    T.check(calls[1] == 2, "the first SpawnCombatants call asks for BOTH missing bodies", T.repr(calls))
    T.check(P[2] and P[3] and P[2].actor ~= P[3].actor, "both opponents get a stand-in", T.repr({ calls, P[2] and P[2].addr, P[3] and P[3].addr }))
    T.check(P[2] and P[3] and P[2].actor["Force Disable Vertex Paint"]==true and P[3].actor["Force Disable Vertex Paint"]==true,
        "actual stand-in claim suppresses native paint before combat protects it")
    local sl1, sl2 = rawget(sps[1], "loc"), rawget(sps[2], "loc")
    T.check(sl1 and sl2 and (sl1.X ~= sl2.X or sl1.Y ~= sl2.Y), "the spawners go to distinct peer poses (no stacked bodies)", T.repr({ sl1, sl2 }))
    -- the round is Live; a fourth player joins in progress and needs a body
    director("Live")
    peers = { 2, 3, 4 }
    local n0 = #calls
    for _ = 1, 48 do beat(); M.run(250) end   -- 12 s
    P = api.puppets()
    T.check(#calls > n0 and calls[#calls] >= 1, "the fallback is allowed while the round is Live", T.repr(calls))
    T.check(P[4] ~= nil, "the join-in-progress peer gets a stand-in in Live")
    T.check(not T.contains(M.logtext(), "LOOP ERR"), "no loop errors", M.logtext())
end

if opts.kind == "spectate" then
    -- After my death only the camera moves (HSMPMatch .spectate.json):
    -- stand-in range is measured from the camera, the spectated stand-in is
    -- always driven, and there is 10% hysteresis at the edge.
    local api = boot(true)
    M.Methods.K2_GetActorLocation = function(self) return rawget(self, "loc") or { X = 0, Y = 0, Z = 100 } end
    local cam = { loc = { X = 0, Y = 0, Z = 200 } }
    local cm = M.new_obj("PlayerCameraManager", "PCM")
    rawset(cm, "GetCameraLocation", function() return cam.loc end)
    M.pc.__props.PlayerCameraManager = cm
    local me = M.new_obj("Willie_BP_C", "Willie_BP_C_ME"); rawset(me, "__addr", 7001)
    me.__props.Health = 100
    M.pc.__props.Pawn = me
    local si = M.new_obj("Willie_BP_C", "Willie_BP_C_5"); rawset(si, "__addr", 7005)
    si.__props.Health = 100
    rawset(si, "loc", { X = 6000, Y = 0, Z = 100 })
    local tick = 0
    local function beat()
        tick = tick + 1
        sidecar({ { 1, "Me" }, { 2, "B" } })
        peer_root(2, 10 + tick, 6000.0, 0.0, 90.0)
        HSMPNative.sc_peer_play(2, { peer_id = 2, seq = tick, v = 2, pt = 0, mode = "interp", cut = 0, root = { 6000.0, 0.0, 100.0, 0.0 }, m = 0, B = {} })
    end
    HSMP_IPC.bus_put("spectate", { target = 0 })
    beat(); M.run(300)
    for _ = 1, 8 do beat(); M.run(250) end
    api.set_puppet(2, { actor = si, nick = "B", claimed_at = 0, gen = 0, addr = 7005, driving = true, in_range = true })
    cam.loc = { X = 5800, Y = 0, Z = 200 }   -- I am dead at the origin; the camera spectates the far fight
    for _ = 1, 3 do beat(); M.run(250) end
    T.check(api.puppets()[2].in_range == true and not T.contains(M.logtext(), "out of range"),
        "range from the camera, not from my corpse", M.logtext())
    cam.loc = { X = 1700, Y = 0, Z = 200 }   -- 4300 uu: inside the 10% hysteresis band
    for _ = 1, 3 do beat(); M.run(250) end
    T.check(api.puppets()[2].in_range == true, "no release inside the hysteresis band")
    cam.loc = { X = 0, Y = 0, Z = 200 }
    for _ = 1, 3 do beat(); M.run(250) end
    T.check(api.puppets()[2].in_range == false, "far from the camera: released")
    HSMP_IPC.bus_put("spectate", { target = 2, nick = "B", round = 1, alive = 1 })
    for _ = 1, 3 do beat(); M.run(250) end
    T.check(api.puppets()[2].in_range == true, "the spectated stand-in is always driven")
    T.check(not T.contains(M.logtext(), "LOOP ERR"), "no loop errors")
end

if opts.kind == "native_servo" then
    -- Native servo: the A/B switch (settings "native_servo"), the native body loop while it
    -- answers, the Lua loop on a refusal, a 5 s pause after a module-level refusal.
    -- (HSMPNative.servo_bodies is the mock's test hook here; the native side has its own tests.)
    T.write(sd .. "/.settings.json", '{"avatars":true,"native_servo":true}\n')
    local api = boot(true)
    local N = rawget(_G, "HSMPNative")
    local seen = {}
    local function ok_hook(a, out)
        seen[#seen + 1] = { mesh = a.mesh, leg_from = a.leg_from, holding = a.holding, gain = a.gain, leg_gain = a.leg_gain }
        out.c, out.v, out.dl, out.gl = { 1, 2, 3, 0, 0, 0, 1 }, { 4, 5, 6, 7, 8, 9 }, { 0.5 }, { 0.25 }
        return 22
    end
    N._servo = ok_hook
    local mesh = M.new_obj("SkeletalMeshComponent", "CharacterMesh0"); rawset(mesh, "__addr", 9001)
    local sv = { com = { { 0, 0, 0 } } }
    local PX = api.PX
    local out = PX.ns_bodies(mesh, {}, sv, 1 / 60, 900, 900, 0.8, nil, true)
    T.check(out and out.v[1] == 4 and seen[1] and seen[1].mesh == 9001 and seen[1].leg_from == 18 and seen[1].holding == true,
        "settings native_servo=true: one native call drives the body loop", T.repr(seen))
    local y = { gain = 0.15 }
    PX.ns_bodies(mesh, {}, sv, 1 / 60, 150, 150, 0.15, y, false)
    T.check(seen[2] and seen[2].leg_gain == 0.15, "yielding: legs use the yield gain too (same as the Lua loop)", T.repr(seen[2]))
    N._servo = function() return nil, "skip:call" end
    T.check(PX.ns_bodies(mesh, {}, sv, 1 / 60, 900, 900, 0.8, nil, false) == nil, "a refused frame -> nil: the Lua loop drives it")
    N._servo = ok_hook
    T.check(PX.ns_bodies(mesh, {}, sv, 1 / 60, 900, 900, 0.8, nil, false) ~= nil, "a per-frame refusal does not pause")
    N._servo = function() return nil, "disabled:/Script/Engine.X: Soft* param refused" end
    PX.ns_bodies(mesh, {}, sv, 1 / 60, 900, 900, 0.8, nil, false)
    N._servo = ok_hook
    local n0 = #seen
    T.check(PX.ns_bodies(mesh, {}, sv, 1 / 60, 900, 900, 0.8, nil, false) == nil and #seen == n0, "a module-level refusal pauses native attempts")
    M.run(5200)
    T.check(PX.ns_bodies(mesh, {}, sv, 1 / 60, 900, 900, 0.8, nil, false) ~= nil, "and retries after 5 s")
    T.write(sd .. "/.settings.json", '{"avatars":true,"native_servo":false}\n')
    M.run(1500)
    n0 = #seen
    T.check(PX.ns_bodies(mesh, {}, sv, 1 / 60, 900, 900, 0.8, nil, false) == nil and #seen == n0, "settings native_servo=false: Lua loop only")
    local log = M.logtext()
    T.check(T.count(log, "native servo ON") == 1 and T.count(log, "native servo off") == 1, "both switches logged once", log:sub(1, 300))
    T.check(T.count(log, "native servo refused (disabled:") == 1, "the refusal is logged once", log:sub(1, 300))
end

if opts.kind == "native_neutralise" then
    -- Native neutralise: the A/B switch (settings "native_neutralise", separate from native_servo), the native
    -- variable writes while they answer, the tonus originals kept by Lua, the Lua path on a refusal.
    T.write(sd .. "/.settings.json", '{"avatars":true,"native_neutralise":true,"native_servo":false}\n')
    local api = boot(true)
    local N = rawget(_G, "HSMPNative")
    local seen = {}
    N._neutralise = function(a)
        seen[#seen + 1] = { actor = a.actor, tonus = a.tonus, motors_off = a.motors_off, mesh = a.mesh }
        return 30
    end
    local actor = M.new_obj("Willie_BP_C", "Willie_BP_C_5"); rawset(actor, "__addr", 7005)
    actor.__props["All Body Tonus"] = 100
    actor.__props["Muscle Power"] = 35
    local mesh = M.new_obj("SkeletalMeshComponent", "CharacterMesh0"); rawset(mesh, "__addr", 9001)
    local p, body = { actor = actor }, { mesh = mesh }
    local PX = api.PX
    T.check(PX.nn(p, body) == true and seen[1] and seen[1].actor == 7005 and seen[1].tonus == true and seen[1].motors_off == true
        and seen[1].mesh == 9001, "settings native_neutralise=true: one native call", T.repr(seen))
    T.check(p.tonus0 and p.tonus0["All Body Tonus"] == 100 and p.tonus0["Muscle Power"] == 35,
        "the tonus originals are read once by Lua (restored on release)", T.repr(p.tonus0))
    T.check(actor.__props["All Body Tonus"] == 100, "the native path does the writes (not Lua)")
    T.check(PX.ns_bodies(mesh, {}, { com = {} }, 1 / 60, 900, 900, 0.8, nil, false) == nil, "G2a stays off: its own switch")
    N._neutralise = function() return nil, "skip:actor" end
    T.check(PX.nn(p, body) == false, "a refused frame -> false: neutralise_bp writes from Lua")
    T.write(sd .. "/.settings.json", '{"avatars":true,"native_neutralise":false}\n')
    N._neutralise = function(a) seen[#seen + 1] = a; return 30 end
    M.run(1500)
    local n0 = #seen
    T.check(PX.nn(p, body) == false and #seen == n0, "settings native_neutralise=false: Lua path only")
    local log = M.logtext()
    T.check(T.count(log, "native neutralise ON") == 1 and T.count(log, "native neutralise off") == 1, "both switches logged", log:sub(1, 300))
end

if opts.kind == "native_wservo" then
    -- Native weapon servo: its own A/B switch; the validated weapon entry's addresses go to the native
    -- call (no extra reflected call); an entry without a root address stays on the Lua path.
    T.write(sd .. "/.settings.json", '{"avatars":true,"native_wservo":true,"native_servo":false}\n')
    local api = boot(true)
    local N = rawget(_G, "HSMPNative")
    local seen = {}
    N._servo = function() return 22 end
    N._servo_weapon = function(a, out)
        seen[#seen + 1] = { actor = a.actor, root = a.root, gain = a.gain, holding = a.holding, n = #a.aim }
        out.x, out.v = { 1, 2, 3, 0, 0, 0, 1 }, { 4, 5, 6 }
        return true
    end
    local PX = api.PX
    local c = { addr = 8001, root_addr = 8002, com = { 0, 0, 0 } }
    local tg = { 1, 2, 3, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0 }
    local out = PX.nw_weapon(c, tg, 1 / 60, false)
    T.check(out and out.x[1] == 1 and seen[1] and seen[1].actor == 8001 and seen[1].root == 8002 and seen[1].n == 13,
        "settings native_wservo=true: the validated entry's addresses go native", T.repr(seen))
    T.check(PX.nw_weapon({ addr = 8001, com = { 0, 0, 0 } }, tg, 1 / 60, false) == nil, "no root address: Lua path")
    T.check(PX.ns_bodies(M.new_obj("SkeletalMeshComponent", "M"), {}, { com = {} }, 1 / 60, 900, 900, 0.8, nil, false) == nil,
        "G2a stays off: its own switch")
    N._servo_weapon = function() return nil, "skip:root" end
    T.check(PX.nw_weapon(c, tg, 1 / 60, true) == nil, "a refused frame -> nil: the Lua servo")
    T.check(T.count(M.logtext(), "native weapon servo ON") == 1, "switch logged")
end

if opts.kind == "wpn_status" then
    -- The stand-in weapon status string is cached on the weapon entry (refreshed at most once a
    -- second; a new entry = a new weapon starts fresh; a freed weapon is never touched).
    local api = boot(true)
    local calls = 0
    M.Methods.GetClass = function(self) calls = calls + 1; return M.new_obj("Class", "ModularWeaponBP_ArmingSword_C") end
    M.Methods.GetAttachParent = function() return nil end
    local wa = M.new_obj("ModularWeaponBP_ArmingSword_C", "W_1")
    local c = { root = M.new_obj("StaticMeshComponent", "W_1_root") }
    local PX = api.PX
    local s1 = PX.wpn_status(c, wa, 1000)
    local s2 = PX.wpn_status(c, wa, 1500)
    T.check(s1 == s2 and calls == 1, "cached within a second: one GetClass for two frames", T.repr({ s1, s2, calls }))
    PX.wpn_status(c, wa, 2100)
    T.check(calls == 2, "refreshed after a second", calls)
    local c2 = { root = M.new_obj("StaticMeshComponent", "W_2_root") }
    rawset(wa, "__dead", true)   -- the old weapon is freed (strict mock: touching it fails the suite)
    local w2 = M.new_obj("ModularWeaponBP_ArmingSword_C", "W_2")
    PX.wpn_status(c2, w2, 2200)
    T.check(calls == 3, "a new entry (weapon change) reads the new weapon at once", calls)
    T.check(PX.wpn_status(nil, nil, 2300) == "-", "no weapon: no call")
    T.check(#M.dead_touch == 0, "the freed weapon was never touched", T.repr(M.dead_touch))
end

if opts.kind == "skeleton_ref" then
    -- Parent offsets measured on a stretched stand-in are replaced by the reference
    -- skeleton (scaled to the character); the stretch measure sees a pulled joint.
    local api = boot(true)
    local P = api.PURE
    local k = 1.06
    local loc = {}
    for i = 2, #P.V2_REF_T do local r = P.V2_REF_T[i]; loc[i] = { r[1] * k, r[2] * k, r[3] * k } end
    local out, fixed, ks = P.ref_loc(loc)
    T.check(fixed == 0 and math.abs(ks - k) < 1e-6, "an intact body keeps its own offsets", T.repr({ fixed, ks }))
    loc[17] = { 60, 0, 0 }   -- hand_r 60 uu from the forearm (reference 27.25 x k)
    loc[19] = nil            -- calf_l not measured
    out, fixed = P.ref_loc(loc)
    T.check(fixed == 2 and math.abs(P.len3(out[17]) - 27.25 * k) < 1e-3 and out[19] ~= nil,
        "a stretched and a missing offset come from the reference", T.repr({ fixed, out[17], out[19] }))
    local neck = P.V2_REF_T[7]
    loc[7] = { P.len3(neck) * k, 0, 0 }
    out, fixed = P.ref_loc(loc)
    T.check(fixed == 3 and P.d3(out[7], { neck[1] * k, neck[2] * k, neck[3] * k }) < 1e-6,
        "an equal-length neck displaced sideways is not cached as the reference joint", T.repr(out[7]))
    -- a pose built from the reference, then one joint pulled 12 uu
    local pos, len = { { 0, 0, 100 } }, {}
    for i = 2, #P.V2_REF_T do
        local par, r = pos[P.V2_PARENT[i]], P.V2_REF_T[i]
        pos[i] = { par[1] + r[1], par[2] + r[2], par[3] + r[3] }
        len[i] = P.len3(r)
    end
    local d0 = P.stretch(pos, len)
    T.check(d0 < 1e-6, "no stretch on the reference pose", d0)
    pos[13] = { pos[13][1] + 12, pos[13][2], pos[13][3] }   -- hand_l pulled off the forearm
    local d, b = P.stretch(pos, len)
    T.check(d > 10 and (b == 13 or b == 12), "a pulled joint is measured", T.repr({ d, b }))
end

if opts.kind == "body_scale" then
    local api = boot(true)
    local n = HSMPNative
    local scale, calls, asset_rebuilds = { X=1, Y=1, Z=1 }, {}, 0
    local mesh = {
        K2_GetComponentScale = function() return scale end,
        SetSimulatePhysics = function(_, on) calls[#calls+1] = on and "on" or "off" end,
        SetWorldScale3D = function(_, s) scale = s; calls[#calls+1] = "scale" end,
        SetPhysicsAsset = function() asset_rebuilds = asset_rebuilds + 1 end,
    }
    local p = { nick="peer", aim={}, shown={}, qhist={}, idlew={}, qfoot={} }
    local body = { mesh=mesh, sv={} }
    local pose = { has_context=true,match_id=10,round=1,life=1 }
    n.sc_put("peer_body2", { version=1, match_id=10,round=1,life=1,pawn="Owner",native_height=.5,actor_scale={1,1,1},
        height_rate=1, muscle_rate=0, mass_scale_bp=1,
        char_scale={ 1.1, 0.95, 1.2 }, rows={} }, 2)
    HSMP_IPC.peer_dir(true)
    local stale_pose = { has_context=true,match_id=10,round=1,life=2 }
    T.check(not api.PX.sync_body_scale(2,p,body,999,stale_pose) and #calls == 0,
        "an old body generation cannot resize the fresh native life")
    T.check(not api.PX.sync_body_scale(2,p,body,999) and #calls == 0,
        "no body geometry is applied before an authenticated pose is displayed")
    T.check(api.PX.sync_body_scale(2, p, body, 1000, pose), "owner mesh scale starts a coordinated geometry reset")
    T.check(T.eq(calls, { "off", "scale" }) and body.repose and body.scale_remeasure,
        "physics stops before resizing, then geometry is marked for measurement", T.repr(calls))
    T.check(asset_rebuilds == 0,
        "scale sync retains native bodies and their active physics-control bindings")
    T.check(body.sv == nil and p.aim == nil and p.shown == nil and p.qhist == nil,
        "old COM/joint offsets and prior pose aims are discarded after resizing")
    T.check(math.abs(scale.X-1.1)<1e-6 and math.abs(scale.Y-0.95)<1e-6 and math.abs(scale.Z-1.2)<1e-6,
        "all three owner scale axes are preserved")
    T.check(not api.PX.sync_body_scale(2, p, body, 1017, pose) and #calls == 2,
        "unchanged geometry does not reset physics every frame")
    n.sc_put("peer_body2", { version=2, match_id=10,round=1,life=1,pawn="Owner",native_height=.5,actor_scale={1,1,1},
        height_rate=1, muscle_rate=0, mass_scale_bp=1,
        char_scale={ 1, 1, 1 }, rows={} }, 2)
    body.sv, p.aim = {}, {}
    T.check(api.PX.sync_body_scale(2, p, body, 2000, pose) and body.sv == nil and p.aim == nil and #calls == 4,
        "a later owner geometry change invalidates a running servo too")
end

if opts.kind=="hand_pipeline"or opts.kind=="limb_burst"or opts.kind=="joint_profile"then
    T.write(sd.."/.settings.json",'{"avatars":true,"native_neutralise":false,"native_servo":false}\n')
    local api=boot(true);local PX,P=api.PX,api.PURE
    if opts.gate then
        if opts.kind=="joint_profile"then
            T.check(not PX.JOINT_PROFILE and PX.joint_profile_row(nil)==nil,"joint profile requires both opt-in flags before any optional reads")
            return
        end
        if opts.kind=="limb_burst"then
            T.check(not PX.LIMB_BURST and PX.limb_burst_row(nil)==nil,"limb burst requires developer mode and explicit flag")
            return
        end
        T.check(not PX.HAND_PIPELINE_PROBE and PX.hand_pipeline_begin(nil,nil,nil,nil)==nil and PX.hand_pipeline_state==nil,
            "hand pipeline requires both developer mode and explicit opt-in before optional reads")
        return
    end
    sidecar({{1,"Own"},{2,"Peer"}})
    HSMPNative.sc_put("session",{seq=1,match_id=419,round=1,phase=3})
    HSMPNative.sc_put("mode",{seq=1,match_id=419,round=1,rows={{peer_id=2,life=2}}})
    api.on_tick()
    local actor=M.new_obj("Willie_BP_C","HAND_PROXY");rawset(actor,"__addr",9901)
    local mesh=M.new_obj("SkeletalMeshComponent","HAND_MESH");rawset(mesh,"__addr",9902)
    actor.__props.Mesh,mesh.__props.Owner=mesh,actor
    M.Methods.GetOwner=function(o)return o.__props.Owner end
    M.Methods.GetWorld=function(o)
        assert(rawget(o,"__cls")~="SkeletalMeshComponent","pinned Lua GetWorld is actor-only")
        return o.__props.NativeWorld or M.world
    end
    local sockets,sim_reads,velocity_reads,writes=0,0,0,0
    local malformed,change_life,change_phase=false,false,false
    M.Methods.GetSocketTransform=function(_,bone,space)
        sockets=sockets+1;assert(space==0,"existing driver uses native World space")
        return {Translation={X=7,Y=0,Z=0},Rotation={X=0,Y=0,Z=0,W=1}}
    end
    M.Methods.SetPhysicsLinearVelocity=function()writes=writes+1 end
    M.Methods.SetPhysicsAngularVelocityInDegrees=function()writes=writes+1 end
    M.Methods.IsSimulatingPhysics=function(_,bone)
        sim_reads=sim_reads+1
        if malformed then return "false"end
        local name=type(bone)=="string"and bone or bone:ToString()
        return name~="hand_l"
    end
    M.Methods.GetPhysicsAngularVelocityInDegrees=function(_,bone)
        velocity_reads=velocity_reads+1
        if change_life then HSMPNative.sc_put("mode",{seq=2,match_id=419,round=1,rows={{peer_id=2,life=3}}})end
        if change_phase then
            HSMPNative.sc_put("session",{seq=9,match_id=419,round=1,phase=3})
            HSMPNative.sc_put("mode",{seq=9,match_id=419,round=1,rows={{peer_id=2,life=1}}})
        end
        return {X=malformed and 0/0 or 12,Y=34,Z=56}
    end
    local slots,sv={}, {n=6,com={},cmd={},wc={},err={n=0,e=0,emax=0,a=0,amax=0,hmax=0,capped=0}}
    for _,i in ipairs(PX.HAND_PIPELINE_SLOTS)do
        local h=(i==13 or i==17)and math.pi/4 or 0
        slots[i]={0,0,0,0,0,math.sin(h),math.cos(h),20,0,0,0,0,0};sv.com[i]={0,0,0}
    end
    local cur={v2=true,has_context=true,match_id=419,round=1,life=2,seq=0,pt=1000,read_at=1000,
        cut=7,age=0,delay=0,rate=1,st=16,mode="interp",slots=slots,weapons={},nbones=6}
    local body={mesh=mesh,mesh_addr=9902,mesh_fname="HAND_MESH",field="Mesh",gravity=false,ctl="servo",motors={},sv=sv}
    local p={actor=actor,addr=9901,body=body,gen=api.generation(),peer=2,driving=true,last=cur,
        shown={has_context=true,pawn="HAND_PROXY",match_id=419,round=1,life=2,cut=7}}
    local records={};local emit=PX.hand_pipeline_emit
    PX.hand_pipeline_emit=function(r,why,q)records[#records+1]=r or {unavailable=why};emit(r,why,q)end
    local function drive(now)
        M.now=now;cur.seq=cur.seq+1;cur.pt,cur.read_at=now,now;PX.frame_no=(PX.frame_no or 0)+1
        cur.slots[13][1]=cur.seq-1
        api.drive_v2(2,p,body,cur,true,now,false,false)
    end
    if opts.kind=="joint_profile"then
        local own=M.new_obj("Willie_BP_C","PROFILE_OWN");rawset(own,"__addr",9801)
        local ownmesh=M.new_obj("SkeletalMeshComponent","PROFILE_OWN_MESH");rawset(ownmesh,"__addr",9802)
        own.__props.Mesh=ownmesh;ownmesh.__props.Owner=own;M.pc.__props.Pawn=own
        HSMPNative.sc_put("session",{seq=2,match_id=419,round=0,spawn_round=1,phase=1,
            rows={{peer_id=1,seat=0,connected=true,spawn_id=257,spawn_pos={0,0,0}},
                {peer_id=2,seat=1,connected=true,spawn_id=258,spawn_pos={0,0,0}}}})
        HSMPNative.sc_put("mode",{seq=2,match_id=419,round=0,rows={{peer_id=1,life=0},{peer_id=2,life=0}}})
        HSMP_IPC.bus_put("spawn_status",{verified=true,pawn="PROFILE_OWN",match_id=419,round=1,life=1,spawn_id=257})
        local sample_life,sample_age=1,0.375
        HSMP_IPC.sample_status=function()return {pose={tick=7,ts=M.now-sample_age,match_id=419,round=1,life=sample_life}}end
        local captures=0
        local source_current,proxy_current
        if opts.change=="fault_wait"then PX.JOINT_PROFILE.trigger="fault"end
        local lookup_changed,old_touches=false,0
        if opts.change=="lookup_pc_world"then
            local find=StaticFindObject;local world_get=M.Methods.GetWorld;local address_get=M.Methods.GetAddress
            StaticFindObject=function(path)
                if path=="/Script/Engine.Default__ConstraintInstanceBlueprintLibrary"then
                    lookup_changed=true;M.pc.__props.NativeWorld=M.new_obj("World","PROFILE_LOOKUP_NEW_WORLD")
                    rawset(M.pc.__props.NativeWorld,"__addr",19991)
                end
                return find(path)
            end
            M.Methods.GetWorld=function(o)if lookup_changed and o==actor then old_touches=old_touches+1 end;return world_get(o)end
            M.Methods.GetAddress=function(o)if lookup_changed and (o==actor or o==mesh)then old_touches=old_touches+1 end;return address_get(o)end
        end
        PX.JOINT_PROFILE.capture=function(self,source,proxy,se,pe)
            captures=captures+1
            source_current,proxy_current=se.current,pe.current
            T.check(source.peer==1 and proxy.peer==2 and source.life==1 and proxy.life==2 and source.pending and not source.qualification,
                "production scopes preserve independent peer/life and actual Mode0 pending source publication")
            T.check(source.sample_ms%1~=0 and source.admission_sample_age_ms==.375 and se.current()and pe.current(),
                "production accepts fresh fractional successful source timestamp with exact current guards")
            self.used=true
            if opts.change=="refusal"then
                return nil,"actual native\nread failed\t"..string.rep("q",140),
                    {stage="source:asset_before",elapsed_available=true,capture_elapsed_ms=2.75}
            end
            if opts.change=="predicate"then
                sample_age=251.125
                local valid,failure=se.current()
                T.check(valid==false and failure.validator=="source"and failure.predicate=="source_sample_age"
                    and failure.field=="sample_age_ms"and failure.observed==251.125,
                    "actual source validator reports its existing expired sample age without changing freshness")
                sample_age=.375
                HSMPNative.sc_put("mode",{seq=3,match_id=419,round=0,rows={{peer_id=1,life=0},{peer_id=2,life=0}}})
                valid,failure=pe.current()
                T.check(valid==false and failure.validator=="proxy"and failure.predicate=="audit_changed"
                    and failure.field=="mode_seq"and failure.expected==2 and failure.observed==3,
                    "actual proxy validator attributes same-life audit sequence publication exactly")
                HSMPNative.sc_put("mode",{seq=3,match_id=419,round=1,rows={{peer_id=1,life=1},{peer_id=2,life=2}}})
                valid,failure=se.current()
                T.check(valid==false and failure.predicate=="context_changed"and failure.field=="qualification"
                    and failure.expected_available and failure.expected==false and failure.observed==true,
                    "actual pending qualification transition reports typed false-to-true without inventing a new life")
                return nil,"scope changed",{stage="proxy:asset_after",elapsed_available=true,capture_elapsed_ms=3.5,first_failure=failure}
            end
            if opts.change=="pc_world"then
                M.pc.__props.NativeWorld=M.new_obj("World","PROFILE_NEW_WORLD");rawset(M.pc.__props.NativeWorld,"__addr",19991)
            end
            return {source=source,proxy=proxy}
        end
        drive(1000)
        T.check(captures==0,"first drive has no fabricated previously applied pose for pairing")
        local before=writes;drive(1016)
        if opts.change=="fault_wait"then
            T.check(captures==0 and PX.JOINT_PROFILE.attempts==0 and writes==before+12,
                "actual main waits for a completed persistent wrist fault without consuming attempts or changing servo writes")
            return
        end
        if opts.change=="lookup_pc_world"then
            T.check(lookup_changed and captures==0 and writes==before and old_touches==0,
                "library lookup travel checks fresh PC world before any old proxy getter or physical writer")
            return
        end
        if opts.change=="pc_world"then
            T.check(captures==1 and writes==before,"actual drive preserves false independent writer recheck after optional PC-world change")
            return
        end
        T.check(captures==1 and writes==before+12,"actual admitted pair does not change normal servo writes")
        if opts.change=="predicate"then
            local log=M.logtext()
            T.check(T.contains(log,"JOINTPROFILE predicate inst=")and T.contains(log,'"validator":"source"')
                and T.contains(log,'"predicate":"context_changed"')and T.contains(log,'"expected":false'),
                "actual refusal logs its first source predicate separately from the active proxy read stage")
            drive(1032);T.check(captures==1,"predicate diagnostics preserve once-only capture and normal writer")
            return
        end
        if opts.change=="refusal"then
            local log=M.logtext()
            T.check(T.contains(log,"stage=source:asset_before capture_elapsed_ms=2.75 reason=actual native read failed "),
                "actual production pcall preserves sanitized second-return reason and copied stage/time")
            T.check(not T.contains(log,"scope_or_capture_unavailable"),"specific capture refusal is not replaced by the old generic reason")
            drive(1032);T.check(captures==1,"refusal preserves once-only capture and normal writer without reopening")
            return
        end
        if opts.change=="cached_reader"then
            local holders=PX.joint_profile_readers
            local sr,pr=holders.source.reader,holders.proxy.reader
            local reads_source,reads_proxy=sr.reads,pr.reads
            T.check(source_current()and proxy_current()and holders.source.reader==sr and holders.proxy.reader==pr
                and sr~=pr and sr.reads>reads_source and pr.reads>reads_proxy,
                "actual scope guards reuse separate allocations while force-polling both current links")
            local header=HSMP_IPC.N.ipc_info
            HSMP_IPC.N.ipc_info=function()return nil end
            T.check(not source_current()and not proxy_current(),
                "cached successful readers cannot borrow an old header after a fresh native read fails")
            HSMP_IPC.N.ipc_info=header
            HSMPNative._st.hb_age=6
            T.check(not source_current()and not proxy_current(),
                "both cached readers reject a newly expired heartbeat without refreshing the source timestamp")
            HSMPNative._st.hb_age=.05
            HSMPNative.sc_put("link",{status=7,state=1,my_peer_id=1})
            T.check(not source_current()and not proxy_current(),
                "forced polling rejects a newly ended link after cached success")
            HSMPNative.sc_put("link",{status=1,state=1,my_peer_id=2})
            T.check(not source_current(),
                "fresh source peer and spawn assignment cannot inherit the cached reader's previous peer")
            HSMPNative.sc_put("link",{status=1,state=1,my_peer_id=1})
            T.check(source_current()and proxy_current()and holders.source.reader==sr and holders.proxy.reader==pr,
                "restored exact fresh scope succeeds with the same two allocations")
            return
        end
        drive(1032);T.check(captures==1,"one whole-run capture adds no optional repeat")
        HSMP_IPC.bus_put("spawn_status",{verified=true,pawn="PROFILE_OWN",match_id=419,round=1,life=1,spawn_id=259})
        T.check(PX.joint_profile_source_scope()==nil,"old verified assignment cannot acquire a new current session spawn ID")
        HSMP_IPC.bus_put("spawn_status",{verified=true,pawn="PROFILE_OWN",match_id=419,round=1,life=1,spawn_id=257})
        sample_life=2;M.now=1048
        T.check(PX.joint_profile_source_scope()==nil,"successful sample tuple disagreement with verified spawn is unavailable")
        sample_life=1;sample_age=251
        T.check(PX.joint_profile_source_scope()==nil,"stale successful native source publication cannot be relabeled current")
        sample_age=.375
        HSMPNative.sc_put("session",{seq=3,match_id=419,round=1,phase=3,
            rows={{peer_id=1,seat=0,connected=true,spawn_id=257,spawn_pos={0,0,0}},
                {peer_id=2,seat=1,connected=true,spawn_id=258,spawn_pos={0,0,0}}}})
        HSMPNative.sc_put("mode",{seq=3,match_id=419,round=1,rows={{peer_id=1,life=2},{peer_id=2,life=2}}})
        T.check(PX.joint_profile_source_scope()==nil,"Live source Mode life mismatch remains strict")
        return
    end
    if opts.kind=="limb_burst"then
        cur.slots[17][6],cur.slots[17][7]=0,1 -- only the measured left hand is faulted.
        M.Methods.GetSocketTransform=function(_,bone,space)
            sockets=sockets+1;assert(space==0);return {Translation={X=0,Y=0,Z=0},Rotation={X=0,Y=0,Z=0,W=1}}
        end
        local pa=M.new_obj("PhysicalAnimationComponent","OLD_PA");rawset(pa,"__addr",9941)
        pa.__props.Owner=actor;pa.__props.SkeletalMeshComponent=mesh;pa.__props.StrengthMultiplyer=0
        local fresh=M.new_obj("PhysicalAnimationComponent","FRESH_PA");rawset(fresh,"__addr",9942)
        fresh.__props.Owner=actor;fresh.__props.SkeletalMeshComponent=mesh;fresh.__props.StrengthMultiplyer=2
        body.motors={pa};body.motor_ids={{address=9941,name="OLD_PA"}}
        actor.__props.PhysicalAnimation=fresh
        actor.__props["Phys Anim Array"]={fresh,ForEach=function(_,f)f(nil,{get=function()return fresh end})end}
        actor.__props.DriverSkeleton=M.new_obj("SkeletalMeshComponent","DRIVER_MESH")
        actor.__props.DriverSkeleton.__props.Owner=actor
        M.Methods.SetStrengthMultiplyer=function(o,v)o.__props.StrengthMultiplyer=v end
        local limb_records={}
        PX.LIMB_BURST=PX.LIMB_BURST_MODULE.new(function(r)limb_records[#limb_records+1]=r end)
        drive(1000);drive(1016)
        T.check(#limb_records==0 and p.settle_state.settle_rot_deg>10,"existing actual driver error triggers diagnostic without onset native reads")
        local before=writes
        if opts.change=="logging"then PX.LIMB_BURST=PX.LIMB_BURST_MODULE.new(function()error("log unavailable")end)end
        if opts.change=="admission_mesh"or opts.change=="admission_world"then
            local info=HSMP_IPC.N.ipc_info
            HSMP_IPC.N.ipc_info=function()
                if opts.change=="admission_mesh"then
                    local replacement=M.new_obj("SkeletalMeshComponent","EARLY_REPLACED_MESH");rawset(replacement,"__addr",9990)
                    replacement.__props.Owner=actor;actor.__props.Mesh=replacement
                else actor.__props.NativeWorld=M.new_obj("World","EARLY_FOREIGN_WORLD");rawset(actor.__props.NativeWorld,"__addr",9991)end
                return info()
            end
        elseif opts.change=="mode_unavailable"then HSMPNative._rec.slots.mode=nil end
        local original_velocity=M.Methods.GetPhysicsAngularVelocityInDegrees
        local changed=false
        local old_touches=0
        local native_world=M.Methods.GetWorld
        if opts.change=="pc_world"then
            M.Methods.GetWorld=function(o)
                if changed and o==actor then old_touches=old_touches+1 end
                return native_world(o)
            end
            local socket=M.Methods.GetSocketTransform
            M.Methods.GetSocketTransform=function(...)
                if changed then old_touches=old_touches+1 end
                return socket(...)
            end
        end
        M.Methods.GetPhysicsAngularVelocityInDegrees=function(...)
            if not changed and opts.change=="mesh"then
                changed=true;local replacement=M.new_obj("SkeletalMeshComponent","REPLACED_MESH");rawset(replacement,"__addr",9990)
                replacement.__props.Owner=actor;actor.__props.Mesh=replacement
            elseif not changed and opts.change=="world"then
                changed=true;actor.__props.NativeWorld=M.new_obj("World","FOREIGN_WORLD");rawset(actor.__props.NativeWorld,"__addr",9991)
            elseif not changed and opts.change=="pc_world"then
                changed=true;M.pc.__props.NativeWorld=M.new_obj("World","NEW_PC_WORLD");rawset(M.pc.__props.NativeWorld,"__addr",9991)
            end
            return original_velocity(...)
        end
        drive(1032)
        if opts.change=="pc_world"then
            T.check(writes==before and #limb_records==0 and old_touches==0,
                "changed PC world with valid old actor is rejected before another old-body read or writer")
            return
        end
        if opts.change=="admission_mesh"or opts.change=="admission_world"then
            T.check(writes==before and #limb_records==0,"early admission "..opts.change.." replacement aborts old-body writer despite unavailable audit")
            return
        elseif opts.change=="mode_unavailable"then
            T.check(writes==before+12 and #limb_records==0,"unavailable diagnostic Mode does not introduce a new normal physical eligibility gate")
            return
        end
        if opts.change=="mesh"or opts.change=="world"then
            T.check(writes==before and PX.LIMB_BURST.closed and #limb_records==0,
                "actual "..opts.change.." replacement during optional read aborts existing driver before stale writes")
            return
        elseif opts.change=="logging"then
            T.check(writes==before+12,"diagnostic emit exception remains contained and normal driver continues under fresh scope")
            return
        end
        local r=limb_records[1]
        T.check(r and r.phase=="pre_driver"and r.params.dt_s==0.0167 and r.params.gain==0.3 and r.params.cap_ang==900
            and r.params.holding==false and r.audit.mode_life==2,"actual production capture copies computed driver parameters and raw current Mode",T.repr(r))
        T.check(#limb_records==2 and limb_records[2].phase=="post_driver"and limb_records[2].params.native_servo==false,
            "existing Lua setter boundary creates two ordered actual phase records")
        if not r then return end
        T.check(r.physical_animation.available and r.physical_animation.value.entries[1].cached==false
            and r.physical_animation.value.entries[1].binding.value.strength==2,
            "fresh uncached PA remains visibly nonzero in read-only evidence")
        api.set_driven(p)
        local hook=M.hooks["/Game/Character/Blueprints/Willie_BP.Willie_BP_C:ReceiveTick"]
        hook.callback({get=function()return actor end},{get=function()return 0.024 end})
        T.check(#limb_records==4 and limb_records[3].phase=="bp_post"and limb_records[4].phase=="policy_post"
            and limb_records[3].params.receive_tick_delta_s==0.024 and limb_records[3].params.receive_tick_delta_available,
            "actual registered Blueprint POST and existing policy capture validated native DeltaSeconds separately")
        T.check(limb_records[4].physical_animation.value.entries[1].binding.value.strength==2
            and pa.__props.StrengthMultiplyer==0,"diagnostic never repairs a fresh component absent from original policy cache")
        drive(1048);hook.callback({get=function()return actor end},0.017)
        drive(1064);hook.callback({get=function()return actor end},nil)
        T.check(#limb_records==12 and limb_records[9].source_seq>limb_records[1].source_seq
            and not limb_records[11].params.receive_tick_delta_available,"three real driver frames preserve advancing source seq and unavailable callback delta")
        local count=velocity_reads;drive(1080);hook.callback({get=function()return actor end},0.017)
        T.check(#limb_records==12 and velocity_reads==count,"fourth drive/BP callback perform no optional native reads")
        return
    end
    drive(1000)
    local r=records[1]
    T.check(r and r.source_seq==1 and r.source_cut==7 and r.life==2 and r.mesh.address==9902 and r.pawn.name=="HAND_PROXY",
        "actual production drive captures full source and fresh body identity",T.repr(r))
    T.check(r and r.decoded[13][1]==0 and r.aim[13][1]>0 and r.current[13][1]==7 and r.current_source=="lua_socket_world",
        "production stages distinguish decoded input, advanced final aim and existing native World socket return")
    T.check(sockets==6 and sim_reads==4 and velocity_reads==4 and writes==12,
        "admitted drive adds only four read-only per-bone readbacks and no extra socket getters or physics writes")
    T.check(r and not r.prior_available and r.readback[13].simulating==false and r.readback[13].angular_velocity[3]==56,
        "first frame has no invented prior aim and preserves actual false simulation plus post-driver velocity")
    local log=M.logtext();local row=log:match("HANDPIPE inst=7[^\n]*slot=13[^\n]*") or ""
    T.check(T.contains(log,"current_phase=pre_driver_returned readback_phase=post_driver")
        and T.contains(row,"parent_slot=12 parent_bone=lowerarm_l") and T.contains(row,"sim_available=true simulating=false")
        and T.contains(row,"decoded_relative=(0.0,0.0,0.707"),
        "namespaced rows identify stages and parent-relative quaternion with false simulation available",row)
    drive(1016)
    T.check(#records==1 and sim_reads==4 and velocity_reads==4 and sockets==12 and writes==24,
        "per-life cadence refuses optional readbacks while the normal driver continues")
    drive(6000);r=records[2]
    T.check(r and r.prior_available and r.prior_seq==2 and r.prior_frame==2 and r.prior[13][1]~=r.aim[13][1],
        "next admitted actual drive copies the real previous aim with its own source and frame")
    local native={c={},v={},dl={},gl={}}
    for _,i in ipairs(PX.HAND_PIPELINE_SLOTS)do
        local b=(i-1)*7;for j,v in ipairs({9,0,0,0,0,0,1})do native.c[b+j]=v end
        b=(i-1)*3;native.v[b+1],native.v[b+2],native.v[b+3]=0,0,0;native.dl[i],native.gl[i]=0,0
    end
    PX.ns_bodies=function()return native end
    drive(11000);r=records[3]
    T.check(r and r.current[13][1]==9 and r.current_source=="native_servo_socket_world" and sockets==18 and writes==36,
        "actual native servo branch copies returned c7 without another getter or Lua physical write")
    native.c[(13-1)*7+1]=999
    T.check(r.current[13][1]==9,"capture owns scalar copies instead of retaining native result buffers")
    local function plain(v)
        if type(v)~="table"then return type(v)=="number"or type(v)=="string"or type(v)=="boolean"or v==nil end
        if getmetatable(v)~=nil then return false end
        for k,x in pairs(v)do if not plain(k)or not plain(x)then return false end end
        return true
    end
    T.check(plain(r)and plain(PX.hand_pipeline_state),"published records and persistent cadence state contain only plain scalar copies")
    PX.hand_pipeline_state=nil;M.now=12000
    local q=PX.hand_pipeline_begin(p,body,cur,p.aim.slots,11000,11016,12000)
    local decoded,aim,prior=q.record.decoded[13][1],q.record.aim[13][1],q.record.prior[13][1]
    cur.slots[13][1],p.aim.slots[13][1]=777,888
    local c={1,2,3,0,0,0,1};PX.hand_pipeline_observe(q,13,c,false);c[1]=555
    T.check(q.record.decoded[13][1]==decoded and q.record.aim[13][1]==aim and q.record.prior[13][1]==prior and q.record.current[13][1]==1,
        "all decoded, final, prior and current arrays are copied before mutable playback storage changes")
    cur.slots[13][1],p.aim.slots[13][1]=decoded,aim
    local before=writes
    T.check(PX.hand_pipeline_finish(q)~=nil and writes==before,"standalone finish makes no physical writes")
    local count=velocity_reads;cur.seq=cur.seq+1
    T.check(PX.hand_pipeline_finish(q)==nil and velocity_reads==count,"changed source sequence refuses readback before optional native calls")
    cur.seq=cur.seq-1;cur.cut=8
    T.check(PX.hand_pipeline_finish(q)==nil and velocity_reads==count,"source cut changes cannot inherit admitted capture identity")
    cur.cut=7;mesh.__props.Owner=M.pc
    T.check(PX.hand_pipeline_finish(q)==nil and velocity_reads==count,"same-world wrong mesh owner fails before optional reads")
    mesh.__props.Owner=actor;actor.__props.NativeWorld=M.new_obj("World","OTHER_WORLD")
    rawset(actor.__props.NativeWorld,"__addr",9910)
    T.check(PX.hand_pipeline_finish(q)==nil and velocity_reads==count,"changed actual pawn world refuses native readback despite matching retained life")
    actor.__props.NativeWorld=nil;HSMPNative.sc_put("link",{status=0,state=0,my_peer_id=1})
    T.check(PX.hand_pipeline_finish(q)==nil and velocity_reads==count,"retained positive life cannot qualify a disconnected capture")
    sidecar({{1,"Own"},{2,"Peer"}});change_life=true
    local sample,why=PX.hand_pipeline_finish(q)
    T.check(sample==nil and why=="scope changed" and velocity_reads==count+1 and writes==before,
        "life change during optional read stops the bounded capture before publishing qualified data")
    change_life=false;HSMPNative.sc_put("mode",{seq=3,match_id=419,round=1,rows={{peer_id=2,life=2}}})
    local replacement=M.new_obj("SkeletalMeshComponent","REPLACED_HAND_MESH");rawset(replacement,"__addr",9903)
    replacement.__props.Owner=actor;actor.__props.Mesh=replacement;rawset(mesh,"__dead",true)
    count=velocity_reads
    T.check(PX.hand_pipeline_finish(q)==nil and velocity_reads==count and #M.dead_touch==0,
        "freed retained mesh is never dereferenced after fresh field replacement",T.repr(M.dead_touch))
    rawset(mesh,"__dead",false);actor.__props.Mesh=mesh
    PX.hand_pipeline_state=nil;cur.slots[13][4]=0/0;M.now=13000
    q=PX.hand_pipeline_begin(p,body,cur,p.aim.slots,11000,11016,13000);malformed=true
    sample=PX.hand_pipeline_finish(q);emit(sample,nil,q)
    log=M.logtext();row=log:match("HANDPIPE inst=7 group=1[^\n]*slot=13[^\n]*decoded_available=false[^\n]*")or""
    T.check(sample and sample.decoded[13]==nil and sample.readback[13].simulating==nil and sample.readback[13].angular_velocity==nil
        and T.contains(row,"sim_available=false simulating=unavailable angular_velocity_available=false"),
        "nonfinite pose/velocity and nonboolean simulation remain explicit unknown rather than plausible values",row)
    cur.slots[13][4]=0;malformed=false
    p.aim.pipeline_mesh={address=9902,name="OTHER_COMPONENT"};PX.hand_pipeline_state=nil
    q=PX.hand_pipeline_begin(p,body,cur,p.aim.slots,11000,11016,13000)
    T.check(q and not q.record.prior_available and q.record.prior[13]==nil,"prior aim cannot cross a component identity change")
    count=velocity_reads
    PX.hand_pipeline_state={used=60,at={}}
    T.check(PX.hand_pipeline_begin(nil,nil,nil,nil)==nil and velocity_reads==count,"total sixty-sample budget rejects before any native read")
    PX.hand_pipeline_state={used=0,started=M.now-180000,at={}}
    T.check(PX.hand_pipeline_begin(nil,nil,nil,nil)==nil and velocity_reads==count,"hard expiry rejects before any native read")
    -- Invalid native scope attempts remain bounded without consuming the
    -- only recording window before any useful sample can complete.
    local mode_slot=HSMPNative._rec.slots.mode
    PX.hand_pipeline_state=nil;HSMPNative._rec.slots.mode=nil;M.now=20000
    T.check(PX.hand_pipeline_begin(p,body,cur,p.aim.slots,0,0,M.now)==nil
        and PX.hand_pipeline_state.used==1 and PX.hand_pipeline_state.started==nil,
        "missing Mode keeps Live strict and counts a rejected attempt without starting recording expiry")
    HSMPNative._rec.slots.mode=mode_slot;M.now=220000
    q=PX.hand_pipeline_begin(p,body,cur,p.aim.slots,0,0,M.now)
    T.check(q and PX.hand_pipeline_state.used==2 and PX.hand_pipeline_state.started==nil,
        "an earlier rejected attempt cannot expire the first later native-scope capture")
    sample=PX.hand_pipeline_finish(q)
    T.check(sample and sample.qualification and sample.authority==false and sample.window_started==220000,
        "first completed native-scope record starts the single180s window without gaining topology authority")
    -- This is the actual initial-placement shape from Native31: the
    -- authoritative Mode still holds round0/life0, while real source and
    -- displayed poses explicitly carry the assigned round1/life1.
    cur.life=1;p.aim=nil;p.shown={has_context=true,pawn="HAND_PROXY",match_id=419,round=1,life=1,cut=7}
    HSMPNative.sc_put("session",{seq=4,match_id=419,round=0,phase=1,rows={{peer_id=2,connected=true,spawn_id=257}}})
    HSMPNative.sc_put("mode",{seq=4,match_id=419,round=0,rows={{peer_id=2,life=0}}})
    local n=#records;drive(225000);sample=records[n+1]
    T.check(sample and sample.authority==false and sample.qualification==false and sample.life==1 and sample.display.life==1
        and sample.audit.mode_match==419 and sample.audit.mode_round==0 and sample.audit.mode_life==0
        and sample.audit.spawn_id==257 and sample.audit.effective_round==1 and sample.window_started==220000,
        "actual pending production drive records separate known source/display life1 and raw authoritative Mode0 values",T.repr(sample))
    q=PX.hand_pipeline_begin(p,body,cur,p.aim.slots,0,0,225000)
    T.check(q==nil,"pending observation retains the existing per-scope cadence")
    local context=M.logtext():match("HANDCONTEXT inst=7 group=3[^\n]*qualification=false[^\n]*")or""
    T.check(T.contains(context,"qualification=false authority=false qualification_reason=mode_tuple_mismatch")
        and T.contains(context,"source_life=1 display_match=419 display_round=1 display_life=1")
        and T.contains(context,"mode_round=0 mode_row_available=true mode_peer=2 mode_life=0"),
        "pending log qualification and actual zero Mode fields cannot be mistaken for authoritative life proof",context)
    M.now=230000;q=PX.hand_pipeline_begin(p,body,cur,p.aim.slots,0,0,M.now)
    T.check(q and not PX.grip_probe_current(q),"HAND-only pending admission leaves strict GRIP qualification unchanged")
    local before=velocity_reads;change_phase=true
    sample,why=PX.hand_pipeline_finish(q)
    T.check(sample==nil and why=="scope changed" and velocity_reads==before+1 and PX.hand_pipeline_state.started==220000,
        "Loading-to-Live transition during getters discards mixed tuple data and never restarts the window")
    change_phase=false
    HSMPNative.sc_put("session",{seq=10,match_id=419,round=0,phase=2,rows={{peer_id=2,connected=true,spawn_id=257}}})
    HSMPNative._rec.slots.mode=nil;M.now=235000
    q=PX.hand_pipeline_begin(p,body,cur,p.aim.slots,0,0,M.now);sample=PX.hand_pipeline_finish(q);emit(sample,nil,q)
    T.check(sample and not sample.qualification and not sample.audit.mode_present and not sample.audit.mode_available
        and sample.audit.mode_match==nil and sample.audit.mode_round==nil and sample.audit.mode_life==nil,
        "Countdown may observe exact pending native scope while absent Mode remains wholly unavailable")
    context=M.logtext():match("HANDCONTEXT inst=7 group=5[^\n]*")or""
    T.check(T.contains(context,"mode_present=false mode_available=false mode_seq=unavailable mode_match=unavailable mode_round=unavailable")
        and T.contains(context,"mode_row_available=false mode_peer=unavailable mode_life=unavailable"),
        "absent Mode never fabricates zero fields or default life1 in observation output",context)
    -- A first other peer's spawn_round must not qualify a mismatched target
    -- peer assignment; missing/disconnected target rows are equally refused.
    HSMPNative.sc_put("session",{seq=11,match_id=419,round=0,phase=1,rows={
        {peer_id=1,connected=true,spawn_id=256},{peer_id=2,connected=true,spawn_id=513}}})
    M.now=240000;before=velocity_reads
    T.check(PX.hand_pipeline_begin(p,body,cur,p.aim.slots,0,0,M.now)==nil and velocity_reads==before,
        "pending assignment encoded round must match this exact peer, not the first peer's spawn_round")
    HSMPNative.sc_put("session",{seq=12,match_id=419,round=0,phase=1,rows={{peer_id=1,connected=true,spawn_id=256}}})
    M.now=245000
    T.check(PX.hand_pipeline_begin(p,body,cur,p.aim.slots,0,0,M.now)==nil,"missing exact peer pending assignment cannot admit an observation")
    HSMPNative.sc_put("session",{seq=13,match_id=419,round=0,phase=1,rows={{peer_id=2,connected=false,spawn_id=257}}})
    M.now=250000
    T.check(PX.hand_pipeline_begin(p,body,cur,p.aim.slots,0,0,M.now)==nil,"disconnected pending spawn row is not current assignment evidence")
    HSMPNative.sc_put("session",{seq=14,match_id=419,round=0,phase=1,rows={{peer_id=2,connected=true,spawn_id=257}}})
    M.now=255000;local used=PX.hand_pipeline_state.used;cur.life=0
    T.check(PX.hand_pipeline_begin(p,body,cur,p.aim.slots,0,0,M.now)==nil and PX.hand_pipeline_state.used==used,
        "missing positive source life cannot use pending assignment as a life1 fallback")
    cur.life=1;q=PX.hand_pipeline_begin(p,body,cur,p.aim.slots,0,0,M.now)
    HSMPNative.sc_put("mode",{seq=15,match_id=419,round=0,rows={{peer_id=2,life=0}}})
    before=velocity_reads
    T.check(PX.hand_pipeline_finish(q)==nil and velocity_reads==before,
        "raw Mode availability change after admission is revalidated before optional getters")
    HSMPNative.sc_put("mode",{seq=16,match_id=419,round=0,rows={}});M.now=260000
    q=PX.hand_pipeline_begin(p,body,cur,p.aim.slots,0,0,M.now);sample=PX.hand_pipeline_finish(q)
    T.check(sample and sample.audit.mode_available and not sample.audit.mode_row_available and sample.audit.mode_life==nil
        and not sample.qualification and sample.life==1,
        "a present raw Mode frame with no peer row cannot invent a life value from source life1")
    M.now=265000;q=PX.hand_pipeline_begin(p,body,cur,p.aim.slots,0,0,M.now)
    HSMPNative.sc_put("session",{seq=17,match_id=419,round=0,phase=1,rows={{peer_id=2,connected=true,spawn_id=258}}})
    before=velocity_reads
    T.check(PX.hand_pipeline_finish(q)==nil and velocity_reads==before,
        "changed exact spawn assignment is refused even when its encoded pending round is unchanged")
    M.now=400000;before=velocity_reads
    T.check(PX.hand_pipeline_begin(nil,nil,nil,nil)==nil and velocity_reads==before and PX.hand_pipeline_state.started==220000,
        "pending captures and later lifecycle changes cannot extend the exact original180s window")
    PX.hand_pipeline_state={used=59,at={}};M.now=500000;HSMPNative._rec.slots.mode=nil
    HSMPNative.sc_put("session",{seq=16,match_id=419,round=1,phase=3})
    T.check(PX.hand_pipeline_begin(p,body,cur,p.aim.slots,0,0,M.now)==nil and PX.hand_pipeline_state.used==60
        and PX.hand_pipeline_state.started==nil and PX.hand_pipeline_begin(nil,nil,nil,nil)==nil,
        "sixty rejected attempts bound optional scope reads even if no recording window ever starts")
end

if opts.kind=="grip_probe"then
    T.write(sd.."/.settings.json",'{"avatars":true,"native_neutralise":false,"native_servo":false}\n')
    local api=boot(true);local PX=api.PX
    if opts.gate then
        T.check(not PX.GRIP_PROBE and PX.grip_probe_begin(nil,nil,nil,nil)==nil and PX.grip_probe_state==nil,
            "grip snapshot requires both developer mode and explicit opt-in before any optional reads")
        return
    end
    sidecar({{1,"Own"},{2,"Peer"}})
    HSMPNative.sc_put("session",{seq=1,match_id=317,round=1,phase=3})
    HSMPNative.sc_put("mode",{seq=1,match_id=317,round=1,rows={{peer_id=1,life=3},{peer_id=2,life=3}}})
    api.on_tick()
    local function fn(n)return {ToString=function()return n end}end
    local function obj(cls,name,addr,owner)
        local o=M.new_obj(cls,name);rawset(o,"__addr",addr);o.__props.Owner=owner;return o
    end
    local pawn=obj("Willie_BP_C","GRIP_PROXY",9711)
    local mesh=obj("SkeletalMeshComponent","GRIP_MESH",9712,pawn);pawn.__props.Mesh=mesh
    local weapon=obj("ModularWeaponBP_Polearm_C","GRIP_WEAPON",9713,pawn)
    local base=obj("StaticMeshComponent","GRIP_BASE",9714,weapon);weapon.__props.BaseMesh=base;pawn.__props["Weapon R"]=weapon
    local reads,writes=0,0
    M.Methods.GetOwner=function(o)return o.__props.Owner end
    M.Methods.GetWorld=function(o)return o.__props.NativeWorld or M.world end
    M.Methods.GetConstrainedComponents=function(c,a,b,x,y)
        reads=reads+1
        local e=c.__props.Endpoints
        if c.__props.OnBinding then c.__props.OnBinding()end
        a.OutComponent1,b.OutBoneName1,x.OutComponent2,y.OutBoneName2=e.one,fn(e.bone1),e.two,fn(e.bone2)
    end
    local function grip(name,addr,bone)
        local c=obj("PhysicsConstraintComponent",name,addr,pawn)
        local v={X=1,Y=2,Z=3}
        local ld={PositionTarget=v,VelocityTarget={X=0,Y=0,Z=0}}
        for _,axis in ipairs({"XDrive","YDrive","ZDrive"})do
            ld[axis]={bEnablePositionDrive=true,bEnableVelocityDrive=false,Stiffness=1200,Damping=0,MaxForce=0}
        end
        local pi={LinearDrive=ld,AngularDrive={SlerpDrive={Stiffness=10,Damping=2,MaxForce=100}},
            LinearLimit={XMotion=0,YMotion=0,ZMotion=0,Limit=0},ConeLimit={Swing1Motion=0,Swing1LimitDegrees=0,Swing2Motion=0,Swing2LimitDegrees=0},
            TwistLimit={TwistMotion=0,TwistLimitDegrees=0}}
        c.__props.ConstraintInstance={ConstraintBone1=fn("None"),ConstraintBone2=fn(bone),Pos1=v,PriAxis1=v,SecAxis1=v,Pos2=v,PriAxis2=v,SecAxis2=v,ProfileInstance=pi}
        c.__props.Endpoints={one=base,two=mesh,bone1="None",bone2=bone}
        return c,pi
    end
    local right,rpi=grip("NATIVE_RIGHT",9715,"hand_r")
    local left,lpi=grip("NATIVE_LEFT",9716,"hand_l")
    pawn.__props["PhysicsConstraint R Hand"],pawn.__props["PhysicsConstraint L Hand"]=right,left
    M.Methods.SetAngularDriveParams=function(c,k,d,f)writes=writes+1;local s=c.ConstraintInstance.ProfileInstance.AngularDrive.SlerpDrive;s.Stiffness,s.Damping,s.MaxForce=k,d,f end
    M.Methods.SetAllMotorsAngularDriveParams=function()writes=writes+1 end
    M.Methods.K2_GetComponentsByClass=function(a)return {a["PhysicsConstraint R Hand"],a["PhysicsConstraint L Hand"]}end
    M.Methods.SetLinearDriveParams=function()error("probe must never write linear params")end
    M.Methods.SetLinearPositionDrive=function(c,x,y,z)
        for i,axis in ipairs({"XDrive","YDrive","ZDrive"})do c.ConstraintInstance.ProfileInstance.LinearDrive[axis].bEnablePositionDrive=({x,y,z})[i]end
    end
    M.Methods.SetLinearVelocityDrive=function(c,x,y,z)
        for i,axis in ipairs({"XDrive","YDrive","ZDrive"})do c.ConstraintInstance.ProfileInstance.LinearDrive[axis].bEnableVelocityDrive=({x,y,z})[i]end
    end
    local p={actor=pawn,addr=9711,gen=api.generation(),peer=2,driving=true,
        last={has_context=true,match_id=317,round=1,life=3,cut=5,seq=61,mode="interp",age=0},
        shown={has_context=true,match_id=317,round=1,life=3,pawn="GRIP_PROXY",cut=5},
        body={mesh=mesh,mesh_addr=9712,mesh_fname="GRIP_MESH",field="Mesh",ctl="servo",motors={}}}
    api.set_puppet(2,p);api.set_driven(p)
    p.grips={at=1000,list={{addr=9715,fname="NATIVE_RIGHT",hand="hand_r"},{addr=9716,fname="NATIVE_LEFT",hand="hand_l"}},by={[9715]=true,[9716]=true}}
    local records={};local emit=PX.grip_probe_emit
    PX.grip_probe_emit=function(r,why,q,stage)records[#records+1]=r or {unavailable=why};emit(r,why,q,stage)end
    M.now=1000
    M.bp_tick(pawn,function()rpi.AngularDrive.SlerpDrive.Stiffness=800;lpi.AngularDrive.SlerpDrive.Stiffness=600 end)
    T.check(#records==2 and records[1].stage=="post_bp" and records[2].stage=="post_policy" and records[1].group==records[2].group,
        "actual second-slot post callback captures an admitted before/after policy pair")
    local before,after=records[1].joints[1],records[2].joints[1]
    T.check(before.complete and after.complete and after.same_pair and before.angular[1]==800 and after.angular[1]==0
        and not after.linear.XDrive.position and not after.linear.XDrive.velocity and after.linear.XDrive.stiffness==1200
        and after.limits[1]==0 and after.linear.XDrive.max_force==0,
        "read-only snapshot distinguishes native enables from the six-flag policy while strength and limits stay unchanged")
    T.check(records[2].joints[2].complete and records[2].joints[2].weapon_field=="Weapon R"
        and after.binding.owner1.address==9713 and after.binding.owner2.address==9711,
        "native left hand binding to the right weapon has exact component/owner provenance")
    local probe_rows,namespaced=0,true
    for _,line in ipairs(M.logs)do
        if line:find("GRIPSTATE",1,true) or line:find("GRIPJOINT",1,true) or line:find("GRIPAXIS",1,true) or line:find("GRIPCALL",1,true)then
            probe_rows=probe_rows+1;namespaced=namespaced and line:find("inst=7",1,true)~=nil
        end
    end
    T.check(probe_rows==19 and namespaced and records[1].instance=="7","every probe header, joint, axis and callback row carries the native instance namespace")
    local count=reads;M.now=1016;M.bp_tick(pawn)
    T.check(reads>count and #records==2 and PX.grip_probe_state.used==2,"cadence refuses optional capture while mandatory current motor binding proof continues")
    count=reads
    PX.GRIP_PROBE=false;M.now=7000;M.bp_tick(pawn)
    T.check(reads>count and #records==2,"disabled probe adds no optional capture to the normal physical callback")
    PX.GRIP_PROBE=true;PX.grip_probe_state=nil
    local q=PX.grip_probe_begin(pawn,9711,"GRIP_PROXY",p)
    local wrote=writes
    local sample=PX.grip_probe_capture(q,"post_bp")
    T.check(sample.joints[1].complete and writes==wrote,"snapshot helpers make no physical writes")
    local function plain(v)
        if type(v)~="table"then return type(v)~="userdata" and type(v)~="function"end
        if rawget(v,"__cls")then return false end
        for _,x in pairs(v)do if not plain(x)then return false end end
        return true
    end
    T.check(plain(sample) and plain(PX.grip_probe_state),"published records and retained cadence state contain no native wrappers")
    rpi.LinearDrive.XDrive.bEnablePositionDrive=nil
    rpi.LinearDrive.YDrive.Damping="0"
    rpi.LinearDrive.ZDrive.Stiffness=0/0
    sample=PX.grip_probe_capture(q,"post_bp")
    T.check(sample.joints[1].binding_current and not sample.joints[1].complete and sample.joints[1].linear.XDrive==nil
        and sample.joints[1].linear.YDrive==nil and sample.joints[1].linear.ZDrive==nil,
        "missing booleans, numeric strings and nonfinite drive values remain explicitly unavailable")
    rpi.LinearDrive.XDrive.bEnablePositionDrive=false;rpi.LinearDrive.YDrive.Damping=0;rpi.LinearDrive.ZDrive.Stiffness=1200
    sample=PX.grip_probe_capture(q,"post_bp")
    T.check(sample.joints[1].complete and sample.joints[1].linear.XDrive.position==false,"actual false is readable evidence rather than unavailable")
    right.__props.OnBinding=function()right.__props.Endpoints.bone2="lowerarm_r"end
    sample=PX.grip_probe_capture(q,"post_bp")
    T.check(not sample.joints[1].binding_current and not sample.joints[1].complete,"constraint bone changes during capture cannot qualify a mixed binding")
    right.__props.OnBinding=nil;right.__props.Endpoints.bone2="hand_r"
    base.__props.Owner=pawn
    sample=PX.grip_probe_capture(q,"post_bp")
    T.check(not sample.joints[1].binding_current,"a component with the wrong actual weapon owner cannot qualify by its field name")
    local log_at=#M.logs;PX.grip_probe_emit(sample)
    local unavailable_axes,availability_ok=0,true
    for i=log_at+1,#M.logs do
        if M.logs[i]:find("GRIPAXIS",1,true)then
            unavailable_axes=unavailable_axes+1
            availability_ok=availability_ok and M.logs[i]:find("available=false",1,true)~=nil
                and M.logs[i]:find("position=unavailable velocity=unavailable stiffness=unavailable",1,true)~=nil
        end
    end
    T.check(unavailable_axes==6 and availability_ok,"unavailable native bindings emit unavailable axes rather than treating boolean false as a drive table")
    base.__props.Owner=weapon
    weapon.__props.NativeWorld=obj("World","FOREIGN_GRIP_WORLD",9800)
    sample=PX.grip_probe_capture(q,"post_bp")
    T.check(not sample.joints[1].binding_current,"a weapon from another exact native world cannot qualify the binding")
    weapon.__props.NativeWorld=nil
    p.last.cut=6;local changed=PX.grip_probe_capture(q,"post_bp")
    T.check(changed==nil,"a same-life source discontinuity invalidates the admitted capture scope")
    p.last.cut=5
    q.before=PX.grip_probe_capture(q,"post_bp")
    local rebuilt=grip("REBUILT_RIGHT",9715,"hand_r")
    rawset(right,"__dead",true);pawn.__props["PhysicsConstraint R Hand"]=rebuilt
    sample=PX.grip_probe_capture(q,"post_policy")
    T.check(sample.joints[1].binding_current and not sample.joints[1].same_pair and not sample.joints[1].complete and #M.dead_touch==0,
        "fresh rebuilt field never touches the dead cached wrapper or inherits the prior pair's identity")
    p.last.life=2;local result,why=PX.grip_probe_capture(q,"post_bp")
    T.check(result==nil and why=="scope changed","old source life rejects the whole diagnostic snapshot")
    p.last.life=3;p.body.mesh_addr=9999
    result=PX.grip_probe_capture(q,"post_bp")
    T.check(result==nil and #M.dead_touch==0,"a body identity change rejects the original snapshot without touching retained components")
    p.body.mesh_addr=9712
    PX.grip_probe_state=nil;records={};wrote=writes
    rebuilt.__props.OnBinding=function()HSMPNative.sc_put("mode",{seq=2,match_id=317,round=1,rows={{peer_id=1,life=3},{peer_id=2,life=4}}})end
    M.bp_tick(pawn)
    T.check(writes==wrote and #records==1 and records[1].unavailable=="scope changed",
        "a life change during admitted native reads fails before the existing physical writer")
    rebuilt.__props.OnBinding=nil
    HSMPNative.sc_put("mode",{seq=3,match_id=317,round=1,rows={{peer_id=1,life=3},{peer_id=2,life=3}}})
    local own=obj("Willie_BP_C","OWN_GRIP",9811);local own_mesh=obj("SkeletalMeshComponent","OWN_MESH",9812,own)
    own.__props.Mesh=own_mesh;M.pc.__props.Pawn=own
    HSMP_IPC.bus_put("spawn_status",{seq=1,verified=true,pawn="OWN_GRIP",match_id=317,round=1,life=3})
    PX.grip_probe_state=nil;records={};wrote=writes;M.bp_tick(own)
    T.check(#records==1 and records[1].role=="owner" and records[1].stage=="post_bp" and not records[1].joints[1].complete and writes==wrote,
        "verified current native owner is sampled read-only with absent hand fields explicitly unavailable")
    PX.grip_probe_state=nil
    local oq=PX.grip_probe_begin(own,9811,"OWN_GRIP",nil)
    local function refused(label)
        T.check(not PX.grip_probe_current(q) and not PX.grip_probe_current(oq),label)
    end
    local session_slot=HSMPNative._rec.slots.session
    HSMPNative._rec.slots.session=nil
    refused("owner and driven snapshots require an available published session")
    HSMPNative._rec.slots.session=session_slot
    HSMPNative.sc_put("session",{seq=2,match_id=0,round=1,phase=3})
    refused("a zero-match session cannot certify retained spawn and pose tuples")
    HSMPNative.sc_put("session",{seq=3,match_id=317,round=1,phase=3})
    local mode_slot=HSMPNative._rec.slots.mode
    HSMPNative._rec.slots.mode=nil
    refused("owner and driven snapshots require the current Mode record")
    HSMPNative._rec.slots.mode=mode_slot
    HSMPNative.sc_put("mode",{seq=4,match_id=317,round=1,rows={}})
    refused("retained native bodies cannot invent a missing current Mode life row")
    HSMPNative.sc_put("mode",{seq=5,match_id=317,round=1,rows={{peer_id=1,life=0},{peer_id=2,life=0}}})
    refused("a zero life row cannot certify either role")
    HSMPNative.sc_put("mode",{seq=6,match_id=317,round=1,rows={{peer_id=1,life=3},{peer_id=2,life=3}}})
    HSMPNative.sc_put("link",{status=0,state=0,my_peer_id=1})
    refused("disconnected retained session/mode/spawn slots remain unavailable")
    sidecar({{1,"Own"},{2,"Peer"}})
    HSMPNative._st.hb_age=10
    refused("an expired heartbeat cannot certify an otherwise matching retained life")
    HSMPNative._st.hb_age=.05
    local native_info=HSMPNative.ipc_info;HSMPNative.ipc_info=function()return nil end
    refused("an unavailable fresh IPC header cannot borrow cached healthy connection data")
    HSMPNative.ipc_info=function()return {sidecar_hb_age_s=.05}end
    refused("missing current sidecar state cannot certify a retained connected link")
    HSMPNative.ipc_info=native_info
    T.check(PX.grip_probe_current(q) and PX.grip_probe_current(oq),"restoring fresh exact session and Mode permits both current roles")
    local retained_mode=require("hsmp_session").mode()
    HSMPNative.sc_put("mode",{seq=7,match_id=317,round=1,rows={{peer_id=1,life=4},{peer_id=2,life=4}}})
    T.check(retained_mode.rows[1].life==3 and retained_mode.rows[2].life==3 and not PX.grip_probe_current(q) and not PX.grip_probe_current(oq),
        "healthy cached default Mode cannot mask newly published life rows without an on_tick or cache reset")
    HSMPNative.sc_put("mode",{seq=8,match_id=317,round=1,rows={{peer_id=1,life=3},{peer_id=2,life=3}}})
    T.check(PX.grip_probe_current(q) and PX.grip_probe_current(oq),"module record views immediately observe restored native versions")
    -- Prepare publishes the next spawn order while session.round still names
    -- the completed round. Only Mode for that exact spawn round may certify it.
    HSMPNative.sc_put("session",{seq=4,match_id=317,round=1,phase=1,rows={{peer_id=1,connected=true,spawn_id=512},{peer_id=2,connected=true,spawn_id=513}}})
    p.last.round,p.shown.round,q.round,oq.round=2,2,2,2
    p.last.life,p.shown.life,q.life,oq.life=1,1,1,1
    HSMP_IPC.bus_put("spawn_status",{seq=2,verified=true,pawn="OWN_GRIP",match_id=317,round=2,life=1})
    refused("Prepare cannot inherit previous-round Mode or its default life1 fallback")
    HSMPNative.sc_put("mode",{seq=7,match_id=317,round=2,rows={{peer_id=1,life=1},{peer_id=2,life=1}}})
    T.check(PX.grip_probe_current(q) and PX.grip_probe_current(oq),"Prepare uses exact positive spawn_round and published next-round life rows")
    HSMPNative.sc_put("session",{seq=5,match_id=317,round=1,phase=2,rows={{peer_id=1,connected=true,spawn_id=512},{peer_id=2,connected=true,spawn_id=513}}})
    T.check(PX.grip_probe_current(q) and PX.grip_probe_current(oq),"Countdown preserves the same exact authoritative spawn-round selection")
    HSMPNative.sc_put("session",{seq=6,match_id=317,round=1,phase=3})
    HSMPNative.sc_put("mode",{seq=8,match_id=317,round=1,rows={{peer_id=1,life=3},{peer_id=2,life=3}}})
    p.last.round,p.shown.round,q.round,oq.round=1,1,1,1
    p.last.life,p.shown.life,q.life,oq.life=3,3,3,3
    HSMP_IPC.bus_put("spawn_status",{seq=3,verified=true,pawn="OWN_GRIP",match_id=317,round=1,life=3})
    HSMP_IPC.bus_put("spawn_status",{seq=2,verified=true,pawn="OWN_GRIP",match_id=317,round=1,life=2})
    M.now=13000;records={};M.bp_tick(own)
    T.check(#records==0,"owner spawn status cannot relabel the authoritative current life")
    PX.grip_probe_state={used=119,started=M.now,at={}}
    count=reads;T.check(PX.grip_probe_begin(pawn,9711,"GRIP_PROXY",p)==nil and reads==count,
        "last single budget slot cannot admit half a driven before/after pair")
    PX.grip_probe_state={used=0,started=M.now-180000,at={}}
    T.check(PX.grip_probe_begin(pawn,9711,"GRIP_PROXY",p)==nil and reads==count,"hard expiry refuses native reads despite a new callback identity")
end

if opts.kind=="grip_linear" then
    T.write(sd.."/.settings.json",'{"avatars":true,"native_neutralise":false,"native_servo":false}\n')
    local api=boot(true);local PX=api.PX
    sidecar({{1,"Own"},{2,"Peer"}})
    HSMPNative.sc_put("session",{seq=1,match_id=517,round=1,phase=3})
    HSMPNative.sc_put("mode",{seq=1,match_id=517,round=1,rows={{peer_id=1,life=3},{peer_id=2,life=3}}})
    api.on_tick()
    local function fn(s)return {ToString=function()return s end}end
    local function obj(cls,name,addr,owner)
        local o=M.new_obj(cls,name);rawset(o,"__addr",addr);o.__props.Owner=owner;return o
    end
    M.Methods.GetOwner=function(o)return o.__props.Owner end
    M.Methods.GetWorld=function(o)return o.__props.NativeWorld or M.world end
    local pawn=obj("Willie_BP_C","LINEAR_PROXY",9911)
    local mesh=obj("SkeletalMeshComponent","LINEAR_MESH",9912,pawn);pawn.__props.Mesh=mesh
    local weapon=obj("ModularWeaponBP_Polearm_C","LINEAR_WEAPON",9913,pawn)
    local root=obj("StaticMeshComponent","LINEAR_BASE",9914,weapon);weapon.__props.BaseMesh=root;pawn.__props["Weapon R"]=weapon
    local originals={true,false,true,false,true,false}
    local function grip(name,addr,hand)
        local c=obj("PhysicsConstraintComponent",name,addr,pawn)
        local ld={PositionTarget={X=7,Y=8,Z=9},VelocityTarget={X=1,Y=2,Z=3}}
        for i,axis in ipairs({"XDrive","YDrive","ZDrive"})do
            ld[axis]={bEnablePositionDrive=originals[i],bEnableVelocityDrive=originals[i+3],Stiffness=({7500,1200,0})[i],Damping=i-1,MaxForce=i*11}
        end
        c.__props.ConstraintInstance={ConstraintBone1=fn("None"),ConstraintBone2=fn(hand),ProfileInstance={LinearDrive=ld,
            AngularDrive={SlerpDrive={Stiffness=10,Damping=2,MaxForce=100}}}}
        c.__props.Endpoints={one=root,two=mesh,bone1="None",bone2=hand}
        return c,ld
    end
    local right,rld=grip("LINEAR_R",9915,"hand_r")
    local left,lld=grip("LINEAR_L",9916,"hand_l")
    pawn.__props["PhysicsConstraint R Hand"],pawn.__props["PhysicsConstraint L Hand"]=right,left
    local calls,fail,lie,binding_count=0,nil,false,0
    M.Methods.K2_GetComponentsByClass=function(a)return {a["PhysicsConstraint R Hand"],a["PhysicsConstraint L Hand"]}end
    M.Methods.GetConstrainedComponents=function(c,a,b,x,y)
        binding_count=binding_count+1
        assert(a==b and a==x and a==y,"all four scalar outputs share one fresh container")
        local e=c.__props.Endpoints
        a.OutComponent1,a.OutBoneName1,a.OutComponent2,a.OutBoneName2=e.one,fn(e.bone1),e.two,fn(e.bone2)
        if c.__props.DuringBinding then c.__props.DuringBinding()end
    end
    M.Methods.SetLinearDriveParams=function()error("linear strengths are not owned")end
    M.Methods.SetLinearPositionDrive=function(c,x,y,z)
        calls=calls+1
        if not lie then for i,axis in ipairs({"XDrive","YDrive","ZDrive"})do c.ConstraintInstance.ProfileInstance.LinearDrive[axis].bEnablePositionDrive=({x,y,z})[i]end end
        if fail=="position"then error("position setter failed after native success")end
    end
    M.Methods.SetLinearVelocityDrive=function(c,x,y,z)
        calls=calls+1
        if fail=="velocity"then error("velocity setter refused")end
        if not lie then for i,axis in ipairs({"XDrive","YDrive","ZDrive"})do c.ConstraintInstance.ProfileInstance.LinearDrive[axis].bEnableVelocityDrive=({x,y,z})[i]end end
    end
    local p={actor=pawn,addr=9911,gen=api.generation(),peer=2,driving=true,
        last={has_context=true,match_id=517,round=1,life=3},shown={has_context=true,match_id=517,round=1,life=3,pawn="LINEAR_PROXY"},
        body={mesh=mesh,mesh_addr=9912,mesh_fname="LINEAR_MESH",field="Mesh",ctl="servo",motors={},handles={},sims={}}}
    api.set_puppet(2,p);api.set_driven(p)
    local function flags(ld)
        local t={};for i,axis in ipairs({"XDrive","YDrive","ZDrive"})do t[i],t[i+3]=ld[axis].bEnablePositionDrive,ld[axis].bEnableVelocityDrive end;return t
    end
    local function native(ld,values)
        for i,axis in ipairs({"XDrive","YDrive","ZDrive"})do ld[axis].bEnablePositionDrive,ld[axis].bEnableVelocityDrive=values[i],values[i+3]end
    end
    local function untouched(ld)
        for i,axis in ipairs({"XDrive","YDrive","ZDrive"})do
            local d=ld[axis];if d.Stiffness~=({7500,1200,0})[i]or d.Damping~=i-1 or d.MaxForce~=i*11 then return false end
        end
        return T.eq(ld.PositionTarget,{X=7,Y=8,Z=9})and T.eq(ld.VelocityTarget,{X=1,Y=2,Z=3})
    end
    M.now=1000;M.bp_tick(pawn)
    local lease=p.linear_grip_leases["PhysicsConstraint R Hand"]
    T.check(T.eq(flags(rld),{false,false,false,false,false,false}) and T.eq(flags(lld),{false,false,false,false,false,false})
        and lease.status=="off_confirmed" and T.eq(lease.flags,originals),"actual Blueprint POST callback reads and confirms all six flags off with immutable mixed originals")
    T.check(p.linear_grip_leases["PhysicsConstraint L Hand"].binding.weapon_field=="Weapon R" and untouched(rld) and untouched(lld),
        "native offhand bound to right weapon is supported without changing distinct per-axis strengths or targets")
    local function plain(t)
        if type(t)~="table"then return type(t)~="userdata"and type(t)~="function"end
        if rawget(t,"__cls")then return false end
        for _,v in pairs(t)do if not plain(v)then return false end end;return true
    end
    T.check(plain(p.linear_grip_leases),"retained flag leases contain only scalar identities and flags")
    local function notes(state)
        local n=0;for _,line in ipairs(M.logs)do
            if line:find("linear grip",1,true)and line:find("state="..state,1,true)then n=n+1 end
        end;return n
    end
    T.check(notes("off_confirmed")==2,"first confirmed suppression logs both current hands once")
    local setters,getters=calls,binding_count;M.now=1008;M.bp_tick(pawn)
    T.check(calls==setters and binding_count-getters==4 and lease.status=="off_confirmed",
        "already-off existing leases use two full fresh resolves per hand and no setters")
    getters=binding_count;setters=calls
    M.now=1016;M.bp_tick(pawn,function()native(rld,{true,true,true,true,true,true});native(lld,{true,true,true,true,true,true})end)
    T.check(T.eq(flags(rld),{false,false,false,false,false,false})and p.linear_grip_leases["PhysicsConstraint R Hand"]==lease
        and T.eq(lease.flags,originals),"later native BP drift is reasserted without recapturing our false flags or overwriting originals")
    T.check(calls-setters==4 and binding_count-getters==8,"native enabled-flag drift still takes both setters and four full resolves per hand")
    T.check(notes("off_confirmed")==2,"steady BP reassertions do not produce per-frame confirmation logs")
    local observed=PX.grip_linear_observe(p)
    T.check(observed.available and T.eq(observed.current_flags,{false,false,false,false,false,false})
        and observed.original_available and T.eq(observed.original_flags,originals) and observed.authority==false,
        "read-only fault helper copies current six flags and same-binding original flags with separate observation time")
    local snapshot_setters,snapshot_logs=calls,#M.logs
    native(rld,originals);observed=PX.grip_linear_observe(p)
    T.check(observed.available and T.eq(observed.current_flags,originals) and calls==snapshot_setters and #M.logs==snapshot_logs
        and p.linear_grip_leases["PhysicsConstraint R Hand"]==lease,
        "nonzero current flags remain readable evidence without setters, log output or lease mutation")
    observed.binding.pawn.name="MUTATED_COPY";observed.original_flags[1]=false
    T.check(lease.binding.pawn.name=="LINEAR_PROXY" and lease.flags[1]==true,"returned observer binding and originals are independent plain copies")
    local retained=p.linear_grip_leases["PhysicsConstraint R Hand"]
    p.linear_grip_leases["PhysicsConstraint R Hand"]=nil
    observed=PX.grip_linear_observe(p)
    T.check(observed.available and not observed.original_available and observed.original_flags==nil
        and p.linear_grip_leases["PhysicsConstraint R Hand"]==nil and calls==snapshot_setters,
        "current readback without an original lease stays independently available and creates no restoration debt")
    p.linear_grip_leases["PhysicsConstraint R Hand"]=retained
    right.__props.DuringBinding=function()p.last.cut=9 end
    observed=PX.grip_linear_observe(p)
    T.check(not observed.available and calls==snapshot_setters,"source cut change during native getter refuses mixed fault observations")
    right.__props.DuringBinding=nil;p.last.cut=nil
    local x=rld.XDrive;local position=x.bEnablePositionDrive
    x.bEnablePositionDrive=nil
    setmetatable(x,{__index=function(_,key)
        if key=="bEnablePositionDrive"then p.last.seq=12;return position end
    end})
    observed=PX.grip_linear_observe(p)
    T.check(not observed.available and observed.reason=="source changed" and calls==snapshot_setters,
        "source sequence change while reading six flags refuses mixed observations without setters")
    setmetatable(x,nil);x.bEnablePositionDrive=position;p.last.seq=nil
    rld.ZDrive.bEnableVelocityDrive=nil;observed=PX.grip_linear_observe(p)
    T.check(not observed.available and calls==snapshot_setters,"missing actual flag produces explicit unavailable without native setters")
    rld.ZDrive.bEnableVelocityDrive=originals[6]
    local observe_reads=0
    right.__props.DuringBinding=function()
        observe_reads=observe_reads+1;if observe_reads==2 then rld.XDrive.bEnablePositionDrive=false end
    end
    observed=PX.grip_linear_observe(p)
    T.check(not observed.available and observed.reason=="observed flags changed" and calls==snapshot_setters,
        "six flags changed between fresh resolves refuse evidence instead of inventing stable state")
    right.__props.DuringBinding=nil;native(rld,{false,false,false,false,false,false})
    fail="velocity";local done=api.release_standin(p)
    T.check(done==false and p.driving==false and p.grips==nil and p.linear_grip_leases and lease.status=="restore_pending",
        "partial release retains independent restoration debt after the grip cache is discarded")
    local n=calls;fail=nil;M.now=1500
    T.check(api.release_standin(p)==false and calls==n,"failed restoration retry is bounded to one attempt per second")
    M.now=2101
    T.check(api.release_standin(p)==true and not p.linear_grip_leases and T.eq(flags(rld),originals)and T.eq(flags(lld),originals)
        and untouched(rld)and untouched(lld),"already-released body retries and confirms exact false/true originals with zero-strength axis preserved")
    T.check(notes("restored")==2,"verified exact restoration is emitted once per original lease")
    p.driving=true;M.now=3000;fail="position";M.bp_tick(pawn)
    lease=p.linear_grip_leases["PhysicsConstraint R Hand"]
    T.check(lease and lease.status=="off_pending"and T.eq(lease.flags,originals)and T.eq(flags(rld),{false,false,false,false,true,false}),
        "setter throwing after partial native success retains originals before the first write")
    fail=nil;M.now=3016;M.bp_tick(pawn)
    T.check(lease.status=="off_confirmed"and T.eq(lease.flags,originals),"partial suppression retry uses the original lease and verifies readback")
    lie=true;native(rld,originals);M.now=3032;M.bp_tick(pawn)
    T.check(lease.status=="off_pending"and p.linear_grip_error=="linear readback unavailable","successful setters without changed flags remain explicitly unconfirmed")
    lie=false;M.now=3048;M.bp_tick(pawn)
    local binding_reads=0
    rld.XDrive.bEnablePositionDrive=true -- exercise the original full setter path
    right.__props.DuringBinding=function()
        binding_reads=binding_reads+1;if binding_reads==4 then rld.XDrive.bEnablePositionDrive=true end
    end
    M.now=3050;PX.grip_linear(p,true)
    T.check(lease.status=="off_pending","native flag drift in final binding getter cannot yield a false confirmed readback")
    right.__props.DuringBinding=nil;M.now=3052;PX.grip_linear(p,true)
    binding_reads=0;setters=calls
    right.__props.DuringBinding=function()
        binding_reads=binding_reads+1;if binding_reads==2 then rld.XDrive.bEnablePositionDrive=true end
    end
    M.now=3054;PX.grip_linear(p,true)
    T.check(calls-setters==2 and lease.status=="off_confirmed" and T.eq(flags(rld),{false,false,false,false,false,false}),
        "flag rewrite during second fast-path resolve falls through to full guarded setters rather than false confirmation")
    right.__props.DuringBinding=nil
    binding_reads=0;setters=calls
    right.__props.DuringBinding=function()
        binding_reads=binding_reads+1;if binding_reads==2 then rld.YDrive.bEnableVelocityDrive=nil end
    end
    M.now=3056;PX.grip_linear(p,true)
    T.check(calls==setters and lease.status=="off_pending" and T.eq(lease.flags,originals),
        "missing bool in final fast-path read refuses setters and confirmation while preserving original debt")
    right.__props.DuringBinding=nil;rld.YDrive.bEnableVelocityDrive=false
    M.now=3058;PX.grip_linear(p,true)
    binding_reads=0;setters=calls
    local during_mesh=obj("SkeletalMeshComponent","LINEAR_MESH_REENTRY",9912,pawn)
    right.__props.DuringBinding=function()
        binding_reads=binding_reads+1
        if binding_reads==2 then pawn.__props.Mesh=during_mesh;rawset(mesh,"__dead",true)end
    end
    M.now=3060;PX.grip_linear(p,true)
    T.check(calls==setters and #M.dead_touch==0 and lease.status=="off_pending",
        "same-world Mesh replacement in second fast resolve aborts before returned dead endpoint access or setters")
    right.__props.DuringBinding=nil;pawn.__props.Mesh=mesh;rawset(mesh,"__dead",false)
    M.now=3062;PX.grip_linear(p,true)
    local rebuilt,newld=grip("LINEAR_R_REBUILT",9915,"hand_r");rawset(right,"__dead",true);pawn.__props["PhysicsConstraint R Hand"]=rebuilt
    n=calls;M.now=3064;M.bp_tick(pawn)
    T.check(not p.linear_grip_leases["PhysicsConstraint R Hand"]and T.eq(flags(newld),originals)and #M.dead_touch==0,
        "same address with a new constraint name forgets old debt without touching the dead wrapper or mutating replacement")
    M.now=3080;M.bp_tick(pawn)
    lease=p.linear_grip_leases["PhysicsConstraint R Hand"]
    rebuilt.__props.Endpoints.one=mesh -- same constraint rebound is a different native binding
    M.now=3096;M.bp_tick(pawn)
    T.check(not p.linear_grip_leases["PhysicsConstraint R Hand"]and #M.dead_touch==0,"rebound endpoints cannot inherit or restore the old flags")
    rebuilt.__props.Endpoints.one=root;native(newld,originals);newld.YDrive.bEnableVelocityDrive=nil
    n=calls;M.now=3112;PX.grip_linear(p,true)
    T.check(not p.linear_grip_leases["PhysicsConstraint R Hand"]and calls==n
        and p.linear_grip_leases["PhysicsConstraint L Hand"].status=="off_confirmed",
        "one missing actual boolean refuses both setters for that hand without blocking already-off verification of the other exact hand")
    newld.YDrive.bEnableVelocityDrive=true;M.now=3128;PX.grip_linear(p,true)
    local fresh=obj("SkeletalMeshComponent","NEW_LINEAR_MESH",9912,pawn);pawn.__props.Mesh=fresh;rawset(mesh,"__dead",true)
    n=calls;M.now=3144;PX.grip_linear(p,true)
    T.check(not p.linear_grip_leases and calls==n and #M.dead_touch==0,"replaced Mesh refuses all linear writes before touching its dead retained wrapper")
    pawn.__props.Mesh=mesh;rawset(mesh,"__dead",false)
    native(newld,originals);native(lld,originals);p.body.mesh_fname="LINEAR_MESH";p.driving=true
    M.now=3160;PX.grip_linear(p,true);n=calls
    p.last.life=4;p.shown.life=4;M.now=3176;PX.grip_linear(p,true)
    T.check(not p.linear_grip_leases and calls==n,"changed full source life forgets old flags without restoring them into another life")
    p.last.life=3;p.shown.life=3;p.peer=1;n=calls;PX.grip_linear(p,true)
    T.check(not p.linear_grip_leases and calls==n,"actual local source peer is never eligible for grip suppression")
    p.peer=2
    local foreign=obj("World","LINEAR_FOREIGN_WORLD",9999)
    rebuilt.__props.DuringBinding=function()
        M.pc.__props.NativeWorld=foreign
        rawset(root,"__dead",true);rawset(mesh,"__dead",true)
    end
    n=calls;M.now=3200;PX.grip_linear(p,true)
    T.check(calls==n and #M.dead_touch==0 and not p.linear_grip_leases,
        "PC-world-only reentry refuses before touching returned dead endpoints or issuing either flag setter")
    rebuilt.__props.DuringBinding=nil;M.pc.__props.NativeWorld=nil
    rawset(root,"__dead",false);rawset(mesh,"__dead",false)
    local previous=PX.linear_grip_notes;PX.linear_grip_notes=64
    local note_count=#M.logs;PX.grip_linear_note(p,"PhysicsConstraint R Hand",nil,"new_cap_case","unavailable")
    T.check(#M.logs==note_count and PX.linear_grip_notes==64,"process-total diagnostic cap refuses additional rows without any native reads")
    PX.linear_grip_notes=previous
end

if opts.kind=="grip_reassert" then
    local api=boot(true)
    sidecar({{1,"Own"},{2,"Peer"}})
    api.on_tick() -- validate this native world before a driven-frame callback
    local function fn(n)return {ToString=function()return n end}end
    local function grip(name,addr)
        local c={writes=0,IsValid=function()return true end,GetAddress=function()return addr end,GetFName=function()return fn(name)end}
        local pi={AngularDrive={SlerpDrive={Stiffness=10,Damping=2,MaxForce=100}},
            LinearLimit={XMotion=2,YMotion=2,ZMotion=1,Limit=31},
            ConeLimit={Swing1Motion=2,Swing1LimitDegrees=10,Swing2Motion=2,Swing2LimitDegrees=20},
            TwistLimit={TwistMotion=2,TwistLimitDegrees=30}}
        c.ConstraintInstance={ConstraintBone2=fn("hand_r"),ProfileInstance=pi}
        for _,axis in ipairs({"X","Y","Z"}) do
            c["SetLinear"..axis.."Limit"]=function(_,mode,value)c.writes=c.writes+1;pi.LinearLimit[axis.."Motion"],pi.LinearLimit.Limit=mode,value end
        end
        c.SetAngularSwing1Limit=function(_,mode,value)c.writes=c.writes+1;pi.ConeLimit.Swing1Motion,pi.ConeLimit.Swing1LimitDegrees=mode,value end
        c.SetAngularSwing2Limit=function(_,mode,value)c.writes=c.writes+1;pi.ConeLimit.Swing2Motion,pi.ConeLimit.Swing2LimitDegrees=mode,value end
        c.SetAngularTwistLimit=function(_,mode,value)c.writes=c.writes+1;pi.TwistLimit.TwistMotion,pi.TwistLimit.TwistLimitDegrees=mode,value end
        c.SetAngularDriveParams=function(_,strength,damping,force)c.writes=c.writes+1;c.drive={strength,damping,force}end
        return c,pi
    end
    local old,pi=grip("GripR",501)
    local current=old
    local weapon={IsValid=function()return true end,GetAddress=function()return 7001 end,GetClass=function()return {GetFName=function()return fn("ModularWeaponBP_Polearm_C")end}end}
    local actor={ ["Weapon R"]=weapon,K2_GetComponentsByClass=function()return {current}end }
    local p={actor=actor,gen=api.generation(),peer=2,driving=true,last={has_context=true,match_id=101,round=1,life=1},
        wservo={["Weapon R"]=1000},wservo_actor={["Weapon R"]=7001}}
    M.now=1000
    local list=api.PX.grip_constraints(p,1000)
    T.check(list[1].freed and pi.LinearLimit.ZMotion==0,"current real weapon servo qualifies the existing free-grip policy")
    old:SetLinearZLimit(1,32) -- actual native timeline writer between frames
    T.check(list[1].freed and pi.LinearLimit.ZMotion==1,"cached free flag cannot describe the native slide-axis limit")
    M.now=1016;api.PX.grips_off(p,true)
    T.check(pi.LinearLimit.XMotion==0 and pi.LinearLimit.YMotion==0 and pi.LinearLimit.ZMotion==0 and pi.LinearLimit.Limit==0,
        "next driven frame reasserts actual free limits after native Z drift before the one-second rescan")
    local writes=old.writes
    p.gen=999;api.PX.grips_off(p,true)
    T.check(old.writes==writes and list[1].c==nil,"a stale world touches no native grip and forgets retained wrappers")
    p.gen=api.generation()
    local retained=p.last;p.last=nil;api.PX.grips_off(p,true)
    T.check(old.writes==writes,"no source pose leaves current native grip untouched")
    p.last=retained
    HSMPNative.sc_put("session",{seq=1,match_id=101,round=1,phase=3})
    HSMPNative.sc_put("mode",{seq=1,match_id=101,round=1,rows={{peer_id=2,life=2}}})
    api.PX.grips_off(p,true)
    T.check(old.writes==writes,"a stale owner life cannot reassert limits on the current native grip")
    p.last.life=2
    local rebuilt=grip("GripR_new",501) -- same address, another native identity
    current=rebuilt;api.PX.grips_off(p,true)
    T.check(rebuilt.writes==0 and old.writes==writes and list[1].c==nil,"a rebuilt grip at a reused address is never written with the old identity's policy")
    current=old
    api.PX.grips_off(p,false)
    T.check(pi.LinearLimit.XMotion==2 and pi.LinearLimit.YMotion==2 and pi.LinearLimit.ZMotion==1 and pi.LinearLimit.Limit==31
        and pi.ConeLimit.Swing1LimitDegrees==10 and pi.TwistLimit.TwistLimitDegrees==30 and T.eq(old.drive,{10,2,100}),
        "release restores first native limits and drive rather than the later timeline drift")
    old:SetLinearZLimit(1,32)
    local before=old.writes
    actor["Weapon R"]={IsValid=function()return true end,GetAddress=function()return 7002 end,GetClass=weapon.GetClass}
    api.PX.grips_off(p,true)
    T.check(pi.LinearLimit.ZMotion==1 and old.writes==before+1,"a different unserved weapon is not freed by the previous weapon's servo timestamp")
end

if opts.kind == "fist_grip" then
    local api = boot(true)
    local states = {}
    local grip = {
        IsValid=function() return true end, GetAddress=function() return 501 end,
        GetFName=function()return {ToString=function()return "GripR" end}end,
        ConstraintInstance={ ConstraintBone2={ToString=function() return "hand_r" end},
            ProfileInstance={AngularDrive={SlerpDrive={Stiffness=10,Damping=2,MaxForce=100}},
                LinearLimit={XMotion=2,YMotion=2,ZMotion=2,Limit=1},
                ConeLimit={Swing1Motion=2,Swing1LimitDegrees=10,Swing2Motion=2,Swing2LimitDegrees=20},
                TwistLimit={TwistMotion=2,TwistLimitDegrees=30}} },
    }
    for _, k in ipairs({"LinearX","LinearY","LinearZ","AngularSwing1","AngularSwing2","AngularTwist"}) do
        grip["Set"..k.."Limit"] = function(_, mode, value) states[k]={mode,value} end
    end
    grip.SetAngularDriveParams = function() end
    local function weapon(cls)
        return {IsValid=function() return true end,
            GetClass=function() return {GetFName=function() return {ToString=function() return cls end} end} end}
    end
    local actor={ ["Weapon R"]=weapon("Weapon_Fists_C"), K2_GetComponentsByClass=function() return {grip} end }
    local p={actor=actor}
    local list=api.PX.grip_constraints(p,1000)
    T.check(list[1].freed and states.LinearX[1]==0 and states.AngularTwist[1]==0,
        "fist grip is freed even without a weapon servo target")
    actor["Weapon R"]=weapon("ModularWeaponBP_Polearm_C")
    list=api.PX.grip_constraints(p,2001)
    T.check(not list[1].freed and states.LinearX[1]==2 and states.AngularTwist[2]==30,
        "an unserved real weapon regains its original grip limits")
    actor["Weapon R"]=weapon("Weapon_Fists_C")
    api.PX.grip_constraints(p,3002)
    actor["Weapon R"]=nil
    list=api.PX.grip_constraints(p,4003)
    T.check(not list[1].freed and states.LinearX[1]==2,"an empty hand regains original grip limits")
    actor["Weapon R"]=weapon("Weapon_Fists_C")
    api.PX.grip_constraints(p,5004)
    api.PX.grips_off(p,false)
    T.check(not list[1].freed and states.LinearX[1]==2 and states.AngularTwist[1]==2,
        "ending the stand-in drive restores its original fist grip")
end
