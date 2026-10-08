local E=dofile(T.path("mods/HSMPLoadout/Scripts/weapon_passport_equal.lua"))
local fields={{"Class","class","class"},{"Head","class","head"},{"Sub1","class","head_sub1"},
    {"Grip","class","grip"},{"Material","int","mat_steel"},{"Mass","num","mass_head"},
    {"Size","vec","head_size"},{"Color","color","color_wood"},{"Name","name","name"},{"ID","int","id"}}
local pass={Class="same-class",Head="head-A",Sub1="sub-A",Grip="grip-A",Material=4,Mass=1.1,
    Size={X=1,Y=1.0001,Z=1},Color={R=.2,G=.3,B=.4,A=1},Name={ToString=function()return "variant"end},ID=7}
local wanted={class="same-class",head="head-A",head_sub1="sub-A",grip="grip-A",mat_steel=4,
    mass_head=1.1,head_size={1,1.0001,1},color_wood={.2,.3,.4,1},name="variant",id=7}
local path=function(value)return value or ""end
local original_key=E.signature(wanted,fields)
T.check(E.matches(pass,wanted,fields,path),"identical full native Passport permits same actor reuse")
for _,key in ipairs({"head","head_sub1","grip","mat_steel","mass_head","id","name"})do
    local old=wanted[key];wanted[key]=type(old)=="number"and old+1 or old.."changed"
    T.check(not E.matches(pass,wanted,fields,path),"same actor class cannot conceal changed "..key)
    T.check(E.signature(wanted,fields)~=original_key,"hands application key includes changed "..key)
    wanted[key]=old
end
wanted.head_size[2]=1.0002
T.check(not E.matches(pass,wanted,fields,path),"submillimeter module size change is not erased by rounded signature")
T.check(E.signature(wanted,fields)~=original_key,"exact shape variant reaches hand applier even when class is unchanged")
wanted.head_size[2]=1.0001
pass.Mass=string.unpack("<f",string.pack("<f",1.1))
T.check(E.matches(pass,wanted,fields,path),"binary f32 wire representation matches native f64 without approximate tolerance")
wanted.color_wood[1]=.2001
T.check(not E.matches(pass,wanted,fields,path),"exact material colour change invalidates same-class reuse")
wanted.color_wood[1]=.2
pass.Mass=0/0
T.check(not E.matches(pass,wanted,fields,path),"unreadable/nonfinite native Passport cannot authorize reuse")
pass.Mass=1.1
local unreadable=setmetatable({}, {__index=function()error("native field unavailable")end})
T.check(not E.matches(unreadable,wanted,fields,path),"native read failure falls through to construction instead of class-only reuse")
