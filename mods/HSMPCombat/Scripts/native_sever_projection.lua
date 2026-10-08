-- Conservative projection of the native completed-cut ledger onto protocol
-- 12's whole-bone mask. Partial geometry and detached parts need a separate
-- topology record; Spawn Bone is deliberately not a whole-subtree mapping.
-- Primary source: Willie Dismember Function Delayed, cooked offsets 2298
-- (Hide Bone Local) and 24563 (Parts Map Add true), pinned game evidence.
local M={}
local roots={[3]="lowerarm_r",[4]="hand_r",[6]="lowerarm_l",[7]="hand_l",
    [9]="calf_r",[10]="foot_r",[12]="calf_l",[13]="foot_l"}
local fields={"world","peer","match_id","round","life","pawn","actor","mesh"}
local function copy_context(c)
    if type(c)~="table" then return nil end
    local out={}
    for _,k in ipairs(fields)do
        local v=c[k]
        if k=="world" or k=="pawn" then
            if type(v)~="string" or #v==0 or v:find("\0",1,true) then return nil end
        elseif type(v)~="number" or not math.tointeger(v) or v<=0 then return nil end
        out[k]=v
    end
    if out.peer>65535 or out.life>65535 or out.round>0xffffffff then return nil end
    if c.mesh_name~=nil then
        if type(c.mesh_name)~="string" or #c.mesh_name==0 then return nil end
        out.mesh_name=c.mesh_name
    end
    return out
end
local function same(a,b)
    if not a or not b then return false end
    for _,k in ipairs(fields)do if a[k]~=b[k] then return false end end
    return true
end
local function names(set)
    local out={}
    for n in pairs(set)do out[#out+1]=n end
    table.sort(out)
    return out
end
function M.new()
    local scope,confirmed,known=nil,{},false
    local api={}
    function api.reset()scope,confirmed,known=nil,{},false end
    function api.observe(expected,topology,env)
        local ctx=copy_context(expected)
        if not ctx then return names(confirmed),known,"context unavailable" end
        if not same(scope,ctx) or scope.mesh_name~=ctx.mesh_name then scope,confirmed,known=ctx,{},false end
        if type(topology)~="table" or not same(ctx,copy_context(topology.context)) then
            return names(confirmed),known,"snapshot unavailable or scope changed"
        end
        local parts=topology.parts
        local process=topology.flags and topology.flags["Dismemberment In Process"]
        if type(parts)~="table" or parts.available~=true or not process
            or process.available~=true or type(process.value)~="boolean" then
            return names(confirmed),known,"native ledger or completion unavailable"
        end
        if process.value then return names(confirmed),known,"native cut in process" end
        if type(env)~="table" or type(env.guard)~="function" or type(env.hidden)~="function" then
            return names(confirmed),known,"fresh physical evidence unavailable"
        end
        local function guard()
            local ok,v=pcall(env.guard,ctx)
            return ok and v==true
        end
        if not guard() then return names(confirmed),known,"completion scope unavailable" end
        local staged,seen,count={},{},0
        if type(parts.values)~="table" then return names(confirmed),known,"typed ledger unavailable" end
        for _,row in ipairs(parts.values)do
            count=count+1
            local part=type(row)=="table" and row.part
            if count>15 or type(part)~="number" or not math.tointeger(part)
                or part<0 or part>14 or seen[part] or type(row.value)~="boolean" then
                return names(confirmed),known,"typed ledger incomplete"
            end
            seen[part]=true
            if row.value and roots[part] and not confirmed[roots[part]] then
                if not guard() then return names(confirmed),known,"completion scope changed" end
                local ok,hidden=pcall(env.hidden,roots[part],ctx)
                if ok and hidden==true then staged[roots[part]]=true end
            end
        end
        if parts.count~=count then return names(confirmed),known,"ledger count mismatch" end
        if not guard() then return names(confirmed),known,"completion scope changed" end
        -- Native completion appends true entries. Within an exact body life,
        -- a transient empty/unavailable read cannot grow back a confirmed limb.
        for n in pairs(staged)do confirmed[n]=true end
        known=true
        return names(confirmed),known,"completed native distal projection"
    end
    return api
end
return M
