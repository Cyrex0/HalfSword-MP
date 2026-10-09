-- Dedicated native rendering adapter. No Willie, AI controller or velocity servo is created.
local M={}
function M.new(env)
    local self={key=nil,loading=nil}
    function self:drop() self.key=nil;self.loading=nil end
    local function progress(stage,done,total)
        if env.progress then env.progress(stage,done,total)end
    end
    local function matches(paths,assets)
        if #paths~=#assets then return false end
        for i,path in ipairs(paths)do if path~=assets[i]then return false end end
        return true
    end
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
        if not env.same(token)then self:drop();return nil,"world changed during source asset load"end
        if self.key~=scope then
            local cursor=self.loading
            if not cursor or cursor.scope~=scope or not env.same(cursor.token)or not matches(cursor.paths,assets)then
                local paths={};for i,path in ipairs(assets)do paths[i]=path end
                cursor={scope=scope,token=token,paths=paths,done=0};self.loading=cursor
            end
            progress("assets",cursor.done,#cursor.paths)
            if cursor.done<#cursor.paths then
                -- One exact cooked object per tick; no UObject survives a yield.
                if not env.same(token)then self:drop();return nil,"world changed during source asset load"end
                local path=cursor.paths[cursor.done+1]
                local asset=StaticFindObject(path)
                if not env.same(token)then self:drop();return nil,"world changed during source asset load"end
                if not asset or not asset:IsValid()then
                    if not env.same(token)then self:drop();return nil,"world changed during source asset load"end
                    asset=LoadAsset(path) -- unsafe: ok game-thread rare exact cooked object path; no SoftObject property read
                end
                if not env.same(token)or not asset or not asset:IsValid()then return nil,"source render asset unavailable: "..path end
                if not env.same(token)then self:drop();return nil,"world changed during source asset load"end
                local name=asset:GetFullName():match("^%S+%s+(.+)$")
                if not env.same(token)then self:drop();return nil,"world changed during source asset load"end
                if name~=path then return nil,"source render asset identity mismatch"end
                -- LoadAsset/name conversion can change the admitted recipe.
                local current,current_scope=env.native.native_scene_assets()
                if not env.same(token)then self:drop();return nil,"world changed during source asset load"end
                if not current or current_scope~=scope or not matches(cursor.paths,current)then
                    self.loading=nil;return nil,current and "client mirror generation"or current_scope
                end
                cursor.done=cursor.done+1;progress("assets",cursor.done,#cursor.paths)
                if cursor.done<#cursor.paths then return nil,"native scene assets loading"end
            end
            self.key=scope;self.loading=nil
        end
        progress("present")
        local ok,reason=env.native.native_present(world:GetAddress())
        if not env.same(token)then self:drop();return nil,"world changed during native mirror batch"end
        return ok,reason
    end
    return self
end
return M
