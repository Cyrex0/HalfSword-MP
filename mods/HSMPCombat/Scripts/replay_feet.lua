-- Native kick pseudo-weapons exist only while the local Kick Timeline runs.
-- Replay sources reproduce its native construction, without assigning gameplay
-- Foot Weapon fields or adding a live physics constraint/contact source.
local M = {}
function M.new(e)
    local cache, errors = {}, 0
    local path = "/Game/Assets/Weapons/Blueprints/Built_Weapons/Weapon_Feet.Weapon_Feet_C"
    local function valid(a) return a and a:IsValid() end
    local function find(name)
        for _,a in ipairs(FindAllOf("Weapon_Feet_C") or {}) do
            if valid(a) and a:GetFName():ToString() == name then return a end
        end
    end
    local function same(a,b) return valid(a) and valid(b) and a:GetAddress()==b:GetAddress() end
    local function stop(a)
        a:SetActorEnableCollision(false)
        a:SetActorHiddenInGame(true)
        a["Temp Disable Damage"] = true
        a.BaseMesh:SetSimulatePhysics(false)
        a.BaseMesh:SetCollisionEnabled(0)
        local n=0
        a["Collision Components Array"]:ForEach(function(_,v)
            n=n+1
            if n>15 then error("native foot array exceeds supported identity") end
            local c=v:get()
            if valid(c) then
                c:SetCollisionEnabled(0)
                if c:GetCollisionEnabled()~=0 then error("foot collision guard failed") end
            end
        end)
        if a:GetActorEnableCollision() or a["Temp Disable Damage"]~=true then error("foot source guard failed") end
    end
    local out = {}
    function out.component(pawn,left,ordinal)
        if ordinal==0 then return nil end -- modern exact source only
        local ok,result=pcall(function()
            local id=pawn:GetAddress()
            local key=tostring(id)..(left and "L" or "R")
            local row=cache[key]
            local a=row and find(row.name)
            if row and (row.retired or row.failed or not valid(a) or a:GetAddress()~=row.address) then error("retained foot source unavailable") end
            local t=pawn.Mesh:GetSocketTransform(FName(left and "foot_l" or "foot_r"),0)
            if not valid(a) then
                local count=0; for _ in pairs(cache) do count=count+1 end
                if count>=64 then error("foot source cache full") end
                local cls=StaticFindObject(path)
                if not valid(cls) then error("native Weapon_Feet class unavailable") end
                local gs=e.UEHelpers.GetGameplayStatics()
                a=gs:BeginDeferredActorSpawnFromClass(pawn,cls,t,1,pawn,2)
                if not valid(a) then error("native foot spawn failed") end
                row={name=a:GetFName():ToString(),pawn=id,address=a:GetAddress(),failed=true}
                cache[key]=row
                -- Original native Kick spawn sets both before construction.
                a["Parent Actor"],a["Last Parent"]=pawn,pawn
                local finished,err=pcall(function()
                    gs:FinishSpawningActor(a,t,2)
                    a["Kick Power"],a["Is Held"]=10,true
                    stop(a)
                end)
                if not finished then a:K2_DestroyActor(); error(err) end -- unsafe: ok spawned exclusively from exact Weapon_Feet_C class above, never Willie
                row.failed=false
            end
            if not same(a:GetOwner(),pawn) or not same(a["Parent Actor"],pawn) or not same(a["Last Parent"],pawn) then
                error("native foot source owner mismatch")
            end
            stop(a)
            if a:K2_SetActorTransform(t,false,{},true)==false then error("native foot transform failed") end
            local c,n=nil,0
            a["Collision Components Array"]:ForEach(function(_,v)
                n=n+1; if n==ordinal then c=v:get() end
            end)
            -- Only the actual native Foot Box is a verified delayed kick source.
            local cc=valid(c) and c:GetClass() or nil
            if not valid(c) or not same(c,a.Box) or not valid(cc) or cc:GetFName():ToString()~="BoxComponent" then
                error("native foot Box ordinal mismatch")
            end
            return c
        end)
        if not ok then
            errors=errors+1
            if errors<=3 or errors%100==0 then e.log("native foot replay source unavailable #%d: %s",errors,tostring(result)) end
            return nil
        end
        return result
    end
    function out.prune()
        if next(cache)==nil then return end
        local current={}
        -- Refresh native wrappers: cached puppets may already have been freed.
        for _,p in ipairs(FindAllOf("Willie_BP_C") or {}) do
            if valid(p) then current[p:GetAddress()]=true end
        end
        for k,r in pairs(cache) do
            if not current[r.pawn] then
                local a=find(r.name)
                if valid(a) and a:GetAddress()==r.address then
                    if not r.retired then a:K2_DestroyActor();r.retired=true end -- unsafe: ok exact fresh native foot identity, never Willie
                else cache[k]=nil end
            end
        end
    end
    -- Travel destroys level actors; never dereference wrappers of the old world.
    function out.clear(why)
        if why and why~="world changed" and why~="no valid world" then cache={} end
    end
    return out
end
return M
