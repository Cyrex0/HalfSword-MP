local M=dofile(T.path("mods/HSMPCombat/Scripts/native_sever_projection.lua"))
local function copy(t)local r={};for k,v in pairs(t)do r[k]=v end;return r end
local ctx={world="world7@Yard",peer=1,match_id=7,round=2,life=130,pawn="Willie_1",actor=10,mesh=11,mesh_name="Mesh_1"}
local function snap(rows,process)
    return {context=copy(ctx),parts={available=true,count=#rows,values=rows},
        flags={["Dismemberment In Process"]={available=true,value=process or false}}}
end
local reads=0
local env={guard=function()return true end,hidden=function()reads=reads+1;return true end}
local p=M.new()
local n,available=p.observe(ctx,nil,env)
T.check(#n==0 and not available,"initial unavailable ledger remains unknown")
n,available=p.observe(ctx,snap({}),env)
T.check(#n==0 and available,"completed empty native ledger is readable")
n,available=p.observe(ctx,snap({{part=3,value=false}}),env)
T.check(#n==0 and reads==0,"false-present native part never qualifies missing body")
n,available=p.observe(ctx,snap({{part=3,value=true}},true),env)
T.check(#n==0 and reads==0,"in-process cut cannot add missing roots")
env.hidden=function()reads=reads+1;return false end
n=p.observe(ctx,snap({{part=3,value=true}}),env)
T.check(#n==0,"map true without actual current-mesh hide does not remove a body")
env.hidden=function()return true end
n=p.observe(ctx,snap({{part=3,value=true}}),env)
T.check(#n==1 and n[1]=="lowerarm_r","native part3 projects distal lowerarm, never proximal Spawn Bone upperarm")
n,available=p.observe(ctx,nil,env)
T.check(#n==1 and available,"unavailable read preserves confirmed positive within exact body life")
n,available=p.observe(nil,nil,env)
T.check(#n==1 and available,"temporary unavailable identity does not erase irreversible evidence")
n,available=p.observe(ctx,nil,env)
T.check(#n==1 and available,"same identity recovers with unavailable ledger and retains sever")
n=p.observe(ctx,snap({}),env)
T.check(#n==1,"transient empty ledger cannot regrow confirmed limb")
local supported={[3]="lowerarm_r",[4]="hand_r",[6]="lowerarm_l",[7]="hand_l",
    [9]="calf_r",[10]="foot_r",[12]="calf_l",[13]="foot_l"}
for part=0,14 do
    local one=M.new()
    n=one.observe(ctx,snap({{part=part,value=true}}),env)
    T.check((supported[part] and #n==1 and n[1]==supported[part]) or (not supported[part] and #n==0),
        "exact native Hide Bone Local projection for enum "..part)
end
for _,key in ipairs({"world","peer","match_id","round","life","pawn","actor","mesh","mesh_name"})do
    local changed=copy(ctx)
    changed[key]=type(changed[key])=="number" and changed[key]+1 or changed[key].."new"
    n,available=p.observe(changed,nil,env)
    T.check(#n==0 and not available,"changed "..key.." resets previous sever evidence")
    p.observe(ctx,snap({{part=3,value=true}}),env)
end
local one=M.new()
local bad=snap({{part=3,value=true},{part=3,value=true}})
n,available=one.observe(ctx,bad,env)
T.check(#n==0 and not available,"duplicate ledger keys reject whole staged update")
bad=snap({{part=3,value=true}});bad.parts.count=2
n,available=one.observe(ctx,bad,env)
T.check(#n==0 and not available,"count mismatch cannot qualify cut")
bad=snap({{part=15,value=true}})
n,available=one.observe(ctx,bad,env)
T.check(#n==0 and not available,"MAX enum rejected")
bad=snap({{part=3,value=1}})
n,available=one.observe(ctx,bad,env)
T.check(#n==0 and not available,"nonboolean value rejected")
bad=snap({{part=3,value=true}});bad.context.life=1
n,available=one.observe(ctx,bad,env)
T.check(#n==0 and not available,"other life snapshot rejected, full life130 preserved")
local guards=0
local changing={guard=function()guards=guards+1;return guards<3 end,hidden=function()return true end}
n,available=one.observe(ctx,snap({{part=3,value=true}}),changing)
T.check(#n==0 and not available,"scope changes after hidden read discard staged missing body")
n,available=one.observe(ctx,snap({{part=3,value=true}}),{guard=function()return true end,hidden=function()error("unreadable")end})
T.check(#n==0,"unreadable native hide never grants sever evidence")
one.observe(ctx,snap({{part=3,value=true}}),env)
one.reset();n,available=one.observe(ctx,nil,env)
T.check(#n==0 and not available,"world reset drops retained cut ledger")
