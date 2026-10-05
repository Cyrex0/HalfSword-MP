-- DEV ONLY: admission/continuation model. No native calls or production wiring.
local M = {}
local function context(a,b)
    return a and b and a.match_id==b.match_id and a.round==b.round and a.life==b.life
end
local function copy(a)
    local b={}; for k,v in pairs(a) do b[k]=v end; return b
end
function M.new(limit)
    return {rows={}, counts={}, limit=limit or 64}
end
function M.create(s,peer,current,event,proof)
    -- proof is server-owned validation output, never a client boolean on wire.
    if not context(current,event) or event.life<1 then return false,'creator_context' end
    if not proof or proof.peer~=peer or proof.actor_id~=event.actor_id
        or proof.world_epoch~=event.world_epoch or proof.level~=event.level
        or proof.class~=event.class or proof.passport~=event.passport
        or proof.module_generation~=event.module_generation
        or not proof.native_creation then return false,'creation_proof' end
    if event.route~='shot' and event.route~='throw' then return false,'route' end
    if event.route=='shot' and not proof.loaded_ammo_being_fired then return false,'native_shot' end
    if event.route=='throw' and not proof.original_held_then_released then return false,'native_release' end
    if not event.incarnation or event.incarnation<1 then return false,'incarnation' end
    local old=s.rows[event.actor_id]
    if old then
        if old.creator==peer and old.incarnation==event.incarnation and context(old,event)
            and old.class==event.class and old.passport==event.passport
            and old.module_generation==event.module_generation and old.route==event.route
            and old.level==event.level and old.world_epoch==event.world_epoch then return true,'duplicate' end
        return false,'id_reuse'
    end
    if (s.counts[peer] or 0)>=s.limit then return false,'capacity' end
    local row=copy(event); row.creator=peer; row.authority=peer; row.authority_version=1
    row.state='active'; row.history={}; s.rows[event.actor_id]=row
    s.counts[peer]=(s.counts[peer] or 0)+1
    return true
end
function M.sample(s,id,incarnation,authority,version,sample)
    local r=s.rows[id]
    if not r or r.state~='active' or r.incarnation~=incarnation then return false,'terminal' end
    if authority~=r.authority or version~=r.authority_version then return false,'authority' end
    if sample.level~=r.level or sample.world_epoch~=r.world_epoch
        or sample.class~=r.class or sample.module_generation~=r.module_generation
        or not sample.original_components then return false,'geometry' end
    local h=r.history
    if #h>0 and sample.at_ms<=h[#h].at_ms then return false,'timestamp' end
    h[#h+1]=copy(sample); if #h>32 then table.remove(h,1) end
    return true
end
function M.claim(s,sender,current_victim,claim)
    local r=s.rows[claim.actor_id]
    if not r or r.state~='active' or r.incarnation~=claim.incarnation then return false,'terminal' end
    if sender~=r.creator or not context(r,claim.creator)
        or not context(current_victim,claim.victim) then return false,'lineage' end
    if claim.level~=r.level or claim.world_epoch~=r.world_epoch then return false,'world' end
    -- Independent component history, never creator root/held hand reach.
    for _,h in ipairs(r.history) do
        if h.at_ms==claim.at_ms and h.components[claim.component]
            and (not claim.box or h.boxes[claim.box]) then
            return true,r.route
        end
    end
    return false,'no_source_history'
end
function M.handoff(s,id,incarnation,new_owner,expected_version)
    local r=s.rows[id]
    if not r or r.state~='active' or r.incarnation~=incarnation or r.authority_version~=expected_version then return false end
    r.authority=new_owner; r.authority_version=r.authority_version+1
    -- Creator/creation generation remain immutable across World lease transfer.
    return true
end
function M.terminal(s,id,incarnation,why)
    local r=s.rows[id]; if not r or r.incarnation~=incarnation then return false end
    r.state=why or 'terminal'; r.history={}
    -- Retain tombstone and count until actual world epoch drop. Never reuse ID.
    return true
end
function M.drop_world(s,level,epoch)
    for id,r in pairs(s.rows) do
        if r.level==level and r.world_epoch==epoch then
            s.counts[r.creator]=s.counts[r.creator]-1; s.rows[id]=nil
        end
    end
end
return M
