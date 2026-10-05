local S = dofile(T.path("mods/HSMPHud/Scripts/hud_state.lua"))
local function record(dism, flags)
    local v = {}
    for i = 1, 19 do v[i] = 100 * 64 end
    v[16] = 0
    return S.vitals_from_record({ v = v, flags = flags or 0, dism = dism or 0 })
end
local intact = record()
T.check(intact.body == 100 and not intact.severed, "intact regenerated body is healthy")
local severed = record(1 << 11)
T.check(severed.hp == 100 and severed.body < 95 and severed.severed,
    "regenerated raw health cannot conceal a severed lower arm")
local headless = record(0, 8)
T.check(headless.body < 80 and headless.severed, "headless flag overrides regenerated head health")
local both = record((1 << 11) | (1 << 21))
T.check(both.body < severed.body, "multiple severed regions reduce displayed body condition")
local fresh = record()
T.check(fresh.body == 100 and not fresh.severed, "a fresh body has no previous life injury")
local crush_record = { seq=5, flags=0, dism=0, v={} }
for i=1,19 do crush_record.v[i] = 6400 end
crush_record.v[11] = 640 -- Head Health (Crush) = 10, normal head health = 100
local crushed = S.vitals_from_record(crush_record)
T.check(crushed.body < 80 and not crushed.severed,
    "regenerated ordinary head health cannot conceal severe native crush damage")
crush_record.v[2], crush_record.v[11] = 640, 6400
T.check(S.vitals_from_record(crush_record).body == crushed.body,
    "the worse native head-health channel determines head condition")
crush_record.v[11] = 65535
T.check(S.vitals_from_record(crush_record).body == crushed.body,
    "unknown crush health preserves the known ordinary head condition")
crush_record.v[2], crush_record.v[11] = 65535, 640
T.check(S.vitals_from_record(crush_record).body == crushed.body,
    "known crush health remains visible when ordinary head health is unknown")
for _, bit in ipairs({32,64,128,256,512,1024,2048}) do
    local r = record(0, bit)
    T.check(r.body < 100 and r.injured and not r.severed and r.hp == 100,
        "current native injury bit " .. bit .. " survives numeric health regeneration")
end
T.check(record(0,4096).body == 100 and not record(0,4096).injured,
    "reserved flag bits do not fabricate a regional injury")
T.check(record().body == 100 and not record().injured,
    "native injury clearing or a fresh life restores functional HUD condition")

local view = {match_id=91,round=3,state="live"}
local mode = {match_id=91,round=3,rows={[2]={life=130}}}
local scoped = {match_id=91,round=3,life=130,flags=1,dism=1<<11,v=crush_record.v}
T.check(S.current_vitals(scoped,view,mode,2).dead,
    "fresh original generation displays native death and injuries")
for _,ctx in ipairs({{90,3,130},{91,2,130},{91,3,2},{91,3,0}}) do
    scoped.match_id,scoped.round,scoped.life = table.unpack(ctx)
    T.check(S.current_vitals(scoped,view,mode,2)==nil,
        "stale/zero HUD generation suppressed even when spawn ids alias")
end
scoped.match_id,scoped.round,scoped.life=91,3,130
view.state="countdown"
T.check(S.current_vitals(scoped,view,mode,2)==nil,"countdown hides preceding round corpse")
scoped.round,scoped.life=4,1
T.check(S.current_vitals(scoped,view,mode,2)~=nil,"countdown accepts original pending round life one")
mode.match_id=92
T.check(S.current_vitals(scoped,view,mode,2)==nil,"mixed session/mode cannot authorize HUD record")
T.check(S.match_from(view).match_id==91,"HUD match retains original match identity")
local d1 = S.death_of({peer_id=2,match_id=91,round=3,life=1})
local d2 = S.death_of({peer_id=2,match_id=91,round=3,life=2})
T.check(S.death_key({},d1)~=S.death_key({},d2),"same-round respawn deaths have independent feed keys")
T.check(S.death_key({},d1)==S.death_key({},S.death_of({peer_id=2,match_id=91,round=3,life=1})),
    "reliable repeat of same-life death remains deduplicated")
local Mo = dofile(T.path("mods/HSMPHud/Scripts/hud_model.lua"))
local snap={sc={my_id=2,nicks={[3]="Opponent"}}}
T.check(Mo.death_text(snap,{victim=2,killer=3,cause=5})=="Opponent defeated You",
    "native defeat is attributed without calling it a killing")
T.check(Mo.death_text(snap,{victim=2,killer=0,cause=5})=="You were defeated",
    "unattributed native defeat does not claim death")
T.check(Mo.death_text(snap,{victim=2,killer=3,cause=6})=="You surrendered",
    "explicit surrender preserves outcome distinct from being killed")
local holdview={state="live",status="connected",match_id=81,round=2,my_peer_id=2,
    alive={[2]=true},spawns={[2]={spawn_id=512}}}
local holdmode={match_id=81,round=2,rows={[2]={life=1,alive=true}}}
local status={verified=true,match_id=81,round=2,life=1,pawn="owner",spawn_id=512}
local meter={active=true,match_id=81,round=2,life=1,pawn="owner",at_ms=5000,progress=.5,remaining_s=1.1}
local active=S.surrender_from(meter,holdview,holdmode,status,5250)
T.check(active and active.progress==.5,"verified original own hold displays its progress")
T.check(S.surrender_from(meter,holdview,holdmode,status,6001)==nil,"stale typed hold cannot remain visible")
for _,field in ipairs({"match_id","round","life","pawn"})do
    local old=meter[field];meter[field]=type(old)=="number" and old+1 or "other"
    T.check(S.surrender_from(meter,holdview,holdmode,status,5250)==nil,"HUD hold rejects wrong "..field)
    meter[field]=old
end
meter.active=false
T.check(S.surrender_from(meter,holdview,holdmode,status,5250)==nil,"release clears native hold prompt")
local centre=Mo.centre({}, {match={state="live"},surrender=active,sc={my_id=2}},5.25)
T.check(centre.title=="Hold to surrender 50%" and centre.sub=="Release to cancel",
    "hold prompt explains explicit progress and cancellation")
