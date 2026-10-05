-- DEV ONLY, passive native-input observer. No FKey reconstruction, physical
-- input injection, native outcome calls, or DeathReport writes.
local M={HOLD_S=2.2}
local PREFIX="/Game/Character/Blueprints/Willie_BP.Willie_BP_C:InpActEvt_Talk_K2Node_InputActionEvent_"
M.PRESS,M.RELEASE=PREFIX.."22",PREFIX.."23"
local function same(a,b)
    if not a or not b then return false end
    for _,k in ipairs({"match_id","round","life","pawn","address","world_key"})do
        if a[k]==nil or a[k]~=b[k]then return false end
    end
    return true
end
local function immutable(ctx)
    local r={}
    for _,k in ipairs({"match_id","round","life","pawn","address","world_key"})do r[k]=ctx[k]end
    return r
end
function M.new(env)
    local P={hooks={},state="idle"}
    local function context(actor)
        local ok,ctx=pcall(env.context,actor)
        return ok and ctx or nil
    end
    local function note(kind,data)
        if env.note then pcall(env.note,kind,data)end
    end
    function P:arm()
        if not (self.hooks[M.PRESS] and self.hooks[M.RELEASE])then return false,"native input hooks missing"end
        local ctx=context()
        if not ctx then return false,"no verified current own life"end
        env.outcomes() -- prime away events queued before this deliberate probe
        self.expected=immutable(ctx);self.journal=nil;self.state="armed"
        note("armed",immutable(ctx));return true
    end
    function P:pressed(actor)
        if self.state~="armed"then return end
        local ctx=context(actor)
        if not same(ctx,self.expected)then return end
        self.journal=immutable(ctx);self.journal.at=env.now();self.state="holding"
        note("native_press",immutable(ctx))
    end
    function P:released(actor)
        local j=self.journal
        if not j or self.state~="holding"then return end
        local ok,id=pcall(env.actor_id,actor)
        if not ok or not id or id.pawn~=j.pawn or id.address~=j.address then return end
        j.released_at=env.now();j.elapsed=j.released_at-j.at
        if j.elapsed<M.HOLD_S then self.state="released_early"else self.state="released_complete"end
        note("native_release",{elapsed=j.elapsed,match_id=j.match_id,round=j.round,life=j.life})
    end
    function P:install(register)
        for _,row in ipairs({{M.PRESS,"pressed"},{M.RELEASE,"released"}})do
            local path,method=row[1],row[2]
            if not self.hooks[path]then
                self.hooks[path]=pcall(register,path,function(actor,_borrowed_key)
                    local own=actor;pcall(function()own=actor:get()end)
                    -- Never retain the borrowed FKey argument beyond callback.
                    pcall(self[method],self,own)
                end)
            end
        end
    end
    function P:cancel(why)
        if self.state=="idle"then return end
        self.state="cancelled";self.journal=nil
        note("cancelled",{reason=why or "context changed"})
    end
    function P:tick()
        local j=self.journal
        if not j then
            if self.state=="armed"and not same(context(),self.expected)then self:cancel()end
            return
        end
        -- Authoritative outcome is read-only, original generation checked,
        -- and cannot count an old queued death before this observed press.
        for _,event in ipairs(env.outcomes()or {})do
            local d=event.data or event
            if event.at and event.at>=j.at and d.cause==6 and d.match_id==j.match_id
                and d.round==j.round and d.life==j.life and d.peer_id==env.my_peer_id()then
                if env.now()-j.at<M.HOLD_S then self.state="unexpected_early_outcome"
                elseif j.elapsed and j.elapsed<M.HOLD_S then self.state="unexpected_cancelled_outcome"
                elseif not j.saw_meter then self.state="outcome_without_verified_meter"
                else self.state="verified_surrender"end
                note(self.state,immutable(j));return
            end
        end
        if self.state=="verified_surrender"then return end
        if not same(context(),j)then self:cancel();return end
        local meter=env.meter()
        if meter and meter.active==true and meter.match_id==j.match_id and meter.round==j.round
            and meter.life==j.life and meter.pawn==j.pawn and type(meter.at_ms)=="number"
            and meter.at_ms>=j.at*1000 and meter.at_ms<=env.now()*1000
            and env.now()*1000-meter.at_ms<=1000 then
            j.saw_meter=true
        elseif self.state=="released_early"and meter and meter.active==false then
            self.state="verified_cancel";note("verified_cancel",immutable(j))
        end
    end
    return P
end
return M
