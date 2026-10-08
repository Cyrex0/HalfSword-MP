local M=dofile(T.path("mods/HSMPSync/Scripts/pose_context.lua"))
local SP=dofile(T.path("mods/HSMPSync/Scripts/spawn_place.lua"))
local v={match_id=51,round=0,spawn_round=1,phase_name="countdown",spawns={[1]={spawn_id=256}}}
local plan=SP.plan_from_view(v)
T.check(plan.match_id==51 and plan.by_peer[1].life==1,"initial order owns next-round life1 before Mode on_live")
local s={match_id=51,round=1,life=1,verified=true,pawn="Pawn_A",spawn_id=256}
T.check(M.of(s,v,nil,1,"Pawn_A").life==1,"verified initial placement permits countdown pose")
v.phase_name="loading"
T.check(M.of(s,v,nil,1,"Pawn_A").round==1,"Loading shares target countdown spawn round")
v.phase_name="live";v.round=1
local mode={match_id=51,round=1,rows={[1]={life=1,respawning=false}}}
T.check(M.of(s,v,mode,1,"Pawn_A").match_id==51,"live exact original placement context")
mode.rows[1].life=2
T.check(M.of(s,v,mode,1,"Pawn_A")==nil,"old pawn status cannot relabel itself as new life")
mode.rows[1].life=1;v.match_id=52;mode.match_id=52
T.check(M.of(s,v,mode,1,"Pawn_A")==nil,"same pawn and spawn256 in new match cannot reuse old verified status")
v.match_id=51;mode.match_id=51
T.check(M.of(s,v,mode,1,"Pawn_B")==nil,"new native pawn requires its own placement")
T.check(M.of(s,v,nil,1,"Pawn_A")==nil,"missing authoritative live mode fails closed")
v.spawns[1].spawn_id=386;mode.rows[1].life=2
local p=SP.plan_from_view(v,mode)
T.check(p.by_peer[1].life==2,"respawn order captures full matching generation")
mode.rows[1].life=3
T.check(SP.plan_from_view(v,mode).by_peer[1].life==0,"Mode/order mismatch cannot bind pose")
mode.rows[1].life=130
T.check(SP.plan_from_view(v,mode).by_peer[1].life==130,"wrapped order bits retain full authoritative life")
T.check(p.by_peer[1].life==2,"later Mode updates do not mutate original placement context")

v.spawns[1].spawn_id=386
s.spawn_id,s.life=386,2
mode.id="deathmatch";mode.rows[1]={life=2,respawning=true}
local preparing=M.for_publication(s,v,mode,1,"Pawn_A")
T.check(preparing and preparing.life==2 and preparing.match_id==51 and preparing.round==1,
    "verified exact deathmatch reload publishes its original life2 before LOADED")
T.check(M.of(s,v,mode,1,"Pawn_A")==nil,"preparation grants no active death context")
s.life=130;mode.rows[1].life=130
T.check(M.for_publication(s,v,mode,1,"Pawn_A").life==130,
    "wrapped respawn order retains full authoritative life130")
s.life=2
T.check(M.for_publication(s,v,mode,1,"Pawn_A")==nil,"same low bits cannot relabel old life2 as life130")
mode.rows[1].life=2
T.check(M.for_publication(s,v,mode,1,"Pawn_B")==nil,"preparation requires exact placed pawn")
s.verified=false
T.check(M.for_publication(s,v,mode,1,"Pawn_A")==nil,"unverified placement cannot start preparation streams")
s.verified=true;mode.id="duel"
T.check(M.for_publication(s,v,mode,1,"Pawn_A")==nil,"respawning flag in another mode grants no preparation")
mode.id="deathmatch";s.spawn_id=256
T.check(M.for_publication(s,v,mode,1,"Pawn_A")==nil,"previous round assignment cannot authorize a reload")
for _,sid in ipairs({258,384,642}) do
    s.spawn_id=sid;v.spawns[1].spawn_id=sid
    T.check(M.for_publication(s,v,mode,1,"Pawn_A")==nil,
        "matching but invalid marker, low life bits or round in order fails closed: "..sid)
end
s.spawn_id=386;v.spawns[1].spawn_id=386;v.phase_name="roundover"
T.check(M.for_publication(s,v,mode,1,"Pawn_A")==nil,"roundover cannot start respawn preparation")
v.phase_name="live";mode.rows[1].respawning=false
T.check(M.of(s,v,mode,1,"Pawn_A").life==2 and M.for_publication(s,v,mode,1,"Pawn_A").life==2,
    "completed reload returns to existing strict active context")
