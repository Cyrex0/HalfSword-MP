local Role = dofile("mods/shared/hsmp_runtime_role.lua")
local Control = dofile("mods/HSMPMatch/Scripts/headless_control.lua")
local Director = dofile("mods/HSMPMatch/Scripts/director.lua")
local Prepare = dofile("mods/HSMPMatch/Scripts/headless_prepare.lua")
local Boundary = dofile("mods/HSMPMatch/Scripts/headless_sample_boundary.lua")
local SpawnDiagnostics = dofile("mods/HSMPMatch/Scripts/headless_spawn_diagnostics.lua")
local SourceLifecycle = dofile("mods/HSMPMatch/Scripts/headless_source_lifecycle.lua")
T.eq(Role.parse(nil), "client", "existing launches stay clients")
T.eq(Role.parse("native_worker"), "native_worker", "explicit worker role")
T.eq(Role.parse("native-workr"), "invalid", "unknown role refuses execution")

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
    T.eq(#dispatched,0,"a subsequent possession change prevents all old-pawn input touches")
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
T.eq(#calls, before, "stopped runtime makes no native calls")
controls:drop(); running = true; controls:tick()
T.eq(#calls, before, "world drop forgets old objects without releasing through them")
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
    local token,current,commits,invalidations={},true,0,0
    local env={same=function(value)return current and value==token end,
        commit=function()commits=commits+1;return true end,
        invalidate=function()invalidations=invalidations+1 end}
    env.sample=function()current=false;return true end
    T.check(not Boundary.sample(env,token,{}),"world loss inside native sample refuses publication")
    T.eq(commits,0,"staged old-world DTO never commits after reflected world reentry")
    T.eq(invalidations,1,"world invalidation retries after the native call releases its lock")
    current=true;env.sample=function()return false,"Health unavailable"end
    T.check(not Boundary.sample(env,token,{}),"native sample refusal never commits")
    T.eq(commits,0,"failed sampling cannot publish previous staged data")
    env.sample=function()return true end
    T.check(not Boundary.sample(env,token,{}),"normal native publication requires the complete render stage")
    env.render=function()return nil,"source cloth unsupported"end
    T.check(not Boundary.sample(env,token,{}),"unsupported source rendering cannot publish a core-only frame")
    T.eq(commits,0,"render refusal cannot expose a partial source scene")
    env.render=function()current=false;return true end
    T.check(not Boundary.sample(env,token,{}),"world loss inside render capture refuses publication")
    T.eq(invalidations,2,"render reentry retries invalidation after its native lock releases")
    current=true;env.render=function()return true end
    T.check(Boundary.sample(env,token,{}),"unchanged qualified world commits its staged sample")
    T.eq(commits,1,"canonical publication occurs only after post-call world qualification")
    env.render=nil;env.allow_core_only=true
    T.check(Boundary.sample(env,token,{}),"explicit diagnostic mode can publish the bootstrap core without client readiness")
end

do
    local clock,world,pawn,captures,registrations,invalidations=0,true,10,0,{},0
    local token={}
    local source={epoch=44,seq=1,entities={{epoch=44,id=1,incarnation=1,slot=0,controller=0,kind=0}}}
    local function binding(index)return {index=index,world_key="world1",pc_address=1,pc_name="PC0",pawn_address=pawn,pawn_name="Willie"..pawn}end
    local env={now_ms=function()return clock end,same=function(t)return world and t==token end,resolve=binding,
        index=function(row)return row.controller end,capture=function()captures=captures+1;return {team=7},{pawn=pawn}end,
        describe=function(meta,recipe,addresses)registrations[#registrations+1]={meta=meta,recipe=recipe,addresses=addresses};return true end,
        invalidate=function()invalidations=invalidations+1 end}
    local lifecycle=SourceLifecycle.new(env)
    T.check(lifecycle.ensure(source,token,1),"exact static source recipe is registered before its first source frame")
    T.check(lifecycle.ensure(source,token,2) and captures==1,"unchanged native recipe is not recaptured in the bone loop")
    T.check(registrations[1].meta.frame_seq==1 and registrations[1].meta.dir_seq==1 and registrations[1].meta.revision==1,
        "descriptor carries its exact ref/directory/source frame generation")
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
    T.eq(#touches,1,"old pawn receives no later reset or mesh touch after possession replacement")
    prep,touches=fixture("mesh_getter")
    T.check(not prep:prepare(0), "mesh replacement inside getter refuses old mesh setters")
    T.eq(#touches,3,"replacement mesh is detected before the first physical setter")
    prep,touches=fixture("owner_getter")
    T.check(not prep:prepare(0), "owner getter possession reentry refuses old actor setters")
    T.eq(#touches,3,"no later old-actor access after owner getter changes possession")
    prep,touches=fixture(nil)
    T.check(prep:prepare(0), "exact current native player and mesh prepare")
    T.eq(#touches,6,"one native input isolation and animation setup per binding")
    T.check(prep:prepare(0), "same qualified binding reuses prepared scalar identity")
    T.eq(#touches,6,"prepared binding does not repeat physical setters")
end

-- A native bootstrap writes the real GI local-multiplayer profile before the
-- single Director-owned travel. It never calls the client placement/census path.
local clock, world, settings, forced, travel, statuses = 0, {ok=true,key="menu#1",short="Map_Menu_Alpha"}, {}, false, {}, {}
local players_ready, diagnostics = false, {}
local env = { now=function()return clock end, world=function()return world end,
    native_settled=function()return true end, sg_force=function(value)forced=value end, sg_active=function()return forced end,
    gi_set=function(name,value)settings[name]=value;return true end, gi_get=function(name)return settings[name]end,
    open_level=function(arena)travel[#travel+1]=arena;return true end,
    native_players_ready=function()return players_ready,"PC1 missing"end,
    native_spawn_diagnostics=function(reason)diagnostics[#diagnostics+1]={at=clock,reason=reason}end,
    native_status=function(state)statuses[#statuses+1]=state end,
}
local boot = Director.new_native_worker(env, { mode = "diagnostic" })
T.check(not boot:tick(), "travel has no premature readiness")
T.eq(settings["FreeMode Multiplayer"], true, "native local-player-two path enabled")
T.eq(settings["Free Mode Foes Amount"], 2, "native spawner supplies two humans plus one AI")
T.eq(settings["Free Mode Activated"], true, "free mode branch selected before travel")
T.eq(#travel, 1, "only one bootstrap travel issued")
T.eq(boot.state, "wait_world", "await fresh native world")
world = {ok=true,key="arena#2",short="Map_Arena_Yard"}; clock=3; boot:tick()
T.eq(boot.state, "native_spawn", "fresh correct arena enters native spawn")
boot:tick(); T.eq(boot.state, "native_spawn", "missing second player cannot qualify")
T.eq(#diagnostics, 1, "missing native player records one actual fault census")
boot:tick(); T.eq(#diagnostics, 1, "unchanged fault census is bounded between polls")
clock=8;boot:tick();T.eq(#diagnostics, 2, "native fault census refreshes after five seconds without spawning")
players_ready=true; T.check(boot:tick(), "two current native players and AI qualify")
T.eq(#travel, 1, "readiness never spawns or reopens the native world")
world.key="arena#3"; T.check(not boot:tick(), "native world change invalidates authority")
T.eq(boot.state, "error", "native world loss is explicit")
boot:stop(); before=#travel; boot:tick(); T.eq(#travel,before,"stopped worker never travels")

do
    local pvp = Director.native_worker_profile("pvp")
    local values = {}
    for _, row in ipairs(pvp) do values[row[1]] = row[2] end
    T.eq(values["Free Mode Foes Amount"], 1, "default PvP requests exactly two native fighters")
    -- Pinned CXXHeaderDump Enum_GameMode_enums.hpp: Arena(NewEnumerator1)=1;
    -- Enum_PlayMode_enums.hpp: Free Mode(NewEnumerator2)=1. Yard's cooked
    -- spawn_points support Arena on multiple native spawners, Tavern on C_0.
    T.eq(values["Current Game Mode Enum"],1,"native authority selects actual Arena mode rather than Tavern")
    T.eq(values["Current Play Mode"],1,"native authority selects actual Free Mode rather than Progression")
    T.eq(values["FreeMode Multiplayer"], true, "PvP retains the native second-player path")
    local unknown = Director.new_native_worker(env, { mode = "coop_abyss" })
    local travels = #travel
    T.check(unknown.state == "error" and not unknown:tick(), "unimplemented native Abyss mode refuses startup")
    T.eq(#travel, travels, "unsupported mode cannot alter the native world")
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
    T.eq(#reads,1,"no old controller, game mode or Willie reader runs after world replacement")
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
