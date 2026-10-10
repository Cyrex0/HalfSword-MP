-- Display-only vitals from the same confirmed scene used by the client.
-- Numeric values have no inferred maximum; this module never samples gameplay.
local M={}
local function finite(value)return type(value)=="number"and value==value and value~=math.huge and value~=-math.huge end
local function integer(value)return type(value)=="number"and math.type(value)=="integer"end
local function vital(value)
    if type(value)~="table"or(value.kind~="f32"and value.kind~="f64")or not finite(value.value)then return nil end
    return string.format(value.kind=="f32"and "%.9g"or "%.17g",value.value)
end
function M.new(env)
    local self={host=nil,snapshot=nil,receipt=nil,failed=false,last_error=nil}
    local serial=0
    local function same(token)return env.WG.same(token)end
    local function checked(token,fn)
        if not same(token)then error("stats world changed",0)end
        local result=fn()
        if not same(token)then error("stats world changed",0)end
        return result
    end
    local function build(token,world,pc)
        serial=serial+1
        local function construct(path,outer,tag)
            local cls=checked(token,function()return env.find(path)end)
            if not cls or not checked(token,function()return cls:IsValid()end)then error("stats widget class unavailable",0)end
            local widget=checked(token,function()return env.construct(cls,outer,env.name("HSMPNativeStats_"..serial.."_"..tag))end)
            if not widget or not checked(token,function()return widget:IsValid()end)then error("stats widget unavailable",0)end
            return widget
        end
        local host={token=token,widget=construct("/Script/UMG.UserWidget",world,"host"),shown=false}
        -- Retain our own partial host for guarded cleanup if construction fails.
        self.host=host
        local tree=construct("/Script/UMG.WidgetTree",host.widget,"tree")
        local canvas=construct("/Script/UMG.CanvasPanel",tree,"canvas")
        checked(token,function()host.widget.WidgetTree=tree end)
        checked(token,function()tree.RootWidget=canvas end)
        checked(token,function()host.widget:SetOwningPlayer(pc)end)
        local text=construct("/Script/UMG.TextBlock",tree,"vitals")
        checked(token,function()text:SetJustification(0)end)
        checked(token,function()local font=text.Font;font.Size=18;text:SetFont(font)end)
        checked(token,function()text:SetColorAndOpacity({SpecifiedColor={R=.95,G=.95,B=.95,A=1},ColorUseRule=0})end)
        checked(token,function()text:SetShadowOffset({X=1,Y=1})end)
        checked(token,function()text:SetShadowColorAndOpacity({R=0,G=0,B=0,A=1})end)
        checked(token,function()text:SetVisibility(3)end)
        local slot=checked(token,function()return canvas:AddChildToCanvas(text)end)
        if not slot then error("stats widget slot unavailable",0)end
        checked(token,function()slot:SetAnchors({Minimum={X=0,Y=0},Maximum={X=1,Y=1}})end)
        checked(token,function()slot:SetOffsets({Left=24,Top=24,Right=24,Bottom=24})end)
        checked(token,function()host.widget:SetVisibility(1)end)
        checked(token,function()host.widget:SetAnchorsInViewport({Minimum={X=0,Y=0},Maximum={X=1,Y=1}})end)
        checked(token,function()host.widget:AddToViewport(100)end)
        host.text=text;return host
    end
    function self:drop()
        -- A dropped world invalidates all widget references without touching them.
        self.host=nil;self.snapshot=nil;self.receipt=nil;self.failed=false;self.last_error=nil
    end
    function self:status(state,scene,own,readiness,sampled_at)
        self.snapshot=nil
        if self.failed or state~="live"or readiness~=true or type(scene)~="table"or scene.state~=2
            or scene.fresh~=true or scene.gameplay_proof~=true or type(own)~="table"
            or type(scene.generation)~="string"or scene.generation==""or not integer(scene.frame_seq)
            or scene.frame_seq<1 or not finite(scene.received_age_ms)or scene.received_age_ms<0 or scene.received_age_ms>250 then return false end
        local now=env.now()
        if not finite(now)or not finite(sampled_at)or sampled_at>now then return false end
        local entities=scene.entities
        if type(entities)~="table"then return false end
        local count=0
        for key in pairs(entities)do
            if not integer(key)or key<1 or key>32 then return false end
            count=count+1
        end
        if count<1 or count>32 then return false end
        local rows,owned,seen={},nil,{}
        for index=1,count do
            local row=entities[index]
            if type(row)~="table"or row.epoch~=scene.epoch or not integer(row.id)or row.id<1
                or not integer(row.incarnation)or row.incarnation<1 or seen[row.id]then return false end
            seen[row.id]=true
            local health,stamina=vital(row.health),vital(row.stamina)
            if not health or not stamina then return false end
            local mine=row.id==own.id and row.incarnation==own.incarnation and row.epoch==own.epoch
            if mine then
                if owned or row.kind~=0 or row.owner_peer~=scene.peer_id then return false end
                owned=index
            end
            rows[index]=(mine and "You"or "Player "..row.id).."  |  Health: "..health.."  |  Stamina: "..stamina
        end
        if not owned then return false end
        -- Put the authenticated human first without changing any source row.
        local lines={rows[owned]};for index=1,count do if index~=owned then lines[#lines+1]=rows[index]end end
        -- The caller samples this same clock immediately before native confirm.
        -- Controller/input callbacks after export cannot renew the copied age.
        local expires=sampled_at+(250-scene.received_age_ms)/1000
        local old=self.receipt
        if old and old.generation==scene.generation then
            if scene.frame_seq<old.frame then return false end
            if scene.frame_seq==old.frame then expires=math.min(expires,old.expires)end
        end
        self.receipt={generation=scene.generation,frame=scene.frame_seq,expires=expires}
        self.snapshot={text=table.concat(lines,"\n"),expires=expires,at=now,token=env.WG.token()}
        return true
    end
    function self:clear()
        self.snapshot=nil;self.receipt=nil
        if not self.host then return true end
        local host=self.host
        if not same(host.token)then self:drop();return true end
        local ok,why=pcall(checked,host.token,function()host.widget:RemoveFromParent()end)
        if not ok then return nil,why end
        self.host=nil;return true
    end
    function self:tick()
        if not env.WG.check()then self:drop();return false end
        local token=env.WG.token()
        if self.host and not same(self.host.token)then self:drop()end
        if self.failed then return nil,self.last_error end
        local snapshot=self.snapshot
        local function eligible()
            local now=env.now()
            return snapshot and finite(now)and now>=snapshot.at and now<snapshot.expires and same(snapshot.token)
        end
        local show=eligible()
        if not show then self.snapshot=nil end
        local ok,why=pcall(function()
            if not self.host then
                if not show then return false end
                local world,pc=env.WG.world(),env.WG.pc()
                if not world or not pc or not same(token)then return false end
                build(token,world,pc)
            end
            local host=self.host
            show=eligible()
            if show and host.text_value~=snapshot.text then
                checked(host.token,function()host.text:SetText(env.text(snapshot.text))end)
                host.text_value=snapshot.text
            end
            show=eligible()
            if host.shown~=not not show then
                checked(host.token,function()host.widget:SetVisibility(show and 3 or 1)end)
                host.shown=not not show
            end
            return not not show
        end)
        if not ok then
            if not same(token)then self:drop();return false end
            self.failed=true;self.snapshot=nil;self.last_error="stats view unavailable: "..tostring(why):sub(1,160)
            -- Clear labels if a same-world update fails. Never keep ready stats.
            if self.host then pcall(checked,token,function()self.host.widget:SetVisibility(1)end)end
            if env.log then env.log("%s",self.last_error)end
            return nil,self.last_error
        end
        return why
    end
    return self
end
return M
