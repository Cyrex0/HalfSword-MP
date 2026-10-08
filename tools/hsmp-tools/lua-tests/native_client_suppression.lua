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
        a.GetWorld=function()return f.world end;a.GetOwner=function(self)return self.owner end;a.GetAttachParentActor=function(self)return self.parent end
        a.IsA=function(self,c)return self.types[c.path]==true end;a.ActorHasTag=function(self)return self.protected==true end
        a.SetActorHiddenInGame=function(self,v)f.mutations=f.mutations+1;self.hidden=v;self.bHidden=v end
        a.SetActorEnableCollision=function(self,v)self.collision=v end;a.SetActorTickEnabled=function(self,v)self.tick=v end
        a.GetActorEnableCollision=function(self)return self.collision end;a.IsActorTickEnabled=function(self)return self.tick end
        a.K2_GetComponentsByClass=function(self)return self.components end
        a.K2_DestroyActor=function(self)self.valid=false;f.destroyed[#f.destroyed+1]=self.name end
        a.IsActorBeingDestroyed=function(self)return not self.valid end
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
        owner.components={c};owner.Mesh=c;return c
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
    return f,S.new(f.env)
end
do
    local f,s=fixture();local ok,why,counts=s:run()
    check(ok,why or"qualified client suppression succeeds")
    check(not f.driver.valid and counts.drivers==1,"native latent spawn owner is destroyed")
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
