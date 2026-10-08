-- Read-only native topology evidence, independent of scalar Vitals. The caller
-- supplies a game-thread/current-world context guard; no native objects are
-- retained or returned. Completed reads describe availability, never 'healthy'
-- or a conversion from the native 15-part enum to the network bone mask.
local M={VERSION=1,ARRAY_LIMIT=64,PART_LIMIT=15}
M.PART_ENUM="/Game/Blueprints/Enumerator/Enum_DismembermentPart.Enum_DismembermentPart"
M.FLAGS={"Headless","Hand R Torn Off","Hand L Torn Off","Leg R Torn Off","Leg L Torn Off",
    "Upper Body Spawned","Dismemberment In Process"}
local fields={"world","peer","match_id","round","life","pawn","actor","mesh"}
local function safe(f)
    local ok,v=pcall(f)
    if ok then return v end
end
local function integer(v,max)
    local n=type(v)=="number" and math.tointeger(v)
    return n and n>=0 and (not max or n<=max) and n or nil
end
local function string_value(v,limit)
    return type(v)=="string" and #v>0 and #v<=limit and not v:find("\0",1,true) and v or nil
end
local function context(c)
    if type(c)~="table" then return nil end
    local t={world=string_value(c.world,1024),pawn=string_value(c.pawn,256)}
    for _,k in ipairs({"peer","match_id","round","life","actor","mesh"})do
        local max=k=="peer" and 65535 or k=="round" and 0xffffffff or k=="life" and 65535 or nil
        local n=integer(c[k],max)
        if not n or n==0 then return nil end
        t[k]=n
    end
    if not t.world or not t.pawn then return nil end
    return t -- copy only plain, validated identity fields
end
local function equal(a,b)
    if not a or not b then return false end
    for _,k in ipairs(fields)do if a[k]~=b[k] then return false end end
    return true
end
local function unknown(reason,count)
    return {available=false,reason=reason,count=count}
end
local function name(v)
    if type(v)~="string" then v=v:ToString() end
    return string_value(v,128)
end

-- env.context must check world validity before touching w; env.unwrap adapts
-- the pinned UE4SS parameter wrappers. Optional env.map_count may use a proven
-- native count API; absent it, ForEach's completed bounded iteration is the map
-- count contract. The result states which contract was available explicitly.
function M.read(w,env)
    if type(env)~="table" or type(env.context)~="function" then return nil,"context unavailable" end
    local before=context(safe(function()return env.context(w)end))
    if not before then return nil,"context unavailable" end
    local invalid
    local function guard()
        if invalid then return false end
        local now=context(safe(function()return env.context(w)end))
        if not equal(before,now) then invalid="scope changed";return false end
        return true
    end
    local function identity()
        if not guard() then return false end
        local ok=safe(function()
            if w:IsValid()~=true or w:GetAddress()~=before.actor or w:GetFName():ToString()~=before.pawn then return false end
            local mesh=w.Mesh
            return mesh and mesh:IsValid()==true and mesh:GetAddress()==before.mesh
        end)
        if ok~=true then invalid="native identity unavailable or changed";return false end
        return true
    end
    if not identity() then return nil,invalid end
    local function unwrap(v)
        if env.unwrap then return env.unwrap(v) end
        return v
    end
    local function field(k)
        if not guard() then return nil end
        return safe(function()return w[k]end)
    end
    local function array(a)
        if a==nil then return unknown("unavailable") end
        if not guard() then return unknown(invalid) end
        local n=integer(safe(function()return a:GetArrayNum()end))
        if n==nil then return unknown("count unavailable") end
        if n>M.ARRAY_LIMIT then return unknown("limit exceeded",n) end
        local values,count={},0
        local ok=pcall(function()a:ForEach(function(_,v)
            if not guard() then error(invalid) end
            count=count+1
            if count>M.ARRAY_LIMIT then error("limit exceeded") end
            local value=name(unwrap(v))
            if not value then error("name unavailable") end
            values[count]=value
        end)end)
        if not ok then return unknown("iteration incomplete",n) end
        if not guard() then return unknown(invalid,n) end
        local after=integer(safe(function()return a:GetArrayNum()end))
        if after~=n or count~=n then return unknown("count changed or mismatch",n) end
        return {available=true,count=n,values=values}
    end
    local function parts(a)
        if a==nil then return unknown("unavailable") end
        if not guard() then return unknown(invalid) end
        local count_before
        if env.map_count then
            count_before=integer(safe(function()return env.map_count(a)end))
            if count_before==nil then return unknown("count unavailable") end
            if count_before>M.PART_LIMIT then return unknown("limit exceeded",count_before) end
        end
        local values,seen,count,present,true_values={},{},0,0,0
        local ok=pcall(function()a:ForEach(function(k,v)
            if not guard() then error(invalid) end
            count=count+1
            if count>M.PART_LIMIT then error("limit exceeded") end
            k,v=integer(unwrap(k),14),unwrap(v)
            if k==nil or type(v)~="boolean" or seen[k] then error("typed map entry unavailable") end
            seen[k]=true;values[count]={part=k,value=v}
            present=present|(1<<k)
            if v then true_values=true_values|(1<<k) end
        end)end)
        if not ok then return unknown("iteration incomplete",count_before) end
        if not guard() then return unknown(invalid,count_before) end
        if env.map_count then
            local count_after=integer(safe(function()return env.map_count(a)end))
            if count_after~=count_before or count~=count_before then return unknown("count changed or mismatch",count_before) end
        end
        table.sort(values,function(a,b)return a.part<b.part end)
        return {available=true,count=count,values=values,present_mask=present,true_mask=true_values,
            count_check=env.map_count and "native" or "iteration"}
    end
    local r={version=M.VERSION,part_enum=M.PART_ENUM,context=before,flags={},read_complete=true}
    r.dism_array=array(field("Dismembered Array"))
    r.dism_bones=array(field("Dismembered Bones"))
    r.parts=parts(field("Dismembered Parts Map"))
    for _,a in ipairs({r.dism_array,r.dism_bones,r.parts})do
        if not a.available then r.read_complete=false end
    end
    for _,k in ipairs(M.FLAGS)do
        local v=safe(function()return unwrap(field(k))end)
        if type(v)=="boolean" then r.flags[k]={available=true,value=v}
        else r.flags[k]=unknown("unavailable");r.read_complete=false end
    end
    if not identity() then return nil,invalid end
    return r
end
return M
