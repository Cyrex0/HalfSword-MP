-- DEV-ONLY, no hooks installed. Requires an actual native function-entry and
-- function-exit bridge. A Blueprint Lua post hook alone CANNOT supply this
-- guarantee. Preserve the original Collision Hit input across nested DCD.
local M={}
local function same(a,b)
    for _,k in ipairs({"match_id","round","life","weapon_actor","source_component","victim_component","victim_bone"})do
        if a[k]==nil or a[k]~=b[k] then return false end
    end
    for i=1,3 do if type(a.point[i])~="number" or type(b.point[i])~="number" or math.abs(a.point[i]-b.point[i])>0.01 then return false end end
    return true
end
function M.new()
    local stack,nextid={},0
    local out={}
    local function copy(input)
        assert(type(input.point)=="table" and #input.point==3,"original contact point required")
        local row={point={input.point[1],input.point[2],input.point[3]}}
        for k,v in pairs(input)do if k~="point" then assert(type(v)~="table" and type(v)~="userdata","plain original contact identity required");row[k]=v end end
        return row
    end
    function out.begin_collision(input)
        assert(type(input.my_bone)=="string" and input.my_bone~="" and input.my_bone~="None","original native MyBoneName required")
        assert(#stack<16,"native collision recursion capacity")
        local row=copy(input);nextid=nextid+1;row.token=nextid;stack[#stack+1]=row
        return nextid
    end
    function out.record(input)
        local entry=stack[#stack]
        if not entry or not same(entry,input) then return nil,"no exact native entry scope" end
        local row=copy(input);row.source_bone=entry.my_bone;row.token=entry.token;return row
    end
    function out.end_collision(token)
        assert(stack[#stack] and stack[#stack].token==token,"native collision exit order mismatch")
        table.remove(stack)
    end
    return out
end
return M
