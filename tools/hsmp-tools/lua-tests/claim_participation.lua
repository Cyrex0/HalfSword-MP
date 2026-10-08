local M=dofile(T.path("mods/HSMPCombat/Scripts/claim_participation.lua"))
local function fixture()
    local q={attacker=1,target=2,match_id=42,round=3,attacker_life=1,victim_life=130}
    local slots={
        link={status=3,my_peer_id=1},
        session={seq=10,match_id=42,round=3,phase=4,rows={{peer_id=1,connected=true},{peer_id=2,connected=true}}},
        mode={seq=20,match_id=42,round=3,sent_ms=0,rows={{peer_id=1,life=1,alive=true},{peer_id=2,life=130,alive=true}}},
    }
    local versions={link=2,session=4,mode=6}
    local ipc={S={ENUMS={sidecar_status={CONNECTED=3}}},N={}}
    ipc.N.ipc_info=function()return {sidecar_state="ready",sidecar_hb_age_s=.1}end
    ipc.rec=function(name)return slots[name],versions[name]end
    ipc.get=function(name)return versions[name]end
    return ipc,q,slots,versions
end
do
    local ipc,q,s=fixture()
    T.check(M.known_down(ipc,q,5)==false,"old unchanged Mode is current participation when native slots and heartbeat are current")
    s.mode.rows[2].alive=false
    local down,why=M.known_down(ipc,q,5)
    T.check(down==true and why=="target_down","exact life130 target boolean false is a known-down veto")
    s.mode.rows[1].alive=false;s.mode.rows[2].alive=true
    down,why=M.known_down(ipc,q,5)
    T.check(down==true and why=="attacker_down","exact original attacker life boolean false is a known-down veto")
end
do
    local ipc,q,s=fixture();s.mode.rows[2].alive=false;s.mode.rows[2].life=1
    T.check(M.known_down(ipc,q,5)==nil,"life1 cannot eliminate the expected full life130")
    s.mode.rows[2].life=130;s.mode.match_id=43
    T.check(M.known_down(ipc,q,5)==nil,"different match cannot supply an eliminated participant")
    s.mode.match_id=42;s.mode.round=2
    T.check(M.known_down(ipc,q,5)==nil,"different round cannot supply an eliminated participant")
end
do
    local ipc,q,s=fixture();s.mode.rows[2].alive=nil
    T.check(M.known_down(ipc,q,5)==nil,"missing raw alive is unknown instead of normalized false")
    s.mode.rows[2].alive=0
    T.check(M.known_down(ipc,q,5)==nil,"numeric alive is unknown instead of a native boolean")
    s.mode.rows[2].alive=false;s.mode.rows[3]={peer_id=2,life=130,alive=true}
    T.check(M.known_down(ipc,q,5)==nil,"duplicate conflicting Mode rows cannot prove target participation")
    s.mode.rows[3]=nil;s.session.rows[2]=nil
    T.check(M.known_down(ipc,q,5)==nil,"missing connected Session target row cannot authorize the veto")
end
do
    local ipc,q,s=fixture();s.mode.rows[2].alive=false
    local cached={sidecar_state="ready",sidecar_hb_age_s=.1}
    ipc.info=cached;ipc.refresh_info=function()return cached end
    ipc.N.ipc_info=function()return nil end
    T.check(M.known_down(ipc,q,5)==nil,"failed direct native header cannot borrow retained healthy facade info")
    ipc.N.ipc_info=function()return {sidecar_state="ready",sidecar_hb_age_s=5.01}end
    T.check(M.known_down(ipc,q,5)==nil,"expired heartbeat cannot certify retained Mode-down")
    ipc.N.ipc_info=function()error("header failed")end
    T.check(M.known_down(ipc,q,5)==nil,"native header exception remains unknown")
end
do
    local ipc,q,s,v=fixture();s.mode.rows[2].alive=false
    ipc.get=function(name)if name=="mode"then return v.mode+2 end return v[name]end
    T.check(M.known_down(ipc,q,5)==nil,"Mode version changing during authority capture remains unknown")
    ipc.get=function(name)return v[name]end
    local n=0;ipc.N.ipc_info=function()n=n+1;return {sidecar_state="ready",sidecar_hb_age_s=n==1 and .1 or math.huge}end
    T.check(M.known_down(ipc,q,5)==nil,"heartbeat changing during slot reads cannot publish a known-down veto")
end
do
    local ipc,q,s=fixture();s.mode.rows[2].alive=false
    for i=3,65 do s.mode.rows[i]={peer_id=i,life=1,alive=true}end
    T.check(M.known_down(ipc,q,5)==nil,"Mode scan exceeding native64-row cap stays unknown")
    s.mode.rows={s.mode.rows[1],s.mode.rows[2]};s.link.my_peer_id=3
    T.check(M.known_down(ipc,q,5)==nil,"another connected peer cannot supply the original attacker authority")
end
