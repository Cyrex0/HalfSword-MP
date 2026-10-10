local Role = dofile("mods/shared/hsmp_runtime_role.lua")
local Control = dofile("mods/HSMPMatch/Scripts/headless_control.lua")
local Director = dofile("mods/HSMPMatch/Scripts/director.lua")
local Prepare = dofile("mods/HSMPMatch/Scripts/headless_prepare.lua")
local Boundary = dofile("mods/HSMPMatch/Scripts/headless_sample_boundary.lua")
local SpawnDiagnostics = dofile("mods/HSMPMatch/Scripts/headless_spawn_diagnostics.lua")
local SourceLifecycle = dofile("mods/HSMPMatch/Scripts/headless_source_lifecycle.lua")
-- Offline native-facts stand-in: each fixture supplies its observed team and
-- scalar world address explicitly; this does not derive game defaults.
local function roster_fixture(env,directory,world_address)
    env.directory=function()return directory end
    env.addresses=function(binding)return {world=world_address,pawn=binding.pawn_address,controller=binding.pc_address}end
    env.roster_facts=function(meta,bindings)
        assert(meta.epoch==directory.epoch and meta.dir_seq==directory.seq and #bindings==#directory.entities)
        local facts={epoch=meta.epoch,base_dir_seq=meta.dir_seq,ack_dir_seq=meta.dir_seq,entities={}}
        for i,row in ipairs(directory.entities)do
            assert(row.team_known==true and row.team~=nil)
            local copied={team=row.team}
            for _,field in ipairs({"epoch","id","incarnation","slot","kind","controller"})do copied[field]=row[field]end
            facts.entities[i]=copied
        end
        return true,facts
    end
    return env
end
T.check(T.eq(Role.parse(nil), "client"), "existing launches stay clients")
T.check(T.eq(Role.parse("native_worker"), "native_worker"), "explicit worker role")
T.check(T.eq(Role.parse("native-workr"), "invalid"), "unknown role refuses execution")

local now, running, scope, calls = 1000, true, "world1:pc0:pawn0", {}
local controls = Control.new({ now_ms = function() return now end, running = function() return running end,
    binding = function(row) return { key = scope, controller = row.controller } end,
    same = function(binding) return binding.key == scope end,
    invoke = function(binding, axes, changed, buttons)
        calls[#calls + 1] = { controller = binding.controller, axes = {table.unpack(axes)}, changed = changed, buttons = buttons }
        return true
    end,
})
local directory = { epoch = 44, seq = 1, entities = {
    {epoch=44,id=1,incarnation=1,owner_peer=10,controller=0,kind=0},
    {epoch=44,id=2,incarnation=1,owner_peer=11,controller=1,kind=0},
    {epoch=44,id=3,incarnation=1,owner_peer=0,controller=255,kind=1},
} }
T.check(controls:set_directory(directory), "native directory accepted")
controls:tick() -- fresh binding, no input yet
local function frame(id, delivery, axes, buttons)
    return {epoch=44,id=id,incarnation=1,delivery_seq=delivery,buttons=buttons or 0,flags=0,axes=axes or {1,0,2,3,1,0,0,0}}
end

do
    local pawn,resolves,prepared,dispatched=10,0,0,{}
    local env={prepare=function()prepared=prepared+1;pawn=11;return true end,
        resolve=function(index)
            resolves=resolves+1
            return {index=index,world_key="native1",pc_address=1,pc_name="PC0",pawn_address=pawn,pawn_name="Willie"..pawn,
                pawn={stale_object=true},pc={stale_object=true},world={stale_object=true}}
        end}
    local row={id=1,incarnation=1,controller=0}
    local binding=Control.fresh_binding(row,env)
    T.check(binding and binding.pawn_address==11 and prepared==1 and resolves==1,
        "actual control helper resolves the replacement pawn after native preparation")
    T.check(not binding.pawn and not binding.pc and not binding.world,"control state retains no UObject from preparation")
    T.check(Control.binding_matches(binding,env),"fresh current possession qualifies its own scalar binding")
    pawn=12
    if Control.binding_matches(binding,env)then dispatched[#dispatched+1]=binding.pawn_address end
    T.check(T.eq(#dispatched,0),"a subsequent possession change prevents all old-pawn input touches")
    env.prepare=function()return false end
    local before=resolves
    T.check(not Control.fresh_binding(row,env) and resolves==before,"failed preparation stops the next controller lookup")
end

-- Use the actual Match entry with a scheduler that ignores callback returns.
-- Cancellation reentry must also observe the latch before invoking start.
for _, branch in ipairs({"worker","presentation"}) do
    local callback, starts, cancelled, queued = nil, 0, 0, 0
    local fake = setmetatable({debug=debug,pcall=pcall,type=type,
        require=function(name)
            if name=="hsmp_runtime_role" then return {worker=function()return branch=="worker"end,
                presentation=function()return branch=="presentation"end,client=function()return false end} end
            if name=="headless_worker" or name=="native_client" then return {start=function()
                T.check(cancelled==1,"Match cancels "..branch.." startup loop before adapter initialization")
                starts=starts+1
            end} end
            error("unexpected client import "..name)
        end,
        LoopInGameThreadWithDelay=function(_,fn)queued=queued+1;callback=fn;return 77 end,
        CancelDelayedAction=function(handle)
            T.check(handle==77,"Match cancels its exact startup handle")
            cancelled=cancelled+1;callback()
        end,
    },{__index=function(_,key)error("Match accessed client API "..key)end})
    assert(loadfile("mods/HSMPMatch/Scripts/main.lua","t",fake))()
    T.check(starts==0 and queued==1,"Match defers "..branch.." initialization onto the game thread")
    for _=1,5 do callback() end
    T.check(starts==1 and cancelled==1,"ignored loop return cannot repeat "..branch.." initialization")
end

-- Exercise the actual worker adapter while the world is unavailable. Native
-- admission precedes host/watchdog calls; refusals retain their exact reason.
for _, parent_result in ipairs({"alive","refused","dead"}) do
    local callback,frames,installs,loops,hosts,stops,quits= nil,0,0,0,0,0,0
    local lines={}
    local N={worker_input=function()return true end,
        host_parent_alive=function()
            T.check(frames>0,"native admission precedes supervisor watchdog")
            if parent_result=="refused"then return nil,"wrong thread"end
            return parent_result=="alive"
        end,
        host_start=function()
            T.check(frames>0,"native admission precedes embedded endpoint start")
            hosts=hosts+1;return true
        end,
        host_stop=function()stops=stops+1;return true end,
    }
    local wg={check=function()return false end,settled=function()return false end,on_drop=function()end}
    local modules={hsmp_runtime_role={worker=function()return true end},UEHelpers={},
        hsmp_wg={new=function()return wg end},hsmp_ipc={N=N,init=function()end,frame=function()frames=frames+1 end},
        hsmp_saveguard={install=function()installs=installs+1 end,set_active=function()end,tick=function()end},
        director={make_ue_env=function()return {quit_native_worker=function()quits=quits+1;return true end}end,
            new_native_worker=function()return {state="boot",tick=function()return false end}end},
        headless_control=Control,headless_prepare=Prepare,hsmp_pose_config={},headless_sample_boundary=Boundary,headless_spawn_diagnostics=SpawnDiagnostics}
    local fake=setmetatable({debug=debug,os={getenv=function()return nil end,clock=function()return 1 end},
        require=function(name)if modules[name]then return modules[name]end;error("optional module absent")end,
        dofile=function()error("optional module absent")end,
        print=function(line)lines[#lines+1]=line end,
        LoopInGameThreadWithDelay=function(_,fn)loops=loops+1;callback=fn;return 81 end,
        CancelDelayedAction=function(handle)assert(handle==81);callback()end,
    },{__index=_G})
    local worker=assert(loadfile("mods/HSMPMatch/Scripts/headless_worker.lua","t",fake))()
    worker.start();worker.start()
    T.check(installs==1 and loops==1 and frames==1,"worker singleton prevents repeated frame, save hooks, and tick loops")
    callback();callback()
    local logs=table.concat(lines,"\n")
    if parent_result=="alive"then
        T.check(hosts==1 and stops==0 and quits==0,"living supervisor starts one embedded endpoint")
    elseif parent_result=="refused"then
        T.check(hosts==0 and stops==1 and quits==1 and logs:find("state=error",1,true)
            and logs:find("native supervisor check refused: wrong thread",1,true)
            and not logs:find("native supervisor exited",1,true),"nil watchdog refusal preserves fault and quits once")
    else
        T.check(hosts==0 and stops==1 and quits==1 and logs:find("state=stopped reason=native supervisor exited",1,true),
            "false watchdog result means confirmed supervisor exit and one graceful quit")
    end
end

-- Exercise actual worker construction of the production source adapter. The
-- full native helper semantics are covered by native_source_descriptor/Rust;
-- this integration cannot hide missing wiring behind capture_render mocks.
do
    local Adapter=dofile("mods/HSMPMatch/Scripts/native_source_adapter.lua")
    for _,missing in ipairs({false,"native_source_scope_begin","native_source_scope_keep","native_source_scope_resolve","native_source_scope_end","native_source_scope_spline_profile","native_source_scope_vertex_state","native_source_roster_facts"})do
        local callback,options,reason,frames,game_thread=nil,nil,nil,0,false
        local world={IsValid=function()return true end}
        local token={key="native#2",drops=0}
        local wg={key=token.key,drops=0,check=function()return true end,settled=function()return true end,
            world=function()return world end,token=function()return token end,same=function(t)return t==token end,on_drop=function()end}
        local N={worker_input=function()return true end,host_start=function()
            assert(game_thread and frames>0,"endpoint must retain game-thread admission");return true end,
            host_directory=function()return {epoch=math.mininteger+123,seq=11,entities={}}end,host_inputs=function()return {}end,
            sample_config=function()return true end,host_describe=function()error("unqualified descriptor",0)end,
            native_capture_render=function()error("unqualified render",0)end,native_sample_world=function()error("unqualified core",0)end}
        for _,name in ipairs({"native_source_scope_begin","native_source_scope_keep","native_source_scope_resolve","native_source_scope_end","native_source_scope_spline_profile","native_source_scope_vertex_state","native_source_roster_facts"})do
            N[name]=function()error("scope helper cannot run before qualified source capture",0)end
        end
        local profile={position_count=3,rotation_count=2,scale_count=2,reparam_count=11,metadata_null=true}
        N.native_source_scope_spline_profile=function(scope_id,handle_id)
            assert(game_thread and frames>0,"profile forwarding must retain game-thread admission")
            assert(scope_id==71,"raw profile call must retain the original scalar scope")
            if handle_id==72 then return profile end
            assert(handle_id==73,"raw profile call must retain the original scalar handle")
            return nil,"original spline handle changed"
        end
        local native_asset={state="native_asset",lod_info_count=1,no_override=true}
        local captured_required={state="captured_required",lod_info_count=1,no_override=false}
        N.native_source_scope_vertex_state=function(scope_id,handle_id)
            assert(game_thread and frames>0,"vertex-state forwarding must retain game-thread admission")
            assert(scope_id==81,"raw vertex-state call must retain the original scalar scope")
            if handle_id==82 then return native_asset end
            if handle_id==83 then return captured_required end
            assert(handle_id==84,"raw vertex-state call must retain the original scalar handle")
            return nil,"original static vertex handle changed"
        end
        if missing then N[missing]=nil end
        local modules={hsmp_runtime_role={worker=function()return true end},UEHelpers={GetGameplayStatics=function()return nil end},
            hsmp_wg={new=function()return wg end},hsmp_ipc={N=N,init=function()end,frame=function()frames=frames+1 end,world_ready=function()end},
            hsmp_saveguard={install=function()end,set_active=function()end,tick=function()end},
            hsmp_log={init=function()end,event=function(name,event)
                if name=="x_native_worker" and event.state=="native_evidence"then reason=event.reason end
            end},director={make_ue_env=function()return {apply_cvars=function()end}end,
                new_native_worker=function()return {state="native_ready",tick=function()return true end}end},
            headless_control={new=function()return {set_directory=function()return true end,tick=function()end}end},
            headless_prepare=Prepare,hsmp_pose_config={},headless_sample_boundary=Boundary,headless_spawn_diagnostics=SpawnDiagnostics,
            native_source_adapter={new=function(opts)options=opts;return Adapter.new(opts)end},
            headless_source_lifecycle={new=function(opts)
                assert(type(opts.capture)=="function" and opts.describe==N.host_describe,"production adapter must reach the normal lifecycle")
                T.check(opts.roster_facts==N.native_source_roster_facts and opts.directory==N.host_directory
                    and type(opts.addresses)=="function","actual worker injects raw native facts, directory ACK and scalar bindings")
                return {ensure=function()return false,"source binding intentionally unavailable"end}
            end}}
        local fake=setmetatable({debug=debug,os={getenv=function()return nil end,clock=function()return 1 end},
            require=function(name)if modules[name]then return modules[name]end;error("optional module absent")end,
            dofile=function()error("optional module absent")end,print=function()end,
            LoopInGameThreadWithDelay=function(_,fn)callback=fn;return 82 end},{__index=_G})
        assert(loadfile("mods/HSMPMatch/Scripts/headless_worker.lua","t",fake))().start()
        game_thread=true;callback();game_thread=false
        if missing then
            T.check(options==nil and reason=="native source identity API unavailable: "..missing,
                "worker explicitly refuses missing production helper "..missing)
        else
            T.check(options and options.capture_render==nil and options.WG==wg and type(options.resolve)=="function",
                "actual production adapter receives the guarded source resolver without a render override")
            local scope=options and options.source_scope
            T.check(scope and scope.begin==N.native_source_scope_begin and scope.keep==N.native_source_scope_keep
                and scope.resolve==N.native_source_scope_resolve and scope.finish==N.native_source_scope_end
                and scope.profile==N.native_source_scope_spline_profile and scope.vertex_state==N.native_source_scope_vertex_state,
                "worker injects all six exact native scope functions as raw dot calls")
            game_thread=true
            local value,why=scope.profile(71,72)
            T.check(value==profile and why==nil,"raw worker profile forwarding preserves the actual copied result")
            value,why=scope.profile(71,73)
            T.check(value==nil and why=="original spline handle changed","raw worker profile forwarding preserves the exact native refusal")
            value,why=scope.vertex_state(81,82)
            T.check(value==native_asset and why==nil,"raw worker vertex-state forwarding preserves the exact all-null native-asset proof")
            value,why=scope.vertex_state(81,83)
            T.check(value==captured_required and why==nil,"raw worker vertex-state forwarding never relabels a present override as native asset")
            value,why=scope.vertex_state(81,84)
            T.check(value==nil and why=="original static vertex handle changed","raw worker vertex-state forwarding preserves the exact native refusal")
            game_thread=false
            T.check(reason=="source binding intentionally unavailable","qualified source refusal cannot publish a guessed frame")
        end
    end
end

-- Reach the real numeric resolver through the worker's production adapter
-- options. The UObject mock records every old-wrapper touch after callbacks;
-- no mocked resolve() can conceal another indexed lookup or stale read.
do
    local callback,options,lookups,same_calls,change,changed,old_reads=nil,nil,0,0,nil,false,{}
    local token={key="native_resolve#2",drops=0}
    local world,pc,pawn,retired,fields={},{},{},{},{}
    local function touched(object,field)
        if retired[object]then old_reads[#old_reads+1]=field;error("old wrapper read: "..field,0)end
    end
    local function identity(object,address,name,class)
        fields[object]={}
        setmetatable(object,{__index=function(_,field)
            if field=="Pawn"or field=="Controller"then touched(object,field);return fields[object][field]end
        end,__newindex=function(_,field,value)
            if field=="Pawn"or field=="Controller"then fields[object][field]=value else rawset(object,field,value)end
        end})
        object.IsValid=function()touched(object,"IsValid");return true end
        object.GetAddress=function()touched(object,"GetAddress");return address end
        object.GetFName=function()touched(object,"GetFName");return {ToString=function()return name end}end
        object.GetClass=function()touched(object,"GetClass");return {GetFName=function()return {ToString=function()return class end}end}end
    end
    identity(world,100,"World","World")
    world.GetFullName=function()return "World /Game/Map_Arena_Yard"end
    local function replace(which)
        if changed then return end
        changed=true
        if which=="pc"then
            retired[pc]=true;pc={};identity(pc,12,"PC1_new","PlayerController")
            local object=pc;pc.GetWorld=function()touched(object,"PC.GetWorld");return world end
            pc.Pawn=pawn;pawn.Controller=pc
        elseif which=="pawn"then
            retired[pawn]=true;pawn={};identity(pawn,22,"Willie_new","Willie_BP_C")
            local object=pawn;pawn.GetWorld=function()touched(object,"Pawn.GetWorld");return world end
            pc.Pawn=pawn;pawn.Controller=pc
        else token={key="native_resolve#3",drops=1}end
    end
    local function reset(which)
        change,changed,lookups,same_calls,old_reads=which,false,0,0,{}
        token={key="native_resolve#2",drops=0};retired={};pc={};pawn={}
        identity(pc,11,"PC1","PlayerController");identity(pawn,21,"Willie","Willie_BP_C")
        pc.Pawn=pawn;pawn.Controller=pc
        local original_pc,original_pawn=pc,pawn
        pc.GetWorld=function()
            touched(original_pc,"PC.GetWorld")
            if change=="pc_world"then replace("pc")elseif change=="pc_world_drop"then replace("world")end
            return world
        end
        pawn.GetWorld=function()
            touched(original_pawn,"Pawn.GetWorld")
            if change=="pawn_world"then replace("pawn")elseif change=="pawn_world_drop"then replace("world")end
            return world
        end
    end
    reset()
    local wg={key=token.key,drops=0,check=function()return token.key=="native_resolve#2"end,settled=function()return true end,
        world=function()return world end,token=function()return token end,on_drop=function()end,
        same=function(value)
            same_calls=same_calls+1
            if change=="world_guard_pc"and same_calls==2 then replace("pc")end
            return value==token
        end}
    local gs={IsValid=function()return true end,GetPlayerController=function(_,actual_world,index)
        if index==0 then return nil end -- startup's unrelated diagnostic census
        assert(actual_world==world and index==1,"resolver must retain the requested native world and controller index")
        lookups=lookups+1;return pc
    end}
    local N={worker_input=function()return true end,host_start=function()return true end,
        host_directory=function()return {epoch=44,seq=1,entities={}}end,host_inputs=function()return {}end,
        sample_config=function()return true end,host_describe=function()error("unexpected publish",0)end,
        native_capture_render=function()error("unexpected render",0)end,native_sample_world=function()error("unexpected core",0)end}
    for _,name in ipairs({"native_source_scope_begin","native_source_scope_keep","native_source_scope_resolve","native_source_scope_end","native_source_scope_spline_profile","native_source_scope_vertex_state","native_source_roster_facts"})do N[name]=function()error("unexpected scope",0)end end
    local modules={hsmp_runtime_role={worker=function()return true end},UEHelpers={GetGameplayStatics=function()return gs end},
        hsmp_wg={new=function()return wg end},hsmp_ipc={N=N,init=function()end,frame=function()end,world_ready=function()end},
        hsmp_saveguard={install=function()end,set_active=function()end,tick=function()end},hsmp_log={init=function()end,event=function()end},
        director={make_ue_env=function()return {apply_cvars=function()end}end,new_native_worker=function()return {state="native_ready",tick=function()return true end}end},
        headless_control={new=function()return {set_directory=function()return true end,tick=function()end}end},headless_prepare=Prepare,
        hsmp_pose_config={},headless_sample_boundary=Boundary,headless_spawn_diagnostics=SpawnDiagnostics,
        native_source_adapter={new=function(opts)options=opts;return {capture=function()end}end},
        headless_source_lifecycle={new=function()return {ensure=function()return false,"fixture source not ready"end}end}}
    local fake=setmetatable({debug=debug,os={getenv=function()return nil end,clock=function()return 1 end},print=function()end,
        require=function(name)if modules[name]then return modules[name]end;error("optional module absent")end,
        dofile=function()error("optional module absent")end,FindAllOf=function()return {}end,
        LoopInGameThreadWithDelay=function(_,fn)callback=fn;return 83 end},{__index=_G})
    assert(loadfile("mods/HSMPMatch/Scripts/headless_worker.lua","t",fake))().start();callback()
    assert(options and type(options.resolve)=="function","actual worker must provide its resolver")
    reset()
    local binding=options.resolve(1)
    T.check(binding and binding.pc==pc and binding.pawn==pawn and binding.world==world and binding.world_key==token.key
        and binding.index==1 and binding.pc_address==11 and binding.pc_name=="PC1" and binding.pawn_address==21 and binding.pawn_name=="Willie",
        "real worker resolver returns the exact freshly qualified scalar binding and wrappers")
    T.check(lookups==3,"unchanged real worker resolver performs exactly three indexed controller lookups")
    T.check(same_calls==5 and #old_reads==0,"resolver retains both actor-world checks and their post-callback world guards")
    for _,which in ipairs({"pc_world","pawn_world","pc_world_drop","pawn_world_drop","world_guard_pc"})do
        reset(which)
        T.check(options.resolve(1)==nil,"actual resolver refuses replacement at "..which)
        T.check(changed and #old_reads==0,"actual resolver makes no old-wrapper/property read after "..which)
    end
    reset()
    local described,captured=0,0
    local directory={epoch=44,seq=1,entities={{epoch=44,id=1,incarnation=1,slot=0,controller=1,kind=0,team_known=true,team=7}}}
    local lifecycle=SourceLifecycle.new(roster_fixture({resolve=options.resolve,same=wg.same,now_ms=function()return 1000 end,
        index=function(row)return row.controller end,capture=function()
            captured=captured+1;replace("pawn");return {team=7},{pawn=21}
        end,describe=function()described=described+1;return true end,invalidate=function()end},directory,100))
    T.check(not lifecycle.ensure(directory,token,1) and captured==1 and described==0,
        "actual indexed worker resolver brackets source capture and refuses same-world possession mutation before publish")
    T.check(#old_reads==0,"source post-callback qualification obtains fresh wrappers without touching the old pawn")
end

-- The actual worker loop emits the latest sampling refusal to its own stream.
-- A later successful sample clears the reason without clearing refusal counts.
-- Cancellation reaches the production resolver and Adapter's render pcall;
-- only the component collector itself is replaced with a two-getter fixture.
for _,cancel_kind in ipairs({"stop","dead","refused","ordinary"})do
    local callback,clock,requested,collecting,getter_active=nil,1,false,false,false
    local reads,parent_checks,getters,ends,describes,stops,quits,cancels=0,0,0,0,0,0,0,0
    local terminal
    local function identity(address,name,class)
        return {IsValid=function()return true end,GetAddress=function()return address end,
            GetFName=function()return {ToString=function()return name end}end,
            GetClass=function()return {GetFName=function()return {ToString=function()return class end}end}end}
    end
    local world=identity(100,"World","World");world.GetFullName=function()return "World /Game/Map_Arena_Yard"end
    local pc,pawn=identity(11,"PC1","PlayerController"),identity(21,"Willie","Willie_BP_C")
    pc.Pawn=pawn;pawn.Controller=pc;pawn["Dismemberment In Process"]=false;pawn["Team Int"]=7
    pc.GetWorld=function()return world end;pawn.GetWorld=function()return world end
    local token={key="native_cancel#1",drops=0}
    local wg={key=token.key,drops=0,check=function()return true end,settled=function()return true end,
        token=function()return token end,same=function(t)return t==token end,world=function()return world end,on_drop=function()end}
    local gs={IsValid=function()return true end,GetPlayerController=function(_,w,index)
        assert(w==world);return index==1 and pc or nil
    end}
    local N={worker_input=function()return true end,host_start=function()return true end,
        host_directory=function()return {epoch=44,seq=1,entities={{epoch=44,id=1,incarnation=1,slot=0,kind=0,controller=1,team_known=true,team=7}}}end,
        native_source_roster_facts=function(meta,bindings)
            T.check(meta.epoch==44 and meta.dir_seq==1 and #bindings==1 and bindings[1].world==100
                and bindings[1].pawn==21 and bindings[1].controller==11 and bindings[1].controller_index==1,
                "production lifecycle submits the complete qualified original native binding before capture")
            return true,{epoch=44,base_dir_seq=1,ack_dir_seq=1,entities={{epoch=44,id=1,incarnation=1,slot=0,kind=0,controller=1,team=7}}}
        end,
        host_inputs=function()return {}end,sample_config=function()return true end,
        host_parent_alive=function()
            assert(not getter_active,"cancellation polling must not reenter an active source getter")
            parent_checks=parent_checks+1
            if requested and cancel_kind=="refused"then return nil,"wrong thread"end
            return not(requested and cancel_kind=="dead")
        end,
        host_stop=function()
            T.check(not collecting and not getter_active and ends==1,"endpoint teardown follows collector unwind and original native scope end")
            stops=stops+1;return true
        end,
        host_describe=function()describes=describes+1;return true end,
        native_capture_render=function()error("no complete core is staged",0)end,
        native_sample_world=function()error("no complete core is staged",0)end,
        native_source_scope_begin=function()return 71 end,
        native_source_scope_keep=function()error("fixture retains no component",0)end,
        native_source_scope_resolve=function()error("fixture retains no component",0)end,
        native_source_scope_spline_profile=function()error("fixture contains no spline",0)end,
        native_source_scope_vertex_state=function()error("fixture contains no static mesh",0)end,
        native_source_scope_end=function(handle)
            T.check(handle==71 and collecting and not getter_active,"production Adapter ends its original scope after the collector getter has unwound")
            ends=ends+1;return true
        end}
    local render={capture=function(env)
        env.guard()
        for _=1,20 do env.guard()end
        T.check(reads==1 and parent_checks==1,"frequent source guards share one throttled startup stop/parent poll")
        getter_active=true;getters=getters+1
        clock=1.26;requested=true -- external request appears during this completed getter
        getter_active=false
        env.guard() -- actual Adapter.current -> actual worker resolver
        getters=getters+1 -- must never run after cancellation admission refuses
        return {team=7,components={},bindings={},topology={detached={},gore={},vertex_state="complete"}}
    end}
    local descriptor={capture=function(env)env.pass=1;local recipe=env.render();recipe.team=env.team();return recipe end,
        read_flags=function()return {}end,read_names=function()return {}end}
    local adapter_env=setmetatable({dofile=function(path)
        if path:match("/native_source_descriptor%.lua$")then return descriptor end
        if path:match("/native_source_render%.lua$")then return render end
        error("unexpected adapter dependency: "..path,0)
    end},{__index=_G})
    local Adapter=assert(loadfile("mods/HSMPMatch/Scripts/native_source_adapter.lua","t",adapter_env))()
    local modules={hsmp_runtime_role={worker=function()return true end},UEHelpers={GetGameplayStatics=function()return gs end},
        hsmp_wg={new=function()return wg end},hsmp_ipc={N=N,init=function()end,frame=function()end,world_ready=function()end,
            read=function(path)
                assert(path=="owned.stop" and not getter_active,"only the owned stop file is polled outside the source getter")
                reads=reads+1;return requested and cancel_kind=="stop"and "stop"or false
            end},hsmp_saveguard={install=function()end,set_active=function()end,tick=function()end},
        hsmp_log={init=function()end,event=function(_,fields)if fields.state=="stopped"or fields.state=="error"then terminal=fields end end},
        director={make_ue_env=function()return {apply_cvars=function()end,quit_native_worker=function()
            T.check(not collecting and not getter_active and ends==1,"Director quit follows full source collector unwind")
            quits=quits+1;return true
        end}end,new_native_worker=function()return {state="native_ready",tick=function()return true end}end},
        headless_control={new=function()return {set_directory=function()return true end,tick=function()end,
            stop=function()T.check(not collecting,"controller cleanup cannot run inside the collector");return true end}end},
        headless_prepare=Prepare,hsmp_pose_config={},headless_sample_boundary=Boundary,headless_spawn_diagnostics=SpawnDiagnostics,
        native_source_adapter={new=function(opts)
            local actual=Adapter.new(opts)
            return {capture=function(...)
                collecting=true;local recipe,why=actual.capture(...);collecting=false;return recipe,why
            end}
        end},headless_source_lifecycle=SourceLifecycle}
    local fake=setmetatable({debug=debug,os={getenv=function(key)if key=="HSMP_NATIVE_STOP_FILE"then return "owned.stop"end end,
        clock=function()return clock end},print=function()end,require=function(name)if modules[name]then return modules[name]end;error("optional module absent")end,
        dofile=function()error("optional module absent")end,FindAllOf=function()return {}end,
        LoopInGameThreadWithDelay=function(_,fn)callback=fn;return 84 end,
        CancelDelayedAction=function(handle)assert(handle==84);cancels=cancels+1;callback()end},{__index=_G})
    assert(loadfile("mods/HSMPMatch/Scripts/headless_worker.lua","t",fake))().start()
    callback();callback()
    if cancel_kind=="ordinary"then
        T.check(getters==2 and ends==1 and describes==1 and stops==0 and quits==0 and cancels==0,"unchanged admission completes ordinary source capture and registration without cleanup")
        T.check(reads==2 and parent_checks==2,"ordinary capture polls once per elapsed250ms while preserving native getter guards")
    else
        T.check(getters==1 and ends==1 and describes==0,"midcapture "..cancel_kind.." admission prevents the very next getter and all descriptor registration")
        T.check(stops==1 and quits==1 and cancels==1,"midcapture "..cancel_kind.." cancellation performs normal cleanup exactly once after unwind")
        local reason=cancel_kind=="stop"and "stop requested"or cancel_kind=="dead"and "native supervisor exited"or "native supervisor check refused: wrong thread"
        T.check(terminal and terminal.reason==reason and terminal.state==(cancel_kind=="refused"and "error"or "stopped"),"midcapture cancellation preserves its original normal/fault reason")
    end
end
do
    local callback, clock, refusal, events, phases, sample_calls, epoch = nil, 1, "source vertex colours unavailable", {}, {}, 0, 44
    local N={worker_input=function()return true end,host_start=function()return true end,
        host_directory=function()return {epoch=epoch,seq=1,entities={}}end,host_inputs=function()return {}end,
        sample_config=function()
            T.check(phases[#phases].stage=="sample_config" and phases[#phases].edge=="enter",
                "worker persists configuration entry before invoking native setup")
            return true
        end,native_sample_world=function()
            sample_calls=sample_calls+1
            if sample_calls==1 then T.check(phases[#phases].stage=="canonical_core" and phases[#phases].edge=="enter",
                "actual worker persists canonical entry before invoking native sample")end
            return refusal==nil,refusal
        end,
        native_commit_world=function()
            T.check(phases[#phases].stage=="canonical_publish" and phases[#phases].edge=="enter",
                "actual worker persists publication entry before invoking native commit")
            return true
        end}
    local world={IsValid=function()return true end}
    local wg={key="native#2",check=function()return true end,settled=function()return true end,
        world=function()return world end,token=function()return 1 end,same=function(token)return token==1 end,
        on_drop=function()end}
    local modules={hsmp_runtime_role={worker=function()return true end},UEHelpers={GetGameplayStatics=function()return nil end},
        hsmp_wg={new=function()return wg end},
        hsmp_ipc={N=N,init=function()end,frame=function()end,world_ready=function()end},
        hsmp_log={init=function()end,event=function(name,fields)
            if name=="x_native_worker" and fields.state=="native_evidence" then events[#events+1]=fields end
            if name=="x_native_worker" and fields.state=="native_capture_phase" then phases[#phases+1]=fields end
        end},hsmp_saveguard={install=function()end,set_active=function()end,tick=function()end},
        director={make_ue_env=function()return {apply_cvars=function()end}end,
            new_native_worker=function()return {state="native_ready",tick=function()return true end}end},
        headless_control={new=function()return {set_directory=function()return true end,tick=function()end}end},
        headless_prepare=Prepare,hsmp_pose_config={},headless_sample_boundary=Boundary,headless_spawn_diagnostics=SpawnDiagnostics}
    local fake=setmetatable({debug=debug,os={getenv=function(key)if key=="HSMP_NATIVE_MODE"then return "diagnostic"end end,
        clock=function()return clock end},require=function(name)if modules[name]then return modules[name]end;error("optional module absent")end,
        dofile=function()error("optional module absent")end,print=function()end,
        LoopInGameThreadWithDelay=function(_,fn)callback=fn;return 82 end}, {__index=_G})
    assert(loadfile("mods/HSMPMatch/Scripts/headless_worker.lua","t",fake))().start()
    callback()
    T.check(#events==1 and events[1].reason==refusal and events[1].refused==1 and events[1].sampled==0,
        "authority evidence retains the exact canonical sampling refusal")
    clock=2;refusal="source descriptor retry pending";callback()
    T.check(#events==2 and events[2].reason==events[1].reason and events[2].refused==2,
        "bounded descriptor retry retains the original capture failure in the authority stream")
    clock=3;refusal=nil;callback()
    T.check(#events==3 and events[3].reason=="" and events[3].refused==2 and events[3].sampled==1,
        "successful canonical sampling clears the prior diagnostic reason")
    clock=4;refusal="native render changed";callback()
    T.check(#events==4 and events[4].reason==refusal and events[4].refused==3 and events[4].sampled==1,
        "a later canonical refusal replaces the old cause in the authority stream")
    local core_entries=0
    for _,phase in ipairs(phases)do if phase.stage=="canonical_core" and phase.edge=="enter"then core_entries=core_entries+1 end end
    T.check(core_entries==1 and sample_calls==4,"advancing source frames do not emit per-frame capture diagnostics")
    T.check(phases[#phases].stage=="canonical_publish" and phases[#phases].edge=="exit" and phases[#phases].ok==true
        and phases[#phases].epoch==44 and phases[#phases].dir_seq==1 and phases[#phases].frame_seq==3,
        "worker phase exits contain copied exact source metadata and the returned native result")
    T.check(phases[#phases].epoch_text=="44","phase preserves the original integer epoch as exact signed decimal text")
    clock=5;epoch=math.mininteger+123;callback()
    T.check(phases[#phases].epoch_text=="-9223372036854775685","phase retains all original 64-bit integer bits beyond JSON floating precision")
    T.check(phases[#phases].epoch==epoch,"phase text never changes the native epoch scalar")
    clock=6;epoch=44.0;callback()
    T.check(phases[#phases].epoch_text=="unknown","a floating epoch is explicitly unknown rather than invented integer bits")
end
T.check(controls:receive(frame(1, 1, nil, 5)), "owned human input accepted")
T.check(not controls:receive(frame(3, 1)), "AI cannot receive a player input")
controls:tick()
local applied
for _, call in ipairs(calls) do if call.controller == 0 and call.buttons == 5 then applied = call end end
T.check(applied and applied.axes[1] == 1 and applied.axes[3] == 2 and applied.axes[4] == 3 and applied.changed == 5,
    "movement, mouse and native press edges reach only PC0")
T.check(not controls:receive(frame(1, 1)), "duplicate does not replay mouse or refresh timeout")
T.check(not controls:receive(frame(1, 0)), "zero delivery sequence refused")
calls = {}; now = now + 16; controls:tick()
local held
for _, call in ipairs(calls) do if call.controller == 0 then held = call end end
T.check(held and held.axes[1] == 1 and held.axes[3] == 0 and held.axes[4] == 0 and held.changed == 0,
    "held movement repeats while mouse delta and button edge execute once")
calls = {}; now = 1250; controls:tick()
for _, call in ipairs(calls) do if call.controller == 0 then held = call end end
T.check(held.axes[1] == 0 and held.buttons == 0 and held.changed == 5, "250ms silence releases native buttons and axes")
local stale = frame(1, 2); stale.incarnation = 2
T.check(not controls:receive(stale), "wrong incarnation refused")
stale = frame(1, 2); stale.epoch = 45
T.check(not controls:receive(stale), "wrong authority epoch refused")
T.check(not controls:receive(frame(1, 2, {1,0,0/0,0,0,0,0,0})), "nonfinite axis refused")
T.check(not controls:receive(frame(1, 2, {1,0,101,0,0,0,0,0})), "out-of-contract mouse axis refused")
T.check(controls:receive(frame(1, 2, nil, 1)), "new current input accepted")
scope = "world1:pc0:newpawn"; calls = {}; controls:tick()
for _, call in ipairs(calls) do if call.controller == 0 then held = call end end
T.check(held.axes[1] == 0 and held.buttons == 0, "possession change cannot reuse old pawn input")
T.check(controls:receive(frame(1, 3, nil, 1)), "fresh input after binding change accepted")
controls:tick()
running = false; local before = #calls; controls:tick()
T.check(T.eq(#calls, before), "stopped runtime makes no native calls")
controls:drop(); running = true; controls:tick()
T.check(T.eq(#calls, before), "world drop forgets old objects without releasing through them")
controls:stop()
T.check(not controls:set_directory(directory), "stop refuses new directory")

do
    local applied, binding = {}, "current-pawn"
    local transfer = Control.new({now_ms=function()return 1000 end,running=function()return true end,
        binding=function(row)return {key=binding,controller=row.controller}end,
        same=function(value)return value.key==binding end,
        invoke=function(_,axes,changed,buttons)
            applied[#applied+1]={changed=changed,buttons=buttons,axes=axes};return true
        end})
    local function owned(incarnation,owner,seq)
        return {epoch=44,seq=seq,entities={{epoch=44,id=1,incarnation=incarnation,owner_peer=owner,controller=0,kind=0}}}
    end
    transfer:set_directory(owned(1,10,1));transfer:tick()
    local input=frame(1,1,{1,0,9,8,0,0,0,0},9)
    T.check(transfer:receive(input),"Run and Jump held before native incarnation rotation")
    transfer:tick();applied={}
    transfer:set_directory(owned(2,11,2));transfer:tick()
    T.check(#applied==1 and applied[1].changed==9 and applied[1].buttons==0 and applied[1].axes[1]==0
        and applied[1].axes[3]==0,"same-pawn incarnation and owner rotation releases old held buttons without replaying axes")
    input.incarnation=2;input.delivery_seq=1
    T.check(transfer:receive(input),"new incarnation starts fresh input sequence")
    transfer:tick();applied={}
    transfer:set_directory(owned(2,0,3));transfer:tick()
    T.check(#applied==1 and applied[1].changed==9 and applied[1].buttons==0,
        "owner disconnect releases Run and Jump on the same qualified pawn")
end

do
    local clock,binding,success,calls,advance=1000,"pawn-a",true,{},false
    local ordered=Control.new({ordered=true,now_ms=function()return clock end,running=function()return true end,
        binding=function()return {key=binding}end,same=function(v)return v.key==binding end,
        invoke=function(_,axes,changed,buttons,request)
            calls[#calls+1]={axes={table.unpack(axes)},changed=changed,buttons=buttons,request=request}
            if request and advance then clock=clock+250 end
            return success
        end})
    local own={epoch=44,seq=1,entities={{epoch=44,id=1,incarnation=1,owner_peer=10,controller=0,kind=0}}}
    ordered:set_directory(own);ordered:tick();calls={}
    local function request(seq,buttons)
        local r=frame(1,seq,{1,0,seq,0,0,0,0,0},buttons);r.seq=seq;return r
    end
    T.check(ordered:receive(request(1,1)) and ordered:receive(request(2,0)),"ordered gameplay admits press and release before one tick")
    ordered:tick()
    T.check(#calls==2 and calls[1].changed==1 and calls[1].buttons==1 and calls[2].changed==1 and calls[2].buttons==0,
        "one tick preserves both native press and release in admitted order")
    T.check(calls[1].request.seq==1 and calls[2].request.delivery_seq==2 and calls[1].axes[3]==1 and calls[2].axes[3]==2,
        "each edge carries its exact request identity and consumes its own mouse delta once")
    calls={};ordered:tick()
    T.check(#calls==1 and calls[1].changed==0 and calls[1].axes[1]==1 and calls[1].axes[3]==0 and not calls[1].request,
        "held movement has no repeated edge, mouse delta, or fabricated execution acknowledgement")
    T.check(not ordered:receive(request(2,1)),"ordered delivery duplicates never replay an edge")
    T.check(ordered:receive(request(3,1)),"next ordered request admitted")
    clock=1250;calls={};ordered:tick()
    T.check(#calls==1 and calls[1].buttons==0 and not calls[1].request,"expired queued request is never executed or acknowledged")
    clock=1300;ordered:receive(request(4,1));binding="pawn-b";calls={};ordered:tick()
    T.check(#calls==1 and calls[1].buttons==0 and not calls[1].request,"possession change discards queued old-pawn requests")
    ordered:receive(request(5,1));success=false;calls={};ordered:tick()
    local edges=0;for _,call in ipairs(calls)do if call.request then edges=edges+1 end end
    success=true;calls={};ordered:tick()
    T.check(edges==1 and #calls==1 and not calls[1].request,"incomplete native request is removed before callbacks and never retried")
    local missing=request(6,0);missing.seq=nil
    T.check(not ordered:receive(missing),"ordered gameplay requires an explicit nonzero request sequence")
    ordered:receive(request(6,1));ordered:receive(request(7,0));calls={};advance=true
    ordered:tick()
    T.check(#calls==2 and calls[1].request.seq==6 and not calls[2].request and calls[2].buttons==0 and calls[2].changed==1,
        "native callback exhausting receipt lifetime prevents next queued request ACK and releases held buttons")
end

do
    local callback,old_calls,configs,reason=nil,0,0,nil
    local N={worker_input=function()return true end,host_start=function()return true end,
        host_directory=function()return {epoch=44,seq=1,entities={}}end,host_inputs=function()return {}end,
        sample_config=function()configs=configs+1;return true end,
        native_sample_world=function()old_calls=old_calls+1;return true end}
    local world={IsValid=function()return true end}
    local wg={key="compact-missing",check=function()return true end,settled=function()return true end,
        world=function()return world end,token=function()return 1 end,same=function(v)return v==1 end,on_drop=function()end}
    local modules={hsmp_runtime_role={worker=function()return true end},UEHelpers={GetGameplayStatics=function()return nil end},
        hsmp_wg={new=function()return wg end},hsmp_ipc={N=N,init=function()end,frame=function()end,world_ready=function()end},
        hsmp_saveguard={install=function()end,set_active=function()end,tick=function()end},
        hsmp_log={init=function()end,event=function(name,fields)
            if name=="x_native_worker" and fields.state=="native_evidence"then reason=fields.reason end
        end},director={make_ue_env=function()return {apply_cvars=function()end}end,
            new_native_worker=function()return {state="native_ready",tick=function()return true end}end},
        headless_control={new=function()return {set_directory=function()return true end,tick=function()end}end},
        headless_prepare=Prepare,hsmp_pose_config={},headless_sample_boundary=Boundary,headless_spawn_diagnostics=SpawnDiagnostics}
    local fake=setmetatable({debug=debug,os={getenv=function(k)if k=="HSMP_NATIVE_GAMEPLAY"then return "1"end end,clock=function()return 1 end},
        require=function(name)if modules[name]then return modules[name]end;error("optional module absent")end,
        dofile=function()error("optional module absent")end,print=function()end,
        LoopInGameThreadWithDelay=function(_,fn)callback=fn;return 82 end},{__index=_G})
    assert(loadfile("mods/HSMPMatch/Scripts/headless_worker.lua","t",fake))().start();callback()
    T.check(old_calls==0 and configs==0 and reason=="canonical sampler unavailable",
        "explicit gameplay mode with missing compact API refuses before invoking legacy capture or configuration")
end

do
    local token,current,commits,invalidations={},true,0,0
    local env={same=function(value)return current and value==token end,
        commit=function()commits=commits+1;return true end,
        invalidate=function()invalidations=invalidations+1 end}
    env.sample=function()current=false;return true end
    T.check(not Boundary.sample(env,token,{}),"world loss inside native sample refuses publication")
    T.check(T.eq(commits,0),"staged old-world DTO never commits after reflected world reentry")
    T.check(T.eq(invalidations,1),"world invalidation retries after the native call releases its lock")
    current=true;env.sample=function()return false,"Health unavailable"end
    T.check(not Boundary.sample(env,token,{}),"native sample refusal never commits")
    T.check(T.eq(commits,0),"failed sampling cannot publish previous staged data")
    env.sample=function()return true end
    T.check(not Boundary.sample(env,token,{}),"normal native publication requires the complete render stage")
    env.render=function()return nil,"source cloth unsupported"end
    T.check(not Boundary.sample(env,token,{}),"unsupported source rendering cannot publish a core-only frame")
    T.check(T.eq(commits,0),"render refusal cannot expose a partial source scene")
    env.render=function()current=false;return true end
    T.check(not Boundary.sample(env,token,{}),"world loss inside render capture refuses publication")
    T.check(T.eq(invalidations,2),"render reentry retries invalidation after its native lock releases")
    current=true;env.render=function()return true end
    T.check(Boundary.sample(env,token,{}),"unchanged qualified world commits its staged sample")
    T.check(T.eq(commits,1),"canonical publication occurs only after post-call world qualification")
    env.render=nil;env.allow_core_only=true
    T.check(Boundary.sample(env,token,{}),"explicit diagnostic mode can publish the bootstrap core without client readiness")
end

do
    local phases={}
    local function phase(context,stage,edge,detail)
        phases[#phases+1]={context=context,stage=stage,edge=edge,detail=detail}
    end
    local token={}
    local env={phase=phase,same=function(value)return value==token end,invalidate=function()end,
        sample=function()
            T.check(phases[#phases].stage=="canonical_core" and phases[#phases].edge=="enter",
                "core entry persists before native sampling starts")
            return true
        end,
        render=function()error("simulated blocked native render",0)end,
        commit=function()error("partial source must not publish",0)end}
    local ok=pcall(Boundary.sample,env,token,{epoch=44,dir_seq=7,frame_seq=8})
    T.check(not ok and phases[#phases].stage=="native_render" and phases[#phases].edge=="enter",
        "a nonreturning native stage leaves its exact entry without an invented exit")
    T.check(phases[#phases].context.epoch==44 and phases[#phases].context.dir_seq==7 and phases[#phases].context.frame_seq==8,
        "native phase identity carries the exact source publication generation")
    env.render=function()return true end
    env.commit=function()
        T.check(phases[#phases].stage=="canonical_publish" and phases[#phases].edge=="enter",
            "publish entry persists before invoking native commit")
        return true
    end
    T.check(Boundary.sample(env,token,{epoch=44,dir_seq=7,frame_seq=9}),"phase diagnostics preserve coherent publication")
    T.check(phases[#phases].stage=="canonical_publish" and phases[#phases].edge=="exit" and phases[#phases].detail.ok==true,
        "native publish exit is recorded only after commit returns")
end

-- Two native teams are queued as one roster transaction before any recipe.
-- No unacknowledged directory may be refreshed into an older capture.
do
    local function fixture()
        local f={token={},world=true,pawns={21,22},teams={1,2},queued=0,captured=0,described=0,invalidated=0,clock=0}
        f.directory={epoch=math.mininteger+123,seq=6,entities={
            {epoch=math.mininteger+123,id=1,incarnation=5,slot=0,kind=0,controller=0,owner_peer=11,team_known=false},
            {epoch=math.mininteger+123,id=2,incarnation=5,slot=1,kind=0,controller=1,owner_peer=12,team_known=false}}}
        f.env={same=function(t)return f.world and t==f.token end,now_ms=function()return f.clock end,
            index=function(row)return row.controller end,directory=function()return f.directory end,
            resolve=function(index)if not f.pawns[index+1]then return nil end;return {index=index,world_key="world100",pc_address=11+index,pc_name="PC"..index,
                pawn_address=f.pawns[index+1],pawn_name="Willie"..f.pawns[index+1]}end,
            addresses=function(binding)return {world=100,pawn=binding.pawn_address,controller=binding.pc_address}end,
            invalidate=function()f.invalidated=f.invalidated+1 end,
            roster_facts=function(meta,bindings)
                f.queued=f.queued+1
                T.check(meta.epoch==math.mininteger+123 and math.type(meta.epoch)=="integer" and meta.dir_seq==6,
                    "preflight preserves every signed high-bit epoch bit and the exact base generation")
                T.check(#bindings==2 and bindings[1].controller_index==0 and bindings[2].controller_index==1
                    and bindings[1].world==100 and bindings[1].pawn==21 and bindings[2].pawn==22
                    and bindings[1].controller==11 and bindings[2].controller==12,
                    "preflight submits all original roster bindings as scalar native addresses")
                local facts={epoch=meta.epoch,base_dir_seq=meta.dir_seq,ack_dir_seq=7,entities={}}
                for i,row in ipairs(f.directory.entities)do
                    facts.entities[i]={epoch=row.epoch,id=row.id,incarnation=row.incarnation,slot=row.slot,
                        kind=row.kind,controller=row.controller,team=f.teams[i]}
                end
                if f.on_facts then return f.on_facts(facts)end
                return true,facts
            end,
            capture=function(index,context)
                f.captured=f.captured+1
                T.check(context.dir_seq==7 and f.directory.seq==7 and f.directory.entities[1].team_known
                    and f.directory.entities[2].team_known,"both recipes capture only after the full exact team ACK")
                if f.on_capture then f.on_capture(index,context)end
                return {team=f.teams[index+1]},{pawn=f.pawns[index+1]}
            end,
            describe=function(meta)
                f.described=f.described+1
                T.check(meta.dir_seq==7 and f.directory.seq==7,"neutral registration retains the acknowledged generation")
                if f.on_describe then f.on_describe(meta)end
                return true
            end}
        f.lifecycle=SourceLifecycle.new(f.env)
        function f:ack()
            self.directory.seq=7
            for i,row in ipairs(self.directory.entities)do row.team_known=true;row.team=self.teams[i]end
        end
        function f:ensure()return self.lifecycle.ensure(self.directory,self.token,1)end
        return f
    end
    local f=fixture()
    local ok,reason=f:ensure()
    T.check(ok==nil and reason=="source roster ACK pending" and f.queued==1 and f.captured==0 and f.described==0,
        "unknown native teams wait without any partial capture or registration")
    T.check(f:ensure()==nil and f.queued==1,"pending native batch is not requeued each worker tick")
    f:ack()
    local accepted,ack=f:ensure()
    T.check(accepted==true and ack~=f.directory and ack.seq==7 and f.captured==2 and f.described==2,
        "one immutable exact ACK snapshot admits both original source recipes")
    T.check(f:ensure()==true and f.queued==1 and f.captured==2,"unchanged acknowledged roster and recipes remain cached")
    f.directory.entities[1].team=9
    T.check(ack.entities[1].team==1,"copied ACK facts cannot be mutated through the returned directory table")
    T.check(f:ensure()==nil and f.captured==2,"same-sequence changed team cannot reuse an accepted recipe")

    for _,change in ipairs({"sequence","incarnation","slot","controller","owner","pawn","team","partial"})do
        f=fixture();f:ensure();f:ack()
        if change=="sequence"then f.directory.seq=8
        elseif change=="incarnation"then f.directory.entities[2].incarnation=6
        elseif change=="slot"then f.directory.entities[2].slot=2
        elseif change=="controller"then f.directory.entities[2].controller=2
        elseif change=="owner"then f.directory.entities[2].owner_peer=13
        elseif change=="pawn"then f.pawns[2]=23
        elseif change=="team"then f.directory.entities[2].team=3
        else f.directory.entities[2].team_known=false;f.directory.entities[2].team=nil end
        -- An unrelated sequence/ref restart may invoke a fresh batch. Return a
        -- concrete refusal there so this check cannot accidentally manufacture an ACK.
        f.env.roster_facts=function()return nil,"changed original roster"end
        T.check(f:ensure()~=true and f.captured==0 and f.described==0,"changed "..change.." cannot adopt the pending ACK")
    end
    for _,change in ipairs({"epoch","base","ack","team","sparse","partial","row","failure","world","binding"})do
        f=fixture()
        f.on_facts=function(facts)
            if change=="epoch"then facts.epoch=44
            elseif change=="base"then facts.base_dir_seq=5
            elseif change=="ack"then facts.ack_dir_seq=8
            elseif change=="team"then facts.entities[2].team=1.5
            elseif change=="sparse"then facts.entities[3]=facts.entities[2];facts.entities[2]=nil
            elseif change=="partial"then facts.entities[2]=nil
            elseif change=="row"then facts.entities[2].controller=0
            elseif change=="failure"then return nil,"actual native property unavailable"
            elseif change=="world"then f.world=false
            else f.pawns[2]=23 end
            return true,facts
        end
        local result,why=f:ensure()
        T.check(result~=true and f.captured==0 and f.described==0,"malformed/refused "..change.." native facts never permit capture")
        if change=="failure"then T.check(result==false and why:find("actual native property unavailable",1,true),
            "native refusal stays a fault rather than an ACK wait")end
        if change=="world"then T.check(result==false and f.invalidated==1,"world loss during preflight invalidates without old object access")end
    end
    for _,where in ipairs({"capture","registration"})do
        f=fixture();f:ensure();f:ack()
        if where=="capture"then f.on_capture=function()f.directory.seq=8 end
        else f.on_describe=function()f.directory.seq=8 end end
        local result,why=f:ensure()
        T.check(result==nil and why:find("source roster changed",1,true) and f.captured==1
            and f.described==(where=="capture"and 0 or 1),"directory change during "..where.." stops before the second recipe without retagging")
    end
    f=fixture();f:ensure();f:ack();f.on_capture=function()f.teams[1]=3 end
    T.check(f:ensure()==nil and f.described==0,"fresh recipe team must equal the acknowledged native fact")
    f=fixture();f.teams={0,-3};f:ensure();f:ack()
    local zero,zero_ack=f:ensure()
    T.check(zero==true and zero_ack.entities[1].team==0 and zero_ack.entities[2].team==-3,
        "native zero and negative teams remain actual known facts without false/default coercion")
    f=fixture();f:ensure();f.pawns[2]=23
    T.check(f:ensure()==nil and f.queued==1 and f.captured==0,"possession change while waiting drops the pending batch before capture")
    f=fixture();f:ensure();f:ack();f.on_capture=function()f.pawns[2]=23 end
    T.check(f:ensure()==nil and f.described==0,"mutation of another original roster binding during capture blocks registration")
    for _,failure in ipairs({"phase","team","describe"})do
        f=fixture();f:ensure();f:ack()
        local capture=f.env.capture;local released=0
        f.env.capture=function(...)
            local recipe,bindings=capture(...);bindings.scope_id=73
            if failure=="team"then recipe.team=recipe.team+1 end
            return recipe,bindings
        end
        f.env.discard=function(bindings)
            T.check(bindings.scope_id==73,"refusal cleanup retains the original captured gameplay scope")
            released=released+1;bindings.scope_id=nil
        end
        if failure=="phase"then f.env.phase=function(_,stage,edge)
            if stage=="source_capture"and edge=="exit"then error("phase callback refused",0)end
        end
        elseif failure=="describe"then f.env.describe=function()error("native describe refused",0)end end
        local result,why=f:ensure()
        T.check(result~=true and released==1 and type(why)=="string",
            "original gameplay scope releases exactly once on "..failure.." refusal before cache admission")
    end
end

-- Exercise the actual worker and actual lifecycle, including the downstream
-- core/render/publish boundary. The acknowledged snapshot is not hidden by a
-- replacement lifecycle that returns the caller's stale directory.
do
    local callback,clock,queued,captures,describes,samples,renders,publishes= nil,1,0,0,0,0,0,0
    local events,logs,sample_args,directory_reads={},{},{},0
    local directory={epoch=44,seq=6,entities={
        {epoch=44,id=1,incarnation=5,slot=0,kind=0,controller=0,owner_peer=11,team_known=false},
        {epoch=44,id=2,incarnation=5,slot=1,kind=0,controller=1,owner_peer=12,team_known=false}}}
    local function object(address,name,class)
        return {IsValid=function()return true end,GetAddress=function()return address end,
            GetFName=function()return {ToString=function()return name end}end,
            GetClass=function()return {GetFName=function()return {ToString=function()return class end}end}end}
    end
    local world=object(100,"World","World")
    world.GetFullName=function()return "World /Game/Map_Arena_Yard"end
    local pcs,pawns={},{}
    for index=0,1 do
        local pc,pawn=object(11+index,"PC"..index,"PlayerController"),object(21+index,"Willie"..index,"Willie_BP_C")
        pc.Pawn=pawn;pawn.Controller=pc;pawn.Mesh=object(31+index,"Mesh"..index,"SkeletalMeshComponent")
        pawn.DisableInput=function()end;pc.ResetIgnoreMoveInput=function()end;pc.ResetIgnoreLookInput=function()end
        pawn.Mesh.GetOwner=function()return pawn end;pawn.Mesh.SetComponentTickEnabled=function()end
        pawn["Team Int"]=index==0 and 0 or -3;pawn.Health=100;pawn.DED=false
        pc.GetWorld=function()return world end;pawn.GetWorld=function()return world end
        pcs[index],pawns[index]=pc,pawn
    end
    local token={key="roster-worker#1",drops=0}
    local wg={key=token.key,drops=0,check=function()return true end,settled=function()return true end,
        token=function()return token end,same=function(t)return t==token end,world=function()return world end,on_drop=function()end}
    local N={worker_input=function()return true end,host_start=function()return true end,
        host_directory=function()directory_reads=directory_reads+1;return directory end,host_inputs=function()return {}end,sample_config=function()return true end,
        native_source_roster_facts=function(meta,bindings)
            queued=queued+1
            T.check(meta.dir_seq==6 and #bindings==2 and bindings[1].pawn==21 and bindings[2].pawn==22,
                "actual worker queues one complete original two-player native fact batch")
            return true,{epoch=44,base_dir_seq=6,ack_dir_seq=7,entities={
                {epoch=44,id=1,incarnation=5,slot=0,kind=0,controller=0,team=0},
                {epoch=44,id=2,incarnation=5,slot=1,kind=0,controller=1,team=-3}}}
        end,
        host_describe=function(meta,recipe)
            describes=describes+1
            T.check(meta.dir_seq==7 and recipe.team==directory.entities[meta.id].team,
                "actual worker describes only source recipes matching the exact ACK native teams")
            return true
        end,
        native_sample_world=function(args)
            samples=samples+1
            sample_args[#sample_args+1]=args
            T.check(args.dir_seq==7 and args.frame_seq==samples and #args.actors==2
                and args.actors[1].incarnation==5 and args.actors[2].controller==12,
                "actual canonical sampler receives the ACK directory rather than stale caller generation")
            T.check(args.dt_ms==0,"actual worker passes explicit unknown native physics step, never capture/publication elapsed time")
            if samples==3 then return nil,"intermittent native sample refusal"end
            return true
        end,
        native_capture_render=function()renders=renders+1;if renders==1 then clock=clock+5.344 end;return true end,
        native_commit_world=function()publishes=publishes+1;if publishes==1 then clock=clock+0.591 end;return true end}
    for _,name in ipairs({"native_source_scope_begin","native_source_scope_keep","native_source_scope_resolve",
        "native_source_scope_end","native_source_scope_spline_profile","native_source_scope_vertex_state"})do
        N[name]=function()error("fixture adapter never borrows a native component",0)end
    end
    local modules={hsmp_runtime_role={worker=function()return true end},UEHelpers={GetGameplayStatics=function()
        return {IsValid=function()return true end,GetPlayerController=function(_,w,index)assert(w==world);return pcs[index]end}
        end},hsmp_wg={new=function()return wg end},hsmp_ipc={N=N,init=function()end,frame=function()end,world_ready=function()end},
        hsmp_saveguard={install=function()end,set_active=function()end,tick=function()end},
        hsmp_log={init=function()end,event=function(name,fields)
            if name=="x_native_worker"and fields.state=="native_evidence"then events[#events+1]=fields end
        end},director={make_ue_env=function()return {apply_cvars=function()end}end,
            new_native_worker=function()return {state="native_ready",tick=function()return true end}end},
        headless_control={new=function()return {set_directory=function()return true end,tick=function()end,stop=function()return true end}end},
        headless_prepare=Prepare,hsmp_pose_config={},headless_sample_boundary=Boundary,headless_spawn_diagnostics=SpawnDiagnostics,
        native_source_adapter={new=function()return {capture=function(index,meta)
            captures=captures+1;T.check(meta.dir_seq==7,"actual lifecycle capture keeps the acknowledged immutable generation")
            return {team=directory.entities[index+1].team},{pawn=pawns[index]:GetAddress()}
        end}end},headless_source_lifecycle=SourceLifecycle}
    local fake=setmetatable({debug=debug,os={getenv=function()return nil end,clock=function()return clock end},print=function(s)logs[#logs+1]=s end,
        require=function(name)if modules[name]then return modules[name]end;error("optional module absent")end,
        dofile=function()error("optional module absent")end,FindAllOf=function()return {}end,
        LoopInGameThreadWithDelay=function(_,fn)callback=fn;return 85 end},{__index=_G})
    assert(loadfile("mods/HSMPMatch/Scripts/headless_worker.lua","t",fake))().start()
    callback();local pending_reads=directory_reads;clock=1.016;callback()
    T.check(queued==1 and captures==0 and directory_reads==pending_reads+1,
        "pending roster attempts retain the33ms schedule without another lifecycle ACK probe on the16ms loop")
    clock=2;callback()
    for _,line in ipairs(logs)do assert(not line:find("worker stopped:",1,true),line)end
    T.check(queued==1 and captures==0 and describes==0 and samples==0 and events[#events].refused==0
        and events[#events].sampled==0 and events[#events].reason=="",
        "actual worker waiting for ACK publishes nothing and does not record a capture fault")
    directory.seq=7
    directory.entities[1].team_known=true;directory.entities[1].team=0
    directory.entities[2].team_known=true;directory.entities[2].team=-3
    clock=3;callback()
    T.check(captures==2 and describes==2 and samples==1 and renders==1 and publishes==1
        and events[#events].sampled==1 and events[#events].refused==0,"actual worker publishes one coherent frame after both ACK-qualified recipes")
    T.check(sample_args[1].ts_ms==3000 and clock==8.935,
        "slow5.935s native rendering/publication leaves the original captured timestamp unchanged")
    callback()
    T.check(samples==2 and renders==2 and publishes==2 and sample_args[2].ts_ms>sample_args[1].ts_ms
        and sample_args[2].dt_ms==0,"the next original source sample survives slow publication with monotonic timestamp and unknown physics step")
    clock=clock+0.016;callback()
    T.check(samples==2,"success attempt scheduling remains33ms and is independent of native step/timestamps")
    clock=clock+0.018;callback()
    T.check(samples==3 and renders==2 and publishes==2,"intermittent native failure cannot publish a partial frame")
    clock=clock+0.016;callback()
    T.check(samples==3,"a failed sample advances the attempt clock instead of busy retrying")
    clock=clock+0.018;callback()
    T.check(samples==4 and renders==3 and publishes==3 and sample_args[4].ts_ms>sample_args[3].ts_ms
        and sample_args[4].dt_ms==0,"next scheduled sample after failure resumes without a timestep latch or invented clamp")
end

do
    local clock,world,pawn,captures,registrations,invalidations=0,true,10,0,{},0
    local token={}
    local source={epoch=44,seq=1,entities={{epoch=44,id=1,incarnation=1,slot=0,controller=0,kind=0,team_known=true,team=7}}}
    local phases={}
    local function binding(index)return {index=index,world_key="world1",pc_address=1,pc_name="PC0",pawn_address=pawn,pawn_name="Willie"..pawn}end
    local env={now_ms=function()return clock end,same=function(t)return world and t==token end,resolve=binding,
        phase=function(context,stage,edge,detail)phases[#phases+1]={context=context,stage=stage,edge=edge,detail=detail}end,
        index=function(row)return row.controller end,capture=function(index,context)
            captures=captures+1
            T.check(phases[#phases].stage=="source_capture" and phases[#phases].edge=="enter" and context.id==1
                and context.epoch==44 and context.incarnation==source.entities[1].incarnation and context.dir_seq==source.seq,
                "static capture receives its exact copied ref after entry has persisted")
            return {team=7},{pawn=pawn}
        end,
        describe=function(meta,recipe,addresses)
            T.check(phases[#phases].stage=="native_bind" and phases[#phases].edge=="enter" and phases[#phases].context.revision==meta.revision,
                "native source registration entry persists before the API call")
            registrations[#registrations+1]={meta=meta,recipe=recipe,addresses=addresses};return true
        end,
        invalidate=function()invalidations=invalidations+1 end}
    local lifecycle=SourceLifecycle.new(roster_fixture(env,source,100))
    T.check(lifecycle.ensure(source,token,1),"exact static source recipe is registered before its first source frame")
    T.check(lifecycle.ensure(source,token,2) and captures==1,"unchanged native recipe is not recaptured in the bone loop")
    T.check(registrations[1].meta.frame_seq==1 and registrations[1].meta.dir_seq==1 and registrations[1].meta.revision==1,
        "descriptor carries its exact ref/directory/source frame generation")
    T.check(#phases==6 and phases[6].stage=="native_bind" and phases[6].edge=="exit" and phases[6].detail.ok==true,
        "cached static recipes add no phase spam to the bone loop")
    source.seq=2
    T.check(lifecycle.ensure(source,token,3) and captures==2 and registrations[2].meta.revision==2,
        "directory rotation recaptures source with a monotonic recipe revision")
    lifecycle.refresh()
    T.check(not lifecycle.ensure(source,token,4) and captures==2,"render-detected recipe changes use a bounded rare recapture")
    clock=1000
    T.check(lifecycle.ensure(source,token,4) and registrations[3].meta.revision==3,"recipe refresh retains its reference and advances revision")
    pawn=11
    T.check(not lifecycle.ensure(source,token,5) and captures==3,"replacement pawn cannot inherit an earlier entity reference")
    lifecycle.drop();source.entities[1].incarnation=2
    T.check(lifecycle.ensure(source,token,6) and registrations[4].addresses.pawn==11,"authoritative generation reset permits a new source binding")
    lifecycle.drop();env.capture=function()world=false;return {team=7},{pawn=11}end
    T.check(not lifecycle.ensure(source,token,7) and #registrations==4 and invalidations==1,
        "world change during static capture never registers stale source bindings")
    world=true;lifecycle.drop();env.capture=function()pawn=12;return {team=7},{pawn=11}end
    T.check(not lifecycle.ensure(source,token,8) and #registrations==4,"same-world possession replacement during capture cannot publish a source recipe")
    lifecycle.drop();env.capture=function()return nil,"unsupported actual source groom"end
    T.check(not lifecycle.ensure(source,token,9),"unsupported source recipe refuses registration without a guessed default")
    env.describe=function()world=false;return true end
    clock=2000;env.capture=function()return {team=7},{pawn=pawn}end
    T.check(not lifecycle.ensure(source,token,10) and invalidations==2,"world loss inside native registration invalidates after the native call")
end

do
    local function fixture(change)
        local pawn, mesh, touched = 10, 20, {}
        local env = {}
        function env.resolve(index)
            return {index=index,world_key="native#1",pc_address=1,pc_name="PC0",pawn_address=pawn,pawn_name="Willie"..pawn}
        end
        function env.disable_input(binding)
            touched[#touched+1]="disable:"..binding.pawn_address
            if change=="pawn"then pawn=11 end
            return true
        end
        function env.reset_move(binding)touched[#touched+1]="move:"..binding.pawn_address;return true end
        function env.reset_look(binding)touched[#touched+1]="look:"..binding.pawn_address;return true end
        function env.mesh(binding,field)
            if field~="Mesh"then return nil end
            local result={address=mesh,name="Mesh"..mesh,owner=binding.pawn_address}
            if change=="mesh_getter"then change=nil;mesh=21 end
            if change=="owner_getter"then change=nil;pawn=11 end
            return result
        end
        for _, operation in ipairs({"always_tick","no_rate_skip","tick_enabled"})do
            env[operation]=function(_, value)touched[#touched+1]=operation..":"..value.address;return true end
        end
        return Prepare.new(env),touched
    end
    local prep,touches=fixture("pawn")
    T.check(not prep:prepare(0), "DisableInput possession reentry refuses later preparation")
    T.check(T.eq(#touches,1),"old pawn receives no later reset or mesh touch after possession replacement")
    prep,touches=fixture("mesh_getter")
    T.check(not prep:prepare(0), "mesh replacement inside getter refuses old mesh setters")
    T.check(T.eq(#touches,3),"replacement mesh is detected before the first physical setter")
    prep,touches=fixture("owner_getter")
    T.check(not prep:prepare(0), "owner getter possession reentry refuses old actor setters")
    T.check(T.eq(#touches,3),"no later old-actor access after owner getter changes possession")
    prep,touches=fixture(nil)
    T.check(prep:prepare(0), "exact current native player and mesh prepare")
    T.check(T.eq(#touches,6),"one native input isolation and animation setup per binding")
    T.check(prep:prepare(0), "same qualified binding reuses prepared scalar identity")
    T.check(T.eq(#touches,6),"prepared binding does not repeat physical setters")
end

-- A native bootstrap writes the real GI local-multiplayer profile before the
-- single Director-owned travel. It never calls the client placement/census path.
local clock, world, settings, forced, travel, statuses = 0, {ok=true,key="menu#1",short="Map_Menu_Alpha"}, {}, false, {}, {}
local players_ready, diagnostics = false, {}
local env = { now=function()return clock end, world=function()return world end,
    native_settled=function()return true end, sg_force=function(value)forced=value end, sg_active=function()return forced end,
    sg_seed=function()return true,"HSMP_native_test_GameProgress"end,
    gi_set=function(name,value)settings[name]=value;return true end, gi_get=function(name)return settings[name]end,
    open_level=function(arena)travel[#travel+1]=arena;return true end,
    native_players_ready=function()return players_ready,"PC1 missing"end,
    native_spawn_diagnostics=function(reason)diagnostics[#diagnostics+1]={at=clock,reason=reason}end,
    native_status=function(state)statuses[#statuses+1]=state end,
}
local boot = Director.new_native_worker(env, { mode = "diagnostic" })
T.check(not boot:tick(), "travel has no premature readiness")
T.check(T.eq(settings["FreeMode Multiplayer"], true), "native local-player-two path enabled")
T.check(T.eq(settings["Free Mode Foes Amount"], 2), "native spawner supplies two humans plus one AI")
T.check(T.eq(settings["Free Mode Activated"], true), "free mode branch selected before travel")
T.check(T.eq(#travel, 1), "only one bootstrap travel issued")
T.check(T.eq(boot.state, "wait_world"), "await fresh native world")
world = {ok=true,key="arena#2",short="Map_Arena_Yard"}; clock=3; boot:tick()
T.check(T.eq(boot.state, "native_spawn"), "fresh correct arena enters native spawn")
boot:tick(); T.check(T.eq(boot.state, "native_spawn"), "missing second player cannot qualify")
T.check(T.eq(#diagnostics, 1), "missing native player records one actual fault census")
boot:tick(); T.check(T.eq(#diagnostics, 1), "unchanged fault census is bounded between polls")
clock=8;boot:tick();T.check(T.eq(#diagnostics, 2), "native fault census refreshes after five seconds without spawning")
players_ready=true; T.check(boot:tick(), "two current native players and AI qualify")
T.check(T.eq(#travel, 1), "readiness never spawns or reopens the native world")
world.key="arena#3"; T.check(not boot:tick(), "native world change invalidates authority")
T.check(T.eq(boot.state, "error"), "native world loss is explicit")
boot:stop(); before=#travel; boot:tick(); T.check(T.eq(#travel,before),"stopped worker never travels")

do
    local pvp = Director.native_worker_profile("pvp")
    local values = {}
    for _, row in ipairs(pvp) do values[row[1]] = row[2] end
    T.check(T.eq(values["Free Mode Foes Amount"], 1), "default PvP requests exactly two native fighters")
    -- Pinned CXXHeaderDump Enum_GameMode_enums.hpp: Arena(NewEnumerator1)=1;
    -- Enum_PlayMode_enums.hpp: Free Mode(NewEnumerator2)=1. Yard's cooked
    -- spawn_points support Arena on multiple native spawners, Tavern on C_0.
    T.check(T.eq(values["Current Game Mode Enum"],1),"native authority selects actual Arena mode rather than Tavern")
    T.check(T.eq(values["Current Play Mode"],1),"native authority selects actual Free Mode rather than Progression")
    T.check(T.eq(values["FreeMode Multiplayer"], true), "PvP retains the native second-player path")
    local unknown = Director.new_native_worker(env, { mode = "coop_abyss" })
    local travels = #travel
    T.check(unknown.state == "error" and not unknown:tick(), "unimplemented native Abyss mode refuses startup")
    T.check(T.eq(#travel, travels), "unsupported mode cannot alter the native world")
end

-- The actual source run verified Arena before travel, then BP_GameManager's
-- BeginPlay Load Game restored career Hell=5. Exercise the real save guard
-- with in-memory native save/load calls: the first arena load must read the
-- verified redirected seed, never the original career slot.
do
    local SG = dofile("mods/shared/hsmp_saveguard.lua")
    local hooks, slots, events, gi = {}, {GameProgress={ ["Current Game Mode Enum"]=5 }}, {}, {}
    local current_world={ok=true,key="native_menu#1",short="Map_Menu_Alpha"}
    local sequence, opens, loads = {}, 0, {}
    local function copy(values)
        local result={};for key,value in pairs(values)do result[key]=value end;return result
    end
    local function parameter(value)
        return {get=function()return value end,set=function(_,next_value)value=next_value end}
    end
    local function slot_call(name)
        local slot=parameter("GameProgress")
        if name=="SaveGameToSlot"then hooks[name](nil,parameter(gi),slot,parameter(0))
        else hooks[name](nil,slot,parameter(0))end
        return slot:get()
    end
    gi.IsValid=function()return true end
    gi["Save Game"]=function()
        sequence[#sequence+1]="native Save Game"
        slots[slot_call("SaveGameToSlot")]=copy(gi)
    end
    SG.install({inst="native_boot_fixture",save_dir="native-memory-only",fresh_per_session=false,reload_gi_on_exit=false,
        get_gi=function()return gi end,print=function()end,clock=function()return 0 end,
        register_hook=function(path,callback)hooks[path:match("([^:]+)$")]=callback end,
        log={event=function(name,fields)events[#events+1]={name=name,fields=fields}end}})
    local native_env={now=function()return 0 end,world=function()return current_world end,native_settled=function()return true end,
        sg_force=SG.set_active,sg_active=SG.is_active,sg_seed=SG.seed_session_slot,
        gi_set=function(key,value)gi[key]=value;return true end,gi_get=function(key)return gi[key]end,
        native_players_ready=function()return gi["Current Game Mode Enum"]==1 and gi["FreeMode Multiplayer"]==true end,
        open_level=function(arena)
            opens=opens+1;sequence[#sequence+1]="native OpenLevel"
            current_world={ok=true,key="native_arena#2",short=arena}
            -- Pinned BP_GameManager ExecuteUbergraph2211 -> GI Load Game2528.
            for _=1,2 do
                local slot=slot_call("LoadGameFromSlot");loads[#loads+1]=slot
                gi=copy(slots[slot])
            end
            return true
        end}
    local native_boot=Director.new_native_worker(native_env,{mode="pvp"})
    native_boot:tick();native_boot:tick()
    T.check(native_boot:tick(),"native BeginPlay reads the verified source profile and can reach native-ready")
    T.check(sequence[1]=="native Save Game" and sequence[2]=="native OpenLevel" and opens==1,
        "one redirected native seed is mandatory before one native travel")
    T.check(T.eq(loads[1],"HSMP_native_boot_fixture_GameProgress"),"first arena Load Game reads the native session seed")
    T.check(T.eq(loads[2],loads[1]),"repeated native arena loads retain the same source profile")
    T.check(gi["Current Game Mode Enum"]==1 and gi["FreeMode Multiplayer"]==true and gi["Free Mode Foes Amount"]==1,
        "actual saved native PvP fields survive arena initialization")
    T.check(T.eq(slots.GameProgress["Current Game Mode Enum"],5),"original career Hell profile is preserved")
    local redirected=false
    for _,event in ipairs(events)do if event.name=="save_redirected" and event.fields.op=="write" and event.fields.ok then redirected=true end end
    T.check(redirected,"seed uses the actual save guard diverted-write proof")
end

-- Native reflected calls may reenter startup/world callbacks. Refusal must
-- stop travel without repairing fields, reseeding, or touching an old world.
do
    local function fixture(failure)
        local current={ok=true,key="menu#1",short="Map_Menu_Alpha"}
        local settings,reads,writes,seeds,opens,active={},{},0,0,0,false
        local native_env={now=function()return 0 end,world=function()return current end,native_settled=function()return true end,
            sg_force=function(value)active=value end,sg_active=function()return active end,
            gi_set=function(key,value)
                settings[key]=value;writes=writes+1
                if failure=="setter_world"then current={ok=false}end
                return true
            end,
            gi_get=function(key)
                reads[#reads+1]=key
                local value=settings[key]
                if failure=="seed_getter_world" and seeds>0 then current={ok=false}end
                return value
            end,
            sg_seed=function()
                seeds=seeds+1
                if failure=="seed_world"then current={ok=false}
                elseif failure=="seed_profile"then settings["Current Game Mode Enum"]=5
                elseif failure=="seed_guard"then active=false
                elseif failure=="seed_refused"then return false,"no redirected write"
                elseif failure=="seed_nil"then return nil,"wrong thread"end
                return true
            end,
            open_level=function()opens=opens+1;return true end,
            travel_hold=function()
                if failure=="hold_wait_world"then return true end
                if failure=="hold_world"then current={ok=false}
                elseif failure=="hold_profile"then settings["Current Game Mode Enum"]=5 end
                return false
            end}
        if failure=="seed_missing"then native_env.sg_seed=nil end
        local native_boot=Director.new_native_worker(native_env)
        native_boot:tick()
        if failure=="hold_wait_world"then current={ok=true,key="different_menu#2",short="Map_Menu_Alpha"}end
        native_boot:tick()
        return native_boot,reads,writes,seeds,opens
    end
    for _,failure in ipairs({"seed_missing","seed_refused","seed_nil","seed_world","seed_profile","seed_guard","seed_getter_world","setter_world","hold_world","hold_profile","hold_wait_world"})do
        local native_boot,reads,writes,seeds,opens=fixture(failure)
        T.check(native_boot.state=="error" and opens==0,"native "..failure.." refuses all arena travel")
        T.check(seeds<=1 and writes<=#Director.NATIVE_WORKER_PROFILE,"native "..failure.." never retries or invents profile repairs")
        if failure=="setter_world"then T.check(T.eq(#reads,0),"setter world reentry prevents the next original-world read")end
        if failure=="seed_getter_world"then T.check(T.eq(#reads,#Director.NATIVE_WORKER_PROFILE+1),
            "post-seed getter world reentry stops every later profile read")end
    end
end

do
    local current,reads=true,0
    local function native_map(rows)
        return setmetatable({ForEach=function(_,callback)
            for _,row in ipairs(rows)do callback({get=function()return row[1]end},{get=function()reads=reads+1;return row[2]end})end
        end},{__len=function()return #rows end})
    end
    local flags=native_map({{0,false},{1,true}})
    T.check(SpawnDiagnostics.map_flag(flags,0,function()return current end)==false,
        "actual native spawner false compatibility remains false")
    T.check(SpawnDiagnostics.map_flag(flags,1,function()return current end)==true,
        "actual native enum key selects its typed compatibility value")
    T.check(SpawnDiagnostics.map_flag(flags,2,function()return current end)==false,
        "a missing native bool map key has the native Map_Find false out-value")
    T.check(SpawnDiagnostics.compatible(true,true,true,2,2)==true and SpawnDiagnostics.compatible(true,true,true,2,3)==false,
        "native compatibility uses Foes+1 >= RequiredCombatants exactly")
    T.check(SpawnDiagnostics.compatible(true,false,true,2,2)==false and SpawnDiagnostics.compatible(nil,true,true,2,2)==nil,
        "incompatible and unavailable source map values stay distinct")
    local stale=setmetatable({ForEach=function(_,callback)
        callback({get=function()current=false;return 1 end},{get=function()reads=reads+1;return true end})
    end},{__len=function()return 1 end})
    local before=reads
    local ok=pcall(SpawnDiagnostics.map_flag,stale,1,function()return current end)
    T.check(not ok and reads==before,"world loss while reading a native spawner key prevents the next old-map value read")
end

do
    local current, reads = true, {}
    local readers={same=function()return current end,world_key=function()return "native#2"end}
    for _, key in ipairs({"profile","controllers","mode","level","willies"}) do
        readers[key]=function()reads[#reads+1]=key;return {state="observed"}end
    end
    readers.level=function()reads[#reads+1]="level";return {p2=SpawnDiagnostics.property(false),player=SpawnDiagnostics.property(true),amount=SpawnDiagnostics.property(0),missing=SpawnDiagnostics.property(nil)}end
    local rows=SpawnDiagnostics.capture(readers,{},"PC1 missing")
    T.check(rows and #reads==5 and SpawnDiagnostics.format(rows):find("native[reason]=PC1 missing",1,true),
        "native spawn diagnostics carry scalar source observations and the actual refusal")
    T.check(rows.level.p2==false and rows.level.player==true and rows.level.amount==0 and rows.level.missing=="unavailable",
        "native diagnostic property conversion preserves actual false, true and zero separately from absence")
    T.check(SpawnDiagnostics.format(rows):find("native[level][p2]=false",1,true),"persisted fault census never turns actual false into unavailable")
    reads={};readers.profile=function()reads[#reads+1]="profile";current=false;return {}end
    T.check(not SpawnDiagnostics.capture(readers,{},"PC1 missing"),"world replacement inside profile refuses the census")
    T.check(T.eq(#reads,1),"no old controller, game mode or Willie reader runs after world replacement")
    current=true;readers.profile=function()error("property unavailable")end
    local ok, value, why=pcall(SpawnDiagnostics.capture,readers,{},"PC1 missing")
    T.check(ok and not value and why:find("property unavailable",1,true),"diagnostic getter refusal cannot replace the bootstrap fault")
end

-- Run the actual main chunks under the worker role with engine APIs forbidden:
-- each must return before its first UEHelpers import or hook registration.
local files = { "HSMPAvatars", "HSMPCombat", "HSMPHud", "HSMPInteract", "HSMPLoadout", "HSMPMenu", "HSMPModHost",
    "HSMPNoCutscene", "HSMPSync", "HSMPWorld", "dev/HSMPDiag", "dev/HSMPDump", "dev/HSMPParity" }
for _, name in ipairs(files) do
    local touched = false
    local fake = setmetatable({ os=os, debug=debug, pcall=pcall, type=type,
        require=function(module)
            if module=="hsmp_runtime_role" then return {client=function()return false end,worker=function()return true end} end
            touched=true;error("worker imported "..module)
        end,
    }, { __index=function(_,key)touched=true;error("worker touched "..key)end })
    local chunk=assert(loadfile("mods/"..name.."/Scripts/main.lua","t",fake))
    T.check(pcall(chunk) and not touched, "worker exits "..name.." before any client registration")
end
