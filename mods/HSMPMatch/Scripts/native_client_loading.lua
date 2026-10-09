-- Native-client loading view. Progress represents completed preparation work;
-- server transfer and unknown view readiness never receive a timer percentage.
local M={}
local labels={connecting="Connecting to your match",travel="Loading the arena",waiting="Waiting for the match",
    assets="Preparing the match",present="Getting players ready",view="Preparing your view",error="Unable to load the match"}
function M.new(env)
    local self={host=nil,stage="connecting",done=nil,total=nil,failed=false,ready=false,retry_at=0,failures=0}
    local serial=0
    local function same(token)return env.WG.same(token)end
    local function checked(token,fn)
        if not same(token)then error("loading world changed",0)end
        local value=fn()
        if not same(token)then error("loading world changed",0)end
        return value
    end
    local function fault(token,why)
        if not same(token)then self:drop();return false end
        local reason="loading view unavailable: "..tostring(why):sub(1,192)
        if reason~=self.last_error and env.log then env.log("%s",reason)end
        self.last_error=reason;self.failures=self.failures+1;self.retry_at=env.now()+1;self:fail()
        return nil,reason
    end
    local function build(token,world,pc)
        serial=serial+1
        local function construct(path,outer,tag)
            local cls=checked(token,function()return env.find(path)end)
            if not cls or not checked(token,function()return cls:IsValid()end)then error("loading widget class unavailable",0)end
            local o=checked(token,function()return env.construct(cls,outer,env.name("HSMPLoading_"..serial.."_"..tag))end)
            if not o or not checked(token,function()return o:IsValid()end)then error("loading widget unavailable",0)end
            return o
        end
        local uw=construct("/Script/UMG.UserWidget",world,"host")
        local tree=construct("/Script/UMG.WidgetTree",uw,"tree")
        local canvas=construct("/Script/UMG.CanvasPanel",tree,"canvas")
        checked(token,function()uw.WidgetTree=tree end)
        checked(token,function()tree.RootWidget=canvas end)
        checked(token,function()uw:SetOwningPlayer(pc)end)
        local function place(w,x,y,width,height,fill)
            local slot=checked(token,function()return canvas:AddChildToCanvas(w)end)
            if not slot then error("loading widget slot unavailable",0)end
            checked(token,function()slot:SetAnchors({Minimum={X=fill and 0 or 0.5,Y=fill and 0 or 0.5},Maximum={X=fill and 1 or 0.5,Y=fill and 1 or 0.5}})end)
            if fill then checked(token,function()slot:SetOffsets({Left=0,Top=0,Right=0,Bottom=0})end)
            else
                checked(token,function()slot:SetAlignment({X=0,Y=0})end)
                checked(token,function()slot:SetPosition({X=x,Y=y})end)
                checked(token,function()slot:SetSize({X=width,Y=height})end)
            end
            checked(token,function()w:SetVisibility(3)end)
        end
        local backing=construct("/Script/UMG.Border",tree,"backing")
        checked(token,function()backing:SetBrushColor({R=0.012,G=0.015,B=0.022,A=1})end)
        place(backing,0,0,0,0,true)
        local function text(tag,label,y,size)
            local w=construct("/Script/UMG.TextBlock",tree,tag)
            checked(token,function()w:SetText(env.text(label))end)
            checked(token,function()w:SetJustification(1)end)
            checked(token,function()local font=w.Font;font.Size=size;w:SetFont(font)end)
            checked(token,function()w:SetColorAndOpacity({SpecifiedColor={R=0.92,G=0.94,B=0.97,A=1},ColorUseRule=0})end)
            place(w,-320,y,640,52,false);return w
        end
        text("title","HALF SWORD MULTIPLAYER",-116,36)
        local stage=text("stage",labels[self.stage],-38,24)
        local detail=text("detail","",32,18)
        local bar=construct("/Script/UMG.ProgressBar",tree,"progress")
        checked(token,function()bar:SetFillColorAndOpacity({R=0.86,G=0.69,B=0.35,A=1})end)
        place(bar,-270,16,540,12,false)
        -- Build the opaque canvas before putting any part of it on screen.
        checked(token,function()uw:SetVisibility(3)end)
        checked(token,function()uw:SetAnchorsInViewport({Minimum={X=0,Y=0},Maximum={X=1,Y=1}})end)
        checked(token,function()uw:AddToViewport(10000)end)
        return {token=token,widget=uw,stage=stage,detail=detail,bar=bar}
    end
    function self:drop()
        self.host=nil;self.ready=false;self.done,self.total=nil,nil
        self.retry_at=0;self.failures=0;self.last_error=nil
        if not self.failed then self.stage="waiting"end
    end -- world drop never touches old UObjects
    function self:set(stage,done,total)
        if self.failed then return end
        if self.ready and stage=="present"then return end
        self.stage=labels[stage]and stage or "waiting";self.done,self.total=nil,nil;self.ready=false
        if stage=="assets"and type(done)=="number"and math.type(done)=="integer"and type(total)=="number"
            and math.type(total)=="integer"and total>0 and done>=0 and done<=total then self.done,self.total=done,total end
    end
    function self:status(state,scene,own)
        if state=="error"or state=="stopped"then self:fail();return end
        if self.failed then return end
        if state=="live"or state=="mirror_ready"then
            self:set("view")
            -- This seam needs an independently verified owned camera. There is
            -- no guessed native view API and LIVE alone cannot uncover the arena.
            if state=="live"and scene and scene.fresh==true and type(env.view_ready)=="function"then
                local token=env.WG.token()
                local ok,value=pcall(checked,token,function()return env.view_ready(scene,own)end)
                self.ready=ok and value==true
            end
        elseif state=="travel"then self:set("travel")
        elseif state=="boot"or state=="connected"then self:set("connecting")
        elseif self.stage~="assets"then self:set("waiting")end
    end
    function self:fail()self.failed=true;self.ready=false;self.stage="error";self.done,self.total=nil,nil end
    function self:tick()
        if not env.WG.check()then return false end
        local token=env.WG.token()
        if self.host and not same(self.host.token)then self:drop()end
        if self.last_error and(self.failures>=3 or env.now()<self.retry_at)then return nil,self.last_error end
        if self.ready then
            if self.host then
                local host=self.host
                local ok,why=pcall(checked,host.token,function()host.widget:RemoveFromParent()end)
                if not ok then return fault(host.token,why)end
                self.host=nil
            end
            return true
        end
        local ok,why=pcall(function()
            if not self.host then
                local world=env.WG.world();if not world or not same(token)then return false end
                local pc=env.WG.pc();if not pc or not same(token)then return false end
                self.host=build(token,world,pc)
            end
            local host=self.host
            local detail=self.failed and "Close and join again to retry."or self.total and string.format("Preparation: %d / %d",self.done,self.total)
                or self.stage=="waiting"and "Waiting for the server. Progress is not available yet."or "Please wait."
            local signature=self.stage..":"..detail
            if host.signature~=signature then
                checked(host.token,function()host.stage:SetText(env.text(labels[self.stage]))end)
                checked(host.token,function()host.detail:SetText(env.text(detail))end)
                checked(host.token,function()host.bar:SetIsMarquee(self.total==nil)end)
                if self.total then checked(host.token,function()host.bar:SetPercent(self.done/self.total)end)end
                host.signature=signature
            end
            return true
        end)
        if not ok then
            return fault(token,why)
        end
        if why==true then self.last_error=nil;self.retry_at=0 end
        return ok and why==true
    end
    return self
end
return M
