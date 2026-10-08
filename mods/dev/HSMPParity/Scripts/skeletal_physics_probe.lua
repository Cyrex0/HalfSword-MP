-- Read-only, caller supplies freshly verified original weapon/component scope.
local M={}
function M.run(component,identity,e,stage)
    if not identity or not identity.verified or not identity.match_id or not identity.life
        or identity.life<1 or not e.current(identity,component) then return nil,'source_context' end
    local rows={};for n=1,4 do
        local bone=e.fname('Joint'..n)
        local row,why=e.inspect_body(component,bone)
        if not row or not row.exists then return nil,why or ('named_body_unproved:Joint'..n) end
        rows[#rows+1]=row
    end
    if not e.current(identity,component) then return nil,'source_changed' end
    local primitives,why=stage.read_component(component,identity,e)
    if not primitives then return nil,why end
    -- Measurements do not prove AggGeom scale semantics. Native contact/support
    -- measurements at multiple orientations/scales are required separately.
    return {bodies=rows,primitives=primitives,source=identity,
        native_frame_verified=false,scale_semantics_verified=false,read_only=true}
end
return M
