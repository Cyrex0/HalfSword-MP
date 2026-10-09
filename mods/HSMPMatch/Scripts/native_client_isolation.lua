-- Rare qualified census. Only copied scalar facts survive this call.
local M={}
local function unknown(why)return{known=false,error=tostring(why or"unavailable"):sub(1,160)}end
function M.assess(info)
    local function value(f)return type(f)=="table"and f.known==true and f.value end
    if value(info.world_valid)~=true then return false,"isolation_world_unavailable"end
    if not info.game_mode_class or info.game_mode_class.known~=true then return false,"isolation_game_mode_class_unavailable"end
    if info.game_mode_class.value~="/Script/Engine.GameModeBase"then return false,"isolation_game_mode_mismatch:"..tostring(info.game_mode_class.value)end
    if info.census_complete~=true then return false,"isolation_census_incomplete"end
    for _,p in ipairs(info.willies or{})do
        local name=value(p.name)or"unavailable"
        if not p.persistent or p.persistent.known~=true then return false,"isolation_persistent_tag_unavailable:"..name end
        if p.persistent.value~=true then
            if not p.retired or p.retired.known~=true or p.retired.value~=true then return false,"isolation_local_fighter:"..name end
            if not p.complete_inert or p.complete_inert.known~=true or p.complete_inert.value~=true then return false,"isolation_retired_fighter_readback:"..name end
        end
        for _,field in ipairs({"mesh_visible","actor_collision","mesh_simulating"})do
            if not p[field]or p[field].known~=true then return false,"isolation_"..field.."_unavailable:"..name end
            if p[field].value~=false then return false,"isolation_"..field..":"..name end
        end
    end
    if value(info.controller_valid)~=true then return false,"isolation_controller_unavailable"end
    for _,field in ipairs({"move_ignored","look_ignored"})do
        if not info[field]or info[field].known~=true then return false,"isolation_"..field.."_unavailable"end
        if info[field].value~=true then return false,"isolation_"..field.."_readback"end
    end
    return true
end
function M.inspect(env)
    local WG=env.WG
    local token=WG.token()
    local info={census_complete=false,willies={}}
    local function fresh()return WG.same(token)end
    local function object(fn)
        if not fresh()then return nil,"world changed"end
        local ok,obj=pcall(fn)
        if not ok then return nil,tostring(obj)end
        if not fresh()then return nil,"world changed"end
        if not obj then return nil,"native null"end
        local valid,yes=pcall(function()return obj:IsValid()end)
        if not valid or yes~=true or not fresh()then return nil,"object unavailable"end
        return obj
    end
    local function read(fn,kind,target)
        if not fresh()then return unknown("world changed")end
        if target and not object(function()return target end)then return unknown("object unavailable")end
        local ok,v=pcall(fn)
        if not fresh()then return unknown("world changed")end
        if target and not object(function()return target end)then return unknown("object unavailable")end
        if not ok then return unknown(v)end
        if type(v)~=kind then return unknown("expected native "..kind)end
        if kind=="string"and#v>512 then return unknown("native diagnostic string bound")end
        return{known=true,value=v}
    end
    local function name(obj)return read(function()return obj:GetFName():ToString()end,"string",obj)end
    local function class(obj)
        local cls,why=object(function()return obj:GetClass()end)
        if not cls then return unknown(why)end
        return read(function()local full=cls:GetFullName();return full:match("^%S+%s+(.+)$")end,"string",cls)
    end
    local world,why=object(WG.world)
    info.world_valid={known=true,value=world~=nil}
    if not world then info.world=unknown(why);return false,"isolation_world_unavailable",info end
    info.world=read(function()return world:GetFullName()end,"string",world)
    local gs=object(env.UEHelpers.GetGameplayStatics)
    local gm,gm_why=object(function()return gs and gs:GetGameMode(world)end)
    info.game_mode=gm and name(gm)or unknown(gm_why)
    info.game_mode_class=gm and class(gm)or unknown(gm_why)
    info.game_mode_options=gm and read(function()return gm.OptionsString end,"string",gm)or unknown(gm_why)
    local pc,pc_why=object(WG.pc)
    info.controller_valid={known=true,value=pc~=nil}
    info.controller=pc and name(pc)or unknown(pc_why)
    info.controller_class=pc and class(pc)or unknown(pc_why)
    if pc then
        local pawn,pawn_why=object(function()return pc:K2_GetPawn()end)
        info.controller_pawn=pawn and name(pawn)or unknown(pawn_why)
        info.controller_pawn_class=pawn and class(pawn)or unknown(pawn_why)
        local target,target_why=object(function()return pc:GetViewTarget()end)
        info.view_target=target and name(target)or unknown(target_why)
        info.view_target_class=target and class(target)or unknown(target_why)
        info.view_target_full=target and read(function()return target:GetFullName()end,"string",target)or unknown(target_why)
        info.view_target_hidden=target and read(function()return target.bHidden end,"boolean",target)or unknown(target_why)
    end
    local listed,pawns=pcall(env.find_all,"Willie_BP_C")
    if listed and type(pawns)=="table"and fresh()then
        local total=0
        for _,candidate in pairs(pawns)do
            total=total+1
            if total>64 then info.census_error="Willie census exceeds64";break end
            local pawn=object(function()return candidate end)
            if pawn then
                local own,own_why=object(function()return pawn:GetWorld()end)
                if not own then info.census_error=own_why;break end
                local owner_address=read(function()return own:GetAddress()end,"number",own)
                local world_address=read(function()return world:GetAddress()end,"number",world)
                if not owner_address.known or not world_address.known then info.census_error="world identity unavailable";break end
                if owner_address.value==world_address.value then
                    local row={name=name(pawn),class=class(pawn),persistent=read(function()return pawn:ActorHasTag(env.FName("Persistent"))end,"boolean",pawn),actor_hidden=read(function()return pawn.bHidden end,"boolean",pawn),actor_collision=read(function()return pawn:GetActorEnableCollision()end,"boolean",pawn)}
                    if env.suppression then
                        row.retired=read(function()return env.suppression:retirement(pawn:GetAddress(),pawn:GetFName():ToString())end,"boolean",pawn)
                        row.complete_inert=read(function()return env.suppression:proof(pawn)end,"boolean",pawn)
                    end
                    local mesh,mesh_why=object(function()return pawn.Mesh end)
                    row.mesh=mesh and name(mesh)or unknown(mesh_why)
                    row.mesh_visible=mesh and read(function()return mesh:IsVisible()end,"boolean",mesh)or unknown(mesh_why)
                    row.mesh_collision=mesh and read(function()return mesh:GetCollisionEnabled()end,"number",mesh)or unknown(mesh_why)
                    row.mesh_simulating=mesh and read(function()return mesh:IsSimulatingPhysics(env.FName("None"))end,"boolean",mesh)or unknown(mesh_why)
                    info.willies[#info.willies+1]=row
                end
            elseif not fresh()then info.census_error="world changed";break end
        end
        info.census_complete=not info.census_error and fresh()
    else info.census_error=listed and"Willie census unavailable"or tostring(pawns):sub(1,160)end
    info.willie_count=#info.willies
    -- Retain the original policy. Never suppress or hide an actual fighter here.
    if pc and info.game_mode_class.known and info.game_mode_class.value=="/Script/Engine.GameModeBase"then
        read(function()pc:SetIgnoreMoveInput(true);return true end,"boolean",pc)
        read(function()pc:SetIgnoreLookInput(true);return true end,"boolean",pc)
        info.move_ignored=read(function()return pc:IsMoveInputIgnored()end,"boolean",pc)
        info.look_ignored=read(function()return pc:IsLookInputIgnored()end,"boolean",pc)
    end
    if not fresh()then info.world_valid=unknown("world changed");info.census_complete=false end
    local ok,reason=M.assess(info)
    return ok,reason,info
end
return M
