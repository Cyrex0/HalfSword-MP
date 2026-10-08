-- Held input for the native worker. The directory owns identity; only fresh
-- input for its exact human entity may reach the currently possessed pawn.
local M = { TIMEOUT_MS = 250, BUTTONS = 127 }
M.AXES = {
    "InpAxisEvt_Move Forward / Backward_K2Node_InputAxisEvent_14",
    "InpAxisEvt_Move Right / Left_K2Node_InputAxisEvent_19",
    "InpAxisEvt_Turn Right / Left Mouse_K2Node_InputAxisEvent_16",
    "InpAxisEvt_Look Up / Down Mouse_K2Node_InputAxisEvent_17",
    "InpAxisEvt_Right Guard Axis_K2Node_InputAxisEvent_6",
    "InpAxisEvt_Left Guard Axis_K2Node_InputAxisEvent_7",
    "InpAxisEvt_Right Arm Axis_K2Node_InputAxisEvent_2",
    "InpAxisEvt_Left Arm Axis_K2Node_InputAxisEvent_3",
}
local function integer(v, lo, hi) return type(v) == "number" and v == math.floor(v) and v >= lo and v <= hi end
local function epoch_valid(value) return type(value) == "number" and math.type(value) == "integer" and value ~= 0 end
local function key(row) return tostring(row.epoch) .. ":" .. tostring(row.id) .. ":" .. tostring(row.incarnation) end
local function ref_valid(row)
    return type(row) == "table" and epoch_valid(row.epoch)
        and integer(row.id, 1, 0xffffffff) and integer(row.incarnation, 1, 0xffffffff)
end
local function axes_valid(axes)
    if type(axes) ~= "table" or #axes ~= 8 then return false end
    for i, value in ipairs(axes) do
        local limit = (i == 3 or i == 4) and 100 or 1
        if type(value) ~= "number" or value ~= value or value < -limit or value > limit then return false end
        if (i == 5 or i == 6) and value < 0 then return false end
    end
    return true
end
local function newer(a, b)
    if not integer(a, 1, 0xffffffff) then return false end
    if b == nil then return true end
    local delta = (a - b) & 0xffffffff
    return delta > 0 and delta < 0x80000000
end
local function zero() return {0, 0, 0, 0, 0, 0, 0, 0} end
local binding_fields = { "index", "world_key", "pc_address", "pc_name", "pawn_address", "pawn_name" }
function M.fresh_binding(row, env)
    if env.prepare(row.controller) ~= true then return nil end
    -- Preparation may enter native callbacks and change possession. Resolve
    -- afterwards and retain only the newly qualified scalar identity.
    local fresh = env.resolve(row.controller)
    if not fresh then return nil end
    local binding = {}
    for _, field in ipairs(binding_fields) do
        if fresh[field] == nil then return nil end
        binding[field] = fresh[field]
    end
    binding.key = binding.world_key .. ":" .. tostring(binding.pc_address) .. ":" .. binding.pc_name
        .. ":" .. tostring(binding.pawn_address) .. ":" .. binding.pawn_name
    binding.entity_id, binding.incarnation, binding.controller_index = row.id, row.incarnation, row.controller
    return binding
end
function M.binding_matches(binding, env)
    local fresh = env.resolve(binding.index)
    if not fresh then return false end
    for _, field in ipairs(binding_fields) do if binding[field] == nil or binding[field] ~= fresh[field] then return false end end
    return true
end
function M.new(env)
    local self = { directory = nil, entities = {}, states = {}, stopped = false }
    -- World drop intentionally performs no engine call. An old pawn may already
    -- be freed; only a newly resolved current binding is eligible for a release.
    function self:drop() self.directory, self.entities, self.states = nil, {}, {} end
    function self:stop() self.stopped = true; self:drop() end
    function self:set_directory(directory)
        if self.stopped then return false end
        if type(directory) ~= "table" or not epoch_valid(directory.epoch)
            or not integer(directory.seq, 1, 0xffffffff) or type(directory.entities) ~= "table"
            or #directory.entities > 32 then self:drop(); return false end
        local entities, controllers = {}, {}
        for _, row in ipairs(directory.entities) do
            if not ref_valid(row) or row.epoch ~= directory.epoch or entities[key(row)] then self:drop(); return false end
            if row.kind == 0 or row.kind == "human" then
                if not integer(row.controller, 0, 1) or controllers[row.controller] then self:drop(); return false end
                controllers[row.controller] = true
            end
            entities[key(row)] = row
        end
        for id, row in pairs(entities) do
            if row.kind == 0 or row.kind == "human" then
                local old = self.entities[id]
                local state = self.states[id]
                if not state then
                    -- Same native controller, new network incarnation: retain
                    -- only the last applied button mask and scalar binding key
                    -- so the current pawn receives its release before fresh input.
                    for old_id, previous in pairs(self.entities) do
                        if (previous.kind == 0 or previous.kind == "human") and previous.controller == row.controller then
                            local applied = self.states[old_id]
                            if applied then state = { buttons = applied.buttons, binding = applied.binding } end
                            break
                        end
                    end
                    state = state or { buttons = 0 }
                end
                if old and old.owner_peer ~= row.owner_peer then
                    state.at, state.axes, state.want_buttons, state.mouse_pending = nil, nil, 0, false
                end
                self.states[id] = state
            end
        end
        self.directory, self.entities = directory, entities
        return true
    end
    function self:receive(frame)
        if self.stopped or not ref_valid(frame) or not self.directory or frame.epoch ~= self.directory.epoch then return false end
        local entity = self.entities[key(frame)]
        if not entity or (entity.kind ~= 0 and entity.kind ~= "human") or (entity.owner_peer or 0) <= 0 then return false end
        if not integer(frame.buttons, 0, M.BUTTONS) or not axes_valid(frame.axes)
            or not integer(frame.flags or 0, 0, 1) then return false end
        local state = self.states[key(frame)] or { buttons = 0 }
        if not newer(frame.delivery_seq, state.delivery_seq) then return false end
        state.delivery_seq, state.at = frame.delivery_seq, env.now_ms()
        state.axes = {}; for i = 1, 8 do state.axes[i] = frame.axes[i] end
        state.want_buttons = frame.buttons
        state.mouse_pending = true
        if frame.flags == 1 then state.axes, state.want_buttons, state.mouse_pending = zero(), 0, false end
        self.states[key(frame)] = state
        return true
    end
    function self:tick()
        if self.stopped or not self.directory or not env.running() then return false end
        local now = env.now_ms()
        for id, state in pairs(self.states) do
            local row = self.entities[id]
            if row and (row.kind == 0 or row.kind == "human") then
                local binding = env.binding(row)
                if not binding then
                    -- Do not touch a pawn retained from a previous possession.
                    state.binding, state.buttons, state.axes, state.want_buttons = nil, 0, nil, 0
                else
                    if state.binding ~= binding.key then
                        state.binding, state.buttons = binding.key, 0
                        state.axes, state.want_buttons, state.mouse_pending = nil, 0, false
                        state.at = nil -- a new binding needs fresh input after identification
                    end
                    local expired = not state.at or now - state.at >= M.TIMEOUT_MS or now < state.at
                        or (row.owner_peer or 0) <= 0
                    local axes = expired and zero() or state.axes or zero()
                    if expired then state.want_buttons, state.mouse_pending = 0, false end
                    if not state.mouse_pending then axes[3], axes[4] = 0, 0 end
                    local wanted = state.want_buttons or 0
                    if env.same(binding) then
                        local ok = env.invoke(binding, axes, state.buttons ~ wanted, wanted)
                        if ok and env.same(binding) then
                            state.buttons, state.mouse_pending = wanted, false
                        else state.at, state.axes, state.want_buttons = nil, nil, 0 end
                    end
                end
            else self.states[id] = nil end
        end
        return true
    end
    return self
end
return M
