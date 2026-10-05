-- Exact-world native actor enumeration. Return verified scope even when GC
-- has unloaded a formerly observed class, so prior absence can be checked.
local M={}
function M.query(o,ref)
    local result={}
    local ok,err=pcall(function()
        local world=o.world()
        if not o.valid(world) then error("native_world_unavailable") end
        result.world_address=world:GetAddress()
        result.world_name=world:GetFullName()
        if result.world_address~=ref.world_address or result.world_name~=ref.world_name then error("native_world_changed") end
        local cls=o.find(ref.class_path)
        if not o.valid(cls) then error("original_native_class_unavailable") end
        result.class_address=cls:GetAddress()
        if result.class_address~=ref.class_address then error("original_native_class_replaced") end
        local gs=o.gameplay_statics()
        if not o.valid(gs) then error("gameplay_statics_unavailable") end
        local native={}
        gs:GetAllActorsOfClass(world,cls,native)
        local actors={}
        for _,entry in pairs(native)do
            local actor=entry:get()
            if not o.valid(actor) then error("invalid_live_actor_entry") end
            actors[#actors+1]=actor
        end
        result.actors=actors
    end)
    if not ok then result.error=tostring(err) end
    return result
end
return M
