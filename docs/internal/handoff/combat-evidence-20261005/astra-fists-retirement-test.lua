local M=dofile('mods/HSMPCombat/Scripts/replay_fists.lua'); local n=0
local function ok(x)assert(x);n=n+1 end
FName=function(x)return x end
local function object(address,name)
 local a={address=address,name=name};function a:IsValid()return true end;function a:GetAddress()return self.address end
 function a:GetFName()return {ToString=function()return self.name end}end;function a:GetFullName()return self.name end;return a
end
local pawn=object(1,'pawn');pawn.Mesh={GetSocketTransform=function(_,bone)
 return {bone=bone,Scale3D={X=2,Y=2,Z=2}}
end};local held=object(888,'real held weapon');pawn['Weapon L']=held
local native_world=object(900,'world');function pawn:GetWorld()return native_world end
local ctx={match_id=3,round=4,life=5};local world='W';local actors={};local spawned,finished=0,0
local cls=object(5,'Weapon_Fists_C');local gs={};local fail_finish=false
function gs:BeginDeferredActorSpawnFromClass(p,c,t,collision,owner)
 spawned=spawned+1;local a=object(10+spawned,'fist'..spawned);a.owner=owner
 a.BaseMesh={SetSimulatePhysics=function()end};a.items={}
 for i=1,10 do
  local x=object(100+i,'component'..i);x.collision=2
  function x:SetCollisionEnabled(v)self.collision=v end;function x:GetCollisionEnabled()return self.collision end
  function x:GetClass()return {GetFName=function()return {ToString=function()return 'SphereComponent'end}end}end
  function x:GetOwner()return a end
  function x:GetSocketTransform()return {Scale3D={X=1,Y=1,Z=1}}end
  function x:GetSocketTransform()return {Scale3D={X=1,Y=1,Z=1}}end
  function x:GetUnscaledSphereRadius()return self.radius or 13 end
  function x:GetScaledSphereRadius()return self.radius or 13 end
  a.items[i]=x
 end
 a.Sphere=a.items[10];a['Collision Components Array']={ForEach=function(_,fn)
  for i,x in ipairs(a.items)do fn(i,{get=function()return x end})end
 end}
 function a:SetActorEnableCollision(v)self.collision=v end;function a:GetActorEnableCollision()return self.collision end
 function a:SetActorHiddenInGame(v)self.hidden=v end;function a:GetOwner()return self.owner end
 function a:GetWorld()return native_world end
 function a:GetClass()return {GetFName=function()return {ToString=function()return 'Weapon_Fists_C'end}end}end
 function a:K2_SetActorTransform(t)self.transform=t;return true end
 function a:K2_DestroyActor()self.destroyed=true end
 actors[#actors+1]=a;return a
end
function gs:FinishSpawningActor(a,t)
 ok(a.hidden and a.collision==false and a['Temp Disable Damage']==true)
 if fail_finish then error('injected native Finish failure')end
 ok(a['Parent Actor']==pawn and a['Last Parent']==pawn)
 ok(t.Scale3D.X==1 and t.Scale3D.Y==1 and t.Scale3D.Z==1);finished=finished+1
end
local e={context=function()return ctx end,world_key=function()return world end,
 find_class=function()return cls end,find_all=function(class)return class=='Willie_BP_C' and {pawn} or actors end,
 UEHelpers={GetGameplayStatics=function()return gs end},log=function()end}
local r=M.new(e);ok(r.component(pawn,true,10,ctx));ok(spawned==1 and finished==1)
ok(pawn['Weapon L']==held);ok(actors[1].hidden and actors[1].collision==false)
ok(actors[1].Sphere:GetCollisionEnabled()==0);ok(r.component(pawn,true,10,ctx));ok(spawned==1)
ok(not r.component(pawn,true,9,ctx));ok(not r.component(pawn,true,10,{match_id=3,round=4,life=6}))
ctx={match_id=3,round=4,life=6};ok(r.component(pawn,true,10,ctx));ok(spawned==2)
world=nil;ok(not r.component(pawn,true,10,ctx));r.clear('no valid world');world='W'
ok(r.component(pawn,true,10,ctx));ok(spawned==2)
world='new';ok(r.component(pawn,true,10,ctx));ok(spawned==2)
r.clear('session lost');ok(r.component(pawn,true,10,ctx));ok(spawned==2)
r.clear('travel');ok(r.component(pawn,true,10,ctx));ok(spawned==2)
native_world=object(901,'new world');ok(r.component(pawn,true,10,ctx));ok(spawned==3)
local bounded=M.new(e);local first=spawned
for life=1,64 do ctx={match_id=3,round=4,life=life};ok(bounded.component(pawn,false,10,ctx));bounded.clear('session lost')end
ctx={match_id=3,round=4,life=65};ok(not bounded.component(pawn,false,10,ctx));ok(spawned-first==64)
local Smoke=dofile('mods/dev/HSMPParity/Scripts/replay_fists_smoke.lua')
e.factory=r;ok(not Smoke.run(e,pawn,true,ctx));e.authorized=true
ok(Smoke.run(e,pawn,true,ctx));local c=r.component(pawn,true,10,ctx);c.radius=12
ok(not Smoke.run(e,pawn,true,ctx));c.radius=13
local failed=M.new(e);fail_finish=true;local before=spawned
ok(not failed.component(pawn,true,10,ctx));ok(spawned==before+1)
local partial=actors[#actors];ok(partial.collision==false and partial.hidden and partial['Temp Disable Damage']==true)
fail_finish=false;ok(not failed.component(pawn,true,10,ctx));ok(spawned==before+1)
local pooled=M.new(e);local real_find=e.find_all;local remove_retired=false;local unavailable=false
e.find_all=function(class)
 if unavailable then return nil end
 if class=='Willie_BP_C' then return {pawn}end
 local list={};for _,a in ipairs(actors)do if not remove_retired or not a.destroyed then list[#list+1]=a end end
 return list
end
for life=100,170 do
 ctx={match_id=3,round=4,life=life};ok(pooled.component(pawn,false,10,ctx))
 ctx={match_id=3,round=4,life=life+1};pooled.prune()
 local retired=actors[#actors];ok(retired.destroyed)
 remove_retired=true;pooled.prune();remove_retired=false
end
-- More than64 positive same-world retirements permit further allocation.
ctx={match_id=3,round=4,life=172};ok(pooled.component(pawn,false,10,ctx))
local last=actors[#actors];local stored_ctx=ctx;e.context=function()return nil end
pooled.prune();ok(not last.destroyed);e.context=function()return ctx end
unavailable=true;ctx={match_id=3,round=4,life=173};pooled.prune();ok(not last.destroyed);unavailable=false
pooled.prune();ok(last.destroyed)
-- Merely requested native Destroy retains slot: same-life retry refuses.
ctx=stored_ctx;ok(not pooled.component(pawn,false,10,ctx))
remove_retired=true;pooled.prune();ok(pooled.component(pawn,false,10,ctx))
local replacement=actors[#actors];replacement.name=replacement.name..'_replacement'
ctx={match_id=3,round=4,life=174};pooled.prune();ok(not replacement.destroyed)
ok(pooled.component(pawn,false,10,ctx))
local garbage=actors[#actors];function garbage:HasAnyFlags(flag)ok(flag==0x40000000);return true end
function garbage:GetWorld()error('retired native World must not be read')end
pooled.prune();ok(pooled.component(pawn,false,10,ctx))
print('staged replay fists retirement: '..n..' checks PASS')


-- Independent replacement-pawn regression: positive generation survives the
-- destruction/replacement of the original native pawn; session loss does not.
local replaced=M.new(e)
local include_old,old_pawn,allow_current=false,nil,true
local function next_pawn(id)
 local x=object(id,'replacement_pawn'..id);x.Mesh={GetSocketTransform=function()return {Scale3D={X=1,Y=1,Z=1}}end}
 function x:GetWorld()return native_world end
 return x
end
e.current_generation=function(peer)ok(peer==77);return allow_current and ctx or nil end
e.current_actor=function(peer)ok(peer==77);return pawn,allow_current and ctx or nil end
e.context=function()return nil end
e.find_all=function(class)
 if unavailable then return nil end
 if class=='Willie_BP_C' then return include_old and {pawn,old_pawn} or {pawn}end
 local list={};for _,a in ipairs(actors)do if not remove_retired or not a.destroyed then list[#list+1]=a end end
 return list
end
for life=200,270 do
 pawn=next_pawn(1000+life);ctx={match_id=3,round=4,life=life,peer_id=77}
 e.context=function(p)return p==pawn and ctx or nil end
 ok(replaced.component(pawn,false,10,ctx));local source=actors[#actors]
 old_pawn=pawn;pawn=next_pawn(2000+life);ctx={match_id=3,round=4,life=life+1,peer_id=77}
 allow_current=false;replaced.prune();ok(not source.destroyed)
 allow_current=true;unavailable=true;replaced.prune();ok(not source.destroyed);unavailable=false
 replaced.prune();ok(source.destroyed)
 remove_retired=true;replaced.prune();remove_retired=false
end
-- Same-generation replacement requires positive current-pawn proof and full
-- absence of the original pawn, not merely a session or lookup failure.
pawn=next_pawn(5000);ctx={match_id=3,round=4,life=400,peer_id=77}
ok(replaced.component(pawn,true,10,ctx));local source=actors[#actors];old_pawn=pawn;pawn=next_pawn(5001)
include_old=true;replaced.prune();ok(not source.destroyed)
include_old=false;e.current_actor=function()return pawn,nil end;replaced.prune();ok(not source.destroyed)
e.current_actor=function()return pawn,ctx end;replaced.prune();ok(source.destroyed)
-- A surviving changed lineage is never destroyed, even when Owner is null.
remove_retired=true;replaced.prune();remove_retired=false
ok(replaced.component(pawn,true,10,ctx));local changed=actors[#actors]
changed.owner=nil;changed['Parent Actor']=next_pawn(9999)
ctx={match_id=3,round=4,life=401,peer_id=77};replaced.prune();ok(not changed.destroyed)
print('production replacement-pawn retirement: '..n..' checks PASS')
