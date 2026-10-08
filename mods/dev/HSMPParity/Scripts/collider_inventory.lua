-- Dev-only, explicitly invoked catalogue inventory. Destruction is requested in
-- the spawn callback; fresh identity lookup confirms it before the next class.
local M = {}
function M.new(o)
    local state,last
    local now=o.now or os.time
    local function valid(x) local ok,v=pcall(function()return x and x:IsValid() end); return ok and v==true end
    local function read(f) local ok,v=pcall(f); if ok then return v end end
    local function name(x)
        if not valid(x) then return "<invalid>" end
        return read(function()return x:GetFName():ToString() end) or "?"
    end
    local function addr(x) return valid(x) and read(function()return x:GetAddress() end) end
    local function unwrap(x) return read(function()return x:get() end) or x end
    local function dead(x) return not valid(x) or read(function()return x:IsActorBeingDestroyed() end)==true end
    local function binding(x)
        if not valid(x) then return nil end
        local cls=read(function()return x:GetClass()end)
        local world=read(function()return x:GetWorld()end)
        return {name=name(x),class=valid(cls) and name(cls) or nil,address=addr(x),
            class_path=valid(cls) and read(function()return cls:GetFullName():match("^%S+ (.+)$")end),
            class_address=addr(cls),world_address=addr(world),
            world_name=valid(world) and read(function()return world:GetFullName()end)}
    end
    local function live_status(ref)
        if not ref.class_path or not ref.class_address or not ref.world_address or not ref.world_name then return "binding_error" end
        local ok,result=pcall(o.lookup_world,ref)
        if not ok then return "lookup_error",false,tostring(result) end
        if type(result)~="table" then return "lookup_error",false,"invalid_result" end
        if result.world_address~=ref.world_address or result.world_name~=ref.world_name then return "scope_mismatch",false,result.error end
        if result.error then return "lookup_error",true,result.error end
        if type(result.actors)~="table" then return "lookup_error",true,"actors_not_table" end
        if result.class_address~=ref.class_address then return "class_mismatch",true end
        for _,x in pairs(result.actors)do
            if not valid(x) then return "invalid_live_entry" end
            if addr(x)==ref.address and name(x)==ref.name then
                local cls=read(function()return x:GetClass()end)
                if addr(cls)~=ref.class_address then return "class_mismatch" end
                return "present",true
            end
        end
        return "world_absent",true
    end
    local function destroy(x,id)
        o.log("INVENTORY destroy class=%s actor=%s pre_valid=%s pre_destroying=%s",id,name(x),tostring(valid(x)),tostring(read(function()return valid(x) and x:IsActorBeingDestroyed()end)))
        if dead(x) then return end
        pcall(function() x:SetActorEnableCollision(false) end)
        -- Only exact temporary weapon actors and their construction children
        -- enter this function; never a pawn found by a global object search.
        local cls=read(function()return x:GetClass()end)
        if not valid(cls) or name(cls):find("Willie",1,true) then
            o.log("INVENTORY destroy class=%s guard_rejected=true native_class=%s",id,name(cls)); return
        end
        local ok,err=pcall(function() x:K2_DestroyActor() end) -- unsafe: ok exact temporary weapon/owned construction child; Willie class rejected above
        o.log("INVENTORY destroy class=%s call_ok=%s error=%s post_valid=%s post_destroying=%s",id,tostring(ok),ok and "none" or tostring(err),tostring(valid(x)),tostring(read(function()return valid(x) and x:IsActorBeingDestroyed()end)))
    end
    local function verify(p)
        local clean=true
        for _,ref in ipairs(p.refs)do
            local status,scope,detail="pre_observation_missing",false,nil
            if ref.live_observed then status,scope,detail=live_status(ref) end
            if status=="world_absent" then ref.world_absent_observed=true end
            if status=="present" then ref.world_absent_observed=false end
            local global,garbage="lookup_error",false
            if ref.class and ref.address and ref.name~="?" then
                local ok,all=pcall(o.lookup,ref.class)
                if ok and type(all)=="table" then
                    global="absent"
                    for _,x in pairs(all)do
                        if valid(x) and addr(x)==ref.address and name(x)==ref.name then
                            global="present"
                            -- Fresh UObject methods, not reflected ProcessEvent:
                            -- UE4SS LuaUObject.hpp exposes HasAnyFlags directly.
                            -- Require original class/world identity before reading
                            -- RF_MirroredGarbage (also used by native actor iterator).
                            local cls=read(function()return x:GetClass()end)
                            -- Retired AActor::GetWorld may return null. Its live
                            -- world binding was captured and positively observed
                            -- before destroy; current world scope is checked
                            -- independently, without invoking the retired actor.
                            if addr(cls)==ref.class_address then
                                garbage=read(function()return x:HasAnyFlags(0x40000000)end)==true
                            end
                            break
                        end
                    end
                end
            end
            o.log("INVENTORY cleanup_probe class=%s actor=%s native_class=%s address=%s status=%s global_uobject=%s pre_live=%s native_garbage=%s same_world=%s prior_world_absent=%s error=%s",p.id,ref.name,tostring(ref.class),tostring(ref.address),status,global,tostring(ref.live_observed==true),tostring(garbage),tostring(scope==true),tostring(ref.world_absent_observed==true),tostring(detail or "none"))
            -- DestroyActor removes the live world actor before deferred GC
            -- removes its global UObject. Global wrapper presence is not
            -- liveness. The world iterator also filters inactive levels, so
            -- absence additionally needs native garbage or global disappearance.
            -- Class UObject can be collected after the actor. Preserve the
            -- earlier successful world absence only when a fresh lookup still
            -- proves this is that same native world, and global actor is absent.
            local retired=scope and ref.live_observed and
                ((status=="world_absent" and garbage) or
                 (ref.world_absent_observed and global=="absent"))
            if not retired then clean=false end
        end
        if clean then p.cleaned=true end
        return clean
    end
    local function step(item)
        local actor, owned = nil, {}
        local ok,why=pcall(function()
            local path=item.path:gsub("^@","/Game/Assets/")
            if not path:find("%.") then path=path.."."..path:match("([^/]+)$").."_C" end
            if path:find("/Game/Assets/Weapons/",1,true)~=1 then error("not_a_weapon_path") end
            o.log("INVENTORY stage class=%s boundary=resolve",item.id)
            local cls=o.resolve(path)
            if not valid(cls) then error("class_unavailable "..path) end
            o.log("INVENTORY stage class=%s boundary=cdo",item.id)
            local cdo=cls:GetCDO()
            if not valid(cdo) then error("cdo_unavailable") end
            o.log("INVENTORY class=%s path=%s cdo=%s",item.id,path,name(cdo))
            -- Read class defaults offline from the PAK. A null ClassProperty
            -- wrapper is not protected from native access faults by pcall.
            o.log("INVENTORY stage class=%s boundary=context",item.id)
            local world,gs,t=o.context()
            if not valid(world) or not valid(gs) then error("world_unavailable") end
            o.log("INVENTORY stage class=%s boundary=begin_deferred",item.id)
            actor=gs:BeginDeferredActorSpawnFromClass(world,cls,t,1,nil,1)
            if not valid(actor) then error("deferred_spawn_failed") end
            o.log("INVENTORY stage class=%s boundary=disable_before_finish",item.id)
            actor:SetActorEnableCollision(false)
            actor:SetActorHiddenInGame(true)
            if actor:GetActorEnableCollision()~=false then error("collision_disable_failed_before_finish") end
            -- Preserve class-default physics initialization. No game tick occurs
            -- between Finish, inspection and destruction, including on failure.
            o.log("INVENTORY stage class=%s boundary=finish_spawn",item.id)
            gs:FinishSpawningActor(actor,t,1)
            if not valid(actor) then error("destroyed_during_construction") end
            o.log("INVENTORY stage class=%s boundary=disable_after_finish",item.id)
            actor:SetActorEnableCollision(false)
            actor:SetActorHiddenInGame(true)
            if actor:GetActorEnableCollision()~=false then error("collision_disable_failed_after_finish") end
            local attached={}
            o.log("INVENTORY stage class=%s boundary=attached_children",item.id)
            actor:GetAttachedActors(attached,true,true)
            for _,entry in pairs(attached) do
                local child=unwrap(entry)
                if valid(child) and addr(child)~=addr(actor) then owned[#owned+1]=child end
            end
            o.log("INVENTORY stage class=%s boundary=spawned_grip",item.id)
            local grip=read(function()return actor["Spawned Weapon Grip"] end)
            if valid(grip) and addr(grip)~=addr(actor) then
                local parent=read(function()return grip:GetOwner() end)
                if addr(parent)==addr(actor) then owned[#owned+1]=grip end
            end
            o.log("INVENTORY stage class=%s boundary=inspect",item.id)
            o.inspect(actor,item.id)
        end)
        local refs,seen={},{}
        o.log("INVENTORY stage class=%s boundary=cleanup children=%d actor=%s",item.id,#owned,name(actor))
        local function release(x)
            local ref=binding(x)
            if ref and not seen[ref.address or ref.name] then
                seen[ref.address or ref.name]=true; refs[#refs+1]=ref
                local before=live_status(ref)
                ref.live_observed=before=="present"
                o.log("INVENTORY retirement_before class=%s actor=%s status=%s world=%s",item.id,ref.name,before,tostring(ref.world_name))
                if not ref.live_observed then ok=false;why="native_live_actor_pre_observation_failed "..before end
                destroy(x,item.id)
            end
        end
        for _,child in ipairs(owned)do release(child)end
        if actor then release(actor)end
        return {id=item.id,refs=refs,ok=ok,why=ok and "ok" or tostring(why),started=now(),next_probe=0}
    end
    local function finish(s,p,cleaned)
        o.log("INVENTORY result class=%s inspected=%s cleanup=%s reason=%s",p.id,tostring(p.ok),tostring(cleaned),p.why)
        if p.ok then s.ok=s.ok+1 else s.failed=s.failed+1 end
        s.index=s.index+1; s.pending=nil
        if not cleaned then state=nil; o.log("INVENTORY stopped cleanup_failed class=%s",p.id); return end
        if s.index>#s.items then
            state=nil
            o.log("INVENTORY done classes=%d inspected=%d failed=%d animated_motion_verified=false",#s.items,s.ok,s.failed)
        end
    end
    return {
        start=function(catalog,key,filter)
            if state then o.log("INVENTORY already_running"); return false end
            if last and #last.refs>0 and not last.cleaned and not verify(last) then
                o.log("INVENTORY refused unresolved_owned_actor class=%s",last.id);return false
            end
            local items={}
            for _,it in pairs(catalog.items or {}) do
                if it.kind=="weapon" and (not filter or filter=="" or it.id==filter) then items[#items+1]={id=it.id,path=it.path} end
            end
            table.sort(items,function(a,b)return a.id<b.id end)
            if #items==0 then o.log("INVENTORY empty_catalogue"); return false end
            state={items=items,key=key,index=1,ok=0,failed=0}
            o.log("INVENTORY start classes=%d destroy_requested_same_callback=true cleanup_verified_next_callback=true animated_motion_verified=false",#items)
            return true
        end,
        tick=function(key,ready)
            local s=state
            if not s then return end
            if not ready or key~=s.key then state=nil; o.log("INVENTORY stopped world_or_session_changed"); return end
            if s.pending then
                local p=s.pending
                local t=now()
                if t<p.next_probe then return end
                p.next_probe=t+1
                local cleaned=verify(p)
                -- Native destruction/GC can take seconds while the game keeps
                -- ticking. Callback count is not evidence of cleanup failure.
                if cleaned or t-p.started>=45 then finish(s,p,cleaned) end
                return
            end
            local p=step(s.items[s.index]); p.key=key; last=p
            if #p.refs==0 then finish(s,p,true) else s.pending=p end
        end,
        stop=function() state=nil; o.log("INVENTORY stopped"); end,
        status=function(key)
            if not last or last.key~=key then o.log("INVENTORY status no_owned_actor_in_current_world"); return false end
            local clean=verify(last)
            o.log("INVENTORY status class=%s cleanup=%s",last.id,tostring(clean))
            return clean
        end,
    }
end
return M
