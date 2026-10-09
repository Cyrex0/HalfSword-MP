-- Synthetic native lifecycle checks; this is not engine/runtime evidence.
local S=dofile("mods/HSMPMatch/Scripts/native_client_suppression.lua")
local I=dofile("mods/HSMPMatch/Scripts/native_client_isolation.lua")
local Arrays=dofile("mods/HSMPMatch/Scripts/native_client_array.lua")
local n=0;local function check(ok,why)n=n+1;T.check(ok,why);assert(ok,why)end
local function fixture()
    local f={valid=true,role=true,actors={},destroyed={},mutations=0,next=10}
    local function actor(name,types)
        f.next=f.next+1
        local a={name=name,address=f.next,valid=true,types=types or{},hidden=false,collision=true,tick=true,components={}}
        a.IsValid=function(self)return self.valid end;a.GetAddress=function(self)return self.address end
        a.GetFName=function(self)return{ToString=function()return self.name end}end
        a.GetClass=function(self)return{IsValid=function()return true end,GetFullName=function()return"Class /Test/"..self.name end}end
        a.GetWorld=function()return f.world end;a.GetOwner=function(self)return self.owner end;a.GetAttachParentActor=function(self)return self.parent end
        a.IsA=function(self,c)return self.types[c.path]==true end;a.ActorHasTag=function(self)return self.protected==true end
        a.SetActorHiddenInGame=function(self,v)f.mutations=f.mutations+1;self.hidden=v;self.bHidden=v end
        a.SetActorEnableCollision=function(self,v)self.collision=v end;a.SetActorTickEnabled=function(self,v)self.tick=v end
        a.GetActorEnableCollision=function(self)return self.collision end;a.IsActorTickEnabled=function(self)return self.tick end
        a.K2_GetComponentsByClass=function(self)return self.components end
        a.K2_DestroyActor=function(self)self.valid=false;f.destroyed[#f.destroyed+1]=self.name end
        a.IsActorBeingDestroyed=function(self)return not self.valid end
        a.HasAnyFlags=function(self,mask)return self.flags and(self.flags&mask)~=0 or false end
        return a
    end
    f.actor=actor
    local function mesh(owner)
        local c=actor(owner.name.."_mesh",{["/Script/Engine.PrimitiveComponent"]=true,["/Script/Engine.SkeletalMeshComponent"]=true,["/Script/Engine.SceneComponent"]=true})
        c.owner=owner;c.simulating=true;c.visible=true;c.collision_mode=3
        c.SetComponentTickEnabled=function(self,v)self.tick=v end;c.IsComponentTickEnabled=c.IsActorTickEnabled
        c.SetAllBodiesSimulatePhysics=function(self,v)self.simulating=v end;c.SetSimulatePhysics=c.SetAllBodiesSimulatePhysics
        c.SetCollisionEnabled=function(self,v)self.collision_mode=v end;c.GetCollisionEnabled=function(self)return self.collision_mode end
        c.SetAllPhysicsLinearVelocity=function()end;c.IsSimulatingPhysics=function(self)return self.simulating end
        c.SetVisibility=function(self,v)self.visible=v end;c.IsVisible=function(self)return self.visible end
        owner.components={c};owner.Mesh=c;owner.RootComponent=c;return c
    end
    f.mesh=mesh
    f.world=actor("world");f.world.GetFullName=function()return"World /Game/Maps/Arenas/Map_Arena_Yard.Map_Arena_Yard"end
    f.gm=actor("GameModeBase");f.gm.GetClass=function()return{IsValid=function()return true end,GetFullName=function()return"Class /Script/Engine.GameModeBase"end}end
    f.pawn=actor("Willie_BP_C_1");f.mesh(f.pawn)
    f.pc=actor("PlayerController",{["/Script/Engine.PlayerController"]=true});f.pc.Pawn=f.pawn;f.pawn.Controller=f.pc
    f.pc.K2_GetPawn=function(self)return self.Pawn end;f.pc.StopMovement=function()end
    f.pc.UnPossess=function(self)self.Pawn.Controller=nil;self.Pawn=nil end
    f.driver=actor("BP_LevelManager_C_1",{["/Game/Blueprints/Managers/BP_LevelManager.BP_LevelManager_C"]=true});f.mesh(f.driver)
    f.weapon=actor("ModularWeaponBP_C_1",{["/Game/Assets/Weapons/Blueprints/ModularWeaponBP.ModularWeaponBP_C"]=true});f.mesh(f.weapon);f.weapon["Parent Actor"]=f.pawn
    f.part=actor("module_1");f.mesh(f.part);f.part.owner=f.weapon
    f.actors.Willie_BP_C={f.pawn};f.actors.BP_LevelManager_C={f.driver}
    f.actors.ModularWeaponBP_C={f.weapon};f.actors.Modular_Weapon_Part_Master_C={f.part}
    f.env={role={presentation=function()return f.role end},WG={key="world1",token=function()return 1 end,same=function()return f.valid end,world=function()return f.world end},
        UEHelpers={GetGameplayStatics=function()return{IsValid=function()return true end,GetGameMode=function()return f.gm end}end},
        find=function(path)return{path=path,IsValid=function()return true end}end,
        find_all=function(class)local out={};for _,a in ipairs(f.actors[class]or{})do if a.valid then out[#out+1]=a end end;return #out>0 and out or nil end,
        FName=function(s)return s end,each=Arrays.each}
    f.env.retire_native=function(world,original,kind)
        assert(world==f.world.address and original==f.driver.address and kind==0)
        f.driver:K2_DestroyActor()
        return{ok=not f.driver.valid,qualified=true,dispatched=true,alive_after=f.driver.valid and 1 or 0,weak_present=f.driver.valid and 1 or 0,weak=1001,address=original,reason=f.driver.valid and"native retirement actor remains live"or"",
            before={world_listed={known=true,value=true}},after={world_listed={known=true,value=f.driver.valid},object_flags={known=f.driver.valid,value=0}}}
    end
    f.env.probe_native=function(_,weak,original)return{ok=true,qualified=true,dispatched=false,alive_after=0,weak_present=0,weak=weak,address=original,before={world_listed={known=true,value=true}},after={world_listed={known=true,value=false}}}end
    f.env.clear_native=function()f.cleared=(f.cleared or 0)+1 end
    return f,S.new(f.env)
end
do
    local f,s=fixture();local ok,why,counts=s:run()
    check(ok,why or"qualified client suppression succeeds")
    check(not f.driver.valid and counts.drivers==1,"native latent spawn owner is destroyed")
    check(#counts.driver_proofs==1 and counts.driver_proofs[1].name==f.driver.name and counts.driver_proofs[1].address==f.driver.address
        and counts.driver_proofs[1].native.before.world_listed.value==true and counts.driver_proofs[1].native.after.world_listed.value==false,
        "successful native retirement retains the exact original before/after proof")
    check(table.concat(f.destroyed,",")=="BP_LevelManager_C_1,module_1,ModularWeaponBP_C_1","owned gear children are destroyed before their parent")
    check(f.pc.valid and f.pc.Pawn==nil and f.pawn.Controller==nil,"normal player controller is preserved and unpossessed")
    check(f.pawn.valid and f.pawn.bHidden==true and f.pawn.collision==false and f.pawn.tick==false,"still-present Willie is retired without assuming actor destruction")
    check(f.pawn.Mesh.visible==false and f.pawn.Mesh.simulating==false and f.pawn.Mesh.collision_mode==0,"body component is visually and physically inert")
    check(s:retirement(f.pawn.address,f.pawn.name)and s:proof(f.pawn),"scalar retirement identity requires complete current readback")
    f.pawn.Mesh.simulating=true;check(not s:proof(f.pawn),"reactivated body cannot pass a cached retirement proof")
    f.pawn.Mesh.simulating=false;f.pawn.components[2]=f.mesh(f.actor("other"))
    check(not s:proof(f.pawn),"component census with a foreign owner refuses")
    s:drop();check(not s:proof(f.pawn),"world drop abandons all retirement identities")
end
do
    local f,s=fixture();f.role=false
    check(not s:run()and f.mutations==0 and f.pc.Pawn==f.pawn,"nonclient role cannot suppress native actors")
end
do
    local f,s=fixture()
    for _,a in ipairs({f.driver,f.pawn,f.weapon,f.part})do
        local rows=a.components
        a.K2_GetComponentsByClass=function()local copied={};for i,c in ipairs(rows)do copied[i]={type=function()return"RemoteUnrealParam"end,get=function()return c end}end;return copied end
    end
    check(s:run()and s:proof(f.pawn),"production iterator handles the real plain-table hard-object component return")
end
do
    local f,s=fixture();f.pawn.protected=true;f.actors.ModularWeaponBP_C={};f.actors.Modular_Weapon_Part_Master_C={}
    local ok=s:run();check(ok and f.pawn.valid and f.pawn.collision and f.pawn.Mesh.simulating and f.pc.Pawn==f.pawn,"protected Persistent pawn remains untouched")
end
do
    local f,s=fixture();f.driver.protected=true
    check(not s:run()and f.driver.valid and f.driver.collision,"protected spawn driver refuses destructive handling")
end
do
    local f,s=fixture();f.driver.K2_DestroyActor=function()end
    local ok,why,counts=s:run();local evidence=counts.refusal
    check(not ok and why=="suppression_native_retirement_refused"and counts.drivers==0,"native still-live driver continues to refuse scene readiness")
    check(evidence.kind=="driver"and evidence.phase=="after_native_retire"and evidence.native.dispatched==true,"refusal records native dispatch without claiming removal")
    check(evidence.name.value==f.driver.name and evidence.class.value=="Class /Test/"..f.driver.name,"refusal identifies the actual native actor and class")
    check(evidence.persistent.known and evidence.persistent.value==false and evidence.before.current_world.value==true,"known false Persistent and original-world proof survive the diagnostic")
    check(evidence.native.alive_after==1 and evidence.before.destroying.value==false,"native weak validity and actual actor destroy flag remain distinct")
    check(evidence.before.begin_destroyed.known and evidence.before.begin_destroyed.value==false and evidence.before.finish_destroyed.value==false,"native UObject teardown flags retain their distinct known false values")
    check(evidence.before.hidden.value==true and evidence.before.collision.value==false and evidence.before.tick.value==false,"inert actor flags do not waive failed native driver destruction")
    check(f.pc.Pawn==f.pawn and not s:retirement(f.pawn.address,f.pawn.name),"failed driver retirement cannot proceed to fighter retirement")
end
do
    local f,s=fixture();local reads=0;local native_flag=f.driver.IsActorBeingDestroyed
    f.driver.IsActorBeingDestroyed=function(self)if not self.valid then reads=reads+1 end;return native_flag(self)end
    local ok=s:run();check(ok and reads==0,"successful invalidation does not read an actor flag on the destroyed object")
end
do
    local f,s=fixture();local reads=0;local collision=f.driver.GetActorEnableCollision
    f.driver.K2_DestroyActor=function(self)self.pending=true end
    f.driver.IsActorBeingDestroyed=function(self)return self.pending==true end
    f.driver.GetActorEnableCollision=function(self)if self.pending then reads=reads+1 end;return collision(self)end
    local ok=s:run();check(not ok and reads==0,"an actor destroy flag cannot waive native still-live driver proof")
end
do
    local f,s=fixture();f.driver.K2_DestroyActor=function()end;f.driver.IsActorBeingDestroyed=function()return 0 end
    local ok,why,counts=s:run();check(not ok and why=="suppression_native_retirement_refused"and counts.refusal.before.destroying.known==false,"non-boolean actor flag stays unavailable while native live proof refuses")
end
do
    local f,s=fixture();f.env.retire_native=nil
    local ok,why=s:run();check(not ok and why=="suppression_native_retirement_unavailable"and f.driver.valid,"missing native retirement API refuses without Lua destroy fallback")
end
do
    local f,s=fixture();local native=f.env.retire_native
    f.env.retire_native=function(...)local proof=native(...);proof.address=proof.address+1;return proof end
    local ok,why=s:run();check(not ok and why=="suppression_native_retirement_refused","native retirement proof must refer to the original actor address")
end
do
    local f,s=fixture();local actor_reads=0
    f.env.retire_native=function(_,original)
        f.driver.native_gone=true
        return{ok=true,qualified=true,dispatched=true,alive_after=0,weak_present=1,weak=1001,address=original,
            before={world_listed={known=true,value=true}},after={world_listed={known=true,value=false},object_flags={known=true,value=0x40000000}}}
    end
    local old=f.driver.IsActorBeingDestroyed
    f.driver.IsActorBeingDestroyed=function(self)if self.native_gone then actor_reads=actor_reads+1 end;return old(self)end
    local ok,_,counts=s:run();check(ok and counts.drivers==1 and f.driver.valid and actor_reads==0,"native world absence plus mirrored garbage succeeds despite live weak/Lua qualifiers without actor events")
    local calls=0
    for _,method in ipairs({"IsValid","GetWorld","GetFName","GetClass","SetActorHiddenInGame","SetActorTickEnabled","SetActorEnableCollision","K2_DestroyActor"})do
        f.driver[method]=function()calls=calls+1;error("pending driver must not be touched")end
    end
    local again,why,counts=s:run();check(again and calls==0,why or"repeated census uses only original scalar native proof before pending driver methods")
    check(#counts.driver_proofs==1 and counts.driver_proofs[1].phase=="probe"and counts.driver_proofs[1].weak==1001,
        "periodic native probe evidence is retained without actor lookup")
    f.env.probe_native=function(_,weak,original)return{ok=false,qualified=true,dispatched=false,alive_after=1,weak=weak,address=original,reason="native identity became live"}end
    local live,refusal=s:run();check(not live and refusal=="suppression_native_retirement_probe_refused"and calls==0,"native-live/reused original proof fails before pending actor methods")
    s:drop();check(next(s.removed_drivers)==nil and f.cleared>=2,"world drop forgets Lua and provider scalar retirement records")
end
do
    local f,s=fixture();f.env.retire_native=function(_,original)return{ok=true,qualified=true,dispatched=true,alive_after=0,weak_present=1,weak=1001,address=original,
        before={world_listed={known=true,value=true}},after={world_listed={known=true,value=false},object_flags={known=true,value=0}}}end
    local ok,why=s:run();check(not ok and why=="suppression_native_retirement_refused","world absence and live weak without native garbage cannot be disguised as a successful native proof")
end
do
    local f,s=fixture();local generator=f.actor("BP_Generator_Weapons_Random_C_19",{
        ["/Game/Blueprints/Generators/BP_Generator_Weapons_Random.BP_Generator_Weapons_Random_C"]=true})
    local null={IsValid=function()return false end,GetAddress=function()return 0 end}
    generator.RootComponent=null;generator.DefaultSceneRoot=null;f.actors.BP_Generator_Weapons_Random_C={generator}
    local ok,why,counts=s:run();local evidence=counts.refusal
    check(not ok and why=="suppression_component_census_empty"and counts.drivers==1,"empty driver census remains refused after prior successful retirement")
    check(evidence.kind=="driver"and evidence.name.value==generator.name and evidence.class.value=="Class /Test/"..generator.name
        and evidence.address.value==generator.address,"empty component census identifies the exact actor rather than assuming fighter identity")
    check(evidence.current_world.value==true and evidence.expected_world.value==f.world.address and evidence.persistent.value==false
        and evidence.mirrored_garbage.value==false and evidence.world_listed.known==false,"native garbage and current world facts retain their actual scope without invented world-membership proof")
    check(evidence.root_component.value.null==true and evidence.root_component.value.address==0
        and evidence.default_scene_root.value.null==true,"actual null hard root properties use wrapper pointer copy without null-object methods")
    check(evidence.components.complete and evidence.components.count==0 and evidence.components.return_type=="table"
        and evidence.components.getter=="/Script/Engine.Actor:K2_GetComponentsByClass"and evidence.components.class=="/Script/Engine.ActorComponent",
        "empty reflected hard-object return records the exact getter, class, completeness and count")
    check(evidence.actor_hidden.value==true and evidence.actor_collision.value==false and evidence.actor_tick.value==false,
        "inert actor flags remain diagnostic and cannot waive an empty component census")
    check(#counts.driver_proofs==1 and counts.driver_proofs[1].native.dispatched==true and f.pc.Pawn==f.pawn,
        "later refusal preserves the prior successful native driver proof while preventing fighter cleanup")
end
do
    local f,s=fixture();f.pawn.components={}
    local ok,why,counts=s:run()
    check(not ok and why=="suppression_component_census_empty"and counts.refusal.kind=="fighter"
        and counts.refusal.name.value==f.pawn.name and not s:retirement(f.pawn.address,f.pawn.name),
        "component-free fighter remains refused with exact target identity")
end
do
    local f,s=fixture();f.driver.flags=0x40000000;local events=0;local root_reads=0
    for _,method in ipairs({"ActorHasTag","SetActorHiddenInGame","SetActorEnableCollision","SetActorTickEnabled","K2_GetComponentsByClass"})do
        f.driver[method]=function()events=events+1;error("garbage actor PE")end
    end
    f.driver.RootComponent=nil;setmetatable(f.driver,{__index=function(_,key)if key=="RootComponent"or key=="DefaultSceneRoot"then root_reads=root_reads+1;error("garbage root field")end end})
    local ok,why,counts=s:run()
    check(not ok and why=="suppression_actor_native_garbage"and counts.refusal.mirrored_garbage.value==true
        and events==0 and root_reads==0,"native garbage observed at inert entry refuses before actor PE or root fields")
    check(counts.refusal.name.value==f.driver.name and counts.refusal.lua_valid.value==true,
        "Lua wrapper validity does not label an engine-garbage actor alive")
end
do
    local f,s=fixture();f.driver.HasAnyFlags=function()return 0 end
    local ok,why,counts=s:run()
    check(not ok and why=="suppression_actor_garbage_unavailable"and counts.refusal.mirrored_garbage.known==false
        and f.mutations==0,"unknown native garbage value fails closed before inert mutation")
end
do
    local f,s=fixture();local touches=0
    f.driver.K2_GetComponentsByClass=function(self)self.flags=0x40000000;return{}end
    for _,method in ipairs({"GetActorEnableCollision","IsActorTickEnabled"})do
        f.driver[method]=function()touches=touches+1;error("post-getter garbage actor PE")end
    end
    local ok,why,counts=s:run()
    check(not ok and why=="suppression_actor_native_garbage"and counts.refusal.components.complete
        and counts.refusal.components.count==0 and touches==0,
        "native garbage raised during the getter retains pure census count and refuses further actor readback")
end
do
    local f,s=fixture();f.valid=false
    check(not s:run()and f.mutations==0,"world loss prevents all native mutations")
end
do
    local f,s=fixture();f.driver.GetWorld=function()return{IsValid=function()return true end,GetAddress=function()return-1 end}end
    local ok=s:run();check(ok and f.driver.valid and f.driver.tick,"foreign world spawn drivers are preserved")
end
do
    local f,s=fixture();f.pawn.Mesh.SetAllBodiesSimulatePhysics=function()end;f.pawn.Mesh.SetSimulatePhysics=function()end
    local ok,why=s:run();check(not ok and why=="suppression_primitive_readback"and not s:proof(f.pawn),"failed native physics shutdown cannot mark retirement complete")
end
do
    local f,s=fixture();local ai=f.actor("AIController",{["/Script/AIModule.AIController"]=true});f.mesh(ai)
    ai.Pawn=f.pawn;ai.K2_GetPawn=f.pc.K2_GetPawn;ai.StopMovement=f.pc.StopMovement;ai.UnPossess=f.pc.UnPossess;f.pawn.Controller=ai
    local ok,_,counts=s:run();check(ok and not ai.valid and counts.ai==1,"qualified owning AI controller is unpossessed and destroyed")
end
do
    local f,s=fixture();f.pc.UnPossess=function()end
    local ok,why=s:run();check(not ok and why=="suppression_unpossess_readback"and not s:proof(f.pawn),"possession teardown requires native readback")
end
do
    local fact=function(v)return{known=true,value=v}end
    local info={world_valid=fact(true),game_mode_class=fact("/Script/Engine.GameModeBase"),census_complete=true,controller_valid=fact(true),move_ignored=fact(true),look_ignored=fact(true),
        willies={{name=fact("retired"),persistent=fact(false),retired=fact(true),complete_inert=fact(false),mesh_visible=fact(false),actor_collision=fact(false),mesh_simulating=fact(false)}}}
    check(not I.assess(info),"retirement identity alone does not loosen isolation")
    info.willies[1].complete_inert=fact(true);check(I.assess(info),"complete verified retired body is excluded from active fighter census")
    info.willies[1].retired=fact(false);check(not I.assess(info),"arbitrary hidden nonpersistent body remains forbidden")
end
print(string.format("native_client_suppression: %d checks passed",n))
