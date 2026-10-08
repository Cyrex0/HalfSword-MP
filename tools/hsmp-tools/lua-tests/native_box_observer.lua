local M=dofile(T.path("mods/dev/HSMPParity/Scripts/native_box_observer.lua"))
-- Stop telemetry cannot prevent cleanup or recurse through a logging callback.
local function stop_logging_fixture(reentrant)
    local obs,live,stops,reads,statuses,resolved=false,false,0,0,0,0
    local n={begin=function()return true end,prepare=function()return true,1 end,
        activate=function()live=true;return true end,stop=function()stops=stops+1;live=false;return true end,
        read=function()reads=reads+1;return {}end,status=function()statuses=statuses+1;return {active=live,reason="capture budget"}end,mark=function()end}
    local s={match_id=1,round=1,life=1,owner_life=1,owner_pawn=20,owner_mesh=30,world_key="w",peer=2,side="r",grip=14}
    for i,k in ipairs({"world","pawn","mesh","box","box_owner"})do s[k]={address=i,path="/"..k}end
    local at=1000
    obs=M.new({developer=function()return true end,native=function()return n end,clock_ms=function()return at end,
        snapshot=function()resolved=resolved+1;return s,nil,{local_ms=at},true end,register=function()return 1,1 end,emit=function()end,
        log=function(fmt)
            if fmt:find("BOXOBS_STATUS",1,true)or fmt:find("BOXOBS stopped",1,true)then
                T.check(not live,"optional stop logging follows native cleanup")
                if reentrant then obs.drop()else error("logger unavailable")end
            end
        end})
    obs.command("2 r 1");at=1033;obs.tick()
    local before_resolved=resolved
    T.check(pcall(obs.drop),"throwing/reentrant stop logger is contained")
    T.check(stops==1 and reads==1 and statuses==1,"stop logger cannot repeat native stop/read/status")
    obs.tick();T.check(resolved==before_resolved,"cleared stop state cannot resolve an old scope after logger")
end
stop_logging_fixture(false);stop_logging_fixture(true)
local logs,emitted,hooks,begin_count,mark_count,stop_count={},{},{},0,0,0
local scope={match_id=123,round=2,life=3,owner_life=7,owner_pawn=20,owner_mesh=30,world_key="world#1",peer=2,side="r",grip=14}
for i,k in ipairs({"world","pawn","mesh","box","box_owner"})do scope[k]={address=i,path="/"..k}end
local function clone(s)local r={};for k,v in pairs(s)do r[k]=type(v)=="table" and {path=v.path,address=v.address} or v end;return r end
local native_active=false
local fixture_at=1000
local pending,pending_role,resolves=1,1,0
local queued={}
local native={begin=function(s)begin_count=begin_count+1;native_active=true;T.check(s.calls==32 and s.duration_ms<=15000,"activation retains fixed native budget");return true end,
    stop=function()stop_count=stop_count+1;native_active=false;return true end,
    status=function()return {active=native_active,pending=pending,pending_role=pending_role,reason="capture budget",entries=2,unmatched=0,discarded=0,
        proven_thread_id=123,foreign_callbacks_process_total=4294967295,same_thread_unavailable_process_total=0,
        unknown_thread_callbacks_process_total=-1,last_refused_caller_admission="would_block",last_refused_caller_admission_available=true}end,
    read=function()local r=queued;queued={};return r end,
    mark=function(s)mark_count=mark_count+1;T.check(s.role==1 or s.role==2,"Lua POST marks explicit native function kind");return nil,"no active exact entry" end}
native.prepare=function(s)native.begin(s);native_active=false;return true,begin_count end
native.activate=function()native_active=true;return true end
local dev=true
local current=scope
local observer=M.new({developer=function()return dev end,native=function()return native end,
    clock_ms=function()return fixture_at end,
    snapshot=function()resolves=resolves+1;return current and clone(current),nil,{local_ms=fixture_at},true end,
    register=function(path,second,third)T.check(type(second)=="function" and third==nil,"only pinned Blueprint second callback is registered");hooks[path]=second;return 1,1 end,
    log=function(fmt,...)logs[#logs+1]=string.format(fmt,...)end,emit=function(r)emitted[#emitted+1]=r end})
observer.tick();T.check(begin_count==0,"default idle does not enroll native objects")
dev=false;observer.command("2 r 15");T.check(begin_count==0,"nondeveloper control cannot enroll")
dev=true;observer.command("2 r 16");T.check(begin_count==0,"oversize duration refuses before native begin")
observer.command("2 r 15");T.check(begin_count==1,"explicit bounded activation enrolls once")
T.check(not native_active,"costly preparation keeps native observation disabled")
fixture_at=fixture_at+33;observer.tick()
local function param(a)return {get=function()return {IsValid=function()return true end,GetAddress=function()return a end}end}end
local dcd="/Game/Character/Blueprints/Willie_BP.Willie_BP_C:Deal Complex Damage"
local gd="/Game/Character/Blueprints/Willie_BP.Willie_BP_C:Get Damage"
local args={};for i=1,14 do args[i]=param(100+i)end;args[15]=param(4)
hooks[dcd](param(2),table.unpack(args));pending_role=2;hooks[gd](param(2),table.unpack(args));pending_role=1
T.check(mark_count==2,"actual registered callbacks unwrap SDK input15 and preserve explicit function kind")
args[15]=param(999);hooks[dcd](param(2),table.unpack(args));T.check(mark_count==2,"different cutting Box never marks older pending entry")
args[15]=param(4);hooks[dcd](param(999),table.unpack(args));T.check(mark_count==2,"different victim never marks enrolled entry")
queued={{qualified=false,reason="lifetime unavailable",pre_available=true,post_available=true,before={6,1,10},after={20,1,25}}}
observer.tick();T.check(#emitted==1 and emitted[1].qualified==false,"serial-zero diagnostic survives drain without becoming qualified")
current=clone(scope);current.owner_life=8;observer.tick()
T.check(stop_count==1,"owner life change cancels even when victim tuple stays identical")
hooks[dcd](param(2),table.unpack(args));T.check(mark_count==2,"cancelled observer performs no callback object reads or marks")
current=clone(scope);observer.command("2 r 1");current.mesh.address=33;observer.tick()
T.check(stop_count==2,"same pawn mesh replacement cancels exact native scope")
current=clone(scope);observer.command("2 r 1");current=nil;observer.drop()
T.check(stop_count==3,"world leave stops scalar observer without touching cached UObjects")
T.check(logs[#logs-1]:find("BOXOBS_STATUS stage=stop",1,true) and logs[#logs-1]:find("foreign_callbacks_process_total=4294967295",1,true),
    "world leave persists copied process totals before native stop without a scope resolver")
current=clone(scope);observer.command("2 r 1");fixture_at=fixture_at+33;observer.tick();native_active=false;observer.tick()
T.check(logs[#logs]:find("BOXOBS finished",1,true)~=nil,"native expiry/budget completion is reported explicitly")
T.check(logs[#logs]:find("foreign_callbacks_process_total=4294967295",1,true) and logs[#logs]:find("same_thread_unavailable_process_total=0",1,true)
    and logs[#logs]:find("unknown_thread_callbacks_process_total=unavailable",1,true),"bounded process counters preserve valid zero and saturation, reject invalid values")
T.check(logs[#logs]:find("last_refused_caller_admission=would_block",1,true) and logs[#logs]:find("foreign_callback_targets_known=false",1,true),
    "copied admission result is persisted without implying skipped callback target coverage")
local before_resolves=resolves;hooks[dcd](param(2),table.unpack(args))
T.check(resolves==before_resolves,"finished native budget skips full callback resolver")
local foreign={};for i=1,33 do foreign[i]={}end;queued=foreign
observer.command("2 r 1");fixture_at=fixture_at+33;observer.tick();observer.tick();T.check(#emitted==1,"oversize native readback is refused without truncation")
observer.command("off")
local registrations=0
local ambiguous=M.new({developer=function()return true end,native=function()return native end,snapshot=function()return clone(scope),nil,nil,true end,
    register=function()registrations=registrations+1;return nil,nil end,log=function()end,emit=function()end})
ambiguous.command("2 r 1");ambiguous.command("2 r 1")
T.check(registrations==1,"ambiguous successful hook submission is never retried")
local signed_registrations=0
local signed=M.new({developer=function()return true end,native=function()return native end,snapshot=function()return clone(scope),nil,nil,true end,
    register=function()signed_registrations=signed_registrations+1;return -2147483648,-2147483648 end,log=function()end,emit=function()end})
signed.command("2 r 1")
T.check(signed_registrations==2,"equal signed int32 Blueprint IDs are accepted")
signed.command("off")

-- Optional timing must measure each stage independently and never control enrollment.
local function timed_fixture(clock_throw,log_throw)
    local at,rows,callbacks=1000,{},{}
    local counts={begin=0,snapshot=0,mark=0,activate=0}
    local n={begin=function()counts.begin=counts.begin+1;at=at+40;return true end,stop=function()return true end,
        status=function()return {active=true,pending=1,pending_role=1}end,read=function()return {}end,
        mark=function()counts.mark=counts.mark+1 end}
    n.prepare=function()n.begin();return true,counts.begin end
    n.activate=function()counts.activate=counts.activate+1;return true end
    local o=M.new({developer=function()return true end,native=function()return n end,
        clock_ms=function()if clock_throw then error("clock unavailable")end;return at end,
        snapshot=function()counts.snapshot=counts.snapshot+1;at=at+7;return clone(scope),nil,
            {local_ms=counts.snapshot==1 and 999 or at,sample_now_ms=at,age_ms=counts.snapshot==1 and at-999 or 0,generation=17},true end,
        register=function(path,f)callbacks[path]=f;at=at+3;return 1,1 end,
        log=function(fmt,...)
            if log_throw and fmt:find("BOXOBS_TIMING",1,true)then error("optional log unavailable")end
            rows[#rows+1]=string.format(fmt,...)
        end,emit=function()end})
    return o,counts,rows,callbacks
end
local timed,tc,tr,th=timed_fixture(false,false)
timed.command("2 r 1")
local timing_row=tr[#tr-1]or ""
T.check(tc.begin==1 and timing_row:find("snapshot_ms=7 snapshot_ms_available=true",1,true)
    and timing_row:find("begin_ms=40 begin_ms_available=true",1,true)
    and timing_row:find("install_ms=6 install_ms_available=true",1,true),"timing separates snapshot, native enrollment and hook registration costs")
T.check(timing_row:find("command_enter_ms=1000",1,true) and timing_row:find("playback_local_ms=999",1,true)
    and timing_row:find("playback_generation=17 playback_generation_available=true",1,true),"timing preserves original same-read sample and generation without refreshing its timestamp")
timed.tick();timed.tick()
T.check(tc.activate==1 and tr[#tr]:find("stage=first_tick",1,true) and tr[#tr]:find("fresh_ms=7 fresh_ms_available=true",1,true),
    "post-prepare application activates before the first active freshness interval")
timed,tc,tr,th=timed_fixture(false,true)
timed.command("2 r 1");timed.tick();th[dcd](param(2),table.unpack(args))
T.check(tc.begin==1 and tc.activate==1 and tc.mark==1 and tc.snapshot==3,"throwing optional timing log cannot interrupt preparation, activation or first native marker")
timed,tc,tr,th=timed_fixture(true,false)
timed.command("2 r 1");timed.tick()
T.check(tc.begin==1 and tc.activate==0 and tr[#tr]:find("preparation_clock_unavailable",1,true),
    "missing functional cutoff clock refuses activation after bounded preparation")

-- Staging never promotes stale context-only evidence or refreshes its applied timestamp.
local function staged_fixture(drop_stage)
    local state={at=1000,stamp=950,eligible=true,scope=clone(scope),prepares=0,activates=0,stops=0,registers=0}
    local o
    local n={begin=function()end,prepare=function()
        state.prepares=state.prepares+1;state.at=state.at+239
        if drop_stage=="prepare" then o.drop()end
        return true,state.prepares
    end,activate=function()
        state.activates=state.activates+1
        if drop_stage=="activate" then o.drop()end
        return true
    end,stop=function()state.stops=state.stops+1;return true end,read=function()return {}end,
        status=function()return {active=true}end,mark=function()end}
    o=M.new({developer=function()return true end,native=function()return n end,clock_ms=function()return state.at end,
        snapshot=function(_,_,is_pending)
            local eligible=state.eligible;if not is_pending then eligible=true end
            return clone(state.scope),is_pending and state.eligible==false and "playback_timeage" or nil,
                {local_ms=state.stamp},eligible
        end,register=function()
            state.registers=state.registers+1
            if drop_stage=="install" then o.drop()end
            return 1,1
        end,log=function()end,emit=function()end})
    return o,state
end
local staged,ss=staged_fixture();staged.command("2 r 1");ss.eligible=false;staged.tick()
T.check(ss.prepares==1 and ss.activates==0 and ss.stops==0 and ss.stamp==950,"stale complete pending context waits with its original applied timestamp and native capture disabled")
ss.eligible=true;ss.stamp=ss.at;staged.tick()
T.check(ss.activates==0,"an applied timestamp equal to the post-install cutoff cannot activate")
ss.at=ss.at+33;ss.stamp=ss.at;staged.tick()
T.check(ss.activates==1,"only a strictly newer qualified applied sample activates prepared native capture")
staged,ss=staged_fixture();staged.command("2 r 1");ss.eligible=false;ss.scope.box.address=44;staged.tick()
T.check(ss.activates==0 and ss.stops==1,"held Box replacement is fatal even while stale pending context would otherwise wait")
staged,ss=staged_fixture();staged.command("2 r 1");ss.eligible="false";staged.tick()
T.check(ss.activates==0 and ss.stops==1,"nonboolean eligibility never promotes context-only pending data")
staged,ss=staged_fixture();staged.command("2 r 1");ss.at=ss.at+251;staged.tick()
T.check(ss.activates==0 and ss.stops==1,"pending wait has one fixed250ms deadline with no restart")
staged,ss=staged_fixture("prepare");staged.command("2 r 1");ss.at=ss.at+33;ss.stamp=ss.at;staged.tick()
T.check(ss.activates==0 and ss.registers==0 and ss.stops==1,"world drop during synchronous prepare invalidates setup before hook installation")
staged,ss=staged_fixture("install");staged.command("2 r 1");ss.at=ss.at+33;ss.stamp=ss.at;staged.tick()
T.check(ss.activates==0 and ss.registers==1 and ss.stops==1,"world drop during hook installation prevents stale pending assignment and further registration")
staged,ss=staged_fixture("activate");staged.command("2 r 1");ss.at=ss.at+33;ss.stamp=ss.at;staged.tick();staged.tick()
T.check(ss.activates==1 and ss.stops==1,"reentrant activation stop cannot republish an active Lua observer")

-- Actual Parity snapshot/enrollment path follows assigned fighter despite a foreign PC Pawn.
local game_logs,loop={},nil
local game_clock=10
local function refused(reason)
    return (game_logs[#game_logs] or ""):find("BOXOBS refused reason="..reason.." authority=false",1,true)~=nil
end
os.getenv=function(k)if k=="HSMP_DEV" or k=="HSMP_INST" then return "1" elseif k=="HSMP_STATE_DIR" then return T.tmpdir("boxobs_")end end
os.clock=function()return game_clock end
print=function(s)game_logs[#game_logs+1]=s end
local world
local function object(name,address,path)
    return {IsValid=function()return true end,GetAddress=function()return address end,
        GetFName=function()return {ToString=function()return name end}end,
        GetFullName=function()return "Object "..(path or "/level."..name)end}
end
world=object("Arena",10,"/Game/Arena.Arena")
local own,pawn,foreign_pawn=object("own",20),object("shown",2),object("foreign",999)
own.Mesh,pawn.Mesh=object("ownmesh",30),object("shownmesh",3)
local weapon,box=object("weapon",5),object("box",4)
for _,a in ipairs({own,pawn,weapon})do a.GetWorld=function()return world end end
pawn.Mesh.GetOwner=function()return pawn end
own["Weapon R"],weapon["Hit Box Collision"]=weapon,box
own.R_GripType_Current=14
box.GetOwner=function()return weapon end
local owner_status={pawn="own",match_id=123,round=2,life=7,spawn_id=512,verified=true}
local shown={peer=2,pawn="shown",match_id=123,round=2,life=3,local_ms=10000,body_ts=9000}
local view={state="live",phase=3,match_id=123,round=2,my_peer_id=1,spawns={[1]={spawn_id=512},[2]={spawn_id=513}}}
local mode={match_id=123,round=2,rows={[1]={life=7,alive=true},[2]={life=3,alive=true}}}
local bus={spawn_status=owner_status,playback={rows={shown}},puppets={rows={{peer=2,name="shown"}}}}
local stream={peer_id=2,match_id=123,round=2,life=3,has_context=true,mode="interp",age=0}
local playback_reads,second_playback=0,nil
HSMP_IPC={bus_table=function(k)
    if k=="playback" then
        playback_reads=playback_reads+1
        if playback_reads==2 and second_playback then return second_playback()end
    end
    return bus[k]
end,peer_slot=function()return 0 end,peer_play=function(_,out)for k,v in pairs(stream)do out[k]=v end end}
local native_header={sidecar_state="ready",sidecar_hb_age_s=0}
local native_header_fails=false
local cached_healthy={sidecar_state="ready",sidecar_hb_age_s=0}
HSMP_IPC.N={ipc_info=function()if native_header_fails then error("current header unavailable")end;return native_header end}
HSMP_IPC.refresh_info=function()return cached_healthy end
local guard={key="world#1",check=function()return true end,settled=function()return true end,pc=function()return {IsValid=function()return true end,Pawn=foreign_pawn}end,
    world=function()return world end,on_drop=function()end,ai_pawn=function()return nil end}
package.preload.UEHelpers=function()return {}end
package.preload.hsmp_wg=function()return {new=function()return guard end}end
package.preload.hsmp_ipc=function()return {init=function()end}end
local link_connected,heartbeat_fresh=true,true
local session={status="connected",poll=function(self,force)T.check(force==true,"Box liveness forces current link poll");self.status=link_connected and "connected" or "disconnected" end,
    live=function(self)return self.status=="connected" and heartbeat_fresh end}
package.preload.hsmp_session=function()return {new=function(opts)
    if opts.ipc then
        local scoped={poll=session.poll,status="connected"}
        scoped.live=function(self)
            local info=opts.ipc.refresh_info(false)
            if not session.live(self) or type(info)~="table" then return false end
            local st=info.sidecar_state
            if st~=nil and st~="ready" and st~=2 then return false end
            local age=tonumber(info.sidecar_hb_age_s)
            return age~=nil and age<=5 -- Match actual shared-session semantics; adapter is stricter.
        end
        return scoped
    end
    return session
end,view=function()return view end,mode=function()return mode end}end
local native_modules={{component=box,id=12,child_of=1}}
package.preload.native_weapon_modules=function()return {of=function(w)T.check(w==weapon,"only exact held native weapon is traversed");return native_modules end}end
FindAllOf=function()return {foreign_pawn,own,pawn}end
LoopAsync=function(_,f)loop=f end
RegisterHook=function(_,second)T.check(type(second)=="function","production hook has a real second callback");return 1,1 end
FName=function(s)return s end
local enroll_options,enroll_count,native_activations=nil,0,0
HSMPNative={box_probe={begin=function(s)enroll_options=s;enroll_count=enroll_count+1;return true end,stop=function()return true end,
    status=function()return {active=true}end,read=function()return {}end,mark=function()return true end}}
HSMPNative.box_probe.prepare=function(s)HSMPNative.box_probe.begin(s);return true,enroll_count end
HSMPNative.box_probe.activate=function()native_activations=native_activations+1;return true end
HSMP_PARITY_TEST={}
dofile(T.path("mods/dev/HSMPParity/Scripts/main.lua"))
HSMP_PARITY_TEST.boxobserve("2 r 1")
T.check(enroll_options and enroll_options.pawn.address==2 and enroll_options.box_owner.address==5
    and enroll_options.owner_pawn==20 and enroll_options.box.address==4,"production enrollment binds displayed victim and assigned owner/held Box, never foreign PC")
enroll_options=nil;stream.mode="stale";HSMP_PARITY_TEST.boxobserve("2 r 1")
T.check(enroll_options==nil and refused("peer_stream_mode"),"publication-fresh held source refuses with its stream-mode stage")
stream.mode="interp";owner_status.verified=false;HSMP_PARITY_TEST.boxobserve("2 r 1")
T.check(enroll_options==nil and refused("owner_context"),"unverified assigned fighter reports unavailable owner context before enrollment")
owner_status.verified=true;own.R_GripType_Current=0;HSMP_PARITY_TEST.boxobserve("2 r 1")
T.check(enroll_options==nil and refused("held_grip"),"remembered weapon with absent native grip reports held-grip refusal")
own.R_GripType_Current=14
view.phase=0;HSMP_PARITY_TEST.boxobserve("2 r 1")
T.check(enroll_options==nil and refused("live_phase"),"retained otherwise-valid actors cannot enroll outside Live")
view.phase=3;shown.local_ms=9000;HSMP_PARITY_TEST.boxobserve("2 r 1")
T.check(enroll_options==nil and refused("playback_timeage"),"stale displayed sample reports exact time-age refusal without enrollment")
shown.local_ms=10000;native_modules=nil;HSMP_PARITY_TEST.boxobserve("2 r 1")
T.check(enroll_options==nil and refused("native_modules"),"unavailable native module traversal is distinguished from missing membership")
native_modules={{component=weapon,id=1,child_of=0}};HSMP_PARITY_TEST.boxobserve("2 r 1")
T.check(enroll_options==nil and refused("box_membership"),"valid traversal without the held Box reports membership refusal")
native_modules={{component=box,id=12,child_of=1}}
local box_full_name=box.GetFullName;box.GetFullName=function()return "Object box"end
HSMP_PARITY_TEST.boxobserve("2 r 1")
T.check(enroll_options==nil and refused("identity_box"),"missing exact native Box path reports scalar-identity refusal")
box.GetFullName=box_full_name
link_connected=false;HSMP_PARITY_TEST.boxobserve("2 r 1")
T.check(enroll_options==nil and refused("session_start"),"disconnected retained records report initial session refusal")
link_connected=true;heartbeat_fresh=false;HSMP_PARITY_TEST.boxobserve("2 r 1")
T.check(enroll_options==nil,"expired heartbeat refuses retained otherwise-fresh displayed tuple")
heartbeat_fresh=true
weapon.GetWorld=function()link_connected=false;return world end
HSMP_PARITY_TEST.boxobserve("2 r 1")
T.check(enroll_options==nil and refused("session_end"),"link loss during held-Box inspection reports final session refusal")
link_connected=true;weapon.GetWorld=function()return world end
native_header.sidecar_hb_age_s=6
HSMP_PARITY_TEST.boxobserve("2 r 1")
T.check(enroll_options==nil and cached_healthy.sidecar_hb_age_s==0,"expired current native header refuses retained healthy facade cache")
native_header.sidecar_hb_age_s=0;native_header_fails=true
HSMP_PARITY_TEST.boxobserve("2 r 1")
T.check(enroll_options==nil and cached_healthy.sidecar_hb_age_s==0,"failed native header read cannot fall back to retained healthy info")
native_header_fails=false;native_header=nil
HSMP_PARITY_TEST.boxobserve("2 r 1")
T.check(enroll_options==nil,"nil current native header is unavailable despite facade healthy cache")
native_header={sidecar_state="ready",sidecar_hb_age_s=0}
weapon.GetWorld=function()native_header.sidecar_hb_age_s=6;return world end
HSMP_PARITY_TEST.boxobserve("2 r 1")
T.check(enroll_options==nil,"native header expiry during component readback fails the end admission check")
native_header.sidecar_hb_age_s=0;weapon.GetWorld=function()native_header_fails=true;return world end
HSMP_PARITY_TEST.boxobserve("2 r 1")
T.check(enroll_options==nil,"failed current native header at final check cannot borrow the healthy start header")
native_header_fails=false;weapon.GetWorld=function()return world end
for _,info in ipairs({{sidecar_hb_age_s=0},{sidecar_state="ready",sidecar_hb_age_s="0"},
    {sidecar_state="ready",sidecar_hb_age_s=-1},{sidecar_state="ready",sidecar_hb_age_s=0/0},
    {sidecar_state="ready",sidecar_hb_age_s=math.huge},{sidecar_state="ready",sidecar_hb_age_s=-math.huge}})do
    native_header=info;HSMP_PARITY_TEST.boxobserve("2 r 1")
    T.check(enroll_options==nil,"malformed native ready/heartbeat fields remain unavailable despite permissive shared predicate")
end
native_header={sidecar_state=2,sidecar_hb_age_s=0};mode.rows[2].life=4
HSMP_PARITY_TEST.boxobserve("2 r 1")
T.check(enroll_options==nil,"current Mode new life rejects retained old placement/playback without a view cadence fallback")
mode.rows[2].life=3
weapon.GetWorld=function()owner_status.life=8;return world end
HSMP_PARITY_TEST.boxobserve("2 r 1")
T.check(enroll_options==nil and refused("revalidation"),"owner context change during native inspection reports revalidation refusal")
owner_status.life=7;weapon.GetWorld=function()return world end
HSMP_PARITY_TEST.boxobserve("2 r 1")
T.check(enroll_options~=nil,"current numeric ready state is accepted with complete exact context")
game_clock=10.033;shown.local_ms=10033;playback_reads=0;loop()
local control_ns=false
for _,line in ipairs(game_logs)do if line:find("BOXOBS started",1,true) and line:find("inst=1",1,true) then control_ns=true end end
T.check(control_ns,"Box control logs include public instance namespace")
HSMPNative.box_probe.read=function()return {{id=1,qualified=false,authority=false,world={address=10}}}end
loop()
local pair_ns=false
for _,line in ipairs(game_logs)do if line:find("BOXOBS_PAIR",1,true) and line:find("inst=1",1,true)
    and line:find('"instance":"1"',1,true) then pair_ns=true end end
T.check(pair_ns,"copied Box pair JSON and log line retain the same instance namespace")
game_clock=10;shown.local_ms=10000

-- The first bus read proves the displayed snapshot; the independently fresh second
-- read can expire/change before a command or active observer tick. Do not borrow
-- the first tuple to hide that refusal, and never inspect a native Box to explain it.
HSMPNative.box_probe.read=function()return {}end
local module_reads=0
package.loaded.native_weapon_modules={of=function(w)
    module_reads=module_reads+1;T.check(w==weapon,"diagnostic keeps the exact held weapon")
    return native_modules
end}
local function playback_diag()
    for i=#game_logs,1,-1 do if game_logs[i]:find("BOXOBS_PLAYBACK ",1,true)then return game_logs[i]end end
end
local function playback_case(changes,reason,detail)
    local actual=clone(shown)
    for k,v in pairs(changes or {})do actual[k]=v end
    game_logs={};enroll_options=nil;playback_reads=0
    second_playback=function()return {rows={actual}}end
    local before=module_reads
    HSMP_PARITY_TEST.boxobserve("2 r 1")
    local d=playback_diag()or ""
    T.check(enroll_options==nil and refused(reason) and module_reads==before,
        "fresh playback "..reason.." fails before native cutting inspection/enrollment")
    T.check(d:find("reason="..reason.." detail="..detail,1,true) and d:find("inst=1",1,true)
        and d:find("expected_round=2 expected_round_available=true",1,true),
        "refusal carries bounded actual/expected context and fixed stage")
    second_playback=nil
    return d
end
local d=playback_case({local_ms=10001},"playback_timefuture","future")
T.check(d:find("local_ms=10001 local_ms_available=true now=10000 now_available=true age_ms=-1",1,true),
    "future time logs actual signed age without clamping")
d=playback_case({local_ms=9749},"playback_timeage","age_over250")
T.check(d:find("age_ms=251 age_available=true",1,true),"251ms fails the unchanged250ms limit")
d=playback_case({local_ms="10000"},"playback_unavailable","local_ms_unavailable")
T.check(d:find("local_ms=unknown local_ms_available=false",1,true) and d:find("age_ms=unknown age_available=false",1,true),
    "nonnumeric time remains independently unavailable without default zero")
d=playback_case({pawn="new-pawn"},"playback_pawn","field_changed")
T.check(d:find('actual_pawn="new-pawn" actual_pawn_available=true expected_pawn="shown"',1,true),
    "pawn refusal records actual replacement separately from expected assigned pawn")
d=playback_case({pawn="shown\n\r\t"..string.char(0,1,127).."tail"},"playback_pawn","field_changed")
T.check(not d:gsub("\n$",""):find("%c") and d:find("x0A",1,true) and d:find("x0D",1,true),
    "newline CR and control bytes stay escaped on one physical bounded row; full original scalar still refuses")
d=playback_case({match_id=0},"playback_match","field_changed")
T.check(d:find("actual_match_id=0 actual_match_id_available=true expected_match_id=123",1,true),
    "actual zero match is available data, never replaced with expected or unknown")
playback_case({round=3},"playback_round","field_changed")
playback_case({life=4},"playback_life","field_changed")
d=playback_case({life=false},"playback_life","field_changed")
T.check(d:find("actual_life=unknown actual_life_available=false expected_life=3 expected_life_available=true",1,true),
    "nonnumeric life remains unknown separately from the proved expected life")
d=playback_case({local_ms=math.huge},"playback_timefuture","future")
T.check(d:find("local_ms=unknown local_ms_available=false",1,true) and d:find("age_ms=unknown age_available=false",1,true),
    "nonfinite time keeps the existing future predicate but diagnostic arithmetic remains unknown")
d=playback_case({local_ms=0/0,life=4},"playback_life","field_changed")
T.check(d:find("local_ms=unknown local_ms_available=false",1,true) and d:find("age_ms=unknown age_available=false",1,true),
    "typed logging does not invent a new eligibility predicate for NaN or fabricate its age")
game_logs={};enroll_options=nil;playback_reads=0
second_playback=function()local p=clone(shown);p.life=nil;return {rows={p}}end
HSMP_PARITY_TEST.boxobserve("2 r 1")
d=playback_diag()or ""
T.check(enroll_options==nil and refused("playback_life") and d:find("detail=field_missing",1,true)
    and d:find("actual_life=unknown actual_life_available=false",1,true),"missing actual life is distinguished from changed or nonnumeric life")
game_logs={};enroll_options=nil;playback_reads=0
second_playback=function()error("private exception text")end
HSMP_PARITY_TEST.boxobserve("2 r 1")
d=playback_diag()or ""
T.check(enroll_options==nil and refused("playback_unavailable") and d:find("detail=read_failed",1,true)
    and not d:find("private exception",1,true),"read exception reports fixed stage without leaking exception text")
game_logs={};enroll_options=nil;playback_reads=0
second_playback=function()return {rows={}}end
HSMP_PARITY_TEST.boxobserve("2 r 1")
d=playback_diag()or ""
T.check(enroll_options==nil and refused("playback_unavailable") and d:find("detail=row_missing",1,true)
    and d:find("actual_pawn=unknown actual_pawn_available=false",1,true),"missing second row is distinguished from a changed pawn")
second_playback=nil;playback_reads=0;enroll_options=nil;shown.local_ms=9750
HSMP_PARITY_TEST.boxobserve("2 r 1")
T.check(enroll_options~=nil,"exact250ms boundary preserves successful original admission")
local pending_activations=native_activations
game_clock=10.033;playback_reads=0;game_logs={}
second_playback=function()local p=clone(shown);p.local_ms=9700;return {rows={p}}end
local pending_modules=module_reads;loop()
local pending_stopped=false
for _,s in ipairs(game_logs)do if s:find("BOXOBS stopped",1,true) then pending_stopped=true end end
T.check(native_activations==pending_activations and not pending_stopped and module_reads>pending_modules,
    "production stale pending row remains context-only while exact held Box is freshly audited")
playback_reads=0;game_logs={}
second_playback=function()local p=clone(shown);p.local_ms=9700;p.life=4;return {rows={p}}end
loop()
local wrong_life_stopped=false
for _,s in ipairs(game_logs)do if s:find("BOXOBS stopped reason=playback_life",1,true) then wrong_life_stopped=true end end
T.check(native_activations==pending_activations and wrong_life_stopped,
    "changed actual life behind a stale timestamp is fatal rather than waiting on age")
game_clock=10;second_playback=nil
shown.local_ms=10000
-- Reproduce enrollment followed46ms later by stale second playback, while the
-- displayed first snapshot and exact native identities are still unchanged.
playback_reads=0;HSMP_PARITY_TEST.boxobserve("2 r 1")
game_clock=10.016;shown.local_ms=10016;playback_reads=0;loop()
game_clock=10.046;playback_reads=0;game_logs={}
second_playback=function()local p=clone(shown);p.local_ms=9700;return {rows={p}}end
loop()
d=playback_diag()or ""
local precise_stop=false
for _,s in ipairs(game_logs)do if s:find("BOXOBS stopped reason=playback_timeage",1,true)then precise_stop=true end end
T.check(precise_stop and d:find("local_ms=9700",1,true) and d:find("age_ms=346",1,true),
    "active observer stops on the precise actual stale timestamp after enrollment")
game_clock=10;shown.local_ms=10000;second_playback=nil
-- Fixed total diagnostic budget and bounded bytes, even under repeated commands.
local diag_rows,max_bytes,enroll_before=0,0,enroll_count
for _=1,50 do
    playback_reads=0;second_playback=function()local p=clone(shown);p.pawn=string.rep("p",4096);return {rows={p}}end
    local from=#game_logs+1;HSMP_PARITY_TEST.boxobserve("2 r 1")
    for i=from,#game_logs do if game_logs[i]:find("BOXOBS_PLAYBACK ",1,true)then
        diag_rows=diag_rows+1;max_bytes=math.max(max_bytes,#game_logs[i])
    end end
end
T.check(diag_rows>0 and diag_rows<=32 and max_bytes<1600,"refusal records and formatted row bytes remain bounded")
T.check(enroll_count==enroll_before and refused("playback_pawn"),"exhausted diagnostic budget still refuses; no new native enrollment")
