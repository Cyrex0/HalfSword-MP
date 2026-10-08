-- Dedicated native rendering adapter. No Willie, AI controller or velocity servo is created.
local M={}
function M.new(env)
    local self={key=nil}
    function self:drop() self.key=nil end
    function self:clear()
        local world,token=env.world()
        if world and env.same(token) then env.native.native_clear_mirrors(world:GetAddress()) end
        self:drop()
    end
    function self:apply()
        local world,token=env.world()
        if not world or not env.same(token)then return nil,"client world unavailable"end
        local assets,scope=env.native.native_scene_assets()
        if not assets then return nil,scope end
        if self.key~=scope then
            for _,path in ipairs(assets)do
                if not env.same(token)then return nil,"world changed during source asset load"end
                local asset=StaticFindObject(path)
                if not asset or not asset:IsValid()then asset=LoadAsset(path)end -- unsafe: ok game-thread rare exact cooked object path; no SoftObject property read
                if not env.same(token)or not asset or not asset:IsValid()then return nil,"source render asset unavailable: "..path end
                local name=asset:GetFullName():match("^%S+%s+(.+)$")
                if name~=path then return nil,"source render asset identity mismatch"end
            end
            self.key=scope
        end
        local ok,reason=env.native.native_present(world:GetAddress())
        if not env.same(token)then self:drop();return nil,"world changed during native mirror batch"end
        return ok,reason
    end
    return self
end
return M
