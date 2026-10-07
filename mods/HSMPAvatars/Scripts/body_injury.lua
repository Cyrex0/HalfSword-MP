-- Reversible Chaos subtree exclusion. Driving may stop without ending the
-- body's lease: only the caller's explicit, current-world lease handoff restores.
local I = {}
local function same_context(a,b,key)
    return a and b and a.body_key==key and a.match_id==b.match_id and a.round==b.round
        and a.life==b.life and a.pawn==b.pawn
end
-- A missing/rejected packet is not an intact-body update. The last verified
-- mask belongs to one exact displayed generation and one owned native body.
function I.select_mask(journal,shown,allowed,vitals,body_key)
    if journal and journal.body_key~=body_key then journal=nil end
    if not allowed then return journal,journal and journal.mask or nil end
    if not same_context(journal,shown,body_key) then
        journal={match_id=shown.match_id,round=shown.round,life=shown.life,
            pawn=shown.pawn,body_key=body_key}
    end
    local mask=type(vitals)=="table"and math.tointeger(vitals.dism)
    local seq=type(vitals)=="table"and math.tointeger(vitals.seq)
    if mask and mask>=0 and mask<1<<23 and seq and seq>=0 and seq<=0xffffffff
        and vitals.match_id==shown.match_id and vitals.round==shown.round
        and vitals.life==shown.life then
        local delta=journal.seq and ((seq-journal.seq)&0xffffffff)
        local advance=journal.seq==nil or (delta>0 and delta<0x80000000)
        if advance then
            journal.seq=seq
            -- A severed part cannot regrow within one native life. A source
            -- array read failure/older zero must not reactivate its collider.
            journal.mask=(journal.mask or 0)|mask
        end
    end
    return journal,journal.mask
end
function I.restore(mesh, state, fname)
    local left = {}
    for bone in pairs(state or {}) do
        local ok = pcall(function() mesh:SetAllBodiesBelowPhysicsDisabled(fname(bone), false, true) end)
        if not ok then left[bone] = true end
    end
    return left
end
function I.apply(mesh, state, wanted, fname, reassert)
    local same,removed = true,false
    for b in pairs(state or {}) do if not wanted[b] then same,removed = false,true end end
    for b in pairs(wanted) do if not (state or {})[b] then same = false end end
    if same and not reassert then return state or {} end
    -- Added owner-confirmed severing never enables an already missing root.
    -- If a diagnostic root is removed, restore all old roots before applying
    -- the remainder: enabling an ancestor last would reactivate its child.
    local left = not removed and (state or {}) or I.restore(mesh, state, fname)
    if removed and next(left) then return left, "restore failed" end
    local why
    for b in pairs(wanted) do
        local ok = pcall(function() mesh:SetAllBodiesBelowPhysicsDisabled(fname(b), true, true) end)
        if ok then left[b] = true else why="disable failed "..b end
    end
    return left,why
end
-- This is native simulation evidence, not a per-body collision getter. UE's
-- reflected GetCollisionEnabled has no bone argument; a contact/trace must
-- separately establish absence of the excluded body's collision in game.
function I.simulation(mesh, roots, slots, parents, fname)
    local out={}
    local selected={}
    for i=1,#slots do selected[i]=true end
    I.omit(selected,roots,slots,parents)
    for i=1,#slots do
        if selected[i]==nil then
            local ok,value=pcall(function()return mesh:IsSimulatingPhysics(fname(slots[i]))end)
            out[slots[i]]=ok and type(value)=="boolean" and tostring(value) or "unavailable"
        end
    end
    return out
end
function I.omit(targets, roots, slots, parents)
    if not targets or not next(roots or {}) then return end
    for i = 1, #parents do
        local p = i
        while p and p > 0 do
            if roots[slots[p]] then targets[i] = nil; break end
            local parent = parents[p]
            if parent == p then break end
            p = parent
        end
    end
end
return I
