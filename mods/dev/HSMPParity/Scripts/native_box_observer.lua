-- Bounded read-only control. Native entry/post pairs are never restoration or damage authority.
local M={}
local objects={"world","pawn","mesh","box","box_owner"}
local fields={"match_id","round","life","world_key","owner_life","owner_pawn","owner_mesh","peer","side"}
local function positive(v)return type(v)=="number" and v>0 and v<math.huge and v%1==0 end
local function hook_id(v)return type(v)=="number" and v%1==0 and v>=-2147483648 and v<=2147483647 end
local function copy(s)
    if type(s)~="table" then return nil end
    local c={}
    for _,k in ipairs(fields)do
        local v=s[k]
        if type(v)~="number" and type(v)~="string" then return nil end
        c[k]=v
    end
    if not positive(c.match_id) or not positive(c.round) or not positive(c.life) or not positive(c.owner_life)
        or not positive(c.owner_pawn) or not positive(c.owner_mesh) or not positive(c.peer)
        or c.world_key=="" or (c.side~="r" and c.side~="l") then return nil end
    for _,k in ipairs(objects)do
        local o=s[k]
        if type(o)~="table" or not positive(o.address) or type(o.path)~="string" or o.path:sub(1,1)~="/"
            or #o.path>2048 or o.path:find("%z") then return nil end
        c[k]={address=o.address,path=o.path}
    end
    return c -- Plain scalar copies; no native wrapper retained.
end
local function same(a,b)
    if not a or not b then return false end
    for _,k in ipairs(fields)do if a[k]~=b[k] then return false end end
    for _,k in ipairs(objects)do if a[k].address~=b[k].address or a[k].path~=b[k].path then return false end end
    return true
end
function M.new(api)
    local active,hooks=nil,{}
    local function native()
        local n=api.native()
        return type(n)=="table" and type(n.begin)=="function" and type(n.mark)=="function"
            and type(n.stop)=="function" and type(n.read)=="function" and type(n.status)=="function" and n or nil
    end
    local function drain(n)
        local ok,rows=pcall(n.read)
        if not ok or type(rows)~="table" then api.log("BOXOBS unavailable readback");return end
        if #rows>32 then api.log("BOXOBS refused oversize native readback");return end
        for _,row in ipairs(rows)do
            -- The native budget is per activation, including nested functions.
            if active and active.drained>=32 then return end
            if active then active.drained=active.drained+1 end
            api.emit(row)
        end
    end
    local function stop(reason)
        local n=native()
        if n then pcall(n.stop);drain(n) end
        active=nil
        api.log("BOXOBS stopped reason=%s authority=false",reason or "developer")
    end
    local function fresh()
        if not active then return nil,"inactive" end
        local ok,s,reason=pcall(api.snapshot,active.scope.peer,active.scope.side)
        if not ok then return nil,"snapshot_exception" end
        if not s then return nil,reason or "snapshot_unavailable" end
        s=copy(s)
        if not s then return nil,"snapshot_fields" end
        if not same(active.scope,s) then return nil,"scope_changed" end
        return s
    end
    local function mark(role,ctx,box_param)
        if not active then return end
        local n=native();if not n then return end
        local admitted,status=pcall(n.status)
        if not admitted or type(status)~="table" or status.active~=true
            or type(status.pending)~="number" or status.pending<1 or status.pending_role~=role then return end
        local s,reason=fresh()
        if not s then stop(reason);return end
        local ok,p,b=pcall(function()return ctx:get(),box_param:get()end)
        if not ok or not p or not b or not p:IsValid() or not b:IsValid()
            or p:GetAddress()~=s.pawn.address or b:GetAddress()~=s.box.address then return end
        -- This marker is pending-only. A Lua POST after native POST remains unpaired.
        pcall(n.mark,{world=s.world.address,pawn=s.pawn.address,mesh=s.mesh.address,box=s.box.address,
            box_owner=s.box_owner.address,match_id=s.match_id,round=s.round,life=s.life,role=role,marker=0})
    end
    local function install()
        local paths={"/Game/Character/Blueprints/Willie_BP.Willie_BP_C:Deal Complex Damage",
            "/Game/Character/Blueprints/Willie_BP.Willie_BP_C:Get Damage"}
        for role,path in ipairs(paths)do
            if hooks[role]=="uncertain" then return false end
            if not hooks[role] then
                -- A thrown/ambiguous submission might already have registered. Never retry it.
                hooks[role]="uncertain"
                local ok,first,second=pcall(api.register,path,function(ctx,...)
                    if not active then return end
                    -- Both local SDK signatures put Hit Box at formal input15.
                    -- Pinned UE4SS Blueprint POST is the SECOND callback; no third callback.
                    pcall(mark,role,ctx,select(15,...))
                end)
                if not ok or not hook_id(first) or not hook_id(second) or first~=second then return false end
                hooks[role]=true
            end
        end
        return true
    end
    local out={}
    function out.command(arg)
        if api.developer()~=true then api.log("BOXOBS refused developer mode required");return end
        if arg=="off" then stop();return end
        local peer,side,seconds=tostring(arg):match("^(%d+)%s+([rl])%s+(%d+)$")
        peer,seconds=tonumber(peer),tonumber(seconds)
        if not positive(peer) or not positive(seconds) or seconds>15 then
            api.log("BOXOBS refused use <peer> <r|l> <seconds1..15> or off");return
        end
        if active then stop("re-enrollment") end
        local n=native();if not n then api.log("BOXOBS unavailable native API");return end
        local ok,s,reason=pcall(api.snapshot,peer,side)
        if not ok then reason="snapshot_exception"
        elseif s then s=copy(s);if not s then reason="snapshot_fields" end
        else reason=reason or "snapshot_unavailable" end
        if not ok or not s then api.log("BOXOBS refused reason=%s authority=false",reason);return end
        s.duration_ms,s.calls=seconds*1000,32
        local started,yes,why=pcall(n.begin,s)
        if not started or yes~=true then api.log("BOXOBS unavailable enrollment=%s",tostring(why or yes));return end
        active={scope=copy(s),drained=0}
        if not install() then stop("Lua POST hook unavailable");return end
        api.log("BOXOBS started peer=%s side=%s seconds=%s calls=32 qualified=unavailable_until_pair authority=false",
            tostring(peer),side,tostring(seconds))
    end
    function out.tick()
        if not active then return end
        local n=native();if not n then active=nil;return end
        local current,reason=fresh()
        if not current then stop(reason);return end
        drain(n)
        local ok,s=pcall(n.status)
        if not ok or type(s)~="table" or s.active~=true then
            api.log("BOXOBS finished reason=%s entries=%s unpaired=%s discarded=%s authority=false",
                tostring(type(s)=="table" and s.reason or "status unavailable"),
                tostring(type(s)=="table" and s.entries or "unavailable"),
                tostring(type(s)=="table" and s.unmatched or "unavailable"),
                tostring(type(s)=="table" and s.discarded or "unavailable"))
            active=nil
        end
    end
    function out.drop()if active then stop("world leave")end end
    return out
end
return M
