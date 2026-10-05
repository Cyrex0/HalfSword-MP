-- Reversible Chaos subtree exclusion. The caller owns the current-world mesh
-- and must restore this state before releasing a pooled stand-in.
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
            pawn=shown.pawn,body_key=body_key,mask=0}
    end
    local mask=type(vitals)=="table"and math.tointeger(vitals.dism)
    if mask and mask>=0 and vitals.match_id==shown.match_id and vitals.round==shown.round
        and vitals.life==shown.life then journal.mask=mask end
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
function I.apply(mesh, state, wanted, fname)
    local same = true
    for b in pairs(state or {}) do if not wanted[b] then same = false end end
    for b in pairs(wanted) do if not (state or {})[b] then same = false end end
    if same then return state or {} end
    -- Restore all old roots first: enabling an ancestor after disabling a
    -- child would otherwise silently reactivate that child's collision.
    local left = I.restore(mesh, state, fname)
    if next(left) then return left, "restore failed" end
    for b in pairs(wanted) do
        local ok = pcall(function() mesh:SetAllBodiesBelowPhysicsDisabled(fname(b), true, true) end)
        if ok then left[b] = true end
    end
    return left
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
