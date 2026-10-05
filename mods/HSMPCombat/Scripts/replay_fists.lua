-- Collision-disabled native fist source for an authenticated historical hit.
-- Never assigns Weapon L/R, adds a hand constraint or replaces held equipment.
local M={}
local path='/Game/Assets/Weapons/Blueprints/Built_Weapons/Weapon_Fists.Weapon_Fists_C'
local function valid(x)return x and x:IsValid()end
local function same(a,b)return valid(a) and valid(b) and a:GetAddress()==b:GetAddress()end
local function context(a,b)return a and b and a.match_id==b.match_id and a.round==b.round and a.life==b.life end
local function generation(c)return type(c)=='table' and type(c.match_id)=='number' and c.match_id>0
    and type(c.round)=='number' and c.round>0 and type(c.life)=='number' and c.life>0 end
function M.new(e)
    local cache,count,world,native_world={},0,nil,nil
    local out={}
    local function find(r)
        for _,a in ipairs((e.find_all or FindAllOf)('Weapon_Fists_C') or {})do
            if valid(a) and a:GetAddress()==r.address and (not r.name or a:GetFName():ToString()==r.name)
                and (not r.full_name or a:GetFullName()==r.full_name) then return a end
        end
    end
    local function stop(a)
        a:SetActorEnableCollision(false);a:SetActorHiddenInGame(true);a['Temp Disable Damage']=true
        a.BaseMesh:SetSimulatePhysics(false)
        local n=0;a['Collision Components Array']:ForEach(function(_,v)
            n=n+1;if n>15 then error('fist array exceeds identity capacity')end
            local c=v:get();if valid(c)then c:SetCollisionEnabled(0);if c:GetCollisionEnabled()~=0 then error('fist collision guard')end end
        end)
        if a:GetActorEnableCollision() or a['Temp Disable Damage']~=true then error('fist actor guard')end
    end
    function out.component(pawn,left,ordinal,original)
        local allocated
        local ok,c=pcall(function()
            if ordinal~=10 then error('fist native Sphere ordinal must be10')end
            if not valid(pawn) or not original or type(original.life)~='number' or original.life<1
                or not original.match_id or not original.round or not e.context
                or not context(original,e.context(pawn)) then error('fist original context mismatch')end
            local wk=e.world_key();if not wk then error('fist world unavailable')end
            local pw=pawn:GetWorld();if not valid(pw)then error('fist native world unavailable')end
            local nw={name=pw:GetFullName(),address=pw:GetAddress()}
            -- Session/PC/WG key churn does not destroy actors. Only a fresh
            -- actual native World identity change can release pool capacity.
            if native_world and (nw.name~=native_world.name or nw.address~=native_world.address)then cache={};count=0 end
            native_world=nw
            world=wk
            local key=tostring(pawn:GetAddress())..(left and 'L' or 'R')..':'..original.match_id..':'..original.round..':'..original.life
            local r=cache[key];local a=r and find(r)
            if r and (r.failed or r.retired or not valid(a)) then error('retained fist identity unavailable')end
            local t=pawn.Mesh:GetSocketTransform(FName(left and 'weapon_lSocket' or 'weapon_rSocket'),0)
            -- Native normal hand setup explicitly discards socket scale before
            -- deferred spawn/Finish (Left2905/Right equivalent); no render bounds.
            t.Scale3D.X,t.Scale3D.Y,t.Scale3D.Z=1,1,1
            if not valid(a)then
                if count>=64 then error('fist source pool full')end
                local cls=(e.find_class or StaticFindObject)(path);if not valid(cls)then error('native fist class unavailable')end
                local gs=e.UEHelpers.GetGameplayStatics()
                a=gs:BeginDeferredActorSpawnFromClass(pawn,cls,t,1,pawn,2)
                if not valid(a)then error('native fist spawn failed')end
                count=count+1 -- count partial spawn BEFORE name/Finish/setup can fail.
                r={failed=true,pawn=pawn:GetAddress(),pawn_name=pawn:GetFullName(),address=a:GetAddress(),
                    original={match_id=original.match_id,round=original.round,life=original.life},peer_id=original.peer_id,
                    native_world={name=nw.name,address=nw.address}};cache[key]=r
                allocated=r
                -- Guard BEFORE Finish/BeginPlay and before optional name reads.
                a:SetActorEnableCollision(false);a:SetActorHiddenInGame(true);a['Temp Disable Damage']=true
                r.name=a:GetFName():ToString()
                r.full_name=a:GetFullName();r.class_name='Weapon_Fists_C'
                a['Parent Actor'],a['Last Parent']=pawn,pawn
                gs:FinishSpawningActor(a,t,2)
                a['Is Held']=true;stop(a);r.failed=false
            end
            if not same(a:GetOwner(),pawn) or not same(a['Parent Actor'],pawn) or not same(a['Last Parent'],pawn)then error('fist owner mismatch')end
            if not same(a:GetWorld(),pawn:GetWorld())then error('fist native world mismatch')end
            stop(a)
            if a:K2_SetActorTransform(t,false,{},true)==false then error('fist transform failed')end
            local c,n=nil,0;a['Collision Components Array']:ForEach(function(_,v)n=n+1;if n==10 then c=v:get()end end)
            if not same(c,a.Sphere) or c:GetClass():GetFName():ToString()~='SphereComponent' then error('fist Sphere identity mismatch')end
            if not context(original,e.context(pawn)) or e.world_key()~=world then error('fist context changed during construction')end
            return c
        end)
        if not ok then
            if allocated then
                allocated.failed=true
                -- Fresh exact allocated native identity only; retain tombstone
                -- and capacity. Never destroy or alter a caller/user actor.
                pcall(function()
                    local a=find(allocated)
                    if valid(a) and valid(pawn) and same(a:GetWorld(),pawn:GetWorld())
                        and a:GetClass():GetFName():ToString()=='Weapon_Fists_C' then stop(a) end
                end)
            end
            if e.log then e.log('native fist replay source unavailable: %s',tostring(c))end;return nil
        end
        return c
    end
    function out.prune()
        if e.world_key()~=world then return end
        local good,pawns=pcall(e.find_all or FindAllOf,'Willie_BP_C')
        local scanned,actors=pcall(e.find_all or FindAllOf,'Weapon_Fists_C')
        if not good or type(pawns)~='table' or not scanned or type(actors)~='table' then return end
        local current={};for _,p in ipairs(pawns)do
            if valid(p)then current[p:GetAddress()]=p end
        end
        for key,r in pairs(cache)do
            pcall(function()
                local a,replacement
                for _,candidate in ipairs(actors)do
                    if valid(candidate) and candidate:GetAddress()==r.address then
                        if (not r.name or candidate:GetFName():ToString()==r.name)
                            and (not r.full_name or candidate:GetFullName()==r.full_name) then a=candidate else replacement=true end
                    end
                end
                -- Complete successful enumeration proves the original identity
                -- absent. A different name at same address is never touched.
                if not a then cache[key]=nil;count=count-1;return end
                -- MirroredGarbage is a native retirement witness; inspect this
                -- flag before GetWorld (retired actors can return NULL world).
                local flag_ok,garbage=pcall(function()return a:HasAnyFlags(0x40000000)end)
                if flag_ok and garbage==true then cache[key]=nil;count=count-1;return end
                local p=current[r.pawn]
                local original_present=valid(p) and p:GetFullName()==r.pawn_name
                local original_now
                if r.peer_id and e.current_generation then original_now=e.current_generation(r.peer_id) end
                if not generation(original_now) and original_present then original_now=e.context(p) end
                local retire=generation(original_now) and not context(r.original,original_now)
                -- A known replacement in the SAME generation also ends this
                -- source's original pawn lifetime. Require both a complete old
                -- pawn absence and independent current-pawn/context evidence.
                if not retire and not original_present and generation(original_now)
                    and context(r.original,original_now) and r.peer_id and e.current_actor then
                    local replacement_pawn,replacement_context=e.current_actor(r.peer_id)
                    if valid(replacement_pawn) and generation(replacement_context)
                        and context(original_now,replacement_context)
                        and replacement_pawn:GetAddress()~=r.pawn
                        and same(current[replacement_pawn:GetAddress()],replacement_pawn) then
                        local rw=replacement_pawn:GetWorld()
                        retire=valid(rw) and rw:GetAddress()==r.native_world.address and rw:GetFullName()==r.native_world.name
                    end
                end
                -- Unknown context/session loss or a missing pawn alone never
                -- authorizes destruction. Changed generation is positive proof
                -- even after the original pawn's native UObject disappeared.
                if not retire then return end
                local aw=a:GetWorld()
                if not valid(aw) or aw:GetAddress()~=r.native_world.address
                    or aw:GetFullName()~=r.native_world.name
                    or a:GetClass():GetFName():ToString()~=r.class_name then return end
                -- Surviving lineage must still identify the original pawn;
                -- null references are expected after its native destruction.
                local owners={a:GetOwner(),a['Parent Actor'],a['Last Parent']}
                for i=1,3 do
                    local owner=owners[i]
                    if valid(owner) and (owner:GetAddress()~=r.pawn or owner:GetFullName()~=r.pawn_name) then return end
                end
                stop(a)
                if not r.retired then a:K2_DestroyActor();r.retired=true end
                -- Destroy request isn't proof. Retain count until a later fresh
                -- complete enumeration/garbage flag confirms native retirement.
            end)
        end
    end
    function out.clear(why)
        -- Defer retirement until component() sees a different valid native
        -- World. Even an explicit travel notification can precede teardown.
        -- No old UObject is touched here; pool identities/count remain intact.
    end
    return out
end
return M
