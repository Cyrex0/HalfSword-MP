local M=dofile(T.path("mods/dev/HSMPParity/Scripts/collider_inventory.lua"))
local logs, actors, inspected, live, made = {}, {}, {}, 0, 0
local fail_inspect, fail_destroy, defer_destroy = false, false, false
local retain_gc,lookup_error,wrong_world=false,false,false
local clock=0
local world={IsValid=function()return true end}
function world:GetAddress()return 900 end
function world:GetFullName()return "World Test.PersistentWorld" end
local cdo={IsValid=function()return true end,GetFName=function()return {ToString=function()return "CDO" end} end}
local cls={IsValid=function()return true end,GetCDO=function()return cdo end}
function cls:GetFName()return {ToString=function()return "Weapon_Test_C" end}end
function cls:GetAddress()return 800 end
function cls:GetFullName()return "BlueprintGeneratedClass /Game/Assets/Weapons/Test.Weapon_Test_C" end
local unsafe_reads=0
setmetatable(cdo,{__index=function(_,key) unsafe_reads=unsafe_reads+1; error("unverified CDO field "..key) end})
local gs={IsValid=function()return true end}
function gs:BeginDeferredActorSpawnFromClass(w,c,t,collision,owner,scale)
    T.check(live==0,"previous temporary actor destroyed before next spawn")
    T.check(w==world and c==cls and collision==1 and owner==nil and scale==1,"verified deferred spawn signature")
    made=made+1; live=live+1
    local a={id=made,collision=true,hidden=false,dead=false}
    function a:IsValid()return not self.dead end
    function a:GetAddress()return self.id end
    function a:GetClass()return cls end
    function a:GetWorld()return world end
    function a:GetFName()return {ToString=function()return "temp"..self.id end} end
    function a:SetActorEnableCollision(v)self.collision=v end
    function a:GetActorEnableCollision()return self.collision end
    function a:SetActorHiddenInGame(v)self.hidden=v end
    function a:GetAttachedActors(out,reset,recursive)T.check(reset and recursive,"attached actor inventory is recursive") end
    function a:IsActorBeingDestroyed()return self.dead end
    function a:HasAnyFlags(mask)T.check(mask==0x40000000,"direct native garbage flag uses binary-proven mask");return self.retired==true end
    function a:K2_DestroyActor()self.requested=true; if fail_destroy or defer_destroy then return end; self.retired=true;self.dead=not retain_gc; live=live-1 end
    actors[#actors+1]=a
    return a
end
function gs:FinishSpawningActor(a,t,scale)
    T.check(not a.collision and a.hidden and scale==1,"collision disabled and hidden before native construction")
    T.check(a["Simulates Physics"]==nil,"class default physics initialization preserved")
    a.collision=true -- a construction script may reset this
end
local inv=M.new({
    now=function()return clock end,
    log=function(...)logs[#logs+1]=string.format(...)end,
    resolve=function(path)T.check(path:find("/Game/Assets/Weapons/",1,true)==1 and path:sub(-2)=="_C","canonical catalogue path expanded"); return cls end,
    lookup=function(class)
        T.check(class=="Weapon_Test_C","cleanup freshly queries the exact native class")
        return actors
    end,
    lookup_world=function(ref)
        if lookup_error then error("native enumeration failed") end
        T.check(ref.class_path=="/Game/Assets/Weapons/Test.Weapon_Test_C","live enumeration uses captured exact native class path")
        local out={};for _,a in ipairs(actors)do if not a.dead and not a.retired then out[#out+1]=a end end
        return {actors=out,world_address=wrong_world and 901 or 900,world_name=world:GetFullName(),class_address=800}
    end,
    context=function()return world,gs,{}end,
    inspect=function(a,id)
        T.check(not a.collision and a.hidden,"post-construction collision re-disabled")
        inspected[#inspected+1]=id
        if fail_inspect then error("probe failure") end
    end,
})
local cat={items={b={id="b",kind="weapon",path="@Weapons/B"},a={id="a",kind="weapon",path="@Weapons/A"},armor={kind="armor"}}}
T.check(inv.start(cat,"w"),"catalogue starts")
T.check(not inv.start(cat,"w"),"cannot overlap inventories")
inv.tick("w",true)
T.check(made==1 and live==0 and inspected[1]=="a","exactly one sorted class per callback, cleaned before returning")
inv.tick("w",true)
T.check(made==1,"next callback verifies cleanup before another class can spawn")
inv.tick("w",true); inv.tick("w",true)
T.check(made==2 and live==0 and inspected[2]=="b" and T.contains(logs[#logs],"classes=2 inspected=2 failed=0"),"all catalogue weapon classes inventoried")
T.check(unsafe_reads==0,"class defaults are never speculatively dereferenced in the live diagnostic")
local before=made
T.check(not inv.start(cat,"w","missing"),"unknown class filter refuses before any spawn")
inv.tick("w",true)
T.check(made==before,"unknown filter does not fall back to full inventory")
T.check(inv.start(cat,"w","b"),"one exact class may be selected")
inv.tick("w",true)
inv.tick("w",true)
T.check(made==before+1 and inspected[#inspected]=="b" and T.contains(logs[#logs],"classes=1 inspected=1 failed=0"),"filtered inventory spawns and cleans only requested class")
local null_dereferences=0
local saved=cdo
cdo=setmetatable({IsValid=function()return false end},{__index=function()null_dereferences=null_dereferences+1;error("native null wrapper dereference")end})
before=made
inv.start(cat,"w","a"); inv.tick("w",true)
T.check(made==before and null_dereferences==0,"invalid CDO wrapper is checked before name/property access or spawning")
T.check(T.contains(logs[#logs-1],"cdo_unavailable"),"invalid CDO is an explicit per-class failure")
cdo=saved
local stage_order={"resolve","cdo","context","begin_deferred","disable_before_finish","finish_spawn","disable_after_finish","attached_children","spawned_grip","inspect","cleanup"}
local last=0
for _,boundary in ipairs(stage_order)do
    local found
    for i=last+1,#logs do if T.contains(logs[i],"boundary="..boundary)then found=i;break end end
    T.check(found~=nil,"native boundary logged before "..boundary)
    last=found or last
end
fail_inspect=true; inv.start(cat,"w"); inv.tick("w",true); inv.tick("w",true)
T.check(live==0 and actors[#actors].dead,"inspection failure still destroys exact temporary actor")
inv.stop(); fail_inspect=false
inv.start(cat,"w"); inv.tick("different",true)
T.check(made==before+1 and T.contains(logs[#logs],"world_or_session_changed"),"world transition stops before spawning")
before=made
defer_destroy=true; inv.start(cat,"w","a"); inv.tick("w",true)
local pending=actors[#actors]
T.check(live==1 and pending.requested and not pending.collision and pending.hidden,"deferred destruction remains disabled and hidden")
T.check(not inv.status("w"),"status freshly reports an exact actor still present")
inv.tick("w",true)
T.check(made==before+1 and live==1,"unconfirmed cleanup blocks further construction")
pending.dead=true;live=live-1;defer_destroy=false
clock=clock+29
inv.tick("w",true)
T.check(T.contains(logs[#logs],"classes=1 inspected=1 failed=0") and inv.status("w"),"fresh absent identity confirms deferred cleanup")
T.check(not inv.status("other"),"status cannot query a prior world's actor identity")
inv.start(cat,"w"); fail_destroy=true; inv.tick("w",true)
local count=made
inv.tick("w",true);inv.tick("w",true);inv.tick("w",true)
T.check(made==count and not T.contains(logs[#logs],"cleanup_failed"),"fast callbacks do not pretend native cleanup timed out")
clock=clock+46; inv.tick("w",true)
T.check(made==count and T.contains(logs[#logs],"cleanup_failed"),"failed destruction stops inventory before another spawn")
T.check(not inv.start(cat,"w","b") and made==count,"explicit restart cannot bypass unresolved actor cleanup")
-- A successful destroy can leave a valid global UObject until GC. Require
-- positive live membership BEFORE destruction, then absence from that world.
actors[#actors].dead=true;live=live-1;fail_destroy=false;retain_gc=true
inv.start(cat,"w","a");inv.tick("w",true)
local retired=actors[#actors]
T.check(retired:IsValid() and not retired:IsActorBeingDestroyed() and retired.retired,"native retired actor reproduces valid-wrapper/false-destroying symptom")
inv.tick("w",true)
T.check(inv.status("w") and T.contains(logs[#logs],"cleanup=true"),"world absence plus exact native garbage confirms retirement while global UObject remains")
T.check(table.concat(logs,"\n"):find("global_uobject=present pre_live=true",1,true)~=nil,"GC-retained global object is diagnostic, not false liveness")
local flags=retired.HasAnyFlags
retired.HasAnyFlags=function()return false end
T.check(not inv.status("w"),"inactive-level omission without garbage cannot prove retirement")
retired.HasAnyFlags=function()error("native flag API unavailable")end
T.check(not inv.status("w"),"unavailable direct flag API fails closed")
retired.HasAnyFlags=flags
local getworld=retired.GetWorld
retired.GetWorld=function()return {IsValid=function()return true end,GetAddress=function()return 901 end}end
T.check(inv.status("w"),"retired object's GetWorld is not required after captured pre-scope and exact current-world proof")
retired.GetWorld=getworld
T.check(inv.status("w"),"fresh original world and native garbage restore valid retirement proof")
retain_gc=false
inv.start(cat,"w","a");inv.tick("w",true);wrong_world=true;clock=clock+1
inv.tick("w",true)
T.check(not inv.status("w"),"different native world cannot prove old actor retirement")
wrong_world=false;lookup_error=true;clock=clock+1;inv.tick("w",true)
T.check(not inv.status("w"),"failed native world enumeration cannot prove absence")
lookup_error=false;clock=clock+1;inv.tick("w",true)
T.check(inv.status("w"),"successful exact world enumeration completes the retained proof")
lookup_error=true
inv.start(cat,"w","a");inv.tick("w",true);lookup_error=false;clock=clock+46;inv.tick("w",true)
T.check(T.contains(logs[#logs],"cleanup_failed") and not inv.status("w"),"missing pre-destroy live observation never becomes a cleanup success")
