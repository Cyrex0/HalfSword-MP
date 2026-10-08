-- One synchronous, bounded native membership read. Results retain scalar
-- identities and existing plain origin records only, never native wrappers.
local M={LIMIT=128}
local function integer(v,positive)
    return type(v)=="number" and math.tointeger(v) and v>=(positive and 1 or 0) and v or nil
end
local function finite(v)return type(v)=="number" and v==v and math.abs(v)<math.huge and v or nil end
local function text(v)return type(v)=="string" and v~="" and not v:find("\0",1,true) and v or nil end
local function call(guard,f)
    if guard and guard()~=true then error("scope changed",0)end
    local ok,v=pcall(f);if not ok then error("native read failed",0)end;return v
end
function M.reference(o,guard)
    local ok,r=pcall(function()
        if not o or call(guard,function()return o:IsValid()end)~=true then return nil end
        local a=integer(call(guard,function()return o:GetAddress()end),true)
        local n=text(call(guard,function()return o:GetFullName()end))
        if a and n then return {address=a,name=n}end
    end)
    return ok and r or nil
end
function M.same(a,b)return a and b and a.address==b.address and a.name==b.name or false end
function M.key(id,c)return tostring(id.address).."@"..id.name.."@"..c.world..":"..c.drops end
local context_fields={"world","drops","attacker_peer","victim_peer","match_id","round","attacker_life","victim_life",
    "victim_name","source_address","target_address","source","source_class","bone","ordinal"}
function M.same_context(a,b)
    if not a or not b or not M.same(a.native_world,b.native_world)then return false end
    for _,k in ipairs(context_fields)do if a[k]~=b[k]then return false end end
    return true
end
local function snapshot_context(c)
    local out={native_world={address=c.native_world.address,name=c.native_world.name}}
    for _,k in ipairs(context_fields)do out[k]=c[k]end
    return out
end
local function binding_context(original,current)
    if not original or not current or not M.same(original.native_world,current.native_world)then return false end
    for _,k in ipairs(context_fields)do if k~="bone" and original[k]~=current[k]then return false end end
    -- Native inside-2 rebinds this same constraint lowerarm_l -> hand_l
    -- (UE4SS.log:1694,1709; server combat.rs). No reverse/adjacent aliases.
    return original.bone==current.bone or original.bone=="lowerarm_l" and current.bone=="hand_l"
end
-- UE4SS's GetWorld helper is an AActor API. Components use the reflected
-- ActorComponent.GetOwner, and that actor's world; constraint Owner is irrelevant.
local function inputs(q,guard)
    local out,owners={},{}
    local function ref(o)
        local id=M.reference(o,guard);if not id then error("identity unavailable",0)end;return id
    end
    local function world(actor)return ref(call(guard,function()return actor:GetWorld()end))end
    for _,k in ipairs({"weapon","victim","collider","body"})do
        out[k]=ref(q[k])
        local actor=q[k]
        if k=="collider" or k=="body" then
            actor=call(guard,function()return actor:GetOwner()end);owners[k]=ref(actor)
            if k=="collider" and not M.same(owners[k],out.weapon)then error("collider owner changed",0)end
            if k=="body" and not M.same(owners[k],out.victim)then error("body owner mismatch",0)end
        end
        if not M.same(world(actor),q.world)then error("world mismatch",0)end
    end
    return out,owners
end
function M.current(q,r,guard)
    local ok,value=pcall(function()
        if not M.same_context(q.context,r.context)then return false end
        local now,owners=inputs(q,guard)
        for _,k in ipairs({"weapon","collider","victim","body"})do if not M.same(now[k],r.inputs[k])then return false end end
        for _,k in ipairs({"collider","body"})do if not M.same(owners[k],r.owners[k])then return false end end
        return call(guard,function()return true end)
    end)
    return ok and value==true
end
local header_fields={"world","drops","attacker_peer","victim_peer","match_id","round","attacker_life","victim_life",
    "victim_name","source_address","target_address","source","source_class","bone","at","ats","vts","vats","flags","gate"}
function M.header(p,c)
    if type(p)~="table" or not text(c.world) or not integer(c.drops)
        or not integer(c.attacker_peer,true) or not integer(c.victim_peer,true) then return nil end
    local h={world=p.world,drops=p.drops,attacker_peer=p.attacker_peer,victim_peer=p.peer,victim_name=p.nm}
    for _,k in ipairs({"match_id","round","attacker_life","victim_life","source_address","target_address","source"})do
        h[k]=integer(p[k],true);if not h[k] or h[k]~=c[k] then return nil end
    end
    if h.world~=c.world or h.drops~=c.drops or h.attacker_peer~=c.attacker_peer
        or h.victim_peer~=c.victim_peer or h.victim_name~=c.victim_name then return nil end
    h.source_class=text(p.source_class);h.bone=text(p.bone)
    if not h.source_class or h.source_class~=c.source_class or not h.bone
        or not (h.bone==c.bone or h.bone=="pelvis" and c.bone=="spine_02") then return nil end
    for _,k in ipairs({"at","ats","vts","vats"})do h[k]=finite(p[k]);if h[k]==nil or h[k]<0 then return nil end end
    h.flags=integer(p.flags);h.gate=p.gate
    if not h.flags or h.gate~=nil and type(h.gate)~="boolean" then return nil end
    if p.cid~=nil and not integer(p.cid,true)then return nil end
    h.cid=p.cid;return h
end
function M.equal_parent(a,pa,b,pb)
    for _,k in ipairs(header_fields)do if a[k]~=b[k] then return false end end
    if a.cid~=b.cid then return false end
    return a.cid~=nil or rawequal(pa,pb) -- two nil cids are not one origin
end
function M.parent_current(h,p,c)
    if type(h)~="table" then return false end
    local now=M.header(p,c);if not now then return false end
    for _,k in ipairs(header_fields)do if h[k]~=now[k]then return false end end
    return h.cid==nil or h.cid==now.cid -- the original flush may assign cid
end
-- BeginPlay remains a candidate association, never native caller/marker
-- authority. All eligible recent origins must agree; do not take the last.
function M.bind(pending,id,c,now)
    local out={constraint={address=id.address,name=id.name},context=snapshot_context(c),state="missing"}
    if #pending>M.LIMIT then out.state="overflow";return out end
    local chosen,h
    for _,p in ipairs(pending)do
        if p.source_address==c.source_address and p.target_address==c.target_address and p.nm==c.victim_name
            and p.source==c.source and (p.bone==c.bone or p.bone=="pelvis" and c.bone=="spine_02")
            and finite(p.at) and now-p.at>=0 and now-p.at<=0.05 then
            local ph=M.header(p,c)
            if not ph then out.state="unavailable";return out end
            if chosen and not M.equal_parent(h,chosen,ph,p) then out.state="ambiguous";return out end
            chosen,h=p,ph
        end
    end
    if chosen then out.state="unique";out.parent=chosen;out.header=h end
    return out
end
function M.resolve(q,env)
    local r={state="unavailable",complete=false,matches=0,context=q.context,inputs={},authority="unavailable"}
    local function read(f)return call(env.guard,f)end
    local function ref(o)
        local id=M.reference(o,env.guard);if not id then error("identity unavailable",0)end;return id
    end
    local function world(o)return ref(read(function()return o:GetWorld()end))end
    local function membership(c)
        for _,pair in ipairs({{"My Weapon","weapon"},{"Weapon Hit Module","collider"},
            {"Hit Actor","victim"},{"Component 2 (Body)","body"}})do
            local target=ref(read(function()return c[pair[1]]end))
            if not M.same(target,r.inputs[pair[2]])then return false end
        end
        local bone=read(function()return c["Bone Name 2"]:ToString()end)
        if not text(bone)then error("bone unavailable",0)end
        return bone==r.context.bone
    end
    local ok,why=pcall(function()
        local expected_world=q.world
        if not expected_world then error("world unavailable",0)end
        r.context=snapshot_context(q.context)
        r.inputs,r.owners=inputs(q,env.guard)
        if r.inputs.collider.address~=q.context.source_address or r.inputs.body.address~=q.context.target_address then
            error("input context mismatch",0)
        end
        local a=read(function()return q.weapon["Stuck Constraints Array"]end)
        local n=integer(read(function()return a:GetArrayNum()end))
        if n==nil then error("count unavailable",0)end
        r.count=n
        if n>M.LIMIT then r.state="overflow";error("array limit exceeded",0)end
        local function storage(arr)
            local at=integer(read(function()return arr:GetArrayAddress()end),true)
            local data=integer(read(function()return arr:GetArrayDataAddress()end))
            if not at or data==nil or n>0 and data==0 then error("storage unavailable",0)end
            return at,data
        end
        local at,data=storage(a)
        local count,seen,selected,header,bad,abort,first_constraint,parent_context=0,{},nil,nil,nil,nil,nil,nil
        local iter_ok=pcall(function()a:ForEach(function(i,entry)
            local entry_ok,err=pcall(function()
                count=count+1
                if count>M.LIMIT or count>n then r.state="overflow";error("iteration limit exceeded",0)end
                if i~=count then error("array order mismatch",0)end
                local c=read(function()return env.unwrap(entry)end)
                if not c then return end
                local live=read(function()return c:IsValid()end)
                if live==false then return end
                if live~=true then error("validity unavailable",0)end
                local id=ref(c)
                if not M.same(world(c),expected_world) then error("constraint world mismatch",0)end
                if not membership(c)then return end
                local key=tostring(id.address).."@"..id.name
                local binding=env.parent(id,q.context)
                if not M.same(id,ref(c)) or not membership(c)then error("constraint membership changed",0)end
                if seen[key] then return end
                seen[key]=true;r.matches=r.matches+1
                if r.matches==1 then
                    r.constraint=id;first_constraint=c
                else r.constraint=nil;first_constraint=nil end
                local h=binding and binding.state=="unique" and M.same(binding.constraint,id)
                    and binding_context(binding.context,q.context)
                    and M.header(binding.parent,binding.context)
                if not h or not M.parent_current(binding.header,binding.parent,binding.context)then bad=bad or "parent unavailable or stale"
                elseif selected and not M.equal_parent(header,selected,h,binding.parent)then bad="distinct parents"
                else selected,header,parent_context=binding.parent,h,binding.context end
            end)
            if not entry_ok then abort=err;return true end -- pinned native early-stop contract
            return false
        end)end)
        if not iter_ok then error("iteration failed",0)end
        if abort then error(abort,0)end
        if count~=n or integer(read(function()return a:GetArrayNum()end))~=n then error("count changed",0)end
        local fresh=read(function()return q.weapon["Stuck Constraints Array"]end)
        local after,after_data=storage(fresh)
        if after~=at or after_data~=data or integer(read(function()return fresh:GetArrayNum()end))~=n then error("array identity changed",0)end
        if not M.current(q,r,env.guard)then error("input identity changed",0)end
        if first_constraint then
            if env.evidence then r.evidence=read(function()return env.evidence(first_constraint)end)end
            if not M.same(r.constraint,ref(first_constraint)) or not M.same(world(first_constraint),expected_world)
                or not membership(first_constraint)then
                error("constraint membership changed",0)
            end
            if not M.current(q,r,env.guard)then error("input identity changed",0)end
        end
        read(function()return true end)
        r.complete=true;r.authority=r.matches==1 and "unique_membership" or "unavailable:multiple_or_no_memberships"
        if r.matches==0 then r.state="none"
        elseif bad then r.state=bad=="distinct parents" and "ambiguous" or "unavailable";r.reason=bad
        else r.state="unique";r.parent=selected;r.header=header;r.parent_context=snapshot_context(parent_context)end
    end)
    if not ok then
        r.state=r.state=="overflow" and "overflow" or "incomplete";r.reason=tostring(why)
        r.constraint=nil;r.evidence=nil;r.parent=nil;r.header=nil;r.parent_context=nil
    end
    return r
end
return M
