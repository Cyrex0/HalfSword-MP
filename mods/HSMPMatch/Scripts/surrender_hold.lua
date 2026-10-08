-- Explicit native Talk hold only; journals contain plain original identities.
local M = { HOLD_S = 2.2 }
M.PRESS = "/Game/Character/Blueprints/Willie_BP.Willie_BP_C:InpActEvt_Talk_K2Node_InputActionEvent_22"
M.RELEASE = "/Game/Character/Blueprints/Willie_BP.Willie_BP_C:InpActEvt_Talk_K2Node_InputActionEvent_23"
local keys = {"match_id","round","life","pawn","address","world","world_key","spawn_id"}
local function same(a,b)
    if not a or not b then return false end
    for _,k in ipairs(keys) do if a[k]~=b[k] then return false end end
    return true
end
-- Shared production adapter; world guard keys are strings, never UObject addresses.
function M.make_context(env)
    return function(input_pawn)
        if not env.ready() then return nil end
        local v,mode=env.view(),env.mode()
        local peer=v and v.my_peer_id
        local row=mode and mode.rows and mode.rows[peer]
        if not v or v.status~="connected" or v.state~="live" or (v.match_id or 0)<=0 or (v.round or 0)<=0
            or not v.alive or v.alive[peer]~=true
            or not mode or mode.match_id~=v.match_id or mode.round~=v.round
            or not row or row.alive~=true or row.respawning or (row.life or 0)<1 then return nil end
        local p,st=env.pawn(),env.status()
        local order=v.spawns and v.spawns[peer]
        if not p or not p:IsValid() or not st or st.verified~=true or not order
            or (order.spawn_id or 0)==0 or not env.world_key() then return nil end
        local name,address=p:GetFName():ToString(),p:GetAddress()
        if input_pawn and (not input_pawn:IsValid() or input_pawn:GetAddress()~=address
            or input_pawn:GetFName():ToString()~=name) then return nil end
        local world,current=p:GetWorld(),env.world()
        if not world or not world:IsValid() or not current or not current:IsValid()
            or world:GetAddress()~=current:GetAddress()
            or world:GetFullName()~=current:GetFullName() then return nil end
        if st.pawn~=name or st.match_id~=v.match_id or st.round~=v.round or st.life~=row.life
            or st.spawn_id~=order.spawn_id then return nil end
        return {match_id=st.match_id,round=st.round,life=st.life,spawn_id=st.spawn_id,
            pawn=name,address=address,world=current:GetFullName(),world_key=env.world_key()}
    end
end
function M.new(env)
    local S = { down=false, journal=nil, submitted=nil, hooked={} }
    local function context(actor)
        local ok,value=pcall(env.context,actor)
        return ok and value or nil
    end
    function S:publish(j)
        if not env.publish then return end
        local now=env.now()
        local progress=j and math.max(0,math.min(1,(now-j.at)/M.HOLD_S)) or 0
        pcall(env.publish,{match_id=j and j.match_id or 0,round=j and j.round or 0,
            life=j and j.life or 0,pawn=j and j.pawn or "",active=j~=nil,
            progress=progress,remaining_s=j and M.HOLD_S*(1-progress) or 0,at_ms=now*1000})
    end
    function S:cancel() self.journal=nil;self:publish(nil) end
    function S:input(actor,press)
        if not press then self.down=false; self:cancel(); return end
        if self.down then return end -- key repeats do not restart or rearm a cancelled hold
        self.down=true
        local ctx=context(actor)
        if not ctx or same(ctx,self.submitted) then return end
        local j={at=env.now()}
        for _,k in ipairs(keys) do j[k]=ctx[k] end
        self.journal=j
        self:publish(j)
    end
    function S:tick()
        local ctx=context()
        if self.submitted and not same(ctx,self.submitted) then self.submitted=nil end
        local j=self.journal
        if j and not same(ctx,j) then self:cancel(); j=nil end
        if j and self.down and env.now()-j.at>=M.HOLD_S then
            if env.send({match_id=j.match_id,round=j.round,life=j.life,reason=2}) then
                self.submitted=j;self.journal=nil
                self:publish(nil)
                if env.log then env.log("manual surrender held %.1f s: match=%s round=%s life=%s pawn=%s",
                    M.HOLD_S,tostring(j.match_id),tostring(j.round),tostring(j.life),j.pawn) end
            end
        end
        if self.journal then self:publish(self.journal) end
    end
    function S:hooks(register)
        for _,row in ipairs({{M.PRESS,true},{M.RELEASE,false}}) do
            local path,press=row[1],row[2]
            if not self.hooked[path] then
                local ok=pcall(register,path,function(actor)
                    if press and not (self.hooked[M.PRESS] and self.hooked[M.RELEASE]) then return end
                    local value=actor
                    pcall(function()value=actor:get()end)
                    if env.accept_actor then
                        local accepted,own=pcall(env.accept_actor,value)
                        if not accepted or not own then return end
                    end
                    self:input(value,press)
                end)
                self.hooked[path]=ok
            end
        end
    end
    return S
end
return M
