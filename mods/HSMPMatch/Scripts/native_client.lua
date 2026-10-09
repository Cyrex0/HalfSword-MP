-- Normal rendered game client. The headless engine owns every combatant and impact.
local M={}
local started=false
local function module(name,path)
    local ok,v=pcall(require,name)
    if ok and type(v)=="table"then return v end
    local source=(debug.getinfo(1,"S").source or ""):gsub("^@","")
    local dir=source:match("^(.*)[/\\]") or "."
    ok,v=pcall(dofile,dir.."/"..(path or name..".lua"))
    if ok and type(v)=="table"then return v end
end
function M.start()
    if started then return false,"already started"end
    started=true
    local Role,HW,IPC,SG,D,Core,Input=module("hsmp_runtime_role"),module("hsmp_wg"),module("hsmp_ipc"),module("hsmp_saveguard"),module("director"),module("native_client_core"),module("native_client_input")
    local Isolation=module("native_client_isolation")
    local Suppression=module("native_client_suppression")
    local Arrays=module("native_client_array")
    local Presentation=module("native_presentation","../../HSMPAvatars/Scripts/native_presentation.lua")
    if not Role or not Role.presentation()or not HW or not IPC or not SG or not D or not Core or not Input or not Isolation or not Suppression or not Arrays or not Presentation or not LoopInGameThreadWithDelay then print("[HSMPNativeClient] startup refused: client dependencies unavailable\n");return end
    local UEH=require("UEHelpers")
    local state_dir=(os.getenv("HSMP_STATE_DIR")or"hsmp_state"):gsub("\\","/")
    local log=function(format,...)print(string.format("[HSMPNativeClient] "..format.."\n",...))end
    IPC.init({mod="HSMPMatch",state_dir=state_dir,log=log});IPC.frame()
    local N=IPC.N
    if not N or not N.client_start or not N.native_present then log("startup refused: native scene API unavailable");return end
    local HL=module("hsmp_log");if HL then HL.init({mod="HSMPMatch",state_dir=state_dir})end
    SG.install({mod="HSMPMatch",state_dir=state_dir,log=HL,fresh_per_session=false,reload_gi_on_exit=false});SG.set_active(true)
    local WG=HW.new({log=log,UEHelpers=UEH})
    local stop_file=os.getenv("HSMP_NATIVE_STOP_FILE")
    local identity=os.getenv("HSMP_NATIVE_IDENTITY_DIR")or state_dir
    local address=os.getenv("HSMP_NATIVE_SERVER")or"127.0.0.1:7777"
    local nick=os.getenv("HSMP_NATIVE_NICK")or"native-client"
    local brain=os.getenv("HSMP_NATIVE_CLIENT_AI")=="1"
    local function now()return N.now_us()/1000000 end
    local each=Arrays.each
    local mapping,last_key,loop
    local view=Presentation.new({native=N,world=function()if not WG.check()or not WG.settled()then return nil end;return WG.world(),WG.token()end,same=WG.same})
    local suppress=Suppression.new({role=Role,WG=WG,UEHelpers=UEH,find=StaticFindObject,find_all=FindAllOf,FName=FName,each=each,
        retire_native=N.native_retire_actor,probe_native=N.native_probe_retirement,clear_native=N.native_forget_retirements,scope_native=N.native_actor_scope})
    WG.on_drop(function()mapping=nil;last_key=nil;suppress:drop();view:drop();IPC.world_leaving()end,"native_client")
    local env=D.make_ue_env({WG=WG,UEHelpers=UEH,log=log,SG=SG,state_dir=state_dir})
    local controller,isolation_token,isolation_at,isolation_report
    local function isolated()
        local token=WG.token();local world=WG.world();if not world or not WG.same(token)then return false,"isolation_world_unavailable"end
        if isolation_token and WG.same(isolation_token)and now()<(isolation_at or 0)then return true end
        local stopped,stop_reason,summary=suppress:run()
        if not stopped and stop_reason and not stop_reason:match("^suppression_game_mode_mismatch:")then
            if HL then HL.event("x_native_client_suppression",{ok=false,reason=stop_reason,summary=summary or{}})end
            return false,stop_reason
        end
        if stopped and summary and(summary.drivers>0 or summary.gear>0 or summary.ai>0 or not isolation_token)then
            if HL then HL.event("x_native_client_suppression",{ok=true,reason="",summary=summary})end
        end
        local ok,reason,info=Isolation.inspect({WG=WG,UEHelpers=UEH,find_all=function(class)
            -- Pinned native FindAllOf nil is an empty native vector.
            local rows=FindAllOf(class);if rows==nil then return{}end;return rows
        end,FName=FName,suppression=suppress})
        local key=tostring(WG.key)..":"..tostring(reason or"isolated")
        if isolation_report~=key then
            isolation_report=key
            info.ok=ok;info.reason=reason or""
            log("isolation=%s reason=%s gm=%s fighters=%s",tostring(ok),tostring(reason or""),tostring(info.game_mode_class and info.game_mode_class.value or"unavailable"),tostring(info.willie_count or"unavailable"))
            if HL then HL.event("x_native_client_isolation",info)end
        end
        if not ok then return false,reason end
        isolation_token=token;isolation_at=now()+1
        return true
    end
    controller=Core.new({now=now,link=N.native_client_status,directory=N.host_directory,scene=N.native_scene,
        world=function()if not WG.check()then return nil end;return{ready=WG.settled(),arena=WG.short(),key=WG.key}end,
        travel=function(arena)
            for _,row in ipairs({{"Free Mode Activated",false},{"FreeMode Multiplayer",false},{"Progression Multiplayer",false},{"Free Mode Foes Amount",0}})do
                if not env.gi_set(row[1],row[2])or env.gi_get(row[1])~=row[2]then return false end
            end
            view:clear();return D.native_client_travel(env,arena)
        end,
        isolated=isolated,present=function()return view:apply()end,clear=function()view:clear()end,
        close=function()N.host_stop();env.quit_native_worker()end,send=N.native_input,
        input=function(scene,own)
            if brain then return Input.ai(scene,own,now())end
            if not mapping then
                local settings=StaticFindObject("/Script/Engine.Default__InputSettings")
                if not settings or not settings:IsValid()then return nil,"native input settings unavailable"end
                local why;mapping,why=Input.mapping(settings,each);if not mapping then return nil,why end
            end
            local pc=WG.pc();if not pc or not pc:IsValid()then return nil,"input controller unavailable"end
            local token=WG.token();local values,why=N.native_key_state(pc:GetAddress(),mapping.keys)
            if not values or not WG.same(token)then return nil,why or"world changed during engine input sample"end
            return Input.frame(mapping,values)
        end,
        report=function(state,reason,scene,own)
            log("state=%s reason=%s frame=%s",state,tostring(reason or""),tostring(scene and scene.frame_seq or 0))
            if HL then HL.event("x_native_client",{state=state,reason=reason or"",epoch=scene and scene.epoch or 0,dir_seq=scene and scene.dir_seq or 0,frame_seq=scene and scene.frame_seq or 0,own_entity=own and own.id or 0,own_incarnation=own and own.incarnation or 0})end
        end})
    local ok,why=N.client_start(address,identity,nick,os.getenv("HSMP_NATIVE_SERVER_KEY"))
    if ok~=true then log("startup refused: %s",tostring(why));return end
    loop=LoopInGameThreadWithDelay(16,function()
        local passed,reason=pcall(function()
            SG.tick();if stop_file and IPC.read(stop_file)then controller:stop("stop requested");env.quit_native_worker();return end
            if WG.check()then if WG.key~=last_key then last_key=WG.key;IPC.world_ready(WG.key)end;IPC.frame(WG.key)end
            controller:tick()
        end)
        if not passed then log("client stopped: %s",tostring(reason));controller:stop(tostring(reason))end
        return controller.stopped
    end)
    return true
end
return M
