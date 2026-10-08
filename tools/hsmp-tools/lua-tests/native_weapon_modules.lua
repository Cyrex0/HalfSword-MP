local M=dofile(T.path("mods/shared/native_weapon_modules.lua"))
FName=function(s)return s end
local nextid=0
local function object(class)
    nextid=nextid+1;local id=nextid
    return {IsValid=function()return true end,GetAddress=function()return id end,
        GetClass=function()return{IsValid=function()return true end,GetFName=function()return{ToString=function()return class end}end}end,
        ComponentHasTag=function()return false end,GetChildrenComponents=function()return{}end}
end
local weapon=object("ModularWeaponBP_Polearm_Mid_Tier_C")
local root,head,grip,box,tip=object("StaticMeshComponent"),object("StaticMeshComponent"),object("StaticMeshComponent"),object("BoxComponent"),object("BoxComponent")
for _,c in ipairs({root,head,grip,box,tip})do c.GetOwner=function()return weapon end end
box.GetAttachParent=function()return head end
tip.GetAttachParent=box.GetAttachParent
tip.ComponentHasTag=function(_,tag)return tag=="Tip"end
local unwraps=0
local function native_children(children)
    return function(_,recursive,out)
        T.check(recursive==true,"native recursive children input preserved")
        for i,c in ipairs(children)do out[i]={get=function()unwraps=unwraps+1;return c end}end
        -- Pinned GetChildrenComponents has only an out parameter: no return.
    end
end
root.GetChildrenComponents=native_children({head,box,tip})
head.GetChildrenComponents=native_children({box,tip})
weapon["Collision Components Array"]={ForEach=function(_,f)
    for i,c in ipairs({root,{IsValid=function()return false end},head,grip})do f(i-1,{get=function()return c end})end
end}
weapon["Hit Box Collision"]=box
local rows=M.of(weapon)
T.check(unwraps==5,"void native out-array RemoteUnrealParam entries unwrap before UObject validation")
T.check(#rows==4 and rows[2].id==3,"invalid slot reserves ordinal without consuming active geometry")
T.check(rows[4].id==5 and rows[4].child_of==3,"recursive alias deduplicates Box and proves nearest actual module ancestor")
T.check(M.box(weapon,head,box)==5,"native selected Box child receives deterministic virtual ID")
T.check(M.box(weapon,grip,box)==5,"native retained previous-module Box preserves current Grip collision")
T.check(M.find(weapon,5,4)==box,"receiver resolves same held weapon child without requiring its different contact-history selection")
weapon["Hit Box Collision"]=nil
T.check(M.box(weapon,head,box)==nil,"unselected arbitrary same-weapon Box cannot be captured")
weapon["Hit Box Collision"]=tip
T.check(M.box(weapon,head,tip)==nil,"Tip-tagged Box is never substituted for native cutting child")
weapon["Hit Box Collision"]=box
local other=object("OtherWeapon");box.GetOwner=function()return other end
T.check(M.box(weapon,head,box)==nil,"wrong-weapon child fails exact ownership")
box.GetOwner=function()return weapon end
box.GetAttachParent=function()return nil end
T.check(M.of(weapon)==nil,"unproved actual parent fails closed")
box.GetAttachParent=function()return head end
weapon["Collision Components Array"]={ForEach=function(_,f)for i=1,15 do f(i-1,{get=function()return i==3 and head or object("StaticMeshComponent")end})end end}
T.check(M.of(weapon)==nil,"virtual ordinal overflow is explicit whole-resolver refusal")
