local P=dofile(T.path("mods/HSMPAvatars/Scripts/joint_profile_session.lua"))
local function records()
    return {seq=2,server_time_ms=100,match_id=419,round=0,phase=1,config={arena="Arena"},
        rows={{peer_id=1,loaded_round=0,waiting=true,spawn_id=257,spawn_pos={0,0,0}},
            {peer_id=2,loaded_round=0,waiting=true,spawn_id=258,spawn_pos={10,0,0}}}},
        {seq=8,server_time_ms=100,match_id=419,round=0,rows={{peer_id=1,life=0,alive=false}}}
end
local s,m=records();local a,meta=P.capture(s,m)
T.check(a and meta.session.seq==2 and meta.mode.server_time_ms==100 and a.mode.rows[1].alive==false,
    "raw zero/false and original heartbeat metadata are copied without defaults")
s.seq,s.server_time_ms,m.seq,m.server_time_ms=3,400,9,400
local b=P.capture(s,m)
T.check(P.same(a,b),"only root Session/Mode seq and server clock may change")
s.rows[2].loaded_round=1
local equal,field,expected,observed=P.same(a,P.capture(s,m))
T.check(not equal and field=="raw.session.rows.2.loaded_round"and expected==0 and observed==1,
    "another roster row load transition remains exact")
s,m=records();m.rows[1].seq=3
T.check(not P.same(a,P.capture(s,m)),"nested seq is semantic, not a stripped heartbeat")
s,m=records();s.future={server_time_ms=1}
T.check(not P.same(a,P.capture(s,m)),"unknown future fields and nested server clocks are preserved")
s,m=records();m.rows[1].alive=nil
T.check(not P.same(a,P.capture(s,m)),"missing alive remains distinct from actual false")
local missing=P.capture(s,nil)
T.check(missing and not missing.mode_present and not P.same(missing,P.capture(s,{})),
    "missing Mode is distinct from an available empty record")
s,m=records();local copied=P.capture(s,m);s.rows[1].spawn_pos[1]=99
T.check(copied.session.rows[1].spawn_pos[1]==0,"copied snapshot does not retain mutable raw tables")
local invalid={}
local cyc={};cyc.self=cyc;invalid[#invalid+1]=cyc
local dup={};invalid[#invalid+1]={a=dup,b=dup}
invalid[#invalid+1]=setmetatable({},{__pairs=function()error("must not enumerate")end})
invalid[#invalid+1]={bad=function()end};invalid[#invalid+1]={bad=0/0}
local oversized={};for i=1,P.MAX_KEYS+1 do oversized[i]=i end;invalid[#invalid+1]=oversized
local refused=0;for _,v in ipairs(invalid)do if P.capture(v,nil)==nil then refused=refused+1 end end
T.check(refused==#invalid,"cyclic/duplicate/metatable/nonplain/nonfinite/oversize records fail closed")
s,m=records();s.seq=math.huge
T.check(P.capture(s,m)==nil,"excluded metadata cannot conceal nonfinite heartbeat values")
s,m=records();s.rows={};m.rows={}
for i=1,64 do s.rows[i]={peer_id=i,loaded_round=0,waiting=false,spawn_id=256+i,spawn_pos={i,0,0}}
    m.rows[i]={peer_id=i,life=1,alive=true}end
local full=P.capture(s,m)
T.check(full and P.same(full,P.capture(s,m)),"schema-sized 64-row Session and Mode snapshots remain bounded and comparable")
