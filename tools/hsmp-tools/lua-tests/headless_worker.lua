local Role = dofile("mods/shared/hsmp_runtime_role.lua")
local Control = dofile("mods/HSMPMatch/Scripts/headless_control.lua")
local Director = dofile("mods/HSMPMatch/Scripts/director.lua")
local Prepare = dofile("mods/HSMPMatch/Scripts/headless_prepare.lua")
local Boundary = dofile("mods/HSMPMatch/Scripts/headless_sample_boundary.lua")
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
        headless_control=Control,headless_prepare=Prepare,hsmp_pose_config={},headless_sample_boundary=Boundary}
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
    T.check(Boundary.sample(env,token,{}),"unchanged qualified world commits its staged sample")
    T.eq(commits,1,"canonical publication occurs only after post-call world qualification")
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
local players_ready = false
local env = { now=function()return clock end, world=function()return world end,
    native_settled=function()return true end, sg_force=function(value)forced=value end, sg_active=function()return forced end,
    gi_set=function(name,value)settings[name]=value;return true end, gi_get=function(name)return settings[name]end,
    open_level=function(arena)travel[#travel+1]=arena;return true end,
    native_players_ready=function()return players_ready,"PC1 missing"end,
    native_status=function(state)statuses[#statuses+1]=state end,
}
local boot = Director.new_native_worker(env)
T.check(not boot:tick(), "travel has no premature readiness")
T.eq(settings["FreeMode Multiplayer"], true, "native local-player-two path enabled")
T.eq(settings["Free Mode Foes Amount"], 2, "native spawner supplies two humans plus one AI")
T.eq(settings["Free Mode Activated"], true, "free mode branch selected before travel")
T.eq(#travel, 1, "only one bootstrap travel issued")
T.eq(boot.state, "wait_world", "await fresh native world")
world = {ok=true,key="arena#2",short="Map_Arena_Yard"}; clock=3; boot:tick()
T.eq(boot.state, "native_spawn", "fresh correct arena enters native spawn")
boot:tick(); T.eq(boot.state, "native_spawn", "missing second player cannot qualify")
players_ready=true; T.check(boot:tick(), "two current native players and AI qualify")
T.eq(#travel, 1, "readiness never spawns or reopens the native world")
world.key="arena#3"; T.check(not boot:tick(), "native world change invalidates authority")
T.eq(boot.state, "error", "native world loss is explicit")
boot:stop(); before=#travel; boot:tick(); T.eq(#travel,before,"stopped worker never travels")

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
