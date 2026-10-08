local clock, hooks, installed = 100, {}, 0
local axis = "InpAxisEvt_Right Arm Axis_K2Node_InputAxisEvent_2"
local path = "/Game/Character/Blueprints/Willie_BP.Willie_BP_C:" .. axis
local function pawn(name)
    local p = { IsValid = function() return true end }
    p.GetFName = function() return { ToString = function() return name end } end
    p[axis] = function(self, value)
        self.axis = value
        if hooks[path] then hooks[path]({ get = function() return self end }) end
    end
    return p
end
local me, other = pawn("owner"), pawn("other")
local pc = { Pawn = me, IsValid = function() return true end }
local world = { key = "one", pc = function() return pc end, on_drop=function()end }
package.preload.UEHelpers = function() return {} end
package.preload.hsmp_wg = function() return { new = function() return world end } end
package.preload.hsmp_ipc = function() return { init = function() end } end
package.preload.hsmp_session = function() return { new = function() return {} end } end
HSMP_PARITY_TEST = {}
os.clock = function() return clock end
local stateDir = T.tmpdir("parity_input_")
os.getenv = function(key)
    if key == "HSMP_STATE_DIR" then return stateDir end
    if key == "HSMP_RUNTIME_ROLE" then return "client" end
    return nil
end
package.loaded.hsmp_runtime_role = nil
LoopAsync = function() end
RegisterHook = function(name, callback) hooks[name] = callback; installed = installed + 1 end
dofile(T.path("mods/dev/HSMPParity/Scripts/main.lua"))
local api = HSMP_PARITY_TEST
api.arm("r 0.5")
me[axis](me, 0)
T.check(me.axis == 1 and api.state().frames == 1,
    "actual native zero-input callback is replaced by held arm input without recursion")
other[axis](other, 0)
T.check(other.axis == 0 and api.state().frames == 1, "test input never affects another actor")
clock = clock + 0.6
me[axis](me, 0)
T.check(me.axis == 0 and api.state() == nil, "timeout returns to native input")
api.arm("r 0.5")
local before = installed
api.arm("r 0.5")
T.check(installed == before, "repeated tests reuse the installed input hook")
world.key = "two"
me[axis](me, 0)
T.check(me.axis == 0 and api.state() == nil, "world change cancels input before using old state")
