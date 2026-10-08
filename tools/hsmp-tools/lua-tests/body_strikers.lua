local M=dofile(T.path("mods/HSMPSync/Scripts/body_strikers.lua"))
FName=function(n)return n end
local function class(n)return {GetFName=function()return {ToString=function()return n end}end}end
local function xf(x,y,z)return {Translation={X=x,Y=y,Z=z},Rotation={X=0,Y=0,Z=0,W=1},Scale3D={X=1,Y=1,Z=1}}end
local function primitive(kind,x)
    return {IsValid=function()return true end,GetClass=function()return class(kind)end,
        GetSocketTransform=function()return xf(x,20,30)end,GetScaledSphereRadius=function()return 13 end,
        GetUnscaledBoxExtent=function()return {X=7.475,Y=14.949,Z=7.475}end}
end
local sphere=primitive("SphereComponent",112)
local box=primitive("BoxComponent",108)
local inert={IsValid=function()return true end,GetClass=function()return class("StaticMeshComponent")end}
local function actor(n,c)
    return {IsValid=function()return true end,GetClass=function()return class(n)end,
        ["Collision Components Array"]={ForEach=function(_,f)for i=1,10 do local a=i==10 and c or inert;f(i-1,{get=function()return a end})end end}}
end
local mesh={GetSocketTransform=function(_,bone,space)T.check(space==0,"source bone transform in world space");return xf(100,20,30)end}
local pawn={["Weapon R"]=actor("Weapon_Fists_C",sphere),["Foot L Weapon"]=actor("Weapon_Feet_C",box)}
local s=M.of(pawn,mesh)
T.check(#s==26 and s[1]==1 and s[2]==10 and s[3]==0 and s[11]==13,"native fist source, sparse Sphere10 and measured radius retained")
T.check(s[4]==12 and s[14]==4 and s[15]==10 and s[16]==1 and s[24]==7.475 and s[25]==14.949,"actual bone-relative fist offset and native left-foot box dimensions")
sphere.GetSocketTransform=function()return xf(120,20,30)end
T.check(M.of(pawn,mesh)[4]==20,"primitive offset resampled every frame, not cached")
pawn["Foot L Weapon"]=nil
T.check(#M.of(pawn,mesh)==13,"transient native foot disappearance is explicit")
pawn["Weapon R"]=actor("ModularWeaponBP_Sword_C",sphere)
T.check(#M.of(pawn,mesh)==0,"held sword cannot become a named fist shape")
