-- Initial physical alignment evidence. Inputs are copied pose context and
-- actual body transforms read by the servo, never UObject or joint-angle caches.
local S = { STABLE_MS=150, POS_UU=5, ROT_DEG=10 }
S.bones = {[11]="upperarm_l",[12]="lowerarm_l",[13]="hand_l",[15]="upperarm_r",[16]="lowerarm_r",[17]="hand_r"}
S.fields = {"settle_world","settle_sample_ms","settle_stable_ms","settle_ready","settle_count",
    "settle_pos_uu","settle_rot_deg","settle_reason","settle_source_seq","settle_source_ts","settle_cut"}
local function finite(n) return type(n)=="number" and n==n and n~=math.huge and n~=-math.huge end
local function frame(t)
    if type(t)~="table" then return false end
    for i=1,7 do if not finite(t[i]) then return false end end
    local norm=t[4]^2+t[5]^2+t[6]^2+t[7]^2
    return finite(norm) and norm>0
end
local function same(a,b)
    return type(a)=="table" and type(b)=="table" and a.has_context==true and b.has_context==true
        and finite(a.match_id) and a.match_id>0 and finite(a.round) and a.round>0 and finite(a.life) and a.life>0
        and a.match_id==b.match_id and a.round==b.round and a.life==b.life
end
function S.invalidate(s,reason,now)
    if not s then return end
    s.since,s.last_sample,s.last_label=nil,nil,nil
    s.enabled=false
    s.settle_ready,s.settle_stable_ms=false,0
    s.settle_reason=reason or "unavailable"
    if finite(now) then s.settle_sample_ms=now end
end
function S.begin(s,world,shown,previous,current,now)
    s=s or {}
    s.enabled=false
    s.settle_world,s.settle_sample_ms=world or "",now
    s.settle_ready,s.settle_count,s.settle_pos_uu,s.settle_rot_deg=false,0,0,0
    s.settle_stable_ms,s.settle_reason=0,"sampling"
    s.settle_source_seq,s.settle_source_ts=0,0
    s.settle_cut=0
    s.fault=nil
    s.seen=0
    if type(world)~="string" or world=="" or not finite(now) or not same(shown,previous)
        or type(shown.pawn)~="string" or shown.pawn=="" or shown.pawn~=previous.pawn or not same(shown,current)
        or previous.world~=world or previous.cut~=current.cut then
        S.invalidate(s,"integrated aim context unavailable",now); return s
    end
    if s.match_id~=shown.match_id or s.round~=shown.round or s.life~=shown.life or s.pawn~=shown.pawn
        or s.world~=world or s.cut~=current.cut then
        S.invalidate(s,"new spawn",now)
        s.logged=nil
        s.match_id,s.round,s.life,s.pawn,s.world,s.cut=shown.match_id,shown.round,shown.life,shown.pawn,world,current.cut
    end
    if (current.mode~="interp" and current.mode~="extrap") or not finite(current.age) or math.abs(current.age)>250 then
        S.invalidate(s,"source held/stale",now); return s
    end
    if not finite(previous.at) or previous.at>=now or now-previous.at>250
        or not finite(previous.label) or previous.label<=0 or not finite(previous.seq) or previous.seq<0 then
        S.invalidate(s,"integrated aim unavailable/stale",now); return s
    end
    if s.last_label and (previous.label<=s.last_label or previous.at~=s.last_sample) then
        S.invalidate(s,"integrated aim not advancing",now)
    end
    s.settle_source_seq,s.settle_source_ts=previous.seq,previous.label
    s.settle_cut=previous.cut
    s.enabled=true
    return s
end
function S.measure(s,i,actual,aim)
    local bone=S.bones[i]
    if not s.enabled or not bone then return end
    s.seen=s.seen | (1<<i)
    if not frame(actual) or not frame(aim) then
        s.settle_reason="missing "..bone; s.fault=true; return
    end
    s.settle_count=s.settle_count+1
    local pos=math.sqrt((actual[1]-aim[1])^2+(actual[2]-aim[2])^2+(actual[3]-aim[3])^2)
    local an=actual[4]^2+actual[5]^2+actual[6]^2+actual[7]^2
    local bn=aim[4]^2+aim[5]^2+aim[6]^2+aim[7]^2
    local dot=math.abs(actual[4]*aim[4]+actual[5]*aim[5]+actual[6]*aim[6]+actual[7]*aim[7])/math.sqrt(an*bn)
    local rot=math.deg(2*math.acos(math.min(1,dot)))
    s.settle_pos_uu=math.max(s.settle_pos_uu,pos)
    s.settle_rot_deg=math.max(s.settle_rot_deg,rot)
    if pos>S.POS_UU then s.settle_reason=bone.." position";s.fault=true
    elseif rot>S.ROT_DEG then s.settle_reason=bone.." rotation";s.fault=true end
end
function S.finish(s,now)
    if not s.enabled then return s end
    if s.settle_count~=6 and not s.fault then
        for i,bone in pairs(S.bones) do
            if s.seen & (1<<i)==0 then s.settle_reason="missing "..bone;break end
        end
        s.fault=true
    end
    if s.fault then
        s.since=nil
    else
        s.since=s.since or now
        s.settle_stable_ms=now-s.since
        s.settle_ready=s.settle_stable_ms>=S.STABLE_MS
        s.settle_reason=s.settle_ready and "settled" or "stabilizing"
    end
    s.last_sample,s.last_label=now,s.settle_source_ts
    s.fault=nil
    return s
end
function S.copy(shown,s)
    if shown and s then for _,key in ipairs(S.fields) do shown[key]=s[key] end end
end
return S
