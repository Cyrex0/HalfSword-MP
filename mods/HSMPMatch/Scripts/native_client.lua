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
function M.start(opts)
    if started then return false,"already started"end
    started=true
    local Role,HW,IPC,SG,D,Core,Input=module("hsmp_runtime_role"),module("hsmp_wg"),module("hsmp_ipc"),module("hsmp_saveguard"),module("director"),module("native_client_core"),module("native_client_input")
    local Isolation=module("native_client_isolation")
    local Suppression=module("native_client_suppression")
    local Arrays=module("native_client_array")
    local Loading=module("native_client_loading")
    local Presentation=module("native_presentation","../../HSMPAvatars/Scripts/native_presentation.lua")
    local gameplay_mode=os.getenv("HSMP_NATIVE_GAMEPLAY")=="1"
    local Gameplay=gameplay_mode and module("native_client_gameplay")
    local Passport=gameplay_mode and module("native_gameplay_passport")
    if not Role or not Role.presentation()or not HW or not IPC or not SG or not D or not Core or not Input or not Isolation or not Suppression or not Arrays or not Loading or(not gameplay_mode and not Presentation)or not LoopInGameThreadWithDelay then print("[HSMPNativeClient] startup refused: client dependencies unavailable\n");return end
    local UEH=require("UEHelpers")
    local state_dir=(os.getenv("HSMP_STATE_DIR")or"hsmp_state"):gsub("\\","/")
    local log=function(format,...)print(string.format("[HSMPNativeClient] "..format.."\n",...))end
    IPC.init({mod="HSMPMatch",state_dir=state_dir,log=log});IPC.frame()
    local N=IPC.N
    if not N or not N.client_start or (gameplay_mode and(not Gameplay or not Passport or not N.native_gameplay_apply)or not gameplay_mode and not N.native_present)then log("startup refused: native client API unavailable");return end
    local HL=module("hsmp_log");if HL then HL.init({mod="HSMPMatch",state_dir=state_dir})end
    SG.install({mod="HSMPMatch",state_dir=state_dir,log=HL,fresh_per_session=false,reload_gi_on_exit=false});SG.set_active(true)
    local WG=HW.new({log=log,UEHelpers=UEH})
    local gameplay
    local loading=Loading.new({WG=WG,find=StaticFindObject,construct=StaticConstructObject,
        name=function(name)return FName(name,FNAME_Add)end,text=FText,log=log,now=function()return N.now_us()/1000000 end,
        view_ready=gameplay_mode and function(scene)return gameplay and gameplay:view_ready(scene)==true end or opts and opts.view_ready})
    local stop_file=os.getenv("HSMP_NATIVE_STOP_FILE")
    local identity=os.getenv("HSMP_NATIVE_IDENTITY_DIR")or state_dir
    local address=os.getenv("HSMP_NATIVE_SERVER")or"127.0.0.1:7777"
    local nick=os.getenv("HSMP_NATIVE_NICK")or"native-client"
    local brain=os.getenv("HSMP_NATIVE_CLIENT_AI")=="1"
    local function now()return N.now_us()/1000000 end
    local each=Arrays.each
    local mapping,last_key,loop,metrics_logged
    local phase_context,phase_generation,phase_seen=nil,nil,{}
    local function phase(api,edge,context,ok,reason)
        if not context then return end
        if phase_generation~=context.generation then phase_generation=context.generation;phase_seen={}end
        local key=api..":"..edge;if phase_seen[key]then return end;phase_seen[key]=true
        if HL then HL.event("x_native_client_phase",{api=api,edge=edge,ok=ok,reason=reason or"",epoch=context.epoch,
            epoch_text=math.type(context.epoch)=="integer"and tostring(context.epoch)or"unknown",dir_seq=context.dir_seq,
            frame_seq=context.frame_seq,own_entity=context.own_entity,own_incarnation=context.own_incarnation})end
    end
    local function traced(api,fn,...)
        local context=phase_context;phase(api,"enter",context)
        local result=table.pack(fn(...))
        phase(api,"exit",context,result[1]~=nil and result[1]~=false,type(result[2])=="string"and result[2]:sub(1,256)or"")
        return table.unpack(result,1,result.n)
    end
    local native={native_clear_mirrors=N.native_clear_mirrors}
    for _,api in ipairs({"native_scene_assets","native_present"})do
        if N[api]then native[api]=function(...)return traced(api,N[api],...)end end
    end
    local view=not gameplay_mode and Presentation.new({native=native,world=function()if not WG.check()or not WG.settled()then return nil end;return WG.world(),WG.token()end,same=WG.same,
        progress=function(stage,done,total)loading:set(stage,done,total)end})or nil
    if gameplay_mode then
        local gameplay_native={}
        for _,api in ipairs({"native_gameplay_begin","native_gameplay_construct","native_gameplay_finish","native_gameplay_apply"})do
            gameplay_native[api]=function(...)return traced(api,N[api],...)end
        end
        gameplay_native.native_gameplay_current=N.native_gameplay_current;gameplay_native.native_gameplay_clear=N.native_gameplay_clear
        gameplay_native.native_gameplay_confirm=N.native_gameplay_confirm
        gameplay=Gameplay.new({native=gameplay_native,passport=Passport,same=WG.same,find=StaticFindObject,
            now_us=N.now_us,diagnostic=function(row)if HL then HL.event("x_native_gameplay_timing",row)end end,
            fname=function(value)return FName(value,FNAME_Add)end,
            world=function()if not WG.check()or not WG.settled()then return nil end;return WG.world(),WG.token(),WG.pc()end,
            world_address=function(token)if not WG.same(token)then error("native gameplay world changed",0)end;local world=WG.world();return world:GetAddress()end,
            progress=function(stage,done,total)loading:set(stage,done,total)end})
    end
    local suppress=Suppression.new({role=Role,WG=WG,UEHelpers=UEH,find=StaticFindObject,find_all=FindAllOf,FName=FName,each=each,
        retire_native=N.native_retire_actor,probe_native=N.native_probe_retirement,clear_native=N.native_forget_retirements,scope_native=N.native_actor_scope,
        owned=gameplay_mode and function(pawn)return gameplay:owned(pawn)end or nil})
    WG.on_drop(function()mapping=nil;last_key=nil;phase_context=nil;phase_generation=nil;phase_seen={};loading:drop();suppress:drop();if view then view:drop()end;if gameplay then gameplay:drop()end;IPC.world_leaving()end,"native_client")
    local env=D.make_ue_env({WG=WG,UEHelpers=UEH,log=log,SG=SG,state_dir=state_dir})
    local controller,isolation_token,isolation_at,isolation_report
    local function loading_tick()
        local ok,why=loading:tick()
        if ok==nil and type(why)=="string"and controller and not controller.stopped then controller:stop(why)end
        return ok
    end
    local function isolated()
        local token=WG.token();local world=WG.world();if not world or not WG.same(token)then return false,"isolation_world_unavailable"end
        if gameplay then gameplay:sync(N.native_gameplay_scene())end
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
        end,FName=FName,suppression=suppress,gameplay=gameplay_mode,owned=gameplay_mode and function(pawn)return gameplay:owned(pawn)end or nil})
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
    controller=Core.new({gameplay=gameplay_mode,now=now,link=N.native_client_status,directory=N.host_directory,scene=function()
        local scene
        if gameplay_mode then scene=N.native_gameplay_scene()else scene=N.native_scene()end
        phase_context=nil
        if scene then for _,own in ipairs(scene.entities or{})do if own.kind==0 and own.owner_peer==scene.peer_id then
            phase_context={generation=scene.generation,epoch=scene.epoch,dir_seq=scene.dir_seq,frame_seq=scene.frame_seq,own_entity=own.id,own_incarnation=own.incarnation};break
        end end end
        return scene
    end,
        world=function()if not WG.check()then return nil end;return{ready=WG.settled(),arena=WG.short(),key=WG.key}end,
        travel=function(arena)
            loading:set("travel");if loading_tick()~=true then return false end
            for _,row in ipairs({{"Free Mode Activated",false},{"FreeMode Multiplayer",false},{"Progression Multiplayer",false},{"Free Mode Foes Amount",0}})do
                if not env.gi_set(row[1],row[2])or env.gi_get(row[1])~=row[2]then return false end
            end
            if gameplay then gameplay:clear()else view:clear()end;return D.native_client_travel(env,arena)
        end,
        isolated=isolated,present=function(scene)if gameplay then return gameplay:apply(scene)end;return traced("presentation_apply",view.apply,view)end,
        clear=function()if gameplay then gameplay:clear()else view:clear()end end,
        close=function()N.host_stop()end,send=N.native_input,
        input=function(scene,own)
            if brain then if gameplay_mode then return Gameplay.intent(scene,own)end;return Input.ai(scene,own,now())end
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
            loading:status(state,scene,own,reason)
            log("state=%s reason=%s frame=%s",state,tostring(reason or""),tostring(scene and scene.frame_seq or 0))
            if HL then HL.event("x_native_client",{state=state,reason=reason or"",epoch=scene and scene.epoch or 0,dir_seq=scene and scene.dir_seq or 0,frame_seq=scene and scene.frame_seq or 0,own_entity=own and own.id or 0,own_incarnation=own and own.incarnation or 0,
                gameplay=gameplay_mode,receipt_age_ms=scene and scene.received_age_ms,native_pawn=scene and scene.gameplay_proof,
                owned_view=gameplay_mode and scene and scene.gameplay_proof or nil,request_seq=own and own.request_seq,delivery_seq=own and own.delivery_seq})end
        end})
    local ok,why
    if gameplay_mode then ok,why=N.client_start(address,identity,nick,os.getenv("HSMP_NATIVE_SERVER_KEY"),"gameplay")
    else ok,why=N.client_start(address,identity,nick,os.getenv("HSMP_NATIVE_SERVER_KEY"))end
    if ok~=true then log("startup refused: %s",tostring(why));controller:stop(why or"client startup refused")end
    local quit=false
    loop=LoopInGameThreadWithDelay(16,function()
        local passed,reason=pcall(function()
            SG.tick();if stop_file and IPC.read(stop_file)then
                controller:stop("stop requested")
                if not quit then quit=true;env.quit_native_worker()end
                return
            end
            if WG.check()then if WG.key~=last_key then last_key=WG.key;IPC.world_ready(WG.key)end;IPC.frame(WG.key)end
            if gameplay_mode and not metrics_logged and type(N.native_gameplay_metrics)=="function"then
                local metrics=N.native_gameplay_metrics()
                if type(metrics)=="table"and type(metrics.decode_samples)=="number"and metrics.decode_samples>=8 then
                    metrics_logged=true
                    log("gameplay compression samples=%d raw=%d wire=%d decode_us=%d",metrics.decode_samples,metrics.decode_raw_bytes,metrics.decode_wire_bytes,metrics.decode_us)
                end
            end
            local covered=loading_tick()
            if covered==true and not controller.stopped then controller:tick()end
            loading_tick()
        end)
        if not passed then log("client stopped: %s",tostring(reason));controller:stop(tostring(reason))end
        return quit
    end)
    return true
end
return M
