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
