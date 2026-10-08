-- Preparation keeps scalar identities only. Every setter resolves the current
-- controller/pawn and exact mesh again, including after a reflected getter.
local M = {}
local fields = { "Mesh", "SK_Skeleton", "BoneCore", "DriverSkeleton" }
local function binding_same(a, b)
    if not a or not b then return false end
    for _, name in ipairs({ "index", "world_key", "pc_address", "pc_name", "pawn_address", "pawn_name" }) do
        if a[name] == nil or a[name] ~= b[name] then return false end
    end
    return true
end
local function mesh_same(a, b, owner)
    return a and b and a.address == b.address and a.name == b.name and a.owner == owner and b.owner == owner
end
M.binding_same = binding_same
function M.new(env)
    local self = { done = {} }
    function self:drop() self.done = {} end
    function self:prepare(index)
        local expected = env.resolve(index)
        if not expected then return false end
        local id = expected.world_key .. ":" .. tostring(expected.pc_address) .. ":" .. expected.pc_name
            .. ":" .. tostring(expected.pawn_address) .. ":" .. expected.pawn_name
        if self.done[id] then return true end
        local function current()
            local fresh = env.resolve(index)
            return binding_same(expected, fresh) and fresh or nil
        end
        for _, operation in ipairs({ "disable_input", "reset_move", "reset_look" }) do
            local binding = current()
            if not binding or env[operation](binding) ~= true or not current() then return false end
        end
        local seen = {}
        for _, field in ipairs(fields) do
            local binding = current()
            if not binding then return false end
            local expected_mesh = env.mesh(binding, field)
            if not current() then return false end
            if expected_mesh and not seen[expected_mesh.address] then
                seen[expected_mesh.address] = true
                for _, operation in ipairs({ "always_tick", "no_rate_skip", "tick_enabled" }) do
                    binding = current()
                    if not binding then return false end
                    local mesh = env.mesh(binding, field)
                    binding = current()
                    if not binding or not mesh_same(expected_mesh, mesh, binding.pawn_address) then return false end
                    -- Only fresh objects returned by the last qualification
                    -- reach the setter; an earlier wrapper is never reused.
                    if env[operation](binding, mesh) ~= true or not current() then return false end
                    binding = current()
                    if not binding then return false end
                    local after = env.mesh(binding, field)
                    if not current() or not mesh_same(expected_mesh, after, expected.pawn_address) then return false end
                end
            end
        end
        if not current() then return false end
        self.done[id] = true
        return true
    end
    return self
end
return M
