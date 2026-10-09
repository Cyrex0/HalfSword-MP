-- Client-only retirement of map-native combat. Never touches presentation mirrors.
-- Actor removal may be deferred. Retain only scalar retirement identities and
-- require a fresh complete inert census for every still-present native fighter.
local M={}
-- The cooked Willie has 384 component exports. Every owned component remains
-- checked; one actor can use the same bounded 1024 budget as the whole pass.
local COMPONENT_BUDGET=1024
local DRIVERS={"BP_LevelManager_C","BP_SpawnerPoint_Willies_C","BP_Generator_Weapons_Random_C"}
local DRIVER_PATHS={
    BP_LevelManager_C="/Game/Blueprints/Managers/BP_LevelManager.BP_LevelManager_C",
    BP_SpawnerPoint_Willies_C="/Game/Blueprints/Spawner/BP_SpawnerPoint_Willies.BP_SpawnerPoint_Willies_C",
    BP_Generator_Weapons_Random_C="/Game/Blueprints/Generators/BP_Generator_Weapons_Random.BP_Generator_Weapons_Random_C",
}
local GEAR={"ModularWeaponBP_C","Modular_Weapon_Part_Master_C","Modular_Weapon_Module_C"}
local function engine_retired(proof)
    local before=type(proof)=="table"and proof.before
    local after=type(proof)=="table"and proof.after
    return type(before)=="table"and type(after)=="table"
        and before.world_listed and before.world_listed.known==true and before.world_listed.value==true
        and after.world_listed and after.world_listed.known==true and after.world_listed.value==false
        and(proof.weak_present==0 or(after.object_flags and after.object_flags.known==true
            and type(after.object_flags.value)=="number"and(after.object_flags.value&0x40000000)~=0))
end
function M.new(env)
    local self={retired={},removed_drivers={},external_identities={},key=nil}
    function self:drop()
        self.retired={};self.removed_drivers={};self.external_identities={};self.key=nil
        if type(env.clear_native)=="function"then env.clear_native()end -- scalar-only; never touches old UObjects
    end
    function self:retirement(address,name)return self.retired[address]==name end
    function self:run()
        if not env.role.presentation()then return false,"suppression_native_client_role_required"end
        local WG=env.WG;local token=WG.token();local world=WG.world()
        if not world or not WG.same(token)then return false,"suppression_world_unavailable"end
        if self.key~=WG.key then self:drop();self.key=WG.key end
        local summary={drivers=0,retired=0,gear=0,ai=0,driver_proofs={},external_drivers={}}
        local result,reason=pcall(function()
            local function fresh()if not WG.same(token)then error("suppression_world_changed",0)end end
            local function live(o)
                fresh();if not o or o:IsValid()~=true then return false end;fresh();return true
            end
            local function call(o,name,...)
                if not live(o)then error("suppression_object_unavailable:"..name,0)end
                local value=o[name](o,...);fresh()
                if not live(o)then error("suppression_object_changed:"..name,0)end
                return value
            end
            local function fact(fn,expected)
                fresh();local ok,value=pcall(fn);fresh()
                if ok and(expected==nil or type(value)==expected)then return{known=true,value=value}end
                return{known=false,error=(ok and"unexpected native value type"or tostring(value)):sub(1,192)}
            end
            local world_address=call(world,"GetAddress")
            local function current(o)
                if not live(o)then return false end
                local own=call(o,"GetWorld");return live(own)and call(own,"GetAddress")==world_address
            end
            local function address(o)return call(o,"GetAddress")end
            local function name(o)return call(o,"GetFName"):ToString()end
            local function cls(path)
                fresh();local c=env.find(path);fresh()
                if not live(c)then error("suppression_class_unavailable:"..path,0)end
                return c
            end
            local gs=env.UEHelpers.GetGameplayStatics();fresh()
            local gm=call(gs,"GetGameMode",world)
            local gm_class=call(gm,"GetClass");local full=call(gm_class,"GetFullName")
            if full~="Class /Script/Engine.GameModeBase"then error("suppression_game_mode_mismatch:"..full,0)end
            local component_class=cls("/Script/Engine.ActorComponent")
            local primitive_class=cls("/Script/Engine.PrimitiveComponent")
            local skeletal_class=cls("/Script/Engine.SkeletalMeshComponent")
            local scene_class=cls("/Script/Engine.SceneComponent")
            local player_class=cls("/Script/Engine.PlayerController")
            local ai_class=cls("/Script/AIModule.AIController")
            local weapon_class=cls("/Game/Assets/Weapons/Blueprints/ModularWeaponBP.ModularWeaponBP_C")
            local components_total=0
            local function objects(class)
                fresh();local rows=env.find_all(class);fresh()
                -- Pinned LuaMod.cpp FindAllOf returns nil only for an empty
                -- native object vector; exceptions and other types still refuse.
                if rows==nil then return{}end
                if type(rows)~="table"then error("suppression_census_type:"..class,0)end
                local n=0;for _ in pairs(rows)do n=n+1 end
                if n>512 then error("suppression_actor_bound:"..class,0)end
                return rows
            end
            local function protected(o)return call(o,"ActorHasTag",env.FName("Persistent"))==true end
            local function inert(actor,kind)
                -- Lua IsValid is a wrapper/memory qualifier, not this engine's
                -- actor liveness rule. HasAnyFlags is pinned direct UObject
                -- metadata (LuaUObject.hpp679), not an actor ProcessEvent.
                local evidence={kind=kind,phase="before_inert",
                    address=fact(function()return address(actor)end,"number"),
                    name=fact(function()return name(actor):sub(1,256)end,"string"),
                    class=fact(function()return call(call(actor,"GetClass"),"GetFullName"):sub(1,512)end,"string"),
                    lua_valid=fact(function()return live(actor)end,"boolean"),
                    mirrored_garbage=fact(function()return call(actor,"HasAnyFlags",0x40000000)end,"boolean"),
                    expected_world={known=true,value=world_address},
                    current_world={known=false,error="native garbage qualification not completed"},
                    persistent={known=false,error="native garbage qualification not completed"},
                    root_component={known=false,error="native garbage qualification not completed"},
                    default_scene_root={known=false,error="native garbage qualification not completed"},
                    world_listed={known=false,error="no native world-class membership census in this diagnostic"},
                    components={getter="/Script/Engine.Actor:K2_GetComponentsByClass",class="/Script/Engine.ActorComponent",complete=false,count=0,
                        budget=COMPONENT_BUDGET,root_included={known=false},primitives=0}}
                summary.refusal=evidence
                -- Never read actor fields or dispatch actor functions after a
                -- garbage/unknown native flag. Existing cohort current() checks
                -- precede this entry; this gate does not claim to qualify those.
                if not evidence.mirrored_garbage.known then error("suppression_actor_garbage_unavailable",0)end
                if evidence.mirrored_garbage.value~=false then error("suppression_actor_native_garbage",0)end
                evidence.current_world=fact(function()return current(actor)end,"boolean")
                evidence.persistent=fact(function()return protected(actor)end,"boolean")
                if not evidence.current_world.known or evidence.current_world.value~=true
                    or not evidence.persistent.known or evidence.persistent.value~=false then error("suppression_protected_or_foreign_actor",0)end
                local function root_fact(field)
                    return fact(function()
                        local root=actor[field];fresh()
                        if root==nil then error("hard object property returned nil",0)end
                        -- Pinned GetAddress copies only the wrapper pointer; a
                        -- native nullptr is identifiable without IsValid/PE.
                        local a=root:GetAddress();fresh()
                        if type(a)~="number"then error("root native pointer unavailable",0)end
                        if a==0 then return{pointer_known=true,address=0,null=true}end
                        local garbage=call(root,"HasAnyFlags",0x40000000)
                        if type(garbage)~="boolean"then error("root native garbage flag unavailable",0)end
                        local value={pointer_known=true,address=a,null=false,mirrored_garbage=garbage}
                        if not garbage then value.name=name(root):sub(1,256);value.class=call(call(root,"GetClass"),"GetFullName"):sub(1,512)end
                        return value
                    end,"table")
                end
                -- RootComponent is a native hard SceneComponent property;
                -- DefaultSceneRoot is a distinct optional Blueprint member.
                evidence.root_component=root_fact("RootComponent")
                evidence.default_scene_root=root_fact("DefaultSceneRoot")
                call(actor,"SetActorHiddenInGame",true);call(actor,"SetActorEnableCollision",false);call(actor,"SetActorTickEnabled",false)
                local rows=call(actor,"K2_GetComponentsByClass",component_class);local count=0
                evidence.phase="component_census";evidence.components.return_type=type(rows)
                env.each(rows,function()
                    count=count+1;components_total=components_total+1
                    evidence.components.count=count
                    if count>COMPONENT_BUDGET or components_total>COMPONENT_BUDGET then error("suppression_component_bound",0)end
                end,COMPONENT_BUDGET)
                evidence.components.complete=true
                evidence.mirrored_garbage_after=fact(function()return call(actor,"HasAnyFlags",0x40000000)end,"boolean")
                if not evidence.mirrored_garbage_after.known then error("suppression_actor_garbage_unavailable",0)end
                if evidence.mirrored_garbage_after.value~=false then error("suppression_actor_native_garbage",0)end
                local seen={};local root=evidence.root_component.known and evidence.root_component.value
                env.each(rows,function(c)
                    if not live(c)then error("suppression_component_owner",0)end
                    if call(c,"HasAnyFlags",0x40000000)~=false then error("suppression_component_native_garbage",0)end
                    if address(call(c,"GetOwner"))~=address(actor)then error("suppression_component_owner",0)end
                    local a=address(c);if seen[a]then error("suppression_component_duplicate",0)end;seen[a]=true
                    call(c,"SetComponentTickEnabled",false)
                    if call(c,"IsA",primitive_class)==true then
                        evidence.components.primitives=evidence.components.primitives+1
                        if call(c,"IsA",skeletal_class)==true then call(c,"SetAllBodiesSimulatePhysics",false)end
                        call(c,"SetSimulatePhysics",false);call(c,"SetCollisionEnabled",0)
                        call(c,"SetAllPhysicsLinearVelocity",{X=0,Y=0,Z=0},false)
                        if call(c,"GetCollisionEnabled")~=0 or call(c,"IsSimulatingPhysics",env.FName("None"))~=false then error("suppression_primitive_readback",0)end
                    end
                    if call(c,"IsA",scene_class)==true then call(c,"SetVisibility",false,true)end
                    if call(c,"IsComponentTickEnabled")~=false then error("suppression_component_tick_readback",0)end
                end,COMPONENT_BUDGET)
                evidence.mirrored_garbage_after=fact(function()return call(actor,"HasAnyFlags",0x40000000)end,"boolean")
                if not evidence.mirrored_garbage_after.known then error("suppression_actor_garbage_unavailable",0)end
                if evidence.mirrored_garbage_after.value~=false then error("suppression_actor_native_garbage",0)end
                evidence.actor_hidden=fact(function()local value=actor.bHidden;fresh();return value end,"boolean")
                evidence.actor_collision=fact(function()return call(actor,"GetActorEnableCollision")end,"boolean")
                evidence.actor_tick=fact(function()return call(actor,"IsActorTickEnabled")end,"boolean")
                if count==0 then error("suppression_component_census_empty",0)end
                local latest_root=root_fact("RootComponent")
                if not root or not root.pointer_known or not latest_root.known or latest_root.value.address~=root.address
                    or(not latest_root.value.null and latest_root.value.mirrored_garbage~=false)
                    or(kind=="fighter"and(root.null or root.mirrored_garbage~=false))then error("suppression_component_root_unavailable",0)end
                evidence.components.root_included={known=true,value=root.null==true or seen[root.address]==true}
                if not evidence.components.root_included.value then error("suppression_component_root_missing",0)end
                if call(actor,"GetActorEnableCollision")~=false or call(actor,"IsActorTickEnabled")~=false or actor.bHidden~=true then error("suppression_actor_readback",0)end
                fresh();summary.refusal=nil
            end
            local function driver_scope(actor,original,path)
                    local evidence={kind="driver",phase="native_scope",address={known=true,value=original},
                        expected_class=path,name={known=false,error="native scope not qualified"},class={known=false,error="native scope not qualified"}}
                    summary.refusal=evidence
                    if type(env.scope_native)~="function"then error("suppression_native_actor_scope_unavailable",0)end
                    fresh();local scope,why=env.scope_native(world_address,original);fresh()
                    evidence.native=scope or{ok=false,reason=tostring(why):sub(1,192)}
                    local state=type(scope)=="table"and scope.state
                    if type(scope)~="table"or scope.ok~=true or scope.qualified~=true or scope.address~=original
                        or type(scope.weak)~="number"or scope.weak==0 or type(state)~="table"
                        or not state.object_flags or state.object_flags.known~=true or type(state.object_flags.value)~="number"
                        or not state.world_listed or state.world_listed.known~=true or type(state.world_listed.value)~="boolean"then
                        error("suppression_native_actor_scope_refused",0)
                    end
                    local expected=cls(path)
                    if type(state.class_address)~="number"or state.class_address~=call(expected,"GetAddress")
                        or type(state.class_weak)~="number"or state.class_weak==0 or type(state.name)~="number"then
                        error("suppression_driver_scope_class",0)
                    end
                    local old=self.external_identities[original]
                    if old and(old.world~=self.key or old.weak~=scope.weak or old.name~=state.name
                        or old.class_address~=state.class_address or old.class_weak~=state.class_weak)then
                        error("suppression_external_driver_identity_changed",0)
                    end
                    local garbage=(state.object_flags.value&0x40000000)~=0
                    if state.world_listed.value==false and garbage then
                        local count=0;for _ in pairs(self.external_identities)do count=count+1 end
                        if(not old and count>=512)or #summary.external_drivers>=512 then error("suppression_external_driver_bound",0)end
                        self.external_identities[original]={weak=scope.weak,name=state.name,class_address=state.class_address,class_weak=state.class_weak,world=self.key}
                        -- No positive pre-removal observation exists here. This
                        -- is a freshly qualified non-live wrapper, never our
                        -- retirement or native-probe proof. No actor lookup/PE.
                        summary.external_drivers[#summary.external_drivers+1]={address=original,weak=scope.weak,expected_class=path,native=scope}
                        summary.refusal=nil;return nil
                    end
                    if garbage then error("suppression_driver_scope_contradiction",0)end
                    if old then error("suppression_external_driver_became_live",0)end
                    -- World absence alone does not prove inactivity or removal.
                    -- Report it, and never call actor PE for that scope.
                    if state.world_listed.value~=true then error("suppression_driver_world_unlisted",0)end
                    evidence.name=fact(function()return name(actor):sub(1,256)end,"string")
                    evidence.class=fact(function()return call(call(actor,"GetClass"),"GetFullName"):sub(1,512)end,"string")
                    return scope
            end
            local function destroy(actor,kind,original_scope)
                local driver_scope=original_scope
                if not current(actor)or protected(actor)then error("suppression_destroy_qualification",0)end
                local function actor_state(valid)
                    local state={valid={known=true,value=valid}}
                    -- UObject IsValid does not prove actor destruction. Record
                    -- the native actor flag separately, and never touch a dead
                    -- object after K2_DestroyActor.
                    if valid then
                        state.destroying=fact(function()return call(actor,"IsActorBeingDestroyed")end,"boolean")
                        if state.destroying.known and state.destroying.value==true then return state end
                        -- Exact EObjectFlags from pinned UE4SS shared/Types.lua
                        -- 229-230. UObject teardown flags are distinct from the
                        -- Actor:IsActorBeingDestroyed result above.
                        state.begin_destroyed=fact(function()return call(actor,"HasAnyFlags",0x00008000)end,"boolean")
                        state.finish_destroyed=fact(function()return call(actor,"HasAnyFlags",0x00010000)end,"boolean")
                        state.authority=fact(function()return call(actor,"HasAuthority")end,"boolean")
                        state.local_role=fact(function()return call(actor,"GetLocalRole")end,"number")
                        state.remote_role=fact(function()return call(actor,"GetRemoteRole")end,"number")
                        state.hidden=fact(function()local value=actor.bHidden;fresh();return value end,"boolean")
                        state.collision=fact(function()return call(actor,"GetActorEnableCollision")end,"boolean")
                        state.tick=fact(function()return call(actor,"IsActorTickEnabled")end,"boolean")
                        state.current_world=fact(function()return current(actor)end,"boolean")
                    end
                    return state
                end
                local evidence={kind=kind,phase="before_destroy",
                    name=fact(function()return name(actor):sub(1,256)end,"string"),
                    class=fact(function()return call(call(actor,"GetClass"),"GetFullName"):sub(1,512)end,"string"),
                    persistent=fact(function()return protected(actor)end,"boolean"),before=actor_state(true)}
                if driver_scope then evidence.scope=driver_scope end
                summary.refusal=evidence
                if kind=="driver"then
                    if type(env.retire_native)~="function"then error("suppression_native_retirement_unavailable",0)end
                    local original=address(actor);fresh()
                    local proof,why=env.retire_native(world_address,original,0,driver_scope.weak);fresh()
                    evidence.phase="after_native_retire";evidence.native=proof or{ok=false,reason=tostring(why):sub(1,192)}
                    -- Native actor liveness differs from UE4SS's legacy weak
                    -- validity rule on this engine. The provider requires live
                    -- exact-world/class presence before dispatch, then world
                    -- absence AND fresh original mirrored garbage/global absence.
                    -- It makes no actor ProcessEvent call after dispatch.
                    if type(proof)~="table"or proof.ok~=true or proof.qualified~=true or proof.dispatched~=true
                        or proof.alive_after~=0 or not engine_retired(proof)or type(proof.weak)~="number"or proof.weak~=driver_scope.weak or proof.address~=original then
                        error("suppression_native_retirement_refused",0)
                    end
                    if not evidence.name.known or not evidence.class.known then error("suppression_native_retirement_identity_unavailable",0)end
                    local count=0;for _ in pairs(self.removed_drivers)do count=count+1 end
                    if count>=512 then error("suppression_native_retirement_bound",0)end
                    self.removed_drivers[original]={weak=proof.weak,address=original,name=evidence.name.value,class=evidence.class.value,world=self.key}
                    summary.driver_proofs[#summary.driver_proofs+1]={phase="retire",name=evidence.name.value,class=evidence.class.value,address=original,weak=proof.weak,native=proof,scope=driver_scope}
                    summary.refusal=nil;return
                end
                actor:K2_DestroyActor() -- unsafe: ok only qualified nonPersistent native spawn drivers, owned gear or AI; never a Willie/player controller
                fresh()
                evidence.phase="after_destroy";evidence.call_returned=true
                local valid=live(actor);evidence.after=actor_state(valid)
                if valid and(not evidence.after.destroying.known or evidence.after.destroying.value~=true)then
                    fresh();error("suppression_destroy_readback",0)
                end
                fresh()
                summary.refusal=nil
            end
            -- Cancel native map-owned latent spawn callbacks by destroying their
            -- qualified owners, rather than merely disabling ReceiveTick.
            local confirmed={}
            for a,record in pairs(self.removed_drivers)do
                if type(env.probe_native)~="function"or record.world~=self.key then error("suppression_native_retirement_probe_unavailable",0)end
                fresh();local proof,why=env.probe_native(world_address,record.weak,record.address);fresh()
                if type(proof)~="table"or proof.ok~=true or proof.qualified~=true or proof.dispatched~=false or proof.alive_after~=0
                    or not engine_retired(proof)or proof.weak~=record.weak or proof.address~=record.address then
                    summary.refusal={kind="driver",phase="native_probe",name={known=true,value=record.name},class={known=true,value=record.class},native=proof or{ok=false,reason=tostring(why):sub(1,192)}}
                    error("suppression_native_retirement_probe_refused",0)
                end
                confirmed[a]=true
                if #summary.driver_proofs>=512 then error("suppression_native_retirement_bound",0)end
                summary.driver_proofs[#summary.driver_proofs+1]={phase="probe",name=record.name,class=record.class,address=record.address,weak=record.weak,native=proof}
            end
            for _,class in ipairs(DRIVERS)do
                for _,driver in pairs(objects(class))do
                    -- Pinned LuaUObject.hpp172 GetAddress copies the wrapper's
                    -- pointer value; it does not dereference the expired actor.
                    fresh();local raw=driver:GetAddress();fresh()
                    if type(raw)~="number"then error("suppression_driver_address",0)end
                    if not confirmed[raw]then
                    local scope=driver_scope(driver,raw,DRIVER_PATHS[class])
                    if scope then
                    -- Complete native world removal replaces the generic inert
                    -- component prerequisite only for these exact spawn drivers.
                    -- Zero components never becomes a body/physics proof.
                    destroy(driver,"driver",scope);summary.drivers=summary.drivers+1
                end end end
            end
            local willies={};local targets={}
            for _,pawn in pairs(objects("Willie_BP_C"))do if current(pawn)and not protected(pawn)then
                if #willies>=64 then error("suppression_fighter_bound",0)end
                willies[#willies+1]=pawn;targets[address(pawn)]=0
            end end
            local gear={};local seen={}
            for _,class in ipairs(GEAR)do for _,item in pairs(objects(class))do if current(item)then
                local a=address(item);if not seen[a]then seen[a]=true;gear[#gear+1]=item end
            end end end
            local selected={}
            -- Preserve ownership before parent destruction; include native child
            -- modules only through their actual owner/attachment chain.
            for _=1,8 do local changed=false
                for _,item in ipairs(gear)do local a=address(item)
                    if not selected[a]then
                        local owner=call(item,"GetOwner");local parent=call(item,"GetAttachParentActor")
                        local source_parent
                        if call(item,"IsA",weapon_class)==true then source_parent=item["Parent Actor"];fresh()end
                        local depth=(live(owner)and targets[address(owner)])or(live(parent)and targets[address(parent)])or(live(source_parent)and targets[address(source_parent)])
                        if depth~=nil and depth~=false then
                            if protected(item)then error("suppression_protected_owned_gear",0)end
                            selected[a]=depth+1;targets[a]=depth+1;changed=true
                        end
                    end
                end
                if not changed then break end
            end
            local removal={};for _,item in ipairs(gear)do local a=address(item);if selected[a]then removal[#removal+1]={actor=item,depth=selected[a]}end end
            table.sort(removal,function(a,b)return a.depth>b.depth end)
            for _,item in ipairs(removal)do inert(item.actor,"gear");destroy(item.actor,"gear");summary.gear=summary.gear+1 end
            for _,pawn in ipairs(willies)do
                local controller=pawn.Controller;fresh()
                if live(controller)then
                    if not current(controller)or address(call(controller,"K2_GetPawn"))~=address(pawn)then error("suppression_controller_binding",0)end
                    local player=call(controller,"IsA",player_class)==true
                    if not player and call(controller,"IsA",ai_class)~=true then error("suppression_controller_class",0)end
                    call(controller,"StopMovement");call(controller,"UnPossess")
                    local after=call(controller,"K2_GetPawn");if live(after)then error("suppression_unpossess_readback",0)end
                    if not player then inert(controller,"ai");destroy(controller,"ai");summary.ai=summary.ai+1 end
                end
                inert(pawn,"fighter")
                local after=pawn.Controller;fresh();if live(after)then error("suppression_fighter_controller_readback",0)end
                local a=address(pawn)
                if not self.retired[a]then local count=0;for _ in pairs(self.retired)do count=count+1 end;if count>=64 then error("suppression_retirement_bound",0)end end
                self.retired[a]=name(pawn);summary.retired=summary.retired+1
            end
        end)
        if not result then return false,tostring(reason),summary end
        return true,nil,summary
    end
    function self:proof(pawn)
        local WG=env.WG;local token=WG.token()
        if not env.role.presentation()or self.key~=WG.key or not WG.same(token)then return false end
        local ok,inert=pcall(function()
            local function read(o,method,...)
                if not WG.same(token)or not o or o:IsValid()~=true then error("proof object changed",0)end
                local v=o[method](o,...)
                if not WG.same(token)or o:IsValid()~=true then error("proof world/object changed",0)end
                return v
            end
            local a=read(pawn,"GetAddress");local n=read(pawn,"GetFName"):ToString()
            if not self:retirement(a,n)or read(pawn,"HasAnyFlags",0x40000000)~=false or pawn.bHidden~=true or read(pawn,"GetActorEnableCollision")~=false or read(pawn,"IsActorTickEnabled")~=false then return false end
            local c=pawn.Controller;if c and c:IsValid()then return false end
            local component=env.find("/Script/Engine.ActorComponent");local primitive=env.find("/Script/Engine.PrimitiveComponent");local scene=env.find("/Script/Engine.SceneComponent")
            if not component or not primitive or not scene or not WG.same(token)then return false end
            local root=pawn.RootComponent;if not root or not WG.same(token)then return false end
            local root_address=read(root,"GetAddress");if root_address==0 or read(root,"HasAnyFlags",0x40000000)~=false then return false end
            local count=0;local seen={};local rows=read(pawn,"K2_GetComponentsByClass",component)
            env.each(rows,function(cmp)
                count=count+1;if count>COMPONENT_BUDGET then error("proof component bound",0)end
                local ca=read(cmp,"GetAddress");if seen[ca]then error("proof duplicate component",0)end;seen[ca]=true
                if read(cmp,"HasAnyFlags",0x40000000)~=false or read(read(cmp,"GetOwner"),"GetAddress")~=a or read(cmp,"IsComponentTickEnabled")~=false then error("proof component changed",0)end
                if read(cmp,"IsA",primitive)==true and(read(cmp,"GetCollisionEnabled")~=0 or read(cmp,"IsSimulatingPhysics",env.FName("None"))~=false)then error("proof primitive active",0)end
                if read(cmp,"IsA",scene)==true and read(cmp,"IsVisible")~=false then error("proof component visible",0)end
            end,COMPONENT_BUDGET)
            local latest=pawn.RootComponent
            return count>0 and seen[root_address]==true and WG.same(token)and latest and read(latest,"GetAddress")==root_address
                and read(latest,"HasAnyFlags",0x40000000)==false
        end)
        return ok and inert==true
    end
    return self
end
return M
