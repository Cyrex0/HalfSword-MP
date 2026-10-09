-- Static source recipes are captured outside the bone sampler and remain tied
-- to their original native binding, entity reference and directory generation.
local M = {}
local fields = { "index", "world_key", "pc_address", "pc_name", "pawn_address", "pawn_name" }
local row_fields = { "epoch", "id", "incarnation", "slot", "kind", "controller", "owner_peer" }
local function integer(value, lo, hi)
    return type(value)=="number" and math.type(value)=="integer" and value>=lo and value<=hi
end
local function dense(rows)
    if type(rows)~="table" then return nil end
    local count=0
    for k in pairs(rows) do
        if not integer(k,1,32) then return nil end
        count=count+1
    end
    for i=1,count do if type(rawget(rows,i))~="table" then return nil end end
    return count
end
local function copy_directory(directory)
    if type(directory)~="table" or not integer(directory.epoch,math.mininteger,math.maxinteger)
        or not integer(directory.seq,1,0xffffffff) then return nil end
    local count=dense(directory.entities)
    if not count then return nil end
    local result={epoch=directory.epoch,seq=directory.seq,entities={}}
    for _,field in ipairs({"state","arena","error"})do result[field]=directory[field] end
    local refs,slots={},{}
    for i=1,count do
        local row=directory.entities[i]
        if row.epoch~=directory.epoch or not integer(row.id,1,0xffffffff) or not integer(row.incarnation,1,0xffffffff)
            or not integer(row.slot,0,31) or not integer(row.kind,0,1) or not integer(row.controller,0,255)
            or type(row.team_known)~="boolean" or (row.team_known and not integer(row.team,-0x80000000,0x7fffffff)) then return nil end
        local ref=tostring(row.id)..":"..tostring(row.incarnation)
        if refs[ref] or slots[row.slot] then return nil end
        refs[ref],slots[row.slot]=true,true
        local copied={team_known=row.team_known,team=row.team}
        for _,field in ipairs(row_fields)do copied[field]=row[field] end
        result.entities[i]=copied
    end
    return result
end
local function same_roster(a,b,teams)
    if not a or not b or a.epoch~=b.epoch or #a.entities~=#b.entities then return false end
    for i,row in ipairs(a.entities)do
        local other=b.entities[i]
        for _,field in ipairs(row_fields)do if row[field]~=other[field] then return false end end
        if teams and (row.team_known~=other.team_known or row.team~=other.team)then return false end
    end
    return true
end
local function same_binding(a, b)
    if not a or not b then return false end
    for _, field in ipairs(fields) do if a[field] == nil or a[field] ~= b[field] then return false end end
    return true
end
function M.new(env)
    assert(type(env.directory)=="function" and type(env.roster_facts)=="function" and type(env.addresses)=="function",
        "native source roster preflight APIs unavailable")
    local cached, revisions, retry_at = {}, {}, {}
    local roster
    local api = {}
    local function key(row) return tostring(row.epoch) .. ":" .. tostring(row.id) .. ":" .. tostring(row.incarnation) end
    local function valid(token, original, index)
        if not env.same(token) then return false end
        local fresh = env.resolve(index)
        return env.same(token) and same_binding(original, fresh)
    end
    local function addresses(binding)
        local a=env.addresses(binding)
        if type(a)~="table" or not integer(a.world,1,math.maxinteger) or not integer(a.pawn,1,math.maxinteger)
            or not integer(a.controller,1,math.maxinteger) or a.pawn~=binding.pawn_address or a.controller~=binding.pc_address then return nil end
        return {world=a.world,pawn=a.pawn,controller=a.controller}
    end
    local function bound(token,record)
        if not env.same(token) then return false end
        local fresh=env.resolve(record.binding.index)
        if not same_binding(record.binding,fresh) then return false end
        local a=addresses(fresh)
        return env.same(token) and a and a.world==record.addresses.world and a.pawn==record.addresses.pawn
            and a.controller==record.addresses.controller
    end
    local function bindings_valid(token,records)
        for _,record in ipairs(records)do if not bound(token,record) then return false end end
        return env.same(token)
    end
    local function current_directory()
        return copy_directory(env.directory())
    end
    local function ack_valid(token)
        if not roster or not roster.ack then return false end
        local before=current_directory()
        if not before or before.seq~=roster.ack.seq or not same_roster(before,roster.ack,true)
            or not bindings_valid(token,roster.bindings) then return false end
        local after=current_directory()
        return env.same(token) and after and after.seq==roster.ack.seq and same_roster(after,roster.ack,true)
    end
    local function preflight(directory,token)
        if not env.same(token)then env.invalidate();roster=nil;return false,"world changed before source roster" end
        local base=copy_directory(directory)
        if not base then return false,"source roster directory malformed" end
        if roster and roster.ack and base.seq==roster.ack.seq and same_roster(base,roster.ack,true) then
            if ack_valid(token)then return true,roster.ack end
            roster=nil;return nil,"source roster binding changed; preflight restart"
        end
        if roster and (not same_roster(base,roster.base,false)
            or (base.seq~=roster.base.seq and base.seq~=roster.facts.ack_dir_seq))then roster=nil end
        if not roster then
            local records,wire={},{}
            for i,row in ipairs(base.entities)do
                local index=env.index(row)
                local binding=env.resolve(index)
                if not binding or not env.same(token)then return nil,"source roster binding unavailable" end
                local old=cached[key(row)]
                if old and not same_binding(old.binding,binding)then return false,"canonical incarnation changed"end
                local a=addresses(binding)
                if not a or not env.same(token)then return nil,"source roster addresses unavailable" end
                local identity={}
                for _,field in ipairs(fields)do identity[field]=binding[field] end
                records[i]={binding=identity,addresses=a}
                wire[i]={epoch=row.epoch,id=row.id,incarnation=row.incarnation,slot=row.slot,kind=row.kind,
                    controller_index=row.controller,world=a.world,pawn=a.pawn,controller=a.controller}
            end
            local before=current_directory()
            if not before or before.seq~=base.seq or not same_roster(before,base,true) or not bindings_valid(token,records)then
                return nil,"source roster changed before preflight"
            end
            local context={epoch=base.epoch,dir_seq=base.seq}
            if env.phase then env.phase(context,"roster_facts","enter",{count=#wire})end
            local accepted,facts=env.roster_facts({epoch=base.epoch,dir_seq=base.seq},wire)
            if env.phase then env.phase(context,"roster_facts","exit",{ok=accepted==true,count=#wire,reason=accepted~=true and tostring(facts) or ""})end
            if not env.same(token)then env.invalidate();return false,"world changed during source roster preflight"end
            if not bindings_valid(token,records)then return nil,"source roster binding changed during preflight"end
            if accepted~=true then
                local after=current_directory()
                if not after or after.seq~=base.seq or not same_roster(after,base,true)then
                    return nil,"source roster changed during preflight; restart"
                end
                return false,"source roster preflight: "..tostring(facts)
            end
            if type(facts)~="table" or facts.epoch~=base.epoch or facts.base_dir_seq~=base.seq
                or not integer(facts.ack_dir_seq,base.seq,0xffffffff) or dense(facts.entities)~=#base.entities then
                return false,"source roster facts malformed"
            end
            local copied={epoch=facts.epoch,base_dir_seq=facts.base_dir_seq,ack_dir_seq=facts.ack_dir_seq,entities={}}
            local changed=false
            for i,row in ipairs(base.entities)do
                local fact=facts.entities[i]
                for _,field in ipairs({"epoch","id","incarnation","slot","kind","controller"})do
                    if fact[field]~=row[field]then return false,"source roster fact identity changed"end
                end
                if not integer(fact.team,-0x80000000,0x7fffffff)then return false,"source roster team malformed"end
                copied.entities[i]={team=fact.team}
                changed=changed or not row.team_known or row.team~=fact.team
            end
            if facts.ack_dir_seq~=base.seq+(changed and 1 or 0)then return false,"source roster ACK sequence malformed"end
            roster={base=base,bindings=records,facts=copied}
        end
        local now=current_directory()
        if not env.same(token)then env.invalidate();roster=nil;return false,"world changed while awaiting source roster"end
        if not now or not same_roster(now,roster.base,false) or (now.seq~=roster.base.seq and now.seq~=roster.facts.ack_dir_seq)then
            roster=nil;return nil,"source roster changed; preflight restart"
        end
        if not bindings_valid(token,roster.bindings)then roster=nil;return nil,"source roster binding changed; preflight restart"end
        if now.seq~=roster.facts.ack_dir_seq then return nil,"source roster ACK pending"end
        for i,row in ipairs(now.entities)do
            if row.team_known~=true or row.team~=roster.facts.entities[i].team then
                if now.seq==roster.base.seq and roster.facts.ack_dir_seq~=roster.base.seq then return nil,"source roster ACK pending"end
                roster=nil;return nil,"source roster ACK facts changed; preflight restart"
            end
        end
        roster.ack=now
        if not ack_valid(token)then roster=nil;return nil,"source roster changed at ACK; preflight restart"end
        return true,roster.ack
    end
    function api.ensure(directory, token, frame_seq)
        local prepared,ack=preflight(directory,token)
        if prepared~=true then return prepared,ack end
        directory=ack
        for _, row in ipairs(directory.entities or {}) do
            if not env.same(token) then env.invalidate(); return false, "world changed before source descriptor" end
            local reference, index = key(row), env.index(row)
            local binding = env.resolve(index)
            if not binding or not env.same(token) then return false, "source descriptor binding unavailable" end
            local old = cached[reference]
            if old and not same_binding(old.binding, binding) then return false, "canonical incarnation changed" end
            if not old or old.dir_seq ~= directory.seq then
                if env.now_ms() < (retry_at[reference] or -math.huge) then return false, "source descriptor retry pending" end
                retry_at[reference] = env.now_ms() + 1000
                local revision = (revisions[reference] or 0) + 1
                local meta = {epoch=row.epoch,id=row.id,incarnation=row.incarnation,
                    slot=row.slot,dir_seq=directory.seq,revision=revision,frame_seq=frame_seq}
                if env.phase then env.phase(meta,"source_capture","enter") end
                local recipe, bindings = env.capture(index,meta)
                if env.phase then env.phase(meta,"source_capture","exit",{ok=recipe~=nil,reason=not recipe and tostring(bindings) or ""}) end
                if not env.same(token) then env.invalidate(); return false, "world changed during source descriptor" end
                if not valid(token, binding, index) then return false, "canonical incarnation changed" end
                if not recipe then return false, "source descriptor " .. reference .. ": " .. tostring(bindings) end
                if not ack_valid(token)then roster=nil;return nil,"source roster changed during source descriptor"end
                if recipe.team~=row.team then roster=nil;return nil,"source team changed during source descriptor"end
                -- Rebuild authority metadata from the directory; the adapter's
                -- diagnostic context cannot mutate native descriptor identity.
                meta = {epoch=row.epoch,id=row.id,incarnation=row.incarnation,
                    slot=row.slot,dir_seq=directory.seq,revision=revision,frame_seq=frame_seq}
                if env.phase then env.phase(meta,"native_bind","enter") end
                local accepted, reason = env.describe(meta,recipe,bindings)
                if env.phase then env.phase(meta,"native_bind","exit",{ok=accepted==true,reason=tostring(reason or "")}) end
                if not env.same(token) then env.invalidate(); return false, "world changed while registering source descriptor" end
                if not valid(token, binding, index) then return false, "canonical incarnation changed" end
                if not ack_valid(token)then roster=nil;return nil,"source roster changed during descriptor registration"end
                if accepted ~= true then return false, "source descriptor registration: " .. tostring(reason) end
                local identity = {}
                for _, field in ipairs(fields) do identity[field] = binding[field] end
                cached[reference], revisions[reference], retry_at[reference] = {binding=identity,dir_seq=directory.seq}, revision, nil
            end
        end
        if not ack_valid(token)then roster=nil;return nil,"source roster changed before canonical sample"end
        return true,directory
    end
    function api.refresh()
        -- Keep the original scalar binding and revision. A replacement actor
        -- cannot turn a recipe refresh into reuse of the old entity reference.
        for reference, value in pairs(cached) do value.dir_seq = nil; retry_at[reference] = env.now_ms() + 1000 end
    end
    function api.drop() cached, revisions, retry_at, roster = {}, {}, {}, nil end
    return api
end
return M
