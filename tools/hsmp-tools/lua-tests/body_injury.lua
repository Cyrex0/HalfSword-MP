local I = dofile(T.path("mods/HSMPAvatars/Scripts/body_injury.lua"))
local P = dofile(T.path("mods/HSMPAvatars/Scripts/avatars_pure.lua"))
local calls, fail = {}, false
local mesh = {SetAllBodiesBelowPhysicsDisabled=function(_,bone,disabled,include)
    if fail and not disabled then error("restore refused") end
    calls[#calls+1] = {bone,disabled,include}
end}
local fname = function(n) return n end
local state = I.apply(mesh,{}, {lowerarm_l=true},fname)
T.check(state.lowerarm_l and #calls==1 and calls[1][2] and calls[1][3],"native subtree disable includes severed root")
I.apply(mesh,state,{lowerarm_l=true},fname)
T.check(#calls==1,"unchanged desired injury does not spam native solver changes")
state = I.apply(mesh,state,{hand_l=true},fname)
T.check(#calls==3 and calls[2][1]=="lowerarm_l" and not calls[2][2] and calls[3][1]=="hand_l" and calls[3][2],
    "restore old ancestor before disabling replacement child")
fail=true
local kept,why = I.apply(mesh,state,{},fname)
T.check(kept.hand_l and why=="restore failed","failed restore retains ownership for retry")
fail=false
state = I.restore(mesh,kept,fname)
T.check(not next(state) and calls[#calls][2]==false,"pooled release restores only successfully disabled roots")
local targets={}
for i=1,25 do targets[i]={i,0,0} end
I.omit(targets,{lowerarm_l=true},P.V2_SLOTS,P.V2_PARENT)
local absent,intact=0,0
for i=1,23 do
    local b=P.V2_SLOTS[i]
    if b=="lowerarm_l" or b=="hand_l" then if targets[i]==nil then absent=absent+1 end
    elseif targets[i] then intact=intact+1 end
end
T.check(absent==2 and intact==21 and targets[24] and targets[25],"only severed subtree body targets omitted; root and weapons survive")
I.omit(targets,{},P.V2_SLOTS,P.V2_PARENT)
T.check(targets[1]~=nil,"empty injury roots terminate safely on self-parented pelvis")
local shown={has_context=true,match_id=81,round=2,life=1,pawn="owned-proxy"}
local v={seq=1,match_id=81,round=2,life=1,dism=1<<11}
local journal,mask=I.select_mask(nil,shown,true,v,"native-body-A")
T.check(mask==1<<11,"original displayed generation admits its exact severing mask")
journal,mask=I.select_mask(journal,shown,true,nil,"native-body-A")
T.check(mask==1<<11,"missing vitals retains previous verified severing in same life")
for _,field in ipairs({"match_id","round","life"})do
    local bad={seq=2,match_id=81,round=2,life=1,dism=0};bad[field]=bad[field]+1
    journal,mask=I.select_mask(journal,shown,true,bad,"native-body-A")
    T.check(mask==1<<11,"stale "..field.." cannot falsely heal current severing")
end
journal,mask=I.select_mask(journal,shown,false,{seq=2,match_id=81,round=2,life=1,dism=0},"native-body-A")
T.check(mask==1<<11,"unauthorized or missing current pose context does not heal same-life body")
shown.life=129
journal,mask=I.select_mask(journal,shown,true,v,"native-body-A")
T.check(mask==nil and journal.life==129,"new displayed full life awaits its own snapshot, never invents intact state")
local calls_before=#calls
state=I.apply(mesh,{lowerarm_l=true},{},fname)
T.check(not next(state) and #calls==calls_before+1 and calls[#calls][2]==false,
    "explicit empty desired set restores previously disabled owned native subtree")
local fresh={seq=1,match_id=81,round=2,life=129,dism=1<<11}
journal,mask=I.select_mask(journal,shown,true,fresh,"native-body-A")
fresh.seq=2;fresh.dism=0;journal,mask=I.select_mask(journal,shown,true,fresh,"native-body-A")
T.check(mask==1<<11,"newer same-life zero cannot regrow an owner-confirmed severed part")
fresh.seq=1;fresh.dism=1<<12;journal,mask=I.select_mask(journal,shown,true,fresh,"native-body-A")
T.check(mask==1<<11,"older same-life snapshot cannot change confirmed structural state")
fresh.seq=3;journal,mask=I.select_mask(journal,shown,true,fresh,"native-body-A")
T.check(mask==((1<<11)|(1<<12)),"later same-life severing accumulates without healing the prior subtree")
journal,mask=I.select_mask(journal,shown,false,nil,"native-body-B")
T.check(journal==nil and mask==nil,"new actor or mesh identity cannot inherit old body's injury journal")
local before=#calls
state=I.apply(mesh,{hand_l=true},{hand_l=true},fname,true)
T.check(#calls==before+1 and calls[#calls][2] and state.hand_l,"reassert unchanged exclusion after native physics rebuild")
local broken={SetAllBodiesBelowPhysicsDisabled=function()error("disable unavailable")end}
state,why=I.apply(broken,{}, {hand_l=true},fname,true)
T.check(not next(state) and why=="disable failed hand_l","native disable failure remains explicit, never claims applied physics")
local readbones={}
mesh.IsSimulatingPhysics=function(_,bone)
    readbones[#readbones+1]=bone
    if bone=="hand_l"then error("native simulation read unavailable")end
    return false
end
local sim=I.simulation(mesh,{lowerarm_l=true},P.V2_SLOTS,P.V2_PARENT,fname)
T.check(#readbones==2 and sim.lowerarm_l=="false"and sim.hand_l=="unavailable"and sim.pelvis==nil,
    "native per-bone simulation readback distinguishes unavailable and only samples excluded subtree")
local wrapping={seq=0xfffffffe,match_id=81,round=2,life=129,dism=1<<11}
journal,mask=I.select_mask(nil,shown,true,wrapping,"native-body-A")
wrapping.seq=1;wrapping.dism=1<<12;journal,mask=I.select_mask(journal,shown,true,wrapping,"native-body-A")
T.check(mask==((1<<11)|(1<<12)),"owner snapshot sequence wrap admits a genuinely advancing severing")
before=#calls
state=I.apply(mesh,{lowerarm_l=true},{lowerarm_l=true,hand_r=true},fname,true)
local enabled=false;for i=before+1,#calls do if calls[i][2]==false then enabled=true end end
T.check(not enabled and state.lowerarm_l and state.hand_r,"additional severing never temporarily enables an existing missing root")
