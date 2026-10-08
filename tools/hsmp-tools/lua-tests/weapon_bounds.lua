local B = dofile(T.path("mods/HSMPSync/Scripts/weapon_bounds.lua"))
local reads, logs = 0, {}
FName = function(s) return s end
local function xf(x,y,z,q,s)
    return {Translation={X=x,Y=y,Z=z},Rotation={X=q[1],Y=q[2],Z=q[3],W=q[4]},Scale3D={X=s[1],Y=s[2],Z=s[3]}}
end
local q = {0,0,math.sqrt(0.5),math.sqrt(0.5)}
local module_x,module_q=1100,q
local comp = {
    IsValid=function() return true end,
    GetLocalBounds=function(_,lo,hi) reads=reads+1; lo.X,lo.Y,lo.Z=-20,-5,-3; hi.X,hi.Y,hi.Z=20,5,3 end,
    GetSocketTransform=function(_,bone,space) T.check(bone=="None" and space==0,"module transform is world space"); return xf(module_x,2000,3000,module_q,{2,1,1}) end,
    GetChildrenComponents=function()return {}end,
}
local bad = {IsValid=function() return false end}
local w = {name="axe1",GetAddress=function() return 42 end,GetFName=function(s) return {ToString=function() return s.name end} end,
    GetTransform=function() return xf(1000,2000,3000,{0,0,0,1},{1,1,1}) end,IsValid=function()return true end}
w["Collision Components Array"]={ForEach=function(_,f)
    f(0,{get=function()return bad end}); f(1,{get=function()return comp end})
end}
local function log(...) logs[#logs+1]=string.format(...) end
local b=B.of(w,"world1",0,log)
T.check(#b==15 and b[1]==2,"unavailable first component does not renumber second component")
T.check(math.abs(b[2]-100)<1e-5 and b[3]==0 and b[4]==0,"module center is relative to weapon")
T.check(math.abs(b[7]-q[3])<1e-6 and b[9]==40 and b[10]==5 and b[11]==3,"module rotation and nonuniform scale preserved")
T.check(reads==1 and T.contains(logs[1],"1 module/cutting row(s)"),"shape availability is explicit")
module_x,module_q=1125,{0,0,0,1}
local moved=B.of(w,"world1",0.005,log)
T.check(moved~=b and reads==2,"each physics sample refreshes independently simulated module geometry")
T.check(moved[2]==125 and moved[7]==0 and moved[8]==1,"flail translation and rotation update inside former one-second cache window")
T.check(b[2]==100 and math.abs(b[7]-q[3])<1e-6,"a later frame cannot mutate a queued prior pose's bounds")
w.name="axe2"; B.of(w,"world1",0.6,log)
T.check(reads==3,"reused address with new actor identity refreshes geometry")
B.of(w,"world2",0.7,log)
T.check(reads==4,"world change invalidates numeric geometry")
w["Collision Components Array"]={ForEach=function(_,f)
    for i=1,15 do local c=(i==10 or i==15) and comp or bad; f(i-1,{get=function()return c end}) end
end}
local sparse=B.of(w,"world3",1,log)
T.check(#sparse==30 and sparse[1]==10 and sparse[16]==15,"sparse native component IDs10 and15 survive independent of max8 boxes")
w["Collision Components Array"]={ForEach=function(_,f)
    for i=1,15 do f(i-1,{get=function()return comp end}) end
end}
local capped=B.of(w,"world4",2,log)
T.check(#capped==0 and T.contains(logs[#logs],"complete native geometry unavailable") and T.contains(logs[#logs],"activeRows=12"),"active capacity overflow omits whole geometry instead of silently truncating with precise native row evidence")
local primitive={IsValid=function()return true end,GetClass=function()return {GetFName=function()return {ToString=function()return "BoxComponent"end}end}end,
    GetChildrenComponents=function()return {}end,
    GetLocalBounds=function()error("native Box must not use render mesh bounds")end,
    GetUnscaledBoxExtent=function()return {X=30,Y=2,Z=12}end,
    GetSocketTransform=function()return xf(1100,2000,3000,{0,0,0,1},{2,1.25,0.75})end}
w.GetClass=function()return {GetFName=function()return {ToString=function()return "ModularWeaponBP_ArmingSword_C"end}end}end
w["Collision Components Array"]={ForEach=function(_,f)f(0,{get=function()return primitive end})end}
local native=B.of(w,"nativebox",3,log)
T.check(#native==15 and native[9]==60 and native[10]==2.5 and native[11]==9,"actual cutting Box uses native extent times world scale")
T.check(native[12]==2 and native[13]==1.25 and native[14]==0.75 and native.class_hash~=0,"independent historical native scale and full class identity accompany pose Box")
local empty_skeletal={IsValid=function()return true end,GetClass=function()return {GetFName=function()return {ToString=function()return "SkeletalMeshComponent"end}end}end,
    GetChildrenComponents=function()return {}end,GetSkeletalMeshAsset=function()return nil end,GetLocalBounds=function()error("unconfigured skeletal slot must not call unsupported bounds")end}
w["Collision Components Array"]={ForEach=function(_,f)for i=1,9 do f(i-1,{get=function()return i==1 and primitive or empty_skeletal end})end end}
local inherited=B.of(w,"empty-slots",4,log)
T.check(#inherited==15 and inherited[1]==1,"nine valid inherited wrappers with eight unconfigured skeletal assets preserve actual one-module geometry")

do
local B=dofile(T.path("mods/HSMPSync/Scripts/weapon_bounds.lua"))
local count=5000
local function obj(class,name)
    count=count+1;local id=count
    return {IsValid=function()return true end,GetAddress=function()return id end,
        GetFName=function()return{ToString=function()return name end}end,
        GetClass=function()return{IsValid=function()return true end,GetFName=function()return{ToString=function()return class end}end}end,
        ComponentHasTag=function()return false end,GetChildrenComponents=function()end}
end
local function xf()return{Translation={X=0,Y=0,Z=0},Rotation={X=0,Y=0,Z=0,W=1},Scale3D={X=1,Y=1,Z=1}}end
local weapon=obj("ModularWeaponBP_Polearm_Mid_Tier_C","ActualFiveModulePolearm")
weapon.GetTransform=xf
local children={}
local function module(index)
    local p=obj("StaticMeshComponent","Module"..index)
    p.StaticMesh={IsValid=function()return true end};p.GetOwner=function()return weapon end
    p.GetSocketTransform=xf
    p.GetLocalBounds=function(_,lo,hi)lo.X,lo.Y,lo.Z=-5,-2,-1;hi.X,hi.Y,hi.Z=5,2,1 end
    local c=obj("BoxComponent","Cutting"..index);children[index]=c
    c.GetOwner=function()return weapon end;c.GetAttachParent=function()return p end;c.GetSocketTransform=xf
    c.GetUnscaledBoxExtent=function()return{X=5,Y=2,Z=1}end
    p.GetChildrenComponents=function(_,recursive,out)out[1]={get=function()return c end}end
    return p
end
local emptySk=obj("SkeletalMeshComponent","EmptySk");emptySk.GetSkeletalMeshAsset=function()return nil end
local emptyStatic=obj("StaticMeshComponent","EmptyStatic")
local actual={module(1),module(2),module(3),emptySk,emptyStatic,emptySk,module(7),module(8),emptySk}
weapon["Collision Components Array"]={ForEach=function(_,f)for i,c in ipairs(actual)do f(i,{get=function()return c end})end end}
local rows=B.of(weapon,"five-module-native-layout",1)
T.check(#rows==10*15,"observed polearm five configured modules and five cutting children fit without losing any geometry")
T.check(rows[1]==1 and rows[4*15+1]==8 and rows[5*15+1]==10 and rows[9*15+1]==14,
    "observed sparse module ordinals and deterministic cutting virtual IDs survive capacity expansion")
T.check(rows[9*15+15]==8,"last child retains its actual Pommel parent ordinal")
actual={};for i=1,6 do actual[i]=module(i)end
T.check(#B.of(weapon,"max12",2)==12*15,"six configured modules plus six cutting children all fit")
actual={};for i=1,7 do actual[i]=module(i)end
local logs={}
local overflow=B.of(weapon,"too-many-active",3,function(...)logs[#logs+1]=string.format(...)end)
T.check(#overflow==0 and T.contains(logs[#logs],"activeRows=12"),"fourteen active rows fail completely with precise row-capacity evidence")

end
